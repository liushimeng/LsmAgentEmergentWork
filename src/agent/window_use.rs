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

use crate::agent::agent_message::AgentMessageManager;
use crate::agent::cancel::CancelToken;
use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::subagent::{SubFlowInput, SubFlowOutcome};
use crate::agent::window_state::{
    build_window_state_message, build_window_state_prompt, WindowSessionState,
    WindowStateManager, WINDOW_STATE_MARKER_END, WINDOW_STATE_MARKER_START,
};
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Usage};

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
        let agent = Agent::new(llm, AgentProfile::window_use_profile());
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
    pub async fn run_unit(
        &self,
        input: &SubFlowInput,
        session_id: &str,
    ) -> Result<SubFlowOutcome> {
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

        // 复用 SubFlowInput 的 prompt 构造(原始 prompt 优先 + 摘要 + 上下游产物),
        // 尾部追加窗口操控作业规范,提醒「先检视再操作」。
        let mut prompt = input.to_user_prompt();

        // ★2) 注入窗口会话状态(如有),让 LLM 感知上一轮操作对象。
        let state_prompt = build_window_state_prompt(&win_state);
        if !state_prompt.is_empty() {
            prompt.push_str(&format!(
                "\n\n【窗口会话上下文(系统注入,非用户输入)】\n{state_prompt}"
            ));
        }

        // ★3) 注入待处理的 Agent 消息(如有),如 SubAgent 发来「请写入这段文本」。
        if let Some(msg) = self.msg_mgr.peek(session_id, AgentRole::WindowUse).await {
            prompt.push_str(&format!(
                "\n\n【来自其他 Agent 的消息】\n{}\n请基于此消息继续操作。",
                msg.hint()
            ));
        }

        prompt.push_str(
            "\n\n【窗口操控作业规范】\n\
             1. 先用 WindowList 找到目标窗口(可用 filter 过滤),再用 WindowInspect 检视控件树;\n\
             2. 依据控件 actions 列表选择合法动作,用 WindowAction 执行;路径失效时重新检视;\n\
             3. 禁止对疑似支付/删除/发送/确认类按钮做无把握点击;只读操作(list/inspect/get_text)优先;\n\
             4. macOS 提示无障碍权限未授予时,把开权限步骤写进最终回答告知用户。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        // 早终止路径语义与 SubAgentRunner 对齐:包装成失败摘要文本 + trace,
        // 交给 Quality-Check 判定,而不是直接升级为 Error。
        let (text, usage, mut trace) = match self
            .agent
            .run_session_cancellable(&mut sub_session, cancel)
            .await
        {
            Ok((t, u, tr)) => (t, u, tr),
            Err(AgentError::RepeatedToolFailure { tool, attempts, last_error }) => {
                let summary = format!(
                    "[RepeatedToolFailure] 工具 {tool} 连续 {attempts} 次失败;last_error: {last_error}"
                );
                let mut tr = ExecutionTrace::default();
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("tool={tool} attempts={attempts}");
                tr.max_consecutive_failures = attempts;
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(AgentError::MaxIterationsExceeded(n)) => {
                let summary =
                    format!("[MaxIterationsExceeded] 迭代达到 {n} 次上限未得到最终答案");
                let mut tr = ExecutionTrace::default();
                tr.iterations = n;
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("max_iter:{n}");
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(e) => return Err(e), // Cancelled / Llm 等真正错误依然上抛
        };

        trace.collect_failure_signals(&text);
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

        Ok(SubFlowOutcome { text, usage, failed, trace })
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
        let _env = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
}
