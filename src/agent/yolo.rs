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

use serde::Deserialize;

use crate::agent::context::AgentRole;
use crate::agent::{Agent, AgentProfile};
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Usage};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
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
        let classification = parse_classification(&text).unwrap_or_else(|e| {
            YOLO_PARSE_FAILURES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("Yolo 分类解析失败,降级为 simple: {}", e);
            TaskClassification {
                task_level: TaskLevel::Simple,
                purpose: String::new(),
                goal_summary: "(解析失败,已降级)".to_string(),
                intent: "unknown".to_string(),
                agent_role: Some(AgentRole::SubAgent),
                decomposition_plan: vec![],
                direct_answer: None,
                user_suggestion_if_fail: String::new(),
                yolo_degraded: true, // 关联报告: 2026-09-09_04 D-002
            }
        });
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
            TaskClassification {
                task_level: TaskLevel::Simple,
                purpose: String::new(),
                goal_summary: "(解析失败,已降级)".to_string(),
                intent: "unknown".to_string(),
                agent_role: Some(AgentRole::SubAgent),
                decomposition_plan: vec![],
                direct_answer: None,
                user_suggestion_if_fail: String::new(),
                yolo_degraded: true, // 关联报告: 2026-09-09_04 D-002
            }
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

/// 从 Yolo 返回的文本中解析结构化分类结果。
///
/// 优先匹配 ```json ... ``` 代码块;找不到则尝试匹配最大合法 JSON 对象。
/// 直接解析失败时自动走 JSON 修复链(`json_repair`,Tier-1 语法修复),
/// 修复不了仍返回 YoloParse(fail-closed 不变)。
pub fn parse_classification(text: &str) -> Result<TaskClassification> {
    if let Some(json_str) = extract_json_block(text) {
        // Yolo 路径:启用 Tier-2 截断补全(LLM 输出被 token 触顶截断的场景),
        // 仅在 Quality-Check fail-closed 路径禁用。关联报告: 2026-09-09_04 D-001。
        return crate::agent::json_repair::try_parse_lenient(json_str)
            .map_err(AgentError::YoloParse);
    }
    if let Some(json_str) = extract_standalone_json(text) {
        return crate::agent::json_repair::try_parse_lenient(json_str)
            .map_err(AgentError::YoloParse);
    }
    Err(AgentError::YoloParse(
        "未找到合法的 JSON 分类结果".to_string(),
    ))
}

/// 提取第一个 ```json ... ``` 代码块中的内容。
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
    let json_text = &text[content_start..content_start + end];
    let json_text = json_text.trim();
    if json_text.is_empty() {
        None
    } else {
        Some(json_text)
    }
}

/// 尝试提取文本中第一个顶层合法 JSON 对象。
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
    fn extract_json_block_basic() {
        let text = "一些前置文本\n```json\n{\"key\": \"value\"}\n```\n后续文本";
        let result = extract_json_block(text).unwrap();
        assert_eq!(result, "{\"key\": \"value\"}");
    }

    #[test]
    fn extract_json_block_no_block() {
        let text = "没有 JSON 代码块的普通文本";
        assert!(extract_json_block(text).is_none());
    }

    #[test]
    fn extract_standalone_json_nested() {
        let text = "prefix {\"a\": {\"b\": 1}, \"c\": [1,2]} suffix";
        let result = extract_standalone_json(text).unwrap();
        assert_eq!(result, "{\"a\": {\"b\": 1}, \"c\": [1,2]}");
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
        };
        let prompt = build_work_prompt(&c);
        assert!(
            !prompt.contains("Yolo 分类解析失败"),
            "正常模式不应含降级警告,实际: {prompt}"
        );
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
            .classify("20260909-120000-abcd1234-1700000000000-1a2b3c", &[ChatMessage::user("hi")])
            .await
            .unwrap();
        assert_eq!(c.task_level, TaskLevel::Simple);
        let seen = cap.seen.lock().expect("session capture");
        assert!(!seen.is_empty());
        assert!(
            seen.iter().all(|s| s == "20260909-120000-abcd1234-1700000000000-1a2b3c"),
            "Yolo 请求的 X-Session-Id 应为任务主会话 ID,实际: {seen:?}"
        );
    }
}
