# opencode 源码深度分析(8 维度)

> 分析对象:`/usr/local/LsmGitOpenSource/opencode`(monorepo,核心包 `packages/opencode`)
> 语言/运行时:TypeScript / Bun + Effect(函数式 Effect 系统)+ AI SDK(Vercel)
> 架构特征:**无"Yolo 入口 Agent"、无独立 QC Agent、无任务三档分类**——与 `laew` 的多 Agent 架构形成鲜明对比。opencode 是**单主 Agent + 多子 Agent 模式**,靠 `plan mode`、`compaction`、`permission`、`skill` 四大机制保障质量。
> 分析日期:2026-09-04

---

## 1. 多轮对话的实现

opencode 没有独立的"对话管理 Agent",多轮对话由 **Session + Processor + LLM Stream** 三层协作实现,核心是一个 `while(true)` 主循环。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `session/prompt.ts` | 1052–1071 | `prompt()` 入口:创建 user message → 调用 `loop()` |
| `session/prompt.ts` | 1081–1341 | `runLoop()` —— **多轮对话主循环**(核心) |
| `session/prompt.ts` | 1343–1347 | `loop()` 包装,通过 `state.ensureRunning()` 保证单轮串行 |
| `session/processor.ts` | 81–710 | `SessionProcessor` —— 单轮 LLM 流式处理 + 工具调度 |
| `session/llm.ts` | 357–384 | `LLM.stream()` —— 统一流输出(`native` / `ai-sdk` 双运行时) |
| `session/run-state.ts` | – | `SessionRunState` —— 会话运行态互斥锁 |

### 关键代码片段

**主循环(`prompt.ts:1081`)**
```ts
const runLoop = Effect.fn("SessionPrompt.run")(function* (sessionID) {
  let step = 0
  while (true) {
    yield* status.set(sessionID, { type: "busy" })
    let msgs = yield* MessageV2.filterCompactedEffect(sessionID)
    const { user: lastUser, assistant: lastAssistant, finished, tasks } = MessageV2.latest(msgs)

    // 退出条件:assistant finish 不是 tool-calls/unknown,且无待执行 tool
    if (lastAssistant?.finish && !["tool-calls", "unknown"].includes(lastAssistant.finish)
        && !hasToolCalls && lastAssistant.parentID === lastUser.id) {
      break
    }
    // 子任务 / 压缩 / 溢出 → continue; 否则调 LLM
    if (task?.type === "subtask") { yield* handleSubtask(...); continue }
    if (task?.type === "compaction") { ...; continue }
    if (finished && (yield* compaction.isOverflow(...))) { yield* compaction.create(...); continue }

    const handle = yield* processor.create({ assistantMessage: msg, sessionID, model })
    const result = yield* handle.process({ user, agent, system, messages, tools, model })
    if (result === "stop") return "break"
    if (result === "compact") { yield* compaction.create(...); return "continue" }
  }
})
```

**Processor 单轮处理(`processor.ts:641`)**
```ts
const process = function* (streamInput) {
  return yield* Effect.gen(function* () {
    const stream = llm.stream(streamInput)
    yield* stream.pipe(Stream.tap(handleEvent), Stream.takeUntil(() => ctx.needsCompaction), Stream.runDrain)
  }).pipe(Effect.retry(SessionRetry.policy(...)), Effect.catch(halt), Effect.ensuring(cleanup()))
  if (ctx.needsCompaction) return "compact"
  if (ctx.blocked || ctx.assistantMessage.error) return "stop"
  return "continue"
}
```

### 关键设计要点

1. **循环退出靠 `finish` reason**:模型返回 `stop` / `length` / `content-filter` 等非 `tool-calls` 且无未执行 tool 时退出(`prompt.ts:1111`)。
2. **三种循环内事件**:`subtask`(子 Agent)、`compaction`(压缩)、普通 LLM 调用——三者共享同一个 `while(true)` 循环,通过 `continue` 复用上下文(`prompt.ts:1144–1168`)。
3. **串行保证**:`SessionRunState.ensureRunning()` 保证同一 session 同时只有一个 loop 在跑,避免并发写 DB。
4. **中断处理**:`Effect.onInterrupt` + `cleanup()` 保证异常时 tool 状态被标记 `error` + `interrupted`,下一轮可识别孤立 tool(`prompt.ts:99`,`isOrphanedInterruptedTool`)。
5. **maxSteps 限制**:`agent.steps ?? Infinity`,到达上限时注入 `MAX_STEPS_PROMPT` 提示(`prompt.ts:1281`)。

---

## 2. Context 的管理和实现

