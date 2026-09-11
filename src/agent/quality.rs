//! Quality-Check Agent:质检层,对 SubAgent-Work / Main-Work / Plan 单元做质量校验。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::agent::extrace::ExecutionTrace;
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
    ///
    /// 2026-09-09 第 05 轮:接收 `ExecutionTrace` 作为辅助判据,让 QC 模型
    /// 既能看实际输出文本,也能看到真实执行证据(工具调用次数 / 失败 / 早终止),
    /// 避免「黑盒判定」误判 silently pass。
    ///
    /// 2026-09-09 第 14 轮:返回 `(QualityReport, Usage)`,把 Quality 调用的 LLM
    /// 用量带回给 Orchestrator 累加到 `total_usage`(修复 stdout 用量与 Debug 报告
    /// 不一致的 Bug)。
    pub async fn check_subagent(
        &self,
        goal: &str,
        unit_scope: &str,
        expected_output: &str,
        actual_output: &str,
        trace: &ExecutionTrace,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let trace_summary = trace.render_prompt();
        // F10(2026-09-10 第 25 轮):判定基准是「本单元职责」,整体目标仅作背景。
        // 此前 prompt 只给整体 goal,QC(真实 LLM)按整体目标判单元产物,SubAgent
        // 只完成了 wf-1 前置检查也被判「核心任务未完成」→ 无效重试风暴
        // (Debug 报告 debug_report_20260910_150805 问题报告 P1)。
        let prompt = format!(
            "【Quality-Check: SubAgent 单元】\n\
             整体目标(仅作背景,不作为本单元判定依据): {goal}\n\
             本单元职责(判定依据): {unit_scope}\n\
             本单元期望输出: {expected_output}\n\
             实际输出: {actual_output}\n\
             \n\
             【执行轨迹】\n{trace_summary}\n\
             \n\
             请基于「本单元职责 + 期望输出 + 实际输出 + 执行轨迹」判定本单元是否完成,**不要用整体目标苛求本单元**\
             (整体目标的其余部分由后续 WorkFlow 单元负责)。按 JSON 输出 verdict/source/issues/suggestion/retryable/evidence。\n\
             判定提示:若轨迹包含 early_terminate / high_error_rate / text_failure_phrase 信号,通常应判 Fail 并把对应信号写入 issues。\n\
             若本单元是“验证预期失败”的负例,底层 Bash 非零本身可能是通过条件;此时必须在 evidence 中说明预期性,并引用最终验收输出 EXPECTED_NEGATIVE_OK。",
        );
        self.run_check(
            prompt,
            AgentRole::SubAgent,
            actual_output,
            session_id,
            Some(trace),
        )
        .await
    }

    /// 校验 Main-Work WorkFlow 计划(返回带 LLM Usage)。
    pub async fn check_main(
        &self,
        goal: &str,
        workflow_json: &str,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let prompt = format!(
            "【Quality-Check: Main-Work 单元】\n目标: {goal}\nWorkFlow JSON: {workflow_json}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。",
        );
        self.run_check(prompt, AgentRole::MainWork, workflow_json, session_id, None)
            .await
    }

    /// 校验 Plan Markdown(返回带 LLM Usage)。
    pub async fn check_plan(
        &self,
        plan_markdown: &str,
        session_id: &str,
    ) -> Result<(QualityReport, Usage)> {
        let prompt = format!(
            "【Quality-Check: Plan 单元】\nPlan Markdown:\n{plan_markdown}\n\n请按 JSON 格式输出 verdict/source/issues/suggestion/retryable/evidence。",
        );
        self.run_check(prompt, AgentRole::Plan, plan_markdown, session_id, None)
            .await
    }

    async fn run_check(
        &self,
        prompt: String,
        source: AgentRole,
        actual: &str,
        session_id: &str,
        trace: Option<&ExecutionTrace>,
    ) -> Result<(QualityReport, Usage)> {
        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = self.agent.run_session(&mut sub_session).await?;
        // P0:fail-closed —— 解析失败时返回 Verdict::Fail + retryable=true,
        // 触发 Yolo 回流,而非 fail-open(默认 Pass)绕过质检门。
        // 设计依据:`docs/Agent源码调研/专题-第八轮-Tool权限策略引擎与沙箱设计深度对比.md`
        // —— 5 态权限状态机 + fail-closed 默认;以及第十四轮 §1.2 错误恢复专题
        // —— silent pass 等于绕过熔断。
        let report = match parse_quality_report(&text, source) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    source = %source.as_str(),
                    "Quality 报告解析失败,fail-closed:返回 Verdict::Fail 触发回流"
                );
                // 落 Agent-Memory 记录原始输出长度,便于未来加入 JSON 修复链
                // (参考 atomcode repair.rs 8 段修复)
                let _ = memory::record_entry(
                    &self.db,
                    AgentRole::QualityCheck,
                    session_id,
                    "parse_failed",
                    &text,
                    Some(&format!("解析失败: {e}")),
                    serde_json::json!({ "raw_output_len": text.len() }),
                );
                QualityReport::fail(
                    source,
                    vec![format!("Quality 报告解析失败: {e}")],
                    "请确保 LLM 输出合法 JSON verdict/report 字段;建议重试或补充上下文。",
                    true,
                )
            }
        };
        let report = match trace {
            Some(trace) => gate_report_on_trace(report, source, trace, actual),
            None => report,
        };

        let _ = memory::record_entry(
            &self.db,
            AgentRole::QualityCheck,
            session_id,
            &format!("check source={}", source.as_str()),
            &format!("verdict={:?}", report.verdict),
            if report.verdict == Verdict::Fail {
                Some(&report.suggestion)
            } else {
                None
            },
            serde_json::json!({ "issues": &report.issues, "retryable": report.retryable }),
        );

        let _ = actual;
        Ok((report, usage))
    }
}

