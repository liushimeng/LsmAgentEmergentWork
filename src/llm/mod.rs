//! LLM 客户端抽象与统一消息模型。
//!
//! 通过 [`LlmClient`] trait 屏蔽不同协议差异,内部实现 Anthropic Messages 与
//! OpenAI Chat Completions 两种协议。

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{Protocol, ProviderRecord};
use crate::error::{AgentError, Result};

pub mod anthropic;
pub mod cache_policy;
pub mod cancellable;
pub mod offline;
pub mod openai;
pub mod resilient;
pub mod sse;

pub use cache_policy::{
    apply_cache_policy, CacheBreakpoints, CacheHint, CachePolicy, CacheTtl, ANTHROPIC_BREAKPOINT_CAP,
};

/// 默认 Cache Policy: Anthropic 路径自动注入(其他协议客户端忽略)。
///
/// 修改此常量即可全局调整策略。laew 当前协议客户端仅 Anthropic
/// 客户端消费它(第十六轮 L1047)。
pub const DEFAULT_CACHE_POLICY: CachePolicy = CachePolicy::Auto;

pub use offline::{Connectivity, ConnectivitySnapshot, ConnectivityTracker, DEGRADED_THRESHOLD, OFFLINE_THRESHOLD};
use resilient::{ResilientLlmClient, CONNECT_TIMEOUT};

/// 统一的 HTTP 客户端构造入口:注入连接超时(防连接挂起导致 TUI 冻结)。
///
/// 流式阶段的 idle / 总超时见 [`sse::stream_chunks`]。
///
/// TLS 校验策略(自签名证书适配,2026-09-10,见
/// `docs/自签名证书TLS适配/01-设计与解决方案.md`):
/// 命中宽松策略(IP 主机自动 / `LAEW_TLS_INSECURE=1` 全局)时跳过证书校验,
/// 使 IP + 自签名证书的内网/自建网关 HTTPS API 可直接使用;rustls 纯 Rust
/// 实现保证 Windows / macOS / CentOS / Ubuntu 行为一致。
pub fn build_http_client(end_point: &str) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT);
    if tls_insecure_for(end_point) {
        // rustls 后端下同时跳过证书链与主机名校验;仅跳过校验,TLS 加密不降级。
        builder = builder.danger_accept_invalid_certs(true);
        tracing::warn!(
            "TLS 证书校验已放宽(LAEW_TLS_INSECURE 或 IP 主机自动策略):{end_point}"
        );
    }
    builder.build().expect("reqwest Client 构建失败")
}

/// 三级 TLS 校验策略判定:
///
/// - `LAEW_TLS_INSECURE=1/true/yes/on` → 全局宽松(所有 endpoint 跳过校验);
/// - `LAEW_TLS_INSECURE=0/false/no/off` → 全局严格(安全基线);
/// - 未设置(默认)→ 自动模式:endpoint 主机为 IP 地址(IPv4/IPv6)时放宽,
///   域名主机仍严格校验(IP 直连的 HTTPS 服务几乎只能是自签名证书)。
pub fn tls_insecure_for(end_point: &str) -> bool {
    match std::env::var("LAEW_TLS_INSECURE") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => endpoint_host_is_ip(end_point),
    }
}

/// 判定 endpoint 主机是否为 IP 字面量(IPv4 / IPv6,含 `[::1]` 写法)。
fn endpoint_host_is_ip(end_point: &str) -> bool {
    let Ok(u) = url::Url::parse(end_point) else {
        return false;
    };
    matches!(u.host(), Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)))
}

/// 解析 `Retry-After` 头(仅支持 delta-seconds;HTTP-date 形式少见,忽略)。
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: u64 = v.trim().parse().ok()?;
    Some(secs.saturating_mul(1000))
}

