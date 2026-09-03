//! Plan Agent:规划层,为 hard 档任务生成 Markdown 方案。
//!
//! Plan 仅持有 Read / Write 工具,且 Write 仅允许写入 plans/ 目录。
//! 单元完成后,Markdown 落盘到 `plans/{session_id}-{seq}.md`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::context::AgentRole;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::{AgentError, Result};
use crate::llm::ChatMessage;
use crate::session;

/// Plan 生成结果。
#[derive(Debug, Clone)]
pub struct PlanOutput {
    pub path: PathBuf,
    pub markdown: String,
}

/// Plan 执行器。
pub struct PlanRunner {
    agent: Agent,
    db: Arc<Db>,
    plans_dir: PathBuf,
}

impl PlanRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>, plans_dir: PathBuf) -> Self {
        let agent = Agent::new(llm, AgentProfile::plan_profile());
        Self { agent, db, plans_dir }
    }

    /// 构造并执行 Plan 生成。
    pub async fn generate(
        &self,
        goal: &str,
        purpose: &str,
        intent: &str,
        decomposition: &[String],
        session_id: &str,
    ) -> Result<PlanOutput> {
        // 确保 plans/ 存在
        std::fs::create_dir_all(&self.plans_dir).map_err(|e| {
            AgentError::PlanGen(format!("无法创建 plans/ 目录: {}", e))
        })?;

        let prompt = format!(
            "【Plan 任务】\n\
             Session: {session_id}\n\
             目的: {purpose}\n\
             目标: {goal}\n\
             意图: {intent}\n\
             分解步骤:\n{decomp}\n\n\
             请按系统提示词中的 Markdown 模板输出方案(完整五段)。",
            decomp = decomposition
                .iter()
                .enumerate()
                .map(|(i, s)| format!("  {}. {}", i + 1, s))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage) = self.agent.run_session(&mut sub_session).await?;

        // 落盘
        let seq = self.db.next_session_seq(session_id)?;
        let path = self.plans_dir.join(format!("{}-{}.md", session_id, seq));
        std::fs::write(&path, &text).map_err(|e| {
            AgentError::PlanGen(format!("写入 Plan 文档失败: {}", e))
        })?;

        // 写 Agent-Memory
        let _ = memory::record_entry(
            &self.db,
            AgentRole::Plan,
            session_id,
            goal,
            &format!("plan_doc: {}", path.display()),
            None,
            serde_json::json!({ "plan_path": path.to_string_lossy(), "seq": seq }),
        );

        let _ = usage;
        Ok(PlanOutput { path, markdown: text })
    }
}

/// 校验 Plan Markdown 是否包含完整五段(目标/WorkFlow/关键决策/风险/验收总览)。
pub fn validate_plan_markdown(content: &str) -> Result<()> {
    let required = ["目标", "WorkFlow 拆解", "关键决策", "风险", "验收总览"];
    for seg in required {
        if !content.contains(seg) && !content.contains(&format!("## {seg}")) {
            return Err(AgentError::PlanGen(format!(
                "Plan 文档缺少必要段: {seg}"
            )));
        }
    }
    Ok(())
}

/// 解析 plans/ 目录(测试 / 工具用)。
pub fn list_plan_docs(plans_dir: &Path) -> Result<Vec<PathBuf>> {
    if !plans_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(plans_dir).map_err(|e| {
        AgentError::PlanGen(format!("无法读取 plans/ 目录: {}", e))
    })? {
        let entry = entry.map_err(|e| AgentError::PlanGen(e.to_string()))?;
        let p = entry.path();
        if p.is_file() && p.extension().map(|s| s == "md").unwrap_or(false) {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Db, Paths};
    use tempfile::tempdir;

    #[test]
    fn validate_plan_markdown_all_segments() {
        let md = "# 任务方案:x\n\n## 一、目标\n...\n## 二、WorkFlow 拆解\n...\n## 三、关键决策\n...\n## 四、风险\n...\n## 五、验收总览\n...";
        validate_plan_markdown(md).unwrap();
    }

    #[test]
    fn validate_plan_markdown_missing_segment() {
        let md = "# x\n\n## 一、目标\n...";
        assert!(validate_plan_markdown(md).is_err());
    }

    #[test]
    fn list_plan_docs_empty() {
        let dir = tempdir().unwrap();
        let plans = list_plan_docs(dir.path()).unwrap();
        assert!(plans.is_empty());
    }

    #[test]
    fn list_plan_docs_sorted() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "a").unwrap();
        std::fs::write(dir.path().join("b.md"), "b").unwrap();
        std::fs::write(dir.path().join("c.txt"), "ignored").unwrap();
        let plans = list_plan_docs(dir.path()).unwrap();
        assert_eq!(plans.len(), 2);
        assert!(plans[0].ends_with("a.md"));
        assert!(plans[1].ends_with("b.md"));
    }

    #[test]
    fn create_plans_dir_writes() {
        let dir = tempdir().unwrap();
        let plans = dir.path().join("plans");
        std::fs::create_dir_all(&plans).unwrap();
        let (db, _d) = {
            let p = dir.path();
            let paths = Paths::for_test(p);
            (Db::open(&paths).unwrap(), p)
        };
        let _ = (db, plans);
    }
}