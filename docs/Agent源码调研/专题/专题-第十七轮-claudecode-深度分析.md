# 专题-第十七轮-claudecode-深度分析
> 调研日期：2026-09-09
> 调研范围：claudecode 崩溃恢复与取证、多租户隔离、RRF检索、LLM网关路由、Pregel图执行、Skill生命周期、Agent预热池、Turn锁、HTTP客户端高级实现、安全加固
> 本轮新增 gap：L1166-L1223

---

## 一、崩溃恢复与取证（CrashDump & Forensics）

### 1.1 多层防御架构

claude-code 的崩溃恢复采用**五层防御 + 事后取证**架构，远超第 22 章已记录的深度：

```
┌─────────────────────────────────────────────────────────────────┐
│                 崩溃恢复与取证五层架构                              │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Layer 1: 进程级信号处理                                         │
│  - SIGINT/SIGTERM/SIGHUP → gracefulShutdown                     │
│  - SIGCONT → resumeHandler (终端暂停/恢复)                       │
│  - SIGUSR2 → sigusr2Handler (Bridge 模式)                        │
│  - uncaughtException/unhandledRejection → 记录后 crash           │
│                                                                 │
│  Layer 2: 优雅关闭协调器 (gracefulShutdown.ts, 529 行)           │
│  - failsafe 定时器 + SIGKILL 兜底                                │
│  - SessionEnd Hook 预算缩放                                      │
│  - 终端模式清理 (cleanupTerminalModes)                           │
│  - 会话数据刷写优先于 Hook/Analytics                              │
│                                                                 │
│  Layer 3: Heap Dump 服务 (heapDumpService.ts)                    │
│  - V8 heap snapshot + 诊断 JSON                                  │
│  - 自动 1.5GB 触发器                                            │
│  - 内存泄漏检测 (detached contexts, active handles)              │
│  - Linux smaps_rollup 详细内存分解                               │
│                                                                 │
│  Layer 4: 会话恢复机制                                           │
│  - conversationRecovery: "Continue from where you left off."     │
│  - Bridge Pointer: 4h TTL 崩溃恢复指针                           │
│  - ResumeConversation 屏: 交互式会话选择                          │
│  - sessionRestore.ts: 文件历史/attribution/todos 恢复            │
│                                                                 │
│  Layer 5: 清理注册表 (cleanupRegistry.ts)                        │
│  - 全局 Set<cleanupFn> 注册                                      │
│  - Promise.all 并发执行                                          │
│  - 2s 超时保护                                                   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 1.2 gracefulShutdown 关键设计

**文件**: `src/utils/gracefulShutdown.ts` (529 行)

核心流程：
```typescript
export async function gracefulShutdown(
  exitCode = 0,
  reason: ExitReason = 'other',
  options?: { getAppState?, setAppState?, finalMessage? }
): Promise<void> {
  // 1. 幂等检查
  if (shutdownInProgress) return
  shutdownInProgress = true

  // 2. Failsafe 定时器 — 保证进程一定退出
  //    预算 = max(5s, SessionEnd Hook 预算 + 3.5s)
  failsafeTimer = setTimeout(
    code => { cleanupTerminalModes(); printResumeHint(); forceExit(code) },
    Math.max(5000, sessionEndTimeoutMs + 3500),
    exitCode,
  )

  // 3. 终端模式清理（同步，确保在进程退出前完成）
  cleanupTerminalModes()
  printResumeHint()

  // 4. 会话数据刷写（最高优先级，2s 超时）
  await Promise.race([runCleanupFunctions(), timeout(2000)])

  // 5. SessionEnd Hook（带 AbortSignal.timeout）
  await executeSessionEndHooks(reason, { signal: AbortSignal.timeout(sessionEndTimeoutMs) })

  // 6. 性能分析报告
  profileReport()

  // 7. 缓存驱逐提示
  logEvent('tengu_cache_eviction_hint', { last_request_id })

  // 8. Analytics 刷写（500ms 上限）
  await Promise.race([Promise.all([shutdown1PEventLogging(), shutdownDatadog()]), sleep(500)])

  // 9. 强制退出
  forceExit(exitCode)
}
```

**关键设计点**：
- **Failsafe 定时器**：防止 MCP 连接等清理操作挂死进程
- **终端模式清理**：无条件发送所有 disable 序列（兼容 tmux/screen）
- **会话数据优先**：终端 dead (SIGHUP) 时 Hook/Analytics 可能 hang，先刷写会话数据
- **Analytics 500ms 上限**：避免 1P exporter 的 10s axios POST 吃光 failsafe 预算

### 1.3 Heap Dump 服务

**文件**: `src/utils/heapDumpService.ts`

```typescript
export async function performHeapDump(
  trigger: 'manual' | 'auto-1.5GB' = 'manual',
  dumpNumber = 0,
): Promise<HeapDumpResult> {
  // 1. 先捕获诊断（heap dump 本身会分配内存，会扭曲数据）
  const diagnostics = await captureMemoryDiagnostics(trigger, dumpNumber)

  // 2. 诊断写入 JSON（便宜，不太可能失败）
  await writeFile(diagPath, jsonStringify(diagnostics, null, 2), { mode: 0o600 })

  // 3. Heap snapshot（大堆可能 crash）
  await writeHeapSnapshot(heapPath)

  // 4. Bun 特殊处理：同步 I/O 避免跨线程克隆
  if (typeof Bun !== 'undefined') {
    writeFileSync(filepath, Bun.generateHeapSnapshot('v8', 'arraybuffer'), { mode: 0o600 })
    Bun.gc(true) // 强制 GC 释放 heap snapshot
  }
}
```

**MemoryDiagnostics 包含**：
- `memoryUsage`: heapUsed/heapTotal/external/arrayBuffers/rss
- `memoryGrowthRate`: bytesPerSecond, mbPerHour
- `v8HeapStats`: heapSizeLimit, mallocedMemory, detachedContexts, nativeContexts
- `v8HeapSpaces`: 每个堆空间的 size/used/available
- `activeHandles/activeRequests`: 泄漏检测
- `openFileDescriptors`: Linux /proc/self/fd 计数
- `smapsRollup`: Linux 详细内存分解
- `analysis.potentialLeaks`: 自动泄漏检测（detached contexts, active handles, native memory > heap, 高增长率, 多 FD）

### 1.4 会话恢复机制

**文件**: `src/utils/conversationRecovery.ts` + `src/utils/sessionRestore.ts`

恢复流程：
1. **conversationRecovery.ts**: 反序列化日志 → 过滤孤立 tool_use → 过滤 thinking-only 消息 → 追加 "Continue from where you left off."
2. **sessionRestore.ts**: 恢复 fileHistory, attribution, contextCollapse, todos
3. **ResumeConversation.tsx**: 交互式会话选择屏（399 行 React 组件）

**关键类型**：
```typescript
export type TurnInterruptionState =
  | { kind: 'none' }
  | { kind: 'interrupted_prompt'; message: NormalizedUserMessage }

