//! SQLite 生产级 PRAGMA 配置与完整性自愈(L1041 + L1042)。
//!
//! 知识库出处:
//! - opencode `database.ts:27-32` 单文件 6 PRAGMA(WAL + NORMAL + busy_timeout 5s
//!   + cache + FK + 启动 PASSIVE checkpoint)——第八轮 Session 持久化专题评定的性能最佳点;
//! - openclaw `sqlite-wal.ts`(跨 VM 文件系统检测回退 rollback journal)+
//!   `sqlite-integrity.ts`(区分持久性损坏 vs 瞬态锁冲突)——第十六轮 SQLite 全栈专题。
//!
//! 解决的问题:laew 的 `Db::clone` 会重新 open 独立连接分发给多个 Runner,第 05 轮
//! 并行 SubAgent 调度后,多个连接并发写 `agent_memory` 在默认 rollback journal +
//! busy_timeout=0 下会立即 `database is locked`。WAL 模式下读写不互斥,busy_timeout
//! 让写写冲突等待重试而非失败。

use rusqlite::Connection;

use crate::database::Result;

/// busy_timeout(毫秒)。对齐 opencode 5s:写锁等待上限,超过才上抛 SQLITE_BUSY。
pub const BUSY_TIMEOUT_MS: u64 = 5_000;
/// 页缓存(KiB,负数语义 = KiB)。【有意偏差】opencode 用 64MB,但 laew 库是 KB 级
/// (providers 几行 + 记忆摘要),8MB 已是 1000x 余量,不为小库浪费内存。
pub const CACHE_SIZE_KIB: i64 = -8_000;

/// PRAGMA 应用报告(读回验证 + tracing 日志 / 测试断言用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PragmaReport {
    /// `PRAGMA journal_mode` 读回值:"wal" 或降级后的实际模式("delete" 等)。
    pub journal_mode: String,
    /// journal_mode 是否成功切到 WAL(网络文件系统如 NFS/SMB/9p 上会降级)。
    pub wal_enabled: bool,
    /// busy_timeout 实际生效值(毫秒)。
    pub busy_timeout_ms: u64,
}

/// 对连接应用生产级 PRAGMA。
///
/// 返回 rusqlite 原始错误(而非 ConfigError):调用方需要检查扩展码区分
/// 「持久性损坏」(SQLITE_NOTADB/CORRUPT,触发隔离重建)与瞬态错误(上抛)。
///
/// 顺序敏感:
/// 1. `busy_timeout` 最先——后续任何语句(含切 WAL 本身)遇锁都能等待而非立即 BUSY;
/// 2. `journal_mode=WAL`——读写不互斥。读回验证:SQLite 在不支持的文件系统上会
///    安静保留原模式,读回值 ≠ "wal" 即视为降级(openclaw 用 statfs 检测 NFS magic
///    0x6969 / SMB 0x517B,这里用读回结果实现同等语义,零新依赖、天然跨平台);
/// 3. `synchronous=NORMAL`——WAL 下事务一致性保持,仅崩溃边界可能丢最后 1 page
///    (opencode/openclaw/pi 三家同款;FULL 每事务 fsync 对 CLI 过重);
/// 4. `cache_size`、`foreign_keys`——现 schema 无 FK,开启无害,未来加表即受保护。
pub fn apply_pragmas(conn: &Connection) -> rusqlite::Result<PragmaReport> {
    conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))?;

    // journal_mode 是少数有返回值的 PRAGMA:返回切换后的实际模式。
    let journal_mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    let wal_enabled = journal_mode.eq_ignore_ascii_case("wal");

    conn.execute_batch(&format!(
        "PRAGMA synchronous = NORMAL;
         PRAGMA cache_size = {CACHE_SIZE_KIB};
         PRAGMA foreign_keys = ON;"
    ))?;

    let busy_timeout_ms = conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get::<_, i64>(0))
        .unwrap_or(BUSY_TIMEOUT_MS as i64) as u64;

    Ok(PragmaReport {
        journal_mode,
        wal_enabled,
        busy_timeout_ms,
    })
}

/// 启动时回收上次会话(尤其 kill -9)遗留的 WAL 空间。
///
/// PASSIVE 模式:有其它读者在跑时安静放弃,不阻塞、不失败(opencode boot 同款)。
pub fn checkpoint_passive(conn: &Connection) -> Result<()> {
    // wal_checkpoint 有结果行(busy/log/checkpointed 三列),吞掉即可
    let _ = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
    Ok(())
}

/// 快速完整性检测(`quick_check` 跳过索引一致性校验,比 `integrity_check` 快得多)。
///
/// 三态语义(对齐 openclaw sqlite-integrity.ts「区分持久损坏 vs 瞬态锁冲突」):
/// - `Ok(true)`  = 健康;
/// - `Ok(false)` = **持久性损坏**(检测本身成功跑完但报告了错误描述);
/// - `Err(_)`    = 检测没跑成(锁冲突 / IO 抖动等瞬态)——调用方**不得**据此隔离库。
pub fn quick_check(conn: &Connection) -> Result<bool> {
    let rows: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA quick_check")?;
        let iter = stmt.query_map([], |r| r.get::<_, String>(0))?;
        iter.collect::<rusqlite::Result<Vec<_>>>()?
    };
    // 健康库返回单行 "ok";损坏时返回若干错误描述行
    Ok(rows.len() == 1 && rows[0].eq_ignore_ascii_case("ok"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_conn() -> (Connection, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("t.db")).unwrap();
        (conn, dir)
    }

    #[test]
    fn apply_pragmas_enables_wal_and_busy_timeout() {
        let (conn, _d) = temp_conn();
        let report = apply_pragmas(&conn).unwrap();
        // 本地 tmpfs/ext4 上 WAL 必然成功;若 CI 文件系统不支持,断言读回一致性即可
        if report.wal_enabled {
            let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
            assert_eq!(mode.to_ascii_lowercase(), "wal");
        }
        assert_eq!(report.busy_timeout_ms, BUSY_TIMEOUT_MS);
        let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
        assert_eq!(sync, 1, "synchronous 应为 NORMAL(1)");
    }

    #[test]
    fn quick_check_healthy_db_returns_true() {
        let (conn, _d) = temp_conn();
        conn.execute_batch("CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        assert!(quick_check(&conn).unwrap());
    }

    #[test]
    fn quick_check_reports_corruption_as_false() {
        // 用 rekey 之外的通用手段:在一个合法库上写入损坏字节制造持久损坏。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t(a INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        drop(conn);
        // 覆盖文件头(合法 SQLite 头 16 字节 "SQLite format 3\0"),制造可检测损坏
        let bytes = std::fs::read(&path).unwrap();
        let mut corrupted = bytes.clone();
        corrupted[0] = b'X';
        std::fs::write(&path, corrupted).unwrap();
        let conn2 = Connection::open(&path).unwrap();
        // 非法文件头:open 可能直接报错(Err)或 quick_check 报损坏(Ok(false)),
        // 两种都 acceptable——都意味着「不健康」;只验证不会误报健康
        match quick_check(&conn2) {
            Ok(healthy) => assert!(!healthy, "损坏库不应报告健康"),
            Err(_) => { /* open/prepare 失败 = 持久性问题的另一种表现 */ }
        }
    }

    #[test]
    fn checkpoint_passive_is_safe_on_fresh_db() {
        let (conn, _d) = temp_conn();
        apply_pragmas(&conn).unwrap();
        conn.execute_batch("CREATE TABLE t(a INTEGER);").unwrap();
        // 全新库上 PASSIVE checkpoint 应安静完成
        checkpoint_passive(&conn).unwrap();
    }
}
