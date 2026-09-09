//! Debug 模式与 Debug Agent(`LsmAgentEmergentWork-Debug`)。
//!
//! 开启方式:`laew -debug`(等价 `laew --debug`),可与 `-p` / `-f` / TUI 组合。
//!
//! 三层结构:
//! - [`DebugCollector`]:内存态调试事件采集器(强类型 [`DebugEvent`] 列表,Mutex 保护)。
//! - [`DebugLlmClient`]:LLM 客户端装饰器,对每次 `complete()` 打点
//!   (Agent 名 / 输入输出摘要 / 耗时 / token 用量 / 错误),对 6 大 Runner 零侵入。
//! - [`DebugRunner`]:第 7 个 Agent,任务结束后对 trace 做 LLM 评估,
//!   产出「任务评估 / 质量报告 / 问题报告 / 优化建议」四章节。
//!
//! 报告落盘:根目录 `DebugReport/debug_report_{YYYYMMDD}_{HHMMSS}_{rand6}.md`。
//!
//! 设计见 `docs/Debug模式与DebugAgent设计/01-设计与解决方案.md`。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::agent::context::AgentRole;
use crate::agent::quality::{QualityReport, Verdict};
use crate::agent::yolo::TaskClassification;
use crate::agent::{Agent, AgentProfile};
use crate::error::Result;
use crate::llm::{ChatMessage, Completion, LlmClient, RequestMeta, ToolDef, Usage};

/// 单字段摘要最大字符数(防止超大 trace 撑爆报告)。
const MAX_FIELD_CHARS: usize = 4000;
/// 喂给 Debug Agent 评估的 trace 最大字符数。
const MAX_TRACE_FOR_EVAL_CHARS: usize = 12000;

// =================== 调试事件 ===================

/// 调试事件(强类型,借鉴 pi schema-driven telemetry 设计)。
#[derive(Debug, Clone)]
pub enum DebugEvent {
    /// 一次 LLM 调用(任意 Agent)
    LlmCall {
        /// Agent 名(从系统提示词识别,识别失败为 "unknown")
        agent: String,
        /// 全局序号(从 1 开始)
        seq: usize,
        /// 开始时间(可读)
        started_at: String,
        /// 耗时(毫秒)
        duration_ms: u128,
        /// 输入摘要(最后一条 user 消息,截断)
        input_summary: String,
        /// 上下文消息条数
        message_count: usize,
        /// 可用工具数
        tool_count: usize,
        /// 输出摘要(文本 + 工具调用,截断)
        output_summary: String,
        /// token 用量
        usage: Usage,
        /// 终止原因
        stop_reason: Option<String>,
        /// 调用失败时的错误信息
        error: Option<String>,
    },
    /// Yolo 分类结果
    Classification {
        level: String,
        goal: String,
        intent: String,
        purpose: String,
    },
    /// 一次 Quality-Check 结论
    QualityCheck {
        source: AgentRole,
        verdict: Verdict,
        issues: Vec<String>,
        suggestion: String,
    },
    /// 任务终态
    TaskEnd {
        outcome: String,
        total_usage: Usage,
        total_duration_ms: u128,
    },
}

// =================== 采集器 ===================

/// 采集器内部可变状态(单 Mutex 保护,便于整体 reset)。
#[derive(Debug)]
struct CollectorState {
    session_id: String,
    started_at: String,
    timer: Instant,
    events: Vec<DebugEvent>,
}

/// 调试事件采集器(内存态,线程安全;TUI 多任务时通过 [`reset`](Self::reset) 复用)。
#[derive(Debug)]
pub struct DebugCollector {
    inner: Mutex<CollectorState>,
}

