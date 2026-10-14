//! Plan 再规划 / 更新规划调度(Plan 再规划/更新规划,设计见
//! `docs/PlanAgent再规划与更新规划/01-设计与解决方案.md` §3.4)。
//!
//! `try_replan`:执行层失败经 [`crate::agent::replan::detect_replan_signal`] 判定为
//! 规划缺口后,把失败现场回灌规划层修订方案——
//! - hard 档 → `PlanRunner::replan_update`(原方案 + 失败现场 + 已完成清单 → 新方案文档);
//! - medium 档 → `MainWorkRunner::plan_workflows_with_delegate(replan_context)`
//!   (更新编排,产物须过确定性校验才进入执行,不过即回落)。
//! 成功后组装 `ReplanPending` 跨轮暂存,下一轮 run_hard / run_medium 消费。
//!
//! 机械路由,不做价值判断:是否再规划由编排器按信号与预算调度,
//! 再规划成什么样由 Plan / Main-Work 模型决定。

use super::*;

/// 再规划 prompt 里原方案注入的截断上限(字符)——保留结构与前几段即可。
const PREV_PLAN_CLIP: usize = 3000;
/// 再规划摘要注入 retry_hint 的截断上限(字符)。
const REPLAN_SUMMARY_CLIP: usize = 600;

impl MultiAgentOrchestrator {
    /// 构建再规划触发上下文(触发类别 + 失败现场 + QC 结论 + 失败信号)。
    pub(super) fn build_replan_context(
        c: &TaskClassification,
        failure: &QualityFailure,
        signal: &crate::agent::replan::ReplanSignal,
    ) -> String {
        let failure_signals = failure
            .trace
            .as_ref()
            .map(|t| t.failure_signals.join(","))
            .unwrap_or_default();
        format!(
            "缺口类别: {}({})\n\
             任务目标: {}\n\
             失败来源: {}\n\
             失败原因: {}\n\
             QC 建议: {}\n\
             执行层失败信号: {}\n\
             命中证据: {}",
            signal.trigger.label(),
            signal.trigger.as_str(),
            c.goal_summary,
            failure.source.as_str(),
            failure.reason,
            failure.suggestion,
            if failure_signals.is_empty() {
                "(无)".to_string()
            } else {
                failure_signals
            },
            signal.evidence,
        )
    }

    /// 尝试一次再规划。成功返回 `(ReplanPending, 本次 LLM 用量)`;
    /// 任何一步失败返回 Err(调用方静默回落常规重试路径)。
    ///
    /// `last_plan_path`:hard 档最近一次生成的 Plan 文档路径(再规划的修订基础)。
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn try_replan(
        &self,
        c: &TaskClassification,
        session: &Session,
        progress: &Option<ProgressTx>,
        failure: &QualityFailure,
        completed_digest: &str,
        last_plan_path: Option<&std::path::Path>,
    ) -> Result<(ReplanPending, Usage)> {
        let user_prompt = Self::task_user_prompt(session).unwrap_or_default();
        match c.task_level {
            TaskLevel::Hard => {
                let prev_plan_path = last_plan_path.ok_or_else(|| {
                    AgentError::PlanGen("再规划缺少原方案路径".to_string())
                })?;
                let prev_markdown_full = std::fs::read_to_string(prev_plan_path)
                    .map_err(|e| AgentError::PlanGen(format!("读取原方案失败: {e}")))?;
                let prev_markdown: String =
                    prev_markdown_full.chars().take(PREV_PLAN_CLIP).collect();
                let signal = signal_from_failure(failure)?;
                let ctx = Self::build_replan_context(c, failure, &signal);
                emit_progress(
                    progress,
                    format!(
                        "Plan 再规划中(修订基础 {},缺口:{})…",
                        prev_plan_path.display(),
                        ctx_trigger_label(&ctx)
                    ),
                );
                let (output, usage) = self
                    .plan
                    .replan_update(
                        &c.goal_summary,
                        &c.purpose,
                        &c.intent,
                        &c.decomposition_plan,
                        session.id(),
                        &user_prompt,
                        &prev_markdown,
                        &ctx,
                        completed_digest,
                    )
                    .await?;
                let summary = format!(
                    "【方案已再规划】新方案 {}:针对「{}」缺口修订了受阻单元;按新方案步骤执行,已完成单元勿重做。",
                    output.path.display(),
                    ctx_trigger_label(&ctx)
                );
                info!(
                    session = %session.id(),
                    replan_path = %output.path.display(),
                    "Plan 再规划完成(决策)"
                );
                Ok((
                    ReplanPending {
                        plan_path: Some(output.path),
                        plan: None,
                        summary,
                    },
                    usage,
                ))
            }
            TaskLevel::Medium => {
                let signal = signal_from_failure(failure)?;
                let ctx = Self::build_replan_context(c, failure, &signal);
                emit_progress(
                    progress,
                    format!(
                        "Main-Work 更新规划中(缺口:{})…",
                        ctx_trigger_label(&ctx)
                    ),
                );
                let (plan, usage) = self
                    .main_work
                    .plan_workflows_with_delegate(
                        &c.goal_summary,
                        &c.decomposition_plan,
                        session.id(),
                        "", // retry_hint 为空:本轮是再规划,失败反馈已在 replan_context 内
                        Self::original_user_prompt(session).as_deref(),
                        c.suggested_delegate.as_deref(),
                        completed_digest,
                        Some(&ctx),
                    )
                    .await?;
                // 再规划产物与正常拆解同标准:确定性自动修复 + 阻断校验,
                // 不过即回落常规重试(不带结构缺陷的计划绝不下发执行层)。
                let mut plan = plan;
                crate::agent::plan_validate::auto_repair_plan(&mut plan);
                let blocking = crate::agent::plan_validate::validate_plan_blocking(&plan);
                if !blocking.is_empty() {
                    return Err(AgentError::PlanGen(format!(
                        "再规划编排未过确定性校验: {}",
                        blocking.join("; ")
                    )));
                }
                let summary = format!(
                    "【编排已更新规划】针对「{}」缺口重排了 {} 个流程单元;按新编排执行,已完成单元勿重做。",
                    ctx_trigger_label(&ctx),
                    plan.workflows.len()
                );
                info!(
                    session = %session.id(),
                    workflows = plan.workflows.len(),
                    "Main-Work 更新规划完成(决策)"
                );
                Ok((
                    ReplanPending {
                        plan_path: None,
                        plan: Some(plan),
                        summary,
                    },
                    usage,
                ))
            }
            // simple 档无规划层,不再规划(单单元任务由既有局部重试/档位重试处理)。
            TaskLevel::Simple => Err(AgentError::PlanGen("simple 档无规划层".to_string())),
        }
    }
}

