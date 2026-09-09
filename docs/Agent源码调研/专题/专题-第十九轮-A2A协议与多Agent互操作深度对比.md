# 专题 — 第十九轮：A2A 协议与多 Agent 互操作深度对比

> 覆盖 7 个参考工程的 **A2A/ACP/E2A/A2UI/MCP 双向/跨语言互操作/协议路由发现** 7 子维度深度对比
>
> 本报告是第十九轮深挖的 D11 核心专题，重点产出 **laew gap L1701-L1760**（新增 60 个 gap）

---

## 一、A2A 协议与多 Agent 互操作总览

### 1.1 核心概念定义（D11 子维度）

| 概念 | 定义 | 主导方 | 传输层 |
|------|------|--------|--------|
| **A2A** | Agent-to-Agent Protocol，跨 Agent 互操作开放协议 | Google | HTTP/SSE + JSON-RPC 2.0 |
| **ACP** | Agent Client Protocol，自动化客户端驱动协议 | Anthropic | stdio/ndJsonStream + JSON-RPC 2.0 |
| **E2A** | Event-to-Agent / Enterprise Agent | 自研 | 事件总线 + 有界发送 |
| **A2UI** | Agent-to-UI，结构化 UI 渲染协议 | 自研 | WebSocket / 渲染指令 |
| **MCP 双向** | Model Context Protocol 资源/工具/Sampling 双向 | Anthropic | stdio / Streamable HTTP |
| **跨语言互操作** | PyO3 / FFI / 类型映射 / 异步运行时适配 | — | 二进制 ABI |
| **协议路由发现** | 服务发现 / Agent 注册表 / 健康检查 | — | 注册表 / 心跳 |

### 1.2 7 工程的 D11 多 Agent 协作矩阵（本轮精细化）

| Agent | A2A | ACP | E2A | A2UI | MCP 双向 | 跨语言 | 路由发现 |
|-------|-----|-----|-----|------|---------|--------|---------|
| atomcode | — | ✅ stdio v1/v2 | — | — | ✅ 7 子模块 | — | ✅ SessionManager |
| claudecode | — | — | — | — | ✅ SSOT | — | — |
| deepseek-harness | — | ✅ Cordis 桥 | ✅ 事件总线 | ✅ Tagged JSON | ✅ MCP 客户端 | — | ✅ Cordis 注册中心 |
| openclaw | ✅ v1.0 3165 行 | ✅ stdio + Gateway 桥 | — | — | ✅ 双向 | — | ✅ SubAgent Registry 100+ 文件 |
| opencode | — | — | — | — | — | — | — |
| pi | — | — | — | — | — | — | — |
| undici | — | — | — | — | — | — | — |

> **关键发现**：仅 openclaw 实现了 A2A Protocol v1.0（3,165 行）；atomcode 和 deepseek-harness 实现了 ACP（atomcode 7,650 行 Rust，deepseek-harness 1,853 行 TypeScript）；openclaw 同时实现了 A2A + ACP 双协议。

---

## 二、D11-1 A2A 协议（Agent-to-Agent）

### 2.1 openclaw：A2A Protocol v1.0 完整实现（3,165 行）

#### 2.1.1 核心协议层（protocol.ts, 173 行）

**文件**: `extensions/a2a/src/protocol.ts:1-173`

```typescript
// protocol.ts:3-5 — 消息体上限与截断标记
const A2A_CONTEXT_PATTERN = /^[A-Za-z0-9._:-]{1,128}$/;
const A2A_MESSAGE_MAX_BYTES = 64 * 1024;
const A2A_TRUNCATION_MARKER = `\n[message truncated at ${A2A_MESSAGE_MAX_BYTES} bytes]`;

// protocol.ts:12-19 — A2A 消息记录
export type A2aMessageRecord = {
  messageId: string;
  contextId?: string;
  taskId?: string;
  role: "ROLE_USER" | "ROLE_AGENT";
  parts: A2aMessagePart[];
  metadata?: Record<string, unknown>;
};

// protocol.ts:21-31 — Task 6 态状态机
type A2aTaskStatus =
  | { state: "TASK_STATE_SUBMITTED" | "TASK_STATE_WORKING"; timestamp: string }
  | { state: "TASK_STATE_COMPLETED" | "TASK_STATE_FAILED" | "TASK_STATE_CANCELED" | "TASK_STATE_REJECTED"; timestamp: string; message?: A2aMessageRecord; };
```

**协议特性**:
- JSON-RPC 2.0 接口（`A2aRpcRequestSchema`, protocol.ts:47-52）
- 支持方法：`SendMessage` / `GetTask`（+ 兼容 `message/send` / `tasks/get`，protocol.ts:90-95）
- **不支持**：CancelTask / ListTasks / SubscribeToTask / PushNotification（protocol.ts:97-113，显式拒绝而非伪造）
- 消息体上限 64KB，超 UTF-8 安全截断（protocol.ts:128-162，多字节字符边界保护）
- contextId 格式限制 `[A-Za-z0-9._:-]{1,128}`（protocol.ts:3）

#### 2.1.2 Task 存储与状态机（task-store.ts, 210 行）

**文件**: `extensions/a2a/src/task-store.ts:1-210`

```typescript
// task-store.ts:5-7 — 终端任务保留策略
const A2A_TERMINAL_MAX_TASKS = 500;
const A2A_TERMINAL_RETENTION_MS = 24 * 60 * 60 * 1000;
const A2A_ERROR_MAX_LENGTH = 512;
```

**关键机制**:
- `A2aTaskStore` 类（task-store.ts:27-210）管理 Task 全生命周期
- 5 个内部 Map：tasks / taskOwners / pendingByContext / terminalTasks / waiters
- **FIFO 会话队列**：`completeNext()` 按 contextId + peerName 排队（task-store.ts:70-103）
- **等待者模式**：`wait()` 返回 Promise，任务完成或超时后 resolve（task-store.ts:114-134）
- **终端清理**：`#pruneTerminalTasks()` 24h 保留 / 500 条上限（task-store.ts:195-205）
- **安全隔离**：`get(taskId, ownerPeer)` 按 peer 隔离（task-store.ts:54-60）

#### 2.1.3 HTTP 服务端（http.ts, 383 行）

**文件**: `extensions/a2a/src/http.ts:1-383`

```typescript
// http.ts:27-33 — 请求/响应限制
const MAX_REQUEST_BODY_BYTES = 1024 * 1024;
const MAX_RESPONSE_BODY_BYTES = 1024 * 1024;
const MAX_BATCH_REQUESTS = 30;
const DEFAULT_REPLY_TIMEOUT_MS = 120_000;
const DEFAULT_RATE_LIMIT_PER_MINUTE = 30;
```

**关键机制**:
- **Agent Card 发现**：`createAgentCard()` 响应 `/.well-known/agent-card.json` 和 `/.well-known/agent.json`（http.ts:121-160）
  - 暴露 `supportedInterfaces[0] = { url, protocolBinding: "JSONRPC", protocolVersion: "1.0" }`
  - `capabilities: { streaming: false, pushNotifications: false }`
  - `skills` 仅暴露 agentId（不暴露 operator 撰写的 description，http.ts:152-158）
- **Peer 认证**：`resolvePeerName()` 用 SHA-256 + timingSafeEqual 比较 Bearer token（http.ts:95-109）
- **批处理**：支持 JSON-RPC batch，上限 30 请求（http.ts:346-372）
- **速率限制**：滑动窗口 30 req/min（http.ts:165-181）
- **SendMessage 处理**：创建 Task → 启动 → 分离 webhook 工作 → 立即返回或等待（http.ts:216-255）

#### 2.1.4 Gateway 路由（gateway.ts, 86 行）

**文件**: `extensions/a2a/src/gateway.ts:1-86`

```typescript
// gateway.ts:12-16 — A2A 固定路由路径
const a2aGatewayRoutePaths = [
  "/.well-known/agent-card.json",
  "/.well-known/agent.json",
  "/a2a/v1",
] as const;
```

**关键机制**:
- 注册 3 条 HTTP 路由（gateway.ts:57-73）
- 重复注册检测：`throwOnFailure: true` 防止 stale handler 覆盖（gateway.ts:59-69，安全公告 GHSA-RQP8-Q22P-5J9Q）
- 关闭顺序：先注销路由 → 再停止 store（gateway.ts:80-84）

#### 2.1.5 入站分发（inbound.ts, 137 行）

**文件**: `extensions/a2a/src/inbound.ts:1-137`

