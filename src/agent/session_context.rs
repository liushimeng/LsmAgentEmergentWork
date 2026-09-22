//! SessionContext Agent:会话层。
//!
//! - 每次用户输入完成后,生成 Markdown 摘要并写入 session_memory 表
//! - 下一轮 Yolo 处理前,把最近 N 条摘要注入历史上下文(MARKER 隔离)
//! - 失败回流时,把 user_suggestion 透传给用户

use std::sync::Arc;

use crate::agent::context::AgentRole;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::{Db, EventType};
use crate::error::Result;
use crate::llm::{ChatMessage, Usage};
use crate::session;

/// 历史注入标记(幂等探测锚点)。
pub const HISTORY_MARKER_START: &str = "<<<LAEW:SESSION_HISTORY>>>";
/// 历史注入结束标记。
pub const HISTORY_MARKER_END: &str = "<<<LAEW:SESSION_HISTORY_END>>>";

/// TODO 状态快照标记(2026-09-22 第 112 轮:D19 持久化增强)。
///
/// SessionContext 收口时,把当前 TodoWrite 快照 JSON 追加到 Summary 行 content 末尾,
/// 标记隔离后下游 `build_history_message` / `/tasks` 等可选解析,
/// 失败时降级为透传文本。
pub const TODO_SNAPSHOT_MARKER: &str = "<<<LAEW:TODOS>>>";
pub const TODO_SNAPSHOT_MARKER_END: &str = "<<<LAEW:TODOS_END>>>";

/// 注入历史摘要的最大条数。
pub const DEFAULT_HISTORY_LIMIT: usize = 3;

/// SessionContext 执行器。
pub struct SessionContextRunner {
    agent: Agent,
    db: Arc<Db>,
}

impl SessionContextRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::session_context_profile());
        Self { agent, db }
    }

    /// 收口:汇总本次任务,生成 Markdown 摘要,写入 session_memory。
    ///
    /// `yolo_degraded`:本次任务 Yolo 是否走了解析失败降级分支(关联报告: 2026-09-09_04 D-002);
    /// Orchestrator 传入后,本函数将其拼入 prompt,SessionContext Agent 据此避免把
    /// "任务完成 + Yolo 降级" 渲染成"目标解析失败(已降级)"的误导性标题。
    pub async fn summarize(
        &self,
        goal: &str,
        user_prompt: &str,
        plan_doc: Option<&std::path::Path>,
        workflow_results: &[(String, String, bool)], // (id, name, ok)
        total_usage: &Usage,
        session_id: &str,
        yolo_degraded: bool,
        task_level: &crate::agent::yolo::TaskLevel,
        todo_snapshot: Option<&str>, // 2026-09-22 第 112 轮:D19 持久化增强
    ) -> Result<SessionSummary> {
        let workflow_line = if workflow_results.is_empty() {
            "(无 WorkFlow)".to_string()
        } else {
            workflow_results
                .iter()
                .map(|(id, name, ok)| format!("{} {}: {}", if *ok { "✅" } else { "❌" }, id, name))
                .collect::<Vec<_>>()
                .join("; ")
        };

        let prompt = format!(
            "【SessionContext 收口】\n\
             目标: {goal}\n\
             用户输入: {user_prompt}\n\
             当前时间: {now}\n\
             任务档位: {level}\n\
             Plan 文档: {plan}\n\
             WorkFlow: {wf}\n\
             用量: input={i}, output={o}\n\
             Yolo 本次降级: {yolo_degraded_this}\n\
             Yolo 降级(本进程累计): {yolo_fallback} 次\n\n\
             请按系统提示词中的 Markdown 模板输出 200 字以内的简洁摘要。\
             摘要中的「时间」与「难度」**必须**原样使用上方给定的当前时间与任务档位,禁止自行推测。\
             若 Yolo 本次降级 = true,** 必须** 按以下规则处理标题与状态行:\n\
             - 标题不要写成「目标解析失败(已降级)」 —— 这是上游 LLM 输出格式问题,不是任务失败;\n\
             - 若 WorkFlow 全部成功:标题用原始 goal 摘要,状态行标记为「Yolo 降级 + 执行成功」;\n\
             - 若 WorkFlow 全部失败:标题用原始 goal 摘要,状态行标记为「Yolo 降级 + 执行失败」并保留失败原因;\n\
             若 Yolo 降级累计 > 0,务必在摘要中以「Yolo 降级 N 次」一行显式记录。",
            goal = goal,
            user_prompt = user_prompt,
            now = session::now_readable(),
            level = task_level.as_str(),
            plan = plan_doc.map(|p| p.display().to_string()).unwrap_or_else(|| "无".into()),
            wf = workflow_line,
            i = total_usage.input_tokens,
            o = total_usage.output_tokens,
            yolo_degraded_this = yolo_degraded,
            yolo_fallback = crate::agent::yolo::yolo_parse_failures(),
        );

        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;

        // 2026-09-22 第 112 轮:D19 TODO 持久化。把当前 todo 快照 JSON 追加到
        // Summary 行 content 末尾(标记隔离),下游 build_history_message 解析后
        // 可注入到下次 Yolo 处理上下文,跨 session 跨任务保持 todo 进度可见。
        let todo_appended = append_todo_snapshot_to_summary(&text, todo_snapshot);

        // 写入 session_memory(Summary 事件)
        let _ = self
            .db
            .insert_session_memory(&crate::config::SessionMemoryEntry {
                session_id: session_id.to_string(),
                role: AgentRole::SessionContext,
                event_type: EventType::Summary,
                content: todo_appended.clone(),
                usage_input: usage.input_tokens,
                usage_output: usage.output_tokens,
            });

        let _ = memory::record_entry(
            &self.db,
            AgentRole::SessionContext,
            session_id,
            goal,
            &text,
            None,
            serde_json::json!({ "summary_seq": self.db.next_session_seq(session_id).unwrap_or(0) }),
        );

        // 运行日志(2026-09-17 第 69 轮):SessionContext 会话摘要(记忆)已写入 session_memory
        tracing::info!(
            agent = "LsmAgentEmergentWork-SessionContext",
            session = session_id,
            summary_chars = text.chars().count(),
            usage_in = usage.input_tokens,
            usage_out = usage.output_tokens,
            "SessionContext 会话摘要(记忆)"
        );

        Ok(SessionSummary { text, usage })
    }

    /// 把失败 + 建议写入 session_memory。
    pub fn record_failure(
        &self,
        session_id: &str,
        goal: &str,
        reason: &str,
        suggestion: &str,
    ) -> Result<()> {
        self.db
            .insert_session_memory(&crate::config::SessionMemoryEntry {
                session_id: session_id.to_string(),
                role: AgentRole::Yolo,
                event_type: EventType::Failure,
                content: format!("目标: {goal}\n原因: {reason}"),
                usage_input: 0,
                usage_output: 0,
            })?;
        if !suggestion.is_empty() {
            self.db
                .insert_session_memory(&crate::config::SessionMemoryEntry {
                    session_id: session_id.to_string(),
                    role: AgentRole::SessionContext,
                    event_type: EventType::Suggestion,
                    content: suggestion.into(),
                    usage_input: 0,
                    usage_output: 0,
                })?;
        }
        Ok(())
    }
}

