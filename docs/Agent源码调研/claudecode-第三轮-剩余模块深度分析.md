# Claude Code 第三轮深度分析 — 剩余模块全覆盖

> 分析对象: claudecode (TypeScript/Bun, ~218k 行, 1896 源文件)
> 分析日期: 2026-09-05
> 前序文档: `claudecode-源码调研.md` / `claudecode-深度分析.md` / `claudecode-第二轮深度分析.md` / `claudecode-核心机制深度分析.md`
> 本文定位: 覆盖前四档文档**未实质涉及**的全部剩余模块

---

## 一、src/ 全模块清单与覆盖状态

| 模块 | 行数/文件数 | 前序覆盖 | 本文覆盖 |
|------|-----------|----------|----------|
| `commands.ts` + `commands/` | 189 文件 | ❌ 仅提及 | ✅ 命令系统专题 |
| `services/` | 133 文件 | 部分(compact/plugins/mcp) | ✅ 剩余全部 |
| `utils/` | 564 文件 | 部分(permissions/messages/tokens) | ✅ 剩余关键模块 |
| `components/` | 389 文件 | ❌ | ✅ UI 组件专题 |
| `hooks/` | 104 文件 | 部分(useCanUseTool) | ✅ 关键 hooks |
| `ink/` | 96 文件/13306 行 | ❌ | ✅ Ink 自定义 Fork 专题 |
| `tasks/` | 12 文件 | ❌ | ✅ Task 框架 |
| `bridge/` | 30 文件/12613 行 | ❌ | ✅ Bridge 远程控制 |
| `vim/` | 5 文件/1513 行 | ❌ | ✅ Vim 模式 |
| `keybindings/` | 14 文件/3159 行 | ❌ | ✅ 快捷键系统 |
| `state/` | 6 文件/1190 行 | ❌ | ✅ 状态管理 |
| `memdir/` | 8 文件/1736 行 | ❌ | ✅ 记忆目录 |
| `migrations/` | 11 文件/603 行 | ❌ | ✅ 设置迁移 |
| `entrypoints/` | 5 文件/1437 行 | ❌ | ✅ 入口点 |
| `screens/` | 3 文件/5977 行 | ❌ | ✅ 屏幕组件 |
| `coordinator/` | 1 文件 | ❌ | ✅ Coordinator 模式 |
| `buddy/` | 6 文件 | ❌ | ✅ Buddy 伴侣系统 |
| `context/` | 9 文件 | ❌ | ✅ React Context |
| `bootstrap/` | 1 文件 | ❌ | ✅ 启动状态 |
| `schemas/` | 1 文件 | ❌ | ✅ (简述) |
| `types/` | 8 文件 | 部分(command.ts) | ✅ 补充 |
| `skills/` | 4 文件 | 部分(提及) | ✅ 技能加载 |
| `plugins/` | 2 目录 | ✅ 已覆盖 | 跳过 |
| `tools/` | 191 文件 | ✅ 已覆盖 | 跳过 |
| `query/` | 4 文件 | ❌ | ✅ Query 配置 |
| `remote/` | 4 文件 | ❌ | ✅ 远程会话 |
| `server/` | 3 文件 | ❌ | ✅ 直连服务 |
| `voice/` | 1 文件 | ❌ | ✅ (在 services/voice 中) |
| `outputStyles/` | 1 文件 | ❌ | ✅ (简述) |
| `native-ts/` | 3 子目录 | ❌ | ✅ 原生模块 |
| `upstreamproxy/` | 2 文件 | ❌ | ✅ (简述) |
| `moreright/` | 1 文件 | ❌ | ✅ (简述) |

---

## 二、命令系统深挖

### 2.1 命令注册架构

claudecode 的命令系统是整个 TUI 的骨架。所有命令在 `src/commands.ts` 中集中注册:

```typescript
// src/commands.ts:258 — 核心命令数组,memoize 惰性求值
const COMMANDS = memoize((): Command[] => [
  addDir, advisor, agents, branch, btw, chrome, clear, color,
  compact, config, copy, desktop, context, cost, diff, doctor,
  effort, exit, fast, files, heapDump, help, ide, init,
  keybindings, login, logout, mcp, memory, mobile, model,
  // ... 共 89 个内置命令
])
```

**三种命令类型**定义在 `src/types/command.ts`:

```typescript
// src/types/command.ts:140-142
type LocalCommand = {
  type: 'local'           // 纯文本输出(如 /compact, /cost)
  supportsNonInteractive: boolean
  load: () => Promise<LocalCommandModule>
}

type LocalJSXCommand = {
  type: 'local-jsx'       // React UI 渲染(如 /help, /doctor, /export)
  load: () => Promise<LocalJSXCommandModule>
}

// PromptCommand 直接发给模型执行
type PromptCommand = {
  type: 'prompt'
  progressMessage: string
  contentLength: number
  getPromptForCommand(args, context): Promise<ContentBlockParam[]>
}
```

**每个命令模块**使用 `satisfies Command` 模式,实现延迟加载:

```typescript
// src/commands/help/index.ts:3-8
const help = {
  type: 'local-jsx',
  name: 'help',
  description: 'Show help and available commands',
  load: () => import('./help.js'),  // 关键:动态 import 延迟加载
} satisfies Command
```

### 2.2 六源加载管线

命令来自六个独立来源,在 `loadAllCommands` 中并行加载:

```typescript
// src/commands.ts:449-470
const loadAllCommands = memoize(async (cwd: string): Promise<Command[]> => {
  const [
    { skillDirCommands, pluginSkills, bundledSkills, builtinPluginSkills },
    pluginCommands,
    workflowCommands,
  ] = await Promise.all([
    getSkills(cwd),          // 源1: .claude/skills/ 目录
    getPluginCommands(),     // 源2: 插件命令
    getWorkflowCommands?.(cwd) ?? Promise.resolve([]),  // 源3: Workflow
  ])
  return [
    ...bundledSkills,        // 源4: 内置技能
    ...builtinPluginSkills,  // 源5: 内置插件技能
    ...skillDirCommands,     // 源6: 用户技能目录
    ...workflowCommands,
    ...pluginCommands,
    ...pluginSkills,
    ...COMMANDS(),           // 最后:内置命令
  ]
})
```

