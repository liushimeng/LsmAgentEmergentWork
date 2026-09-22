//! Agent 自感知(Self-Awareness):身份 / 工具面 / 可启动子 Agent 名册 / 启动上限。
//!
//! 第 114 轮(2026-09-22)新增,设计见
//! `docs/自感知SubAgent动态启动/01-设计与解决方案.md`。
//!
//! 三层自感知:
//! 1. **静态身份段**(本模块 [`prompt_section`]):注入系统提示词,内容由
//!    **`ToolRegistry` 实时生成**(知识库 CC03「ToolRegistry 自省」),
//!    因此提示词与实际工具面永不漂移;
//! 2. **运行时快照**(`dynamic_subagent::snapshot_json`):`SubAgent(action="list")` 返回
//!    深度 / 余额 / 运行中作业等易变值 —— 刻意**不进**系统提示词,保住 prompt cache 前缀;
//! 3. **启动记录**(`dynamic_subagent::recent_events`):TUI `/agents` 可视化。
//!
//! 本模块保持**纯函数 + 无副作用**(配置读取除外),便于单测。

use serde::{Deserialize, Serialize};

/// 动态启动总开关环境变量。
pub const ENV_SELF_SPAWN: &str = "LAEW_SELF_SPAWN";
/// 嵌套深度上限环境变量。
pub const ENV_MAX_DEPTH: &str = "LAEW_SUBAGENT_MAX_DEPTH";
/// 会话级并发上限环境变量。
pub const ENV_MAX_PARALLEL: &str = "LAEW_SUBAGENT_MAX_PARALLEL";
/// 会话级启动预算环境变量。
pub const ENV_MAX_TOTAL: &str = "LAEW_SUBAGENT_MAX_TOTAL";
/// 单子 Agent 迭代上限环境变量。
pub const ENV_MAX_ITERATIONS: &str = "LAEW_SUBAGENT_MAX_ITERATIONS";
/// 单子 Agent 超时环境变量(秒)。
pub const ENV_TIMEOUT_SECS: &str = "LAEW_SUBAGENT_TIMEOUT_SECS";

/// 单个子 Agent 报告文本的截断上限(字符),防止撑爆父上下文。
pub const CHILD_REPORT_CHARS: usize = 8000;
/// batch 合并文本上限(字符)。
pub const BATCH_MERGED_CHARS: usize = 12_000;
/// batch 单子报告在合并文本里的上限(字符)。
pub const BATCH_CHILD_CHARS: usize = 4_000;
/// 单次 batch 最多子任务数。
pub const MAX_BATCH_TASKS: usize = 8;

// ===========================================================================
// 策略
// ===========================================================================

/// 子 Agent 工具能力策略(对齐 opencode `deriveSubagentSessionPermission`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnPolicy {
    /// 不注册 `SubAgent` 工具(QC / SessionContext / Debug / Compact / Compact 等)。
    Disabled,
    /// 只能启动只读子 Agent(Yolo:入口层只持 Read,子 Agent 不得越权写盘)。
    ReadOnlyChildren,
    /// 可启动完整执行层子 Agent(Plan / Main-Work / SubAgent-Work / WorkFlow:
    /// 这些角色本身就是「委派者」,其委派单元既有语义即为全套执行工具)。
    FullChildren,
}

impl SpawnPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ReadOnlyChildren => "read_only",
            Self::FullChildren => "full",
        }
    }

    /// 本策略下放给子 Agent 的工具上限(交集运算的右操作数)。
    pub fn allowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::Disabled => &[],
            Self::ReadOnlyChildren => READ_ONLY_TOOLS,
            Self::FullChildren => EXECUTOR_TOOLS,
        }
    }
}

/// 只读工具集(侦察 / 审查子 Agent 的可用面)。
pub const READ_ONLY_TOOLS: &[&str] = &["Read", "Glob", "Grep"];

/// 执行层工具集(通用子 Agent 的可用面;平台门控工具在交集阶段自然剔除)。
pub const EXECUTOR_TOOLS: &[&str] = &[
    "Bash",
    "Read",
    "Write",
    "Edit",
    "Glob",
    "Grep",
    "TodoWrite",
    "MCP_Web_Use",
    "MCP_Window_Use",
];

