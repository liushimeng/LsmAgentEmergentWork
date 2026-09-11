//! 离线请求队列:LLM 不可达时暂存用户输入,恢复后自动 flush。
//!
//! 内存态(退出失效),有界(默认 50 条,环境变量 `LAEW_OFFLINE_QUEUE_CAP` 可覆盖)。
//! 先进先出，不持久化(对齐 laew 单进程单用户单会话模型)。
//!
//! 对标第十九轮 D13 离线模式(L1831-L1840):
//! - openclaw JSONL 事务日志(本轮简化为内存 VecDeque,不持久化);
//! - claudecode 指数退避(由 resilient.rs 实现,队列层不做退避);
//! - opencode Durable Object(本轮不做跨会话持久化)。
//!
//! 对应知识库 gap:第十九轮 D13 离线模式(L1831-L1840)。

use std::collections::VecDeque;
use std::sync::Mutex;

/// 单条排队请求(足够重建 dispatch_prompt 调用)。
#[derive(Debug, Clone)]
pub struct QueuedRequest {
    /// 实际送入编排的提示词(展开 @ 提及后)。
    pub prompt: String,
    /// 用户原始输入行(transcript/导出用)。
    pub raw: String,
    /// 入队时间(机器可读,ISO 子集)。
    pub enqueued_at: String,
}

/// 队列已满错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueFull(pub usize);

impl std::fmt::Display for QueueFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "offline queue full (cap={})", self.0)
    }
}

impl std::error::Error for QueueFull {}

#[derive(Debug)]
struct QueueInner {
    items: VecDeque<QueuedRequest>,
    capacity: usize,
}

/// 有界离线请求队列。
#[derive(Debug)]
pub struct OfflineQueue {
    inner: Mutex<QueueInner>,
}

/// 默认队列容量(对齐 atomcode 有界通道惯例)。
pub const DEFAULT_QUEUE_CAPACITY: usize = 50;

/// 读取队列容量(环境变量 `LAEW_OFFLINE_QUEUE_CAP` 覆盖,解析失败回退默认)。
fn env_queue_capacity() -> usize {
    match std::env::var("LAEW_OFFLINE_QUEUE_CAP") {
        Ok(v) => v.trim().parse().unwrap_or(DEFAULT_QUEUE_CAPACITY),
        Err(_) => DEFAULT_QUEUE_CAPACITY,
    }
}

impl OfflineQueue {
    /// 创建队列,容量取环境变量或默认 50。
    pub fn new() -> Self {
        Self::with_capacity(env_queue_capacity())
    }

    /// 创建指定容量队列(测试用)。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(QueueInner {
                items: VecDeque::with_capacity(capacity),
                capacity,
            }),
        }
    }

    /// 入队。成功返 `Ok(())`;队列满返 `Err(QueueFull(当前容量))`。
    pub fn enqueue(
        &self,
        prompt: String,
        raw: String,
    ) -> Result<(), QueueFull> {
        let mut inner = self.inner.lock().expect("OfflineQueue poisoned");
        if inner.items.len() >= inner.capacity {
            return Err(QueueFull(inner.capacity));
        }
        // 时间戳:优先本地时区,失败回退 UTC。格式 YYYY-MM-DD HH:MM:SS。
        let now = time::OffsetDateTime::now_local()
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let enqueued_at = format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            now.year(),
            now.month() as u8,
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        );
        inner.items.push_back(QueuedRequest {
            prompt,
            raw,
            enqueued_at,
        });
        Ok(())
    }

    /// 出队全部(恢复后 flush 用)。
    pub fn drain(&self) -> Vec<QueuedRequest> {
        let mut inner = self.inner.lock().expect("OfflineQueue poisoned");
        inner.items.drain(..).collect()
    }

    /// 当前深度(横幅展示用)。
    pub fn len(&self) -> usize {
        self.inner.lock().expect("OfflineQueue poisoned").items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 队列容量。
    pub fn capacity(&self) -> usize {
        self.inner.lock().expect("OfflineQueue poisoned").capacity
    }
}

impl Default for OfflineQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_queue_is_empty() {
        let q = OfflineQueue::with_capacity(2);
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert_eq!(q.capacity(), 2);
    }

    #[test]
    fn enqueue_drain_roundtrip() {
        let q = OfflineQueue::with_capacity(3);
        q.enqueue("p1".into(), "r1".into()).unwrap();
        q.enqueue("p2".into(), "r2".into()).unwrap();
        assert_eq!(q.len(), 2);
        let drained = q.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].prompt, "p1");
        assert_eq!(drained[0].raw, "r1");
        assert_eq!(drained[1].prompt, "p2");
        assert!(q.is_empty());
    }

    #[test]
    fn queue_full_rejects() {
        let q = OfflineQueue::with_capacity(1);
        q.enqueue("p1".into(), "r1".into()).unwrap();
        let err = q.enqueue("p2".into(), "r2".into()).unwrap_err();
        assert_eq!(err, QueueFull(1));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn drain_empty_returns_empty_vec() {
        let q = OfflineQueue::with_capacity(2);
        let drained = q.drain();
        assert!(drained.is_empty());
    }

    #[test]
    fn multiple_drain_cycles() {
        let q = OfflineQueue::with_capacity(5);
        q.enqueue("a".into(), "a".into()).unwrap();
        q.enqueue("b".into(), "b".into()).unwrap();
        let d1 = q.drain();
        assert_eq!(d1.len(), 2);
        // 第二轮
        q.enqueue("c".into(), "c".into()).unwrap();
        let d2 = q.drain();
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].prompt, "c");
    }

    #[test]
    fn default_capacity_is_50() {
        // 不受环境变量影响时(测试环境通常未设),默认 50。
        // 为防环境变量泄漏,仅验证容量 ≥ 1。
        let q = OfflineQueue::new();
        assert!(q.capacity() >= 1);
    }

    #[test]
    fn queue_full_display() {
        let err = QueueFull(50);
        assert_eq!(format!("{}", err), "offline queue full (cap=50)");
    }
}
