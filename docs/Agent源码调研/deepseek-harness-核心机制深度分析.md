# deepseek-harness 核心机制深度分析（第二轮）

- **分析日期**: 2026-09-04
- **源码根路径**: `/usr/local/LsmGitOpenSource/deepseek-harness`
- **分析范围**: Cordis 内核 / Goal 域 / SubAgent / Guard / Context Projection / MCP
- **核心源文件数**: ~45 个（不含测试）
- **方法论**: 逐函数读取源码，提取类名/函数名/关键代码片段

---

## 专题 1：Cordis 插件化内核

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `vendor/cordis/src/context.ts` | Context 根类：Proxy 宿主、extend/isolate/intercept 子上下文 |
| `vendor/cordis/src/service.ts` | Service 抽象基类：命名服务注册 + invoke callable |
| `vendor/cordis/src/fiber.ts` | Fiber：插件生命周期状态机（PENDING→LOADING→ACTIVE→FAILED→DISPOSED） |
| `vendor/cordis/src/registry.ts` | RegistryService：插件注册表 + 依赖声明 + plugin()/inject() |
| `vendor/cordis/src/events.ts` | EventsService：5 种事件派发模式 + 生命周期事件 |
| `vendor/cordis/src/reflect.ts` | ReflectService：Proxy handler 实现服务解析、provide/accessor/mixin |
| `vendor/cordis/src/utils.ts` | symbols 常量、createCallable、DisposableList |

### 1.1 Context —— 一切的根

Context 是整个框架的核心容器。它本身是一个 **Proxy** 对象：

```typescript
// vendor/cordis/src/context.ts:71-84
constructor() {
  this[symbols.isolate] = Object.create(null)
  this[symbols.intercept] = Object.create(null)
  const self = new Proxy<this>(this, ReflectService.handler)
  this.root = self
  this.fiber = new Fiber(self, {}, Object.create(null), null, () => [])
  this.reflect = new ReflectService(self)
  this.registry = new RegistryService(self)
  this.events = new EventsService(self)
  this.logger = new LoggerService(self)
  return self
}
```

**三个关键的子上下文操作**：

- `extend(meta)` — 创建子上下文（原型继承），不修改父
- `isolate(name, label?)` — 为某个服务创建独立作用域（不同的服务实例）
- `intercept(name, config)` — 为某个服务添加配置拦截（用于插件级别的服务配置覆盖）

### 1.2 Service —— 命名服务抽象

Service 是所有服务的基类。构造函数中自动注册到 context：

```typescript
// vendor/cordis/src/service.ts:42-58
constructor(protected ctx: Context, name: string) {
  name ??= this.constructor['provide'] as string
  let self = this
  const tracker: Tracker = { associate: name, property: 'ctx' }
  if (self[symbols.invoke]) {
    self = createCallable(name, joinPrototype(...), tracker)
  }
  self.ctx = ctx
  self.name = name
  self.ctx.reflect.provide(name, self, this[symbols.check])
  return self
}
```

`Service.invoke` 符号让 Service 实例可被调用（如 `ctx.logger('name')`），`Service.extend` 支持派生。

### 1.3 RegistryService —— 插件注册表

支持三种插件形态：

```typescript
// vendor/cordis/src/registry.ts:92-133
export type Plugin<T = any> =
  | Plugin.Function<T>    // (ctx, config) => any
  | Plugin.Constructor<T> // new (ctx, config) => any
  | Plugin.Object<T>      // { apply(ctx, config) }
```

每个插件有 `Plugin.Runtime`（共享元数据）和多个 `Fiber`（每次 `ctx.plugin()` 一个）。

`plugin()` 核心流程：

```typescript
// vendor/cordis/src/registry.ts:316-336
plugin(plugin: Plugin, config?: any) {
  const callback = this.resolve(plugin)   // 提取回调函数
  let runtime = this._internal.get(callback)
  if (!runtime) {
    runtime = { name, callback, fibers: new DisposableList(), Config: plugin.Config }
    this._internal.set(callback, runtime)
  }
  const fiber = new Fiber(this.ctx, config, Inject.resolve(plugin.inject), runtime, ...)
  return wrapped
}
```

### 1.4 Fiber —— 插件生命周期状态机

```typescript
// vendor/cordis/src/fiber.ts:147-154
export const enum FiberState {
  PENDING,    // 等待依赖服务就绪
  LOADING,    // 插件回调正在运行
  ACTIVE,     // 已加载并提供服务
  FAILED,     // 回调或配置抛出异常
  DISPOSED,   // 已移除，不可重启
  UNLOADING,  // disposer 正在运行
}
```

Fiber 的依赖驱动机制：通过 `_checkImpl` 检查每个注入的服务是否可用，通过 `_refresh` 计算 epoch（所有依赖的 uid 拼接），epoch 变化时触发 `_reload` 或 `_unload`：

```typescript
// vendor/cordis/src/fiber.ts:611-623
_refresh() {
  let epoch: string | boolean = false
  epoch = ''
  for (const name of Object.keys(this.inject)) {
    const impl = this._store[name]
    if (!impl) { epoch = INACTIVE; break }
    epoch += ':' + impl.fiber.uid
  }
  this._setEpoch(epoch)
}
```

### 1.5 EventsService —— 5 种派发模式

```typescript
// vendor/cordis/src/events.ts:32
export type DispatchMode = 'emit' | 'parallel' | 'serial' | 'bail' | 'waterfall'
```

| 模式 | 行为 |
|------|------|
| `emit` | 同步触发，忽略返回值 |
| `parallel` | 并发 await 所有监听器 |
| `serial` | 顺序 await，遇到 bail 值停止 |
| `bail` | 同步，第一个 bail 值停止 |
| `waterfall` | 组合式：每个监听器包装 next，可 veto |

### 1.6 ReflectService —— 代理驱动的依赖注入

```typescript
// vendor/cordis/src/reflect.ts:135-171
static handler: ProxyHandler<Context> = {
  get: (target, prop, ctx) => {
    if (isSpecialProperty(prop)) return Reflect.get(target, prop, ctx)
    if (Reflect.has(target, prop)) return getTraceable(ctx, Reflect.get(target, prop, ctx))
    // 服务解析：沿 fiber 链向上查找
    return ctx.events.waterfall('internal/get', ctx, prop, error, () => {
      let fiber = ctx.fiber
      while (true) {
        const impl = fiber.store?.[prop]
        if (impl) return impl.value
        if (prop in fiber.inject) throw error  // 声明了但未提供
        fiber = fiber.parent.fiber
      }
    })
  }
}
```

**mixin 机制**：将服务方法直接暴露到 `ctx` 上下文：

```typescript
// vendor/cordis/src/reflect.ts:219-223
// ReflectService 构造函数中：
this.mixin('reflect', ['get', 'set', 'provide', 'accessor', 'mixin'])
this.mixin('fiber', ['runtime', 'effect'])
this.mixin('registry', ['inject', 'plugin'])
this.mixin('events', ['on', 'once', 'parallel', 'emit', 'serial', 'bail', 'waterfall'])
```

这就是为什么可以直接 `ctx.on()`、`ctx.plugin()` 而不写 `ctx.events.on()`。

### 1.7 核心插件分类

packages/ 目录按功能域组织为 50+ 个子包：