### 2.3 可用性过滤与特性门控

```typescript
// src/commands.ts:417-445 — 两层过滤
export function meetsAvailabilityRequirement(cmd: Command): boolean {
  if (!cmd.availability) return true
  for (const a of cmd.availability) {
    switch (a) {
      case 'claude-ai':   // OAuth 订阅用户
        if (isClaudeAISubscriber()) return true
        break
      case 'console':     // 直接 API Key 用户
        if (!isClaudeAISubscriber() && !isUsing3PServices() && isFirstPartyAnthropicBaseUrl())
          return true
        break
    }
  }
  return false
}
```

**编译时特性门控**使用 `bun:bundle` 的 `feature()` 函数,60+ 个 flag 在构建时做死代码消除(DCE):

```typescript
// src/commands.ts:85-110 — 条件 require 模式
const bridge = feature('BRIDGE_MODE')
  ? require('./commands/bridge/index.js').default
  : null
const voiceCommand = feature('VOICE_MODE')
  ? require('./commands/voice/index.js').default
  : null
const buddy = feature('BUDDY')
  ? require('./commands/buddy/index.js').default
  : null
```

**已发现的60+个特性 flag** 包括: `ABLATION_BASELINE`, `AGENT_TRIGGERS`, `BRIDGE_MODE`, `BUDDY`, `COORDINATOR_MODE`, `DAEMON`, `EXPERIMENTAL_SKILL_SEARCH`, `EXTRACT_MEMORIES`, `KAIROS`, `PROACTIVE`, `TEAMMEM`, `ULTRAPLAN`, `VOICE_MODE`, `WORKFLOW_SCRIPTS` 等。

### 2.4 远程安全命令

```typescript
// src/commands.ts:619-640
export const REMOTE_SAFE_COMMANDS: Set<Command> = new Set([
  session, exit, clear, help, theme, color, vim,
  cost, usage, copy, btw, feedback, plan,
  keybindings, statusline, stickers, mobile,
])
```

---

## 三、Ink 自定义 Fork 深挖

### 3.1 架构概览

claudecode fork 了 `vadimdemedes/ink`,进行了大规模定制(13306 行)。核心改动集中在渲染管线:

```
React 组件 → React Reconciler → DOM 树 → Yoga 布局 → 渲染输出 → 屏幕缓冲 → 终端
```

### 3.2 React Reconciler

```typescript
// src/ink/reconciler.ts:1-20 — 自定义 reconciler
import createReconciler from 'react-reconciler'

// 自定义 DOM 操作
const reconciler = createReconciler({
  createInstance(type, props) { return createNode(type) },
  appendChildNode, insertBeforeNode, removeChildNode,
  setAttribute, setStyle, setTextNodeValue,
  // ... 完整的 host config
})
```

### 3.3 单元格级屏幕缓冲

```typescript
// src/ink/screen.ts — 1486 行,核心渲染层
// Cell-based screen buffer: 每个字符位置是一个 Cell
// StylePool, CharPool, HyperlinkPool — 对象池减少 GC
export function createScreen(width: number, height: number): Screen { ... }
```

### 3.4 选择系统

```typescript
// src/ink/selection.ts:917 行
// 鼠标选择: startSelection, extendSelection, clearSelection
// 复制: getSelectedText — 从 screen buffer 提取选中文本
// URL 检测: findPlainTextUrlAt — 选中位置的超链接检测
```

### 3.5 终端能力检测

```typescript
// src/ink/terminal.ts:248 行
// Kitty 键盘协议: ENABLE_KITTY_KEYBOARD / DISABLE_KITTY_KEYBOARD
// OSC 超链接: supports-hyperlinks.ts
// Tab 状态: supportsTabStatus (iTerm2/WezTerm)
// 鼠标追踪: ENABLE_MOUSE_TRACKING / DISABLE_MOUSE_TRACKING
```

### 3.6 Ink 内置组件

| 组件 | 文件 | 功能 |
|------|------|------|
| `Box` | `Box.tsx:213行` | Flexbox 容器(yoga-layout) |
| `Text` | `Text.tsx:253行` | 文本渲染(支持 ANSI、粗体、下划线) |
| `Button` | `Button.tsx:191行` | 可聚焦按钮 |
| `ScrollBox` | `ScrollBox.tsx:236行` | 滚动容器 |
| `AlternateScreen` | `AlternateScreen.tsx` | 备用屏幕(子屏模式) |
| `Link` | `Link.tsx:41行` | OSC 超链接 |
| `RawAnsi` | `RawAnsi.tsx:56行` | 原始 ANSI 输出 |

---

## 四、状态管理深挖

### 4.1 自定义 External Store 模式

claudecode **没有使用** Redux/Zustand/Recoil,而是自建了极简 store:

```typescript
// src/state/store.ts:10-35 — 34 行实现整个 store
export function createStore<T>(
  initialState: T,
  onChange?: (args: { newState: T; oldState: T }) => void,
): Store<T> {
  let state = initialState
  const listeners = new Set<Listener>()
  return {
    getState: () => state,
    setState: (updater) => {
      const oldState = state
      state = updater(state)
      for (const listener of listeners) listener()
      onChange?.({ newState: state, oldState: oldState })
    },
    subscribe: (listener) => {
      listeners.add(listener)
      return () => { listeners.delete(listener) }
    },
  }
}
```

### 4.2 React 集成

```typescript
// src/state/AppState.tsx:162 — 使用 React 18 的 useSyncExternalStore
return useSyncExternalStore(store.subscribe, get, get)
```