**关键机制**:
- **斜杠命令拒绝**：A2A peer 不能执行 `/` 命令（inbound.ts:27-32）
- **会话隔离**：`dmScope: "per-account-channel-peer"`，每个 peer+context 独立 session（inbound.ts:43）
- **allowlist 策略**：`dmPolicy: "allowlist"`，仅允许配置中的 peer（inbound.ts:58-59）
- **FIFO 交付**：`store.completeNext()` 按会话队列顺序交付（inbound.ts:118）

#### 2.1.6 出站发送（outbound.ts, 132 行）

**文件**: `extensions/a2a/src/outbound.ts:1-132`

**关键机制**:
- **SSRF 防护**：`fetchWithSsrFGuard()` 出站请求走 SSRF 策略（outbound.ts:79-84）
- **重定向禁止**：`maxRedirects: 0`（outbound.ts:86）
- **协议兼容重试**：Hermes-generation A2A 0.3 peer 用 `message/send` 方法名（outbound.ts:110-114）
- **超时**：30s（outbound.ts:11）

#### 2.1.7 配置模型（types.ts, 31 行）

**文件**: `extensions/a2a/src/types.ts:1-31`

```typescript
// types.ts:9-17 — A2A 通道配置
export type A2aChannelConfig = {
  enabled?: boolean;
  configWrites?: boolean;
  advertisedUrl?: string;
  replyTimeoutMs?: number;
  rateLimitPerMinute?: number;
  exposeAgents?: string[];
  peers?: Record<string, A2aPeerConfig>;
};
```

### 2.2 其他工程的 A2A 实现

| Agent | A2A 状态 | 说明 |
|-------|---------|------|
| atomcode | ❌ | 无 A2A 实现，仅 ACP |
| claudecode | ❌ | 无 A2A 实现 |
| deepseek-harness | ❌ | 无独立 A2A 模块（第十一轮报告中的"A2A"标注为 Cordis 事件总线，非 Google A2A Protocol） |
| opencode | ❌ | 无 A2A 实现 |
| pi | ❌ | 无 A2A 实现 |
| undici | ❌ | HTTP 客户端，非 Agent |

> **关键发现**：7 个参考工程中，**仅 openclaw 实现了 Google A2A Protocol v1.0**（3,165 行），其余工程均未实现。这是 laew 与其他 Agent 互操作的最大差距。

---

## 三、D11-2 ACP 协议（Agent Client Protocol）

### 3.1 atomcode：ACP stdio 服务器（7,650 行 Rust）

#### 3.1.1 模块结构

**目录**: `crates/atomcode-cli/src/acp/`（14 个文件，7,650 行）

| 文件 | 行数 | 功能 |
|------|------|------|
| `mod.rs` | 618 | 模块入口 + SharedState + serve_over |
| `v2.rs` | 1,571 | Draft v2 协议链 |
| `dispatch.rs` | 1,057 | 共享 session 表 + prompt turn 循环 |
| `options.rs` | 1,038 | Session config option 目录 |
| `sessions.rs` | 605 | Session 表 + cancel/close/delete |
| `commands.rs` | 449 | 命令处理 |
| `discovery.rs` | 346 | session/list 发现 + keyset 分页 |
| `elicitation.ts` | 345 | 用户输入回环（form/URL） |
| `turn.rs` | 388 | Prompt turn 循环 |
| `replay.rs` | 247 | 会话重放（replayFrom） |
| `permission.rs` | 294 | 权限请求 |
| `mcp.rs` | 213 | 客户端注入 MCP 服务器转换 |
| `translate.rs` | 264 | 协议翻译 |
| `engine.rs` | 215 | Kernel-native session agent |

#### 3.1.2 核心入口（mod.rs, 618 行）

**文件**: `crates/atomcode-cli/src/acp/mod.rs:1-200`

```rust
// mod.rs:3-8 — 模块声明
pub mod commands;
pub mod discovery;
pub mod dispatch;
pub mod elicitation;
pub mod engine;
pub mod mcp;
pub mod options;
pub mod permission;
pub mod replay;
pub mod sessions;
pub mod translate;
pub mod turn;
pub mod v2;

// mod.rs:67-86 — 共享状态
pub(crate) struct SharedState {
    pub sessions: Sessions,
    pub engine: Arc<Option<EngineConfig>>,
    pub provider_factory: Option<Arc<dyn CodingProviderFactory>>,
    pub auto_approve: bool,
    pub config_options: Arc<Vec<SessionConfigOption>>,
    pub model_resolver: Option<Arc<SessionModelResolver>>,
    pub effort_resolver: Option<Arc<SessionModelResolver>>,
    pub msg_ids: Arc<AtomicU64>,
    pub client_elicitation_form: Arc<AtomicBool>,
}

// mod.rs:156-158 — stdio 入口
pub async fn serve_stdio(opts: AcpServeOptions) -> anyhow::Result<()> {
    serve_over(opts, Stdio::new()).await
}
```

**关键机制**:
- **双协议代**：v1 稳定链 + v2 draft 链，按 SDK 协议路由器选择（mod.rs:196-200）
- **共享状态**：单 SharedState 克隆给两条链（mod.rs:177-194）
- **能力广告**：`v1_agent_capabilities()` 声明 load_session / prompt image / MCP http / session list/delete/resume（mod.rs:107-120）

#### 3.1.3 Session 发现（discovery.rs, 346 行）

**文件**: `crates/atomcode-cli/src/acp/discovery.rs:1-346`

```rust
// discovery.rs:20-21 — 分页限制
pub const SESSION_LIST_PAGE_SIZE: usize = 50;
const SESSION_LIST_CURSOR_PREFIX: &str = "atomcode-v1:";
```

**关键机制**:
- **合并视图**：live ACP sessions + 持久化 native session catalog（discovery.rs:53-107）
- **Keyset 分页**：cursor 编码最后 session id，支持 cwd 过滤（discovery.rs:125-143）
- **Legacy 排除**：`CatalogPresence::LegacyOnly` 记录不广告（discovery.rs:92-95）
- **Fork 折叠**：`SessionManager::collapse_fork_lineages()` 折叠分叉谱系（discovery.rs:88）

#### 3.1.4 MCP 服务器转换（mcp.rs, 213 行）

**文件**: `crates/atomcode-cli/src/acp/mcp.rs:1-213`

```rust
// mcp.rs:38-106 — ACP MCP → coding MCP 配置转换
pub fn acp_mcp_server_configs(mcp_servers: &[McpServer]) -> (Vec<McpServerConfig>, Vec<String>) {
    // Stdio（协议基线）+ HTTP（广告 via mcp_capabilities.http）连接
    // SSE 不连接（返回 ignored 列表）
    // source=McpConfigSource::Driver, trust=false（信任边界）
}
```

**关键机制**:
- **Stdio 基线**：协议要求每个 agent 必须支持（mcp.rs:43-76）
- **HTTP 支持**：广告 via `mcp_capabilities.http`（mcp.rs:77-100）
- **SSE 忽略**：不广告 SSE transport，返回 ignored 列表（mcp.rs:101）
- **信任边界**：`source: McpConfigSource::Driver, trust: false`（mcp.rs:72-73）

#### 3.1.5 端到端测试（acp_end_to_end.rs, 2,233 行）

**文件**: `crates/atomcode-cli/tests/acp_end_to_end.rs:1-2233`

**测试覆盖**:
- `initialize_new_prompt_streams_and_stops`：initialize → session/new → session/prompt 完整流（acp_end_to_end.rs:68-273）
- `resume_reconnects_to_persisted_session`：session/resume 重连持久化会话（acp_end_to_end.rs:280-438）
- `error_turn_does_not_poison_next_prompt_on_same_session`：错误 turn 不污染下一 prompt（acp_end_to_end.rs:450-561）
- `v2_client_negotiates_and_runs_prompt_lifecycle`：v2 协议协商 + state_update 流（acp_end_to_end.rs:568-759）
- `session_config_options_mode_and_effort`：mode + reasoning_effort 配置切换（acp_end_to_end.rs:765-943+）

**关键发现**：
- **v2 协议**：`Client.v2()` 驱动同一 agent，state_update 流 `running → chunks → idle`（acp_end_to_end.rs:606-717）
- **replayFrom**：`replayFrom: start` 重放持久化对话为 full-content user_message / agent_message（acp_end_to_end.rs:676-715）
- **未知 cursor 拒绝**：未知 replayFrom cursor 在 restore 前拒绝（acp_end_to_end.rs:700-715）

### 3.2 deepseek-harness：Cordis Fiber + ACP 桥（1,853 行）

#### 3.2.1 ACP 核心（packages/acp/acp/src/, 1,853 行）

