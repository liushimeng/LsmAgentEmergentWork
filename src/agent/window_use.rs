//! WindowUse Agent(第 9 角色,桌面操控层):执行桌面软件窗口的读取与操作。
//!
//! 与 SubAgent-Work 平级的「专项执行单元」:由 Main-Work 拆解 WorkFlow 时按
//! `delegate_to = "windowuse"` 委派,持 Read + WindowList / WindowInspect /
//! WindowAction 工具集,平台差异封闭在 `agent::window` 驱动层
//! (Windows UIA / macOS Accessibility / 其他平台 fallback)。
//!
//! 结构与 [`crate::agent::subagent::SubAgentRunner`] 完全一致:独立 sub-Session、
//! `run_session_cancellable`、ExecutionTrace、Agent-Memory 落库 —— QC /
//! SessionContext / Debug / 取消传播 / 并行调度全链路复用。
//!
//! 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md`。

use std::sync::Arc;

use tracing::warn;

use crate::agent::agent_message::AgentMessageManager;
use crate::agent::cancel::CancelToken;
use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::subagent::{SubFlowInput, SubFlowOutcome};
use crate::agent::window_state::{
    build_window_state_message, check_window_freshness, WindowFreshness, WindowSessionState,
    WindowStateManager, WINDOW_STATE_MARKER_END, WINDOW_STATE_MARKER_START,
};
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Usage};
use crate::session::now_readable;

/// WindowUse 执行器(桌面窗口操控专项单元)。
pub struct WindowUseRunner {
    agent: Agent,
    db: Arc<Db>,
    #[allow(dead_code)]
    max_iterations: usize,
    /// 窗口状态管理器(跨轮持久化)。
    state_mgr: WindowStateManager,
    /// Agent 间消息管理器。
    msg_mgr: AgentMessageManager,
}

