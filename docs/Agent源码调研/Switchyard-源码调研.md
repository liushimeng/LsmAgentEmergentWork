# Switchyard 源码深度调研

> **项目**: Switchyard — NVIDIA 开发的 Rust LLM 流量代理与库
> **仓库**: https://github.com/NVIDIA-NeMo/Switchyard
> **版本**: v0.2.0 (pre-alpha)
> **许可证**: Apache-2.0
> **调研日期**: 2026-09-05

---

## 一、工程结构

### 1.1 Workspace 组织

Switchyard 采用 Rust Workspace 组织，根目录 `Cargo.toml` 声明了 10 个成员 crate：

```toml
[workspace]
resolver = "3"
members = [
    "crates/libsy",                    # 核心路由算法库
    "crates/libsy-llm-client",         # LLM HTTP 客户端
    "crates/prefill-router",           # 预填充特征提取
    "crates/switchyard-py",            # Python PyO3 绑定
    "crates/protocol",                 # Provider-neutral 协议类型
    "crates/switchyard-runner",        # 配置驱动的运行器
    "crates/switchyard-server",        # HTTP 服务器
    "crates/switchyard-skill-distillation",  # 技能蒸馏
    "crates/switchyard-soak",          # Soak 测试
    "crates/switchyard-translation",   # 协议翻译引擎
]
```

**设计思路**：
- **协议层(protocol)**、**翻译层(translation)**、**算法层(libsy)**、**客户端层(llm-client)**、**服务器层(server)**、**运行器层(runner)** 六层分离
- 每层通过 `Cargo.toml` 的 `[workspace.dependencies]` 统一版本管理
- 公共依赖：tokio、serde、reqwest、tracing、tracing-opentelemetry

### 1.2 各 Crate 职责

| Crate | 职责 | 依赖关系 |
|-------|------|----------|
| `protocol` | Provider-neutral 请求/响应/流式类型 | 无内部依赖 |
| `translation` | OpenAI/Anthropic 线格式 ↔ IR 编解码 | protocol |
| `libsy` | 路由算法 trait + 7 种算法实现 | protocol |
| `libsy-llm-client` | 翻译后 HTTP 客户端 | protocol, translation |
| `switchyard-runner` | TOML 配置加载 + 路由表构建 | libsy, protocol, translation, llm-client |
| `switchyard-server` | Axum HTTP 服务器 | runner, translation, libsy |
| `switchyard-py` | PyO3 Python 绑定 | libsy, server |
| `prefill-router` | 预填充特征提取(Transformers 嵌入) | 独立 |
| `switchyard-skill-distillation` | 技能蒸馏数据模型 | 独立 |
| `switchyard-soak` | 浸泡测试 | 独立 |

### 1.3 Python 绑定

Python 绑定位于 `crates/switchyard-py/`，通过 `pyproject.toml` 使用 maturin 构建：

```rust
#[pymodule]
fn _switchyard_rust(module: &Bound<'_, PyModule>) -> PyResult<()> {
    errors::register(module)?;
    libsy_bindings::register(module)?;
    server_bindings::register(module)?;
    Ok(())
}
```

提供 `PyTaskClassifierConfig`、`PyLlmClassifierConfig`、`PyStageRouterConfig` 等 Python 类，通过 `py_serde.rs` 的 `from_python`/`to_python` 实现 Python ↔ Rust 类型转换。

---

## 二、核心架构

### 2.1 整体请求流

Switchyard 的整体架构流可概括为：

```
Client (Claude Code / Codex / OpenAI SDK)
    │
    ▼ [OpenAI Chat / Anthropic Messages / OpenAI Responses]
┌─────────────────────────────────────────────────────┐
│              switchyard-server (Axum)                │
│  ┌───────────────────────────────────────────────┐  │
│  │           HTTP Endpoint Layer                  │  │
│  │  /v1/chat/completions                         │  │
│  │  /v1/messages                                 │  │
│  │  /v1/responses                                │  │
│  └───────────────────────────────────────────────┘  │
    │ decode_request(wire_format, body)
    ▼
┌─────────────────────────────────────────────────────┐
│         switchyard-translation (Engine)              │
│  ┌───────────────────────────────────────────────┐  │
│  │  FormatRegistry → FormatCodec::decode_request │  │
│  │  → LlmRequest (IR)                            │  │
│  └───────────────────────────────────────────────┘  │
    │
    ▼
┌─────────────────────────────────────────────────────┐
│             libsy (Algorithm)                        │
│  ┌───────────────────────────────────────────────┐  │
│  │  Algorithm::run_stream(request) → StepStream  │  │
│  │  Step::CallModel → Driver::call_model → LLM   │  │
│  │  Step::Done(RoutingOutcome)                   │  │
│  └───────────────────────────────────────────────┘  │
    │
    ▼
┌─────────────────────────────────────────────────────┐
│        libsy-llm-client (TranslatingLlmClient)       │
│  ┌───────────────────────────────────────────────┐  │
│  │  encode_request(IR → wire) → HTTP → decode    │  │
│  │  → Response                                  │  │
│  └───────────────────────────────────────────────┘  │
    │
    ▼ [provider-native format]
┌─────────────────────────────────────────────────────┐
│          LLM Backend (vLLM / NIM / Ollama)           │
└─────────────────────────────────────────────────────┘
```

