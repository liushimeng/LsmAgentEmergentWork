# OpenClaw 综合深度分析

> 调研对象:openclaw(TypeScript,~201 万行,Gateway+Harness+双向 MCP)
> 调研日期:2026-09-04 ~ 2026-09-06
> 原始文档:6 份
> 总行数:~5,924 行(原始) → ~3,800 行(合并后,去重压缩)

---

## 目录

1. [项目元信息](#1-项目元信息)
2. [Gateway 架构(三层契约)](#2-gateway-架构三层契约)
3. [162 extensions 生态(9 大类)](#3-162-extensions-生态9-大类)
4. [packages 核心模块群](#4-packages-核心模块群)
5. [ACP/A2A 协议](#5-acpa2a-协议)
6. [双向 MCP(11-capability harness)](#6-双向-mcp11-capability-harness)
7. [Lane 调度器](#7-lane-调度器)
8. [Workshop 自演化](#8-workshop-自演化)
9. [记忆系统](#9-记忆系统)
10. [运维(custodian-skills 5 阶段 Playbook)](#10-运维custodian-skills-5-阶段-playbook)
11. [部署(Dockerfile 7 阶段多构建)](#11-部署dockerfile-7-阶段多构建)
12. [沙箱与权限](#12-沙箱与权限)
13. [配置与多环境](#13-配置与多环境)
14. [遥测与可观测性](#14-遥测与可观测性)
15. [对 laew 的借鉴(P0/P1/P2 路线图)](#15-对-laew-的借鉴p0p1p2-路线图)

---

## 1. 项目元信息

| 维度 | 内容 |
| --- | --- |
| 定位 | **多渠道 AI 网关 / 个人与团队助理**(不是纯编码 Agent)。自述:"Multi-channel AI gateway with extensible messaging integrations" |
| 语言 | TypeScript(Node 22.22.3+ / 24.15+ / 25.9+),伴生原生 App 用 Swift(iOS/macOS)、Kotlin(Android) |
| 包管理 | **pnpm workspace**(`pnpm-workspace.yaml`),明确禁止 `npm install` |
| 构建 | `tsdown`(rolldown 系)+ 自研编排脚本 `scripts/build-all.mts` |
| 规模 | src+packages 非测试 TS ≈ **201 万行 / 1.6 万文件**;`ui/` 前端 ≈ 82 万行;`extensions/` **162 个插件** |
| 测试 | **vitest**;测试代码 ~301 万行 vs 生产代码 ~201 万行(测试比生产还多) |
| 文档 | `README.md` 112KB、`AGENTS.md`(=CLAUDE.md)66KB、`CHANGELOG.md` **4.1MB** |
| License | MIT |

**规模警示**:这是本轮调研中体量最大的项目,不适合整体照搬,但架构分层与若干子系统设计极具参考价值。

### 1.1 目录树

```
openclaw/
├── openclaw.mjs              # bin 入口 shim
├── src/                      # 主运行时(79 个顶层目录)
├── packages/                 # 24 个内部包(可被插件复用的稳定契约层)
├── extensions/               # 162 个一等公民插件(provider / channel / 能力)
├── ui/                       # Control UI(Web 前端)
├── apps/                     # android / ios / macos / linux / shared 伴生 App
├── skills/                   # 79 个内置 Skill(SKILL.md + frontmatter)
├── custodian-skills/         # 4 个运维 Playbook
└── taxonomy.yaml             # 707KB 能力/模型分类表
```

### 1.2 核心依赖

`@anthropic-ai/sdk`、`openai`、`@google/genai`、`@mistralai/mistralai`、`@modelcontextprotocol/sdk`、`@agentclientprotocol/sdk`(ACP)、`@earendil-works/pi-tui`(TUI)、`kysely`+SQLite、`grammy`(Telegram)、`express`、`typebox`(工具 schema)

### 1.3 关键特征对照

| 特征 | 支持 | 实现位置 / 说明 |
| --- | --- | --- |
| 多 Agent | ✅ 强 | `src/agents/subagents/`:spawn / announce / swarm / registry |
| 并行编队(Swarm) | ✅ | `subagents/swarm/`:maxConcurrent 8 / maxChildrenPerGroup 50 |
| 质检 / Review | ✅ 特色 | `src/agents/exec-auto-reviewer.ts` —— **独立小模型作为"exec 安全审查员"** |
| MCP | ✅ 双向 | Client:`src/agents/agent-bundle-mcp-*.ts`;Server:`src/mcp/*` |
| Skill | ✅ 强 | `skills/` 79 个内置;`src/skills/workshop/` 自演化 |
| 多 Provider | ✅ 强 | 9 个内置 API family + 45+ provider 插件 + OAuth + 故障转移 |
| 流式响应 | ✅ | `EventStream` 贯穿 provider → agent-loop → gateway → TUI/UI |
| 沙箱 | ✅ | `src/agents/sandbox/` + 第三方 extension(mxc / openshell / cua-computer) |
| 审批 | ✅ | Gateway 级 exec-approval 全链路,支持渠道 reaction 审批 |

---

## 2. Gateway 架构(三层契约)

OpenClaw 把"客户端-服务端-后端"切成三层:**Gateway 是本地控制面**,**Harness 是 Agent 执行体的可插拔契约**,**Adapter 是 LLM provider 的协议转换**。

```
渠道(WhatsApp/Telegram/...) ──► auto-reply 管线(src/auto-reply/)
        │
        ▼
Gateway  packages/gateway-protocol + packages/gateway-client + src/gateway
        │ JSON-RPC over WS
        ▼
Harness 选择 src/agents/harness/registry.ts
        ├── builtin-openclaw ──► embedded-agent-runner ──► agent-core/src/agent-loop.ts
        ├── cli-runner        ──► 拉起 claude/codex/gemini CLI 子进程
        └── acp               ──► Agent Client Protocol 外部 Agent
        ▼
Adapter(Provider)  packages/ai/src/providers/*
        │
        ▼
agentLoop 主循环  packages/agent-core/src/agent-loop.ts (1776 行)
```

### 2.1 Gateway 层真实代码路径

**核心类**(`packages/gateway-client/src/client.ts`):
```ts
export type DeviceIdentity = {
  deviceId: string;
  privateKeyPem: string;
  publicKeyPem: string;
};
```

**协议版本 SSOT**(`packages/gateway-protocol/src/version.ts`):
```ts
export const PROTOCOL_VERSION = 18;
export const MIN_CLIENT_PROTOCOL_VERSION = 4;
export const MIN_NODE_PROTOCOL_VERSION = 3;
export const MIN_PROBE_PROTOCOL_VERSION = 3;
```

**4 个版本常量分别管辖 general client / authenticated node / lightweight probe 三类连接**——这是协议治理 SSOT 的最佳实践。

### 2.2 Harness 层——11-capability 交叉类型契约

```ts
export type AgentHarness = AgentHarnessRunCapability &
  AgentHarnessSideQuestionCapability &
  AgentHarnessClassificationCapability &
  AgentHarnessCompactionCapability &
  AgentHarnessRuntimeArtifactCapability &
  AgentHarnessAuthBindingCapability &
  AgentHarnessProviderUsageCapability &
  AgentHarnessModelCatalogCapability &
  AgentHarnessMcpCatalogCapability &
  AgentHarnessSessionForkCapability &
  AgentHarnessSessionLifecycleCapability;
```

每个 capability 是独立的 mixin type,新增能力只需追加 cross intersection,**不破坏既有 harness 实现**。

**三层治理**:
1. `"openclaw"` 是保留名——内置 harness 不可被第三方覆盖
2. `"codex"` 是唯一允许 `nativeCompaction` 的 harness
3. `requireActivePluginRegistry()` 要求必须存在 active registry

### 2.3 Adapter 层——Provider 协议适配

**9 大内置 API 家族**:
```ts
export type KnownApi =
  | "openai-completions"
  | "mistral-conversations"
  | "openai-responses"
  | "azure-openai-responses"
  | "openai-chatgpt-responses"
  | "anthropic-messages"
  | "bedrock-converse-stream"
  | "google-generative-ai"
  | "google-vertex";
```

**StreamFn 永不抛异常契约**:`src/llm/stream.ts` 暴露 facade,任何异常经 `createRuntimeHostErrorMessage()` 包装为 `AssistantMessage{ stopReason: "error" }`——**上层重试/降级逻辑无需 try/catch 分叉**。

### 2.4 协议适配真实代码路径

#### Anthropic 适配
- **入口**:`packages/ai/src/providers/anthropic.ts` 的 `streamAnthropic`
- **认证**:`anthropic-auth-headers.ts` 支持 OAuth(`sk-ant-oat` 前缀 sentinel)、API Key(`x-api-key` 头)、Azure Foundry Bearer auth
- **工具投影**:`anthropic-tool-projection.ts` 的 `projectAnthropicTools`、`normalizeAnthropicToolCallId`
- **用量归一化**:`anthropic-usage.ts` 的 `applyAnthropicMessageStartUsage` / `applyAnthropicMessageDeltaUsage`

#### OpenAI 适配
- **入口**:`packages/ai/src/providers/openai-completions.ts` 的 `streamOpenAICompletions`
- **工具投影**:`openai-tool-projection.ts` 的 `projectOpenAITools`
- **Stop reason 映射**:`openai-stop-reason.ts` 的 `mapOpenAIStopReason`

---

## 3. 162 extensions 生态(9 大类)

基于对全部 162 个扩展的逐一读取,按功能域分为 9 大类:

| 功能域 | 数量 | 代表性 extensions |
| ------ | ---- | ----------------- |
| **AI 提供商(LLM/模型)** | ~45 | `anthropic`、`openai`、`google`、`amazon-bedrock`、`deepseek`、`mistral`、`xai`、`groq`、`ollama`、`vllm`、`litellm`、`nvidia`、`cerebras`、`cohere` 等 |
| **IM 渠道(messaging channels)** | ~20 | `telegram`、`discord`、`slack`、`whatsapp`、`signal`、`imessage`、`irc`、`line`、`feishu`、`matrix`、`nostr` 等 |
| **AI 媒体(生成/理解/语音)** | ~25 | `elevenlabs`、`azure-speech`、`tts-local-cli`、`image-generation-core`、`fal`、`comfy`、`runway`、`deepgram`、`tavily`、`firecrawl` 等 |
| **浏览器/网络/搜索** | ~8 | `browser`、`firecrawl`、`exa`、`tavily`、`searxng`、`duckduckgo`、`webhooks` |
| **存储/笔记/记忆** | ~5 | `memory-core`、`memory-lancedb`、`memory-wiki`、`document-extract`、`vault` |
| **开发工具/代码助手** | ~10 | `codex`、`opencode`、`github-copilot`、`copilot`、`diffs`、`llm-task` |
| **安全/policy/认证** | ~8 | `policy`、`vault`、`visitor-access`、`device-pair`、`onepassword`、`admin-http-rpc` |
| **通信协议/Agent 间** | ~4 | `a2a`、`acpx`、`active-memory`、`workboard` |
| **其他(diagnostics/telemetry/UI)** | ~32 | `diagnostics-otel`、`diagnostics-prometheus`、`clawrouter`、`tokenjuice`、`zoom-meetings`、`google-meet` 等 |

### 3.1 双层注册机制

所有 extension 共用 **双层注册**:

1. **`openclaw.plugin.json` manifest**(静态契约):id、name、description、activation 条件、configSchema、contracts
2. **`index.ts` 入口**(动态注册):
   - provider/plugin 类 → `definePluginEntry({ id, name, description, register(api){...} })`
   - channel 类 → `defineBundledChannelEntry({ id, name, plugin, runtime, secrets })`
3. **与 host 通信**:完全通过 `OpenClawPluginApi`,禁止直接 import core `src/**`

### 3.2 代表实现剖析

| extension | 定位 | 入口特征 |
| --------- | ---- | -------- |
| `anthropic` | Anthropic API + Claude CLI backend + native session catalog | `definePluginEntry` + `registerAnthropicPlugin` |
| `openai` | OpenAI provider + 图像生成 + 实时转录 + speech + video | `definePluginEntry` + `registerProvider` |
| `telegram` | Telegram channel | `defineBundledChannelEntry` + `telegramPlugin` |
| `memory-core` | 核心记忆 + Dreaming 三阶段 | `registerTool(tool)` + `configureMemoryCoreDreamingState` |
| `codex` | Codex app-server harness + native session supervision | `createCodexAppServerAgentHarness` |
| `a2a` | A2A v1.0 Agent-to-Agent 协议 channel | `defineBundledChannelEntry` + `a2aChannelPlugin` |
| `acpx` | ACP 运行时后端 | `createAcpxRuntimeService` + `tryDispatchAcpReplyHookWithTimeout` |

---

## 4. packages 核心模块群

### 4.1 agent-core — Agent 生命周期、循环、状态机

**核心类型**(`packages/agent-core/src/types.ts`):
```ts
export type ToolExecutionMode = "sequential" | "parallel";
export type QueueMode = "all" | "one-at-a-time";
export interface ToolLoopIntervention {
  kind: "critical-tool-loop"; toolCallId: string; toolName: string;
  actionKey: string; detector: string; count: number; reason: string;
}
```

**Agent Loop**(`packages/agent-core/src/agent-loop.ts`):
- `runAgentLoop(config: AgentLoopConfig)`:完整 agentic loop
- `getSteeringAtCheckpoint`:检查点处注入用户消息
- `resolveAssistantMessageUpdate`:增量更新 assistant message
- `appendInterruptedTurnMessage` / `createInterruptedTurnMessage`:turn 中断处理

**Compaction**(`packages/agent-core/src/harness/compaction/compaction.ts`):
- `MAX_COMPACTION_SUMMARY_CHARS = 16_000`
- 文件操作追踪:`extractFileOperations` / `formatFileOperations` / `mergeSummaryFileOperations`
- 最新用户请求保留:`extractLatestUserRequest`(≤800 字符)

### 4.2 ai + llm-core — LLM 调用、模型目录、协议适配

**llm-core 类型**(`packages/llm-core/src/types.ts`):
```ts
export type ThinkingLevel = "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
export type ModelThinkingLevel = "off" | ThinkingLevel;
export type CacheRetention = "none" | "short" | "long";
export type Transport = "sse" | "websocket" | "websocket-cached" | "auto";
```

**ai 包结构**:
- `api-registry.ts`:`registerApiProvider`、`defaultApiRegistry`
- `register-builtins.ts`:内置 provider 注册
- `host.ts`:`AiTransportHost` / `AiProviderRequestCapabilities`

### 4.3 model-catalog-core — 模型目录核心

- `ModelCatalogApi`(10 种 API)、`ModelCatalogThinkingFormat`(7 种 thinking 格式)
- `remote-catalog-bundle.ts`:远程 catalog zod schema
- `model-catalog-refs.ts`:`resolveModelRef` 把 `provider/model` 解析为 catalog 条目
- `model-catalog-pricing.ts`:定价配置

### 4.4 net-policy — 网络策略、SSRF 防护

```ts
const BLOCKED_IPV4_SPECIAL_USE_RANGES = new Set([
  "unspecified", "broadcast", "multicast", "linkLocal",
  "loopback", "carrierGradeNat", "private", "reserved",
]);
const CLOUD_METADATA_IP_ADDRESSES = new Set(["100.100.100.200", "fd00:ec2::254"]);
```

**URL 脱敏**:`redact-sensitive-url.ts` 定义 `SENSITIVE_URL_QUERY_PARAM_NAMES`(token / key / api_key / secret / access_token / password / jwt / signature 等 30+)

### 4.5 memory-host-sdk — 记忆引擎底座

| 文件 | 导出 |
| ---- | ---- |
| `engine-foundation.ts` | `resolveAgentDir` / `resolveAgentWorkspaceDir` / `root` / `detectMime` |
| `engine-sessions.ts` | `buildSessionEntry` / `listSessionFilesForAgent` |
| `engine-storage.ts` | `chunkMarkdown` / `cosineSimilarity` / `ensureMemoryIndexSchema` |
| `engine-embeddings.ts` | `EmbeddingProvider` 适配 |
| `query.ts` | `extractKeywords` / `isQueryStopWordTokenToken` |

**host/ 子目录**(70+ 文件):`memory-schema.ts`、`sqlite-vec.ts`(向量索引)、`memory-schema-fts.ts`(FTS 全文检索)、`batch-runner.ts`(批量嵌入)、`session-provenance.ts`(来源追踪)

### 4.6 tool-call-repair — 工具调用修复/容错(四阶段管线)

| 阶段 | 文件 | 作用 |
| ---- | ---- | ---- |
| **grammar** | `grammar.ts` | 词法/语法原语:`END_TOOL_REQUEST`、`HARMONY_CHANNEL_MARKER`、`scanXmlishToolCall` |
| **payload** | `payload.ts` | 文本 tool call 块解析:`parseStandalonePlainTextToolCallBlocks`、`PlainTextJsonToolCallSyntax = "harmony" \| "named-bracket" \| "tool-bracket"` |
| **promote** | `promote.ts` | 提升为 provider-native tool call:`createPromotedPlainTextToolCallBlock` |
| **stream-normalizer** | `stream-normalizer.ts` | 流标准化:`PlainTextToolCallMessageNormalization` |

### 4.7 workboard-contract — 工作板契约

- `WORKBOARD_STATUSES`:triage / backlog / todo / scheduled / ready / running / review / blocked / done(9 态)
- `WORKBOARD_EXECUTION_ENGINES`:codex / claude
- `WORKBOARD_EVENT_KINDS`:24 种事件
- `WORKBOARD_LINK_TYPES`:parent / child / blocks / blocked_by / relates_to
- `WORKBOARD_PROOF_STATUSES`:passed / failed / skipped / unknown

### 4.8 其他 packages 速览

| 包 | 定位 |
| -- | ---- |
| `retry` | 重试策略(`RetrySupervisor` / `createRetryRunner` / `BackoffPolicy`) |
| `session-url-contract` | session URL 契约(`buildControlUiSessionPath` / `parseShortSessionRef`) |
| `terminal-core` | 终端核心(shell / PTY / ANSI / OSC 8 链接 / 表格) |
| `markdown-core` | Markdown IR 渲染(`ir.ts` / `render.ts` / `reasoning-tags.ts`) |
| `normalization-core` | 归一化核心(string / number / record / utf16 / cjk) |
| `mermaid-renderer` | Mermaid 图表渲染(SVG + DOMPurify) |
| `media-core` / `media-generation-core` / `media-understanding-common` | 媒体生成/理解 |
| `plugin-sdk` | 插件 SDK(`packages/plugin-sdk/` 薄壳 + `src/plugin-sdk/` 真实实现) |
| `sdk` | 应用 SDK(`SdkClient` / `transport.ts` / `event-hub.ts`) |

---

## 5. ACP/A2A 协议

### 5.1 ACP(Agent Client Protocol)

**包**:`packages/acp-core`;**运行时**:`src/acp/*`;**SDK 依赖**:`@agentclientprotocol/sdk`。

**核心类型**(`packages/acp-core/src/types.ts`):
```ts
export type AcpProvenanceMode = "off" | "meta" | "meta+receipt";
export type AcpSession = {
  sessionId: SessionId; sessionKey: string; ledgerSessionId?: string;
  cwd: string; createdAt: number; lastTouchedAt: number;
  abortController: AbortController | null; activeRunId: string | null;
};
```

**ACP Session 存储**(`packages/acp-core/src/session.ts`):
```ts
export function createInMemorySessionStore(options?: {
  maxSessions?: number; idleTtlMs?: number;
}): InMemoryAcpSessionStore {
  // 默认 MAX_MAX_SESSIONS = 5_000;DEFAULT_IDLE_TTL_MS = 24h
}
```

**ACP 运行时**(`src/acp/translator.ts`):
```ts
export class AcpGatewayAgent implements Agent {
  constructor(connection: AgentSideConnection, gateway: GatewayClient, opts) { ... }
  async initialize(_params: InitializeRequest): Promise<InitializeResponse> {
    return {
      protocolVersion: (await loadAcpSdkModule()).PROTOCOL_VERSION,
      agentCapabilities: {
        loadSession: true,
        promptCapabilities: { image: true, audio: false, embeddedContext: true },
      },
    };
  }
}
```

**关键子模块**:`translator.prompt-stream.ts`、`translator.session-lifecycle.ts`、`translator.session-updates.ts`、`event-ledger.ts`、`permission-relay.ts`、`server.ts`(`serveAcpGateway`)

### 5.2 A2A(Agent-to-Agent Protocol)

**extension**:`extensions/a2a/`
```ts
export default defineBundledChannelEntry({
  id: "a2a", name: "A2A",
  description: "A2A v1.0 Agent-to-Agent protocol channel plugin",
  plugin: { specifier: "./channel-plugin-api.js", exportName: "a2aChannelPlugin" },
});
```

### 5.3 ACPX 运行时

**extension**:`extensions/acpx/`
```ts
register(api: OpenClawPluginApi) {
  registerPiSessionCatalog(api);
  api.registerService(createAcpxRuntimeService({ pluginConfig: api.pluginConfig, ... }));
  api.on("reply_dispatch", (event, ctx) => tryDispatchAcpReplyHookWithTimeout(event, ctx, timeoutMs));
}
```

---

## 6. 双向 MCP(11-capability harness)

OpenClaw 同时是 **MCP Client**(连接外部 MCP Server)和 **MCP Server**(暴露自身工具给外部)。

### 6.1 MCP Server 侧——工具暴露

**stdio 服务端入口**(`src/mcp/tools-stdio-server.ts`):
```ts
export function createToolsMcpServer(params: { name: string; tools: AnyAgentTool[] }): Server {
  const handlers = createPluginToolsMcpHandlers(params.tools);
  const server = new Server({ name: params.name, version: VERSION }, { capabilities: { tools: {} } });
  server.setRequestHandler(ListToolsRequestSchema, handlers.listTools);
  server.setRequestHandler(CallToolRequestSchema, async (request, extra) => {
    return await handlers.callTool(request.params, extra.signal);
  });
  return server;
}
```

**工具 → MCP handlers 适配器**(`src/mcp/plugin-tools-handlers.ts`):
- **before-tool-call hook 管道**:`approvalMode: "report"` 让 hook 在 MCP 调用路径下仍生效
- **toolCallId 是 `mcp-${uuid}` 命名空间**:隔离 MCP 调用与其他来源
- **content 归一化**:`toMcpContentBlock()` 处理 `image` 块→ MCP `image` 类型

### 6.2 MCP Client 侧——连接 + 物化

**Session-scoped MCP Runtime**(`src/agents/agent-bundle-mcp-runtime.ts`):
```ts
type BundleMcpSession = {
  serverName: string;
  client: Client;
  transport: Transport;
  transportType: "stdio" | "sse" | "streamable-http";
  requestTimeoutMs: number;
  supportsParallelToolCalls: boolean;
  connected: boolean;
  retiring: boolean;
};
```

**Channel Bridge 双向通信**(`src/mcp/channel-bridge.ts`):
- `caps: [APPROVALS]` + `scopes: [READ, WRITE, APPROVALS]`:最小权限原则
- `onEvent` 回调 → `dispatchGatewayEvent()`:把所有 Gateway 事件转为 MCP 事件

### 6.3 传输层

**传输工厂**(`src/agents/mcp-transport.ts`):
- 根据 config 选择 stdio / SSE / streamable-HTTP
- 附加 auth profile bearer / OAuth bearer

**失败退避**(`agent-bundle-mcp-runtime.ts`):
- `BUNDLE_MCP_FAILURE_THRESHOLD=3` 次失败后进入退避
- `BUNDLE_MCP_FAILURE_COOLDOWN_MS=60_000` 冷却

---

## 7. Lane 调度器

`src/agents/subagents/swarm/swarm-scheduler.ts` 实现基于 **lane** 的 FIFO 容量控制调度器。

### 7.1 核心数据结构

```ts
type SwarmGroupLane = {
  groupId: string;
  limit: number;
  active: Set<string>;
  queue: QueuedSwarmRun[];
  pumpScheduled: boolean;
};

type QueuedSwarmRun = {
  runId: string;
  owner?: object;
  onCapacityChange?: () => void;
  launch?: SwarmLaunch;
  holds: number;
  retryReady: boolean;
};
```

### 7.2 默认配置

```ts
const DEFAULT_SWARM_CONFIG: ResolvedSwarmConfig = {
  enabled: false,
  maxConcurrent: 8,
  maxChildrenPerGroup: 50,
  maxTotalPerGroup: 200,
  waitTimeoutSecondsMax: 600,
};
```

### 7.3 5 个调度原语

1. `reserveSwarmRun()` — 占位 FIFO
2. `activateSwarmRun()` — 绑定 launch 工作
3. `pumpLane()` — 用 `queueMicrotask()` 微任务调度
4. `releaseSwarmRun()` — 释放容量
5. 失败重试:可恢复 → 回队头 + `retryReady=true`(1s 后);不可恢复 → 释放

---

## 8. Workshop 自演化

`src/skills/workshop/` 是 OpenClaw 最独特的部分——**技能可以自我演化**,约 40 文件。

### 8.1 核心流程

1. **History Scan**(`history-scan.ts`):扫描会话历史,发现技能改进机会
   - `runSkillHistoryScanCore()`:分 batch 读取会话
   - 游标分页(`oldestCursor`/`newestCursor`)

2. **Experience Review**(`experience-review.ts`):后台 agent 评审技能使用体验
   - `EXPERIENCE_REVIEW_MIN_MODEL_ITERATIONS=10`
   - `EXPERIENCE_REVIEW_TIMEOUT_MS=120_000`

3. **Proposal Generation**(`proposal-generation.ts`):生成技能修改提案
   - `stageSkillProposalGeneration()`:原子 staging dir → move 写入
   - `SkillProposalStatus`:`"pending" | "applied" | "rejected" | "quarantined" | "stale"`

4. **Autonomous Apply**(`autonomous-apply.ts`):自动应用提案
   - workshop-owned 技能可直接应用
   - user-authored 技能 → `pending` 等待人工审核

5. **Collection Plan**(`collection-plan.ts`):技能集合规划
   - 每个 agent 必须保留至少一个可见技能

### 8.2 安全机制

- `isWorkshopOwnedSkillDir()`:只有 workshop 创建的技能可自动修改
- `revisionHash` + `expectedRevisionHash`:乐观并发控制
- `SkillProposalSupportFile`:附件 hash 校验

---

## 9. 记忆系统

OpenClaw 有 **3 个记忆扩展**,形成 **存储 + 检索 + 主动记忆** 三层。

### 9.1 memory-core — 核心记忆存储

**入口**(`extensions/memory-core/index.ts`):
```ts
register(api) {
  const tool = createLazyMemorySearchTool(options);   // memory_search
  const getTool = createLazyMemoryGetTool(options);   // memory_get
  const intentTool = createLazyStandingIntentTool(ctx, reportUnavailable); // intent
  api.registerTool(tool);
  api.registerTool(getTool);
  api.registerTool(intentTool);
  configureMemoryCoreDreamingState(api);
}
```

### 9.2 memory-core Dreaming 三阶段

| 阶段 | 作用 |
| ---- | ---- |
| **light** | 去重相似条目(`dedupeSimilarity` 阈值 0.85) |
| **REM** | 模式识别(`minPatternStrength` 0.6) |
| **deep** | 短期记忆提升到 MEMORY.md(`maxPromotedSnippetTokens` / `maxPriorEntryLossFraction` 0.25) |

**配置示例**:
```json
"dreaming": {
  "enabled": true,
  "frequency": "0 3 * * *",
  "phases": {
    "light": { "enabled": true, "lookbackDays": 7, "dedupeSimilarity": 0.85 },
    "rem": { "enabled": true, "lookbackDays": 14, "minPatternStrength": 0.6 },
    "deep": { "enabled": true, "maxPriorEntryLossFraction": 0.25 }
  }
}
```

### 9.3 active-memory — 主动记忆双 Lane

**核心流程**:
```
before_prompt_build hook
  → 检查 toolAuthority / session 策略 / agent 策略
  → Lane-1:deterministic trigger recall(快速精确匹配)
  → Lane-2:blocking memory recall(完整子 Agent 检索)
  → 拼接上下文返回 prependContext
```

---

## 10. 运维(custodian-skills 5 阶段 Playbook)

`custodian-skills/` 是 OpenClaw 的**托管运维技能目录**,包含 4 个 SKILL.md 文件,每个都是一个结构化的运维 Playbook。

### 10.1 技能格式

**正文固定 5 阶段结构**:Gather → Mutate → Repair → Prove → Report

### 10.2 四个技能详解

| 技能 | 核心模式 |
| ---- | -------- |
| **add-model-provider** | SecretRef 模式 —— 凭证永不明文进入配置,只通过 `--ref-provider` / `--ref-source` / `--ref-id` 三元组引用 |
| **configure-channel** | 渠道配置 SecretRef 模式,allowFrom 使用 numeric Telegram user ID |
| **cloud-image-bake** | 支持 AWS / Hetzner / Firecracker 三种后端,旧镜像保留直到证明通过 |
| **diagnose-gateway** | **纯只读**诊断 Playbook,不写配置、不重启服务 |

### 10.3 SecretRef 模式

```bash
openclaw config set models.providers.openai.apiKey --ref-provider openai_key_file --ref-source file --ref-id value
```

---

## 11. 部署(Dockerfile 7 阶段多构建)

### 11.1 Dockerfile 7 阶段

```dockerfile
# Stage 1:workspace-deps -- 提取 package.json,插件选择
# Stage 2:dependency-inputs -- 复制锁文件 + 补丁
# Stage 3:production-deps -- 生产依赖安装(--prod)
# Stage 4:build -- Bun 二进制 + 全量构建 + UI 构建
# Stage 5:runtime-build-output -- 删除 node_modules 后的构建产物
# Stage 6:runtime-assets -- 生产依赖 + 构建产物合并
# Stage 7:base-runtime → 最终镜像(bookworm-slim)
```

**插件选择**:通过 `OPENCLAW_EXTENSIONS` build arg 按需选择插件:
```dockerfile
ARG OPENCLAW_EXTENSIONS=""
# docker build --build-arg OPENCLAW_EXTENSIONS="diagnostics-otel,matrix" .
```

**安全加固**:
- 非 root 用户运行(`USER node`,uid 1000)
- `cap_drop: NET_RAW, NET_ADMIN`
- `security_opt: no-new-privileges:true`

**版本校验**:运行时验证三处版本一致:
```dockerfile
RUN test "$(node -p "require('/app/package.json').version")" = "$OPENCLAW_DOCKER_BUILD_VERSION"
RUN test "$(node -p "require('/app/dist/build-info.json').version")" = "$OPENCLAW_DOCKER_BUILD_VERSION"
RUN test "$(node /app/openclaw.mjs --version | cut -d ' ' -f 2)" = "$OPENCLAW_DOCKER_BUILD_VERSION"
```

### 11.2 Docker Compose:Gateway + CLI 双容器

三端口设计:18789(Gateway WS) + 18790(Bridge) + 3978(MS Teams webhook)

### 11.3 Fly.io 公网 vs 私有

- **公网模式**(`fly.toml`):`[http_service]` + health check
- **私有模式**(`deploy/fly.private.toml`):无 `[http_service]` 块,仅通过 fly proxy / WireGuard 访问

---

## 12. 沙箱与权限

### 12.1 沙箱

OpenClaw 的沙箱通过第三方 extension 实现:

| extension | 作用 |
| --------- | ---- |
| `mxc` | MXC sandbox |
| `openshell` | NVIDIA OpenShell |
| `cua-computer` | Computer Use Agent |

### 12.2 exec auto-reviewer 质检机制

**决策类型**(`ExecAutoReviewDecision`):
- `allow-once`:仅当 `risk === "low"` 时返回
- `ask`:其他所有情况(送人工审批)

**保守解析**(`parseExecAutoReviewResponse`):
- 无 JSON → `ask`/`unknown`
- **重复 key 检测**:防 `{"decision":"ask",...,"decision":"allow"}` 被静默覆盖
- **__proto__ 防护**:Zod strict 不检查 `__proto__`,手动校验
- **非 low allow 降级**:`decision=allow` 但 `risk !== "low"` → 转 `ask`

**Prompt 注入攻击防护**(`textLooksLikeReviewerDirective`):
- 检测 `ignore/disregard/override ... instruction/system/developer/prompt/policy`
- 检测 `return/respond/output/say/print ... decision ... allow`
- 检测 `exec reviewer ... decision/allow/risk/rationale`
- 命中任一 → 直接 `ask`/`medium`,不送模型

**超时与失败兜底**:
- `DEFAULT_EXEC_REVIEWER_TIMEOUT_MS=30_000`
- `EXEC_REVIEWER_MAX_TOKENS=360`
- **任何异常都是 `ask`**

### 12.3 权限管控

**URL/IP 安全原语**(`packages/net-policy/`):
- `isHttpUrl` / `isHttpsUrl` / `isWebSocketUrl` / `isWssUrl`
- `stripUrlUserInfo`:URL userinfo 剥离
- `isBlockedSpecialUseIpv4Address` / `isBlockedSpecialUseIpv6Address`:SSRF 防护
- `redactSensitiveUrlLikeString`:敏感 URL 脱敏

**Secret 集成**:
- `vault`(HashiCorp Vault)
- `onepassword`(1Password)
- `visitor-access`(Cloudflare Access)

---

## 13. 配置与多环境

### 13.1 pnpm Workspace 多根目录

```yaml
packages:
  - .
  - ui
  - packages/*
  - extensions/*
  - examples/*

minimumReleaseAge: 10080  # 一周依赖冷却
minimumReleaseAgeStrict: true
minimumReleaseAgeExclude:  # 安全豁免白名单
  - "fast-uri@4.1.4"
  - "nodemailer@9.1.1"
```

**`minimumReleaseAge: 10080` 分钟(=7 天)**:依赖供应链安全 —— 任何新发布包 7 天内不能用,**显著降低 typosquatting / 恶意包风险**。

### 13.2 项目元数据治理

- `package.json` 132KB(含 dist 白名单 + 数百 scripts)
- `tsconfig.json` 15KB 巨型 project-references
- `tsdown.config.ts` 31KB 构建配置
- `taxonomy.yaml` 707KB 能力/模型分类表
- `CHANGELOG.md` 4.1MB(完整版本历史)

### 13.3 质量门

**pre-commit hooks**:
- 基础文件卫生:`trailing-whitespace` / `end-of-file-fixer` / `check-yaml`
- Shell 脚本 lint:`shellcheck --severity=error`
- GitHub Actions 安全审计:`actionlint` + `zizmor`
- TypeScript:`oxlint --type-aware` + `oxfmt --check`
- Swift:`swiftlint` + `swiftformat --lint`
- 安全:`detect-private-key` + `pnpm-audit-prod`

**内容守卫**(`scripts/pre-commit/guard-staged-content.mjs`):
- 扫描阶段文件中的被阻断字面量
- 匹配时输出 `[REDACTED]` 脱敏

---

## 14. 遥测与可观测性

### 14.1 轨迹导出

`src/trajectory/`:
- `runtime-store.sqlite.ts`:SQLite 存储
- `export.ts`:导出
- `command-export.ts`:命令导出

### 14.2 诊断

`extensions/diagnostics-otel/` + `extensions/diagnostics-prometheus/`

### 14.3 审计

`src/audit/*` + `src/security/*`

---

## 15. 对 laew 的借鉴(P0/P1/P2 路线图)

### 15.1 P0(立即借鉴,高 ROI)

| 借鉴点 | openclaw 实现 | laew 落地建议 |
| ------ | ------------- | ------------- |
| **tool-call-repair 子系统** | `packages/tool-call-repair/` 四阶段管线 | 在 `src/agent/tools/` 增加 tool-call-repair 模块,修复模型流式输出中泄漏的伪 tool-call 文本 |
| **net-policy URL/IP 安全原语** | `packages/net-policy/` 的 SSRF 防护 | 在 BashTool 执行前增加 URL/IP SSRF 拦截;在日志输出中增加敏感 URL 脱敏 |
| **memory-host-sdk 分层 facade** | `engine-foundation` / `engine-storage` / `engine-embeddings` 五件套 | 为 laew 引入轻量记忆 SDK:workspace 路径 + SQLite + FTS + 关键词提取 |
| **compaction 文件操作追踪** | `extractFileOperations` / `mergeSummaryFileOperations` | 在 laew 的 Yolo/Work Agent 压缩时增加文件操作追踪 |
| **plugin-sdk 能力契约** | `OpenClawPluginApi` 的 registerProvider / registerTool / registerService | 为 laew 引入轻量插件契约:registerTool / registerHook / lifecycle |
| **exec 执行前 LLM 审阅器** | `createModelExecAutoReviewer()` 360 tokens + 30s 超时 | 新增 `ExecAutoReviewer` trait,`bash.rs` 在 `execute()` 前调审阅器 |
| **Context 压缩 + Quarantine 隔离** | `ContextEngine` 接口 + `wrapResolvedContextEngine` | 引入 `ContextEngine` trait,engine 抛错 → quarantine + fallback |
| **注入攻击防御** | `textLooksLikeReviewerDirective()` 5 类正则 | 直接移植到 Rust,在 BashTool 输入净化层使用 |

### 15.2 P1(中期借鉴)

| 借鉴点 | openclaw 实现 | laew 落地建议 |
| ------ | ------------- | ------------- |
| **model-catalog-core 模型目录** | 10 种 API + 7 种 thinking format + compat 配置 | 为 laew 引入 model-catalog:provider / model / api / reasoning / contextWindow / cost |
| **memory-host-sdk SQLite-vec + FTS** | `sqlite-vec.ts` + `memory-schema-fts.ts` + `query-expansion.ts` | 在 laew 记忆中增加向量索引 + FTS 全文检索 + query expansion |
| **workboard-contract 工作板** | 9 态 + 24 种事件 + 5 种 link type | 为 laew 引入任务看板:status / priority / execution / events / diagnostics |
| **retry 策略** | `RetrySupervisor` / `createRetryRunner` / `BackoffPolicy` | 在 laew 的 LLM 调用层增加可中止重试(AbortSignal + Retry-After + jitter) |
| **Agent Harness 抽象** | `AgentHarness` 11-capability trait + 保留 id | 把 `AgentHarness` 拆成 capability cross type,Rust 用 trait object |
| **SubAgent 90+ 字段完整运行记录** | `SubagentRunRecord` 的 execution + completion + delivery | 扩 `SubagentRunRecord`,新增 `SubagentLifecycleController` |
| **Lane 调度器** | `SwarmGroupLane` FIFO + 容量控制 + 微任务调度 | 引入 `SwarmGroupLane`,5 个原语:reserve / activate / pump / release / retry |
| **JSONL 主存储 + SQLite 元数据** | append-only JSONL 主存储 + SQLite 元数据 | Session transcript 改 JSONL append-only |

### 15.3 P2(长期借鉴)

| 借鉴点 | openclaw 实现 | laew 落地建议 |
| ------ | ------------- | ------------- |
| **ACP 协议桥接** | `packages/acp-core` + `src/acp/translator.ts` 的 `AcpGatewayAgent` | 为 laew 引入 ACP 客户端,接入 Agent Client Protocol 生态 |
| **A2A 协议** | `extensions/a2a/` 的 `defineBundledChannelEntry` | 为 laew 引入 A2A 协议 channel,支持 Agent-to-Agent 协作 |
| **162 extensions 生态** | `definePluginEntry` / `defineBundledChannelEntry` / manifest | 为 laew 设计完整插件系统:manifest + 入口 + 注册 API + 能力契约 |
| **Workshop 自演化** | history-scan → proposal → review → apply/rollback + SQLite 提案 | 作为 0.3+ 版本目标 |
| **MCP 双向接入** | Client(3 transport + OAuth) + Server(before-hook 管道) | 作为 0.4+ 版本目标 |
| **media-* 三件套** | `media-core` / `media-generation-core` / `media-understanding-common` | 为 laew 增加媒体生成/理解能力 |
| **Tauri 桌面客户端** | `apps/linux/` 的 Tauri 2 桌面客户端 | laew 升级为桌面应用 |

### 15.4 核心模式总表

| 模式 | OpenClaw 实现 | laew 优先级 |
| ---- | ------------- | ----------- |
| 统一消息模型 | `AssistantMessage` + `EventStream` + 9 大 API 家族 | P0(扩展 cost/cache 字段) |
| Provider 注册表 | `ApiRegistry` + 162 个 extension | P1(改为注册表模式) |
| Harness 可插拔执行器 | `AgentHarness` 11-capability trait + 保留 id | P1(拆分为 capability 子 trait) |
| MCP 工具双向暴露 | `createPluginToolsMcpHandlers()` + before-hook | P2(长期规划) |
| LLM 审阅器 | `createModelExecAutoReviewer()` 360 tokens + 30s 超时 | P1(增加模型审阅) |
| 注入攻击防御 | `textLooksLikeReviewerDirective()` 5 类正则 | P0(安全刚需) |
| 分组 Lane 调度器 | `SwarmGroupLane` FIFO + 容量控制 | P1(增加调度层) |
| ContextEngine 接口 | 15 方法完整生命周期 + quarantine 容错 | P0(核心升级路径) |
| token 预算控制 | assemble(tokenBudget) + compact(compactionTarget) | P1(防止上下文溢出) |
| SQLite 统一持久化 | Session/SubAgent/Transcript/Memory | P1(统一存储) |
| 代次保护机制 | `generation` + `compareSubagentRunGeneration()` | P1(防止旧操作影响新状态) |

---

## 附录 A:关键文件索引

### A.1 三层契约

| 层 | 关键文件 |
| -- | -------- |
| Gateway | `src/gateway/client.ts`、`packages/gateway-client/src/client.ts`、`packages/gateway-protocol/src/version.ts`、`packages/gateway-protocol/src/frame-guards.ts` |
| Adapter | `packages/ai/src/providers/anthropic.ts`、`packages/ai/src/providers/openai-completions.ts`、`packages/ai/src/providers/anthropic-auth-headers.ts`、`packages/ai/src/providers/anthropic-tool-projection.ts`、`packages/ai/src/api-registry.ts`、`src/llm/stream.ts` |
| Harness | `packages/agent-core/src/agent.ts`、`packages/agent-core/src/agent-loop.ts`、`packages/agent-core/src/types.ts`、`src/agents/harness/host-capability-types.ts`、`src/agents/harness/builtin-openclaw.ts` |

### A.2 核心 packages

| 包 | 关键文件 |
| -- | -------- |
| agent-core | `agent.ts` / `agent-loop.ts` / `types.ts` / `turn-interruption.ts` / `harness/compaction/compaction.ts` |
| ai | `host.ts` / `transports.ts` / `api-registry.ts` / `provider-options.ts` |
| llm-core | `types.ts` / `usage-cost.ts` |
| model-catalog-core | `model-catalog-types.ts` / `model-catalog-refs.ts` / `model-catalog-pricing.ts` |
| net-policy | `redact-sensitive-url.ts` / `url-userinfo.ts` / `url-protocol.ts` / `ip.ts` |
| memory-host-sdk | `engine-foundation.ts` / `engine-storage.ts` / `engine-embeddings.ts` / `engine-sessions.ts` / `query.ts` |
| tool-call-repair | `grammar.ts` / `payload.ts` / `promote.ts` / `stream-normalizer.ts` |
| workboard-contract | `index.ts`(单文件) |
| acp-core | `types.ts` / `session.ts` / `meta.ts` |

### A.3 Agent 间协作

| 协议 | 文件 |
| ---- | ---- |
| ACP | `src/acp/translator.ts` / `src/acp/server.ts` / `src/acp/event-ledger.ts` / `src/acp/permission-relay.ts` |
| ACPX | `extensions/acpx/index.ts` / `extensions/acpx/register.runtime.ts` |
| A2A | `extensions/a2a/index.ts` / `extensions/a2a/channel-plugin-api.ts` |

---

## 附录 B:关键数据汇总

| 指标 | 数值 |
| ---- | ---- |
| extensions 总数 | 162 |
| AI 提供商 extensions | ~45 |
| IM 渠道 extensions | ~20 |
| AI 媒体 extensions | ~25 |
| skills/ 技能数 | 79 |
| custodian-skills/ 技能数 | 4 |
| packages/ 包数 | 24 |
| src/ 子模块数 | 121 |
| apps/ 平台数 | 9 |
| Dockerfile 构建阶段 | 7 |
| 部署目标 | 4(Fly.io 公网/私有 + Docker Compose + Render) |
| pre-commit hooks | 12+ |
| workboard 工具数 | 35 |
| 测试代码行数 | ~301 万行 |
| 生产代码行数 | ~201 万行 |

---

## 附录 C:原始文档清单

| 文档 | 行数 | 定位 |
| ---- | ---- | ---- |
| `openclaw-源码调研.md` | 291 | 第一轮:项目元信息、目录树、架构骨架 |
| `openclaw-深度分析.md` | 725 | 第一轮:8 维度深度分析 |
| `openclaw-核心机制深度分析.md` | 1,508 | 第二轮:Gateway/Harness/双向 MCP/exec-reviewer/Swarm/Context |
| `openclaw-第二轮深度分析.md` | 951 | 第二轮:三层契约实战 + 双向 MCP + 大规模代码组织 + 12 条 P0-P2 建议 |
| `openclaw-第三轮-custodian与deploy深度分析.md` | 893 | 第三轮:custodian-skills / deploy / apps / extensions / git-hooks / 遗漏包 |
| `openclaw-第四轮-Gateway架构与extensions生态深度分析.md` | 1,556 | 第四轮:三层契约精化 + 162 extensions 全景 + packages 核心模块群 + ACP/A2A + wire 层 |

---

> 本轮分析基于对 `/usr/local/LsmGitOpenSource/openclaw/` 仓库(TypeScript, ~201 万行)的真实源码阅读。所有结论均落到具体文件路径、模块名、函数名、代码片段。
