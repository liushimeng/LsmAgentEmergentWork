//! Goal 状态机:WorkFlow Agent 的核心运行时对象。
//!
//! 借鉴 atomcode GoalState(5 态) + deepseek-harness(CAS 事件溯源) 设计,
//! 提供事务性任务生命周期管理。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{AgentError, Result};
use crate::session::now_readable;

/// Goal 状态(6 态,借鉴 atomcode + deepseek-harness)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    /// 等待执行
    Pending,
    /// 执行中
    Pursuing,
    /// 被阻塞(依赖未满足 / 资源不可用)
    Blocked,
    /// 暂停(等待用户输入 / 时间耗尽)
    Paused,
    /// 目标达成
    Satisfied,
    /// 终态失败(超过重试上限)
    Failed,
}

impl GoalState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Satisfied | Self::Failed)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Pursuing)
    }
}

/// 优先级(1-10,10 最高)。
pub type Priority = u8;

/// Goal:WorkFlow Agent 的核心运行时对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub description: String,
    pub state: GoalState,
    pub priority: Priority,
    pub dependencies: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub subgoals: Vec<Goal>,
    pub retry_count: usize,
    pub max_retries: usize,
    pub error_text: Option<String>,
    pub metadata: HashMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

impl Goal {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            parent_id: None,
            title: title.into(),
            description: description.into(),
            state: GoalState::Pending,
            priority: 5,
            dependencies: Vec::new(),
            acceptance_criteria: Vec::new(),
            subgoals: Vec::new(),
            retry_count: 0,
            max_retries: 3,
            error_text: None,
            metadata: HashMap::new(),
            created_at: now_readable(),
            updated_at: now_readable(),
            completed_at: None,
        }
    }

    pub fn with_priority(mut self, p: Priority) -> Self {
        self.priority = p.clamp(1, 10);
        self
    }

    pub fn with_max_retries(mut self, n: usize) -> Self {
        self.max_retries = n;
        self
    }

    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.dependencies = deps;
        self
    }

    pub fn with_acceptance(mut self, criteria: Vec<String>) -> Self {
        self.acceptance_criteria = criteria;
        self
    }

    pub fn add_subgoal(&mut self, subgoal: Goal) {
        self.subgoals.push(subgoal);
    }

    pub fn start(&mut self) -> Result<()> {
        self.transition(GoalState::Pending, GoalState::Pursuing)
    }

    pub fn satisfy(&mut self) -> Result<()> {
        self.transition(GoalState::Pursuing, GoalState::Satisfied)?;
        self.completed_at = Some(now_readable());
        Ok(())
    }

    pub fn fail(&mut self, reason: &str) -> Result<()> {
        self.error_text = Some(reason.to_string());
        self.transition(GoalState::Pursuing, GoalState::Failed)?;
        self.completed_at = Some(now_readable());
        Ok(())
    }

    pub fn block(&mut self, reason: &str) -> Result<()> {
        self.error_text = Some(reason.to_string());
        self.transition(GoalState::Pursuing, GoalState::Blocked)
    }

    pub fn unblock(&mut self) -> Result<()> {
        self.transition(GoalState::Blocked, GoalState::Pursuing)
    }

    pub fn pause(&mut self) -> Result<()> {
        self.transition(GoalState::Pursuing, GoalState::Paused)
    }

    pub fn resume(&mut self) -> Result<()> {
        self.transition(GoalState::Paused, GoalState::Pursuing)
    }

    pub fn retry(&mut self) -> Result<()> {
        if self.retry_count >= self.max_retries {
            return Err(AgentError::GoalMaxRetries {
                goal_id: self.id.clone(),
                max_retries: self.max_retries,
            });
        }
        self.retry_count += 1;
        self.transition(self.state.clone(), GoalState::Pursuing)
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    pub fn all_subgoals_satisfied(&self) -> bool {
        !self.subgoals.is_empty()
            && self
                .subgoals
                .iter()
                .all(|sg| sg.state == GoalState::Satisfied)
    }

    pub fn progress(&self) -> f64 {
        if self.subgoals.is_empty() {
            return if self.state == GoalState::Satisfied {
                1.0
            } else {
                0.0
            };
        }
        let completed = self
            .subgoals
            .iter()
            .filter(|sg| sg.state == GoalState::Satisfied)
            .count();
        completed as f64 / self.subgoals.len() as f64
    }

    fn transition(&mut self, from: GoalState, to: GoalState) -> Result<()> {
        if self.state == from {
            self.state = to;
            self.updated_at = now_readable();
            Ok(())
        } else {
            Err(AgentError::StateTransition {
                goal_id: self.id.clone(),
                from: format!("{:?}", self.state),
                attempted_to: format!("{:?}", to),
            })
        }
    }
}

