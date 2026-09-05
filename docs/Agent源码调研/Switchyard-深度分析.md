# Switchyard 深度分析

> **项目**: Switchyard — NVIDIA 开发的 Rust LLM 流量代理与库
> **仓库**: https://github.com/NVIDIA-NeMo/Switchyard
> **版本**: v0.2.0 (pre-alpha)
> **许可证**: Apache-2.0
> **分析日期**: 2026-09-05
> **前置**: 基于 `Switchyard-源码调研.md` 的深入源码级分析

---

## 一、架构总览

### 1.1 六层分离架构

Switchyard 采用 Rust Workspace 组织，核心六层通过 `Cargo.toml` 的 `[workspace.dependencies]` 统一版本管理：

```
┌─────────────────────────────────────────────────────────────────────┐
│  第六层: switchyard-server (Axum HTTP 服务器 + 可观测性)              │
│   ├─ 多端点路由 (/v1/chat/completions, /v1/messages, /v1/responses) │
│   ├─ 优雅关闭 (graceful_shutdown + timeout)                         │
│   ├─ Prometheus 指标 + OTLP 导出                                    │
│   └─ W3C Trace Context 传播                                        │
├─────────────────────────────────────────────────────────────────────┤
│  第五层: switchyard-runner (TOML 配置驱动的运行器)                    │
│   ├─ DeploymentConfig 反序列化                                      │
│   ├─ AlgorithmSpec 多态枚举                                         │
│   └─ Runner 统一编排                                                │
├─────────────────────────────────────────────────────────────────────┤
│  第四层: libsy-llm-client (TranslatingLlmClient)                    │
│   ├─ 协议翻译 → HTTP 发送 → 协议解码                                │
│   ├─ 指数退避重试 (250ms → 2s) + Retry-After                       │
│   └─ 双 reqwest Client (普通 + forward_auth 禁用重定向)             │
├─────────────────────────────────────────────────────────────────────┤
│  第三层: libsy (Algorithm trait + 7 种路由算法)                      │
│   ├─ Noop / Passthrough / Random / LlmClassifier                   │
│   ├─ StageRouter / Composite / AdvisorGate                         │
│   └─ FallThrough<S> 级联骨架 + SessionState TTL 清理                │
├─────────────────────────────────────────────────────────────────────┤
│  第二层: switchyard-translation (TranslationEngine)                  │
│   ├─ FormatRegistry (OpenAiChat / AnthropicMessages / OpenAiResponses)│
│   ├─ StreamCodecRegistry (流式事件翻译)                             │
│   └─ PreservationMetadata 无损保留                                  │
├─────────────────────────────────────────────────────────────────────┤
│  第一层: switchyard-protocol (Provider-neutral IR)                   │
│   ├─ LlmRequest / LlmResponse / LlmResponseChunk                   │
│   ├─ ContentBlock::Unknown 无损保留                                 │
│   ├─ PreservationMetadata / ProviderExtensions                     │
│   └─ ResponseAccumulator 流式折叠                                   │
└─────────────────────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────────────┐
│  独立模块                                                            │
│   ├─ prefill-router (Transformers 嵌入提取, PyO3 嵌入 Python)       │
│   ├─ switchyard-skill-distillation (Trajectory/SkillCandidate)      │
│   ├─ switchyard-py (PyO3 Python 绑定)                               │
│   └─ switchyard-soak (浸泡测试)                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.2 请求流图

```
Client (Claude Code / Codex / OpenAI SDK)
    │
    ▼ [OpenAI Chat / Anthropic Messages / OpenAI Responses]
┌─────────────────────────────────────────────────────────┐
│  switchyard-server (Axum)                                │
│  ┌───────────────────────────────────────────────────┐  │
│  │ 1. W3C Trace Context 提取 (HeaderExtractor)       │  │
│  │ 2. RequestStart 时间戳 (stamp_request_start)      │  │
│  │ 3. resolve_route → decode_request → LlmRequest    │  │
│  │ 4. route.execute → Algorithm::route               │  │
│  │ 5. into_http_response → 编码回 wire format        │  │
│  │ 6. attach_routing_headers (x-model-router-...)    │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
    │
    ▼ [LlmRequest IR]
┌─────────────────────────────────────────────────────────┐
│  libsy (Algorithm)                                       │
│  ┌───────────────────────────────────────────────────┐  │
│  │ run_stream → Driver → Step::CallModel / Step::Done │  │
│  │   ├─ Processor 链 (信号提取/状态更新)              │  │
│  │   ├─ Classifier 链 (评分/放弃)                     │  │
│  │   └─ DefaultTarget (最终回退)                      │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
    │
    ▼ [CallModel → oneshot::Sender]
┌─────────────────────────────────────────────────────────┐
│  libsy-llm-client (TranslatingLlmClient)                 │
│  ┌───────────────────────────────────────────────────┐  │
│  │ 1. encode_request (IR → wire)                     │  │
│  │ 2. set_json_model (强制覆盖 model 字段)           │  │
│  │ 3. strip_anthropic_incompatible_fields            │  │
│  │ 4. enable_anthropic_prompt_caching                │  │
│  │ 5. HTTP POST + 指数退避重试                       │  │
│  │ 6. decode_response (wire → IR)                    │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
    │
    ▼ [provider-native format]
┌─────────────────────────────────────────────────────────┐
│  LLM Backend (vLLM / NIM / Ollama / OpenAI / Anthropic)  │
└─────────────────────────────────────────────────────────┘
```

---

## 二、协议层（switchyard-protocol）深度分析

### 2.1 LlmRequest 完整结构

**文件**: `crates/protocol/src/llm.rs`

```rust
pub struct LlmRequest {
    pub model: Option<String>,                    // 路由时可被替换
    pub instructions: Vec<InstructionBlock>,     // system/developer 指令
    pub messages: Vec<Message>,                  // 对话消息
    pub tools: Vec<ToolDefinition>,              // 工具定义
    pub tool_choice: Option<ToolChoice>,         // 工具选择策略
    pub sampling: SamplingParams,                // temperature/top_p/top_k
    pub output: OutputParams,                    // max_output_tokens/response_format
    pub reasoning: ReasoningParams,              // effort/raw
    pub stream: bool,                            // 是否流式
    pub extensions: ProviderExtensions,          // 供应商特有字段
    pub preservation: PreservationMetadata,      // 原始载荷保留
}
```

**设计要点**:
- `#[serde(default)]` 保证向前兼容，缺失字段不会反序列化失败
- `InstructionBlock` 将 system/developer 指令与对话消息分离，符合 Anthropic Messages API 风格
- `extensions: ProviderExtensions` 用 `Map<String, Value>` 保存非第一类字段，用于跨格式翻译时保留供应商特有字段

