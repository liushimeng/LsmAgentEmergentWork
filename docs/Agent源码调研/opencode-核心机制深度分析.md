# opencode 核心机制深度分析（第二轮）

- **分析日期**：2026-09-03
- **源码根路径**：`/usr/local/LsmGitOpenSource/opencode`
- **核心文件数**：约 50+ 文件（packages/opencode/src + packages/core/src）
- **前置文档**：`opencode-源码调研.md`（目录级概览）、`opencode-深度分析.md`（第一轮）
- **本文定位**：第二轮深入钻取，聚焦 6 个核心机制的源码级实现细节

---

## 专题 1：Effect + Schema 全栈 DI 架构

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/core/src/effect/layer-node.ts` | LayerNode 依赖图定义与编译 |
| `packages/core/src/effect/app-node-builder.ts` | AppNodeBuilder 编排器 |
| `packages/core/src/effect/service-use.ts` | `serviceUse()` 快捷访问辅助 |
| `packages/opencode/src/effect/app-runtime.ts` | AppLayer 总装配（60+ 节点） |
| `packages/opencode/src/effect/app-node-builder-v1.ts` | V1 版编排器 |
| `packages/opencode/src/effect/instance-state.ts` | InstanceState 实例级状态隔离 |

### 1.1 Service 定义模式

全项目共 78 个 `Context.Service`，统一采用如下三段式：

```typescript
// 1. 定义接口
export interface Interface {
  readonly get: (agent: string) => Effect.Effect<Info>
  readonly list: () => Effect.Effect<Info[]>
}

// 2. 创建 Service Tag（字符串 Key + 接口绑定）
export class Service extends Context.Service<Service, Interface>()("@opencode/Agent") {}

// 3. 实现 Layer + 注册依赖
const layer = Layer.effect(Service, Effect.gen(function* () {
  const config = yield* Config.Service   // 声明依赖
  // ... 实现逻辑
  return Service.of({ get, list })
}))

// 4. 导出 LayerNode（依赖图节点）
export const node = LayerNode.make({
  service: Service,
  layer: layer,
  deps: [Config.node, Auth.node, ...],
})
```

### 1.2 LayerNode 依赖图系统

`LayerNode` 是 opencode 自建的依赖图抽象，核心数据结构：

```typescript
// packages/core/src/effect/layer-node.ts
export interface Node<A, E = never, T extends Tag | undefined = undefined> {
  readonly kind: "layer" | "unbound" | "group"
  readonly name: string
  readonly service?: Context.Service.Any
  readonly implementation?: Layer.Any
  readonly dependencies: readonly AnyNode[]
  readonly tag?: T
}
```

三种节点类型：
- **layer**：完整的服务实现（含 Layer + 依赖列表）
- **unbound**：只声明接口，不提供实现（用于环境注入）
- **group**：组合节点，将多个节点聚合

编译过程 `LayerNode.compile()` 通过拓扑排序将依赖图展开为单个 Effect Layer：

```typescript
export function compile<A, E>(root: Node<A, E>, replacements?: Replacements): Layer.Layer<A, E> {
  const layers = flatten(root).map((node) => compileNode(node))
  return layers.reduce((result, layer) => layer.pipe(Layer.provideMerge(result)), Layer.empty)
}
```

### 1.3 AppLayer 总装配

`app-runtime.ts` 中 `AppLayer` 将 60+ 个服务节点组装为一个完整的 Layer：

```typescript
// packages/opencode/src/effect/app-runtime.ts
export const AppLayer = AppNodeBuilderV1.build(
  LayerNode.group([
    Npm.node, FSUtil.node, Database.node, Auth.node,
    Account.node, Config.node, Git.node, Storage.node,
    Snapshot.node, Plugin.node, ModelsDev.node,
    Provider.node, ProviderAuth.node, Agent.node,
    Skill.node, Discovery.node, Question.node,
    Permission.node, Todo.node, Session.node,
    SessionProjector.node, SessionStatus.node,
    BackgroundJob.node, RuntimeFlags.node,
    EventV2Bridge.node, SessionRunState.node,
    SessionProcessor.node, SessionCompaction.node,
    SessionRevert.node, SessionSummary.node,
    SessionPrompt.node, Instruction.node, LLM.node,
    LSP.node, MCP.node, McpAuth.node, Command.node,
    Truncate.node, ToolRegistry.node, Format.node,
    InstanceStore.node, Project.node, Vcs.node,
    Workspace.node, Worktree.node, Installation.node,
    ShareNext.node, SessionShare.node,
  ]),
)
```

`LayerNode.group()` 接受一个节点数组，自动解析依赖关系并编译为 Effect `Layer`。每个 `node` 声明了自己的 `deps`，编译器通过 `walk()` 函数做拓扑遍历，检测循环依赖。

**编译过程详解**：`LayerNode.compile()` 将依赖图展开：

```typescript
// packages/core/src/effect/layer-node.ts
export function compile<A, E>(root: Node<A, E>, replacements?): Layer.Layer<A, E> {
  const cache = new Map<AnyNode, RuntimeLayer>()
  const compileNode = (node: AnyNode) =>
    walk<RuntimeLayer>(node, (node, context) => {
      if (node.kind === "unbound") throw new Error(`Unbound layer node: ${node.name}`)
      const dependencies = node.dependencies.flatMap(flatten).map(context.visit)
      const implementation = node.implementation!
      // 无依赖直接返回；有依赖则 provide 注入
      return dependencies.length === 0
        ? implementation
        : implementation.pipe(Layer.provide(dependencies))
    }, { cache })
  // 将所有节点层合并为一个
  const layers = flatten(root).map(compileNode)
  return layers.reduce((result, layer) => layer.pipe(Layer.provideMerge(result)), Layer.empty)
}
```

**循环检测**：`walk()` 使用 visiting 集合 + stack 实现 DFS 循环检测：

```typescript
function walk<Result>(root, visit, options = {}) {
  const visiting = new Set<AnyNode>()
  const stack: AnyNode[] = []
  const recur = (node): Result => {
    if (options.detectCycles !== false && visiting.has(target)) {
      const start = stack.indexOf(target)
      throw new Error(
        `Cycle detected: ${[...stack.slice(start), target].map(n => n.name).join(" -> ")}`
      )
    }
    visiting.add(target); stack.push(target)
    try { return visit(target, { cache, visit: recur }) }
    finally { stack.pop(); visiting.delete(target) }
  }
}
```

**替换机制**：`compile()` 支持 `replacements` 参数，用于测试时替换特定服务实现：

```typescript
// 测试时替换 Database 为 mock
const testLayer = LayerNode.compile(AppLayer, [[Database.node, mockDatabaseLayer]])
```

### 1.4 Schema 验证系统

**工具参数验证**：每个工具通过 `Tool.define()` 声明 `Schema.Decoder` 类型的 `parameters`：

```typescript
// packages/opencode/src/tool/tool.ts
export interface Def<Parameters extends Schema.Decoder<unknown> = Schema.Decoder<unknown>> {
  id: string
  description: string
  parameters: Parameters
  jsonSchema?: JSONSchema7
  execute(args: Schema.Schema.Type<Parameters>, ctx: Context): Effect.Effect<ExecuteResult>
}
```

执行时自动解码：

```typescript
// Tool.define() 内部包装
const decode = Schema.decodeUnknownEffect(toolInfo.parameters)
toolInfo.execute = (args, ctx) => Effect.gen(function* () {
  const decoded = yield* decode(args).pipe(
    Effect.mapError((error) => new InvalidArgumentsError({ tool: id, detail: String(error) }))
  )
  return yield* execute(decoded, ctx)
})
```

**错误类型**：使用 `Schema.TaggedErrorClass` 定义结构化错误：

```typescript
export class InvalidArgumentsError extends Schema.TaggedErrorClass<InvalidArgumentsError>()(
  "ToolInvalidArgumentsError",
  { tool: Schema.String, detail: Schema.String }
) {
  override get message() {
    return `The ${this.tool} tool was called with invalid arguments: ${this.detail}.`
  }
}
```

**消息/配置/权限 Schema**：`SessionV1.Info`、`ConfigV1.Info`、`PermissionV1.Ruleset` 等全部使用 Effect Schema 定义，贯穿存储层（SQLite Drizzle）、传输层（API）、展示层。

### 1.5 InstanceState 实例隔离

`InstanceState` 是 opencode 独创的状态管理机制，为每个"实例"（工作目录）创建独立状态：

```typescript
const state = yield* InstanceState.make<State>(
  Effect.fn("Agent.state")(function* (ctx) {
    // ctx.directory = 当前工作目录
    // 返回该目录下的状态
    return { get, list, defaultInfo, defaultAgent }
  }),
)
```

这意味着同一进程中可以处理多个工作目录，每个目录有独立的 Agent 配置、MCP 连接、工具列表。

**InstanceState 使用模式**：状态通过 `useEffect` 访问，确保实例已初始化：

```typescript
// 访问状态的包装器
return Service.of({
  get: Effect.fn("Agent.get")(function* (agent: string) {
    return yield* InstanceState.useEffect(state, (s) => s.get(agent))
  }),
  list: Effect.fn("Agent.list")(function* () {
    return yield* InstanceState.useEffect(state, (s) => s.list())
  }),
})
```

### 1.6 插件工具系统

`ToolRegistry` 支持三种工具来源：

```typescript
// 1. 内置工具：ShellTool, ReadTool, EditTool, WriteTool, GlobTool, GrepTool 等
const builtin = [tool.shell, tool.read, tool.glob, tool.grep, tool.edit, tool.write, ...]

