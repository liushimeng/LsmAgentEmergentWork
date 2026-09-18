//! Agent 间消息通信机制。
//!
//! 提供 SubAgent-Work / Main-Work / WorkFlow(squad) 等角色间的结构化数据传递:
//! - 执行单元读到窗口文本后,可发消息让其它单元处理
//! - 处理完后,可发消息让请求方把结果写入目标窗口
//! - Main-Work 可发消息指示执行单元聚焦特定窗口
//!
//! 消息持久化到 SQLite `agent_messages` 表,按 (session_id, to_role, consumed) 索引,
//! 消费后标记 consumed=1,避免重复处理。

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::session::now_readable;

/// Agent 间消息结构。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentMessage {
    pub id: String,
    pub session_id: String,
    pub from_role: AgentRole,
    pub to_role: AgentRole,
    pub payload: MessagePayload,
    pub created_at: String,
    pub consumed: bool,
}

/// 消息载荷类型。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessagePayload {
    /// 窗口操控单元 → SubAgent:「我从窗口读到了这段文本,请处理」。
    WindowTextRead {
        window_id: String,
        window_title: String,
        path: String,
        content: String,
    },
    /// SubAgent → 窗口操控单元:「请把这段文本写入窗口」。
    TextToWindow {
        target_window_id: Option<String>,
        content: String,
    },
    /// Main-Work → 窗口操控单元:「请操作这个窗口」。
    WindowFocus { window_id: String, reason: String },
    /// 通用数据传递。
    Data { key: String, value: String },
}

impl AgentMessage {
    /// 构造一条新的 Agent 消息(consumed 默认 false)。
    pub fn new(
        session_id: &str,
        from_role: AgentRole,
        to_role: AgentRole,
        payload: MessagePayload,
    ) -> Self {
        Self {
            id: format!(
                "msg-{}-{}-{}",
                to_role.as_str(),
                now_readable()
                    .replace([' ', ':', '-'], "")
                    .chars()
                    .collect::<String>(),
                rand::random::<u32>()
            ),
            session_id: session_id.to_string(),
            from_role,
            to_role,
            payload,
            created_at: now_readable(),
            consumed: false,
        }
    }

    /// 获取角色的人类可读名称(用于提示)。
    fn role_display(role: AgentRole) -> &'static str {
        match role {
            AgentRole::Yolo => "Yolo",
            AgentRole::Plan => "Plan",
            AgentRole::MainWork => "Main-Work",
            AgentRole::SubAgent => "SubAgent",
            AgentRole::QualityCheck => "Quality-Check",
            AgentRole::SessionContext => "SessionContext",
            AgentRole::Compact => "Compact",
            AgentRole::WorkFlow => "WorkFlow",
        }
    }

    /// 构造人类可读的消息提示(注入到接收方 prompt)。
    pub fn hint(&self) -> String {
        let from = Self::role_display(self.from_role);
        match &self.payload {
            MessagePayload::WindowTextRead {
                window_id,
                window_title,
                path,
                content,
            } => {
                let preview: String = content.chars().take(200).collect();
                let suffix = if content.chars().count() > 200 {
                    "..."
                } else {
                    ""
                };
                format!(
                    "[来自 {from}] 已从窗口 \"{window_title}\"(id={window_id}, path={path}) 读取到以下内容:\n{preview}{suffix}"
                )
            }
            MessagePayload::TextToWindow {
                target_window_id,
                content,
            } => {
                let preview: String = content.chars().take(200).collect();
                let suffix = if content.chars().count() > 200 {
                    "..."
                } else {
                    ""
                };
                match target_window_id {
                    Some(wid) => {
                        format!("[来自 {from}] 请把以下内容写入窗口 id={wid}:\n{preview}{suffix}")
                    }
                    None => {
                        format!("[来自 {from}] 请把以下内容写入合适的目标窗口:\n{preview}{suffix}")
                    }
                }
            }
            MessagePayload::WindowFocus { window_id, reason } => {
                format!("[来自 {from}] 请聚焦并操作窗口 id={window_id},原因: {reason}")
            }
            MessagePayload::Data { key, value } => {
                let preview: String = value.chars().take(200).collect();
                let suffix = if value.chars().count() > 200 {
                    "..."
                } else {
                    ""
                };
                format!("[来自 {from}] {key} = {preview}{suffix}")
            }
        }
    }
}

