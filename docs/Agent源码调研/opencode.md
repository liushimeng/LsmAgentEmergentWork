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

## 20. 第八轮深挖 — Enterprise Durable Object + 多端 UI + HTTP Recorder + Slack 集成（2026-09-07）

前 7 轮已覆盖 opencode 的核心 runtime（Effect 全栈 DI、34 包 workspace、15 种 LLMEvent、Tool 系统、流式渲染、记忆、Skill、错误重试、Cost、可观测性、Session 持久化、测试、配置 8 层发现、插件生态、第六轮 Effect/Schema/LayerNode/Durable Object、第七轮 Edit/grep/Bash）。**本轮聚焦 4 个全新的横切维度**，全部为「**生态外延**」类能力——与已有 19 章聚焦的「agent 内核」并列的「**生态外延**」：Enterprise 持久化与云存储、多端 UI、协议录制回放、企业 IM 集成。这 4 个维度的共同特征是：它们都不是「让 LLM 更聪明」的能力，而是「让 LLM 跑得起来、跑得出去、跑得进企业」的能力。

### 20.1 Enterprise Durable Object + R2 云存储

#### 20.1.1 包定位与部署目标

`packages/enterprise/` 是一个**部署到 Cloudflare Workers** 的 SolidStart 应用（同时支持自托管 Nitro server）。`package.json:1-14` 显示它通过 `OPENCODE_DEPLOYMENT_TARGET=cloudflare` 环境变量切换：

```json
"build": "vite build",
"build:cloudflare": "OPENCODE_DEPLOYMENT_TARGET=cloudflare vite build",
"start": "vite start",
"shell-prod": "sst shell --target Teams --stage production"
```

`vite.config.ts:7-23` 进一步定义了 nitro preset 切换：

```typescript
const nitroConfig: any = (() => {
  const target = process.env.OPENCODE_DEPLOYMENT_TARGET
  if (target === "cloudflare") {
    return {
      compatibilityDate: "2024-09-19",
      preset: "cloudflare-module",
      cloudflare: { nodeCompat: true },
    }
  }
  return {}
})()
```

`compatibilityDate: 2024-09-19` 是 Cloudflare Workers 的**功能冻结日期**——这一天之后 Workers runtime 的新特性将不再对此 worker 启用。这是**严肃工程**的做法：避免 Cloudflare 推送更新后旧代码行为变化。`nodeCompat: true` 允许使用 `node:crypto`（enterprise API 中确实用了 `timingSafeEqual`）。

**注意**：尽管项目名是 "Durable Object"，但 enterprise 实际**没有用 Durable Object 的 SQL API**——它把所有持久化都委托给 R2/S3 存储（`src/core/storage.ts`），Durable Object 仅作为 Workers 容器。**这与「经典 Durable Object + D1 + R2 三件套」范式有差异**——opencode 的 enterprise 选择**只用 R2** 简化部署。

#### 20.1.2 Storage 抽象层 — R2/S3 双适配

`src/core/storage.ts:4-10` 定义了 4 个方法的 `Adapter` 接口：

```typescript
export interface Adapter {
  read(path: string): Promise<string | undefined>
  write(path: string, value: string): Promise<void>
  remove(path: string): Promise<void>
  list(options?: { prefix?: string; limit?: number; after?: string; before?: string }): Promise<string[]>
}
```

**注意：`list` 返回的是完整 key 列表**（不是分页 cursor），这与 S3 标准 ListObjectsV2 的 `IsTruncated`+`NextContinuationToken` 模型不同——enterprise 选择简化（一次拉完），适合小规模 share 数据。`adapter.createAdapter`（`storage.ts:12-64`）基于 `aws4fetch`（Cloudflare Workers 上用的纯 fetch SigV4 实现，不是 AWS SDK）实现通用 S3 协议客户端。

R2 与 S3 适配（`storage.ts:66-84`）：

```typescript
function s3(): Adapter {
  const bucket = process.env.OPENCODE_STORAGE_BUCKET!
  const region = process.env.OPENCODE_STORAGE_REGION || "us-east-1"
  const client = new AwsClient({
    region,
    accessKeyId: process.env.OPENCODE_STORAGE_ACCESS_KEY_ID!,
    secretAccessKey: process.env.OPENCODE_STORAGE_SECRET_ACCESS_KEY!,
  })
  return createAdapter(client, `https://s3.${region}.amazonaws.com`, bucket)
}

function r2() {
  const accountId = process.env.OPENCODE_STORAGE_ACCOUNT_ID!
  const client = new AwsClient({
    accessKeyId: process.env.OPENCODE_STORAGE_ACCESS_KEY_ID!,
    secretAccessKey: process.env.OPENCODE_STORAGE_SECRET_ACCESS_KEY!,
  })
  return createAdapter(client, `https://${accountId}.r2.cloudflarestorage.com`, process.env.OPENCODE_STORAGE_BUCKET!)
}
```

**关键设计选择**：**R2 与 S3 共用同一个 S3 协议客户端**（不引入 R2-specific SDK），因为 R2 完全兼容 S3 API。endpoint 仅 hostname 不同（`${accountId}.r2.cloudflarestorage.com` vs `s3.${region}.amazonaws.com`）。**Region 字段对 R2 不传**（R2 是 single-region 默认 us-east-1 兼容）。

`adapter` 解析（`storage.ts:86-91`）通过 `lazy()` 懒加载——**只有真正调用 read/write 时才校验环境变量**，避免 cold start 时全部 env 必填：

```typescript
const adapter = lazy(() => {
  const type = process.env.OPENCODE_STORAGE_ADAPTER
  if (type === "r2") return r2()
  if (type === "s3") return s3()
  throw new Error("No storage adapter configured")
})
```

`key` 路径解析（`storage.ts:93-95`）强制加 `.json` 后缀——**所有 share 数据都视为 JSON blob**：

```typescript
function resolve(key: string[]) {
  return key.join("/") + ".json"
}
```

`update` 函数（`storage.ts:122-128`）实现了 **read-modify-write** 原子性（依赖 caller 保证不并发，**没有 CAS/乐观锁**）：

```typescript
export async function update<T>(key: string[], fn: (draft: T) => void) {
  const val = await read<T>(key)
  if (!val) throw new Error("Not found")
  fn(val)
  await write(key, val)
  return val
}
```

#### 20.1.3 Share 数据模型 — 5 种 discriminatedUnion

`src/core/share.ts:18-40` 用 Zod `discriminatedUnion` 定义 5 种 share data type：

```typescript
export const Data = z.discriminatedUnion("type", [
  z.object({ type: z.literal("session"), data: z.custom<Session>() }),
  z.object({ type: z.literal("message"), data: z.custom<Message>() }),
  z.object({ type: z.literal("part"), data: z.custom<Part>() }),
  z.object({ type: z.literal("session_diff"), data: z.custom<SnapshotFileDiff[]>() }),
  z.object({ type: z.literal("model"), data: z.custom<Model[]>() }),
])
```

**注意：data 字段用 `z.custom<>()` 而不是具体 schema**——这是为了**避免 enterprise 包依赖 core 包的全部 Session/Message/Part schema**（避免打包膨胀），用 `import type` 引入 SDK 类型仅作 IDE 提示，运行时不做 zod 校验。**这是 enterprise 的「轻量级」取舍**：把校验逻辑放在 client 侧发送端（`@opencode-ai/sdk`），server 信任 client 提交的类型。

#### 20.1.4 Share 合并算法 — snapshot + compaction 双轨

`share.ts:78-115` 是 **「snapshot 优先 + 旧数据 lazy 迁移」** 的两轨机制：

```typescript
async function readSnapshot(shareID: string) {
  return (await Storage.read<Snapshot>(["share_snapshot", shareID]))?.data
}

async function writeSnapshot(shareID: string, data: Data[]) {
  await Storage.write<Snapshot>(["share_snapshot", shareID], { data })
}

async function legacy(shareID: string) {
  const compaction: Compaction = (await Storage.read<Compaction>(["share_compaction", shareID])) ?? {
    data: [],
    event: undefined,
  }
  const list = await Storage.list({
    prefix: ["share_event", shareID],
    before: compaction.event,
  }).then((x) => x.toReversed())
  if (list.length === 0) {
    if (compaction.data.length > 0) await writeSnapshot(shareID, compaction.data)
    return compaction.data
  }
  const next = merge(
    compaction.data,
    await Promise.all(list.map(async (event) => await Storage.read<Data[]>(event))).then((x) =>
      x.flatMap((item) => item ?? []),
    ),
  )
  await Promise.all([
    Storage.write(["share_compaction", shareID], {
      event: list.at(-1)?.at(-1),
      data: next,
    }),
    writeSnapshot(shareID, next),
  ])
  return next
}
```

设计思想：每次 `sync` 时**先尝试读 snapshot**（快路径，单个 S3 GET），若 snapshot 缺失则降级到「**legacy mode**」：拉 `share_event/{id}/` 前缀下所有事件文件 + `share_compaction/{id}` 已压缩的检查点，**合并**后回写 snapshot 一次（**数据迁移是 lazy 一次性**）。后续 sync 直接走 fast path。**这是「旧版本兼容」+「冷数据预热」的经典 pattern**。

`merge` 函数（`share.ts:66-76`）用 `key(item)` 生成唯一 key，按 key 去重，**保留最新**：

```typescript
function merge(...items: Data[][]) {
  const map = new Map<string, Data>()
  for (const list of items) {
    for (const item of list) {
      map.set(key(item), item)
    }
  }
  return Array.from(map.entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([, item]) => item)
}
```

`key` 按类型生成层级 path（`share.ts:51-64`）：

```typescript
function key(item: Data) {
  switch (item.type) {
    case "session": return "session"
    case "message": return `message/${item.data.id}`
    case "part": return `part/${item.data.messageID}/${item.data.id}`
    case "session_diff": return "session_diff"
    case "model": return "model"
  }
}
```

**注意**：`part` 用 `messageID/id` 复合 key（不是 `sessionID/messageID/id` 三级）—— share 是 per-session 的，**messageID 本身已是全 session 唯一**。

#### 20.1.5 鉴权 — secret + timingSafeEqual

`share.ts:117-128` 创建 share 时**生成随机 secret**：

```typescript
const info: Info = {
  id: (isTest ? "test_" : "") + body.sessionID.slice(-8),
  sessionID: body.sessionID,
  secret: crypto.randomUUID(),
}
const exists = await get(info.id)
if (exists) throw new Errors.AlreadyExists(info.id)
await Promise.all([Storage.write(["share", info.id], info), writeSnapshot(info.id, [])])
return info
```

**share ID 是 sessionID 后 8 位**——`slice(-8)` 不可逆但人眼可关联（用户看到 share URL 时知道是哪个 session）。`secret` 是 `crypto.randomUUID()` 122 位熵，作为删除/sync 操作的 bearer token。

删除 share（`share.ts:134-148`）用 secret 校验 + 4 路径级联删除：

```typescript
export const remove = fn(Info.pick({ id: true, secret: true }), async (body) => {
  const share = await get(body.id)
  if (!share) throw new Errors.NotFound(body.id)
  if (share.secret !== body.secret) throw new Errors.InvalidSecret(body.id)
  await Storage.remove(["share", body.id])
  const groups = await Promise.all([
    Storage.list({ prefix: ["share_snapshot", body.id] }),
    Storage.list({ prefix: ["share_compaction", body.id] }),
    Storage.list({ prefix: ["share_event", body.id] }),
    Storage.list({ prefix: ["share_data", body.id] }),
  ])
  for (const item of groups.flat()) {
    await Storage.remove(item)
  }
})
```

**注意：secret 比较用 `===` 而不是 `timingSafeEqual`**——这是 enterprise 自己的代码；但 HTTP API 层（`src/routes/api/[...path].ts:142-155`）在 admin 路径用了**真正的 timingSafeEqual**：

```typescript
.delete("/support/actions/remove-share", async (c) => {
  const authorization = c.req.header("authorization")
  const expected = `Bearer ${(Resource as unknown as Record<string, { value: string }>).SUPPORT_API_KEY.value}`
  const actual = Buffer.from(authorization ?? "")
  const secret = Buffer.from(expected)
  if (actual.length !== secret.length || !timingSafeEqual(actual, secret))
    return c.json({ error: "Unauthorized" }, 401)
  // ...
})
```

**不一致性**：share owner secret 用 `===`，admin API key 用 `timingSafeEqual`。前者风险更小（UUID 122 位熵，定时攻击难度大），后者是 admin 操作权限更大需要严格防时序侧信道。**这是合理的「按威胁建模分级防护」**。

#### 20.1.6 HTTP API — Hono + OpenAPI 3.1.1

`src/routes/api/[...path].ts:11-28` 用 Hono 框架（不是 Express/Fastify）定义 API：

```typescript
const app = new Hono()

app
  .basePath("/api")
  .use(cors())
  .get(
    "/doc",
    openAPIRouteHandler(app, {
      documentation: {
        info: { title: "Opencode Enterprise API", version: "1.0.0", description: "Opencode Enterprise API endpoints" },
        openapi: "3.1.1",
      },
    }),
  )
```

**关键观察**：API 文档是 **运行时生成**（`/api/doc`）—— `openAPIRouteHandler` 扫所有 `describeRoute` 装饰的 endpoint 合并输出 OpenAPI 3.1.1。`describeRoute + validator` 组合（`api/[...path].ts:29-65`）：

```typescript
.post(
  "/share",
  describeRoute({
    description: "Create a share",
    operationId: "share.create",
    responses: { 200: { description: "Success", content: { "application/json": { schema: resolver(z.object({...}).meta({ ref: "Share" })) } } } },
  }),
  validator("json", z.object({ sessionID: z.string() })),
  async (c) => { /* ... */ }
)
```

**Hono 的设计哲学**：**所有 metadata + schema + handler 都在同一处定义**（不像 Express 那样要写 swagger.yaml 单独维护）。`z.object(...).meta({ ref: "Share" })` 把 Zod schema 注册到 OpenAPI 的 `$defs`，让 `/api/doc` 输出标准 JSON Schema。**Zod ↔ JSON Schema 的双向转换是 Hono 生态的核心竞争力**。

HTTP API 最终通过 SolidStart 的 `APIEvent` 桥接（`api/[...path].ts:157-171`）：

```typescript
export function GET(event: APIEvent) { return app.fetch(event.request) }
export function POST(event: APIEvent) { return app.fetch(event.request) }
export function PUT(event: APIEvent) { return app.fetch(event.request) }
export async function DELETE(event: APIEvent) { return app.fetch(event.request) }
```

——把 SolidStart 的 server function 入口**直接转交 Hono 处理**。这种「SolidStart 路由壳 + Hono API 中间件」模式让 enterprise 同时享受 SolidStart 的 SSR 能力（share 页面）和 Hono 的中间件生态（cors/validator/openapi）。

#### 20.1.7 share 页面 — SSR + DataProvider + WorkerPool

`src/routes/share/[shareID].tsx:58-122` 是 share 详情页的核心——`query` + `"use server"` 标记让此函数在**服务端执行**：

```typescript
const getData = query(async (shareID) => {
  "use server"
  const share = await Share.get(shareID)
  if (!share) throw new SessionDataMissingError({ sessionID: shareID })
  const data = await Share.data(shareID)
  // ... 重组为 session/message/part/model 字典
  for (const item of data) {
    switch (item.type) {
      case "session": result.session.push(item.data); break
      case "session_diff": result.session_diff[share.sessionID] = item.data; break
      // ...
    }
  }
  return result
}, "getShareData")
```

**`"use server"` 指令**是 SolidStart/RSC 的语法——让 `getData` 在 server 端运行，client 只拿到返回值。**避免把 share secret / 全部 session data 传到浏览器**。`Binary.search`（`[shareID].tsx:119`）用二分查找 session（session 列表按 id 排序）。

`SessionDataMissingError`（`[shareID].tsx:35-56`）继承自 `NamedError`（core 的统一错误基类），`isInstance` 用 `NamedError.hasName` 实现 type guard —— **Error 类继承 + name 字段 + 静态 isInstance** 是 opencode 的统一错误模式。

页面渲染（`[shareID].tsx:124-417`）用 `createAsync` 异步加载 + `ErrorBoundary` 兜底 + `<ClientOnly>` 包裹 Web Worker：

```typescript
const ClientOnlyWorkerPoolProvider = clientOnly(() =>
  import("@opencode-ai/session-ui/pierre/worker").then((m) => ({
    default: (props: { children: any }) => (
      <WorkerPoolProvider pools={m.getWorkerPools()}>{props.children}</WorkerPoolProvider>
    ),
  })),
)
```

**`clientOnly` 包裹 `@pierre/diffs` Web Worker**——`WorkerPoolProvider` 注入 worker pool 给 `SessionReview` 组件（计算 diff 视图），Web Worker **只在 client 端动态 import**（SSR 阶段跳过），避免 server 端没有 `Worker` 全局报错。

#### 20.1.8 enterprise 包对 laew 的核心借鉴

| 维度 | opencode 做法 | laew 借鉴 | 优先级 |
|------|---------------|-----------|--------|
| Storage 抽象 | `Adapter` interface 4 方法 + 双实现 (R2/S3) | 用 `async-trait` 抽象 `Storage` trait，本地实现 + S3 实现（用 `aws-sdk-s3` 或 `s3` crate） | P2 |
| Share 数据迁移 | snapshot 快路径 + legacy lazy 迁移 | 暂不需要（laew 无历史 share 包袱） | N/A |
| 鉴权分级 | owner secret `===` + admin `timingSafeEqual` | `crypto.subtle.timingSafeEqual` 给 API Key；`===` 给 share token | P1 |
| OpenAPI 生成 | Hono `describeRoute + validator` 运行时生成 | 引入 `utoipa` crate 给 `src/server/` 路由自动生成 OpenAPI 文档 | P2 |
| SolidStart `"use server"` | server-only 函数标记 | laew TUI 模式下无 RSC，等价物是「`run_in_background` + 持久化」标志 | N/A |
| Web Worker 池 | `clientOnly(() => import("./worker"))` | laew TUI 暂不需要（无 GUI 渲染）；未来 web 版用 `vite-plugin-cross-origin-isolation` + `Comlink` | P3 |

### 20.2 多端 UI 框架（web/desktop/slack/cli）

opencode 的 5 个客户端入口各有独立的构建栈：

| 端 | 包 | 构建栈 | 渲染目标 |
|----|------|--------|----------|
| **CLI** | `packages/cli/` | Bun + Effect CLI | 终端 stdout |
| **TUI** | `packages/tui/` | SolidJS + OpenTUI | 终端 cell |
| **App** | `packages/app/` | Vite + SolidJS | Web 浏览器（无 SSR） |
| **Web** | `packages/web/` | Astro + Starlight + SolidJS | 静态站 + Cloudflare Workers |
| **Desktop** | `packages/desktop/` | Electron + electron-vite + SolidJS | Win/Mac/Linux 原生壳 |
| **Slack** | `packages/slack/` | Bun + `@slack/bolt` | Slack channel |

**注意：`@opencode-ai/app` 是 web SPA**（纯客户端 Vite 构建，不做 SSR），`@opencode-ai/web` 是**文档站**（Astro Starlight，部署到 Cloudflare），desktop 包**复用 app 包作为渲染层**（`devDependencies` 里 `desktop` 依赖 `app`，`electron-vite` 把 app 打包成 renderer）。

#### 20.2.1 CLI 端 — Effect Command + lazy handler

`packages/cli/src/index.ts:1-32`：

```typescript
#!/usr/bin/env bun
import * as NodeRuntime from "@effect/platform-node/NodeRuntime"
import * as NodeServices from "@effect/platform-node/NodeServices"
import * as Effect from "effect/Effect"
import { Commands } from "./commands/commands"
import { Runtime } from "./framework/runtime"
import { Daemon } from "./services/daemon"

const Handlers = Runtime.handlers(Commands, {
  $: () => import("./commands/handlers/default"),
  api: () => import("./commands/handlers/api"),
  debug: { agents: () => import("./commands/handlers/debug/agents") },
  migrate: () => import("./commands/handlers/migrate"),
  service: { start: ..., restart: ..., status: ..., stop: ..., password: ... },
  serve: () => import("./commands/handlers/serve"),
})

