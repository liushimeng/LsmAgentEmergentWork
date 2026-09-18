//! agent_messages 表 DAO —— Agent 间消息持久化。
//!
//! 消息按 (session_id, to_role, consumed) 索引,消费后标记 consumed=1。
//! 设计见 `docs/WindowUse多轮对话与Agent间通信增强设计/01-设计与解决方案.md` §3.3。

use rusqlite::{params, OptionalExtension};

use crate::agent::agent_message::AgentMessage;
use crate::agent::context::AgentRole;
use crate::config::Result;

use super::Db;

impl Db {
    /// 插入一条 Agent 消息。
    pub fn insert_agent_message(&self, msg: &AgentMessage) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO agent_messages (msg_id, session_id, from_role, to_role, payload_json, consumed)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                &msg.id,
                &msg.session_id,
                msg.from_role.as_str(),
                msg.to_role.as_str(),
                serde_json::to_string(&msg.payload)
                    .map_err(|e| crate::config::ConfigError::Serialization(e.to_string()))?,
            ],
        )?;
        Ok(())
    }

    /// 查看指定角色未消费的消息(不标记消费,仅预览,取最早一条)。
    pub fn peek_agent_message(
        &self,
        session_id: &str,
        to_role: AgentRole,
    ) -> Result<Option<AgentMessage>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row: Option<(String, String, String, String)> = conn
            .query_row(
                "SELECT msg_id, from_role, payload_json, created_at
                 FROM agent_messages
                 WHERE session_id = ?1 AND to_role = ?2 AND consumed = 0
                 ORDER BY id ASC LIMIT 1",
                params![session_id, to_role.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        match row {
            Some((msg_id, from_role, payload_json, created_at)) => {
                let from_role: AgentRole = from_role.as_str().into();
                let payload = serde_json::from_str(&payload_json)
                    .map_err(|e| crate::config::ConfigError::Serialization(e.to_string()))?;
                Ok(Some(AgentMessage {
                    id: msg_id,
                    session_id: session_id.to_string(),
                    from_role,
                    to_role,
                    payload,
                    created_at,
                    consumed: false,
                }))
            }
            None => Ok(None),
        }
    }

    /// 消费(取出并标记 consumed=1)一条消息。
    pub fn consume_agent_message(
        &self,
        session_id: &str,
        to_role: AgentRole,
    ) -> Result<Option<AgentMessage>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row: Option<(String, String, String, String)> = conn
            .query_row(
                "SELECT msg_id, from_role, payload_json, created_at
                 FROM agent_messages
                 WHERE session_id = ?1 AND to_role = ?2 AND consumed = 0
                 ORDER BY id ASC LIMIT 1",
                params![session_id, to_role.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        match row {
            Some((msg_id, from_role, payload_json, created_at)) => {
                // 标记已消费
                conn.execute(
                    "UPDATE agent_messages SET consumed = 1 WHERE msg_id = ?1",
                    params![&msg_id],
                )?;
                let from_role: AgentRole = from_role.as_str().into();
                let payload = serde_json::from_str(&payload_json)
                    .map_err(|e| crate::config::ConfigError::Serialization(e.to_string()))?;
                Ok(Some(AgentMessage {
                    id: msg_id,
                    session_id: session_id.to_string(),
                    from_role,
                    to_role,
                    payload,
                    created_at,
                    consumed: true,
                }))
            }
            None => Ok(None),
        }
    }

    /// 标记指定消息为已消费。
    pub fn mark_agent_message_consumed(&self, msg_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "UPDATE agent_messages SET consumed = 1 WHERE msg_id = ?1",
            params![msg_id],
        )?;
        Ok(())
    }

    /// 清理指定 session 的全部消息(Session 结束时调用)。
    pub fn clear_agent_messages(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "DELETE FROM agent_messages WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_message::{AgentMessage, MessagePayload};
    use crate::agent::context::AgentRole;
    use crate::config::Paths;
    use tempfile::tempdir;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    #[test]
    fn insert_and_peek_roundtrip() {
        let (db, _d) = fresh_db();
        let msg = AgentMessage::new(
            "s1",
            AgentRole::WebUse,
            AgentRole::SubAgent,
            MessagePayload::WindowTextRead {
                window_id: "w1".into(),
                window_title: "记事本".into(),
                path: "/0/1".into(),
                content: "hello".into(),
            },
        );
        db.insert_agent_message(&msg).unwrap();

        let peeked = db.peek_agent_message("s1", AgentRole::SubAgent).unwrap();
        assert!(peeked.is_some());
        let peeked = peeked.unwrap();
        assert_eq!(peeked.from_role, AgentRole::WebUse);
        assert_eq!(peeked.consumed, false);
        match &peeked.payload {
            MessagePayload::WindowTextRead { content, .. } => assert_eq!(content, "hello"),
            _ => panic!("payload 类型不匹配"),
        }
    }

    #[test]
    fn peek_nonexistent_returns_none() {
        let (db, _d) = fresh_db();
        let result = db.peek_agent_message("s1", AgentRole::SubAgent).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn consume_marks_consumed() {
        let (db, _d) = fresh_db();
        let msg = AgentMessage::new(
            "s1",
            AgentRole::WebUse,
            AgentRole::SubAgent,
            MessagePayload::Data {
                key: "k".into(),
                value: "v".into(),
            },
        );
        db.insert_agent_message(&msg).unwrap();

        // 消费
        let consumed = db.consume_agent_message("s1", AgentRole::SubAgent).unwrap();
        assert!(consumed.is_some());
        assert_eq!(consumed.unwrap().consumed, true);

        // 再次 peek 应为 None(已消费)
        let peeked = db.peek_agent_message("s1", AgentRole::SubAgent).unwrap();
        assert!(peeked.is_none());
    }

    #[test]
    fn clear_session_removes_all() {
        let (db, _d) = fresh_db();
        for _ in 0..3 {
            let msg = AgentMessage::new(
                "s1",
                AgentRole::WebUse,
                AgentRole::SubAgent,
                MessagePayload::Data {
                    key: "k".into(),
                    value: "v".into(),
                },
            );
            db.insert_agent_message(&msg).unwrap();
        }
        // 插入另一 session 的消息(不应被清理)
        let msg2 = AgentMessage::new(
            "s2",
            AgentRole::WebUse,
            AgentRole::SubAgent,
            MessagePayload::Data {
                key: "k".into(),
                value: "v".into(),
            },
        );
        db.insert_agent_message(&msg2).unwrap();

        db.clear_agent_messages("s1").unwrap();
        assert!(db
            .peek_agent_message("s1", AgentRole::SubAgent)
            .unwrap()
            .is_none());
        // s2 的消息仍在
        assert!(db
            .peek_agent_message("s2", AgentRole::SubAgent)
            .unwrap()
            .is_some());
    }

    #[test]
    fn mark_consumed_by_id() {
        let (db, _d) = fresh_db();
        let msg = AgentMessage::new(
            "s1",
            AgentRole::WebUse,
            AgentRole::SubAgent,
            MessagePayload::Data {
                key: "k".into(),
                value: "v".into(),
            },
        );
        let msg_id = msg.id.clone();
        db.insert_agent_message(&msg).unwrap();
        db.mark_agent_message_consumed(&msg_id).unwrap();

        // 已标记消费,peek 应为 None
        assert!(db
            .peek_agent_message("s1", AgentRole::SubAgent)
            .unwrap()
            .is_none());
    }
}
