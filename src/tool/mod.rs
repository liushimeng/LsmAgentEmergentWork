//! 工具抽象、注册表与内置工具。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AgentError, Result};
use crate::llm::ToolSchema;

/// Agent 可调用的工具
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具名(function calling 中的 name)
    fn name(&self) -> &str;

    /// 给模型看的功能描述
    fn description(&self) -> &str;

    /// 参数 JSON Schema
    fn parameters(&self) -> Value;

    /// 执行工具, 输入为模型给出的 JSON 参数
    async fn execute(&self, args: Value) -> Result<String>;

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            schema_type: "function".into(),
            function: crate::llm::FunctionSchema {
                name: self.name().to_string(),
                description: self.description().to_string(),
                parameters: self.parameters(),
            },
        }
    }
}

/// 工具注册表
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.insert(tool.name().to_string(), tool);
        self
    }

    pub fn get(&self, name: &str) -> Result<&Arc<dyn Tool>> {
        self.tools
            .get(name)
            .ok_or_else(|| AgentError::ToolNotFound(name.to_string()))
    }

    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.schema()).collect()
    }
}

/// 内置示例工具: 原样回显输入
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "将输入文本原样返回, 用于测试工具调用链路"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "要回显的文本" }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 text".into(),
            })?;
        Ok(text.to_string())
    }
}

/// 内置示例工具: 返回当前 UTC 时间
pub struct NowTool;

#[async_trait]
impl Tool for NowTool {
    fn name(&self) -> &str {
        "now"
    }

    fn description(&self) -> &str {
        "返回当前 UTC 时间(秒级 Unix 时间戳)"
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(secs.to_string())
    }
}

/// 内置工具注册表
pub fn builtin_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(EchoTool))
        .register(Arc::new(NowTool))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_tool_returns_input() {
        let out = EchoTool.execute(json!({ "text": "hello" })).await.unwrap();
        assert_eq!(out, "hello");
    }

    #[tokio::test]
    async fn registry_finds_registered_tool() {
        let reg = builtin_registry();
        assert!(reg.get("echo").is_ok());
        assert!(reg.get("missing").is_err());
    }
}
