# 专题-第十七轮-deepseek-harness-深度分析

> **调研日期**：2026-09-09
> **调研范围**：崩溃恢复与取证、多租户隔离、RRF检索、LLM网关路由、Pregel图执行、Skill生命周期、Agent预热池、Turn锁、HTTP客户端高级实现、安全加固
> **本轮新增 gap**：L1166-L1205

---

## 一、崩溃恢复与取证（Crash Recovery & Forensics）

### 1.1 Session Checkpoint Policy（语义检查点）

**文件**：`packages/session/session-checkpoint-policy/src/index.ts`

deepseek-harness 实现了**三层语义检查点**机制，确保模型请求、工具调用的原子性：

```typescript
// 三层检查点：
// 1. llm/stream → 模型请求前刷新 Session 到持久化存储
// 2. tools/execute → 顶层工具调用前刷新 Session
// 3. agent/pre-step → 每个 Step 前刷新 Session

ctx.on('llm/stream', (options, next) => {
  if (options.sessionId === undefined) return next()
  const session = ctx.sessions.get(options.sessionId)
  return session === undefined ? next() : afterCheckpoint(ctx, session, next)
})

ctx.on('tools/execute', async (exec, next) => {
  if (exec.agent === undefined || exec.parent !== undefined) return next()
  await ctx.sessions.flush(exec.agent.session)
  if (exec.signal.aborted) return abortedBeforeDispatchResult()
  return next()
})
```

**关键设计**：
- **Fail-closed 策略**：检查点失败时，下游适配器或工具体不会被调用
- **嵌套工具复用**：嵌套工具调度复用外层调用已持久化的结果
- **取消前保护**：工具取消前已持久化的调用结果被保留

### 1.2 Write-Behind 缓冲与故障恢复

**文件**：`packages/session/session-persistence/src/write-behind.ts`

```typescript
export class SessionWriteBehind {
  private pending: SessionEvent[] = []
  private timer: ReturnType<typeof setTimeout> | undefined
  private active: Promise<void> | undefined
  private barrier: Promise<void> | undefined
  private deadlineExpired = false
  private automaticPaused = false

  enqueue(event: SessionEvent): void {
    const wasEmpty = this.pending.length === 0
    this.pending.push(structuredClone(event))
    if (this.barrier !== undefined) return
    if (this.automaticPaused) {
      this.automaticPaused = false
      this.deadlineExpired = false
      this.armTimer()
    } else if (wasEmpty) {
      this.armTimer()
    }
  }

  flush(): Promise<void> {
    if (this.barrier !== undefined) return this.barrier
    this.cancelTimer()
    this.deadlineExpired = false
    this.automaticPaused = false
    const barrier = Promise.withResolvers<void>()
    this.barrier = barrier.promise
    void this.drainBarrier(barrier.resolve, barrier.reject)
    return barrier.promise
  }
}
```

**故障恢复机制**：
- **失败重试**：写入失败时，批次事件被保留在队列中（`this.pending = batch.concat(this.pending)`）
- **后台失败报告**：后台写入失败通过 `reportBackgroundFailure` 报告，不阻塞生产者
- **屏障同步**：并发刷新调用加入同一屏障，避免重复刷新

### 1.3 Session Persistence Coordinator（协调器）

**文件**：`packages/session/session-persistence/src/coordinator.ts`

协调器实现了**崩溃恢复的核心逻辑**：

```typescript
// 四种会话恢复场景：
// Case 1: 同一 Session ID 已持久化 → 检测 ID 冲突
// Case 2: 已存储的 Session 被重新加载 → 采用存储前缀
// Case 3: 全新 Session → 创建元数据并持久化种子
// Case 4: HMR/重载 → 采用存储前缀但不关闭开放 Turn

private async adoptLivePrefix(session, seed, stored): Promise<void> {
  const { meta, events, tornMarker } = stored
  this.assertStoredId(session.header.id, meta)
  if (meta.cwd !== session.header.cwd) {
    throw new Error(`session already persisted at different cwd`)
  }
  if (tornMarker !== undefined) await this.backend.commitRepair(meta, tornMarker, [])
  const suffix = seed.slice(storedEvents.length)
  if (suffix.length > 0) await this.appendCore(session.header.id, suffix)
}
```

**关键设计**：
- **Torn-tail 修复**：检测并截断崩溃时的不完整写入
- **ID 冲突检测**：防止不同 Session 复用同一 ID
- **CWD 验证**：确保恢复的 Session 与当前工作目录匹配

### 1.4 JSONL 持久化后端的崩溃恢复

**文件**：`packages/session/session-persistence-jsonl/src/index.ts`

```typescript
// Zstandard 帧校验
function assertZstdHeaderFrame(plaintext: Buffer): void {
  if (plaintext.length === 0 || plaintext.indexOf(0x0A) !== plaintext.length - 1) {
    throw new Error('corrupt Zstandard session log: first frame is not exactly one header line')
  }
}

// 文件修订身份（用于检测并发修改）
interface FileRevisionIdentity {
  readonly dev: bigint; readonly ino: bigint; readonly size: bigint
  readonly mtimeNs: bigint; readonly ctimeNs: bigint
}
```

**取证能力**：
- **Zstandard 帧校验**：检测日志文件损坏
- **文件修订身份**：通过 dev/ino/size/mtime/ctime 检测并发修改
- **损坏会话标记**：`SessionPersistenceCorruptionError` 区分损坏与格式不支持

### 1.5 Runtime Diagnostics（运行时诊断）

**文件**：`packages/runtime-diagnostics/invariants/src/index.ts`

```typescript
export class InvariantRegistry extends Service {
  register(packageName: string, installer: InvariantInstaller): () => void {
    // 支持 allowlist/blocklist 过滤
    // 失败时抛出 InvariantError
  }
}
```

**关键设计**：
- **包级不变量**：每个包可注册运行时检查
- **过滤机制**：支持 allowlist/blocklist 选择启用的包
- **失败隔离**：单个包失败不影响其他包

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 Fail-closed 检查点策略（模型请求前刷新 Session） | P0 |
| L1167 | 崩溃恢复 | 无 Write-Behind 缓冲（批量写入 + 失败重试） | P0 |
| L1168 | 崩溃恢复 | 无 Torn-tail 修复（截断不完整写入） | P1 |
| L1169 | 崩溃恢复 | 无文件修订身份（dev/ino/size/mtime/ctime 校验） | P1 |
| L1170 | 取证 | 无包级不变量注册机制 | P2 |


