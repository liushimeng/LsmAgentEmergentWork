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
pub mod subagent;
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

    /// 生成工具子集(2026-09-22 第 114 轮,动态子 Agent 工具收窄用)。
    ///
    /// - `keep` 非空 = 白名单(其余全部剔除);
    /// - `drop` = 黑名单(优先级高于白名单,用于剔除 `SubAgent` 让子 Agent 成为叶子);
    /// - 保持原注册顺序,保证子 Agent 的 tools 列表稳定。
    pub fn subset(&self, keep: &[&str], drop: &[&str]) -> ToolRegistry {
        let mut out = ToolRegistry::new();
        for name in &self.order {
            if drop.contains(&name.as_str()) {
                continue;
            }
            if !keep.is_empty() && !keep.contains(&name.as_str()) {
                continue;
            }
            if let Some(t) = self.tools.get(name) {
                out = out.register(t.clone());
            }
        }
        out
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

/// 条件注册 `SubAgent` 工具(2026-09-22 第 114 轮)。
///
/// 关闭语义:`LAEW_SELF_SPAWN=off` 或 `LAEW_SUBAGENT_MAX_DEPTH=0` 时
/// **不注册工具**,工具面与自感知提示词同时归零(严格向后兼容)。
fn register_subagent(reg: ToolRegistry) -> ToolRegistry {
    let cfg = crate::agent::self_awareness::config();
    if cfg.enabled && cfg.max_depth > 0 {
        reg.register(Arc::new(subagent::SubAgentTool))
    } else {
        reg
    }
}

/// 默认注册表:内置 Bash / Read / Write / Edit / Glob / Grep(SubAgent-Work / 兼容别名)
///
/// 写操作(Write / Edit)带有沙箱拦截,限制在工作目录与系统临时目录。
/// 2026-09-18 第 84 轮:macOS / Windows 追加 MCP_Window_Use(桌面窗口操控统一入口,
/// 平台门控见 [`mcp_window_use::mcp_window_use_available`])。
/// 2026-09-18 第 89 轮:全平台追加 MCP_Web_Use(浏览器网页操控统一入口,
/// CDP 三平台一致,未装浏览器返回结构化 3001 不崩溃,无需平台门控)。
/// 2026-09-22 第 114 轮:追加 SubAgent(自感知动态启动子 Agent 工具,
/// 运行时经 task-local 注入;总开关 `LAEW_SELF_SPAWN=off` 时不注册)。
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
    reg = register_subagent(reg);
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
    reg = register_subagent(reg);
    if mcp_window_use::mcp_window_use_available() {
        reg = reg.register(Arc::new(mcp_window_use::McpWindowUseTool));
    }
    reg
}

/// Yolo Agent 工具注册表:Read(理解上下文)+ 结构化输出通道
/// `submit_task_classification`(2026-09-09 第 13 轮,L6/L19)
/// + SubAgent(第 114 轮:只读子 Agent 侦察,策略 `ReadOnlyChildren`)。
pub fn yolo_registry() -> ToolRegistry {
    register_subagent(
        ToolRegistry::new()
            .register(Arc::new(read::ReadTool))
            .register(Arc::new(emit::SubmitTaskClassification)),
    )
}

/// Plan Agent 工具注册表:Read + Write + Edit + Glob + Grep(规划与调研)
/// + SubAgent(第 114 轮:并行方案调研,策略 `FullChildren`)。
pub fn plan_registry() -> ToolRegistry {
    let sandbox = default_sandbox();
    register_subagent(
        ToolRegistry::new()
            .register(Arc::new(read::ReadTool))
            .register(Arc::new(write::WriteTool::new(sandbox.clone())))
            .register(Arc::new(edit::EditTool::new(sandbox)))
            .register(Arc::new(glob::GlobTool))
            .register(Arc::new(grep::GrepTool)),
    )
}

