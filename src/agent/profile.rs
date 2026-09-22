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
use crate::agent::self_awareness::{self as sa, SpawnPolicy};
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
/// WorkFlow Agent(工作流编排层,第 10 角色:超大型复杂任务自动化编排)
pub const WORK_FLOW_AGENT_NAME: &str = "LsmAgentEmergentWork-WorkFlow";

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
    /// 动态子 Agent 能力策略(第 114 轮,2026-09-22):
    /// `Disabled` = 不参与动态启动(QC / SessionContext / Debug / Compact);
    /// `ReadOnlyChildren` = 只能启动只读子 Agent(入口层 Yolo);
    /// `FullChildren` = 可启动完整执行层子 Agent(Plan / Main-Work / SubAgent-Work / WorkFlow)。
    /// 与系统提示词里的自感知段(§5.1.3)严格一致:策略 Disabled 时不注入该段。
    pub spawn_policy: SpawnPolicy,
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
    /// 结构化输出强制通道时机(2026-09-22 Yolo ReAct 改造):
    ///
    /// `false`(默认;QC / Plan / Main-Work / SubAgent-Work / SessionContext / Debug / Compact
    /// / WorkFlow / 动态子 Agent):每轮请求都注入 `forced_tool` —— 强制通道与多轮探索互斥,
    /// 只能 1 轮直答。
    ///
    /// `true`(仅 Yolo):探索轮(`iter + 1 < max_iterations`)不注入 `forced_tool`,
    /// 模型自由 ReAct —— 在多轮 Thought→Action→Observation 循环中自主调用信息收集工具;
    /// 迭代预算最后一轮强制 emit 收口(保底结构化输出),倒数第二轮追加收口预告 hint 平滑
    /// 交卷。详见 `docs/YoloAgent设计/03-Yolo工具集扩展与ReAct信息收集设计.md`。
    pub defer_emit_force: bool,
}

impl AgentProfile {
    /// Yolo Agent profile(入口层,任务识别 / 难度分级)。
    ///
    /// 结构化输出通道:分类结果必须经 `submit_task_classification` 工具提交。
    /// 2026-09-22 ReAct 改造:开启 `defer_emit_force`,允许 Yolo 在分类前按
    /// Thought→Action→Observation 循环自主调用 Read/Glob/Grep/Bash/MCP_Web_Use
    /// 收集信息;仅迭代预算最后一轮强制 emit 收口。
    pub fn yolo_profile() -> Self {
        let mut p = Self::with_self_awareness(
            YOLO_AGENT_NAME,
            SpawnPolicy::ReadOnlyChildren,
            SystemPrompt::yolo(),
            yolo_registry,
            Some(crate::agent::tools::emit::SUBMIT_TASK_CLASSIFICATION.to_string()),
        );
        p.defer_emit_force = true;
        p
    }

    /// Plan Agent profile(规划层,hard 档任务)。
    pub fn plan_profile() -> Self {
        Self::with_self_awareness(
            PLAN_AGENT_NAME,
            SpawnPolicy::FullChildren,
            SystemPrompt::plan(),
            plan_registry,
            None,
        )
    }

    /// Main-Work Agent profile(流程层,WorkFlow 编排)。
    pub fn main_work_profile() -> Self {
        Self::with_self_awareness(
            MAIN_WORK_AGENT_NAME,
            SpawnPolicy::FullChildren,
            SystemPrompt::main_work(),
            main_work_registry,
            None,
        )
    }

