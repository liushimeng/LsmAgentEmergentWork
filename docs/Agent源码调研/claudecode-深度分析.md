# Claude Code 源码深度分析

> 分析对象:`/usr/local/LsmGitOpenSource/claudecode`(Anthropic 官方 Claude Code CLI 的开源/泄露版本)
> 分析日期:2026-09-04
> 分析维度:8 个(多轮对话 / Context 管理 / Yolo 识别 / 质检检查 / 任务拆解 / 任务分类 / 工具调用 / MCP 设计 / SKILL 设计)
> 注:本文件为对 claudecode 源码的独立深度分析,与 `claudecode-源码调研.md` 视角不同 —— 本文聚焦**关键文件路径、行号、代码片段、设计要点**。

---

## 0. 代码库全局结构速览

| 路径 | 作用 | 关键文件行数 |
|------|------|-------------|
| `src/query.ts` | 多轮对话 REPL 主循环 | 1729 |
| `src/QueryEngine.ts` | 查询引擎入口 | 1295 |
| `src/Tool.ts` | Tool trait 定义 + ToolUseContext | 792 |
| `src/tools.ts` | 工具注册表(内置 + MCP 合并) | 389 |
| `src/context.ts` | 系统/用户上下文 + CLAUDE.md 注入 | 189 |
| `src/services/compact/` | 四级压缩管线 | ~2400(多文件) |
| `src/services/mcp/client.ts` | MCP 客户端核心 | **3348** |
| `src/services/mcp/auth.ts` | MCP OAuth 流 | 2465 |
| `src/services/mcp/config.ts` | MCP 配置加载 | 1578 |
| `src/services/MagicDocs/` | Magic Docs 自动维护 | ~255 |
| `src/services/extractMemories/` | 会话记忆提取 | ~350+ |
| `src/services/tools/` | 工具编排 + 流式执行器 | 718 |
| `src/tools/AgentTool/` | 子 Agent 编排(多文件) | ~4093 |
| `src/coordinator/` | Coordinator 多工人编排 | 369 |
| `src/skills/` | 技能加载 + 内置技能 | ~700+ |

**统一消息模型**:`src/types/message.ts` 定义 `Message = UserMessage | AssistantMessage | SystemMessage | ProgressMessage | AttachmentMessage | TombstoneMessage | ToolUseSummaryMessage`。Agent 循环与工具层**永远不接触协议细节**,协议差异封闭在 `src/services/api/claude.ts` / `src/services/api/openai.ts` 两个客户端内部。

---

## 1. 多轮对话的实现

### 1.1 REPL 主循环位置

**核心入口**:`src/query.ts:219-239` 的 `query()` 函数,它包装了真正的 `queryLoop()`。

```typescript
// src/query.ts:219-239
export async function* query(params: QueryParams): AsyncGenerator<...> {
  const consumedCommandUuids: string[] = []
  const terminal = yield* queryLoop(params, consumedCommandUuids)
  for (const uuid of consumedCommandUuids) {
    notifyCommandLifecycle(uuid, 'completed')
  }
  return terminal
}
```

**真正的循环**:`src/query.ts:241-1728` 的 `queryLoop()` 是一个 `while(true)` 无限循环,每次迭代 = 一次 LLM API 调用 + 工具执行。

### 1.2 Turn 模型与 State 机

`src/query.ts:204-217` 定义了跨迭代的可变状态:

```typescript
type State = {
  messages: Message[]
  toolUseContext: ToolUseContext
  autoCompactTracking: AutoCompactTrackingState | undefined
  maxOutputTokensRecoveryCount: number
  hasAttemptedReactiveCompact: boolean
  maxOutputTokensOverride: number | undefined
  pendingToolUseSummary: Promise<ToolUseSummaryMessage | null> | undefined
  stopHookActive: boolean | undefined
  turnCount: number
  transition: Continue | undefined  // 上一次迭代为何继续
}
```

**设计要点**:`transition` 字段记录"为何继续"(如 `next_turn` / `max_output_tokens_recovery` / `reactive_compact_retry` / `collapse_drain_retry` / `stop_hook_blocking` / `token_budget_continuation`),既用于恢复路径判断,也让测试可以断言某条恢复路径被触发,而无需检查消息内容。

### 1.3 流式处理

`src/query.ts:659-863` 是核心流式消费段:

```typescript
for await (const message of deps.callModel({
  messages: prependUserContext(messagesForQuery, userContext),
  systemPrompt: fullSystemPrompt,
  tools: toolUseContext.options.tools,
  ...
})) {
  // backfillObservableInput:在 yield 前克隆并回填衍生字段,原始消息不动(保护 prompt cache)
  // withheld:暂扣可恢复错误(prompt-too-long / max-output-tokens / media-size),等恢复路径用尽再 surface
  if (!withheld) { yield yieldMessage }
  if (message.type === 'assistant') {
    assistantMessages.push(message)
    // 收集 tool_use blocks
    toolUseBlocks.push(...msgToolUseBlocks)
    needsFollowUp = true
    // 喂给流式执行器
    if (streamingToolExecutor) {
      for (const toolBlock of msgToolUseBlocks) {
        streamingToolExecutor.addTool(toolBlock, message)
      }
    }
  }
}
```

**关键设计 —— 错误暂扣(Withholding)**:`src/query.ts:175-179` 定义了 `isWithheldMaxOutputTokens`,以及 `reactiveCompact.isWithheldPromptTooLong`、`contextCollapse.isWithheldPromptTooLong`、`reactiveCompact.isWithheldMediaSizeError`。这些错误不会立刻 surface,而是先尝试 reactive compact / collapse drain / max_output_tokens_escalate 等恢复路径,只有在恢复用尽后才真正 yield 给调用方。这避免了 SDK 调用方(如 cowork/desktop)在看到 `error` 字段时立即终止会话 —— 恢复循环仍在跑,但已无人监听。

