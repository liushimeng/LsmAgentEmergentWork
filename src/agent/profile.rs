//! Agent 身份档案(多 Agent 架构 — 6 角色)。
//!
//! 内置 6 个 Agent profile:
//! - **Yolo**(`LsmAgentEmergentWork-Yolo`):入口层,任务识别 / 难度分级 / 失败回流。
//! - **Plan**(`LsmAgentEmergentWork-Plan`):规划层,hard 档任务产出 Markdown 方案。
//! - **Main-Work**(`LsmAgentEmergentWork-Main-Work`):流程层,WorkFlow 编排。
//! - **SubAgent-Work**(`LsmAgentEmergentWork-SubAgent-Work`):执行层最小单元,实际执行子任务。
//! - **Quality-Check**(`LsmAgentEmergentWork-Quality-Check`):质检层,单元输出校验。
//! - **SessionContext**(`LsmAgentEmergentWork-SessionContext`):会话层,Session 摘要串联。
//!
//! [`Agent`] 通过持有 [`AgentProfile`] 来获取系统提示词与工具集。
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md`。

use crate::agent::system_prompt::SystemPrompt;
use crate::agent::tools::{
    builtin_registry, main_work_registry, plan_registry, quality_registry, session_context_registry,
    sub_agent_work_registry, yolo_registry, ToolRegistry,
};

// =================== Agent 名称常量 ===================

/// Yolo Agent(入口层)
pub const YOLO_AGENT_NAME: &str = "LsmAgentEmergentWork-Yolo";
/// Plan Agent(规划层)
pub const PLAN_AGENT_NAME: &str = "LsmAgentEmergentWork-Plan";
/// Main-Work Agent(流程层)
pub const MAIN_WORK_AGENT_NAME: &str = "LsmAgentEmergentWork-Main-Work";
/// SubAgent-Work Agent(执行层)
pub const SUB_AGENT_WORK_NAME: &str = "LsmAgentEmergentWork-SubAgent-Work";
/// Quality-Check Agent(质检层)
pub const QUALITY_CHECK_AGENT_NAME: &str = "LsmAgentEmergentWork-Quality-Check";
/// SessionContext Agent(会话层)
pub const SESSION_CONTEXT_AGENT_NAME: &str = "LsmAgentEmergentWork-SessionContext";

/// 兼容旧名(指向 SubAgent-Work)。
pub const WORK_AGENT_NAME: &str = SUB_AGENT_WORK_NAME;
/// 兼容旧名(默认 Agent)。
pub const DEFAULT_AGENT_NAME: &str = SUB_AGENT_WORK_NAME;

// =================== AgentProfile ===================

/// 一个 Agent 身份档案:名称 / 系统提示词 / 工具集。
#[derive(Clone)]
pub struct AgentProfile {
    pub name: String,
    pub system_prompt: SystemPrompt,
    pub tools: ToolRegistry,
}

impl AgentProfile {
    /// Yolo Agent profile(入口层,任务识别 / 难度分级)。
    pub fn yolo_profile() -> Self {
        Self {
            name: YOLO_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::yolo(),
            tools: yolo_registry(),
        }
    }

    /// Plan Agent profile(规划层,hard 档任务)。
    pub fn plan_profile() -> Self {
        Self {
            name: PLAN_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::plan(),
            tools: plan_registry(),
        }
    }

    /// Main-Work Agent profile(流程层,WorkFlow 编排)。
    pub fn main_work_profile() -> Self {
        Self {
            name: MAIN_WORK_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::main_work(),
            tools: main_work_registry(),
        }
    }

    /// SubAgent-Work Agent profile(执行层,最小单元)。
    pub fn sub_agent_work_profile() -> Self {
        Self {
            name: SUB_AGENT_WORK_NAME.to_string(),
            system_prompt: SystemPrompt::sub_agent_work(),
            tools: sub_agent_work_registry(),
        }
    }

    /// Quality-Check Agent profile(质检层)。
    pub fn quality_check_profile() -> Self {
        Self {
            name: QUALITY_CHECK_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::quality_check(),
            tools: quality_registry(),
        }
    }

    /// SessionContext Agent profile(会话层)。
    pub fn session_context_profile() -> Self {
        Self {
            name: SESSION_CONTEXT_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::session_context(),
            tools: session_context_registry(),
        }
    }

    /// 兼容旧名(等价于 sub_agent_work_profile)。
    pub fn work_profile() -> Self {
        Self::sub_agent_work_profile()
    }

    /// 构造默认 profile(兼容别名,等价于 sub_agent_work_profile)。
    pub fn default_profile() -> Self {
        Self::sub_agent_work_profile()
    }

    /// 自定义名称与系统提示词,仍使用内置工具集(SubAgent-Work 全套)。
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
    pub fn with_env_tail(&self, tail: &str) -> Self {
        Self {
            name: self.name.clone(),
            system_prompt: self.system_prompt.append_base(tail),
            tools: self.tools.clone(),
        }
    }

    /// 构造 `User-Agent` 头取值:`{AgentName}/{版本号} {编译时间}`。
    pub fn user_agent(&self) -> String {
        let version = env!("CARGO_PKG_VERSION");
        let build_time = env!("LAEW_BUILD_TIME");
        format!("{}/{version} {build_time}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Protocol;

    fn tool_names(p: &AgentProfile) -> Vec<String> {
        p.tools.defs().iter().map(|d| d.name.clone()).collect()
    }

    #[test]
    fn six_profiles_have_distinct_names() {
        let names = [
            AgentProfile::yolo_profile().name,
            AgentProfile::plan_profile().name,
            AgentProfile::main_work_profile().name,
            AgentProfile::sub_agent_work_profile().name,
            AgentProfile::quality_check_profile().name,
            AgentProfile::session_context_profile().name,
        ];
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 6, "6 个 profile 必须名字互不相同");
    }

    #[test]
    fn yolo_profile_only_has_read() {
        let p = AgentProfile::yolo_profile();
        let names = tool_names(&p);
        assert!(names.contains(&"Read".to_string()));
        assert!(!names.iter().any(|n| n == "Bash"));
        assert!(!names.iter().any(|n| n == "Write"));
    }

    #[test]
    fn plan_profile_has_read_and_write() {
        let p = AgentProfile::plan_profile();
        let names = tool_names(&p);
        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"Write".to_string()));
        assert!(!names.iter().any(|n| n == "Bash"));
    }

    #[test]
    fn main_work_has_bash_and_read() {
        let p = AgentProfile::main_work_profile();
        let names = tool_names(&p);
        assert!(names.contains(&"Bash".to_string()));
        assert!(names.contains(&"Read".to_string()));
        assert!(!names.iter().any(|n| n == "Write"));
    }

    #[test]
    fn sub_agent_has_all_three() {
        let p = AgentProfile::sub_agent_work_profile();
        let names = tool_names(&p);
        assert!(names.contains(&"Bash".to_string()));
        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"Write".to_string()));
    }

    #[test]
    fn quality_session_no_or_one_tool() {
        let q = AgentProfile::quality_check_profile();
        let names = tool_names(&q);
        // Quality 可选 Read
        assert!(names.contains(&"Read".to_string()) || names.is_empty());

        let s = AgentProfile::session_context_profile();
        assert!(tool_names(&s).is_empty());
    }

    #[test]
    fn work_alias_points_to_sub_agent() {
        let p = AgentProfile::work_profile();
        assert_eq!(p.name, SUB_AGENT_WORK_NAME);
        assert_eq!(p.name, WORK_AGENT_NAME);
        assert_eq!(p.name, DEFAULT_AGENT_NAME);
    }

    #[test]
    fn system_prompt_render_not_empty() {
        for p in [
            AgentProfile::yolo_profile(),
            AgentProfile::plan_profile(),
            AgentProfile::main_work_profile(),
            AgentProfile::sub_agent_work_profile(),
            AgentProfile::quality_check_profile(),
            AgentProfile::session_context_profile(),
        ] {
            let rendered = p.system_prompt.render(Protocol::Anthropic);
            assert!(!rendered.is_empty(), "{} 的系统提示词渲染不应为空", p.name);
            assert!(
                rendered.contains(p.name.as_str()),
                "{} 提示词应包含自身名称",
                p.name
            );
        }
    }

    #[test]
    fn user_agent_format() {
        let p = AgentProfile::sub_agent_work_profile();
        let ua = p.user_agent();
        assert!(ua.starts_with(&format!("{}/", SUB_AGENT_WORK_NAME)));
        assert!(ua.contains('/'));
        assert!(ua.contains(' '));
    }
}