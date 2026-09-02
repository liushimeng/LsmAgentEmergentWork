//! Yolo Agent —— 入口级 Agent,负责目标识别、意图识别、任务分类与拆解。
//!
//! Yolo 是用户输入的第一站。它分析用户请求,按难度分为四级:
//! - `trivial`(极其简单):直接回答
//! - `simple`(简单):直接委派给 Work Agent
//! - `medium`(中等难度):给出拆解计划后委派
//! - `hard`(高等难度):给出详细分阶段计划后委派
//!
//! Yolo 仅持有 Read 工具(用于理解上下文),不持有 Bash/Write 等会改变系统状态的工具。

use std::sync::Arc;

use serde::Deserialize;

use crate::agent::{Agent, AgentProfile};
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Usage};
use crate::session::Session;

/// 任务难度等级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskLevel {
    /// 极其简单:无需工具,直接回答
    Trivial,
    /// 简单:单步操作,直接委派
    Simple,
    /// 中等难度:需要 2-5 步操作
    Medium,
    /// 高等难度:复杂系统 / 多模块 / 深度理解
    Hard,
}

impl TaskLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskLevel::Trivial => "trivial",
            TaskLevel::Simple => "simple",
            TaskLevel::Medium => "medium",
            TaskLevel::Hard => "hard",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            TaskLevel::Trivial => "极其简单",
            TaskLevel::Simple => "简单",
            TaskLevel::Medium => "中等难度",
            TaskLevel::Hard => "高等难度",
        }
    }
}

/// Yolo 输出的结构化分类结果
#[derive(Debug, Clone, Deserialize)]
pub struct TaskClassification {
    pub task_level: TaskLevel,
    pub goal_summary: String,
    pub intent: String,
    #[serde(default)]
    pub decomposition_plan: Vec<String>,
    pub direct_answer: Option<String>,
}

/// Yolo 执行结果
pub enum YoloOutcome {
    /// 直接回答(trivial 场景)
    DirectAnswer {
        text: String,
        classification: TaskClassification,
        usage: Usage,
    },
    /// 委派给 Work Agent(simple / medium / hard 场景)
    DelegateToWork {
        classification: TaskClassification,
        /// Yolo 的自然语言分析文本(可展示给用户)
        yolo_text: String,
        /// Yolo 阶段的 token 用量
        usage: Usage,
    },
}

/// Yolo Runner:持有 Yolo + Work 两个 Agent,编排执行流程
pub struct YoloRunner {
    yolo_agent: Agent,
    work_agent: Agent,
}

impl YoloRunner {
    /// 基于同一个 LLM 客户端构造 Yolo + Work 双 Agent。
    pub fn new(llm: Arc<dyn crate::llm::LlmClient>) -> Self {
        let yolo = Agent::new(llm.clone(), AgentProfile::yolo_profile()).with_max_iterations(4);
        let work = Agent::new(llm, AgentProfile::work_profile());
        Self {
            yolo_agent: yolo,
            work_agent: work,
        }
    }

    /// 用自定义 max_iterations 构造 Work Agent
    pub fn with_work_max_iterations(llm: Arc<dyn crate::llm::LlmClient>, max_iter: usize) -> Self {
        let yolo = Agent::new(llm.clone(), AgentProfile::yolo_profile()).with_max_iterations(4);
        let work = Agent::new(llm, AgentProfile::work_profile()).with_max_iterations(max_iter);
        Self {
            yolo_agent: yolo,
            work_agent: work,
        }
    }

    /// Yolo Agent 引用
    pub fn yolo_agent(&self) -> &Agent {
        &self.yolo_agent
    }

    /// Work Agent 引用
    pub fn work_agent(&self) -> &Agent {
        &self.work_agent
    }