impl DebugCollector {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            inner: Mutex::new(CollectorState {
                session_id: session_id.into(),
                started_at: crate::session::now_readable(),
                timer: Instant::now(),
                events: Vec::new(),
            }),
        }
    }

    /// 重置采集器(TUI 模式每个用户任务开始前调用,重新开始计时与事件列表)。
    pub fn reset(&self, session_id: impl Into<String>) {
        let mut g = self.inner.lock().expect("debug collector");
        g.session_id = session_id.into();
        g.started_at = crate::session::now_readable();
        g.timer = Instant::now();
        g.events.clear();
    }

    pub fn session_id(&self) -> String {
        self.inner
            .lock()
            .expect("debug collector")
            .session_id
            .clone()
    }

    pub fn started_at(&self) -> String {
        self.inner
            .lock()
            .expect("debug collector")
            .started_at
            .clone()
    }

    pub fn elapsed_ms(&self) -> u128 {
        self.inner
            .lock()
            .expect("debug collector")
            .timer
            .elapsed()
            .as_millis()
    }

    fn push(&self, ev: DebugEvent) {
        self.inner.lock().expect("debug collector").events.push(ev);
    }

    /// 记录 Yolo 分类结果。
    pub fn record_classification(&self, c: &TaskClassification) {
        self.push(DebugEvent::Classification {
            level: c.task_level.as_str().to_string(),
            goal: c.goal_summary.clone(),
            intent: c.intent.clone(),
            purpose: c.purpose.clone(),
        });
    }

    /// 记录一次 QC 结论。
    pub fn record_quality(&self, report: &QualityReport) {
        self.push(DebugEvent::QualityCheck {
            source: report.source,
            verdict: report.verdict,
            issues: report.issues.clone(),
            suggestion: report.suggestion.clone(),
        });
    }

    /// 记录任务终态。
    pub fn record_task_end(&self, outcome: impl Into<String>, total_usage: Usage) {
        self.push(DebugEvent::TaskEnd {
            outcome: outcome.into(),
            total_usage,
            total_duration_ms: self.elapsed_ms(),
        });
    }

    /// 事件快照(克隆)。
    pub fn events(&self) -> Vec<DebugEvent> {
        self.inner.lock().expect("debug collector").events.clone()
    }

    /// 统计总览 Markdown。
    pub fn render_stats(&self) -> String {
        let events = self.events();
        let llm_calls = events
            .iter()
            .filter(|e| matches!(e, DebugEvent::LlmCall { .. }))
            .count();
        let errors = events
            .iter()
            .filter(|e| matches!(e, DebugEvent::LlmCall { error: Some(_), .. }))
            .count();
        let qc_total = events
            .iter()
            .filter(|e| matches!(e, DebugEvent::QualityCheck { .. }))
            .count();
        let qc_pass = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DebugEvent::QualityCheck {
                        verdict: Verdict::Pass,
                        ..
                    }
                )
            })
            .count();
        let mut input_tokens = 0u32;
        let mut output_tokens = 0u32;
        let mut total_ms = 0u128;
        for e in &events {
            if let DebugEvent::LlmCall {
                usage, duration_ms, ..
            } = e
            {
                input_tokens = input_tokens.saturating_add(usage.input_tokens);
                output_tokens = output_tokens.saturating_add(usage.output_tokens);
                total_ms = total_ms.saturating_add(*duration_ms);
            }
        }
        format!(
            "| 指标 | 值 |\n|---|---|\n\
             | LLM 调用次数 | {llm_calls} |\n\
             | LLM 调用失败数 | {errors} |\n\
             | LLM 累计耗时 | {total_ms} ms |\n\
             | 输入 token 合计 | {input_tokens} |\n\
             | 输出 token 合计 | {output_tokens} |\n\
             | QC 检查次数 | {qc_total} |\n\
             | QC 通过次数 | {qc_pass} |\n\
             | 任务总耗时 | {} ms |",
            self.elapsed_ms()
        )
    }

    /// 渲染原始 trace 明细 Markdown(落盘附录用,全量)。
    pub fn render_trace(&self) -> String {
        let mut out = String::new();
        for (i, e) in self.events().iter().enumerate() {
            out.push_str(&format!("### 事件 #{}\n\n", i + 1));
            match e {
                DebugEvent::LlmCall {
                    agent,
                    seq,
                    started_at,
                    duration_ms,
                    input_summary,
                    message_count,
                    tool_count,
                    output_summary,
                    usage,
                    stop_reason,
                    error,
                } => {
                    out.push_str(&format!(
                        "- 类型: LLM 调用\n- Agent: `{agent}`\n- 调用序号: {seq}\n- 时间: {started_at}\n\
                         - 耗时: {duration_ms} ms\n- 上下文消息数: {message_count}\n- 可用工具数: {tool_count}\n\
                         - token: input={} output={} cache_read={}\n- stop_reason: {}\n",
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_input_tokens,
                        stop_reason.as_deref().unwrap_or("(无)"),
                    ));
                    if let Some(err) = error {
                        out.push_str(&format!("- **错误**: {err}\n"));
                    }
                    out.push_str(&format!("\n输入摘要:\n\n```text\n{input_summary}\n```\n"));
                    out.push_str(&format!(
                        "\n输出摘要:\n\n```text\n{output_summary}\n```\n\n"
                    ));
                }
                DebugEvent::Classification {
                    level,
                    goal,
                    intent,
                    purpose,
                } => {
                    out.push_str(&format!(
                        "- 类型: Yolo 分类\n- 档位: **{level}**\n- 目标: {goal}\n- 意图: {intent}\n- 目的: {purpose}\n\n"
                    ));
                }
                DebugEvent::QualityCheck {
                    source,
                    verdict,
                    issues,
                    suggestion,
                } => {
                    out.push_str(&format!(
                        "- 类型: Quality-Check\n- 被检对象: {}\n- 结论: **{}**\n",
                        source.as_str(),
                        if *verdict == Verdict::Pass {
                            "pass"
                        } else {
                            "fail"
                        },
                    ));
                    if !issues.is_empty() {
                        out.push_str(&format!("- 问题: {}\n", issues.join("; ")));
                    }
                    if !suggestion.is_empty() {
                        out.push_str(&format!("- 建议: {suggestion}\n"));
                    }
                    out.push('\n');
                }
                DebugEvent::TaskEnd {
                    outcome,
                    total_usage,
                    total_duration_ms,
                } => {
                    out.push_str(&format!(
                        "- 类型: 任务终态\n- 结果: {outcome}\n- 总耗时: {total_duration_ms} ms\n\
                         - 总 token: input={} output={}\n\n",
                        total_usage.input_tokens, total_usage.output_tokens,
                    ));
                }
            }
        }
        out
    }
}

