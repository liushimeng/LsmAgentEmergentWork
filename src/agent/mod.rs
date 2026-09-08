//! Agent 核心循环:协议无关的 LLM 规划 -> 工具执行 -> 观察。
//!
//! 多 Agent 架构(6 角色):
//! - Yolo:入口层(任务识别 / 难度分级 / 失败回流)
//! - Plan:规划层(hard 档 Markdown 方案)
//! - Main-Work:流程层(WorkFlow 编排)
//! - SubAgent-Work:执行层(最小单元)
//! - Quality-Check:质检层
//! - SessionContext:会话层
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md`。

pub mod context;
pub mod compact;
pub mod debug;
pub mod json_repair;
pub mod main_work;
pub mod memory;
pub mod orchestrator;
pub mod permissions;
pub mod plan;
pub mod profile;
pub mod project_context;
pub mod quality;
pub mod sandbox_hook;
pub mod session_context;
pub mod subagent;
pub mod system_prompt;
pub mod tools;
pub mod yolo;

use std::sync::Arc;

use tracing::{info, warn};

use crate::agent::profile::AgentProfile;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Completion, ContentBlock, LlmClient, RequestMeta, Usage};
use crate::session::Session;

const DEFAULT_MAX_ITERATIONS: usize = 16;

/// 一个可运行的 Agent 实例。
///
/// 持有 [`AgentProfile`](profile::AgentProfile)(名称 / 系统提示词 / 工具集),
/// 为后续多 Agent 切换预留扩展口。
pub struct Agent {
    llm: Arc<dyn LlmClient>,
    profile: AgentProfile,
    max_iterations: usize,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmClient>, profile: AgentProfile) -> Self {
        Self {
            llm,
            profile,
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn llm(&self) -> Arc<dyn LlmClient> { self.llm.clone() }
    pub fn profile(&self) -> &AgentProfile { &self.profile }
    pub fn max_iterations(&self) -> usize { self.max_iterations }

    /// 单轮任务:传入用户提示,返回最终文本与本次累计 token 用量。
    pub async fn run_once(&self, user_input: &str) -> Result<(String, Usage)> {
        let mut session = Session::new();
        session.context_mut().push(ChatMessage::user(user_input));
        self.run_session(&mut session).await
    }

    /// 复用 Session 上下文的对话循环(用于 TUI 多轮对话)。
    ///
    /// 返回 `(最终回复文本, 本次循环累计 token 用量)`。后者包含所有 LLM 调用的
    /// input/output tokens 之和(由 LlmClient 在 SSE 流中收集)。
    pub async fn run_session(&self, session: &mut Session) -> Result<(String, Usage)> {
        let tool_defs = self.profile.tools.defs();
        let meta: RequestMeta = session.meta();
        let mut total_usage = Usage::default();
        let final_text;

        // 关联报告: 20260908_203854 D-001
        // 连续相同「工具名 + 目标参数」失败的短路过流:当上游 LLM 反复引导同一个
        // 失败调用(典型表现:幻觉一个不存在的文件路径后持续 Read)时,提前终止避免
        // 耗尽 max_iterations。命中条件:连续 N(默认 3)次失败且失败键(工具名+稳定
        // JSON)相同;成功调用或换工具/换目标会立即重置计数。
        const REPEATED_FAILURE_THRESHOLD: usize = 3;
        let mut last_fail_key: Option<String> = None;
        let mut consecutive_failures: usize = 0;

        for iter in 0..self.max_iterations {
            info!(iteration = iter, "agent step");
            // 按当前 LLM 协议渲染系统提示词(支持多协议差异化)
            let system = self.profile.system_prompt.render(self.llm.protocol());
            let completion: Completion = self
                .llm
                .complete(&system, session.context(), &tool_defs, &meta)
                .await?;

            // 累计 usage
            total_usage.input_tokens = total_usage.input_tokens.saturating_add(completion.usage.input_tokens);
            total_usage.output_tokens = total_usage.output_tokens.saturating_add(completion.usage.output_tokens);
            total_usage.cache_read_input_tokens =
                total_usage.cache_read_input_tokens.saturating_add(completion.usage.cache_read_input_tokens);
            total_usage.cache_creation_input_tokens =
                total_usage.cache_creation_input_tokens.saturating_add(completion.usage.cache_creation_input_tokens);

            if !completion.has_tool_calls() {
                info!("agent finished with text answer");
                if !completion.text.trim().is_empty() {
                    session.context_mut().push(ChatMessage::assistant(vec![ContentBlock::text(
                        completion.text.clone(),
                    )]));
                }
                final_text = completion.text;
                return Ok((final_text, total_usage));
            }

            // 记录 assistant 的工具调用请求(同时附带文本,如果有)
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if !completion.text.is_empty() {
                assistant_blocks.push(ContentBlock::text(completion.text.clone()));
            }
            for call in &completion.tool_calls {
                assistant_blocks.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.arguments.clone(),
                });
            }
            session.context_mut().push(ChatMessage::assistant(assistant_blocks));

            // 逐个执行工具并把结果回填上下文(失败也作为 tool_result,is_error=true)
            let mut any_success_this_round = false;
            for call in completion.tool_calls {
                let name = call.name.clone();
                let id = call.id.clone();
                let args = call.arguments;
                info!(tool = %name, "executing tool");

                let (output, is_error) = match self.profile.tools.get(&name) {
                    Ok(tool) => match tool.execute(args.clone()).await {
                        Ok(out) => (out, false),
                        Err(e) => {
                            warn!(tool = %name, error = %e, "tool failed");
                            (
                                format!("[工具执行失败] {}: {}", name, e),
                                true,
                            )
                        }
                    },
                    Err(e) => (format!("{e}"), true),
                };
                if is_error {
                    // 失败键:工具名 + 稳定 JSON(对象按 key 排序后序列化)
                    let fail_key = format!(
                        "{}|{}",
                        name,
                        stable_json_string(&args)
                    );
                    if last_fail_key.as_deref() == Some(fail_key.as_str()) {
                        consecutive_failures += 1;
                    } else {
                        last_fail_key = Some(fail_key);
                        consecutive_failures = 1;
                    }
                    if consecutive_failures >= REPEATED_FAILURE_THRESHOLD {
                        warn!(
                            tool = %name,
                            consecutive = consecutive_failures,
                            "检测到连续相同失败调用,提前终止以避免耗尽迭代"
                        );
                        return Err(AgentError::RepeatedToolFailure {
                            tool: name,
                            attempts: consecutive_failures,
                            last_error: output,
                        });
                    }
                } else {
                    any_success_this_round = true;
                }
                session
                    .context_mut()
                    .push(ChatMessage::tool_result(id, output, is_error));
            }
            if any_success_this_round {
                // 任一成功调用重置失败计数
                last_fail_key = None;
                consecutive_failures = 0;
            }
        }

        Err(AgentError::MaxIterationsExceeded(self.max_iterations))
    }
}

/// 将 `serde_json::Value` 序列化为「对象 key 排序后的字符串」,作为失败键的稳定摘要。
/// 顺序无关,LLM 调换参数顺序不触发「不同目标」误判。
fn stable_json_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = entries
                .into_iter()
                .map(|(k, v)| format!("{}:{}", k, stable_json_string(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(stable_json_string).collect();
            format!("[{}]", parts.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 关联报告: 20260908_203854 D-001
    #[test]
    fn stable_json_string_is_key_order_independent() {
        let a = json!({"path": "/tmp/missing", "limit": 10});
        let b = json!({"limit": 10, "path": "/tmp/missing"});
        assert_eq!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_nested_objects() {
        let a = json!({"outer": {"a": 1, "b": [1, 2, 3]}});
        let b = json!({"outer": {"b": [1, 2, 3], "a": 1}});
        assert_eq!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_different_values_produce_different_keys() {
        let a = json!({"path": "/tmp/missing_a"});
        let b = json!({"path": "/tmp/missing_b"});
        assert_ne!(stable_json_string(&a), stable_json_string(&b));
    }

    #[test]
    fn stable_json_string_arrays_preserve_order() {
        // 数组按设计保持顺序(LLM 调换数组元素顺序确实代表不同输入)
        let a = json!({"items": [1, 2, 3]});
        let b = json!({"items": [3, 2, 1]});
        assert_ne!(stable_json_string(&a), stable_json_string(&b));
    }
}
