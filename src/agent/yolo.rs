//! Yolo Agent —— 入口层 Agent,负责目标识别、意图识别、任务分类与失败回流。
//!
//! Yolo 是用户输入的第一站。它分析用户请求,按难度分为三档:
//! - `simple`(简单):单步操作,委派 SubAgent-Work
//! - `medium`(中等难度):多步流程,委派 Main-Work
//! - `hard`(高等难度):需要书面方案,委派 Plan
//!
//! Yolo 仅持有 Read 工具(用于理解上下文),不持有 Bash/Write 等会改变系统状态的工具。
//!
//! 失败回流:Yolo 接收下游失败摘要后,可决定重试(修订 plan)或给出用户建议。
//!
//! 设计见 `docs/多Agent架构重构/01-设计与解决方案.md` §3。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::context::AgentRole;
use crate::agent::{Agent, AgentProfile};
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, ContentBlock, Role, Usage};
use crate::session::Session;

/// 全局 Yolo 解析失败计数器(关联报告: 20260908_203854 D-002)。
///
/// 用于跨 Session 观察 Yolo 分类降级频率,供 SessionContext 摘要
/// 在 `summary()` 中合并「Yolo 降级 N 次」一行,便于人工/自动发现
/// 系统性上游模型问题。
static YOLO_PARSE_FAILURES: AtomicUsize = AtomicUsize::new(0);

/// 读取 Yolo 解析失败累计次数(供 SessionContext 等调用方合并摘要)。
pub fn yolo_parse_failures() -> usize {
    YOLO_PARSE_FAILURES.load(Ordering::Relaxed)
}

/// 重置计数器(主要给测试使用)。
#[cfg(test)]
pub fn reset_yolo_parse_failures() {
    YOLO_PARSE_FAILURES.store(0, Ordering::Relaxed);
}

/// 任务难度等级(三档)
// 2026-09-16 第 57 轮:补 Serialize — TaskClassification 走 Serialize 时连带需要。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskLevel {
    /// 简单:单步操作,委派 SubAgent-Work
    Simple,
    /// 中等难度:多步流程,委派 Main-Work
    Medium,
    /// 高等难度:需要书面方案,委派 Plan
    Hard,
}

impl TaskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskLevel::Simple => "simple",
            TaskLevel::Medium => "medium",
            TaskLevel::Hard => "hard",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            TaskLevel::Simple => "简单",
            TaskLevel::Medium => "中等难度",
            TaskLevel::Hard => "高等难度",
        }
    }
}

/// Yolo 输出的结构化分类结果(三档 + agent_role)
// 2026-09-16 第 57 轮:补 Serialize — TaskResult 走 Serialize 时需要
// TaskClassification 也实现(derive 一致性)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskClassification {
    pub task_level: TaskLevel,
    /// 三步识别之一:用户目的(为什么问)
    #[serde(default)]
    pub purpose: String,
    /// 三步识别之二:目标(要达成什么)
    pub goal_summary: String,
    /// 三步识别之三:意图标签
    pub intent: String,
    /// 委派目标(由 Yolo 决定,与 task_level 一一对应)。
    /// 旧 JSON 缺省时按 task_level 推断。
    #[serde(default)]
    pub agent_role: Option<AgentRole>,
    #[serde(default)]
    pub decomposition_plan: Vec<String>,
    pub direct_answer: Option<String>,
    /// 失败时给用户的备选建议(默认空)
    #[serde(default)]
    pub user_suggestion_if_fail: String,
    /// Yolo 本次是否走降级分支(JSON 解析失败 → 强制 simple,关联报告: 2026-09-09_04 D-002)。
    /// 旧 JSON 缺省 = false,新写入字段由 yolo runner 填充。
    #[serde(default)]
    pub yolo_degraded: bool,
    /// 2026-09-16 第 59 轮:基于用户原始输入关键词推断的 delegate_to 建议。
    /// Main-Work 拆解 WorkFlow 时优先采用此值设置 delegate_to。
    /// 取值:"subagent" / None(不强制,由 Main-Work 自行判断)。
    /// 2026-09-18 第 84 轮:删除 "windowuse" 产出(WindowUse Agent 已删除,
    /// 桌面窗口操控由 SubAgent-Work 的 MCP_Window_Use 工具承担)。
    /// 2026-09-18 第 89 轮:删除 "webuse" 产出(Chromium-WebUse Agent 已删除,
    /// 浏览器操控由 SubAgent-Work 的 MCP_Web_Use 工具承担)。
    #[serde(default)]
    pub suggested_delegate: Option<String>,
}

