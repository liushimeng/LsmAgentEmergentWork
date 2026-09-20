//! 人工介入(HITL)枢纽 —— Agent 工具与 TUI 之间的「提问 → 人答」闭环。
//!
//! 设计见 `docs/MCP_Web_Use/02-人工介入与窗口可视化方案.md`(2026-09-20 第 100 轮)。
//!
//! 背景:MCP_Web_Use 遇到图形/滑块验证码、短信验证码、扫码登录等无法自动跳过的
//! 流程时,需要把控制权临时交还人类。Tool trait 无法感知调用方是否处于 TUI 上下文,
//! 因此采用**进程内全局枢纽**:
//!
//! - TUI 启动(且 stdin 为 TTY)时调用 [`HumanAssistHub::attach`];
//! - 工具侧调用 [`HumanAssistHub::request`] 注册请求并在 oneshot 上挂起等待;
//! - TUI 的阶段进度协程轮询 [`HumanAssistHub::poll`],渲染请求块、行读 stdin,
//!   再 [`HumanAssistHub::respond`] 回填;
//! - 非 TTY(`-p` 单轮 / 管道)未 attach 时 request 立即失败(fail-fast),
//!   工具层映射 code=4001,由 System Prompt 指引 LLM 如实告知用户改用交互模式。
//!
//! 外部调研参考(`docs/Agent源码调研/专题/专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md` §8):
//! - atomcode AskUserQuestion:结构化标题/选项/自由文本;
//! - opencode WorkerPendingPermission:会话级队列防并发弹窗(此处简化为单 pending 槽位);
//! - claudecode 超时语义(默认 120s/上限 600s):本实现默认 300s/上限 1800s;
//! - deepseek 4-outcome 审计:answered / timeout / cancelled / unavailable 四态。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{oneshot, Notify};

/// 人工介入请求的默认等待时长(5 分钟,短信验证码场景余量充足)。
pub const DEFAULT_HUMAN_ASSIST_TIMEOUT_MS: u64 = 300_000;
/// 上限 30 分钟(对齐 claudecode 600s 上限并放宽,适配扫码/人脸等慢流程)。
pub const MAX_HUMAN_ASSIST_TIMEOUT_MS: u64 = 1_800_000;
/// 下限 10 秒(防止误传 1ms 导致 TUI 来不及渲染)。
pub const MIN_HUMAN_ASSIST_TIMEOUT_MS: u64 = 10_000;

/// 归一化等待时长:0/缺省 → 默认;统一 clamp 到 [10s, 1800s]。
pub fn clamp_human_assist_timeout_ms(ms: u64) -> u64 {
    if ms == 0 {
        DEFAULT_HUMAN_ASSIST_TIMEOUT_MS
    } else {
        ms.clamp(MIN_HUMAN_ASSIST_TIMEOUT_MS, MAX_HUMAN_ASSIST_TIMEOUT_MS)
    }
}

/// TUI 侧轮询拿到的展示形态(工具侧请求的只读投影,不含 responder)。
#[derive(Debug, Clone)]
pub struct HumanAssistDisplay {
    pub id: u64,
    /// 阻断类型:captcha / sms / qr_login / login / manual_verify / custom。
    pub kind: String,
    /// 给人看的具体说明。
    pub message: String,
    /// 编号选项(1~6 个,可为空 —— 空则 TUI 直接读自由文本)。
    pub options: Vec<String>,
    /// 关联页面 URL(展示用)。
    pub url: String,
    /// 关联 page_id(展示用)。
    pub page_id: String,
    pub timeout_ms: u64,
}

/// 工具侧等待结果(四态,对齐 deepseek approval outcome 审计语义)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanAssistOutcome {
    /// 人工已回答(选项文本或自由文本,如短信验证码数字)。
    Answered(String),
    /// 等待超时。
    Timeout,
    /// 人工明确取消(TUI 输入 q/取消)。
    Cancelled,
    /// 无人监听(非 TUI 交互模式)或请求被任务收尾清理。
    Unavailable,
}

/// 待答槽位:展示字段 + oneshot 回填端。respond 时被整体取出发送。
struct PendingSlot {
    display: HumanAssistDisplay,
    responder: oneshot::Sender<Option<String>>,
}

#[derive(Default)]
struct HubState {
    current: Option<PendingSlot>,
    seq: u64,
}

/// 全局人工介入枢纽(进程内单例)。
///
/// 单 pending 槽位语义:同一时刻最多一个待答请求;`request` 在槽位占用时
/// 先等前一个被 respond/cancel/timeout 再注册自己,避免 TUI 提示互相踩踏
/// (opencode WorkerPendingPermission 队列的简化版)。
pub struct HumanAssistHub {
    attached: AtomicBool,
    state: Mutex<HubState>,
    /// 槽位从占用变为空闲时唤醒排队中的 request。
    slot_free: Notify,
}

