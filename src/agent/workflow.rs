//! WorkFlow Agent:第 10 角色 —— 超大型复杂任务自动化编排。
//!
//! 负责 Goal 状态机管理 / Agent Squad 调度 / Phase 执行 / 自适应循环 /
//! 模板库 / 质量门禁 / 进度报告。由 Orchestrator 在 hard 档或超大规模
//! 任务时激活,自动感知 → 自动加载 → 自动规划 → 自动执行 → 自动质量评价 →
//! 循环处理,直到任务目标达成。
//!
//! 设计见 `docs/WorkFlowAgent设计与实现/01-设计与解决方案.md`。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub mod adaptive_loop;
pub mod batch;
pub mod goal;
pub mod phase;
pub mod quality_gate;
pub mod squad;
pub mod template;

pub use adaptive_loop::{AdaptiveLoop, AdaptiveLoopResult, RepairStrategy};
pub use batch::{BatchChannel, BatchResult, BatchTask};
pub use goal::{Goal, GoalState, GoalStore, Priority};
pub use phase::{Phase, PhaseExecutionResult, PhaseExecutor, PhaseFailurePolicy};
pub use quality_gate::{QualityGate, QualityGateResult};
pub use squad::{
    Squad, SquadDispatchResult, SquadDispatcher, SquadMember, SquadRole, SquadStrategy,
};
pub use template::{TemplateCategory, TemplateLibrary, WorkflowTemplate};

use crate::agent::cancel::CancelToken;
use crate::agent::context::AgentRole;
use crate::agent::memory;
use crate::agent::subagent::{SubAgentRunner, SubFlowInput};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Usage};
use crate::session;

/// WorkFlow 执行配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    pub max_retries_per_goal: usize,
    pub max_adaptive_attempts: usize,
    pub max_unproductive: usize,
    pub max_parallel_squads: usize,
    pub max_members_per_squad: usize,
    pub batch_chunk_size: usize,
    pub max_parallel_batches: usize,
    pub timeout_secs: u64,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            max_retries_per_goal: 3,
            max_adaptive_attempts: 5,
            max_unproductive: 3,
            max_parallel_squads: 3,
            max_members_per_squad: 5,
            batch_chunk_size: 10,
            max_parallel_batches: 3,
            timeout_secs: 3600,
        }
    }
}

/// WorkFlow 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub goal_id: String,
    pub goal_state: GoalState,
    pub phases_completed: usize,
    pub total_phases: usize,
    pub squads_dispatched: usize,
    pub tasks_executed: usize,
    pub tasks_succeeded: usize,
    pub total_usage: Usage,
    pub summary: String,
    pub error: Option<String>,
}

/// WorkFlow Runner:第 10 角色执行器。
pub struct WorkFlowRunner {
    sub_agent: SubAgentRunner,
    db: Arc<Db>,
    config: WorkflowConfig,
}

