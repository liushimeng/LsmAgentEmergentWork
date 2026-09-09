# 专题-第十七轮-opencode-深度分析

> **调研日期**：2026-09-09
> **调研范围**：opencode 崩溃恢复与取证 / 多租户隔离 / RRF检索 / LLM网关路由 / Pregel图执行 / Skill生命周期 / Agent预热池 / Turn锁 / HTTP客户端高级实现 / 安全加固
> **本轮新增 gap**：L1166-L1221

---

## 一、崩溃恢复与取证（Crash Recovery & Forensics）

### 1.1 全局生命周期管理（server/global-lifecycle.ts, 35 行）

opencode 通过 `GlobalLifecycle` 模块管理进程级资源的优雅关闭：

```typescript
export const disposeAllInstancesAndEmitGlobalDisposed = Effect.fn("Server.disposeAllInstancesAndEmitGlobalDisposed")(
  function* (options?: { swallowErrors?: boolean }) {
    const store = yield* InstanceStore.Service
    yield* Effect.gen(function* () {
      yield* options?.swallowErrors
        ? store.disposeAll().pipe(Effect.catchCause((cause) => Effect.logWarning("global disposal failed", { cause })))
        : store.disposeAll()
      yield* emitGlobalDisposed
    }).pipe(Effect.uninterruptible)
  },
)
```

**设计要点**：
- `Effect.uninterruptible` 保证关闭过程不被中断
- `swallowErrors` 选项允许批量关闭时忽略单个实例的失败
- 通过 `GlobalBus.emit("event", ...)` 广播 `Disposed` 事件

### 1.2 实例级资源回收（effect/instance-registry.ts, 20 行）

```typescript
const disposers = new Set<(directory: string) => Promise<void>>()

export function registerDisposer(disposer: (directory: string) => Promise<void>) {
  disposers.add(disposer)
  return () => { disposers.delete(disposer) }
}

export async function disposeInstance(directory: string) {
  await Promise.allSettled([...disposers].map((disposer) => disposer(directory)))
}
```

**机制**：全局 `disposers` Set 收集每个实例的清理函数，`disposeInstance` 并行执行所有清理器。`InstanceState` 通过 `registerDisposer` 注册缓存失效回调。

### 1.3 Session 重试策略（session/retry.ts, 312 行）

opencode 实现了完整的指数退避重试机制：

```typescript
export const RETRY_INITIAL_DELAY = 2000
export const RETRY_BACKOFF_FACTOR = 2
export const RETRY_JITTER_FACTOR = 0.25
export const RETRY_MAX_DELAY_NO_HEADERS = 30_000
export const RETRY_MAX_DELAY = 2_147_483_647
export const RETRY_MAX_RETRIES = 5

const RETRYABLE_MESSAGE_PATTERNS = [
  /429|500|502|503|504|524/i,
  /rate increased too quickly|rate limit|rate-limit|rate_limit|too many requests/i,
  /overloaded|service unavailable|service_unavailable|service-unavailable|internal error|internal_error|internal server error|server error|server_error|server-error|provider returned error|provider_returned_error|provider-returned-error/i,
  /terminated|fetch failed|failed to fetch|network[-_\s]error|upstream connect|connection error|connection refused|connection lost|socket connection was closed|socket hang up|reset before headers|getaddrinfo|enotfound|eai_again|econnrefused|econnreset|etimedout/i,
  /^timeout$|\b(?:request|response|connection|network|stream|read) (?:timeout|timed out|time out)\b/i,
  /try your request again|retry your request|resource exhausted|resource_exhausted/i,
  /\btry again (?:later|in\b)|\b(?:currently|temporarily) at capacity\b/i,
]
```

**Retry Policy 工厂**：
```typescript
export function policy(opts: {
  provider: string
  parse: (error: unknown) => Err
  set: (input: { attempt: number; message: string; action?: Retryable["action"]; next: number }) => Effect.Effect<void>
}) {
  return Schedule.fromStepWithMetadata(
    Effect.succeed((meta: Schedule.InputMetadata<unknown>) => {
      const error = opts.parse(meta.input)
      const retry = retryable(error, opts.provider)
      if (!retry) return Cause.done(meta.attempt)
      if (meta.attempt > RETRY_MAX_RETRIES) return Cause.done(meta.attempt)
      return Effect.gen(function* () {
        const wait = delay(meta.attempt, SessionV1.APIError.isInstance(error) ? error : undefined)
        const now = yield* Clock.currentTimeMillis
        yield* opts.set({ attempt: meta.attempt, message: retry.message, action: retry.action, next: now + wait })
        return [meta.attempt, Duration.millis(wait)] as [number, Duration.Duration]
      })
    }),
  )
}
```

**关键设计**：
- 支持 `retry-after-ms` / `retry-after` 响应头解析
- 区分 `free_tier_limit` / `account_rate_limit` 两种限流场景
- Context Overflow 错误不重试
- 通过 `Schedule.fromStepWithMetadata` 实现 Effect 原生重试调度

### 1.4 快照与回滚系统（snapshot/index.ts, 807 行）

opencode 实现了基于 Git 的快照系统，支持完整的 checkpoint/restore：

