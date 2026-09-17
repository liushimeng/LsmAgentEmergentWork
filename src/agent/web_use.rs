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

/// 2026-09-16 第 64 轮:从 BrowserNew tool_result JSON 中提取 page_id 的正则。
///
/// 仅匹配 BrowserNew 成功时返回的 `data.page_id:"p_xxxxxxxx"`(8 位 hex),
/// 防止误匹配其它字段。BrowserNew 失败(code=3001 等)时无 page_id,正则不命中。
pub fn extract_page_id_from_text(text: &str) -> Option<String> {
    const PREFIX: &str = "\"page_id\":\"p_";
    let start = text.find(PREFIX)?;
    let after = &text[start + PREFIX.len()..];
    let end = after
        .find('"')
        .map(|i| start + PREFIX.len() + i + 1)
        .unwrap_or(text.len());
    let pid_start = start + PREFIX.len() - 2; // 含 "p_"
    Some(text[pid_start..end].trim_matches('"').to_string())
}

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
        // 2026-09-16 第 63 轮:WebUse 首迭代强制调用 BrowserNew。
        // 解决"16 次迭代 tool_calls=0"的根因问题:LLM 第 1 轮经常返回纯文本
        // ("让我先...")而不是直接调用 BrowserNew,导致 Agent 循环把纯文本
        // 当作成功回答返回,trace.tool_calls=0 → QC 判失败。
        // 首迭代强制 tool_choice={"type":"tool","name":"BrowserNew"},
        // 后续轮次恢复 auto,让 LLM 自由决策。
        let agent = Agent::new(llm, AgentProfile::web_use_profile())
            .with_first_iter_forced_tool("BrowserNew");
        // 2026-09-17 第 74 轮:WebUse 默认迭代上限从 16 → 32。
        // 环境变量 LAEW_WEBUSE_MAX_ITER 可覆盖(范围 8-128)。
        let default_max = std::env::var("LAEW_WEBUSE_MAX_ITER")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n >= 8 && *n <= 128)
            .unwrap_or(32);
        let agent = agent.with_max_iterations(default_max);
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
    ///
    /// 2026-09-16 第 66 轮:单元总超时(默认 300s,环境变量 LAEW_WEBUSE_TIMEOUT),
    /// 防止 Agent 循环 16 轮迭代总耗时过长(用户反馈 WebUse 任务卡住 58.8s)。
    pub async fn run_unit_with_cancel(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: &CancelToken,
    ) -> Result<SubFlowOutcome> {
        let unit_timeout = std::env::var("LAEW_WEBUSE_TIMEOUT")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300);
        tokio::time::timeout(
            std::time::Duration::from_secs(unit_timeout),
            self.run_unit_inner(input, session_id, Some(cancel)),
        )
        .await
        .map_err(|_| {
            crate::error::AgentError::Llm(format!(
                "WebUse 单元执行超时({}s),请检查网络或稍后重试",
                unit_timeout
            ))
        })?
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
             1. 第一步用 BrowserNew 打开目标页面拿到 page_id(默认无头内存浏览器 mode=hidden);\n\
             2. 后续所有操作都带 page_id:BrowserControl 执行动作、BrowserInspect 观察结果;\n\
             3. 点击链接/新开标签页时,注意响应里的 spawned_page_id,操作新页面要用新 id;\n\
             4. 错误码对策:2000 → 重新 BrowserList 同步索引;2002 → 换 selector 或换 \
                input_text 的 use_js 路径;3001 → 本机未安装 Chrome/Edge/Chromium,如实告知用户;\n\
             5. 截图优先 save_path 落盘;DOM 提取注意 truncated 标记,分段提取;\n\
             6. 禁止对疑似支付/删除/确认提交类按钮做无把握点击;只读操作优先;\n\
             7. 任务完成后用 BrowserClose 关闭不再需要的页面。\n\
             8. 浏览器启动模式:BrowserNew 默认 mode=hidden(纯 CDP 无窗口,不会弹出 macOS 系统浏览器)。\n\
                只有当用户明确要求「看截图/可视化调试」时才用 mode=headed。\n\
             9. BrowserNew 成功返回的 next_steps 字段是关键引导,里面列了 input_text/click/wait/elements\n\
                四步最常见动作的 selector_hint,严格按 next_steps 顺序执行可大幅提升成功率。\n\
             10. 复杂页面(微信公众号后台、电商后台)务必先 BrowserInspect(info=elements) 探测真实\n\
                DOM 结构(尤其是动态加载的输入框/按钮),不要凭 selector 名字硬猜。\n\
             11. ★2026-09-17 第 75 轮:若 BrowserNew 返回 code=3001(未检测到 Chrome/Edge/Chromium),\n\
                这是确定性失败(浏览器不可执行),请立即在最终回答里直接告知用户安装引导,\n\
                **不要再尝试别的浏览器启动方式**(xdg-open/open/etc 都不在 Bash 白名单),\n\
                不要循环重试 BrowserNew,不要改 mode 重试。本机没浏览器 = 任务不可完成。",
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

        // ★2026-09-17 第 75 轮:提取 Runner 角色信息,供 collect_failure_signals 计算
        // delegate_mismatch 弱信号 + TUI [路由错配] 诊断行。
        let runner_role = Some(AgentRole::WebUse);
        let intended_role = input.intended_role;

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
                tr.runner_role = runner_role;
                tr.intended_role = intended_role;
                tr.early_terminated = true;
                tr.early_terminate_reason = format!("tool={tool} attempts={attempts}");
                tr.max_consecutive_failures = attempts;
                tr.collect_failure_signals(&summary);
                (summary, Usage::default(), tr)
            }
            Err(AgentError::MaxIterationsExceeded(n)) => {
                let summary = format!("[MaxIterationsExceeded] 迭代达到 {n} 次上限未得到最终答案");
                let mut tr = ExecutionTrace::default();
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
/// 动作关键词或超过阈值,视为「有实质内容」,否则强制标 failed。
///
/// 2026-09-16 第 63 轮:阈值从 200 → 100 字符,避免"伪动作描述"逃过 QC
/// (如"我会先用 BrowserNew 打开页面..."这种纯文本意图描述)。
fn looks_like_web_ops_action(text: &str) -> bool {
    if text.len() > 100 {
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
        "已导航",
        "页面标题",
        "page_id",
        "spawned_page_id",
        "opened",
        "clicked",
        "typed",
        "screenshot",
        "navigated",
        "crawled",
        "browser_",
        "BrowserNew",
        "BrowserControl",
        "BrowserInspect",
        "title",
    ];
    KEYWORDS.iter().any(|k| text.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::profile::WEB_USE_AGENT_NAME;
    use crate::agent::AgentProfile;

    // 2026-09-16 第 64 轮:验证 page_id 提取函数对常见 BrowserNew 返回格式生效。
    #[test]
    fn extract_page_id_parses_success_envelope() {
        let txt = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","title":"文心一言","final_url":"https://wenxin.baidu.com/"}}"#;
        let pid = extract_page_id_from_text(txt);
        assert_eq!(pid.as_deref(), Some("p_a1b2c3d4"));
    }

    #[test]
    fn extract_page_id_returns_none_for_failure() {
        // code=3001 时不含 page_id
        let txt = r#"{"code":3001,"message":"未检测到浏览器","data":{"install":"请安装 Chrome"}}"#;
        assert_eq!(extract_page_id_from_text(txt), None);
    }

    #[test]
    fn extract_page_id_handles_non_json() {
        assert_eq!(extract_page_id_from_text("plain text"), None);
        assert_eq!(extract_page_id_from_text(""), None);
    }

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
        // 2026-09-16 第 63 轮:阈值从 200 → 100,纯意图描述不算动作
        let text = "我需要先打开页面然后才能继续操作";
        assert!(!looks_like_web_ops_action(text), "无关键词不应判为动作");
        assert!(text.len() <= 100, "短文本不算有实质内容");
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
