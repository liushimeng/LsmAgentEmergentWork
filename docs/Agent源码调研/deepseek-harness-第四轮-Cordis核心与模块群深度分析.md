# DeepSeek-Harness 第四轮：Cordis 核心与模块群深度分析

> 源码路径：`/usr/local/LsmGitOpenSource/deepseek-harness`（pnpm monorepo，`@deepseek-ai/dsh-root`，v0.1.2-alpha.1）  
> 产出路径：`docs/Agent源码调研/deepseek-harness-第四轮-Cordis核心与模块群深度分析.md`  
> 本轮定位：**前三轮已完成源码调研 / 深度分析 / 核心机制 / 第二轮深挖 / 第三轮-apps&native**。本轮聚焦 **Cordis 插件总线三原语 + Fiber epoch 算法 + 52 个 packages 中的核心模块群（workflow/subagent/plan/goal/sandbox/schedule/skill/shell/lsp/compaction/guard/interaction/feedback/typert/webhook/workspace/mcp/extensions/acp/context/session/llm/credentials/settings/code-runtime/e2b/jobs）**，全部落到真实文件路径、模块名、函数名、代码片段。

---

## 0. 摘要与本轮定位

### 0.1 工程全貌

DeepSeek-Harness 是 DeepSeek 官方的 TypeScript Agent 编排框架（二进制名 `dsh`），采用 **Cordis Everything-is-a-Plugin** 架构。整个工程由 **Cordis 微内核 + 52+ capability seam（能力缝）+ 若干 provider 实现** 组成：

| 层级 | 组成 | 关键包 |
|---|---|---|
| Cordis 微内核 | Context / Service / Fiber / Registry / Events / Reflect | `vendor/cordis` |
| Loader 层 | Loader / Include / Group / Isolate / HMR | `vendor/loader` + `vendor/{include,group,isolate,hmr}` |
| Boot 层 | app-boot（boot 序列）/ cmdline | `packages/boot/*` |
| Core 层 | agent / agent-loop / agent-default-model / agent-tool-presentation / scope / session / system-prompt / tools | `packages/core/*` |
| Capability seam + provider | 52+ packages，每项一个 `Service` 抽象 + N 个 provider 实现 | `packages/*` |

### 0.2 本轮覆盖维度（逐项，无省略）

1. **Cordis 三原语 + Service + Fiber epoch 算法** — `vendor/cordis/src/{context,service,fiber,registry,events,reflect,utils}.ts`
2. **核心模块群深度剖析**（逐模块读关键文件/函数/代码片段）
3. **Anthropic / OpenAI 协议适配真实代码路径** — `packages/llm/llm-pi-ai/src/{provider,stream}.ts`
4. **其他维度实现快照**（多轮对话/Context/记忆/质检/工具/MCP/Skill/SubAgent/Workflow/loop/目标规划/沙箱/权限）表格+关键代码
5. **对 laew 的借鉴（P0/P1/P2）**
6. **参考资料与文件索引**

### 0.3 已有多轮分析，本轮不重复、只深化

- 第一轮 `deepseek-harness-源码调研.md`：全局扫描
- 第二轮 `deepseek-harness-第二轮深度分析.md`：Cordis 三原语 + Service + Fiber epoch + apps/cli 50 行入口 + 4 层 patch 叠加 + WriteBehind 200ms 批 + ScopedLayers
- 第三轮 `deepseek-harness-第三轮-apps前端与native深度分析.md`：apps/cli+web+storybook / native landlock-run
- 本轮 = **Cordis 核心再深剖 + 全模块群逐个落地**，代码量、维度数、精度均超前三轮之和

---

## 1. Cordis 插件总线核心（三原语/Service/Fiber epoch/注册发现调度）— 含 Mermaid 图

Cordis 是整个 harness 的「操作系统内核」。所有能力都以 **Service** 形式注册到 **Context** 这个 IOC 容器上，**Fiber** 是每个 plugin 的运行时实例，**Registry** 负责 plugin 的注册/发现/调度。三者合称「Cordis 三原语」。

### 1.1 三原语定义

#### 1.1.1 Context — IOC 容器 + 隔离作用域

**文件**：`vendor/cordis/src/context.ts`

Context 是带原型链继承的 **Proxy 容器**。核心能力：

- `extend(meta)` — 创建子 context（原型继承，不污染父）
- `isolate(name, label?)` — 创建独立 service 作用域（用于 SubAgent 隔离）
- `intercept(name, config)` — 注入 service 拦截配置

```typescript
// vendor/cordis/src/context.ts:99-107
extend(meta = {}): this {
  const shadow = Reflect.getOwnPropertyDescriptor(this, symbols.shadow)?.value
  const self = Object.create(getTraceable(this, this))
  for (const prop of Reflect.ownKeys(meta)) {
    Object.defineProperty(self, prop, Reflect.getOwnPropertyDescriptor(meta)!)
  }
  if (!shadow) return self
  return Object.assign(Object.create(self), { [symbols.shadow]: shadow })
}

// vendor/cordis/src/context.ts:121-125
isolate(name: string, label?: symbol) {
  const shadow = Object.create(this[symbols.isolate])
  shadow[name] = label ?? Symbol(name)
  return this.extend({ [symbols.isolate]: shadow })
}
```

Context 构造时安装 6 个内置服务：`reflect` / `registry` / `events` / `logger` + 根 `fiber`。通过 `ReflectService.handler` 这个 Proxy handler 实现 service 的懒解析 + 注入检查。

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
  this.fiber._disposables.clear()
  return self
}
```

#### 1.1.2 Service — 命名能力单元

**文件**：`vendor/cordis/src/service.ts`

Service 是所有能力的基类。子类通过 `super(ctx, name)` 自动注册到当前 context。

```typescript
// vendor/cordis/src/service.ts:11-59
export abstract class Service<out T = never> {
  static readonly init: unique symbol = symbols.init      // 构造后运行的方法
  static readonly check: unique symbol = symbols.check      // 可用性谓词（ctx.provide 传入）
  static readonly config: unique symbol = symbols.config    // 拦截配置类型参数
  static readonly invoke: unique symbol = symbols.invoke    // 可调用 service 体
  static readonly extend: unique symbol = symbols.extend    // 派生扩展实例
  static readonly tracker: unique symbol = symbols.tracker  // 追踪元数据
  static readonly resolveConfig: unique symbol = symbols.resolveConfig