impl TaskClassification {
    /// 获取 agent_role(若为 None 则按 task_level 推断)
    pub fn effective_agent_role(&self) -> AgentRole {
        self.agent_role.unwrap_or(match self.task_level {
            TaskLevel::Simple => AgentRole::SubAgent,
            TaskLevel::Medium => AgentRole::MainWork,
            TaskLevel::Hard => AgentRole::Plan,
        })
    }
}

/// Yolo 执行结果
pub enum YoloOutcome {
    /// 直接回答(可由 Orchestrator 进一步判断是否委派)
    DirectAnswer {
        text: String,
        classification: TaskClassification,
        usage: Usage,
    },
    /// 委派给下游 Agent
    Delegate {
        classification: TaskClassification,
        yolo_text: String,
        usage: Usage,
    },
}

/// Yolo Runner:持有 Yolo + 下游 Agents 的编排入口。
///
/// 旧版本 `YoloRunner` 同时持有 yolo_agent + work_agent;新版本拆分为
/// `MultiAgentOrchestrator`(在 `orchestrator.rs`),但仍保留 `YoloRunner`
/// 作为单纯的入口层决策(只做分类,不直接调用执行层),保持向后兼容。
pub struct YoloRunner {
    yolo_agent: Agent,
}

impl YoloRunner {
    /// 构造 Yolo Runner(只持有 Yolo Agent)。
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>) -> Self {
        let yolo = Agent::new(llm, AgentProfile::yolo_profile()).with_max_iterations(4);
        Self { yolo_agent: yolo }
    }

    /// 用自定义 max_iterations 构造 Yolo。
    pub fn with_max_iterations(llm: Arc<dyn crate::llm::LlmClient>, max_iter: usize) -> Self {
        let yolo = Agent::new(llm, AgentProfile::yolo_profile()).with_max_iterations(max_iter);
        Self { yolo_agent: yolo }
    }

    /// Yolo Agent 引用
    pub fn yolo_agent(&self) -> &Agent {
        &self.yolo_agent
    }

    /// 用已有 Agent 实例构造。
    pub fn from_agent(yolo_agent: Agent) -> Self {
        Self { yolo_agent }
    }

    /// 跑一次 Yolo 分类(从已有上下文中分析最后一条用户消息)。
    ///
    /// `session_id` 用于 X-Session-Id 传播(2026-09-09 第 08 轮):与其它 6 个
    /// runner 对齐,让 Yolo 的请求在抓包中可关联到所属任务(此前为随机新 ID)。
    ///
    /// 2026-09-16 第 59 轮:从上下文中提取用户原始 prompt,推断 suggested_delegate
    /// 并写入 classification,供 Main-Work 拆解 WorkFlow 时优先采用。
    pub async fn classify(
        &self,
        session_id: &str,
        context: &[ChatMessage],
    ) -> Result<(TaskClassification, String, Usage)> {
        let mut yolo_session = Session::new();
        yolo_session.id = session_id.to_string();
        for msg in context {
            yolo_session.context_mut().push(msg.clone());
        }
        let (text, usage, _trace) = self.yolo_agent.run_session(&mut yolo_session).await?;
        let mut classification = parse_classification(&text).unwrap_or_else(|e| {
            YOLO_PARSE_FAILURES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("Yolo 分类解析失败,降级为 simple: {}", e);
            degraded_classification(context)
        });
        // 2026-09-16 第 59 轮:从上下文提取用户原始 prompt 推断 suggested_delegate
        let user_prompt = extract_user_prompt(context);
        if !user_prompt.is_empty() {
            classification.suggested_delegate = infer_suggested_delegate(&user_prompt);
            if let Some(ref d) = classification.suggested_delegate {
                tracing::info!(suggested_delegate = %d, "Yolo 推断 delegate_to 建议");
            }
        }
        Ok((classification, text, usage))
    }

    /// 旧 API 兼容:直接调用 Work Agent 跑一次(忽略 Yolo 分类)
    pub async fn run_legacy_work(
        &self,
        work_agent: &crate::agent::Agent,
        session: &mut Session,
    ) -> Result<(String, Usage)> {
        let (text, usage, _trace) = work_agent.run_session(session).await?;
        Ok((text, usage))
    }
}

