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
pub mod bash_spill;
pub mod edit;
pub mod emit;
pub mod glob;
pub mod grep;
pub mod mcp_web_use;
pub mod mcp_window_use;
pub mod read;
pub mod read_detect;
pub mod write;
pub mod todo;

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

    /// 注册表内全部工具名(按注册顺序)。用于 ToolNotFound 回填时
    /// 向模型明示可用工具边界,促其立即改道(2026-09-14 第 51 轮 F1)。
    pub fn names(&self) -> Vec<&str> {
        self.order.iter().map(|s| s.as_str()).collect()
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
/// 2026-09-18 第 84 轮:macOS / Windows 追加 MCP_Window_Use(桌面窗口操控统一入口,
/// 平台门控见 [`mcp_window_use::mcp_window_use_available`])。
/// 2026-09-18 第 89 轮:全平台追加 MCP_Web_Use(浏览器网页操控统一入口,
/// CDP 三平台一致,未装浏览器返回结构化 3001 不崩溃,无需平台门控)。
pub fn builtin_registry() -> ToolRegistry {
    let sandbox = default_sandbox();
    let mut reg = ToolRegistry::new()
        .register(Arc::new(bash::BashTool))
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(write::WriteTool::new(sandbox.clone())))
        .register(Arc::new(edit::EditTool::new(sandbox.clone())))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
        .register(Arc::new(todo::TodoWriteTool::shared()))
        .register(Arc::new(mcp_web_use::McpWebUseTool));
    if mcp_window_use::mcp_window_use_available() {
        reg = reg.register(Arc::new(mcp_window_use::McpWindowUseTool));
    }
    reg
}

/// 带指定工作目录的沙箱注册表(供编排器使用)。
pub fn builtin_registry_with_work_dir(work_dir: PathBuf) -> ToolRegistry {
    let sandbox = sandbox_with(work_dir);
    let mut reg = ToolRegistry::new()
        .register(Arc::new(bash::BashTool))
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(write::WriteTool::new(sandbox.clone())))
        .register(Arc::new(edit::EditTool::new(sandbox.clone())))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
        .register(Arc::new(todo::TodoWriteTool::shared()))
        .register(Arc::new(mcp_web_use::McpWebUseTool));
    if mcp_window_use::mcp_window_use_available() {
        reg = reg.register(Arc::new(mcp_window_use::McpWindowUseTool));
    }
    reg
}

/// Yolo Agent 工具注册表:Read(理解上下文)+ 结构化输出通道
/// `submit_task_classification`(2026-09-09 第 13 轮,L6/L19)
pub fn yolo_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(emit::SubmitTaskClassification))
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
        .register(Arc::new(todo::TodoWriteTool::shared()))
}

/// SubAgent-Work Agent 工具注册表:全套工具(执行层最小单元)
pub fn sub_agent_work_registry() -> ToolRegistry {
    builtin_registry()
}

/// Quality-Check Agent 工具注册表:Read + Glob + Grep(质检时可检索与读取)
/// + 结构化输出通道 `submit_quality_report`(2026-09-09 第 13 轮,L6/L19)
pub fn quality_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(glob::GlobTool))
        .register(Arc::new(grep::GrepTool))
        .register(Arc::new(emit::SubmitQualityReport))
}

/// SessionContext Agent 工具注册表:无工具(纯文本生成)
pub fn session_context_registry() -> ToolRegistry {
    ToolRegistry::new()
}

/// Debug Agent 工具注册表:无工具(只做 trace 评估,不修改系统状态)
pub fn debug_registry() -> ToolRegistry {
    ToolRegistry::new()
}

/// Compact Agent 工具注册表:无工具(只做上下文摘要,不修改系统状态)
pub fn compact_registry() -> ToolRegistry {
    ToolRegistry::new()
}

#[cfg(test)]
mod names_tests {
    use super::*;

    #[test]
    fn yolo_registry_names_only_read_and_emit() {
        // F1(2026-09-14 第 51 轮):ToolNotFound 回填文本依赖 names() 列出
        // 可用工具边界;Yolo 注册表必须恰好是 Read + submit_task_classification。
        let reg = yolo_registry();
        let names = reg.names();
        assert_eq!(names, vec!["Read", "submit_task_classification"]);
    }

    #[test]
    fn empty_registry_names_is_empty() {
        assert!(session_context_registry().names().is_empty());
    }

    #[test]
    fn builtin_registry_mcp_window_use_platform_gated() {
        // 2026-09-18 第 84 轮:MCP_Window_Use 仅 macOS / Windows 注册。
        let reg = builtin_registry();
        let names = reg.names();
        let has = names.contains(&"MCP_Window_Use");
        assert_eq!(
            has,
            cfg!(any(target_os = "macos", target_os = "windows")),
            "MCP_Window_Use 注册应与平台门控一致: {names:?}"
        );
        let reg = builtin_registry_with_work_dir(PathBuf::from("."));
        let names = reg.names();
        assert_eq!(
            names.contains(&"MCP_Window_Use"),
            cfg!(any(target_os = "macos", target_os = "windows")),
        );
    }

    #[test]
    fn builtin_registry_mcp_web_use_all_platforms() {
        // 2026-09-18 第 89 轮:MCP_Web_Use 全平台注册(CDP 三平台一致,
        // 未装浏览器返回结构化 3001 信封,无需平台门控)。
        for reg in [builtin_registry(), builtin_registry_with_work_dir(PathBuf::from("."))] {
            let names = reg.names();
            assert!(
                names.contains(&"MCP_Web_Use"),
                "MCP_Web_Use 应注册: {names:?}"
            );
        }
    }

    #[test]
    fn yolo_and_main_work_registries_exclude_mcp_web_use() {
        // 权限面不扩大:Yolo / Main-Work / Plan / QC 不持浏览器操控工具。
        assert!(!yolo_registry().names().contains(&"MCP_Web_Use"));
        assert!(!main_work_registry().names().contains(&"MCP_Web_Use"));
        assert!(!plan_registry().names().contains(&"MCP_Web_Use"));
        assert!(!quality_registry().names().contains(&"MCP_Web_Use"));
    }

    // 第二十轮候选 5(2026-09-21):TodoWrite 注册面验证 —— SubAgent-Work 与
    // Main-Work 应看到TodoWrite;Yolo / Plan / QC 不应持(粒度对齐:规划型工具,
    // Yolo 入口层与 QC 判定层用不到)。
    #[test]
    fn todo_write_registered_in_subagent_and_main_work_only() {
        assert!(builtin_registry().names().contains(&"TodoWrite"));
        assert!(builtin_registry_with_work_dir(PathBuf::from("."))
            .names()
            .contains(&"TodoWrite"));
        assert!(main_work_registry().names().contains(&"TodoWrite"));
        // 仅规划阶段的 Yolo / Plan / QC 不应暴露。
        assert!(!yolo_registry().names().contains(&"TodoWrite"));
        assert!(!plan_registry().names().contains(&"TodoWrite"));
        assert!(!quality_registry().names().contains(&"TodoWrite"));
    }
}
