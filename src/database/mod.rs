//! 数据库模块
//!
//! 负责 SQLite 数据库的所有操作，包括：
//! - 路径解析（`paths`）
//! - 数据模型（`models`）
//! - Schema 初始化（`schema`）
//! - Provider 表的 CRUD 及导入导出（`provider`）

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use thiserror::Error;

pub mod models;
pub mod paths;
pub mod provider;
pub mod schema;

// 重导出常用类型
pub use models::{
    format_context_size, parse_context_size, ExportData, ExportRecord, ImportInput, ImportResult,
    Protocol, ProviderImport, ProviderRecord, DEFAULT_CONTEXT_MAX_SIZE,
};
pub use paths::Paths;

/// 配置/数据库相关错误
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

    #[error("导入失败: {0}")]
    Import(String),

    #[error("导出失败: {0}")]
    Export(String),
}

pub type Result<T> = std::result::Result<T, ConfigError>;

/// SQLite 封装:连接对象内部用 Mutex 包裹。
pub struct Db {
    /// SQLite 连接（Mutex 保护），pub(crate) 供 config 子模块访问
    pub(crate) conn: Mutex<Connection>,
    /// 数据库文件路径，用于 Clone 时重新打开
    pub(crate) db_path: std::path::PathBuf,
}

// 手动实现 Clone:Mutex 不可 Clone,但 Connection 可在同进程内通过 path 复用。
// 这里采用「按路径重新打开」的策略:Clone 后操作的是同一 SQLite 文件(独立 Connection)。
impl Clone for Db {
    fn clone(&self) -> Self {
        // 通过 db_path 重新打开,保留 Mutex 语义
        let conn = Connection::open(&self.db_path).expect("Db::clone: re-open failed");
        Self {
            conn: Mutex::new(conn),
            db_path: self.db_path.clone(),
        }
    }
}

impl Db {
    /// 打开(或创建)数据库,自动建表
    pub fn open(paths: &Paths) -> Result<Self> {
        let conn = Connection::open(&paths.db_path).map_err(|e| ConfigError::Db {
            path: paths.db_path.display().to_string(),
            reason: e.to_string(),
        })?;
        schema::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: paths.db_path.clone(),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}
