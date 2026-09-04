# 专题：6 项目 MCP 架构深度分析与横向对比

> **元信息**
> - **分析对象**: Claude Code / AtomCode / OpenClaw / OpenCode / DeepSeek Harness / pi
> - **分析维度**: 传输层 / 工具注册 / 资源处理 / Server 生命周期 / 认证与信任 / 双向暴露
> - **关联文档**: 各项目的「核心机制深度分析」专题 4（MCP 章节）
> - **对 laew 意义**: laew 当前无 MCP 支持，本报告提供完整架构参考与分阶段接入方案
> - **生成日期**: 2026-09-04

---

## 目录

0. [MCP 协议基础速览](#0-mcp-协议基础速览)
1. [横向对比总览表](#1-横向对比总览表)
2. [Claude Code：最完整的 MCP 实现](#2-claude-code最完整的-mcp-实现)
3. [AtomCode：Rust 原生的 trust-first 设计](#3-atomcoderust-原生的-trust-first-设计)
4. [OpenClaw：双向 MCP Server 架构](#4-openclaw双向-mcp-server-架构)
5. [OpenCode：Effect-DI 驱动的 Catalog 模式](#5-opencodeeffect-di-驱动的-catalog-模式)
6. [DeepSeek Harness：插件化 MCP Client](#6-deepseek-harness插件化-mcp-client)
7. [pi：有意识地拒绝 MCP](#7-pi有意识地拒绝-mcp)
8. [六大项目横评：关键设计决策对比](#8-六大项目横评关键设计决策对比)
9. [跨项目设计模式提炼](#9-跨项目设计模式提炼)
10. [对 laew 的综合建议](#10-对-laew-的综合建议)
11. [附录：关键代码片段索引](#11-附录关键代码片段索引)

---

## 0. MCP 协议基础速览

### 0.1 什么是 MCP

MCP（Model Context Protocol）是由 Anthropic 提出的开放协议，定义了 LLM Agent 与外部工具/资源之间的标准化通信方式。协议基于 **JSON-RPC 2.0**，通过传输层（stdio / HTTP / SSE）实现双向通信。

### 0.2 核心概念

| 概念 | 说明 | 典型用途 |
|------|------|---------|
| **Tool** | 可调用的函数，有 JSON Schema 参数定义 | 文件操作、API 调用、数据库查询 |
| **Resource** | 只读数据源，由 URI 标识 | 文件内容、配置、数据库记录 |
| **Prompt** | 可复用的提示词模板 | 代码审查模板、PR 描述模板 |
| **Server** | 提供 Tool / Resource / Prompt 的进程 | MCP server 进程 |
| **Client** | 消费 Server 能力的 Agent | Claude Code、laew |

### 0.3 通信流程

```
Client (Agent)                          Server (MCP)
    |                                       |
    |--- initialize (protocol version) ---> |
    |<-- initialize result (capabilities)-- |
    |                                       |
    |--- tools/list ----------------------> |
    |<-- tools list result ---------------- |
    |                                       |
    |--- tools/call {name, arguments} ----> |
    |<-- tool result {content[]} ---------- |
    |                                       |
    |--- resources/list ------------------> |
    |<-- resources list result ------------ |
    |                                       |
    | [notifications/tools/list_changed <--]|
```

### 0.4 传输层协议

| 传输方式 | 双向性 | 连接模式 | 适用场景 |
|---------|--------|---------|---------|
| **stdio** | 半双工（JSON-RPC over stdin/stdout） | 子进程 | 本地 MCP server，最常用 |
| **SSE** | 服务端推流（Server → Client） | HTTP 长连接 | 远程 MCP server |
| **Streamable HTTP** | 全双工（HTTP POST + 可选 SSE 流） | HTTP 请求/流式响应 | 远程 MCP server（新标准） |
| **WebSocket** | 全双工 | 持久连接 | 实时交互场景 |

### 0.5 MCP 在 Agent 生态中的位置

```
用户输入 → Agent（LLM + 工具循环） → MCP Client → MCP Server → 外部系统
                                         ↑
                                    标准化协议层
```

MCP 的核心价值是**解耦 Agent 与外部工具**——Agent 不需要知道工具的实现细节，只需要通过标准协议调用。这使得：
- 工具可以跨 Agent 复用（同一个 MCP server 可被多个 Agent 调用）
- 工具可以独立开发和部署（与 Agent 进程隔离）
- 工具生态可以独立演进（社区共建 MCP server）

### 0.6 为什么 6 个项目的 MCP 实现差异这么大

虽然 MCP 协议是标准化的，但**如何集成 MCP** 取决于各项目的架构决策：

| 决策点 | 选项范围 |
|--------|---------|
| **角色** | 纯 Client / 纯 Server / 双向 |
| **传输优先级** | stdio 优先 / HTTP 优先 / 全覆盖 |
| **安全模型** | 无 / 信任 store / OAuth |
| **工具注册方式** | 静态 / 动态合并 / 插件化 |
| **生命周期管理** | 简单启动停止 / 状态机 / 插件生命周期 |
| **是否集成** | 深度集成 / 可选插件 / 拒绝 |

---

## 1. 横向对比总览表

### 1.1 基础信息

| 维度 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek Harness | pi |
|------|-------------|----------|----------|----------|-----------------|-----|
| **语言** | TypeScript/Bun | Rust | TypeScript | TypeScript/Bun + Effect | TypeScript + Cordis | TypeScript |
| **MCP 角色** | Client | Client | **Client + Server** | Client | Client（插件） | **无（拒绝）** |
| **代码规模** | ~3500 行核心客户端 | ~9 个模块文件 | ~15 个文件 | ~6 个文件 | ~4 个文件 | 0 行 |
| **实现层级** | 深度集成 | 独立 crate | 双向架构 | Service + Catalog | Cordis 插件 | N/A |

### 1.2 传输层对比

| 传输方式 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek Harness | pi |
|---------|-------------|----------|----------|----------|-----------------|-----|
| **stdio** | ✅ 主要 | ✅ 三锁设计 | ✅ Server 侧 | ✅ `connectLocal()` | ✅ 标准实现 | — |
| **SSE** | ✅ | ✅ HTTP(SSE) | — | ✅ 回退传输 | — | — |
| **Streamable HTTP** | ✅ `http` | — | — | ✅ 首选 | ✅ | — |
| **WebSocket** | ✅ `ws` | — | — | — | — | — |
| **SDK 进程内** | ✅ `sdk` | — | — | — | — | — |
| **IDE 专用** | ✅ `sse-ide`, `ws-ide` | — | — | — | — | — |
| **Claude.ai 代理** | ✅ `claudeai-proxy` | — | — | — | — | — |
| **传输总数** | **8 种** | **2 种** | **1 种（stdio server）** | **3 种** | **2 种** | **0** |

### 1.3 连接状态对比

| 状态 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek Harness |
|------|-------------|----------|----------|----------|-----------------|
| Connected | ✅ | ✅ `ServerStatus` | ✅ | ✅ `StatusConnected` | ✅ |
| Failed | ✅ | ✅ | ✅ | ✅ `StatusFailed` | ✅ |
| Needs Auth | ✅ `needs-auth` | —（OAuth 处理） | — | ✅ `StatusNeedsAuth` | — |
| Pending | ✅（含重连计数） | — | — | — | —（重连中） |
| Disabled | ✅ | — | — | ✅ `StatusDisabled` | — |
| Needs Client Registration | — | — | — | ✅ `StatusNeedsClientRegistration` | — |

### 1.4 安全与信任对比

| 安全特性 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek Harness | pi |
|---------|-------------|----------|----------|----------|-----------------|-----|
| **OAuth 2.0** | ✅ 完整（含 XAA） | ✅ OAuth 登录/刷新 | —（Gateway 认证） | ✅ 完整流程 | — | — |
| **项目级信任** | — | ✅ `mcp_trust.json` | — | — | — | — |
| **工具风险分级** | —（Hook 审批） | ✅ 三级 Safe/Risky | ✅ before-hook 审批 | — | — | — |
| **自动批准** | — | ✅ `autoApprove` | — | — | — | — |
| **凭证清洗** | — | — | — | — | ✅ `scrubbedParentEnv` | — |
| **结果验证** | — | — | ✅ 归一化 content | — | ✅ MIME+base64 严格验证 | — |

### 1.5 工具命名约定对比

| 项目 | 命名格式 | 示例 | 冲突处理 |
|------|---------|------|---------|
| Claude Code | `mcp__{server}__{tool}` | `mcp__github__list_issues` | 前缀命名空间 |
| AtomCode | `mcp__{server}__{tool}` | 同上 | 超长/非法字符用 SHA256 hash suffix |
| OpenClaw | 原始工具名（MCP 暴露时保留） | `read_file` | MCP server 内部管理 |
| OpenCode | `{serverName}_{toolName}`（sanitize） | `github_list_issues` | sanitize 函数 |
| DeepSeek | `mcp__{serverName}__{rawName}` | 同 Claude Code | 截断到 51 字符 + SHA256 后 12 位 |

---

## 2. Claude Code：最完整的 MCP 实现

### 2.1 传输层

Claude Code 是所有项目中 MCP 传输层支持最广的，实现了 **8 种传输方式**：

```typescript
// src/services/mcp/types.ts:23
export const TransportSchema = z.enum([
  'stdio',       // 标准 I/O（本地进程，最常用）
  'sse',         // Server-Sent Events（远程 HTTP 长连接）
  'sse-ide',     // IDE 扩展专用 SSE
  'http',        // Streamable HTTP（新标准）
  'ws',          // WebSocket
  'sdk',         // SDK 进程内
]);
// 额外：claudeai-proxy、ws-ide
```

**设计要点**：

- **stdio** 是最常用的传输方式，用于本地 MCP server 进程通信
- **sse-ide / ws-ide** 是 IDE 集成专用变体，与常规传输隔离
- **sdk** 是进程内传输（`InProcessTransport`），适用于嵌入场景
- **claudeai-proxy** 用于 Claude.ai 平台代理

### 2.2 连接状态机

Claude Code 使用 **5 种连接状态** 管理 MCP server 的生命周期：

```typescript
type MCPServerConnection =
  | ConnectedMCPServer    // { type: 'connected', client, capabilities, instructions, cleanup }
  | FailedMCPServer       // { type: 'failed', error }
  | NeedsAuthMCPServer    // { type: 'needs-auth' }
  | PendingMCPServer      // { type: 'pending', reconnectAttempt, maxReconnectAttempts }
  | DisabledMCPServer     // { type: 'disabled' }
```

**Pending 状态的独特设计**：Pending 状态携带 `reconnectAttempt` 和 `maxReconnectAttempts`，支持带预算的重连策略。这与其他项目的二元（成功/失败）状态机有本质区别。

### 2.3 配置来源（7 个 scope）

Claude Code 的配置合并体系是所有项目中最复杂的，支持 **7 个 scope**：

```typescript
export type ConfigScope =
  | 'local'       // .claude/settings.local.json
  | 'user'        // ~/.claude/settings.json
  | 'project'     // .claude/settings.json
  | 'dynamic'     // 运行时动态添加
  | 'enterprise'  // 企业管理策略
  | 'claudeai'    // Claude.ai 代理配置
  | 'managed'     // 托管策略
```

**合并策略**：managed → user → project → local，后覆盖前。enterprise 和 claudeai 作为特殊 scope 参与优先级排序。

### 2.4 工具调用完整链路

```
模型输出 tool_use: mcp__server__tool
  → ToolUseContext.options.tools 查找 MCPTool
  → MCPTool.call()
    → MCP 客户端 client.callTool({ name, arguments })
    → 传输层发送请求（stdio/sse/http/ws）
    → MCP 服务器处理
    → 返回结果
  → mapToolResultToToolResultBlockParam()
  → 注入 tool_result 到对话
```

**MCP 工具包装**：MCP 工具通过标准 `Tool` 接口包装，附加 `mcpInfo = { serverName, toolName }` 和 `isMcp = true` 标记。MCP prompts 作为 `loadedFrom: 'mcp'` 的 Skill 注册。

### 2.5 Elicitation 处理

Claude Code 独特地支持了 **MCP Elicitation** 协议：

```typescript
// elicitationHandler.ts - MCP 服务器的 elicit 请求处理
// MCP 服务器可以通过 elicit 协议请求用户输入
// 支持 URL elicitations（跳转浏览器）和表单 elicitations
```

这使得 MCP server 可以在工具执行过程中反向请求用户输入，是双向交互的雏形。

### 2.6 OAuth 认证

```typescript
// auth.ts - 完整 OAuth 流程
// 1. 发现 OAuth metadata（.well-known/oauth-authorization-server）
// 2. 动态注册客户端（client_registration）
// 3. 授权码流程（authorization_code）
// 4. Token 刷新（refresh_token）
// 5. XAA 跨应用访问（SEP-990）
```

Claude Code 的 OAuth 实现最为完整，包含 **XAA（Cross-Application Access）** 扩展，用于跨应用的 token 共享。

### 2.7 MCP Skill 构建器桥接

```typescript
// mcpSkillBuilders.ts - 解决循环依赖的关键设计
// loadSkillsDir.ts → mcpSkillBuilders.ts ← mcpSkills.ts

// 运行时通过 getMCPSkillBuilders() 获取
```

MCP 不仅提供工具，还提供 prompts（提示词模板），这些 prompts 被桥接为 Skill 系统的一部分。构建器模式解决了模块间的循环依赖。

### 2.8 核心评价

| 优势 | 不足 |
|------|------|
| 8 种传输覆盖所有场景 | 代码量大（3500+ 行客户端） |
| 7 scope 配置体系最完善 | 配置优先级复杂，学习成本高 |
| 完整 OAuth + XAA | 无项目级信任机制 |
| Elicitation 双向交互 | — |
| MCP Prompts → Skill 桥接 | — |

---

## 3. AtomCode：Rust 原生的 trust-first 设计

### 3.1 MCP Client Trait

AtomCode 作为唯一的 **Rust 实现**，通过 trait 抽象定义了 MCP 客户端接口：

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

**设计要点**：
- trait 方法完全协议无关，不暴露 JSON-RPC 细节
- `ServerStatus` 替代了 TypeScript 项目中的连接状态联合类型
- 与 laew 的 `Tool` trait 设计哲学一致

### 3.2 传输层实现

AtomCode 支持 **2 种传输**：stdio 和 HTTP(SSE)。

#### stdio 传输：三锁设计

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
}
```

**三锁分离关注点**：

| 锁 | 职责 | 保护的操作 |
|----|------|-----------|
| `request_lock` | 序列化请求/响应往返 | 防止并发 JSON-RPC 消息交错 |
| `operation_lock` | 保活 + 恢复决策 | 保活探测 + 故障恢复在同一个临界区 |
| `reconnect_lock` | teardown + respawn | 子进程销毁和重建的原子性 |

**connection_generation 单调递增**：等待恢复的操作通过比较 generation 检测是否已被其他 caller 修复，避免重复恢复。

### 3.3 MCP Tool 适配

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

**命名规则**：`mcp__{server}__{tool}`，超长或非法字符用 SHA256 hash suffix 保证唯一性。这与 DeepSeek Harness 的策略一致。

**描述前缀**：`[MCP:{server}] {description}` 为工具描述添加 MCP 来源标识，便于模型识别工具来源。

### 3.4 Trust 信任体系

AtomCode 拥有所有项目中**最完整的信任体系**：

#### 项目级信任

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

#### 三级信任判定

```rust
// McpToolAdapter::risk()
fn risk(&self, _args: &str) -> RiskLevel {
    if self.read_only                          // Level 1: server 声明只读
        || self.registry.is_server_trusted(&self.server)  // Level 2: config 里 trust: true
        || self.registry.is_tool_auto_approved(&self.full_name)  // Level 3: autoApprove 列表或"总是"
    {
        RiskLevel::Safe
    } else {
        RiskLevel::Risky
    }
}
```

#### 自动批准机制

| 批准来源 | 机制 | 作用域 |
|---------|------|--------|
| `trusted_servers` | config 里 `trust: true` | 整个 server 的所有工具 |
| `auto_approved_tools` | server 的 `autoApprove` 列表 | 指定工具 |
| 运行时"总是"授权 | 用户选择"始终允许" | 持久化到 trust store |
| `tool_aliases` | sanitized name → original identity 映射 | alias collision fail-closed |

### 3.5 MCP 模块在分层架构中的位置

AtomCode 采用 **L0/L1/L2 三层架构**，MCP 位于 L1（capabilities 层）：

- **L0 kernel** 不知道"编码"/"审查"/"MCP"——只有 Agent 循环 + Tool trait + LifecycleHooks
- **L1 capabilities** 提供中立工具 + MCP + compaction
- **L2 编码** 组合 L0+L1 实现编码流程

这种分层确保 MCP 是可选能力，不影响核心 Agent 循环。

### 3.6 核心评价

| 优势 | 不足 |
|------|------|
| Rust trait 抽象最优雅 | 传输种类少（2 种） |
| 三锁设计保证并发安全 | 无 OAuth（远程 MCP 缺失） |
| 三级信任判定最完善 | 无 SSE/WS 传输 |
| 项目级 trust store | — |
| L0/L1 分层解耦 | — |
| connection_generation 去重恢复 | — |

---

## 4. OpenClaw：双向 MCP Server 架构

### 4.1 架构概述

OpenClaw 是所有项目中**唯一实现双向 MCP** 的——它不仅作为 MCP Client 消费外部 MCP server，还作为 **MCP Server 暴露自身工具**给外部 Agent 使用。

这种双向架构的核心动机是：OpenClaw 作为 **Gateway + Harness** 架构的中间层，需要让外部 Agent（如 Claude Code、Codex）通过 MCP 协议接入 OpenClaw 管理的工具和能力。

### 4.2 MCP Server 标准实现

```typescript
// src/mcp/tools-stdio-server.ts
export function createToolsMcpServer(params: { name: string; tools: AnyAgentTool[] }): Server {
  const handlers = createPluginToolsMcpHandlers(params.tools);
  const server = new Server(
    { name: params.name, version: VERSION },
    { capabilities: { tools: {} } },
  );
  server.setRequestHandler(ListToolsRequestSchema, handlers.listTools);
  server.setRequestHandler(CallToolRequestSchema, async (request, extra) => {
    return await handlers.callTool(request.params, extra.signal);
  });
  return server;
}
```

**设计要点**：
- 标准 MCP `Server` 实例，暴露 `ListTools` 和 `CallTool` 两个 handler
- `connectToolsMcpServerToStdio()` 封装 stdio 传输层完整生命周期
- stdin close / SIGINT / SIGTERM 均触发优雅关闭

### 4.3 工具注册到 MCP 的完整链路

```typescript
// src/mcp/plugin-tools-handlers.ts
export function createPluginToolsMcpHandlers(tools: AnyAgentTool[]) {
  // 第一步：包装 before-tool-call hook（审批/审计）
  const wrappedTools = tools.map((tool) =>
    isToolWrappedWithBeforeToolCallHook(tool)
      ? rewrapToolWithBeforeToolCallHook(tool, undefined, { approvalMode: "report" })
      : wrapToolWithBeforeToolCallHook(tool, undefined, { approvalMode: "report" })
  );

  // 第二步：建立 name → {tool, runId} 映射
  const toolMap = new Map<string, { tool: AnyAgentTool; runId: string | undefined }>();
  for (const tool of wrappedTools) {
    toolMap.set(tool.name, { tool, runId: resolveBeforeToolCallRunId(tool) });
  }

  // 第三步：返回 MCP handler 对象
  return {
    listTools: async () => ({
      tools: wrappedTools.map((tool) => ({
        name: tool.name,
        description: tool.description ?? "",
        inputSchema: resolveJsonSchemaForTool(tool),
      })),
    }),
    callTool: async (params, signal) => { /* ... */ },
  };
}
```

**三层映射**：
1. 工具包装（before-hook 审批/审计）
2. 名称映射（name → tool + runId）
3. MCP handler 返回

### 4.4 Channel Bridge 双向通信

`OpenClawChannelBridge` 是 MCP 工具与 Gateway 的运行时桥接核心：

```typescript
// src/mcp/channel-bridge.ts
export class OpenClawChannelBridge {
  private gateway: GatewayClient | null = null;
  private readonly queue: QueueEvent[] = [];        // 事件队列
  private readonly pendingWaiters = new Set<PendingWaiter>(); // 等待新事件的 MCP 客户端
  private readonly pendingClaudePermissions = new Map<string, number>();
  private readonly pendingApprovals = new Map<string, PendingApprovalEntry>();
  private cursor = 0;          // 事件游标（polling 用）
  private closed = false;
  private ready = false;
  private started = false;
}
```

**事件队列 + Waiter 模式**：
- MCP 客户端通过 `pollEvents()` **拉取事件**
- 通过 `waitEvents()` **长轮询等待新事件**
- `pendingWaiters` 集合维护所有等待中的 waiter
- 新事件到达时立即唤醒所有 waiter

这种模式在 MCP 客户端和 Gateway 之间建立了**松耦合的异步通信通道**。

### 4.5 MCP Server 生命周期

```typescript
// src/mcp/channel-server.ts
export async function serveOpenClawChannelMcp(opts) {
  const { server, start, close } = await createChannelMcpRuntime(opts);
  const transport = new StdioServerTransport();

  const shutdown = () => {
    if (shuttingDown) return;
    shuttingDown = true;
    process.stdin.off("end", shutdown);
    process.off("SIGINT", shutdown);
    closePromise = Promise.resolve().then(close);
  };

  transport["onclose"] = shutdown;        // 传输层关闭 → 关闭 server
  process.stdin.once("end", shutdown);    // stdin 关闭 → 关闭 server
  process.once("SIGINT", shutdown);       // SIGINT → 关闭 server

  await server.connect(transport);        // 1. 连接传输层
  await start();                          // 2. 初始化 Gateway 连接
  await closed;                           // 3. 等待关闭信号
  await closePromise;                     // 4. 等待清理完成
}
```

**四阶段生命周期**：
1. **连接传输层** → 建立 stdio 通道
2. **初始化 Gateway** → 连接后端服务
3. **等待关闭信号** → 三种信号源（stdin end / SIGINT / transport close）
4. **清理完成** → 执行 close 回调

### 4.6 Agent Bundle Manager

OpenClaw 的 MCP 管理被拆分为约 **15 个文件**，按职责分层：

| 文件 | 职责 |
|------|------|
| `agent-bundle-mcp-manager.ts` | MCP manager 核心（install、connect、lifecycle） |
| `agent-bundle-mcp-harness.ts` | MCP harness 适配（将 MCP server 注册为 agent harness） |
| `agent-bundle-mcp-tools.ts` | 将 MCP 工具 materialize 为 `AnyAgentTool[]` |
| `agent-bundle-mcp-requester-connect.ts` | 请求者侧连接管理 |
| `agent-bundle-mcp-runtime.ts` | MCP runtime 配置与缓存 |
| `agent-bundle-mcp-manager-lifecycle.ts` | MCP server 启动/关闭/重启生命周期 |

### 4.7 核心评价

| 优势 | 不足 |
|------|------|
| **唯一实现双向 MCP** | 仅 stdio 传输（Server 侧） |
| Channel Bridge 异步通信成熟 | 无 OAuth 认证 |
| before-hook 审批集成 | 文件数多，架构复杂度高 |
| Agent Bundle 生命周期完整 | — |
| 工具归一化（text/image） | — |

---

## 5. OpenCode：Effect-DI 驱动的 Catalog 模式

### 5.1 传输层：自动回退策略

OpenCode 支持 **3 种传输**，且实现了 **自动回退机制**：

```typescript
// packages/opencode/src/mcp/index.ts:212
type Transport = StdioClientTransport | StreamableHTTPClientTransport | SSEClientTransport
```

| 传输方式 | 适用场景 | 代码位置 |
|---------|---------|---------|
| **StdioClientTransport** | 本地 MCP server（子进程通信） | `connectLocal()` |
| **StreamableHTTPClientTransport** | 远程 MCP server（HTTP 流式） | `connectRemote()` |
| **SSEClientTransport** | 远程 MCP server（SSE 回退） | `connectRemote()` |

**连接策略**：远程服务器先尝试 StreamableHTTP，失败后自动回退到 SSE：

```typescript
const transports = [
  { name: "StreamableHTTP", transport: new StreamableHTTPClientTransport(url, { authProvider }) },
  { name: "SSE", transport: new SSEClientTransport(url, { authProvider }) },
]

for (const { name, transport } of transports) {
  const result = yield* connectTransport(transport, connectTimeout)
  if (result) return { client: result.client, status: "connected" }
  // 如果是 auth 错误，停止尝试其他传输
  if (lastStatus?.status === "needs_auth") break
}
```

**关键决策**：auth 错误不触发回退——如果远程服务器需要认证，切换传输方式不会解决认证问题。

### 5.2 Catalog 概念

OpenCode 引入了 **McpCatalog** 作为工具/资源/提示的注册中心，这是其他项目没有的独特概念：

```typescript
// packages/opencode/src/mcp/catalog.ts

// 工具名标准化
export const toolName = (clientName: string, name: string) =>
  sanitize(clientName) + "_" + sanitize(name)

// 分页遍历（MCP 协议支持游标分页）
export async function paginate<T, R extends { nextCursor?: string }>(
  list: (cursor?: string) => Promise<R>,
  items: (result: R) => T[],
): Promise<T[]> {
  const result: T[] = []
  let cursor: string | undefined
  for (let page = 0; page < MAX_LIST_PAGES; page++) {
    const page = await list(cursor)
    result.push(...items(page))
    if (page.nextCursor === undefined) return result
    cursor = page.nextCursor
  }
}

// MCP 工具转 AI SDK 工具格式
export function convertTool(mcpTool: MCPToolDef, client: Client, timeout?: number): Tool {
  return dynamicTool({
    description: mcpTool.description ?? "",
    inputSchema: jsonSchema(inputSchema),
    execute: async (args, options) => {
      const result = await client.callTool(
        { name: mcpTool.name, arguments: args },
        CallToolResultSchema,
        { resetTimeoutOnProgress: true, signal: options.abortSignal, timeout },
      )
      return result
    },
  })
}
```

**Catalog 的三大职责**：
1. **名称标准化**：`serverName_toolName` 格式，sanitize 处理非法字符
2. **分页遍历**：MCP 协议支持游标分页，Catalog 封装了自动分页逻辑
3. **格式转换**：将 MCP 工具定义转换为 AI SDK 的 `Tool` 格式

### 5.3 MCP Service 状态管理

```typescript
interface State {
  config: Record<string, ConfigMCPV1.Info>   // 运行时配置
  status: Record<string, Status>              // 连接状态
  clients: Record<string, MCPClient>          // 客户端实例
  defs: Record<string, MCPToolDef[]>          // 工具定义缓存
  instructions: Record<string, string>        // 服务器指令缓存
}

// 6 种状态
export const Status = Schema.Union([
  StatusConnected,              // "connected"
  StatusDisabled,               // "disabled"
  StatusFailed,                 // "failed"
  StatusNeedsAuth,              // "needs_auth"
  StatusNeedsClientRegistration, // "needs_client_registration"
])
```

OpenCode 使用 **Effect DI 框架**管理状态，`InstanceState` 作为 Effect 的 Context Service 注入到各模块。

### 5.4 服务器生命周期

**启动**：`InstanceState.make()` 在初始化时 **并发连接所有配置的 MCP server**：

```typescript
yield* Effect.forEach(
  Object.entries(config),
  ([key, mcp]) => Effect.gen(function* () {
    const result = yield* create(key, mcp)
    s.status[key] = result.status
    if (result.mcpClient) {
      s.clients[key] = result.mcpClient
      s.defs[key] = result.defs!
      watch(s, key, result.mcpClient, bridge, mcp.timeout)
    }
  }),
  { concurrency: "unbounded" },  // 无限并发
)
```

**重连/状态监听**：

```typescript
function watch(s: State, name: string, client: MCPClient, bridge, timeout?) {
  client.onclose = () => {
    s.status[name] = { status: "failed", error: "Connection closed" }
    bridge.fork(events.publish(ToolsChanged, { server: name }))
  }

  // 监听工具列表动态变更
  client.setNotificationHandler(ToolListChangedNotificationSchema, async () => {
    const listed = await McpCatalog.defs(client, timeout)
    s.defs[name] = listed
    await bridge.promise(events.publish(ToolsChanged, { server: name }))
  })
}
```

**关闭**：`Effect.addFinalizer` 注册清理函数，杀死子进程及所有后代进程：

```typescript
yield* Effect.addFinalizer(() =>
  Effect.gen(function* () {
    for (const client of clients) {
      const pid = client.transport instanceof StdioClientTransport ? client.transport.pid : null
      if (typeof pid === "number") {
        const pids = yield* descendants(pid)  // pgrep -P 递归查找
        for (const dpid of pids) process.kill(dpid, "SIGTERM")
      }
      yield* Effect.tryPromise(() => client.close())
    }
  })
)
```

### 5.5 MCP 工具集成到 Agent

MCP 工具通过 `ToolRegistry` 和 `SessionTools.resolve()` **两层动态合并**：

```typescript
// session/tools.ts 中 resolve()
for (const item of yield* registry.tools({ modelID, providerID, agent })) {
  tools[item.id] = tool({
    description: item.description,
    inputSchema: jsonSchema(schema),
    execute(args, options) { return run.promise(Effect.gen(...)) },
  })
}

// 另外还注册了 MCP Resource 工具
tools["list_mcp_resources"] = tool({ ... })
tools["list_mcp_resource_templates"] = tool({ ... })
tools["read_mcp_resource"] = tool({ ... })
```

**关键设计**：MCP 工具不进入 `builtin` 列表，而是在 `SessionTools.resolve()` 时动态合并。这保证了内置工具的稳定性和 MCP 工具的动态性。

**MCP Resource 一等公民**：OpenCode 是唯一将 MCP Resource（资源）注册为独立工具的项目——`list_mcp_resources`、`list_mcp_resource_templates`、`read_mcp_resource` 三个工具让 LLM 可以主动发现和读取 MCP 资源。

### 5.6 MCP Server 指令注入

MCP server 可以提供 `instructions` 字段，这些指令会被注入到系统提示词中：

```typescript
// mcp/index.ts
const instructions = Effect.fn("MCP.instructions")(function* () {
  const s = yield* InstanceState.get(state)
  return Object.entries(s.instructions)
    .filter(([name]) => s.status[name]?.status === "connected")
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, item]) => ({
      name,
      instructions: item,
      tools: (s.defs[name] ?? []).map((tool) => McpCatalog.toolName(name, tool.name)),
    }))
})
```

这些指令在 `SystemPrompt.build()` 时被拼接进系统提示词，让 LLM 了解每个 MCP server 提供的工具及其使用规则。

### 5.7 OAuth 认证

OpenCode 实现了完整的 **OAuth 2.0 流程**：

1. `startAuth()` → 发起 OAuth 授权请求，捕获 authorization URL
2. `authenticate()` → 打开浏览器让用户授权，等待回调
3. `finishAuth()` → 用 authorization code 获取 token
4. Token 存储在 `McpAuth` Service 中，支持过期检测

### 5.8 核心评价

| 优势 | 不足 |
|------|------|
| 三种传输自动回退 | 依赖 Effect DI 框架（重） |
| Catalog 分页遍历 | 无项目级信任机制 |
| MCP Resource 一等公民 | — |
| Effect finalizer 清理完整 | — |
| 指令注入系统提示词 | — |
| 完整 OAuth | — |

---

## 6. DeepSeek Harness：插件化 MCP Client

### 6.1 Cordis 插件架构

DeepSeek Harness 的 MCP 实现基于 **Cordis Everything-is-a-Plugin** 哲学：

```typescript
// packages/mcp/mcp-client/src/index.ts:29-33
export const name = 'mcp-client'
export const inject = ['tools']
```

每个 MCP server 对应一个**独立的插件实例**。MCP 不是框架内建能力，而是通过插件系统按需加载。

### 6.2 传输层

支持 **2 种传输**：

```typescript
// packages/mcp/mcp-client/src/transport.ts:31-50
export function createTransport(config: Config): Transport {
  switch (config.transport) {
    case 'stdio':
      return new StdioClientTransport({
        command: config.command,
        args: config.args,
        env: buildChildEnv(config.env),  // 凭证清洗 + 合并自定义环境变量
        cwd: config.cwd,
      })
    case 'streamable-http':
      return new StreamableHTTPClientTransport(
        new URL(config.url),
        { requestInit: { headers: config.headers } },
      )
  }
}
```

**凭证安全**：`scrubbedParentEnv()` 从父进程环境变量中剔除凭证类变量，只传递安全的环境变量给子进程。这是所有项目中**唯一实现环境变量凭证清洗**的。

### 6.3 连接管理器 —— 指数退避重连

DeepSeek Harness 的重连策略是所有项目中**设计最完善的**：

```typescript
// packages/mcp/mcp-client/src/connection.ts
export const RECONNECT_DEFAULTS = {
  enabled: true,
  initialDelayMs: 500,      // 初始延迟
  maxDelayMs: 30_000,       // 最大延迟
  maxAttempts: 10,          // 最大连续失败次数
}
```

**退避算法**：
```
500ms → 1s → 2s → 4s → 8s → 16s → 30s（封顶）
公式：min(maxDelayMs, initialDelayMs * 2^(failedAttempts - 1))
```

**稳定性窗口重置**：
```typescript
if (connectedAt !== undefined && Date.now() - connectedAt >= policy.maxDelayMs)
  failedAttempts = 0
```

连接持续超过 `maxDelayMs`（30s）后重置失败计数，下次断连开始新的预算。这避免了长期稳定运行后的偶发断连被累积为"放弃"。

**放弃逻辑**：`failedAttempts > maxAttempts` 后注销所有工具，停止重连。

### 6.4 工具注册流程

DeepSeek Harness 实现了**两阶段同步**（Fetch + Swap）：

```typescript
// packages/mcp/mcp-client/src/tools.ts:143-193
export async function syncTools(client, ctx, opts, previous): Promise<ToolDisposers> {
  // Phase 1: Fetch — 拉取完整工具列表，不影响注册表
  const definitions = new Map<string, ToolDefinition>()
  let cursor: string | undefined
  do {
    const response = await listToolsUncached(client, cursor)
    for (const tool of response.tools) {
      const publicName = publicToolName(opts.serverName, tool.name)
      definitions.set(publicName, createDefinition(client, ctx, publicName, tool.name, ...))
    }
    cursor = response.nextCursor
  } while (cursor)

  // Phase 2: Swap — 先注销旧工具，再注册新工具
  for (const dispose of previous.values()) dispose()
  const disposers: ToolDisposers = new Map()
  for (const [publicName, definition] of definitions) {
    disposers.set(publicName, ctx.tools.register(definition))
  }
  return disposers
}
```

**两阶段保证原子性**：先完整拉取新工具列表（Phase 1），再一次性注销旧工具并注册新工具（Phase 2）。模型不会看到不完整的工具集。

**命名规则**：
```typescript
export function publicToolName(serverName: string, rawName: string): string {
  const joined = `mcp__${serverName}__${rawName}`
  const normalized = joined.replace(INVALID_NAME_CHARS, '_')
  if (normalized === joined && normalized.length <= 64) return normalized
  const hash = createHash('sha256').update(`${serverName}\0${rawName}`).digest('hex').slice(0, 12)
  return `${normalized.slice(0, 51)}_${hash}`  // 截断 + 哈希避免冲突
}
```

### 6.5 图片投影

DeepSeek Harness 实现了 **MCP 结果中的图片处理**——这是其他项目未见的精细处理：

```typescript
// packages/mcp/mcp-client/src/tools.ts:433-487
async function prepareImageProjection(ctx, exec, content, toolName): Promise<ContentBlock[]> {
  // 1. 解码所有 image 块（验证 MIME type 和 base64 合法性）
  for (const [index, value] of content.entries()) {
    decoded.push(decodeImage(value as McpContentBlock))
    // 严格验证 PNG/JPEG/WebP/GIF + 规范 base64
  }

  // 2. 检查当前模型是否支持图片输入
  attachments = await resolveImageAdmission(ctx, exec)
  // 验证 model.inputModalities 包含 'image'

  // 3. 持久化存储图片
  const refs = await attachments.saveImages(decoded)

  // 4. 替换 base64 数据为 attachment ref
  return projectContent(content, toolName, (_block, index) => ({
    type: 'image', attachment: byIndex.get(index),
  }))
}
```

**安全边界**：MCP server 被视为不可信的外部进程，所有返回数据都经过严格验证：
- MIME type 必须是 `image/png | image/jpeg | image/webp | image/gif`
- base64 必须是规范形式（RFC 4648），不接受 URL-safe 变体
- 解码后重新编码验证一致性

### 6.6 不支持内容的降级处理

```typescript
// tools.ts:509-558
function projectContent(mcpContent, toolName, image): ContentBlock[] {
  for (const block of mcpContent) {
    switch (block.type) {
      case 'text':     // 正常处理
      case 'image':    // 图片投影
      case 'resource_link':  // 变为文本描述
      case 'audio':    // 变为诊断文本
      case 'resource': // 变为诊断文本
      default:         // 变为 "unsupported MCP content type" 文本
    }
  }
}
```

**所有不支持的 MCP 内容类型都降级为文本描述**，而不是静默丢弃。这保证模型总能看到结果。

### 6.7 生命周期管理

- **启动**：`apply()` 中 `await connection.ready`，`failOnStartupError` 控制首次失败是否拒绝插件
- **HMR**：effect-scoped 注册，HMR 时旧实例 dispose（断开连接 + 注销工具），新实例重建
- **停止**：`dispose()` 先停止重连定时器，关闭当前 client，等待 in-flight 操作，注销所有工具

### 6.8 核心评价

| 优势 | 不足 |
|------|------|
| 指数退避重连最完善 | 无 OAuth 认证 |
| 两阶段同步保证原子性 | 无 SSE 传输 |
| 凭证清洗（唯一） | 无项目级信任 |
| 图片投影 + 严格验证 | — |
| 降级策略不丢弃 | — |
| Cordis 插件按需加载 | — |
| 64 字符限制 + SHA256 截断 | — |

---

## 7. pi：有意识地拒绝 MCP

### 7.1 核心哲学

pi 是所有项目中**唯一明确拒绝 MCP** 的。其设计信条：

> "No MCP. Build CLI tools with READMEs."

这句话不是技术能力的限制，而是**深思熟虑的设计决策**。

### 7.2 MCP vs Skill 范式对比

pi 用 **Skill（纯文本指令注入）** 替代了 MCP 的工具协议：

| 特性 | MCP Tool | pi Skill |
|------|----------|----------|
| **定义格式** | JSON Schema | Markdown + YAML frontmatter |
| **发现方式** | 服务端注册 | 文件系统扫描 |
| **调用方式** | JSON-RPC tool call | 模型自行读取遵循 |
| **参数传递** | JSON arguments | 无（纯文本） |
| **运行时执行** | MCP Server 进程 | 无（模型用已有工具） |
| **适用场景** | 复杂 API 集成 | 知识/流程/checklist |
| **协议开销** | JSON-RPC + tool schema | 无 |
| **安全风险** | 外部进程信任 | 无（纯文本注入） |

### 7.3 为什么拒绝 MCP

pi 的拒绝基于三层论证：

**第一层：协议开销**
MCP 本质是**工具协议**（JSON-RPC + tool schema + runtime execution），需要：
- 启动子进程或建立网络连接
- 定义 JSON Schema 参数
- 处理连接管理/重连/超时
- 安全审查外部进程

**第二层：Skill 覆盖了大部分场景**
pi 的 Skill 范式特别适合**流程性知识**（如"添加新 provider 的7步 checklist"）。这类任务不需要新的 API 端点，只需要告诉模型怎么做。MCP 在这种场景下是**过度工程化**。

**第三层：已有工具足够**
pi 认为 bash/read/edit/write 四个基础工具已经覆盖了编码 Agent 的核心能力。Skill 通过文本指令告诉模型**如何组合这些工具**完成特定任务，而不需要引入新的执行层。

### 7.4 Skill 的实现细节

```typescript
// Skill 的系统提示词注入格式
const skillMessage = `<skill name="${skill.name}" source="${skill.filePath}">
References are relative to dirname(skill.filePath).
${skill.content}
</skill>`;
```

关键行为：
1. 扫描 `SKILL.md` 文件，有则加载并立即返回（不再扫描子目录中的 SKILL.md）
2. Ignore 规则从 `.gitignore` / `.ignore` / `.fdignore` 加载
3. 根目录的普通 `.md` 文件也被识别为 skill
4. Skill 注入用户消息流，触发一轮新的 agent run

### 7.5 对 laew 的启示

pi 的选择并不意味着 laew 也应该拒绝 MCP。但 pi 提供了一个重要视角：

**不是所有场景都需要 MCP**。laew 在引入 MCP 之前，应该评估：
1. 哪些场景确实需要外部工具协议（如 GitHub API、数据库查询）
2. 哪些场景可以用文本指令（Skill/README）覆盖（如流程性知识、checklist）
3. 两者结合的最优策略

pi 的 Skill 模式可以作为 laew 的**轻量补充**，与 MCP 并行存在。

### 7.6 pi 的选择对 MCP 生态的意义

pi 的拒绝不是对 MCP 协议本身的否定，而是对**工具协议万能论**的反思。pi 提供了一个重要的反面论证：

**场景 1：流程性知识**
"添加新 provider 的 7 步 checklist"——这不需要 JSON-RPC 调用，只需要文本指令让模型按步骤使用已有工具。

**场景 2：组合已有工具**
"如何用 bash 和 read 工具分析代码结构"——Skill 告诉模型组合方式，不需要新的 API 端点。

**场景 3：快速迭代**
修改一个 Markdown 文件比修改 MCP server 的 JSON Schema 快得多。Skill 的迭代周期是分钟级，MCP 是小时级。

**但 pi 的选择有明确的边界**：
- 对于**复杂 API 集成**（GitHub REST API、Jira、Slack），Skill 无法替代 MCP——模型无法自行构造 HTTP 请求
- 对于**需要认证的远程服务**，Skill 无法处理 OAuth token 刷新
- 对于**实时数据流**，Skill 无法建立持久连接

**结论**：pi 的 Skill 和 MCP 不是互斥的，而是**互补的**。laew 可以同时支持两者——MCP 用于复杂 API 集成，Skill（或类似的文本指令注入）用于流程性知识。

### 7.7 laew 的 Skill 可行性评估

laew 是否需要类似 pi 的 Skill 系统？评估如下：

| 维度 | 评估 | 说明 |
|------|------|------|
| **需求频率** | 中 | 部分场景（如 provider 管理流程）确实只需要文本指令 |
| **实现成本** | 低 | 扫描 Markdown 文件 + 注入系统提示词，约 2 天工作量 |
| **与现有架构兼容** | 高 | laew 的 `system_prompt` 组装机制已支持动态注入 |
| **与 MCP 冲突** | 无 | 两者作用域不同，可并行 |
| **优先级** | P3 | 在 MCP 稳定后再考虑 |

**建议**：laew 的 `project_context.rs`（项目说明文件五级链发现）已经部分实现了 Skill 的思想——将 CLAUDE.md / AGENTS.md 的内容注入系统提示词。可以在此基础上扩展为通用的 Skill 加载机制。

---

## 8. 六大项目横评：关键设计决策对比

### 8.1 传输层策略

| 策略 | 代表项目 | 评价 |
|------|---------|------|
| **全覆盖**（8 种） | Claude Code | 企业级，但复杂度高 |
| **3 种 + 自动回退** | OpenCode | 平衡方案，推荐参考 |
| **2 种 + 三锁** | AtomCode | Rust 并发安全，但传输少 |
| **2 种 + 插件化** | DeepSeek Harness | 按需加载，架构干净 |
| **1 种（stdio server）** | OpenClaw | 专注 Server 侧 |
| **0** | pi | 有意识拒绝 |

**推荐策略**：从 **stdio 起步**，逐步支持 Streamable HTTP。stdio 是 MCP server 的默认传输方式，覆盖 90%+ 的使用场景。

### 8.2 重连策略

| 策略 | 代表项目 | 特点 |
|------|---------|------|
| **带预算的 Pending 状态** | Claude Code | reconnectAttempt + maxReconnectAttempts |
| **三锁 + generation** | AtomCode | 去重恢复，最精细 |
| **指数退避 + 稳定性窗口** | DeepSeek Harness | 500ms→30s，窗口重置 |
| **状态监听 + 事件发布** | OpenCode | onclose + ToolListChanged |
| **简单重启** | OpenClaw | 三种信号源触发 |

**推荐策略**：采用 DeepSeek Harness 的**指数退避 + 稳定性窗口重置**模式——简单、可预测、避免长期运行后的误判。

### 8.3 工具命名

所有项目（除 OpenClaw）都采用了 **双下划线分隔的命名空间** 模式：

```
mcp__{serverName}__{toolName}
```

OpenCode 使用单下划线（`serverName_toolName`），但 sanitize 了非法字符。

**推荐策略**：采用 `mcp__{server}__{tool}` 的标准格式，参考 DeepSeek Harness 的 64 字符限制 + SHA256 截断策略。

### 8.4 安全模型

| 模型 | 代表项目 | 适用场景 |
|------|---------|---------|
| **三级信任判定** | AtomCode | read_only → server_trusted → tool_auto_approved |
| **OAuth + XAA** | Claude Code | 远程 MCP server 认证 |
| **OAuth 2.0** | OpenCode | 远程 MCP server 认证 |
| **凭证清洗** | DeepSeek Harness | 子进程环境变量安全 |
| **before-hook 审批** | OpenClaw | 工具执行前审计 |
| **结果严格验证** | DeepSeek Harness | MCP 返回数据安全 |

**推荐策略**：laew 应构建**分层安全模型**：
1. 本地 MCP server（stdio）：参考 AtomCode 的项目级 trust store
2. 远程 MCP server（HTTP/SSE）：参考 OpenCode 的 OAuth 2.0
3. 所有 MCP 工具：参考 AtomCode 的三级风险判定

### 8.5 资源处理

| 能力 | 代表项目 | 说明 |
|------|---------|------|
| **MCP Resource 一等公民** | OpenCode | list/read resource 工具化 |
| **MCP Prompts → Skill** | Claude Code | prompt 桥接为 Skill |
| **图片投影** | DeepSeek Harness | base64 解码 + 持久化 + attachment ref |
| **指令注入** | OpenCode | server instructions → 系统提示词 |
| **降级处理** | DeepSeek Harness | 不支持类型 → 文本描述 |

**推荐策略**：优先实现**指令注入**（成本最低、价值最高），其次是 **MCP Resource**，最后是图片投影。

### 8.6 MCP 双向暴露

只有 OpenClaw 实现了 **MCP Server 侧**——将自身工具暴露为 MCP server 供外部 Agent 调用。

**对 laew 的意义**：如果 laew 未来要支持外部 Agent 接入（如让 Claude Code 调用 laew 的工具），OpenClaw 的 `createToolsMcpServer()` + `createPluginToolsMcpHandlers()` 是成熟参考。

---

## 9. 跨项目设计模式提炼

### 模式 1：命名空间前缀隔离

**观察**：5 个项目都采用了 `mcp__{server}__{tool}` 或类似的命名空间前缀。

**价值**：防止 MCP 工具与内置工具命名冲突，便于按来源过滤和统计。

**laew 应用**：在 `ToolRegistry` 中为 MCP 工具添加 `mcp__` 前缀，与内置工具（bash/read/write/edit/glob/grep）隔离。

### 模式 2：连接状态机

**观察**：Claude Code（5 种）、OpenCode（5 种）、DeepSeek Harness（3 种隐式）都实现了连接状态机。

**价值**：统一管理连接的生命周期，避免状态不一致。

**laew 应用**：定义 `McpServerState` 枚举，至少包含 `Pending / Connected / Failed / NeedsAuth / Disabled` 五种状态。

### 模式 3：指数退避重连

**观察**：DeepSeek Harness（500ms→30s）、AtomCode（三锁 + generation）、Claude Code（Pending 带预算）都实现了重连机制。

**价值**：MCP server 可能崩溃或暂时不可用，自动重连保证可用性。

**laew 应用**：采用 `initialDelayMs=500, maxDelayMs=30000, maxAttempts=10` 的默认参数。

### 模式 4：工具注册的两阶段同步

**观察**：DeepSeek Harness 的 Fetch + Swap 模式。

**价值**：保证工具列表更新的原子性，模型不会看到不完整的工具集。

**laew 应用**：在 `ToolRegistry::sync_mcp_tools()` 中先拉取完整列表，再一次性替换。

### 模式 5：结果归一化

**观察**：OpenClaw（text/image 归一化）、DeepSeek Harness（降级为文本）都对 MCP 返回结果做了归一化处理。

**价值**：MCP server 返回的内容类型多样，归一化保证 Agent 循环的一致性。

**laew 应用**：定义 `McpContent` 枚举（Text / Image / Unsupported），所有类型最终转换为 `ToolResult`。

### 模式 6：配置多 scope 合并

**观察**：Claude Code（7 scope）、AtomCode（config + trust store）都支持多来源配置。

**价值**：不同层级（全局/项目/本地）的 MCP 配置需要合并。

**laew 应用**：初期支持 3 个 scope：`user`（~/.laew/mcp.json）、`project`（.laew/mcp.json）、`dynamic`（运行时）。存储到 SQLite 而非 JSON 文件，与现有 `providers` 表设计一致。

### 模式 7：凭证清洗

**观察**：DeepSeek Harness 的 `scrubbedParentEnv()`。

**价值**：启动 MCP server 子进程时，不应传递父进程的所有环境变量。

**laew 应用**：白名单模式，只传递 `PATH / HOME / USER / LANG / LC_ALL / TMPDIR` 等安全变量，其余由 `McpServerConfig.env` 显式指定。

### 模式 8：Plugin-as-MCP

**观察**：DeepSeek Harness 的 Cordis 插件化。

**价值**：MCP 作为可选能力，通过插件系统按需加载，不影响核心 Agent 循环。

**laew 应用**：MCP 应在 `L1 capabilities` 层（类似 AtomCode 的分层），与核心 Agent 循环（`agent/mod.rs`）解耦。

### 模式 9：Server 指令注入系统提示词

**观察**：OpenCode 的 `MCP.instructions()`。

**价值**：MCP server 可以通过 `instructions` 字段向 LLM 传递使用指南。

**laew 应用**：在 `SystemPrompt` 组装时，拼接已连接 MCP server 的 instructions。

### 模式 10：双向 MCP

**观察**：OpenClaw 的 `createToolsMcpServer()`。

**价值**：将 Agent 工具暴露为 MCP server，让外部 Agent 通过标准协议调用。

**laew 应用**：长期规划（P2），laew 的 `Tool` trait 可增加 `to_mcp_schema()` 方法，实现 Tool ↔ MCP 双向映射。

---

## 10. 对 laew 的综合建议

### 10.1 当前状态

laew 当前：
- **无 MCP 支持**，只有内置工具（Bash / Read / Write / Edit / Glob / Grep）
- **双协议支持**：Anthropic + OpenAI
- **多 Agent 架构**：6 角色（Yolo / Plan / Main-Work / SubAgent-Work / Quality-Check / SessionContext）
- **配置存储**：SQLite，无 JSON 配置文件
- **语言**：Rust，与 AtomCode 最相似

### 10.2 分阶段接入路线图

#### P0：MCP Client Trait 设计（1 周）

参考 AtomCode 的 `McpClient` trait，定义 laew 的 MCP 客户端抽象：

```rust
// src/mcp/mod.rs
pub mod client;
pub mod transport_stdio;
pub mod config;
pub mod tool_adapter;

#[async_trait]
pub trait McpClient: Send + Sync {
    async fn initialize(&mut self) -> Result<InitializeResult>;
    async fn list_tools(&self) -> Result<Vec<McpToolInfo>>;
    async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<McpCallResult>;
    fn server_name(&self) -> &str;
    fn status(&self) -> McpServerStatus;
}
```

**决策**：MCP 模块放在 `src/mcp/` 下（而非 `src/agent/tools/`），保持与 Agent 核心循环的解耦。

#### P1：stdio 传输实现（2 周）

```rust
// src/mcp/transport_stdio.rs
pub struct StdioTransport {
    child: tokio::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    request_id: AtomicI64,
}
```

**关键实现要点**：
1. JSON-RPC 2.0 over stdio（参考 AtomCode 的三锁设计简化版：1 把锁序列化请求/响应）
2. 子进程 `kill_on_drop(true)` 防止泄漏
3. 超时控制（默认 30s）

#### P1：MCP Tool 适配（1 周）

```rust
// src/mcp/tool_adapter.rs
pub struct McpToolAdapter {
    client: Arc<dyn McpClient>,
    server_name: String,
    tool_name: String,
    full_name: String,      // "mcp__{server}__{tool}"
    description: String,
    schema: serde_json::Value,
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str { &self.full_name }
    fn description(&self) -> &str { &self.description }
    fn parameters_schema(&self) -> serde_json::Value { self.schema.clone() }
    async fn execute(&self, args: serde_json::Value) -> Result<String> {
        let result = self.client.call_tool(&self.tool_name, args).await?;
        Ok(result.to_text())
    }
}
```

MCP 工具通过 `Tool` trait 适配，与内置工具统一注册到 `ToolRegistry`。

#### P1：配置与注册（1 周）

```rust
// src/mcp/config.rs
pub struct McpServerConfig {
    pub transport: McpTransportType,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
    pub timeout_ms: u64,
    pub auto_approve: Vec<String>,
    pub trust: bool,
}
```

**存储方案**：laew 使用 SQLite，MCP 配置建议存入新的 `mcp_servers` 表：

```sql
CREATE TABLE mcp_servers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    transport TEXT NOT NULL DEFAULT 'stdio',
    command TEXT,
    args TEXT,  -- JSON array
    env TEXT,   -- JSON object
    url TEXT,
    timeout_ms INTEGER DEFAULT 30000,
    auto_approve TEXT,  -- JSON array
    trust BOOLEAN DEFAULT FALSE,
    is_active BOOLEAN DEFAULT TRUE,
    scope TEXT DEFAULT 'user',  -- user / project / dynamic
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);
```

#### P1：指数退避重连（1 周）

参考 DeepSeek Harness 的实现：

```rust
// src/mcp/reconnect.rs
pub struct ReconnectPolicy {
    pub initial_delay_ms: u64,   // 500
    pub max_delay_ms: u64,       // 30000
    pub max_attempts: u32,       // 10
}

impl ReconnectPolicy {
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay = self.initial_delay_ms * 2u64.pow(attempt.saturating_sub(1));
        Duration::from_millis(delay.min(self.max_delay_ms))
    }
}
```

#### P2：Streamable HTTP 传输（2 周）

在 stdio 稳定后，增加 HTTP 传输支持远程 MCP server。

#### P2：OAuth 2.0（2 周）

参考 OpenCode 的完整 OAuth 流程实现远程 MCP server 认证。

#### P2：MCP Resource 支持（1 周）

参考 OpenCode 的 `list_mcp_resources` / `read_mcp_resource` 工具。

#### P2：项目级 Trust Store（1 周）

参考 AtomCode 的 `mcp_trust.json`，laew 版本存储到 SQLite `mcp_trust` 表。

#### P3：双向 MCP（长期）

参考 OpenClaw 的 `createToolsMcpServer()`，laew 的 `Tool` trait 增加 `to_mcp_schema()` 方法。

### 10.3 与多 Agent 架构的集成

MCP 与 laew 的 6 角色多 Agent 架构集成点：

| Agent 角色 | MCP 集成方式 | 说明 |
|-----------|-------------|------|
| **Yolo Agent** | 只读 MCP 工具过滤 | `readOnlyHint: true` 的 MCP 工具可被 Yolo 使用 |
| **SubAgent-Work** | MCP 工具全量注册 | MCP 工具通过 `ToolRegistry` 与内置工具统一管理 |
| **Quality-Check** | MCP 工具执行后审计 | 参考 OpenClaw 的 before-hook 模式 |
| **SessionContext** | MCP 工具使用记录 | 记录 MCP 工具调用摘要到 `session_memory` |

**关键决策**：MCP 工具应由 **SubAgent-Work Agent** 持有，而非 Yolo Agent。Yolo 只做任务分类，不应接触外部 MCP server。

### 10.4 CLI 集成

参考现有 `provider` 子命令，建议增加 `mcp` 子命令：

```bash
./laew mcp list              # 列出已配置的 MCP server
./laew mcp add               # 交互式添加 MCP server
./laew mcp remove <name>     # 移除 MCP server
./laew mcp tools <name>      # 列出指定 server 的工具
./laew mcp trust <name>      # 信任 MCP server
```

TUI 子屏可以复用 `ProviderList` / `ProviderForm` 的 Tab 表单模式。

### 10.5 优先级矩阵

| 优先级 | 功能 | 估时 | 参考项目 | 价值 |
|-------|------|------|---------|------|
| **P0** | McpClient trait 设计 | 1 周 | AtomCode | 架构基础 |
| **P1** | stdio 传输 | 2 周 | AtomCode | 覆盖 90% 场景 |
| **P1** | MCP Tool 适配 | 1 周 | 全部 | 统一工具注册 |
| **P1** | 配置存储（SQLite） | 1 周 | 自研 | 与现有架构一致 |
| **P1** | 指数退避重连 | 1 周 | DeepSeek Harness | 生产级可用 |
| **P2** | Streamable HTTP | 2 周 | OpenCode | 远程 MCP |
| **P2** | OAuth 2.0 | 2 周 | OpenCode | 远程认证 |
| **P2** | MCP Resource | 1 周 | OpenCode | 资源一等公民 |
| **P2** | 项目级 Trust Store | 1 周 | AtomCode | 安全模型 |
| **P2** | 指令注入系统提示词 | 3 天 | OpenCode | 低投入高回报 |
| **P3** | 双向 MCP Server | 3 周 | OpenClaw | 外部 Agent 接入 |
| **P3** | Skill 系统 | 2 周 | pi | 轻量知识注入 |

### 10.6 风险与反模式

| 风险 | 说明 | 缓解措施 |
|------|------|---------|
| **过早引入 OAuth** | laew 目前面向本地开发者，远程 MCP 需求低 | P2 阶段再考虑 |
| **配置复杂度** | Claude Code 的 7 scope 体系过于复杂 | 初期 3 scope（user/project/dynamic） |
| **MCP 工具覆盖内置工具** | MCP server 可能注册同名工具 | `mcp__` 前缀强制隔离 |
| **子进程泄漏** | MCP server 子进程未正确退出 | `kill_on_drop(true)` + finalizer |
| **大结果撑爆上下文** | MCP 工具返回超大结果 | 参考 `maxResultSizeChars` + 磁盘持久化 |
| **信任模型缺失** | 未信任的 MCP server 可能返回恶意数据 | 参考 AtomCode 的三级信任判定 |

### 10.7 laew MCP 模块目录结构草案

```
src/mcp/
├── mod.rs                 // 模块入口 + McpClient trait
├── transport_stdio.rs     // stdio JSON-RPC 传输
├── transport_http.rs      // Streamable HTTP 传输（P2）
├── config.rs              // McpServerConfig + SQLite CRUD
├── registry.rs            // McpRegistry — 多 server 管理
├── tool_adapter.rs        // McpToolAdapter — MCP tool → Tool trait 适配
├── reconnect.rs           // 指数退避重连策略
├── trust.rs               // 项目级信任 store（P2）
├── result.rs              // MCP 结果归一化（text/image → ToolResult）
└── oauth.rs               // OAuth 2.0 认证（P2）
```

**与现有架构的集成点**：

| laew 现有模块 | MCP 集成方式 | 说明 |
|--------------|-------------|------|
| `agent/tools/mod.rs` | `McpToolAdapter: Tool` | MCP 工具通过 Tool trait 注册 |
| `agent/tools/bash.rs` | 无直接关联 | Bash 工具不受 MCP 影响 |
| `agent/mod.rs` | `run_session()` 中动态合并工具 | MCP 工具在 session 开始时同步 |
| `agent/profile.rs` | `work_profile()` 扩展 | Work Agent 的工具集包含 MCP 工具 |
| `agent/system_prompt/mod.rs` | 拼接 MCP instructions | MCP server 指令注入系统提示词 |
| `config/mod.rs` | `mcp_servers` 表 CRUD | MCP 配置持久化到 SQLite |
| `tui/mod.rs` | `/mcp` 子命令路由 | TUI 子屏管理 MCP server |
| `session.rs` | Session 级 MCP 工具缓存 | 每个 Session 独立的 MCP 工具实例 |

### 10.8 laew MCP 初始化时序图

```
laew 启动
  │
  ├─ 1. 加载 mcp_servers 表（SQLite）
  │     SELECT * FROM mcp_servers WHERE is_active = 1
  │
  ├─ 2. 为每个 server 创建 StdioTransport
  │     StdioTransport::new(command, args, env)
  │     → tokio::process::Command::new(command).spawn()
  │
  ├─ 3. 发送 initialize 请求
  │     → { "jsonrpc": "2.0", "method": "initialize", "params": {...} }
  │     ← { "result": { "capabilities": { "tools": {} } } }
  │
  ├─ 4. 拉取工具列表（带分页）
  │     → { "method": "tools/list" }
  │     ← { "result": { "tools": [...], "nextCursor": "..." } }
  │
  ├─ 5. 创建 McpToolAdapter 列表
  │     for tool in tools:
  │       adapter = McpToolAdapter::new(client, server_name, tool)
  │       adapters.push(adapter)
  │
  ├─ 6. 注册到 ToolRegistry
  │     registry.register_mcp_tools(adapters)
  │
  └─ 7. 注入系统提示词
        system_prompt.push_mcp_instructions(server_instructions)
```

### 10.9 laew MCP 工具调用时序图

```
用户输入: "查看 GitHub 上的 issue 列表"
  │
  ├─ Yolo Agent: 任务分类 → medium
  │
  ├─ Main-Work Agent: 拆解 WorkFlow
  │     → [SubAgent: 调用 mcp__github__list_issues]
  │
  ├─ SubAgent-Work Agent: 执行
  │     │
  │     ├─ LLM 输出: tool_use { name: "mcp__github__list_issues", input: {repo: "..."} }
  │     │
  │     ├─ ToolRegistry::execute("mcp__github__list_issues", args)
  │     │     → McpToolAdapter::execute(args)
  │     │       → McpClient::call_tool("list_issues", args)
  │     │         → StdioTransport::send_request(jsonrpc_request)
  │     │         → StdioTransport::read_response()
  │     │         → McpCallResult
  │     │       → McpResult::to_text()
  │     │     → ToolResult::Success(text)
  │     │
  │     └─ tool_result 注入对话上下文
  │
  ├─ Quality-Check Agent: 检查结果
  │     → MCP 工具调用记录写入 agent_memory
  │
  └─ SessionContext Agent: 汇总摘要
        → "用户查询了 GitHub issue 列表"
```

---

## 11. 附录：关键代码片段索引

### A. Claude Code MCP 相关文件

| 文件 | 关键类型/函数 | 行数 |
|------|-------------|------|
| `src/services/mcp/client.ts` | MCP 客户端核心 | 3500+ |
| `src/services/mcp/types.ts` | `TransportSchema`, `MCPServerConnection` | 221+ |
| `src/services/mcp/config.ts` | 7 scope 配置加载 | — |
| `src/services/mcp/auth.ts` | OAuth + XAA | — |
| `src/services/mcp/InProcessTransport.ts` | 进程内传输 | — |
| `src/tools/MCPTool/` | MCP 工具包装 | — |
| `src/skills/mcpSkillBuilders.ts` | MCP Skill 构建器桥接 | 45 |

### B. AtomCode MCP 相关文件

| 文件 | 关键类型/函数 | 行数 |
|------|-------------|------|
| `crates/atomcode-capabilities/src/mcp/mod.rs` | `register_mcp_tools()` | — |
| `crates/atomcode-capabilities/src/mcp/client.rs` | `McpClient` trait | — |
| `crates/atomcode-capabilities/src/mcp/registry.rs` | `McpRegistry` | — |
| `crates/atomcode-capabilities/src/mcp/tool.rs` | `McpToolAdapter` | — |
| `crates/atomcode-capabilities/src/mcp/transport_stdio.rs` | `StdioClient`（三锁） | — |
| `crates/atomcode-capabilities/src/mcp/transport_http.rs` | `HttpClient` | — |
| `crates/atomcode-capabilities/src/mcp/trust.rs` | 项目级 trust store | — |
| `crates/atomcode-capabilities/src/mcp/config.rs` | `McpServerConfig` | — |
| `crates/atomcode-capabilities/src/mcp/oauth.rs` | OAuth 登录/刷新 | — |

### C. OpenClaw MCP 相关文件

| 文件 | 关键类型/函数 | 行数 |
|------|-------------|------|
| `src/mcp/channel-server.ts` | `serveOpenClawChannelMcp()` | — |
| `src/mcp/channel-bridge.ts` | `OpenClawChannelBridge` | — |
| `src/mcp/plugin-tools-handlers.ts` | `createPluginToolsMcpHandlers()` | — |
| `src/mcp/tools-stdio-server.ts` | `createToolsMcpServer()` | — |
| `src/agents/agent-bundle-mcp-manager.ts` | MCP manager 核心 | — |
| `src/agents/agent-bundle-mcp-manager-lifecycle.ts` | 生命周期管理 | — |

### D. OpenCode MCP 相关文件

| 文件 | 关键类型/函数 | 行数 |
|------|-------------|------|
| `packages/opencode/src/mcp/index.ts` | MCP Service 主逻辑 | — |
| `packages/opencode/src/mcp/catalog.ts` | `McpCatalog`（分页 + 转换） | — |
| `packages/opencode/src/mcp/auth.ts` | OAuth 认证 | — |
| `packages/opencode/src/mcp/oauth-provider.ts` | OAuth Provider | — |
| `packages/opencode/src/mcp/oauth-callback.ts` | OAuth 回调服务器 | — |
| `packages/opencode/src/session/tools.ts` | MCP 工具集成到 session | — |

### E. DeepSeek Harness MCP 相关文件

| 文件 | 关键类型/函数 | 行数 |
|------|-------------|------|
| `packages/mcp/mcp-client/src/index.ts` | 插件入口（配置验证 + 连接） | — |
| `packages/mcp/mcp-client/src/connection.ts` | `ConnectionSupervisor`（指数退避） | — |
| `packages/mcp/mcp-client/src/transport.ts` | 传输层工厂 | — |
| `packages/mcp/mcp-client/src/tools.ts` | Tool bridge（发现 + 注册 + 执行） | — |

### F. pi Skill vs MCP 对比

| 文件 | 关键内容 |
|------|---------|
| `packages/agent/src/harness/skills.ts` | Skill 加载与注入 |
| `packages/agent/src/harness/types.ts` | Skill 类型定义 |

### G. 关键数据指标汇总

| 指标 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek Harness | pi |
|------|-------------|----------|----------|----------|-----------------|-----|
| MCP 核心代码量 | ~3500 行 | ~9 个模块 | ~15 个文件 | ~6 个文件 | ~4 个文件 | 0 |
| 传输方式数量 | 8 | 2 | 1 | 3 | 2 | 0 |
| 连接状态数 | 5 | 2+ | 2 | 5 | 3 | 0 |
| 配置 scope 数 | 7 | 2 | 1 | 1 | 1 | 0 |
| 认证方式 | OAuth+XAA | OAuth | Gateway | OAuth | 无 | 无 |
| 信任层级 | 无 | 3 级 | 1 级 | 无 | 无 | 无 |
| 重连策略 | Pending 预算 | 三锁+generation | 信号触发 | 事件监听 | 指数退避 | N/A |
| 工具命名格式 | `mcp__s__t` | `mcp__s__t` | 原始名 | `s_t` | `mcp__s__t` | N/A |
| 最大工具名长度 | 无限制 | SHA256 fallback | 无限制 | sanitize | 64 字符 | N/A |
| Resource 支持 | 间接 | 无 | 无 | ✅ 一等公民 | 无 | 无 |
| Prompt 支持 | ✅ Skill 桥接 | 无 | 无 | 无 | 无 | ✅（Skill） |
| Server 侧能力 | 无 | 无 | ✅ | 无 | 无 | 无 |

### H. 术语表

| 术语 | 全称/含义 | 首次出现 |
|------|----------|---------|
| **MCP** | Model Context Protocol，Anthropic 提出的工具协议 | 协议层 |
| **stdio** | Standard I/O，通过子进程的 stdin/stdout 通信 | 传输层 |
| **SSE** | Server-Sent Events，HTTP 服务端推流 | 传输层 |
| **Streamable HTTP** | MCP 新标准传输，HTTP POST + 可选 SSE 流 | 传输层 |
| **JSON-RPC 2.0** | MCP 的消息格式标准 | 协议层 |
| **Tool** | MCP 中可调用的函数，有 JSON Schema 参数定义 | 资源层 |
| **Resource** | MCP 中只读数据源，由 URI 标识 | 资源层 |
| **Prompt** | MCP 中可复用的提示词模板 | 资源层 |
| **Catalog** | OpenCode 中的 MCP 工具/资源/提示注册中心 | 架构层 |
| **Trust Store** | 项目级 MCP server 信任配置存储 | 安全层 |
| **autoApprove** | 自动批准的 MCP 工具列表 | 安全层 |
| **Elicitation** | MCP server 反向请求用户输入的协议扩展 | 协议层 |
| **XAA** | Cross-Application Access，Claude Code 的跨应用 token 共享 | 认证层 |
| **PTC** | Projected Tool Content，MCP 结果的结构化输出模式 | 结果层 |
| **Skill** | pi 的纯文本指令注入，替代 MCP 的轻量方案 | 架构层 |
| **Channel Bridge** | OpenClaw 中 MCP 工具与 Gateway 的运行时桥接 | 架构层 |
| **ConnectionSupervisor** | DeepSeek Harness 中的连接管理器 | 架构层 |
| **McpToolAdapter** | AtomCode 中将 MCP tool 适配为 kernel Tool 的适配器 | 架构层 |
| **McpRegistry** | AtomCode 中多 server 管理 + trust + auto-approve 的注册中心 | 架构层 |
| **generation** | AtomCode 中单调递增的连接代号，用于去重恢复 | 架构层 |

---

> **总结**：6 个项目呈现了 MCP 集成的完整光谱——从 Claude Code 的企业级全覆盖（8 种传输、7 scope 配置、OAuth+XAA），到 AtomCode 的 Rust 原生安全设计（McpClient trait、三锁并发、三级信任判定），到 OpenClaw 的双向架构创新（Client + Server、Channel Bridge、Agent Bundle Manager），到 OpenCode 的 Effect-DI 驱动（Catalog 分页、MCP Resource 一等公民、指令注入），到 DeepSeek Harness 的插件化精巧（指数退避重连、两阶段同步、凭证清洗），到 pi 的有意识拒绝（Skill 替代、极简哲学）。
>
> laew 应以 **AtomCode**（同为 Rust、同为 trait 抽象）为基准参考，结合 **DeepSeek Harness** 的重连策略和 **OpenCode** 的 Catalog/Resource 设计，分阶段从 stdio 传输起步，逐步构建完整的 MCP 能力。
>
> **核心原则**：MCP 是可选能力层，不应侵入 Agent 循环核心。与 laew 的多 Agent 架构集成时，MCP 工具应由 SubAgent-Work Agent 持有，Yolo Agent 只做任务分类不接触外部 MCP server。
