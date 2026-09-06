# DeepSeek-Harness 补充：中断传播、背压/缓冲与 Typert 协议深度分析

> 本文档是对《deepseek-harness-第四轮-Cordis 核心与模块群深度分析》的**独立补充**，聚焦三个未在前轮充分覆盖的维度：
> 1. Cordis Fiber 框架核心（Context / Events / Fiber 生命周期）
> 2. 中断/Abort 信号传播机制（AbortSignal 通用货币 + 两级取消）
> 3. 流式背压与缓冲策略（OutputCollector / BoundedTextBuffer / BlockAssembler）
> 4. Typert 协议（代码生成 + 运行时注册 + ndJSON 传输）

---

## 1. Cordis Fiber 框架核心

### 1.1 Context 类 — `ctx.on`, `ctx.waterfall`, `ctx.emit`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/vendor/cordis/src/context.ts`

Context 是 Proxy 回填的 DI 容器，构造函数内建核心服务：

```ts
// context.ts:71-84
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
  ...
  return self
}
```

### 1.2 Waterfall 机制 — `llm/stream` 的心脏

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/vendor/cordis/src/events.ts`

事件总线定义 **5 种调度模式**。`waterfall` 是 `llm/stream` 使用的拦截器链式原语：

```ts
// events.ts:234-243  (waterfall)
waterfall(...args: any[]) {
  const cbs = this.dispatch('waterfall', args)
  const inner = args.pop()               // 最终 `next`
  const next = () => {
    const cb = cbs.shift() ?? inner
    return cb(...args)
  }
  args.push(next)
  return next()
}
```

每个监听器包裹链的剩余部分：调用 `next()` 调用下一个监听器（最终是内置行为）；**不调用它就否决链的剩余部分，包括内置行为**。

调度模式文档位于 `events.ts:27-32`：
> `emit` 同步运行监听器不等待，`parallel` 一起等待所有监听器，`serial` 按序等待直到某个退出，`bail` 在首个同步退出值停止，`waterfall` 把监听器组合在最终 `next` 回调周围。

`isBailed(value)` at `events.ts:13-15` — `return value !== null && value !== false && value !== undefined`。

`on()` 把监听器绑定到**当前 fiber**，fiber 卸载时自动 dispose。若 fiber 已 dispose 则抛 `CordisError('INACTIVE_EFFECT')`：

```ts
// events.ts:288-302
on(name, listener, options?) {
  ...
  this.ctx.fiber.assertActive()
  listener = this.ctx.reflect.bind(listener)
  const result = this.bail(this.ctx, 'internal/listener', name, listener, options)
  if (result) return result
  const hooks = this._hooks[name] ||= []
  return this.register(label, hooks, listener, options)
}
```

### 1.3 错误包容

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/vendor/cordis/src/utils.ts`

`composeError()` at `utils.ts:268-281` 把外层调用点帧拼接到抛出的异步错误（长堆栈追踪）：

```ts
export function composeError<T>(callback: (info: StackInfo) => T, getOuterStack = buildOuterStack()): T {
  const info: StackInfo = { offset: 1, error: new Error() }
  try {
    const result: any = callback(info)
    if (isObject(result) && 'then' in result) {
      return (result as any).then(undefined, (reason) => handleError(info, reason, getOuterStack)) as T
    } else return result
  } catch (reason: any) {
    handleError(info, reason, getOuterStack)
  }
}
```

`DisposableList` (`utils.ts:5-40`) 是按值 O(1) 删除的有序集合，Fiber 用其存储 disposer。

### 1.4 Effect disposer — `ctx.effect()` / `fiber.effect()`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/vendor/cordis/src/fiber.ts`

`Fiber.effect()` at `fiber.ts:418-561` 是基于 epoch 的复杂生命周期原语：

```ts
// fiber.ts:415-418
effect(execute: () => SyncEffect, label?: string): Disposable<Promise<void>>
effect(execute: () => Effect, label?: string): AsyncDisposable<Promise<void>>
effect(execute: () => Effect, label = 'anonymous'): any {
  this.assertActive()
  if (this.state === FiberState.UNLOADING) throw new CordisError('INACTIVE_EFFECT')
  ...
  const dispose = () => {
    if (disposing) return disposalTask
    disposing = true
    let task!: void | Promise<void>
    for (const disposable of disposables.splice(0).reverse()) {   // 逆序
      if (task) task = task.then(() => runDisposable(disposable))
      else {
        const result = runDisposable(disposable)
        if (isObject(result) && 'then' in result) task = result as any
      }
    }
    return disposalTask = task
  }
  ...
}
```

