# deepseek-harness 源码深度分析

> 调研目标：`/usr/local/LsmGitOpenSource/deepseek-harness`（DeepSeek 出品的 LLM Agent CLI / Harness 框架，TypeScript + pnpm monorepo，~80+ 包）
>
> 调研范围：`packages/{core,llm,goal,plan,guard,subagent,mcp,skill,typert,core/session}/...`
>
> 调研目的：与 `LsmAgentEmergentWork`（Rust `laew`）做架构对标，提炼可借鉴模式与设计取舍。
>
> 本文按 8 个维度组织，每个维度给出关键文件路径、行号锚点、关键代码片段、设计要点。

---

## 1. 多轮对话的实现 — `ReactLoopAgent` 状态机与 Inbox 模型

### 1.1 三相位状态机

deepseek-harness 的 Agent 主循环不是简单的 `while (true) { call() }`，而是 **三相位状态机**（`idle / maintenance / running`），所有状态切换都通过 `setPhase` 集中发布 `agent/status` 事件。

**关键文件**：`packages/core/agent-loop/src/agent.ts`

**核心类型**（`packages/core/agent-loop/src/agent.ts:38-46`）：

```typescript
type Phase =
  | { kind: 'idle'; lastTurn: number }
  | {
    kind: 'maintenance'
    abort: AbortController
    lastTurn: number
    wakeRequested: boolean
  }
  | { kind: 'running'; abort: AbortController; turn: number; step: number; wakeRequested: boolean }
```

**状态机入口**（`agent.ts:106-118`）：

```typescript
get status(): AgentStatus {
  return this.phase.kind === 'idle' || this.phase.kind === 'maintenance' ? 'idle' : 'running'
}

private setPhase(next: Phase): void {
  const previousStatus = this.status
  this.phase = next
  const status = this.status
  if (status !== previousStatus) {
    this.dispatch.emit('agent/status', { status })
  }
}
```

> 设计要点：**status 字段只读、仅在 `setPhase` 内推进**，杜绝了「读 phase 后 race condition」的窗口。所有生命周期事件都集中走 `agentEvents(this.loopCtx, this)` 派发器（`agent.ts:93`）。

### 1.2 Turn / Step 三层边界

`turn()` 是循环入口，每次推进 `turn + 1`；每个 turn 内可有多次 `step`（即多次 LLM 调用 + tool use）。`max-tokens` 状态「黏性」向上传递：

```typescript
// agent.ts:294-297
const stepEnd = await this.step(decision.assembly, decision.startsRequestSeries === true)
// max-tokens stays sticky: a later completed step must not downgrade the turn outcome.
if (turnEnds === null || turnEnds.kind !== 'max-tokens') turnEnds = stepEnd
```

> 设计要点：`turn` = 一次用户消息对应的完整响应；`step` = 一次 LLM 调用（含可能的多 tool call）。状态用 `turnEnds` 累加，下层 step 不允许覆盖上层的「失败结论」。

### 1.3 Inbox 模型 — 两个有序待发队列

**关键文件**：`packages/core/agent/src/inbox.ts`

`Inbox` 维护两个有序 `UserMessage[]`：`next-turn` 与 `next-step`（`inbox.ts:25-26`），并通过 **持久化事件投影** 重建：

```typescript
// inbox.ts:32-39
constructor(
  private readonly session: Session,
  private readonly notifications: InboxNotifications,
) {
  for (const event of session.events.slice(session.header.seedLength ?? 0)) {
    if (event.type !== 'agent/inbox/spliced') continue
    try {
      this.apply(event.data)
    } catch (error: unknown) {
      throw new Error(`invalid persisted inbox splice at session seq ${event.seq}`, { cause: error })
    }
  }
}
```

**核心操作**（`inbox.ts:139-193`）：

```typescript
splice(target, start, deleteCount, inserted): UserMessage[] {
  return this.mutate(target, start, deleteCount, inserted, true)
}

private mutate(target, start, deleteCount, inserted, discardRemoved): UserMessage[] {
  // ...
  const event = this.session.append('agent/inbox/spliced', splice)  // 先 commit
  const removed = inbox.splice(actualStart, actualDeleteCount, ...event.data.inserted)  // 再 mutate
  if (discardRemoved) for (const message of removed) this.notifications.discarded(message)
  for (const message of event.data.inserted) this.notifications.inserted(message)
  return removed
}
```

**关键设计**：

- **「先 durable commit，再 live mutate」**：观察者读到的是 pre-splice 状态，可以从 normalized 坐标还原被移除的消息。
- **去重校验**：`validate()` 用 `Set<messageId>` 跨两个 target 校验（`inbox.ts:203-219`），重复 id 直接抛错。
- **claim 语义**（`inbox.ts:71-78`）：`claim(target, turn)` 把 `next-step` 全量取出，`target === 'next-turn'` 时再取一个 turn。模型在 `preStep` 入口调用一次（`agent.ts:236`）。

### 1.4 Inbox 输入边界

**关键文件**：`packages/core/agent/src/runtime-types.ts`

`Agent` 公开 4 种输入语义（`runtime-types.ts:122-149`）：

| API | target | wakeup | 用途 |
|-----|--------|--------|------|
| `followup(input)` | next-turn | true | 普通追加下一轮 |
| `steer(input)` | next-step | true | 中途插入下一步上下文 |
| `inject(input)` | next-step | false | 模型上下文注入，不唤醒 |
| `send(input, target, wakeup)` | 自选 | 自选 | 通用入口 |

**cancel 重入陷阱处理**（`agent.ts:120-127`）：

```typescript
send(message, target, wakeup): void {
  // Waking input cannot join an aborted activity, so it starts the next turn.
  const wakingAfterAbort = wakeup && this.phase.kind !== 'idle' && this.phase.abort.signal.aborted
  const resolvedTarget = wakingAfterAbort ? 'next-turn' : target
  this.inbox.splice(resolvedTarget, Infinity, 0, [message])
  if (wakeup) this.wakeDriver(wakingAfterAbort)
}
```

> 设计要点：在 inbox 写入之前就锁定 `wakingAfterAbort`，避免观察者 cancel 时再分类导致重入歧义。

### 1.5 驱动唤醒与锁存

**关键代码**（`agent.ts:179-200`）：

```typescript
private wakeDriver(wakeAfterAbort = false): void {
  if (this.phase.kind !== 'idle') {
    // Maintenance and aborted drivers cannot deliver the wake: latch it for replay at convergence.
    const reason = this.phase.abort.signal.reason as AgentCancelCause | undefined
    if (reason?.kind !== 'disposed' && (this.phase.kind === 'maintenance' || wakeAfterAbort)) {
      this.phase.wakeRequested = true
    }
    return
  }
  const driver = Promise.withResolvers<void>()
  this.activityDone = driver.promise
  this.setPhase({ kind: 'running', abort: new AbortController(), turn: this.phase.lastTurn, step: 0, wakeRequested: false })
  this.loopCtx.agents.withInitiator(this, () => this.kick()).then(driver.resolve, driver.reject)
}
```

> 设计要点：仅在 `idle` 相位才会启动 driver；其它相位（`maintenance` / `aborted`）的唤醒被 `wakeRequested` 锁存，等收敛后再 replay；`disposed` 永不锁存，teardown 不再等模型回包。

### 1.6 Pre-Step / Step / Turn 钩子点

**关键文件**：`packages/core/agent/src/runtime-types.ts:225-298`

完整扩展点清单：

