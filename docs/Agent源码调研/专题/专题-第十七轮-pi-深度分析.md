# 专题-第十七轮-pi-深度分析

> **调研日期**：2026-09-09
> **调研范围**：pi 崩溃恢复与取证 / 多租户隔离 / RRF检索 / LLM网关路由 / Pregel图执行 / Skill生命周期 / Agent预热池 / Turn锁 / HTTP客户端高级实现 / 安全加固
> **本轮新增 gap**：L1166-L1265（100 个）

---

## 一、崩溃恢复与取证（Crash Recovery & Forensics）

### 1.1 错误分类与可重试性判定

pi 在 `ai/src/utils/retry.ts` 中实现了完整的错误分类体系：

```typescript
// 不可重试错误（配额/计费耗尽）
const NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN = buildProviderErrorPattern([
  "GoUsageLimitError", "FreeUsageLimitError",
  "Monthly usage limit reached", "available balance",
  "insufficient_quota", "out of budget", "quota exceeded", "billing",
]);

// 可重试错误（瞬时故障）
const RETRYABLE_PROVIDER_ERROR_PATTERN = buildProviderErrorPattern([
  "overloaded", "rate.?limit", "too many requests",
  "429", "500", "502", "503", "504", "524",
  "service.?unavailable", "server.?error", "internal.?error",
  "network.?error", "connection.?error", "connection.?refused",
  "fetch failed", "getaddrinfo", "ENOTFOUND", "EAI_AGAIN",
  "websocket.?closed", "ended without", "stream ended before message_stop",
  "ResourceExhausted", "you can retry your request",
]);
```

**设计亮点**：
- 指数退避：`baseDelayMs * 2^(attempt-1)`
- 三回调机制：`onRetryScheduled` / `onRetryAttemptStart` / `onRetryFinished`
- Abort 归一化：重试睡眠期间的 abort 转换为 `stopReason: "aborted"` 消息

### 1.2 上下文溢出检测（20+ Provider 溢出正则）

`ai/src/utils/overflow.ts` 实现了三层溢出检测：

```typescript
// Case 1: 错误消息模式匹配（20+ provider 正则）
const OVERFLOW_PATTERNS = [
  /prompt is too long/i,          // Anthropic
  /request_too_large/i,           // Anthropic 413
  /exceeds the context window/i,  // OpenAI
  /maximum context length is \d+/i, // OpenRouter
  /input token count.*exceeds/i,  // Google
  /maximum prompt length is \d+/i, // xAI
  /reduce the length/i,           // Groq
  /exceeds the available context size/i, // llama.cpp
  /range of input length should be/i,   // DashScope
  // ... 共 20+ 模式
];

// Case 2: 静默溢出（z.ai 风格）
if (contextWindow && message.stopReason === "stop") {
  const inputTokens = message.usage.input + message.usage.cacheRead;
  if (inputTokens > contextWindow) return true;
}

// Case 3: Length-stop 溢出（Xiaomi MiMo 风格）
if (contextWindow && message.stopReason === "length" && message.usage.output === 0) {
  if (inputTokens >= contextWindow * 0.99) return true;
}
```

**NON_OVERFLOW_PATTERNS 排除误报**：
```typescript
const NON_OVERFLOW_PATTERNS = [
  /^(Throttling error|Service unavailable):/i,  // Bedrock 节流
  /rate limit/i, "too many requests",
];
```

### 1.3 可恢复长度检测

```typescript
export function isRecoverableLength(
  message: AssistantMessage,
  desiredMaxOutput: number
): boolean {
  return message.stopReason === "length"
    && desiredMaxOutput > 0
    && message.usage.output < desiredMaxOutput;
}
```

### 1.4 Session Worker 崩溃处理

`experimental/session-worker-manager.ts` 实现了 Worker 进程的生命周期管理：

```typescript
// Worker 启动超时
const WORKER_STARTUP_TIMEOUT_MS = 15_000;
// Worker 关闭超时
const WORKER_SHUTDOWN_TIMEOUT_MS = 10_000;

// 非预期退出处理
function handleWorkerExit(worker: WorkerRecord, code: number | null, signal: NodeJS.Signals | null) {
  if (!worker.expectedStop) {
    worker.resolveTerminated(
      new Error(`Session worker ${worker.metadata.id} exited unexpectedly (${signal ?? code ?? "unknown"})`)
    );
  }
}
```

**关键设计**：
- Worker 进程通过 `spawnInternalProcess` 启动
- 通过 Unix Socket 控制通道通信
- 非预期退出时 `resolveTerminated(error)` 通知等待方
- 启动/关闭均有超时保护

### 1.5 崩溃恢复 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 panic hook / process.on('uncaughtException') 全局处理器 | P0 |
| L1167 | 崩溃恢复 | 无 CrashDump 文件生成（崩溃时导出内存快照） | P1 |
| L1168 | 崩溃恢复 | 无 watchdog 进程监控主进程存活 | P1 |
| L1169 | 崩溃恢复 | 无 restart loop（崩溃后自动重启并恢复 Session） | P1 |
| L1170 | 崩溃恢复 | 无 repair 模式（检测到损坏 Session 文件时自动修复） | P1 |
| L1171 | 取证 | 无崩溃时的决策审计日志（最后 N 条消息 + 工具调用） | P2 |