// 2. 文件系统工具：从 .opencode/tool/ 目录动态加载
const matches = dirs.flatMap((dir) =>
  Glob.scanSync("{tool,tools}/*.{js,ts}", { cwd: dir, absolute: true })
)
for (const match of matches) {
  const mod = await import(pathToFileURL(match).href)
  for (const [id, def] of Object.entries(mod)) {
    if (isPluginTool(def)) custom.push(fromPlugin(id, def))
  }
}

// 3. 插件工具：通过 Plugin.Service 注册
const plugins = yield* plugin.list()
for (const p of plugins) {
  for (const [id, def] of Object.entries(p.tool ?? {})) {
    custom.push(fromPlugin(id, def))
  }
}
```

**插件工具适配**：插件工具使用 Zod schema 而非 Effect Schema，需要在注册时转换：

```typescript
function fromPlugin(id: string, def: ToolDefinition): Tool.Def {
  const zodParams = z.object(def.args)  // Zod schema
  const jsonSchema = zodJsonSchema(zodParams)  // 转为 JSON Schema
  const parameters = Schema.declare((u) => zodParams.safeParse(u).success)
  return {
    id, parameters, jsonSchema,
    execute: (args, toolCtx) => Effect.gen(function* () {
      const result = yield* Effect.promise(() => def.execute(args, pluginCtx))
      return { title, output, metadata, attachments }
    }),
  }
}
```

### 对 laew 的借鉴价值

1. **LayerNode 依赖图**值得借鉴：laew 可以用 Rust trait object + 类似依赖注入框架，让服务依赖关系在编译期可验证
2. **InstanceState 隔离**思路可借鉴：laew 的多 Session 可以做类似的实例级状态隔离
3. **Schema 驱动的参数验证**：laew 已有 Tool trait，可以在 execute 前加参数 Schema 校验
4. **`Schema.TaggedErrorClass`** 模式值得借鉴：结构化错误类型让错误处理更精确

---

## 专题 2：doom_loop 防死循环机制

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/opencode/src/session/processor.ts` | doom_loop 检测主逻辑 |
| `packages/opencode/src/agent/agent.ts` | 默认权限配置（`doom_loop: "ask"`） |
| `packages/opencode/src/permission/index.ts` | 权限请求/审批机制 |
| `packages/opencode/src/cli/cmd/run/demo.ts` | CLI 端 doom_loop 权限处理 |
| `packages/opencode/src/cli/cmd/run/permission.shared.ts` | 权限 UI 共享逻辑 |

### 2.1 检测算法

检测逻辑在 `SessionProcessor` 的 `handleEvent` 函数中，位于 `"tool-call"` 事件分支：

```typescript
// packages/opencode/src/session/processor.ts:29
const DOOM_LOOP_THRESHOLD = 3

// 在 tool-call 事件处理中（第 353-379 行）：
case "tool-call": {
  // ... 创建/更新 tool call ...
  
  // 取当前 assistant 消息的最近 N 个 parts
  const parts = yield* MessageV2.parts(ctx.assistantMessage.id)
  const recentParts = parts.slice(-DOOM_LOOP_THRESHOLD)

  // 判断是否满足 doom_loop 条件
  if (
    recentParts.length !== DOOM_LOOP_THRESHOLD ||
    !recentParts.every(
      (part) =>
        part.type === "tool" &&
        part.tool === value.name &&              // 同一个工具
        part.state.status !== "pending" &&
        JSON.stringify(part.state.input) === JSON.stringify(input),  // 相同参数
    )
  ) {
    return  // 不触发，正常继续
  }

  // 触发 doom_loop 权限检查
  const agent = yield* agents.get(ctx.assistantMessage.agent)
  yield* permission.ask({
    permission: "doom_loop",
    patterns: [value.name],
    sessionID: ctx.assistantMessage.sessionID,
    metadata: { tool: value.name, input },
    always: [value.name],
    ruleset: agent.permission,
  })
}
```

### 2.2 检测条件详解

触发条件同时满足以下 4 项：
1. **连续性**：最近 3 个 tool part 必须连续（在同一个 assistant 消息内）
2. **同工具**：3 次调用的 `part.tool` 完全相同
3. **同参数**：3 次调用的 `part.state.input` 通过 `JSON.stringify` 严格相等
4. **非 pending**：3 次调用的状态都不是 `"pending"`（即都已开始执行）

