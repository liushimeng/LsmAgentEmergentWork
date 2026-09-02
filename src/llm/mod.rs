//! 大模型客户端抽象。
//!
//! 通过 [`LlmClient`] trait 屏蔽不同供应商差异,
//! 内置 OpenAI 兼容协议(同样适配 DeepSeek / Qwen / Moonshot 等)实现。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{AgentError, Result};

/// 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// assistant 发起工具调用时携带
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// role 为 tool 时, 对应工具调用的 id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 模型请求的一次工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON 字符串形式的参数
    pub arguments: String,
}

/// 工具 Schema(function calling 定义)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub function: FunctionSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// 一次补全的结果: 要么最终文本, 要么请求调用工具
#[derive(Debug)]
pub enum Completion {
    Text(String),
    ToolCalls(Vec<ToolCall>),
}

/// 大模型客户端抽象
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
    ) -> Result<Completion>;
}

/// OpenAI 兼容协议客户端
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiClient {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: &'a [ToolSchema],
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
    ) -> Result<Completion> {
        let req = ChatRequest {
            model: &self.model,
            messages,
            tools,
        };

        let resp = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::Llm(format!("HTTP {status}: {body}")));
        }

        let chat: ChatResponse = resp.json().await?;
        let msg = chat
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AgentError::Llm("响应中没有 choices".into()))?
            .message;

        if let Some(calls) = msg.tool_calls.filter(|c| !c.is_empty()) {
            return Ok(Completion::ToolCalls(calls));
        }
        Ok(Completion::Text(msg.content.unwrap_or_default()))
    }
}