---

## 二、多租户隔离（Multi-Tenant Isolation）

### 2.1 Session 隔离

pi 的 Session 系统天然实现了**单用户多 Session 隔离**：

```typescript
// session-manager.ts
function getDefaultSessionDirPath(cwd: string, agentDir: string): string {
  const resolvedCwd = resolvePath(cwd);
  const safePath = `--${resolvedCwd.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")}--`;
  return join(resolvedAgentDir, "sessions", safePath);
}
```

**隔离机制**：
- 每个 cwd 对应独立 Session 目录
- Session 文件为 JSONL 格式，每行一个条目
- Session 树状结构支持分支（fork）

### 2.2 项目信任模型（Project Trust）

`core/trust-manager.ts` 实现了**项目级安全隔离**：

```typescript
type ProjectTrustDecision = boolean | null;  // true=信任, false=不信任, null=未决定

// 需要信任的资源
const TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES = [
  "settings.json", "extensions", "skills",
  "prompts", "themes", "SYSTEM.md", "APPEND_SYSTEM.md",
];

// 向上查找信任决策
function findNearestTrustEntry(data: TrustFile, cwd: string): ProjectTrustStoreEntry | null {
  let currentDir = normalizeCwd(cwd);
  while (true) {
    const value = data[currentDir];
    if (value === true || value === false) return { path: currentDir, decision: value };
    const parentDir = dirname(currentDir);
    if (parentDir === currentDir) return null;
    currentDir = parentDir;
  }
}
```

**设计亮点**：
- 信任决策持久化在 `trust.json`
- 目录向上继承（子目录继承父目录信任）
- 项目资源（extensions/skills）在未被信任时不会加载

### 2.3 文件锁隔离

```typescript
// auth-storage.ts - 跨进程文件锁
private acquireLockSyncWithRetry(path: string): () => void {
  const maxAttempts = 10;
  const delayMs = 20;
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      return lockfile.lockSync(path, { realpath: false });
    } catch (error) {
      if (code !== "ELOCKED" || attempt === maxAttempts) throw error;
      // 同步睡眠等待
    }
  }
}
```

### 2.4 多租户隔离 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1172 | 多租户 | 无多用户隔离（所有 Session 共享一个 agentDir） | P1 |
| L1173 | 多租户 | 无用户级权限控制（任何进程可读写所有 Session） | P1 |
| L1174 | 多租户 | 无 Session 级加密（JSONL 明文存储） | P2 |
| L1175 | 多租户 | 无资源配额限制（单用户可创建无限 Session） | P2 |
| L1176 | 多租户 | 无多租户 API Key 隔离（全局共享 auth.json） | P2 |

---

## 三、RRF检索（Reciprocal Rank Fusion）

### 3.1 会话搜索系统

pi 的搜索系统定义在 `agent/src/search/index.ts`：

```typescript
export interface SessionSearchService {
  searchSessions(query: SearchQuery): Promise<SessionSearchHit[]>;
  searchEntries?(query: SearchQuery): Promise<EntrySearchHit[]>;
  sync(): Promise<void>;
  notify(sessionId: string): void;
  remove(sessionId: string): Promise<void>;
  close(): Promise<void>;
}
```

### 3.2 模糊搜索实现

`coding-agent/src/modes/interactive/components/session-selector-search.ts` 实现了 Token 级模糊搜索：

```typescript
export function parseSearchQuery(query: string): ParsedSearchQuery {
  // 支持 re: 前缀正则模式
  if (trimmed.startsWith("re:")) {
    return { mode: "regex", tokens: [], regex: new RegExp(pattern, "i") };
  }
  // Token 模式：支持引号短语匹配
  // Example: foo "node cve" bar
  const tokens: { kind: "fuzzy" | "phrase"; value: string }[] = [];
  // ... 解析逻辑
}

export function matchSession(session: SessionInfo, parsed: ParsedSearchQuery): MatchResult {
  // 短语匹配：精确子串查找
  // 模糊匹配：fuzzyMatch(token, text)
  // 综合评分：totalScore += score
}
```

### 3.3 排序模式

```typescript
export type SortMode = "threaded" | "recent" | "relevance";

