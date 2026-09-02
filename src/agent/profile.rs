//! Agent 身份档案(为多 Agent 架构预留)。
//!
//! 当前仅构造一份默认 profile(`LsmAgentEmergentWork`),但 [`Agent`] 通过持有
//! [`AgentProfile`] 来获取系统提示词与工具集,后续扩展为「多 profile 切换」时,
//! Agent 循环无需改动。

use crate::agent::system_prompt::SystemPrompt;
use crate::agent::tools::{builtin_registry, ToolRegistry};

/// 默认 Agent 名称。
pub const DEFAULT_AGENT_NAME: &str = "LsmAgentEmergentWork";

/// 一个 Agent 身份档案:名称 / 系统提示词 / 工具集。
#[derive(Clone)]
pub struct AgentProfile {
    pub name: String,
    pub system_prompt: SystemPrompt,
    pub tools: ToolRegistry,
}

impl AgentProfile {
    /// 构造默认 profile(使用内置工具与默认系统提示词)。
    pub fn default_profile() -> Self {
        Self {
            name: DEFAULT_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::default(),
            tools: builtin_registry(),
        }
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
        assert_eq!(p.name, DEFAULT_AGENT_NAME);
        // 默认系统提示词渲染后非空
        assert!(!p.system_prompt.render(crate::config::Protocol::Anthropic).is_empty());
    }

    #[test]
    fn user_agent_format() {
        let p = AgentProfile::default_profile();
        let ua = p.user_agent();
        // 形如: LsmAgentEmergentWork/0.1.0 2026-09-02 15:30:12 CST
        assert!(
            ua.starts_with(&format!("{}/", DEFAULT_AGENT_NAME)),
            "UA 应以 AgentName/ 开头: {ua}"
        );
        assert!(ua.contains('/'), "UA 应含版本号分隔符");
        // 至少含一个空格(版本号与编译时间之间)
        assert!(ua.contains(' '), "UA 应含空格分隔版本与编译时间");
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
