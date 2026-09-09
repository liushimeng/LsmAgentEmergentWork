# 第十八轮 deepseek-harness 深度分析 — 用户交互体验层

## 元信息

| 字段 | 值 |
|------|------|
| 文档版本 | 第十八轮 / 2026-09-09 |
| 工程 | deepseek-harness（`@deepseek-ai/dsh-*`） |
| 定位 | Web-first TypeScript Agent CLI + Web IDE（双形态） |
| 核心架构 | Cordis Everything-is-a-Plugin + Fiber epoch + Typert Remote + 30+ 子域包 |
| 调研主题 | **用户交互体验层**（8 维度） |
| 行数目标 | 600-1200 行 |
| 调研方法 | 直接 `Read` 关键文件 + `Grep` 关键词定位 + 行为模式提炼 |
| 输出 | 中文分析 + 英文标识符 + 文件:行号 + 30 个新 laew gap (L1456-L1485) |
| 调研路径 | `/usr/local/LsmGitOpenSource/deepseek-harness` |

> **关键发现速览**：本工程是**纯 Web 工程（无 TUI/终端）**——所有「输入」「提及」「菜单」「主题」「状态行」均映射到 React + Lexical 编辑器 + slot 渲染层。因此与 laew（TUI 为主）的映射需要做 1:1 范式翻译：**Web Editor → TUI InputHandler**、**React Portal → TUI 子屏**、**slot 注入 → TUI 子屏路由**。这种范式差异本身就构成一份极有价值的"用户体验工程"参考——所有维度 deepseek 都有"工业级"实现，laew 可以借鉴其"分阶段提交 / 异步防抖 / 选择降级 / 中断取消 / lex 词典快照"等模式。

---

## D1 — @提及系统（@file / @session / @symbol）

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/context/file-reference/src/grammar.ts` | 全文（83 行） | 浏览器安全共享的 `@file` 语法 |
| `packages/context/file-reference/src/types.ts` | 全文（19 行） | `FileReferenceCandidate` 类型 |
| `packages/context/file-reference-local/src/search.ts` | 全文（368 行） | `WorkspaceFileSearch` 模糊索引 + 评分 |
| `packages/context/file-reference-local/src/index.ts` | 全文 | Cordis service `LocalFileReferenceService` + 失效总线 |
| `packages/context/session-reference/src/index.ts` | 全文 | `@session` 提及服务 |
| `packages/client/ui-input-trigger/src/types.ts` | 全文（230+ 行） | `InputTriggerSource` 完整契约 |
| `packages/client/ui-input-trigger/src/core/contract.ts` | 全文 | `MenuState` / `MenuReduce` / `DetectTrigger` |
| `packages/client/ui-input-trigger/src/client/controller.ts` | 全文 | per-Session controller |
| `packages/client/ui-reference/src/client/index.ts` | 全文 | 合并的 `@` 源：file + session |
| `packages/client/ui-conversation/src/client/contract/input.ts` | L40-105 | `PickOutcome` / `ReferenceInsert` / `CommandClaim` |

### 机制剖析

**1. 语法边界严格性**（`grammar.ts:30-40`）：
```ts
export function activeAtToken(line: string, cursorCol: number): ActiveAtToken | undefined {
  const beforeCursor = line.slice(0, cursorCol)
  const quoted = /(?:^|\s)(@"([^"]*))$/u.exec(beforeCursor)
  if (quoted?.[1] !== undefined && quoted[2] !== undefined) {
    return { prefix: quoted[1], query: quoted[2], quoted: true }
  }
  const plain = /(?:^|\s)(@([^\s]*))$/u.exec(beforeCursor)
  // ...
}
```
- `(?:\s|^)` 限定**只在空白后**才识别 `@`——这避免了 `user@host` 邮箱被误识别为提及。
- `quoted` 分支支持 `@"path with spaces"` 显式引号语法——空格键/中文路径安全。
- 关键设计：`query` 不含 `@`，提交时被替换为 `prefix`（含 `@`）。

**2. 提及菜单控制器**（`ui-input-trigger/src/core/contract.ts:48-95`）：
```ts
export type MenuReduce = (state: MenuState, ev: MenuEvent) => MenuState
export type MenuEvent =
  | { type: 'hit'; hit: TriggerHit | null }
  | { type: 'source-settled'; generation: number; source: string; items?: ... }
  | { type: 'source-failed'; generation: number; source: string }
  | { type: 'move'; dir: 1 | -1 }
  | { type: 'hover'; source: string; index: number }
  | { type: 'close' }
```
- **代际（generation）防 stale**：`source-settled` 携带 `generation`，与 trigger hit 时的代际比对；过时回包被纯函数 reducer 静默丢弃，避免竞态写入。
- **失败降级为静默组移除**："Source failure = silent group removal (log only; no error UI tier)"。

**3. 异步 workspace 索引**（`search.ts:80-180`）：
```ts
class WorkspaceFileSearch {
  private settled: SettledIndex | undefined
  private generation: IndexGeneration | undefined
  private invalidations = 0

  async list(rawQuery, signal) {
    // 目录形式（slash 存在）→ 走实时 readdir
    // 裸模糊查询 → 走 settled index + ranking
    const indexed = await this.indexFor(signal)
    return rankCandidates(indexed.filter(...), query, this.config.maxResults)
  }