| 功能域 | 包（代表） | 说明 |
|--------|-----------|------|
| 核心运行时 | `core/agent-loop`, `core/agent`, `core/session`, `core/tools` | Agent 循环、会话、工具注册 |
| Goal 域 | `goal/goal`, `goal/goal-round-driver`, `goal/tool-goal`, `goal/command-goal` | 目标管理与自动轮次 |
| SubAgent | `subagent/subagent`, `subagent/subagent-*-in-process`, `subagent/subagent-acp`, `subagent/subagent-codex`, `subagent/subagent-claude-code` | 子代理服务定义 + 多后端 |
| 上下文管理 | `context/agent-instructions`, `context/file-reference`, `context/session-reference` | 指令注入、文件引用、会话引用 |
| 压缩/上下文 | `compaction/compaction`, `compaction/compaction-basic`, `compaction/compaction-tool-result-pruner` | 对话压缩与 token 管理 |
| Guard | `guard/repeat-tool-reminder`, `guard/timeout-policy` | 工具重复检测、超时强制 |
| MCP | `mcp/mcp-client` | MCP 客户端桥接 |
| 会话投影 | `session/session-projection`, `session/session-projection-cache` | 事件源投影单元 |
| SDK | `sdk/server`, `sdk/client`, `sdk/protocol` | JSON-RPC SDK 协议 |
| LLM | `llm/llm`, + 多协议适配 | 统一 LLM 接口 |
| 宿主 | `host/plugin-inventory`, `host/webserver`, `host/frontend-static` | 插件清单、Web 服务器 |
| 计划 | `plan/plan` | 规划 |
| 工具 | `skill/skill`, `shell/shell`, `fs/fs` | 技能、Shell、文件系统 |

### 1.8 symbols 常量体系

Cordis 使用一组全局 Symbol 作为内部协议，避免命名冲突：

```typescript
// vendor/cordis/src/utils.ts (推断)
// symbols.init: 类插件构造后的初始化钩子
// symbols.check: 可用性断言
// symbols.config: 拦截配置类型参数
// symbols.invoke: 使 Service 实例可调用
// symbols.extend: 派生服务实例
// symbols.tracker: 上下文追踪元数据
// symbols.filter: 事件过滤器
// symbols.isolate: 隔离映射
// symbols.intercept: 拦截映射
// symbols.shadow: 影子上下文
// symbols.receiver: 代理接收者
// symbols.initHooks: 类插件的初始化钩子列表
// symbols.metadata: 方法装饰器元数据
```

### 1.9 DisposableList —— 可清理资源列表

```typescript
// vendor/cordis/src/utils.ts (推断)
export class DisposableList<T extends Disposable> {
  private items: T[] = []
  push(item: T): () => boolean {
    this.items.push(item)
    return () => { const i = this.items.indexOf(item); if (i >= 0) { this.items.splice(i, 1); return true } return false }
  }
  clear(): T[] { return this.items.splice(0) }
  get length(): number { return this.items.length }
  [Symbol.iterator]() { return this.items[Symbol.iterator]() }
}
```

每个 Fiber 拥有一个 `DisposableList<Disposable>` 存储所有注册的清理函数，卸载时逆序执行。

### 1.10 Inject 装饰器 —— 类级别的依赖声明

```typescript
// vendor/cordis/src/registry.ts:37-60
export function Inject<K extends InjectKey>(name: K, config?) {
  return function (value, decorator) {
    if (decorator.kind === 'class') {
      // 类级别：贡献到 inject map
      value.inject[name] = config
    } else if (decorator.kind === 'method') {
      // 方法级别：延迟执行直到服务可用
      decorator.addInitializer(function () {
        (this[symbols.initHooks] ??= []).push(() => {
          this.ctx.inject(inject, (ctx) => value.call(this))
        })
      })
    }
  }
}
```

这允许类方法声明依赖后自动延迟执行：`@Inject('tools') initTools() { ... }` 会在 `tools` 服务就绪后自动调用。

### 1.11 Fiber 的 effect 系统 —— 结构化并发

`Fiber.effect()` 是 Cordis 最精密的部分。它支持五种效果体：

```typescript
// vendor/cordis/src/fiber.ts:83-94
export type Effect<T = any> =
  | Disposable<T>                    // 同步 disposer
  | Promise<Disposable<T>>           // 异步 disposer
  | Iterable<Disposable<T>>          // 同步生成器（多次 yield disposer）
  | AsyncIterable<Disposable<T>>     // 异步生成器
```

生成器效果体允许一个 effect 逐步注册多个 disposer，每个 yield 立即生效，卸载时逆序清理。

### 1.12 "Everything is a Plugin" 的代码证据

1. **核心服务本身就是插件**：`ReflectService`、`EventsService`、`RegistryService`、`LoggerService` 都通过 `ctx.reflect`、`ctx.events` 等注入到 Context
2. **LLM 适配器是插件**：`server.ts:151` `this.llmFiber = await this.ctx.plugin(LlmDeepSeek, {})`
3. **工具是插件**：每个工具包通过 `export function apply(ctx)` 注册
4. **Guard 是插件**：`repeat-tool-reminder`、`timeout-policy` 都是 Cordis 插件
5. **MCP 是插件**：`mcp-client` 作为插件加载，一个实例连接一个 MCP server
6. **SubAgent 后端是插件**：`subagent-fork-in-process` 通过 `ctx.subagents.registerProvider()` 注册
7. **Goal 域是插件**：`GoalService` 继承 `TypertRemoteService`，通过 `ctx.goals` 注入
8. **Compaction 是插件**：`BasicCompactionEngine` 继承 `CompactionEngine` 抽象类，通过 `ctx.compaction` 注入
9. **Session Projection 是插件**：`SessionProjectionRegistry` 是 Service，通过 `ctx.sessionProjections` 注入
10. **SDK Server 是插件**：`HarnessSdkJsonRpcServer` 消费 Context 来创建 session

**对 laew 的借鉴价值**：

- Cordis 的 "Context Proxy + 服务解析 + 原型链继承" 模式非常精巧，可以借鉴到 Rust 的 trait object + Arc 注册表
- Fiber 的依赖等待 epoch 机制：Rust 可以用 tokio::watch channel + 状态机实现
- 5 种事件派发模式覆盖了所有场景（同步/异步/顺序/中断/组合），laew 目前只有简单 emit，可以扩展
- mixin 机制（将服务方法暴露到 ctx）是一个很好的 API 设计，减少了代码量

---

## 专题 2：Goal 域 + round driver

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/goal/goal/src/types.ts` | GoalSnapshot/GoalView/GoalProjection 等纯类型 |
| `packages/goal/goal/src/domain.ts` | GoalOperation/GoalChanged/GoalMessageSource 等宿主侧事件类型 |
| `packages/goal/goal/src/index.ts` | GoalService：CAS 变更 + 事件源缓存 + session 事件写入 |
| `packages/goal/goal/src/fold.ts` | 纯 replay fold：从 session 事件流重建 Goal 状态 |
| `packages/goal/goal/src/runtime.ts` | GoalId 构造、GoalError 错误类 |
| `packages/goal/goal-round-driver/src/index.ts` | round driver：自动续轮调度器 |
| `packages/goal/goal-round-driver/src/prompt.ts` | 续轮 prompt 渲染 |
| `packages/goal/tool-goal/src/index.ts` | 模型工具：get_goal / create_goal / update_goal |

### 2.1 Goal 域模型

```typescript
// packages/goal/goal/src/types.ts:44-49
export type GoalPhase = 'active' | 'paused' | 'blocked' | 'complete'