/// 运行 Yolo Agent(向后兼容旧 API)。
pub async fn run_yolo(yolo_agent: &Agent, context: &[ChatMessage]) -> Result<YoloOutcome> {
    let mut yolo_session = Session::new();
    for msg in context {
        yolo_session.context_mut().push(msg.clone());
    }
    let (text, usage, _trace) = yolo_agent.run_session(&mut yolo_session).await?;
    let classification = match parse_classification(&text) {
        Ok(c) => c,
        Err(e) => {
            YOLO_PARSE_FAILURES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("Yolo 分类解析失败,降级为 simple: {}", e);
            degraded_classification(context)
        }
    };
    if classification.task_level == TaskLevel::Simple && classification.direct_answer.is_some() {
        Ok(YoloOutcome::DirectAnswer {
            text: classification.direct_answer.clone().unwrap_or(text),
            classification,
            usage,
        })
    } else {
        Ok(YoloOutcome::Delegate {
            classification,
            yolo_text: text,
            usage,
        })
    }
}

/// 降级时从上下文提取最近一条 user 消息文本,作为 goal_summary 兜底。
///
/// 2026-09-13 第 50 轮(真实网关批量测试 c03 实测):降级分类的 goal_summary
/// 原为固定占位符「(解析失败,已降级)」,SubAgent 拿不到真实任务描述 →
/// 自行发挥,产物路径翻倍嵌套(`tmpPlan/agent-test/tmpPlan/agent-test/`)。
/// 改为保留用户原文(折叠空白 + 截断),占位符挪到 purpose 字段;
/// 上下文没有 user 文本时才回落占位符。
fn degraded_goal_from_context(context: &[ChatMessage]) -> String {
    let raw = context
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        "(解析失败,已降级)".to_string()
    } else {
        flat.chars().take(300).collect()
    }
}

/// 统一的降级分类构造(2026-09-13 第 50 轮):goal_summary 保留用户原始任务。
fn degraded_classification(context: &[ChatMessage]) -> TaskClassification {
    // 2026-09-16 第 59 轮:降级时也尝试推断 suggested_delegate
    let user_prompt = extract_user_prompt(context);
    let suggested_delegate = if user_prompt.is_empty() {
        None
    } else {
        infer_suggested_delegate(&user_prompt)
    };
    TaskClassification {
        task_level: TaskLevel::Simple,
        purpose: "(Yolo 解析失败,已降级)".to_string(),
        goal_summary: degraded_goal_from_context(context),
        intent: "unknown".to_string(),
        agent_role: Some(AgentRole::SubAgent),
        decomposition_plan: vec![],
        direct_answer: None,
        user_suggestion_if_fail: String::new(),
        yolo_degraded: true, // 关联报告: 2026-09-09_04 D-002
        suggested_delegate,
    }
}