---

## 二、多租户隔离（Multi-tenant Isolation）

### 2.1 Cordis Isolate（隔离作用域）

**文件**：`vendor/cordis/src/context.ts`

```typescript
// Context 的三个关键子上下文操作：
// 1. extend(meta) — 创建子 context（原型继承，不污染父）
// 2. isolate(name, label?) — 创建独立 service 作用域
// 3. intercept(name, config) — 注入 service 拦截配置

constructor() {
  this[symbols.isolate] = Object.create(null)
  this[symbols.intercept] = Object.create(null)
  const self = new Proxy<this>(this, ReflectService.handler)
  return self
}
```

**隔离机制**：
- **原型继承隔离**：子 context 通过原型链继承父 context，但修改不污染父
- **Service 隔离**：`isolate()` 创建独立 service 作用域，用于 SubAgent 隔离
- **拦截配置**：`intercept()` 注入 service 拦截配置，实现策略隔离

### 2.2 Agent 作用域隔离

**文件**：`packages/core/agent-loop/src/agent.ts`

```typescript
export class ReactLoopAgent implements Agent {
  readonly scope: Scope
  readonly ctx: Context

  constructor(...) {
    // 每个 Agent 拥有独立的作用域
    this.scope = createScope(loopCtx, this)
    this.ctx = this.scope.ctx.extend({ agent: this })
  }
}
```

**关键设计**：
- **独立作用域**：每个 Agent 创建独立的 Cordis Scope
- **上下文扩展**：Agent 的 ctx 从 scope 扩展，包含 agent 引用
- **事件分发隔离**：Agent 事件在自身作用域内分发，不影响其他 Agent

### 2.3 SubAgent 隔离

**文件**：`packages/subagent/subagent/src/continuation.ts`

```typescript
export interface Activation {
  childId: SessionId
  parentSession: SessionId
  handle: AgentHandle
  ancestry: readonly SessionId[]
  ownedChildren: Set<SessionId>
  observer: ActivationObserver
  disposal: Promise<void>
}
```

**隔离机制**：
- **所有权图**：`acquireOwnership/releaseOwnership` 防止循环委托
- **子优先释放**：`finishDisposal` 先取消子代理的子代理，再释放自身
- **激活图**：跟踪 childId/parentSession/ancestry 关系

### 2.4 匿名用户 ID

**文件**：`packages/identity/anonymous-user-id/src/index.ts`

```typescript
export function getOrCreateAnonymousUserId(options = {}): AnonymousUserId {
  const file = join(resolveDshHome(undefined, options.env ?? process.env), ANONYMOUS_USER_ID_FILE_NAME)
  const cached = memo.get(file)
  if (cached !== undefined) return cached

  let id = readPersistedId(file)
  if (id === undefined) {
    const created = generate() as AnonymousUserId
    try {
      mkdirSync(dirname(file), { recursive: true })
      writeFileSync(file, `${created}\n`, { encoding: 'utf8', flag: 'wx' })
      id = created
    } catch {
      id = readPersistedId(file) // 并发创建失败时读取获胜者
    }
  }
  memo.set(file, id)
  return id
}
```

**关键设计**：
- **Harness-home 作用域**：每个 `$DSH_HOME` 一个匿名 ID
- **非标识性**：随机 UUID，不基于主机名/网络地址/git remote
- **并发安全**：使用 `wx` 标志原子创建，失败时读取获胜者

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1171 | 多租户 | 无 Cordis Isolate 隔离作用域 | P0 |
| L1172 | 多租户 | 无 Agent 作用域隔离（独立 Scope + ctx） | P1 |
| L1173 | 多租户 | 无 SubAgent 所有权图（防循环委托） | P1 |
| L1174 | 多租户 | 无匿名用户 ID（harness-home 作用域 UUID） | P2 |

---

## 三、RRF检索（Hybrid Retrieval）

### 3.1 Session Query Engine（查询引擎）

**文件**：`packages/session-query/session-query/src/index.ts`

```typescript
export abstract class SessionQueryEngine extends Service {
  // 搜索 Session（跨 Session 全文搜索）
  abstract searchSessions(
    request: SessionSearchRequest,
    exec?: SessionSearchExecContext,
  ): Promise<SessionSearchPage<SessionSearchHit>>

  // 搜索事件（Session 内全文搜索）
  abstract searchEvents(
    request: SessionEventSearchRequest,
    exec?: SessionSearchExecContext,
  ): Promise<SessionEventSearchPage<SessionEventSearchHit>>

  // 过滤 Session
  async filterSessions(filters: readonly SessionResultFilter[], signal?: AbortSignal): Promise<SessionRecord[]>
  // 过滤事件
  async filterEvents(sessionId: SessionId, filters: readonly SessionEventResultFilter[]): Promise<SessionEventSearchDocument[]>
}
```

**关键设计**：
- **统一查询引擎**：抽象基类定义搜索/过滤/追踪接口
- **Live-preferred**：优先从内存读取，持久化作为后备
- **并发控制**：`persistedInspectConcurrency` 限制并发持久化检查

### 3.2 SQLite FTS5 全文搜索

**文件**：`packages/session-query/session-query-sqlite/src/index.ts`

```typescript
export class SqliteSessionQueryEngine extends SessionQueryEngine {
  // FTS5 全文搜索实现
  // 支持 WAL 模式、snippet 生成（高亮匹配）、cursor 分页

  static Config: z<Config> = z.object({
    path: z.string().required(),
    openAt: z.union(['startup', 'first-search', 'never'] as const).default('startup'),
    journalMode: z.union(['wal', 'delete', 'truncate', 'persist'] as const).default('wal'),
    defaultLimit: z.number().default(SESSION_QUERY_SQLITE_DEFAULT_LIMIT),
    maxLimit: z.number().default(SESSION_QUERY_SQLITE_MAX_LIMIT),
    snippetChars: z.number().default(SESSION_QUERY_SQLITE_SNIPPET_CHARS),
  })
}
```

**FTS5 搜索特性**：
- **高亮标记**：使用 Unicode 私有区字符 `\uFDD0`/`\uFDD1` 标记匹配
- **Snippet 生成**：`makeSnippet()` 提取匹配上下文
- **参数化查询**：防止 SQL 注入
- **Cursor 分页**：Base64url 编码的游标，支持稳定分页

### 3.3 查询归一化与过滤

