//! 全内存 HTTP 缓存：强缓存 + 协商缓存 + LRU 淘汰。
//!
//! 阶段四：缓存主文档与子资源，减少重复网络请求。
//! 全内存驻留，进程退出即清空；大小上限默认 128MB，超限按 LRU 淘汰。
//!
//! 模块分工：`mod.rs` 缓存主结构 + LRU；`policy.rs` Cache-Control/Expires 解析；
//! `validation.rs` ETag/Last-Modified 协商。

mod policy;
mod validation;

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

pub use policy::{parse_cache_control, CachePolicy};
pub use validation::{Validator, ValidationResult};

/// 默认缓存大小上限：128MB。
pub const DEFAULT_CACHE_MAX_SIZE: usize = 128 * 1024 * 1024;

/// 缓存条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// 请求 URL。
    pub url: String,
    /// HTTP 状态码。
    pub status: u16,
    /// 响应头。
    pub headers: Vec<(String, String)>,
    /// 响应体。
    pub bytes: Vec<u8>,
    /// 缓存时间（Unix 毫秒）。
    pub cached_at_ms: u128,
    /// 强缓存过期时间（Unix 毫秒）；None 表示无强缓存。
    pub expires_ms: Option<u128>,
    /// ETag（协商缓存）。
    pub etag: Option<String>,
    /// Last-Modified（协商缓存）。
    pub last_modified: Option<String>,
    /// 访问计数（统计用）。
    pub hits: u64,
}

impl CacheEntry {
    /// 从响应头构建缓存条目。
    pub fn new(
        url: String,
        status: u16,
        headers: Vec<(String, String)>,
        bytes: Vec<u8>,
        cached_at_ms: u128,
    ) -> Self {
        let cc = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("cache-control"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        let policy = parse_cache_control(cc);
        let expires_ms = policy.expires_at_ms();
        let etag = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
            .map(|(_, v)| v.clone());
        let last_modified = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("last-modified"))
            .map(|(_, v)| v.clone());
        Self {
            url,
            status,
            headers,
            bytes,
            cached_at_ms,
            expires_ms,
            etag,
            last_modified,
            hits: 0,
        }
    }

    /// 是否已过期（强缓存）。
    pub fn is_expired(&self) -> bool {
        match self.expires_ms {
            Some(exp) => self.cached_at_ms > exp,
            None => false,
        }
    }

    /// 是否有强缓存（Cache-Control/Expires）。
    pub fn has_strong_cache(&self) -> bool {
        self.expires_ms.is_some()
    }

    /// 是否有协商缓存验证器（ETag/Last-Modified）。
    pub fn has_validator(&self) -> bool {
        self.etag.is_some() || self.last_modified.is_some()
    }
}

/// 缓存统计。
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// 命中次数。
    pub hits: u64,
    /// 未命中次数。
    pub misses: u64,
    /// 当前缓存大小（字节）。
    pub size_bytes: usize,
    /// 条目数量。
    pub entry_count: usize,
}

/// 全内存 HTTP 缓存。
#[derive(Debug)]
pub struct HttpCache {
    /// 缓存条目：URL → CacheEntry。
    entries: HashMap<String, CacheEntry>,
    /// LRU 访问顺序（最近访问在尾部）。
    lru: VecDeque<String>,
    /// 当前缓存大小（字节）。
    size: usize,
    /// 大小上限（字节）。
    max_size: usize,
    /// 命中次数。
    hits: u64,
    /// 未命中次数。
    misses: u64,
}

impl Default for HttpCache {
    fn default() -> Self {
        Self::new(DEFAULT_CACHE_MAX_SIZE)
    }
}