  invalidate() { this.invalidations += 1 }  // 不立刻清空 settled，标记陈旧
}
```
- **惰性 + 缓存 + 失效分离**：首次 `list()` 触发一次 BFS 索引（`scanWorkspace`），结果入 `settled`；`invalidate()` 只增计数——下一次"裸查询"感知到陈旧，**后台静默重建**而不阻塞 UI（`void this.ensureIndex().catch(...)`）。
- **关键洞见**：BFS 重建成本可被一次性分摊给所有后续查询；UI 永远拿得到"上一次成功"的候选。

**4. 排序算法**（`search.ts:283-310`）：
```ts
function scoreCandidate(candidate, query): number | undefined {
  if (name === needle) return 1_000 + directoryBonus  // 完全匹配
  if (name.startsWith(needle)) return 900 + directoryBonus  // 名称前缀
  if (name.includes(needle)) return 700 + directoryBonus   // 名称包含
  if (path.includes(needle)) return 500 + directoryBonus   // 路径包含
  const subsequence = subsequenceScore(path, needle)
  return subsequence === undefined ? undefined : 300 + subsequence + directoryBonus
}
```
- 4 级 ladder：完全匹配(1000) > 名称前缀(900) > 名称包含(700) > 路径包含(500) > 子序列(300)
- 目录 +25 加成（让目录优先于文件——给用户"下钻"机会）
- 同分按 path 长度、再按字母序——确定性可测试

**5. 提及生命周期 / 失效总线**（`file-reference-local/src/index.ts:60-95`）：
```ts
ctx.on('session/event', (session, event) => {
  if (event.type !== 'tool/result') return
  const agent = ctx.agents.get(session.id)
  if (agent !== undefined) this.searches.get(agent)?.invalidate()
})
```
- 每次工具结果**都触发索引失效**——保证下一次 `@` 看到的是最新工作区。
- `invalidate` 不阻塞——后台重建+前台继续用陈旧索引。

**6. 统一触发源（trigger source）契约**（`ui-input-trigger/src/types.ts:130-180`）：
```ts
export interface InputTriggerSource {
  readonly trigger: TriggerChar  // '/' | '@'
  readonly name: string
  readonly order?: number
  candidates(session, req): Promise<readonly InputTriggerCandidate[]>
  header?(session, req): readonly InputTriggerCrumb[] | undefined  // breadcrumb
  onPick(pick): PickOutcome  // 'insert' | 'claim' | 'text'
  matchSpace?(session, token): PickOutcome  // 同步 hot-state
  matchEnter?(session, line, signal, envelope): Promise<PickOutcome>  // 异步可等 warmup
  warm?(session): void  // 预热
  lexicon?(session): readonly string[] | undefined  // 同步词典快照（热路径）
  subscribeLexicon?(session, listener): () => void
  readonly codec?: ReferenceCodec  // 序列化为模型可见文本
}
```
- `lexicon` 是**同步 hot-snapshot**——为渲染层提供 O(1) 装饰（编辑器扫 `<trigger><name>` 精确匹配）。"undefined = backing data not warm yet — no decoration, never a fetch"——这是性能与正确性双重保证。
- `matchSpace` 同步热路径 vs `matchEnter` 异步可等——**两条不同预算**的通道。

**7. 文件 + 会话联合源**（`ui-reference/src/client/index.ts:38-85`）：
```ts
async candidates(session, { query, quoted, drilled, signal }) {
  const fileLookup = ctx.remote.fileReferences.list(session.sessionId, query, signal)
    .then(result => result.ok ? result.value : [], () => [])
  const sessionLookup = quoted === true
    ? Promise.resolve([])
    : ctx.remote.sessionReferenceResolver.candidates(...).then(...)
  const [fileItems, sessionItems] = await Promise.all([fileLookup, sessionLookup])
  // ...
}
```
- **并行 + 类型化 Result**：两个 Remote 命名空间并行拉取，**统一 Ok/Err 处理**（`() => []` 错误降级为空数组）。
- 单一 `apply()` 注册一个 source，**一个 mention 触发两个后端**——菜单是单一 UX 表面。

### 设计巧妙点

1. **失效计数 + 静默重建**：`invalidate()` 不清空 `settled`——下一查询继续用旧索引，**后台刷新区分代际**。这把"重索引成本"从"用户输入延迟"完全剥离。
2. **`quoted` 路径语法**：`@"path with spaces"` 用引号包裹含空格/中文路径，且不引号目录**保留开引号**（`@"dir/` 留待下一级）——这是**词法驱动的 UX**。
3. **菜单代际（generation）单值跟踪**：单个递增数防 stale，避免复杂"事件序列号"。
4. **`lexicon` 同步快照**：渲染层（编辑器装饰）永远同步 O(1) 命中，不触发 RPC。
5. **`@` 触发与 `/` 触发同源契约**：一个 `InputTriggerSource` 接口覆盖两种触发字符（`/cmd` 与 `@ref`）——UI 层用同一 reducer 路径。

### laew gap 表（D1）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| L1456 | 无 @file 提及系统（路径补全仅 `/provider` 形式） | P1 | `fuzzy-matcher` + 自实现触发检测 |
| L1457 | 无 `lexicon` 同步快照热路径（提及装饰无同步 O(1) 优化） | P2 | `dashmap` + `arc-swap` |
| L1458 | 无工作区索引失效总线（每次工具结果未触发索引重扫） | P1 | `notify` v6 + `debounce` |
| L1459 | 无代际（generation）防 stale 机制 | P1 | `tokio::sync::watch` + `AtomicU64` |
| L1460 | 无文件排序 ladder 算法（路径补全仅子序列） | P2 | `fuzzy-matcher` 内置分数 |

---