Context 管理是 opencode 最复杂的子系统,采用 **overflow 检测 + prune(剪枝) + compaction(摘要压缩) + tail budget 保留** 四层策略。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `session/overflow.ts` | 1–34 | `usable()` / `isOverflow()` —— 基于 token 的溢出检测 |
| `session/compaction.ts` | 1–608 | `SessionCompaction` —— 摘要压缩 + prune + tail 选择 |
| `session/compaction.ts` | 28–33 | 常量:`PRUNE_MINIMUM=20K`、`PRUNE_PROTECT=40K`、`TOOL_OUTPUT_MAX_CHARS=2K` |
| `session/compaction.ts` | 223–269 | `select()` —— tail budget 选择算法 |
| `session/compaction.ts` | 273–317 | `prune()` —— 旧 tool output 擦除 |
| `session/compaction.ts` | 319–557 | `process()` —— 执行 compaction(调 LLM 生成摘要) |
| `session/processor.ts` | 485–497 | step-finish 时检测 overflow,触发 compaction |
| `session/prompt.ts` | 1161–1168 | 循环内检测 overflow → 创建 compaction 任务 |

### 关键代码片段

**overflow 检测(`overflow.ts:10`)**
```ts
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

**tail 选择算法(`compaction.ts:223`)**
```ts
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

**prune 擦除旧 tool output(`compaction.ts:273`)**
```ts
const prune = function* ({ sessionID }) {
  // 从后往前遍历,跳过最近 2 轮 + 已 compacted + skill 工具
  for (msgIndex 从后往前) {
    if (msg.role === "user") turns++
    if (turns < 2) continue
    if (msg.role === "assistant" && msg.summary) break
    for (partIndex 从后往前) {
      if (part.type !== "tool" || part.state.status !== "completed") continue
      if (PRUNE_PROTECTED_TOOLS.includes(part.tool)) continue  // "skill" 受保护
      if (part.state.time.compacted) break
      total += Token.estimate(part.state.output)
      if (total <= PRUNE_PROTECT) continue  // 40K 保护阈值
      toPrune.push(part)
    }
  }
  if (pruned > PRUNE_MINIMUM) { /* 擦除 output,标记 compacted */ }
}
```

### 关键设计要点

1. **双阶段压缩**:`prune` 先擦除旧 tool output(轻量、同步),`process` 再调 LLM 生成摘要(重量、异步)。
2. **tail budget 动态计算**:`preserveRecentBudget()` 取 `usable*0.25` 并 clamp 到 `[2K, 15K]`,保证近期上下文不被压缩。
3. **turn 内切分**:`splitTurn()` 在单个 turn 内二分查找切分点,最大化利用 budget(`compaction.ts:140`)。
4. **compaction 也是 Agent**:使用隐藏的 `compaction` agent(`agent.ts:219`)调用 LLM 生成摘要,复用 `SessionProcessor` 流程。
5. **overflow 触发时机**:在 `step-finish` 事件(`processor.ts:491`)和循环顶部(`prompt.ts:1161`)双重检测。
6. **compaction 后自动继续**:`result === "continue" && auto` 时注入 synthetic user message "Continue if you have next steps"(`compaction.ts:468–549`)。

---

## 3. Yolo 识别 / 任务分类

**opencode 没有 Yolo Agent、没有意图识别、没有任务三档分类(simple/medium/hard)。** 这是与 `laew` 最大的架构差异。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `agent/agent.ts` | 140–265 | 内置 Agent 定义(无 Yolo) |
| `session/prompt.ts` | 1052–1071 | `prompt()` 直接进 loop,无前置分类 |
| `session/system.ts` | – | 系统提示词组合(无分类逻辑) |

### 关键代码片段

**内置 Agent 列表(`agent.ts:140`)**
```ts
const agents: Record<string, Info> = {
  build:        { mode: "primary", ... },     // 默认执行 Agent
  plan:         { mode: "primary", ... },     // 规划模式
  general:      { mode: "subagent", ... },    // 通用子 Agent
  explore:      { mode: "subagent", ... },    // 代码探索子 Agent
  compaction:   { mode: "primary", hidden: true, prompt: PROMPT_COMPACTION },
  title:        { mode: "primary", hidden: true, prompt: PROMPT_TITLE },
  summary:      { mode: "primary", hidden: true, prompt: PROMPT_SUMMARY },
}
```

### 关键设计要点

1. **无入口 Agent**:用户输入直接进 `prompt()` → `loop()`,不做意图识别 / 任务分类。
2. **无"目的→目标→意图"三步分析**:与 `laew` 的 Yolo 三步分析完全相反。
3. **任务路由靠 Agent 选择**:由用户或配置选择 `build` / `plan` / `general` / `explore` 等 Agent,而非自动分类。
4. **Plan mode 是 Agent 而非流程**:`plan` agent 只是禁用了 edit 工具 + 允许写 `.opencode/plans/*.md` 的 Agent(`agent.ts:156–181`),不是独立的规划流程。
5. **子 Agent 选择靠 LLM**:主 Agent 通过 `task` 工具 + `subagent_type` 参数选择子 Agent,由 LLM 决定用哪个。

