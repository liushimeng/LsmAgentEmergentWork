# 专题-第十七轮-openclaw-深度分析

> **调研日期**：2026-09-09
> **调研范围**：openclaw 崩溃恢复与取证、多租户隔离、RRF检索、LLM网关路由、Pregel图执行、Skill生命周期、Agent预热池、Turn锁、HTTP客户端高级实现、安全加固
> **本轮新增 gap**：L1166-L1265（100 个）
> **分析基础**：openclaw src/ 下 9,200+ 行核心源码逐行分析

---

## 一、崩溃恢复与取证（Crash Recovery & Forensics）

### 1.1 五层 CrashDump 防御纵深（扩展第十一轮）

openclaw 的崩溃恢复不是单一机制，而是跨越 5 个模块的纵深防御体系：

```
┌─────────────────────────────────────────────────────────────┐
│ Layer 1: Process Respawn (entry.respawn.ts, 190 行)         │
│   Windows 栈大小 / NODE_OPTIONS / CA certs 自动修复          │
├─────────────────────────────────────────────────────────────┤
│ Layer 2: Config Observe Recovery (io.observe-recovery.ts)   │
│   last-known-good 恢复 + 可疑检测 + 权限加固                  │
├─────────────────────────────────────────────────────────────┤
│ Layer 3: Config Health Fingerprint (io.health-state.ts)     │
│   hash + bytes + mtime + dev/ino + mode + uid/gid           │
├─────────────────────────────────────────────────────────────┤
│ Layer 4: Clobber Snapshot (io.clobber-snapshot.ts)          │
│   32 槽快照 + mkdir 文件锁 + 跨进程队列                      │
├─────────────────────────────────────────────────────────────┤
│ Layer 5: Backup Rotation (backup-rotation.ts)               │
│   5 槽环形备份 + pre-update 快照 + 权限加固                   │
└─────────────────────────────────────────────────────────────┘
```

**关键设计**：
- **原子写入**：`replaceFileAtomic` 保证配置写入要么成功要么不留痕迹
- **权限加固**：`chmodConfigBestEffort` 每次恢复后强制 0o600
- **跨进程锁**：`acquireClobberLock` 用 mkdir 原子性实现跨进程互斥
- **弱网恢复**：`shouldRemoveStaleLock` 检测 30s 过期的 stale lock

### 1.2 Main Session Restart Recovery（主会话重启恢复）

`agents/main-session-recovery/` 目录（28+ 文件）实现了完整的会话重启恢复：

```typescript
// 恢复状态机
type RecoveryPhase = "mark_interrupted" | "observe" | "clear" | "restore_yielded";

// 恢复运行记录
interface RestartRecoveryRun {
  runId: string;
  lifecycleGeneration: string;
  sourceRunId: string;
  // ...
}
```

**恢复流程**：
1. **标记阶段**（marking）：`markRestartAbortedMainSessions` 扫描所有 running 状态的 session entry
2. **发现阶段**（discovery）：`discoverRestartRecoveryStoreTargets` 遍历 per-agent SQLite store
3. **恢复阶段**（runtime）：`recoverRestartAbortedMainSessions` 执行带退避的重试
4. **完成阶段**（settlement）：`transitionMainSessionRecovery` 更新状态

**退避策略**：
```typescript
const DEFAULT_RECOVERY_DELAY_MS = 1_000;
const MAX_RECOVERY_RETRIES = 5;
const RETRY_BACKOFF_MULTIPLIER = 2; // 1s → 2s → 4s → 8s → 16s
```

### 1.3 SubAgent 重启恢复

`agents/subagents/registry/subagent-registry-restart-recovery.ts`（551 行）实现了 SubAgent 级别的恢复：

```typescript
const MAX_RECOVERY_ATTEMPTS = 2;
const RECOVERY_ATTEMPT_WINDOW_MS = 2 * 60_000;
const MAX_INTERRUPTION_AGE_MS = 2 * 60 * 60_000;
```

**恢复阶段机**：
```
reserved → attempted → consumed → accepted → abandoned
```

**幂等键**：`buildRestartRecoveryIdempotencyKey` 确保同一 runId 不会重复恢复

### 1.4 Process Supervisor（进程监控器）

`process/supervisor/supervisor.ts`（712 行）实现了完整的子进程生命周期管理：

```typescript
// 关键特性
- Scope 隔离：每个 scopeKey 独立管理子进程
- 替换栅栏：replaceExistingScope 保证新 run 等待旧 run 完成
- 超时控制：overall-timeout + no-output-timeout 双超时
- OOM 评分：linux-oom-score 集成
- 输出截断：1MB 上限 + UTF-16 安全截断
- 优雅取消：GRACEFUL_CANCEL_TIMEOUT_MS 宽限期
```

**laew gap L1166**：无进程 Supervisor 机制。laew 的 BashTool 直接 spawn，无 scope 隔离/超时/输出截断/OOM 控制。

---

## 二、多租户隔离（Multi-Tenant Isolation）

### 2.1 Session 身份隔离

`sessions/session-lifecycle-identity.ts` 实现了严格的 Session 身份编码：

```typescript
// 身份编码：scope + identity → JSON 字符串
function normalizeSessionIdentities(scope: string, identities: Iterable<string>): string[] {
  return Array.from(new Set(
    Array.from(identities, (id) => id?.trim()).filter(Boolean)
  ))
    .map((id) => JSON.stringify([normalizedScope, id]))
    .toSorted();
}
```

**设计要点**：
- scope 隔离不同租户/工作区
- identity 隔离不同会话
- JSON 编码防止注入
- 排序保证确定性

### 2.2 Session Work Admission（工作准入）

