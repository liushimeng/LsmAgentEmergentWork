//! LLM 调用自动弹性层:超时感知 + 自动重试 + 指数退避 + jitter(装饰器)。
//!
//! 包住任意 [`LlmClient`],对上仍是 `Arc<dyn LlmClient>`,调用方零改动:
//! - **可重试错误**(`LlmHttp` 408/425/429/5xx/529、`LlmNetwork`、上游流内
//!   `overloaded/rate_limit/api/timeout` 错误)→ 自动指数退避重试;
//! - **熔断器**(Closed/Open/HalfOpen)在重试耗尽后统计连续 Provider 故障,
//!   Open 快速失败,冷却到期自动只放行 1 个 HalfOpen 探测请求;
//! - **429 的 `Retry-After`** 服务端优先(60s 封顶,不做向下抖动);
//! - **不可重试错误**(401/403/404/413/422 等)→ 立即上抛,不浪费时间。
//!
//! 参数对齐知识库结论(专题-第三轮-错误处理重试与容错降级 §附录A 速查表):
//! 基数 500ms / 倍数 2 / 上限 8s / ±25% jitter / 最多 3 次重试。
//! jitter 种子用墙钟亚秒纳秒打散(同 atomcode 做法),**不引入 rand crate**。
//!
//! 重试安全性:laew 的 `complete()` 聚合完整响应后才返回,工具调用发生在其后,
//! 因此整请求重放无副作用(仅可能重复计费失败的请求,行业同款行为)。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::config::Protocol;
use crate::error::{AgentError, Result};
use crate::llm::{ChatMessage, Completion, LlmClient, RequestMeta, ToolDef};

/// 最多重试次数(总尝试 = 1 + max_retries)
pub const DEFAULT_MAX_RETRIES: usize = 3;
/// 退避基数(毫秒)
pub const DEFAULT_BASE_DELAY_MS: u64 = 500;
/// 退避上限(毫秒)
pub const DEFAULT_MAX_DELAY_MS: u64 = 8_000;
/// jitter 比例:延迟落在 [d×(1-r), d×(1+r)] 均匀分布
pub const DEFAULT_JITTER_RATIO: f64 = 0.25;
/// Retry-After 上限(毫秒),防恶意/异常大值
pub const DEFAULT_RETRY_AFTER_CAP_MS: u64 = 60_000;
/// 连续可重试最终失败达到该次数后熔断(对齐第十一轮熔断器对比表)
pub const DEFAULT_CIRCUIT_FAILURE_THRESHOLD: usize = 5;
/// Open → HalfOpen 冷却期
pub const DEFAULT_CIRCUIT_COOLDOWN: Duration = Duration::from_secs(30);
/// TCP 连接超时(所有协议客户端共用,经 [`crate::llm::build_http_client`])
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// SSE 相邻 chunk 间空闲超时(claudecode SSE idle 90s),防半开连接假活
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// 单次尝试总超时(claudecode 总超时 600s),防无限慢流
pub const TOTAL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(600);
/// 等待响应头(TTFB)超时:TCP 连上后服务端迟迟不回 HTTP 头(网关挂起/假活)的兜底。
///
/// 此前该阶段无任何超时(`connect_timeout` 只管握手,流内两级超时只在拿到
/// `Response` 之后生效),真实事故:网关不回包头 → laew 无限等待、CLI 零反馈。
pub const RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(120);

/// CLI 等待心跳开关(TUI 模式保持关,避免写坏 alternate screen;-p/-f 单轮开启)。
static PROGRESS_FEEDBACK: AtomicBool = AtomicBool::new(false);

/// 开启/关闭 CLI 等待心跳(`-p`/`-f` 路径开启;默认关)。
pub fn set_progress_feedback(enabled: bool) {
    PROGRESS_FEEDBACK.store(enabled, Ordering::Relaxed);
}