export type DeserializeResult = {
  messages: Message[]
  turnInterruptionState: TurnInterruptionState
}
```

### 1.5 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1166** | CrashDump | 无自动内存泄漏检测（1.5GB 自动 dump 存在但未暴露自动触发） | P1 |
| **L1167** | CrashDump | 无错误指纹去重（每次崩溃独立记录，无 SHA256 fingerprint） | P1 |
| **L1168** | CrashDump | 无崩溃后自动重启（仅记录，无 restart loop） | P1 |
| **L1169** | CrashDump | 无 core dump（仅 V8 heap snapshot，无操作系统级 core） | P2 |
| **L1170** | CrashDump | 无 Watchdog 进程（无独立监控进程检测 hang） | P2 |
| **L1171** | 会话恢复 | 无增量检查点（全量日志重放，无 WAL 模式） | P0 |
| **L1172** | 会话恢复 | 无崩溃一致性保证（fsync 缺失，断电可能丢数据） | P0 |

---

## 二、多租户隔离（Multi-Tenant Isolation）

### 2.1 隔离层次

claude-code 的多租户隔离主要体现在**沙箱 + 会话 + 文件系统**三个层次：

```
┌─────────────────────────────────────────────────────────────────┐
│                    多租户隔离架构                                 │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  层次 1: 沙箱隔离 (@anthropic-ai/sandbox-runtime)                │
│  - 文件系统: allowWrite/denyWrite/allowRead/denyRead            │
│  - 网络: allowedDomains/deniedDomains                           │
│  - 进程: 资源限制                                                │
│                                                                 │
│  层次 2: 会话隔离                                                │
│  - 独立 Session ID + 对话上下文                                  │
│  - 独立 PID 文件 (concurrentSessions.ts)                         │
│  - 独立 Worktree (EnterWorktreeTool)                             │
│                                                                 │
│  层次 3: 文件系统隔离                                            │
│  - 项目目录: .claude/ 配置隔离                                   │
│  - 内存目录: memdir 路径隔离                                     │
│  - 团队记忆: teamMemPaths 隔离                                   │
│                                                                 │
│  层次 4: 进程隔离 (upstreamproxy)                                 │
│  - CCR 容器级隔离                                                │
│  - prctl(PR_SET_DUMPABLE, 0) 防 ptrace                          │
│  - 独立 CA 证书链                                                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 2.2 沙箱适配器

**文件**: `src/utils/sandbox/sandbox-adapter.ts`

```typescript
export function convertToSandboxRuntimeConfig(settings: SettingsJson): SandboxRuntimeConfig {
  // 网络域隔离
  const allowedDomains: string[] = []
  const deniedDomains: string[] = []

  // 受管沙箱域策略（企业版）
  if (shouldAllowManagedSandboxDomainsOnly()) {
    // 仅使用 policySettings 中的域
    const policySettings = getSettingsForSource('policySettings')
    for (const domain of policySettings?.sandbox?.network?.allowedDomains || []) {
      allowedDomains.push(domain)
    }
  }

  // 文件系统路径隔离
  // CC 特有路径模式: //path → 绝对路径, /path → 相对于 settings 文件目录
  export function resolvePathPatternForSandbox(pattern: string, source: SettingSource): string {
    if (pattern.startsWith('//')) return pattern.slice(1)
    if (pattern.startsWith('/') && !pattern.startsWith('//')) {
      const root = getSettingsRootPathForSource(source)
      return resolve(root, pattern.slice(1))
    }
    return pattern
  }
}
```

### 2.3 并发会话管理

**文件**: `src/utils/concurrentSessions.ts`

```typescript
export type SessionKind = 'interactive' | 'bg' | 'daemon' | 'daemon-worker'
export type SessionStatus = 'busy' | 'idle' | 'waiting'

export async function registerSession(): Promise<boolean> {
  // 跳过 teammates/subagents（避免污染 ps 输出）
  if (getAgentId() != null) return false

  const kind: SessionKind = envSessionKind() ?? 'interactive'
  const pidFile = join(dir, `${process.pid}.json`)

  // 注册清理函数（退出时删除 PID 文件）
  registerCleanup(async () => { await unlink(pidFile) })

  await writeFile(pidFile, jsonStringify({
    pid: process.pid,
    sessionId: getSessionId(),
    cwd: getOriginalCwd(),
    startedAt: Date.now(),
    kind,
    entrypoint: process.env.CLAUDE_CODE_ENTRYPOINT,
    messagingSocketPath: process.env.CLAUDE_CODE_MESSAGING_SOCKET,
  }))
}
```

### 2.4 Agent Context 隔离

**文件**: `src/utils/agentContext.ts`

使用 `AsyncLocalStorage` 实现异步上下文隔离：

```typescript
const agentContextStorage = new AsyncLocalStorage<AgentContext>()

export function getAgentContext(): AgentContext | undefined {
  return agentContextStorage.getStore()
}

// 两种上下文类型：
export type AgentContext = SubagentContext | TeammateAgentContext
```

**为什么用 AsyncLocalStorage 而不是 AppState**：
- 多个 Agent 可在同一进程并发运行（Ctrl+B 后台化）
- AppState 是单一共享状态，会被覆盖
- AsyncLocalStorage 隔离每个异步执行链

### 2.5 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1173** | 多租户 | 无命名空间隔离（所有会话共享进程空间） | P1 |
| **L1174** | 多租户 | 无资源配额（CPU/内存/IO 无 per-session 限制） | P1 |
| **L1175** | 多租户 | 无网络策略 per-session（全局代理设置） | P2 |
| **L1176** | 多租户 | 无租户级审计日志（仅全局 analytics） | P2 |
| **L1177** | 多租户 | 无跨租户数据泄露防护（共享 SQLite 连接） | P1 |

---

## 三、RRF检索（混合检索与排序融合）

### 3.1 记忆检索架构

claude-code 的记忆检索采用**LLM 选择器 + 侧查询**模式，而非传统 RRF：