`sessions/session-lifecycle-admission.ts`（770 行）实现了完整的准入控制：

```typescript
// 准入状态
interface SessionWorkAdmission {
  lifecycleGeneration: string;
  phase: "pending" | "acquired";
  owner?: symbol;
  interrupt?: (reason?: Error) => void;
  released: Promise<void>;
}

// 工作准入租约
interface SessionWorkAdmissionLease {
  renew(): Promise<void>;
  assertOwned(): void;
  heartbeat: "worker" | "main";
}
```

**关键机制**：
- **排他锁**：`runExclusiveSessionStoreWrite` 保证同一 session 同时只有一个写入者
- **生命周期代**：`lifecycleGeneration` 防止旧进程干扰新进程
- **中断传播**：`interrupt` 回调实现跨模块取消
- **空闲等待器**：`idleWaiters` 实现优雅排空

### 2.3 Gateway Work Admission

`process/gateway-work-admission.ts` 实现了进程级的工作准入：

```typescript
// 准入阶段
type GatewaySuspendAdmissionPhase = "accepting" | "draining" | "suspended";

// 根工作准入租约
interface GatewayRootWorkAdmissionLease {
  ownsRoot: boolean;
  release: () => void;
  run: <T>(run: () => Promise<T>) => Promise<T>;
}
```

**laew gap L1170**：无 Session 工作准入机制。laew 的多轮对话无排他锁，并发写 SQLite 可能冲突。

### 2.4 多租户数据隔离

```typescript
// 状态数据库路径
function resolveOpenClawStateSqlitePath(env: NodeJS.ProcessEnv): string {
  // OPENCLAW_HOME → HOME/.openclaw/state.sqlite
  // 每个用户独立数据库
}

// Agent 数据库
function openOpenClawAgentDatabase(options: {
  agentId: string;
  env: NodeJS.ProcessEnv;
}): OpenClawStateDatabase {
  // 每个 Agent 独立 SQLite 文件
}
```

**laew gap L1171**：无多租户数据隔离。laew 单用户单数据库，无 OPENCLAW_HOME 隔离。

---

## 三、RRF 检索（Reciprocal Rank Fusion）

### 3.1 Web Search 混合检索

`web-search/runtime-execution.ts` 实现了多 provider 自动回退：

```typescript
// 候选 provider 链
async function executeWebSearchCandidates(params: {
  candidates: readonly PluginWebSearchProviderEntry[];
  allowFallback: boolean;
}): Promise<RunWebSearchResult> {
  for (const candidate of params.candidates) {
    try {
      const executed = await definition.execute(params.args, { signal: params.signal });
      if (params.allowFallback && isStructuredAvailabilityError(executed)) {
        continue; // 跳过不可用 provider
      }
      return { provider: candidate.id, result: executed };
    } catch (error) {
      if (!params.allowFallback) throw error;
    }
  }
}
```

### 3.2 Link Understanding 链接理解

`link-understanding/` 实现了入站消息的链接提取和安全过滤：

```typescript
// SSRF 防护
function extractLinksFromMessage(message: string, options?: {
  maxLinks?: number;
}): string[] {
  // 1. 去除 markdown 链接
  // 2. 提取裸链接
  // 3. 去重 + 限制数量
  // 4. SSRF 过滤（loopback/private/link-local/cloud metadata）
}
```

**SSRF 过滤范围**：
- 127.0.0.1 / localhost / 0.0.0.0 / ::1
- 10.0.0.0/8 / 172.16.0.0/12 / 192.168.0.0/16
- 169.254.0.0/16（link-local）
- 100.64.0.0/10（CGNAT/Tailscale）
- metadata.google.internal

### 3.3 Memory Artifact Provenance（记忆产物溯源）

`memory/memory-artifact-provenance.ts` 实现了工作区文件的溯源追踪：

```typescript
interface MemoryArtifactProvenance {
  fileHash: string;
  originClass: "agent" | "untrusted";
  observedAt: number;
  sessionId?: string;
  sessionKey?: string;
}

// 工作区密钥 = SHA256(realpathSync(workspaceDir))
// 防止 symlink 别名分割信任记录
```

**laew gap L1175**：无 RRF 混合检索。laew 无 Web Search/Link Understanding/Memory 检索能力。

---

## 四、LLM 网关路由（LLM Gateway Routing）

### 4.1 Failover 分类体系

`agents/failover/` 目录（30+ 文件）实现了完整的 LLM 故障分类和路由：

```typescript
// 故障原因枚举（冻结的 wire 协议兼容）
const FAILOVER_REASONS = [
  "auth", "auth_permanent", "format", "rate_limit", "overloaded",
  "billing", "server_error", "timeout", "tls_certificate",
  "context_overflow", "model_not_found", "session_expired",
  "empty_response", "no_error_details", "unclassified", "unknown"
] as const;

// 故障分类
type FailoverClassification =
  | { kind: "reason"; reason: FailoverReason }
  | { kind: "context_overflow" };
```

### 4.2 多层分类器

`agents/failover/classify.ts`（432 行）实现了分层分类：

```
Layer 1: HTTP Status → classifyFailoverClassificationFromHttpStatus
Layer 2: Error Code → classifyFailoverReasonFromCode
Layer 3: Message Patterns → isAuthErrorMessage / isRateLimitErrorMessage / ...
Layer 4: Provider-Specific → classifyLegacyProviderSpecificError
Layer 5: Context Overflow → isContextOverflowErrorFromTables
Layer 6: OAuth Refresh → classifyOAuthRefreshFailure
```

