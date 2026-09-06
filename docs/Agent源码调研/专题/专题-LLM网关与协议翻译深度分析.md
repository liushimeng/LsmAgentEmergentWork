# 专题：LLM 网关与协议翻译模式深度分析

> **元信息**
> - 生成日期：2026-09-05
> - 分析维度：协议 IR 设计 / 翻译引擎 / 多提供商路由 / 缓存 / 本地代理 / Rust 实现
> - 主要参考项目：Switchyard（Rust）、agent-studio（Python）、cc-switch（Tauri 2 + Rust）
> - 对标对象：laew `src/llm/`（anthropic.rs + openai.rs）+ `src/agent/mod.rs` 双协议支持
> - 横向专题编号：专题 13（接沙箱/权限/质检等 14 个专题合集之后）

---

## 目录

- [1. 横向对比总览](#1-横向对比总览)
- [2. Switchyard：协议 IR 设计的标杆](#2-switchyard协议-ir-设计的标杆)
- [3. 协议翻译引擎模式对比](#3-协议翻译引擎模式对比)
- [4. 多提供商路由算法体系](#4-多提供商路由算法体系)
- [5. 缓存与请求清洗模式](#5-缓存与请求清洗模式)
- [6. 本地代理与网关架构](#6-本地代理与网关架构)
- [7. Rust 实现细节深度对比](#7-rust-实现细节深度对比)
- [8. 流式事件翻译状态机](#8-流式事件翻译状态机)
- [9. 元数据归一化与多 Agent 协同头](#9-元数据归一化与多-agent-协同头)
- [10. laew 当前实现剖析](#10-laew-当前实现剖析)
- [11. 对 laew 的 P0/P1/P2 借鉴路线图](#11-对-laew-的-p0p1p2-借鉴路线图)
- [12. 总结与设计模式提炼](#12-总结与设计模式提炼)

---

## 1. 横向对比总览

### 1.1 项目基本盘

| 项目 | 语言 | 协议支持 | 核心定位 | 关键模块 |
|------|------|---------|---------|---------|
| **Switchyard** | Rust | OpenAI Chat / Anthropic Messages / OpenAI Responses | LLM 流量代理与库（NVIDIA NeMo） | `protocol` / `translation` / `libsy` / `libsy-llm-client` / `switchyard-server` / `switchyard-runner` |
| **agent-studio（openJiuwen Studio）** | Python (FastAPI) | OpenAI 兼容 + Azure/Ollama/SiliconFlow | 一站式 AI Agent 开发平台 | `core/executor/llm_manager` / `model_config` |
| **cc-switch** | Tauri 2 + Rust | 多 Provider 本地代理 + 熔断 | 客户端配置切换器（Provider Switcher） | `src-tauri/src/` `provider.rs` `proxy.rs` |
| **claude-code** | TypeScript | Anthropic 为主 | 终端编码 Agent | `src/services/api/` |
| **opencode** | TypeScript (Effect) | 多 Provider（ProviderV2 ID 抽象） | AI coding CLI | `packages/opencode/src/provider/` |
| **atomcode** | Rust | Provider-neutral via `LlmProvider` trait | 内核中立 SDK | `crates/atomcode-kernel/src/provider.rs` |
| **laew** | Rust | anthropic-messages + openai-completions | LLM Agent CLI | `src/llm/anthropic.rs` + `src/llm/openai.rs` |

### 1.2 协议 IR 设计光谱

```
手写兼容代码(无 IR)                        Provider-neutral IR(强抽象)
←──────────────────────────────────────────────────────────────────→
laew(直接 wire)  cc-switch(直接 wire)  agent-studio(client 复用)  opencode(ProviderV2)  atomcode(LlmProvider)  Switchyard(LlmRequest)
```

laew 当前位于光谱最左端——直接在 `llm/anthropic.rs` 和 `llm/openai.rs` 操作线格式 JSON，没有协议 IR 抽象。Switchyard 处于光谱最右端——拥有完整的 `LlmRequest`/`LlmResponse` 中间表示 + `ContentBlock::Unknown` 无损保留 + `PreservationMetadata` 同格式重放。

### 1.3 多提供商支持深度对比

| 维度 | Switchyard | agent-studio | cc-switch | opencode | atomcode | laew |
|------|-----------|--------------|-----------|----------|----------|------|
| **协议抽象层** | `LlmRequest` IR + `FormatCodec` trait | `_CLIENT_PROVIDER_MAP` 字典 | 直接 HTTP 转发 | `ProviderV2` 接口 | `LlmProvider` trait | 两个客户端硬编码 |
| **协议数量** | 3（Chat/Messages/Responses） | 4+（OpenAI/Azure/Ollama/SiliconFlow） | 30+ Provider 配置 | 20+ Provider | 通过 trait 自由扩展 | 2（Anthropic/OpenAI） |
| **路由算法** | 7 种 + FallThrough 级联 | is_active 单选 | Provider 手动切换 | 按模型名 + 优先级 | 由宿主决定 | Provider 手动切换 |
| **本地代理** | Axum HTTP 服务器 | FastAPI 后端 | 本地 HTTP 代理（axum/hyper） | 无（直连） | 无（库形式） | 无（直连） |
| **熔断/重试** | 指数退避 + Retry-After + 双 reqwest | 无（直连 SDK） | 熔断器 | Effect retry | 由宿主决定 | 无（裸 reqwest） |
| **缓存** | 无（设计上） | `@lru_cache(maxsize=32)` | 无（配置层） | 无 | 无 | 无 |
| **API Key 加密** | TOML 环境变量 | `SecurityUtils.encrypt_api_key` | Tauri safe storage | 由宿主决定 | 由宿主决定 | SQLite 明文（待改进） |
| **流式支持** | SSE + 流式翻译状态机 | 流式执行（AsyncGenerator） | SSE 透传 | Stream + Effect | 由宿主决定 | SSE 解析（sse.rs） |
| **可观测性** | Prometheus + OTLP | Trace SDK | 日志 | Effect 日志 | tracing | tracing |
| **指标** | 7 项（latency/routing/overhead/...） | trace_id | 无 | 无 | 无 | 无 |

### 1.4 总览图

```
┌────────────────────────────────────────────────────────────────────────────┐
│                       LLM 网关与协议翻译模式对比                              │
├────────────────────────────────────────────────────────────────────────────┤
│                                                                            │
│   Switchyard (NVIDIA)            agent-studio (华为)        cc-switch      │
│   ┌──────────────────┐          ┌──────────────────┐    ┌──────────────┐  │
│   │ 6 层 workspace   │          │ FastAPI + 4+ LLM │    │ Tauri 2      │  │
│   │ protocol IR      │          │ LRU cache(32)    │    │ axum proxy   │  │
│   │ 7 算法路由       │          │ API Key 加密     │    │ 30+ Provider │  │
│   │ 3 协议 Codec     │          │ multi-tenant     │    │ 熔断器       │  │
│   └──────────────────┘          └──────────────────┘    └──────────────┘  │
│         ↑ IR 设计标杆                  ↑ 缓存/多 LLM          ↑ 本地代理    │
│         │                              │                       │            │
│         └──────────┬───────────────────┴───────────────────────┘            │
│                    │                                                        │
│                    ▼                                                        │
│   ┌──────────────────────────────────────────────────────────────────┐     │
│   │  laew (Rust Agent CLI)                                            │     │
│   │  src/llm/anthropic.rs + openai.rs                                 │     │
│   │  → 直接 wire 操作, 零 IR, 零路由, 零代理                           │     │
│   │  → 借鉴 Switchyard IR + agent-studio 缓存 + cc-switch 本地代理    │     │
│   └──────────────────────────────────────────────────────────────────┘     │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Switchyard：协议 IR 设计的标杆

### 2.1 六层分离架构

Switchyard 采用 Rust Workspace 组织，核心六层通过 `Cargo.toml` 的 `[workspace.dependencies]` 统一版本管理：

```toml
[workspace]
resolver = "3"
members = [
    "crates/libsy",                    # 核心路由算法库
    "crates/libsy-llm-client",         # LLM HTTP 客户端
    "crates/prefill-router",           # 预填充特征提取
    "crates/protocol",                 # Provider-neutral 协议类型
    "crates/switchyard-runner",        # 配置驱动的运行器
    "crates/switchyard-server",        # HTTP 服务器
    "crates/switchyard-translation",   # 协议翻译引擎
]
```

**设计思路**：
- **协议层(protocol)**、**翻译层(translation)**、**算法层(libsy)**、**客户端层(llm-client)**、**服务器层(server)**、**运行器层(runner)** 六层分离
- 每层通过 trait 抽象层间接口
- 公共依赖：tokio、serde、reqwest、tracing、tracing-opentelemetry

### 2.2 LlmRequest IR 完整结构

**文件**：`crates/protocol/src/llm.rs`

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

**设计要点**：
- `#[serde(default)]` 保证向前兼容，缺失字段不会反序列化失败
- `InstructionBlock` 将 system/developer 指令与对话消息分离，符合 Anthropic Messages API 风格
- `extensions: ProviderExtensions` 用 `Map<String, Value>` 保存非第一类字段，用于跨格式翻译时保留供应商特有字段
- `preservation: PreservationMetadata` 保留原始请求/响应体

### 2.3 ContentBlock::Unknown 无损保留机制

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

**关键设计**：
- `#[serde(tag = "type")]` 使用内部标签策略，序列化为 `{"type": "text", "text": "..."}`
- `Unknown` 变体保留无法归一化的供应商块（如 OpenAI 的 `web_search_call`、Anthropic 的 `server_tool_use`）
- `provider: FormatId` 标记来源，跨格式翻译时可选择丢弃或保留
- `raw: Value` 是完整 JSON，保证 100% 无损往返

### 2.4 PreservationMetadata 同格式重放机制

```rust
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, Value>,   // 原始请求体
    pub responses: BTreeMap<FormatId, Value>,  // 原始响应体
}
```

**设计思路**：
- 翻译引擎的默认保留策略：同格式重放时直接返回存储的原始体，而非从 IR 重新编码
- 调用者修改 IR 后必须清除对应条目，否则重放的是旧数据
- 与 `Request::raw_request`（宿主可选保留）严格分离

### 2.5 流式类型与 ResponseAccumulator

**文件**：`crates/protocol/src/stream.rs`

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

**ResponseAccumulator 折叠逻辑**：
- `MessageStart` / `Usage` / `MessageStop`：后覆盖前
- `TextDelta` / `ReasoningDelta`：字符串拼接
- `ToolCallDelta`：按 `index` 分组，`arguments` 拼接后 JSON 解析
- 最终输出顺序：reasoning → text → tool_calls（单输出）

**`into_stream` 反向转换**：
- `AggLlmResponse` → 合成 `LlmResponseChunk` 流
- 仅 text/reasoning/tool_call 有合成表示，其余丢弃（标注为 lossy）

### 2.6 IR 设计的 4 大优势总结

| 优势 | 实现机制 | 收益 |
|------|---------|------|
| **协议差异封闭** | `LlmRequest` 是唯一沟通媒介 | Agent 循环与工具层永不接触 wire 细节 |
| **无损保留** | `ContentBlock::Unknown { provider, raw }` | 新供应商字段不会丢失 |
| **同格式重放** | `PreservationMetadata` 保留原始体 | 同格式请求零损失直接转发 |
| **诊断可观测** | `DecodedRequest { request, diagnostics }` | 翻译警告不中断流程，便于审计 |

### 2.7 对 laew 的启示

laew 当前在 `src/llm/anthropic.rs` 和 `src/llm/openai.rs` 直接操作线格式 JSON（`serde_json::Value`），agent 循环通过 `LlmClient` trait 调用，但每次新增协议都要写一套转换代码。Switchyard 的 IR 设计可让 laew：
1. 引入 `src/protocol/` 子模块，定义 `LlmRequest`/`LlmResponse`/`ContentBlock`
2. agent 循环只与 IR 交互，新增协议只需实现 `FormatCodec`
3. 用 `Unknown` 变体保证新字段无损（如 Anthropic 新增 `thinking` 块时不会丢）

---

## 3. 协议翻译引擎模式对比

### 3.1 TranslationEngine 核心设计

**文件**：`crates/translation/src/engine.rs`

```rust
pub struct TranslationEngine {
    registry: FormatRegistry,              // 缓冲编解码器注册表
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

**三个内建实现**：
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

**`translate_event` 流程**：
1. `source_codec.decode_event(state, event)` → 规范化事件
2. `target_codec.encode_event(state, canonical)` → 目标格式事件
3. 返回 `Vec<Value>`（一对多映射）

**保留机制**：
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

### 3.5 各项目翻译模式对比

| 项目 | 翻译方式 | IR | 流式支持 | 保留策略 |
|------|---------|-----|---------|---------|
| **Switchyard** | `TranslationEngine` + `FormatCodec` trait | 完整 `LlmRequest` IR | SSE 状态机翻译 | `ContentBlock::Unknown` + `PreservationMetadata` |
| **agent-studio** | `_CLIENT_PROVIDER_MAP` 字典分发 | 无（直接调 SDK） | 流式执行（AsyncGenerator） | 无（依赖 SDK） |
| **cc-switch** | 直接 HTTP 转发 | 无 | SSE 透传 | 无 |
| **opencode** | `ProviderV2` 接口按 provider 分发 | 统一 `ProviderV2` 接口 | Effect Stream | 无（provider 各自实现） |
| **atomcode** | `LlmProvider` trait + 协议适配 | 由 provider 实现 | 由 provider 实现 | 无 |
| **laew** | `LlmClient` trait 分发 | 无 IR（直接 wire） | SSE 解析（sse.rs） | 无 |

### 3.6 翻译策略核心模式提炼

**模式 1：Codec Registry（Switchyard）**
- `FormatRegistry` + `FormatCodec` trait
- 按 `FormatId` 分发，每个 codec 自负责 wire ↔ IR
- 翻译 = `decode(source) → encode(target)`

**模式 2：Provider Map（agent-studio / opencode）**
- 字典映射 `provider → ClientFactory`
- 直接调第三方 SDK（如 openai-python），无需 IR
- 简单但耦合第三方

**模式 3：Trait Adapter（atomcode）**
- `LlmProvider` trait，每个供应商实现一次
- 库使用者掌握路由权，灵活性高但工作量大

**模式 4：直接转发（cc-switch / laew）**
- 无翻译层，按 provider 直接转发 wire
- 最简单但无法跨协议

**对 laew 启示**：当前 laew 是模式 4。Switchyard 的模式 1 是工业级实现，适合 laew 后续扩展（已支持 2 协议，将来可能加 Gemini / Ollama / Azure）。

---

## 4. 多提供商路由算法体系

### 4.1 Switchyard 的 7 种路由算法

| 算法 | 文件 | 描述 | 适用场景 |
|------|------|------|---------|
| `Noop` | `noop.rs` | 不调用模型，直接返回 OK | 测试 |
| `Passthrough` | `passthrough.rs` | 单目标直通 | 单一模型 |
| `Random` | `rand.rs` | 加权随机分流（A/B 测试） | 灰度发布 |
| `LlmTaskClassifier` | `llm_class.rs` | LLM 判断（Capability/Escalation/Custom 三模式） | 复杂任务分流 |
| `StageRouter` | `stage.rs` | 信号驱动的阶段路由 | 高效/强模型双层 |
| `CompositeRouter` | `composite.rs` | 组合路由（LLM 判断 + Stage） | 高级路由 |
| `SubagentRouter` | `subagent.rs` | 子 Agent 路由 | 多 Agent 协作 |
| `AdvisorGate` | `advisor_gate.rs` | 执行者 + 审查者门控 | 质检场景 |

### 4.2 Algorithm trait 与 Driver

```rust
#[async_trait]
pub trait Algorithm: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn route(self: Arc<Self>, driver: Driver, request: Request) -> Result<RoutingOutcome>;
    fn run_stream(self: Arc<Self>, request: Request) -> StepStream { ... }
}
```

**Driver 设计**：
```rust
pub struct Driver {
    step_tx: mpsc::Sender<Result<Step>>,
    algorithm: String,
}
```

- `call_model(request, models)` → 通过 `mpsc::channel(1)` 发送 `Step::CallModel`，等待 `oneshot` 返回
- 容量 1 保持背压，宿主消费后才继续
- `finish(result)` 发送 `Step::Done` 终止流

**Step 流**：
```rust
pub enum Step {
    CallModel(Box<CallModel>),  // 算法请求宿主执行 LLM 调用
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

### 4.3 `drive` 函数并发执行

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

**并发特性**：
- `FuturesUnordered` 并发服务多个 `CallModel`，支持 hedging/fan-out
- 算法可一次发出多个调用，第一个响应者胜出

### 4.4 FallThrough 级联骨架

**文件**：`crates/libsy/src/algorithms/fall_through.rs`

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

**执行流程**：
1. **Processor 链**：每个 processor 看到 `Event::Request { request, driver }`，可修改请求或调用模型
2. **Classifier 链**：顺序尝试，第一个给出 `Scores` 的胜出（`argmax`）
3. **DefaultTarget**：所有 classifier 都 abstain 时使用默认目标
4. **Post-decision replay**：每个 processor 看到 `Event::Decision { request, selected_model_id }`，可绑定状态或改写请求

**SessionState 管理**：
- `SESSION_STATE_TTL = 1 hour` — 会话状态过期时间
- `SESSION_CLEANUP_INTERVAL = 1 hour` — 清理任务间隔
- `remove_inactive_sessions` 通过 `Arc::strong_count > 1` 判断是否有活跃请求持有

### 4.5 LLM Classifier 三模式

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
  - supported → 0 步
  - uncertain/unmatched → 1 步
  - unsupported → 2 步
- `p_solve >= threshold` → efficient，否则 → capable

**`trim_messages` 窗口算法**：
```rust
fn trim_messages(messages: &[Message], recent_turn_window: usize) -> Vec<Message> {
    // 1. 保留所有 instruction (System/Developer)
    // 2. 保留第一个 User 消息（opening task）
    // 3. 保留最近 N 轮（从尾部计数）
}
```

**`window_start` 工具配对算法**：
- 从 newest-to-oldest 扫描，维护 `unpaired: HashSet<&str>`
- `ToolResult` → 插入 `tool_call_id`
- `ToolCall` → 移除 `id`
- 当 `unpaired.is_empty()` 且 `start <= counted` 时返回，保证每个 result 都有对应的 call

**Escalation 模式**：
- 先调用 efficient，再用 judge 评判
- 连续 N 次 escalate 判决后 latch 到 capable
- 支持 `recent_turn_window` 控制 judge 可见的对话轮数

**Custom 模式**：
- 用户提供 JSON Schema + TargetSelector 策略
- Judge 输出经 Schema 验证后，用 JSON Pointer 提取目标名

### 4.6 Stage Router 信号驱动路由

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

### 4.7 Advisor Gate 执行者 + 审查者

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

**预算作用域优先级**：
```rust
enum ScopeKey {
    Instance,        // 实例级（无头客户端）
    Client(String),  // 会话级
    Session(String), // 基准级（proxy_x_session_id）
}
```

### 4.8 Affinity Router 粘性路由

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
- `EveryRequest` — 每个请求都分类（无粘性）
- `NewSession` — 新 session 时分类
- `UserTurn` — 用户轮次时分类

### 4.9 路由算法光谱对比

| 项目 | 路由能力 | 算法数量 | 信号驱动 | LLM-as-Judge | 粘性 |
|------|---------|---------|---------|--------------|------|
| **Switchyard** | 多目标 / 多模式 | 7+ | 是（Stage） | 是（3 模式） | 是（Affinity） |
| **agent-studio** | 单目标（is_active） | 1 | 无 | 无 | 无 |
| **cc-switch** | 手动切换 | 1 | 无 | 无 | 无 |
| **opencode** | 按模型名匹配 | 1 | 无 | 无 | 无 |
| **laew** | 手动切换 | 1 | 无 | 无 | 无 |

### 4.10 对 laew 的启示

laew 当前是单路由（手动切换 Provider），Yolo 通过「目的→目标→意图」三步分析做任务分类（功能上类似 Switchyard 的 Capability 模式，但无算法抽象）。

借鉴点：
1. **引入 `Algorithm` trait**：将 Yolo/Plan/Main-Work 抽象为路由算法，可组合
2. **引入 `CallModel` Step 流**：让 LLM 调用异步化，支持并发 hedging
3. **信号驱动路由**：基于工具结果信号决定下一步，无需额外 LLM 调用
4. **粘性路由**：同一 session 路由到同一模型，提高 prompt cache 命中率

---

## 5. 缓存与请求清洗模式

### 5.1 agent-studio 的 LRU 缓存

**代码路径**：`ops/modules/llm/llm_manager.py`

```python
@functools.lru_cache(maxsize=32)
def get_llm_client(model_id: str, source: str = "config") -> Model:
    cfg = _config_service.get_llm_model_info(model_id, source)
    protocol = cfg.get("protocol_config", "")
    model_client_config = ModelClientConfig(
        client_provider=compatible_provider(protocol.get("provider")),
        api_key=protocol.get("api_key", ""),
        api_base=protocol.get("base_url", ""),
        timeout=protocol.get("timeout", 60),
    )
    return Model(model_client_config, ModelRequestConfig())
```

**设计要点**：
- `@lru_cache(maxsize=32)` 缓存客户端实例，避免重复构造
- `source` 参数区分 config（数据库）和外部配置
- 同一 `model_id` 命中缓存直接返回，复用 HTTP 连接池

### 5.2 agent-studio 的 API Key 加密

```python
class ModelConfig(Base):
    __tablename__ = "model_config"
    id: int
    name: str                # 配置名称
    provider: str            # 提供商（openai/anthropic/...）
    model_type: str          # 模型类型
    base_url: str            # API 地址
    api_key: str             # 加密存储
    timeout: int
    is_active: bool
    space_id: str            # 多租户
```

**设计要点**：
- API Key 通过 `SecurityUtils.encrypt_api_key` 加密存储
- 多租户隔离（`space_id`）
- 配置热切换（`is_active` 控制）

### 5.3 Switchyard 的请求清洗流程

**文件**：`crates/libsy-llm-client/src/client.rs`

**`send_encoded` 流程**：
1. `encode_request(&llm_request, wire_format)` — IR → wire
2. `set_json_model(&mut body, model)` — 强制覆盖 model 字段
3. `strip_anthropic_incompatible_fields` — 移除 `reasoning_effort`、`context_management`
4. `strip_unsigned_thinking_blocks` — 移除无签名的 thinking 块（Bedrock 需要）
5. `merge_extra_body` — 合并目标默认值（不覆盖调用者提供的）
6. `enable_anthropic_prompt_caching` — 在最后消息添加 `cache_control: {type: "ephemeral"}`（最多 4 个）
7. `ensure_openai_stream_usage` — 流式请求添加 `stream_options: {include_usage: true}`

### 5.4 Switchyard 重试退避策略

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

- 第 1 次重试：250ms
- 第 2 次重试：500ms
- 第 3 次重试：1000ms
- 第 4+ 次重试：2000ms（封顶）
- 尊重 `Retry-After` 头，但不超过 60s

**可重试条件**：
- `Transport` / `Timeout` 错误 → 可重试
- `UpstreamHttp` 状态码 → `is_retryable_http_status` 判断（429, 500, 502, 503, 504）
- 400 + `is_context_overflow` → `ContextWindowExceeded`（不可重试）

### 5.5 Switchyard 的 400 错误分类

```rust
fn is_context_overflow(&self, body: &str) -> bool {
    // Anthropic: "prompt is too long"
    // OpenAI: "maximum context length"
    // 通过供应商规则匹配
}
```

**错误分类规则**：
- 400 + 上下文溢出 → `ContextWindowExceeded`
- 401 / 403 → 鉴权失败，不重试
- 429 → 限流，遵循 `Retry-After`
- 5xx → 服务端错误，可重试

### 5.6 缓存策略对比

| 项目 | 客户端缓存 | 请求清洗 | 重试退避 | 错误分类 |
|------|----------|---------|---------|---------|
| **Switchyard** | 无（设计上） | 7 步清洗（strip/merge/cache） | 指数退避 + Retry-After | 6+ 状态码分类 |
| **agent-studio** | LRU(32) | 无 | 依赖 SDK | 异常处理 |
| **cc-switch** | 无 | 透传 | 无 | 简单 |
| **opencode** | 无 | 由 ProviderV2 处理 | Effect retry | 由 ProviderV2 处理 |
| **laew** | 无 | 无 | 无 | 简单 |

### 5.7 对 laew 的启示

laew 当前在 `src/llm/anthropic.rs` 和 `src/llm/openai.rs` 直接构造 reqwest 请求，无缓存无重试。借鉴：
1. **客户端 LRU 缓存**：对 `LlmClient` 做 `(provider, model_name, api_key_hash)` 三维缓存
2. **指数退避重试**：429/5xx 自动重试，尊重 `Retry-After`
3. **400 错误分类**：区分上下文溢出 vs 其他，避免无效重试
4. **请求清洗**：在 `encode_request` 阶段 strip 供应商特有字段

---

## 6. 本地代理与网关架构

### 6.1 cc-switch 的本地代理架构

cc-switch 是 Tauri 2 + Rust 实现的 Provider 配置切换器，提供本地 HTTP 代理能力：

**核心模块**：
- `provider.rs` — Provider 配置管理（30+ Provider）
- `proxy.rs` — 本地 HTTP 代理（axum/hyper）
- `circuit_breaker.rs` — 熔断器

**代理路径**：
```
Client (Claude Code / Codex)
    │
    ▼ [OpenAI Chat / Anthropic Messages]
cc-switch 本地代理（127.0.0.1:<port>）
    │
    ├─ 路径解析（按 URL 路由）
    ├─ 熔断器（per-Provider）
    ├─ Provider 切换（按 is_active）
    │
    ▼
Provider 后端（Anthropic / OpenAI / ...）
```

### 6.2 Switchyard 的 Axum HTTP 服务器

**文件**：`crates/switchyard-server/src/lib.rs`

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

### 6.3 Switchyard 的优雅关闭

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

### 6.4 Switchyard 的错误处理

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

**错误形状按 wire_format 渲染**：
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

### 6.5 agent-studio 的 FastAPI 后端

```python
@asynccontextmanager
async def lifespan_func(app: FastAPI):
    # 1. 创建数据库表（Base.metadata.create_all）
    # 2. 初始化记忆引擎（MemoryEngineManager.init）
    # 3. 检查 Alembic 版本
    # 4. 初始化 Runner（支持 Redis checkpoint）
    # 5. 启动触发器调度器（APScheduler）
    yield
    # 关闭：停止调度器
```

### 6.6 本地代理模式对比

| 项目 | 代理层 | 路由能力 | 熔断器 | 优雅关闭 | 错误渲染 |
|------|-------|---------|--------|---------|---------|
| **Switchyard** | Axum HTTP 服务器 | 9 个端点 + 算法路由 | 无（设计选择） | 30s graceful_shutdown | 按 wire_format 渲染 |
| **agent-studio** | FastAPI + 路由 | routers 分发 | 无 | lifespan 关闭 | HTTPException |
| **cc-switch** | axum/hyper 代理 | 路径解析 + 熔断 | 有（CircuitBreaker） | Tauri lifecycle | 直接转发 |
| **laew** | 无（直连） | N/A | N/A | N/A | N/A |

### 6.7 对 laew 的启示

laew 当前是 CLI 直接连接 LLM（无代理层）。借鉴：
1. **可选代理模式**：增加 `laew serve` 子命令启动 axum 代理，可被其他工具复用
2. **优雅关闭**：TUI 退出时确保 in-flight 请求完成
3. **错误渲染**：错误按 wire_format 渲染（Anthropic 用 `type: "error"`，OpenAI 用 `error.code`）
4. **熔断器**：per-Provider 熔断，避免单个 Provider 故障拖垮整个 CLI

---

## 7. Rust 实现细节深度对比

### 7.1 Switchyard 的借用检查与 async/await 结合

Switchyard 的核心驱动代码是 `drive` 函数：

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

**设计要点**：
- `tokio::pin!(stream)` 在栈上 pin，避免堆分配
- `FuturesUnordered` 持有 `Pin<Box<dyn Future>>`，借用检查通过
- `Arc<dyn Algorithm>` 共享跨请求，线程安全由 `Send + Sync + 'static` 保证

### 7.2 Provider-neutral 类型设计

**LlmRequest 完整结构**（重看）：
```rust
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

**设计原则**：
- 所有字段都是 Owned（`String`、`Vec<T>`），无生命周期参数
- `ProviderExtensions` 是 `Map<String, Value>`，避免在主结构体加 `HashMap`
- `PreservationMetadata` 用 `BTreeMap<FormatId, Value>` 保证有序（便于诊断）

### 7.3 优雅关闭 + 错误按 wire_format 渲染

**优雅关闭**（重看）：
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

**错误按 wire_format 渲染**：
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

### 7.4 laew 的请求头统一封装

**文件**：`src/llm/mod.rs::build_common_headers`

laew 已实现请求头统一封装：
```rust
pub fn build_common_headers(...) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("User-Agent", ...);  // {AgentName}/{版本} {编译时间}
    headers.insert("Authorization", format!("Bearer {}", api_key).parse()?);
    headers.insert("X-Session-Id", session_id.parse()?);
    headers
}
```

**Anthropic 专属**：
- `x-api-key: <api_key>`
- `anthropic-version: 2023-06-01`
- `metadata.user_id`（含 `device_id/account_uuid/session_id`）

### 7.5 laew 的协议差异处理

**Anthropic**：
- `tools[].{name, description, input_schema}`
- system 是独立字段（不是消息）
- 流式事件：`message_start` / `content_block_start` / `content_block_delta` / `content_block_stop` / `message_delta` / `message_stop`

**OpenAI**：
- `tools[].{type:"function", function:{name, description, parameters}}`
- system 是消息角色之一
- 流式事件：`chunk.choices[].delta` / `chunk.choices[].finish_reason`

**laew 现状**：`sse.rs` 解析 SSE 流，差异在两个 client 内部处理。

### 7.6 Rust 借用检查对比

| 项目 | 共享状态 | 借用检查模式 | 错误传播 |
|------|---------|------------|---------|
| **Switchyard** | `Arc<dyn Algorithm>` / `Arc<dyn Classifier>` | trait object + Pin | `Result<T, LibsyError>` |
| **atomcode** | `Arc<RwLock<BTreeMap<...>>>` | 内部可变性 | `ToolResult { is_error }` |
| **laew** | 直接拥有（`LlmClient` trait object） | 直接拥有 | `Result<T, AgentError>` 含 `YoloParse` |

### 7.7 对 laew 的启示

laew 已正确使用 `Arc<dyn LlmClient>` 和 `Result<T, AgentError>`。可借鉴：
1. **引入 IR 后用 `Arc<dyn FormatCodec>`**：替代两套独立的 client
2. **Pin + FuturesUnordered**：实现并发 SubAgent（如 Switchyard 的 `drive`）
3. **错误按 wire_format 渲染**：在 TUI 中显示对应供应商的错误形状

---

## 8. 流式事件翻译状态机

### 8.1 Switchyard 的流式翻译

```rust
pub fn translate_event(&self, state, source, target, event) -> Result<Vec<Value>> {
    let source_codec = self.stream_registry.codec(source)?;
    let target_codec = self.stream_registry.codec(target)?;
    let canonical = source_codec.decode_event(state, event);
    Ok(canonical.into_iter().flat_map(|e| target_codec.encode_event(state, e)).collect())
}
```

**一对多映射**：一个 source event 可能产生多个 target events（如 `message_start` 拆分为 `message_start` + `content_block_start`）。

### 8.2 laew 的 SSE 解析

**文件**：`src/llm/sse.rs`

laew 的 SSE 解析直接消费字节流，按供应商分类：

**Anthropic SSE 事件**：
```
event: message_start
data: {"type":"message_start","message":{"id":"msg_...","role":"assistant","content":[]}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

event: message_stop
data: {"type":"message_stop"}
```

**OpenAI SSE 事件**：
```
data: {"id":"chatcmpl-...","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}

data: {"id":"chatcmpl-...","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"}}]}

data: {"id":"chatcmpl-...","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

### 8.3 流式翻译 vs 直接解析

| 模式 | 优点 | 缺点 |
|------|------|------|
| **直接解析**（laew 当前） | 简单，每协议一套解析 | 多协议时重复 |
| **翻译状态机**（Switchyard） | 统一抽象，可跨协议 | 复杂度高，状态机维护 |

### 8.4 对 laew 的启示

laew 当前 2 协议，直接解析是合适的。当协议数增至 4+ 时，可考虑：
1. **抽出 `SseEvent` 规范化类型**：`TextDelta { index, text }` / `ToolCallDelta { index, id, name, args }` / `StopReason`
2. **Anthropic / OpenAI 都翻译到规范化类型**，再在 TUI 层渲染
3. **保留 source raw**：调试时可看到原始事件

---

## 9. 元数据归一化与多 Agent 协同头

### 9.1 Switchyard 的 Harness 归一化

**文件**：`crates/protocol/src/metadata.rs`

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
- **优先级链**：Switchyard 头 > Claude Code 头 > NeMo Relay 头 > Codex JSON 路径 > 通用头
- **Codex 的 `x-codex-turn-metadata` JSON 头**：通过 `resolve_path` 点分路径解析
- **`parse_sub_agent()` 区分 `is_subagent`（血缘事实）与 `is_delegated_work`（路由信号）**
- **子 Agent 工作类型白名单**：`["collab_spawn", "review"]`

### 9.2 laew 的请求头

**当前实现**（CLAUDE.md）：
- 两协议统一携带 `User-Agent: {AgentName}/{版本} {编译时间}`
- `Authorization: Bearer {api_key}`
- `X-Session-Id`
- Anthropic 请求体 additionally 携带 `metadata.user_id`（含 `device_id/account_uuid/session_id`）

**对比 Switchyard**：
- laew 已实现 `X-Session-Id`，但未实现 `X-Agent-Id` / `X-Parent-Agent-Id` / `X-Task-Id` 等多 Agent 协同头
- 未实现优先级链（只有 laew 自己用）

### 9.3 对 laew 的启示

laew 是 6 角色 Agent 架构（Yolo/Plan/Main-Work/SubAgent-Work/QC/SessionContext），但请求头只传递 Session ID。借鉴：
1. **添加 `X-Agent-Name`**：当前执行 Agent（Yolo/Plan/...）
2. **添加 `X-Parent-Agent-Name`**：父 Agent（用于 SubAgent 路由）
3. **添加 `X-Task-Level`**：simple/medium/hard
4. **添加 `X-Session-Memory-Round`**：Yolo 第几次处理（用于上下文控制）

---

## 10. laew 当前实现剖析

### 10.1 模块组织

```
src/llm/
├── mod.rs              # 统一消息模型 + LlmClient trait + RequestMeta + build_common_headers
├── anthropic.rs        # Anthropic wire 转换（x-api-key + anthropic-version + metadata.user_id）
├── openai.rs           # OpenAI wire 转换（Bearer）
└── sse.rs              # SSE 解析

src/agent/
├── mod.rs              # 协议无关循环：run_session(Session) → complete → tool_calls → 执行 → tool_result 回填
├── profile.rs          # AgentProfile + work_profile()/yolo_profile() + User-Agent
├── system_prompt/      # SystemPrompt 组合与渲染
├── tools/              # Tool trait + ToolRegistry + builtin_registry()/yolo_registry()
├── yolo.rs             # YoloRunner 双 Agent 编排器 + TaskLevel + TaskClassification + JSON 解析
├── orchestrator.rs     # MultiAgentOrchestrator
├── context.rs          # AgentContext
├── memory.rs           # AgentMemory
├── project_context.rs  # 项目说明文件注入
├── session_context.rs  # SessionContext
├── plan.rs             # Plan Agent
├── main_work.rs        # Main-Work Agent
├── subagent.rs         # SubAgent-Work Agent
└── quality.rs          # Quality-Check Agent
```

### 10.2 双协议支持现状

**LlmClient trait**：
```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse>;
}
```

**两个实现**：
- `AnthropicClient` — `src/llm/anthropic.rs`
  - 端点：`{end_point}/v1/messages`
  - 头：`x-api-key` + `anthropic-version` + `metadata.user_id`
  - 工具：`tools[].{name, description, input_schema}`
- `OpenAIClient` — `src/llm/openai.rs`
  - 端点：`{end_point}/chat/completions`
  - 头：`Authorization: Bearer`
  - 工具：`tools[].{type:"function", function:{name, description, parameters}}`

### 10.3 wire 转换位置

```
src/agent/mod.rs（协议无关循环）
    │
    ▼ LlmRequest（统一消息模型）
LlmClient trait
    │
    ▼
┌────────────────────┬────────────────────┐
│ AnthropicClient    │ OpenAIClient       │
│ src/llm/anthropic.rs│ src/llm/openai.rs │
└────────────────────┴────────────────────┘
    │                       │
    ▼                       ▼
wire format JSON       wire format JSON
```

### 10.4 当前局限性

| 局限 | 说明 | 影响 |
|------|------|------|
| **无 IR** | 直接操作 `serde_json::Value` | 新增协议成本高 |
| **无注册表** | `AnthropicClient` / `OpenAIClient` 硬编码 | 难以动态加载 |
| **无缓存** | 每次都构造 reqwest client | 资源浪费 |
| **无重试** | 失败立即返回 | 临时网络错误导致任务失败 |
| **无熔断** | 单 Provider 故障影响全部 | 缺乏 failover |
| **无路由** | 单 Provider 手动切换 | 无法多模型分流 |
| **无代理** | 直连 LLM API | 无法外部工具复用 |
| **SSE 重复实现** | 每个协议一套解析 | 代码重复 |

### 10.5 已有可借鉴基础

| 已有能力 | 对标 Switchyard | 升级路径 |
|---------|----------------|---------|
| `LlmClient` trait | `FormatCodec` trait | 加 IR 抽象 |
| `build_common_headers` | `build_common_headers` | 加多 Agent 协同头 |
| `User-Agent: {AgentName}/{版本}` | 相同 | 保留 |
| `X-Session-Id` | `x-session-id` | 加 `X-Agent-Name` / `X-Parent-Agent-Name` |
| `agent_memory` 表 | `RunObservation` | 已实现 |
| 6 角色 Agent 架构 | `Algorithm` trait 组合 | 抽象为 `Agent` trait |
| Yolo 三档分类 | `LlmClassifier` Capability | 升级为带 p_solve 输出 |

---

## 11. 对 laew 的 P0/P1/P2 借鉴路线图

### 11.1 P0（核心能力 — 必须实现）

#### P0-1: 引入 `protocol` 子模块 + IR

**目标**：消除协议差异对 agent 循环的渗透。

**实现**：
```rust
// src/protocol/llm.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmRequest {
    pub model: Option<String>,
    pub instructions: Vec<InstructionBlock>,    // system/developer 指令
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Reasoning { text: String, signature: Option<String> },
    Image { source: ImageSource },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Unknown {
        provider: FormatId,
        raw: serde_json::Value,  // 无损保留
    },
}
```

**收益**：
- agent 循环完全不接触协议细节
- 新增协议只需实现 `FormatCodec`
- `Unknown` 变体保证新字段无损

#### P0-2: ContentBlock::Unknown 无损保留

**实现**：见 P0-1 的 `Unknown` 变体。

**收益**：
- 新供应商字段不会丢失（如 Anthropic 新增 `thinking` 块）
- 跨格式翻译时可选择丢弃或保留

#### P0-3: PreservationMetadata 同格式重放

**实现**：
```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreservationMetadata {
    pub requests: BTreeMap<FormatId, serde_json::Value>,
    pub responses: BTreeMap<FormatId, serde_json::Value>,
}

impl LlmRequest {
    pub fn preserve(&mut self, format: FormatId, raw: serde_json::Value) {
        self.preservation.requests.insert(format, raw);
    }

    pub fn exact_preserved_request(&self, format: FormatId) -> Option<&serde_json::Value> {
        self.preservation.requests.get(&format)
    }
}
```

**收益**：协议转换失败时可回退到原始体重试。

#### P0-4: 指数退避重试

**实现**：
```rust
// src/llm/retry.rs
pub async fn retry_with_backoff<F, Fut, T, E>(
    mut op: F,
    max_retries: u32,
    initial_delay: Duration,
    max_delay: Duration,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: IsRetryable,
{
    let mut delay = initial_delay;
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) if e.is_retryable() && attempt < max_retries => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}
```

**配置**：
- `INITIAL_RETRY_DELAY = 250ms`
- `MAX_RETRY_BACKOFF = 2s`
- `MAX_RETRY_AFTER = 60s`
- 可重试：429 / 500 / 502 / 503 / 504 / Transport / Timeout
- 不可重试：400（上下文溢出）/ 401 / 403

#### P0-5: 客户端 LRU 缓存

**实现**：
```rust
// src/llm/cache.rs
use lru::LruCache;
use std::sync::Mutex;

pub struct ClientCache {
    cache: Mutex<LruCache<CacheKey, Arc<dyn LlmClient>>>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct CacheKey {
    pub provider: String,
    pub model_name: String,
    pub api_key_hash: u64,
    pub end_point: String,
}

impl ClientCache {
    pub fn new(capacity: usize) -> Self {
        Self { cache: Mutex::new(LruCache::new(capacity)) }
    }

    pub fn get_or_create(&self, key: CacheKey, factory: impl FnOnce() -> Arc<dyn LlmClient>) -> Arc<dyn LlmClient> {
        let mut cache = self.cache.lock().unwrap();
        cache.get_or_insert(key, factory).clone()
    }
}
```

**收益**：避免重复构造 reqwest Client，复用 HTTP 连接池。

#### P0-6: 多 Agent 协同头

**实现**：
```rust
// src/llm/mod.rs::build_common_headers
pub fn build_common_headers(
    agent_name: &str,
    session_id: &str,
    parent_agent: Option<&str>,
    task_level: Option<&str>,
    meta: &RequestMeta,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("User-Agent", format!("{}/{}/{}", agent_name, env!("CARGO_PKG_VERSION"), env!("LAEW_BUILD_TIME")).parse().unwrap());
    headers.insert("Authorization", format!("Bearer {}", meta.api_key).parse().unwrap());
    headers.insert("X-Session-Id", session_id.parse().unwrap());
    headers.insert("X-Agent-Name", agent_name.parse().unwrap());
    if let Some(parent) = parent_agent {
        headers.insert("X-Parent-Agent-Name", parent.parse().unwrap());
    }
    if let Some(level) = task_level {
        headers.insert("X-Task-Level", level.parse().unwrap());
    }
    headers
}
```

**收益**：外部可观测性平台可识别 laew 的多 Agent 调用链。

### 11.2 P1（增强能力 — 重要）

#### P1-1: FormatCodec trait + Codec Registry

**实现**：
```rust
// src/protocol/codec.rs
#[async_trait]
pub trait FormatCodec: Send + Sync {
    fn format(&self) -> FormatId;
    fn decode_request(&self, body: &serde_json::Value, policy: &TranslationPolicy) -> Result<DecodedRequest>;
    fn encode_request(&self, request: &LlmRequest, policy: &TranslationPolicy) -> Result<EncodedRequest>;
    fn decode_response(&self, body: &serde_json::Value, policy: &TranslationPolicy) -> Result<DecodedResponse>;
    fn encode_response(&self, response: &AggLlmResponse, policy: &TranslationPolicy) -> Result<EncodedResponse>;
    fn decode_stream_event(&self, state: &mut StreamTranslationState, event: serde_json::Value) -> Result<Vec<LlmResponseChunk>>;
    fn encode_stream_event(&self, state: &mut StreamTranslationState, event: &LlmResponseChunk) -> Result<Vec<serde_json::Value>>;
}

pub struct FormatRegistry {
    codecs: HashMap<FormatId, Arc<dyn FormatCodec>>,
}

impl FormatRegistry {
    pub fn register(&mut self, codec: Arc<dyn FormatCodec>) {
        self.codecs.insert(codec.format(), codec);
    }
    pub fn codec(&self, format: FormatId) -> Result<&Arc<dyn FormatCodec>> {
        self.codecs.get(&format).ok_or(AgentError::UnsupportedFormat(format))
    }
}
```

**收益**：agent 循环与协议细节完全解耦，新增协议只需实现 `FormatCodec` + 注册到 `FormatRegistry`。

#### P1-2: 流式事件规范化

**实现**：
```rust
// src/protocol/stream.rs
#[derive(Debug, Clone)]
pub enum LlmResponseChunk {
    MessageStart { id: String, model: String },
    TextDelta { index: usize, text: String },
    ReasoningDelta { index: usize, text: String },
    ToolCallDelta { index: usize, id: String, name: Option<String>, arguments_delta: String },
    Usage { input_tokens: u32, output_tokens: u32 },
    MessageStop { reason: StopReason },
    DecodeError { message: String },
    StreamError { message: String },
}

pub struct ResponseAccumulator {
    pub id: Option<String>,
    pub model: Option<String>,
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: BTreeMap<usize, PartialToolCall>,
    pub usage: Usage,
    pub stop_reason: Option<StopReason>,
}

impl ResponseAccumulator {
    pub fn apply(&mut self, chunk: LlmResponseChunk) {
        match chunk {
            LlmResponseChunk::MessageStart { id, model } => {
                self.id = Some(id);
                self.model = Some(model);
            }
            LlmResponseChunk::TextDelta { text, .. } => self.text.push_str(&text),
            LlmResponseChunk::ReasoningDelta { text, .. } => {
                self.reasoning.get_or_insert_with(String::new).push_str(&text);
            }
            LlmResponseChunk::ToolCallDelta { index, id, name, arguments_delta } => {
                let entry = self.tool_calls.entry(index).or_insert_with(|| PartialToolCall::new(id));
                if let Some(name) = name {
                    entry.name = Some(name);
                }
                entry.arguments.push_str(&arguments_delta);
            }
            LlmResponseChunk::Usage { input_tokens, output_tokens } => {
                self.usage.input_tokens = input_tokens;
                self.usage.output_tokens = output_tokens;
            }
            LlmResponseChunk::MessageStop { reason } => self.stop_reason = Some(reason),
            _ => {}
        }
    }

    pub fn finish(self) -> AggLlmResponse {
        AggLlmResponse {
            id: self.id.unwrap_or_default(),
            model: self.model.unwrap_or_default(),
            text: self.text,
            reasoning: self.reasoning,
            tool_calls: self.tool_calls.into_iter().map(|(_, v)| v.finish()).collect(),
            usage: self.usage,
            stop_reason: self.stop_reason,
        }
    }
}
```

**收益**：
- 统一处理流式响应的聚合逻辑
- 支持 tool call 参数拼接
- TUI 流式输出时可在翻译层直接转换

#### P1-3: 400 错误分类

**实现**：
```rust
// src/llm/error.rs
#[derive(Debug, thiserror::Error)]
pub enum LlmClientError {
    #[error("invalid request: {message}")]
    InvalidRequest { message: String },
    #[error("context window exceeded: {message}")]
    ContextWindowExceeded { message: String, model: String },
    #[error("upstream http {status}: {body}")]
    UpstreamHttp { status: u16, body: String },
    #[error("timeout")]
    Timeout { source: Box<dyn std::error::Error + Send + Sync> },
    #[error("transport: {0}")]
    Transport { source: Box<dyn std::error::Error + Send + Sync> },
    #[error("auth failed: {message}")]
    AuthFailed { message: String },
}

impl LlmClientError {
    pub fn is_retryable(&self) -> bool {
        matches!(self,
            Self::UpstreamHttp { status, .. } if matches!(*status, 429 | 500 | 502 | 503 | 504),
            Self::Transport { .. } | Self::Timeout { .. }
        )
    }

    pub fn from_response(status: u16, body: &str) -> Self {
        if status == 401 || status == 403 {
            return Self::AuthFailed { message: body.to_string() };
        }
        if status == 400 && (body.contains("prompt is too long") || body.contains("maximum context length")) {
            return Self::ContextWindowExceeded {
                message: body.to_string(),
                model: "unknown".to_string(),
            };
        }
        Self::UpstreamHttp { status, body: body.to_string() }
    }
}
```

**收益**：区分上下文溢出（不可重试）和临时错误（可重试），避免无效重试。

#### P1-4: Yolo p_solve 置信度输出

**实现**：
```rust
// src/agent/yolo.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskClassification {
    pub task_level: TaskLevel,           // simple / medium / hard
    pub p_solve: f64,                    // 解决概率（0.0-1.0）
    pub capability_boundary: String,     // supported / uncertain / unsupported / unmatched
    pub intent: String,                 // 意图描述
    pub rationale: String,              // 判断理由
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskLevel {
    Simple,
    Medium,
    Hard,
}
```

**系统提示词更新**（Yolo）：
```markdown
输出 JSON：
{
  "task_level": "simple" | "medium" | "hard",
  "p_solve": 0.0-1.0,
  "capability_boundary": "supported" | "uncertain" | "unsupported" | "unmatched",
  "intent": "一句话意图描述",
  "rationale": "判断理由"
}
```

**收益**：
- Yolo 输出置信度，低置信度时回退到用户确认
- 与 Switchyard 的 `LlmClassifier` Capability 模式对齐

#### P1-5: AffinityRouter 粘性路由

**实现**：
```rust
// src/llm/affinity.rs
use std::collections::HashMap;
use std::sync::Mutex;

pub struct AffinityRouter {
    assignments: Mutex<HashMap<RoutingIdentity, String>>,
}

#[derive(Hash, Eq, PartialEq, Clone)]
pub enum RoutingIdentity {
    Session(String),
    Subagent { session: String, agent: String },
}

impl AffinityRouter {
    pub fn get(&self, identity: &RoutingIdentity) -> Option<String> {
        self.assignments.lock().unwrap().get(identity).cloned()
    }

    pub fn assign(&self, identity: RoutingIdentity, model: String) {
        self.assignments.lock().unwrap().insert(identity, model);
    }

    pub fn release(&self, identity: &RoutingIdentity) {
        self.assignments.lock().unwrap().remove(identity);
    }
}
```

**集成到 MultiAgentOrchestrator**：
```rust
// 在选择模型前
let affinity = self.affinity.get(&RoutingIdentity::Session(session_id.clone()));
let model = affinity.unwrap_or_else(|| {
    // 默认 Provider 的模型
    self.default_model()
});
```

**收益**：同一 session 路由到同一模型，提高 prompt cache 命中率。

### 11.3 P2（高级能力 — 可选）

#### P2-1: Algorithm trait + Step 流

**实现**：
```rust
// src/agent/algorithm.rs
pub enum Step {
    CallModel(Box<CallModel>),
    Delegate(Box<DelegateRequest>),
    Done(Box<AgentOutcome>),
}

pub struct CallModel {
    pub algorithm: String,
    pub request: LlmRequest,
    pub models: Vec<String>,
    reply: oneshot::Sender<Result<LlmResponse>>,
}

pub struct DelegateRequest {
    pub sub_agent: String,   // Plan / Main-Work / SubAgent-Work / QC / SessionContext
    pub task: String,
    pub context: AgentContext,
    reply: oneshot::Sender<Result<AgentOutcome>>,
}

#[async_trait]
pub trait Agent: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn execute(self: Arc<Self>, driver: Driver, request: AgentRequest) -> Result<AgentOutcome>;
    fn run_stream(self: Arc<Self>, request: AgentRequest) -> StepStream { ... }
}

pub struct Driver {
    step_tx: mpsc::Sender<Result<Step>>,
    agent: String,
}
```

**收益**：
- Yolo/Plan/Main-Work/SubAgent-Work/QC/SessionContext 可配置化组合
- SubAgent-Work 可并发执行（hedging/fan-out）
- Quality-Check 可与下一个 SubAgent 流水线化

#### P2-2: Stage Router 信号驱动路由

**实现**：
```rust
// src/agent/stage_router.rs
pub struct ToolSignalProcessor {
    recent_window: usize,
}

impl ToolSignalProcessor {
    pub fn process(&self, tool_results: &[ToolResult]) -> StageSignal {
        let recent = tool_results.iter().rev().take(self.recent_window);
        let mut signal = StageSignal::default();
        for result in recent {
            if result.is_error {
                signal.errors += 1;
            }
            if let Some(content) = &result.content {
                if content.contains("permission denied") {
                    signal.permission_denied = true;
                }
            }
        }
        signal
    }
}

pub struct StageRouter {
    efficient_model: String,
    capable_model: String,
    confidence_threshold: f64,
    signal_processor: ToolSignalProcessor,
    classifier: StageClassifier,
}
```

**收益**：
- SubAgent-Work 执行后，根据工具调用结果信号决定下一步
- 无需额外 LLM 调用即可升级到 capable_model

#### P2-3: Advisor Gate 审查者模式

**实现**：
```rust
// src/agent/advisor_gate.rs
pub struct AdvisorGate {
    executor: Arc<dyn Agent>,           // 执行者（SubAgent-Work）
    advisor: Arc<dyn Agent>,            // 审查者（QC）
    config: AdvisorGateConfig,
    state: Mutex<GateState>,
}

pub struct AdvisorGateConfig {
    pub max_reviews: u32,              // 每个 scope 最多审查次数
    pub fail_open: bool,               // 审查失败时默认通过
    pub pattern: Option<String>,       // 触发审查的 pattern
}

impl AdvisorGate {
    pub async fn execute(&self, request: AgentRequest) -> Result<AgentOutcome> {
        // 1. Executor 回答每个轮次
        // 2. 终端轮次被缓冲
        // 3. Advisor 审查：APPROVE 释放 / REDO 反馈
        // 4. 每个 scope 最多 max_reviews 次
        // 5. Advisor 故障时 fail_open
    }
}
```

**收益**：
- 与 laew 的 Quality-Check Agent 功能相似，但更精细化
- 预算作用域、stall 检测、自动 fail_open

#### P2-4: 可选本地代理模式

**实现**：
```rust
// src/bin/laew-serve.rs
use axum::{routing::post, Router};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/v1/chat/completions", post(openai_chat_completions))
        .route("/v1/messages", post(anthropic_messages));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

**收益**：
- laew 可作为其他工具的后端代理
- 复用 6 角色 Agent 架构 + 多协议支持

#### P2-5: W3C Trace Context 传播

**实现**：
```rust
// src/llm/tracing.rs
use opentelemetry::global;
use opentelemetry::propagation::Extractor;

pub fn extract_trace_context(headers: &HeaderMap) -> opentelemetry::Context {
    global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(headers))
    })
}

pub fn inject_trace_context(headers: &mut HeaderMap, cx: &opentelemetry::Context) {
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(cx, &mut HeaderInjector(headers))
    });
}

struct HeaderExtractor<'a>(&'a HeaderMap);
impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }
    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|n| n.as_str()).collect()
    }
}
```

**收益**：
- 兼容 OpenTelemetry Collector + Jaeger / Zipkin 等可观测性平台
- 跨服务追踪 laew 调用链

### 11.4 路线图汇总

| 阶段 | 能力 | 实现成本 | 收益 |
|------|------|---------|------|
| **P0-1** | 引入 `protocol` 子模块 + IR | 高 | 协议差异封闭，无损保留 |
| **P0-2** | ContentBlock::Unknown | 低 | 新字段不丢失 |
| **P0-3** | PreservationMetadata | 中 | 同格式重放 |
| **P0-4** | 指数退避重试 | 中 | 网络抖动容忍 |
| **P0-5** | 客户端 LRU 缓存 | 低 | 资源复用 |
| **P0-6** | 多 Agent 协同头 | 低 | 可观测性 |
| **P1-1** | FormatCodec trait + Registry | 高 | 协议插件化 |
| **P1-2** | 流式事件规范化 | 中 | 流式统一 |
| **P1-3** | 400 错误分类 | 中 | 避免无效重试 |
| **P1-4** | Yolo p_solve 输出 | 中 | 分类置信度 |
| **P1-5** | AffinityRouter 粘性路由 | 中 | cache 命中 |
| **P2-1** | Algorithm trait + Step 流 | 高 | 并发 SubAgent |
| **P2-2** | Stage Router 信号驱动 | 中 | 路由无需 LLM |
| **P2-3** | Advisor Gate 审查者 | 中 | 精细化 QC |
| **P2-4** | 可选本地代理模式 | 中 | 外部工具复用 |
| **P2-5** | W3C Trace Context 传播 | 中 | 跨服务追踪 |

---

## 12. 总结与设计模式提炼

### 12.1 五大核心模式

#### 模式 1：协议 IR + Codec Registry

**来源**：Switchyard

**核心思想**：
- 定义 Provider-neutral 的 `LlmRequest`/`LlmResponse` 中间表示
- 每个供应商实现 `FormatCodec` trait
- 翻译 = `decode(source) → encode(target)`

**优势**：
- Agent 循环与协议完全解耦
- 新增协议只需实现 Codec
- `ContentBlock::Unknown` 保证无损

**laew 借鉴**：P0-1 / P0-2 / P1-1

#### 模式 2：TranslationEngine + Policy

**来源**：Switchyard

**核心思想**：
- 集中的 `TranslationEngine` 协调多个 Codec
- `TranslationPolicy` 控制翻译行为（保留/确定性 ID/流式保留）
- 一对多映射（一个 source event → 多个 target events）

**优势**：
- 翻译策略可配置
- 翻译状态集中管理

**laew 借鉴**：P1-1（FormatCodec 已隐含此模式）

#### 模式 3：Algorithm trait + FallThrough 级联

**来源**：Switchyard

**核心思想**：
- 每个路由算法实现 `Algorithm` trait
- `FallThrough<S>` 顺序尝试多个 Classifier，第一个给出 Scores 的胜出
- `CallModel` Step 流支持异步并发

**优势**：
- 算法可组合
- 支持信号驱动 + LLM 判断混合路由

**laew 借鉴**：P2-1 / P2-2

#### 模式 4：双 reqwest Client + 指数退避

**来源**：Switchyard

**核心思想**：
- 一个普通 client（允许重定向）
- 一个 `forward_auth` client（禁用重定向，防止凭证泄露）
- 指数退避：250ms → 500ms → 1000ms → 2000ms（封顶）
- 尊重 `Retry-After` 头，但不超过 60s

**优势**：
- 凭证安全
- 网络抖动容忍
- 区分上下文溢出 vs 临时错误

**laew 借鉴**：P0-4 / P1-3

#### 模式 5：客户端 LRU 缓存 + Provider 字典

**来源**：agent-studio

**核心思想**：
- `@lru_cache(maxsize=32)` 缓存客户端实例
- `provider → ClientFactory` 字典分发
- API Key 加密存储

**优势**：
- 资源复用（HTTP 连接池）
- 多租户隔离
- 配置热切换

**laew 借鉴**：P0-5

### 12.2 反模式警示

**反模式 1：硬编码协议分支（laew 当前）**
- 现状：`match protocol { Anthropic => ..., OpenAI => ... }` 散落在 agent 循环
- 风险：每加一个协议都要改多处
- 改进：Codec Registry + IR

**反模式 2：无错误分类**
- 现状：所有错误统一返回 `Err`
- 风险：上下文溢出也被重试，浪费 token
- 改进：400 错误分类（P1-3）

**反模式 3：无重试无超时**
- 现状：单次请求失败立即返回
- 风险：临时网络错误导致整个任务失败
- 改进：指数退避 + 分类重试（P0-4）

**反模式 4：无缓存无粘性**
- 现状：每次都构造 reqwest client，无 session-model 绑定
- 风险：prompt cache 命中率低
- 改进：LRU + AffinityRouter（P0-5 / P1-5）

### 12.3 关键借鉴路径总结

```
laew 现状                  Switchyard / agent-studio 借鉴           收益
─────────────────────────────────────────────────────────────────────────────
直接 wire 操作     →    protocol IR + ContentBlock::Unknown       协议差异封闭
无重试             →    指数退避 + Retry-After + 400 分类          网络抖动容忍
无缓存             →    LRU 缓存客户端 + AffinityRouter            资源/cache 命中
无路由             →    Algorithm trait + FallThrough 级联         多 Provider 分流
硬编码两协议       →    FormatCodec trait + FormatRegistry         协议插件化
直连 LLM           →    可选 axum 本地代理 + 优雅关闭               外部工具复用
```

### 12.4 三阶段演进路线

**阶段 1（P0，核心）**：协议 IR + 重试 + 缓存 + 多 Agent 协同头
- 实施周期：1-2 周
- 收益：基础稳健性 + 资源复用 + 可观测性

**阶段 2（P1，增强）**：FormatCodec + 流式规范化 + Yolo p_solve + AffinityRouter
- 实施周期：2-4 周
- 收益：协议插件化 + 流式统一 + 路由智能化

**阶段 3（P2，高级）**：Algorithm trait + Step 流 + 信号驱动 + Advisor Gate + 可选代理 + Trace Context
- 实施周期：4-8 周
- 收益：并发 SubAgent + 精细化 QC + 外部工具复用 + 跨服务追踪

### 12.5 对 laew 整体架构的影响

引入 Switchyard 的协议 IR + 算法抽象后，laew 架构将演化为：

```
┌─────────────────────────────────────────────────────────────────┐
│  TUI / -p 单轮 / -f 文件                                          │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  MultiAgentOrchestrator（基于 Algorithm trait）                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐           │
│  │ Yolo     │ │ Plan     │ │ Main-Work│ │ SubAgent │           │
│  │ (Capability)│ │(Escalation)│ │(Passthrough)│ │(Noop/Passthrough)│
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘           │
│  ┌──────────┐ ┌──────────┐                                       │
│  │ QC       │ │ Session  │                                       │
│  │(Advisor) │ │(Noop)    │                                       │
│  └──────────┘ └──────────┘                                       │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  protocol::LlmRequest IR + ContentBlock                          │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  FormatCodec Registry                                            │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐             │
│  │ Anthropic    │ │ OpenAI       │ │ Future:      │             │
│  │ Codec        │ │ Codec        │ │ Gemini/Cohere│             │
│  └──────────────┘ └──────────────┘ └──────────────┘             │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  LlmClient（LruCache + 重试退避 + 错误分类 + 多 Agent 协同头）    │
└─────────────────────────────────────────────────────────────────┘
```

这一架构将 laew 从「双协议 CLI 工具」升级为「多协议多 Agent 编排器」，与 Switchyard、agent-studio、opencode 同台竞技。

---

## 附录：核心代码路径与文件位置速查

### Switchyard

| 模块 | 路径 | 行数 | 关键设计 |
|------|------|------|----------|
| IR | `crates/protocol/src/llm.rs` | ~600 | `LlmRequest` / `ContentBlock::Unknown` |
| 流式 | `crates/protocol/src/stream.rs` | ~400 | `LlmResponseChunk` / `ResponseAccumulator` |
| 元数据 | `crates/protocol/src/metadata.rs` | ~300 | `HEADER_CONFIG` 优先级链 |
| Codec | `crates/translation/src/codec.rs` | ~1500 | 3 个内建 Codec |
| 引擎 | `crates/translation/src/engine.rs` | ~500 | `TranslationEngine` |
| 算法 | `crates/libsy/src/algorithms/` | ~3000 | 7+ 算法 |
| 驱动 | `crates/libsy/src/core/drive.rs` | ~100 | `drive` 并发 |
| 客户端 | `crates/libsy-llm-client/src/client.rs` | ~800 | `TranslatingLlmClient` + 重试 |
| 服务器 | `crates/switchyard-server/src/` | ~2500 | Axum + 优雅关闭 + 指标 |

### agent-studio

| 模块 | 路径 | 关键设计 |
|------|------|----------|
| LLM 管理 | `ops/modules/llm/llm_manager.py` | `_CLIENT_PROVIDER_MAP` + LRU 缓存 |
| 模型配置 | `models/model_config.py` | `ModelConfig` + 加密存储 |
| FastAPI 后端 | `backend/openjiuwen_studio/main.py` | `lifespan_func` 生命周期 |
| 评估 | `core/executor/evaluation/` | LLM-as-Judge + 扰动矩阵 |

### cc-switch

| 模块 | 路径 | 关键设计 |
|------|------|----------|
| Provider | `src-tauri/src/provider.rs` | 30+ Provider 配置 |
| 代理 | `src-tauri/src/proxy.rs` | axum/hyper 代理 |
| 熔断器 | `src-tauri/src/circuit_breaker.rs` | per-Provider 熔断 |

### laew 当前实现

| 模块 | 路径 | 关键设计 |
|------|------|----------|
| LLM 抽象 | `src/llm/mod.rs` | `LlmClient` trait + `build_common_headers` |
| Anthropic | `src/llm/anthropic.rs` | `AnthropicClient` + wire 转换 |
| OpenAI | `src/llm/openai.rs` | `OpenAIClient` + wire 转换 |
| SSE | `src/llm/sse.rs` | SSE 字节流解析 |
| 错误 | `src/error.rs` | `AgentError` 含 `YoloParse` |
| Agent 循环 | `src/agent/mod.rs` | 协议无关循环 |
| Yolo | `src/agent/yolo.rs` | `YoloRunner` + `TaskClassification` |

---

> **结语**：Switchyard 是 NVIDIA 在 LLM 流量代理领域的 Rust 实践标杆，agent-studio 是企业级 Agent 平台的多 LLM 接入典范，cc-switch 是 Tauri 本地代理的代表。laew 作为 LLM Agent CLI，可借鉴三者的协议 IR 设计、缓存策略、本地代理架构，按 P0→P1→P2 顺序演进，从「双协议 CLI」升级为「多协议多 Agent 编排器」，与 Switchyard、agent-studio、opencode 同台竞技。
>
> **建议优先级**：协议 IR（P0-1）> 重试退避（P0-4）> 客户端缓存（P0-5）> 多 Agent 协同头（P0-6）> FormatCodec（P1-1）> Yolo p_solve（P1-4）> AffinityRouter（P1-5）> Algorithm trait（P2-1）。
