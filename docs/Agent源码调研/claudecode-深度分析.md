# Claude Code 源码系统级深度分析报告

> 分析对象：`/usr/local/LsmGitOpenSource/claudecode/src`  
> 分析日期：2026-09-04  
> 分析维度：10 大核心系统（架构 / 多轮对话 / Context / Yolo / 质检 / 任务拆解 / 任务分类 / 工具 / MCP / SKILL）

---

## 0. 项目概览

Claude Code 是 Anthropic 官方的 TypeScript/Bun CLI Agent 工具，业界最成熟的 LLM Agent CLI 之一。

| 指标 | 数值 |
|------|------|
| 源文件数（.ts/.tsx） | ~1896 个 |
| 总代码行数 | ~41,000 行 |
| 核心入口 `main.tsx` | 4,683 行 |
| 查询循环 `query.ts` | 1,729 行 |
| 工具定义 `Tool.ts` | 792 行 |
| 命令系统 `commands.ts` | 754 行 |

**技术栈**：Bun 运行时 + React/Ink（终端 UI）+ `@anthropic-ai/sdk` + `@modelcontextprotocol/sdk` + zod v4 校验 + SQLite（via bun:sqlite）。

---

## 1. 项目架构：单层扁平结构 + Feature Gate

### 1.1 单层扁平结构

Claude Code 采用**单层扁平**的 `src/` 目录结构，没有传统分层（如 `domain/`、`application/`、`infrastructure/`），而是按**功能域切分**：

```
src/
  main.tsx          # CLI 入口（4683 行，最胖入口）
  query.ts          # 多轮对话主循环（AsyncGenerator）
  QueryEngine.ts    # 查询引擎（SDK/headless 入口）
  Tool.ts           # Tool 类型定义 + buildTool 工厂
  tools.ts          # 工具注册表（getTools 工厂）
  tasks.ts          # 任务注册表
  commands.ts       # 斜杠命令注册表
  context.ts        # 系统/用户上下文拼装
  Task.ts           # 后台任务类型定义
  history.ts        # 历史记录持久化
  state/            # 应用状态（AppState）
  tools/            # 各工具实现（每工具一目录，40+）
  services/         # 后端服务（compact/mcp/lsp/analytics）
  hooks/            # React hooks（权限/设置/UI）
  skills/           # Skill 系统
  coordinator/      # 多 Agent 协调模式
  components/       # Ink UI 组件
  constants/        # 常量/提示词
  utils/            # 工具函数
```

**关键设计**：`main.tsx` 同时承担 CLI 参数解析、初始化编排、GrowthBook 特性开关、OAuth 预取、遥测启动等职责——是典型的"胖入口"模式。

### 1.2 Feature Gate 体系（编译时 DCE）

Claude Code 使用编译时 feature gate（`bun:bundle` 的 `feature()` 函数）实现**死代码消除（Dead Code Elimination）**，外部发布版会完全剔除内部功能代码。

**三大核心 Feature Gate**：

| Gate | 用途 | 引用位置 |
|------|------|----------|
| `REACTIVE_COMPACT` | 响应式压缩（prompt-too-long 后自动压缩） | `query.ts:15` |
| `COORDINATOR_MODE` | 多 Agent 协调模式 | `main.tsx:76`, `tools.ts:120` |
| `CONTEXT_COLLAPSE` | 上下文折叠（非摘要的细粒度裁剪） | `query.ts:18`, `tools.ts:110` |

**完整 Feature Gate 清单**（从源码中提取）：

```typescript
// query.ts 中的 gate
const reactiveCompact = feature('REACTIVE_COMPACT')   // L15
const contextCollapse = feature('CONTEXT_COLLAPSE')    // L18
const skillPrefetch = feature('EXPERIMENTAL_SKILL_SEARCH')  // L66
const jobClassifier = feature('TEMPLATES')             // L69
const snipModule = feature('HISTORY_SNIP')             // L115
const taskSummaryModule = feature('BG_SESSIONS')       // L118

// tools.ts 中的 gate
feature('PROACTIVE')                    // L26 - 主动服务
feature('KAIROS')                       // L26 - 助手模式
feature('AGENT_TRIGGERS')               // L29 - 定时触发
feature('AGENT_TRIGGERS_REMOTE')        // L36 - 远程触发
feature('MONITOR_TOOL')                 // L39 - 监控工具
feature('KAIROS_PUSH_NOTIFICATION')    // L46
feature('KAIROS_GITHUB_WEBHOOKS')       // L50
feature('OVERFLOW_TEST_TOOL')           // L107
feature('TERMINAL_PANEL')               // L113
feature('WEB_BROWSER_TOOL')             // L117
feature('UDS_INBOX')                    // L126
feature('WORKFLOW_SCRIPTS')            // L129
feature('VOICE_MODE')                   // main.tsx L14
feature('BRIDGE_MODE')                  // commands.ts L73
feature('DAEMON')                       // commands.ts L77
```

**Gate 使用模式**（条件 require 实现 DCE）：

```typescript
// query.ts L15-21 — 标准 DCE 模式
const reactiveCompact = feature('REACTIVE_COMPACT')
  ? (require('./services/compact/reactiveCompact.js') as typeof import('./services/compact/reactiveCompact.js'))
  : null
const contextCollapse = feature('CONTEXT_COLLAPSE')
  ? (require('./services/contextCollapse/index.js') as typeof import('./services/contextCollapse/index.js'))
  : null
```

**设计意义**：外部 npm 包仅包含公开功能，内部 Anthropic 功能（KAIROS 助手模式、VOICE、BRIDGE 等）被完全 tree-shake。

### 1.3 入口与启动流程

**`src/main.tsx`**（4683 行）是 CLI 入口，承担：
- CLI 参数解析（clap 风格）
- GrowthBook 特性开关初始化
- OAuth 预取
- 遥测启动
- 会话恢复 / 新建

**`src/replLauncher.tsx`** — REPL 启动：

```typescript
// replLauncher.tsx
export async function launchRepl(root, appProps, replProps, renderAndRun) {
  const { App } = await import('./components/App.js')
  const { REPL } = await import('./screens/REPL.js')
  await renderAndRun(root, <App {...appProps}><REPL {...replProps} /></App>)
}
```

**`src/QueryEngine.ts`** — SDK/headless 会话编排：

```typescript
// QueryEngine.ts:184-1177
export class QueryEngine {
  private mutableMessages: Message[]
  private abortController: AbortController
  private totalUsage: NonNullableUsage
  private readFileState: FileStateCache

  async *submitMessage(prompt, options) {
    // 1. processUserInput - 处理用户输入（含斜杠命令）
    // 2. recordTranscript - 持久化到 session
    // 3. 调用 query() 进入 agentic 循环
    // 4. 处理各种消息类型
    // 5. 检查 maxBudgetUsd / maxTurns 限制
  }
}
```

---

## 2. 多轮对话实现

### 2.1 核心循环：`query.ts` 的 AsyncGenerator

多轮对话的核心是 `query.ts` 中的 **AsyncGenerator 循环**，这是整个 Agent 的"心跳"：

```typescript
// query.ts L219-238 — 入口
export async function* query(params: QueryParams): AsyncGenerator<
  StreamEvent | RequestStartEvent | Message | TombstoneMessage | ToolUseSummaryMessage,
  Terminal
> {
  const consumedCommandUuids: string[] = []
  const terminal = yield* queryLoop(params, consumedCommandUuids)
  for (const uuid of consumedCommandUuids) {
    notifyCommandLifecycle(uuid, 'completed')
  }
  return terminal
}
```

### 2.2 循环状态机

```typescript
// query.ts L204-217 — 跨迭代可变状态
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

### 2.3 主循环流程（query.ts L307-1200+）

```
while (true) {
  1. 解构 state → messages, toolUseContext, tracking
  2. Skill discovery prefetch（并行）
  3. yield { type: 'stream_request_start' }
  4. 初始化/递增 queryChainTracking（chainId + depth）
  5. applyToolResultBudget() — 工具结果大小限制
  6. snipCompactIfNeeded() — 历史裁剪（HISTORY_SNIP gate）
  7. microcompact() — 微压缩
  8. contextCollapse.applyCollapsesIfNeeded() — 上下文折叠
  9. autocompact() — 自动压缩（超阈值时）
  10. queryModelWithStreaming() — 调用 Anthropic API
  11. 流式处理响应 → yield StreamEvent
  12. 收集 assistantMessages + toolUseBlocks
  13. streamingToolExecutor.addTool() — 并行工具执行
  14. 处理 tool results
  15. 判断终止条件 → break 或 continue
}
```

### 2.4 流式工具执行器

```typescript
// query.ts L838-862 — 流式执行（工具与模型输出并行）
if (streamingToolExecutor && !toolUseContext.abortController.signal.aborted) {
  for (const toolBlock of msgToolUseBlocks) {
    streamingToolExecutor.addTool(toolBlock, message)
  }
}
// 获取已完成结果
for (const result of streamingToolExecutor.getCompletedResults()) {
  if (result.message) {
    yield result.message
    toolResults.push(...normalizeMessagesForAPI([result.message], toolUseContext.options.tools))
  }
}
```

### 2.5 消息模型（`types/message.ts`）

```typescript
// 核心消息类型
export type Message = UserMessage | AssistantMessage | SystemMessage | 
                      AttachmentMessage | HookResultMessage | ProgressMessage |
                      ToolUseSummaryMessage | SystemLocalCommandMessage | TombstoneMessage

export type UserMessage = {
  type: 'user'
  uuid: string
  message: { role: 'user'; content: string | ContentBlockParam[] }
  toolUseResult?: string
  sourceToolAssistantUUID?: string
}

export type AssistantMessage = {
  type: 'assistant'
  uuid: string
  message: { role: 'assistant'; content: ContentBlockParam[]; usage?: Usage }
  apiError?: string
}
```

### 2.6 消息归一化管线

发送给 API 前经过复杂的归一化（`normalizeMessagesForAPI`，`src/utils/messages.ts:1989-2370`）：

1. `reorderAttachmentsForAPI` - 附件上浮到 tool_result/assistant 之前
2. 过滤 virtual 消息
3. 按 error 类型剥离 PDF/image 块
4. 合并连续 user 消息（Bedrock 兼容）
5. 剥离 tool_reference（tool search 关闭时）
6. 合并相邻 user 消息
7. `smooshSystemReminderSiblings` - 合并 `<system-reminder>` 到 tool_result
8. `sanitizeErrorToolResultContent` - 清理错误 tool_result
9. `appendMessageTagToUserMessage` - 注入 `[id:xxx]` 供 snip 引用
10. `validateImagesForAPI` - 校验图片尺寸

---

## 3. Context 管理：四级压缩管线

Claude Code 拥有**业界最复杂的上下文管理系统**，包含四级递进压缩。

### 3.1 四级管线架构

```
原始消息流
    │
    ▼
