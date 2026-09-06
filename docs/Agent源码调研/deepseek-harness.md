# DeepSeek Harness 综合深度分析

> 调研对象:deepseek-harness(TypeScript,Cordis Everything-is-a-Plugin,~80+ 包)
> 调研日期:2026-09-04 ~ 2026-09-06
> 原始文档:8 份(含 2 份补充)
> 总行数:~8,857 行(原始) → ~2,500 行(合并后)

---

## 目录

1. [项目元信息](#1-项目元信息)
2. [Cordis 核心(三原语/Fiber epoch/Registry/Reflect/Events)](#2-cordis-核心)
3. [模块群(30+ 核心模块)](#3-模块群)
4. [流式与中断传播](#4-流式与中断传播)
5. [ACP/A2A 协议矩阵](#5-acpa2a-协议矩阵)
6. [决策溯源与遥测](#6-决策溯源与遥测)
7. [Typert 协议](#7-typert-协议)
8. [记忆与 Context](#8-记忆与-context)
9. [沙箱与权限](#9-沙箱与权限)
10. [配置系统](#10-配置系统)
11. [错误处理与容错](#11-错误处理与容错)
12. [会话持久化](#12-会话持久化)
13. [前端与 native](#13-前端与-native)
14. [多轮对话与循环架构](#14-多轮对话与循环架构)
15. [工具/MCP/Skill 三系统](#15-工具mcpskill-三系统)
16. [对 laew 的借鉴](#16-对-laew-的借鉴)

---

## 1. 项目元信息

| 项目 | 取值 |
|---|---|
| 包名 | `@deepseek-ai/dsh-root`(根)、子包 `@deepseek-ai/dsh-*` |
| 当前版本 | `0.1.2-alpha.1`(developer preview) |
| 仓库命名 | DeepSeek Harness,CLI 命令 `dsh` |
| License | MIT |
| 语言 | TypeScript(`type: module`,ESM 全量)+ 少量 Python SDK + Rust native(Landlock) |
| Node 要求 | `^22.19.0 \|\| >=24.0.0` |
| 包管理器 | pnpm `11.7.0` + workspaces |
| 核心框架 | **Cordis**(vendored,`vendor/`)+ `Schemastery` + `Typert`(自研 RPC 类型图) |
| LLM 适配来源 | 自研 + `@earendil-works/pi-ai`(vendored,作为多 Provider 中间层) |
| 测试栈 | vitest(`test`、`test:e2e`、`test:snapshot`、`test:web`、`test:expected`)、`knip`、`jscpd`、Lefthook、mermaid |
| 文档站 | VitePress(双语 `docs/architecture.md` 等) |
| 入口文件 | `apps/cli/src/bin.ts`(CLI 调度器),`pnpm dsh` / `npx @deepseek-ai/dsh web` |
| 应用 profile | `web`、`headless`、`sdk`、`sdk-minimal`、`acp`(均由 `dsh --profile X` 启动) |
| 质量门槛 | 100% per-file 覆盖率(per AGENTS.md)、knip、publint、双语文档 gates |

**架构标语(README + architecture.md)**:"everything-is-a-plugin",基于 Cordis 的可时空组合(参见 [_A Programming Paradigm for Spatiotemporal Composability_](https://arxiv.org/abs/2608.25512))。没有特权内核,所有扩展(模型适配、工具、会话日志、Agent 循环本身)都是插件。

### 1.1 目录结构

```
deepseek-harness/
├── apps/                        # 应用入口(CLI + Web)
│   ├── cli/                     # dsh CLI 调度器(50行入口 + profile-boot)
│   └── web/                     # Vite + React Web GUI(6行入口)
├── packages/                    # 247 个 npm 包,51 个分组
│   ├── core/                    # 产品 API 脊柱(agent/agent-loop/session/tools/system-prompt/scope)
│   ├── llm/                     # LLM 能力族(llm/llm-deepseek/llm-pi-ai/llm-retry/token-meter)
│   ├── mcp/                     # MCP 客户端(mcp-client)
│   ├── skill/                   # Skill 能力族(skill/skill-filesystem/skill-badge/tool-skill)
│   ├── subagent/                # 子 Agent 能力族(11 包)
│   ├── session/                 # 会话数据平面(16 包)
│   ├── context/                 # 请求上下文(6 包)
│   ├── shell/                   # bash 能力族(local + sandbox + pwsh + tool)
│   ├── terminal/                # 持久化 PTY 能力族
│   ├── fs/                      # 文件系统能力族
│   ├── sandbox/                 # 进程隔离(bwrap / Landlock / Seatbelt / Windows ACL)
│   ├── web/                     # 网络能力(8 包:搜索 + fetch)
│   ├── compaction/              # 上下文压缩
│   ├── goal/                    # 同 session 目标管理
│   ├── plan/                    # plan-mode 协作模式
│   ├── workflow/                # 工作流(ralph)
│   ├── schedule/                # 同 session 计划任务
│   ├── typert/                  # 类型图生成/加载/注册 + protocol
│   ├── acp/                     # Agent Client Protocol 服务
│   ├── extensions/              # 自修改: agent 运行时增删插件
│   ├── bundle/                  # 安装型 profile 补丁层
│   ├── client/                  # Web 浏览器半(40+ 包)
│   └── host/                    # Web 服务端半
├── vendor/                      # vendored 源码(cordis、pi-ai、若干子模块)
├── native/                      # node-addon-landlock-run(Rust/C 源)
├── python/                      # Python SDK + bundled runtime
└── docs/                        # 架构 / 子系统 / 教程 / postmortem
```

### 1.2 Bundle 模型与多端复用

**base 是其它 profile 的依赖** —— `packages/bundle/base/cordis.patch.yml` 写了大几十个 `insert:` 块,`web-app`/`headless`/`sdk-app`/`sdk-minimal`/`acp-app` 在它上面叠加各自的差异补丁。

| 轴 | 共享什么 | 差异什么 |
|---|---|---|
| **能力共享** | base bundle 的所有工具、LLM 适配、Session持久化、Guard | 仅加差异 patch |
| **平台隔离** | tools / llm / shell / compaction 在 node 与 web 都用同一份 | webserver / client-hmr / ui-* 只在 web 跑 |
| **部署覆盖** | `cordis.yml` 的 `id: X` 行被 CLI `--patch` 文件覆盖 | `--patch file.yml` 是最后叠加层 |

**根配置永远是空数组 `[]`,所有内容都是 patch 层叠加** —— 这是 "everything is a patch" 的极致体现。

**4 层 patch 叠加 + HMR 监听**:
1. Bundle layers(`dsh.profile.bundles`)
2. Profile 自身 patch(`cordis.patch.yml`)
3. `--patch` CLI 叠加
4. Home patch(`$DSH_HOME/cordis.patch.yml`)

---

## 2. Cordis 核心

Cordis 是整个 harness 的「操作系统内核」。所有能力都以 **Service** 形式注册到 **Context** 这个 IOC 容器上,**Fiber** 是每个 plugin 的运行时实例,**Registry** 负责 plugin 的注册/发现/调度。三者合称「Cordis 三原语」。

### 2.1 Context — IOC 容器 + 隔离作用域

**文件**:`vendor/cordis/src/context.ts`

Context 是带原型链继承的 **Proxy 容器**:

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

三个关键子上下文操作:

- `extend(meta)` — 创建子 context(原型继承,不污染父)
- `isolate(name, label?)` — 创建独立 service 作用域(用于 SubAgent 隔离)
- `intercept(name, config)` — 注入 service 拦截配置

### 2.2 Service — 命名能力单元

**文件**:`vendor/cordis/src/service.ts`

```typescript
// vendor/cordis/src/service.ts:11-59
export abstract class Service<out T = never> {
  static readonly init: unique symbol = symbols.init
  static readonly check: unique symbol = symbols.check
  static readonly config: unique symbol = symbols.config
  static readonly invoke: unique symbol = symbols.invoke
  static readonly extend: unique symbol = symbols.extend
  static readonly tracker: unique symbol = symbols.tracker
  static readonly resolveConfig: unique symbol = symbols.resolveConfig

  constructor(protected ctx: Context, name: string) {
    name ??= this.constructor['provide'] as string
    let self = this
    if (self[symbols.invoke]) {
      self = createCallable(name, joinPrototype(Object.getPrototypeOf(this), Function.prototype), tracker)
    }
    self.ctx = ctx
    self.name = name
    self.ctx.reflect.provide(name, self, this[symbols.check])
    return self
  }
}
```

Service 通过 `provide` 自动暴露到 `ctx` proxy,`invoke` 符号让 Service 实例可被当作工厂函数调用(这是 `ctx.logger('x')` 拿命名 logger、`ctx.agent(...)` 派生新 agent 的基础)。

**拦截配置合并**(`resolveConfig`)沿原型链收集所有祖先 context 对同一 service 的 `intercept` 配置,按「base → 祖先 intercept → head」顺序合并。

### 2.3 Fiber — Plugin 运行时实例 + Epoch 算法

**文件**:`vendor/cordis/src/fiber.ts`

**生命周期状态机**:

```typescript
// vendor/cordis/src/fiber.ts:147-154
export const enum FiberState {
  PENDING,    // 等待依赖 service 可用
  LOADING,    // plugin callback 正在执行
  ACTIVE,     // 已加载、正在提供服务
  FAILED,     // 配置校验或启动抛错
  DISPOSED,   // 已卸载,不可重启
  UNLOADING,  // 正在运行 disposers
}
```

**Fiber Epoch 算法**(核心创新):

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

算法要义:
1. **epoch 字符串** = 当前 fiber 所有依赖 service 的提供 fiber 的 uid 拼接(`:uidA:uidB`)
2. 当任一依赖变化(卸载/重新加载),`_refresh()` 重新计算 epoch
3. `_setEpoch()` 比较新旧 epoch:旧=INACTIVE + 新≠INACTIVE → `_reload()`(启动);旧≠INACTIVE + 新=INACTIVE → `_unload()`(卸载);相同 → 不做任何事

### 2.4 Registry — Plugin 注册/发现/调度

**文件**:`vendor/cordis/src/registry.ts`

支持三种插件形态:

```typescript
// vendor/cordis/src/registry.ts:92-133
export type Plugin<T = any> =
  | Plugin.Function<T>    // (ctx, config) => any
  | Plugin.Constructor<T> // new (ctx, config) => any
  | Plugin.Object<T>      // { apply(ctx, config) }
```

**关键设计**:同一 plugin callback 对应一个 `Runtime`(共享 Config / fibers 列表),但每次 `ctx.plugin()` 调用产生 **独立 Fiber**(独立 config / 独立 dispose)。

### 2.5 Events — 5 种调度模式

**文件**:`vendor/cordis/src/events.ts`

```typescript
export type DispatchMode = 'emit' | 'parallel' | 'serial' | 'bail' | 'waterfall'
```

| 模式 | 语义 | 典型用途 |
|---|---|---|
| `emit` | 同步触发,不等待返回 | 通知 |
| `parallel` | 并发等待所有 listener | 广播 |
| `serial` | 顺序 await,遇 bail 值停止 | 串行策略链 |
| `bail` | 同步遇非 null/false/undefined 即停 | 配置转换拦截 |
| `waterfall` | 最后参数是 `next`,不调用 = veto | middleware 链 |

**listener 自动随 fiber 卸载**:`on()` 内部调用 `fiber.effect()`。

### 2.6 Reflect — Service 解析层

**文件**:`vendor/cordis/src/reflect.ts`

Reflect 是 Context Proxy 背后的「服务定位器」。`get` trap 的解析链:特殊属性 → target 自身 → accessor → 根 fiber 宽松读取 → waterfall('internal/get') → 沿 fiber 链向上查找 `store[name]` → 检查 inject 需求。

**Mixin 机制** — 把服务方法直接挂到 `ctx` 上:

```typescript
this.mixin('reflect', ['get', 'set', 'provide', 'accessor', 'mixin'])
this.mixin('fiber', ['runtime', 'effect'])
this.mixin('registry', ['inject', 'plugin'])
this.mixin('events', ['on', 'once', 'parallel', 'emit', 'serial', 'bail', 'waterfall'])
```

### 2.7 symbols 体系与 DisposableList

**文件**:`vendor/cordis/src/utils.ts`

Cordis 使用全局 Symbol 作为内部 key:`shadow`、`receiver`、`original`、`initHooks`、`checkProto`、`effect`、`filter`、`isolate`、`intercept`、`init`、`check`、`config`、`invoke`、`extend`、`tracker`、`resolveConfig`。`Symbol.for()` 的全局注册确保跨 realm 场景下 brand 一致。

**DisposableList** — O(1) 删除的有序集合,是 Fiber 的 `_disposables` 核心数据结构:`push` 返回 disposer(单步),`clear` 逆序返回(用于卸载),weak map 保证 O(1) by-value 删除。

### 2.8 Effect 系统与 Inject 装饰器

**Effect** 接受同步/异步 disposer、同步/异步 generator(generator 允许注册多个独立跟踪的 disposers,每个 yield 立即生效,卸载时逆序清理)。

**`@Inject` 装饰器** 在 class 上累积静态 `inject` map;在 method 上延迟方法调用直到声明的 services 可用。

### 2.9 跨专题设计模式总结

| 模式 | 说明 |
|---|---|
| Effect-Scoped 资源管理 | 注册/监听/连接都是 effect-scoped,disposer 自动运行在 fiber 卸载时 |
| Capability Seam 三角色 | Service 定义、Provider 注册、消费者三层分离 |
| Event-Sourced Projection | Goal 和其他域的状态通过纯 fold session 事件重建 |
| Fail-Loud 配置验证 | 所有配置通过 schemastery 验证,不匹配直接抛异常 |
| Waterfall 协作 | Guard 通过 waterfall 事件链协作 |
| CAS 乐观并发 | Goal 域的每次变更都要求提供当前 ref,不匹配则拒绝 |
| Serialized Driver Loop | "requested flag + single runner" 模式 |
| Last-Wins Whole-Value Rule | session 事件携带完整快照而非增量 |
| Scoped Shadow | isolate/intercept 让同一插件在不同子树有不同行为 |

---

## 3. 模块群

### 3.1 workflow — DAG 定义与执行引擎

**定位**:模型编写的 JavaScript 编排脚本运行时(非静态 DAG,是动态调用图)。  
**关键包**:`packages/workflow/workflow`(seam)+ `workflow-worker-thread`(引擎)+ `tool-workflow`(模型工具)。

workflow 不是「静态节点/边 DAG」,而是 **模型编写的 JS 脚本**(top-level await,以 `return <json>` 结束),DAG 是脚本执行 `agent()` 调用时产生的 **动态调用图**。

`WorkerThreadWorkflowEngine` 在 **Node worker thread + `node:vm` 上下文** 中执行每个脚本(offload 同步工作 + 强制终止能力,是 containment 而非安全边界)。

`WorkflowExecution` 注入脚本 API:
- `agent(prompt, opts)` — 子 agent = 一个节点
- `parallel(thunks)` — fan-out 屏障(Promise.all)
- `pipeline(items, stages)` — per-item stage chain
- `phase(title)` — 观察性进度分组
- `log(message)` — 叙事行

**错误双层纪律**:Fatal `WorkflowError` 总是杀死脚本;非 fatal 子失败 → 每项 `null`。

**配置**:`maxConcurrentAgents` 默认 `min(16, max(1, cores-2))`,`maxTotalAgents`=1000,`maxItemsPerCall`=4096,`disposeGraceMs`=5000。

### 3.2 subagent — SubAgent 生成/调度/上下文隔离

**定位**:多 provider 的 SubAgent 运行时 + 可续生命周期。  
**关键包**:`packages/subagent/subagent`(seam)+ 6 个 provider 后端。

两种形态:
- **One-shot**:`SubagentRuntime.start(name, request)` → `SubagentRun`
- **Continuable**:`startContinuable(spec)` → 可续会话(`SubagentContinuationManager` 拥有 `AgentHandle`)

**能力声明**(capabilities flags):

```typescript
interface SubagentCapabilities {
  agentOptions: boolean; outputSchema: boolean; depthLimit: boolean
  toolFilter: boolean; persona: boolean
}
```

**内置 SubAgent 后端**:

| 后端 | 包 | 特点 |
|---|---|---|
| `fork` | `subagent-fork-in-process` | 继承父 session 上下文(完成轮次前缀) |
| `spawn` | `subagent-spawn-in-process` | 独立新 Agent |
| `acp` | `subagent-acp` | Agent Communication Protocol |
| `codex` | `subagent-codex` | Codex 协议桥接 |
| `claude-code` | `subagent-claude-code` | Claude Code 子进程 |
| `dsh-sdk` | `subagent-dsh-sdk` | DSH SDK JSON-RPC |

隔离通过 **Cordis scoped composition** 实现:`applyChildComposition` 在子 scope 安装 persona + tool restriction。

**resident vs cold resume**:可续子代理可以是「驻留」(Agent 在内存中)或「冷」(已释放,Session 在磁盘)。`followup` 自动判断:驻留则直接投递 inbox,冷则从持久化恢复。

**depth 管理**:每个 Agent 通过 session header 记录其在委托树中的深度,`maxDepth` 参数限制子代理深度(全局 MAX=5)。

**控制工具**:`send_message`、`interrupt_agent`、`report`(子代理向上汇报)。

### 3.3 plan + goal — 目标规划与任务拆解

#### plan — 协作模式

**关键包**:`packages/plan/plan-mode`。plan 是 **plan mode vs default mode 的协作模式**,不是 Plan/Step 任务分解。状态完全事件源化(`plan/mode` 事件)。

```typescript
class PlanModeController extends Service {
  foldPlanMode(events)          // 纯 last-wins fold of plan/mode → boolean
  set(agent, active)            // committed/queued/cancelled/noop
}
```

#### goal — 事件源化的目标状态机

**关键包**:`packages/goal/goal` + `goal-round-driver` + `tool-goal`。

```typescript
type GoalPhase = 'active' | 'paused' | 'blocked' | 'complete'
interface GoalSnapshot { id, revision, objective, phase, blockedReason?, maxGoalRounds }
type GoalOperation = 'create'|'edit'|'pause'|'resume'|'complete'|'block'|'clear'
```

Goal 是 **事件源**(event-sourced)模型:每次变更都写入 `goal/change` session 事件,当前状态通过 fold 事件流重建。

**round driver 自动续轮机制**:当 armed active goal 的 agent 空闲时,driver 预留下一轮,渲染 `renderGoalRoundPrompt(goal, round)`,`followup()` 一个 `GoalMessageSource` 消息。

**maxGoalRounds 双层保障**:
1. `GoalService.resume()`:恢复时检查 `roundsStarted >= maxGoalRounds`
2. `round driver.drive()`:每轮开始前检查,达到限制则 `block('round-limit')`

**tool-goal 三个工具**:`get_goal`(读)、`create_goal`(创建)、`update_goal`(编辑/暂停/恢复/完成/阻塞)。`authority.ts` 区分人类调用 vs 自动续轮调用。

### 3.4 sandbox + code-runtime + e2b — 沙箱隔离

#### sandbox — 多后端约束

**关键包**:`packages/sandbox/sandbox`(seam)+ `sandbox-local` + `sandbox-policy` + `sandbox-windows-acl`。

```typescript
type SandboxMode = 'read-only' | 'workspace-write' | 'danger-full-access'
```

**平台 runner 链**:`{ linux: ['bwrap','landlock'], darwin: ['seatbelt'], win32: ['windows-acl'] }`

**fail-closed 升级**:`WIDER_MODES` 映射每个 mode 可扩展到的范围,`approveEscalation()` 检查严格扩展 → 解析 `ctx.approval`。

#### code-runtime — 代码执行

**关键包**:`packages/code-runtime/code-runtime` + `code-runtime-worker-thread` + `code-runtime-python`。

```typescript
abstract class CodeRuntime extends Service {
  abstract language: 'typescript' | 'python'
  abstract isolation: 'worker-thread' | 'process' | 'container'
  abstract run(request: CodeRunRequest): Promise<CodeRunResult>
}
```

#### e2b — 远程沙箱

**关键包**:`packages/e2b/e2b` + `fs-e2b` + `subprocess-e2b`。一个共享远程 Linux sandbox。

### 3.5 schedule + jobs — 调度与作业

#### schedule — agent-scoped 持久提醒

三种提醒:`after`(延迟) / `at`(绝对) / `every`(fixed-rate,anchor-aligned,min 5min)。到期时 `runMaintenance` → 渲染 framing → `agent.followup`。

#### jobs — 后台作业

```typescript
type JobStatus = 'running' | 'stopping' | 'completed' | 'killed' | 'failed'
```

`LocalJobRegistry.start()`:`preflight controller admission` → `maxConcurrentJobsPerOwner`(default 10) → 原子注册。

### 3.6 skill — Skill 注册/加载/触发

**关键包**:`packages/skill/skill`(seam)+ `skill-filesystem` + `tool-skill` + `skill-badge`。

```typescript
type SkillSource = 'project-dsh'|'project-agents'|'runtime'|'user-dsh'|'user-agents'|'custom'|'bundled'
interface SkillInvocationPolicy { modelInvocable: boolean; userInvocable: boolean }
```

**FileSystemSkillProvider** 从 ranked roots 发现:project `.dsh/skills`(rank 100)→ `.agents/skills`(200)→ custom(300)→ user `~/.dsh/skills`(400)→ `~/.agents/skills`(500)→ bundled(600)。`SkillWatchManager` 保持有界 Chokidar host watchers。

**触发双路径**:模型调用 `skill` 工具(仅 model-invocable);用户 `/name` gesture 注入渲染 body(user-invocable)。**仅 1 个 bundled skill**(`skill-badge`)。

### 3.7 shell + subprocess — Shell 执行与进程管理

#### subprocess — 进程树原语

`spawnSubprocess()`:detached process-tree spawn,`OutputCollector` — bounded tail-keep + lazy spill file(random + O_EXCL + `0600` path)。`terminate()` = SIGTERM → graceMs → SIGKILL。

#### shell — bash seam

`LocalBashExecutor`:public commands 作为 `bash -c` 通过 `ctx.subprocess` 运行。默认 timeout 120s / maxTimeout 600s / maxOutputBytes 64KB / maxSpillBytes 64MB / graceMs 3s。

### 3.8 lsp — LSP 集成

**关键包**:`packages/lsp/lsp`(seam)+ `lsp-stdio`(通用后端)+ `tool-lsp`。

`LocalLspProvider`:per-canonical-workspace 一个 `LspInstance`,per-workspace 序列化。`tool-lsp` 暴露只读 `lsp` 工具(`goToDefinition`/`findReferences`/`goToImplementation`/`hover`)。

### 3.9 compaction — 上下文压缩

**关键包**:`packages/compaction/compaction`(seam)+ `compaction-basic` + `command-compact` + `compaction-tool-result-pruner`。

```typescript
type CompactionTrigger = 'pressure' | 'context-overflow'
abstract class CompactionEngine extends Service {
  abstract compactIfNeeded(agent, trigger, signal): Promise<CompactionResult | null>
  abstract compactNow(agent, signal): Promise<CompactionResult | null>
  abstract compactRegion(start, end, agent, signal): Promise<CompactionResult>
}
```

**Pipeline**:(1) 可选 model-free tool-result pruning;(2) `selectCompactableRange` 选择 balanced inclusive span;(3) `compactSurfaceRegion` 通过 `ctx.llm.stream()` 重放 prefix → summarize;(4) 替换 span 为一个 summary user message;(5) durable `compaction/start`…`compaction/end` marker pair(压缩锁)。

**token 预算管理**:`thresholdRatio`(如 0.8)和 `retainRatio` / `retainTokens` 控制。多模型策略:每个 provider/model 可独立配置(可用便宜模型做摘要)。

### 3.10 guard + runtime-diagnostics

#### guard — 两个轻量 guard

**repeat-tool-reminder**(advisory,从不 veto):per-agent consecutive-repeat `Chain { key, count }`,`canonicalize(arguments)` = deep key-sort JSON。默认阈值 `[3, 5, 8]`,用户干预重置。

**timeout-policy**(cooperative enforcer):wraps `tools/execute`,读取 tool 定义的 `timeoutMs`,`deadline(exec.signal, timeoutMs, TOOL_TIMEOUT)`,到期替换为结构化 `TOOL_TIMEOUT` 错误。

#### runtime-diagnostics — 不变量注册表

`InvariantRegistry`:`register(packageName, installer)` 在 child fiber 运行 installer,fail 回调抛 `InvariantError`。

### 3.11 interaction + feedback + typert

#### interaction — 用户交互

**关键包**:`packages/interaction/commands` + `user-approval` + `user-questions` + `tool-ask-user` + `permission-presets`。

```typescript
class ApprovalService extends Service {
  request(req): Promise<ApprovalOutcome>
  decide(req, session)
}
type ApprovalOutcome = 'allowed-once' | 'rejected' | 'cancelled' | 'unavailable'   // fail-closed
```

`approval/policy` 持久化,per-session `ask`/`never` policy。`permission-presets` 捆绑 sandbox-mode + approval-policy 为 named presets。

### 3.12 webhook + workspace + web

#### webhook — 规则注册表 + 分发

**关键包**:`packages/webhook/webhook` + `webhook-github`。接收由 provider adapters 完成(GitHub handler 验证 `x-hub-signature-256`),HTTP handler 在 rule settle 前响应 202,fire-and-forget。

#### workspace — 多根工作区注册表

`WorkspaceRegistry`:Session membership 按 canonical-cwd 相等性过滤;操作通过 `enqueueOperation` 序列化;`pendingMutation` markers 使 create/delete crash-recoverable。

### 3.13 mcp、extensions、acp

#### mcp — MCP 客户端桥

**关键包**:`packages/mcp/mcp-client`。

```typescript
const name = 'mcp-client'; const inject = ['tools']
type Config = StdioConfig | StreamableHttpConfig
```

命名空间 `mcp__<serverName>__<rawName>`,归一化到 64-char `[A-Za-z0-9_-]` 契约。`syncTools` 通过 `tools/list` 发现 + 注册;`ToolListChangedNotification` 触发 re-sync。指数退避重连(500ms → 30s,10 attempts)。

#### extensions — Cordis 动态插件

`DynamicCordisRunnerService`:`define` / `undefine` / `run` / `stop` / `inspectPlugin` / `inspectPackage` / `reference` / `snapshot` / `inventory` / `invoke` / `settleUserRun`。Host half 在 VM sandbox 运行;Client half 在浏览器运行;activation 需要人工 approval。

### 3.14 context、session、session-query

#### context — file-reference 发现

`FileReferenceService`:per-agent `WorkspaceFileSearch`,监听 `agent/created`/`agent/disposed`/`session/event`(tool/result 时 invalidate)。

#### session — 事件源化会话日志

**关键包**:`packages/session/session` + `session-persistence` + `session-persistence-sqlite`。

Session 是 **append-only 事件源化日志**(`turn/start`/`turn/end`/`step/start`/`step/end`/`user/message`/`assistant/chunk`/`tool/call`/`tool/result`)。`append` 仅在持久化后 resolve。

#### session-query — 全文搜索

`SqliteSessionQueryEngine`:SQLite FTS5,live-preferred,调和 live + persisted observations → FTS tables,分页通过 opaque `CursorPayload`(base64url JSON)。

### 3.15 llm、credentials、settings

#### llm — Provider-neutral 适配器注册表

```typescript
abstract class LlmAdapter {
  abstract stream(options: GenerateOptions): AsyncIterable<StreamChunk>
}
class LlmRuntime extends TypertRemoteService {
  registerAdapter(providers, adapter)
  prepareCall(config, signal): PreparedLlmCall
  stream(options)
}
```

**pi-ai provider** 支持 3 种 wire protocol:`openai-completions` / `openai-responses` / `anthropic-messages`。错误分类 pattern-matching pi-ai 错误文本 → Harness 标准码(AUTH / QUOTA_EXCEEDED / RATE_LIMIT / INVALID_REQUEST / SERVER / TRANSPORT)。

**DeepSeek 直连适配器**:direct-fetch + SSE 针对 OpenAI-compatible `/chat/completions`,connection facts 通过 thunk 单次解析,bearer token 通过 `resolveApiKey` 每次请求解析。

---

## 4. 流式与中断传播

### 4.1 流式协议翻译管线

**SSE Chunk 解析** — `packages/llm/llm-deepseek/src/sse.ts`:`parseSse(stream, onComment?)` 用 `eventsource-parser/stream`,产出 `[DONE]` sentinel,EOF 未得则抛 `LlmError('STREAM_CLOSED')`。

**StreamChunk 类型** — `packages/llm/llm/src/types.ts`:`'block-start' | 'text-delta' | 'reasoning-delta' | 'tool-call-delta' | 'block-end' | 'usage' | 'finish'`。不相交计数约定(`inputTokens` 排除 cache hits)。

**翻译管线** — `packages/llm/llm-deepseek/src/translate.ts`:`translate(payloads)` 消费 SSE data payload,按 content/reasoning/tool-call index 维护 `OpenBlock` 状态,`mapUsage` 从 prompt_tokens 减去 cache hits。

**BlockAssembler** — `packages/llm/llm/src/assembler.ts`:增量 chunk→message 组装器,对 delta-only 协议容忍(已关闭 index 的 delta 到达时被忽略),`interruptedBlocks()` 在流被 cancel 切断时返回部分内容。

**`llm/stream` waterfall** — `packages/llm/llm/src/index.ts:54-70`:通用流式拦截器,consumer-driven 背压,每个监听器可以短路(否决)、包裹或替换流。

### 4.2 中断 / Abort 传播

**`AbortSignal` 是通用货币**:

- `ReactLoopAgent.cancel()`:`phase.abort.abort(cause)`,阶段机(`Phase`)在 `maintenance` 和 `running` 都携带 `abort: AbortController`
- `AgentCancelCause`:`'user' | 'parent' | 'hook' | 'disposed'`
- ACP session `cancelPrompt`:abort `admissionController`,若消息已排队则传播 `agent.cancel({kind:'user'})`
- `SubprocessSpawnSpec.signal` 触发时启动进程树的 terminate 升级

**信号融合用 `AbortSignal.any`**:见于 `llm-deepseek/src/adapter.ts:476`、`llm-retry/src/index.ts:124`。

**SIGINT / Ctrl-C 传递**:PTY 层向前景进程组传递 SIGINT(Windows 用 `\x03` 输入写),用户可见的中断路径通过 `AbortSignal` → `operation.cancel()` → `this.interrupt(operation)` 接线。

**流式侧 abort**:通过 `signal.throwIfAborted()` 在每个步骤边界传播,`adapterFailureChunk()` 把任何 adapter throw 转为终态 `finish` chunk。

**ACP session 取消**:`AcpSession.close(detail)` = cancelPrompt → await admissionDone → await agent.whenIdle() → await outputTail → drainContinuableDescendants → flush → disposeAgent。

### 4.3 流式背压 / 缓冲策略

Cordis 本身**无**内置背压原语 —— 事件总线要么同步要么基于 await。背压在 consumer 层用三种策略:

**OutputCollector** — `subprocess/subprocess-local/src/spawn.ts:104-251`:有界 tail + spill 文件,内存 TAIL 到 `maxBytes`,可选整流 spill 到文件(懒打开,随机后缀 + O_EXCL + 0o600),无消费 offset-based 读取器(`readFrom(fromByte)`)。

**BoundedTextBuffer** — `terminal/terminal-bash/src/session.ts:45-80`:双上限 `maxLines AND maxBytes`,`utf8Tail` 保留完整字符。

**cancellableDelay 重试背压** — `llm/llm-retry/src/index.ts`:重试等待通过 `AbortSignal.any([signal, lifetime.signal])` 中止,plugin dispose abort `lifetime` 并排空所有 active 恢复。

---

## 5. ACP/A2A 协议矩阵

### ACP(Agent Communication Protocol)完整实现

**包**:`packages/acp/acp/`

- `apply(ctx, config)` — 通过 `@agentclientprotocol/sdk` 在 JSON-RPC stdio 上挂载仅自动化 ACP server
- Methods:`initialize`、`authenticate`、`session.new`、`session.list`、`session.resume`、`session.close`、`session.setConfigOption`、`session.prompt`、`session.cancel`
- `AcpSession`:Per-session ACP module 拥有未发布 Agent 组合、选定路由、one-prompt 准入槽(`InflightPrompt`)、有序标准更新、memoized 静止拆卸
- `onSessionEvent()` 把持久事件投影为 ACP 更新:`assistant/message` → `assistantUpdates`、`tool/call` → `toolCallUpdate`、`tool/result` → `toolResultUpdate`
- `turnEndToStopReason(reason)`:把 harness turn 结束映射为 ACP 终态原因(`end_turn`, `max_tokens`, `cancelled`)
- `mountAcpMcpServers()`:把标准 ACP MCP-server 声明翻译为 Agent-scoped DSH MCP clients

### A2A / E2A / A2UI

**未找到专用 A2A/E2A/A2UI 包** — 最近似物:

- `packages/experimental/agent-team/src/mailbox.ts`:`TeamMailbox` — 持久 Team mailbox 准入、target-local dispatch、acknowledgement、recovery
- `packages/experimental/agent-team/src/types.ts`:`TeamMessageSnapshot`,SessionEventMap 合并:`'team/member'`, `'team/task'`, `'team/message/queued'`, `'team/message/delivered'`
- `packages/experimental/agent-team/src/journal.ts`:`TeamJournal` — 在活跃 Lead Session 日志上序列化 Team 事务

---

## 6. 决策溯源与遥测

### 6.1 SessionEventMap — append-only 真相源

**文件**:`packages/core/session/src/types.ts`

关键事件:`turn/start`、`turn/end`(带 `TurnEndReason`)、`step/start`、`step/end`、`user/message`、`assistant/chunk`(原始 stream chunk 用于回放保真)、`assistant/message`(组装后带 usage + interrupted flag,`sourceEventSeqs: chunkSeqs` 从 chunk 到 message 的因果链)、`tool/call`、`tool/result`、`request/header`(完整 EpochHeader 快照)、`request/context`、`session/end-seed`。

`EpochHeader`:call config, adapterDefaults, system, tools。`TurnEndReasonMap`:`'completed' | 'aborted' | 'blocked' | 'error' | 'max-tokens' | 'interrupted'`。

### 6.2 OTLP Telemetry

**文件**:`packages/session/session-telemetry-otel/src/index.ts` — `OpenTelemetrySessionBackend` 组合 OTel JS SDK:`LoggerProvider` + `BatchLogRecordProcessor` + `OTLPLogExporter`。`SessionTelemetryMode`:`FULL | FEEDBACK_ONLY | DISABLED`。

**文件**:`packages/session/session-telemetry/src/coordinator.ts` — `SessionTelemetryCoordinator` 实时捕获订阅 session firehose + `agent/error` 中继,应用固定 chunk 投影(仅每个 (turn, step) 的首 chunk 外发),`session-telemetry/child` waterfall 用于脱敏。

### 6.3 不可篡改存储

**文件**:`packages/session/session-persistence/src/coordinator.ts` — `PersistenceCoordinator` 编排缓冲、序列化、采纳、修复、处置。Per-session 序列化(promise chain)、append-only contiguous-seq 契约、crash repair(torn-tail 截断 + 合成 closer)。

**文件**:`packages/session/session-log-deepseek/src/index.ts` — `acceptedThrough()` session-log 上传的最高确认序列,通过 `'session-log-deepseek/delivery-accepted'` event 确认水位。

### 6.4 Projection 系统(可视化数据层)

**文件**:`packages/session/session-projection/src/index.ts` — `SessionProjectionRegistry`:合并可扩展的状态驱动计算单元。`ProjectionDefinition`(key, stateSchema, init, apply, wire view, stateVersion),在已提交事件上急切驱动、watermark cache、change feed。

`restoreFloor()` 计算需要重放的最早 seq;`restore` 从 checkpoint + 事件尾部重建状态;`hydrate` 将恢复结果安装到活 Session。**one-below anchor** 设计精妙:返回 `floor - 1` 而非 `floor`,让持久层从 `floor - 1` 开始读取,检测日志是否缩小到了 checkpoint 的 watermark 以下(crash-repair truncation)。

---

## 7. Typert 协议

Typert **不是**有线协议。它是用于生成 Remote 方法产物的**代码生成 + 运行时注册表**(TypeScript 类型驱动的 RPC schema)。实际有线载体(ACP 用 ndJSON over stdio、LLM 用 fetch)在别处。

### 7.1 协议入口 — decorator 与 binding

**文件**:`packages/typert/protocol/src/index.ts`

- `@Remote` decorator 把方法标记为直接 Remote 调用;`@Remote({mode:'stream'})` 标记为逻辑流
- `@RemoteScope(key)` 从一个 Remote Scope 解析方法
- `bindTypertRemote(service, serviceKey, options)` 把 Service 绑定到 Typert Gateway 命名空间

取消通过 TS decorator 元数据注入为**最后一个参数** — `cancellation?: { parameter: 'signal' }`。

### 7.2 Endpoint 语法与 InvocationDescriptor

Endpoint 正则 `^[A-Za-z0-9_$.-]+$`。`InvocationDescriptor` 是载体无关的,包含 id/service/namespace/method/invocation(direct 或 context)/parameters/result(codec 有 strict 与 `src-json` 两种模式)/cancellation。

### 7.3 注册表 — 四个 store

**文件**:`packages/typert/registry/src/service.ts` — `TypertRegistry` 暴露四个子 store:`local`、`remotes`、`lookups`、`contexts`。注册是原子 & 重复拒绝,`ChangeSource` 同步通知观察者(每个监听器 try/catch 保证一个坏观察者不能否决变更)。

### 7.4 Loader — 从 `./typert` 导出自动注册

`TYPERT_HOST_EXPORT = './typert'`,`validateTypertManifest` 验证结构,`apply()` 订阅 `internal/plugin`,Reconciliation 是 microtask-batched,按 entry name 增量(dirty set + flushQueued flag)。

### 7.5 实际有线帧(ACP)— ndJSON over stdio

**文件**:`packages/acp/acp/src/index.ts:372-375`:

```typescript
const stream: Stream = config.stream ?? ndJsonStream(
  Writable.toWeb(process.stdout) as WritableStream<Uint8Array>,
  Readable.toWeb(process.stdin) as ReadableStream<Uint8Array>,
)
```

---

## 8. 记忆与 Context

### 8.1 Session 作为唯一可信源

**文件**:`packages/core/session/src/index.ts` 1156 行 — `Session` 主类、`append`、`deriveMessages`。会话是 **append-only `SessionEvent` 流**,模型可见内容必须能从 log 重建(`Model-visible ⟺ Logged` 不变式)。`SessionEventMap` 是 merge-extensible 的 declaration-merge map,新事件要求严格构建期类型。

### 8.2 Surface 层:模型可见视图

**文件**:`packages/core/surface/src/surface.ts` — `SurfaceManager` 维护模型可见的有序视图,通过 fold 事件流生成:`append` 追加新事件;`{ op: 'replace', start, end }` 替换事件范围(Compaction 使用)。

### 8.3 Context 注入

- `packages/core/context/`(6 包):`agent-instructions`(工作区 AGENTS.md)、`file-reference`、`file-reference-local`、`session-reference`、`time-context`、`tmux-context`。每个都是一个 Cordis 插件,在 `systemPrompt.assemble()` 阶段被收为 prompt 段落。
- `packages/core/system-prompt/src/index.ts`(~596 行)实现 `PromptAssembly`、`renderPrompt`、`joinContextSections`。

### 8.4 上下文投影单元

`SessionProjectionRegistry` 是事件源投影框架:`ProjectionDefinition` 注册纯 fold 框架在每个 session event 提交后自动驱动。引用相等性优化(`Object.is`)跳过不关心的通知。

### 8.5 Checkpoint 策略

`packages/session/session-checkpoint-policy`:三个检查点确保崩溃后不丢失已完成的工作 —— 在模型请求到达 adapter 前、在顶层 tool body 可产生外部副作用前、在每个 step 边界。

---

## 9. 沙箱与权限

### 9.1 三态沙箱模式

| 模式 | 效果 |
|---|---|
| `read-only` | 拒绝写入(除 /dev/null 等必需 sink) |
| `workspace-write` | 允许在 workspace root 下写入 |
| `danger-full-access` | 绕过沙箱限制 |

**失败关闭**:当请求的模式无法强制执行时,调用以 `SANDBOX_UNAVAILABLE` 错误失败,而不是静默运行在无限制状态。

**升级机制**:被拒绝的调用可以请求更宽的模式(`sandbox_permissions`)加上 `justification`,用户看到一次审批提示。

**与 native/landlock-run 的关系**:`sandbox-local` 使用 `probe()` 决定是否使用 Landlock → 失败则检查 bwrap → 都失败则 `SANDBOX_UNAVAILABLE`。

### 9.2 权限管控 — fail-closed 审批

`ApprovalService`:`request(req)` 要求 open turn,追加 `approval/asked` → `decide` → 追加 `approval/decided`。`ApprovalOutcome = 'allowed-once' | 'rejected' | 'cancelled' | 'unavailable'`。

`approval/policy` 持久化,per-session `ask`/`never` policy。`permission-presets` 捆绑 sandbox-mode + approval-policy 为 named presets。

### 9.3 工具超时

`timeout-policy`:wraps `tools/execute`,读取 tool 定义的 `timeoutMs`,`deadline(exec.signal, timeoutMs, TOOL_TIMEOUT)`,到期替换为结构化 `TOOL_TIMEOUT` 错误结果。

### 9.4 Hook 桥接

**关键包**:`packages/hooks/hook-protocol` + `hooks-claude-code` + `hooks-codex`。

Hook 能力:**阻止操作**(exit code 2)、**附加上下文**(hook 返回的额外文本在下一次请求中模型可见)、**请求确认**、**请求停止**(`{"continue": false}`)。合并优先级:`deny > ask > allow`。

---

## 10. 配置系统

### 10.1 凭证分层

`packages/credentials/credentials` + `credentials-local` + `authorization`。解析值 = inherited process env(read-only, wins) > `.credentials.yaml` > `<cwd>/.env` > `$DSH_HOME/.env`。API key 引用机制 — 存储一次,通过名称引用,轮换后下一个请求生效。

### 10.2 Settings 分层

`packages/settings/settings` + `settings-file`。写接受 `expectedRevision` 冲突检测(`SettingsConflictError`, code `SETTINGS_CONFLICT`)。解析值 = schema defaults → composition base → user document section。

### 10.3 通用存储抽象

`Storage` hub + `BackendRegistry`。`StorageForms` 用 declaration-merge 模式,注册是 effect,mount/register 都返回 disposer。Stale disposer 防御:dispose + re-register 后,旧 disposer 不能干掉新 backend。

---

## 11. 错误处理与容错

### 11.1 LLM 错误分类

**pi-ai provider** pattern-matching pi-ai 错误文本 → Harness 标准码:`AUTH` / `QUOTA_EXCEEDED` / `RATE_LIMIT` / `INVALID_REQUEST` / `SERVER` / `TRANSPORT`。pi-ai 把 caught error 扁平化为 `error.message`,丢弃原始 Error + `cause` 链(undici 的 transport detail 在 `cause` 上),只剩 pattern-matching terse words。

### 11.2 LlmRetry — 指数退避 + 可取消

`llm/llm-retry` 作为可叠加的 `LlmRuntime` 装饰器。`cancellableDelay(delayMs, signal)` + `AbortSignal.any([signal, lifetime.signal])`。Plugin dispose abort `lifetime` 并排空所有 active 恢复。

### 11.3 重复工具提醒

`repeat-tool-reminder`:per-agent consecutive-repeat 检测,默认阈值 `[3, 5, 8]`,用户干预重置。参数规范化:deep key-sort JSON → `JSON.stringify`。

---

## 12. 会话持久化

### 12.1 写入路径

```
Session.append('tool/call', data)
  ↓
session-persistence adapter 监听 append 事件
  ↓
SessionWriteBehind.enqueue(structuredClone(event))
  ↓
200ms 定时器到 → startBackground → coordinator.appendBatch(meta, events, isMaterialized)
  ↓
SQLite Backend.appendBatch(BEGIN IMMEDIATE → INSERT events → COMMIT)
  ↓
flush() (checkpoint or shutdown) → drainBarrier 强制排空
```

### 12.2 WriteBehind — 200ms 窗口 + 屏障

`DEFAULT_WRITE_BATCH_MAX_DELAY_MS = 200`(line 31)。不变量:
- `enqueue` 只 push,不动写入
- pending 从 0 变 1 时启动 200ms 定时器
- `flush()` 取消定时器并强制排空到 quiescence,所有并发 caller 共享同一个 barrier promise
- 失败时 batch 重新 prepend 回 pending,标记 `automaticPaused=true`

### 12.3 SQLite 后端

所有 SQL 字符串都打包在 `resources/sql/*.sql` 文件里,通过 `closed union` 类型在编译期保证只有受信任的资源能被调用,杜绝 SQL 注入风险。`SCHEMA_VERSION` 单调递增。

### 12.4 持久后端接口

`PersistenceBackend<TornMarker>` — 必选: `loadStored` / `appendBatch` / `commitRepair`;可选: `loadStoredFrom`(seekable) / `materializeHeader`。Coordinator 抽象出 "Backend 负责 IO,Coordinator 负责 buffer/序列化/repair/dispose" 的清晰分工。

### 12.5 与 laew 对比

| 维度 | deepseek-harness | laew |
|---|---|---|
| 会话写入 | WriteBehind 200ms 批 + barrier 强排空 | 直写 SQLite |
| 持久后端 | JSONL + SQLite 双实现 | 单 SQLite |
| Schema 版本 | `SCHEMA_VERSION` 单调递增 | 单 schema |
| Checkpoint | 每 projection 持久化 + stateVersion 校验 | session_memory 表 |
| SQL 安全 | 资源文件 + closed union 编译期校验 | `format!` 拼字符串 |

---

## 13. 前端与 native

### 13.1 apps/cli — dsh CLI 入口

**apps/cli/src/bin.ts:24-55** 三种调用模式: `profile` / `plugin` / `dump-config`。

**args.ts** 的 launcher 与 app 插件职责分界:launcher 只解析自己拥有的标志,剩余参数原样交给启动的 tree,注入的 app 插件解析自己的标志族并打印自己的 `--help`。

**process-shutdown.ts** — 三级升级策略:正常完成 → `process.exitCode = code`;SIGTERM/SIGINT → graceful dispose → 5s 超时强制 exit;重复信号 → 立即 forceExit。

### 13.2 apps/web — Web GUI 前端

**架构**:极简壳 —— `apps/web/src/main.ts` 仅 6 行: `new AppWebEntry(el).run()`。真正的复杂度在 `packages/client/` 中的 30+ 子包。

**两阶段启动**:模块阶段(加载模块系统 + 预取 immediately 层)→ 插件阶段(激活所有客户端插件)。

**Vite 构建精细分块**:vendor 包含 katex/shiki/mdast/micromark 等;懒语法块(`@shikijs/langs`)不进入 vendor,每个按需加载;启动语法(typescript/shellscript/json)进入 vendor。

**Web 前端不能独立启动** —— `rejectStandaloneServe()` 在 `vite serve` 时抛出错误,确保始终通过 CLI Host 的 `host-webserver` 服务启动。

**Slot 系统** 是 UI 组件的核心组合机制 —— 插件通过声明 slot 名称来注入 UI 片段。

### 13.3 native/landlock-run — Landlock 安全启动器

**核心 C 源文件**:`native/landlock-run/packages/entry/src/main.c` (~300 行纯 C11)。

关键设计:
1. **零依赖**:纯 C11 + libc(musl 静态链接),审计面仅此文件 + 内核 syscall 合约
2. **自定义 UAPI 头**:不依赖 `<linux/landlock.h>`,自定义所有结构体和宏
3. **ABI 协商**:运行时内核可能不支持最新 ABI,启动器自动降级到支持的子集
4. **失败关闭**:任何错误都以 **exit 125** 退出(不执行命令)

**JavaScript API**:`launcherPath()` / `grantArgs(grants)` / `probe(launcher, options)` — `probe()` 是功能探测(在子进程中实际构建并执行一个最大规则集,确认内核确实执行了 Landlock 限制)。

### 13.4 client/ — Web UI 组件库(30+ 包)

覆盖 Web GUI 的所有 UI 组件:`web`、`modules`、`store`、`connection`、`hmr`、`locale`、`ui-chat`、`ui-conversation`、`ui-primitives`、`ui-renderer`、`ui-layout`、`ui-sidebar`、`ui-goal`、`ui-plan`、`ui-subagent`、`ui-workflow-run`、`ui-trajectory`、`ui-settings`、`ui-theme`、`ui-slots`、`ui-tool`、`ui-skill`、`ui-jobs`、`ui-approval` 等。

### 13.5 host/ — HTTP 服务器与前端服务

`host/webserver`:`node:http` 服务器:精确匹配 > 最长前缀 > fallback handler。`host/frontend-static`:SPA dist 静态文件服务。`host/directory-picker`:目录选择器服务(`-auto` / `-browse` / `-native`)。`host/plugin-inventory`:插件清单。

---

## 14. 多轮对话与循环架构

### 14.1 ReactLoopAgent 核心驱动

**文件**:`packages/core/agent-loop/src/agent.ts`

实现 **Turn/Step 双循环** 架构:

```typescript
type Phase =
  | { kind: 'idle'; lastTurn: number }
  | { kind: 'maintenance'; abort: AbortController; lastTurn: number; wakeRequested: boolean }
  | { kind: 'running'; abort: AbortController; turn: number; step: number; wakeRequested: boolean }

private async kick(): Promise<void> {
  try { while (await this.turn()) {} } finally { /* 回到 idle + replay latch */ }
}

private async turn(): Promise<boolean> {
  this.session.append('turn/start', { turn })
  while (true) {
    const decision = await this.preStep(target, { turn, step })
    const stepEnd = await this.step(decision.assembly, ...)
    if (turnEnds && this.inbox.nextStep.length === 0) break
  }
  return this.inbox.hasPending ? (phase.abort = new AbortController(), true) : false
}
```

### 14.2 Inbox 队列

Inbox 是双队列系统:`next-turn`(用户新消息,触发新一轮)、`next-step`(工具结果或中间消息,触发当前 Turn 的下一步)。`send/followup/steer/inject` 写入;`steer/inject` 进入 next-step 槽,`followup` 进入 next-turn 槽。

### 14.3 Step 执行流程

1. 发射 `step/start` 事件
2. 调用 LLM(通过 `PreparedCall` 或 `LlmRuntime.stream()`)
3. 流式消费 chunk(`for await (const chunk of stream) { signal.throwIfAborted(); assembler.push(chunk); append('assistant/chunk') }`)
4. 无工具调用 → Turn 结束
5. 有工具调用 → `executeToolCalls` 并行执行,输出 context 回到 inbox → 继续 Step 循环

### 14.4 工具调用调度

`executeToolCalls` 实现 **bounded rolling parallel pool** + **exclusive barrier**。`maxParallelToolCalls` 来自 `ctx.agentLoop.config`(运行时 getter,每次 scheduler 决策重读)。结果和上下文 **按 model order commit**(即使 dispatch 重叠)。

三态调度器:`prepare` → 决定 dispatch / post-result / final-result;`dispatch` → 真正调工具;`finalize` / `finish` → 收尾。

### 14.5 maintenance 相位

`maintenance` 是 Agent 状态机里的特殊相位,允许在 idle 期执行 checkpoint / projection hydrate 等维护操作,完成后自动 resume inbox 中已 wake 的请求。

---

## 15. 工具/MCP/Skill 三系统

### 15.1 Tool 系统 — 6 事件管线

**文件**:`packages/core/tools/src/index.ts`(`ToolRuntime:788`)

```typescript
'tools/pre-execute'(exec, next): Promise<PreToolDecision>     // allow/deny/ask
'tools/execute'(exec, next): Promise<ToolExecutionResult>     // around: timeout/retry/metrics
'tools/post-execute'(exec, result, next): Promise<PostToolDecision>  // accept/replace/block
'tools/ptc-dispatch-log'(dispatch, next): Promise<ContentBlock[]>
'tools/result'(exec, result): emit                              // 冻结最终结果
'tools/change'(): emit                                          // registry 变更
```

**ScopedLayers**:per-agent tool registration 覆盖 global;restrictions(allow/deny list)继承链交叉。

**PTC 模式**(Program-Then-Collapse):`run_code` 工具让 LLM 在 TS 沙箱里写代码,通过 `tools` 全局变量调用工具。`mode: 'ptc'` 时仅暴露 `run_code` + 生成 SDK prompt,所有其他工具通过程序内 SDK 调用。

**`ToolDefinition` 完整契约**:`output { schema, render, presentationMeta? }` + `execute(args, exec)` + `finalizeContent?` + `timeoutMs?` + `isConcurrencySafe?` + `presentCall?/presentResult?`。

### 15.2 MCP 系统 — stdio/HTTP + 两阶段 sync

`apply()` 三层都用了 `ctx.effect()` 注册清理函数:命名空间释放、连接断开、startup 等待。

**命名空间**:`mcp__<serverName>__<rawName>`,归一化到 64-char 契约,长度超则 `truncate(51) + sha256(...)[0..12]`。

**两阶段同步**:Phase 1 Fetch(拉取完整工具列表,不影响注册表)→ Phase 2 Swap(先注销旧工具,再注册新工具)。保证模型永远看到完整的工具集。

**指数退避重连**:500ms → 1s → 2s → 4s → 8s → 16s → 30s(封顶),最多 10 次尝试。稳定性窗口重置:连接持续超过 `maxDelayMs` 后重置失败计数。

**图片投影**:MCP 返回图片(base64) → 解码 → 持久化存储 → 替换为 attachment ref。严格验证 MIME type 和 base64 合法性。

**凭证清洗**:`scrubbedParentEnv()` 从父进程环境变量中剔除凭证类变量,只传递安全的环境变量给子进程。

### 15.3 Skill 系统 — Layered Registry

`SkillLayer` 内有 `providers: NamedEntries<RegisteredProvider>`(有序)+ `runtime: Map<string, SkillDefinition>`(动态)。`WeakMap<ScopeKey, number>` 给 scope 一个稳定 ID 用于缓存键。

**Rank 系统**:`ProjectDSH=100 < ProjectAgents=200 < Runtime=250 < Custom=300 < UserDSH=400 < UserAgents=500 < Bundled=600`。"rank + providerOrder + name" 三级排序是 layer 内冲突消解的统一算法。

**文件系统 Provider** 用 Chokidar 监听(可配置 `watchUsePolling`,WSL/CI 友好),通过 `fs/observed` 监听宿主写文件事件主动标记缓存失效。

### 15.4 其他亮点

**Continuable 子代理**:`subagent/continuation.ts`(1569 行)用 `ContinuationManager` 维护耐久型子 agent,parent 持 `AgentHandle` 通过 inbox 推消息;`start` vs `startContinuable` 区分一次性 vs 持续型。

**Typert 类型图 + Remote BFF**:自研 `@deepseek-ai/dsh-typert-protocol` + `packages/typert/{generator,loader,registry,protocol}` + `packages/api/gateway` 构成完整 RPC 类型系统,服务端与客户端共享 "类型图",编译期约束。

**快照测试**:`snapshots/{session,web,sdk,acp}` + `vitest.snapshot.config.ts` 录制+回放,keyless;`test:web-stress` 与 `test:web:perf`。

**会话提醒**:`packages/schedule/schedule` — 三种提醒(延迟/绝对/重复),持久化,空闲 agent 立即交付,不发送到会话外。

**Spill 存储**:`packages/spill/spill` — 超大工具输出保存为文件,返回检索定位器 + 模型可读的检索提示。本地后端文件布局 `<root>/session-<hash>/<random>-<safeName>`。

---

## 16. 对 laew 的借鉴

### P0 — 必须借鉴(架构级)

| # | 借鉴点 | 实现位置 | laew 现状 | 落地收益 |
|---|---|---|---|---|
| P0-1 | **Fiber epoch 算法** — 依赖感知的加载/卸载 | `vendor/cordis/src/fiber.ts` | 无 plugin 热加载 | 多 Agent 编排时依赖就绪自动启动 |
| P0-2 | **Capability seam + provider 分离** | 52+ seams | 工具/Bash/Read/Write 硬编码 | 新能力即插即用 |
| P0-3 | **工具 6 事件管线 + ScopedLayers** | `core/tools/src/index.ts` | 无(工具直执行) | 权限拦截、超时、重试、per-agent tool 可见性 |
| P0-4 | **事件源化会话日志** | `Session.append()` | laew 内存态 context | 可 replay、可 audit、可 fork |
| P0-5 | **fail-closed 审批 + 沙箱** | `ApprovalService` + `SandboxProvider.confine()` | 零校验 | 命令行 Agent 安全基线 |
| P0-6 | **timeout-policy guard** | `guard/timeout-policy/src/index.ts:55-81` | BashTool 超时直接 `Err` | 结构化错误码,模型区分超时 vs 失败 |
| P0-7 | **repeat-tool-reminder** | `guard/repeat-tool-reminder/src/index.ts:189-207` | 无 | 阻止模型陷入循环 |

### P1 — 应该借鉴(显著提升)

| # | 借鉴点 | 实现位置 | 落地场景 |
|---|---|---|---|
| P1-1 | **Compaction 双触发 + Tool-result pruning** | `BasicCompactionEngine`, `ToolResultPruner` | 长对话 token 控制 |
| P1-2 | **SubAgent scoped composition + depth budget** | `applyChildComposition`, `resolveChildDepth` | laew SubAgent 上下文隔离 |
| P1-3 | **Workflow 脚本化编排** | `WorkerThreadWorkflowEngine` | 复杂任务的多步编排 |
| P1-4 | **MCP 桥接 + 自动 tool sync** | `mcp-client` `apply()`/`syncTools` | 复用 MCP 生态工具 |
| P1-5 | **Typert 远程 + 类型反射** | `TypertRemoteService` + `@Remote` | TUI ↔ Agent 跨进程类型安全通信 |
| P1-6 | **Goal round-driver + CAS mutation** | `GoalService` + `goal-round-driver` | Yolo 目标识别 → 执行 → 完成闭环 |
| P1-7 | **Session 写入批处理(WriteBehind)** | `session-persistence/src/write-behind.ts` | Session 写入 IO 次数降低 1-2 个数量级 |
| P1-8 | **MCP 命名空间管理** | `publicToolName` + `activeServerNames` | 防止工具名冲突 |

### P2 — 可以借鉴(锦上添花)

| # | 借鉴点 | 实现位置 | 落地场景 |
|---|---|---|---|
| P2-1 | **Skill Markdown + ranked roots** | `SkillRegistry` + `FileSystemSkillProvider` | 用户可注入的「如何做」知识 |
| P2-2 | **Schedule 提醒** | `ScheduleRuntime` | 定时任务/提醒 |
| P2-3 | **Webhook + 规则注册表** | `WebhookRuntime` | CI/CD 触发 Agent |
| P2-4 | **pi-ai 库桥接多 provider** | `llm-pi-ai` | 快速支持新 provider |
| P2-5 | **ACP JSON-RPC bridge** | `dsh-acp` | 受信任程序matic client 接入 |
| P2-6 | **Landlock 原生沙箱** | `native/landlock-run` | Rust 版 Landlock 安全启动器 |
| P2-7 | **多子代理后端** | `subagent/` 11 包 | 支持 spawn/fork/ACP/SDK/Claude Code/Codex |
| P2-8 | **PTC 模式(Program-Then-Collapse)** | `run_code` 工具 | 沙箱内编码,节省 token |

### 不借鉴(anti-patterns)

| 点 | 原因 |
|---|---|
| Cordis 的 `Proxy` + 全局 Symbol 路由 | Rust 不需要,所有权系统已解决 |
| worker thread containment | Rust 原生进程/线程更安全 |
| 52+ npm 包的微粒度过细 | Rust crate 边界即可,不必镜像 |
| `declare module` 类型增强 | Rust trait + impl 更清晰 |

---

## 附录 A:关键文件路径索引

### Cordis 微内核

| 文件 | 核心内容 |
|---|---|
| `vendor/cordis/src/context.ts` | Context IOC 容器 + Proxy + extend/isolate/intercept |
| `vendor/cordis/src/service.ts` | Service 基类 + 拦截配置合并 |
| `vendor/cordis/src/fiber.ts` | Fiber 运行时 + epoch 算法 + effect 生命周期 |
| `vendor/cordis/src/registry.ts` | RegistryService + Inject + plugin 注册 |
| `vendor/cordis/src/events.ts` | EventsService 5 模式 + Hook |
| `vendor/cordis/src/reflect.ts` | ReflectService service 解析 + provide/notify |
| `vendor/cordis/src/utils.ts` | symbols + DisposableList + composeError |

### 核心模块文件索引

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

### laew 架构映射

| laew 概念 | DeepSeek-Harness 对应 | 文件 |
|---|---|---|
| `src/agent/mod.rs` 循环 | `ReactLoopAgent` + `AgentLoop` | `agent-loop/src/{agent,index}.ts` |
| `src/agent/tools/*` | `ToolRuntime` + per-tool plugins | `core/tools` + `packages/shell/tool-bash` 等 |
| `src/llm/*` | `LlmRuntime` + `LlmAdapter` 子类 | `llm/llm` + `llm-{deepseek,pi-ai}` |
| `src/tui/mod.rs` | `CommandRuntime`(斜杠命令注册表) | `interaction/commands/src/index.ts` |
| `src/config/mod.rs` SQLite | `SqliteSessionPersistence` + `FileSettingsProvider` | `session-persistence-sqlite` + `settings-file` |
| Yolo 分类 | `GoalService` CAS + `goal-round-driver` | `goal/goal/src/index.ts` |
| Plan Agent | `PlanModeController` + `WorkflowEngine` | `plan-mode` + `workflow` |
| SubAgent | `SubagentRuntime` + scoped composition | `subagent/subagent/src/index.ts` |
| SessionContext 摘要 | 事件源化日志 + session-query FTS | `session` + `session-query` |

---

**文档合并完成日期**:2026-09-06
**原始文档**:8 份(源码调研 + 深度分析 + 核心机制 + 第二轮深挖 + 第三轮-apps&native + 第四轮-Cordis核心 + 补充-中断传播背压与Typert + 补充-流式ACP与决策溯源)
**原始总行数**:~8,857 行
**合并策略**:第四轮 > 第三轮 > 第二轮 > 第一轮 > 补充,按功能模块重组章节,去重保留独特贡献

---

## 14. 第五轮深挖补充(2026-09-06)

补充前 13 章覆盖薄弱的代码级事实。所有行号来自 `/usr/local/LsmGitOpenSource/deepseek-harness` 当前 head(2026-08-28 release/dsh-0.1.2-alpha.1 merge 后)。

### 14.1 AIAgent 主循环:kick() / turn() / step() 三层

**kick()**(`packages/core/agent-loop/src/agent.ts:217-230`):
```ts
private async kick(): Promise<void> {
    try {
        while (await this.turn()) {}
    } catch (_error) {
        // Reported failures and cancellation are contained at the driver boundary.
    }
}
```
**Turn 内 step 嵌套 while(true)**(`agent.ts:269-308`):
```ts
while (true) {
    signal.throwIfAborted()
    const step = phase.step + 1
    const decision = await this.preStep(target, { turn, step })
    // ...
    const stepEnd = await this.step(decision.assembly, decision.startsRequestSeries === true)
    // max-tokens stays sticky: ...
    if (turnEnds && this.inbox.nextStep.length === 0) break
    target = 'next-step'
}
```
- `TurnEndReason = 'completed' | 'blocked' | 'aborted' | 'error' | 'max-tokens'`

**Step 内 LLM 重试循环**(`agent.ts:346-406`):
```ts
while (true) {
    // ...
    const stream = preparedCall?.stream(request) ?? this.loopCtx.llm.stream(request)
    // ...
    if (finish.kind === 'error' || finish.kind === 'aborted') {
        const action = await this.dispatch.waterfall('agent/request-error', ...)
        if (action?.kind !== 'retry') {
            throw new LlmError(finish.failure.message, finish.failure.code, finish.failure)
        }
        continue
    }
}
```
- **关键设计**:LLM 错误通过 `agent/request-error` 钩子决策,**插件/中间件可改变重试策略**(不只依赖主循环内部)——比纯 retry-loop 更可扩展。

**Wake/abort 模型**(`agent.ts:192-199`):
```ts
this.setPhase({
    kind: 'running',
    abort: new AbortController(),
    turn: this.phase.lastTurn, step: 0, wakeRequested: false,
})
this.loopCtx.agents.withInitiator(this, () => this.kick())
```
- 每个 run 持独立 AbortController,通过 `setPhase` 记录当前 phase,可在外部 kick 唤醒。
- **无 hard maxTurns 上限**——靠 inbox 清空或 turn-end 退出。

### 14.2 Compaction 完整抽象 + Commit Fence

**Seam 抽象类**(`packages/compaction/compaction/src/index.ts:96-117`):
```ts
export abstract class CompactionEngine extends Service {
    abstract compactIfNeeded(agent, trigger: 'pressure'|'context-overflow', signal): Promise<CompactionResult|null>
    abstract compactNow(agent, signal, sourceCommandId?): Promise<CompactionResult|null>
    abstract compactRegion(start, end, agent, signal?): Promise<CompactionResult>
}
```
- trigger 仅两态:`pressure`(主动阈值)与 `context-overflow`(API 拒绝后被动)。
- `ManualCompactionError.code` ∈ `{busy, cancelled, changed, summary, commit, persistence}`。

**Commit Fence**(`packages/compaction/compaction-basic/src/region.ts:191-218`):
```ts
const startEvent = session.append('compaction/start', lifecycle)   // 持久开锁
const assertStable: StabilityCheck = options.stability === 'whole-surface'
    ? assertWholeSurfaceUnchanged
    : assertSelectedSpanStable
let failure: TransactionFailure | undefined
// ... try {
const prepared = prepareCompaction(...)
const summarized = await summarizeCompaction(...)
stage = 'commit'
const pending = commitCompactionBody(session, startEvent, summarized)
closing = true
const endEvent = session.append('compaction/end', lifecycle)       // 持久关锁
closed = true
result = completeCompaction(pending, endEvent)
} catch (error: unknown) {
    failure = { error, stage: closing ? 'commit' : stage }
    if (!closing) {
        closing = true
        try { session.append('compaction/end', { ...lifecycle, error: errorChain(error) }) ... }
```
- **设计要点**:`compaction/start` → `compaction/end` 是**两事件事务**——任意步骤失败必写带 error 的 end 事件;否则 abandoned 中间态会永久污染 session。
- **Stage 字段**(`TransactionFailure['stage']`):`'summary' | 'commit'`,用于诊断失败点。

**Region 范围选择**(保留 priced tail,工具对配对)(`region.ts:100-136`):
```ts
export function selectCompactableRange(session, measurement, retainTokens) {
    const pricedNodes = measurement.nodes
    // ...
    let accumulated = 0
    let keepFromIdx = pricedNodes.length
    for (let index = pricedNodes.length - 1; index >= 0; index -= 1) {
        accumulated += pricedNodes[index]!.tokens
        keepFromIdx = index
        if (accumulated >= retainTokens) break
    }
    if (keepFromIdx === 0) return null
    while (keepFromIdx > 0) {
        if (toolPairingBalancedBefore(session, surfaceNodes[keepFromIdx]!)) break
        keepFromIdx -= 1
    }
    // ...
    return { start: first, end: cutoff }
}
```
- **工具对配对保护**:`toolPairingBalancedBefore`/`After`(`region.ts:317-338`)防止切断 tool-call/result 对——region 边界必须落在 `paired=true` 位置。
- 失败时退到 `keepFromIdx--` 直到配对平衡。

**Checkpoint 标识**(替换用户消息的 source)(`packages/compaction/compaction/src/checkpoint.ts:19,33-51`):
```ts
const COMPACT_CHECKPOINT_MARKER = Object.freeze({ kind: 'plugin', plugin: 'compact' } as const)
// ...
export function compactCheckpointSource(compactionId, sourceCommandId?) {
    return Object.freeze({ ...COMPACT_CHECKPOINT_MARKER, compactionId, ...sourceCommandId === undefined ? {} : { sourceCommandId } })
}
export function isCompactCheckpointSource(source) {
    return source.kind === 'plugin' && source.plugin === COMPACT_CHECKPOINT_MARKER.plugin
}
```
- **设计要点**:压缩产物在 session 中**保留为 user 消息**(`source.kind='plugin'`),而不是删除原消息——保证 session 历史是 append-only 真相源,可从任意点回放。

**Hook 接入**(`packages/compaction/compaction-basic/src/index.ts:147-223`):
```ts
ctx.on('agent/pre-step', async ({ agent, signal }, next) => {
    if (!signal.aborted) {
        try {
            const result = await this.compactIfNeeded(agent, 'pressure', signal)
            // ...
        }
    }
    return next()
})

ctx.on('agent/request-error', async ({ agent, failure, signal }, next) => {
    if (failure.code !== CONTEXT_WINDOW_EXCEEDED_CODE || signal.aborted) return next()
    // ...
    result = await this.compactIfNeeded(agent, 'context-overflow', signal)
    // ...
    return { kind: 'retry' }
})
```
- **双触发点**:pre-step(主动 pressure)+ request-error(被动 overflow),后者返回 `{kind:'retry'}` 让 LLM 重新发起请求——无缝衔接。

### 14.3 Subagent 多形态

`packages/subagent/` 子包(11 个):
`subagent`、`subagent-codex`、`subagent-spawn-in-process`、`subagent-fork-in-process`、`subagent-claude-code`、`subagent-in-process-driver`、`subagent-dsh-sdk`、`subagent-acp`、`tool-subagent`、`tool-subagent-control`、`tool-subagent-report`

**核心 ChildAgent**(`packages/subagent/subagent/src/child-agent.ts:34,114,151,206`):
```ts
super(`subagent depth ${attemptedDepth} exceeds maxDepth ${maxDepth}`)
// ...
subagentDepth: childDepth,
// ...
origin: 'subagent',
// ...
childCtx.systemPrompt.context({ name: 'subagent:delegation', order: 120, text: SUBAGENT_DELEGATION_CONTEXT })
```
- **深度防护**:递归调用 maxDepth 显式抛错。
- **origin 标记**:`'subagent'` 写入 message origin——上溯可达。
- **System Prompt 段**:`subagent:delegation` 注入(`order: 120` 控制拼接位置)——子代理知道自己是被调用的,不是顶级 Agent。

**版本化、持久化描述符**(`packages/subagent/subagent/src/descriptor.ts:88,156`):
```ts
/** The supported durable subagent identity and optional continuation composition. */
// ...
throw new Error(`persisted subagent descriptor ${path} has unknown field "${unknown}"`)
```
- 描述符**版本化 + unknown 字段拒绝**——schema 演进时拒绝静默。

**单次运行的结算**:`packages/subagent/subagent/src/run-settlement.ts` 将 ONE-SHOT 子代理结算为 background-Task 结局——统一的"任务完结"抽象。

### 14.4 对 laew 的 P0/P1/P2 借鉴路线

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | Compaction 两事件事务 | `compaction/start` → `compaction/end`,失败必写带 error 的 end——防止 abandoned 中间态 | region.ts:191-218 |
| **P0** | Region 工具对配对保护 | 边界必须落在 `toolPairingBalancedBefore` 位置,防止切断 call/result 对 | region.ts:317-338 |
| **P0** | Checkpoint as User Message | 压缩产物保留为 `source.kind='plugin'` user 消息——append-only 真相源 | checkpoint.ts:33-51 |
| **P0** | SubAgent depth 防护 | 递归 maxDepth 显式抛错——防止无限下钻 | child-agent.ts:34 |
| **P1** | LLM 错误由钩子决策 | `agent/request-error` 钩子可决定重试/换 prompt/放弃——扩展性远超内置 retry | agent.ts:346-406 |
| **P1** | 双触发压缩 | pre-step pressure + request-error overflow 互补,无缝衔接 | compaction-basic/index.ts:147-223 |
| **P1** | SubAgent origin 标记 | `origin:'subagent'` 写入消息,上溯可达 | child-agent.ts:151 |
| **P1** | SubAgent delegation 段 | systemPrompt 注入 `subagent:delegation`,order=120 控制拼接位置 | child-agent.ts:206 |
| **P1** | 描述符版本化 | unknown 字段拒绝——schema 演进时拒绝静默 | descriptor.ts:156 |
| **P2** | ManualCompactionError.code | 6 种 error code 枚举(busy/cancelled/changed/summary/commit/persistence)便于诊断 | compaction/index.ts:96-117 |
| **P2** | TurnEndReason 5 态 | completed/blocked/aborted/error/max-tokens——单元测试可枚举 | agent.ts:269 |
| **P2** | LlmError 继承 LLM 失败结构 | message/code/cause 三元组——便于监控聚合 | agent.ts:346-406 |
