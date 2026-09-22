//! 决策审计可视化与统计(2026-09-22 第 113 轮 D9-8 闭环)。
//!
//! 把 `decision_audit` 模块已经写入的 JSONL 审计数据,渲染为 TUI 可消费的
//! 表格 / 详情 / 统计 / 校验摘要,供 `/audit` 斜杠命令展示与 `/cost` 末尾集成。
//!
//! ## 设计要点
//!
//! 1. **纯函数渲染**:输入事件列表 + 配置 → 输出 ANSI 字符串,零 IO、零 panic。
//! 2. **CJK 安全**:所有列宽用 `tui::format::display_width` 估算(CJK 计 2)。
//! 3. **截断保护**:outcome / rationale 超长按 `MAX_*_CHARS` 截断并标注 `…`。
//! 4. **零依赖**:不引入新 crate,复用 `tui::theme::palette()` 主题色。
//! 5. **容错**:缺字段事件降级展示为 `—`,不 panic。
//!
//! ## 与 decision_audit 的关系
//!
//! - **写**:`decision_audit::record_*` 由 orchestrator 五个决策点调用。
//! - **读**:本模块消费 `decision_audit::read_events` 返回的事件列表。
//! - **清**:`decision_audit::trim_audit_files` 由本模块的 `handle_clean` 暴露。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub use crate::agent::decision_audit::{
    self, AuditEvent, VerifyLine, VerifyResult,
};
use crate::tui::input::display_width;

/// 表格列宽上限(字符数,CJK 按 2 计)。
const MAX_AGENT_CHARS: usize = 14;
const MAX_DECISION_CHARS: usize = 12;
const MAX_OUTCOME_PREVIEW: usize = 40;
/// 表格最大默认事件数(`/audit` 无参时)。
pub const DEFAULT_TABLE_LIMIT: usize = 10;
/// `/audit last N` 默认与上限。
const DEFAULT_DETAIL_N: usize = 5;
const MAX_DETAIL_N: usize = 50;
/// 自动清理默认保留 session 数。
pub const DEFAULT_KEEP_SESSIONS: usize = 10;

// ============================================================
// 视图层辅助类型
// ============================================================

/// 单条事件的表格展示快照。
#[derive(Debug, Clone)]
pub struct EventRow {
    pub ts_short: String,
    pub agent: String,
    pub decision: String,
    pub outcome: String,
    pub duration_ms: u64,
}

/// 按 (decision, agent) 分组的统计行。
#[derive(Debug, Clone)]
pub struct StatRow {
    pub decision: String,
    pub agent: String,
    pub count: usize,
    pub avg_duration_ms: u64,
    pub total_duration_ms: u64,
    pub issues_total: usize,
}

/// 校验摘要行。
#[derive(Debug, Clone)]
pub struct VerifySummary {
    pub valid_count: usize,
    pub invalid_count: usize,
    pub invalid_lines: Vec<VerifyLine>,
    pub path: PathBuf,
}

// ============================================================
// 加载入口
// ============================================================

/// 加载指定 session 的所有审计事件(包装 `decision_audit::read_events`)。
pub fn load_events(session_id: &str, root_dir: Option<&Path>) -> Vec<AuditEvent> {
    decision_audit::read_events(session_id, root_dir)
}

/// 便捷重导出(2026-09-22 第 113 轮):让 `super::audit_view::count_events` 可用。
pub use decision_audit::count_events;
/// 便捷重导出(2026-09-22 第 113 轮):让 `super::audit_view::file_size` 可用。
pub use decision_audit::file_size;

/// 加载并转成表格行(默认最近 10 条)。
pub fn load_recent_rows(
    session_id: &str,
    root_dir: Option<&Path>,
    limit: usize,
) -> Vec<EventRow> {
    let events = load_events(session_id, root_dir);
    let take_n = limit.min(events.len());
    let start = events.len().saturating_sub(take_n);
    events[start..]
        .iter()
        .map(event_to_row)
        .collect()
}

// ============================================================
// 转换与渲染
// ============================================================

fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let width = display_width(s) as usize;
    if width <= max_chars {
        return s.to_string();
    }
    // 简单按字符数截(忽略宽度,保守起见)
    let mut out = String::new();
    let mut count: usize = 0;
    for ch in s.chars() {
        let w = display_width(&ch.to_string()) as usize;
        if count + w + 1 > max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
        count += w;
    }
    out
}

fn event_to_row(ev: &AuditEvent) -> EventRow {
    let ts_short = ev
        .ts
        .split('T')
        .nth(1)
        .unwrap_or(&ev.ts)
        .trim_end_matches('Z')
        .chars()
        .take(8)
        .collect::<String>();
    EventRow {
        ts_short: if ts_short.is_empty() { "—".into() } else { ts_short },
        agent: truncate_with_ellipsis(&ev.agent, MAX_AGENT_CHARS),
        decision: truncate_with_ellipsis(&ev.decision, MAX_DECISION_CHARS),
        outcome: truncate_with_ellipsis(&ev.outcome, MAX_OUTCOME_PREVIEW),
        duration_ms: ev.duration_ms,
    }
}

/// 渲染事件表格(多行字符串)。
pub fn format_event_table(rows: &[EventRow]) -> String {
    if rows.is_empty() {
        return "  (无审计事件)".to_string();
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "  {:<8}  {:<14}  {:<12}  {:>7}  outcome",
        "时间", "Agent", "decision", "耗时ms"
    );
    let _ = writeln!(
        out,
        "  {:-<8}  {:-<14}  {:-<12}  {:->7}  {:-<40}",
        "", "", "", "", ""
    );
    for r in rows {
        let _ = writeln!(
            out,
            "  {:<8}  {:<14}  {:<12}  {:>7}  {}",
            r.ts_short, r.agent, r.decision, r.duration_ms, r.outcome
        );
    }
    out
}

/// 渲染单条事件的完整详情(多行字符串)。
pub fn format_event_detail(ev: &AuditEvent) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "  时间:     {}", ev.ts);
    let _ = writeln!(out, "  Agent:    {}", ev.agent);
    let _ = writeln!(out, "  decision: {}", ev.decision);
    let _ = writeln!(out, "  duration: {} ms", ev.duration_ms);
    let _ = writeln!(out, "  input:    {}", truncate_with_ellipsis(&ev.input_summary, 200));
    let _ = writeln!(out, "  outcome:  {}", truncate_with_ellipsis(&ev.outcome, 500));
    let _ = writeln!(out, "  rationale:{}", truncate_with_ellipsis(&ev.rationale, 500));
    if let Some(tokens) = &ev.tokens {
        let _ = writeln!(
            out,
            "  tokens:   input={} output={} cache_read={} cache_write={}",
            tokens.input_tokens,
            tokens.output_tokens,
            tokens.cache_read_input_tokens,
            tokens.cache_creation_input_tokens
        );
    }
    if let Some(meta) = &ev.meta {
        let _ = writeln!(out, "  meta:     {}", truncate_with_ellipsis(&meta.to_string(), 240));
    }
    out
}

// ============================================================
// 统计聚合
// ============================================================

/// 按 (decision, agent) 分组聚合事件。
///
/// 顺序:先 decision,后 agent;相同 key 内按事件出现顺序累加。
pub fn aggregate_stats(events: &[AuditEvent]) -> Vec<StatRow> {
    // 使用 BTreeMap 保证稳定输出顺序(decision 字母序 → agent 字母序)
    let mut groups: BTreeMap<(String, String), (usize, u64, usize)> = BTreeMap::new();
    for ev in events {
        let key = (ev.decision.clone(), ev.agent.clone());
        let entry = groups.entry(key).or_insert((0, 0, 0));
        entry.0 += 1;
        entry.1 += ev.duration_ms;
        // issues 仅 verdict 决策类型有意义(meta.issues 数组长度)
        if ev.decision == "verdict" {
            if let Some(meta) = &ev.meta {
                if let Some(arr) = meta.get("issues").and_then(|x| x.as_array()) {
                    entry.2 += arr.len();
                }
            }
        }
    }
    groups
        .into_iter()
        .map(|((decision, agent), (count, total_duration, issues_total))| StatRow {
            decision,
            agent,
            count,
            avg_duration_ms: if count > 0 { total_duration / count as u64 } else { 0 },
            total_duration_ms: total_duration,
            issues_total,
        })
        .collect()
}