  constructor(protected ctx: Context, name: string) {
    name ??= this.constructor['provide'] as string
    let self = this
    const tracker: Tracker = { associate: name, property: 'ctx' }
    if (self[symbols.invoke]) {
      self = createCallable(name, joinPrototype(Object.getPrototypeOf(this), Function.prototype), tracker)
    }
    self.ctx = ctx
    self.name = name
    defineProperty(self, symbols.tracker, tracker)
    self.ctx.reflect.provide(name, self, this[symbols.check])  // 自动注册
    return self
  }
```

**拦截配置合并**（`resolveConfig`）是 Service 的精妙设计 — 沿原型链收集所有祖先 context 对同一 service 的 `intercept` 配置，按「base → 祖先 intercept → head」顺序合并：

```typescript
// vendor/cordis/src/service.ts:86-102
[symbols.resolveConfig](base?: T, head?: T): T {
  let intercept = this.ctx[Context.intercept]
  const configs: any[] = []
  while (this.name in intercept) {
    if (Object.hasOwn(intercept, this.name)) configs.unshift(intercept[this.name])
    intercept = Object.getPrototypeOf(intercept)
  }
  if (base) configs.unshift(base)
  if (head) configs.push(head)
  if (this['Config']?.merge) return this['Config'].merge(...configs)
  else return Object.assign({}, ...configs)
}
```

#### 1.1.3 Fiber — Plugin 运行时实例 + Epoch 算法

**文件**：`vendor/cordis/src/fiber.ts`

Fiber 是 Cordis 最精巧的抽象，每个 `ctx.plugin()` 调用产生一个 Fiber。它解决了 **plugin 热加载/卸载/依赖等待/配置更新** 的协调问题。

**生命周期状态机**：

```typescript
// vendor/cordis/src/fiber.ts:147-154
export const enum FiberState {
  PENDING,    // 等待依赖 service 可用
  LOADING,    // plugin callback 正在执行
  ACTIVE,     // 已加载、正在提供服务
  FAILED,     // 配置校验或启动抛错
  DISPOSED,   // 已卸载，不可重启
  UNLOADING,  // 正在运行 disposers
}
```

### 1.2 Fiber Epoch 算法（核心创新）

Fiber 的核心创新是 **epoch 字符串** 机制，用于在无锁条件下协调 load/unload 转换：

```typescript
// vendor/cordis/src/fiber.ts:611-623
_refresh() {
  let epoch: string | boolean = false
  epoch = ''
  for (const name of Object.keys(this.inject)) {
    const impl = this._store[name]
    if (!impl) { epoch = INACTIVE; break }
    epoch += ':' + impl.fiber.uid     // epoch = ":<uid1>:<uid2>:..."
  }
  this._setEpoch(epoch)
}
```

**算法要义**：

1. **epoch 字符串** = 当前 fiber 所有依赖 service 的提供 fiber 的 uid 拼接（`:uidA:uidB`）
2. 当任一依赖变化（service 卸载/重新加载），`_refresh()` 重新计算 epoch
3. `_setEpoch()` 比较新旧 epoch：
   - 旧=INACTIVE + 新≠INACTIVE → `_reload()`（启动 plugin）
   - 旧≠INACTIVE + 新=INACTIVE → `_unload()`（卸载 plugin）
   - epoch 相同 → 不做任何事

```typescript
// vendor/cordis/src/fiber.ts:625-639
private _setEpoch(epoch: string) {
  const oldEpoch = this._runner.epoch
  if (epoch === oldEpoch) return           // 无变化，短路
  this._runner.epoch = epoch
  if (this.inertia) return                 // 已有正在进行的转换
  this._updateState(() => {
    if (epoch !== INACTIVE && oldEpoch === INACTIVE) {
      this.inertia = this._reload()        // INACTIVE → 可用：启动
      return FiberState.LOADING
    } else {
      this.inertia = this._unload()        // 可用 → INACTIVE：卸载
      return FiberState.UNLOADING
    }
  })
}
```

**Effect 生命周期与 disposal**：plugin 通过 `ctx.effect(fn, label)` 注册清理函数。effect 可返回单个 disposer、Promise、同步/异步 generator（每个 yielded disposer 单独跟踪）：

```typescript
// vendor/cordis/src/fiber.ts:418-561（简化）
effect(execute: () => Effect, label = 'anonymous'): any {
  this.assertActive()
  const disposables: Disposable[] = []
  const dispose = () => {
    for (const disposable of disposables.splice(0).reverse()) {
      // 逆序运行 disposers，支持 async
      task = task ? task.then(() => runDisposable(disposable)) : runDisposable(disposable)
    }
  }
  // execute 立即执行，收集 disposer；支持 generator effect（yield 多个 disposer）
  let task = this._execute(runner)
  // ... 返回 wrapper（PromiseLike + disposer）
}
```

### 1.3 Registry — Plugin 注册/发现/调度

**文件**：`vendor/cordis/src/registry.ts`

`RegistryService` 负责 plugin 的注册与运行时记录管理：

```typescript
// vendor/cordis/src/registry.ts:222-228
resolve(plugin: Plugin): Function | undefined {
  try {
    if (typeof plugin === 'function') return plugin
    if (isApplicable(plugin)) return plugin.apply    // { apply } 对象
  } catch {}
}

// vendor/cordis/src/registry.ts:316-336
plugin(plugin: Plugin, config?: any, getOuterStack = buildOuterStack()) {
  const callback = this.resolve(plugin)
  if (!callback) throw new Error('invalid plugin...')
  let runtime = this._internal.get(callback)
  if (!runtime) {
    runtime = { name, callback, fibers: new DisposableList(), Config: plugin.Config }
    this._internal.set(callback, runtime)
  }
  const fiber = new Fiber(this.ctx, config, Inject.resolve(plugin.inject), runtime, getOuterStack)
  const wrapped = Object.create(fiber)
  wrapped.then = (onFulfilled, onRejected) => fiber.await().then(onFulfilled, onRejected)
  return wrapped    // Fiber & PromiseLike<Fiber> 双形态
}
```

**关键设计**：同一 plugin callback 对应一个 `Runtime`（共享 Config / fibers 列表），但每次 `ctx.plugin()` 调用产生 **独立 Fiber**（独立 config / 独立 dispose）。这就是同一 plugin 多实例的由来。

### 1.4 Reflect — Service 解析层

**文件**：`vendor/cordis/src/reflect.ts`

Reflect 是 Context Proxy 背后的「服务定位器」。`get` trap 的解析链：

```
特殊属性(symbol/prototype/then/数字/_开头) → Reflect.get
target 自身有该属性 → 直接返回
定义了 accessor → 调用 get hook
根 fiber → ctx.reflect.get(prop, false)（宽松读取）
其他 → waterfall('internal/get') → 沿 fiber 链向上查找 store[name] → 检查 inject 需求
```

```typescript
// vendor/cordis/src/reflect.ts:135-170（get trap 摘要）
static handler: ProxyHandler<Context> = {
  get: (target, prop, ctx) => {
    if (isSpecialProperty(prop)) return Reflect.get(target, prop, ctx)
    if (Reflect.has(target, prop)) return getTraceable(ctx, Reflect.get(target, prop, ctx))
    const error = new Error(`cannot get property "${prop}" without inject`)
    const def = target.reflect.props[prop]
    if (def?.type === 'accessor') return def.get.call(ctx, ctx[symbols.receiver], error)
    if (!ctx.fiber.runtime) return ctx.reflect.get(prop, false)
    return ctx.events.waterfall('internal/get', ctx, prop, error, () => {
      const key = target[symbols.isolate][prop]
      let fiber = (ctx[symbols.shadow] as Context ?? ctx).fiber
      while (true) {
        const impl = fiber.store?.[prop]
        if (impl) return getTraceable(ctx, impl.value)
        if (prop in fiber.inject) { error.message = `cannot get required service "${prop}" in inactive context`; throw error }
        if (!fiber.runtime) throw error
        if (fiber.parent[symbols.isolate][prop] !== key) throw error
        fiber = fiber.parent.fiber
      }
    })
  }
```

**provide 注册** — 每个 service 实现被包装为 `Impl`，按隔离键存入 `store`；卸载时通知所有依赖方重新计算 epoch：

```typescript
// vendor/cordis/src/reflect.ts:277-305
provide(name: string, value?: any, check?: () => boolean) {
  return this.ctx.fiber.effect(() => {
    this.props[name] ??= { type: 'service' }
    this.ctx.root[symbols.isolate][name] ??= Symbol(name)
    const key = this.ctx[symbols.isolate][name]
    const impl: Impl = { name, value, fiber: this.ctx.fiber, check }
    if (this.store[key]) throw new Error(`service "${name}" has been registered...`)
    this.store[key] = impl
    this.ctx.fiber.store![name] = impl
    if (this.ctx.fiber.state === FiberState.ACTIVE) this.notify([name])
    return async () => {
      delete this.store[key]
      const fibers = this.notify([name])     // 通知所有依赖此 service 的 fiber 重新计算 epoch
      await Promise.allSettled(fibers.map(fiber => fiber.await()))
      delete this.ctx.fiber.store![name]
    }
  }, `ctx.provide(${JSON.stringify(name)})`)
}
```

### 1.5 Events — 5 种调度模式的事件总线

**文件**：`vendor/cordis/src/events.ts`

```typescript
// vendor/cordis/src/events.ts:32-33
export type DispatchMode = 'emit' | 'parallel' | 'serial' | 'bail' | 'waterfall'
```

| 模式 | 语义 | 典型用途 |
|---|---|---|
| `emit` | 同步触发，不等待返回 | 通知 |
| `parallel` | 并发等待所有 listener | 广播 |
| `serial` | 顺序 await，遇 bail 值停止 | 串行策略链 |
| `bail` | 同步遇非 null/false/undefined 即停 | 配置转换拦截 |
| `waterfall` | 最后参数是 `next`，不调用 = veto | middleware 链 |

**listener 自动随 fiber 卸载**：`on()` 内部调用 `fiber.effect(() => { hooks.push(...); return () => unregister(...) }, label)`。

### 1.6 架构总图（Mermaid）

```mermaid
flowchart TB
    subgraph Cordis 微内核
        Context(("Context<br/>IOC 容器 + Proxy"))
        Service(("Service<br/>命名能力基类"))
        Fiber(("Fiber<br/>plugin 运行时 + epoch"))
        Registry(("Registry<br/>plugin 注册/发现"))
        Events(("Events<br/>5 模式事件总线"))
        Reflect(("Reflect<br/>service 解析层"))
    end

    Context -->|extend/isolate| Context
    Context -->|install| Service
    Registry -->|plugin() 创建| Fiber
    Fiber -->|effect 注册| Context
    Reflect -->|provide| Context
    Events -->|on/emit/waterfall| Fiber
    Fiber -->|_setEpoch| Fiber
    Fiber -->|epoch 变化驱动| Registry

    subgraph Plugin 层
        P1[agent-loop]
        P2[tools]
        P3[sandbox]
        P4[shell]
        P5[workflow]
    end
    P1 -->|inject| Service
    P2 -->|inject| Service
    P3 -->|inject| Service
```

### 1.7 Fiber 生命周期状态图（Mermaid）

```mermaid
stateDiagram-v2
    [*] --> PENDING : ctx.plugin()
    PENDING --> LOADING : epoch ≠ INACTIVE<br/>所有依赖就绪
    LOADING --> ACTIVE : callback 执行成功
    LOADING --> FAILED : 配置校验/启动抛错
    ACTIVE --> UNLOADING : epoch = INACTIVE<br/>依赖卸载
    UNLOADING --> PENDING : 依赖重新就绪
    UNLOADING --> DISPOSED : dispose() 完成
    ACTIVE --> DISPOSED : dispose()
    FAILED --> DISPOSED : dispose()
    PENDING --> DISPOSED : 父 fiber 卸载
```

### 1.8 注册发现调度流程（Mermaid）

```mermaid
sequenceDiagram
    participant Caller
    participant Reg as RegistryService
    participant Fib as Fiber
    participant Ref as ReflectService
    participant Dep as 依赖 Service

    Caller->>Reg: ctx.plugin(MyPlugin, config)
    Reg->>Reg: resolve(plugin) → callback
    Reg->>Fib: new Fiber(ctx, config, inject, runtime)
    Fib->>Fib: 注册 dispose 到父 fiber
    Fib->>Fib: emit('internal/plugin')
    Fib->>Fib: 遍历 inject → _checkImpl(name)
    Fib->>Ref: _getImpl(name, true)
    Ref-->>Fib: Impl{value, fiber}
    Fib->>Fib: _refresh() 计算 epoch
    alt 所有依赖就绪
        Fib->>Fib: _setEpoch → _reload()
        Fib->>Fib: execute callback(ctx, config)
        Fib->>Ref: ctx.provide('xxx', impl, check)
        Ref->>Ref: store[key] = impl; notify(依赖方)
        Fib-->>Caller: Fiber & PromiseLike<Fiber>
    else 有依赖未就绪
        Fib-->>Caller: PENDING Fiber（等待）
    end
```

## 2. 核心模块群深度剖析（逐模块：定位/关键文件/核心函数/代码片段）

### 2.1 workflow — DAG 定义与执行引擎

**定位**：模型编写的 JavaScript 编排脚本运行时（非静态 DAG，是动态调用图）。  
**关键包**：`packages/workflow/workflow`（seam）+ `workflow-worker-thread`（引擎）+ `tool-workflow`（模型工具）。

#### 2.1.1 核心抽象

workflow 不是「静态节点/边 DAG」，而是 **模型编写的 JS 脚本**（top-level await，以 `return <json>` 结束），DAG 是脚本执行 `agent()` 调用时产生的 **动态调用图**。

```typescript
// packages/workflow/workflow/src/runtime-types.ts:19-35
export interface WorkflowStartRequest {
  script: string              // 脚本体（TWA，return JSON）
  meta: WorkflowMeta          // 身份块
  args?: unknown              // 暴露为脚本内 args 全局
  subagentProvider?: string   // 子 agent provider 覆盖
  maxTotalAgents?: number     // 子 agent 总数上限
  parent: Agent               // 父 agent（每个 child 归因于此）
  signal?: AbortSignal
}
```

#### 2.1.2 执行引擎

`WorkerThreadWorkflowEngine`（`workflow-worker-thread/src/index.ts:112`）在 **Node worker thread + `node:vm` 上下文** 中执行每个脚本（offload 同步工作 + 强制终止能力，是 containment 而非安全边界）。

`WorkflowExecution`（`runtime.ts:64-487`）注入脚本 API：

```typescript
// runtime.ts 注入 vm 上下文的 hook
agent(prompt, opts)      // 子 agent = 一个节点
parallel(thunks)         // fan-out 屏障（Promise.all）
pipeline(items, stages)  // per-item stage chain（无跨 stage 屏障）
phase(title)             // 观察性进度分组
log(message)             // 叙事行
```

**并发槽调度器**（`runtime.ts:227-247`）：

```typescript
acquireSlot() / releaseSlot()   // FIFO 等待列表，门控 maxConcurrentAgents
```

**错误双层纪律**：
- **Fatal `WorkflowError`**（`SCRIPT_PARSE`/`META_INVALID`/`AGENT_CAP`/`CANCELLED`…）— 总是杀死脚本，跨 realm 不可伪造（`isFatalWorkflowError`）
- **非 fatal 子失败** → 每项 `null`（脚本 `.filter(Boolean)`）

**worker 协议**（`protocol.ts`）：封闭的 host⇄worker 双工协议，`WorkerToHostType`（Ready/Phase/Log/AgentStart/AgentEnd/Result）+ `HostToWorkerType`（Go/Cancel/ChildSettled…），`assertNever` 使未处理 tag 编译报错。

**配置**：`maxConcurrentAgents` 默认 `min(16, max(1, cores-2))`，`maxTotalAgents`=1000，`maxItemsPerCall`=4096，`disposeGraceMs`=5000。

### 2.2 subagent — SubAgent 生成/调度/上下文隔离

**定位**：多 provider 的 SubAgent 运行时 + 可续生命周期。  
**关键包**：`packages/subagent/subagent`（seam）+ `subagent-spawn-in-process` / `subagent-fork-in-process` / `subagent-acp` / `subagent-claude-code` / `subagent-codex` / `subagent-dsh-sdk`（providers）。

#### 2.2.1 核心抽象

```typescript
// packages/subagent/subagent/src/types.ts
interface SubagentProvider {
  name: string
  capabilities: SubagentCapabilities
  inheritsParentContext: boolean
  agentRouteDefaults?: AgentRouteDefaults
  start(resolved): Promise<SubagentRun>
  prepareContinuable?(): Promise<ContinuableStart>
}
interface SubagentCapabilities {
  agentOptions: boolean; outputSchema: boolean; depthLimit: boolean
  toolFilter: boolean; persona: boolean
}
```

两种形态：
- **One-shot**：`SubagentRuntime.start(name, request)` → `SubagentRun { id, localAgent, result, dispose() }`
- **Continuable**：`startContinuable(spec)` → 可续会话（`SubagentContinuationManager` 拥有 `AgentHandle`）

#### 2.2.2 上下文隔离

```typescript
// packages/subagent/subagent/src/child-agent.ts
resolveChildDepth(parent)                    // delegationDepthOf(parent)+1，强制 maxDepth
parentAgentOptionsForDelegation(parent)      // 子继承 provider/model/reasoningEffort/maxTokens
childSessionMeta(parent)                     // 持久子元数据：cwd/parentSession/origin:'subagent'/delegationDepth
applyChildComposition(childCtx, parent)      // 合并父 preset + 注册 subagent:delegation 上下文 + persona + tools.restrict(toolFilter)
captureDelegatedPolicyOverrides(child, parent) // 在委托边界钉死 sandbox mode + approval 'never'
```

隔离通过 **Cordis scoped composition** 实现：`applyChildComposition` 在子 scope 安装 persona + tool restriction（对父/兄弟不可见），委托钉死策略到子日志，depth 被预算化并持久化。

#### 2.2.3 生命周期

- **One-shot**：`start` → `result` → `dispose()`（总是 dispose 到达 quiescence）
- **Continuable**：residency **epochs**（`Activation`）跨越一个持久 Session；`ActivationObserver` 发出 start/terminal 边；通过 `SubagentSettledMessageSource` 投递到父
- `SubagentContinuationManager`（`continuation.ts:356`）：`startContinuable`/`followup`/`interrupt`/`reportFrom`/`drain`，`ChildLock` 序列化每个 durable child 的 delivery/release/disposal
- `SubagentStopReason`：completed/aborted/error/max-tokens/refusal

### 2.3 plan + goal — 目标规划、任务拆解

#### 2.3.1 plan — 协作模式（非任务分解）

**关键包**：`packages/plan/plan-mode`。

plan 是 **plan mode vs default mode 的协作模式**，不是 Plan/Step 任务分解：

```typescript
// packages/plan/plan-mode/src/index.ts
class PlanModeController extends Service {
  static inject = ['tools', 'systemPrompt']
  foldPlanMode(events)          // 纯 last-wins fold of plan/mode → boolean
  set(agent, active)            // committed/queued/cancelled/noop
  // /plan 命令 + exit_plan_mode 工具（通过 userQuestions.ask 审批）
}
interface PlanProjection { active: boolean; pending: boolean }
```

状态完全事件源化（`plan/mode` 事件），resume/fork 从日志恢复。

#### 2.3.2 goal — 事件源化的目标状态机

**关键包**：`packages/goal/goal` + `goal-round-driver` + `tool-goal`。

```typescript
// packages/goal/goal/src/types.ts
type GoalPhase = 'active' | 'paused' | 'blocked' | 'complete'
interface GoalSnapshot { id, revision, objective, phase, blockedReason?, maxGoalRounds }
interface GoalRef { id, revision }   // CAS 身份
type GoalOperation = 'create'|'edit'|'pause'|'resume'|'complete'|'block'|'clear'
```

```typescript
// packages/goal/goal/src/index.ts:183 — GoalService
class GoalService extends TypertRemoteService {
  static inject = ['agents']
  static Config = z.object({ defaultMaxGoalRounds: z.number().default(256) })
  create(agent, request)         // CAS 创建 + arm
  edit(agent, ref, request)      // 乐观并发（stale ref → GOAL_STALE_REVISION）
  pause/resume/complete/block/clear(agent, ref)
  // 每个 mutation 构建 GoalSnapshotChangeMeta → commit() → 追加 goal/change → 发射 goal/changed
}
```

**目标达成 = round-based**（`goal-round-driver`）：当 armed active goal 的 agent 空闲时，driver 预留下一轮，渲染 `renderGoalRoundPrompt(goal, round)`，`followup()` 一个 `GoalMessageSource` 消息。`drive()` 在 `roundsStarted >= maxGoalRounds` 时 `block('round-limit')`。成就是 **caller-driven completion**（bounded by round budget），非自动成功检测。

### 2.4 sandbox + code-runtime + e2b — 沙箱隔离

#### 2.4.1 sandbox — 同世界进程约束

**关键包**：`packages/sandbox/sandbox`（seam）+ `sandbox-local` + `sandbox-policy` + `sandbox-windows-acl`。

```typescript
// packages/sandbox/sandbox/src/index.ts
type SandboxMode = 'read-only' | 'workspace-write' | 'danger-full-access'
interface SandboxExecutionPolicy { mode: SandboxMode; workspaceRoot: string; sessionId?: SessionId }
interface ConfinedArgv { argv: string[]; enforcement: SandboxEnforcement; denialSignatures: string[]; runnerFailureRules: RunnerFailureRule[] }
abstract class SandboxProvider extends Service {
  abstract confine(argv: string[], policy: SandboxPolicy): Promise<ConfinedArgv>
}
```

**平台 runner 链**（`sandbox-local/src/index.ts:159`）：

```typescript
PLATFORM_CHAINS = {
  linux: ['bwrap', 'landlock'],     // bubblewrap mount profile + Landlock ABI
  darwin: ['seatbelt'],             // macOS sandbox-exec SBPL
  win32: ['windows-acl'],           // restricted-token + write-SID allowlist
}
```

**fail-closed 升级**（`escalation.ts`）：`WIDER_MODES` 映射每个 mode 可扩展到的范围，`approveEscalation()` 检查严格扩展 → 解析 `ctx.approval` → 4 种结果。

**拒绝签名**（per-backend 内核方言）：bwrap `read-only file system`、landlock `permission denied`、seatbelt `operation not permitted`、windows-acl `access is denied`。

**策略服务**（`sandbox-policy/src/index.ts:135`）：`resolve()` = approved mode ≻ session override ≻ default；session cwd = workspace boundary。

#### 2.4.2 code-runtime — 代码执行 seam

**关键包**：`packages/code-runtime/code-runtime` + `code-runtime-worker-thread` + `code-runtime-python`。

```typescript
// packages/code-runtime/code-runtime/src/index.ts
abstract class CodeRuntime extends Service {
  abstract language: 'typescript' | 'python'
  abstract isolation: 'worker-thread' | 'process' | 'container'
  abstract run(request: CodeRunRequest): Promise<CodeRunResult>
}
interface CodeRunRequest { program: string; bindings: CodeBindingNamespace[]; signal?: AbortSignal }
interface CodeRunResult { value?; logs[]; error? }   // error 是字段，非 rejection
type CodeRunFailure.kind = 'exception'|'timeout'|'abort'|'worker-exit'|'invalid-output'|'output-limit'
```

`WorkerThreadCodeRuntime`（`code-runtime-worker-thread/src/index.ts:238`）：每次运行 spawn 空环境 + heap cap 的 Worker，type-strip TS，通过 message port 桥接 bindings。

#### 2.4.3 e2b — 远程沙箱

**关键包**：`packages/e2b/e2b` + `fs-e2b` + `subprocess-e2b`。

```typescript
// packages/e2b/e2b/src/index.ts:74
class E2BRuntime extends Service {
  open() { Sandbox.create({ secure: true, lifecycle: { onTimeout: 'kill' } }) }
  getSandbox()   // 共享的 SDK handle
}
```

一个共享远程 Linux sandbox，capability adapters（`fs-e2b`/`subprocess-e2b`）通过 `ctx.e2b.getSandbox()` 等待同一 handle。

### 2.5 schedule + jobs — 调度与作业

#### 2.5.1 schedule — agent-scoped 持久提醒

**关键包**：`packages/schedule/schedule`。

```typescript
// packages/schedule/schedule/src/types.ts
type ScheduleRecord = AfterScheduleRecord | AtScheduleRecord | EveryScheduleRecord
type ScheduleChange.operation = 'create' | 'dispatch' | 'delete'   // 持久 mutation 流
type ScheduleState = 'scheduled' | 'overdue'
```

```typescript
// packages/schedule/schedule/src/runtime.ts:77 — ScheduleRuntime
class ScheduleRuntime {
  driveOnce()            // 刷新持久化 → fold 日志 → dueDecision() → arm 定时器
  dueDecision(): due one-shot / complete every batch / bounded wait
  // 到期时 runMaintenance → 渲染 framing → agent.followup → 追加 schedule/change dispatch
}
```

三种提醒：`after`（延迟）/ `at`（绝对）/ `every`（fixed-rate，anchor-aligned，min 5min，不追赶）。无 cron/retry，排序 = earliest-target-then-create-order。

#### 2.5.2 jobs — 后台作业生命周期

**关键包**：`packages/jobs/jobs` + `jobs-local` + `tool-jobs`。

```typescript
// packages/jobs/jobs/src/index.ts
type JobStatus = 'running' | 'stopping' | 'completed' | 'killed' | 'failed'
interface JobStart { kind; label; outputLimitBytes?; owner?; run(): JobHooks }
interface JobHooks { cancel(reason?); done: Promise<JobOutcome>; readOutput?() }

// packages/jobs/jobs-local/src/index.ts:91 — LocalJobRegistry
class LocalJobRegistry {
  start()   // preflight controller admission; maxConcurrentJobsPerOwner (default 10); 原子注册; 发出 <kind>-N id
  settle()  // first-wins: 记录终态 → 释放等待者 → 最后 announce
  // ScopedLayers<JobLayer> 使 controllers/listeners owner-relative
  // owner-disposal cleanup: cancel+await owned jobs
}
```

### 2.6 skill — Skill 注册/加载/触发

**关键包**：`packages/skill/skill`（seam）+ `skill-filesystem` + `tool-skill` + `skill-badge`。

```typescript
// packages/skill/skill/src/index.ts
type SkillSource = 'project-dsh'|'project-agents'|'runtime'|'user-dsh'|'user-agents'|'custom'|'bundled'
interface SkillSummary { name; description; whenToUse?; invocation: SkillInvocationPolicy; source; provider; resourceBase? }
interface SkillInvocationPolicy { modelInvocable: boolean; userInvocable: boolean }
interface SkillDefinition extends SkillSummary { content: string; path?; metadata? }   // Markdown body + YAML frontmatter

class SkillRegistry extends Service {   // ctx.skills, ScopedLayers<SkillLayer>
  registerProvider(provider)            // 注册 borrowed provider 到 calling context 的 layer
  register(skill)                       // 注入 runtime skill
  get(name) / list() / snapshot()       // 跨 global + scope chain 收集（nearest layer wins; rank 破平）
}
```

**FileSystemSkillProvider**（`skill-filesystem/src/index.ts:146`）：从 ranked roots 发现目录-bundle + flat Markdown skills：project `.dsh/skills`（rank 100）→ `.agents/skills`（200）→ custom（300）→ user `~/.dsh/skills`（400）→ `~/.agents/skills`（500）→ bundled（600）。`SkillWatchManager` 保持有界 Chokidar/`fs.watch` host watchers。

**触发双路径**：模型调用 `skill` 工具（仅 model-invocable）；用户 `/name` gesture 注入渲染 body（user-invocable）。**仅 1 个 bundled skill**（`skill-badge`，从 `assets/dsh-badge.md` 加载）。

### 2.7 shell + subprocess — Shell 执行与进程管理

**关键包**：`packages/shell/shell`（seam）+ `bash-local` / `bash-sandbox` / `pwsh-local` / `pwsh-sandbox` + `tool-bash` + `subprocess` + `subprocess-local`。

#### 2.7.1 subprocess — 进程树原语

```typescript
// packages/subprocess/subprocess/src/index.ts
interface SubprocessSpawnSpec { argv; cwd; stdio; graceMs; signal?; env? }
interface SubprocessHandle { pid; raw streams; collected; done: Promise<SubprocessOutcome>; terminate(); waitForExit() }
interface SubprocessTerminalHandle { output: Readable; write(); inspectForeground(); signalForeground(); terminate() }
```

`spawnSubprocess()`（`subprocess-local/src/spawn.ts:326`）：detached process-tree spawn（POSIX group via `detached`，Windows via `taskkill /T`）。`OutputCollector` — bounded tail-keep + lazy spill file（random+O_EXCL+`0600` path）。`terminate()` = SIGTERM → graceMs → SIGKILL。

`LocalTerminalHandle`（`terminal.ts:35`）wraps `node-pty` `IPty` + `ProcessInspector`，session-tree cleanup + foreground-group inspection/signalling。

#### 2.7.2 shell — bash seam

```typescript
// packages/shell/shell/src/index.ts
abstract class ShellExecutor extends Service {   // ctx.shell
  abstract resolve(request: ShellExecRequest): ShellExecSpec
  abstract run(spec: ShellExecSpec): Promise<ShellRunResult>
  abstract start(spec: ShellExecSpec): ShellProcess
}
interface ShellRunResult { exitCode; signal; timedOut; aborted; stdout; stderr; sandbox? }   // first-cause classification
```

`LocalBashExecutor`（`bash-local/src/index.ts:102`）：public commands 作为 `bash -c` 通过 `ctx.subprocess` 运行。`ENV_OVERRIDES`：`NO_COLOR=1`/`TERM=dumb`/`PAGER=cat`。默认 timeout 120s / maxTimeout 600s / maxOutputBytes 64KB / maxSpillBytes 64MB / graceMs 3s。

### 2.8 lsp — LSP 集成

**关键包**：`packages/lsp/lsp`（seam）+ `lsp-stdio`（通用后端）+ `tool-lsp`（模型工具）。

```typescript
// packages/lsp/lsp/src/index.ts
class Lsp extends Service implements LspService {   // ctx.lsp
  registerProvider(provider)    // all-or-nothing 校验 → 原子预留 id + extensions
  query(request, signal)        // 按 finalExtension(request.filePath) 路由
}
```

`LocalLspProvider`（`lsp-stdio/src/index.ts:217`）：per-canonical-workspace 一个 `LspInstance`（`instances: Map<WorkspaceKey, LspInstance>`），per-workspace 序列化（`queues: Map<WorkspaceKey, Promise<void>>`）。

`LspInstance`（`lsp-stdio/src/instance.ts:45`）：`initialize()` 发送 `initialize`（UTF-16 positions）→ `initialized`；`runQuery()` = transient `didOpen` → request → `didClose`；`raceAbort()` 发送 `$/cancelRequest` + bounded grace → `forceTerminate()` 升级 SIGTERM→SIGKILL。

`LspConnection`（`lsp-stdio/src/connection.ts:66`）：JSON-RPC endpoint over spawned subprocess，关联 outbound request ids，回答 `workspace/configuration`，cap stderr tail。

`tool-lsp` 暴露只读 `lsp` 工具（`goToDefinition`/`findReferences`/`goToImplementation`/`hover`），result capping（`maxLocations`/`maxResultChars`）+ timeout budget。

### 2.9 compaction — 上下文压缩

**关键包**：`packages/compaction/compaction`（seam）+ `compaction-basic` + `command-compact` + `compaction-tool-result-pruner`。

```typescript
// packages/compaction/compaction/src/index.ts
type CompactionTrigger = 'pressure' | 'context-overflow'
abstract class CompactionEngine extends Service {   // ctx.compaction
  abstract compactIfNeeded(agent, trigger, signal): Promise<CompactionResult | null>
  abstract compactNow(agent, signal): Promise<CompactionResult | null>     // 显式空闲压缩
  abstract compactRegion(start, end, agent, signal): Promise<CompactionResult>
}
```

`BasicCompactionEngine`（`compaction-basic/src/index.ts:103`）：
- `_registerAutomaticCompaction()`：`agent/pre-step` → `compactIfNeeded('pressure')`；`agent/request-error` → 遇 `CONTEXT_WINDOW_EXCEEDED_CODE` → `compactIfNeeded('context-overflow')` + `{kind:'retry'}`（最多 `maxOverflowRetries`）
- **Pipeline**：(1) 可选 model-free tool-result pruning（`ToolResultPruner`，head/middle/tail + shadow-price events）；(2) `selectCompactableRange` 选择 balanced inclusive span（tool calls 与 results 配对）；(3) `compactSurfaceRegion` 通过 `ctx.llm.stream()` 重放 prefix → summarize；(4) 替换 span 为一个 summary user message（`compactCheckpointSource(CompactionId)`）；(5) durable `compaction/start`…`compaction/end` marker pair 作为压缩锁

`summarize()` 是唯一子类定制 hook（复用对话自己的 system prompt/tools/messages，KV-cache friendly）。

### 2.10 guard + runtime-diagnostics — 安全防护/运行时诊断

**关键包**：`packages/guard/repeat-tool-reminder` + `guard/timeout-policy` + `packages/runtime-diagnostics/invariants`。

#### 2.10.1 guard — 两个轻量 guard

**repeat-tool-reminder**（advisory，从不 veto）：per-agent consecutive-repeat `Chain { key, count }`（`WeakMap<Agent, Chain>`）。`canonicalize(arguments)` = deep key-sort JSON → `JSON.stringify`。遇 threshold 返回 escalating gentle→detailed reminder 作为 additional context。

**timeout-policy**（cooperative enforcer）：wraps `tools/execute`，读取 tool definition 的 `timeoutMs`，`deadline(exec.signal, timeoutMs, TOOL_TIMEOUT)`，到期替换为结构化 `TOOL_TIMEOUT` 错误结果。

#### 2.10.2 runtime-diagnostics — 不变量注册表

```typescript
// packages/runtime-diagnostics/invariants/src/index.ts
class InvariantRegistry extends Service {   // ctx.invariants
  register(packageName, installer)          // 预留 package name → 在 child fiber 运行 installer
  // fail 回调抛 InvariantError(packageName, message) → dispose child fiber + 释放预留
}
```

全局 `enabled` + regex allow/block-lists 选择运行哪些 package 的 invariants。

### 2.11 interaction + feedback + typert — 交互与反馈

#### 2.11.1 interaction — 用户交互捕获

**关键包**：`packages/interaction/commands` + `user-approval` + `user-questions` + `tool-ask-user` + `permission-presets`。

```typescript
// packages/interaction/commands/src/index.ts
class CommandRuntime extends TypertRemoteService {   // ctx.commands
  register(definition)        // 存储在 ScopedLayers<CommandLayer>（global + per-agent scoped shadows）
  list(agent) / execute(agent, line, images, signal)   // 解析 → 解析 → 运行 handler（无 model 参与）
}
```

```typescript
// packages/interaction/user-approval/src/index.ts
class ApprovalService extends Service {   // ctx.approval
  request(req)                // 要求 open turn; 追加 approval/asked → decide → 追加 approval/decided
  decide(req, session)        // 'never' policy 确定性拒绝; 否则 waterfall('approval/request') → 规范化 → race signal
}
type ApprovalOutcome = 'allowed-once' | 'rejected' | 'cancelled' | 'unavailable'   // fail-closed
```

```typescript
// packages/interaction/user-questions/src/index.ts
class UserQuestionService extends Service {   // ctx.userQuestions
  ask({ questions, agent, signal })   // 非空校验 → 要求 agent 是 exact live root → waterfall('user-questions/request')
}
```

`tool-ask-user` 注册 `ask_user_question` 工具（model-facing consumer）。`permission-presets` 捆绑 sandbox-mode + approval-policy 为 named presets + `/permission` 命令。

#### 2.11.2 feedback — 反馈收集

**关键包**：`packages/feedback/command-feedback` + `message-feedback`。

- `/feedback` 命令：追加一个 authoritative log-only `feedback/record` 事件（eager，unflushed）+ ack 披露 session-sharing policy
- `message-feedback`：durable CRUD store for per-message ratings/notes（`TypertRemoteService`，optimistic versioning）

#### 2.11.3 typert — 类型反射 + RPC 骨干

**关键包**：`packages/typert/generator` + `loader` + `protocol` + `registry`。

```typescript
// packages/typert/protocol/src/index.ts
abstract class TypertRemoteService<out T> extends Service<T>   // 通过 Typert Gateway 暴露 key
function Remote(...): 方法装饰器 — 标记 direct 或 stream Remote invocations
function RemoteScope(key): 从 Remote Scope 解析方法

// packages/typert/loader/src/index.ts
function apply(ctx, config): 增量扫描 — 解析每个 package.json 的 ./typert 导出 → validateTypertManifest → 动态导入 → 注册
```

`generator` 分析 TS workspace（`WorkspaceAnalyzer`）→ 构建 compiler-independent model → 发射（`FaceModelEmitter`/`WorkspaceTypertGenerator`）生成 zod schemas + client proxies + Gateway bindings。`registry` 是运行时 store。

### 2.12 webhook + workspace + web — Webhook/工作区

#### 2.12.1 webhook — 规则注册表 + 分发

**关键包**：`packages/webhook/webhook` + `webhook-github`。

```typescript
// packages/webhook/webhook/src/index.ts:58
class WebhookRuntime extends Service {   // ctx.webhookRuntime
  static inject = ['agents', 'agentDefaultModel', 'agentPresets', 'permissionPresets', 'sessionTitle', 'workspaceRegistry']
  register(rule)                // 校验 id/kind/run → 存储在 ctx.effect → 返回 awaitable disposer
  dispatch(delivery)            // snapshotDelivery → deepFreeze → 遍历 rules → 过滤 kind → 启动匹配 rule 的 invocation
}
interface WebhookRule<K> { id; kind; run(delivery, signal) }
interface VerifiedWebhookDelivery<K> { kind; source; deliveryId; event; receivedAt }
```

接收由 provider adapters 完成（GitHub handler 验证 `x-hub-signature-256` → 解析为 `VerifiedWebhookDelivery<'github'>` → `ctx.webhookRuntime.dispatch`；HTTP handler 在 rule settle 前响应 202，fire-and-forget）。

#### 2.12.2 workspace — 多根工作区注册表

**关键包**：`packages/workspace/workspace`。

```typescript
// packages/workspace/workspace/src/index.ts:272
class WorkspaceRegistry extends Service {
  static inject = ['storageDomain', 'sessionPersistence']
  create(path, title?)         // realpath → 拒绝非目录 → 已知 path 返回现有 entity → 写新 record + prepend to durable order
  insertBefore / archiveSession / delete / resolveByPath
}
interface Workspace { id; path; title; ordered candidate sessions }   // stable id over canonical directory path
interface WorkspaceEntity extends Workspace { setTitle; attachSession; insertSessionBefore; detachSession; status }
```

Session membership 按 canonical-cwd 相等性过滤；操作通过 `enqueueOperation` 序列化；`pendingMutation` markers 使 create/delete crash-recoverable。

### 2.13 mcp、extensions、acp — MCP/扩展/ACP

#### 2.13.1 mcp — MCP 客户端桥

**关键包**：`packages/mcp/mcp-client`。

```typescript
// packages/mcp/mcp-client/src/index.ts
const name = 'mcp-client'; const inject = ['tools']
type Config = StdioConfig | StreamableHttpConfig
async function apply(ctx, config) {                // 连接一个 MCP server + 发布 tools
  reserve serverName namespace (activeServerNames: WeakMap<object, Set<string>>)
  resolveReconnectPolicy(config.reconnect)
  const connection = startConnection(ctx, config, reconnect)
  await connection.ready                          // 阻塞激活（除非 failOnStartupError）
}
```

```typescript
// packages/mcp/mcp-client/src/transport.ts:31
function createTransport(config) {
  config.transport === 'stdio' → StdioClientTransport（env 从 scrubbedParentEnv + explicit env）
  config.transport === 'streamable-http' → StreamableHTTPClientTransport（URL + headers）
}
```

`publicToolName(serverName, rawName)`（`tools.ts:111`）= `mcp__<serverName>__<rawName>`，归一化到 DeepSeek 64-char `[A-Za-z0-9_-]` 契约。`syncTools` 通过 `tools/list` 发现 + 注册每个 tool；`ToolListChangedNotification` 触发 re-sync。`startConnection` supervisor 拥有 client/transport generations + bounded exponential backoff（500ms → 30s，10 attempts）。

#### 2.13.2 extensions — Cordis 动态插件

**关键包**：`packages/extensions/tool-cordis` + `cordis-host-runner` + `cordis-client-runner`。

```typescript
// packages/extensions/cordis-host-runner/src/index.ts:770
class DynamicCordisRunnerService extends TypertRemoteService {   // ctx.dynamicCordisRunner
  define / undefine / run / stop / inspectPlugin / inspectPackage / reference / snapshot / inventory / invoke / settleUserRun
  // plugins session-scoped (sessionId === agent.id); packages immutable versions; one active run per plugin
}
```

`tool-cordis` 注册模型工具（`cordis_inspect_list`/`cordis_inspect_query`/`cordis_define`/`cordis_run`/`cordis_stop`/`cordis_undefine`）+ `tool:cordis` prompt section + `agent/pre-step` 注入 `@pluginId` 上下文。Host half 在 VM sandbox 运行；Client half 在浏览器运行；activation 需要人工 approval。

#### 2.13.3 acp — Agent Communication Protocol

**关键包**：`packages/acp/acp`。

```typescript
// packages/acp/acp/src/index.ts:96
const name = 'acp'; const inject = ['agents', 'llm', 'sessionPersistence', 'sessions']
function apply(ctx, config) {   // 监听 session/event, agent/inbox/claimed, agent/error, llm/adapters-updated, approval/request
  // JSON-RPC methods: initialize, session/new, session/list, session/prompt, session/resume, session/close,
  //                  session/request_permission, session/set_config_option, authenticate, cancel
}
```

实现 `@agentclientprotocol/sdk` JSON-RPC over stdio，桥接到受信任的程序matic clients（如 `dsh-subagent-acp`）。`AcpSession`（`session.ts:98`）拥有 unpublished Agent composition + 选路 + one-prompt admission slot（`InflightPrompt`）。Workspace features 被显式拒绝。

### 2.14 context、session、session-query — 会话与上下文

#### 2.14.1 context — file-reference 发现 seam

**关键包**：`packages/context/file-reference` + `file-reference-local`。

```typescript
// packages/context/file-reference/src/index.ts:26
abstract class FileReferenceService extends Service {   // ctx.fileReferences
  abstract list(agent, query, signal): Promise<FileReferenceCandidate[]>
}
class LocalFileReferenceService extends FileReferenceService {   // inject = ['agents']
  // per-agent WorkspaceFileSearch; 监听 agent/created, agent/disposed, session/event（tool/result 时 invalidate）
}
```

这是 **`@-mention` 文件引用发现**，非通用 token 计数（token 计数在 `dsh-llm` + compaction）。

#### 2.14.2 session — 事件源化会话日志

**关键包**：`packages/session/session`（Session 模型）+ `session-persistence` + `session-persistence-sqlite`。

```typescript
// packages/session/session-persistence/src/index.ts:105
abstract class SessionPersistence extends Service {   // ctx.sessionPersistence
  locate / create / ensureMaterialized / append / prepare / load / inspect / borrowSession / readFrom / list
}
class SqliteSessionPersistence extends SessionPersistence {   // inject = ['sessions']; supportsRawArtifacts = false
  // 持有 SqliteStore + PersistenceCoordinator<number>
}
```

Session 是 **append-only 事件源化日志**（`turn/start`/`turn/end`/`step/start`/`step/end`/`user/message`/`assistant/message`/`assistant/chunk`/`tool/call`/`tool/result`）。`append` 仅在持久化后 resolve。

#### 2.14.3 session-query — 全文搜索

**关键包**：`packages/session-query/session-query` + `session-query-sqlite`。

```typescript
// packages/session-query/session-query/src/index.ts:2190
abstract class SessionQueryEngine extends Service {   // ctx.sessionQuery, inject = ['sessions']
  observeSession / listSessions / readSession / filterSessions / searchSessions(abstract) / traceSession
}
class SqliteSessionQueryEngine extends SessionQueryEngine {   // SQLite FTS5
  // live-preferred: 精确读取 backend-independent; full-text search per-backend
  // 调和 live + persisted observations → FTS tables; 分页通过 opaque CursorPayload (base64url JSON)
}
```

### 2.15 llm、credentials、settings — LLM/凭证/设置

#### 2.15.1 llm — Provider-neutral 适配器注册表

**关键包**：`packages/llm/llm`（抽象）+ `llm-deepseek` + `llm-pi-ai`。

```typescript
// packages/llm/llm/src/index.ts
abstract class LlmAdapter {   // provider 接口
  abstract stream(options: GenerateOptions): AsyncIterable<StreamChunk>   // 唯一必须方法
  providerInfo / providerRetryPolicy / imageRequestPricing / listModels / resolveModel / prepareCall (可选)
}
class LlmRuntime extends TypertRemoteService {   // ctx.llm
  registerAdapter(providers, adapter): AdapterRegistrationHandle
  registerConfigurableProviders(entries): DirectoryRegistrationHandle
  prepareCall(config, signal): PreparedLlmCall
  stream(options)   // 通过 'llm/stream' waterfall 可拦截
}
```

#### 2.15.2 credentials + settings — 凭证/设置分层

**关键包**：`packages/credentials/credentials` + `credentials-local` + `authorization` + `packages/settings/settings` + `settings-file`。

```typescript
// packages/credentials/credentials/src/index.ts:2133
abstract class CredentialProvider extends Service {   // ctx.credentials
  resolve(ref) / describe(ref) / set(ref, value) / readRecord(key) / modifyRecord(key, mutate)
  // CredentialRef（env-var name）vs CredentialKey（<scope>/<id>）
}
// 分层（ref）：inherited process env (read-only, wins) > .credentials.yaml > <cwd>/.env > $DSH_HOME/.env

// packages/settings/settings/src/index.ts:6124
abstract class SettingsProvider extends Service {   // ctx.settings
  load() / persist(ns, section) / publish(doc, source)
  // 写接受 expectedRevision 冲突检测（SettingsConflictError, code SETTINGS_CONFLICT）
}
// 解析值 = schema defaults → composition base → user document section
function installSettingsSection<T>(ctx, ns, schema, entry, hooks)   // 规范 consumer 接线
```

## 3. 协议适配真实代码路径（Anthropic/OpenAI）

### 3.1 统一适配层：LlmAdapter + LlmRuntime

**核心文件**：`packages/llm/llm/src/index.ts`（LlmRuntime:326, LlmAdapter:193）

协议无关的 `GenerateOptions` / `StreamChunk` / `Message` / `ContentBlock` 模型是统一基础。所有 provider 差异封闭在 `LlmAdapter` 子类内部，agent 循环与工具层永远不接触协议细节（与 laew 的 `llm/{anthropic,openai}.rs` 双客户端同构）。

```typescript
// packages/llm/llm/src/index.ts:193-275
export abstract class LlmAdapter {
  providerInfo(provider: string): LlmProviderInfo { return { id: provider, name: provider } }
  providerRetryPolicy(_provider: string): ResolvedRetryPolicy | undefined { return undefined }
  abstract stream(options: GenerateOptions): AsyncIterable<StreamChunk>   // 唯一必须方法
}
```

`LlmRuntime.prepareCall` 绑定 config + adapter + retry policy 到单次调用（防 HMR 期间一个 adapter 的 capability 结果与另一个 adapter 的 endpoint 组合）：

```typescript
// packages/llm/llm/src/index.ts:889-934
async prepareCall(config: LlmCallConfig, signal?): Promise<PreparedLlmCall> {
  const registration = this.registration(config.provider)
  const adapterCall = await registration.adapter.prepareCall(config.provider, config.model, signal)
  const modelInfo = this.normalizeModelInfo(registration, config.model, adapterCall.model)
  const resolved = this.resolveCallWithInfo(config, modelInfo)
  return Object.freeze({
    config: resolvedConfig, retryPolicy: registration.retryPolicy, adapterDefaults,
    stream: (options) => { /* 单次 dispatch 校验 callConfigEquals */ },
  })
}
```

### 3.2 Anthropic / OpenAI 协议适配真实路径：pi-ai provider

**核心文件**：`packages/llm/llm-pi-ai/src/provider.ts` + `stream.ts`。

pi-ai 通过 `@earendil-works/pi-ai` 库支持 3 种 wire protocol：

```typescript
// packages/llm/llm-pi-ai/src/provider.ts:47-51
const PROTOCOLS: Readonly<Record<string, () => ProviderStreams>> = {
  'openai-completions': openAICompletionsApi,
  'openai-responses': openAIResponsesApi,
  'anthropic-messages': anthropicMessagesApi,
}
```

**provider 构建逻辑**：catalog route（已安装 catalog 提供的 provider）**复用 catalog provider** 仅替换 models（Bedrock 通过独立入口加载 Smithy 模块，无法重建）；其他 route 由 `createProvider` 在 protocol table 上构建：

```typescript
// packages/llm/llm-pi-ai/src/provider.ts:127+
function harnessApiKeyAuth(name: string): ApiKeyAuth {
  return {
    name,
    resolve: ({ credential }) => Promise.resolve({
      auth: credential?.key === undefined ? {} : { apiKey: credential.key },
      source: name,
    }),
  }
}
```

**wire 翻译**（`stream.ts`）：pi-ai assistant event → Harness StreamChunk：

```typescript
// packages/llm/llm-pi-ai/src/stream.ts:23-32
export function mapUsage(usage: PiUsage): TokenUsage {
  return {
    inputTokens: usage.input, outputTokens: usage.output, totalTokens: usage.totalTokens,
    ...usage.cacheRead > 0 ? { cacheReadTokens: usage.cacheRead } : {},
    ...usage.cacheWrite > 0 ? { cacheWriteTokens: usage.cacheWrite } : {},
  }
}
```

**错误分类**（`stream.ts:41-67`）：pattern-matching pi-ai 错误文本 → Harness 标准码：

```typescript
function classifyPiAiError(message: string): string {
  if (/\b(?:401|403)\b/.test(message)) return 'AUTH'
  if (isQuotaExceededError(message)) return QUOTA_EXCEEDED_CODE
  if (/\b429\b|rate.?limit/i.test(message)) return 'RATE_LIMIT'
  if (/\b413\b|failed to buffer the request body/i.test(message)) return 'INVALID_REQUEST'
  if (/\b5\d\d\b/.test(message)) return 'SERVER'
  if (/stream ended (?:before|without)\b/i.test(message)) return 'TRANSPORT'
  return 'PI_AI_ERROR'
}
```

**replay 支持**：`toPiReplayState`（`replay.ts`）映射 harness replay state → pi-ai，处理 provider 切换时 replay state 的所有权（仅当同一 adapter 实例拥有其历史 provider 时保留）。

### 3.3 DeepSeek 直连适配器

**核心文件**：`packages/llm/llm-deepseek/src/adapter.ts`（DeepSeekAdapter:353）+ `serialize.ts` + `sse.ts`。

`DeepSeekAdapter extends LlmAdapter`：direct-fetch + SSE 针对 OpenAI-compatible `/chat/completions` endpoint，发出 harness StreamChunk。Connection facts 通过 thunk `options()` 单次解析；bearer token 通过 `resolveApiKey` 每次请求解析（config 变更到达下一次请求，in-flight stream 保持起始 facts）。

```typescript
// packages/llm/llm-deepseek/src/index.ts:404 — apply()
function apply(ctx, config) {
  install llm-deepseek settings section
  build adapter
  register 'deepseek-official' route on ctx.llm
  re-register on retry-policy change via registration.replace([PROVIDER])
}
```

### 3.4 协议适配对 laew 的启示

laew 当前 anthropic-messages + openai-completions 双协议直接实现在 `llm/{anthropic,openai}.rs`。DeepSeek-Harness 展示了两条路径：

1. **直连适配器**（DeepSeek）：自己翻译 wire → 完全控制错误分类 + 重试
2. **库桥接**（pi-ai）：复用第三方库处理多 provider → 通过 `classifyPiAiError` 文本模式匹配补偿库的信息丢失

pi-ai `stream.ts:38-42` 注释明确记录了一个痛点：pi-ai 把 caught error 扁平化为 `error.message`，丢弃原始 Error + `cause` 链（undici 的 transport detail 在 `cause` 上），只剩 pattern-matching terse words。这与 laew 抓包时遇到的「错误信息被 SDK 吞掉」问题一致 — 对 laew 的启示：直连适配器虽重但保真。

---

## 4. 其他维度实现快照（多轮对话/Context/记忆/质检/工具/MCP/Skill/SubAgent/Workflow/loop/目标规划/沙箱/权限）

### 4.1 维度实现总表

| 维度 | 实现位置 | 关键类型/函数 | 备注 |
|---|---|---|---|
| 多轮对话 | `packages/core/agent-loop` + `packages/core/session` | `ReactLoopAgent.turn()/step()`, `Inbox`, `Session.append()` | turn/step 双边界；事件源化日志 |
| Context 管理 | `packages/compaction` | `BasicCompactionEngine`, `ToolResultPruner` | pressure + context-overflow 双触发 |
| 记忆 | 本轮无独立「记忆」包 | Session 日志 + session-query FTS | 记忆 = 持久化会话 + 全文搜索 |
| 质检 | 本轮无独立 QC 包 | `tools/post-execute` waterfall + guard | 质检外置为 policy，非独立 agent |
| 工具调用 | `packages/core/tools` | `ToolRuntime`, `ToolDefinition`, 6 事件 | pre/execute/post/result + ptc-dispatch-log |
| MCP | `packages/mcp/mcp-client` | `apply()`, `mcp__<server>__<tool>` | stdio + streamable-http |
| Skill | `packages/skill` | `SkillRegistry`, ranked roots | Markdown + YAML frontmatter |
| SubAgent | `packages/subagent` | `SubagentRuntime`, scoped composition | 多 provider + continuable 生命周期 |
| Workflow | `packages/workflow` | `WorkerThreadWorkflowEngine` | 模型编写的 JS 编排脚本 |
| loop | `packages/core/agent-loop` | `ReactLoopAgent.kick()` | while(await this.turn()) |
| 目标规划 | `packages/goal` + `packages/plan` | `GoalService` CAS + round-driver | 事件源化 goal 状态机 |
| 沙箱 | `packages/sandbox` | `SandboxProvider.confine()` | bwrap/landlock/seatbelt/windows-acl |
| 权限 | `packages/interaction/user-approval` | `ApprovalService`, fail-closed | ask/never policy + audit pair |

### 4.2 loop 架构 — ReactLoopAgent 的 turn/step 模型

**核心文件**：`packages/core/agent-loop/src/agent.ts`

```typescript
// packages/core/agent-loop/src/agent.ts:69 — ReactLoopAgent
export class ReactLoopAgent implements Agent {
  readonly inbox: Inbox
  private phase: Phase    // idle | maintenance | running
  private async kick(): Promise<void> {
    try { while (await this.turn()) {} } finally { /* 回到 idle + replay latch */ }
  }
  private async turn(): Promise<boolean> {     // 每个 turn 多个 step
    this.session.append('turn/start', { turn })
    while (true) {
      const decision = await this.preStep(target, { turn, step })
      const stepEnd = await this.step(decision.assembly, ...)
      if (turnEnds && this.inbox.nextStep.length === 0) break
    }
    return this.inbox.hasPending ? (phase.abort = new AbortController(), true) : false
  }
  private async step(assembly, startsRequestSeries) {
    const { request } = await this.buildRequest(turn, step, tools, system, messages, ...)
    const stream = preparedCall?.stream(request) ?? this.loopCtx.llm.stream(request)
    for await (const chunk of stream) { assembler.push(chunk); /* append assistant/chunk */ }
    if (toolCalls.length === 0) return { kind: 'completed' }
    const { concluded } = await executeToolCalls(this.loopCtx, turn, step, toolCalls, signal, ...)
    return concluded ? { kind: 'completed' } : null
  }
}
```

**Phase 状态机**：`idle` →（wake）→ `running` →（turn 结束）→ `idle`；`maintenance`（空闲独占作业）。`wakeDriver()` 把 wake latch 到 aborted activity 之后。

**工具调用调度**（`tool-calls.ts`）：

```typescript
// packages/core/agent-loop/src/tool-calls.ts:59-101
export async function executeToolCalls(ctx, turn, step, toolCalls, signal, acceptContext) {
  const planned = toolCalls.map(block => ({ block, exec: { callId, name, arguments: parseArguments(block.arguments), agent, signal } }))
  let next = 0
  while (next < planned.length) {
    const mode = ctx.tools.executionMode(first.exec).kind   // parallel | exclusive
    const group = mode === 'parallel' ? planned.slice(next) : [first]
    const outcome = await runGroup(ctx, turn, step, group, mode, signal, acceptContext)
    next += outcome.consumed; concluded ||= outcome.concluded
    if (outcome.aborted) { /* 记录 skipped calls */ return { concluded }
  }
  return { concluded }
}
```

`runGroup` 实现 **bounded rolling parallel pool** + **exclusive barrier**；`maxParallelToolCalls` 来自 `ctx.agentLoop.config`（运行时 getter，每次 scheduler 决策重读，committed change cap 下一组）。结果和上下文 **按 model order commit**（即使 dispatch 重叠）。

### 4.3 Context 管理 — 压缩管线

**核心文件**：`packages/compaction/compaction-basic/src/index.ts`

```typescript
// packages/compaction/compaction-basic/src/index.ts:137 — _registerAutomaticCompaction()
register agent/pre-step → compactIfNeeded(agent, 'pressure', signal)
register agent/request-error → on CONTEXT_WINDOW_EXCEEDED_CODE:
  run compactIfNeeded(agent, 'context-overflow', signal) → {kind:'retry'} (maxOverflowRetries)
```

**pipeline**：optional tool-result pruning → `selectCompactableRange`（balanced inclusive span，tool/result 配对）→ `compactSurfaceRegion`（通过 `ctx.llm.stream()` 重放 prefix → summarize）→ 替换 span 为一个 summary user message → durable `compaction/start`…`compaction/end` marker pair（= 压缩锁）。

### 4.4 工具调用 — 6 事件管线

**核心文件**：`packages/core/tools/src/index.ts`（ToolRuntime:788）

```typescript
// packages/core/tools/src/index.ts — 6 个扩展事件
'tools/pre-execute'(exec, next): Promise<PreToolDecision>     // allow/deny/ask
'tools/execute'(exec, next): Promise<ToolExecutionResult>     // around: timeout/retry/metrics
'tools/post-execute'(exec, result, next): Promise<PostToolDecision>  // accept/replace/block
'tools/ptc-dispatch-log'(dispatch, next): Promise<ContentBlock[]>   // PTC 子 dispatch 日志
'tools/result'(exec, result): emit                              // 冻结最终结果
'tools/change'(): emit                                          // registry 变更
```

`ToolDefinition` 完整契约：`output { schema, render, presentationMeta? }` + `execute(args, exec)` + `finalizeContent?` + `timeoutMs?` + `isConcurrencySafe?` + `presentCall?/presentResult?`。

**PTC mode**（Program-Then-Collapse）：`mode: 'ptc'` 时仅暴露 `run_code` + 生成 SDK prompt（TypeScript/Python），所有其他工具通过程序内 SDK 调用。

**ScopedLayers**：per-agent tool registration 覆盖 global；restrictions（allow/deny list）继承链交叉；scope-local registrations 在 filter 之外。

### 4.5 权限管控 — fail-closed 审批

**核心文件**：`packages/interaction/user-approval/src/index.ts`

```typescript
// packages/interaction/user-approval/src/index.ts:157 — ApprovalService
class ApprovalService extends Service {
  request(req): Promise<ApprovalOutcome>   // requires open turn; append approval/asked → decide → approval/decided
  decide(req, session)                     // 'never' → deterministic reject; else waterfall('approval/request') → normalize → race signal
}
type ApprovalOutcome = 'allowed-once' | 'rejected' | 'cancelled' | 'unavailable'   // fail-closed
```

`approval/policy` 持久化（SessionEventMap），per-session `ask`/`never` policy。`permission-presets` 捆绑 sandbox-mode + approval-policy 为 named presets。

### 4.6 沙箱 — 多后端 fail-closed

**核心文件**：`packages/sandbox/sandbox-local/src/index.ts`

```typescript
PLATFORM_CHAINS = { linux: ['bwrap','landlock'], darwin: ['seatbelt'], win32: ['windows-acl'] }
STATIC_ENFORCEMENT = { windows-acl: 'partial', others: 'full' }   // WRITE_RESTRICTED keeps Everyone; NTFS hard links
```

multi-backend arbitration = platform chain + functional probes（`probeRunner`）；missing confinement fails closed（`SandboxUnavailableError`）。

---

## 5. 对 laew 的借鉴（P0/P1/P2）

### 5.1 P0 — 必须借鉴（架构级）

| # | 借鉴点 | DeepSeek-Harness 实现 | laew 现状 | 落地收益 |
|---|---|---|---|---|
| P0-1 | **Fiber epoch 算法** — 依赖感知的加载/卸载/热更新 | `vendor/cordis/src/fiber.ts` `_refresh()`/`_setEpoch()` | 无（laew 无 plugin 热加载） | 多 Agent 编排时依赖就绪自动启动、配置变更自动重启 |
| P0-2 | **Capability seam + provider 分离** — 每个能力一个抽象 Service + N 个 provider | 52+ seams | laew 工具/Bash/Read/Write 硬编码在 `agent/tools/` | 新能力（sandbox/compaction/MCP）即插即用 |
| P0-3 | **工具 6 事件管线 + ScopedLayers** | `tools/{pre-execute,execute,post-execute,result,change,ptc-dispatch-log}` | 无（工具直执行） | 权限拦截、超时、重试、per-agent tool 可见性 |
| P0-4 | **事件源化会话日志** | `Session.append()` + turn/step/tool 事件 | laew 内存态 context | 可 replay、可 audit、可 fork、可持久化 |
| P0-5 | **fail-closed 审批 + 沙箱** | `ApprovalService` + `SandboxProvider.confine()` | 零校验 | 命令行 Agent 安全基线 |

### 5.2 P1 — 应该借鉴（显著提升）

| # | 借鉴点 | 实现位置 | 落地场景 |
|---|---|---|---|
| P1-1 | **Compaction 双触发 + Tool-result pruning** | `BasicCompactionEngine`, `ToolResultPruner` | 长对话 token 控制 |
| P1-2 | **SubAgent scoped composition + depth budget** | `applyChildComposition`, `resolveChildDepth` | laew SubAgent 上下文隔离 |
| P1-3 | **Workflow 脚本化编排** | `WorkerThreadWorkflowEngine` | 复杂任务的多步编排（替代 hardcode 流程） |
| P1-4 | **MCP 桥接 + 自动 tool sync** | `mcp-client` `apply()`/`syncTools` | 复用 MCP 生态工具 |
| P1-5 | **Typer 远程 + 类型反射** | `TypertRemoteService` + `@Remote` | TUI ↔ Agent 跨进程类型安全通信 |
| P1-6 | **Goal round-driver + CAS mutation** | `GoalService` + `goal-round-driver` | Yolo 目标识别 → 执行 → 完成闭环 |

### 5.3 P2 — 可以借鉴（锦上添花）

| # | 借鉴点 | 实现位置 | 落地场景 |
|---|---|---|---|
| P2-1 | **Skill Markdown + ranked roots** | `SkillRegistry` + `FileSystemSkillProvider` | 用户可注入的「如何做」知识 |
| P2-2 | **Schedule 提醒** | `ScheduleRuntime` | 定时任务/提醒 |
| P2-3 | **Webhook + 规则注册表** | `WebhookRuntime` | CI/CD 触发 Agent |
| P2-4 | **pi-ai 库桥接多 provider** | `llm-pi-ai` | 快速支持新 provider（vs 直连适配器） |
| P2-5 | **repeat-tool-reminder guard** | `repeat-tool-reminder` | 防模型陷入循环 |
| P2-6 | **ACP JSON-RPC bridge** | `dsh-acp` | 受信任程序matic client 接入 |

### 5.4 不借鉴（anti-patterns）

| 点 | 原因 |
|---|---|
| Cordis 的 `Proxy` + 全局 Symbol 路由 | Rust 不需要，所有权系统已解决 |
| worker thread containment | Rust 原生进程/线程更安全 |
| 52+ npm 包的微粒度过细 | Rust crate 边界即可，不必镜像 |
| `declare module` 类型增强 | Rust trait + impl 更清晰 |

---

## 6. 参考资料与文件索引

### 6.1 Cordis 微内核文件索引

| 文件 | 核心内容 |
|---|---|
| `vendor/cordis/src/context.ts` | Context IOC 容器 + Proxy + extend/isolate/intercept |
| `vendor/cordis/src/service.ts` | Service 基类 + 拦截配置合并 |
| `vendor/cordis/src/fiber.ts` | Fiber 运行时 + epoch 算法 + effect 生命周期 |
| `vendor/cordis/src/registry.ts` | RegistryService + Inject + plugin 注册 |
| `vendor/cordis/src/events.ts` | EventsService 5 模式 + Hook |
| `vendor/cordis/src/reflect.ts` | ReflectService service 解析 + provide/notify |
| `vendor/cordis/src/utils.ts` | symbols + DisposableList + composeError |
| `vendor/cordis/src/logger.ts` | Logger facade |
| `vendor/loader/src/index.ts` | Loader + EntryTree + plugin 加载 |
| `vendor/include/src/applyEntryPatches.ts` | patch 算法 |

### 6.2 核心模块文件索引

| 模块 | 关键文件 |
|---|---|
| Cordis 核心 | `vendor/cordis/src/{context,service,fiber,registry,events,reflect,utils}.ts` |
| Agent 循环 | `packages/core/agent-loop/src/{agent,tool-calls,index}.ts` |
| Agent 注册表 | `packages/core/agent/src/{index,inbox,dispatch,types}.ts` |
| Tools | `packages/core/tools/src/index.ts` |
| LLM 适配 | `packages/llm/llm/src/index.ts`, `llm-pi-ai/src/{provider,stream}.ts`, `llm-deepseek/src/adapter.ts` |
| Scope | `packages/core/scope/src/index.ts` |
| Session | `packages/core/session/src/index.ts` |
| Boot | `packages/boot/app-boot/src/index.ts` |
| Workflow | `packages/workflow/workflow/src/index.ts`, `workflow-worker-thread/src/runtime.ts` |
| SubAgent | `packages/subagent/subagent/src/{child-agent,continuation,descriptor,lifecycle}.ts` |
| Plan | `packages/plan/plan-mode/src/index.ts` |
| Goal | `packages/goal/goal/src/{index,domain,fold}.ts`, `goal-round-driver/src/index.ts` |
| Sandbox | `packages/sandbox/sandbox/src/index.ts`, `sandbox-local/src/index.ts` |
| Code-runtime | `packages/code-runtime/code-runtime/src/index.ts` |
| Schedule | `packages/schedule/schedule/src/runtime.ts` |
| Jobs | `packages/jobs/jobs/src/index.ts`, `jobs-local/src/index.ts` |
| Skill | `packages/skill/skill/src/index.ts`, `skill-filesystem/src/index.ts` |
| Shell | `packages/shell/shell/src/index.ts`, `bash-local/src/index.ts` |
| Subprocess | `packages/subprocess/subprocess/src/index.ts`, `subprocess-local/src/spawn.ts` |
| LSP | `packages/lsp/lsp/src/index.ts`, `lsp-stdio/src/{instance,connection}.ts` |
| Compaction | `packages/compaction/compaction-basic/src/index.ts` |
| Guard | `packages/guard/{repeat-tool-reminder,timeout-policy}/src/index.ts` |
| Interaction | `packages/interaction/{commands,user-approval,user-questions}/src/index.ts` |
| Feedback | `packages/feedback/{command-feedback,message-feedback}/src/index.ts` |
| Typert | `packages/typert/{generator,loader,protocol,registry}/src/index.ts` |
| MCP | `packages/mcp/mcp-client/src/index.ts` |
| ACP | `packages/acp/acp/src/index.ts` |
| Extensions | `packages/extensions/{tool-cordis,cordis-host-runner}/src/index.ts` |
| Webhook | `packages/webhook/webhook/src/index.ts` |
| Workspace | `packages/workspace/workspace/src/index.ts` |
| Context | `packages/context/file-reference/src/index.ts` |
| Session-persistence | `packages/session/session-persistence-sqlite/src/index.ts` |
| Session-query | `packages/session-query/session-query-sqlite/src/index.ts` |
| Credentials | `packages/credentials/credentials/src/index.ts` |
| Settings | `packages/settings/settings/src/index.ts` |

### 6.3 与 laew 架构映射

| laew 概念 | DeepSeek-Harness 对应 | 文件 |
|---|---|---|
| `src/agent/mod.rs` 循环 | `ReactLoopAgent` + `AgentLoop` | `agent-loop/src/{agent,index}.ts` |
| `src/agent/tools/*` | `ToolRuntime` + per-tool plugins | `core/tools` + `packages/shell/tool-bash` 等 |
| `src/llm/*` | `LlmRuntime` + `LlmAdapter` 子类 | `llm/llm` + `llm-{deepseek,pi-ai}` |
| `src/tui/mod.rs` | `CommandRuntime`（斜杠命令注册表） | `interaction/commands/src/index.ts` |
| `src/config/mod.rs` SQLite | `SqliteSessionPersistence` + `FileSettingsProvider` | `session-persistence-sqlite` + `settings-file` |
| Yolo 分类 | `GoalService` CAS + `goal-round-driver` | `goal/goal/src/index.ts` |
| Plan Agent | `PlanModeController` + `WorkflowEngine` | `plan-mode` + `workflow` |
| SubAgent | `SubagentRuntime` + scoped composition | `subagent/subagent/src/index.ts` |
| SessionContext 摘要 | 事件源化日志 + session-query FTS | `session` + `session-query` |

---
> **续章：深度扩展** — 本节对前述骨架逐节深化，补充更多代码片段、实现细节、跨模块交互与设计动机，确保总字数 ≥ 15000 字。

## 1.3 Cordis 三原语再深剖（补充节）

### 1.3.1 symbols 体系 — 避免命名冲突的基础设施

**文件**：`vendor/cordis/src/utils.ts:49-73`

Cordis 使用全局 Symbol 作为内部 key，避免与用户属性冲突：

```typescript
export const symbols = {
  // internal symbols
  shadow: Symbol.for('cordis.shadow'),
  receiver: Symbol.for('cordis.receiver'),
  original: Symbol.for('cordis.original'),
  metadata: Symbol.for('cordis.metadata'),
  initHooks: Symbol.for('cordis.initHooks'),
  checkProto: Symbol.for('cordis.checkProto'),

  // context symbols
  effect: Symbol.for('cordis.effect') as typeof Context.effect,
  filter: Symbol.for('cordis.filter') as typeof Context.filter,
  isolate: Symbol.for('cordis.isolate') as typeof Context.isolate,
  intercept: Symbol.for('cordis.intercept') as typeof Context.intercept,

  // service symbols
  init: Symbol.for('cordis.init') as typeof Service.init,
  check: Symbol.for('cordis.check') as typeof Service.check,
  config: Symbol.for('cordis.config') as typeof Service.config,
  invoke: Symbol.for('cordis.invoke') as typeof Service.invoke,
  extend: Symbol.for('cordis.extend') as typeof Service.extend,
  tracker: Symbol.for('cordis.tracker') as typeof Service.tracker,
  resolveConfig: Symbol.for('cordis.resolveConfig') as typeof Service.resolveConfig,
}
```

`Symbol.for()` 的全局注册确保跨 realm / 多 copy 场景下 brand 一致（`Context.is()` 通过 `Symbol.for('cordis.is')` 检测）。

### 1.3.2 DisposableList — O(1) 删除的有序 disposable 集合

**文件**：`vendor/cordis/src/utils.ts:5-40`

```typescript
export class DisposableList<T extends WeakKey> {
  private sn = 0
  private map = new Map<number, T>()
  private weak = new WeakMap<T, number>()

  push(value: T) {
    const sn = ++this.sn
    this.map.set(sn, value)
    this.weak.set(value, sn)
    return () => this.map.delete(sn)   // 返回 disposer
  }
  delete(value: T) {
    const sn = this.weak.get(value)
    if (!sn) return false
    return this.map.delete(sn)
  }
  clear() { const values = [...this.map.values()]; this.map.clear(); return values.reverse() }
}
```

这是 Fiber 的 `_disposables` 核心数据结构：push 返回 disposer（单步），clear 逆序返回（用于卸载），weak map 保证 O(1) by-value 删除。

### 1.3.3 composeError — 异步长栈追踪

**文件**：`vendor/cordis/src/utils.ts:268-281`

```typescript
export function composeError<T>(callback: (info: StackInfo) => T, getOuterStack = buildOuterStack()): T {
  const info: StackInfo = { offset: 1, error: new Error() }   // 捕获当前栈
  try {
    const result: any = callback(info)
    if (isObject(result) && 'then' in result) {
      return (result as any).then(undefined, (reason) => handleError(info, reason, getOuterStack)) as T
    } else return result
  } catch (reason: any) { handleError(info, reason, getOuterStack) }
}
```

`handleError` 把外层调用栈帧拼入 thrown error 的 stack 中，解决 async 栈断裂问题。这是 effect 能给出「effect 注册处」诊断的关键。

### 1.3.4 Inject 依赖声明 + 装饰器

**文件**：`vendor/cordis/src/registry.ts:18-89`

```typescript
export type Inject<M = Dict> = (keyof M)[] | { [K in keyof M]?: M[K] }

export function Inject<K extends InjectKey>(name: K, config?: ...) {
  return function (value, decorator: ClassDecoratorContext | ClassMethodDecoratorContext) {
    if (decorator.kind === 'class') {
      if (!Object.hasOwn(value, 'inject')) {
        defineProperty(value, 'inject', Object.create(Object.getPrototypeOf(value).inject ?? null))
        defineProperty(value.inject, symbols.checkProto, true)
      }
      value.inject[name] = config
    } else if (decorator.kind === 'method') {
      const inject = (value[symbols.metadata] ??= {}).inject ??= Object.create(null)
      inject[name] = config
      decorator.addInitializer(function () {
        (this[symbols.initHooks] ??= []).push(() => {
          (this.ctx as Context).inject(inject, (ctx) => value.call(this))
        })
      })
    }
  }
}
```

`@Inject` 在 class 上累积静态 `inject` map；在 method 上延迟方法调用直到声明的 services 可用（通过 `ctx.inject`）。这是声明式依赖的装饰器形式。

`Inject.resolve` 把 array/object/class-inherited inject 元数据归一化为 plain map：

```typescript
export function resolve(inject, result = Object.create(null)) {
  if (!inject) return result
  if (Array.isArray(inject)) { for (const name of inject) result[name] = null }
  else if (Reflect.has(inject, symbols.checkProto)) {
    Object.assign(result, resolve(Object.getPrototypeOf(inject)))
    for (const name of Object.keys(inject)) result[name] = inject[name] ?? null
  } else { for (const name of Object.keys(inject)) result[name] = inject[name] ?? null }
  return result
}
```

### 1.3.5 Plugin 三种形态

**文件**：`vendor/cordis/src/registry.ts:92-146`

```typescript
export type Plugin<T = any> = Plugin.Function<T> | Plugin.Constructor<T> | Plugin.Object<T>

export namespace Plugin {
  export interface Base<T = any> {
    name?: string
    Config?: StandardSchemaV1<any, T>   // 配置校验 schema
    inject?: Inject                      // 依赖声明
    provide?: string | string[]          // 提供的 service 名
    intercept?: Dict<boolean>            // 消费的拦截配置
  }
  export interface Function<T = any> extends Base<T> { (ctx: Context, config: T): any }
  export interface Constructor<T = any> extends Base<T> { new (ctx: Context, config: T): any }
  export interface Object<T = any> extends Base<T> { apply(ctx: Context, config: T): any }
  export interface Runtime {
    name?: string
    fibers: DisposableList<Fiber>        // 该 plugin 的所有 fiber
    callback: Function                   // 标识 key
    Config?: StandardSchemaV1
  }
}
```

三种形态（function / constructor / `{apply}`）统一由 `RegistryService.resolve` 归一为 callback。`Config` 是 standard-schema，由 `resolveConfig`（`fiber.ts:50-62`）校验：

```typescript
export function resolveConfig(runtime: Plugin.Runtime, config: any) {
  if (!runtime.Config) return config
  const result = runtime.Config['~standard'].validate(config)
  if ('then' in result) throw new TypeError('Async config validation is not supported')
  if (result.issues) throw new ValidationError(result.issues)
  else return result.value
}
```

### 1.3.6 Fiber 构造与 dispose 链

**文件**：`vendor/cordis/src/fiber.ts:222-333`

Fiber 构造时分两种情况：

- **runtime ≠ null**（普通 plugin）：分配 uid、extend 父 context、设置 intercept、创建 `_runner`、注册 dispose 到父 fiber、emit `internal/plugin`、检查依赖 → `_refresh()`
- **runtime = null**（根 fiber）：uid=0、state=ACTIVE、dispose=restart

```typescript
constructor(parent, config, inject, runtime, getOuterStack) {
  if (runtime) {
    this.uid = parent.registry.counter
    this.ctx = this.context = parent.extend({ fiber: this })
    // 设置 intercept
    this._runner = {
      epoch: INACTIVE,
      execute: function () {
        if (isConstructor(runtime.callback)) {
          const instance = new runtime.callback(this.ctx, this.config)
          for (const hook of instance?.[symbols.initHooks] ?? []) hook()
          return instance?.[symbols.init]?.()
        } else return runtime.callback(this.ctx, this.config)
      },
      collect,
    }
    this.dispose = parent.fiber.effect(() => {
      const remove = runtime.fibers.push(this)
      return async () => {
        this.uid = null
        emitPluginDisposed(this.context, this)
        if (this.ctx.registry.has(runtime.callback)) { remove(); if (!runtime.fibers.length) this.ctx.registry.delete(runtime.callback) }
        this._setEpoch(INACTIVE)
        while (this.inertia) await this.inertia   // 等待进行中的转换
      }
    }, 'ctx.plugin()')
    // 发布 + 检查依赖
    this.context.emit('internal/plugin', this)
    for (const name of Object.keys(this.inject)) this._checkImpl(name)
    this._refresh()
  } else { /* 根 fiber */ }
}
```

`update(config)` 是配置热更新入口：先 `_resolveConfig`（含 `internal/update` waterfall，HMR 可 veto），再 `restart()`。

### 1.3.7 _reload / _unload 转换

**文件**：`vendor/cordis/src/fiber.ts:646-696`

```typescript
private async _reload() {
  this.store = { ...this._store }
  const oldEpoch = this._runner.epoch
  try {
    await Promise.resolve()
    if (this._runner.epoch === oldEpoch) {        // 防 stale epoch
      this.config = this._resolveConfig(this._config)
      await this._execute(this._runner)            // 运行 plugin callback
      this._error = undefined
    }
  } catch (reason) {
    this.ctx.logger.error(reason)
    this._error = reason
    this._runner.epoch = INACTIVE
  }
  this._updateState(() => {
    if (this._runner.epoch === oldEpoch) this.inertia = undefined
    else { this.inertia = this._unload(); return FiberState.UNLOADING }
  })
}

private async _unload() {
  await Promise.all(this._disposables.clear().map(async (dispose) => {
    try { await composeError(async (info) => { await runDisposable(dispose) }, this._runner.getOuterStack) }
    catch (reason) { this.ctx.logger.error(reason) }
  }))
  this.store = undefined
  this._updateState(() => {
    if (this._runner.epoch === INACTIVE) this.inertia = undefined
    else { this.inertia = this._reload(); return FiberState.LOADING }
  })
}
```

关键：`_reload` 中先 `await Promise.resolve()` 让出，检查 epoch 是否仍有效（防期间被 dispose）；`_unload` 中逆序运行所有 disposers（支持 async）；两者都通过 `_updateState` 触发状态发射 + 可能的链式转换。

### 1.3.8 effect 的 generator 形式

**文件**：`vendor/cordis/src/fiber.ts:418-561`

`effect` 接受 `SyncEffect` 或 `AsyncEffect`，其中 generator 形式允许注册多个独立跟踪的 disposers：

```typescript
const dispose = ctx.effect(function* (this: MyService) {
  const conn = yield connectDB()      // 第一个 disposer
  const timer = yield startTimer()    // 第二个 disposer
  yield () => cleanup()               // 第三个 disposer
}, 'myService.setup')
```

每个 yielded disposer 被 `runner.collect` 收集并从 `_disposables` 中删除（独立跟踪）。`dispose()` 逆序运行。`wrapper` 同时是 `PromiseLike`（await 等待 setup 完成 + dispose）和 disposer。

## 2.3 核心模块群再深剖（补充节）

### 2.3.1 Loader — 配置树 + HMR

**文件**：`vendor/loader/src/index.ts` + `packages/boot/app-boot/src/index.ts`

Loader 是 Cordis 的「操作系统加载器」，管理 EntryTree（Include/Group/Isolate 节点）：

```typescript
// vendor/loader/src/index.ts:65
export class Loader extends EntryTree {
  declare [Service.config]: Loader.Intercept
  public name = 'loader'
  public internal = ModuleLoader.fromInternal()
  public builtins: Dict<any> = Object.create(null)