### 1.4 Steering 队列(命令队列)

`src/utils/messageQueueManager.ts` 提供 `getCommandsByMaxPriority`、`removeFromQueue`、`isSlashCommand`。在 `src/query.ts:1570-1578`,主循环每次迭代会 drain 队列中发给当前 agent 的通知:

```typescript
const queuedCommandsSnapshot = getCommandsByMaxPriority(
  sleepRan ? 'later' : 'next',
).filter(cmd => {
  if (isSlashCommand(cmd)) return false
  if (isMainThread) return cmd.agentId === undefined
  return cmd.mode === 'task-notification' && cmd.agentId === currentAgentId
})
```

**设计要点**:队列是进程级单例,coordinator 与所有 in-process 子 agent 共享。主线程只 drain `agentId===undefined` 的 prompt 与 `task-notification`,子 agent 只 drain 发给自己的 `task-notification`,两者互不干扰。斜命令(`/clear` 等)被排除在 mid-turn drain 之外,必须走 `processSlashCommand`。

### 1.5 多轮继续的 7 个 continue 站点

`queryLoop` 中有 7 处 `state = next; continue`,对应 7 种"为何继续":

| 触发点 | transition.reason | 位置 |
|--------|------------------|------|
| 工具执行完成 | `next_turn` | `query.ts:1725` |
| max_output_tokens 恢复 | `max_output_tokens_recovery` | `query.ts:1246` |
| max_output_tokens 升级 | `max_output_tokens_escalate` | `query.ts:1217` |
| reactive compact 恢复 | `reactive_compact_retry` | `query.ts:1162` |
| context collapse 泄洪 | `collapse_drain_retry` | `query.ts:1110` |
| stop hook 阻塞 | `stop_hook_blocking` | `query.ts:1302` |
| token budget 延续 | `token_budget_continuation` | `query.ts:1338` |

---

## 2. Context 的管理和实现

### 2.1 四级压缩管线 + 两个辅助机制

Claude Code 的 context 管理是业界最复杂的之一,由**四级压缩 + 两个辅助机制**组成:

```
queryLoop 每轮迭代(按顺序):
  1. applyToolResultBudget     —— 工具结果预算(按 tool_use_id 替换旧结果)
  2. snipCompactIfNeeded        —— HISTORY_SNIP 特性,裁剪旧消息
  3. microcompactMessages       —— 微压缩(基于时间 / 缓存编辑)
  4. applyCollapsesIfNeeded     —— ContextCollapse 读时投影
  5. autocompact                —— 自动压缩(超阈值时触发)
```

再加上两个**辅助记忆机制**:
- **extractMemories**:每轮结束后异步提取持久化记忆到 `~/.claude/projects/<path>/memory/`
- **MagicDocs**:后台 fork 子 agent 自动维护 `# MAGIC DOC:` 标记的 markdown 文件

### 2.2 autoCompact —— 主动压缩

**入口**:`src/services/compact/autoCompact.ts:160-200` 的 `shouldAutoCompact`。

**阈值计算**(`autoCompact.ts:72-91`):
```typescript
export function getAutoCompactThreshold(model: string): number {
  const effectiveContextWindow = getEffectiveContextWindowSize(model)
  const autocompactThreshold = effectiveContextWindow - AUTOCOMPACT_BUFFER_TOKENS
  // AUTOCOMPACT_BUFFER_TOKENS = 13_000
  ...
  return autocompactThreshold
}
```

**熔断器**(`autoCompact.ts:70`):`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`,连续 3 次压缩失败后停止重试,避免在上下文已不可恢复时浪费 API 调用(原文注释提到曾观测到单 session 50+ 连续失败、全球每天浪费 ~250K 次 API 调用)。

**递归守护**(`autoCompact.ts:169-183`):`session_memory` 与 `compact` 是 fork 的 agent,如果它们自身触发 autocompact 会死锁,所以直接返回 `false`。

### 2.3 microCompact —— 微压缩

**入口**:`src/services/compact/microCompact.ts:253-293` 的 `microcompactMessages`。

**三条路径**(按优先级):
1. **Time-based MC**(`microCompact.ts:267-270`):若距上次 assistant 消息超过阈值,服务端缓存已过期,直接清空旧 tool result 内容(跳过 cached MC,因为缓存编辑假设缓存是热的)。
2. **Cached MC**(`microCompact.ts:276-286`):使用 `cache_edits` API 在服务端删除旧 tool result,不修改本地消息内容(避免 prompt cache 失效)。仅主线程可用,避免 fork agent 注册了不属于自己的 tool_result。
3. **Legacy MC**:已移除(注释 `tengu_cache_plum_violet is always true`)。

**可压缩工具白名单**(`microCompact.ts:41-50`):
```typescript
const COMPACTABLE_TOOLS = new Set<string>([
  FILE_READ_TOOL_NAME, ...SHELL_TOOL_NAMES, GREP_TOOL_NAME,
  GLOB_TOOL_NAME, WEB_SEARCH_TOOL_NAME, WEB_FETCH_TOOL_NAME,
  FILE_EDIT_TOOL_NAME, FILE_WRITE_TOOL_NAME,
])
```

### 2.4 reactiveCompact —— 反应式压缩

**位置**:`src/services/compact/reactiveCompact.ts`(feature gate `REACTIVE_COMPACT`,ant-only)。

**触发时机**(`query.ts:1119-1166`):当 API 返回 `prompt-too-long` 错误时被调用。先尝试 context collapse 泄洪(廉价,保留细粒度上下文),若仍失败再走 reactive compact(全量摘要)。`hasAttemptedReactiveCompact` 标志防止无限循环。

### 2.5 sessionMemoryCompact —— 会话记忆压缩

**入口**:`src/services/compact/sessionMemoryCompact.ts`。

