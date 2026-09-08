//! 数据库 Schema 初始化

use rusqlite::Connection;

use crate::database::Result;

/// 初始化数据库表结构
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS providers (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            protocol      TEXT NOT NULL CHECK(protocol IN ('anthropic','openai')),
            provider_name TEXT NOT NULL,
            model_name    TEXT NOT NULL,
            end_point     TEXT NOT NULL,
            api_key       TEXT NOT NULL,
            is_active     INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            UNIQUE(protocol, provider_name, model_name, end_point)
        );

        CREATE TABLE IF NOT EXISTS session_memory (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL,
            seq          INTEGER NOT NULL,
            role         TEXT NOT NULL CHECK(role IN ('yolo','plan','main','subagent','quality','session','user')),
            event_type   TEXT NOT NULL CHECK(event_type IN ('input','output','failure','suggestion','summary')),
            content      TEXT NOT NULL,
            usage_input  INTEGER NOT NULL DEFAULT 0,
            usage_output INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            UNIQUE(session_id, seq)
        );

        CREATE TABLE IF NOT EXISTS agent_memory (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL,
            agent_role   TEXT NOT NULL CHECK(agent_role IN ('yolo','plan','main','subagent','quality')),
            input_summary  TEXT NOT NULL,
            output_summary TEXT NOT NULL,
            error_summary  TEXT,
            artifacts    TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime'))
        );

        CREATE INDEX IF NOT EXISTS idx_session_memory_session_seq
            ON session_memory(session_id, seq);
        CREATE INDEX IF NOT EXISTS idx_agent_memory_role
            ON agent_memory(agent_role, id);
        "#,
    )?;
    Ok(())
}