### 2.2 ContentBlock::Unknown 无损保留机制

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Reasoning { text: String, signature: Option<String>, details: Vec<Value> },
    Image { source: ImageSource },
    Audio { source: MediaSource },
    Video { source: MediaSource },
    File { source: FileSource },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Refusal { text: String },
    Unknown {
        provider: FormatId,  // 来源格式标识
        raw: Value,          // 完整原始块
    },
}
```

**关键设计**:
- `#[serde(tag = "type")]` 使用内部标签策略，序列化为 `{"type": "text", "text": "..."}`
- `Unknown` 变体保留无法归一化的供应商块（如 OpenAI 的 `web_search_call`、Anthropic 的 `server_tool_use`）
- `provider: FormatId` 标记来源，跨格式翻译时可选择丢弃或保留
- `raw: Value` 是完整 JSON，保证 100% 无损往返

### 2.3 PreservationMetadata 机制

```rust
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,   // 原始请求体
    pub responses: BTreeMap<FormatId, Value>,  // 原始响应体
}
```

**设计思路**:
- 翻译引擎的默认保留策略：同格式重放时直接返回存储的原始体，而非从 IR 重新编码
- 调用者修改 IR 后必须清除对应条目，否则重放的是旧数据
- 与 `Request::raw_request`（宿主可选保留）严格分离

### 2.4 流式类型与 ResponseAccumulator

**文件**: `crates/protocol/src/stream.rs`

```rust
pub enum LlmResponseChunk {
    MessageStart { id, model },
    TextDelta { index, text },
    ReasoningDelta { index, text },
    ReasoningDetailsDelta { index, details, text },
    ToolCallDelta { index, id, name, arguments_delta },
    Usage(Usage),
    MessageStop { reason },
    DecodeError { message },
    StreamError { message },
}
```

**ResponseAccumulator 折叠逻辑**:
- `MessageStart` / `Usage` / `MessageStop`：后覆盖前
- `TextDelta` / `ReasoningDelta`：字符串拼接
- `ToolCallDelta`：按 `index` 分组，`arguments` 拼接后 JSON 解析
- 最终输出顺序：reasoning → text → tool_calls（单输出）

**`into_stream` 反向转换**:
- `AggLlmResponse` → 合成 `LlmResponseChunk` 流
- 仅 text/reasoning/tool_call 有合成表示，其余丢弃（标注为 lossy）

---

## 三、协议翻译层（switchyard-translation）深度分析

### 3.1 TranslationEngine 结构

**文件**: `crates/translation/src/engine.rs`

```rust
pub struct TranslationEngine {
    registry: FormatRegistry,              // 缓冲编解码器注册表
    stream_registry: StreamCodecRegistry,  // 流式编解码器注册表
}
```

**核心方法**:
- `translate_request(source, target, body, policy)` — 请求翻译
- `translate_response(source, target, body, policy)` — 响应翻译
- `translate_event(state, source, target, event)` — 流式事件翻译
- `decode_stream_event(state, source, event)` — 解码并保留原始事件
- `encode_stream_event(state, target, event)` — 编码（同格式重放保留值）
- `finish_stream(state, target)` — 流结束时的收尾事件

### 3.2 FormatCodec trait

```rust
pub trait FormatCodec: Send + Sync {
    fn format(&self) -> FormatId;
    fn decode_request(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedRequest>;
    fn encode_request(&self, request: &LlmRequest, policy: &TranslationPolicy) -> Result<EncodedRequest>;
    fn decode_response(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedResponse>;
    fn encode_response(&self, response: &AggLlmResponse, policy: &TranslationPolicy) -> Result<EncodedResponse>;
}
```

**三个内建实现**:
- `OpenAiChatCodec` — OpenAI Chat Completions (`/v1/chat/completions`)
- `AnthropicMessagesCodec` — Anthropic Messages (`/v1/messages`)
- `OpenAiResponsesCodec` — OpenAI Responses (`/v1/responses`)

### 3.3 流式翻译状态机

```rust
pub struct StreamTranslationState {
    source: Option<FormatId>,
    target: Option<FormatId>,
    // 内部状态：部分 tool call 缓冲、reasoning 状态等
}
```

**`translate_event` 流程**:
1. `source_codec.decode_event(state, event)` → 规范化事件
2. `target_codec.encode_event(state, canonical)` → 目标格式事件
3. 返回 `Vec<Value>`（一对多映射）

**保留机制**:
- `decode_stream_event` 返回 `LlmResponseStreamEvent::preserved(source, raw, normalized)`
- `encode_stream_event` 检测 source==target 时直接重放 `raw`，否则用 `normalized` 重新编码

### 3.4 TranslationPolicy

```rust
pub struct TranslationPolicy {
    pub preserve_original: bool,       // 是否保留原始体
    pub deterministic_ids: bool,       // 是否生成确定性 ID（保证 tool_use_id 配对）
    pub preserve_streaming: bool,      // 是否保留流式事件
}
```

---

## 四、路由算法层（libsy）深度分析

### 4.1 Algorithm trait 与 Driver

**文件**: `crates/libsy/src/core/algorithm.rs`

```rust
#[async_trait]
pub trait Algorithm: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn route(self: Arc<Self>, driver: Driver, request: Request) -> Result<RoutingOutcome>;
    fn run_stream(self: Arc<Self>, request: Request) -> StepStream { ... }
}
```

**Driver 设计**:
```rust
pub struct Driver {
    step_tx: mpsc::Sender<Result<Step>>,
    algorithm: String,
}
```

