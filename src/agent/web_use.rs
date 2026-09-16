//! Chromium-WebUse Agent(第 11 角色,浏览器操控层):执行网页浏览与浏览器操作。
//!
//! 与 SubAgent-Work / WindowUse 平级的「专项执行单元」:由 Main-Work 拆解 WorkFlow 时按
//! `delegate_to = "webuse"` 委派,持 Read + BrowserNew / BrowserList / BrowserClose /
//! BrowserControl / BrowserInspect 工具集,CDP 协议与浏览器进程管理封闭在
//! `agent::browser` 驱动层(chromiumoxide,Chrome / Edge / Chromium,Windows/macOS/Linux)。
//!
//! 结构与 [`crate::agent::window_use::WindowUseRunner`] 完全一致:独立 sub-Session、
//! `run_session_cancellable`、ExecutionTrace、Agent-Memory 落库 —— QC /
//! SessionContext / Debug / 取消传播 / 并行调度全链路复用。
//!
//! 设计见 `docs/浏览器CDP工具/04-Chromium-WebUse-Agent设计与解决方案.md`。

use std::sync::Arc;

use tracing::warn;

use crate::agent::agent_message::AgentMessageManager;
use crate::agent::cancel::CancelToken;
use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::memory;
use crate::agent::subagent::{SubFlowInput, SubFlowOutcome};
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Usage};

/// Chromium-WebUse 执行器(浏览器网页操控专项单元)。
pub struct WebUseRunner {
    agent: Agent,
    db: Arc<Db>,
    #[allow(dead_code)]
    max_iterations: usize,
    /// Agent 间消息管理器。
    msg_mgr: AgentMessageManager,
}