/// Agent 消息管理器 —— 封装 DB 访问。
#[derive(Clone)]
pub struct AgentMessageManager {
    db: std::sync::Arc<crate::config::Db>,
}

impl AgentMessageManager {
    pub fn new(db: std::sync::Arc<crate::config::Db>) -> Self {
        Self { db }
    }

    /// 发送(持久化)一条 Agent 消息。
    pub async fn send(&self, msg: &AgentMessage) -> crate::error::Result<()> {
        self.db
            .insert_agent_message(msg)
            .map_err(crate::error::AgentError::from)
    }

    /// 查看(to_role 指定角色)未消费的消息(不标记消费,仅预览)。
    pub async fn peek(&self, session_id: &str, to_role: AgentRole) -> Option<AgentMessage> {
        self.db
            .peek_agent_message(session_id, to_role)
            .ok()
            .flatten()
    }

    /// 消费(取出并标记 consumed)一条消息。
    pub async fn consume(&self, session_id: &str, to_role: AgentRole) -> Option<AgentMessage> {
        self.db
            .consume_agent_message(session_id, to_role)
            .ok()
            .flatten()
    }

    /// 标记指定消息为已消费。
    pub async fn mark_consumed(&self, msg_id: &str) -> crate::error::Result<()> {
        self.db
            .mark_agent_message_consumed(msg_id)
            .map_err(crate::error::AgentError::from)
    }

    /// 清理指定 session 的全部消息(Session 结束时调用)。
    pub async fn clear_session(&self, session_id: &str) -> crate::error::Result<()> {
        self.db
            .clear_agent_messages(session_id)
            .map_err(crate::error::AgentError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_window_text_read() {
        let msg = AgentMessage::new(
            "s1",
            AgentRole::Compact,
            AgentRole::SubAgent,
            MessagePayload::WindowTextRead {
                window_id: "w1".into(),
                window_title: "记事本".into(),
                path: "/0/1".into(),
                content: "hello world".into(),
            },
        );
        let hint = msg.hint();
        assert!(hint.contains("Compact"));
        assert!(hint.contains("记事本"));
        assert!(hint.contains("hello world"));
    }

    #[test]
    fn hint_text_to_window_with_target() {
        let msg = AgentMessage::new(
            "s1",
            AgentRole::SubAgent,
            AgentRole::Compact,
            MessagePayload::TextToWindow {
                target_window_id: Some("w2".into()),
                content: "写入这段".into(),
            },
        );
        let hint = msg.hint();
        assert!(hint.contains("w2"));
        assert!(hint.contains("写入这段"));
    }

    #[test]
    fn hint_text_to_window_without_target() {
        let msg = AgentMessage::new(
            "s1",
            AgentRole::SubAgent,
            AgentRole::Compact,
            MessagePayload::TextToWindow {
                target_window_id: None,
                content: "xxx".into(),
            },
        );
        let hint = msg.hint();
        assert!(hint.contains("合适的目标窗口"));
    }

    #[test]
    fn hint_truncates_long_content() {
        let long = "x".repeat(500);
        let msg = AgentMessage::new(
            "s1",
            AgentRole::Compact,
            AgentRole::SubAgent,
            MessagePayload::WindowTextRead {
                window_id: "w".into(),
                window_title: "t".into(),
                path: "/0".into(),
                content: long.clone(),
            },
        );
        let hint = msg.hint();
        assert!(hint.contains("..."));
        // 截断后不超过 200 + 其他文本
        assert!(hint.chars().count() < long.chars().count() + 100);
    }

    #[test]
    fn hint_data_payload() {
        let msg = AgentMessage::new(
            "s1",
            AgentRole::MainWork,
            AgentRole::Compact,
            MessagePayload::Data {
                key: "target_file".into(),
                value: "/path/to/file.rs".into(),
            },
        );
        let hint = msg.hint();
        assert!(hint.contains("target_file"));
        assert!(hint.contains("/path/to/file.rs"));
    }

    #[test]
    fn hint_window_focus() {
        let msg = AgentMessage::new(
            "s1",
            AgentRole::MainWork,
            AgentRole::Compact,
            MessagePayload::WindowFocus {
                window_id: "w5".into(),
                reason: "用户需要操作这个窗口".into(),
            },
        );
        let hint = msg.hint();
        assert!(hint.contains("w5"));
        assert!(hint.contains("用户需要操作这个窗口"));
    }
}