| 事件 | 模式 | 用途 |
|------|------|------|
| `agent/pre-step` | waterfall | 拒绝/改写 step 输入消息 |
| `agent/request` | waterfall | 替换请求配置（provider/model） |
| `agent/request-error` | waterfall | 重试决策 |
| `agent/turn-stopping` | serial | 收口前最后一次机会，可 steer |
| `agent/error` | emit | 错误通知 |
| `agent/status` | emit | 生命周期 |
| `agent/session-start` | emit | 会话开始（startup/resume/clear/compact） |

`turn-stopping` 的设计哲学（`runtime-types.ts:269-285` 注释）：

> "Data decides, so listener order cannot change the outcome. The inverse control (stop a tool loop early) is data too: a tool result carrying `concludesTurn` ends the turn at its step."

### 1.7 LLM 适配的 prepareCall 冻结

**关键代码**（`agent.ts:486-493`）：

```typescript
try {
  preparedCall = await this.loopCtx.llm.prepareCall(proposedConfig, signal)
  config = preparedCall.config
} catch (error: unknown) {
  // Middleware may serve an unregistered route; terminal dispatch still requires an adapter.
  if (!(error instanceof LlmError) || error.code !== 'NO_ADAPTER') throw error
  config = proposedConfig
}
```

> 设计要点：**每次 step 冻结一次调用配置**（adapterDefaults），跨 step 的 `request/header` 变更会触发 `header/reason: 'change'` 事件（`agent.ts:508-516`），保证「在飞请求不会被中途切模型」。

### 1.8 与 `laew` 对标

| laew | deepseek-harness |
|------|------------------|
| `Session` 内存态 context | `Session` 持久化事件日志 + 内存投影 |
| `Agent.run_session` 单循环 | `ReactLoopAgent` 三相位状态机 + 持久化 inbox |
| 工具调用回填到 context.messages | tool call 落 session log，下一轮从 log 派生 messages |
| 单一 wakeup（followup） | 4 种语义（followup / steer / inject / send） |

**借鉴价值**：① 三相位状态机解决 cancel 重入；② inbox 持久化使 resume / fork 可零成本还原；③ turn/step sticky end reason 防止 max-tokens 被降级。

---

## 2. Context 的管理和实现 — Goal 目标栈 / Plan-mode / Session 持久化

### 2.1 Session 持久化日志

deepseek-harness 的 `Session` 是 **不可变 append-only 事件流**，所有 Agent / Goal / Plan 状态都从它 fold 而来。这与 laew 的「内存 context + SQLite 元数据」形成鲜明对比。

**关键设计**：

- **whole-value replace**：`plan/mode` 事件只写 `{ active: boolean }`，fold 取最后一个（`plan-mode/src/index.ts:129-138`）：

```typescript
export function foldPlanMode(events: readonly SessionEvent[], end = events.length): boolean {
  let active = false
  let index = 0
  for (const event of events) {
    if (index >= end) break
    index++
    if (event.type === 'plan/mode') active = event.data.active
  }
  return active
}
```

- **session-event vocabulary 声明合并**：通过 `declare module '@deepseek-ai/dsh-session/types' { interface SessionEventMap { 'plan/mode': { active: boolean } } }`（`plan-mode/src/index.ts:46-55`）把事件类型**注入**到核心类型表，第三方包只需 import 类型模块即可看到。

### 2.2 Goal 目标栈 — 同一 session 内多轮自动续作

**关键文件**：`packages/goal/goal/src/index.ts`

**域对象**（`goal/src/domain.ts:14-41`）：

```typescript
export type GoalOperation = 'create' | 'edit' | 'pause' | 'resume' | 'complete' | 'block' | 'clear'

export interface GoalSnapshotChangeMeta {
  readonly kind: 'goal/change'
  readonly version: 1
  readonly operation: Exclude<GoalOperation, 'clear'>
  readonly goal: GoalSnapshot
  readonly roundsStarted: number
  readonly createdAt: number
  readonly updatedAt: number
}
```

**GoalService 核心契约**（`goal/src/index.ts:183-214`）：

```typescript
export class GoalService extends TypertRemoteService {
  static inject = ['agents']
  static Config: z<Config> = z.object({
    defaultMaxGoalRounds: z.number().default(256),
  })

  private readonly resolved: ResolvedConfig
  private readonly caches = new WeakMap<Session, GoalCache>()

  constructor(ctx: Context, config: Config = {}) {
    super(ctx, 'goals')
    this.resolved = { defaultMaxGoalRounds: resolveMaxGoalRounds(config.defaultMaxGoalRounds ?? 256) }
    ctx.on('agent/session-start', ({ agent }) => {
      this.cache(agent.session).activation = 'disarmed'
    })
    // 注册 'goal' projection unit
    ctx.inject(['sessionProjections'], (projectionCtx) => {
      projectionCtx.sessionProjections.register<'goal', GoalProjection | null>({
        key: 'goal', stateSchema: goalProjectionSchema, init: () => null,
        apply: applyGoalProjection, wire: { viewSchema: goalProjectionSchema, view: state => state },
        stateVersion: 4,
      })
    })
  }
}
```

**六种操作 + 修订号管理**（`goal/src/index.ts:251-389`）：

| 操作 | 允许前置 phase | 激活副作用 |
|------|----------------|-----------|
| `create` | 无 current 或 current.phase === 'complete' | armed |
| `edit` | 任意 phase | 保留原激活 |
| `pause` | active | disarmed |
| `resume` | active/paused/blocked 且 round < cap | armed |
| `complete` | active/paused/blocked | disarmed |
| `block` | active | disarmed |
| `clear` | 任意 phase（保留 tombstone） | disarmed |

**CAS 修订号**（`goal/src/index.ts:401-411`）：

```typescript
private expectCurrent(cache: GoalCache, ref: GoalRef): GoalSnapshot {
  const current = cache.state.goal
  if (current === undefined) throw new GoalError('no current goal', 'GOAL_NOT_FOUND')
  if (ref.id !== current.id || ref.revision !== current.revision) {
    throw new GoalError(`stale goal ref "${ref.id}" revision ${ref.revision}`, 'GOAL_STALE_REVISION')
  }
  return current
}
```

> 设计要点：所有 mutation 都用 `ref` 做 CAS，避免并发提交产生 revision 漂移。

**commit 流程**（`goal/src/index.ts:542-558`）：

```typescript
private commit(agent, cache, change, activation): void {
  const ref = goalChangeRef(change)
  cache.pendingActivation = { seq: agent.session.seq, activation }  // 同步预占
  try {
    agent.session.append('goal/change', change)  // 持久化
    this.sync(agent.session, cache)               // 增量 fold
  } finally {
    cache.pendingActivation = undefined
  }
  // 广播 goal/changed 事件
  agentEvents(this.ctx, agent).emit('goal/changed', { change: notification })
}
```

### 2.3 Goal 续作驱动 — Round Driver

**关键文件**：`packages/goal/goal-round-driver/src/index.ts`

`GoalService` 是「数据」层；`goal-round-driver` 是「调度」层：

```typescript
// goal-round-driver/src/index.ts:76-77
export function apply(ctx: Context): void {
  const states = new Map<Agent, DriverState>()
  // 监听 pre-step，自动注入新一轮消息
  ctx.on('agent/pre-step', ...)
}
```

> 设计要点：**Goal 与 Round Driver 解耦**：Goal 维护状态机，Round Driver 负责把 armed goal 转成 inbox 消息。laew 的「SessionContext 摘要写入数据库」类似但更弱。

### 2.4 Plan-mode — 日志优先的协作模式

**关键文件**：`packages/plan/plan-mode/src/index.ts`

**PlanProjection wire**（`plan-mode/src/types.ts:19-22`）：

```typescript
export interface PlanProjection {
  active: boolean
  pending: boolean
}
```

**pending 状态的 fold 规则**（`plan-mode/src/index.ts:266-291`）：

