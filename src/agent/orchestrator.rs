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
            suggestion: if cancelled { "任务已取消".into() } else { "重试".into() },
            cancelled,
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
    pub fn new(
        llm: Arc<dyn crate::llm::LlmClient>,
        db: Arc<Db>,
        plans_dir: PathBuf,
    ) -> Self {
        Self::with_config(
            llm,
            db,
            plans_dir,
            OrchestratorConfig::default(),
        )
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
        let _gate_guard = self.cancel_gate.guard(cancel.clone());
        self.handle_inner(session, cancel).await
    }

    async fn handle_inner(
        &self,
        session: &mut Session,
        cancel: &CancelToken,
    ) -> Result<OrchestrationOutcome> {
        // 0) 项目上下文首次注入(幂等)
        if let Some(work_dir) = project_context::current_work_dir() {
            project_context::inject_once(session, work_dir);
        }

        // 0.1) 历史 Session 摘要注入(幂等)
        let summaries = self.db.latest_summaries(session.id(), self.cfg.history_limit).unwrap_or_default();
        inject_history_with_entries(session, &summaries);

        // 0.2) Context 自动压缩(达到当前 Provider context_max_size 的 80% 阈值时触发)
        if let Ok(Some(active)) = self.db.get_active() {
            match self.compact.maybe_compact(session, active.context_max_size).await {
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
        let mut classification = self.run_yolo_classification(session).await?;
        Self::check_cancelled(cancel)?;
        self.dbg_classify(&classification);
        let mut total_usage = Usage::default();

        // 1.1) 记录 Yolo 输入事件
        let _ = self.db.insert_session_memory(&crate::config::SessionMemoryEntry {
            session_id: session.id().to_string(),
            role: AgentRole::Yolo,
            event_type: EventType::Input,
            content: format!("goal: {}", classification.goal_summary),
            usage_input: 0,
            usage_output: 0,
        });

        let mut retry_count = 0;
        loop {
            // 重试轮入口:取消短路(取消不是失败,不消耗重试预算)
            Self::check_cancelled(cancel)?;
            retry_count += 1;
            if retry_count > self.cfg.max_retry_per_level {
                // 超过最大重试,输出失败 / 建议
                let suggestion = if classification.user_suggestion_if_fail.is_empty() {
                    "任务执行超过最大重试次数,请补充任务信息或调整目标".to_string()
                } else {
                    classification.user_suggestion_if_fail.clone()
                };
                self.record_failure_event(session.id(), &classification, &suggestion);
                self.dbg_task_end(&format!("failed: {suggestion}"), total_usage);
                return Ok(OrchestrationOutcome::Failed {
                    classification,
                    suggestion,
                    usage: total_usage,
                });
            }

            // 2) 调度执行(执行层取消:token 贯穿 SubAgent / 并行层)
            let exec_result = match classification.task_level {
                TaskLevel::Simple => self.run_simple(&classification, session, cancel).await,
                TaskLevel::Medium => self.run_medium(&classification, session, cancel).await,
                TaskLevel::Hard => self.run_hard(&classification, session, cancel).await,
            };

            match exec_result {
                Ok(mut task_result) => {
                    // 3) SessionContext 收口(收口前再查一次:取消后不再发摘要请求)
                    Self::check_cancelled(cancel)?;
                    let summary = self
                        .session_context
                        .summarize(
                            &task_result.goal,
                            "(用户原始输入已记录)",
                            task_result.plan_doc.as_deref(),
                            &task_result
                                .workflows
                                .iter()
                                .map(|w| (w.id.clone(), w.name.clone(), w.quality_report.verdict == Verdict::Pass))
                                .collect::<Vec<_>>(),
                            &task_result.total_usage,
                            session.id(),
                            task_result.classification.yolo_degraded,
                        )
                        .await?;
                    task_result.summary = summary.text.clone();
                    total_usage.input_tokens =
                        total_usage.input_tokens.saturating_add(summary.usage.input_tokens);
                    total_usage.output_tokens =
                        total_usage.output_tokens.saturating_add(summary.usage.output_tokens);
                    task_result.total_usage = total_usage;
                    self.dbg_task_end("executed", task_result.total_usage);
                    return Ok(OrchestrationOutcome::Executed { result: task_result });
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
                        // 升级到上一层(由 Yolo 重新评估)
                        match self
                            .run_yolo_with_failure(&classification, &failure, session)
                            .await
                        {
                            Ok(new_c) => {
                                classification = new_c;
                                self.dbg_classify(&classification);
                                continue;
                            }
                            Err(e) => {
                                return Err(e);
                            }
                        }
                    }
                    // retryable=true 留在当前档位继续重跑
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
        let outcome = self
            .sub_agent
            .run_unit_with_cancel(&input, session.id(), cancel)
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::SubAgent, "SubAgent 执行失败", &e))?;

        let qc = self
            .quality
            .check_subagent(&c.goal_summary, &input.expected_output, &outcome.text, session.id())
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e))?;
        self.dbg_qc(&qc);

        if qc.verdict == Verdict::Pass {
            Ok(TaskResult {
                goal: c.goal_summary.clone(),
                classification: c.clone(),
                plan_doc: None,
                workflows: vec![WorkflowResult {
                    id: "wf-1".into(),
                    name: "单步执行".into(),
                    subflow_outcome: outcome.text,
                    quality_report: qc,
                    usage: outcome.usage,
                }],
                summary: String::new(),
                total_usage: outcome.usage,
            })
        } else {
            Err(QualityFailure {
                source: AgentRole::SubAgent,
                reason: qc.issues.join("; "),
                retryable: qc.retryable,
                suggestion: qc.suggestion,
                cancelled: false,
            })
        }
    }

    // ========== 中等档 ==========

    async fn run_medium(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 1) Main-Work 拆 WorkFlow
        let plan = self
            .main_work
            .plan_workflows(&c.goal_summary, &c.decomposition_plan, session.id())
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::MainWork, "Main-Work 拆解失败", &e))?;

        // 2) Quality 校验 Main-Work 输出
        let wf_json = serde_json::to_string(&plan).unwrap_or_default();
        let qc_main = self
            .quality
            .check_main(&c.goal_summary, &wf_json, session.id())
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e))?;
        self.dbg_qc(&qc_main);

        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
                cancelled: false,
            });
        }

        // 3) 拓扑排序并执行
        self.execute_workflows(c, &plan, session, cancel).await
    }

    // ========== 高等档 ==========

    async fn run_hard(
        &self,
        c: &TaskClassification,
        session: &Session,
        cancel: &CancelToken,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 1) Plan 生成
        let plan_output = self
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

        // 2) Quality 校验 Plan
        let qc_plan = self
            .quality
            .check_plan(&plan_output.markdown, session.id())
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e))?;
        self.dbg_qc(&qc_plan);
        if qc_plan.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::Plan,
                reason: qc_plan.issues.join("; "),
                retryable: qc_plan.retryable,
                suggestion: qc_plan.suggestion,
                cancelled: false,
            });
        }

        // 3) Main-Work 解析 Plan → WorkFlow
        let plan = self
            .main_work
            .parse_plan(&plan_output.path)
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::MainWork, "解析 Plan 失败", &e))?;

        let qc_main = self
            .quality
            .check_main(&c.goal_summary, &serde_json::to_string(&plan).unwrap_or_default(), session.id())
            .await
            .map_err(|e| QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e))?;
        self.dbg_qc(&qc_main);
        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
                cancelled: false,
            });
        }

        // 4) 执行 WorkFlow
        let mut task_result = self.execute_workflows(c, &plan, session, cancel).await?;
        task_result.plan_doc = Some(plan_output.path);
        Ok(task_result)
    }

    // ========== 通用:执行 WorkFlow 列表 ==========

    async fn execute_workflows(
        &self,
        c: &TaskClassification,
        plan: &WorkFlowPlan,
        session: &Session,
        cancel: &CancelToken,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 依赖分层:同层 WorkFlow 互相无依赖,自动并行;跨层严格串行(自动感知 depends_on)
        let layers = main_work::topo_layers(&plan.workflows).map_err(|e| QualityFailure {
            source: AgentRole::MainWork,
            reason: format!("拓扑分层失败: {e}"),
            retryable: false,
            suggestion: "Plan 中存在循环或未知依赖".into(),
            cancelled: false,
        })?;

        let mut results = Vec::new();
        let mut dep_outputs: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut total_usage = Usage::default();
        let total_layers = layers.len();

        for (layer_idx, layer) in layers.into_iter().enumerate() {
            // 层边界:取消短路(下一层不再启动)
            if let Err(e) = Self::check_cancelled(cancel) {
                return Err(QualityFailure::from_agent_error(AgentRole::MainWork, "WorkFlow 层调度", &e));
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
                )
                .await;
                vec![(wf, outcome)]
            } else {
                let semaphore = Arc::new(tokio::sync::Semaphore::new(
                    self.cfg.max_parallel_workflows,
                ));
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
                total_usage = add_usage(total_usage, ok.usage);
                dep_outputs.insert(wf.id.clone(), ok.outcome_text.clone());
                results.push(WorkflowResult {
                    id: wf.id.clone(),
                    name: wf.name.clone(),
                    subflow_outcome: ok.outcome_text,
                    quality_report: ok.qc,
                    usage: ok.usage,
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
    ) -> Result<TaskClassification> {
        let (mut c, _text, usage) = self.yolo.classify(session.context()).await?;
        let _ = usage;
        // 修正:若 agent_role 缺省,按 task_level 推断
        if c.agent_role.is_none() {
            c.agent_role = Some(match c.task_level {
                TaskLevel::Simple => AgentRole::SubAgent,
                TaskLevel::Medium => AgentRole::MainWork,
                TaskLevel::Hard => AgentRole::Plan,
            });
        }
        Ok(c)
    }

    async fn run_yolo_with_failure(
        &self,
        prev: &TaskClassification,
        failure: &QualityFailure,
        session: &mut Session,
    ) -> Result<TaskClassification> {
        // 构造失败摘要消息,让 Yolo 重新评估
        let failure_msg = format!(
            "[PREVIOUS_FAILURE]\n源: {}\n任务级别: {}\n原目标: {}\n失败原因: {}\n建议: {}\n请重新评估:可重试 → 修订 decomposition_plan 重发;不可重试 → 填 user_suggestion_if_fail 并给出 direct_answer 告知用户。",
            failure.source.as_str(),
            prev.task_level.as_str(),
            prev.goal_summary,
            failure.reason,
            failure.suggestion,
        );
        session
            .context_mut()
            .push(crate::llm::ChatMessage::user(failure_msg));

        self.run_yolo_classification(session).await
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

    fn record_failure_event(
        &self,
        session_id: &str,
        c: &TaskClassification,
        suggestion: &str,
    ) {
        let _ = self.db.insert_session_memory(&crate::config::SessionMemoryEntry {
            session_id: session_id.to_string(),
            role: AgentRole::Yolo,
            event_type: EventType::Failure,
            content: format!("目标: {}\n达到最大重试次数", c.goal_summary),
            usage_input: 0,
            usage_output: 0,
        });
        if !suggestion.is_empty() {
            let _ = self.db.insert_session_memory(&crate::config::SessionMemoryEntry {
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
    usage: Usage,
    qc: QualityReport,
}

/// 执行一个 WorkFlow 单元:SubAgent 执行 + Quality-Check(+ Debug 采集)。
///
/// 自由函数 + Arc 参数化,串行直通与 tokio::spawn 并行两种调用路径共用同一份逻辑;
/// `semaphore` 为并行路径的有界并发许可(串行路径传 None);
/// `cancel` 为任务级取消 token(传播进 SubAgent 的 Agent 循环,LLM/工具即时中断)。
async fn run_wf_unit(
    sub_agent: Arc<SubAgentRunner>,
    quality: Arc<QualityRunner>,
    debug: Option<Arc<DebugCollector>>,
    input: SubFlowInput,
    goal: String,
    session_id: String,
    semaphore: Option<Arc<tokio::sync::Semaphore>>,
    cancel: CancelToken,
) -> std::result::Result<WfUnitOk, QualityFailure> {
    // 有界并发:先抢许可(对齐 atomcode Semaphore(3) FIFO 惯例)
    let _permit = match &semaphore {
        Some(sem) => Some(sem.acquire().await.map_err(|e| QualityFailure {
            source: AgentRole::SubAgent,
            reason: format!("并行调度信号量已关闭: {e}"),
            retryable: true,
            suggestion: "重试".into(),
            cancelled: false,
        })?),
        None => None,
    };

    let wf_id = input.id.clone();
    let outcome = sub_agent
        .run_unit_with_cancel(&input, &session_id, &cancel)
        .await
        .map_err(|e| {
            QualityFailure::from_agent_error(AgentRole::SubAgent, &format!("SubAgent 执行失败(wf={wf_id})"), &e)
        })?;

    let qc = quality
        .check_subagent(&goal, &input.expected_output, &outcome.text, &session_id)
        .await
        .map_err(|e| QualityFailure::from_agent_error(AgentRole::QualityCheck, "Quality 调用失败", &e))?;
    if let Some(d) = &debug {
        d.record_quality(&qc);
    }

    if qc.verdict == Verdict::Fail {
        return Err(QualityFailure {
            source: AgentRole::SubAgent,
            reason: format!("wf={}: {}", wf_id, qc.issues.join("; ")),
            retryable: qc.retryable,
            suggestion: qc.suggestion,
            cancelled: false,
        });
    }

    Ok(WfUnitOk {
        outcome_text: outcome.text,
        usage: outcome.usage,
        qc,
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

// 解决未使用警告:导入但仅在 cfg(test) 用
#[allow(unused_imports)]
use crate::agent::subagent::SubFlowOutcome as _SubFlowOutcome;
#[allow(unused_imports)]
use crate::agent::session_context::SessionSummary as _SessionSummary;

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
        let orch = MultiAgentOrchestrator::new(
            Arc::new(NoopLlm),
            db,
            plans_dir,
        );
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
}