//! 通用 MCP(Model Context Protocol)客户端层(2026-09-23 第 123 轮)。
//!
//! 由 MCP 客户端连接外部 MCP server(stdio 子进程 / Streamable HTTP),完成
//! `initialize` 握手 → `tools/list`(分页折叠)→ `tools/call` → `resources/list` /
//! `resources/read` 最小闭环,把外部服务能力接入 laew 工具面(`MCP_Use`)。
//!
//! - 协议消息 = JSON-RPC 2.0 帧([`jsonrpc`]);
//! - 传输层双实现([`transport`]):stdio **NDJSON 行帧**(不是 LSP 的 Content-Length 帧)
//!   与 Streamable HTTP(`Accept` 双 MIME + `Mcp-Session-Id` / `MCP-Protocol-Version` echo
//!   + SSE 响应解析);
//! - 客户端([`client`])只做协议编排,不接触工具信封;连接生命周期([`manager`])负责
//!   懒连接 / 请求超时 / 指数退避重连(稳定性窗口)/ 冷却;
//! - 本层是**可选能力层**,封闭在 `src/mcp/`,不侵入 `agent_loop.rs` 核心循环。
//!
//! 设计见 `docs/MCP_Use/01-设计与解决方案.md`(唯一最新版)。
//! 调研基础:`docs/Agent源码调研/专题/专题-MCP架构深度分析.md`。

pub mod client;
pub mod jsonrpc;
pub mod manager;
pub mod transport;

#[cfg(test)]
mod tests;

use serde_json::Value;

/// MCP 协议版本(客户端 initialize 声明版;响应 negotiate 出什么后续就 echo 什么)。
pub const PROTOCOL_VERSION: &str = "2025-03-26";

/// 单次工具/资源结果投影文本硬上限(字符)。超出中间截断保头尾 ——
/// 防外部 server 返回大 payload 撑爆上下文(知识库风险清单)。
pub const MAX_RESULT_CHARS: usize = 24_000;

/// MCP 层错误(工具门面按 kind 映射 JSON 信封错误码 3001/5001-5006)。
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP server 未配置: {0}")]
    UnknownServer(String),

    #[error("连接失败: {0}")]
    Connect(String),

    #[error("握手失败: {0}")]
    Handshake(String),

    #[error("请求超时({0}ms)")]
    Timeout(u64),

    #[error("工具调用失败: {message}")]
    Call { message: String, code: Option<String> },

    #[error("资源读取失败: {0}")]
    Resource(String),

    #[error("环境缺失: {0}")]
    Environment(String),

    #[error("协议错误: {0}")]
    Protocol(String),

    #[error("server 冷却中,{retry_after_ms}ms 后可重试")]
    Cooldown { retry_after_ms: u64 },
}

pub type McpResult<T> = std::result::Result<T, McpError>;

/// 传输类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// 本地子进程,JSON-RPC over stdin/stdout(NDJSON 行帧)。
    Stdio,
    /// 远程 Streamable HTTP(POST JSON-RPC,响应 JSON 或 SSE)。
    Http,
}

impl TransportKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransportKind::Stdio => "stdio",
            TransportKind::Http => "http",
        }
    }

    pub fn parse(s: &str) -> McpResult<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stdio" => Ok(TransportKind::Stdio),
            "http" | "https" | "streamable_http" | "streamable-http" => Ok(TransportKind::Http),
            other => Err(McpError::Protocol(format!(
                "未知 transport: {other}(仅支持 stdio / http)"
            ))),
        }
    }
}

/// MCP server 配置(由 SQLite `mcp_servers` 行解密/规整而来,`laew mcp add` 维护)。
///
/// 安全边界:`command`/`args`/`url` 仅来自用户 CLI 显式配置,LLM 运行时不可新增 server。
#[derive(Debug, Clone, Default)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: TransportKindOrDefault,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub timeout_ms: u64,
}

/// 传输类型带 Default(供 `#[derive(Default)]` 的 config 使用)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TransportKindOrDefault {
    #[default]
    Stdio,
    Http,
}

impl From<TransportKindOrDefault> for TransportKind {
    fn from(v: TransportKindOrDefault) -> Self {
        match v {
            TransportKindOrDefault::Stdio => TransportKind::Stdio,
            TransportKindOrDefault::Http => TransportKind::Http,
        }
    }
}

impl McpServerConfig {
    /// 默认单请求超时(毫秒)。对齐知识库推荐 30s。
    pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

    /// 端点描述(仅用于 status 展示;http URL 不回显敏感 query)。
    pub fn endpoint_desc(&self) -> String {
        match TransportKind::from(self.transport) {
            TransportKind::Stdio => {
                let mut s = self.command.clone().unwrap_or_default();
                for a in &self.args {
                    s.push(' ');
                    s.push_str(a);
                }
                s
            }
            TransportKind::Http => self.url.clone().unwrap_or_default(),
        }
    }
}

/// 连接状态(懒连接状态机,设计 §6.1)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatus {
    /// 尚未尝试连接。
    Idle,
    /// 连接/握手中。
    Connecting,
    /// 就绪可调用。
    Connected,
    /// 传输层断开/失败(可重试)。
    Broken,
    /// 握手失败(需显式 connect 或改配置)。
    HandshakeFailed,
    /// 重连冷却中。
    Cooldown,
}

