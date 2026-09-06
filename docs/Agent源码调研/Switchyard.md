# Switchyard 综合深度分析

> 调研对象: Switchyard (Rust, NVIDIA LLM 网关)
> 调研日期: 2026-09-05
> 原始文档: 3 份(Switchyard-源码调研.md 1199行 + Switchyard-深度分析.md 1405行 + Switchyard-核心机制深度分析.md 1623行,合计 4227行)
> 合并后行数: ~1800 行(去重综合)

---

## 目录

- [一、项目元信息](#一项目元信息)
- [二、协议 IR(LlmRequest/LlmResponse)](#二协议irllmrequestllmresponse)
- [三、ContentBlock::Unknown 无损保留](#三contentblockunknown-无损保留)
- [四、TranslationEngine](#四translationengine)
- [五、7 种路由算法](#五7-种路由算法)
- [六、FallThrough 级联](#六fallthrough-级联)
- [七、PyO3 绑定](#七pyo3-绑定)
- [八、缓存与状态管理](#八缓存与状态管理)
- [九、对 laew 的借鉴](#九对-laew-的借鉴)

---

## 一、项目元信息

### 1.1 项目概况

Switchyard 是 NVIDIA 开发的 Rust LLM 流量代理与库,作为 NeMo 生态的一部分,专注于 LLM 请求的智能路由与协议翻译。

| 属性 | 值 |
|------|-----|
| **仓库** | https://github.com/NVIDIA-NeMo/Switchyard |
| **版本** | v0.2.0 (pre-alpha) |
| **许可证** | Apache-2.0 |
| **语言** | Rust (Workspace 架构) |
| **定位** | LLM 流量代理 + 协议翻译 + 智能路由 |
| **核心依赖** | tokio, serde, reqwest, axum, tracing, tracing-opentelemetry, pyo3 |

### 1.2 Workspace 组织

Switchyard 采用 Rust Workspace 组织,根目录 `Cargo.toml` 声明了 10 个成员 crate,形成清晰的六层分离架构:

**核心六层**:

| 层级 | Crate | 职责 | 文件路径 |
|------|-------|------|----------|
| 第一层(协议层) | `switchyard-protocol` | Provider-neutral 请求/响应/流式类型 | `crates/protocol/` |
| 第二层(翻译层) | `switchyard-translation` | OpenAI/Anthropic 线格式 ↔ IR 编解码 | `crates/switchyard-translation/` |
| 第三层(算法层) | `libsy` | 路由算法 trait + 7 种算法实现 | `crates/libsy/` |
| 第四层(客户端层) | `libsy-llm-client` | 翻译后 HTTP 客户端 | `crates/libsy-llm-client/` |
| 第五层(服务器层) | `switchyard-server` | Axum HTTP 服务器 + 可观测性 | `crates/switchyard-server/` |
| 第六层(运行器层) | `switchyard-runner` | TOML 配置驱动的运行器 | `crates/switchyard-runner/` |

**独立模块**:

| 模块 | Crate | 职责 |
|------|-------|------|
| Python 绑定 | `switchyard-py` | PyO3 Python 绑定 |
| 预填充路由 | `prefill-router` | Transformers 嵌入提取 |
| 技能蒸馏 | `switchyard-skill-distillation` | Trajectory/SkillCandidate 数据模型 |
| 浸泡测试 | `switchyard-soak` | 浸泡测试 |

### 1.3 整体请求流

Switchyard 的整体架构流可概括为:

```
Client (Claude Code / Codex / OpenAI SDK)
    │
    ▼ [OpenAI Chat / Anthropic Messages / OpenAI Responses]
┌─────────────────────────────────────────────────────────┐
│  switchyard-server (Axum HTTP 服务器)                     │
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

## 二、协议 IR(LlmRequest/LlmResponse)

### 2.1 LlmRequest 完整结构

**文件**: `crates/protocol/src/llm.rs:303-334`

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
- `#[serde(default)]` 保证向前兼容,缺失字段走 Default,供应商添加新字段时不会导致反序列化失败
- `instructions` 与 `messages` 分离: Anthropic 风格的 system/developer 指令从对话轮次中独立出来,避免混入 `Role::System` 消息
- `extensions: ProviderExtensions` 用 `Map<String, Value>` 保存非第一类字段,跨格式翻译时这一段被 codec 选择性保留(见 `crates/switchyard-translation/src/codecs/openai_chat/buffered.rs:244` 的 `copy_openai_chat_request_extensions`)
- `preservation: PreservationMetadata` 在 decode_request 阶段由 codec 写入(见 `crates/switchyard-translation/src/util.rs:226` 的 `capture_request_preservation`)

### 2.2 LlmResponse 与流式类型

**文件**: `crates/protocol/src/stream.rs`

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

**ResponseAccumulator 折叠逻辑** (`crates/protocol/src/stream.rs:339-447`):
- `MessageStart` / `Usage` / `MessageStop`: 后覆盖前
- `TextDelta` / `ReasoningDelta`: 字符串拼接
- `ToolCallDelta`: 按 `index` 分组收集到 `PartialToolCall`,`arguments` 拼接后 JSON 解析
- `finish()` 输出 `Vec<ContentBlock>` 顺序: reasoning → text → tool_calls

**完整可逆性**: 测试用例 `into_stream_round_trips_and_around_agg`(`stream.rs:652-680`)证明 `Agg → Stream → Agg` 完整可逆。

### 2.3 PreservationMetadata 机制

**文件**: `crates/protocol/src/llm.rs:294-301`

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,   // 原始请求体
    pub responses: BTreeMap<FormatId, Value>,  // 原始响应体
}
```

**写入位置**: `crates/switchyard-translation/src/util.rs:226-249`

```rust
pub fn capture_request_preservation(
    format: impl Into<FormatId>,
    body: &Value,
    policy: &TranslationPolicy,
) -> PreservationMetadata {
    let mut preservation = extract_preservation(body);
    if policy.preservation != PreservationPolicy::Disabled {
        preservation.requests.insert(format.into(), body.clone());
    }
    preservation
}
```

**读出位置**: `crates/switchyard-translation/src/util.rs:252-273`

```rust
pub fn exact_preserved_request(
    preservation: &PreservationMetadata,
    format: impl Into<FormatId>,
    policy: &TranslationPolicy,
) -> Option<Value> { ... }
```

**实际生效**: `crates/switchyard-translation/src/codecs/openai_chat/buffered.rs:186-193`

```rust
fn encode_request(&self, request: &LlmRequest, policy: &TranslationPolicy)
    -> Result<EncodedRequest> {
    if let Some(body) =
        exact_preserved_request(&request.preservation, WireFormat::OpenAiChat, policy)
    {
        return Ok(EncodedRequest {
            body,
            diagnostics: Vec::new(),
        });
    }
    // ... 从 IR 重新编码
}
```

**关键细节**:
- 写入是 full clone(`body.clone()`),读取是 `cloned()` 出来一次性消费
- 开关 preservation 是按调用级 policy 控制,IR 持有方可以随时丢弃
- `prepare_request_for_target`(`util.rs:280-301`)在修改了 prompt 时主动 `preservation.requests.clear()`,避免重放出旧数据
- `stamp_preserved_request_models`(`util.rs:304-315`)在改 model 时只重写 OpenAI/Anthropic 三种已知格式的 body,未知格式丢弃

### 2.4 ProviderExtensions 与元数据归一化

**文件**: `crates/protocol/src/metadata.rs`

`Metadata::from_headers()` 统一处理多种 Agent 框架的关联头:

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

**关键设计**:
- 优先级链: Switchyard 头 > Claude Code 头 > NeMo Relay 头 > Codex JSON 路径 > 通用头
- Codex 的 `x-codex-turn-metadata` JSON 头通过 `resolve_path` 点分路径解析
- `parse_sub_agent()` 区分 `is_subagent`(血缘事实)与 `is_delegated_work`(路由信号)
- 子 Agent 工作类型白名单: `["collab_spawn", "review"]`

---

## 三、ContentBlock::Unknown 无损保留

### 3.1 Unknown 变体定义

**文件**: `crates/protocol/src/llm.rs:124-131`

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

**设计要点**:
- `Unknown` 是真正的"逃生舱口": OpenAI 的 `web_search_call`、Anthropic 的 `server_tool_use`、Codex 的 `compaction` 等无法归一化的块都进这里
- `raw` 是 `serde_json::Value`(不重写),所以往返无损 — 这是 Switchyard 与传统 typed-ir 设计的最大差异
- `provider: FormatId` 让跨格式翻译时 codec 能选择是丢弃还是保留
- 类似的"逃生舱"也出现在 `ImageSource::Raw(Value)`(llm.rs:152)、`FileSource::Raw(Value)`(llm.rs:169)、`MediaSource::Raw(Value)`(llm.rs:191)、`ToolChoice::Raw(Value)`(llm.rs:245)

### 3.2 对 laew 借鉴

- laew 的 `llm/anthropic.rs` / `llm/openai.rs` 把消息直接转成 `serde_json::Value`,没有 IR。新建 `crates/protocol/src/llm.rs` 抽出 IR 是第一步
- `ContentBlock::Unknown { provider, raw }` 这层兜底值得借鉴: laew 的统一消息模型(`agent/mod.rs`)目前只有 `user/assistant/tool_result` 等已知角色,遇到 Anthropic 的 `thinking` 块或 OpenAI 的 `reasoning` 块就丢失

---

## 四、TranslationEngine

### 4.1 TranslationEngine 结构

**文件**: `crates/switchyard-translation/src/engine.rs:82-95`

```rust
pub struct TranslationEngine {
    registry: FormatRegistry,              // 缓冲编解码器注册表
    stream_registry: StreamCodecRegistry,  // 流式编解码器注册表
}

impl Default for TranslationEngine {
    fn default() -> Self {
        Self {
            registry: FormatRegistry::with_builtins(),
            stream_registry: StreamCodecRegistry::with_builtins(),
        }
    }
}
```

**核心方法**:
- `translate_request(source, target, body, policy)` — 请求翻译(engine.rs:148-169)
- `translate_response(source, target, body, policy)` — 响应翻译
- `translate_event(state, source, target, event)` — 流式事件翻译
- `decode_stream_event(state, source, event)` — 解码并保留原始事件
- `encode_stream_event(state, target, event)` — 编码(同格式重放保留值)
- `finish_stream(state, target)` — 流结束时的收尾事件

### 4.2 FormatCodec trait

**文件**: `crates/switchyard-translation/src/codecs/mod.rs:45-68`

```rust
pub trait FormatCodec: Send + Sync {
    fn format(&self) -> FormatId;
    fn decode_request(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedRequest>;
    fn encode_request(&self, request: &LlmRequest, policy: &TranslationPolicy) -> Result<EncodedRequest>;
    fn decode_response(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedResponse>;
    fn encode_response(&self, response: &AggLlmResponse, policy: &TranslationPolicy) -> Result<EncodedResponse>;
}
```

**四个辅助结构体**:
- `DecodedRequest { request: LlmRequest, diagnostics: Vec<TranslationDiagnostic> }`
- `EncodedRequest { body: Value, diagnostics: Vec<TranslationDiagnostic> }`
- `DecodedResponse { response: AggLlmResponse, diagnostics: Vec<TranslationDiagnostic> }`
- `EncodedResponse { body: Value, diagnostics: Vec<TranslationDiagnostic> }`

**三个内建实现**(`engine.rs:59-65`):
- `OpenAiChatCodec` — OpenAI Chat Completions (`/v1/chat/completions`)
- `AnthropicMessagesCodec` — Anthropic Messages (`/v1/messages`)
- `OpenAiResponsesCodec` — OpenAI Responses (`/v1/responses`)

### 4.3 AnthropicMessagesCodec 实现示例

**文件**: `crates/switchyard-translation/src/codecs/anthropic/buffered.rs:34-94`

```rust
impl FormatCodec for AnthropicMessagesCodec {
    fn format(&self) -> FormatId { WireFormat::AnthropicMessages.into() }

    fn decode_request(&self, body: &Value, policy: &TranslationPolicy) -> Result<DecodedRequest> {
        let body = crate::util::object(body, "$")?;
        let mut diagnostics = Vec::new();
        let max_output_tokens = body.get("max_tokens").map(|value| {
            value.as_u64().ok_or_else(|| TranslationError::InvalidValue { ... })
        }).transpose()?;
        let mut request = LlmRequest {
            model: body.get("model").and_then(Value::as_str)
                .filter(|model| !model.is_empty()).map(ToOwned::to_owned),
            output: OutputParams { max_output_tokens, response_format },
            sampling: SamplingParams { temperature, top_p, top_k },
            reasoning: ReasoningParams {
                effort: body.get("output_config").and_then(Value::as_object)
                    .and_then(|object| object.get("effort"))
                    .and_then(Value::as_str).map(ToOwned::to_owned),
                raw: body.get("thinking").cloned(),
            },
            preservation: capture_request_preservation(
                WireFormat::AnthropicMessages,
                &Value::Object(body.clone()),
                policy,
            ),
            ..LlmRequest::default()
        };
        // system → instructions
        if let Some(system) = body.get("system") ...
            request.instructions.push(InstructionBlock { role: Role::System, content });
        // messages → ...
        Ok(DecodedRequest { request, diagnostics })
    }
}
```

**核心模式**:
1. `object(body, "$")` 先校验顶层是对象,返回 typed error(`util.rs:25`)
2. 每个字段都用 `.get(...).and_then(...)` 链式取值;找不到给 `None` 而不是 `Err`,保持 IR 的"宽容缺字段"哲学
3. `body.get("thinking").cloned()` 把 thinking 整段塞进 `ReasoningParams::raw`,后续按需翻译

### 4.4 OpenAiChatCodec 实现要点

**文件**: `crates/switchyard-translation/src/codecs/openai_chat/buffered.rs:33-249`

**关键模式**:
- 第一行短路: 先看有没有 preserved body,有就直接返回,不重新编码
- 把 `instructions` 合并到 `messages[]` 头部(`role: "system"` 或 `"developer"`)
- `text_from_blocks(&instruction.content, "\n\n")` 把 `Vec<ContentBlock>` 拼成纯文本
- `copy_openai_chat_request_extensions(&mut body, &request.extensions.fields)` 把不属于第一类的字段原样写到顶层
- `embed_preservation` 把整个 `PreservationMetadata` 包进 `metadata._switchyard_translation` 键(`util.rs:317-344`),用于多跳 round trip

### 4.5 TranslationPolicy

```rust
pub struct TranslationPolicy {
    pub preserve_original: bool,       // 是否保留原始体
    pub deterministic_ids: bool,       // 是否生成确定性 ID(保证 tool_use_id 配对)
    pub preserve_streaming: bool,      // 是否保留流式事件
}
```

### 4.6 流式翻译状态机

**文件**: `crates/translation/src/engine.rs`

```rust
pub struct StreamTranslationState {
    source: Option<FormatId>,
    target: Option<FormatId>,
    // 内部状态: 部分 tool call 缓冲、reasoning 状态等
}
```

**`translate_event` 流程**:
1. `source_codec.decode_event(state, event)` → 规范化事件
2. `target_codec.encode_event(state, canonical)` → 目标格式事件
3. 返回 `Vec<Value>`(一对多映射)

**保留机制**:
- `decode_stream_event` 返回 `LlmResponseStreamEvent::preserved(source, raw, normalized)`
- `encode_stream_event` 检测 source==target 时直接重放 `raw`,否则用 `normalized` 重新编码

---

## 五、7 种路由算法

### 5.1 Algorithm trait 与 Driver

**文件**: `crates/libsy/src/core/algorithm.rs:353-404`

```rust
#[async_trait]
pub trait Algorithm: Send + Sync + 'static {
    fn name(&self) -> &str;

    /// Run one request to completion: make routing-time model calls with
    /// [`Driver::call_model`] and return the terminal [`RoutingOutcome`].
    async fn route(self: Arc<Self>, driver: Driver, request: Request) -> Result<RoutingOutcome>;

    /// Process a request to completion, returning a stream of [`Step`]s.
    fn run_stream(self: Arc<Self>, request: Request) -> StepStream {
        let (driver, step_rx) = Driver::new(self.name());
        let span = observability::run_span(self.name(), &request);
        let handle = tokio::spawn(async move {
            let algorithm = self.name().to_string();
            let route = AssertUnwindSafe(self.route(driver.clone(), request)).catch_unwind();
            let result = observability::observe_run(&algorithm, async move {
                route.await.unwrap_or_else(|payload| {
                    Err(LibsyError::AlgorithmError {
                        message: format!("algorithm task panicked: {}", panic_message(payload.as_ref()))
                    })
                })
            }).await;
            let _ = driver.finish(result).await;
        }.instrument(span));
        let abort_guard = AbortOnDrop(handle.abort_handle());
        Box::pin(ReceiverStream::new(step_rx).map(move |step| {
            let _keep_alive = &abort_guard;
            step
        }))
    }
}
```

**关键设计**:
- `self: Arc<Self>`: 算法实例被 `Arc<dyn Algorithm>` 共享跨请求,每个实现自己保证线程安全
- `run_stream` 默认实现 = 启动 `tokio::spawn` + `AssertUnwindSafe` + `catch_unwind` 抓 panic + 通过 `mpsc::Sender<Result<Step>>` 把步骤推给消费者
- `AbortOnDrop`: 消费者 drop 后,guard 触发 `AbortHandle::abort()` 取消算法 task

**Driver 设计**:
```rust
pub struct Driver {
    step_tx: mpsc::Sender<Result<Step>>,
    algorithm: String,
}
```

- `call_model(request, models)` → 通过 `mpsc::channel(1)` 发送 `Step::CallModel`,等待 `oneshot` 返回
- 容量 1 保持背压,宿主消费后才继续
- `finish(result)` 发送 `Step::Done` 终止流

**Step 流**(`algorithm.rs:211-217`):
```rust
pub enum Step {
    CallModel(Box<CallModel>),  // 算法请求宿主执行 LLM 调用
    Done(Box<RoutingOutcome>),  // 路由完成
}
```

**`drive` 函数**(`algorithm.rs:231-267`):
```rust
pub async fn drive<F, Fut>(
    algorithm: Arc<dyn Algorithm>,
    request: Request,
    serve: F,
) -> Result<RoutingOutcome>
where
    F: Fn(CallModel) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let stream = algorithm.run_stream(request);
    tokio::pin!(stream);
    let mut in_flight = futures::stream::FuturesUnordered::new();
    let mut final_outcome: Option<RoutingOutcome> = None;
    loop {
        tokio::select! {
            Some(result) = in_flight.next() => match result {
                Ok(()) => {},
                Err(err) => return Err(err),
            },
            step = stream.next() => {
                match step {
                    None => break,
                    Some(item) => match item? {
                        Step::CallModel(call) => in_flight.push(serve(*call)),
                        Step::Done(outcome) => {
                            final_outcome = Some(*outcome);
                            break;
                        }
                    }
                }
            },
        }
    }
    final_outcome.ok_or(LibsyError::MissingFinalResponse)
}
```

**并发机制**:
- `FuturesUnordered` 持有所有 in-flight 的 `serve(call)` 任务
- `tokio::select!` 同时监听 `in_flight.next()` 和 `stream.next()` —— 算法可一次 emit 多个 `CallModel`,serve 这边按 futures unordered 并发执行,hedging/fan-out 自然实现
- `Step::Done` 终止循环,`in_flight` 里的悬挂任务会被 drop(在 `Drop` guard `AbortOnDrop` 里 abort)
- 测试 `requests_are_processed_in_parallel`(`algorithm.rs:739-787`)用 12 个 worker + `Barrier` 证明共享算法是真正并行而非串行

### 5.2 7 种算法总览

| 算法 | 文件 | 描述 |
|------|------|------|
| `Noop` | `noop.rs` | 不调用模型,直接返回 OK |
| `Passthrough` | `passthrough.rs` | 单目标直通,支持 subagents 粘性 |
| `Random` | `rand.rs` | 加权随机分流(A/B 测试),可选 seed |
| `LlmTaskClassifier` | `llm_class.rs` | LLM 判断(Capability/Escalation/Custom 三模式) |
| `StageRouter` | `stage.rs` | 信号驱动的阶段路由(ToolSignalProcessor + StageClassifier) |
| `CompositeRouter` | `composite.rs` | 组合路由(LLM 判断 + Stage) |
| `SubagentRouter` | `subagent.rs` | 子 Agent 路由(AffinityRouter 粘性) |
| `AdvisorGate` | `advisor_gate.rs` | 执行者 + 审查者门控 |

### 5.3 LLM Classifier(三模式)

**文件**: `crates/libsy/src/algorithms/llm_class.rs:218-244`

```rust
pub enum LlmClassifierConfig {
    Capability { judge_target, efficient_target, capable_target, config },
    Escalation { judge_target, efficient_target, capable_target, contract, config, max_output_tokens },
    Custom { judge_target, targets, default_target, config },
}
```

**Capability 模式核心**(`llm_class.rs:547-571`):

```rust
impl JudgePolicy for TaskClassifierPolicy {
    type Verdict = TaskClassifierVerdict;

    fn to_classification(&self, verdict: Option<&Self::Verdict>) -> Classification {
        let Some(verdict) = verdict.filter(|verdict| verdict.is_valid()) else {
            return Classification::Ambiguous(vec![]);
        };
        let Some(threshold) = self.threshold(verdict) else {
            return Classification::Ambiguous(vec![]);
        };
        let target = if verdict.p_solve >= threshold
            || (threshold - verdict.p_solve).abs() <= f64::EPSILON
        {
            &self.efficient_target
        } else {
            &self.capable_target
        };
        Classification::Scores(vec![Score {
            target: target.clone(),
            confidence: 1.0,
        }])
    }
}
```

**boundary_steps 阈值公式**(`llm_class.rs:75-83, 213-215`):

```rust
fn boundary_steps(&self) -> Option<u8> {
    match self.capability_boundary.as_str() {
        "supported" => Some(0),
        "uncertain" | "unmatched" => Some(1),
        "unsupported" => Some(2),
        _ => None,
    }
}

fn threshold(&self, verdict: &TaskClassifierVerdict) -> Option<f64> {
    Some(self.base_threshold + f64::from(verdict.boundary_steps()?) * self.threshold_step)
}
```

- `supported` → threshold = base_threshold(0 步)
- `uncertain` / `unmatched` → threshold = base_threshold + 1·threshold_step(1 步)
- `unsupported` → threshold = base_threshold + 2·threshold_step(2 步)
- `p_solve >= threshold` → efficient target,否则 → capable target

**verdict 验证**(`llm_class.rs:60-73`):

```rust
fn is_valid(&self) -> bool {
    (0.0..=1.0).contains(&self.p_solve)
        && !self.crux.trim().is_empty()
        && matches!(
            (self.primary_rule.as_str(), self.capability_boundary.as_str()),
            ("SUP-1" | "SUP-2" | "SUP-3" | "SUP-4" | "SUP-5", "supported")
                | ("UNC-1" | "UNC-2", "uncertain")
                | ("LIM-1" | "LIM-2", "unsupported")
                | ("none", "unmatched")
        )
}
```

**任务消息裁剪**(`llm_class.rs:95-149`):
- `trim_messages(messages, recent_turn_window)`: 保留 system + developer + 第一个 user + 最近 N 轮
- `window_start` 算法核心: 从 newest-to-oldest 扫描,维护 `unpaired: HashSet<&str>`(ToolResult 的 call_id),遇到 ToolCall 就从 set 移除(说明该 call 已被 result 配对).保证裁剪后的窗口里每个 ToolResult 都有对应的 ToolCall

**TaskInput 注入路由指令**(`llm_class.rs:46-47, 178-186`):

```rust
const TRAILING_ROUTING_INSTRUCTION: &str =
    "Route the conversation above. Output ONLY the routing JSON object, nothing else.";

impl ClassifierInput for TaskInput {
    fn build_messages(&self, _state: &State, request: &Request) -> Vec<Message> {
        let mut messages = match self.recent_turn_window {
            Some(window) => trim_messages(&request.llm_request.messages, window),
            None => task_messages(&request.llm_request.messages),
        };
        if self.recent_turn_window.is_some() {
            messages.push(Message::text(Role::User, TRAILING_ROUTING_INSTRUCTION.to_string()));
        }
        messages
    }
}
```

**Escalation 模式**:
- 先调用 efficient,再用 judge 评判
- 连续 N 次 escalate 判决后 latch 到 capable
- 支持 `recent_turn_window` 控制 judge 可见的对话轮数

**Custom 模式**:
- 用户提供 JSON Schema + TargetSelector 策略
- Judge 输出经 Schema 验证后,用 JSON Pointer 提取目标名

**Validation**(`llm_class.rs:335-375`):
- `base_threshold ∈ [0, 1]`、`threshold_step ≥ 0`、`base_threshold + 2·threshold_step ≤ 1`
- `max_output_tokens ≥ 1`
- `message_hash_fallback` 必须搭配 `classify_trigger = NewSession`

### 5.4 Stage Router(信号驱动)

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

级联顺序:
1. `ToolSignalProcessor` — 提取工具结果信号
2. `StageClassifier` — 基于信号打分(`score_signal`)
3. `LlmTaskClassifier`(可选) — 信号不确定时回退到 LLM 判断
4. `FallOpen` — 最终回退到默认层

### 5.5 Advisor Gate(执行者 + 审查者)

**文件**: `crates/libsy/src/algorithms/advisor_gate.rs`

**核心结构**(`advisor_gate.rs:91-149`):

```rust
pub enum GateTrigger {
    /// First turn without tool calls (subject to `gate_min_tool_results`).
    NoToolCall,
    /// First turn whose visible text matches this regex (searched, not anchored).
    Pattern(String),
}

pub struct AdvisorGateConfig {
    pub reviewer_system_prompt: String,  // include_str!("../prompts/advisor-gate/reviewer-system-prompt.md")
    pub redo_feedback_prefix: String,    // 注入 REDO 反馈前缀
    pub gate_trigger: GateTrigger,
    pub max_reviews: u32,                // 预算作用域
    pub gate_stall_turns: u32,           // 中途 checkpoint
    pub gate_min_tool_results: u32,      // 提前跳过聊天轮次
    pub advisor_max_tokens: u64,
    pub advisor_temperature: Option<f64>,
    pub transcript_max_chars: usize,
    pub fail_open: bool,                 // 顾问失败时默认 APPROVE
}
```

**关键常量**(`advisor_gate.rs:81-89`):
- `MAX_FAILED_CONSULTS = 3`: 失败咨询上限(防止 max_reviews 被悄悄消耗)
- `MAX_TRACKED_SCOPES = 1_024`: 跟踪的作用域上限
- `transcript_max_chars = 200_000`: 转录文本上限
- `BENCH_SESSION_HEADER = "proxy_x_session_id"`: benchmark harness 标头

**工作流**:
1. Executor 回答每个 client 可见轮次
2. 终端轮次(`GateTrigger::NoToolCall` 或 `Pattern(String)`)被缓冲
3. Advisor 审查: `APPROVE` 释放缓冲轮次,`REDO` 把 plan 反馈给 executor 重新生成
4. 每个 scope(bench/session/instance)最多 `max_reviews` 次审查
5. Advisor 故障时 `fail_open`(默认 APPROVE)

**失败处理哲学**(`advisor_gate.rs:18-26`):
> Executor 错误总是传播(包括 ContextWindowExceeded,让客户端看到 400 让 agent 压缩).Advisor 错误遵守 fail_open —— 缓冲轮次作为隐式 APPROVE 通过,退还已消耗的预算,并计入 per-scope 失败上限,超出后停止咨询.

### 5.6 Affinity Router(粘性路由)

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

`ClassifyTrigger` 控制重新分类频率:
- `EveryRequest` — 每个请求都分类(无粘性)
- `NewSession` — 新 session 时分类
- `UserTurn` — 用户轮次时分类

---

## 六、FallThrough 级联

### 6.1 FallThrough<S> 结构

**文件**: `crates/libsy/src/algorithms/fall_through.rs:92-293`

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

### 6.2 核心执行流程

**文件**: `crates/libsy/src/algorithms/fall_through.rs:175-292`

```rust
async fn execute_session(&self, driver: Driver, request: Request) -> Result<RoutingOutcome> {
    let mut request = request;
    let session_state = self.session_state(&request);
    let (target, served) = match session_state {
        Some(state) => {
            let mut state = state.lock().await;
            self.route(&mut state, &driver, &mut request).await?
        }
        None => {
            let mut state = S::default();
            self.route(&mut state, &driver, &mut request).await?
        }
    };
    match served {
        Some(response) => Ok(RoutingOutcome::answered(target, request, response)),
        None => {
            let fallback_models = self.fallbacks(&target);
            Ok(RoutingOutcome::route_to(target, fallback_models, request))
        }
    }
}

async fn route(&self, state: &mut S, driver: &Driver, request: &mut Request)
    -> Result<(ModelId, Option<Response>)> {
    // 1. Processor chain: 把请求侧事实累积到 composition state
    for processor in &self.processors {
        let event = Event::Request { request, driver: Some(driver) };
        processor.process(state, event).await?;
    }

    // 2. Classifier cascade: 第一个给出 Scores 的决定 (argmax)
    let mut routed = None;
    for classifier in &self.classifiers {
        let (scores, response) = classifier.score(state, request, Some(driver)).await?;
        if let Some(score) = scores.argmax(false)? {
            routed = Some((score, Arc::clone(classifier), response));
            break;
        }
    }
    let Some((score, deciding, served)) = routed else {
        return Err(LibsyError::AlgorithmError { message: "every classifier abstained".to_string() });
    };

    // 3. 解析 target 并日志
    algorithm::ensure_model_is_target(&self.targets, &score.target)?;
    let target = score.target.clone();
    let tier = deciding.routing_tier(&target).or_else(|| {
        self.classifiers.iter().find_map(|c| c.routing_tier(&target))
    });
    tracing::info!(algorithm=self.name, target=%score.target, confidence=score.confidence, tier = ?tier, "Model selected");

    // 4. Post-decision replay: 每个 processor 看到选择
    for processor in &self.processors {
        let event = Event::Decision { request, selected_model_id: &target };
        processor.process(state, event).await?;
    }
    Ok((target, served))
}
```

### 6.3 会话状态管理

**文件**: `crates/libsy/src/algorithms/fall_through.rs:222-233, 295-318`

- `session_state()` 取或创建 `Arc<AsyncMutex<S>>`,按 session_id 索引
- `remove_inactive_sessions` 通过 `Arc::strong_count(&session.state) > 1` 判断是否有活跃请求持有 —— 强引用计数 = 1 时说明没人用,可清理
- `cleanup_inactive_sessions` 每 1 小时跑一次,`SESSION_STATE_TTL = 1 hour`,过期 session 释放

**DefaultTarget**(`fall_through.rs:71-87`): 永远 score 0.0,作为 cascade 最后一个"兜底不 abstain"的 classifier

---

## 七、PyO3 绑定

### 7.1 PyO3 绑定设计

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

提供 `PyTaskClassifierConfig`、`PyLlmClassifierConfig`、`PyStageRouterConfig` 等 Python 类,通过 `py_serde.rs` 的 `from_python`/`to_python` 实现 Python ↔ Rust 类型转换。

### 7.2 类型转换

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

通过 serde JSON 作为 Python ↔ Rust 的中介格式,避免为每个类型手写转换。

### 7.3 异步绑定

```rust
#[pyfunction]
fn run_algorithm<'py>(py: Python<'py>, algo: &Bound<'py, PyAny>, request: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    pyo3_asyncio::tokio::future_into_py(py, async move {
        // 驱动算法 step stream
    })
}
```

---

## 八、缓存与状态管理

### 8.1 Session State TTL 清理

Switchyard 通过 `FallThrough<S>` 的 `SessionStates<S>` 管理会话级状态:

| 常量 | 值 | 描述 |
|------|-----|------|
| `SESSION_STATE_TTL` | 1 hour | 会话状态过期时间 |
| `SESSION_CLEANUP_INTERVAL` | 1 hour | 清理任务间隔 |

**清理机制**:
- `remove_inactive_sessions` 通过 `Arc::strong_count(&session.state) > 1` 判断是否有活跃请求持有
- 强引用计数 = 1 时说明没人用,可清理
- `Once::call_once` + `Weak::upgrade` 实现"首次请求启动后台清理任务"模式

### 8.2 Anthropic Prompt Caching

**文件**: `crates/libsy-llm-client/src/client.rs:785-819`

```rust
fn enable_anthropic_prompt_caching(body: &mut Value) {
    if count_cache_control_blocks(body) >= MAX_CACHE_CONTROL_BLOCKS {
        return;
    }
    let Some(content) = body.get_mut("messages")
        .and_then(Value::as_array_mut)
        .and_then(|messages| messages.last_mut())
        .and_then(|message| message.get_mut("content"))
    else { return; };
    match content {
        Value::String(text) => {
            *content = serde_json::json!([{
                "type": "text",
                "text": std::mem::take(text),
                "cache_control": {"type": "ephemeral"}
            }]);
        }
        Value::Array(blocks) => {
            if let Some(block) = blocks.last_mut().and_then(Value::object_mut) {
                block.entry("cache_control".to_string())
                    .or_insert_with(|| serde_json::json!({"type": "ephemeral"}));
            }
        }
        _ => {}
    }
}
```

`MAX_CACHE_CONTROL_BLOCKS = 4`(Anthropic 限额).`cache_control` 标记在最后一个 content block 上。

### 8.3 OpenAI Stream Usage

**文件**: `crates/libsy-llm-client/src/client.rs:822-842`

```rust
fn ensure_openai_stream_usage(body: &mut Value) {
    let Value::Object(object) = body else { return };
    if object.get("stream").and_then(Value::as_bool) != Some(true) { return; }
    match object.get_mut("stream_options") {
        Some(Value::Object(options)) => {
            options.entry("include_usage".to_string())
                .or_insert(Value::Bool(true));
        }
        _ => {
            let mut options = Map::new();
            options.insert("include_usage".to_string(), Value::Bool(true));
            object.insert("stream_options".to_string(), Value::Object(options));
        }
    }
}
```

---

## 九、对 laew 的借鉴

### 9.1 短期(P0)立即借鉴

#### 1. 引入 `crates/protocol` crate 与 IR

**现状**: laew 的 `src/agent/mod.rs::run_session` 直接用 `serde_json::Value`,协议差异渗透到 agent 循环。

**借鉴**:
- 把 `LlmRequest / LlmResponse / ContentBlock::Unknown / PreservationMetadata` 直接搬到 laew `src/agent/protocol/`
- 把统一消息模型 `Message { role: Role, content: Vec<ContentBlock> }` 重构到 laew 当前 `Vec<Message>` 类似的结构
- 立即收益: agent 循环完全不接触协议细节,新增协议(Codex Responses)只需实现 `FormatCodec`

#### 2. 引入 `ContentBlock::Unknown` 兜底

**现状**: laew 的统一消息模型遇 Anthropic `thinking` 块或 OpenAI `reasoning` 块会丢失。

**借鉴**:
```rust
pub enum ContentBlock {
    Text { text: String },
    Reasoning { text: String, signature: Option<String>, details: Vec<Value> },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Refusal { text: String },
    Unknown { provider: FormatId, raw: Value },  // 兜底
}
```

#### 3. 引入 `PreservationMetadata`

**借鉴**:
```rust
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,
    pub responses: BTreeMap<FormatId, Value>,
}
```

翻译失败时可回退原始体重试。

#### 4. 引入指数退避重试

**现状**: laew 的 `LlmClient` 目前没有重试逻辑(`agent/mod.rs::complete` 直接调用一次失败就返回)。

**借鉴**(直接抄到 `src/llm/client.rs`):

```rust
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

fn retry_delay(retry_number: u64, retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or_else(|| {
        let multiplier = 1_u32 << retry_number.min(3);
        INITIAL_RETRY_DELAY.saturating_mul(multiplier).min(MAX_RETRY_BACKOFF)
    })
}
```

- 第 1 次重试: 250ms
- 第 2 次重试: 500ms
- 第 3 次重试: 1000ms
- 第 4+ 次重试: 2000ms(封顶)
- 尊重 `Retry-After` 头,但不超过 60s
- 400 错误根据 body 关键词("prompt is too long" / "maximum context length")分类为 `ContextWindowExceeded`,不重试

#### 5. 引入请求清洗逻辑

**借鉴**: 当用户配置 OpenAI 后端时,应该自动 strip `reasoning_effort`(Anthropic 专属);当用户配置 Anthropic 后端时,应该自动 enable `cache_control` 最多 4 次。

```rust
fn strip_anthropic_incompatible_fields(body: &mut Value) {
    if let Value::Object(object) = body {
        object.remove("reasoning_effort");
        object.remove("context_management");
    }
}
```

### 9.2 中期(P1)借鉴

#### 6. 引入 `Algorithm` trait + `Driver` + `Step` 流

**现状**: `YoloRunner` 硬编码双 Agent 架构。

**借鉴**:
```rust
#[async_trait]
pub trait Algorithm: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn route(self: Arc<Self>, driver: Driver, request: Request) -> Result<RoutingOutcome>;
}

pub async fn drive<F, Fut>(algorithm: Arc<dyn Algorithm>, request: Request, serve: F)
    -> Result<RoutingOutcome> {
    let stream = algorithm.run_stream(request);
    tokio::pin!(stream);
    let mut in_flight = futures::stream::FuturesUnordered::new();
    loop {
        tokio::select! {
            Some(result) = in_flight.next() => { ... }
            step = stream.next() => {
                match step? {
                    Step::CallModel(call) => in_flight.push(serve(*call)),
                    Step::Done(outcome) => break,
                }
            }
        }
    }
    ...
}
```

**收益**: SubAgent-Work 可并发执行,Quality-Check 可与下一个 SubAgent 流水线化。

#### 7. 引入 `FallThrough<S>` 级联模式

**借鉴**:
```rust
pub struct FallThrough<S = ()> {
    processors: Vec<Arc<dyn Processor<S>>>,
    classifiers: Vec<Arc<dyn Classifier<S>>>,
    targets: Vec<ModelId>,
    session_states: Option<Arc<SessionStates<S>>>,
}
```

Yolo 分类 → Plan 决策 → Main-Work 编排 → Quality-Check 审查作为 Classifier 链,第一个给出高置信度的胜出;全部 abstain 时用 DefaultTarget(默认 capable 模型)。

#### 8. 引入 `LLM Classifier` 三模式(Capability / Escalation / Custom)

**借鉴**:
- `Capability` 模式: judge 输出 `{crux, primary_rule, capability_boundary, p_solve}`,threshold 公式 `base_threshold + boundary_steps * threshold_step`
- `Escalation` 模式: 先 efficient,连续 N 次 escalate 判决后 latch 到 capable
- `recent_turn_window` + `trim_messages` + `window_start` 控制 judge 可见的对话轮数
- `recent_turn_window.is_some()` 时追加 `TRAILING_ROUTING_INSTRUCTION` 避免 LLM 把对话当成任务回答

#### 9. 引入 W3C Trace Context 传播

**借鉴**:
```rust
pub fn request_span(headers: &HeaderMap) -> tracing::Span {
    let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
    let span = tracing::info_span!("laew.request", otel.kind = "server");
    span.set_parent(parent);
    span
}
```

#### 10. 引入 Prometheus 指标体系

**借鉴**:
- `seed_outcome_metrics` 预注册所有可能的指标值
- 暴露 `/metrics` 端点,监控 LLM 调用延迟、路由开销、错误率
- OpenInference 语义约定 `openinference.span.kind = "CHAIN"` 与 Phoenix/Arize 等观测平台兼容

### 9.3 长期(P2)借鉴

#### 11. 引入 `AdvisorGate` 精细化 QC

**借鉴**:
- `GateTrigger::NoToolCall / Pattern(String)` 让 QC 在特定条件触发
- `max_reviews` 预算防止无限重试
- `fail_open: bool` 顾问失败时默认 APPROVE
- `MAX_FAILED_CONSULTS = 3` 防止顾问持续故障时消耗预算

#### 12. 引入 `AffinityRouter` 粘性路由

**借鉴**:
```rust
pub struct AffinityRouter {
    assignments: HashMap<RoutingIdentity, ModelId>,
    release_on_user_turn: bool,
    message_hash_fallback: bool,
}

pub enum RoutingIdentity {
    Session(String),
    Subagent { session: String, agent: String },
}
```

Session 级别的粘性路由,同一 session 的请求路由到同一模型,提高缓存命中率。

#### 13. 引入 `StageRouter` 信号驱动路由

**借鉴**: `ToolSignalProcessor` + `StageClassifier`,基于工具调用结果信号决定下一步,无需额外 LLM 调用。

#### 14. 引入 `SkillDistiller` 数据模型

**借鉴**: `Trajectory` + `SkillCandidate` + `SkillProvenance` 数据模型,支持可追溯的技能生成与验证。

- 每次 SubAgent-Work 完成后写入 Trajectory
- 定期调用 SkillDistiller 蒸馏
- 激活前 SkillValidator 校验
- `DistillationRequest::validate_candidate` 的"provenance 必须在 request 内" + "parent_version 必须匹配 base_skill" 双约束值得借鉴,防止训练-推理漂移

### 9.4 核心 Rust 技巧清单

Switchyard 大量运用 Rust 高级特性,值得 laew 学习:

| 技巧 | 用途 | 文件位置 |
|------|------|----------|
| `Arc<dyn Algorithm>` + `mpsc::channel<Result<Step>>` | "宿主驱动算法"的反向控制流 | `algorithm.rs:353-404` |
| `FuturesUnordered` + `tokio::select!` | 多 CallModel 并发服务,支持 hedging/fan-out | `algorithm.rs:231-267` |
| `#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]` | 配置多态反序列化 | `config.rs` |
| `#[tracing::instrument(skip_all, fields(...))]` | OpenInference 语义约定 span,字段后期填充 | `observability.rs` |
| `tracing::field::Empty` + `span.record(...)` | span 字段的延迟写入 | `observability.rs` |
| `AsyncUnwindSafe` + `AssertUnwindSafe` + `catch_unwind` | 算法 panic 捕获,避免 detached task 静默失败 | `algorithm.rs:353-404` |
| `abort_guard: AbortOnDrop` | "流 drop 时 task abort"的生命周期管理 | `algorithm.rs:353-404` |
| `Once::call_once` + `Weak::upgrade` | "首次请求启动后台清理任务"模式 | `fall_through.rs` |
| `Arc::strong_count(&session.state) > 1` | 通过引用计数判断 session 是否仍被使用 | `fall_through.rs` |
| `tokio::select!` 监听 Ctrl-C + SIGTERM | 跨平台优雅关闭 | `lib.rs:386-404` |

---

## 附录 A:HTTP 服务器层(switchyard-server)

### A.1 Axum 路由构建

**文件**: `crates/switchyard-server/src/lib.rs:471-493`

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

### A.2 请求处理主函数

**文件**: `crates/switchyard-server/src/lib.rs:652-714`

**`handle_llm_request` 流程**(`lib.rs:791-848`):
1. `cache_probe = state.track_cache_eligibility.then(|| prefix_probe(&body))` — 缓存资格探测
2. `resolve_route(&state, metadata, body, wire_format)` — 解析路由
3. `observer = stats_observer(state.stats.clone(), ...)` — 注册观察者
4. `route.execute(request, Some(observer)).await` — 执行算法
5. `into_http_response(response, wire_format, response_model, request_extensions)` — 编码回 wire format
6. `attach_routing_headers(&mut response, served_model.as_str())` — 添加 `x-model-router-selected-model` 头
7. `render_error_response(response, wire_format)` — 错误按 wire_format 渲染

### A.3 错误处理

**文件**: `crates/switchyard-server/src/lib.rs:957-1007`

```rust
fn client_error(error: &LlmClientError) -> Response {
    match error {
        LlmClientError::InvalidRequest { message } | LlmClientError::RequestTranslation(message) => 
            error_response(StatusCode::BAD_REQUEST, message, "invalid_request_error", "invalid_request_error"),
        LlmClientError::Configuration { message } => 
            error_response(StatusCode::BAD_GATEWAY, message, "upstream_error", "upstream_configuration_error"),
        LlmClientError::ContextWindowExceeded { message, .. } => 
            error_response(StatusCode::BAD_REQUEST, message, "invalid_request_error", "context_length_exceeded"),
        LlmClientError::UpstreamHttp { status, body } => 
            error_response(*status, upstream_error_message(body), "upstream_error", "upstream_error"),
        LlmClientError::Transport { source } | LlmClientError::InvalidResponse { source } => 
            error_response(StatusCode::BAD_GATEWAY, source.to_string(), "upstream_error", "upstream_error"),
        LlmClientError::ResponseTranslation(message) => 
            error_response(StatusCode::BAD_GATEWAY, message, "upstream_error", "upstream_error"),
        LlmClientError::Timeout { source } => 
            error_response(StatusCode::GATEWAY_TIMEOUT, source.to_string(), "upstream_error", "upstream_timeout"),
        LlmClientError::RequestEncoding(message) => server_error(message),
        _ => server_error(error.to_string()),
    }
}
```

**错误形状按 wire_format 渲染**(`lib.rs:1046-1069`):
```rust
fn into_response(self, wire_format: WireFormat) -> Response {
    let body = match wire_format {
        WireFormat::AnthropicMessages => json!({
            "type": "error",
            "error": {
                "type": anthropic_error_type(self.status),
                "message": self.message.clone(),
            }
        }),
        WireFormat::OpenAiChat | WireFormat::OpenAiResponses => json!({
            "error": {
                "message": self.message.clone(),
                "type": self.error_type,
                "code": self.code,
            }
        }),
    };
    ...
}
```

### A.4 优雅关闭

**文件**: `crates/switchyard-server/src/lib.rs:386-404`

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
            tracing::info!(?timeout, "shutdown signal received; draining active requests");
            handle.graceful_shutdown(Some(timeout));  // 默认 30s
            server.await.map_err(server_io_error)
        }
    }
}
```

`DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT = Duration::from_secs(30)`(`lib.rs:59`).

---

## 附录 B:LLM 客户端层(libsy-llm-client)

### B.1 TranslatingLlmClient 结构

**文件**: `crates/libsy-llm-client/src/client.rs`

```rust
pub struct TranslatingLlmClient {
    model_to_config: HashMap<ModelId, ModelConfig>,
    client: reqwest::Client,
    forward_auth_client: reqwest::Client,  // 禁用重定向,防止凭证泄露
}
```

**双 Client 设计**:
- `client` — 普通请求,允许重定向
- `forward_auth_client` — `redirect(Policy::none())`,用于 forward_auth 模式,防止凭证泄露到第三方

### B.2 send_encoded 流程

**文件**: `crates/libsy-llm-client/src/client.rs:203-289`

1. `encode_request(&llm_request, wire_format)` — IR → wire
2. `set_json_model(&mut body, model)` — 强制覆盖 model 字段
3. `strip_anthropic_incompatible_fields` — 移除 `reasoning_effort`、`context_management`
4. `strip_unsigned_thinking_blocks` — 移除无签名的 thinking 块(Bedrock 需要)
5. `merge_extra_body` — 合并目标默认值(不覆盖调用者提供的)
6. `enable_anthropic_prompt_caching` — 在最后消息添加 `cache_control: {type: "ephemeral"}`(最多 4 个)
7. `ensure_openai_stream_usage` — 流式请求添加 `stream_options: {include_usage: true}`

### B.3 指数退避重试

**文件**: `crates/libsy-llm-client/src/client.rs:54-56, 587-624`

```rust
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

