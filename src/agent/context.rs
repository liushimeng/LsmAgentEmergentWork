//! Agent 角色枚举 + Agent-Context(实时上下文)。
//!
//! - `AgentRole`:6 个 Agent 角色标识。
//! - `AgentContext`:每个 Agent 在执行一个单元时持有的实时上下文(消息流 + 状态),
//!   与 Session 主上下文隔离,生命周期 = 当前单元。
//!
//! Agent-Context 与 Agent-Memory 的区别见 `docs/多Agent架构重构/01-设计与解决方案.md` §9。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::llm::{ChatMessage, ContentBlock};

/// 6 个 Agent 角色。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// 入口层:任务识别 / 难度分级 / 失败回流
    #[default]
    Yolo,
    /// 规划层:hard 档任务产出 Markdown 方案
    Plan,
    /// 流程层:WorkFlow 编排
    #[serde(rename = "main")]
    MainWork,
    /// 执行层:单流程处理单元
    #[serde(rename = "subagent")]
    SubAgent,
    /// 质检层:单元输出校验
    QualityCheck,
    /// 会话层:SessionMemory 摘要
    SessionContext,
}

impl AgentRole {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Yolo => "yolo",
            Self::Plan => "plan",
            Self::MainWork => "main",
            Self::SubAgent => "subagent",
            Self::QualityCheck => "quality",
            Self::SessionContext => "session",
        }
    }
}

/// Agent-Context:单个 Agent 在执行一个单元时持有的实时上下文。
///
/// - 内存态,生命周期 = 当前单元
/// - 不与其它 Agent / 其它单元共享
/// - 由 Orchestrator 在调用前构造,调用结束后丢弃
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub agent_role: AgentRole,
    pub session_id: String,
    pub unit_id: String,
    pub messages: Vec<ChatMessage>,
    pub state: HashMap<String, Value>,
    pub iteration: usize,
    pub created_at: String,
}

impl AgentContext {
    /// 构造新的 Agent-Context(空消息 + 空状态)。
    pub fn new(agent_role: AgentRole, session_id: impl Into<String>, unit_id: impl Into<String>) -> Self {
        Self {
            agent_role,
            session_id: session_id.into(),
            unit_id: unit_id.into(),
            messages: Vec::new(),
            state: HashMap::new(),
            iteration: 0,
            created_at: crate::session::now_readable(),
        }
    }

    /// 推入一条 user 消息。
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.messages.push(ChatMessage::user(text));
    }

    /// 推入 assistant 消息。
    pub fn push_assistant(&mut self, blocks: Vec<ContentBlock>) {
        self.messages.push(ChatMessage::assistant(blocks));
    }

    /// 推入 tool_result。
    pub fn push_tool_result(
        &mut self,
        id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) {
        self.messages
            .push(ChatMessage::tool_result(id, content, is_error));
    }

    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    pub fn state_get(&self, key: &str) -> Option<&Value> {
        self.state.get(key)
    }

    pub fn state_set(&mut self, key: impl Into<String>, value: Value) {
        self.state.insert(key.into(), value);
    }

    pub fn iteration_inc(&mut self) {
        self.iteration += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_str_roundtrip() {
        assert_eq!(AgentRole::Yolo.as_str(), "yolo");
        assert_eq!(AgentRole::Plan.as_str(), "plan");
        assert_eq!(AgentRole::MainWork.as_str(), "main");
        assert_eq!(AgentRole::SubAgent.as_str(), "subagent");
        assert_eq!(AgentRole::QualityCheck.as_str(), "quality");
        assert_eq!(AgentRole::SessionContext.as_str(), "session");
    }

    #[test]
    fn role_serde_roundtrip() {
        let r = AgentRole::MainWork;
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, "\"main\"");
        let back: AgentRole = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn context_push_and_state() {
        let mut ctx = AgentContext::new(AgentRole::SubAgent, "s-1", "wf-1");
        ctx.push_user("任务");
        ctx.push_assistant(vec![ContentBlock::text("响应")]);
        ctx.push_tool_result("t-1", "out", false);
        ctx.state_set("foo", serde_json::json!(42));

        assert_eq!(ctx.messages().len(), 3);
        assert_eq!(ctx.messages()[0].role, crate::llm::Role::User);
        assert_eq!(
            ctx.state_get("foo").and_then(|v| v.as_i64()),
            Some(42)
        );

        ctx.iteration_inc();
        ctx.iteration_inc();
        assert_eq!(ctx.iteration, 2);
    }
}