impl WindowUseRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        // 2026-09-16 第 65 轮 P1-D:WindowUse 首迭代强制调用 WindowOpen。
        // 对齐 WebUse 第 63 轮的 BrowserNew 强制首步机制(2026-09-16 第 63 轮):
        //   - 解决"LLM 第 1 轮返回纯文本('让我先...')而非工具调用 → 16 次迭代
        //     tool_calls=0 → QC 判失败"的同类根因;
        //   - 强制 tool_choice={"type":"tool","name":"WindowOpen"},
        //     后续轮次恢复 auto,让 LLM 自由决策;
        //   - 与 Runner P0-B 兜底(looks_like_window_ops_action)形成双重防御:
        //     即便 LLM 第 1 轮不响应强制,Runner 出口也会拦截。
        let agent = Agent::new(llm, AgentProfile::window_use_profile())
            .with_first_iter_forced_tool("WindowOpen");
        let max_iterations = agent.max_iterations();
        let state_mgr = WindowStateManager::new(db.clone());
        let msg_mgr = AgentMessageManager::new(db.clone());
        Self {
            agent,
            db,
            max_iterations,
            state_mgr,
            msg_mgr,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self.agent = self.agent.with_max_iterations(n);
        self
    }

    /// 跑一次窗口操控单元(不可取消版本,语义对齐 SubAgentRunner::run_unit)。
    #[allow(dead_code)]
    pub async fn run_unit(&self, input: &SubFlowInput, session_id: &str) -> Result<SubFlowOutcome> {
        self.run_unit_inner(input, session_id, None).await
    }

    /// 可取消版本:任务级取消 token 传入 Agent 循环,LLM/工具即时中断。
    pub async fn run_unit_with_cancel(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: &CancelToken,
    ) -> Result<SubFlowOutcome> {
        self.run_unit_inner(input, session_id, Some(cancel)).await
    }

    async fn run_unit_inner(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<SubFlowOutcome> {
        // ★0) 2026-09-16 第 54 轮补丁 A:切换 Bash 工具到 WindowUse 白名单模式。
        //    (在 BashTool.execute 内 check_window_use_bash 检查 LAEW_WINDOW_USE_MODE 环境变量)
        //    Drop guard 保证函数返回时(无论 Ok / Err / early-return)自动清除,
        //    避免污染后续 SubAgent 的 Bash 调用。
        let _wu_mode_guard = WindowUseBashModeGuard::enter();

        // ★0.5) 2026-09-17 第 75 轮:提取 Runner 角色信息,供 collect_failure_signals
        // 计算 delegate_mismatch 弱信号 + TUI [路由错配] 诊断行。
        let runner_role = Some(AgentRole::WindowUse);
        let intended_role = input.intended_role;

        // ★0.8) 2026-09-17 第 77 轮 P0-1:平台权限快速检测。
        // 一次性扫描 macOS 辅助功能 + 屏幕录制状态,缺失时把降级路径注入 prompt,
        // 避免 LLM 在 WindowInspect/WindowOCR/WindowScreenshot 反复失败 14+ 次。
        // 缓存 30s(LAEW_PERMISSION_CACHE_SECS 可调),Runner 重试时零开销。
        let perm_report = crate::agent::window::check_platform_permissions();
        let permission_missing = !perm_report.has_critical_grants();

        // ★1) 加载窗口会话状态(跨轮持久化,首次为空)。
        let mut win_state = self.state_mgr.load(session_id).await.unwrap_or_else(|| {
            let mut s = WindowSessionState::new(session_id);
            // 如果 SubFlowInput 携带了窗口上下文(由 Orchestrator 注入),合并之
            if let Some(ref ctx) = input.window_context {
                s = ctx.clone();
                s.session_id = session_id.to_string();
            }
            s
        });

        // ★1.5) 2026-09-17 第 76 轮 P0-4:校验持久化窗口快照的 stale 性。
        // 上一轮持久化的 cg_window_id / window_id / PID 在新一轮可能已变化:
        //   - Fresh:直接复用,prompt 注入 last_window + 已知列表;
        //   - SamePidCgChanged:窗口重开但 PID 一致,自动更新 cg_window_id
        //     (避免 LLM 用 stale CGWindowID 调 WindowOCR 失败);
        //   - Stale:PID 变化 / 窗口已消失,从 known_windows 移除 last_window,
        //     强制 LLM 走 WindowList 重新枚举(避免 GC 报错盲调)。
        if let Some(ref last) = win_state.last_window.clone() {
            let driver = crate::agent::window::current_driver();
            let current = driver.list_windows(None).unwrap_or_default();
            let freshness = check_window_freshness(last, &current);
            match freshness {
                WindowFreshness::Fresh => {
                    tracing::debug!(
                        window_id = %last.window_id,
                        cg_window_id = ?last.cg_window_id,
                        "WindowUse state fresh, reusing cached cg_window_id"
                    );
                }
                WindowFreshness::SamePidCgChanged { old_cg, new_cg } => {
                    tracing::info!(
                        window_id = %last.window_id,
                        old_cg = ?old_cg,
                        new_cg = ?new_cg,
                        "WindowUse state cg_window_id changed (window reopened), auto-updating"
                    );
                    if let Some(ref mut last_mut) = win_state.last_window {
                        last_mut.cg_window_id = new_cg;
                        last_mut.last_seen_at = now_readable();
                    }
                }
                WindowFreshness::Stale => {
                    tracing::warn!(
                        window_id = %last.window_id,
                        pid = last.pid,
                        "WindowUse state stale (window closed or process restarted), clearing"
                    );
                    win_state
                        .known_windows
                        .retain(|w| w.window_id != last.window_id);
                    win_state.last_window = None;
                }
            }
        }

        // 复用 SubFlowInput 的 prompt 构造(原始 prompt 优先 + 摘要 + 上下游产物),
        // 尾部追加窗口操控作业规范,提醒「先检视再操作」。
        let mut prompt = input.to_user_prompt();

        // ★1.2) 2026-09-17 第 77 轮 P0-1:权限缺失时把降级路径注入 prompt。
        // LLM 第一轮响应即可拿到完整 Bash + osascript 降级文案,
        // 避免反复尝试 WindowInspect 等已知失败的工具浪费迭代次数。
        if permission_missing {
            let perm_msg = crate::agent::window::build_permission_failure_message(&perm_report);
            prompt.push_str(&perm_msg);
            tracing::warn!(
                missing = ?perm_report.missing_summary(),
                "WindowUse 检测到权限缺失,已注入降级路径到 prompt"
            );
        }

        // 2026-09-17 第 77 轮 P1-4:作业规范精简 —— 把 system_prompt 已覆盖的通用规则
        // (双路线决策 / 失败 fallback / 安全红线等)从 Runner user-prompt 中移除,
        // 仅保留**与 Runner 状态相关的指引**与跨平台别名等 LLM 不易推断的信息。
        // 此前 14 条规则合计约 2000 字符,精简到 5 条核心约 600 字符,
        // 减少首轮 system+user 双份加载带来的 token 浪费。
        prompt.push_str(
            "\n\n【Runner 补充作业规范】\n\
             1. 目标应用未打开时优先用 WindowOpen(query,app_name?);已打开(含最小化到托盘)\
                时 WindowOpen 会直接恢复+前置,**不重复启动**。Windows 微信 4.x 进程名是 \
                Weixin.exe(3.x 才是 WeChat.exe),别名表已含 WeChat/微信/Weixin。\n\
             2. macOS 上 WeChat / 部分 Electron 应用 NSWindow title 可能为空,这是正常现象,\
                WindowFind 返回 title=\"\" 时 JSON 含 note 字段说明,**应通过 process_name 定位**。\n\
             3. **【中文 UI 同义词表】**filter 失败时优先试同义词(已在 WindowInspect 工具 \
                description 中完整列出):通讯录/通信录/联系人/Contacts、按钮/Button、\
                输入框/搜索/Search/TextField/Edit、关闭/X/退出、设置/Settings/Preferences。\n\
             4. **【原子发送范式】**定位输入框后首选一个 `type_text_submit` 完成\
                键入完整内容 + Enter 提交;视觉路线传输入框中心 x/y,控件树路线先聚焦/传 path。\
                仅当应用把 Enter 定义为换行时,才用 type_text + click「发送」按钮。\
                发送后重新 WindowInspect/OCR 复查消息已出现在对话区。\n\
             5. **【效率规范 - 2026-09-17 第 77 轮】**:\n\
                - 状态块中的窗口 freshness=Fresh 时直接复用 window_id;禁止重复 \
                  WindowOpen/WindowFind/激活同一目标,避免 UI 反复前置导致焦点断续;\n\
                - 禁止 Bash 调 screencapture(WindowScreenshot 已走 CGWindow 原生路径);\n\
                - 禁止 Bash 调 osascript 枚举 UI 或获取窗口位置(WindowInspect / WindowList 已覆盖);\n\
                - Bash 仅用于:cliclick 坐标点击(控件树+视觉路线都失败时)、open 启动应用、\
                  权限未授权场景的 osascript System Events 降级路径;\n\
                - 每步操作后必须验证(WindowOCR / WindowInspect)再继续;\n\
                - 连续 3 轮无进展立即止损,不要重复相同失败操作。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        // 2026-09-16 第 58 轮 P1-A:窗口会话状态改用 build_window_state_message 注入
        // sub_session 第一条 user 消息(带 WINDOW_STATE_MARKER 锚点,可被 is_window_state_injected
        // 幂等探测),不再追加到 prompt 末尾。原因:
        // 1) 状态消息隔离成独立 turn,LLM 视觉上更易识别「系统注入」与「用户输入」;
        // 2) 锚点标记便于跨轮幂等(避免重复注入时 LLM 看到多份「窗口会话上下文」块);
        // 3) 与 SESSION_HISTORY / PROJECT_CONTEXT 等其它系统注入块对齐风格。
        if let Some(state_msg) = build_window_state_message(&win_state) {
            sub_session.context_mut().push(state_msg);
        }

        // ★3) 注入待处理的 Agent 消息(如有),如 SubAgent 发来「请写入这段文本」。
        if let Some(msg) = self.msg_mgr.peek(session_id, AgentRole::WindowUse).await {
            let agent_msg = format!(
                "【来自其他 Agent 的消息】\n{}\n请基于此消息继续操作。",
                msg.hint()
            );
            sub_session
                .context_mut()
                .push(ChatMessage::user(&agent_msg));
        }

        // 早终止路径语义与 SubAgentRunner 对齐:包装成失败摘要文本 + trace,
        // 交给 Quality-Check 判定,而不是直接升级为 Error。
        let (mut text, usage, mut trace) = match self
            .agent
            .run_session_cancellable(&mut sub_session, cancel)
            .await
        {
            Ok((t, u, tr)) => (t, u, tr),
            Err(AgentError::RepeatedToolFailure {
                tool,
                attempts,
                last_error,
                trace: carried,
            }) => {
                let summary = format!(
                    "[RepeatedToolFailure] 工具 {tool} 连续 {attempts} 次失败;last_error: {last_error}"
                );
                // 2026-09-17 第 75 轮:使用 Agent 循环携带的真实 trace(工具调用历史不丢;
                // early_terminated/reason/max_consecutive_failures 已在抛出点设置)
                let mut tr = *carried;
                tr.runner_role = runner_role;
                tr.intended_role = intended_role;
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(AgentError::MaxIterationsExceeded {
                iterations: n,
                trace: carried,
            }) => {
                let summary = format!("[MaxIterationsExceeded] 迭代达到 {n} 次上限未得到最终答案");
                // 2026-09-17 第 75 轮:使用 Agent 循环携带的真实 trace(工具调用历史不丢)
                let mut tr = *carried;
                tr.runner_role = runner_role;
                tr.intended_role = intended_role;
                tr.iterations = n;
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("max_iter:{n}");
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(e) => return Err(e), // Cancelled / Llm 等真正错误依然上抛
        };

        // 2026-09-17 第 75 轮:把 Runner 实际角色 + WorkFlow 期望角色写入 trace,
        // 供 collect_failure_signals 计算 delegate_mismatch 弱信号。
        trace.runner_role = runner_role;
        trace.intended_role = intended_role;
        // 2026-09-17 第 77 轮 P0-1:权限缺失项写入 trace,供 QC / Debug 报告观测。
        trace.permission_missing = perm_report.missing_summary();
        trace.collect_failure_signals(&text);

        // 给 QC / TUI 追加机器可验证证据,防止最终文本与真实工具轨迹相悖。
        // 2026-09-16 第 65 轮 P2:错误类型分布统计 —— 把 failure_signals 进一步分类汇总,
        // 让 TUI 阶段打印协程显示「not_trusted=0 stale_handle=2 unsupported_action=1 ...」,
        // 加速 LLM retry 阶段收敛(原版只给原始 signals 字符串,难以聚合分析)。
        let signal_counts = count_failure_signals(&trace.failure_signals);
        let error_dist = signal_counts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        // 2026-09-16 第 68 轮 P1-B:forced_tool 效果状态。
        // forced=None 表示未设置;true 表示 iter=0 LLM 确实调用了 forced tool;
        // false 表示 forced tool 被 Provider 降级或 LLM 未响应(常见根因)。
        let forced_eff_str = match trace.forced_tool_effective {
            None => "未设置".to_string(),
            Some(true) => "✅生效".to_string(),
            Some(false) => "❌未生效(可能被降级)".to_string(),
        };
        text.push_str(&format!(
            "\n\n[WindowUse执行证据] session={} iter={} tools={}(ok={},err={}) early={} forced={} signals=[{}] [错误类型分布] {}",
            session_id,
            trace.iterations,
            trace.tool_calls,
            trace.tool_calls_ok,
            trace.tool_calls_err,
            trace.early_terminated,
            forced_eff_str,
            trace.failure_signals.join(","),
            if error_dist.is_empty() { "无".to_string() } else { error_dist },
        ));

        // 2026-09-16 第 58 轮 P0-B:Runner 出口兜底。
        // 根因场景:LLM 在 WindowUse 单元里完全没发出任何工具调用(trace.tool_calls=0),
        // 但返回了一段"我先 WindowList 看看"的纯文本,Agent 循环把它当成功返回。
        // 闸门:tool_calls==0 且文本中既无动作关键词、也不够长 → 强制标 failed。
        // 配合 P0-A(LLM nudge)双重防御:即便 LLM 两次都只回文本,这里也会拦截。
        let looks_like_action = looks_like_window_ops_action(&text);
        if trace.tool_calls == 0 && !looks_like_action {
            warn!(
                window_use_runner = "p0_b_no_tool_use_no_action_text",
                text_len = text.len(),
                "WindowUse 单元未发出任何工具调用,触发出口兜底"
            );
            trace.early_terminated = true;
            trace.early_terminate_reason =
                format!("no_tool_use_no_action_text:text_len={}", text.len());
            trace.collect_failure_signals(&text);
        }
        let failed = trace.is_failed();

        // ★4) 从 trace 中提取窗口操作记录,更新窗口状态。
        //    2026-09-16 第 56 轮:ExecutionTrace.tool_call_log 落地后,这里真实生效
        //    —— 把 WindowAction / WindowList 调用写回 win_state。
        //    (Runner 也可在工具回调中直接维护 win_state,这里作为 trace-driven 兜底)
        crate::agent::window_state::extract_window_state_from_trace(&mut win_state, &trace);

        // ★5) 保存窗口状态供下一轮使用(取消路径已在上面提前返回,不会走到这里)。
        if !win_state.is_empty() {
            self.state_mgr.save(session_id, &win_state).await.ok();
        }

        let error_summary_owned: Option<String> = if failed {
            if !trace.early_terminate_reason.is_empty() {
                Some(trace.early_terminate_reason.clone())
            } else {
                Some(trace.failure_signals.join(","))
            }
        } else {
            None
        };
        let error_summary = error_summary_owned.as_deref();
        let _ = memory::record_entry(
            &self.db,
            AgentRole::WindowUse,
            session_id,
            &input.description,
            &text,
            error_summary,
            serde_json::json!({
                "subflow_id": &input.id,
                "expected": &input.expected_output,
                "trace": &trace,
                "window_state_version": win_state.version,
            }),
        );

        Ok(SubFlowOutcome {
            text,
            usage,
            failed,
            trace,
        })
    }
}

// ============== WindowUse Bash 模式 Guard(2026-09-16 第 54 轮补丁 A) ==============
//
// 在 WindowUseRunner::run_unit_inner 入口临时设置 LAEW_WINDOW_USE_MODE=1,
// 函数返回时通过 Drop 自动清除,避免污染其它 Agent 的 Bash 调用。
//
// 设计要点:
// - Drop 顺序:栈展开时 guard 后入先出,设置时压栈,清除时弹栈,严格嵌套;
// - 跨 await 边界:BashTool.execute 是异步调用,但环境变量是进程全局,
//   设置后整个进程内所有后续 Bash 调用都受 guard 影响;guard 在
//   run_unit_inner 返回(Ok / Err / panic unwind)时自动清除;
// - panic 安全:即使 WindowUse Runner panic,Drop 仍会触发(env::remove_var)。

struct WindowUseBashModeGuard {
    prev: Option<String>,
}

impl WindowUseBashModeGuard {
    fn enter() -> Self {
        let prev = std::env::var(crate::agent::tools::bash::WINDOW_USE_MODE_ENV).ok();
        // SAFETY:进程级环境变量写,主流程内对 LAEW_WINDOW_USE_MODE 的所有读写
        // 都通过 check_window_use_bash 集中,无并发竞争(单进程单线程 tokio 模型)。
        unsafe {
            std::env::set_var(crate::agent::tools::bash::WINDOW_USE_MODE_ENV, "1");
        }
        Self { prev }
    }
}

impl Drop for WindowUseBashModeGuard {
    fn drop(&mut self) {
        // SAFETY:同上,集中串行访问。
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(crate::agent::tools::bash::WINDOW_USE_MODE_ENV, v),
                None => std::env::remove_var(crate::agent::tools::bash::WINDOW_USE_MODE_ENV),
            }
        }
    }
}

/// 2026-09-16 第 65 轮 P2:失败信号分类汇总。
///
/// 把 `failure_signals` 中的原始 token 归并到 5 类结构化错误类型,供 TUI 阶段打印协程
/// 和 QC 报告使用。LLM 拿到这种分类信号后能精确决策:
/// - not_trusted → 切到 Bash 路线 / 提示用户授权;
/// - stale_handle → 重新 WindowList + WindowInspect;
/// - unsupported_action → 改换控件路径或用其它 action;
/// - platform_limit → 提示用户换平台 / 走 Bash 路线;
/// - timeout → 缩小 max_depth / 重试。
///
/// 关键词映射(启发式,基于 WindowUse 已知的 failure 类型):
/// - not_trusted: AX -25211 / NotTrusted / kAXErrorAPIDisabled / accessibility denied / 辅助功能未授权
/// - stale_handle: -25212 / StaleHandle / 路径失效 / UI 已变化 / 控件已失效 / PathInvalid
/// - unsupported_action: -25205 / -25206 / Unsupported / 控件不支持 / ActionUnsupported / AttributeUnsupported
/// - platform_limit: PlatformLimit / PlatformNotSupport / SendKeys 不支持 / 平台不支持
/// - timeout: Timeout / 超时 / killpg / sigterm
fn count_failure_signals(signals: &[String]) -> Vec<(&'static str, usize)> {
    use std::collections::BTreeMap;
    let mut bucket: BTreeMap<&'static str, usize> = BTreeMap::new();
    for s in signals {
        let lower = s.to_lowercase();
        // not_trusted
        if lower.contains("-25211")
            || lower.contains("nottrusted")
            || lower.contains("apidisabled")
            || lower.contains("accessibility denied")
            || lower.contains("辅助功能未授权")
        {
            *bucket.entry("not_trusted").or_insert(0) += 1;
        }
        // stale_handle
        else if lower.contains("-25212")
            || lower.contains("stalehandle")
            || lower.contains("路径失效")
            || lower.contains("ui 已变化")
            || lower.contains("ui已变化")
            || lower.contains("控件已失效")
            || lower.contains("pathinvalid")
            || lower.contains("越界")
        {
            *bucket.entry("stale_handle").or_insert(0) += 1;
        }
        // unsupported_action
        else if lower.contains("-25205")
            || lower.contains("-25206")
            || lower.contains("unsupported")
            || lower.contains("actionunsupported")
            || lower.contains("attributeunsupported")
            || lower.contains("控件不支持")
        {
            *bucket.entry("unsupported_action").or_insert(0) += 1;
        }
        // platform_limit
        else if lower.contains("platformlimit")
            || lower.contains("platformnotsupport")
            || lower.contains("平台不支持")
        {
            *bucket.entry("platform_limit").or_insert(0) += 1;
        }
        // timeout
        else if lower.contains("timeout") || lower.contains("超时") || lower.contains("killpg")
        {
            *bucket.entry("timeout").or_insert(0) += 1;
        }
        // bash_exit_nonzero / other
        else if lower.contains("bash_exit_nonzero") || lower.contains("exit code") {
            *bucket.entry("bash_exit_nonzero").or_insert(0) += 1;
        }
    }
    bucket.into_iter().collect()
}
///
/// 窗口操作动作关键词探测(P0-B 兜底用)。
///
/// 语义:LLM 在 WindowUse 单元里没有调用任何工具时,如果它给的文本里出现以下
/// 中英文动作关键词,说明它已描述了具体操作结果(可能是 mock 场景或纯文本推理),
/// 不算空跑;否则视为「LLM 没干活」,Runner 出口强制标 failed。
///
/// 关键词覆盖:
/// - 中文:已点击/已发送/发送成功/已输入/键入/粘贴成功/激活窗口/已打开
/// - 英文:clicked/sent/submitted/typed/windowId=/Pressed/已激活
///
/// 长度兜底:文本超过 200 字符也算「有实质内容」(复杂报告 / 路径解析 / 状态描述
/// 等都可能无关键词但仍有信息量)。
fn looks_like_window_ops_action(text: &str) -> bool {
    if text.len() > 200 {
        return true;
    }
    const KEYWORDS: &[&str] = &[
        "已点击",
        "已发送",
        "发送成功",
        "成功发送",
        "已输入",
        "键入",
        "粘贴成功",
        "激活窗口",
        "已打开",
        "clicked",
        "sent",
        "submitted",
        "typed",
        "windowId=",
        "Pressed",
        "已激活",
    ];
    KEYWORDS.iter().any(|k| text.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profile::WINDOW_USE_AGENT_NAME;
    use crate::agent::AgentProfile;

    #[test]
    fn window_use_profile_uses_window_registry() {
        let p = AgentProfile::window_use_profile();
        assert_eq!(p.name, WINDOW_USE_AGENT_NAME);
        let names: Vec<_> = p.tools.names().iter().map(|s| s.to_string()).collect();
        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"WindowList".to_string()));
        assert!(names.contains(&"WindowInspect".to_string()));
        assert!(names.contains(&"WindowAction".to_string()));
        // 2026-09-16 第 54 轮补丁 A:WindowUse 工具集扩 Bash(白名单模式)
        // - macOS AX 未授权(-25211)或用户不便授权时,改走 osascript / cliclick / screencapture 路径
        assert!(names.contains(&"Bash".to_string()));
        // WindowUse 不带 Write(写文件不属于窗口操控范围)
        assert!(!names.contains(&"Write".to_string()));
        assert!(p.emit_tool.is_none());
    }

    #[test]
    fn window_use_bash_mode_env_and_guard_combined() {
        // 2026-09-16 第 54 轮补丁 A:WindowUse Bash 白名单 + Drop guard 语义
        // 合并为单测试顺序执行:LAEW_WINDOW_USE_MODE 是进程级 env,
        // 与 cargo test 并行执行的其它 env 敏感测试(utf8_env 等)互相竞态,
        // 这里用独立 mutex 串行化。
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _env = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        use crate::agent::tools::bash::{window_use_mode, WINDOW_USE_MODE_ENV};

        // (a) 默认未设置 → 模式关闭
        unsafe {
            std::env::remove_var(WINDOW_USE_MODE_ENV);
        }
        assert!(!window_use_mode());

        // (b) 设置 "1" → 模式打开
        unsafe {
            std::env::set_var(WINDOW_USE_MODE_ENV, "1");
        }
        assert!(window_use_mode());

        // (c) Drop guard 测试:进入 WindowUse 模式后,Drop 自动还原原值
        unsafe {
            std::env::set_var(WINDOW_USE_MODE_ENV, "0");
        }
        assert!(!window_use_mode());
        {
            let _g = WindowUseBashModeGuard::enter();
            assert!(window_use_mode());
        }
        // Drop 后还原为 "0"
        assert!(!window_use_mode());
        unsafe {
            std::env::remove_var(WINDOW_USE_MODE_ENV);
        }
    }

    // ============== 2026-09-16 第 58 轮 P0-A / P0-B / P1-A 测试 ==============

    #[test]
    fn nudge_for_window_ops_triggers_on_first_iter_window_use() {
        // 2026-09-16 第 68 轮 P0-B 修复:nudge 触发范围扩展到 iter <= 2(前 3 轮均可),
        // 覆盖 iter=0 forced tool 失效 + iter=1 原 nudge + iter=2 补刀三种场景。
        use crate::agent::should_nudge_window_ops;
        let window_use_tools = ["WindowList", "Read"];
        let subagent_tools = ["Read", "Write"];

        // iter 0 + 含 WindowList → 触发(forced tool 失效时补刀)
        assert!(should_nudge_window_ops(&window_use_tools, 0));
        // iter 1 + 含 WindowList → 触发(原 nudge)
        assert!(should_nudge_window_ops(&window_use_tools, 1));
        // iter 2 + 含 WindowList → 触发(补刀)
        assert!(should_nudge_window_ops(&window_use_tools, 2));
        // iter 3 + 含 WindowList → 不触发(避免长任务污染)
        assert!(!should_nudge_window_ops(&window_use_tools, 3));
        // 不含 WindowList(SubAgent) → 不触发
        assert!(!should_nudge_window_ops(&subagent_tools, 0));
        assert!(!should_nudge_window_ops(&subagent_tools, 1));
    }

    #[test]
    fn runner_flags_zero_tool_calls_as_failed() {
        // P0-B:Runner 出口兜底——0 tool_calls + 无动作关键词 → 触发 early_terminate_reason
        let text = "我需要先用 WindowList 查一下微信窗口,然后才能继续操作";
        let looks_like_action = looks_like_window_ops_action(text);
        assert!(!looks_like_action, "无关键词不应判为动作");
        assert!(text.len() <= 200, "短文本不算有实质内容");
    }

    #[test]
    fn runner_passes_action_keyword_text_as_success() {
        // P0-B 兜底反例:含动作关键词 → 不标 failed
        let keywords_texts = [
            "已点击发送按钮",
            "已成功发送消息",
            "已输入赵玲玲并提交",
            "windowId=12345:0 clicked",
            "已激活微信窗口并定位联系人",
        ];
        for text in keywords_texts {
            assert!(
                looks_like_window_ops_action(text),
                "动作关键词应判为有动作:{text}"
            );
        }
    }

    #[test]
    fn runner_passes_long_text_as_success_via_length_fallback() {
        // P0-B 兜底:无关键词但文本长度 > 200 字符也算有实质内容(路径解析/状态描述)
        let long_text = "x".repeat(250);
        assert!(looks_like_window_ops_action(&long_text));
    }

    #[test]
    fn state_injected_as_system_message_with_marker() {
        // P1-A:窗口会话状态通过 build_window_state_message 注入 sub_session 第一条 user 消息,
        // 文本含 WINDOW_STATE_MARKER_START 锚点便于幂等探测。
        use crate::agent::window_state::{build_window_state_message, WindowSessionState};
        let mut state = WindowSessionState::new("test-session");
        state
            .window_aliases
            .insert("wechat".into(), "WeChat".into());
        let msg = build_window_state_message(&state);
        assert!(msg.is_some(), "非空状态应能构造消息");
        // content 是 Vec<ContentBlock>,取首块文本
        let blocks = &msg.unwrap().content;
        let text = blocks
            .first()
            .and_then(|b| match b {
                crate::llm::ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            text.contains(WINDOW_STATE_MARKER_START),
            "state 消息应包含 WINDOW_STATE_MARKER_START 锚点"
        );
        assert!(
            text.contains(WINDOW_STATE_MARKER_END),
            "state 消息应包含 WINDOW_STATE_MARKER_END 锚点"
        );
    }

    // ============== 2026-09-16 第 65 轮 P2 测试 ==============

    #[test]
    fn count_failure_signals_classifies_categories() {
        // 真实任务里 failure_signals 通常包含:bash_exit_nonzero / not_trusted / 等
        let signals = vec![
            "kAXErrorAPIDisabled: -25211 辅助功能未授权".to_string(),
            "AX 错误码 -25205 控件不支持".to_string(),
            "AX 错误码 -25205 控件不支持".to_string(),
            "Bash: 超时(>60000ms)被强制终止".to_string(),
            "Bash: exit code 1".to_string(),
            "路径 /0/2 在段 5 处越界".to_string(),
        ];
        let buckets = count_failure_signals(&signals);
        let map: std::collections::HashMap<_, _> = buckets.into_iter().collect();
        assert_eq!(map.get("not_trusted").copied(), Some(1));
        assert_eq!(map.get("unsupported_action").copied(), Some(2));
        assert_eq!(map.get("timeout").copied(), Some(1));
        assert_eq!(map.get("bash_exit_nonzero").copied(), Some(1));
        assert_eq!(map.get("stale_handle").copied(), Some(1));
    }

    #[test]
    fn runner_emits_error_distribution_in_evidence() {
        // P2:WindowUse 单元结束时的 text 应包含「[错误类型分布]」段。
        // 这里仅验证 error_dist 字符串拼接正确,不跑完整 Runner(避免 LLM mock)。
        let signals = vec!["kAXErrorAPIDisabled".to_string(), "路径越界".to_string()];
        let buckets = count_failure_signals(&signals);
        let distribution = buckets
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(distribution.contains("not_trusted=1"));
        assert!(distribution.contains("stale_handle=1"));
    }

    // ============== 2026-09-17 第 77 轮 P0-1 测试 ==============

    #[test]
    fn permission_message_contains_degradation_guide_when_missing() {
        // 验证权限缺失时 build_permission_failure_message 输出包含降级路径
        // 关键命令(osascript / cliclick / screencapture)。
        use crate::agent::window::{build_permission_failure_message, PermissionReport};

        // Windows 上 granted=true,无缺失
        let r = PermissionReport {
            platform: "windows".into(),
            accessibility: true,
            screen_recording: true,
            can_ocr: true,
            can_screenshot: true,
            accessibility_hint: String::new(),
            screen_recording_hint: String::new(),
        };
        let msg = build_permission_failure_message(&r);
        assert!(msg.contains("✅"), "Windows 全授权应给出成功文案: {msg}");

        // macOS 缺权限
        let r = PermissionReport {
            platform: "macos".into(),
            accessibility: false,
            screen_recording: false,
            can_ocr: false,
            can_screenshot: false,
            accessibility_hint: "授权步骤...".into(),
            screen_recording_hint: "授权步骤...".into(),
        };
        let msg = build_permission_failure_message(&r);
        assert!(msg.contains("⚠️"), "macOS 缺权限应给出警告文案");
        assert!(msg.contains("osascript"), "应包含 osascript 降级命令");
        assert!(msg.contains("cliclick"), "应包含 cliclick 降级命令");
        assert!(
            msg.contains("screencapture"),
            "应包含 screencapture 降级命令"
        );
        assert!(
            msg.contains("禁止再尝试 WindowInspect"),
            "应明确禁止重复尝试"
        );
    }

    #[test]
    fn trace_carries_permission_missing_field() {
        // 验证 ExecutionTrace 新增 permission_missing 字段,可写入 + 反序列化。
        use crate::agent::extrace::ExecutionTrace;
        let mut t = ExecutionTrace::default();
        assert!(t.permission_missing.is_empty());

        t.permission_missing = vec![
            "accessibility(macos)".into(),
            "screen_recording(macos)".into(),
        ];
        let json = serde_json::to_string(&t).expect("serialize ok");
        assert!(json.contains("permission_missing"));
        assert!(json.contains("accessibility(macos)"));

        // 反序列化恢复
        let restored: ExecutionTrace = serde_json::from_str(&json).expect("deserialize ok");
        assert_eq!(restored.permission_missing, t.permission_missing);
    }
}