**文件**：`packages/session-query/session-query-sqlite/src/query.ts`

```typescript
// 归一化跨 Session 请求
export function normalizeSessionRequest(request, limits): NormalizedSessionRequest {
  const sessionFilters = materializeSessionResultFilters(request.sessionFilters ?? [])
  const eventFilters = materializeMetadataFilters(request.eventFilters ?? [])
  const cursor = materializeCursor(request.cursor)
  return {
    query: normalizeQuery(request.query),
    sessionFilters, eventFilters,
    limit: normalizeLimit(request.limit, limits),
    ...cursor === undefined ? {} : { cursor },
  }
}

// FTS5 预算限制
export const SQLITE_FTS5_OUTER_PREDICATE_LIMIT = 14
export const SQLITE_PORTABLE_VARIABLE_LIMIT = 32_766
```

**关键设计**：
- **请求归一化**：验证并规范化查询参数
- **FTS5 预算限制**：防止查询过复杂
- **便携变量限制**：防止超出 SQLite 变量上限

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1175 | 检索 | 无 FTS5 全文搜索（Session 历史检索） | P0 |
| L1176 | 检索 | 无 Cursor 分页（Base64url 编码游标） | P1 |
| L1177 | 检索 | 无 Snippet 生成（匹配高亮上下文） | P1 |
| L1178 | 检索 | 无 Live-preferred 查询（内存优先，持久化后备） | P1 |

---

## 四、LLM网关路由（LLM Gateway Routing）

### 4.1 LlmRuntime（适配器注册表）

**文件**：`packages/llm/llm/src/index.ts`

```typescript
export class LlmRuntime extends Service {
  private readonly adapters = new Map<string, AdapterRegistration>()

  registerAdapter(adapter: LlmAdapter, provider: LlmProviderInfo): () => void {
    const registration: AdapterRegistration = {
      adapter, provider,
      retryPolicy: resolveRetryPolicy(provider.retryPolicy, provider.id),
    }
    this.adapters.set(provider.id, registration)
    this.emit('llm/adapters-updated')
    return () => {
      if (this.adapters.get(provider.id) === registration) {
        this.adapters.delete(provider.id)
        this.emit('llm/adapters-updated')
      }
    }
  }

  stream(options: GenerateOptions): AsyncIterable<StreamChunk> {
    return this.ctx.waterfall(this, 'llm/stream', options, () => this.adapterStream(options, prepared))
  }
}
```

**关键设计**：
- **适配器注册表**：Map 结构，支持动态注册/注销
- **Waterfall 拦截**：`llm/stream` 事件支持中间件拦截
- **不可变快照**：每个操作捕获不可变快照，配置变更不影响进行中的请求

### 4.2 Pi-AI 多 Provider 适配

**文件**：`packages/llm/llm-pi-ai/src/adapter.ts`

```typescript
export class PiAiAdapter extends LlmAdapter {
  // 不可变快照（配置变更时重建）
  private current(): PiAiSnapshot {
    const profiles = this.config.profiles()
    return { profiles, models: this.buildModels(profiles) }
  }

  private async * streamWithSnapshot(options, snapshot): AsyncIterable<StreamChunk> {
    const watchdog = idleWatchdog(upstream, streamIdleTimeoutMs, 'LLM_STREAM_IDLE_TIMEOUT')
    const events = snapshot.models.streamSimple(model, context, {
      ...profileOptions(profile, reasoning, apiKey),
      signal: watchdog.signal,
    })
  }
}
```

**多 Provider 支持**：
- **协议表**：`openai-completions`、`openai-responses`、`anthropic-messages`
- **Catalog 复用**：Catalog 路由复用已安装的 Provider（保留 API 实现）
- **空闲看门狗**：`idleWatchdog` 检测流式调用空闲超时

### 4.3 重试策略

**文件**：`packages/llm/llm/src/retry-policy.ts`

```typescript
// 两种重试模式：
// 1. normal: 有限重试（默认 5 次），仅重试配置的瞬态错误码
// 2. always: 无限重试，直到成功/取消/释放

export interface NormalRetryPolicyConfig {
  mode: 'normal'
  maxRetries?: number        // 默认 5
  retryableCodes?: string[]  // 默认 [EMPTY_RESPONSE, RATE_LIMIT, SERVER, TIMEOUT, TRANSPORT]
  backoff?: BackoffConfig    // 指数退避 + 对称抖动
}

// 有界指数退避 + 对称抖动
export interface BackoffConfig {
  initialDelayMs?: number   // 默认 500
  maxDelayMs?: number       // 默认 10000
  jitterRatio?: number      // 默认 0.1
}
```

**关键设计**：
- **Provider 自有策略**：每个 Provider 路由拥有独立的重试策略
- **指数退避**：`initialDelayMs * 2^retry`，上限 `maxDelayMs`
- **对称抖动**：`1 - jitterRatio + 2 * jitterRatio * random()`
- **Provider Retry-After**：优先使用 Provider 返回的延迟

### 4.4 错误分类

**文件**：`packages/llm/llm/src/error.ts`

```typescript
// Provider 中立错误码
export const CONTEXT_WINDOW_EXCEEDED_CODE = 'CONTEXT_WINDOW_EXCEEDED'
export const QUOTA_EXCEEDED_CODE = 'QUOTA'
export const EMPTY_RESPONSE_CODE = 'EMPTY_RESPONSE'
export const INVALID_CREDENTIAL_CODE = 'INVALID_CREDENTIAL'

// 上下文溢出检测（多模式匹配）
export function isContextWindowExceededError(detail: string): boolean {
  return STRUCTURED_CONTEXT_OVERFLOW.test(detail)
    || TOO_LARGE_FOR_CONTEXT.test(detail)
    || EXCEEDS_MODEL_CONTEXT.test(detail)
}

// 错误链渲染（递归渲染 cause 链 + AggregateError）
export function errorChain(value: unknown): string { ... }
```

**错误分类能力**：
- **上下文溢出检测**：多正则模式匹配 Provider 错误描述
- **配额检测**：区分瞬态速率限制与终端配额耗尽
- **错误链**：递归渲染 cause 链，处理 AggregateError

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1179 | 网关路由 | 无适配器注册表（动态注册/注销 + waterfall 拦截） | P0 |
| L1180 | 网关路由 | 无多 Provider 适配（pi-ai 中间层 + 协议表） | P0 |
| L1181 | 网关路由 | 无 Provider 自有重试策略（normal/always 双模式） | P1 |
| L1182 | 网关路由 | 无错误分类（上下文溢出/配额/空响应检测） | P1 |
| L1183 | 网关路由 | 无空闲看门狗（流式调用超时检测） | P1 |


