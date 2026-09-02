//! Agent 核心: 驱动 "LLM 规划 -> 工具执行 -> 观察" 的循环。

use std::sync::Arc;

use tracing::{info, warn};

use crate::error::{AgentError, Result};
use crate::llm::{Completion, LlmClient, Message, Role};
use crate::tool::ToolRegistry;

const DEFAULT_MAX_ITERATIONS: usize = 10;

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

    /// 运行一次任务, 返回最终文本答案
    pub async fn run(&self, task: &str) -> Result<String> {
        let mut messages = vec![
            Message::system(self.system_prompt.clone()),
            Message::user(task),
        ];
        let tool_schemas = self.tools.schemas();

        for iter in 0..self.max_iterations {
            info!(iteration = iter, "agent step");
            match self.llm.complete(&messages, &tool_schemas).await? {
                Completion::Text(answer) => {
                    info!("agent finished");
                    return Ok(answer);
                }
                Completion::ToolCalls(calls) => {
                    // 记录 assistant 的工具调用请求
                    messages.push(Message {
                        role: Role::Assistant,
                        content: String::new(),
                        tool_calls: Some(calls.clone()),
                        tool_call_id: None,
                    });

                    // 逐个执行工具并把结果回填上下文
                    for call in calls {
                        let name = &call.function.name;
                        let args = serde_json::from_str(&call.function.arguments)
                            .unwrap_or_else(|_| serde_json::json!({}));
                        info!(tool = %name, "executing tool");

                        let output = match self.tools.get(name) {
                            Ok(tool) => match tool.execute(args).await {
                                Ok(out) => out,
                                Err(e) => {
                                    warn!(tool = %name, error = %e, "tool failed");
                                    format!("工具执行失败: {e}")
                                }
                            },
                            Err(e) => format!("{e}"),
                        };
                        messages.push(Message::tool_result(call.id, output));
                    }
                }
            }
        }

        Err(AgentError::MaxIterationsExceeded(self.max_iterations))
    }
}