/// 从 QualityFailure 文本再次提取信号(供上下文构建;调用方已判定过一次,
/// 这里只是拿到结构化类别用于文案)。
fn signal_from_failure(
    failure: &QualityFailure,
) -> Result<crate::agent::replan::ReplanSignal> {
    let signals = failure
        .trace
        .as_ref()
        .map(|t| t.failure_signals.clone())
        .unwrap_or_default();
    crate::agent::replan::detect_replan_signal(&failure.reason, &failure.suggestion, &signals)
        .ok_or_else(|| AgentError::PlanGen("再规划信号已消失".to_string()))
}

/// 摘取上下文首行(「缺口类别: …」)作为进度行/摘要的短标签。
fn ctx_trigger_label(ctx: &str) -> String {
    ctx.lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(40)
        .collect()
}

/// 截断再规划摘要(注入 retry_hint 的口径控制,字符截断防中文 panic)。
pub(super) fn clip_replan_summary(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= REPLAN_SUMMARY_CLIP {
        return s.to_string();
    }
    format!(
        "{}…(截断)",
        chars[..REPLAN_SUMMARY_CLIP.saturating_sub(4)].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hard_classification() -> TaskClassification {
        TaskClassification {
            task_level: TaskLevel::Hard,
            goal_summary: "抓取目标站数据".into(),
            purpose: "采集".into(),
            intent: "网页采集".into(),
            agent_role: None,
            decomposition_plan: vec!["打开站点".into(), "登录".into(), "抓取".into()],
            direct_answer: None,
            user_suggestion_if_fail: String::new(),
            yolo_degraded: false,
            suggested_delegate: None,
            debug_eligible: false,
            target_status: None,
            clarification_question: None,
        }
    }

    fn login_wall_failure() -> QualityFailure {
        QualityFailure {
            source: AgentRole::SubAgent,
            reason: "wf-2: 页面弹出登录框,需要登录后才能继续".into(),
            retryable: true,
            suggestion: "补充登录前置步骤".into(),
            cancelled: false,
            trace: None,
            usage: Usage::default(),
            completed_digest: String::new(),
        }
    }

    #[test]
    fn 再规划上下文包含缺口类别与失败现场() {
        // 上下文构建是纯函数(signal + failure + 分类拼接)。
        let f = login_wall_failure();
        let sig = signal_from_failure(&f).expect("应识别登录墙信号");
        let ctx = MultiAgentOrchestrator::build_replan_context(&hard_classification(), &f, &sig);
        assert!(ctx.contains("登录墙"), "{ctx}");
        assert!(ctx.contains("wf-2"), "{ctx}");
        assert!(ctx.contains("补充登录前置步骤"), "{ctx}");
    }

    #[test]
    fn 摘要截断有界() {
        let long = "x".repeat(2000);
        let clipped = clip_replan_summary(&long);
        assert!(clipped.chars().count() <= REPLAN_SUMMARY_CLIP + 4);
    }

    #[test]
    fn 普通失败不再规划信号() {
        let f = QualityFailure {
            reason: "wf-1: 文件不存在".into(),
            ..login_wall_failure()
        };
        assert!(signal_from_failure(&f).is_err());
    }
}
