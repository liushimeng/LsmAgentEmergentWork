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
19. [第七轮深挖 — 文件编辑补丁策略 + 代码检索索引 + Schema 结构化输出 + Bash 进程管理](#19-第七轮深挖--文件编辑补丁策略--代码检索索引--effect-schema结构化输出与跨provider归一化--bash进程管理)

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

---

## 19. 第七轮深挖 — 文件编辑补丁策略 + 代码检索索引 + Effect Schema结构化输出与跨provider归一化 + Bash进程管理

> 调研日期：2026-09-06　源码：`/usr/local/LsmGitOpenSource/opencode`（TypeScript / Bun，34 packages）
> 本轮四个维度在前六轮（架构 / Effect DI / LLM 协议 / Context / Skill / 工具系统 / 流式 / 持久化 / 第六轮 Effect 运行时 + Durable Object）**均未覆盖**，本章全部为新增内容。
> 所有结论均给出 `packages/xxx/src/yyy.ts:LINE` 的真实路径与行号。

### 19.0 本轮源码地图

| 维度 | 主战场 | 行数 | 关键文件 |
|------|--------|------|----------|
| ① 文件编辑与补丁 | `packages/opencode/src/tool/edit.ts`（737） | V1 精确编辑 + 9 级模糊回退 | `edit.ts` / `write.ts` / `apply_patch.ts` / `patch/index.ts`（686） |
| ① 快照与 undo | `packages/opencode/src/snapshot/index.ts`（807） | 影子 git 仓库 + `write-tree` 哈希 | `snapshot/index.ts` / `session/revert.ts` / `session/processor.ts` |
| ① 新一代（V2） | `packages/core/src/tool/edit.ts`（223） | CAS 乐观并发 + KeyedMutex | `core/src/file-mutation.ts`（207） |
| ② 代码检索与索引 | `packages/core/src/ripgrep.ts`（284） | 外挂 rg 二进制自举 + JSON 流解析 | `ripgrep/binary.ts` / `tool/grep.ts` / `tool/glob.ts` / `lsp/*` |
| ③ 结构化输出 | `packages/opencode/src/tool/json-schema.ts`（164） | Effect Schema → JSON Schema 归一化 | `provider/transform.ts:1546-1686` / `session/llm.ts:296` |
| ④ Bash 进程管理 | `packages/opencode/src/tool/shell.ts`（645） | tree-sitter 解析 + 流式截断 + 进程组 kill | `permission/arity.ts`（163）/ `core/src/tool/bash.ts`（207） |

**V1 / V2 双轨说明（贯穿本章的概念）**：opencode 正在把工具从 `packages/opencode/src/tool/*`（V1，AI SDK `tool()` 包装、`ctx.ask` 权限模型、格式化 + LSP + 快照全家桶）迁移到 `packages/core/src/tool/*`（V2，`Tool.make()` + `ToolRegistry` + `PermissionV2` + `LocationMutation`，能力明显更弱，TODO 里明确写着"待迁移"）。**本章对每个维度同时给出 V1 的成熟实现和 V2 的现状**，因为 V2 的 TODO 注释恰好是 opencode 团队自己承认的"欠账清单"，对 laew 的技术选型价值极高。

---

### 19.1 维度一：文件编辑与补丁策略

#### 19.1.1 三套编辑入口，按模型分流（不是按用户配置）

最反直觉的一点：opencode **不是让用户选**用 Edit 还是 ApplyPatch，而是**按模型 ID 自动切换**（`packages/opencode/src/tool/registry.ts:297-302`）：

```ts
// packages/opencode/src/tool/registry.ts:297
const usePatch =
  input.modelID.includes("gpt-") && !input.modelID.includes("oss") && !input.modelID.includes("gpt-4")
if (tool.id === ApplyPatchTool.id) return usePatch
if (tool.id === EditTool.id || tool.id === WriteTool.id) return !usePatch
```

| 模型 | 暴露的工具 | 说明 |
|------|-----------|------|
| `gpt-5*` / `gpt-5.1*` 等非 oss、非 gpt-4 | **只有 `apply_patch`** | 沿用 Codex CLI 的补丁语言，模型训练语料里就有 |
| 其它（Claude / Gemini / gpt-4 / oss） | **`edit` + `write`** | 经典 `oldString → newString` |
| 全部 | `read` / `glob` / `grep` / `bash` / `task` / `webfetch` … | 通用工具 |

**洞察**：这是"按模型原生语料选择工具形态"的写法——同一个语义（改文件）准备两套 wire format，由编排层在 `registry.tools()` 里按 modelID 静态裁剪，而不是把三套工具都塞进 prompt 让模型挑。工具集裁剪本身就是一种 prompt 优化（少 2 个工具定义 ≈ 省几百 token + 少一种歧义）。

#### 19.1.2 EditTool：参数 Schema、前置校验与创建/修改双路径

参数定义（`packages/opencode/src/tool/edit.ts:47-56`）——注意 `filePath` 的 description 要求"绝对路径"，但代码里同时接受相对路径（下面 80-82 行做 `path.join(instance.directory, ...)`）：

```ts
// packages/opencode/src/tool/edit.ts:47
export const Parameters = Schema.Struct({
  filePath: Schema.String.annotate({ description: "The absolute path to the file to modify" }),
  oldString: Schema.String.annotate({ description: "The text to replace" }),
  newString: Schema.String.annotate({
    description: "The text to replace it with (must be different from oldString)",
  }),
  replaceAll: Schema.optional(Schema.Boolean).annotate({
    description: "Replace all occurrences of oldString (default false)",
  }),
})
```

前置校验（`:71-77`）：`filePath` 空 → 报错；`oldString === newString` → `"No changes to apply: oldString and newString are identical."`。**这两条在 `replace()` 里又重复了一遍**（`:683-689`），属于防御式双保险。

`oldString === ""` 是**创建文件的合法路径**（`:90-121`），但只在目标文件**不存在**时允许：

```ts
// packages/opencode/src/tool/edit.ts:90
if (params.oldString === "") {
  const existed = yield* afs.existsSafe(filePath)
  if (existed) {
    throw new Error(
      "oldString cannot be empty when editing an existing file. Provide the exact text to replace, or use write for an intentional full-file replacement.",
    )
  }
  ... // 走新建分支：BOM 拆分 → diff → ask("edit") → writeWithDirs → 事件 "add"
}
```

也就是说：**"用 edit 全量覆盖已有文件"被显式禁止**，错误信息直接把模型引导到 `write`。这是很干净的行为约束——避免模型用空 oldString + 全文 newString 的方式绕过 write 的语义。

其余前置检查（`:123-127`）：
- `stat` 失败 → `File ${filePath} not found`
- `stat.type === "Directory"` → `Path is a directory, not a file: ${filePath}`

#### 19.1.3 核心：9 级 Replacer 回退链（模糊容错的全部秘密）

`replace()` 是本章最有价值的一段代码（`packages/opencode/src/tool/edit.ts:682-737`）。它不是一个 `indexOf`，而是**按严格度递减顺序尝试 9 个 Replacer 生成器**，第一个能产出唯一命中者胜出：

```ts
// packages/opencode/src/tool/edit.ts:692
let notFound = true

for (const replacer of [
  SimpleReplacer,              // 1. 原样
  LineTrimmedReplacer,         // 2. 逐行 trim 后比较（容忍缩进/尾部空格）
  BlockAnchorReplacer,         // 3. 首行+尾行锚定 + Levenshtein 相似度 ≥ 0.65
  WhitespaceNormalizedReplacer,// 4. \s+ → 单空格 归一化
  IndentationFlexibleReplacer, // 5. 整体去缩进（min-indent 剥离）后比较
  EscapeNormalizedReplacer,    // 6. \n \t \" \\ 等转义序列反转义
  TrimmedBoundaryReplacer,     // 7. 整体 trim 后比较
  ContextAwareReplacer,        // 8. ≥3 行时首/尾行锚定 + 中间行 ≥50% 匹配
  MultiOccurrenceReplacer,     // 9. 产出所有精确命中（交给 replaceAll/lastIndex 判定）
]) {
  for (const search of replacer(content, oldString)) {
    const index = content.indexOf(search)
    if (index === -1) continue
    notFound = false
    if (isDisproportionateMatch(search, oldString)) { throw new Error(...) }
    if (replaceAll) return content.replaceAll(search, newString)
    const lastIndex = content.lastIndexOf(search)
    if (index !== lastIndex) continue        // ← 多处命中则跳过，换下一个 Replacer
    return content.substring(0, index) + newString + content.substring(index + search.length)
  }
}
```

逐个 Replacer 的关键实现：

| Replacer | 行号 | 算法要点 |
|----------|------|----------|
| `SimpleReplacer` | `:244-246` | `yield find`（原样），最严格 |
| `LineTrimmedReplacer` | `:248-286` | 逐行 `trim()` 后逐行比对；命中后手算 `matchStartIndex/matchEndIndex`（累加 `length+1`）把**原始子串**（含原缩进）切出来 —— 保证替换后的文本保留文件里真实的缩进 |
| `BlockAnchorReplacer` | `:288-425` | 要求 ≥3 行；首行/尾行 `trim()` 锚定；块大小容差 `maxLineDelta = max(1, floor(n*0.25))`（`:303`）；中间行用 **Levenshtein 相似度**，单候选阈值 0.65、多候选阈值 0.65（`:220-221`）；多候选取最大相似度者 |
| `WhitespaceNormalizedReplacer` | `:427-469` | `text.replace(/\s+/g," ").trim()`；单行全等 → 单行子串正则（word 之间 `\s+`，`:444`）→ 多行块全等 |
| `IndentationFlexibleReplacer` | `:471-497` | 计算非空行最小缩进 `minIndent`，全部 `slice(minIndent)` 后比较 —— 等价于"整体左移对齐" |
| `EscapeNormalizedReplacer` | `:499-546` | `\\(n|t|r|'|"|`|\\|\n|\$)` → 真实字符（`:501`），先试反转义后直接 `includes`，再试逐块反转义比较 |
| `TrimmedBoundaryReplacer` | `:562-586` | 整体 `trim()`，且仅当 `trimmedFind !== find` 时才有意义（`:565-568`） |
| `ContextAwareReplacer` | `:588-644` | ≥3 行；首/尾行锚定；**块行数必须相等**（`:619`）；中间非空行匹配率 ≥ 0.5（`:635`）；只取第一个命中 |
| `MultiOccurrenceReplacer` | `:548-560` | `while(indexOf)` 产出所有精确匹配，由外层循环决定 `replaceAll` 还是"必须唯一" |

Levenshtein 实现在 `:226-242`（标准二维 DP，`O(n*m)`，注意**没有上限剪枝**，大块文本会有开销）。

**这个设计的精髓**：
1. **严格优先**：模型给的 `oldString` 若精确命中，走 1 号 Replacer，零歧义；
2. **逐级放宽**：缩进/空白/转义这类"模型输出噪声"被 2-8 号吸收，成功率大幅提升；
3. **命中必须唯一**：`index !== lastIndex` → `continue`，即**该 Replacer 产出的子串在文中出现多次就放弃这个 Replacer**，继续往下试；
4. **所有 Replacer 都失败后才报错**，且区分 `notFound`（真找不到）与"找到但多处"（歧义）——两种错误文案完全不同（见下节）。

#### 19.1.4 唯一性校验与三类错误文案（可直接抄）

```ts
// packages/opencode/src/tool/edit.ts:723
if (notFound) {
  throw new Error(
    "Could not find oldString in the file. It must match exactly, including whitespace, indentation, and line endings.",
  )
}
throw new Error("Found multiple matches for oldString. Provide more surrounding context to make the match unique.")
```

| 场景 | 文案 | 行号 | 是否可自愈 |
|------|------|------|-----------|
| `oldString === newString` | `No changes to apply: oldString and newString are identical.` | `:76` `:684` | 模型需换 newString |
| 文件已存在 + `oldString === ""` | `oldString cannot be empty when editing an existing file. Provide the exact text to replace, or use write for an intentional full-file replacement.` | `:93-95` `:687-689` | 引导到 write |
| 文件不存在 | `File ${filePath} not found` | `:124` | 引导 read/glob |
| 路径是目录 | `Path is a directory, not a file: ${filePath}` | `:125` | — |
| 完全找不到 | `Could not find oldString in the file. It must match exactly, including whitespace, indentation, and line endings.` | `:724-726` | 引导重新 read |
| 多处命中 | `Found multiple matches for oldString. Provide more surrounding context to make the match unique.` | `:728` | 引导加上下文 / `replaceAll` |
| 命中块过大（防误替换） | `Refusing replacement because the matched span is much larger than oldString. Re-read the file and provide the full exact oldString for the intended replacement.` | `:710-712` | 引导重新 read |

**"命中块过大"守卫**是很容易被忽略但极重要的一条（`packages/opencode/src/tool/edit.ts:731-737`）：

```ts
function isDisproportionateMatch(search: string, oldString: string) {
  const oldLines = oldString.split("\n").length
  const searchLines = search.split("\n").length
  if (searchLines >= Math.max(oldLines + 3, oldLines * 2)) return true
  if (oldLines === 1) return false
  return search.trim().length > Math.max(oldString.trim().length + 500, oldString.trim().length * 4)
}
```

含义：某个 Replacer 为了"凑上"而锚定了一个远大于原 `oldString` 的区域（行数 ≥ 2×+3，或字符数 ≥ 4×+500）时**拒绝执行**。这防止 `BlockAnchorReplacer` / `ContextAwareReplacer` 这类模糊匹配"顺手吞掉"几百行代码。

#### 19.1.5 replaceAll 的真实语义

`replaceAll` 只在**已经确定 `search` 子串**之后生效（`:714-716`）：

```ts
if (replaceAll) {
  return content.replaceAll(search, newString)
}
```

关键点：`replaceAll` **不要求唯一**。也就是说：
- `replaceAll=true` → 第一個能产出任意命中的 Replacer 就立即返回（把该子串全部替换）；
- `replaceAll=false` → 必须等到某个 Replacer 产出的子串在全文**只出现一次**（`index === lastIndex`）。

这意味着 **`replaceAll=true` 时会跳过更严格的 Replacer 检查**：若 `SimpleReplacer` 命中，`replaceAll` 直接 `replaceAll` 返回；若前 8 个都失败，第 9 个 `MultiOccurrenceReplacer` 会把所有精确命中逐个 yield，非 replaceAll 时因为 `index !== lastIndex` 全被跳过（`notFound` 已被置 false），最终落到"Found multiple matches"错误。

测试用例印证（`packages/opencode/test/tool/edit.test.ts:262`）：`expect(yield* load(filepath)).toBe("qux bar qux baz qux")`。

#### 19.1.6 行尾 / BOM / 编码归一化

编辑前统一把 CRLF 归一化、编辑后按**文件原本的行尾**还原（`packages/opencode/src/tool/edit.ts:129-131`）：

```ts
const ending = detectLineEnding(contentOld)                 // :26-28 含 \r\n 则 CRLF
const old = convertToLineEnding(normalizeLineEndings(params.oldString), ending)
const replacement = convertToLineEnding(normalizeLineEndings(params.newString), ending)
```

`edit.test.ts:300` 有专门的 CRLF 用例（`expect(yield* load(filepath)).toBe("line1\r\nnew\r\nline3")`），`:355-368` 还断言了 LF/CRLF 的计数互斥。

BOM 用独立小模块处理（`packages/opencode/src/util/bom.ts`，27 行）：

```ts
// packages/opencode/src/util/bom.ts:7
export function split(text: string) {
  if (text.charCodeAt(0) !== BOM_CODE) return { bom: false, text }
  return { bom: true, text: text.slice(1) }
}
export function join(text: string, bom: boolean) { ... }
export const readFile = ...   // TextDecoder("utf-8", { ignoreBOM: true })
export const syncFile = ...   // 写回后若 BOM 状态变了，再写一次纠正
```

规则是 `desiredBom = source.bom || next.bom`（`edit.ts:134`）——**原文件有 BOM，或模型新内容带了 BOM，结果就有 BOM**，且 `syncFile` 保证不会双 BOM。`edit.test.ts:172` 断言 `content.charCodeAt(0) === 0xfeff`。

#### 19.1.7 diff 生成与 trimDiff（给模型和 UI 看的都是同一份）

写入前**先算 diff、先过权限、后写盘**（`packages/opencode/src/tool/edit.ts:137-171`）：

```ts
diff = trimDiff(createTwoFilesPatch(filePath, filePath, normalizeLineEndings(contentOld), normalizeLineEndings(contentNew)))
yield* ctx.ask({
  permission: "edit",
  patterns: [path.relative(instance.worktree, filePath)],
  always: ["*"],
  metadata: { filepath: filePath, diff },     // ← 权限弹窗里直接展示 diff
})
yield* afs.writeWithDirs(filePath, Bom.join(contentNew, desiredBom))
if (yield* format.file(filePath)) {           // ← 写入后跑 formatter（prettier/oxfmt 等）
  contentNew = yield* Bom.syncFile(afs, filePath, desiredBom)
}
```

**注意 diff 算了两遍**：格式化器可能改内容，所以 `:164-171` 用格式化的结果重算 diff，保证模型看到的 diff 与磁盘最终状态一致。

`trimDiff`（`:646-680`）把 unified diff 的**公共缩进**剥掉，让 diff 更窄更易读：找出所有 `+/-/ ` 行的最小缩进 `min`，然后每行 `slice(min)`。

additions/deletions 统计用 `diffLines`（`:175-180`），产出 `Snapshot.FileDiff`：

```ts
// packages/opencode/src/tool/edit.ts:181
const filediff: Snapshot.FileDiff = { file: filePath, patch: diff, additions, deletions }
```

#### 19.1.8 并发：per-file 的信号量锁

```ts
// packages/opencode/src/tool/edit.ts:35
const locks = new Map<string, Semaphore.Semaphore>()