## D2 — 自定义斜杠命令与 Prompt 模板

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/interaction/commands/src/index.ts` | 全文 | 命令注册 runtime + 解析 |
| `packages/interaction/commands/src/types.ts` | 全文 | `CommandDefinition` / `CommandResult` / 事件映射 |
| `packages/interaction/commands/src/brand.ts` | 全文 | `CommandId` brand 类型 |
| `packages/skill/skill/src/index.ts` | 全文 | Skill 系统（模板） |
| `packages/skill/tool-skill/` | 全文 | 工具形式的 skill |
| `packages/session-query/session-log-export/src/index.ts` | 全文 | `/export` 命令实例 |
| `packages/interaction/permission-presets/src/index.ts` | L42-65 | `/permission` 命令实例 |
| `packages/extensions/tool-cordis/src/inspect.ts` | 全文 | 反射型命令源 |

### 机制剖析

**1. 解析器**（`commands/src/index.ts:90-103`）：
```ts
export function parseCommand(line: string): ParsedCommand | undefined {
  const match = /^\/([a-z][a-z0-9_-]*)(?=$|[\t\n\r ])/u.exec(line)
  if (match === null) return undefined
  return Object.freeze({ name: match[1], rawInput: line.slice(match[0].length) })
}
```
- **强格式**：`/name` 后必须跟空白或行尾——`/abc` 是名字，`/abc-def` 也是，`/abc.def` 不是（避免与 URL 路径冲突）。
- `name` 小写、freeze——不可变。

**2. 注册接口**（`commands/src/index.ts:48-65`）：
```ts
export interface CommandDefinition {
  readonly name: string
  readonly description: string
  readonly input?: CommandInputDescriptor  // { hint: string, images?: boolean }
  readonly recordInput?: boolean  // 是否将 rawInput 写入日志
  readonly handler: (invocation: CommandInvocation) => CommandResult | Promise<CommandResult>
}
```
- `input.images` 决定是否接受图像附件；接收端可拒绝（handler 抛错回退原草稿）。
- `recordInput: false` 让"由领域事件拥有 payload 的命令"避免重复记录——典型例子：`/export` 不重复记录命令 args（领域事件有）。

**3. 分层注册**（`commands/src/index.ts:71-80`）：
```ts
class CommandLayer implements ScopeLayer {
  readonly commands: NamedEntries<RegisteredCommand>
  constructor(scope: ScopeKey | undefined) {
    this.commands = new NamedEntries(name => new Error(
      scope === undefined
        ? `command "${name}" is already registered (for a per-agent variant, mount a command-injected plugin under that agent's \`agent.ctx\`)`
        : `command "${name}" is already registered in this scope`,
    ))
  }
}
```
- **全局层 + 作用域层**双栈；同名时给清晰的"想 per-agent 变体请挂到 `agent.ctx`"指引。
- `NamedEntries` 提供同名错误的诊断信息——所有 entry 类型（commands / skills / tools）都共用此模式。

**4. 生命周期事件**（`commands/src/types.ts:65-95`）：
```ts
'command/run': { commandId: CommandId; name: string; args?: string; source: CommandSource }
'command/done': { commandId: CommandId; kind: 'success' | 'error'; text?: string; sourceEventSeq?: number }
```
- **配对 id** 贯穿 `command/run` ↔ `command/done`，可与下游 flow node 关联。
- `sourceEventSeq`：成功后引用**先前权威领域事件**——client 可重新计算更丰富的 UI 表现。
- `args` 在 `recordInput: false` 时省略——由权威事件所有。

**5. Skill 系统（命令的兄弟抽象）**（`packages/skill/skill/src/index.ts` 全文）：
- Skill 是文件级提示词模板 + 工具；`tool-skill` 是其工具化暴露。`/export` 这类命令是"短动作"，skill 是"长上下文模板"。

**6. 用户自定义 prompt 模板**（在 deepseek-harness 中）：
- 通过 Skill 机制实现——`packages/skill/skill-filesystem` 挂载文件系统 skill 扫描；用户放在约定目录的文件被注册。
- 命令 vs Skill 边界：**命令 = 即时动作 / 短输出**；**Skill = 上下文注入 + 工具集**。两者都通过 Cordis 注册，**共享注册表模式**。

### 设计巧妙点

1. **命令 vs 工具 vs Skill 三种抽象分层**：命令（`/cmd` 即时）、工具（LLM 调用）、Skill（文件+提示词+工具集）——不同生命周期、不同触发方式。
2. **作用域诊断信息**：`scope === undefined` vs 作用域内——给用户"如何改"的明确路径。
3. **配对事件 id**：与 flow node 关联——`sourceEventSeq` 让 client UI 可以"丰富重渲染"。
4. **att 语义**：`input.images` 声明图像能力，未声明则 server 拒绝含图像提交；handler 拒绝则原草稿保留——**优雅降级不破坏用户输入**。

### laew gap 表（D2）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| L1461 | 无运行时动态命令注册（仅启动期硬编码 `/help` `/clear` 等） | P1 | `inventory` + `linkme` |
| L1462 | 无作用域诊断（同名命令注册无"想 per-agent 变体"指引） | P2 | 自实现 NamedEntries + anyhow |
| L1463 | 无命令生命周期配对 id（`command/run` ↔ `command/done` 关联） | P2 | `uuid` + tracing span |
| L1464 | 无 recordInput=false 优化（领域事件已含 payload 时不重复） | P3 | 设计层 |
| L1465 | 无 Skill 系统（用户自定义 prompt 模板） | P1 | 自实现 + `include_str!` |

---

## D3 — 对话 Rewind / 分支【用户级】

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/session/session-checkpoint-policy/src/index.ts` | 全文（80 行） | 语义检查点（每 step 前 flush） |
| `packages/session/session-checkpoint-policy/README.md` | 全文 | 失败闭合策略文档 |
| `packages/session/session-persistence/` | 全文 | 持久化后端（JSONL / SQLite） |
| `packages/client/ui-chat/src/client/contract/slots.ts` | L68 / L126 | `forkAt: (seq: number) => void` |
| `packages/client/ui-chat/src/client/apply.ts` | L139 | `forkAt` 实际实现 |
| `packages/client/ui-chat/src/client/chat/ChatNodeSeat.tsx` | L82, L183, L188 | 节点级 fork 入口 |
| `packages/client/ui-chat/tests/chat-branch-tails.client.spec.tsx` | 全文 | branch 行为测试 |
| `packages/session/session-checkpoint-policy/tests/fixtures/crash-child.ts` | 全文 | 崩溃后恢复测试 |

### 机制剖析

**1. 语义检查点（基础）**（`session-checkpoint-policy/src/index.ts:60-80`）：
```ts
export function apply(ctx: Context): void {
  ctx.on('llm/stream', (options, next) => {
    if (options.sessionId === undefined) return next()
    const session = ctx.sessions.get(options.sessionId)
    return session === undefined ? next() : afterCheckpoint(ctx, session, next)
  })

  ctx.on('tools/execute', async (exec, next) => {
    if (exec.agent === undefined || exec.parent !== undefined) return next()  // 只 top-level
    await ctx.sessions.flush(exec.agent.session)
    if (exec.signal.aborted) return abortedBeforeDispatchResult()
    return next()
  })

  ctx.on('agent/pre-step', async ({ agent }, next) => {
    await ctx.sessions.flush(agent.session)
    return next()
  })
}
```
- **三处必须 flush**：`llm/stream` 前、`tools/execute` 前（top-level only）、`agent/pre-step` 前。
- **嵌套复用**：`exec.parent !== undefined` 让嵌套工具调用**复用外层 flush**——避免每次嵌套都 flush 父层。
- **失败闭合**：checkpoint 失败 → 拒绝 adapter dispatch / 拒绝 tool body——**never redo a side effect on crash**。
- 流式 chunk 本身不检查点（防爆），但**完整请求/响应已存**——重放时不可重发同一请求。

**2. 用户级 fork**（`ui-chat/src/client/contract/slots.ts:68`）：
```ts
forkAt: (seq: number) => void
```
- 单一方法签名：`forkAt(seq)`——从第 `seq` 节点分叉一个新分支。
- 出现在两处：
  - `TurnTailOwnerProps.forkAt: (seq: number) => void`（行 68）
  - `ChatNodeOwnerProps.forkAt: (seq: number) => void`（行 126）
- **实现位置**（`ui-chat/src/client/apply.ts:139`）：
```ts
forkAt: (seq) => { /* ... */ }
```

**3. 节点级 fork 入口**（`ui-chat/src/client/chat/ChatNodeSeat.tsx:82, 183, 188`）：
```ts
export function ChatNodeSeat({ ... forkAt, ... }) {
  // ...
  forkAt: (seq) => { /* dispatch to apply.ts implementation */ },
  // ...
  {node, selectedCallId, cwd, openFile, inspectCall, forkAt, ...}
}
```
- `forkAt` 是 `ChatNodeOwnerProps` 的成员——**每个节点都持有 fork 能力**，可被任何父级 UI 触发。