/// 渲染统计表(多行字符串)。
pub fn format_stats_table(stats: &[StatRow]) -> String {
    if stats.is_empty() {
        return "  (无决策事件可统计)".to_string();
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "  {:<12}  {:<14}  {:>5}  {:>10}  {:>10}  issues",
        "decision", "agent", "次数", "avgms", "totalms"
    );
    let _ = writeln!(
        out,
        "  {:-<12}  {:-<14}  {:->5}  {:->10}  {:->10}  {:-<6}",
        "", "", "", "", "", ""
    );
    let mut total_count = 0usize;
    let mut total_duration = 0u64;
    let mut total_issues = 0usize;
    for s in stats {
        let _ = writeln!(
            out,
            "  {:<12}  {:<14}  {:>5}  {:>10}  {:>10}  {}",
            s.decision, s.agent, s.count, s.avg_duration_ms, s.total_duration_ms, s.issues_total
        );
        total_count += s.count;
        total_duration += s.total_duration_ms;
        total_issues += s.issues_total;
    }
    let _ = writeln!(out, "  ---");
    let _ = writeln!(
        out,
        "  {:<12}  {:<14}  {:>5}  {:>10}  {:>10}  {}",
        "总计", "—", total_count, "—", total_duration, total_issues
    );
    out
}

// ============================================================
// 校验摘要
// ============================================================

/// 运行完整性校验并转成展示摘要。
pub fn run_verify(session_id: &str, root_dir: Option<&Path>) -> VerifySummary {
    let result: VerifyResult = decision_audit::verify_events(session_id, root_dir);
    let invalid_count = result.invalid_lines.len();
    VerifySummary {
        valid_count: result.valid_count,
        invalid_count,
        invalid_lines: result.invalid_lines,
        path: result.path,
    }
}

/// 渲染校验结果(多行字符串)。
pub fn format_verify_summary(summary: &VerifySummary) -> String {
    let mut out = String::new();
    if summary.valid_count == 0 && summary.invalid_count == 0 {
        let _ = writeln!(out, "  (审计文件不存在或为空)");
        return out;
    }
    let total = summary.valid_count + summary.invalid_count;
    if summary.invalid_count == 0 {
        let _ = writeln!(out, "  ✓ {} 条全部有效", summary.valid_count);
    } else {
        let _ = writeln!(
            out,
            "  ✗ {} 条中 {} 条损坏(详见下方)",
            total, summary.invalid_count
        );
        for line in &summary.invalid_lines {
            let _ = write!(out, "    L{}: ", line.line_no);
            if let Some(dec) = &line.decision {
                let _ = write!(out, "decision={} ", dec);
            }
            if let Some(err) = &line.error {
                let _ = writeln!(out, "{}", err);
            }
        }
    }
    let _ = writeln!(out, "  路径: {}", summary.path.display());
    out
}

// ============================================================
// 清理
// ============================================================

/// 清理旧审计文件,返回 (已清理数, 保留数)。
pub fn handle_clean(root_dir: Option<&Path>, keep: usize) -> (usize, usize) {
    let root = match root_dir {
        Some(p) => p.to_path_buf(),
        None => match decision_audit::audit_root_dir() {
            Some(r) => r,
            None => return (0, 0),
        },
    };
    let removed = decision_audit::trim_audit_files(&root, keep).unwrap_or(0);
    // 计算保留数
    let audit_dir = root.join("AuditTrail");
    let kept = if audit_dir.exists() {
        std::fs::read_dir(&audit_dir)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path()
                            .extension()
                            .and_then(|x| x.to_str())
                            .is_some_and(|x| x == "jsonl")
                    })
                    .count()
            })
            .unwrap_or(0)
    } else {
        0
    };
    (removed, kept)
}

