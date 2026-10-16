//! Reflection（反思）模式：执行—复盘—迭代的自我审视机制。
//!
//! 与 Quality-Check（外部裁判，判「产物对不对」）不同，Reflection 复盘的是
//! **推理路径与工具策略**：三维度校验（逻辑自洽 / 事实准确 / 目标匹配），
//! 产出可跨 attempt 累积的经验教训（lessons）与策略调整（strategy_shift），
//! 反向注入局部重试 hint，驱动多轮迭代优化（Reflexion 架构范式）。
//!
//! **反思是顾问不是闸门**：解析失败 / LLM 失败一律 fail-open 为「无增量教训」，
//! 机械通道（[`mechanical_lessons`]）零 LLM 兜底，绝不阻断主流程。
//!
//! 效率纪律：条件触发（只在「即将带着错误重跑」时深度反思）+ 预算上限 +
//! 轻量机械档（`LAEW_REFLECTION_MODE=light` 零 LLM）+ 杀开关（`LAEW_REFLECTION=off`）。
//!
//! 设计见 `docs/Reflection反思模式/01-设计与解决方案.md`。

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

/// 渲染进 retry_hint 的反思段总长上限（字符）。超出按优先级截断，防止
/// 多轮 lessons 线性叠加撑爆执行层 context。
pub const REFLECTION_BLOCK_MAX_CHARS: usize = 1200;

/// 单条教训的长度上限（字符）。教训要写成可执行的策略句，而非长篇复盘。
const LESSON_MAX_CHARS: usize = 160;

/// 合并后 lessons 的条数上限（超出丢弃优先级低的尾部）。
const LESSONS_MAX_ITEMS: usize = 8;

// =================== 结构化反思结论 ===================

/// 单维度得分与发现。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DimensionScore {
    /// 0=无问题 / 1=有瑕疵 / 2=严重偏差（解析端 clamp 到 0..=2）。
    #[serde(default)]
    pub score: u8,
    /// 具体发现（每条 ≤ 120 字符，解析端截断）。
    #[serde(default)]
    pub findings: Vec<String>,
}

/// Reflection 结论（Reflexion 风格的自我反馈载荷）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReflectionReport {
    /// 一句话根因（下一轮必须正视）。
    #[serde(default)]
    pub root_cause: String,
    /// 三维度校验结果。
    #[serde(default)]
    pub dimensions: Dimensions,
    /// **跨 attempt 累积**的通用经验（策略句，不写具体工具 API）。
    #[serde(default)]
    pub lessons: Vec<String>,
    /// 只对下一轮生效的具体修正指令。
    #[serde(default)]
    pub corrections: Vec<String>,
    /// 工具调用 / 步骤层面的策略调整建议。
    #[serde(default)]
    pub strategy_shift: String,
    /// 本次执行路径是否大体健全（true=局部修补即可；false=策略需要转向）。
    #[serde(default = "default_sound_true")]
    pub sound: bool,
}

fn default_sound_true() -> bool {
    true
}

/// 三维度容器。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Dimensions {
    #[serde(default)]
    pub logic: DimensionScore,
    #[serde(default)]
    pub factual: DimensionScore,
    #[serde(default)]
    pub goal: DimensionScore,
}

impl ReflectionReport {
    /// 反思是否产出增量洞察（lessons / corrections / strategy_shift 至少一项）。
    pub fn has_incremental_insight(&self) -> bool {
        !self.lessons.is_empty()
            || !self.corrections.is_empty()
            || !self.strategy_shift.trim().is_empty()
            || !self.root_cause.trim().is_empty()
    }

    /// 三维度最高分（0..=2），供进度行与审计快速摘要。
    pub fn max_dimension_score(&self) -> u8 {
        self.dimensions
            .logic
            .score
            .max(self.dimensions.factual.score)
            .max(self.dimensions.goal.score)
    }
}

// =================== 环境变量决策 ===================

