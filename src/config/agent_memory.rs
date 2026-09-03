//! `agent_memory` 表 DAO:每个 Agent 的输入/输出/错误经验沉淀。
//!
//! Schema:
//! ```sql
//! CREATE TABLE agent_memory (
//!     id           INTEGER PRIMARY KEY AUTOINCREMENT,
//!     session_id   TEXT NOT NULL,
//!     agent_role   TEXT NOT NULL CHECK(agent_role IN ('yolo','plan','main','subagent','quality')),
//!     input_summary  TEXT NOT NULL,
//!     output_summary TEXT NOT NULL,
//!     error_summary  TEXT,
//!     artifacts    TEXT,
//!     created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime'))
//! );
//! ```

use rusqlite::params;

use crate::agent::context::AgentRole;
use crate::config::Result;

use super::Db;

/// 写入用条目。
#[derive(Debug, Clone)]
pub struct AgentMemoryEntry {
    pub session_id: String,
    pub agent_role: AgentRole,
    pub input_summary: String,
    pub output_summary: String,
    pub error_summary: Option<String>,
    /// JSON 序列化的字符串(由调用方负责 to_string())
    pub artifacts: String,
}

/// 读出的行。
#[derive(Debug, Clone)]
pub struct AgentMemoryRow {
    pub id: i64,
    pub session_id: String,
    pub agent_role: AgentRole,
    pub input_summary: String,
    pub output_summary: String,
    pub error_summary: Option<String>,
    pub artifacts: String,
    pub created_at: String,
}

impl Db {
    /// 写入一条 Agent-Memory。
    pub fn insert_agent_memory(&self, e: &AgentMemoryEntry) -> Result<i64> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO agent_memory(session_id, agent_role, input_summary, output_summary, error_summary, artifacts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                e.session_id,
                e.agent_role.as_str(),
                e.input_summary,
                e.output_summary,
                e.error_summary,
                e.artifacts,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// 读取指定 role + 可选 session 的最近 N 条(按 id DESC)。
    ///
    /// - `session_id = Some(s)`:仅该 session
    /// - `session_id = None`:跨 session(全局经验)
    pub fn list_agent_memory(
        &self,
        role: AgentRole,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AgentMemoryRow>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let (sql, params_vec): (&str, Vec<rusqlite::types::Value>) = match session_id {
            Some(sid) => (
                "SELECT id, session_id, agent_role, input_summary, output_summary, error_summary, artifacts, created_at
                 FROM agent_memory WHERE agent_role = ?1 AND session_id = ?2
                 ORDER BY id DESC LIMIT ?3",
                vec![
                    rusqlite::types::Value::Text(role.as_str().into()),
                    rusqlite::types::Value::Text(sid.to_string()),
                    rusqlite::types::Value::Integer(limit as i64),
                ],
            ),
            None => (
                "SELECT id, session_id, agent_role, input_summary, output_summary, error_summary, artifacts, created_at
                 FROM agent_memory WHERE agent_role = ?1
                 ORDER BY id DESC LIMIT ?2",
                vec![
                    rusqlite::types::Value::Text(role.as_str().into()),
                    rusqlite::types::Value::Integer(limit as i64),
                ],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let iter = stmt.query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
            let role_str: String = row.get(2)?;
            let role = match role_str.as_str() {
                "yolo" => AgentRole::Yolo,
                "plan" => AgentRole::Plan,
                "main" => AgentRole::MainWork,
                "subagent" => AgentRole::SubAgent,
                "quality" => AgentRole::QualityCheck,
                other => {
                    return Err(rusqlite::Error::InvalidColumnType(
                        2,
                        format!("unknown role: {other}"),
                        rusqlite::types::Type::Text,
                    ))
                }
            };
            Ok(AgentMemoryRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                agent_role: role,
                input_summary: row.get(3)?,
                output_summary: row.get(4)?,
                error_summary: row.get(5)?,
                artifacts: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for r in iter {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Db, Paths};
    use tempfile::tempdir;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    #[test]
    fn insert_and_list() {
        let (db, _d) = fresh_db();
        let _ = _d;
        db.insert_agent_memory(&AgentMemoryEntry {
            session_id: "s-1".into(),
            agent_role: AgentRole::SubAgent,
            input_summary: "读取 x.rs".into(),
            output_summary: "已读取 100 行".into(),
            error_summary: None,
            artifacts: "{\"unit\":\"wf-1\"}".into(),
        })
        .unwrap();
        db.insert_agent_memory(&AgentMemoryEntry {
            session_id: "s-2".into(),
            agent_role: AgentRole::SubAgent,
            input_summary: "读取 y.rs".into(),
            output_summary: "失败".into(),
            error_summary: Some("file not found".into()),
            artifacts: "{}".into(),
        })
        .unwrap();

        let in_s1 = db.list_agent_memory(AgentRole::SubAgent, Some("s-1"), 10).unwrap();
        assert_eq!(in_s1.len(), 1);
        assert_eq!(in_s1[0].input_summary, "读取 x.rs");
        assert!(in_s1[0].error_summary.is_none());

        let global = db.list_agent_memory(AgentRole::SubAgent, None, 10).unwrap();
        assert_eq!(global.len(), 2);
        // DESC order: s-2 排在 s-1 之前
        assert_eq!(global[0].session_id, "s-2");
        assert_eq!(global[1].session_id, "s-1");
    }

    #[test]
    fn role_filter_works() {
        let (db, _d) = fresh_db();
        db.insert_agent_memory(&AgentMemoryEntry {
            session_id: "s-1".into(),
            agent_role: AgentRole::SubAgent,
            input_summary: "x".into(),
            output_summary: "y".into(),
            error_summary: None,
            artifacts: "{}".into(),
        })
        .unwrap();
        db.insert_agent_memory(&AgentMemoryEntry {
            session_id: "s-1".into(),
            agent_role: AgentRole::Yolo,
            input_summary: "用户提问".into(),
            output_summary: "simple".into(),
            error_summary: None,
            artifacts: "{}".into(),
        })
        .unwrap();
        let sub = db.list_agent_memory(AgentRole::SubAgent, None, 10).unwrap();
        let yolo = db.list_agent_memory(AgentRole::Yolo, None, 10).unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(yolo.len(), 1);
    }
}