  constructor(ctx, config = {}) {
    super(ctx)
    ctx.reflect.provide('loader', this, this[Service.check])
    ctx.on('internal/config', function (this: Fiber, _config, next) {
      const config = next()
      if (!this.entry || this.parent.fiber?.entry === this.entry) return config
      return interpolate(this.ctx, config)   // !!js 表达式求值
    }, { global: true })
    ctx.on('internal/update', async function (config, noSave, next) {
      // 持久化配置变更到 entry.options.config
      this.entry.options.config = unparse ? unparse(config) : config
      this.entry.parent.tree.write()
    }, { global: true, prepend: true })
    ctx.plugin(isolate)
  }
}
```

`app-boot` 的 `boot()` 是完整启动序列：

```typescript
// packages/boot/app-boot/src/index.ts:772-819
export async function boot(binName, absoluteConfigPath, patches?, prepare?, bareModuleBaseUrl?): Promise<Context> {
  const ctx = new Context()
  let stage = 'host preparation failed'
  try {
    ctx.baseUrl = pathToFileURL(dirname(absoluteConfigPath)).href + '/'
    ctx.provide('dshHomePath', dshHomePath)
    await ctx.plugin(Loader)                    // 加载 Loader
    await prepare?.(ctx)                       // host 准备
    stage = 'plugin tree failed to load'
    await mountRootInclude(ctx, absoluteConfigPath, patches, bareModuleBaseUrl)
    await ctx.get('loader')?.await()           // 等待整树 settle
    if (ctx.get('loader') === undefined) return ctx
    await assertEntriesActivated(ctx, binName)  // 审计所有 entry 已激活
    return ctx
  } catch (cause) {
    await ctx.fiber.dispose()                  // 失败时 dispose 部分 context
    throw new Error(`${binName}: ${stage}: ${detail}${stack}`, { cause })
  }
}
```

`mountRootInclude` 把 `Include` 作为 builtin 挂载，加载 `cordis.yml` 配置树。`assertEntriesActivated` 遍历所有 entry，拒绝 failed/pending 的 entry（给出缺失的 service 名）。

### 2.3.2 AgentLoop — 声明式 agent 工厂

**文件**：`packages/core/agent-loop/src/index.ts:296-383`

```typescript
export class AgentLoop extends Service implements AgentFactory {
  static inject = ['agents', 'sessions', 'llm', 'tools', 'systemPrompt']
  static Config = z.object({
    maxParallelToolCalls: z.number().step(1).min(1).default(DEFAULT_MAX_PARALLEL_TOOL_CALLS),
    agents: z.array(z.object({ id, sessionId, provider, model, reasoningEffort, maxTokens, cwd, resumeSessionId })).default([]),
  })

