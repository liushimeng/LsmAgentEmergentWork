//! OpenAI Chat Completions 协议客户端(支持 SSE 流式响应,也适配兼容该协议的各类网关)。
//!
//! - 请求: `POST {end_point}/chat/completions`,Body 带 `"stream": true` 与
//!   `"stream_options": {"include_usage": true}`,Header `Accept: text/event-stream`。
//! - 解析: 通过 `SseStream` 拉字节流,`OpenAiParser` 把 `SseEvent` 翻译为
//!   `DeltaEvent`,最终由 `ParseSink` 聚合成 [`Completion`]。
//! - Header: `Authorization: Bearer <api_key>`, `Content-Type: application/json`
//! - 工具: `tools: [{ type: "function", function: { name, description, parameters } }]`
//! - 助手消息的 `tool_calls` / 工具消息 `role: "tool", tool_call_id`

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::header::{ACCEPT, HeaderValue};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{AgentError, Result};
use crate::llm::sse::{DeltaEvent, ParseSink, SseEvent, SseStream};
use crate::llm::{
    build_common_headers, normalize_endpoint, ChatMessage, Completion, ContentBlock, LlmClient,
    RequestMeta, Role, ToolDef,
};

pub struct OpenAiClient {
    http: reqwest::Client,
    url: String,
    api_key: String,
    model: String,
    user_agent: String,
}

impl OpenAiClient {
    pub fn new(end_point: &str, api_key: &str, model: &str, user_agent: &str) -> Result<Self> {
        let url = format!("{}/chat/completions", normalize_endpoint(end_point));
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
struct OpenAiRequest<'a> {
    model: &'a str,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    /// 启用 SSE 流式响应。
    stream: bool,
    /// 让上游在尾部单条 chunk 中输出 usage。
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn convert_messages(system: &str, messages: &[ChatMessage]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();

    if !system.trim().is_empty() {
        out.push(json!({ "role": "system", "content": system }));
    }

    for m in messages {
        match m.role {
            Role::System => {
                // 已在外层处理,跳过
            }
            Role::User => {
                let content = extract_text(&m.content);
                out.push(json!({ "role": "user", "content": content }));
            }
            Role::Assistant => {
                let mut obj = serde_json::Map::new();
                obj.insert("role".into(), Value::String("assistant".into()));
                let mut text_buf = String::new();
                let mut tool_calls_arr: Vec<Value> = Vec::new();
                for b in &m.content {
                    match b {
                        ContentBlock::Text { text } => text_buf.push_str(text),
                        ContentBlock::ToolUse { id, name, input } => {
                            let args_str = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
                            tool_calls_arr.push(json!({
                                "id": id,
                                "type": "function",
                                "function": { "name": name, "arguments": args_str }
                            }));
                        }
                        ContentBlock::ToolResult { .. } => { /* 不应出现在 assistant 消息中 */ }
                    }
                }
                if !text_buf.is_empty() {
                    obj.insert("content".into(), Value::String(text_buf));
                } else if tool_calls_arr.is_empty() {
                    obj.insert("content".into(), Value::String(String::new()));
                } else {
                    obj.insert("content".into(), Value::Null);
                }
                if !tool_calls_arr.is_empty() {
                    obj.insert("tool_calls".into(), Value::Array(tool_calls_arr));
                }
                out.push(Value::Object(obj));
            }
            Role::Tool => {
                // 一条 tool 消息一个 tool_use_id;若存在多条则拆为多条
                for b in &m.content {
                    if let ContentBlock::ToolResult { tool_use_id, content, is_error: _ } = b {
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": content,
                        }));
                    }
                }
            }
        }
    }
    out
}

fn extract_text(blocks: &[ContentBlock]) -> String {
    let mut s = String::new();
    for b in blocks {
        if let ContentBlock::Text { text } = b {
            s.push_str(text);
        }
    }
    s
}

fn convert_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect()
}

