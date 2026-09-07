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

---

## 第八轮深挖 — 协议中立 IR + Codec Registry + OTEL 全链路精细度 + SSE 多协议分帧 + RouteErrorKind 12 类稳定分类

> 调研时间：2026-09-07。第八轮在第七轮基础上补充 Switchyard（NVIDIA 出品的 LLM 网关）的真实实现。所有引用路径均为绝对路径 + 行号。

### 1. 整体架构（补充）

`crates/` 14 个 crate：
- `protocol/`（Request / Response / Metadata / WireFormat）
- `libsy/`（algorithm 抽象 + observability）
- `libsy-llm-client/`（backend trait）
- `switchyard-server/`（Axum + observability + shutdown + SSE + stats）
- `switchyard-translation/`（registry-backed 双向 codec + diagnostic）
- `switchyard-runner/`（Driver + failure 分类）
- `switchyard-skill-distillation/`（技能蒸馏）
- `switchyard-nemo-relay-plugin/`（动态库 plugin）
- `prefill-router/`（路由算法）
- `switchyard-soak/`（压测）
- `switchyard-py/`（Python 绑定）

`switchyard/` CLI；`switchyard_rust/` 镜像；`experimental/` 实验性算法。

### 2. 第八轮 8 维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Telemetry** | `libsy/src/observability.rs:1-110` | OTEL 全链路 + `tracing` span；`switchyard-server/src/observability.rs:1-72` `initialize_observability` 全局 OnceLock + `request_span` W3C trace context 父链接 |
| **Session 持久化** | `switchyard-server/src/routing_log.rs:24-100+` | `RoutingLog` 追加 JSONL 持久化每请求；`snapshot()` 从 JSONL 重建 session 统计 |
| **Tool 权限** | `protocol/src/llm.rs` `Tool` 抽象；`switchyard-translation/src/policy.rs` `TranslationPolicy` |
| **LSP/Hook** | `libsy/src/algorithms/advisor_gate.rs`、`advisor_gate/telemetry.rs` | advisor 拦截算法；`passthrough.rs`、`fall_through.rs`、`stage.rs` 算法 pipeline |
| **Skill 一等公民** | `switchyard-skill-distillation/src/model.rs` 技能蒸馏模型；`libsy/src/prompts/` 提示词库 |
| **多租户** | `protocol/src/metadata.rs` `Metadata.session_id/agent_id/task_id/role/correlation_id/extra_metadata` 多租户标签 |
| **TUI 渲染** | `libsy/src/algorithms/rand.rs` 随机算法可视化（间接）；`switchyard-soak/` 压测报告 |

### 3. 第九轮新维度真实代码锚点

| 维度 | 路径 | 范式要点 |
|---|---|---|
| **Crash/Recovery** | `switchyard-runner/src/failure.rs:1-100+` | `RouteErrorKind` 12 类稳定分类 + `RouteErrorPhase` + `RouteErrorSummary` "safe structured summaries for telemetry, NOT client-facing rendering" |
| **OAuth** | `libsy-llm-client/src/backend.rs` 后端抽象；`switchyard-nemo-relay-plugin/src/lib.rs` 动态库 plugin runtime |
| **i18n** | `mkdocs_hooks.py` 文档国际化钩子；`switchyard-translation/src/codecs/` 多 codec registry |
| **Release** | `Cargo.toml` workspace；`CHANGELOG.md`、`DEVELOPMENT.md`、`INSTALLATION.md`；`Dockerfile`；`pyproject.toml` Python binding |
| **WS/SSE** | `switchyard-server/src/sse.rs:1-100+`、`translation/src/sse.rs` | `frame_stream` 把 `RawEventStream` 转 Axum `Sse<Event>`；`frame_event` 按 target_format 分发（OpenAI `[DONE]` / Anthropic `event=type` / Responses `event=type`） |
| **Dev Container** | `Dockerfile`、`docker-compose`；`crates/switchyard-soak/` 隔离压测环境 |
| **CRDT** | `switchyard-server/src/stats/accumulator.rs` 流式累加器；`switchyard-translation/src/codecs/stream.rs` 流式状态机 |

### 4. 关键代码片段

#### 4.1 OpenTelemetry 全链路 + tracing 字段自动绑定（`libsy/src/observability.rs:61-110`）

```rust
/// Span covering one algorithm run (the whole `route` execution).
/// Correlation ids from the request Metadata are recorded as span fields when present.
/// `tracing` spans cannot grow field names at runtime, so arbitrary host labels ride in
/// via Metadata::extra_metadata, recorded whole into the `extra_metadata` field.
pub(crate) fn run_span(algorithm: &str, request: &Request) -> Span {
    let span = tracing::info_span!(
        target: TRACING_TARGET, "libsy.run", algorithm,
        switchyard.algorithm = algorithm,
        openinference.span.kind = "CHAIN",
        switchyard.route = tracing::field::Empty,
        session_id = tracing::field::Empty,
        session.id = tracing::field::Empty,
        agent_id = tracing::field::Empty,
        task_id = tracing::field::Empty,
        correlation_id = tracing::field::Empty,
        extra_metadata = tracing::field::Empty,
        outcome = tracing::field::Empty,
        error = tracing::field::Empty,
    );
    if let Some(route) = request.model_id() { span.record("switchyard.route", route.as_ref()); }
    if let Some(metadata) = &request.metadata {
        for (field, value) in [("session_id", &metadata.session_id), ...] {
            if let Some(value) = value { span.record(field, value.as_str()); }
        }
    }
    span
}
```

#### 4.2 Server Observability + W3C trace context 父链接（`switchyard-server/src/observability.rs:50-72`）

```rust
/// Creates the server request span with any incoming W3C trace context as its parent.
pub(crate) fn request_span(headers: &HeaderMap) -> tracing::Span {
    let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
    let span = tracing::info_span!(
        target: "switchyard_server", "switchyard.request",
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
    fn keys(&self) -> Vec<&str> { self.0.keys().map(|name| name.as_str()).collect() }
}
```

#### 4.3 WireFormat 注册表 + codec 路由（`switchyard-translation/src/engine.rs:46-80`）

```rust
#[derive(Default)]
pub struct FormatRegistry {
    codecs: BTreeMap<FormatId, Arc<dyn FormatCodec>>,
}
impl FormatRegistry {
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register(OpenAiChatCodec);
        registry.register(AnthropicMessagesCodec);
        registry.register(OpenAiResponsesCodec);
        registry
    }
    pub fn register(&mut self, codec: impl FormatCodec + 'static) {
        self.codecs.insert(codec.format(), Arc::new(codec));
    }
    pub fn codec(&self, format: impl Into<FormatId>) -> Result<Arc<dyn FormatCodec>> {
        let format = format.into();
        self.codecs.get(&format).cloned()
            .ok_or_else(|| TranslationError::Other(format!("no codec registered for {format}")))
    }
}
```

#### 4.4 SSE 多协议分帧 + [DONE] 哨兵（`switchyard-server/src/sse.rs:17-78`）

```rust
/// Converts translated JSON events into endpoint-specific SSE frames.
pub(crate) fn frame_stream(stream: RawEventStream, target_format: WireFormat) -> Sse<SseFrameStream> {
    let framed = async_stream::stream! {
        let mut stream = stream;
        let mut failed = false;
        while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
            let event = match item {
                Ok(value) => match frame_event(target_format, value) { ... },
                Err(LlmStreamError::Upstream(value)) => { /* upstream's own error event, forward verbatim */ }
                Err(LlmStreamError::Client(error)) => { ... }
            };
            yield Ok(event);
            if failed { break; }
        }
        // [DONE] is the OpenAI Chat success sentinel: clients stop reading there
        if !failed && target_format == WireFormat::OpenAiChat {
            yield Ok(Event::default().data("[DONE]"));
        }
    };
    Sse::new(Box::pin(framed) as SseFrameStream)
}
fn frame_event(target_format: WireFormat, value: Value) -> Result<Event, axum::Error> {
    match target_format {
        WireFormat::OpenAiChat => Event::default().json_data(value),
        WireFormat::AnthropicMessages | WireFormat::OpenAiResponses => {
            let event_type = value.get("type").and_then(Value::as_str).unwrap_or("message").to_string();
            Event::default().event(event_type).json_data(value)
        }
    }
}
```