**4. 分支尾部测试**（`chat-branch-tails.client.spec.tsx:139`）：
- 测试描述："user bubbles expose clock / copy and neither branch nor edit; copy writes the text"
- 即：用户消息节点**暴露 clock + copy**，**不暴露 branch / edit**——分支只能从 assistant message 出发（用户输入是源头，分支源头是其后任意 assistant message）。
- 进一步："consumed steering renders as a plain user bubble and keeps copy without branch"（行 287）——被消费的 steering 没有分支入口。

**5. 用户消息 vs assistant 消息的差异**（`chat-branch-tails.client.spec.tsx:1006`）：`describe('small branch tails')`——专门测试短分支的视觉表现。

### 设计巧妙点

1. **三处 flush 边界**：`llm/stream` / `tools/execute` / `agent/pre-step` 覆盖了"产生副作用的所有边界"——比"每次事件都 flush"高效（避免 IO 放大），比"只在终止时 flush"安全（不会重做）。
2. **嵌套复用**：`parent !== undefined` 跳过嵌套 flush——精确表达"每个 side effect 之前一次"。
3. **失败闭合**：checkpoint 失败 → 不执行；stream 之前若 abort 直接返回 aborted result——**不重做副作用**是 crash recovery 的核心契约。
4. **`forkAt(seq)` 单方法**：UI 层用 `seq`（surface event seq）作为分支锚点——与 session log 事件序列号天然一致。

### laew gap 表（D3）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| L1466 | 无用户级 fork / branch 能力（仅能完整 `/clear` 开启新 session） | P1 | `rusqlite` + 事件 seq |
| L1467 | 无语义 checkpoint 失败闭合（崩溃后可能重做副作用） | P0 | `tokio::sync::Mutex` + WAL fsync |
| L1468 | 无嵌套 flush 复用（每次嵌套工具都 flush 父层） | P2 | tracing span + 父层缓存 |
| L1469 | 无 `forkAt(seq)` 单方法统一入口 | P2 | 设计层 |
| L1470 | 无 assistant message 独占分支（用户消息不应有 fork 入口） | P2 | 设计层 |

---

## D4 — 文件监视与工作区感知【运行时】

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/context/file-reference-local/src/index.ts` | L60-95 | 工作区索引失效总线 |
| `packages/context/file-reference-local/src/search.ts` | L80-180 | `WorkspaceFileSearch` 失效计数 |
| `packages/client/hmr/src/index.ts` | 全文 | HMR（hot module reload）——唯一 fs.watch 使用 |
| `packages/util/timeout/src/index.ts` | 全文 | timeout util |
| `packages/fs/tool-fs-search/tests/load-path.spec.ts` | 全文 | 路径搜索测试 |
| `packages/settings/settings-file/src/index.ts` | 全文 | settings 文件监听 |

### 机制剖析

**1. 工具结果触发失效**（`file-reference-local/src/index.ts:60-95`）：
```ts
ctx.on('session/event', (session, event) => {
  if (event.type !== 'tool/result') return
  const agent = ctx.agents.get(session.id)
  if (agent !== undefined) this.searches.get(agent)?.invalidate()
})
```
- **事件总线驱动**：监听 `session/event` 事件，**仅在 tool result 触发时**才调 `invalidate()`。
- 失效不立刻重扫——下一次"裸查询"才后台重建（见 D1 的机制 3）。
- 这是一种**懒失效**策略：用户操作产生 tool call → tool result → 标记 stale（不立即重扫） → 下次用户输入 `@` 时才感知陈旧。

**2. 失效计数器**（`search.ts:75-95`）：
```ts
class WorkspaceFileSearch {
  private settled: SettledIndex | undefined
  private invalidations = 0

  async indexFor(signal) {
    const settled = this.settled
    if (settled === undefined) return waitForPromise(this.ensureIndex(), signal)
    if (settled.startedAt < this.invalidations) {
      void this.ensureIndex().catch(() => { /* 静默失败 */ })
    }
    return settled.entries  // 老索引照常回答
  }
}
```
- **陈旧索引继续回答** + 后台静默重建——**用户零感知**。
- 这避免了"每次工具结果都阻塞 UI 重建索引"的问题。

**3. fs.watch 使用情况**（HMR + settings）：
- HMR 模块用 `fs.watch` 监控 client module 变化（**仅 HMR 内部用**，未用于工作区）。
- settings-file 用 `fs.watch` 监控 settings 文件变化——但**这是设置变化，不是工作区**。
- 工作区文件变化感知**完全靠 tool result 事件驱动**——**不依赖 chokidar/fs.watch**。

**4. 没有 chokidar 依赖**（检查 `package.json`）：
- `grep "chokidar"` 根 `package.json` 和 `pnpm-workspace.yaml` 都无结果。
- deepseek-harness 的工作区感知是 **"工具驱动"而非"OS 事件驱动"**——即每次 Bash/Read/Write 工具执行后，索引陈旧，**下一次 @file 查询时后台重建**。

### 设计巧妙点

1. **懒失效 + 后台重建**：把"工作区扫描"完全剥离到"用户下一次 @file 时"——**没有任何后台 fs 监听**。
2. **事件总线驱动**：复用 session event 流，**不引入新的 fs.watch 监听线程**——简化了部署、平台一致性。
3. **静默失败**：`void this.ensureIndex().catch(() => { ... })`——后台重建失败永远不冒到 UI。

### laew gap 表（D4）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| L1471 | 无工作区文件监视（无 chokidar / notify 监听） | P2 | `notify` v6 + `debounce` |
| L1472 | 无懒失效+后台重建（每次失效立即阻塞重扫） | P1 | `Arc<RwLock<Index>>` + `tokio::spawn` |
| L1473 | 无 tool result 事件总线驱动（手动调用 invalidate） | P2 | 设计层 |
| L1474 | 无 `fs.watch` 平台一致性（仅靠工具结果推断） | P3 | 评估 vs `notify` 跨平台 |

---

## D5 — 工具输出富文本内容渲染

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/client/ui-primitives/src/markdown/parse.ts` | 全文 | mdast 解析 |
| `packages/client/ui-primitives/src/markdown/render.tsx` | 全文 | mdast → React 树 |
| `packages/client/ui-primitives/src/markdown/MarkdownText.tsx` | 全文 | 主入口 |
| `packages/client/ui-primitives/src/markdown/highlight.ts` | 全文 | shiki 单例 |
| `packages/client/ui-primitives/src/markdown/CodeBlock.tsx` | 全文 | code block 渲染 |
| `packages/client/ui-primitives/src/markdown/JsonBlock.tsx` | 全文 | JSON 折叠/展开 |
| `packages/client/ui-primitives/src/markdown/incremental.ts` | 全文 | 流式增量解析 |
| `packages/client/ui-primitives/src/markdown/katex.tsx` | 全文 | 公式渲染 |
| `packages/client/ui-chat/src/client/chat/AssistantMarkdown.tsx` | 全文 | assistant 消息包装 |
| `packages/client/ui-tool/src/client/tool/components/ToolRow.tsx` | 全文 | 工具输出行 |
| `packages/client/ui-tool/src/client/tool/components/AskQuestionCard.tsx` | 全文 | 问题卡片 |

