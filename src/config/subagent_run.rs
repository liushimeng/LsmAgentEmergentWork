//! `subagent_run` 表 DAO:动态子 Agent 运行记录(第 115 轮,2026-09-22)。
//!
//! 设计见 `docs/自感知SubAgent自定义类型与运行持久化/01-设计与解决方案.md` §5。
//! 表结构见 `database/schema.rs`(与 `session_memory` / `agent_memory` 同库同风格)。
//!
//! **职责边界**:
//! - 本模块只管**存取**,不做治理判断(预算/深度/策略在 `agent::dynamic_subagent`);
//! - 写入由调用方 fail-open 包裹(持久化是增强能力,绝不能让任务失败)。

use rusqlite::{params, OptionalExtension};

use crate::config::Result;

/// 运行状态字面量(与 `SubAgentReport.status` 对齐,外加落库专属的 `running` / `orphaned`)。
pub const STATUS_RUNNING: &str = "running";
/// 进程被终止后残留的作业(启动期标记)。
pub const STATUS_ORPHANED: &str = "orphaned";

/// 一次运行的入库条目(开始时 status=running,结束时覆盖为终态)。
#[derive(Debug, Clone, Default)]
pub struct SubAgentRunEntry {
    pub run_id: String,
    pub session_id: String,
    pub parent: String,
    pub name: String,
    pub agent_type: String,
    pub depth: usize,
    pub status: String,
    pub task: String,
    pub report: String,
    pub error: Option<String>,
    pub tools: Vec<String>,
    pub dropped_tools: Vec<String>,
    pub iterations: usize,
    pub tool_calls: usize,
    pub wallclock_ms: u64,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// `launch` / `background` / `batch` / `resume`
    pub origin: String,
    /// 血缘:本次续跑自哪个 run_id
    pub resumed_from: Option<String>,
}

/// 读出的行(带 `created_at`)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubAgentRunRow {
    pub run_id: String,
    pub session_id: String,
    pub parent: String,
    pub name: String,
    pub agent_type: String,
    pub depth: usize,
    pub status: String,
    pub task: String,
    pub report: String,
    pub error: Option<String>,
    pub tools: Vec<String>,
    pub dropped_tools: Vec<String>,
    pub iterations: usize,
    pub tool_calls: usize,
    pub wallclock_ms: u64,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub origin: String,
    pub resumed_from: Option<String>,
    pub created_at: String,
}

/// 查询过滤条件(`history` action 与 TUI 面板共用)。
#[derive(Debug, Clone, Default)]
pub struct RunQuery {
    /// `None` = 跨会话。
    pub session_id: Option<String>,
    /// `None` = 全部类型。
    pub agent_type: Option<String>,
    /// 条数上限(1..=50;越界由调用方 clamp)。
    pub limit: usize,
}