---

## 五、Pregel图执行（Pregel Graph Execution）

### 5.1 Workflow Worker Thread 协议

**文件**：`packages/workflow/workflow-worker-thread/src/protocol.ts`

deepseek-harness 的 Workflow 实现了**类 Pregel 的图执行模型**，通过 Worker 线程隔离执行：

```typescript
// Worker → Host 消息（8 种）
export enum WorkerToHostType {
  Ready = 'ready',           // 启动握手
  Phase = 'phase',           // 阶段变更
  Log = 'log',               // 日志输出
  AgentStart = 'agent-start', // Agent 调用开始
  AgentEnd = 'agent-end',     // Agent 调用结束
  ChildStart = 'child-start', // 子 Agent RPC
  ChildDispose = 'child-dispose', // 子 Agent 释放
  Result = 'result',         // 最终结果
}

// Host → Worker 消息（7 种）
export enum HostToWorkerType {
  Go = 'go',                 // 启动执行
  Cancel = 'cancel',         // 取消运行
  ChildStarted = 'child-started',     // 子 Agent 启动成功
  ChildStartError = 'child-start-error', // 子 Agent 启动失败
  ChildSettled = 'child-settled',     // 子 Agent 结果已解决
  ChildFailed = 'child-failed',       // 子 Agent 失败
  ChildDisposed = 'child-disposed',   // 子 Agent 已释放
}
```

**Pregel 模型映射**：
- **Superstep**：每个 `agent()` 调用对应一个 Superstep
- **Message Passing**：`ChildStart`/`ChildSettled` 对应消息传递
- **Barrier Synchronization**：`Result` 消息对应全局同步点
- **Vertex Activation**：`AgentStart`/`AgentEnd` 对应顶点激活/停用

### 5.2 Workflow Engine（工作流引擎）

**文件**：`packages/workflow/workflow/src/index.ts`

```typescript
export abstract class WorkflowEngine extends Service {
  abstract start(request: WorkflowStartRequest): WorkflowRun

  protected emitWorkflowEvent(name: WorkflowEventName, ...args: unknown[]): void {
    for (const callback of this.ctx.events.dispatch('emit', [name, ...args])) {
      try {
        const returned = (callback as (...payload: unknown[]) => unknown)(...args)
        void Promise.resolve(returned).catch((error) => {
          this.ctx.logger.warn(`workflow: ${name} listener rejected`)
        })
      } catch (error) {
        this.ctx.logger.warn(`workflow: ${name} listener threw`)
      }
    }
  }
}
```

**关键设计**：
- **脚本解析**：VM 编译脚本，`lineOffset` 补偿包装行号
- **取消纪律**：取消后每个 hook 调用都抛 CANCELLED
- **错误分类**：`WorkflowError.fatal` 区分可恢复与不可恢复错误

### 5.3 Ralph 固定脚本

**文件**：`packages/workflow/tool-ralph/src/index.ts`

```javascript
// 部署拥有的固定脚本，模型只供应数据
for (let round = 1; round <= args.maxRounds; round += 1) {
  const rawReport = await agent(prompt, { label: 'Ralph round ' + round, schema: reportSchema })
  if (report.status === 'complete') return { status: 'complete', roundsStarted: round, report }
  if (report.status === 'blocked') return { status: 'blocked', roundsStarted: round, report }
  previous = report
}
```

**Pregel 迭代模式**：
- **Fresh Agent**：每轮全新子 agent，`requireFreshProvider` 强制 `inheritsParentContext=false`
- **结构化传递**：轮次间只传递有界报告（`maxHandoffChars=16384`）
- **不可变目标**：每轮都传递原始 objective

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1184 | 图执行 | 无 Worker 线程协议（15 种消息类型） | P1 |
| L1185 | 图执行 | 无类 Pregel 迭代（Superstep + Message Passing） | P1 |
| L1186 | 图执行 | 无脚本取消纪律（Hook 边界取消） | P2 |
| L1187 | 图执行 | 无 Ralph 固定脚本（Fresh Agent 循环） | P2 |

---

## 六、Skill生命周期（Skill Lifecycle）

### 6.1 Skill Registry（注册中心）

**文件**：`packages/skill/skill/src/index.ts`

```typescript
export class SkillRegistry extends Service {
  // 注册 Provider
  registerProvider(factory: (control: SkillProviderControl) => SkillProvider): () => void {
    const provider = factory(control)
    this.providers.set(provider.name, provider)
    return () => {
      if (this.providers.get(provider.name) === provider) {
        this.providers.delete(provider.name)
      }
    }
  }

  // 注册运行时 Skill
  register(skill: SkillRegistration): () => void {
    validateRuntimeSkill(skill)
    return () => { /* 移除候选 */ }
  }

  // 列出所有 Skill（按 rank 排序，去重）
  list(options?: SkillViewOptions): SkillSummary[] { ... }

  // 加载 Skill 定义
  get(name: string, options?: SkillLookupOptions): Promise<SkillDefinition> { ... }
}
```

**生命周期阶段**：
1. **注册**：`registerProvider()` / `register()`
2. **发现**：`list()` 合并所有 Provider 的候选
3. **加载**：`get()` 调用 `provider.get()` 加载定义
4. **卸载**：返回的 disposer 移除注册

### 6.2 Filesystem Skill Provider（文件系统提供者）

**文件**：`packages/skill/skill-filesystem/src/index.ts`

```typescript
export class FileSystemSkillProvider implements SkillProvider {
  // 扫描多个根目录（按 rank 排序）
  private roots: SkillRoot[] = [
    { path: '.dsh/skills', source: 'project-dsh', rank: 100 },
    { path: '.agents/skills', source: 'project-agents', rank: 200 },
    { path: 'custom', source: 'custom', rank: 300 },
    { path: '~/.dsh/skills', source: 'user-dsh', rank: 400 },
    { path: '~/.agents/skills', source: 'user-agents', rank: 500 },
  ]

  // 解析 YAML frontmatter
  private parseFrontmatter(raw: string): { data: Record<string, unknown>; body: string } | undefined {
    const firstLine = raw.slice(0, firstLineEnd).replace(/\r$/, '')
    if (firstLine !== '---') return undefined
    const parsed = parseYaml(yaml)
    return { data: parsed, body: raw.slice(closing.bodyStart) }
  }

  // 监听文件变更（Chokidar + 防抖）
  private watch(): void { ... }
}
```

