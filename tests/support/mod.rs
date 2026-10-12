//! Rust 集成测试公共夹具:进程内 mock LLM 服务 + 断言助手。
//!
//! 取代原 `testReport/run_e2e.sh` 依赖的 `scripts/mock_llm_server.py`:
//! 同一份逻辑改为 Rust 实现,随 `cargo test` 编译,无需 Python 运行时、
//! 无需手工挑选端口(每个 mock 独占 OS 分配的随机端口),抓包直接留在内存里
//! (`MockLlm::requests()`),断言失败时随测试输出打印。
//!
//! 只做「最小可用 HTTP/1.1 服务端」:读请求行 + 头 + `Content-Length` 主体,
//! 回一个 `Connection: close` 的完整响应。不实现 keep-alive / chunked 请求体
//! —— laew 的两个协议客户端都发定长 JSON body。
//!
//! 用法见 `tests/protocol_wire.rs`。

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use lsm_agent::database::models::{Protocol, ProviderRecord};
use lsm_agent::llm::{client_from_record, LlmClient};

// ---------------------------------------------------------------------------
// 抓包模型
// ---------------------------------------------------------------------------

/// 一次被 mock 收到的 HTTP 请求(抓包记录)。
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    /// 请求路径,含补全后的后缀(如 `/v1/messages`)。
    pub path: String,
    /// 头字段名统一小写。
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