> **对比 `laew`**:`laew` 用 Yolo Agent 做三档分类(simple→SubAgent / medium→Main→SubAgent / hard→Plan→Main→SubAgent),opencode 把这个决策交给了 LLM + 用户显式选择。

---

## 4. 质检检查(QC)

**opencode 没有独立的 QC Agent、没有 output 验证机制。** 质量保障靠 **permission 门控 + doom_loop 检测 + schema 校验 + 工具白名单** 实现。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `session/processor.ts` | 29, 356–380 | `DOOM_LOOP_THRESHOLD=3` —— 死循环检测 |
| `permission/` | – | 工具权限规则引擎 |
| `agent/agent.ts` | 119–136 | 默认权限集(`*` allow,`doom_loop` ask,`*.env` ask) |
| `tool/task.ts` | 119–129 | `task` 工具执行前 `ctx.ask()` 权限检查 |
| `tool/tool.ts` | 24–33 | `InvalidArgumentsError` —— schema 校验失败反馈 |

### 关键代码片段

**doom_loop 检测(`processor.ts:356`)**
```ts
case "tool-call": {
  yield* ensureToolCall(value)
  const parts = yield* MessageV2.parts(ctx.assistantMessage.id)
  const recentParts = parts.slice(-DOOM_LOOP_THRESHOLD)  // 最近 3 个 part
  if (recentParts.length === DOOM_LOOP_THRESHOLD &&
      recentParts.every((part) =>
        part.type === "tool" && part.tool === value.name &&
        part.state.status !== "pending" &&
        JSON.stringify(part.state.input) === JSON.stringify(input))) {
    // 连续 3 次相同工具 + 相同输入 → 触发 doom_loop 权限询问
    yield* permission.ask({ permission: "doom_loop", patterns: [value.name], always: [value.name], ruleset: agent.permission })
  }
}
```

**默认权限集(`agent.ts:119`)**
```ts
const defaults = Permission.fromConfig({
  "*": "allow",
  doom_loop: "ask",
  external_directory: { "*": "ask", ... },
  question: "deny",
  plan_enter: "deny",
  plan_exit: "deny",
  read: { "*": "allow", "*.env": "ask", "*.env.*": "ask", "*.env.example": "allow" },
})
```

### 关键设计要点

1. **无 QC Agent**:没有"每个执行单元完成后必经 QC"的环节,与 `laew` 的 Quality-Check Agent 不同。
2. **doom_loop 检测作为兜底**:连续 3 次相同工具调用 + 相同输入 → 触发权限询问,防止 LLM 死循环(`processor.ts:356`)。
3. **permission 作为主要门控**:每个工具执行前可调用 `ctx.ask()` 请求用户确认,规则分 `allow` / `ask` / `deny` 三档。
4. **schema 校验是工具级**:`Tool.define()` 的 `parameters` Schema 在 `tool.ts:111` 解码,失败抛 `InvalidArgumentsError` 让 LLM 重写。
5. **子 Agent 权限继承 + 限制**:`deriveSubagentSessionPermission()` 继承父 session 的 deny + external_directory 规则,默认禁用 `todowrite` 和嵌套 `task`(`subagent-permissions.ts:14–27`)。

---

## 5. 任务拆解

opencode 的任务拆解完全由 **LLM 通过 `task` 工具** 实现,没有独立的 Plan Agent 或任务分解引擎。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `tool/task.ts` | 1–372 | `TaskTool` —— 子 Agent 创建 + 执行 + 后台模式 |
| `tool/task.ts` | 43–62 | `Parameters` —— `prompt` / `subagent_type` / `task_id` / `background` |
| `tool/task.ts` | 92–117 | 子 Agent 深度限制 `subagent_depth` |
| `tool/task.ts` | 136–172 | `task_id` 恢复(复用旧 session) |
| `agent/subagent-permissions.ts` | 1–27 | 子 Agent 权限派生 |
| `session/prompt.ts` | 255–449 | `handleSubtask()` —— 子任务执行 + 结果回填 |

### 关键代码片段

**TaskTool 参数(`task.ts:43`)**
```ts
const BaseParameterFields = {
  description: Schema.String,        // 3-5 词描述
  prompt: Schema.String,             // 任务内容
  subagent_type: Schema.String,      // Agent 类型
  task_id: Schema.optional(Schema.String),  // 恢复旧 task
  command: Schema.optional(Schema.String),
}
export const Parameters = Schema.Struct({
  ...BaseParameterFields,
  background: Schema.optional(Schema.Boolean),  // 后台模式
})
```