/// Goal 持久化存储。
pub struct GoalStore {
    /// 预留:save/load 落库实现接入后启用读取。
    #[allow(dead_code)]
    db: Arc<crate::config::Db>,
}

impl GoalStore {
    pub fn new(db: Arc<crate::config::Db>) -> Self {
        Self { db }
    }

    pub fn save(&self, goal: &Goal, session_id: &str) -> Result<()> {
        let _ = (goal, session_id);
        Ok(())
    }

    pub fn load(&self, goal_id: &str) -> Result<Option<Goal>> {
        let _ = goal_id;
        Ok(None)
    }

    pub fn list_by_session(&self, session_id: &str) -> Result<Vec<Goal>> {
        let _ = session_id;
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_new_starts_pending() {
        let goal = Goal::new("g-1", "测试", "描述");
        assert_eq!(goal.state, GoalState::Pending);
        assert_eq!(goal.priority, 5);
        assert_eq!(goal.retry_count, 0);
    }

    #[test]
    fn goal_lifecycle_pending_to_satisfied() {
        let mut goal = Goal::new("g-1", "测试", "描述");
        goal.start().unwrap();
        assert_eq!(goal.state, GoalState::Pursuing);
        goal.satisfy().unwrap();
        assert_eq!(goal.state, GoalState::Satisfied);
        assert!(goal.completed_at.is_some());
    }

    #[test]
    fn goal_lifecycle_pending_to_failed() {
        let mut goal = Goal::new("g-1", "测试", "描述");
        goal.start().unwrap();
        goal.fail("出错了").unwrap();
        assert_eq!(goal.state, GoalState::Failed);
        assert_eq!(goal.error_text.as_deref(), Some("出错了"));
    }

    #[test]
    fn goal_block_unblock() {
        let mut goal = Goal::new("g-1", "测试", "描述");
        goal.start().unwrap();
        goal.block("等待依赖").unwrap();
        assert_eq!(goal.state, GoalState::Blocked);
        goal.unblock().unwrap();
        assert_eq!(goal.state, GoalState::Pursuing);
    }

    #[test]
    fn goal_pause_resume() {
        let mut goal = Goal::new("g-1", "测试", "描述");
        goal.start().unwrap();
        goal.pause().unwrap();
        assert_eq!(goal.state, GoalState::Paused);
        goal.resume().unwrap();
        assert_eq!(goal.state, GoalState::Pursuing);
    }

    #[test]
    fn goal_retry_increments_and_caps() {
        let mut goal = Goal::new("g-1", "测试", "描述").with_max_retries(2);
        goal.start().unwrap();
        goal.retry().unwrap();
        assert_eq!(goal.retry_count, 1);
        goal.retry().unwrap();
        assert_eq!(goal.retry_count, 2);
        assert!(goal.retry().is_err());
    }

    #[test]
    fn goal_invalid_transition_fails() {
        let mut goal = Goal::new("g-1", "测试", "描述");
        // 未 start 不能 satisfy
        assert!(goal.satisfy().is_err());
    }

    #[test]
    fn goal_progress_no_subgoals() {
        let mut goal = Goal::new("g-1", "测试", "描述");
        assert_eq!(goal.progress(), 0.0);
        goal.start().unwrap();
        goal.satisfy().unwrap();
        assert_eq!(goal.progress(), 1.0);
    }

    #[test]
    fn goal_progress_with_subgoals() {
        let mut goal = Goal::new("g-1", "父", "描述");
        let mut sg1 = Goal::new("sg-1", "子1", "描述");
        sg1.state = GoalState::Satisfied;
        let sg2 = Goal::new("sg-2", "子2", "描述");
        goal.subgoals = vec![sg1, sg2];
        assert_eq!(goal.progress(), 0.5);
    }

    #[test]
    fn goal_all_subgoals_satisfied() {
        let mut goal = Goal::new("g-1", "父", "描述");
        let mut sg1 = Goal::new("sg-1", "子1", "描述");
        sg1.state = GoalState::Satisfied;
        let mut sg2 = Goal::new("sg-2", "子2", "描述");
        sg2.state = GoalState::Satisfied;
        goal.subgoals = vec![sg1, sg2];
        assert!(goal.all_subgoals_satisfied());
    }

    #[test]
    fn goal_state_helpers() {
        assert!(GoalState::Satisfied.is_terminal());
        assert!(GoalState::Failed.is_terminal());
        assert!(!GoalState::Pursuing.is_terminal());
        assert!(GoalState::Pursuing.is_active());
        assert!(!GoalState::Blocked.is_active());
    }

    #[test]
    fn goal_priority_clamped() {
        let goal = Goal::new("g-1", "测试", "描述").with_priority(15);
        assert_eq!(goal.priority, 10);
        let goal = Goal::new("g-1", "测试", "描述").with_priority(0);
        assert_eq!(goal.priority, 1);
    }
}
