# 专题：6 个 Agent 项目的 Agent 协作与调度横向分析

> **元信息**
> - 生成日期：2026-09-04
> - 分析范围：claude-code / atomcode / openclaw / opencode / deepseek-harness / pi
> - 输入文档：`docs/Agent源码调研/` 下 6 份「核心机制深度分析」
> - 对标基准：laew MultiAgentOrchestrator 6 角色调度（Yolo→Plan→MainWork→SubAgent→QC→SessionContext）
> - 定位：横向专题报告，聚焦「Agent 协作运行时与调度」——委派协议、上下文传递、并发调度、通信方式、角色分工、失败回流；与《SubAgent 与多Agent架构》互补（该篇侧重「机制」层面，本篇聚焦「运行时调度」）

---

## 目录

- [1. 横向对比总览表](#1-横向对比总览表)
- [2. AtomCode：Team 事件流 + Semaphore 并发 + Scope 沙箱](#2-atomcode-team-事件流--semaphore-并发--scope-沙箱)
- [3. Claude Code：四层 SubAgent + Swarm 后端 + Coordinator](#3-claude-code-四层-subagent--swarm-后端--coordinator)
- [4. DeepSeek-Harness：Goal 域 round driver + Provider 注册表](#4-deepseek-harness-goal-域-round-driver--provider-注册表)
- [5. OpenClaw：Swarm FIFO 调度 + 9 步 spawn + 代次管理](#5-openclaw-swarm-fifo-调度--9-步-spawn--代次管理)
- [6. OpenCode：Agent 类型系统 + Session 状态机 + doom_loop](#6-opencode-agent-类型系统--session-状态机--doom_loop)
- [7. Pi：Lane 并发树 + 三队列 + Session Tree 分叉](#7-pi-lane-并发树--三队列--session-tree-分叉)
- [8. 设计模式提炼](#8-设计模式提炼)
- [9. 对 laew 的综合建议](#9-对-laew-的综合建议)
- [附录 A：laew 当前编排现状详设](#附录-alae-当前编排现状详设)
- [附录 B：各项目调度/委派代码片段速查](#附录-b各项目调度委派代码片段速查)
- [附录 C：建议实现方案详设](#附录-c建议实现方案详设)

---

## 1. 横向对比总览表

### 1.1 核心机制对比

| 维度 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek-Harness | Pi | **laew** |
|------|-------------|----------|----------|----------|-----------------|-----|---------|
| **委派机制** | AgentTool 工具调用 + Fork + Task + Swarm | TaskTool 工具调用（subagent-by-composition） | `spawnSubagentDirect()` 9 步 spawn | TaskTool 工具调用（child session） | SubAgentProvider 注册表 + `ctx.agents.create()` | AgentLane 接口（steer/followUp/nextRun） | 编排器直调（Yolo→Plan→MainWork→SubAgent） |
| **委派参数** | prompt + agent_type + model + effort + isolation | tasks[]: prompt/subagent_type/difficulty/role/scope | task + label + agentId + spawnMode + contextMode + attachments | prompt + agent + background + task_id | prompt + composition(persona/toolFilter) + descriptor(mode/one-shot/continuable) | message（steer/followUp/nextRun） | WorkFlowSpec（id/description/expected_output/depends_on） |
| **上下文传递** | Fork 共享 cache-safe params；AgentTool 完全隔离 | 独立内核会话 + persona 注入 + scope 中间件 | isolated/fork 两种 contextMode；attachments 材料化 | child session 仅传 task prompt；parent 的 external_directory/deny 继承 | childSessionMeta 持久化 parentSession + delegationDepth + seedLength；preset 组合继承 | Lane 隔离（独立上下文）；view(lane) 分支视图 | 独立 Agent-Context + Agent-Memory（SQLite） |
| **并发模型** | Swarm 多后端（InProcess/Tmux/ITerm2）+ Task 系统 | Semaphore（默认 3）+ JoinSet + tokio::spawn | SwarmGroupLane FIFO 队列 + 容量 limit + pumpLane | Session 状态机（Idle/Running/Shell）+ BackgroundJob | round driver 串行驱动 + maxGoalRounds 限制 | Lane 并发（1 操作/lane）+ QueueMode | **无并发**（串行 topo_sort） |
| **调度策略** | 文件锁（proper-lockfile）+ 后端自动探测 | Semaphore 容量控制 + 非重叠 scope 验证 | reserve→activate→release + 失败回退队头 1s 重试 | Runner.ensureRunning 合并/排队 + 跨 session 纤维并发 | requestDrive 串行化 + checkpoint 持久化 | LaneBusy 互斥 + QueueMode（latest/all） | 拓扑排序 + 逐 WorkFlow 串行 |
| **通信方式** | SendMessageTool + 文件 mailbox + `<task-notification>` | TeamEvent 流（RunStarted/MemberQueued/MemberStarted/MemberActivity/MemberFinished） | Lifecycle Emitter（subagent-complete/error/killed）+ 消息总线 | Effect 服务调用 + 合成 XML tool-result | Cordis 事件系统 + Goal 事件源（fold） | steer/followUp/nextRun 队列 + Deferred Handle | 编排器传递 + dep_outputs HashMap |
| **角色分工** | 动态（BuiltIn/Custom/Plugin + Skill agent 字段） | 14 种 TeamRoleId（Explore/Worker × Simple/Hard） | 三态（Built-in/CLI/ACP）+ 11 capability Harness | 7 种内置 Agent（primary/subagent）+ 用户自定义 | Provider 注册表 + authority 模型（requireDirectHuman/completionAuthority） | 扩展定义（Skill + Tool + drive 模式） | **6 固定角色**（Yolo/Plan/MainWork/SubAgent/QC/SessionContext） |
| **质量门控** | Hook PreToolUse/PostToolUse + SubagentStart/Stop | VerifyCadenceHook + Review Agent + ExecutionPolicy | exec auto-reviewer（4 级风险）+ Harness fallback | doom_loop 权限请求 + 用户审批 | GoalPhase blocked + wrapup 注入 + approval policy='never' | 无显式 QC | **Quality-Check Agent**（每单元必经） |
| **失败回流** | Task Failed/Cancelled + Hook 拦截 + 后端降级 | retryable_content_free_failure 回退 host provider + stream_timeout | Run 4 终态 + 启动失败回退队头 + AbortSignal 传播 | doom_loop 权限请求 + retry Schedule（5 次退避） | round-limit block + GOAL_STALE_REVISION + wrapup | LaneBusy + suspended→resume + branch summary | QualityFailure.retryable + Yolo 重新评估 + max_retry_per_level |
| **最大深度/轮次** | maxTurns + 递归 fork 禁止 | max_rounds（DEFAULT_CHILD_MAX_ROUNDS）+ max_concurrent=3 | maxSpawnDepth + generation 代次 | cfg.subagent_depth ?? 1 | maxGoalRounds（默认 256）+ delegationDepth | 无显式限制 | max_retry_per_level=3 + subagent_max_iterations=16 |

### 1.2 调度策略光谱

```
完全串行                                              完全并行
←──────────────────────────────────────────────────────────────────→
laew       OpenCode     DeepSeek     AtomCode     ClaudeCode     pi
(topo_sort) (per-session) (round-driver) (Semaphore=3) (Swarm)      (Lane并发)
```

### 1.3 编排集中度光谱

```
完全编排                                              完全自主
←──────────────────────────────────────────────────────────────────→
laew        AtomCode      OpenClaw       DeepSeek     OpenCode       pi
(Orchestrator) (TaskTool) (spawn+swarm)  (round-driver) (Runner)      (Lane)
```

### 1.4 上下文隔离光谱

```
完全共享                                              完全隔离
←──────────────────────────────────────────────────────────────────→
ClaudeCode-Fork  DeepSeek  laew  OpenClaw-fork  OpenCode  AtomCode  pi
(cache共享)      (preset继承) (独立+注入) (fork模式)    (child session) (独立内核) (Lane隔离)
```

---

## 2. AtomCode：Team 事件流 + Semaphore 并发 + Scope 沙箱

### 2.1 委派协议

AtomCode 的委派是**工具调用式**的 `TaskTool`（`crates/atomcode-capabilities/src/tools/task.rs`），主 agent 通过 JSON 参数发起一批子任务：

```rust
// task.rs:689-722 - parameters_schema
fn parameters_schema(&self) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "description": {"type": "string"},
                        "prompt": {"type": "string"},
                        "subagent_type": {"type": "string", "enum": ["explore", "worker"]},
                        "difficulty": {"type": "string", "enum": ["simple", "hard"]},
                        "model": {"type": "string"},  // 可选 per-task model
                        "role": {"type": "string", "enum": ["planner", "architect", "explorer", "implementer", "rust", "tui_ux", "reviewer", "tester", "debugger", "security", "performance", "docs_writer", "release_manager", "migration_compat"]},
                        "scope": {"type": "array", "items": {"type": "string"}}  // worker-only
                    },
                    "required": ["description", "prompt", "subagent_type"]
                }
            }
        }
    })
}
```

**关键设计**：
- **批量发射**：一次 `task` 工具调用可派发多个子任务（`tasks: []`），由 `JoinSet` 并行执行
- **难度分档**：`simple` → `make_fast_provider()`，`hard` → `make_capable_provider()`，可 per-task 覆盖 `model`
- **角色系统**：14 种 `TeamRoleId`（Planner/Architect/Explorer/Implementer/Rust/TuiUx/Reviewer/Tester/Debugger/Security/Performance/DocsWriter/ReleaseManager/MigrationCompat），每种有独立的 persona 和 permission

### 2.2 上下文传递

子 agent 跑在**独立内核会话**里，不共享主 agent 上下文：

```rust
// task.rs:1509-1544 - build_task_child
fn build_task_child(
    provider: Arc<dyn LlmProvider>,
    tools: MountedTools,
    persona: String,      // 独立 persona（EXPLORE_PERSONA / WORKER_PERSONA + role persona）
    working_dir: PathBuf,
    cancel: CancellationToken,
    progress: Arc<SubtaskProgressHook>,
    tool_loop_policy: Option<ToolLoopPolicy>,
    max_rounds: Option<u32>,
    stream_timeout: Option<Duration>,
    middlewares: Vec<Arc<dyn ToolMiddleware>>,
) -> Agent {
    Agent::builder()
        .provider(provider).tools(tools).persona(persona)
        .working_dir(working_dir).cancel_token(cancel)
        .hook(progress)...
        .build()
}
```

**persona 注入**（`subtask_persona()`）：
```rust
// task.rs:1187-1196
fn subtask_persona(profile: &TeamRoleProfile) -> String {
    let base = match profile.permission {
        TeamPermission::Explore => EXPLORE_PERSONA,  // "You are a READ-ONLY investigation subagent..."
        TeamPermission::Worker => WORKER_PERSONA,    // "You are a focused EXECUTION subagent..."
    };
    format!("{base}\n\n## TEAM ROLE\nYou are the {} role.\n{}\n{}",
        profile.display_name, profile.persona, profile.when_to_use)
}
```

### 2.3 并发调度

**Semaphore 容量控制**：

```rust
// task.rs:23
const DEFAULT_MAX_CONCURRENT: usize = 3;

// task.rs:862-863
let sem = Arc::new(tokio::sync::Semaphore::new(self.max_concurrent));
let mut set = tokio::task::JoinSet::new();
```

**并行执行 + 完成收集**：

```rust
// task.rs:933-1075
set.spawn(async move {
    let _permit = sem.acquire_owned().await.expect("semaphore not closed");
    // ... 执行子任务
    let handle = tokio::spawn(run_child_to_completion(child, prompt, AutoRespond::AllowAll, progress_hook));
    let mut outcome = match handle.await {
        Ok(o) => o,
        Err(join_err) => Outcome { stop: StopReason::ProviderError, error: Some(...), ..Default::default() },
    };
    // 失败回退到 host provider
    if fallback_provider.is_some() && retryable_content_free_failure(&outcome) && !child_cancel.is_cancelled() {
        outcome = run_child_to_completion(fallback_child, prompt, AutoRespond::AllowAll, progress_hook).await;
    }
    (label, desc, final_model, outcome)
});

// 收集所有结果
let mut collected: Vec<(String, String, String, Outcome)> = Vec::new();
while let Some(res) = set.join_next().await {
    if let Ok(tuple) = res { collected.push(tuple); }
}
aggregate_task_result(collected)
```

**非重叠 scope 验证**（`validate_non_overlapping_worker_scopes`）：并行 worker 的写入 scope 必须不重叠，防止并发写冲突。

### 2.4 通信方式

**TeamEvent 事件流**（`crates/atomcode-capabilities/src/team.rs`）：

```rust
// team.rs:332-378
pub enum TeamEventPayload {
    RunStarted { total: usize },
    MemberQueued { member_id, role, model, description },
    MemberStarted { member_id, role, model, description },
    MemberActivity { member_id, activity, output_tokens },
    MemberFinished { member_id, success, stop, summary, output_tokens },
    RunFinished { total, completed, failed },
}
```

**ProgressSink 实时进度**：通过 `SUBAGENT_ACTIVITY_MARKER`（`\u{1e}`）前缀区分 ephemeral live activity 与 committed scrollback line，TUI 将 marker 前缀的 chunk 路由到 in-place spinner 而非 scrollback。

### 2.5 角色分工

**TeamRoleId 14 种角色**（`team.rs:9-24`）：

| 角色 | Permission | 典型用途 |
|------|-----------|---------|
| Planner | Explore | 任务规划 |
| Architect | Explore | 架构设计 |
| Explorer | Explore | 只读代码探索 |
| Implementer | Worker | 代码实现 |
| Rust | Worker | Rust 特化 |
| TuiUx | Worker | TUI/UX 实现 |
| Reviewer | Explore | 代码审查 |
| Tester | Worker | 测试编写 |
| Debugger | Worker | 调试 |
| Security | Explore | 安全审计 |
| Performance | Worker | 性能优化 |
| DocsWriter | Worker | 文档编写 |
| ReleaseManager | Worker | 发布管理 |
| MigrationCompat | Worker | 迁移兼容 |

**TeamRunnerFactory**（`crates/atomcode-coding/src/team/runner.rs`）提供独立的 Team 运行器，与 TaskTool 共享 `TeamTaskSpec` 但走不同的调度路径。

### 2.6 失败回流

**retryable_content_free_failure 回退**：

```rust
// task.rs:1006-1050
if fallback_provider.is_some() && retryable_content_free_failure(&outcome) && !child_cancel.is_cancelled() {
    let fallback = fallback_provider.expect("checked above");
    progress_hook.publish(Some(format!("{model} failed before producing output; retrying with {fallback_model}")), true);
    let fallback_child = build_task_child(fallback, tools, persona, wd, child_cancel, ...);
    outcome = run_child_to_completion(fallback_child, prompt, AutoRespond::AllowAll, progress_hook).await;
}
```

**多层安全约束**：
- `DenySensitivePaths`：子 agent 运行在 `AutoRespond::AllowAll` 模式，敏感路径硬拒绝（非提示）
- `WorkerScopeGate`：confine 子 agent 的写工具到声明的 scope（glob 匹配）
- `DenyTeamBash`：Team 子 agent 禁止 bash（验证由 parent agent 负责）
- `.git` 内部写入被拒绝（防止子 agent 注入 hooks）

### 2.7 对 laew 的借鉴点

1. **TeamEvent 事件流**：laew 的 MultiAgentOrchestrator 缺少结构化事件流，可借鉴 `RunStarted/MemberQueued/MemberStarted/MemberActivity/MemberFinished` 六元事件
2. **Semaphore 并发**：hard 任务的并行子任务可用 Semaphore 控制并发度（参考 `DEFAULT_MAX_CONCURRENT=3`）
3. **WorkerScopeGate**：SubAgent-Work 的写入范围约束（glob 匹配 + 非重叠验证）
4. **retryable_content_free_failure 回退**：provider 失败时回退到 host provider 重试
5. **persona 注入**：按角色动态组装系统提示词（`subtask_persona()`）

---

## 3. Claude Code：四层 SubAgent + Swarm 后端 + Coordinator

### 3.1 委派协议

Claude Code 有**四层 SubAgent 体系**，由 `AgentTool.call()` 统一路由：

```typescript
// AgentTool.tsx - 路由逻辑（简化）
const teamName = resolveTeamName({ team_name }, appState);
if (teamName && name) {
    // 路径 A: Swarm teammate spawn
    return spawnTeammate({ name, prompt, description, team_name: teamName, ... }, toolUseContext);
}
const effectiveType = subagent_type ?? (isForkSubagentEnabled() ? undefined : GENERAL_PURPOSE_AGENT.agentType);
// 路径 B: Fork 子 Agent（cache 共享）
// 路径 C: 显式 subagent_type
// 路径 D: remote isolation
```

**四层体系**：

| 层次 | 机制 | 上下文隔离 | Prompt Cache | 用途 |
|------|------|-----------|-------------|------|
| Fork 子 Agent | `runForkedAgent()` | 隔离消息，共享 cache-safe 参数 | 共享 | compact、skill fork |
| AgentTool（同步） | `runAgent()` | 完全隔离 | 独立 | 模型启动的子任务 |
| AgentTool（异步） | `runAsyncAgentLifecycle()` | 完全隔离 | 独立 | 后台任务 |
| Task 系统 | `TaskCreateTool` | 完全隔离 | 独立 | 后台异步 + 结果检索 |
| Team/Swarm | `spawnTeammate()` | 独立进程/窗格 | 独立 | 多 Agent 并发 |

**runAgent 核心**（`src/tools/AgentTool/runAgent.ts`）：

```typescript
const agentToolUseContext = createSubagentContext(toolUseContext, {
    options: agentOptions, agentId, agentType: agentDefinition.agentType,
    messages: initialMessages, readFileState: agentReadFileState,
    abortController: agentAbortController, ...
});
for await (const message of query({
    messages: initialMessages, systemPrompt: agentSystemPrompt,
    userContext: resolvedUserContext, systemContext: resolvedSystemContext,
    canUseTool, toolUseContext: agentToolUseContext, querySource,
    maxTurns: maxTurns ?? agentDefinition.maxTurns,
})) { yield message; }
```

### 3.2 上下文传递

**createSubagentContext 隔离原语**（`src/utils/forkedAgent.ts`）：

```typescript
export function createSubagentContext(parentContext: ToolUseContext, overrides?: SubagentContextOverrides): ToolUseContext {
    return {
        readFileState: cloneFileStateCache(overrides?.readFileState ?? parentContext.readFileState),
        nestedMemoryAttachmentTriggers: new Set<string>(),
        contentReplacementState: overrides?.contentReplacementState ?? (parentContext.contentReplacementState ? cloneContentReplacementState(...) : undefined),
        abortController: overrides?.abortController ?? (overrides?.shareAbortController ? parentContext.abortController : createChildAbortController(parentContext.abortController)),
        getAppState: overrides?.getAppState ?? (overrides?.shareAbortController ? parentContext.getAppState : () => { /* wrap */ }),
        setAppState: overrides?.shareSetAppState ? parentContext.setAppState : () => {},  // 默认 no-op
        messages: overrides?.messages ?? parentContext.messages,
        agentId: overrides?.agentId ?? createAgentId(),
        queryTracking: { chainId: randomUUID(), depth: (parentContext.queryTracking?.depth ?? -1) + 1 },
        ...
    };
}
```

**Fork cache 共享**（`CacheSafeParams`）：

```typescript
export type CacheSafeParams = {
    systemPrompt: SystemPrompt   // 必须与 parent 一致才能 cache hit
    userContext: { [k: string]: string }
    systemContext: { [k: string]: string }
    toolUseContext: ToolUseContext
    forkContextMessages: Message[]  // parent 对话前缀
}
```

### 3.3 并发调度

**Swarm 多后端**（`src/utils/swarm/backends/`）：

```typescript
export type TeammateExecutor = {
    readonly type: BackendType  // 'tmux' | 'iterm2' | 'in-process'
    isAvailable(): Promise<boolean>
    spawn(config: TeammateSpawnConfig): Promise<TeammateSpawnResult>
    sendMessage(agentId: string, message: TeammateMessage): Promise<void>
    terminate(agentId: string, reason?: string): Promise<boolean>
    kill(agentId: string): Promise<boolean>
    isActive(agentId: string): Promise<boolean>
}
```

**后端选择**（`registry.ts`）：inside-tmux → tmux；iTerm2+it2 → iTerm2；else tmux-external；`isInProcessEnabled()` 在非交互/非终端环境返回 true。

**Task 系统文件锁**（`src/utils/tasks.ts`）：

```typescript
const LOCK_OPTIONS = { retries: { retries: 30, minTimeout: 5, maxTimeout: 100 } }
export async function createTask(taskListId, taskData) {
    release = await lockfile.lock(lockPath, LOCK_OPTIONS);
    const highestId = await findHighestTaskId(taskListId); ...
}
```

### 3.4 通信方式

**SendMessageTool**（`src/tools/SendMessageTool/SendMessageTool.ts`）：

```typescript
// 三层路由
switch (input.message.type) {
    case 'shutdown_request': return handleShutdownRequest(...)
    case 'shutdown_response': return handleShutdownApproval(...) / handleShutdownRejection(...)
    case 'plan_approval_response': return handlePlanApproval(...) / handlePlanRejection(...)
}
// 跨 session（UDS_INBOX）→ bridge:/uds:
// 进程内 subagent → queuePendingMessage() / resumeAgentBackground()
// Ambient team → handleBroadcast() / handleMessage()
```

**文件 mailbox**（`src/utils/teammateMailbox.ts`）：所有 teammate 共享 `~/.claude/teams/{team}/inboxes/{agent}.json`，`writeToMailbox()` + `readMailbox()` / `readUnreadMessages()`（锁保护）。

**结果回传**：Async agent 通过 `<task-notification>` XML user-role 消息回传给 leader（`enqueueAgentNotification()`）。

### 3.5 角色分工

**三种 Agent 定义**：

```typescript
export type AgentDefinition = BuiltInAgentDefinition | CustomAgentDefinition | PluginAgentDefinition
export type BaseAgentDefinition = {
    agentType, whenToUse, tools?, disallowedTools?, skills?, mcpServers?, hooks?,
    color?, model?, effort?, permissionMode?, maxTurns?, background?, initialPrompt?,
    memory?, isolation?, requiredMcpServers?, omitClaudeMd?, criticalSystemReminder_EXPERIMENTAL?
}
```

**Skill agent 字段**（`src/skills/loadSkillsDir.ts`）：Skill YAML frontmatter 的 `agent` 字段绑定 skill 到特定 agent 类型，`executionContext: 'fork'` 使 skill 在 cache-sharing fork 子 agent 中运行。

**Team/Swarm 角色**：动态、leader-assigned。`TeamFile.members` roster 记录每个 member 的 `agentType`、`mode`、`isActive`。Leader 通过 `setMemberMode()` / `setMultipleMemberModes()` 运行时变更模式。

### 3.6 失败回流

**Hook 系统**（`src/utils/hooks.ts`）：
- `SubagentStart` / `Stop`（匹配 `agent_type`）→ `executeSubagentStartHooks()` 注入 `additionalContexts`
- `PreToolUse` / `PostToolUse` / `PostToolUseFailure`（匹配 `tool_name`）→ 10-min 超时，可返回 `blockingError`
- `TaskCreated` / `TaskCompleted` → `TaskCreateTool` 执行 `executeTaskCreatedHooks()`

**Task Failed/Cancelled**：`LocalAgentTaskState` 状态 `running | completed | failed | killed`。`unassignTeammateTasks()` 重置 owned tasks 为 `pending` 供重新分配。

**Swarm 后端降级**：`markInProcessFallback()` + `inProcessFallbackActive` flag 在无 pane 后端时短路到 in-process。

### 3.7 对 laew 的借鉴点

1. **四层 SubAgent 体系**：laew 目前只有 SubAgent-Work 一层，可参考分层设计（Fork 轻量 / AgentTool 标准 / Task 后台 / Team 并发）
2. **Fork cache 共享**：`CacheSafeParams` 的 5 字段 cache-key 设计，laew 的 SubAgent 可借鉴
3. **SendMessageTool**：Agent 间消息传递工具，laew 的 6 角色间可引入类似机制
4. **Coordinator 模式**：prompt-defined role，laew 的 Yolo Agent 可借鉴其"编排器即角色"设计
5. **文件 mailbox**：Agent 间异步通信的持久化邮箱

---

## 4. DeepSeek-Harness：Goal 域 round driver + Provider 注册表

### 4.1 委派协议

DeepSeek-Harness 的委派基于 **SubAgentProvider 注册表** + **`ctx.agents.create()`**：

```typescript
// packages/subagent/subagent/src/child-agent.ts:199-211
export function applyChildComposition(childCtx: Context, parent: Agent, composition: ChildComposition): void {
    childCtx.get('agentPresets')?.composeFrom(childCtx, parent.ctx)  // 继承 parent 的 preset
    childCtx.systemPrompt.context({ name: 'subagent:delegation', order: 120, text: SUBAGENT_DELEGATION_CONTEXT })
    if (composition.persona !== undefined) {
        childCtx.systemPrompt.section({ name: 'deployment:persona', order: PERSONA_ORDER, text: composition.persona })
    }
    if (composition.toolFilter !== undefined) childCtx.tools.restrict(composition.toolFilter)
}
```

**SUBAGENT_DELEGATION_CONTEXT**：

```typescript
export const SUBAGENT_DELEGATION_CONTEXT
  = 'You are a delegated subagent: your permission scope was fixed when you were started and cannot be '
    + 'widened from inside this session — operations that require approval are rejected automatically. '
    + 'When the task needs access beyond that scope, do not retry the denied operation; state the '
    + 'limitation in your reply so the delegating agent can handle it.'
```

**Descriptor 持久化**（`packages/subagent/subagent/src/descriptor.ts`）：

```typescript
export interface ContinuableSubagentDescriptorData extends SubagentDescriptorBase {
    readonly mode: 'continuable'
    readonly label: string
    readonly agentProvider?: string
    readonly agentModel?: string
    readonly agentReasoningEffort?: ReasoningEffortId
    readonly persona?: string          // 冷 resume 时恢复的 persona
    readonly toolFilter?: ToolRestriction  // 冷 resume 时恢复的工具限制
}
```

### 4.2 上下文传递

**childSessionMeta 持久化**：

```typescript
// child-agent.ts:138-156
export function childSessionMeta(parent: Agent, childDepth: number, lineageSeedLength: number): NonNullable<CreateAgentOptions['meta']> {
    const parentHeader = parent.session.header
    const agentPreset = parent.ctx.get('agentPresets')?.composedPreset(parent.ctx)
    return {
        ...parentHeader.cwd !== undefined ? { cwd: parentHeader.cwd } : {},
        ...agentPreset === undefined ? {} : { agentPreset },
        parentSession: parentHeader.id,
        origin: 'subagent',
        delegationDepth: childDepth,  // 持久化：递归预算必须跨 crash 存活
        ...lineageSeedLength > 0 ? { seedLength: lineageSeedLength } : {},
    }
}
```

**DelegatedPolicyOverrides**：

```typescript
// child-agent.ts:213-240
export function captureDelegatedPolicyOverrides(parent: Agent): DelegatedPolicyOverrides {
    return {
        sandboxMode: parent.ctx.get('sandboxPolicy')?.overrideOf(parent.session),
        approvalPolicy: parent.ctx.get('approval') === undefined ? undefined : 'never',  // 子 agent 审批策略固定为 never
    }
}
```

### 4.3 并发调度

**round driver 串行驱动**（`packages/goal/goal-round-driver/src/index.ts`）：

```typescript
// index.ts:138-205 - drive()
async function drive(state: DriverState): Promise<void> {
    if (!readyToDrive(state)) return
    if (state.needsCheckpoint) {
        await ctx.sessions.flush(agent.session)  // 持久化 checkpoint
        if (!readyAfterCheckpoint(state)) return
    }
    const goal = currentGoal(state)
    if (goal === undefined || goal.phase !== 'active' || goal.activation !== 'armed') return
    if (goal.roundsStarted >= goal.maxGoalRounds) {
        ctx.goals.block(agent, goalRef(goal), { code: 'round-limit', message: `Goal reached its configured limit of ${goal.maxGoalRounds} rounds.` })
        return
    }
    const round = goal.roundsStarted + 1
    const content = renderGoalRoundPrompt(goal, round)
    const message = createUserMessage({ content, source: { kind: 'goal', goalId: goal.id, revision: goal.revision, round } })
    agent.followup(message)
}
```

**requestDrive 串行化**：

```typescript
// index.ts:208-241
function requestDrive(state: DriverState): void {
    state.requested = true
    if (state.run !== undefined) return  // 已有驱动任务
    let run = ctx.agents.withoutInitiator(async () => {
        while (state.requested && !state.stopping) {
            state.requested = false
            await drive(state)
        }
    })
    state.run = run
}
```

### 4.4 通信方式

**Cordis 事件系统**：Goal 是**事件源**（event-sourced）模型，每次变更写入 `goal/change` session 事件，当前状态通过 fold 事件流重建。

**CAS（Compare-And-Set）**：每次变更要求提供当前 `{id, revision}` ref，不匹配则抛 `GOAL_STALE_REVISION`。

**wrapup 上下文注入**：Goal complete/blocked 时自动注入 plugin source 的 user 消息（`renderWrapupContext()`），触发总结。

### 4.5 角色分工

**authority 模型**（`packages/goal/tool-goal/src/authority.ts`）：
- `requireDirectHuman()`：只允许直接人类请求调用（`create_goal`、`edit`、`pause`、`resume`）
- `completionAuthority()`：允许 goal-round 内的自动续轮调用（`complete` 和 `blocked`）

**delegation depth**：`resolveChildDepth()` 从 parent 的 delegation depth +1，超过 `maxDepth` 抛 `SubagentDepthError`。

**Provider 注册表**：SubAgentProvider 注册到全局表，能力匹配选择，工具集限制。

### 4.6 失败回流

**round-limit 阻塞**：达到 `maxGoalRounds`（默认 256）时自动 block。

**GOAL_STALE_REVISION**：CAS 不匹配时抛出，防止并发修改。

**GOAL_INVALID_TRANSITION**：resume 时检查轮次预算，不足则拒绝。

**wrapup 注入**：Goal complete/blocked 时自动注入总结提示，避免用户手动要求。

### 4.7 对 laew 的借鉴点

1. **Goal 域模型**：最完整的任务抽象，laew 可借鉴 GoalPhase 状态机（active/paused/blocked/complete）
2. **round driver 自动续轮**：laew 的 Main-Work Agent 拆 WorkFlow 后可借鉴自动续轮机制
3. **maxGoalRounds 硬限制**：防止无限循环（默认 256），laew 应引入类似机制
4. **wrapup 上下文注入**：任务完成/阻塞时自动注入总结提示
5. **Descriptor 持久化**：子 agent 的 persona/toolFilter 跨 crash 恢复

---

## 5. OpenClaw：Swarm FIFO 调度 + 9 步 spawn + 代次管理

### 5.1 委派协议

**spawnSubagentDirect() 9 步流程**（`src/agents/subagents/spawn/subagent-spawn.ts`）：

```
1. 输入验证：task 非空、label 清理、mount path 安全检查
2. Session 准备：createInitialSubagentSession()
3. Context Engine 准备：prepareContextEngineSubagentSpawn()
4. Attachments 材料化：materializeSubagentAttachments()
5. Delivery Context 绑定：mergeDeliveryContext()
6. Gateway 注册：callNativeSubagentGateway()
7. Swarm 容量预留：activateSwarmRun()
8. Launch Request 构建：buildSubagentLaunchRequest()
9. Lifecycle Emitter：createSubagentSpawnLifecycleEmitter()
```

**Spawn 模式**：

```typescript
export const SUBAGENT_SPAWN_MODES = ["run", "session"] as const;
export const SUBAGENT_SPAWN_CONTEXT_MODES = ["isolated", "fork"] as const;
```

**核心入口**：

```typescript
// subagent-spawn.ts:88-144
export async function spawnSubagentDirect(params: SpawnSubagentParams, ctx: SpawnSubagentContext): Promise<SpawnSubagentResult> {
    const requestResolution = resolveSubagentSpawnRequest(params, ctx, { initial: requestedAgentId, applyDefault(agentId) { requestedAgentId = agentId; return requestedAgentId; } });
    if (!requestResolution.ok) return requestResolution.result;
    const { request, runtime, swarm, admission, childIdem } = requestResolution.resolved;
    // ... 9 步流程
}
```

### 5.2 上下文传递

**createInitialSubagentSession**：创建子 agent 的初始 session，绑定 `completionOwnerSessionKey`、`inheritedToolAllowlist/Denylist`、`swarmGroupId`。

**prepareContextEngineSubagentSpawn**：准备子 agent 的 context engine（`lightContext: true` 可跳过）。

**materializeSubagentAttachments**：将附件材料化到文件系统，注入 `systemPromptSuffix`。

**mergeDeliveryContext**：合并 delivery context（channel/accountId/to/threadId）。

### 5.3 并发调度

**SwarmGroupLane FIFO 队列**（`src/agents/subagents/swarm/swarm-scheduler.ts`）：

```typescript
type SwarmGroupLane = {
    groupId: string;            // 调度组标识
    limit: number;              // 最大并发数
    active: Set<string>;        // 当前活跃 run IDs
    queue: QueuedSwarmRun[];    // FIFO 等待队列
    pumpScheduled: boolean;     // 防止并发 pump
};
```

**调度流程**：

```typescript
// swarm-scheduler.ts:149-164 - reserveSwarmRun（占位）
export function reserveSwarmRun(params: { groupId, runId, maxConcurrent, activeRunIds }): boolean {
    const lane = ensureLane(params);
    if (runLocations.has(params.runId)) { deleteLaneIfIdle(lane); return false; }
    const item: QueuedSwarmRun = { runId: params.runId, holds: 0, retryReady: true };
    lane.queue.push(item);
    runLocations.set(params.runId, { lane, state: "queued", item });
    return true;
}

// swarm-scheduler.ts:199-213 - activateSwarmRun（绑定启动）
export function activateSwarmRun(params: { groupId, runId, start, onStartFailure }): void {
    const location = runLocations.get(params.runId);
    if (!location || location.state !== "queued" || location.lane.groupId !== params.groupId) throw new Error(`swarm scheduler reservation missing for run ${params.runId}`);
    const { lane, item } = location;
    item.launch = { start: params.start, onStartFailure: params.onStartFailure };
    publishCapacityChange(item);
    pumpLane(lane);
}

// swarm-scheduler.ts:96-112 - pumpLane（容量检查 + 微任务调度）
function pumpLane(lane: SwarmGroupLane): void {
    if (lane.pumpScheduled) return;
    lane.pumpScheduled = true;
    queueMicrotask(() => {
        lane.pumpScheduled = false;
        while (lanes.get(lane.groupId) === lane && lane.active.size < lane.limit) {
            const next = lane.queue[0];
            if (!next?.launch || !next.retryReady || next.holds > 0) return;
            lane.queue.shift();
            void startQueuedRun(lane, next, next.launch);
        }
    });
}
```

**失败回退队头 + 1s 重试**：

```typescript
// swarm-scheduler.ts:53-94 - startQueuedRun
async function startQueuedRun(lane: SwarmGroupLane, item: QueuedSwarmRun, launch: SwarmLaunch): void {
    lane.active.add(item.runId);
    try {
        await launch.start();
    } catch (error) {
        let failurePersisted = false;
        try { failurePersisted = await launch.onStartFailure(error); } catch { /* durable queued row still owns this work */ }
        if (failurePersisted) { releaseSwarmRun(item.runId); return; }
        const previouslyFull = lane.active.size >= lane.limit;
        lane.active.delete(item.runId);
        item.retryReady = false;
        lane.queue.unshift(item);  // 放回队头
        const timer = setTimeout(() => { item.retryReady = true; pumpLane(lane); }, isFastTestRuntimeEnv() ? 1 : 1_000);
        timer.unref?.();
    }
}
```

### 5.4 通信方式

**Lifecycle Emitter**（`src/agents/subagents/registry/subagent-lifecycle-events.ts`）：

```typescript
// 终态事件
ended reason: subagent-complete / subagent-error / subagent-killed
outcome:      ok / error / timeout / killed
```

**terminal completion lock**：`acquireTerminalCompletionLock` 保证同一 runId 只有一个 terminal completion 在处理，通过 Promise 链实现串行化。

**generation 代次管理**：`generation` 字段防止旧代次的 run 操作新代次的 session。`newerGenerationOwnsSession()` 检查是否有更新代次的 run 已接管同一 session。

### 5.5 角色分工

**三态 Agent**：
1. **内置 Agent**：框架提供的核心能力
2. **CLI Agent**：外部 CLI 工具包装
3. **ACP Agent**：通过 Agent Communication Protocol 连接的远程 Agent

**AgentHarness 11 capability**：

```typescript
export type AgentHarness = AgentHarnessRunCapability & AgentHarnessSideQuestionCapability &
    AgentHarnessClassificationCapability & AgentHarnessCompactionCapability &
    AgentHarnessRuntimeArtifactCapability & AgentHarnessAuthBindingCapability &
    AgentHarnessProviderUsageCapability & AgentHarnessModelCatalogCapability &
    AgentHarnessMcpCatalogCapability & AgentHarnessSessionForkCapability &
    AgentHarnessSessionLifecycleCapability;
```

**Harness 选择算法**（`src/agents/harness/selection.ts`）：

```typescript
// selectAgentHarness() 核心逻辑
// 1. 遍历所有已注册 harness（listRegisteredAgentHarnesses()）
// 2. 对每个调用 harness.supports(context) 判定兼容性
// 3. compareHarnessSupport() 按 priority 降序排序
// 4. 内置 "openclaw" harness 是兜底（builtin-openclaw.ts 创建）
```

### 5.6 失败回流

**Run 4 种终态**：`completed` / `aborted` / `blocked` / `error`，每种有独立的诊断映射。

**Harness fallback**：`supports()` 返回 `{ supported: false, fallbackRuntime?: "openclaw" }` 可降级到内置 harness。

**exec auto-review 失败回流**：任何审阅异常都回退到 `ask`（人工审批），永远不静默放行。

**AbortSignal 传播链**：外部信号 → run 级 AbortController → tool 执行 AbortSignal。

**cleanupFailedSpawnBeforeAgentStart**：spawn 失败时清理 provisional session、attachment 目录、emit lifecycle hooks。

### 5.7 对 laew 的借鉴点

1. **Swarm FIFO 调度**：laew 的 hard 任务并行子任务可借鉴 `SwarmGroupLane` 的 reserve→activate→release 流程
2. **失败回退队头 + 1s 重试**：启动失败时放回队头延迟重试，避免单次失败阻塞整个队列
3. **pumpLane 微任务调度**：`queueMicrotask()` 防止并发 pump，laew 可借鉴
4. **generation 代次管理**：防止旧代次 run 操作新代次 session
5. **exec auto-reviewer**：命令执行的风险分类（4 级），laew 的 BashTool 可借鉴

---

## 6. OpenCode：Agent 类型系统 + Session 状态机 + doom_loop

### 6.1 委派协议

**Agent 类型系统**（`packages/opencode/src/agent/agent.ts`）：

```typescript
export const Info = Schema.Struct({
    name: Schema.String,
    description: Schema.optional(Schema.String),
    mode: Schema.Literals(["subagent", "primary", "all"]),
    native: Schema.optional(Schema.Boolean),
    hidden: Schema.optional(Schema.Boolean),
    permission: PermissionV1.Ruleset,
    model: Schema.optional(Schema.Struct({ modelID: ModelV2.ID, providerID: ProviderV2.ID })),
    prompt: Schema.optional(Schema.String),
    steps: Schema.optional(Schema.Finite),
}).annotate({ identifier: "Agent" })
```

**7 种内置 Agent**（`agent.ts:140-265`）：

| Agent | mode | 工具权限 | 定位 | 对应 laew 角色 |
|-------|------|---------|------|---------------|
| `build` | primary | 全量 | 默认主 Agent | Yolo + MainWork |
| `plan` | primary | 只读 + 计划文件 | Plan 模式 | Plan |
| `general` | subagent | 全量（无 todo） | 通用子任务 | SubAgent(worker) |
| `explore` | subagent | 只读探索 | 快速代码探索 | SubAgent(explore) |
| `compaction` | primary (hidden) | 无 | 上下文压缩 | SessionContext |
| `title` | primary (hidden) | 无 | 标题生成 | 无对应 |
| `summary` | primary (hidden) | 无 | 摘要生成 | SessionContext |

**TaskTool 委派**（`packages/opencode/src/tool/task.ts`）：

```typescript
// task.ts:92-200 - TaskTool.run
export const run = Effect.fn("TaskTool.run")(function* (params: TaskToolParams) {
    // 1. 深度限制检查
    const depth = yield* computeDepth(params.parentID)
    if (depth >= (cfg.subagent_depth ?? 1)) return Err.maxDepth()
    // 2. 创建 child session
    const childSession = yield* sessions.create({ parentID, title, agent, permission: deriveSubagentSessionPermission(...) })
    // 3. 构建 prompt
    const prompt = yield* ops.resolvePromptParts(params.prompt)
    // 4. 后台/前台执行
    if (params.background) {
        yield* background.start(childSession.id, prompt)
        return Ok({ sessionID: childSession.id, state: "background" })
    }
    const result = yield* background.wait(childSession.id, prompt)
    // 5. 结果聚合
    return Ok(renderOutput(childSession.id, result))
})
```

### 6.2 上下文传递

**child session 仅传 task prompt**：子 agent 不继承 parent 的完整对话历史，只接收任务提示词。

**权限继承**（`packages/opencode/src/agent/subagent-permissions.ts`）：

```typescript
export function deriveSubagentSessionPermission(parent: Permission.Ruleset, subagent: Permission.Ruleset): Permission.Ruleset {
    // 继承 parent 的 external_directory 和 deny 规则
    // 默认禁止 todowrite 和 task（除非 subagent 自己的 ruleset 显式允许）
    return Permission.merge(parentExternalDirectory(parent), parentDeny(parent), subagent, {
        todowrite: "deny",
        task: { general: "deny" },  // 禁止子 agent 再派发子任务
    })
}
```

**Session 状态机**（`packages/opencode/src/effect/runner.ts`）：

```typescript
type State<A, E> =
    | { _tag: "Idle" }
    | { _tag: "Running"; run: RunHandle<A, E> }
    | { _tag: "Shell"; shell: ShellHandle<A, E> }
    | { _tag: "ShellThenRun"; shell: ShellHandle<A, E>; run: PendingHandle<A, E> }

export const ensureRunning = Effect.fn("Runner.ensureRunning")(function* (work: Work<A, E>) {
    switch (ctx.state._tag) {
        case "Idle": return yield* start(work)
        case "Running":
        case "ShellThenRun": return yield* awaitDone()  // 合并/排队
        case "Shell": return yield* queueAsShellThenRun(work)
    }
})
```

### 6.3 并发调度

**per-session 串行**：每个 session 有独立的 `Runner`，同一 session 内严格串行（`ensureRunning` 合并/排队）。

**跨 session 并行**：子 agent 在各自 session 中通过 Effect fiber 并发执行。

**BackgroundJob 注册表**（`packages/core/src/background-job.ts`）：

```typescript
const jobs = new Map<string, Active>()

export const start = Effect.fn("BackgroundJob.start")(function* (id: string, work: Work<unknown>) {
    const scope = yield* Scope.fork(Effect.scope)
    const deferred = yield* Deferred.make<unknown>()
    jobs.set(id, { id, scope, deferred, status: "pending" })
    // fork work into scope
    return { id, deferred }
})

export const extend = Effect.fn("BackgroundJob.extend")(function* (id: string, work: Work<unknown>) {
    const job = jobs.get(id)
    if (!job) return Err.noSuchJob(id)
    // resume existing background job with new work
})
```

### 6.4 通信方式

**Effect 服务调用**：所有跨协调都通过 Effect services + `EventV2Bridge`：
- `SessionStatus.Service` — 发布 `Status` / `Idle` 事件
- `SessionCompaction.Service` — 发布 `Compacted` 事件
- `Command.Event.Executed` — 命令完成

**结果聚合**：TaskTool 通过 `renderOutput` 返回 XML 形状文本块：

```typescript
// tool/task.ts:64-79
function renderOutput(sessionID: string, result: BackgroundJobResult): string {
    return `<task id="${sessionID}" state="${result.state}">
<summary>${result.summary}</summary>
<task_result|task_error>${result.text}</task_result|task_error>
</task>`
}
```

### 6.5 角色分工

**固定 + 动态角色**：
- 固定（native）7 种：build/plan/general/explore/compaction/title/summary
- 动态（user-defined）：config agents 合并（`agent.ts:267-294`）
- 生成式：`Agent.generate` 用 LLM 从描述生成 agent config

**Permission Ruleset**：每个 `Agent.Info.permission: PermissionV1.Ruleset`，通过 `Permission.merge(defaults, agentSpecific, userConfig)` 合并。

**权限评估**（`packages/opencode/src/permission/index.ts`）：

```typescript
export function evaluate(permission: string, pattern: string, ...rulesets: Ruleset[]): Rule {
    return rulesets.flat().findLast(rule =>
        Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern)
    ) ?? { action: "ask", permission, pattern: "*" }
}
```

后匹配优先（`findLast`），允许后面的规则覆盖前面的。

### 6.6 失败回流

**doom_loop 检测**（`packages/opencode/src/session/processor.ts`）：

```typescript
const DOOM_LOOP_THRESHOLD = 3

// processor.ts:331-381
// 检测条件同时满足 4 项：
// 1. 连续性：最近 3 个 tool part 必须连续（同一 assistant 消息内）
// 2. 同工具：3 次调用的 part.tool 完全相同
// 3. 同参数：3 次调用的 part.state.input 通过 JSON.stringify 严格相等
// 4. 非 pending：3 次调用的状态都不是 "pending"

if (recentParts.length !== DOOM_LOOP_THRESHOLD || !recentParts.every(part =>
    part.type === "tool" && part.tool === value.name &&
    part.state.status !== "pending" &&
    JSON.stringify(part.state.input) === JSON.stringify(input))) {
    return  // 不触发
}
// 触发 doom_loop 权限检查
yield* permission.ask({ permission: "doom_loop", patterns: [value.name], ... })
```

**retry Schedule**（`packages/opencode/src/session/retry.ts`）：

```typescript
export const policy: Schedule<unknown> = Schedule.retryable<unknown>({
    delay: RETRY_INITIAL_DELAY,     // 2000ms
    factor: BACKOFF_FACTOR,         // 2
    maxRetries: MAX_RETRIES,        // 5
    jitter: JITTER,                 // 0.25
    maxDelay: MAX_DELAY_NO_HEADERS, // 30s
    while: retryable,               // 429/5xx/overloaded/rate-limit/network
    onRetry: (error, attempt) => Effect.gen(function* () {
        yield* status.set({ type: "retry", attempt, message: ..., action: ..., next: ... })
    }),
})
```

**compaction on overflow**：`isOverflow` 检测 token 数 vs usable context（context limit - 20k reserve），触发 `compaction.process`。

### 6.7 对 laew 的借鉴点

1. **doom_loop 检测**：精确重复 3 次同工具同参数 → 权限请求，laew 的 SubAgent-Work 可借鉴
2. **Session 状态机**：`Idle/Running/Shell/ShellThenRun` 比 laew 的简单 while 更可控
3. **BackgroundJob 注册表**：支持 `extend` 恢复已有后台 job（`task_id` resume 路径）
4. **Permission Ruleset**：后匹配优先的权限评估，laew 的工具集限制可借鉴
5. **retry Schedule**：指数退避 + jitter + retry-after 头解析

---

## 7. Pi：Lane 并发树 + 三队列 + Session Tree 分叉

### 7.1 委派协议

**AgentLane 接口**（`packages/agent/src/harness/agent-harness.ts`）：

```typescript
export interface AgentLane {
    readonly name: string;
    getLeafId(): Promise<string | null>;
    prompt(text: string, images?: ImageContent[]): Promise<RunResult>;
    skill(name: string, additionalInstructions?: string): Promise<RunResult>;
    compact(options?: { customInstructions?: string }): Promise<CompactionResult>;
    navigateTree(targetId: string | null, options?: NavigateOptions): Promise<NavigationResult>;
    resume(): Promise<ResumeResult>;
    abort(): Promise<AbortResult>;
    steer(text: string, images?: ImageContent[]): Promise<QueueResult>;     // 注入调整
    followUp(text: string, images?: ImageContent[]): Promise<QueueResult>;  // 追加后续
    nextRun(text: string, images?: ImageContent[]): Promise<QueueResult>;   // 触发下一轮
    cancelQueued(entryId: string): Promise<CancelQueuedResult>;
    waitForIdle(): Promise<void>;
    runWhenIdle(callback: () => void | Promise<void>): Promise<void>;
    peekAction(): Promise<ActionInfo | undefined>;   // manual drive
    executeAction(): Promise<ActionInfo | undefined>; // manual drive
    runToCompletion(): Promise<void>;
    readonly session: SessionTree;
    watch(): Promise<WatchHandle<LaneSnapshot>>;
}
```

**Lane 创建/分叉**（`packages/agent/src/harness/session/session.ts`）：

```typescript
async createLane(lane: string, at: string | null): Promise<void> {
    await this.storage.createLane(lane, at);  // 在 entry `at` 处分叉
}
async moveLane(lane: string, to: string | null): Promise<void> {
    await this.storage.moveLane(lane, to);   // 移动 lane 的 leaf 指针
}
```

### 7.2 上下文传递

**Lane 隔离**：通过 `view(lane)` 实现——每个 lane 只看到自己分支的 session tree：

```typescript
view(lane: string): SessionTree {
    if (lane === "main") return this;
    return {
        getLeafId: () => this.getLeafIdForLane(lane),
        getEntry: (id) => this.getEntry(id),
        findEntriesOnBranch: (query) => this.queryBranchEntries(lane, query),
        appendMessage: (message) => this.appendMessageToLane(lane, message),
        ...
    };
}
```

**branch-scoped context**（`packages/agent/src/harness/session/context.ts`）：

```typescript
export function buildSessionContext(pathEntries: readonly Entry[], options = {}): SessionContext {
    const state = deriveSessionContextState(pathEntries);  // 从 branch 推导 model/thinking/tools
    const contextEntries = buildContextEntries(pathEntries, options);
    const messages = contextEntries.flatMap((entry, index) =>
        sessionEntryToContextMessages(entry, index, contextEntries, options)
    );
    return { ...state, messages };
}
```

### 7.3 并发调度

**Lane 并发（1 操作/lane）**：

```typescript
export class LaneBusy extends TaggedError("LaneBusy")<{
    lane: string;
    operationId: string;
    operationKind: "run" | "compaction" | "navigation";
    message: string;
}> {}
```

**三队列模型**（`packages/agent/src/types.ts` + `agent.ts`）：

```typescript
export type QueueMode = "all" | "one-at-a-time";

class PendingMessageQueue {
    private messages: AgentMessage[] = [];
    public mode: QueueMode;
    enqueue(message: AgentMessage): void { this.messages.push(message); }
    drain(): AgentMessage[] {
        if (this.mode === "all") {
            const drained = this.messages.slice();
            this.messages = [];
            return drained;
        }
        const first = this.messages[0];
        if (!first) return [];
        this.messages = this.messages.slice(1);
        return [first];
    }
}

// Agent 类中三队列
private readonly steeringQueue: PendingMessageQueue;   // steer：注入调整
private readonly followUpQueue: PendingMessageQueue;   // followUp：追加后续
// nextRun items 在 run 启动时捕获进 initialMessages
```

**三队列协议**（`packages/agent/src/agent-loop.ts`）：

```typescript
async function runLoop(...): Promise<void> {
    let pendingMessages: AgentMessage[] = (await config.getSteeringMessages?.()) || [];
    while (true) {
        let hasMoreToolCalls = true;
        while (hasMoreToolCalls || pendingMessages.length > 0) {
            // 注入 pending messages（steer）
            if (pendingMessages.length > 0) {
                for (const message of pendingMessages) {
                    currentContext.messages.push(message);
                    newMessages.push(message);
                }
                pendingMessages = [];
            }
            // 流式 assistant response，执行 tool calls...
            pendingMessages = (await config.getSteeringMessages?.()) || [];
        }
        // Agent 将停止，检查 follow-up
        const followUpMessages = (await config.getFollowUpMessages?.()) || [];
        if (followUpMessages.length > 0) {
            pendingMessages = followUpMessages;
            continue;
        }
        break;
    }
}
```

### 7.4 通信方式

**steer / followUp / nextRun 队列**：

```typescript
// agent-session.ts
private async _queueSteer(text: string, images?: ImageContent[]): Promise<void> {
    this._steeringMessages.push(text);
    this._emitQueueUpdate();
    this.agent.steer({ role: "user", content: [...], timestamp: Date.now() });
}

private async _queueFollowUp(text: string, images?: ImageContent[]): Promise<void> {
    this._followUpMessages.push(text);
    this._emitQueueUpdate();
    this.agent.followUp({ role: "user", content: [...], timestamp: Date.now() });
}
```

**Deferred Handle**（`packages/ai/src/types.ts`）：

```typescript
export interface DeferredHandle {
    provider: string;
    modelId: string;
    api: string;
    id: string;
    expiresAt?: number;
    pollAfterMs?: number;
    data?: JsonValue;
}
```

**LaneSnapshot 观察**：

```typescript
export interface LaneSnapshot {
    lane: string;
    transcript: Entry[];
    leafId: string | null;
    operation: LaneInfo["operation"];
    queues: { steer: QueuedItem[]; followUp: QueuedItem[]; nextRun: QueuedItem[] };
    pendingWrites: { id: string; entry: ProvisionedEntry }[];
    faulted: boolean;
}
```

### 7.5 角色分工

**drive 模式**（`agent-harness.ts`）：

```typescript
export interface AgentHarnessOptions {
    drive?: "automatic" | "manual";
    steeringMode?: QueueMode;   // 默认 "one-at-a-time"
    followUpMode?: QueueMode;   // 默认 "one-at-a-time"
    toolExecution?: "sequential" | "parallel";
}
```

- `"automatic"`：Harness 自动运行 agent loop 直到完成
- `"manual"`：外部代码逐步驱动（`peekAction()` → `executeAction()`），TUI 可在每个 action 之间插入 UI 更新

**ActionInfo 可归约动作机**：

```typescript
export type ActionInfo =
    | { kind: "append_entry"; entryType: Entry["type"]; entryId: string }
    | { kind: "move_lane"; to: string | null }
    | { kind: "try_finish_run"; outcome: "completed" | "failed" }
    | { kind: "commit_follow_up" }
    | { kind: "consume_queue_item"; queue: "steer" | "followUp"; entryId: string }
    | { kind: "stream_assistant"; step: "assistant" | "compaction" | "branch_summary"; attempt: number }
    | { kind: "execute_tool"; toolCallId: string; toolName: string }
    | { kind: "fetch_deferred" | "cancel_deferred"; provider: string; id: string }
    | { kind: "hook"; name: HookName }
    | { kind: "sleep"; delayMs: number };
```

**Skill 注入**（`packages/agent/src/harness/skills.ts`）：

```typescript
export function formatSkillInvocation(skill: Skill, additionalInstructions?: string): string {
    const skillBlock = `<skill name="${skill.name}" location="${skill.filePath}">
References are relative to ${dirnameEnvPath(skill.filePath)}.
${skill.content}
</skill>`;
    return additionalInstructions ? `${skillBlock}\n\n${additionalInstructions}` : skillBlock;
}
```

### 7.6 失败回流

**LaneBusy 错误**：同一 lane 同时只有一个操作运行，违反时抛出。

**suspended → resume 恢复**：

```typescript
export interface SuspendedOperation {
    lane: string;
    kind: "run" | "compaction" | "navigation";
    id: string;
    startedAt: number;
    reason: "crash" | "deferred";
    prompt?: AgentMessage[];
    deferred?: DeferredHandle;
    aborting?: { steer: AgentMessage[]; followUp: AgentMessage[] };
    missing: { tools: string[]; models: string[] };
}
```

**branch summary（公共祖先算法）**（`packages/coding-agent/src/core/compaction/branch-summarization.ts`）：

```typescript
export function collectEntriesForBranchSummary(session: ReadonlySessionManager, oldLeafId: string | null, targetId: string): CollectEntriesResult {
    if (!oldLeafId) return { entries: [], commonAncestorId: null };
    const oldPath = new Set(session.getBranch(oldLeafId).map(e => e.id));
    const targetPath = session.getBranch(targetId);
    let commonAncestorId: string | null = null;
    for (let i = targetPath.length - 1; i >= 0; i--) {
        if (oldPath.has(targetPath[i].id)) { commonAncestorId = targetPath[i].id; break; }
    }
    const entries: SessionEntry[] = [];
    let current: string | null = oldLeafId;
    while (current && current !== commonAncestorId) {
        const entry = session.getEntry(current);
        if (!entry) break;
        entries.push(entry);
        current = entry.parentId;
    }
    entries.reverse();
    return { entries, commonAncestorId };
}
```

**RecordLogCorruption**（`packages/agent/src/harness/reducer.ts`）：

```typescript
export type RecordLogCorruptionReason =
    | "multiple_open_operations"
    | "unknown_operation"
    | "record_after_finish"
    | "non_consecutive_attempt"
    | "invalid_compaction_reason"
    | "queue_after_abort"
    | "invalid_queue_cancellation"
    | "inconsistent_step"
    | "tool_call_mismatch"
    | "duplicate_tool_invocation"
    | "provisioned_entry_mismatch"
    | "invalid_deferred_handle";
```

### 7.7 对 laew 的借鉴点

1. **Lane 并发**：独立的执行通道，laew 的 hard 任务并行子任务可用 Lane 实现
2. **三队列模型**：steer（注入调整）/ followUp（追加后续）/ nextRun（触发下一轮），laew 的 Main-Work Agent 可借鉴
3. **Session Tree 分叉**：session 不是线性历史而是树，支持分支探索
4. **branch summary**：导航时自动摘要废弃分支（公共祖先算法）
5. **peekAction/executeAction**：manual drive 模式，TUI 可逐步驱动

---

## 8. 设计模式提炼

### 模式 1：工具调用式委派 vs 编排器直调

| 类型 | 代表 | 优势 | 劣势 | 适合场景 |
|------|------|------|------|---------|
| **工具调用式** | AtomCode TaskTool、OpenCode TaskTool、Claude Code AgentTool | 模型自主决定委派时机和参数 | 依赖模型能力，可能误判 | 灵活场景、模型强时 |
| **编排器直调** | laew MultiAgentOrchestrator、DeepSeek round driver | 流程显式可控 | 灵活性差、扩展困难 | 高可靠性场景 |
| **混合式** | OpenClaw spawnSubagentDirect + Harness | 编排器提供框架，模型选择 harness | 复杂度中等 | 平台型产品 |

**结论**：laew 的编排器直调适合高可靠性场景，但可吸收工具调用式的灵活性——在 hard 级别允许模型通过工具调用发起子任务。

### 模式 2：上下文传递的四种策略

| 策略 | 代表 | 优势 | 劣势 |
|------|------|------|------|
| **全量共享** | Claude Code Fork（cache-safe params） | 信息完整、cache 命中 | 隐私泄露、冲突 |
| **裁剪/精简** | OpenCode（仅传 task prompt）、AtomCode（persona 注入） | 隔离好、token 节省 | 信息丢失 |
| **层级继承** | DeepSeek（preset 组合 + delegationDepth） | 职责清晰、可恢复 | 实现复杂 |
| **独立上下文** | laew（Agent-Context + Agent-Memory） | 完全隔离 | 信息不共享 |

**结论**：laew 的独立上下文 + Agent-Memory 持久化是正确方向。可借鉴 DeepSeek 的 Descriptor 持久化，让子 agent 的 persona/toolFilter 跨 crash 恢复。

### 模式 3：并发调度的三种模型

| 模型 | 代表 | 并发粒度 | 实现复杂度 |
|------|------|---------|-----------|
| **Semaphore 容量** | AtomCode（默认 3）、OpenClaw（默认 8） | 子 agent 级 | 中 |
| **Lane 隔离** | Pi | 执行通道级 | 中 |
| **Session 状态机** | OpenCode（Idle/Running/Shell） | Session 级 | 低 |
| **round driver 串行** | DeepSeek | 轮次级 | 低 |
| **无并发** | laew | 无 | 最低 |

**结论**：laew 的串行执行是最大短板。P1 应引入 Semaphore 并发（hard 任务），P2 可考虑 Lane 模型。

### 模式 4：通信方式的三种范式

| 范式 | 代表 | 优势 | 劣势 |
|------|------|------|------|
| **事件流** | AtomCode TeamEvent、DeepSeek Cordis 事件 | 解耦、可追踪 | 调试困难 |
| **消息传递** | Claude Code SendMessageTool + mailbox | 异步、持久化 | 文件 I/O 开销 |
| **返回值** | OpenCode XML tool-result、laew dep_outputs | 简单直接 | 耦合度高 |

**结论**：laew 的 dep_outputs HashMap 适合当前串行模型。引入并发后应切换到事件流范式。

### 模式 5：角色分工的光谱

| 类型 | 代表 | 角色数 | 扩展性 |
|------|------|--------|--------|
| **固定角色** | laew（6 角色）、OpenCode（7 Agent） | 固定 | 低 |
| **参数化角色** | AtomCode（14 TeamRoleId × 2 permission × 2 difficulty） | 有限组合 | 中 |
| **动态注册** | DeepSeek Provider 注册表、OpenClaw Harness 注册表 | 无限 | 高 |
| **完全动态** | Claude Code（hooks + skills + plugins + MCP） | 无限 | 最高 |

**结论**：laew 的 6 固定角色适合当前阶段。长期可借鉴 AtomCode 的参数化角色（难度 × 工具集 × 角色）。

### 模式 6：质量门控的三种实现

| 实现 | 代表 | 触发方式 | 处理方式 |
|------|------|---------|---------|
| **独立 QC Agent** | laew Quality-Check Agent | 每个 SubAgent 必经 | LLM 判定 Pass/Fail/Retry |
| **Hook 拦截** | Claude Code PreToolUse/PostToolUse | 工具调用前后 | 可编程拦截 |
| **纪律注入** | AtomCode VerifyCadenceHook | 编辑后检测 | 注入 nudge 续接 |
| **权限请求** | OpenCode doom_loop | 精确重复 3 次 | 用户审批 |

**结论**：laew 的独立 QC Agent 是最完整的质量门控。可补充 AtomCode 的 VerifyCadenceHook（编辑后验证纪律）和 OpenCode 的 doom_loop 检测。

### 模式 7：失败回流的多层熔断

| 层级 | 触发条件 | 处理方式 | 参考项目 |
|------|---------|---------|---------|
| 警告层 | 连续 3 次相同工具+参数 | 注入 nudge 提醒 | AtomCode NUDGE_AT、OpenCode doom_loop |
| 限制层 | 达到 maxRounds（64/256） | 暂停并通知用户 | DeepSeek maxGoalRounds |
| 强制层 | 达到硬上限（128/256） | 强制 Block 并要求用户干预 | DeepSeek round-limit |
| 兜底层 | 分类/解析失败 | 降级为 simple + 通知用户 | laew 当前已有 |

**结论**：多层熔断（警告 → 限制 → 强制停止）是最佳实践。laew 目前只有兜底层，应增加警告层和限制层。

### 模式 8：Descriptor 持久化与冷恢复

| 项目 | 机制 | 持久化内容 |
|------|------|-----------|
| DeepSeek | `subagent/descriptor` session event | mode/one-shot/continuable + persona + toolFilter + provider/model |
| OpenClaw | `SubagentRunRecord` | 完整 run 状态 + generation + kill intent |
| Pi | `operation_started` record + `reduceLaneState` | 从 durable recovery slice 重建 lane 状态 |
| laew | 无 | — |

**结论**：laew 应借鉴 DeepSeek 的 Descriptor 持久化，让子 agent 的 persona/toolFilter 跨 crash 恢复。

---

## 9. 对 laew 的综合建议

### 9.1 当前架构分析

**laew 的 6 角色架构**（`src/agent/orchestrator.rs`）：

```
Yolo Agent (入口层: 分类/意图)
  ↓
Plan Agent (规划层: 仅 hard 任务)
  ↓
Main-Work Agent (流程层: 拆 WorkFlow)
  ↓
SubAgent-Work Agent (执行层: 最小单元)
  ↓
Quality-Check Agent (质检层: 每单元必经)
  ↓
SessionContext Agent (会话层: 汇总写入)
```

**优势**：
- 角色隔离清晰
- 质检门控严格（每单元必经 QC）
- 流程可控
- 失败回流到 Yolo 重新评估

**劣势**：
- **无并发支持**：WorkFlow 列表串行执行
- simple 任务也走完整流程（过重）
- 无结构化事件流
- 无 Descriptor 持久化（crash 后无法恢复子 agent 状态）
- 无 doom_loop 检测

### 9.2 P0 建议：引入 Semaphore 并发（hard 任务）

借鉴 AtomCode 的 Semaphore 并发模型，为 hard 任务的并行子任务引入并发执行：

```rust
// src/agent/orchestrator.rs 新增
use std::sync::Arc;
use tokio::sync::Semaphore;

const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 3;

pub struct OrchestratorConfig {
    pub max_retry_per_level: usize,
    pub history_limit: usize,
    pub subagent_max_iterations: usize,
    pub max_concurrent_subagents: usize,  // 新增
}

// execute_workflows 改为并行
async fn execute_workflows_parallel(
    &self,
    c: &TaskClassification,
    plan: &WorkFlowPlan,
    session: &Session,
) -> std::result::Result<TaskResult, QualityFailure> {
    let sem = Arc::new(Semaphore::new(self.cfg.max_concurrent_subagents));
    let mut set = tokio::task::JoinSet::new();
    let ordered = main_work::topo_sort(&plan.workflows).map_err(...)?;

    for wf in ordered {
        // 检查依赖是否满足
        if !dependencies_met(&wf, &dep_outputs) { continue; }
        let _permit = sem.acquire_owned().await.expect("semaphore not closed");
        let sub_input = build_subflow_input(&wf, &dep_outputs);
        set.spawn(async move {
            let outcome = self.sub_agent.run_unit(&sub_input, session.id()).await;
            (wf.id.clone(), outcome)
        });
    }

    while let Some(res) = set.join_next().await {
        // 收集结果...
    }
}
```

**工作量**：200-300 行
**影响**：hard 任务执行时间减少 50-70%

### 9.3 P0 建议：增加 doom_loop 检测

借鉴 OpenCode 的 doom_loop 检测：

```rust
// src/agent/subagent.rs 新增
const DOOM_LOOP_THRESHOLD: usize = 3;

fn detect_doom_loop(recent_calls: &[ToolCall]) -> bool {
    if recent_calls.len() < DOOM_LOOP_THRESHOLD { return false; }
    let last_n = &recent_calls[recent_calls.len() - DOOM_LOOP_THRESHOLD..];
    last_n.iter().all(|c| {
        c.tool == last_n[0].tool && c.args == last_n[0].args
    })
}

// 在 SubAgentRunner::run_unit 中
if detect_doom_loop(&recent_calls) {
    return Err(SubAgentError::DoomLoopDetected);
}
```

**工作量**：50-80 行
**影响**：防止无意义循环

### 9.4 P0 建议：增加结构化事件流

借鉴 AtomCode 的 TeamEvent 六元事件：

```rust
// src/agent/events.rs 新增
#[derive(Debug, Clone, Serialize)]
pub enum OrchestratorEvent {
    RunStarted { goal: String, task_level: TaskLevel },
    AgentStarted { role: AgentRole, goal: String },
    AgentFinished { role: AgentRole, success: bool, summary: String },
    WorkflowStarted { id: String, name: String },
    WorkflowFinished { id: String, success: bool, verdict: Verdict },
    RunFinished { goal: String, total_usage: Usage },
}

// 在 orchestrator.rs 中 emit 事件
self.emit_event(OrchestratorEvent::AgentStarted { role: AgentRole::SubAgent, goal: c.goal_summary.clone() });
```

**工作量**：100-150 行
**影响**：可观测性大幅提升，为 TUI 实时进度提供数据源

### 9.5 P1 建议：借鉴 Descriptor 持久化

借鉴 DeepSeek 的 Descriptor 设计：

```rust
// src/agent/subagent.rs 新增
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentDescriptor {
    pub version: u32,
    pub mode: SubAgentMode,  // OneShot / Continuable
    pub label: String,
    pub persona: String,
    pub tool_allowlist: Option<Vec<String>>,
    pub tool_denylist: Option<Vec<String>>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

// 持久化到 agent_memory 表
db.insert_agent_memory(session_id, role, "descriptor", &serde_json::to_string(&descriptor).unwrap())?;
```

**工作量**：100-150 行
**影响**：crash 后可恢复子 agent 状态

### 9.6 P1 建议：增加多层熔断机制

借鉴各项目的熔断设计：

```rust
// src/agent/circuit_breaker.rs 新增
pub struct CircuitBreaker {
    recent_calls: VecDeque<ToolCallRecord>,
    round_count: u32,
    nudge_at: u32,     // 3
    limit_at: u32,     // 64
    stop_at: u32,      // 128
}

pub enum CircuitBreakerAction {
    Continue,
    Nudge(String),     // 注入警告消息
    Pause(String),     // 暂停并通知用户
    Block(String),     // 强制阻塞
}
```

**工作量**：150-200 行
**影响**：防止无意义循环和无限执行

### 9.7 P1 建议：SubAgent scope 约束

借鉴 AtomCode 的 `WorkerScopeGate`：

```rust
// src/agent/subagent.rs 新增
pub struct SubAgentScope {
    pub allowed_write_patterns: Vec<String>,  // glob 模式
    pub denied_paths: Vec<String>,            // 敏感路径
}

// 默认敏感路径列表
const DEFAULT_DENIED_PATHS: &[&str] = &[
    ".env", ".env.*", "id_rsa", "*.pem", "*.key",
    "credentials", "~/.ssh", "~/.aws", ".git/",
];
```

**工作量**：100-150 行
**影响**：安全性提升

### 9.8 P2 建议：引入 Lane 并发模型

借鉴 Pi 的 Lane 并发：

```rust
// src/agent/lane.rs 新增
pub struct Lane {
    pub id: String,
    pub context: AgentContext,
    pub tools: Vec<Tool>,
    pub status: LaneStatus,  // Idle / Running / Completed / Failed
    pub queues: LaneQueues,  // steer / followUp / nextRun
}

pub struct LaneQueues {
    steer: VecDeque<AgentMessage>,
    followUp: VecDeque<AgentMessage>,
    nextRun: VecDeque<AgentMessage>,
}
```

**工作量**：500-800 行
**影响**：支持更复杂的并发模式

### 9.9 P2 建议：引入 Goal 域模型

借鉴 DeepSeek 的 Goal 域模型：

```rust
// src/agent/goal.rs 新增
pub struct Goal {
    id: String,
    status: GoalPhase,  // Active / Paused / Blocked / Complete
    objective: String,
    parent: Option<Box<Goal>>,
    children: Vec<Goal>,
    rounds: u32,
    max_rounds: u32,
}

pub enum GoalPhase {
    Active,
    Paused,
    Blocked { reason: String },
    Complete,
}
```

**工作量**：200-300 行
**影响**：任务生命周期管理

### 9.10 汇总：建议优先级矩阵

| 优先级 | 建议 | 难度 | 收益 | 参考项目 | 预估工作量 |
|--------|------|------|------|---------|-----------|
| P0 | 引入 Semaphore 并发（hard 任务） | 中 | 高 | AtomCode | 3-5 天 |
| P0 | 增加 doom_loop 检测 | 低 | 中 | OpenCode | 1 天 |
| P0 | 增加结构化事件流 | 低 | 高 | AtomCode | 2-3 天 |
| P1 | 借鉴 Descriptor 持久化 | 中 | 中 | DeepSeek | 2-3 天 |
| P1 | 增加多层熔断机制 | 低 | 高 | 全部 6 个项目 | 2-3 天 |
| P1 | SubAgent scope 约束 | 中 | 高 | AtomCode | 2-3 天 |
| P2 | 引入 Lane 并发模型 | 高 | 高 | Pi | 1-2 周 |
| P2 | 引入 Goal 域模型 | 中 | 高 | DeepSeek | 3-5 天 |

---

## 附录 A：laew 当前编排现状详设

### A.1 MultiAgentOrchestrator 结构

```rust
// src/agent/orchestrator.rs
pub struct MultiAgentOrchestrator {
    yolo: YoloRunner,
    plan: PlanRunner,
    main_work: MainWorkRunner,
    sub_agent: SubAgentRunner,
    quality: QualityRunner,
    session_context: SessionContextRunner,
    db: Arc<Db>,
    cfg: OrchestratorConfig,
}

pub struct OrchestratorConfig {
    pub max_retry_per_level: usize,       // 默认 3
    pub history_limit: usize,             // 默认 3
    pub subagent_max_iterations: usize,   // 默认 16
}
```

### A.2 主流程

```rust
// src/agent/orchestrator.rs:150-248
pub async fn handle(&self, session: &mut Session) -> Result<OrchestrationOutcome> {
    // 0) 项目上下文首次注入（幂等）
    project_context::inject_once(session, work_dir);
    // 0.1) 历史 Session 摘要注入（幂等）
    inject_history_with_entries(session, &summaries);
    // 1) Yolo 入口
    let mut classification = self.run_yolo_classification(session).await?;
    loop {
        // 2) 调度执行
        let exec_result = match classification.task_level {
            TaskLevel::Simple => self.run_simple(&classification, session).await,
            TaskLevel::Medium => self.run_medium(&classification, session).await,
            TaskLevel::Hard => self.run_hard(&classification, session).await,
        };
        match exec_result {
            Ok(task_result) => {
                // 3) SessionContext 收口
                let summary = self.session_context.summarize(...).await?;
                return Ok(OrchestrationOutcome::Executed { result: task_result });
            }
            Err(failure) => {
                if !failure.retryable {
                    // 升级到上一层（由 Yolo 重新评估）
                    classification = self.run_yolo_with_failure(&classification, &failure, session).await?;
                }
                continue;
            }
        }
    }
}
```

### A.3 失败回流

```rust
// src/agent/orchestrator.rs:539-559
async fn run_yolo_with_failure(&self, prev: &TaskClassification, failure: &QualityFailure, session: &mut Session) -> Result<TaskClassification> {
    let failure_msg = format!(
        "[PREVIOUS_FAILURE]\n源: {}\n任务级别: {}\n原目标: {}\n失败原因: {}\n建议: {}\n请重新评估:...",
        failure.source.as_str(), prev.task_level.as_str(), prev.goal_summary, failure.reason, failure.suggestion,
    );
    session.context_mut().push(ChatMessage::user(failure_msg));
    self.run_yolo_classification(session).await
}
```

### A.4 当前缺失的能力

| 能力 | 状态 | 影响 |
|------|------|------|
| 并发执行 | 缺失 | hard 任务串行慢 |
| doom_loop 检测 | 缺失 | 可能无限循环 |
| 结构化事件流 | 缺失 | 可观测性差 |
| Descriptor 持久化 | 缺失 | crash 无法恢复 |
| 多层熔断 | 缺失 | 无保护 |
| SubAgent scope 约束 | 缺失 | 安全风险 |

---

## 附录 B：各项目调度/委派代码片段速查

### B.1 AtomCode — TaskTool 子 Agent 派发

```rust
// crates/atomcode-capabilities/src/tools/task.rs
const DEFAULT_MAX_CONCURRENT: usize = 3;
// 主 agent 按难度选档位（fast/capable），按类型选工具集（explore/worker）
// 14 种 TeamRoleId，Semaphore + JoinSet 并行
```

### B.2 Claude Code — AgentTool 四层 SubAgent

```typescript
// src/tools/AgentTool/AgentTool.tsx
// 路径 A: Swarm teammate spawn（teamName + name）
// 路径 B: Fork 子 Agent（cache 共享）
// 路径 C: 显式 subagent_type
// 路径 D: remote isolation
```

### B.3 OpenClaw — spawnSubagentDirect 9 步 spawn

```typescript
// src/agents/subagents/spawn/subagent-spawn.ts
// 1. 输入验证 2. Session 准备 3. Context Engine 准备 4. Attachments 材料化
// 5. Delivery Context 绑定 6. Gateway 注册 7. Swarm 容量预留
// 8. Launch Request 构建 9. Lifecycle Emitter
```

### B.4 OpenCode — TaskTool child session

```typescript
// packages/opencode/src/tool/task.ts
// depth 检查 → sessions.create({ parentID, agent, permission })
// → background.start/wait → renderOutput XML
```

### B.5 DeepSeek-Harness — round driver

```typescript
// packages/goal/goal-round-driver/src/index.ts
// drive(): checkpoint → goal 检查 → maxGoalRounds 限制 → followup(message)
// requestDrive(): serialized loop（while state.requested && !state.stopping）
```

### B.6 Pi — Lane 并发 + 三队列

```typescript
// packages/agent/src/harness/agent-harness.ts
// steer: 注入调整（当前 turn 结束后注入）
// followUp: 追加后续（agent 将停止时注入）
// nextRun: 触发下一轮（run 启动时捕获）
```

---

## 附录 C：建议实现方案详设

### C.1 Semaphore 并发实现方案

**修改文件**：`src/agent/orchestrator.rs`

```rust
// 新增
use std::sync::Arc;
use tokio::sync::Semaphore;

const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 3;

impl OrchestratorConfig {
    pub fn with_max_concurrent_subagents(mut self, n: usize) -> Self {
        self.max_concurrent_subagents = n.max(1);
        self
    }
}

// execute_workflows 改为 execute_workflows_parallel
async fn execute_workflows_parallel(
    &self,
    c: &TaskClassification,
    plan: &WorkFlowPlan,
    session: &Session,
) -> std::result::Result<TaskResult, QualityFailure> {
    let sem = Arc::new(Semaphore::new(self.cfg.max_concurrent_subagents));
    let mut set = tokio::task::JoinSet::new();
    let ordered = main_work::topo_sort(&plan.workflows).map_err(...)?;

    let dep_outputs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    for wf in ordered {
        let sub_input = build_subflow_input(&wf, &dep_outputs.lock().unwrap());
        let sem = sem.clone();
        let dep_outputs = dep_outputs.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore not closed");
            let outcome = self.sub_agent.run_unit(&sub_input, session.id()).await;
            (wf.id.clone(), outcome)
        });
    }

    while let Some(res) = set.join_next().await {
        match res {
            Ok((id, Ok(outcome))) => {
                dep_outputs.lock().unwrap().insert(id, outcome.text.clone());
                // QC 检查...
            }
            Ok((id, Err(e))) => return Err(QualityFailure { source: AgentRole::SubAgent, reason: format!("wf={}: {}", id, e), retryable: true, suggestion: "重试".into() }),
            Err(join_err) => return Err(QualityFailure { source: AgentRole::SubAgent, reason: format!("join error: {}", join_err), retryable: true, suggestion: "重试".into() }),
        }
    }
    // ...
}
```

### C.2 doom_loop 检测实现方案

**修改文件**：`src/agent/subagent.rs`

```rust
const DOOM_LOOP_THRESHOLD: usize = 3;

#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool: String,
    pub args: String,
}

fn detect_doom_loop(recent_calls: &[ToolCallRecord]) -> bool {
    if recent_calls.len() < DOOM_LOOP_THRESHOLD { return false; }
    let last_n = &recent_calls[recent_calls.len() - DOOM_LOOP_THRESHOLD..];
    last_n.iter().all(|c| c.tool == last_n[0].tool && c.args == last_n[0].args)
}

// 在 run_unit 中
pub async fn run_unit(&self, input: &SubFlowInput, session_id: &str) -> Result<SubAgentOutcome> {
    let mut recent_calls: VecDeque<ToolCallRecord> = VecDeque::new();
    loop {
        // ... 执行工具调用
        recent_calls.push_back(ToolCallRecord { tool: call.name.clone(), args: call.args.clone() });
        if recent_calls.len() > DOOM_LOOP_THRESHOLD * 2 {
            recent_calls.pop_front();
        }
        if detect_doom_loop(&recent_calls.iter().cloned().collect::<Vec<_>>()) {
            return Err(SubAgentError::DoomLoopDetected);
        }
    }
}
```

### C.3 结构化事件流实现方案

**新增文件**：`src/agent/events.rs`

```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub enum OrchestratorEvent {
    RunStarted { goal: String, task_level: String },
    AgentStarted { role: String, goal: String },
    AgentFinished { role: String, success: bool, summary: String, usage: Usage },
    WorkflowStarted { id: String, name: String },
    WorkflowFinished { id: String, success: bool, verdict: String },
    RunFinished { goal: String, total_usage: Usage },
}

pub struct EventBus {
    sender: tokio::sync::broadcast::Sender<OrchestratorEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(capacity);
        Self { sender }
    }

    pub fn emit(&self, event: OrchestratorEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<OrchestratorEvent> {
        self.sender.subscribe()
    }
}
```

### C.4 Descriptor 持久化实现方案

**修改文件**：`src/agent/subagent.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentDescriptor {
    pub version: u32,
    pub mode: SubAgentMode,
    pub label: String,
    pub persona: String,
    pub tool_allowlist: Option<Vec<String>>,
    pub tool_denylist: Option<Vec<String>>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubAgentMode {
    OneShot,
    Continuable,
}

const SUB_AGENT_DESCRIPTOR_VERSION: u32 = 1;

impl SubAgentDescriptor {
    pub fn new(label: String, persona: String) -> Self {
        Self {
            version: SUB_AGENT_DESCRIPTOR_VERSION,
            mode: SubAgentMode::OneShot,
            label,
            persona,
            tool_allowlist: None,
            tool_denylist: None,
            provider: None,
            model: None,
            created_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        }
    }

    pub fn persist(&self, db: &Db, session_id: &str, role: AgentRole) -> Result<()> {
        let json = serde_json::to_string(self)?;
        db.insert_agent_memory(session_id, role, "descriptor", &json)?;
        Ok(())
    }
}
```

---

## 附录 D：关键源码文件索引

| 项目 | 调度/委派相关核心文件 |
|------|---------------------|
| Claude Code | `src/tools/AgentTool/AgentTool.tsx`、`src/tools/AgentTool/runAgent.ts`、`src/utils/forkedAgent.ts`、`src/utils/swarm/backends/`、`src/tools/SendMessageTool/`、`src/coordinator/coordinatorMode.ts`、`src/utils/tasks.ts` |
| AtomCode | `crates/atomcode-capabilities/src/tools/task.rs`（TaskTool）、`crates/atomcode-capabilities/src/team.rs`（TeamEvent/TeamRoleId）、`crates/atomcode-coding/src/team/runner.rs`（TeamRunnerFactory）、`crates/atomcode-coding/src/team/manager.rs`（TeamRunManager） |
| OpenClaw | `src/agents/subagents/spawn/subagent-spawn.ts`（spawnSubagentDirect）、`src/agents/subagents/swarm/swarm-scheduler.ts`（SwarmGroupLane）、`src/agents/harness/selection.ts`（Harness 选择）、`src/agents/subagents/registry/subagent-registry-lifecycle.ts`（代次管理） |
| OpenCode | `packages/opencode/src/agent/agent.ts`（Agent 类型）、`packages/opencode/src/session/processor.ts`（doom_loop）、`packages/opencode/src/session/prompt.ts`（主循环）、`packages/opencode/src/tool/task.ts`（TaskTool）、`packages/opencode/src/effect/runner.ts`（Session 状态机） |
| DeepSeek-Harness | `packages/subagent/subagent/src/child-agent.ts`（applyChildComposition）、`packages/subagent/subagent/src/descriptor.ts`（Descriptor 持久化）、`packages/goal/goal-round-driver/src/index.ts`（round driver）、`packages/goal/tool-goal/src/authority.ts`（authority 模型） |
| Pi | `packages/agent/src/harness/agent-harness.ts`（AgentLane）、`packages/agent/src/harness/session/session.ts`（Session Tree）、`packages/agent/src/agent-loop.ts`（三队列协议）、`packages/agent/src/harness/reducer.ts`（Lane 状态还原） |
| laew | `src/agent/orchestrator.rs`（MultiAgentOrchestrator）、`src/agent/yolo.rs`（YoloRunner）、`src/agent/subagent.rs`（SubAgentRunner）、`src/agent/quality.rs`（QualityRunner）、`src/agent/main_work.rs`（MainWorkRunner）、`src/agent/plan.rs`（PlanRunner）、`src/agent/session_context.rs`（SessionContextRunner） |

---

## 附录 E：术语对照表

| 术语 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek-Harness | Pi | laew |
|------|-------------|----------|----------|----------|-----------------|-----|------|
| 任务级别 | effort | fast/capable | 无 | 无 | 无 | 无 | TaskLevel |
| 执行模式 | context(fork/inline) | explore/worker | spawn mode | Agent mode | 无 | drive mode | 无（待增加） |
| 子 Agent | Fork/AgentTool/Task/Team | TaskTool/TeamTool | spawnSubagent | subagent | child agent | Lane | SubAgent-Work |
| 目标管理 | 无 | 无 | 无 | 无 | GoalPhase | 无 | TaskPhase（待增加） |
| 风险分类 | 无 | RiskLevel | ExecAutoReviewRisk | 无 | 无 | 无 | 无（待增加） |
| 熔断 | 无 | REPEAT_ROUNDS | 无 | doom_loop | maxGoalRounds | LaneBusy | 无（待增加） |
| 编排器 | Coordinator | TaskTool/TeamRunManager | Swarm Scheduler | Runner | round driver | AgentHarness | MultiAgentOrchestrator |
| 质检 | Hook system | VerifyCadenceHook | exec auto-reviewer | 无 | 无 | 无 | Quality-Check Agent |
| 会话摘要 | 无 | 无 | 无 | 无 | wrapup 注入 | branch summary | SessionContext Agent |
| 并发控制 | Swarm 后端 | Semaphore(3) | SwarmGroupLane FIFO | Runner 状态机 | round driver 串行 | Lane 并发 | 无（串行） |
| 失败回流 | Task Failed + Hook | retryable fallback | Run 4 终态 | doom_loop + retry | round-limit block | LaneBusy + resume | QualityFailure + Yolo 重评 |

---

## 附录 F：反模式警示

### F.1 无并发保护的并行反模式

**问题**：引入并发执行 SubAgent 但没有依赖分析和冲突检测。

**案例**：如果 laew 的 Main-Work Agent 拆解出 3 个并行 SubAgent，它们同时修改同一个文件，会导致冲突。

**解决**：借鉴 AtomCode 的 `validate_non_overlapping_worker_scopes`，在并行执行前验证 scope 不重叠。

### F.2 无上限的自动续轮反模式

**问题**：自动续轮没有上限，可能无限循环。

**案例**：如果 Main-Work Agent 的 WorkFlow 拆解逻辑有 bug，可能生成无限长的流程列表。

**解决**：借鉴 DeepSeek 的 maxGoalRounds（256），laew 应为每个任务级别设置最大轮次上限。

### F.3 上下文爆炸反模式

**问题**：子 agent 接收全量主上下文，导致 token 爆炸。

**案例**：如果 laew 的 SubAgent-Work 接收完整对话历史，长任务的上下文可能超出限制。

**解决**：借鉴 OpenCode 的"仅传 task prompt"策略，只给必要信息。

### F.4 单点编排器故障反模式

**问题**：编排器是单点故障，一旦失败整个任务丢失。

**案例**：如果 laew 的 MultiAgentOrchestrator 在 hard 任务执行到一半时崩溃，无法恢复。

**解决**：借鉴 DeepSeek 的 Descriptor 持久化 + Pi 的 `reduceLaneState` 恢复机制。

### F.5 无 doom_loop 检测反模式

**问题**：SubAgent 可能陷入重复循环（同工具同参数）。

**案例**：如果 SubAgent-Work 连续 10 次调用 `read_file` 读取同一个文件，没有检测机制。

**解决**：借鉴 OpenCode 的 doom_loop 检测（3 次精确重复 → 权限请求/终止）。

---

## 文档信息

- 总行数：~1700 行
- 覆盖维度：委派协议 / 上下文传递 / 并发调度 / 通信方式 / 角色分工 / 失败回流 / 设计模式 / laew 建议
- 输入文件：6 份核心机制深度分析 + 已有对比报告
- 分析方法：横向对比 + 深入分析 + 设计模式 + laew 建议
- 关键结论引用：真实文件路径 + 函数名/结构体名 + 关键代码片段