**深度限制 + task_id 恢复(`task.ts:92`)**
```ts
const run = function* (params, ctx) {
  // 计算当前深度
  let current = parent; let depth = 0
  while (current.parentID) { depth++; current = yield* sessions.get(current.parentID) }
  if (depth >= (cfg.subagent_depth ?? 1)) return yield* Effect.fail(new Error("Subagent depth limit reached"))

  // task_id 恢复:复用旧 session
  const session = params.task_id
    ? yield* sessions.get(SessionID.make(params.task_id)).pipe(Effect.catchCause(() => Effect.succeed(undefined)))
    : undefined
  const nextSession = session ?? (yield* sessions.create({ parentID: ctx.sessionID, title, agent: next.name, permission }))
  ...
}
```

**子 Agent 权限派生(`subagent-permissions.ts:14`)**
```ts
export function deriveSubagentSessionPermission({ parentSessionPermission, subagent }) {
  const canTask = subagent.permission.some((rule) => rule.permission === "task")
  const canTodo = subagent.permission.some((rule) => rule.permission === "todowrite")
  return [
    ...parentSessionPermission.filter((rule) => rule.permission === "external_directory" || rule.action === "deny"),
    ...(canTodo ? [] : [{ permission: "todowrite", pattern: "*", action: "deny" }]),
    ...(canTask ? [] : [{ permission: "task", pattern: "*", action: "deny" }]),
  ]
}
```

### 关键设计要点

1. **LLM 驱动拆解**:主 Agent 通过 `task` 工具创建子 Agent,由 LLM 决定拆解策略、子任务内容、Agent 类型。
2. **`task_id` 恢复机制**:传入 `task_id` 复用旧 session,实现"继续之前的任务"(`task.ts:136`)。
3. **嵌套深度限制**:`subagent_depth` 默认 1,可配置,防止无限递归(`task.ts:111`)。
4. **后台子 Agent**:`background=true` 时通过 `BackgroundJob` 异步执行,完成后注入结果(`task.ts:227–319`)。
5. **子 Agent 独立 session**:每个子 Agent 创建独立 session,`parentID` 指向父 session,形成树形结构。
6. **结果包装**:子 Agent 输出包装为 `<task id=... state=...><task_result>...</task_result></task>` XML(`task.ts:64`)。

---

## 6. 任务分类

**opencode 没有任务分级、复杂度评估机制。** 任务分类靠 **Agent 类型选择 + Plan mode** 隐式实现。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `agent/agent.ts` | 140–265 | 内置 Agent(build / plan / general / explore / compaction / title / summary) |
| `tool/plan.ts` | 1–79 | `PlanExitTool` —— 退出 plan mode,切换到 build agent |
| `tool/plan-enter.txt` | – | 进入 plan mode 的提示词 |
| `tool/plan-exit.txt` | – | 退出 plan mode 的提示词 |

### 关键代码片段

**PlanExitTool(`plan.ts:15`)**
```ts
export const PlanExitTool = Tool.define("plan_exit", Effect.gen(function* () {
  return {
    execute: (_params, ctx) => Effect.gen(function* () {
      const plan = path.relative(instance.worktree, Session.plan(info, instance))
      const answers = yield* question.ask({
        questions: [{
          question: `Plan at ${plan} is complete. Would you like to switch to the build agent and start implementing?`,
          options: [
            { label: "Yes", description: "Switch to build agent and start implementing the plan" },
            { label: "No", description: "Stay with plan agent to continue refining the plan" },
          ],
        }],
      })
      if (answers[0]?.[0] === "No") yield* new Question.RejectedError()
      // 切换到 build agent
      yield* session.updateMessage({ agent: "build", model, ... })
      yield* session.updatePart({ text: `The plan at ${plan} has been approved, you can now edit files. Execute the plan`, synthetic: true })
    }),
  }
}))
```

**plan agent 权限(`agent.ts:156`)**
```ts
plan: {
  permission: Permission.merge(defaults, Permission.fromConfig({
    question: "allow",
    plan_exit: "allow",
    task: { general: "deny" },                  // 禁止 general 子 Agent
    edit: { "*": "deny",                        // 禁止所有 edit
            [".opencode/plans/*.md"]: "allow",  // 只允许写 plans
            [data/plans/*.md]: "allow" },
  })),
}
```

### 关键设计要点

1. **无自动复杂度评估**:与 `laew` 的 simple/medium/hard 三档分类不同,opencode 完全依赖用户或 LLM 选择 Agent。
2. **Plan mode 是 Agent 而非流程**:`plan` agent 只是禁用了 edit 工具,允许写 plans 文件,不是独立的规划引擎。
3. **Plan → Build 切换靠 `plan_exit` 工具**:LLM 调用 `plan_exit` 后,通过 `question.ask()` 让用户确认,然后切换到 `build` agent(`plan.ts:30–69`)。
4. **子 Agent 类型即分类**:`general`(通用)、`explore`(只读探索)等 Agent 类型隐式定义了任务类型。
5. **用户可自定义 Agent**:配置中 `cfg.agent` 可覆盖 / 新增 Agent,每个可指定 model / prompt / permission / mode(`agent.ts:267–294`)。

