//! 配置层:
//! - 解析「根目录」(二进制所在目录) 与「工作目录」(启动目录);
//! - 使用 SQLite(`LsmAgentEmergentWork.db`)持久化大模型接入记录。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use thiserror::Error;

const DB_FILE_NAME: &str = "LsmAgentEmergentWork.db";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("无法解析根目录: {0}")]
    RootDir(String),

    #[error("无法访问数据库文件 {path}: {reason}")]
    Db { path: String, reason: String },

    #[error("数据库操作失败: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("接入记录不存在: {0}")]
    NotFound(i64),

    #[error("无效的协议: {0}（仅支持 anthropic / openai）")]
    InvalidProtocol(String),
}

pub type Result<T> = std::result::Result<T, ConfigError>;

/// LLM 接入协议
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Protocol {
    Anthropic,
    OpenAi,
}

impl Protocol {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Protocol::Anthropic),
            "openai" => Ok(Protocol::OpenAi),
            other => Err(ConfigError::InvalidProtocol(other.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Anthropic => "anthropic",
            Protocol::OpenAi => "openai",
        }
    }
}

/// 一条完整的大模型接入记录
#[derive(Debug, Clone)]
pub struct ProviderRecord {
    pub id: i64,
    pub protocol: Protocol,
    pub provider_name: String,
    pub model_name: String,
    pub end_point: String,
    pub api_key: String,
    pub is_active: bool,
    pub created_at: String,
}

/// 路径上下文:根目录 / 工作目录 / 数据库路径
#[derive(Debug, Clone)]
pub struct Paths {
    pub root_dir: PathBuf,
    pub work_dir: PathBuf,
    pub db_path: PathBuf,
}

impl Paths {
    /// 自动探测根目录与工作目录
    pub fn detect() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|e| ConfigError::RootDir(e.to_string()))?;
        let root_dir = exe
            .parent()
            .ok_or_else(|| ConfigError::RootDir("current_exe 无父目录".into()))?
            .to_path_buf();
        let work_dir = std::env::current_dir().unwrap_or_else(|_| root_dir.clone());
        let db_path = root_dir.join(DB_FILE_NAME);
        Ok(Self { root_dir, work_dir, db_path })
    }

    /// 用于测试:人为指定目录
    pub fn for_test(dir: &Path) -> Self {
        Self {
            root_dir: dir.to_path_buf(),
            work_dir: dir.to_path_buf(),
            db_path: dir.join(DB_FILE_NAME),
        }
    }
}

