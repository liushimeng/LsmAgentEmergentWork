# atomcode 核心机制深度分析（第二轮）

- 分析日期：2026-09-04
- 源码根路径：`/usr/local/LsmGitOpenSource/atomcode/`
- 核心文件数：~45 个 `.rs` 文件
- 分析方法：逐文件读取 + 函数级追踪（非目录级概览）

---

## 专题 1：L0 Kernel —— 中立 Agent 循环

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `crates/atomcode-kernel/src/agent.rs` (5966行) | Agent 主循环、轮次编排、重试/超时/融合 |
| `crates/atomcode-kernel/src/message.rs` (2064行) | `Message` / `Conversation` / `CompactionPlan` / `sacred_floor` |
| `crates/atomcode-kernel/src/provider.rs` | `LlmProvider` trait + `ChatOptions` + `ToolChoice` |
| `crates/atomcode-kernel/src/stream.rs` | `StreamEvent` / `ProviderError` / `TokenUsage` |
| `crates/atomcode-kernel/src/tool.rs` | `Tool` trait / `ToolCall` / `ToolResult` / `RiskLevel` / `ToolContext` |
| `crates/atomcode-kernel/src/hook.rs` | `LifecycleHooks` trait / `HookChain` / `Continuation` / `TurnCtx` |
| `crates/atomcode-kernel/src/middleware.rs` | `ToolMiddleware` trait（before/after） |
| `crates/atomcode-kernel/src/event.rs` | `AgentEvent` / `AgentCommand` / `StopReason` |

### 1.1 协议中立性保证

**核心设计**：Agent 循环永远不触及协议细节，差异封闭在 `LlmProvider` trait 内部。

```rust
// crates/atomcode-kernel/src/provider.rs
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn model_name(&self) -> &str;
    fn context_window(&self) -> u32 { 0 }
    fn bind_session_id(&self, _session_id: &str) {}
    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        options: &ChatOptions,
    ) -> Result<BoxStream<'static, StreamEvent>, ProviderError>;
}
```

统一消息模型是关键：`Message` 结构体包含 `role`/`text`/`tool_calls`/`tool_call_id`/`is_error`/`reasoning`/`reasoning_blocks`/`images`，所有协议差异在 adapter 层转换。`StreamEvent` 枚举覆盖了 `TextDelta`/`Reasoning`/`ReasoningSignature`/`ToolCall`/`ToolCallDelta`/`Usage`/`ResponseId`/`ResponseModel`/`Error`/`Done` 全部流事件类型。

`ChatOptions` 是中立的请求旋钮：

```rust
// crates/atomcode-kernel/src/provider.rs
pub struct ChatOptions {
    pub reasoning_effort: Option<ReasoningEffort>,  // Low/Medium/High/XHigh/Max
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub tool_choice: ToolChoice,  // Auto/Required/Specific(String)/None
    pub rate_limit_retry_owner: RateLimitRetryOwner,
}
```

### 1.2 Agent 主循环状态机

入口在 `RunningAgent::run_turn()`（agent.rs:1850），是一个复杂的 `loop`，内含多层计数器/融合器：

**轮次循环**：`loop { round += 1; ... }`

**关键计数器**（全部在 run_turn 局部声明）：
- `round: u32` — 当前轮次（1-based）
- `continuations: u32` — `offer_continuation` 续接次数，上限 `max_continuations`（默认 50）
- `truncation_continuations: u32` — 输出截断自动续接，上限 `MAX_TRUNCATION_CONTINUATIONS`（4）
- `overflow_attempt: u8` — 上下文溢出恢复尝试，上限 `MAX_OVERFLOW_ATTEMPTS`（3）
- `provider_retry: u32` — 瞬态 provider 重试，上限 `DEFAULT_MAX_PROVIDER_RETRIES`（3）
- `stream_retry: u32` — 流中断重连，上限 `MAX_STREAM_RETRIES`（5）
- `rate_limit_waits: u32` — 429 限流等待，上限 `MAX_RATE_LIMIT_WAITS`（5）
- `empty_retries: u32` — 空响应重试，上限 `EMPTY_RESPONSE_MAX_RETRIES`（5）
- `repeat_rounds: u32` — 粗粒度重复熔断，`REPEAT_NUDGE_AT`（3）时警告，`MAX_REPEAT_ROUNDS`（6）时停止

**每轮处理流程**：
1. 检查 `round_cap`（可选的交互式 checkpoint）
2. 排空 `steer` 缓冲区（用户中途输入）
3. 克隆消息 → `hooks.pre_request()` 投影（ephemeral，不污染缓存）
4. `hooks.pre_request_options()` 设置请求选项
5. 构建 `ToolDef[]` → `provider.chat_stream()` 发起请求
6. 流式消费 `StreamEvent`，分发到 `on_text_delta` / `on_reasoning_delta` / `on_model_response`
7. 工具调用三阶段：① Classify → ② Execute（并发） → ③ Apply
8. 检查续接/截断/溢出/空响应，决定是否继续循环

### 1.3 流式输出实现