### 2.2 关键抽象：Algorithm trait

```rust
#[async_trait]
pub trait Algorithm: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn route(self: Arc<Self>, driver: Driver, request: Request) -> Result<RoutingOutcome>;
    fn run_stream(self: Arc<Self>, request: Request) -> StepStream { ... }
}
```

**设计思路**：
- `Arc<dyn Algorithm>` 共享跨请求，每个算法自行保证线程安全
- `Driver` 是算法向宿主发起模型调用的唯一通道（通过 `call_model` 返回 oneshot）
- `run_stream` 将 `route` 的执行转化为 `Step` 流（`CallModel` / `Done`），宿主驱动该流
- 支持并发：`drive()` 函数通过 `FuturesUnordered` 并发服务多个 `CallModel`，实现 hedging/fan-out

### 2.3 关键抽象：Step 流

```rust
pub enum Step {
    CallModel(Box<CallModel>),  // 算法请求宿主执行一次 LLM 调用
    Done(Box<RoutingOutcome>),  // 路由完成
}

pub struct CallModel {
    pub algorithm: String,
    pub request: Request,
    pub models: Vec<ModelId>,  // 候选模型，按序尝试
    reply: oneshot::Sender<Result<Response>>,
}
```

`CallModel::respond()` 让宿主回填结果，算法通过 `.await` 获取。

---

## 三、协议层（switchyard-protocol）

### 3.1 Provider-neutral 类型设计

`crates/protocol/src/llm.rs` 定义了与供应商无关的中间表示（IR）：

```rust
pub struct LlmRequest {
    pub model: Option<String>,
    pub instructions: Vec<InstructionBlock>,  // system/developer 指令
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    pub sampling: SamplingParams,
    pub output: OutputParams,
    pub reasoning: ReasoningParams,
    pub stream: bool,
    pub extensions: ProviderExtensions,      // 供应商特有字段
    pub preservation: PreservationMetadata,  // 原始载荷保留
}

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
    Unknown { provider: FormatId, raw: Value },  // 未知块保留
}
```

**设计思路**：
- `ContentBlock` 使用 `#[serde(tag = "type")]` 标签，便于序列化/反序列化
- `Unknown` 变体保留无法归一化的供应商块，实现无损往返
- `PreservationMetadata` 保留原始请求/响应体，用于同格式重放
- `ProviderExtensions` 用 `Map<String, Value>` 保存非第一类字段

### 3.2 流式类型

```rust
pub enum LlmResponse {
    Stream(LlmResponseStream),  // 实时流
    Agg(AggLlmResponse),        // 缓冲聚合
}

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

`ResponseAccumulator` 将 `LlmResponseChunk` 流折叠为 `AggLlmResponse`，处理 tool call 参数拼接、reasoning 文本聚合等。

### 3.3 元数据与 Harness 归一化

`crates/protocol/src/metadata.rs` 的 `Metadata::from_headers()` 统一处理多种 Agent 框架的关联头：

```rust
const HEADER_CONFIG: &HeaderConfig = &[
    (SWITCHYARD_SESSION_ID_HEADER, &[
        SWITCHYARD_SESSION_ID_HEADER,
        CLAUDE_SESSION_ID_HEADER,
        RELAY_SESSION_ID_HEADER,
        OPENCODE_SESSION_ID_PATH,
        CODEX_SESSION_ID_PATH,
        SESSION_ID_HEADER,
    ]),
    // ... agent_id, parent_agent_id, agent_kind, agent_role, task_id, task_kind, turn_id
];
```

**关键设计**：
- 优先级链：Switchyard 头 > Claude Code 头 > NeMo Relay 头 > Codex JSON 路径 > 通用头
- Codex 的 `x-codex-turn-metadata` JSON 头通过 `resolve_path` 点分路径解析
- `parse_sub_agent()` 区分 `is_subagent`（血缘事实）与 `is_delegated_work`（路由信号）
- 子 Agent 工作类型白名单：`["collab_spawn", "review"]`

---

## 四、协议翻译（switchyard-translation）

### 4.1 翻译引擎

`crates/translation/src/engine.rs` 的 `TranslationEngine` 是核心：

```rust
pub struct TranslationEngine {
    registry: FormatRegistry,          // 缓冲编解码器注册表
    stream_registry: StreamCodecRegistry,  // 流式编解码器注册表
}

