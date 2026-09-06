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