---

## 7. 工具调用

工具系统是 opencode 的核心,采用 **Tool trait + Registry + Schema + 流式 tool_use** 四层设计。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `tool/tool.ts` | 1–183 | `Tool` trait —— `define()` / `init()` / `Context` / `Def` |
| `tool/registry.ts` | 1–455 | `ToolRegistry` —— 工具注册 + 过滤 + 权限 |
| `tool/registry.ts` | 230–252 | 内置工具列表顺序 |
| `session/processor.ts` | 278–551 | `handleEvent()` —— 流式 tool_use 事件处理 |
| `session/llm.ts` | 280–354 | `streamText()` —— AI SDK 工具调度 |
| `tool/truncate.ts` | – | 工具输出截断 |

### 关键代码片段

**Tool trait(`tool.ts:151`)**
```ts
export function define<Parameters extends Schema.Decoder<unknown>, Result extends Metadata, R, ID extends string = string>(
  id: ID, init: Effect.Effect<Init<Parameters, Result>, never, R>,
): Effect.Effect<Info<Parameters, Result>, never, R | Truncate.Service | Agent.Service> & { id: ID } {
  return Object.assign(Effect.gen(function* () {
    const resolved = yield* init
    const truncate = yield* Truncate.Service
    const agents = yield* Agent.Service
    return { id, init: wrap(id, resolved, truncate, agents) }
  }), { id })
}
```

**wrap 函数 —— 统一截断 + 错误处理(`tool.ts:99`)**
```ts
function wrap(id, init, truncate, agents) {
  return () => Effect.gen(function* () {
    const toolInfo = typeof init === "function" ? { ...(yield* init()) } : { ...init }
    const decode = Schema.decodeUnknownEffect(toolInfo.parameters)  // 编译一次
    const execute = toolInfo.execute
    toolInfo.execute = (args, ctx) => Effect.gen(function* () {
      const decoded = yield* decode(args).pipe(Effect.mapError((error) => new InvalidArgumentsError({ tool: id, detail })))
      const result = yield* execute(decoded, ctx)
      const agent = yield* agents.get(ctx.agent)
      const truncated = yield* truncate.output(result.output, {}, agent)
      return { ...result, output: truncated.content, metadata: { ...result.metadata, truncated: truncated.truncated } }
    }).pipe(Effect.orDie, Effect.withSpan("Tool.execute", { attributes: { "tool.name": id, ... } }))
    return toolInfo
  })
}
```

**内置工具列表(`registry.ts:230`)**
```ts
builtin: [
  tool.invalid, tool.question, tool.shell, tool.read, tool.glob, tool.grep,
  tool.edit, tool.write, tool.task, tool.fetch, tool.todo, tool.search,
  tool.skill, tool.patch, tool.execute?, tool.lsp?, tool.plan?
]
```

**流式 tool_use 处理(`processor.ts:315`)**
```ts
case "tool-input-start": { yield* ensureToolCall(value); return }
case "tool-input-delta": { yield* ensureToolCall(value); return }
case "tool-input-end":   { yield* ensureToolCall(value); return }
case "tool-call": {
  yield* ensureToolCall(value)
  yield* updateToolCall(value.id, (match) => ({ ...match, tool: value.name, state: { status: "running", input, time: { start: Date.now() } } }))
  // doom_loop 检测(见维度 4)
}
case "tool-result": {
  const rawOutput = toolResultOutput(value)
  yield* completeToolCall(value.id, output)
}
```

### 关键设计要点

1. **Effect 系统深度集成**:每个工具 `execute` 返回 `Effect.Effect`,支持依赖注入、中断、重试。
2. **Schema 即类型**:用 `effect/Schema` 定义参数,编译时 + 运行时双重校验,失败抛 `InvalidArgumentsError`。
3. **统一截断**:`truncate.output()` 在 `wrap()` 中统一处理,所有工具输出自动截断(`tool.ts:135`)。
4. **流式 tool_use**:`tool-input-start/delta/end` 三事件逐步构建 tool call,UI 可实时展示(`processor.ts:315–329`)。
5. **工具可见性过滤**:`Permission.visibleTools()` 根据权限过滤工具列表,LLM 只能看到允许的工具。
6. **插件工具**:`fromPlugin()` 将插件的 Zod args 转为 JSON Schema,兼容旧接口(`registry.ts:125`)。
7. **模型特定工具**:GPT 系列用 `apply_patch` 替代 `edit` + `write`(`registry.ts:297–300`)。

---

## 8. MCP 设计与实现