**不检测的内容**：
- 不检测跨消息的重复（只看当前 assistant 消息内的 parts）
- 不检测相似参数（严格 JSON 字符串比较）
- 不检测时间频率
- 不检测"无进展"（只检测精确重复）

### 2.3 触发后的处理

doom_loop 被视为一种 **权限请求**（`permission: "doom_loop"`），走标准权限流程：

```typescript
// permission/index.ts 中 ask() 函数
const ask = Effect.fn("Permission.ask")(function* (input) {
  // 1. 先查权限规则表
  for (const pattern of request.patterns) {
    const rule = evaluate(request.permission, pattern, ruleset, approved)
    if (rule.action === "deny") throw new PermissionV1.DeniedError(...)
    if (rule.action === "allow") continue  // 已批准过，直接放行
    needsAsk = true
  }
  
  // 2. 需要询问用户
  const deferred = yield* Deferred.make<void, PermissionV1.RejectedError>()
  pending.set(id, { info, deferred })
  yield* events.publish(Event.Asked, info)  // 通知 UI 显示权限请求
  return yield* Deferred.await(deferred)    // 等待用户响应
})
```

用户看到权限提示后的选择：
- **允许（allow）**：后续同工具同参数调用直接放行（写入 `approved` 列表）
- **拒绝（deny）**：抛出 `RejectedError`，在 `failToolCall` 中标记为 error，设置 `ctx.blocked = true`，循环终止

### 2.4 默认权限配置

```typescript
// packages/opencode/src/agent/agent.ts:121
const defaults = Permission.fromConfig({
  "*": "allow",
  doom_loop: "ask",  // doom_loop 默认需要询问用户
  // ...
})
```

所有 Agent 继承此默认值，`doom_loop` 权限被设置为 `"ask"`，即总是弹出询问。

### 2.5 与主循环的集成

doom_loop 检测嵌入在 `processor.ts` 的 `handleEvent` 中，是 LLM 流式事件处理的一部分。它不中断 LLM 流，而是在 tool-call 事件时同步检查。如果触发 doom_loop 且用户拒绝，`ctx.blocked` 被设为 `true`，`process()` 返回 `"stop"`，外层 `loop()` 据此终止。

### 对 laew 的借鉴价值

1. **精确重复检测**思路可直接借鉴：laew 的 SubAgent-Work 可以在工具调用前检查最近 N 次是否完全重复
2. **阈值 3 次**是合理默认值，laew 可采用类似策略
3. **作为权限请求而非硬中断**是优秀设计：laew 可以在 Yolo Agent 中增加类似的"重复检测"分类，给用户选择权
4. **局限性**：当前只检测 JSON 字符串严格相等，不覆盖"语义相似但参数略变"的情况；laew 可以进一步做参数相似度检测

---

## 专题 3：Context 管理 —— prune + compaction

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/opencode/src/session/compaction.ts` | compaction 主逻辑（prune + select + summarize） |
| `packages/opencode/src/session/overflow.ts` | 溢出检测 + 可用窗口计算 |
| `packages/opencode/src/session/message-v2.ts` | 消息序列化 + compaction part 处理 |
| `packages/opencode/src/session/processor.ts` | step-finish 时触发溢出检测 |
| `packages/opencode/src/session/summary.ts` | 每轮摘要生成 |
| `packages/core/src/util/token.ts` | token 估算（字符数/4） |

### 3.1 溢出检测

```typescript
// packages/opencode/src/session/overflow.ts
const COMPACTION_BUFFER = 20_000

export function usable(input: { cfg; model; outputTokenMax? }) {
  const reserved = input.cfg.compaction?.reserved ??
    Math.min(COMPACTION_BUFFER, ProviderTransform.maxOutputTokens(input.model, input.outputTokenMax))
  return input.model.limit.input
    ? Math.max(0, input.model.limit.input - reserved)
    : Math.max(0, context - ProviderTransform.maxOutputTokens(input.model, input.outputTokenMax))
}

export function isOverflow(input) {
  if (input.cfg.compaction?.auto === false) return false
  const count = input.tokens.total || input.tokens.input + output + cache.read + cache.write
  return count >= usable(input)
}
```

**关键公式**：`可用窗口 = 模型上下文窗口 - max(输出token上限, 20000保留缓冲)`

在 `processor.ts` 的 `step-finish` 事件中触发：

```typescript
case "step-finish": {
  // ... usage 统计 ...
  if (!ctx.assistantMessage.summary && isOverflow({ cfg, tokens: usage.tokens, model })) {
    ctx.needsCompaction = true  // 标记需要 compaction
  }
}
```

流处理使用 `Stream.takeUntil(() => ctx.needsCompaction)` 在检测到溢出时停止当前流。

### 3.2 Prune 策略

Prune 是**工具输出裁剪**，在 compaction 之前执行，目标是释放已过时的工具输出空间：

```typescript
// packages/opencode/src/session/compaction.ts
export const PRUNE_MINIMUM = 20_000   // 至少裁剪 20k tokens 才值得执行
export const PRUNE_PROTECT = 40_000   // 保护最近 40k tokens 的工具输出
const PRUNE_PROTECTED_TOOLS = ["skill"]  // skill 工具永不裁剪

