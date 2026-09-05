# atomcode 第二轮深度分析(Rust Workspace + Kernel + MCP + Skill + CodeIntel)

> 分析日期:2026-09-05
> 源码根路径:`/usr/local/LsmGitOpenSource/atomcode/` (workspace, version = 5.0.9)
> 分析方法:在已有三档文档(`atomcode-源码调研.md`、`atomcode-深度分析.md`、`atomcode-核心机制深度分析.md`)"在已有基础上深化",逐文件 / 逐行号钻取 5 个二轮深挖点
> 目标调用链:CLI / TUI / daemon → `CodingRuntime` → `CodingParts::assemble` → kernel `Agent` 的 `RunningAgent::session_loop` → `run_turn` → `Tool` / `LlmProvider` / `LifecycleHooks` / `ToolMiddleware`
> 重点:① Rust workspace 三层 + cargo feature gating 的依赖方向契约;② atomcode-kernel 内部 trait 签名 + AgentCommand/AgentEvent 双向消息协议;③ MCP 7 个子模块的协同(stdio / HTTP / OAuth / Trust);④ Skill 一等公民的加载 / 触发 / 上下文注入;⑤ CodeIntel 的 Symbol Graph + 依赖 + BLAST RADIUS。

---

## 1. Rust Workspace 分层架构实战

### 1.1 顶层 workspace 契约(`Cargo.toml`)

`/usr/local/LsmGitOpenSource/atomcode/Cargo.toml` 显式声明分层 + 闭源 overlay 兼容策略:

```toml
# /usr/local/LsmGitOpenSource/atomcode/Cargo.toml (line 1-19)
[workspace]
resolver = "2"
# Glob picks up the closed-source `atomcode-codingplan-crypto` overlay
# the official build pipeline drops into `crates/` (see the stub at
# `crates/atomcode-codingplan-crypto/`). Adding any future crate is also
# a no-toml-edit operation.
members = ["crates/*"]
```

- **`members = ["crates/*"]`**:cargo 自动收录 `crates/` 下所有 crate,新增 crate 无需改 toml;`default-members` 只列 4 个(cli / daemon / telemetry / tuix),编码特化层 `atomcode-coding` 不在 default(让纯 toolchain 测试可跳过它)
- **release profile** 极致小体积:`opt-level = "z"` + `lto` + `codegen-units = 1` + `strip` + `panic = "abort"`(注释明示"saves ~200-500KB")
- **`#[non_exhaustive]` + `#[serde(default)]`** 是协议兼容性的两大武器(详见 §2.3)

### 1.2 三层依赖方向(atomcode-capabilities 的 Cargo.toml 注释钉死边界)

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/Cargo.toml` 注释里写明:

```toml
# L1 capabilities layered on atomcode-kernel (L0): real provider adapters, tools, mcp, skills. Depends ONLY on the kernel.
[dependencies]
# L0 only. NEVER add atomcode-core or any L2/L3 crate here (layering is compile-enforced).
atomcode-kernel = { path = "../atomcode-kernel" }
atomcode-auth = { path = "../atomcode-auth", optional = true }
```

`atomcode-coding/Cargo.toml` 是 L2 入口(line 17-33):

```toml
# The FULL capability set: L2 is the opinionated assembly — a coding agent ships
# with web/skills/mcp/session/memory wired (the leaner features exist for other
# embedders, not for this crate).
atomcode-capabilities = { path = "../atomcode-capabilities", features = [
    "provider", "tools", "web", "codeintel", "lsp", "skills", "mcp", "session", "memory",
    "cc-hooks", "offline",
] }
```

**为什么依赖方向是契约**:
- `cargo tree -p atomcode-capabilities` 永远不能出现 `atomcode-coding` —— 编译器直接卡死
- `atomcode-kernel` 是中性 SDK(`lib.rs:1-3` "neutral, embeddable agent"),不知道"编码 / 审查 / MCP"
- **L2 composition**:`atomcode-coding/src/parts.rs:445-458` 把 L1 全部能力 + `atomcode-review`(独立 L2 sibling)作为 `Code_review` 工具挂进主 agent —— "L2 composing a sibling L2"

### 1.3 Feature gating 边界(runtime 可编译 vs 不可编译)

`atomcode-capabilities/Cargo.toml` 的 `[features]` 一节(共 16 个 feature)是真正的"能力合约":

| Feature | 依赖 / 影响 | L2 默认 |
|---------|------------|---------|
| `provider` | `reqwest` (rustls-tls) + `hyper` + `atomcode-auth` + `atomcode-config` | ✅ |
| `tools` | `ignore` + `regex` + `grep` + `globset` + `tree-sitter-bash` + `encoding_rs` + `chardetng` + `similar` | ✅ |
| `web` | + `reqwest` + `url`(web_fetch / web_search) | ✅ |
| `codeintel` | tree-sitter 12 个 grammar crate | ✅ |
| `lsp` | + `which` + `url`(rust-analyzer 等 spawn) | ✅ |
| `skills` | 无新 dep(仅 markdown 解析) | ✅ |
| `mcp` | `anyhow` + `sha2` + `uuid` + `toml` + `reqwest/blocking`(OAuth) | ✅ |
| `session` | `chrono` + `fs2` | ✅ |
| `memory` | `dirs` | ✅ |
| `cc-hooks` | 子进程 spawn `hooks.json` | ✅ |
| `offline` | web 失败 → 离线模式自动 flip | ✅ |
| `setup` | 一次性 setup 包(独立 binary) | ❌ |
| `e2e` | 真实 provider 网络测试 | ❌ |
| `lsp-e2e` | 真实 rust-analyzer 等 | ❌ |
| `plugin` | marketplace loader | ❌ |

**关键设计点(注释里写明 why)**:

```toml
# Code-intelligence tools (tree-sitter symbol extraction): list_symbols / read_symbol.
# Opt-in (NOT default) — pulls 12 tree-sitter grammar crates (heavy C compilation).
codeintel = [
    "dep:tree-sitter", "dep:tree-sitter-rust", "dep:tree-sitter-python", ...
],
# `web` capability deps ... NOT in default: a tools-only embedder that wants no
# network egress skips it.
web = ["tools", "dep:reqwest", "dep:url", "tokio/net"],
```

**含义**:consumer 只为"用到的能力"付出编译代价。一个 `tools`-only embedder(不带 MCP / codeintel)会显著更小、build 更快。

### 1.4 Cargo.toml 中对 Rust TLS / native cert 的精细处理(实战坑)

`atomcode-capabilities/Cargo.toml` 里有一段是真正的工业级细节(`#514` issue):

```toml
# `rustls-tls` (webpki base) — an INFALLIBLE base so `.build()` never hard-fails
# on certs. OS native roots + SSL_CERT_FILE are layered on TOP, gracefully, in
# `build_http_client` (`add_trusted_roots`). NOT `rustls-tls-native-roots`:
# reqwest reads native roots / SSL_CERT_FILE STRICTLY there and fails the build
# ("zero valid certificates …") on a bad SSL_CERT_FILE or empty store, which
# cascades into panics at every `.build()` site.
reqwest = { version = "0.12", features = ["stream", "json", "rustls-tls"], default-features = false, optional = true }
# OS native root store loader for corporate MITM-proxy CAs. Best-effort. #514.
rustls-native-certs = { version = "0.8", optional = true }
```

**WHY**:reqwest 的 `rustls-tls-native-roots` 是 STRICT mode —— 用户的 `SSL_CERT_FILE` 一旦脏掉,所有 `.build()` site 全 panic。atomcode 选择 webpki base + 显式 layer 上去,best-effort 处理 MITM CA。

**Windows 平台分支**:

```toml
[target.'cfg(target_os = "windows")'.dependencies]
# Windows SChannel backend for reqwest (dodges rustls-fingerprint RST on *.atomgit.com).
reqwest = { version = "0.12", features = ["native-tls"], default-features = false, optional = true }
```

Windows 用 SChannel 绕过 rustls-fingerprint 检测(在 `*.atomgit.com` 域上 rustls 会被 RST)。

### 1.5 数据流走向(prepare → assemble → Agent)

```
CodingAgentConfig (L2 配置)
    ↓
prepare_with_plugin_hooks_reusing_lease (parts.rs:452-...)
    ├─ Async I/O:
    │  ├─ load_mcp_config + registry.from_config + add_server (BGP: connect stdio/HTTP)
    │  ├─ load skill dirs (home + project)
    │  ├─ session.bind (uuid v4 分配 / resume 重载)
    │  └─ register tools: register_coding_tools_with_vision + register_codeintel_tools + register_lsp_tool
    └─ 同步组合(无 I/O):
       ├─ register_skill_tools (UseSkillTool + ListSkillsTool)
       ├─ registry.mount(names)
       └─ 构造 CodingParts(含 Arc<ApprovalMiddleware>, Arc<HookChain>, Arc<SkillRegistry> ...)
    ↓
assemble (parts.rs:1507-...)
    ├─ 纯组合(无 I/O):
    ├─ 注入 Arc<dyn LlmProvider> (含 review_provider, subagent_provider)
    ├─ 注入 Arc<dyn CompactionStrategy> (Stub + Overflow Ladder)
    ├─ 注册 middlewares (parts.rs:1700-1800): 严格顺序
    │  ├─ TurnExecutionPolicy → PlanModeGate → PlanModeReminderHook
    │  ├─ CredentialBashGate → SensitivePathGate → [atomgit]AtomgitBashGate
    │  ├─ CCExternalHooks (PreToolUse gate)
    │  ├─ OpenFileWorkspaceGate → WriteApprovalGate → BashWorkspaceGate
    │  ├─ ApprovalMiddleware (通用)
    │  └─ DatalogHook (lifecycle + middleware 同时挂)
    ├─ 注册 lifecycle hooks (parts.rs:1837-1858): 严格顺序
    │  ├─ SkillCatalogHook → MemoryHook → SessionContextHook
    │  ├─ McpInstructionsHook → RateLimitHook → CCExternalHooks
    │  ├─ VerifyCadenceHook → TodoHook
    │  └─ TodoEagerHook (model 决定) → DatalogHook
    └─ AgentBuilder.build() → Arc<Agent> / AgentHandle
```

**关键**:中间件 / Hook 注册顺序是 LOAD-BEARING(parts.rs:1704-1705 "Register before every middleware that can `Allow` and bypass downstream approval gates";parts.rs:1750-1759 CC PreToolUse 必须 `BEFORE` WriteApprovalGate,因为 WriteApprovalGate 会用 `Allow` 短路后续链路,让 PreToolUse 看不见)。

### 1.6 借鉴要点(分层 + feature gating)

**P0**:laew 当前是扁平模块(agent / tui / llm / config 等),应该学习 atomcode 的三层 crate 物理隔离:
- 拆 `laew-core`(中性 SDK:会话循环 + 消息 + provider trait + tool trait + hook + middleware + 中性事件协议)
- `laew-tools`(provider / tools / mcp / skills / memory / session / codeintel + feature gating)
- `laew-app`(Yolo/Plan/Main/Sub/QC/SessionContext 6 角色 + 装配)
- 物理依赖方向由 cargo 强制,cargo tree 单向校验

**P1**:Cargo `[features]` 按能力切:`provider` / `tools` / `mcp` / `skill` / `codeintel` / `lsp` / `session` / `memory` —— 嵌入式用 `tools + memory + provider`,重型 IDE 用全套

