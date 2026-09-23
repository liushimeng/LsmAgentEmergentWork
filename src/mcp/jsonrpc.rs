//! JSON-RPC 2.0 帧编解码(MCP 协议底座)。
//!
//! 三种帧:
//! - **请求** `{jsonrpc,id,method,params}`(id = 数字);
//! - **通知** `{jsonrpc,method,params}`(无 id,单向);
//! - **响应** `{jsonrpc,id,result}` 或 `{jsonrpc,id,error:{code,message,data}}`。
//!
//! stdio 传输用 **NDJSON 行帧**(一条消息一行,消息内不得含裸换行 —— MCP 规范;
//! 与 LSP 的 `Content-Length` 头帧不同,勿抄)。

use serde_json::{json, Value};

use super::{McpError, McpResult};

/// 解析出的入站帧(server → client)。
#[derive(Debug, Clone)]
pub enum Incoming {
    /// 响应帧:匹配某个请求 id。
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<RpcError>,
    },
    /// 通知帧(如 `notifications/tools/list_changed`),无 id。
    Notification { method: String, params: Value },
    /// 服务端主动请求(极少,如 sampling/elicitation)—— 本轮按「不支持」应答。
    ServerRequest { id: u64, method: String },
}

/// JSON-RPC 错误对象。
#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn to_mcp(&self) -> McpError {
        McpError::Call {
            message: format!("JSON-RPC error {}: {}", self.code, self.message),
            code: Some(self.code.to_string()),
        }
    }
}

/// 编码请求帧(单行 JSON)。
pub fn encode_request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// 编码通知帧(无 id)。
pub fn encode_notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// 编码对服务端请求的拒绝应答(能力不支持)。
pub fn encode_method_not_found(id: u64) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": -32601, "message": "Method not supported by laew MCP client"}
    })
    .to_string()
}

/// 解析一行 JSON-RPC 消息;非法返回 `None`(容错:坏行跳过不断连)。
pub fn parse_incoming(line: &str) -> Option<Incoming> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let id = v.get("id").and_then(Value::as_u64);
    let method = v.get("method").and_then(Value::as_str);
    match (id, method) {
        (Some(id), Some(method)) => Some(Incoming::ServerRequest {
            id,
            method: method.to_string(),
        }),
        (None, Some(method)) => Some(Incoming::Notification {
            method: method.to_string(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        }),
        (Some(id), None) => {
            let error = v.get("error").and_then(Value::as_object).map(|e| RpcError {
                code: e.get("code").and_then(Value::as_i64).unwrap_or(-1),
                message: e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("(无 message)")
                    .to_string(),
                data: e.get("data").cloned(),
            });
            Some(Incoming::Response {
                id,
                result: v.get("result").cloned(),
                error,
            })
        }
        (None, None) => None,
    }
}

/// 从 HTTP 响应体提取匹配 id 的 result;error 帧上抛 [`McpError`]。
///
/// body 可能是单个 JSON 响应帧,也可能是 SSE 文本(`data:` 行内 JSON)——
/// 两种形态都先剥出候选 JSON 帧再匹配 id。
pub fn extract_result(body: &str, want_id: u64) -> McpResult<Value> {
    let mut last_err: Option<McpError> = None;
    for frame in candidate_frames(body) {
        match parse_incoming(&frame) {
            Some(Incoming::Response {
                id,
                result,
                error,
            }) if id == want_id => {
                if let Some(e) = error {
                    return Err(e.to_mcp());
                }
                return Ok(result.unwrap_or(Value::Null));
            }
            Some(Incoming::Response { error: Some(e), .. }) => {
                // id 不匹配但带错误:记下,万一没有匹配帧时上抛。
                last_err = Some(e.to_mcp());
            }
            _ => {}
        }
    }
    Err(last_err.unwrap_or_else(|| {
        McpError::Protocol(format!(
            "响应中未找到 id={want_id} 的 JSON-RPC 帧(体长 {} 字符)",
            body.len()
        ))
    }))
}

/// 从响应体(HTTP JSON / SSE 混合容错)剥出候选 JSON 帧文本。
pub fn candidate_frames(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = body.trim();
    // 形态 1:整段就是 JSON。
    if trimmed.starts_with('{') {
        out.push(trimmed.to_string());
        return out;
    }
    // 形态 2:SSE —— 逐 `data:` 行取 JSON(`data:` 后可能跟一个空格)。
    for line in trimmed.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("data:") {
            let payload = rest.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            out.push(payload.to_string());
        }
    }
    // 形态 3:兜底 —— 行内嵌 JSON 对象(某些网关会包一层文本)。
    if out.is_empty() {
        for line in trimmed.lines() {
            let line = line.trim();
            if line.starts_with('{') && line.ends_with('}') {
                out.push(line.to_string());
            }
        }
    }
    out
}