/// 摘要结果。
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub text: String,
    pub usage: Usage,
}

/// 构造历史注入消息(由 Orchestrator 在 Yolo 处理前调用)。
pub fn build_history_message(entries: &[crate::config::SessionMemoryRow]) -> Option<ChatMessage> {
    if entries.is_empty() {
        return None;
    }
    let mut text = format!("{HISTORY_MARKER_START}\n[SessionMemory 注入,非用户输入]\n");
    text.push_str("以下是本 Session 内最近的任务摘要,用于关联性参考;\n");
    text.push_str("不要把它本身当作用户请求,用户本轮请求以本消息之后的用户消息为准。\n");

    // 2026-09-22 第 112 轮:D19 TODO 持久化增强 — 从最近 Summary 行提取 TODO 快照,
    // 给下游 Yolo 提供上轮任务的 todo 进度上下文(若有,标"最近一次回复的 TODO")。
    let mut last_todo_snapshot: Option<String> = None;
    for e in entries.iter().rev() {
        if last_todo_snapshot.is_none() {
            if let Some(snap) = extract_todo_snapshot_from_summary(&e.content) {
                if !snap.is_empty() && snap != "{}" {
                    last_todo_snapshot = Some(snap);
                }
            }
        }
    }
    if let Some(ref snap) = last_todo_snapshot {
        text.push_str("\n--- 最近一次回复的 TODO 状态 (D19 持久化) ---\n");
        text.push_str(snap);
        text.push_str("\n--- TODO 结束 ---\n\n");
    }

    text.push_str("--- 摘要开始 ---\n");
    for e in entries.iter().rev() {
        // 旧 → 新 顺序;摘要正文剥除 TODO 标记块(避免重复展示)
        let cleaned = strip_todo_snapshot_block(&cleaned_content_for_history(&e.content));
        text.push_str(&format!(
            "- [seq={}, {}] {}\n",
            e.seq,
            e.created_at,
            cleaned
                .replace('\n', " ")
                .chars()
                .take(160)
                .collect::<String>(),
        ));
    }
    text.push_str("--- 摘要结束 ---\n");
    text.push_str(HISTORY_MARKER_END);
    Some(ChatMessage::user(text))
}