**配置**(`sessionMemoryCompact.ts:57-61`):
```typescript
export const DEFAULT_SM_COMPACT_CONFIG: SessionMemoryCompactConfig = {
  minTokens: 10_000,
  minTextBlockMessages: 5,
  maxTokens: 40_000,
}
```

**核心算法**(`sessionMemoryCompact.ts:324-349` 的 `calculateMessagesToKeepIndex`):从 `lastSummarizedMessageId` 开始往前扩展,直到满足 `minTokens` 与 `minTextBlockMessages`,但不超过 `maxTokens`。

**API 不变量保护**(`sessionMemoryCompact.ts:232-314` 的 `adjustIndexToPreserveAPIInvariants`):确保不会拆散 `tool_use` / `tool_result` 对,也不会拆散共享同一 `message.id` 的 thinking 块(流式传输会把同一 message.id 的 thinking、tool_use 分成多个消息)。这是流式场景下的关键防御性代码。

### 2.6 ContextCollapse —— 上下文折叠

**位置**:`src/services/contextCollapse/`(feature gate `CONTEXT_COLLAPSE`)。

**设计要点**(`query.ts:428-447`):与 reactive compact 不同,collapse 是**读时投影**—— 不修改 REPL 的消息数组,而是在每次 `queryLoop` 入口投影出一个"已折叠的视图"。摘要消息存在 collapse store 中,而非 REPL 数组。这让 collapse 可以跨 turn 持久化:`projectView()` 每次重放 commit log。

**泄洪恢复**(`query.ts:1086-1117`):当 API 返回 413 时,`contextCollapse.recoverFromOverflow` 提交所有 staged 的 collapse,若提交数 > 0 则以 `collapse_drain_retry` 继续循环。

### 2.7 extractMemories —— 会话记忆提取

**入口**:`src/services/extractMemories/extractMemories.ts:296-330` 的 `initExtractMemories`。

**设计要点**:
- 使用 **forked agent 模式**(`runForkedAgent`)—— 主对话的完美 fork,共享 prompt cache
- **闭包作用域状态**(`extractMemories.ts:297-325`):`lastMemoryMessageUuid` 游标、`inProgress` 重叠守卫、`turnsSinceLastExtraction` 计数器、`pendingContext` 尾随运行 stash
- **互斥设计**(`extractMemories.ts:348-359`):若主 agent 本轮写了记忆文件,fork 的提取就跳过,避免重复
- **工具权限**(`extractMemories.ts:171-222` 的 `createAutoMemCanUseTool`):只允许 Read/Grep/Glob(无限制)、只读 Bash、以及 auto-memory 目录内的 Edit/Write

### 2.8 MagicDocs —— 魔法文档自动维护

**入口**:`src/services/MagicDocs/magicDocs.ts:242-254` 的 `initMagicDocs`。

**触发**:注册为 post-sampling hook,仅在 `repl_main_thread` 且上一轮无工具调用时运行。

**检测**(`magicDocs.ts:52-81`):文件首行匹配 `# MAGIC DOC: [title]`,次行斜体为 instructions。

**更新流程**(`magicDocs.ts:114-212` 的 `updateMagicDoc`):
1. 克隆 FileStateCache(删除本文件条目,避免去重返回 `file_unchanged`)
2. 读取最新内容,重新检测 header
3. 构建更新提示
4. 创建只允许 Edit 本文件的 `canUseTool`
5. 通过 `runAgent` 以 `forkContextMessages: messages` fork 运行,共享 prompt cache

---

## 3. Yolo 识别 / 任务分类

### 3.1 关键发现:Claude Code 没有显式的 "Yolo Agent"

与 `LsmAgentEmergentWork` 的三档分类(simple/medium/hard)不同,**Claude Code 没有**一个独立的入口 Agent 来做"意图识别 → 任务分类 → 路由"。它的设计哲学是:

> **模型即路由** —— 让 Claude 自己通过 system prompt + 工具可用性来决定如何行动。

### 3.2 与 "Yolo 识别" 最接近的机制

#### (a) Auto-mode 安全分类器

**位置**:`src/utils/classifierApprovals.ts`、`src/utils/yoloClassifier.ts`(推测)。

**设计要点**(`Tool.ts:555-556`):
```typescript
toAutoClassifierInput(input: z.infer<Input>): unknown
```
每个工具提供一个紧凑表示供分类器判断安全性。MCP 工具在 `client.ts:1801-1803`:
```typescript
toAutoClassifierInput(input) {
  return mcpToolInputToAutoClassifierInput(input, tool.name)
}
```

#### (b) effort 复杂度档位

**位置**:`src/utils/effort.ts` 定义 `EffortValue`。

**设计要点**:Skill 与 Agent 都可以声明 `effort` frontmatter,模型用它来评估输出力度。这是最接近"任务复杂度分级"的机制,但由**声明方**指定,而非入口 Agent 推断。

#### (c) TaskCreateTool / TaskUpdateTool

**位置**:`src/tools/TaskCreateTool/`、`src/tools/TaskUpdateTool/`。

**设计要点**:模型自己创建任务列表来追踪进度,相当于**模型自管理的任务拆解**,而非外部编排器强加。

### 3.3 与 LsmAgentEmergentWork 的对比

| 维度 | LsmAgentEmergentWork | Claude Code |
|------|---------------------|-------------|
| 入口 Agent | Yolo Agent(独立角色) | 无,模型即路由 |
| 分类档位 | simple / medium / hard | 无显式档位 |
| 分类依据 | 三步意图识别(目的→目标→意图) | effort + 模型自行判断 |
| 失败回流 | Yolo 处理 | stop hooks + 恢复路径 |

---

## 4. 质检检查

### 4.1 Review Cadence —— 无固定节奏,由 stop hooks 驱动