```typescript
apply: (state, event) => {
  if (event.type === 'command/run' && event.data.name === 'plan') {
    if (event.data.args === undefined) return state
    const wanted = event.data.args.trim() !== 'off'
    return { ...state, running: { commandId: event.data.commandId, wanted } }
  }
  if (event.type === 'command/done' && event.data.commandId === state.running?.commandId) {
    const wanted = event.data.kind === 'success' && state.running.wanted !== state.active
      ? state.running.wanted
      : null
    return { ...state, wanted, running: null }
  }
  if (event.type === 'plan/mode') {
    return { ...state, active: event.data.active, wanted: null }
  }
  return state
}
```

> 设计要点：**pending 完全从日志 fold 出来**，无 live mirror。host 重启、cold read 都能恢复。
>
> 注释（`plan-mode/src/index.ts:253-260`）：
> > "The plan projection unit (session-projection RFC): a pure event fold serving clients the whole {active, pending} value. ... Pending is thereby a pure replay quantity: host restarts, other tabs, and cold reads all recover it from the log alone."

**PlanModeController 钩子点**（`plan-mode/src/index.ts:223-240`）：

```typescript
ctx.on('agent/pre-step', async ({ agent, signal }, next): Promise<PreStepDecision> => {
  const decision = await next()
  const pending = this.pendingIntents.get(agent.session)
  if (decision.kind === 'reject' || signal.aborted || pending === undefined) return decision
  const narration = this.narration(agent.session, pending.active)
  try {
    this.onBoundary(agent.session)  // 写入 plan/mode 事件
  } catch (error) {
    ctx.logger.warn(...)
    return decision
  }
  return !pending.narrate || narration === undefined
    ? decision
    : { ...decision, messages: [...decision.messages, narration] }
})
```

> 设计要点：**计划模式切换发生在 pre-step 边界**，不在 turn-start；这样 turn 内同 step 的请求复用同一 plan 提示词段。

### 2.5 System Prompt 节组装

**关键文件**：`packages/core/agent-loop/src/agent.ts:237`

```typescript
const assembly = await this.loopCtx.systemPrompt.assemble(assembleContextFor(this, signal))
```

Plan-mode 注册一个 section（`plan-mode/src/index.ts:243-251`）：

```typescript
ctx.systemPrompt.section({
  name: 'plan:policy',
  order: FIRST_PARTY_SECTION_ORDER.PLAN_POLICY,
  text: (context) => {
    if (context.agent === undefined) return ''
    const pending = this.pendingIntents.get(context.agent.session)
    return (pending?.active ?? foldPlanMode(context.agent.session.events)) ? this.section : ''
  },
})
```

### 2.6 与 `laew` 对标