/// 等待心跳守卫:存活期间每 20s 向 stderr 输出一行等待进度,
/// 帮助用户区分「正常生成中」与「已经挂死」;drop 时自动中止后台 ticker。
pub struct ProgressGuard {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProgressGuard {
    pub fn start(label: &str) -> Self {
        if !PROGRESS_FEEDBACK.load(Ordering::Relaxed) {
            return Self { handle: None };
        }
        let label = label.to_string();
        let started_at = Instant::now();
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // interval 首个 tick 立即完成,先消费掉再进入 20s 节奏
            tick.tick().await;
            loop {
                tick.tick().await;
                eprintln!("[laew] {}等待中… {}s", label, started_at.elapsed().as_secs());
            }
        });
        Self { handle: Some(handle) }
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// 重试策略配置(测试可注入更小延迟)。
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    pub jitter_ratio: f64,
    pub retry_after_cap_ms: u64,
    pub circuit_failure_threshold: usize,
    pub circuit_cooldown: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay_ms: DEFAULT_BASE_DELAY_MS,
            max_delay_ms: DEFAULT_MAX_DELAY_MS,
            jitter_ratio: DEFAULT_JITTER_RATIO,
            retry_after_cap_ms: DEFAULT_RETRY_AFTER_CAP_MS,
            circuit_failure_threshold: DEFAULT_CIRCUIT_FAILURE_THRESHOLD,
            circuit_cooldown: DEFAULT_CIRCUIT_COOLDOWN,
        }
    }
}

/// 可重试性判定(纯函数)。
///
/// 状态码清单对齐 atomcode:`408 | 425 | 429 | 500 | 502 | 503 | 504 | 529`;
/// 网络类错误一律可重试;流内错误按上游 error type 白名单。
pub fn is_retryable(err: &AgentError) -> bool {
    match err {
        AgentError::LlmHttp { status, .. } => {
            matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
        }
        AgentError::LlmNetwork(_) => true,
        AgentError::LlmStream { kind, .. } => matches!(
            kind.as_str(),
            "overloaded_error" | "rate_limit_error" | "api_error" | "timeout_error"
        ),
        _ => false,
    }
}

/// 三态熔断器的内部状态。`generation` 用于丢弃已过期的并发请求结果:
/// 例如某个 Closed 请求在途时另一个请求把状态打成 Open,旧请求稍后的
/// Ok/Err 不能再把状态错误地改回 Closed。
#[derive(Debug)]
struct CircuitInner {
    generation: u64,
    state: CircuitState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitState {
    Closed {
        consecutive_failures: usize,
    },
    Open {
        until: Instant,
        consecutive_failures: usize,
    },
    HalfOpen {
        probe_in_flight: bool,
    },
}

/// 面向测试 / 后续遥测的只读状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitStatus {
    Closed,
    Open { retry_in_ms: u64 },
    HalfOpen { probe_in_flight: bool },
}

/// 一次调用获得的熔断放行凭证。HalfOpen 持有 RAII 探测位,
/// 请求被外层取消 / future 被 drop 时也能自动释放,不会永久卡死。
enum CircuitLease {
    Closed { generation: u64 },
    HalfOpen { generation: u64, _guard: ProbeGuard },
}

struct ProbeGuard {
    circuit: Arc<Mutex<CircuitInner>>,
    generation: u64,
}

impl Drop for ProbeGuard {
    fn drop(&mut self) {
        let mut inner = self.circuit.lock().expect("circuit breaker poisoned");
        if inner.generation == self.generation {
            if let CircuitState::HalfOpen { probe_in_flight } = &mut inner.state {
                *probe_in_flight = false;
            }
        }
    }
}

/// 指数退避 + ±jitter 延迟(纯函数,种子外注入以便单测断言区间)。
///
/// `capped = min(base × 2^attempt, max)`;延迟均匀落在
/// `[capped×(1-ratio), capped×(1+ratio)]`。
pub fn backoff_delay_ms(attempt: usize, cfg: &RetryConfig, seed: u64) -> u64 {
    let exp = attempt.min(16) as u32;
    let capped = cfg
        .base_delay_ms
        .saturating_mul(1u64 << exp)
        .min(cfg.max_delay_ms);
    let window = (capped as f64 * cfg.jitter_ratio * 2.0) as u64;
    let lo = capped.saturating_sub(window / 2);
    lo + seed % (window + 1)
}

/// 墙钟亚秒纳秒打散为种子(同 atomcode jitter 做法,不引 rand crate)。
pub fn jitter_seed(salt: u64) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5DEECE66D);
    let mut x = n ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17);
    // xorshift64* 一轮打散
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// 弹性装饰器:实现 `LlmClient`,内部包装真实客户端并做自动重试。
pub struct ResilientLlmClient {
    inner: Arc<dyn LlmClient>,
    cfg: RetryConfig,
    circuit: Arc<Mutex<CircuitInner>>,
}

impl ResilientLlmClient {
    pub fn new(inner: Arc<dyn LlmClient>) -> Self {
        Self::with_config(inner, RetryConfig::default())
    }