```typescript
export interface Interface {
  readonly init: () => Effect.Effect<void>
  readonly cleanup: () => Effect.Effect<void>
  readonly track: () => Effect.Effect<string | undefined>
  readonly patch: (hash: string) => Effect.Effect<Patch>
  readonly restore: (snapshot: string) => Effect.Effect<void>
  readonly revert: (patches: Patch[]) => Effect.Effect<void>
  readonly diff: (hash: string) => Effect.Effect<string>
  readonly diffFull: (from: string, to: string) => Effect.Effect<FileDiff[]>
}
```

**核心机制**：
- 每个项目有独立的 git repo：`Global.Path.data/snapshot/{project.id}/{Hash.fast(worktree)}`
- `track()` 创建 commit 快照，返回 hash
- `restore(hash)` 通过 `git checkout` 恢复到指定快照
- `revert(patches)` 撤销指定 patch 涉及的文件的变更
- `diffFull(from, to)` 计算两个快照间的完整 diff（含 binary 检测）
- 使用 `Semaphore` 保证同一目录的并发安全
- 自动清理循环：每小时执行一次，延迟 1 分钟启动

### 1.5 Session 回滚（session/revert.ts, 160 行）

```typescript
export interface Interface {
  readonly revert: (input: RevertInput) => Effect.Effect<Session.Info, Session.BusyError>
  readonly unrevert: (input: { sessionID: SessionID }) => Effect.Effect<Session.Info, Session.BusyError>
  readonly cleanup: (session: Session.Info) => Effect.Effect<void>
}
```

**机制**：
- `revert` 回滚到指定 message/part，恢复文件状态到快照
- `unrevert` 撤销回滚操作
- `cleanup` 删除回滚标记的消息/part
- 通过 `SessionRunState.assertNotBusy` 保证回滚时 Session 空闲

### 1.6 中断恢复（session/processor.ts — halt/cleanup）

```typescript
const halt = Effect.fn("SessionProcessor.halt")(function* (e: unknown) => {
  yield* Effect.logError("process", {
    "session.id": input.sessionID,
    messageID: input.assistantMessage.id,
    error: errorMessage(e),
    stack: e instanceof Error ? e.stack : undefined,
  })
  const error = parse(e)
  if (SessionV1.ContextOverflowError.isInstance(error)) {
    if ((yield* config.get()).compaction?.auto === false && !ctx.assistantMessage.summary) {
      ctx.assistantMessage.error = error
      ctx.assistantMessage.finish = "error"
      yield* events.publish(Session.Event.Error, { sessionID: ctx.sessionID, error })
      yield* status.set(ctx.sessionID, { type: "idle" })
      return
    }
    ctx.needsCompaction = true
    yield* events.publish(Session.Event.Error, { sessionID: ctx.sessionID, error })
    return
  }
  ctx.assistantMessage.error = error
  yield* events.publish(Session.Event.Error, { sessionID: ctx.assistantMessage.sessionID, error: ctx.assistantMessage.error })
  yield* status.set(ctx.sessionID, { type: "idle" })
})
```

**设计要点**：
- Context Overflow 触发自动 compaction（除非用户禁用）
- 所有错误通过 `EventV2Bridge` 发布到事件总线
- `cleanup()` 标记中断的 tool 为 `interrupted`，防止孤儿 tool call

### 1.7 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 panic hook / CrashDump 机制 | P0 |
| L1167 | 崩溃恢复 | 无 watchdog 进程监控 | P1 |
| L1168 | 崩溃恢复 | 无自动 restart loop | P1 |
| L1169 | 取证 | 无结构化错误报告输出 | P1 |
| L1170 | 快照 | 无增量快照（仅全量 commit） | P2 |

---

## 二、多租户隔离（Multi-Tenant Isolation）

### 2.1 账户系统（account/account.ts, 481 行）

opencode 实现了完整的 OAuth Device Code 认证流程：

```typescript
export interface Interface {
  readonly active: () => Effect.Effect<Option.Option<Info>, AccountError>
  readonly activeOrg: () => Effect.Effect<Option.Option<ActiveOrg>, AccountError>
  readonly list: () => Effect.Effect<Info[], AccountError>
  readonly orgsByAccount: () => Effect.Effect<readonly AccountOrgs[], AccountError>
  readonly remove: (accountID: AccountID) => Effect.Effect<void, AccountError>
  readonly use: (accountID: AccountID, orgID: OrgID) => Effect.Effect<void, AccountError>
  readonly orgs: (accountID: AccountID) => Effect.Effect<Org[], AccountError>
  readonly config: (accountID: AccountID, orgID: OrgID) => Effect.Effect<Option.Option<Record<string, unknown>>, AccountError>
  readonly login: (server: string) => Effect.Effect<Login, AccountError>
  readonly poll: (input: Login) => Effect.Effect<PollResult, AccountError>
}
```

**关键设计**：
- 多账户支持：`list()` 返回所有账户，`use(accountID, orgID)` 切换当前账户
- Org 隔离：每个账户可属于多个 Org，`activeOrg()` 返回当前激活的 Org
- Token 自动刷新：`eagerRefreshThreshold = 5 minutes`，在过期前 5 分钟自动刷新
- Device Code 流程：`login()` 获取 device_code/user_code/verification_uri，`poll()` 轮询 token 获取

### 2.2 工作空间隔离（control-plane/workspace.ts, 966 行）

