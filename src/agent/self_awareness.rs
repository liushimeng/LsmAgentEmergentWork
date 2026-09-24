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
/// 运行记录持久化开关环境变量(第 115 轮)。
pub const ENV_PERSIST: &str = "LAEW_SUBAGENT_PERSIST";
/// 运行记录保留行数环境变量(第 115 轮;`0` = 不清理)。
pub const ENV_RUN_KEEP: &str = "LAEW_SUBAGENT_RUN_KEEP";

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
    /// 只能启动只读子 Agent(Yolo:入口层信息收集型工具面 Read/Glob/Grep/Bash/MCP_Web_Use,
    /// 子 Agent 仍只读侦察,不得越权写盘)。
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
             {rules}",
            name = name,
            label = self.label(),
            id = self.id(),
            hint = self.role_hint(),
            // 只读判定与名册过滤同源:默认工具集 ⊆ 只读集
            rules = common_rules_section(
                self.default_tools()
                    .iter()
                    .all(|t| READ_ONLY_TOOLS.contains(t))
            ),
        )
    }
}

/// 子 Agent 公共规则段(「工作方式 + 边界」)。
///
/// 内置 6 类与自定义类型(定义文件)**共用**本函数渲染,避免两处文案漂移;
/// `read_only = true` 时在边界追加只读约束(自定义定义的 `readonly` 语义落地)。
pub fn common_rules_section(read_only: bool) -> String {
    let mut out = String::from(
        "## 工作方式\n\
         1. 你只拿到**任务描述**这一个输入:它必须已经写明目标、输入路径、期望产物;\n\
         2. 先用只读工具把事实摸清(必要时用 Bash 做只读检查),再动手改/写;\n\
         3. 你无法与用户对话,信息不足时**按代价分级**处理(第 128 轮修订):\n\
            - **实现细节模糊**(用什么算法 / 中间产物放哪 / 输出什么格式 / 遍历多深)\n\
              → 选择最合理的解释,并在回答开头一句话说明你的假设;\n\
            - **目标标识缺失或不可达**(站点 / URL / 文件路径 / 应用名 / 账号名)\n\
              → **必须失败上报**,写明缺什么、试过什么、为什么无法自行决定。\n\
              **严禁猜测或替换成另一个目标** —— 猜错目标的整份产出都是废的,\n\
              还会污染上游汇总与会话记忆;宁可失败,不可乱跑。\n\
              也不要用 Bash echo / Write 落盘假装「已向用户提问」,那是伪造进度;\n\
         4. 完成后用简洁中文回答,回答必须**直接包含**用户要的内容本身\n\
            (禁止只给「已保存到 <路径>」这类占位描述,文件落盘只作为补充说明);\n\
         5. 失败时如实说明失败点与已尝试的路径,不要伪造成功。\n\n\
         ## 边界\n\
         - 只做被委派的这一件事,不要扩大范围(不做规划、不做质检、不写会话摘要);\n",
    );
    if read_only {
        out.push_str("- 本类型为**只读**委派:不要写文件、不要改仓库、不要执行有副作用的命令。\n");
    }
    out.push_str("- 你是叶子 Agent:**不能再启动子 Agent**,需要更多人力时在回答里说明建议。");
    out
}

/// 名册条目(名册渲染 + `list` 输出共用)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterEntry {
    pub id: String,
    pub label: String,
    pub desc: String,
    pub tools: Vec<String>,
    /// 是否来自 `.laew/agents/*.md` 自定义定义(第 115 轮新增;增量字段)。
    #[serde(default)]
    pub custom: bool,
    /// 自定义类型的定义文件路径(内置为 `None`)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// 被名册隐藏的类型及原因(可解释性:用户能知道「我写的定义为什么没生效」)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkippedEntry {
    pub id: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// 一次名册渲染的结果:可见条目 + 被隐藏的自定义类型。
#[derive(Debug, Clone, Default)]
pub struct RosterView {
    pub entries: Vec<RosterEntry>,
    pub skipped: Vec<SkippedEntry>,
}

/// 按父策略过滤后的名册(只读父 Agent 只看到「类型工具 ⊆ 只读集」的类型)。
///
/// 第 115 轮起,`defs` 传入自定义定义([`crate::agent::custom_agents`])后
/// 名册 = 内置 + 自定义;传空切片即为 D114 原语义(既有调用点/单测零变化)。
pub fn roster(policy: SpawnPolicy) -> Vec<RosterEntry> {
    roster_with(&[], policy)
}

/// [`roster`] 的可测核心:显式传入自定义定义集合。
pub fn roster_with(defs: &[crate::agent::custom_agents::AgentDef], policy: SpawnPolicy) -> Vec<RosterEntry> {
    roster_view(defs, policy).entries
}

