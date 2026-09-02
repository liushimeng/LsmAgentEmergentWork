//! Agent 身份档案(为多 Agent 架构预留)。
//!
//! 当前仅构造一份默认 profile(`LsmAgentEmergentWork`),但 [`Agent`] 通过持有
//! [`AgentProfile`] 来获取系统提示词与工具集,后续扩展为「多 profile 切换」时,
//! Agent 循环无需改动。

use crate::tool::{builtin_registry, builtin_system_prompt, ToolRegistry};

/// 默认 Agent 名称。
pub const DEFAULT_AGENT_NAME: &str = "LsmAgentEmergentWork";

/// 一个 Agent 的身份档案:名称 / 系统提示词 / 工具集。
#[derive(Clone)]
pub struct AgentProfile {
    pub name: String,
    pub system_prompt: String,
    pub tools: ToolRegistry,
}

impl AgentProfile {
    /// 构造默认 profile(使用内置工具与系统提示词)。
    pub fn default_profile() -> Self {
        Self {
            name: DEFAULT_AGENT_NAME.to_string(),
            system_prompt: builtin_system_prompt(),
            tools: builtin_registry(),
        }
    }

    /// 自定义名称与系统提示词,仍使用内置工具集。
    pub fn new(name: impl Into<String>, system_prompt: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            system_prompt: system_prompt.into(),
            tools: builtin_registry(),
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
        assert!(!p.system_prompt.is_empty());
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
}
