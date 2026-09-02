//! 工具抽象与注册表。
//!
//! 定义 [`Tool`] trait、[`ToolRegistry`] 注册表,以及内置 Bash / Read / Write
//! 工具的注册入口 [`builtin_registry`]。

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