Claude Code **没有**像 "每 N 轮 review 一次" 的固定 cadence。质检是**事件驱动**的:

**Stop Hooks**(`src/query/stopHooks.ts`):在每次模型响应完成后执行,可以阻塞 continuation。`src/query.ts:1267-1306`:
```typescript
const stopHookResult = yield* handleStopHooks(
  messagesForQuery, assistantMessages, systemPrompt,
  userContext, systemContext, toolUseContext, querySource, stopHookActive,
)
if (stopHookResult.preventContinuation) { return { reason: 'stop_hook_prevented' } }
if (stopHookResult.blockingErrors.length > 0) {
  // 注入错误消息,以 stop_hook_blocking 继续
}
```

**Post-Sampling Hooks**(`src/utils/hooks/postSamplingHooks.js`):在模型响应完成后触发,用于 extractMemories、MagicDocs、confidenceRating 等后台任务。

### 4.2 Commit Review —— 由 Skill 处理

**位置**:`src/skills/bundled/` 目录包含 `verify.ts`、`verifyContent.ts` 等内置技能。

**设计要点**:Commit review 不是硬编码在流程中,而是作为**可被模型调用的 Skill** 存在。模型自己决定何时 `/verify`。

### 4.3 Output 校验 —— SyntheticOutputTool

**位置**:`src/tools/SyntheticOutputTool/SyntheticOutputTool.js`。

**设计要点**:用于 SDK 场景的结构化输出校验,确保模型输出符合预期 schema。

### 4.4 Verification Agent

**位置**:`src/tools/AgentTool/built-in/verificationAgent.ts`(feature gate `tengu_hive_evidence`)。

**设计要点**:独立的内置 agent,专门用于验证。通过 GrowthBook 实验开关控制,尚未全量。

### 4.5 与 LsmAgentEmergentWork 的对比

| 维度 | LsmAgentEmergentWork | Claude Code |
|------|---------------------|-------------|
| 质检角色 | Quality-Check Agent(必经) | 无独立角色,由 hooks + skills 承担 |
| 质检时机 | 每个执行单元完成后 | stop hooks / post-sampling hooks |
| 固定 cadence | 有(每单元) | 无,事件驱动 |

---

## 5. 任务拆解

### 5.1 Plan Mode —— 由 EnterPlanModeTool / ExitPlanModeTool 界定

**位置**:`src/tools/EnterPlanModeTool/`、`src/tools/ExitPlanModeTool/`。

**设计要点**:Plan mode 不是独立 Agent,而是一种**权限模式**。进入 plan mode 后,模型只能读不能写(通过 `permissionMode: 'plan'` 控制)。Plan Agent(`src/tools/AgentTool/built-in/planAgent.ts`)是只读的探索者:

```typescript
// planAgent.ts:77-83
disallowedTools: [
  AGENT_TOOL_NAME, EXIT_PLAN_MODE_TOOL_NAME,
  FILE_EDIT_TOOL_NAME, FILE_WRITE_TOOL_NAME, NOTEBOOK_EDIT_TOOL_NAME,
],
```

### 5.2 子 Agent 编排 —— AgentTool + runAgent

**核心入口**:`src/tools/AgentTool/runAgent.ts:248-329` 的 `runAgent`。

**设计要点**:
- **Fork 上下文共享**(`runAgent.ts:370-373`):`forkContextMessages` 过滤掉不完整的工具调用后作为初始消息
- **Prompt cache 共享**(`runAgent.ts:510-518`):agent 的 system prompt 由父级计算并传入,确保字节级一致以命中 prompt cache
- **权限隔离**(`runAgent.ts:465-479`):`allowedTools` 提供的工具列表作为 session rules,**父级的审批不会泄漏**(cliArg 除外)
- **MCP 服务器隔离**(`runAgent.ts:95-218` 的 `initializeAgentMcpServers`):agent 可以声明自己的 MCP 服务器(frontmatter `mcpServers`),仅该 agent 生命周期内有效

### 5.3 Coordinator / Teammate / DreamTask

#### Coordinator Mode

**位置**:`src/coordinator/coordinatorMode.ts`。

**设计要点**(`coordinatorMode.ts:111-200`):Coordinator 是一个**系统提示词切换** —— 模型变成"编排者",通过 `AgentTool` 派发 worker、`SendMessage` 继续 worker、`TaskStop` 停止 worker。Worker 结果以 `<task-notification>` XML 形式作为 user 消息返回。

**Worker 工具集**(`coordinatorMode.ts:88-101`):在 `CLAUDE_CODE_SIMPLE` 模式下 worker 只有 Bash/Read/Edit,否则使用 `ASYNC_AGENT_ALLOWED_TOOLS`。

#### In-Process Teammates

**位置**:`src/tools/TeamCreateTool/`、`src/tools/TeamDeleteTool/`、`src/tools/SendMessageTool/`。

**设计要点**:Team 是 coordinator 模式下的**持久化 worker**。与一次性 agent 不同,teammate 可以通过 `SendMessage` 继续对话,复用已加载的上下文。

#### DreamTask / autoDream

**位置**:`src/services/autoDream/`。

**设计要点**:后台自动运行的"梦境"任务,用于主动发现与预处理。

### 5.4 与 LsmAgentEmergentWork 的对比

| 维度 | LsmAgentEmergentWork | Claude Code |
|------|---------------------|-------------|
| 规划层 | Plan Agent(独立角色,输出 Markdown 方案到 plans/) | Plan Agent(只读探索者) + Plan Mode(权限模式) |
| 执行层 | Main-Work → SubAgent-Work | AgentTool → runAgent |
| 多工人编排 | 无 | Coordinator + Team + SendMessage |
| 流程编排 | MultiAgentOrchestrator 总编排 | 模型自组织 + stop hooks |

---

## 6. 任务分类

### 6.1 Goal 模式 —— 无显式 Goal 概念

