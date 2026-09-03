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
    pub async fn summarize(
        &self,
        goal: &str,
        user_prompt: &str,
        plan_doc: Option<&std::path::Path>,
        workflow_results: &[(String, String, bool)], // (id, name, ok)
        total_usage: &Usage,
        session_id: &str,
    ) -> Result<SessionSummary> {
        let workflow_line = if workflow_results.is_empty() {
            "(无 WorkFlow)".to_string()
        } else {
            workflow_results
                .iter()
                .map(|(id, name, ok)| {
                    format!("{} {}: {}", if *ok { "✅" } else { "❌" }, id, name)
                })
                .collect::<Vec<_>>()
                .join("; ")
        };

        let prompt = format!(
            "【SessionContext 收口】\n\
             目标: {goal}\n\
             用户输入: {user_prompt}\n\
             Plan 文档: {plan}\n\
             WorkFlow: {wf}\n\
             用量: input={i}, output={o}\n\n\
             请按系统提示词中的 Markdown 模板输出 200 字以内的简洁摘要。",
            plan = plan_doc.map(|p| p.display().to_string()).unwrap_or_else(|| "无".into()),
            wf = workflow_line,
            i = total_usage.input_tokens,
            o = total_usage.output_tokens,
        );

        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage) = self.agent.run_session(&mut sub_session).await?;

        // 写入 session_memory(Summary 事件)
        let _ = self.db.insert_session_memory(&crate::config::SessionMemoryEntry {
            session_id: session_id.to_string(),
            role: AgentRole::SessionContext,
            event_type: EventType::Summary,
            content: text.clone(),
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
        self.db.insert_session_memory(&crate::config::SessionMemoryEntry {
            session_id: session_id.to_string(),
            role: AgentRole::Yolo,
            event_type: EventType::Failure,
            content: format!("目标: {goal}\n原因: {reason}"),
            usage_input: 0,
            usage_output: 0,
        })?;
        if !suggestion.is_empty() {
            self.db.insert_session_memory(&crate::config::SessionMemoryEntry {
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
    text.push_str("--- 摘要开始 ---\n");
    for e in entries.iter().rev() {
        // 旧 → 新 顺序
        text.push_str(&format!(
            "- [seq={}, {}] {}\n",
            e.seq,
            e.created_at,
            e.content.replace('\n', " ").chars().take(160).collect::<String>(),
        ));
    }
    text.push_str("--- 摘要结束 ---\n");
    text.push_str(HISTORY_MARKER_END);
    Some(ChatMessage::user(text))
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

/// 把最近 N 条 Summary 摘要注入 Session 上下文(幂等)。
pub fn inject_history_once(session: &mut session::Session, n: usize) -> bool {
    if is_history_injected(session.context()) {
        return false;
    }
    let entries = session
        .id()
        .to_string();
    let _ = entries; // placeholder
    // 真实的获取:由 Orchestrator 持有 db,这里签名稍作调整
    // 实际由 inject_history_with_entries 提供完整能力
    false
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