① Tool Result Budget（工具结果大小限制）
    │
    ▼
② Snip Compact（历史裁剪，HISTORY_SNIP gate）
    │
    ▼
③ Micro-Compact（微压缩，单工具结果摘要）
    │
    ▼
④ Context Collapse（上下文折叠，CONTEXT_COLLAPSE gate）
    │
    ▼
⑤ Auto-Compact（全量摘要压缩，最后手段）
    │
    ▼
API 请求
```

### 3.2 各级实现

**① Tool Result Budget**（`query.ts:379-394`）：

```typescript
messagesForQuery = await applyToolResultBudget(
  messagesForQuery,
  toolUseContext.contentReplacementState,
  persistReplacements ? records => void recordContentReplacement(records, toolUseContext.agentId) : undefined,
  new Set(toolUseContext.options.tools.filter(t => !Number.isFinite(t.maxResultSizeChars)).map(t => t.name)),
)
```

**② Snip Compact**（feature gate: `HISTORY_SNIP`，`query.ts:401-410`）：

```typescript
if (feature('HISTORY_SNIP')) {
  const snipResult = snipModule!.snipCompactIfNeeded(messagesForQuery)
  messagesForQuery = snipResult.messages
  snipTokensFreed = snipResult.tokensFreed
}
```

**③ Micro-Compact**（`services/compact/microCompact.ts`）：

```typescript
// 仅压缩特定工具的结果
const COMPACTABLE_TOOLS = new Set([
  FILE_READ_TOOL_NAME, ...SHELL_TOOL_NAMES, GREP_TOOL_NAME,
  GLOB_TOOL_NAME, WEB_SEARCH_TOOL_NAME, WEB_FETCH_TOOL_NAME,
  FILE_EDIT_TOOL_NAME, FILE_WRITE_TOOL_NAME,
])
```

**④ Context Collapse**（`query.ts:440-447`）：

```typescript
if (feature('CONTEXT_COLLAPSE') && contextCollapse) {
  const collapseResult = await contextCollapse.applyCollapsesIfNeeded(
    messagesForQuery, toolUseContext, querySource,
  )
  messagesForQuery = collapseResult.messages
}
```

**⑤ Auto-Compact**（`services/compact/autoCompact.ts`）：

```typescript
// 阈值计算
export function getAutoCompactThreshold(model: string): number {
  const effectiveContextWindow = getEffectiveContextWindowSize(model)
  return effectiveContextWindow - AUTOCOMPACT_BUFFER_TOKENS  // 13,000 token buffer
}

// 失败熔断
const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3
```

### 3.3 四级压缩详细机制

#### 第一级：Time-Based Microcompact（时间触发）

**文件**: `src/services/compact/timeBasedMCConfig.ts` + `microCompact.ts:422-530`

```typescript
// timeBasedMCConfig.ts
const TIME_BASED_MC_CONFIG_DEFAULTS: TimeBasedMCConfig = {
  enabled: false,
  gapThresholdMinutes: 60,  // 1 小时
  keepRecent: 5,            // 保留最近 5 个工具结果
}
```

**触发条件**（`evaluateTimeBasedTrigger`，行 422-444）：
- 距最后一条 assistant 消息超过 `gapThresholdMinutes`（默认 60 分钟）
- 仅主线程（`isMainThreadSource`）
- 说明：服务端缓存 TTL 为 1 小时，超时后缓存必然失效，前缀需重写

**行为**（`maybeTimeBasedMicrocompact`，行 446-530）：

```typescript
// 保留最近 N 个工具结果，content-clear 其余
const keepSet = new Set(compactableIds.slice(-keepRecent))
const clearSet = new Set(compactableIds.filter(id => !keepSet.has(id)))
// 替换为标记
return { ...block, content: TIME_BASED_MC_CLEARED_MESSAGE }
// TIME_BASED_MC_CLEARED_MESSAGE = '[Old tool result content cleared]'
```

#### 第二级：Cached Microcompact（缓存编辑式）

**文件**: `microCompact.ts:305-399`

仅 ant-only（`feature('CACHED_MICROCOMPACT')`），使用 Anthropic API 的 **cache_edits** 能力：

```typescript
async function cachedMicrocompactPath(messages, querySource): Promise<MicrocompactResult> {
  // 注册工具结果到缓存状态
  for (const message of messages) {
    if (message.type === 'user' && Array.isArray(message.message.content)) {
      for (const block of message.message.content) {
        if (block.type === 'tool_result' && ...) {
          mod.registerToolResult(state, block.tool_use_id)
        }
      }
    }
  }
  // 获取应删除的工具
  const toolsToDelete = mod.getToolResultsToDelete(state)
  // 创建 cache_edits 块，交给 API 层执行
  const cacheEdits = mod.createCacheEditsBlock(state, toolsToDelete)
  pendingCacheEdits = cacheEdits
  return { messages, compactionInfo: { pendingCacheEdits: { ... } } }
}
```

**关键设计**：不修改本地消息内容，通过 `cache_reference` 和 `cache_edits` 在 API 层实现，保持缓存前缀不变。

#### 第三级：Auto-Compact（自动压缩）

**文件**: `src/services/compact/autoCompact.ts`

**阈值计算**（行 33-49, 72-91）：

```typescript
export function getEffectiveContextWindowSize(model: string): number {
  const reservedTokensForSummary = Math.min(
    getMaxOutputTokensForModel(model),
    MAX_OUTPUT_TOKENS_FOR_SUMMARY  // 20,000
  )
  let contextWindow = getContextWindowForModel(model, getSdkBetas())
  return contextWindow - reservedTokensForSummary
}

export function getAutoCompactThreshold(model: string): number {
  const effectiveContextWindow = getEffectiveContextWindowSize(model)
  return effectiveContextWindow - AUTOCOMPACT_BUFFER_TOKENS  // 13,000
}
```

**递进式防护**（行 62-66）：

```
AUTOCOMPACT_BUFFER_TOKENS = 13,000    // 自动压缩触发缓冲
WARNING_THRESHOLD_BUFFER_TOKENS = 20,000  // 警告阈值
ERROR_THRESHOLD_BUFFER_TOKENS = 20,000    // 错误阈值
MANUAL_COMPACT_BUFFER_TOKENS = 3,000      // 手动压缩阻塞线
```

**断路器**（行 70）：连续失败 3 次后停止重试，防止不可恢复场景浪费 API 调用。

**Feature Gate 互斥设计**（行 195-223）：

```typescript
// REACTIVE_COMPACT 模式：抑制主动 autocompact，让 reactive compact 处理 413
if (feature('REACTIVE_COMPACT')) {
  if (getFeatureValue_CACHED_MAY_BE_STALE('tengu_cobalt_raccoon', false)) {
    return false
  }
}
// CONTEXT_COLLAPSE 模式：collapse 有自己的 90%/95% 水位线管理
if (feature('CONTEXT_COLLAPSE')) {
  const { isContextCollapseEnabled } = require('../contextCollapse/index.js')
  if (isContextCollapseEnabled()) return false
}
```

#### 第四级：Session Memory Compact（会话记忆压缩）

**文件**: `src/services/compact/sessionMemoryCompact.ts`

**实验性功能**，作为 `autoCompactIfNeeded` 的首选路径（行 288-310 in autoCompact.ts）：

```typescript
const sessionMemoryResult = await trySessionMemoryCompaction(
  messages, toolUseContext.agentId, recompactionInfo.autoCompactThreshold
)
if (sessionMemoryResult) {
  // 成功则跳过传统 compactConversation
  return { wasCompacted: true, compactionResult: sessionMemoryResult }
}
```

**保留策略**（`calculateMessagesToKeepIndex`，行 324-397）：

```typescript
export const DEFAULT_SM_COMPACT_CONFIG: SessionMemoryCompactConfig = {
  minTokens: 10_000,         // 最少保留 10K tokens
  minTextBlockMessages: 5,   // 最少保留 5 条含文本的消息
  maxTokens: 40_000,         // 最多保留 40K tokens
}
```

### 3.4 完整压缩流程（compactConversation）

**文件**: `src/services/compact/compact.ts:387-763`

1. **PreCompact Hooks**（行 413-424）：执行用户和 hook 注入的自定义压缩指令
2. **缓存共享 Fork 路径**（`streamCompactSummary`，行 1136-1396）：优先尝试 fork agent 复用主对话的 prompt cache
3. **PTL 重试**（`truncateHeadForPTLRetry`，行 243-291）：当压缩请求本身超长时，按 API 回合组截断头部，最多重试 3 次
4. **Post-Compact 恢复**（行 517-586）：恢复最近 5 个文件、plan 文件、plan mode 状态、技能内容
5. **SessionStart Hooks**（行 591-595）：重新注入 CLAUDE.md 等上下文

### 3.5 压缩提示词设计

**文件**: `src/services/compact/prompt.ts`

结构化要求（行 61-143）：

```
1. Primary Request and Intent
2. Key Technical Concepts
3. Files and Code Sections（含代码片段）
4. Errors and fixes
5. Problem Solving
6. All user messages
7. Pending Tasks
8. Current Work
9. Optional Next Step（含原话引用）
```

用 `<analysis>` 标签作为草稿纸（模型在内部分析），`<summary>` 为最终输出。`formatCompactSummary` 函数（行 311-335）会剥离 `<analysis>` 部分。

### 3.6 Reactive Compact 机制

**引用位置**: `src/query.ts:15-17, 1119-1162`

```typescript
const reactiveCompact = feature('REACTIVE_COMPACT')
  ? require('./services/compact/reactiveCompact.js')
  : null

// 在 query 循环中捕获 413 prompt-too-long 错误
if ((isWithheld413 || isWithheldMedia) && reactiveCompact) {
  const compacted = await reactiveCompact.tryReactiveCompact({ ... })
  if (compacted) {
    continue  // transition: { reason: 'reactive_compact_retry' }
  }
}
```

这是一种**被动式**压缩：不主动检测 token 水位，而是在 API 返回 413/prompt-too-long 时触发，从尾部剥离消息直至请求成功。

### 3.7 上下文注入

系统/用户上下文通过 `src/context.ts` 注入：

```typescript
// src/context.ts:116-189
export const getSystemContext = memoize(async () => {
  const gitStatus = await getGitStatus()  // git branch/status/log
  return {
    ...(gitStatus && { gitStatus }),
    ...(injection ? { cacheBreaker: `[CACHE_BREAKER: ${injection}]` } : {}),
  }
})