const prune = Effect.fn("SessionCompaction.prune")(function* (input) {
  const msgs = yield* session.messages({ sessionID: input.sessionID })
  let total = 0, pruned = 0
  const toPrune: SessionV1.ToolPart[] = []
  let turns = 0

  // 从最新消息向前遍历
  loop: for (let msgIndex = msgs.length - 1; msgIndex >= 0; msgIndex--) {
    const msg = msgs[msgIndex]
    if (msg.info.role === "user") turns++
    if (turns < 2) continue  // 保护最近 2 轮对话
    if (msg.info.role === "assistant" && msg.info.summary) break loop  // 到达上一次 compaction 边界

    for (let partIndex = msg.parts.length - 1; partIndex >= 0; partIndex--) {
      const part = msg.parts[partIndex]
      if (part.type !== "tool" || part.state.status !== "completed") continue
      if (PRUNE_PROTECTED_TOOLS.includes(part.tool)) continue
      if (part.state.time.compacted) break loop  // 已经被裁剪过
      
      const estimate = Token.estimate(part.state.output)
      total += estimate
      if (total <= PRUNE_PROTECT) continue  // 还在保护区内
      pruned += estimate
      toPrune.push(part)
    }
  }

  // 只有裁剪量超过阈值才执行
  if (pruned > PRUNE_MINIMUM) {
    for (const part of toPrune) {
      part.state.time.compacted = Date.now()  // 标记为已裁剪
      yield* session.updatePart(part)
    }
  }
})
```

**Prune 的实际效果**：不是删除工具输出，而是在 `part.state.time.compacted` 上标记时间戳。后续 `serialize()` 函数读到此标记时，输出变为 `"[Old tool result content cleared]"`。

### 3.3 Compaction 策略

Compaction 是**对话压缩**，使用一个专用 Agent 生成摘要：

```typescript
// select() 函数：将消息分为 head（要压缩的）和 tail（要保留的）
const select = Effect.fn("SessionCompaction.select")(function* (input) {
  const budget = preserveRecentBudget({ cfg, model })  // 2k~15k tokens
  const all = turns(input.messages)  // 按 user 消息分轮

  let total = 0, keep: Tail | undefined
  for (let i = recent.length - 1; i >= 0; i--) {
    const turn = recent[i]
    const size = yield* estimate({ messages: turn 区间, model })
    if (total + size <= budget) {
      total += size
      keep = { start: turn.start, id: turn.id }
      continue
    }
    // 当前轮放不下，尝试拆分
    const split = yield* splitTurn({ messages, turn, model, budget: budget - total })
    if (split) keep = split
    break
  }

  return {
    head: messages.slice(0, keep.start),  // 要压缩的部分
    tail_start_id: keep.id,               // 保留部分的起始 ID
  }
})
```

**preserveRecentBudget** 计算：
```typescript
function preserveRecentBudget(input) {
  return input.cfg.compaction?.preserve_recent_tokens ??
    Math.min(15_000, Math.max(2_000, Math.floor(usable(input) * 0.25)))
}
```

**默认保留量**：可用窗口的 25%，最少 2k，最多 15k tokens。

### 3.4 Compaction 执行流程

```typescript
const processCompaction = Effect.fn("SessionCompaction.process")(function* (input) {
  // 1. 使用 "compaction" 专用 Agent
  const agent = yield* agents.get("compaction")
  
  // 2. 序列化历史消息
  const conversation = selected.head.map(serialize).filter(Boolean).join("\n\n")
  
  // 3. 调用 LLM 生成摘要
  const processor = yield* processors.create({ assistantMessage: msg, sessionID, model })
  const result = yield* processor.process({
    agent,
    tools: {},  // compaction agent 无工具
    messages: [{ role: "user", content: nextPrompt }],
    model,
  })

  // 4. 如果压缩成功且配置了 auto-continue
  if (result === "continue" && input.auto) {
    // 自动插入 "Continue if you have next steps" 消息
    yield* session.updatePart({
      type: "text",
      text: "Continue if you have next steps...",
      synthetic: true,
    })
  }
})
```

### 3.5 Token 估算

```typescript
// packages/core/src/util/token.ts
const CHARS_PER_TOKEN = 4
export const estimate = (input: string) => Math.max(0, Math.round(input.length / CHARS_PER_TOKEN))
```

使用简单的字符数/4 估算，不依赖 tiktoken 等库。

### 3.6 消息序列化与裁剪

`serialize()` 函数将消息转为文本摘要，工具输出限制为 2000 字符：

```typescript
const TOOL_OUTPUT_MAX_CHARS = 2_000
const truncate = (value: string) =>
  value.length <= TOOL_OUTPUT_MAX_CHARS ? value : `${value.slice(0, TOOL_OUTPUT_MAX_CHARS)}\n[truncated]`
```

序列化格式示例：
```
[User]: 请帮我修复这个 bug
[Assistant]: 我来查看代码
[Assistant tool call]: read({"path":"src/main.ts"})
[Tool result]: console.log("hello")...
[Assistant]: 问题在于...
```

### 3.7 Compaction Part 与 Tail 保留

Compaction 在消息中插入特殊 part 标记：

```typescript
// session/compaction.ts create()
yield* session.updatePart({
  id: PartID.ascending(),
  messageID: msg.id,
  sessionID: msg.sessionID,
  type: "compaction",    // 特殊 part 类型
  auto: input.auto,      // 是否自动触发
  overflow: input.overflow,  // 是否因溢出触发
  tail_start_id: selected.tail_start_id,  // 保留部分的起始消息 ID
})
```

**Tail 保留机制**：`select()` 将消息分为 head（压缩）和 tail（保留）。`tail_start_id` 记录保留边界。后续 `toModelMessages()` 读取时，会将 tail 部分按原始格式保留，只压缩 head 部分：

```typescript
// message-v2.ts 中 toModelMessages 的 compaction 处理
const compactionIndex = result.findLastIndex(
  (msg) => msg.parts.some((item) => item.type === "compaction" && item.tail_start_id !== undefined),
)
const part = compaction?.parts.find(
  (item) => item.type === "compaction" && item.tail_start_id !== undefined,
)
// 重新排列：[compaction-user, summary, ...retained tail..., continue-user]
if (tailIndex >= 0 && tailIndex < compactionIndex && summaryIndex > compactionIndex) {
  result = [
    ...result.slice(compactionIndex, summaryIndex + 1),  // compaction 摘要
    ...result.slice(tailIndex, compactionIndex),          // 保留的 tail
  ]
}
```

### 3.8 自动 Continue 机制

Compaction 成功后，如果配置了 `auto: true`，会自动插入一条继续消息：

```typescript
if (result === "continue" && input.auto) {
  const continueMsg = yield* session.updateMessage({ role: "user", sessionID })
  const text = (input.overflow
    ? "The previous request exceeded the provider's size limit due to large media attachments..."
    : "") + "Continue if you have next steps, or stop and ask for clarification..."
  yield* session.updatePart({
    type: "text",
    synthetic: true,  // 标记为合成消息
    metadata: { compaction_continue: true },
    text,
  })
}
```

这样 LLM 会在压缩后的上下文中继续工作，用户无感知。

### 3.9 溢出时的 replay 机制

当溢出发生在用户消息（含大文件附件）时，compaction 会将最近一条用户消息"replay"出来：

```typescript
if (input.overflow) {
  // 找到最近的非 compaction 用户消息
  for (let i = idx - 1; i >= 0; i--) {
    const msg = input.messages[i]
    if (msg.info.role === "user" && !msg.parts.some((p) => p.type === "compaction")) {
      replay = { info: msg.info, parts: msg.parts }
      messages = input.messages.slice(0, i)  // 截断到该消息之前
      break
    }
  }
}