```rust
// crates/atomcode-kernel/src/stream.rs
pub enum StreamEvent {
    TextDelta(String),
    Reasoning(String),
    ReasoningSignature { opaque: String, provider: String },
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

`TokenUsage::merge_max()` 处理 Anthropic 风格（split report）和 OpenAI 风格（单次 cumulative）的差异：field-wise MAX，不会 double-count cumulative delta。

`on_text_delta` / `on_reasoning_delta` 钩子允许 REDACTION（流式 chunk 级别），保证流和存储一致性。

### 1.4 错误处理与重试

**三层重试**：
1. **Provider 内部**：adapter 自己的快速退避（~1.5s）
2. **Agent 循环 provider_retry**：`ProviderError::retryable == true` 时，最多 3 次，退避 3/6/9s
3. **429 限流**：`on_rate_limit` 钩子返回 `WaitAndRetry{secs}` 或 `Pause`，默认退避 3/6/12/24/48s（±25% jitter），`MAX_RATE_LIMIT_WAITS=5` 熔断

**空响应重试**：200 但无内容，最多 5 次短退避（瞬态，常见于 DeepSeek 路径）

**流中断重连**：`stream_timeout`（默认 300s 字节空闲），最多 5 次重连，partial output 保留

**截断续接**：`finish_reason=length` 时注入 `TRUNCATION_RESUME_NUDGE`，最多 4 次

**上下文溢出恢复**：`ProviderError::is_context_overflow()` 检测 9 种签名 → 递增 attempt 调用 `CompactTrigger::Overflow`

### 1.5 核心结构体

```rust
// crates/atomcode-kernel/src/message.rs
pub struct Message {
    pub role: Role,            // System/User/Assistant/Tool
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub is_error: bool,
    pub meta: Option<MessageMeta>,
    pub synthetic: bool,       // kernel 注入的合成消息
    pub internal_origin: Option<String>,  // "verify_cadence" 等
    pub reasoning: Option<String>,
    pub images: Vec<ImageContent>,
    pub reasoning_blocks: Vec<ReasoningBlock>,
}

pub struct Conversation {
    pub messages: Vec<Message>,
    pub cache_epoch: u64,  // prefix-cache 世代标记
}
```

`ToolCall` / `ToolResult` / `ToolContext` 定义在 `tool.rs`，`ToolContext` 携带 `working_dir` + `CancellationToken` + `ProgressSink` + `Requester`。

### 对 laew 的借鉴价值

1. **协议中立性**：laew 的 `LlmClient` trait 可以学习 atomcode 的 `LlmProvider::chat_stream()` 返回 `BoxStream<StreamEvent>` 模式，将流式差异封闭在 adapter 内部
2. **多层熔断**：laew 的 Agent 循环可以引入类似的多层计数器（provider retry / stream retry / empty retry / rate limit wait），每个都有独立上限和退避策略
3. **`TurnCtx` 关联 ID**：session_id → turn_id → request_id 的三层单调计数器，用于日志关联和遥测，laew 可以直接复用
4. **`cache_epoch`**：只有 committed compaction 才 bump epoch，保证 prefix cache 稳定，laew 的 SessionContext 可以借鉴
5. **steer 机制**：用户中途输入排入 `SteerBuf`，下一轮 drain 并注入，laew 的 TUI 可以实现类似的中途干预

---

## 专题 2：L1 Capabilities —— 可复用能力层

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `crates/atomcode-kernel/src/tool.rs` | `Tool` trait 定义 / `ToolDef` / `ToolRegistry` / `RiskLevel` |
| `crates/atomcode-capabilities/src/tools/mod.rs` | 工具注册入口 / `coding_tool_names()` / `register_coding_tools()` |
| `crates/atomcode-capabilities/src/tools/bash.rs` | `BashTool` — shell 执行 + 危险命令检测 |
| `crates/atomcode-capabilities/src/tools/read.rs` | `ReadFileTool` — 分页读取 + vision 支持 |
| `crates/atomcode-capabilities/src/tools/write.rs` | `WriteFileTool` — 创建/覆写文件 |
| `crates/atomcode-capabilities/src/tools/edit.rs` | `EditFileTool` — 精确替换 + fuzzy 匹配 |
| `crates/atomcode-capabilities/src/tools/grep.rs` | `GrepTool` — 内容搜索 |
| `crates/atomcode-capabilities/src/tools/glob.rs` | `GlobTool` — 文件名匹配 |
| `crates/atomcode-capabilities/src/tools/approval.rs` | `ApprovalMiddleware` — 通用审批门 |
| `crates/atomcode-capabilities/src/tools/write_approval.rs` | `WriteApprovalGate` — 工作区感知的写审批 |
| `crates/atomcode-capabilities/src/tools/sensitive_path.rs` | `SensitivePathGate` — 敏感路径保护 |
| `crates/atomcode-capabilities/src/tools/task.rs` | `TaskTool` — 子 agent 派发 |
| `crates/atomcode-capabilities/src/middleware.rs` | `ToolMiddleware` trait（before/after） |

### 2.1 Tool trait 定义

```rust
// crates/atomcode-kernel/src/tool.rs
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;  // JSON Schema
    fn risk(&self, args: &str) -> RiskLevel { RiskLevel::Safe }
    fn parallel_safe(&self, args: &str) -> bool { false }
    fn always_grant_scope(&self, args: &str) -> String { args.to_string() }
    fn read_only_hint(&self) -> bool { false }
    async fn execute(&self, args: &str, ctx: &ToolContext) -> ToolResult;
}
```

**关键设计**：
- `risk()` 是 **arg-aware** 的：`BashTool` 根据命令内容判断 Safe/Risky，`ReadFileTool` 永远 Safe
- `parallel_safe()` 控制并发：只读工具（read/grep/glob/bash 的只读命令）可以并行执行
- `always_grant_scope()` 控制"总是允许"的粒度：文件工具是 tool-wide，bash 是 per-command

### 2.2 工具注册与发现

```rust
// crates/atomcode-capabilities/src/tools/mod.rs
pub fn coding_tool_names() -> &'static [&'static str] {
    &["read_file", "write_file", "edit_file", "list_directory",
      "open_file", "bash", "grep", "glob", "search_replace",
      "ast_grep", "todowrite", "fetch_output", "request_user_input"]
}

pub fn register_coding_tools(reg: &mut ToolRegistry) {
    register_coding_tools_with_vision(reg, false);
}