```typescript
export interface Interface {
  readonly create: (input: CreateInput) => Effect.Effect<Info, CreateError>
  readonly sessionWarp: (input: SessionWarpInput) => Effect.Effect<void, SessionWarpError>
  readonly list: () => Effect.Effect<Info[]>
  readonly syncList: () => Effect.Effect<SyncStatus[]>
  readonly get: (id: WorkspaceV2.ID) => Effect.Effect<Info, WorkspaceNotFoundError>
  readonly remove: (id: WorkspaceV2.ID) => Effect.Effect<Info>
  readonly status: () => Effect.Effect<ConnectionStatus[]>
  readonly isSyncing: (workspaceID: WorkspaceV2.ID) => Effect.Effect<boolean>
  readonly waitForSync: (workspaceID: WorkspaceV2.ID, state: Record<string, number>, signal?: AbortSignal, timeout?: number) => Effect.Effect<void, WaitForSyncError>
  readonly startWorkspaceSyncing: (projectID: ProjectV2.ID) => Effect.Effect<void>
}
```

**隔离机制**：
- 每个 Workspace 有独立的 `directory`、`branch`、`projectID`
- `sessionWarp` 支持跨 Workspace 的 Session 迁移（含变更复制）
- `waitForSync` 实现同步栅栏，等待所有事件序列达到指定状态
- `WorkspaceContext` 通过 `LocalContext`（类似 AsyncLocalStorage）实现请求级隔离

### 2.3 位置服务隔离（server/location.ts + middleware/session-location.ts）

```typescript
export function ref(request: HttpServerRequest.HttpServerRequest): Location.Ref {
  const query = new URL(request.url, "http://localhost").searchParams
  const workspaceID = query.get("location[workspace]") || request.headers["x-opencode-workspace"]
  const directory = query.get("location[directory]") ||
    (request.headers["x-opencode-directory"] ? decode(request.headers["x-opencode-directory"]) : process.cwd())
  return Location.Ref.make({
    directory: AbsolutePath.make(directory),
    workspaceID: workspaceID ? WorkspaceV2.ID.make(workspaceID) : undefined,
  })
}
```

**设计要点**：
- `LocationMiddleware` 从 HTTP 请求头/查询参数提取 `workspaceID` + `directory`
- `SessionLocationMiddleware` 从 sessionID 查询对应的 directory + workspaceID
- `LocationServiceMap` 为每个 Location 创建独立的服务实例（per-project 隔离）

### 2.4 实例级隔离（project/instance-store.ts, 450 行）

```typescript
export interface Interface {
  readonly load: (input: LoadInput) => Effect.Effect<InstanceContext>
  readonly reload: (input: LoadInput) => Effect.Effect<InstanceContext>
  readonly dispose: (ctx: InstanceContext) => Effect.Effect<void>
  readonly disposeDirectory: (directory: string) => Effect.Effect<void>
  readonly disposeAll: () => Effect.Effect<void>
  readonly provide: <A, E, R>(input: LoadInput, effect: Effect.Effect<A, E, R>) => Effect.Effect<A, E, R>
}
```

**机制**：
- 每个 directory 对应一个 `InstanceContext`（directory + worktree + project）
- `load()` 使用 `Deferred` 保证同一 directory 只 boot 一次
- `reload()` 先 dispose 旧实例再重新 boot
- `InstanceState` 使用 `ScopedCache` 按 directory 缓存服务实例

### 2.5 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1171 | 多租户 | 无细粒度 RBAC（仅 Org 级别） | P1 |
| L1172 | 多租户 | 无资源配额限制 | P1 |
| L1173 | 多租户 | 无审计日志 | P1 |
| L1174 | 隔离 | 无进程级沙箱隔离 | P0 |
| L1175 | 隔离 | 无网络隔离 | P1 |

---

## 三、RRF检索（Reciprocal Rank Fusion）

### 3.1 检索机制现状

opencode **未实现 RRF 混合检索**。其检索能力主要依赖：

- **Ripgrep**：代码搜索（grep/glob）
- **FileSystemSearch**：文件系统搜索
- **WebSearchTool**：外部 Web 搜索
- **MCPCatalog**：MCP 资源检索

### 3.2 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1176 | 检索 | 无 RRF 混合检索 | P1 |
| L1177 | 检索 | 无向量检索 | P1 |
| L1178 | 检索 | 无语义搜索 | P1 |
| L1179 | 检索 | 无 BM25 + Vector 融合排序 | P2 |
| L1180 | 检索 | 无检索结果重排序 | P2 |

---

## 四、LLM网关路由（LLM Gateway Routing）

### 4.1 Provider 系统（provider/provider.ts, 2068 行）

opencode 实现了完整的 Provider 管理系统：

```typescript
export interface Interface {
  readonly getLanguage: (model: Model) => Effect.Effect<LanguageModelV3>
  readonly getProvider: (providerID: ProviderV2.ID) => Effect.Effect<ProviderV2.Info>
  readonly getModel: (providerID: ProviderV2.ID, modelID: ModelV2.ID) => Effect.Effect<Model>
  readonly getSmallModel: (providerID: ProviderV2.ID) => Effect.Effect<Model>
  readonly listProviders: () => Effect.Effect<ProviderV2.Info[]>
  readonly listModels: (providerID: ProviderV2.ID) => Effect.Effect<Model[]>
}
```