/// 反思档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectionMode {
    /// 条件触发 LLM 深度反思 + 机械教训合并（默认）。
    Deep,
    /// 全程零 LLM，只跑机械教训。
    Light,
}

/// 是否启用 Reflection（`LAEW_REFLECTION=off/0/false/no` 全关）。
pub fn reflection_enabled() -> bool {
    match std::env::var("LAEW_REFLECTION") {
        Ok(v) => {
            let t = v.trim().to_ascii_lowercase();
            !(t == "off" || t == "0" || t == "false" || t == "no")
        }
        Err(_) => true,
    }
}

/// 反思档位（`LAEW_REFLECTION_MODE=light` 走零 LLM 机械档；默认 deep）。
pub fn reflection_mode() -> ReflectionMode {
    match std::env::var("LAEW_REFLECTION_MODE") {
        Ok(v) => {
            let t = v.trim().to_ascii_lowercase();
            if t == "light" || t == "lite" || t == "mechanical" || t == "mech" {
                ReflectionMode::Light
            } else {
                ReflectionMode::Deep
            }
        }
        Err(_) => ReflectionMode::Deep,
    }
}

/// 每单元深度反思次数上限（`LAEW_REFLECTION_BUDGET`，默认 2；0/非法回退默认）。
pub fn reflection_budget() -> usize {
    match std::env::var("LAEW_REFLECTION_BUDGET") {
        Ok(v) => v.trim().parse::<usize>().ok().filter(|n| *n > 0).unwrap_or(2),
        Err(_) => 2,
    }
}

// =================== 机械轻量通道（零 LLM） ===================

/// 从执行轨迹的结构化信号机械映射通用教训（零幻觉、零 LLM、工具无关）。
///
/// 无论 deep/light 档，本函数产出的教训都是反思段的**基线**：
/// LLM 反思失败/关闭/超预算时仍有可执行的策略提示。
pub fn mechanical_lessons(trace: &ExecutionTrace) -> Vec<String> {
    let mut lessons: Vec<String> = Vec::new();

    // 工具错误率过高 → 先只读探查再写操作
    if trace.tool_calls >= 2 && trace.tool_calls_err * 2 > trace.tool_calls {
        lessons.push(
            "工具调用错误率过高:下一轮先用只读探查确认参数与环境,再执行写操作".to_string(),
        );
    }

    // 早终止 → 避免同型重复
    if trace.early_terminated {
        let reason = trace.early_terminate_reason.trim();
        if reason.is_empty() {
            lessons.push("上一轮触发早终止:避免同型重复步骤,改用差异化动作或收敛收口".to_string());
        } else {
            lessons.push(format!(
                "上一轮触发早终止({}):避免同型重复,改用差异化动作或收敛收口",
                clip_chars(reason, 60)
            ));
        }
    }

    // 失败信号 → 先针对性处置
    if !trace.failure_signals.is_empty() {
        let signals = clip_chars(&trace.failure_signals.join(","), 80);
        lessons.push(format!(
            "上一轮出现失败信号({signals}):下一轮先针对性处置该信号再继续"
        ));
    }

    // 同一失败连续出现 → 禁止重复同一失败方法
    if trace.max_consecutive_failures >= 2 {
        lessons.push(
            "同一失败连续出现:禁止重复同一失败方法,更换工具/改变参数/拆细步骤".to_string(),
        );
    }

    // 无进展打转 → 先换观察角度
    if trace.doom_loop_repeats >= 2 {
        lessons.push("出现无进展打转:先换观察角度获取新信息,再决定动作".to_string());
    }

    // 巨型工具调用撞输出上限 → 拆小参数
    if trace.truncated_tool_calls > 0 {
        lessons.push(
            "工具参数曾被输出上限截断:拆小参数/分步执行,避免巨型单次调用".to_string(),
        );
    }

    // LLM 输出被截断续接 → 控制单次输出体量
    if trace.truncation_resumes > 0 {
        lessons.push("LLM 输出曾被截断续接:控制单次输出体量,关键结论前置".to_string());
    }

    lessons
}