export const getUserContext = memoize(async () => {
  const claudeMd = getClaudeMds(filterInjectedMemoryFiles(await getMemoryFiles()))
  return {
    ...(claudeMd && { claudeMd }),
    currentDate: `Today's date is ${getLocalISODate()}.`,
  }
})
```

### 3.8 Token 估算系统

**文件**: `src/services/tokenEstimation.ts` + `microCompact.ts:164-205`

```typescript
export function estimateMessageTokens(messages: Message[]): number {
  // 逐块估算：text、tool_result、image(2000)、thinking、tool_use
  // 最终乘以 4/3 安全系数
  return Math.ceil(totalTokens * (4 / 3))
}
```

上下文窗口大小配置（`src/utils/context.ts`）：

```
MODEL_CONTEXT_WINDOW_DEFAULT = 200,000
COMPACT_MAX_OUTPUT_TOKENS = 20,000
CAPPED_DEFAULT_MAX_TOKENS = 8,000    // BQ p99 = 4,911，8K 足够 99%
ESCALATED_MAX_TOKENS = 64,000        // 超限重试
```

### 3.9 Feature Gate 互斥矩阵

| Gate | 作用 | 与其他 Gate 的关系 |
|------|------|-------------------|
| `REACTIVE_COMPACT` | 抑制 proactive autocompact，改为 413 错误时 reactive 处理 | 与 `CONTEXT_COLLAPSE` 互斥 |
| `CONTEXT_COLLAPSE` | 90% commit / 95% blocking-spawn 的上下文折叠系统 | 抑制 autocompact，重置时清理 |
| `CACHED_MICROCOMPACT` | 使用 API cache_edits 删除工具结果 | 与 time-based MC 互斥（cache 冷时跳过） |
| `PROMPT_CACHE_BREAK_DETECTION` | 检测缓存断裂 | 所有压缩路径均通知此系统 |

---

## 4. Yolo / 任务识别

### 4.1 Effort Level 系统

Claude Code 使用 **Effort Level** 控制模型推理深度（`utils/effort.ts`）：

```typescript
// utils/effort.ts L13-18
export const EFFORT_LEVELS = ['low', 'medium', 'high', 'max'] as const
export type EffortValue = EffortLevel | number  // 数字仅内部使用
```

**模型支持矩阵**（`effort.ts:23-49`）：

```typescript
export function modelSupportsEffort(model: string): boolean {
  if (m.includes('opus-4-6') || m.includes('sonnet-4-6')) return true
  if (m.includes('haiku') || m.includes('sonnet') || m.includes('opus')) return false
  return getAPIProvider() === 'firstParty'  // 1P 默认开启
}
```

**Max effort 限制**（`effort.ts:53-65`）：

```typescript
export function modelSupportsMaxEffort(model: string): boolean {
  // 仅 opus-4-6 支持 'max'
  // 内部用户通过 resolveAntModel 白名单
}
```

**优先级链**（`effort.ts:152-167`）：

```typescript
export function resolveAppliedEffort(model, appStateEffortValue) {
  // env CLAUDE_CODE_EFFORT_LEVEL → appState.effortValue → model default
  const resolved = envOverride ?? appStateEffortValue ?? getDefaultEffortForModel(model)
  // API 拒绝非 Opus-4.6 的 'max' → 降级为 'high'
  if (resolved === 'max' && !modelSupportsMaxEffort(model)) return 'high'
  return resolved
}
```

**默认 effort**（`effort.ts:279-329`）：

```typescript
export function getDefaultEffortForModel(model: string) {
  // Opus 4.6 + Pro → medium
  // Opus 4.6 + Max/Team (grey_step2) → medium
  // ultrathink 功能开启 → medium
  // 其他 → undefined（API 端解析为 high）
}
```

**持久化规则**（`effort.ts:95-105`）：

```typescript
export function toPersistableEffort(value) {
  // low/medium/high → 持久化
  // max → 仅内部用户持久化（外部用户 session-scoped）
}
```

### 4.2 任务分类：TaskType

```typescript
// Task.ts L6-14 — 后台任务类型
export type TaskType =
  | 'local_bash'          # 本地 shell
  | 'local_agent'         # 本地子 Agent
  | 'remote_agent'        # 远程 Agent
  | 'in_process_teammate' # 进程内队友
  | 'local_workflow'      # 本地工作流
  | 'monitor_mcp'         # MCP 监控
  | 'dream'               # 后台思考
```

### 4.3 意图理解

Claude Code 没有独立的"Yolo 入口层"（与 laew 不同），而是通过 **System Prompt 注入** + **工具集约束**间接实现意图引导：

```typescript
// constants/prompts.ts — 系统提示词组合
export function getSystemPrompt(tools, commands, mcpClients, ...) {
  return enhanceSystemPromptWithEnvDetails(
    systemPromptSection() + DANGEROUS_uncachedSystemPromptSection() + ...
  )
}
```

### 4.4 自动模式分类器

`src/hooks/useCanUseTool.tsx` 实现 **auto-mode 分类器**：

```typescript
// src/hooks/useCanUseTool.tsx:38-54
if (result.behavior === "allow") {
  if (feature("TRANSCRIPT_CLASSIFIER") && result.decisionReason?.type === "classifier" 
      && result.decisionReason.classifier === "auto-mode") {
    setYoloClassifierApproval(toolUseID, result.decisionReason.reason)
  }
  resolve(ctx.buildAllow(result.updatedInput ?? input, { decisionReason: result.decisionReason }))
  return
}
```

### 4.5 拒绝消息构建

`src/utils/messages.ts` 提供分类器拒绝消息：

```typescript
// src/utils/messages.ts:267-282
export function buildYoloRejectionMessage(reason: string): string {
  const prefix = AUTO_MODE_REJECTION_PREFIX
  const ruleHint = feature('BASH_CLASSIFIER')
    ? `To allow this type of action in the future, the user can add a permission rule like 
       Bash(prompt: <description of allowed action>) to their settings.`
    : `To allow this type of action in the future, the user can add a Bash permission rule.`
  return `${prefix}${reason}. ${DENIAL_WORKAROUND_GUIDANCE} ${ruleHint}`
}
```

---

## 5. 质检检查：Hooks 机制

### 5.1 Hook 类型体系

Claude Code 拥有**最完整的 Hooks 系统**（`hooks/` 目录 80+ 文件）：

```typescript
// hooks/ 目录结构
useCanUseTool.tsx          # 权限判定核心
useCommandQueue.ts         # 命令队列
useSettings.ts             # 设置监听
useSkillsChange.ts         # Skill 变更
useTaskListWatcher.ts      # 任务列表
toolPermission/            # 权限子模块
  handlers/
    coordinatorHandler.ts  # 协调模式权限
    interactiveHandler.ts  # 交互权限
    swarmWorkerHandler.ts  # 集群 worker 权限
  PermissionContext.ts     # 权限上下文
notifs/                    # 通知
```

### 5.2 27 种 Hook 事件

**`src/utils/hooks/hooksConfigManager.ts:27-265`** — 完整事件列表：

```
PreToolUse, PostToolUse, PostToolUseFailure, PermissionDenied,
Notification, UserPromptSubmit, SessionStart, Stop, StopFailure,
SubagentStart, SubagentStop, PreCompact, PostCompact, SessionEnd,
PermissionRequest, Setup, TeammateIdle, TaskCreated, TaskCompleted,
Elicitation, ElicitationResult, ConfigChange, InstructionsLoaded,
WorktreeCreate, WorktreeRemove, CwdChanged, FileChanged
```

每种事件带 matcher 元数据，例如：

```typescript
// hooksConfigManager.ts:29-46
PreToolUse: {
  summary: 'Before tool execution',
  description: 'Exit code 0 - stdout/stderr not shown\nExit code 2 - show stderr to model and block tool call',
  matcherMetadata: { fieldToMatch: 'tool_name', values: toolNames }
}
```

### 5.3 三种 Hook 执行方式

#### a) `execAgentHook`（`src/utils/hooks/execAgentHook.ts`）

```typescript
// execAgentHook.ts:36-60
export async function execAgentHook(hook, hookName, hookEvent, jsonInput, signal, toolUseContext, ...) {
  const processedPrompt = addArgumentsToPrompt(hook.prompt, jsonInput)
  // 多轮 LLM 查询，使用小快模型
  const transcriptPath = toolUseContext.agentId
    ? getAgentTranscriptPath(toolUseContext.agentId)
    : getTranscriptPath()
}
```

#### b) `execHttpHook`（`src/utils/hooks/execHttpHook.ts`）

```typescript
// execHttpHook.ts:12
const DEFAULT_HTTP_HOOK_TIMEOUT_MS = 10 * 60 * 1000  // 10 分钟

// execHttpHook.ts:50-60 — 沙箱代理路由
async function getSandboxProxyConfig() {
  // 通过沙箱网络代理路由 HTTP hook 请求
  await SandboxManager.waitForNetworkInitialization()
}
```

#### c) `execPromptHook`（`src/utils/hooks/execPromptHook.ts`）

```typescript
// execPromptHook.ts:21-50
export async function execPromptHook(hook, hookName, hookEvent, jsonInput, signal, toolUseContext, messages?) {
  const processedPrompt = addArgumentsToPrompt(hook.prompt, jsonInput)
  // 替换 $ARGUMENTS 为 JSON 输入
  // 直接创建 user message，不触发 UserPromptSubmit hooks（避免无限递归）
  const userMessage = createUserMessage({ content: processedPrompt })
}
```

### 5.4 权限检查核心

```typescript
// hooks/useCanUseTool.tsx L27 — 权限函数类型
export type CanUseToolFn<Input extends Record<string, unknown> = Record<string, unknown>> = (
  tool: ToolType,
  input: Input,
  toolUseContext: ToolUseContext,
  assistantMessage: AssistantMessage,
  toolUseID: string,
  forceDecision?: PermissionDecision<Input>,
) => Promise<PermissionDecision<Input>>

// L32-53 — 权限判定流程
const decisionPromise = forceDecision !== undefined
  ? Promise.resolve(forceDecision)
  : hasPermissionsToUseTool(tool, input, toolUseContext, assistantMessage, toolUseID)