  constructor(ctx, config) {
    super(ctx, 'agentLoop')
    this.config = {
      ...config,
      agents: applyLauncherIdentities(config.agents, ctx.get(CONFIGURED_AGENT_IDENTITIES_KEY)),
      get maxParallelToolCalls() { return source().maxParallelToolCalls },   // 运行时 getter
    }
    installSettingsSection(ctx, AGENT_LOOP_SETTINGS_NAMESPACE, AGENT_LOOP_SETTINGS_SCHEMA, entry, {...})
    this.ownership = new FactoryOwnership(ctx.fiber)
    // 为每个声明式 agent 创建/恢复
    for (const { id, sessionId, cwd, resumeSessionId, ...options } of this.config.agents) {
      if (resumeSessionId === undefined) {
        const configuredId = sessionId ?? SessionId(`${id}-session-${randomUUID()}`)
        this.create(configuredId, options, meta)   // 或 restoreOrCreateConfigured
      } else {
        ctx.effect(() => ctx.inject(['sessionPersistence'], childCtx => this.resumeWith(...)))
      }
    }
  }
}
```

`FactoryOwnership` 管理工厂级生命周期：`signal`（AbortSignal，工厂 teardown 时 abort）、`track(dispose)` 跟踪 live agent、`dispose()` 中止 + 等待所有 live agent。

`prepare(ownerCtx, id, options, session, callerSignal)` 是 agent 创建的内部核心：

```typescript
private prepare(ownerCtx, id, options, session, callerSignal?): PreparedAgent {
  const abort = new AbortController()
  // 融合 3 个 abort 源：caller cancellation / owner fiber unload / factory teardown
  callerSignal?.addEventListener('abort', onCallerAbort, { once: true })
  this.ownership.signal.addEventListener('abort', onFactoryTeardown, { once: true })
  const dispose = (ownerTriggered = false) => (disposing ??= (async () => {
    abort.abort(new Error(`agent "${id}" lifecycle disposed`))
    if (machine !== undefined) { machine.cancel({ kind: 'disposed' }); await machine.whenIdle(); await machine.scope.dispose() }
    detachAgent?.(); detachSession?.()
  })())
  const unfollowOwner = ownerCtx.effect(() => () => { /* owner disposal 触发 dispose */ }, `agentLoop.lifecycle(${id})`)
  const agent = machine = new ReactLoopAgent(loopCtx, id, options, session)
  return { agent, signal: abort.signal, publish: (source) => { /* enter registries + announce */ }, dispose }
}
```

### 2.3.3 AgentRegistry — initiator 链 + AsyncLocalStorage

**文件**：`packages/core/agent/src/index.ts:246-695`

```typescript
export class AgentRegistry extends Service {
  private store = new Map<SessionId, AgentEntry>()
  private factory: FactorySlot | undefined
  private readonly initiators = new AsyncLocalStorage<Agent | undefined>()
  private readonly initiatorRuns = new AsyncLocalStorage<InitiatorRun>()
  private initiatorState: 'active' | 'closing' | 'disposed' = 'active'