/// OpenAI 协议 parser:逐个 SseEvent → DeltaEvent → sink。
///
/// 内部以 `chunk.tool_calls[].index` 为 key 缓存 in_flight tool_calls,
/// 流结束时 (`[DONE]` 或 finish_reason 已收到且无新数据) 把它们一次性 flush 给 sink。
struct OpenAiParser {
    in_flight: HashMap<u32, InFlightToolCall>,
    /// 维持 chunk index 出现顺序
    order: Vec<u32>,
    finish_reason: Option<String>,
    /// 是否已 emit 终止信号
    stopped: bool,
}

#[derive(Debug, Clone, Default)]
struct InFlightToolCall {
    id: String,
    name: String,
    json_buf: String,
}

impl OpenAiParser {
    fn new() -> Self {
        Self {
            in_flight: HashMap::new(),
            order: Vec::new(),
            finish_reason: None,
            stopped: false,
        }
    }

    fn feed(&mut self, ev: &SseEvent, sink: &mut ParseSink) -> Result<()> {
        let data = ev.data.trim();
        // [DONE] 哨兵
        if data == "[DONE]" {
            self.flush_into(sink)?;
            if !self.stopped {
                sink.feed(DeltaEvent::Stop {
                    stop_reason: self.finish_reason.take(),
                })?;
                self.stopped = true;
            }
            return Ok(());
        }
        if data.is_empty() {
            return Ok(());
        }

        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, data = %data, "OpenAI SSE data 非 JSON,跳过");
                return Ok(());
            }
        };

        // 尾部 usage chunk(choices 可能为空,usage 非 null)
        if v.get("usage").map(|u| !u.is_null()).unwrap_or(false) {
            let u = &v["usage"];
            sink.feed(DeltaEvent::InputUsage {
                input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                cache_read: u["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0) as u32,
                cache_creation: 0,
            })?;
            if let Some(out) = u["completion_tokens"].as_u64() {
                sink.feed(DeltaEvent::OutputUsage {
                    output_tokens: out as u32,
                })?;
            }
        }

        if let Some(choices) = v["choices"].as_array() {
            for c in choices {
                let delta = &c["delta"];
                if let Some(text) = delta["content"].as_str() {
                    if !text.is_empty() {
                        sink.feed(DeltaEvent::TextDelta(text.to_string()))?;
                    }
                }
                if let Some(tcs) = delta["tool_calls"].as_array() {
                    for tc in tcs {
                        let idx = tc["index"].as_u64().unwrap_or(0) as u32;
                        let entry = self.in_flight.entry(idx).or_insert_with(|| {
                            if !self.order.contains(&idx) {
                                self.order.push(idx);
                            }
                            InFlightToolCall::default()
                        });
                        if let Some(id) = tc["id"].as_str() {
                            entry.id = id.to_string();
                        }
                        if let Some(name) = tc["function"]["name"].as_str() {
                            entry.name = name.to_string();
                        }
                        if let Some(args) = tc["function"]["arguments"].as_str() {
                            entry.json_buf.push_str(args);
                        }
                    }
                }
                if let Some(fr) = c["finish_reason"].as_str() {
                    self.finish_reason = Some(fr.to_string());
                    if !self.stopped {
                        // OpenAI 通常 finish_reason 与 usage 都在尾部;若没有 usage chunk,
                        // 这里先 stop,后续 sse.finish() 时再补 in_flight。
                        sink.feed(DeltaEvent::Stop {
                            stop_reason: self.finish_reason.clone(),
                        })?;
                        self.stopped = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// 把 in_flight 累积的 tool_calls 按 order 顺序喂给 sink。
    fn flush_into(&mut self, sink: &mut ParseSink) -> Result<()> {
        let order = std::mem::take(&mut self.order);
        let mut map = std::mem::take(&mut self.in_flight);
        for idx in order {
            if let Some(call) = map.remove(&idx) {
                // 允许 id/name 在某些 chunk 缺失(只在首个 chunk 出现),缺失则用 idx 占位
                let id = if call.id.is_empty() {
                    format!("call_{idx}")
                } else {
                    call.id.clone()
                };
                sink.feed(DeltaEvent::ToolCallStart {
                    id,
                    name: call.name.clone(),
                })?;
                if !call.json_buf.is_empty() {
                    sink.feed(DeltaEvent::ToolCallJsonDelta(call.json_buf.clone()))?;
                }
                sink.feed(DeltaEvent::ToolCallEnd)?;
            }
        }
        Ok(())
    }

    /// 流结束时调用:把仍未 flush 的 in_flight 也强制灌入 sink。
    fn finish_into(mut self, sink: &mut ParseSink) -> Result<()> {
        if !self.in_flight.is_empty() {
            self.flush_into(sink)?;
        }
        Ok(())
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion> {
        let converted = convert_messages(system, messages);
        let req = OpenAiRequest {
            model: &self.model,
            messages: converted,
            tools: convert_tools(tools),
            tool_choice: Some("auto"),
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
        };

        // 通用头:Content-Type / User-Agent / Authorization / X-Session-Id
        let mut headers = build_common_headers(&self.api_key, meta, &self.user_agent)?;
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
        let mut parser = OpenAiParser::new();
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
                Ok(None) => break,
                Err(e) => return Err(AgentError::Llm(format!("SSE 传输中断: {e}"))),
            }
        }
        if let Some(ev) = sse.finish()? {
            parser.feed(&ev, &mut sink)?;
        }
        parser.finish_into(&mut sink)?;
        sink.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_appends_chat_completions() {
        let c = OpenAiClient::new("https://api.openai.com/v1", "k", "m", "ua").unwrap();
        assert_eq!(c.url, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn url_without_v1_still_works() {
        let c = OpenAiClient::new("https://example.com", "k", "m", "ua").unwrap();
        assert_eq!(c.url, "https://example.com/chat/completions");
    }

    #[test]
    fn system_message_promoted_to_top() {
        let msgs = vec![ChatMessage::user("hi")];
        let v = convert_messages("you are x", &msgs);
        assert_eq!(v[0]["role"], "system");
        assert_eq!(v[0]["content"], "you are x");
        assert_eq!(v[1]["role"], "user");
    }

    #[test]
    fn assistant_with_tool_calls_format() {
        let msgs = vec![ChatMessage::assistant(vec![
            ContentBlock::text("ok "),
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "Read".into(),
                input: json!({"file_path": "/a"}),
            },
        ])];
        let v = convert_messages("", &msgs);
        let m = &v[0];
        assert_eq!(m["role"], "assistant");
        assert_eq!(m["content"], "ok ");
        assert_eq!(m["tool_calls"][0]["id"], "a");
        assert_eq!(m["tool_calls"][0]["type"], "function");
        assert_eq!(m["tool_calls"][0]["function"]["name"], "Read");
    }

    #[test]
    fn tool_results_become_tool_messages() {
        let msgs = vec![ChatMessage::tool_result("t1", "out", false)];
        let v = convert_messages("", &msgs);
        assert_eq!(v[0]["role"], "tool");
        assert_eq!(v[0]["tool_call_id"], "t1");
        assert_eq!(v[0]["content"], "out");
    }

    #[test]
    fn tools_use_function_wrapper() {
        let tools = vec![ToolDef::new("Bash", "run", json!({"type":"object"}))];
        let v = convert_tools(&tools);
        assert_eq!(v[0]["type"], "function");
        assert_eq!(v[0]["function"]["name"], "Bash");
        assert_eq!(v[0]["function"]["parameters"]["type"], "object");
    }

    fn ev_with_data(data: &str) -> SseEvent {
        SseEvent {
            event: None,
            data: data.into(),
            id: None,
            retry: None,
        }
    }

    #[test]
    fn parser_text_chunks_concat() {
        let mut sink = ParseSink::new();
        let mut p = OpenAiParser::new();
        let ev1 = ev_with_data(&json!({"choices":[{"delta":{"content":"Hello "},"finish_reason":null}]}).to_string());
        p.feed(&ev1, &mut sink).unwrap();
        let ev2 = ev_with_data(&json!({"choices":[{"delta":{"content":"world"},"finish_reason":null}]}).to_string());
        p.feed(&ev2, &mut sink).unwrap();
        let ev3 = ev_with_data(&json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string());
        p.feed(&ev3, &mut sink).unwrap();
        p.finish_into(&mut sink).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.text, "Hello world");
        assert_eq!(c.stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parser_tool_calls_assemble_arguments_by_index() {
        let mut sink = ParseSink::new();
        let mut p = OpenAiParser::new();
        // 第一个 tool_call 的 start chunk
        let ev1 = ev_with_data(
            &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"Bash","arguments":""}}]}}]}).to_string(),
        );
        p.feed(&ev1, &mut sink).unwrap();
        // arguments 增量
        let ev2 = ev_with_data(
            &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]}}]}).to_string(),
        );
        p.feed(&ev2, &mut sink).unwrap();
        let ev3 = ev_with_data(
            &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}).to_string(),
        );
        p.feed(&ev3, &mut sink).unwrap();
        // 收尾
        let ev4 = ev_with_data(&json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}).to_string());
        p.feed(&ev4, &mut sink).unwrap();
        p.finish_into(&mut sink).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].name, "Bash");
        assert_eq!(c.tool_calls[0].id, "c1");
        assert_eq!(c.tool_calls[0].arguments["command"], "ls");
    }

    #[test]
    fn parser_done_sentinel_emits_stop() {
        let mut sink = ParseSink::new();
        let mut p = OpenAiParser::new();
        let ev1 = ev_with_data(&json!({"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}).to_string());
        p.feed(&ev1, &mut sink).unwrap();
        let ev2 = ev_with_data(&json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string());
        p.feed(&ev2, &mut sink).unwrap();
        let ev3 = ev_with_data("[DONE]");
        p.feed(&ev3, &mut sink).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.text, "hi");
        assert_eq!(c.stop_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parser_usage_chunk_overrides_output_tokens() {
        let mut sink = ParseSink::new();
        let mut p = OpenAiParser::new();
        let ev_text = ev_with_data(&json!({"choices":[{"delta":{"content":"x"}}]}).to_string());
        p.feed(&ev_text, &mut sink).unwrap();
        let ev_usage = ev_with_data(&json!({"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":99,"total_tokens":106,"prompt_tokens_details":{"cached_tokens":3}}}).to_string());
        p.feed(&ev_usage, &mut sink).unwrap();
        let ev_done = ev_with_data("[DONE]");
        p.feed(&ev_done, &mut sink).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.usage.input_tokens, 7);
        assert_eq!(c.usage.output_tokens, 99);
        assert_eq!(c.usage.cache_read_input_tokens, 3);
    }

    #[test]
    fn parser_bad_data_skips_without_failing() {
        let mut sink = ParseSink::new();
        let mut p = OpenAiParser::new();
        let bad = ev_with_data("not json at all");
        p.feed(&bad, &mut sink).unwrap();
        // 正常 chunk
        let ev = ev_with_data(&json!({"choices":[{"delta":{"content":"ok"},"finish_reason":null}]}).to_string());
        p.feed(&ev, &mut sink).unwrap();
        let ev_done = ev_with_data(&json!({"choices":[{"delta":{},"finish_reason":"stop"}]}).to_string());
        p.feed(&ev_done, &mut sink).unwrap();
        p.finish_into(&mut sink).unwrap();
        let c = sink.finish().unwrap();
        assert_eq!(c.text, "ok");
    }

    #[test]
    fn request_serializes_stream_options() {
        let req = OpenAiRequest {
            model: "m",
            messages: vec![json!({"role":"user","content":"hi"})],
            tools: vec![],
            tool_choice: Some("auto"),
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"stream\":true"));
        assert!(s.contains("\"include_usage\":true"));
        assert!(s.contains("\"tool_choice\":\"auto\""));
    }
}