**P1**:`release = opt-level "z" + lto + codegen-units=1 + strip + panic="abort"` 二进制体积优化套路直接照搬(laew 当前 release profile 没这条设置)

---

## 2. atomcode-kernel 内部细节

### 2.1 模块边界(kernel/src/lib.rs)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/lib.rs (line 1-29)
//! atomcode-kernel (spike) — a domain-neutral agent driven by a bidirectional,
//! serializable Command/Event handle.
//!
//! Phase A0: internals are minimal/throwaway; the public API *shape* is what
//! Phase A1 carries the proven hot-path code into. The kernel knows nothing
//! about approval, persona, or code-intelligence.

pub mod agent;
pub mod checkpoint;
pub mod clock;
pub mod conformance;
pub mod event;
pub use event::{OUTPUT_TRUNCATION_CHECKPOINT_KIND, ROUND_CAP_CHECKPOINT_KIND};
pub mod hook;
pub mod message;
pub mod middleware;
pub mod provider;
pub mod request;
pub mod stream;
pub mod testkit;
pub mod tool;
```

**模块职责矩阵**(按行数排序):

| 模块 | 行数 | 职责 |
|------|------|------|
| `agent.rs` | 5966 | Agent 主循环 + retry ladder + 失败模式策略 |
| `message.rs` | 2064 | Message / Conversation / CompactionPlan / sacred_floor |
| `testkit.rs` | 1571 | MockProvider / scripted StreamEvents / ApprovalMiddleware |
| `hook.rs` | 638 | LifecycleHooks + HookChain + TurnCtx + RateLimitHint/Decision |
| `tool.rs` | 604 | Tool trait + ToolDef + ToolRegistry + MountedTools / Publisher |
| `event.rs` | 531 | AgentCommand + AgentEvent + StopReason + PolicyIntervention |
| `provider.rs` | 269 | LlmProvider trait + ChatOptions + ReasoningEffort + ToolChoice |
| `stream.rs` | 280 | StreamEvent + TokenUsage::merge_max + ProviderError |
| `middleware.rs` | 158 | ToolMiddleware + BeforeOutcome + AfterOutcome |
| `request.rs` | 137 | RequestCtx / Requester (kernel/driver 双向握手) |
| `conformance/` | - | 一致性测试(被 testkit 调用) |
| `checkpoint.rs` | 43 | CompactionCheckpoint trait (持久化快照) |
| `clock.rs` | 76 | 可注入的时钟(test-only 虚拟时钟) |

### 2.2 协议中立性:`LlmProvider` trait(provider.rs:120-157)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/provider.rs (line 120-157)
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn model_name(&self) -> &str;
    /// Effective context window in tokens. 0 = unknown.
    fn context_window(&self) -> u32 { 0 }
    /// Bind this provider to its owning Agent's session id, ONCE. The kernel calls
    /// this at spawn — the single point where the session id meets the provider —
    /// so no driver re-threads it. An adapter forwards it as the
    /// `x-atomcode-session-id` header, letting a forwarding gateway (LiteLLM) pin the
    /// whole conversation to one upstream for prefix-cache affinity.
    fn bind_session_id(&self, _session_id: &str) {}
    /// Open the stream for one turn. `Err` = a failed OPEN (auth/connect/etc.);
    /// the stream itself may then still fail mid-flight via `StreamEvent::Error`.
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        options: &ChatOptions,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError>;
}
```

**关键设计**:
- 唯一契约是 `chat_stream` —— 流式为唯一协议(line 9-19 注释明示 `SLOT, not POLICY` —— kernel 是 mechanism,adapter 负责 policy)
- `bind_session_id` 是一次性绑定(`OnceLock` 实现)—— kernel 在 spawn 时单点注入,驱动层无需手传 session id
- `ChatOptions` 中立携带 `reasoning_effort` / `tool_choice` / `temperature` / `max_tokens` —— adapter 自己映射到 OpenAI / Anthropic / Ollama 的 wire 格式
- `RateLimitRetryOwner` 标注运行时谁拥有 429 重试(`#[serde(skip)]` 不持久化)—— 直接 provider 调用者保留默认;kernel turn loop override,让 wait 可被 cancel

### 2.3 `AgentCommand` / `AgentEvent` 双向消息协议(event.rs)

这是 kernel **对外协议的中立骨架**,从 TUI / daemon / ACP / WebUI 都可以套用同一份 wire format。

**`AgentCommand`(driver → agent, 124-171)**:
```rust
#[non_exhaustive]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentCommand {
    SendMessage { text: String, #[serde(default)] images: Vec<ImageContent> },
    SendMessageWithContext { text: String, #[serde(default)] images, context: String },
    SendSyntheticMessage { text: String },
    Respond { id: RequestId, value: serde_json::Value },
    Snapshot,
    Compact { focus: Option<String> },
    Cancel,
    Shutdown,
}
```

**`AgentEvent`(agent → driver, 188-385)` 22 个 variant —— 涵盖所有 driver 需要的信号(TextDelta / ToolStarted / ToolResult / Request / ResponseId / Usage / Snapshot / TurnComplete / Error / Cancelled / Reasoning / Warning / StreamRecovery / ProviderRetry / OutputTruncationRecovery / RateLimited / Steered / CompactionStarted / Compacted / CompactionFailed 等)。

**协议兼容性三大武器**(注释里反复强调):
1. `#[non_exhaustive]` —— 新 variant 不破下游 `match`
2. `#[serde(default)]` —— 老 wire 缺字段仍可反序列化(`SendMessage {text}` → 无 images)
3. `#[serde(default, skip_serializing_if = "Option::is_none")]` —— 可选字段不强制序列化

**WHY 这些设计(StopReason 注释 line 76-119)**:
```rust
/// WHY a turn ended (FAILURE PERCEPTION). Carried by the terminal
/// `AgentEvent::TurnComplete { reason }` and aggregated into `Outcome::stop`, so a
/// driver (TUI / SWE-bench grader / CI) can ALWAYS tell a clean stop from a
/// failure — a failed turn can never look like an empty SUCCESS.
```

`Outcome::default()` 是 `Stopped`(line 90 `#[default] Stopped`),保证 Default 编译过;`Outcome.error` 是 LAST Error,"a failed open/mid-stream/timeout/fuse yields `Outcome { stop: ProviderError, error: Some(..) }`, not an empty `Outcome::default()` masquerading as success"。

### 2.4 `LifecycleHooks` + `HookChain` 旋钮(hook.rs:183-497)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/hook.rs (line 183-322)
#[async_trait]
pub trait LifecycleHooks: Send + Sync {
    async fn session_start(&self, _convo: &mut Conversation, _resumed: bool) {}
    async fn user_prompt_submit(&self, _text: &mut String) -> Result<(), String> { Ok(()) }
    async fn turn_start(&self, _convo: &mut Conversation) {}
    async fn pre_request(&self, _messages: &mut Vec<Message>, _ctx: &TurnCtx) {}
    async fn pre_request_options(&self, _messages: &[Message], _options: &mut ChatOptions, _ctx: &TurnCtx) {}
    async fn on_request(&self, _messages: &[Message], _tools: &[ToolDef], _options: &ChatOptions, _ctx: &TurnCtx) {}
    async fn on_text_delta(&self, _delta: &mut String) {}
    async fn on_reasoning_delta(&self, _delta: &mut String) {}
    async fn on_model_response(&self, _response: &mut Message) {}
    async fn offer_continuation(&self, _convo: &Conversation) -> Option<String> { None }
    async fn offer_typed_continuation(&self, convo: &Conversation) -> Option<Continuation> { ... }
    async fn turn_complete(&self, _convo: &Conversation, _reason: &StopReason, _ctx: &TurnCtx) {}
    async fn on_error(&self, _error: &str) {}
    async fn on_rate_limit(&self, _hint: &RateLimitHint) -> Option<RateLimitDecision> { None }
    async fn session_end(&self, _convo: &Conversation) {}
}
```

**永久 vs ephemeral 区分**(注释 line 207-247 强调):
- **`session_start` / `turn_start` / `on_model_response`** —— PERMANENT(写入历史)
- **`pre_request` / `on_text_delta` / `on_reasoning_delta`** —— EPHEMERAL(操作克隆上,不污染 prefix cache)
- **`on_request`** —— 只读观察(`&` 不能改 wire)

**`HookChain` 组合契约**(hook.rs:336-365 注释里逐 method 钉死):
- `session_start` / `turn_start`:全部按注册顺序跑(后续 hook 看见前面 hook 的改动)
- `user_prompt_submit`:第一个 `Err` 短路阻断
- `offer_continuation`:**first Some wins**,后续 Some 忽略(loop 一次只注入一条续接)
- `turn_complete` / `on_error` / `session_end`:全部按顺序观察

**WHY 这些约定(line 7-15 模块头)**:
```rust
/// Composes MANY `LifecycleHooks` into one by FANNING OUT each method over an
/// ordered list. This is the seam that lets independent capabilities coexist —
/// codeintel + compaction + redaction can each register a hook instead of fighting
/// over a single slot. A `HookChain` itself implements `LifecycleHooks`, so the
/// Agent still holds exactly one `Arc<dyn LifecycleHooks>` and every call site in
/// the run loop stays unchanged.
```

独立能力(codeintel / compaction / 脱敏)每个注册自己的 hook,零冲突。

**`TurnCtx` 关联 ID**(hook.rs:11-47):
- `session_id`:driver 注入(kernel 不 mint)
- `turn_id`:kernel 单调递增(确定性,不是 clock/random)—— 1-based within session
- `request_id`:kernel 全局单调 —— 1-based across session
- `round`:当前 turn 内的轮次(每 turn reset)
- `max_rounds` / `cache_epoch` / `context_window` / `used_tokens`

**WHY deterministic**:注释 line 14-15 钉死 —— "deterministic — NOT clock/random — so log stitching stays reproducible"。

### 2.5 `ToolMiddleware` + `BeforeOutcome` / `AfterOutcome`(middleware.rs)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/middleware.rs (line 34-103)
#[async_trait]
pub trait ToolMiddleware: Send + Sync {
    /// EPHEMERAL rewrite + approval gate; BEFORE chain runs in registration order.
    /// The first middleware to return `Deny` blocks; `Allow` short-circuits remaining
    /// before gates.
    async fn before(&self, _call: &mut ToolCall, _tool: &Arc<dyn Tool>, _rt: &RequestCtx)
        -> BeforeOutcome { BeforeOutcome::Proceed }

    /// RESULT transform + continuation gate; AFTER chain runs in registration order.
    async fn after(&self, _result: &mut ToolResult, _tool: Option<&Arc<dyn Tool>>)
        -> AfterOutcome { AfterOutcome::Proceed }
}

pub enum BeforeOutcome {
    #[default] Proceed,
    Allow { reason: Option<String> },
    Ask { reason: Option<String> },
    Deny { reason: String },
    DenyTurn { reason: String },
    DenyTurnWithIntervention { reason: String, intervention: PolicyIntervention },
}
```

**为什么 BEFORE 链注册顺序是契约**(注释 line 8-23 反复强调):

> "ORDERING IS LOAD-BEARING. Middlewares run in REGISTRATION ORDER: the `before` chain forward (the first-registered runs first; the first to `Err` blocks and stops the chain), then the `after` chain (also in registration order). This is a documented contract, not an accident of iteration. **Any normalization or repair that changes the arguments which will execute MUST run before every observer, policy gate, and user approval, so all of them inspect the exact bytes the tool receives**."

