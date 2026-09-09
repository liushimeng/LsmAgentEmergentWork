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
pub mod pragmas;
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
        // 与 Db::open 走同一工厂:clone 出的连接同样带 WAL / busy_timeout 等 PRAGMA,
        // 否则并行 SubAgent 持 clone 连接写库会立即 database is locked(L1041)
        let conn = open_connection(&self.db_path).unwrap_or_else(|e| {
            panic!(
                "Db::clone: 重新打开数据库 {} 失败: {e}",
                self.db_path.display()
            )
        });
        Self {
            conn: Mutex::new(conn),
            db_path: self.db_path.clone(),
        }
    }
}

/// 打开 + PRAGMA + 完整性检测的三态结果。
///
/// `Db::open` 据此决定:健康 → 直接用;持久损坏 → 隔离重建;瞬态错误 → 上抛
/// (不隔离,防误报毁数据)。
enum CheckOutcome {
    /// 健康连接(含 quick_check 瞬态失败但库本身可用的情形)。
    Healthy(Connection),
    /// 持久性损坏:文件头坏(NOTADB)/ quick_check 报损坏(CORRUPT)。
    Corrupt { reason: String },
    /// 瞬态错误(锁冲突/权限/IO 抖动):上抛给调用方。
    Failed(ConfigError),
}

/// 判断 rusqlite 错误是否为「持久性数据库损坏」(对齐 openclaw sqlite-integrity.ts
/// 的核心区分:持久损坏才修复,瞬态锁冲突不动作)。
///
/// 依据 SQLite 扩展结果码:26 = `SQLITE_NOTADB`(文件不是库,典型文件头损坏),
/// 11 = `SQLITE_CORRUPT`(结构损坏)。锁冲突(BUSY/LOCKED)、权限、IO 短错都不是损坏。
fn is_persistent_corruption(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(fe, _)
            if fe.extended_code == 26 || fe.extended_code == 11
    )
}

/// 打开连接 → 应用 PRAGMA → 启动 checkpoint → quick_check,产出三态结果。
fn try_open_and_check(db_path: &Path) -> CheckOutcome {
    let map_err = |e: rusqlite::Error, stage: &str| ConfigError::Db {
        path: db_path.display().to_string(),
        reason: format!("{stage} 失败: {e}"),
    };
    let conn = match Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            return if is_persistent_corruption(&e) {
                CheckOutcome::Corrupt {
                    reason: format!("打开失败: {e}"),
                }
            } else {
                CheckOutcome::Failed(map_err(e, "打开数据库"))
            };
        }
    };
    match pragmas::apply_pragmas(&conn) {
        Ok(report) => {
            if !report.wal_enabled {
                // 网络文件系统(NFS/SMB/9p)等不支持共享内存 WAL 的场景:降级
                // rollback journal 仍可工作(对齐 openclaw sqlite-wal.ts 回退语义)
                tracing::warn!(
                    journal_mode = %report.journal_mode,
                    "WAL 不可用,已降级 rollback journal(并发写性能受限)"
                );
            }
        }
        Err(e) => {
            return if is_persistent_corruption(&e) {
                CheckOutcome::Corrupt {
                    reason: format!("应用 PRAGMA 失败: {e}"),
                }
            } else {
                CheckOutcome::Failed(map_err(e, "应用 PRAGMA"))
            };
        }
    }
    // 启动时回收上次会话(尤其 kill -9)遗留的 WAL 空间;PASSIVE 失败不阻塞启动
    let _ = pragmas::checkpoint_passive(&conn);
    match pragmas::quick_check(&conn) {
        Ok(true) => CheckOutcome::Healthy(conn),
        Ok(false) => CheckOutcome::Corrupt {
            reason: "quick_check 报告持久性损坏".to_string(),
        },
        // quick_check 没跑成(瞬态):库本身没证据说坏,按旧版行为继续用
        Err(_) => CheckOutcome::Healthy(conn),
    }
}

/// 打开连接并应用生产级 PRAGMA(`Db::clone` 与隔离后重建用)。
///
/// `Db::clone` 经此工厂保证 clone 出去的连接与主连接 PRAGMA **完全一致**——
/// 这是并行 SubAgent 写 `agent_memory` 不再 `database is locked` 的关键
/// (只配主连接没用,clone 是独立连接)。
fn open_connection(db_path: &Path) -> Result<Connection> {
    match try_open_and_check(db_path) {
        CheckOutcome::Healthy(conn) => Ok(conn),
        CheckOutcome::Corrupt { reason } => Err(ConfigError::Db {
            path: db_path.display().to_string(),
            reason: format!("数据库损坏: {reason}"),
        }),
        CheckOutcome::Failed(e) => Err(e),
    }
}