```
┌─────────────────────────────────────────────────────────────────┐
│                    记忆检索管线                                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. 扫描阶段 (memoryScan.ts)                                     │
│     - readdir(memoryDir, { recursive: true })                    │
│     - 过滤 .md 文件（排除 MEMORY.md）                             │
│     - 读取 frontmatter（前 30 行）                                │
│     - 排序: mtime 降序，上限 200 文件                             │
│                                                                 │
│  2. 选择阶段 (findRelevantMemories.ts)                           │
│     - 格式化 manifest: [type] filename (timestamp): description  │
│     - 调用 Sonnet sideQuery 选择 ≤5 个相关记忆                    │
│     - JSON Schema 输出: { selected_memories: string[] }          │
│     - 过滤已展示过的文件（alreadySurfaced）                       │
│                                                                 │
│  3. 注入阶段                                                     │
│     - 读取选中记忆文件内容                                        │
│     - 包装为 AttachmentMessage                                   │
│     - 注入到对话上下文                                            │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 关键实现

**文件**: `src/memdir/findRelevantMemories.ts`

```typescript
export async function findRelevantMemories(
  query: string,
  memoryDir: string,
  signal: AbortSignal,
  recentTools: readonly string[] = [],
  alreadySurfaced: ReadonlySet<string> = new Set(),
): Promise<RelevantMemory[]> {
  // 1. 扫描记忆文件
  const memories = (await scanMemoryFiles(memoryDir, signal))
    .filter(m => !alreadySurfaced.has(m.filePath))

  // 2. LLM 选择
  const selectedFilenames = await selectRelevantMemories(query, memories, signal, recentTools)

  // 3. 返回路径 + mtime
  return selected.map(m => ({ path: m.filePath, mtimeMs: m.mtimeMs }))
}

async function selectRelevantMemories(...): Promise<string[]> {
  const result = await sideQuery({
    model: getDefaultSonnetModel(),
    system: SELECT_MEMORIES_SYSTEM_PROMPT,
    messages: [{ role: 'user', content: `Query: ${query}\n\nAvailable memories:\n${manifest}${toolsSection}` }],
    max_tokens: 256,
    output_format: { type: 'json_schema', schema: { ... } },
    signal,
    querySource: 'memdir_relevance',
  })
  // 解析 JSON 输出
}
```

**选择器系统提示**：
```
You are selecting memories that will be useful to Claude Code as it processes a user's query.
Return a list of filenames for the memories that will clearly be useful (up to 5).
Only include memories that you are certain will be helpful.
If you are unsure, do not include them. Be selective and discerning.
```

### 3.3 与 RRF 的对比

| 维度 | Claude Code | 传统 RRF |
|------|-------------|---------|
| 检索方式 | LLM 选择器 | 向量 + 关键词 + 语义 |
| 排序 | LLM 判断 | 倒数排名融合 |
| 索引 | 无（全量扫描） | 倒排 + 向量索引 |
| 扩展性 | ≤200 文件 | 百万级 |
| 延迟 | ~1-2s（LLM 调用） | ~50ms |

### 3.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1178** | 检索 | 无向量检索（纯 LLM 选择，无语义相似度） | P0 |
| **L1179** | 检索 | 无 RRF 混合检索（无多源排序融合） | P1 |
| **L1180** | 检索 | 无倒排索引（全量扫描 ≤200 文件） | P1 |
| **L1181** | 检索 | 无 Embedding 模型集成 | P1 |
| **L1182** | 检索 | 无检索缓存（每次查询独立 LLM 调用） | P2 |
| **L1183** | 检索 | 无 BM25/TF-IDF 关键词匹配 | P2 |

---

## 四、LLM网关路由（Model Routing & Failover）

### 4.1 路由架构

```
┌─────────────────────────────────────────────────────────────────┐
│                  LLM 网关路由架构                                 │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  提供商层 (providers.ts)                                         │
│  - firstParty (api.anthropic.com)                               │
│  - bedrock (AWS)                                                │
│  - vertex (GCP)                                                 │
│  - foundry (Azure)                                              │
│                                                                 │
│  路由层 (query.ts)                                               │
│  - 主模型选择: 用户配置 / 默认 Sonnet                             │
│  - 回退模型: fallbackModel (Opus 4.6)                            │
│  - 升级路径: 8k → 64k max_output_tokens                         │
│                                                                 │
│  重试层 (withRetry.ts)                                           │
│  - 529 过载: 最多 3 次重试（仅前台查询源）                        │
│  - 429 限流: 指数退避                                            │
│  - ECONNRESET: 禁用 keep-alive                                   │
│  - 非交互模式: 无限重试 + 心跳                                   │
│                                                                 │
│  清理层 (query.ts:893-953)                                       │
│  - 孤立 tool_use 块 → 错误结果                                   │
│  - 丢弃失败尝试的待处理结果                                       │
│  - 更新 tool use context 为新模型                                 │
│  - 剥离 thinking signatures（模型绑定）                           │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 4.2 模型回退机制

**文件**: `src/query.ts` (行 894-953)

```typescript
// FallbackTriggeredError 触发时
if (innerError instanceof FallbackTriggeredError && fallbackModel) {
  currentModel = fallbackModel

  // 1. 为孤立 tool_use 块生成错误结果
  // 2. 丢弃失败尝试的待处理结果
  // 3. 更新 tool use context 为新模型
  toolUseContext.options.mainLoopModel = fallbackModel

  // 4. 剥离 thinking signatures（模型绑定）
  // 5. 记录回退事件
  logEvent('tengu_model_fallback_triggered', {
    original_model: innerError.originalModel,
    fallback_model: innerError.fallbackModel,
  })

  // 6. 通知用户
  yield `Switched to ${renderModelName(innerError.fallbackModel)} due to high demand...`
}
```

### 4.3 重试策略

**文件**: `src/services/api/withRetry.ts`

```typescript
const DEFAULT_MAX_RETRIES = 10
const MAX_529_RETRIES = 3
const BASE_DELAY_MS = 500

// 仅前台查询源重试 529
const FOREGROUND_529_RETRY_SOURCES = new Set<QuerySource>([
  'repl_main_thread', 'sdk', 'agent:custom', 'agent:default',
  'agent:builtin', 'compact', 'hook_agent', 'hook_prompt',
  'verification_agent', 'side_question', 'auto_mode',
])

// 非交互模式：无限重试 + 心跳
function isPersistentRetryEnabled(): boolean {
  return feature('UNATTENDED_RETRY')
    ? isEnvTruthy(process.env.CLAUDE_CODE_UNATTENDED_RETRY)
    : false
}

// 退避策略
const PERSISTENT_MAX_BACKOFF_MS = 5 * 60 * 1000  // 5 分钟上限
const PERSISTENT_RESET_CAP_MS = 6 * 60 * 60 * 1000  // 6 小时重置
const HEARTBEAT_INTERVAL_MS = 30_000  // 30s 心跳
```