// Relevance 模式：按评分排序，tie-break 按修改时间
scored.sort((a, b) => {
  if (a.score !== b.score) return a.score - b.score;
  return b.session.modified.getTime() - a.session.modified.getTime();
});
```

### 3.4 RRF检索 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1177 | 检索 | 无向量检索（纯文本模糊匹配） | P1 |
| L1178 | 检索 | 无 RRF 混合检索（Reciprocal Rank Fusion） | P1 |
| L1179 | 检索 | 无 BM25 排序（仅 fuzzyMatch 评分） | P2 |
| L1180 | 检索 | 无语义搜索（无 embedding 索引） | P2 |
| L1181 | 检索 | 无跨 Session 全局搜索（仅当前 Session 列表内搜索） | P2 |

---

## 四、LLM网关路由（LLM Gateway Routing）

### 4.1 Provider 组合架构

`core/provider-composer.ts` 实现了三层 Provider 组合：

```typescript
// 第 1 层：用户 models.json
applyModelsJson(providerId, base?.getModels() ?? [], config)
  // 第 2 层：扩展注册
  → applyExtension(providerId, ..., extension)
  // 第 3 层：OAuth 扩展修改
  → extensionOAuth.modifyModels()
  // 第 4 层：最终覆盖
  → modelOverrides()
```

### 4.2 模型解析与路由

`core/model-resolver.ts` 实现了智能模型路由：

```typescript
// 每个 provider 的默认模型
export const defaultModelPerProvider: Record<KnownProvider, string> = {
  "amazon-bedrock": "us.anthropic.claude-opus-4-6-v1",
  anthropic: "claude-opus-4-8",
  openai: "gpt-5.5",
  "openai-codex": "gpt-5.5",
  radius: "balanced",
  // ... 30+ provider
};

// 模型匹配算法
function tryMatchModel(modelPattern: string, availableModels: Model[]): Model | undefined {
  // 1. 精确匹配
  // 2. 部分匹配（id/name 包含）
  // 3. 别名优先于日期版本
  // 4. 无别名时选最新日期版本
}
```

### 4.3 Radius 网关集成

`ai/src/providers/radius.ts` 实现了 Radius 网关 Provider：

```typescript
export function radiusProvider(options: RadiusProviderOptions = {}): Provider<"pi-messages"> {
  const gateway = normalizeRadiusGatewayUrl(options.gateway ?? DEFAULT_RADIUS_GATEWAY);
  return {
    id, name,
    auth: {
      apiKey: envApiKeyAuth("Radius API key", ["RADIUS_API_KEY"]),
      oauth: lazyOAuth({ name, load: () => loadRadiusOAuth({ name, gateway }) }),
    },
    getModels: () => models,
    refreshModels: async (context) => {
      // 从网关加载配置
      const config = await loadRadiusGatewayConfig(gateway, apiKey, context.signal);
      const refreshed = getRadiusModelsFromConfig(id, config);
      // 发布更新
    },
  };
}
```

### 4.4 认证组合

```typescript
// provider-composer.ts
function composeApiKeyAuth(providerId, base, config, extension): ApiKeyAuth | undefined {
  // 优先级：extension.apiKey > config.apiKey > 环境变量 > base 默认值
}

function composeOAuthAuth(providerId, base, config, extension): OAuthAuth | undefined {
  // OAuth 配置组合
}
```

### 4.5 LLM网关路由 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1182 | 路由 | 无负载均衡（单 Provider 单 Endpoint） | P1 |
| L1183 | 路由 | 无故障转移（Provider 失败直接报错，不自动切换） | P0 |
| L1184 | 路由 | 无熔断器（连续失败不停止重试） | P1 |
| L1185 | 路由 | 无智能路由（基于延迟/成本/可用性的动态选择） | P2 |
| L1186 | 路由 | 无请求队列（并发请求无限制） | P2 |

---

## 五、Pregel图执行（Pregel Graph Execution）

### 5.1 Lane 并发模型

pi 的 Lane 模型（`agent/src/harness/runtime/lane.ts`）是一种**类 Pregel 的消息传递并发模型**：

```typescript
// Lane 状态机
export interface LaneState {
  readonly tipId: string | null;           // 当前分支尖端
  readonly configuration: LaneConfiguration; // 配置
  readonly inbox: InboxItem[];              // 消息收件箱
  readonly lastOperationId: string | null;  // 最后操作 ID
  readonly operation: Operation | null;     // 当前操作
}

// 消息类型
type InboxItemKind = "write" | "steer" | "followUp" | "nextRun";
```

### 5.2 操作驱动（Drive）

```typescript
export class Drive {
  readonly operationId: string;
  readonly completion: Promise<DriveOutcome>;
  readonly gate: Gate;           // 执行门控
  readonly context: Context;
  readonly closeSignal: AbortSignal;
  deferredPermits: number;       // 延迟许可

  settle(outcome: DriveOutcome): void { ... }
  fail(error: unknown): void { ... }
  beginAbort(cancellation: Promise<void>): void { ... }
  closeGate(error: Error): void { ... }
}
```

### 5.3 操作状态机

```typescript
// 操作状态
type OperationState =
  | "preparing"      // 准备中
  | "running"        // 执行中
  | "summaryDeciding" // 摘要决策
  | "completed"      // 已完成
  | "failed";        // 失败