**`Panic Contract`(line 25-33)**:kernel 不隔离 panic(`panic = "abort"` 下 `catch_unwind` 没用),所以 middleware 必须不 panic —— 与 tool sandbox contract 同级 —— 阻断必须 return `BeforeOutcome::Deny`。

### 2.6 `Tool` trait + 双重注册(tool.rs:206-328)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/tool.rs (line 206-272)
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    /// Risk classification for THIS call — arg-aware (e.g. bash: `rm -rf` Risky vs `ls` Safe).
    fn risk(&self, _args: &str) -> RiskLevel { RiskLevel::Safe }
    /// Known read-only (intrinsic); MCP sets from `annotations.readOnlyHint`.
    fn read_only_hint(&self) -> bool { false }
    /// Output is self-bounded; skip generic truncation (e.g. read_file).
    fn self_bounds_output(&self) -> bool { false }
    /// Whether THIS call may run concurrently with other tools (parallel-safe).
    fn parallel_safe(&self, _args: &str) -> bool { self.read_only_hint() }
    /// "Always" approval scope (empty for tool-wide like mcp__).
    fn always_grant_scope(&self, args: &str) -> String { args.to_string() }
    /// Lift a policy intervention discovered behind this tool's execution boundary.
    fn take_policy_intervention(&self, _result: &mut ToolResult) -> Option<PolicyIntervention> { None }
    async fn execute(&self, args: &str, ctx: &ToolContext) -> ToolResult;
}
```

**`ToolRegistry` + `MountedTools` 二阶段**(tool.rs:277-348):
- `register(Arc<dyn Tool>)`:放入全集(`BTreeMap<name, Arc<dyn Tool>>`,BTreeMap 保证 deterministic prompt ordering)
- `mount(names)` / `mount_updatable(names)`:选择子集暴露给 LLM
- `MountedTools { current: Arc<RwLock<Arc<MountedToolsSnapshot>>> }`:write-on-copy 快照
- `MountedToolsPublisher`:runtime 独占写方,原子发布

**WHY 这种二阶段**:注释 line 274-277 钉死 —— "Holds *all* available tools. Clones share the same registry so a runtime-owned background capability reconciler can register tools before publishing a new mounted snapshot" —— MCP 在后台初始化完后原子 publish 新快照,agent 下一轮拿到的工具集自动更新。

### 2.7 StreamEvent 流事件(stream.rs:90-156)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/stream.rs (line 90-156)
pub enum StreamEvent {
    TextDelta(String),
    Reasoning(String),
    ReasoningSignature { opaque: String, provider: String },  // Anthropic / OpenAI encrypted_content / Gemini thoughtSignature
    ToolCall(ToolCall),
    ToolCallDelta { index: u32, id: Option<String>, name: Option<String>, arguments: String },
    Usage(TokenUsage),
    ResponseId(String),
    ResponseModel(String),
    Error(ProviderError),
    Malformed,
    Done { truncated: bool },
}
```

**`TokenUsage::merge_max` 字段级 max**(注释 line 19-23):
```rust
/// Usage 合并: Anthropic 风格(split report)vs OpenAI 风格(单次 cumulative)的差异
/// 字段级 MAX,不会 double-count cumulative delta
pub fn merge_max(&mut self, other: TokenUsage) { ... }
```

**`on_text_delta` / `on_reasoning_delta` 是 transform seam**:每 chunk 原地变换(agent.rs:2542-2594),保证流式 + 存储一致(脱敏同时进 live stream + storage,关闭 reasoning 通道泄露)。

### 2.8 借鉴要点(kernel 模块化)

**P0**:laew 当前 `src/agent/mod.rs`(140 行 `run_session`)缺独立 trait 抽象 —— 应该把 `LlmClient` 升级为中立 trait(`chat_stream` 返回流),把 `Message` / `Conversation` / `CompactionPlan` / `sacred_floor` 这套中性模型迁出;引入 `AgentCommand` / `AgentEvent` 双向消息协议(`#[non_exhaustive]` + `serde`)让 TUI / daemon / WebUI 共享 wire

**P0**:kernel 必须 `mount` / `register` 二阶段 —— laew 的 `ToolRegistry` 现在 register + mount 合一,应该拆出 `MountedTools` 写时复制快照机制,让 hot-swap tool catalog 不影响 agent 当前 turn

**P1**:`ToolMiddleware::before/after` 注册顺序 = 契约 —— laew 的 QC 审批/工具脱敏应该有 `BeforeOutcome::Allow / Ask / Deny / DenyTurn` 而不是简单 `Result`,让"总是允许"和"总是否定"显式短路

**P1**:`HookChain` 扇出 + 每个 hook 独立 slot —— laew 应该把 Yolo / SessionContext / Verify / Todo / Datadog 各自实现 `LifecycleHooks`,然后用 `HookChain` 组合

**P1**:`RateLimitDecision::WaitAndRetry / Pause` 区分自动等待 vs 用户决策 —— laew 当前 429 处理粗糙,可以借鉴 `RATE_LIMIT_AUTO_WAIT_SECS = 120` 阈值

**P2**:`TurnCtx` 三层关联 ID(session/turn/request)确定性单调 —— laew 的 `session_memory` 表可以借此补齐 per-turn correlation

---

## 3. MCP 完整实现细节

### 3.1 模块切分(7 个子模块 + types + util)

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/` 子模块 + 行数:

| 子模块 | 行数 | 职责 |
|--------|------|------|
| `mod.rs` | 62 | 模块入口 + `register_mcp_tools()` |
| `client.rs` | 41 | `McpClient` trait + `McpToolInfo` |
| `registry.rs` | 1638 | `McpRegistry`(server 连接管理 + Trust + Alias) |
| `config.rs` | 1075 | `McpServerConfig` / `load_mcp_config()` |
| `oauth.rs` | 845 | MCP OAuth login + token store |
| `transport_http.rs` | 852 | Streamable HTTP/SSE 传输 + OAuth 自动 refresh |
| `transport_stdio.rs` | 910 | Stdio JSON-RPC 传输 + 三锁分离 |
| `tool.rs` | 355 | `McpToolAdapter`(MCP tool → kernel Tool) |
| `trust.rs` | 242 | 项目级 trust store (`mcp_trust.json`) |
| `types.rs` | 270 | JSON-RPC 类型 + ServerStatus + ContentBlock |
| `util.rs` | 23 | `config_dir()`(项目级工具) |

### 3.2 `McpClient` trait(client.rs)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/client.rs
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn initialize(&mut self) -> Result<InitializeResult>;
    async fn list_tools(&self) -> Result<ListToolsResult>;
    async fn call_tool(&self, tool_name: &str, arguments: serde_json::Value) -> Result<CallToolResult>;
    fn server_name(&self) -> &str;
    fn status(&self) -> ServerStatus;
}
```

**接口干净**:5 个方法足以承载 JSON-RPC `initialize` / `tools/list` / `tools/call`。`status()` 是 sync 访问(让 `Tool::risk` 在 sync 路径上能 inspect),不阻塞。

### 3.3 `McpRegistry` —— 多 server + trust + auto-approve 集中管理(registry.rs:123-155)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs (line 123-155)
pub struct McpRegistry {
    servers: Arc<RwLock<BTreeMap<String, Arc<dyn McpClient>>>>,
    server_timeouts_ms: Arc<RwLock<BTreeMap<String, u64>>>,
    failed_servers: Arc<RwLock<BTreeMap<String, String>>>,
    status_overrides: Arc<std::sync::RwLock<BTreeMap<String, ServerStatus>>>,
    configured_servers: Arc<std::sync::RwLock<HashSet<String>>>,
    connect_events: Option<mpsc::UnboundedSender<McpConnectEvent>>,
    initial_ready: watch::Sender<bool>,
    cancelled: watch::Sender<bool>,
    trusted_servers: Arc<std::sync::RwLock<HashSet<String>>>,
    auto_approved_tools: Arc<std::sync::RwLock<HashSet<String>>>,
    tool_aliases: Arc<std::sync::RwLock<BTreeMap<String, (String, String)>>>,
    server_instructions: Arc<std::sync::RwLock<BTreeMap<String, String>>>,
}
```

**核心字段分类**:
1. **连接状态**:`servers` + `server_timeouts_ms` + `failed_servers` + `status_overrides` + `configured_servers`
2. **广播同步**:`connect_events`(一次性事件给 TUI 滚动展示)+ `initial_ready`(level-triggered 广播)
3. **取消**:`cancelled`(dropped/replaced runtime 取消待 work)
4. **信任**:`trusted_servers` + `auto_approved_tools`(std::sync RwLock,**因为 `Tool::risk` 是 sync 的不能 await**)
5. **别名映射**:`tool_aliases`(sanitized name → original `(server, tool)` identity)
6. **运行时 prompt 注入**:`server_instructions`(initialize-time 拿到,注入 `<mcp-server-instructions>` 块)

### 3.4 `add_server` 流程(registry.rs:647-714)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs (line 647-714)
pub async fn add_server(&self, config: McpServerConfig) -> Result<()> {
    // 1. configured_servers 集合加入(用于 /mcp 显示)
    // 2. apply_trust_from_config: trust 在 config 上,不依赖连接(断线重连后仍然 trust)
    // 3. 根据 config.config 选 StdioClient 或 HttpClient
    // 4. client.initialize() 失败时记入 failed_servers(不消失,/mcp 仍然显示)
    // 5. 成功:servers / timeouts 写入;failed_servers / status_overrides 清理
}
```

**WHY 信任"基于 config 而非连接"**(注释 line 656-658):
> "Trust is config-based, not connection-based: record it up front so a tool's risk() can consult it (and so a reconnect after a transient failure is still trusted)."

### 3.5 `call_tool` 锁策略(registry.rs:851-886)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs (line 851-886)
pub async fn call_tool(&self, server_name: &str, tool_name: &str, arguments: serde_json::Value)
    -> Result<String>
{
    // Take a stable client snapshot, then release the registry lock before the
    // potentially slow transport call. Reload/add-server writes must not wait
    // for an MCP tool execution to finish or time out.
    let client = {
        let servers = self.servers.read().await;
        servers.get(server_name).map(Arc::clone)
            .ok_or_else(|| anyhow::anyhow!("MCP server '{}' not found", server_name))?
    };
    let result = client.call_tool(tool_name, arguments).await?;
    // Extract text from content blocks ...
}
```

**WHY 快照 + 释放**:注释 line 856-859 钉死 —— 不能跨 `.await` 持锁,否则慢 MCP 调用阻塞 /mcp reload 和 add-server 写。

### 3.6 Stdio 传输 —— 三锁分离(transport_stdio.rs:31-73)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/transport_stdio.rs (line 31-73)
pub struct StdioClient {
    // ...
    /// Serialize request/response round-trips.
    /// MCP over stdio is a single ordered byte stream. Allowing concurrent
    /// in-flight requests can lead to response mix-ups or one caller
    /// consuming the other's response, causing timeouts.
    request_lock: Arc<Mutex<()>>,
    /// Keeps an operation's request, recovery decision, and optional retry in
    /// one critical section. Without this, a request from the failed generation
    /// can overlap the replacement process's initialize handshake.
    operation_lock: Arc<Mutex<()>>,
    /// Serializes teardown + respawn. Concurrent callers that observe the same
    /// dead pipe share one reconnect instead of spawning duplicate servers.
    reconnect_lock: Arc<Mutex<()>>,
    /// Wakes operations that arrived while an uncertain tool call's transport
    /// was being rebuilt in the background.
    recovery_notify: Arc<Notify>,
    recovery_in_progress: Arc<AtomicBool>,
    /// Advances after every successful initialize handshake. A waiter compares
    /// its failed generation after taking `reconnect_lock` to detect that another
    /// caller already repaired the connection.
    connection_generation: Arc<AtomicU64>,
}
```