MCP(Model Context Protocol)是 opencode 最重要的外部集成,支持三种 transport + OAuth + Catalog + 动态工具注册。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `mcp/index.ts` | 1–1004 | `MCP Service` —— 客户端管理 + 连接 + 状态 |
| `mcp/index.ts` | 218–370 | `connectTransport` / `connectRemote` / `connectLocal` |
| `mcp/index.ts` | 38–50 | `CLIENT_OPTIONS` —— capabilities(roots) |
| `mcp/catalog.ts` | 1–170 | `McpCatalog` —— 工具 / prompt / resource 列表 + 转换 |
| `mcp/catalog.ts` | 42–83 | `convertTool()` —— MCP tool → AI SDK dynamicTool |
| `mcp/auth.ts` | 1–163 | `McpAuth` —— OAuth token 存储 |
| `mcp/oauth-provider.ts` | – | OAuth provider 实现 |
| `mcp/oauth-callback.ts` | – | OAuth 回调处理 |
| `mcp/browser.ts` | – | 浏览器打开 OAuth 页面 |

### 关键代码片段

**三种 transport(`mcp/index.ts:212`)**
```ts
type Transport = StdioClientTransport | StreamableHTTPClientTransport | SSEClientTransport

const connectRemote = function* (key, mcp) {
  const url = remoteURL(mcp.url)
  const authProvider = !oauthDisabled ? new McpOAuthProvider(key, mcp.url, {...}, auth) : undefined
  const transports = [
    { name: "StreamableHTTP", transport: new StreamableHTTPClientTransport(url, { authProvider, requestInit }) },
    { name: "SSE",           transport: new SSEClientTransport(url, { authProvider, requestInit }) },
  ]
  for (const { name, transport } of transports) {
    const result = yield* connectTransport(transport, connectTimeout).pipe(Effect.catch((error) => {
      if (isAuthError) { pendingOAuthTransports.set(key, { transport }); lastStatus = { status: "needs_auth" } }
      ...
    }))
    if (result) return { client: result.client, status: { status: "connected" } }
    if (lastStatus?.status === "needs_auth" || lastStatus?.status === "needs_client_registration") break
  }
}

const connectLocal = function* (key, mcp) {
  const [cmd, ...args] = mcp.command
  const transport = new StdioClientTransport({ command: cmd, args, cwd, env: { ...process.env, ...mcp.environment } })
  return yield* connectTransport(transport, connectTimeout)
}
```

**convertTool —— MCP tool → AI SDK(`catalog.ts:42`)**
```ts
export function convertTool(mcpTool: MCPToolDef, client: Client, timeout?: number): Tool {
  const inputSchema: JSONSchema7 = {
    ...(mcpTool.inputSchema as JSONSchema7),
    type: "object",
    properties: (mcpTool.inputSchema.properties ?? {}) as JSONSchema7["properties"],
    additionalProperties: false,
  }
  return dynamicTool({
    description: mcpTool.description ?? "",
    inputSchema: jsonSchema(inputSchema),
    execute: async (args, options) => {
      const result = await client.callTool({ name: mcpTool.name, arguments: args || {} }, CallToolResultSchema, {
        resetTimeoutOnProgress: true, signal: options.abortSignal, timeout, onprogress: () => {},
      })
      if (result.isError) throw new Error(result.content.flatMap(...).join("\n\n") || "MCP tool returned an error")
      if (result.content.length > 0 || result.structuredContent === undefined) return result
      return { ...result, content: [{ type: "text", text: JSON.stringify(result.structuredContent) }] }
    },
  })
}
```

**状态机(`mcp/index.ts:83`)**
```ts
export const Status = Schema.Union([
  StatusConnected, StatusDisabled, StatusFailed, StatusNeedsAuth, StatusNeedsClientRegistration,
])
```

### 关键设计要点

1. **三种 transport 自动降级**:remote 先尝试 `StreamableHTTP`,失败降级 `SSE`;local 用 `Stdio`。
2. **OAuth 完整实现**:`McpOAuthProvider` + `McpOAuthCallback` + `McpAuth`(token 存储),支持动态客户端注册。
3. **needs_auth 状态**:OAuth 失败时进入 `needs_auth`,用户需手动 `opencode mcp auth <name>`(`mcp/index.ts:312`)。
4. **Catalog 分页 + 容错**:`paginate()` 处理 listTools/listPrompts/listResources,`TolerantListToolsResultSchema` 容忍 outputSchema 错误(`catalog.ts:14`)。
5. **工具名消毒**:`McpCatalog.sanitize()` 将 `[^a-zA-Z0-9_-]` 替换为 `_`,避免 LLM 工具名冲突(`catalog.ts:117`)。
6. **watch 机制**:监听 `ToolListChangedNotification`,动态更新工具列表(`mcp/index.ts:462`)。
7. **资源读取**:`readResource()` 支持 text + blob,限制 10MB,仅允许 pdf / gif / jpeg / png / webp(`prompt.ts:715–782`)。
8. **instructions 注入**:MCP server 的 `getInstructions()` 注入到系统提示词(`prompt.ts:1261`)。

---

## 9. SKILL 设计