- `call_model(request, models)` → 通过 `mpsc::channel(1)` 发送 `Step::CallModel`，等待 `oneshot` 返回
- 容量 1 保持背压，宿主消费后才继续
- `finish(result)` 发送 `Step::Done` 终止流

**Step 流**:
```rust
pub enum Step {
    CallModel(Box<CallModel>),  // 算法请求宿主执行 LLM 调用
    Done(Box<RoutingOutcome>),  // 路由完成
}
```

**`drive` 函数**:
```rust
pub async fn drive<F, Fut>(algorithm: Arc<dyn Algorithm>, request: Request, serve: F) -> Result<RoutingOutcome> {
    let stream = algorithm.run_stream(request);
    tokio::pin!(stream);
    let mut in_flight = futures::stream::FuturesUnordered::new();
    loop {
        tokio::select! {
            Some(result) = in_flight.next() => { ... }
            step = stream.next() => {
                match step {
                    None => break,
                    Some(item) => match item? {
                        Step::CallModel(call) => in_flight.push(serve(*call)),
                        Step::Done(outcome) => { final_outcome = Some(*outcome); break; }
                    }
                }
            },
        }
    }
    final_outcome.ok_or(LibsyError::MissingFinalResponse)
}
```

**并发特性**:
- `FuturesUnordered` 并发服务多个 `CallModel`，支持 hedging/fan-out
- 算法可一次发出多个调用，第一个响应者胜出

### 4.2 FallThrough 级联骨架

**文件**: `crates/libsy/src/algorithms/fall_through.rs`

```rust
pub struct FallThrough<S = ()> {
    name: String,
    processors: Vec<Arc<dyn Processor<S>>>,
    classifiers: Vec<Arc<dyn Classifier<S>>>,
    targets: Vec<ModelId>,
    session_states: Option<Arc<SessionStates<S>>>,
    cleanup_started: Once,
}
```

**执行流程**:
1. **Processor 链**: 每个 processor 看到 `Event::Request { request, driver }`，可修改请求或调用模型
2. **Classifier 链**: 顺序尝试，第一个给出 `Scores` 的胜出（`argmax`）
3. **DefaultTarget**: 所有 classifier 都 abstain 时使用默认目标
4. **Post-decision replay**: 每个 processor 看到 `Event::Decision { request, selected_model_id }`，可绑定状态或改写请求

**SessionState 管理**:
- `SESSION_STATE_TTL = 1 hour` — 会话状态过期时间
- `SESSION_CLEANUP_INTERVAL = 1 hour` — 清理任务间隔
- `remove_inactive_sessions` 通过 `Arc::strong_count > 1` 判断是否有活跃请求持有

### 4.3 LLM Classifier（三模式）

**文件**: `crates/libsy/src/algorithms/llm_class.rs`

```rust
pub enum LlmClassifierConfig {
    Capability { judge_target, efficient_target, capable_target, config },
    Escalation { judge_target, efficient_target, capable_target, contract, config, max_output_tokens },
    Custom { judge_target, targets, default_target, config },
}
```

**Capability 模式**:
- Judge 模型输出 `{crux, primary_rule, capability_boundary, p_solve}` 判决
- `capability_boundary` ∈ {supported, uncertain, unsupported, unmatched}
- 阈值公式：`threshold = base_threshold + boundary_steps * threshold_step`
  - supported → 0 步
  - uncertain/unmatched → 1 步
  - unsupported → 2 步
- `p_solve >= threshold` → efficient，否则 → capable

**`trim_messages` 窗口算法**:
```rust
fn trim_messages(messages: &[Message], recent_turn_window: usize) -> Vec<Message> {
    // 1. 保留所有 instruction (System/Developer)
    // 2. 保留第一个 User 消息（opening task）
    // 3. 保留最近 N 轮（从尾部计数）
}
```

**`window_start` 工具配对算法**:
- 从 newest-to-oldest 扫描，维护 `unpaired: HashSet<&str>`
- `ToolResult` → 插入 `tool_call_id`
- `ToolCall` → 移除 `id`
- 当 `unpaired.is_empty()` 且 `start <= counted` 时返回，保证每个 result 都有对应的 call

**Escalation 模式**:
- 先调用 efficient，再用 judge 评判
- 连续 N 次 escalate 判决后 latch 到 capable
- 支持 `recent_turn_window` 控制 judge 可见的对话轮数

**Custom 模式**:
- 用户提供 JSON Schema + TargetSelector 策略
- Judge 输出经 Schema 验证后，用 JSON Pointer 提取目标名

### 4.4 Advisor Gate（执行者 + 审查者）

**文件**: `crates/libsy/src/algorithms/advisor_gate.rs`

```rust
pub struct AdvisorGate {
    executor: ModelId,     // 执行者（回答每个客户端可见轮次）
    advisor: ModelId,      // 审查者（审查终端轮次）
    config: AdvisorGateConfig,
    trigger: CompiledTrigger,
    verdict_re: regex::Regex,
    state: Mutex<GateState>,
}
```

**工作流**:
1. Executor 回答每个轮次
2. 终端轮次（无 tool_use 或匹配 pattern）被缓冲
3. Advisor 审查：`APPROVE` 释放缓冲轮次，`REDO` 将计划反馈给 executor 重新生成
4. 每个 scope（bench/session/instance）最多 `max_reviews` 次审查
5. Advisor 故障时 `fail_open`（默认 APPROVE）

**预算作用域优先级**:
```rust
enum ScopeKey {
    Instance,        // 实例级（无头客户端）
    Client(String),  // 会话级
    Session(String), // 基准级（proxy_x_session_id）
}
```

**关键常量**:
- `MAX_FAILED_CONSULTS = 3` — 失败咨询上限
- `MAX_TRACKED_SCOPES = 1024` — 跟踪的作用域上限
- `transcript_max_chars = 200_000` — 转录文本上限

### 4.5 其他算法

