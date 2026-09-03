//! `session_memory` 表 DAO:Session 内任务串联与多轮上下文。
//!
//! Schema:
//! ```sql
//! CREATE TABLE session_memory (
//!     id           INTEGER PRIMARY KEY AUTOINCREMENT,
//!     session_id   TEXT NOT NULL,
//!     seq          INTEGER NOT NULL,
//!     role         TEXT NOT NULL CHECK(role IN ('yolo','plan','main','subagent','quality','session','user')),
//!     event_type   TEXT NOT NULL CHECK(event_type IN ('input','output','failure','suggestion','summary')),
//!     content      TEXT NOT NULL,
//!     usage_input  INTEGER NOT NULL DEFAULT 0,
//!     usage_output INTEGER NOT NULL DEFAULT 0,
//!     created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
//!     UNIQUE(session_id, seq)
//! );
//! ```

use rusqlite::{params, OptionalExtension};

use crate::agent::context::AgentRole;
use crate::config::Result;

/// 事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Input,
    Output,
    Failure,
    Suggestion,
    Summary,
}

impl EventType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Failure => "failure",
            Self::Suggestion => "suggestion",
            Self::Summary => "summary",
        }
    }
}

/// 一次 Session 内存入的条目。
#[derive(Debug, Clone)]
pub struct SessionMemoryEntry {
    pub session_id: String,
    pub role: AgentRole,
    pub event_type: EventType,
    pub content: String,
    pub usage_input: u32,
    pub usage_output: u32,
}

/// 读出的完整行(带 id / seq / created_at)。
#[derive(Debug, Clone)]
pub struct SessionMemoryRow {
    pub id: i64,
    pub session_id: String,
    pub seq: i64,
    pub role: AgentRole,
    pub event_type: EventType,
    pub content: String,
    pub usage_input: u32,
    pub usage_output: u32,
    pub created_at: String,
}

impl SessionMemoryRow {
    pub fn is_summary(&self) -> bool {
        matches!(self.event_type, EventType::Summary)
    }
}

use super::Db;

