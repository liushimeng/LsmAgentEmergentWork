//! Agent 核心循环:协议无关的 LLM 规划 -> 工具执行 -> 观察。

use std::sync::Arc;

use tracing::{info, warn};

use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Completion, ContentBlock, LlmClient};
use crate::tool::ToolRegistry;

const DEFAULT_MAX_ITERATIONS: usize = 16;

/// 一个可运行的 Agent 实例
pub struct Agent {
    llm: Arc<dyn LlmClient>,
    tools: ToolRegistry,
    system_prompt: String,
    max_iterations: usize,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmClient>, tools: ToolRegistry, system_prompt: impl Into<String>) -> Self {
        Self {
            llm,
            tools,
            system_prompt: system_prompt.into(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn llm(&self) -> Arc<dyn LlmClient> { self.llm.clone() }
    pub fn tools(&self) -> &ToolRegistry { &self.tools }
    pub fn system_prompt(&self) -> &str { &self.system_prompt }
    pub fn max_iterations(&self) -> usize { self.max_iterations }

    /// 单轮任务:传入用户提示,返回最终文本
    pub async fn run_once(&self, user_input: &str) -> Result<String> {
        let mut history = vec![ChatMessage::user(user_input)];
        self.run_with_history(&mut history).await
    }

    /// 复用历史上下文的对话循环(用于 TUI 多轮对话)
    pub async fn run_with_history(&self, history: &mut Vec<ChatMessage>) -> Result<String> {
        let tool_defs = self.tools.defs();

        for iter in 0..self.max_iterations {
            info!(iteration = iter, "agent step");
            let completion: Completion = self
                .llm
                .complete(&self.system_prompt, history, &tool_defs)
                .await?;

            if !completion.has_tool_calls() {
                info!("agent finished with text answer");
                if !completion.text.trim().is_empty() {
                    history.push(ChatMessage::assistant(vec![ContentBlock::text(
                        completion.text.clone(),
                    )]));
                }
                return Ok(completion.text);
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
            history.push(ChatMessage::assistant(assistant_blocks));

            // 逐个执行工具并把结果回填上下文(失败也作为 tool_result,is_error=true)
            for call in completion.tool_calls {
                let name = call.name.clone();
                let id = call.id.clone();
                let args = call.arguments;
                info!(tool = %name, "executing tool");

                let (output, is_error) = match self.tools.get(&name) {
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
                history.push(ChatMessage::tool_result(id, output, is_error));
            }
        }

        Err(AgentError::MaxIterationsExceeded(self.max_iterations))
    }
}