fn retry_delay(retry_number: u64, retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or_else(|| {
        let multiplier = 1_u32 << retry_number.min(3);  // 1, 2, 4, 8
        INITIAL_RETRY_DELAY.saturating_mul(multiplier).min(MAX_RETRY_BACKOFF)
    })
}
```

**可重试条件**:
- `Transport` / `Timeout` 错误 → 可重试
- `UpstreamHttp` 状态码 → `is_retryable_http_status` 判断(429, 500, 502, 503, 504)
- 400 + `is_context_overflow` → `ContextWindowExceeded`(不可重试)

### B.4 RunObserver

```rust
pub enum RunObservation {
    AnswerCall(LlmCallObservation),  // 最终回答调用
    LlmCall(LlmCallObservation),     // 分类器/Judge 调用
    RoutingOverhead(Duration),       // 路由开销
}

pub type RunObserver = Arc<dyn Fn(RunObservation) + Send + Sync>;
```

宿主通过 observer 接收调用事件,用于统计/日志,不干扰算法逻辑。

---

## 附录 C:可观测性

### C.1 Prometheus 指标体系

**文件**: `crates/switchyard-server/src/metrics.rs`

| 指标 | 类型 | 描述 |
|------|------|------|
| `switchyard.upstream_attempts` | Counter | 上游尝试次数(按 outcome/code 分类) |
| `switchyard.client_responses` | Counter | 客户端响应(success/retryable_error/other_error) |
| `switchyard.model_call_latency_ms` | Histogram | 模型调用延迟 |
| `switchyard.total_latency_ms` | Histogram | 端到端延迟 |
| `switchyard.routing_overhead_ms` | Histogram | 路由开销 |
| `switchyard.router_retry_recovered` | Counter | 路由重试恢复次数 |
| `switchyard.build_info` | Gauge | 构建信息(版本号) |

**直方图桶边界**(`metrics.rs:17-27`):
```rust
const ROUTING_OVERHEAD_BUCKETS_MS: &[f64] = &[
    0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0,
];
const LLM_LATENCY_BUCKETS_MS: &[f64] = &[
    0.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0,
    1000.0, 2500.0, 5000.0, 7500.0, 10_000.0, 15_000.0, 30_000.0, 60_000.0, 120_000.0, 300_000.0,
];
```

`seed_outcome_metrics`(`metrics.rs:113-134`): 预注册所有可能的状态码(200/404/429/500/504/None),便于仪表盘在首次命中前就显示指标。

### C.2 OpenTelemetry 集成

**文件**: `crates/switchyard-server/src/observability.rs`

**OTLP 启用条件**:
- `OTEL_SDK_DISABLED` 不为 true
- `OTEL_{SIGNAL}_EXPORTER` 不包含 `otlp`(如果显式禁用 OTLP)
- `OTEL_EXPORTER_OTLP_ENDPOINT` 或 `OTEL_EXPORTER_OTLP_{SIGNAL}_ENDPOINT` 已设置

### C.3 W3C Trace Context 传播

**文件**: `crates/switchyard-server/src/observability.rs:51-73`

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

**测试**(`observability.rs:172-199`): 注入 `traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01` + `tracestate: vendor=opaque-value`,验证 `span_context.trace_id() == "4bf92f3577b34da6a3ce929d0e0e4736"`,且 `tracestate` 被保留。

### C.4 OpenInference 语义约定

**文件**: `crates/libsy/src/observability.rs:68-110`

```rust
pub(crate) fn run_span(algorithm: &str, request: &Request) -> Span {
    let span = tracing::info_span!(
        target: TRACING_TARGET,
        "libsy.run",
        algorithm,
        switchyard.algorithm = algorithm,
        openinference.span.kind = "CHAIN",
        switchyard.route = tracing::field::Empty,
        session_id = tracing::field::Empty,
        session.id = tracing::field::Empty,
        agent_id = tracing::field::Empty,
        task_id = tracing::field::Empty,
        task_kind = tracing::field::Empty,
        agent_role = tracing::field::Empty,
        correlation_id = tracing::field::Empty,
        extra_metadata = tracing::field::Empty,
        outcome = tracing::field::Empty,
        error = tracing::field::Empty,
    );
    // ...
    span
}
```

---

## 附录 D:运行器层(switchyard-runner)

### D.1 TOML 配置系统

**文件**: `crates/switchyard-runner/src/config.rs`

完整配置示例:

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

### D.2 AlgorithmSpec 多态枚举

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
- `deny_unknown_fields` 拒绝未知字段,避免配置错误
- `rename_all = "snake_case"` 匹配 TOML 风格

### D.3 环境变量

| 变量 | 描述 |
|------|------|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP 导出端点 |
| `OTEL_SERVICE_NAME` | 服务名称 |
| `OTEL_SDK_DISABLED` | 禁用 OpenTelemetry |
| `RUST_LOG` | 日志级别过滤 |

---

## 附录 E:技能蒸馏(switchyard-skill-distillation)

### E.1 Trajectory 结构体

**文件**: `crates/switchyard-skill-distillation/src/model.rs:129-150`

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trajectory {
    pub schema_version: u16,                  // SCHEMA_VERSION = 1
    pub id: SkillEvidenceId,                  // 证据 ID(去重 + 追溯)
    pub task: TaskDescriptor,                 // { description, task_id?, metadata }
    pub execution: ExecutionMetadata,         // { harness?, model?, started_at?, ended_at? }
    pub source: TrajectorySourceInfo,         // { kind, id?, metadata }
    pub events: Vec<TrajectoryEvent>,         // 有序事件
    pub outcome: Option<TrajectoryOutcome>,   // { label?, score?, error?, metrics }
    pub metadata: Metadata,                   // BTreeMap<String, Value>
}
```

