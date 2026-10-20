//! 资源加载器：管理页面所有子资源的加载、缓存与优先级。
//!
//! 阶段四：统一加载图片、字体、CSS @import 等子资源。
//! 模块分工：`mod.rs` 加载器主结构；`font.rs` 字体加载与解码；
//! `import.rs` CSS @import 递归；`scheduler.rs` 优先级调度。
//!
//! 全内存架构：资源缓存驻留内存，进程退出即清空。

mod font;
mod import;
mod scheduler;

use std::collections::HashMap;
use std::sync::Arc;

pub use font::{decode_font, is_font_content_type, FontResource, MAX_FONT_BYTES};
pub use import::{has_import, resolve_imports, MAX_IMPORT_DEPTH};
pub use scheduler::{ResourcePriority, ResourceScheduler};

/// 已加载资源。
#[derive(Debug, Clone)]
pub struct LoadedResource {
    /// 资源 URL。
    pub url: String,
    /// Content-Type。
    pub content_type: String,
    /// 原始字节。
    pub bytes: Vec<u8>,
    /// 字体对象（仅字体资源）。
    pub font: Option<Arc<fontdue::Font>>,
    /// 加载耗时（毫秒）。
    pub elapsed_ms: u64,
}

/// 资源加载器。
#[derive(Debug)]
pub struct ResourceLoader {
    /// 已加载资源缓存：URL → LoadedResource。
    resources: HashMap<String, Arc<LoadedResource>>,
    /// 加载中的请求（用于去重）。
    inflight: HashMap<String, Arc<tokio::sync::Semaphore>>,
    /// 已提交集合（含加载中与排队中，用于去重）。
    submitted: std::collections::HashSet<String>,
    /// 调度器。
    scheduler: ResourceScheduler,
    /// 并发信号量。
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl Default for ResourceLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceLoader {
    /// 创建默认资源加载器（并发上限 6）。
    pub fn new() -> Self {
        let concurrency = ResourcePriority::DEFAULT_CONCURRENCY;
        Self {
            resources: HashMap::new(),
            inflight: HashMap::new(),
            submitted: std::collections::HashSet::new(),
            scheduler: ResourceScheduler::new(concurrency),
            semaphore: Arc::new(tokio::sync::Semaphore::new(concurrency)),
        }
    }

    /// 创建指定并发上限的资源加载器。
    pub fn with_concurrency(concurrency: usize) -> Self {
        Self {
            resources: HashMap::new(),
            inflight: HashMap::new(),
            submitted: std::collections::HashSet::new(),
            scheduler: ResourceScheduler::new(concurrency),
            semaphore: Arc::new(tokio::sync::Semaphore::new(concurrency.max(1))),
        }
    }

    /// 尝试从缓存获取资源。
    pub fn get(&self, url: &str) -> Option<Arc<LoadedResource>> {
        self.resources.get(url).cloned()
    }

    /// 提交资源加载请求（进入调度队列）。
    pub fn submit(&mut self, url: String, priority: ResourcePriority) {
        // 已在缓存、加载中或已排队，跳过
        if self.resources.contains_key(&url)
            || self.inflight.contains_key(&url)
            || self.submitted.contains(&url)
        {
            return;
        }
        self.submitted.insert(url.clone());
        self.scheduler.submit(url, priority);
    }

    /// 是否可以开始下一个请求。
    pub fn can_load(&self) -> bool {
        self.scheduler.can_start()
    }

    /// 取出下一个要加载的 URL。
    pub fn next_url(&mut self) -> Option<String> {
        self.scheduler.next()
    }

    /// 标记加载完成（释放并发槽位）。
    pub fn finish_load(&mut self) {
        self.scheduler.finish();
    }

    /// 存入已加载资源。
    pub fn put(&mut self, resource: LoadedResource) {
        let url = resource.url.clone();
        self.resources.insert(url, Arc::new(resource));
    }

    /// 清空缓存。
    pub fn clear(&mut self) {
        self.resources.clear();
        self.inflight.clear();
    }

    /// 缓存资源数量。
    pub fn cached_count(&self) -> usize {
        self.resources.len()
    }

    /// 等待队列长度。
    pub fn pending_count(&self) -> usize {
        self.scheduler.pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loader_get_未命中() {
        let loader = ResourceLoader::new();
        assert!(loader.get("http://example.com/").is_none());
    }

    #[test]
    fn loader_put_get_命中() {
        let mut loader = ResourceLoader::new();
        let res = LoadedResource {
            url: "http://example.com/style.css".to_string(),
            content_type: "text/css".to_string(),
            bytes: b"body {}".to_vec(),
            font: None,
            elapsed_ms: 10,
        };
        loader.put(res);
        let cached = loader.get("http://example.com/style.css").unwrap();
        assert_eq!(cached.content_type, "text/css");
        assert_eq!(loader.cached_count(), 1);
    }

    #[test]
    fn loader_submit_去重() {
        let mut loader = ResourceLoader::new();
        loader.submit("http://a.com/".to_string(), ResourcePriority::Document);
        loader.submit("http://a.com/".to_string(), ResourcePriority::Document);
        assert_eq!(loader.pending_count(), 1);
    }

    #[test]
    fn loader_clear() {
        let mut loader = ResourceLoader::new();
        loader.put(LoadedResource {
            url: "http://example.com/".to_string(),
            content_type: "text/html".to_string(),
            bytes: vec![],
            font: None,
            elapsed_ms: 0,
        });
        assert_eq!(loader.cached_count(), 1);
        loader.clear();
        assert_eq!(loader.cached_count(), 0);
    }

    #[test]
    fn loader_concurrency() {
        let loader = ResourceLoader::with_concurrency(3);
        assert_eq!(loader.semaphore.available_permits(), 3);
    }
}