pub fn register_coding_tools_with_vision(reg: &mut ToolRegistry, vision: bool) {
    reg.register(Arc::new(ReadFileTool::new(vision)));
    reg.register(Arc::new(WriteFileTool));
    reg.register(Arc::new(EditFileTool));
    reg.register(Arc::new(ListDirTool));
    reg.register(Arc::new(OpenFileTool));
    reg.register(Arc::new(BashTool));
    reg.register(Arc::new(GrepTool));
    reg.register(Arc::new(GlobTool));
    reg.register(Arc::new(SearchReplaceTool));
    // ... 更多工具
}
```

注册是 `register()`（放入全集），暴露给模型是 `mount()`（选择子集）。这是两阶段设计：register 放入所有可用工具，mount 决定哪些暴露给 LLM。

### 2.3 核心工具实现细节

**BashTool**：
- 默认超时 60s，最大 300s；静默杀死阈值 90s（`SILENT_KILL_SECS`）
- 注入非交互环境变量：`TERM=dumb` / `PAGER=cat` / `GIT_PAGER=cat` / `GIT_TERMINAL_PROMPT=0`
- `setsid()` + `TIOCNOTTY` 将子进程从控制终端分离
- `check_destructive_command()` 检测特权提升、递归强删、`find -delete`、`dd`、fork bomb 等
- `parallel_safe` 对只读 bash 命令返回 true（`is_read_only_bash`）

**ReadFileTool**：
- 默认分页 1500 行，输出预算 50KB（`MAX_READ_OUTPUT_BYTES`）
- 单行截断 2000 字符（`MAX_LINE_LEN`）
- 支持多范围读取（`render_multi_range`），共享 50KB 预算
- Vision 模式下图片文件返回 base64（最大 4MB）
- 300+ 行代码文件返回 symbol skeleton（codeintel feature）

**EditFileTool**：
- 精确匹配 → EOL 容错匹配 → fuzzy 匹配（行首空白忽略）→ whitespace-insensitive 匹配，四级降级
- 文件路径级锁（`edit_path_lock`）防止并发编辑冲突
- 支持 GBK/GB18030 编码检测和原编码写回
- `replace_all` 批量替换模式

**WriteFileTool**：
- 自动创建父目录（`create_dir_all`）
- 覆写时报告行数变化，大缩水（>50%）时发出警告
- 永远写 UTF-8（与 edit_file 的编码保留不同，这是有意的）

### 2.4 沙箱 / 安全机制

atomcode 的核心理念：**kernel 不做沙箱，OS 级隔离是嵌入者的责任**。

```rust
// crates/atomcode-kernel/src/tool.rs（模块文档）
// The kernel is a neutral, embeddable SDK. It does NOT sandbox the tools it hosts.
// MOUNTING a tool GRANTS its `execute` the host process's full ambient authority.
```

安全机制分三层：

1. **RiskLevel（advisory metadata）**：工具自报 Safe/Risky，kernel 不强制执行
2. **ToolMiddleware（gate）**：`ApprovalMiddleware` 对 Risky 调用向 driver 发审批请求
3. **WriteApprovalGate**：工作区感知的写审批
   - 工作区内非敏感 → 自动批准
   - 敏感路径 → 每次都提示，永不记忆
   - 工作区外 → 提示，"总是"按 canonical directory 记忆
   - 敏感路径列表：`.env`/`id_rsa`/`credentials`/`~/.ssh`/云凭证等

**SensitivePathGate**：读取敏感路径也需要审批（防止 exfiltration）

**子 agent 安全**（task.rs）：
- `DenySensitivePaths`：子 agent 运行在 `AutoRespond::AllowAll` 模式，所以对敏感路径是硬拒绝而非提示
- `WorkerScopeGate`：confine 子 agent 的写工具到声明的 scope（glob 匹配）

**工具结果大小限制**：
```rust
// crates/atomcode-kernel/src/agent.rs
pub const DEFAULT_MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;  // 64 KiB
```
`cap_tool_result()` 做 HEAD+TAIL 截断（保留首尾各半），中段省略标记。

### 对 laew 的借鉴价值

1. **两级注册**：register（全集）+ mount（子集）的设计可以让 laew 的 ToolRegistry 灵活控制暴露给不同 Agent 的工具集
2. **arg-aware risk**：laew 的 BashTool 可以借鉴 `check_destructive_command()` 实现细粒度的风险分类
3. **WriteApprovalGate**：工作区内自动批准 + 敏感路径永不记忆的策略，laew 可以直接复用
4. **编辑四级降级**：exact → EOL → fuzzy → whitespace-insensitive，大幅降低模型编辑失败率
5. **子 agent scope 约束**：`WorkerScopeGate` 用 glob 限制写入范围，laew 的 SubAgent-Work 可以借鉴

---

## 专题 3：L2 Coding —— 业务层（编码 Agent）

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `crates/atomcode-coding/src/lib.rs` | L2 编码特化入口，组装 kernel + capabilities |
| `crates/atomcode-coding/src/assemble.rs` | `build_coding_agent()` — 组装完整 Agent |
| `crates/atomcode-coding/src/parts.rs` | `CodingParts` — 完整运行时部件集合 |
| `crates/atomcode-coding/src/persona.rs` | `coding_persona()` — 编码系统提示词 |
| `crates/atomcode-coding/src/plan_mode.rs` | `PlanModeGate` / `PlanModeReminderHook` |
| `crates/atomcode-coding/src/runtime.rs` | `CodingRuntime` — 驱动控制平面 |
| `crates/atomcode-coding/src/execution_policy.rs` | `TurnExecutionPolicy` — 每轮执行约束 |
| `crates/atomcode-coding/src/discipline/verify.rs` | `VerifyCadenceHook` — 编辑后验证纪律 |
| `crates/atomcode-coding/src/todo.rs` | `TodoHook` — 任务清单 |
| `crates/atomcode-coding/src/team/runner.rs` | Team Agent 运行器 |

### 3.1 系统提示词结构

```rust
// crates/atomcode-coding/src/persona.rs
pub fn coding_persona_with_capabilities(...) -> String {
    let mut p = format!(
        "You are AtomCode, an AI coding agent by AtomGit running the {model} model. \
         ...身份声明...\
         \n\n## PRECEDENCE:\n\
         Any GLOBAL / PROJECT / USER instruction blocks ... take PRECEDENCE ...\
         {CONTENT_SAFETY}\n\n{RULES}\n\n\
         ## GIT COMMITS:\n{commit_language}\n..."
    );
    // 按条件注入
    if model_needs_firm_tool_steering(model) { p.push_str(FIRM_TOOL_DISCIPLINE); }
    if model_needs_firm_execution(model) { p.push_str(FIRM_EXECUTION_DISCIPLINE); }
    if todo_enabled { p.push_str(TODO_USAGE); }
    if request_user_input_enabled { p.push_str(REQUEST_USER_INPUT_USAGE); }
    if subagents_enabled { p.push_str(SUBAGENT_DELEGATION); p.push_str(TEAM_DELEGATION); }
    if review_enabled { p.push_str(CODE_REVIEW_USAGE); }
    p.push_str(SKILLS_USAGE);
    if is_offline_active() { p.push_str(&offline_environment_block()); }
    p.push_str(&date_anchor_line(...));  // 日期锚点
    p
}
```

**关键设计**：
- 系统提示词是**参数化的**：根据 model/todo/enabled/subagent 等开关条件注入不同段落
- 弱模型（GLM/DeepSeek/Qwen/LongCat）额外注入 `FIRM_TOOL_DISCIPLINE`（强制用专用工具而非 shell）
- DeepSeek/Qwen 额外注入 `FIRM_EXECUTION_DISCIPLINE`（不删代码清错误、不未经验证就提交等）
- `CONTENT_SAFETY` 块始终注入（涉政/涉黄/涉暴内容安全边界）
- 日期锚点冻结到系统提示词中（跨天 resume 时刷新）

### 3.2 Plan 模式 / Apply 模式

```rust
// crates/atomcode-coding/src/plan_mode.rs
pub struct PlanModeGate {
    active: Arc<AtomicBool>,
    mcp_grants: Arc<dyn PermissionStore>,
}
```

**Plan 模式策略**：
- 内置 Risky 工具（bash/edit/write）→ **硬阻断**
- MCP 工具 `readOnlyHint: true` → **允许**（只读外部查询）
- 其他 MCP 工具 → **提示**用户决定
- `PlanModeReminderHook` 通过 `pre_request` 注入提醒："Do NOT create, edit, or delete files"

切换机制：`Arc<AtomicBool>` 共享给 `PlanModeGate`（ToolMiddleware）和 `PlanModeReminderHook`（LifecycleHooks），driver 调用 `CodingRuntime::set_mode()` 即时切换，无需 respawn。

### 3.3 执行策略（ExecutionPolicy）

```rust
// crates/atomcode-coding/src/execution_policy.rs
const NO_BUILD: u8 = 1 << 0;
const NO_TEST: u8 = 1 << 1;
const NO_SCRIPT: u8 = 1 << 2;
const NO_SHELL: u8 = 1 << 3;
const NO_VERIFY: u8 = 1 << 4;

