//! 编排器共享数据类型(2026-09-17 自 orchestrator.rs 拆分,方案见 tmpPlan/2026-09-17_01)。
//!
//! 配置 / 结果 / 阶段耗时 / 重试记录 / 分层信息 / 任务结果 / 编排结局 / 质检失败等
//! 结构体集中于此,供编排器各子模块与外部调用方(main.rs / tui)共用。

use super::*;

pub struct OrchestratorConfig {
    /// 同一档位最大重试次数(超过后输出 user_suggestion)
    pub max_retry_per_level: usize,
    /// 历史注入条数
    pub history_limit: usize,
    /// SubAgent-Work 单次单元最大迭代
    ///
    /// 第 118 轮(2026-09-22):默认值 16 → 32。
    /// 验证码 OCR + 登录链路实测需 20-30 iter,16 iter 撞线率极高;
    /// 提到 32 配套「探索预算」分段(见 [`Self::subagent_explore_budget`])。
    pub subagent_max_iterations: usize,
    /// 第 118 轮新增:SubAgent 探索阶段预算(iter < explore_budget 鼓励
    /// inspect/screenshot/eval_js 等只读探查);超出后 Runner 通过 RuntimeHint
    /// 注入「进入执行期」提示,要求 LLM 收敛到 input_text/click/wait。
    ///
    /// 默认 = `subagent_max_iterations / 4`(32 → 8)。
    /// 设为 0 = 关闭该机制(等同旧行为)。
    pub subagent_explore_budget: usize,
    /// 同层无依赖 WorkFlow 的最大并行数(信号量上限,对齐 atomcode Semaphore(3) 惯例)
    pub max_parallel_workflows: usize,
    /// ★2026-09-19 第 95 轮:执行单元级局部重试预算 —— 单单元 QC 判 retryable 后,
    /// **仅重试该单元**(不连坐同层姊妹单元)的次数;0 = 关闭(保持旧行为:QC 拒即升级到档位级),
    /// 默认 2 = 单单元最多 3 次尝试(首执行 + 2 次局部重试),预算耗尽才升级到档位级
    /// 重试(loop / Yolo 回流)。零配置即生效,向后兼容。
    /// 实现见 tmpPlan/2026-09-19_06-单元级局部重试闭环方案.md。
    pub unit_retry_budget: usize,
    /// 调试事件采集器(`-debug` 调试模式时注入,默认 None 零开销)
    pub debug: Option<Arc<DebugCollector>>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        // 第 118 轮:max_iter 16 → 32,同步加 explore_budget = max_iter / 4 = 8
        let subagent_max_iterations = 32_usize;
        let subagent_explore_budget = subagent_max_iterations / 4; // 8
        Self {
            max_retry_per_level: 3,
            history_limit: DEFAULT_HISTORY_LIMIT,
            subagent_max_iterations,
            subagent_explore_budget,
            max_parallel_workflows: 3,
            unit_retry_budget: 2,
            debug: None,
        }
    }
}

/// 单个 WorkFlow 执行结果(对外可读)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub id: String,
    pub name: String,
    pub subflow_outcome: String,
    pub quality_report: QualityReport,
    pub usage: Usage,
    /// SubAgent 执行轨迹(2026-09-09 第 05 轮),失败时为 None。
    pub subflow_trace: Option<ExecutionTrace>,
    /// 2026-09-16 第 57 轮:执行器角色(当前统一为 SubAgent),TUI 据此标识责任 Agent
    #[serde(default = "default_wf_exec_role")]
    pub exec_role: AgentRole,
    /// 2026-09-16 第 57 轮:执行器墙钟耗时(毫秒)
    #[serde(default)]
    pub wallclock_ms: u64,
    /// 2026-09-16 第 57 轮:QC LLM 调用单独耗时(毫秒)
    #[serde(default)]
    pub qc_wallclock_ms: u64,
}

pub(super) fn default_wf_exec_role() -> AgentRole {
    AgentRole::SubAgent
}

/// 阶段耗时记录(2026-09-16 第 57 轮):每个 Yolo / Main-Work / QC / WF / SessionContext
/// 阶段的开始偏移 + 实际耗时,供 TUI 终端打印阶段时间线。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDuration {
    /// 阶段名:yolo / main_work / qc_main / wf / qc_wf / session_context / plan
    pub stage: String,
    /// wf / qc_wf 时填,标识所属 WorkFlow id
    pub wf_id: Option<String>,
    /// 相对任务开始的偏移(毫秒)
    pub started_offset_ms: u64,
    /// 阶段耗时(毫秒)
    pub elapsed_ms: u64,
}