impl ServerStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServerStatus::Idle => "idle",
            ServerStatus::Connecting => "connecting",
            ServerStatus::Connected => "connected",
            ServerStatus::Broken => "broken",
            ServerStatus::HandshakeFailed => "handshake_failed",
            ServerStatus::Cooldown => "cooldown",
        }
    }
}

/// tools/call 投影结果。
#[derive(Debug, Clone, Default)]
pub struct CallToolOutcome {
    pub is_error: bool,
    pub content: Vec<ContentProjection>,
    /// 双信封:JSON-RPC error.code 或 isError 语义码(human + machine 可读并存)。
    pub error_code: Option<String>,
}

/// 资源读取投影条目。
#[derive(Debug, Clone, Default)]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: String,
}

/// 单条内容块投影(`text` 透传 / 其余降级为文本,**永不静默丢弃**)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentProjection {
    pub kind: &'static str,
    pub text: String,
}

/// tools/list 条目(容错解析:description 缺失当空串、annotations 可选、未知字段忽略)。
#[derive(Debug, Clone, Default)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// `annotations.readOnlyHint` 声明只读(server 自证,非强制)。
    pub read_only: bool,
}

/// resources/list 条目。
#[derive(Debug, Clone, Default)]
pub struct ResourceInfo {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

/// initialize 响应摘要(connect 信封用)。
#[derive(Debug, Clone, Default)]
pub struct ConnectInfo {
    pub server_info: Value,
    pub capabilities: Value,
    pub instructions: Option<String>,
    pub protocol_version: String,
}

/// 把 `tools/call` / `resources/read` 的 content 数组投影为文本块列表。
///
/// 处理矩阵(设计 §2.1):`text` 原样透传;`image` 降级 `[image …]` 标注;
/// `resource`/`resource_link`/`audio`/未知类型降级诊断文本 —— **永不静默丢弃**
/// (对齐知识库 `专题-MCP架构深度分析.md:954-965` 六工程收敛惯例)。
pub fn project_content_blocks(v: &Value) -> Vec<ContentProjection> {
    let mut out = Vec::new();
    let items = match v {
        Value::Array(arr) => arr.clone(),
        Value::Object(map) => map
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    for item in items {
        let typ = item.get("type").and_then(Value::as_str).unwrap_or("");
        match typ {
            "text" => {
                let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                out.push(ContentProjection {
                    kind: "text",
                    text: text.to_string(),
                });
            }
            "image" => {
                let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("?");
                let b64_len = item.get("data").and_then(Value::as_str).map(str::len).unwrap_or(0);
                out.push(ContentProjection {
                    kind: "image",
                    text: format!("[image mime={mime} base64_len={b64_len} 已省略(工具结果纯文本通道)]"),
                });
            }
            "resource" | "resource_link" => {
                let res = item.get("resource").unwrap_or(&Value::Null);
                let uri = res
                    .get("uri")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("uri").and_then(Value::as_str))
                    .unwrap_or("?");
                let mime = res
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                let text = res.get("text").and_then(Value::as_str).unwrap_or("");
                if text.is_empty() {
                    out.push(ContentProjection {
                        kind: "resource",
                        text: format!("[{typ} uri={uri} mime={mime}]"),
                    });
                } else {
                    out.push(ContentProjection {
                        kind: "resource",
                        text: format!("[{typ} uri={uri} mime={mime}] {text}"),
                    });
                }
            }
            "audio" => {
                let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("?");
                out.push(ContentProjection {
                    kind: "audio",
                    text: format!("[audio mime={mime} 已省略]"),
                });
            }
            other => {
                out.push(ContentProjection {
                    kind: "unsupported",
                    text: format!("[unsupported MCP content type: {other}]"),
                });
            }
        }
    }
    if out.is_empty() {
        out.push(ContentProjection {
            kind: "empty",
            text: "(空结果)".to_string(),
        });
    }
    // 总量截断:先拼接再判,超限中间截断保头尾。
    let joined_len: usize = out.iter().map(|p| p.text.chars().count()).sum();
    if joined_len > MAX_RESULT_CHARS {
        let joined: Vec<String> = out.iter().map(|p| p.text.clone()).collect();
        let merged = truncate_middle(&joined.join("\n"), MAX_RESULT_CHARS);
        out = vec![ContentProjection {
            kind: "text",
            text: merged,
        }];
    }
    out
}

/// 中间截断保头尾:`head …[省略 N 字符]… tail`。
pub fn truncate_middle(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(32) / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(total - keep).collect();
    format!("{head}\n…[省略 {} 字符]…\n{tail}", total - keep * 2)
}

/// 供工具门面把 [`McpError`] 投到 JSON 信封错误码(设计 §4.2)。
pub fn error_envelope_code(e: &McpError) -> i32 {
    match e {
        McpError::UnknownServer(_) => 5001,
        McpError::Connect(_) | McpError::Cooldown { .. } => 5002,
        McpError::Handshake(_) | McpError::Protocol(_) => 5003,
        McpError::Timeout(_) => 5004,
        McpError::Call { .. } => 5005,
        McpError::Resource(_) => 5006,
        McpError::Environment(_) => 3001,
    }
}
