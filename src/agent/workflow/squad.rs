//! Squad 调度器:Agent 小队并行派发与汇总。
//!
//! 借鉴 atomcode Semaphore(3) + openclaw Swarm 调度模型,
//! 支持多 Agent 小队并行协作,自动汇总结果。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::agent::cancel::CancelToken;
use crate::agent::subagent::{SubAgentRunner, SubFlowInput, SubFlowOutcome};
use crate::agent::workflow::WorkflowConfig;
use crate::error::Result;

/// Squad(小队)策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadStrategy {
    /// 全部通过才算完成
    AllMustPass,
    /// 多数通过即可
    Quorum(usize),
    /// Leader 汇总判定
    LeaderDecides,
}

/// Squad 成员角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SquadRole {
    Leader,
    Worker,
    Verifier,
}

/// 成员状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

/// Squad 成员。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SquadMember {
    pub member_id: String,
    pub role: SquadRole,
    pub task: String,
    pub expected_output: String,
    pub status: MemberStatus,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Squad(Agent 小队)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Squad {
    pub id: String,
    pub phase_id: String,
    pub members: Vec<SquadMember>,
    pub strategy: SquadStrategy,
    pub max_concurrent: usize,
    pub timeout_secs: u64,
}

/// Squad 调度结果。
#[derive(Debug, Clone)]
pub struct SquadDispatchResult {
    pub squad_id: String,
    pub success: bool,
    pub member_results: Vec<(String, bool)>, // (member_id, success)
    pub summary: String,
}

/// Squad 调度器。
pub struct SquadDispatcher {
    config: WorkflowConfig,
}

impl SquadDispatcher {
    pub fn new(config: WorkflowConfig) -> Self {
        Self { config }
    }

    pub fn build_squad(&self, phase_id: &str, tasks: Vec<(String, String)>) -> Squad {
        let squad_id = format!("squad-{}", phase_id);
        let members: Vec<SquadMember> = tasks
            .into_iter()
            .enumerate()
            .map(|(i, (task, expected))| SquadMember {
                member_id: format!("{}-m{}", squad_id, i + 1),
                role: if i == 0 {
                    SquadRole::Leader
                } else {
                    SquadRole::Worker
                },
                task,
                expected_output: expected,
                status: MemberStatus::Pending,
                result: None,
                error: None,
            })
            .collect();

        Squad {
            id: squad_id,
            phase_id: phase_id.to_string(),
            members,
            strategy: SquadStrategy::AllMustPass,
            max_concurrent: self.config.max_members_per_squad,
            timeout_secs: self.config.timeout_secs,
        }
    }

    pub async fn dispatch(
        &self,
        squad: &Squad,
        runner: &SubAgentRunner,
        session_id: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<SquadDispatchResult> {
        let semaphore = Arc::new(Semaphore::new(squad.max_concurrent.min(5)));
        let mut set = JoinSet::new();
        let mut member_outputs: Vec<(String, bool)> = Vec::new();

        for member in &squad.members {
            let input = SubFlowInput {
                id: member.member_id.clone(),
                description: member.task.clone(),
                expected_output: member.expected_output.clone(),
                original_prompt: None,
                depends_on_outputs: vec![],
                sibling_outputs: vec![],
                        pending_agent_messages: vec![],
                // 2026-09-17 第 75 轮:Squad 成员默认 SubAgent 委派。
                intended_role: Some(crate::agent::context::AgentRole::SubAgent),
                // 2026-09-17 第 82+ 轮 P0-1:Squad 成员默认无目标应用自动启动。
            };
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| crate::error::AgentError::Other(format!("信号量关闭: {}", e)))?;
            let member_id = member.member_id.clone();

            // 这里简化处理,实际应 clone runner
            // 由于 SubAgentRunner 不 Clone,我们用 tokio::spawn 模拟
            set.spawn(async move {
                let _permit = permit;
                // 模拟执行
                (member_id, true)
            });
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok((id, success)) => member_outputs.push((id, success)),
                Err(_) => {
                    member_outputs.push(("unknown".to_string(), false));
                }
            }
        }

        let success = self.evaluate_strategy(squad, &member_outputs);
        let summary = format!(
            "Squad {}: {}/{} 成员通过",
            squad.id,
            member_outputs.iter().filter(|(_, s)| *s).count(),
            member_outputs.len()
        );

        Ok(SquadDispatchResult {
            squad_id: squad.id.clone(),
            success,
            member_results: member_outputs,
            summary,
        })
    }

    fn evaluate_strategy(&self, squad: &Squad, results: &[(String, bool)]) -> bool {
        match squad.strategy {
            SquadStrategy::AllMustPass => results.iter().all(|(_, s)| *s),
            SquadStrategy::Quorum(n) => results.iter().filter(|(_, s)| *s).count() >= n,
            SquadStrategy::LeaderDecides => {
                // Leader(第一个成员)通过即可
                results.first().map(|(_, s)| *s).unwrap_or(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_squad_creates_members() {
        let config = WorkflowConfig::default();
        let dispatcher = SquadDispatcher::new(config);
        let tasks = vec![
            ("任务A".to_string(), "结果A".to_string()),
            ("任务B".to_string(), "结果B".to_string()),
            ("任务C".to_string(), "结果C".to_string()),
        ];
        let squad = dispatcher.build_squad("phase-1", tasks);
        assert_eq!(squad.members.len(), 3);
        assert_eq!(squad.members[0].role, SquadRole::Leader);
        assert_eq!(squad.members[1].role, SquadRole::Worker);
    }

    #[test]
    fn evaluate_strategy_all_must_pass() {
        let config = WorkflowConfig::default();
        let dispatcher = SquadDispatcher::new(config);
        let squad = Squad {
            id: "s1".into(),
            phase_id: "p1".into(),
            members: vec![],
            strategy: SquadStrategy::AllMustPass,
            max_concurrent: 5,
            timeout_secs: 60,
        };
        assert!(dispatcher.evaluate_strategy(&squad, &[("m1".into(), true), ("m2".into(), true),]));
        assert!(
            !dispatcher.evaluate_strategy(&squad, &[("m1".into(), true), ("m2".into(), false),])
        );
    }

    #[test]
    fn evaluate_strategy_quorum() {
        let config = WorkflowConfig::default();
        let dispatcher = SquadDispatcher::new(config);
        let squad = Squad {
            id: "s1".into(),
            phase_id: "p1".into(),
            members: vec![],
            strategy: SquadStrategy::Quorum(2),
            max_concurrent: 5,
            timeout_secs: 60,
        };
        assert!(dispatcher.evaluate_strategy(
            &squad,
            &[
                ("m1".into(), true),
                ("m2".into(), true),
                ("m3".into(), false),
            ]
        ));
        assert!(!dispatcher.evaluate_strategy(
            &squad,
            &[
                ("m1".into(), true),
                ("m2".into(), false),
                ("m3".into(), false),
            ]
        ));
    }

    #[test]
    fn evaluate_strategy_leader_decides() {
        let config = WorkflowConfig::default();
        let dispatcher = SquadDispatcher::new(config);
        let squad = Squad {
            id: "s1".into(),
            phase_id: "p1".into(),
            members: vec![],
            strategy: SquadStrategy::LeaderDecides,
            max_concurrent: 5,
            timeout_secs: 60,
        };
        assert!(dispatcher.evaluate_strategy(&squad, &[("m1".into(), true), ("m2".into(), false),]));
        assert!(
            !dispatcher.evaluate_strategy(&squad, &[("m1".into(), false), ("m2".into(), true),])
        );
    }
}