/// 重试记录(2026-09-16 第 57 轮):把 orchestrator handle_inner 的 retry loop
/// 暴露给 TUI,用户能看到「第 N 轮重试当前档位」+ 上轮失败原因。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryRecord {
    pub retry_count: usize,
    pub retry_hint: String,
    pub started_offset_ms: u64,
    pub elapsed_ms: u64,
}

/// 分层执行记录(2026-09-16 第 57 轮):把 execute_workflows 的层结构 +
/// 并行/串行语义 + 每层耗时暴露给 TUI。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerInfo {
    /// 0-based 层序号
    pub layer_idx: usize,
    /// 本层 WorkFlow id 列表(保持确定性顺序)
    pub wf_ids: Vec<String>,
    /// 是否并行层(layer.len() > 1)
    pub parallel: bool,
    /// 本层开始相对任务起点的偏移(毫秒)
    pub started_offset_ms: u64,
    /// 本层耗时:并行层=墙钟,串行层=累计
    pub elapsed_ms: u64,
}

/// 任务结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub goal: String,
    pub classification: TaskClassification,
    pub plan_doc: Option<PathBuf>,
    pub workflows: Vec<WorkflowResult>,
    pub summary: String,
    pub total_usage: Usage,
    /// 2026-09-16 第 57 轮:阶段耗时记录(Yolo/Main-Work/QC-main/每个 WF/每个 QC/SessionContext)
    #[serde(default)]
    pub stage_durations: Vec<StageDuration>,
    /// 2026-09-16 第 57 轮:重试事件(retry_count + retry_hint + 耗时)
    #[serde(default)]
    pub retry_log: Vec<RetryRecord>,
    /// 2026-09-16 第 57 轮:分层执行信息
    #[serde(default)]
    pub layer_log: Vec<LayerInfo>,
    /// 2026-09-16 第 57 轮:任务总墙钟(进入 handle_inner 到离开)
    #[serde(default)]
    pub wallclock_ms: u64,
}

/// Orchestrator 终态
#[derive(Debug, Clone)]
pub enum OrchestrationOutcome {
    /// 直接回答(Yolo direct_answer,无下游执行)
    DirectAnswer {
        text: String,
        classification: TaskClassification,
        usage: Usage,
    },
    /// 已执行
    Executed { result: TaskResult },
    /// 失败(达到最大重试 / Yolo 给出 user_suggestion)
    Failed {
        classification: TaskClassification,
        /// 最后一轮的真实失败原因(2026-09-10 第 25 轮 F4):此前只有 suggestion,
        /// 可能与真实失败无关(如 Yolo 预判「jq 未安装」而实际是解析失败),误导用户。
        reason: String,
        suggestion: String,
        usage: Usage,
        /// 2026-09-16 第 58 轮 P0-C:最后一次失败的 ExecutionTrace,
        /// 让 Failed 分支能复用 format_task_result 渲染 [trace] / [tool] / [failure] 段,
        /// 用户一眼看到「哪个工具失败 / early_terminate_reason / failure_signals」。
        last_trace: Option<Arc<ExecutionTrace>>,
        /// 2026-09-16 第 58 轮 P0-C:阶段耗时,与 Executed 分支共享同一渲染管线。
        stage_durations: Vec<StageDuration>,
        /// 2026-09-16 第 58 轮 P0-C:重试记录(每轮的失败原因与耗时)。
        retry_log: Vec<RetryRecord>,
        /// 2026-09-16 第 58 轮 P0-C:任务总墙钟,毫秒。
        wallclock_ms: u64,
    },
}

