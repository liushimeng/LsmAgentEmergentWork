//! 会话持久化与跨进程恢复(第 96 轮,2026-09-19)。
//!
//! 知识库依据:`专题/专题-第八轮-Session持久化与崩溃恢复深度对比.md` §7 laew 借鉴路线
//! (P0 每 turn 落盘 + P1 SQLite 索引 /resume)。**选型刻意偏离该路线的 JSONL 双后端**,
//! 改用 openclaw §3.4 的 SQLite 单一后端:laew SQLite 层已具备 WAL + busy_timeout +
//! quick_check 隔离自愈(L1041/L1042),事务原子性天然免除 JSONL 撕裂修复整套机制。
//!
//! 写入策略为**整快照重写**:每轮任务收口后在同一事务内 DELETE 该会话全部轮次 →
//! INSERT 全量 → UPSERT 会话行 →(新会话时)超额淘汰。`/rewind` 截断、`/fork` 换 id
//! 拷贝、`/switch` 旧 id 恢复、`/resume` 跨进程载入全部 mutation 路径自动一致,零补洞特例。
//!
//! 恢复语义:轮次 `prompt` 是 @ 提及/自定义命令展开后的版本(忠实还原模型当年看到的输入),
//! `context_response` 是 LLM 上下文回填版(与人类展示版 `response` 分列,第 30 轮语义)。


use rusqlite::Connection;

use crate::database::Result;
use crate::llm::Usage;

/// 自动保留的会话数上限(按 updated_at 新→旧,超出随新会话落盘同事务淘汰)。
pub const CHAT_SESSIONS_KEEP: usize = 50;

/// 单轮对话的持久化行(数据库形态,与 TUI TranscriptEntry 由调用方互转)。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTurnRow {
    pub seq: i64,
    /// 本轮时间(HH:MM:SS 本地)。
    pub ts: String,
    /// 用户原始输入行。
    pub raw_input: String,
    /// 实际送入编排的提示词(@/命令展开后)。
    pub prompt: String,
    /// assistant 输出人类版(transcript/导出展示)。
    pub response: String,
    /// assistant 输出上下文回填版(恢复重建 context 用)。
    pub context_response: String,
    /// 结局:'direct'|'executed'|'failed'|'error'|'cancelled'。
    pub outcome: String,
    pub usage: Usage,
    /// 本轮成本估算(USD);无价模型/零用量轮为 None。
    pub cost_usd: Option<f64>,
}

/// 会话索引行(`/sessions` 列表项)。
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub turn_count: i64,
    pub work_dir: String,
    pub model_name: Option<String>,
    pub title: String,
    pub total_input: i64,
    pub total_output: i64,
}

/// 恢复规格解析结果(TUI `/resume` 与 CLI `--resume` 共用)。
#[derive(Debug, Clone, PartialEq)]
pub enum ResumeResolution {
    /// 恢复最近一次会话(`latest`)。
    Latest,
    /// 命中列表中的第 idx 项(0 基,新→旧序)。
    Index(usize),
    /// session-id 唯一前缀命中。
    Prefix(usize),
    /// 前缀命中多个,携带候选 id(调用方提示歧义)。
    Ambiguous(Vec<String>),
    NotFound,
}

/// 解析恢复规格:`latest`(或空)→ 最近;纯数字**且在列表范围内** → 列表序号
/// (1 基,新→旧);其余(含越界数字,如日期形态的 id 前缀)→ session-id 唯一前缀。
/// 纯函数,便于单测。
pub fn resolve_chat_session(sessions: &[SessionSummary], spec: &str) -> ResumeResolution {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("latest") || spec == "-" {
        return ResumeResolution::Latest;
    }
    // 纯数字且在范围内 → 列表序号(序号优先)。注意 session-id 以 YYYYMMDD 开头,
    // 日期前缀也是纯数字:越界数字(如 "20260918")不在此返回,落到下方前缀匹配,
    // 让 `/resume 20260918-14` 这类 id 前缀可用。
    if let Ok(n) = spec.parse::<usize>() {
        if n >= 1 && n <= sessions.len() {
            return ResumeResolution::Index(n - 1);
        }
    }
    // 前缀匹配:唯一命中 → Prefix;多命中 → Ambiguous
    let hits: Vec<usize> = sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| s.session_id.starts_with(spec))
        .map(|(i, _)| i)
        .collect();
    match hits.len() {
        1 => ResumeResolution::Prefix(hits[0]),
        0 => ResumeResolution::NotFound,
        _ => ResumeResolution::Ambiguous(
            hits.iter()
                .map(|&i| sessions[i].session_id.clone())
                .collect(),
        ),
    }
}

