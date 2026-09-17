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
pub mod cancel;
pub mod compact;
pub mod context;
pub mod debug;
pub mod extrace;
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
pub mod session_context;
pub mod session_fork;
pub mod subagent;
pub mod system_prompt;
pub mod tool_schema_validator;
pub mod tools;
pub mod web_use;
pub mod window;
pub mod window_state;
pub mod window_use;
pub mod workflow;
pub mod workflow_json_validate;
pub mod workspace;
pub mod yolo;

#[cfg(test)]
mod tests;

// 运行时辅助项再导出:保持拆分前 `crate::agent::Xxx` 路径对外完全兼容
// (window_use.rs 等测试直接引用 `crate::agent::should_nudge_window_ops`)。
pub(crate) use runtime_hints::{
    build_runtime_hints, should_nudge_web_ops, should_nudge_window_ops, web_ops_nudge_text,
    FORCED_TOOL_NUDGE_TEXT, WINDOW_OPS_NUDGE_TEXT,
};
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

const DEFAULT_MAX_ITERATIONS: usize = 16;

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
    /// 首迭代强制工具(2026-09-16 第 63 轮):WebUse/WindowUse 等专项 Agent 在第 0 轮
    /// 强制调用指定工具(如 BrowserNew),后续轮次恢复 auto。None 表示不强制。
    /// 设计见 tmpPlan/2026-09-16_08-WebUse全链路优化与TUI重复输出修复方案.md。
    first_iter_forced_tool: Option<String>,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmClient>, profile: AgentProfile) -> Self {
        Self {
            llm,
            profile,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            max_truncation_resume: DEFAULT_MAX_TRUNCATION_RESUME,
            max_overflow_recoveries: DEFAULT_MAX_OVERFLOW_RECOVERIES,
            first_iter_forced_tool: None,
        }
    }

    pub fn with_max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    /// 设置首迭代强制工具(2026-09-16 第 63 轮):仅在首次 LLM 调用时强制
    /// 调用指定工具,后续轮次恢复 auto。用于 WebUse/WindowUse 等专项 Agent
    /// 确保首步必定执行工具调用,避免"纯文本空转"。
    pub fn with_first_iter_forced_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.first_iter_forced_tool = Some(tool_name.into());
        self
    }

    /// 读取首迭代强制工具(测试/诊断用)。
    pub fn first_iter_forced_tool(&self) -> Option<&str> {
        self.first_iter_forced_tool.as_deref()
    }

    /// 复制本 Agent 的配置(llm/profile/迭代上限等),但**不带**首迭代强制工具。
    ///
    /// 2026-09-17 第 79 轮:WebUse 多轮场景使用——已有存活浏览器页面时,
    /// 强制 BrowserNew 反而会重复开页;换用本副本让首迭代自由决策
    /// (prompt 中注入「已打开页面」列表 + 出口兜底防纯文本空转)。
    pub fn replicate_without_forced_tool(&self) -> Self {
        Self {
            llm: self.llm.clone(),
            profile: self.profile.clone(),
            max_iterations: self.max_iterations,
            max_truncation_resume: self.max_truncation_resume,
            max_overflow_recoveries: self.max_overflow_recoveries,
            first_iter_forced_tool: None,
        }
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