pub(crate) struct ExecutionPolicy(u8);
```

从用户文本中解析约束（如"不要运行测试"），通过 `TurnExecutionPolicy`（同时实现 `LifecycleHooks` 和 `ToolMiddleware`）在每轮更新并执行。`skips_verification()` 为 true 时 `VerifyCadenceHook` 跳过验证。

### 3.4 子 Agent 派发

```rust
// crates/atomcode-capabilities/src/tools/task.rs
const DEFAULT_MAX_CONCURRENT: usize = 3;
```

`TaskTool` 把子任务派发给隔离上下文的子 agent。主 agent 按难度选档位（fast/capable），按类型选工具集（explore 只读 / worker 可编辑）。子 agent 跑在独立内核会话里，结果用 `<task_result>` 包回。

安全约束：
- `DenySensitivePaths`：硬拒绝敏感路径访问
- `WorkerScopeGate`：confine 写入到声明的 scope（glob 匹配）
- `.git` 目录内部写入被拒绝（防止子 agent 注入 hooks）

### 对 laew 的借鉴价值

1. **参数化系统提示词**：laew 的 Yolo/Plan/Work Agent 可以借鉴这种条件注入模式，根据任务级别动态组装提示词
2. **PlanModeGate**：用 `Arc<AtomicBool>` + ToolMiddleware + LifecycleHooks 实现即时模式切换，laew 的 Plan Agent 可以直接复用
3. **ExecutionPolicy 位图**：用位图表示 NO_BUILD/NO_TEST/NO_SHELL 等约束，解析自用户文本，laew 可以引入类似的用户意图约束
4. **子 Agent scope 约束**：`WorkerScopeGate` 用 glob 限制写入范围 + `.git` 硬拒绝，laew 的 SubAgent-Work 应该引入类似机制
5. **弱模型强化提示**：针对 GLM/DeepSeek 等弱模型注入额外的工具纪律和执行纪律，laew 可以按模型能力分级

---

## 专题 4：MCP 与 trust 体系

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `crates/atomcode-capabilities/src/mcp/mod.rs` | MCP 模块入口 / `register_mcp_tools()` |
| `crates/atomcode-capabilities/src/mcp/client.rs` | `McpClient` trait / `McpToolInfo` |
| `crates/atomcode-capabilities/src/mcp/registry.rs` | `McpRegistry` — 多 server 管理 / trust / auto-approve |
| `crates/atomcode-capabilities/src/mcp/tool.rs` | `McpToolAdapter` — 将 MCP tool 适配为 kernel Tool |
| `crates/atomcode-capabilities/src/mcp/transport_stdio.rs` | `StdioClient` — stdio JSON-RPC 传输 |
| `crates/atomcode-capabilities/src/mcp/transport_http.rs` | `HttpClient` — HTTP(SSE) 传输 |
| `crates/atomcode-capabilities/src/mcp/trust.rs` | 项目级 trust store（`mcp_trust.json`） |
| `crates/atomcode-capabilities/src/mcp/config.rs` | `McpServerConfig` / `load_mcp_config()` |
| `crates/atomcode-capabilities/src/mcp/oauth.rs` | OAuth 登录 / token 刷新 |

### 4.1 MCP Client Trait

```rust
// crates/atomcode-capabilities/src/mcp/client.rs
#[async_trait]
pub trait McpClient: Send + Sync {
    async fn initialize(&mut self) -> Result<InitializeResult>;
    async fn list_tools(&self) -> Result<ListToolsResult>;
    async fn call_tool(&self, tool_name: &str, arguments: serde_json::Value) -> Result<CallToolResult>;
    fn server_name(&self) -> &str;
    fn status(&self) -> ServerStatus;
}
```

### 4.2 stdio 传输实现

```rust
// crates/atomcode-capabilities/src/mcp/transport_stdio.rs
pub struct StdioClient {
    server_name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    timeout_ms: u64,                    // 默认 30s
    request_lock: Arc<Mutex<()>>,       // 序列化请求/响应往返
    operation_lock: Arc<Mutex<()>>,     // 保活请求+恢复决策在一个临界区
    reconnect_lock: Arc<Mutex<()>>,     // 序列化 teardown + respawn
    recovery_notify: Arc<Notify>,       // 唤醒等待恢复的操作
    recovery_in_progress: Arc<AtomicBool>,
    connection_generation: Arc<AtomicU64>,
    owns_transport_lifetime: bool,
    // ...
}
```

关键设计：
- 三把锁分离关注点：`request_lock`（序列化请求）、`operation_lock`（保活+恢复）、`reconnect_lock`（teardown+respawn）
- `connection_generation` 单调递增，等待者比较 generation 检测是否已被其他 caller 修复
- 子进程 `kill_on_drop(true)` 确保进程泄漏安全
- Windows 自动通过 `cmd.exe /C` 包装 `.cmd` 脚本

### 4.3 MCP Tool 适配

```rust
// crates/atomcode-capabilities/src/mcp/tool.rs
pub struct McpToolAdapter {
    registry: Arc<McpRegistry>,
    server: String,
    tool: String,
    full_name: String,      // "mcp__{server}__{tool}"
    description: String,    // "[MCP:{server}] {description}"
    schema: serde_json::Value,
    read_only: bool,        // server-declared readOnlyHint
}
```

命名规则：`mcp__{server}__{tool}`，超长或非法字符用 SHA256 hash suffix 保证唯一性。

### 4.4 Trust 信任体系

**项目级信任**：
```rust
// crates/atomcode-capabilities/src/mcp/trust.rs
pub fn trust_store_path() -> PathBuf { ... }  // ~/.atomcode/mcp_trust.json
pub fn is_project_trusted(project_dir: &Path) -> bool { ... }
pub fn trust_project(project_dir: &Path) -> anyhow::Result<()> { ... }
pub fn untrust_project(project_dir: &Path) -> anyhow::Result<bool> { ... }

