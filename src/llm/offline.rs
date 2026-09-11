//! 连接状态跟踪:基于 LLM 调用结果的被动离线检测。
//!
//! 不主动发心跳 ping(避免 API 成本),基于真实 LLM 调用结果推断连接状态:
//! - 成功调用 → 复位到 [`Connectivity::Online`];
//! - 网络类错误(连接失败 / 超时 / SSE 中断)累计升态:
//!   Online → Degraded(≥2 次) → Offline(≥5 次)。
//!
//! 对标第十九轮 D13 离线模式(L1821-L1830):
//! - openclaw 16 种 FailoverReason + 时间驱动冷却(本轮简化为累计阈值);
//! - claudecode cache break 检测 + 指数退避(对齐 resilient.rs 重试逻辑);
//! - opencode WebSocket 重连 + Effect DI(恢复由用户输入触发,无后台任务)。
//!
//! 对应知识库 gap:第十九轮 D13 离线模式(L1821-L1830)。

use std::sync::Mutex;
use std::time::Instant;

/// 连接三态(对齐 openclaw FailoverReason 子集的简化)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connectivity {
    /// 在线:所有调用正常进入重试逻辑。
    Online,
    /// 降级:近 2 次网络错误。UI 显示警告,仍尝试调用(给 LLM 网关恢复的窗口)。
    Degraded,
    /// 离线:近 5 次网络错误。UI 显示离线,新输入进入队列,不发起 LLM 调用。
    Offline,
}

impl Connectivity {
    /// 人类可读标签(用于 TUI 横幅显示)。
    pub fn as_str(&self) -> &'static str {
        match self {
            Connectivity::Online => "Online",
            Connectivity::Degraded => "Degraded",
            Connectivity::Offline => "Offline",
        }
    }

    /// 是否应尝试 LLM 调用(Online/Degraded 尝试,Offline 跳过避免浪费重试链)。
    pub fn should_attempt_llm(&self) -> bool {
        !matches!(self, Connectivity::Offline)
    }
}

/// 进入 Degraded 的连续网络错误阈值(2 次,提前预警)。
pub const DEGRADED_THRESHOLD: u32 = 2;
/// 进入 Offline 的连续网络错误阈值(5 次,对齐熔断器 DEFAULT_CIRCUIT_FAILURE_THRESHOLD)。
pub const OFFLINE_THRESHOLD: u32 = 5;

#[derive(Debug)]
struct ConnInner {
    state: Connectivity,
    /// 连续网络错误计数器(成功时清零)。
    consecutive_network_errors: u32,
    /// 最后一次网络错误的时间戳(展示用)。
    last_network_error_at: Option<Instant>,
    /// 最后一次网络错误的类型标签(展示用,如 "connection_refused"/"timeout")。
    last_network_error_kind: Option<String>,
}

/// 连接状态机:累计网络错误 / 成功复位。
///
/// 线程安全:内部 `Mutex` 保护,可跨任务共享(`Arc<ConnectivityTracker>`)。
#[derive(Debug)]
pub struct ConnectivityTracker {
    inner: Mutex<ConnInner>,
}

/// 连接状态快照(供 TUI 展示,避免锁泄漏到视图层)。
#[derive(Debug, Clone)]
pub struct ConnectivitySnapshot {
    pub state: Connectivity,
    pub consecutive_network_errors: u32,
    pub last_network_error_ago_secs: Option<u64>,
    pub last_network_error_kind: Option<String>,
}

impl ConnectivityTracker {
    /// 创建状态机,初始为 Online。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(ConnInner {
                state: Connectivity::Online,
                consecutive_network_errors: 0,
                last_network_error_at: None,
                last_network_error_kind: None,
            }),
        }
    }

    /// 记录一次网络错误(连接失败 / 超时 / SSE 中断等),累计并升态。
    ///
    /// `kind` 为错误类型标签(仅供展示,不影响状态机逻辑)。
    pub fn record_network_error(&self, kind: impl Into<String>) {
        let mut inner = self.inner.lock().expect("ConnectivityTracker poisoned");
        inner.consecutive_network_errors += 1;
        inner.last_network_error_at = Some(Instant::now());
        inner.last_network_error_kind = Some(kind.into());
        inner.state = if inner.consecutive_network_errors >= OFFLINE_THRESHOLD {
            Connectivity::Offline
        } else if inner.consecutive_network_errors >= DEGRADED_THRESHOLD {
            Connectivity::Degraded
        } else {
            Connectivity::Online
        };
    }

    /// 记录一次成功调用,复位到 Online 并清零计数器。
    pub fn record_success(&self) {
        let mut inner = self.inner.lock().expect("ConnectivityTracker poisoned");
        // 仅在非 Online 时记录复位(减少日志噪音,且保留最后一态是 Online 的语义)。
        let _was_offline = !matches!(inner.state, Connectivity::Online);
        inner.state = Connectivity::Online;
        inner.consecutive_network_errors = 0;
        inner.last_network_error_at = None;
        inner.last_network_error_kind = None;
        // 不在此处打印恢复日志(由调用方决定,避免在 LLM 层引入 IO 耦合)。
        let _ = _was_offline;
    }

    /// 读取当前状态(轻量,供 dispatch 层决策)。
    pub fn state(&self) -> Connectivity {
        self.inner.lock().expect("ConnectivityTracker poisoned").state
    }

    /// 是否应尝试 LLM 调用(便捷方法,等价于 `state().should_attempt_llm()`)。
    pub fn should_attempt_llm(&self) -> bool {
        self.state().should_attempt_llm()
    }

    /// 读取完整快照(供 TUI 横幅展示)。
    pub fn snapshot(&self) -> ConnectivitySnapshot {
        let inner = self.inner.lock().expect("ConnectivityTracker poisoned");
        ConnectivitySnapshot {
            state: inner.state,
            consecutive_network_errors: inner.consecutive_network_errors,
            last_network_error_ago_secs: inner
                .last_network_error_at
                .map(|t| t.elapsed().as_secs()),
            last_network_error_kind: inner.last_network_error_kind.clone(),
        }
    }
}