    /// 处理一次用户输入:Yolo 分类 → 直接回答 / 委派 Work。
    ///
    /// - `session`: 主会话(包含历史上下文);Yolo 阶段的输出会入栈
    /// - 返回:(最终回复文本, 累计 token 用量)
    pub async fn handle(&self, session: &mut Session) -> Result<(String, Usage)> {
        // --- 阶段 1: Yolo 分类 ---
        let outcome = run_yolo(&self.yolo_agent, session.context()).await?;

        let mut total_usage = Usage::default();

        match outcome {
            YoloOutcome::DirectAnswer { text, classification, usage } => {
                total_usage = add_usage(total_usage, usage);
                // Yolo 已直接回答,作为 assistant 消息入栈
                session
                    .context_mut()
                    .push(ChatMessage::assistant(vec![crate::llm::ContentBlock::text(
                        text.clone(),
                    )]));
                // 在文本末尾附上 Yolo 分类标记(便于调试/透明性)
                let final_text = format!(
                    "{}\n\n[yolo 分类: {} / 意图: {}]",
                    text,
                    classification.task_level.display_name(),
                    classification.intent
                );
                Ok((final_text, total_usage))
            }
            YoloOutcome::DelegateToWork { classification, yolo_text, usage } => {
                total_usage = add_usage(total_usage, usage);

                // 将 Yolo 的分析作为 assistant 消息入栈,保持对话上下文连贯
                session
                    .context_mut()
                    .push(ChatMessage::assistant(vec![crate::llm::ContentBlock::text(
                        yolo_text.clone(),
                    )]));

                // 构造 Work 的任务提示(结构化计划 + 原始需求)
                let work_prompt = build_work_prompt(&classification);
                session
                    .context_mut()
                    .push(ChatMessage::user(work_prompt));

                // --- 阶段 2: Work Agent 执行 ---
                let (work_text, work_usage) = self.work_agent.run_session(session).await?;
                total_usage = add_usage(total_usage, work_usage);

                Ok((work_text, total_usage))
            }
        }
    }

    /// 用已有的 Agent 实例构造(用于特殊场景,如 NoopLlm 占位)。
    pub fn from_agents(yolo_agent: Agent, work_agent: Agent) -> Self {
        Self {
            yolo_agent,
            work_agent,
        }
    }

    /// 为 Work Agent 添加环境上下文尾部(用于 -p / -f 单轮模式)。
    pub fn with_work_env_tail(&self, tail: &str) -> Self {
        let work_profile = self.work_agent.profile().with_env_tail(tail);
        let yolo_profile = self.yolo_agent.profile().clone();
        let llm_work = self.work_agent.llm();
        let llm_yolo = self.yolo_agent.llm();
        Self {
            yolo_agent: Agent::new(llm_yolo, yolo_profile).with_max_iterations(4),
            work_agent: Agent::new(llm_work, work_profile)
                .with_max_iterations(self.work_agent.max_iterations()),
        }
    }
}

/// 运行 Yolo Agent:从已有上下文中分析最后一条用户消息,返回分类结果。
async fn run_yolo(yolo_agent: &Agent, context: &[ChatMessage]) -> Result<YoloOutcome> {
    // 构造 Yolo 的独立 session(拷贝历史上下文)
    let mut yolo_session = Session::new();
    // 拷贝上下文(不含本轮输入 — 由调用方负责把本轮输入放入 context)
    for msg in context {
        yolo_session.context_mut().push(msg.clone());
    }

    let (text, usage) = yolo_agent.run_session(&mut yolo_session).await?;

    // 解析 JSON 代码块
    let classification = match parse_classification(&text) {
        Ok(c) => c,
        Err(e) => {
            // 容错:解析失败降级为 simple,直接委派
            tracing::warn!("Yolo 分类解析失败,降级为 simple: {}", e);
            TaskClassification {
                task_level: TaskLevel::Simple,
                goal_summary: "(解析失败,已降级)".to_string(),
                intent: "unknown".to_string(),
                decomposition_plan: vec![],
                direct_answer: None,
            }
        }
    };

    match classification.task_level {
        TaskLevel::Trivial => {
            let answer = classification
                .direct_answer
                .clone()
                .unwrap_or_else(|| text.clone());
            Ok(YoloOutcome::DirectAnswer {
                text: answer,
                classification,
                usage,
            })
        }
        _ => Ok(YoloOutcome::DelegateToWork {
            classification,
            yolo_text: text,
            usage,
        }),
    }
}