| laew | deepseek-harness |
|------|------------------|
| SQLite `session_memory` 表存 Markdown 摘要 | 完整事件日志 fold（projection unit） |
| `YoloRunner` 三档任务分类 | `Goal` 域 + `GoalRoundDriver` 续作调度 |
| `Plan` Agent 写 plans/*.md | `PlanModeController` + `exit_plan_mode` 工具 + `/plan` 命令 |
| `SessionContext` 注入 `<<<LAEW:SESSION_HISTORY>>>` | `goal/change` + `plan/mode` 通过 projection registry wire 出 |

**借鉴价值**：① projection registry 是 session 事件的「可重放视图」抽象；② plan-mode 的 pending 纯 fold 设计避免了「live mirror + log 重放」双源不一致。

---

## 3. Yolo 识别 / 任务分类

> 调研结论：**deepseek-harness 没有 Yolo / 入口层意图识别 Agent**。

`grep -rn "Yolo\|taskLevel\|TaskClassification" packages/` 无任何匹配。

### 3.1 等价物：`Goal` 目标栈 + Round Driver

deepseek-harness 把「任务分类」拆解到 **Goal 域**，由人类显式创建目标，机器自动续作（`goal-round-driver`），无 LLM 介入的意图分类。

| 维度 | laew Yolo | deepseek-harness |
|------|-----------|------------------|
| 触发时机 | 每条用户输入 | 用户显式 `goals.create` |
| 分类方式 | LLM 三步分析（目的→目标→意图）+ 三档 | 编译期类型 + maxGoalRounds 配额 |
| 失败回流 | 用户建议 | round exhausted → `GOAL_INVALID_TRANSITION` 抛错 |
| 入口工具 | 仅 Read | 无（Goal 通过 Typert Remote 暴露给客户端） |

### 3.2 入口层设计的另一种思路

deepseek-harness 把「入口层」拆给 **三个不同层**：

1. **Host 客户端**：浏览器或 CLI 接收用户输入 → 通过 Typert RPC 直接调 `goals.create` / `plan` 命令。
2. **Session 启动**：`agent/session-start` 事件携带 `SessionStartSource = 'startup' | 'resume' | 'clear' | 'compact'`（`runtime-types.ts:69`），由 plugins 各自决定初始化语义。
3. **Tool 层**：`exit_plan_mode`（plan-mode/src/index.ts:342-430）是模型主动调用进入 / 退出的工具。

**核心差异**：laew 用 LLM 判断任务分类（动态但耗 token），deepseek-harness 用「目标域」+「round 配额」做约束（静态但可解释）。

### 3.3 与 `laew` 对标

**借鉴价值**：
- 「入口层 Yolo」未必适合所有架构。如果有显式 goal 域，可省去 LLM 分类开销。
- `GoalMessageSource`（`goal/src/domain.ts:47-53`）为续作消息打标 `kind: 'goal', goalId, revision, round`，便于后续 fold 区分。

---

## 5. 质检检查 — `guard/` 模块与 Guardrail 机制

### 5.1 Guard 包族结构

**关键目录**：`packages/guard/`

```
guard/
├── repeat-tool-reminder/   重复工具调用提醒（advisory）
└── timeout-policy/         工具调用超时策略
```

> 与 laew 的 `Quality-Check Agent` 不同：deepseek-harness 的 guard 是 **可叠加的水管式 plugin**，不强制每个执行单元都过一次 LLM judge。

### 5.2 Repeat-Tool-Reminder — 重复调用检测

**关键文件**：`packages/guard/repeat-tool-reminder/src/index.ts`

**核心配置**（`index.ts:28-50`）：

```typescript
export interface Config {
  thresholds?: number[]           // 默认 [3, 5, 8]
  include?: string[]              // 追踪白名单（glob）
  exclude?: string[]              // 不追踪黑名单（glob）
  argumentsPreviewChars?: number  // 默认 500
}

export const Config: z<Config> = z.object({
  thresholds: z.array(z.number()).default([3, 5, 8]),
  include: z.array(z.string()).default([]),
  exclude: z.array(z.string()).default([]),
  argumentsPreviewChars: z.number().default(500),
})
```

**canonical 参数比对**（`index.ts:89-105`）：

```typescript
function sortJsonValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortJsonValue)
  if (value !== null && typeof value === 'object') {
    const record = value as Record<string, unknown>
    const sorted: Record<string, unknown> = {}
    for (const key of Object.keys(record).sort()) sorted[key] = sortJsonValue(record[key])
    return sorted
  }
  return value
}
function canonicalize(argumentsValue: unknown): string {
  return JSON.stringify(sortJsonValue(argumentsValue))
}
```

> 设计要点：**深 key-sort 后 stringify**，避免 `{a:1,b:2}` 与 `{b:2,a:1}` 误判。

**链式计数 + 两档提醒**（`index.ts:189-225`）：

```typescript
function observe(exec: ToolExecution): UserMessage | undefined {
  if (!exec.agent) return undefined
  if (!tracked(exec.name)) return undefined
  const canonical = canonicalize(exec.arguments)
  const key = JSON.stringify([exec.name, canonical])
  const chain = chains.get(exec.agent)
  const count = chain !== undefined && chain.key === key ? chain.count + 1 : 1
  chains.set(exec.agent, { key, count })
  if (!thresholdSet.has(count)) return undefined
  const text = count === thresholds[0]
    ? GENTLE_REMINDER
    : detailedReminder(exec.name, count, previewArguments(canonical, argumentsPreviewChars))
  return createUserMessage({ content: [{ type: 'text', text }],
    source: { ...PLUGIN_SOURCE, form: 'notice', summary: `${exec.name} × ${count}` } })
}

ctx.on('tools/post-execute', async (exec, _result, next): Promise<PostToolDecision> => {
  const reminder = observe(exec)
  const downstream = await next()
  if (!reminder) return downstream
  if (downstream.kind === 'block') {
    return { kind: 'block', feedback: downstream.feedback,
      additionalContexts: prependContext(reminder, downstream.additionalContexts) }
  }
  return { ...downstream, additionalContexts: prependContext(reminder, downstream.additionalContexts) }
})
```

> 设计要点：
>
> ① **DELEGATE 再 fold**：先 observe（无论下游成功与否），再 `await next()`，最后把提醒 prepend 到 `additionalContexts` —— 拒绝路径也能收到提醒。
>
> ② **user interjection reset**（`index.ts:229-232`）：
>
> ```typescript
> ctx.on('agent/pre-step', ({ agent, messages }, next): Promise<PreStepDecision> => {
>   if (messages.some(message => message.source.kind === 'user')) chains.delete(agent)
>   return next()
> })
> ```
>
> 用户中途插话 → 清空计数链，避免误报。

### 5.3 Timeout-Policy — 工具超时

**关键文件**：`packages/guard/timeout-policy/src/index.ts`

```typescript
// 关键结构：tool-call timeout policy（基于 dsh-timeout 包）
export function apply(ctx: Context, config: Config): void {
  // 监听 tools/pre-execute，附加 idleWatchdog
  ctx.on('tools/pre-execute', async (exec, next) => {
    // ...
  })
}
```

> 详细实现依赖 `@deepseek-ai/dsh-timeout` 的 `idleWatchdog / timeoutOf`，核心思想是「每次 tool call 启动 watchdog，超时后 abort signal 让 ToolRuntime 拒绝继续」。

### 5.4 与 `laew` 的 Quality-Check Agent 对标

| laew Quality-Check | deepseek-harness guard |
|---------------------|------------------------|
| LLM Judge（每次执行单元完成后调一次） | Plugin waterfall（post-execute 钩子） |
| 输出 pass / fail + 反馈 | 仅注入 reminder 到 next step context |
| 失败可阻断 | 不阻断，只是「advisory」 |

**借鉴价值**：
- 「轻量 guardrail」更适合通用场景，LLM judge 仅在 hard 任务用。
- `thresholds: [3, 5, 8]` 双档（gentle / detailed）防止「第一次就上重锤」。
- 参数 canonical 化是「避免假阳性」的关键工程细节。

---

## 6. 任务拆解 — Plan-mode / SubAgent / Capability Seam

### 6.1 三层拆解范式

deepseek-harness 把任务拆解拆给 **三个独立层**：

1. **Plan-mode**：用户在交互层开启 / 关闭，由模型通过 `exit_plan_mode` 主动呈现方案。
2. **SubAgent（continuable / one-shot）**：父 Agent 通过工具调用派生子 Agent。
3. **Capability seam**：每个 subagent provider 声明其能力（agentOptions / outputSchema / depthLimit / toolFilter / persona），subagent registry 做 capability check。

### 6.2 SubAgent Capability Seam

**关键文件**：`packages/subagent/subagent/src/index.ts`

**核心类型**（从 `subagent/src/types.ts` re-export，`subagent/src/index.ts:84-95`）：

```typescript
export type {
  ContinuableCreateRequest,
  ContinuableCreateSpec,
  ResolvedSubagentStartRequest,
  SubagentCapabilities,    // 关键
  SubagentProvider,
  SubagentResult,
  SubagentRun,
  SubagentStartRequest,
  SubagentStopReason,
  SubagentStopReasonMap,
} from './types.ts'
```

**`SubagentCapabilities` 五项**（在 `assertCapabilities` 中枚举，`subagent/src/index.ts:619-635`）：

```typescript
const needs: { when: boolean; cap: keyof SubagentCapabilities }[] = [
  { when: request.agentOptions !== undefined, cap: 'agentOptions' },
  { when: request.outputSchema !== undefined, cap: 'outputSchema' },
  { when: request.maxDepth !== undefined, cap: 'depthLimit' },
  { when: request.toolFilter !== undefined, cap: 'toolFilter' },
  { when: request.persona !== undefined, cap: 'persona' },
]
for (const { when, cap } of needs) {
  if (when && !provider.capabilities[cap]) {
    throw new SubagentError(`subagent provider "${provider.name}" does not support the "${cap}" capability`,
      'UNSUPPORTED_CAPABILITY')
  }
}
```

### 6.3 11 个 SubAgent Provider

**关键目录**：`packages/subagent/`

| Provider | 描述 |
|----------|------|
| `subagent-acp/` | ACP 协议子进程（206 行） |
| `subagent-claude-code/` | Claude Code 子进程（157 行） |
| `subagent-codex/` | Codex CLI 子进程（140 行） |
| `subagent-dsh-sdk/` | 自家 SDK 子进程（200 行） |
| `subagent-fork-in-process/` | 同进程 fork（101 行） |
| `subagent-in-process-driver/` | 同进程 driver（233 行） |
| `subagent-spawn-in-process/` | 同进程 spawn（70 行） |
| `tool-subagent/` | 模型面对的委派工具（693 行） |
| `tool-subagent-control/` | 控制面工具（120 行） |
| `tool-subagent-report/` | 汇报面工具（142 行） |
| `subagent/`（Service 定义）| registry / runtime / 638 行 |

> 11 个 provider 体现「**多个进程模型共存**」的设计：fork / spawn / 跨进程 SDK / ACP 各有适用场景。

### 6.4 委派工具 `tool-subagent`

**关键文件**：`packages/subagent/tool-subagent/src/index.ts`

**核心配置**（`tool-subagent/src/index.ts:49-100`）：

```typescript
export interface Config {
  provider: string                                  // provider 名称
  toolName?: string                                 // 模型面对的工具名
  modelSelectionSettings?: boolean                  // 子 session 继承父模型选择
  enableRunInBackground?: boolean                   // 暴露 run_in_background 参数
  backgroundMode?: 'one-shot' | 'continuable'       // 后台策略
  agentOptions?: AgentOptions                       // 子 Agent 共享配置
  persona?: string                                  // 子 Agent 人设
  toolFilter?: { allow?: string[]; deny?: string[] } // 工具过滤
  maxDepth?: number | 'provider-managed'            // 子 Agent 嵌套深度
}
```

> 设计要点：
>
> ① **Capability seam + tool filter + depth limit**：三个独立维度共同保证委派的边界。
>
> ② **backgroundMode**：`one-shot` = 父等子结束；`continuable` = 父不等、后台持续运行，需要 `prepareContinuable` 能力。
>
> ③ **provider 校验**（`tool-subagent/src/index.ts`）—— `assertAllowedModelSelection` / `preflightChildLlmRoute` 在委派前做模型可用性预检。

### 6.5 持续子 Agent (Continuable)

**关键文件**：`packages/subagent/subagent/src/continuation.ts`（隐含于 import）

```typescript
// subagent/src/index.ts:237-239
async startContinuable(spec: ContinuableStartSpec): Promise<ContinuableStart> {
  return this.requireContinuations().startContinuable(spec)
}

// subagent/src/index.ts:256-263
async followup(parent, childId, content, options): Promise<MessageId> {
  return this.requireContinuations().followup(parent, childId, content, options)
}

// subagent/src/index.ts:280-282
interrupt(targetSessionId, authority): void {
  this.continuations?.interrupt(targetSessionId, authority)
}
```

> 设计要点：**continuable 子 Agent 是「独立 Session + 自己的 Agent 驱动」**，父 Agent 通过 inbox (`agent.followup`) 投递消息，无需关心子 Agent 是否常驻。
>
> 注释（`subagent/src/index.ts:266-273`）：
> > "Unclaimed pending inbox work, the Activation, and published descendants are preserved; claimed work is not requeued."

### 6.6 与 `laew` 对标

| laew | deepseek-harness |
|------|------------------|
| `MultiAgentOrchestrator` 编排 6 角色 | `SubagentRuntime` 编排 N provider |
| `SubAgent-Work` 单类型执行层 | 7 种 provider 形态（in-process / fork / spawn / ACP / Claude Code / Codex / dsh-sdk） |
| `maxDepth` 未显式 | `maxDepth` + `'provider-managed'` 双重控制 |
| 工具过滤未建模 | `toolFilter: { allow, deny }` 显式建模 |
| Quality-Check 必经 | 无强制；通过 guard plugin 异步叠加 |

**借鉴价值**：
- ① **Capability seam 抽象**：避免把「子 Agent 能力」耦合到单一 provider。
- ② **tool-filter + depth-limit** 是委派安全的两个核心维度。
- ③ **continuable 模型**适合 long-running 后台 agent；one-shot 适合同步委派。

---

## 7. 任务分类 — Goal 模式 + 任务分级

### 7.1 Goal 模式 vs Yolo 三档

deepseek-harness 没有 LLM 任务分类，**目标分级靠域模型硬约束**：

**关键文件**：`packages/goal/goal/src/index.ts:142-147`

```typescript
function resolveMaxGoalRounds(value: number): number {
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new GoalError('maxGoalRounds must be a positive safe integer', 'GOAL_INVALID_MAX_ROUNDS')
  }
  return value
}
```

**Goal 配置**（`goal/src/index.ts:186-188`）：

```typescript
static Config: z<Config> = z.object({
  defaultMaxGoalRounds: z.number().default(256),
})
```

### 7.2 Goal 的四 phase 状态机

| phase | 进入条件 | 退出条件 |
|-------|---------|---------|
| `active` | `create` / `resume` | `pause` / `complete` / `block` |
| `paused` | `pause` | `resume` / `complete` |
| `blocked` | `block(reason)` | `resume` / `complete` |
| `complete` | `complete` | （终态，可用 `create` 替换） |

**关键代码**（`goal/src/index.ts:298-346`）：

```typescript
@Remote('pause')
pause(agent: Agent, ref: GoalRef): GoalView {
  return this.transition(agent, ref, 'pause', ['active'], 'paused', 'disarmed')
}

@Remote('resume')
resume(agent: Agent, ref: GoalRef): GoalView {
  const cache = this.prepareMutation(agent)
  const current = this.expectCurrent(cache, ref)
  const resumable: readonly GoalPhase[] = ['active', 'paused', 'blocked']
  if (!resumable.includes(current.phase)) throw this.transitionError(current, 'resume', resumable)
  if (current.phase === 'active' && cache.activation === 'armed') {
    throw new GoalError(`goal "${current.id}" is already active and armed`, 'GOAL_INVALID_TRANSITION')
  }
  if (cache.state.roundsStarted >= current.maxGoalRounds) {
    throw new GoalError(`goal "${current.id}" exhausted ${current.maxGoalRounds} goal rounds; increase maxGoalRounds before resuming`,
      'GOAL_INVALID_TRANSITION')
  }
  return this.commitCurrent(agent, cache, 'resume', this.withPhase(current, 'active'), 'armed')
}
```

### 7.3 Round 计数与续作

**关键代码**（`goal/src/types.ts` + `goal-round-driver`）：

```typescript
// goal/src/types.ts: GoalSnapshot
{
  id: GoalId(`goal-${randomUUID()}`),
  revision: 1,
  objective: spec.objective,
  phase: 'active',
  maxGoalRounds: spec.maxGoalRounds,  // 默认 256
}
```

`GoalMessageSource`（`goal/src/domain.ts:47-53`）：

```typescript
export interface GoalMessageSource {
  readonly kind: 'goal'
  readonly goalId: GoalId
  readonly revision: number
  readonly round: number  // 正整数
}
```

> 设计要点：**round > 0 是「自动续作」标记**，round-driver 据此识别哪些 inbox 消息是机器注入的，哪些是用户/工具结果。

### 7.4 与 `laew` 对标

| laew 任务分级 | deepseek-harness |
|--------------|------------------|
| LLM 三步意图分析（目的→目标→意图） | 无对应层 |
| simple/medium/hard 三档 | `maxGoalRounds` 一个数值参数 |
| Yolo Agent 唯一入口 | 用户显式创建 goal，无入口层 |
| Quality-Check 必经 | `block(reason)` 由调用方主动置 |

**借鉴价值**：
- ① **`maxGoalRounds` 是显式可解释的资源配额**，比 LLM 分类更可控。
- ② **四 phase + 修订号 CAS** 状态机可直接复用做 laew 任务状态管理。

---

## 9. 工具调用 — Typert 类型图 / LLM 适配器 / Tool Definition

### 9.1 Typert — 类型驱动的 RPC / Lookup / Schema 系统

**关键包**：`packages/typert/{protocol,registry,generator,loader}/`

| 包 | 作用 |
|----|------|
| `protocol` | RPC / Context / Lookup 装饰器（`Remote`、`TypertRemoteService`） |
| `registry` | 运行时服务注册表（738 行） |
| `generator` | TS 类型图分析 + schema/remote/face emitter |
| `loader` | 编译产物加载（442 行） |

**核心类型**（`typert/protocol/src/index.ts:17-38`）：

```typescript
export class TypertLookupFailure<Failure = unknown> extends Error {
  readonly failure: Failure
  constructor(failure: Failure) {
    super('Typert lookup policy rejected the requested identity')
    this.name = 'TypertLookupFailure'
    this.failure = failure
  }
}

export class TypertRemoteFailure extends Error {
  readonly failure: RemoteFailure
  constructor(failure: RemoteFailure) {
    super(failure.message)
    this.name = 'TypertRemoteFailure'
    this.failure = failure
  }
}
```

**Remote 装饰器**（`protocol/src/index.ts` 中的 `@Remote('xxx')`）—— 出现在 `goal/src/index.ts:276` 等处：

```typescript
@Remote('edit')
edit(agent: Agent, ref: GoalRef, request: EditGoalRequest): GoalView { ... }

@Remote('pause')
pause(agent: Agent, ref: GoalRef): GoalView { ... }
```

> 设计要点：**`@Remote` 把 service method 投影成 wire-format callable**，参数和返回值的 schema 由 Typert generator 从 TS 类型图静态推导。
>
> 注释（`protocol/src/index.ts` 顶部）：
> > "Remote decorators and explicit Gateway bindings backed only by private module state. Strict reflection remains a Typert compiler responsibility."

### 9.2 类型图核心模型

**关键文件**：`packages/typert/generator/src/model.ts`

**核心类型**（节选）：

```typescript
// model.ts:64-71
export interface ServiceModel extends DocumentationModel {
  readonly key: string             // ctx.xxx 的 key
  readonly symbol: SymbolId
  readonly export: ExportModel
  readonly members: readonly string[]
  readonly location: SourceLocation
}

// model.ts:73-81
export interface EventModel extends DocumentationModel {
  readonly name: string
  readonly signature: TypeNodeId
  readonly text: string            // body-free declaration text
  readonly mode?: string           // waterfall / serial / emit
  readonly location: SourceLocation
}

// model.ts:83-88
export interface ObjectModel extends DocumentationModel {
  readonly export: ExportModel
  readonly symbol: SymbolId
  readonly passing: 'reference'    // 引用传递语义
}

// model.ts:90-95
export interface SchemaModel extends DocumentationModel {
  readonly export: ExportModel
  readonly symbol: SymbolId
  readonly type: TypeNodeId        // 选中用于 schema 生成的类型
}
```

> 设计要点：**Typert 把 Cordis Context / Events / Schema / Remote 都建模成「类型图节点」**，跨 host / client 双面编译时保证类型一致。

### 9.3 LLM 适配器：DeepSeek 直连

**关键文件**：`packages/llm/llm-deepseek/src/index.ts`、`packages/llm/llm-deepseek/src/adapter.ts`

**Provider 路由**（`llm-deepseek/src/index.ts:86-112`）：

```typescript
const NS = settingsNamespace('llm-deepseek')
const DEFAULT_API_KEY_ENV = 'DEEPSEEK_API_KEY'
const PROVIDER = 'deepseek-official'

const DEFAULT_MODELS: DeepSeekCatalogModel[] = [
  { id: 'deepseek-v4-flash', name: 'DeepSeek-V4-Flash', contextWindow: DEFAULT_CONTEXT_WINDOW },
  { id: 'deepseek-v4-pro', name: 'DeepSeek-V4-Pro', contextWindow: DEFAULT_CONTEXT_WINDOW },
  { id: 'deepseek-v4-flash-vision-exp', name: 'DeepSeek-V4-Flash-Vision-Exp',
    inputModalities: ['text', 'image'],
    imagePixelBudget: DEFAULT_REQUEST_IMAGE_PIXEL_BUDGET,
    imageMaxBytes: DEFAULT_REQUEST_IMAGE_MAX_BYTES },
]
```

**Adapter 注册**（`llm-deepseek/src/adapter.ts:707` —— `DeepSeekAdapter` class）：

- **连接事实 per-request resolve**：base URL / catalog / API key 都在每次请求时从 `ctx.settings` + `ctx.credentials` 取，**不冻结**于加载时。
- **retry policy 是唯一在注册时冻结的事实**，变更时重新 in-place 注册路由。

**注释**（`llm-deepseek/src/index.ts:1-12`）：
> "The one registration-captured fact — the retry policy — re-registers the route in place when it changes."

### 9.4 LLM 适配器：pi-ai 通用代理

**关键文件**：`packages/llm/llm-pi-ai/src/adapter.ts`

**核心契约**（`llm-pi-ai/src/adapter.ts:65-112`）：

```typescript
interface PiAiSnapshot {
  profiles: ReadonlyMap<string, ResolvedPiAiProviderProfile>
  models: Models   // 不可变集合
}

export interface PiAiAdapterOptions {
  profiles: () => ReadonlyMap<string, ResolvedPiAiProviderProfile>
  resolveApiKey: (provider, profile) => Promise<string | undefined>
  auth: PiAiAuthInjection     // 凭据存储 + auth context
  resolveAttachments?: () => AttachmentStore | undefined
  resolveImageAccess?: (attachments, ref) => ImageAttachmentAccess | undefined
  onReplayDegrade?: (detail) => void
}
```

> 设计要点：
>
> ① **immutable snapshot**：每次请求捕获完整 profile+models 快照（`adapter.ts:11-13` 注释）：
>
> > "A configuration change builds a *new* collection rather than mutating the one in use, because `Models.streamSimple()` is lazy..."
>
> ② **per-step call freeze**：与 `ReactLoopAgent.prepareCall` 配合，确保「在飞请求不会被切模型」。
>
> ③ **apiKey as request override**：credential 通过 `apiKey` 注入 pi-ai options（`adapter.ts:17-19` 注释）。

### 9.5 Tool 定义

**关键文件**：`packages/plan/plan-mode/src/index.ts:342-430`（`exit_plan_mode` 工具定义）

```typescript
ctx.tools.register(defineTool({
  name: EXIT_PLAN_MODE,
  description: EXIT_DESCRIPTION,
  parameters: {
    plan: { type: 'string', required: true, description: 'The complete plan, as markdown, starting with a # heading that names it.' },
  },
  output: {
    schema: {
      type: 'object',
      additionalProperties: false,
      properties: {
        approved: { type: 'boolean', const: true, required: true },
      },
    },
    render: () => [{ type: 'text', text: 'Plan approved — plan mode exited; carry out the plan starting with your next step.' }],
  },
  execute: async (args, exec) => { /* ... */ },
  presentCall: args => ({ card: 'generic', title: firstHeading(args.plan) ?? 'Plan', kind: 'other',
    content: [{ type: 'text', text: args.plan }] }),
  presentResult: (_args, result) => ({ card: 'generic', title: 'Plan review', content: result.content }),
}))
```

> 设计要点：
>
> ① **JSON Schema 参数定义**与 Anthropic / OpenAI 工具协议天然对齐。
>
> ② **`presentCall` / `presentResult`** 把工具调用卡片化，UI 侧无需自行渲染。
>
> ③ **`defineTool` 工厂**封装了 `Tool` trait 的注册。

### 9.6 工具后执行管道

**关键代码**（`guard/repeat-tool-reminder/src/index.ts:213-224`）：

```typescript
ctx.on('tools/post-execute', async (exec, _result, next): Promise<PostToolDecision> => {
  const reminder = observe(exec)
  const downstream = await next()
  // 把 reminder prepend 到 additionalContexts
})
```

> 工具执行后挂多个监听器（如 repeat-tool-reminder / timeout-policy），各自往 `additionalContexts` 追加 user message。

### 9.7 与 `laew` 对标

| laew | deepseek-harness |
|------|------------------|
| Rust `Tool` trait + `ToolRegistry` | TS `defineTool` + `ctx.tools.register` |
| Bash / Read / Write 三个工具 | Bash / Read / Write + 几十个工具（plan / goal / skill / subagent / file-api …） |
| 手动 wire 协议差异 | Typert generator 自动生成 schema / wire |
| 工具调用回填到 context.messages | tool call 落 session log，由 `deriveMessages()` 重放 |

**借鉴价值**：
- ① **Typert 类型图** 把「context 类型」「event 类型」「schema 类型」「remote 类型」四合一建模。
- ② **per-request immutable snapshot**：模型切配置不会污染在飞请求。
- ③ **presentCall / presentResult** 是工具可视化的标准模式。

---

## 10. MCP 设计与实现

### 10.1 包结构

**关键目录**：`packages/mcp/mcp-client/`

```
mcp-client/
├── src/
│   ├── connection.ts  351 行 — 连接监管 + 自动重连
│   ├── tools.ts       559 行 — 工具同步桥
│   ├── transport.ts    50 行 — stdio / streamable-http 工厂
│   ├── index.ts       188 行 — Plugin 入口
│   └── invariant.ts    30 行
└── tests/             reconnect / load-path / apply / fixture
```

### 10.2 连接监管 + 指数退避

**关键文件**：`packages/mcp/mcp-client/src/connection.ts`

**重连策略**（`connection.ts:27-45`）：

```typescript
export interface ReconnectConfig {
  enabled?: boolean            // 默认 true
  initialDelayMs?: number      // 默认 500
  maxDelayMs?: number          // 默认 30_000
  maxAttempts?: number         // 默认 10
}

