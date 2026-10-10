//! 人工介入(HITL)枢纽 —— Agent 工具与呈现前端(弹窗 UI / TUI)之间的「提问 → 人答」闭环。
//!
//! 设计见 `docs/MCP_Web_Use/02-人工介入与窗口可视化方案.md`(2026-09-20 第 100 轮)
//! 与 `docs/MCP_Web_Use/03-人工介入弹窗UI动态加载方案.md`(2026-10-08 第 130 轮)。
//!
//! 背景:MCP_Web_Use 遇到图形/滑块验证码、短信验证码、扫码登录等无法自动跳过的
//! 流程时,需要把控制权临时交还人类。Tool trait 无法感知调用方是否处于 TUI 上下文,
//! 因此采用**进程内全局枢纽**:
//!
//! - 工具侧调用 [`HumanAssistHub::request`] 注册请求并在 oneshot 上挂起等待;
//! - **呈现端双通道**(第 130 轮):`human_ui::enabled()` 为真(macOS/Windows 桌面)
//!   时入位即 spawn 桌面弹窗(`via=gui`,持续置顶+倒计时+时间轴);否则/弹窗失败
//!   由 TUI 阶段协程轮询 [`HumanAssistHub::poll`] 行读 stdin(`via=tui`);
//!   `-p`/管道模式在桌面会话同样可弹 —— 只有「弹窗与 TUI 均不可用」才 fail-fast;
//! - 两个前端经 [`HumanAssistHub::respond`] 幂等回填(后到者拿 false),互斥不双答;
//! - 工具层按结局映射 code=0/4001/4002,由 System Prompt 指引 LLM 如实收口。
//!
//! 外部调研参考(`docs/Agent源码调研/专题/专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md` §8):
//! - atomcode AskUserQuestion:结构化标题/选项/自由文本;
//! - opencode WorkerPendingPermission:会话级队列防并发弹窗(此处简化为单 pending 槽位);
//! - claudecode 超时语义(默认 120s/上限 600s):本实现按 reason 分档 120s/300s、上限 1800s;
//! - deepseek 4-outcome 审计:answered / timeout / cancelled / unavailable 四态。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{oneshot, Notify};

/// 人工介入请求的默认等待时长。
///
/// 第 118 轮(2026-09-22):从 300_000(5 分钟)降到 120_000(2 分钟),对齐
/// claudecode 默认 120s / 上限 600s 的实践经验。验证码/短信/2FA 等
/// 「短文本回 TUI」场景 120s 余量充足;扫码/人脸/账密登录场景由
/// `act_request_human` 按 reason 分档默认 300_000 覆盖。LLM 仍可通过
/// `timeout_ms` 参数显式覆盖。
pub const DEFAULT_HUMAN_ASSIST_TIMEOUT_MS: u64 = 120_000;
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

/// 前端呈现通道(应答来源,信封 `assist_channel` 与审计标注用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistVia {
    /// 桌面弹窗(macOS/Windows,第 130 轮)。
    Gui,
    /// TUI 终端行读(兜底;Linux 全程)。
    Tui,
}

impl AssistVia {
    pub fn as_str(self) -> &'static str {
        match self {
            AssistVia::Gui => "gui",
            AssistVia::Tui => "tui",
        }
    }
}

/// TUI/日志侧的呈现事件流(弹窗生命周期通知;TUI 轮询 [`HumanAssistHub::take_events`] 打印)。
#[derive(Debug, Clone)]
pub enum AssistEvent {
    /// 弹窗已弹出(请求由 GUI 前端接管,TUI 不应读 stdin)。
    GuiLaunched { id: u64 },
    /// 弹窗启动失败(已降级 TUI 行读)。
    GuiFailed { id: u64 },
    /// 弹窗应答成功。
    GuiAnswered { id: u64, text: String },
    /// 弹窗人工取消。
    GuiCancelled { id: u64 },
    /// 弹窗倒计时归零(与 hub 超时收口汇合)。
    GuiTimeout { id: u64 },
    /// 第 145 轮:人工在弹窗点「⏱ +2 分钟」延长等待(累计延长量随事件透出,
    /// TUI 打「已延长等待」通知行;到达上限后 added_ms=0 仍上报但不生效)。
    GuiExtended { id: u64, added_ms: u64 },
    /// 第 134 轮:弹窗接管期间,用户在终端用 `/hitl <应答>` 应急通道作答
    /// (`text=None` 即 `/hitl cancel`)。TUI 事件循环据此打印结果行。
    TuiEscapeAnswered { id: u64, text: Option<String> },
}