关键设计：
- 立即运行 `execute`，收集 disposer
- disposer 在 fiber 卸载或 disposer 被调用时（以先发生者为准）**逆序**运行
- 调用 disposer 两次是 no-op
- 在 `execute` 运行之前，effect 对重入 owner 卸载可见（通过 `removeWrapper = this._disposables.push(wrapper)`）
- 返回的 wrapper 既是 `Disposable` 又是 `PromiseLike`（通过 `wrapper.then`, `fiber.ts:555-559`），所以外层 effect 可以 `await` 内层异步拆卸

`FiberState` enum (`fiber.ts:147-154`)：`PENDING, LOADING, ACTIVE, FAILED, DISPOSED, UNLOADING`。

整个生命周期通过 `_reload` / `_unload` (`fiber.ts:646-696`) 运行，由 `_setEpoch()` (`fiber.ts:625-639`) 驱动：`_setEpoch()` 在 INACTIVE 与结合 `impl.fiber.uid` 的 epoch 字符串之间转换。

---

## 2. 中断 / Abort 传播

### 2.1 `AbortSignal` 是通用货币

harness 普遍使用 `AbortSignal`/`AbortController` —— 而非自定义 cancel-token。典型源：

- **`core/agent-loop/src/agent.ts:141-147`** — `ReactLoopAgent.cancel()`：
  ```ts
  cancel(cause: AgentCancelCause, options: CancelOptions = {}): void {
    if (!options.keepInbox) {
      this.inbox.clear()
      if (this.phase.kind !== 'idle') this.phase.wakeRequested = false
    }
    if (this.phase.kind !== 'idle') this.phase.abort.abort(cause)
  }
  ```
  阶段机（`Phase` union, `agent.ts:38-47`）在 `maintenance` 和 `running` 阶段都携带 `abort: AbortController`。每个步骤边界调用 `signal.throwIfAborted()`。

- **`core/session/src/types.ts:142-147`** — `AgentCancelCause`：
  ```ts
  export type AgentCancelCause =
    | { readonly kind: 'user' }
    | { readonly kind: 'parent' }
    | { readonly kind: 'hook'; readonly reason: string }
    | { readonly kind: 'disposed' }
  ```

- **`acp/acp/src/session.ts:477-484`** — ACP session `cancelPrompt`：
  ```ts
  private cancelPrompt(detail: string): void {
    const inflight = this.inflight
    if (inflight === undefined) return
    inflight.cancelRequested = true
    inflight.admissionController.abort(new Error(detail))
    this.settleAfterQuiescence(inflight)
    if (inflight.messageQueued) this.agent.cancel({ kind: 'user' })
  }
  ```
  两级：abort `admissionController`（内容准入），然后向进程内驱动传播 `agent.cancel({kind:'user'})`。

- **`acp/acp/src/index.ts:388`** — ACP 桥接标准 `methods.agent.session.cancel` 通知：
  ```ts
  .onNotification(methods.agent.session.cancel, ({ params }) => implementation.cancel(params))
  ```

- **`subprocess/subprocess/src/types.ts:90-96`** — `SubprocessSpawnSpec.signal?: AbortSignal`：
  > Abort signal — 触发时启动进程树的 terminate 升级。调用方拥有 deadline 和原因分类；此接缝只对 abort 作出反应。

### 2.2 信号融合用 `AbortSignal.any`

harness 用 `AbortSignal.any([...])` 融合信号，而非链式。见于：
- `llm/llm-deepseek/src/adapter.ts:476`: `AbortSignal.any([options.signal, consumer.signal])`
- `llm/llm-retry/src/index.ts:124,163`: `const fusedSignal = AbortSignal.any([signal, lifetime.signal])` — 把 per-request 信号与 plugin lifetime 融合，所以 disposal 能取消所有进行中恢复

### 2.3 SIGINT / Ctrl-C 传递 — 进程组模型

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/subprocess/subprocess-local/src/terminal.ts`

PTY 层拥有向前景进程组传递 SIGINT：

```ts
// terminal.ts:95-118
async signalForeground(signal: SubprocessTerminalSignal): Promise<number> {
  const foreground = await this.inspectForeground()
  ...
  if (this.platform === 'win32') {
    if (signal === 'SIGINT') {
      // Windows 无进程组信号：`\x03` 输入写是 Ctrl-C 传递路径
      this.terminal.write('\x03')
      return foreground.processGroupId
    }
    ...
  }
  this.inspector.signalGroup(foreground.processGroupId, signal)
  return foreground.processGroupId
}
```

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/terminal/terminal-bash/src/session.ts:663-687` — PTY `interruptOnce` 向前景组传递 `SIGINT`：