**关键设计**：
- **多级根目录**：project-dsh → project-agents → custom → user-dsh → user-agents
- **Rank 排序**：rank 低的优先（project-dsh=100 > user-agents=500）
- **Frontmatter 解析**：YAML 元数据 + Markdown 正文
- **文件监听**：Chokidar 监听变更，防抖后刷新目录

### 6.3 Skill 验证与调用策略

```typescript
const SKILL_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/

export interface SkillInvocationPolicy {
  modelInvocable: boolean   // 模型是否可调用
  userInvocable: boolean    // 用户是否可调用
}

function parseInvocationPolicy(data: Record<string, unknown>): SkillInvocationPolicy {
  const disableModelInvocation = frontmatterBoolean(data, 'disable-model-invocation')
  const userInvocable = frontmatterBoolean(data, 'user-invocable')
  return {
    modelInvocable: disableModelInvocation !== true,
    userInvocable: userInvocable !== false,
  }
}
```

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1188 | Skill | 无 Skill 注册中心（Provider 注册 + Rank 排序） | P0 |
| L1189 | Skill | 无 Filesystem Provider（多级根目录 + Frontmatter） | P1 |
| L1190 | Skill | 无 Skill 调用策略（modelInvocable/userInvocable） | P1 |
| L1191 | Skill | 无文件监听（Chokidar 防抖刷新） | P2 |

---

## 七、Agent预热池（Agent Warm-up Pool）

### 7.1 Session Preparation（会话准备）

**文件**：`packages/session/session-persistence/src/preparations.ts`

```typescript
export class SessionPreparations {
  // 准备缓存（LRU）
  private readonly cache = new Map<string, SessionPreparation>()
  private readonly maxSize: number  // DEFAULT_PREPARED_SESSION_CACHE_SIZE = 5

  async prepare(id: SessionId, signal?: AbortSignal): Promise<SessionPreparation> {
    const cached = this.cache.get(id)
    if (cached !== undefined) return cached
    const preparation = await this.loadPreparation(id)
    this.cache.set(id, preparation)
    return preparation
  }
}
```

**预热机制**：
- **LRU 缓存**：默认保留 5 个准备
- **预加载**：启动时预加载常用会话
- **缓存命中**：避免重复加载

### 7.2 SubAgent 延续管理器

**文件**：`packages/subagent/subagent/src/continuation.ts`

```typescript
export class SubagentContinuationManager {
  private readonly activations = new Map<SessionId, Activation>()

  // 冷恢复（从持久化 Session 恢复驻留 Agent）
  async coldResume(childId: SessionId, options: ColdResumeOptions): Promise<Activation> {
    const preparation = await this.requirePersistence().prepare(childId)
    const agent = await this.recreateAgent(preparation)
    const activation: Activation = {
      childId, parentSession: options.parentSession, handle: agent,
      ancestry: [...options.ancestry, childId],
      ownedChildren: new Set(),
      observer: this.createObserver(childId, options.parent),
      disposal: Promise.resolve(),
    }
    this.activations.set(childId, activation)
    return activation
  }
}
```

**Agent 复用**：
- **冷恢复**：从持久化 Session 恢复 Agent
- **激活图**：跟踪所有活跃的 SubAgent
- **所有权图**：防止循环委托

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1192 | Agent池 | 无 Session 准备缓存（LRU 预热） | P1 |
| L1193 | Agent池 | 无 SubAgent 冷恢复（从持久化重建 Agent） | P1 |
| L1194 | Agent池 | 无激活图（跟踪活跃 SubAgent） | P2 |

---

## 八、Turn锁（Turn Lock）

### 8.1 Agent Inbox（消息队列）

**文件**：`packages/core/agent/src/inbox.ts`

```typescript
export class Inbox {
  // 两个待处理队列
  private readonly state: InboxTarget = { 'next-turn': [], 'next-step': [] }

  // 认领消息（Turn 排他性）
  claim(target: InboxTarget, turn: number): UserMessage[] {
    const claimed = this.mutate('next-step', 0, this.nextStep.length, [], false)
    if (target === 'next-turn') {
      claimed.push(...this.mutate('next-turn', 0, 1, [], false))
    }
    for (const message of claimed) this.notifications.claimed(message, turn)
    return claimed
  }

  // 验证消息 ID 唯一性
  private validate(splice): void {
    const candidate = inbox.toSpliced(splice.start, splice.removedCount ?? 0, ...splice.inserted)
    const ids = new Set<string>()
    for (const message of splice.target === 'next-turn' ? [...candidate, ...this.nextStep] : [...this.nextTurn, ...candidate]) {
      if (ids.has(message.id)) throw new Error(`message "${message.id}" is already pending`)
      ids.add(message.id)
    }
  }
}
```

**Turn 排他性**：
- **双队列**：`next-turn`（Turn 级）+ `next-step`（Step 级）
- **认领机制**：`claim()` 原子性取走消息
- **ID 唯一性**：防止重复消息

### 8.2 Agent Loop（循环驱动）

**文件**：`packages/core/agent-loop/src/agent.ts`

```typescript
export class ReactLoopAgent implements Agent {
  private phase: Phase

  type Phase =
    | { kind: 'idle'; lastTurn: number }
    | { kind: 'maintenance'; abort: AbortController; lastTurn: number; wakeRequested: boolean }
    | { kind: 'running'; abort: AbortController; turn: number; step: number; wakeRequested: boolean }

  send(message: UserMessage, target: InboxTarget, wakeup: boolean): void {
    const wakingAfterAbort = wakeup && this.phase.kind !== 'idle' && this.phase.abort.signal.aborted
    const resolvedTarget = wakingAfterAbort ? 'next-turn' : target
    this.inbox.splice(resolvedTarget, Infinity, 0, [message])
    if (wakeup) this.wakeDriver(wakingAfterAbort)
  }
}
```

**关键设计**：
- **状态机**：idle → running → idle（通过 Phase 转换）
- **唤醒机制**：`wakeDriver()` 唤醒空闲 Agent
- **Abort 感知**：Abort 后唤醒自动转为 `next-turn`

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1195 | Turn锁 | 无 Agent Inbox 双队列（next-turn + next-step） | P0 |
| L1196 | Turn锁 | 无 Turn 排他性（claim 原子性取走） | P0 |
| L1197 | Turn锁 | 无 Agent 状态机（idle/running/maintenance） | P1 |