// =================== LLM 装饰器 ===================

/// 已知 Agent 名称列表(用于从系统提示词识别调用方)。
const KNOWN_AGENT_NAMES: &[&str] = &[
    crate::agent::profile::YOLO_AGENT_NAME,
    crate::agent::profile::PLAN_AGENT_NAME,
    crate::agent::profile::MAIN_WORK_AGENT_NAME,
    crate::agent::profile::SUB_AGENT_WORK_NAME,
    crate::agent::profile::QUALITY_CHECK_AGENT_NAME,
    crate::agent::profile::SESSION_CONTEXT_AGENT_NAME,
    crate::agent::profile::DEBUG_AGENT_NAME,
];

/// 从系统提示词中识别 Agent 名(各 profile 提示词均含自身名称,由单测保证)。
fn detect_agent_name(system: &str) -> String {
    for name in KNOWN_AGENT_NAMES {
        if system.contains(name) {
            return (*name).to_string();
        }
    }
    "unknown".to_string()
}

/// 截取最后一条 user 消息的文本作为输入摘要。
fn summarize_input(messages: &[ChatMessage]) -> String {
    for msg in messages.iter().rev() {
        if msg.role == crate::llm::Role::User {
            let mut text = String::new();
            for block in &msg.content {
                if let crate::llm::ContentBlock::Text { text: t } = block {
                    text.push_str(t);
                }
            }
            if !text.is_empty() {
                return truncate_chars(&text, MAX_FIELD_CHARS);
            }
        }
    }
    "(无 user 消息)".to_string()
}