**关键设计**：
- 16+ 内置 Provider（Anthropic, OpenAI, Google, Bedrock, Azure, xAI, GitHub Copilot 等）
- 每个 Provider 有独立的 SDK 加载器（`BUNDLED_PROVIDERS`）
- 支持自定义 Provider（通过 Plugin 系统）
- `getModel()` 返回标准化的 `Provider.Model` 对象

### 4.2 模型路由（core/session/runner/model.ts, 200 行）

```typescript
export const resolve = (session: SessionSchema.Info, model: ModelV2.Info, credential?: Credential.Value) =>
  withVariant(model, session.model?.variant).pipe(Effect.flatMap((model) => fromCatalogModel(model, credential)))

export const fromCatalogModel = (
  model: ModelV2.Info,
  credential?: Credential.Value,
): Effect.Effect<Model, UnsupportedApiError> => {
  const key = apiKey(resolved, credential)
  if (resolved.api.type === "aisdk" && resolved.api.package === "@ai-sdk/openai") {
    return Effect.succeed(
      withDefaults(resolved, OpenAIResponses.route)
        .with({ auth: key === undefined ? Auth.none : Auth.bearer(key) })
        .model({ id: resolved.api.id }),
    )
  }
  // ... 其他 Provider 路由
}
```

**路由逻辑**：
- `withVariant()` 应用模型变体（variant）覆盖 headers/body
- `fromCatalogModel()` 根据 `api.package` 选择对应的 Route
- `withDefaults()` 注入默认的 endpoint/headers/http body/limits

### 4.3 Provider Transform（provider/transform.ts, 1890 行）

```typescript
export function normalizeMessages(msgs: ModelMessage[], model: Provider.Model, _options: Record<string, unknown>): ModelMessage[] {
  // 1. Sanitize surrogates
  // 2. Anthropic: 过滤空 content、移除空 text/reasoning parts
  // 3. Bedrock: 特定转换
  // 4. OpenAI: reasoning_content 处理
  // 5. Gemini: thoughtSignature 处理
}

export function schema(model: Provider.Model, schema: ToolJsonSchema): JSONSchema7 {
  // 根据 Provider 调整 tool schema（如 Anthropic 的 input_schema 格式）
}
```

### 4.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1181 | 网关 | 无智能负载均衡 | P1 |
| L1182 | 网关 | 无故障自动转移 | P1 |
| L1183 | 网关 | 无模型降级策略 | P1 |
| L1184 | 网关 | 无请求级路由规则 | P2 |
| L1185 | 网关 | 无 Provider 健康检查 | P1 |

---

## 五、Pregel图执行（Pregel Graph Execution）

### 5.1 现状

opencode **未实现 Pregel 图计算模型**。其 Agent 协作模式基于：

- **TaskTool**：SubAgent 委派
- **SessionPrompt.runLoop()**：主循环迭代
- **Plugin 系统**：扩展点

### 5.2 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1186 | 图计算 | 无 Pregel 模型 | P2 |
| L1187 | 图计算 | 无消息传递机制 | P2 |
| L1188 | 图计算 | 无迭代计算框架 | P2 |
| L1189 | 图计算 | 无顶点程序抽象 | P2 |
| L1190 | 图计算 | 无图分区策略 | P2 |

---

## 六、Skill生命周期（Skill Lifecycle）

### 6.1 Skill 发现（skill/index.ts, 354 行）

opencode 实现了完整的 Skill 发现与注册系统：

```typescript
export interface Interface {
  readonly get: (name: string) => Effect.Effect<Info | undefined>
  readonly require: (name: string) => Effect.Effect<Info, NotFoundError>
  readonly all: () => Effect.Effect<Info[]>
  readonly dirs: () => Effect.Effect<string[]>
  readonly available: (agent?: Agent.Info) => Effect.Effect<Info[]>
}
```

**发现链**：
1. **内置 Skill**：`customize-opencode`（优先级最高，可被用户覆盖）
2. **项目 Skill**：`{skill,skills}/**/SKILL.md`（项目目录向上查找）
3. **外部 Skill**：`.claude/skills/**/SKILL.md`、`.agents/skills/**/SKILL.md`
4. **配置 Skill**：`opencode.json` 的 `skills.paths` / `skills.urls`
5. **远程 Skill**：通过 URL 拉取（`Discovery.pull()`）

### 6.2 Skill 远程拉取（skill/discovery.ts, 160 行）

```typescript
export interface Interface {
  readonly pull: (url: string) => Effect.Effect<string[]>
}

const pull = Effect.fn("Discovery.pull")(function* (url: string) {
  const base = url.endsWith("/") ? url : `${url}/`
  const index = new URL("index.json", base).href
  // 1. 获取 index.json
  // 2. 解析 skill 列表（name, files, version）
  // 3. 并发下载 skill 文件（skillConcurrency=4, fileConcurrency=8）
  // 4. 版本管理：staging + backup + atomic rename
  // 5. 缓存到 Global.Path.cache/skills/{skill.name}/
})
```