export const RECONNECT_DEFAULTS: Required<ReconnectConfig> = Object.freeze({
  enabled: true, initialDelayMs: 500, maxDelayMs: 30_000, maxAttempts: 10,
})
```

**重连调度**（`connection.ts:192-225`）：

```typescript
function scheduleReconnect(): void {
  const lostEstablishedConnection = connectedAt !== undefined
  if (!policy.enabled) {
    ctx.logger.error(`${label}: connection lost and reconnect is disabled — ...`)
    return
  }
  // A connection that stayed up past the stability window (= maxDelayMs, the longest backoff spacing)
  // ended the previous outage: start a fresh budget.
  if (connectedAt !== undefined && Date.now() - connectedAt >= policy.maxDelayMs) failedAttempts = 0
  connectedAt = undefined
  failedAttempts += 1
  if (failedAttempts > policy.maxAttempts) {
    syncChain = syncChain.then(() => {
      for (const dispose of disposers.values()) dispose()
      disposers = new Map()
    })
    ctx.logger.error(`${label}: giving up after ${policy.maxAttempts} consecutive failed reconnect attempts — tools unregistered`)
    return
  }
  const delayMs = Math.min(policy.maxDelayMs, policy.initialDelayMs * 2 ** (failedAttempts - 1))
  // ...
}
```

> 设计要点：
>
> ① **stable window = maxDelayMs**：连接稳定超过退避上限后，认为上次 outage 已结束，重新计数 budget。
>
> ② **指数退避带 cap**：`min(maxDelayMs, initialDelayMs * 2^(n-1))`。
>
> ③ **exhaustion 后 unregister tools**：超过 maxAttempts 主动注销工具，避免「永远注册但永远调不通」。
>
> ④ **process unref**：`reconnectTimer.unref()` —— 防止 timer 阻塞进程退出。

### 10.3 串行化 syncChain

**关键代码**（`connection.ts:161-170`）：

```typescript
let syncChain: Promise<void> = Promise.resolve()
function enqueueSync(generation: Client, syncOpts: ToolBridgeOptions = opts): Promise<void> {
  const run = syncChain.then(async () => {
    if (!isCurrent(generation)) return
    disposers = await syncTools(generation, ctx, syncOpts, disposers)
  })
  syncChain = run.catch(() => {})  // chain tail survives failure
  return run
}
```

> 设计要点：**`syncChain` 是「先 dispose 上一代工具，再 register 新工具」原子化的串行队列**，避免两代工具交错（双重 dispose / 泄漏）。

### 10.4 Transport 工厂

**关键文件**：`packages/mcp/mcp-client/src/transport.ts:31-50`

```typescript
export function createTransport(config: Config): Transport {
  switch (config.transport) {
    case 'stdio':
      return new StdioClientTransport({
        command: config.command,
        args: config.args,
        env: buildChildEnv(config.env),  // scrubbedParentEnv + spec env
        cwd: config.cwd,
      })
    case 'streamable-http':
      return new StreamableHTTPClientTransport(new URL(config.url),
        { requestInit: { headers: config.headers } }) as Transport
  }
}