impl WebUseRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::web_use_profile());
        let max_iterations = agent.max_iterations();
        let msg_mgr = AgentMessageManager::new(db.clone());
        Self {
            agent,
            db,
            max_iterations,
            msg_mgr,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self.agent = self.agent.with_max_iterations(n);
        self
    }

    /// 跑一次浏览器操控单元(不可取消版本,语义对齐 SubAgentRunner::run_unit)。
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
        // 复用 SubFlowInput 的 prompt 构造(原始 prompt 优先 + 摘要 + 上下游产物),
        // 尾部追加浏览器操控作业规范。
        let mut prompt = input.to_user_prompt();
        prompt.push_str(
            "\n\n【浏览器操控作业规范】\n\
             1. 第一步用 BrowserNew 打开目标页面拿到 page_id(默认无头内存浏览器);\n\
             2. 后续所有操作都带 page_id:BrowserControl 执行动作、BrowserInspect 观察结果;\n\
             3. 点击链接/新开标签页时,注意响应里的 spawned_page_id,操作新页面要用新 id;\n\
             4. 错误码对策:2000 → 重新 BrowserList 同步索引;2002 → 换 selector 或换 \
                input_text 的 use_js 路径;3001 → 本机未安装 Chrome/Edge/Chromium,如实告知用户;\n\
             5. 截图优先 save_path 落盘;DOM 提取注意 truncated 标记,分段提取;\n\
             6. 禁止对疑似支付/删除/确认提交类按钮做无把握点击;只读操作优先;\n\
             7. 任务完成后用 BrowserClose 关闭不再需要的页面。",
        );

        let mut sub_session = crate::session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        // 注入待处理的 Agent 消息(如有)。
        if let Some(msg) = self.msg_mgr.peek(session_id, AgentRole::WebUse).await {
            let agent_msg = format!(
                "【来自其他 Agent 的消息】\n{}\n请基于此消息继续操作。",
                msg.hint()
            );
            sub_session
                .context_mut()
                .push(ChatMessage::user(&agent_msg));
        }

        // 早终止路径语义与 SubAgentRunner / WindowUseRunner 对齐:包装成失败摘要文本 + trace,
        // 交给 Quality-Check 判定,而不是直接升级为 Error。
        let (text, usage, mut trace) = match self
            .agent
            .run_session_cancellable(&mut sub_session, cancel)
            .await
        {
            Ok((t, u, tr)) => (t, u, tr),
            Err(AgentError::RepeatedToolFailure {
                tool,
                attempts,
                last_error,
            }) => {
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
                let summary = format!("[MaxIterationsExceeded] 迭代达到 {n} 次上限未得到最终答案");
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

        // Runner 出口兜底(对齐 WindowUse P0-B):0 工具调用且无动作关键词 → 强制标 failed。
        let looks_like_action = looks_like_web_ops_action(&text);
        if trace.tool_calls == 0 && !looks_like_action {
            warn!(
                web_use_runner = "no_tool_use_no_action_text",
                text_len = text.len(),
                "WebUse 单元未发出任何工具调用,触发出口兜底"
            );
            trace.early_terminated = true;
            trace.early_terminate_reason =
                format!("no_tool_use_no_action_text:text_len={}", text.len());
            trace.collect_failure_signals(&text);
        }
        let failed = trace.is_failed();

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
            AgentRole::WebUse,
            session_id,
            &input.description,
            &text,
            error_summary,
            serde_json::json!({
                "subflow_id": &input.id,
                "expected": &input.expected_output,
                "trace": &trace,
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

/// 浏览器操作动作关键词探测(出口兜底用)。
///
/// 语义同 WindowUse 的 looks_like_window_ops_action:无工具调用时,文本含中英文
/// 动作关键词或超过 200 字符,视为「有实质内容」,否则强制标 failed。
fn looks_like_web_ops_action(text: &str) -> bool {
    if text.len() > 200 {
        return true;
    }
    const KEYWORDS: &[&str] = &[
        "已打开",
        "已点击",
        "已输入",
        "已截图",
        "已抓取",
        "已采集",
        "已登录",
        "页面标题",
        "page_id",
        "opened",
        "clicked",
        "typed",
        "screenshot",
        "navigated",
        "crawled",
        "title",
    ];
    KEYWORDS.iter().any(|k| text.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profile::WEB_USE_AGENT_NAME;
    use crate::agent::AgentProfile;

    #[test]
    fn web_use_profile_uses_web_registry() {
        let p = AgentProfile::web_use_profile();
        assert_eq!(p.name, WEB_USE_AGENT_NAME);
        let names: Vec<_> = p.tools.names().iter().map(|s| s.to_string()).collect();
        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"BrowserNew".to_string()));
        assert!(names.contains(&"BrowserList".to_string()));
        assert!(names.contains(&"BrowserClose".to_string()));
        assert!(names.contains(&"BrowserControl".to_string()));
        assert!(names.contains(&"BrowserInspect".to_string()));
        // WebUse 不带 Bash/Write(网页操控收窄权限面)
        assert!(!names.contains(&"Bash".to_string()));
        assert!(!names.contains(&"Write".to_string()));
        assert!(p.emit_tool.is_none());
    }

    #[test]
    fn runner_flags_zero_tool_calls_as_failed() {
        let text = "我需要先用 BrowserNew 打开页面,然后才能继续操作";
        assert!(!looks_like_web_ops_action(text), "无关键词不应判为动作");
        assert!(text.len() <= 200, "短文本不算有实质内容");
    }

    #[test]
    fn runner_passes_action_keyword_text_as_success() {
        for text in [
            "已打开 example.com 并截图",
            "已点击登录按钮",
            "page_id=p_ab12cd34 页面标题:Example",
            "screenshot saved",
        ] {
            assert!(
                looks_like_web_ops_action(text),
                "动作关键词应判为有动作:{text}"
            );
        }
    }

    #[test]
    fn runner_passes_long_text_as_success_via_length_fallback() {
        let long_text = "x".repeat(250);
        assert!(looks_like_web_ops_action(&long_text));
    }
}