---

## 九、HTTP客户端高级实现（HTTP Client Advanced）

### 9.1 公共地址解析与连接钉扎

**文件**：`packages/web/web-fetch-http/src/network.ts`

```typescript
// 1. 解析公共地址（拒绝私有地址）
export async function resolvePublicAddresses(hostname, signal, resolver = systemLookup): Promise<PublicAddress[]> {
  const resolved = await raceWithSignal(resolver(unbracketed, { all: true, order: 'verbatim' }), signal)
  for (const entry of resolved) {
    if (!isPublicIpAddress(entry.address)) {
      throw new WebError(`URL hostname resolves to a non-public IP address`, 'WEB_BLOCKED_URL')
    }
  }
  return addresses
}

// 2. 连接钉扎（使用已验证的地址集）
export async function requestPinned(url, addresses, headers, signal): Promise<PinnedResponse> {
  const { Agent, fetch } = await import('undici')
  const dispatcher = new Agent({
    autoSelectFamily: true,
    connect: { lookup: createPinnedLookup(addresses) },
  })
  const response = await fetch(url, { method: 'GET', redirect: 'manual', headers, signal, dispatcher })
  return { response, close: async () => { await dispatcher.close() } }
}

// 3. NAT64 检测（RFC 7050）
async function discoverNat64Prefixes(signal, resolver): Promise<Nat64Prefix[]> {
  const discovered = await raceWithSignal(resolver(IPV4ONLY_DISCOVERY_HOST, ...), signal)
  // 提取 RFC 6052 前缀
}
```

**安全特性**：
- **SSRF 防护**：拒绝解析到私有地址的请求
- **连接钉扎**：使用已验证的地址集，防止 DNS 重绑定
- **NAT64 检测**：检测并阻止通过 NAT64 绕过私有地址限制

### 9.2 URL 验证与内容分类

**文件**：`packages/web/web-fetch-http/src/policy.ts`

```typescript
export function validateFetchUrl(input: string): URL {
  if (input.length > WEB_FETCH_MAX_URL_LENGTH) {  // 2048
    throw new WebError(`URL exceeds the maximum length`, 'WEB_INVALID_URL')
  }
  return parseFetchUrl(input)
}

export function isSameOrigin(a: URL, b: URL): boolean {
  return a.protocol === b.protocol && a.hostname === b.hostname && a.port === b.port
}

export function classifyContentType(contentType: string | null): FetchableKind | undefined {
  const mime = (contentType ?? '').replace(/;.*$/s, '').trim().toLowerCase()
  if (mime === 'text/html' || mime === 'application/xhtml+xml') return 'html'
  if (mime.startsWith('text/')) return 'text'
  if (mime === 'application/json' || mime === 'application/xml' || mime.endsWith('+json') || mime.endsWith('+xml')) return 'text'
  return undefined
}
```

**关键设计**：
- **URL 长度限制**：`WEB_FETCH_MAX_URL_LENGTH = 2048`
- **同源策略**：跨域重定向被拒绝
- **内容类型白名单**：仅支持 html/text/json/xml

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1198 | HTTP客户端 | 无 SSRF 防护（公共地址验证 + 连接钉扎） | P0 |
| L1199 | HTTP客户端 | 无 NAT64 检测（RFC 6052 前缀发现） | P1 |
| L1200 | HTTP客户端 | 无 URL 验证（长度限制 + 同源检测） | P1 |
| L1201 | HTTP客户端 | 无内容类型白名单（html/text/json/xml） | P2 |

---

## 十、安全加固（Security Hardening）

### 10.1 Credential Provider（凭证提供者）

**文件**：`packages/credentials/credentials/src/index.ts`

```typescript
export abstract class CredentialProvider extends Service {
  abstract resolve(ref: CredentialRef): Promise<ResolvedCredential | undefined>
  abstract set(ref: CredentialRef, value: string): Promise<void>
  abstract unset(ref: CredentialRef): Promise<void>
  abstract readRecord(key: CredentialKey): Promise<CredentialRecord | undefined>

  // 序列化读修改写（唯一写入路径）
  abstract modifyRecord(
    key: CredentialKey,
    mutate: (current: CredentialRecord | undefined) => Promise<CredentialRecord | undefined>,
  ): Promise<CredentialRecord | undefined>
}
```

**安全设计**：
- **引用与值分离**：配置中只存引用（环境变量名），不存值
- **序列化写入**：`modifyRecord()` 是唯一的写入路径，支持原子更新
- **无枚举**：引用无法枚举，防止凭证泄露

### 10.2 Local Credential Provider（本地凭证提供者）

**文件**：`packages/credentials/credentials-local/src/index.ts`

```typescript
// 分层凭证存储
// inherited process environment      (read-only, wins)
// > $DSH_HOME/.credentials.yaml      (provider-managed, writable)
// > <invocation cwd>/.env            (read-only fallback)
// > $DSH_HOME/.env                   (read-only fallback)

export class LocalCredentialProvider extends CredentialProvider {
  // 文件权限验证
  private async assertOwnerOnly(file: string): Promise<void> {
    const info = await stat(file)
    if (info.mode & GROUP_OTHER_BITS) {  // 0o077
      throw new Error(`credentials file has overly permissive mode`)
    }
  }

  // 原子写入（跨进程锁 + 0o600 权限）
  private async writeDocument(document: CredentialsDocument): Promise<void> {
    await withFileLock(this.spec.filename, async () => {
      await writeFileAtomic(this.spec.filename, content, { mode: 0o600, dirMode: 0o700 })
    }, { waitMs: DOCUMENT_LOCK_WAIT_MS })
  }
}
```

**安全特性**：
- **文件权限**：`0o600`（仅所有者可读写）
- **跨进程锁**：`withFileLock()` 防止并发写入冲突
- **原子写入**：`writeFileAtomic()` 防止写入中断导致损坏
- **热重载**：Chokidar 监听文件变更，防抖后刷新

### 10.3 Timeout Policy（超时策略）

**文件**：`packages/guard/timeout-policy/src/index.ts`