**TrajectoryEventKind**(`model.rs:70-86`): `Message / ToolCall / ToolResult / Observation / Error / FinalOutput`,`#[non_exhaustive]` 允许扩展。

**Validation**(`model.rs:152-172`): 检查 schema_version == SCHEMA_VERSION,task.description 非空,event sequence 连续从 0 开始,outcome.score 有限(`is_finite()`)。

### E.2 SkillCandidate 结构体

**文件**: `crates/switchyard-skill-distillation/src/model.rs:354-371`

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillCandidate {
    pub schema_version: u16,
    pub namespace: SkillNamespace,
    pub version: SkillVersionId,
    pub skill_md: String,                       // Portable Agent Skills document
    pub provenance: SkillProvenance,             // { source_evidence_ids, parent_version?, generator?, generated_at }
    pub validation: Option<ValidationReport>,
    pub metadata: Metadata,
}
```

**SkillProvenance**(`model.rs:266-282`): 记录来源轨迹 ID、父版本(用于增量更新)、生成器、时间戳。

**ValidationReport**(`model.rs:314-329`): `status: ValidationStatus`(`Passed/Failed/NeedsReview`) + `checks: Vec<ValidationCheck>` + `metrics: BTreeMap<String, f64>` + `notes: Vec<String>` + `evaluated_at`。

### E.3 端口抽象

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
- 端口与实现分离,允许替换轨迹源(本地运行器/基准测试导入器)、蒸馏器(LLM/规则)、验证器、存储
- `SkillProvenance` 记录技能的来源轨迹,实现可追溯性
- `ActivationRecord` 记录技能激活历史,支持回滚

### E.4 验证机制

**DistillationRequest::validate_candidate**(`model.rs:226-262`):

```rust
pub fn validate_candidate(&self, candidate: &SkillCandidate) -> Result<()> {
    self.validate()?;
    candidate.validate()?;
    if candidate.namespace != self.namespace { return Err(...); }

    let request_ids: HashSet<_> = self.trajectories.iter()
        .map(|trajectory| &trajectory.id).collect();
    if candidate.provenance.source_evidence_ids.iter()
        .any(|id| !request_ids.contains(id)) {
        return Err(invalid_record("skill candidate",
            "provenance references skill evidence outside the request"));
    }

    let expected_parent = self.base_skill.as_ref().map(|skill| &skill.version);
    if candidate.provenance.parent_version.as_ref() != expected_parent {
        return Err(invalid_record("skill candidate",
            "parent version does not match the request base skill"));
    }
    Ok(())
}
```

---

## 附录 F:预填充路由(prefill-router)

### F.1 PrefillForward trait

**文件**: `crates/prefill-router/src/lib.rs`

```rust
pub trait PrefillForward: Send {
    fn forward(&mut self, request: &ForwardRequest) -> Result<ForwardOutput>;
    fn unload(&mut self) -> Result<()>;
}
```

**设计思路**:
- trait 不含 Python 类型,允许 Candle 等纯 Rust 实现替换
- `unload()` 显式释放模型资源

### F.2 TransformersForward

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
- `LayerSelection::UpperHalf` 提取上半层隐藏状态(与 NVIDIA LLM Router 参考实现一致)

### F.3 ForwardRequest / ForwardOutput

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
- 与 NVIDIA LLM Router 参考实现一致,提取模型后半段的隐藏状态作为路由特征

---

## 附录 G:测试体系

### G.1 单元测试

每个 crate 内嵌 `#[cfg(test)] mod tests`,覆盖:
- IR 序列化/反序列化(`serde_uses_python_friendly_dictionary_shapes`)
- 流式折叠(`folds_text_usage_and_stop_reason`)
- 工具调用参数拼接(`assembles_tool_calls_by_index`)
- 头解析优先级(`sy_header_resolves_paths_in_order_and_descends_into_json`)
- 子 Agent 路由信号(`subagent_routing_honors_explicit_signals_and_delegated_work_kinds`)

