//! 会话导出(第十八轮 D8):transcript 记录 + Markdown/JSON 落盘。
//!
//! **为什么是 TUI 层独立 transcript**:主 Session 上下文只回填 user 消息
//! (assistant 输出走 `session_memory` 摘要链路),导出属于展示层概念,
//! 引入独立记录不动主上下文,对 Yolo 分类 / Compact 压缩 / token 估算零影响。
//!
//! 触发:TUI `/export [path]`。默认落工作目录 `laew-export-{YYYYMMDD-HHmmss}.md`;
//! 显式路径后缀 `.json` 导出 JSON,其余一律 Markdown;已存在的显式路径拒绝覆盖。
//! 设计见 `tmpPlan/2026-09-10_01-自定义斜杠命令与会话导出方案.md`。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::llm::Usage;

/// 本地时间格式化(time crate;格式串语法同 session.rs::format_readable)。
fn local_datetime(fmt: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let base = time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let dt = match time::UtcOffset::current_local_offset() {
        Ok(offset) => base.to_offset(offset),
        Err(_) => base,
    };
    dt.format(
        &time::format_description::parse_borrowed::<2>(fmt).expect("format"),
    )
    .expect("fmt")
}

/// 当前本地时间 HH:MM:SS(transcript 轮次时间戳)。
pub fn now_clock() -> String {
    local_datetime("[hour]:[minute]:[second]")
}

/// 当前本地时间 YYYYMMDD-HHMMSS(默认导出文件名时间段,与 Session ID 同形态)。
pub fn now_export_stamp() -> String {
    local_datetime("[year][month][day]-[hour][minute][second]")
}

/// 当前本地时间 YYYY-MM-DD HH:MM:SS(导出文档可读时间)。
pub fn now_export_human() -> String {
    local_datetime("[year]-[month]-[day] [hour]:[minute]:[second]")
}

/// "YYYYMMDD-HHMMSS"(session::now_readable 形态)→ "YYYY-MM-DD HH:MM:SS";
/// 长度/形态不符时原样返回(容忍历史数据)。
pub fn humanize_compact(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() == 15
        && b[0..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'-'
        && b[9..15].iter().all(u8::is_ascii_digit)
    {
        format!(
            "{}-{}-{} {}:{}:{}",
            &s[0..4],
            &s[4..6],
            &s[6..8],
            &s[9..11],
            &s[11..13],
            &s[13..15]
        )
    } else {
        s.to_string()
    }
}

/// 单轮对话记录(用户输入 + assistant 输出 + 本轮用量)。
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptEntry {
    /// 本轮时间(HH:MM:SS 本地)。
    pub ts: String,
    /// 用户原始输入行(自定义命令记 `/cmd args` 原文)。
    pub raw_input: String,
    /// 实际送入编排的提示词(命令展开后;与 raw_input 相同时导出不重复展示)。
    pub prompt: String,
    /// assistant 输出文本(与屏幕显示同源)。
    pub response: String,
    /// 本轮 token 用量。
    #[serde(flatten)]
    pub usage: Usage,
    /// 本轮结局。
    pub outcome: OutcomeKind,
}

/// 任务结局(导出展示与统计用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// Yolo 直接回答(simple 档短路)。
    DirectAnswer,
    /// 完整编排执行(workflows + QC)。
    Executed,
    /// 编排失败(带建议)。
    Failed,
    /// 框架错误(非任务失败)。
    Error,
    /// 用户取消。
    Cancelled,
}

impl OutcomeKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            OutcomeKind::DirectAnswer => "直接回答",
            OutcomeKind::Executed => "已执行",
            OutcomeKind::Failed => "失败",
            OutcomeKind::Error => "错误",
            OutcomeKind::Cancelled => "已取消",
        }
    }
}

/// 导出文档元信息。
#[derive(Debug, Clone, Serialize)]
pub struct ExportMeta {
    pub session_id: String,
    pub session_created_at: String,
    pub exported_at: String,
    pub model: String,
    pub turns: usize,
    /// 累计 token 用量(全轮次相加)。
    #[serde(flatten)]
    pub total_usage: Usage,
}

/// JSON 导出的顶层结构。
#[derive(Debug, Serialize)]
pub struct ExportDump {
    pub meta: ExportMeta,
    pub entries: Vec<TranscriptEntry>,
}