impl TranslationEngine {
    pub fn translate_request(&self, source, target, body, policy) -> Result<TranslationOutput> {
        let decoded = self.registry.codec(source)?.decode_request(body, policy)?;
        let encoded = self.registry.codec(target)?.encode_request(&decoded.request, policy)?;
        Ok(TranslationOutput { body: encoded.body, diagnostics: ... })
    }
    
    pub fn translate_event(&self, state, source, target, event) -> Result<Vec<Value>> {
        let source_codec = self.stream_registry.codec(source)?;
        let target_codec = self.stream_registry.codec(target)?;
        let canonical = source_codec.decode_event(state, event);
        Ok(canonical.into_iter().flat_map(|e| target_codec.encode_event(state, e)).collect())
    }
}
```

### 4.2 FormatCodec trait

```rust
pub trait FormatCodec: Send + Sync {
    fn format(&self) -> FormatId;
    fn decode_request(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedRequest>;
    fn encode_request(&self, request: &LlmRequest, policy: &TranslationPolicy) -> Result<EncodedRequest>;
    fn decode_response(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedResponse>;
    fn encode_response(&self, response: &AggLlmResponse, policy: &TranslationPolicy) -> Result<EncodedResponse>;
}
```

三个内建实现：
- `OpenAiChatCodec` — OpenAI Chat Completions
- `AnthropicMessagesCodec` — Anthropic Messages
- `OpenAiResponsesCodec` — OpenAI Responses

### 4.3 Anthropic 编解码示例

```rust
impl FormatCodec for AnthropicMessagesCodec {
    fn decode_request(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedRequest> {
        let body = crate::util::object(body, "$")?;
        let mut request = LlmRequest {
            model: body.get("model").and_then(Value::as_str).map(ToOwned::to_owned),
            output: OutputParams {
                max_output_tokens: body.get("max_tokens").map(|v| v.as_u64()...),
                response_format: decode_anthropic_output_format(body, &mut diagnostics, policy)?,
            },
            sampling: SamplingParams {
                temperature: body.get("temperature").and_then(Value::as_f64),
                top_p: body.get("top_p").and_then(Value::as_f64),
                top_k: body.get("top_k").and_then(Value::as_i64),
            },
            reasoning: ReasoningParams {
                effort: body.get("output_config").and_then(...).and_then(|o| object.get("effort"))...,
                raw: body.get("thinking").cloned(),
            },
            preservation: capture_request_preservation(WireFormat::AnthropicMessages, &Value::Object(body.clone()), policy),
            ..LlmRequest::default()
        };
        // system → instructions, messages → decode_anthropic_content, tools → decode_anthropic_tools
        Ok(DecodedRequest { request, diagnostics })
    }
}
```

**设计思路**：
- `preservation` 保留原始体，同格式重放时直接返回（`exact_preserved_request`）
- `diagnostics` 收集所有翻译警告/错误，不中断流程
- `TranslationPolicy` 控制 ID 生成策略（确定性 ID 保证 tool_use_id 配对）

---

## 五、路由算法（libsy）

### 5.1 算法体系总览

`crates/libsy/src/algorithms/` 实现了 7 种路由算法：

| 算法 | 文件 | 描述 |
|------|------|------|
| `Noop` | `noop.rs` | 不调用模型，直接返回 OK |
| `Passthrough` | `passthrough.rs` | 单目标直通 |
| `Random` | `rand.rs` | 加权随机分流（A/B 测试） |
| `LlmTaskClassifier` | `llm_class.rs` | LLM 判断（Capability/Escalation/Custom 三模式） |
| `StageRouter` | `stage.rs` | 信号驱动的阶段路由 |
| `CompositeRouter` | `composite.rs` | 组合路由（LLM 判断 + Stage） |
| `SubagentRouter` | `subagent.rs` | 子 Agent 路由 |
| `AdvisorGate` | `advisor_gate.rs` | 执行者 + 审查者门控 |

### 5.2 FallThrough 级联模式

多个算法共享 `FallThrough<State>` 级联骨架：

```rust
pub struct FallThrough<S> {
    processors: Vec<Arc<dyn Processor<S>>>,
    classifiers: Vec<Arc<dyn Classifier<S>>>,
}

impl<S> FallThrough<S> {
    pub fn execute(&self, driver: Driver, request: Request) -> Result<RoutingOutcome> {
        // 1. 运行所有 processors（信号提取/状态更新）
        // 2. 顺序尝试 classifiers，直到有一个给出 Scores
        // 3. 全部 abstain → 使用默认目标
    }
}
```

### 5.3 LLM Classifier（三模式）

```rust
pub enum LlmClassifierConfig {
    Capability { judge_target, efficient_target, capable_target, config },
    Escalation { judge_target, efficient_target, capable_target, contract, config, max_output_tokens },
    Custom { judge_target, targets, default_target, config },
}
```

**Capability 模式**：
- Judge 模型输出 `{crux, primary_rule, capability_boundary, p_solve}` 判决
- `capability_boundary` ∈ {supported, uncertain, unsupported, unmatched}
- 阈值公式：`threshold = base_threshold + boundary_steps * threshold_step`
- `p_solve >= threshold` → efficient，否则 → capable

**Escalation 模式**：
- 先调用 efficient，再用 judge 评判
- 连续 N 次 escalate 判决后 latch 到 capable
- 支持 `recent_turn_window` 控制 judge 可见的对话轮数

**Custom 模式**：
- 用户提供 JSON Schema + TargetSelector 策略
- Judge 输出经 Schema 验证后，用 JSON Pointer 提取目标名

### 5.4 Stage Router（信号驱动）

```rust
pub struct StageRouterConfig {
    pub mode: PickerMode,              // EfficientFirst / CapableFirst
    pub confidence_threshold: f64,     // 置信度阈值
    pub recent_window: Option<usize>,  // 最近工具结果窗口
    pub handoff_notes: Option<HandoffNoteConfig>,  // 升级/降级说明
    pub tier_prompts: TargetPrompts,   // 每层系统提示
    pub llm_fallback: Option<LlmFallback>,  // 信号不确定时的 LLM 回退
}
```

级联顺序：
1. `ToolSignalProcessor` — 提取工具结果信号
2. `StageClassifier` — 基于信号打分（`score_signal`）
3. `LlmTaskClassifier`（可选）— 信号不确定时回退到 LLM 判断
4. `FallOpen` — 最终回退到默认层

### 5.5 Advisor Gate（执行者 + 审查者）

```rust
pub struct AdvisorGate {
    executor: ModelId,     // 执行者（回答每个客户端可见轮次）
    advisor: ModelId,      // 审查者（审查终端轮次）
    config: AdvisorGateConfig,
    trigger: CompiledTrigger,
    state: Mutex<GateState>,
}
```

**工作流**：
1. Executor 回答每个轮次
2. 终端轮次（无 tool_use 或匹配 pattern）被缓冲
3. Advisor 审查：`APPROVE` 释放缓冲轮次，`REDO` 将计划反馈给 executor 重新生成
4. 每个 scope（bench/session/instance）最多 `max_reviews` 次审查
5. Advisor 故障时 `fail_open`（默认 APPROVE）

### 5.6 Affinity Router（粘性路由）

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

`ClassifyTrigger` 控制重新分类频率：
- `EveryRequest` — 每个请求都分类
- `NewSession` — 新 session 时分类
- `UserTurn` — 用户轮次时分类

---

## 六、HTTP 服务器（switchyard-server）

### 6.1 服务器架构

基于 Axum 框架：

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
    // ...
}
```

### 6.2 请求处理流

```rust
async fn handle_endpoint_inner(state, started, headers, body, wire_format) -> Response {
    let metadata = metadata_from_headers(headers);
    let (route, request) = resolve_route(&state, metadata, body, wire_format)?;
    let observer = stats_observer(state.stats.clone(), ...);
    let output = route.execute(request, Some(observer)).await?;
    let response = into_http_response(response, wire_format, response_model, request_extensions)?;
    attach_routing_headers(&mut response, served_model.as_str());
    response
}
```

### 6.3 错误处理

```rust
fn client_error(error: &LlmClientError) -> Response {
    match error {
        LlmClientError::InvalidRequest { message } => error_response(StatusCode::BAD_REQUEST, ...),
        LlmClientError::ContextWindowExceeded { message, .. } => error_response(StatusCode::BAD_REQUEST, ...),
        LlmClientError::UpstreamHttp { status, body } => error_response(*status, ...),
        LlmClientError::Timeout { source } => error_response(StatusCode::GATEWAY_TIMEOUT, ...),
        // ...
    }
}
```

错误按 wire_format 渲染为对应供应商的错误形状（Anthropic/OpenAI）。

### 6.4 优雅关闭

```rust
async fn serve_until_shutdown(server, handle, timeout, shutdown) {
    tokio::select! {
        result = &mut server => result.map_err(server_io_error),
        _ = shutdown => {
            handle.graceful_shutdown(Some(timeout));
            server.await.map_err(server_io_error)
        }
    }
}
```

---

## 七、LLM 客户端（libsy-llm-client）

### 7.1 TranslatingLlmClient

```rust
pub struct TranslatingLlmClient {
    model_to_config: HashMap<ModelId, ModelConfig>,
    client: reqwest::Client,
    forward_auth_client: reqwest::Client,  // 禁用重定向，防止凭证泄露
}

impl TranslatingLlmClient {
    pub async fn call_rewrite_model(&self, model, wire_format, request, metadata) -> Result<Response> {
        let backend = self.backend_for(model, wire_format)?;
        let encoded = encode_request(wire_format, &request, &TranslationPolicy::default())?;
        let http_response = self.send_encoded(backend, wire_format, llm_request, metadata, model, UpstreamEndpoint::Completions).await?;
        // 解码响应...
    }
}
```

### 7.2 后端抽象

```rust
pub trait Backend: Send + Sync {
    fn wire_format(&self) -> WireFormat;
    fn base_url(&self) -> &str;
    fn apply_auth(&self, request: RequestBuilder, model: &ModelId) -> Result<RequestBuilder>;
    fn send(&self, request: RequestBuilder, endpoint: UpstreamEndpoint) -> Result<EncodedResponse>;
}

pub struct HttpBackendConfig {
    pub format: WireFormat,
    pub base_url: String,
    pub auth: AuthConfig,
    pub headers: HeaderMap,
    pub timeout: Duration,
    pub max_retries: u32,
}
```

### 7.3 重试与容错

```rust
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
```

- 指数退避 + 尊重 `Retry-After` 头
- 400 错误通过供应商规则分类为 `ContextWindowExceeded`
- 流式响应在成功头到达后立即返回

### 7.4 Run Observer

```rust
pub enum RunObservation {
    AnswerCall(LlmCallObservation),  // 最终回答调用
    LlmCall(LlmCallObservation),     // 分类器/Judge 调用
    RoutingOverhead(Duration),       // 路由开销
}

pub type RunObserver = Arc<dyn Fn(RunObservation) + Send + Sync>;
```

宿主通过 observer 接收调用事件，用于统计/日志。

---

## 八、Python 绑定（switchyard-py）

### 8.1 PyO3 绑定设计

```rust
#[pyclass(name = "TaskClassifierConfig", module = "switchyard.libsy", frozen, skip_from_py_object)]
struct PyTaskClassifierConfig {
    inner: TaskClassifierConfig,
}

#[pymethods]
impl PyTaskClassifierConfig {
    #[new]
    fn new(base_threshold: f64, threshold_step: f64, ...) -> PyResult<Self> { ... }
}
```

### 8.2 类型转换

```rust
// py_serde.rs
pub fn from_python<'py, T: Deserialize<'py>>(obj: &'py Bound<'py, PyAny>) -> PyResult<T> {
    let json = python_to_json(obj)?;
    T::deserialize(json).map_err(|e| PyValueError::new_err(e.to_string()))
}

pub fn to_python<'py, T: Serialize>(py: Python<'py>, value: &T) -> PyResult<Bound<'py, PyAny>> {
    let json = serde_json::to_value(value)?;
    json_to_python(py, &json)
}
```

通过 serde JSON 作为 Python ↔ Rust 的中介格式。

### 8.3 异步绑定

```rust
#[pyfunction]
fn run_algorithm<'py>(py: Python<'py>, algo: &Bound<'py, PyAny>, request: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    pyo3_asyncio::tokio::future_into_py(py, async move {
        // 驱动算法 step stream
    })
}
```

---

## 九、技能蒸馏（switchyard-skill-distillation）

### 9.1 数据模型

```rust
pub struct Trajectory {
    pub schema_version: u16,
    pub id: SkillEvidenceId,
    pub task: TaskDescriptor,
    pub execution: ExecutionMetadata,
    pub source: TrajectorySourceInfo,
    pub events: Vec<TrajectoryEvent>,
    pub outcome: Option<TrajectoryOutcome>,
    pub metadata: Metadata,
}

pub enum TrajectoryEventKind {
    Message, ToolCall, ToolResult, Observation, Error, FinalOutput,
}

pub struct SkillCandidate {
    pub id: SkillVersionId,
    pub namespace: SkillNamespace,
    pub content: String,  // 技能内容（Markdown/Skill 格式）
    pub provenance: SkillProvenance,
    pub evidence: Vec<SkillEvidenceId>,
    pub validation: Option<ValidationReport>,
}
```

### 9.2 端口抽象

```rust
#[async_trait]
pub trait TrajectorySource {
    async fn list_trajectories(&self, after: Option<&SkillEvidenceId>) -> Result<Vec<Trajectory>>;
}

#[async_trait]
pub trait SkillDistiller {
    async fn distill(&self, request: &DistillationRequest) -> Result<SkillCandidate>;
}

#[async_trait]
pub trait SkillValidator {
    async fn validate(&self, candidate: &SkillCandidate, trajectories: &[Trajectory]) -> Result<ValidationReport>;
}

#[async_trait]
pub trait SkillStore {
    async fn load(&self, namespace: &SkillNamespace) -> Result<Option<SkillCandidate>>;
    async fn save(&self, candidate: &SkillCandidate) -> Result<()>;
    async fn activate(&self, id: &SkillVersionId) -> Result<()>;
}
```

**设计思路**：
- 端口与实现分离，允许替换轨迹源（本地运行器/基准测试导入器）、蒸馏器（LLM/规则）、验证器、存储
- `SkillProvenance` 记录技能的来源轨迹，实现可追溯性
- `ActivationRecord` 记录技能激活历史

---

## 十、预填充路由（prefill-router）

### 10.1 特征提取

```rust
pub trait PrefillForward: Send {
    fn forward(&mut self, request: &ForwardRequest) -> Result<ForwardOutput>;
    fn unload(&mut self) -> Result<()>;
}

pub struct TransformersForward {
    model: Py<PyAny>,  // HuggingFace Transformers 模型
    tokenizer: Py<PyAny>,
    device: String,
}

pub struct ForwardRequest {
    pub prompts: Vec<String>,
    pub chat_template_kwargs: serde_json::Map<String, serde_json::Value>,
    pub layers: LayerSelection,      // UpperHalf / All / Selected
    pub pooling: Vec<Pooling>,       // Last / Mean
    pub batch_size: usize,
    pub max_length: usize,
}

pub struct ForwardOutput {
    pub hidden_last: BTreeMap<usize, Vec<Vec<f32>>>,  // 每层 → 每 prompt → 隐藏向量
    pub hidden_mean: BTreeMap<usize, Vec<Vec<f32>>>,
    pub n_layers: usize,
    pub hidden_dim: usize,
}
```

**设计思路**：
- 通过 PyO3 嵌入 Python，复用 HuggingFace Transformers 的 forward 路径
- `LayerSelection::UpperHalf` 提取上半层隐藏状态（与 NVIDIA LLM Router 参考实现一致）
- `PrefillForward` trait 允许替换为 Candle 等纯 Rust 实现

---

## 十一、可观测性

### 11.1 Prometheus 指标

`crates/switchyard-server/src/metrics.rs` 定义了完整的指标体系：

| 指标 | 类型 | 描述 |
|------|------|------|
| `switchyard.upstream_attempts` | Counter | 上游尝试次数（按状态码分类） |
| `switchyard.client_responses` | Counter | 客户端响应（success/retryable_error/other_error） |
| `switchyard.model_call_latency_ms` | Histogram | 模型调用延迟 |
| `switchyard.total_latency_ms` | Histogram | 端到端延迟 |
| `switchyard.routing_overhead_ms` | Histogram | 路由开销 |
| `switchyard.router_retry_recovered` | Counter | 路由重试恢复次数 |
| `switchyard.build_info` | Gauge | 构建信息 |

直方图桶边界：
```rust
const LLM_LATENCY_BUCKETS_MS: &[f64] = &[
    0.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0,
    1000.0, 2500.0, 5000.0, 7500.0, 10_000.0, 15_000.0, 30_000.0, 60_000.0, 120_000.0, 300_000.0,
];
const ROUTING_OVERHEAD_BUCKETS_MS: &[f64] = &[
    0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0,
];
```

### 11.2 OpenTelemetry 集成

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
    Ok(Observability { tracer_provider })
}
```

- 支持 W3C Trace Context 传播（`traceparent` / `tracestate`）
- OTLP 导出器通过环境变量配置（`OTEL_EXPORTER_OTLP_ENDPOINT`）
- `OTEL_SDK_DISABLED=true` 可禁用

### 11.3 Tracing  Span

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
        ...
    )
)]
pub async fn call_model(&self, mut request: Request, models: Vec<ModelId>) -> Result<Response> { ... }
```

使用 OpenInference 语义约定（`openinference.span.kind`），兼容可观测性平台。

---

## 十二、运行器（switchyard-runner）

### 12.1 TOML 配置系统

```toml
schema_version = 1

[llm_clients.openai]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = "sk-..."
forward_auth = false

[targets.gpt4]
llm_client = "openai"
id = "gpt-4o"

[routes.my_route]
id = "my-route"
type = "llm_classifier"
classifier_target = "judge-model"
strong_target = "gpt-4o"
weak_target = "gpt-4o-mini"
base_threshold = 0.5
```

### 12.2 配置加载

```rust
pub(crate) struct DeploymentConfig {
    schema_version: u32,
    llm_clients: BTreeMap<String, LlmClientConfig>,
    targets: BTreeMap<String, TargetConfig>,
    routes: BTreeMap<String, RouteConfig>,
}

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

### 12.3 AlgorithmSpec 枚举

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

通过 serde 的 `tag = "type"` 实现多态反序列化，配置驱动算法选择。

---

## 十三、测试

### 13.1 单元测试

每个 crate 内嵌 `#[cfg(test)] mod tests`，覆盖：
- IR 序列化/反序列化（`serde_uses_python_friendly_dictionary_shapes`）
- 流式折叠（`folds_text_usage_and_stop_reason`）
- 工具调用参数拼接（`assembles_tool_calls_by_index`）
- 头解析优先级（`sy_header_resolves_paths_in_order_and_descends_into_json`）
- 子 Agent 路由信号（`subagent_routing_honors_explicit_signals_and_delegated_work_kinds`）

### 13.2 集成测试

`crates/libsy/src/core/testing.rs` 提供 `test_drive` 辅助函数：

```rust
async fn test_drive<F, Fut>(algorithm: Arc<dyn Algorithm>, request: Request, serve: F) -> Result<(ModelId, Response)>
where F: Fn(ModelId, Request) -> Fut, Fut: Future<Output = Result<Response>>
```

`serve` 闭包模拟 LLM 调用，用于测试算法逻辑。

### 13.3 Soak 测试

`crates/switchyard-soak/tests/soak.rs` 启动真实 Axum 服务器，模拟后端：

```rust
async fn serve(app: Router) -> Result<TestServer, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service()).await;
    });
    Ok(TestServer { base_url: format!("http://{addr}"), task })
}
```

测试覆盖：健康检查、指标、模型列表、chat/messages/responses 端点、错误处理。

### 13.4 基准测试

`benchmark/` 目录包含性能基准测试，用于测量路由开销、翻译延迟等。

---

## 十四、配置系统

### 14.1 TOML 配置

完整配置示例：

```toml
schema_version = 1

[llm_clients.openai]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
forward_auth = false
timeout_seconds = 120
max_retries = 3

[llm_clients.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "${ANTHROPIC_API_KEY}"
forward_auth = true

[targets.gpt4]
llm_client = "openai"
id = "gpt-4o"
extra_body = { "temperature" = 0.7 }

[targets.claude]
llm_client = "anthropic"
id = "claude-sonnet-4-20250514"

[routes.auto]
id = "auto"
type = "stage_router"
strong_target = "claude"
weak_target = "gpt-4o"
picker = "efficient_first"
confidence_threshold = 0.6
recent_window = 5
classifier = { classifier_target = "gpt-4o-mini", base_threshold = 0.5 }
```

### 14.2 环境变量

| 变量 | 描述 |
|------|------|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP 导出端点 |
| `OTEL_SERVICE_NAME` | 服务名称 |
| `OTEL_SDK_DISABLED` | 禁用 OpenTelemetry |
| `RUST_LOG` | 日志级别过滤 |

---

## 十五、对 laew 工程的借鉴建议

### 15.1 Rust 实现方面

#### 15.1.1 分层架构

Switchyard 的六层分离（protocol/translation/libsy/llm-client/server/runner）是 laew 可借鉴的样板：

- **laew 现状**：`llm/anthropic.rs` 和 `llm/openai.rs` 已完成协议分离，但协议差异仍渗透到 agent 循环
- **借鉴**：引入 `protocol` crate，定义 `LlmRequest`/`LlmResponse` IR，让 agent 循环完全不接触协议细节

#### 15.1.2 Trait 抽象

```rust
// Switchyard 的 Algorithm trait 是 laew MultiAgentOrchestrator 可借鉴的模式
#[async_trait]
pub trait Algorithm: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn route(self: Arc<Self>, driver: Driver, request: Request) -> Result<RoutingOutcome>;
}
```

- **laew 现状**：`YoloRunner` 硬编码双 Agent 架构
- **借鉴**：将每个 Agent 抽象为 `Agent` trait，`MultiAgentOrchestrator` 通过配置组合不同 Agent

#### 15.1.3 并发模式

Switchyard 的 `drive()` 函数通过 `FuturesUnordered` 并发服务多个 `CallModel`：

```rust
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

- **laew 借鉴**：SubAgent-Work 可并发执行，Quality-Check 可与下一个 SubAgent 流水线化

### 15.2 协议翻译方面

#### 15.2.1 IR 设计

Switchyard 的 `ContentBlock::Unknown` 变体值得借鉴：

```rust
pub enum ContentBlock {
    // ... 已知变体
    Unknown {
        provider: FormatId,
        raw: Value,  // 保留原始块
    },
}
```

- **laew 现状**：`llm/anthropic.rs` 和 `llm/openai.rs` 直接操作线格式
- **借鉴**：定义 `protocol` crate，`Unknown` 变体保证新字段不会丢失

#### 15.2.2 Preservation 机制

Switchyard 的 `PreservationMetadata` 保留原始体用于同格式重放：

```rust
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,
    pub responses: BTreeMap<FormatId, Value>,
}
```

- **laew 借鉴**：在协议转换失败时，可回退到原始体重试

#### 15.2.3 流式翻译

Switchyard 的 `translate_event` 逐事件翻译，支持流式协议转换：

```rust
pub fn translate_event(&self, state, source, target, event) -> Result<Vec<Value>> {
    let canonical = source_codec.decode_event(state, event);
    Ok(canonical.into_iter().flat_map(|e| target_codec.encode_event(state, e)).collect())
}
```

- **laew 借鉴**：TUI 流式输出时，可在翻译层直接转换 SSE 事件

### 15.3 路由算法方面

#### 15.3.1 任务分类

Switchyard 的 `LlmTaskClassifier` 的 Capability 模式与 laew 的 Yolo 三档分类（simple/medium/hard）异曲同工：

| Switchyard | laew |
|------------|------|
| `capability_boundary` ∈ {supported, uncertain, unsupported} | `TaskLevel` ∈ {simple, medium, hard} |
| `p_solve` 概率 | 三步分析（目的→目标→意图） |
| `base_threshold` 阈值 | 分类阈值 |

- **借鉴**：
  - 引入 `p_solve` 概率，让 Yolo 输出置信度
  - 使用 `recent_turn_window` 控制 Yolo 可见的对话历史
  - 引入 `AffinityRouter` 实现 session 级别的粘性路由

#### 15.3.2 信号驱动路由

Switchyard 的 `StageRouter` 基于工具结果信号路由，无需额外 LLM 调用：

```rust
pub struct ToolSignalProcessor {
    recent_window: usize,
}
```

- **laew 借鉴**：
  - SubAgent-Work 执行后，根据工具调用结果信号决定下一步
  - 失败回流时，根据错误类型信号选择重试策略

#### 15.3.3 Advisor Gate

Switchyard 的 `AdvisorGate` 与 laew 的 Quality-Check Agent 功能相似：

| Switchyard AdvisorGate | laew Quality-Check |
|------------------------|-------------------|
| Executor 回答每个轮次 | SubAgent-Work 执行任务 |
| Advisor 审查终端轮次 | QC Agent 审查执行结果 |
| `APPROVE` / `REDO` | 通过 / 拒绝 + 反馈 |
| `max_reviews` 预算 | 重试次数限制 |
| `fail_open` 策略 | 审查失败时的回退策略 |

- **借鉴**：
  - 引入 `GateTrigger` 机制，只在特定条件（如无 tool_use 的终端轮次）触发 QC
  - 引入 `max_reviews` 预算，防止无限重试
  - 引入 `fail_open` 策略，QC 失败时默认通过

#### 15.3.4 FallThrough 级联

Switchyard 的 `FallThrough` 级联模式可简化 laew 的多 Agent 编排：

```rust
pub struct FallThrough<S> {
    processors: Vec<Arc<dyn Processor<S>>>,
    classifiers: Vec<Arc<dyn Classifier<S>>>,
}
```

- **laew 借鉴**：
  - 将 Yolo/Plan/Main-Work/SubAgent-Work/QC/SessionContext 抽象为 `Classifier` 链
  - 每个 Agent 独立评分，第一个给出高置信度的胜出
  - 全部 abstain 时使用默认策略

### 15.4 可观测性方面

#### 15.4.1 Prometheus 指标

Switchyard 的指标体系值得 laew 借鉴：

| Switchyard 指标 | laew 对应 |
|-----------------|----------|
| `switchyard.model_call_latency_ms` | 每次 LLM 调用延迟 |
| `switchyard.routing_overhead_ms` | 路由决策开销 |
| `switchyard.upstream_attempts` | 上游调用次数（按状态码） |
| `switchyard.client_responses` | 客户端响应统计 |

#### 15.4.2 OpenTelemetry

Switchyard 的 W3C Trace Context 传播：

```rust
pub(crate) fn request_span(headers: &HeaderMap) -> tracing::Span {
    let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
    let span = tracing::info_span!(...);
    let _ = span.set_parent(parent);
    span
}
```

- **laew 借鉴**：在 TUI 中注入 trace context，便于与外部可观测性平台集成

### 15.5 配置系统方面

Switchyard 的 TOML 配置 + serde 反序列化模式：

```rust
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlgorithmSpec {
    StageRouter { tiers, picker, classifier, subagents },
    LlmClassifier { config },
    // ...
}
```

- **laew 借鉴**：
  - 将多 Agent 架构配置化，通过 TOML 选择算法组合
  - 使用 `#[serde(tag = "type")]` 实现多态配置

---

## 十六、总结

Switchyard 是 NVIDIA 在 LLM 流量代理领域的 Rust 实践，其核心优势在于：

1. **分层架构清晰**：protocol/translation/libsy/llm-client/server/runner 六层分离，每层职责单一
2. **Provider-neutral IR**：`LlmRequest`/`LlmResponse` IR 隔离协议差异，`Unknown` 变体保证无损
3. **可组合路由算法**：`Algorithm` trait + `FallThrough` 级联，支持 7 种算法自由组合
4. **生产级可观测性**：Prometheus + OpenTelemetry 双轨，W3C Trace Context 传播
5. **配置驱动**：TOML 配置 + serde 反序列化，算法选择无需改代码

对 laew 的启示：
- **短期**：引入 `protocol` crate 和 IR，简化协议差异处理
- **中期**：将多 Agent 架构抽象为 `Agent` trait + 级联编排
- **长期**：引入信号驱动路由和 Advisor Gate，优化 Quality-Check 流程

Switchyard 的代码质量、文档完整度和工程成熟度都较高，是 laew 在 Rust 实现、协议翻译、路由算法三个方向上的重要参考。