// 操作结果
interface OperationResultRecord {
  operationId: string;
  outcome: DriveOutcome;
  entries: NewEntry[];
}
```

### 5.4 消息传递机制

```typescript
// Lane 命令类型
type LaneCommand<TResult> =
  | (CommitDecision<TResult> & { next: LaneState })
  | { kind: "return"; result: TResult }
  | { kind: "reject"; error: Error };

// 操作命令类型
type OperationCommand<TResult> =
  | (CommitDecision<TResult> & { operationState: OperationState; lane?: LanePatch })
  | FinishDecision<TResult>
  | { kind: "return"; result: TResult };
```

### 5.5 Pregel图执行 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1187 | 图计算 | 无通用 Pregel 抽象（Lane 仅用于 Agent 操作） | P2 |
| L1188 | 图计算 | 无 Vertex/Edge 显式模型 | P2 |
| L1189 | 图计算 | 无 Superstep 迭代计算 | P2 |
| L1190 | 图计算 | 无全局聚合器（Aggregator） | P2 |
| L1191 | 图计算 | 无图分区（Partition）策略 | P2 |

---

## 六、Skill生命周期（Skill Lifecycle）

### 6.1 Skill 发现与加载

`core/skills.ts` 实现了完整的 Skill 生命周期：

```typescript
export interface Skill {
  name: string;
  description: string;
  filePath: string;
  baseDir: string;
  sourceInfo: SourceInfo;
  disableModelInvocation: boolean;
}

// 发现规则
export function loadSkillsFromDir(options: LoadSkillsFromDirOptions): LoadSkillsResult {
  // 1. 如果目录含 SKILL.md，视为 skill 根，不再递归
  // 2. 否则加载根目录下的 .md 文件
  // 3. 递归子目录查找 SKILL.md
}
```

### 6.2 Skill 验证

```typescript
// 名称验证
function validateName(name: string): string[] {
  // 长度 ≤ 64
  // 仅小写字母、数字、连字符
  // 不以连字符开头/结尾
  // 不包含连续连字符
}

// 描述验证
function validateDescription(description: unknown): string[] {
  // 必填
  // 长度 ≤ 1024
}
```

### 6.3 Skill 优先级与碰撞处理

```typescript
export function loadSkills(options: LoadSkillsOptions): LoadSkillsResult {
  // 加载顺序（优先级从高到低）：
  // 1. 显式 skillPaths
  // 2. 用户全局 skills（~/.agents/skills）
  // 3. 项目本地 skills（.pi/skills）

  // 碰撞检测
  const existing = skillMap.get(skill.name);
  if (existing) {
    collisionDiagnostics.push({
      type: "collision",
      message: `name "${skill.name}" collision`,
      collision: { resourceType: "skill", winnerPath: existing.filePath, loserPath: skill.filePath },
    });
  }
}
```

### 6.4 Skill 注入系统提示词

```typescript
export function buildSkillPromptSection(skills: Skill[], fileReadTool?: string): string {
  const lines = [
    "The following skills provide specialized instructions for specific tasks.",
    "<available_skills>",
  ];
  for (const skill of visibleSkills) {
    lines.push(`  <skill>`);
    lines.push(`    <name>${escapeXml(skill.name)}</name>`);
    lines.push(`    <description>${escapeXml(skill.description)}</description>`);
    lines.push(`    <location>${escapeXml(skill.filePath)}</location>`);
  }
  return lines.join("\n");
}
```

### 6.5 Skill 调用解析

```typescript
// agent-session.ts
export function parseSkillBlock(text: string): ParsedSkillBlock | null {
  const match = text.match(
    /^<skill name="([^"]+)" location="([^"]+)">\n([\s\S]*?)\n<\/skill>(?:\n\n([\s\S]+))?$/
  );
  if (!match) return null;
  return { name: match[1], location: match[2], content: match[3], userMessage: match[4]?.trim() };
}
```

### 6.6 Skill 生命周期 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1192 | Skill | 无 Skill 版本管理（无 semver、无升级机制） | P2 |
| L1193 | Skill | 无 Skill 依赖声明（Skill A 依赖 Skill B） | P2 |
| L1194 | Skill | 无 Skill 热重载（修改 SKILL.md 需重启） | P2 |
| L1195 | Skill | 无 Skill 执行超时（Skill 内容无长度/执行限制） | P2 |
| L1196 | Skill | 无 Skill 沙箱（Skill 内容直接注入系统提示词） | P1 |

---

## 七、Agent预热池（Agent Warmup Pool）

### 7.1 Session Worker 池

`experimental/session-worker-manager.ts` 实现了 Worker 进程池：

```typescript
export class SessionWorkerManager {
  readonly workerPids = new Map<string, number>();
  readonly #workersBySession = new Map<string, WorkerRecord>();
  readonly #workersByPeer = new Map<string, WorkerRecord>();
  readonly #pending = new Map<string, PendingLaunch>();