```typescript
export function apply(ctx: Context): void {
  ctx.on('tools/execute', async (exec, next): Promise<ToolExecutionResult> => {
    const timeoutMs = ctx.tools.get(exec.name, exec.agent)?.timeoutMs
    if (timeoutMs === undefined) return next()

    using d = deadline(exec.signal, timeoutMs, TOOL_TIMEOUT)
    const upstream = exec.signal
    exec.signal = d.signal
    try {
      const result = await next()
      if (timeoutOf(d.signal, TOOL_TIMEOUT) !== undefined) {
        return toolTimeoutResult(timeoutMs)
      }
      return result
    } finally {
      exec.signal = upstream
    }
  })
}
```

**关键设计**：
- **工具级超时**：每个工具可声明 `timeoutMs`
- **信号替换**：临时替换 `exec.signal`，执行后恢复
- **超时结果替换**：超时时替换为结构化错误结果

### 10.4 Invariant Registry（不变量注册）

**文件**：`packages/runtime-diagnostics/invariants/src/index.ts`

```typescript
export class InvariantRegistry extends Service {
  register(packageName: string, installer: InvariantInstaller): () => void {
    const registration = ctx.effect(async () => {
      if (!this.selected(packageName)) {
        return () => { registrations.delete(packageName) }
      }
      const child = ctx.plugin(...)
      await child
      return async () => {
        await child.dispose()
        registrations.delete(packageName)
      }
    }, `invariants.register(${JSON.stringify(packageName)})`)
    return registration
  }
}
```

**安全应用**：
- **包级不变量**：每个包可注册运行时安全检查
- **过滤机制**：支持 allowlist/blocklist 选择启用的包
- **失败隔离**：单个包失败不影响其他包

### 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1202 | 安全 | 无凭证引用与值分离（引用存配置，值存安全存储） | P0 |
| L1203 | 安全 | 无文件权限验证（0o600 + 跨进程锁） | P0 |
| L1204 | 安全 | 无工具级超时策略（timeoutMs + 信号替换） | P1 |
| L1205 | 安全 | 无包级不变量注册（运行时安全检查） | P2 |

---

## 十一、其他关键发现

### 11.1 BlockAssembler（块组装器）

**文件**：`packages/llm/llm/src/assembler.ts`

```typescript
export class BlockAssembler {
  push(chunk: StreamChunk): void {
    switch (chunk.type) {
      case 'block-start': // 创建新的 partial
      case 'text-delta':  // 追加文本
      case 'tool-call-delta': // 追加工具调用参数
      case 'block-end':   // 冻结 partial
      case 'usage':       // 记录 token 使用
      case 'finish':      // 记录结束原因
    }
  }

  // 中断安全组装（仅保留已关闭的文本/推理块）
  interruptedBlocks(): ContentBlock[] {
    return this.order
      .map((index) => {
        const partial = this.mustGet(index)
        const type = partial.block?.type ?? partial.blockType
        if (type !== 'text' && type !== 'reasoning') return undefined
        return this.assemble(partial, index)
      })
      .filter((block): block is ContentBlock =>
        (block?.type === 'text' || block?.type === 'reasoning') && block.text.trim() !== '')
  }
}
```

**关键设计**：
- **增量组装**：流式处理，无需等待完整响应
- **中断安全**：中断时仅保留已关闭的文本/推理块
- **Max-tokens 截断**：自动丢弃不完整的工具调用

### 11.2 Agent Event Dispatch（事件分发）

**文件**：`packages/core/agent/src/dispatch.ts`

```typescript
export function agentEvents(ctx, agent, carrier = agentCarrier(agent)): AgentEventDispatch {
  const fused = <K extends AgentSubjectEvent>(payload: PayloadRest<K>): PayloadOf<K> =>
    ({ ...payload, agent } as PayloadOf<K>)
  return {
    emit(name, payload) {
      // 包含式分发：每个监听器独立，失败不影响其他
      const callbacks = ctx.events.dispatch('emit', args)
      for (const callback of callbacks) {
        try {
          const returned = callback(...args)
          void Promise.resolve(returned).catch((error) => {
            ctx.logger.warn(`agent event "${name}" listener rejected`)
          })
        } catch (error) {
          ctx.logger.warn(`agent event "${name}" listener threw`)
        }
      }
    },
    async serial(name, payload) { /* 串行分发 */ },
    waterfall(name, payload, ...rest) { /* 洋葱模型 */ },
  }
}
```

**关键设计**：
- **融合分发**：subject 与 scope carrier 绑定，防止不一致
- **包含式通知**：监听器失败不影响其他监听器
- **三种分发模式**：emit（并行）、serial（串行）、waterfall（洋葱）

### 11.3 Session Write-Behind（写后缓冲）

**文件**：`packages/session/session-persistence/src/write-behind.ts`

```typescript
export class SessionWriteBehind {
  enqueue(event: SessionEvent): void {
    const wasEmpty = this.pending.length === 0
    this.pending.push(structuredClone(event))
    if (this.barrier !== undefined) return
    if (this.automaticPaused) {
      this.automaticPaused = false; this.deadlineExpired = false; this.armTimer()
    } else if (wasEmpty) {
      this.armTimer()
    }
  }

  flush(): Promise<void> {
    if (this.barrier !== undefined) return this.barrier
    this.cancelTimer(); this.deadlineExpired = false; this.automaticPaused = false
    const barrier = Promise.withResolvers<void>()
    this.barrier = barrier.promise
    void this.drainBarrier(barrier.resolve, barrier.reject)
    return barrier.promise
  }
}
```

**关键设计**：
- **批量写入**：`maxDelayMs` 窗口内的事件合并写入
- **屏障同步**：并发刷新加入同一屏障
- **失败重试**：写入失败时保留事件在队列中

---

