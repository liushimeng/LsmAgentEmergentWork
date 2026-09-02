//! OpenAI Chat Completions 协议客户端(也适配兼容该协议的各类网关)。
//!
//! - 请求: `POST {end_point}/chat/completions`
//! - Header: `Authorization: Bearer <api_key>`, `Content-Type: application/json`
//! - 工具: `tools: [{ type: "function", function: { name, description, parameters } }]`
//! - 助手消息的 `tool_calls` / 工具消息 `role: "tool", tool_call_id`

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{AgentError, Result};
use crate::llm::{build_common_headers, normalize_endpoint, ChatMessage, Completion, ContentBlock, LlmClient, RequestMeta, Role, ToolCallReq, ToolDef};

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
        };

        // 通用头:Content-Type / User-Agent / Authorization / X-Session-Id
        let headers = build_common_headers(&self.api_key, meta, &self.user_agent)?;

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
    let choices = body
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| AgentError::Llm("OpenAI 响应缺少 choices[]".into()))?;
    let msg = choices
        .first()
        .and_then(|c| c.get("message"))
        .ok_or_else(|| AgentError::Llm("OpenAI 响应缺少 message".into()))?;

    let mut text = String::new();
    if let Some(s) = msg.get("content").and_then(Value::as_str) {
        text.push_str(s);
    }
    let mut tool_calls: Vec<ToolCallReq> = Vec::new();
    if let Some(arr) = msg.get("tool_calls").and_then(Value::as_array) {
        for c in arr {
            let id = c.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let name = c
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args_str = c
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let arguments: Value = serde_json::from_str(args_str).unwrap_or_else(|_| {
                // 模型偶发返回非严格 JSON,这里兜底,原文作为 _raw 字段
                json!({ "_raw": args_str })
            });
            tool_calls.push(ToolCallReq { id, name, arguments });
        }
    }
    Ok(Completion { text, tool_calls })
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

    #[test]
    fn parse_response_with_tool_calls() {
        let body = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "x",
                        "type": "function",
                        "function": {"name": "Bash", "arguments": "{\"command\":\"ls\"}"}
                    }]
                }
            }]
        });
        let c = parse_response(&body).unwrap();
        assert!(c.text.is_empty());
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].name, "Bash");
        assert_eq!(c.tool_calls[0].arguments["command"], "ls");
    }

    #[test]
    fn parse_response_handles_bad_arguments_json() {
        let body = json!({
            "choices": [{
                "message": {
                    "content": "hello",
                    "tool_calls": [{
                        "id": "x",
                        "type": "function",
                        "function": {"name": "Bash", "arguments": "not-json"}
                    }]
                }
            }]
        });
        let c = parse_response(&body).unwrap();
        assert_eq!(c.text, "hello");
        assert_eq!(c.tool_calls[0].arguments["_raw"], "not-json");
    }

    #[test]
    fn request_serializes_tool_choice_auto() {
        let req = OpenAiRequest {
            model: "m",
            messages: vec![json!({"role":"user","content":"hi"})],
            tools: vec![],
            tool_choice: Some("auto"),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"tool_choice\":\"auto\""));
    }
}