### 机制剖析

**1. mdast 解析**（`markdown/parse.ts` 全文）：使用 `parseGfm` + `parseGfmWithMath`——支持 GFM（含表格/任务列表/删除线/自动链接）+ LaTeX 数学公式（katex）。

**2. 流式增量解析**（`markdown/MarkdownText.tsx:8-20`）：
```ts
/**
 * While a message streams, all but the trailing two blocks freeze as cached
 * React elements and only the source tail behind them re-parses per chunk,
 * so per-chunk work tracks the tail size instead of the whole reply. Frozen
 * blocks keep their source-offset keys when they cross the freeze boundary,
 * so React reconciles instead of remounting.
 */
```
- **流式 O(tail)**：只重解析尾部两个 block 之前的缓存——前面已 React 缓存。
- **跨冻结边界保持 key**：source-offset keys 让 React 复用 DOM 而非重挂载。

**3. shiki 单例**（`markdown/highlight.ts:1-30`）：
```ts
/**
 * The client's ONE syntax highlighter: a synchronous fine-grained shiki core
 * (JavaScript regex engine — no oniguruma WASM, bundle-friendly) with an
 * explicit grammar allowlist and a CSS-variables theme. Colors live in the
 * theme package's token sheets as `--shiki-*` custom properties (light and
 * dark blocks), never here — the repo's tokens-only styling rule.
 */
```
- **JavaScript regex engine**：避免 oniguruma WASM——bundle 友好。
- **boot 3 grammar + lazy**：启动期加载 TS/Shell/JSON 三种；其他语言懒加载（"a session that never opens a read card in one of those languages pays neither the ~1.6 MB of grammar modules nor their synchronous init"）。
- 第一个 lazy 语言回退到 plain text，**加载完成通知订阅者重渲染**（`onGrammarLoaded`）。
- **未知语言永不报错**——`An unknown or absent language falls back to plain text`。

**4. JsonBlock 折叠/展开**（`JsonBlock.tsx`）：未读全文但功能从命名可见——支持 JSON 折叠/展开。

**5. ToolRow 组件**（`ui-tool/src/client/tool/components/ToolRow.tsx`）：每个 tool 调用的展示行；测试文件提到 `summarySuffix` 字段（`apps/web/tests/todo-row.expected.e2e.ts:9`）——非收缩的后缀。

**6. AskQuestionCard 组件**（`ui-tool/src/client/tool/components/AskQuestionCard.tsx`）：多选问题卡片——属于 A2UI 风格的"交互组件"。

**7. Katex 公式**（`markdown/katex.tsx`）：CSS-only 数学公式（`import 'katex/dist/katex.min.css'`）。

### 设计巧妙点

1. **流式 O(tail) 解析**：大消息分块到达时只重解析尾部——**性能与流式天然兼容**。
2. **Boot 3 + lazy N grammar**：所有 session 必须的 3 种强制加载，扩展集懒加载——首屏 bundle 极致小。
3. **同步 JS regex engine**：放弃 oniguruma WASM，避免异步高亮、避免大 wasm 体积。
4. **冻结 key 复用**：跨 chunk 边界的 stable key 让 React 增量更新，不重挂 DOM。
5. **未知语言静默降级**：从不抛错——"never an error"是核心 UX 原则。

### laew gap 表（D5）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| L1475 | 无 markdown 渲染（无 pulldown-cmark / comrak） | P1 | `pulldown-cmark` + `termimad` |
| L1476 | 无流式增量解析（每次完整消息重解析） | P1 | 自实现 tail reparse |
| L1477 | 无语法高亮（无 syntect / tree-sitter-highlight） | P1 | `syntect` + lazy 加载 |
| L1478 | 无 JSON 折叠/展开控件 | P2 | `serde_json` + `tui-textarea` |
| L1479 | 无数学公式渲染（katex / mathjax） | P3 | `latex2mathml` + `image` crate |

---

## D6 — 输入体验工程

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/client/ui-conversation/src/client/input/machine.ts` | 全文（240+ 行） | `SubmitMachine` 状态机 |
| `packages/client/ui-conversation/src/client/input/editor/keymap.ts` | 全文 | Lexical keymap |
| `packages/client/ui-conversation/src/client/contract/input.ts` | 全文 | 输入契约 |
| `packages/client/ui-conversation/src/client/input/submission-policy.ts` | 全文 | 提交策略 |
| `packages/client/ui-conversation/src/submission-settings.ts` | 全文 | 提交设置 |

### 机制剖析

**1. SubmitMachine 状态机**（`machine.ts:50-150`）：
- 阶段：`plain` / `claimed` / `adjudicating` / `submitting`
- `claimed`：声明一个 command token 后，**phase = claimed**；编辑器前缀被锁定。
- `submitting`：实际提交中，编辑器已 detach；**可接收新输入**（`detached` map）。
- `adjudicating`：触发 `/` 触发器后，轮询每个 source 的 `matchEnter`——第一个非 undefined 胜出。

**2. claimed 完整性**（`machine.ts:90-100`）：
```ts
private onDraftChanged(draft: string): readonly InputEffect[] {
  if (this.phase === 'claimed' && this.claim !== undefined && !draft.startsWith(this.claim.token)) {
    this.phase = 'plain'
    this.claim = undefined
  }
  return []
}
```
- **draft 破坏 token 前缀 → 自动释放 claim**——用户编辑会破坏声明的合法性。

**3. Lexical keymap**（`editor/keymap.ts:1-30`）：
```ts
/**
 * IME guard: a composition-closing Enter/Space must not submit or adjudicate.
 * KeyboardEvent.isComposing covers most engines; Safari delivers the closing
 * keydown AFTER compositionend, so a root-element composition watch holds the
 * guard for 10ms more (the old textarea's proven window); keyCode
 * 229 is the legacy signal engines emit without isComposing.
 */