impl HttpCache {
    /// 创建指定大小上限的缓存。
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: HashMap::new(),
            lru: VecDeque::new(),
            size: 0,
            max_size,
            hits: 0,
            misses: 0,
        }
    }

    /// 尝试从缓存获取。返回 None 表示未命中或已过期。
    pub fn get(&mut self, url: &str) -> Option<&CacheEntry> {
        // 先检查是否存在
        let entry = self.entries.get(url);
        match entry {
            None => {
                self.misses += 1;
                return None;
            }
            Some(e) if e.is_expired() => {
                self.remove(url);
                self.misses += 1;
                return None;
            }
            Some(_) => {}
        }
        // 命中：更新 LRU 顺序 + 计数
        self.hits += 1;
        // 更新 LRU：先移除再插入尾部
        if let Some(pos) = self.lru.iter().position(|u| u == url) {
            self.lru.remove(pos);
        }
        self.lru.push_back(url.to_string());
        self.entries.get(url)
    }

    /// 存入缓存（LRU 淘汰）。
    pub fn put(&mut self, entry: CacheEntry) {
        let url = entry.url.clone();
        let entry_size = entry.bytes.len();
        // 如果已存在，先移除旧条目
        if self.entries.contains_key(&url) {
            self.remove(&url);
        }
        // 如果单条超过上限，不缓存
        if entry_size > self.max_size {
            return;
        }
        // LRU 淘汰：腾出空间
        while self.size + entry_size > self.max_size && !self.lru.is_empty() {
            if let Some(oldest) = self.lru.pop_front() {
                self.remove(&oldest);
            }
        }
        // 插入新条目
        self.size += entry_size;
        self.entries.insert(url.clone(), entry);
        self.lru.push_back(url);
    }

    /// 移除指定 URL 的缓存条目。
    pub fn remove(&mut self, url: &str) -> Option<CacheEntry> {
        if let Some(entry) = self.entries.remove(url) {
            self.size = self.size.saturating_sub(entry.bytes.len());
            if let Some(pos) = self.lru.iter().position(|u| u == url) {
                self.lru.remove(pos);
            }
            Some(entry)
        } else {
            None
        }
    }

    /// 清空缓存。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.size = 0;
    }

    /// 缓存统计。
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            size_bytes: self.size,
            entry_count: self.entries.len(),
        }
    }

    /// 当前缓存大小（字节）。
    pub fn size_bytes(&self) -> usize {
        self.size
    }

    /// 条目数量。
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(url: &str, bytes: &[u8]) -> CacheEntry {
        CacheEntry::new(
            url.to_string(),
            200,
            vec![("content-type".to_string(), "text/html".to_string())],
            bytes.to_vec(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
    }

    #[test]
    fn cache_put_get_命中() {
        let mut cache = HttpCache::new(1024);
        let entry = make_entry("http://example.com/", b"hello");
        cache.put(entry);
        assert!(cache.get("http://example.com/").is_some());
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 0);
    }

    #[test]
    fn cache_get_未命中() {
        let mut cache = HttpCache::new(1024);
        assert!(cache.get("http://example.com/").is_none());
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn cache_lru_淘汰() {
        let mut cache = HttpCache::new(10);
        let e1 = make_entry("http://a.com/", b"12345");
        let e2 = make_entry("http://b.com/", b"67890");
        let e3 = make_entry("http://c.com/", b"abcde");
        cache.put(e1);
        cache.put(e2);
        cache.put(e3); // 应淘汰 a
        assert!(cache.get("http://a.com/").is_none());
        assert!(cache.get("http://b.com/").is_some());
        assert!(cache.get("http://c.com/").is_some());
    }

    #[test]
    fn cache_remove() {
        let mut cache = HttpCache::new(1024);
        let entry = make_entry("http://example.com/", b"hello");
        cache.put(entry);
        assert!(cache.remove("http://example.com/").is_some());
        assert!(cache.get("http://example.com/").is_none());
    }

    #[test]
    fn cache_clear() {
        let mut cache = HttpCache::new(1024);
        cache.put(make_entry("http://a.com/", b"1"));
        cache.put(make_entry("http://b.com/", b"2"));
        cache.clear();
        assert_eq!(cache.size_bytes(), 0);
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn cache_过期条目() {
        let mut entry = make_entry("http://example.com/", b"hello");
        // 手动设置过期时间为过去
        entry.expires_ms = Some(0);
        let mut cache = HttpCache::new(1024);
        cache.put(entry);
        // 过期条目应被移除
        assert!(cache.get("http://example.com/").is_none());
    }

    #[test]
    fn cache_单条超上限() {
        let mut cache = HttpCache::new(5);
        let entry = make_entry("http://example.com/", b"1234567890");
        cache.put(entry);
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn cache_stats() {
        let mut cache = HttpCache::new(1024);
        cache.put(make_entry("http://a.com/", b"1"));
        cache.get("http://a.com/");
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.entry_count, 1);
        assert!(stats.size_bytes > 0);
    }
}