impl Default for ConnectivityTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_online() {
        let t = ConnectivityTracker::new();
        assert_eq!(t.state(), Connectivity::Online);
        assert!(t.should_attempt_llm());
    }

    #[test]
    fn one_error_stays_online() {
        let t = ConnectivityTracker::new();
        t.record_network_error("timeout");
        assert_eq!(t.state(), Connectivity::Online);
        assert!(t.should_attempt_llm());
    }

    #[test]
    fn two_errors_enter_degraded() {
        let t = ConnectivityTracker::new();
        t.record_network_error("timeout");
        t.record_network_error("connection_refused");
        assert_eq!(t.state(), Connectivity::Degraded);
        assert!(t.should_attempt_llm());
    }

    #[test]
    fn five_errors_enter_offline() {
        let t = ConnectivityTracker::new();
        for i in 0..5 {
            t.record_network_error(format!("error_{i}"));
        }
        assert_eq!(t.state(), Connectivity::Offline);
        assert!(!t.should_attempt_llm());
    }

    #[test]
    fn success_resets_to_online() {
        let t = ConnectivityTracker::new();
        for _ in 0..4 {
            t.record_network_error("timeout");
        }
        assert_eq!(t.state(), Connectivity::Degraded);
        t.record_success();
        assert_eq!(t.state(), Connectivity::Online);
        assert!(t.should_attempt_llm());
    }

    #[test]
    fn success_resets_from_offline() {
        let t = ConnectivityTracker::new();
        for _ in 0..7 {
            t.record_network_error("timeout");
        }
        assert_eq!(t.state(), Connectivity::Offline);
        t.record_success();
        assert_eq!(t.state(), Connectivity::Online);
    }

    #[test]
    fn snapshot_reports_fields() {
        let t = ConnectivityTracker::new();
        t.record_network_error("timeout");
        t.record_network_error("connection_refused");
        let snap = t.snapshot();
        assert_eq!(snap.state, Connectivity::Degraded);
        assert_eq!(snap.consecutive_network_errors, 2);
        assert!(snap.last_network_error_ago_secs.is_some());
        assert_eq!(snap.last_network_error_kind.as_deref(), Some("connection_refused"));
    }

    #[test]
    fn snapshot_after_success_clears_error() {
        let t = ConnectivityTracker::new();
        t.record_network_error("timeout");
        t.record_success();
        let snap = t.snapshot();
        assert_eq!(snap.state, Connectivity::Online);
        assert_eq!(snap.consecutive_network_errors, 0);
        assert!(snap.last_network_error_ago_secs.is_none());
        assert!(snap.last_network_error_kind.is_none());
    }

    #[test]
    fn as_str_labels() {
        assert_eq!(Connectivity::Online.as_str(), "Online");
        assert_eq!(Connectivity::Degraded.as_str(), "Degraded");
        assert_eq!(Connectivity::Offline.as_str(), "Offline");
    }

    #[test]
    fn boundary_degraded_then_more_errors() {
        // 2 errors → Degraded, 3-4 errors still Degraded, 5 → Offline
        let t = ConnectivityTracker::new();
        t.record_network_error("e1");
        t.record_network_error("e2");
        assert_eq!(t.state(), Connectivity::Degraded);
        t.record_network_error("e3");
        assert_eq!(t.state(), Connectivity::Degraded);
        t.record_network_error("e4");
        assert_eq!(t.state(), Connectivity::Degraded);
        t.record_network_error("e5");
        assert_eq!(t.state(), Connectivity::Offline);
    }
}