**三锁分离的关键洞察**(注释 line 43-72 完整说明):

| 锁 | 关注点 | 持有者 |
|----|-------|--------|
| `request_lock` | 序列化 req/rsp 字节流(一问一答严格对应) | 每次 call_tool |
| `operation_lock` | 保活 + 恢复决策同处一临界区(防竞态) | 长时间操作 |
| `reconnect_lock` | teardown + respawn 序列化(并发观察者共享一次重连) | 后台重连 |

**`connection_generation` 单调递增**(注释 line 69-72):等待者重连后比较自己的 generation,发现已被其他 caller 修复就直接放行。

### 3.7 HTTP/SSE 传输 —— streamable HTTP(transport_http.rs:30-58)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/transport_http.rs (line 30-58)
const MCP_HTTP_ACCEPT: &str = "application/json, text/event-stream";
/// Since protocol revision `2025-06-18` the HTTP transport REQUIRES every request that
/// follows `initialize` to carry the negotiated revision in this header; a server that
/// enforces it rejects unlabelled requests with HTTP 400.
const MCP_PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";

/// HTTP-based MCP client.
pub struct HttpClient {
    server_name: String,
    url: String,
    headers: BTreeMap<String, String>,
    auth: Option<McpHttpAuthConfig>,
    timeout_ms: u64,
    status: Arc<Mutex<ServerStatus>>,
    next_id: AtomicU64,
    client: reqwest::Client,
    /// Streamable-HTTP session id. Stateful servers (e.g. Figma Dev Mode) return
    /// `Mcp-Session-Id` on the `initialize` response and REJECT later requests that
    /// don't echo it; we capture it from every response and replay it on every request.
    session_id: Arc<Mutex<Option<String>>>,
    /// Protocol revision the server agreed to in its `initialize` result — the value
    /// every later request must echo in `MCP-Protocol-Version`.
    negotiated_version: Arc<Mutex<Option<String>>>,
}
```

**HTTP 传输关键约束**(line 29-38):
- `Accept` 必须同时包含 `application/json` 和 `text/event-stream`(streamable HTTP 端点强制要求)
- `MCP-Protocol-Version` 在 `initialize` 之后必须 echo 服务端 negotiate 的版本
- Stateful server 返回的 `Mcp-Session-Id` 必须每次都 echo

### 3.8 OAuth 登录 + Refresh(oauth.rs)

`McpTokenStore` 用 TOML 存 `~/.atomcode/mcp_auth.toml`(line 102-152),**`fs::atomic_write` + `0o600`** 写入权限控制。

`login_mcp_oauth` 完整流程(line 231-370):
1. **discover_oauth_metadata** 探测 well-known endpoints(authorization_endpoint / token_endpoint / registration_endpoint)
2. **bind_callback_listener** 起本地 TCP listener 收回调
3. **PKCE 流程**:state (uuid) + verifier (双 uuid 拼接) + challenge (SHA256 → base64url)
4. **register_oauth_client** 必要时动态注册 client
5. 构造 authorize_url,**`open_browser()` 唤起系统浏览器**(fallback to `xdg-open` / `open`)
6. **await_oauth_callback** 阻塞等待回调(含 state 校验防 CSRF)
7. token exchange 后 `token_from_response` 构造 `McpOAuthToken`

**`refresh_mcp_oauth_token` 失败 fail-fast**(注释 line 161-193):refresh token 缺失、token_endpoint 缺失、client_id 缺失分别 bail。

### 3.9 Trust 模型(trust.rs + registry.rs:61-82)

**项目级 trust key 算法**(registry.rs:61-82 + 注释 line 47-60 钉死):
```
1. strip_verbatim_prefix (BEFORE backslash replacement)
2. 替换 \\ → /
3. 去掉尾部 / (除根)
4. Windows 上 lowercase
5. 用 PathBuf (component-prefix hash) 不用 str::hash
6. 格式化为 {:016x}
```

**3 级 trust 判定**(tool.rs:136-147):
```rust
fn risk(&self, _args: &str) -> RiskLevel {
    // 1. server-declared readOnlyHint → Safe
    // 2. trusted_servers (config trust:true) → Safe
    // 3. auto_approved_tools (server autoApprove 列表 / "总是") → Safe
    // 4. 其余 → Risky (需审批)
    if self.read_only
        || self.registry.is_server_trusted(&self.server)
        || self.registry.is_tool_auto_approved(&self.full_name)
    {
        RiskLevel::Safe
    } else {
        RiskLevel::Risky
    }
}
```

### 3.10 `mcp_tool_full_name` + 命名规范化(tool.rs:24-67)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/tool.rs (line 19-67)
/// Replace every character that OpenAI/litellm forbids in a `function.name`
/// (anything outside `[a-zA-Z0-9_-]`) with `-`. Real MCP servers routinely
/// declare server/tool names containing spaces, dots, colons or CJK characters;
/// unsanitized they break the whole request with a 400
/// `invalid_request_error` (see issue #1289).
pub fn sanitize_name_segment(s: &str) -> String { ... }

pub const MAX_MCP_TOOL_NAME_LEN: usize = 64;

pub fn mcp_tool_full_name(server: &str, tool: &str) -> String {
    let raw = format!("mcp__{server}__{tool}");
    if raw.len() <= MAX_MCP_TOOL_NAME_LEN
        && raw.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return raw;
    }
    // Too long OR invalid chars → SHA256 hash suffix 保证唯一性
    let readable = format!("mcp__{}__{}", sanitize_name_segment(server), sanitize_name_segment(tool));
    let mut hasher = Sha256::new();
    hasher.update((server.len() as u64).to_be_bytes());
    hasher.update(server.as_bytes());
    hasher.update((tool.len() as u64).to_be_bytes());
    hasher.update(tool.as_bytes());
    let suffix = format!("__{}", &format!("{:x}", hasher.finalize())[..32]);
    // ...
}
```

**WHY 这个 hash suffix**(注释 line 41-43):超长 / 非法字符时稳定 hash 保证两个 sanitize 后 readable prefix 相同的名字仍然 distinct。CJK 场景实测通过(test line 300-315)。

**`always_grant_scope` 返回空串**(tool.rs:155-160)—— 让"总是允许"绑到 tool name 而不是 args,跨不同 args 复用 grant:
```rust
fn always_grant_scope(&self, _args: &str) -> String {
    String::new()  // 工具级别的 grant
}
```

### 3.11 `McpToolAdapter` execute 错误统一映射(tool.rs:162-198)

```rust
async fn execute(&self, args: &str, _ctx: &ToolContext) -> ToolResult {
    // 1. 解析 args (JSON)
    // 2. registry.call_tool(server, tool, arguments)
    // 3. Ok(content) → ToolResult { content, is_error: false }
    // 4. Err(e) → ToolResult { content: e.to_string(), is_error: true }
    // 永远不 panic (kernel PANIC CONTRACT)
}
```

**关键**:MCP 的 `Result<String>` 错误不会泄漏到 kernel —— 统一映射为 `ToolResult { is_error: true }`。

### 3.12 借鉴要点(MCP)

**P0**:laew 当前没有 MCP,借鉴 `McpClient` trait(5 个方法)+ `McpToolAdapter` 把 MCP tool 包装成 kernel Tool 是最快落地路径。建议的 trait:
```rust
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn initialize(&mut self) -> Result<InitializeResult>;
    async fn list_tools(&self) -> Result<ListToolsResult>;
    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<CallToolResult>;
    fn server_name(&self) -> &str;
    fn status(&self) -> ServerStatus;
}
```

**P0**:`mcp__{server}__{tool}` 命名 + `MAX_MCP_TOOL_NAME_LEN=64` + `sanitize_name_segment` 防止 OpenAI/litellm 400,这是生产踩过的坑(`#1289`)

**P0**:stdio 传输的三锁分离(request / operation / reconnect)+ `connection_generation` 单调递增,laew 的 stdio MCP 集成必须抄这套,否则会活锁

**P1**:trust 三级判定(read_only → server_trusted → tool_auto_approved)—— laew 的 tool 审批可以统一用这个降级链

**P1**:`always_grant_scope` 返回空串做 tool-wide grant —— laew 的"总是允许"目前是 per-call,可以改为 tool-wide 让用户少点几次确认

**P2**:OAuth PKCE + 动态 client registration + `McpTokenStore` TOML 持久化(`0o600`)—— laew 的 MCP OAuth 可以全套抄

---

## 4. Skill 一等公民设计