impl HumanAssistHub {
    pub fn global() -> Arc<Self> {
        static HUB: OnceLock<Arc<HumanAssistHub>> = OnceLock::new();
        HUB.get_or_init(|| {
            Arc::new(Self {
                attached: AtomicBool::new(false),
                state: Mutex::new(HubState::default()),
                slot_free: Notify::new(),
            })
        })
        .clone()
    }

    /// TUI(TTY)启动时调用;非 TTY 模式不调用,request 将 fail-fast。
    pub fn attach(&self) {
        self.attached.store(true, Ordering::SeqCst);
    }

    /// TUI 退出时调用(可重入)。
    pub fn detach(&self) {
        self.attached.store(false, Ordering::SeqCst);
        self.cancel_pending();
    }

    pub fn is_attached(&self) -> bool {
        self.attached.load(Ordering::SeqCst)
    }

    /// 工具侧:注册人工介入请求并等待回答。
    ///
    /// - 未 attach(非 TUI 交互模式)→ 立即 [`HumanAssistOutcome::Unavailable`];
    /// - 槽位被占用时排队等待(前一个请求被 respond/超时/取消后自动入位);
    /// - 超时 → [`HumanAssistOutcome::Timeout`],并主动清槽(若仍是本请求)。
    #[allow(clippy::too_many_arguments)]
    pub async fn request(
        &self,
        kind: &str,
        message: &str,
        options: Vec<String>,
        url: &str,
        page_id: &str,
        timeout_ms: u64,
    ) -> HumanAssistOutcome {
        if !self.is_attached() {
            return HumanAssistOutcome::Unavailable;
        }
        let timeout_ms = clamp_human_assist_timeout_ms(timeout_ms);

        // 竞争槽位:占用则等待 slot_free 通知后重试。
        let receiver = loop {
            let notified = {
                let mut state = lock_state(&self.state);
                if state.current.is_none() {
                    state.seq += 1;
                    let id = state.seq;
                    let (tx, rx) = oneshot::channel();
                    state.current = Some(PendingSlot {
                        display: HumanAssistDisplay {
                            id,
                            kind: kind.to_string(),
                            message: message.to_string(),
                            options,
                            url: url.to_string(),
                            page_id: page_id.to_string(),
                            timeout_ms,
                        },
                        responder: tx,
                    });
                    break rx;
                }
                // 槽位占用:注册 notified 守卫后重查,避免唤醒丢失
                self.slot_free.notified()
            };
            notified.await;
        };

        match tokio::time::timeout(Duration::from_millis(timeout_ms), receiver).await {
            // 通道正常收到回答
            Ok(Ok(Some(answer))) => {
                self.slot_free.notify_waiters();
                HumanAssistOutcome::Answered(answer)
            }
            // respond(None):人工取消
            Ok(Ok(None)) => {
                self.slot_free.notify_waiters();
                HumanAssistOutcome::Cancelled
            }
            // responder 被 drop(cancel_pending / 进程收尾):视为不可用
            Ok(Err(_)) => {
                self.slot_free.notify_waiters();
                HumanAssistOutcome::Unavailable
            }
            // 超时:若槽位仍是本请求则清掉
            Err(_) => {
                {
                    let mut state = lock_state(&self.state);
                    // seq 单调递增且只有入位才自增:current.id == seq 即本请求;
                    // 若期间已被 respond 清槽(current=None),同样无需处理。
                    if state.current.as_ref().map(|c| c.display.id) == Some(state.seq) {
                        state.current = None;
                    }
                }
                self.slot_free.notify_waiters();
                HumanAssistOutcome::Timeout
            }
        }
    }

    /// TUI 侧:轮询当前待答请求(同一请求会重复返回,调用方按 id 去重)。
    pub fn poll(&self) -> Option<HumanAssistDisplay> {
        lock_state(&self.state)
            .current
            .as_ref()
            .map(|c| c.display.clone())
    }

    /// TUI 侧:按 id 回填。`answer=Some` 为人工输入;`None` 表示人工取消。
    /// 返回 false 表示 id 已失效(超时/已被处理)。
    pub fn respond(&self, id: u64, answer: Option<String>) -> bool {
        let taken = {
            let mut state = lock_state(&self.state);
            match state.current.as_ref().map(|c| c.display.id) {
                Some(cur) if cur == id => state.current.take(),
                _ => None,
            }
        };
        match taken {
            Some(slot) => {
                let _ = slot.responder.send(answer);
                true
            }
            None => false,
        }
    }

