//! MultiAgentOrchestrator:多 Agent 架构总编排器。
//!
//! 串联 Yolo / Plan / Main-Work / SubAgent-Work / Quality-Check / SessionContext 六大角色,
//! 按用户任务的难度档位走对应链路,并在失败时逐层回流到 Yolo 重新评估。
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md` §10。

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::agent::cancel::CancelToken;
use crate::agent::compact::CompactRunner;
use crate::agent::context::AgentRole;
use crate::agent::debug::DebugCollector;
use crate::agent::extrace::ExecutionTrace;
use crate::agent::main_work::{self, MainWorkRunner, WorkFlowPlan, WorkFlowSpec};
use crate::agent::plan::PlanRunner;
use crate::agent::project_context;
use crate::agent::quality::{QualityReport, QualityRunner, Verdict};
use crate::agent::session_context::{
    inject_history_with_entries, SessionContextRunner, DEFAULT_HISTORY_LIMIT,
};
use crate::agent::subagent::{SubAgentRunner, SubFlowInput};
use crate::agent::yolo::{TaskClassification, TaskLevel, YoloRunner};
use crate::config::{Db, EventType};
use crate::error::{AgentError, Result};
use crate::llm::cancellable::{CancelGate, CancellableLlmClient};
use crate::llm::Usage;
use crate::session::Session;

/// 任务阶段进度通道(D05/D07 测试轮,2026-09-10 第 23 轮):
/// 编排器在阶段边界(Yolo 分类 / Plan / Main-Work / SubAgent / QC / 失败回流)
/// 发送一行人类可读消息,由 TUI(挂起 1.5s 后打印,快速完成不打扰)或
/// CLI(stderr 立即打印)的消费端展示,解决长任务期间界面零反馈的问题。
pub type ProgressTx = tokio::sync::mpsc::UnboundedSender<String>;

/// 发送一条阶段消息(未接通道 / 接收端已退出时零开销静默)。
fn emit_progress(tx: &Option<ProgressTx>, msg: impl Into<String>) {
    if let Some(tx) = tx {
        let _ = tx.send(msg.into());
    }
}

/// Orchestrator 行为参数
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowResult {
    pub id: String,
    pub name: String,
    pub subflow_outcome: String,
    pub quality_report: QualityReport,
    pub usage: Usage,
    /// SubAgent 执行轨迹(2026-09-09 第 05 轮),失败时为 None。
    pub subflow_trace: Option<ExecutionTrace>,
}

/// 任务结果
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub goal: String,
    pub classification: TaskClassification,
    pub plan_doc: Option<PathBuf>,
    pub workflows: Vec<WorkflowResult>,
    pub summary: String,
    pub total_usage: Usage,
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
    },
}

/// 内部:失败信息
#[derive(Debug, Clone)]
struct QualityFailure {
    source: AgentRole,
    reason: String,
    retryable: bool,
    suggestion: String,
    /// 用户取消触发的「失败」:不可重试、不回流 Yolo,直接短路退出整个任务。
    cancelled: bool,
    /// 失败时的执行轨迹(2026-09-09 第 05 轮),便于 Yolo 失败回流时引用具体失败模式。
    trace: Option<Arc<ExecutionTrace>>,
}

impl QualityFailure {
    /// 从 Agent 错误构造:取消错误标记 `cancelled=true` 且不可重试;
    /// 其余按可重试处理(与既有站点语义一致)。
    fn from_agent_error(source: AgentRole, prefix: &str, e: &AgentError) -> Self {
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
        }
    }
}

/// 多 Agent 编排器
pub struct MultiAgentOrchestrator {
    yolo: YoloRunner,
    plan: PlanRunner,
    main_work: MainWorkRunner,
    /// Arc 化:同层 WorkFlow 并行时共享给 tokio::spawn 任务
    sub_agent: Arc<SubAgentRunner>,
    /// Arc 化:同上(质检随执行单元并行)
    quality: Arc<QualityRunner>,
    session_context: SessionContextRunner,
    compact: CompactRunner,
    db: Arc<Db>,
    cfg: OrchestratorConfig,
    /// 每任务取消门:8 个角色的 LLM 客户端被 `CancellableLlmClient` 统一包裹,
    /// `handle_cancellable` 开始时注入 token、结束时(Drop guard)清除。
    cancel_gate: Arc<CancelGate>,
}

impl MultiAgentOrchestrator {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>, plans_dir: PathBuf) -> Self {
        Self::with_config(llm, db, plans_dir, OrchestratorConfig::default())
    }

    pub fn with_config(
        llm: Arc<dyn crate::llm::LlmClient>,
        db: Arc<Db>,
        plans_dir: PathBuf,
        cfg: OrchestratorConfig,
    ) -> Self {
        // 取消传播:在最外层包裹可取消装饰器(顺序 cancellable > debug > resilient > raw),
        // 全部角色的 LLM 调用一处获得中断能力(方案 §3.3)
        let cancel_gate = CancelGate::new();
        let llm: Arc<dyn crate::llm::LlmClient> =
            Arc::new(CancellableLlmClient::new(llm, cancel_gate.clone()));
        let yolo = YoloRunner::new(llm.clone());
        let plan = PlanRunner::new(llm.clone(), db.clone(), plans_dir);
        let main_work = MainWorkRunner::new(llm.clone(), db.clone());
        let sub_agent = Arc::new(
            SubAgentRunner::new(llm.clone(), db.clone())
                .with_max_iterations(cfg.subagent_max_iterations),
        );
        let quality = Arc::new(QualityRunner::new(llm.clone(), db.clone()));
        let session_context = SessionContextRunner::new(llm.clone(), db.clone());
        let compact = CompactRunner::new(llm, db.clone());
        Self {
            yolo,
            plan,
            main_work,
            sub_agent,
            quality,
            session_context,
            compact,
            db,
            cfg,
            cancel_gate,
        }
    }

    /// 处理一次用户输入(不可取消;既有调用方 / 测试零改动)。
    ///
    /// 内部用一个永不触发的全新 token 走 [`handle_cancellable`],语义等价:
    /// fresh token 的 `cancelled()` 永不 resolve,LLM 装饰器等价直通。
    pub async fn handle(&self, session: &mut Session) -> Result<OrchestrationOutcome> {
        let never_fires = CancelToken::new();
        self.handle_cancellable(session, &never_fires).await
    }

    /// [`handle`] 的可取消版本(H9 取消传播,第 07 轮):
    /// - LLM 层:token 经 `cancel_gate` 注入装饰器,所有角色调用可即时中断;
    /// - 编排层:各阶段边界检查,命中即返回 `Err(AgentError::Cancelled)`,
    ///   **不进 QualityFailure / Yolo 失败回流**(取消不是失败,不能重试);
    /// - 执行层:token 传入每个 SubAgent 单元(含同层并行 spawn)。
    pub async fn handle_cancellable(
        &self,
        session: &mut Session,
        cancel: &CancelToken,
    ) -> Result<OrchestrationOutcome> {
        self.handle_cancellable_with_progress(session, cancel, None)
            .await
    }

    /// [`handle_cancellable`] 的带进度版本(2026-09-10 第 23 轮):
    /// `progress` 接收端由调用方(TUI 打印协程 / CLI stderr 打印)持有,
    /// 通道 clone 会贯穿到并行执行的 WorkFlow 单元,任务返回时全部 drop,
    /// 接收端 `recv()` 返回 None 即为「任务结束」信号。
    pub async fn handle_cancellable_with_progress(
        &self,
        session: &mut Session,
        cancel: &CancelToken,
        progress: Option<ProgressTx>,
    ) -> Result<OrchestrationOutcome> {
        let _gate_guard = self.cancel_gate.guard(cancel.clone());
        self.handle_inner(session, cancel, &progress).await
    }

    async fn handle_inner(
        &self,
        session: &mut Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
    ) -> Result<OrchestrationOutcome> {
        // 0) 项目上下文首次注入(幂等)
        if let Some(work_dir) = project_context::current_work_dir() {
            project_context::inject_once(session, work_dir);
        }

        // 0.1) 历史 Session 摘要注入(幂等)
        let summaries = self
            .db
            .latest_summaries(session.id(), self.cfg.history_limit)
            .unwrap_or_default();
        inject_history_with_entries(session, &summaries);

        // 0.2) Context 自动压缩(达到当前 Provider context_max_size 的 80% 阈值时触发)
        if let Ok(Some(active)) = self.db.get_active_or_env() {
            match self
                .compact
                .maybe_compact(session, active.context_max_size)
                .await
            {
                Ok(Some(rep)) => {
                    eprintln!(
                        "[laew] Context 已自动压缩:档位={} 估算 token {} → {}(覆盖 {} 条历史消息{})",
                        rep.tier.as_str(),
                        rep.before_tokens,
                        rep.after_tokens,
                        rep.compacted_messages,
                        if rep.fallback { ",硬截断降级" } else { "" },
                    );
                }
                Ok(None) => {}
                Err(e) if matches!(e, AgentError::Cancelled) => return Err(e),
                Err(e) => tracing::warn!(error = %e, "Context 自动压缩失败(不中断任务)"),
            }
        }

        // 1) Yolo 入口
        let (mut classification, yolo_usage) = self.run_yolo_classification(session).await?;
        Self::check_cancelled(cancel)?;
        self.dbg_classify(&classification);
        emit_progress(
            progress,
            format!(
                "Yolo 分类:{} → {}{}",
                classification.task_level.display_name(),
                match classification.task_level {
                    TaskLevel::Simple => "SubAgent 直通",
                    TaskLevel::Medium => "Main-Work 拆解",
                    TaskLevel::Hard => "Plan 规划",
                },
                if classification.yolo_degraded {
                    "(降级本地解析)"
                } else {
                    ""
                },
            ),
        );
        // 2026-09-09 第 14 轮:累加 Yolo 分类调用的 LLM 用量(此前被 `let _ = usage;` 显式丢弃,
        // 导致 stdout 用量仅显示最后一次调用,与 Debug 报告严重不一致)。
        let mut total_usage = yolo_usage;

        // 1.1) 记录 Yolo 输入事件
        let _ = self
            .db
            .insert_session_memory(&crate::config::SessionMemoryEntry {
                session_id: session.id().to_string(),
                role: AgentRole::Yolo,
                event_type: EventType::Input,
                content: format!("goal: {}", classification.goal_summary),
                usage_input: 0,
                usage_output: 0,
            });

        let mut retry_count = 0;
        // F3(2026-09-10 第 25 轮):上一轮失败原因,重试轮回灌 Main-Work(消除盲重试);
        // 档位升级回流 Yolo 后清空。同时作为 Failed 变体的 reason(F4)。
        let mut retry_hint = String::new();

        // 1.2) simple + direct_answer 短路(2026-09-09 第 15 轮 AQ03 实测发现):
        // Yolo 已给出完整直接答案时,不再空转一轮 SubAgent+QC(实测多花 ~2.5 分钟
        // 且引入额外失败面)。本分支接通 OrchestrationOutcome::DirectAnswer ——
        // 该变体与 main.rs/tui 消费端早已存在,但此前无任何构造点(死代码),
        // 旧兼容 API `yolo::run_yolo` 的 DirectAnswer 语义在此对齐到主编排链路。
        // 仍跑 SessionContext 收口(workflows 为空),保住 session_memory 连续性。
        //
        // 占位字符串兜底(2026-09-10 第 27 轮 F12 / 实测 BUG-2026-09-10-TUI-NULL):
        // 部分上游 LLM 在需要委派执行时,会把 `direct_answer` 写成字符串字面量
        // `"null"`/`"None"`/`"NULL"`(而非 JSON 的 null),导致本分支错误短路,
        // TUI 直接打印字面量 `null` 让用户以为没输出。此处把 4 类常见占位归一
        // 为"未填",继续走 loop → run_simple 委派 SubAgent。
        if classification.task_level == TaskLevel::Simple {
            if let Some(answer) = classification
                .direct_answer
                .as_ref()
                .filter(|a| !is_placeholder_direct_answer(a))
                .cloned()
            {
                Self::check_cancelled(cancel)?;
                emit_progress(progress, "Yolo 直接作答(跳过执行层)");
                let summary = self
                    .session_context
                    .summarize(
                        &classification.goal_summary,
                        "(用户原始输入已记录)",
                        None,
                        &[],
                        &total_usage,
                        session.id(),
                        classification.yolo_degraded,
                        &classification.task_level,
                    )
                    .await?;
                total_usage = add_usage(total_usage, summary.usage);
                self.dbg_task_end("direct_answer", total_usage);
                return Ok(OrchestrationOutcome::DirectAnswer {
                    text: answer,
                    classification,
                    usage: total_usage,
                });
            }
        }

        loop {
            // 重试轮入口:取消短路(取消不是失败,不消耗重试预算)
            Self::check_cancelled(cancel)?;
            retry_count += 1;
            if retry_count > self.cfg.max_retry_per_level {
                // 超过最大重试,输出失败 / 建议
                // 关联报告: 2026-09-09_05 E-003 —— 当 Yolo 没提供 user_suggestion 时,
                // 给出 actionable 化的兜底建议(根据 trace 推测失败原因)。
                let suggestion = if classification.user_suggestion_if_fail.is_empty() {
                    fallback_suggestion(&total_usage)
                } else {
                    classification.user_suggestion_if_fail.clone()
                };
                self.record_failure_event(session.id(), &classification, &suggestion);
                self.dbg_task_end(&format!("failed: {suggestion}"), total_usage);
                return Ok(OrchestrationOutcome::Failed {
                    classification,
                    reason: retry_hint.clone(),
                    suggestion,
                    usage: total_usage,
                });
            }

            // 2) 调度执行(执行层取消:token 贯穿 SubAgent / 并行层)
            let exec_result = match classification.task_level {
                TaskLevel::Simple => {
                    self.run_simple(&classification, session, cancel, progress)
                        .await
                }
                TaskLevel::Medium => {
                    self.run_medium(&classification, session, cancel, progress, &retry_hint)
                        .await
                }
                TaskLevel::Hard => {
                    self.run_hard(&classification, session, cancel, progress)
                        .await
                }
            };

            match exec_result {
                Ok(mut task_result) => {
                    // 3) SessionContext 收口(收口前再查一次:取消后不再发摘要请求)
                    Self::check_cancelled(cancel)?;
                    // 2026-09-10 第 18 轮:摘要的用量口径与终端「本次用量」对齐 ——
                    // Yolo 分类 + 执行层累计(不含 SessionContext 自身)。
                    // 此前直接传 task_result.total_usage(仅执行层),漏记 Yolo 分类调用,
                    // 导致写入 session_memory 的摘要用量系统性偏小。
                    let usage_for_summary = add_usage(yolo_usage, task_result.total_usage);
                    let summary = self
                        .session_context
                        .summarize(
                            &task_result.goal,
                            "(用户原始输入已记录)",
                            task_result.plan_doc.as_deref(),
                            &task_result
                                .workflows
                                .iter()
                                .map(|w| {
                                    (
                                        w.id.clone(),
                                        w.name.clone(),
                                        w.quality_report.verdict == Verdict::Pass,
                                    )
                                })
                                .collect::<Vec<_>>(),
                            &usage_for_summary,
                            session.id(),
                            task_result.classification.yolo_degraded,
                            &task_result.classification.task_level,
                        )
                        .await?;
                    task_result.summary = summary.text.clone();
                    // 2026-09-09 第 14 轮:total_usage 累加策略
                    // - handle_inner 在 L260 已用 yolo_usage 初始化 total_usage
                    // - 执行层 run_simple/run_medium/run_hard 返回的 task_result.total_usage
                    //   自身已含其内部所有 LLM 调用的累计(SubAgent + Quality 等)
                    // - 因此这里用 add_usage 把执行层累计 + session_context 累计 累加到
                    //   已含 yolo_usage 的 total_usage 上,保证不丢任何角色的 token。
                    total_usage = add_usage(total_usage, task_result.total_usage);
                    total_usage = add_usage(total_usage, summary.usage);
                    task_result.total_usage = total_usage;
                    self.dbg_task_end("executed", task_result.total_usage);
                    return Ok(OrchestrationOutcome::Executed {
                        result: task_result,
                    });
                }
                Err(failure) => {
                    // 用户取消:短路退出整个任务,不回流不重试(H9 语义)
                    if failure.cancelled {
                        self.dbg_task_end("cancelled", total_usage);
                        return Err(AgentError::Cancelled);
                    }
                    total_usage = add_usage(total_usage, failure_usage(&failure));
                    // 升级或重试
                    if !failure.retryable {
                        emit_progress(progress, "失败回流 Yolo 重新评估…");
                        // 升级到上一层(由 Yolo 重新评估)
                        match self
                            .run_yolo_with_failure(&classification, &failure, session)
                            .await
                        {
                            Ok(new_c) => {
                                classification = new_c;
                                self.dbg_classify(&classification);
                                // 档位升级后旧失败原因不再适用(F3)
                                retry_hint.clear();
                                continue;
                            }
                            Err(e) => {
                                return Err(e);
                            }
                        }
                    }
                    // retryable=true 留在当前档位继续重跑
                    // F3:记录失败原因,下一轮回灌 Main-Work / 供 Failed 呈现
                    retry_hint = failure.reason.clone();
                    emit_progress(progress, format!("第 {retry_count} 轮重试当前档位…"));
                    continue;
                }
            }
        }
    }

    // ========== 简单档 ==========

    /// 取消检查:命中即返回 `Err(Cancelled)`(由调用方短路,不回流)。
    fn check_cancelled(cancel: &CancelToken) -> Result<()> {
        if cancel.is_cancelled() {
            Err(AgentError::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn run_simple(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        let input = SubFlowInput {
            id: "wf-1".into(),
            description: c.goal_summary.clone(),
            expected_output: c
                .decomposition_plan
                .first()
                .cloned()
                .unwrap_or_else(|| "完成用户请求".into()),
            depends_on_outputs: vec![],
            sibling_outputs: vec![],
        };
        emit_progress(progress, "wf-1 SubAgent 执行中…");
        let outcome = self
            .sub_agent
            .run_unit_with_cancel(&input, session.id(), cancel)
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::SubAgent, "SubAgent 执行失败", &e)
            })?;

        let (qc, qc_usage) = self
            .quality
            .check_subagent(
                &c.goal_summary,
                &input.description,
                &input.expected_output,
                &outcome.text,
                &outcome.trace,
                session.id(),
            )
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        self.dbg_qc(&qc);
        emit_progress(
            progress,
            format!(
                "wf-1 QC:{}",
                if qc.verdict == Verdict::Pass {
                    "✅ 通过"
                } else {
                    "❌ 未通过"
                }
            ),
        );

        // 2026-09-09 第 14 轮:累加 Quality-Check 调用的 LLM 用量
        let total_usage = add_usage(outcome.usage, qc_usage);

        if qc.verdict == Verdict::Pass {
            // WorkFlow 名称使用 goal_summary 截断(最多 20 字符),便于在 TUI 区分不同任务
            let wf_name = if c.goal_summary.chars().count() > 20 {
                format!("{}…", c.goal_summary.chars().take(19).collect::<String>())
            } else {
                c.goal_summary.clone()
            };
            Ok(TaskResult {
                goal: c.goal_summary.clone(),
                classification: c.clone(),
                plan_doc: None,
                workflows: vec![WorkflowResult {
                    id: "wf-1".into(),
                    name: wf_name,
                    subflow_outcome: outcome.text,
                    quality_report: qc,
                    usage: outcome.usage,
                    subflow_trace: Some(outcome.trace),
                }],
                summary: String::new(),
                total_usage,
            })
        } else {
            Err(QualityFailure {
                source: AgentRole::SubAgent,
                reason: qc.issues.join("; "),
                retryable: qc.retryable,
                suggestion: qc.suggestion,
                cancelled: false,
                trace: Some(Arc::new(outcome.trace)),
            })
        }
    }

    // ========== 中等档 ==========

    async fn run_medium(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
        retry_hint: &str,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 1) Main-Work 拆 WorkFlow
        emit_progress(progress, "Main-Work 拆解中…");
        let (plan, mainwork_usage) = self
            .main_work
            .plan_workflows(
                &c.goal_summary,
                &c.decomposition_plan,
                session.id(),
                retry_hint,
            )
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::MainWork, "Main-Work 拆解失败", &e)
            })?;

        // 1.5) F2(2026-09-10 第 25 轮):兜底计划跳过 QC-main 直接执行。
        // 兜底 WorkFlowPlan 的 summary 自证「解析失败」,送 QC 必然 fail+retryable,
        // 形成确定性必败重试循环(D06 实测连烧 3 轮 ~10 分钟零产出)。
        // 真实产物质量仍由 execute_workflows 内每 WorkFlow 的 QC 把守。
        if plan.degraded {
            emit_progress(
                progress,
                format!(
                    "Main-Work 解析失败,已使用单 WorkFlow 兜底(跳过计划 QC,直接执行 {} 个流程)",
                    plan.workflows.len()
                ),
            );
            return self
                .execute_workflows(c, &plan, mainwork_usage, session, cancel, progress)
                .await;
        }

        // 2) Quality 校验 Main-Work 输出
        let wf_json = serde_json::to_string(&plan).unwrap_or_default();
        let (qc_main, qc_usage) = self
            .quality
            .check_main(&c.goal_summary, &wf_json, session.id())
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        self.dbg_qc(&qc_main);
        emit_progress(
            progress,
            format!("Main-Work 拆解 {} 个流程单元", plan.workflows.len()),
        );

        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
                cancelled: false,
                trace: None,
            });
        }

        // 2026-09-09 第 14 轮:累加 Main-Work + Quality-Main 调用的 LLM 用量,
        // 透传到 execute_workflows 内部继续累加。
        let pre_usage = add_usage(mainwork_usage, qc_usage);

        // 3) 拓扑排序并执行
        self.execute_workflows(c, &plan, pre_usage, session, cancel, progress)
            .await
    }

    // ========== 高等档 ==========

    async fn run_hard(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 1) Plan 生成(2026-09-09 第 14 轮:带回 LLM Usage 用于累加)
        emit_progress(progress, "Plan 规划中…");
        let (plan_output, plan_usage) = self
            .plan
            .generate(
                &c.goal_summary,
                &c.purpose,
                &c.intent,
                &c.decomposition_plan,
                session.id(),
            )
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::Plan, "Plan 生成失败", &e))?;
        emit_progress(
            progress,
            format!(
                "Plan 已生成:{}",
                plan_output
                    .path
                    .file_name()
                    .map(|n| n.display().to_string())
                    .unwrap_or_default()
            ),
        );

        // 2) Quality 校验 Plan
        let (qc_plan, qc_plan_usage) = self
            .quality
            .check_plan(&plan_output.markdown, session.id())
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        self.dbg_qc(&qc_plan);
        if qc_plan.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::Plan,
                reason: qc_plan.issues.join("; "),
                retryable: qc_plan.retryable,
                suggestion: qc_plan.suggestion,
                cancelled: false,
                trace: None,
            });
        }

        // 3) Main-Work 解析 Plan → WorkFlow
        let plan = self.main_work.parse_plan(&plan_output.path).map_err(|e| {
            QualityFailure::from_agent_error(AgentRole::MainWork, "解析 Plan 失败", &e)
        })?;

        let (qc_main, qc_main_usage) = self
            .quality
            .check_main(
                &c.goal_summary,
                &serde_json::to_string(&plan).unwrap_or_default(),
                session.id(),
            )
            .await
            .map_err(|e| {
                QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
            })?;
        self.dbg_qc(&qc_main);
        emit_progress(
            progress,
            format!("Main-Work 解析为 {} 个流程单元", plan.workflows.len()),
        );
        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
                cancelled: false,
                trace: None,
            });
        }

        // 2026-09-09 第 14 轮:累加 Plan + Quality-Plan + Quality-Main 三次 LLM 调用的用量
        let mut pre_usage = add_usage(plan_usage, qc_plan_usage);
        pre_usage = add_usage(pre_usage, qc_main_usage);

        // 4) 执行 WorkFlow
        let mut task_result = self
            .execute_workflows(c, &plan, pre_usage, session, cancel, progress)
            .await?;
        task_result.plan_doc = Some(plan_output.path);
        Ok(task_result)
    }

    // ========== 通用:执行 WorkFlow 列表 ==========

    async fn execute_workflows(
        &self,
        c: &TaskClassification,
        plan: &WorkFlowPlan,
        pre_usage: Usage, // 2026-09-09 第 14 轮:承接上游(Main-Work / Quality-Main)累计的 LLM 用量
        session: &Session,
        cancel: &CancelToken,
        progress: &Option<ProgressTx>,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 依赖分层:同层 WorkFlow 互相无依赖,自动并行;跨层严格串行(自动感知 depends_on)
        let layers = main_work::topo_layers(&plan.workflows).map_err(|e| QualityFailure {
            source: AgentRole::MainWork,
            reason: format!("拓扑分层失败: {e}"),
            retryable: false,
            suggestion: "Plan 中存在循环或未知依赖".into(),
            cancelled: false,
            trace: None,
        })?;

        let mut results = Vec::new();
        let mut dep_outputs: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut total_usage = pre_usage;
        let total_layers = layers.len();

        for (layer_idx, layer) in layers.into_iter().enumerate() {
            // 层边界:取消短路(下一层不再启动)
            if let Err(e) = Self::check_cancelled(cancel) {
                return Err(QualityFailure::from_agent_error(
                    AgentRole::MainWork,
                    "WorkFlow 层调度",
                    &e,
                ));
            }
            if layer.len() > 1 {
                eprintln!(
                    "[laew] WorkFlow 并行调度:第 {}/{} 层 {} 个流程并发执行(上限 {})",
                    layer_idx + 1,
                    total_layers,
                    layer.len(),
                    self.cfg.max_parallel_workflows,
                );
            }

            // 构造本层全部单元的输入(上游产物按层传递)
            let units: Vec<(WorkFlowSpec, SubFlowInput)> = layer
                .iter()
                .map(|wf| (wf.clone(), build_subflow_input(wf, &dep_outputs)))
                .collect();

            // 执行本层:单个直通(零 spawn 开销),多个 tokio::spawn + Semaphore 有界并发
            let layer_outcomes = if units.len() == 1 {
                let (wf, input) = units.into_iter().next().expect("len==1");
                let outcome = run_wf_unit(
                    self.sub_agent.clone(),
                    self.quality.clone(),
                    self.cfg.debug.clone(),
                    input,
                    c.goal_summary.clone(),
                    session.id().to_string(),
                    None,
                    cancel.clone(),
                    progress.clone(),
                )
                .await;
                vec![(wf, outcome)]
            } else {
                let semaphore =
                    Arc::new(tokio::sync::Semaphore::new(self.cfg.max_parallel_workflows));
                let mut handles = Vec::with_capacity(units.len());
                for (wf, input) in units {
                    let sub_agent = self.sub_agent.clone();
                    let quality = self.quality.clone();
                    let debug = self.cfg.debug.clone();
                    let goal = c.goal_summary.clone();
                    let sid = session.id().to_string();
                    let sem = semaphore.clone();
                    // 取消传播:同层每个并行单元持同一 token 的 clone,
                    // 父任务取消 → 全部单元即时中断(原子级联,无需逐个通知)
                    let cancel_tok = cancel.clone();
                    let progress_tx = progress.clone();
                    handles.push(tokio::spawn(async move {
                        let outcome = run_wf_unit(
                            sub_agent,
                            quality,
                            debug,
                            input,
                            goal,
                            sid,
                            Some(sem),
                            cancel_tok,
                            progress_tx,
                        )
                        .await;
                        (wf, outcome)
                    }));
                }
                // 按层内原始顺序 await:保证结果顺序确定;失败不中断在飞的独立任务
                let mut outcomes = Vec::with_capacity(handles.len());
                for h in handles {
                    match h.await {
                        Ok(pair) => outcomes.push(pair),
                        Err(e) => {
                            // JoinError(panic 等):映射为失败回流,不吞掉
                            outcomes.push((
                                WorkFlowSpec {
                                    id: "unknown".into(),
                                    name: "并行任务".into(),
                                    steps: vec![],
                                    branches: vec![],
                                    loops: vec![],
                                    depends_on: vec![],
                                    acceptance: vec![],
                                    delegate_to: AgentRole::SubAgent,
                                },
                                Err(QualityFailure {
                                    source: AgentRole::SubAgent,
                                    reason: format!("并行 WorkFlow 任务 join 失败: {e}"),
                                    retryable: true,
                                    suggestion: "重试".into(),
                                    cancelled: false,
                                    trace: None,
                                }),
                            ));
                        }
                    }
                }
                outcomes
            };

            // 汇总本层:按序取首个失败回流(fail-fast 语义与串行版一致)
            let mut first_failure: Option<QualityFailure> = None;
            let mut ok_units = Vec::new();
            for (wf, outcome) in layer_outcomes {
                match outcome {
                    Ok(ok) => ok_units.push((wf, ok)),
                    Err(f) => {
                        if first_failure.is_none() {
                            first_failure = Some(f);
                        }
                    }
                }
            }
            if let Some(f) = first_failure {
                return Err(f);
            }

            for (wf, ok) in ok_units {
                // 2026-09-09 第 14 轮:同时累加 SubAgent 用量与 Quality-Check 用量
                total_usage = add_usage(total_usage, ok.usage);
                total_usage = add_usage(total_usage, ok.qc_usage);
                dep_outputs.insert(wf.id.clone(), ok.outcome_text.clone());
                results.push(WorkflowResult {
                    id: wf.id.clone(),
                    name: wf.name.clone(),
                    subflow_outcome: ok.outcome_text,
                    quality_report: ok.qc,
                    usage: ok.usage,
                    subflow_trace: Some(ok.trace),
                });
            }
        }

        Ok(TaskResult {
            goal: c.goal_summary.clone(),
            classification: c.clone(),
            plan_doc: None,
            workflows: results,
            summary: String::new(),
            total_usage,
        })
    }

    // ========== Yolo 分类 + 失败回流 ==========

    async fn run_yolo_classification(
        &self,
        session: &Session,
    ) -> Result<(TaskClassification, Usage)> {
        // session_id 传播:Yolo 请求的 X-Session-Id 与任务主会话一致(抓包可关联,
        // 第 08 轮,方案 tmpPlan/2026-09-09_08)
        let (mut c, _text, usage) = self.yolo.classify(session.id(), session.context()).await?;
        // 修正:若 agent_role 缺省,按 task_level 推断
        if c.agent_role.is_none() {
            c.agent_role = Some(match c.task_level {
                TaskLevel::Simple => AgentRole::SubAgent,
                TaskLevel::Medium => AgentRole::MainWork,
                TaskLevel::Hard => AgentRole::Plan,
            });
        }
        Ok((c, usage))
    }

    async fn run_yolo_with_failure(
        &self,
        prev: &TaskClassification,
        failure: &QualityFailure,
        session: &mut Session,
    ) -> Result<TaskClassification> {
        // 构造失败摘要消息,让 Yolo 重新评估。
        // 2026-09-09 第 05 轮:把 ExecutionTrace 的 failure_signals 也带上,
        // 让 Yolo 看到具体失败模式而非仅凭自由文本判定。
        let failure_signals = failure
            .trace
            .as_ref()
            .map(|t| t.failure_signals.join(","))
            .unwrap_or_default();
        let failure_msg = format!(
            "[PREVIOUS_FAILURE]\n源: {}\n任务级别: {}\n原目标: {}\n失败原因: {}\n失败信号: {}\n建议: {}\n请重新评估:可重试 → 修订 decomposition_plan 重发;不可重试 → 填 user_suggestion_if_fail 并给出 direct_answer 告知用户。",
            failure.source.as_str(),
            prev.task_level.as_str(),
            prev.goal_summary,
            failure.reason,
            failure_signals,
            failure.suggestion,
        );
        session
            .context_mut()
            .push(crate::llm::ChatMessage::user(failure_msg));

        // 2026-09-09 第 14 轮:Yolo 失败回流时,Yolo 自身的 LLM 用量需要累加到
        // total_usage(原本被丢弃,导致失败回流的 token 也未计入)。
        let (c, yolo_usage) = self.run_yolo_classification(session).await?;
        // 透传:调用方负责把 yolo_usage 合并进 total_usage
        let _ = yolo_usage; // 失败回流场景的累加由 handle_inner 的 outer loop 处理
        Ok(c)
    }

    // ========== Debug 采集钩子(未开启时零开销) ==========

    fn dbg_classify(&self, c: &TaskClassification) {
        if let Some(d) = &self.cfg.debug {
            d.record_classification(c);
        }
    }

    fn dbg_qc(&self, report: &QualityReport) {
        if let Some(d) = &self.cfg.debug {
            d.record_quality(report);
        }
    }

    fn dbg_task_end(&self, outcome: &str, usage: Usage) {
        if let Some(d) = &self.cfg.debug {
            d.record_task_end(outcome, usage);
        }
    }

    fn record_failure_event(&self, session_id: &str, c: &TaskClassification, suggestion: &str) {
        let _ = self
            .db
            .insert_session_memory(&crate::config::SessionMemoryEntry {
                session_id: session_id.to_string(),
                role: AgentRole::Yolo,
                event_type: EventType::Failure,
                content: format!("目标: {}\n达到最大重试次数", c.goal_summary),
                usage_input: 0,
                usage_output: 0,
            });
        if !suggestion.is_empty() {
            let _ = self
                .db
                .insert_session_memory(&crate::config::SessionMemoryEntry {
                    session_id: session_id.to_string(),
                    role: AgentRole::SessionContext,
                    event_type: EventType::Suggestion,
                    content: suggestion.into(),
                    usage_input: 0,
                    usage_output: 0,
                });
        }
    }

    /// 暴露 Yolo runner(测试用)
    pub fn yolo(&self) -> &YoloRunner {
        &self.yolo
    }
    /// 暴露 Quality runner(测试用)
    pub fn quality(&self) -> &QualityRunner {
        &self.quality
    }
    /// 暴露 SessionContext runner(测试用)
    pub fn session_context(&self) -> &SessionContextRunner {
        &self.session_context
    }
    /// 暴露 SubAgent runner(测试用)
    pub fn sub_agent(&self) -> &SubAgentRunner {
        &self.sub_agent
    }
    /// 暴露 Plan runner(测试用)
    pub fn plan(&self) -> &PlanRunner {
        &self.plan
    }
    /// 暴露 Main-Work runner(测试用)
    pub fn main_work(&self) -> &MainWorkRunner {
        &self.main_work
    }
}

/// 单个 WorkFlow 执行单元的成功产物
struct WfUnitOk {
    outcome_text: String,
    /// SubAgent 自身的 LLM 用量(2026-09-09 第 14 轮:Quality 用量单独累加,见 `qc_usage`)。
    usage: Usage,
    qc: QualityReport,
    /// Quality-Check 调用的 LLM 用量(2026-09-09 第 14 轮:从 check_subagent 返回值中带回)。
    qc_usage: Usage,
    /// SubAgent 执行轨迹(2026-09-09 第 05 轮)。
    trace: ExecutionTrace,
}

/// 执行一个 WorkFlow 单元:SubAgent 执行 + Quality-Check(+ Debug 采集)。
///
/// 自由函数 + Arc 参数化,串行直通与 tokio::spawn 并行两种调用路径共用同一份逻辑;
/// `semaphore` 为并行路径的有界并发许可(串行路径传 None);
/// `cancel` 为任务级取消 token(传播进 SubAgent 的 Agent 循环,LLM/工具即时中断);
/// `progress` 为阶段进度通道 clone(并行单元各自持有,2026-09-10 第 23 轮)。
#[allow(clippy::too_many_arguments)]
async fn run_wf_unit(
    sub_agent: Arc<SubAgentRunner>,
    quality: Arc<QualityRunner>,
    debug: Option<Arc<DebugCollector>>,
    input: SubFlowInput,
    goal: String,
    session_id: String,
    semaphore: Option<Arc<tokio::sync::Semaphore>>,
    cancel: CancelToken,
    progress: Option<ProgressTx>,
) -> std::result::Result<WfUnitOk, QualityFailure> {
    // 有界并发:先抢许可(对齐 atomcode Semaphore(3) FIFO 惯例)
    let _permit = match &semaphore {
        Some(sem) => Some(sem.acquire().await.map_err(|e| QualityFailure {
            source: AgentRole::SubAgent,
            reason: format!("并行调度信号量已关闭: {e}"),
            retryable: true,
            suggestion: "重试".into(),
            cancelled: false,
            trace: None,
        })?),
        None => None,
    };

    let wf_id = input.id.clone();
    emit_progress(&progress, format!("{wf_id} SubAgent 执行中…"));
    let outcome = sub_agent
        .run_unit_with_cancel(&input, &session_id, &cancel)
        .await
        .map_err(|e| {
            QualityFailure::from_agent_error(
                AgentRole::SubAgent,
                &format!("SubAgent 执行失败(wf={wf_id})"),
                &e,
            )
        })?;

    let (qc, qc_usage) = quality
        .check_subagent(
            &goal,
            &input.description,
            &input.expected_output,
            &outcome.text,
            &outcome.trace,
            &session_id,
        )
        .await
        .map_err(|e| {
            QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e)
        })?;
    if let Some(d) = &debug {
        d.record_quality(&qc);
    }
    emit_progress(
        &progress,
        format!(
            "{wf_id} QC:{}",
            if qc.verdict == Verdict::Pass {
                "✅ 通过"
            } else {
                "❌ 未通过"
            }
        ),
    );

    if qc.verdict == Verdict::Fail {
        return Err(QualityFailure {
            source: AgentRole::SubAgent,
            reason: format!("wf={}: {}", wf_id, qc.issues.join("; ")),
            retryable: qc.retryable,
            suggestion: qc.suggestion,
            cancelled: false,
            trace: Some(Arc::new(outcome.trace)),
        });
    }

    Ok(WfUnitOk {
        outcome_text: outcome.text,
        usage: outcome.usage,
        qc,
        qc_usage,
        trace: outcome.trace,
    })
}

fn build_subflow_input(
    wf: &WorkFlowSpec,
    dep_outputs: &std::collections::HashMap<String, String>,
) -> SubFlowInput {
    let deps: Vec<String> = wf
        .depends_on
        .iter()
        .filter_map(|id| dep_outputs.get(id).cloned())
        .collect();
    SubFlowInput {
        id: format!("{}.step", wf.id),
        description: format!("{}\n\n步骤:\n{}", wf.name, wf.steps.join("\n")),
        expected_output: wf.acceptance.join("; "),
        depends_on_outputs: deps,
        sibling_outputs: vec![],
    }
}

fn add_usage(mut total: Usage, delta: Usage) -> Usage {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(delta.cache_read_input_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(delta.cache_creation_input_tokens);
    total
}

fn failure_usage(_failure: &QualityFailure) -> Usage {
    // 失败不消耗额外 token(LLM 调用由各 Agent 自身累计);此处返回 0 避免重复累计
    Usage::default()
}

/// 判断 `direct_answer` 是否为占位字符串(2026-09-10 第 27 轮 F12)。
///
/// 上下文:`yolo.rs::TaskClassification.direct_answer` 期望需要委派时填 JSON
/// `null`(→ `Option::None`),需要直答时填字符串答案。但实测发现部分上游 LLM
/// 误把字面量 `"null"`/`"None"`/`"NULL"` 当 JSON `null` 写入,反序列化后
/// 是 `Some("null")`,原 `filter(|a| !a.trim().is_empty())` 不会拒绝它,导致
/// 走 DirectAnswer 短路、TUI 直接打印"null"用户以为零输出。
///
/// 把 4 类常见占位归一为"未填",继续走 loop → run_simple 委派 SubAgent。
fn is_placeholder_direct_answer(s: &str) -> bool {
    let t = s.trim();
    t.is_empty()
        || t.eq_ignore_ascii_case("null")
        || t.eq_ignore_ascii_case("none")
        || t.eq_ignore_ascii_case("nil")
}

/// 当 Yolo 没有给出 user_suggestion 时,根据累计 usage 给出 actionable 兜底建议。
///
/// 关联报告: 2026-09-09_05 E-003。当前仅按 output token(代表 LLM 实际产出)
/// 启发式区分:
/// - output_token = 0 → LLM 完全没产出,提示「任务描述不够具体」;
/// - output_token > 0 → LLM 产出了文本但未触发成功判定,提示「可能需要拆分 / 改路径」。
fn fallback_suggestion(total_usage: &Usage) -> String {
    if total_usage.output_tokens == 0 {
        // 完全无产出:任务描述可能不明确,或被早终止
        "未产出任何答复:请补充任务信息(目标 / 验收标准 / 输入数据)或调整目标粒度,\
         让 LLM 能给出明确的输出"
            .to_string()
    } else if total_usage.output_tokens < 50 {
        // 产出极少:可能被无文本收敛短路,或 LLM 反复试探
        "答复信息密度极低:可能因反复工具调用未收敛。请把任务描述得更具体(目标 / 输入 / \
         期望产出),或确认工具调用所需的资源(文件路径 / 环境)是否可达"
            .to_string()
    } else {
        // 已有较多产出但仍失败:通常是质量判定 / 路径错误类
        // F11(2026-09-10 第 25 轮):此前误把 output_tokens 当「迭代次数」展示
        // ("已迭代 398 次"),数值来自 token 统计,严重误导。改为如实描述。
        format!(
            "任务累计产出 {} output tokens 仍未通过质量判定:请确认任务目标是否合理、\
             工具调用结果是否正确,或拆分成更小的子任务",
            total_usage.output_tokens
        )
    }
}

// 解决未使用警告:导入但仅在 cfg(test) 用
#[allow(unused_imports)]
use crate::agent::session_context::SessionSummary as _SessionSummary;
#[allow(unused_imports)]
use crate::agent::subagent::SubFlowOutcome as _SubFlowOutcome;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Db, Paths};
    use tempfile::tempdir;

    fn fresh_orchestrator() -> (MultiAgentOrchestrator, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Arc::new(Db::open(&paths).unwrap());
        // 用 NoopLlm 即可
        struct NoopLlm;
        #[async_trait::async_trait]
        impl crate::llm::LlmClient for NoopLlm {
            async fn complete(
                &self,
                _system: &str,
                _messages: &[crate::llm::ChatMessage],
                _tools: &[crate::llm::ToolDef],
                _meta: &crate::llm::RequestMeta,
            ) -> Result<crate::llm::Completion> {
                Ok(crate::llm::Completion {
                    text: "noop".into(),
                    tool_calls: vec![],
                    usage: Usage::default(),
                    stop_reason: None,
                })
            }
            fn protocol(&self) -> crate::config::Protocol {
                crate::config::Protocol::Anthropic
            }
        }
        let plans_dir = dir.path().join("plans");
        let orch = MultiAgentOrchestrator::new(Arc::new(NoopLlm), db, plans_dir);
        (orch, dir)
    }

    #[test]
    fn build_subflow_input_with_no_deps() {
        let wf = WorkFlowSpec {
            id: "wf-1".into(),
            name: "读取".into(),
            steps: vec!["读 a".into(), "读 b".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec!["OK".into()],
            delegate_to: AgentRole::SubAgent,
        };
        let input = build_subflow_input(&wf, &std::collections::HashMap::new());
        assert_eq!(input.id, "wf-1.step");
        assert!(input.description.contains("读 a"));
        assert_eq!(input.expected_output, "OK");
    }

    #[test]
    fn build_subflow_input_with_deps() {
        let mut deps = std::collections::HashMap::new();
        deps.insert("wf-1".into(), "已读取 a.rs".into());
        let wf = WorkFlowSpec {
            id: "wf-2".into(),
            name: "修改".into(),
            steps: vec!["改 a.rs".into()],
            branches: vec![],
            loops: vec![],
            depends_on: vec!["wf-1".into()],
            acceptance: vec!["修改完成".into()],
            delegate_to: AgentRole::SubAgent,
        };
        let input = build_subflow_input(&wf, &deps);
        assert_eq!(input.depends_on_outputs.len(), 1);
        assert!(input.depends_on_outputs[0].contains("已读取 a.rs"));
    }

    #[test]
    fn orchestrator_constructs_with_all_components() {
        let (orch, _d) = fresh_orchestrator();
        // 各 runner 都已构造
        let _ = orch.yolo();
        let _ = orch.quality();
        let _ = orch.session_context();
        let _ = orch.sub_agent();
        let _ = orch.plan();
        let _ = orch.main_work();
    }

    // ========== actionable 失败文案(第 05 轮 E-003,方案 tmpPlan/2026-09-09_05) ==========

    #[test]
    fn fallback_suggestion_distinguishes_output_levels() {
        // output_token = 0: 任务描述不够具体
        let zero = fallback_suggestion(&Usage::default());
        assert!(
            zero.contains("未产出任何答复"),
            "output=0 应提示「未产出任何答复」,实际: {zero}"
        );

        // output_token 极小(< 50): 可能反复工具调用未收敛
        let tiny = fallback_suggestion(&Usage {
            output_tokens: 10,
            ..Usage::default()
        });
        assert!(
            tiny.contains("答复信息密度极低"),
            "output=10 应提示「答复信息密度极低」,实际: {tiny}"
        );

        // output_token 较大: 提示拆分 / 调整目标
        // F11(2026-09-10 第 25 轮):措辞不再把 output_tokens 谎报为「迭代次数」
        let ample = fallback_suggestion(&Usage {
            output_tokens: 200,
            ..Usage::default()
        });
        assert!(
            ample.contains("output tokens") && ample.contains("拆分"),
            "output=200 应如实描述产出并提示拆分,实际: {ample}"
        );
    }

    // ========== 占位字符串归一化(2026-09-10 第 27 轮 F12 / BUG-2026-09-10-TUI-NULL) ==========

    #[test]
    fn placeholder_direct_answer_normalizes() {
        // 字面量 "null" / "None" / "NULL" / "Nil" / 空字符串 → 视为未填(继续走委派)
        for s in ["", "  ", "null", "NULL", "Null", "None", "none", "NIL", "nil"] {
            assert!(
                is_placeholder_direct_answer(s),
                "{s:?} 应被识别为占位字符串,实际未识别"
            );
        }
    }

    #[test]
    fn placeholder_direct_answer_keeps_real_answers() {
        // 真正含答案的字符串不应被误判为占位
        for s in [
            "答:巴黎",
            "1+1=2",
            "答案是42",
            "nullable", // 含 "null" 子串但不是占位
            "nonempty answer",
            "nullabc",
        ] {
            assert!(
                !is_placeholder_direct_answer(s),
                "{s:?} 不应被识别为占位,实际被误判"
            );
        }
    }
}