### G.2 集成测试

`crates/libsy/src/core/testing.rs` 提供 `test_drive` 辅助函数:

```rust
async fn test_drive<F, Fut>(algorithm: Arc<dyn Algorithm>, request: Request, serve: F) -> Result<(ModelId, Response)>
where F: Fn(ModelId, Request) -> Fut, Fut: Future<Output = Result<Response>>
```

`serve` 闭包模拟 LLM 调用,用于测试算法逻辑。

### G.3 Soak 测试

`crates/switchyard-soak/tests/soak.rs` 启动真实 Axum 服务器,模拟后端:

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

测试覆盖: 健康检查、指标、模型列表、chat/messages/responses 端点、错误处理。

### G.4 基准测试

`benchmark/` 目录包含性能基准测试,用于测量路由开销、翻译延迟等。

---

## 总结

Switchyard 是 NVIDIA 在 LLM 流量代理领域的 Rust 实践,其核心优势在于:

1. **分层架构清晰**: protocol/translation/libsy/llm-client/server/runner 六层分离,每层职责单一,通过 trait 抽象层间接口
2. **Provider-neutral IR**: `LlmRequest`/`LlmResponse` IR 隔离协议差异,`ContentBlock::Unknown` 变体保证无损往返,`PreservationMetadata` 保留原始体用于同格式重放
3. **可组合路由算法**: `Algorithm` trait + `FallThrough` 级联,支持 7 种算法自由组合,`Driver` + `Step` 流实现并发 hedging/fan-out
4. **生产级可观测性**: Prometheus + OpenTelemetry 双轨,W3C Trace Context 传播,OpenInference 语义约定
5. **配置驱动**: TOML 配置 + serde 反序列化,算法选择无需改代码
6. **技能蒸馏**: 端口与实现分离,Trajectory/SkillCandidate 数据模型支持可追溯的技能生成与验证
7. **预填充路由**: `PrefillForward` trait + Transformers 嵌入,提取隐藏状态作为路由特征

对 laew 的启示:
- **短期(P0)**: 引入 `protocol` crate 和 IR,简化协议差异处理;引入 `ContentBlock::Unknown` 无损保留;引入 Prometheus 指标;引入指数退避重试
- **中期(P1)**: 将多 Agent 架构抽象为 `Agent` trait + 级联编排;引入 `GateTrigger` 和 `max_reviews` 优化 QC;引入 W3C Trace Context 传播
- **长期(P2)**: 引入 `Algorithm` trait 的 Step 流实现并发 SubAgent;引入信号驱动路由;引入 Advisor Gate 精细化 QC;引入 SkillDistiller 数据模型

Switchyard 的代码质量、文档完整度和工程成熟度都较高,是 laew 在 Rust 实现、协议翻译、路由算法三个方向上的重要参考。

---

> **原始文档保留**: `Switchyard-源码调研.md`、`Switchyard-深度分析.md`、`Switchyard-核心机制深度分析.md` 三份原始文件未删除,仍可独立查阅。
