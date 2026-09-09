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
    builtin_registry, compact_registry, debug_registry, main_work_registry, plan_registry,
    quality_registry, session_context_registry, sub_agent_work_registry, yolo_registry,
    ToolRegistry,
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
/// Debug Agent(调试层,仅在 `-debug` 调试模式下启用)
pub const DEBUG_AGENT_NAME: &str = "LsmAgentEmergentWork-Debug";
/// Compact Agent(压缩层,Context 超阈值时自动压缩)
pub const COMPACT_AGENT_NAME: &str = "LsmAgentEmergentWork-Compact";

/// 兼容旧名(指向 SubAgent-Work)。
pub const WORK_AGENT_NAME: &str = SUB_AGENT_WORK_NAME;
/// 兼容旧名(默认 Agent)。
pub const DEFAULT_AGENT_NAME: &str = SUB_AGENT_WORK_NAME;

// =================== AgentProfile ===================

/// 一个 Agent 身份档案:名称 / 系统提示词 / 工具集 / 结构化输出通道。
#[derive(Clone)]
pub struct AgentProfile {
    pub name: String,
    pub system_prompt: SystemPrompt,
    pub tools: ToolRegistry,
    /// 结构化输出通道(emit tool,2026-09-09 第 13 轮实现 L6/L19):
    ///
    /// `Some(tool_name)` 时,Agent 循环做两件事:
    /// 1. 把 `tool_name` 注入 `RequestMeta.forced_tool` → 协议层 wire 发出
    ///    forced `tool_choice`(Anthropic `{"type":"tool"}` / OpenAI
    ///    `{"type":"function"}` 指名),模型必须以 tool_use 返回结构化结果;
    /// 2. tool_calls 命中该工具时短路(不执行),input 序列化为 ```json 块
    ///    作为最终文本 → 下游解析链(`parse_classification` 等)零改动。
    ///
    /// 对应工具定义见 `tools/emit.rs`;Provider 不支持 forced 时由
    /// `llm/resilient.rs` 自动降级为 auto 重试(全程无需用户配置)。
    pub emit_tool: Option<String>,
}

impl AgentProfile {
    /// Yolo Agent profile(入口层,任务识别 / 难度分级)。
    ///
    /// 结构化输出通道:分类结果必须经 `submit_task_classification` 工具提交
    /// (forced tool_choice,Provider 不支持时自动降级文本 JSON,见 resilient.rs)。
    pub fn yolo_profile() -> Self {
        Self {
            name: YOLO_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::yolo(),
            tools: yolo_registry(),
            emit_tool: Some(crate::agent::tools::emit::SUBMIT_TASK_CLASSIFICATION.to_string()),
        }
    }

    /// Plan Agent profile(规划层,hard 档任务)。
    pub fn plan_profile() -> Self {
        Self {
            name: PLAN_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::plan(),
            tools: plan_registry(),
            emit_tool: None,
        }
    }

    /// Main-Work Agent profile(流程层,WorkFlow 编排)。
    pub fn main_work_profile() -> Self {
        Self {
            name: MAIN_WORK_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::main_work(),
            tools: main_work_registry(),
            emit_tool: None,
        }
    }

    /// SubAgent-Work Agent profile(执行层,最小单元)。
    pub fn sub_agent_work_profile() -> Self {
        Self {
            name: SUB_AGENT_WORK_NAME.to_string(),
            system_prompt: SystemPrompt::sub_agent_work(),
            tools: sub_agent_work_registry(),
            emit_tool: None,
        }
    }

    /// Quality-Check Agent profile(质检层)。
    ///
    /// 结构化输出通道:质检结论必须经 `submit_quality_report` 工具提交。
    pub fn quality_check_profile() -> Self {
        Self {
            name: QUALITY_CHECK_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::quality_check(),
            tools: quality_registry(),
            emit_tool: Some(crate::agent::tools::emit::SUBMIT_QUALITY_REPORT.to_string()),
        }
    }

    /// SessionContext Agent profile(会话层)。
    pub fn session_context_profile() -> Self {
        Self {
            name: SESSION_CONTEXT_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::session_context(),
            tools: session_context_registry(),
            emit_tool: None,
        }
    }

    /// Debug Agent profile(调试层,任务评估 / 质量报告 / 问题报告,无工具)。
    pub fn debug_profile() -> Self {
        Self {
            name: DEBUG_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::debug(),
            tools: debug_registry(),
            emit_tool: None,
        }
    }

    /// Compact Agent profile(压缩层,上下文摘要,无工具)。
    pub fn compact_profile() -> Self {
        Self {
            name: COMPACT_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::compact(),
            tools: compact_registry(),
            emit_tool: None,
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
            emit_tool: None,
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
            emit_tool: None,
        }
    }

    /// 基于当前 profile 构造新 profile,在系统提示词末尾追加环境上下文。
    /// 结构化输出通道随原 profile 保留(emit_tool 克隆)。
    pub fn with_env_tail(&self, tail: &str) -> Self {
        Self {
            name: self.name.clone(),
            system_prompt: self.system_prompt.append_base(tail),
            tools: self.tools.clone(),
            emit_tool: self.emit_tool.clone(),
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
            AgentProfile::compact_profile().name,
        ];
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 7, "7 个 profile 必须名字互不相同");
    }

    #[test]
    fn yolo_profile_only_has_read() {
        let p = AgentProfile::yolo_profile();
        let names = tool_names(&p);
        assert!(names.contains(&"Read".to_string()));
        assert!(!names.iter().any(|n| n == "Bash"));
        assert!(!names.iter().any(|n| n == "Write"));
    }

    // ========== 结构化输出通道(L6/L19,2026-09-09 第 13 轮) ==========

    #[test]
    fn emit_tool_only_on_yolo_and_quality() {
        // Yolo / Quality 声明 emit 通道;其余角色 None
        assert_eq!(
            AgentProfile::yolo_profile().emit_tool.as_deref(),
            Some("submit_task_classification")
        );
        assert_eq!(
            AgentProfile::quality_check_profile().emit_tool.as_deref(),
            Some("submit_quality_report")
        );
        for p in [
            AgentProfile::plan_profile(),
            AgentProfile::main_work_profile(),
            AgentProfile::sub_agent_work_profile(),
            AgentProfile::session_context_profile(),
            AgentProfile::debug_profile(),
            AgentProfile::compact_profile(),
        ] {
            assert!(p.emit_tool.is_none(), "{} 不应声明 emit 工具", p.name);
        }
    }

    #[test]
    fn emit_tools_registered_in_registries() {
        let yolo_names = tool_names(&AgentProfile::yolo_profile());
        assert!(yolo_names.contains(&"submit_task_classification".to_string()));
        let q_names = tool_names(&AgentProfile::quality_check_profile());
        assert!(q_names.contains(&"submit_quality_report".to_string()));
        // emit 工具不进入其他 registry
        assert!(!tool_names(&AgentProfile::sub_agent_work_profile())
            .iter()
            .any(|n| n.starts_with("submit_")));
    }

    #[test]
    fn with_env_tail_keeps_emit_tool() {
        let p = AgentProfile::yolo_profile().with_env_tail("\n环境尾巴");
        assert_eq!(
            p.emit_tool.as_deref(),
            Some("submit_task_classification"),
            "with_env_tail 应保留结构化输出通道"
        );
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
            AgentProfile::compact_profile(),
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