# Switchyard 核心机制深度分析

> **项目**: Switchyard — NVIDIA 开发的 Rust LLM 流量代理与库
> **仓库**: https://github.com/NVIDIA-NeMo/Switchyard
> **版本**: v0.2.0 (pre-alpha)
> **许可证**: Apache-2.0
> **分析日期**: 2026-09-05
> **前置**: `Switchyard-源码调研.md` + `Switchyard-深度分析.md` 的进一步源码级钻取

本文在源码调研和深度分析基础上，对 Switchyard 七个核心机制进行"代码路径级别"的二次深挖：每个机制都列出真实文件路径、行号、关键结构体/特质名、核心代码片段，并给出对 laew 工程的具体借鉴点。

---

## 一、协议 IR 核心代码路径

### 1.1 LlmRequest 结构体定义

**文件**: `crates/protocol/src/llm.rs:303-334`

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmRequest {
    pub model: Option<String>,
    pub instructions: Vec<InstructionBlock>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    pub sampling: SamplingParams,
    pub output: OutputParams,
    pub reasoning: ReasoningParams,
    pub stream: bool,
    pub extensions: ProviderExtensions,
    pub preservation: PreservationMetadata,
}
```

**关键设计点**：
- 整个结构体标注 `#[serde(default)]`，缺失字段走 Default，保证向前兼容 — 供应商添加新字段时不会导致反序列化失败。
- `instructions` 与 `messages` 分离：Anthropic 风格的 system/developer 指令从对话轮次中独立出来，避免混入 `Role::System` 消息。
- `extensions: ProviderExtensions` 用 `Map<String, Value>` 保存非第一类字段，跨格式翻译时这一段被 codec 选择性保留（见 `crates/switchyard-translation/src/codecs/openai_chat/buffered.rs:244` 的 `copy_openai_chat_request_extensions`）。
- `preservation: PreservationMetadata` 在 decode_request 阶段由 codec 写入（见 `crates/switchyard-translation/src/util.rs:226` 的 `capture_request_preservation`）。

### 1.2 ContentBlock::Unknown 无损保留

**文件**: `crates/protocol/src/llm.rs:124-131`

```rust
/// Provider block that has no normalized representation.
Unknown {
    /// Wire format that supplied the block.
    provider: FormatId,
    /// Exact provider block.
    raw: Value,
},
```

整个枚举标注 `#[serde(tag = "type", rename_all = "snake_case")]`（llm.rs:77），序列化为 `{"type": "unknown", "provider": ..., "raw": ...}`。

**设计要点**：
- `Unknown` 是真正的"逃生舱口"：OpenAI 的 `web_search_call`、Anthropic 的 `server_tool_use`、Codex 的 `compaction` 等无法归一化的块都进这里。
- `raw` 是 `serde_json::Value`（不重写），所以往返无损 — 这是 Switchyard 与传统 typed-ir 设计的最大差异。
- `provider: FormatId` 让跨格式翻译时 codec 能选择是丢弃还是保留（`Raw(Value)` 模式用于更细粒度的字段级 unknown）。
- 类似的"逃生舱"也出现在 `ImageSource::Raw(Value)` (llm.rs:152)、`FileSource::Raw(Value)` (llm.rs:169)、`MediaSource::Raw(Value)` (llm.rs:191)、`ToolChoice::Raw(Value)` (llm.rs:245)。

**对 laew 借鉴**：
- laew 的 `llm/anthropic.rs` / `llm/openai.rs` 把消息直接转成 `serde_json::Value`，没有 IR。新建 `crates/protocol/src/llm.rs` 抽出 IR 是第一步。
- `ContentBlock::Unknown { provider, raw }` 这层兜底值得借鉴：laew 的统一消息模型（`agent/mod.rs`）目前只有 `user/assistant/tool_result` 等已知角色，遇到 Anthropic 的 `thinking` 块或 OpenAI 的 `reasoning` 块就丢失。

### 1.3 PreservationMetadata 存储与恢复

**文件**: `crates/protocol/src/llm.rs:294-301`

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,
    pub responses: BTreeMap<FormatId, Value>,
}
```

写入位置：`crates/switchyard-translation/src/util.rs:226-249`

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

读出位置：`crates/switchyard-translation/src/util.rs:252-273`

```rust
pub fn exact_preserved_request(
    preservation: &PreservationMetadata,
    format: impl Into<FormatId>,
    policy: &TranslationPolicy,
) -> Option<Value> { ... }
```

实际生效在 `crates/switchyard-translation/src/codecs/openai_chat/buffered.rs:186-193`：

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
    let mut diagnostics = Vec::new();
    validate_request_capabilities(request, &mut diagnostics, policy)?;
    // ... 从 IR 重新编码
}
```

**关键细节**：
- 写入是 full clone（`body.clone()`），读取是 `cloned()` 出来一次性消费。这意味着开关 preservation 是按调用级 policy 控制，IR 持有方可以随时丢弃。
- `prepare_request_for_target`（util.rs:280-301）在修改了 prompt 时主动 `preservation.requests.clear()`，避免重放出旧数据。
- `stamp_preserved_request_models`（util.rs:304-315）在改 model 时只重写 OpenAI/Anthropic 三种已知格式的 body（`format.as_str() == WireFormat::OpenAiChat.as_str()`），未知格式丢弃。

**对 laew 借鉴**：
- laew 当前没有 IR，多协议请求在 `Request::raw_body`（`provider_extensions`）里保留原文。当上游服务商在翻译中失败时，可以直接重放原始 body，而不用 panic。

### 1.4 AggLlmResponse 与 ResponseAccumulator

**文件**: `crates/protocol/src/stream.rs:339-348`