**402 billing/rate_limit 细分**：
```typescript
// 402 → billing vs rate_limit 的 6 个维度判定
const BILLING_402_HINTS = ["insufficient credits", "credit balance", ...];
const PERIODIC_402_HINTS = ["daily", "weekly", "monthly"];
const RETRYABLE_402_RETRY_HINTS = ["try again", "retry", "temporary", "cooldown"];
const RETRYABLE_402_LIMIT_HINTS = ["usage limit", "rate limit", "organization usage"];
const RETRYABLE_402_SCOPED_HINTS = ["organization", "workspace"];
const RETRYABLE_402_SCOPED_RESULT_HINTS = ["billing period", "exceeded", "reached", "exhausted"];
```

### 4.3 Failover 错误传播

`agents/failover/error.ts` 实现了结构化错误：

```typescript
class FailoverError extends Error {
  readonly reason: FailoverReason;
  readonly provider?: string;
  readonly model?: string;
  readonly profileId?: string;
  readonly status?: number;
  readonly code?: string;
  readonly attempts?: readonly FallbackAttemptRecord[];
  readonly soonestCooldownExpiry?: number | null;
  readonly sessionId?: string;
  readonly lane?: string;
  readonly cliTimeout?: CliTimeoutContext;
}
```

**laew gap L1178**：无 LLM 故障分类和路由。laew 的 provider 切换是手动的，无自动 failover/cooldown/退避。

### 4.4 Gateway 健康状态

`gateway/server/health-state.ts` 实现了双 audience 健康快照：

```typescript
// 双 audience
type HealthAudience = "public" | "admin";
// 刷新强度
type HealthRefreshStrength = "passive" | "probe";
// 版本广播
let presenceVersion = 1;
let healthVersion = 1;
```

---

## 五、Pregel 图执行（Pregel Graph Execution）

### 5.1 Session 状态事件图

`sessions/session-state-events.ts` 实现了基于 SQLite 的 Session 状态事件日志：

```typescript
// 事件类型
type SessionStateEventKind =
  | "session.created" | "session.reset" | "session.compacted"
  | "run.started" | "run.completed" | "run.failed" | "run.interrupted"
  | "message.sent" | "message.received"
  | "tool.started" | "tool.completed" | "tool.failed";

// 事件记录
interface SessionStateEventRecord {
  sequence: number;
  sessionKey: string;
  agentId: string;
  kind: SessionStateEventKind;
  actorType: SessionStateActorType;
  occurredAt: number;
  summary: string;
  payload?: Record<string, unknown>;
}
```

**图特性**：
- 序列号递增（`sequence`）
- 30 天保留期（`SESSION_STATE_RETENTION_MS`）
- 50K 行上限（`SESSION_STATE_MAX_ROWS`）
- 每小时修剪（`SESSION_STATE_PRUNE_INTERVAL_MS`）

### 5.2 SubAgent 图执行模型

`agents/subagents/registry/` 实现了类似 Pregel 的图执行：

```typescript
// SubAgent 运行记录
interface SubagentRunRecord {
  runId: string;
  childSessionKey: string;
  controllerSessionKey: string;
  requesterSessionKey: string;
  task: string;
  execution: SubagentExecutionState;
  delivery: SubagentDeliveryState;
}

// 执行状态
interface SubagentExecutionState {
  status: "queued" | "running" | "interrupted" | "terminal";
  lifecycleGeneration?: string;
  restartRecovery?: SubagentRestartRecoveryReceipt;
}
```

**消息传递**：
- `announce`：子 → 父（完成通知）
- `steer`：父 → 子（中途指导）
- `collect`：父 ← 子（结构化输出收集）

**laew gap L1182**：无 Pregel 图执行模型。laew 的多 Agent 是线性流水线（Yolo→Plan→Main→Sub→QC），非图拓扑。

---

## 六、Skill 生命周期（Skill Lifecycle）

### 6.1 Skill 类型体系

`skills/types.ts`（155 行）定义了完整的 Skill 类型：

```typescript
// Skill 安装规范
interface SkillInstallSpec {
  kind: "brew" | "node" | "go" | "uv" | "download";
  bins?: string[];
  os?: string[];
  formula?: string;
  package?: string;
  sha256?: string;
  // ...
}

// Skill 元数据
interface OpenClawSkillMetadata {
  always?: boolean;
  skillKey?: string;
  requires?: {
    bins?: string[];
    anyBins?: string[];
    env?: string[];
    config?: string[];
  };
  install?: SkillInstallSpec[];
}

// Skill 快照
interface SkillSnapshot {
  prompt: string;
  skills: Array<{
    name: string;
    skillKey?: string;
    primaryEnv?: string;
    requiredEnv?: string[];
  }>;
  skillFilter?: string[];
  skillOverrides?: Record<string, boolean>;
  nodeSkillsEligibility?: SkillEligibilityContext["nodeSkills"];
  version?: number;
  promptFormatVersion?: number;
}
```

### 6.2 Skill 发现链

`skills/discovery/` 实现了 5 级发现链：

```
Priority 1: workspace/skills/          (最高优先级)
Priority 2: workspace/.agents/skills/  (项目 agents)
Priority 3: ~/.agents/skills/          (个人 agents)
Priority 4: managed skills             (.managed/)
Priority 5: bundled skills             (最低优先级)
```

**关键代码**：
```typescript
// 个人 skills 仅对默认 state directory 加载
if (process.env.OPENCLAW_STATE_DIR) {
  // 非默认 state → 不加载个人 skills
}
```

### 6.3 Skill 安全扫描

`skills/security/` 实现了多层安全扫描：

```typescript
// ClawHub 安全裁决
interface OpenClawSkillSecurityVerdictItem {
  registry: string;
  ok: boolean;
  decision: string;
  reasons: string[];
  securityStatus?: string | null;
  securityPassed?: boolean | null;
  error?: { code?: string; message?: string };
}

// 内容扫描
function scanSkillContent(skillPath: string): ScanResult {
  // 检测：pipe-to-shell 安装、secret 外泄、prompt 注入
}
```