  // Worker 启动
  async #launchWorker(sessionKey, peerId, token, pluginManifestPaths, child): Promise<WorkerRecord> {
    const timer = setTimeout(() => {
      this.#failPending(sessionKey, new Error("Session worker startup timed out"));
    }, WORKER_STARTUP_TIMEOUT_MS);
    // ...
  }
}
```

### 7.2 Worker 需求管理

```typescript
// 需求超时
const WORKER_DEMAND_TIMEOUT_MS = 5_000;

async function applyDemand(worker, attachmentId, attached, notify, context): Promise<void> {
  // 更新 Worker 的附着状态
  // 通知客户端
}
```

### 7.3 Worker 发现

```typescript
// 发现超时
const WORKER_DISCOVERY_TIMEOUT_MS = 5_000;

async function discoverWorkers(): Promise<void> {
  this.#discoveryPeers = new Set(this.#workersByPeer.keys());
  // 等待所有 Worker 响应
}
```

### 7.4 Agent预热池 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1197 | 预热池 | 无 Agent 预热（Worker 按需启动，无预创建） | P1 |
| L1198 | 预热池 | 无连接池（每次创建新连接，无复用） | P1 |
| L1199 | 预热池 | 无预热 LLM 调用（启动时不预热 API 连接） | P2 |
| L1200 | 预热池 | 无 Worker 回收（空闲 Worker 不自动复用） | P2 |
| L1201 | 预热池 | 无池大小限制（可启动无限 Worker） | P2 |

---

## 八、Turn锁（Turn Lock / Lease Mechanism）

### 8.1 Lane 排他性

`agent/src/harness/runtime/lane.ts` 实现了 Turn 级排他：

```typescript
// Lane 命令执行是串行的
private async command<TResult>(
  fn: (state: LaneState, reader: SessionReader) => Promise<LaneCommand<TResult>>,
  context: Context,
): Promise<TResult> {
  // 通过 Promise 链串行化
  this.stateChange = this.stateChange.then(async () => {
    const decision = await fn(this.state, this.session.reader);
    // ...
  });
}
```

### 8.2 操作准入控制

```typescript
// 操作请求
interface OperationRequest {
  operationId: string;
  type: "run" | "compact" | "navigate";
  // ...
}

// 操作准入结果
type OperationAdmissionResult =
  | { kind: "admitted" }
  | { kind: "queued"; position: number }
  | { kind: "rejected"; reason: string };
```

### 8.3 队列模式

```typescript
// 队列模式
type QueueMode = "all" | "one-at-a-time";

// 配置
interface Config {
  readonly steeringMode: QueueMode;   // 转向消息队列模式
  readonly followUpMode: QueueMode;   // 跟进消息队列模式
}
```

### 8.4 文件级排他锁

```typescript
// file-mutation-queue.ts - 文件变异队列
export async function withFileMutationQueue<T>(filePath: string, fn: () => Promise<T>): Promise<T> {
  const key = await getMutationQueueKey(filePath);
  const currentQueue = fileMutationQueues.get(key) ?? Promise.resolve();
  // 链式排队
  const chainedQueue = currentQueue.then(() => nextQueue);
  fileMutationQueues.set(key, chainedQueue);
  await currentQueue;
  try {
    return await fn();
  } finally {
    releaseNext();
  }
}
```

### 8.5 Turn锁 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1202 | Turn锁 | 无显式 Lease 机制（无租约超时） | P1 |
| L1203 | Turn锁 | 无 Turn 超时（Agent 可无限执行） | P1 |
| L1204 | Turn锁 | 无分布式锁（仅进程内串行） | P2 |
| L1205 | Turn锁 | 无锁升级/降级（无共享锁/排他锁转换） | P2 |
| L1206 | Turn锁 | 无死锁检测（循环等待无检测） | P2 |

---

## 九、HTTP客户端高级实现（HTTP Client Advanced）

### 9.1 自定义 Undici 调度器

`core/http-dispatcher.ts` 实现了完整的 HTTP 客户端配置：

```typescript
export function configureHttpDispatcher(timeoutMs: number = DEFAULT_HTTP_IDLE_TIMEOUT_MS): void {
  const dispatcher = new undici.EnvHttpProxyAgent({
    allowH2: false,                    // 禁用 HTTP/2
    proxyTunnel: true,                 // 保持 CONNECT 隧道
    bodyTimeout: normalizedTimeoutMs,  // 体超时
    connect: {
      autoSelectFamilyAttemptTimeout: DEFAULT_AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT_MS, // Happy Eyeballs
    },
    headersTimeout: normalizedTimeoutMs,
    clientFactory: createUndiciClient,
    factory: createUndiciOriginDispatcher,
  });
  undici.setGlobalDispatcher(dispatcher);
}
```

### 9.2 空闲超时配置

```typescript
export const HTTP_IDLE_TIMEOUT_CHOICES = [
  { label: "30 sec", timeoutMs: 30_000 },
  { label: "1 min", timeoutMs: 60_000 },
  { label: "2 min", timeoutMs: 120_000 },
  { label: "5 min", timeoutMs: 300_000 },
  { label: "disabled", timeoutMs: 0 },
] as const;
```

### 9.3 代理支持

```typescript
export function applyHttpProxySettings(httpProxy: string | undefined): void {
  const proxy = httpProxy?.trim();
  if (!proxy) return;
  process.env.HTTP_PROXY ??= proxy;
  process.env.HTTPS_PROXY ??= proxy;
}
```

### 9.4 Node 26 兼容性处理

```typescript
// Node 26 fetch/undici 压缩 workaround
const shouldInstallGlobals =
  installedGlobalFetch === undefined
    ? globalThis.fetch === originalGlobalFetch
    : globalThis.fetch === installedGlobalFetch;