/// 汇总 Completion 输出为摘要文本。
fn summarize_output(c: &Completion) -> String {
    let mut out = String::new();
    if !c.text.is_empty() {
        out.push_str(&truncate_chars(&c.text, MAX_FIELD_CHARS));
    }
    for call in &c.tool_calls {
        out.push_str(&format!(
            "\n[tool_call] {}({})",
            call.name,
            truncate_chars(&call.arguments.to_string(), 500)
        ));
    }
    if out.is_empty() {
        out.push_str("(空输出)");
    }
    out
}

/// 按字符数截断(防溢出)。
fn truncate_chars(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        format!(
            "{}…(截断,原 {} 字符)",
            chars[..max].iter().collect::<String>(),
            chars.len()
        )
    }
}

/// LLM 客户端装饰器:打点每次 complete() 并写入 [`DebugCollector`]。
pub struct DebugLlmClient {
    inner: Arc<dyn LlmClient>,
    collector: Arc<DebugCollector>,
    seq: Mutex<usize>,
}

impl DebugLlmClient {
    pub fn new(inner: Arc<dyn LlmClient>, collector: Arc<DebugCollector>) -> Self {
        Self {
            inner,
            collector,
            seq: Mutex::new(0),
        }
    }

    fn next_seq(&self) -> usize {
        let mut g = self.seq.lock().expect("debug seq");
        *g += 1;
        *g
    }
}

#[async_trait::async_trait]
impl LlmClient for DebugLlmClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion> {
        let seq = self.next_seq();
        let started_at = crate::session::now_readable();
        let timer = Instant::now();
        let result = self.inner.complete(system, messages, tools, meta).await;
        let duration_ms = timer.elapsed().as_millis();

        let (output_summary, usage, stop_reason, error) = match &result {
            Ok(c) => (summarize_output(c), c.usage, c.stop_reason.clone(), None),
            Err(e) => (
                "(调用失败)".to_string(),
                Usage::default(),
                None,
                Some(e.to_string()),
            ),
        };

        self.collector.push(DebugEvent::LlmCall {
            agent: detect_agent_name(system),
            seq,
            started_at,
            duration_ms,
            input_summary: summarize_input(messages),
            message_count: messages.len(),
            tool_count: tools.len(),
            output_summary,
            usage,
            stop_reason,
            error,
        });

        result
    }

    fn protocol(&self) -> crate::config::Protocol {
        self.inner.protocol()
    }
}

// =================== Debug Agent Runner ===================

/// Debug Agent 执行器:对 trace 做 LLM 评估,产出报告正文章节。
pub struct DebugRunner {
    agent: Agent,
}

impl DebugRunner {
    pub fn new(llm: Arc<dyn LlmClient>) -> Self {
        // Debug Agent 只做一次性文本评估,限制迭代次数防跑偏
        let agent = Agent::new(llm, AgentProfile::debug_profile()).with_max_iterations(2);
        Self { agent }
    }

    /// 对 trace Markdown 做评估,返回「任务评估 / 质量报告 / 问题报告 / 优化建议」Markdown。
    ///
    /// `session_id` 为被评估任务的主会话 ID(X-Session-Id 传播,2026-09-09 第 08 轮):
    /// Debug Agent 的请求在抓包中可关联到所属任务(此前 `run_once` 生成随机新 ID)。
    pub async fn evaluate(
        &self,
        session_id: &str,
        trace_markdown: &str,
        stats_markdown: &str,
    ) -> Result<String> {
        let prompt = format!(
            "以下是 laew 多 Agent 系统一次任务的调试 trace。请完成评估。\n\n\
             【统计总览】\n{stats_markdown}\n\n\
             【原始 trace(可能截断)】\n{}\n\n\
             请严格按四章节输出 Markdown:## 任务评估 / ## 质量报告 / ## 问题报告(问题按 P0/P1/P2 分级) / ## 优化建议。",
            truncate_chars(trace_markdown, MAX_TRACE_FOR_EVAL_CHARS),
        );
        let mut session = crate::session::Session::new();
        session.id = session_id.to_string();
        session
            .context_mut()
            .push(crate::llm::ChatMessage::user(&prompt));
        let (text, _usage, _trace) = self.agent.run_session(&mut session).await?;
        Ok(text)
    }
}

