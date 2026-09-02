//! Agent 身份档案(多 Agent 架构)。
//!
//! 内置两种 Agent profile:
//! - **Work Agent** (`LsmAgentEmergentWork-Work`): 工作级 Agent,持有 Bash/Read/Write
//!   全套工具,负责实际执行任务。
//! - **Yolo Agent** (`LsmAgentEmergentWork-Yolo`): 入口级 Agent,负责目标识别、
//!   意图识别、任务分类与拆解。
//!
//! [`Agent`] 通过持有 [`AgentProfile`] 来获取系统提示词与工具集,
//! 后续扩展为更多 profile 时,Agent 循环无需改动。

use crate::agent::system_prompt::SystemPrompt;
use crate::agent::tools::{builtin_registry, yolo_registry, ToolRegistry};

/// Work Agent 名称(工作级 Agent,执行实际任务)。
pub const WORK_AGENT_NAME: &str = "LsmAgentEmergentWork-Work";

/// Yolo Agent 名称(入口级 Agent,任务识别与分类)。
pub const YOLO_AGENT_NAME: &str = "LsmAgentEmergentWork-Yolo";

/// 默认 Agent 名称(兼容别名,指向 Work Agent)。
pub const DEFAULT_AGENT_NAME: &str = WORK_AGENT_NAME;

/// 一个 Agent 身份档案:名称 / 系统提示词 / 工具集。
#[derive(Clone)]
pub struct AgentProfile {
    pub name: String,
    pub system_prompt: SystemPrompt,
    pub tools: ToolRegistry,
}

impl AgentProfile {
    /// 构造 Work Agent profile(工作级 Agent,全套 Bash/Read/Write 工具)。
    pub fn work_profile() -> Self {
        Self {
            name: WORK_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::default(),
            tools: builtin_registry(),
        }
    }

    /// 构造 Yolo Agent profile(入口级 Agent,任务识别与分类,仅含 Read 工具)。
    pub fn yolo_profile() -> Self {
        Self {
            name: YOLO_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::yolo(),
            tools: yolo_registry(),
        }
    }

    /// 构造默认 profile(兼容别名,等价于 work_profile)。
    pub fn default_profile() -> Self {
        Self::work_profile()
    }

    /// 自定义名称与系统提示词,仍使用内置工具集。
    pub fn new(name: impl Into<String>, system_prompt: SystemPrompt) -> Self {
        Self {
            name: name.into(),
            system_prompt,
            tools: builtin_registry(),
        }
    }

    /// 完全自定义(名称 / 系统提示词 / 工具集),用于多 Agent 扩展。
    pub fn with_tools(
        name: impl Into<String>,
        system_prompt: SystemPrompt,
        tools: ToolRegistry,
    ) -> Self {
        Self {
            name: name.into(),
            system_prompt,
            tools,
        }
    }

    /// 基于当前 profile 构造新 profile,在系统提示词末尾追加环境上下文。
    ///
    /// 用于单轮模式(-p / -f)注入根目录、工作目录、当前模型等信息。
    pub fn with_env_tail(&self, tail: &str) -> Self {
        Self {
            name: self.name.clone(),
            system_prompt: self.system_prompt.append_base(tail),
            tools: self.tools.clone(),
        }
    }

    /// 构造 `User-Agent` 头取值:`{AgentName}/{版本号} {编译时间}`。
    ///
    /// - 版本号取自 `CARGO_PKG_VERSION`
    /// - 编译时间取自 `LAEW_BUILD_TIME`(由 `build.rs` 注入)
    pub fn user_agent(&self) -> String {
        let version = env!("CARGO_PKG_VERSION");
        let build_time = env!("LAEW_BUILD_TIME");
        format!("{}/{version} {build_time}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_name() {
        let p = AgentProfile::default_profile();
        assert_eq!(p.name, WORK_AGENT_NAME);
        assert_eq!(p.name, DEFAULT_AGENT_NAME);
        // 默认系统提示词渲染后非空
        assert!(!p.system_prompt.render(crate::config::Protocol::Anthropic).is_empty());
    }

    #[test]
    fn work_profile_has_full_tools() {
        let p = AgentProfile::work_profile();
        assert_eq!(p.name, WORK_AGENT_NAME);
        assert!(p.tools.defs().len() >= 3, "Work Agent 应有至少 3 个工具");
    }

    #[test]
    fn yolo_profile_has_read_only() {
        let p = AgentProfile::yolo_profile();
        assert_eq!(p.name, YOLO_AGENT_NAME);
        let defs = p.tools.defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        // Yolo 只有 Read 工具
        assert!(names.contains(&"Read"), "Yolo Agent 应包含 Read 工具");
        assert!(!names.contains(&"Bash"), "Yolo Agent 不应包含 Bash 工具");
        assert!(!names.contains(&"Write"), "Yolo Agent 不应包含 Write 工具");
    }

    #[test]
    fn user_agent_format() {
        let p = AgentProfile::work_profile();
        let ua = p.user_agent();
        // 形如: LsmAgentEmergentWork-Work/0.1.0 2026-09-02 15:30:12 CST
        assert!(
            ua.starts_with(&format!("{}/", WORK_AGENT_NAME)),
            "UA 应以 WorkAgentName/ 开头: {ua}"
        );
        assert!(ua.contains('/'), "UA 应含版本号分隔符");
        // 至少含一个空格(版本号与编译时间之间)
        assert!(ua.contains(' '), "UA 应含空格分隔版本与编译时间");
    }

    #[test]
    fn yolo_user_agent_format() {
        let p = AgentProfile::yolo_profile();
        let ua = p.user_agent();
        assert!(
            ua.starts_with(&format!("{}/", YOLO_AGENT_NAME)),
            "Yolo UA 应以 YoloAgentName/ 开头: {ua}"
        );
    }

    #[test]
    fn with_tools_uses_custom_registry() {
        use crate::agent::tools::ToolRegistry;
        let profile = AgentProfile::with_tools(
            "Custom",
            SystemPrompt::without_tools("测试"),
            ToolRegistry::new(), // 空工具集
        );
        assert_eq!(profile.name, "Custom");
        assert!(profile.tools.defs().is_empty());
    }
}