// types.ts:58-68
export interface GoalSnapshot extends GoalRef {
  readonly objective: string
  readonly phase: GoalPhase
  readonly blockedReason?: GoalBlockReason
  readonly maxGoalRounds: number
}
```

Goal 是一个 **事件源**（event-sourced）模型：每次变更都写入 `goal/change` session 事件，当前状态通过 fold 事件流重建。

状态转换图：
```
create → active → pause → paused
active → block  → blocked
active → complete → complete
paused/blocked/complete → resume → active（需有轮次预算）
```

### 2.2 GoalService —— CAS 变更服务

```typescript
// packages/goal/goal/src/index.ts:183-197
export class GoalService extends TypertRemoteService {
  static inject = ['agents']
  static Config: z<Config> = z.object({
    defaultMaxGoalRounds: z.number().default(256),  // 默认 256 轮
  })
  private readonly caches = new WeakMap<Session, GoalCache>()
```

核心提交流程：

```typescript
// index.ts:542-558
private commit(agent, cache, change, activation) {
  const ref = goalChangeRef(change)
  cache.pendingActivation = { seq: agent.session.seq, activation }
  agent.session.append('goal/change', change)  // 写入 session 事件
  this.sync(agent.session, cache)               // 增量 fold
  const goal = this.view(cache)
  agentEvents(this.ctx, agent).emit('goal/changed', { change: notification })
}
```

CAS（Compare-And-Set）保证：每次变更都要求提供当前的 `{id, revision}` ref，不匹配则抛 `GOAL_STALE_REVISION`。

### 2.3 round driver —— 自动续轮机制

round driver 是一个 Cordis 插件，监听 `agent/status` 变为 `idle` 时检查是否需要自动发起下一轮：

```typescript
// packages/goal/goal-round-driver/src/index.ts:138-205
async function drive(state: DriverState): Promise<void> {
  if (!readyToDrive(state)) return

  // 持久化 checkpoint
  if (state.needsCheckpoint) {
    await ctx.sessions.flush(agent.session)
    if (!readyAfterCheckpoint(state)) return
  }

  const goal = currentGoal(state)
  if (goal === undefined || goal.phase !== 'active' || goal.activation !== 'armed') return

  // 轮次上限检测 —— 核心！
  if (goal.roundsStarted >= goal.maxGoalRounds) {
    ctx.goals.block(agent, goalRef(goal), {
      code: 'round-limit',
      message: `Goal reached its configured limit of ${goal.maxGoalRounds} rounds.`,
    })
    return
  }

  // 发起下一轮
  const round = goal.roundsStarted + 1
  const content = renderGoalRoundPrompt(goal, round)
  const message = createUserMessage({ content, source: { kind: 'goal', ... } })
  agent.followup(message)
}
```

**防竞态设计**：`requestDrive` 使用 serialized loop 确保同一 agent 上只有一个驱动任务运行：

```typescript
// index.ts:208-241
function requestDrive(state: DriverState): void {
  state.requested = true
  if (state.run !== undefined) return  // 已有驱动任务
  let run = ctx.agents.withoutInitiator(async () => {
    while (state.requested && !state.stopping) {
      state.requested = false
      await drive(state)
    }
  })
  state.run = run
}
```

### 2.4 maxGoalRounds 检测

两层保障：
1. **GoalService.resume()**: 恢复时检查 `roundsStarted >= maxGoalRounds`，抛 `GOAL_INVALID_TRANSITION`
2. **round driver.drive()**: 每轮开始前检查，达到限制则自动 block

```typescript
// goal-round-driver/src/index.ts:166-170
if (goal.roundsStarted >= goal.maxGoalRounds) {
  ctx.goals.block(agent, goalRef(goal), {
    code: 'round-limit',
    message: `Goal reached its configured limit of ${goal.maxGoalRounds} rounds.`,
  })
  return
}
```

### 2.5 fold —— 纯事件流重建

```typescript
// packages/goal/goal/src/fold.ts:271-306
export function applyGoalChange(state: GoalFoldState, change: GoalChangeMeta): void {
  if (change.operation === 'clear') {
    state.goal = undefined
    state.roundsStarted = 0
    return
  }
  if (change.operation === 'create') {
    if (change.goal.revision !== 1 || change.goal.phase !== 'active' || change.roundsStarted !== 0
      || (state.goal !== undefined && state.goal.phase !== 'complete')
      || state.seenGoalIds.has(change.goal.id)) {
      throw new Error('goal create requires a fresh active revision-one goal with zero rounds')
    }
    state.seenGoalIds.add(change.goal.id)
  } else {
    validateSnapshotTransition(state, change, current)  // 严格状态转换验证
  }
  state.goal = change.goal
  state.roundsStarted = change.roundsStarted
}
```

fold 不仅重建状态，还 **验证** 每个事件的合法性（状态转换规则），fail-loud。

### 2.6 tool-goal —— 模型面对的工具

```typescript
// packages/goal/tool-goal/src/index.ts:195-206
ctx.tools.register(defineTool({
  name: 'get_goal',
  description: 'Read the current same-session goal...',
  execute(_args, exec) {
    return Promise.resolve(goalValue(ctx.goals.get(execution.agent)))
  },
}))
```

三个工具：`get_goal`（读）、`create_goal`（创建）、`update_goal`（编辑/暂停/恢复/完成/阻塞）。

### 2.7 Goal 的 projection unit 实现细节

Goal 域同时注册了一个 SessionProjection 单元，供客户端（浏览器）实时查看：

```typescript
// packages/goal/goal/src/index.ts:96-113
export function applyGoalProjection(state: GoalProjection | null, event: SessionEvent): GoalProjection | null {
  if (event.type !== 'goal/change') return state  // 不关心的事件，返回相同引用
  // ...
  return change.operation === 'clear' ? null : { goal: change.goal, roundsStarted, createdAt, updatedAt }
}
```

注意这是一个 "last-wins" fold：每次 goal/change 事件携带完整快照，直接替换旧状态。这比增量 fold 更简单（对比 fold.ts 的严格验证）。

### 2.8 tool-goal 的 authority 模型

tool-goal 工具通过 `authority.ts` 区分调用者身份：

```typescript
// packages/goal/tool-goal/src/authority.ts (推断)
// requireDirectHuman(): 只允许直接人类请求调用（不允许子代理）
// completionAuthority(): 允许 goal-round 内的自动续轮调用
// goalToolExecution(): 从 ToolExecution 提取 Agent 上下文
```

create_goal、edit、pause、resume 都要求 `requireDirectHuman()`（人类发起）。
complete 和 blocked 在 goal-round 内自动续轮时也允许（`completionAuthority`）。

### 2.9 Goal wrapup —— 完成/阻塞时的上下文注入

```typescript
// packages/goal/tool-goal/src/wrapup.ts (推断)
// renderWrapupContext(objective, blockedReason?) → ContentBlock[]
// 在 goal complete/blocked 时，注入一个 plugin source 的 user 消息，
// 告诉模型："目标已完成/阻塞，请总结工作"
```

**对 laew 的借鉴价值**：

- Goal 的事件源设计非常优雅：每次变更写事件、fold 重建状态、CAS 保证并发安全。Rust 中可用 `serde_json` + `Vec<Event>` 实现
- round driver 的 serialized loop + request coalescing 模式值得借鉴，避免并发轮次调度
- maxGoalRounds 的双层保障（resume 检查 + drive 检查）是健壮性设计的好范例
- GoalPhase 状态机清晰，laew 的 TaskLevel（simple/medium/hard）可以借鉴此状态管理
- tool-goal 的 authority 模型（区分人类调用 vs 自动续轮调用）很实用，laew 可以借鉴到 Yolo Agent 的权限控制
- wrapup 上下文注入（完成时自动告诉模型总结）是一个巧妙的 UX 设计

---

## 专题 3：SubAgent provider 注册表

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/subagent/subagent/src/index.ts` | SubagentRuntime：provider 注册表 + start/followup/interrupt API |
| `packages/subagent/subagent/src/types.ts` | SubagentProvider/SubagentRun/SubagentCapabilities 接口 |
| `packages/subagent/subagent/src/continuation.ts` | SubagentContinuationManager：可续对话管理 |
| `packages/subagent/subagent/src/lifecycle.ts` | 生命周期事件发射器 |
| `packages/subagent/subagent-fork-in-process/src/index.ts` | ForkInProcessProvider：fork 子代理后端 |
| `packages/subagent/subagent-spawn-in-process/` | 独立 Agent 子代理后端 |
| `packages/subagent/subagent-acp/` | ACP（Agent Communication Protocol）后端 |
| `packages/subagent/subagent-codex/` | Codex 子代理后端 |
| `packages/subagent/subagent-claude-code/` | Claude Code 子代理后端 |
| `packages/subagent/subagent-dsh-sdk/` | DSH SDK 子代理后端 |
| `packages/subagent/tool-subagent/src/` | 模型面对的 delegation 工具 |
| `packages/subagent/tool-subagent-control/src/index.ts` | send_message / interrupt_agent 工具 |
| `packages/subagent/tool-subagent-report/src/` | 子代理向上汇报工具 |

### 3.1 SubagentProvider 接口

```typescript
// packages/subagent/subagent/src/types.ts:300-346
export interface SubagentProvider {
  readonly name: string
  readonly capabilities: SubagentCapabilities
  readonly inheritsParentContext: boolean
  readonly agentRouteDefaults?: Readonly<{ provider: string; model: string }>
  start(request: ResolvedSubagentStartRequest): Promise<SubagentRun>
  prepareContinuable?(request: ContinuableCreateRequest): Promise<ContinuableCreateSpec>
}
```

**能力声明**：

```typescript
// types.ts:86-92
export interface SubagentCapabilities {
  readonly agentOptions: boolean   // 能否覆盖 provider/model
  readonly outputSchema: boolean   // 能否输出结构化 JSON
  readonly depthLimit: boolean     // 能否限制委托深度
  readonly toolFilter: boolean     // 能否限制子代理工具集
  readonly persona: boolean        // 能否设置独立人设
}
```

### 3.2 SubagentRuntime 注册表

```typescript
// packages/subagent/subagent/src/index.ts:196-209
export class SubagentRuntime extends TypertRemoteService {
  private providers = new Map<string, SubagentProvider>()
  private continuations: SubagentContinuationManager | undefined
  private readonly setupRegistry = new SubagentActivationSetupRegistry()
```

**注册流程**（effect-scoped + HMR-safe）：

```typescript
// index.ts:507-523
registerProvider(provider: SubagentProvider): () => void {
  return this.ctx.effect(function* () {
    if (this.providers.has(name)) throw new SubagentError('duplicate', 'DUPLICATE_PROVIDER')
    this.providers.set(name, provider)
    yield () => {
      this.providers.delete(name)
      this.emitLifecycle('subagent/provider-removed', name)
    }
    this.ctx.emit('subagent/provider-added', provider)
  }.bind(this), 'subagents.registerProvider()')
}
```

### 3.3 启动与上下文传递

**一次性子代理（one-shot）**：

```typescript
// index.ts:552-564
async start(name: string, request: SubagentStartRequest): Promise<SubagentRun> {
  const provider = this.expectProvider(name)
  this.assertCapabilities(provider, request)    // 检查能力声明
  assertSubagentMaxDepth(request.maxDepth)      // 深度限制
  if (request.outputSchema !== undefined) assertObjectJsonSchema(request.outputSchema)
  const descriptor = snapshotSubagentDescriptor({ mode: 'one-shot', provider: name, label })
  const resolved: ResolvedSubagentStartRequest = { ...request, descriptor }
  return observeRun(this.emitLifecycle, name, request.parent, await provider.start(resolved))
}
```

**Fork 后端的上下文传递**（继承父 session 已完成的 turn 前缀）：

```typescript
// packages/subagent/subagent-fork-in-process/src/index.ts:48-54
function completedTurnPrefix(parent: Agent): SessionEvent[] {
  const events = parent.session.events
  const lastEnd = events.findLast(e => e.type === 'turn/end')
  if (lastEnd === undefined) return []
  return events.slice(0, lastEnd.seq + 1)  // 不包含当前未完成的 turn
}
```

### 3.4 工具集限制

通过 `toolFilter` 实现：

```typescript
// types.ts:146-151
readonly toolFilter?: ToolRestriction
// "in-process backends apply it as a scoped tools.restrict() in the child's
//  creation window: the named tools vanish from the child's prompt AND refuse
//  to execute"
```

### 3.5 结果回传

```typescript
// types.ts:227-253
export interface SubagentResult {
  readonly output: ContentBlock[]          // 最后一个非空 assistant 消息
  readonly structured?: unknown            // outputSchema 对应的结构化结果
  readonly diagnostic?: string             // 非 assistant 的失败详情
  readonly stopReason: SubagentStopReason  // completed/aborted/error/max-tokens/refusal
}
```

`SubagentRun` 接口：

```typescript
// types.ts:264-290
export interface SubagentRun {
  readonly id: SessionId
  readonly localAgent: Agent | undefined
  readonly result: Promise<SubagentResult>
  dispose(): Promise<void>
}
```

### 3.6 内置 SubAgent 后端列表

| 后端名 | 包 | 特点 |
|--------|-----|------|
| `fork` | `subagent-fork-in-process` | 继承父 session 上下文（completed turn prefix） |
| `spawn` | `subagent-spawn-in-process` | 独立新 Agent，不继承上下文 |
| `acp` | `subagent-acp` | Agent Communication Protocol，跨进程 |
| `codex` | `subagent-codex` | Codex 响应协议桥接 |
| `claude-code` | `subagent-claude-code` | Claude Code 子进程桥接 |
| `dsh-sdk` | `subagent-dsh-sdk` | DSH SDK JSON-RPC 远程子代理 |

### 3.7 控制工具与汇报工具

- `send_message`：向子代理发送后续消息
- `interrupt_agent`：中断子代理当前轮次
- `report` 工具：子代理向上汇报结果

### 3.8 SubagentContinuationManager —— 可续对话管理器

除了 one-shot 子代理，deepseek-harness 还支持 **可续对话**（continuable children）。ContinuationManager 管理这类子代理的完整生命周期：

```typescript
// packages/subagent/subagent/src/continuation.ts (推断)
class SubagentContinuationManager {
  // 创建可续子代理：预留身份 → 准备 seed → 创建 Agent → 投递初始 prompt
  async startContinuable(spec: ContinuableStartSpec): Promise<ContinuableStart>

  // 后续消息投递：resident → 直接 inbox；absent → cold resume
  async followup(parent, childId, content, options): Promise<MessageId>

  // 中断当前轮次（保留 pending inbox）
  interrupt(targetSessionId, authority): void

  // 子代理向上汇报
  async reportFrom(child, content, options): Promise<MessageId>

  // 清理：停止子代理并等待 quiescence
  async drainDescendants(parents): Promise<void>
  async drainChildren(parent, childIds): Promise<void>
}
```

**resident vs cold resume**：可续子代理可以是"驻留"（Agent 在内存中）或"冷"（Agent 已释放，Session 持久化在磁盘）。`followup` 会自动判断：驻留则直接投递到 inbox，冷则从持久化 Session 恢复。

### 3.9 in-process-driver —— 进程内子代理驱动

`subagent-in-process-driver` 是所有进程内后端共享的驱动模块，负责：

```typescript
// packages/subagent/subagent-in-process-driver/ (推断)
// 1. 创建子 Agent（合并 parent AgentOptions + 子代理覆盖）
// 2. 注入 seed（父 session 已完成的 turn 前缀，仅 fork）
// 3. 投递 prompt 作为第一条 user message
// 4. 监听 Agent 生命周期事件
// 5. 返回 SubagentRun handle（含 result promise + dispose）
```

### 3.10 depth 管理 —— 委托深度限制

```typescript
// packages/subagent/subagent/src/depth.ts (推断)
export function assertSubagentMaxDepth(maxDepth?: number): void { ... }
export function delegationDepthOf(agent: Agent): number { ... }
```

每个 Agent 通过 session header 记录其在委托树中的深度。`maxDepth` 参数限制子代理的深度，防止无限递归委托。

**对 laew 的借鉴价值**：

- SubagentProvider 的能力声明模式（capabilities flags）很实用，laew 可以借鉴来管理不同 SubAgent 后端的能力差异
- `toolFilter` 限制子代理工具集的设计值得借鉴，laew 的 SubAgent 可以限制为只持有特定工具
- fork vs spawn 的区分（是否继承父上下文）是关键设计选择，laew 需要类似的上下文传递策略
- `send_message` + `interrupt_agent` 的组合实现了对子代理的持续控制
- resident vs cold resume 模式对 laew 有启发：长任务子代理可以释放内存、持久化 Session，需要时再恢复
- depth 限制（委托深度管理）是防止递归爆炸的必要机制，laew 应在 MultiAgentOrchestrator 中实现

---

## 专题 4：Guard plugin 质检机制

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/guard/repeat-tool-reminder/src/index.ts` | 重复工具调用检测器（advisory，不 veto） |
| `packages/guard/timeout-policy/src/index.ts` | 工具调用超时强制器 |

### 4.1 Guard 设计哲学

deepseek-harness 的 guard 采用 **advisory（建议性）** 而非 **enforcing（强制性）** 模式。两个 guard 都是 Cordis 插件，通过事件水印链（waterfall）协作：

- `repeat-tool-reminder`：检测重复调用，注入提醒消息，**不阻止调用**
- `timeout-policy`：设置 deadline，超时后替换结果，**不取消调用**

### 4.2 repeat-tool-reminder

**触发时机**：`tools/post-execute` 事件（工具调用完成后）

**检测逻辑**：

```typescript
// packages/guard/repeat-tool-reminder/src/index.ts:189-207
function observe(exec: ToolExecution): UserMessage | undefined {
  if (!exec.agent) return undefined
  if (!tracked(exec.name)) return undefined
  const canonical = canonicalize(exec.arguments)  // 深度 key 排序后 JSON.stringify
  const key = JSON.stringify([exec.name, canonical])
  const chain = chains.get(exec.agent)
  const count = chain !== undefined && chain.key === key ? chain.count + 1 : 1
  chains.set(exec.agent, { key, count })
  if (!thresholdSet.has(count)) return undefined  // 未达到阈值
  const text = count === thresholds[0]
    ? GENTLE_REMINDER       // 第一次：温和提醒
    : detailedReminder(...)  // 后续：详细（含工具名、次数、参数预览）
  return createUserMessage({ content: [{ type: 'text', text }], source: PLUGIN_SOURCE })
}
```

**阈值策略**：默认 `[3, 5, 8]`，可配置。第一次（3 次）温和提醒，后续详细提醒并递进。

**用户干预重置**：

```typescript
// index.ts:229-232
ctx.on('agent/pre-step', ({ agent, messages }, next) => {
  if (messages.some(message => message.source.kind === 'user')) chains.delete(agent)
  return next()
})
```

当用户发送消息时，重置该 agent 的重复计数，因为用户干预意味着上下文已改变。

**参数规范化**：深度排序所有 JSON key 后 stringify，确保 `{"b":1,"a":2}` 和 `{"a":2,"b":1}` 被识别为相同参数。

### 4.3 timeout-policy

**触发时机**：`tools/execute` 事件（工具调用前拦截）

```typescript
// packages/guard/timeout-policy/src/index.ts:55-81
export function apply(ctx: Context): void {
  ctx.on('tools/execute', async (exec, next): Promise<ToolExecutionResult> => {
    const timeoutMs = ctx.tools.get(exec.name, exec.agent)?.timeoutMs
    if (timeoutMs === undefined) return next()  // 无超时配置，直接放行

    using d = deadline(exec.signal, timeoutMs, TOOL_TIMEOUT)
    const upstream = exec.signal
    exec.signal = d.signal  // 临时替换为带 deadline 的 signal
    try {
      const result = await next()
      if (timeoutOf(d.signal, TOOL_TIMEOUT) !== undefined) {
        return toolTimeoutResult(timeoutMs)  // 超时，替换结果
      }
      return result
    } finally {
      exec.signal = upstream  // 恢复上游 signal
    }
  })
}
```

超时结果的结构化设计：

```typescript
// index.ts:41-48
function toolTimeoutResult(timeoutMs: number): ToolExecutionResult {
  const message = `tool call timed out after ${timeoutMs}ms`
  return {
    content: [{ type: 'text', text: `Error: ${message}` }],
    isError: true,
    error: { message, info: { name: 'ToolTimeoutError', code: TOOL_TIMEOUT } },
  }
}
```

### 4.4 与其他插件的协作

- **waterfall 链**：`tools/execute` 和 `tools/post-execute` 都是 waterfall 事件，guard 通过 `next()` 委托给下游
- **不 veto 原则**：repeat-tool-reminder 永远调用 `next()`，只在结果上追加提醒
- **超时保护**：timeout-policy 替换 `exec.signal` 后委托，工具本身必须尊重 `signal.aborted`
- **配置来源**：timeoutMs 来自工具定义（`ctx.tools.get(name).timeoutMs`），不是全局配置

### 4.5 Guard 的协作链路图

```
模型发出工具调用
  ↓
tools/execute (waterfall)
  → timeout-policy: 替换 exec.signal → next() → 恢复 signal
    → [其他 guard 拦截器]
      → [实际工具执行]
    ← 工具结果
  ← timeout-policy: 检查是否超时 → 超时则替换结果
← 工具结果
  ↓
tools/post-execute (waterfall)
  → repeat-tool-reminder: observe() 计数 → next() → 追加提醒
    → [其他 post-execute guard]
    ← PostToolDecision（allow/block + additionalContexts）
  ← repeat-tool-reminder: 在 decision 上追加提醒消息
← 最终 decision
```

### 4.6 DeepSeek-harness 的 guard 缺失分析

与 laew 的 Quality-Check Agent 对比，deepseek-harness 的 guard 层明显更轻量：

| 检查类型 | deepseek-harness | laew |
|----------|-----------------|------|
| 安全检查 | 无显式 guard | QC Agent |
| 语法/格式检查 | 无显式 guard | QC Agent |
| 测试验证 | 无显式 guard | QC Agent |
| 工具超时 | timeout-policy（强制） | 无 |
| 重复调用 | repeat-tool-reminder（建议） | 无 |
| 代码质量 | 无 | QC Agent |

deepseek-harness 选择把安全/质量检查留给外部 MCP server 或 CI pipeline，不在 Agent 内部做。

**对 laew 的借鉴价值**：

- laew 的 guard 目前是"硬性质检"（Quality-Check Agent），可以借鉴 advisory 模式：在工具结果后注入提示而非直接阻止
- `repeat-tool-reminder` 的检测模式（canonicalize 参数 + 连续计数 + 多级阈值 + 用户重置）非常适合 Rust 实现
- `timeout-policy` 的 `deadline + signal 替换` 模式可以移植到 laew 的 BashTool 超时控制
- deepseek-harness 的轻量 guard 哲学值得思考：是否所有质检都必须在 Agent 内部？外部化（MCP + CI）可能更灵活
- waterfall 事件链的协作模式（每个 guard 守住自己的职责，不破坏链路）是可扩展性的关键

---

## 专题 5：Context 管理 —— Session Projection Unit

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/session/session-projection/src/index.ts` | SessionProjectionRegistry：投影单元注册表 + 驱动引擎 |
| `packages/session/session-projection/src/types.ts` | SessionProjectionMap / SessionProjectionStateMap 类型表 |
| `packages/session/session-projection-cache/` | 持久化投影缓存 |
| `packages/goal/goal/src/index.ts` | `applyGoalProjection` —— goal 域的投影单元实现 |
| `packages/compaction/compaction/src/index.ts` | CompactionEngine：压缩服务抽象 |
| `packages/compaction/compaction-basic/src/index.ts` | BasicCompactionEngine：基于 token meter 的压缩实现 |
| `packages/compaction/compaction-basic/src/region.ts` | 范围选择与表面区域压缩 |
| `packages/compaction/compaction-basic/src/summarizer.ts` | LLM 摘要生成 |

### 5.1 SessionProjectionRegistry —— 投影单元驱动引擎

**核心概念**：session-projection 是一个 **事件源投影**（event-sourced projection）框架。每个投影单元（ProjectionDefinition）注册一个纯 fold 函数，框架在每个 session event 提交后自动驱动：

```typescript
// packages/session/session-projection/src/index.ts:42-83
export interface ProjectionDefinition<K, S> {
  key: K
  stateSchema: ZodType<S>
  init(header: SessionHeader): S
  apply(state: S, event: SessionEvent): S  // 纯同步状态转换
  wire?: { viewSchema; view(state): View }  // 可选：客户端视图
  stateVersion: number  // 持久化缓存失效版本
}
```

**关键规则**：`apply` 必须是纯同步函数；不关心的事件必须返回相同引用（`Object.is`），利用引用相等性跳过下游通知：

```typescript
// index.ts:614-637
private drive(session: Session, event: SessionEvent): void {
  for (const registration of this.registrations.values()) {
    const next = registration.def.apply(cell.state, event)
    const changed = !Object.is(next, cell.state)  // 引用相等性检测
    cell.state = next
    cell.observedSeq = event.seq
    if (changed && registration.def.wire !== undefined) {
      const value = this.viewCell(registration, cell)
      for (const listener of this.listeners) {
        listener(session, key, value, event.seq)  // 变更通知
      }
    }
  }
}
```

### 5.2 Goal 的投影单元实现

```typescript
// packages/goal/goal/src/index.ts:96-113
export function applyGoalProjection(state: GoalProjection | null, event: SessionEvent): GoalProjection | null {
  if (event.type !== 'goal/change') return state  // 不关心的事件，返回相同引用
  let change = decodeGoalChange(event.data)
  if (change === undefined) return state
  return change.operation === 'clear'
    ? null
    : { goal: change.goal, roundsStarted: change.roundsStarted, createdAt, updatedAt }
}
```

注册：

```typescript
// index.ts:204-213
ctx.inject(['sessionProjections'], (projectionCtx) => {
  projectionCtx.sessionProjections.register<'goal', GoalProjection | null>({
    key: 'goal',
    stateSchema: goalProjectionSchema,
    init: () => null,
    apply: applyGoalProjection,
    wire: { viewSchema: goalProjectionSchema, view: state => state },
    stateVersion: 4,
  })
})
```

### 5.3 持久化投影缓存

```typescript
// session-projection/src/index.ts:377-388
checkpoint(session: Session): ProjectionCheckpoint {
  const rows: ProjectionCheckpoint = {}
  for (const registration of this.registrations.values()) {
    const cell = this.cellFor(registration, session)
    rows[registration.def.key] = {
      ver: registration.def.stateVersion,
      seq: cell.observedSeq,
      val: structuredClone(cell.state),  // 结构化克隆，永不暴露活引用
    }
  }
  return rows
}
```

`restoreFloor` 计算需要重放的最早 seq；`restore` 从 checkpoint + 事件尾部重建状态；`hydrate` 将恢复结果安装到活 Session。

### 5.4 CompactionEngine —— 上下文压缩服务

CompactionEngine 是抽象基类，提供三个核心方法：

```typescript
// packages/compaction/compaction/src/index.ts:96-170
export abstract class CompactionEngine extends Service {
  abstract compactIfNeeded(agent, trigger, signal): Promise<CompactionResult | null>
  abstract compactNow(agent, signal, sourceCommandId?): Promise<CompactionResult | null>
  abstract compactRegion(start, end, agent, signal?): Promise<CompactionResult>
}
```

**触发时机**（自动模式）：

```typescript
// packages/compaction/compaction-basic/src/index.ts:137-165
private _registerAutomaticCompaction(): void {
  // 步骤压力：每步开始前检查
  ctx.on('agent/pre-step', async ({ agent, signal }, next) => {
    const result = await this.compactIfNeeded(agent, 'pressure', signal)
    return next()
  })

  // 上下文溢出：LLM 返回 context_window_exceeded 时触发
  ctx.on('agent/request-error', async ({ agent, failure, signal }, next) => {
    if (failure.code !== CONTEXT_WINDOW_EXCEEDED_CODE) return next()
    const result = await this.compactIfNeeded(agent, 'context-overflow', signal)
    if (result !== null) return { kind: 'retry' }  // 重试请求
    return next()
  })
}
```

### 5.5 BasicCompactionEngine 的压缩流程

```typescript
// compaction-basic/src/index.ts:258-332
override async compactIfNeeded(agent, trigger, signal) {
  const meter = this.ctx.tokenMeter
  let measurement = meter.measure(agent.session)

  // 1. 修剪工具结果（可选）
  const prune = this.ctx.get('toolResultPruner')
  if (prune !== undefined) { prune.pruneSession(agent.session); measurement = meter.measure(...) }

  // 2. 检查是否超过阈值
  if (measurement.totalTokens < spec.thresholdTokens) return null

  // 3. 选择可压缩范围
  const range = selectCompactableRange(agent.session, measurement, spec.retainTokens)

  // 4. 用 LLM 摘要 + 替换表面区域
  return this.compactRegion(range.start, range.end, agent, signal)
}
```

**token 预算管理**：通过 `thresholdRatio`（如 0.8）和 `retainRatio` / `retainTokens` 控制：
- `thresholdTokens = contextWindow * thresholdRatio` — 超过此值触发压缩
- `retainTokens` — 保留最近的 N 个 token 不压缩
- `selectCompactableRange` — 在保留尾部之外选择平衡的（tool call/result 成对）范围

### 5.6 工具结果修剪器

`compaction-tool-result-pruner` 是一个独立的可选服务，独立于压缩引擎工作。它在压缩之前先修剪大工具结果（如长文件读取、长命令输出），降低 token 使用量。

### 5.7 工具配对保护

压缩的一个关键约束是 **tool call/result 必须成对**。`selectCompactableRange` 通过 `toolPairingBalancedBefore` / `toolPairingBalancedAfter` 检查边界：

```typescript
// packages/compaction/compaction/src/tool-pairing.ts (推断)
export function toolPairingBalancedBefore(session, seq): boolean
export function toolPairingBalancedAfter(session, seq): boolean
// 检查 seq 位置是否处于一个平衡的（未匹配的 tool call 之外的）位置
```

如果范围边界落在一个 tool call 和它的 result 之间，压缩会破坏配对，这是不允许的。

### 5.8 压缩的 session 事件

压缩产生三个 session 事件：

```typescript
// packages/compaction/compaction/src/types.ts:16-89
'compaction/start'   // 锁定标记，防止并发压缩
'compaction/summary' // 摘要内容 + 影子范围 + token 统计 + LLM 调用元数据
'compaction/prune'   // 模型无关的修剪（无摘要，直接删除）
```

`compaction/summary` 事件携带完整的 LLM 调用元数据（provider、model、maxTokens、usage），支持从日志重建摘要。

### 5.9 多模型策略

```typescript
// packages/compaction/compaction-basic/src/index.ts:82-93
const modelPolicy: z<ModelCompactPolicyConfig> = z.object({
  provider: z.string().required(),
  model: z.string().required(),
  thresholdRatio: thresholdRatioSchema,
  retainRatio: retainRatioSchema,
  retainTokens: retainTokensSchema,
  summarizationProvider: summarizationProviderSchema,  // 可以用不同模型做摘要
  summarizationModel: summarizationModelSchema,
  maxTokens: maxTokensSchema,
  compactionRetries: compactionRetriesSchema,
  maxOverflowRetries: maxOverflowRetriesSchema,
})
```

每个 provider/model 组合可以有不同的压缩策略（阈值、保留量、摘要模型）。例如 deepseek-chat 可以用更便宜的模型做摘要。

### 5.10 持久化投影缓存的恢复算法

```typescript
// session-projection/src/index.ts:406-416
restoreFloor(checkpoint): number | undefined {
  let floor: number | undefined
  for (const registration of this.registrations.values()) {
    const row = checkpoint[registration.def.key]
    const need = row !== undefined && row.ver === registration.def.stateVersion
      ? Math.max(row.seq + 1, 0)  // 从 checkpoint 的下一个 seq 开始
      : 0                          // version 不匹配，从头 fold
    floor = floor === undefined ? need : Math.min(floor, need)
  }
  return floor === undefined ? undefined : Math.max(floor - 1, 0)  // "one-below anchor"
}
```

**one-below anchor** 的设计精妙：返回 `floor - 1` 而非 `floor`，让持久层从 `floor - 1` 开始读取，这样可以检测日志是否缩小到了 checkpoint 的 watermark 以下（crash-repair truncation）。

**对 laew 的借鉴价值**：

- **SessionProjectionRegistry 的投影单元模式**非常值得借鉴：定义一个纯 fold 函数，框架自动在每个事件后驱动。laew 可以用类似模式实现 goal、todo 等状态的自动跟踪
- **引用相等性优化**（`Object.is`）是一个巧妙的性能技巧，Rust 可以用 `Rc::ptr_eq` 或自定义 `PartialEq` 实现
- **Compaction 的双触发**（步前压力 + context-overflow 恢复）是成熟的做法，laew 应该实现类似的 token 管理
- **`selectCompactableRange` 的平衡范围选择**（保持 tool call/result 成对）很重要，laew 也需要这个约束
- 持久化投影缓存（checkpoint/restore）的版本管理（stateVersion）确保了向前兼容
- 多模型压缩策略（每个 provider/model 独立配置）适合 laew 的多 Agent 架构（不同 Agent 可用不同压缩策略）
- **compaction/summary 事件携带 LLM 调用元数据** 的设计支持日志重建，laew 的 session 记录应考虑类似的完整性

---

## 专题 6：MCP 架构

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/mcp/mcp-client/src/index.ts` | MCP 客户端插件入口：配置验证 + 命名空间预留 + 连接启动 |
| `packages/mcp/mcp-client/src/connection.ts` | ConnectionSupervisor：连接管理 + 指数退避重连 |
| `packages/mcp/mcp-client/src/transport.ts` | 传输层工厂：stdio / Streamable HTTP |
| `packages/mcp/mcp-client/src/tools.ts` | Tool bridge：工具发现 + 注册 + 执行 + 结果映射 |

### 6.1 插件结构

```typescript
// packages/mcp/mcp-client/src/index.ts:29-33
export const name = 'mcp-client'
export const inject = ['tools']
```

每个 MCP server 对应一个独立的插件实例。支持两种传输：

```typescript
// index.ts:50-95
export interface StdioConfig {
  transport: 'stdio'
  serverName: string       // mcp__<serverName>__<rawName> 的命名空间
  command: string
  args: string[]
  env: Record<string, string>
  toolCallTimeoutMs: number  // 默认 60s
}

export interface StreamableHttpConfig {
  transport: 'streamable-http'
  serverName: string
  url: string
  headers: Record<string, string>
}
```

### 6.2 传输层实现

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

**凭证安全**：`scrubbedParentEnv()` 从父进程环境变量中剔除凭证类变量，只传递安全的环境变量给子进程。

### 6.3 连接管理器 —— 指数退避重连

```typescript
// packages/mcp/mcp-client/src/connection.ts:123-351
export function startConnection(ctx: Context, config: Config, policy): ConnectionHandle {
```

**重连策略**：

```typescript
// connection.ts:40-45
export const RECONNECT_DEFAULTS = {
  enabled: true,
  initialDelayMs: 500,      // 初始延迟
  maxDelayMs: 30_000,       // 最大延迟（= 稳定性窗口）
  maxAttempts: 10,          // 最大连续失败次数
}
```

**退避算法**：

```typescript
// connection.ts:216
const delayMs = Math.min(policy.maxDelayMs, policy.initialDelayMs * 2 ** (failedAttempts - 1))
```

500ms → 1s → 2s → 4s → 8s → 16s → 30s（封顶）

**稳定性窗口重置**：

```typescript
// connection.ts:203
if (connectedAt !== undefined && Date.now() - connectedAt >= policy.maxDelayMs) failedAttempts = 0
```

连接持续超过 `maxDelayMs` 后重置失败计数，下次断连开始新的预算。

**放弃逻辑**：`failedAttempts > maxAttempts` 后注销所有工具，停止重连。

### 6.4 工具注册流程

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

**命名规则**：

```typescript
// tools.ts:111-117
export function publicToolName(serverName: string, rawName: string): string {
  const joined = `mcp__${serverName}__${rawName}`
  const normalized = joined.replace(INVALID_NAME_CHARS, '_')
  if (normalized === joined && normalized.length <= 64) return normalized
  const hash = createHash('sha256').update(`${serverName}\0${rawName}`).digest('hex').slice(0, 12)
  return `${normalized.slice(0, 51)}_${hash}`  // 截断 + 哈希避免冲突
}
```

### 6.5 工具执行

```typescript
// tools.ts:303-361
function createExecutor(client, ctx, rawName, taskRequired, opts, projections) {
  return async (args, exec) => {
    if (taskRequired) throw new Error('task-based execution not supported')
    const argsObj = typeof args === 'object' && args !== null ? args : {}
    const result = await callToolUncached(client, rawName, argsObj, exec, opts)

    if (result.isError === true) throw new Error(text)  // MCP isError → throw → isError tool result

    const value = { content, ...structuredContent }
    if (containsImage(content)) {
      // 异步图片投影：解码 → 持久化存储 → 替换为 attachment ref
      const projected = await prepareImageProjection(ctx, exec, content, rawName)
      projections.set(exec, { value, fallback, content: projected })
    }
    return value
  }
}
```

### 6.6 工具列表变更通知

```typescript
// connection.ts:257-269
generation.setNotificationHandler(ToolListChangedNotificationSchema, async () => {
  if (!isCurrent(generation)) return
  ctx.logger.info(`${label}: tool list changed, re-syncing`)
  await enqueueSync(generation)
})
```

MCP server 发送 `notifications/tools/list_changed` 时，客户端自动重新同步工具列表。

### 6.7 生命周期管理

- **启动**：`apply()` 中 `await connection.ready`，`failOnStartupError` 控制首次失败是否拒绝插件
- **HMR**：effect-scoped 注册，HMR 时旧实例 dispose（断开连接 + 注销工具），新实例重建
- **停止**：`dispose()` 先停止重连定时器，关闭当前 client，等待 in-flight 操作，注销所有工具

### 6.8 图片投影 —— MCP 结果中的图片处理

MCP server 可以返回图片（base64 编码），deepseek-harness 的处理流程：

```typescript
// packages/mcp/mcp-client/src/tools.ts:433-487
async function prepareImageProjection(ctx, exec, content, toolName): Promise<ContentBlock[]> {
  // 1. 解码所有 image 块（验证 MIME type 和 base64 合法性）
  for (const [index, value] of content.entries()) {
    decoded.push(decodeImage(value as McpContentBlock))  // 严格验证 PNG/JPEG/WebP/GIF + 规范 base64
  }

  // 2. 检查当前模型是否支持图片输入
  attachments = await resolveImageAdmission(ctx, exec)  // 验证 model.inputModalities 包含 'image'

  // 3. 持久化存储图片
  const refs = await attachments.saveImages(decoded)

  // 4. 替换 base64 数据为 attachment ref
  return projectContent(content, toolName, (_block, index) => ({
    type: 'image', attachment: byIndex.get(index),
  }))
}
```

**安全边界**：MCP server 是不可信的外部进程，所有返回数据都经过严格验证：
- MIME type 必须是 `image/png | image/jpeg | image/webp | image/gif`
- base64 必须是规范形式（RFC 4648），不接受 URL-safe 变体
- 解码后重新编码验证一致性

### 6.9 不支持的内容类型的降级处理

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

所有不支持的 MCP 内容类型都降级为文本描述，而不是静默丢弃。这保证模型总能看到结果。

### 6.10 结构化输出（PTC 模式）

```typescript
// tools.ts:275-291
function createOutput(rawName, structuredSchema) {
  return {
    schema: {
      type: 'object',
      properties: {
        content: { type: 'array', items: {} },
        structuredContent: structuredSchema ?? {},
      },
      required: structuredSchema === undefined ? ['content'] : ['content', 'structuredContent'],
    },
    render(_args, value) {
      return [{ type: 'text', text: extractText(result.content, rawName) }]
    },
  }
}
```

MCP 工具支持两种输出模式：text 模式（默认，提取纯文本）和 PTC 模式（structured content，保留原始 JSON 结构）。

**对 laew 的借鉴价值**：

- MCP 的**命名空间**设计（`mcp__<serverName>__<rawName>`）值得借鉴，laew 可以用类似方式管理不同来源的工具
- **指数退避重连**（500ms → 30s，最大 10 次尝试）是生产级实现，laew 的 MCP 支持应采用类似策略
- **两阶段同步**（fetch + swap）保证了工具列表更新的原子性，避免模型看到不完整的工具集
- **凭证清洗**（scrubbedParentEnv）是安全最佳实践，laew 在启动子进程时也应实现
- **图片投影**（解码 → 持久化存储 → attachment ref）处理了大结果的上下文管理，laew 的 ReadTool 可以借鉴
- **不支持内容的降级策略**（全部变为文本而非丢弃）是好的 UX 实践，laew 的 MCP 实现应遵循
- **serverName 命名空间冲突检测**（`activeServerNames` WeakMap）防止同名 server 冲突，laew 应实现类似机制

---

## 跨专题设计模式总结

### 模式 1：Effect-Scoped 资源管理

贯穿所有模块：注册、监听、连接都是 effect-scoped，disposer 自动运行在 fiber 卸载时。

```typescript
// 通用模式（出现在所有包中）
ctx.effect(() => {
  const dispose = ctx.on('event', handler)   // 注册资源
  return () => dispose()                      // 返回清理函数
}, 'label for diagnostics')
```

Rust 等价物是 RAII + Drop，但 Cordis 的 effect 支持异步清理（Promise），这是 Rust 的 `async Drop` 目前不具备的。

### 模式 2：Capability Seam（能力接缝）

Service 定义、Provider 注册、消费者三层分离：

```
Service Definition (dsh-subagent)     ← 类型接口 + 注册表
  ↑ registerProvider()
Service Provider (dsh-subagent-fork)  ← 具体实现
  ↑ inject
Consumer (dsh-tool-subagent)          ← 模型面对的工具
```

其他例子：
- `CompactionEngine`(抽象) ← `BasicCompactionEngine`(实现) ← `command-compact`(消费者)
- `SessionProjectionRegistry`(注册表) ← 各域的 `ProjectionDefinition`(实现) ← 客户端 snapshot(消费者)

### 模式 3：Event-Sourced Projection

Goal 和其他域的状态通过纯 fold session 事件重建，持久化 checkpoint 加速恢复。

```
Session Events → fold(state, event) → current state → view(state) → wire value
                  ↑ apply()             ↑ cell cache    ↑ view()
```

### 模式 4：Fail-Loud 配置验证

所有配置通过 schemastery（Zod-like）验证，不匹配直接抛异常：

```typescript
export const Config: z<Config> = z.object({
  serverName: z.string().required().pattern(SERVER_NAME_PATTERN),
  toolCallTimeoutMs: z.number().default(60_000),
})
```

每个 `invariant.ts` 文件在开发模式下运行断言，检测违反不变量的情况。Rust 中用 `serde` + 自定义 validator + `debug_assert!`。

### 模式 5：Waterfall 协作

Guard 通过 waterfall 事件链协作，每个 guard 可以在调用前后注入行为但不破坏链路：

```typescript
ctx.on('tools/execute', async (exec, next) => {
  // 前置处理
  exec.signal = modifiedSignal
  try {
    const result = await next()  // 委托给下游
    // 后置处理
    return result
  } finally {
    exec.signal = originalSignal  // 恢复
  }
})
```

### 模式 6：Scoped Provider + Namespace Reservation

MCP 和 SubAgent 都使用命名空间预留 + 作用域隔离：

```typescript
// MCP：每个 serverName 在作用域内唯一
const activeServerNames = new WeakMap<object, Set<string>>()
// SubAgent：每个 provider name 在全局唯一
private providers = new Map<string, SubagentProvider>()
```

### 模式 7：CAS（Compare-And-Set）乐观并发

Goal 域的每次变更都要求提供当前 ref `{id, revision}`，不匹配则拒绝。这避免了锁，适合事件源架构。

### 模式 8：Serialized Driver Loop

round driver 和 connection supervisor 都使用 "requested flag + single runner" 模式：

```typescript
function requestDrive(state) {
  state.requested = true
  if (state.run !== undefined) return  // 已有 runner
  state.run = loop()
}
async function loop() {
  while (state.requested) {
    state.requested = false
    await doWork()
  }
  state.run = undefined
}
```

### 模式 9：Last-Wins Whole-Value Rule

session 事件携带完整快照而非增量：

```
goal/create → {完整 GoalSnapshot}
goal/edit   → {完整 GoalSnapshot}
goal/block  → {完整 GoalSnapshot}
```

fold 只需 "last wins"，不需要增量合并。这简化了投影逻辑和缓存恢复。

### 模式 10：Scoped Shadow（作用域影子）

```typescript
isolate(name, label) → 创建子 Context，该服务的读写在新 scope 内
intercept(name, config) → 为子树内的插件覆盖服务配置
```

这允许同一个插件在不同子树中有不同的行为（例如不同 Agent 看到不同的工具集）。

---

## 对 laew 架构的整体建议

基于以上分析，以下是按优先级排序的借鉴建议：

### P0（立即可做）

1. **实现 timeout-policy guard**：BashTool 的超时是高频问题，Rust 的 `tokio::time::timeout` 可以直接实现
2. **实现 repeat-tool-reminder**：检测模型陷入重复调用循环，注入提醒消息

### P1（中期规划）

3. **Session Event 投影单元**：为 Goal/Todo 等域实现纯 fold 函数 + 注册表，替代当前的数据库查询
4. **SubAgent 能力声明**：为每个 SubAgent 后端声明 capabilities，统一能力检查
5. **MCP 命名空间管理**：工具名加前缀 `mcp__<serverName>__<rawName>`，防止冲突

### P2（长期参考）

6. **Compaction 机制**：实现 token 预算管理 + 自动压缩 + context-overflow 恢复
7. **Cordis 风格的依赖注入**：虽然 Rust 生态不同，但 "服务注册表 + 自动依赖解析 + 生命周期管理" 的思路可以借鉴
8. **SubAgent 冷恢复**：长任务子代理的内存释放 + Session 持久化 + 按需恢复
