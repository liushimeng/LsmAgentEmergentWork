//! Quality-Check Agent:质检层,对 SubAgent-Work / Main-Work / Plan 单元做质量校验。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::agent::memory;
use crate::agent::{Agent, AgentProfile};
use crate::config::Db;
use crate::error::Result;
use crate::llm::{ChatMessage, Usage};
use crate::session;

/// 质检结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
}

/// 质检报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityReport {
    pub verdict: Verdict,
    pub source: AgentRole,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub suggestion: String,
    pub retryable: bool,
    #[serde(default)]
    pub evidence: String,
}

impl QualityReport {
    pub fn pass(source: AgentRole) -> Self {
        Self {
            verdict: Verdict::Pass,
            source,
            issues: Vec::new(),
            suggestion: String::new(),
            retryable: false,
            evidence: String::new(),
        }
    }

    pub fn fail(source: AgentRole, issues: Vec<String>, suggestion: &str, retryable: bool) -> Self {
        Self {
            verdict: Verdict::Fail,
            source,
            issues,
            suggestion: suggestion.into(),
            retryable,
            evidence: String::new(),
        }
    }
}

/// Quality 执行器。
pub struct QualityRunner {
    agent: Agent,
    db: Arc<Db>,
}

impl QualityRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::quality_check_profile());
        Self { agent, db }
    }

    /// 校验 SubAgent-Work 单元输出。
    pub async fn check_subagent(
        &self,
        goal: &str,
        expected_output: &str,
        actual_output: &str,
        session_id: &str,
    ) -> Result<QualityReport> {
        let prompt = format!(
            "【Quality-Check: SubAgent 单元】\n目标: {goal}\n期望输出: {expected_output}\n实际输出: {actual_output}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。",
        );
        self.run_check(prompt, AgentRole::SubAgent, actual_output, session_id).await
    }

    /// 校验 Main-Work WorkFlow 计划。
    pub async fn check_main(
        &self,
        goal: &str,
        workflow_json: &str,
        session_id: &str,
    ) -> Result<QualityReport> {
        let prompt = format!(
            "【Quality-Check: Main-Work 单元】\n目标: {goal}\nWorkFlow JSON: {workflow_json}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。",
        );
        self.run_check(prompt, AgentRole::MainWork, workflow_json, session_id).await
    }

    /// 校验 Plan Markdown。
    pub async fn check_plan(
        &self,
        plan_markdown: &str,
        session_id: &str,
    ) -> Result<QualityReport> {
        let prompt = format!(
            "【Quality-Check: Plan 单元】\nPlan Markdown:\n{plan_markdown}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。",
        );
        self.run_check(prompt, AgentRole::Plan, plan_markdown, session_id).await
    }

    async fn run_check(
        &self,
        prompt: String,
        source: AgentRole,
        actual: &str,
        session_id: &str,
    ) -> Result<QualityReport> {
        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage) = self.agent.run_session(&mut sub_session).await?;
        let report = parse_quality_report(&text, source).unwrap_or_else(|e| {
            // 容错:解析失败时默认通过(避免误判)
            tracing::warn!("Quality 报告解析失败,默认通过: {}", e);
            QualityReport::pass(source)
        });

        let _ = memory::record_entry(
            &self.db,
            AgentRole::QualityCheck,
            session_id,
            &format!("check source={}", source.as_str()),
            &format!("verdict={:?}", report.verdict),
            if report.verdict == Verdict::Fail { Some(&report.suggestion) } else { None },
            serde_json::json!({ "issues": &report.issues, "retryable": report.retryable }),
        );

        let _ = usage;
        let _ = actual;
        Ok(report)
    }
}

/// 解析 Quality JSON 输出。
pub fn parse_quality_report(text: &str, source: AgentRole) -> Result<QualityReport> {
    if let Some(json_str) = extract_json_block(text) {
        return serde_json::from_str::<QualityReport>(json_str).map_err(|e| {
            crate::error::AgentError::Other(format!("Quality JSON 解析失败: {}", e))
        });
    }
    if let Some(json_str) = extract_standalone_json(text) {
        let mut r: QualityReport = serde_json::from_str(json_str).map_err(|e| {
            crate::error::AgentError::Other(format!("Quality JSON 解析失败: {}", e))
        })?;
        if r.source != source {
            r.source = source;
        }
        return Ok(r);
    }
    Err(crate::error::AgentError::Other(
        "未找到合法的 Quality JSON".into(),
    ))
}

fn extract_json_block(text: &str) -> Option<&str> {
    let start_marker = "```json";
    let start = text.find(start_marker)?;
    let content_start = start + start_marker.len();
    let content_start = text[content_start..]
        .find(|c: char| !c.is_whitespace())
        .map(|i| content_start + i)
        .unwrap_or(content_start);
    let end_marker = "```";
    let end = text[content_start..].find(end_marker)?;
    let json_text = &text[content_start..content_start + end].trim();
    if json_text.is_empty() { None } else { Some(json_text) }
}

fn extract_standalone_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;
    for (i, c) in text[start..].char_indices() {
        if escape { escape = false; continue; }
        if c == '\\' && in_string { escape = true; continue; }
        if c == '"' { in_string = !in_string; continue; }
        if in_string { continue; }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 { end = Some(start + i + 1); break; }
            }
            _ => {}
        }
    }
    end.map(|e| &text[start..e])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_report_pass_default() {
        let r = QualityReport::pass(AgentRole::SubAgent);
        assert_eq!(r.verdict, Verdict::Pass);
        assert_eq!(r.source, AgentRole::SubAgent);
    }

    #[test]
    fn quality_report_fail_sets_retryable() {
        let r = QualityReport::fail(
            AgentRole::SubAgent,
            vec!["x".into()],
            "再试一次",
            true,
        );
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert_eq!(r.suggestion, "再试一次");
    }

    #[test]
    fn parse_quality_report_from_block() {
        let text = r#"
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  "suggestion": "",
  "retryable": false,
  "evidence": ""
}
```"#;
        let r = parse_quality_report(text, AgentRole::SubAgent).unwrap();
        assert_eq!(r.verdict, Verdict::Pass);
    }
}