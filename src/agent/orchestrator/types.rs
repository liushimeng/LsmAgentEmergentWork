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
    pub subagent_max_iterations: usize,
    /// 同层无依赖 WorkFlow 的最大并行数(信号量上限,对齐 atomcode Semaphore(3) 惯例)
    pub max_parallel_workflows: usize,
    /// 调试事件采集器(`-debug` 调试模式时注入,默认 None 零开销)
    pub debug: Option<Arc<DebugCollector>>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_retry_per_level: 3,
            history_limit: DEFAULT_HISTORY_LIMIT,
            subagent_max_iterations: 16,
            max_parallel_workflows: 3,
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
        }
    }
}