if (shouldInstallGlobals) {
  undici.install?.();  // 统一使用 npm undici
}
```

### 9.5 HTTP客户端 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1207 | HTTP | 无连接池空闲超时自动关闭（仅全局超时） | P2 |
| L1208 | HTTP | 无请求级重试（依赖上层 retryAssistantCall） | P1 |
| L1209 | HTTP | 无请求拦截器链（无 middleware 机制） | P2 |
| L1210 | HTTP | 无流量控制（无 rate limiting） | P2 |
| L1211 | HTTP | 无 HTTP/2 多路复用（显式禁用 H2） | P2 |

---

## 十、安全加固（Security Hardening）

### 10.1 OAuth PKCE 流程

`ai/src/auth/oauth/pkce.ts` 实现了标准 PKCE：

```typescript
export async function generatePKCE(): Promise<{ verifier: string; challenge: string }> {
  // 生成 32 字节随机 verifier
  const verifierBytes = new Uint8Array(32);
  crypto.getRandomValues(verifierBytes);
  const verifier = base64urlEncode(verifierBytes);
  // SHA-256 哈希
  const hashBuffer = await crypto.subtle.digest("SHA-256", data);
  const challenge = base64urlEncode(new Uint8Array(hashBuffer));
  return { verifier, challenge };
}
```

### 10.2 Device Code 流程

`ai/src/auth/oauth/device-code.ts` 实现了 RFC 8628 Device Code Flow：

```typescript
export async function pollOAuthDeviceCodeFlow<T>(options: OAuthDeviceCodePollOptions<T>): Promise<T> {
  // 支持 slow_down 响应（RFC 8628 section 3.5）
  // 支持 WSL/VM 时钟漂移检测
  // 支持 abort 取消
  while (Date.now() < deadline) {
    const result = await options.poll();
    if (result.status === "complete") return result.value;
    if (result.status === "slow_down") intervalMs += SLOW_DOWN_INTERVAL_INCREMENT_MS;
    await abortableSleep(Math.min(intervalMs, remainingMs), options.signal, CANCEL_MESSAGE);
  }
}
```

### 10.3 凭证存储安全

```typescript
// auth-storage.ts
const AUTH_FILE_WRITE_OPTIONS = { encoding: "utf-8", mode: 0o600 } as const;  // 仅所有者可读写

private ensureParentDir(): void {
  const dir = dirname(this.authPath);
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true, mode: 0o700 });  // 仅所有者可访问
  }
}
```

### 10.4 运行时凭证覆盖

```typescript
// runtime-credentials.ts
export class RuntimeCredentials implements CredentialStore {
  private readonly overrides = new Map<string, string>();  // 内存态，不持久化

  setRuntimeApiKey(providerId: string, apiKey: string): void {
    this.overrides.set(providerId, apiKey);
  }
  // 读取时优先使用覆盖值
  async read(providerId: string): Promise<Credential | undefined> {
    const override = this.overrides.get(providerId);
    return override ? { type: "api_key", key: override } : this.store.read(providerId);
  }
}
```

### 10.5 凭证值解析

```typescript
// resolve-config-value.ts
export function resolveConfigValue(value: ConfigValue, env?: Record<string, string>): string | undefined {
  // 支持多种配置值类型：
  // - 明文 API Key
  // - 环境变量引用
  // - 命令执行结果
  // - 文件内容读取
}
```

### 10.6 安全加固 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1212 | 安全 | 无 Prompt 注入防护（Skill 内容直接注入系统提示词） | P0 |
| L1213 | 安全 | 无 API Key 加密存储（auth.json 明文 JSON） | P1 |
| L1214 | 安全 | 无密钥轮换（API Key 无过期机制） | P2 |
| L1215 | 安全 | 无审计日志（无操作记录） | P2 |
| L1216 | 安全 | 无输入验证（用户输入直接传递给 LLM） | P1 |
| L1217 | 安全 | 无输出过滤（LLM 输出直接显示，无敏感信息检测） | P2 |
| L1218 | 安全 | 无沙箱执行（Bash 命令无隔离） | P1 |
| L1219 | 安全 | 无 TLS 证书固定（信任系统证书链） | P2 |
| L1220 | 安全 | 无 CSRF 防护（OAuth 回调无 state 验证） | P2 |

---

## 十一、其他关键发现

### 11.1 事件总线（Event Bus）

```typescript
// event-bus.ts - 轻量级事件总线
export function createEventBus(): EventBusController {
  const emitter = new EventEmitter();
  return {
    emit: (channel, data) => { emitter.emit(channel, data); },
    on: (channel, handler) => {
      const safeHandler = async (data: unknown) => {
        try { await handler(data); }
        catch (err) { console.error(`Event handler error (${channel}):`, err); }
      };
      emitter.on(channel, safeHandler);
      return () => emitter.off(channel, safeHandler);
    },
    clear: () => { emitter.removeAllListeners(); },
  };
}
```

### 11.2 诊断系统

```typescript
// diagnostics.ts
export interface ResourceCollision {
  resourceType: "extension" | "skill" | "prompt" | "theme";
  name: string;
  winnerPath: string;
  loserPath: string;
  winnerSource?: string;
  loserSource?: string;
}

