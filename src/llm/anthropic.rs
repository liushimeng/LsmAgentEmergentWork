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
    apply_cache_policy, build_common_headers, normalize_endpoint, ChatMessage, Completion,
    ContentBlock, LlmClient, RequestMeta, ToolDef, DEFAULT_CACHE_POLICY,
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
            http: crate::llm::build_http_client(end_point),
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
    /// Anthropic 系统提示词,作为文本块数组(允许携带 `cache_control` 断点)。
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<Value>>,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    /// forced tool_choice(L6/L19,2026-09-09 第 13 轮):
    /// `{"type":"tool","name":X,"disable_parallel_tool_use":true}`。
    /// `None` 时不发送该字段(= Anthropic 默认 auto)。
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    /// Anthropic 会话/设备标识(metadata.user_id)。
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Metadata>,
    /// 启用 SSE 流式响应(本期始终 true)。
    stream: bool,
}

#[derive(Serialize)]
struct Metadata {
    /// JSON 字符串,形如: {"device_id":"...","account_uuid":"","session_id":"...","agent":"..."}
    user_id: String,
}

/// 构造 metadata.user_id 的 JSON 字符串。
///
/// `agent` 为发起请求的 Agent 名称(取 User-Agent 首段,2026-09-09 第 08 轮),
/// 使请求体自身也可辨识角色;对上游是 opaque 字符串,不影响真实 API。
fn build_user_id(device_id: &str, session_id: &str, agent_name: &str) -> String {
    json!({
        "device_id": device_id,
        "account_uuid": "",
        "session_id": session_id,
        "agent": agent_name,
    })
    .to_string()
}

fn convert_messages(messages: &[ChatMessage]) -> Vec<Value> {
    let raw: Vec<Value> = messages
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
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
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
        .collect();
    merge_adjacent_same_role(raw)
}

/// 出站前合并相邻同角色消息(2026-09-10 第 28 轮 B09/B10 修复)。
///
/// laew 的上下文构造会产生连续多条 role=user —— 典型序列:
/// `[user SESSION_HISTORY] [user PROJECT_CONTEXT] [user 提示词]`。
/// 官方 api.anthropic.com 与宽容网关可接受;但**严格 Anthropic 协议实现会
/// 400 拒绝连续同角色消息**(LsmAgentGame 工程 CLAUDE.md §14.1 记录的 DouBao
/// 严格代理同类事故,其修复即出站前合并)。laew 支持任意 endpoint 接入,
/// 在 wire 层统一合并:相邻同角色消息拼接 content blocks 为一条。
/// `apply_cache_policy` 在本函数之后运行,cache_control 仍落最后一条 user
/// 消息的末 text block,语义不受影响。
fn merge_adjacent_same_role(entries: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(last) = out.last_mut() {
            if last["role"] == entry["role"] {
                let mut blocks = last["content"].as_array().cloned().unwrap_or_default();
                blocks.extend(entry["content"].as_array().cloned().unwrap_or_default());
                last["content"] = Value::Array(blocks);
                continue;
            }
        }
        out.push(entry);
    }
    out
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

/// 把字符串 system 切分为单个文本块数组(Anthropic 同时支持字符串与数组形态,
/// 数组形态才能携带 `cache_control` 断点 — L1047)。
///
/// 空字符串返回空数组(`AnthropicRequest.system: None` 时整字段被 skip_serializing)。
/// 非空则返回 `[{ "type": "text", "text": <原文> }]`,`apply_cache_policy`
/// 在最后一个(也是唯一)文本块上打 cache_control。
fn convert_system_blocks(system: &str) -> Vec<Value> {
    if system.trim().is_empty() {
        Vec::new()
    } else {
        vec![json!({ "type": "text", "text": system })]
    }
}

#[cfg(test)]
mod cache_policy_tests {
    //! 验证 `convert_system_blocks` 与 `apply_cache_policy` 在 Anthropic 协议层的端到端集成。

