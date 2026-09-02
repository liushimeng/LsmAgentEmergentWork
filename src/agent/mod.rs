//! Agent 核心循环:协议无关的 LLM 规划 -> 工具执行 -> 观察。

pub mod profile;
pub mod system_prompt;
pub mod tools;

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
            for call in completion.tool_calls {
                let name = call.name.clone();
                let id = call.id.clone();
                let args = call.arguments;
                info!(tool = %name, "executing tool");

                let (output, is_error) = match self.profile.tools.get(&name) {
                    Ok(tool) => match tool.execute(args).await {
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
                session.context_mut().push(ChatMessage::tool_result(id, output, is_error));
            }
        }

        Err(AgentError::MaxIterationsExceeded(self.max_iterations))
    }
}