**设计要点**：
- 版本控制：通过 `.opencode-version` 文件跟踪版本
- 原子更新：先下载到 `.tmp-{uuid}`，成功后 rename
- 失败回滚：保留 `.old-{uuid}` 备份，失败时恢复
- 并发控制：skill 级 4 并发，file 级 8 并发

### 6.3 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1191 | Skill | 无 Skill 版本冲突检测 | P1 |
| L1192 | Skill | 无 Skill 依赖管理 | P1 |
| L1193 | Skill | 无 Skill 签名验证 | P1 |
| L1194 | Skill | 无 Skill 沙箱执行 | P1 |
| L1195 | Skill | 无 Skill 热更新 | P2 |

---

## 七、Agent预热池（Agent Warmup Pool）

### 7.1 现状

opencode **未实现 Agent 预热池**。其 Agent 创建模式为按需实例化：

```typescript
// packages/opencode/src/agent/agent.ts
export interface Interface {
  readonly get: (agent: string) => Effect.Effect<Info>
  readonly list: () => Effect.Effect<Info[]>
  readonly defaultInfo: () => Effect.Effect<Info>
  readonly defaultAgent: () => Effect.Effect<string>
  readonly generate: (input: { description: string; model?: { providerID: ProviderV2.ID; modelID: ModelV2.ID } }) => Effect.Effect<{ identifier: string; whenToUse: string; systemPrompt: string }, Provider.DefaultModelError>
}
```

### 7.2 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1196 | Agent池 | 无 Agent 预热池 | P2 |
| L1197 | Agent池 | 无 Agent 实例复用 | P2 |
| L1198 | Agent池 | 无 Agent 池化管理 | P2 |
| L1199 | Agent池 | 无预热策略 | P2 |
| L1200 | Agent池 | 无池大小动态调整 | P2 |

---

## 八、Turn锁（Turn Lock / Lease）

### 8.1 Runner 状态机（effect/runner.ts, 217 行）

opencode 实现了完整的 Turn 排他控制：

```typescript
export type State<A, E> =
  | { readonly _tag: "Idle" }
  | { readonly _tag: "Running"; readonly run: RunHandle<A, E> }
  | { readonly _tag: "Shell"; readonly shell: ShellHandle<A, E> }
  | { readonly _tag: "ShellThenRun"; readonly shell: ShellHandle<A, E>; readonly run: PendingHandle<A, E> }

export interface Runner<A, E = never> {
  readonly state: State<A, E>
  readonly busy: boolean
  readonly ensureRunning: (work: Effect.Effect<A, E>) => Effect.Effect<A, E>
  readonly startShell: (work: Effect.Effect<A, E>, ready?: Latch.Latch) => Effect.Effect<A, E | Busy>
  readonly cancel: Effect.Effect<void>
}
```

**状态转换**：
- `Idle` → `Running`：`ensureRunning()` 空闲时启动
- `Idle` → `Shell`：`startShell()` 空闲时启动 shell
- `Shell` → `ShellThenRun`：shell 运行时收到 `ensureRunning()` 请求
- `ShellThenRun` → `Running`：shell 完成后自动切换到 run
- `Running` → `Idle`：run 完成或取消

### 8.2 SessionRunState（session/run-state.ts, 200 行）

```typescript
export interface Interface {
  readonly assertNotBusy: (sessionID: SessionID) => Effect.Effect<void, Session.BusyError>
  readonly cancel: (sessionID: SessionID) => Effect.Effect<void>
  readonly ensureRunning: (sessionID: SessionID, onInterrupt: Effect.Effect<SessionV1.WithParts>, work: Effect.Effect<SessionV1.WithParts>) => Effect.Effect<SessionV1.WithParts>
  readonly startShell: (sessionID: SessionID, onInterrupt: Effect.Effect<SessionV1.WithParts>, work: Effect.Effect<SessionV1.WithParts>, ready?: Latch.Latch) => Effect.Effect<SessionV1.WithParts, Session.BusyError>
}
```

**机制**：
- 每个 Session 有独立的 `Runner` 实例（通过 `InstanceState` 缓存）
- `ensureRunning()` 保证同一 Session 只有一个活跃 Turn
- `startShell()` 在 Runner 空闲时启动 shell，否则返回 `BusyError`
- `cancel()` 取消当前运行并清理后台任务

### 8.3 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1201 | Turn锁 | 无 Lease 超时机制 | P1 |
| L1202 | Turn锁 | 无 Turn 优先级调度 | P2 |
| L1203 | Turn锁 | 无 Turn 抢占机制 | P2 |
| L1204 | Turn锁 | 无分布式 Turn 锁 | P2 |

---

## 九、HTTP客户端高级实现

### 9.1 Transport 抽象（llm/src/route/transport/）

opencode 实现了双层 Transport 抽象：

```typescript
// HTTP JSON Transport
export interface HttpJsonTransport<Body, Frame> extends Transport<Body, HttpPrepared<Frame>, Frame> {
  readonly with: (patch: HttpJsonPatch<Body, Frame>) => HttpJsonTransport<Body, Frame>
}

// WebSocket JSON Transport
export interface JsonTransport<Body, Message> extends Transport<Body, JsonPrepared, string> {
  readonly with: (patch: JsonPatch<Body, Message>) => JsonTransport<Body, Message>
}
```

### 9.2 WebSocket 连接池（plugin/openai/ws-pool.ts, 300 行）