function lock(filePath: string) {
  const resolvedFilePath = FSUtil.resolve(filePath)
  const hit = locks.get(resolvedFilePath)
  if (hit) return hit
  const next = Semaphore.makeUnsafe(1)
  locks.set(resolvedFilePath, next)
  return next
}
```

使用处（`:88-172`）：整个「读 → 算 diff → 问权限 → 写 → 格式化 → 重算 diff → 发事件」块被 `lock(filePath).withPermits(1)(...)` 包住。

**注意两点**：
1. **锁的范围不包含权限等待之外的时间**——它确实包住了 `ctx.ask`，即权限弹窗期间锁一直持有（避免用户看 diff 时文件被改）；
2. 这是**进程内锁**（`Map` + `Semaphore`），不跨进程。多窗口 / 外部编辑器同时改文件没有保护。真正的跨进程保护来自 **19.1.13 的 CAS 与 19.1.14 的快照**。

V2 用了更正规的 `KeyedMutex`（`packages/core/src/file-mutation.ts:78-82`）：

```ts
const locks = KeyedMutex.makeUnsafe<string>()
const withTargetLock = (target: Target) => <A, E, R>(effect: Effect.Effect<A, E, R>) =>
  locks.withLock(target.canonical)(Effect.uninterruptible(effect))
```

`Effect.uninterruptible` 是关键——**持锁期间不允许被打断**，避免超时/取消导致锁内半写状态。

#### 19.1.9 写入：V1 非原子，V2 用 CAS（乐观并发）

**V1（当前主力）**：`packages/core/src/fs-util.ts:127-144` 的 `writeWithDirs` —— **直接 `writeFileString`，失败（NotFound）时补建父目录再写一次，没有 tmp + rename**：

```ts
// packages/core/src/fs-util.ts:127
const writeWithDirs = Effect.fn("FileSystem.writeWithDirs")(function* (path, content, mode?) {
  const write = typeof content === "string" ? fs.writeFileString(path, content) : fs.writeFile(path, content)
  yield* write.pipe(
    Effect.catchIf((e) => e.reason._tag === "NotFound", () =>
      Effect.gen(function* () {
        yield* fs.makeDirectory(dirname(path), { recursive: true })
        yield* write
      }),
    ),
  )
  if (mode) yield* fs.chmod(path, mode)
})
```

即：**opencode 生产路径上的文件写入不是原子的**（崩溃可能留下半截文件）。

**V2 引入了真正的乐观并发**（`packages/core/src/file-mutation.ts:144-157`）：

```ts
const writeIfUnchanged = Effect.fn("FileMutation.writeIfUnchanged")((input: ConditionalWriteInput) =>
  withTargetLock(input.target)(
    Effect.gen(function* () {
      const current = yield* fs.readFile(input.target.canonical)
      if (!sameBytes(current, input.expected)) {
        return yield* new StaleContentError({ path: input.target.canonical })
      }
      yield* typeof input.content === "string"
        ? fs.writeFileString(input.target.canonical, input.content)
        : fs.writeFile(input.target.canonical, input.content)
      return writeResult(input.target, true)
    }),
  ),
)
```

调用侧把错误翻成**面向模型的、可自愈的文案**（`packages/core/src/tool/edit.ts:110-119`）：

```ts
error instanceof FileMutation.StaleContentError
  ? new ToolFailure({ message: "File changed after permission approval. Read it again before editing." })
  : new ToolFailure({ message: `Unable to edit ${input.path}` })
```

**这是"编辑冲突"处理的最优解**：不做锁等待、不做 merge，而是"读到的字节 == 期望字节"则提交，否则告诉模型"文件变了，重新读"。对 laew 这种 Rust 实现，`expected: &[u8]` + `std::fs::read` 后 memcmp + `write` 是最直接的对应。

V2 的 `create` 用 `flag: "wx"` 实现真正的"不存在才创建"（`packages/core/src/file-mutation.ts:129-137`），`AlreadyExists` → `TargetExistsError`。V1 没有这个能力。

#### 19.1.10 路径越界：external_directory 权限

所有编辑/写入/读取/grep/glob 都先过 `assertExternalDirectoryEffect`（`packages/opencode/src/tool/external-directory.ts:15-45`）：

```ts
const ins = yield* InstanceState.context
const full = process.platform === "win32" ? FSUtil.normalizePath(target) : target
if (containsPath(full, ins)) return false                 // 在项目内 → 放行
const kind = options?.kind ?? "file"
const dir = kind === "directory" ? full : path.dirname(full)
const glob = process.platform === "win32"
  ? FSUtil.normalizePathPattern(path.join(dir, "*"))
  : path.join(dir, "*").replaceAll("\\", "/")