// =================== 教训合并 / 渲染 ===================

/// 规范化单条教训：压空白 + 截断到 [`LESSON_MAX_CHARS`]。
fn normalize_lesson(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(clip_chars(trimmed, LESSON_MAX_CHARS))
}

/// 教训去重键：去标点 + 转小写，容忍模型复述时的标点/大小写抖动。
fn lesson_dedup_key(lesson: &str) -> String {
    lesson
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect::<String>()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 合并多源教训（机械基线 + LLM lessons）并去重、限量。
///
/// 顺序语义：机械教训在前（零幻觉、优先保留），LLM 增量教训在后；
/// 重复项保留首见版本。
pub fn merge_lessons(base: &[String], report: &ReflectionReport) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut push_all = |items: &[String]| {
        for raw in items {
            if let Some(lesson) = normalize_lesson(raw) {
                let key = lesson_dedup_key(&lesson);
                if seen.insert(key) {
                    merged.push(lesson);
                }
            }
        }
    };

    push_all(base);
    push_all(&report.lessons);
    // corrections 也进教训账本（它们是「下一轮必须做」的更强指令）
    push_all(&report.corrections);

    merged.truncate(LESSONS_MAX_ITEMS);
    merged
}

/// 把教训账本渲染成注入 retry_hint 的文本段。
///
/// 空账本返回空串（零噪声）。渲染结构：
/// ```text
///
/// 🔍 反思教训(跨尝试累积,下一轮必须遵守):
/// 1. …
/// 【策略调整】…
/// 【根因】…
/// ```
pub fn render_reflection_block(lessons: &[String], report: Option<&ReflectionReport>) -> String {
    let strategy_shift = report
        .map(|r| r.strategy_shift.trim().to_string())
        .unwrap_or_default();
    let root_cause = report
        .map(|r| r.root_cause.trim().to_string())
        .unwrap_or_default();

    if lessons.is_empty() && strategy_shift.is_empty() && root_cause.is_empty() {
        return String::new();
    }

    let mut block = String::from("\n\n🔍 反思教训(跨尝试累积,下一轮必须遵守):");
    if lessons.is_empty() {
        block.push_str("\n(无增量教训)");
    } else {
        for (idx, lesson) in lessons.iter().enumerate() {
            block.push_str(&format!("\n{}. {}", idx + 1, lesson));
        }
    }
    if !strategy_shift.is_empty() {
        block.push_str(&format!(
            "\n【策略调整】{}",
            clip_chars(&strategy_shift, 200)
        ));
    }
    if !root_cause.is_empty() {
        block.push_str(&format!("\n【根因】{}", clip_chars(&root_cause, 120)));
    }

    // 总长封顶：超出时保留段头 + 截断标注（教训本身已限长，这里主要防极端膨胀）
    if block.chars().count() > REFLECTION_BLOCK_MAX_CHARS {
        let clipped = clip_chars(&block, REFLECTION_BLOCK_MAX_CHARS);
        format!("{clipped}\n…(反思段已截断)")
    } else {
        block
    }
}

/// 按字符数安全截断（CJK 多字节安全）。
fn clip_chars(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}…")
}

// =================== JSON 解析（fail-open） ===================

/// 从模型输出提取反思 JSON 并反序列化。
///
/// **fail-open**：解析链任何一步失败返回 `None`（与 QC 的 fail-closed 相反——
/// 反思是顾问不是闸门），上层继续用机械教训。
pub fn parse_reflection_report(text: &str) -> Option<ReflectionReport> {
    if let Some(json_str) = extract_json_block(text) {
        if let Ok(mut report) = crate::agent::json_repair::try_parse::<ReflectionReport>(json_str) {
            sanitize_report(&mut report);
            return Some(report);
        }
    }
    if let Some(json_str) = extract_standalone_json(text) {
        if let Ok(mut report) = crate::agent::json_repair::try_parse::<ReflectionReport>(json_str) {
            sanitize_report(&mut report);
            return Some(report);
        }
    }
    None
}