**文件**: `packages/acp/acp/src/index.ts:1-534`

```typescript
// index.ts:1-10 — 模块说明
/**
 * Automation-only Agent Client Protocol server over JSON-RPC stdio.
 * The bridge exposes persistent harness sessions to trusted programmatic clients.
 */

// index.ts:59-61 — 注入服务
export const name = 'acp'
export const inject = ['agents', 'llm', 'sessionPersistence', 'sessions']
```

**关键机制**:
- **Cordis 插件**：通过 `apply(ctx, config)` 挂载（index.ts:96）
- **Session 表**：`Map<SessionId, AcpSession>`（index.ts:102）
- **事件桥接**：session/event + agent/inbox/claimed + agent/error + llm/adapters-updated（index.ts:134-149）
- **权限通道**：`approval/request` 事件桥接为 `session/request_permission`（index.ts:154-172）
  - 仅提供 `allow-once` / `reject-once` 单次选择
  - 从不从未知客户端响应推断持久授权

#### 3.2.2 Session 管理（session.ts, 527 行）

**文件**: `packages/acp/acp/src/session.ts:1-527`

```typescript
// session.ts:98-117 — AcpSession 类
export class AcpSession {
  readonly agent: Agent
  private readonly modelControl: AcpModelControl
  private outputTail = Promise.resolve()
  private inflight: InflightPrompt | undefined
  private closing: Promise<void> | undefined
  private readonly pendingSelections = new Map<string, ModelSelection>()
  // ...
}
```

**关键机制**:
- **创建/恢复**：`AcpSession.create()` / `AcpSession.resume()`（session.ts:126-172）
- **Agent 组合**：创建时安装 modelControl + mountAcpMcpServers（session.ts:126-139）
- **所有权验证**：`owns(agent)` / `ownsSession(session)` 精确引用比较（session.ts:179-190）
- **Inflight 管理**：`InflightPrompt` 结构跟踪进行中的 prompt（session.ts:48-62）

#### 3.2.3 协议编解码（codec.ts, 34 行）

**文件**: `packages/acp/acp/src/codec.ts:1-34`

```typescript
// codec.ts:14-34 — TurnEndReason → StopReason 映射
export function turnEndToStopReason(reason: TurnEndReason): StopReason {
  switch (reason.kind) {
    case 'completed': return 'end_turn'
    case 'max-tokens': return 'max_tokens'
    case 'aborted': return 'end_turn'  // hook/owner 中止 = 普通静止
    case 'interrupted': return 'cancelled'
    case 'blocked': case 'error': return 'end_turn'
    default: return 'end_turn'
  }
}
```

**关键设计**：`cancelled` 保留给显式客户端取消（`session/cancel`）和 disposal；hook/owner 中止报告 `end_turn`。

#### 3.2.4 MCP 挂载（mcp.ts, 143 行）

**文件**: `packages/acp/acp/src/mcp.ts:1-143`

```typescript
// mcp.ts:26-33 — MCP 服务器挂载
export async function mountAcpMcpServers(
  agentCtx: Context, servers: readonly McpServer[], sessionCwd: string,
): Promise<void> {
  const configs = resolveMcpConfigs(servers, sessionCwd)
  for (const config of configs) await agentCtx.plugin(McpClient, config)
}
```

**关键机制**:
- **Stdio 验证**：`isAbsolute(server.command)` 要求绝对路径（mcp.ts:45-46）
- **HTTP 验证**：`assertHttpUrl()` 限制 http/https 协议（mcp.ts:125-132）
- **名称规范化**：`normalizeServerName()` NFKD + slug + sha256 摘要（mcp.ts:111-122）
- **重复检测**：`Set<string>` 检测重复规范化名称（mcp.ts:38-41）

#### 3.2.5 模型控制（model-control.ts, 237 行）

**文件**: `packages/acp/acp/src/model-control.ts:1-237`

```typescript
// model-control.ts:32-52 — AcpModelControl 类
export class AcpModelControl {
  readonly selection: ModelSelectionRef
  private tail = Promise.resolve()
  private selected: ModelSelection | undefined
  private turnSelection: { turn: number; selection: ModelSelection } | undefined
  // ...
}
```

**关键机制**:
- **序列化**：`serialize()` 保持客户端突变接收顺序（model-control.ts:137-141）
- **Turn 固定**：`pinTurn()` / `releaseTurn()` 每个 prompt 固定路由（model-control.ts:75-85）
- **配置选项**：`model` + `reasoning_effort` 两个标准选项（model-control.ts:188-221）

#### 3.2.6 更新流（updates.ts, 111 行）

**文件**: `packages/acp/acp/src/updates.ts:1-111`

```typescript
// updates.ts:16-45 — assistant message → ACP 更新
export async function assistantUpdates(...): Promise<SessionUpdate[]> {
  // reasoning → agent_thought_chunk
  // content → agent_message_chunk
  // usage → usage_update
}

// updates.ts:52-61 — tool call → tool_call update
export function toolCallUpdate(event: SessionEvent<'tool/call'>): SessionUpdate {
  // sessionUpdate: 'tool_call', status: 'in_progress'
}

// updates.ts:69-85 — tool result → tool_call_update
export async function toolResultUpdate(...): Promise<SessionUpdate> {
  // status: result.isError ? 'failed' : 'completed'
}
```

### 3.3 openclaw：ACP stdio + Gateway 桥（17,001 行）

#### 3.3.1 ACP 服务端（src/acp/server.ts, 207 行）

**文件**: `src/acp/server.ts:1-207`

```typescript
// server.ts:1-2 — 模块说明
/** ACP stdio server that bridges Agent Client Protocol clients to the OpenClaw Gateway. */

// server.ts:5-11 — SDK 导入
import {
  AGENT_METHODS, AgentSideConnection, PROTOCOL_VERSION, ndJsonStream, type AnyMessage,
} from "@agentclientprotocol/sdk";
```

**关键机制**:
- **Gateway 桥**：ACP client → Gateway Client → OpenClaw Gateway（server.ts:95-100）
- **启动缓冲**：`createStartupInputMonitor()` 1MiB 启动输入缓冲（server.ts:37-92）
- **事件账本**：`createSqliteAcpEventLedger()` SQLite 持久化（server.ts:28）

#### 3.3.2 ACP Core（packages/acp-core/, 1,825 行）

**文件**: `packages/acp-core/src/types.ts:1-97`

```typescript
// types.ts:22-31 — AcpSession 类型
export type AcpSession = {
  sessionId: SessionId;
  sessionKey: string;
  ledgerSessionId?: string;
  cwd: string;
  createdAt: number;
  lastTouchedAt: number;
  abortController: AbortController | null;
  activeRunId: string | null;
};

// types.ts:65-82 — AcpSessionRuntimeOptions
export type AcpSessionRuntimeOptions = {
  runtimeMode?: string;  // "plan" / "normal" / "auto"
  model?: string;
  thinking?: string;
  cwd?: string;
  permissionProfile?: string;
  timeoutSeconds?: number;
  backendExtras?: Record<string, string>;
};
```

**文件**: `packages/acp-core/src/session.ts:1-191`

```typescript
// session.ts:33-34 — 默认限制
const DEFAULT_MAX_SESSIONS = 5_000;
const DEFAULT_IDLE_TTL_MS = 24 * 60 * 60 * 1_000;
```

**关键机制**:
- **内存 Session 存储**：`createInMemorySessionStore()`（session.ts:37-191）
- **容量上限**：5,000 session（session.ts:33）
- **空闲 TTL**：24h（session.ts:34）
- **LRU 驱逐**：`evictOldestIdleSession()` 驱逐最老空闲 session（session.ts:72-89）
- **活跃保护**：有 activeRunId 的 session 不被驱逐（session.ts:105-106）

#### 3.3.3 ACP 运行时（src/acp/, 17,001 行）

**关键文件**:
- `translator.ts`（207 行）：ACP ↔ Gateway 协议翻译
- `translator.session-state.ts`（252 行）：Session 状态机
- `translator.session-updates.ts`（243 行）：Session 更新流
- `translator.stop-reason.ts`（908 行测试）：StopReason 映射
- `policy.ts`：ACP 策略
- `event-ledger.ts`：SQLite 事件账本

### 3.4 其他工程的 ACP 实现

| Agent | ACP 状态 | 说明 |
|-------|---------|------|
| claudecode | ❌ | 无 ACP 实现 |
| opencode | ❌ | 无 ACP 实现 |
| pi | ❌ | 无 ACP 实现 |
| undici | ❌ | HTTP 客户端，非 Agent |

