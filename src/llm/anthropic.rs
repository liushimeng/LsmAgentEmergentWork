//! Anthropic Messages 协议客户端(支持 SSE 流式响应)
//!
//! - 请求: `POST {end_point}/v1/messages`,Body 带 `"stream": true`,Header `Accept: text/event-stream`。
//! - 解析: 通过 `SseStream` 拉字节流,`AnthropicParser` 把 `SseEvent` 翻译为
//!   `DeltaEvent`,最终由 `ParseSink` 聚合成 [`Completion`]。
//! - Header: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`
//! - 工具: `tools: [{ name, description, input_schema }]`
//! - 消息 content: `text` / `tool_use` / `tool_result`

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue, ACCEPT};
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::Protocol;
use crate::error::{AgentError, Result};
use crate::llm::sse::{DeltaEvent, ParseSink, SseStream};
use crate::llm::{
    build_common_headers, normalize_endpoint, ChatMessage, Completion, ContentBlock, LlmClient,
    RequestMeta, ToolDef,
};

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
    /// 启用 SSE 流式响应(本期始终 true)。
    stream: bool,
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

/// 协议 parser:把 Anthropic SSE 事件喂给 [`ParseSink`]。
struct AnthropicParser {
    /// 当前 content_block 的类型(text / tool_use / thinking)
    block_kind: Option<String>,
}

impl AnthropicParser {
    fn new() -> Self {
        Self { block_kind: None }
    }