### 4.1 模块切分(5 个子模块)

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/` 子模块 + 行数:

| 子模块 | 行数 | 职责 |
|--------|------|------|
| `mod.rs` | 53 | 模块入口 + `register_skill_tools()` |
| `registry.rs` | 571 | `SkillRegistry` + scan + get + render_catalog |
| `render.rs` | 401 | 渲染 catalog + source_rank + budget gate |
| `skill.rs` | 519 | `Skill` + frontmatter + expand + shell injection |
| `use_skill.rs` | 316 | `UseSkillTool` + `ListSkillsTool` |
| `catalog_hook.rs` | 128 | `SkillCatalogHook` (注入到 session_start) |

### 4.2 Skill 加载 —— 两形态 + 命名空间(skill.rs:8-26 + registry.rs:36-74)

```rust
// skill.rs:8-26
pub struct Skill {
    pub name: String,
    pub description: String,
    pub template: String,
    /// Tools the specialization MAY auto-approve while this skill is active
    /// (metadata; the L1 capability does not enforce it — that's an L2 approval-policy concern).
    pub allowed_tools: Vec<String>,
    /// If false (`user-invocable: false`), hidden from the `/` menu; the model
    /// can still auto-invoke it. Absent → true.
    pub user_invocable: bool,
    /// Directory containing the skill file (for `${CLAUDE_SKILL_DIR}`).
    pub skill_dir: PathBuf,
    pub source_path: PathBuf,
}
```

**两种文件形态**(registry.rs:46-74 `scan_skill_dir`):
- **flat `*.md`** —— 仅 depth=0;`name = file stem`;内容 = frontmatter + template
- **dir `<dir>/SKILL.md`** —— `name = directory name`;可捆绑 `scripts/ / references/`
- **嵌套 group dir** —— 没有 `SKILL.md` 的子目录递归 scan(depth 限 8 防 symlink cycle)

**frontmatter 字段**(skill.rs:196-214):
- `name`:`description` 默认取 template 首段
- `allowed-tools`:空格/逗号分隔的工具列表(metadata)
- `user-invocable: false`:隐藏于 `/` 菜单,模型仍可自动调用

**命名空间**:插件技能注册为 `<namespace>:<skill-name>`(registry.rs:152-156),允许 `~/.claude/skills` 和 `~/.atomcode/skills` 同名 skill 共存而不冲突。

### 4.3 Skill 展开引擎(skill.rs:28-92)

```rust
// skill.rs:28-92
pub fn expand(&self, arguments: &str, session_id: &str) -> String {
    let positional: Vec<&str> = arguments.split_whitespace().collect();
    let skill_dir = self.skill_dir.to_string_lossy();

    // SINGLE left-to-right pass: each substitution's value is emitted literally and
    // never re-scanned — so an argument that itself contains `$1` is NOT re-expanded.
    let t = self.template.as_str();
    let mut result = String::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        let rest = &t[i..];
        if let Some((value, len)) = match_substitution(rest, ...) {
            result.push_str(value);
            i += len;
        } else {
            let ch = rest.chars().next().unwrap();
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    // A template with no `$ARGUMENTS` token at all still gets the full args appended.
    if !self.template.contains("$ARGUMENTS") && !arguments.trim().is_empty() {
        result = format!("{}\n\nARGUMENTS: {}", result.trim_end(), arguments);
    }
    expand_shell_injections(&result)
}

pub fn expand_for_injection(&self, arguments: &str, session_id: &str) -> String {
    let body = self.expand(arguments, session_id);
    match self.bundled_resource_note() {
        Some(note) => format!("{note}\n\n{body}"),
        None => body,
    }
}
```

**支持的 substitution 语法**(match_substitution, skill.rs:107-148):
- `$ARGUMENTS[N]` / `$N` —— 位置参数
- `$ARGUMENTS` —— 全部参数(追加若模板没出现)
- `${CLAUDE_SESSION_ID}` / `${CLAUDE_SKILL_DIR}` —— 内置变量
- `!`cmd`` —— shell 注入(`sh -c` 执行后取 stdout)

**关键设计点**:
- **Single left-to-right pass**:替换值不再被重扫 —— 防止 `$1` 类递归展开(注释 line 39-40)
- **Longest-token-first**:最长匹配优先
- **DEFINED positional indices only**:`$99` 当 positional 不到 99 时保留字面量
- **数字 greedy run**:`$10` ≠ `$1 + 0`,maximal digit run

**`bundled_resource_note` 系统提醒**(skill.rs:76-91):目录型 skill 自动追加 `<\system-reminder>` 指引 bundled 资源在 skill_dir 下 —— 避免模型去 cwd 找 `scripts/x.py` 失败。

**`expand_shell_injections` shell 注入**(skill.rs:150-191):

```rust
fn run_shell_command(cmd: &str) -> String {
    let mut command = Command::new("sh");
    command.arg("-c").arg(cmd);
    #[cfg(unix)]
    crate::process_utils::apply_utf8_locale_env_sync(&mut command);
    // No console-window flash when run from a console-less daemon
    crate::process_utils::suppress_console_window_sync(&mut command);
    match command.output() {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !stderr.trim().is_empty() {
                    s.push('\n');
                    s.push_str(stderr.trim());
                }
            }
            s.trim_end().to_string()
        }
        Err(e) => format!("[error: {e}]"),
    }
}
```

**WHY shell 注入是设计意图**(skill.rs:1-7 模块注释):
> "skills are TRUSTED, user-authored content (the same trust as a slash command the user installed), so this is by design, not arbitrary remote code."

### 4.4 Skill 目录优先级(render.rs:72-101 `source_rank`)

```rust
// render.rs:72-101
pub fn source_rank(path: &Path) -> u8 {
    if native_config_root().is_some_and(|root| path.starts_with(&root)) {
        return 0;  // 本构建 own tree(用 ATOMCODE_HOME prefix,不依赖名字)
    }
    let s = path.to_string_lossy();
    if s.contains(".atomcode") { 0 }
    else if s.contains(".claude") { 1 }
    else if s.contains(".agents") { 2 }
    else { 3 }
}

fn native_config_root() -> Option<std::path::PathBuf> {
    std::env::var_os("ATOMCODE_HOME").filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
}
```

**关键陷阱注释**(render.rs:50-71):

> "Prefix, not substring: `$ATOMCODE_HOME` can be any path, and a substring test on a short directory name would rank unrelated paths that merely contain it. The `.atomcode` branch stays as the fallback for the window before the variable is settled (and for tests that do not set it), which is also what keeps this byte-identical on a build whose home IS `~/.atomcode`."

**WHY 这个修复**:原版用字面量 `.atomcode` 字串匹配,当 build 重命名为 `.longcode` 时,自家插件被 rank=3,劣于 `.claude` (1) 和 `.agents` (2) —— budget squeeze 时自家插件被裁,第三方的留下。症状是从外部看不见的:`/plugin list` 显示正常但 model 完全不知道这个 skill 存在。

### 4.5 Catalog 渲染 —— Budget gate(render.rs:24-178)

```rust
// render.rs:24-178
pub const CATALOG_BYTE_BUDGET: usize = 8000;
pub const PER_SKILL_DESC_CAP: usize = 1024;
pub const CATALOG_HEADER: &str = "=== AVAILABLE SKILLS ===";

const GUIDANCE: &str = "Skills are reusable instruction templates for specific tasks. ... \
    If a task clearly matches a shown skill's description — not only when the user names \
    the skill — you MUST load that exact skill with `use_skill` and follow it BEFORE \
    doing the work, INCLUDING before asking clarifying questions, exploring, or planning. \
    If this catalog says skills were omitted, call `list_skills` before using an omitted \
    or otherwise unlisted name, and use only an exact name it returns. \
    If no available skill matches, proceed normally. ...";

pub fn render_skill_catalog_prioritizing(entries: &[CatalogEntry], priority_names: &[String])
    -> Option<String>
{
    if entries.is_empty() { return None; }

    let mut sorted: Vec<&CatalogEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| {
        let a_priority = priority_names.iter().any(|name| name == &a.name);
        let b_priority = priority_names.iter().any(|name| name == &b.name);
        b_priority.cmp(&a_priority).then_with(|| {
            a.source_rank.cmp(&b.source_rank).then_with(|| a.name.cmp(&b.name))
        })
    });

    let mut lines = Vec::new();
    let mut body_bytes = 0usize;
    let mut omitted = 0usize;
    for e in &sorted {
        let hint = e.hint.as_deref().map(|h| format!(" {h}")).unwrap_or_default();
        let line = format!("- {}{}: {}", e.name, hint, truncate_desc(&e.description));
        let cost = line.len() + 1;
        // Always emit at least the top-ranked skill even if it alone is huge.
        if lines.is_empty() || body_bytes + cost <= CATALOG_BYTE_BUDGET {
            body_bytes += cost;
            lines.push(line);
        } else {
            omitted += 1;
        }
    }

    let mut out = String::new();
    out.push_str(CATALOG_HEADER);
    out.push('\n');
    out.push_str(GUIDANCE);
    out.push('\n');
    out.push_str(&lines.join("\n"));
    if omitted > 0 {
        out.push('\n');
        out.push_str(&format!("... and {omitted} more lower-priority skills not shown."));
    }
    Some(out)
}
```

**关键设计**:
- **预算 8000 字节**(不是字符数,CJK bite 早 —— 注释 line 22-25 明示)
- **Per-skill 描述上限 1024 字符**(用 `…` 截断,char boundary 安全)
- **Always emit top-ranked**:即使其一行超 budget
- **Priority names**:用户在项目/用户指令里显式提到的 skill 名优先生存(即使 rank 低)
- **Source-rank 三档**:自家 > `.claude` > `.agents` > 其它
- **Omission note**:`... and N more lower-priority skills not shown.` 提示用 `list_skills` 发现
- **GUIDANCE 严格措辞**:"only skill names you may pass directly", "never invent or guess", "say why if skipped" —— 防 hallucinate skill name + 强制 justify skip

### 4.6 Catalog Hook —— session_start 注入(catalog_hook.rs:23-66)

```rust
// catalog_hook.rs:23-66
pub struct SkillCatalogHook {
    catalog: Option<String>,
}

#[async_trait]
impl LifecycleHooks for SkillCatalogHook {
    async fn session_start(&self, convo: &mut Conversation, _resumed: bool) {
        let existing = convo.messages.iter()
            .position(|m| m.role == Role::System && m.text.starts_with(CATALOG_HEADER));
        match (&self.catalog, existing) {
            (Some(block), Some(i)) => convo.messages[i] = Message::system(block.clone()),
            (Some(block), None) => {
                let at = leading_system_count(convo);
                convo.messages.insert(at, Message::system(block.clone()));
            }
            // No skills now but a stale catalog survives a resume → drop it.
            (None, Some(i)) => convo.messages.remove(i),
            (None, None) => {}
        }
    }
}

fn leading_system_count(convo: &Conversation) -> usize {
    convo.messages.iter().take_while(|m| m.role == Role::System).count()
}
```

**WHY position-based reconcile**:注释 line 47-48 钉死 —— "Position-based reconcile handles fresh (absent → insert) and resume (present → replace in place, byte-identical when unchanged) uniformly"。`--resume` 重载直接 in-place 替换而不增长消息数(test line 102-128)。

**插入点 = `leading_system_count`**:紧跟 persona 后的第一个 system 块,保证 catalog 在 persona + 上下文块之后、user 消息之前。

### 4.7 渲染管线整体(驱动 + catalog hook + tool)

1. **prepare 时**(parts.rs:785):`SkillRegistry::load(standard_skill_dirs)` 加载
2. **render_catalog**:`render_skill_catalog_prioritizing(&[], ...)` 渲染文本
3. **SkillCatalogHook 注册到 HookChain**:`parts.hooks.push(Arc::new(SkillCatalogHook::new(rendered)))`
4. **`session_start` 自动触发**:插入到 `convo.messages` 第一个 user 消息前
5. **模型看到 catalog**:决定是否调用 `use_skill` / `list_skills`
6. **`use_skill` 调用**:`UseSkillTool::execute` → `Skill::expand_for_injection` → 替换后的文本作为 tool result 回到上下文

### 4.8 `UseSkillTool` / `ListSkillsTool`(use_skill.rs:12-127)

`UseSkillTool::execute` 流程:
1. 解析 args → skill_name + arguments
2. `registry.get(skill_name)` 解析(支持 bare name fallback,registry.rs:95-126 详细注释)
3. `skill.expand_for_injection(arguments, session_id)` 替换
4. 返回 ToolResult(若 skill 不存在则列出可用 skill 名)

`ListSkillsTool`:列出 `(name, description)` tuples,按 name 排序。

### 4.9 借鉴要点(Skill)

**P0**:laew 当前没有 Skill 系统,应该引入 `Skill` struct + `SkillRegistry` + `render_skill_catalog` + `SkillCatalogHook`(`session_start` 触发注入)。frontmatter 兼容 Claude-Code 格式(`name` / `description` / `allowed-tools` / `user-invocable`)

**P0**:catalog 渲染必须有 budget gate + 优先级 + omission note —— 否则 60+ skill 直接 dump 到 system prompt,挤爆 context window

**P1**:`source_rank` 用 `ATOMCODE_HOME` prefix 匹配,**不要**用字串匹配 —— 这是 atomcode 实战踩过的坑(重命名后自家插件被裁)

**P1**:`SkillCatalogHook` 用 position-based reconcile(`starts_with(CATALOG_HEADER)`)处理 fresh + resume 两个路径,**不要**append 一个新的 system 消息 —— 防止 resume 后 catalog 累积

**P1**:`expand` 用 single left-to-right pass,禁止替换值再被扫 —— 防止 `$1` 类递归

**P2**:`expand_for_injection` 给 dir skill 加 `<\system-reminder>` 指明 skill_dir,避免模型去 cwd 找 bundled scripts

**P2**:shell 注入是**设计意图**(trusted content),但要用 `apply_utf8_locale_env_sync` + `suppress_console_window_sync` 处理 locale + Windows 闪窗

**P2**:GUIDANCE 文本要把防 hallucinate + 强制 justify skip 都写死("only skill names you may pass directly", "never invent or guess", "say why if skipped")

---

## 5. CodeIntel 代码智能

### 5.1 模块切分(11 个子模块 + LSP)

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/` 子模块 + 行数:

| 子模块 | 行数 | 职责 |
|--------|------|------|
| `mod.rs` | 216 | 模块入口 + `register_codeintel_tools()` + `LspSettings` |
| `graph.rs` | 356 | `CodeGraph` + `SymbolNode` + `Edge` + BFS trace |
| `index.rs` | 520 | `build_graph` + `CodeIndex` (cache) + tree-sitter parse + 解析 calls |
| `symbols.rs` | 294 | `extract_symbols` (tree-sitter queries) |
| `list_symbols.rs` | 176 | `ListSymbolsTool` |
| `read_symbol.rs` | 200 | `ReadSymbolTool` |
| `find_references.rs` | 227 | `FindReferencesTool` (whole-word text scan) |
| `trace_callers.rs` | 151 | `TraceCallersTool` |
| `trace_callees.rs` | 134 | `TraceCalleesTool` |
| `trace_chain.rs` | 151 | `TraceChainTool` |
| `blast_radius.rs` | 172 | `BlastRadiusTool` (BLAST RADIUS) |
| `file_deps.rs` | 151 | `FileDependenciesTool` |
| `diagnostics.rs` | 151 | `DiagnosticsTool` (LSP only) |
| `lsp_tool.rs` | 460 | `LspTool` (LSP only) |
| `lang.rs` | 165 | `Lang` enum + grammar mapping |

### 5.2 分层架构(mod.rs:7-17 注释)

```rust
//! # Layers
//!
//! - **symbol layer** (single-file, STATELESS): `list_symbols` / `read_symbol` parse one
//!   file on demand — no shared state, nothing from the kernel `ToolContext` beyond
//!   `working_dir`.
//! - **graph layer** (cross-file): `find_references` (whole-word text scan) plus
//!   `trace_callers` / `trace_callees` / `trace_chain` / `blast_radius` /
//!   `file_dependencies`, backed by a shared, lazily-built [`CodeIndex`] (the symbol
//!   layer's statelessness ends here — these tools HOLD an `Arc<CodeIndex>`).
//!
//! Deferred vs production: visibility inference; import-aware call
//! resolution; background/incremental indexing (we rebuild on mtime change).
```

**8 个 tool 挂载清单**(mod.rs:68-79):
```rust
pub fn codeintel_tool_names() -> &'static [&'static str] {
    &[
        "list_symbols", "read_symbol",
        "find_references",
        "trace_callers", "trace_callees", "trace_chain",
        "blast_radius",
        "file_dependencies",
    ]
}
```

### 5.3 `CodeGraph` 数据模型(graph.rs:14-82)

```rust
// graph.rs:14-82
pub type SymbolId = u64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function, Method, Struct, Class, Trait, Interface,
    Enum, Constant, Variable, Module, Import, TypeAlias,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public, Private, Protected, Internal, Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SymbolNode {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Calls, Imports, Inherits, Implements, References,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub to: SymbolId,
    pub kind: EdgeKind,
    pub line: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CodeGraph {
    pub nodes: HashMap<SymbolId, SymbolNode>,
    pub edges_out: HashMap<SymbolId, Vec<Edge>>,
    pub edges_in: HashMap<SymbolId, Vec<Edge>>,
    pub file_symbols: HashMap<PathBuf, Vec<SymbolId>>,
    pub file_mtimes: HashMap<PathBuf, u64>,
    /// name → symbol ids. Derivable from `nodes`; `#[serde(skip)]` keeps it out of any
    /// serialized form, so `rebuild_name_index` must be called after a deserialize.
    #[serde(skip)]
    pub by_name: HashMap<String, Vec<SymbolId>>,
}
```

**Edge 约定(load-bearing)**(graph.rs:6-7 注释钉死):
> "**EDGE CONVENTION (load-bearing, from production)**: `edges_out[from]` holds `Edge{to: callee}` (forward); `edges_in[to]` holds `Edge{to: from}` — i.e. in the reverse map the `to` field stores the SOURCE (caller). Do not 'fix' this."

**`make_id` 确定性**(graph.rs:88-96):
```rust
pub fn make_id(file: &Path, name: &str, start_line: usize) -> SymbolId {
    let mut h = DefaultHasher::new();
    file.hash(&mut h);
    name.hash(&mut h);
    start_line.hash(&mut h);
    h.finish()
}
```

`DefaultHasher` + `(file, name, start_line)` 三元组 → 跨 run 稳定的 SymbolId。

**`add_edge` 同步双向**(graph.rs:106-117):
```rust
pub fn add_edge(&mut self, from: SymbolId, edge: Edge) {
    let to = edge.to;
    let kind = edge.kind.clone();
    let line = edge.line;
    self.edges_out.entry(from).or_default().push(edge);
    // reverse map stores the SOURCE in `to` (production convention).
    self.edges_in.entry(to).or_default().push(Edge {
        to: from, kind, line,
    });
}
```

### 5.4 BFS Trace(graph.rs:151-180)

```rust
// graph.rs:151-180
/// BFS over incoming edges (who calls `id`), up to `max_depth`.
pub fn trace_callers(&self, id: SymbolId, max_depth: usize) -> Vec<(SymbolId, usize)> {
    self.trace(id, max_depth, true)
}
/// BFS over outgoing edges (what `id` calls), up to `max_depth`.
pub fn trace_callees(&self, id: SymbolId, max_depth: usize) -> Vec<(SymbolId, usize)> {
    self.trace(id, max_depth, false)
}
fn trace(&self, id: SymbolId, max_depth: usize, callers: bool) -> Vec<(SymbolId, usize)> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut result = Vec::new();
    visited.insert(id);
    queue.push_back((id, 0usize));
    while let Some((cur, depth)) = queue.pop_front() {
        if depth >= max_depth { continue; }
        let edges = if callers {
            self.edges_in.get(&cur)
        } else {
            self.edges_out.get(&cur)
        };
        for edge in edges.into_iter().flatten() {
            if visited.insert(edge.to) {
                result.push((edge.to, depth + 1));
                queue.push_back((edge.to, depth + 1));
            }
        }
    }
    result
}
```

返回 `(caller_id, depth)` pairs,deduped。

### 5.5 `CodeIndex` 缓存(index.rs:302-324)

```rust
// index.rs:302-324
/// Shared, lazily-built code index the graph tools hold. `get` returns a cached graph
/// when the indexed files' (path, mtime) fingerprint is unchanged, else rebuilds. O(repo)
/// and CPU-bound — call from a blocking context (the tools use `spawn_blocking`).
#[derive(Default)]
pub struct CodeIndex {
    cache: Mutex<Option<(u64, Arc<CodeGraph>)>>,
}

impl CodeIndex {
    pub fn get(&self, root: &Path) -> Arc<CodeGraph> {
        let root = super::canonical(root);
        let files = collect_files(&root);
        let fp = fingerprint(&files);
        if let Some((cfp, g)) = self.cache.lock().unwrap().as_ref() {
            if *cfp == fp {
                return g.clone();
            }
        }
        let g = Arc::new(build_from_files(&root, files));
        *self.cache.lock().unwrap() = Some((fp, g.clone()));
        g
    }
}
```

**指纹 = `(path, mtime_ns, len)`**(index.rs:140-200):
```rust
struct Walked {
    path: PathBuf,
    /// mtime in NANOSECONDS — coarse whole seconds would miss a same-second edit and
    /// serve a stale graph.
    mtime_ns: u128,
    /// file length — defends against a same-instant edit whose mtime didn't move (content
    /// length almost always changes on a real edit).
    len: u64,
}

fn fingerprint(files: &[Walked]) -> u64 {
    let mut h = DefaultHasher::new();
    for w in files {
        w.path.hash(&mut h);
        w.mtime_ns.hash(&mut h);
        w.len.hash(&mut h);
    }
    h.finish()
}
```

**WHY nanos + len 双字段**(注释 line 144-148 钉死):粗粒度 seconds 会漏掉同秒编辑;mtime 不动但内容变也常见(length 几乎必变)—— test line 396-412 `same_second_edit_triggers_rebuild` 显式覆盖。

**`WalkBuilder` 配置**(index.rs:151-189):
```rust
WalkBuilder::new(root)
    .hidden(true)        // 含 .git 等 hidden
    .git_ignore(true)    // 遵守 .gitignore
    .git_global(true)    // 遵守 ~/.gitignore_global
    .git_exclude(true)   // 遵守 .git/info/exclude
    .build()