// ===========================================================================
// 子 Agent 类型名册
// ===========================================================================

/// 可启动的子 Agent 类型(对齐 claudecode `subagent_type` / opencode 6 内置 Agent)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubAgentType {
    /// 通用执行者:全套执行层工具。
    GeneralPurpose,
    /// 只读侦察:定位文件 / 搜索符号 / 读代码给结论。
    Explore,
    /// 资料研究:只读 + 浏览器检索。
    Researcher,
    /// 方案设计:只读 + 写方案文件。
    Plan,
    /// 代码审查:只读问题清单。
    CodeReviewer,
    /// GUI / 浏览器操作:真实操控浏览器或桌面窗口。
    Operator,
}

impl SubAgentType {
    /// 全类型清单(名册渲染顺序即此顺序)。
    pub const ALL: [SubAgentType; 6] = [
        SubAgentType::GeneralPurpose,
        SubAgentType::Explore,
        SubAgentType::Researcher,
        SubAgentType::Plan,
        SubAgentType::CodeReviewer,
        SubAgentType::Operator,
    ];

    pub fn id(&self) -> &'static str {
        match self {
            Self::GeneralPurpose => "general-purpose",
            Self::Explore => "explore",
            Self::Researcher => "researcher",
            Self::Plan => "plan",
            Self::CodeReviewer => "code-reviewer",
            Self::Operator => "operator",
        }
    }

    /// 宽松解析(容错大小写 / 下划线 / 常见别名),未知返回 `None`。
    pub fn parse(raw: &str) -> Option<Self> {
        let key = raw.trim().to_lowercase().replace('_', "-");
        match key.as_str() {
            "general-purpose" | "general" | "generalpurpose" | "default" | "通用" | "通用执行" => {
                Some(Self::GeneralPurpose)
            }
            "explore" | "explorer" | "scout" | "侦察" | "只读侦察" => Some(Self::Explore),
            "researcher" | "research" | "search" | "研究" | "资料研究" => Some(Self::Researcher),
            "plan" | "planner" | "规划" | "方案设计" => Some(Self::Plan),
            "code-reviewer" | "codereviewer" | "review" | "reviewer" | "审查" | "代码审查" => {
                Some(Self::CodeReviewer)
            }
            "operator" | "gui" | "browser" | "操控" | "操作" => Some(Self::Operator),
            _ => None,
        }
    }

    /// 中文定位(名册展示用)。
    pub fn label(&self) -> &'static str {
        match self {
            Self::GeneralPurpose => "通用执行",
            Self::Explore => "只读侦察",
            Self::Researcher => "资料研究",
            Self::Plan => "方案设计",
            Self::CodeReviewer => "代码审查",
            Self::Operator => "界面操控",
        }
    }

    /// 一句话场景说明。
    pub fn role_hint(&self) -> &'static str {
        match self {
            Self::GeneralPurpose => "多步执行:写代码 / 跑命令 / 批量改造(可写文件)",
            Self::Explore => "只读侦察:定位文件、搜索符号、读代码给结论(零副作用)",
            Self::Researcher => "资料研究:网页检索 + 资料整理 + 引用来源",
            Self::Plan => "方案设计:产出实施步骤 / 风险 / 验收清单(可落盘)",
            Self::CodeReviewer => "代码审查:找实现缺陷与风险,输出 P0-P2 问题清单",
            Self::Operator => "界面操控:真实驱动浏览器 / 桌面窗口完成任务",
        }
    }

    /// 类型默认工具(与父策略取交集后才真正生效)。
    pub fn default_tools(&self) -> &'static [&'static str] {
        match self {
            Self::GeneralPurpose => EXECUTOR_TOOLS,
            Self::Explore | Self::CodeReviewer => READ_ONLY_TOOLS,
            Self::Researcher => &["Read", "Glob", "Grep", "MCP_Web_Use"],
            Self::Plan => &["Read", "Glob", "Grep", "Write"],
            Self::Operator => &["Bash", "Read", "MCP_Web_Use", "MCP_Window_Use"],
        }
    }

    /// 子 Agent 的角色定位(用于派生 UA 名 `LsmAgentEmergentWork-SubAgent-{Pascal}`)。
    pub fn pascal(&self) -> &'static str {
        match self {
            Self::GeneralPurpose => "General",
            Self::Explore => "Explore",
            Self::Researcher => "Researcher",
            Self::Plan => "Plan",
            Self::CodeReviewer => "Reviewer",
            Self::Operator => "Operator",
        }
    }

    /// 子 Agent 系统提示词(自包含:子 Agent 看不到父对话上下文)。
    pub fn system_prompt(&self, name: &str) -> String {
        format!(
            "你是 laew 的动态子 Agent「{name}」,类型:{label}({id})。\n\n\
             ## 你的职责\n{hint}\n\n\
             ## 工作方式\n\
             1. 你只拿到**任务描述**这一个输入:它必须已经写明目标、输入路径、期望产物;\n\
             2. 先用只读工具把事实摸清(必要时用 Bash 做只读检查),再动手改/写;\n\
             3. 不要询问澄清问题 —— 信息不足时,选择最合理的解释并**在回答开头一句话说明你的假设**;\n\
             4. 完成后用简洁中文回答,回答必须**直接包含**用户要的内容本身\n\
                (禁止只给「已保存到 <路径>」这类占位描述,文件落盘只作为补充说明);\n\
             5. 失败时如实说明失败点与已尝试的路径,不要伪造成功。\n\n\
             ## 边界\n\
             - 只做被委派的这一件事,不要扩大范围(不做规划、不做质检、不写会话摘要);\n\
             - 你是叶子 Agent:**不能再启动子 Agent**,需要更多人力时在回答里说明建议。",
            name = name,
            label = self.label(),
            id = self.id(),
            hint = self.role_hint(),
        )
    }
}

