//! Anthropic Messages 协议客户端
//!
//! - 请求: `POST {end_point}/v1/messages`
//! - Header: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`
//! - 工具: `tools: [{ name, description, input_schema }]`
//! - 消息 content: `text` / `tool_use` / `tool_result`

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{AgentError, Result};
use crate::llm::{build_common_headers, normalize_endpoint, ChatMessage, Completion, ContentBlock, LlmClient, RequestMeta, ToolCallReq, ToolDef};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct AnthropicClient {
    http: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    user_agent: String,
}

impl AnthropicClient {
    pub fn new(end_point: &str, api_key: &str, model: &str, user_agent: &str) -> Result<Self> {
        let url = format!("{}/v1/messages", normalize_endpoint(end_point));
        Ok(Self {
            http: reqwest::Client::new(),
            url,
            api_key: api_key.to_string(),
            model: model.to_string(),
            user_agent: user_agent.to_string(),
        })
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    /// Anthropic 会话/设备标识(metadata.user_id)。
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Metadata>,
}

#[derive(Serialize)]
struct Metadata {
    /// JSON 字符串,形如: {"device_id":"...","account_uuid":"","session_id":"..."}
    user_id: String,
}

/// 构造 metadata.user_id 的 JSON 字符串。
fn build_user_id(device_id: &str, session_id: &str) -> String {
    json!({
        "device_id": device_id,
        "account_uuid": "",
        "session_id": session_id,
    })
    .to_string()
}

fn convert_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            let role = match m.role {
                crate::llm::Role::User => "user",
                crate::llm::Role::Assistant => "assistant",
                // Anthropic 协议下 Tool 结果以 user + tool_result 块表达
                crate::llm::Role::Tool => "user",
                // System 字段在请求体顶层,不进入 messages
                crate::llm::Role::System => "user",
            };
            let content: Vec<Value> = m
                .content
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
                    ContentBlock::ToolUse { id, name, input } => {
                        json!({ "type": "tool_use", "id": id, "name": name, "input": input })
                    }
                    ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                        let mut v = json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                        });
                        if *is_error {
                            v["is_error"] = json!(true);
                        }
                        v
                    }
                })
                .collect();
            json!({ "role": role, "content": content })
        })
        .collect()
}

fn convert_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect()
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion> {
        let req = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: DEFAULT_MAX_TOKENS,
            system: if system.trim().is_empty() { None } else { Some(system.to_string()) },
            messages: convert_messages(messages),
            tools: convert_tools(tools),
            metadata: Some(Metadata {
                user_id: build_user_id(&meta.device_id, &meta.session_id),
            }),
        };

        // 通用头:Content-Type / User-Agent / Authorization / X-Session-Id
        let mut headers = build_common_headers(&self.api_key, meta, &self.user_agent)?;
        // Anthropic 专属头:x-api-key + anthropic-version(保留官方 SDK 风格)
        headers.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(&self.api_key)
                .map_err(|e| AgentError::Llm(format!("x-api-key header 非法: {e}")))?,
        );
        headers.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );

        let resp = self
            .http
            .post(&self.url)
            .headers(headers)
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(AgentError::Llm(format!("HTTP {status}: {body_text}")));
        }
        let body: Value = serde_json::from_str(&body_text)?;
        parse_response(&body)
    }
}

fn parse_response(body: &Value) -> Result<Completion> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| AgentError::Llm("Anthropic 响应缺少 content[]".into()))?;

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCallReq> = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
            Some("tool_use") => {
                let id = block.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let name = block.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                tool_calls.push(ToolCallReq { id, name, arguments: input });
            }
            _ => {}
        }
    }
    Ok(Completion { text, tool_calls })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;
    use serde_json::json;

    #[test]
    fn url_appends_v1_messages() {
        let c = AnthropicClient::new("https://api.example.com/", "k", "m", "ua").unwrap();
        assert_eq!(c.url, "https://api.example.com/v1/messages");
    }

    #[test]
    fn url_trims_trailing_slash() {
        let c = AnthropicClient::new("https://api.example.com", "k", "m", "ua").unwrap();
        assert_eq!(c.url, "https://api.example.com/v1/messages");
    }

    #[test]
    fn convert_messages_handles_tool_results() {
        let msgs = vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Bash".into(),
                input: json!({"command": "ls"}),
            }]),
            ChatMessage::tool_result("t1", "file1\nfile2", false),
        ];
        let v = convert_messages(&msgs);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0]["role"], "user");
        assert_eq!(v[1]["role"], "assistant");
        assert_eq!(v[2]["role"], "user");
        assert_eq!(v[2]["content"][0]["type"], "tool_result");
        assert_eq!(v[2]["content"][0]["tool_use_id"], "t1");
    }

    #[test]
    fn convert_tools_format() {
        let tools = vec![ToolDef::new("Bash", "run cmd", json!({"type": "object"}))];
        let v = convert_tools(&tools);
        assert_eq!(v[0]["name"], "Bash");
        assert_eq!(v[0]["input_schema"]["type"], "object");
    }

    #[test]
    fn parse_response_text_and_tool_use() {
        let body = json!({
            "content": [
                {"type": "text", "text": "hello "},
                {"type": "tool_use", "id": "id1", "name": "Read", "input": {"path": "/a"}},
                {"type": "text", "text": "world"},
            ]
        });
        let c = parse_response(&body).unwrap();
        assert_eq!(c.text, "hello world");
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].name, "Read");
        assert_eq!(c.tool_calls[0].id, "id1");
        assert_eq!(c.tool_calls[0].arguments["path"], "/a");
    }

    #[test]
    fn request_skips_empty_system() {
        // 确认 system 字段在 system 为空时被跳过
        let req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 1,
            system: None,
            messages: vec![],
            tools: vec![],
            metadata: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains("system"));
    }

    #[test]
    fn request_metadata_user_id_is_json_string() {
        let req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 1,
            system: None,
            messages: vec![],
            tools: vec![],
            metadata: Some(Metadata {
                user_id: build_user_id("dev1234567890", "sess-1"),
            }),
        };
        let s = serde_json::to_string(&req).unwrap();
        // metadata.user_id 应为 JSON 字符串
        let v: Value = serde_json::from_str(&s).unwrap();
        let uid = v["metadata"]["user_id"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(uid).unwrap();
        assert_eq!(parsed["device_id"], "dev1234567890");
        assert_eq!(parsed["session_id"], "sess-1");
        assert_eq!(parsed["account_uuid"], "");
    }

    #[test]
    fn assistant_role_serialized() {
        let msgs = vec![ChatMessage::assistant(vec![ContentBlock::text("hi")])];
        let v = convert_messages(&msgs);
        assert_eq!(v[0]["role"], "assistant");
    }

    #[test]
    fn _role_marker() {
        let _ = Role::System;
    }
}
