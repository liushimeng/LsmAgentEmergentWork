# atomcode 综合深度分析

> 调研对象:`/usr/local/LsmGitOpenSource/atomcode`(v5.0.9)
> 调研日期:2026-09-04 ~ 2026-09-06
> 原始文档:6 份(源码调研/深度分析/核心机制/第二轮/第三轮/第四轮)
> 总行数:~5,500 行(合并后,原始 19,875 行去重压缩)
> 定位:Rust 终端 AI 编码 Agent,TUI/WebUI/Daemon/ACP/Mobile 多端

---

## 目录

1. [项目元信息与目录结构](#1-项目元信息与目录结构)
2. [L0/L1/L2/LEAF 分层架构](#2-l0l1l2leaf-分层架构)
3. [Kernel 核心(Agent 循环/Provider/Message)](#3-kernel-核心agent-循环providermessage)
4. [协议适配(Anthropic/OpenAI wire 层)](#4-协议适配anthropicopenai-wire-层)
5. [工具系统(Trait + 注册 + 中间件)](#5-工具系统trait--注册--中间件)
6. [Hook 系统(LifecycleHooks + HookChain)](#6-hook-系统lifecyclehooks--hookchain)
7. [MCP 7 子模块](#7-mcp-7-子模块)
8. [Skill 系统](#8-skill-系统)
9. [CodeIntel 七件套](#9-codeintel-七件套)
10. [Session 管理与 Context 压缩](#10-session-管理与-context-压缩)
11. [SubAgent / Team 多角色](#11-subagent--team-多角色)
12. [Daemon 守护进程](#12-daemon-守护进程)
13. [Coding / CodingPlan 运行时](#13-coding--codingplan-运行时)
14. [TUI (tuix)](#14-tui-tuix)
15. [Auth + Config + Telemetry + Updater + Review](#15-auth--config--telemetry--updater--review)
16. [WebUI / Extensions / Evals / Docker](#16-webui--extensions--evals--docker)
17. [对 laew 的借鉴(P0/P1/P2)](#17-对-laew-的借鉴p0p1p2)
18. [关键文件清单与常量速查](#18-关键文件清单与常量速查)

---

## 1. 项目元信息与目录结构

### 1.1 元信息

| 维度 | 内容 |
| --- | --- |
| 定位 | 开源终端 AI 编码 Agent(TUI / WebUI / Daemon / ACP / Mobile),AtomGit 自托管闭源 CodingPlan 配套 |
| 语言 | **Rust 2021 edition**,1.88+;WebUI 为 TypeScript(`webui/`,构建后由二进制 embed) |
| 仓库组织 | Cargo **workspace**,根 `Cargo.toml` 用 `members = ["crates/*"]` 收集;`closed-source overlay` 通过 `crates/atomcode-codingplan-crypto` 走 `codingplan-crypto` feature 引入 |
| Crate 总数 | 14 个:`atomcode-auth`、`atomcode-capabilities`、`atomcode-cli`(主 binary `atomcode`)、`atomcode-clix`、`atomcode-coding`、`atomcode-codingplan`(闭源 placeholder)、`atomcode-codingplan-crypto`、`atomcode-config`、`atomcode-daemon`、`atomcode-kernel`、`atomcode-review`、`atomcode-telemetry`、`atomcode-tuix`、`atomcode-updater` |
| 入口点 | `crates/atomcode-cli/src/main.rs`(二进制 `atomcode`);同时 lib `src/lib.rs`(`default-run = "atomcode"`,方便 `tests/` 集成) |
| Daemon | 独立 binary `atomcode-daemon`,监听 HTTP,暴露 session history + SSE 聊天 |
| ACP | 独立子命令 `atomcode acp`,基于 `agent-client-protocol = "=2.0.0"`(feature `unstable_protocol_v2`) |
| 构建/发布 | `cargo install --path crates/atomcode-cli --locked`;`profile.release = opt-level "z" + lto + codegen-units=1 + strip + panic="abort"`(追求极致小体积) |
| License | MIT |
| 协议 | 强 serde 可序列化(`AgentCommand`/`AgentEvent` 双向),便于本地 + 跨进程(daemon/web)统一 |
| 注释量 | 极重 RustDoc(单文件动辄上千行内嵌说明),关注点:kernel 边界契约 + 失败模式 + 中间件 load-bearing 语义 |

### 1.2 目录树

```
atomcode/
├── crates/
│   ├── atomcode-kernel/       ← L0: 中性 Agent SDK(核心)
│   │   └── src/  agent.rs, provider.rs, message.rs, stream.rs,
│   │             event.rs, tool.rs, hook.rs, middleware.rs,
│   │             checkpoint.rs, clock.rs, request.rs, conformance/, testkit.rs
│   │
│   ├── atomcode-capabilities/ ← L1: 可插拔能力(核心)
│   │   └── src/  provider/(openai_compat|anthropic|ollama|atomgit_sign|reasoning|retry|sign)
│   │             tools/(bash|read|write|edit|task|todo|web_fetch|web_search|approval|...)
│   │             mcp/(client|config|registry|oauth|transport_http|transport_stdio|trust|tool)
│   │             skills/(registry|render|use_skill|catalog_hook)
│   │             codeintel/(graph|index|list_symbols|read_symbol|trace_*|lsp_tool|file_deps|blast_radius)
│   │             session/(manager|snapshot|transcript|recall|rewind|context|presentation|status_reminder)
│   │             subagent/(claude_code|codex|proc|tool)
│   │             memory/, cc_hooks.rs, datalog.rs, compaction.rs, plugin/,
│   │             team.rs, fs.rs, hooks.rs, proxy.rs, file_index.rs
│   │
│   ├── atomcode-coding/       ← L2: 编码特化(核心)
│   │   └── src/  runtime.rs, parts.rs, assemble.rs, persona.rs,
│   │             controllers.rs, plan_mode.rs, todo.rs,
│   │             discipline/verify.rs, team/(manager|runner|tool),
│   │             init_prompt.rs, subagent_tiers.rs, skill_first.rs,
│   │             next_prompt_suggestion.rs, mcp_instructions.rs,
│   │             provider_factory.rs, rate_limit.rs, vision.rs,
│   │             telemetry.rs, execution_policy.rs, plugin_hooks.rs
│   │
│   ├── atomcode-tuix/         ← TUI: 终端 UI(核心)
│   │   └── src/  event_loop/{mod,commands,bg_runtime,monitor,oauth_poll,...}.rs
│   │             render/{retained,plain,cell,interaction,worker,screen}.rs
│   │             modals/(model_picker|session_picker|diff_viewer|provider_panel|rewind|...).rs
│   │             input/, highlight/, i18n/, git/, plus
│   │             state.rs, terminal.rs, markdown.rs, commands.rs, session.rs, team.rs
│   │
│   ├── atomcode-cli/          ← 辅助:CLI 入口 + clap 子命令 + 调度器
│   ├── atomcode-daemon/       ← 辅助:HTTP/SSE 服务
│   ├── atomcode-config/       ← 辅助:配置、模型目录、settings、store
│   ├── atomcode-auth/         ← 辅助:OAuth/SSO/CodingPlan 登录
│   ├── atomcode-telemetry/    ← 辅助:匿名遥测
│   ├── atomcode-review/       ← 辅助:`code_review` 子代理实现
│   ├── atomcode-clix/         ← 辅助:CLI 实验包
│   ├── atomcode-codingplan/   ← CodingPlan 模型目录与配额窗口
│   ├── atomcode-codingplan-crypto/ ← 闭源 request signer(开源 stub)
│   └── atomcode-updater/      ← 辅助:自更新
│
├── webui/                     ← TypeScript WebUI(构建后由 atomcode-daemon embed)
├── docs/                      ← 设计文档 + plan + 报告
├── evals/                     ← 评测(对抗性 prompt 集等)
├── examples/                  ← Rust 用法示例
├── extensions/                ← 扩展
├── docker/                    ← Docker 镜像
├── scripts/                   ← 安装/构建脚本
└── Cargo.toml                 ← workspace root
```

**职责标注**:
- **核心**:kernel / capabilities(L1)/ coding(L2)/ tuix(TUI)
- **辅助**:cli / daemon / config / auth / telemetry / review / clix / codingplan / updater / webui
- **文档/支撑**:docs / evals / examples / extensions / docker / scripts

---

## 2. L0/L1/L2/LEAF 分层架构

### 2.1 分层架构总览

```mermaid
graph TD
    subgraph L3["L3 产品层 (CLI/TUI/WebUI/daemon/clix)"]
        CLI[atomcode-cli]
        TUI[atomcode-tuix]
        DAEMON[atomcode-daemon]
        CLIX[atomcode-clix]
        WEBUI[WebUI SPA]
    end
    subgraph L2["L2 业务特化层"]
        CODING[atomcode-coding]
        REVIEW[atomcode-review]
    end
    subgraph L1["L1 能力层 (feature-gated)"]
        PROV[provider]
        TOOLS[tools]
        MCP[mcp]
        SKILLS[skills]
        SESSION[session]
        MEM[memory]
        CODEINTEL[codeintel]
    end
    subgraph L0["L0 内核层"]
        KERNEL[atomcode-kernel]
    end
    subgraph LEAF["叶子依赖"]
        AUTH[atomcode-auth]
        CONFIG[atomcode-config]
        TELE[atomcode-telemetry]
        UPDATER[atomcode-updater]
    end
    CLI --> CODING
    TUI --> CODING
    DAEMON --> CODING
    CLIX --> REVIEW
    CODING --> KERNEL
    CODING --> PROV
    CODING --> TOOLS
    CODING --> MCP
    CODING --> SKILLS
    CODING --> SESSION
    CODING --> MEM
    REVIEW --> KERNEL
    PROV --> KERNEL
    TOOLS --> KERNEL
    MCP --> KERNEL
    SKILLS --> KERNEL
    SESSION --> KERNEL
    MEM --> KERNEL
    CODEINTEL --> KERNEL
    DAEMON --> AUTH
    DAEMON --> CONFIG
    DAEMON --> TELE
    CLI --> UPDATER
    CLI --> CONFIG
    TUI --> CONFIG
    CLIX --> CONFIG
    CLIX --> TELE
    AUTH --> CONFIG
    AUTH --> TELE
```

### 2.2 三层依赖方向契约

`atomcode-capabilities/Cargo.toml` 注释里写明:

```toml
# L1 capabilities layered on atomcode-kernel (L0): real provider adapters, tools, mcp, skills. Depends ONLY on the kernel.
[dependencies]
# L0 only. NEVER add atomcode-core or any L2/L3 crate here (layering is compile-enforced).
atomcode-kernel = { path = "../atomcode-kernel" }
atomcode-auth = { path = "../atomcode-auth", optional = true }
```

`atomcode-coding/Cargo.toml` 是 L2 入口:

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

### 2.3 Feature gating 边界

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

### 2.4 Cargo.toml 中对 Rust TLS / native cert 的精细处理

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

### 2.5 数据流走向(prepare → assemble → Agent)

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

---

## 3. Kernel 核心(Agent 循环/Provider/Message)

### 3.1 模块边界(kernel/src/lib.rs)

```rust
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

### 3.2 关键事实:`Agent` 是 struct,不是 trait

kernel 中**不存在 `Agent` trait**——`Agent` 是一个具体的 struct,通过 `AgentBuilder` 流水线式组装:

```rust
// crates/atomcode-kernel/src/agent.rs:794-895
pub struct Agent {
    provider: Arc<dyn LlmProvider>,
    tools: MountedTools,
    persona: String,
    middlewares: Vec<Arc<dyn ToolMiddleware>>,
    hooks: Arc<dyn LifecycleHooks>,
    max_rounds: Option<u32>,
    max_provider_retries: u32,
    tool_loop_policy: Option<ToolLoopPolicy>,
    max_continuations: Option<u32>,
    resume: Option<SessionSnapshot>,
    max_tool_result_bytes: usize,        // 默认 64KiB
    max_parallel_tools: Option<usize>,
    compaction: Arc<dyn CompactionStrategy>,
    compact_threshold: Option<f32>,
    compaction_checkpoint: Option<Arc<dyn CompactionCheckpoint>>,
    stream_timeout: Option<Duration>,
    request_timeout: Option<Duration>,
    chat_options: ChatOptions,
    working_dir: Option<PathBuf>,
    shared_cwd: Option<Arc<RwLock<PathBuf>>>,
    cancel_token: Option<CancellationToken>,
    session_id: Option<Arc<str>>,
    clock: Arc<dyn Clock>,
    keep_interrupted_context: bool,
    round_cap_checkpoint: bool,
}
```

### 3.3 协议中立性:`LlmProvider` trait

```rust
// crates/atomcode-kernel/src/provider.rs:120-157
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
- 唯一契约是 `chat_stream` —— 流式为唯一协议(注释明示 `SLOT, not POLICY` —— kernel 是 mechanism,adapter 负责 policy)
- `bind_session_id` 是一次性绑定(`OnceLock` 实现)—— kernel 在 spawn 时单点注入,驱动层无需手传 session id
- `ChatOptions` 中立携带 `reasoning_effort` / `tool_choice` / `temperature` / `max_tokens` —— adapter 自己映射到 OpenAI / Anthropic / Ollama 的 wire 格式
- `RateLimitRetryOwner` 标注运行时谁拥有 429 重试(`#[serde(skip)]` 不持久化)—— 直接 provider 调用者保留默认;kernel turn loop override,让 wait 可被 cancel

**`ChatOptions` 中性请求拨杆**:

```rust
pub struct ChatOptions {
    pub reasoning_effort: Option<ReasoningEffort>,  // Low/Medium/High/XHigh/Max
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub tool_choice: ToolChoice,  // Auto/Required/Specific(String)/None
    pub rate_limit_retry_owner: RateLimitRetryOwner,
}
```

### 3.4 `AgentCommand` / `AgentEvent` 双向消息协议

这是 kernel **对外协议的中立骨架**,从 TUI / daemon / ACP / WebUI 都可以套用同一份 wire format。

**`AgentCommand`(driver → agent)**:

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

**`AgentEvent`(agent → driver)** 涵盖所有 driver 需要的信号:

```rust
#[non_exhaustive]
pub enum AgentEvent {
    TurnStarted, TextDelta(String),
    ToolCallStreaming { index, id, name, arguments },
    ToolBatchStarted { batch_id, calls },
    ToolBatchCompleted { batch_id, ok, total, elapsed_ms },
    ToolStarted { call }, ToolProgress { call_id, message },
    ToolResult { result },
    PolicyIntervention { intervention },
    Request { id, kind, payload },
    Usage(MessageMeta),
    Snapshot { snapshot },
    TurnComplete { reason: StopReason },
    Error { message, http_status, code, retryable },
    Cancelled, Reasoning(String), Warning(String),
    StreamRecovery { attempt, max_attempts, recovered },
    ProviderRetry { attempt, max_attempts, backoff_secs, reason },
    OutputTruncationRecovery { attempt, max_attempts },
    RateLimited { reset_at_display, reset_label, secs_until_reset, auto_resuming, server_message },
    Steered { count, inputs },
    CompactionStarted { trigger },
    Compacted { trigger, epoch, removed, bytes_before, bytes_after, committed, snapshot },
    CompactionFailed { trigger, error },
}
```

**协议兼容性三大武器**:
1. `#[non_exhaustive]` —— 新 variant 不破下游 `match`
2. `#[serde(default)]` —— 老 wire 缺字段仍可反序列化(`SendMessage {text}` → 无 images)
3. `#[serde(default, skip_serializing_if = "Option::is_none")]` —— 可选字段不强制序列化

**`StopReason` 是"失败感知"核心**:

```rust
/// WHY a turn ended (FAILURE PERCEPTION). Carried by the terminal
/// `AgentEvent::TurnComplete { reason }` and aggregated into `Outcome::stop`, so a
/// driver (TUI / SWE-bench grader / CI) can ALWAYS tell a clean stop from a
/// failure — a failed turn can never look like an empty SUCCESS.
```

`Outcome::default()` 是 `Stopped`(line 90 `#[default] Stopped`),保证 Default 编译过;`Outcome.error` 是 LAST Error,"a failed open/mid-stream/timeout/fuse yields `Outcome { stop: ProviderError, error: Some(..) }`, not an empty `Outcome::default()` masquerading as success"。

### 3.5 Agent 主循环状态机

入口在 `RunningAgent::run_turn()`(agent.rs:1850),是一个复杂的 `loop`,内含多层计数器/融合器:

**轮次循环**:`loop { round += 1; ... }`

**关键计数器**(全部在 run_turn 局部声明):
- `round: u32` — 当前轮次(1-based)
- `continuations: u32` — `offer_continuation` 续接次数,上限 `max_continuations`(默认 50)
- `truncation_continuations: u32` — 输出截断自动续接,上限 `MAX_TRUNCATION_CONTINUATIONS`(4)
- `overflow_attempt: u8` — 上下文溢出恢复尝试,上限 `MAX_OVERFLOW_ATTEMPTS`(3)
- `provider_retry: u32` — 瞬态 provider 重试,上限 `DEFAULT_MAX_PROVIDER_RETRIES`(3)
- `stream_retry: u32` — 流中断重连,上限 `MAX_STREAM_RETRIES`(5)
- `rate_limit_waits: u32` — 429 限流等待,上限 `MAX_RATE_LIMIT_WAITS`(5)
- `empty_retries: u32` — 空响应重试,上限 `EMPTY_RESPONSE_MAX_RETRIES`(5)
- `repeat_rounds: u32` — 粗粒度重复熔断,`REPEAT_NUDGE_AT`(3)时警告,`MAX_REPEAT_ROUNDS`(6)时停止

**每轮处理流程**:
1. 检查 `round_cap`(可选的交互式 checkpoint)
2. 排空 `steer` 缓冲区(用户中途输入)
3. 克隆消息 → `hooks.pre_request()` 投影(ephemeral,不污染缓存)
4. `hooks.pre_request_options()` 设置请求选项
5. 构建 `ToolDef[]` → `provider.chat_stream()` 发起请求
6. 流式消费 `StreamEvent`,分发到 `on_text_delta` / `on_reasoning_delta` / `on_model_response`
7. 工具调用三阶段:① Classify → ② Execute(并发) → ③ Apply
8. 检查续接/截断/溢出/空响应,决定是否继续循环

### 3.6 流式输出实现

```rust
// crates/atomcode-kernel/src/stream.rs:90-156
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

**`TokenUsage::merge_max` 字段级 max**(注释):
```rust
/// Usage 合并: Anthropic 风格(split report)vs OpenAI 风格(单次 cumulative)的差异
/// 字段级 MAX,不会 double-count cumulative delta
pub fn merge_max(&mut self, other: TokenUsage) { ... }
```

**`on_text_delta` / `on_reasoning_delta` 是 transform seam**:每 chunk 原地变换(agent.rs:2542-2594),保证流式 + 存储一致(脱敏同时进 live stream + storage,关闭 reasoning 通道泄露)。

### 3.7 用户中断 / Steering

- **Steer 机制**(`agent.rs:725-733`):用户在 turn 中提交的 `SendMessage` 不排队成新 turn,而是写入 `SteerBuf`,在**下一轮 round 边界**折叠进当前 turn(`agent.rs:2017-2047`)
  - 真实 steer 会重置 tool-loop 状态、清除 repeat 检测
- **Cancel**:`CancellationToken` 协作式取消
  - per-turn token 由 `new_turn_token` 铸造(`agent.rs:1096-1101`),外部 cancel token 的 child
  - 在 stream 消费(`agent.rs:2412-2420`)、工具执行(`agent.rs:3619-3628`)、sleep 等待处均 `biased` 轮询 cancel
  - `keep_interrupted_context`(`agent.rs:889-892`)控制 cancel 是 UNDO(回滚到 `rollback_len`)还是保留部分工作
- **Shutdown**:`agent.rs:1559-1571` 区分 `internal_cancel` + `turn_token.cancel()`,等 turn 正常终结,避免跳过 `finish_cancelled / turn_complete`

### 3.8 多轮对话实现

atomcode 没有独立的 REPL 屏,多轮对话由 **kernel 的 session loop** 与 **driver 的命令队列** 协同实现:

- **session loop 入口**:`atomcode-kernel/src/agent.rs:1418-1468` `RunningAgent::session_loop`
  - 持有 `cmd_rx: UnboundedReceiver<AgentCommand>`,循环接收 `SendMessage / SendSyntheticMessage / Cancel / Shutdown / Snapshot / Compact` 等命令
  - 每个真实用户消息走 `process_send_message`(`agent.rs:1475`),synthetic 消息(FIFO 队列)在 turn 边界排空
- **driver 侧**:`atomcode-coding/src/runtime.rs` 的 `CodingRuntimeHandle` 暴露 `submit() / steer() / cancel()` 命令,经 channel 灌入 kernel
- **TUI 前端**:`atomcode-cli/src/tui/` 通过 crossterm 原始模式读键,整串命令发给 runtime

**Turn 模型**:Turn 是 kernel 的原子执行单元,**一个 user message = 一个 turn**,内部可含多轮 LLM 调用(round):

- **TurnCtx**(`atomcode-kernel/src/hook.rs:19-47`):携带 `session_id / turn_id / request_id / round / max_rounds / cache_epoch / context_window / used_tokens`
  - `turn_id` 单调递增(`agent.rs:1076-1078 turn_counter`),`request_id` 全局唯一
  - 注释明示"deterministic — NOT clock/random — so log stitching stays reproducible"

### 3.9 安全熔断常量

```rust
// crates/atomcode-kernel/src/agent.rs
const DEFAULT_MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;  // 64KiB
const MAX_OVERFLOW_ATTEMPTS: u8 = 3;
const DEFAULT_MAX_PROVIDER_RETRIES: u32 = 3;
const MAX_STREAM_RETRIES: u32 = 5;
const MAX_PARTIAL_STREAM_RECOVERIES: u32 = 1;
const MAX_RATE_LIMIT_WAITS: u32 = 5;
const SILENT_FIRST_RATE_LIMIT_RETRY: Duration = Duration::from_secs(1);
const EMPTY_RESPONSE_MAX_RETRIES: u32 = 5;
const MAX_TRUNCATION_CONTINUATIONS: u32 = 4;
const MAX_REPEAT_ROUNDS: u32 = 6;
```

---

## 4. 协议适配(Anthropic/OpenAI wire 层)

### 4.1 Anthropic Messages 适配

```rust
// crates/atomcode-capabilities/src/provider/anthropic.rs:43-151
pub struct AnthropicConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub supports_vision: bool,
    pub context_window: u32,
    pub max_tokens: u32,  // REQUIRED
    pub anthropic_version: String,  // "2023-06-01"
    pub thinking: bool,
    pub send_sampling_params: bool,
    pub idle_timeout: Duration,
    pub connect_timeout: Duration,
    pub open_timeout: Duration,
    pub retry: RetryPolicy,
    pub user_agent: Option<String>,
    pub skip_tls_verify: bool,
}
```

**请求构造**:
- Auth:`x-api-key` + `anthropic-version` 头(非 Bearer)
- URL:`{base_url}/v1/messages`
- `system` 是顶层字段(非 message role)
- `max_tokens` 必填
- tool `input_schema`(非 `function.parameters`)
- `tool_use.input` 是 JSON 对象(非字符串)

**流式解码**:
- 事件类型 SSE:`message_start` / `content_block_*` / `message_delta` / `message_stop`
- thinking 块带 opaque `signature`,必须原样回显
- `input_json_delta` 片段按 content-block index 缓冲,在 `content_block_stop` 发射完整 `ToolCall`

### 4.2 OpenAI 兼容适配

```rust
// crates/atomcode-capabilities/src/provider/openai_compat.rs:111-280
pub struct OpenAiCompatConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub supports_vision: bool,
    pub context_window: u32,
    pub max_tokens: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub tool_choice: ToolChoice,
    pub temperature: Option<f32>,
    pub idle_timeout: Duration,
    pub connect_timeout: Duration,
    pub open_timeout: Duration,
    pub retry: RetryPolicy,
    pub request_signer: Option<Arc<dyn RequestSigner>>,
    pub user_agent: Option<String>,
    pub skip_tls_verify: bool,
}
```

**请求构造**:
- Auth:`Authorization: Bearer {api_key}`
- URL:`{base_url}/chat/completions`
- `reasoning_effort` 映射:`low` / `medium` / `high` / `xhigh` / `max`
- OpenRouter 归因头:`HTTP-Referer` / `X-OpenRouter-Title` / `X-OpenRouter-Categories`

**流式解码**:
- choice-delta chunks
- `tool_calls[]` 按 index 缓冲,在 `finish_reason == "tool_calls"` 发射完整 `ToolCall`
- usage 最后 wins,发射单个 `Usage` 事件

### 4.3 错误映射与重试

```rust
// crates/atomcode-capabilities/src/provider/retry.rs
pub struct RetryPolicy {
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub backoff_multiplier: f64,
}
pub enum ProviderError { Retryable { ... }, NonRetryable { ... } }
```

**重试分层**:
1. **Provider 层**:OPEN 失败快速回退(~1.5s),最多 3 次
2. **Kernel 层**:`max_provider_retries` = 3(瞬态失败)
3. **Stream 层**:`MAX_STREAM_RETRIES` = 5(mid-stream 重连)
4. **429 层**:`MAX_RATE_LIMIT_WAITS` = 5,首次静默重试

### 4.4 Wire Dump 字节级调试

```rust
// crates/atomcode-capabilities/src/provider/mod.rs
pub fn wire_dump_request(model: &str, body: &Value) {
    if std::env::var("ATOMCODE_WIRE_DUMP").is_ok() {
        // 写入 config_dir()/wire-dump/<model>-<timestamp>.json
    }
}
```

---

## 5. 工具系统(Trait + 注册 + 中间件)

### 5.1 `Tool` trait

```rust
// crates/atomcode-kernel/src/tool.rs:206-272
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

**信任模型**(模块级文档明确):
- `RiskLevel` 是**建议元数据**,不是执行边界
- 内核**不沙箱化**工具——工具获得宿主进程完整权限
- 唯一内置安全机制:`max_tool_result_bytes`(默认 64KiB)
- OS 级隔离是**嵌入者**的责任

### 5.2 `ToolRegistry` + `MountedTools` 二阶段

```rust
// crates/atomcode-kernel/src/tool.rs:277-418
pub struct ToolRegistry { tools: Arc<RwLock<BTreeMap<String, Arc<dyn Tool>>>> }
impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) { ... }
    pub fn mount(&self, names: &[&str]) -> MountedTools { ... }
    pub fn mount_updatable(&self, names: &[&str]) -> (MountedTools, MountedToolsPublisher) { ... }
}
pub struct MountedTools { current: Arc<RwLock<Arc<MountedToolsSnapshot>>> }
impl MountedToolsPublisher {
    pub fn publish(&self, registry: &ToolRegistry, names: &[&str]) -> ToolCatalogRevision { ... }
}
```

**设计要点**:
- `BTreeMap` 保证确定性 prompt 排序
- `MountedToolsSnapshot` 不可变,发布新版本时原子替换
- 同一 turn 内 model request 与 tool execution 看到相同工具集

**WHY 这种二阶段**:注释钉死 —— "Holds *all* available tools. Clones share the same registry so a runtime-owned background capability reconciler can register tools before publishing a new mounted snapshot" —— MCP 在后台初始化完后原子 publish 新快照,agent 下一轮拿到的工具集自动更新。

### 5.3 工具三阶段执行(Phase ①②③)

- **位置**:`agent.rs:3232-3801`
- **Phase ① CLASSIFY**(按序):
  - 重复检测:`result_ids`(mode A,同 id) / `seen_calls`(mode B,同 name+args)
  - 截断保护:`finish_reason=length` 时所有 call 返回 coach 结果
  - middleware `before` 链:`Proceed / Ask / Allow / Deny / DenyTurn / DenyTurnWithIntervention`
  - 产出 `Vec<CallPlan>`:`Execute / Skip / Result`
- **Phase ② EXECUTE**(并发):
  - `FuturesOrdered` + `RwLock` gate + `Semaphore` cap
  - `parallel_safe` 工具拿 read lock(可并发),副作用工具拿 write lock(独占)
  - cap:`ATOMCODE_MAX_PARALLEL_TOOLS`(默认 4,`clamp(1, MAX_PARALLEL_TOOLS_CEILING)`)
  - 每个 future 内 `biased` 轮询 cancel
- **Phase ③ APPLY**(按序):
  - middleware `after` 链
  - `cap_tool_result` 截断
  - 收集 loop fingerprint(用于 loop 检测)
  - vision 图片收割 → 附加到 follow-up user message
  - emit `ToolResult` + push `Message::tool_result`

### 5.4 关键工具清单

| 工具 | 文件 | 风险 | 特性 |
|------|------|------|------|
| bash | `tools/bash.rs` | arg-aware | AST 只读分类(tree-sitter-bash)、Job Object 进程树 |
| read_file | `tools/read.rs` | Safe | 自截断 + 行号分页 |
| write_file | `tools/write.rs` | Risky | 需审批 |
| edit_file | `tools/edit.rs` | Risky | 行级 diff |
| list_dir | `tools/list.rs` | Safe | - |
| grep | `tools/grep.rs` | Safe | 流式搜索 + ignore |
| glob | `tools/glob.rs` | Safe | - |
| task | `tools/task.rs` | Risky | SubAgent 驱动 |
| web_fetch | `tools/web_fetch.rs` | Risky | SSRF DNS 检查 |
| web_search | `tools/web_search.rs` | Risky | - |
| use_skill | `skills/use_skill.rs` | Safe | - |
| list_skills | `skills/render.rs` | Safe | - |
| mcp__*__* | `mcp/tool.rs` | 来自 server annotations | 工具适配 |

---

## 6. Hook 系统(LifecycleHooks + HookChain)

### 6.1 `LifecycleHooks` trait

```rust
// crates/atomcode-kernel/src/hook.rs:183-322
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

**永久 vs ephemeral 区分**(注释强调):
- **`session_start` / `turn_start` / `on_model_response`** —— PERMANENT(写入历史)
- **`pre_request` / `on_text_delta` / `on_reasoning_delta`** —— EPHEMERAL(操作克隆上,不污染 prefix cache)
- **`on_request`** —— 只读观察(`&` 不能改 wire)

### 6.2 `HookChain` 组合契约

```rust
/// Composes MANY `LifecycleHooks` into one by FANNING OUT each method over an
/// ordered list. This is the seam that lets independent capabilities coexist —
/// codeintel + compaction + redaction can each register a hook instead of fighting
/// over a single slot. A `HookChain` itself implements `LifecycleHooks`, so the
/// Agent still holds exactly one `Arc<dyn LifecycleHooks>` and every call site in
/// the run loop stays unchanged.
```

**`HookChain` 组合契约**(hook.rs:336-365 注释里逐 method 钉死):
- `session_start` / `turn_start`:全部按注册顺序跑(后续 hook 看见前面 hook 的改动)
- `user_prompt_submit`:第一个 `Err` 短路阻断
- `offer_continuation`:**first Some wins**,后续 Some 忽略(loop 一次只注入一条续接)
- `turn_complete` / `on_error` / `session_end`:全部按顺序观察

### 6.3 `ToolMiddleware` + `BeforeOutcome` / `AfterOutcome`

```rust
// crates/atomcode-kernel/src/middleware.rs:34-103
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

**为什么 BEFORE 链注册顺序是契约**(注释反复强调):

> "ORDERING IS LOAD-BEARING. Middlewares run in REGISTRATION ORDER: the `before` chain forward (the first-registered runs first; the first to `Err` blocks and stops the chain), then the `after` chain (also in registration order). This is a documented contract, not an accident of iteration. **Any normalization or repair that changes the arguments which will execute MUST run before every observer, policy gate, and user approval, so all of them inspect the exact bytes the tool receives**."

**`Panic Contract`**:kernel 不隔离 panic(`panic = "abort"` 下 `catch_unwind` 没用),所以 middleware 必须不 panic —— 与 tool sandbox contract 同级 —— 阻断必须 return `BeforeOutcome::Deny`。

### 6.4 `TurnCtx` 关联 ID

- `session_id`:driver 注入(kernel 不 mint)
- `turn_id`:kernel 单调递增(确定性,不是 clock/random)—— 1-based within session
- `request_id`:kernel 全局单调 —— 1-based across session
- `round`:当前 turn 内的轮次(每 turn reset)
- `max_rounds` / `cache_epoch` / `context_window` / `used_tokens`

**WHY deterministic**:注释钉死 —— "deterministic — NOT clock/random — so log stitching stays reproducible"。

---

## 7. MCP 7 子模块

### 7.1 模块切分

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

### 7.2 `McpClient` trait

```rust
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

### 7.3 `McpRegistry` —— 多 server + trust + auto-approve 集中管理

```rust
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

### 7.4 Stdio 传输 —— 三锁分离

```rust
pub struct StdioClient {
    /// Serialize request/response round-trips.
    request_lock: Arc<Mutex<()>>,
    /// Keeps an operation's request, recovery decision, and optional retry in
    /// one critical section.
    operation_lock: Arc<Mutex<()>>,
    /// Serializes teardown + respawn. Concurrent callers that observe the same
    /// dead pipe share one reconnect instead of spawning duplicate servers.
    reconnect_lock: Arc<Mutex<()>>,
    /// Wakes operations that arrived while an uncertain tool call's transport
    /// was being rebuilt in the background.
    recovery_notify: Arc<Notify>,
    recovery_in_progress: Arc<AtomicBool>,
    connection_generation: Arc<AtomicU64>,
}
```

| 锁 | 关注点 | 持有者 |
|----|-------|--------|
| `request_lock` | 序列化 req/rsp 字节流(一问一答严格对应) | 每次 call_tool |
| `operation_lock` | 保活 + 恢复决策同处一临界区(防竞态) | 长时间操作 |
| `reconnect_lock` | teardown + respawn 序列化(并发观察者共享一次重连) | 后台重连 |

**`connection_generation` 单调递增**:等待者重连后比较自己的 generation,发现已被其他 caller 修复就直接放行。

### 7.5 HTTP/SSE 传输 —— streamable HTTP

```rust
const MCP_HTTP_ACCEPT: &str = "application/json, text/event-stream";
const MCP_PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";

pub struct HttpClient {
    server_name: String,
    url: String,
    headers: BTreeMap<String, String>,
    auth: Option<McpHttpAuthConfig>,
    timeout_ms: u64,
    status: Arc<Mutex<ServerStatus>>,
    next_id: AtomicU64,
    client: reqwest::Client,
    session_id: Arc<Mutex<Option<String>>>,
    negotiated_version: Arc<Mutex<Option<String>>>,
}
```

**HTTP 传输关键约束**:
- `Accept` 必须同时包含 `application/json` 和 `text/event-stream`
- `MCP-Protocol-Version` 在 `initialize` 之后必须 echo 服务端 negotiate 的版本
- Stateful server 返回的 `Mcp-Session-Id` 必须每次都 echo

### 7.6 Trust 模型

**3 级 trust 判定**:
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

### 7.7 `mcp_tool_full_name` + 命名规范化

```rust
/// Replace every character that OpenAI/litellm forbids in a `function.name`
/// (anything outside `[a-zA-Z0-9_-]`) with `-`. Real MCP servers routinely
/// declare server/tool names containing spaces, dots, colons or CJK characters;
/// unsanitized they break the whole request with a 400
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

**`always_grant_scope` 返回空串** —— 让"总是允许"绑到 tool name 而不是 args,跨不同 args 复用 grant。

### 7.8 OAuth 登录 + Refresh

`McpTokenStore` 用 TOML 存 `~/.atomcode/mcp_auth.toml`,`fs::atomic_write` + `0o600` 写入权限控制。

完整流程:
1. **discover_oauth_metadata** 探测 well-known endpoints
2. **bind_callback_listener** 起本地 TCP listener 收回调
3. **PKCE 流程**:state (uuid) + verifier (双 uuid 拼接) + challenge (SHA256 → base64url)
4. **register_oauth_client** 必要时动态注册 client
5. 构造 authorize_url,`open_browser()` 唤起系统浏览器
6. **await_oauth_callback** 阻塞等待回调(含 state 校验防 CSRF)
7. token exchange 后构造 `McpOAuthToken`

---

## 8. Skill 系统

### 8.1 模块切分

| 子模块 | 行数 | 职责 |
|--------|------|------|
| `mod.rs` | 53 | 模块入口 + `register_skill_tools()` |
| `registry.rs` | 571 | `SkillRegistry` + scan + get + render_catalog |
| `render.rs` | 401 | 渲染 catalog + source_rank + budget gate |
| `skill.rs` | 519 | `Skill` + frontmatter + expand + shell injection |
| `use_skill.rs` | 316 | `UseSkillTool` + `ListSkillsTool` |
| `catalog_hook.rs` | 128 | `SkillCatalogHook` (注入到 session_start) |

### 8.2 Skill 定义格式

```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub template: String,
    /// Tools the specialization MAY auto-approve while this skill is active
    pub allowed_tools: Vec<String>,
    /// If false (`user-invocable: false`), hidden from the `/` menu
    pub user_invocable: bool,
    pub skill_dir: PathBuf,
    pub source_path: PathBuf,
}
```

**两种文件形态**:
- **flat `*.md`** —— 仅 depth=0;`name = file stem`;内容 = frontmatter + template
- **dir `<dir>/SKILL.md`** —— `name = directory name`;可捆绑 `scripts/ / references/`

**frontmatter 字段**:
- `name`:`description` 默认取 template 首段
- `allowed-tools`:空格/逗号分隔的工具列表(metadata)
- `user-invocable: false`:隐藏于 `/` 菜单,模型仍可自动调用

**命名空间**:插件技能注册为 `<namespace>:<skill-name>`,允许 `~/.claude/skills` 和 `~/.atomcode/skills` 同名 skill 共存而不冲突。

### 8.3 Skill 展开引擎

```rust
pub fn expand(&self, arguments: &str, session_id: &str) -> String {
    // SINGLE left-to-right pass: each substitution's value is emitted literally and
    // never re-scanned — so an argument that itself contains `$1` is NOT re-expanded.
    // ...
}
```

**支持的 substitution 语法**:
- `$ARGUMENTS[N]` / `$N` —— 位置参数
- `$ARGUMENTS` —— 全部参数(追加若模板没出现)
- `${CLAUDE_SESSION_ID}` / `${CLAUDE_SKILL_DIR}` —— 内置变量
- `` !`cmd` `` —— shell 注入(`sh -c` 执行后取 stdout)

**关键设计点**:
- **Single left-to-right pass**:替换值不再被重扫 —— 防止 `$1` 类递归展开
- **Longest-token-first**:最长匹配优先
- **DEFINED positional indices only**:`$99` 当 positional 不到 99 时保留字面量

**WHY shell 注入是设计意图**(模块注释):
> "skills are TRUSTED, user-authored content (the same trust as a slash command the user installed), so this is by design, not arbitrary remote code."

### 8.4 use_skill / list_skills 工具

- **UseSkillTool**:`execute`:查找 skill → `expand_for_injection` → 返回内容
- **ListSkillsTool**:列出所有 skill
- **SkillCatalogHook**:`session_start` 时注入 `=== AVAILABLE SKILLS ===` 目录到 system prompt

---

## 9. CodeIntel 七件套

### 9.1 核心能力

| 工具 | 职责 |
|------|------|
| `list_symbols` | tree-sitter 提取当前文件符号 |
| `read_symbol` | 读取指定符号定义 |
| `find_references` | 查找符号引用 |
| `trace_callers` | 追踪调用者 |
| `trace_callees` | 追踪被调用者 |
| `trace_chain` | 追踪调用链 |
| `file_deps` | 文件依赖分析 |
| `blast_radius` | 变更影响面分析 |
| `lsp_tool` | 本地语言服务器(rust-analyzer 等) |

### 9.2 技术选型

- **tree-sitter** 12 种语言 grammar:Rust / Python / TypeScript / JavaScript / Go / Java / C / C++ / C# / Ruby / PHP / Swift
- **feature-gated**:`codeintel` feature 默认开启,但 pulls 12 tree-sitter grammar crates(heavy C compilation),embedder 可 opt-out

---

## 10. Session 管理与 Context 压缩

### 10.1 消息历史持久化

```rust
pub struct Conversation {
    messages: Vec<Message>,
    cache_epoch: u64,  // prefix 缓存分界标记
}
pub struct SessionSnapshot {
    pub version: u32,
    pub messages: Vec<Message>,
    pub cache_epoch: u64,
    pub turn_counter: u64,
    pub request_counter: u64,
    // ...
}
```

**Snapshot 持久化**:`atomcode-capabilities/src/session/` 提供 `SessionManager`,`SnapshotHook`(`parts.rs:936-944`)在 `turn_complete` 时落盘 `.snapshot + .meta + .jsonl`

### 10.2 压缩触发条件

- **任务边界自动触发**(`agent.rs:1102-1128 should_compact`):在 `process_send_message` 中,新 user message 入历史**之前**、turn 运行**之前**触发
  - 读取最近 assistant turn 的 `meta.used_tokens`,按**当前模型窗口**重新计算利用率(避免换小窗口模型时误判)
- **紧急溢出触发**(`agent.rs:2060-2106`):pre-send 估算 `est(messages) >= limit` 时,在 round 内压缩(有 `MAX_OVERFLOW_ATTEMPTS` 上限)
- **硬溢出恢复**(`agent.rs:2184-2205`):provider 返回 context-overflow 错误时,压缩后重试同一 round(`round -= 1`)
- **手动 `/compact`**:mid-turn 的 compact 命令被排队到 `pending`,在 turn 边界执行(`agent.rs:1612-1620`),避免破坏 within-turn cache

### 10.3 分层摘要(CompactionStrategy)

```rust
pub trait CompactionStrategy: Send + Sync {
    fn plan(&self, view: &CompactionView) -> CompactionPlan;
    fn will_summarize(&self) -> bool { true }
}
pub struct CompactionView {
    pub messages: &[Message],
    pub trigger: CompactTrigger,
    pub ctx_window: u32,
    pub used_tokens: u32,
    pub utilization: f32,
    pub sacred_floor: usize,
}
pub struct CompactionPlan {
    pub drain_from: usize,
    pub drain_to: usize,
    pub summary: String,
    pub rewrites: Vec<(usize, Message)>,
    pub resume_note: Option<String>,
}
```

**sacred_floor**(`message.rs:550-567 Conversation::sacred_floor`):保护前缀 = 首部 System + 第一个非 synthetic User,永不被 drain

**net-loss guard**(`message.rs:615-762 prepare_plan`):候选 messages 的 wire bytes 必须**严格小于**原 bytes 才 commit,否则拒绝(不 bump epoch)

**cache_epoch**:committed compaction 时 `cache_epoch += 1`(`message.rs:737`),refused 时不变,保证 prefix cache 一致性

**压力释放**:commit 后按 `bytes_after/bytes_before` 缩放最近 assistant 的 `used_tokens / utilization`,避免立即再次触发

**默认策略**:`NoCompaction`,embedder 必须显式注入

### 10.4 工具结果截断

- **kernel 级 cap**:`cap_tool_result(&mut result, self.max_tool_result_bytes)`(`agent.rs:3727`),默认 `DEFAULT_MAX_TOOL_RESULT_BYTES`
  - 在 after-chain 之后、push+emit 之前应用,保证 history / model / driver 看到一致
- **self_bounds_output**:`Tool::self_bounds_output()`(tool.rs:227-237)声明工具自约束输出,跳过通用截断(如 `read_file`)
- **stub tool results**:超大工具结果可被替换为 stub

### 10.5 二级持久化

- **`<id>.snapshot`** —— 压缩快照,供 RESUME
- **`<id>.jsonl`** —— append-only transcript,供 RECALL
- **`<id>.meta`** —— 用于 `/resume` 选择列表的快速 metadata
- **rewind** —— 事务化(`<id>.rewind.json`),turn-by-turn 回滚
- **recall** —— 跨 session 检索

---

## 11. SubAgent / Team 多角色

### 11.1 SubAgent:`TaskTool` + 分层

```rust
pub struct TaskTool { ... }
impl Tool for TaskTool {
    fn name(&self) -> &str { "task" }
    async fn execute(&self, args: &str, ctx: &ToolContext) -> ToolResult {
        // 1. 解析 task 描述
        // 2. 创建子 Agent(可能用不同 model)
        // 3. 运行 run_to_completion
        // 4. 返回结果
    }
}
```

**SubAgent 分层**:
```rust
pub fn resolve_tier_keys(config: &Config, host_model: &str) -> Option<(String, String)> {
    // host 必须参与 + ≥2 参与者
    // fast = 最低排名,capable = 最高排名
}
```

**外部子代理**:`subagent/(claude_code|codex|proc|tool)`,驱动 Claude Code / Codex 作为命名 `subagent_<name>` 工具

### 11.2 Team 多角色架构

- **TeamRole**:多种角色(如 `explorer / implementer / reviewer`),各有独立 persona、工具集、权限
  - `TeamRoleId` + `TeamRoleProfile`(`capabilities/src/team.rs`)
- **TeamTaskSpec**(`capabilities/src/team.rs`):`description / prompt / role / permission / difficulty / scope`
- **TeamRunner**(`runner.rs`):编排多角色并行/串行执行
  - `TeamProviderFactory`:`Fn(TeamDifficulty) -> Arc<dyn LlmProvider>`(按难度选模型)
- **TeamManager**(`manager.rs`):管理 team 生命周期、任务分配
- **team tool**(`team/tool.rs`):暴露 `team_*` 工具给主 agent

### 11.3 Goal 模式(单 agent)

- **GoalState**(`runtime.rs`):用户可设定一个 `condition`(目标完成条件)
- **GoalPhase**:`Pursuing / Paused / PausedAtCap / Satisfied / Ended`
- **GoalTerminal**:`Met / Stopped / Failed / Cancelled`
- **goal continuation**:turn 结束时,若 goal 未达成,注入 `goal_continuation_message`继续自主工作
- **evaluate_goal**:每轮 goal 结束后,用独立 LLM 调用判定目标是否达成
  - `temperature: 0.0`,`tool_choice: None`
  - 返回 `Verdict: yes/no`

### 11.4 TeamDifficulty 枚举

- `TeamDifficulty::Simple` → 映射到 `fast-model`
- `TeamDifficulty::Hard` → 映射到 `capable-model`
- **无 Medium 档**:只有两档(对比 laew 的三档)

---

## 12. Daemon 守护进程

### 12.1 双形态架构:独立二进制 + 进程内嵌入

daemon 的核心设计是**一套代码、两种启动方式**:

- **独立二进制** `atomcode-daemon`:由 VS Code 扩展或用户直接启动,完整启动横幅,写 `~/.atomcode/daemon-<port>.json` token 文件。
- **进程内嵌入**:TUI 的 `/webui` 命令通过 `run_server` 在进程内启动,quiet 模式,不写 token 文件。

```rust
// crates/atomcode-daemon/src/main.rs:129-192
async fn main() {
    atomcode_config::distribution::bootstrap_home();
    let (host, port, cli_override, idle_timeout_secs, startup_mode) = parse_daemon_args();
    let token_store = atomcode_daemon::auth_token::WebuiTokenStore::new();
    let daemon_token = atomcode_daemon::resolve_daemon_token(
        std::env::var("ATOMCODE_DAEMON_TOKEN").ok(), &token_store);
    run_server(ServerOpts { host, port, cli_override, idle_timeout_secs,
        startup_mode, webui_tokens: Some(token_store), quiet: false,
        working_dir_override: None, prebound_listener: None,
        app_user_id: None, daemon_token_file: Some(daemon_token) }).await?;
}
```

### 12.2 `run_server` 启动序列(14 步)

`run_server` 是 daemon 的核心入口(`lib.rs:6141`):

```rust
pub async fn run_server(opts: ServerOpts) -> anyhow::Result<()> {
    // Step 1: 加载 config(容错,失败回退默认)
    let startup_config = match Config::load(&Config::default_path()) { ... };
    // Step 2: 解析遥测状态
    let resolved = resolve(&cfg_telemetry, &cli_override, ...);
    // Step 3: 打印遥测状态行(quiet 模式跳过)
    // Step 4: 初始化遥测运行时
    let telemetry = Telemetry::init(resolved, env!("CARGO_PKG_VERSION").into());
    // Step 4.5: 安装 panic hook
    install_panic_hook(telemetry.clone());
    // Step 5: 预计算 repo_origin
    let project_state = init_project_state(working_dir_override);
    let repo_origin = detect_repo_origin(&project_state.working_dir);
    // Step 6: 从存储的 auth 种子化 account_id
    telemetry.set_account_id(auth::get_stored_auth().map(|a| a.user.id));
    // Step 7: 初始化 MCP registry
    let mcp_registry = McpRegistry::from_config_background(&project_state.working_dir);
    // Step 8: 构建 AppState
    let state = AppState { project, active_chats, mcp_registry, ... };
    // Step 9: 绑定 listener
    // Step 10-11: 进入 CurrentContext scope + 发射 OpenAtomcode
    // Step 13: 带 graceful shutdown 的 axum::serve
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx)).await?;
    // Step 14: 最终遥测 flush
    telemetry.shutdown(Duration::from_millis(500)).await;
}
```

### 12.3 `AppState` 核心结构

```rust
pub struct AppState {
    pub project: ProjectStateStore,
    active_chats: ActiveChatRegistry,         // 单飞准入 + 取消
    pub mcp_registry: Arc<RwLock<Arc<McpRegistry>>>,
    pub mcp_cache: Arc<RwLock<HashMap<PathBuf, CachedMcpRegistry>>>,
    pub(crate) login_sessions: LoginSessionsStore,
    pub(crate) login_start_lock: Arc<Mutex<()>>,
    pub(crate) daemon_instance_id: Arc<str>,
    pub telemetry: Arc<Telemetry>,
    pub repo_origin: RepoOrigin,
    pub shutdown_tx: watch::Sender<bool>,
    pub last_activity: Arc<AtomicI64>,        // 用于 idle timeout
    pub active_connections: Arc<AtomicUsize>,
    pub webui_tokens: auth_token::WebuiTokenStore,
    pub enforce_token: bool,                  // webui 模式强制 token
    pub app_user_id: String,                  // App 远程访问 user_id 校验
    pub pending_permissions: permission_bridge::PermissionResponders,
    pub pending_user_inputs: permission_bridge::UserInputResponders,
    pub bind_host: String, pub bind_port: u16,
    pub webui_cookie_name: String,            // 端口隔离的 cookie 名
}
```

### 12.4 `ActiveChatRegistry`:单飞准入 + 取消

```rust
impl ActiveChatRegistry {
    async fn admit(&self, session_id: Option<&str>, request_id: Option<&str>)
        -> Result<ActiveChatAdmission, ActiveChatAdmissionError> {
        let mut index = self.inner.write().await;
        if session_id.is_some_and(|alias| index.aliases.contains_key(alias)) {
            return Err(ActiveChatAdmissionError::SessionBusy);
        }
        // ... 分配 operation_id + CancellationToken ...
    }
    async fn stop_alias(&self, alias: &str) -> bool { /* 标记 + cancel */ }
    async fn complete(&self, operation_id: &str) -> bool { /* 清理 */ }
}
```

### 12.5 路由分层:public vs protected

```rust
let public = Router::new()
    .route("/health", get(health))
    .route("/", get(serve_webui_index))
    .fallback(webui::serve_webui);

let protected = Router::new()
    .route("/shutdown", post(shutdown_handler))
    .route("/sessions", get(get_all_sessions).post(create_session))
    .route("/chat", post(chat_stream).layer(DefaultBodyLimit::max(32MB)))
    .route("/live", get(live_api::live_stream))
    .route("/auth/login/start", post(api_auth::auth_login_start))
    .route("/codingplan/setup", post(api_codingplan::codingplan_setup))
    // ... 60+ 路由 ...
    .route_layer(from_fn_with_state(state.clone(), auth_token::require_webui_token))
    .route_layer(from_fn_with_state(state.clone(), auth_token::require_app_user_id));
```

### 12.6 Idle Timeout 看门狗

```rust
fn spawn_idle_timeout_task(idle_timeout_secs: u64, last_activity: Arc<AtomicI64>,
    active_connections: Arc<AtomicUsize>, active_chats: ActiveChatRegistry,
    shutdown_tx: watch::Sender<bool>) {
    if idle_timeout_secs == 0 { return; }
    let timeout_ms = (idle_timeout_secs * 1000) as i64;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let elapsed = now_unix_ms() - last_activity.load(Ordering::Relaxed);
            let connected = active_connections.load(Ordering::Relaxed);
            let chatting = active_chats.has_active_operations().await;
            if elapsed > timeout_ms && connected == 0 && !chatting {
                shutdown_tx.send(true).ok();
                break;
            }
        }
    });
}
```

### 12.7 LiveViewHub:多客户端实时同步

daemon 内部有一个 `LiveViewHub`,通过 `broadcast` channel 实现多客户端(WebUI 多标签 + TUI)实时同步:

```rust
pub enum LiveViewEvent {
    InputAccepted { input: UserInput, client_input_id: Option<String> },
    Steered { count: usize, inputs: Vec<SteeredInput>, client_input_ids: Vec<Option<String>> },
    CommandOutput(String),
    RequestResolved { request_id: RequestId, kind: String },
    Runtime(CodingRuntimeEvent),
}
```

---

## 13. Coding / CodingPlan 运行时

### 13.1 `CodingRuntime` 结构

```rust
pub struct CodingRuntime {
    pub handle: CodingRuntimeHandle,
    pub events: CodingRuntimeEvents,
    pub task: tokio::task::JoinHandle<RuntimeExit>,
    pub session: Option<RuntimeSessionInfo>,
}
pub struct CodingRuntimeHandle {
    tx: mpsc::UnboundedSender<CodingRuntimeControl>,
    state: Arc<AtomicU64>,
    provider_unavailable_reason: Arc<AtomicU8>,
    terminal: watch::Receiver<Option<RuntimeExit>>,
}
```

### 13.2 `DriverCommand` 驱动协议

```rust
pub enum DriverCommand {
    Submit(UserInput),
    Respond { id: RequestId, value: serde_json::Value },
    ResolvePolicyIntervention { intervention_id, action },
    Cancel, PauseGoal, Compact(Option<String>),
    SetMode(RuntimeMode), QueueLocalContext(LocalContextInput),
    ReloadProvider(CodingAgentConfig), ReprepareConfig(CodingAgentConfig),
    DeactivateProvider(ProviderUnavailableReason),
    UndoToPrompt(Option<usize>),
    Rewind { turn_id, scope },
    RefreshContextStats,
    RestoreSnapshot(SessionSnapshot),
    StartGoal(String), StopGoal, StartLoop(String), StopLoop, Shutdown,
}
```

### 13.3 `CodingRuntimeEvent` 事件流

```rust
pub enum CodingRuntimeEvent {
    Agent(AgentEvent),
    Team { generation, event: TeamEvent },
    VisionPreprocessSuccess { vl_model, char_count },
    VisionPreprocessFailed { reason },
    SteerAcknowledged { inputs },
    CompactionStarted { trigger },
    CompactionFinished { completion },
    RuntimeStopped(RuntimeExit),
    Request(RuntimeRequest),
    TurnFinished(TurnCompletion),
    PolicyInterventionResolved { intervention_id, action },
    PolicyInterventionCleared { intervention_id },
    ModeChanged { mode },
    Reconfiguring { operation }, Reconfigured { operation },
    ProviderChanged { provider, model },
    ReasoningEffortChanged { provider, effort, applicable },
    ProviderUnavailable { reason, forced },
    SessionNameSuggested { name },
    NextPromptSuggested { generation, session_id, turn_id, text },
    SessionChanged(SessionChanged),
    WorkingDirectoryChanged(PathBuf),
    GoalChanged(GoalProgress),
    LoopChanged(LoopProgress),
    UndoFinished(...), RewindCatalogRefreshed(...), RewindFinished(...),
    ContextStatsRefreshed(...), SnapshotRestoreFinished(...),
}
```

### 13.4 控制器:Goal 与 Loop

```rust
pub struct GoalProgress {
    pub active: bool, pub terminal: Option<GoalTerminal>,
    pub pub phase: GoalPhase, pub round: u32, pub max_rounds: Option<u32>,
    pub elapsed_secs: u64, pub condition: String, pub last_reason: Option<String>,
}
pub enum GoalTerminal { Met, Stopped, Failed, Cancelled }
pub enum GoalPhase { Pursuing, Paused, PausedAtCap, Satisfied, Ended }
pub struct LoopProgress { pub active: bool, pub round: u32, pub elapsed_secs: u64, pub label: String, ... }
```

### 13.5 执行策略(用户意图解析)

```rust
const NO_BUILD: u8 = 1;    // "不要编译"
const NO_TEST: u8 = 2;     // "不要测试"
const NO_SCRIPT: u8 = 4;   // "不要运行脚本"
const NO_SHELL: u8 = 8;    // "不要执行命令"
const NO_VERIFY: u8 = 16;  // "不要验证"
```

### 13.6 Plan Mode 门控

```rust
pub struct PlanModeGate {
    active: Arc<AtomicBool>,
    mcp_grants: Arc<dyn PermissionStore>,
}
```

策略:
- 内置 `Risky` 工具(bash/edit/write)→ **硬阻断**
- MCP 工具声明 `readOnlyHint: true` → **允许**
- 其他 MCP 工具 → **提示用户**

### 13.7 VerifyCadenceHook(编辑后验证节律)

- **位置**:`atomcode-coding/src/discipline/verify.rs:30 VerifyCadenceHook`
- **机制**:实现 `LifecycleHooks::offer_continuation`(`verify.rs:378-419`)
  - 当模型停止(无工具调用)且本轮有编辑、但**未运行检查命令**时,注入一次性 nudge
  - nudge 文本(`verify.rs:23-25`):"Run a fast check (`cargo check`, `tsc --noEmit`, or the equivalent)"
- **状态机**(`verify.rs:47-53 State`):`nudged_for: Option<NudgedEdit>`,保证每个 edit-batch 只 nudge 一次
- **bash 排除名单**(`verify.rs:334`):`ls / echo / cat / pwd / tree / find / grep / wc` 等只读命令不算验证
- **工作区门禁**(`verify.rs:147-200 path_in_workspace_lexical`):编辑目标在 workspace 外(如 `/tmp`)不触发
- **文档排除**(`verify.rs:217`):`.md / .txt / .json / .yaml` 等 doc/data 文件不触发
- **attended 模式**(`verify.rs:86-101 attended`):交互 TUI 下**抑制**强制验证(人在场可主动要求);headless/scheduled 保持强制

### 13.8 codingplan 客户端

```rust
pub struct Client {
    http: std::sync::RwLock<reqwest::blocking::Client>,
    token: String,
}
impl Client {
    pub fn from_stored_auth() -> Result<Self> { ... }
    pub fn claim_v2(&self, plan_type: PlanType) -> Result<ClaimResponse> { ... }
    pub fn list_models_v2(&self, plan_type: PlanType) -> Result<Vec<ModelEntry>> { ... }
    pub fn status_v2(&self) -> Result<StatusResponse> { ... }
    pub fn usage(&self) -> Result<UsageResponse> { ... }
}
```

**TLS 降级兜底**:`send_with_tls_fallback` 在 connect 级失败时自动重试 TLS 1.2。

### 13.9 codingplan-crypto

`atomcode-codingplan-crypto` 是闭源 overlay,通过 optional dep 接入:

```toml
# crates/atomcode-auth/Cargo.toml
[features]
codingplan-crypto = ["dep:atomcode-codingplan-crypto"]
```

**占位 crate 模式**:open-source 编译时只是 stub;official build 通过 `build-official.sh` 覆盖整个文件,注入真正的签名逻辑。

---

## 14. TUI (tuix)

### 14.1 模块结构

```rust
pub mod commands;
pub mod custom_commands;
pub mod event_loop;
pub mod git;
pub mod glyph;
pub mod highlight;
pub mod i18n;
pub mod input;
pub mod markdown;
pub mod modals;
pub mod platform;
pub mod render;
pub mod sanitize;
pub mod session;
pub mod state;
pub mod team;
pub mod terminal;
pub mod think;
pub mod title;
pub mod trace;
pub mod version_check;
pub mod width;
```

**关键特性**:
- crossterm 原始模式 + Kitty 键盘协议(CSI u)
- 保留模式渲染(`RetainedRenderer`)+ 任务渲染(`TaskRenderer`)
- 终端能力检测(`TerminalCaps`)
- 语法高亮(`highlight/`)
- Git 集成(`git.rs` / `git_diff.rs`)
- 思考动画(`think.rs`)
- 后台 slot(`bg_runtime.rs`):`/bg` 把任务移到 detached slot,TUI 不阻塞

### 14.2 与 laew TUI 对比

| 维度 | atomcode tuix | laew TUI |
|------|--------------|---------|
| 渲染 | 保留模式 + 任务渲染 | 全量重绘 present |
| 输入 | crossterm + Kitty 协议 | crossterm 原始模式 |
| 子屏 | modals/ 模态框 | Screen 栈 |
| 补全 | - | 斜杠命令 + 文件路径 |
| 语法高亮 | 内置 highlight/ | - |
| Git 集成 | 内置 git_diff | - |
| 后台 slot | bg_runtime `/bg` | - |

---

## 15. Auth + Config + Telemetry + Updater + Review

### 15.1 auth:OAuth + 凭证安全存储

```rust
pub fn start_login() -> Result<LoginSession> {
    std::thread::spawn(move || {
        let login_url = platform_login_url();
        let was_capped = atomcode_config::tls::should_cap_url(&login_url);
        match attempt_login(was_capped) {
            Err(first) if atomcode_config::tls::should_try_fallback(...) => {
                match attempt_login(true) {  // TLS 1.2 降级重试
                    Ok(session) => { atomcode_config::tls::latch_managed_tls12(); Ok(session) }
                    Err(fallback) => Err(fallback.context(...)),
                }
            }
            other => other,
        }
    }).join()?
}
```

**凭证文件安全存储**:
- **目录 0o700**:`ensure_private_dir` 强制父目录权限
- **文件 0o600**:创建时即设置,不依赖 umask
- **临时文件 + 原子 rename**:避免半写状态
- **fsync**:确保落盘

### 15.2 config:TOML 多环境配置

```rust
pub struct Config {
    pub default_provider: String,
    pub evaluator_provider: Option<String>,
    pub default_workdir: Option<String>,
    pub providers: HashMap<String, ProviderConfig>,
    pub provider_accounts: HashMap<String, ProviderAccountConfig>,
    pub models: HashMap<String, ModelProfileConfig>,
    pub default_model: Option<String>,
    pub datalog: DatalogConfig,
    pub notifications: NotificationConfig,
    pub network: NetworkConfig,
    pub auto_update: bool,
    pub telemetry: TelemetryConfig,
    pub lsp: LspConfig,
    pub auto_commit: bool,
    pub subagent: SubAgentConfig,
    pub loop_config: LoopConfig,
    pub coding: CodingConfig,
    pub tools: ToolsConfig,
    pub vision_preprocessor_provider: Option<String>,
    pub language: Option<Locale>,
    pub init_prompt_file: Option<PathBuf>,
    pub ui: UiConfig,
    pub plugin: PluginConfig,
    pub web_search: WebSearchConfig,
    pub keep_interrupted_context: bool,
    pub offline_mode: OfflineMode,
    pub offline_note: Option<String>,
    #[serde(skip)] pub quarantined_providers: BTreeMap<String, toml::Value>,
}
```

### 15.3 telemetry:6 事件集

```rust
pub enum Event {
    OpenAtomcode { dangerously_skip_permissions: bool },
    LlmChat { duration_ms, tool_calls_count, input_tokens, output_tokens, cached_tokens, had_error, context_window, ... },
    ToolCall { name, success, duration_ms, error_kind, error_data },
    UseCommand { type_, success, error_kind, error_data },
    McpConnect { server_name, transport, success, duration_ms, error_kind, error_data },
    LoginSuccess { invite_code, install_uuid },
    InstallCompleted { invite_code, install_uuid },
    TakeCodingplan { type_, error_kind, error_data },
    Panic { location, message_head, thread, backtrace_top_5, error_kind, error_data },
    TelemetryDisabled,
    CodingplanOfficialBuildRequired,
}
```

**Envelope 公共字段**:`device_id / launch_id / account_id / session_id / turn_id / ts / schema_version / app_version / os / arch / locale / provider / model / repo_origin / mode / surface`

### 15.4 updater:三进制交换

```rust
fn replace_binary(new_bin: &Path, exe: &Path) -> Result<()> {
    #[cfg(unix)] { perm.set_mode(0o755); }
    let backup = backup_path(exe);      // exe.bak
    let rolling = rolling_path(exe);    // .atomcode.rolling
    try_remove_stale(&rolling);
    // Step 1: live → rolling(Windows 允许重命名运行中的 exe)
    robust_rename(exe, &rolling)?;
    // Step 2: new → live
    if let Err(e) = robust_rename(new_bin, exe) {
        let _ = std::fs::rename(&rolling, exe);  // 回滚
        return Err(...);
    }
    // Step 3: best-effort 删除旧 .bak
    let bak_removed = try_remove_stale(&backup);
    // Step 4: rolling → .bak
    if bak_removed { let _ = std::fs::rename(&rolling, &backup); }
    Ok(())
}
```

**Deferred upgrade**:会话内下载、下次启动应用。

### 15.5 review:代码审查/质检

```rust
impl Tool for ReviewTool {
    fn name(&self) -> &str { "code_review" }
    fn risk(&self, _args: &str) -> RiskLevel { RiskLevel::Safe }  // 只读
    async fn execute(&self, args: &str, _ctx: &ToolContext) -> ToolResult { ... }
}
```

**deep 模式 fan-out**:
```rust
pub const REVIEW_DIMENSIONS: &[&str] = &["security", "performance", "correctness", ...];
pub fn run_deep_review(...) -> Vec<DimensionOutcome> { ... }
```

### 15.6 clix:独立 code review CLI

`atomcodex` 是独立二进制,支持 `code` / `sessions` / `review` 三个子命令。

**与主 CLI 的关系**: clix 是**独立二进制**,不依赖 atomcode-core/coding,直接组装 kernel + capabilities,适合嵌入 CI/CD 或作为轻量工具。

---

## 16. WebUI / Extensions / Evals / Docker

### 16.1 WebUI:内嵌式 Preact SPA

**技术栈**: Preact + Tailwind CSS + Vite。选 Preact 而非 React 的核心考量是打包体积(Preact 3KB gzip vs React 45KB)。

**daemon 内嵌 + rust-embed**:WebUI 的静态资源在**编译期**通过 `rust-embed` 嵌入 daemon 二进制:

```rust
#[derive(RustEmbed)]
#[folder = "../../webui/dist/"]
#[allow_missing = true]
pub struct WebuiAssets;

pub fn asset_or_index(path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
    let p = path.trim_start_matches('/');
    if let Some(f) = WebuiAssets::get(p) {
        return Some(f.data);
    }
    WebuiAssets::get("index.html").map(|f| f.data)
}
```

**四种审批模式**:
- **Build**: 交互式审批(默认),每个敏感操作需用户确认
- **AcceptEdits**: 文件编辑自动通过,bash 等仍需确认
- **Auto**(bypass): 全自动,所有操作直接通过
- **Plan**: 只读探索,不允许执行

**乐观会话 + URL 恢复**:首条消息发出瞬间用前 10 字做临时标题,乐观插入侧栏;后端回传真实 session_id 后按 id 去重覆盖。URL 中存储 session id 前 8 位,刷新后通过 `resolveSession()` 跨所有桶定位完整记录。

**Live Steer 实时注入**:WebUI 支持在 AI 正在生成时**实时注入新输入**,前端有完整的状态机管理(`reconcileSteerReceipt`)。

**通知 + PWA**:回合完成时推送浏览器通知,支持安装到桌面。

### 16.2 Extensions:IDE 扩展

#### VSCode 扩展

**架构**: daemon 进程管理 + webview-ui 通信。扩展本身是薄壳,核心逻辑在 daemon 中。

**关键特性**:
1. **自动重连**: daemon 空闲超时退出后,下次请求自动重启(共享 promise 防并发)
2. **CodeAction 提供者**: 右键菜单 explain/fix/optimize,自动注入编辑器上下文
3. **WebviewPanel 序列化器**: 跨重启恢复 Tab 状态
4. **30 秒健康检查**: 定时探测 daemon 存活

**webview-ui**: 使用 React + esbuild 构建,独立于 webui/ 目录。

#### JetBrains 扩展

**架构**: Kotlin + IntelliJ Platform SDK,直接嵌入 daemon 二进制。

**官方构建验证**: JetBrains 插件要求 daemon 必须经过官方签名(`build-official.sh`),否则拒绝启动。

**亮点**: JetBrains 扩展有独立的 `SensitivePathClassifier`(敏感路径分类器)和 `SecretRedactor`(密钥脱敏),安全意识强。

### 16.3 Evals:配对评估框架

`evals/deepseek-v4-flash/` 是一个**配对评估框架**,对比两个候选模型(AtomGit 官方 vs 火山引擎)在同一组 case 上的表现。

**设计原则**:
- **配对同时启动**: 两个候选在同一个 case 上同时运行,消除时间偏差
- **盲评**: 评估者(Codex)不知道哪个是 A/B,消除偏见
- **多层验证**: 机器验证(脚本) + LLM 盲评(Codex) + 统计分析(bootstrap CI)

**五步流水线**: `prepare` → `run` → `judge` → `summarize` → `report`

**两层 case**:
- **model 层**(20 case): 纯文本能力(调试/逻辑/代码/指令遵循/上下文/tool schema),无工具调用
- **agent 层**(8 case): 带 fixture 的工具调用(修 bug/跨文件/重构/诊断/契约修复)

每个 agent case 有 `fixture`(拷贝的工作目录)、`verify`(可执行的验证命令)、`rubric`(评分标准)。

**统计分析**: 使用 bootstrap 置信区间(10000 次重采样)评估配对差异的统计显著性。

**评估维度**: `correctness` / `quality` / `instruction_following` / `agent_execution`

### 16.4 Docker:容器化部署

**双镜像策略**:

| 镜像 | 用途 | 基础镜像 |
|------|------|----------|
| Dockerfile-Daemon | NAS/服务器常驻 daemon | debian:bookworm-slim |
| Dockerfile-TUI | macOS/Windows 体验 Linux 版 | debian:bookworm-slim |

**docker-compose 安全设计**:
- 默认绑定 127.0.0.1,必须显式 `BIND_ADDR=0.0.0.0` 才对外开放
- 配置文件只读挂载,防止容器意外改写
- `no-new-privileges` 禁止提权
- 健康检查用 `/dev/tcp` 而非 curl(镜像无 curl)

### 16.5 scripts:构建与发布

**release.sh**: 六目标交叉编译(macOS ARM/Intel, Linux x64/ARM64, Windows x64/ARM64)

**版本来源**: 从 `Cargo.toml` 的 `[workspace.package].version` 读取,而非 git tag。

**latest.json 生成**: 自动为 `/upgrade` 自更新生成 manifest。

**release-daemon.sh**: 专门为 VSCode 扩展打包 daemon 二进制,必须先构建 webui。

**npm 分平台 wrapper**: wrapper 脚本检测当前平台,`require.resolve` 对应子包,然后 `spawn` 真实二进制。

---

## 17. 对 laew 的借鉴(P0/P1/P2)

### 17.1 P0:必须借鉴(架构级)

| 借鉴点 | atomcode 实现 | laew 现状 | 建议 |
|--------|-------------|----------|------|
| **三层垂直架构** | L0 kernel / L1 capabilities / L2 coding,compile-enforced | 扁平模块 | 拆 `laew-core` / `laew-tools` / `laew-app`,cargo 强制依赖方向 |
| **双向 Command/Event 协议** | `AgentCommand`/`AgentEvent` 可序列化,跨进程 | `YoloRunner` 进程内 | 定义 `LaewCommand`/`LaewEvent` 枚举,为 daemon 化做准备 |
| **Feature gating** | 16 个 feature 按能力切 | 全量编译 | 按 `provider`/`tools`/`mcp`/`skill` 切 feature |
| **安全熔断常量** | `MAX_REPEAT_ROUNDS`/`MAX_TRUNCATION`/`MAX_RATE_LIMIT` | 无 | 在 agent 循环中添加多层熔断 |
| **prefix-cache 安全** | `cache_epoch` + `synthetic` 标记 | 无 | 在 Message 添加 `synthetic` 标记 |
| **失败感知 StopReason** | 11 种终止原因 | 无 | 定义 `StopReason` 枚举 |
| **release profile** | `opt-level "z" + lto + codegen-units=1 + strip + panic="abort"` | 未配置 | 直接照搬,节省 ~200-500KB |
| **prepare → assemble 两阶段** | 异步 I/O + 纯组合 + `Arc<…>` 跨 respawn 持久 grants | 无 | 把 provider 装配与内核组合分离 |

### 17.2 P1:应该借鉴(能力增强)

| 借鉴点 | atomcode 实现 | laew 现状 | 建议 |
|--------|-------------|----------|------|
| **MCP 支持** | `McpRegistry` + `McpToolAdapter` | 无 | 实现 MCP 客户端 capability |
| **Skill 系统** | `SkillRegistry` + frontmatter 加载 | 无 | 实现 skill 加载 + `use_skill` 工具 |
| **Session 持久化** | `SnapshotHook` + `TranscriptHook` | 无 | 实现双层持久化 |
| **Memory 注入** | `MemoryHook` session_start 注入 | 无 | 实现 memory.md + 注入 |
| **审批链** | `ApprovalMiddleware` + `PermissionStore` | 无 | 实现工具审批 + "always" 授权 |
| **429 分层处理** | provider/kernel/stream 三层 | 单层 | 实现分层重试 |
| **TLS 降级兜底** | `send_with_tls_fallback` | 无 | 实现 TLS 1.2 自动降级 |
| **Tool 信任模型** | 明确"不沙箱化" + RiskLevel 元数据 | 无 | 在 `Tool` trait 增加 `risk()` 方法 |
| **HookChain 扇出** | 每个 hook 独立 slot,Yolo/QC/Verify 各自注册 | 无 | 用 `LifecycleHooks` 替代硬编码 |
| **ToolMiddleware load-bearing** | before/after 注册顺序 = 契约 | 无 | 用 `BeforeOutcome::Allow/Ask/Deny` |

### 17.3 P2:可以借鉴(体验优化)

| 借鉴点 | atomcode 实现 | laew 现状 | 建议 |
|--------|-------------|----------|------|
| **WebUI** | axum + WebUI SPA | 无 | 可选:添加 WebUI 支持 |
| **自动更新** | 三进制交换 + deferred upgrade | 无 | 可选:添加自更新 |
| **遥测** | 6 事件集 + privacy scrub | 无 | 可选:添加匿名遥测 |
| **Goal/Loop 控制器** | 跨 turn 自主循环 | 无 | 可选:添加 /goal /loop |
| **Plan Mode** | 只读探索模式 | 无 | 可选:添加 plan 模式 |
| **后台 slot** | `/bg` 不阻塞主 REPL | 无 | 可选:添加后台任务 |
| **Eval 框架** | 配对评估 + 盲评 + bootstrap CI | 无 | 参考 `evals/deepseek-v4-flash/` |

### 17.4 架构差异警示

1. **Agent 是 struct vs trait**:atomcode 选择具体 struct + Builder,laew 选择 trait(`LlmClient`)。两种都可行,但 atomcode 的方式更适合"内核 SDK"定位。
2. **分层严格性**:atomcode 通过 compile-enforced 分层(capabilities 不依赖 core),laew 目前较扁平。
3. **沙箱决策**:atomcode 明确"不沙箱化",把责任推给嵌入者。laew 同样无沙箱,但应明确声明。
4. **Yolo 差异**:atomcode **没有**独立的 Yolo Agent,它的"任务分类"是 goal-mode 的 followup 分类器 + TeamDifficulty 两档,不是 laew 的三档(简单/中/难)入口分类。

---

## 18. 关键文件清单与常量速查

### 18.1 核心文件清单

| 文件 | 行数 | 说明 |
|------|------|------|
| `crates/atomcode-daemon/src/lib.rs` | 9072 | daemon 核心,含 run_server、AppState、ActiveChatRegistry |
| `crates/atomcode-daemon/src/main.rs` | 193 | daemon 二进制入口 |
| `crates/atomcode-daemon/src/live_api.rs` | 3000+ | live_* 系列 handler |
| `crates/atomcode-daemon/src/auth_token.rs` | 305 | WebuiTokenStore + 中间件 |
| `crates/atomcode-auth/src/lib.rs` | 211 | 凭证安全写入 |
| `crates/atomcode-auth/src/oauth.rs` | 2039 | OAuth 完整流程 |
| `crates/atomcode-kernel/src/agent.rs` | 5966 | Agent struct + turn loop |
| `crates/atomcode-kernel/src/event.rs` | 531 | AgentCommand/AgentEvent |
| `crates/atomcode-kernel/src/tool.rs` | 604 | Tool trait + ToolRegistry |
| `crates/atomcode-kernel/src/provider.rs` | 269 | LlmProvider trait |
| `crates/atomcode-kernel/src/hook.rs` | 638 | LifecycleHooks |
| `crates/atomcode-kernel/src/middleware.rs` | 158 | ToolMiddleware |
| `crates/atomcode-kernel/src/stream.rs` | 280 | StreamEvent |
| `crates/atomcode-kernel/src/message.rs` | 2064 | Message + SessionSnapshot |
| `crates/atomcode-coding/src/runtime.rs` | 2000+ | CodingRuntime + DriverCommand |
| `crates/atomcode-coding/src/parts.rs` | 1500+ | prepare/assemble |
| `crates/atomcode-coding/src/controllers.rs` | 500+ | Goal/Loop 控制器 |
| `crates/atomcode-coding/src/discipline/verify.rs` | 992 | VerifyCadenceHook |
| `crates/atomcode-capabilities/src/lib.rs` | 201 | L1 层模块结构 |
| `crates/atomcode-capabilities/src/tools/mod.rs` | 120+ | 工具清单 |
| `crates/atomcode-capabilities/src/tools/approval.rs` | 300+ | ApprovalMiddleware |
| `crates/atomcode-capabilities/src/provider/anthropic.rs` | 2024 | Anthropic 适配 |
| `crates/atomcode-capabilities/src/provider/openai_compat.rs` | 4092 | OpenAI 兼容适配 |
| `crates/atomcode-capabilities/src/mcp/registry.rs` | 1200+ | McpRegistry |
| `crates/atomcode-capabilities/src/mcp/tool.rs` | 300+ | McpToolAdapter |
| `crates/atomcode-capabilities/src/skills/registry.rs` | 400+ | SkillRegistry |
| `crates/atomcode-capabilities/src/memory/hook.rs` | 200+ | MemoryHook |
| `crates/atomcode-capabilities/src/memory/store.rs` | 200+ | MemoryStore |
| `crates/atomcode-config/src/config/mod.rs` | 900+ | Config 结构体 |
| `crates/atomcode-telemetry/src/event.rs` | 800+ | Event 枚举 |
| `crates/atomcode-tuix/src/lib.rs` | 1081 | TUI 模块结构 |
| `crates/atomcode-review/src/review_tool.rs` | 1674 | ReviewTool |
| `crates/atomcode-clix/src/main.rs` | 1853 | atomcodex CLI |
| `crates/atomcode-updater/src/lib.rs` | 1932 | 自动更新 |

### 18.2 关键常量速查

| 常量 | 值 | 位置 |
|------|---|------|
| `DEFAULT_MAX_TOOL_RESULT_BYTES` | 64 KiB | `agent.rs:33` |
| `MAX_OVERFLOW_ATTEMPTS` | 3 | `agent.rs:112` |
| `DEFAULT_MAX_PROVIDER_RETRIES` | 3 | `agent.rs:121` |
| `MAX_STREAM_RETRIES` | 5 | `agent.rs:126` |
| `MAX_RATE_LIMIT_WAITS` | 5 | `agent.rs:142` |
| `EMPTY_RESPONSE_MAX_RETRIES` | 5 | `agent.rs:165` |
| `MAX_TRUNCATION_CONTINUATIONS` | 4 | `agent.rs:176` |
| `MAX_REPEAT_ROUNDS` | 6 | `agent.rs:182` |
| `RATE_LIMIT_AUTO_WAIT_SECS` | 120 | `hook.rs:52` |
| `DAEMON_PORT` | 13456 | `distribution.rs` |
| `CHAT_REQUEST_BODY_LIMIT_BYTES` | 32 MiB | `lib.rs:112` |
| `DEFAULT_IDLE_TIMEOUT_SECS` | 30 分钟 | `main.rs:19` |
| `MCP_CACHE_MAX` | 5 | `lib.rs:667` |
| `MAX_MCP_TOOL_NAME_LEN` | 64 | `mcp/tool.rs` |
| `MAX_SERVER_INSTRUCTIONS_CHARS` | 4,000 | `mcp_instructions.rs` |
| `MAX_TOTAL_INSTRUCTIONS_CHARS` | 16,000 | `mcp_instructions.rs` |

### 18.3 核心 trait 一览

| Trait | 位置 | 核心方法 |
|-------|------|---------|
| `LlmProvider` | `kernel/src/provider.rs` | `chat_stream` 返回 `BoxStream<StreamEvent>` |
| `Tool` | `kernel/src/tool.rs` | `execute` + `risk` + `read_only_hint` + `parallel_safe` |
| `ToolMiddleware` | `kernel/src/middleware.rs` | `before` / `after` |
| `LifecycleHooks` | `kernel/src/hook.rs` | 12 个生命周期钩子 |
| `CompactionStrategy` | `kernel/src/message.rs` | `plan` + `will_summarize` |
| `McpClient` | `mcp/client.rs` | `initialize` / `list_tools` / `call_tool` |
| `PermissionStore` | `tools/approval.rs` | `is_allowed` / `allow` / `deny` |

### 18.4 原始文档索引

- `atomcode-源码调研.md` — 首轮总览(213 行)
- `atomcode-深度分析.md` — 8 维度深度分析(437 行)
- `atomcode-核心机制深度分析.md` — 核心机制(896 行)
- `atomcode-第二轮深度分析.md` — 第二轮深挖 kernel/MCP/Skill/CodeIntel(1660 行)
- `atomcode-第三轮边缘模块深度分析.md` — 第三轮深挖边缘模块(804 行)
- `atomcode-第四轮-daemon与coding核心深度分析.md` — 第四轮最新最完整(1493 行)

---

> **本轮合并基于真实源码阅读,所有行号与代码片段均来自 `/usr/local/LsmGitOpenSource/atomcode` 仓库(版本 5.0.9)。合并策略:以第四轮为主要框架,补充第二轮 kernel/agent loop 详细代码分析,补充第三轮边缘模块独立成章,删除明显重复段落。**

---

## 19. 第五轮深挖补充(2026-09-06)

针对前 18 章覆盖薄弱/遗漏的代码级细节,基于真实源码再核一遍。**与前文不重复**:仅补足 loop 内 select 驱动、Cancel 双路传播、压缩两层策略、并发执行闸位、Session append-only + inflight 恢复、5.0.9 release 真实变更点。

### 19.1 session_loop 与 run_turn 的 select 驱动

**顶层 session_loop**(`crates/atomcode-kernel/src/agent.rs:1283`):

```rust
async fn session_loop(self, mut cmd_rx: UnboundedReceiver<AgentCommand>) {
    loop {
        let cmd = cmd_rx.recv().await;  // L1361
        match cmd { /* 处理 SendMessage/Snapshot/Compact/Cancel/Shutdown/Respond */ }
    }
}
```
- spawn 入口:`agent.rs:966` `tokio::spawn(running.session_loop(cmd_rx));`

**单轮 select 驱动(turn + 中途命令)**(`agent.rs:1541-1624`):核心是把 `run_turn` 的 future 与命令接收 race 在一起:

```rust
let mut turn = Box::pin(self.run_turn(..., turn_token.clone(), internal_cancel.clone(), ...));
let mut shutdown = false;
loop {
    tokio::select! {
        _ = &mut turn => break,  // turn 自身完成
        maybe = cmd_rx.recv() => match maybe {
            Some(AgentCommand::Shutdown) => { shutdown=true; internal_cancel.store(true, Release); turn_token.cancel(); self.rt.cancel_pending(); ... }
            Some(AgentCommand::Cancel)   => { turn_token.cancel(); self.rt.cancel_pending(); ... }
            Some(c @ AgentCommand::Snapshot)            => pending.push_back(c),
            Some(c @ AgentCommand::SendSyntheticMessage) => pending.push_back(c),
            Some(c @ AgentCode::SendMessageWithContext) => pending.push_back(c),
            Some(AgentCommand::SendMessage{text,images})=> steer.lock().push_back(SteerInput{...}),
            Some(c @ AgentCommand::Compact{..})         => pending.push_back(c),
            None => { shutdown=true; break; }
        }
    }
}
```
- **Steer vs Pending 的区分**:`SendMessage`(用户新增消息)在 turn 进行中**入 steer 队列**(`agent.rs:1606`),不是 pending——下一轮请求会把它 fold 进同轮;其它命令入 `pending: VecDeque<AgentCommand>`(`agent.rs:1663` `while let Some(queued) = pending.pop_front()`)。
- 这种"中途用户消息不打断 turn 而是排队"的语义是 steering(steer queue),区别于 explicit cancel——后者会立刻终止 turn。

### 19.2 StopReason 完整 12 变体与熔断点

`crates/atomcode-kernel/src/event.rs:88-119`:

```rust
pub enum StopReason {
    #[default] Stopped,            // 自然停止
    MaxRounds,                     // LLM 轮数熔断
    MaxContinuations,              // offer_continuation 钩子无限续轮熔断
    RepeatLoop,                    // 粗粒度重复熔断
    ToolLoopDetected,              // 精确工具循环熔断
    ProviderError,                 // 流打开/中途错误
    Timeout,                       // stream_timeout
    Cancelled,                     // 协作式取消
    PromptRejected,                // user_prompt_submit 钩子拒绝
    PolicyDenied,                  // 工具中间件硬阻断
    RateLimited,                   // 429 host 选 PAUSE
}
```

各触发点:
- `MaxRounds`:`agent.rs:1999 / 2010`
- `MaxContinuations`:`agent.rs:3086`(钩子在 `hook.rs:293` `offer_continuation`)
- `RepeatLoop`(粗):`agent.rs:3931`
- `ToolLoopDetected`(精):`agent.rs:3895`
- `RateLimited`:`agent.rs:2247 / 2296 / 2692 / 2709 / 2758`
- `ProviderError`:`agent.rs:2349 / 2782 / 2907`

**finishing 汇聚点**:`agent.rs:1745` `async fn finish_turn(&self, convo, reason: StopReason, ctx: &TurnCtx)`;`Outcome.stop: StopReason`(`agent.rs:761`)。**Cancelled 路径**:`agent.rs:1831` 直接 `finish_turn(convo, StopReason::Cancelled, ctx)`。

熔断默认 ON:`max_continuations: Some(50)`(`agent.rs:809-815`)。

### 19.3 Cancel 双路传播(CancellationToken + cancel_pending)

**类型**:统一用 `tokio_util::sync::CancellationToken`(不是 `Notify`)。

```rust
// agent.rs:880 字段
cancel_token: Option<tokio_util::sync::CancellationToken>;
// setter
pub fn cancel_token(mut self, t: tokio_util::sync::CancellationToken) -> Self { ... } // L4274

// agent.rs:1096-1101 派生
fn new_turn_token(&self) -> CancellationToken {
    self.cancel_token.as_ref().map(|t| t.child_token()).unwrap_or_default()
}
```

**两路 cancel(turn 路径)**(`agent.rs:1567-1580`):

```rust
internal_cancel.store(true, Ordering::Release);  // ① AtomicBool 协作信号
turn_token.cancel();                              // ② CancellationToken 协作信号
self.rt.cancel_pending();                         // ③ 强解 oneshot 等待的中间件(审批等)
```

`cancel_pending`(`crates/atomcode-kernel/src/request.rs:84-94`):

```rust
/// Resolve EVERY pending request to Value::Null ... Called on AgentCommand::Cancel.
pub(crate) fn cancel_pending(&self) {
    for (_, tx) in self.pending.lock().unwrap().drain() {
        let _ = tx.send(Value::Null);  // JSON-RPC null,fail-closed
    }
}
```
- 注释明确:oneshot `pending` 是中间件/审批等待,token 触不到——必须用 `Null` flush。

**流与工具执行中的取消检查点**:
- 串行路径 between-tools 兜底:注释 `agent.rs:3613` `INSIDE-EXECUTE backstop: poll cancel while the tool future runs`。
- 并发批次路径:`agent.rs:3561` `if cancel.is_cancelled() { return (idx, None); }`(cancel-skipped,不发 `ToolStarted`)。
- 工具内 `tokio::select!`:`agent.rs:3619-3628`:
  ```rust
  let mut r = tokio::select! {
      biased;
      r = tool.execute(&call.arguments, &ctx) => r,
      _ = cancel.cancelled() => ToolResult {
          call_id, content: "(cancelled — side effects unknown)".into(),
          is_error: true, ...
      },
  };
  ```
- 工具上下文暴露 cancel:`agent.rs:3591` `ToolContext { ... cancel: cancel.clone(), ... }`。

**Cancel 后消息层 backfill**(`crates/atomcode-kernel/src/message.rs:438-439`):
```rust
pub fn backfill_cancelled_tool_results(&mut self) {
    self.backfill_missing_tool_results("(cancelled)");
}
```
- **保证**:取消发生后,任何已发出 `tool_use` 都补回配对 `tool_result` 再发模型——这是协议一致性的硬要求(anthropic/openai 都拒绝 orphan tool_use)。

### 19.4 压缩:StubCompaction + OverflowCompaction 双层

`crates/atomcode-capabilities/src/compaction.rs:143-192` 核心常量:

```rust
const RECENT_KEEP_FRACTION: f32 = 0.25;          // 保留 25% 最近内容
const MIN_RECENT_KEEP_TOKENS: usize = 8_000;
const MAX_RECENT_KEEP_TOKENS: usize = 256_000;
const MAX_SUMMARY_BYTES:   usize = 64 * 1024;    // 摘要 64 KiB 硬上限 (#747)
const MAX_SUMMARY_TOKENS:  u32    = 16_000;     // 摘要软上限
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(180);
const SUMMARY_TOOL_OUTPUT_MAX_CHARS: usize = 2_000;  // 摘要输入截断头
const SUMMARY_TOOL_OUTPUT_TAIL_CHARS: usize = 500;   // 摘要输入保留尾
```

**两层策略**:
- `StubCompaction`(温和,内层)(`compaction.rs:101-105`):仅原地 stub 旧 tool result,**无** summary/drain/resume note,常用于"自动压缩一律静默"(5.0.9 `b9afca1d` 提交实现)。
- `OverflowCompaction`(硬阶梯,外层)(`compaction.rs:194-214`):
  - attempt 0:aggressive stub
  - attempt 1:hard-truncate 超大消息
  - attempt 2:drain 老轮到 1 个 LLM summary(`summary_provider=None` 时退化 plain drain)

**触发判断**(`agent.rs:1113-1128`):
```rust
fn should_compact(&self, convo) -> Option<CompactTrigger> {
    // 按当前模型 context_window() 重算 used/window >= thresh
    // 不依赖历史记录里的 ratio(防切换小窗口模型后误判)
}
```
- `compact_threshold: Option<f32>`(`agent.rs:836`),默认 `None` = 永不自动压缩;setter `agent.rs:4182`,默认填充 `0.7`(`compaction.rs:127` 注释)。
- **续轮前补一次**:`agent.rs:1138` `compact_before_internal_continuation`,每 policy stage 至多一次——防 thrashing。
- **手动 `/compact`**:`AgentCommand::Compact { focus }`(`event.rs:166-168`)无视 auto threshold 强制执行;`apply_plan` 拒绝 net-loss/no-op。

### 19.5 工具并发:Semaphore + RwLock 双闸 + FuturesOrdered

`agent.rs:3480-3653` Phase ② 并发执行骨架:

```rust
use futures::stream::FuturesOrdered;
let gate = Arc::new(tokio::sync::RwLock::new(()));   // 闸
let cap  = self.max_parallel_tools
    .unwrap_or_else(env_max_parallel_tools)
    .clamp(1, MAX_PARALLEL_TOOLS_CEILING);
let sem  = Arc::new(tokio::sync::Semaphore::new(cap));  // 容量
// ...
let mut ordered: FuturesOrdered<...> = (0..plans.len()).map(|idx| {
    let sem2 = sem.clone(); let gate2 = gate.clone();
    async move {
        let _g = gate2.read().await;       // ① 默认 read-lock 允许并行
        let _p = sem2.acquire_owned().await; // ② 容量许可
        // execute tool ...
    }
}).collect();
// ...
while let Some((idx, r)) = ordered.next().await { results[idx] = r; }
```

**常量与配置**(`agent.rs:204-225`):
- `MAX_PARALLEL_TOOLS_CEILING = 256`
- `ATOMCODE_MAX_PARALLEL_TOOLS` 环境变量默认 4
- setter `agent.rs:4147` `max_parallel_tools(n)`

**write-lock 排他屏障**(`agent.rs:3546-3551`):有 mutating tool 时可 upgrade 到 write-lock,**全局串行化**,用于强一致批操作(如对同一文件的多个 edit)。

**为什么用 `FuturesOrdered` 而不是 `FuturesUnordered`**(`agent.rs` Phase ③ 应用):应用阶段按原始 `plans` 顺序消费结果,**保持 emission 顺序与发起顺序一致**——这对后续模型看到 tool_result 顺序、replay 起点选取都至关重要。

**进程级并发**(能力层,MCP/CC hooks):
- `mcp/registry.rs:608` `futures::future::join_all(tasks).await;`
- `mcp/registry.rs:740` `FuturesUnordered<...> = server_snapshot...`
- CC hooks 并发:`cc_hooks.rs:600/634/694/727` 均 `join_all(...)`。

**CPU/IO 隔离**(常用 `spawn_blocking` 位置):
- `tools/list.rs:81`、`tools/glob.rs:96`、`tools/grep.rs:120`、`tools/parallel_edit.rs:324`、`tools/search_replace.rs:109`、`tools/bash.rs:2679`
- `tools/mod.rs:517` `tokio::time::timeout(timeout, tokio::task::spawn_blocking(f)).await;`
- `tools/task.rs:934` Semaphore 限并发
- `codeintel/*` 多个文件均 `spawn_blocking`(CPU bound);`provider/atomgit_sign.rs:64`、`skills/use_skill.rs:84`、`session/snapshot.rs:761/843` 同。

### 19.6 Session 持久化:jsonl + meta + inflight + snapshot

**4 件套**(路径在 `crates/atomcode-capabilities/src/session/manager.rs:999` `path_for(id, ext)`):
- `<id>.jsonl` — append-only transcript
- `<id>.meta` — 元数据
- `<id>.inflight` — 正在写未 commit 的工作集
- `<id>.snapshot` — 工作集快照

**Append-only 写**( `manager.rs:2859-2895` `append_jsonl_line`):

```rust
pub(crate) fn append_jsonl_line(&self, id: &str, line: &[u8]) -> SessionResult<()> {
    self.ensure_native_writable(id, "append transcript")?;
    if line.len() > MAX_JSONL_LINE_BYTES { return Err(...); }
    let path = self.jsonl_path(id)?;
    let mut file = retry_transient_file_access(|| open_append_file(&path))?;
    fs2::FileExt::lock_exclusive(&file).map_err(...)?;  // 进程间文件锁
    let current = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
    let next = current.checked_add(line.len()).ok_or(TooLarge{...})?;
    if next > MAX_JSONL_BYTES { return Err(TooLarge{...}); }
    file.write_all(line).map_err(|e| io_at(&path, e))
}
```

**上限常量**(`manager.rs:40-42`):
```rust
pub const MAX_JSONL_LINE_BYTES: usize = 16 * 1024 * 1024;  // 单行 16 MiB
pub const MAX_JSONL_BYTES:      usize = 512 * 1024 * 1024; // 文件 512 MiB
pub const MAX_JSONL_LINES:      usize = 1_000_000;
```

**崩溃恢复(Inflight)**(`manager.rs:1235-1283` `load_native_session_for_resume`):

```rust
pub fn load_native_session_for_resume(&self, lease: &SessionLease)
    -> SessionResult<(LoadedSession, Option<Message>)> {
    self.validate_active_lease(lease)?;
    let mut loaded = self.load_native_session(lease.id())?;
    let inflight = match self.load_inflight_snapshot(lease.id()) { ... };
    let canonical_len = loaded.snapshot.messages.len();
    let recoverable = inflight.snapshot.messages.len() == canonical_len.saturating_add(1)
        && inflight.snapshot.messages[..canonical_len] == loaded.snapshot.messages
        && inflight.snapshot.messages.last().is_some_and(|m| {
            m.role == Role::User && !m.synthetic
                && m.internal_origin.is_none() && m.tool_calls.is_empty()
        });
    if recoverable {
        if inflight.replay_safe {
            Ok((loaded, inflight.snapshot.messages.last().cloned()))
        } else {
            loaded.snapshot = inflight.snapshot;
            Ok((loaded, None))
        }
    } else {
        self.clear_inflight_snapshot(lease.id());  // 不污染 committed
        Ok((loaded, None))
    }
}
```
**接受条件**:canonical prefix + exactly 1 final accepted user message(`!synthetic && internal_origin.is_none() && tool_calls.is_empty()`)。

**Orphan 修复**(`agent.rs:1302`):`Conversation::repair_pairing(&mut c.messages);`——resume seed 时调用,处理 dangling tool_call / orphan tool_result。

**Lease(互斥)**(`manager.rs:1300-1304`):`acquire_lease` 保证同时只有一个 active runtime,错误为 `SessionStoreError::SessionInUse`。

**Snapshot hook**(`session/snapshot.rs:98-105`):
```rust
pub struct SnapshotHook {
    persistence_status: SnapshotPersistenceStatus,  // 持久化状态信号
    checkpoint: SnapshotCheckpoint,                  // 检查点
    target_snapshot: Option<SessionSnapshot>,        // 目标快照
    // ...
}
```
- 注释明示:"persists the working-set snapshot + session metadata every turn. Hangs off `turn_complete`"——每个 turn 完成后落一次。

### 19.7 v5.0.9 真实近期变更(2026-08-25 ~ 2026-09-03)

`git log --oneline --since=2026-08-25 | head -30` 中关键条目分类:

**压缩/TUI 体验打磨**:
- `7f21f6cd` 压缩 spinner 只显示时钟,去掉误导的 token 计数
- `fcd3855d` spinner 格式对齐 thinking spinner + 去掉"较慢"变体
- `c561984c` 手动 /compact 微小节省显示"当前会话无需压缩"而非成功横幅
- `16f548ff` CompactionMark 主题感知 muted 色
- `c8639008` **split boundary 永不 drain 整条尾部**——保留至少最近一条真实交换(防"压缩后空转")
- `18246520` **单个巨型回合可被压缩**——drain 活动回合旧前缀 + overflow 阶梯不再被中间空转挡打断
- `b9afca1d` **自动压缩一律静默**——不再打 "Tool output folded · saved ~N tok" 横幅
- `a8df1838` `/compact` 摘要防弱模型重跑旧问题 + 记录被压缩的图片

**CLI/TUIX 体验**:
- `6d9328df` `cli`: resume 指定会话(`-p --resume` / `resume` 子命令) + 退出展示会话 id
- `a4276cf2` `tuix`: /resume 放宽准备超时并修正误导文案
- `ee8d2dfb` `tuix`: Tab 接受下一条建议 + 输入框显示 `Tab:` 提示
- `67c9888b` `webui`: @ 提及支持递归模糊匹配
- `dbf75063` `tuix`: MCP 工具调用渲染优化——去 mcp 前缀 + 参数不转义/摘要化 + 剥离结果标题
- `d080cce5` `tuix`: 工具调用成功时 ● 圆点变绿(串行/并行/resume 一致)
- `f65994ad` `tuix`: 面板/spinner 标签统一驼峰(Subtasks→SubTasks 等)
- `c34bda74` `webui`: 强度下拉补上缺失的 xhigh 选项
- `e39fd066` `tuix`: Bash 多行命令保留换行 + 封顶 3 行
- `725defca` `tuix`: /provider 编辑账号保存后进入该账号模型列表

**Provider / 模型**:
- `33057fc7` revert next-prompt-suggestion cap to 128(1024 hit the gateway limit)
- `90085dae` raise next-prompt-suggestion token cap 让 reasoning 模型能 emit 建议
- `64e7f8f6` provider:为 OpenRouter 请求注入 app 归因头
- `9c3c443a` provider:is_openrouter_url 剥离 userinfo 防归因头泄漏 + 复用 host_matches_domain
- `13ab87f5` provider:死代理连不上时报错给出 /proxy 引导
- `04d3cc16` kernel:重试通知断言同步 ProviderRetry 结构化事件

**版本同步**:`287bff70` `chore: sync latest.json from release v5.0.9`、`52ca5e6c` merge release/v5.0.9 into main、`eda3e3ef` 更新 5.0.8→5.0.9、`525a9bf7` bump v5.0.9、`8c837360` merge release/v5.0.9 into main。

### 19.8 对 laew 的 P0/P1/P2 借鉴路线

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | Cancel 双路 | `CancellationToken + AtomicBool + cancel_pending(oneshot→Null)` 三件套,孤儿 tool_result 自动 backfill | agent.rs:1541-1624, message.rs:438-439, request.rs:84-94 |
| **P0** | 消息层 backfill | 取消后 `backfill_missing_tool_results("(cancelled)")`——协议硬一致 | message.rs:438-439 |
| **P0** | 持久化 4 件套 | jsonl + meta + inflight + snapshot,inflight 只在 prefix + 1 message 完全匹配时采纳 | manager.rs:2859-2895, 1235-1283 |
| **P0** | 进程间文件锁 | `fs2::FileExt::lock_exclusive` 防并发 append 损坏 | manager.rs:2859-2895 |
| **P0** | repair_pairing | resume seed 时调用,处理 dangling tool_call / orphan tool_result | agent.rs:1302 |
| **P1** | 双层压缩 | StubCompaction(内层静默 stub)+ OverflowCompaction(外层三阶梯)——自动/手动分离 | compaction.rs:101-214 |
| **P1** | 压缩摘要硬限 | MAX_SUMMARY_BYTES=64KiB,MAX_SUMMARY_TOKENS=16k;超时 180s 降级 | compaction.rs:143-192 |
| **P1** | 工具并发 | Semaphore + RwLock 双闸 + FuturesOrdered 保持 emission 顺序 | agent.rs:3480-3653 |
| **P1** | steer queue | 用户中途消息入 steer(下一轮 fold)而非 pending(打断 turn)——区分主动取消 vs 自然增信 | agent.rs:1606, 1663 |
| **P1** | 摘要 anchor | `<previous-summary>` 提示让模型续写而非重写,防弱模型退化 | compaction.rs:184-192, a8df1838 |
| **P2** | StopReason 12 变体 | 完备终态枚举,使单元测试可枚举 | event.rs:88-119 |
| **P2** | 触发阈值动态化 | 切换小窗口模型后用 `model.context_window()` 重算,不用历史 ratio | agent.rs:1113-1128 |
| **P2** | auto 静默化 | 自动压缩不打横幅,避免噪声 | b9afca1d |
| **P2** | split boundary 保留尾 | 防止"压缩后空转" | c8639008 |

## 20. 第六轮深挖 — Anthropic/OpenAI 协议 wire + 流式 + 错误重试

针对 atomcode capabilities 层(协议适配)的代码级深度调研,覆盖 `crates/atomcode-capabilities/src/provider/{anthropic,openai_compat,retry,reasoning,mod}.rs` 与 `crates/atomcode-kernel/src/{stream,event,provider,message}.rs` 的核心契约。**目标**:把"字节进出 + 状态机 + 错误恢复"这条最长的横向逻辑链完整画一遍,与第 4 章"协议适配"互为正交补充(第 4 章偏总体,本节偏"为什么这样写"的逐行理由)。

### 20.1 设计总览:LlmProvider + StreamEvent 协议中立契约

**为什么这么设计**(架构意图来自 `crates/atomcode-kernel/src/provider.rs:120-156` 的 `LlmProvider` trait 注释):

- **内核不感知协议**:turn loop 只调用一次 `chat_stream`,消费 `StreamEvent` 流,从不出现 `Anthropic`/`OpenAI` 字样。协议差异封闭在两个 `*_provider.rs` 实现里。这是第 3 章「Kernel 核心」已确立的"统一消息模型"原则。
- **`StreamEvent` 第一公民**:`stream.rs:93-156` 的 `enum StreamEvent` 不只是包络,它区分了"回放敏感"(replay-sensitive)与"回放安全"(metadata-only)的事件,以驱动透明重开(reopen)逻辑:
  ```rust
  pub enum StreamEvent {
      TextDelta(String),                              // replay-sensitive
      Reasoning(String),                              // replay-sensitive
      ReasoningSignature { opaque, provider },         // replay-sensitive(签名块)
      ToolCall(ToolCall),                             // replay-sensitive(执行依据)
      ToolCallDelta { index, id, name, arguments },   // replay-sensitive(UI 实时显示)
      Usage(TokenUsage),                              // metadata-only
      ResponseId(String),                             // metadata-only
      ResponseModel(String),                          // metadata-only
      Error(ProviderError),                           // terminal
      Malformed,                                      // 解析失败的 signal,非内容
      Done { truncated: bool },                       // terminal
  }
  ```
  `retry.rs:24-43` 的 `is_replay_sensitive_event` / `is_attempt_metadata_event` 把这两组事件分桶 — 后者可以延迟到下一次重连后再发出;前者一旦进入消费者,就**禁止**整体重发,否则会产生重复输出或重复工具副作用。
- **`TokenUsage::merge_max` 而非 last-wins**(`stream.rs:25-29`):Anthropic 在 `message_start` 给输入 tokens、`message_delta` 给累积输出 tokens,字段是**分裂**的;OpenAI 一次性给完整 usage。两种习惯同框出现时,last-wins 会把先前事件的字段清零(例如后续 delta 只携带 `completion` → `prompt` 被踩为 0),merge_max 取逐字段最大值,**两种风格都不重复计算、不丢失**。
- **`ProviderError` 五元组**(`stream.rs:36-51`):`retryable + message + http_status + code + retry_after_secs` — 把"是否重试""用户看得懂的描述""HTTP code""供应商结构化 code(如 model_not_found)""Retry-After 秒数"五件事分开,让上层(rate-limit 路径、checkpoint 路径、TUI 渲染)各自取需要的字段,**避免字符串匹配**。
- **`bind_session_id` 一次性绑定**(`provider.rs:141`):不是 setter,而是 `OnceLock`(两个 adapter 的 `session_id: OnceLock<String>`),因为 session id 在 Agent 的生命周期内不变,/session 切换是重建 Agent 而非原地换 id。空 ⇒ 头部省略(无亲和性的 sub-agent/summary)。

### 20.2 Anthropic wire 适配(`anthropic.rs`)

文件:`crates/atomcode-capabilities/src/provider/anthropic.rs`(2024 行,含 19 个 inline test + 2 个真 socket 端到端测试)。

#### 20.2.1 请求体 — 字节级确定性(BTreeMap-backed Map)

`build_request_body`(`anthropic.rs:400-474`):

```rust
let mut body = Map::new();                                    // BTreeMap-backed,排序序列化
body.insert("model".into(), json!(model));
body.insert("max_tokens".into(), json!(options.max_tokens.unwrap(cfg.max_tokens)));  // REQUIRED
body.insert("stream".into(), json!(true));
// sampling params 默认不发出(避免 Opus 4.7+ 400,见 send_sampling_params)
if cfg.thinking && !forces_tool_use {                          // forced tool_use 与 thinking 不兼容
    body.insert("thinking".into(), json!({ "type": "adaptive" }));
}
if let Some(effort) = options.reasoning_effort {
    body.insert("output_config".into(), json!({ "effort": effort_str(effort) }));  // 5 个常量
}
if !tools.is_empty() {
    body.insert("tools".into(), json!(tools.iter().map(|td| json!({
        "name": td.name, "description": td.description,
        "input_schema": td.parameters,                          // Anthropic 字段名
    })).collect::<Vec<_>>()));
}
```

**关键设计点**(对应文件的 30 行模块注释):

| 关注点 | 决策 | 原因 |
|---|---|---|
| 序列化字节稳定性 | `serde_json::Map`(BTreeMap-backed)+ 无时间戳/uuid | Anthropic 的 prompt cache 按字节前缀;同 `(system, messages, tools)` 必须序列化恒等 |
| `temperature` 默认不发 | `cfg.send_sampling_params: false` | Opus 4.7+ 已移除 sampling params,发则 400 |
| `thinking` 与 forced tool_use 互斥 | 单次请求抑制 thinking,下一轮恢复 | Anthropic 拒绝 thinking + tool_choice=any 同时存在 |
| `tool_choice` 映射 | Auto→省略、Required→`{type:"any"}`、Specific→`{type:"tool",name}`、None→`{type:"none"}` | 对齐 Anthropic 的 4 种对象/字符串形态 |
| `output_config.effort` | 5 档:low/medium/high/xhigh/max(对应 `effort_str` `anthropic.rs:677-685`) | kernel 端中立枚举 → 协议串 |
| `max_tokens` REQUIRED | `options.max_tokens.unwrap(cfg.max_tokens)` | Anthropic 拒绝无 `max_tokens` 请求 |

测试 `body_serialization_is_deterministic`(`anthropic.rs:1447-1490`)连续 100 次序列化同一输入,逐字节比对 — 这是 prompt cache 的硬约束。

#### 20.2.2 消息→wire 转换:三件易错事

`format_messages_with_vision`(`anthropic.rs:484-551`):

1. **System 提升到顶层**(Anthropic 没有 `role:"system"` 消息):
   ```rust
   let system_text: String = messages.iter()
       .filter(|m| m.role == Role::System)
       .map(|m| m.text.as_str())
       .collect::<Vec<_>>()
       .join("\n\n");
   let system = if system_text.is_empty() { None } else { Some(system_text) };
   ```
   所有连续的 `Role::System` 用 `\n\n` 拼成单个 string,挂到 body 的 `system` 字段。

2. **连续 Tool 结果折成一个 user 消息**(`anthropic.rs:519-539`):
   ```rust
   Role::Tool => {
       let mut blocks: Vec<Value> = Vec::new();
       while i < messages.len() && messages[i].role == Role::Tool {
           if let Some(id) = tr.tool_call_id.as_deref().filter(|s| !s.is_empty()) {
               blocks.push(json!({"type":"tool_result","tool_use_id":id,
                                  "content":tr.text,"is_error":tr.is_error}));
           }
           i += 1;
       }
       if !blocks.is_empty() { out.push(json!({"role":"user","content":blocks})); }
   }
   ```
   **N 个并行 tool_call** 的 N 个结果合并到**一个** user message 的 content blocks — Anthropic 不允许相邻 user/assistant 嵌套混乱。

3. **强制 user/assistant 严格交替**(`merge_consecutive_user` `anthropic.rs:556-601`):
   - Anthropic 协议硬约束:`messages` 必须 user↔assistant 严格交替
   - 但实际场景里:<system-reminder> 后缀、post-compaction synthetic summary、tail hook 都会注入**额外的** user 消息,把"上一轮 tool_result 折成的 user" 和 "本轮 reminder user" 拼到一起
   - 修复:扫描整个序列,把相邻的 `role:"user"` 全部合并;两条 string 拼接成 `\n\n` 一条 string;遇到 block/array 内容则合并成 block 数组(`merge_user_content`)
   - **测试 `no_consecutive_user_after_tool_fold_then_reminder`** (`anthropic.rs:1059-1093`) + `no_consecutive_user_post_compaction_summary_beside_user` (`anthropic.rs:1095-1117`) 锁住了这两条真实路径

#### 20.2.3 签名 thinking 块 — 跨协议不互译

Anthropic extended thinking 返回的 `signature` 是**供应商绑定**的;replay 给 OpenAI/Gemini 会失败。`format_assistant_message` (`anthropic.rs:637-675`):

```rust
let echoable = |b: &ReasoningBlock| {
    echo_thinking && b.provider.as_deref() == Some("anthropic")    // 只回 anthropic 的
};
// ... 空 text + opaque ⇒ redacted_thinking(data=opaque)
// ... 正常 text + opaque ⇒ thinking(thinking=text, signature=opaque)
```

`ReasoningBlock` 的 `provider` 字段(`message.rs:75-94` 的 `INVARIANT: opaque.is_some() ⇒ provider.is_some()`)就是为这个准备的;foreign / `None` block 一律不回。

测试矩阵(`anthropic.rs:1198-1298`)覆盖了:
- `signed_thinking_is_echoed_when_enabled` ✓(anthropic + text)
- `signed_thinking_is_dropped_when_thinking_disabled` ✓(echo_thinking=false)
- `foreign_provider_thinking_block_is_not_echoed` ✓(openai 来源 / provider=None)
- `mixed_provider_blocks_echo_only_anthropic_ones_in_order` ✓(混合)

#### 20.2.4 SSE 解码:有状态、按块鲁棒

`AnthropicSseDecoder`(`anthropic.rs:756-1024`)是一个**有状态**的字节→事件映射器:

- **按字节切分而非按行 split**:`feed` 中 `self.buf.extend_from_slice(chunk); while let Some(pos) = self.buf.iter().position(|&b| b == b'\n')` — 即便 chunk 把一行 UTF-8 中文字符切成多段,也不会丢字符。`sse_byte_split_robust_and_utf8_safe` 测试(`anthropic.rs:1696-1722`)对比了"整段喂入"vs"逐字节喂入"两条路径的事件流,逐字符相等。

- **`blocks: Vec<BlockState>` 按 index 跟踪**(Anthropic SSE 用 index 而非 id):
  ```rust
  fn block_mut(&mut self, index: usize) -> &mut BlockState {
      while self.blocks.len() <= index { self.blocks.push(BlockState::default()); }
      &mut self.blocks[index]
  }
  ```
  - tool_use 累积 `input_json`(partial_json 字符串拼)
  - thinking 累积 `signature`(signature_delta 字符串拼)
  - redacted_thinking 累积 `redacted_data`

- **content_block_stop 一次性 take**:把 `std::mem::take(&mut self.blocks[index])` 整块拿出来,避免后续 index 复用时的状态污染。

- **usage 三段拼**:
  - `message_start`: `input_tokens + cache_read_input_tokens + cache_creation_input_tokens`(`anthropic.rs:850-866`)
  - `message_delta`: 输出 tokens 是累积的,取最大值(`if o > 0 { self.output_tokens = o }`,`anthropic.rs:984-999`)
  - `message_stop`: 发 `Usage(self.usage())` + `Done { truncated }`(`anthropic.rs:1001-1007`)
  - 最终 `usage()`(`anthropic.rs:820-826`):
    ```rust
    TokenUsage { prompt: input_tokens + cache_read + cache_creation,
                 completion: output_tokens,
                 cached: cache_read }
    ```
  - 这与 `TokenUsage::merge_max` 的设计完美契合:`prompt/completion/cached` 三个独立字段,merge_max 会"取最大",不会因分裂上报而丢字段。

- **finish() 兜底**:流被截断不收到 `message_stop` 时,`finish()` 刷最后的 usage + `Done`(`anthropic.rs:801-814`)。

- **truncated 标记**:`message_delta.delta.stop_reason == "max_tokens"` ⇒ `self.truncated = true`(`anthropic.rs:984-993`)。

- **mid-stream error**:收到 `event.type == "error"` ⇒ `StreamEvent::Error + self.done = true`(`anthropic.rs:1008-1020`);非 retryable,因为上半段可能已发出。

#### 20.2.5 Open + 重试 + 透明重开:三档机制

`AnthropicProvider::chat_stream`(`anthropic.rs:167-299`)的实现揭示了"**3 层重试叠加**":

```text
┌─ 顶层:Kernel turn-loop OPEN retry (外层,只 Err 才触发)
│
├─ open_stream() 内 RetryPolicy (status/transport 错误,408/425/429/500/502/503/504/529)
│
└─ 'reopen loop: MAX_STREAM_ATTEMPTS=3  (流中途 reset 重连,只在未发出 replay-sensitive 前)
```

**OPEN retry**(同 `openai_compat.rs`,复用 `retry.rs`):

```rust
// anthropic.rs:329-345
let send = match tokio::time::timeout(open_timeout, req.send()).await {
    Err(_elapsed) => {
        if attempt < policy.max_attempts {
            tokio::time::sleep(retry::compute_backoff(attempt, policy)).await;
            attempt += 1; continue;
        }
        return Err(ProviderError { retryable: true,
            message: format!("open failed: 等待首字节超过 {}s(网关无响应)", open_timeout.as_secs()),
            ..Default::default() });
    }
    Ok(r) => r,
};
```
- **`open_timeout` 是 per-attempt TTFB watchdog**,不绑定整体 retry 循环 — 慢但活着的网关仍拿到满 budget
- `Retry-After` 在解析 body 之前先抓 `retry::parse_retry_after(resp.headers())`(`anthropic.rs:362-365`),避免 `text()` 消费 resp 后无法读 header — 这是 429 自愈的关键

**Mid-stream 重连**(`anthropic.rs:211-296` 的 `'reopen: loop`):
```rust
const MAX_STREAM_ATTEMPTS: u32 = 3;
let mut stream_attempt = 1u32;
let mut reconnect_attempts = 0u32;
let mut emitted_replay_sensitive = false;
let mut pending_metadata = Vec::new();              // 缓存 id/model/usage
loop {
    let mut dec = AnthropicSseDecoder::new();
    let byte_stream = resp.bytes_stream();
    futures::pin_mut!(byte_stream);
    loop {
        match tokio::time::timeout(idle, byte_stream.next()).await {
            Err(_elapsed) => {
                yield StreamEvent::Error(... "stream idle timeout"); return;   // 中途 idle 不重试
            }
            Ok(None) => { /* 流 EOF:刷 pending_metadata + finish */ return; }
            Ok(Some(Err(e))) => {
                if !emitted_replay_sensitive && stream_attempt < MAX_STREAM_ATTEMPTS {
                    tokio::time::sleep(compute_backoff(...)).await;
                    if let Ok(fresh) = open_stream(...).await {
                        stream_attempt += 1; resp = fresh; continue 'reopen;   // 透明重连
                    }
                }
                yield StreamEvent::Error(... "网络连接中断:... 自动重连 N 次后仍失败"); return;
            }
            Ok(Some(Ok(chunk))) => { /* feed 进 decoder,产生 events */ }
        }
    }
}
```

**`emitted_replay_sensitive` 闸门**(`anthropic.rs:229-241`):
```rust
if retry::is_attempt_metadata_event(&ev) {
    pending_metadata.push(ev); continue;                    // id/model/usage 缓存
}
if retry::is_replay_sensitive_event(&ev)
   || matches!(ev, StreamEvent::Done {..} | StreamEvent::Error(_)) {
    for metadata in pending_metadata.drain(..) { yield metadata; }   // 冲刷累积 metadata
}
emitted_replay_sensitive |= retry::is_replay_sensitive_event(&ev);
yield ev;
```

**关键不变量**:
- 流未发出任何 text/tool/reasoning 才允许整条重发
- metadata(id/model/usage)是**延迟到下一次重连后再发**,因为这是当前 attempt 的 metadata — 但重连后丢弃就丢失了元数据,所以集中缓存后冲刷
- mid-stream idle timeout 永远非 retryable(下半段可能已发出,partial-response 风险)
- 真 socket 测试 `midstream_eof_before_any_event_reopens_and_succeeds`(`anthropic.rs:1823-1883`)+ `midstream_reset_twice_before_any_event_reopens_until_success`(`anthropic.rs:1885-1947`)用 TCP listener 真实重现"网关 LB 在压力下连续 reset 两条连接"场景

### 20.3 OpenAI compat 适配(`openai_compat.rs`)

文件:4093 行(被截断读到 2250 行,后续已通过调用点+测试理解全貌)。

#### 20.3.1 与 Anthropic wire 的根本差异

| 维度 | Anthropic | OpenAI compat |
|---|---|---|
| **认证头** | `x-api-key` + `anthropic-version`(`anthropic.rs:320-321`) | `Authorization: Bearer {key}`(`openai_compat.rs:770`) |
| **system 字段** | 顶层 `system` 字符串 | 消息 `{role:"system",content}` |
| **tool 定义** | `input_schema`(对象) | `function.parameters`(对象,JSON Schema 嵌套必须 `properties` 键) |
| **tool_call args** | 顶层 `tool_use.input` **对象**;`input_json_delta` 累积字符串 | `tool_calls[].function.arguments` **字符串**;`tool_calls[].function.name/arguments` 增量 |
| **content 类型** | `content` 数组块(text/tool_use/thinking) | `content` 字符串(可与 `tool_calls` 并列) |
| **流事件** | `message_start` / `content_block_start/delta/stop` / `message_delta` / `message_stop` | `choices[0].delta.content` + `choices[0].finish_reason` + 末尾 `usage` |
| **finish_reason** | `end_turn` / `max_tokens` / `tool_use` / `stop_sequence` | `stop` / `tool_calls` / `length` / `content_filter` / **`""`(空串,见 20.3.4)** |
| **reasoning** | `thinking` content block + `signature` 签名 | `delta.reasoning_content` 字符串,**无签名** |
| **usage 报告时机** | 入口 `message_start` 给输入 + 末尾累积 `message_delta` | 仅末尾单条 `usage`(需 `stream_options.include_usage=true`) |
| **cache 字段** | `cache_read_input_tokens` + `cache_creation_input_tokens` | 字段不固定:OpenAI→`prompt_tokens_details.cached_tokens`、GLM→`cached_tokens`、DeepSeek→`prompt_cache_hit_tokens` + `prompt_cache_miss_tokens` |

#### 20.3.2 请求体构造 — 工具 schema 强制补 `properties`

`build_request_body`(`openai_compat.rs:1081-1151`):

```rust
body.insert("stream".into(), json!(true));
body.insert("stream_options".into(), json!({ "include_usage": true }));   // 必须,否则末尾无 usage
// ...
if let Some(mt) = options.max_tokens.or(cfg.max_tokens) {                 // OAI max_tokens 可省
    body.insert("max_tokens".into(), json!(mt));
}
if supports_tool_choice(model) {                                          // DeepSeek-V4 拒绝 tool_choice!
    match &options.tool_choice {
        ToolChoice::Auto => {},                                            // 省略 = byte-identical "no opinion"
        ToolChoice::Required => { body.insert("tool_choice".into(), json!("required")); }
        ToolChoice::Specific(name) => { body.insert("tool_choice".into(),
            json!({"type":"function","function":{"name":name}})); }
        ToolChoice::None => { body.insert("tool_choice".into(), json!("none")); }
    }
}
```

**`normalize_openai_tool_schema`**(`openai_compat.rs:1160-1216`)— 关键适配:

LM Studio 等严格校验器**拒绝** `{"type":"object"}` 而无 `properties` 键。`recursive` 遍历:
```rust
fn normalize_openai_tool_schema_in_place(schema: &mut Value) {
    let Value::Object(map) = schema else { return; };
    let allows_object = match map.get("type") {
        Some(Value::String(k)) => k == "object",
        Some(Value::Array(ks)) => ks.iter().any(|k| k == "object"),
        _ => false,
    };
    if allows_object && !map.contains_key("properties") {
        map.insert("properties".into(), Value::Object(Map::new()));   // 补空 map,非 undefined
    }
    // recurse into properties / items / anyOf / oneOf / allOf / $defs
    // 但 const/enum/default/examples 这些字面值保持原样
}
```
**注释明确**: literal-bearing 关键字(const/enum/default/examples)的 data 虽然"看起来像 schema",但 schema validator 要按字面值处理,**不能改**。

**`supports_tool_choice` 模型谓词**(`openai_compat.rs:1221-1224`):
```rust
fn supports_tool_choice(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    !m.contains("deepseek-v4")
}
```
DeepSeek-V4 思考模式拒绝 `tool_choice` 控制字段(尽管仍接受 auto 模式的 tools)。这是模型协议约束而非端点属性 — 注释明示"通过不同 gateway 暴露同一 model id 也可能不同"。

**`reasoning_effort` session-scoped 自适应**(`openai_compat.rs:507-518, 1240-1259`):
```rust
// Provider 字段
effort_unsupported: std::sync::atomic::AtomicBool,

// chat_stream 入口:
let effort_known_unsupported = self.effort_unsupported.load(Ordering::Relaxed);
let stripped_opts;
let options = if effort_known_unsupported && options.reasoning_effort.is_some() {
    let mut o = options.clone();
    o.reasoning_effort = None;
    stripped_opts = o;
    &stripped_opts
} else { options };
```

SenseNova 接受 low/medium/high/xhigh 但拒绝 `max`(DeepSeek API 才接受);一旦 400,后续整个 session 都剥掉这个字段,避免每个 turn 触发同样的 400。

**`push_system_coalesced` 共用辅助**(`provider/mod.rs:110-124`):
```rust
pub(crate) fn push_system_coalesced(out: &mut Vec<Value>, text: &str) {
    if let Some(last) = out.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some("system") {
            // 拼到上一条 system(用 \n\n);保证多个 system 角色消息合并成 1 条
        }
    }
    out.push(json!({"role":"system","content":text}));
}
```
- Strict chat template(Qwen/vLLM)**只接受 1 条 system**
- 注释:Anthropic 路径有自己顶层 `system` 字段的不同保证;这条 helper 只给走"role:system"消息的两条协议用

**`is_openrouter_url` 归因头**(`openai_compat.rs:63-83`):
- 仅当目标 host 是 `openrouter.ai`(用 `host_matches_domain` 共享谓词)时,追加 `HTTP-Referer / X-OpenRouter-Title / X-OpenRouter-Categories`
- 显式剥 `userinfo@` 防 `https://openrouter.ai:x@evil.com/...` 类型的归因头泄漏到 evil.com
- 注释解释 hand-rolled host 提取:不引入 `url` crate,因为那是 optional dependency 在 web/mcp 下才用,`provider` 不能依赖

#### 20.3.3 SSE 解码 — buffering + last-wins + 容错

`SseDecoder`(`openai_compat.rs:1418-1636`):

- **tool_calls 累积**:`self.tool_calls: Vec<(String, String, String)>` 按 index 跟踪 `(id, name, accumulated_args)`,在 `finish_reason` 非空时**整批** flush(`openai_compat.rs:1622-1630`)
- **last-wins usage**:`last_usage: Option<TokenUsage>`;每条 chunk 覆盖(`openai_compat.rs:1549-1551`),`finish` 时发出
- **`usage` 多源归一化**(`map_usage` `openai_compat.rs:1638-1657`):
  ```rust
  let cached = u.prompt_cache_hit_tokens
      .or(u.cached_tokens)
      .or_else(|| u.prompt_tokens_details.and_then(|d| d.cached_tokens))
      .unwrap_or(0);
  let prompt = u.prompt_tokens.unwrap_or_else(|| {
      u.prompt_cache_hit_tokens.unwrap_or(0)
          .saturating_add(u.prompt_cache_miss_tokens.unwrap_or(0))
  });
  ```
  OpenAI `prompt_tokens_details.cached_tokens` / GLM `cached_tokens` / DeepSeek `prompt_cache_hit_tokens + prompt_cache_miss_tokens` 三种字段名归一为 `{prompt, completion, cached}`。DeepSeek 风格可能省略 `prompt_tokens` 只给精确缓存拆分,需 fallback 重算 total。

- **`[DONE]` 哨兵走 finish**:`openai_compat.rs:1496-1505` — 收到哨兵时调 `self.finish()`,避免只报告 `finish_reason:""`(见 20.3.4)的网关漏掉 buffered tool_call

- **Malformed 信号**:`openai_compat.rs:1519` — 非空、非 `[DONE]`、非 JSON 的 `data:` 行 ⇒ `StreamEvent::Malformed`(空载荷)。让 kernel 区分"网关发垃圾"vs"网关发空",否则两者 retry 一样,后者修了前者没修。注释强调:`Malformed` 不算内容,不算 round 的输出量。

#### 20.3.4 finish_reason:"" 是 bug 触发点 — 必须非空才算终止

`openai_compat.rs:1613-1634`:
```rust
// Only a NON-EMPTY finish_reason is terminal. SenseNova's free `deepseek-v4-flash`
// sends `"finish_reason":""` (empty string, not null) on EVERY chunk — including the
// reasoning and tool_call-fragment chunks that precede the real `"tool_calls"`. Arming
// `seen_finish` on the empty string makes the `if self.seen_finish { return }` guard
// above discard every subsequent tool_call delta, so the whole call is dropped and the
// model shows "0 工具". Treat "" as non-terminal.
if let Some(fr) = choice.finish_reason.filter(|s| !s.is_empty()) {
    self.seen_finish = true;
    for (id, name, args) in std::mem::take(&mut self.tool_calls) {
        // ... 整批 flush
    }
    if fr == "length" { self.truncated = true; }
}
```
- `finish_reason = null` ⇒ `filter()` 自动过滤
- `finish_reason = ""`(空串) ⇒ `filter(|s| !s.is_empty())` 过滤
- `finish_reason = "tool_calls" / "stop" / "length" / "content_filter"` ⇒ terminal

`MAX_TOOL_CALL_DELTAS: usize = 20000`(`openai_compat.rs:1412`)— 防止异常 provider 无限堆 delta;到达上限后自动 finish。

#### 20.3.5 Mid-stream 重连 + SwappableClient 池子修复

OpenAI-compat 的 mid-stream reopen 流程(`openai_compat.rs:582-712`)与 Anthropic 几乎对称,但多了一个**关键修复**:`SwappableClient`:

```rust
// openai_compat.rs:444-480
pub(crate) struct SwappableClient {
    current: std::sync::RwLock<reqwest::Client>,
    build: Box<dyn Fn(bool) -> Result<reqwest::Client, ProviderError> + Send + Sync>,
}
// rebuild() 构造全新 client → EMPTY pool → 原子换入
```

**触发场景**(注释 `openai_compat.rs:228-243`):
- 池中 keep-alive 连接被 LB 静默关闭
- 下次请求复用 → write OK / read → ConnectionReset
- **每个 retry 重用同一个池** → 持续失败直到重启 / `/login`(原方案)
- 自动方案:mid-stream 收到 stale-conn 错误时,先 `client.rebuild(...)` 拿到 empty pool,再 reopen

`rebuild` 路径(`openai_compat.rs:650-665`):
```rust
if retry::is_stale_connection_error(&e) || retry::chain_has_tls_corruption(&e) {
    if let Err(rebuild_error) = client.rebuild(atomcode_config::tls::should_cap_url(&url)) {
        yield StreamEvent::Error(rebuild_error); return;
    }
}
if let Ok(fresh) = open_stream(&client, ...).await {
    stream_attempt += 1; resp = fresh; continue 'reopen;
}
```

**`build_http_client` 双层 trust-root backstop**(`openai_compat.rs:282-367`)— issue #514 修复:
- 第一层:webpki base + OS native store + `SSL_CERT_FILE`(通过 `add_trusted_roots`)
- 第二层(失败时):bare webpki base only — `tracing::warn!(... backstop (issue #514) ...)`,公共 CA 端点仍能工作,corporate MITM 根丢失但客户端不会死

**`add_trusted_roots`** (`openai_compat.rs:375-436`):
- **每个 OS 根都过一遍 rustls 验证**:`rustls::RootCertStore::empty().add(der.clone()).is_err()` ⇒ 跳过。**关键**:reqwest 的 `Certificate::from_der` 不验证,延迟到 `.build()` 内部,只要 rustls 拒绝一个就**整个 client 死掉**。预筛选把 "一个坏 OS 根搞死所有 provider" 这类问题隔离
- `SSL_CERT_FILE` 不是预筛选(missing PEM 字节手头没有),malformed 时 `.build()` 失败 ⇒ 触发 backstop 重试

### 20.4 重试框架核心(`retry.rs`)

文件:1081 行,几乎全是用例测试(纯函数 + 异常链 walking + backoff 数学)。

#### 20.4.1 重试策略表

`is_retryable_status`(`retry.rs:92-94`):
```rust
matches!(code, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
// 含 529(Anthropic-style Overloaded,某些 OpenAI 兼容网关透传)
```

`should_retry_open_status`(`retry.rs:99-101`)— 429 走 per-call owner:
```rust
is_retryable_status(code) && (code != 429 || owner == RateLimitRetryOwner::Provider)
```
- `RateLimitRetryOwner::Provider`(默认):provider 内部 3 次 fast retry 后放弃 → 报 Err 给 kernel
- `RateLimitRetryOwner::Kernel`:kernel 第一次 429 就直接 surface → 走 lifecycle-aware 策略(可取消等待、倒计时、fuse)。direct consumer(provider 直接调用,如 SDK 测试)用 Provider;turn loop 用 Kernel

#### 20.4.2 错误分类:三层 walking

`is_retryable_reqwest_error`(`retry.rs:119-124`):
```rust
err.is_timeout()
    || err.is_connect()
    || is_stale_connection_error(err)        // 走 source chain
    || chain_has_tls_corruption(err)         // 走 source chain
```

**`chain_has_transient_io`**(`retry.rs:136-156`)— 沿 `source()` 链找 `io::Error`:
```rust
matches!(io.kind(), ConnectionReset | ConnectionAborted | BrokenPipe
                   | UnexpectedEof | NotConnected | TimedOut)
// 含 TimedOut:Linux ETIMEDOUT(110)/Windows 10060 — 流中途死
```

**为什么需要 chain walking**(`retry.rs:108-118` 注释):
- `err.is_timeout() || err.is_connect()` 太窄:reqwest 只在**连接建立阶段**失败时 `is_connect()==true`
- 真实场景:keep-alive 连接被 LB 静默关闭,复用时 `is_connect()==false`,底层 `io::Error(ConnectionReset)` 在 source 链
- 老逻辑(只 `is_timeout() || is_connect()`)→ 错分类为 fatal → 报"open failed",用户不得不 `/login`(手清池)

**`chain_has_tls_corruption`**(`retry.rs:181-194`):专门检测 rustls 的两类 BadRecordMac:
- 方向 1:peer 拒绝我们发的记录 → 我们收到 fatal alert `received fatal alert: BadRecordMac / DecryptError`
- 方向 2:中间件乱改 peer 给我们的记录 → 我们 `cannot decrypt peer's message`(rustls 0.23 error.rs:991 的字符串)
- 注释明示:**排除** `HandshakeFailure`(握手失败归 `is_connect()` 路径,不归 corruption 类 — TLS-1.2 降级对此无解)
- 注释明示:`InvalidData` 可能是 body 损坏(逻辑失败,**不应**重试);所以检测靠 alert 字符串而非 `io::ErrorKind::InvalidData`

**`chain_has_incomplete_message`**(`retry.rs:205-216`)— hyper 的 `Error::is_incomplete_message`:
- "connection closed before message completed"
- **不包 `io::Error`**(`is_stale_connection_error` 通过 `chain_has_transient_io || chain_has_incomplete_message` 统一收纳)

**`is_stale_connection_error`**(`retry.rs:226-228`):
```rust
chain_has_transient_io(err) || chain_has_incomplete_message(err)
```
**重放风险注释**(`retry.rs:222-225`):"Neither form proves the server failed to receive or process the request. Retrying may cause a duplicate inference (and duplicate billing). This is accepted deliberately" — 选择"接受重放"作为不完美的妥协,理由是"用户已等几分钟再硬失败"更糟。**注意**:不允许文档化为"安全/幂等"重放。

#### 20.4.3 Backoff:指数 + 真实 ±25% jitter

`compute_backoff`(`retry.rs:411-413`)→ `compute_backoff_jittered`(`retry.rs:419-434`):
```rust
let exp = policy.base_delay.saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
let capped = exp.min(policy.max_delay);
// ±25% 窗口:capped_ms/2 = window_ms
// jitter in [0.0, 1.0) → offset in [0, window_ms)
// result = (capped_ms - window_ms/2) + offset = [floor..floor+window_ms)
```
- **真实 jitter**(anti-thundering-herd):`random_jitter_fraction()`(`retry.rs:440-446`)用 `SystemTime::now().subsec_nanos()`,跨进程/跨调用去相关;**不引入 `rand` crate**
- 测试 `backoff_random_source_stays_within_window`(`retry.rs:624-640`)1000 次随机 jitter,确认 100% 在 ±25% 内
- 测试用 `compute_backoff_jittered(attempt, policy, fraction)` 注入固定 jitter,**不污染生产 randomness**
- sub-2ms delays:`window_ms=0`,jitter no-op,直接返回 `capped`,不会 underflow

#### 20.4.4 Retry-After 双格式

`parse_retry_after`(`retry.rs:387-400`):
- delta-seconds:`Retry-After: 3` ⇒ `Duration::from_secs(3)`
- HTTP-date:`Retry-After: Wed, 21 Oct 2015 07:28:00 GMT` ⇒ `when - now`(过期 ⇒ `Duration::ZERO`)
- 不解析/缺失 ⇒ `None` ⇒ 用 `compute_backoff` 兜底

#### 20.4.5 友好的中文错误

`stream_read_error_message`(`retry.rs:254-277`)三档 lead:
- `RetryExhausted { attempts }` → "网络连接中断:远端关闭或重置了连接,自动重连 {N} 次后仍失败,可重试。"
- `PartialResponse` → "响应中断:为避免重复输出或工具执行,未自动重放;已保留可安全保存的部分回复,可继续。"
- 逻辑错误(非 transport)→ "stream read error: <chain>"(verbatim)

Windows `os error 10054`(WSAECONNRESET)→ 追加 "此错误常见于公司网络或代理环境"(`connection_reset_hint` `retry.rs:284-293`),macOS 54 / Linux 104 → **不**加这个 hint(避免误指 proxy)。**locale-independent**:用数字代码而非中文字符串。

**`open_failed_message`**(`retry.rs:373-381`):拼 `err_chain` + proxy unreachable hint。
- `proxy_unreachable_hint` 检测 hyper-util `TunnelError::ConnectFailed` 标记("failed to create underlying connection"),**只**在 proxy CONNECT 路径触发;直接连失败不触发。
- 注释明示 marker 是 transitive-dep Display string(`"tunnel error: failed to create underlying connection"`),future hyper-util bump 可能改写;fails open (hint 静默消失),never crash。

#### 20.4.6 Replay-sensitivity 边界

`is_replay_sensitive_event`(`retry.rs:24-33`):
```rust
matches!(event,
    TextDelta(_) | Reasoning(_) | ReasoningSignature {..}
    | ToolCall(_) | ToolCallDelta {..})
```

`is_attempt_metadata_event`(`retry.rs:38-43`):
```rust
matches!(event,
    ResponseId(_) | ResponseModel(_) | Usage(_))
```

**注释要点**:`Usage` 既 replay-safe(数字不重复,字段可 merge)又算 metadata — 它延迟到下次重连后再发出不会被吞;`Malformed` 两者都不算(纯 signal,内容无关)。

### 20.5 跨协议 reasoning round-trip 策略(`reasoning.rs`)

文件:183 行,几乎全测试。

#### 20.5.1 三类厂商的 wire 行为

`reasoning.rs:8-22` 注释明示:

```text
deepseek-v4*           REQUIRES reasoning_content echoed on assistant tool-call
                       turns (HTTP 400 "must be passed back" otherwise); an empty
                       string is rejected, so a non-empty REASONING_PLACEHOLDER is
                       sent when no reasoning was captured.
deepseek-r1/reasoner   FORBIDS echoing reasoning_content (HTTP 400 if sent).
GLM / everything else  safe default: do not echo (GLM does not error either way;
                       omitting keeps requests minimal).
```

`ReasoningPolicy::derive`(`reasoning.rs:68-91`):
```rust
pub fn derive(model: &str, base_url: &str) -> Self {
    let m = model.to_ascii_lowercase();
    let u = base_url.to_ascii_lowercase();
    if m.contains("deepseek-reasoner") || m.contains("deepseek-r1") {
        ReasoningPolicy::Exclude
    } else if m.contains("deepseek-v4") {
        ReasoningPolicy::Include
    } else if m.starts_with("kimi-") || m.starts_with("moonshot")
              || m.starts_with("mimo-")
              || u.contains("moonshot") || u.contains("kimi")
              || u.contains("xiaomimimo") || u.contains("mimo") {
        ReasoningPolicy::Include
    } else {
        ReasoningPolicy::Exclude
    }
}
```
**坑**:r1 即使在 moonshot host 上仍 wins(`reasoning.rs:120-124` 测试);MiMo 用 model 名(`mimo-v2.5-pro` 在 generic gateway)也 Include(复用 DeepSeek-V4 协议)。

#### 20.5.2 REASONING_PLACEHOLDER 选用 `·`(单字符)

`reasoning.rs:32`:
```rust
pub const REASONING_PLACEHOLDER: &str = "·";   // 不是英文句子!
```
注释(`reasoning.rs:25-31`)解释:历史里若全用英文占位句,**DeepSeek-V4-Flash 会 MIMIC 它**当作自己的 assistant text 输出,stalling the turn。bare middle-dot 满足"非空"硬要求又**不**给模型可模仿的散文。

#### 20.5.3 与 Anthropic `signature` 模式的对比

| 维度 | OpenAI compat (reasoning.rs) | Anthropic (anthropic.rs:signature_delta) |
|---|---|---|
| 形态 | `delta.reasoning_content: String` 平铺 | `signature_delta` 累积 opaque signature + provider 标签 |
| 多块? | 1 个 message 1 个 reasoning 字符串 | `reasoning_blocks: Vec<ReasoningBlock>`,每个块独立 opaque + provider |
| 跨厂商 echo | **不** echo(同协议同 wire 即够) | **必须**按 `provider` 字段过滤(anthropic 的 opaque 给 openai 会 400) |
| 占位符 | Include 模式用 `REASONING_PLACEHOLDER` | redacted_thinking 用 `data = opaque`(空 text + opaque) |

### 20.6 Wire Dump 与友好错误

#### 20.6.1 wire_dump_request(`provider/mod.rs:61-93`)

- 环境变量 `ATOMCODE_WIRE_DUMP=1` 开启
- 输出到 `<config_dir>/wire-dump/<seq>-<ts>-<safe_model>.req.json`
- 关键:**byte-level 保真** — `serde_json::to_string_pretty` 写回文件,parser 验证 round-trip 相等(`provider/mod.rs:216-217` 测试)
- model 名 sanitize:`/:` → `_`(防路径逃逸,`provider/mod.rs:78-83`)
- 适配器级(Known JSON shape 各异),不是 kernel 级(内核有 `WireLogHooks` 但只 log kernel `Message` 视图)

#### 20.6.2 friendly_http_error(`provider/mod.rs:141-156`)

集中处理认证/余额:
- `401` → "API key 未授权或已失效(HTTP 401)"
- `402` → "账户余额不足(HTTP 402)"
- `403` + 含 "user has no codingplan" → "CodingPlan 未领取或已失效(HTTP 403)。请运行 /login 重新登录并领取 CodingPlan。"
- 其他 `403` → 保留 raw detail(AtomGit 用 403 表 session-concurrency 冲突,structured reason 必须活下来)
- `429` → 必须保留字面 `HTTP 429: ` 前缀,因为 kernel 的 rate-limit 路径用前缀剥法提取 `server_message`
- 其他 → `HTTP {code}: {detail}`

**注释明示**(`provider/mod.rs:135-140`):401/402 的 detail **故意**丢,因为 headline 已经说明;短形式能折成 `✗ 已中断:账户余额不足(HTTP 402)` 干净地塞进中断摘要。

### 20.7 与其它项目的横向对比

#### 20.7.1 与 claudecode(TypeScript / Bun)

| 维度 | atomcode | claudecode |
|---|---|---|
| 协议抽象 | 单一 `LlmProvider` trait + 2 个实现 | 适配器模式,每个 provider 类 |
| 流事件中立化 | 强(`StreamEvent` 11 变体) | 弱(直接用各 provider 原生事件) |
| mid-stream 重连 | 是(`MAX_STREAM_ATTEMPTS=3` + `emitted_replay_sensitive` 闸) | 否(失败即整轮 fail) |
| 透明 HTTP 池修复 | 是(`SwappableClient::rebuild`) | 否(手动 `/login`) |
| TLS-1.2 自动降级 | 是(`tls.rs latch_managed_tls12` + managed endpoint 检测) | 否 |
| reasoning 跨协议 | Anthropic `signature` by provider 字段 / OAI `reasoning_content` flat | 主要面向 Anthropic |
| 缓存报告归一 | 是(`prompt` = input + cache_read + cache_creation) | 部分(OpenAI 风格) |

#### 20.7.2 与 opencode(TypeScript / Bun)

| 维度 | atomcode | opencode |
|---|---|---|
| 字节稳定性 | 强制(BTreeMap + 确定性 build) | 无(可能影响 cache 命中率) |
| 错误分类粒度 | 5 元组(`retryable + message + http_status + code + retry_after_secs`) | 主要 retryable boolean + message |
| backoff | 真 jitter(防 thundering-herd) | 大多确定性 |
| tool schema 强校验补 | 是(`normalize_openai_tool_schema` 递归补 `properties`) | 否 |
| `tool_choice` per-model 谓词 | 是(`supports_tool_choice` 排除 DeepSeek-V4) | 否 |

#### 20.7.3 与 deepseek-harness(TypeScript)

| 维度 | atomcode | deepseek-harness |
|---|---|---|
| multi-provider fanout | 同一 `LlmProvider` 单实现串行 | `Cordis` everything-is-a-plugin,Fiber epoch 并发 |
| usage 归一 | 三源(`prompt_tokens_details` / `cached_tokens` / `prompt_cache_hit_tokens`)归 `{prompt, completion, cached}` | 各 provider 各用各的,不下沉 |
| 重试窗口 | OPEN 3 次 + reopen 3 次 + kernel turn-loop 三层 | Plugin-level 装饰,自定义 |
| 透明 keep-alive 修复 | 自动(`SwappableClient`) | 罕有 |

#### 20.7.4 与 pi(TypeScript)

| 维度 | atomcode | pi |
|---|---|---|
| 流协议中立化 | 强(`StreamEvent`) | 中等 |
| 中途协议破坏修复 | 透明 reopen(replay-sensitivity 闸) | 较少 |
| retry-after 解析 | 完整(delta-seconds + HTTP-date) | 通常 delta-seconds only |
| 错误信息本地化 | 是(`网络连接中断` lead + 详细 chain) | 否 |

### 20.8 对 laew 的 P0/P1/P2 借鉴路线

#### P0(立即)

1. **请求体字节稳定性** — 把 Anthropic/OpenAI 请求体的 key 序列固定成 BTreeMap-backed 或 literal 顺序,消除 HashMap 随机性。laew 当前 `llm/anthropic.rs` 用 `serde_json::json!({...})`(literal map)已稳定;`llm/openai.rs` 同。建议**显式注释"prefix-cache-safe"** 并加 single-round 序列化等性测试(类似 atomcode 的 `body_serialization_is_deterministic`),避免未来加字段时引入 HashMap 顺序漂移。

2. **TTFB watchdog** — `tokio::time::timeout(open_timeout, req.send())`,per-attempt,不绑整体 retry。laew 当前流式请求**没有** TTFB 上限,gateway 收连接不回响应的边界情况会**永久挂起**(`crates/atomcode-capabilities/src/provider/anthropic.rs:329-345` `open_timeout: Duration` 字段)。建议至少为 OpenAI-compat 加 60s TTFB 上限。

3. **mid-stream replay 闸** — 流中断时**不能**整条重发已有内容。laew 当前 `agent/mod.rs` 拿到 provider response 后简单转发给 tool 循环,没有 replay-tracking。建议实现 `emitted_replay_sensitive: bool` 闸:第一次发出任何 tool_call / text_delta 后,流错误只 surface 不重连(参考 `crates/atomcode-capabilities/src/provider/openai_compat.rs:582-712` 的 `'reopen: loop`)。

4. **`finish_reason:""` 兼容** — `crates/atomcode-capabilities/src/provider/openai_compat.rs:1613-1634` 处理 SenseNova 的 free `deepseek-v4-flash` 每个 chunk 都报空串 `finish_reason`,**必须**用 `filter(|s| !s.is_empty())`。laew 当前 `llm/openai.rs` 解析时若裸 `finish_reason == ""` 会和 `finish_reason: null` 行为不一致。建议加归一过滤。

5. **tool schema properties 强制补** — `crates/atomcode-capabilities/src/provider/openai_compat.rs:1160-1216` 在发 OAI 之前递归补 `properties: {}`,防止 LM Studio 等严格校验器 400。laew 当前 tool 定义测试场景覆盖少,生产可能撞上。

#### P1(规划)

6. **`is_openrouter_url` 归因头 + host_matches_domain 共享谓词** — 复用 host 提取函数(避免引入 `url` crate),确保归因头**只**发到 openrouter.ai,避免 `https://openrouter.ai:x@evil.com` 之类的 userinfo 拼接泄漏。参考 `crates/atomcode-capabilities/src/provider/openai_compat.rs:63-83`。

7. **error chain walking(`chain_has_transient_io`)** — 用 io 错误码 + source() walking 覆盖 keep-alive reset 这类**真连接已断但 reqwest 不分类**的 case(`crates/atomcode-capabilities/src/provider/retry.rs:119-156`)。laew 当前只用 `e.is_timeout() || e.is_connect()`,生产中 half-open keep-alive 复用会"硬失败"。

8. **backoff 真 jitter** — 用 `SystemTime::subsec_nanos()` 而非 `rand::random::<f64>()`,避免多依赖,但保留跨进程/跨调用 decorrelation(`crates/atomcode-capabilities/src/provider/retry.rs:440-446`)。laew 当前无 jitter,多 client 重试时易撞 thundering herd。

9. **thinking/redacted_thinking 分块 + opaque signature** — `crates/atomcode-kernel/src/message.rs:75-94` 的 `ReasoningBlock { text, opaque, provider }` + `crates/atomcode-capabilities/src/provider/anthropic.rs:637-675` 的 `format_assistant_message` provider 过滤。laew 当前 `agent/tools/reasoning` 没有结构化 reasoning 块,只有平铺文本;若未来接 Claude extended thinking 必须先建块。

10. **`ReasoningPolicy::derive` 模型名 + host 双判** — 同一 model id 在不同网关下表现可能不同(`reasoning.rs:128-152` 测试矩阵覆盖 moonshot/kimi/mimo by-host)。laew 若接 DeepSeek-V4 必须有 `Include` 模式(否则 400 "must be passed back")。

11. **Anthropic prompt cache 字节稳定性 + merged consecutive user** — `crates/atomcode-capabilities/src/provider/anthropic.rs:556-601` 的 `merge_consecutive_user` 处理 <system-reminder> 后缀和 post-compaction summary 等真实路径。laew 的 Anthropic 客户端可借鉴其测试矩阵:`no_consecutive_user_after_tool_fold_then_reminder` / `no_consecutive_user_post_compaction_summary_beside_user` / `consecutive_tool_results_fold_into_one_user_message`(`anthropic.rs:1059-1196`)。

#### P2(后续)

12. **`SwappableClient` 池子自动修复** — `crates/atomcode-capabilities/src/provider/openai_compat.rs:444-480` 是 reqwest Client 的"可重建"包装,持 `RwLock<Client>` + `Box<dyn Fn(bool) -> ...>`,mid-stream stale-conn 错误触发 `rebuild()` 得到 empty pool。laew 不太需要(单 CLI 进程),但若加 daemon 模式需借鉴。

13. **stream-idle watchdog + mid-stream idle 非 retryable** — `crates/atomcode-capabilities/src/provider/anthropic.rs:218-225` 中途 idle timeout 永远报 `retryable: false` + "stream idle timeout"。laew 当前也无,但通常没需求。

14. **byte-level wire dump** — `ATOMCODE_WIRE_DUMP=1` 写出 `.req.json`(`provider/mod.rs:61-93`),byte-level 保真。laew 当前可加 `LAEW_WIRE_DUMP=1` 环境变量,后续协议调试加速(特别是发现"prompt cache 未命中"这类问题)。

15. **结构化 error code(openai-compat 五元组)** — `ProviderError { retryable, message, http_status, code, retry_after_secs }`(`crates/atomcode-kernel/src/stream.rs:36-51`)。laew 当前 `AgentError::YoloParse` 较少结构化。建议补 `code: Option<String>` + `retry_after_secs: Option<u64>` 给上层 rate-limit self-heal 用。

16. **`Malformed` 信号事件** — `crates/atomcode-kernel/src/stream.rs:151`:`StreamEvent::Malformed`(空载荷,纯 signal)让 kernel 区分"网关发垃圾"vs"网关发空 200"。laew 若接弱网关(自部署转发)可能用到。

17. **TLS-1.2 自动降级** — `crates/atomcode-config/src/tls.rs` 的 `latch_managed_tls12()` + `atomcode_config::tls::should_try_fallback`(`openai_compat.rs:903-907`)对 managed endpoint 探测 TLS-1.3 失败后降级 TLS-1.2。laew 暂不需要(Anthropic / OpenAI 直接 TLS-1.3 良好),但若接国内网关(自建,可能有中间件)可借鉴。

18. **`MaxRounds` / `StopReason` 12 变体 + 明确"中途 idle timeout"** — laew 当前 `AgentError` 单一 enum。建议拆 6-8 变体,便于单测枚举。参考 `crates/atomcode-kernel/src/event.rs:88-119`。

#### 关键文件路径汇总

- 协议实现:`crates/atomcode-capabilities/src/provider/anthropic.rs`(2024 行)、`crates/atomcode-capabilities/src/provider/openai_compat.rs`(4093 行)
- 重试框架:`crates/atomcode-capabilities/src/provider/retry.rs`(1081 行,大半是测试)
- 跨协议 reasoning 策略:`crates/atomcode-capabilities/src/provider/reasoning.rs`(183 行)
- Wire dump + 友好错误:`crates/atomcode-capabilities/src/provider/mod.rs`(共 ~250 行)
- 内核契约:`crates/atomcode-kernel/src/stream.rs`(281 行)、`crates/atomcode-kernel/src/event.rs`(532 行)、`crates/atomcode-kernel/src/provider.rs`、`crates/atomcode-kernel/src/message.rs:75-94` 的 `ReasoningBlock`

—— 调研者注:本文档为「第六轮深挖」,聚焦 byte-stable 请求体、流式解码状态机、错误分类与重试三层叠加;不重复前 19 章的总览与前 5 轮已覆盖的 session/compact/Cancel/并发执行闸位等主题。