> **关键发现**：3 个工程实现了 ACP（atomcode 7,650 行 Rust、deepseek-harness 1,853 行 TypeScript、openclaw 17,001 行 TypeScript）。atomcode 是唯一用 Rust 实现 ACP 的工程，且支持 v1/v2 双协议代。

---

## 四、D11-3 E2A / Embedded Agent

### 4.1 deepseek-harness：E2A 事件总线

**核心发现**: deepseek-harness 的 E2A（Event-to-Agent）通过 Cordis Fiber 消息总线实现。

- **有界发送**：6MB 消息上限
- **心跳**：ping 30s / timeout 300s
- **LLM SSE 流补丁**：monkeypatch OpenAI SDK
- **事件类型**：session/event + agent/inbox/claimed + agent/error + llm/adapters-updated + approval/request

### 4.2 claudecode：Bridge 远程控制

**核心发现**: claudecode 的 Bridge 是**远程控制协议**（非 E2A）。

- **v1**: 基础 WebSocket 控制
- **v2**: 增强版，支持多路复用
- **Direct Connect Server**: 直接连接模式

### 4.3 openclaw：Talk Realtime Relay

**文件**: `src/gateway/talk-realtime-relay-session-create.ts`（685 行）

**关键机制**:
- 音频格式：20ms 帧的 24kHz mono PCM16（RELAY_OUTPUT_AUDIO_FRAME_BYTES=960）
- 支持 `session.continuity.reset`（连接重置后状态恢复）
- 支持 `forcedAgentConsult`（语音最终转录触发 agent 咨询）
- 支持 `RelayToolCallLedger`（工具调用去重）
- 支持 WebRTC / provider-websocket / gateway-relay / managed-room 四种传输
- 支持 agent-consult / direct-tools / none 三种 Brain 模式

### 4.4 其他工程的 E2A / Embedded

| Agent | E2A 状态 | 说明 |
|-------|---------|------|
| atomcode | — | 无显式 E2A，ACP 嵌入式场景 |
| opencode | — | 无显式 E2A |
| pi | — | 无显式 E2A |
| undici | — | HTTP 客户端 |

---

## 五、D11-4 A2UI（Agent-to-UI）

### 5.1 deepseek-harness：A2UI Tagged JSON

**核心发现**: deepseek-harness 的 A2UI 通过 Tagged JSON 实现。

- `build_prompt(language=...)` 语言分流
- 验证脚本 `verify_a2ui_bundle.py`
- 结构化 UI 渲染协议

### 5.2 atomcode：LiveViewHub 实时同步

**核心发现**: atomcode 的 LiveViewHub 是**实时 UI 同步**机制。

- 通过 Unix Socket IPC 推送 UI 更新
- 支持多客户端连接
- 消息序列化：serde_json / bincode

### 5.3 其他工程的 A2UI

| Agent | A2UI 状态 | 说明 |
|-------|---------|------|
| openclaw | — | 无显式 A2UI，Talk Realtime 部分覆盖 |
| claudecode | — | Ink Fork 渲染，非 A2UI 协议 |
| opencode | — | 无显式 A2UI |
| pi | — | 无显式 A2UI |
| undici | — | HTTP 客户端 |

---

## 六、D11-5 MCP 双向通信

### 6.1 openclaw：双向 MCP

**核心发现**: openclaw 的 MCP 是**双向**的（Resource / Tool / Prompt / Sampling）。

- 162 个 Extensions 中多个是 MCP 桥
- `mcp/` 目录实现 MCP 客户端/服务端
- 支持 stdio / SSE / Streamable HTTP 传输

### 6.2 claudecode：MCP SSOT

**核心发现**: claudecode 的 MCP 是**单一真相源**（Single Source of Truth）。

- 17 个 schema 迁移
- WebDAV/S3 同步
- 配置版本化

### 6.3 atomcode：MCP 7 子模块

**核心发现**: atomcode 内置 **7 个 MCP 子模块**。

| 子模块 | 功能 |
|--------|------|
| mcp-server | MCP 服务端 |
| mcp-client | MCP 客户端 |
| mcp-transport-stdio | stdio 传输 |
| mcp-transport-sse | SSE 传输 |
| mcp-transport-streamable-http | Streamable HTTP 传输 |
| mcp-tool-registry | 工具注册 |
| mcp-resource-manager | 资源管理 |

### 6.4 deepseek-harness：MCP 客户端

**文件**: `packages/mcp/mcp-client/`

**关键机制**:
- ACP 桥接：`mountAcpMcpServers()` 将 ACP MCP 声明转为 DSH MCP 客户端
- 支持 stdio / streamable-http 传输
- 名称规范化 + 重复检测 + 信任边界

### 6.5 与 D11-1/2/3 的差异

| 协议 | 方向 | 主导方 | 典型用途 |
|------|------|--------|---------|
| A2A | Agent ↔ Agent | Google | 跨 Agent 任务委派 |
| ACP | Client → Agent | Anthropic | 自动化客户端驱动 |
| E2A | Event → Agent | 自研 | 事件驱动 Agent |
| A2UI | Agent → UI | 自研 | 结构化 UI 渲染 |
| MCP | Client ↔ Server | Anthropic | 工具/资源/提示 |

---

## 七、D11-6 跨语言互操作

### 7.1 Switchyard：PyO3 工业级桥

**核心发现**: Switchyard 通过 **PyO3** 暴露 Python 接口。

- 6 平台 wheel 矩阵
- maturin 构建
- PyPI Trusted Publishing
- 协议 IR（ContentBlock::Unknown 保留未知字段）

### 7.2 atomcode：Rust 原生 ACP

**核心发现**: atomcode 的 ACP 服务器是**纯 Rust 实现**。

- 使用 `agent_client_protocol` crate（Rust ACP SDK）
- tokio 异步运行时
- 无 FFI 边界（全 Rust 栈）

### 7.3 其他工程的跨语言互操作

| Agent | 跨语言状态 | 说明 |
|-------|----------|------|
| openclaw | — | 纯 TypeScript |
| claudecode | — | TypeScript/Bun |
| deepseek-harness | — | 纯 TypeScript |
| opencode | — | TypeScript/Bun |
| pi | — | 纯 TypeScript |
| undici | — | 纯 JavaScript |

---

## 八、D11-7 协议路由与发现

### 8.1 openclaw：SubAgent Registry + Swarm FIFO

**核心发现**: openclaw 的 SubAgent Registry 是**最完善的 Agent 注册中心**。

**目录**: `src/agents/subagents/registry/`（100+ 文件）

| 文件 | 功能 |
|------|------|
| `subagent-registry.ts` (678 行) | 核心注册/生命周期/交付/steering/恢复 |
| `SubagentLifecycleController` | 生命周期控制 |
| `SubagentRegistryCompletionRuntime` | 完成运行时 |
| `SubagentRegistrySweeper` | 清理/退休 |
| `SubagentRegistryRestorer` | 重启恢复 |
| `SubagentRegistryListener` | 事件监听 |
| `store.sqlite.ts` | SQLite 持久化 |
| `subagent-orphan-recovery.ts` | 孤儿恢复 |
| `subagent-parent-recovery.ts` | 父恢复 |
| `suspended-delivery.ts` | 暂停交付 |
| `restart-recovery.ts` 系列 | 重启恢复 |

**Swarm 调度器** (`src/agents/subagents/swarm/swarm-scheduler.ts`):
- 每个 group 有并发上限 limit
- 容量等待队列
- `publishCapacityChange()` 通知父 agent 容量变化
- `code-mode-swarm.runtime.ts` 实现代码模式多 Agent 并行

### 8.2 deepseek-harness：Cordis 注册中心

**核心发现**: deepseek-harness 的 Cordis 是**一切皆插件**架构的注册中心。

- **六态生命周期**: registered → initialized → running → stopped → disposed → failed
- **Fiber epoch**: 每个 Fiber 实例有唯一 epoch 编号，防止重启后旧任务完成污染
- **事件分发**: session/event + agent/inbox/claimed + agent/error + approval/request

### 8.3 atomcode：SessionManager 发现

**文件**: `crates/atomcode-cli/src/acp/discovery.rs:1-346`

**关键机制**:
- **合并视图**：live ACP sessions + 持久化 native session catalog
- **Keyset 分页**：cursor 编码最后 session id
- **cwd 过滤**：支持按工作目录过滤
- **Legacy 排除**：不可恢复的记录不广告

### 8.4 其他工程的协议路由

| Agent | 路由发现状态 | 说明 |
|-------|------------|------|
| claudecode | — | 无显式注册中心 |
| opencode | — | Durable Object 持久化 |
| pi | — | Lane 三态 + Session Backends |
| undici | — | HTTP 客户端 |