    use super::*;
    use crate::llm::{ChatMessage, ToolDef};
    use serde_json::json;

    #[test]
    fn convert_system_blocks_empty_returns_empty() {
        assert!(convert_system_blocks("").is_empty());
        assert!(convert_system_blocks("   ").is_empty());
        assert!(convert_system_blocks("\n\t").is_empty());
    }

    #[test]
    fn convert_system_blocks_single_text_block() {
        let blocks = convert_system_blocks("you are a helpful assistant");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "you are a helpful assistant");
        // 未应用 cache policy 时不带 cache_control
        assert!(blocks[0].get("cache_control").is_none());
    }

    #[test]
    fn convert_system_blocks_plus_apply_marks_last_block() {
        let blocks = convert_system_blocks("s");
        let (out_sys, _, _, bp) = apply_cache_policy(
            DEFAULT_CACHE_POLICY,
            blocks,
            vec![json!({"name":"T","description":"d","input_schema":{}})],
            vec![json!({"role":"user","content":[{"type":"text","text":"hi"}]})],
        );
        assert_eq!(out_sys.len(), 1);
        assert!(out_sys[0].get("cache_control").is_some());
        assert_eq!(bp.dropped, 0);
    }

    #[test]
    fn anthropic_request_system_field_serializes_as_array() {
        // 直接验证序列化形态
        #[derive(serde::Serialize)]
        struct Probe {
            system: Option<Vec<Value>>,
        }
        let probe = Probe {
            system: Some(vec![json!({
                "type":"text",
                "text":"hello",
                "cache_control": {"type":"ephemeral"}
            })]),
        };
        let s = serde_json::to_string(&probe).unwrap();
        assert!(s.contains("\"type\":\"text\""));
        assert!(s.contains("\"cache_control\":{\"type\":\"ephemeral\"}"));
    }

    #[test]
    fn end_to_end_request_body_has_cache_control_on_three_breakpoints() {
        // 模拟一次完整 complete() 路径的请求体构造
        let system = "you are x";
        let tools = vec![
            ToolDef::new("Read", "read file", json!({"type":"object"})),
            ToolDef::new("Write", "write file", json!({"type":"object"})),
        ];
        let messages = vec![
            ChatMessage::user("first"),
            ChatMessage::assistant(vec![ContentBlock::text("ok")]),
            ChatMessage::user("second"),
        ];

        let sys_blocks = convert_system_blocks(system);
        let tool_blocks = convert_tools(&tools);
        let msg_blocks = convert_messages(&messages);
        let (sys_blocks, tool_blocks, msg_blocks, _) = apply_cache_policy(
            DEFAULT_CACHE_POLICY,
            sys_blocks,
            tool_blocks,
            msg_blocks,
        );

        // 1) last tool
        assert!(tool_blocks.last().unwrap().get("cache_control").is_some());
        // 2) last system
        assert!(sys_blocks.last().unwrap().get("cache_control").is_some());
        // 3) latest user message text block
        let latest_user = msg_blocks.iter().rfind(|m| m["role"] == "user").unwrap();
        let content = latest_user["content"].as_array().unwrap();
        assert!(content.last().unwrap().get("cache_control").is_some());
    }
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
                    cache_creation: usage["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                        as u32,
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
                // 兼容网关形态差异:真实 Anthropic 在 message_start 给全量 usage,
                // 部分兼容网关(如 LongCat)message_start 给空 usage、
                // 到 message_delta 才带上 input_tokens/cache_* —— 此处补解析,
                // 缺失字段回填 sink 当前值,避免抹掉 message_start 已给的信息。
                let usage = &v["usage"];
                let has_input = usage["input_tokens"].is_u64()
                    || usage["cache_read_input_tokens"].is_u64()
                    || usage["cache_creation_input_tokens"].is_u64();
                if has_input {
                    let cur = sink.usage();
                    sink.feed(DeltaEvent::InputUsage {
                        input_tokens: usage["input_tokens"]
                            .as_u64()
                            .map(|v| v as u32)
                            .unwrap_or(cur.input_tokens),
                        cache_read: usage["cache_read_input_tokens"]
                            .as_u64()
                            .map(|v| v as u32)
                            .unwrap_or(cur.cache_read_input_tokens),
                        cache_creation: usage["cache_creation_input_tokens"]
                            .as_u64()
                            .map(|v| v as u32)
                            .unwrap_or(cur.cache_creation_input_tokens),
                    })?;
                }
                if let Some(out) = usage["output_tokens"].as_u64() {
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
                sink.feed(DeltaEvent::Stop { stop_reason: None })?;
            }
            "ping" => {}
            "error" => {
                let kind = v["error"]["type"].as_str().unwrap_or("unknown").to_string();
                let msg = v["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown upstream error")
                    .to_string();
                sink.feed(DeltaEvent::Error { kind, message: msg })?;
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
        // Agent 身份逐请求注入(2026-09-09 第 08 轮):meta 注入的 UA 优先,
        // 空则回退构造期默认 —— 同一客户端上 8 角色请求在抓包层面各自可辨识。
        let user_agent = meta.resolve_user_agent(&self.user_agent);
        let agent_name = user_agent.split('/').next().unwrap_or_default();
        // Prompt Caching(第十六轮 L1047):Anthropic 路径自动注入 cache_control 断点。
        // OpenAI 协议忽略(走隐式 prefix caching),故无需在此处判断协议。
        // 三元组顺序:tools → system → messages,与 opencode cache-policy.ts 一致。
        let sys_blocks = convert_system_blocks(system);
        let tool_blocks = convert_tools(tools);
        let msg_blocks = convert_messages(messages);
        let (sys_blocks, tool_blocks, msg_blocks, _bp) = apply_cache_policy(
            DEFAULT_CACHE_POLICY,
            sys_blocks,
            tool_blocks,
            msg_blocks,
        );

        let req = AnthropicRequest {
            model: self.model.clone(),
            // max_tokens 静默升级(2026-09-09 第 09 轮,实现 L1037):
            // 会话级 MaxTokensState 在 LLM 输出被截断时翻倍,默认 8K → 16K → 32K → 64K 上限。
            // 协议层仅负责读取 meta 注入的 override,不参与升级逻辑。
            max_tokens: meta.max_tokens_override.unwrap_or(DEFAULT_MAX_TOKENS),
            system: if sys_blocks.is_empty() { None } else { Some(sys_blocks) },
            messages: msg_blocks,
            tools: tool_blocks,
            // 结构化输出强制通道(L6/L19):forced 指名调用 + 禁并行,
            // 引导模型先 Read 后单独 submit(分轮),避免混合并行调用被忽略。
            tool_choice: meta.forced_tool.as_ref().map(|name| {
                json!({ "type": "tool", "name": name, "disable_parallel_tool_use": true })
            }),
            metadata: Some(Metadata {
                user_id: build_user_id(&meta.device_id, &meta.session_id, agent_name),
            }),
            stream: true,
        };

        // 通用头:Content-Type / User-Agent / Authorization / X-Session-Id
        let mut headers = build_common_headers(&self.api_key, meta, user_agent)?;
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

        // 响应头(TTFB)超时兜底:网关挂起不回包头时转可重试错误,而非无限等待
        // (此前 connect_timeout 只管握手,流内 idle/总超时只在拿到 Response 后生效)
        let resp = tokio::time::timeout(
            crate::llm::resilient::RESPONSE_HEADERS_TIMEOUT,
            self.http
                .post(&self.url)
                .headers(headers)
                .json(&req)
                .send(),
        )
        .await
        .map_err(|_| {
            AgentError::LlmNetwork(format!(
                "等待响应头超时(>{:?}):服务端未返回 HTTP 头,疑似网关挂起",
                crate::llm::resilient::RESPONSE_HEADERS_TIMEOUT
            ))
        })??;

        let status = resp.status();
        if !status.is_success() {
            // 结构化错误:保留状态码与 Retry-After,供弹性层(resilient.rs)分类重试
            let retry_after_ms = crate::llm::parse_retry_after(resp.headers());
            let body_text = resp.text().await.unwrap_or_default();
            return Err(AgentError::LlmHttp {
                status: status.as_u16(),
                retry_after_ms,
                message: format!("HTTP {status}: {body_text}"),
            });
        }

        // 流式读取 + 两级超时(idle 90s / 总 600s),超时错误可被弹性层自动重试
        let mut sse = SseStream::new();
        let mut parser = AnthropicParser::new();
        let mut sink = ParseSink::new();
        {
            let mut on_chunk = |bytes: &[u8]| -> Result<()> {
                for ev in sse.push(bytes)? {
                    parser.feed(&ev, &mut sink)?;
                }
                Ok(())
            };
            crate::llm::sse::stream_chunks(
                resp,
                crate::llm::resilient::STREAM_IDLE_TIMEOUT,
                crate::llm::resilient::TOTAL_ATTEMPT_TIMEOUT,
                &mut on_chunk,
            )
            .await?;
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
    use crate::llm::sse::SseEvent;
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
            data: json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}})
                .to_string(),
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
            data: json!({"message":{"usage":{"input_tokens":42,"cache_read_input_tokens":7}}})
                .to_string(),
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
                data:
                    json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}})
                        .to_string(),
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
                data: json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":123}})
                    .to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.usage().output_tokens, 123);
    }

    #[test]
    fn parser_message_delta_backfills_input_usage() {
        // 网关形态兼容(2026-09-09 第 15 轮):部分兼容网关(LongCat 等)
        // message_start 给空 usage,input_tokens 到 message_delta 才出现。
        let mut sink = ParseSink::new();
        let mut p = AnthropicParser::new();
        p.feed(
            &SseEvent {
                event: Some("message_start".into()),
                data: json!({"message":{"usage":{}}}).to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.usage().input_tokens, 0);
        p.feed(
            &SseEvent {
                event: Some("message_delta".into()),
                data: json!({"delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":8,"output_tokens":64}})
                    .to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.usage().input_tokens, 8);
        assert_eq!(sink.usage().output_tokens, 64);
    }

    #[test]
    fn parser_message_delta_preserves_message_start_cache_fields() {
        // message_delta 只带 input/output 时,不得抹掉 message_start 已给的 cache 字段。
        let mut sink = ParseSink::new();
        let mut p = AnthropicParser::new();
        p.feed(
            &SseEvent {
                event: Some("message_start".into()),
                data: json!({"message":{"usage":{"input_tokens":42,"cache_read_input_tokens":7}}})
                    .to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        p.feed(
            &SseEvent {
                event: Some("message_delta".into()),
                data: json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":20}})
                    .to_string(),
                id: None,
                retry: None,
            },
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.usage().input_tokens, 42);
        assert_eq!(sink.usage().cache_read_input_tokens, 7);
        assert_eq!(sink.usage().output_tokens, 20);
    }

    #[test]
    fn parser_error_event_propagates() {
        let mut sink = ParseSink::new();
        let mut p = AnthropicParser::new();
        p.feed(
            &SseEvent {
                event: Some("error".into()),
                data: json!({"error":{"type":"overloaded_error","message":"overloaded"}})
                    .to_string(),
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
            tool_choice: None,
            metadata: None,
            stream: true,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(!s.contains("system"));
    }

    // ========== 结构化输出强制通道 wire(L6/L19,2026-09-09 第 13 轮) ==========

    #[test]
    fn request_serializes_forced_tool_choice() {
        let req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 1,
            system: None,
            messages: vec![],
            tools: vec![],
            tool_choice: Some(json!({
                "type": "tool",
                "name": "submit_task_classification",
                "disable_parallel_tool_use": true
            })),
            metadata: None,
            stream: true,
        };
        let s = serde_json::to_string(&req).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["tool_choice"]["type"], "tool", "forced wire 应为 tool 指名形态");
        assert_eq!(v["tool_choice"]["name"], "submit_task_classification");
        assert_eq!(
            v["tool_choice"]["disable_parallel_tool_use"], true,
            "forced wire 应禁并行(引导先 Read 后单独 submit 分轮)"
        );
        // None 时不发送 tool_choice 字段(= 默认 auto)
        let req2 = AnthropicRequest {
            model: "m".into(),
            max_tokens: 1,
            system: None,
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: None,
            stream: true,
        };
        let s2 = serde_json::to_string(&req2).unwrap();
        assert!(!s2.contains("tool_choice"));
    }

    #[test]
    fn request_metadata_user_id_is_json_string() {
        let req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 1,
            system: None,
            messages: vec![],
            tools: vec![],
            tool_choice: None,
            metadata: Some(Metadata {
                user_id: build_user_id("dev1234567890", "sess-1", "LsmAgentEmergentWork-Yolo"),
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
        // 第 08 轮:agent 字段使请求体自身可辨识发起角色
        assert_eq!(parsed["agent"], "LsmAgentEmergentWork-Yolo");
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

    // ===== 第 28 轮 B09/B10:出站前合并相邻同角色消息(wire 兼容严格 Anthropic 网关) =====

    #[test]
    fn consecutive_user_messages_merged_into_one() {
        // laew 典型上下文:SESSION_HISTORY + PROJECT_CONTEXT + 用户提示词
        // 三条连续 user —— 严格网关会 400,必须在 wire 层合并为一条。
        let msgs = vec![
            ChatMessage::user("<<<LAEW:SESSION_HISTORY>>> 摘要"),
            ChatMessage::user("<<<LAEW:PROJECT_CONTEXT>>> 背景"),
            ChatMessage::user("香农信息熵是什么?"),
        ];
        let v = convert_messages(&msgs);
        assert_eq!(v.len(), 1, "三条连续 user 必须合并为一条: {v:?}");
        assert_eq!(v[0]["role"], "user");
        let blocks = v[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["text"], "<<<LAEW:SESSION_HISTORY>>> 摘要");
        assert_eq!(blocks[2]["text"], "香农信息熵是什么?");
    }

    #[test]
    fn user_then_tool_result_merged_as_user_blocks() {
        // Role::Tool 映射为 user + tool_result 块;紧邻 user 文本时同样合并
        // (Anthropic 允许同一 user 消息内 text 与 tool_result 块共存)。
        let msgs = vec![
            ChatMessage::user("跑个命令"),
            ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t9".into(),
                name: "Bash".into(),
                input: json!({"command": "ls"}),
            }]),
            ChatMessage::tool_result("t9", "ok", false),
            ChatMessage::user("结果很好,继续"),
        ];
        let v = convert_messages(&msgs);
        // 期望:user(文本) → assistant(tool_use) → user(tool_result+text 合并)
        assert_eq!(v.len(), 3, "got: {v:?}");
        assert_eq!(v[2]["role"], "user");
        let blocks = v[2]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[1]["type"], "text");
    }

    #[test]
    fn alternating_roles_untouched() {
        // 正常交替序列不受合并影响(逐条保持原样)。
        let msgs = vec![
            ChatMessage::user("q1"),
            ChatMessage::assistant(vec![ContentBlock::text("a1")]),
            ChatMessage::user("q2"),
        ];
        let v = convert_messages(&msgs);
        assert_eq!(v.len(), 3);
        assert_eq!(
            v.iter().map(|m| m["role"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["user", "assistant", "user"]
        );
    }
}
