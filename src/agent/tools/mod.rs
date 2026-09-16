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
pub mod browser;
pub mod edit;
pub mod emit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod window;
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

/// WindowUse Agent 工具注册表(第 9 角色,桌面操控层):
///
/// 2026-09-16 第 54 轮补丁 A(综合修复):
/// Read(读文件) + WindowList / WindowInspect / WindowAction(窗口操控)
/// + **Bash**(白名单模式,仅放行桌面操控类命令)。
///
/// 2026-09-16 第 56 轮:新增 WindowFind(按标题/进程名查窗口,省一次 WindowList 后
/// 人工匹配)与 WindowScreenshot(跨平台截图落盘,为后续 OCR / 视觉验证铺路)。
///
/// 设计见 `docs/WindowUse桌面窗口操控Agent/01-设计与解决方案.md` §2.5。
pub fn window_use_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(bash::BashTool))
        .register(Arc::new(window::WindowOpenTool))
        .register(Arc::new(window::WindowListTool))
        .register(Arc::new(window::WindowFindTool))
        .register(Arc::new(window::WindowInspectTool))
        .register(Arc::new(window::WindowActionTool))
        .register(Arc::new(window::WindowScreenshotTool))
}

/// Chromium-WebUse Agent 工具注册表(第 11 角色,浏览器操控层,2026-09-16 第 61 轮):
/// Read(读文件) + BrowserNew / BrowserList / BrowserClose / BrowserControl / BrowserInspect。
/// 不带 Bash/Write(网页操控单元收窄权限面)。
/// 设计见 `docs/浏览器CDP工具/04-Chromium-WebUse-Agent设计与解决方案.md`。
pub fn web_use_registry() -> ToolRegistry {
    ToolRegistry::new()
        .register(Arc::new(read::ReadTool))
        .register(Arc::new(browser::BrowserNewTool))
        .register(Arc::new(browser::BrowserListTool))
        .register(Arc::new(browser::BrowserCloseTool))
        .register(Arc::new(browser::BrowserControlTool))
        .register(Arc::new(browser::BrowserInspectTool))
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
    fn web_use_registry_names() {
        let reg = web_use_registry();
        assert_eq!(
            reg.names(),
            vec![
                "Read",
                "BrowserNew",
                "BrowserList",
                "BrowserClose",
                "BrowserControl",
                "BrowserInspect",
            ]
        );
    }

    #[test]
    fn empty_registry_names_is_empty() {
        assert!(session_context_registry().names().is_empty());
    }

    #[test]
    fn window_use_registry_names() {
        let reg = window_use_registry();
        // 2026-09-16 第 56 轮:WindowUse 工具集新增 WindowFind(标题/进程名查窗口)
        // + WindowScreenshot(截图落盘,为后续 OCR 铺路)。
        assert_eq!(
            reg.names(),
            vec![
                "Read",
                "Bash",
                "WindowOpen",
                "WindowList",
                "WindowFind",
                "WindowInspect",
                "WindowAction",
                "WindowScreenshot",
            ]
        );
    }
}