impl CapturedRequest {
    /// 取头部(大小写不敏感,键已小写)。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(|s| s.as_str())
    }

    /// `body.messages` 数组(两种协议都有该字段)。
    pub fn messages(&self) -> &[Value] {
        self.body["messages"].as_array().map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// `body.tools` 数组。
    pub fn tools(&self) -> &[Value] {
        self.body["tools"].as_array().map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// `body.system`,Anthropic 是 blocks 数组、OpenAI 是字符串,统一取拼接文本。
    pub fn system_text(&self) -> String {
        match &self.body["system"] {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }

    /// 把全部消息正文压成一个大字符串,便于做「某段提示词是否上 wire」断言。
    pub fn message_corpus(&self) -> String {
        self.messages()
            .iter()
            .flat_map(|m| {
                let content = m["content"].clone();
                match content {
                    Value::String(s) => vec![s],
                    Value::Array(parts) => parts
                        .iter()
                        .map(|p| match p["type"].as_str() {
                            Some("text") => p["text"].as_str().unwrap_or_default().to_string(),
                            Some("tool_result") => {
                                let c = &p["content"];
                                match c {
                                    Value::String(s) => s.clone(),
                                    Value::Array(inner) => inner
                                        .iter()
                                        .filter_map(|x| x["text"].as_str())
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                    other => other.to_string(),
                                }
                            }
                            Some("tool_use") => p["input"].to_string(),
                            _ => String::new(),
                        })
                        .collect::<Vec<_>>(),
                    other => vec![other.to_string()],
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 本次请求注册的工具名列表(兼容两种协议的 schema 形状)。
    pub fn tool_names(&self) -> Vec<String> {
        self.tools()
            .iter()
            .map(|t| match t {
                // Anthropic: {"name": ..., "input_schema": ...}
                Value::Object(_) if t.get("name").is_some() => {
                    t["name"].as_str().unwrap_or_default().to_string()
                }
                // OpenAI: {"type":"function","function":{"name":...}}
                other => other["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            })
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// 强制 tool_choice 的工具名(取不到返回 `None`)。
    pub fn forced_tool(&self) -> Option<String> {
        let tc = &self.body["tool_choice"];
        if let Some(s) = tc["name"].as_str() {
            return Some(s.to_string());
        }
        tc["function"]["name"].as_str().map(|s| s.to_string())
    }
}

// ---------------------------------------------------------------------------
// 响应脚本
// ---------------------------------------------------------------------------

/// mock 对一次请求的响应指令。
#[derive(Debug, Clone)]
pub enum Reply {
    /// 回一段已构造好的 SSE 帧(协议由构造方决定,见 [`MockLlm::text`] 等)。
    Sse(String),
    /// 直接回一段裸 body(用于构造错误码 / 畸形响应等异常场景)。
    Raw { status: u16, body: String },
    /// 延迟若干毫秒再响应(用于取消传播、并发类用例)。
    DelayMs(u64, Box<Reply>),
}

/// 根据「已收到的请求」+「这是第几次请求」决定回什么。
pub type Responder = Arc<dyn Fn(&CapturedRequest, usize) -> Reply + Send + Sync>;

// ---------------------------------------------------------------------------
// MockLlm
// ---------------------------------------------------------------------------

/// 进程内 mock LLM 服务。
///
/// - 监听 OS 分配的随机端口(`127.0.0.1:0`),**天然消除端口复用冲突**——
///   原 e2e 脚本 18901~18913 固定端口靠串行 + `kill;wait` 才没炸。
/// - 每次请求都记进内存,测试用 [`MockLlm::requests`] 取抓包。
/// - `Drop` 时 abort 后台任务,不留游离线程。
pub struct MockLlm {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    responder: Responder,
    task: tokio::task::JoinHandle<()>,
}

impl MockLlm {
    /// 启动一个固定回复的 mock(每个请求都回同样的东西)。
    ///
    /// 协议由 [`Reply::Sse`] 的内容决定 —— 用 [`MockLlm::text`] /
    /// [`MockLlm::tool_use`] 构造,别手拼协议。
    pub async fn fixed(reply: Reply) -> Self {
        let r = reply.clone();
        Self::routing(move |_req, _n| r.clone()).await
    }

    /// 固定回一段纯文本(按协议选对 SSE 形态)。
    pub async fn text(protocol: Protocol, s: impl Into<String>) -> Self {
        let s = s.into();
        let body = match protocol {
            Protocol::Anthropic => anthropic_text_sse(&s),
            Protocol::OpenAi => openai_text_sse(&s),
        };
        Self::fixed(Reply::Sse(body)).await
    }

    /// 固定回一次工具调用(按协议选对 SSE 形态)。
    pub async fn tool_use(protocol: Protocol, name: &str, args: Value) -> Self {
        let body = match protocol {
            Protocol::Anthropic => anthropic_tool_use_sse(name, &args),
            Protocol::OpenAi => openai_tool_use_sse(name, &args),
        };
        Self::fixed(Reply::Sse(body)).await
    }

    /// 启动一个按请求内容 / 序号动态应答的 mock。
    ///
    /// 这是主力入口:编排链路类用例需要「第 1 轮返工具调用、第 2 轮返最终文本」
    /// 这类多轮脚本。
    pub async fn routing<F>(responder: F) -> Self
    where
        F: Fn(&CapturedRequest, usize) -> Reply + Send + Sync + 'static,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock LLM 绑定随机端口失败");
        let addr = listener.local_addr().expect("取 mock 端口失败");
        let requests: Arc<Mutex<Vec<CapturedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(AtomicUsize::new(0));
        let responder: Responder = Arc::new(responder);

        let task = {
            let requests = Arc::clone(&requests);
            let seen = Arc::clone(&seen);
            let responder = Arc::clone(&responder);
            tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        break;
                    };
                    let requests = Arc::clone(&requests);
                    let seen = Arc::clone(&seen);
                    let responder = Arc::clone(&responder);
                    // 每次连接一个短生命周期任务:允许并发请求(编排并行用例要靠它)。
                    tokio::spawn(async move {
                        let Some(req) = read_request(&mut sock).await else {
                            return;
                        };
                        let n = seen.fetch_add(1, Ordering::SeqCst);
                        requests.lock().unwrap().push(req.clone());
                        let reply = responder(&req, n);
                        let _ = write_reply(&mut sock, reply).await;
                    });
                }
            })
        };

        Self { addr, requests, responder, task }
    }

    /// mock 的 base URL(不含 `/v1/messages` 后缀,交由客户端补全)。
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// 取全部抓包(按接收顺序)。
    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// 请求总数。
    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// 判据式取值:是否存在满足 `pred` 的请求。
    pub fn has_request(&self, pred: impl Fn(&CapturedRequest) -> bool) -> bool {
        self.requests().iter().any(pred)
    }

    /// 等待至少出现 `n` 次请求(轮询式,避免固定 sleep 抖动)。
    /// 返回是否在 5s 内达成。
    pub async fn wait_for_requests(&self, n: usize) -> bool {
        for _ in 0..500 {
            if self.request_count() >= n {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        false
    }

    /// 响应脚本(测试里偶尔需要在启动后改行为时保留引用)。
    pub fn responder(&self) -> &Responder {
        &self.responder
    }
}

impl Drop for MockLlm {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ---------------------------------------------------------------------------
// HTTP 读写
// ---------------------------------------------------------------------------

/// 读一个完整的 HTTP/1.1 请求(头 + Content-Length 主体)。
async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<CapturedRequest> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];

    // 1) 读到完整头部(以 \r\n\r\n 收尾)。
    let head_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let start = lines.next()?;
    let mut sp = start.split_whitespace();
    let method = sp.next().unwrap_or_default().to_string();
    let path = sp.next().unwrap_or_default().to_string();

    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    // 2) 按 Content-Length 补齐主体。
    let want: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < want {
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }

    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    // 让服务端读到 EOF 时不必额外处理:回包走 Connection: close。
    let _ = sock.flush().await;
    Some(CapturedRequest { method, path, headers, body })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 把 [`Reply`] 落成 HTTP 响应字节。
async fn write_reply(sock: &mut tokio::net::TcpStream, reply: Reply) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    // 用循环摊平 DelayMs,避免 async fn 递归导致的无限大 future(E0733)。
    let mut reply = reply;
    let (status, body) = loop {
        match reply {
            Reply::Sse(body) => break (200, body),
            Reply::Raw { status, body } => break (status, body),
            Reply::DelayMs(ms, inner) => {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                reply = *inner;
            }
        }
    };

    let head = format!(
        "HTTP/1.1 {status} OK\r\n\
         Content-Type: text/event-stream\r\n\
         Cache-Control: no-cache\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body.as_bytes()).await?;
    sock.flush().await
}

// ---------------------------------------------------------------------------
// SSE 构造(两种协议)
// ---------------------------------------------------------------------------

fn sse_frame(event: Option<&str>, data: &Value) -> String {
    let mut out = String::new();
    if let Some(e) = event {
        out.push_str(&format!("event: {e}\n"));
    }
    out.push_str(&format!("data: {data}\n\n"));
    out
}

/// Anthropic 纯文本回复的 SSE 帧序列(与 wire 规范一致)。
pub fn anthropic_text_sse(text: &str) -> String {
    let mut out = String::new();
    out.push_str(&sse_frame(
        Some("message_start"),
        &json!({"type":"message_start","message":{"id":"mock-msg","type":"message","role":"assistant",
            "content":[],"model":"mock-anthropic","stop_reason":null,
            "usage":{"input_tokens":30,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
    ));
    out.push_str(&sse_frame(
        Some("content_block_start"),
        &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
    ));
    out.push_str(&sse_frame(
        Some("content_block_delta"),
        &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
    ));
    out.push_str(&sse_frame(Some("content_block_stop"), &json!({"type":"content_block_stop","index":0})));
    out.push_str(&sse_frame(
        Some("message_delta"),
        &json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},
            "usage":{"output_tokens":20}}),
    ));
    out.push_str(&sse_frame(Some("message_stop"), &json!({"type":"message_stop"})));
    out
}

/// Anthropic 工具调用回复的 SSE 帧序列。
pub fn anthropic_tool_use_sse(name: &str, args: &Value) -> String {
    let args_json = args.to_string();
    let mut out = String::new();
    out.push_str(&sse_frame(
        Some("message_start"),
        &json!({"type":"message_start","message":{"id":"mock-msg-tool","type":"message","role":"assistant",
            "content":[],"model":"mock-anthropic","stop_reason":null,
            "usage":{"input_tokens":30,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
    ));
    out.push_str(&sse_frame(
        Some("content_block_start"),
        &json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"mock-tool-1","name":name,"input":{}}}),
    ));
    out.push_str(&sse_frame(
        Some("content_block_delta"),
        &json!({"type":"content_block_delta","index":0,
            "delta":{"type":"input_json_delta","partial_json":args_json}}),
    ));
    out.push_str(&sse_frame(Some("content_block_stop"), &json!({"type":"content_block_stop","index":0})));
    out.push_str(&sse_frame(
        Some("message_delta"),
        &json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},
            "usage":{"output_tokens":20}}),
    ));
    out.push_str(&sse_frame(Some("message_stop"), &json!({"type":"message_stop"})));
    out
}

/// OpenAI 纯文本回复的 SSE 帧序列。
pub fn openai_text_sse(text: &str) -> String {
    let mut out = String::new();
    for delta in [
        json!({"role":"assistant","content":""}),
        json!({"content":text}),
        json!({}),
    ] {
        out.push_str(&sse_frame(
            None,
            &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,
                "model":"mock-openai","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
        ));
    }
    out.push_str(&sse_frame(
        None,
        &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,
            "model":"mock-openai","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
    ));
    out.push_str(&sse_frame(
        None,
        &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock-openai",
            "choices":[],"usage":{"prompt_tokens":30,"completion_tokens":20,"total_tokens":50,
            "prompt_tokens_details":{"cached_tokens":0}}}),
    ));
    out
}

/// OpenAI 工具调用回复的 SSE 帧序列。
pub fn openai_tool_use_sse(name: &str, args: &Value) -> String {
    let args_json = args.to_string();
    let mut out = String::new();
    out.push_str(&sse_frame(
        None,
        &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock-openai",
            "choices":[{"index":0,"delta":{"role":"assistant","content":null},"finish_reason":null}]}),
    ));
    out.push_str(&sse_frame(
        None,
        &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock-openai",
            "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"mock-call-1","type":"function",
            "function":{"name":name,"arguments":""}}]},"finish_reason":null}]}),
    ));
    out.push_str(&sse_frame(
        None,
        &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock-openai",
            "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,
            "function":{"arguments":args_json}}]},"finish_reason":null}]}),
    ));
    out.push_str(&sse_frame(
        None,
        &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock-openai",
            "choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
    ));
    out.push_str(&sse_frame(
        None,
        &json!({"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1,"model":"mock-openai",
            "choices":[],"usage":{"prompt_tokens":30,"completion_tokens":20,"total_tokens":50,
            "prompt_tokens_details":{"cached_tokens":0}}}),
    ));
    out
}