```

### 5.6 Call resolution(index.rs:209-252 `resolve_callee`)

```rust
// index.rs:209-252
/// Resolve a callee name to a symbol id, preferring closer candidates (production
/// scoring): same file (4) > same dir (2) > same top-level component (1) > any (0).
/// (Import-based score 3 is omitted — like production, we do not parse imports yet.)
/// Ties are broken DETERMINISTICALLY by the smallest (file, start_line) — production's
/// tie-break depends on HashMap iteration order, which is not reproducible.
fn resolve_callee(g: &CodeGraph, callee: &str, caller_file: &Path, root: &Path)
    -> Option<SymbolId>
{
    let score = |n: &SymbolNode| -> i32 {
        if n.file == caller_file { 4 }
        else if n.file.parent().is_some() && n.file.parent() == caller_file.parent() { 2 }
        else {
            let a = top_component(&n.file, root);
            if a.is_some() && a == top_component(caller_file, root) { 1 } else { 0 }
        }
    };
    let mut best: Option<&SymbolNode> = None;
    let mut best_score = i32::MIN;
    for n in g.find_by_name(callee) {
        let s = score(n);
        let better = match best {
            None => true,
            Some(b) => {
                s > best_score
                    || (s == best_score
                        && (n.file.as_path(), n.start_line) < (b.file.as_path(), b.start_line))
            }
        };
        if better { best = Some(n); best_score = s; }
    }
    best.map(|n| n.id)
}
```

**评分体系**:
- 同文件 4 分
- 同目录 2 分
- 同顶层组件 1 分
- 其他 0 分
- 平局 deterministic:`(file, start_line)` 字典序最小

**WHY deterministic 平局**(注释 line 211-213):production 用 HashMap 迭代顺序(不重现),需要 `DefaultHasher` 的稳定 hash 保证跨 run 一致。test line 414-445 `tie_break_resolution_is_deterministic` 显式覆盖。

### 5.7 `extract_calls` —— tree-sitter 调用捕获(index.rs:50-108)

```rust
// index.rs:50-108
fn extract_calls(source: &str, lang: Lang, syms: &[Symbol]) -> Vec<RawCall> {
    let Some(q_src) = lang.calls_query() else { return Vec::new(); };
    let grammar = lang.grammar();
    let mut parser = Parser::new();
    if parser.set_language(&grammar).is_err() { return Vec::new(); }
    let Some(tree) = parser.parse(source, None) else { return Vec::new(); };
    let Ok(query) = Query::new(&grammar, q_src) else { return Vec::new(); };
    let Some(callee_idx) = query.capture_index_for_name("callee") else { return Vec::new(); };

    let mut cursor = QueryCursor::new();
    let mut calls = Vec::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    loop {
        matches.advance();
        let m = match matches.get() { Some(m) => m, None => break };
        for cap in m.captures {
            if cap.index != callee_idx { continue; }
            let callee_name = source[cap.node.start_byte()..cap.node.end_byte()].to_string();
            let line = cap.node.start_position().row + 1;
            // Innermost enclosing Function/Method (max start_line whose range covers the call).
            let caller = syms.iter()
                .filter(|s| matches!(
                    classify_symbol_kind(&s.kind),
                    SymbolKind::Function | SymbolKind::Method))
                .filter(|s| s.start_line <= line && line <= s.end_line)
                .max_by_key(|s| s.start_line);
            if let Some(caller) = caller {
                if caller.name != callee_name {  // 排除 self-call
                    calls.push(RawCall {
                        caller_name: caller.name.clone(),
                        caller_line: caller.start_line,  // 用于 make_id 精确重算
                        callee_name, line,
                    });
                }
            }
        }
    }
    calls
}
```

**为什么 caller_line 而非 caller_name 查找**(注释 line 41-42 钉死):

> "caller's start_line — lets the build reconstruct the caller's exact id via `make_id` instead of a name lookup (removes a scan and fixes wrong-caller attribution)."

test line 469-519 `caller_attribution_is_per_file` 显式覆盖:两个同名 `handler` 函数(a.rs / b.rs),如果用 name 查找,**两条 edge 都挂在一个 handler 上**。`caller_line` 配 `caller_file` 精确 make_id,修复错误归属。

### 5.8 `classify_symbol_kind` —— 跨语言 node kind 映射(index.rs:18-37)

```rust
// index.rs:18-37
fn classify_symbol_kind(ts: &str) -> SymbolKind {
    match ts {
        "function_item" | "function_definition" | "function_declaration" | "func_literal"
            => SymbolKind::Function,
        "method_definition" | "method_declaration" => SymbolKind::Method,
        "struct_item" | "struct_specifier" | "struct_type" => SymbolKind::Struct,
        "class_definition" | "class_declaration" | "class_specifier" => SymbolKind::Class,
        "trait_item" => SymbolKind::Trait,
        "interface_declaration" | "interface_type" => SymbolKind::Interface,
        "enum_item" | "enum_declaration" | "enum_specifier" => SymbolKind::Enum,
        "const_item" | "const_declaration" => SymbolKind::Constant,
        "let_declaration" | "variable_declaration" | "static_item" => SymbolKind::Variable,
        "mod_item" | "module" => SymbolKind::Module,
        "use_declaration" | "import_statement" | "import_declaration" => SymbolKind::Import,
        "type_item" | "type_alias_declaration" => SymbolKind::TypeAlias,
        "impl_item" => SymbolKind::Other("impl".to_string()),
        other => SymbolKind::Other(other.to_string()),
    }
}
```

12 种语言的 node kind 用一个大 match 统一映射到中性 SymbolKind 枚举。

### 5.9 `BlastRadiusTool` —— 爆炸半径(blast_radius.rs)

```rust
// /usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/blast_radius.rs (line 67-113)
fn render(index: &CodeIndex, root: &Path, file: &Path, display: &str) -> ToolResult {
    let g = index.get(root);
    let croot = canonical(root);
    let root: &Path = &croot;
    let cfile = canonical(file);
    let file: &Path = &cfile;
    let symbols = match g.symbols_in_file(file) {
        Some(s) if !s.is_empty() => s.clone(),
        _ => return err(format!("File '{display}' not found in the code graph (no indexed symbols).")),
    };

    // Direct dependents (depth 1): files whose symbols directly call this file's symbols.
    let mut direct: HashSet<std::path::PathBuf> = HashSet::new();
    for sid in &symbols {
        if let Some(edges) = g.callers(*sid) {
            for e in edges {
                if let Some(node) = g.node(e.to) {
                    if node.file != file { direct.insert(node.file.clone()); }
                }
            }
        }
    }
    // Indirect dependents (depth 2-3) = transitive dependents minus the direct ones.
    let indirect: HashSet<std::path::PathBuf> = g.file_dependents(file, 3).into_iter()
        .filter(|f| !direct.contains(f)).collect();
    let total = direct.len() + indirect.len();

    let mut out = format!("Blast radius for {}:\n\n", display_path(file, root));
    out.push_str(&format!("DIRECT DEPENDENTS ({} files):\n", direct.len()));
    out.push_str(&format_files(&direct, root));
    out.push_str(&format!("\nINDIRECT DEPENDENTS ({} files):\n", indirect.len()));
    out.push_str(&format_files(&indirect, root));
    out.push_str(&format!("\nTOTAL IMPACT: {total} files\n"));
    ok(out)
}
```

**算法**:
1. 取文件的所有 symbols
2. **Direct = depth=1 callers**(caller 是 caller,这里遍历所有 symbols 的 incoming edges,收集去重 file set)
3. **Indirect = depth=2..3 dependents 减去 direct**
4. 输出三段式:`DIRECT DEPENDENTS` + `INDIRECT DEPENDENTS` + `TOTAL IMPACT`

**`spawn_blocking` 包装**(line 61-64):
```rust
tokio::task::spawn_blocking(move || render(&index, &root, &file, &display))
    .await
    .unwrap_or_else(|_| err("blast_radius: task failed"))
```

graph 工具都是 O(repo) CPU-bound,工具必须用 `spawn_blocking` 包装(index.rs:301 注释明示)。

### 5.10 `file_dependents`(graph.rs 末尾 + file_deps.rs)

`file_dependencies` 双向展示:
- **which files it USES**:own symbols 的 callees(沿 edges_out 走)
- **which files USE it**:callers(沿 edges_in 走)

### 5.11 `canonical` + `display_path` 路径处理(mod.rs:177-199)

```rust
// mod.rs:177-199
/// Canonicalize a path (resolve symlinks / `.`/`..`), falling back to the original on
/// error. The graph build AND the tool lookups both canonicalize, so a file referenced
/// via a different alias (e.g. macOS `/var` vs `/private/var`) still matches the graph's
/// stored paths instead of a false "not found".
pub(crate) fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Display a path relative to `root` when possible, else shortened to `.../last3`.
pub(crate) fn display_path(p: &Path, root: &Path) -> String {
    if let Ok(rel) = p.strip_prefix(root) {
        return rel.display().to_string();
    }
    let comps: Vec<_> = p.components().collect();
    if comps.len() <= 3 {
        p.display().to_string()
    } else {
        format!(".../{}", comps[comps.len()-3..].iter()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>().join("/"))
    }
}
```

**WHY 双 canonicalize**(注释 line 175-178):macOS 的 `/var` vs `/private/var` alias 必须解析到同一 path,避免 graph 找不到。

### 5.12 借鉴要点(CodeIntel)

**P0**:laew 当前完全没有代码智能能力。如果要落地,建议先做基础三层:
- `extract_symbols(source, lang)` —— tree-sitter 单文件解析(最小依赖,先 Rust 一种)
- `build_graph(root)` —— cross-file 全仓扫
- 工具:`list_symbols` / `read_symbol` / `find_references` / `trace_callers` / `trace_callees` / `blast_radius` / `file_dependencies` —— 与 atomcode 同名

**P0**:`CodeIndex` 缓存 + `(path, mtime_ns, len)` 指纹 —— nanos + len 双字段防同秒编辑漏掉(重要!这是 atomcode 实测覆盖的场景)

**P0**:`extract_calls` 用 `caller_line` + `make_id` 精确归属 caller,不要用 name lookup(同名 handler 会错位,atomcode test line 469-519 覆盖)

**P1**:`resolve_callee` 评分:同文件(4) > 同目录(2) > 同顶层组件(1) > 其它(0);平局 deterministic tie-break `(file, start_line)` —— 不要依赖 HashMap 迭代顺序

**P1**:blast_radius 算法:`direct = depth=1 callers` + `indirect = depth=2..3 dependents \ direct` —— 用 `file_dependents(file, 3)` BFS

**P1**:graph 工具必须 `spawn_blocking`(O(repo) CPU-bound),不能 inline 在 tokio runtime

**P2**:tree-sitter 12 语言 grammar 已经为 `codeintel` feature 预留 cargo feature,**不要**全开 —— 初次实现先只 Rust(`tree-sitter-rust`),按需扩展

**P2**:`canonical()` 双 canonicalize 兼容 macOS `/var` vs `/private/var` 别名

**P2**:`Edge { to: caller }` in `edges_in` 的反向存储约定 —— 工具实现时严格遵守,不要按直觉"修正"

---

## 6. Rust 端借鉴要点(给 laew)

> 综合 5 个二轮深挖点,提炼 laew 当前可以落地的改造建议,按优先级 P0/P1/P2 排列。

### P0(高优先级,立即可落地)

1. **三层 workspace + cargo feature gating**(§1):把 laew 拆 `laew-core`(中性 SDK:会话循环 + 消息 + provider trait + tool trait + hook + middleware)/ `laew-tools`(provider / tools / mcp / skills / memory / session / codeintel + feature gating)/ `laew-app`(6 角色 + 装配)。物理依赖方向由 cargo 强制,cargo tree 单向校验。**预期收益**:编译时间下降、嵌入式裁剪、能力边界清晰。

2. **`AgentCommand` / `AgentEvent` 双向消息协议**(§2.3):参考 atomcode 的 `#[non_exhaustive]` + `#[serde(default)]` + `StopReason`(FAILED PERCEPTION)设计,引入 `AgentCommand` 枚举(SendMessage / SendSynthetic / Compact / Cancel / Snapshot / Respawn 等)+ `AgentEvent`(TextDelta / ToolStarted / ToolResult / TurnComplete / Usage / Error / Cancelled 等)。**预期收益**:TUI / daemon / WebUI 共享同一份 wire 协议,三种前端不再各自维护控制面。

3. **`HookChain` 扇出**(§2.4):把 laew 当前各 Agent 的特殊逻辑(Yolo 分类、SessionContext 摘要、Verify 验证、Todo 管理)各自实现 `LifecycleHooks` trait,然后用 `HookChain` 组合注册。`user_prompt_submit` 短路、`offer_continuation` first Some wins 等组合契约严格遵守。**预期收益**:独立能力共存、各 hook 独立 slot、零冲突。

4. **`mount` / `register` 二阶段 + `MountedTools` 写时复制快照**(§2.6):把 laew 的 `ToolRegistry` 拆出 `register`(全集)+ `mount`(子集)+ `MountedTools { current: Arc<RwLock<Arc<MountedToolsSnapshot>>> }`,让 hot-swap tool catalog 不影响 agent 当前 turn。**预期收益**:MCP 工具动态加载 / 卸载不影响 agent 正在执行的 turn。

5. **`ToolMiddleware::before/after` 顺序契约**(§2.5):把 laew 的审批 / 工具脱敏升级为 `BeforeOutcome::Allow / Ask / Deny / DenyTurn` 五档语义,`Allow` 短路后续 approval chain 的语义显式表达。注册顺序 = load-bearing(注释里钉死)。**预期收益**:`is_allowed` / `is_denied` 简单布尔判断升级为强类型业务决策。

