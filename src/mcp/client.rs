//! MCP 客户端:握手 / 工具发现 / 工具调用 / 资源读取(协议编排)。
//!
//! 只做 JSON-RPC 方法编排与结果解析,不接触工具信封(那是 `MCP_Use` 门面的事);
//! 传输细节见 [`super::transport`]。

use std::time::Instant;

use serde_json::{json, Value};

use super::jsonrpc::encode_notification;
use super::transport::{build_transport, McpTransport};
use super::{
    project_content_blocks, CallToolOutcome, ConnectInfo, McpError, McpResult, McpServerConfig,
    ResourceContent, ResourceInfo, ToolInfo, PROTOCOL_VERSION,
};

pub struct McpClient {
    transport: Box<dyn McpTransport>,
    next_id: u64,
    pub connect_info: ConnectInfo,
    /// 连接建立时刻(退避稳定性窗口判定用)。
    pub connected_at: Instant,
    timeout_ms: u64,
}

impl McpClient {
    /// 建立传输 + `initialize` 握手 + `notifications/initialized`。
    pub async fn connect(cfg: &McpServerConfig) -> McpResult<Self> {
        let transport = build_transport(cfg).await?;
        let timeout_ms = if cfg.timeout_ms > 0 {
            cfg.timeout_ms
        } else {
            McpServerConfig::DEFAULT_TIMEOUT_MS
        };
        let mut client = Self {
            transport,
            next_id: 0,
            connect_info: ConnectInfo::default(),
            connected_at: Instant::now(),
            timeout_ms,
        };

        // initialize 握手。
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "laew", "version": env!("CARGO_PKG_VERSION")},
        });
        let result = client.rpc("initialize", params, timeout_ms).await?;

        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION)
            .to_string();
        client.connect_info = ConnectInfo {
            server_info: result.get("serverInfo").cloned().unwrap_or(Value::Null),
            capabilities: result.get("capabilities").cloned().unwrap_or(Value::Null),
            instructions: result
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_string),
            protocol_version: protocol_version.clone(),
        };
        // negotiate 版本回填(HTTP 后续请求 echo MCP-Protocol-Version;stdio 空实现)。
        client.transport.set_protocol_version(&protocol_version);
        client
            .transport
            .notify(&encode_notification(
                "notifications/initialized",
                json!({}),
            ))
            .await?;
        client.connected_at = Instant::now();
        Ok(client)
    }

    /// 发一次 JSON-RPC 请求(自增 id + 超时包裹在 transport 内)。
    async fn rpc(&mut self, method: &str, params: Value, timeout_ms: u64) -> McpResult<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let frame = super::jsonrpc::encode_request(id, method, params);
        // 超时销毁语义由 manager 层处理(半开连接不复用);这里只包超时。
        let fut = self.transport.request(&frame, id);
        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms.max(1)), fut).await {
            Ok(inner) => inner,
            Err(_) => Err(McpError::Timeout(timeout_ms)),
        }
    }

    /// tools/list,**自动折叠 nextCursor 分页**(模型不接触 cursor,设计 §2.1)。
    pub async fn list_tools(&mut self) -> McpResult<Vec<ToolInfo>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self.rpc("tools/list", params, self.timeout_ms).await?;
            let tools = result
                .get("tools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for t in tools {
                out.push(parse_tool_info(&t));
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
            if out.len() > 500 {
                // 防御:server 恶意/异常翻页不止。
                return Err(McpError::Protocol(
                    "tools/list 分页超过 500 条,疑似 nextCursor 循环".into(),
                ));
            }
        }
        Ok(out)
    }

    /// tools/call(参数原样转交;content 投影 + isError 归一)。
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout_ms: Option<u64>,
    ) -> McpResult<CallToolOutcome> {
        let timeout = timeout_ms.unwrap_or(self.timeout_ms).max(1);
        let result = self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}), timeout)
            .await?;
        let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let content = project_content_blocks(&result);
        Ok(CallToolOutcome {
            is_error,
            content,
            error_code: None,
        })
    }

    /// resources/list(折叠分页)。
    pub async fn list_resources(&mut self) -> McpResult<Vec<ResourceInfo>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self.rpc("resources/list", params, self.timeout_ms).await?;
            let items = result
                .get("resources")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for r in items {
                out.push(ResourceInfo {
                    uri: r.get("uri").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: r.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    description: r
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    mime_type: r
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if cursor.is_none() || out.len() > 500 {
                break;
            }
        }
        Ok(out)
    }

    /// resources/read(blob 降级文本标注)。
    pub async fn read_resource(&mut self, uri: &str) -> McpResult<Vec<ResourceContent>> {
        let result = self
            .rpc("resources/read", json!({"uri": uri}), self.timeout_ms)
            .await?;
        let items = result
            .get("contents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if items.is_empty() {
            return Err(McpError::Resource(format!("uri 无内容: {uri}")));
        }
        let mut out = Vec::new();
        for c in items {
            let text = match (c.get("text").and_then(Value::as_str), c.get("blob")) {
                (Some(t), _) => t.to_string(),
                (None, Some(b)) => {
                    let n = b.as_str().map(str::len).unwrap_or(0);
                    format!("[blob base64_len={n} 已省略(纯文本通道)]")
                }
                (None, None) => "(空内容)".to_string(),
            };
            out.push(ResourceContent {
                uri: c
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or(uri)
                    .to_string(),
                mime_type: c
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                text: super::truncate_middle(&text, super::MAX_RESULT_CHARS),
            });
        }
        Ok(out)
    }

    pub async fn close(&mut self) {
        self.transport.close().await;
    }
}

/// tools/list 条目容错解析(description 缺失当空串、annotations 可选、未知字段忽略;
/// 对齐 OpenCode `TolerantListToolsResultSchema` 精神)。
pub fn parse_tool_info(t: &Value) -> ToolInfo {
    let read_only = t
        .get("annotations")
        .and_then(|a| a.get("readOnlyHint"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ToolInfo {
        name: t.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
        description: t
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        input_schema: t
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"})),
        read_only,
    }
}