| 算法 | 文件 | 核心逻辑 |
|------|------|----------|
| `Noop` | `noop.rs` | 不调用模型，直接返回 OK |
| `Passthrough` | `passthrough.rs` | 单目标直通，支持 subagents 粘性 |
| `Random` | `rand.rs` | 加权随机分流（A/B 测试），可选 seed |
| `StageRouter` | `stage.rs` | 信号驱动的阶段路由（ToolSignalProcessor + StageClassifier） |
| `CompositeRouter` | `composite.rs` | 组合路由（LLM 判断 + Stage） |
| `SubagentRouter` | `subagent.rs` | 子 Agent 路由（AffinityRouter 粘性） |

**AffinityRouter**:
```rust
pub struct AffinityRouter {
    assignments: HashMap<RoutingIdentity, ModelId>,
    release_on_user_turn: bool,
    message_hash_fallback: bool,
}

pub enum RoutingIdentity {
    Session(String),                      // 根请求按 session 粘性
    Subagent { session: String, agent: String },  // 子 Agent 按 session+agent 粘性
}
```

**ClassifyTrigger**:
- `EveryRequest` — 每个请求都分类（无粘性）
- `NewSession` — 新 session 时分类
- `UserTurn` — 用户轮次时分类

---

## 五、LLM 客户端层（libsy-llm-client）深度分析

### 5.1 TranslatingLlmClient 结构

**文件**: `crates/libsy-llm-client/src/client.rs`

```rust
pub struct TranslatingLlmClient {
    model_to_config: HashMap<ModelId, ModelConfig>,
    client: reqwest::Client,
    forward_auth_client: reqwest::Client,  // 禁用重定向，防止凭证泄露
}
```

**双 Client 设计**:
- `client` — 普通请求，允许重定向
- `forward_auth_client` — `redirect(Policy::none())`，用于 forward_auth 模式，防止凭证泄露到第三方

### 5.2 请求编码与清洗

**`send_encoded` 流程**:
1. `encode_request(&llm_request, wire_format)` — IR → wire
2. `set_json_model(&mut body, model)` — 强制覆盖 model 字段
3. `strip_anthropic_incompatible_fields` — 移除 `reasoning_effort`、`context_management`
4. `strip_unsigned_thinking_blocks` — 移除无签名的 thinking 块（Bedrock 需要）
5. `merge_extra_body` — 合并目标默认值（不覆盖调用者提供的）
6. `enable_anthropic_prompt_caching` — 在最后消息添加 `cache_control: {type: "ephemeral"}`（最多 4 个）
7. `ensure_openai_stream_usage` — 流式请求添加 `stream_options: {include_usage: true}`

### 5.3 重试退避策略

```rust
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
```

**`retry_delay` 计算**:
```rust
fn retry_delay(retry_number: u64, retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or_else(|| {
        let multiplier = 1_u32 << retry_number.min(3);  // 1, 2, 4, 8
        INITIAL_RETRY_DELAY.saturating_mul(multiplier).min(MAX_RETRY_BACKOFF)
    })
}
```

- 第 1 次重试：250ms
- 第 2 次重试：500ms
- 第 3 次重试：1000ms
- 第 4+ 次重试：2000ms（封顶）
- 尊重 `Retry-After` 头，但不超过 60s

**可重试条件**:
- `Transport` / `Timeout` 错误 → 可重试
- `UpstreamHttp` 状态码 → `is_retryable_http_status` 判断（429, 500, 502, 503, 504）
- 400 + `is_context_overflow` → `ContextWindowExceeded`（不可重试）

### 5.4 400 错误分类

```rust
fn is_context_overflow(&self, body: &str) -> bool {
    // Anthropic: "prompt is too long"
    // OpenAI: "maximum context length"
    // 通过供应商规则匹配
}
```

### 5.5 RunObserver

```rust
pub enum RunObservation {
    AnswerCall(LlmCallObservation),  // 最终回答调用
    LlmCall(LlmCallObservation),     // 分类器/Judge 调用
    RoutingOverhead(Duration),       // 路由开销
}

pub type RunObserver = Arc<dyn Fn(RunObservation) + Send + Sync>;
```

宿主通过 observer 接收调用事件，用于统计/日志，不干扰算法逻辑。

---

## 六、HTTP 服务器层（switchyard-server）深度分析

### 6.1 Axum 路由构建

**文件**: `crates/switchyard-server/src/lib.rs`

```rust
pub fn build_switchyard_router(state: ServerState) -> Router {
    let mut router = Router::new()
        .route("/v1/chat/completions", post(openai_chat_completions))
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/responses", post(openai_responses))
        .route("/v1/decision", post(decision))
        .route("/v1/messages/count_tokens", post(anthropic_count_tokens))
        .route("/v1/models", get(models))
        .route("/v1/stats", get(get_stats))
        .route("/v1/stats/reset", post(reset_stats))
        .route("/metrics", get(prometheus_metrics))
        .route("/health", get(health));
    if state.routing_log.is_some() {
        router = router.route("/v1/routing/session-stats", get(get_session_stats));
    }
    router
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(DEFAULT_MAX_REQUEST_BODY_BYTES))  // 32MB
        .layer(axum::middleware::from_fn(stamp_request_start))
        .with_state(state)
}
```

### 6.2 请求处理流

**`handle_endpoint_inner` 流程**:
1. `metadata_from_headers(headers)` — 提取 W3C Trace Context + 自定义头
2. `llm_json_body(body)` — 验证 JSON 对象
3. `resolve_route(state, metadata, body, wire_format)` — 解码 + 路由解析
4. `route.execute(request, Some(observer)).await` — 执行算法
5. `into_http_response(response, wire_format, ...)` — 编码回 wire format
6. `attach_routing_headers(&mut response, served_model)` — 添加 `x-model-router-selected-model`
7. `render_error_response(response, wire_format)` — 错误按 wire_format 渲染

### 6.3 错误处理

**`client_error` 映射**:
```rust
fn client_error(error: &LlmClientError) -> Response {
    match error {
        LlmClientError::InvalidRequest { message } => error_response(400, ...),
        LlmClientError::ContextWindowExceeded { message, .. } => error_response(400, ...),
        LlmClientError::UpstreamHttp { status, body } => error_response(*status, ...),
        LlmClientError::Timeout { source } => error_response(504, ...),
        LlmClientError::Transport { source } => error_response(502, ...),
        LlmClientError::ResponseTranslation(message) => error_response(502, ...),
        _ => error_response(500, ...),
    }
}
```