/// 用不可伪造的执行轨迹约束 Quality-Check 的“通过”结论。
///
/// LLM QC 是语义判断,但不能覆盖进程级事实:
/// - 强失败信号(`is_failed()`)出现时,pass 一律降级为 fail;
/// - Bash 非零退出是弱信号:若 QC 想把它解释为预期负例,必须提供非空 evidence;
///   空 evidence 的 pass 视为无效质检,触发回流/失败收口。
///
/// 这样保留“命令预期失败”的测试空间,同时杜绝 D01Q4 这类 exit=1 被静默判成功。
fn gate_report_on_trace(
    report: QualityReport,
    source: AgentRole,
    trace: &ExecutionTrace,
    actual_output: &str,
) -> QualityReport {
    if report.verdict == Verdict::Fail {
        return report;
    }

    let strong_failure = trace.is_failed();
    // 负例测试契约:底层命令非零是“通过条件”,最终断言必须返回零并输出
    // EXPECTED_NEGATIVE_OK。这样避免 QC 模型忘记填 evidence 时,把已经成功验证
    // 的预期负例反复重试;同时仍要求最终验收命令成功,防止用文字口头豁免。
    let expected_negative_confirmed = actual_output.contains("EXPECTED_NEGATIVE_OK")
        && trace.bash_exit_nonzero_count > 0
        && trace.last_bash_exit_code == 0;
    let unevidenced_bash_failure = trace.bash_exit_nonzero_count > 0
        && report.evidence.trim().is_empty()
        && !expected_negative_confirmed;
    if !strong_failure && !unevidenced_bash_failure {
        return report;
    }

    let issue = if strong_failure {
        "执行轨迹包含强失败信号,Quality-Check 的 pass 结论被 trace 证据门拒绝".to_string()
    } else {
        "Bash 命令非零退出且 Quality-Check 未提供非空 evidence 说明预期性".to_string()
    };
    let mut gated = QualityReport::fail(
        source,
        vec![issue],
        "请修正命令或产物后重跑;若非零退出是预期负例,Quality-Check 必须在 evidence 中说明。",
        true,
    );
    gated.evidence = trace.render_prompt();
    gated
}