// ============================================================
// /audit 命令参数解析
// ============================================================

/// `/audit` 子命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditSubcmd {
    Table,
    Last(usize),
    Stats,
    Verify,
    Clean(usize),
    Help,
}

impl AuditSubcmd {
    pub fn parse(args: &str) -> Self {
        let mut parts = args.split_whitespace();
        match parts.next() {
            None => AuditSubcmd::Table,
            Some("last") => {
                let n = parts
                    .next()
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(DEFAULT_DETAIL_N)
                    .min(MAX_DETAIL_N);
                AuditSubcmd::Last(n)
            }
            Some("stats") => AuditSubcmd::Stats,
            Some("verify") => AuditSubcmd::Verify,
            Some("clean") => {
                // 支持 --keep N 与位置 N 两种
                let mut keep = DEFAULT_KEEP_SESSIONS;
                while let Some(p) = parts.next() {
                    if p == "--keep" {
                        if let Some(v) = parts.next() {
                            if let Ok(n) = v.parse::<usize>() {
                                keep = n;
                            }
                        }
                    } else if let Ok(n) = p.parse::<usize>() {
                        keep = n;
                    }
                }
                AuditSubcmd::Clean(keep)
            }
            Some("help" | "--help" | "-h") => AuditSubcmd::Help,
            Some(other) => {
                // 兼容直接传数字 → 等价 last N
                if let Ok(n) = other.parse::<usize>() {
                    AuditSubcmd::Last(n.min(MAX_DETAIL_N))
                } else {
                    AuditSubcmd::Help
                }
            }
        }
    }
}

// ============================================================
// 帮助文本
// ============================================================