**错误形状按 wire_format 渲染**:
```rust
fn into_response(self, wire_format: WireFormat) -> Response {
    let body = match wire_format {
        WireFormat::AnthropicMessages => json!({
            "type": "error",
            "error": { "type": anthropic_error_type(self.status), "message": self.message }
        }),
        WireFormat::OpenAiChat | WireFormat::OpenAiResponses => json!({
            "error": { "message": self.message, "type": self.error_type, "code": self.code }
        }),
    };
    ...
}
```

### 6.4 优雅关闭

```rust
async fn serve_until_shutdown(
    server: impl Future<Output = std::io::Result<()>>,
    handle: axum_server::Handle<SocketAddr>,
    timeout: Duration,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> ServerResult<()> {
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result.map_err(server_io_error),
        _ = shutdown => {
            handle.graceful_shutdown(Some(timeout));  // 30s 默认
            server.await.map_err(server_io_error)
        }
    }
}
```

### 6.5 请求日志

**`RequestLogContext`**:
- 记录 `started` 时间戳（`Instant`）
- 终端响应时计算 `handling_duration_ms`
- 根据状态码选择日志级别：5xx → ERROR, 4xx → WARN, 其他 → INFO
- 通过 `tracing::event!` 输出结构化日志

---

## 七、技能蒸馏层（switchyard-skill-distillation）深度分析

### 7.1 数据模型

**文件**: `crates/switchyard-skill-distillation/src/model.rs`

```rust
pub struct Trajectory {
    pub schema_version: u16,              // 当前 SCHEMA_VERSION = 1
    pub id: SkillEvidenceId,              // 证据 ID
    pub task: TaskDescriptor,             // 任务描述
    pub execution: ExecutionMetadata,     // 执行元数据（harness/model/时间）
    pub source: TrajectorySourceInfo,     // 来源信息
    pub events: Vec<TrajectoryEvent>,     // 有序事件
    pub outcome: Option<TrajectoryOutcome>, // 结果（label/score/error）
    pub metadata: Metadata,               // 扩展元数据
}

pub enum TrajectoryEventKind {
    Message, ToolCall, ToolResult, Observation, Error, FinalOutput,
}

pub struct SkillCandidate {
    pub schema_version: u16,
    pub namespace: SkillNamespace,
    pub version: SkillVersionId,
    pub skill_md: String,                 // 技能内容（Markdown）
    pub provenance: SkillProvenance,      // 来源轨迹
    pub validation: Option<ValidationReport>, // 验证报告
    pub metadata: Metadata,
}
```

### 7.2 端口抽象

**文件**: `crates/switchyard-skill-distillation/src/ports.rs`

```rust
#[async_trait]
pub trait TrajectorySource: Send + Sync {
    async fn load(&self, namespace: &SkillNamespace) -> Result<Vec<Trajectory>>;
}

#[async_trait]
pub trait SkillDistiller: Send + Sync {
    async fn distill(&self, request: &DistillationRequest) -> Result<SkillCandidate>;
}

#[async_trait]
pub trait SkillValidator: Send + Sync {
    async fn validate(&self, candidate: &SkillCandidate, evaluation: &[Trajectory]) -> Result<ValidationReport>;
}

#[async_trait]
pub trait SkillStore: Send + Sync {
    async fn active(&self, namespace: &SkillNamespace) -> Result<Option<SkillCandidate>>;
    async fn save_candidate(&self, candidate: &SkillCandidate) -> Result<()>;
    async fn activate(&self, namespace: &SkillNamespace, version: &SkillVersionId) -> Result<ActivationRecord>;
    async fn rollback(&self, namespace: &SkillNamespace) -> Result<ActivationRecord>;
}
```

**设计思路**:
- 端口与实现分离，允许替换轨迹源（本地运行器/基准测试导入器）、蒸馏器（LLM/规则）、验证器、存储
- `SkillProvenance` 记录技能的来源轨迹，实现可追溯性
- `ActivationRecord` 记录技能激活历史，支持回滚

### 7.3 验证机制

```rust
pub struct ValidationReport {
    pub status: ValidationStatus,         // Passed / Failed / NeedsReview
    pub checks: Vec<ValidationCheck>,     // 检查项
    pub metrics: BTreeMap<String, f64>,   // 聚合指标
    pub notes: Vec<String>,               // 人工审阅笔记
    pub evaluated_at: String,
}
```

**`validate_candidate` 校验**:
- 候选 namespace 必须匹配请求 namespace
- `provenance.source_evidence_ids` 必须全部在请求轨迹内
- `parent_version` 必须匹配 `base_skill.version`

---

## 八、预填充路由层（prefill-router）深度分析

### 8.1 PrefillForward trait

**文件**: `crates/prefill-router/src/lib.rs`

```rust
pub trait PrefillForward: Send {
    fn forward(&mut self, request: &ForwardRequest) -> Result<ForwardOutput>;
    fn unload(&mut self) -> Result<()>;
}
```

**设计思路**:
- trait 不含 Python 类型，允许 Candle 等纯 Rust 实现替换
- `unload()` 显式释放模型资源

### 8.2 TransformersForward

**文件**: `crates/prefill-router/src/transformers.rs`

```rust
pub struct TransformersForward {
    model: Py<PyAny>,      // HuggingFace Transformers 模型
    tokenizer: Py<PyAny>,  // 分词器
    device: String,        // cpu / cuda:0
}
```

**通过 PyO3 嵌入 Python**:
- 复用 HuggingFace Transformers 的 forward 路径
- `LayerSelection::UpperHalf` 提取上半层隐藏状态（与 NVIDIA LLM Router 参考实现一致）

### 8.3 ForwardRequest / ForwardOutput

```rust
pub struct ForwardRequest {
    pub prompts: Vec<String>,
    pub chat_template_kwargs: serde_json::Map<String, serde_json::Value>,
    pub layers: LayerSelection,      // UpperHalf / All / Selected(Vec<usize>)
    pub pooling: Vec<Pooling>,       // Last / Mean
    pub batch_size: usize,
    pub max_length: usize,
}

pub struct ForwardOutput {
    hidden_last: BTreeMap<usize, Vec<Vec<f32>>>,  // 每层 → 每 prompt → 隐藏向量
    hidden_mean: BTreeMap<usize, Vec<Vec<f32>>>,
    n_layers: usize,
    hidden_dim: usize,
}
```