/// 解析 Quality JSON 输出。
/// 直接解析失败时自动走 JSON 修复链(`json_repair` Tier-1 语法修复);
/// 修复不了仍返回 Err(上轮 P0 fail-closed 语义不变,截断 JSON 刻意不补全)。
pub fn parse_quality_report(text: &str, source: AgentRole) -> Result<QualityReport> {
    if let Some(json_str) = extract_json_block(text) {
        return crate::agent::json_repair::try_parse::<QualityReport>(json_str).map_err(|diag| {
            crate::error::AgentError::Other(format!("Quality JSON 解析失败: {diag}"))
        });
    }
    if let Some(json_str) = extract_standalone_json(text) {
        let mut r: QualityReport =
            crate::agent::json_repair::try_parse(json_str).map_err(|diag| {
                crate::error::AgentError::Other(format!("Quality JSON 解析失败: {diag}"))
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
    if json_text.is_empty() {
        None
    } else {
        Some(json_text)
    }
}

fn extract_standalone_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;
    for (i, c) in text[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' && in_string {
            escape = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i + 1);
                    break;
                }
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
        let r = QualityReport::fail(AgentRole::SubAgent, vec!["x".into()], "再试一次", true);
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

    // ========== P0 fail-closed 测试 ==========
    // 验证 JSON 解析失败时返回 Verdict::Fail + retryable=true
    // (替代历史的 fail-open 默认 Pass)

    #[test]
    fn parse_quality_report_no_json_fails_closed() {
        // 完全无 JSON → 应返回 Err(让上游 fail-closed 转为 Fail)
        let text = "这是一段不含 JSON 的自然语言,无法被解析为质检报告。";
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_truncated_json_fails_closed() {
        // JSON 被截断 / 不合法 → 应返回 Err
        let text = r#"以下是质检报告:
```json
{
  "verdict": "pass",
  "source": "subagent",
  "issues": [],
  // 后面被截断"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("JSON"));
    }

    #[test]
    fn parse_quality_report_missing_required_field_fails_closed() {
        // 缺 verdict 字段 → serde 反序列化失败 → Err
        let text = r#"```json
{
  "source": "subagent",
  "issues": [],
  "retryable": false
}
```"#;
        let err = parse_quality_report(text, AgentRole::SubAgent).unwrap_err();
        assert!(format!("{err}").contains("verdict"));
    }

    #[test]
    fn quality_report_fail_constructor_sets_retryable() {
        // 验证 fail-closed 路径所需的构造函数
        let r = QualityReport::fail(
            AgentRole::SubAgent,
            vec!["Quality 报告解析失败: 字段缺失".to_string()],
            "请确保 LLM 输出合法 JSON verdict/report 字段",
            true,
        );
        assert_eq!(r.verdict, Verdict::Fail);
        assert!(r.retryable);
        assert!(r.issues.iter().any(|i| i.contains("解析失败")));
        assert!(r.suggestion.contains("JSON"));
    }

    #[test]
    fn trace_gate_rejects_empty_evidence_for_bash_nonzero() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("完成");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "完成",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.retryable);
        assert!(gated.evidence.contains("bash_exit_nonzero=1"));
    }

    #[test]
    fn trace_gate_allows_expected_bash_nonzero_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 2;
        trace.collect_failure_signals("负向用例按预期返回非零");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "exit=2 是本负向用例的预期结果".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_strong_failure_even_with_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.early_terminated = true;
        trace.early_terminate_reason = "max_iter:2".into();
        trace.collect_failure_signals("完成");

        let mut report = QualityReport::pass(AgentRole::SubAgent);
        report.evidence = "模型解释这是预期情况".into();
        let gated = gate_report_on_trace(report, AgentRole::SubAgent, &trace, "完成");

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("强失败信号"));
    }

    #[test]
    fn trace_gate_allows_explicit_negative_confirmation_without_qc_evidence() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 0;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let output = "工具输出摘录\nEXPECTED_NEGATIVE_OK\n<exit_code>0</exit_code>";
        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            output,
        );

        assert_eq!(gated.verdict, Verdict::Pass);
    }

    #[test]
    fn trace_gate_rejects_negative_marker_without_final_success() {
        let mut trace = ExecutionTrace::default();
        trace.bash_exit_nonzero_count = 1;
        trace.last_bash_exit_code = 1;
        trace.collect_failure_signals("EXPECTED_NEGATIVE_OK");

        let gated = gate_report_on_trace(
            QualityReport::pass(AgentRole::SubAgent),
            AgentRole::SubAgent,
            &trace,
            "EXPECTED_NEGATIVE_OK",
        );

        assert_eq!(gated.verdict, Verdict::Fail);
        assert!(gated.issues[0].contains("Bash 命令非零退出"));
    }
}
