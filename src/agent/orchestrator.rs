//! MultiAgentOrchestrator:多 Agent 架构总编排器。
//!
//! 串联 Yolo / Plan / Main-Work / SubAgent-Work / Quality-Check / SessionContext 六大角色,
//! 按用户任务的难度档位走对应链路,并在失败时逐层回流到 Yolo 重新评估。
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md` §10。

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

use crate::agent::context::AgentRole;
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
use crate::error::Result;
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
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_retry_per_level: 3,
            history_limit: DEFAULT_HISTORY_LIMIT,
            subagent_max_iterations: 16,
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
}

/// 多 Agent 编排器
pub struct MultiAgentOrchestrator {
    yolo: YoloRunner,
    plan: PlanRunner,
    main_work: MainWorkRunner,
    sub_agent: SubAgentRunner,
    quality: QualityRunner,
    session_context: SessionContextRunner,
    db: Arc<Db>,
    cfg: OrchestratorConfig,
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
        let yolo = YoloRunner::new(llm.clone());
        let plan = PlanRunner::new(llm.clone(), db.clone(), plans_dir);
        let main_work = MainWorkRunner::new(llm.clone(), db.clone());
        let sub_agent = SubAgentRunner::new(llm.clone(), db.clone())
            .with_max_iterations(cfg.subagent_max_iterations);
        let quality = QualityRunner::new(llm.clone(), db.clone());
        let session_context = SessionContextRunner::new(llm, db.clone());
        Self {
            yolo,
            plan,
            main_work,
            sub_agent,
            quality,
            session_context,
            db,
            cfg,
        }
    }

    /// 处理一次用户输入
    pub async fn handle(&self, session: &mut Session) -> Result<OrchestrationOutcome> {
        // 0) 项目上下文首次注入(幂等)
        if let Some(work_dir) = project_context::current_work_dir() {
            project_context::inject_once(session, work_dir);
        }

        // 0.1) 历史 Session 摘要注入(幂等)
        let summaries = self.db.latest_summaries(session.id(), self.cfg.history_limit).unwrap_or_default();
        inject_history_with_entries(session, &summaries);

        // 1) Yolo 入口
        let mut classification = self.run_yolo_classification(session).await?;
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
            retry_count += 1;
            if retry_count > self.cfg.max_retry_per_level {
                // 超过最大重试,输出失败 / 建议
                let suggestion = if classification.user_suggestion_if_fail.is_empty() {
                    "任务执行超过最大重试次数,请补充任务信息或调整目标".to_string()
                } else {
                    classification.user_suggestion_if_fail.clone()
                };
                self.record_failure_event(session.id(), &classification, &suggestion);
                return Ok(OrchestrationOutcome::Failed {
                    classification,
                    suggestion,
                    usage: total_usage,
                });
            }

            // 2) 调度执行
            let exec_result = match classification.task_level {
                TaskLevel::Simple => self.run_simple(&classification, session).await,
                TaskLevel::Medium => self.run_medium(&classification, session).await,
                TaskLevel::Hard => self.run_hard(&classification, session).await,
            };

            match exec_result {
                Ok(mut task_result) => {
                    // 3) SessionContext 收口
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
                        )
                        .await?;
                    task_result.summary = summary.text.clone();
                    total_usage.input_tokens =
                        total_usage.input_tokens.saturating_add(summary.usage.input_tokens);
                    total_usage.output_tokens =
                        total_usage.output_tokens.saturating_add(summary.usage.output_tokens);
                    task_result.total_usage = total_usage;
                    return Ok(OrchestrationOutcome::Executed { result: task_result });
                }
                Err(failure) => {
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

    async fn run_simple(
        &self,
        c: &TaskClassification,
        session: &Session,
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
            .run_unit(&input, session.id())
            .await
            .map_err(|e| QualityFailure {
                source: AgentRole::SubAgent,
                reason: format!("SubAgent 执行失败: {e}"),
                retryable: true,
                suggestion: "请重试".into(),
            })?;

        let qc = self
            .quality
            .check_subagent(&c.goal_summary, &input.expected_output, &outcome.text, session.id())
            .await
            .map_err(|e| QualityFailure {
                source: AgentRole::QualityCheck,
                reason: format!("Quality 调用失败: {e}"),
                retryable: true,
                suggestion: "重试".into(),
            })?;

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
            })
        }
    }

    // ========== 中等档 ==========

    async fn run_medium(
        &self,
        c: &TaskClassification,
        session: &Session,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        // 1) Main-Work 拆 WorkFlow
        let plan = self
            .main_work
            .plan_workflows(&c.goal_summary, &c.decomposition_plan, session.id())
            .await
            .map_err(|e| QualityFailure {
                source: AgentRole::MainWork,
                reason: format!("Main-Work 拆解失败: {e}"),
                retryable: true,
                suggestion: "重试".into(),
            })?;

        // 2) Quality 校验 Main-Work 输出
        let wf_json = serde_json::to_string(&plan).unwrap_or_default();
        let qc_main = self
            .quality
            .check_main(&c.goal_summary, &wf_json, session.id())
            .await
            .map_err(|e| QualityFailure {
                source: AgentRole::QualityCheck,
                reason: format!("Quality 调用失败: {e}"),
                retryable: true,
                suggestion: "重试".into(),
            })?;

        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
            });
        }

        // 3) 拓扑排序并执行
        self.execute_workflows(c, &plan, session).await
    }

    // ========== 高等档 ==========

    async fn run_hard(
        &self,
        c: &TaskClassification,
        session: &Session,
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
            .map_err(|e| QualityFailure {
                source: AgentRole::Plan,
                reason: format!("Plan 生成失败: {e}"),
                retryable: true,
                suggestion: "重试".into(),
            })?;

        // 2) Quality 校验 Plan
        let qc_plan = self
            .quality
            .check_plan(&plan_output.markdown, session.id())
            .await
            .map_err(|e| QualityFailure {
                source: AgentRole::QualityCheck,
                reason: format!("Quality 调用失败: {e}"),
                retryable: true,
                suggestion: "重试".into(),
            })?;
        if qc_plan.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::Plan,
                reason: qc_plan.issues.join("; "),
                retryable: qc_plan.retryable,
                suggestion: qc_plan.suggestion,
            });
        }

        // 3) Main-Work 解析 Plan → WorkFlow
        let plan = self
            .main_work
            .parse_plan(&plan_output.path)
            .map_err(|e| QualityFailure {
                source: AgentRole::MainWork,
                reason: format!("解析 Plan 失败: {e}"),
                retryable: true,
                suggestion: "重试".into(),
            })?;

        let qc_main = self
            .quality
            .check_main(&c.goal_summary, &serde_json::to_string(&plan).unwrap_or_default(), session.id())
            .await
            .map_err(|e| QualityFailure {
                source: AgentRole::QualityCheck,
                reason: format!("Quality 调用失败: {e}"),
                retryable: true,
                suggestion: "重试".into(),
            })?;
        if qc_main.verdict == Verdict::Fail {
            return Err(QualityFailure {
                source: AgentRole::MainWork,
                reason: qc_main.issues.join("; "),
                retryable: qc_main.retryable,
                suggestion: qc_main.suggestion,
            });
        }

        // 4) 执行 WorkFlow
        let mut task_result = self.execute_workflows(c, &plan, session).await?;
        task_result.plan_doc = Some(plan_output.path);
        Ok(task_result)
    }

    // ========== 通用:执行 WorkFlow 列表 ==========

    async fn execute_workflows(
        &self,
        c: &TaskClassification,
        plan: &WorkFlowPlan,
        session: &Session,
    ) -> std::result::Result<TaskResult, QualityFailure> {
        let ordered = main_work::topo_sort(&plan.workflows).map_err(|e| QualityFailure {
            source: AgentRole::MainWork,
            reason: format!("拓扑排序失败: {e}"),
            retryable: false,
            suggestion: "Plan 中存在循环或未知依赖".into(),
        })?;

        let mut results = Vec::new();
        let mut dep_outputs: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut total_usage = Usage::default();

        for wf in ordered {
            let sub_input = build_subflow_input(&wf, &dep_outputs);
            let sub_outcome = self
                .sub_agent
                .run_unit(&sub_input, session.id())
                .await
                .map_err(|e| QualityFailure {
                    source: AgentRole::SubAgent,
                    reason: format!("SubAgent 执行失败(wf={}): {}", wf.id, e),
                    retryable: true,
                    suggestion: "重试".into(),
                })?;
            total_usage = add_usage(total_usage, sub_outcome.usage);

            let qc = self
                .quality
                .check_subagent(
                    &c.goal_summary,
                    &sub_input.expected_output,
                    &sub_outcome.text,
                    session.id(),
                )
                .await
                .map_err(|e| QualityFailure {
                    source: AgentRole::QualityCheck,
                    reason: format!("Quality 调用失败: {e}"),
                    retryable: true,
                    suggestion: "重试".into(),
                })?;

            if qc.verdict == Verdict::Fail {
                return Err(QualityFailure {
                    source: AgentRole::SubAgent,
                    reason: format!("wf={}: {}", wf.id, qc.issues.join("; ")),
                    retryable: qc.retryable,
                    suggestion: qc.suggestion,
                });
            }

            dep_outputs.insert(wf.id.clone(), sub_outcome.text.clone());
            results.push(WorkflowResult {
                id: wf.id.clone(),
                name: wf.name.clone(),
                subflow_outcome: sub_outcome.text,
                quality_report: qc,
                usage: sub_outcome.usage,
            });
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