Claude Code **没有**像 "Goal / Task / Step" 这样的显式层级。最接近的是:

**TaskCreateTool / TaskUpdateTool / TaskGetTool / TaskListTool**(`src/tools/TaskCreateTool/` 等):模型自己创建任务列表来追踪进度。这是**模型自管理**的,而非外部编排器强加。

### 6.2 复杂度评估 —— effort 字段

**位置**:`src/utils/effort.ts`。

**设计要点**:
- Skill frontmatter 可声明 `effort`:low / medium / high / max 或整数
- Agent frontmatter 同样可声明 `effort`
- `runAgent.ts:481-486`:agent 的 effort 覆盖全局 effort

### 6.3 与 LsmAgentEmergentWork 的对比

| 维度 | LsmAgentEmergentWork | Claude Code |
|------|---------------------|-------------|
| 任务层级 | Goal → Task → Step(显式) | 无,模型自管理 Task 列表 |
| 复杂度输入 | Yolo 三档分类 | effort 声明 + 模型判断 |
| 编排方式 | 中央编排器(MultiAgentOrchestrator) | 分布式(模型 + hooks + skills) |

---

## 7. 工具调用

### 7.1 Tool.ts 设计 —— 巨型接口 + buildTool 默认值

**Tool trait 定义**(`src/Tool.ts:362-695`):包含 ~40 个方法/字段,是代码库中最庞大的接口之一。

**关键方法**:
- `call()` —— 执行工具(`Tool.ts:379-385`)
- `checkPermissions()` —— 权限检查(`Tool.ts:500-503`)
- `validateInput()` —— 输入校验(`Tool.ts:489-492`)
- `isConcurrencySafe()` —— 并发安全(`Tool.ts:402`)
- `isReadOnly()` —— 只读(`Tool.ts:404`)
- `isDestructive()` —— 破坏性(`Tool.ts:406`)
- `interruptBehavior()` —— 中断行为 cancel/block(`Tool.ts:416`)
- `backfillObservableInput()` —— 回填衍生字段(`Tool.ts:481`)
- `toAutoClassifierInput()` —— 安全分类器输入(`Tool.ts:556`)

**buildTool 工厂**(`src/Tool.ts:783-791`):
```typescript
export function buildTool<D extends AnyToolDef>(def: D): BuiltTool<D> {
  return {
    ...TOOL_DEFAULTS,
    userFacingName: () => def.name,
    ...def,
  } as BuiltTool<D>
}
```

**默认值**(`Tool.ts:757-769`):
```typescript
const TOOL_DEFAULTS = {
  isEnabled: () => true,
  isConcurrencySafe: () => false,  // 默认不安全(fail-closed)
  isReadOnly: () => false,         // 默认假设写
  isDestructive: () => false,
  checkPermissions: (input) => Promise.resolve({ behavior: 'allow', updatedInput: input }),
  toAutoClassifierInput: () => '',
  userFacingName: () => '',
}
```

**设计要点**:`isConcurrencySafe` 默认 `false`(保守),`checkPermissions` 默认 `allow`(交给通用权限系统)。这让新工具只需覆盖关心的方法。

### 7.2 ToolUseContext —— 工具执行的"上下文对象"

**定义**(`src/Tool.ts:158-300`):包含 ~50 个字段,是工具执行的"上帝对象":

```typescript
export type ToolUseContext = {
  options: {
    commands, debug, mainLoopModel, tools, verbose, thinkingConfig,
    mcpClients, mcpResources, isNonInteractiveSession, agentDefinitions,
    maxBudgetUsd, customSystemPrompt, appendSystemPrompt, querySource, refreshTools
  }
  abortController: AbortController
  readFileState: FileStateCache
  getAppState(): AppState
  setAppState(f: (prev: AppState) => AppState): void
  setAppStateForTasks?: ...  // 始终共享的 setAppState(给子 agent 用)
  handleElicitation?: ...    // URL elicitation 处理
  setToolJSX?: ...
  addNotification?: ...
  ...
  messages: FileReadingLimits, globLimits, toolDecisions, queryTracking,
            requestPrompt, contentReplacementState, renderedSystemPrompt, ...
}
```

**设计要点**:`setAppStateForTasks` 是给子 agent 用的 —— 异步 agent 的 `setAppState` 是 no-op(嵌套 async→async),所以 session 级写入(hooks, bash tasks)必须走这个始终连到 root store 的通道。

### 7.3 工具注册表 —— tools.ts

**getAllBaseTools**(`src/tools.ts:193-251`):返回所有内置工具,按特性门控条件包含。

**assembleToolPool**(`src/tools.ts:345-367`):
```typescript
export function assembleToolPool(permissionContext, mcpTools): Tools {
  const builtInTools = getTools(permissionContext)
  const allowedMcpTools = filterToolsByDenyRules(mcpTools, permissionContext)
  // 按名称排序(稳定 prompt cache),内置工具优先
  const byName = (a, b) => a.name.localeCompare(b.name)
  return uniqBy(
    [...builtInTools].sort(byName).concat(allowedMcpTools.sort(byName)),
    'name',
  )
}
```

**设计要点**:内置工具与 MCP 工具**按名称排序后合并**,`uniqBy` 保留插入顺序,名称冲突时内置工具优先。排序是为了 prompt cache 稳定性 —— 服务端在最后一个前缀匹配的内置工具后放全局 cache breakpoint,平坦排序避免 MCP 工具交错到内置工具中间导致下游 cache key 失效。

### 7.4 StreamingToolExecutor —— 流式工具执行器

**位置**:`src/services/tools/StreamingToolExecutor.ts`。

**核心设计**(`StreamingToolExecutor.ts:34-51`):
```typescript
export class StreamingToolExecutor {
  private tools: TrackedTool[] = []
  private hasErrored = false
  private erroredToolDescription = ''
  // 兄弟 abort controller —— Bash 错误时杀死兄弟子进程,但不结束 turn
  private siblingAbortController: AbortController
  private discarded = false
  private progressAvailableResolve?: () => void
}
```