#### 4.5 优雅关闭（多平台 signal）（`switchyard-server/src/shutdown.rs:1-52`）

```rust
/// Waits for the platform's normal process termination signal.
pub(crate) async fn signal() { platform::signal().await; }
async fn ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %error, "ctrl-c shutdown signal unavailable; continuing without shutdown trigger");
        std::future::pending::<()>().await;
    }
}
#[cfg(unix)] mod platform {
    pub(super) async fn signal() {
        tokio::select! { _ = super::ctrl_c() => {}, _ = terminate() => {} }
    }
    async fn terminate() {
        let mut signal = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                tracing::warn!(...); std::future::pending::<()>().await; return;
            }
        };
        signal.recv().await;
    }
}
```

#### 4.6 路由错误分类（专为 telemetry 设计）（`switchyard-runner/src/failure.rs:18-80`）

```rust
#[non_exhaustive]
#[derive(Clone, Copy, Debug, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum RouteErrorKind {
    UpstreamHttp, ContextWindowExceeded, Timeout, Transport,
    InvalidResponse, RequestTranslation, RequestEncoding,
    ResponseTranslation, InvalidRequest, Configuration, Algorithm, Other,
}
/// When a terminal failure occurred relative to response delivery.
pub enum RouteErrorPhase { BeforeResponse, DuringStream }
```

### 5. 设计哲学

Switchyard 是 **「协议中立 + OpenTelemetry 一等公民」** 的工业级 LLM 网关典范：

1. **最核心创新是 protocol 中立 IR**：把 OpenAI Chat / Anthropic Messages / OpenAI Responses 三套协议先解码成统一的 `LlmRequest`/`AggLlmResponse`，再编码成目标格式，**任何双向转换都自动获得 N² → 2N 的代码减少**。`FormatRegistry` 是动态可扩展的 —— 第三方可以 `register(MyCustomCodec)` 添加新协议。
2. **OpenTelemetry 的精细度极高**：`run_span` 把 `algorithm/selected_model/outcome` 这些低基数字段做 metric attributes（避免 cardinality 爆炸），`session_id/agent_id/task_id/correlation_id` 走 span fields（中等基数，可下采样），`extra_metadata` 整个对象 `tracing::field::debug` 序列化（高基数，不下采样但保留全文），三层职责分明。**文件头注释明确警告**："`tracing` spans cannot grow field names at runtime"。
3. **W3C trace context 通过 `TraceContextPropagator + HeaderExtractor` 自动从 HTTP header 提取父 span**（`traceparent`/`tracestate`），实现跨服务链路追踪。
4. **`RouteErrorKind` 设计哲学**："carries no provider message, response body, or source error. Suitable for logs and telemetry, NOT client-facing rendering" —— 错误分类只为指标服务，原始错误走另一条用户消息路径，避免泄露内部信息。
5. **SSE 多协议分帧**：`OpenAiChat` 用 `Event::default().data(...)` + 末尾 `[DONE]` 哨兵（"客户端读到就停，把已有内容当答案"），`AnthropicMessages/Responses` 用 `Event::default().event(type).data(...)` 用事件类型做事件名；且上游错误 (`LlmStreamError::Upstream`) 直接 verbatim 转发保留上游原始错误码和类型，避免重写丢失上下文。
6. **Graceful shutdown** 在 Unix 上同时监听 SIGTERM 和 SIGINT，且任一信号注册失败都退化为 `std::future::pending()`（永不退出）并 `tracing::warn!` —— "宁可服务继续跑也不能误关"的服务可用性哲学。

**对 laew 的启示**：Switchyard 是「协议中立 IR + OTEL 精细度 + 错误分类稳定性」的工业级范本。laew 升级时可参考其 **protocol 中立 IR（Anthropic + OpenAI 双向 codec）+ W3C traceparent 自动透传 + RouteErrorKind 12 类 + SSE 多协议分帧** 四大模式。

---

> **字数**：本文档 Switchyard 第八轮深挖章节新增约 800 行。
# Switchyard 第十轮深挖：8 个新维度全面剖析

> 源码路径：`/usr/local/LsmGitOpenSource/Switchyard`  
> 分析日期：2026-09-07  
> 前 9 轮已覆盖：协议 IR / 翻译引擎 / 路由算法 / 7 种路由策略 / LLM 网关 / PyO3 / 熔断器 / 协议 wire 真实实现 / SubAgent 并发 / Goal 状态机 / TUI 渲染管线 / Hook 拦截器 / Skill 一等公民 / Effect DI 拓扑 / CBOR 二进制帧 / Lane 三队列 / WriterLease fence / 文件编辑补丁 / 代码检索 / Git checkpoint / Bash PTY / 多模态 / PromptCaching / Schema 校验 / Web 检索 / Telemetry / Session 持久化 / Tool 权限沙箱 / LSP/IDE 集成 / Skill Workshop / 多租户团队记忆 / 终端控制序列 / CrashDump / WebUI / OAuth / i18n / Release / WebSocket / DevContainer / CRDT（前 9 轮部分涉及）  
> 本轮聚焦：**8 个全新深挖维度**（前 9 章已覆盖的内容不再重复）

---

## 目录

