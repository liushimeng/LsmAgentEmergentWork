//! Agent-Memory:每个 Agent 独立的记忆层。
//!
//! 单元完成后,Orchestrator 把本次输入/输出/错误/产物摘要写入 `agent_memory` 表;
//! 下次同类 Agent 处理前,`AgentMemory::load()` 从表中加载最近 N 条作为背景知识提示。
//!
//! 与 Agent-Context 的区别见 `docs/多Agent架构重构/01-设计与解决方案.md` §9。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::context::AgentRole;
use crate::config::{AgentMemoryRow, Db};

/// Agent-Memory 单条记录(进程内表示;SQLite 持久化在 `agent_memory` 表)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub agent_role: AgentRole,
    pub session_id: String,
    pub input_summary: String,
    pub output_summary: String,
    pub error_summary: Option<String>,
    pub artifacts: Value,
    pub created_at: String,
}

impl From<AgentMemoryRow> for MemoryEntry {
    fn from(r: AgentMemoryRow) -> Self {
        Self {
            agent_role: r.agent_role,
            session_id: r.session_id,
            input_summary: r.input_summary,
            output_summary: r.output_summary,
            error_summary: r.error_summary,
            artifacts: serde_json::from_str(&r.artifacts).unwrap_or(Value::Null),
            created_at: r.created_at,
        }
    }
}

/// Agent-Memory:同一 (role, session) 下的记忆集合。
#[derive(Debug, Clone, Default)]
pub struct AgentMemory {
    pub role: AgentRole,
    pub session_id: String,
    pub entries: Vec<MemoryEntry>,
}

impl AgentMemory {
    /// 从数据库加载指定 role + session 的最近 N 条记忆(按 id DESC)。
    pub fn load(db: &Arc<Db>, role: AgentRole, session_id: &str, limit: usize) -> Self {
        let rows = db
            .list_agent_memory(role, Some(session_id), limit)
            .unwrap_or_default();
        Self {
            role,
            session_id: session_id.to_string(),
            entries: rows.into_iter().map(MemoryEntry::from).collect(),
        }
    }

    /// 加载 role 的全局最近 N 条(跨 session,用于第一次进入新 session 的经验)。
    pub fn load_global(db: &Arc<Db>, role: AgentRole, limit: usize) -> Self {
        let rows = db.list_agent_memory(role, None, limit).unwrap_or_default();
        Self {
            role,
            session_id: String::new(),
            entries: rows.into_iter().map(MemoryEntry::from).collect(),
        }
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 把记忆渲染成 LLM 提示词(Markdown)。
    ///
    /// 长度受 `max_chars` 控制,默认 800。
    pub fn render_prompt(&self, max_chars: usize) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        out.push_str(&format!(
            "[Agent-Memory: {}] 共 {} 条经验:\n",
            self.role.as_str(),
            self.entries.len()
        ));
        for (i, e) in self.entries.iter().enumerate() {
            let line = if let Some(err) = &e.error_summary {
                format!(
                    "- #{}({}): input={} | output={} | error={}",
                    i + 1,
                    e.created_at,
                    truncate(e.input_summary.as_str(), 30),
                    truncate(e.output_summary.as_str(), 30),
                    truncate(err.as_str(), 30),
                )
            } else {
                format!(
                    "- #{}({}): input={} | output={}",
                    i + 1,
                    e.created_at,
                    truncate(e.input_summary.as_str(), 30),
                    truncate(e.output_summary.as_str(), 30),
                )
            };
            out.push_str(&line);
            out.push('\n');
        }
        truncate(&out, max_chars)
    }
}

/// 写入一条 Agent-Memory 记录(供 Orchestrator 调用)。
pub fn record_entry(
    db: &Arc<Db>,
    role: AgentRole,
    session_id: &str,
    input_summary: &str,
    output_summary: &str,
    error_summary: Option<&str>,
    artifacts: Value,
) -> crate::error::Result<()> {
    db.insert_agent_memory(&crate::config::AgentMemoryEntry {
        session_id: session_id.to_string(),
        agent_role: role,
        input_summary: truncate(input_summary, 160),
        output_summary: truncate(output_summary, 160),
        error_summary: error_summary.map(|s| truncate(s, 160)),
        artifacts: artifacts.to_string(),
    })?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    // 留 3 字符给省略号
    let p: String = s.chars().take(max.saturating_sub(3)).collect();
    format!("{p}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Db, Paths};
    use tempfile::tempdir;

    fn fresh_db() -> (Arc<Db>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let paths = Paths::for_test(dir.path());
        let db = Db::open(&paths).unwrap();
        (Arc::new(db), dir)
    }

    #[test]
    fn record_and_load() {
        let (db, _d) = fresh_db();
        record_entry(
            &db,
            AgentRole::SubAgent,
            "s-1",
            "读取 src/foo.rs",
            "已读取 100 行",
            None,
            serde_json::json!({"unit_id": "wf-1"}),
        )
        .unwrap();

        let mem = AgentMemory::load(&db, AgentRole::SubAgent, "s-1", 10);
        assert_eq!(mem.entries.len(), 1);
        assert_eq!(mem.entries[0].input_summary, "读取 src/foo.rs");
        assert!(mem.entries[0].error_summary.is_none());
    }

    #[test]
    fn render_prompt_empty() {
        let mem = AgentMemory {
            role: AgentRole::Yolo,
            session_id: "s".into(),
            entries: vec![],
        };
        assert!(mem.render_prompt(100).is_empty());
        assert!(mem.is_empty());
    }

    #[test]
    fn render_prompt_truncates() {
        let mem = AgentMemory {
            role: AgentRole::SubAgent,
            session_id: "s".into(),
            entries: vec![MemoryEntry {
                agent_role: AgentRole::SubAgent,
                session_id: "s".into(),
                input_summary: "x".repeat(200),
                output_summary: "y".repeat(200),
                error_summary: None,
                artifacts: serde_json::json!({}),
                created_at: "2026-09-03 15:30:00".into(),
            }],
        };
        let s = mem.render_prompt(120);
        assert!(s.chars().count() <= 120);
        assert!(s.contains("[Agent-Memory: subagent]"));
    }
}