### 4.4 提供商路由

**文件**: `src/utils/model/providers.ts`

```typescript
export type APIProvider = 'firstParty' | 'bedrock' | 'vertex' | 'foundry'

export function getAPIProvider(): APIProvider {
  return isEnvTruthy(process.env.CLAUDE_CODE_USE_BEDROCK) ? 'bedrock'
    : isEnvTruthy(process.env.CLAUDE_CODE_USE_VERTEX) ? 'vertex'
    : isEnvTruthy(process.env.CLAUDE_CODE_USE_FOUNDRY) ? 'foundry'
    : 'firstParty'
}
```

### 4.5 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1184** | 网关路由 | 无负载均衡（单端点，无多实例分发） | P1 |
| **L1185** | 网关路由 | 无健康检查（无主动探测端点健康） | P1 |
| **L1186** | 网关路由 | 无熔断器（无 failsafe/circuit-breaker） | P1 |
| **L1187** | 网关路由 | 无请求级路由（无基于内容的路由） | P2 |
| **L1188** | 网关路由 | 无成本优化路由（无 cheapest-model-first） | P2 |
| **L1189** | 网关路由 | 无多 Provider 并发请求（无 race-to-complete） | P2 |

---

## 五、Pregel图执行（Graph Execution）

### 5.1 现状分析

claude-code **无 Pregel 图执行引擎**。其多 Agent 协作采用**消息传递 + 邮箱**模式，而非图计算：

```
┌─────────────────────────────────────────────────────────────────┐
│              Claude Code 多 Agent 通信模式                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Leader (主 Agent)                                              │
│      │                                                          │
│      ├── spawn → Teammate A (子 Agent)                           │
│      │     └── mailbox ←→ Leader mailbox                        │
│      │                                                          │
│      ├── spawn → Teammate B (子 Agent)                           │
│      │     └── mailbox ←→ Leader mailbox                        │
│      │                                                          │
│      └── spawn → Teammate C (子 Agent)                           │
│            └── mailbox ←→ Leader mailbox                        │
│                                                                 │
│  通信方式:                                                       │
│  - writeToMailbox / readMailbox (异步消息传递)                    │
│  - sendPermissionRequestViaMailbox (权限请求)                    │
│  - createIdleNotification (空闲通知)                             │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 与 Pregel 的对比

| 维度 | Claude Code | Pregel |
|------|-------------|--------|
| 计算模型 | 消息传递 | 顶点中心 |
| 迭代 | 无 superstep | 同步迭代 |
| 终止 | 无全局检测 | 全局投票 |
| 状态 | 邮箱消息 | 顶点状态 |
| 通信 | 点对点 | 邻居广播 |

### 5.3 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1190** | 图执行 | 无 Pregel 引擎（无顶点中心计算） | P2 |
| **L1191** | 图执行 | 无 superstep 同步（无全局迭代屏障） | P2 |
| **L1192** | 图执行 | 无图拓扑定义（无 DAG/Graph 结构） | P2 |
| **L1193** | 图执行 | 无消息传递 Combiner（无消息聚合） | P2 |
| **L1194** | 图执行 | 无 Aggregator（无全局聚合器） | P2 |

---

## 六、Skill生命周期（Skill Lifecycle）

### 6.1 完整生命周期

```
┌─────────────────────────────────────────────────────────────────┐
│                  Skill 全生命周期                                │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. 发现 (Discovery)                                             │
│     - loadSkillsDir.ts: 扫描 .claude/skills/ 目录                │
│     - bundledSkills.ts: 内置 Skill 注册                          │
│     - mcpSkillBuilders.ts: MCP Skill 发现                        │
│     - 六源加载: skills → plugin → managed → bundled → mcp → 命令  │
│                                                                 │
│  2. 解析 (Parsing)                                               │
│     - frontmatterParser.ts: YAML frontmatter 解析                │
│     - 字段: name/description/whenToUse/model/hooks/tools         │
│     - 参数解析: parseArgumentNames + substituteArguments          │
│                                                                 │
│  3. 注册 (Registration)                                          │
│     - registerBundledSkill(): 内置 Skill 注册表                   │
│     - registerMCPSkillBuilders(): MCP Skill 构建器注册            │
│     - commands.ts: 命令注册表整合                                │
│                                                                 │
│  4. 加载 (Loading)                                               │
│     - 懒加载: load 字段动态 import                               │
│     - 内容长度估算: estimateSkillFrontmatterTokens               │
│     - 文件身份: realpath 去重（符号链接处理）                     │
│                                                                 │
│  5. 执行 (Execution)                                             │
│     - SkillTool.ts: 模型调用 Skill                               │
│     - fork 模式: prepareForkedCommandContext                     │
│     - inline 模式: 直接注入                                      │
│     - 参数替换: $ARGUMENTS → 用户输入                            │
│                                                                 │
│  6. 清理 (Cleanup)                                               │
│     - clearInvokedSkillsForAgent: Agent 切换时清理               │
│     - addInvokedSkill: 记录已调用 Skill                          │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 6.2 内置 Skill 注册

**文件**: `src/skills/bundledSkills.ts`

```typescript
export type BundledSkillDefinition = {
  name: string
  description: string
  aliases?: string[]
  whenToUse?: string
  argumentHint?: string
  allowedTools?: string[]
  model?: string
  disableModelInvocation?: boolean
  userInvocable?: boolean
  isEnabled?: () => boolean
  hooks?: HooksSettings
  context?: 'inline' | 'fork'
  agent?: string
  files?: Record<string, string>  // 首次调用时提取到磁盘
  getPromptForCommand: (args: string, context: ToolUseContext) => Promise<ContentBlockParam[]>
}

export function registerBundledSkill(definition: BundledSkillDefinition): void {
  const { files } = definition
  let skillRoot: string | undefined
  let getPromptForCommand = definition.getPromptForCommand

  if (files && Object.keys(files).length > 0) {
    skillRoot = getBundledSkillExtractDir(definition.name)
    // 闭包内 memoization: 每个进程提取一次
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
    hooks: definition.hooks,
    skillRoot,
    context: definition.context,
    agent: definition.agent,
    isEnabled: definition.isEnabled,
    isHidden: !(definition.userInvocable ?? true),
    getPromptForCommand,
  }
  bundledSkills.push(command)
}
```

### 6.3 Skill 执行

**文件**: `src/tools/SkillTool/SkillTool.ts`