pub fn partition_by_trust(configs: Vec<McpServerConfig>, project_dir: &Path) -> TrustPartition {
    // 未信任项目 → Project 来源的 server 被 blocked
    // 已信任项目 → 全部 allowed
}
```

**三级信任判定**（在 `McpToolAdapter::risk()` 中）：
```rust
fn risk(&self, _args: &str) -> RiskLevel {
    if self.read_only                          // server 声明只读
        || self.registry.is_server_trusted(&self.server)  // config 里 trust: true
        || self.registry.is_tool_auto_approved(&self.full_name)  // autoApprove 列表或"总是"
    {
        RiskLevel::Safe
    } else {
        RiskLevel::Risky
    }
}
```

**自动批准机制**（`McpRegistry`）：
- `trusted_servers`：config 里 `trust: true` 的 server，所有工具自动 Safe
- `auto_approved_tools`：server 的 `autoApprove` 列表 + 运行时"总是"授权
- `tool_aliases`：sanitized name → original identity 的映射，alias collision fail-closed

### 对 laew 的借鉴价值

1. **MCP Client Trait**：laew 可以引入 MCP 支持，`McpClient` trait 是好的起点
2. **三锁分离**：stdio 传输的 request/operation/reconnect 三锁设计，laew 的外部工具通信可以借鉴
3. **项目级 trust store**：`mcp_trust.json` 的 hash-based project key 设计，laew 可以引入类似的项目信任机制
4. **风险三级判定**：read_only → server_trusted → tool_auto_approved 的逐级降级，laew 的工具审批可以复用
5. **connection_generation**：单调递增 generation 检测连接修复，laew 的 MCP 集成可以借鉴

---

## 专题 5：Context 管理 —— 三级 overflow ladder

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `crates/atomcode-capabilities/src/compaction.rs` (2345行) | `StubCompaction` / `OverflowCompaction` / 三级 ladder |
| `crates/atomcode-kernel/src/message.rs` | `Conversation::apply_plan()` / `sacred_floor()` / `CompactionPlan` |
| `crates/atomcode-kernel/src/agent.rs` | `run_compaction()` / `auto_compact_trigger()` / overflow retry loop |
| `crates/atomcode-kernel/src/checkpoint.rs` | `CompactionCheckpoint` — 持久化快照 |

### 5.1 三级 Overflow Ladder

```rust
// crates/atomcode-capabilities/src/compaction.rs
pub struct OverflowCompaction {
    inner: StubCompaction,
    summary_provider: Option<Arc<dyn LlmProvider>>,
}

