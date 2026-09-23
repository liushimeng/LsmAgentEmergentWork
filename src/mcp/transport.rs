//! MCP 传输层:stdio(NDJSON 子进程)与 Streamable HTTP。
//!
//! - **stdio**:`tokio::process` 子进程,stdin/stdout 逐行 JSON-RPC(NDJSON),
//!   `kill_on_drop(true)` 防子进程泄漏;stderr 单独收集(4KB 环形)供失败归因;
//!   **单请求锁**串行化 req/rsp 配对(AtomCode 三锁的 laew 简化,见设计 §2.2)。
//! - **HTTP**:POST JSON-RPC,`Accept: application/json, text/event-stream` 双 MIME,
//!   `Mcp-Session-Id` / `MCP-Protocol-Version` echo(stateful server 约束);
//!   响应兼容单 JSON 帧与 SSE `data:` 流。
//!
//! HTTP 复用 `llm::build_http_client`(TLS 三级策略:IP 主机自动放宽自签证书,
//! `LAEW_TLS_INSECURE` 全局开关)。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

use super::jsonrpc::{self, Incoming};
use super::{McpError, McpResult, McpServerConfig, TransportKind};

/// 传输抽象:发一帧、收匹配响应;通知单向。
#[async_trait]
pub trait McpTransport: Send {
    /// 发送请求帧并等待 `want_id` 匹配的响应 result(JSON-RPC error 上抛)。
    async fn request(&mut self, frame: &str, want_id: u64) -> McpResult<serde_json::Value>;
    /// 发送通知帧(无需响应)。
    async fn notify(&mut self, frame: &str) -> McpResult<()>;
    /// 显式关闭(stdio 杀子进程 / http 清 session)。
    async fn close(&mut self);
    /// 传输描述(status 展示)。
    fn describe(&self) -> String;
    /// initialize 之后回填 negotiate 协议版本
    /// (HTTP 后续请求 echo `MCP-Protocol-Version`;stdio 无此约束,空实现)。
    fn set_protocol_version(&self, _version: &str) {}
}

/// 按配置构造传输层(stdio spawn 失败归 Environment 3001)。
pub async fn build_transport(cfg: &McpServerConfig) -> McpResult<Box<dyn McpTransport>> {
    match TransportKind::from(cfg.transport) {
        TransportKind::Stdio => {
            let command = cfg
                .command
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| McpError::Environment("stdio 配置缺少 command".into()))?;
            let t = StdioTransport::spawn(command, &cfg.args, &cfg.env, cfg.timeout_ms).await?;
            Ok(Box::new(t))
        }
        TransportKind::Http => {
            let url = cfg
                .url
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| McpError::Environment("http 配置缺少 url".into()))?;
            // SSRF 防护(D9-7 同套):私网/CGNAT 默认拦截,LAEW_ALLOW_PRIVATE_ENDPOINT=1 放行。
            crate::agent::safety::url_safety::is_safe_endpoint(url).map_err(|e| {
                McpError::Environment(format!("url 安全校验未通过: {e}"))
            })?;
            Ok(Box::new(HttpTransport::new(url, &cfg.headers)))
        }
    }
}

// ===========================================================================
// stdio 传输(NDJSON 行帧)
// ===========================================================================

pub struct StdioTransport {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: BufReader<tokio::process::ChildStdout>,
    stderr_buf: Arc<Mutex<String>>,
    timeout_ms: u64,
}