/// 从上下文消息中提取用户原始 prompt(最近一条 user 消息的文本内容)。
fn extract_user_prompt(context: &[ChatMessage]) -> String {
    context
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// 2026-09-16 第 59 轮:基于用户原始 prompt 关键词推断 delegate_to。
///
/// 代码/文件操作类 → "subagent";无法判断 → None(让 Main-Work 自行判断)。
/// 2026-09-18 第 84 轮:删除窗口操控关键词表与 "windowuse" 产出 ——
/// WindowUse Agent 已删除,桌面窗口操控任务走 medium 档由 Main-Work
/// 关键词推断委派给 SubAgent-Work(MCP_Window_Use 工具)。
/// 2026-09-18 第 89 轮:删除网页操控关键词表与 "webuse" 产出 ——
/// Chromium-WebUse Agent 已删除,浏览器操控由 SubAgent-Work 的 MCP_Web_Use
/// 工具承担(网页类任务仍按 Yolo 提示词最低判 medium,拆解后统一 SubAgent 执行)。
///
/// 关键词表覆盖:
/// - 代码/文件:编写/修改/创建文件/代码/rust/python/git/cargo/test
const SUBAGENT_KEYWORDS: &[&str] = &[
    "编写代码",
    "修改代码",
    "创建文件",
    "代码",
    "rust",
    "python",
    "git",
    "cargo",
    "test",
    "编写",
    "修改",
    "创建",
    "重构",
    "实现",
    "函数",
    "类",
    "模块",
    "接口",
    "算法",
    "write code",
    "programming",
];

pub fn infer_suggested_delegate(user_prompt: &str) -> Option<String> {
    let lower = user_prompt.to_lowercase();
    let code_hit = SUBAGENT_KEYWORDS.iter().any(|k| lower.contains(k));
    if code_hit {
        return Some("subagent".to_string());
    }
    None
}

/// 从 Yolo 返回的文本中解析结构化分类结果。
///
/// 优先匹配 ```json ... ``` 代码块;找不到则尝试匹配最大合法 JSON 对象。
/// 直接解析失败时自动走 JSON 修复链(`json_repair`,Tier-1 语法修复),
/// 修复不了仍返回 YoloParse(fail-closed 不变)。
///
/// ★ 2026-09-17 第 79 轮 P0-2:多候选提取。此前只试「第一个 ```json 块」和
/// 「第一个顶层 `{`」,LLM 在 JSON 前后夹带回显内容(用户输入含 HTML 围栏时高发,
/// 实测 llaew_20260917_135249.log 两轮输入全部降级)就直接放弃 → 降级 simple。
/// 现在遍历**全部** ```json 块 + **全部**顶层平衡 `{...}` 候选,逐个过修复链,
/// 首个可解析者胜出;全部失败才 YoloParse(错误信息含候选数,便于日志定位)。
pub fn parse_classification(text: &str) -> Result<TaskClassification> {
    // 候选按优先级排序:显式 ```json 块在前(模型明示意图),裸 `{}` 对象在后。
    let mut candidates: Vec<&str> = extract_all_json_blocks(text);
    candidates.extend(extract_all_json_objects(text));
    let total = candidates.len();
    let mut last_err = String::new();
    for (idx, json_str) in candidates.iter().enumerate() {
        // Yolo 路径:启用 Tier-2 截断补全(LLM 输出被 token 触顶截断的场景),
        // 仅在 Quality-Check fail-closed 路径禁用。关联报告: 2026-09-09_04 D-001。
        match crate::agent::json_repair::try_parse_lenient::<TaskClassification>(json_str) {
            Ok(c) => {
                if idx > 0 {
                    tracing::debug!(candidate = idx, "Yolo 多候选提取命中非首选候选");
                }
                return Ok(c);
            }
            Err(e) => last_err = e,
        }
    }
    Err(AgentError::YoloParse(format!(
        "未找到合法的 JSON 分类结果(共尝试 {total} 个候选;最后错误: {last_err})"
    )))
}

/// 提取**全部** ```json ... ``` 代码块中的内容(第 79 轮:单块 → 多块迭代)。
fn extract_all_json_blocks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let start_marker = "```json";
    let mut rest = text;
    // 起点(字节偏移),用于把子串结果映射回原 text 的切片。
    let mut base = 0usize;
    while let Some(rel) = rest.find(start_marker) {
        let start = base + rel;
        let content_start = start + start_marker.len();
        let content_start = text[content_start..]
            .find(|c: char| !c.is_whitespace())
            .map(|i| content_start + i)
            .unwrap_or(content_start);
        let Some(end_rel) = text[content_start..].find("```") else {
            break; // 无闭合围栏:后面的块也不可能闭合
        };
        let json_text = text[content_start..content_start + end_rel].trim();
        if !json_text.is_empty() {
            out.push(json_text);
        }
        // 从闭合围栏之后继续找下一个块。
        let next_base = content_start + end_rel + 3;
        if next_base >= text.len() {
            break;
        }
        rest = &text[next_base..];
        base = next_base;
    }
    out
}

/// 提取**全部**顶层平衡 `{...}` 候选(第 79 轮:首个 → 全部)。
///
/// 跳过字符串字面量内的花括号;从某个 `{` 出发未平衡(前置噪声花括号吞掉后续)
/// 时**从下一个 `{` 重试**而非放弃——保证噪声段落不会遮蔽后面的真实 JSON。
fn extract_all_json_objects(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        let mut end = None;
        let mut j = i;
        while j < bytes.len() {
            let c = bytes[j] as char;
            if escape {
                escape = false;
                j += 1;
                continue;
            }
            if c == '\\' && in_string {
                escape = true;
                j += 1;
                continue;
            }
            if c == '"' {
                in_string = !in_string;
                j += 1;
                continue;
            }
            if !in_string {
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(j + 1);
                        break;
                    }
                }
            }
            j += 1;
        }
        match end {
            Some(e) => {
                out.push(&text[start..e]);
                i = e; // 从闭合 `}` 之后继续扫下一个候选
            }
            None => {
                // 未平衡:该 `{` 是噪声,从它的下一个字符重试后续 `{`
                i = start + 1;
            }
        }
    }
    out
}