// 实现 CompactionStrategy
async fn plan(&self, view: &CompactionView<'_>) -> CompactionPlan {
    match view.trigger {
        CompactTrigger::Auto { .. } => self.inner.plan(view).await,    // Tier 0
        CompactTrigger::Overflow { attempt } => self.overflow_plan(view, *attempt).await,  // Tier 0-2
        CompactTrigger::Manual { focus } => self.manual_plan(view, focus.as_deref()).await,
    }
}
```

**三级 ladder**：

| Tier | 触发条件 | 策略 | 特点 |
|------|---------|------|------|
| **Tier 0** | `Auto` 或 `Overflow{attempt=0}` | `StubCompaction`：旧工具结果 stub（>500B → 一行摘要） | cache-friendly，单调（stub 不可逆） |
| **Tier 1** | `Overflow{attempt=1}` | `truncate_rewrites`：硬截断超长消息（budget = ctx_window × 2 chars） | 更激进，幂等（TRUNCATE_MARKER） |
| **Tier 2** | `Overflow{attempt=2}` | drain + summarize：LLM 生成摘要替换旧历史 | 最激进，有超时保护（180s） |

### 5.2 StubCompaction（Tier 0 — 正常路径）

```rust
// crates/atomcode-capabilities/src/compaction.rs
pub const MIN_COLLAPSE_SIZE: usize = 500;  // 低于此大小的不 stub

pub struct StubCompaction {
    keep_recent_turns: usize,   // 默认 1（只保留活跃轮次完整）
    exempt_read_file: bool,     // 默认 true（read_file 结果不 stub）
}
```

**核心逻辑**（`StubCompaction::plan`）：
1. 找到活跃轮次的起始位置（最近 N 个非合成 User 消息）
2. 遍历 `sacred_floor..boundary` 范围的消息
3. 跳过：非 Tool 角色 / 已小消息（≤500B）/ read_file 结果（如果 exempt）
4. 生成 stub：`"[tool: bash] Command output (847 chars, success)"` 格式

**为什么 cache-friendly**：stub 是 COMMITTED 到历史的（不可逆），所以重跑不会改变历史字节，prefix cache 只在 stub 那一轮 break 一次。

### 5.3 Auto 触发机制

```rust
// crates/atomcode-kernel/src/agent.rs
fn auto_compact_trigger(used_tokens: u32, ctx_window: u32, threshold: f32) -> Option<CompactTrigger> {
    if ctx_window == 0 { return None; }
    let utilization = used_tokens as f32 / ctx_window as f32;
    (utilization >= threshold).then_some(CompactTrigger::Auto { utilization })
}
```

**两级自动触发**：
- `compact_threshold`（默认 0.7）：触发 Tier 0 stub compaction
- `AUTO_DRAIN_UTILIZATION`（0.78）：触发 drain + summarize（在 Auto 路径内升级）

### 5.4 Manual `/compact` 命令

```rust
async fn manual_plan(&self, view: &CompactionView<'_>, focus: Option<&str>) -> CompactionPlan {
    // 按 recent_keep_budget 保留近期轮次
    // drain 老历史 → LLM 摘要
    // 超时（180s）→ 降级为 gentle stub
}
```

`recent_keep_budget`：
```rust
const RECENT_KEEP_FRACTION: f32 = 0.25;  // 保留窗口 25%
const MIN_RECENT_KEEP_TOKENS: usize = 8_000;
const MAX_RECENT_KEEP_TOKENS: usize = 256_000;

fn recent_keep_budget(ctx_window: u32) -> usize {
    ((window as f32 * RECENT_KEEP_FRACTION) as usize)
        .clamp(MIN_RECENT_KEEP_TOKENS, MAX_RECENT_KEEP_TOKENS)
        .min(window / 2)
}
```

### 5.5 Sacred Floor（神圣前缀保护）

```rust
// crates/atomcode-kernel/src/message.rs
pub fn sacred_floor(&self) -> usize {
    let lead_system = usize::from(matches!(self.messages.first().map(|m| &m.role), Some(Role::System)));
    match self.messages.iter().position(|m| m.role == Role::User && !m.synthetic) {
        Some(idx) => idx + 1,  // System + 第一个真实 User 消息
        None => lead_system,
    }
}
```

**绝不删除**：System 消息 + 第一个非合成 User 消息（任务提示词）永远不被 compaction 删除。

### 5.6 摘要 LLM 调用

```rust
const SUMMARY_SYSTEM_PROMPT: &str = "You are an anchored context summarization assistant...";
const MAX_SUMMARY_BYTES: usize = 64 * 1024;   // 64KB 硬上限
const MAX_SUMMARY_TOKENS: u32 = 16_000;       // 发给 LLM 的 max_tokens
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(180);
```

摘要有 `<previous-summary>` 增量更新模式：如果有已有摘要，LLM 更新而非重写。

### 5.7 Prompt Cache 集成

`cache_epoch` 只在 committed compaction 时 bump。`pre_request` 投影是 ephemeral（克隆上操作），不污染存储历史 → prefix cache 稳定。每轮只在 tail break 一次 cache（stub 那轮），之后冻结。

### 对 laew 的借鉴价值

1. **三级 ladder**：laew 的 SessionContext 可以引入类似的分级压缩：stub → truncate → summarize，按压力递增
2. **cache-friendly stub**：stub 是 committed 单调的，不反复改变历史字节，laew 的工具结果压缩应该遵循同样的单调性
3. **sacred_floor**：永远保护 System + 第一个真实 User 消息，laew 的 Session 压缩必须引入类似保护
4. **recent_keep_budget**：drain 时保留 25% 近期轮次（8K-256K tokens），避免"摘要后什么都忘了"的问题
5. **摘要超时降级**：180s 超时后降级为 gentle stub，laew 的 summarize 也应该有降级路径
6. **read_file exemption**：read_file 结果不 stub（保留行号上下文），laew 可以对 ReadTool 结果做类似豁免

---

## 专题 6：质检与 VerifyCadenceHook

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `crates/atomcode-coding/src/discipline/verify.rs` (992行) | `VerifyCadenceHook` — 编辑后验证纪律 |
| `crates/atomcode-coding/src/discipline/mod.rs` | discipline 模块入口 |
| `crates/atomcode-kernel/src/hook.rs` (639行) | `LifecycleHooks` trait / `HookChain` / `Continuation` |
| `crates/atomcode-review/src/lib.rs` | Review Agent（只读代码审查子 agent） |
| `crates/atomcode-review/src/assemble.rs` | `build_review_agent()` |
| `crates/atomcode-review/src/fanout.rs` | 多维度并行审查 / verify |
| `crates/atomcode-coding/src/execution_policy.rs` | `TurnExecutionPolicy` — 验证开关 |
| `crates/atomcode-coding/src/todo.rs` | `TodoHook` — 任务清单管理 |
| `crates/atomcode-coding/tests/verify_cadence.rs` | 验证纪律的端到端测试 |

### 6.1 VerifyCadenceHook 实现细节

```rust
// crates/atomcode-coding/src/discipline/verify.rs
const NUDGE: &str = "You made code edits but have not verified them. Run a fast check \
    (`cargo check`, `tsc --noEmit`, or the equivalent for this project) to catch errors \
    before finishing. Do NOT start a long-running process (dev server, watcher, full build).";

