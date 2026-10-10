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
use crate::llm::{ChatMessage, Usage};
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
        Self {
            agent,
            db,
            plans_dir,
        }
    }

    /// 构造并执行 Plan 生成(2026-09-09 第 14 轮:带回 LLM Usage 用于 Orchestrator 累加)。
    pub async fn generate(
        &self,
        goal: &str,
        purpose: &str,
        intent: &str,
        decomposition: &[String],
        session_id: &str,
    ) -> Result<(PlanOutput, Usage)> {
        self.generate_with_retry_hint(goal, purpose, intent, decomposition, session_id, "", "")
            .await
    }

    /// I3(2026-09-14 第 51 轮):带上一轮失败反馈生成 Plan。
    /// 此前 hard 档 Plan-QC 失败重试时 Plan 看不到 QC 的拒绝理由,盲重生成
    /// 产出几乎相同的方案被再次拒绝,直至 max_retries 整任务失败
    /// (第 51 轮 fl02/fl07/et02/et07/et08 实测簇)。
    pub async fn generate_with_retry_hint(
        &self,
        goal: &str,
        purpose: &str,
        intent: &str,
        decomposition: &[String],
        session_id: &str,
        retry_hint: &str,
        completed_digest: &str,
    ) -> Result<(PlanOutput, Usage)> {
        // 确保 plans/ 存在
        std::fs::create_dir_all(&self.plans_dir)
            .map_err(|e| AgentError::PlanGen(format!("无法创建 plans/ 目录: {}", e)))?;

        let retry_block = if retry_hint.trim().is_empty() {
            String::new()
        } else {
            // 第 149 轮文案纠偏:旧版固定写「上一轮 Quality-Check 拒绝理由」——
            // 但 retry_hint 的真实来源可能是解析校验失败(QC 实际判 pass),
            // 失实归因 + 「必须修复」措辞会构成对模型的施压(实测 2026-10-10:
            // QC 三次 pass,解析门三次以「QC 拒绝」名义要求重出,Plan 明确抗议
            // 「换格式重试只是把同一份方案写得更可解析」)。改为中性、如实表述。
            format!(
                "\n【上一轮失败反馈(可能来自 Quality-Check 或解析校验,如实标注于下)】\n{}\n\
                 若该反馈属于格式/结构问题,请针对性调整输出模板;若属于任务本身无法执行,\
                 请如实说明原因,不要为通过校验而改变结论。\n",
                retry_hint.trim()
            )
        };
        // 第 146 轮:断点续跑 —— 已完成单元清单注入,Plan 只规划剩余部分,
        // 不再从零重写全量方案(实测 v2 全量重写 365s 且把已通过单元重跑了一遍)。
        let completed_block = if completed_digest.trim().is_empty() {
            String::new()
        } else {
            format!(
                "\n【上一轮已完成且 QC 通过的单元(产物已留存,本次方案禁止重复规划)】\n{}\n\
                 只规划剩余目标;需要引用已完成单元结论的步骤,写「复用上轮产物:<要点>」。\n",
                completed_digest.trim()
            )
        };
        let prompt = format!(
            "【Plan 任务】\n\
             Session: {session_id}\n\
             目的: {purpose}\n\
             目标: {goal}\n\
             意图: {intent}\n\
             分解步骤:\n{decomp}\n{retry_block}{completed_block}\n\
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

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;

        // 落盘
        let seq = self.db.next_session_seq(session_id)?;
        let path = self.plans_dir.join(format!("{}-{}.md", session_id, seq));
        std::fs::write(&path, &text)
            .map_err(|e| AgentError::PlanGen(format!("写入 Plan 文档失败: {}", e)))?;

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

        // 运行日志(2026-09-17 第 69 轮):Plan 生成(决策)—— hard 档规划落盘
        tracing::info!(
            agent = "LsmAgentEmergentWork-Plan",
            session = session_id,
            plan_path = %path.display(),
            markdown_chars = text.chars().count(),
            plan_head = %crate::logging::clip_for_log(&text, 400),
            "Plan 生成(决策)"
        );

        let _ = usage;
        Ok((
            PlanOutput {
                path,
                markdown: text,
            },
            usage,
        ))
    }
}

/// 校验 Plan Markdown 是否包含完整五段(目标/WorkFlow/关键决策/风险/验收总览)。
pub fn validate_plan_markdown(content: &str) -> Result<()> {
    let required = ["目标", "WorkFlow 拆解", "关键决策", "风险", "验收总览"];
    for seg in required {
        if !content.contains(seg) && !content.contains(&format!("## {seg}")) {
            return Err(AgentError::PlanGen(format!("Plan 文档缺少必要段: {seg}")));
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
    for entry in std::fs::read_dir(plans_dir)
        .map_err(|e| AgentError::PlanGen(format!("无法读取 plans/ 目录: {}", e)))?
    {
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