/// 内部:失败信息
#[derive(Debug, Clone)]
pub(super) struct QualityFailure {
    pub(super) source: AgentRole,
    pub(super) reason: String,
    pub(super) retryable: bool,
    pub(super) suggestion: String,
    /// 用户取消触发的「失败」:不可重试、不回流 Yolo,直接短路退出整个任务。
    pub(super) cancelled: bool,
    /// 失败时的执行轨迹(2026-09-09 第 05 轮),便于 Yolo 失败回流时引用具体失败模式。
    pub(super) trace: Option<Arc<ExecutionTrace>>,
    /// 本轮已发生的 LLM 用量(2026-09-11 第 16 轮)。
    ///
    /// 失败路径原先只累加 Yolo 用量,终端 Failed 用量与 Debug Collector
    /// 统计严重不一致;SubAgent + QC 用量必须在 QC fail 时随失败一起回传。
    pub(super) usage: Usage,
    /// 第 146 轮:失败升级时**已完成且 QC 通过**的单元摘要(每单元一行:
    /// id + 职责截断 + 产物要点)。档位级重试/Hard 重规划此前只能从零重拆全量重跑
    /// (实测 v2 轮重跑了 v1 已通过的登录与建档单元);本字段随失败上抛,由
    /// pipeline 跨轮累计后注入 Plan/Main-Work 提示词,实现「只规划剩余部分」。
    pub(super) completed_digest: String,
}

/// 同因熔断器 —— 连续 N 轮失败原因完全相同即熔断,提前终止重试。
///
/// 背景(2026-10-10 实测):Plan 解析失败三轮完全同因(「未解析出任何
/// WorkFlow」),retry_hint 已注入仍原样失败,每轮照烧 Plan+QC ~107s。
/// 同因第 3 次重试的先验成功率极低,熔断把浪费上界从 max_retry_per_level
/// 收紧到 2 轮。
///
/// 这也是删除程序级「安全拒绝终态门」后**唯一**的通用空转止动器:模型自行
/// 拒绝任务时,其 Plan 输出同样解析不出 WorkFlow,归一化后三轮逐字相同,
/// 第 2 次即熔断收口(不再由编排器代替模型做价值判断)。
pub(super) struct SameCauseBreaker {
    last_norm: Option<String>,
    streak: usize,
    /// 连续相同原因达到该次数即熔断(默认 2:首次失败 + 一次修正机会)。
    threshold: usize,
}

/// 归一化失败原因(同因熔断的比对键)。
///
/// 口径:trim → 取首行(多行 reason 只比第一行)→ 截 120 字符。
/// 「解析 Plan 失败: 方案生成失败: Plan 文档未解析出任何 WorkFlow」这类
/// 稳定重复的 reason 三轮完全一致;而带时间戳/UUID 的 reason 首行也稳定。
fn normalize_failure_reason(reason: &str) -> String {
    let first_line = reason.trim().lines().next().unwrap_or("").trim();
    first_line.chars().take(120).collect()
}

impl SameCauseBreaker {
    pub(super) fn new() -> Self {
        Self {
            last_norm: None,
            streak: 0,
            threshold: 2,
        }
    }

    /// 记录一次失败原因;返回 `true` 表示熔断触发(应停止重试)。
    pub(super) fn record(&mut self, reason: &str) -> bool {
        let norm = normalize_failure_reason(reason);
        if norm.is_empty() {
            // 空原因不参与同因判定(无法归因的失败交给 max_retry 兜底)
            return false;
        }
        if Some(&norm) == self.last_norm.as_ref() {
            self.streak += 1;
        } else {
            self.streak = 1;
            self.last_norm = Some(norm);
        }
        self.streak >= self.threshold
    }

    /// 失败上下文变化时复位(Yolo 回流重分类后,失败语境已不同)。
    pub(super) fn reset(&mut self) {
        self.last_norm = None;
        self.streak = 0;
    }

    /// 当前连续同因次数(观测用)。
    pub(super) fn streak(&self) -> usize {
        self.streak
    }
}

impl Default for SameCauseBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl QualityFailure {
    /// 从 Agent 错误构造:取消错误标记 `cancelled=true` 且不可重试;
    /// 其余按可重试处理(与既有站点语义一致)。
    pub(super) fn from_agent_error(source: AgentRole, prefix: &str, e: &AgentError) -> Self {
        let cancelled = matches!(e, AgentError::Cancelled);
        Self {
            source,
            reason: format!("{prefix}: {e}"),
            retryable: !cancelled,
            suggestion: if cancelled {
                "任务已取消".into()
            } else {
                "重试".into()
            },
            cancelled,
            trace: None,
            usage: Usage::default(),
            completed_digest: String::new(),
        }
    }
}