## 十二、新增 gap 清单（汇总）

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 Fail-closed 检查点策略（模型请求前刷新 Session） | P0 |
| L1167 | 崩溃恢复 | 无 Write-Behind 缓冲（批量写入 + 失败重试） | P0 |
| L1168 | 崩溃恢复 | 无 Torn-tail 修复（截断不完整写入） | P1 |
| L1169 | 崩溃恢复 | 无文件修订身份（dev/ino/size/mtime/ctime 校验） | P1 |
| L1170 | 取证 | 无包级不变量注册机制 | P2 |
| L1171 | 多租户 | 无 Cordis Isolate 隔离作用域 | P0 |
| L1172 | 多租户 | 无 Agent 作用域隔离（独立 Scope + ctx） | P1 |
| L1173 | 多租户 | 无 SubAgent 所有权图（防循环委托） | P1 |
| L1174 | 多租户 | 无匿名用户 ID（harness-home 作用域 UUID） | P2 |
| L1175 | 检索 | 无 FTS5 全文搜索（Session 历史检索） | P0 |
| L1176 | 检索 | 无 Cursor 分页（Base64url 编码游标） | P1 |
| L1177 | 检索 | 无 Snippet 生成（匹配高亮上下文） | P1 |
| L1178 | 检索 | 无 Live-preferred 查询（内存优先，持久化后备） | P1 |
| L1179 | 网关路由 | 无适配器注册表（动态注册/注销 + waterfall 拦截） | P0 |
| L1180 | 网关路由 | 无多 Provider 适配（pi-ai 中间层 + 协议表） | P0 |
| L1181 | 网关路由 | 无 Provider 自有重试策略（normal/always 双模式） | P1 |
| L1182 | 网关路由 | 无错误分类（上下文溢出/配额/空响应检测） | P1 |
| L1183 | 网关路由 | 无空闲看门狗（流式调用超时检测） | P1 |
| L1184 | 图执行 | 无 Worker 线程协议（15 种消息类型） | P1 |
| L1185 | 图执行 | 无类 Pregel 迭代（Superstep + Message Passing） | P1 |
| L1186 | 图执行 | 无脚本取消纪律（Hook 边界取消） | P2 |
| L1187 | 图执行 | 无 Ralph 固定脚本（Fresh Agent 循环） | P2 |
| L1188 | Skill | 无 Skill 注册中心（Provider 注册 + Rank 排序） | P0 |
| L1189 | Skill | 无 Filesystem Provider（多级根目录 + Frontmatter） | P1 |
| L1190 | Skill | 无 Skill 调用策略（modelInvocable/userInvocable） | P1 |
| L1191 | Skill | 无文件监听（Chokidar 防抖刷新） | P2 |
| L1192 | Agent池 | 无 Session 准备缓存（LRU 预热） | P1 |
| L1193 | Agent池 | 无 SubAgent 冷恢复（从持久化重建 Agent） | P1 |
| L1194 | Agent池 | 无激活图（跟踪活跃 SubAgent） | P2 |
| L1195 | Turn锁 | 无 Agent Inbox 双队列（next-turn + next-step） | P0 |
| L1196 | Turn锁 | 无 Turn 排他性（claim 原子性取走） | P0 |
| L1197 | Turn锁 | 无 Agent 状态机（idle/running/maintenance） | P1 |
| L1198 | HTTP客户端 | 无 SSRF 防护（公共地址验证 + 连接钉扎） | P0 |
| L1199 | HTTP客户端 | 无 NAT64 检测（RFC 6052 前缀发现） | P1 |
| L1200 | HTTP客户端 | 无 URL 验证（长度限制 + 同源检测） | P1 |
| L1201 | HTTP客户端 | 无内容类型白名单（html/text/json/xml） | P2 |
| L1202 | 安全 | 无凭证引用与值分离（引用存配置，值存安全存储） | P0 |
| L1203 | 安全 | 无文件权限验证（0o600 + 跨进程锁） | P0 |
| L1204 | 安全 | 无工具级超时策略（timeoutMs + 信号替换） | P1 |
| L1205 | 安全 | 无包级不变量注册（运行时安全检查） | P2 |

---

## 十三、架构图

### 13.1 崩溃恢复架构

```
┌─────────────────────────────────────────────────────────────┐
│                    Agent Loop Driver                         │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐ │
│  │ llm/stream  │  │tools/execute│  │  agent/pre-step     │ │
│  │  checkpoint │  │  checkpoint │  │    checkpoint       │ │
│  └──────┬──────┘  └──────┬──────┘  └──────────┬──────────┘ │
│         └────────────────┴────────────────────┘            │
│                          │                                  │
│                    ┌─────▼─────┐                           │
│                    │  Session   │                           │
│                    │  flush()   │                           │
│                    └─────┬─────┘                           │
│              ┌───────────▼───────────┐                     │
│              │  Write-Behind Buffer   │                     │
│              │  (batch + retry)       │                     │
│              └───────────┬───────────┘                     │
│         ┌────────────────┼────────────────┐                │
│  ┌──────▼──────┐  ┌──────▼──────┐  ┌──────▼──────┐       │
│  │ JSONL       │  │ SQLite      │  │ Custom      │       │
│  │ Backend     │  │ Backend     │  │ Backend     │       │
│  └─────────────┘  └─────────────┘  └─────────────┘       │
└─────────────────────────────────────────────────────────────┘
```

### 13.2 LLM 网关路由架构

```
┌─────────────────────────────────────────────────────────────┐
│                      LlmRuntime                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              Adapter Registry                        │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐             │   │
│  │  │ pi-ai   │  │ deepseek│  │ custom  │             │   │
│  │  │ adapter │  │ adapter │  │ adapter │             │   │
│  │  └────┬────┘  └────┬────┘  └────┬────┘             │   │
│  │  ┌────▼────────────▼────────────▼────┐             │   │
│  │  │     Retry Policy (per route)      │             │   │
│  │  │  normal: maxRetries=5             │             │   │
│  │  │  always: infinite                 │             │   │
│  │  └───────────────────────────────────┘             │   │
│  └─────────────────────────────────────────────────────┘   │
│                    ┌───────────┐                           │
│                    │  Block     │                           │
│                    │  Assembler │                           │
│                    └───────────┘                           │
└─────────────────────────────────────────────────────────────┘
```

### 13.3 Skill 生命周期架构

```
┌─────────────────────────────────────────────────────────────┐
│                     Skill Registry                           │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              Provider Registry                        │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐             │   │
│  │  │filesystem│  │bundled  │  │ custom  │             │   │
│  │  │provider  │  │provider │  │provider │             │   │
│  │  └────┬────┘  └────┬────┘  └────┬────┘             │   │
│  │  ┌────▼────────────▼────────────▼────┐             │   │
│  │  │     Candidate Merge + Rank        │             │   │
│  │  │  project-dsh(100) > user-agents(500)            │   │
│  │  └───────────────────────────────────┘             │   │
│  └─────────────────────────────────────────────────────┘   │
│                    ┌───────────┐                           │
│                    │  Skill     │                           │
│                    │  Definition│                           │
│                    └───────────┘                           │
└─────────────────────────────────────────────────────────────┘
```

---

**报告完成日期**：2026-09-09
**分析基础**：deepseek-harness packages/ 下 51 个包逐行分析
**总行数**：~4,500+ 行（含代码片段）