    pub fn with_config(inner: Arc<dyn LlmClient>, cfg: RetryConfig) -> Self {
        Self {
            inner,
            cfg,
            circuit: Arc::new(Mutex::new(CircuitInner {
                generation: 0,
                state: CircuitState::Closed {
                    consecutive_failures: 0,
                },
            })),
        }
    }

    /// 当前熔断状态快照(测试与后续遥测复用;锁内不 await)。
    pub fn circuit_status(&self) -> CircuitStatus {
        let inner = self.circuit.lock().expect("circuit breaker poisoned");
        self.status_from(&inner)
    }

    fn status_from(&self, inner: &CircuitInner) -> CircuitStatus {
        match &inner.state {
            CircuitState::Closed { .. } => CircuitStatus::Closed,
            CircuitState::Open { until, .. } => CircuitStatus::Open {
                retry_in_ms: until
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(u64::MAX as u128) as u64,
            },
            CircuitState::HalfOpen { probe_in_flight } => CircuitStatus::HalfOpen {
                probe_in_flight: *probe_in_flight,
            },
        }
    }

    /// 获取调用资格。Open 冷却未到期快速失败;到期后转入 HalfOpen 并
    /// 保证同一时间只有一个探测请求。
    fn acquire_circuit(&self) -> std::result::Result<CircuitLease, AgentError> {
        let mut inner = self.circuit.lock().expect("circuit breaker poisoned");
        match inner.state {
            CircuitState::Closed { .. } => Ok(CircuitLease::Closed {
                generation: inner.generation,
            }),
            CircuitState::Open {
                until,
                consecutive_failures,
            } => {
                let now = Instant::now();
                if until > now {
                    let retry_in_ms = (until - now).as_millis().min(u64::MAX as u128) as u64;
                    tracing::warn!(
                        retry_in_ms,
                        consecutive_failures,
                        "LLM Provider 熔断器仍打开,快速失败"
                    );
                    return Err(AgentError::LlmCircuitOpen {
                        retry_in_ms,
                        consecutive_failures,
                    });
                }
                inner.generation += 1;
                inner.state = CircuitState::HalfOpen {
                    probe_in_flight: true,
                };
                let generation = inner.generation;
                drop(inner);
                tracing::info!("LLM Provider 熔断冷却期到期,自动进入 HalfOpen 单探测");
                Ok(CircuitLease::HalfOpen {
                    generation,
                    _guard: ProbeGuard {
                        circuit: self.circuit.clone(),
                        generation,
                    },
                })
            }
            CircuitState::HalfOpen {
                probe_in_flight: false,
            } => {
                inner.state = CircuitState::HalfOpen {
                    probe_in_flight: true,
                };
                let generation = inner.generation;
                drop(inner);
                tracing::debug!("LLM Provider HalfOpen 探测位已释放,允许下一个探测");
                Ok(CircuitLease::HalfOpen {
                    generation,
                    _guard: ProbeGuard {
                        circuit: self.circuit.clone(),
                        generation,
                    },
                })
            }
            CircuitState::HalfOpen {
                probe_in_flight: true,
            } => Err(AgentError::LlmCircuitOpen {
                retry_in_ms: 0,
                consecutive_failures: self.cfg.circuit_failure_threshold,
            }),
        }
    }

    fn record_success(&self, lease: &CircuitLease) {
        let generation = lease.generation();
        let mut inner = self.circuit.lock().expect("circuit breaker poisoned");
        if inner.generation != generation {
            return;
        }
        match inner.state {
            CircuitState::Closed { .. } => {
                inner.state = CircuitState::Closed {
                    consecutive_failures: 0,
                };
            }
            CircuitState::HalfOpen { .. } => {
                inner.generation += 1;
                inner.state = CircuitState::Closed {
                    consecutive_failures: 0,
                };
                tracing::info!("LLM Provider 半开探测成功,熔断器自动关闭");
            }
            CircuitState::Open { .. } => {}
        }
    }

    fn record_failure(&self, lease: &CircuitLease) {
        let generation = lease.generation();
        let mut inner = self.circuit.lock().expect("circuit breaker poisoned");
        if inner.generation != generation {
            return;
        }
        match inner.state {
            CircuitState::Closed {
                consecutive_failures,
            } => {
                let failures = consecutive_failures.saturating_add(1);
                if failures >= self.failure_threshold() {
                    inner.generation += 1;
                    inner.state = CircuitState::Open {
                        until: Instant::now() + self.cfg.circuit_cooldown,
                        consecutive_failures: failures,
                    };
                    tracing::warn!(
                        consecutive_failures = failures,
                        cooldown_ms = self.cfg.circuit_cooldown.as_millis(),
                        "连续可重试失败达到阈值,LLM Provider 熔断器自动打开"
                    );
                } else {
                    inner.state = CircuitState::Closed {
                        consecutive_failures: failures,
                    };
                }
            }
            CircuitState::HalfOpen { .. } => {
                inner.generation += 1;
                inner.state = CircuitState::Open {
                    until: Instant::now() + self.cfg.circuit_cooldown,
                    consecutive_failures: self.failure_threshold(),
                };
                tracing::warn!(
                    cooldown_ms = self.cfg.circuit_cooldown.as_millis(),
                    "LLM Provider 半开探测失败,熔断器重新打开"
                );
            }
            CircuitState::Open { .. } => {}
        }
    }