#[derive(Default)]
pub struct VerifyCadenceHook {
    workspace: PathBuf,
    execution_policy: Arc<TurnExecutionPolicy>,
    state: Mutex<State>,
    suppress_verify_continuation: bool,  // 交互模式下跳过
}
```

**触发逻辑**（`offer_typed_continuation`）：
1. 检查是否被抑制（交互模式 + `ATOMCODE_VERIFY=0`）
2. 检查执行策略是否跳过验证（`skips_verification()`）
3. 调用 `unverified_edit()` 扫描对话历史
4. 如果有未验证的编辑 → 注入 `Continuation::verify_cadence(NUDGE)`
5. 每个编辑只 nudged 一次（`nudged_for` 状态追踪）

### 6.2 unverified_edit() —— 核心检测逻辑

```rust
fn unverified_edit(convo: &Conversation, workspace: &Path) -> Option<NudgedEdit> {
    let start = current_real_user_start(convo);  // 最近一个真实 User 消息位置
    let mut names: HashMap<&str, &str> = HashMap::new();
    let mut bash_cmds: HashMap<&str, String> = HashMap::new();
    let mut edit_paths: HashMap<&str, String> = HashMap::new();
    let mut last_edit_id: Option<String> = None;
    let mut bash_after_edit = false;

    for msg in &convo.messages[start..] {
        match msg.role {
            Role::Assistant => {
                // 收集 tool_call id → name 映射
                // 收集 bash 命令和 edit 路径
            }
            Role::Tool if !msg.is_error => {
                match names.get(id) {
                    Some("edit_file") | Some("write_file")
                        if path_in_workspace_lexical(p, workspace)
                        && !path_is_noncode_doc(p) =>
                    {
                        last_edit_id = Some(id.to_string());
                        bash_after_edit = false;  // 重置
                    }
                    Some("bash") if bash_verifies(cmd) => {
                        bash_after_edit = true;  // 真实验证命令
                    }
                    _ => {}
                }
            }
        }
    }
    // 有编辑但没有后续验证 → 返回未验证编辑
    match last_edit_id {
        Some(id) if !bash_after_edit => Some(NudgedEdit { turn_start: start, edit_id: id }),
        _ => None,
    }
}
```

### 6.3 bash_verifies() —— 语言无关的验证检测

```rust
fn bash_verifies(cmd: &str) -> bool {
    cmd.split(|c| c == '|' || c == ';' || c == '&')
        .map(str::trim)
        .filter(|seg| !seg.is_empty())
        .any(segment_is_work)
}