```typescript
export async function getAllCommands(context: ToolUseContext): Promise<Command[]> {
  // 仅包含 MCP skills（loadedFrom === 'mcp'），不包含 MCP prompts
  const mcpSkills = context.getAppState().mcp.commands
    .filter(cmd => cmd.type === 'prompt' && cmd.loadedFrom === 'mcp')
  if (mcpSkills.length === 0) return getCommands(getProjectRoot())
  const localCommands = await getCommands(getProjectRoot())
  return uniqBy([...localCommands, ...mcpSkills], 'name')
}
```

### 6.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1195** | Skill | 无 Skill 版本管理（无 semver/升级机制） | P2 |
| **L1196** | Skill | 无 Skill 依赖解析（Skill 间无依赖声明） | P2 |
| **L1197** | Skill | 无 Skill 沙箱（Skill 提示词可访问全局状态） | P2 |
| **L1198** | Skill | 无 Skill 热重载（修改需重启进程） | P2 |
| **L1199** | Skill | 无 Skill 市场/分发（仅 bundled + 本地） | P2 |

---

## 七、Agent预热池（Agent Warmup Pool）

### 7.1 预热机制

claude-code 的预热主要体现在 **API 预连接** 和 **Fork 缓存共享**：

```
┌─────────────────────────────────────────────────────────────────┐
│                    预热机制架构                                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  API 预连接 (apiPreconnect.ts)                                   │
│  - TCP+TLS 握手并行化（~100-200ms 重叠）                         │
│  - HEAD 请求预热 keep-alive 连接池                                │
│  - Bun fetch 全局连接池共享                                       │
│  - 跳过条件: proxy/mTLS/unix socket/Bedrock/Vertex/Foundry       │
│                                                                 │
│  Fork 缓存共享 (forkedAgent.ts)                                  │
│  - CacheSafeParams: 与父共享 prompt cache                        │
│  - 缓存键: system prompt + tools + model + messages + thinking   │
│  - saveCacheSafeParams / getLastCacheSafeParams                  │
│                                                                 │
│  MCP 预连接                                                      │
│  - 启动时建立 MCP 连接                                           │
│  - 连接池复用                                                    │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 7.2 API 预连接实现

**文件**: `src/utils/apiPreconnect.ts`

```typescript
export function preconnectAnthropicApi(): void {
  if (fired) return
  fired = true

  // 跳过云提供商（不同端点 + 认证）
  if (isEnvTruthy(process.env.CLAUDE_CODE_USE_BEDROCK) || ...) return

  // 跳过代理/mTLS/unix（SDK 自定义 dispatcher 不共享全局池）
  if (process.env.HTTPS_PROXY || process.env.CLAUDE_CODE_CLIENT_CERT || ...) return

  const baseUrl = process.env.ANTHROPIC_BASE_URL || getOauthConfig().BASE_API_URL

  // Fire-and-forget HEAD 请求
  void fetch(baseUrl, {
    method: 'HEAD',
    signal: AbortSignal.timeout(10_000),
  }).catch(() => {})
}
```

### 7.3 Fork 缓存共享

**文件**: `src/utils/forkedAgent.ts`

```typescript
export type CacheSafeParams = {
  systemPrompt: SystemPrompt    // 系统提示词 — 必须匹配
  userContext: { [k: string]: string }  // 用户上下文
  systemContext: { [k: string]: string } // 系统上下文
  toolUseContext: ToolUseContext  // 工具上下文（含 tools + model）
  forkContextMessages: Message[] // 父上下文消息
}

// 槽位：handleStopHooks 每次 turn 后写入
let lastCacheSafeParams: CacheSafeParams | null = null

export function saveCacheSafeParams(params: CacheSafeParams | null): void {
  lastCacheSafeParams = params
}
```

### 7.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1200** | 预热池 | 无 Agent 池化（每次 spawn 新建 Agent） | P1 |
| **L1201** | 预热池 | 无连接池预热（仅 API 预连接，无 LLM 连接池） | P1 |
| **L1202** | 预热池 | 无预热 Agent 复用（SubAgent 无对象池） | P2 |
| **L1203** | 预热池 | 无预热提示词编译（系统提示词无缓存编译） | P2 |
| **L1204** | 预热池 | 无预热工具注册表（每次重建工具集） | P2 |

---

## 八、Turn锁（Turn Lock & Lease）

### 8.1 Turn 并发控制

claude-code 的 Turn 控制采用 **AbortController 链 + turnCount** 模式：

```
┌─────────────────────────────────────────────────────────────────┐
│                  Turn 并发控制架构                                │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  AbortController 链 (abortController.ts)                         │
│  - createAbortController: 创建控制器（MaxListeners=50）           │
│  - createChildAbortController: 子控制器（WeakRef 防泄漏）         │
│  - 父 abort → 子自动 abort                                      │
│  - 子 abort → 不影响父                                          │
│                                                                 │
│  Turn 计数 (query.ts)                                            │
│  - turnCount: 当前 turn 数（从 1 开始）                          │
│  - turnCounter: 跨 compact 的累计计数                            │
│  - turnId: 每次 compact 后重置                                   │
│  - maxTurns: 最大 turn 限制                                     │
│                                                                 │
│  Abort 原因                                                      │
│  - 'interrupt': 用户中断（Ctrl+C）                               │
│  - 'aborted_streaming': 流式过程中中断                           │
│  - 'aborted_tools': 工具执行中中断                               │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 8.2 AbortController 实现

**文件**: `src/utils/abortController.ts`

```typescript
export function createChildAbortController(
  parent: AbortController,
  maxListeners?: number,
): AbortController {
  const child = createAbortController(maxListeners)

  // 快速路径：父已 abort
  if (parent.signal.aborted) {
    child.abort(parent.signal.reason)
    return child
  }

  // WeakRef 防止父保留被遗弃的子
  const weakChild = new WeakRef(child)
  const weakParent = new WeakRef(parent)
  const handler = propagateAbort.bind(weakParent, weakChild)

  parent.signal.addEventListener('abort', handler, { once: true })

  // 自动清理：子 abort 时移除父监听器
  child.signal.addEventListener('abort',
    removeAbortHandler.bind(weakParent, new WeakRef(handler)),
    { once: true },
  )

  return child
}
```

### 8.3 Turn 计数与终止

**文件**: `src/query.ts`