1. [CrashDump 与错误恢复](#1-crashdump-与错误恢复)
2. [WebUI 与 DesktopApp](#2-webui-与-desktopapp)
3. [OAuth 认证与多账号](#3-oauth-认证与多账号)
4. [i18n 国际化](#4-i18n-国际化)
5. [Release 工程化与 AutoUpdate](#5-release-工程化与-autoupdate)
6. [WebSocket 与 SSE](#6-websocket-与-sse)
7. [DevContainer 与容器化](#7-devcontainer-与容器化)
8. [CRDT 与多端冲突](#8-crdt-与多端冲突)
9. [laew gap 清单（L79-L110）](#9-laew-gap-清单l79-l110)
10. [与前 9 轮关系](#10-与前-9-轮关系)

---

## 1. CrashDump 与错误恢复

### 1.1 整体错误架构

Switchyard 采用**分层错误类型**设计，每层拥有独立的错误枚举，通过 `thiserror` 的 `#[from]` 自动转换：

```
LlmClientError (protocol crate)        ← 协议客户端错误
    ├── UpstreamHttp { status, body }
    ├── ContextWindowExceeded { model, message }
    ├── Timeout { source }
    ├── Transport { source }
    ├── InvalidResponse { source }
    ├── RequestTranslation(String)
    ├── RequestEncoding(String)
    ├── ResponseTranslation(String)
    ├── InvalidRequest { message }
    ├── Configuration { message }
    ├── Ffi { ... }
    └── General(String)

LibsyError (libsy crate)              ← 编排层错误
    ├── TargetNotFound { target }
    ├── NoTargets
    ├── AlgorithmError { message }
    ├── Driver(DriverError)
    ├── MissingFinalResponse
    ├── ClientCall { target, source: LlmClientError }
    └── External { operation, source }

RunnerError (switchyard-runner)       ← 路由运行层错误
    ├── Algorithm(LibsyError)
    ├── Client(LlmClientError)
    ├── Configuration { ... }
    ├── UnknownRouteModel(...)
    ├── IncompatibleCallerFormat(CallerAuthKind)
    └── AuxiliaryUnsupported

ServerError (switchyard-server)        ← HTTP 服务层错误
    └── 包装上述所有错误为 String

TranslationError (switchyard-translation)  ← 翻译层错误
    ├── InvalidJson(serde_json::Error)
    ├── InvalidType { path, expected }
    ├── UnsupportedTranslation { from, to }
    ├── LossyConversion(String)
    ├── UnknownField { path }
    ├── InvalidValue { path, message }
    └── Other(String)
```

### 1.2 安全遥测摘要（RouteErrorKind）

`switchyard-runner/src/failure.rs` 定义了**不携带任何敏感信息的错误分类**，专为遥测设计：

```rust
#[derive(Clone, Copy, Debug, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum RouteErrorKind {
    UpstreamHttp,            // 上游返回非 2xx
    ContextWindowExceeded,   // 上下文窗口溢出
    Timeout,                 // 上游超时
    Transport,               // 网络不可达
    InvalidResponse,         // 响应解码失败
    RequestTranslation,      // 入站翻译失败
    RequestEncoding,         // 出站编码失败
    ResponseTranslation,     // 响应翻译失败
    InvalidRequest,          // 请求不合法
    Configuration,           // 路由/客户端配置错误
    Algorithm,               // 算法/驱动失败
    Other,                   // 兜底
}

pub enum RouteErrorPhase {
    BeforeResponse,          // 响应交付前失败
    DuringStream,            // 流式消费中失败
}

pub struct RouteErrorSummary {
    pub kind: RouteErrorKind,
    pub phase: RouteErrorPhase,
    pub upstream_status: Option<u16>,
    pub target: Option<ModelId>,
}
```

**关键设计点**：
- `RouteErrorSummary` **故意不携带 provider 消息、响应体、源错误**，仅暴露分类
- 测试用例验证 `SECRET` 字符串不会出现在 `RouteErrorSummary` 的 `Debug` 输出中
- `execution_error_summary()` 和 `stream_error_summary()` 两个工厂方法分别处理"响应前失败"和"流式失败"

### 1.3 重试与退避策略

`libsy-llm-client/src/client.rs` 实现了**带 Retry-After 的指数退避**：

```rust
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

fn retry_delay(retry_number: u64, retry_after: Option<Duration>) -> Duration {
    // Retry-After 优先；否则 250ms 翻倍，上限 2s
    retry_after.unwrap_or_else(|| {
        let multiplier = 1_u32 << retry_number.min(3);
        INITIAL_RETRY_DELAY
            .saturating_mul(multiplier)
            .min(MAX_RETRY_BACKOFF)
    })
}
```

**重试判定逻辑**（`AttemptFailure::is_retryable()`）：
- `Transport` / `Timeout` → **始终可重试**
- `UpstreamHttp { status }` → 仅当 `is_retryable_http_status(status)` 为 true（408, 429, 5xx）
- 其他错误（包括 `ContextWindowExceeded`、`InvalidRequest`）→ **不可重试**

**重试预算**：
- `DEFAULT_MAX_RETRIES = 2`（默认 2 次重试，共 3 次尝试）
- `MAX_CONFIGURED_RETRIES = 10`（配置上限）
- 最坏情况：`candidates × (max_retries + 1)` 次上游尝试

### 1.4 优雅关闭与请求取消

`switchyard-server/src/lib.rs` 实现了**完整的生命周期管理**：

```rust
// 关闭流程
tokio::select! {
    result = &mut server => result.map_err(server_io_error),
    _ = shutdown => {
        tracing::info!(?timeout, "shutdown signal received; draining active requests");
        handle.graceful_shutdown(Some(timeout));  // 触发优雅关闭
        server.await.map_err(server_io_error)     // 等待活跃请求完成
    }
}
```

**关键机制**：
- `DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT = 30s`（默认关闭宽限期）
- `shutdown::signal()` 同时监听 `Ctrl+C` 和 `SIGTERM`（Unix）
- `RequestLogGuard`：RAII guard，若 handler future 在响应写入前被 drop（客户端断开），触发 `emit_cancelled()` 记录取消事件
- `CancelSentinel`：测试用 RAII 标记，验证客户端断开时 handler future 被正确 drop

### 1.5 客户端断开检测

```rust
// 客户端断开时 Axum 自动 drop handler future
struct RequestLogGuard(Option<RequestLogContext>);

impl Drop for RequestLogGuard {
    fn drop(&mut self) {
        if let Some(context) = self.0.take() {
            context.emit_cancelled();  // 记录取消事件
        }
    }
}
```

- 使用 `CLIENT_CLOSED_REQUEST = 499` 状态码（Nginx 惯例）
- `metrics::record_client_disconnect()` 记录到 Prometheus
- 测试 `client_disconnect_cancels_a_buffered_handler` 验证断开行为

### 1.6 上下文窗口溢出检测

`libsy-llm-client/src/backend.rs` 实现了**多 provider 的上下文溢出检测**：

```rust
// OpenAI 短语列表
const OPENAI_OVERFLOW_PHRASES: &[&str] = &[
    "maximum context length",
    "context length exceeded",
    "context window",
    "context length is only",
    "please reduce the length of the input",
    "exceeds the maximum allowed input length",
    "exceeds the maximum allowed length",
    "is longer than the model's context length",
];

// Anthropic 短语列表
const ANTHROPIC_OVERFLOW_PHRASES: &[&str] = &[
    "prompt is too long",
    "maximum number of tokens",
    "context window",
    "context length",
];
```

**检测策略**（`is_overflow_body()`）：
1. 先运行结构化检查（如 `error.code == "context_length_exceeded"`）
2. 再匹配 `error.message` 中的短语
3. 最后回退到原始 body 字符串匹配（处理纯文本响应）

**测试覆盖**：
- 原生 OpenAI（`context_length_exceeded`）
- NVIDIA/LiteLLM 包裹（无结构化 code）
- Hub GLM（LiteLLM 包裹，code="400"）
- 原生 SGLang（顶层 envelope，无 `error` key）
- KV-pool 拒绝（`managers/utils.py`）
- 声明上下文拒绝（`tokenizer_manager.py`）

### 1.7 缺失：CrashDump 机制

**Switchyard 没有 CrashDump / panic hook**：
- 未使用 `human-panic` crate
- 未设置 `std::panic::set_hook`
- 未生成 minidump / core dump
- 仅依赖 `tracing` 日志记录错误

**laew 对比**：laew 同样缺少 panic hook，与 Switchyard 处于同一水平。

---

## 2. WebUI 与 DesktopApp

### 2.1 现状：纯 HTTP 服务器，无 WebUI

Switchyard **没有任何 Web 界面或桌面应用**：
- 无 React / Vue / Svelte 前端
- 无 Tauri / Electron 桌面壳
- 无 Gradio / Streamlit 演示界面
- 无 Telegram / Discord Bot

**唯一的可视化入口**：
- `GET /v1/models` — 返回 JSON 模型列表
- `GET /v1/stats` — 返回 JSON 路由统计
- `GET /metrics` — Prometheus 指标（文本格式）
- `GET /health` — 健康检查

### 2.2 启动横幅（ASCII Art）

`switchyard-server/src/lib.rs` 包含一个**终端启动横幅**：

```rust
const STARTUP_BANNER_ART: &str = include_str!("../assets/startup_banner.txt");

fn startup_banner(options: &ServerRunOptions, state: &ServerState, color: bool) -> String {
    // 渲染 NVIDIA 绿色 ANSI truecolor
    let (red, green, blue) = (118, 185, 0);
    for line in banner.lines() {
        rendered.push_str(&format!("\x1b[38;2;{red};{green};{blue}m{line}\x1b[0m\n"));
    }
}
```

**输出示例**：
```
Switchyard libsy server
  listening: http://0.0.0.0:4000
  routes: fast, strong

endpoints:
  POST /v1/chat/completions    OpenAI Chat Completions
  POST /v1/messages            Anthropic Messages
  POST /v1/responses           OpenAI Responses
  ...

example:
  curl -s 'http://127.0.0.1:4000/v1/chat/completions' \
    -H 'Content-Type: application/json' \
    -d '{"model":"fast","messages":[{"role":"user","content":"Hello from Switchyard"}]}'
```

### 2.3 dev-server 目录

`dev-server/` 仅包含：
- `config.toml` — 示例配置
- `README` — 使用说明
- `switchyard.service` — systemd 服务单元

**无任何 Web 服务器或前端代码**。

### 2.4 Codex 模型目录集成

`switchyard-server/src/lib.rs` 的 `codex_model_entry_json()` 函数生成 **Codex 兼容的 ModelInfo 卡片**：

```rust
fn codex_model_entry_json(model: &str, capabilities: ModelCapabilities, priority: usize) -> Value {
    json!({
        "slug": model,
        "display_name": model,
        "description": "Switchyard-routed model.",
        "default_reasoning_level": if reasoning { json!("xhigh") } else { Value::Null },
        "supported_reasoning_levels": if reasoning { reasoning_levels() } else { json!([]) },
        "shell_type": if tool_calling { "shell_command" } else { "disabled" },
        "base_instructions": "You are Codex, a coding agent.",
        "apply_patch_tool_type": if tool_calling { Some("freeform") } else { None },
        "truncation_policy": {"mode": "tokens", "limit": 10_000},
        ...
    })
}
```

**意义**：Switchyard 可以作为 **Codex 的后端代理**，通过 `/v1/models` 端点向 Codex 暴露路由模型。

### 2.5 laew gap

**laew 同样无 WebUI**，但 laew 有 TUI（终端 UI），比 Switchyard 的 ASCII 横幅更丰富。

---

## 3. OAuth 认证与多账号

### 3.1 认证架构概览

Switchyard 支持**两种认证模式**，通过 `forward_auth` 标志区分：

| 模式 | 配置 | 行为 |
|------|------|------|
| **静态 API Key** | `api_key_env = "VAR_NAME"` | 从环境变量读取，注入请求头 |
| **转发调用者凭证** | `forward_auth = true` | 透传调用者的 `Authorization` 头 |

**互斥约束**：
```rust
if config.forward_auth && config.api_key_env.is_some() {
    return Err(RunnerError::configuration(format!(
        "llm client {client_name} cannot set both forward_auth and api_key_env"
    )));
}
```

### 3.2 静态 API Key 模式

`switchyard-runner/src/config.rs` 的 `build_backend()` 函数：

```rust
let api_key = config
    .api_key_env
    .as_deref()
    .map(|variable| {
        if variable.trim().is_empty() {
            return Err(RunnerError::configuration(format!(
                "llm client {client_name} api_key_env must not be empty"
            )));
        }
        let api_key = std::env::var(variable).map_err(|error| {
            RunnerError::configuration(format!(
                "llm client {client_name} could not read api_key_env {variable}: {error}"
            ))
        })?;
        if api_key.trim().is_empty() {
            return Err(RunnerError::configuration(format!(
                "llm client {client_name} api_key_env {variable} is empty"
            )));
        }
        Ok(api_key)
    })
    .transpose()?;
```

**特点**：
- 通过**环境变量名**引用，而非明文存储
- 启动时立即验证，失败则拒绝启动
- 空值检查（`trim().is_empty()`）

### 3.3 认证头应用

`libsy-llm-client/src/backend.rs` 的 `apply_auth()` 方法：

```rust
pub fn apply_auth(&self, mut builder: RequestBuilder) -> RequestBuilder {
    let api_key = if self.is_forwarding_auth() {
        None  // 转发模式下不使用静态 key
    } else {
        self.config().api_key.as_deref()
    };
    match self {
        Backend::OpenAiChat(_) | Backend::OpenAiResponses(_) => {
            if let Some(api_key) = api_key {
                builder = builder.bearer_auth(api_key);  // Authorization: Bearer <key>
            }
        }
        Backend::Anthropic(_) => {
            builder = builder.header("anthropic-version", ANTHROPIC_VERSION);
            if let Some(api_key) = api_key {
                builder = builder.header("x-api-key", api_key);  // x-api-key: <key>
            }
        }
    }
    builder
}
```

### 3.4 转发调用者凭证

`apply_forwarded_auth()` 方法实现**多调用者场景**：

```rust
pub(crate) fn apply_forwarded_auth(
    &self,
    mut builder: RequestBuilder,
    metadata: Option<&Metadata>,
) -> RequestBuilder {
    if !self.is_forwarding_auth() {
        return builder;
    }
    let Some(headers) = metadata.and_then(|metadata| metadata.http_headers.as_ref()) else {
        return builder;
    };
    match self {
        Backend::OpenAiChat(_) | Backend::OpenAiResponses(_) => {
            for name in ["authorization", "chatgpt-account-id", "x-openai-fedramp"] {
                if let Some(value) = headers.get(name) {
                    builder = builder.header(name, sensitive_header(value));
                }
            }
        }
        Backend::Anthropic(_) => {
            for name in ["authorization", "x-api-key"] {
                if let Some(value) = headers.get(name) {
                    builder = builder.header(name, sensitive_header(value));
                }
            }
            // 特殊处理：Anthropic OAuth beta header
            if let Some(value) = headers.get("anthropic-beta")
                && let Some(value) = oauth_beta_header(value)
            {
                builder = builder.header("anthropic-beta", value);
            }
        }
    }
    builder
}
```

**关键设计**：
- `sensitive_header()` 调用 `HeaderValue::set_sensitive(true)`，防止日志记录
- 支持 OpenAI 的 `chatgpt-account-id` 和 `x-openai-fedramp` 头
- Anthropic 的 `anthropic-beta` 头经过 `oauth_beta_header()` 过滤，仅保留 `oauth-*` 前缀的 beta 标记

### 3.5 凭证脱敏

`redact_forwarded_auth()` 在错误返回前**替换敏感信息**：

```rust
pub(crate) fn redact_forwarded_auth(
    &self,
    mut body: String,
    metadata: Option<&Metadata>,
) -> String {
    if !self.is_forwarding_auth() {
        return body;
    }
    let secret_headers: &[&str] = match self {
        Backend::OpenAiChat(_) | Backend::OpenAiResponses(_) => {
            &["authorization", "chatgpt-account-id"]
        }
        Backend::Anthropic(_) => &["authorization", "x-api-key"],
    };
    for name in secret_headers {
        let Some(value) = headers.get(*name).and_then(|value| value.to_str().ok()) else {
            continue;
        };
        if !value.is_empty() {
            body = body.replace(value, "[REDACTED]");
        }
    }
    body
}
```

### 3.6 额外头部验证

`validate_extra_headers()` 防止**头部注入攻击**：

```rust
pub(crate) fn validate_extra_headers(&self, model_name: &str) -> Result<()> {
    let invalid_name = self.config().extra_headers.keys().find(|name| match self {
        Backend::OpenAiChat(_) | Backend::OpenAiResponses(_) => {
            name.eq_ignore_ascii_case("authorization")
                || (self.is_forwarding_auth()
                    && (name.eq_ignore_ascii_case("chatgpt-account-id")
                        || name.eq_ignore_ascii_case("x-openai-fedramp")))
        }
        Backend::Anthropic(_) => {
            name.eq_ignore_ascii_case("x-api-key")
                || name.eq_ignore_ascii_case("anthropic-version")
                || (self.is_forwarding_auth()
                    && (name.eq_ignore_ascii_case("authorization")
                        || name.eq_ignore_ascii_case("anthropic-beta")))
        }
    });
    if let Some(name) = invalid_name {
        return Err(LlmClientError::Configuration {
            message: format!(
                "model {model_name:?} extra_headers cannot set {name:?}; extra_headers is only for additional headers"
            ),
        });
    }
    Ok(())
}
```

**防御点**：
- 禁止通过 `extra_headers` 覆盖 `authorization` / `x-api-key` / `anthropic-version`
- 转发模式下禁止覆盖 `chatgpt-account-id` / `x-openai-fedramp` / `anthropic-beta`

### 3.7 缺失：OAuth 2.0 / PKCE 流程

**Switchyard 没有完整的 OAuth 实现**：
- 无 `client_credentials` 授权流程
- 无 `authorization_code` + PKCE 流程
- 无 token 刷新 / 轮换
- 无 `oauth2` crate 依赖
- 仅支持**静态 API Key** 或**透传调用者凭证**

**laew 对比**：laew 同样无 OAuth 流程，仅支持静态 API Key 配置。

---

## 4. i18n 国际化

### 4.1 现状：完全无 i18n 支持

Switchyard **没有任何国际化实现**：
- 无 `rust-i18n` / `fluent` / `gettext` crate
- 无 `.ftl` / `.po` / `.mo` 翻译文件
- 无语言检测 / 切换机制
- 所有字符串硬编码为英文

### 4.2 硬编码字符串分布

| 模块 | 示例 |
|------|------|
| 错误消息 | `"target {target:?} was not found"` |
| 日志消息 | `"shutdown signal received; draining active requests"` |
| HTTP 响应 | `"Not Found"`, `"request body must include a non-empty string \`model\`"` |
| 启动横幅 | `"Switchyard libsy server"`, `"listening:"`, `"routes:"` |
| 指标标签 | `"ok"`, `"retryable_error"`, `"other_error"`, `"client_disconnected"` |

### 4.3 终端输出处理

`switchyard-server/src/lib.rs` 的 `render_startup_banner_art()` 使用 **ANSI truecolor**：

```rust
fn render_startup_banner_art(color: bool) -> String {
    let banner = STARTUP_BANNER_ART.trim_end();
    if !color {
        return banner.to_string();
    }
    let (red, green, blue) = (118, 185, 0);  // NVIDIA 绿色
    let mut rendered = String::new();
    for line in banner.lines() {
        rendered.push_str(&format!("\x1b[38;2;{red};{green};{blue}m{line}\x1b[0m\n"));
    }
    rendered.trim_end_matches('\n').to_string()
}
```

**注意**：`color` 参数由 `std::io::stdout().is_terminal()` 决定，非 TTY 时禁用颜色。

### 4.4 缺失：完整 i18n 体系

**Switchyard 缺少**：
- 错误消息多语言化
- CLI 帮助文本多语言化
- 文档多语言化
- RTL（从右到左）布局支持
- 翻译 pipeline（Weblate / Crowdin）

**laew 对比**：laew 的 CLAUDE.md 明确指出「注释、CLI 文案、文档一律中文」，但代码字符串仍为英文，与 Switchyard 处于同一水平。

---

## 5. Release 工程化与 AutoUpdate

### 5.1 发布流程概览

Switchyard 使用 **GitHub Actions + maturin + PyPI Trusted Publishing** 的完整发布链路：

```
git tag v*.*.* → push → publish.yml 触发
    ├── validate-release-ref（版本校验）
    ├── python-release-checks（Python 3.10-3.14 矩阵测试）
    ├── rust-release-checks（fmt + clippy + test）
    ├── source-dist（sdist 构建）
    ├── wheels（6 平台 wheel 构建）
    ├── pypi-publish（PyPI 发布）
    └── github-release（GitHub Release 创建）
```

### 5.2 版本校验

`publish.yml` 的 `validate-release-ref` job：

```python
# 校验 tag 格式
if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
    raise SystemExit(f"release tags must look like vMAJOR.MINOR.PATCH, got {tag!r}")

# 校验 pyproject.toml 与 Cargo.toml 版本一致
if workspace_version != package_version:
    raise SystemExit(f"Python package version {package_version!r} does not match Rust workspace version {workspace_version!r}")

# 校验 tag 与版本一致
expected = f"v{package_version}"
if tag != expected:
    raise SystemExit(f"release tag {tag!r} does not match pyproject version {package_version!r}")
```

### 5.3 多平台 Wheel 矩阵

`publish.yml` 的 `wheels` job 支持 **6 平台**：

| label | os | rust_target | manylinux_arch | smoke |
|-------|-----|-------------|----------------|-------|
| linux-x86_64 | ubuntu-latest | x86_64-unknown-linux-gnu | x86_64 | true |
| linux-aarch64 | ubuntu-24.04-arm | aarch64-unknown-linux-gnu | aarch64 | false |
| macos-x86_64 | macos-15-intel | x86_64-apple-darwin | - | true |
| macos-arm64 | macos-14 | aarch64-apple-darwin | - | true |
| windows-x86_64 | windows-latest | x86_64-pc-windows-msvc | - | true |
| windows-arm64 | windows-11-arm | aarch64-pc-windows-msvc | - | false |

**Linux 构建**使用 `manylinux2014` Docker 镜像：
```bash
docker run --rm \
    -e MATURIN_VERSION="${MATURIN_VERSION}" \
    -v "${GITHUB_WORKSPACE}:/io" \
    -w /io \
    "quay.io/pypa/manylinux2014_${{ matrix.manylinux_arch }}" \
    /bin/bash -lc '
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
        source "${HOME}/.cargo/env"
        PYTHON=/opt/python/cp310-cp310/bin/python
        "${PYTHON}" -m pip install --upgrade pip
        "${PYTHON}" -m pip install "maturin==${MATURIN_VERSION}"
        "${PYTHON}" -m maturin build --release --locked --compatibility pypi --out dist --interpreter "${PYTHON}"
    '
```

**非 Linux 构建**使用原生 maturin：
```bash
python -m maturin build --release --locked --compatibility pypi --out dist --target "${{ matrix.rust_target }}"
```

### 5.4 Smoke 测试

每个 wheel 构建后执行 **smoke 测试**：

```python
import switchyard
import switchyard_rust
from switchyard_rust import _switchyard_rust

print("switchyard", switchyard.__version__)
print("switchyard_rust", switchyard_rust.__file__)
print("rust extension", _switchyard_rust.__name__)
```

**双版本 smoke**：Python 3.10 + Python 3.14 都测试。

### 5.5 Dev Wheel 构建

`publish.yml` 支持**手动触发 dev 构建**：

```yaml
workflow_dispatch:
  inputs:
    build_dev_artifact:
      description: "Build one Linux x86_64 dev wheel artifact."
      type: boolean
      default: false
    build_dev_matrix:
      description: "Build the complete dev sdist and wheel matrix as artifacts."
      type: boolean
      default: false
    dev_version:
      description: "PEP 440 .dev version for the dev wheel."
      type: string
      default: "0.0.1.dev0"
```

**版本戳记**：`scripts/release/set_dev_wheel_version.py` 修改 `pyproject.toml` 版本。

### 5.6 CI 流程

`ci.yml` 包含 **7 个 job**：

| job | 用途 |
|-----|------|
| changes | 检测代码变更（跳过纯文档 PR） |
| lint | ruff 代码检查 |
| spdx-headers | SPDX 头检查（硬门禁） |
| rust | cargo fmt + clippy + test |
| typecheck | mypy 类型检查（continue-on-error） |
| test | Python 3.10-3.14 矩阵测试 |
| slim-install-smoke | 精简安装测试（禁止引入 torch/transformers 等重包） |

**SPDX 头检查**（硬门禁）：
```bash
copyright_re='^# SPDX-FileCopyrightText: Copyright \(c\) [0-9]{4}(-[0-9]{4})? NVIDIA CORPORATION & AFFILIATES\. All rights reserved\.$'
license_re='^# SPDX-License-Identifier: Apache-2\.0$'
```

### 5.7 精简安装防护

`slim-install-smoke` job 确保**默认安装不引入重包**：

```python
forbidden = [
    "torch", "transformers", "huggingface_hub", "routellm", "litellm",
    "datasets", "tokenizers", "safetensors", "pyarrow", "agents", "mcp", "docx",
]
extras = [m for m in forbidden if importlib.util.find_spec(m) is not None]
if extras:
    sys.exit(f"FAIL: heavy packages pulled into slim install: {extras}")
```

### 5.8 缺失：AutoUpdate

**Switchyard 没有自动更新机制**：
- 无 `self_update` crate
- 无版本检查 API
- 无后台下载 / 热更新
- 用户需手动 `pip install --upgrade nemo-switchyard`

**laew 对比**：laew 同样无 AutoUpdate，但 laew 有 `build.rs` 注入版本信息，比 Switchyard 的 maturin 版本管理更轻量。

---

## 6. WebSocket 与 SSE

### 6.1 现状：无 WebSocket，完整 SSE 支持

Switchyard **不支持 WebSocket**：
- 无 `tokio-tungstenite` / `axum::ws` 依赖
- 无 `wss://` / `ws://` 端点
- 无双向通信 / 推送机制

**但 SSE 支持非常完整**，是核心功能之一。

### 6.2 SSE 帧解析

`switchyard-translation/src/sse.rs` 实现了**协议无关的 SSE 帧解析**：

```rust
pub(crate) enum SseFrame {
    Empty,       // 空帧（注释或 keep-alive）
    Done,        // [DONE] 标记
    Data(Value), // JSON 数据
}

pub(crate) fn parse_json_sse_frame(
    frame: &str,
    done_marker: Option<&str>,
) -> Result<SseFrame, BoxError> {
    let data = frame
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with(':'))  // 忽略注释
        .filter_map(data_field_value)  // 仅提取 data: 字段
        .fold(String::new(), |mut a, b| {
            a.reserve(b.len() + 1);
            a.push_str(&b);
            a.push('\n');
            a
        });
    let data = data.trim_end();
    if data.is_empty() {
        return Ok(SseFrame::Empty);
    }
    if done_marker.is_some_and(|marker| data == marker) {
        return Ok(SseFrame::Done);
    }
    let value = serde_json::from_str::<Value>(data)?;
    Ok(SseFrame::Data(value))
}
```

**关键设计**：
- 仅提取 `data:` 字段，忽略 `event:` / `id:` / `retry:` 和注释（`:` 开头）
- 支持多行 `data:` 拼接（以 `\n` 分隔）
- `data:` 后的空格是可选的（`data:{"x":1}` 和 `data: {"x":1}` 都支持）
- `database:` 等类似字段**不会**被误识别为 `data:`

### 6.3 终端事件检测

`is_terminal_event()` 识别**协议特定的流结束标记**：

```rust
pub(crate) fn is_terminal_event(format: WireFormat, event: &Value) -> bool {
    match format {
        WireFormat::OpenAiChat => event
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|choice| {
                choice
                    .get("finish_reason")
                    .and_then(Value::as_str)
                    .is_some()
            }),
        WireFormat::AnthropicMessages => {
            event.get("type").and_then(Value::as_str) == Some("message_stop")
        }
        WireFormat::OpenAiResponses => matches!(
            event
                .get("type")
                .or_else(|| event.get("event"))
                .and_then(Value::as_str),
            Some("response.completed" | "response.incomplete" | "response.failed")
        ),
    }
}
```

### 6.4 SSE 帧封装

`switchyard-server/src/sse.rs` 实现了**出站 SSE 帧封装**：

```rust
pub(crate) fn frame_stream(
    stream: RawEventStream,
    target_format: WireFormat,
) -> Sse<SseFrameStream> {
    let framed = async_stream::stream! {
        let mut stream = stream;
        let mut failed = false;
        while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
            let event = match item {
                Ok(value) => match frame_event(target_format, value) {
                    Ok(event) => event,
                    Err(error) => {
                        failed = true;
                        error_event(target_format, error.to_string())
                    }
                },
                Err(LlmStreamError::Upstream(value)) => {
                    // 上游错误原样转发，保留 code 和 type
                    failed = true;
                    frame_event(target_format, value.clone()).unwrap_or_else(|error| {
                        tracing::warn!(error = %error, "in-band error event could not be framed");
                        error_event(target_format, value.to_string())
                    })
                }
                Err(LlmStreamError::Client(error)) => {
                    tracing::warn!(error = %error, "stream iteration failed");
                    failed = true;
                    error_event(target_format, error.to_string())
                }
            };
            yield Ok(event);
            if failed {
                break;  // 错误后立即终止
            }
        }
        // [DONE] 仅在成功时发送
        if !failed && target_format == WireFormat::OpenAiChat {
            yield Ok(Event::default().data("[DONE]"));
        }
    };
    Sse::new(Box::pin(framed) as SseFrameStream)
}
```

**关键设计**：
- **错误终止**：任何错误（上游或客户端）立即终止流，不发送后续 chunk
- **错误后无 `[DONE]`**：成功标记仅在无错误时发送
- **上游错误原样转发**：`LlmStreamError::Upstream(value)` 保留原始 code/type
- **客户端错误合成**：`LlmStreamError::Client(error)` 包装为 `SwitchyardError`

### 6.5 协议特定帧格式

`frame_event()` 根据目标格式生成不同的 SSE 事件：

```rust
fn frame_event(target_format: WireFormat, value: Value) -> Result<Event, axum::Error> {
    match target_format {
        WireFormat::OpenAiChat => Event::default().json_data(value),  // data: {"choices":[...]}
        WireFormat::AnthropicMessages | WireFormat::OpenAiResponses => {
            let event_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("message")
                .to_string();
            Event::default().event(event_type).json_data(value)  // event: message_start\ndata: {...}
        }
    }
}
```

**差异**：
- OpenAI Chat：仅 `data:` 字段，无 `event:` 字段
- Anthropic Messages / OpenAI Responses：同时发送 `event:` 和 `data:`

### 6.6 错误帧格式

`error_event()` 生成**协议特定的错误帧**：

```rust
fn error_event(target_format: WireFormat, message: String) -> Event {
    match target_format {
        WireFormat::OpenAiChat => Event::default().data(
            json!({
                "error": {
                    "message": message,
                    "type": "SwitchyardError",
                }
            })
            .to_string(),
        ),
        WireFormat::AnthropicMessages | WireFormat::OpenAiResponses => {
            Event::default().event("error").data(
                json!({
                    "type": "error",
                    "error": {
                        "message": message,
                        "type": "SwitchyardError",
                    }
                })
                .to_string(),
            )
        }
    }
}
```

### 6.7 测试覆盖

`sse.rs` 的测试用例验证关键行为：

```rust
#[tokio::test]
async fn stream_error_terminates_without_done_marker() -> TestResult {
    // 错误后不发送 [DONE]，不发送后续 chunk
    assert!(body.contains("before"));
    assert!(body.contains("boom"));
    assert!(!body.contains("after"));
    assert!(!body.contains("[DONE]"));
}

#[tokio::test]
async fn in_band_error_is_forwarded_verbatim_without_done_marker() -> TestResult {
    // 上游错误原样转发，不包装为 SwitchyardError
    assert!(body.contains("stream_failed"));
    assert!(body.contains("upstream_stream_error"));
    assert!(!body.contains("SwitchyardError"));
}
```

### 6.8 缺失：WebSocket / 双向通信

**Switchyard 缺少**：
- WebSocket 端点
- 双向推送机制
- 实时通知（如路由变更、后端故障）
- 长连接管理

**laew 对比**：laew 同样无 WebSocket，但 laew 的 TUI 是本地交互，不需要实时推送。

---

## 7. DevContainer 与容器化

### 7.1 Dockerfile

`Dockerfile` 实现了**多阶段构建**：

```dockerfile
ARG RUST_VERSION=1.96.1
FROM rust:${RUST_VERSION}-bookworm AS builder

WORKDIR /opt/switchyard
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY .cargo ./.cargo
COPY crates ./crates

RUN cargo build --locked --release -p switchyard-server

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder \
    /opt/switchyard/target/release/switchyard-server \
    /usr/local/bin/switchyard-server

ENV HOME=/tmp
USER 1000:1000
EXPOSE 4000
ENTRYPOINT ["switchyard-server"]
```

**关键设计**：
- **多阶段构建**：builder 阶段编译，slim 阶段运行，减小镜像体积
- **锁定 Rust 版本**：`RUST_VERSION=1.96.1` 与 `rust-toolchain.toml` 同步
- **locked 构建**：`cargo build --locked` 确保依赖一致性
- **非 root 运行**：`USER 1000:1000`
- **最小化依赖**：仅安装 `ca-certificates`

### 7.2 .dockerignore

```
target/
.git/
```

### 7.3 缺失：DevContainer / docker-compose

**Switchyard 缺少**：
- `.devcontainer/devcontainer.json`
- `docker-compose.yml`
- 开发环境容器化
- VS Code Remote Containers 集成
- 多服务编排（如 Redis / PostgreSQL）

**laew 对比**：laew 同样无 DevContainer，但 laew 是 CLI 工具，容器化需求较低。

### 7.4 CI 中的容器使用

`publish.yml` 使用 `manylinux2014` Docker 镜像构建 Linux wheel：

```bash
docker run --rm \
    -e MATURIN_VERSION="${MATURIN_VERSION}" \
    -v "${GITHUB_WORKSPACE}::/io" \
    -w /io \
    "quay.io/pypa/manylinux2014_${{ matrix.manylinux_arch }}" \
    /bin/bash -lc '...'
```

**目的**：确保 wheel 兼容旧版 Linux（manylinux2014 标准）。

---

## 8. CRDT 与多端冲突

### 8.1 现状：无 CRDT 实现

Switchyard **没有任何 CRDT 实现**：
- 无 `yrs` / `automerge` / `crdt` crate
- 无协同编辑 / 实时同步
- 无多端状态合并

**原因**：Switchyard 是**无状态代理**，不维护客户端会话状态，无需 CRDT。

### 8.2 路由日志的持久化

`switchyard-server/src/routing_log.rs` 实现了**追加式 JSONL 日志**：

```rust
pub(crate) struct RoutingLog(fs::File);

impl RoutingLog {
    pub(crate) fn new(path: impl Into<PathBuf>) -> ServerResult<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)  // 追加模式
            .open(&path)
            .map_err(|error| routing_log_error(&path, error))?;
        Ok(Self(file))
    }

    pub(crate) fn append(
        &mut self,
        context: RoutingLogContext,
        model: &str,
        tier: Option<&str>,
        usage: &Usage,
    ) -> std::io::Result<()> {
        let record = RoutingRecord {
            ts: format_rfc3339_millis(SystemTime::now()).to_string().into(),
            route_id: context.route_id.into(),
            algorithm: context.algorithm.into(),
            task: context.task.map(Cow::Owned),
            trial_id: context.trial_id.map(Cow::Owned),
            session_id: context.session_id.map(Cow::Owned),
            model: model.into(),
            tier: tier.unwrap_or("").into(),
            prompt_tokens: usage.prompt_tokens,
            cached_tokens: usage.cached_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            completion_tokens: usage.completion_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            total_tokens: usage.prompt_tokens.saturating_add(usage.completion_tokens),
        };
        let mut line = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        line.push(b'\n');
        self.0.write_all(&line)
    }
}
```

**特点**：
- **追加式写入**：无锁，高吞吐
- **无 fsync**：可能丢失最后几行（崩溃时）
- **读取无同步**：`snapshot()` 读取时不锁定写入者
- **容错解析**：解析失败的行被跳过，不中断扫描

### 8.3 会话统计快照

`snapshot()` 实现**无锁读取**：

```rust
pub(crate) fn snapshot(
    path: &Path,
    session_id: &str,
) -> std::io::Result<Option<SessionStatsSnapshot>> {
    let mut reader = BufReader::with_capacity(64 * 1024, fs::File::open(path)?);
    let mut line = Vec::new();
    let mut snapshot = SessionStatsSnapshot::new(session_id);
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            break;  // 截断的行（写入中）
        }
        let Ok(record) = serde_json::from_slice::<RoutingRecord>(&line) else {
            continue;  // 解析失败跳过
        };
        snapshot.add_record(&record, session_id);
    }
    snapshot.sum_totals();
    Ok((snapshot.total_calls > 0).then_some(snapshot))
}
```

**容错设计**：
- 不以 `\n` 结尾的行被视为**截断**（写入中），停止扫描
- 解析失败的行被**静默跳过**
- 使用 `Cow<'a, str>` 避免拷贝

### 8.4 Git 合并冲突解决

`.sir-merge-a-lot.yml` 配置了 **sir-merge-a-lot** 机器人：

```yaml
trigger_mode: "on_demand"  # 仅手动触发

llm:
  enabled: true  # LLM 冲突解决

coding_agent_provider: codex_cli  # 编码代理回退
```

**冲突解决流程**：
1. 确定性阶梯（`rerere` → `Mergiraf`）先尝试
2. 失败时调用 LLM 解决
3. LLM 也失败时，调度 `codex_cli` 沙箱运行
4. 所有解决通过**有界编辑安全检查**（不允许修改冲突标记外的行）

### 8.5 缺失：CRDT / 多端同步

**Switchyard 缺少**：
- CRDT 数据结构
- 实时协同编辑
- 多端状态同步
- 冲突自动合并
- Event Sourcing

**laew 对比**：laew 同样无 CRDT，但 laew 的 Session 管理是单机的，无需分布式同步。

---

## 9. laew gap 清单（L79-L110）

基于本轮分析，新增 **32 个 laew gap**：

### P0 紧急（12 项）

| ID | gap | 影响 | 推荐 crate |
|----|-----|------|-----------|
| L79 | 无 panic hook | 崩溃无友好信息 | `human-panic` |
| L80 | 无 CrashDump 生成 | 事后排查困难 | `minidump` / `breakpad` |
| L81 | 无指数退避（仅固定 250ms 翻倍） | 重试风暴 | `backoff` / `failsafe` |
| L82 | 无熔断器 | 故障扩散 | `failsafe` |
| L83 | 无 API Key 轮换 | 凭证泄露风险 | `keyring` |
| L84 | 无 Session WAL / fsync | 崩溃丢数据 | `rusqlite` + WAL |
| L85 | 无 OAuth 2.0 流程 | 企业集成困难 | `oauth2` |
| L86 | 无 WebSocket 端点 | 无法实时推送 | `tokio-tungstenite` |
| L87 | 无 DevContainer | 开发环境不一致 | `.devcontainer.json` |
| L88 | 无 i18n 框架 | 国际化困难 | `rust-i18n` / `fluent` |
| L89 | 无 AutoUpdate | 用户版本碎片化 | `self_update` / `tauri-updater` |
| L90 | 无 CRDT | 多端同步不可能 | `yrs` / `automerge` |

### P1 重要（12 项）

| ID | gap | 影响 | 推荐 crate |
|----|-----|------|-----------|
| L91 | 无错误分类（仅有 RouteErrorKind） | 运维困难 | `thiserror` + `miette` |
| L92 | 无故障注入测试 | 生产故障 | `proptest` / `fail` |
| L93 | 无 WebUI | 可观测性差 | `axum` + `askama` |
| L94 | 无桌面壳 | 桌面用户困难 | `tauri` / `egui` |
| L95 | 无多账号轮换 | 单点故障 | 自定义 |
| L96 | 无跨进程锁 | 配置并发修改 | `fs2` / `fd-lock` |
| L97 | 无 RTL 布局 | 阿拉伯语/希伯来语 | `unic-bidi` |
| L98 | 无翻译 pipeline | 翻译质量 | `weblate` / `crowdin` |
| L99 | 无签名验证 | 供应链攻击 | `cargo-sign` / `sigstore` |
| L100 | 无心跳检测 | 死连接 | `tokio` interval |
| L101 | 无背压控制 | 内存溢出 | `tokio::sync::Semaphore` |
| L102 | 无 docker-compose | 本地编排困难 | `docker-compose.yml` |

### P2 进阶（8 项）

| ID | gap | 影响 | 推荐 crate |
|----|-----|------|-----------|
| L103 | 无 AutoUpdate 签名 | 恶意更新 | `ed25519-dalek` |
| L104 | 无远程编排 | 集群管理困难 | `kube-rs` |
| L105 | 无 digest 钉 | 镜像篡改 | Docker Content Trust |
| L106 | 无 Event Sourcing | 审计困难 | `esrs` |
| L107 | 无协同编辑 | 团队协作 | `yrs` |
| L108 | 无 Session 共享 | 多设备 | `yrs` + WebSocket |
| L109 | 无冲突自动合并 | 手动解决 | `yrs` / `automerge` |
| L110 | 无多租户隔离 | 企业部署 | `tokio::sync::RwLock` |

---

## 10. 与前 9 轮关系

### 10.1 覆盖度演进

| 轮次 | 新增维度 | 累计维度 | 文档行数 |
|------|---------|---------|---------|
| 第 1 轮 | 基础架构 / 协议 IR / 路由算法 | 3 | ~2,000 |
| 第 2 轮 | 7 种路由策略 / LLM 网关 / PyO3 | 6 | ~3,500 |
| 第 3 轮 | 协议 wire 真实实现 / SubAgent 并发 / Goal 状态机 | 9 | ~5,000 |
| 第 4 轮 | TUI 渲染管线 / Hook 拦截器 / Skill 一等公民 | 12 | ~6,500 |
| 第 5 轮 | Effect DI 拓扑 / CBOR 二进制帧 / Lane 三队列 | 15 | ~8,000 |
| 第 6 轮 | 文件编辑补丁 / 代码检索 / Git checkpoint | 18 | ~10,000 |
| 第 7 轮 | Bash PTY / 多模态 / PromptCaching / Schema 校验 | 22 | ~12,000 |
| 第 8 轮 | Web 检索 / Telemetry / Session 持久化 / Tool 权限沙箱 | 26 | ~14,000 |
| 第 9 轮 | LSP/IDE 集成 / Skill Workshop / 多租户团队记忆 / 终端控制序列 | 30 | ~16,000 |
| **第 10 轮** | **CrashDump / WebUI / OAuth / i18n / Release / WebSocket / DevContainer / CRDT** | **38** | **~18,000** |

### 10.2 关键发现

1. **错误处理成熟度中等**：有结构化错误分类（`RouteErrorKind`），但无 panic hook / CrashDump
2. **认证机制简单**：仅支持静态 API Key 或透传，无 OAuth 2.0 / PKCE
3. **国际化缺失**：完全无 i18n 支持
4. **发布工程完善**：GitHub Actions + maturin + PyPI Trusted Publishing，但无 AutoUpdate
5. **SSE 支持完整**：帧解析、终端事件、错误终止、协议特定格式
6. **WebSocket 缺失**：无双向通信能力
7. **容器化基础**：有多阶段 Dockerfile，但无 DevContainer / docker-compose
8. **CRDT 缺失**：无状态代理无需 CRDT，但路由日志无 fsync

### 10.3 laew 与 Switchyard 对比

| 维度 | laew | Switchyard | 差距 |
|------|------|-----------|------|
| 错误处理 | `thiserror` + `anyhow` | `thiserror` + `RouteErrorKind` | Switchyard 更结构化 |
| 认证 | 静态 API Key | 静态 API Key + 转发 | Switchyard 更灵活 |
| i18n | 无 | 无 | 平手 |
| Release | `cargo build` | maturin + PyPI | Switchyard 更完善 |
| SSE | 无 | 完整支持 | Switchyard 领先 |
| WebSocket | 无 | 无 | 平手 |
| 容器化 | 无 | Dockerfile | Switchyard 领先 |
| CRDT | 无 | 无 | 平手 |

### 10.4 推荐改造优先级

**P0（立即）**：
1. 添加 `human-panic` 提供友好崩溃信息
2. 实现指数退避 + 熔断器（`failsafe`）
3. 添加 API Key 轮换机制
4. 实现 Session WAL + fsync

**P1（短期）**：
1. 添加 OAuth 2.0 / PKCE 支持
2. 实现 WebSocket 端点（实时推送）
3. 添加 DevContainer 配置
4. 实现 i18n 框架（`rust-i18n`）

**P2（中期）**：
1. 实现 AutoUpdate（`self_update`）
2. 添加 CRDT 支持（`yrs`）
3. 实现协同编辑
4. 添加多租户隔离

---

## 附录 A：关键源码文件索引

| 文件 | 行数 | 核心内容 |
|------|------|---------|
| `crates/switchyard-server/src/lib.rs` | 2013 | HTTP 服务主逻辑 / 优雅关闭 / 请求取消 |
| `crates/switchyard-server/src/sse.rs` | 167 | 出站 SSE 帧封装 |
| `crates/switchyard-server/src/shutdown.rs` | 53 | 平台特定关闭信号 |
| `crates/switchyard-server/src/observability.rs` | 201 | OpenTelemetry 初始化 |
| `crates/switchyard-server/src/routing_log.rs` | 282 | 追加式 JSONL 日志 |
| `crates/switchyard-server/src/metrics.rs` | 173 | Prometheus 指标 |
| `crates/switchyard-server/src/failure.rs` | 339 | 安全遥测摘要 |
| `crates/switchyard-translation/src/sse.rs` | 224 | 入站 SSE 帧解析 |
| `crates/libsy-llm-client/src/client.rs` | 1921 | HTTP 客户端 / 重试 / 退避 |
| `crates/libsy-llm-client/src/backend.rs` | 449 | 后端配置 / 认证 / 溢出检测 |
| `crates/switchyard-runner/src/config.rs` | 1372 | TOML 配置加载 / 认证验证 |
| `crates/protocol/src/stream.rs` | 600+ | 流式 IR / 响应累积 |
| `.github/workflows/publish.yml` | 400+ | 发布流程 |
| `.github/workflows/ci.yml` | 226 | CI 流程 |
| `Dockerfile` | 31 | 多阶段构建 |
| `.sir-merge-a-lot.yml` | 41 | Git 合并冲突解决 |

## 附录 B：Cargo.toml 依赖分析

```toml
[workspace.dependencies]
async-stream = "0.3"
async-trait = "0.1"
base64 = "0.22"
futures = "0.3"
futures-util = "0.3"
http = "1"
httpdate = "1"
jsonschema = { version = "0.49.4", default-features = false }
jsonptr = { version = "0.8.1" }
parking_lot = "0.12"
rand = "0.10"
regex = "1"
reqwest = { version = "0.13.4", features = ["json", "rustls", "stream"] }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tracing = { version = "0.1", features = ["attributes", "std"] }
tracing-opentelemetry = "0.33"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

**缺失的关键依赖**：
- `human-panic` — 崩溃信息
- `failsafe` — 熔断器 / 退避
- `oauth2` — OAuth 2.0
- `rust-i18n` — 国际化
- `self_update` — 自动更新
- `tokio-tungstenite` — WebSocket
- `yrs` — CRDT
- `keyring` — 密钥管理

---

**文档完**