/// 名册视图(可见 + 被隐藏),供 `list` 快照与 `/agents` 面板复用。
pub fn roster_view(
    defs: &[crate::agent::custom_agents::AgentDef],
    policy: SpawnPolicy,
) -> RosterView {
    if policy == SpawnPolicy::Disabled {
        return RosterView::default();
    }
    let allowed = policy.allowed_tools();
    let mut entries: Vec<RosterEntry> = SubAgentType::ALL
        .iter()
        .filter(|t| t.default_tools().iter().all(|tool| allowed.contains(tool)))
        .map(|t| RosterEntry {
            id: t.id().to_string(),
            label: t.label().to_string(),
            desc: t.role_hint().to_string(),
            tools: t.default_tools().iter().map(|s| s.to_string()).collect(),
            custom: false,
            source: None,
        })
        .collect();

    let mut skipped: Vec<SkippedEntry> = Vec::new();
    for d in defs {
        let declared = d.declared_tools();
        if crate::agent::custom_agents::visible_under(&declared, policy) {
            entries.push(RosterEntry {
                id: d.id.clone(),
                label: d.label.clone(),
                desc: d.description.clone(),
                tools: declared,
                custom: true,
                source: Some(d.source.display().to_string()),
            });
        } else {
            skipped.push(SkippedEntry {
                id: d.id.clone(),
                reason: if declared.is_empty() {
                    "定义未声明任何工具".to_string()
                } else {
                    format!(
                        "需要 `{}`,超出现角色可授予范围(当前上限:{})",
                        declared.join("/"),
                        allowed.join("/")
                    )
                },
                source: Some(d.source.display().to_string()),
            });
        }
    }
    RosterView { entries, skipped }
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
    /// 运行记录是否落 SQLite `subagent_run`(第 115 轮;关闭后零 DB 读写)。
    pub persist: bool,
    /// 启动期保留的最新运行记录条数(0 = 不清理)。
    pub run_keep: usize,
}