pub fn help_text() -> &'static str {
    "  /audit                 列出当前 session 最近 10 条事件(表格)\n\
       /audit last [N]        列出最近 N 条详细事件(默认 5,上限 50)\n\
       /audit stats           按 (decision, agent) 分组统计 + 总耗时\n\
       /audit verify          校验 JSONL 完整性(损坏行 + 必填字段)\n\
       /audit clean [--keep N] 清理旧 session 审计文件(默认保留 10 个)\n\
       /audit help            显示本帮助"
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::decision_audit::AuditEvent;
    use crate::llm::Usage;

    fn ev(decision: &str, agent: &str, duration_ms: u64) -> AuditEvent {
        AuditEvent::new("s1", agent, decision, "i", "o", "r", duration_ms)
    }

    #[test]
    fn table_renders_empty() {
        let s = format_event_table(&[]);
        assert!(s.contains("无审计事件"));
    }

    #[test]
    fn table_renders_rows() {
        let rows = vec![EventRow {
            ts_short: "12:34:56".into(),
            agent: "Yolo".into(),
            decision: "classify".into(),
            outcome: "level=medium".into(),
            duration_ms: 1234,
        }];
        let s = format_event_table(&rows);
        assert!(s.contains("Yolo"));
        assert!(s.contains("classify"));
        assert!(s.contains("1234"));
    }

    #[test]
    fn detail_renders_all_fields() {
        let mut e = ev("compact", "compact", 200);
        e = e.with_tokens(Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        });
        let s = format_event_detail(&e);
        assert!(s.contains("decision: compact"));
        assert!(s.contains("duration: 200"));
        assert!(s.contains("tokens:"));
        assert!(s.contains("input=100"));
    }

    #[test]
    fn aggregate_groups_by_decision_and_agent() {
        let events = vec![
            ev("classify", "yolo", 100),
            ev("classify", "yolo", 200),
            ev("verdict", "quality", 50),
        ];
        let stats = aggregate_stats(&events);
        assert_eq!(stats.len(), 2);
        let classify = stats.iter().find(|s| s.decision == "classify").unwrap();
        assert_eq!(classify.count, 2);
        assert_eq!(classify.avg_duration_ms, 150);
        assert_eq!(classify.total_duration_ms, 300);
        let verdict = stats.iter().find(|s| s.decision == "verdict").unwrap();
        assert_eq!(verdict.count, 1);
    }

    #[test]
    fn stats_table_renders() {
        let stats = vec![StatRow {
            decision: "classify".into(),
            agent: "yolo".into(),
            count: 3,
            avg_duration_ms: 100,
            total_duration_ms: 300,
            issues_total: 0,
        }];
        let s = format_stats_table(&stats);
        assert!(s.contains("classify"));
        assert!(s.contains("yolo"));
        assert!(s.contains("总计"));
    }

    #[test]
    fn truncate_with_ellipsis_works() {
        let long = "x".repeat(100);
        let s = truncate_with_ellipsis(&long, 10);
        assert!(s.chars().count() <= 11);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn parse_subcmd_table() {
        assert_eq!(AuditSubcmd::parse(""), AuditSubcmd::Table);
        assert_eq!(AuditSubcmd::parse("   "), AuditSubcmd::Table);
    }

    #[test]
    fn parse_subcmd_last() {
        assert_eq!(AuditSubcmd::parse("last"), AuditSubcmd::Last(DEFAULT_DETAIL_N));
        assert_eq!(AuditSubcmd::parse("last 3"), AuditSubcmd::Last(3));
        assert_eq!(AuditSubcmd::parse("5"), AuditSubcmd::Last(5));
        // 上限截断
        assert_eq!(
            AuditSubcmd::parse(&format!("last {}", MAX_DETAIL_N + 100)),
            AuditSubcmd::Last(MAX_DETAIL_N)
        );
    }

    #[test]
    fn parse_subcmd_stats() {
        assert_eq!(AuditSubcmd::parse("stats"), AuditSubcmd::Stats);
    }

    #[test]
    fn parse_subcmd_verify() {
        assert_eq!(AuditSubcmd::parse("verify"), AuditSubcmd::Verify);
    }

    #[test]
    fn parse_subcmd_clean() {
        assert_eq!(
            AuditSubcmd::parse("clean"),
            AuditSubcmd::Clean(DEFAULT_KEEP_SESSIONS)
        );
        assert_eq!(AuditSubcmd::parse("clean 5"), AuditSubcmd::Clean(5));
        assert_eq!(AuditSubcmd::parse("clean --keep 7"), AuditSubcmd::Clean(7));
    }

    #[test]
    fn parse_subcmd_help() {
        assert_eq!(AuditSubcmd::parse("help"), AuditSubcmd::Help);
        assert_eq!(AuditSubcmd::parse("--help"), AuditSubcmd::Help);
        assert_eq!(AuditSubcmd::parse("foo"), AuditSubcmd::Help);
    }

    #[test]
    fn verify_summary_empty_when_no_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let summary = run_verify("s_nonexistent", Some(dir.path()));
        assert_eq!(summary.valid_count, 0);
        assert_eq!(summary.invalid_count, 0);
    }

    #[test]
    fn verify_summary_ok_for_valid_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let sid = "s_ok";
        // 写入 3 条有效事件
        let writer = decision_audit::session_writer(sid, root).expect("writer");
        for _ in 0..3 {
            let mut w = writer.lock().expect("lock");
            w.append(&ev("classify", "yolo", 0)).expect("append");
        }
        drop(writer);
        let summary = run_verify(sid, Some(root));
        assert_eq!(summary.valid_count, 3);
        assert_eq!(summary.invalid_count, 0);
        let rendered = format_verify_summary(&summary);
        assert!(rendered.contains("全部有效"));
    }

    #[test]
    fn handle_clean_returns_counts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let audit_dir = root.join("AuditTrail");
        std::fs::create_dir_all(&audit_dir).expect("dir");
        // 写入 5 个旧 session 文件
        for i in 0..5 {
            let path = audit_dir.join(format!("audit_s{i}.jsonl"));
            std::fs::write(&path, "{}").expect("write");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (removed, kept) = handle_clean(Some(root), 2);
        assert_eq!(removed, 3);
        assert_eq!(kept, 2);
    }
}