    /// SubAgent-Work Agent profile(执行层,最小单元)。
    pub fn sub_agent_work_profile() -> Self {
        Self::with_self_awareness(
            SUB_AGENT_WORK_NAME,
            SpawnPolicy::FullChildren,
            SystemPrompt::sub_agent_work(),
            sub_agent_work_registry,
            None,
        )
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
            spawn_policy: SpawnPolicy::Disabled,
            defer_emit_force: false,
        }
    }

    /// SessionContext Agent profile(会话层)。
    pub fn session_context_profile() -> Self {
        Self {
            name: SESSION_CONTEXT_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::session_context(),
            tools: session_context_registry(),
            emit_tool: None,
            spawn_policy: SpawnPolicy::Disabled,
            defer_emit_force: false,
        }
    }

    /// Debug Agent profile(调试层,任务评估 / 质量报告 / 问题报告,无工具)。
    pub fn debug_profile() -> Self {
        Self {
            name: DEBUG_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::debug(),
            tools: debug_registry(),
            emit_tool: None,
            spawn_policy: SpawnPolicy::Disabled,
            defer_emit_force: false,
        }
    }

    /// Compact Agent profile(压缩层,上下文摘要,无工具)。
    pub fn compact_profile() -> Self {
        Self {
            name: COMPACT_AGENT_NAME.to_string(),
            system_prompt: SystemPrompt::compact(),
            tools: compact_registry(),
            emit_tool: None,
            spawn_policy: SpawnPolicy::Disabled,
            defer_emit_force: false,
        }
    }

    /// WorkFlow Agent profile(工作流编排层,第 10 角色:Goal 状态机 + Squad 调度)。
    /// 工具集:Bash(执行编排命令) + Read(查看状态) + Write(产出报告)。
    pub fn work_flow_profile() -> Self {
        Self::with_self_awareness(
            WORK_FLOW_AGENT_NAME,
            SpawnPolicy::FullChildren,
            SystemPrompt::work_flow(),
            sub_agent_work_registry,
            None,
        )
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
            spawn_policy: SpawnPolicy::FullChildren,
            defer_emit_force: false,
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
            spawn_policy: SpawnPolicy::FullChildren,
            defer_emit_force: false,
        }
    }

    /// 动态子 Agent profile(2026-09-22 第 114 轮)。
    ///
    /// 由 `dynamic_subagent` 运行时组装:类型提示词 + 收窄后的工具集 +
    /// (可继续委派时)自感知段。工具面**不含**超出父能力上限的工具。
    pub fn dynamic_child(
        name: impl Into<String>,
        base_prompt: String,
        tools: ToolRegistry,
        spawn_policy: SpawnPolicy,
    ) -> Self {
        let name = name.into();
        let prompt = build_self_aware_prompt(&name, SystemPrompt::new(base_prompt), &tools, spawn_policy);
        Self {
            name,
            system_prompt: prompt,
            tools,
            emit_tool: None,
            spawn_policy,
            defer_emit_force: false,
        }
    }

    /// 内置角色 profile 的公共装配:先建注册表,再由注册表生成自感知段。
    fn with_self_awareness(
        name: &str,
        spawn_policy: SpawnPolicy,
        prompt: SystemPrompt,
        registry: fn() -> ToolRegistry,
        emit_tool: Option<String>,
    ) -> Self {
        let tools = registry();
        let prompt = build_self_aware_prompt(name, prompt, &tools, spawn_policy);
        Self {
            name: name.to_string(),
            system_prompt: prompt,
            tools,
            emit_tool,
            spawn_policy,
            defer_emit_force: false,
        }
    }

    /// 基于当前 profile 构造新 profile,在系统提示词末尾追加环境上下文。
    /// 结构化输出通道随原 profile 保留(emit_tool 克隆 + defer_emit_force 沿用)。
    pub fn with_env_tail(&self, tail: &str) -> Self {
        Self {
            name: self.name.clone(),
            system_prompt: self.system_prompt.append_base(tail),
            tools: self.tools.clone(),
            emit_tool: self.emit_tool.clone(),
            spawn_policy: self.spawn_policy,
            defer_emit_force: self.defer_emit_force,
        }
    }

    /// 构造 `User-Agent` 头取值:`{AgentName}/{版本号} {编译时间}`。
    pub fn user_agent(&self) -> String {
        let version = env!("CARGO_PKG_VERSION");
        let build_time = env!("LAEW_BUILD_TIME");
        format!("{}/{version} {build_time}", self.name)
    }
}