```typescript
// Turn 跟踪
let turnCount: number = 1
let turnCounter: number = 0
let turnId: string = deps.uuid()

// 每次迭代
tracking.turnCounter++
turnId = tracking.turnId

// 终止条件
if (maxTurns && nextTurnCountOnAbort > maxTurns) {
  return { reason: 'max_turns', turnCount: nextTurnCountOnAbort }
}

// 中断检测
if (toolUseContext.abortController.signal.aborted) {
  if (toolUseContext.abortController.signal.reason !== 'interrupt') {
    // 非用户中断的处理
  }
  return { reason: 'aborted_tools' }
}
```

### 8.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1205** | Turn锁 | 无 Turn 排他锁（无 Lease 机制） | P1 |
| **L1206** | Turn锁 | 无 Steer/Redirect（无法重定向正在执行的 Turn） | P1 |
| **L1207** | Turn锁 | 无 Turn 超时（无 per-turn 超时限制） | P1 |
| **L1208** | Turn锁 | 无 Turn 优先级（无优先级调度） | P2 |
| **L1209** | Turn锁 | 无 Turn 租约续期（Lease 无 renew 机制） | P2 |

---

## 九、HTTP客户端高级实现

### 9.1 连接池与代理架构

```
┌─────────────────────────────────────────────────────────────────┐
│                  HTTP 客户端架构                                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  代理层 (proxy.ts)                                               │
│  - HttpsProxyAgent: 标准 HTTP CONNECT 代理                       │
│  - undici.EnvHttpProxyAgent: 自动 NO_PROXY 感知                  │
│  - 连接池: Bun 全局 keep-alive 池                                │
│  - keepAlive 禁用: ECONNRESET 后自动禁用                         │
│                                                                 │
│  mTLS 层 (mtls.ts)                                               │
│  - CLAUDE_CODE_CLIENT_CERT/KEY/PASSPHRASE 环境变量               │
│  - 自定义 CA 证书 (getCACertificates)                            │
│  - WebSocket TLS 选项                                            │
│                                                                 │
│  重试层 (withRetry.ts)                                           │
│  - 529 过载: 3 次重试（仅前台）                                  │
│  - 429 限流: 指数退避                                            │
│  - ECONNRESET: 禁用 keep-alive + 重试                            │
│  - 非交互模式: 无限重试 + 30s 心跳                               │
│                                                                 │
│  上游代理层 (upstreamproxy/)                                      │
│  - CCR 容器 CONNECT-over-WebSocket 中继                          │
│  - Protobuf 编码 (UpstreamProxyChunk)                            │
│  - 30s ping 保活                                                 │
│  - 512KB 分块上限                                                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 9.2 代理配置

**文件**: `src/utils/proxy.ts`

```typescript
// 代理 URL 获取（优先级: 小写 > 大写）
export function getProxyUrl(env: EnvLike = process.env): string | undefined {
  return env.https_proxy || env.HTTPS_PROXY || env.http_proxy || env.HTTP_PROXY
}

// NO_PROXY 绕过（支持通配符、域名后缀、端口、IP）
export function shouldBypassProxy(urlString: string, noProxy?: string): boolean {
  if (noProxy === '*') return true
  const noProxyList = noProxy.split(/[,\s]+/).filter(Boolean)
  return noProxyList.some(pattern => {
    if (pattern.includes(':')) return hostWithPort === pattern
    if (pattern.startsWith('.')) {
      return hostname === pattern.substring(1) || hostname.endsWith(suffix)
    }
    return hostname === pattern
  })
}

// undici EnvHttpProxyAgent（自动 NO_PROXY 感知）
export const getProxyAgent = memoize((uri: string): undici.Dispatcher => {
  const proxyOptions: undici.EnvHttpProxyAgent.Options & { requestTls?: {...} } = {
    httpProxy: uri,
    httpsProxy: uri,
    noProxy: process.env.NO_PROXY || process.env.no_proxy,
  }
  if (mtlsConfig || caCerts) {
    proxyOptions.connect = tlsOpts
    proxyOptions.requestTls = tlsOpts
  }
  return new undiciMod.EnvHttpProxyAgent(proxyOptions)
})

// keep-alive 禁用（ECONNRESET 后）
let keepAliveDisabled = false
export function disableKeepAlive(): void { keepAliveDisabled = true }
```

### 9.3 上游代理（CCR 容器）

**文件**: `src/upstreamproxy/relay.ts`

```typescript
// CONNECT-over-WebSocket 中继
// 协议: UpstreamProxyChunk protobuf (bytes data = 1)
const MAX_CHUNK_BYTES = 512 * 1024  // Envoy 上限
const PING_INTERVAL_MS = 30_000     // 50s 空闲超时内

export function encodeChunk(data: Uint8Array): Uint8Array {
  // 手动 protobuf 编码（单字段 bytes 消息）
  // tag = (1 << 3) | 2 = 0x0a
  const varint: number[] = []
  let n = len
  while (n > 0x7f) { varint.push((n & 0x7f) | 0x80); n >>>= 7 }
  varint.push(n)
  const out = new Uint8Array(1 + varint.length + len)
  out[0] = 0x0a
  out.set(varint, 1)
  out.set(data, 1 + varint.length)
  return out
}
```

### 9.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1210** | HTTP | 无连接池调优（无 maxSockets/maxFreeSockets 配置） | P1 |
| **L1211** | HTTP | 无 HTTP/2 多路复用（无显式 H2 配置） | P1 |
| **L1212** | HTTP | 无请求管道化（无 pipelining 支持） | P2 |
| **L1213** | HTTP | 无代理链（无多跳代理 SOCKS→HTTP） | P2 |
| **L1214** | HTTP | 无 DNS 钉扎（无自定义 DNS 解析） | P2 |
| **L1215** | HTTP | 无连接池监控（无 pool stats/指标） | P2 |

---

## 十、安全加固（Security Hardening）

### 10.1 威胁模型与防护

```
┌─────────────────────────────────────────────────────────────────┐
│                  安全加固架构                                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Layer 1: Bash 命令安全 (bashSecurity.ts)                        │
│  - 命令替换检测: $(), <(), >(), =()                             │
│  - Zsh 危险命令: zmodload, emulate, sysopen, zpty, ztcp         │
│  - IFS 注入检测                                                 │
│  - 控制字符/Unicode 空白检测                                     │
│  - Tree-sitter AST 分析                                         │
│                                                                 │
│  Layer 2: 文件系统安全                                           │
│  - 路径验证 (pathValidation.ts)                                  │
│  - 只读验证 (readOnlyValidation.ts)                              │
│  - Git 安全 (gitSafety.ts)                                       │
│  - 破坏性命令警告 (destructiveCommandWarning.ts)                  │
│                                                                 │
│  Layer 3: 沙箱逃逸防护                                           │
│  - sandbox-runtime 集成                                          │
│  - 受管域策略 (allowManagedDomainsOnly)                          │
│  - 受管路径策略 (allowManagedReadPathsOnly)                       │
│  - prctl(PR_SET_DUMPABLE, 0) 防 ptrace                          │
│                                                                 │
│  Layer 4: 凭证安全                                               │
│  - API Key 环境变量（非文件存储）                                 │
│  - OAuth 令牌加密存储                                            │
│  - 会话 Token 文件即用即删                                        │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 10.2 Bash 安全检测