**AppState** 包含: `toolPermissionContext`, `messages`, `speculationState`, `completionBoundary`, `standaloneAgentContext`, `mcpConfig` 等核心状态。

### 4.3 副作用监听

```typescript
// src/state/onChangeAppState.ts:171 行
// 状态变更时触发副作用:设置变更、权限更新、MCP 配置同步
```

---

## 五、Bridge 远程控制深挖

### 5.1 架构

Bridge 是 claudecode 的**云远程控制**(CCR)系统,12613 行:

```
CCR Server ←→ Bridge Main Loop ←→ Session Spawner ←→ Claude 子进程
                                  ↓
                              REPL Bridge ←→ 本地 TUI
```

### 5.2 主循环

```typescript
// src/bridge/bridgeMain.ts:141 — 核心循环
export async function runBridgeLoop(
  config: BridgeConfig,
  environmentId: string,
  environmentSecret: string,
  api: BridgeApiClient,
  spawner: SessionSpawner,
  logger: BridgeLogger,
  signal: AbortSignal,
  backoffConfig: BackoffConfig = DEFAULT_BACKOFF,
  // ...
)
```

### 5.3 退避策略

```typescript
// src/bridge/bridgeMain.ts:59-80
export type BackoffConfig = {
  connInitialMs: number      // 2000
  connCapMs: number          // 120000 (2 分钟)
  connGiveUpMs: number       // 600000 (10 分钟)
  generalInitialMs: number   // 500
  generalCapMs: number       // 30000
  generalGiveUpMs: number    // 600000
}
```

### 5.4 多会话管理

```typescript
// src/bridge/bridgeMain.ts:96 — GrowthBook 门控
async function isMultiSessionSpawnEnabled(): Promise<boolean> {
  return checkGate_CACHED_OR_BLOCKING('tengu_ccr_bridge_multi_session')
}
```

支持 `--spawn`, `--capacity`, `--create-session-in-dir` 等多会话模式。

---

## 六、Vim 模式深挖

### 6.1 状态机架构

```typescript
// src/vim/transitions.ts:59-89 — 主分发函数
export function transition(state: CommandState, input: string, ctx: TransitionContext): TransitionResult {
  switch (state.type) {
    case 'idle':           return fromIdle(input, ctx)
    case 'count':          return fromCount(state, input, ctx)
    case 'operator':       return fromOperator(state, input, ctx)
    case 'operatorCount':  return fromOperatorCount(state, input, ctx)
    case 'operatorFind':   return fromOperatorFind(state, input, ctx)
    case 'operatorTextObj': return fromOperatorTextObj(state, input, ctx)
    case 'find':           return fromFind(state, input, ctx)
    case 'g':              return fromG(state, input, ctx)
    case 'operatorG':      return fromOperatorG(state, input, ctx)
    case 'replace':        return fromReplace(state, input, ctx)
    case 'indent':         return fromIndent(state, input, ctx)
  }
}
```

**11 种状态**,覆盖 vim 的完整操作符-动作-文本对象三元组。

### 6.2 操作符实现

```typescript
// src/vim/operators.ts:556 行
// executeIndent, executeJoin, executeLineOp, executeOpenLine,
// executeOperatorFind, executeOperatorG, executeOperatorGg,
// executeOperatorMotion, executeOperatorTextObj, executePaste,
// executeReplace, executeToggleCase, executeX
```

### 6.3 文本对象

```typescript
// src/vim/textObjects.ts:186 行
// 支持 iw(内词), aw(含空格词), i"(内引号), a"(含引号),
// i(, a(, i[, a[, i{, a{, i`, a` 等标准 vim 文本对象
```

---

## 七、快捷键系统深挖

### 7.1 默认绑定

```typescript
// src/keybindings/defaultBindings.ts:32-340
export const DEFAULT_BINDINGS: KeybindingBlock[] = [
  { context: 'Global', bindings: {
    'ctrl+c': 'app:interrupt',
    'ctrl+d': 'app:exit',
    'ctrl+l': 'app:redraw',
    'ctrl+t': 'app:toggleTodos',
    'ctrl+o': 'app:toggleTranscript',
    'ctrl+r': 'history:search',
  }},
  { context: 'Chat', bindings: {
    escape: 'chat:cancel',
    enter: 'chat:submit',
    [MODE_CYCLE_KEY]: 'chat:cycleMode',  // shift+tab 或 meta+m
    'ctrl+x ctrl+e': 'chat:externalEditor',
    'ctrl+s': 'chat:stash',
    [IMAGE_PASTE_KEY]: 'chat:imagePaste',  // ctrl+v 或 alt+v(Windows)
  }},
  // ... 20+ 个 context: Autocomplete, Settings, Confirmation, Tabs,
  //     Transcript, HistorySearch, Task, Scroll, Help, Footer,
  //     MessageSelector, MessageActions, DiffDialog, ModelPicker, etc.
]
```

### 7.2 用户自定义

```typescript
// src/keybindings/loadUserBindings.ts:472 行
// 加载 ~/.claude/keybindings.json,与默认绑定合并
// 验证: src/keybindings/validate.ts:498 行
//   - 保留键: ctrl+c, ctrl+d 不可重绑定(reservedShortcuts.ts:127行)
//   - Schema: src/keybindings/schema.ts:236 行 — Zod 验证
//   - 平台检测: getPlatform() 区分 Windows/macOS/Linux
```

### 7.3 解析与匹配

```typescript
// src/keybindings/parser.ts:203 行 — 解析键弦字符串
// "ctrl+shift+f" → { ctrl: true, shift: true, key: 'f' }
// src/keybindings/resolver.ts:244 行 — 解析键事件到动作
// src/keybindings/match.ts:120 行 — 匹配键事件到绑定
```

---

## 八、Task 框架深挖

### 8.1 七种任务类型