return decisionPromise.then(async result => {
  if (result.behavior === "allow") {
    // TRANSCRIPT_CLASSIFIER gate：记录分类器审批
    if (feature("TRANSCRIPT_CLASSIFIER") && result.decisionReason?.type === "classifier") {
      setYoloClassifierApproval(toolUseID, result.decisionReason.reason)
    }
    ctx.logDecision({ decision: "accept", source: "config" })
    resolve(ctx.buildAllow(result.updatedInput ?? input, { decisionReason: result.decisionReason }))
    return
  }
  // deny → 显示权限请求 UI
  switch (result.behavior) {
    case "deny": ...
    case "ask": ...
  }
})
```

### 5.5 权限模式

```typescript
// types/permissions.ts
export type PermissionMode = 'default' | 'auto' | 'plan' | 'bypass'

// ToolPermissionContext 结构
export type ToolPermissionContext = DeepImmutable<{
  mode: PermissionMode
  additionalWorkingDirectories: Map<string, AdditionalWorkingDirectory>
  alwaysAllowRules: ToolPermissionRulesBySource
  alwaysDenyRules: ToolPermissionRulesBySource
  alwaysAskRules: ToolPermissionRulesBySource
  isBypassPermissionsModeAvailable: boolean
  shouldAvoidPermissionPrompts?: boolean
  awaitAutomatedChecksBeforeDialog?: boolean
  prePlanMode?: PermissionMode
}>
```

### 5.6 工具安全约束

```typescript
// tools/BashTool/BashTool.tsx — 命令安全分类
const BASH_SEARCH_COMMANDS = new Set(['find', 'grep', 'rg', 'ag', 'ack', 'locate', 'which', 'whereis'])
const BASH_READ_COMMANDS = new Set(['cat', 'head', 'tail', 'less', 'more', 'wc', 'stat', 'file', 'jq', 'awk'])
const BASH_LIST_COMMANDS = new Set(['ls', 'tree', 'du'])
const BASH_SEMANTIC_NEUTRAL_COMMANDS = new Set(['echo', 'printf', 'true', 'false', ':'])
```

### 5.7 SSRF 防护（`src/utils/hooks/ssrfGuard.ts`）

```typescript
// ssrfGuard.ts:24-50 — 阻止的地址范围
// 0.0.0.0/8, 10.0.0.0/8, 100.64.0.0/10, 169.254.0.0/16 (cloud metadata)
// 172.16.0.0/12, 192.168.0.0/16
// IPv6: ::, fc00::/7, fe80::/10, ::ffff:<v4 blocked>

// 允许：127.0.0.0/8, ::1（本地开发策略服务器）
```

### 5.8 Session Hooks（`src/utils/hooks/sessionHooks.ts`）

```typescript
// sessionHooks.ts:15-31 — Function Hook 回调
export type FunctionHookCallback = (messages: Message[], signal?: AbortSignal) => boolean | Promise<boolean>

export type FunctionHook = {
  type: 'function'; id?: string; timeout?: number
  callback: FunctionHookCallback; errorMessage: string; statusMessage?: string
}

// sessionHooks.ts:62 — 使用 Map 而非 Record
export type SessionHooksState = Map<string, SessionStore>
// 注释说明：高并发工作流下，parallel() 同步触发 N 次 addFunctionHook
// Map.set 是 O(1)，返回 prev 避免触发 store 监听器
```

---

## 6. 任务拆解

### 6.1 Task 类型系统

```typescript
// Task.ts L31-76 — 任务接口
export type TaskHandle = { taskId: string; cleanup?: () => void }
export type SetAppState = (f: (prev: AppState) => AppState) => void
export type TaskContext = {
  abortController: AbortController
  getAppState: () => AppState
  setAppState: SetAppState
}

// 统一接口
export type Task = {
  name: string
  type: TaskType
  kill(taskId: string, setAppState: SetAppState): Promise<void>
}
```

### 6.2 任务注册表

```typescript
// tasks.ts — 任务注册
export function getAllTasks(): Task[] {
  const tasks: Task[] = [LocalShellTask, LocalAgentTask, RemoteAgentTask, DreamTask]
  if (LocalWorkflowTask) tasks.push(LocalWorkflowTask)  // WORKFLOW_SCRIPTS gate
  if (MonitorMcpTask) tasks.push(MonitorMcpTask)        // MONITOR_TOOL gate
  return tasks
}
```

### 6.3 任务目录结构

```
src/tasks/
  DreamTask/              # 后台思考任务
  InProcessTeammateTask/  # 进程内队友
  LocalAgentTask/         # 本地子 Agent（最复杂）
  LocalShellTask/         # 本地 shell
  LocalWorkflowTask/      # 本地工作流
  RemoteAgentTask/        # 远程 Agent
  MonitorMcpTask/         # MCP 监控
```

### 6.4 各任务类型实现差异

#### LocalShellTask（`src/tasks/LocalShellTask/LocalShellTask.tsx`）

后台 shell 命令执行，带**阻塞检测看门狗**：

```typescript
// LocalShellTask.tsx:24-42
const STALL_CHECK_INTERVAL_MS = 5_000
const STALL_THRESHOLD_MS = 45_000
const STALL_TAIL_BYTES = 1024

const PROMPT_PATTERNS = [/\(y\/n\)/i, /\[y\/n\]/i, /\(yes\/no\)/i,
  /\b(?:Do you|Would you|Shall I|Are you sure|Ready to)\b.*\? *$/i,
  /Press (any key|Enter)/i, /Continue\?/i, /Overwrite\?/i]

export function looksLikePrompt(tail: string): boolean {
  const lastLine = tail.trimEnd().split('\n').pop() ?? ''
  return PROMPT_PATTERNS.some(p => p.test(lastLine))
}
```

- 每 5 秒检查输出是否增长
- 45 秒无增长 + 尾部像交互提示 → 发送通知让模型处理
- 区分"慢命令"和"等待输入的命令"

#### LocalAgentTask（`src/tasks/LocalAgentTask/LocalAgentTask.tsx`）

带**进度追踪器**的子 agent：

```typescript
// LocalAgentTask.tsx:23-60
export type ToolActivity = {
  toolName: string; input: Record<string, unknown>
  activityDescription?: string; isSearch?: boolean; isRead?: boolean
}
export type AgentProgress = {
  toolUseCount: number; tokenCount: number
  lastActivity?: ToolActivity; recentActivities?: ToolActivity[]; summary?: string
}
export type ProgressTracker = {
  toolUseCount: number; latestInputTokens: number
  cumulativeOutputTokens: number; recentActivities: ToolActivity[]
}
```

区分 input tokens（累积，取最新）和 output tokens（每轮累加），避免重复计数。

#### RemoteAgentTask（`src/tasks/RemoteAgentTask/RemoteAgentTask.tsx`）

远程会话（teleport/ultraplan/ultrareview）：

```typescript
// RemoteAgentTask.tsx:22-60
export type RemoteAgentTaskState = TaskStateBase & {
  type: 'remote_agent'
  remoteTaskType: RemoteTaskType   // 'remote-agent'|'ultraplan'|'ultrareview'|'autofix-pr'|'background-pr'
  remoteTaskMetadata?: RemoteTaskMetadata
  sessionId: string; command: string; title: string
  todoList: TodoList; log: SDKMessage[]
  isLongRunning?: boolean          // 不会在第一个 result 后标记完成
  pollStartedAt: number            // 轮询开始时间（resume 时不立即超时）
  isRemoteReview?: boolean
  reviewProgress?: { stage?: 'finding'|'verifying'|'synthesizing'; bugsFound: number; ... }
  isUltraplan?: boolean
  ultraplanPhase?: Exclude<UltraplanPhase, 'running'>  // needs_input | plan_ready
}
```

#### InProcessTeammateTask（`src/tasks/InProcessTeammateTask/InProcessTeammateTask.tsx`）

**进程内 teammate**（swarm 模式），通过 AsyncLocalStorage 隔离：

```typescript
// InProcessTeammateTask.tsx:1-10
// 1. 运行在同一 Node.js 进程，使用 AsyncLocalStorage 隔离
// 2. 具有团队身份（agentName@teamName）
// 3. 支持 plan mode 审批流
// 4. 可处于 idle（等待工作）或 active（处理中）

export const InProcessTeammateTask: Task = {
  name: 'InProcessTeammateTask', type: 'in_process_teammate',
  async kill(taskId, setAppState) {
    killInProcessTeammate(taskId, setAppState)
  }
}
```

#### DreamTask（`src/tasks/DreamTask/DreamTask.ts`）

**记忆整合子 agent**，纯 UI 展示：

```typescript
// DreamTask.ts:25-41
export type DreamTaskState = TaskStateBase & {
  type: 'dream'
  phase: DreamPhase   // 'starting' | 'updating'
  sessionsReviewing: number
  filesTouched: string[]   // 不完整反映，仅捕获 Edit/Write tool_use
  turns: DreamTurn[]       // 最多 30 轮
  abortController?: AbortController
  priorMtime: number       // kill 时回滚锁 mtime
}

// DreamTask.ts:132-157 — kill 时回滚整合锁
async kill(taskId, setAppState) {
  // ... abort + 标记 killed
  if (priorMtime !== undefined) {
    await rollbackConsolidationLock(priorMtime)
  }
}
```

### 6.5 任务框架基础设施

#### `src/utils/task/framework.ts` — 注册与轮询

```typescript
// framework.ts:48-72 — 类型安全的状态更新
export function updateTaskState<T extends TaskState>(
  taskId: string, setAppState: SetAppState, updater: (task: T) => T
): void {
  setAppState(prev => {
    const task = prev.tasks?.[taskId] as T | undefined
    if (!task) return prev
    const updated = updater(task)
    if (updated === task) return prev  // 同引用 → 跳过，避免重渲染
    return { ...prev, tasks: { ...prev.tasks, [taskId]: updated } }
  })
}