/// 解析后整形：clamp 维度分、截断 findings/字段长度。
fn sanitize_report(report: &mut ReflectionReport) {
    let clamp_dim = |dim: &mut DimensionScore| {
        dim.score = dim.score.min(2);
        dim.findings = dim
            .findings
            .iter()
            .filter_map(|f| normalize_lesson(f))
            .take(4)
            .collect();
    };
    clamp_dim(&mut report.dimensions.logic);
    clamp_dim(&mut report.dimensions.factual);
    clamp_dim(&mut report.dimensions.goal);
    report.root_cause = clip_chars(report.root_cause.trim(), 200);
    report.strategy_shift = clip_chars(report.strategy_shift.trim(), 240);
    report.lessons = report
        .lessons
        .iter()
        .filter_map(|l| normalize_lesson(l))
        .take(LESSONS_MAX_ITEMS)
        .collect();
    report.corrections = report
        .corrections
        .iter()
        .filter_map(|c| normalize_lesson(c))
        .take(6)
        .collect();
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
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
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
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

// =================== ReflectionRunner（LLM 深度反思） ===================

/// 反思请求输入（执行层无关六元组 + QC 结论，见设计 §5.2）。
pub struct ReflectionInput<'a> {
    pub goal: &'a str,
    pub unit_scope: &'a str,
    pub expected_output: &'a str,
    pub actual_output: &'a str,
    pub qc_issues: &'a [String],
    pub qc_suggestion: &'a str,
    pub trace: &'a ExecutionTrace,
    /// 反思聚焦提示（如「这是第 2 次局部重试前的复盘」），可为空。
    pub focus: &'a str,
}

/// Reflection Agent 运行器（与 [`crate::agent::quality::QualityRunner`] 同构）。
pub struct ReflectionRunner {
    agent: Agent,
    db: Arc<Db>,
}

impl ReflectionRunner {
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>, db: Arc<Db>) -> Self {
        let agent = Agent::new(llm, AgentProfile::reflection_profile());
        Self { agent, db }
    }

    /// 执行一次深度反思。
    ///
    /// 返回 `(Option<ReflectionReport>, Usage)`：
    /// - `None` = 解析失败或空产出（**fail-open**，调用方用机械教训继续）；
    /// - `Usage` 恒带回（即使解析失败，LLM 调用已发生，用量必须入账）。
    pub async fn reflect(
        &self,
        input: &ReflectionInput<'_>,
        session_id: &str,
    ) -> Result<(Option<ReflectionReport>, Usage)> {
        let prompt = build_reflection_prompt(input);
        let mut sub_session = session::Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        let (text, usage, _trace) = match self.agent.run_session(&mut sub_session).await {
            Ok(t) => t,
            Err(e) => {
                // LLM 层失败：fail-open 为无增量教训（反思是顾问不是闸门），
                // 但不吞错误——记日志 + Agent-Memory 便于排查。
                tracing::warn!(error = %e, "Reflection LLM 调用失败,fail-open 落回机械教训");
                let _ = memory::record_entry(
                    &self.db,
                    AgentRole::Reflection,
                    session_id,
                    "reflect_llm_failed",
                    "",
                    Some(&format!("LLM 调用失败: {e}")),
                    serde_json::json!({}),
                );
                return Ok((None, Usage::default()));
            }
        };

        let report = parse_reflection_report(&text);
        if let Some(r) = &report {
            tracing::info!(
                session = %session_id,
                root_cause = %crate::logging::clip(&r.root_cause),
                lessons = r.lessons.len(),
                corrections = r.corrections.len(),
                max_dim_score = r.max_dimension_score(),
                sound = r.sound,
                "Reflection 深度反思完成"
            );
        } else {
            tracing::debug!(
                session = %session_id,
                text_len = text.chars().count(),
                "Reflection 输出未能解析为报告,fail-open 为无增量教训"
            );
        }

        // Agent-Memory 留痕（fail-open，不影响主流程）
        let _ = memory::record_entry(
            &self.db,
            AgentRole::Reflection,
            session_id,
            &clip_chars(input.unit_scope, 80),
            &match &report {
                Some(r) => format!(
                    "root_cause={} lessons={} sound={}",
                    clip_chars(&r.root_cause, 60),
                    r.lessons.len(),
                    r.sound
                ),
                None => "parse_failed".to_string(),
            },
            None,
            serde_json::json!({
                "has_report": report.is_some(),
                "usage": {
                    "input": usage.input_tokens,
                    "output": usage.output_tokens,
                }
            }),
        );

        Ok((report, usage))
    }
}