/// 把 TaskClassification 转成 work prompt 文本(用于把分类结果作为下一步 user 消息)。
///
/// 关联报告 2026-09-09_06 F-003: 当 `classification.yolo_degraded == true` 时
/// 在 prompt 末尾追加降级警告 + 建议,让 SubAgent 明确知道"为什么降级"以及
/// "现在应该怎么办",避免在空 `goal_summary` 上做无意义工具调用。
pub fn build_work_prompt(classification: &TaskClassification) -> String {
    let mut prompt = String::new();
    prompt.push_str("【任务计划】\n");
    if !classification.purpose.is_empty() {
        prompt.push_str(&format!("目的: {}\n", classification.purpose));
    }
    prompt.push_str(&format!("目标: {}\n", classification.goal_summary));
    prompt.push_str(&format!(
        "难度: {} ({})\n",
        classification.task_level.display_name(),
        classification.task_level.as_str()
    ));
    prompt.push_str(&format!("意图: {}\n", classification.intent));
    prompt.push_str(&format!(
        "委派 Agent: {}\n",
        classification.effective_agent_role().as_str()
    ));

    if !classification.decomposition_plan.is_empty() {
        prompt.push_str("\n分解步骤:\n");
        for (i, step) in classification.decomposition_plan.iter().enumerate() {
            prompt.push_str(&format!("  {}. {}\n", i + 1, step));
        }
    }

    prompt
        .push_str("\n请按照以上计划执行任务。使用可用工具完成目标,完成后用简洁中文回复最终结果。");

    // F-003:Yolo 解析失败降级时显式告知 SubAgent
    if classification.yolo_degraded {
        prompt.push_str(
            "\n\n⚠️ [Yolo 分类解析失败,已降级为 Simple 直答模式]\n\
             说明: 上层 Yolo Agent 返回的 JSON 分类结果无法解析,已自动 fallback 到 Simple + SubAgent。\n\
             建议:\n\
               - 若这是知识问答 / 简单查询,请直接给出最终答案(不要再拆解任务);\n\
               - 若涉及多步工具调用,请显式提示用户重新表述或补充任务边界(目标 / 验收标准 / 输入数据);\n\
               - 若需要更复杂的规划,请明确告知用户「需要重新启动并提供更明确的任务描述」。",
        );
    }

    prompt
}