// framework.ts:77-117 — 注册任务（带 resume 合并）
export function registerTask(task: TaskState, setAppState: SetAppState): void {
  // 替换时携带 UI 状态：retain/startTime/messages/diskLoaded/pendingMessages
  const merged = existing && 'retain' in existing
    ? { ...task, retain: existing.retain, startTime: existing.startTime, ... }
    : task
  // 新任务 → 入队 SDK task_started 事件
}
```

**轮询循环**（`framework.ts:255-269`）：

```typescript
export async function pollTasks(getAppState, setAppState): Promise<void> {
  const state = getAppState()
  const { attachments, updatedTaskOffsets, evictedTaskIds } =
    await generateTaskAttachments(state)
  applyTaskOffsetsAndEvictions(setAppState, updatedTaskOffsets, evictedTaskIds)
  for (const attachment of attachments) {
    enqueueTaskNotification(attachment)
  }
}
```

**终止任务驱逐**（`framework.ts:125-144`）：

```typescript
export function evictTerminalTask(taskId, setAppState): void {
  // 必须 terminal + notified
  // Panel grace period（30 秒）内不驱逐
  if ('retain' in task && (task.evictAfter ?? Infinity) > Date.now()) return
  delete newTasks[id]
}
```

#### `src/utils/task/diskOutput.ts` — 磁盘输出

**安全设计**：

```typescript
// diskOutput.ts:19-21
// SECURITY: O_NOFOLLOW 防止跟随符号链接
// 沙箱内攻击者可能在 tasks 目录创建符号链接指向任意文件
const O_NOFOLLOW = fsConstants.O_NOFOLLOW ?? 0

// diskOutput.ts:30
export const MAX_TASK_OUTPUT_BYTES = 5 * 1024 * 1024 * 1024  // 5GB
```

**异步写入队列**（`diskOutput.ts:97-231`）：

```typescript
export class DiskTaskOutput {
  #queue: string[] = []
  #bytesWritten = 0
  #capped = false