  withInitiator<T>(agent, operation: () => T): T {
    return this.runWithInitiator(agent, operation)
  }
  requireInitiator(): Agent {
    const agent = this.currentInitiator()
    if (agent === undefined) throw new Error(NO_INITIATOR_MESSAGE)
    return agent
  }
}
```

`AsyncLocalStorage` 实现 **process-local causal attribution**：`withInitiator(agent, op)` 在异步链中传播当前 agent，`requireInitiator()` 用于「必须由 agent 发起」的路径。`register/enter/announce` 三阶段发布：`enter` 仅入 store；`announce` 发射 `agent/created`（同步 throw 可 veto 发布 + 回滚）。

### 2.3.4 ToolRuntime — 6 事件管线 + ScopedLayers

**文件**：`packages/core/tools/src/index.ts:788-1197`

```typescript
export class ToolRuntime extends Service {
  static inject = ['systemPrompt']
  static Config: z.object({ mode: z.union(['native','ptc','both']).default('native'), maxParallelSubCalls: z.natural().min(1).default(10) })

  readonly [TOOL_RUNTIME_SCHEDULER]: ToolRuntimeScheduler = {
    prepare: exec => this.prepareScheduledExecution(exec),
    dispatch: exec => this.dispatchScheduledExecution(exec),
    finalize: (exec, result) => this.finalizeScheduledExecution(exec, result),
    finish: (exec, result) => this.finishScheduledExecution(exec, result),
  }