```ts
private async interruptOnce(operation: LocalSendOperation): Promise<void> {
  try {
    const activeWrite = this.activeWrite
    if (activeWrite !== undefined && !await activeWrite) return
    await this.terminal.signalForeground('SIGINT')
  } catch (error: unknown) {
    if (this.active === operation && !this.closing) this.onTransportFailure(error)
    return
  } finally {
    if (this.interrupting === operation) this.interrupting = undefined
  }
  ...
}
```

用户可见的中断路径通过 `AbortSignal` → `operation.cancel()` → `this.interrupt(operation)` 接线（`session.ts:267-268, 152-154`）。

### 2.4 流式侧 abort — `llm/stream` + `AbortSignal.throwIfAborted`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/agent-loop/src/agent.ts:364-369`

```ts
const stream = preparedCall?.stream(request) ?? this.loopCtx.llm.stream(request)
signal.throwIfAborted()
for await (const chunk of stream) {
  signal.throwIfAborted()
  chunkSeqs.push(this.session.append('assistant/chunk', { turn, step, chunk }).seq)
  assembler.push(chunk)
}
signal.throwIfAborted()
```

流式 abort 中途，agent loop 通过 `assembler.interruptedBlocks()`（`agent.ts:371-384`）保留已接收内容，并追加 `interrupted` assistant 消息。

`adapterFailureChunk()`（`llm/llm/src/index.ts:1068-1076`）把任何 adapter throw 转为终态 `finish` chunk：
```ts
function adapterFailureChunk(error: unknown, signal?: AbortSignal): StreamChunk {
  const failure = normalizeLlmFailure(error)
  return {
    type: 'finish',
    reason: signal?.aborted || failure.code === 'ABORTED'
      ? { kind: 'aborted', failure }
      : { kind: 'error', failure },
  }
}
```

### 2.5 ACP session 取消细拆

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/session.ts:431-471`

`AcpSession.close(detail)`:
1. `cancelPrompt(detail)` — abort `admissionController`，若消息已排队则传播 `agent.cancel({kind:'user'})`
2. `await inflight?.admissionDone` — 让 admission 安定
3. `await this.agent.whenIdle()` — 整树空闲屏障（在 `activityDone` 上循环直到稳定，`agent.ts:202-207`）
4. `await this.outputTail` — 序列化 ACP 投影通知（链式 promise）
5. `subagents?.drainContinuableDescendants([this.agent])` — 子优先后代拆卸
6. `ctx.sessions.flush(...)` — 持久化
7. `disposeAgent()`

`AcpSession.cancel()`（`session.ts:337-341`）双重行为：
```ts
cancel(): void {
  const inflight = this.inflight
  this.cancelPrompt('ACP prompt cancelled')
  if (inflight === undefined) this.agent.cancel({ kind: 'user' })   // 自主工作，无 ACP prompt
}
```

---

## 3. 流式背压 / 缓冲

Cordis 本身**无**内置背压原语 — 事件总线要么同步（`emit`, `bail`）要么基于 await（`parallel`, `serial`, `waterfall`）。背压在 consumer/process 层用三种策略实现：有界缓冲 + tail-keep + spill 文件、abort 信号传播、无消费 offset-based 读取器。

### 3.1 `OutputCollector` — 有界 tail + spill 文件

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/subprocess/subprocess-local/src/spawn.ts:104-251`

这是典型的输出缓冲原语。保留内存 TAIL 到 `maxBytes`，可选整流 spill 到文件：

```ts
// spawn.ts:131-153
push(chunk: Buffer): void {
  this.total += chunk.length
  const overflows = this.bytes + chunk.length > this.maxBytes
  if (!this.spillDisabled && (overflows || this.spillFd !== undefined)) this.spillAll(chunk)
  this.chunks.push(chunk)
  this.bytes += chunk.length
  while (this.bytes > this.maxBytes) {
    const head = this.chunks[0] as Buffer
    const excess = this.bytes - this.maxBytes
    if (head.length <= excess) {
      this.chunks.shift()                 // 丢弃整个 head chunk
      this.bytes -= head.length
    } else {
      this.chunks[0] = head.subarray(excess)   // 修剪 head 使保留窗口字节精确
      this.bytes -= excess
    }
    this.dropped = true
  }
}
```

