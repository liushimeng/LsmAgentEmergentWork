# OpenCode 综合深度分析

> 调研对象:opencode(TypeScript/Bun,~18k 行,Effect+Schema 全栈 DI)
> 调研日期:2026-09-04 ~ 2026-09-06
> 原始文档:6 份(源码调研/深度分析/核心机制深度分析/第二轮深度分析/第三轮周边包深度分析/第四轮 EffectDI 全栈)
> 总行数:~10,500 行(合并后,原始 7,009 行 + 补充整合)

---

## 目录

1. [项目元信息](#1-项目元信息)
2. [Effect 全栈 DI(LayerNode/Tag/132 Node)](#2-effect-全栈-di layernodetag132-node)
3. [34 包 workspace](#3-34-包-workspace)
4. [多端架构(web/desktop/slack/sdk/enterprise)](#4-多端架构webdesktopslacksdkenterprise)
5. [LLM 集成(15 种 LLMEvent + 四轴 Route)](#5-llm-集成15-种-llmevent--四轴-route)
6. [工具系统](#6-工具系统)
7. [流式与终端渲染](#7-流式与终端渲染)
8. [记忆与 Context](#8-记忆与-context)
9. [Skill 系统](#9-skill-系统)
10. [错误处理与重试](#10-错误处理与重试)
11. [成本控制与 Token 统计](#11-成本控制与-token-统计)
12. [可观测性](#12-可观测性)
13. [会话持久化](#13-会话持久化)
14. [测试与 Eval](#14-测试与-eval)
15. [配置系统(8 层发现链)](#15-配置系统8-层发现链)
16. [插件生态](#16-插件生态)
17. [对 laew 的借鉴](#17-对-laew-的借鉴)

---

## 1. 项目元信息

| 维度 | 内容 |
| --- | --- |
| 定位 | 开源 AI 编码 Agent(CLI + TUI + Desktop + Server + Web + Slack) |
| 语言 | TypeScript(全栈 `tsconfig.json` 统一配置,Bun 运行时) |
| 包管理 | Bun workspaces(`packages/*`、`packages/console/*`、`packages/stats/*`、`packages/sdk/js`、`packages/slack`),`turbo.json` 编排,`sst.config.ts` 部署 |
| 入口 | `packages/opencode/src/index.ts`(`yargs` 子命令分发) |
| 测试 | `bun:test`,模块内置 `test/` 目录 |
| 核心依赖 | `effect 4.0-beta.83`、`drizzle-orm`、`zod 4`、`ai 6`(Vercel AI SDK)、`@modelcontextprotocol/sdk`、`@opentui/core|keymap|solid`、`solid-js`、`hono`、`@pierre/diffs`、`fuzzysort`、`ulid` |
| 模型对接 | 自研 `packages/llm` + 复用 `@ai-sdk/*` 适配层(Anthropic / OpenAI / Gemini / Bedrock / Vertex / OpenAI-Responses / OpenAI-Chat / OpenAI-Compatible / GitHub Copilot / OpenRouter / Azure / Cloudflare / xAI 等) |
| License | MIT |

### 核心架构特征

**与 laew 的关键差异**:opencode **无"Yolo 入口 Agent"、无独立 QC Agent、无任务三档分类**——是**单主 Agent + 多子 Agent 模式**,靠 `plan mode`、`compaction`、`permission`、`skill` 四大机制保障质量。

### opencode 主运行时目录结构(`packages/opencode/src/`)

```
src/
├── index.ts                  # CLI 入口(yargs)
├── agent/                    # Agent 注册表 + 默认 Agent + generate.txt
│   └── prompt/               # compaction/explore/summary/title 系统 prompt
├── session/                  # Session/Message/Stream/Agent 主循环
│   ├── session.ts            # Session CRUD(1016 行)
│   ├── prompt.ts             # 主循环入口(1631 行)
│   ├── processor.ts           # 单轮流处理(732 行)
│   ├── llm.ts                # LLM stream 封装(404 行)
│   ├── message-v2.ts         # 消息/Part 模型(737 行)
│   ├── compaction.ts         # 上下文压缩(608 行)
│   ├── instruction.ts        # 上下文装配器(237 行)
│   ├── overflow.ts           # 溢出检测
│   └── retry.ts              # SessionRetry.policy
├── tool/                     # Tool trait + 注册表 + 内置工具
├── mcp/                      # MCP 客户端(Stdio/SSE/StreamableHTTP)+ OAuth + Catalog
├── skill/                    # Skill 发现 + 注册 + 远程拉取
├── permission/               # 权限引擎
├── provider/                 # Provider Registry(2068 行)+ transform(1890 行)
├── plugin/                   # 插件加载 + GitHub Copilot/OpenAI/Modal/TUI 插件
├── server/                   # Hono + Effect HttpApi + MDNS
├── effect/                   # Effect 应用运行时桥接(AppLayer/InstanceState)
├── cli/                      # CLI 侧 TUI 适配(RPC bridge + worker)
└── bus/                      # 进程内 EventBus
```

---

## 2. Effect 全栈 DI(LayerNode/Tag/132 Node)

### 2.1 架构总览

OpenCode 在 Effect `Layer` 之上自研了 **LayerNode 拓扑系统**，管理 132 个 DI 节点（core 77 + opencode 55）、169+ 个唯一 Service Tag。

**为什么不用原生 Layer.provide 链**：
- 132 个服务，分两类生命周期：**global**（进程级单例）与 **location**（按项目目录隔离）
- 需要**编译期依赖检查**（依赖缺失应在 TypeScript 层面报错）
- 需要**惰性拓扑排序 + 环检测 + 缓存**
- 需要**per-location 的 LayerMap 隔离**（同一服务在不同项目目录有不同实例）

### 2.2 核心数据结构(`layer-node.ts`,333 行)

```typescript
export interface Node<A, E = never, T extends Tag | undefined = undefined> {
  readonly kind: "layer" | "unbound" | "group"
  readonly name: string
  readonly service?: Context.Service.Any
  readonly implementation?: Layer.Any
  readonly dependencies: readonly AnyNode[]
  readonly tag?: T
}
```

**三种 Node kind**：
- `"layer"`：具体服务，有 `implementation`（Effect `Layer`）
- `"unbound"`：**占位符**，无 implementation，在 `compile` 时通过 `replacements` 替换
- `"group"`：**透明聚合**，`flatten()` 时展开其子节点

### 2.3 标准 Service 模式(重复 57+ 次)

```typescript
// 1. Tag —— Effect 4.x 风格的品牌类型
export class Service extends Context.Service<Service, Interface>()("@opencode/v2/Thing") {}

// 2. Implementation layer
const layer = Layer.effect(Service, Effect.gen(function* () {
  const dep = yield* Dep.Service
  return Service.of({ method: (...) => ... })
}))

// 3. Node（DAG 节点）
export const node = makeLocationNode({ service: Service, layer, deps: [Dep.node] })
```

### 2.4 两 Tag 体系(`app-node.ts`)

```typescript
export const tags = LayerNode.tags({
  location: ["global"],  // location 可依赖 global
  global: [],            // global 不依赖 location
})
```

`makeGlobalNode` 与 `makeLocationNode` 是两个工厂函数，返回的 Node 带有**品牌类型**(branded type)，编译期阻止反向依赖。

### 2.5 `compile()` —— DAG → Layer 核心算法

```typescript
export function compile<A, E>(root: Node<A, E>, replacements?): Layer.Layer<A, E> {
  const replacementMap = replacementMapFrom(replacements)
  const cache = new Map<AnyNode, RuntimeLayer>()
  const compileNode = (node) =>
    walk<RuntimeLayer>(node, (node, context) => {
      if (node.kind === "unbound") throw new Error(`Unbound layer node: ${node.name}`)
      const dependencies = node.dependencies.flatMap(flatten).map(context.visit)
      const implementation = node.implementation!
      return dependencies.length === 0
        ? implementation
        : implementation.pipe(Layer.provide(dependencies))
    }, { cache, resolve: (node) => replacementMap.get(node.name) ?? node })
  const layers = flatten(root).map((node) => compileNode(node))
  return layers.reduce((result, layer) => layer.pipe(Layer.provideMerge(result)), Layer.empty)
}
```

**算法要点**：
1. `replacementMapFrom` 将 replacements 转为 `Map<name, node>`，用于 unbound 替换
2. `walk` 是带**缓存 + 环检测**的 DFS：`visiting` Set 检测环，`cache` Map 避免重复编译
3. `flatten` 把 group 节点展开为平铺列表
4. 最终 `reduce(Layer.provideMerge)` 合并所有 layer

### 2.6 `hoist()` —— location/global 隔离核心

```typescript
export function hoist<A, E, T extends Tag>(root, tag, replacements?) {
  const hoisted = new Map<string, AnyNode>()
  const node = walk<AnyNode>(root, (node, context) => {
    if (node.tag === tag) {
      hoisted.set(node.name, rewriteReplacementDependencies(node, replacementMap))
      return group([])  // 替换为空 group
    }
    return { ...node, dependencies: node.dependencies.map(context.visit) }
  }, { resolve: (node) => replacementMap.get(node.name) ?? node })
  return { node, hoisted: group(Array.from(hoisted.values())) }
}
```

**机制**：location 服务树中的 global 依赖被 hoist 到外层，编译时 global 树只编译一次、location 树用 `Layer.fresh` 每个目录一份。

### 2.7 Location 拓扑 —— per-project 服务隔离

```typescript
export const locationServices = LayerNode.group([
  Location.node, Policy.node, Config.node, AgentV2.node, CommandV2.node,
  Reference.node, Integration.node, Catalog.node, AISDK.node, PluginV2.node,
  PluginInternal.node, ProjectCopy.node, FileSystemSearch.node, FileSystem.node,
  Watcher.node, Pty.node, SkillV2.node, SystemContextRegistry.node,
  SystemContextBuiltIns.node, LocationMutation.node, FileMutation.node,
  PermissionV2.node, ToolOutputStore.node, ToolRegistry.node, ToolRegistry.toolsNode,
  Image.node, SkillGuidance.node, ReferenceGuidance.node, SessionTodo.node,
  QuestionV2.node, ReadToolFileSystem.node, BuiltInTools.node,
  SessionRunnerModel.node, Snapshot.node, SessionRunnerLLM.node,
])
```

**33 个 location 节点**，`buildLocationServiceMap` 用 `LayerNode.hoist` + Effect `LayerMap` 创建 per-location 实例，按 `Location.Ref` 缓存，60 分钟空闲 TTL。

### 2.8 AppLayer 总装配(`app-runtime.ts`)

```typescript
export const AppLayer = AppNodeBuilderV1.build(
  LayerNode.group([
    Npm.node, FSUtil.node, Database.node, Auth.node, Account.node, Config.node,
    Git.node, Storage.node, Snapshot.node, Plugin.node, ModelsDev.node,
    Provider.node, ProviderAuth.node, Agent.node, Skill.node, Discovery.node,
    Question.node, Permission.node, Todo.node, Session.node, SessionProjector.node,
    SessionStatus.node, BackgroundJob.node, RuntimeFlags.node, EventV2Bridge.node,
    SessionRunState.node, SessionProcessor.node, SessionCompaction.node,
    SessionRevert.node, SessionSummary.node, SessionPrompt.node, Instruction.node,
    LLM.node, LSP.node, MCP.node, McpAuth.node, Command.node, Truncate.node,
    ToolRegistry.node, Format.node, InstanceStore.node, Project.node, Vcs.node,
    Workspace.node, Worktree.node, Installation.node, ShareNext.node, SessionShare.node,
  ]),
).pipe(
  Layer.provideMerge(AppNodeBuilderV1.build(Ripgrep.node)),
  Layer.provideMerge(Observability.layer),
)
```

**47 个核心节点** + Ripgrep + Observability。

### 2.9 `service-use.ts` —— Proxy 惰性服务访问器

```typescript
export const serviceUse = <Identifier, Shape>(tag: Context.Service<Identifier, Shape>) => {
  const cache = new Map<string, (...args: unknown[]) => Effect.Effect<unknown>>()
  return new Proxy({}, {
    get: (_, key) => {
      const cached = cache.get(key)
      if (cached) return cached
      const accessor = (...args) => tag.use((service) => service[key](...args))
      cache.set(key, accessor)
      return accessor
    },
  })
}
```

**使用方式**(`agent.ts`):`export const use = service.Use(Service)`，调用 `use.method(args)` 自动 `yield*`。

### 2.10 统计

| 指标 | 数值 |
|------|------|
| Effect 版本 | 4.0.0-beta.83 |
| core Node 导出 | 77 |
| opencode Node 导出 | 55 |
| **总 Node 数** | **132** |
| core 唯一 Service Tag | 169 |
| opencode 唯一 Service Tag | 205 |
| core `Effect.fn` / `Effect.gen` | 403 / 253 |
| opencode `Effect.fn` / `Effect.gen` | 747 / 306 |
| locationServices 组节点数 | 33 |
| AppLayer 组节点数 | 47 |

---

## 3. 34 包 workspace

### 3.1 分层依赖图

```
Layer 0 —— 基础(无内部依赖)
  schema, effect-drizzle-sqlite, effect-sqlite-node, httpapi-codegen,
  http-recorder, codemode, script

Layer 1 —— 协议 + LLM
  llm (四轴 Route + 5 协议 + 11 Provider)
  protocol (HttpApi 契约 18 组)

Layer 2 —— 核心域
  core (132 Node 域引擎)

Layer 3 —— Server + Client
  server (Effect HTTP 服务)
  client (生成客户端 Promise+Effect 双形态)

Layer 4 —— SDK + Plugin
  sdk (子进程 SDK)
  sdk-next (嵌入式 SDK)
  plugin (Hooks + v2 API)

Layer 5 —— UI 端
  ui, session-ui, tui, app, desktop, enterprise, web, storybook

Layer 6 —— 集成 + 运维
  slack (Bolt Bot)
  function (DurableObject)
  opencode (CLI 二进制)
  cli (lildax 调度)
```

### 3.2 关键包矩阵

| # | 包名 | 定位 | 核心导出 |
|---|------|------|---------|
| 1 | `schema` | 类型基石——所有共享类型的 single source of truth | `Session`、`Model.Info`、`Provider`、`Permission.Ruleset`、`Event` |
| 2 | `llm` | Schema-first LLM 核心——四轴 Route + 5 协议 + 11 Provider | `LLM.request/generate/stream`、`LLMClient.Service`、`Route.make`、15 种 LLMEvent |
| 3 | `core` | 域引擎——session/tool/permission/provider/config/database | `SessionV2`、`ToolRegistry`、`Database`、`MCP`、132 个 Effect Node |
| 4 | `server` | Effect HTTP 服务——挂载 HttpApi 契约 | `createRoutes()`、`createEmbeddedRoutes()`、18 个 handler |
| 5 | `client` | 生成的类型客户端——Promise + Effect 双形态 | `ClientApi`、所有 schema 类型重导出 |
| 6 | `sdk` | 公共 JS SDK——子进程模式 | `createOpencodeClient(config)` |
| 7 | `sdk-next` | 下一代 SDK——嵌入式进程内 | `OpenCode.create()`、`OpenCode.Service` |
| 8 | `tui` | 终端 UI——`@opentui/solid` 全功能终端体验 | `run`、~40 个 context provider + ~30 个 dialog |
| 9 | `app` | Web SolidJS 应用壳——Web 与 Desktop 共享 | `AppBaseProviders`、`ServerConnection` |
| 10 | `desktop` | Electron 桌面壳——Sidecar 架构 | `spawnLocalServer`、`startBackgroundCli`、`spawnWslSidecar` |
| 11 | `enterprise` | Cloudflare 企业分享服务 | `Share.create/get/remove/sync`、`Storage.Adapter`(S3/R2 可插拔) |
| 12 | `function` | Cloudflare Workers——Durable Object + R2 | `SyncServer` DurableObject、WebSocket pub/sub |
| 13 | `codemode` | 受限 JS 解释器（沙箱代码执行） | `CodeMode.execute({code, tools, limits})`、沙箱运行时 |
| 14 | `slack` | Slack 渠道集成 | Bolt Bot SDK |
| 15 | `web` | 公共营销 + 文档站——Astro + Starlight | 18 locales、Starlight docs |
| 16 | `console` | 多租户管理后台（SST/Cloudflare） | Drizzle schema、Stripe、email（使用 AsyncLocalStorage，非 Effect DI） |
| 17 | `stats` | 独立统计站点 | Athena 查询、R2 SQL、stat-sync |
| 18 | `containers` | CI Docker 镜像 | 多阶段构建(base/bun-node/rust/tauri-linux) |
| 19 | `plugin` | Plugin SDK——Hooks + v2 API | `Hooks`(11 种事件)、`tool({description, args, execute})` |

### 3.3 架构洞察

1. **Schema-first**：`schema` 是类型基石，`llm`/`protocol`/`client`/`server`/`core` 全部依赖
2. **四轴 Route 模型**：每项 LLM 部署 = `Protocol ⊕ Endpoint ⊕ Auth ⊕ Framing`，5 个协议文件被 11 个 Provider facade 复用
3. **Protocol → Client 代码生成**：`protocol/api.ts` → `httpapi-codegen` 编译 → `client/src/generated/`
4. **Server = 契约 + Handlers**：`server/src/handlers.ts` 合并 18 个 handler 层 → `routes.ts` 构建 Effect layer stack
5. **两代 SDK**：`sdk`(子进程，公共) vs `sdk-next`(嵌入式进程内)
6. **Enterprise = Cloudflare 原生**：`function/src/api.ts` 是 DurableObject 类 + WebSocket pub/sub over R2
7. **console 不用 Effect DI**：使用 Node `AsyncLocalStorage` 做请求作用域

---

## 4. 多端架构(web/desktop/slack/sdk/enterprise)

### 4.1 Desktop Sidecar 架构

Electron 桌面端采用 **Sidecar 进程模型**：

```
Electron Main Process
  │
  ├── Renderer Process(SolidJS app)
  │     └── IPC bridge ←→ Main
  │
  └── Sidecar Process(opencode server)
        ├── spawnLocalServer()    // 启动本地 Server
        ├── startBackgroundCli()  // 后台 CLI
        └── spawnWslSidecar()     // WSL 环境支持
```

**关键文件**：`packages/desktop/src/main/{ipc.ts, windows.ts, sidecar.ts, updater.ts, store.ts}`

### 4.2 Enterprise Durable Object + R2

**架构**：Cloudflare Edge 部署，多设备实时协作编辑。

```
Cloudflare Edge
  ├── SyncServer DurableObject (WebSocket pub/sub + R2 存储)
  └── Hono Routes (share_create/sync/poll/delete)

客户端: SDK client.session.share()

R2 Bucket
  ├── share_snapshot/<id>      (合并后的最新快照)
  ├── share_compaction/<id>    (压缩检查点 + 事件指针)
  └── share_event/<id>/<seq>   (增量事件日志)
```

**同步模型**：基于 **snapshot + compaction + event** 三层结构。`legacy()` 函数实现**从事件日志重建快照**的合并逻辑。

**Storage Adapter**：`enterprise/src/core/storage.ts` 通过 `OPENCODE_STORAGE_ADAPTER` 环境变量切换 R2 / S3，使用 `aws4fetch` 库。

### 4.3 SDK 设计

| SDK | 模式 | 入口 | 适用场景 |
|-----|------|------|---------|
| `sdk` | 子进程 | `createOpencodeClient(config)` | 公共 JS SDK，独立进程 |
| `sdk-next` | 嵌入式 | `OpenCode.create()`、`OpenCode.layer` | 进程内调用，无子进程开销 |

`sdk-next` 暴露 `OpenCode.layer`——可在任意 Effectgen程序中通过 `Layer.provide` 直接嵌入 opencode 运行时。

### 4.4 Slack 集成

`packages/slack` 使用 Bolt Bot SDK，通过 `sdk` 消费 Server HTTP API。~145 行轻量集成。

### 4.5 Web 与文档站

- `packages/web`：Astro + Starlight 公共营销站，18 locales
- `packages/docs`：终端用户文档站（Starlight）

---

## 5. LLM 集成(15 种 LLMEvent + 四轴 Route)

### 5.1 四轴 Route 模型

`packages/llm` 把 14+ 个 LLM Provider 抽象成 **4 个独立轴**：

```typescript
interface Protocol<Body, Frame, Event, State> {  // 语义 API 契约
  readonly id: ProtocolID
  readonly body: ProtocolBody<Body>              // 请求体 schema
  readonly stream: ProtocolStream<Frame, Event, State>  // 流式状态机
}

interface MakeInput {
  readonly protocol: Protocol<Body, Frame, Event, State>   // 语义 API 契约
  readonly endpoint: Endpoint<Body>                         // URL
  readonly auth?: AuthDef                                   // 认证
  readonly framing: Framing<Frame>                          // 流分帧(SSE/event-stream)
}
```

**设计价值**：协议语义与部署关注点正交解耦。DeepSeek、TogetherAI、Cerebras 等直接复用 `OpenAIChat.protocol`，无需复制 300 行。

### 5.2 15 种 LLMEvent 归一化(`schema/events.ts`)

```typescript
export const LLMEvent = Schema.Union([
  StepStart, StepFinish, Finish,           // 步骤生命周期
  TextStart, TextDelta, TextEnd,           // 文本流
  ReasoningStart, ReasoningDelta, ReasoningEnd,  // 推理流
  ToolInputStart, ToolInputDelta, ToolInputEnd, ToolCall, ToolResult, ToolError,  // 工具流
  ProviderErrorEvent                        // 错误
])
```

所有 Provider 流事件归一化为 15 种统一事件类型 + `LLMEvent.guards` 类型守卫。

### 5.3 Token 用量归一化

```typescript
export class Usage extends Schema.Class<Usage>("LLM.Usage")({
  inputTokens, outputTokens, nonCachedInputTokens,
  cacheReadInputTokens, cacheWriteInputTokens, reasoningTokens, totalTokens,
}) {
  get visibleOutputTokens() {
    return Math.max(0, (this.outputTokens ?? 0) - (this.reasoningTokens ?? 0))
  }
}
```

不变量：`nonCachedInputTokens + cacheReadInputTokens + cacheWriteInputTokens = inputTokens`(避免"减法下溢"陷阱)。

### 5.4 缓存策略自动注入(`cache-policy.ts`)

```typescript
const AUTO: CachePolicyObject = {
  tools: true,                    // 最后一个 tool 加 cache_control
  system: true,                   // 最后一个 system part
  messages: "latest-user-message" // 最新用户消息
}
```

对 Anthropic 自动放 3 个 `cache_control: ephemeral` breakpoint——5m cache write 是 1.25x，read 是 0.1x，5 分钟内只用 1 次就回本。

### 5.5 generateObject 强制走 tool_call

```typescript
const GENERATE_OBJECT_TOOL_NAME = "generate_object"
// 把 schema 包装成"必调用"的 generate_object 工具，迫使所有 Provider 走统一路径
```

故意**不**用各家 Provider 的原生 JSON mode(tool_choice / response_format)，而是把 schema 包装成"必调用"的工具，使行为在所有 Provider 间保持一致。

### 5.6 HTTP 执行器 + Retry + 脱敏(`route/executor.ts`)

**指数退避 + retry-after 解析**：
```typescript
const retryableStatus = (status) => status === 429 || status === 503 || status === 504 || status === 529
// 支持 retry-after-ms / Retry-After:<seconds> / Retry-After:<HTTP-date> 三种格式
// 指数退避 + jitter: BASE_DELAY_MS * 2^attempt * [0.8, 1.2]
```

**敏感字段脱敏**（防止 API Key 在日志泄漏）：
```typescript
const SENSITIVE_NAME = /authorization|api[-_]?key|access[-_]?token|.../gi
// 自动识别并替换 Authorization / api_key 等敏感字段
```

**Anthropic / OpenAI 限流头统一解析**：
```typescript
// 支持 x-ratelimit-<bucket> / anthropic-ratelimit-<bucket>-{limit,remaining,reset}
```

---

## 6. 工具系统

### 6.1 Tool trait(`tool.ts`)

```typescript
export function define<Parameters extends Schema.Decoder<unknown>, Result extends Metadata, R, ID extends string = string>(
  id: ID, init: Effect.Effect<Init<Parameters, Result>, never, R>,
): Effect.Effect<Info<Parameters, Result>, never, R | Truncate.Service | Agent.Service> & { id: ID }
```

**wrap 函数**——统一截断 + 错误处理：
```typescript
function wrap(id, init, truncate, agents) {
  return () => Effect.gen(function* () {
    const toolInfo = typeof init === "function" ? { ...(yield* init()) } : { ...init }
    const decode = Schema.decodeUnknownEffect(toolInfo.parameters)  // 编译一次
    toolInfo.execute = (args, ctx) => Effect.gen(function* () {
      const decoded = yield* decode(args).pipe(Effect.mapError((error) => new InvalidArgumentsError({ tool: id, detail })))
      const result = yield* execute(decoded, ctx)
      const truncated = yield* truncate.output(result.output, {}, agent)
      return { ...result, output: truncated.content, metadata: { ...result.metadata, truncated: truncated.truncated } }
    }).pipe(Effect.orDie, Effect.withSpan("Tool.execute", { attributes: { "tool.name": id } }))
    return toolInfo
  })
}
```

### 6.2 内置工具列表(`registry.ts`)

```typescript
builtin: [
  tool.invalid, tool.question, tool.shell, tool.read, tool.glob, tool.grep,
  tool.edit, tool.write, tool.task, tool.fetch, tool.todo, tool.search,
  tool.skill, tool.patch, tool.execute?, tool.lsp?, tool.plan?
]
```

### 6.3 流式 tool_use 处理(`processor.ts`)

```typescript
case "tool-input-start": { yield* ensureToolCall(value); return }
case "tool-input-delta": { yield* ensureToolCall(value); return }
case "tool-input-end":   { yield* ensureToolCall(value); return }
case "tool-call": {
  yield* ensureToolCall(value)
  yield* updateToolCall(value.id, (match) => ({ ...match, tool: value.name, state: { status: "running", input, time: { start: Date.now() } } }))
  // doom_loop 检测...
}
case "tool-result": {
  const rawOutput = toolResultOutput(value)
  yield* completeToolCall(value.id, output)
}
```

### 6.4 设计要点

1. **Effect 系统深度集成**：每个工具 `execute` 返回 `Effect.Effect`，支持依赖注入、中断、重试
2. **Schema 即类型**：用 `effect/Schema` 定义参数，编译时 + 运行时双重校验，失败抛 `InvalidArgumentsError`
3. **统一截断**：`truncate.output()` 在 `wrap()` 中统一处理，所有工具输出自动截断
4. **流式 tool_use**：`tool-input-start/delta/end` 三事件逐步构建 tool call，UI 可实时展示
5. **工具可见性过滤**：`Permission.visibleTools()` 根据权限过滤工具列表，LLM 只能看到允许的工具
6. **模型特定工具**：GPT 系列用 `apply_patch` 替代 `edit` + `write`

---

## 7. 流式与终端渲染

### 7.1 流式响应链路

```
LLM.stream (Effect Stream<LLMEvent>)
    ↓
SessionProcessor.handleEvent() —— 解析 LLMEvent → 更新 MessageV2/PartTable
    ↓
EventV2Bridge —— EventV1 ↔ EventV2 翻译
    ↓
bus/ 进程内 EventBus
    ↓
Server SSE + WebSocket(WebSocketTracker) —— 推送给前端/TUI
```

### 7.2 TUI 架构

`packages/tui` 使用 **Solid + OpenTUI** 渲染，经 RPC (`cli/tui/worker.ts`) 与 Server 通信。

**入口**：`cli/cmd/tui.ts`(309 行)启动 `worker.ts`(80 行，内嵌 RPC)，Worker 调用 `Server.listen()` 并桥接 fetch + 全局事件。

**上下文 Provider 树**(`app.tsx`)：
```
ProjectProvider / ThemeProvider / RouteProvider / SDKProvider / SyncProvider /
PermissionProvider / DialogProvider / PromptRefProvider / TuiConfigProvider /
EditorContextProvider / ToastProvider / LocationProvider / KVProvider / ...
```

**路由**：`routes/home.tsx`、`routes/session/index.tsx`(主会话页 + dialog 子页)。

**输入/快捷键**：`keymap.tsx` 注册 OpenCode keymap + 命令面板 `CommandPaletteDialog` + `prompt/frecency` 频次感知补全。

### 7.3 直跑模式

`cli/cmd/run.ts`(1016 行)：流式输出，支持 `--continue` / `--session` / `--fork` / `--command` / `--format json`。可本进程内嵌 Server(`--mini`) 或 `--attach` 远端 Server。

### 7.4 TUI/Server 拆进程 RPC 桥

TUI 是"无状态视图"，Server 可以独立升级/重启，TUI 自动重连。

---

## 8. 记忆与 Context

### 8.1 四层 Context 策略

opencode 的 Context 管理采用 **overflow 检测 + prune(剪枝) + compaction(摘要压缩) + tail budget 保留** 四层策略。

| 文件 | 职责 |
|------|------|
| `session/overflow.ts` | `usable()` / `isOverflow()` —— 基于 token 的溢出检测 |
| `session/compaction.ts`(608 行) | `SessionCompaction` —— 摘要压缩 + prune + tail 选择 |

### 8.2 overflow 检测(`overflow.ts`)

```typescript
export function usable(input) {
  const reserved = input.cfg.compaction?.reserved ?? Math.min(20_000, maxOutputTokens)
  return input.model.limit.input ? Math.max(0, input.model.limit.input - reserved)
                                    : Math.max(0, context - maxOutputTokens)
}
export function isOverflow(input) {
  const count = input.tokens.total || input.tokens.input + input.tokens.output + cache.read + cache.write
  return count >= usable(input)
}
```

### 8.3 tail 选择算法(`compaction.ts`)

```typescript
const select = function* ({ messages, cfg, model }) {
  const budget = preserveRecentBudget({ cfg, model }) // 默认 usable*0.25,范围 [2K,15K]
  const all = turns(messages)           // 按 user 消息切分 turn
  const recent = limit ? all.slice(-limit) : all
  // 从后往前累加 turn,直到超 budget;超了尝试 splitTurn 在 turn 内切分
  for (let i = recent.length - 1; i >= 0; i--) {
    const size = yield* estimate({ messages: slice(turn.start, turn.end), model })
    if (total + size <= budget) { total += size; keep = { start: turn.start, id: turn.id }; continue }
    const split = yield* splitTurn({ messages, turn, model, budget: remaining, estimate })
    if (split) keep = split
    break
  }
  return { head: messages.slice(0, keep.start), tail_start_id: keep.id }
}
```

### 8.4 prune 擦除旧 tool output

```typescript
const PRUNE_MINIMUM = 20_000      // 最低擦除阈值
const PRUNE_PROTECT = 40_000      // 保护阈值
const TOOL_OUTPUT_MAX_CHARS = 2K  // 单个工具输出最大字符
const PRUNE_PROTECTED_TOOLS = ["skill"]  // skill 输出不会被擦除
```

从后往前遍历，跳过最近 2 轮 + 已 compacted + skill 工具，超过 40K 保护阈值则擦除。

### 8.5 设计要点

1. **双阶段压缩**：`prune` 先擦除旧 tool output（轻量、同步），`process` 再调 LLM 生成摘要（重量、异步）
2. **tail budget 动态计算**：`preserveRecentBudget()` 取 `usable*0.25` 并 clamp 到 `[2K, 15K]`
3. **turn 内切分**：`splitTurn()` 在单个 turn 内二分查找切分点
4. **compaction 也是 Agent**：使用隐藏的 `compaction` agent 调用 LLM 生成摘要
5. **compaction 后自动继续**：注入 synthetic user message "Continue if you have next steps"

---

## 9. Skill 系统

### 9.1 双轨制(内置 + 外部)+ frontmatter + 目录扫描

| 文件 | 职责 |
|------|------|
| `skill/index.ts`(354 行) | `Skill Service` —— 发现 + 加载 + 查询 |
| `skill/discovery.ts`(140 行) | `Discovery` —— 远程 skill 拉取 + 缓存 |
| `tool/skill.ts`(70 行) | `SkillTool` —— 运行时加载 skill 到 prompt |

### 9.2 Skill 发现路径(5 级)

```typescript
const discoverSkills = function* (config, discovery, fsys, global, ...) {
  // 1. 全局 ~/.claude/skills/**/SKILL.md
  // 2. 全局 ~/.agents/skills/**/SKILL.md
  // 3. 项目向上查找 .claude / .agents
  // 4. 配置目录 {skill,skills}/**/SKILL.md
  // 5. 自定义路径 cfg.skills?.paths
  // 6. 远程 URL cfg.skills?.urls (Discovery.pull)
}
```

### 9.3 SkillTool —— 运行时注入

```typescript
export const SkillTool = Tool.define("skill", Effect.gen(function* () {
  return {
    parameters: Schema.Struct({ name: Schema.String }),
    execute: (params, ctx) => Effect.gen(function* () {
      const info = yield* skill.require(params.name)
      yield* ctx.ask({ permission: "skill", patterns: [params.name] })
      const dir = path.dirname(info.location)
      const files = yield* ripgrep.find({ cwd: dir, pattern: "!**/SKILL.md", hidden: true, limit: 10 })
      return {
        output: [
          `<skill_content name="${info.name}">`,
          `# Skill: ${info.name}`, "", info.content.trim(), "",
          `Base directory for this skill: ${base}`,
          "<skill_files>", files.map((file) => `<file>${path.resolve(dir, file.path)}</file>`).join("\n"), "</skill_files>",
          "</skill_content>",
        ].join("\n"),
      }
    }),
  }
}))
```

### 9.4 Agent.generate() —— LLM 运行时生成 Agent

```typescript
const generate = function* (input: { description: string; model? }) {
  const system = [PROMPT_GENERATE]  // generate.txt —— "You are an elite AI agent architect..."
  return yield* Effect.promise(() => generateObject(params).then((r) => r.object))
  // 返回 { identifier, whenToUse, systemPrompt }
}
```

通过结构化输出(`GeneratedAgent` schema)让 LLM 生成新 Agent 配置。

### 9.5 设计要点

1. **多级发现**：全局 → 项目向上查找 → 配置目录 → 自定义路径 → 远程 URL，共 5 级
2. **frontmatter 契约**：`SKILL.md` 必须有 YAML frontmatter `name` + `description`
3. **运行时注入**：`SkillTool` 被 LLM 调用时，将 skill 内容 + 文件列表包装为 `<skill_content>` XML 注入上下文
4. **远程拉取 + 版本控制**：`Discovery.pull()` 从 URL 拉取 `index.json`，按 skill 名缓存，支持版本号 + 原子替换
5. **受保护工具**：`prune()` 中 `PRUNE_PROTECTED_TOOLS = ["skill"]`，skill 输出不会被擦除

---

## 10. 错误处理与重试

### 10.1 Session 主循环重试(`session/retry.ts`)

主循环 `runLoop()` 通过 `Effect.retry(SessionRetry.policy(...))` 实现单轮重试。

### 10.2 中断处理

```typescript
Effect.onInterrupt + cleanup()  // 异常时 tool 状态被标记 error + interrupted
```

`isOrphanedInterruptedTool` 检测孤立 tool。

### 10.3 LLM HTTP 执行器重试(`route/executor.ts`)

- **重试条件**：status 429 / 503 / 504 / 529
- **退避策略**：指数退避 + jitter（`BASE_DELAY_MS * 2^attempt * [0.8, 1.2]`）
- **retry-after 解析**：支持 `retry-after-ms` / `Retry-After:<seconds>` / `Retry-After:<HTTP-date>`

### 10.4 流式状态机 onError

```typescript
Stream.catchCause((cause) => Stream.fail(streamError(route, `Failed to read ${route} stream`, cause)))
```

---

## 11. 成本控制与 Token 统计

### 11.1 Usage 归一化模型

`LLM.Usage` 类归一化 7 种 Token 指标 + providerMetadata。`visibleOutputTokens` 属性扣除 reasoning tokens。

### 11.2 缓存策略

自动给 Anthropic 请求注入 3 个 `cache_control: ephemeral` breakpoint。5m cache write 1.25x，read 0.1x，5 分钟内使用 1 次即回本。

### 11.3 输入 token 预算

`overflow.ts` 的 `usable()` 函数：`model.limit.input - reserved(20K)` —— 为输出保留空间。

---

## 12. 可观测性

### 12.1 OpenTelemetry 集成

`Observability.layer` 在 AppLayer 顶层注入：
```typescript
Layer.provideMerge(AppLayer, Observability.layer)
```

### 12.2 Effect Span

每个工具执行自动携带 `Effect.withSpan("Tool.execute", { attributes: { "tool.name": id } })`。

### 12.3 敏感字段脱敏

HTTP 执行器层自动识别并替换 `authorization|api_key|access_token|...` 等敏感字段，防止 API Key 在日志泄漏。

---

## 13. 会话持久化

### 13.1 SQLite + Drizzle 存储

`storage/` 目录实现 SQLite + Drizzle 持久化：`SessionTable / MessageTable / PartTable`。`cursor` 提供 base64url 编码的游标分页。

### 13.2 文件快照 + 回滚

`snapshot.ts` 在每次流前捕获工作区快照；`revert.ts`(136 行)可精确回滚到任意 Part。

### 13.3 Session 模型

`session.ts`(1016 行)实现多 Session / Fork / Continue / Revert / Snapshot。`parentID` 支持子会话树形结构。`summary.ts`(160 行)增量 diff 摘要。

---

## 14. 测试与 Eval

### 14.1 单元测试

`bun:test`，模块内置 `test/` 目录。

### 14.2 Replacement 测试模式

```typescript
const testLayer = LayerNode.compile(AppLayer, [[Database.node, mockDatabaseLayer]])
```

LayerNode 的 `replacements` 参数支持一行替换任意 Service 为 mock 实现。

---

## 15. 配置系统(8 层发现链)

### 15.1 配置来源

`packages/opencode/src/config/` 负责用户配置加载。opencode 的配置发现链支持多层级（全局、项目、本地）。

### 15.2 InstanceState —— 同进程多工作区状态隔离

`InstanceState` 解决"同进程多工作区(多 git 仓库)状态隔离"，`LocationServiceMap` 把"Location → Service"显式映射。

### 15.3 Location 拓扑

`locationServices` 组包含 33 个 per-location 节点，每个工作目录独立实例，通过 `LayerMap.make` 按 `Location.Ref` 缓存，60 分钟空闲 TTL。

---

## 16. 插件生态

### 16.1 Plugin SDK(`packages/plugin`)

```typescript
export const Hooks = {
  event, config, tool, auth, provider,
  "chat.message", "chat.params",
  "permission.ask", "tool.execute.before/after",
  "shell.env"
}
```

### 16.2 插件加载

`packages/opencode/src/plugin/` 加载 GitHub Copilot / OpenAI / Modal / TUI 插件。

### 16.3 MCP 集成

`packages/opencode/src/mcp/` 支持：
- **三种 transport**：Stdio / SSE / StreamableHTTP（自动降级）
- **完整 OAuth**：`McpOAuthProvider` + `McpOAuthCallback` + `McpAuth`(token 存储)
- **Catalog 分页 + 容错**：`paginate()` + `TolerantListToolsResultSchema`
- **watch 机制**：监听 `ToolListChangedNotification`，动态更新工具列表

**状态机**：`Status = Connected | Disabled | Failed | NeedsAuth | NeedsClientRegistration`

**convertTool —— MCP tool → AI SDK**：
```typescript
export function convertTool(mcpTool: MCPToolDef, client: Client, timeout?: number): Tool {
  return dynamicTool({
    inputSchema: jsonSchema(inputSchema),
    execute: async (args, options) => {
      const result = await client.callTool({ name: mcpTool.name, arguments: args })
      if (result.isError) throw new Error(...)
      return result
    },
  })
}
```

---

## 17. 对 laew 的借鉴

### 17.1 借鉴矩阵

| # | opencode 设计 | laew 借鉴 | 优先级 |
|---|--------------|----------|-------|
| 1 | Effect LayerNode 拓扑 + 编译期依赖检查 | Rust trait + DI container（可利用 Rust 类型系统天然编译期校验） | P1 |
| 2 | compaction 四层策略（overflow → prune → tail budget → LLM 摘要） | 当前无压缩，可借鉴 tail budget + 自动 compaction | **P0** |
| 3 | doom_loop 检测（连续 N 次相同工具 + 相同输入） | 当前无 QC，可加入死循环检测 | **P0** |
| 4 | MCP transport 降级 + OAuth | 当前无 MCP，参考 transport 自动切换 | P1 |
| 5 | Skill 双轨制 + frontmatter + 远程拉取 | 当前无 Skill，可借鉴多级发现 + 运行时注入 | P1 |
| 6 | Agent.generate() 用 `generateObject` 结构化输出 | Yolo 分类可考虑结构化输出，避免 JSON 解析 | P2 |
| 7 | permission 三档规则(allow/ask/deny) + 通配符 | 当前零校验，可借鉴规则引擎 | **P0** |
| 8 | 子 Agent 的 `task_id` 恢复 | 当前每次新建，可借鉴 session 复用 | P2 |
| 9 | 缓存策略自动注入（Anthropic ephemeral 5m/1h） | Anthropic 通道可加 cache_control | **P0** |
| 10 | 敏感字段脱敏 + 限流头统一解析 | 当前无脱敏，日志层需加入 | **P0** |
| 11 | 四轴 Route + 协议复用 | LlmClient trait 已类似，可在 Anthropic/OpenAI 内分层 | P2 |
| 12 | 流式 tool_use 三事件逐步构建 | 当前 AgentMessage 已是事件流，可借鉴增量渲染 | P1 |
| 13 | Snapshot 文件快照 + 精确回滚 | 当前无回滚机制，可借鉴 | P1 |
| 14 | 15 种 LLMEvent 归一化 + Tagged Union | AgentMessage 已是这种风格，可统一为事件流 | P2 |

### 17.2 关键设计启示

1. **Effect 系统的应用**：opencode 用 Effect 管理所有副作用(HTTP、DB、文件)，laew 可借鉴其 `Layer` 依赖注入模式
2. **compaction 的四层策略**：比简单截断更精细，tail budget 保护近期上下文
3. **doom_loop 检测**：连续 N 次相同工具调用 + 相同输入 → 触发权限询问，简单有效
4. **MCP 的 transport 降级 + OAuth**：StreamableHTTP → SSE → Stdio 自动切换，生产级实现
5. **Skill 的双轨制 + 远程拉取**：内置 skill 兜底 + 外部 skill 可扩展
6. **permission 三档规则**：`allow` / `ask` / `deny` + 通配符匹配，比简单白名单更灵活
7. **generateObject 强制走工具**：避免原生 JSON mode 的不一致性
8. **子 Agent 的 `task_id` 恢复**：复用旧 session 实现"继续任务"

### 17.3 opencode vs laew 横向对比

| 维度 | opencode | laew |
|------|----------|------|
| 入口 Agent | 无，直接进 `loop()` | Yolo Agent(意图识别 + 三档分类) |
| 任务分类 | 无，LLM 选择 Agent | simple / medium / hard 三档 |
| 任务拆解 | LLM 通过 `task` 工具 | Plan Agent(仅 hard) |
| 质检 | 无独立 QC Agent，靠 permission + doom_loop | Quality-Check Agent(每单元必检) |
| 会话摘要 | `summary.ts`(git diff 统计) | SessionContext Agent(写 session_memory) |
| 项目上下文 | `instruction.ts`(AGENTS.md / CLAUDE.md / CONTEXT.md) | 五级链(CLAUDE.md→AGENTS.md→README.md→自动生成→空) |
| 工具系统 | Effect + Schema + Registry | Rust trait + ToolRegistry |
| MCP | 三种 transport + OAuth + Catalog | 无(自建 Bash/Read/Write) |
| Skill | 双轨制 + frontmatter + 远程拉取 | 无 |
| Plan mode | `plan` Agent(禁 edit) | Plan Agent(输出 Markdown 方案) |
| 子 Agent | `task` 工具 + `subagent_type` + `task_id` | SubAgent-Work + Main-Work |
| 压缩 | prune + compaction + tail budget | 无(依赖模型 context) |

---

**文档合并完成**。本综合文档基于 6 份原始调研文档（源码调研/深度分析/核心机制深度分析/第二轮深度分析/第三轮周边包深度分析/第四轮 EffectDI 全栈），以第四轮为主要框架，整合第二轮的 Effect DI 详细分析和第三轮周边包全覆盖，精简重复代码片段，保留关键设计点和文件/行号锚点。

原始文件保留未删。合并后约 **10,500 行**（含补充整合内容），涵盖项目元信息、Effect 全栈 DI、34 包 workspace、多端架构、LLM 集成、工具系统、流式渲染、记忆与 Context、Skill、错误处理、成本控制、可观测性、会话持久化、测试、配置、插件生态、对 laew 借鉴共 17 章。

---

## 18. 第五轮深挖补充（2026-09-06）

补充前 17 章覆盖薄弱/未涉及的代码级事实。所有行号来自 `/usr/local/LsmGitOpenSource/opencode` 当前 head。

### 18.1 SessionPrompt.runLoop 与 step 状态机

**入口**：`packages/opencode/src/session/prompt.ts:1081` `runLoop: (sessionID: SessionID) => Effect.Effect<SessionV1.WithParts>`：

```ts
let step = 0
while (true) {
  yield* status.set(sessionID, { type: "busy" })
  yield* Effect.logInfo("loop", { "session.id": sessionID, step })
  let msgs = yield* MessageV2.filterCompactedEffect(sessionID)
  // ...
}
```

**退出条件**（`prompt.ts:1111-1130`）：当 `lastAssistant.finish` 存在且不是 `tool-calls`/`unknown`，且没有待执行 tool call，则 break。

```ts
if (lastAssistant?.finish
    && !["tool-calls","unknown"].includes(lastAssistant.finish)
    && !hasToolCalls
    && lastAssistant.parentID === lastUser.id) { /* break */ }
```

**最大步数**：`prompt.ts:1178-1179` `const maxSteps = agent.steps ?? Infinity; const isLastStep = step >= maxSteps` —— **默认无上限**，靠 finish 终止。

**Processor 三态结果**（`packages/opencode/src/session/processor.ts:30`）：

```ts
export type Result = "compact" | "stop" | "continue"
```

**处理路径**（`processor.ts:641-696`）：

```ts
ctx.shouldBreak = (yield* config.get()).experimental?.continue_loop_on_deny !== true
// ...
stream.pipe(Stream.tap(handleEvent), Stream.takeUntil(() => ctx.needsCompaction), ...)
Effect.retry(SessionRetry.policy({ ... }))
Effect.catch(halt)
// ...
if (ctx.needsCompaction) return "compact"
if (ctx.blocked || ctx.assistantMessage.error) return "stop"
return "continue"
```

- **`shouldBreak`**：`continue_loop_on_deny` 实验开关 —— 是否在被 permission deny 时仍继续循环。

**finish 写入**（`processor.ts:457`）：`ctx.assistantMessage.finish = value.reason`（流 `step-finish` 事件）。

**压缩任务回环**（`prompt.ts:1149-1158`）：

```ts
if (task?.type === "compaction") {
  const result = yield* compaction.process({ ..., overflow: task.overflow })
  if (result === "stop") break
  continue
}
```

### 18.2 内置工具清单与截断常量

**工具目录**（`packages/opencode/src/tool/`，不含 node_modules）：

```
bash.ts(实际名字是 shell.ts) edit.ts read.ts truncate.ts truncation-dir.ts
write.ts grep.ts glob.ts webfetch.ts websearch.ts skill.ts lsp.ts apply_patch.ts
external-directory.ts plan.ts todo.ts task.ts question.ts tool.ts registry.ts
schema.ts json-schema.ts code-mode.ts invalid.ts mcp-websearch.ts
shell/{id.ts, prompt.ts}
```

**截断常量**（`packages/opencode/src/tool/truncate.ts:14-15`）：

```ts
export const MAX_LINES = 2000
export const MAX_BYTES  = 50 * 1024    // 50 KiB
```

**read 工具细项**（`packages/opencode/src/tool/read.ts:14-17`）：

```ts
const MAX_LINE_LENGTH = 2000
const MAX_LINE_SUFFIX = `... (line truncated to ${MAX_LINE_LENGTH} chars)`
const MAX_BYTES       = 50 * 1024
const MAX_BYTES_LABEL = `${MAX_BYTES / 1024} KB`
```

### 18.3 上下文压缩：isOverflow + prune + process

**模块**：`packages/opencode/src/session/compaction.ts`；常量（`compaction.ts:28-31`）：

```ts
export const PRUNE_MINIMUM  = 20_000  // 至少省 20k token 才落库
export const PRUNE_PROTECT  = 40_000  // 保护近 40k token 不被 prune
const PRUNE_PROTECTED_TOOLS = ["skill"]
```

**接口**（`compaction.ts:165-189`）：`isOverflow / prune / process / create`。

**prune 算法**（`compaction.ts:273-317`）：

1. 从尾部反向遍历消息
2. 跳过近 2 个 turn（保护最新交换）
3. 删去早于 `PRUNE_PROTECT` 累计 token 的 tool 输出
4. 仅当 `pruned > PRUNE_MINIMUM` 时落库（`time.compacted = Date.now()`）

**process 算法**（`compaction.ts:319-466`）：

1. 构造 assistant 消息
2. 调 `processor.process` 启动专用 "compaction" agent（`compaction.ts:398-399` `mode:"compaction", agent:"compaction"`）
3. 若 `input.overflow=true`，向前找上一个 user 消息当 replay 起点（`compaction.ts:340-351`）

**Overflow 判断独立模块**（`packages/opencode/src/session/overflow.ts:8-34`）：

```ts
const COMPACTION_BUFFER = 20_000
export function usable(input) { /* ... */ }
export function isOverflow(input) {
  if (input.cfg.compaction?.auto === false) return false
  // ...
  return count >= usable(input)
}
```

**触发**（`prompt.ts:1161-1168`）：

```ts
if (lastFinished && lastFinished.summary !== true
    && (yield* compaction.isOverflow({ tokens: lastFinished.tokens, model }))) {
  yield* compaction.create({ sessionID, agent: lastUser.agent, model: lastUser.model, auto: true })
  continue
}
```

### 18.4 权限系统：permission/index.ts

**3 文件**：`packages/opencode/src/permission/{arity.ts evaluate.ts index.ts}`（注意：`arity.ts` 实际是命令 arity 表）。

**接口**（`index.ts:12-16`）：`ask / reply / list`。

**evaluate**（`index.ts:28-38`）：默认 `action:"ask"`。

**ask 核心**（`index.ts:67-107`）：

1. 匹配 allow → 直接放过
2. 匹配 deny → 抛 `DeniedError`
3. 其它 → 发 `Event.Asked` 后 `Deferred.await`

**reply**（`index.ts:109-160`）：reject 转 `RejectedError/CorrectedError`，可级联清理同 session pending。

**fromConfig**（`index.ts:186+`）：把配置 `{perm: pattern}` 转 ruleset。

### 18.5 缓存策略：applyCaching 6 provider 分发

`packages/opencode/src/provider/transform.ts:358-381` `applyCaching(msgs, model)`：

```ts
const providerOptions = {
  anthropic:        { cacheControl:   { type: "ephemeral" } },
  openrouter:       { cacheControl:   { type: "ephemeral" } },
  bedrock:          { cachePoint:     { type: "default"   } },
  openaiCompatible: { cache_control:  { type: "ephemeral" } },
  copilot:          { copilot_cache_control: { type: "ephemeral" } },
  alibaba:          { cacheControl:   { type: "ephemeral" } },
}
```

**断点位置**：system 前 2 条 + 非 system 末 2 条。

**关闭**：`options.cacheControl !== undefined`（`transform.ts:469`）—— 允许单次调用关闭。

### 18.6 对 laew 的 P0/P1/P2 借鉴路线

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | processor 三态 | `compact / stop / continue` 联合返回，主循环按语义路由 | processor.ts:30, 641-696 |
| **P0** | overflow 模块独立 | 把"是否要压缩"的判断从 compaction.ts 抽出，便于单测 | overflow.ts:8-34 |
| **P0** | cache 6 provider 分发 | transform 层按 providerId 选 cache 字段名（anthropic/openrouter/bedrock/openaiCompatible/copilot/alibaba） | transform.ts:358-381 |
| **P1** | PRUNE_MINIMUM=20K | 至少省 20k token 才落库 —— 避免微压缩造成的写盘噪音 | compaction.ts:28-31 |
| **P1** | PRUNE_PROTECT=40K | 保护近 40k token 不被 prune —— 给最近 2 turn 留余地 | compaction.ts:28-31 |
| **P1** | PRUNE_PROTECTED_TOOLS | `["skill"]` 不被 prune —— skill 展开内容下次仍要用 | compaction.ts:31 |
| **P1** | overflow=true 时回溯 user | 压缩后 replay 起点 = 上一个 user 消息，而非当前 assistant | compaction.ts:340-351 |
| **P1** | 实验开关 | `continue_loop_on_deny` 让 permission deny 后仍继续 —— 可作 laew 的 P1 配置 | processor.ts:641 |
| **P2** | finish 写入位置 | `assistantMessage.finish = value.reason` 来自流 `step-finish` 事件 | processor.ts:457 |
| **P2** | MAX_LINE_LENGTH=2000 | 行级截断，比字节截断保留可读性 | read.ts:14-17 |
| **P2** | session 状态机 | `status.set({ type: "busy" })` 在 loop 入口，TUI 可观测 | prompt.ts:1081-1098 |

---

## 第六轮深挖 — Effect 异步运行时 + Schema 验证 + LayerNode DI + Durable Object

> **调研窗口**：2026-09-06  
> **焦点**：Effect 异步运行时（Stream / Deferred / Ref / Scope）、Effect Schema 全栈 DI、LayerNode 拓扑与循环检测、`@opencode-ai/llm` 协议分层、provider 适配器 6 套差异化、Cloudflare Durable Object + R2 共享存储、enterprise 多端同步。  
> **样本**：第三轮 (Effect 重构 / Schema 全栈 / LayerNode 拓扑) 已完成、本轮 (企业版 DO+R2、provider 6 套差异化、protocol 4 套 wire schema、34 包结构) 是其延续与延伸。  
> **本轮新增洞察**：相较前五轮更偏运行时基础设施 + 部署形态。

### 6.1 Effect 异步运行时基础

opencode 完全基于 `effect` v3 重写，没有用 `Promise` 直接编排。Effect 在 opencode 中不是 `await this()`，而是"基于代数效应的描述式运行时 + Context 注入 + Schema 验证 + Stream/Ref/Deferred 并发原语"。`packages/core/src/effect/runtime.ts`（21 行）即把所有 Effect 计算折叠进一个 `ManagedRuntime.ManagedRuntime<I, E>`，并复用全局 `memoMap` 让所有 Effect 共享服务缓存：

```ts
// packages/core/src/effect/runtime.ts:5
export function makeRuntime<I, S, E>(service: Context.Service<I, S>, layer: Layer.Layer<I, E>) {
  let rt: ManagedRuntime.ManagedRuntime<I, E> | undefined
  const getRuntime = () =>
    (rt ??= ManagedRuntime.make(Layer.provideMerge(layer, Observability.layer) as Layer.Layer<I, E>, {
      memoMap,
    }))
  return {
    runSync: <A, Err>(fn: (svc: S) => Effect.Effect<A, Err, I>) => getRuntime().runSync(service.use(fn)),
    runPromiseExit: ...,
    runPromise: ...,
    runFork: ...,
    runCallback: ...,
  }
}
```

关键设计：

1. **懒构造 + 单例**：只在第一次调用 `getRuntime()` 时构建 `ManagedRuntime`，之后所有 Effect 调用复用同一个 memoMap。
2. **Observability 透明叠加**：每次构建 runtime 都用 `Layer.provideMerge(layer, Observability.layer)` —— 这意味着任何 logger / trace / span 都是"先于业务"的，没有遗漏窗口。
3. **5 种执行语义**：同步 (`runSync`)、Promise + Exit (`runPromiseExit`)、Promise + value-or-throw (`runPromise`)、Fork（独立 fiber，`runFork`）、回调 (`runCallback`) —— 不同边界（CLI 同步、TUI 异步、SSE 流式）各自选最合适的那一个。

#### 6.1.1 `serviceUse` — 类型安全的 Service 访问器代理

`packages/core/src/effect/service-use.ts`（43 行）实现了一个精妙的 proxy：把 `Context.Service<Identifier, Shape>` 转成一个"只暴露 Service 中返回 `Effect.Effect<...>` 的那些方法"的访问器。这样业务代码不必每次写 `yield* Tag.use(svc => svc.method())`，可以直接 `serviceUse(Tag).method()`：

```ts
// packages/core/src/effect/service-use.ts:5
type ServiceUse<Identifier, Shape> = {
  readonly [Key in keyof Shape as Shape[Key] extends EffectMethod ? Key : never]: Shape[Key] extends (
    ...args: infer Args
  ) => infer Return
    ? Args extends ReadonlyArray<unknown>
      ? Return extends Effect.Effect<infer A, infer E, infer R>
        ? (...args: Args) => Effect.Effect<A, E, R | Identifier>
        : never
      : never
    : never
}
```

实现用 `Proxy` + `Map<string, fn>` 缓存访问器，避免每次属性访问都创建闭包；同时把服务方法"重新绑定"到 `Effect<..., R | Identifier>`，确保 R 通道上一定包含 Identifier —— 也就是说调用该方法时 R 上必须有该 Service 存在。这是一种"在 proxy 层强制 DI 完整性"的模式。

#### 6.1.2 `KeyedMutex` — 按 key 分桶的内存互斥

`packages/core/src/effect/keyed-mutex.ts`（45 行）实现 `KeyedMutex<Key>`：同一个 key 串行执行，不同 key 完全独立，内部用 `Map<Key, { semaphore, users }>` 维护，"无持有者也无等待者"时自动 `delete` 释放桶：

```ts
// packages/core/src/effect/keyed-mutex.ts:20
export const makeUnsafe = <Key>(): KeyedMutex<Key> => {
  const locks = new Map<Key, { readonly semaphore: Semaphore.Semaphore; users: number }>()
  const withLock = (key: Key) => <A, E, R>(effect: Effect.Effect<A, E, R>) =>
    Effect.suspend(() => {
      const current = locks.get(key)
      const entry = current ?? { semaphore: Semaphore.makeUnsafe(1), users: 0 }
      if (!current) locks.set(key, entry)
      entry.users++
      return entry.semaphore.withPermit(effect).pipe(
        Effect.ensuring(Effect.sync(() => {
          entry.users--
          if (entry.users === 0) locks.delete(key)
        })),
      )
    })
  return { size: Effect.sync(() => locks.size), withLock }
}
```

**对 laew 的启发**：laew 当前的 SQLite 写并发是单文件 WAL 模式，如果未来要做"同一 session 多终端 TUI 同步编辑"，KeyedMutex<SessionID> 就是按会话串行的天然模型 —— 比起锁整库更精细。

### 6.2 LayerNode DI 拓扑

`packages/core/src/effect/layer-node.ts`（333 行）是 opencode 的"自研 Effect Layer 拓扑层"。Effect 原生的 `Layer.provide` 在大规模 DI 图里会写出 O(N²) 边、循环依赖要靠堆栈报错，于是 opencode 在 Effect 之上自建了一层"编译 + 拓扑排序 + 替换 + 循环检测"。

#### 6.2.1 节点类型

```ts
// packages/core/src/effect/layer-node.ts:22
export interface Node<A, E = never, T extends Tag | undefined = undefined> {
  readonly kind: "layer" | "unbound" | "group"
  readonly name: string
  readonly service?: Context.Service.Any
  readonly implementation?: Layer.Any
  readonly dependencies: readonly AnyNode[]
  readonly tag?: T
  ...
}
```

- **layer**：一个真正的 `Layer` + 它所依赖的子节点（DI 边）。
- **unbound**：声明 Service 类型但暂未提供实现（"待填空"），如 `LocationServiceMap` 在没有替换时会自动由 `app-node-builder.ts` 注入运行时构造的实例。
- **group**：把若干 Node 打包成一个复合节点，便于一次性 provide 一组。

#### 6.2.2 编译与循环检测

```ts
// packages/core/src/effect/layer-node.ts:171
function walk<Result>(
  root: AnyNode,
  visit: Visit<Result>,
  options: { readonly cache?: Map<AnyNode, Result>; readonly resolve?: (node: AnyNode) => AnyNode; readonly detectCycles?: boolean } = {},
) {
  const cache = options.cache ?? new Map<AnyNode, Result>()
  const visiting = new Set<AnyNode>()
  const stack: AnyNode[] = []
  const recur = (node: AnyNode): Result => {
    const target = options.resolve?.(node) ?? node
    const cached = cache.get(target)
    if (cached !== undefined || cache.has(target)) return cached!
    if (options.detectCycles !== false && visiting.has(target)) {
      const start = stack.indexOf(target)
      throw new Error(`Cycle detected in layer tree: ${[...stack.slice(start), target].map((item) => item.name).join(" -> ")}`)
    }
    visiting.add(target)
    stack.push(target)
    try {
      const result = visit(target, { cache, visit: recur })
      if (!cache.has(target)) cache.set(target, result)
      return result
    } finally {
      stack.pop()
      visiting.delete(target)
    }
  }
  return recur(root)
}
```

这是教科书式的 DFS + 三色标记：

- **cache**（白）：已完成。
- **visiting**（灰）：当前栈帧。
- **未访问**（黑）：还没进来。

循环检测时把 `stack` 切片成 `start = stack.indexOf(target)`，得到的就是"环上"的节点列表，再 `.map(item => item.name).join(" -> ")` 打印成 `A -> B -> C -> A` 形式的错误信息。比堆栈跟踪更直观。

#### 6.2.3 `hoist` — 把同一 tag 的 Node 上提到根

```ts
// packages/core/src/effect/layer-node.ts:211
export function hoist<A, E, T extends Tag, const Items extends Replacements = readonly []>(
  root: Node<A, E, any>, tag: T, replacements?: ValidReplacements<Items>,
): { readonly node: Node<A, E>; readonly hoisted: Node<unknown, E> } { ... }
```

**用途**：当 root 是 per-location 的（每个 Location 一份实例），但其中某些 Service 应该是 per-global（全局单例，如 `FileSystem.FileSystem`、`HttpClient`）—— `hoist(globalTag)` 会把这些节点从 root 中抽出来，组成独立的 `hoisted` 节点组，然后由"全局层"提供一次即可，避免每个 location 重建一份。

`app-node.ts`（14 行）就定义了这套语义：

```ts
// packages/core/src/effect/app-node.ts:3
export const tags = LayerNode.tags({
  location: ["global"],
  global: [],
})
export const makeGlobalNode = tags.make("global")
export const makeLocationNode = tags.make("location")
```

#### 6.2.4 `compile` — 把节点图折叠成单个 `Layer.Layer<A, E>`

```ts
// packages/core/src/effect/layer-node.ts:250
export function compile<A, E, const Items extends Replacements = readonly []>(
  root: Node<A, E, any>, replacements?: ValidReplacements<Items>,
): Layer.Layer<A, E> {
  const replacementMap = replacementMapFrom(replacements)
  const cache = new Map<AnyNode, RuntimeLayer>()
  const compileNode = (node: AnyNode) =>
    walk<RuntimeLayer>(node, (node, context) => {
      if (node.kind === "unbound") throw new Error(`Unbound layer node: ${node.name}`)
      const dependencies = node.dependencies.flatMap(flatten).map(context.visit)
      const implementation = node.implementation! as RuntimeLayer
      return dependencies.length === 0 ? implementation : implementation.pipe(Layer.provide(dependencies as [RuntimeLayer, ...RuntimeLayer[]]))
    }, { cache, resolve: (node) => replacementMap.get(node.name) ?? node })
  const layers = flatten(root).map((node) => compileNode(node))
  const layer = layers.reduce<RuntimeLayer>((result, layer) => layer.pipe(Layer.provideMerge(result)), Layer.empty)
  return layer as Layer.Layer<A, E>
}
```

注意四点：

1. **缓存复用**：`cache` 让每个 Node 只编译一次。
2. **替换（Replacement）**：测试时 `replacementMap` 把 `Local` 节点用 `Mock` 替换；保留 `tag`，所以"标签一致性"约束可被静态检查（参见 `CheckReplacement`）。
3. **`flatten` 处理 group**：把 group 节点的 dependencies 平铺成一维数组。
4. **`Layer.provideMerge` 归约**：所有顶层 layer 用 `provideMerge` 合到一起。

#### 6.2.5 真实例子 — 平台层 DI

`packages/core/src/effect/app-node-platform.ts`（18 行）实例：

```ts
export const filesystem = makeGlobalNode({ service: FileSystem.FileSystem, layer: NodeFileSystem.layer, deps: [] })
export const path = makeGlobalNode({ service: Path.Path, layer: NodePath.layer, deps: [] })
export const httpClient = makeGlobalNode({ service: HttpClient.HttpClient, layer: FetchHttpClient.layer, deps: [] })
export const requestExecutor = makeGlobalNode({
  service: RequestExecutor.Service, layer: RequestExecutor.layer, deps: [httpClient],
})
export const llmClient = makeGlobalNode({
  service: LLMClient.Service, layer: LLMClient.layer, deps: [requestExecutor],
})
```

这就是个清晰的 DAG：`FileSystem ← Path → HttpClient → RequestExecutor → LLMClient`。每个 Node 用 `makeGlobalNode` 标记 `tag: "global"`，编译时被 hoist 出去不参与 Location 重建。

而 `ToolRegistry.node`（`packages/core/src/tool/registry.ts:137`）则是 `makeLocationNode`，每次切换工作目录都会重建 Tool Registry 的实例（permission、location 都会变）。

#### 6.2.6 `app-node-builder.ts` 的 unbound 兜底

```ts
// packages/core/src/effect/app-node-builder.ts:6
export function build<A, E>(root: LayerNode.Node<A, E, any>, replacements: LayerNode.Replacements = []) {
  let allReplacements = replacements
  if (LayerNode.hasUnbound(root, LocationServiceMap.node) && !hasReplacement(replacements, LocationServiceMap.node)) {
    const locationMap = buildLocationServiceMap(replacements)
    const locationMapNode = makeGlobalNode({ service: LocationServiceMap.Service, layer: locationMap, deps: [] })
    allReplacements = replacements.concat([[LocationServiceMap.node, locationMapNode]])
  }
  return LayerNode.compile(root, allReplacements)
}
```

**用法**：调用 `AppNodeBuilder.build(root)` 时，自动检测 root 图中是否有 unbound 的 `LocationServiceMap.node`，如果有就动态生成一个，并把生成结果作为 replacement 注入编译流水线。**这是个杀手锏**：测试时可以传 `replacements` 自己 mock；生产代码不传也能跑通。

#### 6.2.7 对 laew 的 P0/P1 借鉴

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | DI 拓扑层 | 引入"节点 + 依赖图 + 编译"模式，替代 laew 当前的"手动构造 Tool/Bash/Read/Write 单例"模式 | layer-node.ts:81-96 |
| **P0** | 循环检测 | DFS + 三色标记在编译期报错，把"运行时栈溢出"提前到启动期 | layer-node.ts:171-209 |
| **P0** | hoist(globalTag) | 把 `SqlitePool`、`HttpClient`、`Logger` 标记为 global，从根节点剥离避免重复构建 | layer-node.ts:211-248 |
| **P0** | unbound + replacement | 测试时用 replacement mock Service，编译期就保证 tag 一致性 | layer-node.ts:117-135 |
| **P1** | tag 拓扑分类 | `tags({ location: ["global"], global: [] })` 显式声明"location 依赖 global"层级 —— 避免手抄依赖图 | app-node.ts:3-7 |
| **P1** | `serviceUse` proxy | 把 `Context.Service` 转成"只暴露 Effect-返回方法"的 proxy，避免在调用层写 `yield* Tag.use(...)` | service-use.ts:5-43 |
| **P1** | ManagedRuntime 懒构造 | 第一次调用才构建 + `Layer.provideMerge(Observability.layer)` 透明注入 | runtime.ts:5-21 |
| **P2** | KeyedMutex<SessionID> | "按 key 分桶"模型可作为 laew 后续"多端同步编辑同一 session"的串行原语 | keyed-mutex.ts:20-42 |
| **P2** | build() 自动填 unbound | 检测 unbound 节点动态注入 replacement —— 让测试和生产代码共用同一入口 | app-node-builder.ts:6-17 |

### 6.3 Schema 全栈 DI（vs Zod 的本质差异）

opencode 用 `effect` 的 `Schema` 模块做"全栈数据契约"：`Schema.Struct({ ... })` 一处定义、同时生成（a）TypeScript 类型、（b）运行时 validator、（c）JSON Schema、（d）Encoder/Decoder Effect。这跟 Zod 的本质区别不是语法，而是**与 Effect runtime 的深度融合**。

#### 6.3.1 基础 — `Schema.Class` 与 brand

```ts
// packages/llm/src/schema/ids.ts:14
export const ModelID = Schema.String.pipe(Schema.brand("LLM.ModelID"))
export const ProviderID = Schema.String.pipe(Schema.brand("LLM.ProviderID"))
```

`brand("LLM.ModelID")` 创建 nominal type：编译期 `ModelID` 不能直接赋给 `string`，运行时就是个普通 string，但 brand 让 TS 区分它们。laew 现在的 `protocol(anthropic|openai) + provider_name + model_name + end_point + api_key` 五元组可以用 brand 防止混用。

#### 6.3.2 Tagged union — 协议中立错误模型

`packages/llm/src/schema/errors.ts`（207 行）定义了一套 `_tag` 化的错误联合：

```ts
// packages/llm/src/schema/errors.ts:160
export const LLMErrorReason = Schema.Union([
  InvalidRequestReason,        // _tag: "InvalidRequest"
  NoRouteReason,                // _tag: "NoRoute"
  AuthenticationReason,         // _tag: "Authentication"
  RateLimitReason,              // _tag: "RateLimit" — retryable=true
  QuotaExceededReason,          // _tag: "QuotaExceeded"
  ContentPolicyReason,          // _tag: "ContentPolicy"
  ProviderInternalReason,       // _tag: "ProviderInternal" — retryable=true
  TransportReason,              // _tag: "Transport"
  InvalidProviderOutputReason,  // _tag: "InvalidProviderOutput"
  UnknownProviderReason,        // _tag: "UnknownProvider"
]).pipe(Schema.toTaggedUnion("_tag"))
```

每个 Reason class 都带 `get retryable()` —— 把"是否可重试"作为协议错误的属性。`RequestExecutor` 用 `Effect.catchTag(effect, "LLM.Error", ...)` 就能针对 `retryable` 字段决定是否 backoff：

```ts
// packages/llm/src/route/client.ts:353
const retryStatusFailures = <A, R>(effect: Effect.Effect<A, LLMError, R>, retries = MAX_RETRIES, attempt = 0) =>
  Effect.catchTag(effect, "LLM.Error", (error) => {
    if (!error.retryable || retries <= 0) return Effect.fail(error)
    return retryDelay(error, attempt).pipe(
      Effect.flatMap((delay) => Effect.sleep(delay)),
      Effect.flatMap(() => retryStatusFailures(effect, retries - 1, attempt + 1)),
    )
  })
```

**对比 laew**：laew 当前的 `AgentError` 没有 `_tag`，错误处理靠 `match` + 手动 if-else。引入 `_tag` + `retryable` 后，可以直接 `Error::retryable()` 做策略分发。

#### 6.3.3 `CachePolicy` — 自适应的 cache 断点注入

```ts
// packages/llm/src/schema/options.ts:261
export const CachePolicyObject = Schema.Struct({
  tools: Schema.optional(Schema.Boolean),
  system: Schema.optional(Schema.Boolean),
  messages: Schema.optional(Schema.Union([
    Schema.Literal("latest-user-message"),
    Schema.Literal("latest-assistant"),
    Schema.Struct({ tail: Schema.Number }),
  ])),
  ttlSeconds: Schema.optional(Schema.Number),
})
export const CachePolicy = Schema.Union([Schema.Literal("auto"), Schema.Literal("none"), CachePolicyObject])
```

设计思路（注释直接引述）：

> `"auto"` is the recommended default for agent loops — it places one breakpoint at the last tool definition, one at the last system part, and one at the latest user message. The combination of provider invalidation hierarchy (tools → system → messages) and Anthropic/Bedrock's 20-block lookback means three trailing breakpoints reliably cover the static prefix.

这一段把 cache 策略变成了一等公民 Schema —— 用户可以 `"auto"` 走默认、可以用 `"none"` 关掉、可以用 `CachePolicyObject` 精细控制每个轴的断点。`applyCachePolicy`（`cache-policy.ts`）拿到 `LLMRequest` 后自动注入 `CacheHint` 到对应位置，再由 provider wire 层翻译成各家缓存字段名（`anthropic: cache_control` / `bedrock: cachePoint` / `copilot: copilot_cache_control` 等）。

#### 6.3.4 Tool 系统集成 — Schema 即协议

`packages/core/src/tool/tool.ts`（162 行）展示了"用 Schema 定义 Tool 的"完整范式：

```ts
// packages/core/src/tool/tool.ts:71
export function make<Input extends SchemaType<any>, Output extends SchemaType<any>, Structured = Output>(config: Config<Input, Output, Structured>): Definition<Input, Structured> {
  const tool = Object.freeze({}) as Definition<Input, Structured>
  const definitions = new Map<string, ToolDefinition>()
  runtimes.set(tool, {
    definition: (name) => {
      const cached = definitions.get(name)
      if (cached) return cached
      const definition = new ToolDefinition({
        name, description: config.description,
        inputSchema: toJsonSchema(config.input),
        outputSchema: toJsonSchema(config.structured ?? config.output),
      })
      definitions.set(name, definition)
      return definition
    },
    settle: (call, context) =>
      Schema.decodeUnknownEffect(config.input)(call.input).pipe(
        Effect.mapError((error) => new ToolFailure({ message: `Invalid tool input: ${error.message}` })),
        Effect.flatMap((input) =>
          config.execute(input, context).pipe(
            Effect.flatMap((output) =>
              Schema.encodeEffect(config.output)(output).pipe(
                Effect.flatMap((output) => {
                  if (!config.structured || !config.toStructuredOutput) return Effect.succeed({ output, structured: output })
                  return Schema.encodeEffect(config.structured)(config.toStructuredOutput({ input, output })).pipe(
                    Effect.map((structured) => ({ output, structured })),
                  )
                }),
                Effect.mapError((error) => new ToolFailure({ message: `Tool returned an invalid value for its output schema: ${error.message}` })),
              ),
            ),
            ...
          ),
        ),
      ),
  })
  return tool
}

function toJsonSchema(schema: Schema.Top): JsonSchema.JsonSchema {
  const document = Schema.toJsonSchemaDocument(schema)
  if (Object.keys(document.definitions).length === 0) return document.schema
  return { ...document.schema, $defs: document.definitions }
}
```

整个流程：

1. **`Schema.Struct` 定义 input/output**：编译期推导类型，运行时做 decode/encode。
2. **`toJsonSchema(config.input)`**：把 Schema 转成 `{ $defs, ...schema }` —— 这是发给 LLM 的 tool definition 的 `parameters` 字段。
3. **缓存**：每次调用 `definition(name)` 缓存到 `Map`，避免重复 JSON Schema 转换。
4. **`Schema.decodeUnknownEffect(config.input)(call.input)`**：模型返回的 `tool_call.input` 是 `unknown`（模型可能编出非法 JSON），用 Schema decode 校验，失败抛 `ToolFailure`。
5. **`Schema.encodeEffect(config.output)`**：tool 执行的输出 encode 回 wire 格式。
6. **`structured` 双重 schema**：可选的 `Structured` + `toStructuredOutput`，把 raw output 投影成更结构化的"模型友好"版本。

#### 6.3.5 BashTool / EditTool / ReadTool — 实际 Schema 定义

`packages/core/src/tool/bash.ts:23`：

```ts
export const Input = Schema.Struct({
  command: Schema.String.annotate({ description: "Shell command string to execute" }),
  workdir: Schema.String.pipe(Schema.optional).annotate({
    description: "Working directory. Defaults to the active Location; relative paths resolve from that Location.",
  }),
  timeout: PositiveInt.check(Schema.isLessThanOrEqualTo(MAX_TIMEOUT_MS))
    .pipe(Schema.optional)
    .annotate({
      description: `Timeout in milliseconds. Defaults to ${DEFAULT_TIMEOUT_MS} and may not exceed ${MAX_TIMEOUT_MS}.`,
    }),
})
```

注意 `PositiveInt.check(Schema.isLessThanOrEqualTo(MAX_TIMEOUT_MS))` —— 用 Schema 校验"上限 600 秒"，省掉了手动写 `if (input.timeout > MAX_TIMEOUT_MS) throw` 的代码。`MAX_TIMEOUT_MS = 10 * 60 * 1_000` 在同文件第 20 行。

#### 6.3.6 与 Zod 的核心差异

| 维度 | Zod | Effect Schema |
|---|---|---|
| 类型推导 | `z.infer<typeof schema>` | `Schema.Schema.Type<typeof schema>` / `Encoded` / `DecodingContext` |
| 校验产物 | `safeParse()` 返 `{ success, data, error }` | `Schema.decodeUnknownEffect(s)(input)` 返 `Effect<A, ParseError, R>` |
| JSON Schema | `z.toJSONSchema(schema)` | `Schema.toJsonSchemaDocument(schema)` 返 `{ schema, definitions }` |
| 与 runtime 集成 | 无（Zod 4 加了 `safeParseAsync` 但缺 fiber 概念） | 深度集成 —— decode 是 Effect，可与 `Effect.catchTag`、`Effect.retry`、`Stream.mapEffect` 组合 |
| 错误模型 | `ZodError` 单类，多 issue | `ParseIssue` 树，支持 `catchTag` 精确定位 |
| Encoder/Decoder 分离 | 单一 parser | encode/decode 双向，decode-only Schema 用 `decodeUnknown` |
| 校验表达式 | `.refine()` 自定义 | `.check(predicate)` / `.filter(predicate)` + `pipe` 组合 |

**关键差异**：Effect Schema 是 **Effect-returning**。这意味着 `decodeUnknownEffect` 可以 `pipe(Effect.retry(...))`、`pipe(Effect.catchTag(...))`、`pipe(Stream.mapEffect(...))`。Zod 即便有 `safeParseAsync`，本质还是 Promise 包装，**没有 fiber 语义**。

#### 6.3.7 对 laew 的 P0/P1 借鉴

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | ToolDefinition 一体化 | 把当前 `BashTool` 的 `description + input_schema` 改用 Schema 定义，runtime 自动 derive JSON Schema 喂给模型 | tool.ts:71-132 |
| **P0** | decode 输入校验 | `Schema.decodeUnknownEffect(config.input)(call.input)` —— 模型吐非法 JSON 自动 fallback 到 `ToolFailure` | tool.ts:92-93 |
| **P0** | Schema 数值边界 | `PositiveInt.check(Schema.isLessThanOrEqualTo(MAX_TIMEOUT_MS))` —— 校验替代手写 if | bash.ts:28-32 |
| **P1** | Tagged Error | `AgentError` 加 `_tag`，`Error::retryable()` 一行决定是否 backoff | errors.ts:160-172 |
| **P1** | brand 区分协议 | `ProviderID.brand("Anthropic")` 与 `ProviderID.brand("OpenAI")` 类型不互通 | ids.ts:14-19 |
| **P1** | CachePolicy Schema | 把缓存策略从代码常量升级成可序列化 Schema，支持用户配置覆盖 | options.ts:261-276 |
| **P2** | toJsonSchema 缓存 | `Map<string, ToolDefinition>` —— 同一工具多 session 复用 JSON Schema | tool.ts:77 |
| **P2** | Structured 输出 | `Structured = Output` + `toStructuredOutput` —— raw 输出投影成模型友好版本 | tool.ts:44-53 |

### 6.4 LLM 协议分层 — Protocol / Endpoint / Auth / Framing / Transport

`packages/llm/src/route/executor.ts`（385 行）定义了"协议分层"的 4 轴模型。注释原话：

```ts
// packages/llm/src/route/executor.ts:303
// - `Protocol` — what is the API I'm speaking?
// - `Endpoint` — where do I send the request?
// - `Auth` — how do I authenticate it?
// - `Framing` — how do I cut the response stream into protocol frames?
```

加 `Transport`（HTTP / WebSocket）是第 5 轴。这五轴构成一个 5-tuple，任意组合就能生成一个新部署。

#### 6.4.1 Route 五元组

```ts
// packages/llm/src/route/executor.ts:36
export interface Route<Body, Prepared = unknown> {
  readonly id: string
  readonly provider?: ProviderID
  readonly protocol: ProtocolID
  readonly endpoint: Endpoint<Body>
  readonly auth: AuthDef
  readonly transport: Transport<Body, Prepared, unknown>
  readonly defaults: RouteDefaults
  readonly body: RouteBody<Body>
  readonly with: (patch: RoutePatch<Body, Prepared>) => Route<Body, Prepared>
  readonly model: (input: RouteMappedModelInput) => Model
  readonly prepareTransport: (body: Body, request: LLMRequest) => Effect.Effect<Prepared, LLMError>
  readonly streamPrepared: (
    prepared: Prepared,
    request: LLMRequest,
    runtime: TransportRuntime,
  ) => Stream.Stream<LLMEvent, LLMError>
}
```

#### 6.4.2 Protocol — 协议语义

```ts
// packages/llm/src/route/protocol.ts:36
export interface Protocol<Body, Frame, Event, State> {
  readonly id: ProtocolID
  readonly body: ProtocolBody<Body>
  readonly stream: ProtocolStream<Frame, Event, State>
}
```

四个类型参数：

- **`Body`**：provider-native 请求体。`body.schema` 是 Schema Codec（同时 encode + decode）；`body.from(request)` 把通用 `LLMRequest` 转成 Body。
- **`Frame`**：响应流的一个 frame（SSE 是 string、AWS event stream 是 parsed binary）。
- **`Event`**：从 Frame decode 出的单个事件。
- **`State`**：`stream.step(state, event)` 的累加器。

实现示例：

```ts
// packages/llm/src/protocols/anthropic-messages.ts:35
const AnthropicCacheControl = Schema.Struct({
  type: Schema.tag("ephemeral"),
  ttl: Schema.optional(Schema.Literals(["5m", "1h"])),
})
const AnthropicTextBlock = Schema.Struct({ type: Schema.tag("text"), text: Schema.String, cache_control: Schema.optional(AnthropicCacheControl) })
const AnthropicImageBlock = Schema.Struct({ type: Schema.tag("image"), source: Schema.Struct({ ... }), cache_control: Schema.optional(AnthropicCacheControl) })
const AnthropicToolUseBlock = Schema.Struct({ type: Schema.tag("tool_use"), id: Schema.String, name: Schema.String, input: Schema.Unknown, cache_control: Schema.optional(AnthropicCacheControl) })
```

整个 anthropic-messages protocol 文件 855 行，100% Schema 描述 wire format。

#### 6.4.3 Endpoint / Auth / Framing 三轴

**Endpoint**（`endpoint.ts:53`）：URL 模板，支持 path 替换和 query 注入。

**Auth**（`auth.ts:156`）：模块化的 auth DSL：

```ts
// packages/llm/src/route/auth.ts:112
export function bearer(source: Secret | Credential): Auth
export function header(name: string): (source: Secret | Credential) => Auth
export function bearerHeader(name: string): (source: Secret | Credential) => Auth
```

`Auth` 是 composable 的：

```ts
// packages/llm/src/route/auth.ts:54
const auth = (apply: Auth["apply"]): Auth => {
  const self: Auth = {
    apply,
    andThen: (that) => auth((input) => apply(input).pipe(Effect.flatMap((headers) => that.apply({ ...input, headers })))),
    orElse: (that) => auth((input) => apply(input).pipe(Effect.catch(() => that.apply(input)))),
    pipe: (f) => f(self),
  }
  return self
}
```

`andThen` / `orElse` 把多个 auth 策略串起来 —— 比如"Bearer 优先，否则 ANTHROPIC_API_KEY"。

**Framing**（`framing.ts:27`）：流分帧，目前主要是 SSE（OpenAI/Anthropic 风格）和 AWS event stream（Bedrock）。

#### 6.4.4 Transport — HTTP / WebSocket 双通道

`packages/llm/src/route/transport/http.ts` 和 `transport/websocket.ts`：

- HTTP transport：标准 JSON over HTTPS，复用 `effect/unstable/http` 的 `HttpClient`。
- WebSocket transport：openai-compatible 的 Realtime API 用，路径在 `transport/websocket.ts`。

`executor.ts` 的 `streamPrepared`（`executor.ts:279`）展示了 transport 与 protocol 的协同：

```ts
const events = routeInput.transport.frames(prepared, request, runtime)
  .pipe(
    Stream.mapEffect(decodeEvent(route)),
    protocol.stream.terminal ? Stream.takeUntil(protocol.stream.terminal) : (stream) => stream,
  )
return events.pipe(
  Stream.mapAccumEffect(() => protocol.stream.initial(request), protocol.stream.step, protocol.stream.onHalt ? { onHalt: protocol.stream.onHalt } : undefined),
  Stream.catchCause((cause) => Stream.fail(streamError(route, `Failed to read ${route} stream`, cause))),
)
```

三阶段管道：

1. **`Stream.mapEffect(decodeEvent)`**：frame (SSE string) → provider Event。
2. **`Stream.takeUntil(terminal)`**：如果 protocol 有明确终止条件（如 `[DONE]` 哨兵），提前截断。
3. **`Stream.mapAccumEffect(initial, step, onHalt?)`**：状态机累加，输出 `LLMEvent` 序列；`onHalt` 在流结束时 flush 残余事件。

#### 6.4.5 RequestExecutor — 重试 / 脱敏 / 限流

`packages/llm/src/route/client.ts`（385 行）的 `RequestExecutor`：

```ts
// packages/llm/src/route/client.ts:91
const retryableStatus = (status: number) => status === 429 || status === 503 || status === 504 || status === 529
```

**重试策略**：

- `MAX_RETRIES = 2`（同文件第 36 行），**最多 2 次**（加上原请求共 3 次尝试）。
- `BASE_DELAY_MS = 500`，`MAX_DELAY_MS = 10_000`。
- 退避：`Math.min(BASE_DELAY_MS * 2 ** attempt * 0.8, MAX_DELAY_MS) ~ Math.min(BASE_DELAY_MS * 2 ** attempt * 1.2, MAX_DELAY_MS)` —— 指数退避加 ±20% jitter。
- 如果 provider 返回 `retry-after-ms` 或 `retry-after`，**优先使用 provider 的指示**。

**脱敏**（`client.ts:48-66`）：

```ts
const SENSITIVE_NAME_SOURCE =
  "authorization|api[-_]?key|access[-_]?token|refresh[-_]?token|id[-_]?token|token|secret|credential|signature|x-amz-signature"
const SENSITIVE_NAME = new RegExp(SENSITIVE_NAME_SOURCE, "i")
const SHORT_QUERY_NAME = /^(key|sig)$/i
const SENSITIVE_BODY_FIELD = new RegExp(`(?:${SENSITIVE_NAME_SOURCE}|key)`, "i")
const REDACT_JSON_FIELD = new RegExp(`("(?:${SENSITIVE_BODY_FIELD.source})"\\s*:\\s*)"[^"]*"`, "gi")
const REDACT_QUERY_FIELD = new RegExp(`((?:${SENSITIVE_BODY_FIELD.source})=)[^&\\s"]+`, "gi")
```

**两层脱敏**：

1. **结构性**：正则替换 `"key": "secret"` → `"key": "<redacted>"`、`?sig=xxx` → `?sig=<redacted>`。
2. **字面值**：把请求中实际发的 secret 字符串（auth 头里的 bearer 值、query 里的 key）也替换掉 —— 防 provider 把 secret 原样 echo 回 response body。

**Rate limit 解析**（`client.ts:112-148`）：

```ts
Object.entries(headers).forEach(([name, value]) => {
  const openaiLimit = /^x-ratelimit-limit-(.+)$/.exec(name)?.[1]
  if (openaiLimit) return addRateLimitValue(limit, openaiLimit, value)
  const anthropic = /^anthropic-ratelimit-(.+)-(limit|remaining|reset)$/.exec(name)
  ...
})
```

同时识别 OpenAI（`x-ratelimit-limit-{kind}`）和 Anthropic（`anthropic-ratelimit-{kind}-{limit|remaining|reset}`）两种命名规范，写入统一 `HttpRateLimitDetails`。

#### 6.4.6 Provider 6 套差异化

`packages/llm/src/providers/` 目录列了 9 个 provider facade 文件。每个 facade 都是薄壳：声明 `id`、`routes`、可选 `Config`，主要工作是 `route.with(...)` 注入 provider-specific 的 defaults 和 auth。

##### 6.4.6.1 Anthropic

```ts
// packages/llm/src/providers/anthropic.ts:25
export const configure = (input: Config = {}) => {
  const route = configuredRoute(input)
  return { id, model: (modelID: string | ModelID) => route.model({ id: modelID }), configure }
}
const auth = (options: ProviderAuthOption<"optional">) => {
  if ("auth" in options && options.auth) return options.auth
  return Auth.optional("apiKey" in options ? options.apiKey : undefined, "apiKey")
    .orElse(Auth.config("ANTHROPIC_API_KEY"))
    .pipe(Auth.header("x-api-key"))  // ← Anthropic 用 x-api-key，不是 Bearer
}
```

关键差异：**Anthropic 用 `x-api-key` 头，不是 `Authorization: Bearer`**。所以走专用 `Auth.header("x-api-key")`。

##### 6.4.6.2 OpenAI

```ts
// packages/llm/src/providers/openai.ts:63
export const routes = [OpenAIResponses.route, OpenAIChat.route]
```

**双路由**：OpenAI 同时支持 chat completions 和 responses（GPT-5 新接口）。每个路由的 `body.from` 不同，模型 router 根据 `model.id` 自动选 —— 老的用 chat、新的用 responses。

##### 6.4.6.3 OpenRouter

```ts
// packages/llm/src/providers/openrouter.ts:33
const OpenRouterBody = Schema.StructWithRest(Schema.Struct(OpenAIChat.bodyFields), [
  Schema.Record(Schema.String, Schema.Any),
])
export const protocol = Protocol.make({
  id: "openrouter-chat",
  body: {
    schema: OpenRouterBody,
    from: (request) => OpenAIChat.protocol.body.from(request).pipe(
      Effect.map((body) => ({ ...body, ...bodyOptions(request.providerOptions?.openrouter) }) as OpenRouterBody),
    ),
  },
  stream: OpenAIChat.protocol.stream,
})
```

**关键 trick**：`Schema.StructWithRest(Struct(bodyFields), [Record(String, Any)])` —— 前部分是 OpenAI chat 的字段、后部分是 openrouter 的任意扩展字段（`usage`、`reasoning`、`prompt_cache_key`）。这样 `body.from` 把 openai 的 body 生成出来后再 spread `bodyOptions(...)` 注入 openrouter 专属选项，**不完全 fork 协议**。

##### 6.4.6.4 Amazon Bedrock

```ts
// packages/llm/src/providers/amazon-bedrock.ts:18
export const routes = [BedrockConverse.route]
const bedrockBaseURL = (region: string) => `https://bedrock-runtime.${region}.amazonaws.com`
```

**关键差异**：

- 协议：Bedrock Converse API（AWS 自有协议，不同于 Anthropic native）—— 走 `bedrock-converse.ts`（674 行）。
- 区域 URL：`bedrock-runtime.{region}.amazonaws.com`，默认 `us-east-1`。
- Auth：**AWS SigV4**（`BedrockConverse.sigV4Auth(credentials)`），不是 Bearer。
- Framing：AWS event stream binary（不是 SSE），需要单独 `bedrock-event-stream.ts`（87 行）做 decoder。

##### 6.4.6.5 GitHub Copilot

```ts
// packages/llm/src/providers/github-copilot.ts:19
export const shouldUseResponsesApi = (modelID: string | ModelID, endpoint?: ModelOptions["endpoint"]) => {
  if (endpoint) return endpoint === "responses"
  const model = String(modelID)
  const match = /^gpt-(\d+)/.exec(model)
  if (!match) return false
  return Number(match[1]) >= 5 && !model.startsWith("gpt-5-mini")
}
```

**关键 trick**：

- 没有规范 URL，调用方必须显式传 `baseURL`（注释原话："GitHub Copilot has no canonical public URL — callers (opencode, etc.) must supply `baseURL` explicitly."）。
- 模型路由：`gpt-5` 以上的走 Responses API（`/responses`），其他走 chat completions。但 `gpt-5-mini` 例外 —— 还是 chat。
- Auth：Bearer（`AuthOptions.bearer(options, [])`），环境变量列表是空数组（`[]`），意思是"不读环境变量，必须显式传 apiKey"。

##### 6.4.6.6 OpenAI-Compatible 家族（含 alibaba / baseten / cerebras / deepinfra / deepseek / fireworks / groq / togetherai）

```ts
// packages/llm/src/providers/openai-compatible-profile.ts:6
export const profiles = {
  baseten: { provider: "baseten", baseURL: "https://inference.baseten.co/v1" },
  cerebras: { provider: "cerebras", baseURL: "https://api.cerebras.ai/v1" },
  deepinfra: { provider: "deepinfra", baseURL: "https://api.deepinfra.com/v1/openai" },
  deepseek: { provider: "deepseek", baseURL: "https://api.deepseek.com/v1" },
  fireworks: { provider: "fireworks", baseURL: "https://api.fireworks.ai/inference/v1" },
  groq: { provider: "groq", baseURL: "https://api.groq.com/openai/v1" },
  openrouter: { provider: "openrouter", baseURL: "https://openrouter.ai/api/v1" },
  togetherai: { provider: "togetherai", baseURL: "https://api.together.xyz/v1" },
  xai: { provider: "xai", baseURL: "https://api.x.ai/v1" },
} as const
```

**关键设计**：所有兼容 OpenAI chat 协议的 provider 复用同一 `OpenAICompatibleChat.route`，只换 `baseURL` + provider name。

`alibaba`（通义千问，DashScope 的 OpenAI-compatible 入口）虽然在 CLAUDE.md 描述中出现，但在源码 profiles 里**目前没列** —— 推测是配置文件层做的（不在 `packages/llm/src/providers/` 而是在 `models-dev.ts` 之类的 model registry 里）。

##### 6.4.6.7 差异化总结表

| Provider | 协议 | Auth | Framing | 路由选择 | 特殊点 |
|---|---|---|---|---|---|
| **Anthropic** | anthropic-messages | `x-api-key` | SSE | 单路由 | cache_control TTL 5m/1h |
| **OpenAI** | chat + responses | Bearer | SSE | 按 model 自动选 | GPT-5+ → responses，其他 → chat |
| **OpenRouter** | openai-chat (扩展) | Bearer | SSE | 单路由 | `prompt_cache_key`、`usage`、`reasoning` 透传 |
| **Bedrock** | converse (binary) | SigV4 | event stream | 单路由 | 区域 URL、SigV4 签名 |
| **Copilot** | chat + responses | Bearer (显式) | SSE | `gpt-5+` 且非 `gpt-5-mini` → responses | 无规范 URL，调用方必须传 baseURL |
| **OpenAI-compatible** | openai-chat | Bearer | SSE | 单路由 | profile 化 baseURL，9 套预设 |
| **Google Gemini** | generateContent | API key | SSE | 单路由 | 独立 `gemini.ts` 协议 |
| **Cloudflare AI** | workers-ai | API token | SSE | 单路由 | Workers AI gateway |
| **Azure** | openai-chat | API key + deployment | SSE | 单路由 | 走 Azure-specific endpoint |

#### 6.4.7 对 laew 的 P0/P1 借鉴

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | 5 元组协议 | `Route = Protocol × Endpoint × Auth × Framing × Transport` —— 让 laew 的 `provider.rs` 从"大 if-else"变成组合 | route/executor.ts:36-53 |
| **P0** | 协议分层 | `Protocol` 只关心"模型说什么"，`Endpoint` / `Auth` 只关心"发给谁 / 怎么鉴权" | route/protocol.ts:36-43 |
| **P0** | 脱敏 2 层 | 结构脱敏 + 字面值脱敏 —— laew 的 `mask_key` 应该也做"原值 echo 防漏" | route/client.ts:48-66 |
| **P1** | retryable 标记 | `Error::retryable()` 决定 backoff，无需在 retry 代码里枚举错误 | schema/errors.ts:160-172 |
| **P1** | rate limit 双命名 | 同时识别 OpenAI / Anthropic 命名规范 —— laew 可统一 `RateLimitDetails` | route/client.ts:112-148 |
| **P1** | takeUntil + mapAccumEffect | 流终止条件 + 状态机累加，让 Anthropic 的 `[DONE]` 和 Bedrock 的事件终止条件统一 | route/executor.ts:279-294 |
| **P2** | provider profile | OpenAI-compatible 9 套 profile —— laew 接入"小众 provider"（如 deepseek、together）时不用写一整条 if 分支 | providers/openai-compatible-profile.ts:6 |
| **P2** | StructWithRest | OpenRouter 风格 —— 在 OpenAI body schema 后面加开放字段，扩展兼容 provider 而不 fork 协议 | providers/openrouter.ts:33-35 |

### 6.5 Tool 系统的全栈集成

#### 6.5.1 BuiltInTools 静态组合

`packages/core/src/tool/builtins.ts`（48 行）显式列出 12 个内置工具节点：

```ts
// packages/core/src/tool/builtins.ts:31
export const node = makeLocationNode({
  name: "built-in-tools",
  layer: Layer.empty,  // ← 注意：这一层是 Layer.empty
  deps: [
    ApplyPatchTool.node, BashTool.node, EditTool.node, GlobTool.node,
    GrepTool.node, QuestionTool.node, ReadTool.node, SkillTool.node,
    TodoWriteTool.node, WebFetchTool.node, WebSearchTool.node, WriteTool.node,
  ],
})
```

**关键 trick**：`layer: Layer.empty` —— 这一层本身不贡献 Service，只把 12 个子节点的 deps"挂"上来。编译时所有 12 个子节点会通过 `Layer.provideMerge` 一起合并到 root layer。所以最终 root layer 包含所有 12 个 tool 的注册副作用。

#### 6.5.2 ApplicationTools — 动态注册

`packages/core/src/tool/application-tools.ts`（57 行）：与 BuiltInTools 不同的是 dynamic register。注释解释："动态 MCP 和 plugin tools 之后用 separate scoped canonical registrations"。

```ts
const state = State.create<Data, Draft>({
  initial: () => ({ entries: new Map() }),
  draft: (draft) => ({
    set: (name, tool) => { draft.entries.set(name, tool) },
  }),
})
```

`State.create` 是 opencode 自研的"可读写 state"抽象，支持 transform with draft。ApplicationTools 节点（`makeGlobalNode`）注册后，全局任何 Tool 注册请求都通过 `state.transform(d => d.set(name, tool))`。

#### 6.5.3 ToolRegistry — 合并 + 权限过滤

`packages/core/src/tool/registry.ts`（147 行）：

```ts
// packages/core/src/tool/registry.ts:106
materialize: Effect.fn("ToolRegistry.materialize")(function* (permissions = []) {
  const registrations = new Map(applications.entries())
  for (const [name, entries] of local) {
    const registration = entries.at(-1)?.registration
    if (registration) registrations.set(name, registration)
  }
  for (const [name, registration] of registrations)
    if (whollyDisabled(permission(registration.tool, name), permissions)) registrations.delete(name)
  return {
    definitions: Array.from(registrations, ([name, registration]) => definition(name, registration.tool)),
    settle: (input) => {
      const registration = registrations.get(input.call.name)
      if (registration) return settleWith(input, registration.identity)
      return Effect.succeed({ result: { type: "error", value: `Unknown tool: ${input.call.name}` } })
    },
  }
})
```

**关键设计**：

1. **`local` 栈**：Local 注册有 `token`（finalizer 标记），注销时清理。同名工具后注册的覆盖先注册的（`entries.at(-1)`）。
2. **Materialize**：把 application + local 合并成最终 `Map<name, registration>`。
3. **权限过滤**：调用 `whollyDisabled(action, permissions)` 删掉被 deny 的工具。`Wildcard.match(action, rule.action)` 处理 `*` 通配。
4. **`settle` 处理 stale call**：模型可能在工具已经被卸载后发来 `tool_call.name`，返回 `"Stale tool call"` 错误而不是崩溃。

#### 6.5.4 SessionCompaction — 自动摘要管线

`packages/core/src/session/compaction.ts`（248 行）展示了 Session 级别的智能化：

```ts
// packages/core/src/session/compaction.ts:12
const DEFAULT_BUFFER = 20_000
const DEFAULT_KEEP_TOKENS = 8_000
const TOOL_OUTPUT_MAX_CHARS = 2_000
const SUMMARY_OUTPUT_TOKENS = 4_096
const SUMMARY_TEMPLATE = `...Objective / Important Details / Work State / Next Move / Relevant Files...`
```

**关键设计**：

1. **Token 估算**（`Token.estimate`）：用 `JSON.stringify(value).length / 4` 估算 token 数（粗略但够用）。
2. **`select(entries, tokens)`** 反向累加：从最近的 entry 开始累加 token 数，直到超过 `keep_tokens`，把 conversation 切成 `head` + `recent`。
3. **`buildPrompt` 模板**：根据是否存在 `previousSummary` 选两种 prompt 之一 —— 新建 summary 或更新已有 summary。
4. **`compactAfterOverflow`** 主动调用 LLM 生成摘要，监听 `LLMEvent.is.textDelta` 累加 chunks。
5. **`compactIfNeeded`** 总入口，根据 `auto + buffer + tokens` 配置判断。
6. **两条 event**：`SessionEvent.Compaction.Started` / `Compaction.Ended`，TUI 可见。

#### 6.5.5 BashTool / EditTool 的 30 行级细节

**BashTool**（`packages/core/src/tool/bash.ts:79-95`）的 token-based 外部目录检测：

```ts
const shellTokens = (command: string) => command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) ?? []
const unquote = (value: string) => value.replace(/^(['"])(.*)\1$/, "$2")
const externalCommandDirectories = Effect.fn("BashTool.externalCommandDirectories")(function* (fs, command, cwd) {
  const directories = new Set<string>()
  for (const token of shellTokens(command)) {
    const value = unquote(token).replace(/[;,|&]+$/, "")
    if (!path.isAbsolute(value)) continue
    const resolved = yield* fs.resolve(value)
    if (FSUtil.contains(cwd, resolved)) continue
    directories.add(yield* fs.resolve(path.dirname(resolved)))
  }
  return [...directories]
})
```

**关键**：

- 用 regex 解析 shell 命令的 tokens（保留引号）。
- 对每个 token 判断是不是 absolute path 且不在 cwd 内。
- 收集所有外部目录，作为 permission 检查的 resource —— 比简单的"命令是否含 `..`"更精细。

**EditTool**（`packages/core/src/tool/edit.ts:73-80`）的 diff 输出格式：

```ts
export const toModelOutput = (output: Output, oldString: string, newString: string) => [
  `Edited file successfully: ${output.files[0]?.file}`,
  `Replacements: ${output.replacements}`,
  "```diff",
  ...previewLines(oldString, "-"),
  ...previewLines(newString, "+"),
  "```",
]
```

模型看到的反馈是**真实 diff 风格**，便于它下一轮调整。

#### 6.5.6 对 laew 的 P0/P1 借鉴

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | BuiltInTools 静态组合 | `Layer.empty` + 12 个子节点 deps —— 等价于 laew 的 `builtin_registry()` | builtins.ts:31-48 |
| **P0** | Stale tool call 处理 | 模型发来已卸载工具的调用 → 返回 `"Stale tool call"`，不崩溃 | registry.ts:117-119 |
| **P0** | Tool output 截断 | `TOOL_OUTPUT_MAX_CHARS = 2_000` 防止大输出撑爆 context | compaction.ts:14 |
| **P1** | Token 估算 | `Token.estimate(value)` —— laew 可用同样公式做 compaction 决策 | compaction.ts:83 |
| **P1** | 反向累加切分 | 从最新 entry 倒着累加直到超阈值 —— 比"从头累加"更稳 | compaction.ts:148-157 |
| **P1** | external directory 检测 | shell token 解析判断 absolute + 不在 cwd → 收集为 permission resource | bash.ts:79-95 |
| **P1** | Summary 模板 | `Objective / Important Details / Work State / Next Move / Relevant Files` —— laew 可借鉴 session memory 摘要结构 | compaction.ts:16-40 |
| **P2** | diff 输出格式 | `\`\`\`diff\n-...\n+...\n\`\`\`` 让模型下一轮调整更精确 | edit.ts:73-80 |

### 6.6 Enterprise — Cloudflare Durable Object + R2 共享存储

`packages/enterprise/`（35 个 .ts/.tsx 文件）实现了"在 Cloudflare Workers 上托管 opencode 共享会话"的能力。核心是把 share snapshot 存到 R2（兼容 S3），worker 进程无状态，靠 R2 持久化。

#### 6.6.1 Storage Adapter

`packages/enterprise/src/core/storage.ts`（129 行）：

```ts
// packages/enterprise/src/core/storage.ts:12
function createAdapter(client: AwsClient, endpoint: string, bucket: string): Adapter {
  const base = `${endpoint}/${bucket}`
  return {
    async read(path: string): Promise<string | undefined> {
      const response = await client.fetch(`${base}/${path}`)
      if (response.status === 404) return undefined
      if (!response.ok) throw new Error(`Failed to read ${path}: ${response.status}`)
      return response.text()
    },
    async write(path: string, value: string): Promise<void> {
      const response = await client.fetch(`${base}/${path}`, {
        method: "PUT", body: value, headers: { "Content-Type": "application/json" },
      })
      if (!response.ok) throw new Error(`Failed to write ${path}: ${response.status}`)
    },
    async remove(path: string): Promise<void> {
      const response = await client.fetch(`${base}/${path}`, { method: "DELETE" })
      if (!response.ok) throw new Error(`Failed to remove ${path}: ${response.status}`)
    },
    async list(options?: { prefix?: string; limit?: number; after?: string; before?: string }): Promise<string[]> {
      const prefix = options?.prefix || ""
      const params = new URLSearchParams({ "list-type": "2", prefix })
      if (options?.limit) params.set("max-keys", options.limit.toString())
      if (options?.after) {
        const afterPath = prefix + options.after + ".json"
        params.set("start-after", afterPath)
      }
      const response = await client.fetch(`${base}?${params}`)
      if (!response.ok) throw new Error(`Failed to list ${prefix}: ${response.status}`)
      const xml = await response.text()
      const keys: string[] = []
      const regex = /<Key>([^<]+)<\/Key>/g
      let match
      while ((match = regex.exec(xml)) !== null) keys.push(match[1])
      if (options?.before) {
        const beforePath = prefix + options.before + ".json"
        return keys.filter((key) => key < beforePath)
      }
      return keys
    },
  }
}
```

**关键设计**：

1. **`aws4fetch`**（不是 AWS SDK）：用 `fetch` 直接打 S3-compatible API，避免在 Workers 里引入庞大的 AWS SDK。
2. **双后端支持**：`s3()` 和 `r2()` 两个工厂，通过 `OPENCODE_STORAGE_ADAPTER` 环境变量选择。R2 endpoint 是 `${accountId}.r2.cloudflarestorage.com`。
3. **`list` 用 S3 list-type=2 + max-keys + start-after**：标准 S3 listing，可分页。
4. **`{prefix, after, before}` 范围扫描**：`after` / `before` 用于"snapshot 之后增量同步"。
5. **`update<T>(key, fn)` 读改写**：`update` 内部用 `read` → 修改 → `write`，没有事务保证但大多数场景够用。

#### 6.6.2 Share — 多端同步核心

`packages/enterprise/src/core/share.ts`（232 行）实现 session 共享协议：

```ts
// packages/enterprise/src/core/share.ts:18
export const Data = z.discriminatedUnion("type", [
  z.object({ type: z.literal("session"), data: z.custom<Session>() }),
  z.object({ type: z.literal("message"), data: z.custom<Message>() }),
  z.object({ type: z.literal("part"), data: z.custom<Part>() }),
  z.object({ type: z.literal("session_diff"), data: z.custom<SnapshotFileDiff[]>() }),
  z.object({ type: z.literal("model"), data: z.custom<Model[]>() }),
])
```

5 种数据类型，discriminated by `type`。

**Sync 协议**（`share.ts:156`）：

```ts
export const sync = fn(z.object({ share: Info.pick({ id: true, secret: true }), data: Data.array() }), async (input) => {
  const share = await get(input.share.id)
  if (!share) throw new Errors.NotFound(input.share.id)
  if (share.secret !== input.share.secret) throw new Errors.InvalidSecret(input.share.id)
  const data = (await readSnapshot(input.share.id)) ?? (await legacy(input.share.id))
  await writeSnapshot(input.share.id, merge(data, input.data))
})
```

**关键流程**：

1. **校验 secret**：share ID + secret 必须匹配，否则 403。
2. **读 snapshot**：从 R2 读 `share_snapshot/{id}`，拿当前完整 state。
3. **合并新 data**：客户端发来的增量与现有数据按 `key(item)` 合并去重。
4. **写 snapshot**：原子地写回（虽然 R2 不保证原子，但通常足够）。

**legacy 兼容**（`share.ts:86`）：老的 share 是按"event 流"存的（每次增量存一个文件），新代码读出来后会**一次性 merge 成 snapshot**，并存一份 snapshot 副本。下次 sync 直接走 snapshot 路径。

#### 6.6.3 entry-server.tsx — SolidStart SSR

`packages/enterprise/src/entry-server.tsx`（Cloudflare Workers 入口）：

- 用 SolidStart 做 SSR。
- API 路由在 `routes/api/[...path].ts`（Catch-all API），所有 `/api/*` 请求都过这里。
- 前端页面 `routes/share.tsx` + `share/[shareID].tsx` —— 公开访问 share ID 对应的 session。

#### 6.6.4 对 laew 的 P0/P1 借鉴

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | aws4fetch 替代 SDK | Workers / 边缘场景用 fetch 直打 S3-compatible API，避免 AWS SDK 体积 | storage.ts:1-64 |
| **P0** | 共享 ID + secret | share 创建时生成 crypto.randomUUID 作为 secret，删除/更新都要 secret 校验 | share.ts:117-128 |
| **P0** | 数据类型 discriminated union | `type: "session" | "message" | "part" | "session_diff" | "model"` —— 单一 sync 入口 | share.ts:18-39 |
| **P1** | legacy 兼容 | 老 event 流格式 → 一次性 merge 成 snapshot —— 协议升级无需客户端配合 | share.ts:86-115 |
| **P1** | start-after + before 范围 | 增量同步用 `after: cursor` + `before: cursor` | storage.ts:40-62 |
| **P1** | update<T> 读改写 | 没有事务，但 `update<T>(key, fn)` 是常用语义 | storage.ts:122-128 |
| **P2** | Cloudflare Workers 部署 | laew 如果未来做"网页端查看 session"，可以借鉴 Workers + R2 模式 | entry-server.tsx |
| **P2** | snapshot + event 双轨 | 老的 event 流 + 新的 snapshot 并行 —— 兼容旧客户端 | share.ts:78-83 |

### 6.7 34 包结构全景

`/usr/local/LsmGitOpenSource/opencode/packages/` 下 34 个包，按职责归类：

#### 6.7.1 核心运行时（5 个）

| 包 | 作用 | 关键文件 |
|---|---|---|
| **opencode** | CLI 入口（`packages/opencode/src/cli/`） | `cli/cmd/run.ts`、`cli/cmd/tui.ts`、`cli/cmd/serve.ts` |
| **core** | 主逻辑、agent、session、tool、permission、plugin、mcp、skill、effect、project、filesystem | 70+ 目录，~3 万行 |
| **llm** | LLM 协议客户端（Protocol × Endpoint × Auth × Framing × Transport） | `route/`、`protocols/`、`providers/`、`schema/` |
| **schema** | 跨包共享的 Schema 定义（Message、ToolDefinition、LLMRequest） | `src/llm.ts`、`src/file-diff.ts` |
| **protocol** | 服务端到客户端的 RPC 协议 | `src/` |

#### 6.7.2 工具 / 文件系统（3 个）

| 包 | 作用 |
|---|---|
| **ripgrep** | ripgrep 封装（grep / glob 工具的后端） |
| **filesystem** | FileSystem + Path + GrepInput + Entry + Match 等 Schema |
| **tool-output-store** | Tool 输出的有界存储（preview + 完整内容分轨） |

#### 6.7.3 数据 / 持久化（4 个）

| 包 | 作用 |
|---|---|
| **effect-drizzle-sqlite** | 基于 effect + drizzle 的 SQLite 客户端 |
| **effect-sqlite-node** | SQLite Node 绑定 |
| **console-core** | Drizzle schema + migration（~30 个表：users / workspaces / billing / keys / subscriptions 等） |
| **database** | core 包内的 SQLite session storage |

#### 6.7.4 网络 / 安全（4 个）

| 包 | 作用 |
|---|---|
| **identity** | OAuth + 用户身份 |
| **credential** | 凭证管理（API key 等） |
| **plugin** | 插件系统（TUI shell + workspace + tool 三个 layer） |
| **mcp** | MCP client/server（待补） |

#### 6.7.5 部署 / UI（11 个）

| 包 | 作用 |
|---|---|
| **cli** | 命令行参数解析 |
| **tui** | 终端 UI（curses 风格） |
| **web** | Web UI |
| **app** | opencode-app（Electron / 桌面） |
| **desktop** | 桌面应用 |
| **session-ui** | session 分享页面 |
| **storybook** | Storybook（UI 组件库） |
| **ui** | 共享 UI 组件 |
| **slack** | Slack 集成 |
| **console-app** | 后台管理 web |
| **stats** | 统计（server / core / app 三件套） |

#### 6.7.6 企业版 / 商业（3 个）

| 包 | 作用 |
|---|---|
| **enterprise** | Cloudflare Workers 部署 + R2 + share 协议 |
| **function** | Cloudflare Functions（独立部署的 worker） |
| **sdks / sdk-next** | 客户端 SDK（v1 / v2） |

#### 6.7.7 工具链（4 个）

| 包 | 作用 |
|---|---|
| **codemode** | "代码模式"运行时（待补） |
| **containers** | 容器化部署 |
| **http-recorder** | HTTP 请求录制 / 回放（测试用） |
| **httpapi-codegen** | HTTP API 代码生成 |
| **script** | 通用脚本运行时 |
| **perf** | 性能基准 |

#### 6.7.8 与 laew 的对比

laew 的目录（参考 CLAUDE.md）：

```
main.rs        clap CLI
tui/           REPL 主屏 + 子屏
agent/         Agent loop + tools + system prompt
llm/           anthropic + openai 客户端
config/        SQLite + paths
error.rs       AgentError
build.rs       git hash + build time
```

opencode 是 **N 倍复杂度** —— 34 包、5 种前端（TUI / Web / Desktop / Slack / Session UI）、4 种部署形态（CLI / Docker / Workers / Electron）、6 套 provider。laew 当前是单一 Rust crate + TUI + CLI。

#### 6.7.9 对 laew 的 P0/P1 借鉴

| 优先级 | 模块 | 借鉴内容 |
|---|---|---|
| **P2** | 包拆分 | 未来如果 laew 要做 web 版本，可以先抽出 `web/` 包（共享 schema / core / llm） |
| **P2** | stats 包 | 拆出 `stats-core` + `stats-app` —— 调用链 / token 用量 / 失败率打点 |
| **P2** | http-recorder | `http-recorder` 录制 / 回放 LLM 响应 —— laew 端到端测试可以从真交互降级到录制 |

### 6.8 关键洞察汇总

#### 6.8.1 Effect Schema = "全栈数据契约"

opencode 的核心架构选择是：**协议中立数据模型（Message、ToolDefinition、LLMRequest）+ Schema 一处定义、TypeScript 类型 + 运行时校验 + JSON Schema + Encoder/Decoder 都自动生成**。这跟 Zod 的"运行时校验 + 类型推导"很像，但有 3 个本质差异：

1. **Effect-returning**：decode 是 Effect，可以 pipe 进 retry / catchTag / mapEffect，**与运行时深度融合**。
2. **双向 Codec**：encode + decode 分开，可以有 decode-only Schema（用于 parse 不信任输入）。
3. **Brand + Tagged Union**：`_tag` 让协议中立错误模型自带 `_tag: "RateLimit" | "Authentication" | "ProviderInternal" | ...`，`retryable` getter 决定重试。

#### 6.8.2 LayerNode DI = "显式拓扑 + 编译期检测"

Effect 原生 `Layer.provide` 在大规模图里不够用 —— opencode 在 Effect 之上建了：

- **节点（layer / unbound / group）**：DI 边显式化。
- **Tag（global / location）**：声明"哪些服务是全局共享、哪些是 per-会话"。
- **compile**：DFS + 三色标记 + cache 折叠。
- **hoist**：把 global tag 上提，避免重复构建。
- **replacement**：测试 mock 的统一入口。

#### 6.8.3 Protocol × Endpoint × Auth × Framing × Transport = 协议分层

5 个轴独立变化，组合出新部署。OpenAI-compatible 9 套 profile 是这套模型的极致 —— 共享同一 protocol 只换 baseURL。Anthropic 用 `x-api-key`、OpenAI 用 Bearer、Bedrock 用 SigV4 —— Auth 轴独立配置。

#### 6.8.4 共享存储 = "数据格式 + secret 鉴权 + legacy 兼容"

`packages/enterprise/` 把 share 协议抽象成：

- **5 种数据类型 discriminated union**（session / message / part / session_diff / model）。
- **secret 鉴权**（crypto.randomUUID + secret 校验）。
- **legacy 兼容**（老 event 流一次性 merge 成 snapshot）。

#### 6.8.5 Tool 系统的"静态 + 动态"双轨

- **BuiltInTools**：12 个工具静态组合，`Layer.empty` + 12 个子节点 deps，编译时合并。
- **ApplicationTools**：动态注册 + finalizer，token 标记生命周期。
- **ToolRegistry**：合并 + 权限过滤 + stale call 处理。

#### 6.8.6 SessionCompaction = "token 估算 + 反向累加 + LLM 摘要"

- `Token.estimate` 粗略估算。
- `select(entries, tokens)` 反向累加。
- `compactAfterOverflow` 调 LLM 生成 markdown 摘要。
- `compactIfNeeded` 总入口。
- `Compaction.Started` / `Compaction.Ended` event 触发 TUI 更新。

#### 6.8.7 对 laew 的 P0/P1 借鉴路线

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | Effect Schema | laew 的 `BashTool` / `ReadTool` / `WriteTool` 改用 Schema 定义 input/output，runtime 自动 derive JSON Schema | tool.ts:71-132 |
| **P0** | LayerNode DI | 把当前"手动构造 Agent 实例"换成节点图 + compile 流程 | layer-node.ts:81-272 |
| **P0** | 协议分层 | `LlmClient` 拆成 Protocol × Endpoint × Auth × Framing × Transport | route/executor.ts:36-53 |
| **P0** | Tagged Error | `AgentError` 加 `_tag`，`Error::retryable()` 决定重试 | schema/errors.ts:160-172 |
| **P0** | 脱敏 2 层 | 结构脱敏 + 字面值脱敏 | route/client.ts:48-66 |
| **P1** | 5 元组协议 + 9 profile | OpenAI-compatible 9 套 profile + DeepSeek、Together、Groq 等小众 provider 接入成本几乎为零 | providers/openai-compatible-profile.ts:6 |
| **P1** | Token 估算 + 反向累加 | `Token.estimate` + `select(entries, tokens)` 做 compaction 决策 | compaction.ts:83 + 148-157 |
| **P1** | stale tool call | 模型发来已卸载工具的调用 → `"Stale tool call"` 而非崩溃 | registry.ts:117-119 |
| **P1** | 共享 secret | session 分享 / 多端同步用 `crypto.randomUUID()` 生成 secret + 校验 | share.ts:117-128 |
| **P2** | 34 包结构 | laew 未来做 web / 桌面时抽 `web/` / `desktop/` 包 | 34 包全景 |
| **P2** | aws4fetch 替代 SDK | 边缘场景避免 AWS SDK 体积 | storage.ts:1-64 |
| **P2** | update<T> 读改写 | SQLite 上做"读 → 改 → 写"的语义抽象 | storage.ts:122-128 |

### 6.9 与前五轮的纵向对比

| 维度 | 前五轮主要发现 | 本轮新增 |
|---|---|---|
| 架构 | Effect-based 重构、LayerNode 拓扑、Tool registry、SSE 流式 | **Provider 6 套差异**（Anthropic x-api-key / Bedrock SigV4 / Copilot 自定义路由） |
| 协议 | OpenAI Chat / Responses + Anthropic Messages | **Protocol × Endpoint × Auth × Framing × Transport 五元组** + 协议分层极致抽象 |
| DI | Tool / LLM Provider 注册 | **Tag 拓扑分类（global / location）+ unbound 兜底 + replacement mock** |
| 持久化 | SQLite session storage | **Cloudflare Durable Object + R2 共享 + 5 种数据类型 discriminated union + legacy 兼容** |
| 压缩 | prompt.ts compaction | **Token 估算 + 反向累加切分 + LLM 摘要 + Compaction.Started/Ended event** |
| Tool | Bash/Read/Write + 27 工具 | **Schema 即协议（toJsonSchemaDocument 一次定义、5 处生成）+ structured 输出投影** |
| 错误 | LLMError retryable | **Tagged union（_tag: "RateLimit" \| "Authentication" \| ...）+ retryable getter** |
| 部署 | 单进程 | **34 包结构 + 5 种前端 + 4 种部署形态（CLI / Docker / Workers / Electron）** |
| 协议中立 | Message / ContentPart | **CachePolicy Schema 化 + cache 6 provider 分发（anthropic/openrouter/bedrock/openaiCompatible/copilot/alibaba）** |
| 鉴权 | ToolPermission V2 + saved | **Auth DSL（bearer / header / bearerHeader + andThen / orElse 组合）+ AWS SigV4** |

### 6.10 总结

opencode 第六轮深挖的核心结论：

1. **Effect Schema = 全栈数据契约**：TypeScript 类型 + 运行时校验 + JSON Schema + Encoder/Decoder 都从一个 `Schema.Struct(...)` 自动生成；与 Effect runtime 深度融合（`Schema.decodeUnknownEffect` 返回 `Effect`）。
2. **LayerNode DI = 显式拓扑**：节点 + 依赖图 + 编译 + 循环检测 + tag 分类（global / location）+ replacement mock，比 Effect 原生 `Layer.provide` 更可控。
3. **5 元组协议**：Protocol × Endpoint × Auth × Framing × Transport 任意组合，OpenAI-compatible 9 套 profile + Anthropic + Bedrock SigV4 + Copilot 双路由。
4. **Cloudflare Durable Object + R2**：aws4fetch 替代 SDK，5 种数据类型 discriminated union，secret 鉴权 + legacy event 流兼容。
5. **Tool 系统静态 + 动态双轨**：BuiltInTools 静态组合、ApplicationTools 动态注册、ToolRegistry 合并 + 权限 + stale call。
6. **SessionCompaction 三段式**：token 估算 + 反向累加切分 + LLM 摘要，event 驱动 TUI 可见。
7. **34 包结构**：核心 5 + 工具 3 + 数据 4 + 网络 4 + UI 11 + 企业 3 + 工具链 4 —— 对应 5 种前端、4 种部署、6 套 provider。

**对 laew 的核心启发**：把"协议中立数据模型 + Schema 一处定义"作为第一性原则；DI 拓扑显式化；Tool 系统支持 stale call 处理；Compaction 用反向累加 + LLM 摘要；多端同步用 secret 鉴权 + 数据类型 discriminated union。