/// 启动期恢复请求(`--resume` / `-c`):main 在 TUI 启动前写入,TUI 首条用户输入
/// (`handle_user_input` 统一入口)消费一次。经静态量传递而非改 `run_with_debug`
/// 签名,规避对并行任务占用中的 `tui/mod.rs` 的改动(本轮硬约束)。
/// 用 `Mutex<Option>` 而非 OnceLock:恢复必须**只执行一次**,take 语义要求可清空。
static STARTUP_RESUME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 排定启动恢复(spec 语义同 [`resolve_chat_session`])。
pub fn set_startup_resume(spec: String) {
    *STARTUP_RESUME.lock().expect("startup resume") = Some(spec);
}

/// 取走启动恢复请求(只消费一次;未排定/已消费返回 None)。
pub fn take_startup_resume() -> Option<String> {
    STARTUP_RESUME
        .lock()
        .expect("startup resume")
        .take()
}

/// 落盘入口可见性辅助(测试用):确认启动恢复静态量已被排定。
#[cfg(test)]
pub(crate) fn startup_resume_is_set() -> bool {
    STARTUP_RESUME.lock().expect("startup resume").is_some()
}

impl crate::database::Db {
    /// 整快照落盘:一个事务内 UPSERT 会话行 + 重写全部轮次;新会话时顺带超额淘汰。
    ///
    /// `turn_count`/`total_*` 以传入 turns 现算,与重写后的轮次表严格一致;
    /// `title`/`work_dir`/`created_at` 只在首次插入生效(会话身份字段),`model_name`/
    /// `updated_at` 每次刷新。
    pub fn save_chat_snapshot(
        &self,
        session_id: &str,
        created_at: &str,
        work_dir: &str,
        model_name: Option<&str>,
        title: &str,
        turns: &[ChatTurnRow],
    ) -> Result<()> {
        let conn = self.conn.lock().expect("db conn");
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            // 会话行是否已存在(query_row 命中 = 已存在);首落盘(!existed)时才触发超额淘汰
            let existed: bool = conn
                .query_row(
                    "SELECT 1 FROM chat_sessions WHERE session_id = ?1",
                    [session_id],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            let total_input: i64 = turns.iter().map(|t| t.usage.input_tokens as i64).sum();
            let total_output: i64 = turns.iter().map(|t| t.usage.output_tokens as i64).sum();
            conn.execute(
                "INSERT INTO chat_sessions
                    (session_id, created_at, updated_at, turn_count, work_dir, model_name, title, total_input, total_output)
                 VALUES (?1, ?2, datetime('now','localtime'), ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(session_id) DO UPDATE SET
                    updated_at = datetime('now','localtime'),
                    turn_count = excluded.turn_count,
                    model_name = excluded.model_name,
                    total_input = excluded.total_input,
                    total_output = excluded.total_output",
                rusqlite::params![
                    session_id,
                    created_at,
                    turns.len() as i64,
                    work_dir,
                    model_name,
                    title,
                    total_input,
                    total_output,
                ],
            )?;
            conn.execute(
                "DELETE FROM chat_turns WHERE session_id = ?1",
                [session_id],
            )?;
            let mut stmt = conn.prepare_cached(
                "INSERT INTO chat_turns
                    (session_id, seq, ts, raw_input, prompt, response, context_response,
                     outcome, input_tokens, output_tokens, cache_read, cache_creation, cost_usd)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            )?;
            for t in turns {
                stmt.execute(rusqlite::params![
                    session_id,
                    t.seq,
                    t.ts,
                    t.raw_input,
                    t.prompt,
                    t.response,
                    t.context_response,
                    t.outcome,
                    t.usage.input_tokens as i64,
                    t.usage.output_tokens as i64,
                    t.usage.cache_read_input_tokens as i64,
                    t.usage.cache_creation_input_tokens as i64,
                    t.cost_usd,
                ])?;
            }
            drop(stmt);
            if !existed {
                prune_old_sessions_tx(&conn)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// 列出持久化会话(updated_at 新→旧,至多 `limit` 条)。
    pub fn list_chat_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let conn = self.conn.lock().expect("db conn");
        let mut stmt = conn.prepare_cached(
            "SELECT session_id, created_at, updated_at, turn_count, work_dir,
                    model_name, title, total_input, total_output
             FROM chat_sessions ORDER BY updated_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit as i64], row_to_summary)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// 读回某会话全部轮次(seq 升序)。
    pub fn load_chat_turns(&self, session_id: &str) -> Result<Vec<ChatTurnRow>> {
        let conn = self.conn.lock().expect("db conn");
        let mut stmt = conn.prepare_cached(
            "SELECT seq, ts, raw_input, prompt, response, context_response, outcome,
                    input_tokens, output_tokens, cache_read, cache_creation, cost_usd
             FROM chat_turns WHERE session_id = ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt
            .query_map([session_id], |r| {
                Ok(ChatTurnRow {
                    seq: r.get(0)?,
                    ts: r.get(1)?,
                    raw_input: r.get(2)?,
                    prompt: r.get(3)?,
                    response: r.get(4)?,
                    context_response: r.get(5)?,
                    outcome: r.get(6)?,
                    usage: Usage {
                        input_tokens: r.get::<_, i64>(7)? as u32,
                        output_tokens: r.get::<_, i64>(8)? as u32,
                        cache_read_input_tokens: r.get::<_, i64>(9)? as u32,
                        cache_creation_input_tokens: r.get::<_, i64>(10)? as u32,
                    },
                    cost_usd: r.get(11)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn row_to_summary(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        session_id: r.get(0)?,
        created_at: r.get(1)?,
        updated_at: r.get(2)?,
        turn_count: r.get(3)?,
        work_dir: r.get(4)?,
        model_name: r.get(5)?,
        title: r.get(6)?,
        total_input: r.get(7)?,
        total_output: r.get(8)?,
    })
}

/// 事务内淘汰超额会话(须在 BEGIN..COMMIT 之间调用)。
fn prune_old_sessions_tx(conn: &Connection) -> Result<()> {
    // 先删轮次再删会话行,避免留下孤儿轮次
    conn.execute_batch(&format!(
        "DELETE FROM chat_turns WHERE session_id IN (
             SELECT session_id FROM chat_sessions
             ORDER BY updated_at DESC, rowid DESC LIMIT -1 OFFSET {CHAT_SESSIONS_KEEP});
         DELETE FROM chat_sessions WHERE session_id IN (
             SELECT session_id FROM chat_sessions
             ORDER BY updated_at DESC, rowid DESC LIMIT -1 OFFSET {CHAT_SESSIONS_KEEP});"
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{Db, Paths};

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    fn turn(seq: i64, raw: &str, prompt: &str, response: &str, ctx: &str) -> ChatTurnRow {
        ChatTurnRow {
            seq,
            ts: format!("10:00:0{seq}"),
            raw_input: raw.to_string(),
            prompt: prompt.to_string(),
            response: response.to_string(),
            context_response: ctx.to_string(),
            outcome: "direct".to_string(),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: 7,
                cache_creation_input_tokens: 3,
            },
            cost_usd: Some(0.001),
        }
    }

    #[test]
    fn snapshot_roundtrip_all_fields() {
        let (db, _d) = fresh_db();
        let turns = vec![
            turn(1, "原始输入一", "展开后提示词一(含附件块)", "人类版回答一", "上下文版回答一"),
            turn(2, "原始输入二", "展开后提示词二", "人类版回答二", "上下文版回答二"),
        ];
        db.save_chat_snapshot("sess-a", "2026-09-19 10:00:00", "/tmp/w", Some("gpt-4o-mini"), "标题", &turns)
            .unwrap();
        let loaded = db.load_chat_turns("sess-a").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded, turns, "整快照读回应逐字段一致(CJK/用量/成本)");

        let list = db.list_chat_sessions(10).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, "sess-a");
        assert_eq!(list[0].turn_count, 2);
        assert_eq!(list[0].title, "标题");
        assert_eq!(list[0].total_input, 200);
        assert_eq!(list[0].total_output, 100);
        assert_eq!(list[0].model_name.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn snapshot_rewrite_replaces_old_tail() {
        // /rewind 后落盘:旧第 3 轮必须被整快照重写清除,不能残留
        let (db, _d) = fresh_db();
        let three = vec![turn(1, "a", "a", "a", "a"), turn(2, "b", "b", "b", "b"), turn(3, "c", "c", "c", "c")];
        db.save_chat_snapshot("sess-b", "2026-09-19 11:00:00", "/w", None, "t", &three)
            .unwrap();
        // rewind 到第 2 轮后新增一轮 → 快照为 3 轮,但第 3 轮内容已换
        let after = vec![turn(1, "a", "a", "a", "a"), turn(2, "b", "b", "b", "b"), turn(3, "新轮", "新轮", "新轮", "新轮")];
        db.save_chat_snapshot("sess-b", "2026-09-19 11:00:00", "/w", None, "t", &after)
            .unwrap();
        let loaded = db.load_chat_turns("sess-b").unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[2].raw_input, "新轮");
        // 再 rewind 到 1 轮:旧 2/3 轮清除
        let one = vec![turn(1, "a", "a", "a", "a")];
        db.save_chat_snapshot("sess-b", "2026-09-19 11:00:00", "/w", None, "t", &one)
            .unwrap();
        assert_eq!(db.load_chat_turns("sess-b").unwrap().len(), 1);
        let list = db.list_chat_sessions(10).unwrap();
        assert_eq!(list[0].turn_count, 1, "会话行 turn_count 与快照同步");
    }

    #[test]
    fn list_orders_newest_first() {
        let (db, _d) = fresh_db();
        let t = vec![turn(1, "x", "x", "x", "x")];
        db.save_chat_snapshot("sess-old", "2026-09-18 09:00:00", "/w", None, "旧", &t)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100)); // updated_at 秒级粒度
        db.save_chat_snapshot("sess-new", "2026-09-19 09:00:00", "/w", None, "新", &t)
            .unwrap();
        let list = db.list_chat_sessions(10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].session_id, "sess-new", "新→旧排序");
        assert_eq!(list[1].session_id, "sess-old");
    }

    #[test]
    fn resolve_latest_index_prefix_ambiguous() {
        let mk = |id: &str| SessionSummary {
            session_id: id.to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            turn_count: 1,
            work_dir: String::new(),
            model_name: None,
            title: String::new(),
            total_input: 0,
            total_output: 0,
        };
        let sessions = vec![mk("20260919-1"), mk("20260918-2"), mk("20260917-3")];
        assert_eq!(resolve_chat_session(&sessions, "latest"), ResumeResolution::Latest);
        assert_eq!(resolve_chat_session(&sessions, ""), ResumeResolution::Latest);
        assert_eq!(resolve_chat_session(&sessions, "2"), ResumeResolution::Index(1));
        assert_eq!(resolve_chat_session(&sessions, "9"), ResumeResolution::NotFound, "越界序号");
        assert_eq!(resolve_chat_session(&sessions, "20260918"), ResumeResolution::Prefix(1));
        // 同日前缀命中多个 → Ambiguous
        match resolve_chat_session(&sessions, "2026") {
            ResumeResolution::Ambiguous(ids) => assert_eq!(ids.len(), 3),
            other => panic!("应歧义,实际 {other:?}"),
        }
        assert_eq!(resolve_chat_session(&sessions, "2030"), ResumeResolution::NotFound);
    }

    #[test]
    fn prune_keeps_only_recent_sessions() {
        let (db, _d) = fresh_db();
        let t = vec![turn(1, "x", "x", "x", "x")];
        for i in 0..=(CHAT_SESSIONS_KEEP as i64) {
            let id = format!("sess-prune-{i:03}");
            db.save_chat_snapshot(&id, "2026-09-19 12:00:00", "/w", None, "t", &t)
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let list = db.list_chat_sessions(1000).unwrap();
        assert_eq!(list.len(), CHAT_SESSIONS_KEEP, "超额会话被同事务淘汰");
        // 最旧的 sess-prune-000 被淘汰,其轮次不残留;最新的 sess-prune-050 完整存活
        assert!(db.load_chat_turns("sess-prune-000").unwrap().is_empty());
        assert!(!db.load_chat_turns("sess-prune-050").unwrap().is_empty());
    }

    #[test]
    fn startup_resume_static_take_once() {
        // take 语义:排定 → 取走一次 → 后续为 None(恢复只执行一次的核心保证)
        set_startup_resume("latest-test".to_string());
        assert!(startup_resume_is_set());
        assert_eq!(take_startup_resume().as_deref(), Some("latest-test"));
        assert!(take_startup_resume().is_none(), "已消费后不得再次返回");
        assert!(!startup_resume_is_set());
    }
}