/// 累加 token 用量
pub fn add_usage(mut total: Usage, delta: Usage) -> Usage {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.cache_read_input_tokens = total
        .cache_read_input_tokens
        .saturating_add(delta.cache_read_input_tokens);
    total.cache_creation_input_tokens = total
        .cache_creation_input_tokens
        .saturating_add(delta.cache_creation_input_tokens);
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::AgentRole;

    #[test]
    fn degraded_goal_keeps_raw_user_prompt() {
        // 2026-09-13 第 50 轮(c03 实测):降级分类应保留用户原始任务文本,
        // 避免 SubAgent 在占位符上自行发挥导致产物路径翻倍嵌套。
        let context = vec![
            ChatMessage::assistant(vec![ContentBlock::text("上一轮回复")]),
            ChatMessage::user("  Write 备份脚本 tmpPlan/agent-test/backup.sh\n并跑一次验证  "),
        ];
        let c = degraded_classification(&context);
        assert!(c.yolo_degraded);
        assert_eq!(c.task_level, TaskLevel::Simple);
        assert_eq!(
            c.goal_summary,
            "Write 备份脚本 tmpPlan/agent-test/backup.sh 并跑一次验证"
        );
        assert!(c.purpose.contains("已降级"));
    }

    #[test]
    fn degraded_goal_falls_back_to_placeholder_without_user_text() {
        let context = vec![ChatMessage::assistant(vec![ContentBlock::text(
            "只有 assistant 消息",
        )])];
        let c = degraded_classification(&context);
        assert_eq!(c.goal_summary, "(解析失败,已降级)");
    }

    #[test]
    fn degraded_goal_truncates_long_prompt() {
        let long = "长".repeat(1000);
        let context = vec![ChatMessage::user(long)];
        let c = degraded_classification(&context);
        assert_eq!(c.goal_summary.chars().count(), 300);
    }

    #[test]
    fn extract_json_block_basic() {
        let text = "一些前置文本\n```json\n{\"key\": \"value\"}\n```\n后续文本";
        let blocks = extract_all_json_blocks(text);
        assert_eq!(blocks, vec!["{\"key\": \"value\"}"]);
    }

    #[test]
    fn extract_json_block_no_block() {
        let text = "没有 JSON 代码块的普通文本";
        assert!(extract_all_json_blocks(text).is_empty());
    }

    #[test]
    fn extract_standalone_json_nested() {
        let text = "prefix {\"a\": {\"b\": 1}, \"c\": [1,2]} suffix";
        let objs = extract_all_json_objects(text);
        assert_eq!(objs, vec!["{\"a\": {\"b\": 1}, \"c\": [1,2]}"]);
    }

    // ========== 2026-09-17 第 79 轮 P0-2:多候选提取测试 ==========

    #[test]
    fn extract_all_json_blocks_finds_multiple() {
        // 两个 ```json 块都要被提取(旧版只取第一个)
        let text = "噪声\n```json\n{\"a\":1}\n```\n中间噪声\n```json\n{\"b\":2}\n```";
        let blocks = extract_all_json_blocks(text);
        assert_eq!(blocks.len(), 2, "应提取两个块,实际: {blocks:?}");
        assert_eq!(blocks[0], "{\"a\":1}");
        assert_eq!(blocks[1], "{\"b\":2}");
    }

    #[test]
    fn extract_all_json_objects_multiple_and_string_safe() {
        // 前置噪声含 `{`(非平衡)+ 字符串内花括号不算深度
        let text = "杂项 { 不完整\n首{\"k\":\"a{b}c\"}\n尾{\"x\":2}";
        let objs = extract_all_json_objects(text);
        assert_eq!(objs.len(), 2, "应提取两个平衡对象,实际: {objs:?}");
        assert!(objs[0].contains("a{b}c"));
        assert_eq!(objs[1], "{\"x\":2}");
    }

    #[test]
    fn parse_classification_survives_leading_noise_objects() {
        // 第 79 轮核心场景:LLM 在真正的分类 JSON 之前回显了含 `{}` 的噪声
        // (用户输入带 HTML 围栏时高发),旧版取第一个 `{` 候选失败即降级,
        // 新版应继续尝试后续候选并成功解析。
        // 无 ```json 围栏,纯裸对象序列:噪声对象(缺 task_level 等必填字段)在前,
        // 真分类对象在后 → 旧版取首个 `{` 即失败降级,新版跳过噪声命中后者。
        let text = "我先分析一下任务:表单 {\"class\":\"ci-submit-button\"} 是提交按钮。\n\
                    分类如下:{\"task_level\":\"medium\",\"goal_summary\":\"打开文心一言对话\",\"intent\":\"web_dialog\"} 完毕";
        let c = parse_classification(text).unwrap();
        assert_eq!(c.task_level, TaskLevel::Medium);
        assert_eq!(c.goal_summary, "打开文心一言对话");
    }

    #[test]
    fn parse_classification_falls_back_to_second_json_block() {
        // 第一个 ```json 块非法(截断),第二个块合法 → 应解析成功
        let text = "```json\n{\"task_level\": \"simp\n```\n重试后:\n```json\n{\"task_level\":\"simple\",\"goal_summary\":\"g\",\"intent\":\"t\"}\n```";
        let c = parse_classification(text).unwrap();
        assert_eq!(c.task_level, TaskLevel::Simple);
    }

    #[test]
    fn parse_classification_all_candidates_fail_reports_count() {
        // 全部候选失败 → 错误信息含候选数(fail-closed 语义不变)
        let text = "```json\n{broken\n```\n还有 { 也broken";
        let err = match parse_classification(text) {
            Err(AgentError::YoloParse(msg)) => msg,
            other => panic!("应返回 YoloParse,实际: {other:?}"),
        };
        assert!(err.contains("候选"), "错误应含候选数,实际: {err}");
    }

    #[test]
    fn parse_three_levels() {
        for (level, role) in [("simple", "subagent"), ("medium", "main"), ("hard", "plan")] {
            let json = format!(
                r#"```json
{{
  "task_level": "{level}",
  "purpose": "测试目的",
  "goal_summary": "测试目标",
  "intent": "test",
  "agent_role": "{role}",
  "decomposition_plan": ["步骤1"],
  "direct_answer": null,
  "user_suggestion_if_fail": ""
}}
```"#
            );
            let result = parse_classification(&json).unwrap();
            assert_eq!(result.task_level.as_str(), level);
            assert_eq!(result.effective_agent_role().as_str(), role);
            assert_eq!(result.purpose, "测试目的");
        }
    }

    #[test]
    fn parse_classification_legacy_json_without_agent_role() {
        // 旧格式无 agent_role 字段
        let json = r#"```json
{
  "task_level": "simple",
  "goal_summary": "旧目标",
  "intent": "legacy",
  "decomposition_plan": [],
  "direct_answer": null
}
```"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.task_level, TaskLevel::Simple);
        assert_eq!(result.purpose, "");
        // agent_role 缺省时 effective_agent_role 应按 simple → SubAgent
        assert_eq!(result.effective_agent_role(), AgentRole::SubAgent);
    }

    #[test]
    fn build_work_prompt_contains_agent_role() {
        let c = TaskClassification {
            task_level: TaskLevel::Medium,
            purpose: "验证".into(),
            goal_summary: "测试目标".into(),
            intent: "code_change".into(),
            agent_role: Some(AgentRole::MainWork),
            decomposition_plan: vec!["第一步".into(), "第二步".into()],
            direct_answer: None,
            user_suggestion_if_fail: String::new(),
            yolo_degraded: false, // 关联报告: 2026-09-09_04 D-002(新字段)
            suggested_delegate: None,
        };
        let prompt = build_work_prompt(&c);
        assert!(prompt.contains("验证"));
        assert!(prompt.contains("测试目标"));
        assert!(prompt.contains("中等难度"));
        assert!(prompt.contains("委派 Agent: main"));
        assert!(prompt.contains("code_change"));
        assert!(prompt.contains("第一步"));
        assert!(prompt.contains("第二步"));
    }

    #[test]
    fn task_level_display_names() {
        assert_eq!(TaskLevel::Simple.display_name(), "简单");
        assert_eq!(TaskLevel::Medium.display_name(), "中等难度");
        assert_eq!(TaskLevel::Hard.display_name(), "高等难度");
    }

    // ========== F-003:Yolo 降级警告(2026-09-09_06,方案 tmpPlan/2026-09-09_06) ==========

    #[test]
    fn build_work_prompt_with_yolo_degraded_adds_warning() {
        // yolo_degraded=true → prompt 应含「Yolo 分类解析失败」警告 + actionable 建议
        let c = TaskClassification {
            task_level: TaskLevel::Simple,
            purpose: String::new(),
            goal_summary: "(解析失败,已降级)".into(),
            intent: "unknown".into(),
            agent_role: Some(AgentRole::SubAgent),
            decomposition_plan: vec![],
            direct_answer: None,
            user_suggestion_if_fail: String::new(),
            yolo_degraded: true,
            suggested_delegate: None,
        };
        let prompt = build_work_prompt(&c);
        assert!(
            prompt.contains("Yolo 分类解析失败"),
            "降级模式应含警告,实际: {prompt}"
        );
        assert!(
            prompt.contains("已降级为 Simple"),
            "应说明降级结果,实际: {prompt}"
        );
        assert!(
            prompt.contains("建议"),
            "应给 actionable 建议,实际: {prompt}"
        );
    }

    #[test]
    fn build_work_prompt_normal_no_warning() {
        // yolo_degraded=false → prompt 不应含警告
        let c = TaskClassification {
            task_level: TaskLevel::Medium,
            purpose: "验证".into(),
            goal_summary: "正常目标".into(),
            intent: "code_change".into(),
            agent_role: Some(AgentRole::MainWork),
            decomposition_plan: vec!["步骤1".into()],
            direct_answer: None,
            user_suggestion_if_fail: String::new(),
            yolo_degraded: false,
            suggested_delegate: None,
        };
        let prompt = build_work_prompt(&c);
        assert!(
            !prompt.contains("Yolo 分类解析失败"),
            "正常模式不应含降级警告,实际: {prompt}"
        );
    }

    // ========== 2026-09-16 第 59 轮:suggested_delegate 推断测试 ==========

    #[test]
    fn infer_suggested_delegate_desktop_window_no_longer_windowuse() {
        // 2026-09-18 第 84 轮:窗口操控类任务不再产出 "windowuse"
        // (WindowUse Agent 已删除,桌面窗口操控由 SubAgent-Work 的
        // MCP_Window_Use 工具承担;返回 None 交 Main-Work 关键词推断)。
        let cases = [
            "帮我打开微信软件,找到赵玲玲,给她发一个消息",
            "打开 QQ 给张三发一条消息",
            "启动 Slack 切换到工作区",
            "open WeChat and send a message",
            "点击按钮关闭窗口",
        ];
        for prompt in cases {
            assert_ne!(
                infer_suggested_delegate(prompt),
                Some("windowuse".to_string()),
                "窗口操控类不应再推断 windowuse: {prompt}"
            );
        }
    }

    #[test]
    fn infer_suggested_delegate_subagent_keywords() {
        // 代码/文件操作类任务 → subagent
        let cases = [
            "编写一个 Rust 函数实现排序算法",
            "修改 main.rs 中的 bug",
            "创建一个新的 Python 脚本",
            "write code for a web server",
        ];
        for prompt in cases {
            assert_eq!(
                infer_suggested_delegate(prompt),
                Some("subagent".to_string()),
                "代码类应推断 subagent: {prompt}"
            );
        }
    }

    #[test]
    fn infer_suggested_delegate_web_keywords_return_none() {
        // 2026-09-18 第 89 轮:Chromium-WebUse Agent 已删除(降级为 MCP_Web_Use 工具),
        // 网页类关键词不再产出 "webuse";执行器统一 SubAgent,无需专项委派。
        let cases = [
            "帮我打开网页 https://example.com 并截图",
            "抓取这个网站的标题列表",
            "登录网站后台下载报表",
            "用浏览器访问 https://example.com 查看控制台报错",
        ];
        for prompt in cases {
            assert_eq!(
                infer_suggested_delegate(prompt),
                None,
                "网页操控类不再产出专项委派: {prompt}"
            );
        }
        // 「修改网页代码」含代码关键词 → 判 subagent。
        assert_eq!(
            infer_suggested_delegate("修改网页代码里的 bug"),
            Some("subagent".to_string())
        );
    }

    #[test]
    fn infer_suggested_delegate_ambiguous_returns_none() {
        // 无法判断 → None
        let cases = [
            "帮我处理一下这个任务",       // 无明确关键词
            "",                           // 空串
        ];
        for prompt in cases {
            assert_eq!(
                infer_suggested_delegate(prompt),
                None,
                "模糊/空串应返回 None: {prompt}"
            );
        }
    }

    #[test]
    fn extract_user_prompt_extracts_last_user_message() {
        let context = vec![
            ChatMessage::assistant(vec![ContentBlock::text("上一轮回复")]),
            ChatMessage::user("  Write 备份脚本\n并跑一次验证  "),
            ChatMessage::assistant(vec![ContentBlock::text("中间回复")]),
            ChatMessage::user("最新的用户输入"),
        ];
        assert_eq!(extract_user_prompt(&context), "最新的用户输入");
    }

    #[test]
    fn extract_user_prompt_empty_without_user() {
        let context = vec![ChatMessage::assistant(vec![ContentBlock::text(
            "只有 assistant 消息",
        )])];
        assert_eq!(extract_user_prompt(&context), "");
    }

    #[test]
    fn parse_classification_includes_user_suggestion() {
        let json = r#"```json
{
  "task_level": "hard",
  "goal_summary": "重写架构",
  "intent": "code_change",
  "agent_role": "plan",
  "decomposition_plan": ["分析", "设计"],
  "direct_answer": null,
  "user_suggestion_if_fail": "请补充更多信息"
}
```"#;
        let result = parse_classification(json).unwrap();
        assert_eq!(result.user_suggestion_if_fail, "请补充更多信息");
    }

    #[test]
    fn yolo_parse_failure_counter_starts_at_zero_and_resets() {
        // 关联报告: 20260908_203854 D-002
        // 解析失败时计数器 +1,初始 0,reset 后归零。
        reset_yolo_parse_failures();
        assert_eq!(yolo_parse_failures(), 0);

        // 模拟一次失败增加(走静态原子,等价于生产代码路径)
        YOLO_PARSE_FAILURES.fetch_add(1, Ordering::Relaxed);
        assert_eq!(yolo_parse_failures(), 1);
        YOLO_PARSE_FAILURES.fetch_add(1, Ordering::Relaxed);
        assert_eq!(yolo_parse_failures(), 2);

        reset_yolo_parse_failures();
        assert_eq!(yolo_parse_failures(), 0);
    }

    // ========== X-Session-Id 传播(第 08 轮,方案 tmpPlan/2026-09-09_08) ==========

    /// 捕获 complete() 收到的 session_id,验证 classify 传播任务主会话 ID。
    struct SessionCaptureLlm {
        seen: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl crate::llm::LlmClient for SessionCaptureLlm {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[crate::llm::ToolDef],
            meta: &crate::llm::RequestMeta,
        ) -> Result<crate::llm::Completion> {
            self.seen
                .lock()
                .expect("session capture")
                .push(meta.session_id.clone());
            Ok(crate::llm::Completion {
                text: r#"```json
{"task_level": "simple", "goal_summary": "g", "intent": "t", "decomposition_plan": [], "direct_answer": null}
```"#
                .into(),
                tool_calls: vec![],
                usage: Usage::default(),
                stop_reason: None,
            })
        }
        fn protocol(&self) -> crate::config::Protocol {
            crate::config::Protocol::Anthropic
        }
    }

    #[tokio::test]
    async fn classify_propagates_parent_session_id() {
        let cap = std::sync::Arc::new(SessionCaptureLlm {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let runner = YoloRunner::new(cap.clone());
        let (c, _text, _usage) = runner
            .classify(
                "20260909-120000-abcd1234-1700000000000-1a2b3c",
                &[ChatMessage::user("hi")],
            )
            .await
            .unwrap();
        assert_eq!(c.task_level, TaskLevel::Simple);
        let seen = cap.seen.lock().expect("session capture");
        assert!(!seen.is_empty());
        assert!(
            seen.iter()
                .all(|s| s == "20260909-120000-abcd1234-1700000000000-1a2b3c"),
            "Yolo 请求的 X-Session-Id 应为任务主会话 ID,实际: {seen:?}"
        );
    }
}
