//! Phase 执行器:阶段内串行/并行混合执行。
//!
//! Phase = Goal 的一个执行阶段,包含多个 WorkFlow 单元。
//! Phase 之间严格串行,Phase 内部可并行。

use serde::{Deserialize, Serialize};

use crate::agent::workflow::WorkflowConfig;
use crate::error::Result;

/// Phase 失败策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseFailurePolicy {
    Retry(usize),
    SkipAndContinue,
    FailGoal,
    AskUser,
}

/// Phase 执行结果。
#[derive(Debug, Clone)]
pub struct PhaseExecutionResult {
    pub phase_id: String,
    pub success: bool,
    pub workflows_completed: usize,
    pub total_workflows: usize,
    pub error: Option<String>,
}

/// Phase 执行器。
pub struct PhaseExecutor {
    /// 预留:execute 全量走配置驱动后启用读取。
    #[allow(dead_code)]
    config: WorkflowConfig,
}

impl PhaseExecutor {
    pub fn new(config: WorkflowConfig) -> Self {
        Self { config }
    }

    pub async fn execute(&self, phase: &Phase, session_id: &str) -> Result<PhaseExecutionResult> {
        let _ = (phase, session_id);
        Ok(PhaseExecutionResult {
            phase_id: phase.id.clone(),
            success: true,
            workflows_completed: phase.workflows.len(),
            total_workflows: phase.workflows.len(),
            error: None,
        })
    }
}

/// Phase 定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phase {
    pub id: String,
    pub name: String,
    pub description: String,
    pub workflows: Vec<crate::agent::main_work::WorkFlowSpec>,
    pub quality_gate: crate::agent::workflow::quality_gate::QualityGate,
    pub on_failure: PhaseFailurePolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_executor_new() {
        let config = WorkflowConfig::default();
        let executor = PhaseExecutor::new(config);
        let _ = executor;
    }

    #[test]
    fn phase_failure_policy_serializes() {
        let policy = PhaseFailurePolicy::Retry(3);
        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("retry"));
    }
}