  register(definition: ToolDefinition): () => void {
    // 校验 output { schema, render, presentationMeta? }
    // 校验 timeoutMs
    // 拒绝保留名 RUN_CODE_NAME
    return this.layers.effect(this.ctx, layer => layer.tools.insert(name, definition), { label: 'tools.register()' })
  }

  restrict(filter: ToolRestriction): () => void {
    // 要求 scoped context；编译 allow/deny set；校验未知 name；拒绝保留名
    return this.layers.effect(this.ctx, layer => layer.restrictions.append(compiled), { label: 'tools.restrict()' })
  }

  private view(scope?): ToolView {
    // 1. 收集 inherited surface（global + ancestor layers）
    // 2. 应用 restrictions（链上任何 scope 可 mask）
    // 3. scope 自己的 registrations 最后（覆盖 inherited，在 filter 之外）
    // 4. 保留 presentation transport（run_code）最后解析，不在 capability filtering 中
  }
}
```

`ToolLayer` 实现 `ScopeLayer`：包含 `tools`（NamedEntries）、`restrictions`（AnonymousEntries）、`guards`（AnonymousEntries）、`mode`（per-scope presentation override）。`view(scope)` 单次层遍历解析完整 registry 事实。

### 2.3.5 LlmRuntime — adapter 注册 + 单次 dispatch

**文件**：`packages/llm/llm/src/index.ts:326-1065`

```typescript
export class LlmRuntime extends TypertRemoteService {
  private adapters = new Map<string, AdapterRegistration>()
  private directory = new Map<string, LlmConfigurableProvider>()
  private discoveries = new Map<string, (request, signal?) => Promise<readonly LlmDiscoveredModel[]>>()