```
- **IME 三重防护**：`isComposing` + `keyCode === 229` + `recentlyComposing()`（10ms window）。
- 注册优先级 `COMMAND_PRIORITY_CRITICAL`——在 Lexical 默认 keymap 之前。
- 拦截：`up`/`down`/`enter`/`escape`/`tab`/`space`/`paste`。

**4. 提交策略**（`submission-policy.ts` 全文）：
- `InputSubmitMode`：定义了不同的 enter 提交模式。
- 普通提交（`default-sink`）在编辑器 commit 之前**先抓拍 input**——保证 snapshot 与实际提交一致。

**5. 提交设置**（`submission-settings.ts` 全文）：用户可配置 enter 行为（如 shift+enter vs enter 提交）。

**6. 触发器优先级 vs Lexical 默认**（`keymap.ts:30-50`）：返回 `false` 落到 Lexical 默认；返回 `true` 表示"消费"——**两层 keymap 协作**而非替换。

### 设计巧妙点

1. **SubmitMachine 状态机**：`plain`/`claimed`/`adjudicating`/`submitting` 四相分离——比"只有 in-flight"更细。
2. **claimed 完整性 watchdog**：`onDraftChanged` 检测 draft 破坏前缀即释放——用户编辑期间声明失效是自然的。
3. **Detached 模式**：提交中编辑器已清空，**用户可继续输入**——并行心智。
4. **IME 三重防护**：isComposing + keyCode 229 + 10ms post-composition window——处理 Safari 异步 deliver。
5. **优先级 critical keymap**：在 Lexical 默认之前——`false` 落回默认（Shift+Enter 仍能换行）。

### laew gap 表（D6）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| L1480 | 无 IME 防护（中文/日文输入下 enter 可能误提交） | P1 | `crossterm` + composition state |
| L1481 | 无 submit 状态机（仅简单的"输入→回车"两态） | P1 | 自实现 + `tokio::sync::Mutex` |
| L1482 | 无 claimed 完整性 watchdog（编辑时声明不释放） | P2 | 设计层 |
| L1483 | 无 detached 模式（提交中编辑器仍 lock） | P1 | 设计层 |
| L1484 | 无 submit mode 用户配置（默认 enter 提交，无 shift+enter 换行等） | P2 | 配置层 |

---

## D7 — Onboarding / 目录信任 / 主题偏好

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/client/ui-settings-models/src/onboarding-copy.ts` | 全文（10 行） | welcome notice 版本 |
| `packages/client/ui-settings-models/src/client/WelcomeNotice.tsx` | 全文 | 欢迎模态 |
| `packages/client/ui-settings-models/src/client/welcome-store.ts` | 全文 | store |
| `packages/client/ui-theme/src/theme-settings.ts` | 全文 | 主题 schema |
| `packages/client/ui-theme/src/boot-theme.ts` | 全文 | 内联 boot script（无 FOUC） |
| `packages/client/ui-theme/src/client/index.ts` | L50-180 | theme registry |
| `packages/client/ui-theme/src/client/AppearanceRow.tsx` | 全文 | appearance 设置行 |
| `packages/interaction/user-approval/src/index.ts` | 全文 | approval 能力（信任） |

### 机制剖析

**1. 版本化 welcome notice**（`onboarding-copy.ts`）：
```ts
export const WELCOME_NOTICE_SETTINGS_NAMESPACE = 'ui-onboarding'
export const WELCOME_NOTICE_ACK_FIELD = 'welcomeNoticeVersion'
export const WELCOME_NOTICE_VERSION = '2026-08-13.1'
```
- **版本号机制**：每次 notice 内容变化 → 升版本号；用户确认的版本号持久化；启动时比对——只有不一致才显示。
- "Bump only when the notice changes materially and every user should see it again"。

**2. 欢迎确认状态机**（`WelcomeNotice.tsx:35-65`）：
```ts
const [state, setState] = useWelcome(snapshot => snapshot)
const finished = useRef(false)
useEffect(() => {
  if (state.status === 'idle') void controller.load()
}, [controller, state.status])
useEffect(() => {
  if (state.acknowledged) finish()
}, [finish, state.acknowledged])
```
- 状态机：`idle` → `loading` → 显示/隐藏 → `saving` → `finished`。
- 流程：load → 检查 ack 版本 → 显示 notice（若不匹配） → 用户点 continue → `acknowledge()` 写 durable ack。

**3. 主题 schema + 持久化**（`theme-settings.ts:50-67`）：
```ts
export const ThemeSettingsSchema: z<ThemeSettings> = z.object({
  [THEME_PREFERENCE_FIELD]: z.union([...THEME_PREFERENCES]).default(DEFAULT_PREFERENCE),
  [FONT_SIZE_FIELD]: z.number().step(1).min(FONT_SIZE_MIN).max(FONT_SIZE_MAX).default(DEFAULT_FONT_SIZE),
})
```
- **内置偏好**：`light` / `dark` / `system`——`system` 是首次加载时 `matchMedia` 解析。
- **字体大小**：12-17px，整数——硬边界。
- **durable schema**——Host 持久化、Client 校验共用。

**4. 无 FOUC 主题 boot**（`boot-theme.ts:1-30`）：
```ts
export function bootThemeInjection(
  preference: ThemePreference = DEFAULT_PREFERENCE,
  fontSize: number = DEFAULT_FONT_SIZE,
): IndexInjection {
  return { kind: 'script', placement: 'body', text: bootThemeScript(preference, fontSize) }
}
```
- **内联 boot script** 紧跟 `<body>` 打开后——React mount 前已设置 `data-ds-dark-theme` 和 `--dsh-content-font-size` CSS 变量。
- **不等待 React**——避免"白屏闪一下"。

**5. 主题 registry + override layers**（`ui-theme/src/client/index.ts:55-90`）：
```ts
export interface ThemeDefinition {
  id: string
  colorScheme: 'light' | 'dark'  // 基础调色板
  tokens: ThemeTokens  // alias-layer 覆盖
}
```
- **基础调色板 + alias 覆盖**两层叠加：基础永远是 light/dark 之一；用户/主题可在 alias 上覆盖 token。
- 切换通过 `body[data-ds-dark-theme]` —— **never from the id**（id 仅作为 label 存在，调色板只来自 colorScheme 字段）。

**6. Approval（目录信任的轻量版）**（`user-approval/src/index.ts`）：
- `ApprovalPolicy: 'ask' | 'never'`——会话级策略。
- "Missing answerers fail closed; grants apply only to the requested action."
- **不持久化 directory trust**——每次操作都问，除非用户选"never"。
- 配合 `permission-presets`：4 档预设（`CUSTOM_PRESET` 兜底）组合 sandbox + approval。

**7. 目录信任 — 不在**（grep `directory.?trust|trusted.?directory`）：
- deepseek-harness **没有"目录信任"机制**——每次 tool 调用都通过 approval 流程决定（`sandbox` + `approval` 二元）。
- 这是 Web-first 工程的取舍：Web 用户对"信任当前目录"概念弱（每次操作都是新页面/新会话），CLI 用户对此敏感。

### 设计巧妙点