---

## 九、7 工程 D11 子维度对比总表

### 9.1 A2A Protocol 对比

| 特性 | openclaw | atomcode | deepseek | claudecode | opencode | pi | undici |
|------|---------|---------|----------|-----------|---------|-----|--------|
| **实现规模** | 3,165 行 | — | — | — | — | — | — |
| **协议代** | v1.0 | — | — | — | — | — | — |
| **Agent Card** | ✅ /.well-known | — | — | — | — | — | — |
| **SendMessage** | ✅ | — | — | — | — | — | — |
| **GetTask** | ✅ | — | — | — | — | — | — |
| **CancelTask** | ❌ 显式拒绝 | — | — | — | — | — | — |
| **批处理** | ✅ 30 req/batch | — | — | — | — | — | — |
| **速率限制** | ✅ 30 req/min | — | — | — | — | — | — |
| **SSRF 防护** | ✅ | — | — | — | — | — | — |
| **消息上限** | 64KB | — | — | — | — | — | — |
| **Task 保留** | 24h / 500 条 | — | — | — | — | — | — |

### 9.2 ACP 协议对比

| 特性 | atomcode | deepseek-harness | openclaw |
|------|---------|-----------------|----------|
| **实现语言** | Rust | TypeScript | TypeScript |
| **实现规模** | 7,650 行 | 1,853 行 | 17,001 行 |
| **协议代** | v1 + v2 draft | v1 | v1 |
| **传输层** | stdio / Channel | stdio | stdio + Gateway 桥 |
| **Session 管理** | ✅ SessionManager | ✅ AcpSession | ✅ AcpSessionStore |
| **MCP 挂载** | ✅ stdio + http | ✅ stdio + http | ✅ |
| **模型控制** | ✅ mode + effort | ✅ AcpModelControl | ✅ |
| **权限请求** | ✅ | ✅ approval/request | ✅ |
| **Elicitation** | ✅ form/URL | — | — |
| **Session 恢复** | ✅ resume + replay | ✅ resume | ✅ |
| **发现分页** | ✅ keyset 50/page | — | — |
| **v2 state_update** | ✅ running→idle | — | — |
| **v2 replayFrom** | ✅ start + cursor | — | — |

### 9.3 跨语言互操作对比

| 特性 | Switchyard | atomcode | 其他 |
|------|-----------|---------|------|
| **桥接技术** | PyO3 | 全 Rust（无 FFI） | — |
| **平台矩阵** | 6 平台 wheel | cargo build | — |
| **构建工具** | maturin | cargo | — |
| **发布** | PyPI Trusted | crates.io | — |

### 9.4 协议路由发现对比

| 特性 | openclaw | deepseek-harness | atomcode |
|------|---------|-----------------|----------|
| **注册中心** | SubAgent Registry 100+ 文件 | Cordis 注册中心 | SessionManager |
| **持久化** | SQLite | Session | Native session catalog |
| **恢复机制** | 重启/孤儿/父恢复 | 续传 | resume + replay |
| **并发控制** | CommandLane 8 种 | Fiber epoch | — |
| **调度** | Swarm FIFO | — | — |
| **容量通知** | ✅ publishCapacityChange | — | — |

---

## 十、laew 现状（基于专题-laew实现进度对照表）

### 10.1 已实现的 D11 相关能力

| 能力 | 状态 | 实现位置 | 轮次 |
|------|------|---------|------|
| SubAgent 并行调度 | ✅ | `agent/orchestrator.rs::topo_layers` + `execute_workflows` | 第 05 轮 |
| SubAgent 执行轨迹 | ✅ | `src/agent/extrace.rs` | 第 05 轮 |
| 取消传播到 SubAgent | ✅ | `agent/cancel.rs` + `llm/cancellable.rs` | 第 07 轮 |
| LLM 熔断器 | ✅ | `llm/resilient.rs` | 第 01 轮 |
| MCP 工具注册 | 🟡 | `mcp-tool-registry` 子集 | 第 08 轮 |

### 10.2 缺失的 D11 核心能力

| 能力 | 状态 | 影响 |
|------|------|------|
| A2A Protocol | ❌ | 无法与其他 Agent 互操作 |
| ACP Server | ❌ | 无法被外部 ACP client 驱动 |
| SubAgent Registry | ❌ | SubAgent 无状态，跨重启全丢 |
| 并发控制（Lane） | ❌ | 多任务互相阻塞 |
| 协议路由发现 | ❌ | 无法发现其他 Agent |
| 跨语言桥接 | ❌ | 无法接入 Python/TS 生态 |

---

## 十一、laew gap L1701-L1760（新增 60 个 gap）

### 11.1 P0 紧急（20 个）

| Gap ID | 描述 | 影响 | 借鉴对象 |
|--------|------|------|---------|
| L1701 | 无 A2A Protocol 实现 | 无法与其他 Agent 互操作 | openclaw A2A 3,165 行 |
| L1702 | 无 ACP Server 实现 | 无法被外部 ACP client 驱动 | atomcode ACP 7,650 行 |
| L1703 | 无 Agent Card 暴露 | 其他 Agent 无法发现 laew | openclaw /.well-known/agent-card.json |
| L1704 | 无 SubAgent 注册/生命周期管理 | SubAgent 无状态，跨重启全丢 | openclaw Registry 100+ 文件 |
| L1705 | 无并发控制（Lane / Semaphore） | 多任务互相阻塞 | openclaw CommandLane 734 行 |
| L1706 | 无 SubAgent 恢复机制 | 崩溃后 SubAgent 全部丢失 | openclaw Restorer + Orphan Recovery |
| L1707 | 无 A2A SendMessage 处理 | 无法接收外部 Agent 任务 | openclaw protocol.ts 173 行 |
| L1708 | 无 A2A GetTask 查询 | 无法查询外部任务状态 | openclaw task-store.ts 210 行 |
| L1709 | 无 ACP initialize 握手 | 无法响应 ACP 客户端初始化 | atomcode mod.rs 618 行 |
| L1710 | 无 ACP session/new 创建 | 无法创建 ACP 会话 | deepseek-harness session.ts 527 行 |
| L1711 | 无 ACP session/prompt 驱动 | 无法响应 ACP prompt | deepseek-harness index.ts 534 行 |
| L1712 | 无 ACP MCP 挂载 | 无法连接客户端 MCP 服务器 | atomcode mcp.rs 213 行 |
| L1713 | 无 ACP 模型控制 | 无法切换模型/reasoning | deepseek-harness model-control.ts 237 行 |
| L1714 | 无 A2A 消息截断保护 | 超长消息崩溃 | openclaw protocol.ts:128-162 |
| L1715 | 无 A2A 速率限制 | 可被 DoS | openclaw http.ts:165-181 |
| L1716 | 无 A2A SSRF 防护 | 出站请求可被利用 | openclaw outbound.ts:79-84 |
| L1717 | 无 A2A Peer 认证 | 任何人可连接 | openclaw http.ts:95-109 |
| L1718 | 无 ACP StopReason 映射 | 协议不兼容 | deepseek-harness codec.ts 34 行 |
| L1719 | 无 ACP 更新流 | 客户端无进度 | deepseek-harness updates.ts 111 行 |
| L1720 | 无 A2A FIFO 会话队列 | 并发任务乱序 | openclaw task-store.ts:70-103 |

### 11.2 P1 重要（20 个）

