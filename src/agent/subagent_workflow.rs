//! 动态 SubAgent 轻量工作流(D116)。
//!
//! 与中央 `MultiAgentOrchestrator` 互补:中央链路负责完整工程任务和 QC 收口;
//! 本模块负责“父 Agent 已决定自主委派”的小型 DAG。执行仍复用 D114 运行时,
//! 因此权限收窄、并发、取消、超时、用量和 D115 持久化都不旁路。

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::Serialize;

use crate::agent::custom_agents::ResolvedAgentType;
use crate::agent::dynamic_subagent::{
    clip_chars, SpawnError, SubAgentReport, SubAgentRequest, SubAgentRuntime, UsageSnapshot,
};
use crate::agent::self_awareness as sa;

/// 单个上游结论注入下游任务时的截断长度。
pub const UPSTREAM_REPORT_CHARS: usize = 2_000;
/// 一个步骤所有上游结论注入后的总截断长度。
pub const UPSTREAM_TOTAL_CHARS: usize = 6_000;

/// 一个 DAG 步骤。`agent_type` 在工具层已解析,执行期不再查询自定义定义文件。
#[derive(Debug, Clone)]
pub struct WorkflowStep {
    pub id: String,
    pub task: String,
    pub agent_type: ResolvedAgentType,
    pub name: Option<String>,
    pub system_prompt: Option<String>,
    pub tools: Vec<String>,
    pub expected_output: String,
    pub max_iterations: Option<usize>,
    pub depends_on: Vec<String>,
}

/// 工作流中的一个节点终态。
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStepReport {
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub name: String,
    pub agent_type: String,
    pub depends_on: Vec<String>,
    /// 被阻断时列出的未成功直接上游。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_dependencies: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_tools: Vec<String>,
    #[serde(default)]
    pub iterations: usize,
    #[serde(default)]
    pub tool_calls: usize,
    #[serde(default)]
    pub wallclock_ms: u64,
    #[serde(default)]
    pub usage: UsageSnapshot,
}

/// 整个工作流的聚合结果。
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowOutcome {
    pub status: String,
    pub total_steps: usize,
    pub executed_steps: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub blocked: usize,
    pub cancelled: usize,
    pub steps: Vec<WorkflowStepReport>,
    pub summary: String,
    pub usage: UsageSnapshot,
}

impl WorkflowOutcome {
    fn from_step_reports(steps: Vec<WorkflowStepReport>) -> Self {
        let count =
            |pred: &dyn Fn(&WorkflowStepReport) -> bool| steps.iter().filter(|s| pred(s)).count();
        let succeeded = count(&|s| s.status == "ok");
        let failed = count(&|s| matches!(s.status.as_str(), "failed" | "timeout"));
        let skipped = count(&|s| s.status == "skipped");
        let blocked = count(&|s| s.status == "blocked");
        let cancelled = count(&|s| s.status == "cancelled");
        let executed_steps = count(&|s| s.run_id.is_some());
        let status = if cancelled == steps.len() {
            "cancelled"
        } else if succeeded == steps.len() {
            "completed"
        } else if succeeded == 0 {
            "failed"
        } else {
            "partial"
        };
        let successful_reports = steps
            .iter()
            .filter(|s| s.status == "ok")
            .map(|s| SubAgentReport {
                run_id: s.run_id.clone().unwrap_or_default(),
                name: s.name.clone(),
                agent_type: s.agent_type.clone(),
                status: s.status.clone(),
                text: s.text.clone().unwrap_or_default(),
                error: s.error.clone(),
                tools: s.tools.clone(),
                dropped_tools: s.dropped_tools.clone(),
                iterations: s.iterations,
                tool_calls: s.tool_calls,
                wallclock_ms: s.wallclock_ms,
                usage: s.usage.clone(),
                origin: "workflow".into(),
                resumed_from: None,
            })
            .collect::<Vec<_>>();
        Self {
            status: status.to_string(),
            total_steps: steps.len(),
            executed_steps,
            succeeded,
            failed,
            skipped,
            blocked,
            cancelled,
            summary: merge_workflow_text(&successful_reports),
            usage: steps.iter().fold(UsageSnapshot::default(), |mut acc, s| {
                acc.input_tokens = acc.input_tokens.saturating_add(s.usage.input_tokens);
                acc.output_tokens = acc.output_tokens.saturating_add(s.usage.output_tokens);
                acc.cache_read_input_tokens = acc
                    .cache_read_input_tokens
                    .saturating_add(s.usage.cache_read_input_tokens);
                acc.cache_creation_input_tokens = acc
                    .cache_creation_input_tokens
                    .saturating_add(s.usage.cache_creation_input_tokens);
                acc
            }),
            steps,
        }
    }
}