6. **`prepare` → `assemble` 两阶段装配**(§1.5 + §3.4):把 laew 当前所有能力初始化混在一个 `init` 函数拆为 `prepare`(异步 I/O:MCP 后台连接 + skill 加载 + session bind)+ `assemble`(纯组合:注入 provider + 串接 middleware + 注册 hooks)。返回 `CodingParts` 持有 `Arc<ApprovalMiddleware>` / `Arc<HookChain>` / `Arc<SkillRegistry>`,**模型 swap 只重建 provider,复用同一个 Parts**。**预期收益**:hot-swap provider / hot-reload MCP 不丢失用户授权状态。

### P1(中优先级,1-2 月内落地)

7. **MCP 一等公民**(§3):引入 `McpClient` trait(5 个方法)+ `McpToolAdapter` 把 MCP tool 包装成 kernel Tool;stdio 传输三锁分离(request / operation / reconnect)+ `connection_generation` 单调递增;trust 三级判定(read_only → server_trusted → tool_auto_approved);`mcp__{server}__{tool}` 命名 + `MAX_MCP_TOOL_NAME_LEN=64` + `sanitize_name_segment` 防止 OpenAI 400。**预期收益**:MCP 生态接入。

8. **Skill 一等公民**(§4):引入 `Skill` struct + `SkillRegistry` + `render_skill_catalog` budget gate + `SkillCatalogHook`(`session_start` 触发注入)+ `expand_for_injection`(single left-to-right pass,禁止替换值再被扫)。frontmatter 兼容 Claude-Code。**预期收益**:插件化 workflow 模板,技能市场可建立。

9. **`RateLimitDecision::WaitAndRetry / Pause` 自动等待 vs 用户决策**(§2.4):把 laew 的 429 处理升级为 `RATE_LIMIT_AUTO_WAIT_SECS = 120` 阈值,120s 内 WaitAndRetry + cancellable sleep + jitter,超过 Pause 让用户决策。`auto_resuming` 字段区分语义。**预期收益**:CodingPlan 限流 / 余额不足 / 通用 429 三种语义清晰分流。

10. **多层熔断计数器**(§2.4 TurnCtx + agent.rs:110-210):把 laew 当前单一 max_rounds 升级为多个独立计数器:`round / continuations / truncation_continuations / overflow_attempt / provider_retry / stream_retry / rate_limit_waits / empty_retries / repeat_rounds`,每个有独立上限和退避策略。`MAX_RATE_LIMIT_WAITS = 5` + `SILENT_FIRST_RATE_LIMIT_RETRY = 1s` 静默首 429。`EMPTY_RESPONSE_MAX_RETRIES = 5` 空 200 单独重试。**预期收益**:失败模式细粒度控制。

11. **`TurnCtx` 三层关联 ID**(§2.4):session_id(driver 注入)→ turn_id(kernel 单调递增,1-based within session)→ request_id(kernel 全局单调,1-based across session)→ round(每 turn reset)。确定性单调,不用 clock/random。**预期收益**:日志关联、遥测统计、debug 可重现。

12. **`Tool::risk(args) / read_only_hint / self_bounds_output / parallel_safe / always_grant_scope` 五元组**(§2.6 + §3.9):把 laew 的工具风险分类升级为这套 arg-aware 元数据:`risk(args)` 参数感知(如 `bash` 的 `rm -rf` vs `ls`)、`read_only_hint` 来自 MCP `annotations.readOnlyHint`、`self_bounds_output` 让 read_file 跳过通用截断、`parallel_safe(args)` 决定并发、`always_grant_scope(args)` 控制"总是允许"粒度。**预期收益**:approval / plan-mode / 并发 三套语义统一。

13. **`source_rank` 用 prefix 匹配**(§4.4):laew 引入 Skill 系统时**不要**用字串匹配(`.atomcode` / `.claude` / `.agents` 字串 contains)—— 必须用环境变量 `LAEW_HOME` prefix 匹配 + fallback。这是 atomcode 实战踩过的坑(重命名后自家插件被裁)。

### P2(低优先级,长远规划)

14. **CodeIntel 代码智能**(§5):把 laew 的"理解代码"能力从纯 Read/Bash 升级为 `list_symbols` / `read_symbol` / `find_references` / `trace_callers` / `trace_callees` / `blast_radius` / `file_dependencies` 七件套。基础三层:`extract_symbols` 单文件 + `build_graph` 全仓 + `CodeIndex` 缓存(`(path, mtime_ns, len)` 指纹)。先 Rust 一种语言(`tree-sitter-rust`),按需扩展。

15. **STREAMABLE HTTP/SSE MCP**(§3.7):用 `Accept: application/json, text/event-stream` + `MCP-Protocol-Version` echo + `Mcp-Session-Id` capture/replay。这套对 Playwright / Figma Dev Mode 等 stateful MCP server 必备。

16. **OAuth PKCE + 动态 client registration**(§3.8):MCP OAuth 完整流程抄 atomcode:discover well-known → bind callback listener → PKCE (SHA256 verifier + base64url challenge) → register_oauth_client(必要时)→ open_browser → await_oauth_callback → token exchange → atomic write `0o600`。

17. **`On_Text_Delta` / `On_Reasoning_Delta` transform seam**(§2.7 + agent.rs:2542-2594):让 laew 的脱敏 hook 同时进 live stream + 存储(不只改 storage),关闭 reasoning 通道泄露。每 chunk 原地变换,hook 看见前一个 hook 改动。

18. **`always_grant_scope` 返回空串做 tool-wide grant**(§3.10):MCP tool 的"总是允许"绑 tool name 而不是 args,跨不同 args 复用 grant —— 用户少点几次确认。

19. **`expand_shell_injections` 设计意图 + locale 处理**(§4.3):Skill 的 `!`cmd`` shell 注入是 trusted content 的设计意图,但要用 `apply_utf8_locale_env_sync` 处理 locale + `suppress_console_window_sync` 处理 Windows 闪窗。

20. **Graph 工具 `spawn_blocking`**(§5.5):任何 O(repo) CPU-bound 的代码智能 / 数据分析工具都必须用 `tokio::task::spawn_blocking` 包装,不能 inline 在 tokio runtime(会阻塞整个 reactor)。

---

## 附录 A:关键文件索引

| 关注点 | 路径(绝对) | 核心行号 / 函数 |
|--------|--------------|-----------------|
| Workspace 根 + Cargo 顶级契约 | `/usr/local/LsmGitOpenSource/atomcode/Cargo.toml` | line 1-50 |
| Capabilities 依赖方向契约 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/Cargo.toml` | line 17-30 + feature 注释 |
| Coding L2 装配入口 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/Cargo.toml` | line 17-33 |
| Kernel 模块边界 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/lib.rs` | line 1-29 |
| `LlmProvider` trait | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/provider.rs` | line 120-157 |
| `AgentCommand` / `AgentEvent` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/event.rs` | line 124-385 |
| `LifecycleHooks` + `HookChain` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/hook.rs` | line 183-497 |
| `ToolMiddleware` + `BeforeOutcome` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/middleware.rs` | line 34-145 |
| `Tool` trait + `ToolRegistry` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/tool.rs` | line 206-348 |
| `session_loop` / `process_send_message` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/agent.rs` | line 1418 / 1475 |
| `run_turn` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/agent.rs` | line 1850 |
| 工具三阶段执行 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/agent.rs` | line 3232-3801 |
| `StreamEvent` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-kernel/src/stream.rs` | line 90-156 |
| MCP 模块入口 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/mod.rs` | line 1-63 |
| `McpClient` trait | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/client.rs` | line 31-37 |
| `McpRegistry` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs` | line 123-1050 |
| `add_server` 流程 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs` | line 647-714 |
| `call_tool` 锁策略 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs` | line 851-886 |
| `StdioClient` 三锁分离 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/transport_stdio.rs` | line 31-73 |
| `HttpClient` HTTP/SSE | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/transport_http.rs` | line 41-88 |
| `McpTokenStore` + `login_mcp_oauth` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/oauth.rs` | line 102-370 |
| `McpToolAdapter` + 风险判定 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/tool.rs` | line 71-198 |
| `mcp_tool_full_name` + sanitize | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/tool.rs` | line 24-67 |
| `project_trust_key` 算法 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/mcp/registry.rs` | line 61-82 |
| Skill 模块入口 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/mod.rs` | line 1-53 |
| `Skill::expand` + 替换 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/skill.rs` | line 28-148 |
| `Skill::expand_shell_injections` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/skill.rs` | line 150-191 |
| `SkillRegistry::scan_skill_dir` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/registry.rs` | line 36-74 |
| `source_rank` prefix 匹配 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/render.rs` | line 72-101 |
| `render_skill_catalog_prioritizing` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/render.rs` | line 123-178 |
| `SkillCatalogHook` position reconcile | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/catalog_hook.rs` | line 23-66 |
| CodeIntel 模块入口 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/mod.rs` | line 1-217 |
| `CodeGraph` + Edge 约定 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/graph.rs` | line 14-180 |
| `build_graph` + `CodeIndex` 缓存 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/index.rs` | line 254-324 |
| `extract_calls` + caller_line 精确归属 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/index.rs` | line 50-108 |
| `classify_symbol_kind` 12 语言映射 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/index.rs` | line 18-37 |
| `resolve_callee` 评分 + deterministic 平局 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/index.rs` | line 209-252 |
| `BlastRadiusTool` 算法 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/blast_radius.rs` | line 67-113 |
| `canonical` + `display_path` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/codeintel/mod.rs` | line 177-199 |
| `prepare_with_plugin_hooks_reusing_lease` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 452-... |
| `CodingParts` 全部句柄 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 334-428 |
| `assemble` 装载 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 1507-... |
| Middleware 注册顺序 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 1700-1800 |
| Hook 注册顺序 | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 1837-1858 |
| `register_codeintel_tools` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 492 |
| `register_skill_tools` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 785 |
| `register_mcp_tools` | `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs` | line 1425 / 1472 |

## 附录 B:与 laew 的核心差异(第二轮新发现)

1. **三层 workspace + cargo feature gating 是物理边界**(不是模块注释)—— laew 当前是扁平模块,cargo tree 不能阻止意外跨层引用
2. **`AgentCommand` / `AgentEvent` 中立协议**(§2.3)—— laew 的 TUI / WebUI / daemon / 单轮 `-p` 各自维护独立控制面,缺一份中立序列化协议
3. **`MountedTools` 写时复制快照**(§2.6)—— laew 的 ToolRegistry mount 后是只读,缺 publisher + revision 机制做 hot-swap
4. **MCP 三锁分离**(§3.6)—— laew 完全没 MCP,连设计起点都没有
5. **Skill 一等公民 + catalog budget gate**(§4.5)—— laew 完全没 Skill,prompt 注入靠手工拼装,无 budget 控制
6. **CodeIntel cross-file graph + blast_radius**(§5.9)—— laew 完全没代码智能,agent 看到的就是纯文本
7. **`source_rank` prefix 匹配陷阱**(§4.4)—— atomcode 实战踩坑的隐式 lesson
8. **`on_text_delta` / `on_reasoning_delta` transform seam**(§2.7)—— laew 的脱敏如果只改 storage 会出现"live 流泄露 secret"

报告完。