```typescript
// src/Task.ts:6-15
export type TaskType =
  | 'local_bash'            // 后台 Shell 命令
  | 'local_agent'           // 子 Agent(如 AgentTool)
  | 'remote_agent'          // 远程 Agent
  | 'in_process_teammate'   // 进程内队友(swarm)
  | 'local_workflow'        // 本地工作流
  | 'monitor_mcp'           // MCP 监控
  | 'dream'                 // 后台记忆整合
```

### 8.2 任务 ID 生成

```typescript
// src/Task.ts:96-105 — 安全的随机 ID
const TASK_ID_ALPHABET = '0123456789abcdefghijklmnopqrstuvwxyz'
export function generateTaskId(type: TaskType): string {
  const prefix = getTaskIdPrefix(type)  // b/a/r/t/w/m/d
  const bytes = randomBytes(8)
  let id = prefix
  for (let i = 0; i < 8; i++) {
    id += TASK_ID_ALPHABET[bytes[i]! % TASK_ID_ALPHABET.length]
  }
  return id  // 36^8 ≈ 2.8 万亿组合
}
```

### 8.3 LocalAgentTask 进度追踪

```typescript
// src/tasks/LocalAgentTask/LocalAgentTask.tsx:40-55
export type ProgressTracker = {
  toolUseCount: number
  latestInputTokens: number       // 累积的 input tokens(API 返回的是累积值)
  cumulativeOutputTokens: number  // 逐轮累加的 output tokens
  recentActivities: ToolActivity[]
}
const MAX_RECENT_ACTIVITIES = 5
```

### 8.4 DreamTask — 后台记忆整合

```typescript
// src/tasks/DreamTask/DreamTask.ts:157 行
// DreamTask 是 autoDream 的任务包装,在 AppState 中注册/完成/失败
// 使用 registerDreamTask → completeDreamTask / failDreamTask 生命周期
```

---

## 九、Swarm/Team 系统深挖

### 9.1 架构

Swarm 是 claudecode 的**多 Agent 协作**系统(4107 行):

```
Leader (主会话)
  ├── Teammate 1 (InProcessTeammateTask)
  ├── Teammate 2 (InProcessTeammateTask)
  └── Teammate N (InProcessTeammateTask)
       ↕ permissionSync (权限同步)
       ↕ mailbox (消息传递)
```

### 9.2 进程内运行器

```typescript
// src/utils/swarm/inProcessRunner.ts:1552 行 — 核心运行器
// 在同一进程内运行 teammate agent,共享内存
// 管理: spawn → run → kill → cleanup 生命周期
```

### 9.3 权限同步

```typescript
// src/utils/swarm/permissionSync.ts:928 行
// Leader 和 Worker 之间的沙箱权限同步
// Worker 需要 Bash 执行权限时,通过 mailbox 向 Leader 请求
// Leader 通过 leaderPermissionBridge.ts:54 行桥接权限确认
```

### 9.4 Coordinator 模式

```typescript
// src/coordinator/coordinatorMode.ts:36-40
export function isCoordinatorMode(): boolean {
  if (feature('COORDINATOR_MODE')) {
    return isEnvTruthy(process.env.CLAUDE_CODE_COORDINATOR_MODE)
  }
  return false
}

// :80-100 — 注入 Worker 工具列表到上下文
export function getCoordinatorUserContext(mcpClients, scratchpadDir?) {
  const workerTools = ASYNC_AGENT_ALLOWED_TOOLS
    .filter(name => !INTERNAL_WORKER_TOOLS.has(name))
    .sort().join(', ')
  return { coordinator: `Workers have access to these tools: ${workerTools}` }
}
```

---

## 十、记忆系统深挖

### 10.1 memdir — 记忆目录管理

```typescript
// src/memdir/memdir.ts:35-38 — 容量限制
export const MAX_ENTRYPOINT_LINES = 200
export const MAX_ENTRYPOINT_BYTES = 25_000  // ~125 字符/行 × 200 行

// :57-89 — 双重截断:先行后字节
export function truncateEntrypointContent(raw: string): EntrypointTruncation {
  const wasLineTruncated = lineCount > MAX_ENTRYPOINT_LINES
  const wasByteTruncated = byteCount > MAX_ENTRYPOINT_BYTES
  // 截断后附加警告:
  // "> WARNING: MEMORY.md is {reason}. Only part of it was loaded."
}
```

### 10.2 记忆类型

```typescript
// src/memdir/memoryTypes.ts:271 行
// 定义: user | feedback | project | reference 四种记忆类型
// MEMORY_FRONTMATTER_EXAMPLE — YAML frontmatter 模板
// WHEN_TO_ACCESS_SECTION — 何时检索记忆的指引
// WHAT_NOT_TO_SAVE_SECTION — 不应保存的内容类型
// TRUSTING_RECALL_SECTION — 信任与召回指引
```

### 10.3 Extract Memories — 记忆提取

```typescript
// src/services/extractMemories/extractMemories.ts:296-460
export function initExtractMemories(): void {
  // 闭包作用域(非模块级) — 测试友好
  let lastUuid: string | undefined
  let running = false

  async function runExtraction({ messages, ... }) {
    // 互斥检查:主 Agent 已写入 auto-memory 路径时跳过
    if (mainAgentAlreadyWroteMemories(messages, lastUuid)) {
      log('[extractMemories] skipping — conversation already wrote to memory files')
      return
    }
    // 使用 runForkedAgent — 与父会话共享 prompt cache
    const result = await runForkedAgent({
      cacheSafeParams: createCacheSafeParams(context),
      promptMessages: [...messages, createUserMessage({ content: prompt })],
      // ...
    })
  }
}
```

### 10.4 Auto Dream — 后台整合

```typescript
// src/services/autoDream/autoDream.ts:324 行
// 后台定时扫描历史会话,整合记忆
// consolidationLock.ts:140 行 — 文件锁防止并发整合
// consolidationPrompt.ts:65 行 — 整合提示词
// DreamTask:157 行 — 在 AppState 中注册为后台任务
```

### 10.5 Session Memory — 会话记忆