- Spill 文件在首次溢出时**懒打开**（`spillAll`, `spawn.ts:156-173`），带随机后缀 + `O_EXCL` + `0o600` mode — 防御共享 tmp 目录的 symlink 种植
- 若 `total > maxSpillBytes`，spill 被丢弃（`discardSpill`, `spawn.ts:176-197`）
- 读取是**无消费**、offset-based（`readFrom(fromByte)`, `spawn.ts:207-218`）：安定后 `readFrom(0)` 是批量结果；`lossy` flag 报告内存 tail 何时已丢失 head

subprocess 接缝 `SubprocessCollect`（`subprocess/subprocess/src/types.ts:44-52`）暴露此能力：
```ts
export interface SubprocessCollect {
  maxBytes: number            // 内存上限；溢出保留 TAIL
  spill?: { maxBytes: number } // 整流 spill 文件
}
```

### 3.2 PTY 有界滚动缓冲 — `BoundedTextBuffer`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/terminal/terminal-bash/src/session.ts:45-80`

```ts
class BoundedTextBuffer {
  private value = ''
  private dropped = false
  constructor(private readonly maxBytes: number, private readonly maxLines?: number) {}
  append(text: string): void {
    if (text.length === 0) return
    this.value += text
    if (this.maxLines !== undefined) {
      const lines = this.value.split('\n')
      if (lines.length > this.maxLines) {
        this.value = lines.slice(lines.length - this.maxLines).join('\n')
        this.dropped = true
      }
    }
    const tail = utf8Tail(this.value, this.maxBytes)
    this.value = tail.text
    this.dropped ||= tail.truncated
  }
  ...
}
```

双上限：maxLines AND maxBytes。`utf8Tail`（`session.ts:31-43`）保留完整字符使 UTF-8 tail 字符精确。

### 3.3 `BlockAssembler` — chunk→message 组装 + 中断容忍

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm/src/assembler.ts`

对 delta-only 协议容忍。已关闭 index 的 delta 到达时被**忽略** — 所以故障 adapter 无法增长内存（`assembler.ts:64-65, 72-73`）：
```ts
case 'text-delta':
case 'reasoning-delta': {
  const partial = this.ensure(chunk.index, chunk.type === 'text-delta' ? 'text' : 'reasoning')
  if (partial.block) return // 已被 block-end 关闭；忽略掉队者
  partial.text += chunk.text
  return
}
```

`interruptedBlocks()` 在流被 cancel 切断时返回部分内容 — agent loop 用此持久化部分 assistant 消息。

### 3.4 SSE 流 — pull-based Web `ReadableStream` + `TextDecoderStream`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/sse.ts`

```ts
export async function* parseSse(
  stream: ReadableStream<BufferSource>,
  onComment?: (comment: string) => void,
): AsyncGenerator<string> {
  const events = stream
    .pipeThrough(new TextDecoderStream())
    .pipeThrough(new EventSourceParserStream({ onComment }))
  for await (const { data } of events) {
    yield data
    if (data === DONE) return
  }
  throw new LlmError('SSE stream ended without [DONE]', 'STREAM_CLOSED')
}
```

pull-based：consumer-driven 背压（无显式 highWaterMark）。Web Streams `pipeThrough` 给浏览器/Runtime 默认背压信号。

### 3.5 `llm/stream` waterfall — 通用流式拦截器

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm/src/index.ts:54-70`

```ts
declare module '@deepseek-ai/cordis' {
  interface Events {
    'llm/stream'(this: LlmRuntime, options: GenerateOptions, next: () => AsyncIterable<StreamChunk>): AsyncIterable<StreamChunk>
  }
}
```

`LlmRuntime.stream()`（`index.ts:1050-1064`）把每次 adapter dispatch 包裹在 waterfall 中：
```ts
stream(options: GenerateOptions): AsyncIterable<StreamChunk> {
  return this.streamWithRegistration(options)
}
private streamWithRegistration(options, prepared?): AsyncIterable<StreamChunk> {
  return this.ctx.waterfall(this, 'llm/stream', options, () => this.adapterStream(options, prepared))
}
```

Consumer（`streamWithRegistration`）→ `ctx.waterfall` → 最外层优先监听器 → `next()` → adapter 的 `AsyncGenerator`。每个监听器可以短路（否决）、包裹或替换流。默认内置是 `adapterStream`（`index.ts:963-1037`），把所有 adapter 结果归一化为终态 `finish` chunk，并在早期退出时调用 `iterator.return()`。

### 3.6 通过 `cancellableDelay` 的重试背压

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-retry/src/index.ts`