/// 校验 DAG 并返回 Kahn 分层;层内下标升序,结果稳定。
pub fn plan_layers(steps: &[WorkflowStep]) -> Result<Vec<Vec<usize>>, SpawnError> {
    if steps.is_empty() {
        return Err(SpawnError::InvalidArgs("workflow.steps 不能为空".into()));
    }
    if steps.len() > sa::MAX_BATCH_TASKS {
        return Err(SpawnError::InvalidArgs(format!(
            "单张 workflow 最多 {} 个步骤(收到 {})",
            sa::MAX_BATCH_TASKS,
            steps.len()
        )));
    }
    let mut index = HashMap::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        if step.id.trim().is_empty() {
            return Err(SpawnError::InvalidArgs(
                "workflow.steps[].id 必须是非空标识符".into(),
            ));
        }
        if step.id.chars().count() > 64 {
            return Err(SpawnError::InvalidArgs(
                "workflow.steps[].id 不能超过 64 字符".into(),
            ));
        }
        if step.task.trim().is_empty() {
            return Err(SpawnError::InvalidArgs(format!(
                "workflow step `{}` 缺少非空 task(子 Agent 看不到父上下文)",
                step.id
            )));
        }
        if index.insert(step.id.clone(), i).is_some() {
            return Err(SpawnError::InvalidArgs(format!(
                "workflow.steps[].id 重复:`{}`",
                step.id
            )));
        }
    }
    let mut indegree = vec![0usize; steps.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); steps.len()];
    for (i, step) in steps.iter().enumerate() {
        let mut seen = std::collections::HashSet::new();
        for dep in &step.depends_on {
            let Some(&j) = index.get(dep) else {
                return Err(SpawnError::InvalidArgs(format!(
                    "workflow step `{}` 依赖了未声明的 `{}`",
                    step.id, dep
                )));
            };
            if j == i {
                return Err(SpawnError::InvalidArgs(format!(
                    "workflow step `{}` 不能依赖自己",
                    step.id
                )));
            }
            if seen.insert(j) {
                indegree[i] += 1;
                dependents[j].push(i);
            }
        }
    }

    let mut queue: VecDeque<usize> = (0..steps.len()).filter(|&i| indegree[i] == 0).collect();
    let mut ordered = Vec::with_capacity(steps.len());
    let mut layers = Vec::new();
    while !queue.is_empty() {
        let mut next = VecDeque::new();
        let mut layer = Vec::new();
        for i in queue.drain(..) {
            ordered.push(i);
            layer.push(i);
            for &dep in &dependents[i] {
                indegree[dep] -= 1;
                if indegree[dep] == 0 {
                    next.push_back(dep);
                }
            }
        }
        layers.push(layer);
        queue = next;
    }
    if ordered.len() != steps.len() {
        let cyclic: Vec<String> = indegree
            .iter()
            .enumerate()
            .filter(|(_, &d)| d > 0)
            .map(|(i, _)| steps[i].id.clone())
            .collect();
        return Err(SpawnError::InvalidArgs(format!(
            "workflow.steps 存在循环依赖:{}",
            cyclic.join(", ")
        )));
    }
    Ok(layers)
}