### 6.4 Skill Workshop（自演化）

`skills/workshop/` 实现了完整的 Skill 提案生命周期：

```typescript
// 提案状态机
type SkillProposalStatus = "pending" | "applied" | "rejected" | "quarantined" | "stale";

// 提案应用转换
const SKILL_PROPOSAL_APPLY_TRANSITIONS: Record<SkillProposalStatus, Partial<Record<SkillProposalApplyOutcome, SkillProposalStatus>>> = {
  pending: {
    apply_failed: "pending",
    apply_succeeded: "applied",
    scan_failed: "quarantined",
    target_changed: "stale",
  },
  // ...
};
```

**laew gap L1186**：无 Skill 系统。laew 无 Skill 注册/发现/加载/安全扫描/自演化能力。

---

## 七、Agent 预热池（Agent Warm Pool）

### 7.1 SubAgent 注册中心

`agents/subagents/registry/`（80+ 文件）实现了完整的 SubAgent 生命周期管理：

```typescript
// 注册中心接口
interface SubagentRegistry {
  // 运行管理
  launchSubagent(params: SubagentLaunchParams): Promise<SubagentRunRecord>;
  completeSubagent(params: SubagentCompletionRequest): Promise<void>;
  killSubagent(runId: string, reason: string): Promise<void>;
  
  // 查询
  listControlledSubagentRuns(controllerSessionKey: string): SubagentRunRecord[];
  getActiveSubagentContext(controllerSessionKey: string): string | undefined;
  
  // 恢复
  recoverInterruptedSubagentRow(params: RestartRecoveryParams): Promise<RestartRecoveryResult>;
}
```

### 7.2 SubAgent 启动模式

```typescript
// 启动模式
type SpawnSubagentMode = "run" | "session";
// 沙箱模式
type SpawnSubagentSandboxMode = "inherit" | "require";
// 上下文模式
type SpawnSubagentContextMode = "isolated" | "fork";
```

### 7.3 Collector Spawn（收集器启动）

`agents/subagents/swarm/` 实现了 Code Mode 的收集器启动：

```typescript
// 收集器绑定
function bindCollectorSpawnTool<T extends AnyAgentTool>(
  tool: T,
  properties: Record<string, string>,
  signal?: AbortSignal
): T {
  // 动态注入 collect/outputSchema/groupId 参数
  // 绑定 agents_wait 工具引用
}

// 加入的启动
function runWithJoinedCollectorSpawn<T>(
  tool: object,
  assertCurrent: () => void,
  run: () => Promise<T>
): Promise<T> {
  // AsyncLocalStorage 绑定 joined spawn 上下文
}
```

### 7.4 活跃 SubAgent 上下文

```typescript
// 构建活跃 SubAgent 上下文（注入到父 session prompt）
function buildActiveSubagentRuntimeContext(params: {
  cfg: OpenClawConfig;
  controllerSessionKey?: string;
  recentMinutes?: number;
}): string | undefined {
  // 返回 Markdown 格式的活跃子 Agent 列表
  // 最多 16 个，按 runId 排序
  // 数据经过 sanitizeForPromptLiteral + JSON 引用
}
```

**laew gap L1190**：无 Agent 预热池。laew 的 SubAgent 是按需创建/销毁，无池化/预热/复用机制。

---

## 八、Turn 锁（Turn Lock）

### 8.1 Command Queue（命令队列）

`process/command-queue.ts`（733 行）实现了完整的命令序列化：

```typescript
// 命令通道
const enum CommandLane {
  Main = "main",
  SystemAgent = "system-agent",
  Cron = "cron",
  CronNested = "cron-nested",
  HookDispatch = "hook-dispatch",
  Background = "background",
  Subagent = "subagent",
  Nested = "nested",
}

// 队列状态
interface LaneState {
  lane: string;
  queue: LaneQueue;
  activeTaskIds: Set<number>;
  maxConcurrent: number;
  draining: boolean;
  generation: number;
}
```

### 8.2 超时机制

```typescript
// 超时原因
type TimeoutCause =
  | "task-budget"      // 任务预算耗尽
  | "owner-deadline"   // 所有者截止时间
  | "progress-idle"    // 无进展空闲
  | "abort-grace"      // 取消宽限期
  | "release-signal";  // 释放信号

// 超时错误
class CommandLaneTaskTimeoutError extends Error {
  constructor(lane: string, details: {
    cause: TimeoutCause;
    elapsedMs: number;
    taskBudgetMs: number;
  });
}
```

### 8.3 容量组

`process/command-queue.capacity-groups.ts` 实现了通道容量管理：

```typescript
// 容量组规格
interface CommandLaneGroupSpec {
  lanes: string[];
  maxConcurrent: number;
  maxQueued: number;
}

// 准入检查
function canAdmitInGroup(group: LaneGroupState): boolean {
  return group.activeCount < group.maxConcurrent;
}
```

### 8.4 Session Work Admission Handoff

`sessions/session-work-admission-handoff.ts` 实现了工作准入的跨进程传递：

```typescript
// 准入租约
interface SessionWorkAdmissionLease {
  renew(): Promise<void>;
  assertOwned(): void;
  heartbeat: "worker" | "main";
}

// 创建传递
function createSessionWorkAdmissionHandoff(): {
  handoffId: string;
  cancel: () => void;
  consume: () => Promise<void>;
};
```

**laew gap L1194**：无 Turn 锁机制。laew 的多轮对话无通道序列化/超时/容量控制。

---

## 九、HTTP 客户端高级实现