// compaction 成功后，将 replay 消息重新插入
if (replay) {
  const replayMsg = yield* session.updateMessage({ role: "user", ... })
  for (const part of replay.parts) {
    // 大文件附件替换为文本描述
    const replayPart = part.type === "file" && MessageV2.isMedia(part.mime)
      ? { type: "text", text: `[Attached ${part.mime}: ${part.filename}]` }
      : part
    yield* session.updatePart({ ...replayPart, id: PartID.ascending() })
  }
}
```

### 对 laew 的借鉴价值

1. **Prune 策略**值得借鉴：laew 的 SubAgent-Work 可以在工具输出标记"已过时"，后续压缩时替换为 `[Old tool result content cleared]`
2. **保护最近 N 轮**的思路：laew 可以在 SessionContext 摘要生成时，保护最近 2-3 轮对话不被压缩
3. **专用 compaction Agent**是好设计：laew 可以新增一个隐藏的"压缩"角色，专门负责摘要生成
4. **token 估算的简单性**：laew 已有类似策略，字符数/token 比 4:1 是合理默认值
5. **PRUNE_PROTECT/PRUNE_MINIMUM 双阈值**值得借鉴：避免频繁的小量裁剪

---

## 专题 4：MCP 架构（3 传输 + Catalog）

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/opencode/src/mcp/index.ts` | MCP Service 主逻辑（连接、状态、工具/资源/提示） |
| `packages/opencode/src/mcp/catalog.ts` | MCP Catalog：工具列表、分页、转换 |
| `packages/opencode/src/mcp/auth.ts` | MCP OAuth 认证 |
| `packages/opencode/src/mcp/oauth-provider.ts` | OAuth Provider 实现 |
| `packages/opencode/src/mcp/oauth-callback.ts` | OAuth 回调服务器 |
| `packages/opencode/src/mcp/browser.ts` | 浏览器打开（OAuth 授权页） |
| `packages/opencode/src/session/tools.ts` | MCP 工具集成到 session |

### 4.1 三种传输方式

```typescript
// packages/opencode/src/mcp/index.ts:212
type Transport = StdioClientTransport | StreamableHTTPClientTransport | SSEClientTransport
```

| 传输方式 | 适用场景 | 代码位置 |
|---------|---------|---------|
| **StdioClientTransport** | 本地 MCP server（通过子进程通信） | `connectLocal()` |
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

### 4.2 Catalog 概念

`McpCatalog` 是 MCP 工具/资源/提示的注册中心，核心功能：

```typescript
// packages/opencode/src/mcp/catalog.ts

// 工具名标准化：serverName_toolName
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

### 4.3 MCP Service 状态管理

```typescript
interface State {
  config: Record<string, ConfigMCPV1.Info>   // 运行时配置
  status: Record<string, Status>              // 连接状态
  clients: Record<string, MCPClient>          // 客户端实例
  defs: Record<string, MCPToolDef[]>          // 工具定义缓存
  instructions: Record<string, string>        // 服务器指令缓存
}

