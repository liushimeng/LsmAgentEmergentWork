//! Agent 核心循环:协议无关的 LLM 规划 -> 工具执行 -> 观察。
//!
//! 多 Agent 架构(6 角色):
//! - Yolo:入口层(任务识别 / 难度分级 / 失败回流)
//! - Plan:规划层(hard 档 Markdown 方案)
//! - Main-Work:流程层(WorkFlow 编排)
//! - SubAgent-Work:执行层(最小单元)
//! - Quality-Check:质检层
//! - SessionContext:会话层
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md`。

pub mod agent_loop;
pub mod agent_message;
pub mod attachments;
pub mod browser;
pub mod browser_watchdog;
pub mod cancel;
pub mod compact;
pub mod context;
pub mod custom_agents;
pub mod debug;
pub mod decision_audit;
pub mod dynamic_subagent;
pub mod extrace;
pub mod human_assist;
pub mod json_repair;
pub mod main_work;
pub mod max_tokens_state;
pub mod memory;
pub mod offline_queue;
pub mod orchestrator;
pub mod overflow;
pub mod partial_json;
pub mod permissions;
pub mod plan;
pub mod plan_validate;
pub mod profile;
pub mod project_context;
pub mod quality;
pub mod runtime_hints;
pub mod safety;
pub mod sandbox_hook;
pub mod self_awareness;
pub mod session_context;
pub mod session_fork;
pub mod subagent;
pub mod system_prompt;
pub mod tool_schema_validator;
pub mod tools;
pub mod window;
pub mod workflow;
pub mod workflow_json_validate;
pub mod workspace;
pub mod yolo;
pub mod todo_state;

#[cfg(test)]
mod tests;

// 运行时辅助项再导出:保持拆分前 `crate::agent::Xxx` 路径对外完全兼容
pub(crate) use runtime_hints::build_runtime_hints;
// 原私有辅助:经本模块命名空间供 agent_loop / tests 子模块 `use super::*` 取用
use runtime_hints::{
    forced_tools_enabled, forced_tools_enabled_from, is_truncation_stop_reason, stable_json_string,
};

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::agent::cancel::{backfill_cancelled_tool_results, CancelToken};
use crate::agent::extrace::ExecutionTrace;
use crate::agent::max_tokens_state::MaxTokensState;
use crate::agent::profile::AgentProfile;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Completion, ContentBlock, LlmClient, RequestMeta, Usage};
use crate::session::Session;

const DEFAULT_MAX_ITERATIONS: usize = 20;

/// 默认最大截断续接次数(对齐 AtomCode `MAX_TRUNCATION_RESUME = 4` 惯例)。
///
/// 当 LLM 输出因 token 上限被截断时(`stop_reason = "max_tokens"` Anthropic /
/// `"length"` OpenAI),自动注入 nudge 消息让模型从断点继续,最多续接此数值次。
const DEFAULT_MAX_TRUNCATION_RESUME: usize = 4;

/// 默认最大上下文溢出恢复次数(对齐截断续接的有界设计)。
///
/// 当 LLM 返回 prompt-too-long 类溢出错误时,自动执行「排水 → 折叠」两级本地
/// 恢复后重试;全会话累计恢复不超过此值,防止恢复与溢出之间打转。
const DEFAULT_MAX_OVERFLOW_RECOVERIES: usize = 4;

/// 一个可运行的 Agent 实例。
///
/// 持有 [`AgentProfile`](profile::AgentProfile)(名称 / 系统提示词 / 工具集),
/// 为后续多 Agent 切换预留扩展口。
pub struct Agent {
    llm: Arc<dyn LlmClient>,
    profile: AgentProfile,
    max_iterations: usize,
    /// 最大截断续接次数(输出被 token 上限截断时自动续接的上限)。
    max_truncation_resume: usize,
    /// 最大上下文溢出恢复次数(排水/折叠重试的全会话预算)。
    max_overflow_recoveries: usize,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmClient>, profile: AgentProfile) -> Self {
        Self {
            llm,
            profile,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_truncation_resume: DEFAULT_MAX_TRUNCATION_RESUME,
            max_overflow_recoveries: DEFAULT_MAX_OVERFLOW_RECOVERIES,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    /// 设置最大截断续接次数(测试 / 特殊场景用)。
    pub fn with_max_truncation_resume(mut self, n: usize) -> Self {
        self.max_truncation_resume = n;
        self
    }

    /// 设置最大上下文溢出恢复次数(测试 / 特殊场景用)。
    pub fn with_max_overflow_recoveries(mut self, n: usize) -> Self {
        self.max_overflow_recoveries = n;
        self
    }

    pub fn llm(&self) -> Arc<dyn LlmClient> {
        self.llm.clone()
    }
    pub fn profile(&self) -> &AgentProfile {
        &self.profile
    }
    pub fn max_iterations(&self) -> usize {
        self.max_iterations
    }
    pub fn max_truncation_resume(&self) -> usize {
        self.max_truncation_resume
    }
    pub fn max_overflow_recoveries(&self) -> usize {
        self.max_overflow_recoveries
    }
}