/// 在系统提示词末尾注入自感知段(由**注册表**生成,保证与真实工具面不漂移)。
///
/// 关闭条件(任一命中即原样返回,保证 `LAEW_SELF_SPAWN=off` 时提示词零变化):
/// 功能关闭 / 策略 Disabled / 注册表里没有 `SubAgent` 工具 / 深度上限为 0。
fn build_self_aware_prompt(
    name: &str,
    base: SystemPrompt,
    tools: &ToolRegistry,
    spawn_policy: SpawnPolicy,
) -> SystemPrompt {
    let tool_registered = tools
        .names()
        .contains(&crate::agent::tools::subagent::SUBAGENT_TOOL);
    let names = tools.names();
    // 第 115 轮:名册含用户/项目 `.laew/agents/*.md` 自定义类型(定义即生效,不缓存)
    let defs = crate::agent::custom_agents::discover_process().defs;
    let section = sa::prompt_section(
        name,
        &names,
        spawn_policy,
        sa::config(),
        tool_registered,
        &defs,
    );
    if section.is_empty() {
        base
    } else {
        base.append_base(&section)
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
    fn all_profiles_have_distinct_names() {
        let names = [
            AgentProfile::yolo_profile().name,
            AgentProfile::plan_profile().name,
            AgentProfile::main_work_profile().name,
            AgentProfile::sub_agent_work_profile().name,
            AgentProfile::quality_check_profile().name,
            AgentProfile::session_context_profile().name,
            AgentProfile::compact_profile().name,
            AgentProfile::work_flow_profile().name,
            AgentProfile::debug_profile().name,
        ];
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 9, "9 个 profile 必须名字互不相同");
    }

    #[test]
    fn yolo_profile_has_react_recon_tool_surface() {
        // 2026-09-22 ReAct 改造:Yolo 信息收集型工具面 5 件 + 结构化 emit + SubAgent;
        // 仍不持 Write/Edit/TodoWrite(入口层不落盘、不管理任务清单)。
        // defer_emit_force 仅 Yolo 开启,其余 profile 默认 false。
        let p = AgentProfile::yolo_profile();
        let names = tool_names(&p);
        for t in [
            "Read",
            "Glob",
            "Grep",
            "Bash",
            "MCP_Web_Use",
            "submit_task_classification",
        ] {
            assert!(names.contains(&t.to_string()), "Yolo 工具面应含 {t}: {names:?}");
        }
        for forbidden in ["Write", "Edit", "TodoWrite"] {
            assert!(
                !names.iter().any(|n| n == forbidden),
                "Yolo 不应持 {forbidden}: {names:?}"
            );
        }
        assert!(
            p.defer_emit_force,
            "Yolo 应开启 ReAct 延迟强制(defer_emit_force=true)"
        );
        assert!(!AgentProfile::quality_check_profile().defer_emit_force);
        assert!(!AgentProfile::sub_agent_work_profile().defer_emit_force);
        assert!(!AgentProfile::main_work_profile().defer_emit_force);
        assert!(!AgentProfile::plan_profile().defer_emit_force);
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

    /// 2026-09-18 第 89 轮:SubAgent-Work 持 MCP_Web_Use(浏览器操控统一入口,
    /// 全平台),系统提示词同步注入使用说明(与工具注册一致)。
    #[test]
    fn sub_agent_has_mcp_web_use_tool_and_prompt() {
        let p = AgentProfile::sub_agent_work_profile();
        let names = tool_names(&p);
        assert!(
            names.contains(&"MCP_Web_Use".to_string()),
            "SubAgent-Work 工具面应含 MCP_Web_Use: {names:?}"
        );
        let rendered = p.system_prompt.render(crate::config::Protocol::Anthropic);
        assert!(
            rendered.contains("MCP_Web_Use 工具使用说明"),
            "SubAgent-Work 系统提示词应含 MCP_Web_Use 使用说明"
        );
        for action in ["open", "control", "inspect"] {
            assert!(
                rendered.contains(action),
                "使用说明应提及 action={action}"
            );
        }
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

#[cfg(test)]
mod self_awareness_tests {
    use super::*;
    use crate::config::Protocol;

    fn rendered(p: &AgentProfile) -> String {
        p.system_prompt.render(Protocol::Anthropic)
    }

    #[test]
    fn spawn_policy_matches_role_responsibility() {
        // 委派者:入口只读 / 编排与执行全量;质控与会话收口角色不参与动态启动。
        assert_eq!(
            AgentProfile::yolo_profile().spawn_policy,
            SpawnPolicy::ReadOnlyChildren
        );
        for p in [
            AgentProfile::plan_profile(),
            AgentProfile::main_work_profile(),
            AgentProfile::sub_agent_work_profile(),
            AgentProfile::work_flow_profile(),
        ] {
            assert_eq!(p.spawn_policy, SpawnPolicy::FullChildren, "{}", p.name);
        }
        for p in [
            AgentProfile::quality_check_profile(),
            AgentProfile::session_context_profile(),
            AgentProfile::debug_profile(),
            AgentProfile::compact_profile(),
        ] {
            assert_eq!(p.spawn_policy, SpawnPolicy::Disabled, "{}", p.name);
        }
    }

    #[test]
    fn self_awareness_section_injected_for_delegating_roles() {
        let p = AgentProfile::sub_agent_work_profile();
        let text = rendered(&p);
        assert!(text.contains("## 自感知:你的身份与可启动的子 Agent"));
        assert!(
            text.contains("`Bash`") && text.contains("`SubAgent`"),
            "工具清单必须由注册表生成"
        );
        assert!(text.contains("`explore`"), "应含可启动类型名册");
        assert!(text.contains("禁止只回答「已委派给若干 SubAgent」"));
        assert!(text.contains("本会话最多 8 个子 Agent"));
    }

    #[test]
    fn read_only_role_roster_is_narrowed() {
        let text = rendered(&AgentProfile::yolo_profile());
        assert!(text.contains("自感知"));
        assert!(text.contains("`explore`"));
        assert!(
            !text.contains("`operator`"),
            "只读父 Agent 的名册不得出现需要写/操控的类型"
        );
        assert!(!text.contains("`general-purpose`"));
    }

    #[test]
    fn non_delegating_roles_have_no_self_awareness_section() {
        for p in [
            AgentProfile::quality_check_profile(),
            AgentProfile::session_context_profile(),
            AgentProfile::debug_profile(),
            AgentProfile::compact_profile(),
        ] {
            let text = rendered(&p);
            assert!(
                !text.contains("自感知:你的身份与可启动的子 Agent"),
                "{} 不应注入自感知段",
                p.name
            );
            assert!(
                !p.tools.names().contains(&"SubAgent"),
                "{} 不应持 SubAgent 工具",
                p.name
            );
        }
    }

    #[test]
    fn dynamic_child_leaf_prompt_has_no_roster_and_keeps_identity() {
        let child = AgentProfile::dynamic_child(
            "LsmAgentEmergentWork-SubAgent-Explore-x",
            crate::agent::self_awareness::SubAgentType::Explore
                .system_prompt("LsmAgentEmergentWork-SubAgent-Explore-x"),
            crate::agent::tools::sub_agent_work_registry()
                .subset(&["Read", "Glob", "Grep"], &["SubAgent"]),
            SpawnPolicy::Disabled,
        );
        let text = rendered(&child);
        assert!(text.contains("不能再启动子 Agent"), "叶子语义必须在提示词里");
        assert!(
            !text.contains("你可启动的子 Agent 类型"),
            "叶子不应看到可启动名册"
        );
        assert!(text.contains(child.name.as_str()), "身份段应含自身名");
        // with_env_tail 不应丢失策略(供 workspace 注入复用)
        let with_tail = child.with_env_tail("\n尾巴");
        assert_eq!(with_tail.spawn_policy, SpawnPolicy::Disabled);
        assert!(with_tail
            .system_prompt
            .render(Protocol::Anthropic)
            .contains("尾巴"));
    }
}