/// 从 summary content 中剥除 TODO 快照块(避免正文摘要里混着 JSON)。
fn strip_todo_snapshot_block(content: &str) -> String {
    if let Some(start) = content.find(TODO_SNAPSHOT_MARKER) {
        if let Some(end_rel) = content[start..].find(TODO_SNAPSHOT_MARKER_END) {
            let end = start + end_rel + TODO_SNAPSHOT_MARKER_END.len();
            let mut out = String::with_capacity(content.len());
            out.push_str(&content[..start]);
            out.push_str(&content[end..]);
            return out;
        }
    }
    content.to_string()
}

/// content 用于 history 注入展示的清洗(裁剪尾部空白等)。
fn cleaned_content_for_history(content: &str) -> &str {
    content.trim_end()
}

/// 上下文中是否已注入历史(幂等探测)。
pub fn is_history_injected(context: &[ChatMessage]) -> bool {
    context.iter().any(|m| {
        m.content.iter().any(|b| match b {
            crate::llm::ContentBlock::Text { text } => text.contains(HISTORY_MARKER_START),
            _ => false,
        })
    })
}

/// 由 Orchestrator 调用,把指定条目注入 Session 上下文(幂等)。
pub fn inject_history_with_entries(
    session: &mut session::Session,
    entries: &[crate::config::SessionMemoryRow],
) -> bool {
    if is_history_injected(session.context()) {
        return false;
    }
    match build_history_message(entries) {
        Some(msg) => {
            session.context_mut().insert(0, msg);
            true
        }
        None => false,
    }
}

/// 把 TODO 快照 JSON 拼接到 Summary 文本末尾(2026-09-22 第 112 轮:D19 持久化增强)。
///
/// 2026-09-22 第 112 轮抽离:纯文本拼接,供 `summarize()` 与未来其它写入路径共用;
/// 标记隔离 + 空快照降级 + 标记必成对存在。
pub fn append_todo_snapshot_to_summary(text: &str, snapshot: Option<&str>) -> String {
    match snapshot {
        None => text.to_string(),
        Some(s) if s.is_empty() || s == "{}" => text.to_string(),
        Some(s) => format!(
            "{text}\n\n{TODO_SNAPSHOT_MARKER}\n{s}\n{TODO_SNAPSHOT_MARKER_END}",
        ),
    }
}