/// 组装反思 prompt。
///
/// 材料六元组全部来自执行层现场，不含工具 schema（通用性约束见设计 §5.1）。
fn build_reflection_prompt(input: &ReflectionInput<'_>) -> String {
    let trace_summary = input.trace.render_prompt();
    let issues = if input.qc_issues.is_empty() {
        "(QC 未列出阻断性缺陷)".to_string()
    } else {
        input.qc_issues.join(" | ")
    };
    let focus_section = if input.focus.trim().is_empty() {
        String::new()
    } else {
        format!("\n反思聚焦: {}\n", input.focus.trim())
    };
    format!(
        "【Reflection 反思复盘】\n\
         {focus_section}\
         整体目标(背景): {goal}\n\
         本单元职责: {unit_scope}\n\
         本单元期望输出: {expected}\n\
         实际输出(截断): {actual}\n\
         \n\
         【QC 结论(已判明的缺陷,不要复述)】\n\
         issues: {issues}\n\
         suggestion: {suggestion}\n\
         \n\
         【执行轨迹】\n{trace}\n\
         \n\
         请从**推理路径**角度复盘(不是再判一次产物对错):\n\
         ① 逻辑自洽:推理链有无矛盾/跳步/循环,结论是否被证据支撑;\n\
         ② 事实准确:工具结果有无误读/选择性忽略/无依据编造,证据与结论是否对得上;\n\
         ③ 目标匹配:执行路径是否服务验收标准,有无目标漂移的苗头。\n\
         \n\
         输出要求:只输出一个合法 JSON 对象,不要输出其它文本:\n\
         {{\"root_cause\":\"一句话根因\",\"dimensions\":{{\"logic\":{{\"score\":0,\"findings\":[]}},\"factual\":{{\"score\":0,\"findings\":[]}},\"goal\":{{\"score\":0,\"findings\":[]}}}},\"lessons\":[\"通用经验句\"],\"corrections\":[\"下一轮立即修正指令\"],\"strategy_shift\":\"工具调用策略调整\",\"sound\":true}}\n\
         【措辞规范】lessons/corrections 写**策略**而非具体工具 API(写「先只读探查再写操作」,\
         不写「先调某某 action」);不要复述 QC 已列明的缺陷清单——只产出**增量洞察**;\
         score 取 0(无问题)/1(有瑕疵)/2(严重偏差)。",
        goal = clip_chars(input.goal, 400),
        unit_scope = clip_chars(input.unit_scope, 400),
        expected = clip_chars(input.expected_output, 400),
        actual = clip_chars(input.actual_output, 2000),
        issues = clip_chars(&issues, 400),
        suggestion = clip_chars(input.qc_suggestion, 300),
        trace = clip_chars(&trace_summary, 3000),
    )
}