// 5 种状态
export const Status = Schema.Union([
  StatusConnected,    // "connected"
  StatusDisabled,     // "disabled"
  StatusFailed,       // "failed"
  StatusNeedsAuth,    // "needs_auth"
  StatusNeedsClientRegistration,  // "needs_client_registration"
])
```

### 4.4 服务器生命周期管理

**启动**：`InstanceState.make()` 在初始化时并发连接所有配置的 MCP server：

```typescript
yield* Effect.forEach(
  Object.entries(config),
  ([key, mcp]) => Effect.gen(function* () {
    const result = yield* create(key, mcp)
    s.status[key] = result.status
    if (result.mcpClient) {
      s.clients[key] = result.mcpClient
      s.defs[key] = result.defs!
      watch(s, key, result.mcpClient, bridge, mcp.timeout)  // 注册监听器
    }
  }),
  { concurrency: "unbounded" },
)
```

**重连/状态监听**：`watch()` 函数监听连接关闭和工具列表变更：

```typescript
function watch(s: State, name: string, client: MCPClient, bridge, timeout?) {
  client.onclose = () => {
    s.status[name] = { status: "failed", error: "Connection closed" }
    // 通知工具列表变更
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

**关闭**：`Effect.addFinalizer` 注册清理函数，关闭所有客户端并杀死子进程：

```typescript
yield* Effect.addFinalizer(() =>
  Effect.gen(function* () {
    // 杀死 stdio 传输的子进程及所有后代
    for (const client of clients) {
      const pid = client.transport instanceof StdioClientTransport ? client.transport.pid : null
      if (typeof pid === "number") {
        const pids = yield* descendants(pid)  // pgrep -P 递归查找
        for (const dpid of pids) process.kill(dpid, "SIGTERM")
      }
      yield* Effect.tryPromise(() => client.close())
    }
  }),
)
```

### 4.5 MCP 工具集成到 Agent

MCP 工具通过 `ToolRegistry` 和 `SessionTools.resolve()` 两层集成：

```typescript
// session/tools.ts 中 resolve()
for (const item of yield* registry.tools({ modelID, providerID, agent })) {
  tools[item.id] = tool({
    description: item.description,
    inputSchema: jsonSchema(schema),
    execute(args, options) {
      return run.promise(Effect.gen(function* () {
        const result = yield* item.execute(args, ctx)
        return output
      }))
    },
  })
}

// 另外还注册了 MCP Resource 工具
tools["list_mcp_resources"] = tool({ ... })
tools["list_mcp_resource_templates"] = tool({ ... })
tools["read_mcp_resource"] = tool({ ... })
```

**MCP 工具在 registry 中的位置**：MCP 工具不进入 `builtin` 列表，而是在 `SessionTools.resolve()` 时动态合并。`MCP.tools()` 返回 `Record<string, McpTool>`，每个 `McpTool` 包含原始定义、客户端引用和超时配置：

```typescript
// mcp/index.ts
const tools = Effect.fn("MCP.tools")(function* () {
  const result: Record<string, McpTool> = {}
  for (const [clientName, client] of Object.entries(s.clients)) {
    if (s.status[clientName]?.status !== "connected") continue
    for (const def of listed) {
      result[McpCatalog.toolName(clientName, def.name)] = { def, client, timeout }
    }
  }
  return result
})
```

### 4.6 MCP Server 指令注入

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

### 4.6 OAuth 认证流

远程 MCP server 支持完整的 OAuth 2.0 流程：
1. `startAuth()` → 发起 OAuth 授权请求，捕获 authorization URL
2. `authenticate()` → 打开浏览器让用户授权，等待回调
3. `finishAuth()` → 用 authorization code 获取 token
4. Token 存储在 `McpAuth` Service 中，支持过期检测

### 对 laew 的借鉴价值

1. **三种传输的自动回退**是优秀设计：laew 接入 MCP 时可采用类似策略
2. **Catalog 分页遍历**值得借鉴：MCP server 可能有大量工具，分页是必要能力
3. **工具名标准化**（`server_toolName`）避免命名冲突，laew 应采用类似策略
4. **OAuth 完整实现**参考价值高：laew 远程 MCP 接入时可直接参考
5. **连接状态机**（connected/disabled/failed/needs_auth）值得借鉴用于 laew 的 Provider 管理
6. **Resource 工具注册**（list/read resource）为 MCP 资源提供了一等公民支持，laew 可以考虑类似模式

---

## 专题 5：Session 状态机与 Agent 类型系统

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/opencode/src/agent/agent.ts` | Agent 定义、权限配置、Agent 列表 |
| `packages/opencode/src/session/session.ts` | Session CRUD、状态字段 |
| `packages/opencode/src/session/status.ts` | Session 运行状态管理 |
| `packages/opencode/src/session/prompt.ts` | Session 主循环（prompt/loop/shell） |
| `packages/opencode/src/session/processor.ts` | SessionProcessor（流事件处理） |
| `packages/opencode/src/session/run-state.ts` | SessionRunState（并发控制） |

### 5.1 Session 状态

Session 本身是持久化实体，运行时状态由 `SessionStatus` 管理：

```typescript
// session/status.ts 中的状态类型
type SessionStatusType =
  | { type: "idle" }
  | { type: "busy" }
  | { type: "retry"; attempt: number; message: string; action?: string; next?: number }
```

状态转换：
```
idle → busy（开始处理）
busy → idle（处理完成/出错/被拒绝）
busy → retry（自动重试，如 rate limit）
busy → busy（compaction 后继续）
```

### 5.2 SessionPrompt 主循环

`SessionPrompt.loop()` 是 Agent 的核心循环，处理一次用户输入到最终回复的完整流程：

```typescript
// session/prompt.ts
const loop = Effect.fn("SessionPrompt.loop")(function* (input: LoopInput) {
  while (true) {
    // 1. 构建系统提示词、工具、消息
    const system = yield* sys.build({ agent, model, session })
    const tools = yield* SessionTools.resolve({ agent, model, session, processor, messages })
    
    // 2. 调用 LLM 流
    const stream = llm.stream({ sessionID, model, agent, system, messages, tools })
    const result = yield* processor.process(stream)
    
    // 3. 根据结果决定下一步
    switch (result) {
      case "compact":
        // 触发 compaction，压缩后继续循环
        yield* compaction.process({ parentID, messages, sessionID, auto: true, overflow: true })
        continue
      case "stop":
        return  // 用户拒绝 / 错误 / 被中断
      case "continue":
        // 检查是否需要 compaction
        if (needsCompaction) continue
        return  // 正常完成
    }
  }
})
```

### 5.3 内置 Agent 类型

```typescript
// packages/opencode/src/agent/agent.ts
const agents: Record<string, Info> = {
  build: {
    name: "build",
    description: "The default agent. Executes tools based on configured permissions.",
    mode: "primary",       // 主 Agent，用户可选
    native: true,
    permission: Permission.merge(defaults, {
      question: "allow",   // 允许提问
      plan_enter: "allow", // 允许进入 plan 模式
    }),
  },

  plan: {
    name: "plan",
    description: "Plan mode. Disallows all edit tools.",
    mode: "primary",
    native: true,
    permission: Permission.merge(defaults, {
      task: { general: "deny" },           // 禁止派发子任务
      edit: {
        "*": "deny",                        // 禁止所有编辑
        ".opencode/plans/*.md": "allow",    // 只允许编辑计划文件
      },
      plan_exit: "allow",
    }),
  },

  general: {
    name: "general",
    description: "General-purpose agent for researching complex questions...",
    mode: "subagent",      // 子 Agent，不能作为主 Agent
    permission: Permission.merge(defaults, { todowrite: "deny" }),
  },

  explore: {
    name: "explore",
    description: "Fast agent specialized for exploring codebases...",
    mode: "subagent",
    prompt: PROMPT_EXPLORE,  // 专用系统提示词
    permission: Permission.merge(defaults, {
      "*": "deny",
      grep: "allow", glob: "allow", list: "allow",
      bash: "allow", webfetch: "allow", websearch: "allow", read: "allow",
    }),
  },

  compaction: {
    name: "compaction",
    mode: "primary",
    hidden: true,          // 用户不可见
    prompt: PROMPT_COMPACTION,
    permission: { "*": "deny" },  // 无工具权限
  },

  title: {
    name: "title",
    mode: "primary",
    hidden: true,
    temperature: 0.5,
    prompt: PROMPT_TITLE,
    permission: { "*": "deny" },
  },

  summary: {
    name: "summary",
    mode: "primary",
    hidden: true,
    prompt: PROMPT_SUMMARY,
    permission: { "*": "deny" },
  },
}
```

### 5.4 Agent 选择机制

```typescript
// 获取默认 Agent
const defaultAgent = Effect.fnUntraced(function* () {
  const c = yield* config.get()
  if (c.default_agent) {
    const agent = agents[c.default_agent]
    if (!agent) throw new Error(`default agent "${c.default_agent}" not found`)
    if (agent.mode === "subagent") throw new Error(...)  // 子 Agent 不能作为默认
    if (agent.hidden === true) throw new Error(...)       // 隐藏 Agent 不能作为默认
    return agent
  }
  // 没配置时，取第一个 primary + 非 hidden 的
  return Object.values(agents).find(a => a.mode !== "subagent" && !a.hidden)
})
```

**用户自定义 Agent**：通过配置文件 `agent` 字段，可以覆盖内置 Agent 或新增自定义 Agent：

```typescript
for (const [key, value] of Object.entries(cfg.agent ?? {})) {
  if (value.disable) { delete agents[key]; continue }
  let item = agents[key]
  if (!item) {
    item = agents[key] = { name: key, mode: "all", permission: ... }
  }
  // 覆盖配置
  if (value.model) item.model = Provider.parseModel(value.model)
  if (value.prompt) item.prompt = value.prompt
  // ...
}
```

### 5.5 Permission 系统

每个 Agent 拥有独立的 `permission: PermissionV1.Ruleset`，是规则数组：

```typescript
// 评估逻辑
export function evaluate(permission: string, pattern: string, ...rulesets: Ruleset[]): Rule {
  return rulesets.flat().findLast(
    (rule) => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern)
  ) ?? { action: "ask", permission, pattern: "*" }
}
```

规则是 **后匹配优先**（`findLast`），后面的规则覆盖前面的。这允许 `defaults` 设置基础规则，Agent 特定规则覆盖特定项。

### 对 laew 的借鉴价值

1. **Agent 模式（primary/subagent/all）**值得借鉴：laew 的 6 角色可以明确标注哪些可作为入口、哪些只能被委派
2. **Permission Ruleset 作为 Agent 的核心差异化**是优秀设计：laew 的不同 Agent 角色应有明确的工具权限边界
3. **hidden Agent**（compaction/title/summary）是好模式：laew 的 SessionContext Agent 可以作为隐藏角色
4. **用户自定义 Agent**思路可借鉴：laew 允许用户自定义 Agent 配置（模型/提示词/权限）
5. **`findLast` 后匹配优先**的权限评估：laew 可以采用类似模式，基础规则 + 角色覆盖 + 用户覆盖

---

## 专题 6：主循环与工具调用

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/opencode/src/session/prompt.ts` | 主循环（prompt/loop/shell/command） |
| `packages/opencode/src/session/processor.ts` | 流事件处理器（tool-call/tool-result/text 等） |
| `packages/opencode/src/session/llm.ts` | LLM 流式输出适配 |
| `packages/opencode/src/session/tools.ts` | 工具解析与注册 |
| `packages/opencode/src/tool/tool.ts` | Tool trait 定义（Def/Info/define） |
| `packages/opencode/src/tool/registry.ts` | ToolRegistry（内置 + MCP + 插件工具） |
| `packages/opencode/src/session/retry.ts` | 重试策略 |

### 6.1 主循环流程

`SessionPrompt.loop()` 是核心循环，完整链路：

```
用户输入
  → SessionPrompt.prompt() 构建 PromptInput
    → SessionPrompt.loop() 进入循环
      → SystemPrompt.build() 构建系统提示词
      → SessionTools.resolve() 解析所有可用工具
      → LLM.stream() 启动 LLM 流
        → SessionProcessor.process() 处理流事件
          → handleEvent() 逐事件处理
            → tool-call: 创建 tool part，触发 doom_loop 检测
            → tool-result: 完成 tool part
            → text-delta: 增量更新文本
            → step-finish: 检查溢出，触发 summary
      → 返回 "compact" | "stop" | "continue"
        → compact: 执行 compaction 后继续循环
        → stop/continue: 结束
```

### 6.2 流式输出处理

LLM 流通过 `LLMEvent` 统一事件模型，两种运行时适配：

```typescript
// session/llm.ts
export type StreamInput = {
  user: SessionV1.User
  sessionID: string
  model: Provider.Model
  agent: Agent.Info
  system: string[]
  messages: ModelMessage[]
  tools: Record<string, Tool>
}

const stream: Interface["stream"] = (input) =>
  Stream.scoped(Stream.unwrap(Effect.gen(function* () {
    const result = yield* run({ ...input, abort: ctrl.signal })
    
    if (result.type === "native") return result.stream  // @opencode-ai/llm 原生运行时
    
    // AI SDK 运行时：将 fullStream 转为 LLMEvent
    return Stream.fromAsyncIterable(result.result.fullStream).pipe(
      Stream.mapEffect((event) => LLMAISDK.toLLMEvents(state, event)),
      Stream.flatMap((events) => Stream.fromIterable(events)),
    )
  })))
```

**原生运行时 vs AI SDK 运行时**：opencode 支持两种 LLM 执行路径，通过 `flags.experimentalNativeLlm` 切换。原生运行时直接返回 `LLMEvent` 流；AI SDK 运行时需要通过 `LLMAISDK.toLLMEvents()` 适配。

### 6.3 工具调用完整链路

**注册阶段**：

```typescript
// tool/registry.ts
const tools: Interface["tools"] = Effect.fn("ToolRegistry.tools")(function* (input) {
  const filtered = (yield* all()).filter(tool => {
    // 根据模型/提供者过滤工具
    if (tool.id === WebSearchTool.id) return webSearchEnabled(input.providerID)
    if (tool.id === ApplyPatchTool.id) return usePatch  // GPT 模型用 patch 而非 edit
    return true
  })
  
  return yield* Effect.forEach(visible, function* (tool) {
    // 允许插件修改工具描述/参数
    yield* plugin.trigger("tool.definition", { toolID: tool.id }, output)
    return { id, description, parameters, jsonSchema, execute }
  })
})
```

**解析阶段**：`SessionTools.resolve()` 将 `Tool.Def` 转为 AI SDK `Tool` 格式：

```typescript
// session/tools.ts
for (const item of yield* registry.tools({ modelID, providerID, agent })) {
  tools[item.id] = tool({
    description: item.description,
    inputSchema: jsonSchema(schema),
    execute(args, options) {
      return run.promise(Effect.gen(function* () {
        yield* plugin.trigger("tool.execute.before", ...)
        const result = yield* item.execute(args, ctx)
        yield* plugin.trigger("tool.execute.after", ...)
        return output
      }))
    },
  })
}
```

**执行阶段**：`processor.ts` 的 `handleEvent` 处理 `tool-call` 事件：

```typescript
case "tool-call": {
  yield* ensureToolCall(value)  // 创建 tool part
  yield* updateToolCall(value.id, (match) => ({
    ...match,
    tool: value.name,
    state: { status: "running", input, time: { start: Date.now() } },
  }))
  // doom_loop 检测在此处...
}
```

**工具执行由 AI SDK 驱动**：AI SDK 内部调用 `tool.execute()`，结果通过 `tool-result` 事件回到 `processor.ts`。

### 6.4 工具输出截断

每个工具执行后自动截断输出：

```typescript
// tool/tool.ts wrap() 函数
toolInfo.execute = (args, ctx) => Effect.gen(function* () {
  const result = yield* execute(decoded, ctx)
  const truncated = yield* truncate.output(result.output, {}, agent)
  return {
    ...result,
    output: truncated.content,
    metadata: { truncated: truncated.truncated, outputPath: truncated.outputPath },
  }
})
```

### 6.5 错误处理与重试

**工具错误**：`InvalidArgumentsError` 被 AI SDK 作为工具错误返回给 LLM，LLM 可以据此修正参数。

**LLM 错误重试**：`processor.ts` 使用 `SessionRetry.policy()` 重试策略：

```typescript
Effect.retry(
  SessionRetry.policy({
    provider: input.model.providerID,
    parse,
    set: (info) => status.set(ctx.sessionID, {
      type: "retry",
      attempt: info.attempt,
      message: info.message,
      next: info.next,
    }),
  }),
)
```

**工具调用修复**：AI SDK 的 `experimental_repairToolCall` 尝试修复无效的工具调用：

```typescript
async experimental_repairToolCall(failed) {
  // 尝试大小写修复
  const lower = failed.toolCall.toolName.toLowerCase()
  if (lower !== failed.toolCall.toolName && prepared.tools[lower]) {
    return { ...failed.toolCall, toolName: lower }
  }
  // 无法修复，转为 "invalid" 工具
  return { ...failed.toolCall, toolName: "invalid", input: JSON.stringify({ error }) }
}
```

**中断处理**：`processor.ts` 使用 `Effect.onInterrupt` 处理用户中断：

```typescript
Effect.onInterrupt(() =>
  Effect.gen(function* () {
    aborted = true
    if (!ctx.assistantMessage.error) {
      yield* halt(new DOMException("Aborted", "AbortError"))
    }
  }),
)
```

**清理函数**：`cleanup()` 确保中断后所有状态正确关闭：

```typescript
const cleanup = Effect.fn("SessionProcessor.cleanup")(function* () {
  // 1. 完成 snapshot diff
  if (ctx.snapshot) {
    const patch = yield* snapshot.patch(ctx.snapshot)
    if (patch.files.length) yield* session.updatePart({ type: "patch", ... })
  }
  // 2. 完成进行中的文本
  if (ctx.currentText) { ctx.currentText.time = { ...ctx.currentText.time, end: Date.now() } }
  // 3. 等待所有工具调用完成（250ms 超时）
  yield* Effect.forEach(Object.values(ctx.toolcalls), (call) =>
    Deferred.await(call.done).pipe(Effect.timeout("250 millis"), Effect.ignore),
  )
  // 4. 将未完成的工具标记为 error + interrupted
  for (const toolCallID of Object.keys(ctx.toolcalls)) {
    yield* session.updatePart({
      ...part,
      state: { status: "error", error: "Tool execution aborted", metadata: { interrupted: true } },
    })
  }
})
```

### 6.6 SessionProcessor 事件驱动架构

`SessionProcessor` 是整个工具调用链的核心，采用事件驱动模式处理 LLM 流：

```typescript
// processor.ts 核心事件类型
type StreamEvent =
  | { type: "reasoning-start" | "reasoning-delta" | "reasoning-end"; id: string }
  | { type: "tool-input-start" | "tool-input-delta" | "tool-input-end"; id: string; name: string }
  | { type: "tool-call"; id: string; name: string; input: unknown }
  | { type: "tool-result"; id: string; result: { type: "json"; value: unknown } }
  | { type: "tool-error"; id: string; error?: Error; message?: string }
  | { type: "text-start" | "text-delta" | "text-end"; text?: string }
  | { type: "reasoning-start" | "reasoning-delta" | "reasoning-end" }
  | { type: "step-start" | "step-finish"; reason?: string; usage?: Usage }
  | { type: "provider-error"; message: string }
  | { type: "finish" }
```

每个事件类型触发不同的状态转换，`ProcessorContext` 维护当前处理状态：

```typescript
interface ProcessorContext extends Input {
  toolcalls: Record<string, ToolCall>    // 进行中的工具调用
  shouldBreak: boolean                    // 是否在拒绝后中断
  snapshot: string | undefined            // 文件系统快照 ID
  blocked: boolean                        // 是否被权限阻止
  needsCompaction: boolean                // 是否需要 compaction
  currentText: SessionV1.TextPart | undefined  // 当前文本 part
  reasoningMap: Record<string, SessionV1.ReasoningPart>  // 推理 part 映射
}
```

### 6.6 权限控制

工具执行前通过 `Permission.ask()` 检查权限：

```typescript
// session/tools.ts 中 context 构建
ask: (req) =>
  permission.ask({
    ...req,
    sessionID: input.session.id,
    tool: { messageID: input.processor.message.id, callID: options.toolCallId },
    ruleset: Permission.merge(input.agent.permission, input.session.permission ?? []),
  }).pipe(Effect.orDie),
```

权限规则按优先级评估（`findLast`），用户在 UI 上的批准写入 `approved` 列表，后续同类调用自动放行。

### 对 laew 的借鉴价值

1. **双 LLM 运行时适配**思路可借鉴：laew 已有 Anthropic/OpenAI 双协议，可以进一步抽象为统一运行时
2. **`experimental_repairToolCall`** 是有价值的防御：laew 可以在工具调用失败时尝试参数修复
3. **插件钩子**（tool.execute.before/after）值得借鉴：laew 可以在工具执行前后注入 hook
4. **AI SDK 驱动的工具调度**与 laew 的手动调度形成对比：laew 可以考虑引入类似自动化调度
5. **工具输出自动截断**是实用设计：laew 的 SubAgent-Work 工具输出也需要截断策略
6. **权限与 Agent 绑定**的模式值得借鉴：laew 不同 Agent 角色应有不同的工具调用确认策略

---

## 总结：opencode 核心设计模式

| 模式 | 描述 | laew 借鉴优先级 |
|------|------|----------------|
| **LayerNode 依赖图** | 编译期可验证的服务依赖关系 | P1 |
| **doom_loop 检测** | 连续 N 次相同工具调用 → 权限询问 | P0（直接可用） |
| **Prune + Compaction** | 工具输出裁剪 + 对话压缩双策略 | P0 |
| **MCP 三传输自动回退** | StreamableHTTP → SSE → Stdio | P1 |
| **Permission Ruleset** | Agent 级权限规则 + findLast 优先级 | P0 |
| **InstanceState 隔离** | 按工作目录隔离服务状态 | P2 |
| **Schema 驱动验证** | 工具参数 / 消息 / 配置全链路 Schema | P1 |
| **双 LLM 运行时** | AI SDK + 原生运行时自动选择 | P2 |
| **隐藏 Agent** | compaction/title/summary 专用角色 | P1 |

---

## 附录：关键类型定义速查

### A.1 Agent.Info Schema

```typescript
// packages/opencode/src/agent/agent.ts
export const Info = Schema.Struct({
  name: Schema.String,
  description: Schema.optional(Schema.String),
  mode: Schema.Literals(["subagent", "primary", "all"]),
  native: Schema.optional(Schema.Boolean),
  hidden: Schema.optional(Schema.Boolean),
  topP: Schema.optional(Schema.Finite),
  temperature: Schema.optional(Schema.Finite),
  color: Schema.optional(Schema.String),
  permission: PermissionV1.Ruleset,
  model: Schema.optional(Schema.Struct({
    modelID: ModelV2.ID,
    providerID: ProviderV2.ID,
  })),
  variant: Schema.optional(Schema.String),
  prompt: Schema.optional(Schema.String),
  options: Schema.Record(Schema.String, Schema.Unknown),
  steps: Schema.optional(Schema.Finite),
})
```

### A.2 Tool.Def 接口

```typescript
// packages/opencode/src/tool/tool.ts
export interface Def<Parameters extends Schema.Decoder<unknown>, M extends Metadata> {
  id: string
  description: string
  parameters: Parameters
  jsonSchema?: JSONSchema7
  execute(args: Schema.Schema.Type<Parameters>, ctx: Context): Effect.Effect<ExecuteResult<M>>
  formatValidationError?(error: unknown): string
}

export interface ExecuteResult<M extends Metadata> {
  title: string
  metadata: M
  output: string
  attachments?: Omit<SessionV1.FilePart, "id" | "sessionID" | "messageID">[]
}
```

### A.3 MCP.Status 联合类型

```typescript
// packages/opencode/src/mcp/index.ts
export const Status = Schema.Union([
  Schema.Struct({ status: Schema.Literal("connected") }),
  Schema.Struct({ status: Schema.Literal("disabled") }),
  Schema.Struct({ status: Schema.Literal("failed"), error: Schema.String }),
  Schema.Struct({ status: Schema.Literal("needs_auth") }),
  Schema.Struct({ status: Schema.Literal("needs_client_registration"), error: Schema.String }),
])
```

### A.4 SessionProcessor.Result 类型

```typescript
// packages/opencode/src/session/processor.ts
export type Result = "compact" | "stop" | "continue"
// "compact"  → 触发 compaction，压缩后继续
// "stop"     → 用户拒绝 / 错误 / 被中断
// "continue" → 正常完成，检查是否需要 compaction
```

### A.5 Permission 评估流程

```
用户输入 → Agent.permission + Session.permission 合并
  → 对每个 pattern 调用 evaluate(permission, pattern, ...rulesets)
    → findLast 匹配（后定义的规则优先）
      → "allow" → 直接执行
      → "deny"  → 抛出 DeniedError
      → "ask"   → 创建 Deferred，发布 Asked 事件，等待用户响应
        → 用户批准 → 写入 approved 列表，后续同类调用自动放行
        → 用户拒绝 → 抛出 RejectedError
```