/// 渲染 Markdown 导出文档。
pub fn render_markdown(meta: &ExportMeta, entries: &[TranscriptEntry]) -> String {
    let mut out = String::new();
    out.push_str("# laew 会话导出\n\n");
    out.push_str("| 项 | 值 |\n|---|---|\n");
    out.push_str(&format!("| Session | {} |\n", meta.session_id));
    out.push_str(&format!("| 创建时间 | {} |\n", meta.session_created_at));
    out.push_str(&format!("| 导出时间 | {} |\n", meta.exported_at));
    out.push_str(&format!("| 模型 | {} |\n", meta.model));
    out.push_str(&format!("| 对话轮数 | {} |\n", meta.turns));
    if meta.total_usage.input_tokens > 0 || meta.total_usage.output_tokens > 0 {
        out.push_str(&format!(
            "| 累计用量 | input={} output={}{} |\n",
            meta.total_usage.input_tokens,
            meta.total_usage.output_tokens,
            cache_suffix(&meta.total_usage)
        ));
    }
    out.push('\n');

    if entries.is_empty() {
        out.push_str("_(会话尚无对话内容)_\n");
        return out;
    }

    for (i, e) in entries.iter().enumerate() {
        out.push_str(&format!("## {}. 用户 · {}\n\n", i + 1, e.ts));
        out.push_str(&format!("```text\n{}\n```\n", e.raw_input));
        // 自定义命令展开:提示词与原始输入不同时附引用块,保留「实际发了什么」
        if e.prompt.trim() != e.raw_input.trim() && !e.prompt.trim().is_empty() {
            out.push_str(&format!("\n> 命令展开提示词:\n> \n```text\n{}\n```\n", e.prompt));
        }
        out.push_str(&format!(
            "\n**laew**({}):\n\n",
            e.outcome.display_name()
        ));
        if e.response.trim().is_empty() {
            out.push_str("_(无输出)_\n");
        } else {
            out.push_str(&format!("```text\n{}\n```\n", e.response));
        }
        if e.usage.input_tokens > 0 || e.usage.output_tokens > 0 {
            out.push_str(&format!(
                "\n本轮用量: input={} output={}{}\n",
                e.usage.input_tokens,
                e.usage.output_tokens,
                cache_suffix(&e.usage)
            ));
        }
        out.push('\n');
    }
    out
}

/// 渲染 JSON 导出文档(pretty)。
pub fn render_json(meta: &ExportMeta, entries: &[TranscriptEntry]) -> String {
    let dump = ExportDump {
        meta: meta.clone(),
        entries: entries.to_vec(),
    };
    serde_json::to_string_pretty(&dump).unwrap_or_else(|_| "{}".to_string())
}

fn cache_suffix(u: &Usage) -> String {
    let mut s = String::new();
    if u.cache_read_input_tokens > 0 {
        s.push_str(&format!(" cache_read={}", u.cache_read_input_tokens));
    }
    if u.cache_creation_input_tokens > 0 {
        s.push_str(&format!(" cache_creation={}", u.cache_creation_input_tokens));
    }
    s
}

/// 导出格式(按目标路径推断)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
}

/// 解析导出目标:
/// - `None` → 工作目录默认文件名 `{prefix}-{ts}.md`(Markdown);
/// - `Some(p)` → 原样使用,后缀 `.json` 为 JSON,否则 Markdown。
///
/// 默认文件名冲突时自动追加 `-1`/`-2`(最多 100 次,对齐 openclaw);
/// **显式路径已存在则返回错误,拒绝覆盖用户文件**。
pub fn resolve_target(
    work_dir: &Path,
    explicit: Option<&str>,
    default_prefix: &str,
    default_ts: &str,
) -> std::io::Result<(PathBuf, ExportFormat)> {
    match explicit {
        Some(p) => {
            let path = PathBuf::from(p);
            if path.exists() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("目标文件已存在,拒绝覆盖: {}", path.display()),
                ));
            }
            let fmt = if path.extension().and_then(|e| e.to_str()) == Some("json") {
                ExportFormat::Json
            } else {
                ExportFormat::Markdown
            };
            Ok((path, fmt))
        }
        None => {
            let mut candidate = work_dir.join(format!("{default_prefix}-{default_ts}.md"));
            // 冲突时 -1/-2 后缀(防秒级时间戳同秒覆盖);默认导出固定 Markdown
            let fmt = ExportFormat::Markdown;
            for n in 1..=100 {
                if !candidate.exists() {
                    return Ok((candidate, fmt));
                }
                candidate = work_dir.join(format!("{default_prefix}-{default_ts}-{n}.md"));
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "默认导出文件名冲突超过 100 次",
            ))
        }
    }
}

