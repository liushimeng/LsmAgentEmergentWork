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
            role         TEXT NOT NULL CHECK(role IN ('yolo','plan','main','subagent','quality','session','user','compact','debug')),
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
            agent_role   TEXT NOT NULL CHECK(agent_role IN ('yolo','plan','main','subagent','quality','session','debug','compact')),
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
    migrate(conn)?;
    Ok(())
}

/// 幂等迁移:
/// 1. providers 表补 `context_max_size` 列(ADD COLUMN 带常量 DEFAULT,存量行自动回填);
/// 2. session_memory / agent_memory 的 role CHECK 约束扩展 'compact'/'debug'/'session'
///    (SQLite 不支持修改 CHECK,采用表重建;仅当旧 CHECK 不含 'compact' 时执行)。
pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_providers_context_max_size(conn)?;
    rebuild_role_check(conn, "session_memory")?;
    rebuild_role_check(conn, "agent_memory")?;
    Ok(())
}

/// providers 表补 context_max_size 列(默认 800K tokens,存量记录自动补全)。
fn migrate_providers_context_max_size(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(providers)")?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if cols.iter().any(|c| c == "context_max_size") {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE providers ADD COLUMN context_max_size INTEGER NOT NULL DEFAULT 800000",
        [],
    )?;
    Ok(())
}

/// 若 table 的建表 SQL 中 role CHECK 不含 'compact',重建该表以扩展角色值域。
fn rebuild_role_check(conn: &Connection, table: &str) -> Result<()> {
    let sql: Option<String> = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get(0),
    )?;
    let Some(sql) = sql else { return Ok(()) };
    if sql.contains("'compact'") {
        return Ok(());
    }
    let (create_sql, cols) = match table {
        "session_memory" => (
            r#"CREATE TABLE session_memory (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL,
            seq          INTEGER NOT NULL,
            role         TEXT NOT NULL CHECK(role IN ('yolo','plan','main','subagent','quality','session','user','compact','debug')),
            event_type   TEXT NOT NULL CHECK(event_type IN ('input','output','failure','suggestion','summary')),
            content      TEXT NOT NULL,
            usage_input  INTEGER NOT NULL DEFAULT 0,
            usage_output INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            UNIQUE(session_id, seq)
        )"#,
            "id, session_id, seq, role, event_type, content, usage_input, usage_output, created_at",
        ),
        "agent_memory" => (
            r#"CREATE TABLE agent_memory (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL,
            agent_role   TEXT NOT NULL CHECK(agent_role IN ('yolo','plan','main','subagent','quality','session','debug','compact')),
            input_summary  TEXT NOT NULL,
            output_summary TEXT NOT NULL,
            error_summary  TEXT,
            artifacts    TEXT,
            created_at   TEXT NOT NULL DEFAULT (datetime('now','localtime'))
        )"#,
            "id, session_id, agent_role, input_summary, output_summary, error_summary, artifacts, created_at",
        ),
        _ => return Ok(()),
    };
    let batch = format!(
        "BEGIN;
         ALTER TABLE {table} RENAME TO {table}_old;
         {create_sql};
         INSERT INTO {table} ({cols}) SELECT {cols} FROM {table}_old;
         DROP TABLE {table}_old;
         COMMIT;"
    );
    conn.execute_batch(&batch)?;
    // 重建后索引随旧表 DROP 丢失,重新建立(幂等)
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_session_memory_session_seq ON session_memory(session_id, seq);
         CREATE INDEX IF NOT EXISTS idx_agent_memory_role ON agent_memory(agent_role, id);",
    )?;
    Ok(())
}