| Gap ID | 描述 | 影响 | 借鉴对象 |
|--------|------|------|---------|
| L1721 | 无 A2A 批处理支持 | 低效单请求 | openclaw http.ts:346-372 |
| L1722 | 无 A2A 等待者模式 | 同步等待阻塞 | openclaw task-store.ts:114-134 |
| L1723 | 无 A2A 终端任务清理 | 内存泄漏 | openclaw task-store.ts:195-205 |
| L1724 | 无 A2A 会话隔离 | 任务互相干扰 | openclaw inbound.ts:43 |
| L1725 | 无 A2A allowlist 策略 | 未授权访问 | openclaw inbound.ts:58-59 |
| L1726 | 无 A2A 出站发送 | 无法主动联系其他 Agent | openclaw outbound.ts 132 行 |
| L1727 | 无 A2A 协议兼容重试 | Hermes 0.3 不兼容 | openclaw outbound.ts:110-114 |
| L1728 | 无 ACP v2 协议支持 | 不支持新特性 | atomcode v2.rs 1,571 行 |
| L1729 | 无 ACP replayFrom 重放 | 无法恢复历史 | atomcode replay.rs 247 行 |
| L1730 | 无 ACP elicitation 回环 | 无法请求用户输入 | atomcode elicitation.rs 345 行 |
| L1731 | 无 ACP session/list 发现 | 客户端无法枚举会话 | atomcode discovery.rs 346 行 |
| L1732 | 无 ACP keyset 分页 | 大表性能差 | atomcode discovery.rs:125-143 |
| L1733 | 无 ACP 权限请求通道 | 无法请求工具权限 | deepseek-harness index.ts:154-172 |
| L1734 | 无 ACP 事件账本 | 无审计追踪 | openclaw event-ledger.ts |
| L1735 | 无 ACP 会话容量上限 | 资源耗尽 | openclaw session.ts:33 |
| L1736 | 无 ACP 空闲 TTL | 僵尸会话 | openclaw session.ts:34 |
| L1737 | 无 ACP LRU 驱逐 | 内存泄漏 | openclaw session.ts:72-89 |
| L1738 | 无 ACP 活跃保护 | 运行中被驱逐 | openclaw session.ts:105-106 |
| L1739 | 无 SubAgent 暂停/恢复 | 无法暂停长任务 | openclaw suspended-delivery |
| L1740 | 无 SubAgent 孤儿恢复 | 父死子丢 | openclaw subagent-orphan-recovery |

### 11.3 P2 进阶（20 个）

| Gap ID | 描述 | 影响 | 借鉴对象 |
|--------|------|------|---------|
| L1741 | 无 E2A 事件总线 | 无法事件驱动 | deepseek-harness Cordis |
| L1742 | 无 A2UI 协议 | 无法结构化 UI 渲染 | deepseek-harness A2UI |
| L1743 | 无 Talk Realtime Relay | 无法语音对话 | openclaw 685 行 |
| L1744 | 无 AgentHarness 注册契约 | 无法接入外部 Agent 运行时 | openclaw 592 行 |
| L1745 | 无 Workboard 多 Agent 工作板 | 多 Agent 无法协作 | openclaw 24K 行 |
| L1746 | 无 Swarm 并行调度 | 无法多 Agent 并行执行 | openclaw swarm-scheduler |
| L1747 | 无 Leader-Teammate 模式 | 无法多角色协作 | jiuwenswarm Leader-Teammate |
| L1748 | 无 Cordis Fiber 消息总线 | 无法插件间通信 | deepseek-harness Cordis |
| L1749 | 无 SubAgent 11 包适配 | 无法适配多种 SubAgent 后端 | deepseek-harness 11 包 |
| L1750 | 无二进制帧协议 | 延迟高 | pi 4-byte + CBOR |
| L1751 | 无 WriterLease 乐观锁 | 写冲突 | pi proper-lockfile |
| L1752 | 无 PyO3 跨语言桥 | 无法接入 Python 生态 | Switchyard PyO3 |
| L1753 | 无 MCP 双向 Resource | 无法暴露资源 | openclaw 双向 MCP |
| L1754 | 无 MCP 双向 Sampling | 无法代理 LLM 调用 | openclaw 双向 MCP |
| L1755 | 无 MCP SSOT | 配置不一致 | claudecode MCP SSOT |
| L1756 | 无协议 IR | 协议扩展性差 | Switchyard IR |
| L1757 | 无服务健康检查 | 无法检测 Agent 故障 | openclaw gateway/health |
| L1758 | 无 Agent 注册表分页 | 大表性能差 | atomcode keyset 分页 |
| L1759 | 无 ACP 配置选项目录 | 无法动态配置 | atomcode options.rs 1,038 行 |
| L1760 | 无 ACP 翻译层 | 协议不兼容 | openclaw translator.ts 207 行 |

---

## 十二、推荐 Rust crate 清单

| 类别 | crate 名称 | 用途 | 优先级 |
|------|-----------|------|--------|
| **A2A 协议** | `jsonrpsee` | A2A JSON-RPC 2.0 服务端 | P0 |
| **A2A 协议** | `serde_json` | JSON-RPC 消息序列化 | P0 |
| **ACP 协议** | `agent-client-protocol` | ACP SDK（atomcode 同款） | P0 |
| **ACP 协议** | `tokio::io::AsyncBufRead` | ndJsonStream 解析 | P0 |
| **并发控制** | `tokio::sync::Semaphore` | 替代 CommandLane | P0 |
| **并发控制** | `tokio::task::JoinSet` | SubAgent 并发管理 | P0 |
| **持久化** | `rusqlite` + WAL | Subagent Registry 持久化 | P0 |
| **HTTP 服务** | `axum` | A2A HTTP 服务端 | P0 |
| **SSRF 防护** | `url` + 自定义策略 | 出站请求 SSRF 防护 | P0 |
| **速率限制** | `governor` | 滑动窗口速率限制 | P1 |
| **二进制帧** | `ciborium` | CBOR 二进制帧 | P2 |
| **WebSocket** | `tokio-tungstenite` | WebSocket 传输层 | P1 |
| **文件锁** | `fs2` | WriterLease 乐观锁 | P2 |
| **跨语言** | `pyo3` | Python 桥接 | P2 |
| **服务发现** | `mdns-sd` | 局域网 Agent 发现 | P2 |
| **健康检查** | `health-check` | Agent 健康检查 | P2 |

---

## 十三、与前 18 轮关系

```
第十九轮（2026-09-09）  ← 本轮 D11 A2A 协议与多 Agent 互操作（7 子维度）
  ↓
第十八轮（2026-09-09）  ← 用户交互体验层 D1-D8（L1396-L1590）
  ↓
第十七轮（2026-09-09）  ← 崩溃恢复/多租户/RRF/Pregel/Skill/预热池/Turn 锁/HTTP/安全（L1166-L1395+）
  ↓
第十六轮（2026-09-09）  ← 多轮对话恢复/压缩管线/内存加密/SQLite/Hook/租约/IoC/LLM协议栈/扩展加载/录制回放/守护进程（L1036-L1165+）
  ↓
第十一轮（2026-09-07）  ← Agent 协作与多 Agent 通信协议（L143-L165，A2A/ACP/E2A/A2UI 基础覆盖）
  ↓
...
```

> **与第十一轮的关系**：第十一轮首次覆盖 A2A/ACP/E2A/A2UI 基础概念（L143-L165），本轮（D11）进行**源码级深度对比**，新增 60 个 gap（L1701-L1760），覆盖 7 子维度 × 7 工程的完整实现细节。

---

## 十四、A2A/ACP 协议 Wire Format 深度对比

### 14.1 A2A SendMessage Wire Format（openclaw 实现）

**文件**: `extensions/a2a/src/outbound.ts:60-73`

```json
{
  "jsonrpc": "2.0",
  "id": "<uuid>",
  "method": "SendMessage",
  "params": {
    "message": {
      "messageId": "<uuid>",
      "role": "ROLE_USER",
      "contextId": "ctx-oc-<peerName>",
      "parts": [{ "text": "..." }]
    },
    "configuration": { "returnImmediately": true }
  }
}
```

**协议细节**（protocol.ts:60-78）:
- `contextId` 格式：`^[A-Za-z0-9._:-]{1,128}$`
- `role`：`ROLE_USER` / `ROLE_AGENT`（兼容小写 `user` / `agent`）
- `parts`：`text` / `data` / `url` / `raw` 四种
- `configuration.acceptedOutputModes`：可选输出模式
- `configuration.historyLength`：历史长度限制
- `configuration.returnImmediately`：立即返回 vs 等待完成

### 14.2 A2A GetTask Wire Format

**文件**: `extensions/a2a/src/protocol.ts:80-84`

```json
{
  "jsonrpc": "2.0",
  "id": "<uuid>",
  "method": "GetTask",
  "params": {
    "id": "<taskId>",
    "historyLength": 10,
    "tenant": "<tenant>"
  }
}
```

### 14.3 A2A Agent Card Wire Format

**文件**: `extensions/a2a/src/http.ts:121-160`

```json
{
  "name": "OpenClaw",
  "description": "OpenClaw agent gateway using the Agent2Agent protocol.",
  "supportedInterfaces": [{
    "url": "https://host/a2a/v1",
    "protocolBinding": "JSONRPC",
    "protocolVersion": "1.0"
  }],
  "version": "1.0.0",
  "capabilities": {
    "streaming": false,
    "pushNotifications": false
  },
  "defaultInputModes": ["text/plain"],
  "defaultOutputModes": ["text/plain"],
  "skills": [
    { "id": "agent-1", "name": "agent-1", "description": "OpenClaw agent agent-1.", "tags": ["openclaw"] }
  ]
}
```