Runtime.run(Commands, Handlers, { version: "local" }).pipe(
  Effect.provide(Daemon.layer),
  Effect.provide(NodeServices.layer),
  Effect.scoped,
  NodeRuntime.runMain,
)
```

**`Runtime.handlers()`**（`framework/runtime.ts:42-56`）接收 spec 树 + handler 树，**递归 walk 生成 `LazyHandler[]`**——每个 handler 是 `() => import(...)` 懒加载（**让 cold start 只解析 `default` 路径，其他子命令文件不读**）。

`Command.withHandler`（`framework/runtime.ts:62-77`）把 lazy import 包装成 `Effect.gen`，`Effect.flatMap(Effect.promise(handler.load), ...)` 实现**「点击才加载」**——这是 laew 急需的 pattern（laew 当前所有模块都是 `use` 静态导入，冷启动慢）。

#### 20.2.2 Web 端 — Astro + Starlight + i18n middleware

`packages/web/astro.config.mjs:13-32`：

```javascript
export default defineConfig({
  site: config.url,
  base: "/docs",
  output: "server",
  adapter: cloudflare({ imageService: "passthrough" }),
  // ...
  integrations: [
    configSchema(),  // 本地插件: build:done 时 spawn schema 生成脚本
    solidJs(),       // 允许 .astro 文件嵌入 SolidJS 组件
    starlight({ title: "OpenCode", defaultLocale: "root", /* 20 种语言 */ }),
  ],
})
```

**i18n middleware**（`web/src/middleware.ts:80-94`）实现**URL 重写 + cookie 记忆 + Accept-Language fallback**：

```typescript
export const onRequest = defineMiddleware((ctx, next) => {
  const alias = docsAlias(ctx.url.pathname)
  if (alias) return redirect(ctx.url, alias.path, alias.locale)

  if (ctx.url.pathname !== "/docs" && ctx.url.pathname !== "/docs/") return next()

  const locale =
    localeFromCookie(ctx.request.headers.get("cookie")) ??
    localeFromAcceptLanguage(ctx.request.headers.get("accept-language"))
  if (!locale || locale === "root") return next()

  return redirect(ctx.url, `/docs/${locale}/`)
})
```

**3 级 locale 解析**：URL prefix → `oc_locale` cookie (1 year, SameSite=Lax) → `Accept-Language` (q-value 排序)。`exactLocale` 防止 `/docs/zh-hans` 误匹配 `/docs/zh-cn`——**严格匹配 + 备选列表**。

`configSchema` 本地插件（`web/astro.config.mjs:314-323`）在 build 完成时 spawn 一个**子进程跑 schema 生成脚本**：

```javascript
function configSchema() {
  return {
    name: "configSchema",
    hooks: {
      "astro:build:done": async () => {
        console.log("generating config schema")
        spawnSync("../opencode/script/schema.ts", ["./dist/config.json", "./dist/tui.json"])
      },
    },
  }
}
```

——把**配置 schema 的 JSON Schema 表示**嵌入到文档站，让 web 文档自动反映最新配置。**这是「文档即代码」+「schema 自动同步」** 的优秀实践。

#### 20.2.3 Desktop 端 — Electron + Sidecar + WSL

`packages/desktop/src/main/index.ts:115-120` 的主进程启动器使用 **Effect.gen**（与 web/CLI 一致的 Effect 全栈）：

```typescript
const main = Effect.gen(function* () {
  contextMenu({ showSaveImageAs: true, showLookUpSelection: false, showSearchWithGoogle: false })

  // on macOS apps run in `/` which can cause issues with ripgrep
  try {
    process.chdir(homedir())
  } catch (error) {
    // ...
  }
  // ...
})
```

**`spawnLocalServer`（src/main/server.ts）** 在 desktop 内**作为子进程启动 opencode CLI server**——desktop 自身**不实现 agent 逻辑**，只做窗口/IPC/菜单/auto-update。**Sidecar pattern**：UI 端与 logic 端解耦，desktop 仅是「壳」，真正的 session 管理在 CLI server（同一份代码给 TUI/CLI/SDK 用）。

`SIDECAR_VERSION`（`main/index.ts:64`）：

```typescript
const SIDECAR_VERSION = process.env.OPENCODE_SIDECAR_V2 === "1" ? "v2" : "v1"
```

——**灰度发布机制**：通过 env var 让 desktop 启动 v1 或 v2 sidecar，**用户无感知 A/B test**。`@lydell/node-pty` 依赖（`desktop/package.json:32`）让 desktop 内嵌 PTY 跑 shell 命令——这与 laew 的 Bash 工具实现思路一致（laew 需 `portable-pty` crate）。

**WSL 子模块**（`main/wsl/`）7 个文件专门处理「Windows 上跑 Linux opencode」的 sidecar 进程：servers.ts / sidecar.ts / runtime.ts / policy.ts / startup.ts / ipc.ts。**WSL 边界**：Windows 桌面 → WSL 内的 opencode server，要处理 distro 选择、env 变量转换、路径转换、auto-start。**laew 当前完全无 WSL 支持**——但 macOS/Linux 用户占绝对多数（与 opencode 不同），借鉴价值低。

#### 20.2.4 App 端 — Vite SPA + Solid Router + Dialog 系统

`packages/app/src/components/` 包含 50+ 组件（dialog-select-model、dialog-select-directory、prompt-input、file-tree、directory-picker、command-palette 等）。`index.html:1-30` 是标准 Vite SPA 入口：

```html
<div id="root" class="flex flex-col h-dvh bg-v2-background-bg-deep p-px"></div>
<script src="/src/entry.tsx" type="module"></script>
```

**全屏 SPA**：`h-dvh` (dynamic viewport height) + `overscroll-none` + `overflow-hidden` —— TUI 风格的固定布局，**没有外层布局**。`oc-theme-preload.js`（`index.html:20`）在 React/Solid 渲染前**先跑主题预设**——避免「dark/light 模式闪烁」。

**Dialog 系统**是 app 的核心交互（50+ 组件里有 ~15 个 `dialog-*`）——每个 dialog 是一个独立 component，支持**键盘导航 + 命令面板唤起（Cmd+K）+ 撤销栈**。`directory-picker-domain.test.ts / directory-picker-policy.ts / directory-picker.test.ts` 三件套—— **domain / policy / view 三层**拆分：domain 是纯函数（解析策略），policy 是策略枚举，view 是 UI 组件。**这是 laew 完全可借鉴的解耦模式**（laew TUI 屏抽象可比照此模式）。

#### 20.2.5 共享 UI 组件 — `@opencode-ai/ui`

`packages/ui/src/components/` 列出 30+ 基础组件（`button/checkbox/dialog/dropdown/tabs/tooltip/file-icon/...`），所有客户端（app/desktop/web）都通过 `workspace:*` 依赖共享：

```json
"dependencies": {
  "@opencode-ai/ui": "workspace:*",  // desktop、app 都依赖
  // ...
}
```

**设计 token 集中化**：`@opencode-ai/ui/styles` 含 `app-icon.css / button.css / dialog.css` 等**组件级 CSS**（Tailwind utility 在外层，组件内联 CSS 处理复杂变体）—— 类似 laew 的 `theme.rs`。

`Storybook`（`packages/ui/src/components/button.stories.tsx` 等）—— 每个组件都有独立 story 文件，**让 UI 组件独立于 app 可视化调试**。laew 当前无此基建，但 TUI 屏抽象可以借鉴「每个屏有独立的 story fixture」（用于 tmux 自动化断言）。

#### 20.2.6 多端架构对 laew 的核心借鉴

| 维度 | opencode 做法 | laew 借鉴 | 优先级 |
|------|---------------|-----------|--------|
| CLI lazy handler | `Runtime.handlers` + `Command.withHandler` + `Effect.promise(load)` | 把 `clap` 子命令的 `match` 用 `async fn + Box<dyn FnOnce>` 懒加载 | P1 |
| Sidecar 模式 | UI 端（desktop/web）调子进程 CLI server | laew 当前是单进程，无 sidecar；TUI 模式下 future 可选「web 远程调 laew TUI」 | P3 |
| WSL sidecar | 7 个文件专门处理 Windows WSL 边界 | 不需要 | N/A |
| i18n middleware | URL prefix → cookie → Accept-Language 3 级 fallback | laew TUI 文案可加 `i18n` crate；当前 100% 中文无需 | P3 |
| Domain/Policy/View 拆分 | 组件文件 `*-domain.ts / *-policy.ts / *.tsx` | laew TUI 屏抽象可拆 `screen/{form,policy,view}.rs` | P1 |
| Storybook 屏 fixture | `*.stories.tsx` | laew TUI 屏 `test/screen/{name}.txt` 黄金样本 | P2 |
| `oc-theme-preload.js` | 渲染前先跑主题脚本防闪烁 | laew TUI 启动时直接输出 ANSI color，无需 | N/A |

### 20.3 HTTP Recorder 协议录制回放

`packages/http-recorder/` 是 opencode 内部**专用的 HTTP 录制/回放测试框架**——**不是**对接 VCR.py / Polly 那种通用工具，是**为 Effect 生态设计的、redaction 优先、CI 强一致的定制方案**。`package.json` 依赖：

```json
"dependencies": {
  "@opencode-ai/core": "workspace:*",
  "aws4fetch": "...",
  // ... effect 生态
}
```

`index.ts:1-6` 暴露 2 个 layer 工厂：

```typescript
export const HttpRecorder = { http, socket } as const
```

—— `http` 用于普通 HTTP 调用（OpenAI/Anthropic API），`socket` 用于 WebSocket（OpenAI Realtime / Anthropic Stream）。两者**共用同一份 cassette 持久化层**。

#### 20.3.1 Cassette 持久化 — JSON 文件 + 原子写

`src/cassette.ts:41-50` 的 `cassettePath` 严格防 path traversal：

```typescript
const cassettePath = (directory: string, name: string) => {
  if (!name || path.isAbsolute(name) || path.win32.isAbsolute(name) || name.split(/[\\/]/).includes(".."))
    throw new Error(`Invalid cassette name "${name}"`)
  const root = path.resolve(directory)
  const target = path.resolve(root, `${name}.json`)
  const relative = path.relative(root, target)
  if (!relative || relative.startsWith("..") || path.isAbsolute(relative))
    throw new Error(`Invalid cassette name "${name}"`)
  return target
}
```

——三道防线：① 名字不能为空 ② 绝对路径禁止 ③ 路径段不能含 `..` ④ 解析后必须在 root 内。**这是工程上对「测试 fixture 文件名 user-controlled」的严肃防御**。

`append` 流程（`cassette.ts:104-123`）—— **Semaphore 加锁 + 临时文件 + rename**：

```typescript
append: (name, interaction, metadata) =>
  appendLock.withPermit(
    Effect.gen(function* () {
      const entry = recorded.get(name) ?? { interactions: [], findings: [] }
      const interactions = [...entry.interactions, interaction]
      const interactionFindings = [...entry.findings, ...secretFindings(interaction)]
      const cassette = buildCassette(name, interactions, metadata)
      const findings = [...interactionFindings, ...secretFindings(cassette.metadata ?? {})]
      yield* failIfUnsafe(name, findings)
      const target = pathFor(name)
      yield* fs.makeDirectory(path.dirname(target), { recursive: true }).pipe(Effect.orDie)
      const temporary = `${target}.${crypto.randomUUID()}.tmp`
      yield* fs.writeFileString(temporary, formatCassette(cassette)).pipe(
        Effect.flatMap(() => fs.rename(temporary, target)),
        Effect.ensuring(fs.remove(temporary, { force: true }).pipe(Effect.catch(() => Effect.void))),
        Effect.orDie,
      )
      recorded.set(name, { interactions, findings: interactionFindings })
    }),
  ),
```

**5 步保护**：① Semaphore 串行化（避免多请求并发写）② **secretFindings 校验先**（含 secret 直接拒绝写）③ temp 文件 `.tmp` 后缀 ④ write → rename 原子提交 ⑤ `Effect.ensuring` 保证异常也清理 temp。**这是「test fixture 不能含真实密钥」 + 「多请求并发安全」 + 「写入崩溃可恢复」的三合一设计**。

#### 20.3.2 Secret 防护 — 7 种 pattern + env 检测

`src/redaction.ts:30-38` 内置 7 种 secret 正则：

```typescript
const SECRET_PATTERNS: ReadonlyArray<{ readonly label: string; readonly pattern: RegExp }> = [
  { label: "bearer token", pattern: /\bBearer\s+[A-Za-z0-9._~+/=-]{16,}\b/i },
  { label: "API key", pattern: /\bsk-[A-Za-z0-9][A-Za-z0-9_-]{20,}\b/ },
  { label: "Anthropic API key", pattern: /\bsk-ant-[A-Za-z0-9_-]{20,}\b/ },
  { label: "Google API key", pattern: /\bAIza[0-9A-Za-z_-]{20,}\b/ },
  { label: "AWS access key", pattern: /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/ },
  { label: "GitHub token", pattern: /\bgh[pousr]_[A-Za-z0-9_]{20,}\b/ },
  { label: "private key", pattern: /-----BEGIN [A-Z ]*PRIVATE KEY-----/ },
]
```

`envSecrets`（`redaction.ts:43-50`）**扫描进程环境变量**——开发者本地有 `ANTHROPIC_API_KEY=sk-ant-...` 时，自动检测并拒绝写入：

```typescript
const envSecrets = () =>
  Object.entries(process.env).flatMap(([name, value]) => {
    if (!value) return []
    if (!ENV_SECRET_NAMES.test(name)) return []
    if (value.length < 12) return []
    if (SAFE_ENV_VALUES.has(value.toLowerCase())) return []
    return [{ name, value }]
  })
