//! Plan Agent:规划层,为 hard 档任务生成 Markdown 方案。
//!
//! Plan 持有 Read / Glob / Grep / Bash(只读侦察)/ MCP_Web_LsmAgentBrowser_Use / MCP_Window_Use
//! 观察类工具 —— 规划前先探索(代码类任务读相关文件,网页/桌面类任务观察界面结构),
//! 有充足事实依据后再成文;Write 仅允许写入 plans/ 目录,不修改源代码。
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
        // explore_budget(0):runtime hints 的「进入执行期禁止再只读探查」提示
        // 是为执行层设计的,对 Plan 是反向指令(规划本来就靠先探索后成文)。
        let agent = Agent::new(llm, AgentProfile::plan_profile()).with_explore_budget(0);
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
        user_prompt: &str,
    ) -> Result<(PlanOutput, Usage)> {
        self.generate_with_retry_hint(
            goal,
            purpose,
            intent,
            decomposition,
            session_id,
            "",
            "",
            user_prompt,
        )
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
        user_prompt: &str,
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
        let user_block = if user_prompt.trim().is_empty() {
            String::new()
        } else {
            format!(
                "\n【用户原始提示词(与上面 Yolo 摘要互为补充,细节以原文为准)】\n{}\n",
                user_prompt.trim()
            )
        };
        let prompt = format!(
            "【Plan 任务】\n\
             Session: {session_id}\n\
             目的: {purpose}\n\
             目标: {goal}\n\
             意图: {intent}\n\
             分解步骤:\n{decomp}\n{retry_block}{completed_block}{user_block}\n\
             请先按系统提示词「探索阶段」收集足够事实,再按 Markdown 模板输出方案(完整五段)。",
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

        // 2026-10-16 闲鱼实测:thinking-only 响应产出空文本,空文件写入 plans/
        // 后 QC 判「正文完全缺失」白烧一轮。空文本直接报错,让调用方重试。
        if text.trim().is_empty() {
            return Err(AgentError::PlanGen(
                "Plan 输出为空文本(thinking-only 或异常响应),拒绝写入空方案文件;请重试"
                    .into(),
            ));
        }

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

    /// 再规划 / 更新规划:执行层遭遇规划缺口(登录墙 / 表单 / 访问受阻等)后,
    /// 把失败现场回灌 Plan,在原方案基础上修订受阻部分,而非从零重出方案。
    ///
    /// - `prev_plan_markdown`:原方案 Markdown(调用方截断注入,建议 ≤3000 字符)。
    /// - `replan_context`:再规划触发上下文(触发类别 + 失败单元 + QC 结论 + 失败信号)。
    /// - `completed_digest`:已完成且 QC 通过的单元清单(断点续跑,禁止重复规划)。
    ///
    /// 产物落盘到**新** seq 的 `plans/{session_id}-{seq}.md`(旧方案保留可回溯),
    /// 提示词要求输出仍是完整五段模板(可选「## 六、未决阻碍与再规划触发点」)。
    /// 设计见 `docs/PlanAgent再规划与更新规划/01-设计与解决方案.md` §3.2。
    #[allow(clippy::too_many_arguments)]
    pub async fn replan_update(
        &self,
        goal: &str,
        purpose: &str,
        intent: &str,
        decomposition: &[String],
        session_id: &str,
        user_prompt: &str,
        prev_plan_markdown: &str,
        replan_context: &str,
        completed_digest: &str,
    ) -> Result<(PlanOutput, Usage)> {
        std::fs::create_dir_all(&self.plans_dir)
            .map_err(|e| AgentError::PlanGen(format!("无法创建 plans/ 目录: {}", e)))?;

        let completed_block = if completed_digest.trim().is_empty() {
            String::new()
        } else {
            format!(
                "\n【已完成且 QC 通过的单元(产物已留存,本次方案禁止重复规划)】\n{}\n",
                completed_digest.trim()
            )
        };
        let user_block = if user_prompt.trim().is_empty() {
            String::new()
        } else {
            format!(
                "\n【用户原始提示词(与上面 Yolo 摘要互为补充,细节以原文为准)】\n{}\n",
                user_prompt.trim()
            )
        };
        let prev_block = if prev_plan_markdown.trim().is_empty() {
            String::new()
        } else {
            format!(
                "\n【原方案(在此基础上修订,保留已验证有效的部分)】\n{}\n",
                prev_plan_markdown.trim()
            )
        };
        let prompt = format!(
            "【Plan 再规划 / 更新规划】\n\
             Session: {session_id}\n\
             目的: {purpose}\n\
             目标: {goal}\n\
             意图: {intent}\n\
             分解步骤:\n{decomp}\n\
             【再规划触发上下文(执行层按原方案执行时遭遇的规划缺口,如实转述)】\n{ctx}\n\
             这是**再规划 / 更新规划**,不是从头规划:\n\
             ① 保留原方案中已验证有效的部分,只修订受阻 / 未覆盖的 WorkFlow;\n\
             ② 针对上述缺口补齐处置策略:登录前置步骤(含 request_human 人工介入点)、\n\
             表单分步填写与字段核对、换路径 / 换入口、访问受限时的降级方案;\n\
             ③ 已完成单元禁止重复规划;需要其结论的步骤写「复用上轮产物:<要点>」;\n\
             ④ 仍按系统提示词 Markdown 模板输出完整方案(五段必备,可附「## 六、未决阻碍与再规划触发点」)。\n\
             {completed_block}{user_block}{prev_block}",
            decomp = decomposition
                .iter()
                .enumerate()
                .map(|(i, s)| format!("  {}. {}", i + 1, s))
                .collect::<Vec<_>>()
                .join("\n"),
            ctx = replan_context.trim(),
        );

        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;

        // 2026-10-16 闲鱼实测:空文本守卫(与 generate 同步)
        if text.trim().is_empty() {
            return Err(AgentError::PlanGen(
                "Plan 再规划输出为空文本(thinking-only 或异常响应),拒绝写入空方案文件;请重试"
                    .into(),
            ));
        }

        // 落盘(新 seq,旧方案保留)
        let seq = self.db.next_session_seq(session_id)?;
        let path = self.plans_dir.join(format!("{}-{}.md", session_id, seq));
        std::fs::write(&path, &text)
            .map_err(|e| AgentError::PlanGen(format!("写入 Plan 文档失败: {}", e)))?;

        let _ = memory::record_entry(
            &self.db,
            AgentRole::Plan,
            session_id,
            goal,
            &format!("plan_replan: {}", path.display()),
            None,
            serde_json::json!({ "plan_path": path.to_string_lossy(), "seq": seq, "replan": true }),
        );

        // 运行日志:Plan 再规划(决策)
        tracing::info!(
            agent = "LsmAgentEmergentWork-Plan",
            session = session_id,
            plan_path = %path.display(),
            replan = true,
            markdown_chars = text.chars().count(),
            plan_head = %crate::logging::clip_for_log(&text, 400),
            "Plan 再规划(决策)"
        );

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