**安全设计**（http.ts:152-158）：Agent Card 无认证暴露，仅 agentId 跨越发现边界，operator 撰写的 description 不发布。

### 14.4 ACP Initialize Wire Format（atomcode 实现）

**文件**: `crates/atomcode-cli/tests/acp_end_to_end.rs:125-128`

```rust
// atomcode ACP Initialize 请求
let init = conn
    .send_request(InitializeRequest::new(ProtocolVersion::V1))
    .block_task()
    .await?;
```

**能力广告**（mod.rs:107-120）:
```rust
fn v1_agent_capabilities() -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(PromptCapabilities::new().image(true))
        .mcp_capabilities(McpCapabilities::new().http(true))
        .session_capabilities(
            SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .delete(SessionDeleteCapabilities::new())
                .close(SessionCloseCapabilities::new())
                .resume(SessionResumeCapabilities::new())
                .additional_directories(SessionAdditionalDirectoriesCapabilities::new()),
        )
}
```

### 14.5 ACP Session/New Wire Format（deepseek-harness 实现）

**文件**: `packages/acp/acp/src/index.ts:195-200`

```typescript
async newSession(params: NewSessionRequest, signal: AbortSignal): Promise<NewSessionResponse> {
  assertOpen()
  validateWorkspaceParams(params)
  const sessionId = SessionId(randomUUID())
  // ... Agent 组合 + MCP 挂载
}
```

### 14.6 ACP StopReason 映射（deepseek-harness codec.ts）

**文件**: `packages/acp/acp/src/codec.ts:14-34`

| DSH TurnEndReason | ACP StopReason |
|-------------------|----------------|
| `completed` | `end_turn` |
| `max-tokens` | `max_tokens` |
| `aborted` | `end_turn` |
| `interrupted` | `cancelled` |
| `blocked` | `end_turn` |
| `error` | `end_turn` |

**关键设计**：`cancelled` 保留给显式客户端取消（`session/cancel`）和 disposal；hook/owner 中止报告 `end_turn`。

### 14.7 ACP Session Update Wire Format（deepseek-harness updates.ts）

**文件**: `packages/acp/acp/src/updates.ts:16-85`

```typescript
// assistant message → 3 种 update
{ sessionUpdate: 'agent_thought_chunk', messageId, content: { type: 'text', text } }
{ sessionUpdate: 'agent_message_chunk', messageId, content }
{ sessionUpdate: 'usage_update', used, size }

// tool call → tool_call update
{ sessionUpdate: 'tool_call', toolCallId, title, kind: 'other', status: 'in_progress', rawInput }

// tool result → tool_call_update
{ sessionUpdate: 'tool_call_update', toolCallId, status: 'completed'|'failed', content }
```

### 14.8 ACP Model Control Wire Format（deepseek-harness model-control.ts）

**文件**: `packages/acp/acp/src/model-control.ts:188-221`

```typescript
// 配置选项结构
const options: SessionConfigOption[] = [{
  id: 'model',
  name: 'Model',
  category: 'model',
  type: 'select',
  currentValue: JSON.stringify([provider, model]),
  options: groups  // provider → models
}, {
  id: 'reasoning_effort',
  name: 'Reasoning effort',
  category: 'thought_level',
  type: 'select',
  currentValue: resolved.reasoningEffort ?? '',
  options: [
    { value: '', name: 'Provider default' },  // 仅当无默认时
    ...info.reasoning.efforts.map(e => ({ value: e.id, name: e.name }))
  ]
}]
```

---

## 十五、openclaw A2A 扩展完整文件清单

### 15.1 扩展入口与配置

| 文件 | 行数 | 功能 |
|------|------|------|
| `extensions/a2a/index.ts` | 16 | 扩展入口，defineBundledChannelEntry |
| `extensions/a2a/setup-entry.ts` | 13 | 设置入口 |
| `extensions/a2a/setup-plugin-api.ts` | 1 | 设置插件 API |
| `extensions/a2a/runtime-api.ts` | 7 | 运行时 API |
| `extensions/a2a/openclaw.plugin.json` | — | 插件清单 |
| `extensions/a2a/package.json` | — | 包配置 |
| `extensions/a2a/tsconfig.json` | — | TS 配置 |

### 15.2 核心实现

| 文件 | 行数 | 功能 |
|------|------|------|
| `extensions/a2a/src/protocol.ts` | 173 | 核心协议 + Task 6 态 + 方法路由 |
| `extensions/a2a/src/types.ts` | 31 | 配置类型 |
| `extensions/a2a/src/task-store.ts` | 210 | Task 存储 + FIFO 队列 + 等待者 |
| `extensions/a2a/src/gateway.ts` | 86 | Gateway 路由注册 |
| `extensions/a2a/src/http.ts` | 383 | HTTP 服务端 + Agent Card + 批处理 |
| `extensions/a2a/src/inbound.ts` | 137 | 入站分发 + 会话隔离 |
| `extensions/a2a/src/outbound.ts` | 132 | 出站发送 + SSRF 防护 |
| `extensions/a2a/src/channel.ts` | 87 | 通道插件定义 |
| `extensions/a2a/src/channel-base.ts` | — | 通道基类 |
| `extensions/a2a/src/channel.setup.ts` | — | 通道设置 |
| `extensions/a2a/src/accounts.ts` | — | 账户解析 |
| `extensions/a2a/src/status.ts` | — | 状态报告 |
| `extensions/a2a/src/config-schema.ts` | — | 配置 Schema |

### 15.3 测试

| 文件 | 行数 | 功能 |
|------|------|------|
| `extensions/a2a/src/protocol.test.ts` | — | 协议测试 |
| `extensions/a2a/src/gateway.test.ts` | — | Gateway 测试 |
| `extensions/a2a/src/http.test.ts` | — | HTTP 测试 |
| `extensions/a2a/src/channel.test.ts` | — | 通道测试 |
| `extensions/a2a/src/inbound.test.ts` | — | 入站测试 |
| `extensions/a2a/src/outbound.test.ts` | — | 出站测试 |

---

## 十六、atomcode ACP 完整文件清单

### 16.1 核心实现

| 文件 | 行数 | 功能 |
|------|------|------|
| `crates/atomcode-cli/src/acp/mod.rs` | 618 | 模块入口 + SharedState + serve_over |
| `crates/atomcode-cli/src/acp/v2.rs` | 1,571 | Draft v2 协议链 |
| `crates/atomcode-cli/src/acp/dispatch.rs` | 1,057 | 共享 session 表 + prompt turn 循环 |
| `crates/atomcode-cli/src/acp/options.rs` | 1,038 | Session config option 目录 |
| `crates/atomcode-cli/src/acp/sessions.rs` | 605 | Session 表 + cancel/close/delete |
| `crates/atomcode-cli/src/acp/commands.rs` | 449 | 命令处理 |
| `crates/atomcode-cli/src/acp/discovery.rs` | 346 | session/list 发现 + keyset 分页 |
| `crates/atomcode-cli/src/acp/elicitation.rs` | 345 | 用户输入回环（form/URL） |
| `crates/atomcode-cli/src/acp/turn.rs` | 388 | Prompt turn 循环 |
| `crates/atomcode-cli/src/acp/replay.rs` | 247 | 会话重放（replayFrom） |
| `crates/atomcode-cli/src/acp/permission.rs` | 294 | 权限请求 |
| `crates/atomcode-cli/src/acp/mcp.rs` | 213 | 客户端注入 MCP 服务器转换 |
| `crates/atomcode-cli/src/acp/translate.rs` | 264 | 协议翻译 |
| `crates/atomcode-cli/src/acp/engine.rs` | 215 | Kernel-native session agent |

### 16.2 测试与示例

| 文件 | 行数 | 功能 |
|------|------|------|
| `crates/atomcode-cli/examples/acp_stdio_agent.rs` | 7 | stdio 入口示例 |
| `crates/atomcode-cli/tests/acp_end_to_end.rs` | 2,233 | 端到端集成测试 |
| `crates/atomcode-cli/tests/acp_initialize.rs` | — | Initialize 测试 |

---

## 十七、deepseek-harness ACP 完整文件清单

### 17.1 核心实现

| 文件 | 行数 | 功能 |
|------|------|------|
| `packages/acp/acp/src/index.ts` | 534 | 插件入口 + 事件桥接 |
| `packages/acp/acp/src/session.ts` | 527 | Session 生命周期 |
| `packages/acp/acp/src/content.ts` | 237 | 内容转换 |
| `packages/acp/acp/src/model-control.ts` | 237 | 模型控制 |
| `packages/acp/acp/src/mcp.ts` | 143 | MCP 挂载 |
| `packages/acp/acp/src/codec.ts` | 34 | StopReason 映射 |
| `packages/acp/acp/src/updates.ts` | 111 | Session 更新流 |
| `packages/acp/acp/src/invariant.ts` | 30 | 不变量检查 |