**文件**: `src/tools/BashTool/bashSecurity.ts`

```typescript
// 22 种安全检查（BASH_SECURITY_CHECK_IDS）
const BASH_SECURITY_CHECK_IDS = {
  INCOMPLETE_COMMANDS: 1,
  JQ_SYSTEM_FUNCTION: 2,
  JQ_FILE_ARGUMENTS: 3,
  OBFUSCATED_FLAGS: 4,
  SHELL_METACHARACTERS: 5,
  DANGEROUS_VARIABLES: 6,
  NEWLINES: 7,
  DANGEROUS_PATTERNS_COMMAND_SUBSTITUTION: 8,
  DANGEROUS_PATTERNS_INPUT_REDIRECTION: 9,
  DANGEROUS_PATTERNS_OUTPUT_REDIRECTION: 10,
  IFS_INJECTION: 11,
  GIT_COMMIT_SUBSTITUTION: 12,
  PROC_ENVIRON_ACCESS: 13,
  MALFORMED_TOKEN_INJECTION: 14,
  BACKSLASH_ESCAPED_WHITESPACE: 15,
  BRACE_EXPANSION: 16,
  CONTROL_CHARACTERS: 17,
  UNICODE_WHITESPACE: 18,
  MID_WORD_HASH: 19,
  ZSH_DANGEROUS_COMMANDS: 20,
  BACKSLASH_ESCAPED_OPERATORS: 21,
  COMMENT_QUOTE_DESYNC: 22,
}

// Zsh 危险命令集
const ZSH_DANGEROUS_COMMANDS = new Set([
  'zmodload', 'emulate', 'sysopen', 'sysread', 'syswrite',
  'sysseek', 'zpty', 'ztcp', 'zsocket', 'mapfile',
  'zf_rm', 'zf_mv', 'zf_ln', 'zf_chmod', 'zf_chown',
  'zf_mkdir', 'zf_rmdir', 'zf_chgrp',
])

// 命令替换模式检测
const COMMAND_SUBSTITUTION_PATTERNS = [
  { pattern: /<\(/, message: 'process substitution <()' },
  { pattern: />\(/, message: 'process substitution >()' },
  { pattern: /\$\(/, message: '$() command substitution' },
  { pattern: /\$\{/, message: '${} parameter substitution' },
  // ... 更多模式
]
```

### 10.3 Prompt 注入防护

claude-code **无显式 Prompt 注入检测器**。其防护主要依赖：

1. **系统提示词隔离**：用户输入通过 `<system-reminder>` 标签隔离
2. **工具结果隔离**：工具结果通过 `tool_result` 类型隔离
3. **Hook 系统**：可配置外部策略端点（http Hook）

### 10.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| **L1216** | 安全 | 无 Prompt 注入检测器（无显式注入模式识别） | P0 |
| **L1217** | 安全 | 无输入消毒框架（无统一 sanitize 层） | P1 |
| **L1218** | 安全 | 无密钥轮换（API Key 无自动轮换） | P1 |
| **L1219** | 安全 | 无审计日志（无操作审计追踪） | P1 |
| **L1220** | 安全 | 无沙箱逃逸检测（无逃逸行为监控） | P1 |
| **L1221** | 安全 | 无 TLS 指纹钉扎（无证书固定） | P2 |
| **L1222** | 安全 | 无请求签名（无 HMAC 请求签名） | P2 |
| **L1223** | 安全 | 无速率限制（无 per-user/API 速率限制） | P2 |

---

## 十一、综合架构图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Claude Code 第十七轮分析总览                           │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        用户交互层                                     │   │
│  │  REPL.tsx ←→ ResumeConversation.tsx ←→ Ink Fork (13,306 行)         │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        查询引擎层                                     │   │
│  │  query.ts (1729 行) 七阶段管线 + Turn 控制 + 模型回退                  │   │
│  │  QueryEngine.ts (1295 行) SDK/headless 入口                          │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        多 Agent 编排层                                │   │
│  │  coordinatorMode + swarm + AgentTool + SkillTool                     │   │
│  │  AsyncLocalStorage 上下文隔离 + 邮箱消息传递                           │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        基础设施层                                     │   │
│  │  gracefulShutdown + heapDump + sessionRestore + cleanupRegistry       │   │
│  │  proxy + mtls + upstreamproxy + withRetry                            │   │
│  │  sandbox-adapter + bashSecurity + pathValidation                     │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                    │                                        │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │                        数据层                                        │   │
│  │  sessionStorage (JSONL) + memdir (记忆) + concurrentSessions (PID)   │   │
│  │  findRelevantMemories (LLM 选择器) + extractMemories (后台提取)       │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## 十二、新增 gap 清单汇总

### 12.1 P0 紧急（4 项）

| 编号 | 维度 | 描述 | 推荐方案 |
|------|------|------|---------|
| **L1171** | 会话恢复 | 无增量检查点（全量日志重放，无 WAL 模式） | `rusqlite` + WAL + 6 PRAGMA |
| **L1172** | 会话恢复 | 无崩溃一致性保证（fsync 缺失，断电可能丢数据） | `synchronous=FULL` + `fsync` |
| **L1178** | 检索 | 无向量检索（纯 LLM 选择，无语义相似度） | `faiss-rs` + `ort` (ONNX) |
| **L1216** | 安全 | 无 Prompt 注入检测器（无显式注入模式识别） | 自定义分类器 + `linfa` |

### 12.2 P1 重要（20 项）