/// 校验并执行整张 DAG。启动前原子预扣全部步骤预算,避免并发后台作业把图拆散。
pub async fn run_workflow(
    rt: &std::sync::Arc<SubAgentRuntime>,
    steps: Vec<WorkflowStep>,
) -> Result<WorkflowOutcome, SpawnError> {
    let layers = plan_layers(&steps)?;
    if rt.is_cancelled() {
        return Err(SpawnError::Cancelled);
    }
    if !rt.can_spawn() {
        return Err(SpawnError::DepthExceeded {
            depth: rt_depth(rt),
            max: rt_cfg(rt).max_depth,
        });
    }
    if !rt.governor.try_charge(steps.len()) {
        return Err(SpawnError::BudgetExhausted {
            used: rt.governor.used(),
            max: rt_cfg(rt).max_total,
        });
    }

    let by_id: BTreeMap<String, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.clone(), i))
        .collect();
    let mut reports: Vec<Option<WorkflowStepReport>> = (0..steps.len()).map(|_| None).collect();

    for layer in layers {
        if rt.is_cancelled() {
            for &i in &layer {
                reports[i] = Some(WorkflowStepReport {
                    id: steps[i].id.clone(),
                    status: "cancelled".into(),
                    run_id: None,
                    name: step_name(&steps[i]),
                    agent_type: steps[i].agent_type.id().to_string(),
                    depends_on: steps[i].depends_on.clone(),
                    failed_dependencies: Vec::new(),
                    text: None,
                    error: Some("父任务在派发前已取消".into()),
                    tools: Vec::new(),
                    dropped_tools: Vec::new(),
                    iterations: 0,
                    tool_calls: 0,
                    wallclock_ms: 0,
                    usage: UsageSnapshot::default(),
                });
            }
            continue;
        }

        let mut reqs = Vec::new();
        let mut req_indexes = Vec::new();
        for &i in &layer {
            let mut failed_deps = Vec::new();
            let mut ready = true;
            let mut direct_bad = false;
            let mut indirect_bad = false;
            for dep in &steps[i].depends_on {
                let j = by_id[dep];
                let state = reports[j]
                    .as_ref()
                    .map(|r| r.status.as_str())
                    .unwrap_or("blocked");
                if state == "ok" {
                    continue;
                }
                ready = false;
                failed_deps.push(dep.clone());
                if matches!(state, "failed" | "timeout" | "cancelled") {
                    direct_bad = true;
                } else {
                    indirect_bad = true;
                }
            }
            if !ready {
                reports[i] = Some(WorkflowStepReport {
                    id: steps[i].id.clone(),
                    status: if direct_bad { "skipped" } else { "blocked" }.into(),
                    run_id: None,
                    name: step_name(&steps[i]),
                    agent_type: steps[i].agent_type.id().to_string(),
                    depends_on: steps[i].depends_on.clone(),
                    failed_dependencies: failed_deps,
                    text: None,
                    error: if direct_bad {
                        Some("直接上游未成功,本步骤不执行".into())
                    } else {
                        Some("上游链路已被阻断,本步骤不可达".into())
                    },
                    tools: Vec::new(),
                    dropped_tools: Vec::new(),
                    iterations: 0,
                    tool_calls: 0,
                    wallclock_ms: 0,
                    usage: UsageSnapshot::default(),
                });
                let _ = indirect_bad;
                continue;
            }
            reqs.push(make_request(&steps[i], &reports, &by_id));
            req_indexes.push(i);
        }

        if reqs.is_empty() {
            continue;
        }
        let wave = rt.batch_reserved(reqs, None).await?;
        for (report, &i) in wave.into_iter().zip(req_indexes.iter()) {
            reports[i] = Some(step_report(&steps[i], report));
        }
    }

    let steps_out: Vec<WorkflowStepReport> = reports.into_iter().flatten().collect();
    Ok(WorkflowOutcome::from_step_reports(steps_out))
}

fn rt_depth(rt: &SubAgentRuntime) -> usize {
    rt.depth()
}

fn rt_cfg(rt: &SubAgentRuntime) -> sa::SelfAwarenessConfig {
    rt.cfg().clone()
}

fn step_name(step: &WorkflowStep) -> String {
    step.name
        .clone()
        .unwrap_or_else(|| format!("workflow-{}", step.id))
}