/// 从 Summary content 提取 TODO 快照 JSON(下游可选解析)。
///
/// 解析失败返回 None,降级行为:下游继续按原文本处理。
pub fn extract_todo_snapshot_from_summary(content: &str) -> Option<String> {
    let start = content.find(TODO_SNAPSHOT_MARKER)?;
    let end = content.find(TODO_SNAPSHOT_MARKER_END)?;
    if end <= start {
        return None;
    }
    let inner_start = start + TODO_SNAPSHOT_MARKER.len();
    Some(content[inner_start..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Db, Paths, SessionMemoryEntry, SessionMemoryRow};
    use tempfile::tempdir;

    fn fresh_db() -> (Db, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (db, dir)
    }

    fn fake_row(seq: i64, content: &str) -> SessionMemoryRow {
        SessionMemoryRow {
            id: seq,
            session_id: "s-1".into(),
            seq,
            role: AgentRole::SessionContext,
            event_type: EventType::Summary,
            content: content.into(),
            usage_input: 0,
            usage_output: 0,
            created_at: "2026-09-03 15:30:00".into(),
        }
    }

    #[test]
    fn build_history_message_empty_returns_none() {
        let msg = build_history_message(&[]);
        assert!(msg.is_none());
    }

    #[test]
    fn build_history_message_includes_all_entries() {
        let rows = vec![fake_row(1, "first task"), fake_row(2, "second task")];
        let msg = build_history_message(&rows).unwrap();
        let text = match &msg.content[0] {
            crate::llm::ContentBlock::Text { text } => text.clone(),
            _ => panic!("应包含文本块"),
        };
        assert!(text.contains(HISTORY_MARKER_START));
        assert!(text.contains(HISTORY_MARKER_END));
        assert!(text.contains("first task"));
        assert!(text.contains("second task"));
    }

    #[test]
    fn inject_history_with_entries_idempotent() {
        let mut sess = session::Session::new();
        let rows = vec![fake_row(1, "first")];
        assert!(inject_history_with_entries(&mut sess, &rows));
        assert!(is_history_injected(sess.context()));
        // 二次:幂等
        assert!(!inject_history_with_entries(&mut sess, &rows));
    }

    #[test]
    fn append_todo_snapshot_with_none_returns_original() {
        let text = "original summary text";
        let out = append_todo_snapshot_to_summary(text, None);
        assert_eq!(out, text);
    }

    #[test]
    fn append_todo_snapshot_with_empty_returns_original() {
        let text = "original";
        let out = append_todo_snapshot_to_summary(text, Some(""));
        assert_eq!(out, text);
        let out = append_todo_snapshot_to_summary(text, Some("{}"));
        assert_eq!(out, text);
    }

    #[test]
    fn append_todo_snapshot_with_json_appends_block() {
        let text = "original summary";
        let snap = r#"{"total":2,"completed":1}"#;
        let out = append_todo_snapshot_to_summary(text, Some(snap));
        assert!(out.starts_with(text));
        assert!(out.contains(TODO_SNAPSHOT_MARKER));
        assert!(out.contains(TODO_SNAPSHOT_MARKER_END));
        assert!(out.contains(snap));
    }

    #[test]
    fn extract_todo_snapshot_round_trip() {
        let snap = r#"{"total":3,"items":[]}"#;
        let summary = append_todo_snapshot_to_summary("summary body", Some(snap));
        let extracted = extract_todo_snapshot_from_summary(&summary);
        assert_eq!(extracted.as_deref(), Some(snap));
    }

    #[test]
    fn extract_todo_snapshot_missing_returns_none() {
        // 没有标记 → None
        assert!(extract_todo_snapshot_from_summary("plain text").is_none());
        // 只有开始标记 → None
        let partial = format!("text{TODO_SNAPSHOT_MARKER}json");
        assert!(extract_todo_snapshot_from_summary(&partial).is_none());
    }

    #[test]
    fn build_history_message_includes_todo_snapshot() {
        // 模拟 session_memory 中有一条带 TODO 标记的 Summary
        let snap = r#"{"total":2,"completed":1,"in_progress":1}"#;
        let content = append_todo_snapshot_to_summary("summary body", Some(snap));
        let rows = vec![SessionMemoryRow {
            id: 1,
            session_id: "s-1".into(),
            seq: 1,
            role: AgentRole::SessionContext,
            event_type: EventType::Summary,
            content,
            usage_input: 0,
            usage_output: 0,
            created_at: "2026-09-22 10:00:00".into(),
        }];
        let msg = build_history_message(&rows).expect("build");
        // 标记不应出现在注入文本里(下游 Yolo 应能看到干净的 TODO JSON 块)
        let text = match msg.content.first().unwrap() {
            crate::llm::ContentBlock::Text { text } => text,
            _ => panic!("expected text block"),
        };
        assert!(text.contains("最近一次回复的 TODO 状态"), "应包含 TODO 块标题: {text}");
        assert!(text.contains(snap), "应包含 TODO 快照 JSON: {text}");
        // 主摘要不应再包含原始 marker(避免双展示)
        let snapshot_only = format!("{TODO_SNAPSHOT_MARKER}\n{snap}\n{TODO_SNAPSHOT_MARKER_END}");
        let parts = text.matches(&snapshot_only).count();
        assert_eq!(parts, 0, "stripped 后 marker 不应再出现: {text}");
    }

    #[test]
    fn strip_todo_snapshot_block_basic() {
        let inner_json = "{\"total\":1}";
        let content = format!(
            "summary text\n\n{TODO_SNAPSHOT_MARKER}\n{inner_json}\n{TODO_SNAPSHOT_MARKER_END}\nend"
        );
        let out = strip_todo_snapshot_block(&content);
        assert!(!out.contains(TODO_SNAPSHOT_MARKER));
        assert!(!out.contains(TODO_SNAPSHOT_MARKER_END));
        assert!(out.contains("summary text"));
        assert!(out.contains("end"));
    }

    #[test]
    fn record_failure_writes_two_events() {
        let (db, _d) = fresh_db();
        let sid = "s-1";
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: sid.into(),
            role: AgentRole::Yolo,
            event_type: EventType::Failure,
            content: "目标: x\n原因: y".into(),
            usage_input: 0,
            usage_output: 0,
        })
        .unwrap();
        db.insert_session_memory(&SessionMemoryEntry {
            session_id: sid.into(),
            role: AgentRole::SessionContext,
            event_type: EventType::Suggestion,
            content: "请补充信息".into(),
            usage_input: 0,
            usage_output: 0,
        })
        .unwrap();
        let rows = db.list_session_memory(sid, 10).unwrap();
        assert_eq!(rows.len(), 2);
    }
}