Skill 是 opencode 的知识注入机制,采用 **双轨制(内置 + 外部)+ frontmatter + 目录扫描 + 远程拉取** 设计。

### 关键文件与行号

| 文件 | 行 | 职责 |
|---|---|---|
| `skill/index.ts` | 1–354 | `Skill Service` —— 发现 + 加载 + 查询 |
| `skill/index.ts` | 21–35 | 内置 `customize-opencode` skill |
| `skill/index.ts` | 173–233 | `discoverSkills()` —— 多目录扫描 |
| `skill/index.ts` | 105–140 | `add()` —— frontmatter 解析 + 去重 |
| `skill/discovery.ts` | 1–140 | `Discovery` —— 远程 skill 拉取 + 缓存 |
| `tool/skill.ts` | 1–70 | `SkillTool` —— 运行时加载 skill 到 prompt |
| `agent/agent.ts` | 368–436 | `Agent.generate()` —— LLM 运行时生成 Agent |

### 关键代码片段

**Skill 发现路径(`skill/index.ts:173`)**
```ts
const discoverSkills = function* (config, discovery, fsys, global, disableExternalSkills, disableClaudeCodeSkills, directory, worktree) {
  const state = { matches: new Set(), dirs: new Set() }
  const externalDirs = []
  if (!disableExternalSkills) {
    if (!disableClaudeCodeSkills) externalDirs.push(".claude")   // ~/.claude/skills/**/SKILL.md
    externalDirs.push(".agents")                                 // ~/.agents/skills/**/SKILL.md
    for (const dir of externalDirs) {
      yield* scan(state, path.join(global.home, dir), "skills/**/SKILL.md", { dot: true, scope: "global" })
    }
    const upDirs = yield* fsys.up({ targets: externalDirs, start: directory, stop: worktree })  // 向上查找
    for (const root of upDirs) { yield* scan(state, root, "skills/**/SKILL.md", { dot: true, scope: "project" }) }
  }
  const configDirs = yield* config.directories()            // {skill,skills}/**/SKILL.md
  for (const dir of configDirs) { yield* scan(state, dir, "{skill,skills}/**/SKILL.md") }
  for (const item of cfg.skills?.paths ?? []) { yield* scan(state, dir, "**/SKILL.md") }  // 自定义路径
  for (const url of cfg.skills?.urls ?? []) {              // 远程拉取
    const pulledDirs = yield* discovery.pull(url)
    for (const dir of pulledDirs) { yield* scan(state, dir, "**/SKILL.md") }
  }
  return { matches: Array.from(state.matches), dirs: Array.from(state.dirs) }
}
```

**frontmatter 解析 + 去重(`skill/index.ts:105`)**
```ts
const add = function* (state, match, events) {
  const md = yield* Effect.tryPromise({ try: () => ConfigMarkdown.parse(match), catch: (err) => err })
  if (!isSkillFrontmatter(md.data)) return  // 必须有 name
  if (state.skills[md.data.name]) { yield* Effect.logWarning("duplicate skill name", {...}) }  // 去重
  state.dirs.add(path.dirname(match))
  state.skills[md.data.name] = { name: md.data.name, description: md.data.description, location: match, content: md.content }
}
```