fn make_request(
    step: &WorkflowStep,
    reports: &[Option<WorkflowStepReport>],
    by_id: &BTreeMap<String, usize>,
) -> SubAgentRequest {
    let upstream: Vec<String> = step
        .depends_on
        .iter()
        .filter_map(|id| {
            let j = by_id.get(id)?;
            let Some(report) = reports.get(*j) else {
                return None;
            };
            let report = report.as_ref()?;
            if report.status != "ok" {
                return None;
            }
            let text = report.text.clone().unwrap_or_else(|| "(无文本输出)".into());
            Some(format!(
                "### 上游 {}({})\n{}",
                report.id,
                report.agent_type,
                clip_chars(&text, UPSTREAM_REPORT_CHARS)
            ))
        })
        .collect();
    let mut task = step.task.trim().to_string();
    if !upstream.is_empty() {
        task.push_str("\n\n## 可直接使用的上游结论(不要重复上游工作)\n");
        task.push_str(&clip_chars(&upstream.join("\n\n"), UPSTREAM_TOTAL_CHARS));
    }
    SubAgentRequest {
        task,
        agent_type: step.agent_type.clone(),
        name: Some(step_name(step)),
        system_prompt: step.system_prompt.clone(),
        tools: step.tools.clone(),
        expected_output: step.expected_output.clone(),
        max_iterations: step.max_iterations,
        prior: None,
    }
}

fn step_report(step: &WorkflowStep, report: SubAgentReport) -> WorkflowStepReport {
    WorkflowStepReport {
        id: step.id.clone(),
        status: report.status,
        run_id: Some(report.run_id),
        name: report.name,
        agent_type: report.agent_type,
        depends_on: step.depends_on.clone(),
        failed_dependencies: Vec::new(),
        text: if report.text.trim().is_empty() {
            None
        } else {
            Some(report.text)
        },
        error: report.error,
        tools: report.tools,
        dropped_tools: report.dropped_tools,
        iterations: report.iterations,
        tool_calls: report.tool_calls,
        wallclock_ms: report.wallclock_ms,
        usage: report.usage,
    }
}