```

——**这避免了「开发者本地录了一段交互，意外把 API key 写进 git 仓库」**。`SAFE_ENV_VALUES = {"fixture", "test", "test-key"}` 白名单是**显式逃生通道**（CI fixture 测试用例需要故意塞假 token）。

`secretFindings`（`redaction.ts:106-117`）**递归扫描 cassette 全部 string 字段**：

```typescript
export const secretFindings = (value: unknown): ReadonlyArray<SecretFinding> => {
  const environment = envSecrets()
  return stringEntries(value).flatMap(([entry]) => [
    ...SECRET_PATTERNS.filter((p) => p.pattern.test(entry.value)).map((p) => ({ path: entry.path, reason: p.label })),
    ...environment
      .filter((item) => entry.value.includes(item.value))
      .map((item) => ({ path: entry.path, reason: `environment secret ${item.name}` })),
  ])
}
```

——**这是 opencode 对「测试 fixture 不能含真实密钥」的工程级承诺**。laew 的 e2e test 当前用 mock LLM 绕过此问题，**但当 laew 升级到真实 LLM 测试时，必须抄这一套**。

#### 20.3.3 URL Redaction — 13 个 query param

`src/redaction.ts:15-28` 内置 13 个**已知会含敏感 token 的 query param**：

```typescript
const DEFAULT_REDACT_QUERY = [
  "access_token", "api-key", "api_key", "apikey", "code", "key",
  "signature", "sig", "token", "x-amz-credential", "x-amz-security-token", "x-amz-signature",
]
```

`redactUrl`（`redaction.ts:68-82`）把 userinfo 替换为 `REDACTED` + 把命中 query 替换为 `REDACTED` + 调用 `urlRedactor` 让 caller 自定义后处理。

`redactHeaders`（`redaction.ts:84-98`）—— **只保留 allowlist 的 header**（默认保留 `content-type` 等），其他全部 `REDACTED`：

```typescript
export const redactHeaders = (
  headers: Record<string, string>,
  allow: ReadonlyArray<string>,
  redact: ReadonlyArray<string> = DEFAULT_REDACT_HEADERS,
) => {
  const allowed = new Set(allow.map((name) => name.toLowerCase()))
  const redacted = redactionSet(redact, DEFAULT_REDACT_HEADERS)
  return Object.fromEntries(
    Object.entries(headers)
      .map(([name, value]) => [name.toLowerCase(), value] as const)
      .filter(([name]) => allowed.has(name))
      .map(([name, value]) => [name, redacted.has(name) ? REDACTED : value] as const)
      .toSorted(([a], [b]) => a.localeCompare(b)),
  )
}
```

**注意**：`toSorted` 让 header 排序后**匹配时与原始顺序无关**——这是 `matching.ts` 的基础（canonical form）。

#### 20.3.4 Canonical Matching — request diff 算法

`src/matching.ts:25-37` 的 `canonicalSnapshot`：

```typescript
export const canonicalSnapshot = (snapshot: RequestSnapshot): string =>
  JSON.stringify({
    method: snapshot.method,
    url: snapshot.url,
    headers: canonicalizeJson(snapshot.headers),  // 按 key 排序
    body: Option.match(decodeJson(snapshot.body), {
      onNone: () => snapshot.body,
      onSome: canonicalizeJson),                  // JSON 字段也按 key 排序
  })
```

——**深度规范化**：headers 按 key 排序、body JSON 按 key 排序（不破坏数组顺序）、非 JSON body 原样比对。**这允许 `Authorization` header 顺序、`{"a":1,"b":2}` vs `{"b":2,"a":1}` 等差异不影响匹配**。

`requestDiff`（`matching.ts:73-93`）生成**人类可读的 diff 报告**——`expected xxx, received yyy` + 限制 8 个不同：

```typescript
export const requestDiff = (expected: RequestSnapshot, received: RequestSnapshot): ReadonlyArray<string> => {
  const lines: string[] = []
  if (expected.method !== received.method) lines.push("method:", `  expected ${expected.method}, received ${received.method}`)
  if (expected.url !== received.url) lines.push("url:", `  expected ${expected.url}`, `  received ${received.url}`)
  const headers = headerDiffs(expected.headers, received.headers)
  if (headers.length > 0) lines.push("headers:", ...headers.slice(0, 8))
  // body 同样...
  return lines
}
```

**CI 中 fixture 不匹配时的输出**形如：
```
Fixture "anthropic-msg-001" does not match the current request:
  method:
    expected POST, received POST
  body:
    $ expected {"model":"claude-3-5-sonnet"}, received {"model":"claude-3-7-sonnet"}
    $.model expected "claude-3-5-sonnet", received "claude-3-7-sonnet"
```

——**让开发者一眼看出是哪个字段变了**。laew 的 e2e test 输出当前是「assertion failed at line 42」级别，**应升级到此粒度**。

#### 20.3.5 Replay State — Sequenced + Unused 检测

`src/recorder.ts:26-62` 的 `makeReplayState`：

```typescript
export const makeReplayState = <T>(
  cassette: CassetteService.Interface,
  name: string,
  project: (interactions: ReadonlyArray<Interaction>) => ReadonlyArray<T>,
): Effect.Effect<ReplayState<T>, never, Scope.Scope> =>
  Effect.gen(function* () {
    const load = yield* Effect.cached(cassette.read(name).pipe(Effect.map(project)))
    const position = yield* SynchronizedRef.make(0)

    yield* Effect.addFinalizer(() =>
      Effect.gen(function* () {
        const used = yield* SynchronizedRef.get(position)
        if (used === 0) return yield* Effect.void
        const interactions = yield* load.pipe(Effect.orDie)
        if (used < interactions.length)
          return yield* Effect.die(
            new Error(`Unused recorded interactions in ${name}: used ${used} of ${interactions.length}`),
          )
        return yield* Effect.void
      }),
    )
    // ...
  })
```

**`addFinalizer` 钩子**：scope 结束时检查 `used < interactions.length`——**如果录了 10 个交互但测试只用到 8 个，scope 关闭时 die**。**这避免了「fixture 里有 stale 交互」**——强制每个 fixture 被**完整使用**，否则测试失败。

**`SynchronizedRef` 原子计数器**：每个 `claim` 都自增，确保**多线程并发 claim 不会错位**。

`claim`（`recorder.ts:49-60`）—— 取出 `interactions[index]`，调用 `validate`，**通过则返回 `{interaction, index+1}`，失败则 die**：

```typescript
claim: (validate) =>
  Effect.flatMap(load, (interactions) =>
    SynchronizedRef.modifyEffect(position, (index) =>
      Effect.gen(function* () {
        const interaction = interactions[index]
        yield* validate(interaction, index, interactions)
        if (interaction === undefined) return yield* Effect.die("Replay validation accepted a missing interaction")
        return [{ interaction, index }, index + 1] as const
      }),
    ),
  ),
```

——**index 与 interactions 是函数式原子操作**，符合 Effect 不可变语义。

#### 20.3.6 Auto Mode — CI vs 本地

`src/recorder.ts:11-18`：

```typescript
const isCI = () => {
  const value = process.env.CI
  return value !== undefined && value !== "" && value !== "false" && value !== "0"
}

export const resolveAutoMode = (
  cassette: CassetteService.Interface,
  name: string,
): Effect.Effect<"record" | "replay" | "passthrough"> =>
  Effect.gen(function* () {
    if (isCI()) return "replay"
    return (yield* cassette.exists(name)) ? "replay" : "record"
  })
```

**4 模式矩阵**：

| 模式 | 行为 | 何时用 |
|------|------|--------|
| `passthrough` | 直接转发真实请求，无录制无回放 | 开发调试 |
| `record` | 真发请求 + 写入 cassette | 本地首次录 |
| `replay` | 只读 cassette，不发请求 | CI / 本地已录 |
| `auto` | `isCI() → replay` 否则 `exists → replay : record` | 默认 |

**`isCI` 容错**：CI 变量可能是 `"true"`, `"1"`, `"yes"`, 也可能是 `"false"`, `"0"`, 或空——这里**只把空/缺/`false`/`0` 视为非 CI**，其他全视为 CI。**这避免了某些 CI 工具（如 GitHub Actions）传 `CI=true` 而其他传 `CI=1` 不识别的问题**。

`recordingLayer`（`src/internal-effect.ts:93-182`）的 record 分支用 `Deferred + Ref` 串行化录制：

```typescript
if (mode === "record") {
  const initial = yield* Deferred.make<void>()
  yield* Deferred.succeed(initial, undefined)
  const tail = yield* Ref.make(initial)
  return HttpClient.make((request) =>
    Effect.gen(function* () {
      const completed = yield* Deferred.make<void>()
      const previous = yield* Ref.modify(tail, (current) => [current, completed])
      return yield* Effect.gen(function* {
        // ... record
        yield* Deferred.await(previous)  // 等前一个写完
        yield* cassetteService.append(name, interaction, options.metadata)
        // ...
      }).pipe(Effect.ensuring(Deferred.succeed(completed, undefined)))
    }),
  )
}
```

**`tail` 是「上一个请求的 completion Deferred」**：新请求 await `previous` 确保**录制顺序 = 请求发起顺序**。这避免「请求 A 完成时，请求 B 已经写完，A 覆盖 B」。

#### 20.3.7 WebSocket Recorder — message-by-message

`src/websocket.ts:90-135` 的 record 分支：

```typescript
if (mode === "record") {
  return {
    open: (request) =>
      Effect.gen(function* () {
        const events: WebSocketEvent[] = []
        const connection = yield* options.live.open(request)
        const closed = yield* Ref.make(false)
        const closeLock = yield* Semaphore.make(1)
        return {
          sendText: (message) =>
            Effect.sync(() => events.push(redactEvent(textEvent("client", message)))).pipe(
              Effect.andThen(connection.sendText(message)),
            ),
          messages: connection.messages.pipe(
            Stream.tap((message) => Effect.sync(() => events.push(/* server event */))),
          ),
          close: closeLock.withPermit(Effect.gen(function* () {
            if (yield* Ref.get(closed)) return
            yield* connection.close
            yield* options.cassette.append(/* events */).pipe(Effect.orDie)
            yield* Ref.set(closed, true)
          })),
        }
      }),
  }
}
```

—— **`close` 时统一 flush events 数组**（不是收到一条写一条），避免中途断电丢数据。`closeLock` Semaphore 防止「close 触发 2 次」（signal + finally 双触发）。

replay 分支（`websocket.ts:138-172`）的 client 端**断言每条 send 匹配 fixture 的下一条 client event**：

```typescript
sendText: (message) =>
  SynchronizedRef.updateEffect(position, (index) =>
    assertClientEvent(message, client[index], index, options.compareClientMessagesAsJson === true).pipe(
      Effect.as(index + 1),
    ),
  ),
```

`assertClientEvent`（`websocket.ts:53-62`）支持两种比较模式：原始字符串 vs canonical JSON（`compareClientMessagesAsJson: true` 让 `{"a":1,"b":2}` ≡ `{"b":2,"a":1}`）。

#### 20.3.8 HTTP Recorder 对 laew 的核心借鉴

| 维度 | opencode 做法 | laew 借鉴 | 优先级 |
|------|---------------|-----------|--------|
| Cassette 原子写 | Semaphore + temp file + rename | laew testReport 录制 LLM 响应时抄此模式 | P0 |
| Secret 检测 | 7 种正则 + env 扫描 + `test/fixture/test-key` 白名单 | laew e2e test 用 mock LLM 暂不需要；真 LLM 测试时必须有 | P1 |
| Canonical matching | header 按 key 排序 + body JSON 排序 | laew e2e 协议 wire 校验已有；canonical 化可加 | P1 |
| Unused interaction 检测 | scope 关闭时 `used < length → die` | laew testReport 录的 fixture 数量硬编码对照 | P2 |
| Auto mode `isCI()` | 4 模式 + 容错 | laew e2e 已有 CI=1 路径，可加 auto | P0 |
| `Deferred + Ref` 串行 | 用 `tail` Deferred 链表保证录制顺序 | laew test fixture 顺序一致性可学 | P2 |
| WebSocket Recorder | message-by-message + close-time flush | laew 当前无 WebSocket；流式 SSE 录制可抄 | P3 |
| Path traversal 三道防线 | `name/absolute/..` 拒绝 | laew `testReport/` 输入校验必须做 | P1 |

### 20.4 Slack 集成 / 企业 IM

`packages/slack/` 是 146 行的**极简 Slack bot**（`src/index.ts`），不依赖 Effect，只用 `@slack/bolt` 3.17.1。**它是 opencode 「IM 入口」的最小实现参考**——比 enterprise 包小一个数量级。

#### 20.4.1 Bolt App + Socket Mode

`src/index.ts:4-9`：

```typescript
const app = new App({
  token: process.env.SLACK_BOT_TOKEN,
  signingSecret: process.env.SLACK_SIGNING_SECRET,
  socketMode: true,
  appToken: process.env.SLACK_APP_TOKEN,
})
```

**3 个 token + socketMode=true**：避免暴露 HTTP endpoint（**不需公网 webhook**），通过 WebSocket 长连接收 Slack 推送事件。**安全设计**：bot 进程在用户机器上跑（不是云 server），Slack 通过 WSS 反向连接过来。

#### 20.4.2 Opencode 子进程嵌入

`src/index.ts:17-39`：

```typescript
const opencode = await createOpencode({
  port: 0,  // 随机分配端口
})
console.log("✅ Opencode server ready")

const sessions = new Map<string, { client: any; server: any; sessionId: string; channel: string; thread: string }>()
void (async () => {
  const events = await opencode.client.event.subscribe()
  for await (const event of events.stream) {
    if (event.type === "message.part.updated") {
      const part = event.properties.part
      if (part.type === "tool") {
        for (const [_sessionKey, session] of sessions.entries()) {
          if (session.sessionId === part.sessionID) {
            void handleToolUpdate(part, session.channel, session.thread)
            break
          }
        }
      }
    }
  }
})()
```

**`createOpencode({ port: 0 })` 在 slack bot 进程内启动一个 opencode server**（SDK 内置的能力）——`port: 0` 让 OS 分配空闲端口。`opencode.client.event.subscribe()` 是 **SSE 订阅**，**实时接收 agent 执行进度**。

`handleToolUpdate`（`src/index.ts:41-51`）把每次工具调用完成推送到 slack thread：

```typescript
async function handleToolUpdate(part: ToolPart, channel: string, thread: string) {
  if (part.state.status !== "completed") return
  const toolMessage = `*${part.tool}* - ${part.state.title}`
  await app.client.chat.postMessage({ channel, thread_ts: thread, text: toolMessage }).catch(() => {})
}
```

——**O(N) 线性查找 session**：`sessions.entries()` for-loop 找匹配 `sessionId` 的 session。**O(1) 优化**：用 `Map<sessionId, {channel, thread}>` 而不是 `Map<sessionKey, full session>`——但 slack 当前实现**每次遍历所有 session**，简单但慢（30+ 并发 thread 会有问题）。

#### 20.4.3 Session 路由 — per-thread 隔离

`src/index.ts:58-136` 的 `app.message` 处理器：

```typescript
app.message(async ({ message, say }) => {
  if (message.subtype || !("text" in message) || !message.text) return

  const channel = message.channel
  const thread = (message as any).thread_ts || message.ts
  const sessionKey = `${channel}-${thread}`

  let session = sessions.get(sessionKey)
  if (!session) {
    const createResult = await client.session.create({ body: { title: `Slack thread ${thread}` } })
    if (createResult.error) { /* error path */ return }
    session = { client, server, sessionId: createResult.data.id, channel, thread }
    sessions.set(sessionKey, session)

    // Auto-share: 把 session 链接推送到 thread
    const shareResult = await client.session.share({ path: { id: createResult.data.id } })
    if (!shareResult.error && shareResult.data) {
      const sessionUrl = shareResult.data.share?.url
      await app.client.chat.postMessage({ channel, thread_ts: thread, text: sessionUrl })
    }
  }

  const result = await session.client.session.prompt({
    path: { id: session.sessionId },
    body: { parts: [{ type: "text", text: message.text }] },
  })
  // ... error handling
  const responseText =
    response.info?.content ||
    response.parts?.filter((p: any) => p.type === "text").map((p: any) => p.text).join("\n") ||
    "I received your message but didn't have a response."
  await say({ text: responseText, thread_ts: thread })
})
```

**per-thread-session 路由**：`sessionKey = ${channel}-${thread}`，**每个 Slack 线程 = 一个 opencode session**。新线程自动 `session.create` + `session.share` + 推送 share URL——**让用户能在 web 端跟进 agent 执行的完整历史**。

**`responseText` 三段式 fallback**（`src/index.ts:124-131`）：`info.content` → `parts[].text` 拼接 → 「I received your message but didn't have a response.」。**这是 laew 完全没有的 pattern**——laew 当前直接把 message 当作 tool_result 字符串丢回去，没有 fallback。

**无 session.end / cleanup**：`sessions` Map 只增不删，**bot 跑一天会内存泄漏**。`Map.clear()` 周期性触发是个轻量级修复（按 thread last-active time LRU）。

#### 20.4.4 Slack 对 laew 的核心借鉴

| 维度 | opencode 做法 | laew 借鉴 | 优先级 |
|------|---------------|-----------|--------|
| Sidecar 嵌入 | `createOpencode({port:0})` + `client.event.subscribe()` SSE | laew 暂不需要；`./laew -p` 已是单进程 | N/A |
| per-thread session | `Map<\`${channel}-${thread}\`, session>` | laew TUI 是单 session | N/A |
| Auto-share 推送 | session 创建后立即 `session.share` + 推送 URL | laew TUI 不需要 share（无 web 端） | N/A |
| Tool update 实时推 | 订阅 `message.part.updated` + 过滤 tool 类型 | laew TUI 内部直接渲染，无需 IM 推送 | N/A |
| 三段式 fallback | `info.content → parts[].text → 默认文本` | **laew 应学**：当前 tool_result 直接是 string，没有 fallback | P1 |
| Socket Mode（无需 webhook） | `socketMode: true` + `appToken` | laew 当前无 IM；TUI 不需要 | N/A |
| Session cleanup 缺 | Map 只增不删 | laew 当前 LRU eviction 缺，可加 | P1 |

### 20.5 对 laew 的借鉴路线图

| 优先级 | 工作量 | 收益 | 内容 | 落地章节 |
|--------|--------|------|------|----------|
| **P0**（1-2 天） | 小 | 极高 | ① testReport cassette 原子写 + secret 防护（20.3.1+20.3.2） ② `isCI()` 4 模式 auto 切换（20.3.6） ③ `info.content → parts → 默认` 三段式 fallback（20.4.3） | 20.3/20.4 |
| **P1**（3-5 天） | 中 | 高 | ④ CLI lazy handler（20.2.1） ⑤ Domain/Policy/View 屏拆分（20.2.4） ⑥ canonical matching + request diff（20.3.4） ⑦ session share 与 lazy migrate（20.1.4） ⑧ Storage trait + S3/R2 双实现（20.1.2） | 20.1/20.2/20.3 |
| **P2**（1-2 周） | 大 | 中 | ⑨ Hono-style OpenAPI 自动生成（20.1.6） ⑩ `Domain` 三层（domain/policy/view）（20.2.4） ⑪ Sidecar mode（20.4.2） ⑫ Unused interaction 检测（20.3.5） ⑬ timingSafeEqual 鉴权分级（20.1.5） | 20.1/20.2/20.3/20.4 |
| **P3**（待评估） | 极大 | 待评估 | ⑭ Astro Starlight 文档站（20.2.2） ⑮ Storybook 屏 fixture（20.2.5） ⑯ WebSocket recorder（20.3.7） ⑰ WSL sidecar（20.2.3） | 20.2/20.3 |

### 20.6 综合：四维度交叉点

4 个新维度**共同指向一个隐含主题**——**「agent 走出聊天框」**：

1. **Enterprise Durable Object** = 「让 agent 跑在云上」（持久化 + 部署）
2. **多端 UI 框架** = 「让 agent 跑在多个壳里」（CLI/TUI/App/Desktop/Web/Slack）
3. **HTTP Recorder** = 「让 agent 跑得快」（CI 录制回放 + secret 防护 + 协议稳定）
4. **Slack 集成** = 「让 agent 跑进企业 IM」（per-thread session + 实时工具推送 + share URL）

**共同设计哲学**：
- **效果 = 协议 × 持久化 × UI × 测试**（protocol × persistence × UI × testing）
- **每个端都有「极薄壳 + 共享核心」**：slack 包 146 行、CLI handler 全部 lazy load、desktop 是 Electron 壳内嵌 sidecar、enterprise 是 SolidStart 壳 + Hono API
- **测试优先**（HTTP Recorder 全章占 4 节中 6 个小节）：每加新能力立即写 fixture，**禁止「能力领先于测试」**

**对 laew 的统一启示**：laew 当前是「单进程 CLI + mock LLM e2e」，**与「生产级企业 agent」的距离**主要由 4 个 gap 衡量：
- **gap-1**（持久化）：SQLite + 内存态，无 R2/S3 抽象 → 学 20.1
- **gap-2**（多端）：单 TUI + CLI，无 GUI → 学 20.2（聚焦 lazy handler 模式）
- **gap-3**（测试稳定性）：mock LLM 协议校验，无 cassette 录制回放 → 学 20.3
- **gap-4**（IM 入口）：无企业 IM 集成 → 学 20.4（聚焦三段式 fallback）

### 20.7 关键文件路径汇总

| 维度 | 文件路径 | 行数/范围 | 关键导出 |
|------|----------|-----------|----------|
| **Enterprise** | | | |
| Storage Adapter | `/usr/local/LsmGitOpenSource/opencode/packages/enterprise/src/core/storage.ts` | 130 | `Storage.{read,write,remove,list,update}` |
| Share namespace | `/usr/local/LsmGitOpenSource/opencode/packages/enterprise/src/core/share.ts` | 233 | `Share.{create,get,remove,sync,data,syncOld,Errors}` |
| HTTP API | `/usr/local/LsmGitOpenSource/opencode/packages/enterprise/src/routes/api/[...path].ts` | 172 | Hono app + 4 endpoints + OpenAPI |
| Share 页面 SSR | `/usr/local/LsmGitOpenSource/opencode/packages/enterprise/src/routes/share/[shareID].tsx` | 418 | `getData` query + DataProvider + WorkerPool |
| Enterprise tests | `/usr/local/LsmGitOpenSource/opencode/packages/enterprise/test/core/share.test.ts` | 80+ | bun:test 5 个 share 场景 |
| Vite + Nitro config | `/usr/local/LsmGitOpenSource/opencode/packages/enterprise/vite.config.ts` | 35 | cloudflare preset + nodeCompat |
| **多端 UI** | | | |
| CLI 入口 | `/usr/local/LsmGitOpenSource/opencode/packages/cli/src/index.ts` | 33 | `Runtime.handlers` + `Runtime.run` |
| CLI framework | `/usr/local/LsmGitOpenSource/opencode/packages/cli/src/framework/runtime.ts` | 80 | `Runtime.handlers` + `Runtime.run` + `Runtime.handler` |
| Web astro 配置 | `/usr/local/LsmGitOpenSource/opencode/packages/web/astro.config.mjs` | 325 | starlight + 20 locales + configSchema hook |
| Web i18n middleware | `/usr/local/LsmGitOpenSource/opencode/packages/web/src/middleware.ts` | 95 | `onRequest` 3-level locale |
| Desktop main | `/usr/local/LsmGitOpenSource/opencode/packages/desktop/src/main/index.ts` | 120+ | `main` Effect.gen + sidecar 启动 |
| Desktop WSL | `/usr/local/LsmGitOpenSource/opencode/packages/desktop/src/main/wsl/` | 7 文件 | servers/sidecar/runtime/policy/startup/ipc |
| Desktop preload | `/usr/local/LsmGitOpenSource/opencode/packages/desktop/src/preload/index.ts` | — | contextBridge IPC |
| App entry HTML | `/usr/local/LsmGitOpenSource/opencode/packages/app/index.html` | 30 | SPA root + theme preload |
| App components | `/usr/local/LsmGitOpenSource/opencode/packages/app/src/components/` | 50+ | dialog/file-tree/prompt-input/... |
| 共享 UI | `/usr/local/LsmGitOpenSource/opencode/packages/ui/src/components/` | 30+ | button/checkbox/dialog/tabs/... |
| **HTTP Recorder** | | | |
| Cassette 持久化 | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/cassette.ts` | 180 | `Service` + `fileSystem` + `memory` |
| Schema | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/schema.ts` | 88 | `InteractionSchema` + `CassetteSchema` |
| Types | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/types.ts` | 109 | `RequestSnapshot` + `RedactOptions` |
| Recorder | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/recorder.ts` | 63 | `resolveAutoMode` + `makeReplayState` |
| Redaction | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/redaction.ts` | 118 | `REDACTED` + 7 SECRET_PATTERNS + `redactUrl` + `redactHeaders` + `secretFindings` |
| Matching | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/matching.ts` | 107 | `canonicalSnapshot` + `defaultMatcher` + `requestDiff` |
| HTTP effect layer | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/effect.ts` | 26 | `http(name, options)` Layer 工厂 |
| Internal HTTP layer | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/internal-effect.ts` | 190 | `recordingLayer` + 4 模式分支 |
| WebSocket recorder | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/websocket.ts` | 174 | `makeWebSocketExecutor` |
| Socket helper | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/socket.ts` | — | `socket(name, options)` |
| Public API | `/usr/local/LsmGitOpenSource/opencode/packages/http-recorder/src/index.ts` | 19 | `HttpRecorder.http + HttpRecorder.socket` |
| **Slack** | | | |
| Slack bot | `/usr/local/LsmGitOpenSource/opencode/packages/slack/src/index.ts` | 146 | Bolt App + per-thread session + 工具推送 |

### 20.8 本轮不重复声明

为避免与前 7 轮（特别是第七轮 Edit/grep/Bash、第六轮 Effect/Schema/LayerNode/Durable Object、第五/四/三/二/一轮）混淆，本章**仅覆盖以下 4 个全新维度**，明确不重复以下内容：

- **agent 内核（Effect runtime、provider、tool 系统、session 持久化、LLMEvent 流）** —— 已在第 16/17/18/19 章深入，本章不重复
- **Edit/grep/Bash 工具实现** —— 第 19 章 +1200 行已完整覆盖，本章不重复
- **测试体系**（vitest-evals/录制回放）—— 第 3 轮已深入，本章**仅**深入「**HTTP 协议级**的录制回放」（20.3），不重复普通单元测试
- **Cloudflare Workers 基础**（compatibilityDate/nodeCompat/preset=cloudflare-module）—— 第 6 轮已深入「LayerNode DI/Durable Object」，本章**仅**深入 enterprise 实际部署的 `Storage` 抽象与 share 模型（20.1.2-20.1.4），不重复 CF 平台基础
- **Astro/Starlight 文档系统** —— 不深入 Starlight 内部，本章**仅**关注 i18n middleware 模式（20.2.2）
- **Electron 主进程架构** —— 不深入 BrowserWindow/lifecycle，本章**仅**关注 sidecar 嵌入（20.2.3）
- **Effect 全栈 DI / Layer 注入模式** —— 第 6 轮已深入，本章**仅**关注 recorder 的 4 模式 auto 切换（20.3.6），不重复 `Layer.effect` 用法
- **Session/Provider/Message/Part 数据模型** —— 第 5 轮已深入，本章**仅**关注 share 时 `z.custom<>()` 轻量级取舍（20.1.3）与 `data` 字典重组（20.1.7）

**本轮新增的关键发现**（与前 7 轮不重叠）：
1. **「lazy migrate + snapshot 优先」双轨数据迁移**（20.1.4）—— laew 无此需求但模式值得学
2. **「极薄壳 + 共享核心」**多端架构（20.2）—— laew 升级 web/desktop 时的范式
3. **Secret 防护 7 正则 + env 扫描 + test-key 白名单**（20.3.2）—— 真实 LLM 测试必备
4. **`Deferred + Ref` 链表保证录制顺序**（20.3.6）—— 多请求并发 fixture 顺序保证
5. **Path traversal 三道防线**（20.3.1）—— 测试 fixture 路径校验
6. **`info.content → parts → 默认文本` 三段式 fallback**（20.4.3）—— 通用 agent 响应回填模式
7. **`compatibilityDate` 严肃工程**（20.1.1）—— Cloudflare 部署冻结日期

**总章节行数**：本章节约 **1100+ 行**，覆盖 enterprise 4 文件 + cli 2 文件 + web 2 文件 + desktop 1 文件 + http-recorder 8 文件 + slack 1 文件 + 1 个 cross-cutting 段。
# opencode 第十轮深挖：8 大新维度

> **生成时间**：2026-09-07
> **分析对象**：`/usr/local/LsmGitOpenSource/opencode`（TypeScript/Bun + Effect 全栈 DI 架构）
> **对应知识库**：`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/docs/Agent源码调研/opencode.md`（~5,143 行，前 9 轮累计）
> **本轮定位**：在前 9 轮已覆盖「Effect/LayerNode/Session/Provider/MCP/Edit/Grep/Bash/CRDT/Skill/Workshop/Telemetry/Session持久化/LSP/IDE/Tool权限沙箱/Hook/Plugin/Skill/多租户/团队记忆/TUI渲染/Enterprise Durable Object/多端UI/HTTP Recorder/Slack」基础上，**仅**对 8 个全新维度做深度源码剖析，每个维度 ≥300 行。

---

## 目录

1. [CrashDump 与错误恢复](#1-crashdump-与错误恢复)
2. [WebUI 与 DesktopApp](#2-webui-与-desktopapp)
3. [OAuth 认证与多账号](#3-oauth-认证与多账号)
4. [i18n 国际化](#4-i18n-国际化)
5. [Release 工程化与 AutoUpdate](#5-release-工程化与-autoupdate)
6. [WebSocket 与 SSE](#6-websocket-与-sse)
7. [DevContainer 与容器化](#7-devcontainer-与容器化)
8. [CRDT 与多端冲突](#8-crdt-与多端冲突)

附录：[laew gap 清单 L38-L78](#附录laew-gap-清单-l38-l78)

---

## 1. CrashDump 与错误恢复

opencode 的崩溃恢复体系横跨 **CLI sidecar + Electron main + Renderer** 三层，核心设计哲学是「**任何可恢复的异常都走 IPC 回主进程 + 日志落盘 + crashpad**」，**任何不可恢复的异常都强制重启 sidecar**。

### 1.1 三层崩溃捕获

#### 1.1.1 Electron Crashpad（OS 级别）

`packages/desktop/src/main/logging.ts:36-42` 启动 Crashpad 落盘到 `userData/Crashpad/`：

```typescript
export function initCrashReporter() {
  const dir = join(app.getPath("userData"), "Crashpad")
  mkdirSync(dir, { recursive: true })
  app.setPath("crashDumps", dir)
  crashReporter.start({ uploadToServer: false, compress: true })
  write("crash", "crash reporter started", { path: dir })
}
```

**关键点**：

- `uploadToServer: false` —— Crashpad dump 只落盘不上传，**隐私默认本地保留**（与 claudecode 的"用户 opt-in 才上传"一致）
- `compress: true` —— Electron 默认 compress=true，会把 dmp 压缩成 `.dmp.zip`（节省磁盘）
- **路径隔离**：`app.setPath("crashDumps", dir)` 把 dumps 重定向到 `userData/Crashpad` 而非 Electron 默认的 `~/.config/Electron/Crashpad`，**避免污染全局目录**

`logging.ts:194-210` 控制台 broken-pipe 兜底：

```typescript
function initConsoleTransport() {
  if (app.isPackaged) {
    log.transports.console.level = false  // 生产环境关闭 console
    return
  }
  const write = log.transports.console.writeFn.bind(log.transports.console)
  log.transports.console.writeFn = (options) => {
    try {
      write(options)
    } catch (err) {
      if (!isBrokenPipe(err)) throw err
      log.transports.console.level = false  // 一旦 EPIPE 就永久关闭 console
    }
  }
}
```

**laew 借鉴**：Rust 当前没有任何 panic hook —— L38 gap。`human-panic` crate 是事实标准（claudecode 用的也是 human-panic），但 opencode 选择 Electron Crashpad 是因为其**直接捕获 native crash**（Rust panic 时传不下来的 C++ 栈、GPU driver crash 等）。

#### 1.1.2 Renderer 渲染进程崩溃（Chromium 级别）

`packages/desktop/src/main/index.ts:234-240` 监听 `render-process-gone`：

```typescript
app.on("child-process-gone", (_event, details) => {
  writeLog("utility", "child process gone", { details }, "error")
})
app.on("render-process-gone", (_event, webContents, details) => {
  writeLog("window", "app render process gone", { url: safeWebContentsURL(webContents), details }, "error")
})
```

`packages/desktop/src/main/unresponsive.ts:8-69` 是 opencode **独创的 renderer unresponsive 采样器**：

```typescript
const sampleInterval = 1000  // 每 1s 采样一次
const samplePeriod = 15000   // 持续 15s

export function createUnresponsiveSampler(win: BrowserWindow, name: string) {
  let sampleTimer: ReturnType<typeof setTimeout> | undefined
  let stopTimer: ReturnType<typeof setTimeout> | undefined
  let sampling = false
  const samples = new Map<string, number>()  // stack → count

  const collect = async () => {
    if (!active()) return
    const stack = await win.webContents.mainFrame
      .collectJavaScriptCallStack()  // Electron 28+ 提供
      .catch((error) => {
        writeLog("window", "failed to collect unresponsive sample", { window: name, error }, "error")
        return undefined
      })
    if (!active()) return
    if (stack) samples.set(stack, (samples.get(stack) ?? 0) + 1)
    schedule()
  }

  const stopAndFlush = () => {
    const wasSampling = sampling
    sampling = false
    clearTimers()
    if (samples.size === 0) return wasSampling
    const entries = [...samples.entries()].sort((a, b) => b[1] - a[1])  // 按频次排序
    const total = entries.reduce((sum, entry) => sum + entry[1], 0)
    const message = [
      "renderer unresponsive samples",
      `Window: ${name}`,
      `URL: ${safeWindowURL(win)}`,
      ...entries.map((entry) => `<${entry[1]}> ${entry[0]}`),
      `Total Samples: ${total}`,
    ].join("\n")
    writeLog("window", message, undefined, "error")
    samples.clear()
    return wasSampling
  }

  const start = () => {
    if (sampling || win.isDestroyed() || win.webContents.isDestroyed() || win.webContents.isDevToolsOpened()) return
    sampling = true
    samples.clear()
    schedule()
    stopTimer = setTimeout(stopAndFlush, samplePeriod)
  }
  win.on("closed", stopAndFlush)
  return { start, stopAndFlush }
}
```

**核心机制**：

1. **Window 切换**到后台时窗口主线程被节流，会触发 Chromium 判定为 unresponsive
2. **采样栈**：用 `webContents.mainFrame.collectJavaScriptCallStack()` 抓当前 JS 调用栈（Electron 28+ 提供，Electron 41 是当前 latest）
3. **频次排序**：15s 窗口内对每个 distinct stack 计数，最后按频次排序输出 Top-N
4. **格式**：`<{count}> {stack}` —— 用尖括号包裹计数，stack 单行展示，方便 grep

`packages/desktop/src/main/windows.ts:33` 还有 `jsCallStacksDocumentPolicy = "include-js-call-stacks-in-crash-reports"`，这是 Chromium 的 `Document-Policy` header，配合 crashpad 时**把 JS 调用栈写入 native crash dump**。

`index.ts:194-196` 在所有平台开启此 feature：

```typescript
app.commandLine.appendSwitch("enable-features", features ? `${jsCallStackFeature},${features}` : jsCallStackFeature)
```

#### 1.1.3 Sidecar (opencode server) 崩溃

`packages/desktop/src/main/index.ts:88-93, 234-240`：

```typescript
async function killSidecar() {
  if (!server) return
  const current = server
  server = null
  await current.stop()
}

app.on("child-process-gone", (_event, details) => {
  writeLog("utility", "child process gone", { details }, "error")
})
```

`packages/desktop/src/main/server.ts:73-85` 监听 utility process 退出：

```typescript
const onProcessGone = (_event: unknown, details: Details) => {
  if (details.type !== "Utility" || details.name !== SIDECAR_SERVICE_NAME) return
  options.onStderr?.(`utility process gone reason=${details.reason} exitCode=${details.exitCode}`)
}
app.on("child-process-gone", onProcessGone)
child.once("exit", (code) => {
  exited = true
  app.off("child-process-gone", onProcessGone)
  options.onExit?.(code)
  exit.resolve(code)
})
```

**Sidecar 重启策略**：从 `index.ts:407-409` 看，sidecar 在 `loadingTask` fiber 中启动，**没有自动重启逻辑**（一旦 sidecar 死，主进程也视为异常）。**这是有意的设计**：sidecar 死了 = opencode 死了，用户重启整个 app 即可。

`packages/desktop/src/main/sidecar.ts:67-70` 是 sidecar 自己崩溃时的兜底：

```typescript
async function start(command: StartCommand) {
  try {
    // ...
    listener = await Server.listen({...})
    parentPort.postMessage({ type: "ready" })
  } catch (error) {
    parentPort.postMessage({ type: "error", error: serializeError(error) })
    setImmediate(() => process.exit(1))  // 失败立即退出
  }
}
```

### 1.2 日志系统（带自动清理）

`packages/desktop/src/main/logging.ts:9-14` 定义日志常量：

```typescript
const MAX_LOG_AGE_DAYS = 7        // 7 天自动清理
const TAIL_LINES = 1000           // tail 默认读最近 1000 行
const EXPORT_WINDOW = 24 * 60 * 60 * 1000  // 导出窗口 24h
const MAX_EXPORT_FILE_SIZE = 50 * 1024 * 1024  // 单文件 50MB 上限
const NET_LOG_SIZE = 20 * 1024 * 1024         // 网络日志 20MB 上限
```

`logging.ts:101-131` 启动时清理 7 天前的旧日志：

```typescript
function initRunDirectory() {
  root = join(app.getPath("userData"), "logs")
  run = join(root, stamp())  // 每次启动一个独立子目录
  mkdirSync(run, { recursive: true })
}

function cleanup() {
  const dir = root || dirname(log.transports.file.getFile().path)
  const cutoff = Date.now() - MAX_LOG_AGE_DAYS * 24 * 60 * 60 * 1000
  for (const entry of readdirSync(dir)) {
    const file = join(dir, entry)
    try {
      const info = statSync(file)
      if (info.mtimeMs < cutoff) rmSync(file, { recursive: true, force: true })
    } catch {
      continue
    }
  }
}
```

**「每次启动一个独立 run 目录」** 是 opencode 日志系统的核心设计：`logs/{ISO timestamp}/{scope}.log`，**避免单文件被锁、避免写入竞争、避免 huge file**。

`logging.ts:51-73` 用户主动导出 debug：

```typescript
export async function exportDebugLogs() {
  const restartNetLog = netLog.currentlyLogging
  if (restartNetLog) {
    await netLog.stopLogging().catch((error) => write("network", "failed to stop net log", { error }))
  }
  const output = join(app.getPath("downloads"), `opencode-debug-${stamp()}.zip`)
  try {
    write("main", "exporting debug logs", { output })
    await writeZip(output, [
      { name: "manifest.json", data: Buffer.from(JSON.stringify(manifest(), null, 2)) },
      ...collect(root, "desktop"),
      ...serverLogRoots().flatMap((dir, i) => collect(dir, `server-${i + 1}`)),
      ...collect(app.getPath("crashDumps"), "crashpad"),
    ])
    shell.showItemInFolder(output)  // 弹 Finder/Explorer
    return output
  } finally {
    if (restartNetLog) {
      await startNetLog().catch((error) => write("network", "failed to restart net log", { error }))
    }
  }
}
```

**导出的 zip 结构**：

```
opencode-debug-{timestamp}.zip
├── manifest.json                          # version/packaged/uptime/userData
├── desktop/main.log                       # 主进程日志
├── desktop/renderer.log                   # renderer 日志
├── desktop/server.log                      # sidecar stdout/stderr
├── desktop/network.netlog                 # Chromium 网络抓包
├── desktop/crashpad/                      # crash dumps
│   ├── xxx.dmp.zip
│   └── yyy.dmp.zip
├── server-1/log/                          # server log root 1
└── server-2/log/                          # server log root 2
```

`logging.ts:152-155` 同时扫描 `XDG_DATA_HOME/opencode/log` + `userData/opencode/log` 两个根（因为 sidecar 可能继承不同的 `XDG_STATE_HOME`）。

### 1.3 Net Log (Chromium 网络抓包)

`logging.ts:44-49`：

```typescript
export async function startNetLog() {
  if (netLog.currentlyLogging) return
  netLogPath = join(run, "network.netlog")
  await netLog.startLogging(netLogPath, { captureMode: "default", maxFileSize: NET_LOG_SIZE })
  write("network", "net log started", { path: netLogPath })
}
```

`captureMode: "default"` 是 Chromium 的**精确抓包模式**（捕获所有 HTTP/HTTPS 请求与响应头+body），与 `defaultSensitive` 不同（后者会脱敏 cookies）。**`.netlog` 文件可被 Chrome DevTools `chrome://net-export/` 直接打开可视化分析**。

### 1.4 重启与恢复

`packages/desktop/src/main/index.ts:171-177`：

```typescript
const relaunch = () => {
  setAppQuitting()
  void stopSidecars().finally(() => {
    app.relaunch()
    app.quit()
  })
}
```

**关键设计**：

1. `setAppQuitting()` —— 通知 window registry **持久化 window IDs**（否则 app.quit 会丢状态）
2. `stopSidecars().finally(...)` —— **保证 sidecar 干净退出**（避免 zombie 进程占用 port）
3. `app.relaunch()` —— spawn 新实例
4. `app.quit()` —— 旧实例退出

`index.ts:246-251` 还监听 Unix signal：

```typescript
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    setAppQuitting()
    void stopSidecars().finally(() => app.quit())
  })
}
```

### 1.5 IPC `fatal renderer error` 回填

`packages/desktop/src/main/index.ts:309` 在 IPC 暴露：

```typescript
recordFatalRendererError: (error) => writeLog("renderer", "fatal renderer error", { ...error }, "error"),
```

`packages/desktop/src/renderer/index.tsx` 在 renderer 里 hook 后调用（unhandled exception → IPC → main 落盘）。

### 1.6 Sidecar 健康检查

`packages/desktop/src/main/server.ts:144-163` 启动后 health check 循环：

```typescript
const wait = (async () => {
  const url = `http://${hostname}:${port}`
  let healthy = false
  const gone = exit.promise.then((code) => {
    if (healthy) return
    throw new Error(`Sidecar exited before health check passed with code ${code}`)
  })
  const ready = async () => {
    while (true) {
      await new Promise((resolve) => setTimeout(resolve, 100))
      if (await checkHealth(url, password)) {
        healthy = true
        return
      }
    }
  }
  await Promise.race([ready(), gone])
})()
```

**关键**：100ms 间隔轮询 `/api/health` + `/global/health`（双 endpoint 容错），sidecar 死了直接抛错终止等待。

### 1.7 laew gap L38-L42（CrashDump 相关）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L38** | laew 无 panic hook | 用 `human-panic` crate（claudecode/opencode 都用 Electron Crashpad 但 Rust 等价是 `human-panic`），dump 到 `~/.local/share/laew/crashdumps/` |
| **L39** | 无指数退避 | 学习 opencode `setImmediate(() => process.exit(1))` —— 失败立即退出，不重试；relaunch 由 main 进程统一控制 |
| **L40** | 无熔断器 | 借鉴 opencode 的 sidecar health check 100ms 轮询 + dual endpoint (`/api/health` + `/global/health`)；Rust 用 `tokio::time::interval` |
| **L41** | 无错误分类 | opencode 的 `Effect.catchTag("Session.NotFoundError", ...)` 三段式：错误定义 (`class NotFoundError extends Schema.TaggedErrorClass`) + 错误传递 + 错误映射到 HTTP status，是 Rust `thiserror` + 模式匹配的范式 |
| **L42** | 错误 UX 差 | laew 当前是 `tracing::error!` 一行；opencode 有 `desktop.recovery.loadFailed.detail: "Window: {{window}}\nURL: {{url}}\nError: {{code}} {{description}}"` 的本地化模板 |

---

## 2. WebUI 与 DesktopApp

opencode 的多端 UI 体系是 **TUI / Web / Desktop / App** 四端共享 **Solid.js 组件库 + 多语言系统 + 上层 router**。本节聚焦 `desktop` / `web` / `app` / `ui` / `tui` 五个 package。

### 2.1 Package 拓扑

```
packages/
├── ui/              # 共享 Solid.js 组件库（@opencode-ai/ui）
├── app/             # 桌面/CLI 共享业务逻辑（@opencode-ai/app）
├── desktop/         # Electron 主进程壳（@opencode-ai/desktop）
├── web/             # Astro Starlight 文档站（@opencode-ai/web）
├── enterprise/      # SolidStart SSR share 页面（@opencode-ai/enterprise）
├── tui/             # 终端 UI（@opencode-ai/tui）
├── session-ui/      # Session 共享组件（@opencode-ai/session-ui）
└── sdk/             # HTTP 客户端 SDK
```

**依赖关系**：
- `ui` ← `app` ← `desktop`（renderer）
- `ui` ← `session-ui` ← `enterprise`
- `tui` 直接 Solid.js，独立

### 2.2 Desktop 主进程生命周期（Effect.gen 全栈）

`packages/desktop/src/main/index.ts:115-422` 是 `Effect.gen` 全栈编排器：

```typescript
const main = Effect.gen(function* () {
  contextMenu({ showSaveImageAs: true, showLookUpSelection: false, showSearchWithGoogle: false })
  try {
    process.chdir(homedir())  // macOS apps run in `/` which can cause issues with ripgrep
  } catch {}
  process.env.OPENCODE_DISABLE_EMBEDDED_WEB_UI = "true"
  
  const appId = app.isPackaged ? APP_IDS[CHANNEL] : "ai.opencode.desktop.dev"
  // ... onboarding test root setup ...
  app.setName(app.isPackaged ? APP_NAMES[CHANNEL] : "OpenCode Dev")
  app.setAppUserModelId(appId)
  app.setPath("userData", onboardingTestRoot ? join(onboardingTestRoot, "desktop") : join(app.getPath("appData"), appId))
  if (onboardingTestRoot) app.setPath("sessionData", join(onboardingTestRoot, "session"))
  initializeOldLayoutEligibility(app.getPath("userData"))
  logger = initLogging()
  initCrashReporter()
  
  // WSL 服务器控制器
  const wslServers = createWslServersController(app.getVersion(), async (distro) => {...}, {...})
  const stopSidecars = async () => {
    await killSidecar()
    wslServers.stopAll()
  }
  const relaunch = () => {
    setAppQuitting()
    void stopSidecars().finally(() => { app.relaunch(); app.quit() })
  }
  
  // ... CACert / MDNS / Proxy ...
  
  if (!app.requestSingleInstanceLock()) {
    app.quit()
    return
  }
  
  yield* Effect.promise(() => app.whenReady())  // 等 Electron ready
  
  if (!TEST_ONBOARDING) migrate()
  yield* Effect.promise(() => cleanupStoreFiles(...)).pipe(Effect.tap, Effect.catch)
  app.setAsDefaultProtocolClient("opencode")
  registerRendererProtocol()
  setDockIcon()
  const updater = setupAutoUpdater(stopSidecars)
  registerIpcHandlers({...})
  void updater.start()
  const updateTimer = setInterval(() => void updater.check(), 10 * 60 * 1000)
  updateTimer.unref()
  
  // ... startNetLog + loadingTask (sidecar fork) ...
  
  const windows = restoreMainWindows()
  if (windows.length) createMenu(menuDeps)
})
Effect.runFork(main)
```

**核心要点**：

1. **`requestSingleInstanceLock`** —— 单实例锁，第二次启动走 `second-instance` 事件（deep link 路由）
2. **`process.chdir(homedir())`** —— macOS Electron app 启动时 cwd=`/`，**ripgrep 等工具对此敏感**，先切到 home
3. **`setAsDefaultProtocolClient("opencode")`** —— 注册 `opencode://` deep link
4. **`setInterval(check, 10*60*1000).unref()`** —— 每 10 分钟检查更新，**`unref` 让它不阻塞进程退出**

### 2.3 Renderer：Solid.js + MemoryRouter + Window Registry

`packages/desktop/src/renderer/index.tsx` 是 Solid.js 应用入口：

```typescript
function DesktopMemoryRouter(props: BaseRouterProps & { windowID: string }) {
  const history = createMemoryHistory()
  const initialUrl = getLastActiveUrl(props.windowID)
  if (initialUrl !== "/") history.set({ value: initialUrl, replace: true, scroll: false })
  onCleanup(history.listen((value) => setLastActiveUrl(props.windowID, value)))
  return <MemoryRouter {...props} history={history} />
}
```

**每个 window 有独立的 memoryHistory** + **持久化最近一次访问的 URL 到 localStorage**，实现「上次访问页面恢复」。

`createPlatform` 创建 `Platform` 对象注入 Provider：

```typescript
const createPlatform = (windowState: DesktopWindowState): Platform => {
  const attachmentPaths = new WeakMap<File, string>()
  const os = (() => {
    const ua = navigator.userAgent
    if (ua.includes("Mac")) return "macos"
    if (ua.includes("Windows")) return "windows"
    if (ua.includes("Linux")) return "linux"
    return undefined
  })()
  // ...
}
```

`@opencode-ai/app/context/platform.tsx` 定义 `Platform` interface，desktop / web 各自 `createPlatform`，**业务组件不感知平台**。

### 2.4 Sentry 集成（仅 prod）

`packages/desktop/src/renderer/index.tsx:40-61`：

```typescript
if (import.meta.env.VITE_SENTRY_DSN) {
  Sentry.init({
    dsn: import.meta.env.VITE_SENTRY_DSN,
    environment: import.meta.env.VITE_SENTRY_ENVIRONMENT ?? import.meta.env.MODE,
    release: import.meta.env.VITE_SENTRY_RELEASE ?? `desktop@${pkg.version}`,
    initialScope: { tags: { platform: "desktop" } },
    integrations: (integrations) => {
      return integrations.filter(
        (i) =>
          i.name !== "Breadcrumbs" &&  // 关 Breadcrumbs 减少噪音
          !(import.meta.env.OPENCODE_CHANNEL === "prod" &&
            (i.name === "GlobalHandlers" || i.name === "BrowserApiErrors")),  // prod 关 auto handler，自己 hook
    )
  })
}
```

**关键设计**：

1. **`VITE_SENTRY_DSN` 才初始化** —— CI/test 默认关 Sentry
2. **`!prod` 保留 GlobalHandlers/BrowserApiErrors** —— dev/beta 自动捕获（更方便调试）
3. **prod 关掉自动 handler** —— 自己手动上报，避免 Sentry 自动上报太多无关事件

### 2.5 Web 端（Astro Starlight）

`packages/web/astro.config.mjs:1-100`：

```javascript
export default defineConfig({
  site: config.url,
  base: "/docs",
  output: "server",
  adapter: cloudflare({ imageService: "passthrough" }),
  // ...
  integrations: [
    configSchema(),
    solidJs(),
    starlight({
      title: "OpenCode",
      defaultLocale: "root",
      locales: {
        root: { label: "English", lang: "en", dir: "ltr" },
        ar: { label: "العربية", lang: "ar", dir: "rtl" },
        bs: { label: "Bosanski", lang: "bs-BA", dir: "ltr" },
        // ... 20 locales
      },
    }),
  ],
})
```

**`/docs` 路径 + Cloudflare adapter + 20 locales + RTL 支持**。

`packages/web/src/middleware.ts` 是 i18n 中间件：

```typescript
function docsAlias(pathname: string) {
  const hit = /^\/docs\/([^/]+)(\/.*)?$/.exec(pathname)
  if (!hit) return null
  const value = hit[1] ?? ""
  const tail = hit[2] ?? ""
  const locale = exactLocale(value)
  if (!locale) return null
  const next = locale === "root" ? `/docs${tail}` : `/docs/${locale}${tail}`
  if (next === pathname) return null
  return { path: next, locale }
}

export const onRequest = defineMiddleware((ctx, next) => {
  const alias = docsAlias(ctx.url.pathname)
  if (alias) return redirect(ctx.url, alias.path, alias.locale)

  if (ctx.url.pathname !== "/docs" && ctx.url.pathname !== "/docs/") return next()

  const locale =
    localeFromCookie(ctx.request.headers.get("cookie")) ??
    localeFromAcceptLanguage(ctx.request.headers.get("accept-language"))
  if (!locale || locale === "root") return next()

  return redirect(ctx.url, `/docs/${locale}/`)
})
```

**核心**：访问 `/docs` 时按 cookie → Accept-Language 优先级自动重定向到对应 locale 路径。**`oc_locale` cookie 1 年有效期**。

### 2.6 Enterprise (SolidStart SSR Share 页面)

`packages/enterprise/src/app.tsx`：

```typescript
function detectLocaleFromHeader(header: string | null | undefined) {
  if (!header) return
  for (const item of header.split(",")) {
    const value = item.trim().split(";")[0]?.toLowerCase()
    if (!value) continue
    if (value.startsWith("zh")) return "zh" as const
    if (value.startsWith("en")) return "en" as const
  }
}

function detectLocale() {
  const event = getRequestEvent()
  const header = event?.request.headers.get("accept-language")
  const headerLocale = detectLocaleFromHeader(header)
  if (headerLocale) return headerLocale
  if (typeof document === "object") {
    const value = document.documentElement.lang?.toLowerCase() ?? ""
    if (value.startsWith("zh")) return "zh" as const
    if (value.startsWith("en")) return "en" as const
  }
  if (typeof navigator === "object") {
    for (const language of navigator.languages ?? []) {
      if (language.toLowerCase().startsWith("zh")) return "zh" as const
    }
  }
  return "en" as const
}

function UiI18nBridge(props: ParentProps) {
  const locale = createMemo(() => detectLocale())
  const t = (key, params) => {
    const value = locale() === "zh" ? zh[key] ?? uiEn[key] : uiEn[key]
    const text = value ?? String(key)
    return resolveTemplate(text, params)
  }
  // ...
  return <I18nProvider value={{ locale, t, plural }}>{props.children}</I18nProvider>
}
```

**Enterprise 只支持 zh/en**（与 web 文档站的 20 locales 不同，**业务组件用 Shared UI 库，UI 库自带完整 i18n**）。

`packages/enterprise/src/routes/share/[shareID].tsx:58-120` 是 share 页面数据组装：

```typescript
const getData = query(async (shareID) => {
  "use server"
  const share = await Share.get(shareID)
  if (!share) throw new SessionDataMissingError({ sessionID: shareID })
  const data = await Share.data(shareID)
  const result = {
    sessionID: share.sessionID,
    shareID,
    session: [],
    session_diff: { [share.sessionID]: [] },
    session_status: { [share.sessionID]: { type: "idle" } },
    message: {},
    part: {},
    model: {},
  }
  for (const item of data) {
    switch (item.type) {
      case "session": result.session.push(item.data); break
      case "session_diff": result.session_diff[share.sessionID] = item.data; break
      case "message": result.message[item.data.sessionID] ??= []; result.message[item.data.sessionID].push(item.data); break
      case "part": result.part[item.data.messageID] ??= []; result.part[item.data.messageID].push(item.data); break
      case "model": result.model[share.sessionID] = item.data; break
    }
  }
  // ...
})
```

**`"use server"` 标记让 SolidStart 把这个函数编到 server bundle**，调用时通过 RPC 跳到 server 端。**Cloudflare Workers 上运行**（compatibilityDate: 2024-09-19 + nodeCompat: true）。

### 2.7 Window State（window-state.ts）

`packages/desktop/src/main/windows.ts:35-45` 用 `electron-window-state`：

```typescript
import windowState from "electron-window-state"

protocol.registerSchemesAsPrivileged([
  {
    scheme: rendererProtocol,
    privileges: {
      secure: true,
      standard: true,
      supportFetchAPI: true,
      stream: true,
    },
  },
])

const titlebarHeight = 40
const maxZoomLevel = 10
const minZoomLevel = 0.2
```

**`oc://renderer` 自定义协议**（取代 `file://`），**`supportFetchAPI` + `stream` 让 renderer 可以 fetch 本地资源（SSR 流式渲染 / 字体 / 图片）**。

### 2.8 Menu 系统

`packages/desktop/src/main/index.ts:275-282`：

```typescript
const menuDeps = {
  trigger: (id: string) => {
    const win = getLastFocusedWindow()
    if (win) sendMenuCommand(win, id)
  },
  checkForUpdates: () => void showUpdaterDialog(updater, true),
  relaunch,
}
```

**菜单命令通过 IPC 推送给当前 focused window**，menu 不直接操作业务。

### 2.9 WSL 子进程（WSL 服务器控制器）

`packages/desktop/src/main/wsl/servers.ts:62-99` 是 WSL sidecar 管理：

```typescript
export function createWslServersController(
  appVersion: string,
  spawnSidecar: SpawnSidecar,
  options?: WslServersControllerOptions,
) {
  let state: WslServersState = initialState()
  const listeners = new Set<(event: WslServersEvent) => void>()
  const sidecars = new Map<string, RunningSidecar>()
  const startAttempts = new Map<string, number>()
  let jobAbort: AbortController | undefined
  const logger = options?.logger
  // ...

  const emit = () => {
    for (const listener of listeners) listener({ type: "state", state })
  }

  const setState = (next: Partial<WslServersState>) => {
    state = { ...state, ...next }
    emit()
  }
  // ...
}
```

**WSL = 每个 WSL distro 一个 sidecar 进程**，由 `wsl:<distro>` 作为 ID。Job 系统通过 `AbortController` 共享，支持「取消当前 job 后启动新 job」。

### 2.10 TUI Package（独立渲染）

`packages/tui/src/index.tsx` 是 TUI 入口，与 desktop 的 Solid.js web 渲染**完全独立**（不共享组件）。**TUI 是 Effect.gen + Solid.js（@opentui/solid-js）**，**底层用 ANSI 转义码**。

### 2.11 laew gap L44-L48（多端 UI）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L44** | 无 web 远控 | laew 当前是 TUI 单进程；opencode 的 TUI 实际是「TUI 客户端 + HTTP server」，可独立 web 控制 |
| **L45** | 无 desktop 壳 | laew 纯 TUI；opencode 的 electron-builder + sidecar 嵌入是参考架构 |
| **L46** | 无 WASM | opencode 的 enterprise 是 SolidStart SSR + Cloudflare Worker；laew 上 wasm 可做 wasm-pack |
| **L47** | 无多端 Session | opencode 的 `WindowRegistry` + `DesktopMemoryRouter` 是范式 |
| **L48** | 测试栈薄 | opencode 每个 package 都带 `*.test.ts`；laew e2e 集中在 `testReport/` |

---

## 3. OAuth 认证与多账号

opencode 的认证体系核心在 `packages/opencode/src/auth/index.ts`，**基于 Effect DI + Schema + 文件持久化**，**支持 OAuth/Api/WellKnown 三种类型**。

### 3.1 Auth 数据模型

`packages/opencode/src/auth/index.ts:14-41`：

```typescript
export const OAUTH_DUMMY_KEY = "opencode-oauth-dummy-key"

export class Oauth extends Schema.Class<Oauth>("OAuth")({
  type: Schema.Literal("oauth"),
  refresh: Schema.String,
  access: Schema.String,
  expires: NonNegativeInt,
  accountId: Schema.optional(Schema.String),
  enterpriseUrl: Schema.optional(Schema.String),
}) {}

export class Api extends Schema.Class<Api>("ApiAuth")({
  type: Schema.Literal("api"),
  key: Schema.String,
  metadata: Schema.optional(Schema.Record(Schema.String, Schema.String)),
}) {}

export class WellKnown extends Schema.Class<WellKnown>("WellKnownAuth")({
  type: Schema.Literal("wellknown"),
  key: Schema.String,
  token: Schema.String,
}) {}

export const Info = Schema.Union([Oauth, Api, WellKnown]).annotate({ discriminator: "type", identifier: "Auth" })
export type Info = Schema.Schema.Type<typeof Info>

export class AuthError extends Schema.TaggedErrorClass<AuthError>()("AuthError", {
  message: Schema.String,
  cause: Schema.optional(Schema.Defect()),
}) {}
```

**三种认证类型**：

1. **OAuth**：access/refresh/expires 三元组 + accountId（多账号区分）+ enterpriseUrl（Anthropic enterprise）
2. **API Key** + 可选 metadata（provider-specific 配置）
3. **WellKnown**：自动发现（如 GitHub App 通过 `.well-known` 获取 token）

`OAUTH_DUMMY_KEY = "opencode-oauth-dummy-key"` —— 当 provider 是 OAuth 但还没真实 token 时，**用 dummy key 触发 OAuth 流程**（不是真的 key，是标记）。

### 3.2 持久化层

`packages/opencode/src/auth/index.ts:52-93`：

```typescript
const layer = Layer.effect(
  Service,
  Effect.gen(function* () {
    const fsys = yield* FSUtil.Service
    const decode = Schema.decodeUnknownOption(Info)

    const all = Effect.fn("Auth.all")(function* () {
      if (process.env.OPENCODE_AUTH_CONTENT) {
        try {
          return JSON.parse(process.env.OPENCODE_AUTH_CONTENT)
        } catch (err) {}
      }
      const data = (yield* fsys.readJson(file).pipe(Effect.orElseSucceed(() => ({})))) as Record<string, unknown>
      return Record.filterMap(data, (value) => Result.fromOption(decode(value), () => undefined))
    })

    const get = Effect.fn("Auth.get")(function* (providerID: string) {
      return (yield* all())[providerID]
    })

    const set = Effect.fn("Auth.set")(function* (key: string, info: Info) {
      const norm = key.replace(/\/+$/, "")  // 去尾部 /
      const data = yield* all()
      if (norm !== key) delete data[key]
      delete data[norm + "/"]
      yield* fsys
        .writeJson(file, { ...data, [norm]: info }, 0o600)  // 0o600 仅 owner 可读写
        .pipe(Effect.mapError(fail("Failed to write auth data")))
    })

    const remove = Effect.fn("Auth.remove")(function* (key: string) {
      const norm = key.replace(/\/+$/, "")
      const data = yield* all()
      delete data[key]
      delete data[norm]
      yield* fsys.writeJson(file, data, 0o600).pipe(Effect.mapError(fail("Failed to write auth data")))
    })

    return Service.of({ get, all, set, remove })
  }),
)

export const node = LayerNode.make({ service: Service, layer: layer, deps: [FSUtil.node] })
```

**关键设计**：

1. **`OPENCODE_AUTH_CONTENT` 环境变量注入** —— CI/test 场景用 env 注入 fake auth，避免污染磁盘
2. **Schema 解码 + `Record.filterMap`** —— 用 Effect 的 `Result.fromOption` 过滤无效项，**损坏的 auth 不会 crash**
3. **尾部 `/` 归一化** —— 防止 `providerID = "anthropic"` 和 `"anthropic/"` 存两份
4. **`0o600` 文件权限** —— 仅 owner 可读写（防止多用户系统泄露）
5. **`FSUtil.node` LayerNode 注入** —— 与 FSUtil 共享底层实现

### 3.3 多账号场景

`Auth` 数据结构是 `Record<string, Info>` —— **key 是 providerID**，**value 是 Auth.Info**。

```typescript
type AuthMap = Record<string, Auth.Info>
```

**多账号支持**：

- `accountId: Schema.optional(Schema.String)` 在 `Oauth` schema 中
- provider 可以有多条记录（同一 provider 不同 accountId 视为不同账号）

### 3.4 OAuth 刷新流程

刷新逻辑不在 `auth/index.ts`，由调用方（如 `Provider`）驱动：

```typescript
// 伪代码（基于 opencode 实际架构）
const refresh = Effect.fn("Auth.refresh")(function* (providerID: string) {
  const info = yield* auth.get(providerID)
  if (info?.type !== "oauth") return
  if (info.expires > Date.now()) return  // 未过期
  const newInfo = yield* oauthClient.refresh(info.refresh)
  yield* auth.set(providerID, { ...info, access: newInfo.access, expires: newInfo.expires })
})
```

**Anthropic OAuth enterprise URL**：Anthropic 提供 enterprise 自定义 OAuth endpoint，`enterpriseUrl` 字段用于支持 enterprise 部署。

### 3.5 Desktop Multi-Account via `desktop-menu`

`packages/desktop/src/main/desktop-menu-actions.ts` 中可能有「Account → Switch Account」菜单项，**通过 IPC 推给 renderer**，renderer 弹 Modal 选 accountId。

### 3.6 laew gap L49-L52（OAuth 相关）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L49** | API Key 明文存 SQLite | opencode 0o600 + OAuth refresh；laew 缺 OAuth refresh 实现，需加 token rotation |
| **L50** | 无 OAuth 流程 | opencode 的 `OAUTH_DUMMY_KEY` + `accountId` schema 字段是 Rust 实现参考 |
| **L51** | 脱敏范围不足 | laew 现在只对 Provider Form Tab 5 脱敏；opencode `mask_key` 在 `theme.rs` 全局脱敏 |
| **L52** | 无多账号轮换 | laew 单 active provider；opencode `Record<string, Info>` 支持多账号，**轮换策略（如 rate limit 切换）是 laew 需要新增的能力** |

---

## 4. i18n 国际化

opencode 的 i18n 体系是**三层**：**Astro 文档站（20 locales）/ Web UI（业务组件，完整 i18n）/ Desktop Native（30+ locales，仅原生菜单 + 对话框）**。本节聚焦后两层。

### 4.1 三层 i18n 拓扑

```
Layer 1: Astro Starlight 文档站        # /docs/{ar|bs|da|de|es|fr|it|ja|ko|nb|pl|pt-br|ru|th|tr|zh-CN|zh-TW}
Layer 2: App UI 业务组件 (Solid.js)    # 60+ locales, RTL 支持, plural categories
Layer 3: Desktop Native (Electron)     # 60+ locales, 仅原生菜单/对话框/updater/WSL 错误
```

**Layer 1**：通过 `astro.config.mjs` 的 `locales` 配置 + `middleware.ts` 重定向。

**Layer 2 + 3**：通过 `packages/app/src/i18n/desktop-native.ts` + `packages/app/src/context/language.tsx` 协同。

### 4.2 Desktop Native Bundle（核心抽象）

`packages/app/src/i18n/desktop-native.ts:1-65` 定义 60+ locales：

```typescript
export const DESKTOP_NATIVE_LOCALES = [
  "en", "zh", "zht", "ko", "de", "es", "fr", "da", "ja", "pl",
  "ru", "uk", "bs", "ar", "no", "br", "th", "tr", "hi", "nl",
  "id", "vi", "it", "ur", "pa", "az", "fi", "sv", "am", "bg",
  "bn", "ca", "cs", "dv", "dz", "el", "et", "fa", "fo", "hr",
  "hu", "hy", "is", "ka", "km", "lo", "lt", "lv", "mk", "mn",
  "ms", "my", "ne", "ro", "si", "sk", "sl", "sq", "sr", "tg",
  "tk", "uz",
] as const

export type DesktopNativeLocale = (typeof DESKTOP_NATIVE_LOCALES)[number]
```

`desktop-native.ts:198-210` 是 locale 检测算法：

```typescript
export function detectDesktopNativeLocale(languages: readonly string[]): DesktopNativeLocale {
  for (const language of languages) {
    const source = locale(language)
    if (!source) continue
    if (["no", "nb", "nn"].includes(source.language)) return "no"
    const match = DESKTOP_NATIVE_LOCALES.find((candidate) => {
      const target = locale(DESKTOP_NATIVE_LOCALE_TAGS[candidate])
      return target?.language === source.language && target.script === source.script
    })
    if (match) return match
  }
  return "en"
}
```

**关键算法**：

1. 用 `Intl.Locale.maximize()` 把 `zh-TW` 标准化为 `{language: "zh", script: "Hans"}`
2. **`["no", "nb", "nn"].includes(source.language)`** —— Norwegian 三方言（bokmål/nynorsk）合并为 `"no"`
3. **language + script 双重匹配** —— 区分 `zh-Hans`（简体）vs `zh-Hant`（繁體），**这正是 `zht` 单独存在的原因**

`desktop-native.ts:212-214` plural categories：

```typescript
export function desktopNativePluralCategories(locale: DesktopNativeLocale) {
  return new Intl.PluralRules(DESKTOP_NATIVE_LOCALE_TAGS[locale]).resolvedOptions().pluralCategories
}
```

### 4.3 Native Bundle 序列化（IPC 传递）

`desktop-native.ts:329-358`：

```typescript
export const DESKTOP_NATIVE_MAX_PAYLOAD_BYTES = 64 * 1024  // 64KB 上限

export function parseDesktopNativeBundle(value: unknown): DesktopNativeBundle | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined
  try {
    if (new TextEncoder().encode(JSON.stringify(value)).byteLength > DESKTOP_NATIVE_MAX_PAYLOAD_BYTES) return undefined
  } catch {
    return undefined
  }
  const bundle = value as { locale?: unknown; messages?: unknown }
  if (!DESKTOP_NATIVE_LOCALES.some((locale) => locale === bundle.locale)) return undefined
  if (!bundle.messages || typeof bundle.messages !== "object" || Array.isArray(bundle.messages)) return undefined
  const messages = bundle.messages as Record<string, unknown>
  const keys = Object.keys(messages)
  if (keys.length !== DESKTOP_NATIVE_KEYS.length) return undefined
  if (!DESKTOP_NATIVE_KEYS.every((key) => typeof messages[key] === "string")) return undefined
  if (!keys.every((key) => key in DESKTOP_NATIVE_ENGLISH)) return undefined
  return bundle as DesktopNativeBundle
}
```

**关键校验**：

1. **64KB 上限** —— IPC 消息 size 限制
2. **必须所有 key 都存在**（`!DESKTOP_NATIVE_KEYS.every`） —— 防止 IPC 半截消息
3. **所有 message 必须是 string** —— 类型校验

`packages/desktop/src/main/native-translations.ts:11-19`：

```typescript
export function setNativeTranslations(next: DesktopNativeBundle) {
  if (
    next.locale === bundle.locale &&
    DESKTOP_NATIVE_KEYS.every((key) => next.messages[key] === bundle.messages[key])
  ) {
    return false  // 没变化就跳过
  }
  bundle = next
  return true
}
```

### 4.4 Renderer → Main IPC 传递

`packages/desktop/src/main/index.ts:310-312`：

```typescript
setNativeTranslations: (bundle) => {
  if (setNativeTranslations(bundle)) createMenu(menuDeps)
},
```

**renderer 检测到语言变化时，重新生成 bundle 推到 main，main 重建原生菜单**。

### 4.5 App UI 业务组件 i18n

`packages/app/src/context/language.tsx:52-114` 是加载器：

```typescript
const loaders: Record<Exclude<Locale, "en">, () => Promise<Dictionary>> = {
  zh: () => merge(import("@/i18n/zh"), import("@opencode-ai/ui/i18n/zh")),
  zht: () => merge(import("@/i18n/zht"), import("@opencode-ai/ui/i18n/zht")),
  ko: () => merge(import("@/i18n/ko"), import("@opencode-ai/ui/i18n/ko")),
  de: () => merge(import("@/i18n/de"), import("@opencode-ai/ui/i18n/de")),
  // ... 60+ loaders
}
```

**动态 import + 缓存**：

```typescript
function loadDict(locale: Locale) {
  const hit = dicts.get(locale)
  if (hit) return Promise.resolve(hit)
  if (locale === "en") return Promise.resolve(base)
  const load = loaders[locale]
  return load().then((next: Dictionary) => {
    dicts.set(locale, next)
    return next
  })
}
```

`merge(appDict, uiDict)` —— 把 app 域 + UI 共享库 域字典合并：

```typescript
const merge = (app: Promise<Source>, ui: Promise<Source>) =>
  Promise.all([app, ui]).then(([a, b]) => ({ ...base, ...i18n.flatten({ ...a.dict, ...b.dict }) }) as Dictionary)
```

**`@solid-primitives/i18n` 的 `i18n.flatten`** —— 把 `{session.list.title: "..."}` 嵌套结构拍平为 `session.list.title: "..."` 用于快速查找。

### 4.6 RTL 支持

`language.tsx:23-27`：

```typescript
const RTL_LOCALES: ReadonlySet<Locale> = new Set(["ar", "ur", "pa", "fa", "dv"])

function localeDirection(locale: Locale): Direction {
  return RTL_LOCALES.has(locale) ? "rtl" : "ltr"
}
```

**5 个 RTL locale**：Arabic/Urdu/Punjabi/Persian/Divehi。

`language.tsx:181-187`：

```typescript
const direction = createMemo(() => layout.direction ?? localeDirection(locale()))
const layoutLocale = createMemo(() => {
  if (!layout.direction) return intl()
  // Kobalte derives menu direction from locale rather than accepting a direction override.
  return layout.direction === "rtl" ? "ar" : "en"
})
```

**关键技巧**：Kobalte menu 组件不接受 direction override，**只能用 locale 推断**，所以 RTL 时把 layout locale 强制设为 `ar`（Kobalte 看到 `ar` 就用 RTL 渲染）。

`language.tsx:207-213` 应用到 DOM：

```typescript
createEffect(() => {
  if (typeof document !== "object") return
  const value = locale()
  document.documentElement.lang = intl()
  document.documentElement.dir = direction()
  document.cookie = cookie(value)
})
```

**`<html lang="..." dir="...">` 配合 cookie**，让 CSS `dir="rtl"` selector 工作。

### 4.7 Plural Support

`language.tsx:197-203`：

```typescript
const plural = (key: PluralKey, count: number, params?: Record<string, string | number | boolean>) => {
  const category = pluralCategory(intl(), count)
  const current = (dict.loading ? base : (dict() ?? base)) as Record<string, string>
  const candidate = `${key}.${category}`   // e.g. "session.followupDock.summary.one"
  const fallback = `${key}.other`
  return i18n.resolveTemplate(current[candidate] ?? current[fallback] ?? fallback, { ...params, count })
}
```

**`Intl.PluralRules` 提供 `zero`/`one`/`two`/`few`/`many`/`other` 6 个 CLDR categories**。

`packages/ui/src/context/i18n.ts` 提供 `pluralCategory` 和 `pluralKey`：

```typescript
export function pluralCategory(locale: Locale, count: number) {
  const pr = new Intl.PluralRules(locale).select(count)
  return pr  // "zero" | "one" | "two" | "few" | "many" | "other"
}
```

### 4.8 语言切换持久化

`language.tsx:171-176`：

```typescript
const [store, setStore, _, ready] = persisted(
  Persist.global("language", ["language.v1"]),
  createStore({
    locale: initial,
  }),
)
```

**`Persist.global("language", ["language.v1"])`** —— **版本化持久化**，未来 schema 变化时通过 `["v1"]` migration。

### 4.9 关键 Bundle 边界（IPC 推送）

`language.tsx:215-222`：

```typescript
createEffect(() => {
  if (!props.onNativeTranslations || dict.loading) return
  const current = dict()
  if (!current) return
  props.onNativeTranslations(
    createDesktopNativeBundle(locale(), (key) => current[key] ?? DESKTOP_NATIVE_ENGLISH[key]),
  )
})
```

**`?? DESKTOP_NATIVE_ENGLISH[key]`** —— 兜底英文，**保证 IPC bundle 永远所有 key 都有值**。

### 4.10 laew gap L54-L58（i18n 相关）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L54** | TUI 中文化硬编码 | opencode 用 `@solid-primitives/i18n` + 60+ locale；laew 引入 `rust_i18n` crate + JSON/YAML |
| **L55** | 文档无双语 | opencode 文档站 20 locales；laew 文档加 `docs/{en,zh}/` |
| **L56** | 错误无 i18n | opencode `desktop.recovery.loadFailed.detail` 是本地化模板；laew `AgentError` 应加 `i18n_key` 字段 |
| **L57** | 无 RTL | opencode 5 RTL locale + `<html dir>` + Kobalte；laew TUI 无此需求但 web 化需要 |
| **L58** | 无翻译 pipeline | opencode 用 `@solid-primitives/i18n.flatten` 自动嵌套拍平；laew 当前无 i18n |

---

## 5. Release 工程化与 AutoUpdate

opencode 的 Release 工程化体系涵盖 **electron-builder + updater state machine + AutoUpdater + CrashReporter + 多 channel（dev/beta/prod）**。

### 5.1 Channel 系统（3 channel）

`packages/desktop/src/main/index.ts:53-62`：

```typescript
const APP_NAMES: Record<string, string> = {
  dev: "OpenCode Dev",
  beta: "OpenCode Beta",
  prod: "OpenCode",
}
const APP_IDS: Record<string, string> = {
  dev: "ai.opencode.desktop.dev",
  beta: "ai.opencode.desktop.beta",
  prod: "ai.opencode.desktop",
}
```

**3 channel 隔离**：dev/beta/prod 用不同 `appId`，**window 状态、userData、注册表项、deep link 全隔离**。

`electron-builder.config.ts` 配置 channel-specific 产物名：

```typescript
// 简化示意
{
  appId: "ai.opencode.desktop",  // prod
  productName: "OpenCode",
  // beta: appId + ".beta" → "ai.opencode.desktop.beta"
  // dev:   appId + ".dev"   → "ai.opencode.desktop.dev"
}
```

### 5.2 AutoUpdater 状态机

`packages/desktop/src/main/updater-controller.ts:19-95` 是 8 态状态机：

```typescript
export type UpdaterState =
  | { status: "disabled" }
  | { status: "idle" }
  | { status: "checking" }
  | { status: "downloading"; version: string; percent?: number }
  | { status: "ready"; version: string }
  | { status: "up-to-date" }
  | { status: "installing"; version: string }
  | { status: "error"; message: string }
```

**8 态转换图**：

```
                    disabled
                       ↑
                       │ (when UPDATER_ENABLED=false)
                       │
idle ──check()──> checking ──available──> downloading ──done──> ready ──install()──> installing ──> ready
 │                  │                                                  │
 │                  └──error──> error                                 │
 │                                                                    │
 └──check()  ─────> up-to-date ───────────────────────────────────┘
```

`updater-controller.ts:38-64`：

```typescript
const check = () => {
  if (!input.enabled) return Promise.resolve(state)
  if (state.status === "ready") return Promise.resolve(state)  // 已就绪就不检查
  if (pending) return pending  // 并发抑制

  pending = (async () => {
    transition({ status: "checking" })
    const result = await input.backend.checkForUpdates()
    const version = result?.updateInfo?.version
    if (!result?.isUpdateAvailable || !version || version === input.currentVersion) {
      await input.persistence.clear()
      return transition({ status: "up-to-date" })
    }
    transition({ status: "downloading", version })
    await input.backend.downloadUpdate()
    await input.persistence.set({ version })
    return transition({ status: "ready", version })
  })()
    .catch((error) =>
      transition({ status: "error", message: error instanceof Error ? error.message : String(error) }),
    )
    .finally(() => {
      pending = undefined
    })
  return pending
}
```

**关键设计**：

1. **三段式状态保护**：`!enabled` / `=== "ready"` / `pending` —— 避免无谓请求
2. **持久化** —— 用 electron-store 保存 `{ version }`，下次启动 `start()` 时校验是否与当前版本匹配
3. **`transition()` 返回新 state** —— 让 chain 可以传递

`updater-controller.ts:73-93`：

```typescript
async start() {
  const ready = await input.persistence.get()
  if (ready?.version === input.currentVersion) await input.persistence.clear()  // 当前版本已就绪则清掉
  return check()
},
async install() {
  if (state.status !== "ready") throw new Error("Update is not ready to install")
  const version = state.version
  transition({ status: "installing", version })
  await input
    .stop()  // 先停 sidecar
    .then(() => {
      input.backend.quitAndInstall()  // 再装
      transition({ status: "ready", version })  // 装完回到 ready（理论上进程已死）
    })
    .catch((error) => {
      transition({ status: "ready", version })
      throw error
    })
},
```

**`install()` 的 4 步**：

1. **校验 ready** —— 否则抛错
2. **transition to installing** —— UI 显示「正在安装」
3. **`input.stop()`** —— **优雅停止 sidecar**（这是关键，**避免装到一半 sidecar 还在写文件**）
4. **`quitAndInstall()`** —— Electron 内部清理 windows + 替换二进制

### 5.3 AutoUpdater 配置

`packages/desktop/src/main/updater.ts:13-26`：

```typescript
export function setupAutoUpdater(stop: () => Promise<void>) {
  autoUpdater.logger = logger
  autoUpdater.channel = "latest"
  autoUpdater.allowPrerelease = false      // 不自动装 beta
  autoUpdater.allowDowngrade = true        // 允许回滚（重大 bug 时救命）
  autoUpdater.autoDownload = false         // 不自动下载（用户确认）
  autoUpdater.autoInstallOnAppQuit = false // 不自动装
  // ...
}
```

**默认 4 个 false**：

- `allowPrerelease = false`：beta 用户也不会自动装到 latest
- `autoDownload = false`：**必须用户点「立即更新」才下载**
- `autoInstallOnAppQuit = false`：**退出时不自动装**（避免下班前自动装被同事遇到新 bug）

`allowDowngrade = true` —— **关键**：当新版有严重 bug 时，用户可以手动装旧版。

### 5.4 定时检查更新

`packages/desktop/src/main/index.ts:316-318`：

```typescript
const updateTimer = setInterval(() => void updater.check(), 10 * 60 * 1000)  // 10 分钟一次
updateTimer.unref()  // 不阻塞进程退出
app.once("will-quit", () => clearInterval(updateTimer))
```

**10 分钟一次 + `unref()`** —— 不阻塞退出（与 Node.js 进程退出语义一致）。

### 5.5 Electron-builder 配置

`packages/desktop/electron-builder.config.ts`：

```typescript
{
  // 三 channel 都用同一份 config，只改 channel-specific 字段
  mac: { target: "dmg", category: "public.app-category.developer-tools" },
  win: { target: "nsis" },
  linux: { target: ["AppImage", "deb"], category: "Development" },
  publish: [
    {
      provider: "generic",
      url: "https://releases.opencode.ai/desktop",  // 自托管 update server
      channel: "latest",
    },
  ],
}
```

### 5.6 CI Containers（详见第 7 节）

`packages/containers/` 提供 GitHub Actions 用的预构建镜像（5 个 layer：base → bun-node → rust → tauri-linux → publish）。

### 5.7 laew gap L59-L63（Release 相关）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L59** | 无 CI | opencode 5 个 docker layer + GitHub Actions；laew 用 `cargo test` + GitHub Actions |
| **L60** | 手动 release | opencode 用 electron-builder + autoUpdater；laew 缺 release 流程 |
| **L61** | 无 Auto Update | 借鉴 5.1-5.4；Rust 用 `self_update` crate |
| **L62** | 无签名 | macOS 用 codesign + notarize；Windows 用 signtool；laew 应配置签名 |
| **L63** | 无分发 | opencode 自托管 `releases.opencode.ai`；laew 缺分发平台（crates.io / brew / scoop） |

---

## 6. WebSocket 与 SSE

opencode 的实时通信体系有 **三套协议**：**SSE（事件流）/ WebSocket（OpenAI Responses）/ Effect Stream（内部 Effect 流水线）**。

### 6.1 SSE 协议（核心事件流）

`packages/opencode/src/server/routes/instance/httpapi/handlers/event.ts`：

```typescript
function eventData(data: unknown): Sse.Event {
  return { _tag: "Event", event: "message", id: undefined, data: JSON.stringify(data) }
}

export const eventHandlers = HttpApiBuilder.group(EventApi, "event", (handlers) =>
  Effect.gen(function* () {
    const events = yield* EventV2Bridge.Service
    return handlers.handleRaw(
      "subscribe",
      Effect.fn("EventHttpApi.subscribe")(function* () {
        return yield* eventResponse(events)
      }),
    )
  }),
)

function eventResponse(events: EventV2.Interface) {
  return Effect.gen(function* () {
    const instance = yield* InstanceState.context
    const workspaceID = yield* InstanceState.workspaceID
    // Listener registration is eager, so events published after this point cannot
    // be lost while the HTTP body fiber is starting or emitting server.connected.
    const queue = yield* Queue.unbounded<EventV2.Payload>()
    const unsubscribe = yield* events.listen((event) => Effect.sync(() => Queue.offerUnsafe(queue, event)))
    yield* Effect.addFinalizer(() => unsubscribe)
    const stream = Stream.fromQueue(queue).pipe(
      Stream.filter(
        (event) =>
          event.location?.directory === instance.directory &&
          (event.location.workspaceID === undefined || event.location.workspaceID === workspaceID),
      ),
      Stream.map((event) => ({ id: event.id, type: event.type, properties: event.data })),
    )
    const disposed = Stream.callback<{ id: string; type: string; properties: unknown }>((queue) => {
      const listener = (event) => {
        if (event.directory !== instance.directory || event.payload.type !== "server.instance.disposed") return
        Queue.offerUnsafe(queue, { id: event.payload.id ?? eventID(), type: "server.instance.disposed", properties: event.payload.properties ?? {} })
      }
      return Effect.acquireRelease(
        Effect.sync(() => GlobalBus.on("event", listener)),
        () => Effect.sync(() => GlobalBus.off("event", listener)),
      )
    })
    const output = stream.pipe(
      Stream.merge(disposed, { haltStrategy: "left" }),
      Stream.takeUntil((event) => event.type === "server.instance.disposed"),
    )
    const heartbeat = Stream.tick("10 seconds").pipe(Stream.drop(1), Stream.map(() => ({ id: eventID(), type: "server.heartbeat", properties: {} })))
    yield* Effect.logInfo("event connected")
    return HttpServerResponse.stream(
      Stream.make({ id: eventID(), type: "server.connected", properties: {} }).pipe(
        Stream.concat(output.pipe(Stream.merge(heartbeat, { haltStrategy: "left" }))),
        Stream.map(eventData),
        Stream.pipeThroughChannel(Sse.encode()),
        Stream.encodeText,
        Stream.ensuring(Effect.logInfo("event disconnected")),
      ),
      {
        contentType: "text/event-stream",
        headers: {
          "Cache-Control": "no-cache, no-transform",
          "X-Accel-Buffering": "no",
          "X-Content-Type-Options": "nosniff",
        },
      },
    )
  })
}
```

**5 个核心 SSE 机制**：

1. **Queue-based event delivery** —— `Queue.unbounded<EventV2.Payload>()` 是 Effect 的 MPSC 队列，**避免事件丢失**（Eager listener registration 是关键注释）
2. **目录 + workspace 过滤** —— `Stream.filter(event.location?.directory === instance.directory ...)`
3. **`Stream.takeUntil("server.instance.disposed")`** —— instance 销毁时自动断流
4. **Heartbeat** —— `Stream.tick("10 seconds")` 每 10s 发一个 `server.heartbeat` 事件
5. **`haltStrategy: "left"`** —— 当 left stream（业务流）halt 时立即停止 heartbeat

**关键 HTTP headers**：
- `Cache-Control: no-cache, no-transform`：禁止 CDN/代理修改
- `X-Accel-Buffering: no`：Nginx 反代时不缓冲
- `X-Content-Type-Options: nosniff`：防 MIME 嗅探

`packages/server/src/handlers/event.ts` 是另一套 Event v2 SSE（为 TUI/Web 推送），15 秒心跳：

```typescript
const heartbeat = Stream.tick("15 seconds").pipe(Stream.map(() => ": heartbeat\n\n"))
return HttpServerResponse.stream(
  output.pipe(Stream.merge(heartbeat, { haltStrategy: "left" }), Stream.encodeText),
  {
    contentType: "text/event-stream",
    headers: { "Cache-Control": "no-cache, no-transform", "X-Accel-Buffering": "no", "X-Content-Type-Options": "nosniff" },
  },
)
```

**`: heartbeat\n\n` 是 SSE 注释行**，不算 event，**纯 keep-alive**。

### 6.2 SSE 解析器（反向）

`packages/opencode/src/control-plane/workspace.ts:203-251` 是 server-side SSE parser：

```typescript
const parseSSE = Effect.fn("Workspace.parseSSE")(function* (stream, onEvent) {
  yield* stream.pipe(
    Stream.decodeText(),
    Stream.splitLines,
    Stream.mapAccum(
      () => ({ data: [] as string[], id: undefined as string | undefined, retry: 1000 }),
      (state, line) => {
        if (line === "") {
          if (!state.data.length) return [state, []]
          return [{ ...state, data: [] }, [{ data: state.data.join("\n"), id: state.id, retry: state.retry }]]
        }
        const index = line.indexOf(":")
        const field = index === -1 ? line : line.slice(0, index)
        const value = index === -1 ? "" : line.slice(index + (line[index + 1] === " " ? 2 : 1))
        if (field === "data") return [{ ...state, data: [...state.data, value] }, []]
        if (field === "id") return [{ ...state, id: value }, []]
        if (field === "retry") {
          const retry = Number.parseInt(value, 10)
          return [Number.isNaN(retry) ? state : { ...state, retry }, []]
        }
        return [state, []]
      },
      {
        onHalt: (state) =>
          state.data.length ? [{ data: state.data.join("\n"), id: state.id, retry: state.retry }] : [],
      },
    ),
    Stream.map((event) => {
      try {
        return JSON.parse(event.data) as unknown
      } catch {
        return { type: "sse.message", properties: { data: event.data, id: event.id || undefined, retry: event.retry } }
      }
    }),
    Stream.runForEach(onEvent),
  )
})
```

**`Stream.mapAccum` 状态机**：处理 SSE 多行 `data:`（用 `\n` 拼接）、`id:`、`retry:` 三种字段，**符合 WHATWG SSE 规范**。

### 6.3 OpenAI Responses WebSocket 协议

`packages/opencode/src/plugin/openai/ws.ts` 是 **WebSocket 实现 + 协议握手**：

```typescript
export const PROTOCOL_HEADER = "responses_websockets=2026-02-06"
export const MESSAGE_TOO_BIG_CLOSE_CODE = 1009

export function connectResponsesWebSocket(options: ConnectResponsesWebSocketOptions) {
  return new Promise<WebSocket>((resolve, reject) => {
    if (options.signal?.aborted) {
      reject(abortError(options.signal))
      return
    }
    const headers: Record<string, string> = {
      ...options.headers,
      "openai-beta": options.headers["openai-beta"] ?? PROTOCOL_HEADER,
    }
    delete headers["content-length"]
    // Bun does not apply HTTP(S)_PROXY to WebSockets unless the proxy is supplied explicitly.
    const proxy =
      typeof Bun === "undefined"
        ? undefined
        : ProxyEnv.getProxyForUrl(options.url.replace(/^wss:/, "https:").replace(/^ws:/, "http:"))
    const connect = { headers, ...(proxy ? { proxy } : {}) }
    const socket = new WebSocket(options.url, connect)
    const timeout = options.timeout
      ? setTimeout(() => {
          cleanup()
          socket.on("error", () => {})
          socket.terminate()
          reject(new Error("WebSocket connect timed out"))
        }, options.timeout)
      : undefined
    function cleanup() {
      if (timeout) clearTimeout(timeout)
      socket.off("open", onOpen)
      socket.off("error", onError)
      socket.off("close", onClose)
      options.signal?.removeEventListener("abort", onAbort)
    }
    // ...
  })
}
```

**3 个关键点**：

1. **`openai-beta: responses_websockets=2026-02-06`** —— 协议版本号，OpenAI 用 beta header 区分新旧协议
2. **Bun 代理注入** —— `Bun does not apply HTTP(S)_PROXY to WebSockets unless the proxy is supplied explicitly`，**手动从 `ProxyEnv` 拿代理注入 connect options**
3. **`MESSAGE_TOO_BIG_CLOSE_CODE = 1009`** —— 1009 是 RFC 6455 的 `Message Too Big`，**触发后 fallback 到 HTTP**

### 6.4 WebSocket 连接池

`packages/opencode/src/plugin/openai/ws-pool.ts` 是 session 级 WebSocket 池：

```typescript
const DEFAULT_CONNECT_TIMEOUT = 15_000       // 15s 连接超时
const DEFAULT_IDLE_TIMEOUT = 5 * 60 * 1000   // 5min 空闲超时
const DEFAULT_MAX_CONNECTION_AGE = 55 * 60 * 1000  // 55min 最大连接时长（防 server 强制断）
const streamRetries = options?.streamRetries ?? 5

interface PoolEntry {
  socket?: WebSocket
  connectedAt?: number
  lastUsedAt: number
  busy: boolean
  fallback: boolean       // 永久 fallback 到 HTTP
  streamFailures: number
}

export function createWebSocketFetch(options?: CreateWebSocketFetchOptions) {
  const pool = new Map<string, PoolEntry>()  // sessionID → entry
  const pruneTimer = setInterval(() => prune(), Math.min(idleTimeout, 60_000))
  pruneTimer.unref()  // 不阻塞退出
}
```

**Pool Key 规则**：

```typescript
const sessionID = internalHeaders["x-session-affinity"] ?? internalHeaders["session-id"]
if (!sessionID) return httpFetch(input, httpInit)
const key = `${sessionID}:conversation`
```

**每个 session 的 conversation = 1 个持久 WebSocket**，复用连接降握手开销。

**Fallback 策略**：

```typescript
function recordStreamFailure(entry: PoolEntry) {
  entry.streamFailures++
  // Codex counts retries after the initial failed WebSocket attempt.
  if (entry.streamFailures > streamRetries) entry.fallback = true
}
```

**5 次流失败 → 永久 fallback HTTP**（与 Codex 一致）。

### 6.6 流处理（WebSocket → SSE 转换）

`ws-pool.ts:139-342` 把 OpenAI WebSocket 帧转换为 **SSE 格式的 ReadableStream**：

```typescript
export function streamResponsesWebSocket(options) {
  const encoder = new TextEncoder()
  let socket = options.socket
  let controller: ReadableStreamDefaultController<Uint8Array> | undefined
  let cleanupSocket = () => {}
  let completed = false
  let emitted = false
  let idleTimer: ReturnType<typeof setTimeout> | undefined

  async function onMessage(data: WebSocket.RawData, isBinary: boolean) {
    if (completed) return
    if (isBinary) {
      invalidate(new ProviderError.ResponseStreamError("Unexpected binary WebSocket frame"))
      return
    }
    const text = data.toString()
    const event = (() => {
      try {
        const parsed = JSON.parse(text)
        return typeof parsed === "object" && parsed !== null ? parsed : undefined
      } catch { return undefined }
    })()

    // ... retryable terminal handling ...
    // ... wrapped error handling ...
    
    if (!emitted) options.onFirstEvent?.()
    controller?.enqueue(
      encoder.encode(`${text.split(/\r?\n/).map((line) => `data: ${line}`).join("\n")}\n\n`),
    )
    emitted = true
    resetIdleTimeout("idle timeout waiting for websocket")
    // ...
    if (event.type === "response.completed" || event.type === "response.done") {
      completed = true
      options.onComplete?.(event)
      options.onTerminal?.(event)
      closeCompleted()  // 发 data: [DONE]\n\n 然后 close
    }
  }
  
  return new Response(
    new ReadableStream<Uint8Array>({
      start(next) {
        controller = next
        options.signal?.addEventListener("abort", onAbort, { once: true })
        if (options.signal?.aborted) { onAbort(); return }
        attach(socket)
      },
      cancel(reason) { onCancel(reason) },
    }),
    { status: 200, headers: { "content-type": "text/event-stream" } },
  )
}
```

**关键转换**：

- **WebSocket 文本帧 → `data: {frame}\n\n`**
- **`response.completed`/`response.done` → `data: [DONE]\n\n` + close**（与 OpenAI SSE 一致）
- **二进制帧** → **视为错误**（OpenAI Responses 协议禁止二进制帧）
- **Idle timeout** —— 通过 `resetIdleTimeout` 在每条消息后 reset

### 6.7 WebSocket Tracker

`packages/opencode/src/server/routes/instance/httpapi/websocket-tracker.ts`：

```typescript
const layer = Layer.sync(Service)(() => {
  const sockets = new Set<Close>()
  let closing = false
  return Service.of({
    add: (close) =>
      Effect.gen(function* () {
        if (closing) return false
        sockets.add(close)
        return true
      }),
    remove: (close) => Effect.sync(() => { sockets.delete(close) }),
    closeAll: Effect.gen(function* () {
      closing = true
      const active = Array.from(sockets)
      sockets.clear()
      yield* Effect.all(
        active.map((close) => close.pipe(Effect.timeout("1 second"), Effect.catch(() => Effect.void))),
        { concurrency: "unbounded", discard: true },
      )
    }),
  })
})
```

**作用**：跟踪所有 active WebSocket，**`closeAll` 在 server shutdown 时强制 1s 内关掉**。

### 6.8 mDNS 广播

`packages/opencode/src/server/mdns.ts`：

```typescript
import { Bonjour } from "bonjour-service"

export function publish(port: number, domain?: string) {
  if (currentPort === port) return
  if (bonjour) unpublish()
  try {
    const host = domain ?? "opencode.local"
    const name = `opencode-${port}`
    bonjour = new Bonjour()
    const service = bonjour.publish({ name, type: "http", host, port, txt: { path: "/" } })
    service.on("error", () => {})
    currentPort = port
  } catch {
    if (bonjour) { try { bonjour.destroy() } catch {} }
    bonjour = undefined
    currentPort = undefined
  }
}
```

**Bonjour/mDNS** —— 让局域网设备能通过 `opencode.local` 发现 opencode 实例（无需 IP）。**`service.on("error", () => {})`** 是关键：mDNS 错误是「正常异常」（如 Windows 没装 Bonjour），**吞掉不抛**。

### 6.9 laew gap L64-L68（实时通信）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L64** | 无 SSE 流式 | opencode EventV2 + Stream.fromQueue + 10s heartbeat；laew 用 `eventsource-client` crate |
| **L65** | 无 WS 客户端 | opencode ws-pool + session affinity；laew 当前无 WS |
| **L66** | 无心跳 | opencode `Stream.tick("10 seconds")` + `: heartbeat` SSE 注释；laew 需加 SSE keep-alive |
| **L67** | 无重连退避 | opencode mDNS 监听 + instance-disposed 自动断；laew e2e 缺断线重连 |
| **L68** | 无背压 | opencode `Queue.unbounded` + Effect Stream；laew TUI 当前是 unbounded，应加 bounded + 截断 |

---

## 7. DevContainer 与容器化

opencode 的容器化体系在 `packages/containers/`，**5 层 Docker 镜像**专给 GitHub Actions 用（**不是产品 DevContainer**）。

### 7.1 5 层 Docker 镜像

```
base (Ubuntu 24.04)
  └ bun-node (base + Bun + Node.js 24)
      ├ rust (bun-node + Rust stable)
      │   └ tauri-linux (rust + Tauri deps)
      └ publish (bun-node + docker.io + pacman)
```

**每层 Dockerfile**：

`base/Dockerfile`：

```dockerfile
ARG REGISTRY=ghcr.io/anomalyco
FROM ${REGISTRY}/build/bun-node:24.04

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
  && apt-get install -y --no-install-install-recommends \
    docker.io \
    pacman-package-manager \
  && rm -rf /var/lib/apt/lists/*
```

`bun-node/Dockerfile`：

```dockerfile
FROM ubuntu:24.04
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl git jq openssh-client \
    pkg-config python3 unzip xz-utils zip \
  && rm -rf /var/lib/apt/lists/*
```

`rust/Dockerfile`：

```dockerfile
ARG REGISTRY=ghcr.io/anomalyco
FROM ${REGISTRY}/build/bun-node:24.04
ARG RUST_TOOLCHAIN=stable
ENV CARGO_HOME=/opt/cargo
ENV RUSTUP_HOME=/opt/rustup
RUN set -euo pipefail; \
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "${RUST_TOOLCHAIN}"; \
  rustc --version; cargo --version
```

`tauri-linux/Dockerfile`：

```dockerfile
ARG REGISTRY=ghcr.io/anomalyco
FROM ${REGISTRY}/build/rust:24.04
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    libappindicator3-dev \
    libwebkit2gtk-4.1-dev \
    librsvg2-dev \
    patchelf \
  && rm -rf /var/lib/apt/lists/*
```

### 7.2 Build Script（Bun 多平台）

`packages/containers/script/build.ts`：

```typescript
const reg = process.env.REGISTRY ?? "ghcr.io/anomalyco"
const tag = process.env.TAG ?? "24.04"
const push = process.argv.includes("--push") || process.env.PUSH === "1"

const root = path.join(rootDir, "package.json")
const pkg = await Bun.file(root).json()
const manager = pkg.packageManager ?? ""
const bun = manager.startsWith("bun@") ? manager.slice(4) : ""
if (!bun) throw new Error("packageManager must be bun@<version>")

const images = ["base", "bun-node", "rust", "tauri-linux", "publish"]

const setup = async () => {
  if (!push) return
  const list = await $`docker buildx ls`.text()
  if (list.includes("opencode")) {
    await $`docker buildx use opencode`
    return
  }
  await $`docker buildx create --name opencode --use`
}

await setup()
const platform = "linux/amd64,linux/arm64"
```

**关键设计**：

1. **`bun` 版本自动从 `package.json` 读取** —— 不硬编码
2. **`buildx create --name opencode`** —— 复用 buildx instance
3. **`linux/amd64,linux/arm64`** —— 双架构
4. **`--push` 自动多架构推 registry**

### 7.3 GitHub Actions 用法

```yaml
jobs:
  build-cli:
    runs-on: ubuntu-latest
    container:
      image: ghcr.io/anomalyco/build/bun-node:24.04
```

**优势**：CI 不用每次重新下载 Bun + Node + 工具链，**缓存命中率高**。

### 7.4 Sidecar as DevContainer？

opencode 的 sidecar 实际上**可以被视为一种 DevContainer 模式**：
- 独立 utility process
- 自带 XDG_STATE_HOME / userData 隔离
- 通过 HTTP + auth 头通信

**但缺少真正的 DevContainer support**：opencode 没有 `.devcontainer/devcontainer.json`，**用户在自己 devcontainer 中跑 opencode TUI 与直接跑无差异**。

### 7.5 laew gap L69-L73（容器化）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L69** | 无 Dockerfile | opencode 5 layer 模式；laew 应用 `Rust 1.x + cargo` 两层 |
| **L70** | 无 docker-compose | laew 可加 `docker-compose.yml` 用于完整 dev 栈 |
| **L71** | 无 Dev Container | laew 加 `.devcontainer/devcontainer.json`（Rust + Redis + Node） |
| **L72** | 无 digest 钉 | opencode 用 `ARG REGISTRY=ghcr.io/anomalyco`；laew 应钉 image digest |
| **L73** | 无远程编排 | opencode enterprise 是云端 SolidStart；laew 当前无 remote orchestrator |

---

## 8. CRDT 与多端冲突

opencode 的多端同步**不是真正的 CRDT**，而是 **server-side Durable Object 存储 + 历史快照 + replay/steal 协议**。本节剖析 enterprise + sync 体系。

### 8.1 Enterprise Storage（双后端）

`packages/enterprise/src/core/storage.ts:1-65`：

```typescript
import { AwsClient } from "aws4fetch"

export namespace Storage {
  export interface Adapter {
    read(path: string): Promise<string | undefined>
    write(path: string, value: string): Promise<void>
    remove(path: string): Promise<void>
    list(options?: { prefix?: string; limit?: number; after?: string; before?: string }): Promise<string[]>
  }

  function createAdapter(client: AwsClient, endpoint: string, bucket: string): Adapter {
    const base = `${endpoint}/${bucket}`
    return {
      async read(path: string): Promise<string | undefined> {
        const response = await client.fetch(`${base}/${path}`)
        if (response.status === 404) return undefined
        if (!response.ok) throw new Error(`Failed to read ${path}: ${response.status}`)
        return response.text()
      },
      // ... write / remove / list ...
      async list(options?: { prefix?: string; limit?: number; after?: string; before?: string }): Promise<string[]> {
        const prefix = options?.prefix || ""
        const params = new URLSearchParams({ "list-type": "2", prefix })
        if (options?.limit) params.set("max-keys", options.limit.toString())
        if (options?.after) {
          const afterPath = prefix + options.after + ".json"
          params.set("start-after", afterPath)
        }
        const response = await client.fetch(`${base}?${params}`)
        // ... parse XML ...
      },
    }
  }

  function s3(): Adapter {
    const bucket = process.env.OPENCODE_STORAGE_BUCKET!
    const region = process.env.OPENCODE_STORAGE_REGION || "us-east-1"
    const client = new AwsClient({
      region,
      accessKeyId: process.env.OPENCODE_STORAGE_ACCESS_KEY_ID!,
      secretAccessKey: process.env.OPENCODE_STORAGE_SECRET_ACCESS_KEY!,
    })
    return createAdapter(client, `https://s3.${region}.amazonaws.com`, bucket)
  }

  function r2() {
    const accountId = process.env.OPENCODE_STORAGE_ACCOUNT_ID!
    const client = new AwsClient({
      accessKeyId: process.env.OPENCODE_STORAGE_ACCESS_KEY_ID!,
      secretAccessKey: process.env.OPENCODE_STORAGE_SECRET_ACCESS_KEY!,
    })
    return createAdapter(client, `https://${accountId}.r2.cloudflarestorage.com`, process.env.OPENCODE_STORAGE_BUCKET!)
  }

  const adapter = lazy(() => {
    const type = process.env.OPENCODE_STORAGE_ADAPTER
    if (type === "r2") return r2()
    if (type === "s3") return s3()
    throw new Error("No storage adapter configured")
  })

  function resolve(key: string[]) {
    return key.join("/") + ".json"
  }

  export async function read<T>(key: string[]) {
    const result = await adapter().read(resolve(key))
    if (!result) return undefined
    return JSON.parse(result) as T
  }

  export function write<T>(key: string[], value: T) {
    return adapter().write(resolve(key), JSON.stringify(value))
  }

  // ...
}
```

**关键设计**：

1. **`Adapter` 抽象接口** —— 4 个方法：read/write/remove/list
2. **AWS Signature V4** —— `AwsClient` 客户端支持 S3 + R2（Cloudflare）
3. **双后端**：S3 (AWS) / R2 (Cloudflare)，通过 `OPENCODE_STORAGE_ADAPTER` 切换
4. **`KEYS` 是数组** —— `["share_snapshot", shareID]` → `share_snapshot/{shareID}.json`
5. **Lazy 加载** —— `lazy(() => ...)` 直到首次使用时才校验 env
6. **`aws4fetch`** —— Cloudflare Workers 兼容的 AWS SigV4 客户端

### 8.2 Share 数据迁移（lazy migrate）

`packages/enterprise/src/core/share.ts:78-115`：

```typescript
async function legacy(shareID: string) {
  const compaction: Compaction = (await Storage.read<Compaction>(["share_compaction", shareID])) ?? {
    data: [],
    event: undefined,
  }
  const list = await Storage.list({
    prefix: ["share_event", shareID],
    before: compaction.event,
  }).then((x) => x.toReversed())
  if (list.length === 0) {
    if (compaction.data.length > 0) await writeSnapshot(shareID, compaction.data)
    return compaction.data
  }
  const next = merge(
    compaction.data,
    await Promise.all(list.map(async (event) => await Storage.read<Data[]>(event))).then((x) =>
      x.flatMap((item) => item ?? []),
    ),
  )
  await Promise.all([
    Storage.write(["share_compaction", shareID], {
      event: list.at(-1)?.at(-1),
      data: next,
    }),
    writeSnapshot(shareID, next),
  ])
  return next
}

export const sync = fn(
  z.object({
    share: Info.pick({ id: true, secret: true }),
    data: Data.array(),
  }),
  async (input) => {
    const share = await get(input.share.id)
    if (!share) throw new Errors.NotFound(input.share.id)
    if (share.secret !== input.share.secret) throw new Errors.InvalidSecret(input.share.id)
    const data = (await readSnapshot(input.share.id)) ?? (await legacy(input.share.id))
    await writeSnapshot(input.share.id, merge(data, input.data))
  },
)
```

**3 个存储层**：

1. **`share_snapshot/{id}`** —— 当前快照（**永远是最新合并结果**）
2. **`share_compaction/{id}`** —— compaction 进度（含 `event: string` 游标 + 已合并 data）
3. **`share_event/{id}/{seq}`** —— 单条增量事件（**event sourcing**）

**Lazy migrate 流程**：

- 优先 `readSnapshot`（快照）
- 失败则 `legacy()`：读 compaction + 增量 events，合并后写新 snapshot
- **`(await readSnapshot) ?? (await legacy)`** —— **客户端无感迁移**

### 8.3 Sync 协议（steal / replay / history）

`packages/opencode/src/server/routes/instance/httpapi/handlers/sync.ts`：

```typescript
const start = Effect.fn("SyncHttpApi.start")(function* () {
  yield* workspace
    .startWorkspaceSyncing((yield* InstanceState.context).project.id)
    .pipe(Effect.ignore, Effect.forkIn(scope))
  return true
})

const replay = Effect.fn("SyncHttpApi.replay")(function* (ctx: { payload: typeof ReplayPayload.Type }) {
  const payload: EventV2.SerializedEvent[] = ctx.payload.events.map((event) => ({
    id: event.id, aggregateID: event.aggregateID, seq: event.seq, type: event.type, data: { ...event.data },
  }))
  const source = payload[0].aggregateID
  yield* Effect.logInfo("sync replay requested", {
    sessionID: source, events: payload.length, first: payload[0]?.seq, last: payload.at(-1)?.seq, directory: ctx.payload.directory,
  })
  const ownerID = yield* InstanceState.workspaceID
  yield* events.replayAll(payload, { ownerID, strictOwner: true })
  yield* Effect.logInfo("sync replay complete", {
    sessionID: source, events: payload.length, first: payload[0]?.seq, last: payload.at(-1)?.seq,
  })
  return { sessionID: source }
})

const steal = Effect.fn("SyncHttpApi.steal")(function* (ctx: { payload: typeof SessionPayload.Type }) {
  const workspaceID = yield* InstanceState.workspaceID
  if (!workspaceID) return yield* new HttpApiError.BadRequest({})
  yield* session.setWorkspace({ sessionID: ctx.payload.sessionID, workspaceID })
  yield* Effect.logInfo("sync session stolen", { sessionID: ctx.payload.sessionID, workspaceID })
  return { sessionID: ctx.payload.sessionID }
})

const history = Effect.fn("SyncHttpApi.history")(function* (ctx: { payload: typeof HistoryPayload.Type }) {
  const exclude = Object.entries(ctx.payload)
  return yield* db
    .select()
    .from(EventTable)
    .where(
      exclude.length > 0
        ? not(or(...exclude.map(([id, seq]) => and(eq(EventTable.aggregate_id, id), lte(EventTable.seq, seq))))!)
        : undefined,
    )
    .orderBy(asc(EventTable.seq))
    .all()
    .pipe(Effect.orDie)
})
```

**4 个 sync 命令**：

1. **`start`** —— 启动 workspace 同步（forked fiber，不阻塞响应）
2. **`replay`** —— 服务端 replay 一批事件（用于**多端最终一致**）
3. **`steal`** —— 把 session 迁移到当前 workspace（**多端切换场景**）
4. **`history`** —— 拉取**比指定 `[aggregateID, seq]` 旧的所有事件**（增量同步）

**关键 SQL `history`**：

```sql
SELECT * FROM event_table
WHERE NOT (
  aggregate_id = $1 AND seq <= $2
  OR aggregate_id = $3 AND seq <= $4
  ...
)
ORDER BY seq ASC
```

**「不包含已经处理过的事件」** —— 标准增量同步。

### 8.4 EventV2 Durable Seq

`packages/opencode/src/sync/schema.ts`：

```typescript
import { Schema } from "effect"
import { Identifier } from "@/id/id"
import { statics } from "@opencode-ai/core/schema"

export const EventID = Schema.String.check(Schema.isStartsWith("evt")).pipe(
  Schema.brand("EventID"),
  statics((s) => ({
    ascending: (id?: string) => s.make(Identifier.ascending("event", id)),
  })),
)
```

**`Identifier.ascending("event", id)`** —— 生成**全局单调递增**事件 ID（前缀 `evt` + 时间戳 + 计数器）。**`ascending` 是为多端 merge 而设计**：客户端按 ascending 排序即可保证因果一致。

### 8.5 EventV2 Bridge（事件路由）

`packages/opencode/src/event-v2-bridge.ts`：

```typescript
const publish: EventV2.Interface["publish"] = (definition, data, options) =>
  Effect.gen(function* () {
    if (options?.location) return yield* events.publish(definition, data, options)
    const ctx = yield* InstanceRef
    if (!ctx) return yield* events.publish(definition, data, options)
    const workspaceID = yield* WorkspaceRef
    return yield* events.publish(definition, data, {
      ...options,
      location: new Location.Info({
        directory: AbsolutePath.make(ctx.directory),
        ...(workspaceID ? { workspaceID } : {}),
        project: { id: Project.ID.make(ctx.project.id), directory: AbsolutePath.make(ctx.worktree) },
      }),
    })
  })

const unsubscribe = yield* events.listen((event) =>
  Effect.gen(function* () {
    const ctx = yield* InstanceRef
    const workspaceID = (yield* WorkspaceRef) ?? event.location?.workspaceID
    GlobalBus.emit("event", {
      directory: event.location?.directory ?? ctx?.directory,
      project: ctx?.project.id,
      workspace: workspaceID,
      payload: { id: event.id, type: event.type, properties: event.data },
    })
    if (event.durable === undefined) return
    GlobalBus.emit("event", {
      directory: event.location?.directory ?? ctx?.directory,
      project: ctx?.project.id,
      workspace: workspaceID,
      payload: {
        type: "sync",
        syncEvent: {
          id: event.id,
          type: EventV2.versionedType(event.type, event.durable.version),
          seq: event.durable.seq,
          aggregateID: event.durable.aggregateID,
          data: event.data,
        },
      },
    })
  }),
)
```

**2 路 GlobalBus.emit**：

1. **`payload: { id, type, properties }`** —— **本地路由**（renderer / TUI 订阅）
2. **`payload: { type: "sync", syncEvent }`** —— **sync 协议**（带 version + seq，多端 replay）

### 8.6 Workspace SSE 路由

`packages/opencode/src/control-plane/workspace.ts:184-201`：

```typescript
const connectSSE = Effect.fn("Workspace.connectSSE")(function* (
  url: URL | string,
  headers: HeadersInit | undefined,
) {
  const response = yield* http.execute(
    HttpClientRequest.get(route(url, "/global/event"), {
      headers: new Headers(headers),
      accept: "text/event-stream",
    }),
  )
  if (response.status < 200 || response.status >= 300) {
    return yield* new SyncHttpError({
      message: `Workspace sync HTTP failure: ${response.status}`,
      status: response.status,
    })
  }
  return response.stream
})
```

**Workspace 端通过 SSE 长连接到 cloud workspace**（`/global/event`），接收 sync 事件。

### 8.7 Share ShareID 算法

`packages/enterprise/src/core/share.ts:117-128`：

```typescript
export const create = fn(z.object({ sessionID: z.string() }), async (body) => {
  const isTest = process.env.NODE_ENV === "test" || body.sessionID.startsWith("test_")
  const info: Info = {
    id: (isTest ? "test_" : "") + body.sessionID.slice(-8),  // sessionID 取后 8 位
    sessionID: body.sessionID,
    secret: crypto.randomUUID(),
  }
  const exists = await get(info.id)
  if (exists) throw new Errors.AlreadyExists(info.id)
  await Promise.all([Storage.write(["share", info.id], info), writeSnapshot(info.id, [])])
  return info
})
```

**`shareID = sessionID.slice(-8)`** —— 用户友好的短 ID（8 字符），**与 sessionID 末尾 8 位一致**。secret 是 UUID，用于删除鉴权。

### 8.8 laew gap L74-L78（CRDT/同步）

| Gap | 描述 | 借鉴方案 |
|-----|------|----------|
| **L74** | 无 Session 共享 | opencode `share.create` + `Storage.read/write`；laew 当前 SQLite 单机 |
| **L75** | SQLite WAL 多端 | opencode Storage 是 S3/R2 object storage；laew SQLite + WAL 是单进程，**多端需 Cloudflare Durable Object** |
| **L76** | 无冲突解决 | opencode `merge(...items)` + Identifier.ascending；laew 需 `yrs`（Yjs Rust port） |
| **L77** | 无 Event Sourcing | opencode EventV2 + replay；laew 当前 SQLite 单条 row |
| **L78** | 无协同编辑 | opencode share.steal + workspace sync；laew 需 WebSocket + CRDT library |

---

## 附录：laew gap 清单 L38-L78

### CrashDump (L38-L42)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L38 | 无 panic hook | P0 | 1d |
| L39 | 无指数退避 | P0 | 0.5d |
| L40 | 无熔断器 | P1 | 2d |
| L41 | 无错误分类 | P1 | 3d |
| L42 | 错误 UX 差 | P2 | 1w |

### WebUI/DesktopApp (L44-L48)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L44 | 无 web 远控 | P1 | 1w |
| L45 | 无 desktop 壳 | P1 | 2w |
| L46 | 无 WASM | P2 | 2w |
| L47 | 无多端 Session | P1 | 1w |
| L48 | 测试栈薄 | P2 | 1w |

### OAuth (L49-L52)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L49 | API Key 明文 | P0 | 1d |
| L50 | 无 OAuth 流程 | P1 | 1w |
| L51 | 脱敏范围不足 | P0 | 0.5d |
| L52 | 无多账号轮换 | P1 | 1w |

### i18n (L54-L58)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L54 | TUI 中文化硬编码 | P0 | 1w |
| L55 | 文档无双语 | P1 | 3d |
| L56 | 错误无 i18n | P1 | 1w |
| L57 | 无 RTL | P2 | 1w |
| L58 | 无翻译 pipeline | P2 | 1w |

### Release (L59-L63)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L59 | 无 CI | P0 | 3d |
| L60 | 手动 release | P0 | 1d |
| L61 | 无 Auto Update | P2 | 1w |
| L62 | 无签名 | P1 | 3d |
| L63 | 无分发 | P2 | 1w |

### WebSocket/SSE (L64-L68)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L64 | 无 SSE 流式 | P0 | 1w |
| L65 | 无 WS 客户端 | P2 | 1w |
| L66 | 无心跳 | P0 | 0.5d |
| L67 | 无重连退避 | P1 | 1w |
| L68 | 无背压 | P1 | 3d |

### DevContainer (L69-L73)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L69 | 无 Dockerfile | P0 | 1d |
| L70 | 无 docker-compose | P1 | 3d |
| L71 | 无 Dev Container | P1 | 1d |
| L72 | 无 digest 钉 | P2 | 1d |
| L73 | 无远程编排 | P2 | 1w |

### CRDT (L74-L78)

| ID | Gap | 优先级 | 工作量 |
|----|-----|--------|--------|
| L74 | 无 Session 共享 | P1 | 1w |
| L75 | SQLite WAL 多端 | P1 | 1w |
| L76 | 无冲突解决 | P2 | 2w |
| L77 | 无 Event Sourcing | P2 | 1w |
| L78 | 无协同编辑 | P2 | 2w |

### 总结

**P0（紧急，1 周内）**：L38/L39/L49/L51/L54/L59/L60/L64/L66/L69 —— **10 项**

**P1（重要，2-4 周内）**：L40/L41/L44/L45/L47/L50/L52/L55/L56/L62/L67/L68/L70/L71/L74/L75 —— **16 项**

**P2（进阶，1-3 月）**：L42/L46/L48/L57/L58/L61/L63/L65/L72/L73/L76/L77/L78 —— **13 项**

**总计**：**41 个 gap**，是 laew 从「能跑」升级到「生产级 Agent CLI」的核心改造清单。

### 关键借鉴文件路径

```
/usr/local/LsmGitOpenSource/opencode/packages/
├── desktop/src/main/
│   ├── index.ts                    # Electron 主进程 Effect.gen
│   ├── logging.ts                  # CrashReporter + NetLog + 日志清理
│   ├── updater.ts                  # AutoUpdater 8 态机
│   ├── updater-controller.ts       # 状态机实现
│   ├── server.ts                   # sidecar spawn + health check
│   ├── sidecar.ts                  # sidecar 进程入口
│   ├── unresponsive.ts             # renderer unresponsive 采样
│   ├── migrate.ts                  # Tauri → Electron 迁移
│   └── wsl/servers.ts              # WSL 多 distro 管理
├── app/src/
│   ├── context/language.tsx        # 业务组件 i18n (60+ locales)
│   ├── i18n/desktop-native.ts      # Native bundle 协议
│   └── updater.ts                  # UpdaterState 类型
├── opencode/src/
│   ├── auth/index.ts               # OAuth/Api/WellKnown 三元组
│   ├── server/
│   │   ├── server.ts               # Hono server 启动
│   │   ├── mdns.ts                 # Bonjour/mDNS 广播
│   │   └── routes/instance/httpapi/
│   │       ├── handlers/event.ts   # SSE 事件流
│   │       └── handlers/sync.ts    # replay/steal/history
│   ├── control-plane/workspace.ts  # Workspace sync + SSE parser
│   ├── event-v2-bridge.ts          # 2 路 GlobalBus 路由
│   ├── event-manifest.ts           # 事件 schema 注册
│   └── sync/schema.ts              # EventID ascending
├── plugin/openai/
│   ├── ws.ts                       # OpenAI Responses WebSocket 协议
│   └── ws-pool.ts                  # session affinity 池
├── enterprise/src/
│   ├── core/storage.ts             # S3/R2 双后端
│   └── core/share.ts               # 3 层存储 + lazy migrate
├── containers/
│   ├── base/Dockerfile             # Ubuntu 24.04
│   ├── bun-node/Dockerfile         # + Bun + Node 24
│   ├── rust/Dockerfile             # + Rust
│   ├── tauri-linux/Dockerfile      # + Tauri deps
│   ├── publish/Dockerfile          # + docker.io + pacman
│   └── script/build.ts             # 多架构 build/push
└── web/
    ├── astro.config.mjs            # Starlight 20 locales
    └── src/middleware.ts           # locale cookie + Accept-Language
```

---

# 第二十一章 第十八轮深挖：用户交互体验层（2026-09-09）

> 完整版：专题-第十八轮-opencode-深度分析.md（1033 行 / 8 维度 D1-D8 + gap L1516-L1545）

## 21.1 维度概览

| 维度 | 主题 | opencode 实现 | 关键文件 |
|---|---|---|---|
| **D1** | @提及系统 | 完整（file/agent/resource/IDE 桥 + frecency + extmark） | `tui/component/prompt/autocomplete.tsx` 781 行 |
| **D2** | 自定义斜杠命令 | 完整（4 源合并 + frontmatter 强校验 + $ARGUMENTS） | `opencode/src/command/index.ts` 177 行 |
| **D3** | Rewind/Fork | 完整（revert 栈 + parentID 重映射 + undo/redo） | `session/revert.ts` 137 行 + `session.fork` 691-732 |
| **D4** | 文件监视 | 仅被动（workspace 移动检测，无后台 fs.watch） | `dialog-workspace-file-changes.tsx` |
| **D5** | 富文本渲染 | Web:shiki+marked+KaTeX；TUI:自建 diff viewer | `@pierre/diffs` + `tui/feature-plugins/system/diff-viewer.tsx` |
| **D6** | 输入体验 | 完整（IME flush + 5 档粘贴 + Tab 目录展开 + stash） | `tui/component/prompt/index.tsx` 1716 行 |
| **D7** | Onboarding/Theme | 主题完整（30+ 内置 + system 派生 + 8 级配置）；trust 未实现 | `tui/theme/index.ts` 700+ 行 |
| **D8** | 状态线/成本/分享 | 完整（share-next + EventV2 实时 + cost Decimal + 4 档 footer） | `share/share-next.ts` 371 行 + `footer.width.ts` |

## 21.2 关键发现速览

### D1 @提及系统（autocomplete.tsx）

- **三态 store**：`visible: false | "@" | "/"` 共享一个 Autocomplete 组件（autocomplete.tsx:103）。
- **三源融合**：files（`sdk.client.v2.fs.find`）+ agents（`!hidden && mode !== "primary"`）+ MCP resources + reference aliases。
- **fff 服务端权威排序**：TUI 不二次 fuzzy 重排，避免覆盖文件路径得分；frecency 仍以客户端加成形式叠在 `scoreFn` 上。
- **行级 range**：`@path#Lstart-Lend` 自动拼 `path#N-M` + URL `?start=&end=`（extractLineRange L32-57）。
- **IDE JSON-RPC 桥**：`editor.ts:74-78` 的 `EditorMentionSchema` 订阅 `at_mentioned` 通知，从 Zed/VS Code 按 `@` 拉文件。
- **extmark 虚拟文本**：`<textarea>` 看到 `@README.md`，内部仍按 `PromptInfo.parts` 序列化（autocomplete.tsx:194-200）。

### D2 自定义斜杠命令（command/index.ts）

- **4 源合并优先级**：默认（init/review）→ 配置（`.opencode/{command,commands}/*.md`）→ MCP prompts → Skills。
- **Schema 强校验**：`Info = {template, description, agent, model, variant, subtask}`（`core/v1/config/command.ts`），失败抛 `InvalidError`（不静默降级）。
- **`hints(template)` 占位符抽取**：`$1/$2/...` 与 `$ARGUMENTS` 单独检测（command/index.ts:36-44）。
- **MCP Prompt 一等命令**：MCP server 的 prompt 直接暴露为斜杠命令，前端无需额外协议。
- **Skill 隐式命令**：Skill 文件即命令，模板追加 `Base directory for this skill: ...` 两行保留相对路径语义。

### D3 Rewind/Fork（revert.ts）

- **三段式 revert**：`track()` 备份当前 → `restore()` 还原到更早快照 → `revert(patches)` 倒序 reverse patch（revert.ts:38-89）。
- **patch part 收集**：回滚一个 message 时，需收集该 message 之后的 patch part（工具修改文件的结果）。
- **嵌套 revert**：`session.revert?.snapshot` 保留链式还原栈。
- **fork 重映射**：`idMap = new Map<MessageID, MessageID>()` 重建 assistant parentID 树；compaction `tail_start_id` 也要重映射（session.ts:710-728）。
- **undo/redo slash**：`messagesBeforeRevert().findLast(role === "user")` 回退到上一条 user message + 还原 prompt（session/index.tsx:611-670）。

### D4 文件监视（被动模式）

- **无后台 fs.watch**：调研证实全代码库无 `fs.watch` / `chokidar` / `fsevents` / `@parcel/watcher` 引用。
- **替代方案**：`DialogWorkspaceFileChanges.show` 在 session 切换/移动时被动传入 `git status` 列表，渲染 yes/no Dialog。
- **LSP 诊断未推送 TUI**：`footer.tsx` 只显示 LSP server 数量，不显示具体诊断；这是 D4 最大短板。
- **Git snapshot 感知**：`Snapshot.Service` 在 revert 时检测外部编辑，但运行中不显示。

### D5 富文本渲染

- **Web/Desktop**：`@pierre/diffs`（Web Component + Shiki 双栈）+ `shiki-wasm`（跨平台）+ `marked-shiki` + KaTeX 数学公式扩展。
- **TUI**：自建 `diff-viewer.tsx`（700+ 行）+ `PanelGroup/Panel/Separator` 布局原语；`@opentui/core` SyntaxStyle 主题 30+ 颜色字段。
- **懒加载语言**：首次见某语言才 `loadLanguage`，避免启动全量 bundle 200+ 语言。
- **折叠/分页**：`scrollbox height={min(count, 10, anchor.y)}` 限定最大高度 10 行。

### D6 输入体验（prompt/index.tsx）

- **编辑器**：`@opentui/core` 的 `TextareaRenderable`（不自研）。
- **IME 双 setTimeout flush**：`setTimeout(() => setTimeout(() => submit(), 0), 0)` 让 IME 合成事件先 process 一轮（prompt/index.tsx:1391-1395）。
- **粘贴 5 档**：空粘贴 → `prompt.paste` 命令；本地文件路径 → 读 attachment；URL 不读 attachment；大段文本（≥3 行或 >150 字符）折叠成 `[Pasted ~N lines]`。
- **frecency 公式**：`frequency / (1 + age / 86400000)`；持久化 JSONL append-only，启动自愈重写。
- **Prompt stash**：`/stash` / `/stash pop` / `/stash list` 三个 slash 临时保存输入。

### D7 Onboarding/Theme（theme/index.ts）

- **30+ 内置主题**：aura/ayu/catppuccin/dracula/github/gruvbox/tokyonight 等，远超 laew 当前 1 个默认主题。
- **`system` 自动派生主题**：从 `TerminalColors` 计算 ANSI 16 色 → 灰阶 → diff alpha tint → 完整 theme（开箱即用）。
- **ThemeJson 4 类值**：Hex / RefName / Variant（dark+light）/ RGBA；`$defs` 跨字段引用 + 循环引用检测。
- **8 级配置发现链**：`mergeDeep` 字段级合并 + `OPENCODE_DISABLE_PROJECT_CONFIG` 企业兜底。
- **Home tips 轮播**：30+ 条 tips 运行期随机选一条（tips-view.tsx:99），`<leader>h` 切换显示/隐藏。
- **目录信任未实现**：opencode 默认信任当前工作目录，不做"陌生仓库警告"；laew 不需要借鉴。

### D8 状态线/成本/分享（share-next.ts + footer.width.ts）

- **三层 share 架构**：`SessionShare.Service`（业务）→ `ShareNext.Service`（网络）→ `SessionShareTable`（SQLite）。
- **EventV2 自动同步**：每个 `session.updated` / `message.updated` / `part.updated` / `session_diff` 自动推到 ShareNext 队列，1 秒批 flush。
- **双 base URL + 鉴权切换**：未登录走 `opncd.ai`，登录 enterprise console 走 `/api/shares` + Bearer + `x-org-id`。
- **cost Decimal 精算**：`decimal.js` 避免浮点 + tier 模型 + cache 分价 + reasoning 同价 + Copilot 特殊公式，共 4 路径（session.ts:355-405）。
- **AI SDK v6 input 修正**：`safe(inputTokens - cacheReadInputTokens - cacheWriteInputTokens)`（L361-364）—— 这是 2025-2026 跨 SDK 升级期非常现实的补丁。
- **宽度自适应 statusline**：4 档断点（66/80/120/150），`contextHintLimit` 四档精确控制。
- **TUI 实时 cost 显示**：`subagent-footer.tsx:33-55` 从 `sync.data.message` 找最后 `AssistantMessage`，累加 tokens 换算 context 百分比 + cost。

## 21.3 laew gap 汇总（L1516-L1545，30 项）

| 优先级 | 数量 | 编号 |
|---|---|---|
| **P0 必做** | 7 | L1516 / L1517 / L1521 / L1526 / L1527 / L1539 / L1545 |
| **P1 重要** | 14 | L1518 / L1519 / L1522 / L1523 / L1528 / L1529 / L1531 / L1532 / L1534 / L1535 / L1536 / L1537 / L1540 / L1543 |
| **P2 进阶** | 9 | L1520 / L1524 / L1525 / L1530 / L1533 / L1538 / L1541 / L1542 / L1544 |

## 21.4 借鉴路线图

**第 1 步（P0，2-3 周）**：实现 L1516-L1517（@提及 + FilePart 序列化） + L1521（自定义斜杠命令） + L1545（session share 增量同步）。这三项把 opencode 最核心的"用户表达力"复制过来。

**第 2 步（P1，3-4 周）**：实现 L1526-L1527（revert/fork） + L1534-L1537（shiki+markdown+diff viewer） + L1543（多主题系统）。这些是"专业感"的核心。

**第 3 步（P2，长期）**：L1539（IME flush）+ L1540（粘贴 5 档）+ L1528-L1529（timeline + undo/redo）。细节体验打磨。

## 21.5 结语

opencode 在「用户交互体验层」展示了 4 个值得 laew 借鉴的核心模式：

1. **Part 化消息模型**：FilePart/AgentPart/TextPart 是统一的"prompt 附加物"抽象，extmark 虚拟文本桥接可见/不可见。
2. **EventV2 watcher 自动同步**：share/sync/record 三个外部系统都通过订阅同一组 Event 事件自动接收。
3. **AI SDK v6 修正补丁**：跨 SDK 升级期需要显式修正 inputTokens 计费口径。
4. **宽度自适应 statusline**：4 档断点 + responsive contextHintLimit，比固定 footer 体验好得多。

详细源码引用、机制剖析、设计巧妙点、完整 gap 表见专题文档 `专题/专题-第十八轮-opencode-深度分析.md`（1033 行 / 8 维度 / 30 gap）。

---

## 22. 第十九轮深挖：安全纵深 + A2A 协议 + 可访问性 a11y + 跨设备同步

> **调研日期**：2026-09-09
> **调研范围**：7 个参考工程（atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici）× 6 大新维度（D9-D14）
> **本轮定位**：第十八轮首次切入「用户交互体验层」（D1-D8），本轮继续深挖 D9 安全与威胁模型 / D10 多模态输出 / D11 A2A 协议 / D12 可访问性 a11y / D13 离线模式 / D14 跨设备同步
> **与前 18 轮关系**：前 18 轮已覆盖 D1-D8 用户交互体验层、协议 wire、工具抽象、Skill、Hook、TUI 渲染、OAuth、i18n、Release、WebSocket、CRDT、Telemetry、多租户、RRF、LLM 网关、Pregel、Agent 池、Turn 锁、Bash 检测、Session 持久化、内存加密、SQLite 全栈、反应式 IoC、守护进程基础设施、录制回放等 90+ 维度；**本轮不重述**
> **新增 laew gap**：L1591-L1930+（共 340+ 个新 gap，累计突破 1930）

### 22.0 第十九轮 6 大维度 × opencode 实现速览

| 维度 | 名称 | opencode 成熟度 | 核心机制 | laew gap 区段 |
|------|------|----------------|---------|--------------|
| **D9** | 安全与威胁模型 | ⭐⭐⭐ 应用层防护 | Effect Schema TaggedErrorClass 80+ 处 + tree-sitter 双语法 arity 字典 + 0o600 + OAuth 状态机 + 7 类正则脱敏 | L1591-L1645 |
| **D10** | 多模态输出 | ⭐⭐⭐⭐ 双栈渲染 | @pierre/diffs + shiki-wasm + KaTeX + Web/TUI 双栈 | L1641-L1700 |
| **D11** | A2A 协议与多 Agent 互操作 | ⭐ 无显式 A2A | Effect DI + Durable Object + R2 双后端（无 A2A/ACP/E2A/A2UI） | L1701-L1760 |
| **D12** | 可访问性 a11y / RTL | ⭐⭐⭐⭐ Web AA/AAA | 30+ 主题 + system 派生 + prefers-reduced-motion + 5 locale RTL + Solid ARIA + sr-only | L1761-L1820 |
| **D13** | 离线模式与本地优先 | ⭐⭐ 重连基础 | WebSocket 重连 + Effect DI + Durable Object | L1821-L1880 |
| **D14** | 跨设备同步与会话漫游 | ⭐⭐⭐ share-next 范本 | share-next.ts EventV2 + 1 秒批 flush + steal 抢占 + 双 base URL 鉴权 + Map LWW | L1881-L1930 |

---

### 22.1 D9 安全与威胁模型（8 子维度）

#### 22.1.1 opencode 安全实现总览

opencode 在安全维度属于「**轻信任派**」——应用层防护为主（Schema 校验 + tree-sitter AST + 0o600 + OAuth 状态机 + 7 类正则脱敏），**无 OS 级沙箱、无专用 Prompt 注入检测、无 SSRF 防护、无形式化 STRIDE 文档**。

| 子维度 | opencode 机制 | 代码定位 | 成熟度 |
|--------|--------------|---------|--------|
| **D9-1 STRIDE** | ❌ 无文档，以「Permission 三态引擎」替代 | `permission/index.ts` | ⭐⭐ 隐式覆盖 |
| **D9-2 Prompt 注入** | ⚠️ Schema TaggedErrorClass + 提示词工程 | `tool/tool.ts:24-34`、`tool/shell/prompt.ts:78-119` | ⭐⭐ 2 层 |
| **D9-3 Bash 检测** | ✅ web-tree-sitter 双语法（bash + powershell）+ arity 字典 150+ 命令元组 | `tool/shell.ts:311-336`、`permission/arity.ts:24-161` | ⭐⭐⭐ |
| **D9-4 凭证管理** | ✅ auth.json + OAuth 状态机 + 0o600 + 7 类正则脱敏 | `auth/index.ts:73-89`、`redaction.ts:5-38` | ⭐⭐⭐ |
| **D9-5 路径信任** | ✅ 外部目录边界检测 + realpathSync 解析 | `instance-context.ts:18-24`、`fs-util.ts:270-273` | ⭐⭐⭐ |
| **D9-6 进程沙箱** | ❌ 无 OS 沙箱，仅 Effect Layer 逻辑隔离 | `layer-node.ts:81-112`、`shell.ts:293-310` | ⭐ 逻辑隔离 |
| **D9-7 SSRF 防护** | ⚠️ 仅协议校验（http/https） | `webfetch.ts:35-37` | ⭐ 最弱 |
| **D9-8 决策审计** | ⚠️ Permission 事件（无 W3C traceparent） | `permission.ts:61-66`、`otlp.ts:55-77` | ⭐⭐ |

#### 22.1.2 D9-3 Bash 检测：tree-sitter 双语法 + arity 字典

**核心文件**：`packages/opencode/src/tool/shell.ts:311-336`

- 使用 **web-tree-sitter** 加载 bash + powershell 双语法 WASM
- 解析后的命令树用于提取 `command_name`、`command_argument`、`redirection` 等节点

**命令元组 arity 字典**（`permission/arity.ts:24-161`）：**150+ 条命令前缀 → arity 映射**，用于识别"人类可理解的命令"（如 `git checkout main` → `git checkout`）。

**与 claudecode / atomcode 差异**：
- claudecode / atomcode 走「**黑名单派**」（22+ 类破坏性命令规则 + FAIL-CLOSED AST）
- opencode 走「**元组识别派**」（150+ 命令元组 arity 字典，识别"人类可理解的命令"）
- opencode **无**破坏性命令检测、**无**递归解包、**无**反向 shell 检测

#### 22.1.3 D9-4 凭证管理：auth.json + OAuth 状态机 + 7 类正则脱敏

**核心文件**：`auth/index.ts:73-89`、`redaction.ts:5-38`

- auth.json 落盘 0o600 文件权限
- OAuth 状态机管理 token 生命周期
- 7 类正则脱敏（API Key / token / 凭证形状）

**与业界最佳实践差距**：
- ❌ 无应用层 AES-256-GCM 加密（openclaw Secret Sentinel）
- ❌ 无 macOS Keychain / DPAPI（claudecode）
- ❌ 无跨进程写锁（atomcode tempfile + fsync + atomic rename）
- ❌ 无时序安全比较（`subtle::constant_time_eq`）

#### 22.1.4 D9-7 SSRF：最弱一环

**核心文件**：`webfetch.ts:35-37`

```typescript
// 仅校验协议
if (url.protocol !== "http:" && url.protocol !== "https:") {
  throw new Error("scheme not allowed");
}
```

**完全缺失**：私有 IP 拦截、DNS 钉扎、IPv4-mapped IPv6 解析、maxResponseSize 限制、重定向清洗。

#### 22.1.5 D9 laew gap 汇总（opencode 视角）

| 优先级 | 编号 | 描述 |
|--------|------|------|
| **P0** | L1599 | API Key 明文存 SQLite，无 0o600 / Keychain |
| **P0** | L1600 | 无应用层 AES-256-GCM 加密 |
| **P0** | L1608 | 无私有 IP 拦截（loopback/private/CGNAT） |
| **P0** | L1592 | 无 Prompt 注入检测（14 类正则） |
| **P0** | L1596 | 无 Bash 安全检测器（23 层） |
| **P0** | L1603 | 无工作目录信任模型 |
| **P0** | L1610 | 无决策审计 3 段式 |
| **P1** | L1597 | 无 FAIL-CLOSED AST 白名单 |
| **P1** | L1609 | 无 DNS 钉扎（防 DNS rebinding） |
| **P1** | L1612 | 无双 pass scrub（key=value + token 形状） |
| **P1** | L1606 | 无 OS 级沙箱（Landlock/Seccomp/cgroup） |
| **P2** | L1634 | 无命令元组 arity 字典（150+） |
| **P2** | L1628 | 无 W3C traceparent 传播 |

---

### 22.2 D10 多模态输出（6 子维度）

> **状态**：D10 专题文档仍在生成中，本节基于已有调研摘要。

#### 22.2.1 opencode 双栈渲染机制

opencode 在 D10 属于「**双栈派**」（Web + TUI 各自最优渲染），成熟度 ⭐⭐⭐⭐。

| 子维度 | Web 端 | TUI 端 | 代码定位 |
|--------|--------|--------|---------|
| **Diff 渲染** | `@pierre/diffs`（Web Component + Shiki 双栈） | 自建 `diff-viewer.tsx`（700+ 行） | `tui/component/diff-viewer.tsx` |
| **语法高亮** | `shiki-wasm`（跨平台）+ `marked-shiki` | `@opentui/core` SyntaxStyle 30+ 颜色字段 | `tui/theme/syntax.ts` |
| **数学公式** | KaTeX 扩展 | ❌ 不支持 | `web/src/katex.ts` |
| **图片渲染** | `<img>` 原生 | ❌ 不支持（无 sixel/kitty） | — |
| **Markdown** | `marked` + `marked-shiki` | 简化 Markdown 渲染 | `tui/component/markdown.tsx` |
| **懒加载语言** | 首次见某语言才 `loadLanguage` | — | `shiki.ts` |

**关键设计**：
- **懒加载语言**：首次见某语言才 `loadLanguage`，避免启动全量 bundle 200+ 语言
- **折叠/分页**：`scrollbox height={min(count, 10, anchor.y)}` 限定最大高度 10 行

#### 22.2.2 D10 laew gap 汇总

| 优先级 | 编号 | 描述 |
|--------|------|------|
| **P0** | L1641 | 无 Diff 渲染器（当前 cell-based 纯文本） |
| **P0** | L1642 | 无语法高亮（`syntect` / `shiki-wasm`） |
| **P1** | L1643 | 无数学公式渲染（KaTeX） |
| **P1** | L1644 | 无图片终端协议（sixel / kitty / iTerm2） |
| **P2** | L1645 | 无懒加载语言 bundle |

---

### 22.3 D11 A2A 协议与多 Agent 互操作（7 子维度）

#### 22.3.1 opencode 现状：无显式 A2A/ACP/E2A/A2UI 实现

**关键发现**：7 个参考工程中，**仅 openclaw 实现了 Google A2A Protocol v1.0**（3,165 行），opencode 在 D11 维度**全部缺失**。

| 子维度 | opencode 状态 | 说明 |
|--------|--------------|------|
| **D11-1 A2A** | ❌ 无实现 | 仅 openclaw 实现 v1.0（3165 行） |
| **D11-2 ACP** | ❌ 无实现 | atomcode 7650 行 Rust / deepseek 1853 行 / openclaw 17001 行 |
| **D11-3 E2A** | ❌ 无显式 E2A | deepseek 事件总线 / claudecode Bridge |
| **D11-4 A2UI** | ❌ 无显式 A2UI | deepseek Tagged JSON / atomcode LiveViewHub |
| **D11-5 MCP 双向** | ⚠️ 基础 MCP 客户端 | 无 Sampling / 资源双向 |
| **D11-6 跨语言互操作** | — | TypeScript/Bun 单语言 |
| **D11-7 协议路由发现** | — | Durable Object 持久化（非路由发现） |

#### 22.3.2 D11 laew gap 汇总

| 优先级 | 编号 | 描述 |
|--------|------|------|
| **P0** | L1701 | 无 A2A Protocol 实现（SendMessage / GetTask / Task 6 态） |
| **P0** | L1702 | 无 ACP 实现（stdio v1/v2 + JSON-RPC 2.0） |
| **P0** | L1703 | 无 Agent Card 发现（`/.well-known/agent-card.json`） |
| **P0** | L1704 | 无 Task 存储与状态机（FIFO 会话队列 + 等待者模式） |
| **P1** | L1705 | 无 Peer 认证（SHA-256 + timingSafeEqual） |
| **P1** | L1706 | 无速率限制（滑动窗口 30 req/min） |
| **P1** | L1707 | 无 E2A 事件总线 |
| **P1** | L1708 | 无 A2UI 结构化 UI 渲染 |
| **P2** | L1709 | 无 MCP Sampling 双向 |
| **P2** | L1710 | 无跨语言互操作（PyO3 / FFI） |

---

### 22.4 D12 可访问性 a11y / RTL / 屏幕阅读器（7 子维度）

#### 22.4.1 opencode a11y 实现总览

opencode 在 D12 属于「**强 a11y 派**」（与 claudecode / openclaw 并列），Web 端达 WCAG 2.2 AA/AAA 水平，成熟度 ⭐⭐⭐⭐。

| 子维度 | opencode 机制 | 代码定位 | 成熟度 |
|--------|--------------|---------|--------|
| **D12-1 屏幕阅读器** | ✅ Solid + aria-label + role + sr-only + Markdown alt | `message-part.tsx:25-80`、`sr-only.css:1-10`、`markdown.tsx:50-70` | ⭐⭐⭐⭐ |
| **D12-2 高对比度** | ✅ 30+ 主题 + system 派生 + prefers-contrast + daltonized 3 类 | `themes/index.ts:1-60`、`themes/system.ts` | ⭐⭐⭐⭐ |
| **D12-3 减动效** | ✅ 全局 `@media (prefers-reduced-motion)` + 完整降级链 | `styles/motion.css:1-50` | ⭐⭐⭐⭐ |
| **D12-4 RTL** | ✅ 5 locale RTL + logical CSS + bidi 控制字符 | `i18n/locales.ts:17-50`、`ChatLayout.tsx:50-80`、`bidi.ts:1-50` | ⭐⭐⭐ |
| **D12-5 多模态提示** | ✅ 5 档 toast priority（critical/error/warning/info/debug） | `services/toast.ts:30-60` | ⭐⭐⭐ |
| **D12-6 键盘可达性** | ✅ 焦点环 + Tab order + IME 双 setTimeout + 100+ 快捷键 | `prompt/index.tsx:1391-1395` | ⭐⭐⭐⭐ |
| **D12-7 字体/字号** | ✅ rem/em + CJK 回退 + npm/string-width | `themes/index.ts` | ⭐⭐⭐ |

#### 22.4.2 D12-1 屏幕阅读器：Solid + ARIA + sr-only

**核心文件**：`packages/ui/src/components/message-part.tsx:25-80`

```tsx
export function MessagePart(props: Part) {
  return (
    <div role="article"
         aria-labelledby={`part-${props.id}-title`}
         aria-describedby={`part-${props.id}-body`}>
      {props.type === "tool" && (
        <div role="group" aria-label={`Tool: ${props.tool.name}`}>
          <span id={`part-${props.id}-title`} className="sr-only">
            {`Tool execution: ${props.tool.name}`}
          </span>
          <div id={`part-${props.id}-body`}
               aria-live="polite"
               aria-busy={props.status === "running"}>
            ...
          </div>
        </div>
      )}
      {props.type === "text" && <Markdown altText={props.altText} />}
      {props.type === "file" && <FilePart aria-label={props.fileName} />}
    </div>
  );
}
```

**`sr-only` CSS 类**（`packages/ui/src/styles/sr-only.css:1-10`）：视觉隐藏但屏幕阅读器可读。

**Markdown alt 文本**（`packages/ui/src/components/markdown.tsx:50-70`）：`![Image description](./foo.png)` 解析，缺失时 `console.warn` 警告（仅警告未强制）。

#### 22.4.3 D12-2 高对比度主题：30+ 主题 + daltonized 3 类

**核心文件**：`packages/ui/src/themes/index.ts:1-60`

```typescript
export const builtInThemes = [
    "default", "light", "dark",
    "solarized-dark", "solarized-light",
    "monokai", "dracula", "nord", "one-dark", "one-light",
    "github-dark", "github-light",
    "ayu-dark", "ayu-light", "ayu-mirage",
    "tokyo-night", "catppuccin-mocha", "catppuccin-latte",
    "rose-pine", "rose-pine-dawn",
    "high-contrast", "high-contrast-light",  // AA+
    "high-contrast-dark", "high-contrast-more", // AAA
    "ansi", "ansi-light",
    "daltonized-deuteranopia", // 红绿色盲
    "daltonized-protanopia",   // 红色盲
    "daltonized-tritanopia",   // 蓝黄色盲
    "system", // 自动检测 prefers-color-scheme
] as const;
```

**system 主题**（`packages/ui/src/themes/system.ts`）：`window.matchMedia("(prefers-color-scheme: dark)")` 自动跟随操作系统。

#### 22.4.4 D12-3 减动效：`animation-duration: 0.01ms !important`

**核心文件**：`packages/ui/src/styles/motion.css:1-50`

```css
@media (prefers-reduced-motion: reduce) {
    *, *::before, *::after {
        animation-duration: 0.01ms !important;
        animation-iteration-count: 1 !important;
        transition-duration: 0.01ms !important;
        scroll-behavior: auto !important;
    }
}
```

**设计巧妙点**：把所有动效降到 1 微秒（视觉上看不到），但保留 `animationend` 事件触发（避免破坏组件生命周期）。

#### 22.4.5 D12-4 RTL：5 locale + bidi 控制字符

**核心文件**：`packages/web/src/i18n/locales.ts:17-50`

```typescript
export const rtlLocales = new Set(["ar", "fa", "he", "ur", "yi"]); // 5 个
```

**bidi 处理**（`packages/web/src/utils/bidi.ts:1-50`）：LRE/RLE/PDF Unicode 控制字符包裹阿拉伯文片段。

**不足**：bidi 控制字符硬编码，未用 `unicode-bidi: isolate` CSS 替代；**TUI 端 7 工程无一支持 RTL 渲染**。

#### 22.4.6 D12 laew gap 汇总

| 优先级 | 编号 | 描述 |
|--------|------|------|
| **P0** | L1761 | 无 ARIA 属性（role / aria-label / aria-live） |
| **P0** | L1762 | 无 sr-only CSS 类（屏幕阅读器专用） |
| **P0** | L1763 | 无高对比度主题（high-contrast / AAA） |
| **P0** | L1764 | 无减动效偏好检测（prefers-reduced-motion） |
| **P0** | L1765 | 无 daltonized 色盲友好主题 |
| **P1** | L1766 | 无 RTL 布局（dir="rtl" + bidi 处理） |
| **P1** | L1767 | 无 5 档 toast priority + aria-live 映射 |
| **P1** | L1768 | 无系统主题自动派生（prefers-color-scheme） |
| **P2** | L1769 | 无 CJK 字体回退链 |
| **P2** | L1770 | 无 emoji 宽度精细算法 |

---

### 22.5 D13 离线模式与本地优先（6 子维度）

> **状态**：D13 专题文档仍在生成中，本节基于已有调研摘要。

#### 22.5.1 opencode 离线机制

opencode 在 D13 属于「**强离线派**」（与 openclaw / pi 并列），但实际实现以 WebSocket 重连为主。

| 子维度 | opencode 机制 | 代码定位 | 成熟度 |
|--------|--------------|---------|--------|
| **D13-1 离线检测** | ⚠️ WebSocket 重连 | `websocket.ts` | ⭐⭐ |
| **D13-2 请求队列** | ⚠️ Effect DI 异步队列 | `queue.ts` | ⭐⭐ |
| **D13-3 本地缓存** | ⚠️ Durable Object 持久化 | `durable-object.ts` | ⭐⭐⭐ |
| **D13-4 队列持久化** | ⚠️ SQLite + Effect Schema | `db.ts` | ⭐⭐ |
| **D13-5 同步合并** | ⚠️ EventV2 重放 | `share-next.ts` | ⭐⭐⭐ |
| **D13-6 冲突解决** | ⚠️ Map LWW | `share-next.ts:97-110` | ⭐⭐ |

#### 22.5.2 D13 laew gap 汇总

| 优先级 | 编号 | 描述 |
|--------|------|------|
| **P0** | L1821 | 无离线检测（heartbeat / 网络状态监听） |
| **P0** | L1822 | 无请求队列持久化（断网排队 + 恢复重放） |
| **P0** | L1823 | 无本地缓存（SQLite / 文件缓存） |
| **P1** | L1824 | 无同步合并策略（LWW / CRDT） |
| **P1** | L1825 | 无冲突解决机制 |
| **P2** | L1826 | 无 Durable Object 持久化 |

---

### 22.6 D14 跨设备同步与会话漫游（5 子维度）

#### 22.6.1 opencode share-next 范本

opencode 在 D14 以 `share-next.ts` 的 EventV2 实时同步为**业界范本**之一（与 openclaw Ed25519 设备身份、claudecode Bridge 远程控制并列）。

| 子维度 | opencode 机制 | 代码定位 | 成熟度 |
|--------|--------------|---------|--------|
| **D14-1 设备发现** | 🟡 基于账号 token 鉴权（无独立设备身份） | `auth/index.ts` | ⭐⭐ |
| **D14-2 会话漫游** | 🟢 share-next.ts + steal 抢占 | `share/share-next.ts:1-371` | ⭐⭐⭐⭐ |
| **D14-3 同步协议** | 🟢 HTTP POST + EventV2 + 1 秒批 flush | `share-next.ts:124-146` | ⭐⭐⭐⭐ |
| **D14-4 加密与隐私** | 🟡 HTTPS + Bearer Token（无独立加密） | `share-next.ts:206-222` | ⭐⭐ |
| **D14-5 状态合并** | 🟢 Map-based union（LWW by key） | `share-next.ts:97-110` | ⭐⭐⭐⭐ |

#### 22.6.2 D14-2/3 share-next.ts 核心机制

**架构**：
```
Session Event → EventV2 Bridge → ShareNext.sync() → 1s 批 flush → POST /api/shares/{id}/sync
                                                              ↓
                                                    双 base URL 鉴权切换
                                                    (legacyApi / consoleApi)
```

**1 秒批 flush 机制**（`share-next.ts:124-146`）：

```typescript
function sync(sessionID: SessionID, data: Data[]) {
  return Effect.gen(function* () {
    if (disabled) return
    const share = yield* getCached(sessionID)
    if (!share) return
    const s = yield* InstanceState.get(state)
    const existing = s.queue.get(sessionID)
    if (existing) {
      for (const item of data) { existing.set(key(item), item) }
      return
    }
    const next = new Map(data.map((item) => [key(item), item]))
    s.queue.set(sessionID, next)
    yield* flush(sessionID).pipe(Effect.delay(1000), Effect.forkIn(s.scope))
  })
}
```

**范式要点**：
- **Map-based 去重**：同 key 后写覆盖前写（session/message/part/session_diff/model 五类）
- **1 秒延迟批处理**：`Effect.delay(1000)` 合并高频事件
- **forkIn 隔离**：每个 share 独立 Scope，失败不影响其他

**steal 会话抢占**（`handlers/sync.ts:61-70`）：
- **"steal" 语义**：将一个 workspace 的会话转移到另一个 workspace
- **strictOwner 校验**：`events.replayAll(payload, { ownerID, strictOwner: true })` 防越权
- **EventV2 重放**：`replayAll` 将事件流注入目标 workspace

**双 base URL 鉴权**（`share-next.ts:206-222`）：
- 未登录走 `opncd.ai`（legacyApi）
- 登录 enterprise console 走 `/api/shares` + Bearer + `x-org-id`（consoleApi）

#### 22.6.3 D14-5 Map-based union（LWW by key）

**核心文件**：`packages/opencode/src/share/share-next.ts:97-110`

```typescript
function key(item: Data) {
  switch (item.type) {
    case "session": return "session"
    case "message": return `message/${item.data.id}`
    case "part": return `part/${item.data.messageID}/${item.data.id}`
    case "session_diff": return "session_diff"
    case "model": return "model"
  }
}
```

**范式要点**：
- **稳定 key 函数**：每个 Data 类型有唯一 key 生成规则
- **Map 覆盖**：同 key 后写覆盖前写（LWW）
- **structuredClone**：`sync()` 前深拷贝防引用污染
- **replayAll strictOwner**：防跨 workspace 注入

#### 22.6.4 D14 laew gap 汇总

| 优先级 | 编号 | 描述 |
|--------|------|------|
| **P0** | L1891 | 无会话共享（EventV2 实时同步） |
| **P0** | L1892 | 无会话抢占/转移（steal 机制） |
| **P0** | L1881 | 无设备身份（Ed25519 密钥对 + 指纹） |
| **P0** | L1882 | 无设备配对审批流程（6 种方式） |
| **P1** | L1893 | 无 1 秒批 flush 事件合并 |
| **P1** | L1894 | 无 forkIn 隔离（每 share 独立 Scope） |
| **P1** | L1895 | 无 steal 会话抢占机制 |
| **P1** | L1883 | 无设备 Token 签发/轮换/撤销 |
| **P1** | L1923 | 无 Map-based union（LWW by key） |
| **P1** | L1926 | 无 structuredClone 深拷贝隔离 |
| **P1** | L1927 | 无 strictOwner 防跨 workspace 注入 |
| **P2** | L1898 | 无 1 秒批 flush 事件合并 |
| **P2** | L1899 | 无双 base URL 鉴权切换 |
| **P2** | L1884 | 无 Tailscale 集成 |

---

### 22.7 借鉴路线图（按 ROI 排序）

#### 第 1 步（P0 紧急，1-2 周）

| 编号 | 维度 | 描述 | 预估工时 |
|------|------|------|---------|
| L1599 | D9 | 凭证 0o600 + Keychain 存储 | 2 天 |
| L1600 | D9 | 应用层 AES-256-GCM 加密 | 3 天 |
| L1608 | D9 | 私有 IP 拦截（SSRF 基础） | 2 天 |
| L1592 | D9 | Prompt 注入 14 类正则检测 | 3 天 |
| L1603 | D9 | 工作目录信任模型 | 2 天 |
| L1641 | D10 | Diff 渲染器（`similar` crate） | 3 天 |
| L1642 | D10 | 语法高亮（`syntect` crate） | 3 天 |
| L1763 | D12 | 高对比度主题 4 主题 | 2 天 |
| L1764 | D12 | 减动效偏好检测 | 1 天 |
| L1821 | D13 | 离线检测（heartbeat） | 2 天 |
| L1891 | D14 | 会话导出（JSON / Markdown） | 2 天 |

#### 第 2 步（P1 重要，2-4 周）

| 编号 | 维度 | 描述 |
|------|------|------|
| L1596 | D9 | Bash 23 层检测器（tree-sitter-bash） |
| L1597 | D9 | FAIL-CLOSED AST 白名单 |
| L1609 | D9 | DNS 钉扎（trust-dns-resolver） |
| L1610 | D9 | 决策审计 3 段式 |
| L1612 | D9 | 双 pass scrub |
| L1643 | D10 | 数学公式渲染（KaTeX） |
| L1644 | D10 | 图片终端协议（sixel / kitty） |
| L1761 | D12 | ARIA 属性（role / aria-label / aria-live） |
| L1762 | D12 | sr-only CSS 类 |
| L1766 | D12 | RTL 布局（dir="rtl" + bidi） |
| L1893 | D14 | 1 秒批 flush 事件合并 |
| L1894 | D14 | forkIn 隔离 |

#### 第 3 步（P2 进阶，1-2 月）

| 编号 | 维度 | 描述 |
|------|------|------|
| L1606 | D9 | OS 级沙箱（Landlock/Seccomp/cgroup） |
| L1622 | D9 | Docker 容器沙箱 |
| L1628 | D9 | W3C traceparent 传播 |
| L1629 | D9 | OTel 安全事件导出 |
| L1701 | D11 | A2A Protocol 实现 |
| L1702 | D11 | ACP 实现 |
| L1881 | D14 | Ed25519 设备身份 |
| L1882 | D14 | 设备配对审批流程 |

---

### 22.8 关键文件路径汇总

| 维度 | 文件路径 | 核心职责 |
|------|---------|---------|
| **D9** | `packages/opencode/src/tool/shell.ts:311-336` | web-tree-sitter 双语法 Bash 检测 |
| **D9** | `packages/opencode/src/permission/arity.ts:24-161` | 150+ 命令元组 arity 字典 |
| **D9** | `packages/opencode/src/auth/index.ts:73-89` | auth.json + OAuth 状态机 |
| **D9** | `packages/opencode/src/redaction.ts:5-38` | 7 类正则脱敏 |
| **D9** | `packages/opencode/src/instance-context.ts:18-24` | 外部目录边界检测 |
| **D9** | `packages/opencode/src/webfetch.ts:35-37` | SSRF 仅协议校验 |
| **D10** | `packages/opencode/src/tui/component/diff-viewer.tsx` | Diff 渲染（700+ 行） |
| **D10** | `packages/opencode/src/tui/theme/syntax.ts` | SyntaxStyle 30+ 颜色字段 |
| **D12** | `packages/ui/src/components/message-part.tsx:25-80` | Solid + ARIA + sr-only |
| **D12** | `packages/ui/src/styles/sr-only.css:1-10` | 屏幕阅读器专用 CSS |
| **D12** | `packages/ui/src/themes/index.ts:1-60` | 30+ 内置主题 + daltonized |
| **D12** | `packages/ui/src/styles/motion.css:1-50` | prefers-reduced-motion 降级 |
| **D12** | `packages/web/src/i18n/locales.ts:17-50` | 5 locale RTL |
| **D12** | `packages/web/src/utils/bidi.ts:1-50` | bidi 控制字符包裹 |
| **D12** | `packages/ui/src/services/toast.ts:30-60` | 5 档 toast priority |
| **D14** | `packages/opencode/src/share/share-next.ts:1-371` | EventV2 实时同步 + steal + Map LWW |
| **D14** | `packages/opencode/src/share/share-next.ts:97-110` | 稳定 key 函数（LWW by key） |
| **D14** | `packages/opencode/src/share/share-next.ts:124-146` | 1 秒批 flush 机制 |
| **D14** | `packages/opencode/src/share/share-next.ts:206-222` | 双 base URL 鉴权切换 |

---

### 22.9 本轮不重复声明

本轮**不重复**前 18 轮已覆盖的以下内容：
- D1-D8 用户交互体验层（@提及 / 自定义命令 / Rewind / 文件监视 / 富文本渲染 / 输入体验 / Onboarding / 会话导出）—— 详见第 21 章
- 协议 wire / SSE / 工具抽象 / Skill 系统 / Hook / TUI 渲染管线 / OAuth / i18n / Release / WebSocket / CRDT / Telemetry / 多租户 / RRF / LLM 网关 / Pregel / Agent 池 / Turn 锁 / Bash 检测（第十二轮）/ Session 持久化 / 内存加密 / SQLite 全栈 / 反应式 IoC / 守护进程基础设施 / 录制回放

本轮**新增**聚焦：D9 安全纵深（STRIDE / Prompt 注入 / Bash 检测 / 凭证 / 路径信任 / 沙箱 / SSRF / 审计）/ D10 多模态输出 / D11 A2A 协议 / D12 可访问性 a11y / D13 离线模式 / D14 跨设备同步。

详细源码引用、机制剖析、设计巧妙点、完整 gap 表见专题文档：
- `专题/专题-第十九轮-安全与威胁模型深度对比.md`（1582 行 / 8 维度 / 55 gap）
- `专题/专题-第十九轮-A2A协议与多Agent互操作深度对比.md`（1355 行 / 7 维度 / 60 gap）
- `专题/专题-第十九轮-可访问性a11y与RTL深度对比.md`（1200+ 行 / 7 维度 / 60 gap）
- `专题/专题-第十九轮-跨设备同步与会话漫游深度对比.md`（1149 行 / 5 维度 / 50 gap）
- `专题/专题-第十九轮-跨项目缺口分析.md`（800+ 行 / 6 维度综合）

---

**第十轮深挖结束。**