/// 名册条目(名册渲染 + `list` 输出共用)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterEntry {
    pub id: String,
    pub label: String,
    pub desc: String,
    pub tools: Vec<String>,
}

/// 按父策略过滤后的名册(只读父 Agent 只看到「类型工具 ⊆ 只读集」的类型)。
pub fn roster(policy: SpawnPolicy) -> Vec<RosterEntry> {
    if policy == SpawnPolicy::Disabled {
        return Vec::new();
    }
    let allowed = policy.allowed_tools();
    SubAgentType::ALL
        .iter()
        .filter(|t| t.default_tools().iter().all(|tool| allowed.contains(tool)))
        .map(|t| RosterEntry {
            id: t.id().to_string(),
            label: t.label().to_string(),
            desc: t.role_hint().to_string(),
            tools: t.default_tools().iter().map(|s| s.to_string()).collect(),
        })
        .collect()
}

// ===========================================================================
// 配置
// ===========================================================================

/// 自感知 / 动态启动配置(进程级,环境变量一次性读取)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfAwarenessConfig {
    /// 总开关;false 时工具不注册、提示词不注入、运行时零开销。
    pub enabled: bool,
    /// 允许的最大嵌套深度(0 = 禁止启动;1 = 只允许一层子 Agent)。
    pub max_depth: usize,
    /// 会话级并发槽位。
    pub max_parallel: usize,
    /// 会话级累计启动预算。
    pub max_total: usize,
    /// 单个子 Agent 迭代上限。
    pub max_iterations: usize,
    /// 单个子 Agent 墙钟超时(秒)。
    pub timeout_secs: u64,
}

impl Default for SelfAwarenessConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 1,
            max_parallel: 3,
            max_total: 8,
            max_iterations: 12,
            timeout_secs: 300,
        }
    }
}