**SkillTool —— 运行时注入(`tool/skill.ts:12`)**
```ts
export const SkillTool = Tool.define("skill", Effect.gen(function* () {
  return {
    parameters: Schema.Struct({ name: Schema.String }),
    execute: (params, ctx) => Effect.gen(function* () {
      const info = yield* skill.require(params.name)
      yield* ctx.ask({ permission: "skill", patterns: [params.name] })
      const dir = path.dirname(info.location)
      const files = yield* ripgrep.find({ cwd: dir, pattern: "!**/SKILL.md", hidden: true, limit: 10 })
      return {
        title: `Loaded skill: ${info.name}`,
        output: [
          `<skill_content name="${info.name}">`,
          `# Skill: ${info.name}`, "", info.content.trim(), "",
          `Base directory for this skill: ${base}`,
          "Relative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.",
          "<skill_files>", files.map((file) => `<file>${path.resolve(dir, file.path)}</file>`).join("\n"), "</skill_files>",
          "</skill_content>",
        ].join("\n"),
      }
    }),
  }
}))
```

**Agent.generate() —— LLM 运行时生成 Agent(`agent.ts:368`)**
```ts
const generate = function* (input: { description: string; model? }) {
  const cfg = yield* config.get()
  const model = input.model ?? (yield* provider.defaultModel())
  const resolved = yield* provider.getModel(model.providerID, model.modelID)
  const language = yield* provider.getLanguage(resolved)
  const system = [PROMPT_GENERATE]  // generate.txt —— "You are an elite AI agent architect..."
  const existing = yield* InstanceState.useEffect(state, (s) => s.list())
  const params = {
    temperature: 0.3,
    messages: [
      ...system.map((item): ModelMessage => ({ role: "system", content: item })),
      { role: "user", content: `Create an agent configuration based on this request: "${input.description}".\n\nIMPORTANT: The following identifiers already exist and must NOT be used: ${existing.map((i) => i.name).join(", ")}` },
    ],
    model: language,
    schema: Object.assign(Schema.toStandardSchemaV1(GeneratedAgent), Schema.toStandardJSONSchemaV1(GeneratedAgent)),
  }
  return yield* Effect.promise(() => generateObject(params).then((r) => r.object))
  // 返回 { identifier, whenToUse, systemPrompt }
}
```

### 关键设计要点

1. **双轨制**:内置 skill(`customize-opencode`)+ 外部 skill(磁盘扫描 + 远程拉取),内置优先但可被覆盖(`skill/index.ts:277–284`)。
2. **多级发现**:全局(`~/.claude/skills/`、`~/.agents/skills/`)→ 项目向上查找 → 配置目录 → 自定义路径 → 远程 URL,共 5 级。
3. **frontmatter 契约**:`SKILL.md` 必须有 YAML frontmatter `name` + `description`,`ConfigMarkdown.ts` 解析。
4. **运行时注入**:`SkillTool` 被 LLM 调用时,将 skill 内容 + 文件列表包装为 `<skill_content>` XML 注入上下文(`skill.ts:47–61`)。
5. **Agent.generate() 用 `generateObject`**:通过结构化输出(`GeneratedAgent` schema)让 LLM 生成新 Agent 配置,返回 `{ identifier, whenToUse, systemPrompt }`。
6. **远程拉取 + 版本控制**:`Discovery.pull()` 从 URL 拉取 `index.json`,按 skill 名缓存,支持版本号 + 原子替换(`discovery.ts:49–132`)。
7. **权限控制**:`skill` 工具受 `permission` 规则约束,可 deny 特定 skill(`skill/index.ts:314`)。
8. **受保护工具**:`prune()` 中 `PRUNE_PROTECTED_TOOLS = ["skill"]`,skill 输出不会被擦除(`compaction.ts:31`)。

---

## 横向对比:opencode vs laew(多 Agent 架构)

| 维度 | opencode | laew |
|---|---|---|
| 入口 Agent | 无,直接进 `loop()` | Yolo Agent(意图识别 + 三档分类) |
| 任务分类 | 无,LLM 选择 Agent | simple / medium / hard 三档 |
| 任务拆解 | LLM 通过 `task` 工具 | Plan Agent(仅 hard) |
| 质检 | 无独立 QC Agent,靠 permission + doom_loop | Quality-Check Agent(每单元必检) |
| 会话摘要 | `summary.ts`(git diff 统计) | SessionContext Agent(写 session_memory) |
| 项目上下文 | `instruction.ts`(AGENTS.md / CLAUDE.md / CONTEXT.md) | 五级链(CLAUDE.md→AGENTS.md→README.md→自动生成→空) |
| 工具系统 | Effect + Schema + Registry | Rust trait + ToolRegistry |
| MCP | 三种 transport + OAuth + Catalog | 无(自建 Bash/Read/Write) |
| Skill | 双轨制 + frontmatter + 远程拉取 | 无 |
| Plan mode | `plan` Agent(禁 edit) | Plan Agent(输出 Markdown 方案) |
| 子 Agent | `task` 工具 + `subagent_type` + `task_id` | SubAgent-Work + Main-Work |
| 压缩 | prune + compaction + tail budget | 无(依赖模型 context) |

---

## 关键设计启示(对 laew 的借鉴)

1. **Effect 系统的应用**:opencode 用 Effect 管理所有副作用(HTTP、DB、文件),laew 可借鉴其 `Layer` 依赖注入模式。
2. **compaction 的四层策略**:overflow 检测 → prune → tail budget → LLM 摘要,比简单截断更精细。
3. **doom_loop 检测**:连续 N 次相同工具调用 + 相同输入 → 触发权限询问,简单有效。
4. **MCP 的 transport 降级 + OAuth**:StreamableHTTP → SSE → Stdio 自动切换,生产级实现。
5. **Skill 的双轨制 + 远程拉取**:内置 skill 兜底 + 外部 skill 可扩展,远程拉取 + 版本控制保证一致性。
6. **Agent.generate() 的 `generateObject`**:用结构化输出让 LLM 生成 Agent 配置,比 JSON 解析更可靠。
7. **permission 三档规则**:`allow` / `ask` / `deny` + 通配符匹配,比简单白名单更灵活。
8. **子 Agent 的 `task_id` 恢复**:复用旧 session 实现"继续任务",比每次新建 session 更优雅。

---

**分析完成**。共覆盖 9 个维度(多轮对话、Context、Yolo、质检、任务拆解、任务分类、工具调用、MCP、Skill),每个维度均提供文件路径 + 行号锚点 + 关键代码片段 + 设计要点。