impl StdioTransport {
    async fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        timeout_ms: u64,
    ) -> McpResult<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // 显式 env 注入(叠加在宿主 env 上)—— 用户在 `laew mcp add --env` 里点名的变量。
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                McpError::Environment(format!("命令不存在: {command}(请安装后重试)"))
            } else {
                McpError::Connect(format!("启动 {command} 失败: {e}"))
            }
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::Connect("子进程 stdin 未建立".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::Connect("子进程 stdout 未建立".into())
        })?;
        // stderr 后台收集(4KB 截断),防管道写满阻塞子进程。
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let buf = stderr_buf.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let mut g = buf.lock().await;
                            if g.len() < 4096 {
                                g.push_str(&line);
                            }
                        }
                    }
                }
            });
        }
        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: BufReader::new(stdout),
            stderr_buf,
            timeout_ms,
        })
    }

    async fn write_line(&self, frame: &str) -> McpResult<()> {
        let mut g = self.stdin.lock().await;
        g.write_all(frame.as_bytes())
            .await
            .map_err(|e| McpError::Connect(format!("写 stdin 失败: {e}")))?;
        g.write_all(b"\n")
            .await
            .map_err(|e| McpError::Connect(format!("写 stdin 换行失败: {e}")))?;
        g.flush()
            .await
            .map_err(|e| McpError::Connect(format!("刷 stdin 失败: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&mut self, frame: &str, want_id: u64) -> McpResult<serde_json::Value> {
        self.write_line(frame).await?;
        let timeout = Duration::from_millis(self.timeout_ms.max(1));
        let read = async {
            let mut line = String::new();
            loop {
                line.clear();
                let n = self
                    .stdout
                    .read_line(&mut line)
                    .await
                    .map_err(|e| McpError::Connect(format!("读 stdout 失败: {e}")))?;
                if n == 0 {
                    let stderr = self.stderr_buf.lock().await.clone();
                    return Err(McpError::Connect(format!(
                        "子进程 stdout EOF(可能已退出);stderr: {}",
                        stderr.trim()
                    )));
                }
                match jsonrpc::parse_incoming(&line) {
                    Some(Incoming::Response { id, result, error }) if id == want_id => {
                        return match error {
                            Some(e) => Err(e.to_mcp()),
                            None => Ok(result.unwrap_or(serde_json::Value::Null)),
                        };
                    }
                    Some(Incoming::ServerRequest { id, .. }) => {
                        // 服务端反向请求:应答「不支持」,避免对方悬挂等待。
                        self.write_line(&jsonrpc::encode_method_not_found(id)).await?;
                    }
                    // 通知(如 tools/list_changed)与无关响应:跳过。
                    _ => {}
                }
            }
        };
        tokio::time::timeout(timeout, read)
            .await
            .map_err(|_| McpError::Timeout(self.timeout_ms))?
    }

    async fn notify(&mut self, frame: &str) -> McpResult<()> {
        self.write_line(frame).await
    }

    async fn close(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    fn describe(&self) -> String {
        "stdio".to_string()
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // 双保险:close() 之外的路径(超时销毁/进程退出)也不留孤儿进程。
        let _ = self.child.start_kill();
    }
}

// ===========================================================================
// Streamable HTTP 传输
// ===========================================================================

pub struct HttpTransport {
    http: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    /// stateful server 的会话 id(initialize 响应头 Mcp-Session-Id),后续每请求 echo。
    session_id: Mutex<Option<String>>,
    /// negotiate 出的协议版本(initialize 响应体),后续每请求 echo `MCP-Protocol-Version`。
    negotiated_version: Mutex<Option<String>>,
    timeout_ms: u64,
}

/// `Accept` 双 MIME(Streamable HTTP 硬约束,对齐 AtomCode `MCP_HTTP_ACCEPT`)。
const MCP_HTTP_ACCEPT: &str = "application/json, text/event-stream";

impl HttpTransport {
    pub fn new(url: &str, headers: &[(String, String)]) -> Self {
        Self {
            http: crate::llm::build_http_client(url),
            url: url.trim_end_matches('/').to_string(),
            headers: headers.to_vec(),
            session_id: Mutex::new(None),
            negotiated_version: Mutex::new(None),
            timeout_ms: 0, // 由 request 层按 config/args 覆盖;0 = 用连接级默认
        }
    }

    fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    async fn post_frame(&self, frame: &str) -> McpResult<String> {
        let mut req = self
            .http
            .post(&self.url)
            .header("Accept", MCP_HTTP_ACCEPT)
            .header("Content-Type", "application/json")
            .body(frame.to_string());
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(sid) = self.session_id.lock().await.as_deref() {
            req = req.header("Mcp-Session-Id", sid);
        }
        if let Some(ver) = self.negotiated_version.lock().await.as_deref() {
            req = req.header("MCP-Protocol-Version", ver);
        }
        let timeout = if self.timeout_ms > 0 {
            Duration::from_millis(self.timeout_ms)
        } else {
            Duration::from_millis(super::McpServerConfig::DEFAULT_TIMEOUT_MS)
        };
        let resp = tokio::time::timeout(timeout, req.send())
            .await
            .map_err(|_| McpError::Timeout(self.timeout_ms))?
            .map_err(|e| McpError::Connect(format!("HTTP 请求失败: {e}")))?;

        // stateful server:保存会话 id(仅 initialize 响应头会给)。
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            let mut g = self.session_id.lock().await;
            if g.is_none() {
                *g = Some(sid.to_string());
            }
        }

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| McpError::Connect(format!("读响应体失败: {e}")))?;

        // session 过期双信号:HTTP 404 + JSON-RPC -32001(知识库 L1060 形态)→ 归 Connect 触发重连。
        if status.as_u16() == 404 {
            return Err(McpError::Connect(format!(
                "HTTP 404(session 过期或端点不存在): {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        if !status.is_success() {
            return Err(McpError::Connect(format!(
                "HTTP {status}: {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        Ok(body)
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn request(&mut self, frame: &str, want_id: u64) -> McpResult<serde_json::Value> {
        let body = self.post_frame(frame).await?;
        jsonrpc::extract_result(&body, want_id)
    }

    async fn notify(&mut self, frame: &str) -> McpResult<()> {
        // 通知期待 2xx(无响应体语义);失败不影响主链路(尽力而为)。
        match self.post_frame(frame).await {
            Ok(_) => Ok(()),
            Err(McpError::Protocol(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn close(&mut self) {
        *self.session_id.lock().await = None;
    }

    fn describe(&self) -> String {
        format!("http {}", self.url)
    }

    /// initialize 之后由 client 回填协商版本(后续请求 echo `MCP-Protocol-Version`)。
    fn set_protocol_version(&self, version: &str) {
        if let Ok(mut g) = self.negotiated_version.try_lock() {
            *g = Some(version.to_string());
        }
    }
}

/// 给 `build_transport` 外的调用方(测试)直接构造 HTTP 传输并设超时。
#[allow(dead_code)]
pub fn http_transport_for_test(url: &str, headers: &[(String, String)], timeout_ms: u64) -> HttpTransport {
    HttpTransport::new(url, headers).with_timeout(timeout_ms)
}