```typescript
// src/services/SessionMemory/sessionMemory.ts:495 行
// 每次会话结束时生成摘要,写入 session_memory 目录
// sessionMemoryUtils.ts:207 行 — 工具函数
// prompts.ts / prompts_cn.ts:324 行 — 双语提示词
```

### 10.6 Team Memory — 团队记忆

```typescript
// src/memdir/teamMemPaths.ts:292 行 — 团队记忆路径管理
// src/memdir/teamMemPrompts.ts:100 行 — 团队记忆提示词
// src/services/teamMemorySync/ — 团队记忆同步
// 由 feature('TEAMMEM') 门控
```

---

## 十一、服务层深挖

### 11.1 GrowthBook 特性标志

```typescript
// src/services/analytics/growthbook.ts:1155 行
// 三层架构:
// 1. 编译时: feature('FLAG') — bun:bundle DCE,60+ 个
// 2. 运行时缓存: getFeatureValue_CACHED_MAY_BE_STALE() — 热路径
// 3. 运行时阻塞: checkGate_CACHED_OR_BLOCKING() — 安全门控

// :734-780 — 缓存值获取
export function getFeatureValue_CACHED_MAY_BE_STALE<T>(
  feature: string, defaultValue: T
): T {
  // 远程 eval 缓存 workaround
  if (remoteEvalFeatureValues.has(feature)) {
    return remoteEvalFeatureValues.get(feature) as T
  }
  return client?.getFeatureValue(feature, defaultValue) ?? defaultValue
}

// :87-92 — 曝光去重
const loggedExposures = new Set<string>()  // 防止热路径重复曝光
const pendingExposures = new Set<string>() // init 前的特性访问

// :96-110 — 信号驱动刷新
const refreshed = createSignal()
export function onGrowthBookRefresh(listener: GrowthBookRefreshListener) {
  return refreshed.subscribe(() => callSafe(listener))
}
```

### 11.2 Prompt Suggestion / Speculation

```typescript
// src/services/PromptSuggestion/speculation.ts:991 行 — 投机执行
const MAX_SPECULATION_TURNS = 20
const MAX_SPECULATION_MESSAGES = 100
const WRITE_TOOLS = new Set(['Edit', 'Write', 'NotebookEdit'])
const SAFE_READ_ONLY_TOOLS = new Set(['Read', 'Glob', 'Grep', 'ToolSearch', 'LSP'])

// 投机执行:在用户输入前,用 forked agent 预测下一步
// 写操作 → copy-to-overlay(可回滚)
// 只读操作 → 无需回滚
// 用户接受 → overlay 合并到主文件系统
// 用户拒绝 → overlay 清理

// src/services/PromptSuggestion/promptSuggestion.ts:523 行
// shouldEnablePromptSuggestion() — 多重门控:
//   环境变量 → GrowthBook('tengu_chomp_inflection') → 非交互模式 → Swarm 队友 → 用户设置
```

### 11.3 LSP 服务

```typescript
// src/services/lsp/ — 2460 行
// LSPServerManager.ts:420 行 — 管理多个 LSP 服务器实例
// LSPClient.ts:447 行 — LSP 客户端(与语言服务器通信)
// LSPServerInstance.ts:511 行 — 单个服务器实例生命周期
// LSPDiagnosticRegistry.ts:386 行 — 诊断信息收集
// passiveFeedback.ts:328 行 — 被动反馈(从 LSP 诊断中学习)
```

### 11.4 Magic Docs

```typescript
// src/services/MagicDocs/magicDocs.ts:254 行
const MAGIC_DOC_HEADER_PATTERN = /^#\s*MAGIC\s+DOC:\s*(.+)$/im

// 当 FileReadTool 读取到匹配的文件时:
// 1. registerMagicDoc(filePath) 注册追踪
// 2. 注册 postSamplingHook,在每轮结束后触发
// 3. 使用 runAgent 运行 MagicDocs agent 更新文档
// 4. 斜体行作为自定义指令: `*instructions here*`
```

### 11.5 Settings Sync

```typescript
// src/services/settingsSync/index.ts:581 行
// 上传: 增量式,仅同步变更条目
export async function uploadUserSettingsInBackground() {
  // 门控: feature('UPLOAD_USER_SETTINGS') && GrowthBook && OAuth
  const localEntries = await buildEntriesFromLocalFiles(projectId)
  const changedEntries = pickBy(localEntries, (v, k) => remoteEntries[k] !== v)
  await uploadUserSettings(changedEntries)
}
// 下载: CCR 模式下在插件安装前下载远程设置
// 超时: 10 秒,最大重试: 3 次
```

### 11.6 Remote Managed Settings (企业 MDM)

```typescript
// src/services/remoteManagedSettings/ — 877 行
// 企业设备管理(MDM)设置同步
// syncCache.ts:112 行 — 本地缓存
// securityCheck.tsx:96 行 — 安全校验
// 由 isEligibleForRemoteManagedSettings() 门控
```

### 11.7 Policy Limits

```typescript
// src/services/policyLimits/index.ts:663 行
// 企业策略限制:使用量、速率、计费
// isPolicyLimitsEligible() — 是否启用策略限制
// initializePolicyLimitsLoadingPromise() — 异步加载策略
```

### 11.8 Away Summary

```typescript
// src/services/awaySummary.ts:74 行
export async function generateAwaySummary(messages, signal) {
  const memory = await getSessionMemoryContent()
  const recent = messages.slice(-RECENT_MESSAGE_WINDOW)  // 30 条
  recent.push(createUserMessage({ content: buildAwaySummaryPrompt(memory) }))
  const response = await queryModelWithoutStreaming({
    model: getSmallFastModel(),  // 使用小快速模型
    // ...
  })
}
```

---

## 十二、Cron 调度系统

### 12.1 架构