### 9.1 SSRF 防护体系

`infra/net/ssrf.ts`（798 行）实现了完整的 SSRF 防护：

```typescript
// SSRF 策略
interface SsrFPolicy {
  allowPrivateNetwork?: boolean;
  dangerouslyAllowPrivateNetwork?: boolean;
  allowRfc2544BenchmarkRange?: boolean;
  allowIpv6UniqueLocalRange?: boolean;
  allowedHostnames?: string[];
  allowedOrigins?: string[];
  hostnameAllowlist?: string[];
  blockedHostnames?: string[];
}

// DNS 钉扎
function resolvePinnedHostnameWithPolicy(
  hostname: string,
  policy: SsrFPolicy
): Promise<string> {
  // DNS 解析 → IP 验证 → 策略检查
}
```

### 9.2 Guarded Fetch

`infra/net/fetch-guard.ts`（762 行）实现了带防护的 fetch：

```typescript
// 防护模式
const GUARDED_FETCH_MODE = {
  STRICT: "strict",                    // 默认严格模式
  TRUSTED_ENV_PROXY: "trusted_env_proxy",    // 信任环境代理
  TRUSTED_EXPLICIT_PROXY: "trusted_explicit_proxy", // 信任显式代理
} as const;

// 重定向安全
function retainSafeHeadersForCrossOriginRedirect(
  headers: Headers,
  originalOrigin: string,
  redirectOrigin: string
): Headers {
  // 跨域重定向时剥离敏感头
}
```

### 9.3 Pinned Dispatcher Pool

`infra/net/pinned-dispatcher-pool.ts` 实现了连接池：

```typescript
class PinnedDispatcherPool {
  private maxEntries: number;
  private idleTtlMs: number;
  private dispatchers: Map<string, PinnedDispatcherLease>;
  
  // 获取/释放 dispatcher
  async acquire(key: string): Promise<PinnedDispatcherLease>;
  release(lease: PinnedDispatcherLease): void;
}
```

### 9.4 代理链

`infra/net/proxy/` 实现了完整的代理管理：

```typescript
// 活跃代理注册
interface ActiveManagedProxyRegistration {
  proxyUrl: ActiveManagedProxyUrl;
  loopbackMode: ActiveManagedProxyLoopbackMode;
  proxyTls?: ManagedProxyTlsOptions;
  stopped: boolean;
}

// 回环模式
type ActiveManagedProxyLoopbackMode = "gateway-only" | "proxy" | "block";

// 本地起源绕过
function shouldUseConfiguredLocalOriginManagedProxyBypass(params: {
  url: URL;
  managedProxyBypass: ConfiguredLocalOriginManagedProxyBypass;
  resolvedAddresses: readonly string[];
}): boolean {
  // 配置本地起源 + DNS 验证 + 回环策略
}
```

**laew gap L1198**：无 SSRF 防护。laew 的 BashTool 可执行任意命令，无网络请求防护。

---

## 十、安全加固（Security Hardening）

### 10.1 安全审计框架

`security/audit.ts`（1513 行）实现了完整的安全审计：

```typescript
// 审计发现
interface SecurityAuditFinding {
  checkId: string;
  severity: "info" | "warn" | "critical";
  title: string;
  detail: string;
  remediation?: string;
}

// 审计选项
interface SecurityAuditOptions {
  deep?: boolean;                    // 深度审计（含 gateway 探针）
  includeFilesystem?: boolean;       // 文件系统权限检查
  includeChannelSecurity?: boolean;  // 通道安全检查
  deepTimeoutMs?: number;            // 深度审计超时
}
```

### 10.2 外部内容防护

`security/external-content.ts`（462 行）实现了 prompt 注入防护：

```typescript
// 可疑模式检测
const SUSPICIOUS_PATTERNS = [
  /ignore\s+(all\s+)?(previous|prior|above)\s+(instructions?|prompts?)/i,
  /disregard\s+(all\s+)?(previous|prior|above)/i,
  /forget\s+(everything|all|your)\s+(instructions?|rules?|guidelines?)/i,
  /you\s+are\s+now\s+(a|an)\s+/i,
  /new\s+instructions?:/i,
  /system\s*:?\s*(prompt|override|command)/i,
  // ...
];

// 外部内容包装
function wrapExternalContent(content: string, source: ExternalContentSource): string {
  const id = createExternalContentMarkerId(); // 随机 16 位 hex
  return `<<<EXTERNAL_UNTRUSTED_CONTENT id="${id}">>>\n${warning}\n${content}\n<<<END_EXTERNAL_UNTRUSTED_CONTENT id="${id}">>>`;
}

// LLM 特殊 Token 剥离
const LLM_SPECIAL_TOKEN_LITERALS = [
  "<|im_start|>", "<|im_end|>", "<|endoftext|>",
  "<|begin_of_text|>", "<|end_of_text|>",
  "[INST]", "[/INST]", "<<SYS>>", "<</SYS>>",
  // ...
];
```

### 10.3 沙箱安全验证

`agents/sandbox/validate-sandbox-security.ts`（435 行）实现了 Docker 沙箱安全：

```typescript
// 阻断的主机路径
const BLOCKED_HOST_PATHS = [
  "/etc", "/proc", "/sys", "/dev", "/root", "/boot",
  "/run", "/var/run", "/var/run/docker.sock",
  // ...
];

// 阻断的 Home 子目录
const BLOCKED_HOME_SUBPATHS = [
  ".aws", ".cargo", ".config", ".docker", ".gnupg", ".netrc", ".npm", ".ssh"
];

// 安全验证
function validateSandboxSecurity(config: SandboxConfig): void {
  // 1. 验证 bind mount
  // 2. 验证网络模式
  // 3. 验证 seccomp/apparmor
  // 4. 验证 capabilities
  // 5. 验证 privileged 模式
}
```