1. **版本化 welcome notice**：内容变化才升版本——避免每次启动都打扰用户。
2. **durable schema = wire envelope**：`ThemeSettingsSchema` 既是 Host 持久化、又是 Client 校验——单一真理源。
3. **无 FOUC 主题**：内联 script 优先于 React mount——视觉连续性。
4. **基础调色板 + alias 覆盖**：`colorScheme` 与 `id` 解耦——用户主题插件只需声明 alias token，覆盖 light/dark 两套（避免单边失明）。
5. **Approval fail-closed**：未配置 answerers 时一律拒绝——**默认安全**。

### laew gap 表（D7）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| L1485 | 无 welcome notice / 首次引导（CLI 启动直接进 REPL） | P2 | 设计层 + ratatui modal |

> 注：此维度 deepseek 实现了版本化 welcome + 主题持久化 + 无 FOUC boot + 主题 override——但**没有目录信任**（Web 工程不需要）。laew 当前也没目录信任，未来是否需要可参考 deepseek 的"每次操作 approval"模式。

---

## D8 — 会话导出共享 + statusline + 实时成本

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/session-query/session-log-export/src/index.ts` | 全文（70 行） | `/export` 命令 + 路由 |
| `packages/session-query/session-log-export/src/archive.ts` | 全文 | fflate 流式 ZIP |
| `packages/llm/token-meter/src/breakdown-projection.ts` | 全文 | context 组成投影 |
| `packages/llm/token-meter/src/usage-projection.ts` | 全文 | 用量投影 |
| `packages/llm/token-meter/src/route-pricing.ts` | 全文 | 路由定价 |
| `packages/llm/token-meter/src/surface-projection.ts` | 全文 | O(1) 表面 token 折叠 |
| `packages/client/ui-chat/src/client/chat/StatsLine.tsx` | 全文（200+ 行） | chat 状态行 |
| `packages/client/ui-chat/src/client/chat/ContextBody.tsx` | 全文 | 上下文组成展示 |
| `packages/client/ui-trajectory/src/client/trajectory-event-projection.ts` | 全文 | 事件投影 |

### 机制剖析

**1. `/export` 命令 + 流式 ZIP**（`session-log-export/src/index.ts:60-70`）：
```ts
ctx.effect(() => ctx.commands.register({
  name: 'export',
  description: 'Download this Session log as a ZIP archive',
  handler: invocation => Promise.resolve(invocation.rawInput.trim() === ''
    ? REQUESTED
    : { kind: 'error', text: 'The Web /export command does not accept a path.' }),
}), 'session-log-download: command')
```
- **命令注册** + **路由注册**（`/api/session.export`）双绑定。
- 命令不接受 path（Web-only 固定路由）。

**2. fflate 流式 ZIP**（`archive.ts:1-30`）：
```ts
/**
 * Compression runs on the host with fflate's streaming Zip API, so the archive
 * bytes are produced incrementally and the host never holds the whole archive
 * in one buffer; production waits for consumer pull whenever the response queue
 * reaches its byte high-water mark, so a slow consumer bounds accumulation to
 * the fixed 64 KiB response queue plus one synchronous fflate push.
 */
