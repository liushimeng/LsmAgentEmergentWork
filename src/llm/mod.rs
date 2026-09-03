//! LLM 客户端抽象与统一消息模型。
//!
//! 通过 [`LlmClient`] trait 屏蔽不同协议差异,内部实现 Anthropic Messages 与
//! OpenAI Chat Completions 两种协议。

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Protocol, ProviderRecord};
use crate::error::{AgentError, Result};

pub mod anthropic;
pub mod openai;
pub mod sse;

/// 请求元数据:会话 / 设备标识,由协议层写入 HTTP 头与请求体。
#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub session_id: String,
    pub device_id: String,
}

/// 构造两协议通用的请求头:`Content-Type` / `User-Agent` / `Authorization` / `X-Session-Id`。
pub fn build_common_headers(api_key: &str, meta: &RequestMeta, user_agent: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("user-agent"),
        HeaderValue::from_str(user_agent)
            .map_err(|e| AgentError::Llm(format!("User-Agent header 非法: {e}")))?,
    );
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|e| AgentError::Llm(format!("Authorization header 非法: {e}")))?,
    );
    headers.insert(
        HeaderName::from_static("x-session-id"),
        HeaderValue::from_str(&meta.session_id)
            .map_err(|e| AgentError::Llm(format!("X-Session-Id header 非法: {e}")))?,
    );
    Ok(headers)
}

/// 对话角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 消息内容块(统一表示文本 / 工具调用 / 工具结果)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }

    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
        }
    }
}

/// 一条对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl ChatMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, content: vec![ContentBlock::text(text)] }
    }

    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self { role: Role::Assistant, content: blocks }
    }

    pub fn tool_result(tool_use_id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentBlock::tool_result(tool_use_id, content, is_error)],
        }
    }
}

/// 工具定义(协议无关)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema 对象,对应 Anthropic 的 `input_schema` / OpenAI 的 `parameters`
    pub input_schema: Value,
}

impl ToolDef {
    pub fn new(name: impl Into<String>, description: impl Into<String>, input_schema: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// 一次补全的结果:文本与工具调用可同时存在(以文本为主,工具调用为辅)
#[derive(Debug, Default, Clone)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCallReq>,
    /// Token 用量(由 SSE 流中的 message_start / message_delta / 尾部 usage chunk 汇总)。
    pub usage: Usage,
    /// 终止原因(Anthropic stop_reason / OpenAI finish_reason),可选。
    pub stop_reason: Option<String>,
}

impl Completion {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// Token 用量统计。所有字段为 0 表示上游未提供。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入侧 token 数(Anthropic input_tokens / OpenAI prompt_tokens)。
    pub input_tokens: u32,
    /// 输出侧 token 数(Anthropic output_tokens / OpenAI completion_tokens)。
    pub output_tokens: u32,
    /// Anthropic cache_read_input_tokens / OpenAI prompt_tokens_details.cached_tokens。
    pub cache_read_input_tokens: u32,
    /// Anthropic cache_creation_input_tokens(可选,OpenAI 不发)。
    pub cache_creation_input_tokens: u32,
}

/// 一次工具调用请求(协议无关)
#[derive(Debug, Clone)]
pub struct ToolCallReq {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// LLM 客户端抽象
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion>;

    /// 当前客户端使用的协议(用于系统提示词按协议渲染)。
    fn protocol(&self) -> Protocol;
}

/// 根据数据库记录创建对应协议的用户端(注入 User-Agent)。
pub fn client_from_record(record: &ProviderRecord, user_agent: &str) -> Result<Arc<dyn LlmClient>> {
    match record.protocol {
        Protocol::Anthropic => {
            let c = anthropic::AnthropicClient::new(
                &record.end_point,
                &record.api_key,
                &record.model_name,
                user_agent,
            )?;
            Ok(Arc::new(c))
        }
        Protocol::OpenAi => {
            let c = openai::OpenAiClient::new(
                &record.end_point,
                &record.api_key,
                &record.model_name,
                user_agent,
            )?;
            Ok(Arc::new(c))
        }
    }
}

/// 规整 end_point:去除尾部 `/`
pub fn normalize_endpoint(ep: &str) -> String {
    ep.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_common_headers_contains_all() {
        let meta = RequestMeta {
            session_id: "20260902-153012-abcd1234-1700000000000-1a2b3c".into(),
            device_id: "45d277355416ee1b2f42758fb292b60b45170a57a5b4dec5cb7fa1a40fdd17ec".into(),
        };
        let headers = build_common_headers("sk-xxx", &meta, "LsmAgentEmergentWork-Work/0.1.0 2026-09-02 15:30:12 CST").unwrap();
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
        let ua = headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ua, "LsmAgentEmergentWork-Work/0.1.0 2026-09-02 15:30:12 CST");
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer sk-xxx");
        assert_eq!(
            headers.get("x-session-id").unwrap(),
            "20260902-153012-abcd1234-1700000000000-1a2b3c"
        );
    }
}