/// 写入导出文件(父目录缺失时报错提示,不自动创建——用户显式指定目录应有感知)。
pub fn write_export(
    meta: &ExportMeta,
    entries: &[TranscriptEntry],
    path: &Path,
    fmt: ExportFormat,
) -> std::io::Result<()> {
    let content = match fmt {
        ExportFormat::Markdown => render_markdown(meta, entries),
        ExportFormat::Json => render_json(meta, entries),
    };
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(outcome: OutcomeKind, response: &str) -> TranscriptEntry {
        TranscriptEntry {
            ts: "14:23:05".into(),
            raw_input: "跑个任务".into(),
            prompt: "跑个任务".into(),
            response: response.into(),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                ..Default::default()
            },
            outcome,
        }
    }

    fn meta(turns: usize) -> ExportMeta {
        ExportMeta {
            session_id: "20260910-120000-abcd12-123-abc123".into(),
            session_created_at: "2026-09-10 12:00:00".into(),
            exported_at: "2026-09-10 14:24:00".into(),
            model: "[anthropic] mock/claude-mock".into(),
            turns,
            total_usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                ..Default::default()
            },
        }
    }

    #[test]
    fn markdown_contains_meta_and_turns() {
        let entries = vec![entry(OutcomeKind::Executed, "任务完成")];
        let md = render_markdown(&meta(1), &entries);
        assert!(md.contains("# laew 会话导出"));
        assert!(md.contains("20260910-120000-abcd12-123-abc123"));
        assert!(md.contains("对话轮数 | 1"));
        assert!(md.contains("## 1. 用户 · 14:23:05"));
        assert!(md.contains("跑个任务"));
        assert!(md.contains("任务完成"));
        assert!(md.contains("已执行"));
        assert!(md.contains("input=100 output=50"));
    }

    #[test]
    fn markdown_empty_session() {
        let md = render_markdown(&meta(0), &[]);
        assert!(md.contains("会话尚无对话内容"));
    }

    #[test]
    fn markdown_command_expansion_shown_once() {
        let mut e = entry(OutcomeKind::Executed, "ok");
        e.raw_input = "/review src/main.rs".into();
        e.prompt = "请评审 src/main.rs".into();
        let md = render_markdown(&meta(1), &[e]);
        assert!(md.contains("/review src/main.rs"));
        assert!(md.contains("命令展开提示词"));
        assert!(md.contains("请评审 src/main.rs"));
    }

    #[test]
    fn markdown_plain_input_no_expansion_block() {
        let md = render_markdown(&meta(1), &[entry(OutcomeKind::Executed, "ok")]);
        assert!(!md.contains("命令展开提示词"));
    }

    #[test]
    fn json_roundtrip() {
        let entries = vec![entry(OutcomeKind::Failed, "失败输出")];
        let js = render_json(&meta(1), &entries);
        let v: serde_json::Value = serde_json::from_str(&js).expect("valid json");
        assert_eq!(v["meta"]["turns"], 1);
        assert_eq!(v["entries"][0]["outcome"], "failed");
        assert_eq!(v["entries"][0]["input_tokens"], 100);
    }

    #[test]
    fn resolve_default_conflict_appends_suffix() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let ts = "20260910-120000";
        // 第一次:无冲突
        let (p1, fmt) = resolve_target(tmp.path(), None, "laew-export", ts).unwrap();
        assert_eq!(fmt, ExportFormat::Markdown);
        assert!(p1.ends_with("laew-export-20260910-120000.md"));
        std::fs::write(&p1, "x").unwrap();
        // 第二次:同秒冲突 → -1 后缀
        let (p2, _) = resolve_target(tmp.path(), None, "laew-export", ts).unwrap();
        assert!(p2.ends_with("laew-export-20260910-120000-1.md"));
    }

    #[test]
    fn resolve_explicit_existing_rejected() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let existing = tmp.path().join("out.md");
        std::fs::write(&existing, "已有内容").unwrap();
        let err = resolve_target(tmp.path(), Some(existing.to_str().unwrap()), "x", "t").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn resolve_explicit_json_extension() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let (p, fmt) =
            resolve_target(tmp.path(), Some("/tmp/whatever/a.json"), "x", "t").unwrap();
        assert_eq!(fmt, ExportFormat::Json);
        assert!(p.ends_with("a.json"));
        // 无后缀 / .md → Markdown
        let (_, f2) = resolve_target(tmp.path(), Some("/tmp/whatever/a"), "x", "t").unwrap();
        assert_eq!(f2, ExportFormat::Markdown);
        let (_, f3) = resolve_target(tmp.path(), Some("/tmp/whatever/a.md"), "x", "t").unwrap();
        assert_eq!(f3, ExportFormat::Markdown);
    }

    #[test]
    fn write_export_creates_file() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let target = tmp.path().join("e.md");
        write_export(&meta(0), &[], &target, ExportFormat::Markdown).unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("# laew 会话导出"));
    }

    #[test]
    fn humanize_compact_formats_and_tolerates() {
        assert_eq!(
            humanize_compact("20260910-093820"),
            "2026-09-10 09:38:20"
        );
        // 形态不符原样返回
        assert_eq!(humanize_compact("随便什么"), "随便什么");
        assert_eq!(humanize_compact(""), "");
        // 长度对但非数字/分隔符不符
        assert_eq!(humanize_compact("2026091x-093820"), "2026091x-093820");
    }

    #[test]
    fn time_helpers_shapes() {
        assert_eq!(now_clock().len(), 8); // HH:MM:SS
        assert!(now_clock().matches(':').count() == 2);
        assert_eq!(now_export_stamp().len(), 15); // YYYYMMDD-HHMMSS
        assert_eq!(now_export_human().len(), 19); // YYYY-MM-DD HH:MM:SS
    }
}