**`LayerSelection::UpperHalf`**:
- 索引范围 `[n_layers/2, n_layers)`
- 与 NVIDIA LLM Router 参考实现一致，提取模型后半段的隐藏状态作为路由特征

---

## 九、可观测性深度分析

### 9.1 Prometheus 指标体系

**文件**: `crates/switchyard-server/src/metrics.rs`

| 指标 | 类型 | 描述 |
|------|------|------|
| `switchyard.upstream_attempts` | Counter | 上游尝试次数（按 outcome/code 分类） |
| `switchyard.client_responses` | Counter | 客户端响应（success/retryable_error/other_error） |
| `switchyard.model_call_latency_ms` | Histogram | 模型调用延迟 |
| `switchyard.total_latency_ms` | Histogram | 端到端延迟 |
| `switchyard.routing_overhead_ms` | Histogram | 路由开销 |
| `switchyard.router_retry_recovered` | Counter | 路由重试恢复次数 |
| `switchyard.build_info` | Gauge | 构建信息（版本号） |

**直方图桶边界**:
```rust
const LLM_LATENCY_BUCKETS_MS: &[f64] = &[
    0.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0,
    1000.0, 2500.0, 5000.0, 7500.0, 10_000.0, 15_000.0, 30_000.0, 60_000.0, 120_000.0, 300_000.0,
];
const ROUTING_OVERHEAD_BUCKETS_MS: &[f64] = &[
    0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0,
];
```

**`seed_outcome_metrics`**: 预注册所有可能的状态码，便于仪表盘在首次命中前就显示指标。

### 9.2 OpenTelemetry 集成

**文件**: `crates/switchyard-server/src/observability.rs`

```rust
fn initialize() -> Result<Observability, String> {
    metrics::registry()?;
    let tracer_provider = otlp_enabled("TRACES").then(build_tracer_provider).transpose()?;
    let filter = log_filter()?;
    let format = tracing_subscriber::fmt::layer().with_ansi(false).with_writer(std::io::stderr).with_filter(filter);
    if let Some(provider) = &tracer_provider {
        let tracer = provider.tracer("switchyard");
        tracing_subscriber::registry()
            .with(format)
            .with(tracing_opentelemetry::layer().with_tracer(tracer).with_filter(log_filter()?))
            .try_init()
            .map_err(...)?;
    }
    ...
}
```

**OTLP 启用条件**:
- `OTEL_SDK_DISABLED` 不为 true
- `OTEL_EXPORTER_OTLP_ENDPOINT` 或 `OTEL_EXPORTER_OTLP_{SIGNAL}_ENDPOINT` 已设置
- `OTEL_{SIGNAL}_EXPORTER` 不包含 `otlp`

### 9.3 W3C Trace Context 传播

```rust
pub(crate) fn request_span(headers: &HeaderMap) -> tracing::Span {
    let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
    let span = tracing::info_span!(
        target: "switchyard_server",
        "switchyard.request",
        otel.kind = "server",
        openinference.span.kind = "CHAIN",
    );
    let _ = span.set_parent(parent);
    span
}

struct HeaderExtractor<'a>(&'a HeaderMap);
impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|name| name.as_str()).collect()
    }
}
```

**支持**:
- `traceparent` 头（W3C 标准）
- `tracestate` 头（供应商特定）
- 通过 `TraceContextPropagator` 提取父上下文

### 9.4 Tracing Span 语义约定

**OpenInference 语义约定**:
```rust
#[tracing::instrument(
    target = "libsy",
    name = "libsy.llm_call",
    skip_all,
    fields(
        algorithm = self.algorithm,
        selected_model = %models.first().map(ModelId::as_str).unwrap_or("NoTargets"),
        openinference.span.kind = "CHAIN",
        outcome = tracing::field::Empty,
        error = tracing::field::Empty,
        input_tokens = tracing::field::Empty,
        output_tokens = tracing::field::Empty,
        total_tokens = tracing::field::Empty,
        reasoning_tokens = tracing::field::Empty,
    )
)]
```

- `openinference.span.kind = "CHAIN"` 标识为链式调用
- 兼容 OpenTelemetry Collector + OpenInference 生态

---

## 十、运行器层（switchyard-runner）深度分析

### 10.1 TOML 配置系统

**文件**: `crates/switchyard-runner/src/config.rs`

```rust
pub(crate) struct DeploymentConfig {
    schema_version: u32,
    llm_clients: BTreeMap<String, LlmClientConfig>,
    targets: BTreeMap<String, TargetConfig>,
    routes: BTreeMap<String, RouteConfig>,
}
```

### 10.2 AlgorithmSpec 多态枚举

```rust
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlgorithmSpec {
    Noop {},
    Random { targets, weights, seed },
    Passthrough { target, subagents },
    LlmClassifier { config: LlmClassifierRouteConfig },
    StageRouter { tiers, picker, classifier, subagents },
    Composite { classifier, stage, subagents },
    Advisor { executor_target, advisor_target, ... },
}
```

**设计思路**:
- `tag = "type"` 实现多态反序列化
- `deny_unknown_fields` 拒绝未知字段，避免配置错误
- `rename_all = "snake_case"` 匹配 TOML 风格

### 10.3 配置构建

```rust
impl DeploymentConfig {
    fn build(self) -> RunnerResult<Runner> {
        // 1. 校验 schema_version
        // 2. 构建所有 llm_clients
        // 3. 构建 targets 映射
        // 4. 构建每个 route 的 algorithm + clients + capabilities
        // 5. 返回 Runner
    }
}
```

---

## 十一、Python 绑定层（switchyard-py）深度分析

### 11.1 PyO3 绑定设计

**文件**: `crates/switchyard-py/src/lib.rs`

```rust
#[pymodule]
fn _switchyard_rust(module: &Bound<'_, PyModule>) -> PyResult<()> {
    errors::register(module)?;
    libsy_bindings::register(module)?;
    server_bindings::register(module)?;
    Ok(())
}
```