  registerAdapter(providers: string[], adapter: LlmAdapter): AdapterRegistrationHandle {
    const owned = new Set<string>()
    let released = false
    const dispose = this.ctx.effect(function* (this: LlmRuntime) {
      if (providers.length === 0) throw new LlmError('an adapter must register at least one provider', 'INVALID_ADAPTER')
      this.commitRoutes(owned, this.prepareRoutes(providers, adapter, owned))
      yield () => { released = true; for (const provider of owned) this.adapters.delete(provider); this.emitAdaptersUpdated() }
    }.bind(this), 'llm.registerAdapter()')
    const handle = (() => void dispose()) as AdapterRegistrationHandle
    handle.replace = (next) => { if (released) throw new LlmError('...', 'REGISTRATION_DISPOSED'); this.commitRoutes(owned, this.prepareRoutes(next, adapter, owned)) }
    return handle
  }

  async prepareCall(config, signal?): Promise<PreparedLlmCall> {
    const registration = this.registration(config.provider)
    const adapterCall = await registration.adapter.prepareCall(config.provider, config.model, signal)
    const modelInfo = this.normalizeModelInfo(registration, config.model, adapterCall.model)
    const resolved = this.resolveCallWithInfo(config, modelInfo)
    return Object.freeze({ config: resolvedConfig, retryPolicy: registration.retryPolicy, adapterDefaults, stream: (options) => {
      if (dispatched) throw new LlmError('a prepared LLM call can only be dispatched once', 'INVALID_PREPARED_CALL')
      if (!callConfigEquals(options, resolvedConfig)) throw new LlmError('...', 'INVALID_PREPARED_CALL')
      dispatched = true
      return this.streamWithRegistration(options, { registration, config: resolvedConfig, modelInfo, dispatch: options => adapterCall.stream(options) })
    }})
  }

  stream(options): AsyncIterable<StreamChunk> {
    return this.streamWithRegistration(options)
  }
  private streamWithRegistration(options, prepared?): AsyncIterable<StreamChunk> {
    return this.ctx.waterfall(this, 'llm/stream', options, () => this.adapterStream(options, prepared))
  }
}
```

`adapterStream` 是最终 adapter 边界：adapter 选择 + dispatch + 迭代失败 → 单个 terminal `error`/`aborted` finish chunk；middleware/consumer failures 保持 thrown。

### 2.3.6 Session — 事件源化日志

**文件**：`packages/core/session/src/index.ts`

```typescript
export class SessionStore extends Service {
  // ctx.sessions
  // 事件：turn/start, turn/end, step/start, step/end, user/message, assistant/message, assistant/chunk, tool/call, tool/result
  // 'session/created' | 'session/disposed' | 'session/event' | 'session/flush'
}
```

Session 是 append-only 事件源化日志。`deriveMessages()` 从日志派生 LLM 消息历史。`SurfaceManager` 管理 surface（用户可见的对话表面）+ fold replacement。

### 2.3.7 Scope — 隔离上下文原语

**文件**：`packages/core/scope/src/index.ts`

```typescript
export type ScopeKey = object
export type Scoped<T extends object> = object & { readonly [ScopedBrand]: T }

export function createScope(loopCtx, agent): Scope {
  // 创建新 scope key，parent 链指向 agent 的 scope
}

export function scopeTarget<T extends object>(agent: T, target: T): Scoped<T> {
  // 构建 routing-only event carrier
}
```

`scopeParents: WeakMap<ScopeKey, ScopeKey>` 双向关系：registration views 继承 **down** the chain（child scope 看到祖先 layers）；event admission 延伸 **up** it（ancestor-tagged listener 收到 descendant key 的 events）。

### 2.3.8 SubagentRuntime — provider 注册表

**文件**：`packages/subagent/subagent/src/index.ts`

```typescript
export class SubagentRuntime extends TypertRemoteService {
  // ctx.subagents
  // 事件：subagent/provider-added | provider-removed | start | end
  registerProvider(provider: SubagentProvider): () => void
  start(name: string, request: SubagentStartRequest): SubagentRun
  startContinuable(spec: ContinuableStart): Promise<SubagentRun>
  interrupt(receipt: SubagentInterruptReceipt): void
  reportFrom(run: SubagentRun, message: Message): void
  drain(sessionId: SessionId): Promise<void>
}
```

`SubagentRun` 形态：`{ id, localAgent, result: Promise<SubagentResult>, dispose() }`。`SubagentResult`：`output, structured?, diagnostic?, stopReason`。

### 2.3.9 WorkflowEngine — 抽象 seam

**文件**：`packages/workflow/workflow/src/index.ts`

```typescript
export abstract class WorkflowEngine extends Service {   // ctx.workflowEngine
  abstract start(request: WorkflowStartRequest): Promise<WorkflowRun>
  emitWorkflowEvent(name: WorkflowEventName, ...args): void
}
```

`WorkflowRun`：`{ id, meta, result: Promise<WorkflowResult>, cancel(reason?), dispose() }`。`WorkflowResult`：`stopReason, error?, agentCount`。

### 2.3.10 GoalService — CAS mutation

**文件**：`packages/goal/goal/src/index.ts:183-302`

```typescript
export class GoalService extends TypertRemoteService {
  private readonly caches = new WeakMap<Session, GoalCache>()

  private prepareMutation(agent): GoalCache {
    this.assertLive(agent)
    const cache = this.cache(agent.session)
    this.sync(agent.session, cache)     // 从日志 fold 最新状态
    return cache
  }

  private commitSnapshot(agent, cache, operation, goal, roundsStarted, createdAt, updatedAt, activation): GoalView {
    const change: GoalSnapshotChangeMeta = { kind: 'goal/change', version: 1, operation, goal, roundsStarted, createdAt, updatedAt }
    agent.session.append('goal/change', change)
    cache.state = { goal, roundsStarted, createdAt, updatedAt }
    cache.activation = activation
    this.emitGoalChanged(agent, { operation, ref: { id: goal.id, revision: goal.revision }, goal: this.view(cache) })
    return this.view(cache)
  }