// ---------------------------------------------------------------------------
// Provider / 客户端构造
// ---------------------------------------------------------------------------

/// 造一条指向本 mock 的 provider 记录。
///
/// `allow_private_endpoint: true` 让 loopback mock 绕过 SSRF 拦截 —— 比原 e2e 靠
/// 全局 `LAEW_ALLOW_PRIVATE_ENDPOINT=1` 一刀切更干净(不影响其它 provider)。
pub fn mock_record(protocol: Protocol, base_url: &str, model: &str) -> ProviderRecord {
    ProviderRecord {
        id: 1,
        protocol,
        provider_name: "mock".into(),
        model_name: model.into(),
        end_point: base_url.into(),
        api_key: "mock-key".into(),
        is_active: true,
        created_at: "1970-01-01T00:00:00Z".into(),
        context_max_size: 800_000,
        allow_private_endpoint: true,
    }
}

/// 由 provider 记录造真实客户端(自带弹性层),端到端跑真实 wire 编码。
pub fn mock_client(protocol: Protocol, base_url: &str, model: &str) -> Arc<dyn LlmClient> {
    let rec = mock_record(protocol, base_url, model);
    client_from_record(&rec, "mock-agent/0.1.0").expect("构造 mock 客户端失败")
}

/// 造一条带指定 User-Agent 的请求元信息。
///
/// `RequestMeta` 没有 `Default`,而直接结构体字面量在字段增删时会连带改一堆测试;
/// 这里集中收敛,只暴露测试真正关心的两个旋钮。
pub fn meta(user_agent: &str) -> lsm_agent::llm::RequestMeta {
    let mut m = lsm_agent::llm::RequestMeta::new("sess-mock-0001", "dev-mock-0001");
    m.user_agent = user_agent.to_string();
    m
}