### 11.2 类型转换

**文件**: `crates/switchyard-py/src/py_serde.rs`

```rust
pub fn from_python<'py, T: Deserialize<'py>>(obj: &'py Bound<'py, PyAny>) -> PyResult<T> {
    let json = python_to_json(obj)?;
    T::deserialize(json).map_err(|e| PyValueError::new_err(e.to_string()))
}

pub fn to_python<'py, T: Serialize>(py: Python<'py>, value: &T) -> PyResult<Bound<'py, PyAny>> {
    let json = serde_json::to_value(value)?;
    json_to_python(py, &json)
}
```

**通过 serde JSON 作为 Python ↔ Rust 的中介格式**，避免为每个类型手写转换。

### 11.3 异步绑定

```rust
#[pyfunction]
fn run_algorithm<'py>(py: Python<'py>, algo: &Bound<'py, PyAny>, request: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    pyo3_asyncio::tokio::future_into_py(py, async move {
        // 驱动算法 step stream
    })
}
```

---

## 十二、对 laew 工程的深度借鉴建议

### 12.1 Rust 实现方面

#### P0（立即借鉴）

**1. 引入 `protocol` crate 和 IR**

**laew 现状**: `llm/anthropic.rs` 和 `llm/openai.rs` 直接操作线格式，协议差异渗透到 agent 循环。

**借鉴**:
```rust
// 新建 crates/protocol/src/llm.rs
pub struct LlmRequest {
    pub model: Option<String>,
    pub instructions: Vec<InstructionBlock>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub sampling: SamplingParams,
    pub output: OutputParams,
    pub reasoning: ReasoningParams,
    pub stream: bool,
    pub extensions: ProviderExtensions,
    pub preservation: PreservationMetadata,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Reasoning { text: String, signature: Option<String>, details: Vec<Value> },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Unknown { provider: FormatId, raw: Value },  // 关键：无损保留
}
```

**收益**: agent 循环完全不接触协议细节，新增协议只需实现 `FormatCodec`。

**2. 引入 `translation` crate**

**借鉴**:
```rust
pub struct TranslationEngine {
    registry: FormatRegistry,
    stream_registry: StreamCodecRegistry,
}

impl TranslationEngine {
    pub fn translate_request(&self, source, target, body, policy) -> Result<TranslationOutput> {
        let decoded = self.registry.codec(source)?.decode_request(body, policy)?;
        let encoded = self.registry.codec(target)?.encode_request(&decoded.request, policy)?;
        Ok(TranslationOutput { body: encoded.body, diagnostics: ... })
    }
}
```

**收益**: TUI 流式输出时可在翻译层直接转换 SSE 事件。

#### P1（中期借鉴）

**3. 将多 Agent 架构抽象为 `Agent` trait**

**laew 现状**: `YoloRunner` 硬编码双 Agent 架构。

**借鉴**:
```rust
#[async_trait]
pub trait Agent: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn execute(self: Arc<Self>, driver: Driver, request: Request) -> Result<AgentOutcome>;
}

pub struct Driver {
    step_tx: mpsc::Sender<Result<Step>>,
    agent: String,
}

pub enum Step {
    CallModel(Box<CallModel>),
    Delegate(Box<delegateRequest>),
    Done(Box<AgentOutcome>),
}
```

**收益**: Yolo/Plan/Main-Work/SubAgent-Work/QC/SessionContext 可配置化组合。

**4. 引入 `FallThrough` 级联模式**

**借鉴**:
```rust
pub struct FallThrough<S = ()> {
    processors: Vec<Arc<dyn Processor<S>>>,
    classifiers: Vec<Arc<dyn Classifier<S>>>,
    targets: Vec<ModelId>,
    session_states: Option<Arc<SessionStates<S>>>,
}
```

**收益**: 将 Yolo 分类、Plan 决策、Main-Work 编排抽象为 Classifier 链，第一个给出高置信度的胜出。

#### P2（长期借鉴）

**5. 引入 `Algorithm` trait 的 Step 流**

**借鉴**:
```rust
pub enum Step {
    CallModel(Box<CallModel>),
    Done(Box<RoutingOutcome>),
}

pub async fn drive<F, Fut>(algorithm: Arc<dyn Algorithm>, request: Request, serve: F) -> Result<RoutingOutcome> {
    let stream = algorithm.run_stream(request);
    tokio::pin!(stream);
    let mut in_flight = futures::stream::FuturesUnordered::new();
    loop {
        tokio::select! {
            Some(result) = in_flight.next() => { ... }
            step = stream.next() => { ... }
        }
    }
}
```

**收益**: SubAgent-Work 可并发执行，Quality-Check 可与下一个 SubAgent 流水线化。

### 12.2 协议翻译方面

#### P0（立即借鉴）

**1. `ContentBlock::Unknown` 无损保留**

**laew 现状**: 新字段在翻译时丢失。

**借鉴**:
```rust
Unknown {
    provider: FormatId,
    raw: Value,  // 保留原始块
}
```

**收益**: 新字段不会丢失，跨格式翻译时可选择丢弃或保留。

**2. `PreservationMetadata` 机制**

**借鉴**:
```rust
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,
    pub responses: BTreeMap<FormatId, Value>,
}
```

**收益**: 协议转换失败时可回退到原始体重试。

#### P1（中期借鉴）

**3. 流式事件翻译**

**借鉴**:
```rust
pub fn translate_event(&self, state, source, target, event) -> Result<Vec<Value>> {
    let canonical = source_codec.decode_event(state, event);
    Ok(canonical.into_iter().flat_map(|e| target_codec.encode_event(state, e)).collect())
}
```

**收益**: TUI 流式输出时可在翻译层直接转换 SSE 事件。

**4. `ResponseAccumulator` 流式折叠**

**借鉴**:
```rust
pub struct ResponseAccumulator {
    id: Option<String>,
    model: Option<String>,
    text: String,
    reasoning: Option<String>,
    reasoning_details: Vec<Value>,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    usage: Usage,
    stop_reason: Option<StopReason>,
}
```

**收益**: 统一处理流式响应的聚合逻辑，支持 tool call 参数拼接。