/// 请求元数据:会话 / 设备标识,由协议层写入 HTTP 头与请求体。
#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub session_id: String,
    pub device_id: String,
    /// 本次调用的输出 token 上限(由会话级 `MaxTokensState` 状态机计算)。
    ///
    /// - Anthropic 协议:作为请求体 `max_tokens` 字段(覆盖默认 8192)。
    /// - OpenAI 协议:作为请求体 `max_tokens` 字段(此前完全不传,长代码回复易静默截断)。
    ///
    /// `None` = 协议层使用自身默认值(Anthropic 8192,OpenAI 不传)。
    /// 设计见 `tmpPlan/2026-09-09_09-max-tokens静默升级与失败计数预警方案.md`。
    pub max_tokens_override: Option<u32>,
    /// 发起本次请求的 Agent 的 User-Agent(如 `LsmAgentEmergentWork-Yolo/0.1.2 <build>`)。
    ///
    /// 由 `Agent::run_session_inner` 按当前 profile 逐请求注入,使 8 角色在
    /// 抓包层面可辨识(2026-09-09 第 08 轮,方案 tmpPlan/2026-09-09_08);
    /// 为空时协议层回退到客户端构造期的默认 UA。
    pub user_agent: String,
    /// 结构化输出强制通道(2026-09-09 第 13 轮,实现 L6/L19):
    ///
    /// `Some(tool_name)` 时协议层发出 forced `tool_choice`——
    /// - Anthropic:`{"type":"tool","name":X,"disable_parallel_tool_use":true}`
    /// - OpenAI:`{"type":"function","function":{"name":X}}`
    ///
    /// 模型必须以 tool_use 形式返回结构化结果(input 即合法 JSON 对象)。
    /// 由 `Agent::run_session_inner` 从 `AgentProfile.emit_tool` 注入;
    /// Provider 不支持时由 `resilient.rs` 自动去掉 forced 降级重试。
    /// `None` = 默认行为(Anthropic 不发 tool_choice / OpenAI 发 "auto")。
    pub forced_tool: Option<String>,
}

impl RequestMeta {
    /// 便捷构造:仅设置会话 / 设备 ID,max_tokens 走协议默认。
    pub fn new(session_id: impl Into<String>, device_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            device_id: device_id.into(),
            max_tokens_override: None,
            user_agent: String::new(),
            forced_tool: None,
        }
    }

    /// 带 max_tokens override 的构造(供 Agent 循环注入状态机当前值)。
    pub fn with_max_tokens(
        session_id: impl Into<String>,
        device_id: impl Into<String>,
        max_tokens: u32,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            device_id: device_id.into(),
            max_tokens_override: Some(max_tokens),
            user_agent: String::new(),
            forced_tool: None,
        }
    }

    /// 解析本次请求实际使用的 User-Agent:meta 逐请求注入值优先,空则回退默认。
    ///
    /// 协议客户端(anthropic / openai)在 `complete()` 内调用,默认值取构造期
    /// 烧入的 UA(main / tui 传入的 work_profile UA,兜底用)。
    pub fn resolve_user_agent<'a>(&'a self, fallback: &'a str) -> &'a str {
        if self.user_agent.trim().is_empty() {
            fallback
        } else {
            self.user_agent.as_str()
        }
    }
}

/// 构造两协议通用的请求头:`Content-Type` / `User-Agent` / `Authorization` / `X-Session-Id`。
pub fn build_common_headers(
    api_key: &str,
    meta: &RequestMeta,
    user_agent: &str,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("user-agent"),
        HeaderValue::from_str(user_agent)
            .map_err(|e| AgentError::Llm(format!("User-Agent header 非法: {e}")))?,
    );
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|e| AgentError::Llm(format!("Authorization header 非法: {e}")))?,
    );
    headers.insert(
        HeaderName::from_static("x-session-id"),
        HeaderValue::from_str(&meta.session_id)
            .map_err(|e| AgentError::Llm(format!("X-Session-Id header 非法: {e}")))?,
    );
    Ok(headers)
}

/// 对话角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 消息内容块(统一表示文本 / 工具调用 / 工具结果)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error,
        }
    }
}