    fn failure_threshold(&self) -> usize {
        self.cfg.circuit_failure_threshold.max(1)
    }

    /// 单次重试应等待的时长:429/503 的 Retry-After 服务端优先(封顶,不向下抖动),
    /// 其余走指数退避 + jitter。
    fn delay_for(&self, err: &AgentError, attempt: usize) -> u64 {
        if let AgentError::LlmHttp {
            retry_after_ms: Some(ms),
            ..
        } = err
        {
            return (*ms).min(self.cfg.retry_after_cap_ms);
        }
        backoff_delay_ms(attempt, &self.cfg, jitter_seed(attempt as u64))
    }

    async fn complete_with_retry(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion> {
        let lease = self.acquire_circuit()?;
        // CLI 等待心跳:覆盖全部尝试 + 退避睡眠;TUI 模式开关关闭时为空操作
        let _progress = ProgressGuard::start("LLM 响应");
        let mut attempt: usize = 0;
        let result = loop {
            match self.inner.complete(system, messages, tools, meta).await {
                Ok(c) => break Ok(c),
                Err(e) => {
                    if attempt >= self.cfg.max_retries || !is_retryable(&e) {
                        break Err(e);
                    }
                    let delay = self.delay_for(&e, attempt);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries = self.cfg.max_retries,
                        delay_ms = delay,
                        error = %e,
                        "LLM 调用失败,自动重试(指数退避)"
                    );
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    attempt += 1;
                }
            }
        };

        match &result {
            Ok(_) => self.record_success(&lease),
            Err(e) if matches!(e, AgentError::Cancelled) => {
                // 用户取消不是 Provider 故障;lease drop 仅释放 HalfOpen 探测位。
            }
            Err(e) if is_retryable(e) => self.record_failure(&lease),
            // 401/400/422 等请求侧错误说明服务可达,不应把 Provider 熔断。
            Err(_) => self.record_success(&lease),
        }
        result
    }
}

impl CircuitLease {
    fn generation(&self) -> u64 {
        match self {
            Self::Closed { generation } => *generation,
            Self::HalfOpen { generation, .. } => *generation,
        }
    }
}

#[async_trait]
impl LlmClient for ResilientLlmClient {
    async fn complete(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
        meta: &RequestMeta,
    ) -> Result<Completion> {
        // 结构化输出强制通道的自适应降级(L6/L19,2026-09-09 第 13 轮):
        // forced tool_choice 被第三方网关/代理拒绝(4xx + tool_choice 相关报错)时,
        // 自动去掉 forced 降级为默认 auto 再走一次完整重试管线。
        // forced 拒绝是不可重试 4xx(不污染熔断计数),降级后模型回退文本 JSON,
        // 由既有三重解析链 + 修复链兜底 —— 全程无需用户配置。
        match self.complete_with_retry(system, messages, tools, meta).await {
            Ok(c) => Ok(c),
            Err(e) if meta.forced_tool.is_some() && looks_like_tool_choice_rejection(&e) => {
                tracing::warn!(
                    tool = meta.forced_tool.as_deref().unwrap_or_default(),
                    error = %e,
                    "forced tool_choice 被 Provider 拒绝,自动降级为 auto 重试"
                );
                let mut degraded = meta.clone();
                degraded.forced_tool = None;
                self.complete_with_retry(system, messages, tools, &degraded).await
            }
            Err(e) => Err(e),
        }
    }

    fn protocol(&self) -> Protocol {
        self.inner.protocol()
    }
}