**并发控制**(`StreamingToolExecutor.ts:129-135`):
```typescript
private canExecuteTool(isConcurrencySafe: boolean): boolean {
  const executingTools = this.tools.filter(t => t.status === 'executing')
  return (
    executingTools.length === 0 ||
    (isConcurrencySafe && executingTools.every(t => t.isConcurrencySafe))
  )
}
```

**Bash 错误级联**(`StreamingToolExecutor.ts:354-364`):只有 Bash 工具错误会取消兄弟工具(因为 Bash 命令常有隐式依赖链,如 mkdir 失败 → 后续命令无意义)。Read/WebFetch 等独立工具一个失败不会杀死其他。

**中断行为**(`StreamingToolExecutor.ts:210-241`):`getAbortReason` 区分 `sibling_error` / `user_interrupted` / `streaming_fallback`,`getToolInterruptBehavior` 查询工具的 `interruptBehavior()` 决定是 cancel 还是 block。

### 7.5 toolOrchestration.ts —— 非流式路径

**位置**:`src/services/tools/toolOrchestration.ts`。

**分区算法**(`toolOrchestration.ts:84-116`):
```typescript
function partitionToolCalls(toolUseMessages, toolUseContext): Batch[] {
  return toolUseMessages.reduce((acc, toolUse) => {
    const isConcurrencySafe = tool?.isConcurrencySafe(parsedInput)
    if (isConcurrencySafe && acc[acc.length - 1]?.isConcurrencySafe) {
      acc[acc.length - 1]!.blocks.push(toolUse)  // 合并到当前只读批次
    } else {
      acc.push({ isConcurrencySafe, blocks: [toolUse] })  // 新批次
    }
    return acc
  }, [])
}
```

**执行策略**(`toolOrchestration.ts:19-82`):只读批次并发执行(`runToolsConcurrently`,最多 10 个),非只读批次串行执行(`runToolsSerially`)。contextModifier 在只读批次中排队,批次结束后统一应用。

---

## 8. MCP 设计与实现

### 8.1 client.ts(3348 行) —— MCP 核心

**连接工厂**(`client.ts:595-1641` 的 `connectToServer`):
- 使用 lodash `memoize` 缓存连接,key = `name-${JSON.stringify(config)}`
- 支持 7 种传输:`stdio` / `sse` / `sse-ide` / `ws-ide` / `http` / `ws` / `claudeai-proxy` / `sdk`
- 每种传输的初始化逻辑独立 ~100 行

**HTTP 传输**(`client.ts:784-865`):
```typescript
const authProvider = new ClaudeAuthProvider(name, serverRef)
const transportOptions: StreamableHTTPClientTransportOptions = {
  authProvider,
  fetch: wrapFetchWithTimeout(
    wrapFetchWithStepUpDetection(createFetchWithInit(), authProvider),
  ),
  requestInit: { headers: { 'User-Agent': getMCPUserAgent(), ...combinedHeaders } },
}
transport = new StreamableHTTPClientTransport(new URL(serverRef.url), transportOptions)
```

**连接超时**(`client.ts:1048-1077`):`Promise.race([connectPromise, timeoutPromise])`,默认 30s。

**错误恢复**(`client.ts:1266-1402`):
- **Session 过期**(`client.ts:1313-1329`):HTTP 404 + JSON-RPC -32001 → 关闭传输,下次调用重连
- **终端错误级联**(`client.ts:1350-1365`):连续 3 次 ECONNRESET/ETIMEDOUT/EPIPE 等 → 关闭并重连
- **SSE 重连耗尽**(`client.ts:1342-1348`):SDK 的 StreamableHTTP 在 maxRetries(默认 2)次后触发,但不会调 onclose,这里手动关闭

**进程清理**(`client.ts:1429-1562`):stdio 服务器关闭时发送 SIGINT → 等 100ms → SIGTERM → 等 400ms → SIGKILL,总计 ~500ms 升级序列。

**工具获取**(`client.ts:1743-1998` 的 `fetchToolsForClient`):
- LRU 缓存(大小 20),key = server name
- 将 MCP 工具转换为内部 `Tool` 格式,设置 `mcpInfo`、`searchHint`、`alwaysLoad`、`isConcurrencySafe`、`isReadOnly`、`isDestructive`、`isOpenWorld` 等注解
- 描述截断到 `MAX_MCP_DESCRIPTION_LENGTH = 2048`

**批量连接**(`client.ts:2226-2403` 的 `getMcpToolsCommandsAndResources`):
- 本地服务器(stdio/sdk)并发数默认 3,远程服务器默认 20
- 使用 `pMap` 而非固定批次 —— 一个慢服务器只占用一个 slot,不阻塞整个批次
- 跳过 15 分钟内返回过 401 的服务器(避免重复探测)

### 8.2 auth.ts(2465 行) —— OAuth 实现

**位置**:`src/services/mcp/auth.ts`。

**核心类**:`ClaudeAuthProvider` 实现 MCP SDK 的 `OAuthClientProvider` 接口。

**OAuth 流**(`auth.ts:1-150`):
- 发现 AS metadata(`discoverAuthorizationServerMetadata`)
- DCR(Dynamic Client Registration)
- 本地 HTTP 回调服务器(`auth.ts:48` 的 `buildRedirectUri`、`findAvailablePort`)
- 浏览器打开授权页(`openBrowser`)
- Token 刷新与缓存

**非标准错误处理**(`auth.ts:147-150`):
```typescript
const NONSTANDARD_INVALID_GRANT_ALIASES = new Set([
  'invalid_refresh_token', 'expired_refresh_token', 'token_expired',
])
```
Slack 等 OAuth 服务器用非标准错误码,RFC 6749 规定应是 `invalid_grant`,这里做归一化。