### 12.3 路由算法方面

#### P0（立即借鉴）

**1. `p_solve` 置信度输出**

**laew 现状**: Yolo 三档分类（simple/medium/hard）无置信度。

**借鉴**:
```rust
pub struct TaskClassifierVerdict {
    pub crux: String,
    pub primary_rule: String,
    pub capability_boundary: String,  // supported/uncertain/unsupported/unmatched
    pub p_solve: f64,                  // 解决概率
}
```

**收益**: Yolo 输出置信度，低置信度时回退到用户确认。

**2. `recent_turn_window` 对话窗口控制**

**借鉴**:
```rust
fn trim_messages(messages: &[Message], recent_turn_window: usize) -> Vec<Message> {
    // 保留 instruction + opening task + 最近 N 轮
}
```

**收益**: 控制 Yolo 可见的对话历史，避免上下文溢出。

#### P1（中期借鉴）

**3. `GateTrigger` 机制**

**借鉴**:
```rust
pub enum GateTrigger {
    NoToolCall,        // 无 tool_use 的终端轮次
    Pattern(String),   // 正则匹配
}
```

**收益**: Quality-Check 只在特定条件（如无 tool_use 的终端轮次）触发，减少不必要的 QC 调用。

**4. `max_reviews` 预算**

**借鉴**:
```rust
pub struct AdvisorGateConfig {
    pub max_reviews: u32,          // 每个 scope 最多审查次数
    pub fail_open: bool,           // 审查失败时默认通过
}
```

**收益**: 防止 QC 无限重试，QC 失败时默认通过。

**5. `AffinityRouter` 粘性路由**

**借鉴**:
```rust
pub struct AffinityRouter {
    assignments: HashMap<RoutingIdentity, ModelId>,
    release_on_user_turn: bool,
}
```

**收益**: session 级别的粘性路由，同一 session 的请求路由到同一模型，提高缓存命中率。

#### P2（长期借鉴）

**6. `StageRouter` 信号驱动路由**

**借鉴**:
```rust
pub struct ToolSignalProcessor {
    recent_window: usize,
}
```

**收益**: SubAgent-Work 执行后，根据工具调用结果信号决定下一步，无需额外 LLM 调用。

**7. `AdvisorGate` 执行者 + 审查者模式**

**借鉴**:
```rust
pub struct AdvisorGate {
    executor: ModelId,   // 执行者
    advisor: ModelId,    // 审查者
    config: AdvisorGateConfig,
}
```

**收益**: 与 laew 的 Quality-Check Agent 功能相似，但更精细化（预算作用域、stall 检测）。

### 12.4 可观测性方面

#### P0（立即借鉴）

**1. Prometheus 指标体系**

**借鉴**:
```rust
pub fn initialize() -> Result<Metrics, String> {
    let registry = Registry::new();
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .build()
        .map_err(...)?;
    ...
}
```

**收益**: 暴露 `/metrics` 端点，监控 LLM 调用延迟、路由开销、错误率。

#### P1（中期借鉴）

**2. W3C Trace Context 传播**

**借鉴**:
```rust
pub(crate) fn request_span(headers: &HeaderMap) -> tracing::Span {
    let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
    let span = tracing::info_span!(...);
    let _ = span.set_parent(parent);
    span
}
```

**收益**: 在 TUI 中注入 trace context，便于与外部可观测性平台集成。

**3. OpenInference 语义约定**

**借鉴**:
```rust
#[tracing::instrument(
    fields(
        openinference.span.kind = "CHAIN",
        outcome = tracing::field::Empty,
        input_tokens = tracing::field::Empty,
        output_tokens = tracing::field::Empty,
    )
)]
```

**收益**: 兼容 OpenTelemetry Collector + OpenInference 生态。

### 12.5 配置系统方面

#### P1（中期借鉴）

**1. TOML 配置 + serde 反序列化**

**借鉴**:
```rust
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlgorithmSpec {
    StageRouter { tiers, picker, classifier, subagents },
    LlmClassifier { config },
    ...
}
```

**收益**: 将多 Agent 架构配置化，通过 TOML 选择算法组合。

**2. `deny_unknown_fields` 严格校验**

**收益**: 拒绝未知字段，避免配置错误。

---

## 十三、总结

Switchyard 是 NVIDIA 在 LLM 流量代理领域的 Rust 实践，其核心优势在于：

1. **分层架构清晰**: protocol/translation/libsy/llm-client/server/runner 六层分离，每层职责单一，通过 trait 抽象层间接口。
2. **Provider-neutral IR**: `LlmRequest`/`LlmResponse` IR 隔离协议差异，`ContentBlock::Unknown` 变体保证无损往返，`PreservationMetadata` 保留原始体用于同格式重放。
3. **可组合路由算法**: `Algorithm` trait + `FallThrough` 级联，支持 7 种算法自由组合，`Driver` + `Step` 流实现并发 hedging/fan-out。
4. **生产级可观测性**: Prometheus + OpenTelemetry 双轨，W3C Trace Context 传播，OpenInference 语义约定。
5. **配置驱动**: TOML 配置 + serde 反序列化，算法选择无需改代码。
6. **技能蒸馏**: 端口与实现分离，Trajectory/SkillCandidate 数据模型支持可追溯的技能生成与验证。
7. **预填充路由**: `PrefillForward` trait + Transformers 嵌入，提取隐藏状态作为路由特征。

对 laew 的启示：
- **短期（P0）**: 引入 `protocol` crate 和 IR，简化协议差异处理；引入 `ContentBlock::Unknown` 无损保留；引入 Prometheus 指标。
- **中期（P1）**: 将多 Agent 架构抽象为 `Agent` trait + 级联编排；引入 `GateTrigger` 和 `max_reviews` 优化 QC；引入 W3C Trace Context 传播。
- **长期（P2）**: 引入 `Algorithm` trait 的 Step 流实现并发 SubAgent；引入信号驱动路由；引入 Advisor Gate 精细化 QC。

Switchyard 的代码质量、文档完整度和工程成熟度都较高，是 laew 在 Rust 实现、协议翻译、路由算法三个方向上的重要参考。