/// workflow 成功产物合并文本;父模型可先读它,再按需读取 JSON 中的完整步骤。
pub fn merge_workflow_text(reports: &[SubAgentReport]) -> String {
    if reports.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for report in reports {
        let body = if report.text.trim().is_empty() {
            report
                .error
                .clone()
                .unwrap_or_else(|| "(无输出)".to_string())
        } else {
            clip_chars(&report.text, sa::BATCH_CHILD_CHARS)
        };
        out.push_str(&format!(
            "\n### {} | {} ({})\n{}\n",
            report.name, report.agent_type, report.run_id, body
        ));
    }
    clip_chars(out.trim_start(), sa::BATCH_MERGED_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::dynamic_subagent::test_runtime;
    use crate::agent::self_awareness::{SelfAwarenessConfig, SpawnPolicy, SubAgentType};
    use crate::error::AgentError;
    use crate::llm::{ChatMessage, Completion, LlmClient, RequestMeta, ToolDef};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone, Copy, PartialEq)]
    enum Script {
        Echo,
        FailFirst,
    }

    struct ScriptLlm {
        calls: AtomicUsize,
        mode: Script,
        users: std::sync::Mutex<Vec<String>>,
    }

    impl ScriptLlm {
        fn new(mode: Script) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                mode,
                users: std::sync::Mutex::new(Vec::new()),
            })
        }

        fn users(&self) -> Vec<String> {
            self.users.lock().expect("users poisoned").clone()
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptLlm {
        async fn complete(
            &self,
            _system: &str,
            messages: &[ChatMessage],
            _tools: &[ToolDef],
            _meta: &RequestMeta,
        ) -> crate::error::Result<Completion> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(last) = messages.iter().rev().find(|m| m.role == crate::llm::Role::User) {
                self.users.lock().expect("users poisoned").push(
                    last.content
                        .iter()
                        .filter_map(|block| match block {
                            crate::llm::ContentBlock::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            if self.mode == Script::FailFirst && n == 0 {
                return Err(AgentError::Llm("故意失败".into()));
            }
            Ok(Completion {
                text: format!("结论-{n}"),
                tool_calls: vec![],
                usage: crate::llm::Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                    ..crate::llm::Usage::default()
                },
                stop_reason: None,
            })
        }

        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    fn step(id: &str, task: &str, deps: &[&str]) -> WorkflowStep {
        WorkflowStep {
            id: id.into(),
            task: task.into(),
            agent_type: ResolvedAgentType::Builtin(SubAgentType::Explore),
            name: None,
            system_prompt: None,
            tools: vec![],
            expected_output: "一行结论".into(),
            max_iterations: None,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn runtime(llm: Arc<dyn LlmClient>, session: &str) -> Arc<SubAgentRuntime> {
        crate::agent::dynamic_subagent::reset_governors_for_test();
        test_runtime(
            llm,
            session,
            SpawnPolicy::FullChildren,
            0,
            SelfAwarenessConfig::default(),
        )
    }

    #[test]
    fn plan_layers_orders_independent_then_dependent_steps() {
        let steps = vec![
            step("final", "汇总", &["a", "b"]),
            step("b", "调研 B", &[]),
            step("a", "调研 A", &[]),
            step("review", "审查", &["final"]),
        ];
        let layers = plan_layers(&steps).unwrap();
        let ids: Vec<Vec<&str>> = layers
            .iter()
            .map(|l| l.iter().map(|&i| steps[i].id.as_str()).collect())
            .collect();
        assert_eq!(ids, vec![vec!["b", "a"], vec!["final"], vec!["review"]]);
    }

    #[test]
    fn plan_rejects_missing_cycle_and_over_limit() {
        let err = plan_layers(&[step("a", "A", &["missing"])]).unwrap_err();
        assert_eq!(err.code(), 1001);
        let err = plan_layers(&[
            step("a", "A", &["b"]),
            step("b", "B", &["a"]),
        ])
        .unwrap_err();
        assert_eq!(err.code(), 1001);
        let too_many: Vec<_> = (0..9).map(|i| step(&format!("s{i}"), "task", &[])).collect();
        assert_eq!(plan_layers(&too_many).unwrap_err().code(), 1001);
    }

    #[tokio::test]
    async fn workflow_executes_layers_and_injects_upstream_reports() {
        let llm = ScriptLlm::new(Script::Echo);
        let rt = runtime(llm.clone(), "wf-ok");
        let steps = vec![
            step("a", "调研 A", &[]),
            step("b", "调研 B", &[]),
            step("final", "汇总 A/B", &["a", "b"]),
        ];
        let outcome = crate::agent::dynamic_subagent::scope(
            rt.clone(),
            run_workflow(&rt, steps),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, "completed");
        assert_eq!(outcome.succeeded, 3);
        assert_eq!(outcome.executed_steps, 3);
        assert_eq!(outcome.usage.input_tokens, 21);
        assert_eq!(rt.governor.used(), 3, "整图启动前应原子预扣");
        let final_task = llm
            .users()
            .into_iter()
            .find(|t| t.contains("汇总 A/B"))
            .expect("下游应被调用");
        assert!(final_task.contains("上游结论"));
        assert!(final_task.contains("结论-"));
        assert!(outcome.summary.contains("workflow-final"));
    }

    #[tokio::test]
    async fn workflow_failure_skips_direct_and_blocks_indirect_children() {
        let llm = ScriptLlm::new(Script::FailFirst);
        let rt = runtime(llm, "wf-fail");
        let steps = vec![
            step("root", "失败根", &[]),
            step("direct", "直接下游", &["root"]),
            step("indirect", "间接下游", &["direct"]),
            step("independent", "独立成功", &[]),
        ];
        let outcome = crate::agent::dynamic_subagent::scope(
            rt.clone(),
            run_workflow(&rt, steps),
        )
        .await
        .unwrap();
        assert_eq!(outcome.status, "partial");
        let state = |id: &str| {
            outcome
                .steps
                .iter()
                .find(|s| s.id == id)
                .unwrap()
                .status
                .as_str()
        };
        assert_eq!(state("root"), "failed");
        assert_eq!(state("direct"), "skipped");
        assert_eq!(state("indirect"), "blocked");
        assert_eq!(state("independent"), "ok");
        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.blocked, 1);
    }

    #[tokio::test]
    async fn workflow_budget_is_all_or_nothing_before_any_step() {
        crate::agent::dynamic_subagent::reset_governors_for_test();
        let cfg = SelfAwarenessConfig {
            max_total: 2,
            ..SelfAwarenessConfig::default()
        };
        let rt = test_runtime(
            ScriptLlm::new(Script::Echo),
            "wf-budget",
            SpawnPolicy::FullChildren,
            0,
            cfg,
        );
        let steps = vec![step("a", "A", &[]), step("b", "B", &[]), step("c", "C", &[])];
        let err = crate::agent::dynamic_subagent::scope(
            rt.clone(),
            run_workflow(&rt, steps),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), 2001);
        assert_eq!(rt.governor.used(), 0, "预算不足时不得部分启动");
    }
}
