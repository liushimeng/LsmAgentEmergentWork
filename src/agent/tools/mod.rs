//! 工具抽象与注册表。
//!
//! 定义 [`Tool`] trait、[`ToolRegistry`] 注册表,以及内置 Bash / Read / Write /
//! Edit / Glob / Grep 工具的注册入口 [`builtin_registry`]。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::agent::sandbox_hook::SandboxConfig;
use crate::error::{AgentError, Result};
use crate::llm::ToolDef;

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
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

/// 构造沙箱配置。
/// 实际工作目录由调用方提供;临时目录使用系统默认。
fn default_sandbox() -> SandboxConfig {
    let work_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    SandboxConfig::new(work_dir)
}

/// 从指定工作目录构造沙箱配置。
fn sandbox_with(work_dir: PathBuf) -> SandboxConfig {
    SandboxConfig::new(work_dir)
}

/// 默认注册表:内置 Bash / Read / Write / Edit / Glob / Grep(SubAgent-Work / 兼容别名)
///
/// 写操作(Write / Edit)带有沙箱拦截,限制在工作目录与系统临时目录。
pub fn builtin_registry() -> ToolRegistry {
    let sandbox = default_sandbox();
    ToolRegistry::new()
        .register(Arc::new(bash::BashTool))
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(write::WriteTool::new(sandbox.clone())))
        .register(Arc::new(edit::EditTool::new(sandbox.clone())))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
}

/// 带指定工作目录的沙箱注册表(供编排器使用)。
pub fn builtin_registry_with_work_dir(work_dir: PathBuf) -> ToolRegistry {
    let sandbox = sandbox_with(work_dir);
    ToolRegistry::new()
        .register(Arc::new(bash::BashTool))
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(write::WriteTool::new(sandbox.clone())))
        .register(Arc::new(edit::EditTool::new(sandbox.clone())))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
}

/// Yolo Agent 工具注册表:仅 Read(用于理解上下文,不修改系统状态)
pub fn yolo_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(read::ReadTool))
}

/// Plan Agent 工具注册表:Read + Write + Edit + Glob + Grep(规划与调研)
pub fn plan_registry() -> ToolRegistry {
    let sandbox = default_sandbox();
    ToolRegistry::new()
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(write::WriteTool::new(sandbox.clone())))
        .register(Arc::new(edit::EditTool::new(sandbox)))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
}

/// Main-Work Agent 工具注册表:Bash + Read + Glob + Grep(流程层可检索,不写文件)
pub fn main_work_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(bash::BashTool))
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
}

/// SubAgent-Work Agent 工具注册表:全套工具(执行层最小单元)
pub fn sub_agent_work_registry() -> ToolRegistry {
    builtin_registry()
}

/// Quality-Check Agent 工具注册表:Read + Glob + Grep(质检时可检索与读取)
pub fn quality_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
}

/// SessionContext Agent 工具注册表:无工具(纯文本生成)
pub fn session_context_registry() -> ToolRegistry {
    ToolRegistry::new()
}