```typescript
export function createWebSocketFetch(options?: CreateWebSocketFetchOptions) {
  const pool = new Map<string, PoolEntry>()
  const connectTimeout = options?.connectTimeout ?? 15_000
  const idleTimeout = options?.idleTimeout ?? 5 * 60 * 1000
  const maxConnectionAge = options?.maxConnectionAge ?? 55 * 60 * 1000
  const streamRetries = options?.streamRetries ?? 5

  // 连接复用：基于 sessionID + conversation 的 key
  // 空闲修剪：每 min(idleTimeout, 60s) 执行一次
  // 故障降级：streamFailures > streamRetries 后 fallback 到 HTTP
}
```

**设计要点**：
- 连接池基于 `sessionID:conversation` 键
- 连接年龄限制：55 分钟（避免长连接问题）
- 空闲超时：5 分钟
- 故障降级：连续 5 次流失败后回退到 HTTP
- 连接限制处理：`websocket_connection_limit_reached` 错误码

### 9.3 SSE 超时控制（provider/provider.ts）

```typescript
function wrapSSE(res: Response, ms: number, ctl: AbortController) {
  if (typeof ms !== "number" || ms <= 0) return res
  if (!res.body) return res
  if (!res.headers.get("content-type")?.includes("text/event-stream")) return res

  const reader = res.body.getReader()
  const body = new ReadableStream<Uint8Array>({
    async pull(ctrl) {
      const part = await new Promise((resolve, reject) => {
        const id = setTimeout(() => {
          const err = new ProviderError.ResponseStreamError("SSE read timed out")
          ctl.abort(err)
          reader.cancel(err).catch(() => {})
          reject(err)
        }, ms)
        reader.read().then(
          (part) => { clearTimeout(id); resolve(part) },
          (err) => { clearTimeout(id); reject(err) },
        )
      })
      if (part.done) { ctrl.close(); return }
      ctrl.enqueue(part.value)
    },
    async cancel(reason) { ctl.abort(reason); await reader.cancel(reason) },
  })
  return new Response(body, { headers: new Headers(res.headers), status: res.status, statusText: res.statusText })
}
```

### 9.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1205 | HTTP | 无连接池空闲超时配置 | P2 |
| L1206 | HTTP | 无 HTTP/2 多路复用 | P1 |
| L1207 | HTTP | 无代理链支持 | P1 |
| L1208 | HTTP | 无请求级超时配置 | P1 |
| L1209 | HTTP | 无 TLS 指纹 Pinning | P2 |

---

## 十、安全加固（Security Hardening）

### 10.1 权限引擎（permission/index.ts, 200 行）

opencode 实现了基于规则引擎的权限系统：

```typescript
export function evaluate(permission: string, pattern: string, ...rulesets: PermissionV1.Ruleset[]): PermissionV1.Rule {
  return (
    rulesets
      .flat()
      .findLast((rule) => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern)) ?? {
      action: "ask",
      permission,
      pattern: "*",
    }
  )
}

export interface Interface {
  readonly ask: (input: PermissionV1.AskInput) => Effect.Effect<void, PermissionV1.Error>
  readonly reply: (input: PermissionV1.ReplyInput) => Effect.Effect<void, PermissionV1.NotFoundError>
  readonly list: () => Effect.Effect<ReadonlyArray<PermissionV1.Request>>
}
```

**权限模型**：
- 三态策略：`allow` / `deny` / `ask`
- 通配符匹配：`Wildcard.match()` 支持 glob 模式
- 规则优先级：`findLast()` 后添加的规则优先
- 会话级权限：`session.permission` 可覆盖 Agent 默认权限
- 权限持久化：`reply("always")` 将规则添加到 `approved` 列表

### 10.2 权限规则示例

```typescript
const defaults = Permission.fromConfig({
  "*": "allow",
  doom_loop: "ask",
  external_directory: { "*": "ask", ...whitelistedDirs.map((dir) => [dir, "allow"]) },
  question: "deny",
  plan_enter: "deny",
  plan_exit: "deny",
  read: { "*": "allow", "*.env": "ask", "*.env.*": "ask", "*.env.example": "allow" },
})
```

### 10.3 认证系统（provider/auth.ts, 200 行）

```typescript
export interface Interface {
  readonly methods: () => Effect.Effect<Methods>
  readonly authorize: (input: { providerID: ProviderV2.ID } & AuthorizeInput) => Effect.Effect<Authorization | undefined, Error>
  readonly callback: (input: { providerID: ProviderV2.ID } & CallbackInput) => Effect.Effect<void, Error>
}
```

**认证方式**：
- OAuth Device Code 流程
- API Key 认证
- 自定义认证 Hook（通过 Plugin 系统）

### 10.4 服务器认证（server/auth.ts, 50 行）

```typescript
export function required(config: Info) {
  return Option.isSome(config.password) && config.password.value !== ""
}

export function authorized(credentials: DecodedCredentials, config: Info) {
  return (
    Option.isSome(config.password) &&
    credentials.username === config.username &&
    Redacted.value(credentials.password) === config.password.value
  )
}
```