**Slack 200-with-error 问题**(`auth.ts:128-146`):Slack 对所有响应返回 HTTP 200,错误在 JSON body 中。SDK 只在 `!response.ok` 时调用 `parseErrorResponse`,所以 200 + `{"error":"invalid_grant"}` 会被喂给 `OAuthTokensSchema.parse()` 抛出 ZodError。这里包装 fetch,检测 2xx body 是否匹配 `OAuthErrorResponseSchema`,若是则改写为 400 Response。

### 8.3 InProcessTransport —— 进程内传输

**位置**:`src/services/mcp/InProcessTransport.ts`。

**设计要点**(`InProcessTransport.ts:11-63`):
```typescript
class InProcessTransport implements Transport {
  private peer: InProcessTransport | undefined
  private closed = false

  async send(message: JSONRPCMessage): Promise<void> {
    if (this.closed) throw new Error('Transport is closed')
    // 异步投递到对端,避免同步 request/response 的栈深度问题
    queueMicrotask(() => { this.peer?.onmessage?.(message) })
  }
}
```

**使用场景**(`client.ts:909-943`):
- **Chrome MCP**:避免启动 ~325MB 子进程,直接 in-process 运行
- **Computer Use MCP**:同上,包内的 CallTool handler 是桩,真实分发走 wrapper.tsx

### 8.4 SdkControlTransport —— SDK 控制通道

**位置**:`src/services/mcp/SdkControlTransport.ts`。

**架构**(`SdkControlTransport.ts:1-37`):
```
CLI → SDK: SdkControlClientTransport
  1. MCP Client 调用工具 → JSONRPC 请求发到 ClientTransport
  2. 包装进 control request(server_name + request_id)
  3. 通过 stdout 发到 SDK 进程
  4. SDK 的 StructuredIO 接收 control response 并路由回 transport
  5. 解包返回给 MCP Client

SDK → CLI: SdkControlServerTransport
  1. Query 收到 control 消息,调 transport.onmessage
  2. MCP server 处理,调 transport.send() 发响应
  3. transport 调 sendMcpMessage 回调
  4. Query 的 pending promise resolve
```

**设计要点**:支持多个 SDK MCP 服务器同时运行,`server_name` 用于路由,message ID 全程保留用于关联。

### 8.5 与 LsmAgentEmergentWork 的对比

| 维度 | LsmAgentEmergentWork | Claude Code |
|------|---------------------|-------------|
| MCP 客户端 | 无(仅 Anthropic/OpenAI 双协议) | 3348 行,7 种传输 |
| OAuth | 无 | 完整实现(2465 行) |
| 进程内传输 | 无 | InProcessTransport |
| SDK 桥接 | 无 | SdkControlTransport |
| 连接管理 | 无 | memoize + LRU + 批量 + 熔断 |

---

## 9. SKILL 设计

### 9.1 loadSkillsDir.ts —— 技能加载核心

**位置**:`src/skills/loadSkillsDir.ts`。

**技能来源**(`loadSkillsDir.ts:67-73`):
```typescript
export type LoadedFrom =
  | 'commands_DEPRECATED' | 'skills' | 'plugin'
  | 'managed' | 'bundled' | 'mcp'
```

**路径解析**(`loadSkillsDir.ts:78-94` 的 `getSkillsPath`):
- `policySettings` → `<managed>/.claude/skills`
- `userSettings` → `~/.claude/skills`
- `projectSettings` → `.claude/skills`
- `plugin` → `plugin`

**Frontmatter 解析**(`loadSkillsDir.ts:185-265` 的 `parseSkillFrontmatterFields`):
```typescript
export function parseSkillFrontmatterFields(frontmatter, markdownContent, resolvedName, ...) {
  return {
    displayName, description, hasUserSpecifiedDescription,
    allowedTools, argumentHint, argumentNames, whenToUse,
    version, model, disableModelInvocation, userInvocable,
    hooks, executionContext, agent, effort, shell,
  }
}
```

**createSkillCommand**(`loadSkillsDir.ts:270-399`):
- 注入 `Base directory for this skill: <dir>` 前缀
- 参数替换 `$ARGUMENTS` 与命名参数
- `${CLAUDE_SKILL_DIR}` → 技能自己的目录(用于 bash 注入 `!`...``)
- `${CLAUDE_SESSION_ID}` → 当前 session ID
- **安全限制**:MCP 技能(远程、不可信)从不执行内联 shell 命令

### 9.2 bundled/ —— 内置技能

**位置**:`src/skills/bundled/` 目录,包含 16 个内置技能:

| 技能 | 用途 |
|------|------|
| `batch.ts` | 批处理 |
| `claudeApi.ts` / `claudeApiContent.ts` | Claude API 交互 |
| `claudeInChrome.ts` | Chrome 集成 |
| `debug.ts` | 调试 |
| `keybindings.ts` | 快捷键 |
| `loop.ts` | 循环任务 |
| `remember.ts` | 记忆 |
| `scheduleRemoteAgents.ts` | 调度远程 agent |
| `simplify.ts` | 简化代码(对应 `/simplify`) |
| `skillify.ts` | 创建技能 |
| `stuck.ts` | 卡住检测 |
| `updateConfig.ts` | 更新配置 |
| `verify.ts` / `verifyContent.ts` | 验证(对应 `/verify`) |