/// 一条对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl ChatMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content: blocks,
        }
    }

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentBlock::tool_result(tool_use_id, content, is_error)],
        }
    }

    /// 提取消息内全部文本块的合并内容(测试 / 日志 / 展示用;
    /// tool_use / tool_result 块不参与)。
    pub fn content_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// 工具定义(协议无关)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema 对象,对应 Anthropic 的 `input_schema` / OpenAI 的 `parameters`
    pub input_schema: Value,
}

impl ToolDef {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// 一次补全的结果:文本与工具调用可同时存在(以文本为主,工具调用为辅)
#[derive(Debug, Default, Clone)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCallReq>,
    /// Token 用量(由 SSE 流中的 message_start / message_delta / 尾部 usage chunk 汇总)。
    pub usage: Usage,
    /// 终止原因(Anthropic stop_reason / OpenAI finish_reason),可选。
    pub stop_reason: Option<String>,
}

impl Completion {
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// Token 用量统计。所有字段为 0 表示上游未提供。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入侧 token 数(Anthropic input_tokens / OpenAI prompt_tokens)。
    pub input_tokens: u32,
    /// 输出侧 token 数(Anthropic output_tokens / OpenAI completion_tokens)。
    pub output_tokens: u32,
    /// Anthropic cache_read_input_tokens / OpenAI prompt_tokens_details.cached_tokens。
    pub cache_read_input_tokens: u32,
    /// Anthropic cache_creation_input_tokens(可选,OpenAI 不发)。
    pub cache_creation_input_tokens: u32,
}

/// 一次工具调用请求(协议无关)
#[derive(Debug, Clone)]
pub struct ToolCallReq {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// LLM 客户端抽象
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion>;

    /// 当前客户端使用的协议(用于系统提示词按协议渲染)。
    fn protocol(&self) -> Protocol;
}

/// 根据数据库记录创建对应协议的用户端(注入 User-Agent 兜底值)。
///
/// 注意:此处传入的 `user_agent` 仅作为**兜底默认值**——正常运行时
/// `Agent::run_session_inner` 会按当前 profile 逐请求覆盖
/// (`RequestMeta::user_agent`),使 8 角色在抓包层面各自可辨识;
/// 仅绕过 Agent 循环的直接调用(测试等)才会落到该兜底值。
///
/// 自动包一层 [`ResilientLlmClient`]:超时感知 + 自动重试 + 指数退避 +
/// 三态熔断,调用方(main / tui)零改动即获得弹性与故障隔离。
pub fn client_from_record(record: &ProviderRecord, user_agent: &str) -> Result<Arc<dyn LlmClient>> {
    // D9-7 SSRF 防护(L1608/L1625):创建 LLM 客户端前校验 end_point 安全性,
    // 拒绝私网/CGNAT/link-local 请求,防 SSRF 攻击。
    crate::agent::safety::url_safety::is_safe_endpoint(&record.end_point)
        .map_err(|e| crate::error::AgentError::Config(crate::database::ConfigError::UrlSafety(format!("end_point 不安全: {e}"))))?;
    let inner: Arc<dyn LlmClient> = match record.protocol {
        Protocol::Anthropic => {
            let c = anthropic::AnthropicClient::new(
                &record.end_point,
                &record.api_key,
                &record.model_name,
                user_agent,
            )?;
            Arc::new(c)
        }
        Protocol::OpenAi => {
            let c = openai::OpenAiClient::new(
                &record.end_point,
                &record.api_key,
                &record.model_name,
                user_agent,
            )?;
            Arc::new(c)
        }
    };
    Ok(Arc::new(ResilientLlmClient::new(inner)))
}