### 10.5 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1210 | 安全 | 无 Prompt 注入防护 | P0 |
| L1211 | 安全 | 无密钥加密存储 | P0 |
| L1212 | 安全 | 无输入验证框架 | P1 |
| L1213 | 安全 | 无输出过滤 | P1 |
| L1214 | 安全 | 无威胁模型文档 | P1 |
| L1215 | 安全 | 无 API Key 轮换 | P1 |
| L1216 | 安全 | 无请求签名 | P2 |
| L1217 | 安全 | 无 IP 白名单 | P2 |

---

## 十一、其他关键发现

### 11.1 指令文件发现（session/instruction.ts, 237 行）

```typescript
export interface Interface {
  readonly clear: (messageID: MessageID) => Effect.Effect<void>
  readonly systemPaths: () => Effect.Effect<Set<string>, FSUtil.Error>
  readonly system: () => Effect.Effect<string[], FSUtil.Error>
  readonly find: (dir: string) => Effect.Effect<string | undefined, FSUtil.Error>
  readonly resolve: (messages: SessionV1.WithParts[], filepath: string, messageID: MessageID) => Effect.Effect<{ filepath: string; content: string }[], FSUtil.Error>
}
```

**发现链**：
1. 全局文件：`~/.config/opencode/AGENTS.md`、`~/.claude/CLAUDE.md`
2. 项目文件：`AGENTS.md` → `CLAUDE.md` → `CONTEXT.md`（向上查找）
3. 配置引用：`opencode.json` 的 `instructions` 数组
4. 远程指令：`https://` URL 拉取

### 11.2 会话压缩（session/compaction.ts, 608 行）

```typescript
export const PRUNE_MINIMUM = 20_000
export const PRUNE_PROTECT = 40_000
const TOOL_OUTPUT_MAX_CHARS = 2_000
const PRUNE_PROTECTED_TOOLS = ["skill"]
const MIN_PRESERVE_RECENT_TOKENS = 2_000
const MAX_PRESERVE_RECENT_TOKENS = 15_000
```

**压缩策略**：
- Token 预算：`usable = model.limit.input - reserved`（reserved = 20K）
- 保留最近：`preserveRecentBudget = min(15K, max(2K, floor(usable * 0.25)))`
- 工具输出截断：2000 字符
- 保护工具：`skill` 工具的输出不截断
- 溢出检测：`isOverflow = tokens >= usable`

### 11.3 溢出检测（session/overflow.ts, 30 行）

```typescript
export function usable(input: { cfg: ConfigV1.Info; model: Provider.Model; outputTokenMax?: number }) {
  const context = input.model.limit.context
  if (context === 0) return 0
  const reserved = input.cfg.compaction?.reserved ??
    Math.min(COMPACTION_BUFFER, ProviderTransform.maxOutputTokens(input.model, input.outputTokenMax))
  return input.model.limit.input
    ? Math.max(0, input.model.limit.input - reserved)
    : Math.max(0, context - ProviderTransform.maxOutputTokens(input.model, input.outputTokenMax))
}

export function isOverflow(input: { cfg: ConfigV1.Info; tokens: SessionV1.Assistant["tokens"]; model: Provider.Model; outputTokenMax?: number }) {
  if (input.cfg.compaction?.auto === false) return false
  if (input.model.limit.context === 0) return false
  const count = input.tokens.total || input.tokens.input + input.tokens.output + input.tokens.cache.read + input.tokens.cache.write
  return count >= usable(input)
}
```

### 11.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1218 | 指令 | 无指令文件签名验证 | P1 |
| L1219 | 压缩 | 无压缩质量评估 | P2 |
| L1220 | 溢出 | 无动态 buffer 调整 | P2 |
| L1221 | 摘要 | 无增量摘要更新 | P2 |

---