**注册机制**(`src/skills/bundledSkills.ts:53-100` 的 `registerBundledSkill`):
```typescript
export function registerBundledSkill(definition: BundledSkillDefinition): void {
  const { files } = definition
  let skillRoot, getPromptForCommand = definition.getPromptForCommand
  if (files && Object.keys(files).length > 0) {
    skillRoot = getBundledSkillExtractDir(definition.name)
    // 懒提取:首次调用时把参考文件写入磁盘
    getPromptForCommand = async (args, ctx) => {
      extractionPromise ??= extractBundledSkillFiles(definition.name, files)
      const extractedDir = await extractionPromise
      const blocks = await inner(args, ctx)
      if (extractedDir === null) return blocks
      return prependBaseDir(blocks, extractedDir)
    }
  }
  bundledSkills.push(command)
}
```

**设计要点**:内置技能可以携带参考文件(`files` 字段),首次调用时懒提取到 `~/.claude/bundled-skills/<name>/`,提示词前缀 `Base directory for this skill: <dir>` 让模型可以 Read/Grep 这些文件。

### 9.3 mcpSkillBuilders —— MCP 技能桥接

**位置**:`src/skills/mcpSkillBuilders.ts`。

**设计要点**(`mcpSkillBuilders.ts:1-44`):
```typescript
export type MCPSkillBuilders = {
  createSkillCommand: typeof createSkillCommand
  parseSkillFrontmatterFields: typeof parseSkillFrontmatterFields
}

let builders: MCPSkillBuilders | null = null

export function registerMCPSkillBuilders(b: MCPSkillBuilders): void {
  builders = b
}

export function getMCPSkillBuilders(): MCPSkillBuilders {
  if (!builders) throw new Error('MCP skill builders not registered...')
  return builders
}
```

**为什么需要这个间接层**(`mcpSkillBuilders.ts:14-23` 注释):
- 非字面量动态导入(`await import(variable)`)在 Bun 打包的二进制中失败 —— specifier 解析到 `/$bunfs/root/...` 而非源树
- 字面量动态导入在 bunfs 中工作,但 dependency-cruiser 会追踪它,而 loadSkillsDir 几乎触及所有模块,单条新边会在 diff 检查中扇出成许多新环
- 所以用**注册模式**:loadSkillsDir.ts 在模块初始化时注册两个函数,MCP 技能发现时通过 getter 获取

### 9.4 与 LsmAgentEmergentWork 的对比

| 维度 | LsmAgentEmergentWork | Claude Code |
|------|---------------------|-------------|
| 技能系统 | 无 | 完整(加载 + 注册 + frontmatter + MCP 桥接) |
| 内置技能 | 无 | 16 个(bundled/) |
| 技能来源 | 无 | 用户/项目/策略/插件/MCP/内置 |
| 技能格式 | 无 | Markdown + YAML frontmatter |
| 参数注入 | 无 | `$ARGUMENTS` / `${CLAUDE_SKILL_DIR}` / `${CLAUDE_SESSION_ID}` |

---

## 10. 跨维度设计模式总结

### 10.1 Feature Gate 无处不在

Claude Code 使用 `feature('FLAG_NAME')`(来自 `bun:bundle`)做编译时死码消除。关键标志:
- `REACTIVE_COMPACT` —— 反应式压缩
- `CONTEXT_COLLAPSE` —— 上下文折叠
- `CACHED_MICROCOMPACT` —— 缓存编辑微压缩
- `HISTORY_SNIP` —— 历史裁剪
- `COORDINATOR_MODE` —— 协调者模式
- `MCP_SKILLS` —— MCP 技能发现
- `CHICAGO_MCP` —— Computer Use MCP

**设计要点**:`feature()` 只在 if/三元条件下工作(bun:bundle 的 tree-shaking 约束),所以代码中常见嵌套 if 而非组合条件。

### 10.2 Forked Agent 模式

多处使用 `runForkedAgent` / `createSubagentContext` 创建主对话的**完美 fork**:
- extractMemories —— 后台记忆提取
- MagicDocs —— 后台文档维护
- autoDream —— 后台梦境任务
- Task Summary —— 后台进度摘要

**优势**:共享 prompt cache(字节级一致的系统提示词 + 上下文),节省 token。

### 10.3 闭包作用域状态

多个模块使用**闭包 + 模块级变量**模式管理状态,而非 class:
- `extractMemories.ts:297-325` —— `initExtractMemories` 创建闭包
- `confidenceRating.ts` —— 同样模式

**优势**:模块级变量被闭包捕获,外部无法直接访问;测试可以调用 init 函数获取 fresh closure。

### 10.4 巨型类型 + 默认值模式

`Tool.ts` 的 ~40 方法接口 + `buildTool` 默认值,`ToolUseContext` 的 ~50 字段,`State` 的 ~10 字段 —— Claude Code 偏好**一个巨大的上下文对象**贯穿全局,而非层层传递小参数。

### 10.5 与 LsmAgentEmergentWork 的架构差异总结

| 设计哲学 | LsmAgentEmergentWork | Claude Code |
|----------|---------------------|-------------|
| 编排模式 | 中央编排器(MultiAgentOrchestrator) | 分布式(模型 + hooks + skills) |
| Agent 角色 | 6 个固定角色 | 动态(内置 + 用户 + 插件 + MCP) |
| 任务分类 | 显式三档(simple/medium/hard) | 隐式(模型自管理 + effort) |
| 质检 | 独立 Quality-Check Agent | hooks + skills + verification agent |
| Context 管理 | 无 | 四级压缩 + 两个辅助机制 |
| MCP | 无 | 完整实现(3348 行) |
| 技能系统 | 无 | 完整(加载 + 注册 + MCP 桥接) |

**核心洞察**:Claude Code 的设计哲学是**"模型即编排器"** —— 不强制规定流程,而是提供丰富的工具、hooks、skills,让模型自己决定如何完成任务。这与 LsmAgentEmergentWork 的**"中央编排器 + 固定角色"**形成鲜明对比。Claude Code 的复杂性不在于角色多,而在于**机制多**(压缩管线、权限系统、MCP、hooks、skills、coordinator),这些机制共同构成一个高度灵活但极其复杂的系统。