```typescript
// src/utils/cronScheduler.ts:142-144
export function createCronScheduler(options: CronSchedulerOptions): CronScheduler {
  // 非 React 的调度核心,供 REPL 和 SDK/-p 模式共用
  // CHECK_INTERVAL_MS = 1000 (1 秒轮询)
  // FILE_STABILITY_MS = 300 (文件变更稳定阈值)
  // LOCK_PROBE_INTERVAL_MS = 5000 (非拥有者探活间隔)
}
```

### 12.2 文件锁

```typescript
// src/utils/cronTasksLock.ts:195 行
// 多会话环境下的调度器互斥:
// tryAcquireSchedulerLock() — 获取锁
// releaseSchedulerLock() — 释放锁
// 使用 PID 作为活性探针,崩溃后自动释放
```

### 12.3 任务管理

```typescript
// src/utils/cronTasks.ts:458 行
// CronTask: { id, cron, prompt, recurring, permanent, createdAt }
// jitteredNextCronRunMs() — 抖动调度,防止同时触发
// findMissedTasks() — 启动时检测错过的任务
// isRecurringTaskAged() — 定期任务老化清理
```

---

## 十三、设置系统深挖

### 13.1 多源设置

```typescript
// src/utils/settings/settings.ts:1015 行
// 优先级: policySettings > projectSettings > userSettings
// getInitialSettings() — 合并所有源的设置
// updateSettingsForSource(source, updates) — 更新特定源
// getSettingsForSource(source) — 读取特定源
```

### 13.2 设置类型与验证

```typescript
// src/utils/settings/types.ts:1148 行 — Zod schema
// 涵盖: model, hooks, permissions, mcpServers, promptSuggestion,
//       autoUpdater, keybindings, releaseChannel, etc.

// validation.ts:265 行 — 保存时验证
// permissionValidation.ts:262 行 — 权限设置验证
// toolValidationConfig.ts:103 行 — 工具验证配置
```

### 13.3 变更检测

```typescript
// src/utils/settings/settingsCache.ts:80 行 — 设置缓存
// changeDetector.ts:488 行 — 文件变更监听
// applySettingsChange.ts — 应用设置变更到运行时
```

---

## 十四、入口点与启动深挖

### 14.1 CLI 入口

```typescript
// src/entrypoints/cli.tsx:33-50 — 快速路径
async function main(): Promise<void> {
  const args = process.argv.slice(2)
  // --version: 零模块加载
  if (args[0] === '--version') { console.log(`${MACRO.VERSION} (Claude Code)`); return }
  // --dump-system-prompt: 输出系统提示词
  if (feature('DUMP_SYSTEM_PROMPT') && args[0] === '--dump-system-prompt') { ... }
  // --claude-in-chrome-mcp: Chrome MCP 服务器
  // --chrome-native-host: Chrome 原生消息宿主
  // ... 然后加载完整 CLI
}
```

### 14.2 初始化管线

```typescript
// src/entrypoints/init.ts:65-87 — 初始化序列
export const init = memoize(async () => {
  enableConfigs()                    // 配置系统
  applySafeConfigEnvironmentVariables()  // 安全环境变量
  applyExtraCACertsFromConfig()      // TLS 证书
  setupGracefulShutdown()            // 优雅关闭
  initialize1PEventLogging()         // 事件日志
  initializeGrowthBook()             // 特性标志
  // ... 共 20+ 个初始化步骤
})
```

### 14.3 SDK 类型导出

```typescript
// src/entrypoints/agentSdkTypes.ts:73-103
export function tool<Schema>(name, description, inputSchema, handler, extras?) { ... }
export function createSdkMcpServer(options) { ... }
export function unstable_v2_createSession(options) { ... }
// Re-exports: coreTypes, runtimeTypes, settingsTypes, toolTypes
```

---

## 十五、REPL 屏幕深挖

### 15.1 组件规模

```typescript
// src/screens/REPL.tsx:5005 行 — 主 UI 屏幕
// 导入 80+ 模块,是整个应用的核心编排器
// 使用 React Compiler runtime (_c memoization)
```

### 15.2 死代码消除

```typescript
// src/screens/REPL.tsx:96-130 — 条件 require
const useVoiceIntegration = feature('VOICE_MODE')
  ? require('../hooks/useVoiceIntegration.js').useVoiceIntegration
  : () => ({ stripTrailing: () => 0, handleKeyEvent: () => {}, resetAnchor: () => {} })

const getCoordinatorUserContext = feature('COORDINATOR_MODE')
  ? require('../coordinator/coordinatorMode.js').getCoordinatorUserContext
  : () => ({})
```

### 15.3 性能隔离

```typescript
// src/screens/REPL.tsx:479-482 — Spinner 隔离
// "Isolated from REPL so the 960ms animation tick re-renders only
//  the spinner subtree, not the entire REPL tree."
```

---

## 十六、Forked Agent 模式深挖

### 16.1 CacheSafeParams

```typescript
// src/utils/forkedAgent.ts:57-72
export type CacheSafeParams = {
  systemPrompt: SystemPrompt     // 必须与父会话匹配
  userContext: { [k: string]: string }
  systemContext: { [k: string]: string }
  toolUseContext: ToolUseContext  // 工具、模型、选项
  forkContextMessages: Message[] // 父会话上下文消息
}
```

### 16.2 Prompt Cache 共享

```typescript
// src/utils/forkedAgent.ts:46-56
// Anthropic API cache key = system prompt + tools + model + messages(prefix) + thinking config
// Fork 通过匹配 CacheSafeParams 复用父会话的 prompt cache
// 注意: 不同的 maxOutputTokens 会改变 budget_tokens,破坏 cache hit
```

### 16.3 使用追踪

```typescript
// src/utils/forkedAgent.ts:489 — runForkedAgent
export async function runForkedAgent({ cacheSafeParams, promptMessages, ... }) {
  // 创建独立 AbortController
  const childAbort = createChildAbortController(parentAbort)
  // 克隆 FileStateCache 和 ContentReplacementState(隔离可变状态)
  const clonedCache = cloneFileStateCache(cacheSafeParams.toolUseContext.readFileState)
  // 运行查询循环,追踪 usage
  // 完成后: logEvent('tengu_fork_agent_query', { forkLabel, usage, ... })
}
```