/// TUI 侧轮询拿到的展示形态(工具侧请求的只读投影,不含 responder)。
#[derive(Debug, Clone)]
pub struct HumanAssistDisplay {
    pub id: u64,
    /// 阻断类型:captcha / sms / qr_login / login / real_name / two_factor / oauth /
    /// manual_verify / custom。
    pub kind: String,
    /// 给人看的具体说明。
    pub message: String,
    /// 编号选项(1~6 个,可为空 —— 空则 TUI 直接读自由文本)。
    pub options: Vec<String>,
    /// 关联页面 URL(展示用)。
    pub url: String,
    /// 关联 page_id(展示用)。
    pub page_id: String,
    /// 第 132 轮:验证码等阻断现场的截图路径(空 = 无图)。
    /// 由 `act_request_human` 解析(显式 `params.image_path` 优先,`reason=captcha`
    /// 自动 CDP 视口截图),弹窗前端把它直接渲染出来 —— 人工不必切去浏览器找图,
    /// 在弹窗里即可读码;TUI 兜底行读时打印路径供 `open` 查看。
    pub image_path: String,
    pub timeout_ms: u64,
    /// 第 119 轮新增:输入提示符同行可视宽度(中文/全角算 2 列),
    /// TUI 协程用它把倒计时右对齐到屏幕右侧(避免压在提示符上)。
    /// 默认 0 走「整行重写」回退路径, 不参与右对齐。
    pub prompt_visual_width: u16,
    /// 第 130 轮:请求提出时刻(unix 毫秒,本地时区展示)。弹窗时间轴
    /// 「提出时间 / 超时截止」与 TUI 通知行共用;0 = 未记录(测试构造用)。
    pub created_at_ms: u64,
}

/// 工具侧等待结果(四态,对齐 deepseek approval outcome 审计语义)。
/// `via` 标注人工实际作答的前端(弹窗 / TUI),供信封 `assist_channel` 与审计对账。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanAssistOutcome {
    /// 人工已回答(选项文本或自由文本,如短信验证码数字)。
    Answered { text: String, via: AssistVia },
    /// 等待超时。
    Timeout { via: AssistVia },
    /// 人工明确取消(弹窗取消/Esc/关窗,或 TUI 输入 q/取消)。
    Cancelled { via: AssistVia },
    /// 无人监听(弹窗与 TUI 均不可用)或请求被任务收尾清理。
    Unavailable,
}

/// 待答槽位:展示字段 + oneshot 回填端。respond 时被整体取出发送。
struct PendingSlot {
    display: HumanAssistDisplay,
    responder: oneshot::Sender<(Option<String>, AssistVia)>,
}

#[derive(Default)]
struct HubState {
    current: Option<PendingSlot>,
    seq: u64,
    /// 当前由弹窗前端接管的请求 id(`mark_gui_failed` 后清空 → TUI 降级接管)。
    gui_id: Option<u64>,
    /// 呈现事件环(上限 32,防 -p 模式无人消费时无界增长)。
    events: VecDeque<AssistEvent>,
    /// 第 145 轮:当前请求经弹窗「⏱ +2 分钟」累计延长的毫秒数(仅 id 匹配当前
    /// 槽位时有效;等待循环每轮重读,实现「动态截止时刻」)。槽位收口时清空。
    extend_ms: Option<(u64, u64)>,
}

// ---------- kind 单一事实源(第 130 轮自 control.rs / tui/dispatch.rs 收口迁入) ----------

/// reason → 弹窗/TUI 展示标签(request_human 的 reason 映射)。
/// 新增 reason 必须同步更新此处 + [`default_message`] + `HUMAN_ASSIST_ALLOWED_REASONS`
/// + blockers 关键词表 + 文档,保证 LLM / TUI / 弹窗 / 阻断检测四处对齐。
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "captcha" => "图形/滑块验证码",
        "sms" => "短信验证码",
        "qr_login" => "扫码登录",
        "login" => "账密登录",
        "real_name" => "实名认证/人脸核身",
        "two_factor" => "二次验证/2FA",
        "oauth" => "第三方授权",
        "manual_verify" => "人工核验",
        _ => "人工介入",
    }
}