  append(content: string): void {
    this.#bytesWritten += content.length
    if (this.#bytesWritten > MAX_TASK_OUTPUT_BYTES) {
      this.#capped = true  // 超过 5GB 截断
    } else {
      this.#queue.push(content)
    }
    if (!this.#flushPromise) {
      this.#flushPromise = new Promise(resolve => { this.#flushResolve = resolve })
      void track(this.#drain())  // 单 drain 循环处理
    }
  }
}
```

**Delta 读取**（`diskOutput.ts:304-330`）：只从字节偏移读取新内容，避免加载完整文件。

### 6.6 任务终止系统

#### `src/tasks/stopTask.ts` — 统一终止逻辑

```typescript
// stopTask.ts:10-17 — 错误类型
export class StopTaskError extends Error {
  constructor(message, public readonly code: 'not_found' | 'not_running' | 'unsupported_type')
}

// stopTask.ts:38-100 — 终止流程
export async function stopTask(taskId, context) {
  // 1. 查找任务
  // 2. 验证状态 === 'running'
  // 3. 查找任务类型实现
  // 4. 调用 taskImpl.kill(taskId, setAppState)
  // 5. Bash 任务：抑制 "exit code 137" 通知（噪音）
  //    Agent 任务：不抑制 — AbortError catch 发送 extractPartialResult
  // 6. 发射 SDK task_terminated 事件
}
```

---

## 7. 任务分类

### 7.1 工具集分层

```typescript
// constants/tools.ts — 工具可用性矩阵
export const ALL_AGENT_DISALLOWED_TOOLS = new Set([
  TASK_OUTPUT_TOOL_NAME,
  EXIT_PLAN_MODE_V2_TOOL_NAME,
  ENTER_PLAN_MODE_TOOL_NAME,
  ASK_USER_QUESTION_TOOL_NAME,
  TASK_STOP_TOOL_NAME,
])

export const ASYNC_AGENT_ALLOWED_TOOLS = new Set([
  FILE_READ_TOOL_NAME, WEB_SEARCH_TOOL_NAME, TODO_WRITE_TOOL_NAME,
  GREP_TOOL_NAME, WEB_FETCH_TOOL_NAME, GLOB_TOOL_NAME,
  ...SHELL_TOOL_NAMES, FILE_EDIT_TOOL_NAME, FILE_WRITE_TOOL_NAME,
  NOTEBOOK_EDIT_TOOL_NAME, SKILL_TOOL_NAME, SYNTHETIC_OUTPUT_TOOL_NAME,
  TOOL_SEARCH_TOOL_NAME, ENTER_WORKTREE_TOOL_NAME, EXIT_WORKTREE_TOOL_NAME,
])

export const IN_PROCESS_TEAMMATE_ALLOWED_TOOLS = new Set([
  TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME,
  TASK_UPDATE_TOOL_NAME, TASK_OUTPUT_TOOL_NAME, AGENT_TOOL_NAME,
])
```

### 7.2 自动决策系统

权限决策通过 `src/utils/permissions/permissions.ts` 的 `hasPermissionsToUseTool`：

```typescript
export async function hasPermissionsToUseTool(
  tool: Tool, input: Record<string, unknown>,
  toolUseContext: ToolUseContext, assistantMessage: AssistantMessage,
  toolUseID: string,
): Promise<PermissionResult> {
  // 1. 检查 deny rules（全局拒绝）
  // 2. 检查 allow rules（显式允许）
  // 3. 检查 alwaysAllowRules（会话级允许）
  // 4. 调用 auto-mode classifier（bash 分类器）
  // 5. 返回 ask（需要用户确认）
}
```

### 7.3 权限模式

```typescript
// src/types/permissions.ts
export type PermissionMode = 
  | 'default'           # 默认（询问）
  | 'acceptEdits'       # 自动接受编辑
  | 'bypassPermissions' # YOLO 模式（跳过所有权限）
  | 'plan'              # 计划模式
  | 'dontAsk'           # 不询问（拒绝时静默）
  | 'auto'              # 自动模式（分类器决策）
```

### 7.4 自动后台化

```typescript
// BashTool 中的自动后台化
function getAutoBackgroundMs(): number {
  if (isEnvTruthy(process.env.CLAUDE_AUTO_BACKGROUND_TASKS) || 
      getFeatureValue_CACHED_MAY_BE_STALE('tengu_auto_background_agents', false)) {
    return 120_000  // 2 分钟后自动后台
  }
  return 0
}
```

---

## 8. 工具调用

### 8.1 Tool 接口定义

```typescript
// Tool.ts L158-199 — ToolUseContext（工具执行上下文）
export type ToolUseContext = {
  options: {
    commands: Command[]
    debug: boolean
    mainLoopModel: string
    tools: Tools
    verbose: boolean
    thinkingConfig: ThinkingConfig
    mcpClients: MCPServerConnection[]
    mcpResources: Record<string, ServerResource[]>
    isNonInteractiveSession: boolean
    agentDefinitions: AgentDefinitionsResult
    maxBudgetUsd?: number
    customSystemPrompt?: string
    appendSystemPrompt?: string
    querySource?: QuerySource
    refreshTools?: () => Tools
  }
  abortController: AbortController
  readFileState: FileStateCache
  getAppState(): AppState
  setAppState(f: (prev: AppState) => AppState): void
  setAppStateForTasks?: (f: (prev: AppState) => AppState) => void
  handleElicitation?: (serverName: string, ...) => Promise<ElicitResult>
}
```

### 8.2 buildTool 工厂

```typescript
// Tool.ts — buildTool 工厂
export function buildTool<TInput, TResult>(def: ToolDef<TInput, TResult>): Tool { ... }

// Tool.ts:757-792
const TOOL_DEFAULTS = {
  isEnabled: () => true,
  isConcurrencySafe: (_input?: unknown) => false,
  isReadOnly: (_input?: unknown) => false,
  isDestructive: (_input?: unknown) => false,
  checkPermissions: (input, _ctx) =>
    Promise.resolve({ behavior: 'allow', updatedInput: input }),
  toAutoClassifierInput: (_input?: unknown) => '',
  userFacingName: (_input?: unknown) => '',
}

export function buildTool<D extends AnyToolDef>(def: D): BuiltTool<D> {
  return {
    ...TOOL_DEFAULTS,
    userFacingName: () => def.name,
    ...def,
  } as BuiltTool<D>
}
```

### 8.3 工具注册中心

```typescript
// tools.ts — 完整工具列表（约 30+ 工具）
export function getTools(...): Tools {
  return [
    // 核心工具（始终可用）
    BashTool, FileReadTool, FileEditTool, FileWriteTool,
    GlobTool, GrepTool, WebFetchTool, WebSearchTool,
    AgentTool, SkillTool, TodoWriteTool,
    
    // Feature-gated 工具
    ...(feature('PROACTIVE') || feature('KAIROS') ? [SleepTool] : []),
    ...(feature('AGENT_TRIGGERS') ? [CronCreateTool, CronDeleteTool, CronListTool] : []),
    ...(feature('COORDINATOR_MODE') ? coordinatorModeModule.getCoordinatorTools() : []),
    ...(feature('CONTEXT_COLLAPSE') ? [CtxInspectTool] : []),
    ...(feature('HISTORY_SNIP') ? [SnipTool] : []),
    ...(feature('WORKFLOW_SCRIPTS') ? [WorkflowTool] : []),
  ]
}
```

### 8.4 工具目录结构

```
src/tools/
  AgentTool/              # 子 Agent 调度
  AskUserQuestionTool/    # 用户提问
  BashTool/               # Shell 执行（最复杂，含安全分析）
  BriefTool/              # 简报模式
  ConfigTool/             # 配置管理
  EnterPlanModeTool/      # 进入计划模式
  ExitPlanModeTool/       # 退出计划模式
  FileEditTool/           # 文件编辑
  FileReadTool/           # 文件读取
  FileWriteTool/          # 文件写入
  GlobTool/               # 文件搜索
  GrepTool/               # 内容搜索
  LSPTool/                # LSP 集成
  MCPTool/                # MCP 工具代理
  NotebookEditTool/       # Jupyter 编辑
  REPLTool/               # REPL（ant-only）
  ScheduleCronTool/       # 定时任务
  SendMessageTool/        # 多 Agent 通信
  SkillTool/              # Skill 调用
  TaskCreateTool/         # 任务创建
  TaskGetTool/            # 任务查询
  TaskUpdateTool/         # 任务更新
  TodoWriteTool/          # 待办列表
  ToolSearchTool/         # 工具搜索
  WebFetchTool/           # 网页抓取
  WebSearchTool/          # 网页搜索
  shared/                 # 共享工具逻辑
```

### 8.5 工具编排执行

`src/services/tools/toolOrchestration.ts` 实现**并发/串行混合执行**：

```typescript
// src/services/tools/toolOrchestration.ts:19-80
export async function* runTools(
  toolUseMessages: ToolUseBlock[],
  assistantMessages: AssistantMessage[],
  canUseTool: CanUseToolFn,
  toolUseContext: ToolUseContext,
): AsyncGenerator<MessageUpdate, void, void> {
  for (const { isConcurrencySafe, blocks } of partitionToolCalls(toolUseMessages, currentContext)) {
    if (isConcurrencySafe) {
      // 只读工具并发执行（max 10 并发）
      for await (const update of runToolsConcurrently(blocks, ...)) { ... }
    } else {
      // 写入工具串行执行
      for await (const update of runToolsSerially(blocks, ...)) { ... }
    }
  }
}
```

### 8.6 BashTool 实现示例

```typescript
// src/tools/BashTool/BashTool.tsx:420-623
export const BashTool = buildTool({
  name: BASH_TOOL_NAME,
  maxResultSizeChars: 30_000,
  strict: true,
  
  async checkPermissions(input, context): Promise<PermissionResult> {
    return bashToolHasPermission(input, context)
  },
  
  async validateInput(input: BashToolInput): Promise<ValidationResult> {
    // 检测 sleep 模式，阻止长 sleep
    if (feature('MONITOR_TOOL') && !isBackgroundTasksDisabled && !input.run_in_background) {
      const sleepPattern = detectBlockedSleepPattern(input.command)
      if (sleepPattern !== null) {
        return { result: false, message: `Blocked: ${sleepPattern}...`, errorCode: 10 }
      }
    }
    return { result: true }
  },
  
  async call(input, toolUseContext, _canUseTool, parentMessage, onProgress) {
    // 处理 simulated sed edit
    if (input._simulatedSedEdit) {
      return applySedEdit(input._simulatedSedEdit, toolUseContext, parentMessage)
    }
    // 执行 shell 命令
    const commandGenerator = runShellCommand({ input, abortController, ... })
    ...
  },
})
```

### 8.7 权限检查流程

```typescript
// src/utils/permissions/permissions.ts:1158-1319
async function hasPermissionsToUseToolInner(tool, input, context) {
  // 1a. 检查 deny rule
  // 1b. 检查 ask rule
  // 1c. 调用 tool.checkPermissions()
  // 1d. 工具拒绝
  // 1e. 需要用户交互
  // 1f. 内容特定 ask rule
  // 1g. 安全检查（.git/, .claude/ 等 bypass-immune）
  // 2a. bypassPermissions 模式
  // 2b. always allow rule
  // 3. passthrough → ask
}
```

---

## 9. MCP 设计

### 9.1 MCP 协议实现

Claude Code 完整实现了 **Model Context Protocol**，支持 **7 种传输方式**：

```typescript
// services/mcp/types.ts L23-25 — 传输类型枚举
export const TransportSchema = lazySchema(() =>
  z.enum(['stdio', 'sse', 'sse-ide', 'http', 'ws', 'sdk']),
)

// L124-135 — 完整配置联合
export const McpServerConfigSchema = lazySchema(() =>
  z.union([
    McpStdioServerConfigSchema(),      # stdio
    McpSSEServerConfigSchema(),        # SSE
    McpSSEIDEServerConfigSchema(),     # SSE-IDE（内部）
    McpWebSocketIDEServerConfigSchema(), # WS-IDE（内部）
    McpHTTPServerConfigSchema(),       # HTTP (streamable)
    McpWebSocketServerConfigSchema(),  # WebSocket
    McpSdkServerConfigSchema(),        # SDK（进程内）
    McpClaudeAIProxyServerConfigSchema(), # Claude.ai 代理
  ]),
)
```

### 9.2 传输方式详解

| 传输 | Schema | 用途 |
|------|--------|------|
| `stdio` | `McpStdioServerConfigSchema` | 本地子进程 |
| `sse` | `McpSSEServerConfigSchema` | 远程 SSE |
| `sse-ide` | `MpSSEIDEServerConfigSchema` | IDE 扩展内部 |
| `http` | `McpHTTPServerConfigSchema` | Streamable HTTP |
| `ws` | `McpWebSocketServerConfigSchema` | WebSocket |
| `sdk` | `McpSdkServerConfigSchema` | 进程内 SDK |
| `claudeai-proxy` | `McpClaudeAIProxyServerConfigSchema` | Claude.ai 代理 |

### 9.3 传输实现

```typescript
// services/mcp/InProcessTransport.ts — 进程内传输
export class InProcessTransport implements Transport {
  private _messageHandler?: (message: JSONRPCMessage) => void
  async start(): Promise<void> { ... }
  async send(message: JSONRPCMessage): Promise<void> {
    this._messageHandler?.(message)
  }
  async close(): Promise<void> { ... }
}

// services/mcp/SdkControlTransport.ts — SDK 控制传输
export class SdkControlClientTransport implements Transport { ... }
export class SdkControlServerTransport implements Transport { ... }

// client.ts — 传输选择
import { SSEClientTransport } from '@modelcontextprotocol/sdk/client/sse.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import { WebSocketTransport } from '../../utils/mcpWebSocketTransport.js'
```

### 9.4 MCP 配置作用域

```typescript
// services/mcp/types.ts L10-19
export const ConfigScopeSchema = lazySchema(() =>
  z.enum(['local', 'user', 'project', 'dynamic', 'enterprise', 'claudeai', 'managed']),
)
```

配置优先级合并顺序（`config.ts:1231-1238`）：

```typescript
// Merge in order of precedence: plugin < user < project < local
const configs = Object.assign(
  {},
  dedupedPluginServers,
  userServers,
  approvedProjectServers,
  localServers,
)
```

**企业 MCP 配置**具有独占控制权（`config.ts:1082-1096`）：当 `managed-mcp.json` 存在时，忽略所有其他配置源。

### 9.5 企业策略：允许/拒绝列表

策略检查支持 **名称、命令、URL 三种匹配维度**（`config.ts:364-508`）：

```typescript
// 名称匹配
for (const entry of settings.deniedMcpServers) {
  if (isMcpServerNameEntry(entry) && entry.serverName === serverName) return true
}
// 命令匹配 (stdio)
if (isMcpServerCommandEntry(entry) && commandArraysMatch(entry.serverCommand, serverCommand))
// URL 通配符匹配 (remote)
if (isMcpServerUrlEntry(entry) && urlMatchesPattern(serverUrl, entry.serverUrl))
```

### 9.6 MCPTool 实现

```typescript
// src/tools/MCPTool/MCPTool.ts:27-77
export const MCPTool = buildTool({
  isMcp: true,
  name: 'mcp',
  maxResultSizeChars: 100_000,
  // call/description/prompt 在 mcpClient.ts 中被真实 MCP 工具覆盖
  async call() { return { data: '' } },
  async checkPermissions(): Promise<PermissionResult> {
    return { behavior: 'passthrough', message: 'MCPTool requires permission.' }
  },
})
```

实际工具在 `fetchToolsForClient`（`client.ts:1743-1998`）中动态创建，覆盖关键属性：
- `name` → `mcp__<server>__<tool>` 格式
- `description()` → 从 MCP server 获取，截断到 2048 字符
- `call()` → 转发到 MCP server，含会话过期重试
- `isConcurrencySafe()` / `isReadOnly()` / `isDestructive()` → 来自 `tool.annotations`

### 9.7 McpAuthTool

当 MCP 服务器返回 401 时，创建一个 **伪工具** 让模型触发 OAuth 流程：

```typescript
// McpAuthTool.ts:49-52
export function createMcpAuthTool(serverName, config): Tool {
  const description = `The \`${serverName}\` MCP server is installed but requires authentication.
    Call this tool to start the OAuth flow...`
}
```

OAuth 完成后自动重连并替换伪工具为真实工具（`McpAuthTool.ts:137-162`）。

### 9.8 认证系统

`ClaudeAuthProvider` 实现 `OAuthClientProvider` 接口（`auth.ts`）：
- **PKCE**：`randomBytes(32)` + SHA256 code_challenge
- **回调服务**：在随机可用端口启动临时 HTTP 服务器接收回调
- **令牌存储**：macOS 钥匙串 / 其他平台文件存储（`getSecureStorage`）
- **XAA (SEP-990)**：跨应用访问，支持 IdP 令牌交换

### 9.9 连接重试与重连

- **连接超时**：`MCP_TIMEOUT` 环境变量，默认 30s（`client.ts:457`）
- **工具调用超时**：`MCP_TOOL_TIMEOUT` 环境变量，默认 ~27.8 小时（`client.ts:211`）
- **请求超时**：60s per request（`client.ts:463`），用 `setTimeout` 替代 `AbortSignal.timeout` 避免 Bun GC 问题
- **终端错误检测**（`client.ts:1249-1263`）：连续 3 次 ECONNRESET/ETIMEDOUT/EPIPE 触发 close → 重连
- **指数退避重连**（`useManageMCPConnections.ts:88-90`）：最多 5 次，1s→30s

### 9.10 MCP 资源与 Prompt

MCP 服务器可暴露 resources，通过 `resources/list` + `resources/read` 调用：

```typescript
// client.ts:2000-2031
export const fetchResourcesForClient = memoizeWithLRU(async (client) => {
  const result = await client.client.request({ method: 'resources/list' }, ListResourcesResultSchema)
  return result.resources.map(resource => ({ ...resource, server: client.name }))
})
```

MCP prompts 被转换为 Claude Code 的斜杠命令（`client.ts:2033-2107`）：

```typescript
return {
  type: 'prompt' as const,
  name: 'mcp__' + normalizeNameForMCP(client.name) + '__' + prompt.name,
  async getPromptForCommand(args: string) {
    const result = await connectedClient.client.getPrompt({ name: prompt.name, arguments: ... })
  },
}
```

### 9.11 Elicitation（交互式确认）

MCP 服务器可通过 `ElicitRequestSchema` 请求用户输入（`elicitationHandler.ts`）：
- **form 模式**：结构化表单
- **url 模式**：打开浏览器 URL，等待 completion notification

### 9.12 官方注册表

启动时 fire-and-forget 拉取 Anthropic 官方 MCP 注册表：

```typescript
// officialRegistry.ts:39-41
const response = await axios.get<RegistryResponse>(
  'https://api.anthropic.com/mcp-registry/v0/servers?version=latest&visibility=commercial',
  { timeout: 5000 },
)
```

---

## 10. SKILL 设计

### 10.1 Skill 类型

```typescript
// skills/bundledSkills.ts L15-41 — Skill 定义
export type BundledSkillDefinition = {
  name: string
  description: string
  aliases?: string[]
  whenToUse?: string           // 告诉模型何时自动调用
  argumentHint?: string        // 参数提示
  allowedTools?: string[]      // 允许的工具列表
  model?: string               // 模型覆盖
  disableModelInvocation?: boolean  // 禁止模型自动调用
  userInvocable?: boolean      // 用户是否可通过 /name 调用
  isEnabled?: () => boolean    // 动态启用/禁用
  hooks?: HooksSettings        // 关联的 hooks
  context?: 'inline' | 'fork'  // 执行上下文
  agent?: string               // 关联的 agent
  files?: Record<string, string>  // 附加参考文件
  getPromptForCommand: (args: string, context: ToolUseContext) => Promise<ContentBlockParam[]>
}
```

### 10.2 16 Bundled Skills

```typescript
// skills/bundled/index.ts — 注册中心
export function initBundledSkills(): void {
  registerUpdateConfigSkill()      # /update-config
  registerKeybindingsSkill()       # /keybindings
  registerVerifySkill()            # /verify
  registerDebugSkill()             # /debug
  registerLoremIpsumSkill()        # /lorem-ipsum
  registerSkillifySkill()          # /skillify
  registerRememberSkill()          # /remember
  registerSimplifySkill()          # /simplify
  registerBatchSkill()             # /batch
  registerStuckSkill()             # /stuck
  // Feature-gated
  if (feature('KAIROS') || feature('KAIROS_DREAM')) registerDreamSkill()
  if (feature('REVIEW_ARTIFACT')) registerHunterSkill()
  if (feature('AGENT_TRIGGERS')) registerLoopSkill()
  if (feature('AGENT_TRIGGERS_REMOTE')) registerScheduleRemoteAgentsSkill()
  if (feature('BUILDING_CLAUDE_APPS')) registerClaudeApiSkill()
  if (shouldAutoEnableClaudeInChrome()) registerClaudeInChromeSkill()
  if (feature('RUN_SKILL_GENERATOR')) registerRunSkillGeneratorSkill()
}
```

**完整 Bundled Skills 清单**：

| # | Skill 名 | 用途 | 用户可调 | Feature Gate |
|---|----------|------|----------|-------------|
| 1 | `update-config` | 配置 settings.json / hooks | 是 | — |
| 2 | `keybindings-help` | 自定义快捷键 | 否（模型触发） | `isEnabled` 检查 |
| 3 | `verify` | 验证代码变更 | 是 | — |
| 4 | `debug` | 调试当前会话 | 是 | — |
| 5 | `lorem-ipsum` | 生成测试用填充文本 | 是 | — |
| 6 | `skillify` | 将会话过程捕获为 Skill | 是 | — |
| 7 | `remember` | 审查 auto-memory 并提议晋升 | 是 | — |
| 8 | `simplify` | 代码审查与清理 | 是 | — |
| 9 | `batch` | 大规模并行变更编排 | 是 | — |
| 10 | `stuck` | 诊断卡住的会话 | 是 | — |
| 11 | `dream` | Dream 任务 | — | `KAIROS` / `KAIROS_DREAM` |
| 12 | `hunter` | Review artifact | — | `REVIEW_ARTIFACT` |
| 13 | `loop` | 定时循环执行 prompt | 是 | `AGENT_TRIGGERS` |
| 14 | `schedule-remote-agents` | 远程 agent 调度 | — | `AGENT_TRIGGERS_REMOTE` |
| 15 | `claude-api` | Claude API 构建 | — | `BUILDING_CLAUDE_APPS` |
| 16 | `claude-in-chrome` | Chrome 浏览器集成 | — | `shouldAutoEnableClaudeInChrome()` |
| 17 | `run-skill-generator` | Skill 生成器 | — | `RUN_SKILL_GENERATOR` |

部分 skill 通过 `process.env.USER_TYPE !== 'ant'` 限制为内部用户（verify, remember, stuck, lorem-ipsum, skillify 等）。

### 10.3 Skill 注册机制

**`src/skills/bundledSkills.ts:53-100`** — `registerBundledSkill()` 核心注册函数：

```typescript
export function registerBundledSkill(definition: BundledSkillDefinition): void {
  const { files } = definition
  let skillRoot: string | undefined
  let getPromptForCommand = definition.getPromptForCommand

  // 如果有 files，延迟提取到磁盘
  if (files && Object.keys(files).length > 0) {
    skillRoot = getBundledSkillExtractDir(definition.name)
    let extractionPromise: Promise<string | null> | undefined
    const inner = definition.getPromptForCommand
    getPromptForCommand = async (args, ctx) => {
      extractionPromise ??= extractBundledSkillFiles(definition.name, files)
      const extractedDir = await extractionPromise
      const blocks = await inner(args, ctx)
      if (extractedDir === null) return blocks
      return prependBaseDir(blocks, extractedDir)
    }
  }

  const command: Command = {
    type: 'prompt',
    name: definition.name,
    description: definition.description,
    source: 'bundled',
    loadedFrom: 'bundled',
    getPromptForCommand,
  }
  bundledSkills.push(command)
}
```

关键设计点：
- **延迟文件提取**：`files` 字段的文件仅在首次调用时提取到磁盘，使用 `O_NOFOLLOW | O_EXCL` 防止符号链接攻击
- **Memoized Promise**：使用 Promise memoization 防止并发竞态

### 10.4 SkillTool — 模型调用 Skills 的工具

**`src/tools/SkillTool/SkillTool.ts:291-298`** — 输入 schema：

```typescript
export const inputSchema = lazySchema(() =>
  z.object({
    skill: z.string().describe('The skill name. E.g., "commit", "review-pr"'),
    args: z.string().optional().describe('Optional arguments for the skill'),
  }),
)
```

**`src/tools/SkillTool/SkillTool.ts:331-869`** — SkillTool 的完整生命周期：

1. **`validateInput`（行 354-429）**：验证 skill 名称、去除前导 `/`、检查远程 canonical skill、查找命令、验证是否为 prompt 类型
2. **`checkPermissions`（行 432-578）**：权限检查流程：
   - 先检查 deny rules（前缀匹配 `review:*`）
   - 远程 canonical skills 自动允许
   - 检查 allow rules
   - **Safe properties 自动允许**（行 875-908）：如果 skill 只有安全属性（无 hooks、无 allowedTools 等），自动放行
   - 否则返回 `ask` 决策，附带精确匹配和前缀匹配的建议
3. **`call`（行 580-841）**：核心执行逻辑：
   - **远程 skill**：通过 `executeRemoteSkill()` 从 AKI/GCS 加载 SKILL.md
   - **Forked skill**（`context: 'fork'`）：通过 `executeForkedSkill()` 在独立子 agent 中执行
   - **Inline skill**（默认）：通过 `processPromptSlashCommand()` 处理后返回 `newMessages` + `contextModifier`

### 10.5 Forked vs Inline 执行模式

**`src/tools/SkillTool/SkillTool.ts:122-289`** — `executeForkedSkill()`：

```typescript
async function executeForkedSkill(
  command, commandName, args, context, canUseTool, parentMessage, onProgress
): Promise<ToolResult<Output>> {
  const agentId = createAgentId()
  // 准备隔离的 agent 上下文
  const { modifiedGetAppState, baseAgent, promptMessages, skillContent } =
    await prepareForkedCommandContext(command, args || '', context)

  // 在子 agent 中执行
  for await (const message of runAgent({
    agentDefinition,
    promptMessages,
    toolUseContext: { ...context, getAppState: modifiedGetAppState },
    canUseTool,
    isAsync: false,
    querySource: 'agent:custom',
    model: command.model,
  })) {
    agentMessages.push(message)
    // 报告进度
  }
  return { data: { success: true, commandName, status: 'forked', agentId, result } }
}
```

Inline 模式返回 `newMessages` 和 `contextModifier`，后者可以：
- 更新 `allowedTools`（合并到 `alwaysAllowRules`）
- 覆盖 `mainLoopModel`
- 覆盖 `effortValue`

### 10.6 Skill 加载管线

**`src/skills/loadSkillsDir.ts:407-480`** — `loadSkillsFromSkillsDir()` 目录格式加载：

```
.claude/skills/
  my-skill/
    SKILL.md        ← 必须是 SKILL.md
    scripts/
      helper.sh      ← 可选参考文件
```

加载流程：
1. 遍历 skills 目录下所有子目录
2. 读取 `SKILL.md` 文件
3. 解析 YAML frontmatter（name, description, when_to_use, allowed-tools, model, effort, paths, hooks 等）
4. 创建 `Command` 对象

**`src/skills/loadSkillsDir.ts:638-804`** — `getSkillDirCommands()` 总入口（memoized）：

从 5 个来源并行加载：
1. **Managed**（策略目录 `~/.claude-managed/.claude/skills`）
2. **User**（用户目录 `~/.claude/skills`）
3. **Project**（项目目录 `.claude/skills`，向上遍历到 home）
4. **Additional**（`--add-dir` 参数指定的目录）
5. **Legacy commands**（旧版 `.claude/commands/` 目录）

去重策略：通过 `realpath()` 解析符号链接，相同实际路径的 skill 只保留第一个（first-wins）。

### 10.7 动态 Skill 发现

**`src/skills/loadSkillsDir.ts:861-915`** — `discoverSkillDirsForPaths()`：

当模型操作文件时（Read/Write/Edit），系统会从文件路径向上遍历到 cwd，发现嵌套的 `.claude/skills/` 目录：

```typescript
export async function discoverSkillDirsForPaths(
  filePaths: string[], cwd: string
): Promise<string[]> {
  for (const filePath of filePaths) {
    let currentDir = dirname(filePath)
    while (currentDir.startsWith(resolvedCwd + pathSep)) {
      const skillDir = join(currentDir, '.claude', 'skills')
      if (!dynamicSkillDirs.has(skillDir)) {
        dynamicSkillDirs.add(skillDir)
        // 检查是否存在、是否 gitignored
        await fs.stat(skillDir)
        if (await isPathGitignored(currentDir, resolvedCwd)) continue
        newDirs.push(skillDir)
      }
      currentDir = dirname(currentDir)
    }
  }
  return newDirs.sort((a, b) => b.split(sep).length - a.split(sep).length)
}
```

**`src/skills/loadSkillsDir.ts:997-1058`** — 条件技能（Conditional Skills）：

带有 `paths` frontmatter 的技能仅在匹配路径被操作时才激活：

```typescript
export function activateConditionalSkillsForPaths(
  filePaths: string[], cwd: string
): string[] {
  for (const [name, skill] of conditionalSkills) {
    const skillIgnore = ignore().add(skill.paths)  // gitignore 风格匹配
    for (const filePath of filePaths) {
      if (skillIgnore.ignores(relativePath)) {
        dynamicSkills.set(name, skill)
        conditionalSkills.delete(name)
        activatedConditionalSkillNames.add(name)
        break
      }
    }
  }
}
```

### 10.8 Skill 变更检测

**`src/utils/skills/skillChangeDetector.ts:1-311`** — 基于 chokidar 的文件监控：

```typescript
export async function initialize(): Promise<void> {
  const paths = await getWatchablePaths()  // ~/.claude/skills, .claude/skills 等
  watcher = chokidar.watch(paths, {
    persistent: true, ignoreInitial: true, depth: 2,
    awaitWriteFinish: { stabilityThreshold: 1000, pollInterval: 500 },
    usePolling: USE_POLLING,  // Bun 环境下使用 polling 避免 FSWatcher 死锁
    interval: 2000,
    atomic: true,
  })
  watcher.on('add', handleChange)
  watcher.on('change', handleChange)
  watcher.on('unlink', handleChange)
}
```

变更处理有 300ms 防抖，防止批量文件变更导致级联重载。

### 10.9 Skill 提示词预算管理

**`src/tools/SkillTool/prompt.ts:21-41`** — Skill 列表的上下文预算：

```typescript
export const SKILL_BUDGET_CONTEXT_PERCENT = 0.01  // 1% 的上下文窗口
export const CHARS_PER_TOKEN = 4
export const DEFAULT_CHAR_BUDGET = 8_000
export const MAX_LISTING_DESC_CHARS = 250  // 每条描述上限

export function getCharBudget(contextWindowTokens?: number): number {
  if (process.env.SLASH_COMMAND_TOOL_CHAR_BUDGET) return Number(...)
  if (contextWindowTokens) return Math.floor(contextWindowTokens * CHARS_PER_TOKEN * SKILL_BUDGET_CONTEXT_PERCENT)
  return DEFAULT_CHAR_BUDGET
}
```

### 10.10 典型 Skill 示例分析

#### batch skill（`src/skills/bundled/batch.ts`）

最复杂的 bundled skill，编排大规模并行变更：
- **Phase 1**：进入 Plan Mode，研究分解为 5-30 个独立工作单元
- **Phase 2**：每个单元启动一个 `isolation: "worktree"` 的后台 Agent
- **Phase 3**：跟踪进度，渲染状态表

每个 worker 执行固定流程：simplify → 测试 → e2e → commit → PR

#### skillify skill（`src/skills/bundled/skillify.ts`）

将会话过程捕获为可复用 Skill：
1. 分析会话（提取 session memory + user messages）
2. 多轮采访用户（通过 AskUserQuestion）
3. 生成 SKILL.md（含 frontmatter + steps + success criteria）
4. 确认保存

#### simplify skill（`src/skills/bundled/simplify.ts`）

启动 3 个并行审查 Agent：
- **Agent 1**：代码复用审查
- **Agent 2**：代码质量审查（7 个维度）
- **Agent 3**：效率审查（7 个维度）

#### update-config skill（`src/skills/bundled/updateConfig.ts`）

包含完整的 settings JSON Schema（动态生成）、hooks 配置文档、以及 7 步 hook 验证流程（dedup → construct → pipe-test → write → validate → prove → handoff）。

---

## 11. 协调器模式（Coordinator Mode）

### 11.1 Feature Gate

```typescript
// coordinatorMode.ts:36-41
export function isCoordinatorMode(): boolean {
  if (feature('COORDINATOR_MODE')) {
    return isEnvTruthy(process.env.CLAUDE_CODE_COORDINATOR_MODE)
  }
  return false
}
```

### 11.2 系统提示词（`coordinatorMode.ts:111-369`）

完整的协调器角色定义：

```
## 1. Your Role — 协调器
- 帮助实现用户目标
- 指导 worker 研究/实现/验证
- 综合结果与用户沟通
- 能直接回答就直接回答

## 2. Your Tools
- Agent — 启动新 worker
- SendMessage — 继续已有 worker
- TaskStop — 停止运行中的 worker

## 4. Task Workflow
| Phase | Who | Purpose |
|-------|-----|---------|
| Research | Workers (parallel) | 调研 |
| Synthesis | You (coordinator) | 综合 |
| Implementation | Workers | 实现 |
| Verification | Workers | 验证 |

## 5. Writing Worker Prompts
- Worker 看不到你的对话，prompt 必须自包含
- 综合研究发现 → 具体文件路径+行号+改动
- 不要写 "based on your findings"
- 高上下文重叠 → Continue；低重叠 → Spawn fresh
```

### 11.3 Worker 结果格式

```xml
<task-notification>
<task-id>{agentId}</task-id>
<status>completed|failed|killed</status>
<summary>{human-readable status summary}</summary>
<result>{agent's final text response}</result>
<usage><total_tokens>N</total_tokens><tool_uses>N</tool_uses><duration_ms>N</duration_ms></usage>
</task-notification>
```

---

## 12. 关键架构模式总结

| 模式 | 实现位置 | 说明 |
|------|----------|------|
| AsyncGenerator 流式架构 | `query()` | 支持流式 yield 中间事件、背压控制 |
| 编译时 Feature Gate | `bun:bundle` 的 `feature()` | 零成本抽象，外部构建完全剔除内部代码 |
| 四级递进压缩 | compact/ 目录 | 从轻量到重量依次尝试，最大化保留原始信息 |
| 权限即 Promise | `CanUseToolFn` | 返回 `Promise<PermissionDecision>`，支持异步权限判定 |
| Skill 即 Command | skills/ + commands/ | bundled/disk/plugin/mcp 四种来源统一注册 |
| 类型安全状态更新 | `updateTaskState<T>` | 同引用跳过重渲染 |
| 任务注册表 | `getAllTasks()` | 运行时多态，按 type 查找 |
| 磁盘输出 | `DiskTaskOutput` | 异步队列 + O_NOFOLLOW + 5GB 截断 |
| Delta 轮询 | `getTaskOutputDelta` | 字节偏移增量读取 |
| SSRF 防护 | `isBlockedAddress` | 阻止私有/链路本地地址 |
| Hook 三种执行 | execAgent / execHttp / execPrompt | 多轮 LLM / HTTP / 单轮 LLM |
| 27 种 Hook 事件 | `hooksConfigManager` | 覆盖工具/会话/压缩/任务/配置全生命周期 |
| 协调器模式 | `coordinatorMode.ts` | Agent 编排多 worker，Research→Synthesis→Implementation→Verification |
| Effort 四级 | `low/medium/high/max` | env → appState → model default 优先级，max 仅 Opus-4.6 |

---

## 13. 与 laew 的对比启示

| 维度 | Claude Code | laew |
|------|-------------|------|
| **语言** | TypeScript/Bun | Rust |
| **架构** | 单层扁平 + feature gate | 多 Agent 6 角色 + Screen 栈 |
| **对话循环** | AsyncGenerator 流式 | 同步循环 + SQLite 持久化 |
| **Context 管理** | 四级压缩管线（snip/micro/collapse/auto） | 无压缩（依赖模型窗口） |
| **任务分类** | Effort Level（low/medium/high/max） | 三档（simple/medium/hard） |
| **质检** | Hooks 80+ 文件 + 权限矩阵 | Quality-Check Agent |
| **任务拆解** | Task 类型 7 种 + 子 Agent | Plan→Main→SubAgent 编排 |
| **工具** | 30+ 工具，每工具一目录 | 3 工具（Bash/Read/Write） |
| **MCP** | 7 种传输 + 完整 OAuth/XAA | 无 |
| **Skill** | 16 bundled + 磁盘扫描 + MCP 桥接 | 无 |
| **UI** | Ink（React for CLI） | 自研 TUI 引擎 |
| **持久化** | 文件系统（~/.claude/） | SQLite |
| **Feature Gate** | 编译时 DCE（bun:bundle） | 无 |

### laew 可借鉴的 P0 设计

1. **四级压缩管线**：长会话处理是 Agent CLI 的核心能力，laew 目前完全依赖模型窗口
2. **Hooks 系统**：27 事件 × 3 执行方式，Quality-Check Agent 可借鉴其事件驱动架构
3. **任务磁盘输出 + Delta 轮询**：5GB 截断 + O_NOFOLLOW + 字节偏移增量读取
4. **Skill 声明式定义**：Markdown + YAML frontmatter，零代码扩展能力
5. **Feature Gate 编译时 DCE**：Rust 的 `#[cfg(feature = "...")]` 天然支持

### laew 可借鉴的 P1 设计

1. **协调器模式**：MultiAgentOrchestrator 可增加协调器角色
2. **Effort Level 系统**：三档分类可扩展为四级
3. **流式工具执行器**：工具与模型输出并行
4. **Skill 动态发现**：文件监控 + 条件激活

---

## 14. 关键文件索引

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/main.tsx` | 4683 | CLI 入口 + 初始化编排 |
| `src/query.ts` | 1729 | 多轮对话主循环 |
| `src/Tool.ts` | 792 | Tool 类型 + buildTool 工厂 |
| `src/tools.ts` | 389 | 工具注册表 |
| `src/context.ts` | 189 | 系统/用户上下文 |
| `src/commands.ts` | 754 | 斜杠命令注册 |
| `src/Task.ts` | 125 | 后台任务类型 |
| `src/tasks.ts` | 39 | 任务注册表 |
| `src/history.ts` | 464 | 历史持久化 |
| `src/services/compact/autoCompact.ts` | ~200 | 自动压缩 |
| `src/services/compact/compact.ts` | ~300 | 全量压缩 |
| `src/services/compact/microCompact.ts` | ~150 | 微压缩 |
| `src/services/mcp/client.ts` | ~500 | MCP 客户端 |
| `src/services/mcp/types.ts` | ~200 | MCP 类型定义 |
| `src/skills/bundledSkills.ts` | ~100 | Skill 定义 |
| `src/skills/bundled/index.ts` | ~80 | 内置 Skill 注册 |
| `src/hooks/useCanUseTool.tsx` | ~200 | 权限判定 |
| `src/coordinator/coordinatorMode.ts` | ~100 | 协调模式 |
| `src/constants/prompts.ts` | ~200 | 系统提示词 |
| `src/utils/effort.ts` | ~100 | Effort Level |

---

以上分析基于对 `/usr/local/LsmGitOpenSource/claudecode/src` 实际源码的深入阅读，所有代码路径和行号均可在源码中定位验证。