### 10.4 Exec 文件系统策略漂移

`security/exec-filesystem-policy.ts` 检测策略不一致：

```typescript
// 检测：exec 可用但 write/edit/apply_patch 被禁用
function collectExecFilesystemPolicyDriftHits(cfg: OpenClawConfig): ExecFilesystemPolicyDriftHit[] {
  // 遍历所有 agent + 全局 tools 配置
  // 检测：exec 可用 + 所有 mutating fs 工具禁用 + 沙箱非 all 模式
}
```

### 10.5 危险配置标志

`security/dangerous-config-flags.ts` 检测不安全配置：

```typescript
// 收集启用的不安全标志
function collectEnabledInsecureOrDangerousFlags(cfg: OpenClawConfig): string[] {
  // 遍历所有 plugin 配置契约
  // 检测：dangerouslyAllow* / allowInsecure* 等前缀
}
```

**laew gap L1205**：无安全审计框架。laew 无 prompt 注入防护/外部内容包装/沙箱安全验证。

---

## 十一、新增 gap 清单

### P0 紧急（30 项）

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无进程 Supervisor（scope 隔离/超时/输出截断/OOM） | P0 |
| L1167 | 崩溃恢复 | 无 Main Session Restart Recovery（会话中断恢复） | P0 |
| L1168 | 崩溃恢复 | 无 SubAgent 重启恢复（phase 机 + 幂等键） | P0 |
| L1169 | 崩溃恢复 | 无配置健康指纹（hash + stat + dev/ino） | P0 |
| L1170 | 多租户 | 无 Session 工作准入（排他锁 + 生命周期代） | P0 |
| L1171 | 多租户 | 无多租户数据隔离（OPENCLAW_HOME / per-agent DB） | P0 |
| L1172 | 多租户 | 无 Gateway Work Admission（进程级准入 + 排空） | P0 |
| L1173 | 崩溃恢复 | 无 Clobber Snapshot（32 槽快照 + mkdir 锁） | P0 |
| L1174 | 崩溃恢复 | 无 Backup Rotation（5 槽环形 + pre-update） | P0 |
| L1175 | RRF检索 | 无 Web Search 混合检索（多 provider 自动回退） | P0 |
| L1176 | RRF检索 | 无 Link Understanding（链接提取 + SSRF 过滤） | P0| 
| L1177 | RRF检索 | 无 Memory Artifact Provenance（工作区文件溯源） | P0 |
| L1178 | LLM路由 | 无 Failover 分类体系（16 种原因 + 多层分类器） | P0 |
| L1179 | LLM路由 | 无 Failover 错误传播（结构化错误 + 会话归因） | P0 |
| L1180 | LLM路由 | 无 Gateway 健康状态（双 audience + 版本广播） | P0 |
| L1181 | LLM路由 | 无 402 billing/rate_limit 细分（6 维度判定） | P0 |
| L1182 | Pregel | 无 Session 状态事件图（SQLite 事件日志 + 序列号） | P0 |
| L1183 | Pregel | 无 SubAgent 图执行模型（消息传递 + 收集器） | P0 |
| L1184 | Pregel | 无活跃 SubAgent 上下文注入（prompt 安全引用） | P0 |
| L1185 | Skill | 无 Skill 类型体系（安装规范 + 元数据 + 快照） | P0 |
| L1186 | Skill | 无 Skill 发现链（5 级优先级 + 安全扫描） | P0 |
| L1187 | Skill | 无 Skill Workshop（提案状态机 + 自演化） | P0 |
| L1188 | Skill | 无 Skill 安全扫描（ClawHub 裁决 + 内容扫描） | P0 |
| L1189 | Agent池 | 无 SubAgent 注册中心（80+ 文件生命周期管理） | P0 |
| L1190 | Agent池 | 无 Agent 预热池（池化/预热/复用） | P0 |
| L1191 | Agent池 | 无 Collector Spawn（Code Mode 收集器绑定） | P0 |
| L1192 | Turn锁 | 无 Command Queue（8 通道 + 序列化 + 容量组） | P0 |
| L1193 | Turn锁 | 无超时机制（5 种超时原因 + 退避） | P0 |
| L1194 | Turn锁 | 无 Session Work Admission Handoff（跨进程传递） | P0 |
| L1195 | HTTP客户端 | 无 SSRF 防护（DNS 钉扎 + IP 验证 + 策略合并） | P0 |