/// 判断错误是否为「Provider 不支持 forced tool_choice」的拒绝形态。
///
/// 特征:4xx(400/404/422)且错误文本提及 tool_choice / tool choice /
/// tool + not support / function calling 不支持(大小写不敏感)。
/// 命中即触发降级;401 鉴权错、400 溢出等其他 4xx 不命中。
pub fn looks_like_tool_choice_rejection(err: &AgentError) -> bool {
    let AgentError::LlmHttp { status, message, .. } = err else {
        return false;
    };
    if !matches!(status, 400 | 404 | 422) {
        return false;
    }
    let lower = message.to_lowercase();
    lower.contains("tool_choice")
        || lower.contains("tool choice")
        || (lower.contains("tool") && lower.contains("not support"))
        || lower.contains("function calling is not supported")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::sync::Barrier;

    /// 脚本化 Mock:按预定顺序弹出响应,并统计调用次数。
    struct MockClient {
        responses: Mutex<VecDeque<Result<Completion>>>,
        calls: Arc<AtomicUsize>,
        proto: Protocol,
    }

    impl MockClient {
        fn new(responses: Vec<Result<Completion>>, proto: Protocol) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    responses: Mutex::new(responses.into()),
                    calls: calls.clone(),
                    proto,
                },
                calls,
            )
        }
    }

    fn ok_completion() -> Completion {
        Completion {
            text: "ok".into(),
            ..Default::default()
        }
    }

    fn http_err(status: u16, retry_after_ms: Option<u64>) -> AgentError {
        AgentError::LlmHttp {
            status,
            retry_after_ms,
            message: format!("HTTP {status}"),
        }
    }

    #[async_trait]
    impl LlmClient for MockClient {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[ToolDef],
            _meta: &RequestMeta,
        ) -> Result<Completion> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ok_completion()))
        }

        fn protocol(&self) -> Protocol {
            self.proto
        }
    }

    /// 测试专用小延迟配置(总重试耗时 < 100ms)。
    fn fast_cfg() -> RetryConfig {
        RetryConfig {
            max_retries: 3,
            base_delay_ms: 1,
            max_delay_ms: 4,
            jitter_ratio: 0.25,
            retry_after_cap_ms: 50,
            circuit_failure_threshold: DEFAULT_CIRCUIT_FAILURE_THRESHOLD,
            circuit_cooldown: DEFAULT_CIRCUIT_COOLDOWN,
        }
    }

    /// 熔断器专用配置:不重试、阈值小、冷却期短。
    fn circuit_cfg(threshold: usize, cooldown_ms: u64) -> RetryConfig {
        RetryConfig {
            max_retries: 0,
            base_delay_ms: 1,
            max_delay_ms: 1,
            jitter_ratio: 0.0,
            retry_after_cap_ms: 1,
            circuit_failure_threshold: threshold,
            circuit_cooldown: Duration::from_millis(cooldown_ms),
        }
    }

    fn test_meta() -> RequestMeta {
        RequestMeta {
            session_id: "test-session".into(),
            device_id: "test-device".into(),
            max_tokens_override: None,
            user_agent: String::new(),
            forced_tool: None,
        }
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let (mock, calls) = MockClient::new(
            vec![
                Err(http_err(500, None)),
                Err(AgentError::LlmNetwork("连接被重置".into())),
                Ok(ok_completion()),
            ],
            Protocol::Anthropic,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        let out = client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert_eq!(out.text, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_retryable_fails_immediately() {
        let (mock, calls) = MockClient::new(vec![Err(http_err(401, None))], Protocol::Anthropic);
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        let err = client
            .complete("", &[], &[], &test_meta())
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::LlmHttp { status: 401, .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "401 不应重试");
    }

    #[tokio::test]
    async fn retries_exhausted_returns_last_error() {
        let (mock, calls) = MockClient::new(
            vec![
                Err(http_err(503, None)),
                Err(http_err(503, None)),
                Err(http_err(503, None)),
                Err(http_err(503, None)),
            ],
            Protocol::OpenAi,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        let err = client
            .complete("", &[], &[], &test_meta())
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::LlmHttp { status: 503, .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 4, "1 次初始 + 3 次重试");
    }

    #[tokio::test]
    async fn circuit_opens_after_consecutive_final_failures() {
        let (mock, calls) = MockClient::new(
            vec![Err(http_err(503, None)), Err(http_err(503, None))],
            Protocol::Anthropic,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), circuit_cfg(2, 30));

        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        match client.circuit_status() {
            CircuitStatus::Open { retry_in_ms } => assert!(retry_in_ms <= 30),
            status => panic!("熔断器应处于 Open,实际 {status:?}"),
        }

        let err = client
            .complete("", &[], &[], &test_meta())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AgentError::LlmCircuitOpen {
                consecutive_failures: 2,
                ..
            }
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "Open 后必须快速失败,不再触达底层客户端"
        );
    }

    #[tokio::test]
    async fn half_open_probe_success_closes_circuit() {
        let (mock, calls) = MockClient::new(
            vec![Err(http_err(500, None)), Ok(ok_completion())],
            Protocol::OpenAi,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), circuit_cfg(1, 1));

        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        assert!(matches!(
            client.circuit_status(),
            CircuitStatus::Open { .. }
        ));

        tokio::time::sleep(Duration::from_millis(5)).await;
        let out = client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert_eq!(out.text, "ok");
        assert_eq!(client.circuit_status(), CircuitStatus::Closed);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn half_open_failure_reopens_circuit() {
        let (mock, calls) = MockClient::new(
            vec![
                Err(http_err(503, None)),
                Err(AgentError::LlmNetwork("连接仍失败".into())),
            ],
            Protocol::Anthropic,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), circuit_cfg(1, 1));

        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());

        assert!(matches!(
            client.circuit_status(),
            CircuitStatus::Open { .. }
        ));
        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2, "重新 Open 后应快速失败");
    }

    #[tokio::test]
    async fn dropped_half_open_future_releases_probe_slot() {
        struct HangingProbeMock {
            failed_first: AtomicUsize,
        }

        #[async_trait]
        impl LlmClient for HangingProbeMock {
            async fn complete(
                &self,
                _system: &str,
                _messages: &[ChatMessage],
                _tools: &[ToolDef],
                _meta: &RequestMeta,
            ) -> Result<Completion> {
                if self.failed_first.swap(1, Ordering::SeqCst) == 0 {
                    return Err(http_err(503, None));
                }
                std::future::pending().await
            }

            fn protocol(&self) -> Protocol {
                Protocol::Anthropic
            }
        }

        let client = Arc::new(ResilientLlmClient::with_config(
            Arc::new(HangingProbeMock {
                failed_first: AtomicUsize::new(0),
            }),
            circuit_cfg(1, 1),
        ));
        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        tokio::time::sleep(Duration::from_millis(5)).await;

        let meta = test_meta();
        let mut probe = Box::pin(client.complete("", &[], &[], &meta));
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(1)) => {}
            _ = &mut probe => panic!("测试 Mock 不应完成"),
        }
        assert_eq!(
            client.circuit_status(),
            CircuitStatus::HalfOpen {
                probe_in_flight: true
            }
        );
        drop(probe);

        assert_eq!(
            client.circuit_status(),
            CircuitStatus::HalfOpen {
                probe_in_flight: false
            },
            "外层取消导致 future 被 drop 时,探测位必须释放"
        );
    }

    #[tokio::test]
    async fn half_open_allows_only_one_probe_during_parallel_calls() {
        struct ProbeMock {
            failed_first: AtomicUsize,
            entered: Arc<Barrier>,
            release: Arc<Barrier>,
            calls: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl LlmClient for ProbeMock {
            async fn complete(
                &self,
                _system: &str,
                _messages: &[ChatMessage],
                _tools: &[ToolDef],
                _meta: &RequestMeta,
            ) -> Result<Completion> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if self.failed_first.swap(1, Ordering::SeqCst) == 0 {
                    return Err(http_err(503, None));
                }
                self.entered.wait().await;
                self.release.wait().await;
                Ok(ok_completion())
            }

            fn protocol(&self) -> Protocol {
                Protocol::Anthropic
            }
        }

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let calls = Arc::new(AtomicUsize::new(0));
        let mock = ProbeMock {
            failed_first: AtomicUsize::new(0),
            entered: entered.clone(),
            release: release.clone(),
            calls: calls.clone(),
        };
        let client = Arc::new(ResilientLlmClient::with_config(
            Arc::new(mock),
            circuit_cfg(1, 1),
        ));

        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        tokio::time::sleep(Duration::from_millis(5)).await;

        let probe_client = client.clone();
        let probe =
            tokio::spawn(async move { probe_client.complete("", &[], &[], &test_meta()).await });
        entered.wait().await;

        let second = tokio::time::timeout(
            Duration::from_millis(200),
            client.complete("", &[], &[], &test_meta()),
        )
        .await
        .expect("第二个请求应立即快速失败")
        .unwrap_err();
        assert!(matches!(second, AgentError::LlmCircuitOpen { .. }));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "只有 HalfOpen 探测应触达底层"
        );

        release.wait().await;
        probe.await.unwrap().unwrap();
        assert_eq!(client.circuit_status(), CircuitStatus::Closed);
    }

    #[tokio::test]
    async fn non_retryable_error_does_not_trip_circuit() {
        let (mock, calls) = MockClient::new(vec![Err(http_err(401, None))], Protocol::Anthropic);
        let client = ResilientLlmClient::with_config(Arc::new(mock), circuit_cfg(1, 1));

        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        assert_eq!(client.circuit_status(), CircuitStatus::Closed);

        client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn success_resets_consecutive_failure_count() {
        let (mock, calls) = MockClient::new(
            vec![
                Err(http_err(503, None)),
                Ok(ok_completion()),
                Err(http_err(503, None)),
                Ok(ok_completion()),
            ],
            Protocol::OpenAi,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), circuit_cfg(2, 1));

        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert!(client.complete("", &[], &[], &test_meta()).await.is_err());
        client.complete("", &[], &[], &test_meta()).await.unwrap();

        assert_eq!(client.circuit_status(), CircuitStatus::Closed);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn retry_after_is_respected_with_cap() {
        let (mock, calls) = MockClient::new(
            vec![Err(http_err(429, Some(30))), Ok(ok_completion())],
            Protocol::Anthropic,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        let start = std::time::Instant::now();
        client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(25),
            "应等待 Retry-After(30ms),实际 {:?}",
            start.elapsed()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stream_overloaded_is_retried_but_invalid_request_is_not() {
        // overloaded_error → 可重试
        let (mock, calls) = MockClient::new(
            vec![
                Err(AgentError::LlmStream {
                    kind: "overloaded_error".into(),
                    message: "overloaded".into(),
                }),
                Ok(ok_completion()),
            ],
            Protocol::Anthropic,
        );
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        client.complete("", &[], &[], &test_meta()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // invalid_request_error → 不可重试
        let (mock2, calls2) = MockClient::new(
            vec![Err(AgentError::LlmStream {
                kind: "invalid_request_error".into(),
                message: "bad".into(),
            })],
            Protocol::Anthropic,
        );
        let client2 = ResilientLlmClient::with_config(Arc::new(mock2), fast_cfg());
        let _ = client2
            .complete("", &[], &[], &test_meta())
            .await
            .unwrap_err();
        assert_eq!(calls2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn is_retryable_classification_table() {
        for s in [408u16, 425, 429, 500, 502, 503, 504, 529] {
            assert!(is_retryable(&http_err(s, None)), "{s} 应可重试");
        }
        for s in [400u16, 401, 403, 404, 413, 422, 501] {
            assert!(!is_retryable(&http_err(s, None)), "{s} 不应重试");
        }
        assert!(is_retryable(&AgentError::LlmNetwork("timeout".into())));
        assert!(!is_retryable(&AgentError::Llm("其它".into())));
        assert!(!is_retryable(&AgentError::YoloParse("解析".into())));
        assert!(!is_retryable(&AgentError::LlmCircuitOpen {
            retry_in_ms: 1,
            consecutive_failures: 5,
        }));
    }

    #[test]
    fn backoff_respects_bounds_and_cap() {
        let cfg = RetryConfig::default(); // 500/8000/±25%
        for attempt in 0..6u64 {
            let expected_cap = (500u64.saturating_mul(1 << attempt)).min(8_000);
            let lo = (expected_cap as f64 * 0.75) as u64;
            let hi = (expected_cap as f64 * 1.25) as u64 + 1;
            for salt in [0u64, 1, 42, 0xDEAD_BEEF, u64::MAX] {
                let d = backoff_delay_ms(attempt as usize, &cfg, jitter_seed(salt));
                assert!(
                    d >= lo && d <= hi,
                    "attempt={attempt} delay={d} 不在 [{lo}, {hi}]"
                );
            }
        }
    }

    #[test]
    fn retry_after_delay_is_capped() {
        let client = ResilientLlmClient::with_config(
            Arc::new(MockClient::new(vec![], Protocol::Anthropic).0),
            RetryConfig::default(),
        );
        let err = http_err(429, Some(600_000));
        assert_eq!(
            client.delay_for(&err, 0),
            60_000,
            "Retry-After 应被 60s 封顶"
        );
    }

    #[tokio::test]
    async fn protocol_passthrough() {
        let (mock, _) = MockClient::new(vec![], Protocol::OpenAi);
        let client = ResilientLlmClient::new(Arc::new(mock));
        assert_eq!(client.protocol(), Protocol::OpenAi);
    }

    // ========== forced tool_choice 自适应降级(L6/L19,2026-09-09 第 13 轮) ==========

    /// 记录每次请求 forced_tool 的 mock(脚本式回复)。
    struct ForcedAwareMock {
        responses: Mutex<VecDeque<Result<Completion>>>,
        seen_forced: Arc<std::sync::Mutex<Vec<Option<String>>>>,
    }

    impl ForcedAwareMock {
        fn new(responses: Vec<Result<Completion>>) -> (Self, Arc<std::sync::Mutex<Vec<Option<String>>>>) {
            let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    responses: Mutex::new(responses.into()),
                    seen_forced: seen.clone(),
                },
                seen,
            )
        }
    }

    #[async_trait]
    impl LlmClient for ForcedAwareMock {
        async fn complete(
            &self,
            _system: &str,
            _messages: &[ChatMessage],
            _tools: &[ToolDef],
            meta: &RequestMeta,
        ) -> Result<Completion> {
            self.seen_forced
                .lock()
                .unwrap()
                .push(meta.forced_tool.clone());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(ok_completion()))
        }
        fn protocol(&self) -> Protocol {
            Protocol::Anthropic
        }
    }

    fn forced_meta() -> RequestMeta {
        let mut m = test_meta();
        m.forced_tool = Some("submit_task_classification".into());
        m
    }

    fn tool_choice_rejection() -> AgentError {
        AgentError::LlmHttp {
            status: 400,
            retry_after_ms: None,
            message: "HTTP 400: {\"error\":{\"message\":\"tool_choice is not supported by this endpoint\"}}".into(),
        }
    }

    #[tokio::test]
    async fn forced_tool_choice_rejection_degrades_to_auto() {
        // 首次 forced 请求被 Provider 拒绝(400 tool_choice) → 自动去掉 forced
        // 降级重试 → 第二次(forced=None)成功返回。
        let (mock, seen) = ForcedAwareMock::new(vec![
            Err(tool_choice_rejection()),
            Ok(ok_completion()),
        ]);
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        let c = client
            .complete("sys", &[], &[], &forced_meta())
            .await
            .expect("降级重试后应成功");
        assert_eq!(c.text, "ok");

        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.len(),
            2,
            "应恰好发生 2 次请求(forced 拒绝 + 降级 auto),实际: {seen:?}"
        );
        assert_eq!(seen[0].as_deref(), Some("submit_task_classification"));
        assert!(seen[1].is_none(), "第二次请求不应携带 forced tool_choice");
    }

    #[tokio::test]
    async fn non_rejection_4xx_does_not_degrade() {
        // 401 鉴权错不含 tool_choice 语义 → 不触发降级,原错误上抛
        let (mock, seen) = ForcedAwareMock::new(vec![Err(http_err(401, None))]);
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        let err = client
            .complete("sys", &[], &[], &forced_meta())
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::LlmHttp { status: 401, .. }));
        assert_eq!(seen.lock().unwrap().len(), 1, "401 不应触发降级重试");
    }

    #[tokio::test]
    async fn degradation_without_forced_tool_is_noop() {
        // 无 forced 的普通调用即使收到 tool_choice 形态报错也不重试
        // (错误来自 Provider 自身配置,与本地 forced 注入无关)。
        let (mock, seen) = ForcedAwareMock::new(vec![Err(tool_choice_rejection())]);
        let client = ResilientLlmClient::with_config(Arc::new(mock), fast_cfg());
        let err = client
            .complete("sys", &[], &[], &test_meta())
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::LlmHttp { status: 400, .. }));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn looks_like_rejection_matching() {
        // 命中:4xx + tool_choice 语义
        for (status, msg) in [
            (400, "Invalid value for 'tool_choice'"),
            (422, "tool choice not supported"),
            (400, "this endpoint does not support tool parameter"),
            (422, "function calling is not supported on this model"),
        ] {
            let e = AgentError::LlmHttp {
                status,
                retry_after_ms: None,
                message: msg.into(),
            };
            assert!(looks_like_tool_choice_rejection(&e), "应命中: {status} {msg}");
        }
        // 不命中:其他状态码 / 无关 4xx / 非网络错误
        for e in [
            AgentError::LlmHttp {
                status: 429,
                retry_after_ms: None,
                message: "tool_choice ...".into(),
            },
            AgentError::LlmHttp {
                status: 400,
                retry_after_ms: None,
                message: "prompt is too long".into(),
            },
            AgentError::LlmHttp {
                status: 401,
                retry_after_ms: None,
                message: "invalid api key".into(),
            },
            AgentError::Other("whatever".into()),
        ] {
            assert!(!looks_like_tool_choice_rejection(&e), "不应命中: {e}");
        }
    }
}