fn parse_tools(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

use super::Db;

impl Db {
    /// 写入一条「开始」记录(status=running)。
    ///
    /// 同一 `run_id` 重复写入用 `INSERT OR REPLACE`:重试语义幂等,不会因残留行报错。
    pub fn insert_subagent_run(&self, e: &SubAgentRunEntry) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT OR REPLACE INTO subagent_run
                (run_id, session_id, parent, name, agent_type, depth, status, task, report, error,
                 tools, dropped_tools, iterations, tool_calls, wallclock_ms,
                 input_tokens, output_tokens, origin, resumed_from)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                &e.run_id,
                &e.session_id,
                &e.parent,
                &e.name,
                &e.agent_type,
                e.depth as i64,
                &e.status,
                &e.task,
                &e.report,
                e.error.as_deref(),
                serde_json::to_string(&e.tools).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&e.dropped_tools).unwrap_or_else(|_| "[]".into()),
                e.iterations as i64,
                e.tool_calls as i64,
                e.wallclock_ms as i64,
                e.input_tokens as i64,
                e.output_tokens as i64,
                &e.origin,
                e.resumed_from.as_deref(),
            ],
        )?;
        Ok(())
    }

    /// 覆盖更新终态(状态 / 产物 / 用量 / 工具面 / 耗时)。
    ///
    /// 行不存在时(例如持久化中途被关闭)静默无操作 —— 调用方 fail-open。
    pub fn finish_subagent_run(&self, e: &SubAgentRunEntry) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        conn.execute(
            "UPDATE subagent_run SET
                status = ?2, report = ?3, error = ?4, tools = ?5, dropped_tools = ?6,
                iterations = ?7, tool_calls = ?8, wallclock_ms = ?9,
                input_tokens = ?10, output_tokens = ?11
             WHERE run_id = ?1",
            params![
                &e.run_id,
                &e.status,
                &e.report,
                e.error.as_deref(),
                serde_json::to_string(&e.tools).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&e.dropped_tools).unwrap_or_else(|_| "[]".into()),
                e.iterations as i64,
                e.tool_calls as i64,
                e.wallclock_ms as i64,
                e.input_tokens as i64,
                e.output_tokens as i64,
            ],
        )?;
        Ok(())
    }

    /// 按条件查询(时间倒序)。
    pub fn list_subagent_runs(&self, q: &RunQuery) -> Result<Vec<SubAgentRunRow>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let limit = q.limit.clamp(1, 50) as i64;
        let mut stmt = conn.prepare(
            "SELECT run_id, session_id, parent, name, agent_type, depth, status, task, report,
                    error, tools, dropped_tools, iterations, tool_calls, wallclock_ms,
                    input_tokens, output_tokens, origin, resumed_from, created_at
             FROM subagent_run
             WHERE (?1 IS NULL OR session_id = ?1)
               AND (?2 IS NULL OR agent_type = ?2)
             ORDER BY created_at DESC, rowid DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![q.session_id.as_deref(), q.agent_type.as_deref(), limit],
            row_to_run,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 取单条(不存在返回 `None`)。
    pub fn get_subagent_run(&self, run_id: &str) -> Result<Option<SubAgentRunRow>> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let row = conn
            .query_row(
                "SELECT run_id, session_id, parent, name, agent_type, depth, status, task, report,
                        error, tools, dropped_tools, iterations, tool_calls, wallclock_ms,
                        input_tokens, output_tokens, origin, resumed_from, created_at
                 FROM subagent_run WHERE run_id = ?1",
                params![run_id],
                row_to_run,
            )
            .optional()?;
        Ok(row)
    }

    /// 统计指定会话(或全部)的条数。
    pub fn count_subagent_runs(&self, session_id: Option<&str>) -> Result<usize> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM subagent_run WHERE (?1 IS NULL OR session_id = ?1)",
            params![session_id],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as usize)
    }

    /// 统计孤儿作业数(供 TUI 提示)。
    pub fn count_subagent_orphans(&self, session_id: Option<&str>) -> Result<usize> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM subagent_run
             WHERE status = ?2 AND (?1 IS NULL OR session_id = ?1)",
            params![session_id, STATUS_ORPHANED],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as usize)
    }

    /// 启动期孤儿标记:把**非当前会话**残留的 `running` 行改判为 `orphaned`
    /// (父进程已消失;对齐 openclaw Orphan Recovery 的最小可用子集)。
    ///
    /// 返回本次标记的行数。保留当前会话的 `running` 行 —— 同一 DB 上可能还有另一个
    /// 并行运行的 laew 实例,它的活作业不该被误判。
    pub fn mark_subagent_orphans(&self, current_session: &str) -> Result<usize> {
        let conn = self.conn.lock().expect("db mutex poisoned");
        let n = conn.execute(
            "UPDATE subagent_run SET status = ?2, error = COALESCE(error, '父进程已退出,作业未完成')
             WHERE status = ?1 AND session_id != ?3",
            params![STATUS_RUNNING, STATUS_ORPHANED, current_session],
        )?;
        Ok(n)
    }

    /// 保留最新 `keep` 行,删除更旧行(启动期调用;`keep == 0` 表示不清理)。
    pub fn trim_subagent_runs(&self, keep: usize) -> Result<usize> {
        if keep == 0 {
            return Ok(0);
        }
        let conn = self.conn.lock().expect("db mutex poisoned");
        let n = conn.execute(
            "DELETE FROM subagent_run WHERE rowid NOT IN (
                SELECT rowid FROM subagent_run ORDER BY created_at DESC, rowid DESC LIMIT ?1
             )",
            params![keep as i64],
        )?;
        Ok(n)
    }
}