/// 从 Yolo 返回的文本中解析结构化分类结果。
///
/// 优先匹配 ```json ... ``` 代码块;找不到则尝试匹配最大合法 JSON 对象。
fn parse_classification(text: &str) -> Result<TaskClassification> {
    // 1. 尝试匹配 ```json ... ``` 代码块
    if let Some(json_str) = extract_json_block(text) {
        return serde_json::from_str::<TaskClassification>(json_str).map_err(|e| {
            AgentError::YoloParse(format!("JSON 代码块解析失败: {}", e))
        });
    }

    // 2. 容错:尝试匹配第一个 { ... } 结构
    if let Some(json_str) = extract_standalone_json(text) {
        return serde_json::from_str::<TaskClassification>(json_str).map_err(|e| {
            AgentError::YoloParse(format!("JSON 对象解析失败: {}", e))
        });
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
    // 跳过起始标记后的换行/空白
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

/// 尝试提取文本中第一个顶层合法 JSON 对象(从第一个 '{' 到匹配的 '}')。
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

/// 根据分类结果构造 Work Agent 的任务提示。
fn build_work_prompt(classification: &TaskClassification) -> String {
    let mut prompt = String::new();
    prompt.push_str("【任务计划】\n");
    prompt.push_str(&format!("目标: {}\n", classification.goal_summary));
    prompt.push_str(&format!(
        "难度: {} ({})\n",
        classification.task_level.display_name(),
        classification.task_level.as_str()
    ));
    prompt.push_str(&format!("意图: {}\n", classification.intent));

    if !classification.decomposition_plan.is_empty() {
        prompt.push_str("\n分解步骤:\n");
        for (i, step) in classification.decomposition_plan.iter().enumerate() {
            prompt.push_str(&format!("  {}. {}\n", i + 1, step));
        }
    }

    prompt.push_str(
        "\n请按照以上计划执行任务。使用可用工具完成目标,完成后用简洁中文回复最终结果。",
    );
    prompt
}

/// 累加 token 用量
fn add_usage(mut total: Usage, delta: Usage) -> Usage {
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
    fn extract_json_block_with_newlines() {
        let text = "```json\n{\n  \"a\": 1,\n  \"b\": 2\n}\n```";
        let result = extract_json_block(text).unwrap();
        assert!(result.contains("\"a\": 1"));
        assert!(result.contains("\"b\": 2"));
    }

    #[test]
    fn extract_standalone_json_simple() {
        let text = "一些文字 {\"x\": 1} 更多";
        let result = extract_standalone_json(text).unwrap();
        assert_eq!(result, "{\"x\": 1}");
    }

    #[test]
    fn extract_standalone_json_nested() {
        let text = "prefix {\"a\": {\"b\": 1}, \"c\": [1,2]} suffix";
        let result = extract_standalone_json(text).unwrap();
        assert_eq!(result, "{\"a\": {\"b\": 1}, \"c\": [1,2]}");
    }

    #[test]
    fn parse_classification_all_levels() {
        for level in ["trivial", "simple", "medium", "hard"] {
            let json = format!(
                r#"```json
{{
  "task_level": "{}",
  "goal_summary": "测试目标",
  "intent": "test",
  "decomposition_plan": ["步骤1"],
  "direct_answer": {}
}}
```"#,
                level,
                if level == "trivial" {
                    "\"直接回答\""
                } else {
                    "null"
                }
            );
            let result = parse_classification(&json).unwrap();
            assert_eq!(result.goal_summary, "测试目标");
            assert_eq!(result.intent, "test");
            if level == "trivial" {
                assert_eq!(result.task_level, TaskLevel::Trivial);
                assert!(result.direct_answer.is_some());
            } else if level == "simple" {
                assert_eq!(result.task_level, TaskLevel::Simple);
            } else if level == "medium" {
                assert_eq!(result.task_level, TaskLevel::Medium);
            } else {
                assert_eq!(result.task_level, TaskLevel::Hard);
            }
        }
    }

    #[test]
    fn build_work_prompt_contains_all_fields() {
        let c = TaskClassification {
            task_level: TaskLevel::Medium,
            goal_summary: "测试目标".into(),
            intent: "code_change".into(),
            decomposition_plan: vec!["第一步".into(), "第二步".into()],
            direct_answer: None,
        };
        let prompt = build_work_prompt(&c);
        assert!(prompt.contains("测试目标"));
        assert!(prompt.contains("中等难度"));
        assert!(prompt.contains("code_change"));
        assert!(prompt.contains("第一步"));
        assert!(prompt.contains("第二步"));
    }

    #[test]
    fn task_level_display_names() {
        assert_eq!(TaskLevel::Trivial.display_name(), "极其简单");
        assert_eq!(TaskLevel::Simple.display_name(), "简单");
        assert_eq!(TaskLevel::Medium.display_name(), "中等难度");
        assert_eq!(TaskLevel::Hard.display_name(), "高等难度");
    }
}