```rust
#[derive(Default)]
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

折叠逻辑（stream.rs:366-413）：`MessageStart` 后覆盖前；`TextDelta` 拼接；`ReasoningDelta` 拼接；`ToolCallDelta` 按 `index` 收集到 `PartialToolCall`；最终在 `finish()` (stream.rs:417-447) 输出 `Vec<ContentBlock>`（reasoning → text → tool_calls 顺序）。

测试用例 `into_stream_round_trips_and_around_agg`（stream.rs:652-680）证明 `Agg → Stream → Agg` 完整可逆。

**对 laew 借鉴**：
- laew 的 TUI 同样面对 SSE 流式事件折叠（`tui/engine.rs`）。这个 `ResponseAccumulator` 是教科书式的"按 index 拼接 + 后覆盖前"实现，可以直接抄到 `crates/agent/src/stream_accumulator.rs` 中。

---

## 二、协议翻译核心代码路径

### 2.1 TranslationEngine

**文件**: `crates/switchyard-translation/src/engine.rs:82-95`

```rust
pub struct TranslationEngine {
    registry: FormatRegistry,
    stream_registry: StreamCodecRegistry,
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

`translate_request` (engine.rs:148-169) 的核心路径：

```rust
pub fn translate_request(
    &self,
    source: impl Into<FormatId>,
    target: impl Into<FormatId>,
    body: &Value,
    policy: &TranslationPolicy,
) -> Result<TranslationOutput> {
    let decoded = self.registry.codec(source.clone())?
        .decode_request(body, policy)?;
    let encoded = self.registry.codec(target.clone())?
        .encode_request(&decoded.request, policy)?;
    Ok(TranslationOutput {
        body: encoded.body,
        diagnostics: with_formats(decoded.diagnostics, encoded.diagnostics, source, target),
    })
}
```

### 2.2 FormatCodec trait

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

四个辅助结构体（同文件 21-42）：
- `DecodedRequest { request: LlmRequest, diagnostics: Vec<TranslationDiagnostic> }`
- `EncodedRequest { body: Value, diagnostics: Vec<TranslationDiagnostic> }`
- `DecodedResponse { response: AggLlmResponse, diagnostics: Vec<TranslationDiagnostic> }`
- `EncodedResponse { body: Value, diagnostics: Vec<TranslationDiagnostic> }`

`FormatRegistry::with_builtins` (engine.rs:59-65) 注册三个 codec：

```rust
pub fn with_builtins() -> Self {
    let mut registry = Self::new();
    registry.register(OpenAiChatCodec);
    registry.register(AnthropicMessagesCodec);
    registry.register(OpenAiResponsesCodec);
    registry
}
```

### 2.3 AnthropicMessagesCodec 实现

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
        // ... 逐字段填充 LlmRequest
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

**核心模式**：
1. `object(body, "$")` 先校验顶层是对象，返回 typed error（util.rs:25）。
2. 每个字段都用 `.get(...).and_then(...)` 链式取值；找不到给 `None` 而不是 `Err`，保持 IR 的"宽容缺字段"哲学。
4. `body.get("thinking").cloned()` 把 thinking 整段塞进 `ReasoningParams::raw`，后续按需翻译。

### 2.4 OpenAiChatCodec 实现

**文件**: `crates/switchyard-translation/src/codecs/openai_chat/buffered.rs:33-249`

`encode_request` 的核心（buffered.rs:181-247）：

```rust
fn encode_request(&self, request: &LlmRequest, policy: &TranslationPolicy)
    -> Result<EncodedRequest> {
    if let Some(body) = exact_preserved_request(&request.preservation, WireFormat::OpenAiChat, policy) {
        return Ok(EncodedRequest { body, diagnostics: Vec::new() });
    }
    let mut diagnostics = Vec::new();
    validate_request_capabilities(request, &mut diagnostics, policy)?;
    let mut body = Map::new();
    if let Some(model) = &request.model {
        body.insert("model".to_string(), Value::String(model.clone()));
    }

    let mut messages = Vec::new();
    for instruction in &request.instructions {
        let role = match instruction.role {
            Role::Developer => "developer",
            _ => "system",
        };
        messages.push(json!({
            "role": role,
            "content": text_from_blocks(&instruction.content, "\n\n"),
        }));
    }
    for message in &request.messages {
        messages.extend(encode_message_to_openai(message, &mut diagnostics, policy)?);
    }
    body.insert("messages".to_string(), Value::Array(messages));
    // tools / tool_choice / max_completion_tokens / temperature / top_p / stream / reasoning_effort / response_format
    copy_openai_chat_request_extensions(&mut body, &request.extensions.fields);
    let body = embed_preservation(Value::Object(body), &request.preservation, policy);
    Ok(EncodedRequest { body, diagnostics })
}
```

**关键模式**：
- 第一行短路：先看有没有 preserved body，有就直接返回，不重新编码。
- 把 `instructions` 合并到 `messages[]` 头部（`role: "system"` 或 `"developer"`）。
- `text_from_blocks(&instruction.content, "\n\n")` 把 `Vec<ContentBlock>` 拼成纯文本（`common.rs` 提供）。
- `copy_openai_chat_request_extensions(&mut body, &request.extensions.fields)` 把不属于第一类的字段原样写到顶层。
- 最后 `embed_preservation` 把整个 `PreservationMetadata` 包进 `metadata._switchyard_translation` 键（util.rs:317-344），用于多跳 round trip。

**对 laew 借鉴**：
- 这套 IR + Codec 抽象直接搬到 laew。`validate_request_capabilities` 在翻译前校验 `TargetCapabilities`（policy.rs:48-60 有 `supports_tools/supports_images/...` 一整张白名单），codec 自己跑断言，发现目标不支持的能力就返回 `TranslationError::UnsupportedCapability`，避免把"OpenAI 风格 `reasoning_effort`"误发给 Anthropic 后端。
- `extensions` + `copy_openai_chat_request_extensions` 是 laew 当前缺失的关键能力：现有 `agent/mod.rs::run_session` 在循环里直接用 `serde_json::Value`，遇到未知字段会无声丢失。

---

## 三、路由算法核心代码路径

### 3.1 Algorithm trait 定义

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
        let handle = tokio::spawn(
            async move {
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
            }
            .instrument(span),
        );
        let abort_guard = AbortOnDrop(handle.abort_handle());
        Box::pin(ReceiverStream::new(step_rx).map(move |step| {
            let _keep_alive = &abort_guard;
            step
        }))
    }
}
```

**关键设计**：
- `self: Arc<Self>`：算法实例被 `Arc<dyn Algorithm>` 共享跨请求，每个实现自己保证线程安全。
- `run_stream` 默认实现（trait method）= 启动 `tokio::spawn` + `AssertUnwindSafe` + `catch_unwind` 抓 panic + 通过 `mpsc::Sender<Result<Step>>` 把步骤推给消费者。
- `AbortOnDrop`：消费者 drop 后，guard 触发 `AbortHandle::abort()` 取消算法 task。

**Step enum**（同文件 211-217）：

```rust
pub enum Step {
    CallModel(Box<CallModel>),
    Done(Box<RoutingOutcome>),
}
```

### 3.2 drive() 函数的并发实现

**文件**: `crates/libsy/src/core/algorithm.rs:231-267`

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

**并发机制**：
- `FuturesUnordered` 持有所有 in-flight 的 `serve(call)` 任务，任务返回的 `Result<()>` 通过 `next()` 弹出。
- `tokio::select!` 同时监听 `in_flight.next()` 和 `stream.next()` —— 这意味着算法可一次 emit 多个 `CallModel`，serve 这边按 futures unordered 并发执行，hedging/fan-out 自然实现。
- `Step::Done` 终止循环，`in_flight` 里的悬挂任务会被 drop（在 `Drop` guard `AbortOnDrop` 里 abort）—— 测试 `requests_are_processed_in_parallel`（algorithm.rs:739-787）用 12 个 worker + `Barrier` 证明共享算法是真正并行而非串行。

**对 laew 借鉴**：
- laew 的 `MultiAgentOrchestrator`（`src/agent/yolo.rs`）目前是串行编排：Yolo → 分类 → Main-Work → SubAgent-Work → Quality-Check → SessionContext。`Algorithm` trait + `FuturesUnordered` 模式可以直接复用：SubAgent-Work 可并发执行，Quality-Check 可与下一个 SubAgent 流水线化。

### 3.3 FallThrough<S> 级联实现

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

**核心执行流程**（fall_through.rs:175-292）：

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

**会话状态管理**（fall_through.rs:222-233, 295-318）：
- `session_state()` 取或创建 `Arc<AsyncMutex<S>>`，按 session_id 索引。
- `remove_inactive_sessions` 通过 `Arc::strong_count(&session.state) > 1` 判断是否有活跃请求持有 —— 强引用计数 = 1 时说明没人用，可清理。
- `cleanup_inactive_sessions` 每 1 小时跑一次，`SESSION_STATE_TTL = 1 hour`，过期 session 释放。

**DefaultTarget**（fall_through.rs:71-87）：永远 score 0.0，作为 cascade 最后一个"兜底不 abstain"的 classifier。

**对 laew 借鉴**：
- `FallThrough<S = ()>` 的"processor + classifier 链"模式天然适合 laew 的多 Agent 架构：
  - Processor：ToolSignalProcessor（提取工具调用信号）、SessionContextProcessor（注入历史摘要）。
  - Classifier：YoloClassifier、MainWorkClassifier、QualityCheckClassifier。
  - DefaultTarget：Capable Target（默认路由）。
- 借鉴 `cleanup_inactive_sessions` + `Arc::strong_count`：laew 当前 `session_memory` 表用人工管理，可改为基于 `Arc::strong_count` 的自动 TTL。

### 3.4 LLM Classifier 三模式判断

**文件**: `crates/libsy/src/algorithms/llm_class.rs:218-244`

**Capability 模式核心**：

```rust
impl JudgePolicy for TaskClassifierPolicy {
    type Verdict = TaskClassifierVerdict;