/// reason → 默认说明文案(LLM 未传 message 时使用)。
/// 第 145 轮:收口动作统一引导到弹窗「提交 / 继续」按钮(旧文案的「回到终端确认」
/// 是 TUI 时代口径;桌面弹窗优先的当下,人工的收工动作就是回弹窗点『提交 / 继续』,
/// 把「交棒给 Agent」这个语义显性化)。
pub fn default_message(kind: &str) -> &'static str {
    match kind {
        "captcha" => {
            "页面出现验证码/滑块,Agent 无法自动完成。请人工在浏览器窗口完成验证后回到弹窗点『提交 / 继续』;若是图形/文字验证码,也可直接在弹窗输入框填入后点『提交 / 继续』。"
        }
        "sms" => "页面要求短信验证码,Agent 无法获取。请把手机收到的验证码数字直接输入在弹窗输入框,点『提交 / 继续』。",
        "qr_login" => "页面要求扫码登录。请人工用手机完成扫码/确认后回到弹窗点『提交 / 继续』。",
        "login" => {
            "页面要求账号密码登录。请人工在浏览器窗口完成登录后回到弹窗点『提交 / 继续』(不要把密码发给 Agent)。"
        }
        "real_name" => {
            "页面要求实名认证/上传身份证/人脸核身,Agent 无法代为核验。请人工在浏览器窗口完成验证(刷脸/上传证件)后回到弹窗点『提交 / 继续』。"
        }
        "two_factor" => {
            "页面要求二次验证(2FA/TOTP/邮箱验证码)。请把手机 Authenticator 或邮箱里看到的动态码直接输入在弹窗输入框,点『提交 / 继续』。"
        }
        "oauth" => {
            "页面跳到第三方授权页(GitHub/微信/Google/SSO 等),Agent 无法跨设备授权。请人工在浏览器窗口完成授权后回到弹窗点『提交 / 继续』。"
        }
        "manual_verify" => "页面流程需要人工核验/确认。请人工在浏览器窗口完成后回到弹窗点『提交 / 继续』。",
        _ => "Agent 无法继续当前流程,需要人工处理。",
    }
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
    /// 第 119 轮新增:hub 端超时后通知 TUI 协程, 让 TUI 立即清理
    /// 视觉残留(蓝框 + 输入提示符 + 倒计时行)。
    /// TUI 协程用 `now_or_never()` 非阻塞检查, 不破坏原有 250ms tick 节奏。
    timeout_notify: Notify,
}

impl HumanAssistHub {
    pub fn global() -> Arc<Self> {
        static HUB: OnceLock<Arc<HumanAssistHub>> = OnceLock::new();
        HUB.get_or_init(|| {
            Arc::new(Self {
                attached: AtomicBool::new(false),
                state: Mutex::new(HubState::default()),
                slot_free: Notify::new(),
                timeout_notify: Notify::new(),
            })
        })
        .clone()
    }