/// 规整 end_point:去除尾部 `/`
pub fn normalize_endpoint(ep: &str) -> String {
    ep.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_common_headers_contains_all() {
        let meta = RequestMeta {
            session_id: "20260902-153012-abcd1234-1700000000000-1a2b3c".into(),
            device_id: "45d277355416ee1b2f42758fb292b60b45170a57a5b4dec5cb7fa1a40fdd17ec".into(),
            max_tokens_override: None,
            user_agent: String::new(),
            forced_tool: None,
        };
        let headers = build_common_headers(
            "sk-xxx",
            &meta,
            "LsmAgentEmergentWork-Work/0.1.0 2026-09-02 15:30:12 CST",
        )
        .unwrap();
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
        let ua = headers.get("user-agent").unwrap().to_str().unwrap();
        assert_eq!(
            ua,
            "LsmAgentEmergentWork-Work/0.1.0 2026-09-02 15:30:12 CST"
        );
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer sk-xxx");
        assert_eq!(
            headers.get("x-session-id").unwrap(),
            "20260902-153012-abcd1234-1700000000000-1a2b3c"
        );
    }

    // ========== Agent 身份逐请求注入(第 08 轮,方案 tmpPlan/2026-09-09_08) ==========

    #[test]
    fn resolve_user_agent_prefers_meta_injection() {
        // meta 注入优先:8 角色各自的 UA 覆盖构造期默认
        let mut meta = RequestMeta::new("sess-1", "dev-1");
        meta.user_agent = "LsmAgentEmergentWork-Yolo/0.1.2 build-x".into();
        assert_eq!(
            meta.resolve_user_agent("LsmAgentEmergentWork-SubAgent-Work/0.1.2 build-x"),
            "LsmAgentEmergentWork-Yolo/0.1.2 build-x"
        );
    }

    #[test]
    fn resolve_user_agent_falls_back_when_empty() {
        // 空(含纯空白)→ 回退构造期默认:绕过 Agent 循环的直接调用(测试等)不受影响
        let meta = RequestMeta::new("sess-1", "dev-1");
        assert_eq!(
            meta.resolve_user_agent("LsmAgentEmergentWork-SubAgent-Work/0.1.2 build-x"),
            "LsmAgentEmergentWork-SubAgent-Work/0.1.2 build-x"
        );
        let mut blank = RequestMeta::new("sess-1", "dev-1");
        blank.user_agent = "   ".into();
        assert_eq!(
            blank.resolve_user_agent("FALLBACK"),
            "FALLBACK",
            "纯空白 UA 应视为空,回退默认"
        );
    }

    // ========== 自签名证书 TLS 适配(2026-09-10) ==========

    #[test]
    fn endpoint_host_is_ip_detects_ip_literals() {
        assert!(endpoint_host_is_ip("https://8.130.85.252:29003/Anthropic"));
        assert!(endpoint_host_is_ip("https://127.0.0.1:8443"));
        assert!(endpoint_host_is_ip("https://[::1]:8443/v1"));
        assert!(endpoint_host_is_ip("http://192.168.1.10"));
    }

    #[test]
    fn endpoint_host_is_ip_rejects_domains_and_bad_urls() {
        assert!(!endpoint_host_is_ip("https://api.anthropic.com"));
        assert!(!endpoint_host_is_ip("https://example.com:8443/v1"));
        assert!(!endpoint_host_is_ip("not-a-url"));
        assert!(!endpoint_host_is_ip(""));
    }

    #[test]
    fn tls_insecure_auto_mode_relaxes_only_ip_hosts() {
        // 未设置环境变量 → 自动模式:IP 放宽 / 域名严格
        std::env::remove_var("LAEW_TLS_INSECURE");
        assert!(tls_insecure_for("https://8.130.85.252:29003"));
        assert!(!tls_insecure_for("https://api.anthropic.com"));
    }

    #[test]
    fn tls_insecure_env_overrides_auto_mode() {
        // 全局宽松
        std::env::set_var("LAEW_TLS_INSECURE", "1");
        assert!(tls_insecure_for("https://api.anthropic.com"));
        std::env::set_var("LAEW_TLS_INSECURE", "true");
        assert!(tls_insecure_for("https://api.anthropic.com"));
        // 全局严格(即使 IP 主机也不放宽)
        std::env::set_var("LAEW_TLS_INSECURE", "0");
        assert!(!tls_insecure_for("https://8.130.85.252:29003"));
        std::env::set_var("LAEW_TLS_INSECURE", "off");
        assert!(!tls_insecure_for("https://8.130.85.252:29003"));
        std::env::remove_var("LAEW_TLS_INSECURE");
    }
}