fn row_to_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<SubAgentRunRow> {
    let tools: String = r.get(10)?;
    let dropped: String = r.get(11)?;
    Ok(SubAgentRunRow {
        run_id: r.get(0)?,
        session_id: r.get(1)?,
        parent: r.get(2)?,
        name: r.get(3)?,
        agent_type: r.get(4)?,
        depth: r.get::<_, i64>(5)? as usize,
        status: r.get(6)?,
        task: r.get(7)?,
        report: r.get(8)?,
        error: r.get(9)?,
        tools: parse_tools(&tools),
        dropped_tools: parse_tools(&dropped),
        iterations: r.get::<_, i64>(12)? as usize,
        tool_calls: r.get::<_, i64>(13)? as usize,
        wallclock_ms: r.get::<_, i64>(14)? as u64,
        input_tokens: r.get::<_, i64>(15)? as u32,
        output_tokens: r.get::<_, i64>(16)? as u32,
        origin: r.get(17)?,
        resumed_from: r.get(18)?,
        created_at: r.get(19)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Paths;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    fn entry(run_id: &str, session: &str, status: &str, origin: &str) -> SubAgentRunEntry {
        SubAgentRunEntry {
            run_id: run_id.to_string(),
            session_id: session.to_string(),
            parent: "LsmAgentEmergentWork-SubAgent-Work".into(),
            name: format!("LsmAgentEmergentWork-SubAgent-Explore-{run_id}"),
            agent_type: "explore".into(),
            depth: 1,
            status: status.into(),
            task: "统计 Cargo.toml 行数".into(),
            report: "结论:77 行".into(),
            error: None,
            tools: vec!["Read".into(), "Glob".into()],
            dropped_tools: vec!["Bash(超出父 Agent 能力上限)".into()],
            iterations: 2,
            tool_calls: 1,
            wallclock_ms: 1200,
            input_tokens: 11,
            output_tokens: 7,
            origin: origin.into(),
            resumed_from: None,
        }
    }

    #[test]
    fn insert_then_finish_updates_row() {
        let (db, _g) = fresh_db();
        db.insert_subagent_run(&entry("sa-1", "s1", STATUS_RUNNING, "launch"))
            .unwrap();
        let row = db.get_subagent_run("sa-1").unwrap().expect("行应存在");
        assert_eq!(row.status, "running");
        assert_eq!(row.tools, vec!["Read", "Glob"]);
        assert_eq!(row.depth, 1);
        assert!(!row.created_at.is_empty(), "created_at 应自动填充");

        let mut fin = entry("sa-1", "s1", "ok", "launch");
        fin.report = "结论:77 行(已复核)".into();
        fin.input_tokens = 30;
        db.finish_subagent_run(&fin).unwrap();
        let row = db.get_subagent_run("sa-1").unwrap().unwrap();
        assert_eq!(row.status, "ok");
        assert_eq!(row.report, "结论:77 行(已复核)");
        assert_eq!(row.input_tokens, 30);
        assert_eq!(row.dropped_tools.len(), 1);
    }

    #[test]
    fn list_filters_by_session_type_and_limit() {
        let (db, _g) = fresh_db();
        db.insert_subagent_run(&entry("sa-1", "s1", "ok", "launch")).unwrap();
        let mut b = entry("sa-2", "s1", "ok", "batch");
        b.agent_type = "plan".into();
        db.insert_subagent_run(&b).unwrap();
        db.insert_subagent_run(&entry("sa-3", "s2", "ok", "launch")).unwrap();

        let all = db
            .list_subagent_runs(&RunQuery {
                session_id: None,
                agent_type: None,
                limit: 10,
            })
            .unwrap();
        assert_eq!(all.len(), 3);

        let s1 = db
            .list_subagent_runs(&RunQuery {
                session_id: Some("s1".into()),
                agent_type: None,
                limit: 10,
            })
            .unwrap();
        assert_eq!(s1.len(), 2);

        let plans = db
            .list_subagent_runs(&RunQuery {
                session_id: None,
                agent_type: Some("plan".into()),
                limit: 10,
            })
            .unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].run_id, "sa-2");

        let one = db
            .list_subagent_runs(&RunQuery {
                session_id: None,
                agent_type: None,
                limit: 1,
            })
            .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(db.count_subagent_runs(None).unwrap(), 3);
        assert_eq!(db.count_subagent_runs(Some("s1")).unwrap(), 2);
    }

    #[test]
    fn insert_is_idempotent_on_same_run_id() {
        let (db, _g) = fresh_db();
        db.insert_subagent_run(&entry("sa-dup", "s1", STATUS_RUNNING, "launch"))
            .unwrap();
        db.insert_subagent_run(&entry("sa-dup", "s1", STATUS_RUNNING, "launch"))
            .unwrap();
        assert_eq!(db.count_subagent_runs(None).unwrap(), 1);
    }

    #[test]
    fn finish_on_missing_row_is_noop() {
        let (db, _g) = fresh_db();
        db.finish_subagent_run(&entry("sa-none", "s1", "ok", "launch"))
            .unwrap();
        assert_eq!(db.count_subagent_runs(None).unwrap(), 0);
    }

    #[test]
    fn orphan_marking_skips_current_session() {
        let (db, _g) = fresh_db();
        db.insert_subagent_run(&entry("sa-live", "mine", STATUS_RUNNING, "background"))
            .unwrap();
        db.insert_subagent_run(&entry("sa-dead", "other", STATUS_RUNNING, "background"))
            .unwrap();
        db.insert_subagent_run(&entry("sa-done", "other", "ok", "launch"))
            .unwrap();

        let n = db.mark_subagent_orphans("mine").unwrap();
        assert_eq!(n, 1, "只标记非当前会话的 running 行");
        assert_eq!(db.get_subagent_run("sa-live").unwrap().unwrap().status, "running");
        let dead = db.get_subagent_run("sa-dead").unwrap().unwrap();
        assert_eq!(dead.status, STATUS_ORPHANED);
        assert!(dead.error.is_some(), "孤儿应带解释性错误文本");
        assert_eq!(db.get_subagent_run("sa-done").unwrap().unwrap().status, "ok");
        assert_eq!(db.count_subagent_orphans(None).unwrap(), 1);
    }

    #[test]
    fn trim_keeps_newest_rows() {
        let (db, _g) = fresh_db();
        for i in 0..5 {
            let mut e = entry(&format!("sa-{i}"), "s1", "ok", "launch");
            e.wallclock_ms = i as u64;
            db.insert_subagent_run(&e).unwrap();
        }
        // created_at 精度为秒,同一秒内靠 rowid DESC 兜底排序
        let removed = db.trim_subagent_runs(2).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(db.count_subagent_runs(None).unwrap(), 2);
        let rows = db
            .list_subagent_runs(&RunQuery {
                session_id: None,
                agent_type: None,
                limit: 10,
            })
            .unwrap();
        assert!(rows.iter().any(|r| r.run_id == "sa-4"), "最新行必须保留");
        assert_eq!(db.trim_subagent_runs(0).unwrap(), 0, "0 = 不清理");
    }
}