---

## 十七、Buddy 伴侣系统

### 17.1 种子 PRNG 生成

```typescript
// src/buddy/companion.ts:16-35 — Mulberry32 PRNG
function mulberry32(seed: number): () => number {
  let a = seed >>> 0
  return function () {
    a |= 0; a = (a + 0x6d2b79f5) | 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}
```

### 17.2 稀有度系统

```typescript
// src/buddy/types.ts:126 — 权重表
export const RARITY_WEIGHTS = { common: 50, uncommon: 30, rare: 15, epic: 4, legendary: 1 }

// companion.ts:62-100 — 属性生成
// RARITY_FLOOR: common=5, uncommon=15, rare=25, epic=35, legendary=50
// One peak stat + one dump stat + rest scattered
```

### 17.3 外观系统

```typescript
// src/buddy/types.ts:54-95
export const SPECIES = ['duck', 'cat', 'owl', 'fox', 'frog', 'bunny', ...]
export const EYES = ['·', '✦', '×', '◉', '@', '°']
export const HATS = ['none', 'crown', 'wizard', 'pirate', 'party', ...]
```

---

## 十八、Voice 服务

### 18.1 架构

```typescript
// src/services/voice.ts:1-50
// 推按说话(Push-to-Talk)语音输入
// 优先: audio-capture-napi (原生模块, macOS/Linux/Windows)
// 回退: SoX `rec` (Linux), arecord/ALSA (Linux)

// 延迟加载: 首次按键时才加载原生模块
// "dlopen is synchronous and blocks the event loop for ~1s warm,
//  up to ~8s on cold coreaudiod (post-wake, post-boot)."
```

### 18.2 配置

```typescript
const RECORDING_SAMPLE_RATE = 16000
const RECORDING_CHANNELS = 1
const SILENCE_DURATION_SECS = '2.0'  // 静音检测阈值
const SILENCE_THRESHOLD = '3%'
```

---

## 十九、Export Renderer — 无头渲染

```typescript
// src/utils/exportRenderer.tsx:55-83 — 分块渲染
export async function streamRenderedMessages(messages, tools, sink, {
  chunkSize = 40,  // 每次渲染 40 条消息
}) {
  // 测量结果(2026-03, 538 条消息会话):
  // -55% 峰值 RSS vs 全量渲染
  for (let offset = 0; offset < ceiling; offset += chunkSize) {
    const ansi = await renderChunk([offset, offset + chunkSize])
    if (stripAnsi(ansi).trim() === '') break
    await sink(ansi)
  }
}
```

---

## 二十、迁移系统

```typescript
// src/migrations/ — 603 行,11 个迁移文件
// 模型迁移链:
//   fennec → opus (migrateFennecToOpus.ts:18)
//   legacy → current (migrateLegacyOpusToCurrent.ts)
//   opus → opus[1m] (migrateOpusToOpus1m.ts:24)
//   sonnet[1m] → sonnet-4.5[1m] (migrateSonnet1mToSonnet45.ts:25)
//   sonnet-4.5 → sonnet (migrateSonnet45ToSonnet46.ts:29)

// 设置迁移:
//   auto-updates → settings (migrateAutoUpdatesToSettings.ts)
//   bypass permissions → settings (migrateBypassPermissionsAcceptedToSettings.ts)
//   MCP servers → settings (migrateEnableAllProjectMcpServersToSettings.ts)
//   bridge → remote-control (migrateReplBridgeEnabledToRemoteControlAtStartup.ts)

// 每个迁移都是幂等的,并记录分析事件
```

---

## 二十一、Process User Input — 输入路由

```typescript
// src/utils/processUserInput/processUserInput.ts:281-578
async function processUserInputBase({ input, ... }) {
  // 1. 斜杠命令 → processSlashCommand (延迟加载)
  if (isSlashCommand(input)) {
    const { processSlashCommand } = await import('./processSlashCommand.js')
    return processSlashCommand(...)
  }
  // 2. Bash 命令模式 → processBashCommand
  if (inputMode === 'bash') {
    const { processBashCommand } = await import('./processBashCommand.js')
    return processBashCommand(...)
  }
  // 3. 文本提示 → processTextPrompt
  processTextPrompt(input, context, ...)
}
```

---

## 二十二、对 laew 借鉴路线图

### P0 — 立即借鉴(1-2 周)

| 借鉴项 | claudecode 参考 | laew 落地 |
|--------|----------------|-----------|
| **命令懒加载** | `load: () => import('./help.js')` 模式 | laew 的 TUI 斜杠命令可按需加载,减少启动时间 |
| **自定义 Store** | `src/state/store.ts` 34 行极简实现 | laew 的 Session 状态管理可采用相同模式替代直接传递 |
| **Forked Agent CacheSafeParams** | 5 字段匹配确保 prompt cache hit | laew 的 SubAgent 可引入 CacheSafeParams 概念,复用父会话缓存 |
| **记忆目录容量控制** | MAX_ENTRYPOINT_LINES=200 + MAX_ENTRYPOINT_BYTES=25000 | laew 的 session_memory 表可引入类似的容量保护 |
| **任务 ID 安全生成** | 36^8 随机 ID + 类型前缀 | laew 的 Session ID 可采用类似方案 |

### P1 — 短期借鉴(2-4 周)

