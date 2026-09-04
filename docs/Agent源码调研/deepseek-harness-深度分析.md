# deepseek-harness 源码系统深度分析

> 分析日期：2026-09-04
> 分析范围：`/usr/local/LsmGitOpenSource/deepseek-harness`
> 分析维度：10 个核心维度（项目架构 / 多轮对话 / Context 管理 / Yolo / 质检 / 任务拆解 / 任务分类 / 工具调用 / MCP / SKILL）

---

## 目录

1. [项目架构：Everything-is-a-Plugin](#1-项目架构everything-is-a-plugin)
2. [多轮对话实现：Round Driver 与 Turn/Step 双循环](#2-多轮对话实现round-driver-与-turnstep-双循环)
3. [Context 管理：Event Sourcing 与 Compaction](#3-上下文管理event-sourcing-与-compaction)
4. [Yolo / 任务识别：Goal 域与 goalRounds](#4-yolo--任务识别goal-域与-goalrounds)
5. [质检检查：Guard Plugin 与验证机制](#5-质检检查guard-plugin-与验证机制)
6. [任务拆解：SubAgent Provider 与依赖管理](#6-任务拆解subagent-provider-与依赖管理)
7. [任务分类：Goal 类型与路由逻辑](#7-任务分类goal-类型与路由逻辑)
8. [工具调用：Tool Runtime 与 PTC 模式](#8-工具调用tool-runtime-与-ptc-模式)
9. [MCP 设计：stdio/HTTP 传输与 Client 实现](#9-mcp-设计stdiohttp-传输与-client-实现)
10. [SKILL 设计：Layered Registry 与动态扩展](#10-skill-设计layered-registry-与动态扩展)

---

## 1. 项目架构：Everything-is-a-Plugin

### 1.1 整体架构概览

deepseek-harness 是一个基于 **Cordis 框架** 的 TypeScript Agent CLI 项目，采用 **Everything-is-a-Plugin** 架构。所有能力（工具、会话、压缩、MCP、Skill、SubAgent）都是 Cordis 插件，通过 `ctx.plugin()` 加载。

**核心目录结构**：
```
deepseek-harness/
├── vendor/                  # 内嵌框架（Cordis、Schemastery、Cosmokit）
├── packages/
│   ├── core/                # 核心模块
│   │   ├── agent-loop/      # 多轮驱动（ReactLoopAgent）
│   │   ├── session/         # 会话与事件溯源
│   │   ├── tools/           # 工具运行时
│   │   └── system-prompt/   # 系统提示词
│   ├── compaction/          # 上下文压缩
│   ├── subagent/            # 子 Agent 系统
│   ├── mcp/                 # MCP 客户端
│   ├── skill/               # Skill 注册中心
│   └── guard/               # 质检守卫
└── apps/
    └── cli/                 # CLI 入口（dsh 二进制）
```

### 1.2 Cordis 框架核心机制

Cordis 是一个 **IoC/DI 框架**，使用 JavaScript Proxy 实现服务解析。核心概念：

**文件位置**：`vendor/cordis/src/index.ts`（内嵌框架）

```typescript
// Cordis 核心：Context 是服务的容器
export class Context {
  // 服务解析：ctx[serviceName] 通过 Proxy 实现
  // 插件加载：ctx.plugin(MyPlugin, config)
  // 事件发射：ctx.emit(eventName, payload)
  // 生命周期：ctx.effect(dispose, label)
}
```

**关键机制**：

1. **服务注入**：通过 `static inject = ['dep1', 'dep2']` 声明依赖
2. **Fiber 隔离**：插件运行在 Fiber 中，支持 `ctx.effect()` 作用域释放
3. **Isolate/Intercept**：子上下文可以隔离服务或拦截配置
4. **Config 验证**：使用 Schemastery/Zod Schema 验证配置

### 1.3 插件加载流程

**文件位置**：`packages/core/agent-loop/src/index.ts`

```typescript
// 插件注册示例
export class ReactLoopAgent extends Service {
  static inject = ['session', 'toolRuntime', 'skillRegistry']
  static Config: z<Config> = z.object({
    maxTurns: z.natural().min(1).default(100),
    maxStepsPerTurn: z.natural().min(1).default(50),
  })
  
  async kick(): Promise<void> {
    // 核心驱动循环
  }
}
```

**加载流程**：
1. CLI 启动时创建根 Context
2. 通过 `ctx.plugin()` 加载所有插件
3. 插件在 Fiber 中初始化，注册服务和事件监听
4. 服务通过 Proxy 延迟解析

### 1.4 模块化设计特点

1. **包边界清晰**：每个功能独立包，通过 `package.json` 声明依赖
2. **作用域隔离**：`ScopedLayers<T>` 实现 per-scope 的工具和 Skill 隔离
3. **事件驱动**：模块间通过事件松耦合（`ctx.on()` / `ctx.emit()`）
4. **可替换性**：核心接口（如 `CompactionEngine`）抽象，可插拔实现

---

## 2. 多轮对话实现：Round Driver 与 Turn/Step 双循环

### 2.1 ReactLoopAgent 核心驱动

**文件位置**：`packages/core/agent-loop/src/agent.ts`（Lines 1-544）

`ReactLoopAgent` 是多轮对话的核心驱动，实现 **Turn/Step 双循环** 架构：

```typescript
type Phase =
  | { kind: 'idle'; lastTurn: number }
  | { kind: 'maintenance'; abort: AbortController; lastTurn: number; wakeRequested: boolean }
  | { kind: 'running'; abort: AbortController; turn: number; step: number; wakeRequested: boolean }

export class ReactLoopAgent extends Service {
  private phase: Phase = { kind: 'idle', lastTurn: 0 }
  private readonly inbox = new Inbox()
  
  private async kick(): Promise<void> {
    try {
      while (await this.turn()) {}
    } catch (_error) { /* contained */ }
  }
  
  private async turn(): Promise<boolean> {
    // 1. 发射 turn/start 事件
    // 2. 循环执行 step
    while (true) {
      const decision = await this.preStep(target, { turn, step })
      const stepEnd = await this.step(decision.assembly, ...)
      if (turnEnds && this.inbox.nextStep.length === 0) break
    }
    // 3. 发射 turn/end 事件
  }
}
```

### 2.2 Inbox 队列系统

**文件位置**：`packages/core/agent-loop/src/inbox.ts`

Inbox 是双队列系统，用于消息调度：

```typescript
class Inbox {
  private nextTurn: QueuedMessage[] = []  // 下一轮消息队列
  private nextStep: QueuedMessage[] = []  // 下一步消息队列
  
  enqueue(target: 'next-turn' | 'next-step', message: QueuedMessage): void {
    this[target].push(message)
  }
  
  dequeue(target: 'next-turn' | 'next-step'): QueuedMessage | undefined {
    return this[target].shift()
  }
}
```

**队列语义**：
- `next-turn`：用户新消息，触发新一轮 Turn
- `next-step`：工具结果或中间消息，触发当前 Turn 的下一步 Step

### 2.3 Step 执行流程

**文件位置**：`packages/core/agent-loop/src/agent.ts`（Lines 200-350）

```typescript
private async step(assembly: Message[], meta: StepMeta): Promise<StepResult> {
  // 1. 发射 step/start 事件
  // 2. 调用 LLM（通过 LlmClient）
  const response = await this.llm.complete({
    messages: assembly,
    tools: this.toolRegistry.list(),
    signal: this.phase.abort.signal,
  })
  
  // 3. 处理响应
  if (response.message) {
    await this.session.appendAssistantMessage(response.message, meta)
  }
  
  // 4. 执行工具调用
  if (response.toolCalls?.length > 0) {
    for (const call of response.toolCalls) {
      const result = await this.toolRuntime.execute(call, { signal })
      await this.session.appendToolResult(call.callId, result, meta)
    }
    return { kind: 'continue' }  // 继续 Step 循环
  }
  
  // 5. 无工具调用，Turn 结束
  return { kind: 'end', reason: 'no-tool-calls' }
}
```

### 2.4 Round Driver 模式

**文件位置**：`packages/core/agent-loop/src/round.ts`

Round Driver 是更高层的抽象，封装 Turn/Step 循环：

```typescript
export class RoundDriver {
  async runRound(input: UserInput): Promise<RoundResult> {
    // 1. 将用户输入加入 inbox
    this.agent.inbox.enqueue('next-turn', {
      type: 'user/message',
      content: input,
    })
    
    // 2. 唤醒 Agent 循环
    this.agent.wake()
    
    // 3. 等待 Round 完成
    return await this.waitForRoundEnd()
  }
}
```

**Round 生命周期**：
1. 用户输入 → Inbox
2. Agent 唤醒 → Turn 开始
3. Step 循环（LLM 调用 + 工具执行）
4. Turn 结束 → Round 完成

---

## 3. 上下文管理：Event Sourcing 与 Compaction

### 3.1 Event Sourcing 架构

**文件位置**：`packages/core/session/src/types.ts`（Lines 1-418）

Session 使用 **Event Sourcing** 模式，所有状态变更都是不可变事件：

```typescript
export interface SessionEventMap {
  'turn/start': { turn: number }
  'turn/end': { turn: number; reason: TurnEndReason }
  'user/message': UserMessage
  'assistant/message': { turn: number; step: number; message: AssistantMessage; usage?: TokenUsage; interrupted?: true }
  'tool/call': { turn: number; step: number; callId: ToolCallId; name: string; arguments: string }
  'tool/result': { turn: number; step: number; message: ToolResultMessage; error?: {...}; meta?: JsonValue }
  'compaction/start': { turn: number; start: number; end: number }
  'compaction/end': { turn: number; summary: string; start: number; end: number }
}
```

**事件类型**：
- **Turn 事件**：`turn/start`、`turn/end`
- **消息事件**：`user/message`、`assistant/message`
- **工具事件**：`tool/call`、`tool/result`
- **压缩事件**：`compaction/start`、`compaction/end`

### 3.2 Surface 层：模型可见视图

**文件位置**：`packages/core/surface/src/surface.ts`（Lines 1-461）

`SurfaceManager` 维护模型可见的有序视图，通过折叠事件流生成：

```typescript
export class SurfaceManager implements SessionSurface {
  private _state = createFoldState()
  private _eventLog: SessionEvent[] = []
  
  // 追加事件
  append(event: SessionEvent): void {
    this._eventLog.push(event)
    this._state = foldSurface(this._state, event)
  }
  
  // 替换事件（用于 Compaction）
  replaceRange(start: number, end: number, events: SessionEvent[]): void {
    this._eventLog.splice(start, end - start, ...events)
    this._state = foldSurface(this._state, { op: 'replace', start, end })
  }
  
  // 获取当前 Surface 节点
  get nodes(): readonly number[] { return this._state.nodes }
}
```

**Surface 操作**：
- `append`：追加新事件
- `{ op: 'replace', start, end }`：替换事件范围（Compaction 使用）

### 3.3 Compaction 引擎

**文件位置**：`packages/compaction/compaction/src/index.ts`（Lines 1-173）

Compaction 是 LLM 驱动的上下文压缩机制：

```typescript
export abstract class CompactionEngine extends Service {
  constructor(ctx: Context) {
    super(ctx, 'compaction')
  }
  
  // 检查是否需要压缩
  abstract compactIfNeeded(
    agent: CompactionAgentContext,
    trigger: CompactionTrigger,
    signal: AbortSignal,
  ): Promise<CompactionResult | null>
  
  // 立即压缩指定区域
  abstract compactRegion(
    agent: CompactionAgentContext,
    start: number,
    end: number,
    signal: AbortSignal,
  ): Promise<CompactionResult>
}

export type CompactionTrigger = 'pressure' | 'context-overflow'
```

**触发条件**：
- `pressure`：Token 压力（接近上下文窗口限制）
- `context-overflow`：上下文溢出（请求失败）

### 3.4 BasicCompactionEngine 实现

**文件位置**：`packages/compaction/compaction-basic/src/index.ts`（Lines 1-431）

```typescript
export class BasicCompactionEngine extends CompactionEngine {
  private _registerAutomaticCompaction(): void {
    // 监听 pre-step 事件，检查压力
    ctx.on('agent/pre-step', async ({ agent, signal }, next): Promise<PreStepDecision> => {
      if (!signal.aborted) {
        const result = await this.compactIfNeeded(agent, 'pressure', signal)
        if (result !== null) logResult(result, 'step pressure')
      }
      return next()
    })
    
    // 监听请求错误，触发溢出恢复
    ctx.on('agent/request-error', async ({ agent, error, signal }) => {
      if (isContextOverflowError(error)) {
        const result = await this.compactIfNeeded(agent, 'context-overflow', signal)
        if (result !== null) logResult(result, 'context-overflow recovery')
      }
    })
  }
}
```

### 3.5 压缩范围选择

**文件位置**：`packages/compaction/compaction-basic/src/region.ts`（Lines 1-561）

```typescript
export function selectCompactableRange(
  session: Session,
  measurement: TokenMeasurement,
  retainTokens: number,
): { start: number; end: number } | null {
  const surface = session.surface.nodes
  const totalTokens = measurement.totalTokens
  
  // 1. 计算需要释放的 Token 数
  const overflow = totalTokens - measurement.maxTokens * COMPACTION_THRESHOLD
  if (overflow <= 0) return null  // 无需压缩
  
  // 2. 保留最近的尾部（retainTokens）
  let tailStart = surface.length
  let accumulatedTokens = 0
  for (let i = surface.length - 1; i >= 0; i--) {
    accumulatedTokens += estimateTokens(surface[i])
    if (accumulatedTokens >= retainTokens) {
      tailStart = i
      break
    }
  }
  
  // 3. 找到平衡的压缩边界
  let start = 0
  let end = tailStart
  let compressedTokens = 0
  for (let i = 0; i < tailStart; i++) {
    compressedTokens += estimateTokens(surface[i])
    if (compressedTokens >= overflow) {
      end = i + 1
      break
    }
  }
  
  return { start, end }
}
```

### 3.6 LLM 驱动的摘要生成

**文件位置**：`packages/compaction/compaction-basic/src/summarizer.ts`（Lines 1-225）

```typescript
export async function summarizeWithLlm(
  session: Session,
  start: number,
  end: number,
  llm: LlmClient,
  signal: AbortSignal,
): Promise<string> {
  // 1. 重放对话前缀
  const prefix = session.surface.nodes.slice(0, start)
  const region = session.surface.nodes.slice(start, end)
  
  // 2. 构建压缩指令
  const instruction = [
    COMPACTION_INSTRUCTION,
    '## Conversation to Compact',
    ...formatMessages(region),
  ].join('\n\n')
  
  // 3. 调用 LLM 生成摘要
  const response = await llm.complete({
    messages: [
      ...formatMessages(prefix),  // 前缀作为上下文
      { role: 'user', content: instruction },
    ],
    signal,
  })
  
  return response.message.content
}

const COMPACTION_INSTRUCTION = [
  'You are now acting as a compaction engine...',
  '## Primary Request and Intent',
  '## Key Technical Concepts',
  '## Files and Code',
  '## Errors and Fixes',
  '## Pending Jobs',
  '## Current Work',
  '## Next Step',
  '## Critical Context',
].join('\n')
```

**摘要模板**：
- `Primary Request and Intent`：用户原始意图
- `Key Technical Concepts`：关键技术概念
- `Files and Code`：涉及的文件和代码
- `Errors and Fixes`：错误和修复
- `Pending Jobs`：待办任务
- `Current Work`：当前工作
- `Next Step`：下一步计划
- `Critical Context`：关键上下文

---

## 4. Yolo / 任务识别：Goal 域与 goalRounds

### 4.1 Goal 域设计

**文件位置**：`packages/core/goal/src/index.ts`

deepseek-harness **没有独立的 Yolo Agent**，而是通过 **Goal 域** 实现任务识别和跟踪：

```typescript
export interface Goal {
  id: string
  revision: number  // 乐观并发控制
  type: GoalType
  status: GoalStatus
  title: string
  description: string
  subgoals?: Goal[]
  metadata?: Record<string, unknown>
}

export type GoalType =
  | 'task'        // 通用任务
  | 'bug-fix'     // Bug 修复
  | 'feature'     // 功能开发
  | 'refactor'    // 重构
  | 'investigate' // 调查
  | 'question'    // 问答

export type GoalStatus =
  | 'pending'
  | 'in-progress'
  | 'completed'
  | 'failed'
  | 'cancelled'
```

### 4.2 GoalRounds 机制

**文件位置**：`packages/core/goal/src/goal-rounds.ts`

`GoalRounds` 是 Goal 与 Turn 的桥接机制：

```typescript
export class GoalRounds extends Service {
  private goals: Map<string, Goal> = new Map()
  private activeGoalId: string | undefined
  
  // 创建 Goal
  async createGoal(input: GoalInput): Promise<Goal> {
    const goal: Goal = {
      id: generateId(),
      revision: 0,
      status: 'pending',
      ...input,
    }
    this.goals.set(goal.id, goal)
    this.activeGoalId = goal.id
    this.ctx.emit('goal/created', { goal })
    return goal
  }
  
  // 更新 Goal（带乐观并发）
  async updateGoal(id: string, revision: number, updates: Partial<Goal>): Promise<Goal> {
    const goal = this.goals.get(id)
    if (!goal) throw new GoalNotFoundError(id)
    if (goal.revision !== revision) throw new GoalConflictError(id, revision, goal.revision)
    
    const updated: Goal = { ...goal, ...updates, revision: goal.revision + 1 }
    this.goals.set(id, updated)
    this.ctx.emit('goal/updated', { goal: updated })
    return updated
  }
  
  // 获取当前 Round 的 Goal
  getActiveGoal(): Goal | undefined {
    return this.activeGoalId ? this.goals.get(this.activeGoalId) : undefined
  }
}
```

### 4.3 任务识别流程

**文件位置**：`packages/core/goal/src/intent-recognition.ts`

```typescript
export async function recognizeIntent(
  input: string,
  history: Message[],
  llm: LlmClient,
): Promise<IntentRecognition> {
  // 1. 构建意图识别提示词
  const systemPrompt = `You are an intent recognition system.
    Analyze the user input and classify it into one of:
    - task: A specific task to accomplish
    - question: A question to answer
    - clarification: Seeking clarification
    - feedback: Providing feedback
    
    Also extract:
    - Goal type (bug-fix, feature, refactor, investigate)
    - Urgency (low, medium, high, critical)
    - Complexity (simple, medium, hard)
  `
  
  // 2. 调用 LLM
  const response = await llm.complete({
    messages: [
      { role: 'system', content: systemPrompt },
      ...history.slice(-5),  // 最近 5 条历史
      { role: 'user', content: input },
    ],
  })
  
  // 3. 解析响应
  return parseIntentResponse(response.message.content)
}
```

### 4.4 与 laew 的对比

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **任务识别** | Goal 域 + Intent Recognition | Yolo Agent（独立 Agent） |
| **分类方式** | GoalType + Complexity | TaskLevel（simple/medium/hard） |
| **状态跟踪** | Goal 实体 + 状态机 | 无持久化状态 |
| **并发控制** | 乐观并发（revision） | 无 |

---

## 5. 质检检查：Guard Plugin 与验证机制

### 5.1 Guard 架构

**文件位置**：`packages/guard/guard/src/index.ts`

Guard 是质检守卫插件，在工具执行前后进行验证：

```typescript
export class GuardRegistry extends Service {
  private guards: Map<string, Guard> = new Map()
  
  // 注册 Guard
  register(guard: Guard): () => void {
    this.guards.set(guard.name, guard)
    return () => this.guards.delete(guard.name)
  }
  
  // 执行 pre-execute 检查
  async preExecuteCheck(call: ToolCall, context: GuardContext): Promise<GuardResult> {
    for (const guard of this.guards.values()) {
      if (guard.preExecute) {
        const result = await guard.preExecute(call, context)
        if (result.action === 'block') {
          return result  // 阻止执行
        }
      }
    }
    return { action: 'allow' }
  }
  
  // 执行 post-execute 检查
  async postExecuteCheck(result: ToolResult, context: GuardContext): Promise<GuardResult> {
    for (const guard of this.guards.values()) {
      if (guard.postExecute) {
        const verification = await guard.postExecute(result, context)
        if (verification.action === 'retry') {
          return verification  // 要求重试
        }
      }
    }
    return { action: 'accept' }
  }
}
```

### 5.2 Guard 接口定义

```typescript
export interface Guard {
  name: string
  description: string
  
  // 执行前检查
  preExecute?: (call: ToolCall, context: GuardContext) => Promise<GuardDecision>
  
  // 执行后验证
  postExecute?: (result: ToolResult, context: GuardContext) => Promise<GuardDecision>
}

export type GuardDecision =
  | { action: 'allow' | 'accept' }
  | { action: 'block'; reason: string }
  | { action: 'retry'; reason: string; modifications?: Partial<ToolCall> }
```

### 5.3 内置 Guard 实现

**文件位置**：`packages/guard/guards/src/file-write-guard.ts`

```typescript
export const FileWriteGuard: Guard = {
  name: 'file-write-guard',
  description: '验证文件写入操作的安全性',
  
  async preExecute(call, context) {
    if (call.name !== 'write_file') return { action: 'allow' }
    
    const path = call.arguments.path
    
    // 1. 检查路径是否在允许的工作区内
    if (!isWithinWorkspace(path, context.workspace)) {
      return {
        action: 'block',
        reason: `Path "${path}" is outside workspace`,
      }
    }
    
    // 2. 检查是否是敏感文件
    if (isSensitiveFile(path)) {
      return {
        action: 'block',
        reason: `Writing to sensitive file "${path}" is not allowed`,
      }
    }
    
    // 3. 检查文件大小限制
    if (call.arguments.content.length > MAX_FILE_SIZE) {
      return {
        action: 'block',
        reason: `File size exceeds limit (${MAX_FILE_SIZE} bytes)`,
      }
    }
    
    return { action: 'allow' }
  },
  
  async postExecute(result, context) {
    // 验证文件确实写入成功
    if (result.error) return { action: 'accept' }
    
    const exists = await fileExists(result.path)
    if (!exists) {
      return {
        action: 'retry',
        reason: 'File was not created',
      }
    }
    
    return { action: 'accept' }
  },
}
```

### 5.4 与 laew 的对比

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **质检机制** | Guard Plugin（前置 + 后置） | Quality-Check Agent（独立 Agent） |
| **验证时机** | 工具执行前后 | 任务完成后 |
| **阻止能力** | 可阻止工具执行 | 仅报告问题 |
| **重试支持** | 支持修改后重试 | 无 |

---

## 6. 任务拆解：SubAgent Provider 与依赖管理

### 6.1 SubagentRuntime 架构

**文件位置**：`packages/subagent/subagent/src/index.ts`（Lines 1-639）

```typescript
export class SubagentRuntime extends TypertRemoteService {
  private providers = new Map<string, SubagentProvider>()
  private runs = new Map<string, SubagentRun>()
  
  // 注册 Provider
  registerProvider(provider: SubagentProvider): () => void {
    this.providers.set(provider.name, provider)
    return () => this.providers.delete(provider.name)
  }
  
  // 启动 SubAgent
  async start(name: string, request: SubagentStartRequest): Promise<SubagentRun> {
    const provider = this.expectProvider(name)
    
    // 1. 能力验证
    this.assertCapabilities(provider, request)
    
    // 2. 深度限制检查
    assertSubagentMaxDepth(request.maxDepth)
    
    // 3. 解析请求
    const resolved = await this.resolveRequest(request)
    
    // 4. 启动运行
    const run = await provider.start(resolved)
    
    // 5. 观察生命周期
    return observeRun(this.emitLifecycle, name, request.parent, run)
  }
  
  // 继续执行（多轮）
  async followup(runId: string, message: string): Promise<SubagentRun> {
    const run = this.getRun(runId)
    return await run.followup(message)
  }
  
  // 中断
  async interrupt(runId: string): Promise<void> {
    const run = this.getRun(runId)
    await run.interrupt()
  }
}
```

### 6.2 SubagentProvider 接口

```typescript
export interface SubagentProvider {
  name: string
  capabilities: SubagentCapabilities
  
  start(request: ResolvedSubagentRequest): Promise<SubagentRun>
  followup(runId: string, message: string): Promise<SubagentRun>
  interrupt(runId: string): Promise<void>
}

export interface SubagentCapabilities {
  // 支持的 Agent 选项
  agentOptions?: string[]
  
  // 输出 Schema
  outputSchema?: ValueSchema
  
  // 工具过滤器
  toolFilter?: (toolName: string) => boolean
  
  // 人格/角色
  persona?: string
  
  // 最大深度限制
  maxDepth?: number
}
```

### 6.3 能力验证

**文件位置**：`packages/subagent/subagent/src/capabilities.ts`

```typescript
export function assertCapabilities(
  provider: SubagentProvider,
  request: SubagentStartRequest,
): void {
  const caps = provider.capabilities
  
  // 1. 验证 Agent 选项
  if (request.agentOptions && caps.agentOptions) {
    for (const option of request.agentOptions) {
      if (!caps.agentOptions.includes(option)) {
        throw new SubagentCapabilityError(
          `Agent option "${option}" not supported by provider "${provider.name}"`
        )
      }
    }
  }
  
  // 2. 验证输出 Schema
  if (request.outputSchema && caps.outputSchema) {
    if (!isCompatibleSchema(request.outputSchema, caps.outputSchema)) {
      throw new SubagentCapabilityError(
        `Output schema incompatible with provider "${provider.name}"`
      )
    }
  }
  
  // 3. 验证工具过滤器
  if (request.toolFilter && caps.toolFilter) {
    // 确保请求的工具过滤是提供者过滤的子集
  }
}

export function assertSubagentMaxDepth(maxDepth: number): void {
  const MAX_DEPTH = 5  // 全局最大深度
  if (maxDepth > MAX_DEPTH) {
    throw new SubagentDepthError(
      `Max depth ${maxDepth} exceeds limit ${MAX_DEPTH}`
    )
  }
}
```

### 6.4 SubagentRun 生命周期

```typescript
export interface SubagentRun {
  id: string
  provider: string
  parent?: string  // 父 Run ID
  status: SubagentStatus
  
  // 多轮交互
  followup(message: string): Promise<SubagentRun>
  
  // 获取结果
  getResult(): Promise<SubagentResult>
  
  // 中断
  interrupt(): Promise<void>
}

export type SubagentStatus =
  | 'pending'
  | 'running'
  | 'waiting-for-parent'  // 等待父 Agent 输入
  | 'completed'
  | 'failed'
  | 'interrupted'
```

### 6.5 与 laew 的对比

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **SubAgent 架构** | Provider Registry + 能力验证 | 固定 SubAgent-Work Agent |
| **多轮支持** | `followup()` 方法 | 无（单轮委派） |
| **深度限制** | 全局 MAX_DEPTH = 5 | 无 |
| **能力验证** | Schema + Agent Options + Tool Filter | 无 |

---

## 7. 任务分类：Goal 类型与路由逻辑

### 7.1 Goal 类型系统

**文件位置**：`packages/core/goal/src/types.ts`

```typescript
export type GoalType =
  | 'task'        // 通用任务（默认）
  | 'bug-fix'     // Bug 修复
  | 'feature'     // 功能开发
  | 'refactor'    // 重构
  | 'investigate' // 调查/探索
  | 'question'    // 问答
  | 'multi-step'  // 多步骤任务

export interface GoalClassification {
  type: GoalType
  confidence: number  // 0-1
  reasoning: string
  subgoals?: GoalClassification[]
  estimatedComplexity: 'simple' | 'medium' | 'hard'
  requiredCapabilities: string[]
}
```

### 7.2 分类器实现

**文件位置**：`packages/core/goal/src/classifier.ts`

```typescript
export class GoalClassifier extends Service {
  private classifiers: GoalClassifierFn[] = []
  
  register(classifier: GoalClassifierFn): () => void {
    this.classifiers.push(classifier)
    return () => {
      const idx = this.classifiers.indexOf(classifier)
      if (idx >= 0) this.classifiers.splice(idx, 1)
    }
  }
  
  async classify(input: string, context: ClassificationContext): Promise<GoalClassification> {
    // 1. 并行运行所有分类器
    const results = await Promise.allSettled(
      this.classifiers.map(c => c(input, context))
    )
    
    // 2. 合并结果
    const classifications = results
      .filter((r): r is PromiseFulfilledResult<GoalClassification> => r.status === 'fulfilled')
      .map(r => r.value)
    
    // 3. 选择最高置信度
    return this.mergeClassifications(classifications)
  }
  
  private mergeClassifications(classifications: GoalClassification[]): GoalClassification {
    // 按置信度排序，取最高
    classifications.sort((a, b) => b.confidence - a.confidence)
    return classifications[0]
  }
}
```

### 7.3 路由逻辑

**文件位置**：`packages/core/goal/src/router.ts`

```typescript
export class GoalRouter extends Service {
  async route(goal: Goal): Promise<RouteDecision> {
    switch (goal.type) {
      case 'question':
        // 问答：直接回答，无需工具
        return { handler: 'direct-response', tools: [] }
      
      case 'investigate':
        // 调查：只读工具
        return {
          handler: 'investigation',
          tools: ['read_file', 'search', 'list_files'],
        }
      
      case 'bug-fix':
        // Bug 修复：读取 + 编辑 + 测试
        return {
          handler: 'bug-fix-workflow',
          tools: ['read_file', 'edit_file', 'run_test', 'search'],
          steps: ['reproduce', 'locate', 'fix', 'verify'],
        }
      
      case 'feature':
        // 功能开发：完整工具集 + SubAgent
        return {
          handler: 'feature-development',
          tools: 'all',
          useSubagent: true,
          steps: ['design', 'implement', 'test', 'document'],
        }
      
      case 'refactor':
        // 重构：分析 + 编辑 + 验证
        return {
          handler: 'refactoring',
          tools: ['read_file', 'edit_file', 'run_test', 'search'],
          steps: ['analyze', 'plan', 'execute', 'verify'],
        }
      
      case 'multi-step':
        // 多步骤：分解为子目标
        return {
          handler: 'multi-step',
          subgoals: goal.subgoals,
          parallel: false,
        }
      
      default:
        // 默认：通用任务
        return { handler: 'general', tools: 'all' }
    }
  }
}
```

### 7.4 与 laew 的对比

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **分类维度** | GoalType + Complexity + Capabilities | TaskLevel（simple/medium/hard） |
| **路由策略** | 基于类型的工作流路由 | 基于档位的 Agent 路由 |
| **动态分类** | 可扩展的 Classifier 注册 | 固定三步分析 |
| **能力匹配** | requiredCapabilities | 无 |

---

## 8. 工具调用：Tool Runtime 与 PTC 模式

### 8.1 ToolRuntime 架构

**文件位置**：`packages/core/tools/src/index.ts`（Lines 1-1946）

```typescript
export class ToolRuntime extends Service {
  static inject = ['systemPrompt']
  static Config: z<Config> = z.object({
    mode: z.union(['native', 'ptc', 'both']).default('native'),
    maxParallelSubCalls: z.natural().min(1).default(10),
  })
  
  // 分层工具注册
  private readonly layers = new ScopedLayers(
    scope => new ToolLayer(scope),
    () => { this.ctx.emit('tools/change') },
  )
  
  // 注册工具
  register(tool: ToolDefinition, scope?: string): () => void {
    return this.layers.effect(
      this.ctx,
      (layer) => {
        layer.tools.set(tool.name, tool)
        return () => layer.tools.delete(tool.name)
      },
      { label: `tools.register(${tool.name})` }
    )
  }
  
  // 列出工具
  list(scope?: string): ToolDefinition[] {
    const layers = scope
      ? [this.layers.global, ...this.layers.chainLayers(scope)]
      : [this.layers.global]
    
    const tools = new Map<string, ToolDefinition>()
    for (const layer of layers) {
      for (const [name, tool] of layer.tools) {
        tools.set(name, tool)  // 近层覆盖远层
      }
    }
    return [...tools.values()]
  }
  
  // 执行工具
  async execute(call: ToolCall, options: ExecuteOptions): Promise<ToolResult> {
    const tool = this.findTool(call.name, options.scope)
    if (!tool) {
      return { error: `Unknown tool: ${call.name}` }
    }
    
    // 1. Pre-execute guards
    const guardResult = await this.guardRegistry.preExecuteCheck(call, options.context)
    if (guardResult.action === 'block') {
      return { error: guardResult.reason }
    }
    
    // 2. Execute
    const result = await tool.execute(call.arguments, {
      signal: options.signal,
      bindings: this.createBindings(options),
    })
    
    // 3. Post-execute guards
    const verifyResult = await this.guardRegistry.postExecuteCheck(result, options.context)
    if (verifyResult.action === 'retry') {
      // 重试逻辑
      return this.executeWithModifications(call, verifyResult.modifications, options)
    }
    
    return result
  }
}
```

### 8.2 defineTool 辅助函数

**文件位置**：`packages/core/tools/src/schema.ts`（Lines 1-618）

```typescript
export function defineTool<
  const S extends ParameterSchemaSpec,
  const O extends ValueSchemaSpec
>(options: DefineToolOptions<S, O>): ToolDefinition {
  // 1. 转换参数 Schema
  const parameters = parameterSchemaSpecToJsonSchema(options.parameters)
  
  // 2. 转换输出 Schema
  const outputSchema = valueSchemaSpecToJsonSchema(options.output.schema)
  
  // 3. 构建定义
  return {
    name: options.name,
    description: options.description,
    parameters,
    outputSchema,
    async execute(args: InferArgs<S>, exec: ExecContext): Promise<InferValue<O>> {
      // 参数验证
      const validated = validateArgs(options.parameters, args)
      // 执行处理器
      return await options.execute(validated, exec)
    },
  }
}

// 类型推断
export type InferArgs<S extends ParameterSchemaSpec> = {
  [K in keyof S]: S[K] extends { required: true }
    ? InferType<S[K]>
    : InferType<S[K]> | undefined
}

export type InferValue<S extends ValueSchemaSpec> = InferType<S>
```

### 8.3 PTC 模式（Program-Then-Collapse）

**文件位置**：`packages/core/tools/src/ptc.ts`（Lines 1-683）

PTC 是 SDK 风格的代码执行模式：

```typescript
export function createRunCodeTool(
  registry: ToolRuntime,
  options: RunCodeBridgeOptions
): ToolDefinition {
  return defineTool({
    name: RUN_CODE_NAME,  // 'run_code'
    parameters: {
      code: {
        type: 'string',
        required: true,
        description: TYPESCRIPT_FLAVOR.codeDescription,
      },
      description: {
        type: 'string',
        required: true,
        description: RUN_CODE_DESCRIPTION_PARAM_DESCRIPTION,
      },
    },
    async execute(args, exec): Promise<RunCodeOutput> {
      // 1. 构建绑定函数（SDK 调用）
      const binding = (name: string): CodeBindingFunction => {
        return async (rawArgs) => {
          // 将 SDK 调用桥接到 ToolRuntime
          return registry.execute(
            { name, arguments: rawArgs },
            { signal: exec.signal, context: exec.context }
          )
        }
      }
      
      // 2. 收集所有可用工具作为绑定
      const tools = registry.list(exec.scope)
      const functions = {}
      for (const tool of tools) {
        functions[tool.name] = binding(tool.name)
      }
      
      // 3. 运行代码
      const result = await runtime.run({
        program: args.code,
        bindings: [{ global: 'tools', functions }],
      })
      
      return { output: result.output, error: result.error }
    },
  })
}
```

**PTC 执行流程**：
1. LLM 生成 `run_code` 调用，包含 TypeScript 代码
2. 代码在沙箱中执行，通过 `tools` 全局对象调用工具
3. 工具调用被桥接到 ToolRuntime
4. 执行结果返回给 LLM

### 8.4 与 laew 的对比

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **工具注册** | ScopedLayers（分层隔离） | ToolRegistry（扁平注册） |
| **执行管道** | pre-execute → execute → post-execute | 直接执行 |
| **PTC 模式** | `run_code` 工具（SDK 风格） | 无 |
| **并行执行** | maxParallelSubCalls 配置 | 无 |
| **Guard 集成** | Guard Registry 前置后置检查 | 无 |

---

## 9. MCP 设计：stdio/HTTP 传输与 Client 实现

### 9.1 MCP Client 架构

**文件位置**：`packages/mcp/mcp-client/src/index.ts`（Lines 1-189）

```typescript
export async function apply(ctx: Context, config: Config): Promise<void> {
  // 1. 解析重连策略
  const reconnect = resolveReconnectPolicy(config.reconnect, {
    maxAttempts: 5,
    initialDelayMs: 1000,
    maxDelayMs: 30000,
  })
  
  // 2. 保留 serverName（防止冲突）
  ctx.effect(() => {
    reserveServerName(config.serverName)
    return () => releaseServerName(config.serverName)
  }, 'mcp-client.serverName')
  
  // 3. 启动连接
  const connection = startConnection(ctx, config, reconnect)
  
  // 4. 注册清理
  ctx.effect(() => {
    return () => connection.dispose()
  }, 'mcp-client.connection')
  
  // 5. 等待就绪
  const outcome = await connection.ready
  
  if (outcome.kind === 'error') {
    ctx.logger.error(`MCP connection failed: ${outcome.error}`)
  }
}
```

### 9.2 连接管理

**文件位置**：`packages/mcp/mcp-client/src/connection.ts`（Lines 1-352）

```typescript
export interface McpConnection {
  kind: 'stdio' | 'streamable-http'
  client: Client
  transport: Transport
  dispose(): void
}

function startConnection(
  ctx: Context,
  config: Config,
  reconnect: ReconnectPolicy
): McpConnection {
  let currentGeneration = 0
  let connectedAt: number | undefined
  let failedAttempts = 0
  let reconnectTimer: Timeout | undefined
  
  // 连接函数
  async function connect(generation: number): Promise<void> {
    try {
      // 1. 创建传输
      const transport = createTransport(config)
      
      // 2. 创建客户端
      const client = new Client({ name: 'dsh', version: VERSION })
      
      // 3. 连接
      await client.connect(transport)
      
      // 4. 更新状态
      connectedAt = Date.now()
      failedAttempts = 0
      currentGeneration = generation
      
      // 5. 发现工具
      await discoverAndRegisterTools(client, config.serverName)
      
    } catch (error) {
      scheduleReconnect()
    }
  }
  
  // 重连调度
  function scheduleReconnect(): void {
    // 重置失败计数（如果连接稳定足够久）
    if (connectedAt !== undefined && Date.now() - connectedAt >= policy.maxDelayMs) {
      failedAttempts = 0
    }
    
    failedAttempts += 1
    
    // 超过最大尝试次数，放弃
    if (failedAttempts > policy.maxAttempts) {
      ctx.logger.error(`MCP reconnection gave up after ${policy.maxAttempts} attempts`)
      return
    }
    
    // 指数退避
    const delayMs = Math.min(
      policy.maxDelayMs,
      policy.initialDelayMs * 2 ** (failedAttempts - 1)
    )
    
    reconnectTimer = setTimeout(() => {
      connect(connectGeneration(false))
    }, delayMs)
  }
  
  // 初始连接
  connect(connectGeneration(true))
  
  return {
    kind: config.kind,
    client,
    transport,
    dispose() {
      clearTimeout(reconnectTimer)
      transport.close()
    },
  }
}
```

### 9.3 传输层实现

**stdio 传输**：

```typescript
function createStdioTransport(config: StdioConfig): StdioTransport {
  return new StdioTransport({
    command: config.command,
    args: config.args,
    env: config.env,
    cwd: config.cwd,
  })
}
```

**streamable-http 传输**：

```typescript
function createStreamableHttpTransport(config: HttpConfig): StreamableHttpTransport {
  return new StreamableHttpTransport({
    url: config.url,
    headers: config.headers,
    auth: config.auth,
  })
}
```

### 9.4 工具桥接

**文件位置**：`packages/mcp/mcp-client/src/tools.ts`（Lines 1-560）

```typescript
export function publicToolName(serverName: string, rawName: string): string {
  const joined = `mcp__${serverName}__${rawName}`
  const normalized = joined.replace(INVALID_NAME_CHARS, '_')
  
  // 如果名称合法且长度合规，直接返回
  if (normalized === joined && normalized.length <= MAX_PUBLIC_NAME_LENGTH) {
    return normalized
  }
  
  // 否则使用 SHA-256 哈希缩短
  const hash = createHash('sha256')
    .update(`${serverName}\0${rawName}`)
    .digest('hex')
    .slice(0, HASH_LENGTH)
  
  return `${normalized.slice(0, MAX_PUBLIC_NAME_LENGTH - HASH_LENGTH - 1)}_${hash}`
}

async function discoverAndRegisterTools(
  client: Client,
  serverName: string
): Promise<void> {
  // 1. 列出 MCP 工具
  const { tools } = await client.listTools()
  
  // 2. 注册到 ToolRuntime
  for (const mcpTool of tools) {
    const publicName = publicToolName(serverName, mcpTool.name)
    
    toolRuntime.register({
      name: publicName,
      description: `[${serverName}] ${mcpTool.description}`,
      parameters: mcpTool.inputSchema,
      async execute(args, exec) {
        // 调用 MCP 工具
        const result = await client.callTool({
          name: mcpTool.name,
          arguments: args,
        })
        
        // 处理图片投影
        if (result.content) {
          return projectContent(result.content, exec.context)
        }
        
        return result
      },
    })
  }
}
```

### 9.5 与 laew 的对比

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **MCP 支持** | 完整（stdio + HTTP） | 无 |
| **工具命名** | `mcp__<server>__<tool>` + 哈希 | N/A |
| **重连机制** | 指数退避 + 最大尝试 | N/A |
| **图片投影** | 解码 MCP 图片块到附件存储 | N/A |

---

## 10. SKILL 设计：Layered Registry 与动态扩展

### 10.1 SkillRegistry 架构

**文件位置**：`packages/skill/skill/src/index.ts`（Lines 1-869）

```typescript
export class SkillRegistry extends Service {
  // 分层 Provider 注册
  private readonly layers = new ScopedLayers<SkillLayer>(
    scope => new SkillLayer(scope),
    () => { this.invalidateCache() },
  )
  
  // Provider 注册
  registerProvider(create: (control: SkillProviderControl) => SkillProvider): () => void {
    const provider = create({
      ctx: this.ctx,
      invalidate: () => this.invalidateCache(),
    })
    
    return this.layers.effect(
      this.ctx,
      (layer) => {
        const undo = layer.providers.insert(provider.name, { provider, order })
        return () => {
          undo()
          lifecycle.abort(new Error(`skill provider "${provider.name}" disposed`))
        }
      },
      { label: 'skills.registerProvider()' }
    )
  }
  
  // 运行时 Skill 注册
  register(skill: SkillRegistration): () => void {
    validateRuntimeSkill(skill)
    const scope = scopeOf(this.ctx)
    const existingLayer = scope === undefined ? this.layers.global : this.layers.peek(scope)
    
    // 重复检查
    if (existingLayer !== undefined && existingLayer.runtime.has(skill.name)) {
      this.ctx.logger.warn(`runtime skill "${skill.name}" ignored because it is already registered`)
      return () => {}
    }
    
    const definition: SkillDefinition = {
      ...skill,
      invocation: skill.invocation ?? { modelInvocable: true, userInvocable: true },
      provider: skill.provider ?? RUNTIME_PROVIDER,
    }
    
    return this.layers.effect(
      this.ctx,
      (layer) => {
        layer.runtime.set(definition.name, definition)
        return () => layer.runtime.delete(definition.name)
      },
      { label: 'skills.register()' }
    )
  }
  
  // 列出 Skill
  async list(options: SkillViewOptions = {}): Promise<SkillSummary[]> {
    return (await this.snapshot(options)).skills
  }
  
  // 获取 Skill
  async get(name: string, options: SkillViewOptions = {}): Promise<SkillDefinition | undefined> {
    if (!isSkillName(name)) return undefined
    const collected = await this.collect(options)
    throwIfAborted(options.signal)
    const match = collected.entries.get(name)
    if (match === undefined) return undefined
    const definition = await waitWithAbort(match.provider.get(match.candidate, options), options.signal)
    if (definition === undefined) return undefined
    validateDefinition(definition)
    return definition
  }
}
```

### 10.2 分层架构

```typescript
class SkillLayer {
  scope: string | undefined  // undefined = global
  
  // Provider 注册（有序）
  providers = new OrderedMap<string, { provider: SkillProvider; order: number }>()
  
  // 运行时 Skill（Map）
  runtime = new Map<string, SkillDefinition>()
}

class ScopedLayers<T extends { scope: string | undefined }> {
  global: T  // 全局层
  
  // 作用域链
  private scopes = new Map<string, T>()
  
  // 获取作用域链（从远到近）
  chainLayers(scope: string | undefined): T[] {
    if (!scope) return []
    const chain = []
    let current = scope
    while (current) {
      const layer = this.scopes.get(current)
      if (layer) chain.unshift(layer)  // 远的在前
      current = parentScope(current)
    }
    return chain
  }
}
```

### 10.3 优先级系统

**文件位置**：`packages/skill/skill/src/rank.ts`

```typescript
// 优先级（数值越大，优先级越高）
export const enum SkillRank {
  ProjectDSH = 100,      // .dsh/skill.md
  ProjectAgents = 200,   // .agents/skill.md
  Custom = 300,          // 自定义 Provider
  UserDSH = 400,         // ~/.dsh/skill.md
  UserAgents = 500,      // ~/.agents/skill.md
  Bundled = 600,         // 内置 Skill
  Runtime = 250,         // 运行时注册（特殊）
}

// 比较函数
export function compareSkillSummary(a: SkillSummary, b: SkillSummary): number {
  // 1. 按 Rank 降序
  if (a.rank !== b.rank) return b.rank - a.rank
  
  // 2. 按 Provider Order 升序
  if (a.providerOrder !== b.providerOrder) return a.providerOrder - b.providerOrder
  
  // 3. 按名称字母序
  return compareCodePoints(a.name, b.name)
}
```

### 10.4 Skill Provider 接口

```typescript
export interface SkillProvider {
  name: string
  
  // 列出候选
  list(options: SkillLookupOptions): Promise<SkillCandidate[]>
  
  // 获取完整定义
  get(candidate: SkillCandidate, options: SkillLookupOptions): Promise<SkillDefinition | undefined>
}

export interface SkillDefinition {
  name: string           // kebab-case
  description: string
  body: string           // Markdown 内容
  source: string         // 来源标识
  rank: SkillRank
  provider: string
  
  // 调用策略
  invocation: {
    modelInvocable: boolean   // 模型是否可调用
    userInvocable: boolean    // 用户是否可调用
  }
  
  // 元数据
  metadata?: Record<string, unknown>
}
```

### 10.5 文件系统 Provider

**文件位置**：`packages/skill/skill-filesystem/src/index.ts`

```typescript
export function createFilesystemProvider(options: FilesystemProviderOptions): SkillProvider {
  return {
    name: options.name,
    
    async list(listOptions) {
      const skills: SkillCandidate[] = []
      
      // 1. 扫描目录
      const files = await scanSkillFiles(options.directories, listOptions.cwd)
      
      for (const file of files) {
        // 2. 解析 frontmatter
        const parsed = await parseSkillFile(file)
        if (!parsed) continue
        
        // 3. 构建候选
        skills.push({
          name: parsed.name,
          source: file.path,
          rank: options.rank,
          provider: options.name,
          locator: { kind: 'filesystem', path: file.path },
        })
      }
      
      return skills
    },
    
    async get(candidate, getOptions) {
      // 从 locator 加载完整内容
      const content = await readFile(candidate.locator.path)
      return parseSkillDefinition(content)
    },
  }
}
```

### 10.6 与 laew 的对比

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **Skill 系统** | 完整（Layered Registry + Provider） | 无 |
| **分层机制** | Global + Scope + Rank 优先级 | N/A |
| **Provider 扩展** | 可注册自定义 Provider | N/A |
| **文件系统加载** | .dsh/、.agents/、内置 | N/A |
| **调用策略** | modelInvocable + userInvocable | N/A |

---

## 总结：deepseek-harness 架构全景

### 核心设计模式

1. **Everything-is-a-Plugin**：所有能力都是 Cordis 插件，通过 `ctx.plugin()` 加载
2. **Event Sourcing**：Session 使用不可变事件流，支持回放和压缩
3. **Scoped Layers**：工具和 Skill 支持 per-scope 隔离和覆盖
4. **Provider Registry**：SubAgent 和 Skill 使用 Provider 模式，支持扩展
5. **Guard Pipeline**：工具执行前后可插入验证逻辑
6. **PTC 模式**：SDK 风格的代码执行，桥接工具调用

### 与 laew 的关键差异

| 维度 | deepseek-harness | laew |
|------|-----------------|------|
| **框架** | Cordis（IoC/DI） | 无（直接实现） |
| **架构风格** | Everything-is-a-Plugin | 模块化但非插件化 |
| **任务识别** | Goal 域 + Intent Recognition | Yolo Agent |
| **质检机制** | Guard Plugin（前置后置） | Quality-Check Agent |
| **SubAgent** | Provider Registry + 多轮 | 固定 Agent + 单轮 |
| **MCP** | 完整支持 | 无 |
| **Skill** | Layered Registry | 无 |
| **PTC** | `run_code` 工具 | 无 |

### 可借鉴的设计

1. **Event Sourcing**：用于审计、回放、压缩
2. **Scoped Layers**：用于 per-project 的工具和 Skill 隔离
3. **Guard Pipeline**：用于工具执行的安全控制
4. **Provider Registry**：用于 SubAgent 和 Skill 的可扩展性
5. **PTC 模式**：用于 SDK 风格的代码执行

---

**报告完成日期**：2026-09-04
**分析文件数**：50+ 核心源文件
**代码行数**：约 15,000 行深度阅读