    /// 第 119 轮新增:订阅超时事件(用于 TUI 协程非阻塞检查)。
    pub fn timeout_notified(&self) -> &Notify {
        &self.timeout_notify
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

    /// 当前是否有待答的人工介入请求(第 152 轮:浏览器工具的输入挂起闸读它)。
    ///
    /// 提问期间页面所有权归人工,Agent 的输入类动作必须挂起 —— 否则其收尾的
    /// 「先解后锁」会把人工正在操作的页面重新锁死。
    pub fn is_pending(&self) -> bool {
        lock_state(&self.state).current.is_some()
    }

    /// 工具侧:注册人工介入请求并等待回答。
    ///
    /// - 弹窗与 TUI 均不可用 → 立即 [`HumanAssistOutcome::Unavailable`];
    /// - 桌面会话(`human_ui::enabled()`)入位即 spawn 弹窗前端(`via=gui`),
    ///   弹窗失败自动降级 TUI 行读;TUI 会话无弹窗时走既有行读(`via=tui`);
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
        image_path: &str,
    ) -> HumanAssistOutcome {
        // 第 130 轮:桌面弹窗可用时即使非 TTY(-p/管道)也放行 —— 只有
        // 「弹窗与 TUI 均不可用」才 fail-fast。
        let gui = crate::agent::human_ui::enabled();
        if !self.is_attached() && !gui {
            return HumanAssistOutcome::Unavailable;
        }
        let timeout_ms = clamp_human_assist_timeout_ms(timeout_ms);

        // 竞争槽位:占用则等待 slot_free 通知后重试。
        let (receiver, display) = loop {
            let notified = {
                let mut state = lock_state(&self.state);
                if state.current.is_none() {
                    state.seq += 1;
                    let id = state.seq;
                    let (tx, rx) = oneshot::channel();
                    let display = HumanAssistDisplay {
                        id,
                        kind: kind.to_string(),
                        message: message.to_string(),
                        options,
                        url: url.to_string(),
                        page_id: page_id.to_string(),
                        image_path: image_path.to_string(),
                        timeout_ms,
                        prompt_visual_width: 0,
                        created_at_ms: now_unix_ms(),
                    };
                    state.current = Some(PendingSlot {
                        display: display.clone(),
                        responder: tx,
                    });
                    break (rx, display);
                }
                // 槽位占用:注册 notified 守卫后重查,避免唤醒丢失
                self.slot_free.notified()
            };
            notified.await;
        };

        // 第 130 轮:桌面弹窗前端(独立任务;槽位释放即 drop → kill_on_drop 回收弹窗进程)。
        // 第 145 轮:display 已 move 进弹窗任务,循环内用自己的 id 副本(动态截止收口用)。
        let request_id = display.id;
        if gui {
            {
                let mut state = lock_state(&self.state);
                state.gui_id = Some(display.id);
                push_event(&mut state, AssistEvent::GuiLaunched { id: display.id });
            }
            tokio::spawn(async move {
                Self::present_via_gui(display).await;
            });
        }

        // 第 145 轮:等待循环改为「动态截止时刻」——弹窗「⏱ +2 分钟」按钮经
        // [`HumanAssistHub::extend_timeout`] 累计延长量,每轮重算 remaining。
        let mut receiver = receiver;
        let base_deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let deadline =
                base_deadline + Duration::from_millis(self.extension_of(request_id));
            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                // 已过截止(理论上首轮不会走到;延长后重入时可能):按超时收口。
                return self.finish_timeout(request_id);
            };
            let answered = tokio::select! {
                res = &mut receiver => Some(res),
                _ = tokio::time::sleep(remaining) => None,
            };
            if let Some(res) = answered {
                self.slot_free.notify_waiters();
                return match res {
                    // 通道正常收到回答
                    Ok((Some(answer), via)) => HumanAssistOutcome::Answered { text: answer, via },
                    // respond(None):人工取消
                    Ok((None, via)) => HumanAssistOutcome::Cancelled { via },
                    // responder 被 drop(cancel_pending / 进程收尾):视为不可用
                    Err(_) => HumanAssistOutcome::Unavailable,
                };
            }
            // 超时臂醒来,两步竞态宽容(顺序不可换):
            // ① respond 可能恰与截止同时到达 —— 先非阻塞收一次应答。不能只靠
            //    下面的延长量复核判定:respond 会清空 extend_ms,刚批准的延长会
            //    「隐身」,把已作答的请求误收成 Timeout(单测锁死该竞态)。
            if let Ok(v) = receiver.try_recv() {
                self.slot_free.notify_waiters();
                return match v {
                    (Some(answer), via) => HumanAssistOutcome::Answered { text: answer, via },
                    (None, via) => HumanAssistOutcome::Cancelled { via },
                };
            }
            // ② 截止瞬间恰有延长到达(尚未被 respond 清空)→ 按新截止继续等。
            let extended =
                base_deadline + Duration::from_millis(self.extension_of(request_id));
            if extended > tokio::time::Instant::now() {
                continue;
            }
            return self.finish_timeout(request_id);
        }
    }

    /// 超时收口(原 request 内联逻辑,第 145 轮抽出供动态截止循环复用):
    /// 若槽位仍是本请求则清掉,通知 TUI 协程清理视觉,返回 Timeout。
    fn finish_timeout(&self, id: u64) -> HumanAssistOutcome {
        let via = {
            let mut state = lock_state(&self.state);
            // seq 单调递增且只有入位才自增:current.id == id 即本请求;
            // 若期间已被 respond 清槽(current=None),同样无需处理。
            let mine = state.current.as_ref().map(|c| c.display.id) == Some(id);
            let was_gui = state.gui_id == Some(id);
            if mine {
                state.current = None;
                state.gui_id = None;
                state.extend_ms = None;
                if was_gui {
                    push_event(&mut state, AssistEvent::GuiTimeout { id });
                }
            }
            if was_gui {
                AssistVia::Gui
            } else {
                AssistVia::Tui
            }
        };
        self.slot_free.notify_waiters();
        // 第 119 轮:通知 TUI 协程 hub 已超时, 让它立即清理视觉(避免卡死)。
        self.timeout_notify.notify_waiters();
        HumanAssistOutcome::Timeout { via }
    }

    /// 第 145 轮:读取指定请求的累计延长毫秒数(等待循环每轮重读)。
    fn extension_of(&self, id: u64) -> u64 {
        lock_state(&self.state)
            .extend_ms
            .filter(|(eid, _)| *eid == id)
            .map(|(_, ms)| ms)
            .unwrap_or(0)
    }

    /// 第 145 轮:延长当前请求的等待时限(弹窗「⏱ +2 分钟」按钮 → 子进程 stdout
    /// `{"status":"extend"}` 中间行 → `human_ui::run_dialog_process` 调用)。
    ///
    /// - 仅当 `id` 仍是当前槽位请求时生效(弹窗延迟到达的 extend 行不复活旧请求);
    /// - 累计延长后**总等待时长**封顶 [`MAX_HUMAN_ASSIST_TIMEOUT_MS`](原始 timeout_ms
    ///   + 累计延长 ≤ 30 分钟,与 clamp 语义同源);
    /// - 生效时入 `GuiExtended` 事件(TUI 打「已延长等待」通知行)。
    pub fn extend_timeout(&self, id: u64, ms: u64) -> bool {
        let mut state = lock_state(&self.state);
        if state.current.as_ref().map(|c| c.display.id) != Some(id) {
            return false;
        }
        let base = state
            .current
            .as_ref()
            .map(|c| c.display.timeout_ms)
            .unwrap_or(0);
        let cap = MAX_HUMAN_ASSIST_TIMEOUT_MS.saturating_sub(base);
        let acc = state.extend_ms.filter(|(eid, _)| *eid == id).map(|(_, m)| m).unwrap_or(0);
        let added = ms.min(cap.saturating_sub(acc));
        state.extend_ms = Some((id, acc + added));
        push_event(&mut state, AssistEvent::GuiExtended { id, added_ms: added });
        true
    }

    /// 弹窗前端呈现任务:应答经 respond 回填;失败降级 TUI;槽位提前释放即丢弃
    /// 弹窗 future(tokio kill_on_drop 回收弹窗子进程,不留孤儿窗口)。
    async fn present_via_gui(display: HumanAssistDisplay) {
        use crate::agent::human_ui::UiResult;
        let hub = HumanAssistHub::global();
        let id = display.id;
        tokio::select! {
            res = crate::agent::human_ui::present(&display) => match res {
                UiResult::Answered(text) => {
                    if hub.respond(id, Some(text.clone()), AssistVia::Gui) {
                        let mut state = lock_state(&hub.state);
                        push_event(&mut state, AssistEvent::GuiAnswered { id, text });
                    }
                }
                UiResult::Cancelled => {
                    if hub.respond(id, None, AssistVia::Gui) {
                        let mut state = lock_state(&hub.state);
                        push_event(&mut state, AssistEvent::GuiCancelled { id });
                    }
                }
                UiResult::Timeout => {
                    // 弹窗倒计时自灭;hub 超时几乎同时收口,仅留事件供 TUI 观测。
                    let mut state = lock_state(&hub.state);
                    push_event(&mut state, AssistEvent::GuiTimeout { id });
                }
                UiResult::Error(e) => {
                    tracing::warn!("人工介入弹窗启动失败,降级 TUI 行读: {e}");
                    hub.mark_gui_failed(id);
                    let mut state = lock_state(&hub.state);
                    push_event(&mut state, AssistEvent::GuiFailed { id });
                    drop(state);
                    // 无 TUI 兜底时不必空等超时:直接收口为 Unavailable(4001)。
                    if !hub.is_attached() {
                        hub.cancel_slot(id);
                    }
                }
            },
            _ = wait_slot_gone(id) => {
                // 其它前端(TUI/收尾/超时)已收口:丢弃 present future → 回收弹窗进程。
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

    /// 当前请求是否由弹窗前端接管(TUI 据此跳过 stdin 行读,只打通知行)。
    pub fn is_gui_presenting(&self, id: u64) -> bool {
        lock_state(&self.state).gui_id == Some(id)
    }

    /// 弹窗启动失败降级:清 gui 标记,TUI 轮询下一轮改走行读。
    pub fn mark_gui_failed(&self, id: u64) {
        let mut state = lock_state(&self.state);
        if state.gui_id == Some(id) {
            state.gui_id = None;
        }
    }

    /// 取走呈现事件(弹窗弹出/失败/应答/取消;TUI 轮询打印通知行)。
    pub fn take_events(&self) -> Vec<AssistEvent> {
        lock_state(&self.state).events.drain(..).collect()
    }

    /// TUI/前端侧:按 id 回填。`answer=Some` 为人工输入;`None` 表示人工取消。
    /// 返回 false 表示 id 已失效(超时/已被另一前端处理)。
    pub fn respond(&self, id: u64, answer: Option<String>, via: AssistVia) -> bool {
        let taken = {
            let mut state = lock_state(&self.state);
            let taken = match state.current.as_ref().map(|c| c.display.id) {
                Some(cur) if cur == id => state.current.take(),
                _ => None,
            };
            if taken.is_some() {
                state.extend_ms = None;
                if state.gui_id == Some(id) {
                    state.gui_id = None;
                }
            }
            taken
        };
        match taken {
            Some(slot) => {
                let _ = slot.responder.send((answer, via));
                true
            }
            None => false,
        }
    }

    /// 第 134 轮:终端 `/hitl <应答>` 应急通道回填(`answer=None` 即取消)。
    ///
    /// 弹窗接管期间 TUI 默认不读 stdin(避免与弹窗抢应答),但弹窗端一旦失联
    /// (屏外 / 被遮挡 / 进程卡住)就等价于 120s 死等。本方法让 TUI 能并联一条
    /// **只认 `/hitl` 前缀** 的行读并把应答送进同一个 hub 通道 —— 谁先应答谁生效,
    /// 另一侧回填时 `respond` 返回 false 自然作废。
    pub fn respond_via_tui_escape(&self, id: u64, answer: Option<String>) -> bool {
        let ok = self.respond(id, answer.clone(), AssistVia::Tui);
        if ok {
            let mut state = lock_state(&self.state);
            push_event(
                &mut state,
                AssistEvent::TuiEscapeAnswered {
                    id,
                    text: answer,
                },
            );
        }
        ok
    }

    /// 丢弃指定请求(responder drop → 工具侧 Unavailable)。弹窗失败且无 TUI 兜底时用。
    fn cancel_slot(&self, id: u64) {
        let taken = {
            let mut state = lock_state(&self.state);
            match state.current.as_ref().map(|c| c.display.id) {
                Some(cur) if cur == id => state.current.take(),
                _ => None,
            }
        };
        drop(taken);
        {
            let mut state = lock_state(&self.state);
            state.extend_ms = None;
            if state.gui_id == Some(id) {
                state.gui_id = None;
            }
        }
        self.slot_free.notify_waiters();
        self.timeout_notify.notify_waiters();
    }

    /// 任务收尾兜底:丢弃未答请求(responder drop → 工具侧 Unavailable)。
    pub fn cancel_pending(&self) {
        {
            let mut state = lock_state(&self.state);
            drop(state.current.take());
            state.extend_ms = None;
        }
        self.slot_free.notify_waiters();
        // 第 119 轮:任务收尾时也通知 TUI, 让它清理残留视觉。
        self.timeout_notify.notify_waiters();
    }
}

/// lock 辅助:中毒锁(其它线程 panic)时重建空状态 —— HITL 属可失败增强路径,
/// 不因锁中毒拖垮主任务(fail-open 语义,对齐 decision_audit)。
fn lock_state(state: &Mutex<HubState>) -> std::sync::MutexGuard<'_, HubState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 事件环入队(上限 32,溢出丢最旧 —— `-p` 模式无人消费也不无界增长)。
fn push_event(state: &mut HubState, ev: AssistEvent) {
    if state.events.len() >= 32 {
        state.events.pop_front();
    }
    state.events.push_back(ev);
}

/// 等待指定请求离开 pending 槽位(被 respond / 超时 / 收尾清槽)。
/// 弹窗呈现任务用它感知「其它前端已收口」,从而丢弃弹窗 future 回收弹窗进程。
async fn wait_slot_gone(id: u64) {
    let hub = HumanAssistHub::global();
    loop {
        {
            let state = lock_state(&hub.state);
            let still = state.current.as_ref().map(|c| c.display.id) == Some(id);
            if !still {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// unix 毫秒时间戳(弹窗时间轴「提出时间」用)。
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::human_ui;

    /// 全局单例测试互斥:并行的 attach/detach/poll 会互相污染 pending 槽位,
    /// 所有触达 HumanAssistHub 的测试串行执行。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 测试收尾:恢复弹窗开关/后端默认(防跨用例泄漏)。
    fn reset_ui_hooks() {
        human_ui::set_test_override(None);
        human_ui::set_test_backend(None);
    }

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
        reset_ui_hooks();
        let hub = HumanAssistHub::global();
        let was = hub.is_attached();
        hub.detach();
        let out = hub
            .request("captcha", "x", vec![], "https://a", "p_1", 1000, "")
            .await;
        assert_eq!(out, HumanAssistOutcome::Unavailable);
        if was {
            hub.attach();
        }
    }

    #[tokio::test]
    async fn request_poll_respond_answered() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
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
                    "/tmp/laew_hitl_captcha_demo.png",
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
        // 第 132 轮:验证码截图路径应原样投影到弹窗/TUI 侧的展示形态
        assert_eq!(
            display.image_path, "/tmp/laew_hitl_captcha_demo.png",
            "image_path 应随请求透传"
        );
        assert!(display.created_at_ms > 0, "created_at_ms 应记录提出时刻");
        assert!(hub.respond(display.id, Some("123456".into()), AssistVia::Tui));
        assert_eq!(
            task.await.unwrap(),
            HumanAssistOutcome::Answered {
                text: "123456".into(),
                via: AssistVia::Tui
            }
        );
        assert!(hub.poll().is_none(), "respond 后槽位应清空");
        hub.detach();
    }

    #[tokio::test]
    async fn respond_none_means_cancelled() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        let hub = HumanAssistHub::global();
        hub.attach();
        let task = tokio::spawn({
            let hub = hub.clone();
            async move { hub.request("custom", "取消我", vec![], "", "p_3", 60_000, "").await }
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
        assert!(hub.respond(display.id, None, AssistVia::Tui));
        assert_eq!(
            task.await.unwrap(),
            HumanAssistOutcome::Cancelled {
                via: AssistVia::Tui
            }
        );
        hub.detach();
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_clears_slot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        let hub = HumanAssistHub::global();
        hub.attach();
        let out = hub
            .request("captcha", "x", vec![], "", "p_4", MIN_HUMAN_ASSIST_TIMEOUT_MS, "")
            .await;
        assert_eq!(
            out,
            HumanAssistOutcome::Timeout {
                via: AssistVia::Tui
            }
        );
        assert!(hub.poll().is_none(), "超时后槽位应被清理");
        hub.detach();
    }

    /// 第 119 轮:hub 端超时 / 任务收尾时必须通知 TUI 协程, 让 TUI 立即清理
    /// 残留视觉(倒计时同行右对齐 + 输入提示符), 避免 TUI 卡死等用户输入。
    ///
    /// 用 `cancel_pending`(同一条 notify_waiters 通道)做确定性验证, 避免依赖
    /// MIN_HUMAN_ASSIST_TIMEOUT_MS(10s 下限)导致测试挂 10 秒。
    #[tokio::test]
    async fn cancel_pending_notifies_tui_via_timeout_notify() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        let hub = HumanAssistHub::global();
        hub.attach();
        // 入位一个长超时请求(不依赖真实超时)
        let task = tokio::spawn({
            let hub = hub.clone();
            async move { hub.request("captcha", "x", vec![], "", "p_n", 60_000, "").await }
        });
        // 等 request 入位(poll 能看到)
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if hub.poll().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("poll 应拿到请求");
        // 先注册通知守卫并 poll 一次(Notified 首次 poll 才注册到 waiter 列表)
        let mut notified = Box::pin(hub.timeout_notified().notified());
        tokio::select! {
            _ = &mut notified => panic!("尚未触发超时, 不应收到通知"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        // 触发 notify_waiters(cancel_pending 同样走该通道)
        hub.cancel_pending();
        tokio::time::timeout(Duration::from_secs(1), notified)
            .await
            .expect("cancel_pending 后应收到 timeout_notified 通知");
        // 工具侧应收敛为 Unavailable(responder drop)
        assert_eq!(task.await.unwrap(), HumanAssistOutcome::Unavailable);
        hub.detach();
    }

    #[tokio::test]
    async fn queued_request_takes_freed_slot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        let hub = HumanAssistHub::global();
        hub.attach();
        let first = tokio::spawn({
            let hub = hub.clone();
            async move { hub.request("captcha", "first", vec![], "", "p_5", 60_000, "").await }
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
            async move { hub.request("sms", "second", vec![], "", "p_6", 60_000, "").await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let polled = hub.poll().expect("槽位仍应是第一个请求");
        assert_eq!(polled.id, d1.id);
        // respond 第一个 → 第二个入位
        assert!(hub.respond(d1.id, Some("ok".into()), AssistVia::Tui));
        assert_eq!(
            first.await.unwrap(),
            HumanAssistOutcome::Answered {
                text: "ok".into(),
                via: AssistVia::Tui
            }
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
        assert!(hub.respond(d2.id, None, AssistVia::Tui));
        assert_eq!(
            second.await.unwrap(),
            HumanAssistOutcome::Cancelled {
                via: AssistVia::Tui
            }
        );
        hub.detach();
    }

    // ---------- 第 130 轮:弹窗前端接线 ----------

    /// 弹窗应答经 oneshot 到达工具侧,channel=gui;事件环有 GuiLaunched/GuiAnswered。
    #[tokio::test]
    async fn gui_answer_reaches_tool_with_via_gui() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        fn stub(_: &HumanAssistDisplay) -> human_ui::UiResult {
            human_ui::UiResult::Answered("482913".into())
        }
        human_ui::set_test_backend(Some(stub));
        human_ui::set_test_override(Some(true));
        let hub = HumanAssistHub::global();
        let was = hub.is_attached();
        hub.detach(); // 纯弹窗路径(模拟 -p 桌面模式)
        let out = hub
            .request("sms", "验证码", vec![], "https://c", "p_g", 60_000, "")
            .await;
        assert_eq!(
            out,
            HumanAssistOutcome::Answered {
                text: "482913".into(),
                via: AssistVia::Gui
            }
        );
        let events = hub.take_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AssistEvent::GuiLaunched { id: _ })),
            "应有 GuiLaunched 事件: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                AssistEvent::GuiAnswered { text, .. } if text == "482913"
            )),
            "应有 GuiAnswered 事件: {events:?}"
        );
        reset_ui_hooks();
        if was {
            hub.attach();
        }
    }

    /// 弹窗脚本失败 → GuiFailed 事件 + gui 标记清除 → TUI 行读可无缝接管同一请求。
    #[tokio::test]
    async fn gui_failure_falls_back_to_tui_stdin() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        fn stub_err(_: &crate::agent::human_assist::HumanAssistDisplay) -> human_ui::UiResult {
            human_ui::UiResult::Error("脚本不存在".into())
        }
        human_ui::set_test_backend(Some(stub_err));
        human_ui::set_test_override(Some(true));
        let hub = HumanAssistHub::global();
        hub.attach();
        let task = tokio::spawn({
            let hub = hub.clone();
            async move { hub.request("captcha", "x", vec![], "", "p_gf", 60_000, "").await }
        });
        // 等弹窗失败降级(GuiFailed 事件 + gui 标记清除)
        let display = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(d) = hub.poll() {
                    if !hub.is_gui_presenting(d.id) && hub.take_events().iter().any(|e| matches!(e, AssistEvent::GuiFailed { .. })) {
                        break d;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("应降级为 TUI 接管");
        // TUI 行读路径照常回填
        assert!(hub.respond(display.id, Some("1".into()), AssistVia::Tui));
        assert_eq!(
            task.await.unwrap(),
            HumanAssistOutcome::Answered {
                text: "1".into(),
                via: AssistVia::Tui
            }
        );
        reset_ui_hooks();
        hub.detach();
    }

    /// 第 145 轮:「⏱ +2 分钟」延长等待 —— 过原始截止时刻槽位仍存活、应答路径
    /// 不受影响、收口后对旧 id 的延长请求失效。
    #[tokio::test(start_paused = true)]
    async fn extend_timeout_pushes_deadline_and_expires_with_slot() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        let hub = HumanAssistHub::global();
        hub.attach();
        let task = tokio::spawn({
            let hub = hub.clone();
            async move {
                hub.request("captcha", "x", vec![], "", "p_x", MIN_HUMAN_ASSIST_TIMEOUT_MS, "")
                    .await
            }
        });
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
        // 原始截止前延长 +10s(总等待 20s)
        tokio::time::advance(Duration::from_secs(9)).await;
        assert!(hub.extend_timeout(display.id, 10_000), "槽位匹配的延长应生效");
        // 过原始 10s 截止:槽位仍存活
        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(hub.poll().is_some(), "延长后不应在原截止时刻被清槽");
        // 应答照常回填
        assert!(hub.respond(display.id, Some("z7z2".into()), AssistVia::Tui));
        assert_eq!(
            task.await.unwrap(),
            HumanAssistOutcome::Answered {
                text: "z7z2".into(),
                via: AssistVia::Tui
            }
        );
        // 收口后对旧 id 的延长不再生效
        assert!(!hub.extend_timeout(display.id, 10_000));
        hub.detach();
    }

    /// 第 145 轮:延长只推迟不取消超时 —— 延长量耗尽后仍按 Timeout 收口;
    /// 累计延长封顶 = MAX_HUMAN_ASSIST_TIMEOUT_MS - 原始 timeout_ms。
    #[tokio::test(start_paused = true)]
    async fn timeout_still_fires_after_extension_expires() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        let hub = HumanAssistHub::global();
        hub.attach();
        let task = tokio::spawn({
            let hub = hub.clone();
            async move {
                hub.request("captcha", "x", vec![], "", "p_x2", MIN_HUMAN_ASSIST_TIMEOUT_MS, "")
                    .await
            }
        });
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
        // 延长到总等待上限(10s 原始 + 1790s = 1800s):单次给超大值应被 cap 收敛
        assert!(hub.extend_timeout(display.id, u64::MAX));
        assert_eq!(
            hub.extension_of(display.id),
            MAX_HUMAN_ASSIST_TIMEOUT_MS - MIN_HUMAN_ASSIST_TIMEOUT_MS,
            "累计延长应封顶在 MAX - 原始 timeout"
        );
        // id 仍匹配 → 受理返回 true,但累计量不再增长
        assert!(hub.extend_timeout(display.id, 10_000));
        assert_eq!(
            hub.extension_of(display.id),
            MAX_HUMAN_ASSIST_TIMEOUT_MS - MIN_HUMAN_ASSIST_TIMEOUT_MS,
            "到达上限后延长不再累计"
        );
        tokio::time::advance(Duration::from_secs(1_900)).await;
        assert_eq!(
            task.await.unwrap(),
            HumanAssistOutcome::Timeout {
                via: AssistVia::Tui
            }
        );
        assert!(hub.poll().is_none(), "超时后槽位应被清理");
        hub.detach();
    }

    /// 弹窗失败且无 TUI 兜底(-p 无桌面 UI 兜底):立即 Unavailable,不空等超时。
    #[tokio::test]
    async fn gui_failure_without_tui_unavailable_immediately() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_ui_hooks();
        fn stub_err(_: &crate::agent::human_assist::HumanAssistDisplay) -> human_ui::UiResult {
            human_ui::UiResult::Error("解释器缺失".into())
        }
        human_ui::set_test_backend(Some(stub_err));
        human_ui::set_test_override(Some(true));
        let hub = HumanAssistHub::global();
        let was = hub.is_attached();
        hub.detach();
        let out = tokio::time::timeout(
            Duration::from_secs(3),
            hub.request("custom", "x", vec![], "", "p_gu", 60_000, ""),
        )
        .await
        .expect("应立即收口,不等 60s");
        assert_eq!(out, HumanAssistOutcome::Unavailable);
        assert!(hub.poll().is_none());
        reset_ui_hooks();
        if was {
            hub.attach();
        }
    }
}