// ---------------------------------------------------------------------------
// 断言助手
// ---------------------------------------------------------------------------

/// 抓包摘要:断言失败时打出来,替代 e2e 时代落盘的 jsonl 抓包。
pub fn dump_requests(label: &str, reqs: &[CapturedRequest]) -> String {
    let mut out = format!("\n===== mock 抓包 [{label}] 共 {} 条 =====\n", reqs.len());
    for (i, r) in reqs.iter().enumerate() {
        out.push_str(&format!(
            "--- #{} {} {} | UA={:?} | tools={:?} | system_len={}\n",
            i,
            r.method,
            r.path,
            r.header("user-agent").unwrap_or(""),
            r.tool_names(),
            r.system_text().len()
        ));
    }
    out.push_str("========================================\n");
    out
}

/// 断言「至少有一条请求命中」,失败时附带抓包摘要。
#[track_caller]
pub fn assert_some_request(reqs: &[CapturedRequest], label: &str, pred: impl Fn(&CapturedRequest) -> bool) {
    assert!(
        reqs.iter().any(&pred),
        "没有任何请求命中判据 [{label}]\n{}",
        dump_requests(label, reqs)
    );
}

/// 断言「请求数落在区间内」,失败时附带抓包摘要。
#[track_caller]
pub fn assert_request_count(reqs: &[CapturedRequest], label: &str, min: usize, max: usize) {
    assert!(
        reqs.len() >= min && reqs.len() <= max,
        "[{label}] 请求数 {} 不在 [{min}, {max}] 区间\n{}",
        reqs.len(),
        dump_requests(label, reqs)
    );
}