/// Main-Work Agent 工具注册表:Bash + Read + Glob + Grep(流程层可检索,不写文件)
/// + SubAgent(第 114 轮:把独立 WorkFlow 交给并行子 Agent)。 
pub fn main_work_registry() -> ToolRegistry {
    register_subagent(
        ToolRegistry::new()
            .register(Arc::new(bash::BashTool))
            .register(Arc::new(read::ReadTool))
            .register(Arc::new(glob::GlobTool))
            .register(Arc::new(grep::GrepTool))
            .register(Arc::new(todo::TodoWriteTool::shared())),
    )
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
        // 可用工具边界;Yolo 注册表必须恰好是 Read + submit_task_classification
        // (+ 第 114 轮的自感知委派工具 SubAgent,Yolo 策略为 ReadOnlyChildren)。
        let reg = yolo_registry();
        let names = reg.names();
        assert_eq!(names, vec!["Read", "submit_task_classification", "SubAgent"]);
    }

    // ========== 第 114 轮(2026-09-22):自感知 SubAgent 动态启动 注册面 ==========

    #[test]
    fn subagent_tool_registered_for_delegating_roles_only() {
        // 可委派角色(入口 / 规划 / 流程 / 执行)持 SubAgent。
        for (label, reg) in [
            ("yolo", yolo_registry()),
            ("plan", plan_registry()),
            ("main_work", main_work_registry()),
            ("sub_agent_work", sub_agent_work_registry()),
            ("builtin_with_work_dir", builtin_registry_with_work_dir(PathBuf::from("."))),
        ] {
            assert!(
                reg.names().contains(&"SubAgent"),
                "{label} 应登记 SubAgent 工具: {:?}",
                reg.names()
            );
        }
        // 质检 / 会话 / 调试 / 压缩角色必须保持单线程语义:不得持委派工具。
        for (label, reg) in [
            ("quality", quality_registry()),
            ("session_context", session_context_registry()),
            ("debug", debug_registry()),
            ("compact", compact_registry()),
        ] {
            assert!(
                !reg.names().contains(&"SubAgent"),
                "{label} 不应登记 SubAgent 工具: {:?}",
                reg.names()
            );
        }
    }

    #[test]
    fn subagent_tool_schema_and_description_cover_all_actions() {
        let reg = sub_agent_work_registry();
        let tool = reg.get("SubAgent").expect("SubAgent 工具已注册");
        let schema = tool.parameters();
        let actions: Vec<&str> = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action.enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // 与实现常量逐字对齐(第 115 轮增 history / resume),避免手工维护清单漂移
        assert_eq!(
            actions,
            crate::agent::tools::subagent::action_names().to_vec(),
            "action 枚举应与实现一致"
        );
        assert_eq!(schema["required"][0], "action");
        // description 必须把「何时不要启动」写清楚(防模型滥用)
        let desc = tool.description();
        for needle in [
            "何时启动",
            "何时不要启动",
            "batch",
            "4001",
            "自包含",
            // 第 115 轮:自定义类型必须在描述里可见(否则模型不知道能用)
            ".laew/agents",
            "resume",
            "history",
        ] {
            assert!(desc.contains(needle), "description 应含 `{needle}`");
        }
        // agent_type 不得退回 enum(否则 tool_schema_validator 会硬拒自定义 id)
        assert!(
            schema["properties"]["agent_type"].get("enum").is_none(),
            "agent_type 必须是自由字符串(自定义类型不可枚举)"
        );
    }

    #[test]
    fn tool_registry_subset_narrows_and_drops() {
        // subset 是动态子 Agent 工具收窄的核心原语:白名单 + 黑名单 + 顺序保持。
        let reg = sub_agent_work_registry();
        let narrowed = reg.subset(&["Read", "Grep"], &[]);
        assert_eq!(narrowed.names(), vec!["Read", "Grep"], "白名单且保持注册顺序");
        let dropped = reg.subset(&[], &["SubAgent"]);
        assert!(!dropped.names().contains(&"SubAgent"), "黑名单应剔除 SubAgent");
        assert!(dropped.names().contains(&"Bash"), "黑名单不应影响其它工具");
        let both = reg.subset(&["SubAgent", "Read"], &["SubAgent"]);
        assert_eq!(both.names(), vec!["Read"], "黑名单优先于白名单");
        let empty_keep = reg.subset(&[], &[]);
        assert_eq!(empty_keep.names(), reg.names(), "空白名单=全量");
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
