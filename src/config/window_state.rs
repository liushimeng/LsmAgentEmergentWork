//! window_state 表 DAO —— WindowUse Agent 的窗口会话状态持久化。
//!
//! 每 session_id 一条记录,覆盖更新,存储 WindowSessionState 的 JSON 序列化。
//! 设计见 `docs/WindowUse多轮对话与Agent间通信增强设计/01-设计与解决方案.md` §3.2。

use rusqlite::{params, OptionalExtension};

use crate::agent::window_state::WindowSessionState;
use crate::config::Result;

use super::Db;

/// 建表 SQL(由 `Db::initialize` 调用)。
pub const CREATE_TABLE_SQL: &str = "
CREATE TABLE IF NOT EXISTS window_state (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT NOT NULL UNIQUE,
    state_json      TEXT NOT NULL,
    version         INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now','localtime')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);
CREATE INDEX IF NOT EXISTS idx_window_state_session ON window_state(session_id);
";

impl Db {
    /// 保存窗口状态(覆盖写入)。
    pub fn save_window_state(&self, session_id: &str, state: &WindowSessionState) -> Result<()> {
        let json = state
            .to_json()
            .map_err(|e| crate::config::ConfigError::Serialization(e.to_string()))?;
        let now = crate::session::now_readable();

        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO window_state (session_id, state_json, version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(session_id) DO UPDATE SET
               state_json = excluded.state_json,
               version = excluded.version,
               updated_at = ?4",
            params![session_id, json, state.version as i64, now],
        )?;
        Ok(())
    }

    /// 加载窗口状态(不存在返回 None)。
    pub fn load_window_state(&self, session_id: &str) -> Result<Option<WindowSessionState>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row: Option<String> = conn
            .query_row(
                "SELECT state_json FROM window_state WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?;

        match row {
            Some(json) => {
                let state = WindowSessionState::from_json(&json)
                    .map_err(|e| crate::config::ConfigError::Serialization(e.to_string()))?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    /// 删除指定 session 的窗口状态(Session 结束时可选调用)。
    pub fn delete_window_state(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "DELETE FROM window_state WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::window_state::WindowSessionState;
    use crate::config::Paths;
    use tempfile::tempdir;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    #[test]
    fn save_and_load_roundtrip() {
        let (db, _d) = fresh_db();
        let mut state = WindowSessionState::new("s1");
        state.record_action("w1", "记事本", "notepad.exe", 1234, "/0/1", "click", "ok");
        state.set_alias("我的记事本", "w1");

        db.save_window_state("s1", &state).unwrap();
        let loaded = db.load_window_state("s1").unwrap().expect("应加载到状态");
        assert_eq!(loaded.session_id, "s1");
        assert_eq!(loaded.action_history.len(), 1);
        assert_eq!(loaded.window_aliases.get("我的记事本").unwrap(), "w1");
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let (db, _d) = fresh_db();
        let loaded = db.load_window_state("no-such-session").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn save_overwrites_existing() {
        let (db, _d) = fresh_db();
        let mut s1 = WindowSessionState::new("s1");
        s1.record_action("w1", "A", "a.exe", 1, "/0", "click", "ok");
        db.save_window_state("s1", &s1).unwrap();

        let mut s2 = WindowSessionState::new("s1");
        s2.record_action("w2", "B", "b.exe", 2, "/0", "set_text", "hi");
        s2.version = 5;
        db.save_window_state("s1", &s2).unwrap();

        let loaded = db.load_window_state("s1").unwrap().unwrap();
        // 应是新状态
        assert_eq!(loaded.action_history[0].window_title, "B");
        assert_eq!(loaded.version, 5);
    }

    #[test]
    fn delete_removes_state() {
        let (db, _d) = fresh_db();
        let state = WindowSessionState::new("s1");
        db.save_window_state("s1", &state).unwrap();
        assert!(db.load_window_state("s1").unwrap().is_some());

        db.delete_window_state("s1").unwrap();
        assert!(db.load_window_state("s1").unwrap().is_none());
    }
}