    /// 任务收尾兜底:丢弃未答请求(responder drop → 工具侧 Unavailable)。
    pub fn cancel_pending(&self) {
        drop(lock_state(&self.state).current.take());
        self.slot_free.notify_waiters();
    }
}

/// lock 辅助:中毒锁(其它线程 panic)时重建空状态 —— HITL 属可失败增强路径,
/// 不因锁中毒拖垮主任务(fail-open 语义,对齐 decision_audit)。
fn lock_state(state: &Mutex<HubState>) -> std::sync::MutexGuard<'_, HubState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局单例测试互斥:并行的 attach/detach/poll 会互相污染 pending 槽位,
    /// 所有触达 HumanAssistHub 的测试串行执行。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn timeout_clamp() {
        assert_eq!(
            clamp_human_assist_timeout_ms(0),
            DEFAULT_HUMAN_ASSIST_TIMEOUT_MS
        );
        assert_eq!(
            clamp_human_assist_timeout_ms(1),
            MIN_HUMAN_ASSIST_TIMEOUT_MS
        );
        assert_eq!(
            clamp_human_assist_timeout_ms(u64::MAX),
            MAX_HUMAN_ASSIST_TIMEOUT_MS
        );
    }

    #[tokio::test]
    async fn unattached_request_fails_fast() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let hub = HumanAssistHub::global();
        let was = hub.is_attached();
        hub.detach();
        let out = hub
            .request("captcha", "x", vec![], "https://a", "p_1", 1000)
            .await;
        assert_eq!(out, HumanAssistOutcome::Unavailable);
        if was {
            hub.attach();
        }
    }

    #[tokio::test]
    async fn request_poll_respond_answered() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let hub = HumanAssistHub::global();
        hub.attach();
        let task = tokio::spawn({
            let hub = hub.clone();
            async move {
                hub.request(
                    "sms",
                    "请输入短信验证码",
                    vec!["已完成".into()],
                    "https://b",
                    "p_2",
                    60_000,
                )
                .await
            }
        });
        // 等 request 入位
        let display = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(d) = hub.poll() {
                    break d;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("poll 应拿到请求");
        assert_eq!(display.kind, "sms");
        assert_eq!(display.options, vec!["已完成".to_string()]);
        assert!(hub.respond(display.id, Some("123456".into())));
        assert_eq!(
            task.await.unwrap(),
            HumanAssistOutcome::Answered("123456".into())
        );
        assert!(hub.poll().is_none(), "respond 后槽位应清空");
        hub.detach();
    }

    #[tokio::test]
    async fn respond_none_means_cancelled() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let hub = HumanAssistHub::global();
        hub.attach();
        let task = tokio::spawn({
            let hub = hub.clone();
            async move { hub.request("custom", "取消我", vec![], "", "p_3", 60_000).await }
        });
        let display = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(d) = hub.poll() {
                    break d;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("poll");
        assert!(hub.respond(display.id, None));
        assert_eq!(task.await.unwrap(), HumanAssistOutcome::Cancelled);
        hub.detach();
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_clears_slot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let hub = HumanAssistHub::global();
        hub.attach();
        let out = hub
            .request("captcha", "x", vec![], "", "p_4", MIN_HUMAN_ASSIST_TIMEOUT_MS)
            .await;
        assert_eq!(out, HumanAssistOutcome::Timeout);
        assert!(hub.poll().is_none(), "超时后槽位应被清理");
        hub.detach();
    }

    #[tokio::test]
    async fn queued_request_takes_freed_slot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let hub = HumanAssistHub::global();
        hub.attach();
        let first = tokio::spawn({
            let hub = hub.clone();
            async move { hub.request("captcha", "first", vec![], "", "p_5", 60_000).await }
        });
        let d1 = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(d) = hub.poll() {
                    break d;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("first poll");
        // 第二个请求应排队而非覆盖
        let second = tokio::spawn({
            let hub = hub.clone();
            async move { hub.request("sms", "second", vec![], "", "p_6", 60_000).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let polled = hub.poll().expect("槽位仍应是第一个请求");
        assert_eq!(polled.id, d1.id);
        // respond 第一个 → 第二个入位
        assert!(hub.respond(d1.id, Some("ok".into())));
        assert_eq!(
            first.await.unwrap(),
            HumanAssistOutcome::Answered("ok".into())
        );
        let d2 = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(d) = hub.poll() {
                    break d;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("second poll");
        assert_eq!(d2.message, "second");
        assert!(hub.respond(d2.id, None));
        assert_eq!(second.await.unwrap(), HumanAssistOutcome::Cancelled);
        hub.detach();
    }
}