// =================== 报告生成与落盘 ===================

/// 报告元信息(头部展示)。
pub struct ReportMeta {
    /// 运行模式描述,如 "-p 单轮" / "-f 文件" / "TUI 多轮"
    pub mode: String,
    /// 用户任务描述(截断展示)
    pub task: String,
    /// 当前模型描述,如 "anthropic my-provider/claude-x @ https://..."
    pub model: String,
}

/// 生成报告文件名:`debug_report_{YYYYMMDD}_{HHMMSS}_{rand6}.md`。
pub fn report_file_name() -> String {
    let now = crate::session::now_readable(); // YYYYMMDD-HHMMSS
    let compact = now.replace('-', "_"); // YYYYMMDD_HHMMSS
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seed = nanos ^ (std::process::id() as u64);
    let mut hasher = Sha256::new();
    hasher.update(seed.to_le_bytes());
    let bytes = hasher.finalize();
    format!(
        "debug_report_{}_{:02x}{:02x}{:02x}.md",
        compact, bytes[0], bytes[1], bytes[2]
    )
}

/// 脱敏:替换 Bearer token / api_key / sk-* 等形状(借鉴 atomcode scrub.rs emit 端 redact)。
pub fn scrub_secrets(text: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"(?i)bearer\s+[A-Za-z0-9_\-\.]{8,}").expect("re"),
            Regex::new(r#"(?i)(api[_-]?key["'\s:=]+)[A-Za-z0-9_\-\.]{8,}"#).expect("re"),
            // 前置非字母边界,防 task-management-system 中 "sk-" 子串误命中(借鉴 atomcode scrub.rs 误报保护)
            Regex::new(r"(^|[^A-Za-z])sk-[A-Za-z0-9_\-]{8,}").expect("re"),
        ]
    });
    let mut out = text.to_string();
    for re in PATTERNS.iter() {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                if let Some(prefix) = caps.get(1) {
                    format!("{}****REDACTED", prefix.as_str())
                } else {
                    "****REDACTED".to_string()
                }
            })
            .to_string();
    }
    out
}

/// 生成并落盘 Debug 报告,返回文件路径。
///
/// - `collector`: 本次任务的事件采集器
/// - `llm`: 真实 LLM 客户端(用于 Debug Agent 评估;**不要**传装饰后的,避免自我采集递归)
/// - `report_dir`: 报告目录(根目录/DebugReport)
pub async fn finalize_report(
    collector: &Arc<DebugCollector>,
    llm: Arc<dyn LlmClient>,
    report_dir: &Path,
    meta: &ReportMeta,
) -> Result<PathBuf> {
    std::fs::create_dir_all(report_dir)?;

    let stats = collector.render_stats();
    let trace = collector.render_trace();

    // Debug Agent 评估;失败不阻塞报告落盘 —— 改为填入「降级骨架」+ 头部横幅提示。
    // 关联报告: 2026-09-09_07 F-007-2
    // session_id 取自采集器(第 08 轮):Debug 请求与被评估任务同 X-Session-Id
    let (evaluation, degraded) = match DebugRunner::new(llm)
        .evaluate(&collector.session_id(), &trace, &stats)
        .await
    {
        Ok(text) => (text, false),
        Err(e) => (
            render_degraded_evaluation(&anyhow::Error::from(e), collector),
            true,
        ),
    };
    let banner = if degraded {
        "\n> ⚠️ **Debug Agent 评估失败,已降级到基于 trace 的自检骨架** — 下方「任务评估 / 质量报告 / 问题报告 / 优化建议」\
         章节由 `src/agent/debug.rs::render_degraded_evaluation` 根据 trace 指标自动归类,\
         **不代表真实 LLM 评估意见**。请检查 Provider 配置 / 网络 / Mock LLM 是否正常。\n"
    } else {
        ""
    };

    let content = format!(
        "# laew Debug 报告\n\n\
         - 生成时间: {}\n- Session ID: `{}`\n- 运行模式: {}\n- 当前模型: {}\n- 任务: {}\n{}\n\n\
         ## 一、统计总览\n\n{}\n\n\
         ## 二、Debug Agent 评估\n\n{}\n\n\
         ## 三、原始 Trace 附录\n\n{}\n",
        crate::session::now_readable(),
        collector.session_id(),
        meta.mode,
        meta.model,
        truncate_chars(&meta.task, 500),
        banner,
        stats,
        evaluation,
        trace,
    );

    let path = report_dir.join(report_file_name());
    std::fs::write(&path, scrub_secrets(&content))?;
    Ok(path)
}