| 编号 | 维度 | 描述 | 推荐方案 |
|------|------|------|---------|
| **L1166** | CrashDump | 无自动内存泄漏检测 | 定时 `getHeapStatistics()` + 阈值触发 |
| **L1167** | CrashDump | 无错误指纹去重 | SHA256 fingerprinting |
| **L1168** | CrashDump | 无崩溃后自动重启 | restart loop + backoff |
| **L1173** | 多租户 | 无命名空间隔离 | `unshare` + `namespaces` |
| **L1174** | 多租户 | 无资源配额 | `cgroups-rs` |
| **L1177** | 多租户 | 无跨租户数据泄露防护 | per-tenant SQLite 连接 |
| **L1179** | 检索 | 无 RRF 混合检索 | `rerank` + 多源融合 |
| **L1180** | 检索 | 无倒排索引 | `tantivy` (Rust 全文搜索) |
| **L1181** | 检索 | 无 Embedding 模型集成 | `ort` + `tokenizers` |
| **L1184** | 网关路由 | 无负载均衡 | 多端点 + round-robin |
| **L1185** | 网关路由 | 无健康检查 | 主动探测 + 被动检测 |
| **L1186** | 网关路由 | 无熔断器 | `failsafe` crate |
| **L1200** | 预热池 | 无 Agent 池化 | 对象池模式 |
| **L1201** | 预热池 | 无连接池预热 | 预热 TCP+TLS 连接 |
| **L1205** | Turn锁 | 无 Turn 排他锁 | `tokio::sync::Mutex` + Lease |
| **L1206** | Turn锁 | 无 Steer/Redirect | 消息通道 + 重定向协议 |
| **L1207** | Turn锁 | 无 Turn 超时 | `tokio::time::timeout` |
| **L1210** | HTTP | 无连接池调优 | `hyper` + 自定义 Connector |
| **L1211** | HTTP | 无 HTTP/2 多路复用 | `hyper::h2` |
| **L1217** | 安全 | 无输入消毒框架 | 统一 sanitize 层 |
| **L1218** | 安全 | 无密钥轮换 | `keyring-rs` + 定时 rotate |
| **L1219** | 安全 | 无审计日志 | `tracing` + 审计后端 |
| **L1220** | 安全 | 无沙箱逃逸检测 | `landlock` + `seccomp` |

### 12.3 P2 进阶（20 项）

| 编号 | 维度 | 描述 | 推荐方案 |
|------|------|------|---------|
| **L1169** | CrashDump | 无 core dump | `mincore` + 信号处理 |
| **L1170** | CrashDump | 无 Watchdog 进程 | 独立监控线程 |
| **L1175** | 多租户 | 无网络策略 per-session | `landlock` 网络规则 |
| **L1176** | 多租户 | 无租户级审计日志 | per-tenant 审计后端 |
| **L1182** | 检索 | 无检索缓存 | `lru` + `ttl_cache` |
| **L1183** | 检索 | 无 BM25/TF-IDF | `tantivy` BM25 |
| **L1187** | 网关路由 | 无请求级路由 | 基于内容的路由表 |
| **L1188** | 网关路由 | 无成本优化路由 | 成本模型 + 路由策略 |
| **L1189** | 网关路由 | 无多 Provider 并发请求 | race-to-complete |
| **L1190-L1194** | 图执行 | 无 Pregel 引擎（5 项） | `timely-dataflow` 或自研 |
| **L1195-L1199** | Skill | 无版本管理/依赖/沙箱/热重载/市场（5 项） | 自定义 Skill 运行时 |
| **L1202-L1204** | 预热池 | 无 Agent 复用/提示词编译/工具注册表（3 项） | 对象池 + 缓存 |
| **L1208-L1209** | Turn锁 | 无优先级/租约续期（2 项） | 优先级队列 + Lease renew |
| **L1212-L1215** | HTTP | 无管道化/代理链/DNS 钉扎/池监控（4 项） | `hyper` + 自定义层 |
| **L1221-L1223** | 安全 | 无 TLS 钉扎/请求签名/速率限制（3 项） | `native-tls` + HMAC + token bucket |

---

## 十三、与前 16 轮 gap 的关系

| 轮次 | gap 数量 | gap 范围 | 本轮新增 |
|------|---------|---------|---------|
| 第 1-5 轮 | 12 | L1-L12 | - |
| 第 6 轮 | 15 | L13-L27 | - |
| 第 7 轮 | 18 | L28-L45 | - |
| 第 8 轮 | 22 | L46-L67 | - |
| 第 9 轮 | 41 | L68-L108 | - |
| 第 10 轮 | 64 | L79-L142 | - |
| 第 11-15 轮 | ~200 | L143-L1035 | - |
| 第 16 轮 | 20 | L1036-L1065 | - |
| **第 17 轮** | **48** | **L1166-L1223** | **+48** |

**注**: 第 17 轮与前 16 轮有部分维度重叠（如 CrashDump 在第 22 章已有涉及），但本轮对每个维度进行了**更深入的代码级分析**，并发现了新的 gap。

---

## 十四、核心发现与对 laew 的启示

### 14.1 核心发现

1. **gracefulShutdown 是生产级实现**：failsafe 定时器、终端模式清理、会话数据优先、Analytics 500ms 上限——每一处都体现了对"优雅退出"的深刻理解
2. **Heap Dump 服务完善**：V8 snapshot + 诊断 JSON + 自动泄漏检测，是 laew 缺失的关键能力
3. **记忆检索是 LLM 选择器而非 RRF**：用 Sonnet 选择 ≤5 个相关记忆，简单有效但扩展性差（≤200 文件）
4. **模型回退是完整清理**：孤立 tool_use 块处理、thinking signatures 剥离、用户通知——六步清理流程
5. **AsyncLocalStorage 是 Agent 上下文隔离的关键**：解决了多 Agent 并发时的状态污染问题
6. **bashSecurity 是 22 层防御**：从命令替换检测到 Zsh 危险命令，层层设防
7. **upstreamproxy 是 CONNECT-over-WebSocket 中继**：Protobuf 编码、30s ping、512KB 分块——生产级实现

### 14.2 对 laew 的启示

1. **优先补齐 P0 能力**：WAL 模式、fsync、向量检索、Prompt 注入检测
2. **借鉴 gracefulShutdown 设计**：failsafe 定时器 + 分层清理 + 超时保护
3. **引入 Heap Dump 服务**：V8 snapshot 的 Rust 等价是 `jemalloc_ctl` + `backtrace`
4. **采用 AsyncLocalStorage 等价**：Rust 可用 `tokio::task_local!` 实现类似隔离
5. **bashSecurity 的 22 层防御值得参考**：laew 的 Bash 工具零校验是最大安全风险
6. **模型回退的六步清理**：laew 需要类似的完整清理流程
7. **API 预连接模式**：laew 可借鉴 TCP+TLS 握手并行化

---

**文档生成信息**:
- 分析日期: 2026-09-09
- 源码版本: claudecode (最新)
- 分析工具: Codex
- 总行数: ~15,000+ 行
- 覆盖维度: 10 个全新维度
- 新增 gap: 48 项（L1166-L1223）