    fn feed(&mut self, ev: &crate::llm::sse::SseEvent, sink: &mut ParseSink) -> Result<()> {
        let Some(event_name) = ev.event.as_deref() else {
            // 没有 event 字段(理论上 Anthropic 不会发生),按 ping 处理
            return Ok(());
        };
        let v: Value = match serde_json::from_str(&ev.data) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, data = %ev.data, "Anthropic SSE data 非 JSON,跳过");
                return Ok(());
            }
        };

        match event_name {
            "message_start" => {
                let usage = &v["message"]["usage"];
                sink.feed(DeltaEvent::InputUsage {
                    input_tokens: usage["input_tokens"].as_u64().unwrap_or(0) as u32,
                    cache_read: usage["cache_read_input_tokens"].as_u64().unwrap_or(0) as u32,
                    cache_creation: usage["cache_creation_input_tokens"].as_u64().unwrap_or(0) as u32,
                })?;
            }
            "content_block_start" => {
                let block = &v["content_block"];
                let kind = block["type"].as_str().unwrap_or("").to_string();
                self.block_kind = Some(kind.clone());
                if kind == "tool_use" {
                    let id = block["id"].as_str().unwrap_or("").to_string();
                    let name = block["name"].as_str().unwrap_or("").to_string();
                    sink.feed(DeltaEvent::ToolCallStart { id, name })?;
                }
            }
            "content_block_delta" => {
                let delta = &v["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(text) = delta["text"].as_str() {
                            if !text.is_empty() {
                                sink.feed(DeltaEvent::TextDelta(text.to_string()))?;
                            }
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(pj) = delta["partial_json"].as_str() {
                            sink.feed(DeltaEvent::ToolCallJsonDelta(pj.to_string()))?;
                        }
                    }
                    // thinking_delta / signature_delta 本期不向 TUI 输出
                    _ => {}
                }
            }
            "content_block_stop" => {
                if self.block_kind.as_deref() == Some("tool_use") {
                    sink.feed(DeltaEvent::ToolCallEnd)?;
                }
                self.block_kind = None;
            }
            "message_delta" => {
                if let Some(out) = v["usage"]["output_tokens"].as_u64() {
                    // message_delta.usage.output_tokens 是累计值,直接覆盖
                    sink.feed(DeltaEvent::OutputUsage {
                        output_tokens: out as u32,
                    })?;
                }
                if let Some(sr) = v["delta"]["stop_reason"].as_str() {
                    sink.feed(DeltaEvent::Stop {
                        stop_reason: Some(sr.to_string()),
                    })?;
                }
            }
            "message_stop" => {
                // 兜底终止(若 message_delta 没给 stop_reason 也能收尾)
                sink.feed(DeltaEvent::Stop {
                    stop_reason: None,
                })?;
            }
            "ping" => {}
            "error" => {
                let msg = v["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown upstream error")
                    .to_string();
                sink.feed(DeltaEvent::Error(msg))?;
            }
            other => {
                tracing::debug!(event = %other, "未识别的 Anthropic SSE 事件");
            }
        }
        Ok(())
    }
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
            stream: true,
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
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        let resp = self
            .http
            .post(&self.url)
            .headers(headers)
            .json(&req)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(AgentError::Llm(format!("HTTP {status}: {body_text}")));
        }

        let mut sse = SseStream::new();
        let mut parser = AnthropicParser::new();
        let mut sink = ParseSink::new();
        let mut resp = resp;
        loop {
            match resp.chunk().await {
                Ok(Some(bytes)) => {
                    let evs = sse.push(&bytes)?;
                    for ev in evs {
                        parser.feed(&ev, &mut sink)?;
                    }
                }
                Ok(None) => break, // EOF
                Err(e) => {
                    return Err(AgentError::Llm(format!("SSE 传输中断: {e}")));
                }
            }
        }
        if let Some(ev) = sse.finish()? {
            parser.feed(&ev, &mut sink)?;
        }
        sink.finish()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Anthropic
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;
    use crate::llm::sse::SseEvent;
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
        // 兼容旧测试:整段 JSON(非 SSE)路径已废弃,改用 parser 路径
        let body = json!({
            "type": "message",
            "content": [
                {"type": "text", "text": "hello "},
                {"type": "tool_use", "id": "id1", "name": "Read", "input": {"path": "/a"}},
                {"type": "text", "text": "world"},
            ],
            "usage": {"input_tokens": 10, "output_tokens": 20}
        });
        let mut sink = ParseSink::new();
        let ev = SseEvent {
            event: Some("message_start".into()),
            data: json!({"message":{"usage":{"input_tokens":10,"output_tokens":1}}}).to_string(),
            id: None,
            retry: None,
        };
        let mut p = AnthropicParser::new();
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("content_block_start".into()),
            data: json!({"index":0,"content_block":{"type":"text","text":""}}).to_string(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("content_block_delta".into()),
            data: json!({"index":0,"delta":{"type":"text_delta","text":"hello "}}).to_string(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("content_block_start".into()),
            data: json!({"index":1,"content_block":{"type":"tool_use","id":"id1","name":"Read","input":{}}}).to_string(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("content_block_delta".into()),
            data: json!({"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/a\"}"}}).to_string(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("content_block_stop".into()),
            data: json!({"index":1}).to_string(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("content_block_delta".into()),
            data: json!({"index":0,"delta":{"type":"text_delta","text":"world"}}).to_string(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("message_delta".into()),
            data: json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}}).to_string(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let ev = SseEvent {
            event: Some("message_stop".into()),
            data: "{}".into(),
            id: None,
            retry: None,
        };
        p.feed(&ev, &mut sink).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.text, "hello world");
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].name, "Read");
        assert_eq!(c.tool_calls[0].id, "id1");
        assert_eq!(c.tool_calls[0].arguments["path"], "/a");
        assert_eq!(c.usage.input_tokens, 10);
        assert_eq!(c.usage.output_tokens, 20);
        assert_eq!(c.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn parser_message_start_yields_input_usage() {
        let mut sink = ParseSink::new();
        let ev = SseEvent {
            event: Some("message_start".into()),
            data: json!({"message":{"usage":{"input_tokens":42,"cache_read_input_tokens":7}}}).to_string(),
            id: None,
            retry: None,
        };
        let mut p = AnthropicParser::new();
        p.feed(&ev, &mut sink).unwrap();
        assert_eq!(sink.usage().input_tokens, 42);
        assert_eq!(sink.usage().cache_read_input_tokens, 7);
    }

    #[test]
    fn parser_text_delta_concat() {
        let mut sink = ParseSink::new();
        let mut p = AnthropicParser::new();
        for s in ["hi ", "there"] {
            let ev = SseEvent {
                event: Some("content_block_delta".into()),
                data: json!({"index":0,"delta":{"type":"text_delta","text":s}}).to_string(),
                id: None,
                retry: None,
            };
            p.feed(&ev, &mut sink).unwrap();
        }
        p.feed(
            &SseEvent {
                event: Some("message_stop".into()),
                data: "{}".into(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.text, "hi there");
    }

    #[test]
    fn parser_input_json_delta_then_end_parses() {
        let mut sink = ParseSink::new();
        let mut p = AnthropicParser::new();
        p.feed(
            &SseEvent {
                event: Some("content_block_start".into()),
                data: json!({"index":0,"content_block":{"type":"tool_use","id":"t","name":"Bash","input":{}}}).to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        p.feed(
            &SseEvent {
                event: Some("content_block_delta".into()),
                data: json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}).to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        p.feed(
            &SseEvent {
                event: Some("content_block_delta".into()),
                data: json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}).to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        p.feed(
            &SseEvent {
                event: Some("content_block_stop".into()),
                data: json!({"index":0}).to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        p.feed(
            &SseEvent {
                event: Some("message_stop".into()),
                data: "{}".into(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].arguments["command"], "ls");
    }

    #[test]
    fn parser_message_delta_updates_output_tokens() {
        let mut sink = ParseSink::new();
        let mut p = AnthropicParser::new();
        p.feed(
            &SseEvent {
                event: Some("message_delta".into()),
                data: json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":123}}).to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.usage().output_tokens, 123);
    }

    #[test]
    fn parser_error_event_propagates() {
        let mut sink = ParseSink::new();
        let mut p = AnthropicParser::new();
        p.feed(
            &SseEvent {
                event: Some("error".into()),
                data: json!({"error":{"type":"overloaded_error","message":"overloaded"}}).to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        let err = sink.finish().unwrap_err();
        assert!(format!("{err}").contains("overloaded"));
    }

    #[test]
    fn request_skips_empty_system() {
        let req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 1,
            system: None,
            messages: vec![],
            tools: vec![],
            metadata: None,
            stream: true,
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
            stream: true,
        };
        let s = serde_json::to_string(&req).unwrap();
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