yield* ctx.ask({ permission: "external_directory", patterns: [glob], always: [glob], metadata: { filepath: full, parentDir: dir } })
```

要点：
- 判定单位是**父目录 + `*`**（不是单文件），所以一次授权覆盖整个外部目录；
- `read` 支持 `bypass`（`:355` `bypass: Boolean(ctx.extra?.["bypassCwdCheck"])`），`edit`/`write`/`grep`/`glob` 传 `bypass: false` 强制拦截；
- 顺序是 **external_directory → 业务权限（edit/read/grep…）**，`core/src/tool/edit.ts:1-5` 的注释明确写了这个顺序。

#### 19.1.11 Write 工具与「Write 前必须 Read」的真相

`packages/opencode/src/tool/write.ts`（104 行）逻辑比 edit 简单得多：读旧内容 → 算 diff → `ask("edit")` → `writeWithDirs` → formatter → 事件 → LSP 诊断。

**重要发现：prompt 里承诺的"必须先 Read"在实现层并没有强制**。

`packages/opencode/src/tool/edit.txt` 第 2 行写着：

```
- You must use your `Read` tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file.
```

`write.txt` 同样写着 "This tool will fail if you did not read the file first."

但通读 `edit.ts` / `write.ts`，**没有任何"检查本会话是否 Read 过该文件"的代码**。全仓库 grep `read before|before edit|mustRead|readFirst` 只命中 `core/src/tool/edit.ts:115` 那条"文件被改过"的错误，与"是否 read 过"无关。

也就是说：**这是纯 prompt 约束（soft contract），不是硬校验**。原因推测：opencode 是本地 CLI，模型 Read 与否的风险远低于"覆盖用户未提交的改动"（后者由 Snapshot 兜底，见 19.1.14）。对比 claude-code 是有硬校验的——这是两者在"编辑安全"上的路线差异。

**另一半真相**：Read 工具确实在**结果层**做了引导（`packages/opencode/src/tool/read.ts:404`）：

```ts
const loaded = yield* instruction.resolve(ctx.messages, filepath, ctx.messageID)
```

`session/instruction.ts:17-32` 的 `extract()` 会扫历史消息里 `tool === "read" && status === "completed"` 的 part，收集 `metadata.loaded` 里的路径。所以 **Read 的"副作用"是登记路径 + 注入 AGENTS.md 之类的指令文件**，而不是门禁。

`write.ts` 还有一个易被忽略的限流（`:18`、`:78-89`）：`MAX_PROJECT_DIAGNOSTICS_FILES = 5` —— 写文件后回灌的 LSP 诊断，**本文件之外的最多只报 5 个文件**，防止一次 write 把整个项目的类型错误全灌进上下文。

#### 19.1.12 ApplyPatch：Codex 补丁语言的完整移植

`apply_patch` 的输入是一个单字符串 `patchText`（`packages/opencode/src/tool/apply_patch.ts:18-20`），语法见 `apply_patch.txt`：

```
*** Begin Patch
*** Add File: hello.txt
+Hello world
*** Update File: src/app.py
*** Move to: src/main.py
@@ def greet():
-print("Hi")
+print("Hello, world!")
*** Delete File: obsolete.txt
*** End Patch
```

**解析器**（`packages/opencode/src/patch/index.ts:185-241`，注释 `:12` 明说"Core types matching the Rust implementation"，即对齐 Codex 的 Rust 实现）：

- `:195-200` 必须有 `*** Begin Patch` / `*** End Patch`，否则 `Invalid patch format: missing Begin/End markers`；
- `:70-100` `parsePatchHeader` 识别 `Add File` / `Delete File` / `Update File` + 可选的 `*** Move to:`；
- `:176-183` `stripHeredoc` 先剥掉 `cat <<'EOF' ... EOF` 外壳（模型经常这么写）；
- `:244-298` `maybeParseApplyPatch` 额外支持 `apply_patch <patch>` 与 `bash -lc 'apply_patch <<"EOF" ...'` 两种 shell 调用形态。

**模糊匹配：四遍 seekSequence**（`packages/opencode/src/patch/index.ts:460-484`）——与 edit 的 9 级 Replacer 是同一思想的另一实现：

```ts
function seekSequence(lines: string[], pattern: string[], startIndex: number, eof = false): number {
  if (pattern.length === 0) return -1
  const exact = tryMatch(lines, pattern, startIndex, (a, b) => a === b, eof)                    // Pass 1 精确
  if (exact !== -1) return exact
  const rstrip = tryMatch(lines, pattern, startIndex, (a, b) => a.trimEnd() === b.trimEnd(), eof) // Pass 2 去尾部空白
  if (rstrip !== -1) return rstrip
  const trim = tryMatch(lines, pattern, startIndex, (a, b) => a.trim() === b.trim(), eof)         // Pass 3 去两端空白
  if (trim !== -1) return trim
  const normalized = tryMatch(lines, pattern, startIndex,                                        // Pass 4 Unicode 标点归一
    (a, b) => normalizeUnicode(a.trim()) === normalizeUnicode(b.trim()), eof)
  return normalized
}
```

`normalizeUnicode`（`:418-425`）把智能引号/破折号/省略号/不换行空格统一成 ASCII——这是模型从富文本里复制代码时最常见的坑：

```ts
.replace(/[‘’‚‛]/g, "'")   // 单引号
.replace(/[“”„‟]/g, '"')   // 双引号
.replace(/[‐‑‒–—―]/g, "-") // 各种破折号
.replace(/…/g, "...")      // 省略号
.replace(/ /g, " ")        // NBSP
```

`tryMatch`（`:429-458`）支持 **`is_end_of_file` 锚定**：若 chunk 标记 EOF，先从 `lines.length - pattern.length` 反向匹配一次。

**替换应用**（`:398-415`）：所有 replacement 先按起始行排序，然后**从后往前** splice，避免索引漂移。

**失败即整体失败**：`computeReplacements` 里任何 chunk 找不到就 `throw`（`:388`），`apply_patch.ts:44-45` 统一包装成 `apply_patch verification failed: ${error}`。**但多文件应用不是事务的**——`apply_patch.ts:220-258` 是 `for (const change of fileChanges) { ...写盘... }` 顺序执行，中途失败会留下部分文件已改。`packages/core/src/file-mutation.ts:205-206` 的 TODO 明确承认了这点：

```ts
// TODO: Design multi-file transactions / rollback if apply_patch needs atomic edits.
// Until then, edits are sequential and report partial application.
```

**apply_patch 的亮点：先算后做、批量授权**（`apply_patch.ts:39-215`）：
1. 解析出全部 hunk；
2. 逐个文件读旧内容 + 推导新内容 + 算 diff + 统计 additions/deletions（`:72-191`）——**全程不写盘**；
3. `ctx.ask` 一次带 `files` 数组（`:194-215`），UI 可以一次性展示"将修改 A/M/D 哪些文件、各多少行"；
4. 授权通过后才 `for` 循环写盘（`:220-258`）。

对比 `edit`：edit 是**一次一个文件一次授权**。对"改 5 个文件"的任务，apply_patch 少 4 次交互。

#### 19.1.13 Snapshot 与编辑工具的联动（undo 的真实机制）

**这是 opencode 最被低估的子系统**：`packages/opencode/src/snapshot/index.ts`（807 行）—— 用一个**完全独立的影子 git 仓库**记录工作区，实现 undo。

关键常量与初始化（`snapshot/index.ts:23-27`、`:71`）：

```ts
const prune = "7.days"                       // gc 保留期
const limit = 2 * 1024 * 1024                // 单文件 > 2MB 的 untracked 文件不进快照
const core = ["-c", "core.longpaths=true", "-c", "core.symlinks=true"]
const cfg  = ["-c", "core.autocrlf=false", ...core]
const quote = [...cfg, "-c", "core.quotepath=false"]
// ...
gitdir: path.join(Global.Path.data, "snapshot", ctx.project.id, Hash.fast(ctx.worktree)),
```

**不改用户的 `.git`**，而是 `--git-dir <data>/snapshot/<projectID>/<hash(worktree)> --work-tree <worktree>`（`:75`）。首次 `track()` 时（`:318-347`）会 `git init` 并按大仓库调优：

```ts
yield* git(["--git-dir", state.gitdir, "config", "core.autocrlf", "false"])
yield* git(["--git-dir", state.gitdir, "config", "core.fsmonitor", "false"])
// Tuning for very large worktrees so the first add stays bounded.
yield* git(["--git-dir", state.gitdir, "config", "feature.manyFiles", "true"])
yield* git(["--git-dir", state.gitdir, "config", "index.version", "4"])
yield* git(["--git-dir", state.gitdir, "config", "index.threads", "true"])
yield* git(["--git-dir", state.gitdir, "config", "core.untrackedCache", "true"])
```

**尊重 .gitignore**（`:102-130`）：用 `git check-ignore --no-index --stdin -z` 批量过滤候选文件，被忽略的文件不进快照、也不会出现在 patch 里（`:369-377`、`:727-733` 两处 "Hide ignored-file removals from the user-facing patch/diff output"）。

`track()`（`:318-347`）产出 `git write-tree` 的 **tree hash** 作为快照 ID。

**与编辑工具的联动点**在 `packages/opencode/src/session/processor.ts`：

```ts
// packages/opencode/src/session/processor.ts:102
const initialSnapshot = yield* snapshot.track()          // 会话处理开始
// ...
case "step-start":
  if (!ctx.snapshot) ctx.snapshot = yield* snapshot.track()   // :425 每个 step 开始前（幂等）
  yield* session.updatePart({ ..., snapshot: ctx.snapshot, type: "step-start" })  // :426-432
  return