impl Db {
    /// 打开(或创建)数据库,自动建表。
    ///
    /// 打开即做完整性自愈(L1042,对齐 openclaw `sqlite-integrity.ts`):
    /// 持久性损坏(文件头坏 / quick_check 报损坏)→ 把坏库隔离为
    /// `{db}.corrupt-{时间戳}.bak`(连同名 `-wal`/`-shm` 一并搬走,防脏 WAL 干扰
    /// 新库)→ 重建空库继续跑(fail-open,CLI 可用性优先,数据有备份可手动恢复);
    /// 瞬态错误(锁冲突等)→ 不隔离上抛或继续,防 quick_check 误报毁数据。
    pub fn open(paths: &Paths) -> Result<Self> {
        let conn = match try_open_and_check(&paths.db_path) {
            CheckOutcome::Healthy(conn) => conn,
            CheckOutcome::Failed(e) => return Err(e),
            CheckOutcome::Corrupt { reason } => {
                // 真损坏:conn 已在 try_open_and_check 内 drop,可安全 rename
                quarantine_corrupted(&paths.db_path)?;
                eprintln!(
                    "⚠️  数据库文件损坏,已隔离为 .corrupt-*.bak 备份并重建空库: {}\n    {reason}\
                     \n    受影响数据:接入记录(providers)/会话记忆/Agent 记忆。\
                     \n    如需抢救,可在根目录找到对应 .corrupt-*.bak,复制回 {} 后重启。",
                    paths.db_path.display(),
                    paths.db_path.display()
                );
                return Self::open_fresh(&paths.db_path);
            }
        };
        schema::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: paths.db_path.clone(),
        })
    }

    /// 隔离后重建:新文件 + PRAGMA + 建表。
    fn open_fresh(db_path: &Path) -> Result<Self> {
        let conn = open_connection(db_path)?;
        schema::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            db_path: db_path.to_path_buf(),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

/// 把损坏的库文件隔离为 `{db}.corrupt-{YYYYMMDD-HHMMSS}.bak`,连同 `-wal`/`-shm`。
///
/// 时间戳到秒,多次损坏不互相覆盖。文件系统层面 rename 失败时上抛(此时无法
/// 安全继续——新库会与坏库同名互斥)。
fn quarantine_corrupted(db_path: &Path) -> Result<()> {
    let now = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    // 手工拼 YYYYMMDD-HHMMSS,避开 time crate 已 deprecated 的 format_description::parse
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let bak = db_path.with_extension(format!(
        "{}.corrupt-{stamp}.bak",
        db_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
    ));
    std::fs::rename(db_path, &bak).map_err(|e| ConfigError::Db {
        path: db_path.display().to_string(),
        reason: format!("隔离损坏库到 {} 失败: {e}", bak.display()),
    })?;
    for suffix in ["-wal", "-shm"] {
        let side = db_path.with_file_name(format!(
            "{}{suffix}",
            db_path.file_name().and_then(|n| n.to_str()).unwrap_or_default()
        ));
        if side.exists() {
            // 脏 WAL/SHM 跟着坏库走;搬不动就算了(新库有自己的命名空间)
            let _ = std::fs::rename(
                &side,
                side.with_file_name(format!(
                    "{}{suffix}.corrupt-{stamp}.bak",
                    db_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default()
                )),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    /// 读取当前连接的 PRAGMA 值(测试辅助;兼容 TEXT 与 INTEGER 返回)。
    fn pragma_string(db: &Db, name: &str) -> String {
        use rusqlite::types::Value;
        let conn = db.conn.lock().unwrap();
        let v: Value = conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap();
        match v {
            Value::Text(s) => s,
            Value::Integer(i) => i.to_string(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn pragmas_applied_on_open() {
        let (db, _d) = fresh_db();
        // 本地临时目录上 WAL 必然成功;busy_timeout / synchronous 读回验证
        assert_eq!(pragma_string(&db, "busy_timeout"), "5000");
        assert_eq!(pragma_string(&db, "synchronous"), "1", "synchronous 应为 NORMAL(1)");
        let mode = pragma_string(&db, "journal_mode");
        assert!(
            mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("delete"),
            "journal_mode 读回应为 wal(或降级 delete): {mode}"
        );
    }

    #[test]
    fn clone_connection_shares_pragmas() {
        // 防回归:Db::clone 出的连接必须同样带 PRAGMA,否则并行写立即 BUSY(L1041)
        let (db, _d) = fresh_db();
        let cloned = db.clone();
        assert_eq!(pragma_string(&cloned, "busy_timeout"), "5000");
        assert_eq!(pragma_string(&cloned, "synchronous"), "1");
        let mode = pragma_string(&cloned, "journal_mode");
        assert!(
            mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("delete"),
            "clone 连接 journal_mode 读回异常: {mode}"
        );
    }

    #[test]
    fn concurrent_writers_succeed_under_wal() {
        // 模拟并行 SubAgent 各持独立连接并发写 agent_memory(第 05 轮并行调度场景)。
        // WAL + busy_timeout 下写写冲突等待重试;原 rollback + 0 超时下会立即
        // `database is locked` 失败。
        let (db, dir) = fresh_db();
        drop(db);
        let db_path = dir.path().join("LsmAgentEmergentWork.db");

        const THREADS: usize = 8;
        const ROWS_EACH: usize = 50;
        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let path = db_path.clone();
                std::thread::spawn(move || {
                    let conn = open_connection(&path).unwrap();
                    conn.execute_batch(
                        "CREATE TABLE IF NOT EXISTS cw (t INTEGER, i INTEGER);
                         BEGIN IMMEDIATE;",
                    )
                    .unwrap();
                    for i in 0..ROWS_EACH {
                        conn.execute("INSERT INTO cw VALUES (?1, ?2)", [t as i64, i as i64])
                            .unwrap();
                    }
                    conn.execute_batch("COMMIT;").unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().expect("并发写线程不应 panic");
        }
        let conn = open_connection(&db_path).unwrap();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM cw", [], |r| r.get(0)).unwrap();
        assert_eq!(total as usize, THREADS * ROWS_EACH, "所有并发写都应落库");
    }

    #[test]
    fn corrupted_db_quarantined_and_rebuilt() {
        let (db, dir) = fresh_db();
        // 写入一条记录确保库非空
        db.conn.lock().unwrap().execute(
            "INSERT INTO providers (protocol, provider_name, model_name, end_point, api_key, is_active)
             VALUES ('openai', 'p', 'm', 'http://e', 'k', 0)",
            [],
        )
        .unwrap();
        drop(db);
        let db_path = dir.path().join("LsmAgentEmergentWork.db");
        // 覆盖文件头制造持久损坏(合法头 16 字节 "SQLite format 3\0")
        let bytes = std::fs::read(&db_path).unwrap();
        let mut corrupted = bytes.clone();
        for (i, b) in b"GARBAGE!".iter().enumerate() {
            corrupted[i] = *b;
        }
        std::fs::write(&db_path, corrupted).unwrap();

        // 重新 open:应隔离坏库 + 重建空库(fail-open)
        let paths = Paths::for_test(dir.path());
        let db2 = Db::open(&paths).unwrap();
        // 新库可用:providers 可正常 CRUD
        let count: i64 = db2
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM providers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "重建后应为空库");
        drop(db2);
        // 坏库被隔离为 .corrupt-*.bak
        let baks: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".corrupt-")
            })
            .collect();
        assert!(!baks.is_empty(), "应存在 .corrupt-*.bak 隔离备份");
    }

    #[test]
    fn healthy_db_not_quarantined() {
        // 防误伤:健康库反复 open 不产生隔离备份(quick_check 假阳性会毁数据)
        let (db, dir) = fresh_db();
        db.conn
            .lock()
            .unwrap()
            .execute("INSERT INTO providers (protocol, provider_name, model_name, end_point, api_key, is_active) VALUES ('openai', 'p', 'm', 'http://e', 'k', 1)", [])
            .unwrap();
        drop(db);
        let paths = Paths::for_test(dir.path());
        let db2 = Db::open(&paths).unwrap();
        drop(db2);
        let baks: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
            .collect();
        assert!(baks.is_empty(), "健康库不应被隔离");
    }
}