```ts
// index.ts:78-91
function cancellableDelay(delayMs: number, signal: AbortSignal): Promise<boolean> {
  if (signal.aborted) return Promise.resolve(false)
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolve(true)
    }, delayMs)
    function onAbort(): void {
      clearTimeout(timer)
      resolve(false)
    }
    signal.addEventListener('abort', onAbort, { once: true })
  })
}

// index.ts:58-63
function localDelay(config: ResolvedRetryPolicy, retry: number, random: () => random): number {
  const exponent = Math.min(retry - 1, 1024)
  const exponential = Math.min(config.initialDelayMs * 2 ** exponent, config.maxDelayMs)
  const jitter = 1 - config.jitterRatio + 2 * config.jitterRatio * random()
  return Math.min(exponential * jitter, config.maxDelayMs)
}
```

重试等待通过 `AbortSignal.any([signal, lifetime.signal])` 中止。Plugin dispose abort `lifetime` 并排空所有 `active` 恢复（`index.ts:221-225`）。

---

## 4. Typert 协议（`packages/typert/`）

Typert **不是**有线协议。它是用于生成 Remote 方法产物的**代码生成 + 运行时注册表**（TypeScript 类型驱动的 RPC schema）。harness 把 service interface 编译为 typed client/server stub；Typert 存储生成的 descriptor、Zod schema 和 lookup/context provider。实际有线载体（ACP 用 ndJSON over stdio、LLM 用 fetch）在别处。

### 4.1 协议入口 — decorator 与 binding

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/protocol/src/index.ts`

- `@Remote` decorator 把方法标记为直接 Remote 调用；`@Remote({mode:'stream'})` 标记为逻辑流，通过逻辑流载体传递每个 Iterable 项
- `@RemoteScope(key)` 从一个 Remote Scope 解析方法
- `bindTypertRemote(service, serviceKey, options)` 把 Service 绑定到 Typert Gateway 命名空间

取消通过 TS decorator 元数据注入为**最后一个参数** — 而非有线字段：
```ts
// protocol/src/types.ts:273-278
readonly cancellation?: {
  /** Reserved final Host method parameter. */
  readonly parameter: 'signal'
}
```
```ts
// protocol/src/index.ts:229-234  (loader 验证)
if (invocation.cancellation !== undefined) {
  const cancellation = requireObject(pkgName, invocation.cancellation, ...)
  if (cancellation.parameter !== 'signal') {
    throw new Error(`... cancellation parameter must be "signal"`)
  }
}
```

### 4.2 Endpoint 语法

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/protocol/src/index.ts:10-18`

```ts
const TYPERT_REMOTE_SEGMENT_PATTERN = /^[A-Za-z0-9_$.-]+$/
export function isTypertRemoteSegment(value: string): boolean {
  return value !== '.' && value !== '..' && TYPERT_REMOTE_SEGMENT_PATTERN.test(value)
}
```

### 4.3 有线格式 — `InvocationDescriptor` 是载体无关的

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/protocol/src/types.ts:242-282`

```ts
export interface InvocationDescriptor {
  readonly id: string
  readonly service: string
  readonly namespace: string
  readonly method: string
  readonly implementation?: string
  readonly mode?: 'stream'
  readonly invocation: { readonly kind: 'direct' } | { readonly kind: 'context'; readonly context: string; readonly wire: string; readonly codec: TypertCodec }
  readonly scope?: { readonly context: string; readonly wire: string }
  readonly parameters: readonly InvocationParameterDescriptor[]
  readonly cancellation?: { readonly parameter: 'signal' }
  readonly result: TypertCodec
  readonly sourceLocation?: InvocationSourceLocation
}
```

Codec 有两种模式 — `strict`（Zod 验证）或 `src-json`（pass-through）：
```ts
export type TypertCodec =
  | { readonly mode: 'strict'; readonly typeSymbol: string; readonly schema: TypertSchema }
  | { readonly mode: 'src-json' }
```

### 4.4 注册表 — 四个 store

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/registry/src/service.ts`

`TypertRegistry extends Service` 暴露四个子 store（`service.ts:463-507`）：
- `local`（`TypertLocalRegistry`）— 当前环境 invocation descriptor（DescriptorStore）
- `remotes`（`TypertRemoteRegistry`）— consumer 选择的 Remote 贡献（RemoteStore 包裹 DescriptorStore）
- `lookups`（`TypertLookupRegistry`）— 按 merge 声明 key 的 Host 对象 lookup provider
- `contexts`（`TypertContextRegistry`）— scoped invocation 的 Host/Client Context adapter