```
- **流式压缩**：fflate 同步 push + 64 KiB 响应队列背压。
- **慢消费者自适应**：超过高水位暂停生产——host 永不 OOM。
- 子代 lineage + 媒体对象**内容寻址**——共享图片不重复（`media/<attachmentId>.<ext>`）。

**3. Context breakdown 投影**（`breakdown-projection.ts:30-50`）：
```ts
const contextBreakdownStateSchema = z.object({
  systemTokens: tokenCount,
  toolsTokens: tokenCount,
  messageTokens: tokenCount,
  claim: z.object({ start: tokenCount, end: tokenCount, tokens: tokenCount }).optional(),
}).strict()
```
- **三段构成**：`system` / `tools` / `messages`——可解释的 context 占用。
- `claim` 是 shadow-price 协议的状态（见 surface-projection）。
- 状态版本 `stateVersion: 2`——Schema 演进有版本。

**4. 路由定价**（`route-pricing.ts:30-60`）：
```ts
export function priceSurface(nodes, pricing): PricedSurface {
  const images = pricing === undefined ? [] : nodes.flatMap(node => node.images)
  if (pricing === undefined || images.length === 0) {
    // 固定启发式
  }
  // 用路由价格替换每张图
  // ...
  if (prices.length !== images.length) {
    throw new Error(`token meter: route image pricing answered ${prices.length} prices for ${images.length} occurrences`)
  }
}
```
- **provider 中立回退**：无定价时用固定启发式。
- **图像路由定价**：provider 声明 `LlmImageRequestPricing` 后，每张图按路由价 + 模型可见文本计价。
- **不匹配必抛**：长度不齐 throw——misprice 比不 price 更糟。

**5. O(1) 表面 token 折叠**（`surface-projection.ts:1-20`）：
- **shadow-price 协议**：compaction 紧邻事件前发 `compaction/summary` 或 `compaction/prune` 声明被替换区间的启发式价格；折叠函数保留 at most one claim + 累计 total。
- **bounded state 关键**：投影状态会被持久化到 cache，**不能随 session 增长**——必须 O(1)。

**6. StatsLine 实时显示**（`ui-chat/src/client/chat/StatsLine.tsx:140-200`）：
```ts
// Pipe-separated groups (figma stats strip); a group with no data drops out whole.
const groups: string[] = []
if (stats.steps > 0) {
  groups.push(t('stats.counts', { turns: stats.turns, steps: stats.steps }))
  // 持续时间、ttft、tps 累加
  if (usage !== undefined && (billedInputTokens(usage) > 0 || usage.outputTokens > 0)) {
    // cache hit % + tokens
  }
}
```
- **pipe-separated groups**：每组空时整组消失——视觉上"该有的都有，不该有的不出现"。
- **durable projection 优先**：账单数据从 `tokenUsage` projection 来（`useProjection('tokenUsage')`）—— paging + compaction 不影响计数。
- **fallback 折叠**：无 projection 服务时用 `deriveStats(nodes)` —— 字段名镜像 projection，**两个数据源字段一致可整体切换**。
- **elide + tooltip**：超长时省略号 + hover tooltip 完整内容（带 500ms delay）；`disabled={!truncated}`。
- **位置**：`conversation.composer.dock`——粘在 composer 同侧，跟随会话 scrollport。

**7. Cache hit 百分比**（`StatsLine.tsx:104-130`）：
- 分子：`cacheReadTokens`
- 分母：`uncachedInputTokens + cacheReadTokens + cacheWriteTokens`（三段 disjoint）
- 显示精度：integer 时 < 100；否则最小精度；100% 命中 → "100"；无计费 → `null`（不显示）。

### 设计巧妙点

1. **fflate 流式 + 64 KiB 背压**：host 内存有界——大 session 导出也不 OOM。
2. **shadow-price 协议**：compaction 紧邻声明价格让 fold 保持 O(1)——不存历史。
3. **路由定价 vs 启发式**：`pricing === undefined` 走固定启发式；否则逐图替换——provider 中立。
4. **pipe-separated + 整组消失**：stats 永远"该有的都有"——视觉简洁。
5. **durable projection 优先 + fallback 折叠**：durable 优先保证 paging/compaction 不影响；fallback 字段名镜像保证可整体切换。
6. **超长 elide + tooltip**：先 ellipsize，再 hover 显全——不挤压布局。

### laew gap 表（D8）

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|----------|----------------|
| **（本维度 7 个候选 gap 已超出 L1456-L1485 区间，保留为分析叙述，下一轮合并入 L1496+ 区间）** | — | — | — |

> D8 候选 gap（叙述级）：
> - 无 `/export` 会话导出（无 fflate 流式 ZIP） → `zip` crate + `async-stream`
> - 无 subagent lineage 包含（导出仅顶层） → 设计层
> - 无 context breakdown 投影（system/tools/messages 三段不可视） → `tiktoken-rs` + 自实现 fold
> - 无路由定价（无 provider 声明 image 价格时回退） → 设计层
> - 无 O(1) 表面 token 折叠（session 越长 cost 估算越慢） → `Arc<RwLock<Claim>>`
> - 无 pipe-separated stats line（无 turn/step/timing 实时显示） → `ratatui` + tracing
> - 无 cache hit % 显示（用户看不到 cache 节省） → 设计层

---

## 末章 — gap 汇总与设计建议

### 18 轮累计 gap 数

| 轮次 | 范围 | 数量 |
|------|------|------|
| 第十五轮 | L836-L1035 | 200 |
| 第十六轮 | L1036-L1165+ | 130+ |
| 第十七轮 | L1166-L1395+ | 230+ |
| **第十八轮（本轮）** | **L1456-L1485** | **30**（实测 30 个） |

> 区间保留 L1396-L1455 给未来轮次（18 轮 + 后续合并使用）。

### 维度优先级矩阵

| 维度 | 数量 | 最高优先级 | 立即可做 |
|------|------|------------|----------|
| D1 @ 提及 | 5 | P1 | L1458（事件总线驱动失效） |
| D2 命令 | 5 | P1 | L1461（运行时命令注册） |
| D3 分支 | 5 | **P0** | **L1467（语义 checkpoint 失败闭合）** |
| D4 工作区感知 | 4 | P1 | L1472（懒失效+后台重建） |
| D5 富文本渲染 | 5 | P1 | L1475（markdown 渲染） |
| D6 输入体验 | 5 | P1 | L1480（IME 防护） |
| D7 Onboarding | 1 | P2 | L1485（welcome notice） |
| D8 导出/状态/成本 | 0（7 候选保留入 L1496+） | — | D8 仅叙述级，下轮补 |

### 关键设计建议（对 laew）

1. **Web → TUI 范式翻译清单**：
   - `Lexical Editor` → `tui-textarea` 或 `crossterm` raw mode
   - `React Portal` → `engine::Screen` 栈（laew 已有）
   - `slot 注入` → `handle_slash` 路由（laew 已有）
   - `dispatch` 事件 → `mpsc::Sender<AgentEvent>`（laew 已有 Cordis-like 模式可借鉴）
   - `ObservableSnapshot` → `Arc<RwLock<State>>`

2. **最有借鉴价值的 5 个模式**：
   - **代际（generation）防 stale**：单个递增数 + reducer 静默丢弃——比事件序列号简单。
   - **懒失效 + 后台重建**：`invalidate()` 不清空，**下次查询后台刷新区分代际**——把重索引成本完全剥离到用户输入延迟。
   - **shadow-price 协议**：compaction 紧邻声明价格让 fold 保持 O(1)——bounded state 关键。
   - **claimed 完整性 watchdog**：draft 破坏前缀自动释放 claim——状态机完整性的自然表达。
   - **durable schema = wire envelope**：Host 持久化与 Client 校验共用 schema——单一真理源。

3. **范式差异注意点**：
   - **Web 无 TUI 概念**——所有"屏幕"是 React Portal + slot 渲染。
   - **Web 无目录信任**——每次操作 approval（`sandbox` + `approval` 二元）。
   - **Web 强 IME 防护**（多浏览器+多引擎）——laew 单终端也需防护（中文/日文 IME）。
   - **Web 流式解析**（Lexical/mdast）——laew 终端需 `tui-textarea` 风格。

4. **laew 1 周可做的 3 个改进**（基于本轮 gap）：
   - **L1467（语义 checkpoint 失败闭合，P0）**：在 laew `agent/mod.rs` 的 `run_session` 循环里，**tool 执行前**先 `db.flush_async()` 一次，**失败时 return error 不执行**。当前 laew 缺此保护——崩溃后可能重做副作用。
   - **L1480（IME 防护，P1）**：在 `tui/input.rs` 加 `is_composing` 状态机，**composition 中不消费 Enter/Space**——`crossterm` 不直接给 isComposing，需要自己维护 state。
   - **D8 候选（context breakdown 投影，P1）**：在 `tui` 加一个 `ContextMeter` 面板，分三段显示 system/tools/messages——用 `tiktoken-rs` 计算。**让用户看到"为什么 context 满了"**。

5. **3-6 月中长期**：
   - **L1456（@file 提及）** + **L1458（事件总线驱动失效）**：让 TUI 输入行支持 `@<Tab>` 路径补全，且每次 tool result 触发工作区索引后台重建。
   - **L1475（markdown 渲染）** + **L1477（语法高亮）**：用 `pulldown-cmark` + `syntect`（lazy）让 laew TUI 支持 assistant 输出 markdown + code block 高亮。
   - **D8 候选（/export）** + **D8 候选（O(1) surface fold）**：用 `zip` crate + `Arc<RwLock<Claim>>` 实现 `/export` + 实时 context 估算不随 session 增长。

### 一句话总结

> deepseek-harness 在第十八轮的 8 维度展现了**Web-first 工程如何把"用户体验"做成严谨的形式化契约**：代际防 stale、shadow-price 协议、claimed 完整性 watchdog、durable schema = wire envelope、版本化 welcome notice、pipe-separated stats with durable projection priority、fflate 流式 + 64 KiB 背压、boot 主题内联避免 FOUC。
> 对 laew（TUI 为主）的核心启示：**用"显式状态机 + 单调计数器 + bounded fold + 单事件源"替代"无意识副作用"**——每个 UX 维度都该有可重放的契约、可验证的不变量、可解释的投影。
