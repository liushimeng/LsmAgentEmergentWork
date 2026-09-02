//! 工具抽象与注册表。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{AgentError, Result};
use crate::llm::ToolDef;

pub mod bash;
pub mod read;
pub mod write;

/// 工具需要实现的异步 trait
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str;

    /// JSON Schema 对象
    fn parameters(&self) -> Value;

    /// 给协议无关层消费的 `ToolDef`
    fn def(&self) -> ToolDef {
        ToolDef::new(self.name(), self.description(), self.parameters())
    }

    /// 执行工具
    async fn execute(&self, args: Value) -> Result<String>;
}

/// 工具注册表(保持注册顺序,保证 tools 列表稳定)
#[derive(Default, Clone)]
pub struct ToolRegistry {
    order: Vec<String>,
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(mut self, tool: Arc<dyn Tool>) -> Self {
        let name = tool.name().to_string();
        if !self.tools.contains_key(&name) {
            self.order.push(name.clone());
        }
        self.tools.insert(name, tool);
        self
    }

    pub fn get(&self, name: &str) -> Result<&Arc<dyn Tool>> {
        self.tools
            .get(name)
            .ok_or_else(|| AgentError::ToolNotFound(name.to_string()))
    }

    /// 协议无关层的工具定义列表(按注册顺序)
    pub fn defs(&self) -> Vec<ToolDef> {
        self.order
            .iter()
            .filter_map(|n| self.tools.get(n))
            .map(|t| t.def())
            .collect()
    }
}

/// 默认注册表:内置 Bash / Read / Write
pub fn builtin_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(bash::BashTool))
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(write::WriteTool))
}

/// 给模型看的统一系统说明(描述当前可用的工具)
pub fn builtin_system_prompt() -> String {
    String::from(
        "你是一个基于工具调用的 Agent。可使用工具完成任务,完成后用一段简洁中文回答用户。\n\
         工具调用规范:\n\
         - 仅在必要时调用工具;能用更专用工具(如 Read/Write)完成的事不要退化为 Bash。\n\
         - 工具参数需严格遵守给定 JSON Schema。\n\
         - 并行无依赖的工具调用请一次性发出。\n\n可用工具:\n\
         - Bash(command, timeout_ms?, description?): 在工作目录下执行 bash 命令并返回 stdout/stderr/退出码。\n\
         - Read(file_path, offset?, limit?): 读取文本文件,带行号。offset/limit 用于分页。\n\
         - Write(file_path, content): 覆盖写入(或新建)文件,自动创建父目录。\n",
    )
}
