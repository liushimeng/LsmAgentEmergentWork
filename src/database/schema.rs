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

        -- ========== WorkFlow Agent(第 10 角色)新表 ==========

        -- Goal 状态机表
        CREATE TABLE IF NOT EXISTS goals (
            id              TEXT PRIMARY KEY,
            session_id      TEXT NOT NULL,
            parent_id       TEXT,
            title           TEXT NOT NULL,
            description     TEXT NOT NULL,
            state           TEXT NOT NULL DEFAULT 'pending'
                                CHECK(state IN ('pending','pursuing','blocked','paused','satisfied','failed')),
            priority        INTEGER NOT NULL DEFAULT 5,
            phase_id        TEXT,
            acceptance_criteria TEXT,
            retry_count     INTEGER DEFAULT 0,
            max_retries     INTEGER DEFAULT 3,
            error_text      TEXT,
            metadata        TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            completed_at    TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_goals_session ON goals(session_id, state);
        CREATE INDEX IF NOT EXISTS idx_goals_parent ON goals(parent_id);

        -- Squad 表
        CREATE TABLE IF NOT EXISTS squads (
            id              TEXT PRIMARY KEY,
            goal_id         TEXT NOT NULL,
            phase_id        TEXT NOT NULL,
            strategy        TEXT NOT NULL DEFAULT 'all_must_pass',
            max_concurrent  INTEGER NOT NULL DEFAULT 5,
            status          TEXT NOT NULL DEFAULT 'pending'
                                CHECK(status IN ('pending','running','completed','failed','cancelled')),
            created_at      TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            completed_at    TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_squads_goal ON squads(goal_id);

        -- Squad 成员表
        CREATE TABLE IF NOT EXISTS squad_members (
            id              TEXT PRIMARY KEY,
            squad_id        TEXT NOT NULL,
            member_role     TEXT NOT NULL DEFAULT 'worker',
            task            TEXT NOT NULL,
            expected_output TEXT,
            actual_output   TEXT,
            status          TEXT NOT NULL DEFAULT 'pending'
                                CHECK(status IN ('pending','running','completed','failed','skipped')),
            retry_count     INTEGER DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            completed_at    TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_squad_members_squad ON squad_members(squad_id);

        -- WorkFlow 模板表
        CREATE TABLE IF NOT EXISTS workflow_templates (
            id              TEXT PRIMARY KEY,
            name            TEXT NOT NULL UNIQUE,
            description     TEXT NOT NULL,
            category        TEXT NOT NULL,
            definition      TEXT NOT NULL,
            tags            TEXT,
            usage_count     INTEGER DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now','localtime'))
        );

        -- ========== 会话持久化(第 96 轮,2026-09-19) ==========
        -- 设计见 tmpPlan/2026-09-19_02-会话持久化与跨进程恢复方案.md:
        -- SQLite 单一后端(openclaw §3.4 派)替代知识库建议的 JSONL 双后端 —— WAL/事务
        -- 原子性天然免除 JSONL 撕裂修复;整快照重写使 rewind/fork/switch/resume 全部
        -- mutation 路径自动一致。chat_turns.context_response 与 response 分列存储,
        -- 对应第 30 轮「上下文回填版与人类版分流」语义。

        -- 会话索引行(一个 TUI 会话一行,updated_at 为列表排序键)
        CREATE TABLE IF NOT EXISTS chat_sessions (
            session_id   TEXT PRIMARY KEY,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,
            turn_count   INTEGER NOT NULL DEFAULT 0,
            work_dir     TEXT NOT NULL DEFAULT '',
            model_name   TEXT,
            title        TEXT NOT NULL DEFAULT '',
            total_input  INTEGER NOT NULL DEFAULT 0,
            total_output INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_chat_sessions_updated
            ON chat_sessions(updated_at DESC);

        -- 对话轮次(整快照重写:每轮收口后事务内 DELETE 全量 + INSERT 全量)
        CREATE TABLE IF NOT EXISTS chat_turns (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id       TEXT NOT NULL,
            seq              INTEGER NOT NULL,
            ts               TEXT NOT NULL,
            raw_input        TEXT NOT NULL,
            prompt           TEXT NOT NULL,
            response         TEXT NOT NULL,
            context_response TEXT NOT NULL,
            outcome          TEXT NOT NULL CHECK(outcome IN ('direct','executed','failed','error','cancelled')),
            input_tokens     INTEGER NOT NULL DEFAULT 0,
            output_tokens    INTEGER NOT NULL DEFAULT 0,
            cache_read       INTEGER NOT NULL DEFAULT 0,
            cache_creation   INTEGER NOT NULL DEFAULT 0,
            cost_usd         REAL,
            created_at       TEXT NOT NULL DEFAULT (datetime('now','localtime')),
            UNIQUE(session_id, seq)
        );
        CREATE INDEX IF NOT EXISTS idx_chat_turns_session
            ON chat_turns(session_id, seq);

        -- ========== Agent 间消息表 ==========

        -- Agent 间消息表(SubAgent ↔ WorkFlow/squad 等)
        CREATE TABLE IF NOT EXISTS agent_messages (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            msg_id          TEXT NOT NULL UNIQUE,
            session_id      TEXT NOT NULL,
            from_role       TEXT NOT NULL,
            to_role         TEXT NOT NULL,
            payload_json    TEXT NOT NULL,
            consumed        INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT NOT NULL DEFAULT (datetime('now','localtime'))
        );
        CREATE INDEX IF NOT EXISTS idx_agent_messages_session ON agent_messages(session_id, to_role, consumed);
        "#,
    )?;
    migrate(conn)?;
    Ok(())
}

/// 幂等迁移:
/// 1. providers 表补 `context_max_size` 列(ADD COLUMN 带常量 DEFAULT,存量行自动回填);
/// 2. providers 表补 `allow_private_endpoint` 列(默认 0/false,存量记录保持原 SSRF fail-closed 行为);
/// 3. session_memory / agent_memory 的 role CHECK 约束扩展 'compact'/'debug'/'session'
///    (SQLite 不支持修改 CHECK,采用表重建;仅当旧 CHECK 不含 'compact' 时执行)。
pub fn migrate(conn: &Connection) -> Result<()> {
    migrate_providers_context_max_size(conn)?;
    migrate_providers_allow_private_endpoint(conn)?;
    rebuild_role_check(conn, "session_memory")?;
    rebuild_role_check(conn, "agent_memory")?;
    Ok(())
}

/// providers 表补 allow_private_endpoint 列(默认 0/false,存量记录保持原 SSRF fail-closed 行为)。
///
/// 用于本地 Ollama / LMStudio / mock LLM 用户显式放行 loopback / 私网 endpoint,
/// 避免全局 `LAEW_ALLOW_PRIVATE_ENDPOINT=1` 一刀切(第 72 轮 per-provider 支持)。
fn migrate_providers_allow_private_endpoint(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(providers)")?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if cols.iter().any(|c| c == "allow_private_endpoint") {
        return Ok(());
    }
    conn.execute(
        "ALTER TABLE providers ADD COLUMN allow_private_endpoint INTEGER NOT NULL DEFAULT 0",
        [],
    )?;
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