fn segment_is_work(seg: &str) -> bool {
    const READONLY: &[&str] = &[
        "ls", "cat", "pwd", "echo", "cd", "which", "head", "tail", "find", "grep",
        "git", "wc", "sort", "uniq", "awk", "sed", "cut", "diff", "less", "more", ...
    ];
    const WRAPPERS: &[&str] = &["sudo", "env", "time", "nice", "command", "exec"];
    // 跳过 VAR=val 环境赋值和 wrapper
    // 返回 !READONLY.contains(&head)
}
```

**语言无关**：不排除特定构建命令（不枚举 cargo/npm），而是排除已知只读/导航命令。任何非只读命令都算"验证"（`cargo check`/`tsc --noEmit`/`make test` 都通过）。

### 6.4 工作区门控

```rust
fn path_in_workspace_lexical(raw: &str, workspace: &Path) -> bool {
    // 纯词法判断（不访问文件系统，避免阻塞）
    // 相对路径 → 解析为工作区内
    // ~ 前缀 → 展开 HOME
    // 绝对路径 → lexical_normalize + 前缀检查
    // 无法判断 → 保守返回 true（保持 cadence）
}
```

工作区外的编辑（如 `/tmp/notes.txt`）不触发验证 cadence——不是项目代码，不需要编译检查。

### 6.5 LifecycleHooks 与 HookChain

```rust
// crates/atomcode-kernel/src/hook.rs
#[async_trait]
pub trait LifecycleHooks: Send + Sync {
    async fn session_start(&self, _convo: &mut Conversation, _resumed: bool) {}
    async fn user_prompt_submit(&self, _text: &mut String) -> Result<(), String> { Ok(()) }
    async fn turn_start(&self, _convo: &mut Conversation) {}
    async fn pre_request(&self, _messages: &mut Vec<Message>, _ctx: &TurnCtx) {}
    async fn pre_request_options(&self, ...) {}
    async fn on_request(&self, ...) {}
    async fn on_text_delta(&self, _delta: &mut String) {}
    async fn on_reasoning_delta(&self, _delta: &mut String) {}
    async fn on_model_response(&self, _response: &mut Message) {}
    async fn offer_continuation(&self, _convo: &Conversation) -> Option<String> { None }
    async fn offer_typed_continuation(&self, ...) -> Option<Continuation> { ... }
    async fn turn_complete(&self, ...) {}
    async fn on_error(&self, _error: &str) {}
    async fn on_rate_limit(&self, _hint: &RateLimitHint) -> Option<RateLimitDecision> { None }
    async fn session_end(&self, _convo: &Conversation) {}
}
```

`HookChain` 将多个 `LifecycleHooks` 组合为一个：
- `session_start`/`turn_start`/`pre_request`/`on_request`/`on_text_delta`/`on_model_response`：全部按注册顺序执行
- `user_prompt_submit`：短路——第一个 `Err` 阻断后续
- `offer_continuation`：第一个 `Some` 胜出，后续忽略
- `on_rate_limit`：第一个返回 `Some` 的胜出

### 6.6 Review Agent（代码审查子 agent）

```rust
// crates/atomcode-review/src/lib.rs
// 只读代码审查 agent，报告结构化 Finding
pub use assemble::{build_review_agent, build_review_agent_with, build_review_agent_with_cancel};
pub use review_tool::{ReviewTool, ReviewToolConfig, SharedReviewProvider};
pub use fanout::{
    dimension_coverage, finalize_deep_review, merge_deep_findings,
    ReviewDimension, REVIEW_DIMENSIONS, VERIFY_CONCURRENCY, VERIFY_LENS,
};
```

Review Agent 是一个**只读子 agent**：
- 工具集：read/grep/glob/ast_grep/codeintel/web_search + `report_finding` sink
- 无 bash/edit/write（严格只读）
- `ReviewTool` 可以作为主 agent 的一个工具挂载，实现"审查当前变更"能力
- 多维度并行审查（`REVIEW_DIMENSIONS`），`VERIFY_CONCURRENCY` 控制并发

### 6.7 TodoHook

```rust
// crates/atomcode-coding/src/todo.rs
// 任务清单管理 hook
```

TodoHook 维护一个结构化的任务清单（`todowrite` 工具），在系统提示词中注入使用指导。弱模型（GLM）特别需要系统提示词级别的引导才会使用。

### 6.8 质检触发时机总结

| 机制 | 触发时机 | 行为 |
|------|---------|------|
| **VerifyCadenceHook** | 模型停止且有未验证编辑 | 注入 nudge 续接 |
| **ExecutionPolicy** | 用户文本解析 | 控制是否允许 build/test/shell |
| **ToolLoopPolicy** | 同一工具调用重复 | warning(3次) → stop(4次) |
| **MAX_REPEAT_ROUNDS** | 同一 round signature 重复 | nudge(3) → stop(6) |
| **Review Agent** | 用户/主 agent 主动触发 | 只读审查 + 结构化 Finding |
| **TodoHook** | 每轮 | 任务清单管理 |

### 对 laew 的借鉴价值

1. **VerifyCadenceHook 模式**：laew 的 Quality-Check Agent 可以借鉴这种"编辑后验证"纪律——在 SubAgent-Work 完成代码修改后，自动注入验证续接
2. **语言无关检测**：不枚举构建命令，而是排除只读命令（`bash_verifies` 的反向逻辑），laew 的 QC 可以复用
3. **HookChain 组合模式**：多个独立 hook（verify/todohook/redaction）通过 HookChain 组合为一个，laew 的多 Agent 架构可以引入类似的 hook 组合
4. **offer_continuation 续接**：laew 的 QC 可以在检测到未验证修改时，通过续接机制强制 Agent 回去验证
5. **Review Agent 作为工具**：将 Review Agent 挂载为主 agent 的一个工具，laew 可以让 Quality-Check Agent 以类似方式被 SubAgent-Work 调用
6. **nudged_for 幂等**：每个编辑只 nudged 一次，避免循环，laew 的 QC 应该引入类似的幂等机制

---

## 附录：关键设计模式总结

### 模式 1：Ephemeral vs Permanent 分离

- `pre_request` 操作克隆消息（ephemeral），不污染存储历史 → prefix cache 安全
- `session_start`/`turn_start`/`on_model_response` 操作原始消息（permanent），写入历史

### 模式 2：Kernel 中立 + L1/L2 特化

- L0 kernel 不知道"编码"/"审查"/"MCP"——只有 Agent 循环 + Tool trait + LifecycleHooks
- L1 capabilities 提供中立工具 + MCP + compaction
- L2 coding 组装 persona + discipline + plan mode

### 模式 3：Advisory metadata + Composable gate

- `RiskLevel` 是 advisory 的，kernel 不强制
- `ToolMiddleware`（ApprovalMiddleware/WriteApprovalGate/SensitivePathGate）是 composable 的 gate
- 每个 gate 独立，通过中间件链组合

### 模式 4：Monotonic / Idempotent compaction

- stub 是 monotonic 的（已 stub 的不重复）
- truncation 是 idempotent 的（TRUNCATE_MARKER 防止重复截断）
- summary 只在 drain 范围有非 anchor 内容时生成

### 模式 5：Fail-closed + Degrade gracefully

- 信任判定：corrupt store → untrusted
- 审批通道故障：Null response → Deny（不是 Allow）
- 摘要超时：降级为 gentle stub
- 空响应：重试 5 次后报错