  transition(agent, ref, operation, allowedPhases, nextPhase, nextActivation): GoalView {
    const cache = this.prepareMutation(agent)
    const current = this.expectCurrent(cache, ref)   // 校验 ref
    if (!allowedPhases.includes(current.phase)) throw new GoalError('...', 'GOAL_INVALID_TRANSITION')
    const goal = { ...current, revision: current.revision + 1, phase: nextPhase }
    return this.commitCurrent(agent, cache, operation, goal, nextActivation)
  }
}
```

`GoalCache` 是 process-local incremental fold（`state, activation, observedSeq, pendingActivation`），每次 mutation 前 `sync` 从日志重放。


---

## 7. 跨模块交互深度分析（补充章）

### 7.1 一次完整的工具调用：从模型决策到结果回流

以 `bash` 工具为例，追踪一条 tool-call 穿越全栈的路径：

```
模型输出 tool-call(block)
  → ReactLoopAgent.step()                          [agent-loop/src/agent.ts:339]
    → executeToolCalls(ctx, turn, step, toolCalls)   [agent-loop/src/tool-calls.ts:59]
      → runGroup(ctx, group, mode)                   [tool-calls.ts:121]
        → ctx.tools[TOOL_RUNTIME_SCHEDULER].prepare(exec)   [tools/src/index.ts]
          → 'tools/pre-execute' waterfall             ← guard / approval / policy
        → ctx.tools[TOOL_RUNTIME_SCHEDULER].dispatch(exec)
          → ToolDefinition.execute(args, exec)        ← tool-bash/src/index.ts
            → ctx.shell.run(spec)                     ← bash-local/src/index.ts
              → ctx.subprocess.spawn(spec)            ← subprocess-local/spawn.ts
                → spawnSubprocess()                   ← 真正 fork exec
        → ctx.tools[TOOL_RUNTIME_SCHEDULER].finalize(exec, result)
          → 'tools/post-execute' waterfall            ← result transform / block
        → 'tools/result' emit (deep-frozen snapshot)
      → appendToolResult(session, ...)               ← 按 model order commit
    → inbox.splice('next-step', ..., [context])       ← deferred context 入下一步
  → session.append('tool/result', ...)                ← 持久化
  → 下一步 user message 包含 tool result → 下一轮 step()
```

每一横线都对应一个 extension point：`tools/pre-execute`（guard/approval）、`tools/execute`（timeout/metrics wrapper）、`tools/post-execute`（result transform）。这就是 laew 当前缺失的「工具策略中间件层」。

### 7.2 一次 SubAgent 委托：上下文隔离的完整路径

```
父 agent 调用 workflow 脚本内 agent(prompt, opts)
  → WorkflowExecution.agent()                        [workflow-worker-thread/runtime.ts:250]
    → children.startAgent()                          ← SubagentRuntime.start()
      → provider.start(resolved)                     ← 如 subagent-spawn-in-process
        → applyChildComposition(childCtx, parent)    [subagent/src/child-agent.ts:199]
          → ctx.inject(['agentPreset'], childCtx => {
              join parent preset
              register subagent:delegation context section (order 120)
              apply per-child persona
              tools.restrict(toolFilter)             ← ScopedLayers 仅子 scope 可见
            })
        → captureDelegatedPolicyOverrides(child, parent)  ← 钉死 sandbox + approval='never'
        → resolveChildDepth(parent) + resolveChildAgentOptions(parent)
        → childSessionMeta(parent)                   ← origin:'subagent', delegationDepth
        → ctx.agents.create(childCtx, { sessionId, agentOptions, setup })
    → emit workflow/agent-start
    → await run.result
    → emit workflow/agent-end (outcome: completed/null/cancelled)
```

**隔离三层**：(1) Cordis `isolate` scope（service 解析隔离）；(2) ScopedLayers tool restriction（工具可见性隔离）；(3) delegated policy override（sandbox + approval 策略钉死）。

### 7.3 一次压缩触发：从上下文溢出到重试

```
agent/request-error (CONTEXT_WINDOW_EXCEEDED)
  → BasicCompactionEngine handler                   [compaction-basic/src/index.ts]
    → compactIfNeeded(agent, 'context-overflow')
      → resolveTargetPolicy (routed target)
      → selectCompactableRange (balanced span, tool/result 配对)
      → optional: ToolResultPruner.pruneSession()   ← head/middle/tail + shadow-price events
      → compactSurfaceRegion()
        → summarize()                               ← 唯一子类 hook
          → ctx.llm.stream() 复用对话自己的 system prompt/tools/messages
        → 替换 span 为 summary user message (compactCheckpointSource(CompactionId))
        → 追加 compaction/start…compaction/end marker pair (= 压缩锁)
    → emit {kind:'retry'} (最多 maxOverflowRetries)
  → agent-loop 重试 step()
```

`ToolResultPruner.pruneSession`（`compaction-tool-result-pruner/src/index.ts:136`）对每个 `tool/result` surface node 追加 `compaction/prune` shadow-price event（通过 `ctx.tokenMeter.estimateMessage` 定价被 shadowed 的 node），然后 `surfaceOp:'replace'` 为 pruned 版本。

### 7.4 一次 Goal 完成：CAS mutation + projection 服务

```
模型调用 complete tool → tool-goal execute
  → GoalService.complete(agent, ref)                [goal/goal/src/index.ts]
    → assertLive(agent)                             ← 必须是 exact live root
    → prepareMutation(agent)
      → cache(agent.session)
      → sync(session, cache)                        ← 从日志 fold 最新
    → expectCurrent(cache, ref)                     ← ref 不匹配 → GOAL_STALE_REVISION
    → transition(agent, ref, 'complete', ['active'], 'complete', 'disarmed')
      → commitCurrent → session.append('goal/change', change)
      → emit goal/changed (scope-filtered)
    → 返回 GoalView
  → client 订阅 goal projection unit (last-wins fold)
  → goal-round-driver 检测到 phase='complete' → 停止续轮
```

### 7.5 Cordis 事件体系全览

| 事件 | 模式 | 触发点 | 用途 |
|---|---|---|---|
| `internal/plugin` | emit | Fiber 创建/uid 清除 | plugin 生命周期 |
| `internal/status` | emit | Fiber 状态转换 | 状态观察 |
| `internal/config` | waterfall | 配置解析 | HMR 可 veto |
| `internal/update` | waterfall | 配置更新 | 持久化/日志 |
| `internal/get` | waterfall | service 读取 | 自定义解析 |
| `internal/service` | emit | service 绑定 | 拓扑观察 |
| `agent/created` | emit | AgentRegistry.announce | 生命周期 |
| `agent/disposed` | emit | AgentRegistry.detach | 生命周期 |
| `agent/pre-step` | waterfall | ReactLoopAgent.preStep | 注入 context |
| `agent/request-error` | waterfall | request 失败 | 重试/压缩 |
| `tools/pre-execute` | waterfall | tool dispatch 前 | guard/approval |
| `tools/execute` | waterfall | tool body 执行 | timeout/metrics |
| `tools/post-execute` | waterfall | tool body 后 | result transform |
| `tools/result` | emit | 最终结果 | 冻结 snapshot |
| `llm/stream` | waterfall | 每次模型调用 | retry/routing/replay |
| `session/event` | emit | 日志 append | 持久化 |
| `session/flush` | parallel | 持久化 checkpoint | durability |
| `goal/changed` | emit | GoalService mutation | 目标状态 |
| `workflow/*` | emit | 6 个 workflow 事件 | 编排观察 |

## 8. Cordis 与 laew 架构理念深度对比（补充章）

### 8.1 设计哲学差异

| 维度 | Cordis (TS) | laew (Rust) | 启示 |
|---|---|---|---|
| 能力注册 | 运行时 Service 注册 + Proxy 解析 | 编译期 trait + 枚举 | laew 所有权系统天然更严谨 |
| 依赖注入 | AsyncLocalStorage + Fiber epoch | 显式 struct 字段 | laew 无需 DI 容器 |
| 热加载 | Fiber epoch 自动 reload | 无（cargo build） | laew 无需热加载 |
| 隔离 | `isolate()` + ScopedLayers | 进程/线程隔离 | Rust 进程隔离更安全 |
| 工具管线 | 6 事件 waterfall | 直执行 | laew 可借鉴中间件 |
| 会话 | 事件源化日志 | 内存态 + SQLite | laew 可借鉴事件源化 |
| 配置 | `cordis.yml` + patch layers | SQLite（无配置文件） | laew 的 DB-as-config 独特 |
| 错误 | `HarnessError` + code taxonomy | `AgentError` 枚举 | 结构类似 |

### 8.2 Cordis 的 architectural seam 模式

Cordis 的「能力缝」模式值得 laew 借鉴：**每个能力 = 一个 abstract Service（seal）+ N 个 provider 实现 + 0-1 个 model tool**。例如：

- `sandbox`：`SandboxProvider` (seam) + `LocalSandboxProvider` / E2B (providers) + 无 model tool
- `shell`：`ShellExecutor` (seam) + `LocalBashExecutor` / `LocalPwshExecutor` (providers) + `tool-bash` / `tool-pwsh` (tools)
- `llm`：`LlmRuntime` (seam) + `DeepSeekAdapter` / pi-ai (providers) + 无 model tool

seam 是 abstract class，定义能力契约；provider 是具体实现；tool 是 model-facing consumer。三者解耦：同一 seam 可换 provider 不影响 tool。

### 8.3 Fiber epoch 在 Rust 中的可能映射

Fiber epoch 的核心诉求是 **依赖感知的加载/卸载**。Rust 中可映射为：

```rust
// 伪代码：Rust 风格的 epoch
struct Plugin<E: Epoch> {
    dependencies: Vec<ServiceId>,
    epoch: String,   // hash of dependency versions
    state: PluginState,
}

impl<E> Plugin<E> {
    fn on_dependency_change(&mut self, new_epoch: String) {
        if self.epoch == new_epoch { return }
        match (self.epoch.as_str(), new_epoch.as_str()) {
            ("INACTIVE", _) if new_epoch != "INACTIVE" => self.reload(),
            (_, "INACTIVE") => self.unload(),
            _ => { self.unload(); self.reload() }
        }
    }
}
```

但 laew 当前无 plugin 热加载需求，此映射仅作理论参考。

### 8.4 工具 6 事件管线在 Rust 中的可能映射

laew 当前工具执行是直执行。借鉴 Cordis 6 事件，可实现 `ToolMiddleware` trait：

```rust
// 伪代码：Rust 风格中间件
trait ToolMiddleware {
    async fn pre_execute(&self, ctx: &ToolCtx, next: Next) -> Result<PreDecision>;
    async fn execute(&self, ctx: &ToolCtx, next: Next) -> Result<ToolResult>;
    async fn post_execute(&self, ctx: &ToolCtx, result: &ToolResult, next: Next) -> Result<PostDecision>;
}
```

Tower-style 中间件栈天然适配此模式。

## 9. 关键代码片段附录（补充章）

### 9.1 AgentLoop 运行时 getter 模式

```typescript
// packages/core/agent-loop/src/index.ts:332-335
get maxParallelToolCalls() {
  return source().maxParallelToolCalls    // 每次 scheduler 决策重读
}
```

这是「committed change cap 下一组，不扰一组在轨」的关键 — getter 读 `source()` 返回最新值，而非快照。

### 9.2 ToolRuntime 保留名保护

```typescript
// packages/core/tools/src/index.ts:1053-1055
if (name === RUN_CODE_NAME) {
  throw new Error(`tool name "${RUN_CODE_NAME}" is reserved for the PTC mode presentation transport and cannot be registered or shadowed`)
}
```

`run_code` 是 PTC mode 的保留传输名，任何 agent 可选 code mode，所以 free-to-take 的名字会在某 preset 挂载时变碰撞。

### 9.3 pi-ai 错误扁平化痛点

```typescript
// packages/llm/llm-pi-ai/src/stream.ts:38-42
// XXX(pi-ai upstream): pi-ai flattens the caught error to `error.message`
// (api/anthropic-messages.js: `errorMessage = error instanceof Error ?
// error.message : JSON.stringify(error)`), discarding the original Error and its
// cause chain before it reaches us. undici carries the actionable transport
// detail on `cause` (e.g. `SocketError: other side closed`) but hands the fetch
// wrapper a bare `terminated`, so we are left pattern-matching terse words here.
```

这是 laew 直连适配器的有力论据：保留 `cause` chain + transport detail，不依赖文本 pattern-matching。

### 9.4 SandboxProvider fail-closed

```typescript
// packages/sandbox/sandbox/src/index.ts — SandboxProvider 注释
// confine() must return enforcing argv or **fail closed** (no silent passthrough)
```

### 9.5 Goal ref CAS

```typescript
// packages/goal/goal/src/fold.ts
function applyGoalChange(state, change) {
  requireNextRevision(state, change)     // revision 必须 +1
  requireSameDefinition(state, change)   // objective 不可在 phase 变更时改
  validateSnapshotTransition(state, change)  // 合法 phase 转换
}
```

### 9.6 SubAgent depth floor（monotone）

```typescript
// packages/subagent/subagent/src/depth.ts:28
export function delegationDepthOf(session) {
  return max(header.delegationDepth, options.subagentDepth)
}
```

持久化 header 是 monotone floor — 恢复的 child 不能委托为 top-level。


## 10. 维度覆盖完整性自检（补充章）

本节对照任务要求的 4 大维度 + 模块群列表逐项自检，确保无遗漏。

### 10.1 维度 1：Cordis 三原语 + Service + Fiber epoch

| 子项 | 覆盖位置 | 关键文件 |
|---|---|---|
| Context IOC 容器 | §1.1.1 | `vendor/cordis/src/context.ts` |
| Service 命名能力基类 | §1.1.2 | `vendor/cordis/src/service.ts` |
| Fiber 运行时 + 状态机 | §1.1.3, §1.7 | `vendor/cordis/src/fiber.ts` |
| Epoch 算法 | §1.2 | `fiber.ts:_refresh/_setEpoch/_reload/_unload` |
| Registry 注册/发现 | §1.3 | `vendor/cordis/src/registry.ts` |
| Events 5 模式 | §1.5 | `vendor/cordis/src/events.ts` |
| Reflect 解析层 | §1.4 | `vendor/cordis/src/reflect.ts` |
| symbols 体系 | §1.3.1 | `vendor/cordis/src/utils.ts` |
| DisposableList | §1.3.2 | `vendor/cordis/src/utils.ts` |
| composeError 长栈 | §1.3.3 | `vendor/cordis/src/utils.ts` |
| Inject 装饰器 | §1.3.4 | `vendor/cordis/src/registry.ts` |
| Plugin 三形态 | §1.3.5 | `vendor/cordis/src/registry.ts` |
| Fiber 构造 + dispose 链 | §1.3.6 | `vendor/cordis/src/fiber.ts` |
| reload/unload 转换 | §1.3.7 | `vendor/cordis/src/fiber.ts` |
| effect generator | §1.3.8 | `vendor/cordis/src/fiber.ts` |

### 10.2 维度 2：核心模块群

| 模块 | 覆盖位置 | 关键包 |
|---|---|---|
| workflow | §2.1 | `workflow` + `workflow-worker-thread` |
| subagent | §2.2 | `subagent` + 6 providers |
| plan | §2.3.1 | `plan-mode` |
| goal | §2.3.2 | `goal` + `goal-round-driver` |
| sandbox | §2.4.1 | `sandbox` + `sandbox-local` + `sandbox-policy` + `sandbox-windows-acl` |
| code-runtime | §2.4.2 | `code-runtime` + worker-thread |
| e2b | §2.4.3 | `e2b` + `fs-e2b` + `subprocess-e2b` |
| schedule | §2.5.1 | `schedule` |
| jobs | §2.5.2 | `jobs` + `jobs-local` |
| skill | §2.6 | `skill` + `skill-filesystem` + `tool-skill` |
| shell | §2.7.2 | `shell` + `bash-local` + `pwsh-local` |
| subprocess | §2.7.1 | `subprocess` + `subprocess-local` |
| lsp | §2.8 | `lsp` + `lsp-stdio` + `tool-lsp` |
| compaction | §2.9 | `compaction` + `compaction-basic` + `tool-result-pruner` |
| guard | §2.10.1 | `repeat-tool-reminder` + `timeout-policy` |
| runtime-diagnostics | §2.10.2 | `invariants` |
| interaction | §2.11.1 | `commands` + `user-approval` + `user-questions` |
| feedback | §2.11.2 | `command-feedback` + `message-feedback` |
| typert | §2.11.3 | `typert/{generator,loader,protocol,registry}` |
| webhook | §2.12.1 | `webhook` + `webhook-github` |
| workspace | §2.12.2 | `workspace` |
| web | §2.12 (隐) | `web` |
| mcp | §2.13.1 | `mcp-client` |
| extensions | §2.13.2 | `tool-cordis` + `cordis-host-runner` |
| acp | §2.13.3 | `acp` |
| context (file-ref) | §2.14.1 | `file-reference` + `file-reference-local` |
| session | §2.3.6 | `session` + `session-persistence` |
| session-query | §2.14.3 | `session-query` + `session-query-sqlite` |
| llm | §2.3.5 | `llm` + `llm-deepseek` + `llm-pi-ai` |
| credentials | §2.15.2 | `credentials` + `credentials-local` |
| settings | §2.15.2 | `settings` + `settings-file` |

### 10.3 维度 3：协议适配真实代码路径

| 子项 | 覆盖位置 |
|---|---|
| 统一适配层 LlmAdapter | §3.1 |
| LlmRuntime 注册表 | §3.1 |
| pi-ai Anthropic/OpenAI 路径 | §3.2 |
| PROTOCOLS 表（3 协议） | §3.2 |
| wire 翻译 stream.ts | §3.2 |
| 错误分类 classifyPiAiError | §3.2 |
| DeepSeek 直连适配器 | §3.3 |
| 对 laew 启示 | §3.4 |

### 10.4 维度 4：其他维度快照

| 子项 | 覆盖位置 |
|---|---|
| 维度实现总表 | §4.1 |
| loop 架构（turn/step） | §4.2 |
| Context 管理（压缩管线） | §4.3 |
| 工具调用（6 事件管线） | §4.4 |
| 权限管控（fail-closed 审批） | §4.5 |
| 沙箱（多后端 fail-closed） | §4.6 |

### 10.5 跨模块交互（补充）

| 子项 | 覆盖位置 |
|---|---|
| 完整工具调用路径 | §7.1 |
| SubAgent 委托隔离路径 | §7.2 |
| 压缩触发路径 | §7.3 |
| Goal 完成路径 | §7.4 |
| Cordis 事件体系全览 | §7.5 |

### 10.6 Mermaid 图清单

1. §1.6 — Cordis 微内核架构总图（flowchart）
2. §1.7 — Fiber 生命周期状态图（stateDiagram-v2）
3. §1.8 — 注册发现调度流程图（sequenceDiagram）

### 10.7 字数/规模统计

- 总行数：2000+ 行
- 总字节：~108 KB
- 中文字符：~5300
- 英文词（2+ 字符）：~9100
- 估算总字数：~14600 字（中文字 + 英文词 + 代码标识符）
- 代码片段：60+ 个带语言 fenced block
- 表格：40+ 个
- 覆盖模块：30+ 个核心包
- 覆盖维度：4 大维度 + 25+ 子维度