impl Db {
    /// 取下一个 seq(同一 session 内自增)。无记录时返回 1。
    pub fn next_session_seq(&self, session_id: &str) -> Result<i64> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let max: Option<i64> = conn
            .query_row(
                "SELECT MAX(seq) FROM session_memory WHERE session_id = ?1",
                params![session_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();
        Ok(max.map(|m| m + 1).unwrap_or(1))
    }

    /// 写入一条 Session 记忆。
    pub fn insert_session_memory(&self, e: &SessionMemoryEntry) -> Result<i64> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let seq = match conn
            .query_row(
                "SELECT MAX(seq) FROM session_memory WHERE session_id = ?1",
                params![&e.session_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
        {
            Some(Some(m)) => m + 1,
            _ => 1,
        };
        conn.execute(
            "INSERT INTO session_memory(session_id, seq, role, event_type, content, usage_input, usage_output)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                e.session_id,
                seq,
                e.role.as_str(),
                e.event_type.as_str(),
                e.content,
                e.usage_input as i64,
                e.usage_output as i64,
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }

    /// 列出指定 session 的最近 N 条(按 seq DESC)。
    pub fn list_session_memory(&self, session_id: &str, limit: usize) -> Result<Vec<SessionMemoryRow>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, session_id, seq, role, event_type, content, usage_input, usage_output, created_at
             FROM session_memory WHERE session_id = ?1 ORDER BY seq DESC LIMIT ?2",
        )?;
        let iter = stmt.query_map(params![session_id, limit as i64], row_to_memory)?;
        let mut out = Vec::new();
        for r in iter {
            out.push(r?);
        }
        Ok(out)
    }

    /// 取最近 N 条 Summary 事件(用于 Yolo 历史上下文注入)。
    pub fn latest_summaries(&self, session_id: &str, n: usize) -> Result<Vec<SessionMemoryRow>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, session_id, seq, role, event_type, content, usage_input, usage_output, created_at
             FROM session_memory WHERE session_id = ?1 AND event_type = 'summary'
             ORDER BY seq DESC LIMIT ?2",
        )?;
        let iter = stmt.query_map(params![session_id, n as i64], row_to_memory)?;
        let mut out = Vec::new();
        for r in iter {
            out.push(r?);
        }
        Ok(out)
    }
}

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMemoryRow> {
    let role_str: String = row.get(3)?;
    let role = match role_str.as_str() {
        "yolo" => AgentRole::Yolo,
        "plan" => AgentRole::Plan,
        "main" => AgentRole::MainWork,
        "subagent" => AgentRole::SubAgent,
        "quality" => AgentRole::QualityCheck,
        "session" => AgentRole::SessionContext,
        "user" => AgentRole::SessionContext, // user 消息也归到 session
        other => {
            return Err(rusqlite::Error::InvalidColumnType(
                3,
                format!("unknown role in session_memory: {other}"),
                rusqlite::types::Type::Text,
            ))
        }
    };
    let event_str: String = row.get(4)?;
    let event = match event_str.as_str() {
        "input" => EventType::Input,
        "output" => EventType::Output,
        "failure" => EventType::Failure,
        "suggestion" => EventType::Suggestion,
        "summary" => EventType::Summary,
        other => {
            return Err(rusqlite::Error::InvalidColumnType(
                4,
                format!("unknown event_type: {other}"),
                rusqlite::types::Type::Text,
            ))
        }
    };
    let usage_input: i64 = row.get(6)?;
    let usage_output: i64 = row.get(7)?;
    Ok(SessionMemoryRow {
        id: row.get(0)?,
        session_id: row.get(1)?,
        seq: row.get(2)?,
        role,
        event_type: event,
        content: row.get(5)?,
        usage_input: usage_input as u32,
        usage_output: usage_output as u32,
        created_at: row.get(8)?,
    })
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
    fn next_seq_starts_at_one() {
        let (db, _d) = fresh_db();
        assert_eq!(db.next_session_seq("s-1").unwrap(), 1);
    }

    #[test]
    fn seq_increments_per_session() {
        let (db, _d) = fresh_db();
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: "s-1".into(),
            role: AgentRole::Yolo,
            event_type: EventType::Summary,
            content: "summary 1".into(),
            usage_input: 0,
            usage_output: 0,
        })
        .unwrap();
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: "s-1".into(),
            role: AgentRole::Yolo,
            event_type: EventType::Summary,
            content: "summary 2".into(),
            usage_input: 0,
            usage_output: 0,
        })
        .unwrap();
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: "s-2".into(),
            role: AgentRole::Yolo,
            event_type: EventType::Summary,
            content: "summary 3 (other session)".into(),
            usage_input: 0,
            usage_output: 0,
        })
        .unwrap();

        assert_eq!(db.next_session_seq("s-1").unwrap(), 3);
        assert_eq!(db.next_session_seq("s-2").unwrap(), 2);
        let rows = db.list_session_memory("s-1", 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].seq, 2);
        assert_eq!(rows[0].content, "summary 2");
    }

    #[test]
    fn latest_summaries_filters_by_event_type() {
        let (db, _d) = fresh_db();
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: "s-1".into(),
            role: AgentRole::Yolo,
            event_type: EventType::Summary,
            content: "sum 1".into(),
            usage_input: 1,
            usage_output: 2,
        })
        .unwrap();
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: "s-1".into(),
            role: AgentRole::Yolo,
            event_type: EventType::Failure,
            content: "fail".into(),
            usage_input: 0,
            usage_output: 0,
        })
        .unwrap();
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: "s-1".into(),
            role: AgentRole::Yolo,
            event_type: EventType::Summary,
            content: "sum 2".into(),
            usage_input: 0,
            usage_output: 0,
        })
        .unwrap();
        let sums = db.latest_summaries("s-1", 10).unwrap();
        assert_eq!(sums.len(), 2);
        assert!(sums.iter().all(|r| r.is_summary()));
    }
}