case "step-finish": {
  const completedSnapshot = yield* snapshot.track()      // :436 step 结束后再打一个
  // ...
  if (ctx.snapshot) {
    const patch = yield* snapshot.patch(ctx.snapshot)    // :472 diff 出本次 step 改了哪些文件
    if (patch.files.length) {
      yield* session.updatePart({ id: ..., type: "patch", hash: patch.hash, files: patch.files })  // :474-481
    }
    ctx.snapshot = undefined
  }
```

**粒度是 step（一次 LLM 往返 + 其中的所有工具调用），不是单次工具调用**。这一点很重要：
- 优点：快照次数与 step 数成正比，开销可控（一次 step 里改 10 个文件只记一次 patch）；
- 缺点：无法"只撤销第 3 个工具调用"——undo 的最小单位是一次 step。

`cleanup` 分支（`:553-567`）保证异常路径也会 flush patch part。

**撤销**：`packages/opencode/src/session/revert.ts:38-89`：

```ts
rev.snapshot = session.revert?.snapshot ?? (yield* snap.track())      // :70
if (session.revert?.snapshot) yield* snap.restore(session.revert.snapshot)  // :71 先整体回滚到历史快照
yield* snap.revert(patches)                                           // :72 再把"未被撤销的那部分"重放回去
if (rev.snapshot) rev.diff = yield* snap.diff(rev.snapshot)           // :73
```

`revert` 的实现（`snapshot/index.ts:408-524`）是 `git checkout <hash> -- <file>`，并且做了**批量优化**（`:445-457`）：

```ts
const clash = (a: string, b: string) => a === b || a.startsWith(`${b}/`) || b.startsWith(`${a}/`)
for (let i = 0; i < ops.length; ) {
  const run = [first]
  // Only batch adjacent files when their paths cannot affect each other.
  while (j < ops.length && run.length < 100) { ... if (run.some((item) => clash(item.rel, next.rel))) break ... }
  ...
}
```

即：同 hash 的相邻文件，只要路径不构成父子关系就批量 `git checkout`（最多 100 个一批），批失败则回退到单文件。文件在快照里不存在 → 直接 `remove`（`:441-442`）。

`restore`（`:382-406`）是 `git read-tree <snapshot>` + `git checkout-index -a -f`，即**整工作区硬回滚**。

`diffFull`（`:546-759`）产出 UI 用的结构化 diff：`git diff --numstat` 拿 additions/deletions/binary，`structuredPatch(..., { context: Number.MAX_SAFE_INTEGER })` 拿全量 patch（`:737`），100 个一批（`:735`）。

`cleanup` 是常驻后台纤程（`:761-766`）：`Effect.repeat(Schedule.spaced(Duration.hours(1)))` + 初始延迟 1 分钟，跑 `git gc --prune=7.days`。

另外整个 Snapshot 服务被 `Semaphore` 保护（`:55-64` 的 `lock(key)`），并且是 **per-instance**（`InstanceState.make`，`:66`）——多工作区隔离。

`packages/core/src/tool/edit.ts:87` 的 TODO 显示 V2 尚未移植快照：`// TODO: Add snapshots / undo after design exists.`

#### 19.1.14 编辑后 diff 摘要回灌给模型的三条通道

| 通道 | 载体 | 代码位置 | 面向 |
|------|------|----------|------|
| ① 工具结果 | `metadata.diff` + `metadata.filediff` + `output` 里的 LSP 诊断 | `edit.ts:188-211` | 模型（同一轮立即看到） |
| ② patch part | `type: "patch", hash, files[]` 持久化到消息流 | `processor.ts:474-481` / `:557-564` | UI + 后续 revert |
| ③ session diff | `Snapshot.diffFull(from, to)` → `session_diff` 存储 + `Session.Event.Diff` | `summary.ts:82-100`、`revert.ts:76-78` | UI 会话级统计（files/additions/deletions） |

通道 ① 的 `output` 构造（`edit.ts:196-211`）：

```ts
let output = "Edit applied successfully."
yield* lsp.touchFile(filePath, "document")
const diagnostics = yield* lsp.diagnostics()
const block = LSP.Diagnostic.report(filePath, diagnostics[normalizedFilePath] ?? [])
if (block) output += `\n\nLSP errors detected in this file, please fix:\n${block}`
return { metadata: { diagnostics, diff, filediff }, title: `${path.relative(instance.worktree, filePath)}`, output }
```

**注意：模型的工具结果里只回灌了"统一 diff + 增删行数 + 本文件 LSP 错误"，没有把整个新文件内容回灌**（除非开了 formatter 且内容变了）。这是刻意的省 token 设计。

通道 ③ 的快照对（from/to）来自 step-start / step-finish part 上挂的 snapshot 字段（`summary.ts:85-99`），所以 **diff 统计天然是"每个 assistant step 一组"**，与 19.1.13 的粒度一致。

#### 19.1.15 Notebook 支持：明确没有

全仓库（`packages/opencode/src`、`packages/core/src`）grep `ipynb|notebook` **零命中**。opencode 没有 Jupyter 编辑器，`.ipynb` 会走普通 JSON 文本路径（`edit`/`write` 按字符串处理）。对比 claude-code 有 `NotebookEditTool`。这是 opencode 的能力缺口，但对 laew 也意味着：**不必优先投入**。

#### 19.1.16 维度一 对 laew 的借鉴

laew 现状（参照 CLAUDE.md）：`src/agent/tools/write.rs` 只有 Write（全量覆盖），**没有 Edit、没有 ApplyPatch、没有快照/undo**。

| 优先级 | 建议 | 依据 | Rust 落地要点 |
|--------|------|------|--------------|
| **P0** | **实现 `edit` 工具：`oldString/newString/replaceAll`** | `edit.ts:682-737` | 先只做 `SimpleReplacer` + 唯一性校验（`matches == 1` 才改，0 → "找不到"，>1 → "多处命中"），错误文案照抄 19.1.4 表格。约 120 行 Rust |
| **P0** | **工具结果的错误文案必须"可自愈"** | `edit.ts:724/728`、`core/src/tool/edit.ts:115` | 错误里明确告诉模型下一步动作（重新 Read / 加上下文 / 用 write / 用 replaceAll）。laew 当前 Write 失败只回裸 io::Error |
| **P0** | **写前算 diff + 回灌 diff 摘要而非全文** | `edit.ts:137-186` | 用 `similar` crate 或手写 LCS；回灌 `+N/-M` 与 20 行上下文即可 |
| **P1** | **模糊回退链（至少 3 级）** | `edit.ts:694-703` | 建议只移植 `LineTrimmed` + `WhitespaceNormalized` + `BlockAnchor(Levenshtein)` 三级；Rust 侧 Levenshtein 可用 `strsim` 或 30 行 DP |
| **P1** | **`isDisproportionateMatch` 守卫** | `edit.ts:731-737` | 8 行代码，防"模糊匹配吞掉半个文件"的灾难性误替换，性价比极高 |
| **P1** | **BOM / CRLF 归一化** | `edit.ts:26-33`、`util/bom.ts` | Rust 侧 `detect_line_ending` + 写回时还原；BOM 用 `u8` 前缀判定（0xEF 0xBB 0xBF） |
| **P1** | **per-file 互斥锁** | `edit.ts:35-45` | `DashMap<PathBuf, tokio::sync::Semaphore>`（信号量 permits=1）；laew 的 SubAgent 并发执行单元改同一文件时会踩坑 |
| **P1** | **影子 git 快照（undo）** | `snapshot/index.ts:71/318-347` | 用 `git2` crate 或直接 `Command::new("git")`；`--git-dir` 指向 `LsmAgentEmergentWork.db` 同级的 `snapshot/<hash>`。**laew 已有 SQLite，可以把 tree hash 存 `session_memory` 表** |
| **P2** | **CAS 乐观并发写** | `core/src/file-mutation.ts:144-157` | `read → memcmp(expected) → write`，冲突报"文件已变更，请重新 Read"。比加锁简单且与锁互补 |
| **P2** | **apply_patch 工具（GPT 系模型专用）** | `registry.ts:297-302` | laew 接了 OpenAI 协议（含 gpt-5 类模型），可按 `model_name.contains("gpt-")` 切工具集；解析器可参考 `patch/index.ts:185-241` 移植 |
| **P2** | **写入后跑 formatter** | `edit.ts:112-114`、`:156-158` | laew 是 Rust 项目，可 hook `cargo fmt`/`rustfmt`；注意格式化后要**重算 diff** |
| **P2** | **`external_directory` 越界授权** | `external-directory.ts:15-45` | laew 的"根目录 ≠ 工作目录"双路径设计天然需要这条；授权粒度用"父目录 + *" |
| — | **不建议** 优先做 Notebook | 全仓库零命中 | 投入产出比低 |

**对 laew 的最小可落地切片（P0，约 1 天）**：
1. 新增 `src/agent/tools/edit.rs`，参数 `file_path / old_string / new_string / replace_all`；
2. 读文件 → `matches = content.matches(old_string).count()`；
3. `0` → 报 "Could not find oldString…"；`>1 && !replace_all` → 报 "Found multiple matches…"；
4. 写回前 `create_dir_all(parent)`；
5. 结果回灌 `format!("Edit applied successfully.\n+{} -{}", add, del)`。

---

### 19.2 维度二：代码检索与索引

opencode 把"代码检索与索引"拆成**三档工具**与**一条外挂二进制**，组合清晰：

| 工具 | 主要场景 | 后端 | 行号 |
|------|----------|------|------|
| `glob` | 按文件名 / 通配符找文件 | ripgrep `--files --glob` | `packages/opencode/src/tool/glob.ts` (76) |
| `grep` | 按正则搜文件内容 | ripgrep `--json` | `packages/opencode/src/tool/grep.ts` (115) |
| `lsp` | 定义 / 引用 / hover / 符号树 | LSP 客户端（按文件后缀启 server） | `packages/opencode/src/tool/lsp.ts` (113) |
| （底层） | 上述三者都依赖的 ripgrep 适配器 | `packages/core/src/ripgrep.ts` (284) | — |
| （外挂） | ripgrep 二进制自举 | `packages/core/src/ripgrep/binary.ts` (131) | — |

#### 19.2.1 ripgrep 二进制自举（不内嵌 ripgrep，全部走外挂）

`packages/core/src/ripgrep/binary.ts:14-23` 把"用哪个 ripgrep"做成一个**查找优先级表**：

```ts
// packages/core/src/ripgrep/binary.ts:14
const VERSION = "15.1.0"
const PLATFORM = {
  "arm64-darwin": { platform: "aarch64-apple-darwin", extension: "tar.gz" },
  "arm64-linux": { platform: "aarch64-unknown-linux-gnu", extension: "tar.gz" },
  "x64-darwin":  { platform: "x86_64-apple-darwin", extension: "tar.gz" },
  "x64-linux":   { platform: "x86_64-unknown-linux-musl", extension: "tar.gz" },
  "arm64-win32": { platform: "aarch64-pc-windows-msvc", extension: "zip" },
  "ia32-win32":  { platform: "i686-pc-windows-msvc", extension: "zip" },
  "x64-win32":   { platform: "x86_64-pc-windows-msvc", extension: "zip" },
} as const
```

查找顺序（`packages/core/src/ripgrep/binary.ts:92-122`）：

```ts
// packages/core/src/ripgrep/binary.ts:92
const system = yield* Effect.sync(() => which(process.platform === "win32" ? "rg.exe" : "rg"))
if (system && (yield* fs.isFile(system).pipe(Effect.orDie))) return system
// 1. 系统 PATH
const target = path.join(Global.Path.bin, `rg${process.platform === "win32" ? ".exe" : ""}`)
if (yield* fs.isFile(target).pipe(Effect.orDie)) return target
// 2. 全局 bin 目录已下载
// 3. 按 platformKey 在 PLATFORM 表里查；查不到 → throw unsupported
const filename = `ripgrep-${VERSION}-${config.platform}.${config.extension}`
const url = `https://github.com/BurntSushi/ripgrep/releases/download/${VERSION}/${filename}`
// 4. 下载 + 解压（tar.gz → tar / zip → powershell Expand-Archive）
```

**关键设计**：

1. **`Effect.cached` 包住整个查找链**（`:92`）—— 同一进程多次调用 `ripgrep.filepath()` 不会重复下载；第二次拿到的就是 `Effect.succeed(filepath)` 的纯缓存值。
2. **`Global.Path.bin`** 跨用户共享；下载/解压完成后 `fs.remove(archive, { force: true })`（`:119`）立刻清掉压缩包，只留解压产物。
3. **zip / tar.gz 双后端**（`:58-89`）—— Windows 上 fallback 到 `powershell.exe`/`pwsh.exe` + `Expand-Archive`，POSIX 走 `tar -xzf`；两条路径都用 `[stdout, stderr, code] = Effect.all([...], { concurrency: "unbounded" })` 并发读双流，避免 stderr 卡死 stdout。
4. **不内嵌 rg**：opencode 选了"下载现成二进制"而不是 vendor ripgrep 源码进 Bun 编译产物 —— **进程模型简单**（spawn + JSON 流），但代价是首次启动有网络依赖。

#### 19.2.2 ripgrep core 适配器：JSON 流式解析 + 截断魔法

`packages/core/src/ripgrep.ts` 把 ripgrep 当成一个**异步 Stream** 来消费：

```ts
// packages/core/src/ripgrep.ts:18
const ERROR_BYTES = 8 * 1024
const MAX_RECORD_BYTES = 64 * 1024
const MAX_SUBMATCHES = 100
```

**核心 run() 函数**（`:98-152`）：

```ts
const handle = yield* process.spawn(
  ChildProcess.make(yield* binary.filepath, input.args, {
    cwd: input.cwd, extendEnv: true, stdin: "ignore"
  }),
)
const stderrFiber = yield* collectStream(handle.stderr, ERROR_BYTES).pipe(Effect.forkScoped)
// ↑ stderr 只取前 8KB，防 ripgrep 自己输出炸掉

let observed = 0
const rows = yield* Stream.decodeText(handle.stdout).pipe(
  Stream.splitLines,                                                  // 按 \n 分行
  Stream.filter((line) => line.length > 0),
  Stream.mapEffect(input.parse),                                      // 每行 JSON.parse + Schema.decode
  Stream.filter((row): row is A => row !== undefined),
  Stream.tap((row) => {
    if (!input.onItem || observed++ >= input.limit) return Effect.void
    return input.onItem(row)
  }),
  Stream.take(input.limit + 1),                                       // ← 多取一行用于检测截断
  Stream.runCollect,
)
const truncated = rows.length > input.limit
if (truncated) return { items: rows.slice(0, input.limit), truncated, partial: false }
const code = yield* handle.exitCode
if (input.pattern && code === 2 && isInvalidPattern(stderr))
  return yield* new InvalidPatternError({ pattern: input.pattern, message: stderr.trim() })
if (code !== 0 && code !== 1 && code !== 2)
  return yield* failure(stderr.trim() || `ripgrep failed with code ${code}`)
return { items: code === 1 ? [] : rows, truncated: false, partial: code === 2 }
```

**亮点 1：`Stream.take(input.limit + 1)` 截断魔法**。多取 1 条只是用来**事后判定** `truncated = rows.length > input.limit`，这是经典的 "fence-post" 技巧 —— 不要边消费边判定，因为消费者要拿到前 N 条完整数据才能告知模型"截了"。

**亮点 2：ripgrep 退出码语义**。`code === 1` 是 "no matches found"（正常）；`code === 2` 是 "regex invalid OR partial search"（partial 表示二进制文件干扰）；其它 → 真错误。这个区分让 `grep` 工具能区分"没找到" vs "出错"。

**亮点 3：单行字节上限 64 KB**（`:233`）。如果 ripgrep 某行 JSON 输出超过 64 KB（一般是 huge binary match），**直接 fail** 而不是默默截断 —— 失败比"显示半行 JSON"对模型更友好。

**亮点 4：submatch 截断到 100**（`:247`）和**单行文本截断到 2000 字符**（`:268`），都是为了防止模型侧被一行 huge match 炸到。

#### 19.2.3 glob：走 `--files --glob` 而非 `find`

`packages/core/src/ripgrep.ts:155-186` glob 实现：

```ts
args: [
  "--no-config",
  "--files",
  ...(input.hidden ? ["--hidden"] : []),
  ...(input.follow ? ["--follow"] : []),
  `--glob=${input.pattern}`,
  "--glob=!**/.git/**",   // ← 永远忽略 .git
  ".",
],
parse: (line) =>
  Effect.succeed(
    line.replace(/^(?:\.[\\/])+/u, "")   // 去掉 ./ 前缀
        .replace(/^[\\/]+/u, "")          // 去掉绝对路径前缀
        .replaceAll("\\", "/"),           // Windows → POSIX 路径
  ),
```

**洞察**：opencode **默认不传 `--no-ignore`** —— ripgrep 会自动尊重 `.gitignore` / `.ignore` / global gitignore。这与 `find` 的行为完全不同（`find` 默认全部找）。代价是 model 写 glob 时**默认看不见 `node_modules`、`.git`、`target`**，但这是绝大多数情况下用户期望的语义。

`tool/glob.ts:49-63` 加上 `limit = 100`、truncated 文案、path 必须目录（`:40-43`）：

```ts
if (info?.type === "File") {
  throw new Error(`glob path must be a directory: ${search}`)
}
```

#### 19.2.4 grep：JSON 输出 + 行号聚合 + `include` glob

`packages/core/src/ripgrep.ts:218-279` grep args：

```ts
args: [
  "--no-config",
  "--json",          // ← 让 ripgrep 输出 JSON，每行一条 record
  "--hidden",        // ← 强制搜隐藏文件（但仍尊重 gitignore）
  "--no-messages",   // 屏蔽 "regex invalid" 之类的 stderr 噪音
  ...(input.include ? [`--glob=${input.include}`] : []),
  "--glob=!**/.git/**",
  "--",
  input.pattern,
  input.file ?? ".",
],
parse: (line) => /* Schema.decodeUnknownEffect(RawMatch) ... */
```

`grep.ts` 输出格式（`:84-97`）—— 按文件聚合、相同路径只输出一次：

```ts
let current = ""
for (const match of final) {
  if (current !== match.path) {
    if (current !== "") output.push("")
    current = match.path
    output.push(`${match.path}:`)
  }
  output.push(`  Line ${match.line}: ${match.text}`)
}
if (truncated) output.push("(Results truncated. Consider using a more specific path or pattern.)")
```

**与 laew 的 grep 对比**：opencode **强制截断 limit = 100**，并且**截断时明确告诉模型"换更精确的 path 或 pattern"** —— 这正是 laew 的 grep 工具目前缺失的能力。

#### 19.2.5 LSP：9 种操作 + 自动 server bootstrap

`packages/opencode/src/tool/lsp.ts:11-21` 列出了 9 种 LSP 操作：

```ts
const operations = [
  "goToDefinition",
  "findReferences",
  "hover",
  "documentSymbol",
  "workspaceSymbol",
  "goToImplementation",
  "prepareCallHierarchy",
  "incomingCalls",
  "outgoingCalls",
] as const
```

**这是 opencode 相对 laew 的最大能力差** —— laew 完全没有 LSP 集成。

参数 Schema（`:23-35`）非常严格：

```ts
line: Schema.Int.check(Schema.isGreaterThanOrEqualTo(1)).annotate({
  description: "The line number (1-based, as shown in editors)",
}),
character: Schema.Int.check(Schema.isGreaterThanOrEqualTo(1)),
query: Schema.optional(Schema.String),    // workspaceSymbol 专用
```

执行流程（`:45-109`）：

1. `path.isAbsolute(args.filePath) ? ... : path.join(instance.directory, ...)` —— 相对路径基于 instance.worktree；
2. `assertExternalDirectoryEffect` 越界校验；
3. `ctx.ask({ permission: "lsp", patterns: ["*"], always: ["*"] })` —— **每次 LSP 调用都要求权限**（即使 read-only 也弹权限）—— 这是一个保守设计；
4. `lsp.hasClients(file)` 检查后缀是否有 server（`:77`）→ 没有就报 "No LSP server available for this file type"；
5. `lsp.touchFile(file, "document")` 把这次访问告诉 LSP server（让 server 真正去 load 文件）；
6. 分派到 9 种 LSP request；
7. 结果 `JSON.stringify(result, null, 2)` 返回（无任何格式化压缩）。

**自动 server bootstrap** 在 `packages/opencode/src/lsp/lsp.ts`（507 行）。opencode 通过**项目根目录的配置文件**告诉它哪些后缀启哪些 server（如 `typescript-language-server` 对应 `.ts/.tsx/.js/.jsx`），`Server.lookup(file)` 根据后缀匹配并启动 stdio client。**LSP 是 V1 才有的能力**，V2 core TODO 列表里写明"待迁移"（`core/src/tool/edit.ts:88`）。

#### 19.2.6 不存在的部分：embedding / 向量索引 / 大仓库特殊优化

**全仓库（`packages/opencode/src`、`packages/core/src`）grep `embedding|sqlite-vec|vector` 命中都在 GitHub Copilot 集成的 OpenAI Responses API 文件里**（`packages/core/src/github-copilot/responses/openai-responses-prepare-tools.ts` 等），是 **GitHub Copilot 的"file search"工具能力**，不是 opencode 自带的代码检索增强。opencode 主体**没有**本地 embedding / 向量索引。

**大仓库性能优化**有两个工程细节：

1. **snapshot seed alternates**（`packages/opencode/src/snapshot/index.ts:198-233`）—— 快照首次 init 时把源仓库的 `objects/info/alternates` 链复制到 shadow git 的 alternates，使 huge repo（如 chromium）的**已存在 blob 通过 ODB 共享**而非重新 `git add`，避免 `git hash-object` 数分钟；并复制 `index` 文件复用已有 hash。
2. **ripgrep 选 `--no-messages`**（`packages/core/src/ripgrep.ts:225`）—— 大仓库下 "Permission denied" / "binary file X matches" 之类的 stderr 噪音会显著拖慢启动，关掉它即可。

#### 19.2.7 维度二 对 laew 的借鉴

laew 现状（参照 CLAUDE.md）：`src/agent/tools/` 目前只有 `bash.rs / read.rs / write.rs` —— **没有 glob、没有 grep、没有 LSP**。文件检索靠 model 自己用 `bash rg`，性能 / 截断 / gitignore 都不一致。

| 优先级 | 建议 | 依据 | Rust 落地要点 |
|--------|------|------|--------------|
| **P0** | **新增 `grep` 工具**：参数 `pattern/path/include/limit`，spawn `rg --json --no-messages`，按文件聚合输出 | `packages/core/src/ripgrep.ts:218-279`、`tool/grep.ts:84-97` | 用 `tokio::process::Command`；line stream 用 `BufReader::lines()` + `serde_json`；limit 默认 100，截断时回灌"换更精确 pattern" |
| **P0** | **新增 `glob` 工具**：参数 `pattern/path`，spawn `rg --files --glob` | `packages/core/src/ripgrep.ts:155-186`、`tool/glob.ts:49-63` | 复用 ripgrep 路径解析；path 必须是目录否则报错 |
| **P0** | **ripgrep 二进制自举** | `packages/core/src/ripgrep/binary.ts:92-122` | 用 `which::which("rg")` crate；若失败则按 `target_triple` 从 GitHub release 下载到 `~/.laew/bin/`；`tokio::sync::OnceCell` 做全局缓存 |
| **P1** | **截断 fence-post `take(limit + 1)`** | `ripgrep.ts:126-131` | Rust 侧 `rows.truncate(limit+1); let truncated = rows.len() > limit; rows.truncate(limit);` |
| **P1** | **schema 强校验工具参数** | `grep.ts:10-18` 用 `Schema.Struct` 严格声明 | laew 的 Bash/Read/Write 当前是手写 struct；建议用 `schemars` crate 生成 JSON Schema 给 LLM 用，但内部仍用 `serde::Deserialize` 校验 |
| **P1** | **子匹配 / 行字节上限** | `ripgrep.ts:233-247` | 单行 JSON > 64 KB fail；单行文本 > 2KB 截断；submatch > 100 截断 |
| **P1** | **`include` glob 参数** | `grep.ts:15-17`、`ripgrep.ts:226` | `grep "*.{ts,tsx}"` 这种语法；model 友好 |
| **P1** | **`--no-messages` + stderr 8KB 截断** | `ripgrep.ts:19, 112-114, 225` | 防止 rg 自身 stderr 噪音污染模型上下文 |
| **P2** | **LSP 工具集成** | `tool/lsp.ts:11-21, 82-103` | 引入 `tower-lsp` crate（或 `lsp-types` + 自管 client），按后缀启 server；9 种 operation 都暴露。投入大，建议先做 `goToDefinition + documentSymbol + findReferences` 三种 |
| **P2** | **embedding / 向量索引** | 全仓库无命中 | laew 可以做轻量版：`ripgrep` + SQLite FTS5 全文索引（`rusqlite` crate 内置）；向量索引当前阶段不必做 |
| **P2** | **大仓库 alternates 共享** | `snapshot/index.ts:198-233` | 仅在引入 git 快照后才有意义；先不做 |

**对 laew 的最小可落地切片（P0，约 1 天）**：

1. `src/agent/tools/grep.rs`：spawn `rg --json --no-messages --glob=!**/.git/** -e <pattern> -- <path>`，逐行 `serde_json::from_str` → 按文件聚合 → 输出 `\nfile:\n  Line N: <text>\n`；
2. `src/agent/tools/glob.rs`：spawn `rg --files --glob=<pattern> --glob=!**/.git/** <path>`，直接按行输出；
3. 在 `rebuild_restart_app.sh` 中加 `which rg || curl -L <release-url> | tar xz -C ~/.laew/bin/` 自举；
4. 两者都用 `tokio::sync::OnceCell<PathBuf>` 缓存 `rg` 路径。

### 19.3 维度三：结构化输出与 Schema 校验

opencode 把"模型契约"做成三段：**Schema 定义 → JSON Schema 归一化 → provider 差异适配**。三段都被集中在一处（`packages/opencode/src/tool/json-schema.ts` + `packages/opencode/src/provider/transform.ts:1546-1686`），而不是分散在每个 provider 客户端里。

#### 19.3.1 三段式架构

```
┌─────────────────────┐   fromSchema()   ┌──────────────────┐   schema()   ┌────────────────┐
│ Effect.Schema.Struct │ ───────────────▶ │ JSON Schema 7    │ ───────────▶ │ provider wire  │
│ (TS 类型 + 运行时)   │                 │ (draft-2020-12)  │              │ (OpenAI/Gemini │
│                     │                 │  + inline + 归一化 │              │  Moonshot/...) │
└─────────────────────┘                 └──────────────────┘              └────────────────┘
```

#### 19.3.2 fromSchema：Effect Schema → JSON Schema + 归一化（核心 164 行）

`packages/opencode/src/tool/json-schema.ts:8-22` 是入口：

```ts
const cache = new WeakMap<Schema.Top, JSONSchema7>()          // 按 Schema 实例缓存（GC 友好）

export function fromSchema(schema: Schema.Top): JSONSchema7 {
  const cached = cache.get(schema)
  if (cached) return cached

  const document = Schema.toJsonSchemaDocument(schema, { additionalProperties: true })
  const result = normalize({
    $schema: JsonSchema.META_SCHEMA_URI_DRAFT_2020_12,
    ...document.schema,
    ...(Object.keys(document.definitions).length > 0 ? { $defs: document.definitions } : {}),
  })
  const inlined = dropDefinitionsIfResolved(inlineLocalReferences(result))
  if (!isJsonSchema(inlined)) throw new Error("tool JSON Schema helper produced a non-schema value")
  cache.set(schema, inlined)
  return inlined
}
```

**关键设计**：

1. **`WeakMap` 缓存** —— 用 `Schema.Top` 对象做 key，Schema 实例被 GC 时缓存自动失效，避免内存膨胀；**但前提是 Schema 定义模块不能被反复 import（否则拿到的是不同对象实例）**—— opencode 是 SSR，所有 Schema 都是模块顶层常量，所以这个假设成立。laew 的 Rust 端没有 GC 但可以用 `OnceCell<HashMap<TypeId, Arc<JSONSchema>>>` 模拟。
2. **`Schema.toJsonSchemaDocument`** —— Effect 4.x 的官方方法，把 `Schema.Struct(...)` 完整转成 JSON Schema（含 `$defs`）。
3. **归一化 → $ref 展平 → $defs 清理**三步流水线。

**normalize() 函数**（`:28-88`）做**6 种归一化**：

| 规则 | 目的 | 行号 |
|------|------|------|
| `additionalProperties: true` 删除 | 大多数 provider 把它当成"严格"，多余的反而污染 | `:49` |
| 可选字段的 `anyOf: [T, { type: "null" }]` 去 null | 模型更愿意填 null 而非省略 | `:51-54` |
| `anyOf: [{type:"number"}, {enum:["NaN","Infinity",...]}]` 简化成 `{type:"number"}` | Effect Schema 用 stringy enum 表示非有限数 | `:56-65` |
| 空结构 union → `{type:"object",properties:{}}` | `Schema.Union(Schema.Struct({}), Schema.Array(Schema.Unknown))` 这种 | `:67-70` |
| `anyOf` 只有一项 → 展开 | 减少深度 | `:72-75` |
| `allOf` 可展平 → 合并对象 | 同名 key 不冲突时 | `:78-81` |
| `integer` 缺 min/max → `[MIN_SAFE_INTEGER, MAX_SAFE_INTEGER]` | 防止精度推断歧义 | `:83-85` |

**inlineLocalReferences() 函数**（`:121-144`）递归展开 `$ref`：

```ts
function inlineLocalReferences(value: unknown, definitions?: JsonObject, seen = new Set<string>()): unknown {
  if (Array.isArray(value)) return value.map((item) => inlineLocalReferences(item, definitions, seen))
  if (!isRecord(value)) return value
  const localDefinitions = definitions ?? (isRecord(value.$defs) ? value.$defs : undefined)
  if (typeof value.$ref === "string" && localDefinitions) {
    const name = value.$ref.match(/^#\/\$defs\/(.+)$/)?.[1] ?? value.$ref.match(/^#\/definitions\/(.+)$/)?.[1]
    if (name && !seen.has(name)) {
      const target = localDefinitions[name]
      if (target) {
        const { $ref: _, ...rest } = value
        return inlineLocalReferences(
          { ...(isRecord(target) ? target : {}), ...rest },     // ← target 在前，rest 在后，允许覆盖
          localDefinitions,
          new Set(seen).add(name),
        )
      }
    }
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, inlineLocalReferences(item, localDefinitions, seen)]),
  )
}
```

**亮点**：用 `seen` Set 防**循环引用**（Schema A 内嵌 Schema B，B 又嵌 A）；`{...target, ...rest}` 的顺序保证 `$ref` 上 sibling 字段（如 `description`）优先于被引用对象自身的同名字段。

**dropDefinitionsIfResolved()**（`:146-150`）在所有 `$ref` 都展开完了之后，把 `$defs` / `definitions` 整块删掉 —— provider 看到这些空定义会困惑。

#### 19.3.3 provider 差异适配：sanitizeOpenAISchema + sanitizeGemini + sanitizeMoonshot

`packages/opencode/src/provider/transform.ts:1546-1686` 是 provider 差异集中处。三个 sanitize 路径都遵循同一模式：**白名单关键字 + 强制 type + 删 sibling keywords**。

**sanitizeOpenAISchema**（`:1463-1544`）—— OpenAI strict mode：

```ts
const types = ["string", "number", "boolean", "integer", "object", "array", "null"]
const compositionKeys = ["anyOf", "oneOf", "allOf"]

// OpenAI 工具 schema 不接受 boolean 形式（true/false 整个 schema）
if (typeof value === "boolean") return { type: "string" }

// 仅白名单这些关键字透传：$ref、description、enum/const、properties、required、items、
// additionalProperties、anyOf/oneOf/allOf、$defs/definitions
```

最巧妙的是**类型推断**（`:1523-1536`）：

```ts
// MCP server 经常省略 type 但保留 properties/required/items → 推断为 object/array/string/number
const inferredTypes =
  schemaTypes.length > 0
    ? schemaTypes
    : ["properties", "required", "additionalProperties"].some((key) => key in value)
      ? ["object"]
      : ["items", "prefixItems"].some((key) => key in value)
        ? ["array"]
        : "enum" in result || "format" in value
          ? ["string"]
          : ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf"].some((key) => key in value)
            ? ["number"]
            : []
if (inferredTypes.length === 0) return {}      // ← 完全没法推断则返回 {}
result.type = inferredTypes.length === 1 ? inferredTypes[0] : inferredTypes
if (inferredTypes.includes("object") && !("properties" in result)) result.properties = {}
if (inferredTypes.includes("array") && !("items" in result)) result.items = { type: "string" }
```

**洞察**：这是从 MCP server 拿到的"残缺 schema"做标准化 —— MCP 协议允许 server 不写 `type`，但 OpenAI/Gemini 不接受缺 type 的 object。opencode 不报错而是补全默认。

**sanitizeMoonshot**（`:1570-1586`）：

```ts
if ("$ref" in obj && typeof obj.$ref === "string") return { $ref: obj.$ref }
// 任何 $ref 节点上 sibling keywords（如 description）都丢
// MFJS 不支持 tuple-style items，要求单一 schema
if (Array.isArray(result.items)) result.items = result.items[0] ?? {}
```

**洞察**：Moonshot (Kimi) 的合规验证是"行为一致但语法更严格"，所以 sibling keywords 反而有害 —— 这是个**反直觉但很常见**的 provider 特性。

**sanitizeGemini**（`:1589-1682`）—— 处理最复杂：

```ts
// 1. enum 全部转字符串；整数 enum 改 type 为 string
if (key === "enum" && Array.isArray(value)) {
  result[key] = value.map((v) => String(v))
  if (result.type === "integer" || result.type === "number") result.type = "string"
}

// 2. type 数组（如 ["number","string"]）拆成 anyOf；含 null → nullable:true
if (Array.isArray(result.type)) {
  const hasNull = result.type.includes("null")
  const nonNull = result.type.filter((entry) => entry !== "null")
  if (nonNull.length === 0) result.type = "null"
  else {
    delete result.type
    result.anyOf = nonNull.map((entry: unknown) => ({ type: entry }))
    if (hasNull) result.nullable = true
  }
}

// 3. required 只保留 properties 里存在的字段
if (result.type === "object" && result.properties && Array.isArray(result.required)) {
  result.required = result.required.filter((field: any) => field in result.properties)
}

// 4. array 没 items 补 { items: {} }；items 无 schema intent → 默认 type:string
if (result.type === "array" && !hasCombiner(result)) {
  if (result.items == null) result.items = {}
  if (isPlainObject(result.items) && !hasSchemaIntent(result.items)) {
    result.items.type = "string"
  }
}

// 5. 非 object 类型删 properties/required
if (result.type && result.type !== "object" && !hasCombiner(result)) {
  delete result.properties
  delete result.required
}
```

**洞察**：每一条都是 Gemini "AI SDK default" 与 "OpenAI-compatible 透传"行为不一致的具体修复 —— `:1640-1645` 注释明确说 `plain @ai-sdk/google 会把 type 数组拆 anyOf，但 OpenAI-compatible 透传（如 Copilot proxy Gemini）会原样转发并被 backend 拒`。

**Bedrock 不在 schema() 列表里**。`packages/opencode/src/provider/transform.ts:1546` 函数只为 4 类 provider 做 sanitize（OpenAI / Azure / Moonshot / Gemini）；Bedrock / Anthropic / Vertex 走的是 AI SDK 默认实现 + 自身的 strict mode。这意味着**对 Bedrock 的特殊 schema 限制（如某些 `$ref` 结构）opencode 不主动修复，靠 AI SDK 兜底** —— 这是 opencode 自己的"待办"（第六轮文档已提及）。

#### 19.3.4 请求前 schema 校验 + 错误自修复路径

V1 在 `packages/opencode/src/tool/tool.ts:107-145` 集中处理工具参数 decode：

```ts
const decode = Schema.decodeUnknownEffect(toolInfo.parameters)   // ← 预编译一次

toolInfo.execute = (args, ctx) => {
  return Effect.gen(function* () {
    const decoded = yield* decode(args).pipe(
      Effect.mapError(
        (error) => new InvalidArgumentsError({
          tool: id,
          detail: toolInfo.formatValidationError
            ? toolInfo.formatValidationError(error)               // ← 工具可自定义错误格式
            : String(error),
        }),
      ),
    )
    const result = yield* execute(decoded as Schema.Schema.Type<Parameters>, ctx)
    // ...
  })
}
```

**三个细节**：

1. **`Schema.decodeUnknownEffect` 预编译闭包**（`:111`）—— 注释明确说"per-call allocate 太贵，hoist 一次"。这是**性能优化**而非可读性。
2. **`formatValidationError` 自定义** —— 每个工具可以覆写 `InvalidArgumentsError` 的 `detail` 字符串，让错误信息更可读（参考 `packages/opencode/src/tool/invalid.ts:32` 的固定文案 "The {tool} tool was called with invalid arguments: {detail}. Please rewrite the input so it satisfies the expected schema."）。
3. **`InvalidArgumentsError` 类型本身** —— 让上层（processor）能识别"参数错"并把消息回传给模型，模型重新生成合规参数。**这是 schema 校验失败的"自修复"机制**：模型拿到错误 → 重新生成工具调用 → 再次校验。

**invalid 工具**（`packages/opencode/src/tool/invalid.ts:9-21`）是兜底 —— 当模型声明了一个不存在的工具名时：

```ts
export const InvalidTool = Tool.define(
  "invalid",
  Effect.succeed({
    description: "Do not use",
    parameters: Parameters,
    execute: (params) => Effect.succeed({
      title: "Invalid Tool",
      output: `The arguments provided to the tool are invalid: ${params.error}`,
      metadata: {},
    }),
  }),
)
```

**洞察**：把 invalid 当成一个真实工具注册，让"模型声明未知工具"的边缘情况也走标准 tool result 通路，而不是 throw 抛到 processor 走错误分支。**一致性优先于错误类型层级**。

#### 19.3.5 没有的部分：partial / 截断 JSON 修复、模型返 JSON 的容错解析

**全仓库 grep `parsePartialJson|jsonrepair|partial.?json` 零命中**。opencode 的策略是：

- 工具参数 JSON 由模型自己生成（Anthropic `tool_use.input` / OpenAI `tool_calls[].function.arguments`）—— 这些字段由 **AI SDK / Anthropic SDK 客户端解析**，opencode 不再二次解析；
- 工具**返回值**模型从不看（返回的是工具真实产物 + `output` 字符串）；
- 结构化输出（`tool.output` Schema）目前是**V2 才有的能力**（`packages/core/src/tool/bash.ts:41-46`），且只用于 bash（exit/truncated/timeout 三元组）。

这意味着 **opencode 没有专门的 partial-JSON 修复库**（如 jsonrepair 或 partial-json）—— 因为它的工具参数流是**模型直出 SDK 客户端**，两端契约清晰；如果模型产出非法 JSON，SDK 客户端抛错 → 包成 `InvalidArgumentsError` → 模型自修复重试。

#### 19.3.6 维度三 对 laew 的借鉴

laew 现状：工具参数是 `serde::Deserialize` + `serde_json::from_str(...)`，错误信息是裸 JSON parse 错误，模型看到 `expected `,` or `}` at line 5 column 12` 这种字符串无法自修复。

| 优先级 | 建议 | 依据 | Rust 落地要点 |
|--------|------|------|--------------|
| **P0** | **工具参数 decode 失败时输出"期望字段 + 实际字段"对照** | `tool.ts:107-145`、`invalid.ts:32` | 引入 `schemars` crate 给每个工具生成 JSON Schema，错误信息里附 `expected: { "filePath": string, "oldString": string }`，`actual: missing 'oldString'` |
| **P0** | **集中 Schema → JSON Schema 归一化** | `json-schema.ts:8-22, 28-88` | 用 `schemars` + 自定义 wrapper 把 `serde::Deserialize` 类型转 JSON Schema 7；用 `once_cell::sync::Lazy<HashMap<TypeId, Arc<JSONSchema>>>` 做缓存 |
| **P1** | **provider-specific schema sanitize** | `transform.ts:1463-1686` | laew 只接 Anthropic + OpenAI 两个 provider；至少要做：① Anthropic `additionalProperties: false` 注入；② OpenAI strict mode 白名单关键字；③ Moonshot `$ref` sibling 字段剥离 |
| **P1** | **`$ref` 展平** | `json-schema.ts:121-144` | `schemars` 输出带 `$defs` 时直接 inline；用 `HashSet<String>` 防循环 |
| **P1** | **enum 字符串化（Gemini 兼容）** | `transform.ts:1626-1632` | Rust 端可在 Schema 生成阶段把 `enum: [1,2,3]` 转 `enum: ["1","2","3"]`，再注入到 OpenAI 协议的 tools 里 |
| **P2** | **`required` 过滤** | `transform.ts:1659-1661` | 清理指向不存在 `properties` 的 `required` 项 |
| **P2** | **`type` 数组拆 `anyOf`** | `transform.ts:1646-1656` | Gemini 透传路径的兼容性，laew 暂不需要（不接 Gemini） |
| **P2** | **`integer` 钳制 `[MIN_SAFE_INTEGER, MAX_SAFE_INTEGER]`** | `transform.ts:83-85` | 防止 LLM 漏写 min/max 导致精度歧义 |
| — | **不建议** 引入 partial-JSON 修复库 | 全仓库零命中 | AI SDK 客户端已处理上游解析，laew 没必要多此一举 |
| — | **不建议** 做 Bedrock 自定义 | `transform.ts` 不处理 Bedrock | laew 不接 Bedrock |

**对 laew 的最小可落地切片（P0，约半天）**：

1. 给每个工具的 `Parameters` struct 派生 `JsonSchema`（`#[derive(serde::Deserialize, schemars::JsonSchema)]`）；
2. 工具 execute 入口包一层 `decode_or_inform(args: Value, schema: JSONSchema) -> Result<T, ToolError>`，失败时返回 `ToolError::InvalidArgs { expected, actual }`；
3. 在 `src/llm/anthropic.rs` / `openai.rs` 里：传 tool 给 LLM 前用 `tools.to_json_schema()` → `sanitize_anthropic()`（注入 `additionalProperties: false`） / `sanitize_openai()`（白名单关键字）；
4. `model.tool_result` 错误回灌时附 `expected/actual` 字段，模型自然能自愈。

### 19.4 维度四：命令执行与进程管理（bash）

opencode 的 bash 工具**两套实现**并行：V1（`packages/opencode/src/tool/shell.ts`，645 行，能力齐全）+ V2（`packages/core/src/tool/bash.ts`，207 行，能力极简）。下面分别拆。

#### 19.4.1 spawn 方式：PowerShell / cmd / POSIX 三合一（V1）

`packages/opencode/src/tool/shell.ts:293-310` 的 `cmd()` 函数把"用什么 shell"做成显式分支：

```ts
function cmd(shell: string, command: string, cwd: string, env: NodeJS.ProcessEnv) {
  if (process.platform === "win32" && Shell.ps(shell)) {
    return ChildProcess.make(shell, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", command], {
      cwd, env, stdin: "ignore", detached: false,
    })
  }
  return ChildProcess.make(command, [], {
    shell, cwd, env, stdin: "ignore", detached: process.platform !== "win32",
  })
}
```

**关键设计**：

1. **Windows PowerShell vs POSIX sh 用不同参数矩阵** —— PS 走 `-Command` 直接传字符串；POSIX 走 `shell:` 字段（让 Node spawn 选 `/bin/sh -c`）。
2. **`stdin: "ignore"`** —— 所有 bash 调用的 stdin 立即关闭；不允许模型"喂输入"。
3. **`detached: process.platform !== "win32"`** —— POSIX 上开新进程组（`setpgid`），保证 timeout/abort 时能 kill **整棵进程树**（不是只 kill leader）。Windows 上不 detach 因为没有 `setpgid` 等价物。
4. **`shellEnv` plugin hook**（`:416-426`）—— 通过 `plugin.trigger("shell.env", { cwd, sessionID, callID }, { env: {} })` 合并 `process.env + extra.env`；**插件可以注入额外变量**（如 `MY_API_KEY`）。V2 在 TODO 里（`core/src/tool/bash.ts:70`）。

#### 19.4.2 超时与 kill：三路 race + forceKillAfter: "3 seconds"

`packages/opencode/src/tool/shell.ts:533-557` 是**整个 bash 工具的核心控制循环**：

```ts
const abort = Effect.callback<void>((resume) => {
  if (ctx.abort.aborted) return resume(Effect.void)
  const handler = () => resume(Effect.void)
  ctx.abort.addEventListener("abort", handler, { once: true })
  return Effect.sync(() => ctx.abort.removeEventListener("abort", handler))
})

const timeout = Effect.sleep(`${input.timeout + 100} millis`)   // ← +100ms 让 exit 优先

const exit = yield* Effect.raceAll([
  handle.exitCode.pipe(Effect.map((code) => ({ kind: "exit" as const, code }))),
  abort.pipe(Effect.map(() => ({ kind: "abort" as const, code: null }))),
  timeout.pipe(Effect.map(() => ({ kind: "timeout" as const, code: null }))),
])

if (exit.kind === "abort") {
  aborted = true
  yield* handle.kill({ forceKillAfter: "3 seconds" }).pipe(Effect.orDie)
}
if (exit.kind === "timeout") {
  expired = true
  yield* handle.kill({ forceKillAfter: "3 seconds" }).pipe(Effect.orDie)
}
```

**亮点**：

1. **`Effect.raceAll([exit, abort, timeout])`** —— 三路并发，谁先返回谁赢；
2. **`timeout + 100 millis`** —— timeout 比用户给的多 100ms，让 exitCode 自然先返回；
3. **`kill({ forceKillAfter: "3 seconds" })`** —— 先发 SIGTERM，3 秒不退就 SIGKILL（POSIX `setpgid` 后能整组 kill；Windows 上只能 kill leader + 子进程可能泄漏，这是已知 TODO）；
4. **`abort` 来自 `ctx.abort`（AbortSignal）** —— 上层 Session 取消、用户 Ctrl+C、batch 终止都会触发。

V2 在 `packages/core/src/tool/bash.ts:163-184` 简化为：

```ts
const command = ChildProcess.make(input.command, [], {
  cwd: target.canonical, shell,
  stdin: "ignore",
  detached: process.platform !== "win32",
  forceKillAfter: Duration.seconds(3),
})
const timeout = input.timeout ?? DEFAULT_TIMEOUT_MS
const result = yield* appProcess.run(command, {
  combineOutput: true,
  timeout: Duration.millis(timeout),
  maxOutputBytes: MAX_CAPTURE_BYTES,
})
```

V2 把**超时、kill、合并输出、最大字节**全压到 `AppProcess.run()` 一个调用里，由 `packages/core/src/process`（底层 `ChildProcessSpawner`）兜底。

#### 19.4.3 默认超时 2 分钟 / 最大 10 分钟（V2 强校验）

V1 `packages/opencode/src/tool/shell.ts:347`：

```ts
const defaultTimeoutMs = flags.bashDefaultTimeoutMs ?? 2 * 60 * 1000
```

V2 `packages/core/src/tool/bash.ts:19-32`：

```ts
export const DEFAULT_TIMEOUT_MS = 2 * 60 * 1_000
export const MAX_TIMEOUT_MS = 10 * 60 * 1_000
export const MAX_CAPTURE_BYTES = 1024 * 1024

export const Input = Schema.Struct({
  command: Schema.String.annotate({...}),
  workdir: Schema.String.pipe(Schema.optional).annotate({...}),
  timeout: PositiveInt.check(Schema.isLessThanOrEqualTo(MAX_TIMEOUT_MS))
    .pipe(Schema.optional)
    .annotate({...}),
})
```

**V2 把 `MAX_TIMEOUT_MS` 提升到了 Schema 层** —— 模型想传 1 小时？直接 schema decode 失败，错误信息 `Expected a value less than or equal to 600000, but received 3600000`。这是**协议级强制**而非运行时校验，模型下次调用自然就合规了。laew 当前是裸 `u64`，学 V2 这条。

#### 19.4.4 输出流式与截断：sliding window + preview tail + overflow to file

V1 `packages/opencode/src/tool/shell.ts:438-580` 把输出处理拆成**三段**：

**段一：内存 sliding window**（`:440-497`）：

```ts
const limits = yield* trunc.limits()
const keep = limits.maxBytes * 2                // ← 内存缓冲放大到 2 倍上限，给溢出检测留时间
let full = ""
let last = ""
const list: Chunk[] = []
let used = 0
let file = ""
let sink: ReturnType<typeof createWriteStream> | undefined
let cut = false

// 每来一个 chunk：
list.push({ text: chunk, size })
used += size
while (used > keep && list.length > 1) {        // ← 滑动窗口：从头部丢旧 chunk
  const item = list.shift()
  if (!item) break
  used -= item.size
  cut = true
}
last = preview(last + chunk)                    // metadata 用的"最近 30KB"预览
```

**段二：溢出到文件**（`:500-523`）：

```ts
if (file) {
  sink?.write(chunk)                             // 已经溢出了：持续写文件
} else {
  full += chunk
  if (Buffer.byteLength(full, "utf-8") > limits.maxBytes) {
    return trunc.write(full).pipe(
      Effect.andThen((next) => Effect.sync(() => {
        file = next
        cut = true
        sink = createWriteStream(next, { flags: "a" })
        full = ""
      })),
      Effect.andThen(ctx.metadata({ metadata: { output: last } })),
    )
  }
}
```

**段三：结尾归一化**（`:561-580`）：

```ts
const raw = list.map((item) => item.text).join("")
const end = tail(raw, limits.maxLines, limits.maxBytes)            // ← 头尾截断
if (end.cut) cut = true
if (!file && end.cut) file = append                              // 内存里尾部截断也算溢出

let output = end.text
if (!output) output = "(no output)"

if (cut && file) {
  output = `...output truncated...\n\nFull output saved to: ${file}\n\n` + output
}
if (meta.length > 0) {
  output += "\n\n<shell_metadata>\n" + meta.join("\n") + "\n</shell_metadata>"
}
```

**`tail()` 实现**（`:225-255`）—— 字节/行双重限制 + UTF-8 安全：

```ts
function tail(text: string, maxLines: number, maxBytes: number) {
  const lines = text.split("\n")
  if (lines.length <= maxLines && Buffer.byteLength(text, "utf-8") <= maxBytes) {
    return { text, cut: false }
  }
  const out: string[] = []
  let bytes = 0
  for (let i = lines.length - 1; i >= 0 && out.length < maxLines; i--) {
    const size = Buffer.byteLength(lines[i], "utf-8") + (out.length > 0 ? 1 : 0)
    if (bytes + size > maxBytes) {
      if (out.length === 0) {
        const buf = Buffer.from(lines[i], "utf-8")
        let start = buf.length - maxBytes
        if (start < 0) start = 0
        while (start < buf.length && (buf[start] & 0xc0) === 0x80) start++   // ← UTF-8 边界保护
        out.unshift(buf.subarray(start).toString("utf-8"))
      }
      break
    }
    out.unshift(lines[i])
    bytes += size
  }
  return { text: out.join("\n"), cut: true }
}
```

**`preview()` 实现**（`:220-223`）—— metadata 用的"最近 30KB 预览"：

```ts
const MAX_METADATA_LENGTH = 30_000
function preview(text: string) {
  if (text.length <= MAX_METADATA_LENGTH) return text
  return "...\n\n" + text.slice(-MAX_METADATA_LENGTH)        // ← 只取末尾 30KB
}
```

**三段式输出**（`:582-584`）—— `<shell_metadata>` 块让 timeout/abort/exit 信息与 stdout 严格分离：

```
{output}

<shell_metadata>
shell tool terminated command after exceeding timeout 120000 ms...
</shell_metadata>
```

#### 19.4.5 Truncate 服务：MAX_LINES 2000 / MAX_BYTES 50KB + 任务代理提示

`packages/opencode/src/tool/truncate.ts` 是**所有工具输出截断的中央服务**（grep / glob / read / bash 都复用）：

```ts
// packages/opencode/src/tool/truncate.ts:12
const RETENTION = Duration.days(7)                  // 溢出文件保留 7 天
export const MAX_LINES = 2000
export const MAX_BYTES = 50 * 1024
export const DIR = TRUNCATION_DIR                    // ~/.local/share/opencode/tool-output
export const GLOB = path.join(TRUNCATION_DIR, "*")

const cleanup = Effect.fn("Truncate.cleanup")(function* () {
  const cutoff = Date.now() - Duration.toMillis(RETENTION)
  // 列出 tool_ 前缀的文件，删 mtime < cutoff 的
})

const write = Effect.fn("Truncate.write")(function* (text: string) {
  const file = path.join(TRUNCATION_DIR, ToolID.ascending())    // ← 工具 ID 作文件名
  yield* fs.ensureDir(TRUNCATION_DIR)
  yield* fs.writeFileString(file, text)
  return file
})

const limits = Effect.fn("Truncate.limits")(function* () {
  const configSvc = yield* Effect.serviceOption(Config.Service)
  if (Option.isNone(configSvc)) return { maxLines: MAX_LINES, maxBytes: MAX_BYTES }
  const cfg = yield* configSvc.value.get().pipe(Effect.catch(() => Effect.succeed(undefined)))
  return {
    maxLines: cfg?.tool_output?.max_lines ?? MAX_LINES,
    maxBytes: cfg?.tool_output?.max_bytes ?? MAX_BYTES,
  }
})
```

**关键点**：

1. **每个工具一个上限**，可由用户在 `opencode.json` 的 `tool_output.max_lines/max_bytes` 覆盖；
2. **溢出文件命名 `tool_<ID>`**（`ToolID.ascending()` 来自 `packages/opencode/src/tool/schema.ts`），便于 cleanup 按前缀批量删；
3. **后台 hourly cleanup**（`:143-148`）—— `Effect.repeat(Schedule.spaced(Duration.hours(1)))` + `Effect.delay(Duration.minutes(1))`，每 1 小时扫一遍，删 7 天前的；
4. **截断后给不同 agent 不同提示**（`:129-131`）：

```ts
const hint = hasTaskTool(agent)
  ? `The tool call succeeded but the output was truncated. Full output saved to: ${file}\nUse the Task tool to have explore agent process this file with Grep and Read (with offset/limit). Do NOT read the full file yourself - delegate to save context.`
  : `The tool call succeeded but the output was truncated. Full output saved to: ${file}\nUse Grep to search the full content or Read with offset/limit to view specific sections.`
```

**洞察**：**根据 agent 是否有 Task 工具（delegation capability）切换截断提示语** —— 主 agent 应该委派 explore 子 agent 去读，自己读就浪费 context。**这是把"agent 编排能力"反映到"工具文案"上的精细设计**。

#### 19.4.6 退出码 + 非零输出 + timeout 信息统一封装

V1（`:561-594`）：

```ts
const meta: string[] = []
if (expired) {
  meta.push(`shell tool terminated command after exceeding timeout ${input.timeout} ms. If this command is expected to take longer and is not waiting for interactive input, retry with a larger timeout value in milliseconds.`)
}
if (aborted) meta.push("User aborted the command")
const raw = list.map((item) => item.text).join("")
const end = tail(raw, limits.maxLines, limits.maxBytes)
// ...截断/溢出处理
return {
  title: input.command,
  metadata: {
    output: last || preview(output),
    exit: code,
    truncated: cut,
    ...(cut && file ? { outputPath: file } : {}),
  },
  output,
}
```

V2 `packages/core/src/tool/bash.ts:51-57`：

```ts
const modelOutput = (output: Output) => {
  const warnings = output.warnings?.length
    ? `\n\nWarnings:\n${output.warnings.map((warning) => `- ${warning}`).join("\n")}`
    : ""
  if (output.timeout) return `${warnings.trimStart()}${warnings ? "\n\n" : ""}Command timed out before completion.`
  return `${warnings.trimStart()}${warnings ? "\n\n" : ""}Command exited with code ${output.exit}.`
}
```

**V2 的结构化输出模型**（`:35-46`）：

```ts
const StructuredOutput = Schema.Struct({
  exit: Schema.Number.pipe(Schema.optional),         // 可能 undefined（timeout 时无 exit）
  truncated: Schema.Boolean,
  timeout: Schema.Boolean.pipe(Schema.optional),
})
const Output = Schema.Struct({
  ...StructuredOutput.fields,
  output: Schema.String,
  warnings: Schema.Array(Schema.String).pipe(Schema.optional),
})
```

**洞察**：V2 把 `output`（文本）+ `StructuredOutput`（exit/truncated/timeout/warnings）拆开 → **模型可以从结构化字段直接判断命令是否成功，不用 parse 文本**。**这是给 agent 的"机器可读信号"**，比"读 output 字符串找 'error'" 强很多。

#### 19.4.7 cwd 解析与 workdir 参数

V1 `packages/opencode/src/tool/shell.ts:612-617`：

```ts
const cwd = params.workdir
  ? yield* resolvePath(params.workdir, instanceCtx.directory, shell)   // ← 解析 workdir
  : instanceCtx.directory
if (params.timeout !== undefined && params.timeout < 0) {
  throw new Error(`Invalid timeout value: ${params.timeout}. Timeout must be a positive number.`)
}
```

V1 prompt（`packages/opencode/src/tool/shell/prompt.ts:112-118`）**显式禁止** `cd ... && ...`：

```
- AVOID using `cd <directory> && <command>`. Use the `workdir` parameter to change directories instead.
<good-example>Use workdir="/foo/bar" with command: pytest tests</good-example>
<bad-example>cd /foo/bar && pytest tests</bad-example>
```

**洞察**：把"避免反模式"写进 prompt 比靠权限拦截更有效 —— 模型**根本不会想到** 用 `cd &&`，直接走 `workdir`。`workdir` 还接受 PowerShell 路径转换（`cygpath -w`，`:349-356`）。

#### 19.4.8 环境变量：plugin hook 注入

`packages/opencode/src/tool/shell.ts:416-426`：

```ts
const shellEnv = Effect.fn("ShellTool.shellEnv")(function* (ctx: Tool.Context, cwd: string) {
  const extra = yield* plugin.trigger(
    "shell.env",
    { cwd, sessionID: ctx.sessionID, callID: ctx.callID },
    { env: {} },
  )
  return {
    ...process.env,
    ...extra.env,      // ← 插件可注入 MY_API_KEY 之类
  }
})
```

**洞察**：环境变量不是"工具私有配置"，而是通过**插件系统**集中注入 —— 这让多个工具能共享同一套环境策略（API keys、PATH additions），同时支持运行时按 session/call 注入（`sessionID` + `callID` 入参）。V2 TODO（`core/src/tool/bash.ts:70`）。

#### 19.4.9 并发与危险命令：没有

**opencode 的 bash 没有并发限制** —— 模型并行发多个 `bash` 调用时，processor 同时 spawn 多个进程，由 OS 调度。**没有任何 semaphore / permit 限制**。也没有 `rm -rf /` 之类的危险命令黑名单 —— 完全靠"permission patterns + external_directory ask + BashArity 前缀授权"三件套。

#### 19.4.10 权限联动：tree-sitter parse → external_directory + BashArity

V1 bash 在执行命令前做两步（`packages/opencode/src/tool/shell.ts:620-628`）：

```ts
yield* Effect.scoped(
  Effect.gen(function* () {
    const tree = yield* Effect.acquireRelease(parse(params.command, ps), (tree) =>
      Effect.sync(() => tree.delete()),       // ← scoped 释放 tree-sitter AST
    )
    const scan = yield* collect(tree.rootNode, cwd, ps, shell, instanceCtx)
    if (!containsPath(cwd, instanceCtx)) scan.dirs.add(cwd)
    yield* ask(ctx, scan, params)              // ← 把 scan 结果交给权限引擎
  }),
)
```

`parse()` 用 **web-tree-sitter** 跑 bash 或 PowerShell 语法树（`:257-336`）—— **不调用真实 shell，只静态解析 AST**。把命令拆成 part 列表：

```ts
function parts(node: Node) {
  const out: Part[] = []
  for (let i = 0; i < node.childCount; i++) {
    const child = node.child(i)
    if (!child) continue
    if (child.type === "command_elements") {
      for (let j = 0; j < child.childCount; j++) {
        const item = child.child(j)
        if (!item || item.type === "command_argument_sep" || item.type === "redirection") continue
        out.push({ type: item.type, text: item.text })
      }
      continue
    }
    // command_name / word / string / raw_string / concatenation
    out.push({ type: child.type, text: child.text })
  }
  return out
}
```

**collect()**（`:378-414`）提取三类风险：

| 集合 | 含义 | 用途 | 行号 |
|------|------|------|------|
| `CWD` (`cd`, `chdir`, `popd`, `pushd`, …) | 改变工作目录 | 跳过这些命令（不做 external_directory 检查） | `:28` |
| `FILES` (`rm`, `cp`, `mv`, `mkdir`, `chmod`, `cat`, …) | 接触文件的命令 | 每个参数都解析成绝对路径，越界则加入 `scan.dirs` | `:29-50` |
| `CMD_FILES` (Windows cmd 别名) | `copy`/`del`/`dir`/`erase`/… | 同上 | `:51-64` |

```ts
if (cmd && (FILES.has(cmd) || (shellKind === "cmd" && CMD_FILES.has(cmd)))) {
  for (const arg of pathArgs(command, ps, shellKind === "cmd")) {
    const resolved = yield* argPath(arg, cwd, ps, shell)
    if (!resolved || containsPath(resolved, instance)) continue
    const dir = (yield* fs.isDir(resolved)) ? resolved : path.dirname(resolved)
    scan.dirs.add(dir)
  }
}
```

`ask()`（`:263-291`）把两类授权交给 permission 引擎：

```ts
if (scan.dirs.size > 0) {
  const directories = Array.from(scan.dirs)
  const globs = directories.map((dir) => {
    if (process.platform === "win32") return FSUtil.normalizePathPattern(path.join(dir, "*"))
    return path.join(dir, "*")
  })
  yield* ctx.ask({
    permission: "external_directory",
    patterns: globs,
    always: globs,
    metadata: { command: input.command, directories, patterns: globs },
  })
}

if (scan.patterns.size === 0) return
yield* ctx.ask({
  permission: ShellID.ToolID,
  patterns: Array.from(scan.patterns),
  always: Array.from(scan.always),                          // ← BashArity 前缀
  metadata: { command: input.command },
})
```

**`BashArity` 前缀授权**（`packages/opencode/src/permission/arity.ts`）：

```ts
const ARITY: Record<string, number> = {
  cat: 1, // cat file.txt
  cd: 1,
  ...
  npm: 2, // npm install
  "npm run": 3, // npm run dev
  git: 2, // git checkout main
  "git stash": 3,
  ...
}

export function prefix(tokens: string[]) {
  for (let len = tokens.length; len > 0; len--) {
    const prefix = tokens.slice(0, len).join(" ")
    const arity = ARITY[prefix]
    if (arity !== undefined) return tokens.slice(0, arity)
  }
  if (tokens.length === 0) return []
  return tokens.slice(0, 1)
}
```

**洞察**：`prefix(["npm", "run", "dev", "--port", "3000"])` 返回 `["npm", "run", "dev"]`（因为 `npm run` arity=3），`always` 加 `"*"` 后是 `"npm run dev *"` —— 用户授权 `npm run dev *` 就允许 `npm run dev --port 3000`、`npm run dev --watch` 等所有变体，**不需要为每个 flag 重复询问**。这是一份**手工维护的 100+ 命令前缀表**，但收益巨大 —— 用户配置成本降低 10 倍。

**权限求值**（`packages/opencode/src/permission/index.ts:28-38`）：

```ts
export function evaluate(permission: string, pattern: string, ...rulesets: PermissionV1.Ruleset[]): PermissionV1.Rule {
  return (
    rulesets
      .flat()
      .findLast((rule) => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern)) ?? {
      action: "ask",
      permission,
      pattern: "*",
    }
  )
}
```

**三态规则**（`action: "allow" | "ask" | "deny"`）+ wildcard pattern + `findLast`（后定义优先）= **完整的权限策略引擎**。

#### 19.4.11 后台任务：设计明确但能力未启

V2 `packages/core/src/tool/bash.ts:72-74` TODO 列出了**完整的后台任务设计哲学**：

```ts
// TODO: Persist background job status and define restart recovery before exposing remote observation.
// TODO: Re-add model-facing background launch only with owner-bound get/wait/cancel tools and completion delivery.
// TODO: Add HTTP background-job observation only after durable status, restart recovery, and authorization are defined.
```

但 `packages/opencode/src/background/job.ts`（通过 `@opencode-ai/core/background-job`）**已经实现了底层服务**：list/get/start/extend/wait/waitForPromotion/promote/cancel —— 只是还没有 model-facing tool 包装。V1 在 TUI 里也支持 /background 子命令。

**opencode 的设计哲学**：后台任务需要 ① 持久化状态（崩溃恢复）② owner-bound 访问控制（get/wait/cancel 必须验证 owner）③ 完成事件分发 —— **三件齐全后才暴露给模型**。当前只完成 ①，②③ 未完成所以模型仍只能"前台 + 超时"。

#### 19.4.12 维度四 对 laew 的借鉴

laew 现状（参照 CLAUDE.md）：`src/agent/tools/bash.rs` 只有基础 `tokio::process::Command`，没有超时强校验、没有 tree-sitter、没有 permission 联动、没有截断服务、没有后台任务。

| 优先级 | 建议 | 依据 | Rust 落地要点 |
|--------|------|------|--------------|
| **P0** | **超时强校验 + schema 上限** | `core/src/tool/bash.ts:28-32` | `timeout: u64` 改成 `0 < timeout <= 600_000` 的 newtype，schema 同步体现 |
| **P0** | **统一 Truncate 服务（MAX_LINES 2000 / MAX_BYTES 50KB）** | `tool/truncate.ts:14-15` | `src/util/truncate.rs`：`fn tail(text, maxLines, maxBytes) -> Result<String, ...>`，UTF-8 安全（同 `:243` 字节边界检查） |
| **P0** | **输出截断后写溢出文件 + 提示"用 Read with offset/limit"** | `truncate.ts:129-140` | 溢出文件路径：`./.laew/tool-output/tool_<ULID>`（保留 7 天，cron 清理） |
| **P0** | **timeout/abort/exit 三路 race** | `shell.ts:533-557` | `tokio::select! { exit = child.wait() => ..., _ = sleep(timeout) => ..., _ = abort_signal => ... }`，kill 用 `child.kill().await`；POSIX `setpgid` 用 `Command::pre_exec` |
| **P0** | **`workdir` 参数 + 反 `cd &&` 提示** | `shell.ts:612-617`、`shell/prompt.ts:112-118` | `bash` 工具 schema 加 `workdir: Option<String>`，系统提示词写明"用 workdir 不用 cd" |
| **P1** | **结构化 output（exit / truncated / timeout）** | `core/src/tool/bash.ts:35-46` | 工具返回值不再是 `String`，而是 `{output: String, exit: i32, truncated: bool, timeout: bool}`；model 走 `toModelOutput` 看到 stdout + 一行 "Command exited with code N." |
| **P1** | **stderr 单独保留 / 合并（按场景）** | V2 `combineOutput: true`、V1 双流 `handle.stderr`/`stdout` 各自处理 | laew 可以默认合并，但加 `capture_stderr: bool` 参数 |
| **P1** | **`stdin: ignore`** | `shell.ts:298, 307` | Rust 端 `Stdio::null()` |
| **P1** | **超时给模型留 +100ms buffer** | `shell.ts:540` | 让 exit code 自然先到 |
| **P1** | **`forceKillAfter: Duration.seconds(3)`** | `core/src/tool/bash.ts:163` | SIGTERM → 等 3 秒 → SIGKILL；Windows 用 `taskkill /F /T` |
| **P1** | **Truncate 后台 hourly cleanup（保留 7 天）** | `truncate.ts:53-66, 143-148` | `tokio::spawn(async move { loop { sleep(1h).await; cleanup().await } })` |
| **P1** | **溢出文件按"agent 是否有子 agent 能力"切换提示语** | `truncate.ts:129-131` | laew 的 SubAgent 模型可借鉴：truncate 后告诉主 agent "用 Task 委派 explore 子 agent 读" |
| **P2** | **tree-sitter bash AST 解析** | `shell.ts:312-336, 91-117` | 引入 `tree-sitter` crate + `tree-sitter-bash` grammar；解析命令参数列表 → 越界目录探测 |
| **P2** | **BashArity 前缀授权表** | `permission/arity.ts:24-161` | 100 行 Rust，复制该表 + `prefix()` 函数 |
| **P2** | **plugin shell.env hook** | `shell.ts:416-426` | laew 暂没有 plugin 系统，可以先 hardcode `process.env` |
| **P2** | **后台任务** | `core/src/tool/bash.ts:72-74`、`background/job.ts` | **不急做**。先把前台 + timeout + kill 做对，再考虑 owner-bound get/wait/cancel |
| **P2** | **POSIX `setpgid` + 整组 kill** | `shell.ts:308`（`detached: true`） | `Command::pre_exec(|| unsafe { libc::setpgid(0, 0) });`，timeout 时 kill 整组 `killpg(pgid, SIGTERM)` |
| — | **不建议** 做并发限制 semaphore | V1/V2 都没有 | 模型并行发多个 bash 调用是合法行为；OS 调度足够 |
| — | **不建议** 做危险命令黑名单 | opencode 完全不做 | permission patterns + external_directory ask 已经覆盖 90%；黑名单容易误伤 |

**对 laew 的最小可落地切片（P0，约 1 天）**：

1. 在 `src/agent/tools/bash.rs` 加 `timeout: u32` schema 字段，校验 `0 < timeout <= 600_000`；
2. `src/util/truncate.rs` 工具：UTF-8 安全的 tail，按字节/行双阈值；
3. bash 输出先过 truncate，超限写到 `./.laew/tool-output/tool_<ULID>.txt`；
4. 结果结构化为 `{output: String, exit: i32, truncated: bool, timeout: bool}`；
5. 启动后台 task：每小时扫一遍溢出文件目录，删 7 天前的。

### 19.5 第七轮深挖 — 核心结论与对 laew 的总览

#### 19.5.1 四个维度的关键发现

| 维度 | opencode 核心设计 | 最值得 laew 抄的一条 |
|------|------------------|---------------------|
| **19.1 文件编辑** | 9 级 Replacer 模糊回退 + 唯一性校验 + 错误文案"可自愈" + V2 CAS 乐观并发 + 影子 git 快照 + 按 modelID 切换 Edit/ApplyPatch | **错误文案必带可执行的下一步**（"重新 Read"/"加上下文"/"用 write"/"用 replaceAll"） |
| **19.2 代码检索** | ripgrep 外挂自举（15.1.0，8 平台） + JSON 流式解析 + `take(limit+1)` 截断魔法 + 9 种 LSP 操作 | **`take(limit+1)` 后判定截断** —— Rust 用 `Vec::truncate(limit+1)` 复刻 |
| **19.3 Schema 结构化输出** | 三段式架构（Effect Schema → JSON Schema 7 → provider sanitize）+ WeakMap 缓存 + $ref 展平 + 4 provider 白名单（OpenAI/Moonshot/Gemini/...) | **请求前 schema decode 失败 → 给模型"expected/actual"对照信息**（用 schemars crate） |
| **19.4 Bash 进程管理** | tree-sitter 静态 AST + 三路 race + sliding window + 溢出文件 + MAX_LINES/MAX_BYTES Truncate 服务 + BashArity 前缀授权 + 后台任务"先持久化后暴露" | **`timeout` 强校验在协议层 + 三路 race abort/timeout/exit + 结构化 output** |

#### 19.5.2 opencode 的工程哲学（贯穿四维度）

1. **数据契约集中化**：协议差异封闭在 `provider/transform.ts` 与 `tool/json-schema.ts` 两处，工具作者永远只面对 Effect Schema —— **laew 应把 Anthropic vs OpenAI 协议差异集中到 `src/llm/` 两个文件，业务代码零感知**。
2. **工具失败要"可自愈"**：每个工具的错误信息都设计成能让模型**下一步行动**（Re-read / Add context / Use replaceAll / Increase timeout / Use workdir）—— **laew 当前所有工具错误都是裸 io::Error，对 agent 不友好**。
3. **截断是显式 API**：`Truncate.Service` + `tool_output.max_lines/max_bytes` 配置 + overflow 文件 + 按 agent 角色切换提示语 —— **laew 应建一个统一的 `src/util/truncate.rs`，所有工具复用**。
4. **能力未启就先 TODO**：V2 edit / bash / grep 都大量用 TODO 注释列出"为什么没做 + 什么时候做"—— **laew 应效仿：每个未做能力写明"待迁移 + 阻塞原因"，避免知识丢失**。
5. **协议细节先观察再适配**：`transform.ts:1547-1563` 有 16 行 OpenAI strict mode 的注释代码被**注释掉**（"Codex also applies lossy compaction above 4 KB; defer that until OpenCode needs the same schema budget."）—— **laew 不要在没观测到 case 时过度适配**。

#### 19.5.3 对 laew 的总落地路线图（按性价比排序）

| 优先级 | 工作量 | 收益 | 内容 |
|--------|--------|------|------|
| **P0**（1-2 天） | 小 | 极高 | ① 新增 edit 工具（19.1）② grep/glob 工具 + rg 自举（19.2）③ bash timeout/schema 强校验（19.4）④ 统一 Truncate 服务（19.4） |
| **P1**（3-5 天） | 中 | 高 | ⑤ 9 级 Replacer 模糊回退（19.1）⑥ per-file 互斥锁 + 影子 git 快照（19.1）⑦ 三段式 schema 归一化（19.3）⑧ structured output 模型（19.4） |
| **P2**（1-2 周） | 大 | 中 | ⑨ LSP 集成（19.2）⑩ tree-sitter bash AST 权限联动（19.4）⑪ BashArity 前缀授权表（19.4）⑫ 后台任务 + 持久化（19.4） |
| **P3**（待评估） | 极大 | 待评估 | ⑬ embedding / 向量索引（19.2）⑭ Moonshot / Gemini provider 接入（19.3） |

**第一性原则**：laew 与 opencode 的最大差异是 **laew 没有持续投入 6 轮 100+ 工程师月的资源**。**P0 全部抄到（4 个工具）就能让 laew 的"代码能力"对标 opencode 80%**，剩下的 20% 是 V2 core 那套重构（TODO 多 = 实现薄），**不急**。

---