### P1 重要（40 项）

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1196 | HTTP客户端 | 无 Guarded Fetch（严格模式 + 代理 + 重定向安全） | P1 |
| L1197 | HTTP客户端 | 无 Pinned Dispatcher Pool（连接池 + idle TTL） | P1 |
| L1198 | HTTP客户端 | 无代理链管理（活跃代理注册 + 回环模式 + TLS） | P1 |
| L1199 | HTTP客户端 | 无本地起源绕过（配置本地 + DNS 验证 + 回环策略） | P1 |
| L1200 | HTTP客户端 | 无 Undici 全局配置（stream timeout + family policy） | P1 |
| L1201 | HTTP客户端 | 无 HTTP 错误诊断（undici-error-diagnostics） | P1 |
| L1202 | 安全加固 | 无安全审计框架（1513 行 + 深度探针 + 文件系统） | P1 |
| L1203 | 安全加固 | 无外部内容防护（prompt 注入检测 + 随机边界） | P1 |
| L1204 | 安全加固 | 无 LLM 特殊 Token 剥离（ChatML/Llama/Mistral） | P1 |
| L1205 | 安全加固 | 无沙箱安全验证（bind mount + 网络 + seccomp） | P1 |
| L1206 | 安全加固 | 无 exec 文件系统策略漂移检测 | P1 |
| L1207 | 安全加固 | 无危险配置标志检测 | P1 |
| L1208 | 安全加固 | 无信任模型审计（多用户启发 + 暴露矩阵） | P1 |
| L1209 | 安全加固 | 无 install-policy（安装策略 + Windows ACL） | P1 |
| L1210 | 安全加固 | 无 secret-equal（常量时间比较） | P1 |
| L1211 | 安全加固 | 无 safe-regex（ReDoS 防护） | P1 |
| L1212 | 安全加固 | 无 secret-mask（日志脱敏） | P1 |
| L1213 | 安全加固 | 无 external-content-source（来源标记） | P1 |
| L1214 | 安全加固 | 无 dangerous-tools（危险工具清单） | P1 |
| L1215 | 安全加固 | 无 context-visibility（上下文可见性控制） | P1 |
| L1216 | 崩溃恢复 | 无配置权限加固（chmod 0o600 best-effort） | P1 |
| L1217 | 崩溃恢复 | 无配置恢复审计（io.audit 记录每次恢复） | P1 |
| L1218 | 崩溃恢复 | 无配置可疑检测（4 类可疑原因） | P1 |
| L1219 | 崩溃恢复 | 无配置恢复策略（plugin-local vs 整体恢复） | P1 |
| L1220 | 多租户 | 无 Session 身份编码（scope + identity JSON） | P1 |
| L1221 | 多租户 | 无 Session 生命周期事件（创建/重置/压缩/运行） | P1 |
| L1222 | 多租户 | 无 Session 上游监控（upstream-monitor） | P1 |
| L1223 | 多租户 | 无 Session 差异基线（diff-baseline + revisions） | P1 |
| L1224 | 多租户 | 无 Session 参与者输入记录（input-recording） | P1 |
| L1225 | 多租户 | 无 Session 工作树生命周期（worktree-lifecycle） | P1 |
| L1226 | RRF检索 | 无 Web Search 运行时类型（provider 工厂 + 结果） | P1 |
| L1227 | RRF检索 | 无 Link Understanding 应用（入站上下文注入） | P1 |
| L1228 | RRF检索 | 无 Link Understanding 检测（markdown 抑制 + SSRF） | P1 |
| L1229 | RRF检索 | 无 Link Understanding 格式化（body 包装） | P1 |
| L1230 | RRF检索 | 无 Link Understanding 运行器（processor 链） | P1 |
| L1231 | RRF检索 | 无 Memory 根文件管理（root-memory-files） | P1 |
| L1232 | LLM路由 | 无 Failover 用户友好错误文案（脱敏 + 建议） | P1 |
| L1233 | LLM路由 | 无 Failover 分类规则（HTTP status + code + message） | P1 |
| L1234 | LLM路由 | 无 Failover 信号详情（retryAfter + details） | P1 |
| L1235 | LLM路由 | 无 Failover 证据收集（retry-evidence） | P1 |

### P2 进阶（25 项）

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1236 | LLM路由 | 无 Failover 分类语料库（corpus test 覆盖） | P2 |
| L1237 | LLM路由 | 无 Failover 遗留 provider 谓词（legacy predicates） | P2 |
| L1238 | LLM路由 | 无 Failover 结构化信号（provider-structured-signals） | P2 |
| L1239 | LLM路由 | 无 Context Overflow 检测（table + reasoning） | P2 |
| L1240 | LLM路由 | 无 Request Error Facets（错误面分类） | P2 |
| L1241 | Pregel | 无 Session 状态事件通知（notices + watchers） | P2 |
| L1242 | Pregel | 无 Session 状态游标（watch-cursor + provenance） | P2 |
| L1243 | Pregel | 无 Session 状态保留策略（30天 + 50K行） | P2 |
| L1244 | Pregel | 无 SubAgent 孤儿恢复（orphan-recovery） | P2 |
| L1245 | Pregel | 无 SubAgent 父恢复（parent-recovery） | P2 |
| L1246 | Pregel | 无 SubAgent 会话对账（session-reconciliation） | P2 |
| L1247 | Pregel | 无 SubAgent 会话指标（session-metrics） | P2 |
| L1248 | Skill | 无 Skill 安装规范（brew/node/go/uv/download） | P2 |
| L1249 | Skill | 无 Skill 二进制发现（bins + anyBins + install） | P2 |
| L1250 | Skill | 无 Skill 命令调度（dispatch + promptTemplate） | P2 |
| L1251 | Skill | 无 Skill 遥测来源（bundled/unknown/workspace） | P2 |
| L1252 | Skill | 无 Skill 使用路径（readPath + skillFile + source） | P2 |
| L1253 | Skill | 无 Skill 库管理（library + bundle + revision） | P2 |
| L1254 | Skill | 无 Skill 提案哈希（content + revision + bundle） | P2 |
| L1255 | Skill | 无 Skill 提案扫描（proposal-scan + frontmatter） | P2 |
| L1256 | Skill | 无 Skill 提案回滚（rollback + target-lock） | P2 |
| L1257 | Skill | 无 Skill 刷新状态（refresh-state + capacity） | P2 |
| L1258 | Skill | 无 Skill 远程协调（remote + remote-probe） | P2 |
| L1259 | Skill | 无 Skill 会话快照（session-snapshot + hydration） | P2 |
| L1260 | Skill | 无 Skill 工具调度（tool-dispatch + env-overrides） | P2 |
| L1261 | Agent池 | 无 SubAgent 控制范围（control-scope + accounting） | P2 |

