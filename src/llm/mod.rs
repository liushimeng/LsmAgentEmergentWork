//! LLM 客户端抽象与统一消息模型。
//!
//! 通过 [`LlmClient`] trait 屏蔽不同协议差异,内部实现 Anthropic Messages 与
//! OpenAI Chat Completions 两种协议。

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Protocol, ProviderRecord};
use crate::error::Result;

pub mod anthropic;
pub mod openai;

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
}

impl Completion {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
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
    ) -> Result<Completion>;
}

/// 根据数据库记录创建对应协议的客户端
pub fn client_from_record(record: &ProviderRecord) -> Result<Arc<dyn LlmClient>> {
    match record.protocol {
        Protocol::Anthropic => {
            let c = anthropic::AnthropicClient::new(
                &record.end_point,
                &record.api_key,
                &record.model_name,
            )?;
            Ok(Arc::new(c))
        }
        Protocol::OpenAi => {
            let c = openai::OpenAiClient::new(
                &record.end_point,
                &record.api_key,
                &record.model_name,
            )?;
            Ok(Arc::new(c))
        }
    }
}

/// 规整 end_point:去除尾部 `/`
pub fn normalize_endpoint(ep: &str) -> String {
    ep.trim().trim_end_matches('/').to_string()
}