/// SQLite 封装:连接对象内部用 Mutex 包裹。
pub struct Db {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

impl Db {
    /// 打开(或创建)数据库,自动建表
    pub fn open(paths: &Paths) -> Result<Self> {
        let conn = Connection::open(&paths.db_path).map_err(|e| ConfigError::Db {
            path: paths.db_path.display().to_string(),
            reason: e.to_string(),
        })?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: paths.db_path.clone(),
        })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
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
            "#,
        )?;
        Ok(())
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 新增一条记录;若库为空则自动激活
    pub fn add(
        &self,
        protocol: Protocol,
        provider_name: &str,
        model_name: &str,
        end_point: &str,
        api_key: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM providers", [], |r| r.get::<_, i64>(0))?;
        let activate = count == 0;
        conn.execute(
            "INSERT INTO providers(protocol, provider_name, model_name, end_point, api_key, is_active)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                protocol.as_str(),
                provider_name,
                model_name,
                end_point,
                api_key,
                if activate { 1 } else { 0 }
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }

    pub fn list(&self) -> Result<Vec<ProviderRecord>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, protocol, provider_name, model_name, end_point, api_key, is_active, created_at
             FROM providers ORDER BY id ASC",
        )?;
        let iter = stmt.query_map([], row_to_record)?;
        let mut out = Vec::new();
        for r in iter {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_active(&self) -> Result<Option<ProviderRecord>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row = conn
            .query_row(
                "SELECT id, protocol, provider_name, model_name, end_point, api_key, is_active, created_at
                 FROM providers WHERE is_active = 1 LIMIT 1",
                [],
                row_to_record,
            )
            .optional()?;
        Ok(row)
    }

    pub fn get(&self, id: i64) -> Result<ProviderRecord> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row = conn
            .query_row(
                "SELECT id, protocol, provider_name, model_name, end_point, api_key, is_active, created_at
                 FROM providers WHERE id = ?1",
                params![id],
                row_to_record,
            )
            .optional()?;
        row.ok_or(ConfigError::NotFound(id))
    }

    /// 把指定 id 设为唯一激活;其他全部清零
    pub fn set_active(&self, id: i64) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex poisoned");
        let tx = conn.transaction()?;
        let updated = tx.execute("UPDATE providers SET is_active = 0", [])?;
        let target = tx.execute(
            "UPDATE providers SET is_active = 1 WHERE id = ?1",
            params![id],
        )?;
        if target == 0 {
            return Err(ConfigError::NotFound(id));
        }
        let _ = updated;
        tx.commit()?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let n = conn.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(ConfigError::NotFound(id));
        }
        Ok(())
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderRecord> {
    let proto_str: String = row.get(1)?;
    let protocol = match proto_str.as_str() {
        "anthropic" => Protocol::Anthropic,
        "openai" => Protocol::OpenAi,
        // CHECK 约束保证只会是这两个值
        _ => unreachable!("unknown protocol in db: {proto_str}"),
    };
    let is_active_int: i64 = row.get(6)?;
    Ok(ProviderRecord {
        id: row.get(0)?,
        protocol,
        provider_name: row.get(2)?,
        model_name: row.get(3)?,
        end_point: row.get(4)?,
        api_key: row.get(5)?,
        is_active: is_active_int != 0,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    #[test]
    fn schema_initialized_empty() {
        let (db, _d) = fresh_db();
        assert!(db.list().unwrap().is_empty());
        assert!(db.get_active().unwrap().is_none());
    }

    #[test]
    fn first_add_becomes_active() {
        let (db, _d) = fresh_db();
        let id = db
            .add(Protocol::Anthropic, "p1", "claude-3", "https://x", "k1")
            .unwrap();
        let active = db.get_active().unwrap().unwrap();
        assert_eq!(active.id, id);
        assert_eq!(active.provider_name, "p1");
        assert!(active.is_active);
    }

    #[test]
    fn set_active_is_exclusive() {
        let (db, _d) = fresh_db();
        let a = db.add(Protocol::OpenAi, "p1", "m1", "e1", "k1").unwrap();
        let _b = db.add(Protocol::OpenAi, "p2", "m2", "e2", "k2").unwrap();
        // a 是第一条,被自动激活;切换到 b 后 a 不再激活
        db.set_active(a).unwrap();
        assert!(db.get(a).unwrap().is_active);
        let b_id = db.list().unwrap().into_iter().find(|r| r.provider_name == "p2").unwrap().id;
        db.set_active(b_id).unwrap();
        assert!(!db.get(a).unwrap().is_active);
        assert!(db.get(b_id).unwrap().is_active);
    }

    #[test]
    fn unique_constraint_on_full_record() {
        let (db, _d) = fresh_db();
        db.add(Protocol::OpenAi, "p", "m", "e", "k").unwrap();
        let dup = db.add(Protocol::OpenAi, "p", "m", "e", "k");
        assert!(dup.is_err());
    }

    #[test]
    fn delete_unknown_id_errors() {
        let (db, _d) = fresh_db();
        assert!(matches!(db.delete(999), Err(ConfigError::NotFound(999))));
    }

    #[test]
    fn invalid_protocol_rejected() {
        assert!(matches!(Protocol::parse("foo"), Err(ConfigError::InvalidProtocol(_))));
        assert_eq!(Protocol::parse("Anthropic").unwrap(), Protocol::Anthropic);
        assert_eq!(Protocol::parse("OPENAI").unwrap(), Protocol::OpenAi);
    }
}