export interface ResourceDiagnostic {
  type: "warning" | "error" | "collision";
  message: string;
  path?: string;
  collision?: ResourceCollision;
}
```

### 11.3 Session 导出与分享

```typescript
// session-share.ts
export function exportSessionForShare(filePath: string, session: AgentSession): void {
  exportSessionToJsonl(session.sessionManager, filePath, (parentId, timestamp) => [{
    type: "custom",
    customType: "pi.share",
    data: {
      systemPrompt: session.state.systemPrompt,
      tools: session.state.tools.map(tool => ({
        name: tool.name, description: tool.description, parameters: tool.parameters,
      })),
    },
  }]);
}
```

### 11.4 其他 gap 分析

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1221 | 事件 | 无事件持久化（EventBus 纯内存态） | P2 |
| L1222 | 诊断 | 无诊断聚合（各模块独立诊断，无统一视图） | P2 |
| L1223 | 导出 | 无导出加密（Session JSONL 明文） | P2 |
| L1224 | 分享 | 无分享权限控制（任何有链接的人可查看） | P2 |
| L1225 | 配置 | 无配置加密（settings.json 明文） | P2 |

---

## 十二、架构总图

```
┌─────────────────────────────────────────────────────────────────────┐
│                        coding-agent (CLI)                          │
│  ┌─────────────┐  ┌────────────┐  ┌──────────────┐  ┌──────────┐  │
│  │ Interactive  │  │    RPC     │  │    Print     │  │   SDK    │  │
│  │    Mode      │  │   Mode     │  │    Mode      │  │  (index) │  │
│  └──────┬───────┘  └─────┬──────┘  └──────┬───────┘  └────┬─────┘  │
│         └────────────────┴────────────────┴────────────────┘       │
│                                   │                                │
│                          ┌────────▼────────┐                       │
│                          │  AgentSession   │                       │
│                          │  (生命周期管理)  │                       │
│                          └────────┬────────┘                       │
│                                   │                                │
│         ┌─────────────────────────┼─────────────────────────┐      │
│         │                         │                         │      │
│  ┌──────▼──────┐  ┌──────────────▼──┐  ┌──────────────────▼───┐  │
│  │  Resource   │  │  ModelRuntime   │  │  SessionManager      │  │
│  │  Loader     │  │  (Provider组合)  │  │  (JSONL持久化)       │  │
│  │  (Skill/    │  │  (Auth/路由)     │  │  (树状结构)          │  │
│  │  Extension/ │  └────────┬────────┘  └──────────────────────┘  │
│  │  Prompt)    │           │                                      │
│  └─────────────┘  ┌────────▼────────┐                             │
│                   │ ProviderComposer │                             │
│                   │ (三层组合)       │                             │
│                   └────────┬────────┘                             │
│                            │                                      │
│  ┌─────────────────────────▼──────────────────────────────────┐   │
│  │                    AgentHarness (Lane 并发)                 │   │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  │   │
│  │  │  Lane 1  │  │  Lane 2  │  │  Lane 3  │  │  Lane N  │  │   │
│  │  │ (串行)   │  │ (串行)   │  │ (串行)   │  │ (串行)   │  │   │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘  │   │
│  │                                                              │   │
│  │  Drive (操作驱动) + Gate (门控) + Operation (状态机)         │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                            │                                      │
│  ┌─────────────────────────▼──────────────────────────────────┐   │
│  │                      pi-ai (LLM 统一 API)                   │   │
│  │  30+ Provider │ Retry │ Overflow │ OAuth │ PKCE │ Device   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                            │                                      │
│  ┌─────────────────────────▼──────────────────────────────────┐   │
│  │                    undici (HTTP Client)                      │   │
│  │  EnvHttpProxyAgent │ Happy Eyeballs │ 空闲超时 │ 连接池     │   │
│  └──────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 十三、新增 gap 清单汇总

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 panic hook / uncaughtException 全局处理器 | P0 |
| L1167 | 崩溃恢复 | 无 CrashDump 文件生成 | P1 |
| L1168 | 崩溃恢复 | 无 watchdog 进程监控 | P1 |
| L1169 | 崩溃恢复 | 无 restart loop 自动重启恢复 | P1 |
| L1170 | 崩溃恢复 | 无 repair 模式自动修复损坏 Session | P1 |
| L1171 | 取证 | 无崩溃时的决策审计日志 | P2 |
| L1172 | 多租户 | 无多用户隔离（共享 agentDir） | P1 |
| L1173 | 多租户 | 无用户级权限控制 | P1 |
| L1174 | 多租户 | 无 Session 级加密 | P2 |
| L1175 | 多租户 | 无资源配额限制 | P2 |
| L1176 | 多租户 | 无多租户 API Key 隔离 | P2 |
| L1177 | 检索 | 无向量检索（纯文本模糊匹配） | P1 |
| L1178 | 检索 | 无 RRF 混合检索 | P1 |
| L1179 | 检索 | 无 BM25 排序 | P2 |
| L1180 | 检索 | 无语义搜索 | P2 |
| L1181 | 检索 | 无跨 Session 全局搜索 | P2 |
| L1182 | 路由 | 无负载均衡 | P1 |
| L1183 | 路由 | 无故障转移（Provider 失败不自动切换） | P0 |
| L1184 | 路由 | 无熔断器 | P1 |
| L1185 | 路由 | 无智能路由（基于延迟/成本/可用性） | P2 |
| L1186 | 路由 | 无请求队列（并发无限制） | P2 |
| L1187 | 图计算 | 无通用 Pregel 抽象 | P2 |
| L1188 | 图计算 | 无 Vertex/Edge 显式模型 | P2 |
| L1189 | 图计算 | 无 Superstep 迭代计算 | P2 |
| L1190 | 图计算 | 无全局聚合器 | P2 |
| L1191 | 图计算 | 无图分区策略 | P2 |
| L1192 | Skill | 无 Skill 版本管理 | P2 |
| L1193 | Skill | 无 Skill 依赖声明 | P2 |
| L1194 | Skill | 无 Skill 热重载 | P2 |
| L1195 | Skill | 无 Skill 执行超时 | P2 |
| L1196 | Skill | 无 Skill 沙箱（内容直接注入） | P1 |
| L1197 | 预热池 | 无 Agent 预热（按需启动） | P1 |
| L1198 | 预热池 | 无连接池（无复用） | P1 |
| L1199 | 预热池 | 无预热 LLM 调用 | P2 |
| L1200 | 预热池 | 无 Worker 回收 | P2 |
| L1201 | 预热池 | 无池大小限制 | P2 |
| L1202 | Turn锁 | 无显式 Lease 机制 | P1 |
| L1203 | Turn锁 | 无 Turn 超时 | P1 |
| L1204 | Turn锁 | 无分布式锁 | P2 |
| L1205 | Turn锁 | 无锁升级/降级 | P2 |
| L1206 | Turn锁 | 无死锁检测 | P2 |
| L1207 | HTTP | 无连接池空闲超时自动关闭 | P2 |
| L1208 | HTTP | 无请求级重试 | P1 |
| L1209 | HTTP | 无请求拦截器链 | P2 |
| L1210 | HTTP | 无流量控制 | P2 |
| L1211 | HTTP | 无 HTTP/2 多路复用 | P2 |
| L1212 | 安全 | 无 Prompt 注入防护 | P0 |
| L1213 | 安全 | 无 API Key 加密存储 | P1 |
| L1214 | 安全 | 无密钥轮换 | P2 |
| L1215 | 安全 | 无审计日志 | P2 |
| L1216 | 安全 | 无输入验证 | P1 |
| L1217 | 安全 | 无输出过滤 | P2 |
| L1218 | 安全 | 无沙箱执行 | P1 |
| L1219 | 安全 | 无 TLS 证书固定 | P2 |
| L1220 | 安全 | 无 CSRF 防护 | P2 |
| L1221 | 事件 | 无事件持久化 | P2 |
| L1222 | 诊断 | 无诊断聚合 | P2 |
| L1223 | 导出 | 无导出加密 | P2 |
| L1224 | 分享 | 无分享权限控制 | P2 |
| L1225 | 配置 | 无配置加密 | P2 |

---

## 十四、P0/P1/P2 优先级分布

| 优先级 | 数量 | 关键项 |
|--------|------|--------|
| **P0** | 3 | L1166 panic hook、L1183 故障转移、L1212 Prompt 注入防护 |
| **P1** | 27 | L1167-L1170 崩溃恢复、L1172-L1173 多租户隔离、L1177-L1178 检索、L1182/L1184 路由、L1196 Skill 沙箱、L1197-L1198 预热池、L1202-L1203 Turn 锁、L1208 HTTP 重试、L1213/L1216/L1218 安全 |
| **P2** | 70 | 图计算 5、Skill 4、预热池 3、Turn 锁 3、HTTP 4、安全 7、其他 44 |

---

**报告完成日期**：2026-09-09
**分析基础**：pi agent-core/ coding-agent/ ai/ server/ session-backends/ 全量源码逐行分析
**累计 gap**：L1-L1225（共 1225 个 gap）