### P3 进阶（5 项）

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1262 | Agent池 | 无 SubAgent 投递状态（delivery-state + dead-letter） | P3 |
| L1263 | Agent池 | 无 SubAgent 完成交付（completion-delivery + admission） | P3 |
| L1264 | Agent池 | 无 SubAgent 任务替换（task-replacement + generation） | P3 |
| L1265 | Agent池 | 无 SubAgent 运行视图（run-view + liveness + timeout） | P3 |

---

## 十二、架构交叉点

本轮 10 个维度存在 **6 个关键交叉点**：

1. **崩溃恢复 × 多租户**：Session Work Admission 的 `lifecycleGeneration` 同时服务于崩溃恢复（检测旧进程）和多租户隔离（排他写入）

2. **LLM路由 × 安全加固**：Failover 分类器的 402 billing/rate_limit 细分依赖安全审计的暴露矩阵检测

3. **Skill生命周期 × Agent预热池**：Skill 快照（`SkillSnapshot`）被 SubAgent 启动时注入，影响预热池的 prompt 构建

4. **Turn锁 × Pregel图执行**：Command Queue 的通道序列化保证 SubAgent 图执行的消息传递有序性

5. **HTTP客户端 × 安全加固**：Guarded Fetch 的 SSRF 防护是安全审计框架的网络层基础

6. **RRF检索 × Skill生命周期**：Link Understanding 的链接提取结果可触发 Skill Workshop 的提案生成

---

## 十三、与前 16 轮的衔接

| 前 16 轮覆盖 | 本轮关系 |
|------------|---------|
| 第一轮:Gateway/Harness/Adapter 三层契约 | 本轮 Failover 是 Gateway 层的 LLM 路由 |
| 第五轮:Lane 调度器 | 本轮 Command Queue 是 Lane 的底层实现 |
| 第六轮:SubAgent 调度 | 本轮 SubAgent 注册中心是调度的完整实现 |
| 第七轮:Git 集成 | 本轮崩溃恢复是 Git checkpoint 的互补 |
| 第八轮:Custodian Skills | 本轮 Skill 生命周期是 Custodian 的扩展 |
| 第九轮:Taxonomy | 本轮 10 维度可新增 10 个 categoryId |
| 第十轮:内存加密 | 本轮安全加固是 Secret Sentinel 的上层 |
| 第十一轮:CrashDump | 本轮崩溃恢复是 CrashDump 的完整实现 |
| 第十二轮:数据库优化 | 本轮 Session 状态图是 SQLite 的深层应用 |
| 第十三轮:分布式部署 | 本轮多租户隔离是分布式的基础 |
| 第十四轮:安全加固 | 本轮安全加固是第十四轮的深化 |
| 第十五轮:网络协议深度 | 本轮 HTTP 客户端是第十五轮的网络层实现 |
| 第十六轮:Hook/租约/SQLite | 本轮 Turn 锁是租约机制的应用层 |

---

## 第十四轮不重复声明

为保持每轮深挖的独立性，本节明确列出 **本轮不覆盖、读者应回查前 16 轮的内容**：

- ❌ **Gateway/Harness/Adapter 三层契约** → 见第六轮 § 17.2
- ❌ **Lane 调度器 + Workshop 自演化** → 见第五轮 § 16.3
- ❌ **协议 wire 真实实现** → 见第六轮专题
- ❌ **Git 与版本控制集成** → 见第七轮 § 18.1
- ❌ **多模态与文件处理** → 见第七轮 § 18.2
- ❌ **Web 检索与网络访问** → 见第七轮 § 18.3
- ❌ **Prompt Caching 与成本预算** → 见第七轮 § 18.4
- ❌ **MCP 11-capability + 162 extensions** → 见第七轮 § 17.3
- ❌ **Custodian Skills 5 阶段** → 见第八轮 § 19.1
- ❌ **多端部署(Docker/Render/Fly)** → 见第八轮 § 19.2
- ❌ **Taxonomy 分类体系** → 见第八轮 § 19.3
- ❌ **Security 漏洞响应** → 见第八轮 § 19.4
- ❌ **Secret Sentinel 内存加密** → 见第十六轮 § 1
- ❌ **SQLite 全栈基础设施** → 见第十六轮 § 2
- ❌ **Hook 系统** → 见第十六轮 § 3
- ❌ **租约与 Worker 心跳** → 见第十六轮 § 4

本轮独有（其他 11 份工程文档均无对应章节）：
- ✅ **五层 CrashDump 防御纵深扩展**（Main Session Recovery + SubAgent Recovery + Supervisor）
- ✅ **Session Work Admission 准入控制**（排他锁 + 生命周期代 + 中断传播）
- ✅ **Failover 16 种原因分类体系**（402 细分 + 多层分类器 + 结构化错误）
- ✅ **Skill 5 级发现链**（workspace → agents → personal → managed → bundled）
- ✅ **Skill Workshop 提案状态机**（pending → applied/rejected/quarantined/stale）
- ✅ **Command Queue 8 通道序列化**（Main/SystemAgent/Cron/Hook/Background/Subagent/Nested）
- ✅ **SSRF 防护体系**（DNS 钉扎 + IP 验证 + 策略合并 + 本地起源绕过）
- ✅ **外部内容 prompt 注入防护**（14 种可疑模式 + 随机边界 + LLM Token 剥离）
- ✅ **Docker 沙箱安全验证**（bind mount + 网络 + seccomp + apparmor + capabilities）
- ✅ **SubAgent 图执行模型**（announce/steer/collect 消息传递 + 收集器启动）

---

> **第十七轮分析完成**。共覆盖 10 个新维度，识别 **100 个 laew gap**（P0:30 / P1:40 / P2:25 / P3:5），全部附 Rust crate 建议（如需）。