## 十二、新增 gap 清单（汇总）

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 panic hook / CrashDump 机制 | P0 |
| L1167 | 崩溃恢复 | 无 watchdog 进程监控 | P1 |
| L1168 | 崩溃恢复 | 无自动 restart loop | P1 |
| L1169 | 取证 | 无结构化错误报告输出 | P1 |
| L1170 | 快照 | 无增量快照（仅全量 commit） | P2 |
| L1171 | 多租户 | 无细粒度 RBAC（仅 Org 级别） | P1 |
| L1172 | 多租户 | 无资源配额限制 | P1 |
| L1173 | 多租户 | 无审计日志 | P1 |
| L1174 | 隔离 | 无进程级沙箱隔离 | P0 |
| L1175 | 隔离 | 无网络隔离 | P1 |
| L1176 | 检索 | 无 RRF 混合检索 | P1 |
| L1177 | 检索 | 无向量检索 | P1 |
| L1178 | 检索 | 无语义搜索 | P1 |
| L1179 | 检索 | 无 BM25 + Vector 融合排序 | P2 |
| L1180 | 检索 | 无检索结果重排序 | P2 |
| L1181 | 网关 | 无智能负载均衡 | P1 |
| L1182 | 网关 | 无故障自动转移 | P1 |
| L1183 | 网关 | 无模型降级策略 | P1 |
| L1184 | 网关 | 无请求级路由规则 | P2 |
| L1185 | 网关 | 无 Provider 健康检查 | P1 |
| L1186 | 图计算 | 无 Pregel 模型 | P2 |
| L1187 | 图计算 | 无消息传递机制 | P2 |
| L1188 | 图计算 | 无迭代计算框架 | P2 |
| L1189 | 图计算 | 无顶点程序抽象 | P2 |
| L1190 | 图计算 | 无图分区策略 | P2 |
| L1191 | Skill | 无 Skill 版本冲突检测 | P1 |
| L1192 | Skill | 无 Skill 依赖管理 | P1 |
| L1193 | Skill | 无 Skill 签名验证 | P1 |
| L1194 | Skill | 无 Skill 沙箱执行 | P1 |
| L1195 | Skill | 无 Skill 热更新 | P2 |
| L1196 | Agent池 | 无 Agent 预热池 | P2 |
| L1197 | Agent池 | 无 Agent 实例复用 | P2 |
| L1198 | Agent池 | 无 Agent 池化管理 | P2 |
| L1199 | Agent池 | 无预热策略 | P2 |
| L1200 | Agent池 | 无池大小动态调整 | P2 |
| L1201 | Turn锁 | 无 Lease 超时机制 | P1 |
| L1202 | Turn锁 | 无 Turn 优先级调度 | P2 |
| L1203 | Turn锁 | 无 Turn 抢占机制 | P2 |
| L1204 | Turn锁 | 无分布式 Turn 锁 | P2 |
| L1205 | HTTP | 无连接池空闲超时配置 | P2 |
| L1206 | HTTP | 无 HTTP/2 多路复用 | P1 |
| L1207 | HTTP | 无代理链支持 | P1 |
| L1208 | HTTP | 无请求级超时配置 | P1 |
| L1209 | HTTP | 无 TLS 指纹 Pinning | P2 |
| L1210 | 安全 | 无 Prompt 注入防护 | P0 |
| L1211 | 安全 | 无密钥加密存储 | P0 |
| L1212 | 安全 | 无输入验证框架 | P1 |
| L1213 | 安全 | 无输出过滤 | P1 |
| L1214 | 安全 | 无威胁模型文档 | P1 |
| L1215 | 安全 | 无 API Key 轮换 | P1 |
| L1216 | 安全 | 无请求签名 | P2 |
| L1217 | 安全 | 无 IP 白名单 | P2 |
| L1218 | 指令 | 无指令文件签名验证 | P1 |
| L1219 | 压缩 | 无压缩质量评估 | P2 |
| L1220 | 溢出 | 无动态 buffer 调整 | P2 |
| L1221 | 摘要 | 无增量摘要更新 | P2 |

---

## 十三、关键代码片段索引

| 模块 | 文件 | 行数 | 核心功能 |
|------|------|------|----------|
| 崩溃恢复 | server/global-lifecycle.ts | 35 | 全局生命周期管理 |
| 崩溃恢复 | effect/instance-registry.ts | 20 | 实例资源回收 |
| 崩溃恢复 | session/retry.ts | 312 | 指数退避重试 |
| 崩溃恢复 | snapshot/index.ts | 807 | Git 快照系统 |
| 崩溃恢复 | session/revert.ts | 160 | 会话回滚 |
| 多租户 | account/account.ts | 481 | OAuth 多账户 |
| 多租户 | control-plane/workspace.ts | 966 | 工作空间隔离 |
| 多租户 | server/location.ts | 50 | 位置服务隔离 |
| 多租户 | project/instance-store.ts | 450 | 实例级隔离 |
| Skill | skill/index.ts | 354 | Skill 发现与注册 |
| Skill | skill/discovery.ts | 160 | 远程 Skill 拉取 |
| Turn锁 | effect/runner.ts | 217 | Runner 状态机 |
| Turn锁 | session/run-state.ts | 200 | Session 运行状态 |
| HTTP | llm/src/route/transport/http.ts | 120 | HTTP JSON Transport |
| HTTP | llm/src/route/transport/websocket.ts | 230 | WebSocket Transport |
| HTTP | plugin/openai/ws-pool.ts | 300 | WebSocket 连接池 |
| 安全 | permission/index.ts | 200 | 权限引擎 |
| 安全 | provider/auth.ts | 200 | Provider 认证 |
| 安全 | server/auth.ts | 50 | 服务器认证 |
| 压缩 | session/compaction.ts | 608 | 上下文压缩 |
| 溢出 | session/overflow.ts | 30 | 溢出检测 |
| 摘要 | session/summary.ts | 150 | 会话摘要 |
| 指令 | session/instruction.ts | 237 | 指令文件发现 |
| 提醒 | session/reminders.ts | 80 | 会话提醒 |
| 工具 | tool/registry.ts | 200 | 工具注册表 |
| 工具 | session/tools.ts | 590 | 会话工具解析 |
| LLM | session/llm.ts | 404 | LLM 流式调用 |
| Provider | provider/provider.ts | 2068 | Provider 管理 |
| Provider | provider/transform.ts | 1890 | Provider 消息转换 |
| 处理器 | session/processor.ts | 732 | 单轮流处理 |
| 提示词 | session/prompt.ts | 1631 | 主循环编排 |
| 消息 | session/message-v2.ts | 737 | 消息/Part 模型 |
| 会话 | session/session.ts | 1016 | Session CRUD |

---

**报告完成日期**：2026-09-09
**分析基础**：opencode packages/opencode/src/ + packages/core/src/ + packages/llm/src/ + packages/server/src/ 逐行分析