    fn to_classification(&self, verdict: Option<&Self::Verdict>) -> Classification {
        // Judge output is untrusted. An absent, invalid, or inconsistent verdict is
        // ambiguous so the surrounding router applies its configured fallback.
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

**boundary_steps 阈值公式**（llm_class.rs:75-83, 213-215）：

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

- `supported` → threshold = base_threshold（0 步）
- `uncertain` / `unmatched` → threshold = base_threshold + 1·threshold_step（1 步）
- `unsupported` → threshold = base_threshold + 2·threshold_step（2 步）
- `p_solve >= threshold` → efficient target，否则 → capable target

**verdict 验证**（llm_class.rs:60-73）：

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

**任务消息裁剪**（llm_class.rs:95-149）：
- `trim_messages(messages, recent_turn_window)`：保留 system + developer + 第一个 user + 最近 N 轮。
- `window_start` 算法核心：从 newest-to-oldest 扫描，维护 `unpaired: HashSet<&str>`（ToolResult 的 call_id），遇到 ToolCall 就从 set 移除（说明该 call 已被 result 配对）。保证裁剪后的窗口里每个 ToolResult 都有对应的 ToolCall。

**TaskInput 注入路由指令**（llm_class.rs:46-47, 178-186）：

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

**Validation**（llm_class.rs:335-375）：
- `base_threshold ∈ [0, 1]`、`threshold_step ≥ 0`、`base_threshold + 2·threshold_step ≤ 1`
- `max_output_tokens ≥ 1`
- `message_hash_fallback` 必须搭配 `classify_trigger = NewSession`

**对 laew 借鉴**：
- laew Yolo 三档分类（simple/medium/hard）可以引入 `p_solve` + `capability_boundary`，让分类输出置信度。
- `trim_messages` + `window_start` 完美适配 TUI 多轮对话：保留指令 + 首个任务 + 最近 N 轮对话窗口。
- `recent_turn_window.is_some()` 时追加 `TRAILING_ROUTING_INSTRUCTION` 是个非常实用的工程经验，避免 LLM 把对话当成任务回答。

### 3.5 AdvisorGate 实现

**文件**: `crates/libsy/src/algorithms/advisor_gate.rs`

**核心结构**（advisor_gate.rs:91-149）：

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

**关键常量**（advisor_gate.rs:81-89）：
- `MAX_FAILED_CONSULTS = 3`：失败咨询上限（防止 max_reviews 被悄悄消耗）。
- `MAX_TRACKED_SCOPES = 1_024`：跟踪的作用域上限。
- `transcript_max_chars = 200_000`：转录文本上限。
- `BENCH_SESSION_HEADER = "proxy_x_session_id"`：benchmark harness 标头。

**工作流**：
1. Executor 回答每个 client 可见轮次。
2. 终端轮次（`GateTrigger::NoToolCall` 或 `Pattern(String)`）被缓冲。
3. Advisor 审查：`APPROVE` 释放缓冲轮次，`REDO` 把 plan 反馈给 executor 重新生成。
4. 每个 scope（bench/session/instance）最多 `max_reviews` 次审查。
5. Advisor 故障时 `fail_open`（默认 APPROVE）。

**失败处理哲学**（advisor_gate.rs:18-26）：
> Executor 错误总是传播（包括 ContextWindowExceeded，让客户端看到 400 让 agent 压缩）。Advisor 错误遵守 fail_open —— 缓冲轮次作为隐式 APPROVE 通过，退还已消耗的预算，并计入 per-scope 失败上限，超出后停止咨询。

**对 laew 借鉴**：
- AdvisorGate 与 laew 的 Quality-Check Agent 高度同构，但更精细：
  - `GateTrigger::Pattern(String)` 让文本协议 harness（无 tool_use）也能触发 QC。
  - `max_reviews` 预算防止无限重试。
  - `fail_open` 防止 QC 故障变成服务故障。
- laew 当前 QC 是无限重试（`docs/Agent源码调研/Switchyard-深度分析.md` 12.3.4 节），可借鉴引入 `max_reviews` 上限 + `fail_open` 策略。

---

## 四、HTTP 服务器核心代码路径

### 4.1 Axum 路由构建函数

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
        .layer(axum::middleware::from_fn(stamp_request_start))         // 必须放最后
        .with_state(state)
}
```

**关键模式**：
- `DEFAULT_MAX_REQUEST_BODY_BYTES = 32 * 1024 * 1024`（32MB），通过 `DefaultBodyLimit::max` 显式声明。
- `stamp_request_start` 必须在 `with_state` 之前，否则 layer 只包装已有路由。
- 路由按 wire format 一对一：`/v1/chat/completions`（OpenAI Chat）、`/v1/messages`（Anthropic）、`/v1/responses`（OpenAI Responses）。

### 4.2 请求处理主函数

**文件**: `crates/switchyard-server/src/lib.rs:652-714`

```rust
async fn handle_endpoint_inner(
    state: ServerState,
    started: RequestStart,
    headers: HeaderMap,
    body: std::result::Result<Json<Value>, JsonRejection>,
    wire_format: WireFormat,
) -> Response {
    let metadata = metadata_from_headers(headers);
    let routing_log_context = state.routing_log.as_ref()
        .map(|_| routing_log::RoutingLogContext::from_metadata(&metadata));
    let request_log = RequestLogContext {
        started: started.0,
        wire_format,
        requested_model: body.as_ref().ok()
            .and_then(|body| body.0.get("model"))
            .and_then(Value::as_str).map(str::to_string),
        streaming: body.as_ref().ok()
            .and_then(|body| body.0.get("stream"))
            .and_then(Value::as_bool).unwrap_or(false),
        session_id: metadata.session_id.clone(),
        correlation_id: metadata.correlation_id.clone(),
    };

    let response = match llm_json_body(body) {
        Ok(body) => handle_llm_request(state, started, metadata, body, wire_format, routing_log_context).await,
        Err((status, message)) => invalid_body_error(status, message),
    };
    let response = render_error_response(response, wire_format);
    metrics::record_client_response(response.status().as_u16());
    request_log.emit(&response);
    response
}
```

**`handle_llm_request` 流程**（lib.rs:791-848）：
1. `cache_probe = state.track_cache_eligibility.then(|| prefix_probe(&body))` — 缓存资格探测。
2. `resolve_route(&state, metadata, body, wire_format)` — 解析路由（lib.rs:739-789），解码 + 校验 + 选路。
3. `observer = stats_observer(state.stats.clone(), ...)` — 注册观察者（lib.rs:420-459）。
4. `route.execute(request, Some(observer)).await` — 执行算法。
5. `into_http_response(response, wire_format, response_model, request_extensions)` — 编码回 wire format（response.rs:22-48）。
6. `attach_routing_headers(&mut response, served_model.as_str())` — 添加 `x-model-router-selected-model` 头（lib.rs:921-923）。
7. `render_error_response(response, wire_format)` — 错误按 wire_format 渲染（lib.rs:1072-1077）。

### 4.3 错误处理

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

**错误形状按 wire_format 渲染**（lib.rs:1046-1069）：

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

**对 laew 借鉴**：
- laew 当前没有 HTTP 服务器，但 `-p` 单轮模式（`src/main.rs`）直接通过 `LlmClient` 发出请求；如果未来要加 HTTP 服务（哪怕是本地 TUI 调试服务器），Switchyard 的三套 wire format × 路由分发模式可直接抄。

### 4.4 优雅关闭

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

`shutdown::signal()`（crates/switchyard-server/src/shutdown.rs:7-9, 21-44）：跨平台监听 SIGINT（Ctrl-C）+ SIGTERM（Unix only）。

`DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT = Duration::from_secs(30)`（lib.rs:59）。

**对 laew 借鉴**：
- laew 是 CLI + TUI 模式，关机语义不同（用户在 TUI 中 `/exit`）。但 `tokio::select!` 模式可以借鉴到 SubAgent-Work 的并发取消逻辑：父任务被取消时，子任务通过 `AbortHandle` 一并 abort。

---

## 五、LLM 客户端核心代码路径

### 5.1 TranslatingLlmClient::send_encoded

**文件**: `crates/libsy-llm-client/src/client.rs:203-289`

```rust
async fn send_encoded(
    &self,
    backend: &Backend,
    wire_format: WireFormat,
    llm_request: LlmRequest,
    metadata: Option<&Metadata>,
    model: &ModelId,
    endpoint: UpstreamEndpoint,
) -> Result<EncodedResponse> {
    let mut body = encode_request(&llm_request, wire_format)
        .map_err(|error| LlmClientError::RequestEncoding(error.to_string()))?;
    set_json_model(&mut body, model);
    if matches!(backend, Backend::Anthropic(_)) {
        strip_anthropic_incompatible_fields(&mut body);
        strip_unsigned_thinking_blocks(&mut body);
    }
    merge_extra_body(&mut body, backend.extra_body());
    if matches!(backend, Backend::Anthropic(_)) {
        enable_anthropic_prompt_caching(&mut body);
    }
    if matches!(backend, Backend::OpenAiChat(_)) {
        ensure_openai_stream_usage(&mut body);
    }
    let streaming = endpoint.allows_streaming()
        && body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let url = endpoint.url(backend);
    record_gen_ai_request(&url, model, streaming);

    let max_retries = u64::from(backend.max_retries());
    let max_attempts = max_retries + 1;
    let mut attempt = 0_u64;
    loop {
        let span = tracing::debug_span!(
            target: "libsy",
            "libsy.upstream_attempt",
            model = %model,
            wire_format = %wire_format,
            attempt = attempt + 1,
            max_attempts,
            retry = attempt > 0,
            openinference.span.kind = "CHAIN",
            outcome = tracing::field::Empty,
            status_code = tracing::field::Empty,
            will_retry = tracing::field::Empty,
            retry_delay_ms = tracing::field::Empty,
        );
        let result = self.send_once(&url, backend, &body, metadata, model, streaming)
            .instrument(span.clone()).await;
        match result {
            Ok(response) => {
                span.record("outcome", "success");
                span.record("status_code", response.status());
                span.record("will_retry", false);
                if attempt > 0 {
                    metrics::record_retry_recovered();
                }
                return Ok(response);
            }
            Err(failure) => {
                let will_retry = attempt < max_retries && failure.is_retryable();
                span.record("outcome", "error");
                if let Some(status) = failure.status {
                    span.record("status_code", status.as_u16());
                }
                span.record("will_retry", will_retry);
                if !will_retry { return Err(failure.error); }

                let delay = retry_delay(attempt, failure.retry_after);
                span.record("retry_delay_ms", duration_millis(delay));
                drop(span);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}
```

### 5.2 请求清洗（strip / merge / cache）

**文件**: `crates/libsy-llm-client/src/client.rs`

**strip_anthropic_incompatible_fields**（client.rs:699-704）：

```rust
fn strip_anthropic_incompatible_fields(body: &mut Value) {
    if let Value::Object(object) = body {
        object.remove("reasoning_effort");
        object.remove("context_management");
    }
}
```

**strip_unsigned_thinking_blocks**（client.rs:713-744）：

```rust
fn strip_unsigned_thinking_blocks(body: &mut Value) {
    let Value::Object(object) = body else { return; };
    let Some(Value::Array(messages)) = object.get_mut("messages") else { return; };
    for message in messages {
        strip_unsigned_thinking_from_message(message);
    }
}

fn strip_unsigned_thinking_from_message(message: &mut Value) {
    let Value::Object(message) = message else { return; };
    let Some(Value::Array(blocks)) = message.get("content") else { return; };
    if !blocks.iter().any(is_unsigned_thinking_block) { return; }
    let Some(Value::Array(blocks)) = message.get_mut("content") else { return; };
    blocks.retain(|block| !is_unsigned_thinking_block(block));
    if blocks.is_empty() {
        message.insert("content".to_string(), Value::String(String::new()));
    }
}
```

**merge_extra_body**（client.rs:758-783）：合并目标默认值，不覆盖调用者提供的字段。

**enable_anthropic_prompt_caching**（client.rs:785-819）：

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

`MAX_CACHE_CONTROL_BLOCKS = 4`（Anthropic 限额）。`cache_control` 标记在最后一个 content block 上。

**ensure_openai_stream_usage**（client.rs:822-842）：

```rust
fn ensure_openai_stream_usage(body: &mut Value) {
    let Value::Object(object) = body else { return; };
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

**对 laew 借鉴**：
- laew 的 `client.rs`（`crates/agent/src/client/` 假设）目前在 agent 循环里直接通过 `reqwest` 调用，没有 strip/merge/cache 这一层。当用户配置 OpenAI 后端时，应该自动 strip `reasoning_effort`（Anthropic 专属）；当用户配置 Anthropic 后端时，应该自动 enable `cache_control` 最多 4 次。这套清洗逻辑建议直接搬到 `crates/llm/src/backend_anthropic.rs`。

### 5.3 指数退避重试

**文件**: `crates/libsy-llm-client/src/client.rs:54-56, 587-624`

```rust
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

impl AttemptFailure {
    fn is_retryable(&self) -> bool {
        match &self.error {
            LlmClientError::Transport { .. } | LlmClientError::Timeout { .. } => true,
            LlmClientError::UpstreamHttp { status, .. } => {
                metrics::is_retryable_http_status(status.as_u16())
            }
            _ => false,
        }
    }
}

fn retry_after_delay(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    let delay = if let Ok(seconds) = value.parse::<u64>() {
        Duration::from_secs(seconds)
    } else {
        let retry_at = httpdate::parse_http_date(value).ok()?;
        retry_at.duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO)
    };
    Some(delay.min(MAX_RETRY_AFTER))
}

fn retry_delay(retry_number: u64, retry_after: Option<Duration>) -> Duration {
    // Retry-After wins; otherwise double 250 ms up to the two-second cap.
    retry_after.unwrap_or_else(|| {
        let multiplier = 1_u32 << retry_number.min(3);
        INITIAL_RETRY_DELAY
            .saturating_mul(multiplier)
            .min(MAX_RETRY_BACKOFF)
    })
}
```

**退避序列**：
- 第 1 次重试：250ms（multiplier=1）
- 第 2 次重试：500ms（multiplier=2）
- 第 3 次重试：1000ms（multiplier=4）
- 第 4+ 次重试：2000ms（multiplier=8，封顶）

**尊重 Retry-After 头**：
- 解析为秒数（如 `Retry-After: 30`）。
- 解析为 HTTP-date（如 `Retry-After: Wed, 21 Oct 2015 07:28:00 GMT`）。
- 上限 60s，防止上游恶意拖延。

**可重试条件**：
- `Transport` / `Timeout` → 总是重试。
- `UpstreamHttp` 状态码：`is_retryable_http_status(status.as_u16())`（429/500/502/503/504）。
- 其它（含 `ContextWindowExceeded` / `InvalidRequest`）不重试。

**对 laew 借鉴**：
- laew 的 `LlmClient` 目前没有重试逻辑（`agent/mod.rs::complete` 直接调用一次失败就返回）。可立即引入：
  - `INITIAL_RETRY_DELAY = 250ms`、`MAX_RETRY_BACKOFF = 2s`、`MAX_RETRY_AFTER = 60s`。
  - `is_retryable` 区分 429/5xx 与 4xx 客户端错误。
  - 400 错误根据 body 关键词（"prompt is too long" / "maximum context length"）分类为 `ContextWindowExceeded`，不重试。

---

## 六、技能蒸馏核心代码路径

### 6.1 Trajectory 结构体

**文件**: `crates/switchyard-skill-distillation/src/model.rs:129-150`

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trajectory {
    pub schema_version: u16,                  // SCHEMA_VERSION = 1
    pub id: SkillEvidenceId,                  // 证据 ID（去重 + 追溯）
    pub task: TaskDescriptor,                 // { description, task_id?, metadata }
    pub execution: ExecutionMetadata,         // { harness?, model?, started_at?, ended_at? }
    pub source: TrajectorySourceInfo,         // { kind, id?, metadata }
    pub events: Vec<TrajectoryEvent>,         // 有序事件
    pub outcome: Option<TrajectoryOutcome>,   // { label?, score?, error?, metrics }
    pub metadata: Metadata,                   // BTreeMap<String, Value>
}
```

**TrajectoryEventKind**（model.rs:70-86）：`Message / ToolCall / ToolResult / Observation / Error / FinalOutput`，`#[non_exhaustive]` 允许扩展。

**Validation**（model.rs:152-172）：检查 schema_version == SCHEMA_VERSION，task.description 非空，event sequence 连续从 0 开始，outcome.score 有限（`is_finite()`）。

### 6.2 SkillCandidate 结构体

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

**SkillProvenance**（model.rs:266-282）：记录来源轨迹 ID、父版本（用于增量更新）、生成器、时间戳。

**ValidationReport**（model.rs:314-329）：`status: ValidationStatus`（`Passed/Failed/NeedsReview`）+ `checks: Vec<ValidationCheck>` + `metrics: BTreeMap<String, f64>` + `notes: Vec<String>` + `evaluated_at`。

### 6.3 SkillDistiller trait

**文件**: `crates/switchyard-skill-distillation/src/ports.rs:24-29`

```rust
#[async_trait]
pub trait SkillDistiller: Send + Sync {
    /// Produces a candidate without implicitly activating it.
    async fn distill(&self, request: &DistillationRequest) -> Result<SkillCandidate>;
}
```

### 6.4 SkillValidator trait

**文件**: `crates/switchyard-skill-distillation/src/ports.rs:32-40`

```rust
#[async_trait]
pub trait SkillValidator: Send + Sync {
    /// Returns validation evidence; activation remains a caller decision.
    async fn validate(
        &self,
        candidate: &SkillCandidate,
        evaluation: &[Trajectory],
    ) -> Result<ValidationReport>;
}
```

**测试 StubDistiller/StubValidator**（`crates/switchyard-skill-distillation/tests/contracts.rs:140-173`）：

```rust
#[async_trait]
impl SkillDistiller for StubDistiller {
    async fn distill(&self, request: &DistillationRequest) -> Result<SkillCandidate> {
        request.validate()?;
        candidate(request.namespace.clone(), "v1")
    }
}

#[async_trait]
impl SkillValidator for StubValidator {
    async fn validate(&self, candidate: &SkillCandidate, evaluation: &[Trajectory])
        -> Result<ValidationReport> {
        candidate.validate()?;
        Ok(ValidationReport {
            status: ValidationStatus::Passed,
            checks: vec![ValidationCheck {
                name: "has-evidence".to_string(),
                status: ValidationStatus::Passed,
                message: Some(format!("{} trajectories", evaluation.len())),
                metrics: BTreeMap::new(),
            }],
            metrics: BTreeMap::new(),
            notes: Vec::new(),
            evaluated_at: "2026-06-30T00:03:00Z".to_string(),
        })
    }
}
```

**SkillStore trait**（ports.rs:42-60）：`active() / save_candidate() / activate() / rollback()`。

**DistillationRequest::validate_candidate**（model.rs:226-262）：

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

**对 laew 借鉴**：
- laew 当前没有显式的 skill 概念，但 SessionContext 摘要机制（`session.rs` 注入 `<<<LAEW:SESSION_HISTORY>>>` 标记）已经在做类似事情。
- 可借鉴 `Trajectory` + `SkillCandidate` 模型：
  - 每次 SubAgent-Work 完成后写入 Trajectory。
  - 定期调用 SkillDistiller 蒸馏。
  - 激活前 SkillValidator 校验。
- `DistillationRequest::validate_candidate` 的"provenance 必须在 request 内" + "parent_version 必须匹配 base_skill" 双约束值得借鉴，防止训练-推理漂移。

---

## 七、可观测性核心代码路径

### 7.1 Prometheus 指标的注册和收集

**文件**: `crates/switchyard-server/src/metrics.rs:44-71`

```rust
fn initialize() -> Result<Metrics, String> {
    let registry = Registry::new();
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .build()
        .map_err(|error| format!("failed to initialize Prometheus metrics: {error}"))?;
    let mut builder = SdkMeterProvider::builder()
        .with_reader(exporter)
        .with_view(routing_overhead_buckets)
        .with_view(llm_latency_buckets)
        .with_resource(crate::observability::resource());
    if crate::observability::otlp_enabled("METRICS") {
        let exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .build()
            .map_err(...)?;
        builder = builder.with_periodic_exporter(exporter);
    }
    let provider = builder.build();
    global::set_meter_provider(provider.clone());
    switchyard_llm_client::initialize_metrics();
    global::meter("switchyard")
        .u64_gauge("switchyard.build_info")
        .build()
        .record(1, &[KeyValue::new("version", env!("CARGO_PKG_VERSION"))]);
    seed_outcome_metrics();
    Ok(Metrics { registry, provider })
}
```

**直方图桶边界**（metrics.rs:17-27）：

```rust
const ROUTING_OVERHEAD_BUCKETS_MS: &[f64] = &[
    0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0,
];
const LLM_LATENCY_BUCKETS_MS: &[f64] = &[
    0.0, 5.0, 10.0, 25.0, 50.0, 75.0, 100.0, 250.0, 500.0, 750.0,
    1000.0, 2500.0, 5000.0, 7500.0, 10_000.0, 15_000.0, 30_000.0, 60_000.0, 120_000.0, 300_000.0,
];
```

`seed_outcome_metrics`（metrics.rs:113-134）：预注册所有可能的状态码（200/404/429/500/504/None），便于仪表盘在首次命中前就显示指标。

### 7.2 OTLP 导出的实现

**文件**: `crates/switchyard-server/src/observability.rs:75-96, 145-156`

```rust
pub(crate) fn otlp_enabled(signal: &str) -> bool {
    if env_var_is_true("OTEL_SDK_DISABLED") {
        return false;
    }
    if env::var(format!("OTEL_{signal}_EXPORTER"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|value| {
            !value.split(',').any(|exporter| exporter.trim().eq_ignore_ascii_case("otlp"))
        })
    {
        return false;
    }
    [
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        &format!("OTEL_EXPORTER_OTLP_{signal}_ENDPOINT"),
    ].into_iter()
    .any(|name| env::var(name).is_ok_and(|value| !value.trim().is_empty()))
}

fn build_tracer_provider() -> Result<SdkTracerProvider, String> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
        .map_err(|error| format!("failed to initialize OTLP trace exporter: {error}"))?;
    let provider = SdkTracerProvider::builder()
        .with_resource(resource())
        .with_batch_exporter(exporter)
        .build();
    opentelemetry::global::set_tracer_provider(provider.clone());
    Ok(provider)
}
```

**OTLP 启用条件**：
- `OTEL_SDK_DISABLED` 不为 true
- `OTEL_{SIGNAL}_EXPORTER` 不包含 `otlp`（如果显式禁用 OTLP）
- `OTEL_EXPORTER_OTLP_ENDPOINT` 或 `OTEL_EXPORTER_OTLP_{SIGNAL}_ENDPOINT` 已设置

### 7.3 W3C Trace Context 传播

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

**测试**（observability.rs:172-199）：注入 `traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01` + `tracestate: vendor=opaque-value`，验证 `span_context.trace_id() == "4bf92f3577b34da6a3ce929d0e0e4736"`，且 `tracestate` 被保留。

**OpenInference 语义约定**（`crates/libsy/src/observability.rs:68-110`）：

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
    if let Some(route) = request.model_id() {
        span.record("switchyard.route", route.as_ref());
    }
    if let Some(metadata) = &request.metadata {
        for (field, value) in [
            ("session_id", &metadata.session_id),
            ("agent_id", &metadata.agent_id),
            ("task_id", &metadata.task_id),
            ("task_kind", &metadata.task_kind),
            ("agent_role", &metadata.agent_role),
            ("correlation_id", &metadata.correlation_id),
        ] {
            if let Some(value) = value {
                span.record(field, value.as_str());
            }
        }
        ...
    }
    span
}
```

**对 laew 借鉴**：
- laew 目前没有 Prometheus/OTel 集成（仅有 `tracing` 日志）。可借鉴：
  - `seed_outcome_metrics` 预注册所有可能的指标值。
  - `W3C Trace Context` 注入：客户端 → Switchyard → 上游 LLM，全链路追踪。
  - OpenInference 语义约定 `openinference.span.kind = "CHAIN"` 与 Phoenix/Arize 等观测平台兼容。

---

## 八、对 laew 工程的最终借鉴清单

### 8.1 短期（P0）立即借鉴

#### 1. 引入 `crates/protocol` crate 与 IR

**现状**：laew 的 `src/agent/mod.rs::run_session` 直接用 `serde_json::Value`，协议差异渗透到 agent 循环。

**借鉴**：
- 把 `LlmRequest / LlmResponse / ContentBlock::Unknown / PreservationMetadata` 直接搬到 laew `src/agent/protocol/`。
- 把统一消息模型 `Message { role: Role, content: Vec<ContentBlock> }` 重构到 laew 当前 `Vec<Message>` 类似的结构。
- 立即收益：agent 循环完全不接触协议细节，新增协议（Codex Responses）只需实现 `FormatCodec`。

#### 2. 引入 `ContentBlock::Unknown` 兜底

**现状**：laew 的统一消息模型遇 Anthropic `thinking` 块或 OpenAI `reasoning` 块会丢失。

**借鉴**：
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

**借鉴**：
```rust
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,
    pub responses: BTreeMap<FormatId, Value>,
}
```

翻译失败时可回退原始体重试。

#### 4. 引入指数退避重试

**借鉴**（直接抄到 `src/llm/client.rs`）：

```rust
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

fn retry_delay(retry_number: u64, retry_after: Option<Duration>) -> Duration {
    retry_after.un_or_else(|| {
        let multiplier = 1_u32 << retry_number.min(3);
        INITIAL_RETRY_DELAY.saturating_mul(multiplier).min(MAX_RETRY_BACKOFF)
    })
}
```

### 8.2 中期（P1）借鉴

#### 5. 引入 `Algorithm` trait + `Driver` + `Step` 流

**现状**：`YoloRunner` 硬编码双 Agent 架构。

**借鉴**：
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

**收益**：SubAgent-Work 可并发执行，Quality-Check 可与下一个 SubAgent 流水线化。

#### 6. 引入 `FallThrough<S>` 级联模式

**借鉴**：
```rust
pub struct FallThrough<S = ()> {
    processors: Vec<Arc<dyn Processor<S>>>,
    classifiers: Vec<Arc<dyn Classifier<S>>>,
    targets: Vec<ModelId>,
    session_states: Option<Arc<SessionStates<S>>>,
}
```

Yolo 分类 → Plan 决策 → Main-Work 编排 → Quality-Check 审查作为 Classifier 链，第一个给出高置信度的胜出；全部 abstain 时用 DefaultTarget（默认 capable 模型）。

#### 7. 引入 `LLM Classifier` 三模式（Capability / Escalation / Custom）

**借鉴**：
- `Capability` 模式：judge 输出 `{crux, primary_rule, capability_boundary, p_solve}`，threshold 公式 `base_threshold + boundary_steps * threshold_step`。
- `Escalation` 模式：先 efficient，连续 N 次 escalate 判决后 latch 到 capable。
- `recent_turn_window` + `trim_messages` + `window_start` 控制 judge 可见的对话轮数。

#### 8. 引入 W3C Trace Context 传播

**借鉴**：
```rust
pub fn request_span(headers: &HeaderMap) -> tracing::Span {
    let parent = TraceContextPropagator::new().extract(&HeaderExtractor(headers));
    let span = tracing::info_span!("laew.request", otel.kind = "server");
    span.set_parent(parent);
    span
}
```

### 8.3 长期（P2）借鉴

#### 9. 引入 `AdvisorGate` 精细化 QC

**借鉴**：
- `GateTrigger::NoToolCall / Pattern(String)` 让 QC 在特定条件触发。
- `max_reviews` 预算防止无限重试。
- `fail_open: bool` 顾问失败时默认 APPROVE。
- `MAX_FAILED_CONSULTS = 3` 防止顾问持续故障时消耗预算。

#### 10. 引入 `AffinityRouter` 粘性路由

**借鉴**：
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

Session 级别的粘性路由，同一 session 的请求路由到同一模型，提高缓存命中率。

#### 11. 引入 `StageRouter` 信号驱动路由

**借鉴**：`ToolSignalProcessor` + `StageClassifier`，基于工具调用结果信号决定下一步，无需额外 LLM 调用。

#### 12. 引入 `SkillDistiller` 数据模型

**借鉴**：`Trajectory` + `SkillCandidate` + `SkillProvenance` 数据模型，支持可追溯的技能生成与验证。

---

## 九、总结

Switchyard 的核心机制遵循"协议层 → 翻译层 → 算法层 → 客户端层 → 服务器层 → 观测层"的六层分离，每层通过 trait 抽象层间接口，且大量运用 Rust 高级特性：

1. **`Arc<dyn Algorithm>` + `mpsc::channel<Result<Step>>`** 实现"宿主驱动算法"的反向控制流，host 提供 serve 闭包即可控制 LLM 调用实现。
2. **`FuturesUnordered` + `tokio::select!`** 实现多 CallModel 并发服务，支持 hedging/fan-out 而无需算法额外编码。
3. **`#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields")]`** 实现配置多态反序列化，TOML 配置驱动算法选择。
4. **`#[tracing::instrument(skip_all, fields(...))]`** 实现 OpenInference 语义约定 span，字段后期填充。
5. **`tracing::field::Empty` + `span.record(...)`** 实现 span 字段的延迟写入（span 创建时不知道值）。
6. **`AsyncUnwindSafe` + `AssertUnwindSafe` + `catch_unwind`** 实现算法 panic 捕获，避免 detached task 静默失败。
7. **`abort_guard: AbortOnDrop`** 实现"流 drop 时 task abort"的生命周期管理。
8. **`Once::call_once` + `Weak::upgrade`** 实现"首次请求启动后台清理任务"模式。
9. **`Arc::strong_count(&session.state) > 1`** 通过引用计数判断 session 是否仍被使用。
10. **`tokio::select!` 监听 Ctrl-C + SIGTERM** 跨平台优雅关闭。

对 laew 的启示集中于三点：
- **协议 IR + Unknown 兜底**：消除协议差异对 agent 循环的渗透。
- **Algorithm trait + Driver/Step 流**：让多 Agent 架构可配置化、可并发。
- **指数退避 + Retry-After + Capability 校验**：让 laew 具备生产级 LLM 容错能力。

Switchyard 是 laew 在 Rust 实现、协议翻译、路由算法三个方向上的标杆实践。