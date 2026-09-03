//! Session 管理:本机指纹 + Session ID 生成 + 独立对话上下文。
//!
//! 一个 [`Session`] 拥有独立的 `context`(对话历史),Session ID 在创建时生成,
//! 同一 TUI 进程内保持不变,仅 `/new` 或 `/clear` 时重置。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::llm::ChatMessage;
use crate::llm::RequestMeta;

/// 本机指纹(sha256 hex,64 位),进程生命周期内缓存。
///
/// 输入素材优先级:`/etc/machine-id` → `hostname + username`,拼接后 sha256。
pub fn device_id() -> &'static str {
    static CELL: OnceLock<String> = OnceLock::new();
    CELL.get_or_init(compute_device_id)
}

fn compute_device_id() -> String {
    let mut input = String::new();

    // 1) /etc/machine-id(Linux 标准机器唯一 ID)
    if let Ok(s) = std::fs::read_to_string("/etc/machine-id") {
        let s = s.trim();
        if !s.is_empty() {
            input.push_str(s);
        }
    }
    // 2) hostname
    if let Ok(out) = std::process::Command::new("hostname").output() {
        if let Ok(s) = String::from_utf8(out.stdout) {
            input.push('|');
            input.push_str(s.trim());
        }
    }
    // 3) 用户名
    if let Ok(s) = std::env::var("USER") {
        input.push('|');
        input.push_str(&s);
    }

    if input.is_empty() {
        // 兜底:用启动时间 + pid,保证进程间不同
        let pid = std::process::id();
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        input = format!("fallback-{pid}-{nanos}");
    }

    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 生成 Session ID:`{YYYYMMDD-HHmmss}-{本机指纹前8位}-{Unix毫秒}-{6位随机hex}`。
pub fn generate_session_id(device: &str) -> String {
    let prefix = device.get(..8).unwrap_or(device);

    // 可读时间前缀(本地时区,失败退化为 UTC)
    let now = SystemTime::now();
    let (secs, subsec_millis) = match now.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs(), d.subsec_millis()),
        Err(_) => (0, 0),
    };
    let readable = format_readable(secs, subsec_millis);

    // Unix 毫秒时间戳(单调)
    let millis = secs.saturating_mul(1000).saturating_add(u64::from(subsec_millis));

    // 随机成分:nanos ^ pid ^ 原子计数器,再哈希取 6 位 hex
    let rand_part = {
        let pid = std::process::id() as u64;
        let counter = AtomicU64::new(0);
        let c = counter.fetch_add(1, Ordering::SeqCst);
        let nanos = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let seed = nanos ^ pid ^ c ^ millis;
        let mut hasher = Sha256::new();
        hasher.update(seed.to_le_bytes());
        let bytes = hasher.finalize();
        // 取前 3 字节 → 6 位 hex
        format!("{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
    };

    format!("{readable}-{prefix}-{millis}-{rand_part}")
}

/// 当前本地可读时间 `YYYY-MM-DD HH:MM:SS`。
pub(crate) fn now_readable() -> String {
    let now = SystemTime::now();
    let secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_readable(secs, 0)
}

/// 将 Unix 秒转为 `YYYYMMDD-HHmmss` 格式。优先本地时区,失败退化为 UTC。
fn format_readable(secs: u64, _subsec: u32) -> String {
    let base = time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let dt = match time::UtcOffset::current_local_offset() {
        Ok(offset) => base.to_offset(offset),
        Err(_) => base,
    };
    dt.format(
        &time::format_description::parse("[year][month][day]-[hour][minute][second]")
            .expect("format"),
    )
    .expect("fmt")
}

/// 一个会话:独立 Session ID + 独立对话上下文。
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub device_id: String,
    pub created_at: String,
    pub context: Vec<ChatMessage>,
}

impl Session {
    /// 创建新 Session(生成新 ID 与空上下文)。
    pub fn new() -> Self {
        let device = device_id().to_string();
        let id = generate_session_id(&device);
        let created_at = now_readable();
        Self {
            id,
            device_id: device,
            created_at,
            context: Vec::new(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn device_id_str(&self) -> &str {
        &self.device_id
    }

    pub fn context(&self) -> &Vec<ChatMessage> {
        &self.context
    }

    pub fn context_mut(&mut self) -> &mut Vec<ChatMessage> {
        &mut self.context
    }

    /// 构造协议层所需的请求元数据。
    pub fn meta(&self) -> RequestMeta {
        RequestMeta {
            session_id: self.id.clone(),
            device_id: self.device_id.clone(),
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_is_64_hex_chars() {
        let d = device_id();
        assert_eq!(d.len(), 64, "device_id 应为 64 位 hex,实际 {d}");
        assert!(d.chars().all(|c| c.is_ascii_hexdigit()), "device_id 应全为 hex");
    }

    #[test]
    fn device_id_stable_within_process() {
        assert_eq!(device_id(), device_id());
    }

    #[test]
    fn session_id_format() {
        let s = Session::new();
        let parts: Vec<&str> = s.id.split('-').collect();
        // YYYYMMDD-HHmmss-prefix-millis-rand → 5 段
        assert_eq!(parts.len(), 5, "Session ID 应为 5 段: {}", s.id);
        assert_eq!(parts[0].len(), 8, "日期段 8 位"); // YYYYMMDD
        assert_eq!(parts[1].len(), 6, "时间段 6 位"); // HHmmss
        assert_eq!(parts[2].len(), 8, "指纹段 8 位");
        assert!(parts[3].parse::<u64>().is_ok(), "毫秒段为数字");
        assert_eq!(parts[4].len(), 6, "随机段 6 位 hex");
    }

    #[test]
    fn two_sessions_have_distinct_ids() {
        let a = Session::new();
        let b = Session::new();
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn meta_exposes_session_and_device() {
        let s = Session::new();
        let m = s.meta();
        assert_eq!(m.session_id, s.id);
        assert_eq!(m.device_id, s.device_id);
    }
}