impl SelfAwarenessConfig {
    /// 从环境变量读取(缺省 / 非法值一律回退默认)。
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            enabled: parse_on_off(std::env::var(ENV_SELF_SPAWN).ok().as_deref(), d.enabled),
            max_depth: parse_usize(
                std::env::var(ENV_MAX_DEPTH).ok().as_deref(),
                d.max_depth,
                0,
                3,
            ),
            max_parallel: parse_usize(
                std::env::var(ENV_MAX_PARALLEL).ok().as_deref(),
                d.max_parallel,
                1,
                8,
            ),
            max_total: parse_usize(
                std::env::var(ENV_MAX_TOTAL).ok().as_deref(),
                d.max_total,
                1,
                64,
            ),
            max_iterations: parse_usize(
                std::env::var(ENV_MAX_ITERATIONS).ok().as_deref(),
                d.max_iterations,
                4,
                32,
            ),
            timeout_secs: parse_usize(
                std::env::var(ENV_TIMEOUT_SECS).ok().as_deref(),
                d.timeout_secs as usize,
                10,
                3600,
            ) as u64,
        }
    }

    /// 本配置下是否允许某个深度的 Agent 继续启动子 Agent。
    pub fn can_spawn_at(&self, depth: usize) -> bool {
        self.enabled && self.max_depth > 0 && depth < self.max_depth
    }
}

/// 进程级缓存的配置(避免每次渲染提示词都读环境变量)。
pub fn config() -> &'static SelfAwarenessConfig {
    static CFG: std::sync::OnceLock<SelfAwarenessConfig> = std::sync::OnceLock::new();
    CFG.get_or_init(SelfAwarenessConfig::from_env)
}

/// 解析布尔开关:`off/0/false/no` 为 false,`on/1/true/yes` 为 true,其它回退 `default`。
pub fn parse_on_off(raw: Option<&str>, default: bool) -> bool {
    match raw.map(|s| s.trim().to_lowercase()).as_deref() {
        None | Some("") => default,
        Some("off") | Some("0") | Some("false") | Some("no") => false,
        Some("on") | Some("1") | Some("true") | Some("yes") => true,
        Some(_) => default,
    }
}

/// 解析范围受限的 usize(非法 / 越界一律 clamp 或回退默认)。
pub fn parse_usize(raw: Option<&str>, default: usize, min: usize, max: usize) -> usize {
    match raw.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        None => default,
        Some(s) => match s.parse::<usize>() {
            Ok(v) => v.clamp(min, max),
            Err(_) => default,
        },
    }
}

// ===========================================================================
// 静态身份段渲染
// ===========================================================================

/// 由 Agent 名推断中文角色定位(动态子 Agent 名含 `SubAgent-` 前缀,单独归类)。
pub fn role_label_for(agent_name: &str) -> &'static str {
    if agent_name.contains("-Yolo") {
        "入口层(任务识别 / 难度分级 / 失败回流)"
    } else if agent_name.contains("-Plan") {
        "规划层(hard 档方案产出)"
    } else if agent_name.contains("-Main-Work") {
        "流程层(WorkFlow 编排 / 委派)"
    } else if agent_name.contains("-SubAgent-Work") {
        "执行层最小单元(工具执行)"
    } else if agent_name.contains("-Quality-Check") {
        "质检层(单元输出校验)"
    } else if agent_name.contains("-SessionContext") {
        "会话层(摘要串联)"
    } else if agent_name.contains("-WorkFlow") {
        "工作流编排层(Goal / Squad)"
    } else if agent_name.contains("-Debug") {
        "调试层(trace 评估)"
    } else if agent_name.contains("-Compact") {
        "压缩层(上下文摘要)"
    } else if agent_name.contains("SubAgent-") {
        "动态子 Agent(叶子单元)"
    } else {
        "Agent"
    }
}