/// Debug Agent 评估失败时的降级骨架 —— 基于 trace 指标做最朴素的归类,
/// 仍按 Debug Agent prompt 要求的 4 章节输出 Markdown,只是把 LLM 的语义评估
/// 替换为「从数字看得见的现象」。这不是为了替代 LLM 评估,而是为了让报告
/// 在 Debug Agent 不可用时仍有可读的章节结构。
/// 关联报告: 2026-09-09_07 F-007-2
fn render_degraded_evaluation(err: &anyhow::Error, collector: &Arc<DebugCollector>) -> String {
    use DebugEvent as E;
    let events = collector.events();
    let llm_calls = events
        .iter()
        .filter(|e| matches!(e, E::LlmCall { .. }))
        .count();
    let llm_errors = events
        .iter()
        .filter(|e| matches!(e, E::LlmCall { error: Some(_), .. }))
        .count();
    let qc_total = events
        .iter()
        .filter(|e| matches!(e, E::QualityCheck { .. }))
        .count();
    let qc_pass = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                E::QualityCheck {
                    verdict: crate::agent::quality::Verdict::Pass,
                    ..
                }
            )
        })
        .count();
    let qc_fail = qc_total.saturating_sub(qc_pass);
    let total_ms: u128 = events
        .iter()
        .filter_map(|e| {
            if let E::LlmCall { duration_ms, .. } = e {
                Some(*duration_ms)
            } else {
                None
            }
        })
        .sum();
    let task_ms = collector.elapsed_ms();

    // P0/P1/P2 自检分级(按数字可见信号):
    //   P0: LLM 错误率 >= 50% / QC 失败 >= 1
    //   P1: 总耗时 > 5s / 截断续接 >= 1 / overflow 恢复 >= 1
    //   P2: 仅事件极少量或无 QC(链路未跑完整)
    // 用 (String, String) 持有自有 buffer,避免 format! 临时值借用问题。
    let mut issues: Vec<(String, String)> = Vec::new(); // (level, desc)
    if qc_fail >= 1 {
        issues.push((
            "P0".into(),
            format!("QC 失败 {qc_fail} 次 —— 至少一个 SubAgent 单元未通过质检"),
        ));
    }
    if llm_calls > 0 && llm_errors * 2 >= llm_calls {
        issues.push((
            "P0".into(),
            format!("LLM 错误率 {llm_errors}/{llm_calls} ≥ 50% —— Provider / 网络疑似异常"),
        ));
    }
    if llm_errors > 0 {
        issues.push((
            "P1".into(),
            format!("LLM 错误 {llm_errors} 次(数字同 P0 时合并显示;单独出现归 P1)"),
        ));
    }
    if task_ms > 5000 {
        issues.push((
            "P1".into(),
            format!("任务总耗时 {task_ms} ms 超过 5s —— 可能存在慢请求或上下文压缩未生效"),
        ));
    }

    // 恢复与截断属于已自动处理的弱信号:不升级为故障,但必须在降级报告中
    // 明确呈现,否则 Debug Agent 不可用时用户无法知道链路曾经自愈。
    let recovery_hints: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            E::LlmCall {
                input_summary,
                output_summary,
                ..
            } => {
                let text = format!("{input_summary}\n{output_summary}");
                let mut signals = Vec::new();
                if text.contains("truncation_resumes") || text.contains("max_tokens") {
                    signals.push("检测到截断/续接提示");
                }
                if text.contains("overflow_recoveries") || text.contains("prompt-too-long") {
                    signals.push("检测到上下文溢出恢复提示");
                }
                if signals.is_empty() {
                    None
                } else {
                    Some(signals.join("、"))
                }
            }
            _ => None,
        })
        .collect();
    if !recovery_hints.is_empty() {
        issues.push((
            "P1".into(),
            format!(
                "自动恢复/截断事件: {} —— 属于已处理的可观测弱信号",
                recovery_hints.join("；")
            ),
        ));
    }
    if llm_calls == 0 {
        issues.push((
            "P2".into(),
            "无任何 LLM 调用 —— 任务在进入 Orchestrator 之前已结束(Provider 未配置 / 解析失败)"
                .into(),
        ));
    }
    if qc_total == 0 && llm_calls > 0 {
        issues.push((
            "P2".into(),
            "链路无 QC 环节 —— SubAgent 输出未经质检直接交付".into(),
        ));
    }

    let issues_md = if issues.is_empty() {
        "- (无)".to_string()
    } else {
        issues
            .iter()
            .map(|(lvl, desc)| format!("- **{lvl}**: {desc}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // 优化建议按 issues 反推
    let suggestions = if issues.is_empty() {
        "- 维持现状,当前 trace 未发现明显异常。\n- 可在 Debug Agent 可用后重跑该任务做语义评估补全。"
    } else {
        "- 优先排查标记为 P0 的项(Provider 凭证 / QC 失败原因);\n- P1 项可结合完整 trace 附录判断是否影响功能;\n- 如 Debug Agent 长期不可用,可在 docs/Debug模式与DebugAgent设计/ 增补离线分析脚本。"
    };

    format!(
        "## 任务评估\n\n- 评估来源: **⚠️ 降级模式**(Debug Agent 调用失败: `{err}`)\n- 总览: 本次任务共 LLM 调用 {llm_calls} 次(失败 {llm_errors})、QC {qc_total} 次(通过 {qc_pass})、累计 LLM 耗时 {total_ms} ms、任务总耗时 {task_ms} ms。\n\n\
         ## 质量报告\n\n- LLM 链路: 调用 {llm_calls} 次 / 失败 {llm_errors} 次\n- QC 链路: {qc_pass}/{qc_total} 通过\n- 耗时分布: LLM 累计 {total_ms} ms / 任务总 {task_ms} ms\n\n\
         ## 问题报告(按 P0/P1/P2 分级)\n\n{issues_md}\n\n\
         ## 优化建议\n\n{suggestions}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_file_name_format() {
        let name = report_file_name();
        assert!(name.starts_with("debug_report_"));
        assert!(name.ends_with(".md"));
        // debug_report_YYYYMMDD_HHMMSS_rand6.md → 按下划线切分应为 4 段
        let stem = name.trim_end_matches(".md");
        let parts: Vec<&str> = stem.split('_').collect();
        assert_eq!(parts.len(), 5, "实际: {name}"); // debug, report, date, time, rand
        assert_eq!(parts[2].len(), 8, "日期段应为 8 位: {name}");
        assert_eq!(parts[3].len(), 6, "时间段应为 6 位: {name}");
        assert_eq!(parts[4].len(), 6, "随机段应为 6 位 hex: {name}");
        // 两次生成应不同(随机数)
        assert_ne!(name, report_file_name());
    }

    #[test]
    fn scrub_secrets_masks_tokens() {
        let s = "Authorization: Bearer sk-abcdef1234567890 api_key=xyz9876543210abc";
        let out = scrub_secrets(s);
        assert!(!out.contains("abcdef1234567890"), "sk- 形状应被脱敏: {out}");
        assert!(out.contains("****REDACTED"));
    }

    #[test]
    fn scrub_secrets_keeps_normal_text() {
        let s = "任务管理系统 task-management-system 正常运行";
        assert_eq!(scrub_secrets(s), s);
    }

    /// F-007-2 降级骨架 —— 直接构造一个空 DebugCollector,验证降级评估渲染包含 4 章节 + P0/P1/P2 自检。
    #[test]
    fn render_degraded_evaluation_has_four_sections() {
        let c = Arc::new(DebugCollector::new("sess-degrade-test"));
        let err = anyhow::anyhow!("mock LLM 未启动 / 端口不通");
        let md = render_degraded_evaluation(&err, &c);
        assert!(md.contains("## 任务评估"), "应包含「任务评估」章节");
        assert!(md.contains("## 质量报告"), "应包含「质量报告」章节");
        assert!(md.contains("## 问题报告"), "应包含「问题报告」章节");
        assert!(md.contains("## 优化建议"), "应包含「优化建议」章节");
        assert!(md.contains("降级模式"), "应明确标注降级模式");
        assert!(md.contains("P2"), "空 trace 应触发 P2 自检(无 LLM 调用)");
    }

    #[test]
    fn degraded_evaluation_mentions_recovery_hints() {
        let c = Arc::new(DebugCollector::new("sess-recovery-test"));
        c.push(DebugEvent::LlmCall {
            agent: "SubAgent".into(),
            seq: 1,
            started_at: "2026-09-09 00:00:00".into(),
            duration_ms: 10,
            input_summary:
                "<<<LAEW:RUNTIME_HINTS>>> truncation_resumes=1 overflow_recoveries=1".into(),
            message_count: 1,
            tool_count: 0,
            output_summary: "完成".into(),
            usage: Usage::default(),
            stop_reason: Some("end_turn".into()),
            error: None,
        });
        let md = render_degraded_evaluation(&anyhow::anyhow!("Debug Agent 不可用"), &c);
        assert!(md.contains("自动恢复/截断事件"));
        assert!(md.contains("截断/续接"));
        assert!(md.contains("上下文溢出恢复"));
    }

    #[test]
    fn collector_records_and_renders() {
        let c = DebugCollector::new("sess-1");
        c.record_classification(&TaskClassification {
            task_level: crate::agent::yolo::TaskLevel::Simple,
            purpose: "测试目的".into(),
            goal_summary: "测试目标".into(),
            intent: "test".into(),
            agent_role: None,
            decomposition_plan: vec![],
            direct_answer: Some("答".into()),
            user_suggestion_if_fail: String::new(),
            yolo_degraded: false, // 关联报告: 2026-09-09_04 D-002
        });
        c.record_quality(&QualityReport::pass(AgentRole::SubAgent));
        c.record_task_end("executed", Usage::default());
        let events = c.events();
        assert_eq!(events.len(), 3);
        let trace = c.render_trace();
        assert!(trace.contains("Yolo 分类"));
        assert!(trace.contains("Quality-Check"));
        assert!(trace.contains("任务终态"));
        let stats = c.render_stats();
        assert!(stats.contains("QC 通过次数 | 1"));
    }

    #[test]
    fn detect_agent_name_from_prompt() {
        let prompt = format!(
            "你是 {},用户对话的第一层入口 Agent。",
            crate::agent::profile::YOLO_AGENT_NAME
        );
        assert_eq!(
            detect_agent_name(&prompt),
            crate::agent::profile::YOLO_AGENT_NAME
        );
        assert_eq!(detect_agent_name("你是某个无名助手"), "unknown");
    }

    #[test]
    fn truncate_chars_works() {
        let s = "a".repeat(100);
        let out = truncate_chars(&s, 10);
        assert!(out.contains("截断"));
        let short = "短文本";
        assert_eq!(truncate_chars(short, 100), short);
    }
}