impl WorkFlowRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let sub_agent = SubAgentRunner::new(llm, db.clone());
        Self {
            sub_agent,
            db,
            config: WorkflowConfig::default(),
        }
    }

    pub fn with_config(mut self, config: WorkflowConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn run_goal(
        &self,
        goal: &mut Goal,
        session_id: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<WorkflowResult> {
        let mut result = WorkflowResult {
            goal_id: goal.id.clone(),
            goal_state: GoalState::Pending,
            phases_completed: 0,
            total_phases: goal.subgoals.len(),
            squads_dispatched: 0,
            tasks_executed: 0,
            tasks_succeeded: 0,
            total_usage: Usage::default(),
            summary: String::new(),
            error: None,
        };

        self.load_memories(goal, session_id).await?;
        goal.start()?;
        result.goal_state = GoalState::Pursuing;

        let adaptive = AdaptiveLoop::new(self.config.clone());
        let loop_result = adaptive
            .run(
                goal,
                session_id,
                &self.sub_agent,
                &self.db,
                cancel,
                &mut result,
            )
            .await;

        match loop_result {
            Ok(_) => {
                result.goal_state = goal.state.clone();
                result.summary = format!("Goal '{}' 终态: {:?}", goal.title, goal.state);
            }
            Err(e) => {
                goal.fail(&e.to_string())?;
                result.goal_state = GoalState::Failed;
                result.error = Some(e.to_string());
                result.summary = format!("Goal '{}' 失败: {}", goal.title, e);
            }
        }

        self.persist_goal(goal, session_id).await?;
        Ok(result)
    }

    pub async fn run_batch_squads(
        &self,
        tasks: Vec<(String, String)>,
        phase_id: &str,
        session_id: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<Vec<(String, bool)>> {
        let dispatcher = SquadDispatcher::new(self.config.clone());
        let squad = dispatcher.build_squad(phase_id, tasks);
        let result = dispatcher
            .dispatch(&squad, &self.sub_agent, session_id, cancel)
            .await?;
        Ok(result.member_results)
    }

    pub fn should_activate(decomposition_steps: &[String], intent: &str) -> bool {
        decomposition_steps.len() > 5
            || intent.contains("批量")
            || intent.contains("大规模")
            || intent.contains("batch")
            || intent.contains("全部")
            || intent.contains("all")
    }

    async fn load_memories(&self, goal: &mut Goal, session_id: &str) -> Result<()> {
        let mem = memory::AgentMemory::load_global(&self.db, AgentRole::WorkFlow, 5);
        if !mem.is_empty() {
            let history = mem.render_prompt(800);
            goal.description = format!("{}\n\n【历史经验】\n{}", goal.description, history);
        }
        let _ = session_id;
        Ok(())
    }

    async fn persist_goal(&self, goal: &Goal, session_id: &str) -> Result<()> {
        let _ = (goal, session_id);
        Ok(())
    }
}

pub fn build_subflow_from_goal(goal: &Goal, subgoal: &Goal) -> SubFlowInput {
    SubFlowInput {
        id: subgoal.id.clone(),
        description: subgoal.description.clone(),
        expected_output: subgoal.acceptance_criteria.join("; "),
        original_prompt: Some(goal.description.clone()),
        depends_on_outputs: vec![],
        sibling_outputs: vec![],
        pending_agent_messages: vec![],
        // 2026-09-17 第 75 轮:Goal 派生 WorkFlow 默认 SubAgent 委派。
        intended_role: Some(crate::agent::context::AgentRole::SubAgent),
        // 2026-09-17 第 82+ 轮 P0-1:Goal 路径默认无目标应用自动启动,LLM 可显式指定。
        // 2026-09-19 第 91 轮 P0-7/P0-8:
        retry_count: 0, retry_hint: String::new(), max_iterations: None,
        pre_explore: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_config_default() {
        let cfg = WorkflowConfig::default();
        assert_eq!(cfg.max_retries_per_goal, 3);
        assert_eq!(cfg.max_adaptive_attempts, 5);
        assert_eq!(cfg.max_parallel_squads, 3);
        assert_eq!(cfg.max_members_per_squad, 5);
    }

    #[test]
    fn should_activate_with_many_steps() {
        let steps = vec![
            "s1".to_string(),
            "s2".to_string(),
            "s3".to_string(),
            "s4".to_string(),
            "s5".to_string(),
            "s6".to_string(),
        ];
        assert!(WorkFlowRunner::should_activate(&steps, "normal"));
    }

    #[test]
    fn should_activate_with_batch_intent() {
        let steps = vec!["s1".to_string()];
        assert!(WorkFlowRunner::should_activate(&steps, "批量处理所有文件"));
    }

    #[test]
    fn should_not_activate_small_task() {
        let steps = vec!["s1".to_string(), "s2".to_string()];
        assert!(!WorkFlowRunner::should_activate(&steps, "简单查询"));
    }

    #[test]
    fn build_subflow_from_goal_constructs_input() {
        let parent = Goal::new("g-1", "父目标", "完成整个模块重构");
        let subgoal = Goal::new("g-1-1", "子目标1", "重构错误处理");
        let input = build_subflow_from_goal(&parent, &subgoal);
        assert_eq!(input.id, "g-1-1");
        assert_eq!(input.description, "重构错误处理");
        assert_eq!(input.original_prompt, Some("完成整个模块重构".to_string()));
    }

    #[test]
    fn workflow_result_default_state() {
        let result = WorkflowResult {
            goal_id: "g-1".into(),
            goal_state: GoalState::Pending,
            phases_completed: 0,
            total_phases: 3,
            squads_dispatched: 0,
            tasks_executed: 0,
            tasks_succeeded: 0,
            total_usage: Usage::default(),
            summary: String::new(),
            error: None,
        };
        assert_eq!(result.goal_state, GoalState::Pending);
        assert_eq!(result.total_phases, 3);
    }
}