function buildChildEnv(extra: Record<string, string>): Record<string, string> {
  return { ...scrubbedParentEnv(), ...extra }  // 凭据形 + DSH_* 名被过滤
}
```

> 设计要点：
>
> ① **stdio 子进程环境由 dsh-subprocess 包的 `scrubbedParentEnv()` 清理**，避免敏感凭据泄漏给 MCP server。
>
> ② **Streamable HTTP transport**：直接连接 URL，无 stdio 复杂度。

### 10.5 工具同步桥

**关键文件**：`packages/mcp/mcp-client/src/tools.ts`

`tools.ts` 把 MCP server 的 `tools/list` 结果注册到 `ctx.tools` 命名空间下：

- `serverName` 前缀避免多 server 工具冲突。
- `registrationFailure: 'throw' | 'contain'`：启动时是否容忍冲突。
- `toolCallTimeoutMs`：每个 tool call 的超时上限。

### 10.6 与 `laew` 对标

| laew | deepseek-harness |
|------|------------------|
| 无 MCP 支持 | 完整 MCP-client（stdio / streamable-http） |
| 内置三工具（Bash/Read/Write） | mcp server 工具 + 内置工具并存 |

**借鉴价值**：
- ① **exponential backoff + stable window** 重连策略直接可借鉴。
- ② **`syncChain` 串行化** 是「多代连接原子切换」的工程范式。
- ③ **`scrubbedParentEnv()` 凭据脱敏** 是 stdio MCP 的安全标配。

---

## 11. SKILL 设计

### 11.1 包结构

**关键目录**：`packages/skill/`

```
skill/
├── skill/               868 行 — Service 定义：registry + 合并策略
├── skill-filesystem/   1041 行 — 文件系统 provider（项目级 / 用户级）
├── skill-badge/          60 行 — UI 徽章
└── tool-skill/          430 行 — 模型面对的 `skill` 工具
```

### 11.2 Skill 域类型

**关键文件**：`packages/skill/skill/src/index.ts`

**核心常量与类型**（`skill/src/index.ts:20-83`）：

```typescript
const SKILL_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/   // kebab-case 名称
const DEFAULT_COLLECT_CACHE_ENTRIES = 128
const MAX_COLLECT_ATTEMPTS = 2
const RUNTIME_PROVIDER = 'runtime'
const RUNTIME_RANK = 250
export const BUNDLED_SKILL_RANK = 600