/// 渲染静态自感知段(注入系统提示词)。
///
/// 返回空串表示「本 Agent 不需要该段」:功能关闭 / 策略 Disabled / 深度已用尽 /
/// 注册表里没有 `SubAgent` 工具(三重条件都满足才注入,确保关闭时提示词零变化)。
pub fn prompt_section(
    agent_name: &str,
    tool_names: &[&str],
    policy: SpawnPolicy,
    cfg: &SelfAwarenessConfig,
    tool_registered: bool,
) -> String {
    if !tool_registered || !cfg.enabled || policy == SpawnPolicy::Disabled || cfg.max_depth == 0 {
        return String::new();
    }
    let entries = roster(policy);
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("\n\n---\n\n## 自感知:你的身份与可启动的子 Agent\n");
    out.push_str(&format!(
        "- 你的身份:`{agent_name}`({label})\n",
        label = role_label_for(agent_name)
    ));
    out.push_str(&format!(
        "- 你当前持有的工具:`{}`\n",
        tool_names.join("` / `")
    ));
    out.push_str(
        "- 需要确认「剩余额度 / 运行中的子 Agent」时,调用 `SubAgent(action=\"list\")` 取实时快照。\n",
    );
    out.push_str("- 你可启动的子 Agent 类型:\n");
    for e in &entries {
        out.push_str(&format!(
            "  - `{}`({}):{};默认工具 [{}]\n",
            e.id,
            e.label,
            e.desc,
            e.tools.join(", ")
        ));
    }
    out.push_str("\n### 启动规则(必须遵守)\n");
    out.push_str(
        "1. **触发**:任务可分解为 2 个以上**相互独立**的子任务,或用户提示词显式出现\n\
         「启动 SubAgent / 多开几个 Agent / 并行 / 分工 / 分别调研 / 同时处理 / 各写一份」等意图时,启动子 Agent。\n\
         2. **禁止**:单步任务、步骤间有严格依赖、需要你自己看中间结果再决策的任务 —— 直接自己做,\n\
         启动子 Agent 要额外付一次完整的 LLM 成本。\n\
         3. **批量优先**:多个独立子任务用一次 `SubAgent(action=\"batch\", tasks=[...])`,批内自动并行;\n\
         有依赖关系时再用 `SubAgent(action=\"launch\", ...)` 逐个启动。\n\
         4. **任务自包含**:子 Agent 看不到你的对话上下文,task 必须写明「目标 + 输入路径 + 期望产物 + 验收口径」。\n\
         5. **结果必汇总**:子 Agent 的输出只是中间产物,必须由你整合成面向用户的最终回答;\n\
         禁止只回答「已委派给若干 SubAgent」。\n\
         6. **失败如实报**:子 Agent 失败时说明失败子任务与原因,并给出你自己补做或改派后的结果。\n",
    );
    out.push_str(&format!(
        "\n硬上限(超出会被工具直接拒绝):本会话最多 {} 个子 Agent / 并发 {} / 嵌套 {} 层 /\n\
         单子 Agent 最多 {} 轮迭代、{} 秒超时。\n",
        cfg.max_total, cfg.max_parallel, cfg.max_depth, cfg.max_iterations, cfg.timeout_secs
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SelfAwarenessConfig {
        SelfAwarenessConfig::default()
    }

    #[test]
    fn config_defaults_are_conservative() {
        let c = SelfAwarenessConfig::default();
        assert!(c.enabled);
        assert_eq!(c.max_depth, 1, "默认只允许一层子 Agent(对齐 openclaw)");
        assert_eq!(c.max_parallel, 3, "默认并发 3(对齐 atomcode)");
        assert_eq!(c.max_total, 8);
        assert_eq!(c.max_iterations, 12);
        assert_eq!(c.timeout_secs, 300);
    }

    #[test]
    fn config_env_parse_clamps() {
        assert_eq!(parse_usize(Some("99"), 3, 1, 8), 8, "越界应 clamp 到上限");
        assert_eq!(parse_usize(Some("0"), 3, 1, 8), 1, "越界应 clamp 到下限");
        assert_eq!(parse_usize(Some("abc"), 3, 1, 8), 3, "非法应回退默认");
        assert_eq!(parse_usize(None, 3, 1, 8), 3);
        assert!(!parse_on_off(Some("off"), true));
        assert!(!parse_on_off(Some("0"), true));
        assert!(parse_on_off(Some("yes"), false));
        assert!(parse_on_off(Some(""), true));
        assert!(parse_on_off(Some("whatever"), true));
    }

    #[test]
    fn can_spawn_at_respects_depth_and_switch() {
        let c = cfg();
        assert!(c.can_spawn_at(0));
        assert!(!c.can_spawn_at(1), "max_depth=1 时深度 1 不能再启动");
        let mut off = cfg();
        off.enabled = false;
        assert!(!off.can_spawn_at(0));
        let mut zero = cfg();
        zero.max_depth = 0;
        assert!(!zero.can_spawn_at(0));
    }

    #[test]
    fn type_parse_is_lenient() {
        assert_eq!(SubAgentType::parse("Explore"), Some(SubAgentType::Explore));
        assert_eq!(
            SubAgentType::parse("code_reviewer"),
            Some(SubAgentType::CodeReviewer)
        );
        assert_eq!(
            SubAgentType::parse("通用"),
            Some(SubAgentType::GeneralPurpose)
        );
        assert_eq!(SubAgentType::parse("nope"), None);
        // id ↔ parse 往返一致
        for t in SubAgentType::ALL {
            assert_eq!(SubAgentType::parse(t.id()), Some(t), "id={}", t.id());
        }
    }

    #[test]
    fn roster_full_policy_lists_all_six() {
        let r = roster(SpawnPolicy::FullChildren);
        assert_eq!(r.len(), 6);
        assert!(r.iter().any(|e| e.id == "operator"));
        assert!(r.iter().any(|e| e.id == "researcher"));
    }

    #[test]
    fn roster_read_only_policy_hides_privileged_types() {
        let r = roster(SpawnPolicy::ReadOnlyChildren);
        let ids: Vec<&str> = r.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["explore", "code-reviewer"], "只读父只能启动只读类型");
        assert!(roster(SpawnPolicy::Disabled).is_empty());
    }

    #[test]
    fn read_only_subset_of_executor() {
        for t in READ_ONLY_TOOLS {
            assert!(
                EXECUTOR_TOOLS.contains(t),
                "{t} 应属于执行层工具集(否则只读策略交集恒空)"
            );
        }
    }

    #[test]
    fn prompt_section_injected_for_full_policy() {
        let tools = vec!["Bash", "Read", "SubAgent"];
        let s = prompt_section(
            "LsmAgentEmergentWork-SubAgent-Work",
            &tools,
            SpawnPolicy::FullChildren,
            &cfg(),
            true,
        );
        assert!(s.contains("自感知"), "应含自感知标题");
        assert!(s.contains("LsmAgentEmergentWork-SubAgent-Work"));
        assert!(s.contains("`Bash` / `Read` / `SubAgent`"), "工具面应程序化生成");
        assert!(s.contains("explore"), "应含名册");
        assert!(s.contains("batch"), "应含批量启动契约");
        assert!(s.contains("最多 8 个子 Agent"));
    }

    #[test]
    fn prompt_section_empty_when_disabled_or_unregistered() {
        let tools = vec!["Read"];
        assert!(prompt_section(
            "LsmAgentEmergentWork-Yolo",
            &tools,
            SpawnPolicy::Disabled,
            &cfg(),
            true
        )
        .is_empty());
        assert!(prompt_section(
            "LsmAgentEmergentWork-Yolo",
            &tools,
            SpawnPolicy::ReadOnlyChildren,
            &cfg(),
            false
        )
        .is_empty());
        let mut off = cfg();
        off.enabled = false;
        assert!(prompt_section(
            "LsmAgentEmergentWork-Yolo",
            &tools,
            SpawnPolicy::ReadOnlyChildren,
            &off,
            true
        )
        .is_empty());
    }

    #[test]
    fn role_label_recognizes_known_agents() {
        assert!(role_label_for("LsmAgentEmergentWork-Yolo").contains("入口层"));
        assert!(role_label_for("LsmAgentEmergentWork-SubAgent-Work").contains("执行层"));
        assert!(role_label_for("LsmAgentEmergentWork-SubAgent-Explore").contains("动态子 Agent"));
        assert_eq!(role_label_for("weird-name"), "Agent");
    }

    #[test]
    fn child_system_prompt_is_self_contained() {
        let p = SubAgentType::Explore.system_prompt("侦察-A");
        assert!(p.contains("侦察-A"));
        assert!(p.contains("explore"));
        assert!(p.contains("不能再启动子 Agent"), "子 Agent 必须是叶子语义");
        assert!(p.contains("任务描述"));
    }
}