// =================== 单元测试 ===================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_trace() -> ExecutionTrace {
        ExecutionTrace::default()
    }

    // ---------- 机械教训 ----------

    #[test]
    fn 无信号时机械教训为空() {
        assert!(mechanical_lessons(&sample_trace()).is_empty());
    }

    #[test]
    fn 工具错误率过半触发只读探查教训() {
        let mut t = sample_trace();
        t.tool_calls = 4;
        t.tool_calls_ok = 1;
        t.tool_calls_err = 3;
        let lessons = mechanical_lessons(&t);
        assert!(
            lessons.iter().any(|l| l.contains("只读探查")),
            "错误率过半应产出只读探查教训: {lessons:?}"
        );
    }

    #[test]
    fn 早终止触发差异化动作教训() {
        let mut t = sample_trace();
        t.early_terminated = true;
        t.early_terminate_reason = "RepeatedToolFailure".into();
        let lessons = mechanical_lessons(&t);
        assert!(
            lessons.iter().any(|l| l.contains("早终止") && l.contains("RepeatedToolFailure")),
            "{lessons:?}"
        );
    }

    #[test]
    fn 失败信号触发针对性处置教训() {
        let mut t = sample_trace();
        t.failure_signals = vec!["target_drift".into()];
        let lessons = mechanical_lessons(&t);
        assert!(lessons.iter().any(|l| l.contains("target_drift")), "{lessons:?}");
    }

    #[test]
    fn 连续失败与打转与截断各自产出教训() {
        let mut t = sample_trace();
        t.max_consecutive_failures = 2;
        t.doom_loop_repeats = 3;
        t.truncated_tool_calls = 1;
        t.truncation_resumes = 1;
        let lessons = mechanical_lessons(&t);
        assert!(lessons.iter().any(|l| l.contains("禁止重复同一失败方法")));
        assert!(lessons.iter().any(|l| l.contains("无进展打转")));
        assert!(lessons.iter().any(|l| l.contains("拆小参数")));
        assert!(lessons.iter().any(|l| l.contains("截断续接")));
    }

    // ---------- 合并 / 渲染 ----------

    #[test]
    fn 合并教训去重且机械优先() {
        let base = vec!["先只读探查再执行写操作".to_string()];
        let report = ReflectionReport {
            lessons: vec![
                "先只读探查再执行写操作!".to_string(), // 标点差异应被去重
                "换观察角度获取新信息".to_string(),
            ],
            corrections: vec!["把重复点击改为等待后单击".to_string()],
            ..Default::default()
        };
        let merged = merge_lessons(&base, &report);
        assert_eq!(merged.len(), 3, "去重后应为 3 条: {merged:?}");
        assert_eq!(merged[0], base[0], "机械教训应排前");
    }

    #[test]
    fn 合并教训限量且规范空白() {
        let base: Vec<String> = (0..20)
            .map(|i| format!("   教训{i}   带  多余   空白   "))
            .collect();
        let merged = merge_lessons(&base, &ReflectionReport::default());
        assert!(merged.len() <= LESSONS_MAX_ITEMS, "{}", merged.len());
        assert!(merged[0].contains("教训0 带 多余 空白"), "{}", merged[0]);
    }

    #[test]
    fn 空教训渲染为空段() {
        assert!(render_reflection_block(&[], None).is_empty());
    }

    #[test]
    fn 渲染段含序号策略调整与根因() {
        let lessons = vec!["第一条".to_string(), "第二条".to_string()];
        let report = ReflectionReport {
            strategy_shift: "先探查后执行".into(),
            root_cause: "误读了工具返回".into(),
            ..Default::default()
        };
        let block = render_reflection_block(&lessons, Some(&report));
        assert!(block.contains("1. 第一条"));
        assert!(block.contains("2. 第二条"));
        assert!(block.contains("【策略调整】先探查后执行"));
        assert!(block.contains("【根因】误读了工具返回"));
    }

    #[test]
    fn 渲染段超长截断() {
        let lessons: Vec<String> = (0..8)
            .map(|i| format!("很长的教训{i}{}", "x".repeat(200)))
            .collect();
        let block = render_reflection_block(&lessons, None);
        assert!(
            block.chars().count() <= REFLECTION_BLOCK_MAX_CHARS + 20,
            "len={}",
            block.chars().count()
        );
    }

    // ---------- JSON 解析 ----------

    #[test]
    fn 解析json块形态() {
        let text = "好的,反思如下:\n```json\n{\"root_cause\":\"误读返回\",\"lessons\":[\"先核对再下结论\"],\"sound\":false}\n```";
        let report = parse_reflection_report(text).expect("应解析成功");
        assert_eq!(report.root_cause, "误读返回");
        assert!(!report.sound);
        assert_eq!(report.lessons.len(), 1);
    }

    #[test]
    fn 解析独立json形态() {
        let text = "{\"root_cause\":\"路径绕远\",\"strategy_shift\":\"拆细步骤\",\"sound\":true}";
        let report = parse_reflection_report(text).expect("应解析成功");
        assert_eq!(report.strategy_shift, "拆细步骤");
    }

    #[test]
    fn 畸形输出fail_open为none() {
        assert!(parse_reflection_report("我觉得没问题,继续吧").is_none());
        assert!(parse_reflection_report("```json\n{{{broken\n```").is_none());
    }

    #[test]
    fn 解析后clamp维度分并截断字段() {
        let text = r#"{"dimensions":{"logic":{"score":9,"findings":["ok","x"]}},"lessons":["   "]}"#;
        let report = parse_reflection_report(text).expect("应解析成功");
        assert_eq!(report.dimensions.logic.score, 2, "score 应 clamp 到 2");
        // 空白教训应被过滤
        assert!(report.lessons.is_empty());
    }

    // ---------- 环境变量 ----------

    #[test]
    fn 环境变量档位与预算解析() {
        // 用串行锁避免与其它单测抢 env（同 GLOBAL_ENV_CWD_LOCK 思路，本测试自带锁）
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        std::env::set_var("LAEW_REFLECTION", "off");
        assert!(!reflection_enabled());
        std::env::set_var("LAEW_REFLECTION", "on");
        assert!(reflection_enabled());

        std::env::set_var("LAEW_REFLECTION_MODE", "light");
        assert_eq!(reflection_mode(), ReflectionMode::Light);
        std::env::set_var("LAEW_REFLECTION_MODE", "deep");
        assert_eq!(reflection_mode(), ReflectionMode::Deep);

        std::env::set_var("LAEW_REFLECTION_BUDGET", "5");
        assert_eq!(reflection_budget(), 5);
        std::env::set_var("LAEW_REFLECTION_BUDGET", "0");
        assert_eq!(reflection_budget(), 2, "0 应回退默认");
        std::env::set_var("LAEW_REFLECTION_BUDGET", "abc");
        assert_eq!(reflection_budget(), 2, "非法值应回退默认");

        // 清理，避免影响其它用例
        std::env::remove_var("LAEW_REFLECTION");
        std::env::remove_var("LAEW_REFLECTION_MODE");
        std::env::remove_var("LAEW_REFLECTION_BUDGET");
    }

    // ---------- profile / 角色 ----------

    #[test]
    fn 反思角色字符串roundtrip() {
        assert_eq!(AgentRole::Reflection.as_str(), "reflection");
        assert_eq!(AgentRole::from("reflection"), AgentRole::Reflection);
    }

    #[test]
    fn 反思profile无工具且提示词含自身名称() {
        use crate::config::Protocol;
        let p = AgentProfile::reflection_profile();
        assert_eq!(p.name, crate::agent::profile::REFLECTION_AGENT_NAME);
        assert!(p.tools.names().is_empty(), "Reflection 不应持工具");
        assert!(p.emit_tool.is_none());
        let rendered = p.system_prompt.render(Protocol::Anthropic);
        assert!(rendered.contains(p.name.as_str()), "提示词应含自身名称");
        assert!(rendered.contains("逻辑自洽"), "提示词应含三维度定义");
    }

    #[test]
    fn 反思报告增量洞察判定() {
        let empty = ReflectionReport::default();
        assert!(!empty.has_incremental_insight());
        let with_lesson = ReflectionReport {
            lessons: vec!["x".into()],
            ..Default::default()
        };
        assert!(with_lesson.has_incremental_insight());
    }
}