impl Default for SelfAwarenessConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 1,
            max_parallel: 3,
            max_total: 8,
            // 第 119 轮: 与 OrchestratorConfig.subagent_max_iterations 对齐 32,
            // 防止动态子 Agent 在验证码/登录链路上过早 max_iter 撞线
            // (实测 max_iter=12 时子 Agent 刚启动就退出, 浪费调度与 round-trip)。
            max_iterations: 32,
            timeout_secs: 300,
            persist: true,
            run_keep: 500,
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
            persist: parse_on_off(std::env::var(ENV_PERSIST).ok().as_deref(), d.persist),
            run_keep: parse_usize(
                std::env::var(ENV_RUN_KEEP).ok().as_deref(),
                d.run_keep,
                0,
                20_000,
            ),
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
///
/// 第 115 轮起 `defs` 传入自定义子 Agent 定义(`.laew/agents/*.md`);
/// **只渲染会话内恒定内容**(id/label/描述/工具名),不放定义文件路径与易变额度,
/// 以保住 prompt cache 前缀(路径只在 `list` 快照与 `/agents` 面板出现)。
pub fn prompt_section(
    agent_name: &str,
    tool_names: &[&str],
    policy: SpawnPolicy,
    cfg: &SelfAwarenessConfig,
    tool_registered: bool,
    defs: &[crate::agent::custom_agents::AgentDef],
) -> String {
    if !tool_registered || !cfg.enabled || policy == SpawnPolicy::Disabled || cfg.max_depth == 0 {
        return String::new();
    }
    let entries = roster_with(defs, policy);
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
            "  - `{}`({}{}):{};可用工具 [{}]\n",
            e.id,
            e.label,
            if e.custom { ", 自定义" } else { "" },
            e.desc,
            e.tools.join(", ")
        ));
    }
    if entries.iter().any(|e| e.custom) {
        out.push_str(
            "(标注「自定义」的类型来自用户/项目 `.laew/agents/*.md` 定义文件;\
             如需更细的工具面与适用场景,调用 `SubAgent(action=\"list\")` 查看实时名册。)\n",
        );
    }
    out.push_str("\n### 启动规则(必须遵守)\n");
    out.push_str(
        "1. **触发**:任务可分解为 2 个以上**相互独立**的子任务,或用户提示词显式出现\n\
         「启动 SubAgent / 多开几个 Agent / 并行 / 分工 / 分别调研 / 同时处理 / 各写一份」等意图时,启动子 Agent。\n\
         2. **禁止**:单步任务、步骤间有严格依赖、需要你自己看中间结果再决策的任务 —— 直接自己做,\n\
         启动子 Agent 要额外付一次完整的 LLM 成本。\n\
         3. **批量优先**:多个**相互独立**子任务用一次 `SubAgent(action=\"batch\", tasks=[...])`,批内自动并行;\n\
         有依赖的多阶段流程用一次 `SubAgent(action=\"workflow\", steps=[...])`,步骤声明 `id` 与 `depends_on`,\n\
         执行器会拓扑分层并把成功上游结论注入下游;只有单步续跑才用 `launch`。\n\
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
        // 第 119 轮:与 OrchestratorConfig.subagent_max_iterations(=32) 对齐,
        // 验证码/登录链路实测需要 ≥20 iter, 12 撞线率极高
        assert_eq!(c.max_iterations, 32);
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
            &[],
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
            true,
            &[],
        )
        .is_empty());
        assert!(prompt_section(
            "LsmAgentEmergentWork-Yolo",
            &tools,
            SpawnPolicy::ReadOnlyChildren,
            &cfg(),
            false,
            &[],
        )
        .is_empty());
        let mut off = cfg();
        off.enabled = false;
        assert!(prompt_section(
            "LsmAgentEmergentWork-Yolo",
            &tools,
            SpawnPolicy::ReadOnlyChildren,
            &off,
            true,
            &[],
        )
        .is_empty());
    }

    // ========== 第 115 轮(2026-09-22):自定义子 Agent 类型名册 ==========

    fn custom_def(id: &str, tools: Option<&str>, readonly: bool) -> crate::agent::custom_agents::AgentDef {
        crate::agent::custom_agents::AgentDef {
            id: id.to_string(),
            label: format!("{id} 标签"),
            description: format!("{id} 场景说明"),
            extends: SubAgentType::GeneralPurpose,
            tools: tools
                .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default(),
            read_only: readonly,
            body: "专属职责正文".to_string(),
            source: std::path::PathBuf::from(format!("/tmp/{id}.md")),
            scope: crate::agent::custom_agents::DefScope::Project,
        }
    }

    #[test]
    fn roster_without_defs_matches_builtin_only() {
        for policy in [
            SpawnPolicy::FullChildren,
            SpawnPolicy::ReadOnlyChildren,
            SpawnPolicy::Disabled,
        ] {
            assert_eq!(roster_with(&[], policy), roster(policy), "空定义集 = D114 语义");
            assert!(roster(policy).iter().all(|e| !e.custom));
        }
    }

    #[test]
    fn roster_includes_custom_types_with_source() {
        let defs = vec![custom_def("fe-reviewer", Some("Read, Glob, Grep"), true)];
        let entries = roster_with(&defs, SpawnPolicy::FullChildren);
        let custom = entries.iter().find(|e| e.custom).expect("自定义类型应入名册");
        assert_eq!(custom.id, "fe-reviewer");
        assert_eq!(custom.label, "fe-reviewer 标签");
        assert_eq!(custom.tools, vec!["Read", "Glob", "Grep"]);
        assert_eq!(custom.source.as_deref(), Some("/tmp/fe-reviewer.md"));
    }

    #[test]
    fn roster_hides_custom_type_beyond_policy_and_reports_reason() {
        let defs = vec![custom_def("patcher", Some("Read, Write"), false)];
        let view = roster_view(&defs, SpawnPolicy::ReadOnlyChildren);
        assert!(view.entries.iter().all(|e| e.id != "patcher"), "不得越权可见");
        assert_eq!(view.skipped.len(), 1);
        assert_eq!(view.skipped[0].id, "patcher");
        assert!(view.skipped[0].reason.contains("Write"));
        // 同一类型在完整策略下可见
        let full = roster_view(&defs, SpawnPolicy::FullChildren);
        assert!(full.entries.iter().any(|e| e.id == "patcher"));
        assert!(full.skipped.is_empty());
    }

    #[test]
    fn prompt_section_lists_custom_types_without_path() {
        let defs = vec![custom_def("fe-reviewer", Some("Read, Glob, Grep"), true)];
        let tools = vec!["Read", "SubAgent"];
        let s = prompt_section(
            "LsmAgentEmergentWork-Yolo",
            &tools,
            SpawnPolicy::ReadOnlyChildren,
            &cfg(),
            true,
            &defs,
        );
        assert!(s.contains("`fe-reviewer`"), "自定义类型应进名册");
        assert!(s.contains("自定义"), "应标注自定义来源");
        assert!(
            !s.contains("/tmp/fe-reviewer.md"),
            "静态段不得含定义文件路径(保 prompt cache 前缀)"
        );
    }

    #[test]
    fn prompt_section_disabled_state_ignores_custom_defs() {
        let defs = vec![custom_def("fe-reviewer", Some("Read"), true)];
        let mut off = cfg();
        off.enabled = false;
        assert!(prompt_section(
            "LsmAgentEmergentWork-Yolo",
            &["Read", "SubAgent"],
            SpawnPolicy::ReadOnlyChildren,
            &off,
            true,
            &defs,
        )
        .is_empty());
    }

    #[test]
    fn common_rules_section_mentions_readonly_when_asked() {
        assert!(!common_rules_section(false).contains("只读**委派"));
        let ro = common_rules_section(true);
        assert!(ro.contains("只读**委派"));
        assert!(ro.contains("不能再启动子 Agent"));
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