export type SkillSource = 'project-dsh' | 'project-agents' | 'runtime' | 'user-dsh' | 'user-agents' | 'custom' | 'bundled' | (string & {})

export type SkillResourceBase =
  | { readonly kind: 'directory'; readonly path: string }
  | { readonly kind: 'url'; readonly url: string }
  | { readonly kind: 'opaque'; readonly description: string }

export interface SkillInvocationPolicy {
  readonly modelInvocable: boolean
  readonly userInvocable: boolean
}

export interface SkillSummary {
  readonly name: string
  readonly description: string
  readonly whenToUse?: string
  readonly invocation: SkillInvocationPolicy
  readonly source: SkillSource
  readonly provider: string
  readonly resourceBase?: SkillResourceBase
}

export interface SkillCandidate extends SkillSummary {
  readonly rank: number      // 越小越优先
  readonly locator: unknown   // provider-owned handle
  readonly path?: string
}

export interface SkillDefinition extends SkillSummary {
  readonly content: string    // markdown body
  readonly path?: string
  readonly metadata?: Readonly<Record<string, unknown>>
}
```

### 11.3 模型渲染

**关键代码**（`skill/src/index.ts:171-184`）：

```typescript
export function renderSkillContent(skill: Pick<SkillDefinition, 'name' | 'provider' | 'resourceBase' | 'content'>): string {
  const resourceHint = renderResourceHint(skill)
  return [
    `<skill_content name="${escapeAttr(skill.name)}">`,
    '<skill_resources>',
    ...resourceHint,
    '</skill_resources>',
    '',
    '<skill_instructions>',
    skill.content,
    '</skill_instructions>',
    '</skill_content>',
  ].join('\n')
}
```

> 设计要点：**模型看到的 `<skill_content>` 包装层是单一格式**，无论是 `tool-skill` 的 result 还是 user-explicit invocation 的 context injection，都走同一渲染。

### 11.4 Skill Layered Registry

**关键代码**（`skill/src/index.ts:328-403`）：

```typescript
class SkillLayer implements ScopeLayer {
  readonly providers: NamedEntries<RegisteredProvider>
  readonly runtime = new Map<string, SkillDefinition>()
  constructor(scope: ScopeKey | undefined) {
    this.providers = new NamedEntries(name => new Error(scope === undefined
      ? `a skill provider named "${name}" is already registered`
      : `a skill provider named "${name}" is already registered in this scope`))
  }
}