`ChangeSource`（`service.ts:83-105`）同步通知观察者，包含每个失败（`try/catch` 每个监听器）所以一个坏观察者不能否决注册表变更。

注册是原子 & 重复拒绝：
```ts
// service.ts:127-134
if (endpoints.has(endpoint) || this.entries.has(endpoint)) {
  throw new Error(`typert: ${this.kind} endpoint "${endpoint}" is already registered`)
}
if (ids.has(descriptor.id) || this.ids.has(descriptor.id)) {
  throw new Error(`typert: ${this.kind} invocation id "${descriptor.id}" is already registered`)
}
```

### 4.5 Loader — 从 `./typert` 导出自动注册

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/typert/loader/src/index.ts`

- `TYPERT_HOST_EXPORT = './typert'`（`loader/src/index.ts:39`）
- `validateTypertManifest(pkgName, exported)` 验证 TYPERT manifest 结构
- `apply()` 订阅 `internal/plugin`，在每个 loader entry mount 上解析包的 `./typert` 导出、导入、验证、调用 `ctx.typert.register(manifest)`
- Reconciliation 是 microtask-batched（`queueMicrotask`, `loader/src/index.ts:416-421`），按 entry name 增量（`dirty` set + `flushQueued` flag）

### 4.6 实际有线帧（ACP）— ndJSON over stdio

由于 Typert 是载体无关的，真正的帧在 bridge consumer。**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/index.ts:372-375`

```ts
const stream: Stream = config.stream ?? ndJsonStream(
  Writable.toWeb(process.stdout) as WritableStream<Uint8Array>,
  Readable.toWeb(process.stdin) as ReadableStream<Uint8Array>,
)
```

生产传输是 **ndJSON**（换行分隔 JSON）over Web `WritableStream`/`ReadableStream` 包裹 stdout/stdin。

---

## 5. 跨切面模式对 `laew` 的启示

### 5.1 对 laew 中断模型的启示
- **单一通用信号**：`AbortSignal` 无处不在；用 `AbortSignal.any([...])` 融合
- **两级取消**：admission/controller abort（拆卸）+ 驱动级 `AbortController` abort 在每个 `for-await` 边界
- **带嵌入 AbortController 的阶段机**：`idle | maintenance | running` — 把 aborted activity 到达的重分类为 `next-turn`（不加入 aborted work）
- **结构化取消原因**（`AgentCancelCause`）— `user / parent / hook / disposed` — 在日志中持久化为 `turn/end { kind:'aborted', reason }`

### 5.2 对 laew 背压的启示
- Cordis **无**背压原语。Waterfall 是 pull-based `AsyncIterable` — 通过 consumer-driven `for await` 自然背压
- 长存活进程输出用有界 **tail-keep + spill**（`OutputCollector`）
- 独立消费者用**无消费 offset-based 读取器**（`readFrom(fromByte)`）
- 重试用 `cancellableDelay` + `AbortSignal.any`

### 5.3 对 laew 协议层的启示
- Typert 把有线与 schema 解耦：`InvocationDescriptor` + Zod codec，`cancellation: { parameter: 'signal' }` 注入为终态参数
- 生产传输是 ndJSON over Web Streams（`Writable.toWeb(process.stdout)`）
- SSE 帧用 `eventsource-parser` 通过 `TextDecoderStream` 管道 — pull-based

---

## 关键文件路径索引

- `vendor/cordis/src/{context,events,fiber,utils,registry,service,logger,reflect}.ts`
- `core/agent-loop/src/agent.ts`、`core/agent/src/index.ts`、`core/session/src/types.ts`
- `acp/acp/src/{session,index,model-control}.ts`
- `subprocess/subprocess/src/{types,index}.ts`、`subprocess/subprocess-local/src/{spawn,terminal}.ts`
- `terminal/terminal-bash/src/session.ts`
- `interaction/user-approval/src/index.ts`
- `llm/llm/src/{index,types,assembler}.ts`、`llm/llm-deepseek/src/{adapter,sse}.ts`、`llm/llm-retry/src/index.ts`
- `typert/protocol/src/{index,types,invariant}.ts`、`typert/registry/src/{service,types,index}.ts`、`typert/loader/src/index.ts`