### 17.2 测试

| 文件 | 行数 | 功能 |
|------|------|------|
| `packages/acp/acp/tests/approval.spec.ts` | — | 权限测试 |
| `packages/acp/acp/tests/edges.spec.ts` | — | 边界测试 |
| `packages/acp/acp/tests/turns.spec.ts` | — | Turn 测试 |
| `packages/acp/acp/tests/codec.spec.ts` | — | 编解码测试 |
| `packages/acp/acp/tests/content.spec.ts` | — | 内容测试 |
| `packages/acp/acp/tests/mcp.spec.ts` | — | MCP 测试 |
| `packages/acp/acp/tests/model-control.spec.ts` | — | 模型控制测试 |
| `packages/acp/acp/tests/dispose.spec.ts` | — | 销毁测试 |
| `packages/acp/acp/tests/updates.spec.ts` | — | 更新测试 |
| `packages/acp/acp/tests/multi-session.spec.ts` | — | 多 Session 测试 |
| `packages/acp/acp/tests/bridge.spec.ts` | — | 桥接测试 |
| `packages/acp/acp/tests/harness.ts` | — | 测试工具 |

---

## 十八、openclaw ACP 完整文件清单

### 18.1 ACP Core（packages/acp-core/）

| 文件 | 行数 | 功能 |
|------|------|------|
| `packages/acp-core/src/types.ts` | 96 | 核心类型定义 |
| `packages/acp-core/src/session.ts` | 191 | Session 存储 + LRU 驱逐 |
| `packages/acp-core/src/session-interaction-mode.ts` | 56 | Session 交互模式 |
| `packages/acp-core/src/session-lineage-meta.ts` | 78 | Session 谱系元数据 |
| `packages/acp-core/src/meta.ts` | 51 | 元数据 |
| `packages/acp-core/src/error-format.ts` | — | 错误格式化 |
| `packages/acp-core/src/structured-auth-redaction.ts` | 409 | 结构化认证脱敏 |
| `packages/acp-core/src/index.ts` | — | 入口 |

### 18.2 ACP 运行时（src/acp/）

| 文件 | 行数 | 功能 |
|------|------|------|
| `src/acp/server.ts` | 207 | stdio 服务端 |
| `src/acp/translator.ts` | 207 | 协议翻译 |
| `src/acp/translator.session-state.ts` | 252 | Session 状态机 |
| `src/acp/translator.session-updates.ts` | 243 | Session 更新流 |
| `src/acp/translator.stop-reason.ts` | — | StopReason 映射 |
| `src/acp/translator.agent-events.ts` | — | Agent 事件翻译 |
| `src/acp/translator.presentation.ts` | — | 呈现翻译 |
| `src/acp/translator.replay.ts` | — | 重放翻译 |
| `src/acp/translator.prompt-state.ts` | — | Prompt 状态 |
| `src/acp/policy.ts` | — | 策略 |
| `src/acp/event-ledger.ts` | — | 事件账本 |
| `src/acp/event-ledger.types.ts` | — | 事件账本类型 |
| `src/acp/types.ts` | 10 | 类型 |
| `src/acp/secret-file.ts` | — | 密钥文件 |

---

## 十九、关键修正：第十一轮报告偏差

### 14.1 修正 1：A2A 实现归属

| 第十一轮描述 | 实际归属 | 修正 |
|------------|---------|------|
| "deepseek-harness: A2A ✅" | ❌ 无 A2A Protocol | deepseek-harness 仅有 Cordis 事件总线，非 Google A2A Protocol |
| "jiuwenswarm: A2A ✅" | ❌ 无 A2A Protocol | jiuwenswarm 通过 MCP 暴露，非 A2A Protocol |
| "openclaw: A2A v1.0 ✅" | ✅ 正确 | openclaw 是唯一实现 A2A Protocol v1.0 的工程 |

### 14.2 修正 2：ACP 实现补充

| 第十一轮描述 | 实际归属 | 修正 |
|------------|---------|------|
| "atomcode: ACP —" | ❌ 有 ACP | atomcode 有 7,650 行 Rust ACP 实现（v1 + v2） |
| "claudecode: ACP —" | ✅ 正确 | claudecode 无 ACP 实现 |
| "openclaw: ACP stdio ✅" | ✅ 正确 | openclaw 有 17,001 行 ACP 实现 |

### 14.3 修正 3：E2A/A2UI 实现

| 第十一轮描述 | 实际归属 | 修正 |
|------------|---------|------|
| "openclaw: E2A —" | ✅ 正确 | openclaw 无显式 E2A |
| "openclaw: A2UI —" | ✅ 正确 | openclaw 无显式 A2UI |
| "deepseek-harness: E2A ✅" | ✅ 正确 | Cordis 事件总线 |
| "deepseek-harness: A2UI ✅" | ✅ 正确 | Tagged JSON |

---

## 十五、总结

### 15.1 核心发现

1. **openclaw 的 A2A Protocol v1.0**（3,165 行）是 7 个参考工程中**唯一**的 A2A 实现，包含完整的 Agent Card 发现、Task 状态机、FIFO 会话队列、SSRF 防护、速率限制
2. **atomcode 的 ACP stdio 服务器**（7,650 行 Rust）是**唯一**用 Rust 实现的 ACP，支持 v1/v2 双协议代、session resume、replayFrom、elicitation 回环
3. **deepseek-harness 的 ACP 桥**（1,853 行）通过 Cordis Fiber 消息总线实现事件驱动，支持 MCP 挂载、模型控制、权限请求
4. **openclaw 的 ACP 实现**（17,001 行）是最完善的，包含 Gateway 桥、事件账本、Session 存储、翻译层
5. **claudecode / opencode / pi / undici** 均无 A2A/ACP 实现
6. **Switchyard 的 PyO3** 是唯一的跨语言互操作实现

### 15.2 laew 最急需的 5 个 D11 能力

1. **ACP Server**（P0）— 被外部 ACP client 驱动（atomcode 7,650 行 Rust 可直接借鉴）
2. **A2A Protocol**（P0）— 与其他 Agent 互操作（openclaw 3,165 行）
3. **SubAgent Registry**（P0）— 注册/生命周期/恢复（openclaw 100+ 文件）
4. **Agent Card 暴露**（P0）— 被其他 Agent 发现（openclaw /.well-known/agent-card.json）
5. **并发控制 Lane**（P0）— 多任务调度（openclaw CommandLane 734 行）

### 15.3 关键数据点

| 维度 | 数据 |
|------|------|
| openclaw A2A 扩展 | 3,165 行 |
| openclaw A2A 文件数 | 13 个 |
| openclaw A2A Task 保留 | 24h / 500 条 |
| openclaw A2A 消息上限 | 64KB |
| openclaw A2A 速率限制 | 30 req/min |
| openclaw A2A 批处理上限 | 30 req/batch |
| atomcode ACP 规模 | 7,650 行 Rust |
| atomcode ACP 文件数 | 14 个 |
| atomcode ACP 协议代 | v1 + v2 draft |
| atomcode ACP 分页 | 50/page keyset |
| deepseek-harness ACP 规模 | 1,853 行 |
| deepseek-harness ACP 文件数 | 8 核心 + 12 测试 |
| openclaw ACP 规模 | 17,001 行 |
| openclaw ACP Core | 1,825 行 |
| openclaw ACP 翻译层 | 207 行 |
| openclaw ACP Session 上限 | 5,000 |
| openclaw ACP 空闲 TTL | 24h |
| openclaw SubAgent Registry | 100+ 文件 |
| openclaw Swarm 调度 | ✅ |
| openclaw Talk Realtime | 685 行 |
| 新增 laew gap | L1701-L1760（60 个） |
| 累计 laew gap | L1-L1760（1,760 个） |

---

> **本报告完成标记**
> - 文件路径: `专题-第十九轮-A2A协议与多Agent互操作深度对比.md`
> - 覆盖 7 个源码工程
> - 新增 laew gap: L1701-L1760（60 个）
> - 累计 laew gap: L1-L1760（1,760 个）
> - 包含 15 个章节，涵盖 A2A/ACP/E2A/A2UI/MCP/跨语言/路由发现 7 子维度
> - 精确行号标注：openclaw A2A 13 文件、atomcode ACP 14 文件、deepseek-harness ACP 8 文件、openclaw ACP 17,001 行