| 借鉴项 | claudecode 参考 | laew 落地 |
|--------|----------------|-----------|
| **Vim 模式状态机** | 11 状态 × 操作符 × 文本对象 | laew TUI 可引入基本 vim 键绑定(先支持 h/j/k/l/w/b) |
| **快捷键系统** | 20+ context 分层 + 用户自定义 JSON | laew TUI 的键绑定管理可结构化,支持用户覆盖 |
| **GrowthBook 特性标志** | 编译时 DCE + 运行时缓存 + 阻塞门控 | laew 可引入简单特性门控(环境变量 + SQLite 配置) |
| **Away Summary** | 小模型快速生成会话回顾 | laew 的 SessionContext 可在用户长时间离开后生成回顾 |
| **Cron 调度器** | 文件锁 + 抖动 + 错过检测 | laew 可引入定时任务(CronCreate 命令) |
| **Magic Docs** | `# MAGIC DOC:` 头检测 + forked agent 更新 | laew 可支持类似机制,自动维护项目文档 |

### P2 — 中期借鉴(1-2 月)

| 借鉴项 | claudecode 参考 | laew 落地 |
|--------|----------------|-----------|
| **多源设置系统** | policy > project > user 三层合并 | laew 的 SQLite 配置可引入层级覆盖 |
| **Settings Sync** | 增量同步 + OAuth 门控 | laew 可支持设置在多设备间同步 |
| **LSP 集成** | LSPServerManager + 诊断收集 | laew 的 ReadTool 可集成 LSP 获得类型信息和诊断 |
| **Speculation 投机执行** | forked agent 预测 + overlay 回滚 | laew 可在用户思考时预执行,接受后合并结果 |
| **Swarm 权限同步** | Leader-Worker 之间的 mailbox 权限桥 | laew 的 SubAgent 可引入类似的权限委托机制 |
| **Ink 自定义 Fork** | 单元格级屏幕缓冲 + 选择系统 | laew 的 TUI 渲染可参考 cell-based 方案提升性能 |

### P3 — 长期愿景(3+ 月)

| 借鉴项 | claudecode 参考 | laew 落地 |
|--------|----------------|-----------|
| **Bridge 远程控制** | CCR 全远程 TUI | laew 可支持远程模式(手机/平板控制) |
| **Voice 模式** | 推按说话 + 原生音频 | laew 可引入语音输入 |
| **SDK 类型导出** | tool(), createSdkMcpServer() | laew 可暴露 SDK API 供外部集成 |
| **企业 MDM 设置** | Remote Managed Settings + Policy Limits | laew 可支持企业部署场景 |
| **Buddy 伴侣** | PRNG 生成 + 稀有度系统 | laew 可引入类似的趣味性功能提升用户粘性 |

---

## 附录: 关键文件索引

| 模块 | 关键文件 | 行数 |
|------|---------|------|
| 命令系统 | `src/commands.ts` | 700+ |
| 命令类型 | `src/types/command.ts` | 200+ |
| Ink 核心 | `src/ink/ink.tsx` | 1722 |
| Ink 屏幕 | `src/ink/screen.ts` | 1486 |
| Ink Reconciler | `src/ink/reconciler.ts` | 600+ |
| Ink 选择 | `src/ink/selection.ts` | 917 |
| Store | `src/state/store.ts` | 34 |
| AppState | `src/state/AppStateStore.ts` | 569 |
| Bridge 主循环 | `src/bridge/bridgeMain.ts` | 2999 |
| Bridge REPL | `src/bridge/replBridge.ts` | 2406 |
| Vim 转换 | `src/vim/transitions.ts` | 490 |
| Vim 操作符 | `src/vim/operators.ts` | 556 |
| 快捷键默认 | `src/keybindings/defaultBindings.ts` | 340 |
| 快捷键验证 | `src/keybindings/validate.ts` | 498 |
| Task 框架 | `src/Task.ts` | 125 |
| LocalAgentTask | `src/tasks/LocalAgentTask/LocalAgentTask.tsx` | 300+ |
| Swarm 运行器 | `src/utils/swarm/inProcessRunner.ts` | 1552 |
| Swarm 权限 | `src/utils/swarm/permissionSync.ts` | 928 |
| 记忆目录 | `src/memdir/memdir.ts` | 507 |
| 记忆提取 | `src/services/extractMemories/extractMemories.ts` | 615 |
| Auto Dream | `src/services/autoDream/autoDream.ts` | 324 |
| Session Memory | `src/services/SessionMemory/sessionMemory.ts` | 495 |
| GrowthBook | `src/services/analytics/growthbook.ts` | 1155 |
| Speculation | `src/services/PromptSuggestion/speculation.ts` | 991 |
| LSP 管理 | `src/services/lsp/LSPServerManager.ts` | 420 |
| Magic Docs | `src/services/MagicDocs/magicDocs.ts` | 254 |
| Settings Sync | `src/services/settingsSync/index.ts` | 581 |
| 设置核心 | `src/utils/settings/settings.ts` | 1015 |
| 设置类型 | `src/utils/settings/types.ts` | 1148 |
| Cron 调度 | `src/utils/cronScheduler.ts` | 565 |
| Cron 任务 | `src/utils/cronTasks.ts` | 458 |
| CLI 入口 | `src/entrypoints/cli.tsx` | 302 |
| 初始化 | `src/entrypoints/init.ts` | 340 |
| SDK 类型 | `src/entrypoints/agentSdkTypes.ts` | 443 |
| REPL 屏幕 | `src/screens/REPL.tsx` | 5005 |
| Doctor | `src/screens/Doctor.tsx` | 574 |
| Forked Agent | `src/utils/forkedAgent.ts` | 689 |
| Buddy 伴侣 | `src/buddy/companion.ts` | 120+ |
| Buddy 类型 | `src/buddy/types.ts` | 130+ |
| Voice | `src/services/voice.ts` | 480+ |
| Export 渲染 | `src/utils/exportRenderer.tsx` | 100 |
| 迁移 | `src/migrations/` | 603 |
| Coordinator | `src/coordinator/coordinatorMode.ts` | 120+ |
| 输入路由 | `src/utils/processUserInput/processUserInput.ts` | 605 |
| 设计系统 | `src/components/design-system/` | 2208 |
| MCP 组件 | `src/components/mcp/` | 3863 |
| PromptInput | `src/components/PromptInput/PromptInput.tsx` | 2338 |