export class SkillRegistry extends Service {
  private readonly layers = new ScopedLayers<SkillLayer>(
    scope => new SkillLayer(scope),
    () => { this.invalidateCache() },
  )
  // ...
  registerProvider(create: (control: SkillProviderControl) => SkillProvider): () => void {
    const lifecycle = new AbortController()
    let registration: { layer: SkillLayer; name: string } | undefined
    let provider: SkillProvider
    const control: SkillProviderControl = {
      signal: lifecycle.signal,
      invalidate: () => { /* ... */ },
    }
    // ...
  }
}
```

> 设计要点：
>
> ① **ScopedLayers**：每个 Cordis scope 一个 SkillLayer，最近一层胜出（nearest-wins），同层 rank 排序。
>
> ② **global + per-scope 合并**：host plugin + agent preset 的 plugin 同时可见。
>
> ③ **AbortController + invalidate**：provider 注销时同步取消所有 in-flight 加载。

### 11.5 SkillProvider 接口

**关键代码**（`skill/src/index.ts:248-268`）：

```typescript
export interface SkillProvider {
  readonly name: string
  readonly list: (options: SkillLookupOptions) =>
    Promise<readonly SkillCandidate[] | SkillProviderObservation>
  readonly get: (candidate: SkillCandidate, options: SkillLookupOptions) =>
    Promise<SkillDefinition | undefined>
}
```

### 11.6 Filesystem Provider

**关键文件**：`packages/skill/skill-filesystem/src/index.ts`（1041 行）

- 扫描项目级 `.claude/skills/` / `.agents/skills/` / 用户级 `~/.config/.../skills/` 等多根。
- 按 frontmatter 解析 metadata、rank、source。
- `MAX_COLLECT_ATTEMPTS = 2` 限制发现重试。

### 11.7 Tool 入口

**关键文件**：`packages/skill/tool-skill/src/index.ts`（430 行）

注册 `skill` 工具，模型可主动 `load_skill` 加载完整 body（而非只看 summary）。

### 11.8 与 `laew` 对标

| laew | deepseek-harness |
|------|------------------|
| 无 Skill 系统 | 完整 Skill 系统（registry + filesystem provider + 渲染） |
| 工具 / 命令平铺 | Skill 是「模型可调用的、有 Markdown 指令的资源」 |
| `Yolo` 入口工具 | `skill` / `load_skill` 工具 + 自动 catalog |

**借鉴价值**：
- ① **Skill 是「轻量 MCP」**：把可复用的领域知识封装成可由模型主动加载的资源。
- ② **`<skill_content>` 单一渲染格式**：所有 skill 加载路径（tool / invocation）模型侧一致。
- ③ **ScopedLayers + rank + source**：三维度的合并策略可借鉴到 laew 的工具发现。

---

## 总体对比与借鉴建议

### 总体架构差异

| 维度 | laew（Rust） | deepseek-harness（TypeScript） |
|------|--------------|-------------------------------|
| **语言范式** | 强类型 enum / trait | `declare module` 类型合并 + 装饰器 |
| **多 Agent 架构** | 6 角色硬编码 + 三档任务分类 | SubAgent provider registry + capability seam |
| **状态管理** | 内存 context + SQLite 元数据 | 不可变 append-only session log + 内存投影 |
| **意图识别** | LLM 三步分类（Yolo） | 显式 Goal 域（maxGoalRounds） |
| **质检** | 必经 LLM judge（QC Agent） | 可叠加 plugin guard（repeat-tool-reminder / timeout-policy） |
| **工具** | 内置 3 工具 + Rust trait | 内置 N 工具 + MCP + Skill + defineTool |
| **LLM 适配** | 双协议手写 wire（Anthropic + OpenAI） | DeepSeek 直连 + pi-ai 通用代理 |
| **类型系统** | Rust trait + 泛型 | TS 装饰器 + Typert 类型图生成器 |

### 关键借鉴清单

1. **三相位状态机**（`idle / maintenance / running`）+ 显式 `wakeRequested` 锁存 → 解决 cancel 重入。
2. **Inbox 持久化**（`agent/inbox/spliced` 事件 + 双队列 `next-turn / next-step`）→ resume / fork 零成本还原。
3. **sticky turn end reason**（max-tokens 不被 completed step 降级）→ 跨 step 的失败结论稳定。
4. **per-request immutable LLM snapshot** → 在飞请求不受配置变更影响。
5. **Typert 类型图**（Service / Event / Object / Schema 四类节点）→ 编译期保证 RPC + wire 一致。
6. **Capability seam**（subagent 五项能力声明 + assertCapabilities）→ 多 provider 安全共存。
7. **Plan-mode pending 纯 fold** → 无 live mirror，重启恢复零成本。
8. **Guard plugin waterfall**（observe + delegate + fold reminder）→ 比 LLM judge 更轻量。
9. **MCP 重连策略**（指数退避 + stable window + exhaustion unregister）→ 健壮性工程细节。
10. **Skill layered registry + rank + source** → 多源资源合并的可借鉴抽象。

### laew 可改进点

1. **`Session.context` 应支持 turn / step 边界事件**，便于 resume 后 fold 一致。
2. **可引入类似 `guard/repeat-tool-reminder` 的轻量 guard 插件**，避免 hard 任务才上 LLM judge。
3. **MCP 客户端可参考**：`scrubbedParentEnv()` + 指数退避 + `syncChain` 串行化。
4. **Tool 定义可拆 `defineTool` 工厂**，把 schema + presentCall/presentResult 标准化。
5. **Skill / Subagent 可引入 layered registry**，避免硬编码。

---

## 参考文件路径清单

### 核心 Agent
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/agent-loop/src/agent.ts` — ReactLoopAgent（543 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/agent/src/inbox.ts` — Inbox（220 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/agent/src/runtime-types.ts` — Agent 公开契约（299 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/agent/src/types.ts` — SessionEventMap 声明合并（46 行）

### Goal / Plan
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/goal/goal/src/index.ts` — GoalService（593 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/goal/goal/src/domain.ts` — Goal 域类型 + SessionEventMap 注入
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/goal/goal-round-driver/src/index.ts` — Round Driver
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/plan/plan-mode/src/index.ts` — PlanModeController（515 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/plan/plan-mode/src/types.ts` — PlanProjection 唯一声明（30 行）

### Guard
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/guard/repeat-tool-reminder/src/index.ts` — 重复调用检测（234 行）

### SubAgent
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/subagent/subagent/src/index.ts` — SubagentRuntime（638 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/subagent/tool-subagent/src/index.ts` — 委派工具（693 行）

### LLM
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/index.ts` — DeepSeek 直连 plugin
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/adapter.ts` — DeepSeekAdapter（707 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-pi-ai/src/adapter.ts` — pi-ai 通用代理（420 行）

### MCP
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/mcp/mcp-client/src/connection.ts` — 连接监管 + 退避（351 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/mcp/mcp-client/src/transport.ts` — stdio / http transport（50 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/mcp/mcp-client/src/tools.ts` — 工具同步桥（559 行）

### Skill
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/skill/skill/src/index.ts` — SkillRegistry（868 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/skill/skill-filesystem/src/index.ts` — Filesystem Provider（1041 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/skill/tool-skill/src/index.ts` — `skill` 工具（430 行）

### Typert
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/protocol/src/index.ts` — Remote / Lookup 装饰器（325 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/generator/src/model.ts` — 类型图核心模型（438 行）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/generator/src/emitter.ts` — Schema / Remote / Face emitter（937 行）
