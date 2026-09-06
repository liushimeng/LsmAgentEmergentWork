# 专题：6 个 Agent 项目的 Workflow 设计横向分析

> **元信息**
> - 生成日期：2026-09-04
> - 分析范围：claude-code / atomcode / openclaw / opencode / deepseek-harness / pi
> - 输入文档：`docs/Agent源码调研/` 下 6 份「核心机制深度分析」
> - 对标基准：laew MultiAgentOrchestrator + Main-Work WorkFlow 列表
> - 定位：横向专题报告，聚焦「工作流如何被定义、编排、调度、执行、重试与回滚」，覆盖 DSL/代码/状态机三类定义方式、线性/DAG/并行/循环四类拓扑、以及 workflow 与 agent loop 的内外两层关系

---

## 目录

- [1. 横向对比总览表](#1-横向对比总览表)
- [2. Claude Code：Task 系统 + Coordinator 编排 + Swarm 扇出](#2-claude-code-task-系统--coordinator-编排--swarm-扇出)
- [3. AtomCode：Goal/Loop 控制器 + Team 并发 + Plan/Verify 纪律](#3-atomcodeloop-控制器--team-并发--planverify-纪律)
- [4. OpenClaw：TaskFlow 注册表 + Swarm 调度 + Worker 执行](#4-openclawtaskflow-注册表--swarm-调度--worker-执行)
- [5. OpenCode：Session Agent Loop + 多 Agent 类型 + 状态机](#5-opencode-session-agent-loop--多-agent-类型--状态机)
- [6. DeepSeek-Harness：脚本化 Workflow 引擎 + 组合子 + Worker 隔离](#6-deepseek-harness脚本化-workflow-引擎--组合子--worker-隔离)
- [7. Pi：Lane 并发树 + 队列驱动 + 分支编排](#7-pilane-并发树--队列驱动--分支编排)
- [8. 设计模式提炼](#8-设计模式提炼)
- [9. 对 laew 的综合建议](#9-对-laew-的综合建议)
- [附录 A：laew 当前编排现状](#附录-alae-当前编排现状)
- [附录 B：各项目 workflow 定义代码片段速查](#附录-b各项目-workflow-定义代码片段速查)
- [附录 C：建议实现方案详设](#附录-c建议实现方案详设)

---

## 1. 横向对比总览表

### 1.1 核心机制对比

| 维度 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek-Harness | Pi |
|------|-------------|----------|----------|----------|-----------------|-----|
| **Workflow 定义方式** | 代码内 Task 对象 + Coordinator prompt 隐式阶段 | 代码结构体 `WorkFlowSpec` + Goal/Loop 控制器 | TaskFlowRecord 状态机 + 托管 runtime | Session 内 Agent Loop + Todo 列表 | **JS 脚本 DSL**（`agent`/`parallel`/`pipeline`/`phase`/`log` 组合子） | Lane 树 + 队列驱动（steer/followUp/nextRun） |
| **编排拓扑** | 线性 + 并行扇出（多 AgentTool 并发） | 线性 + Team 并发（max_concurrent=3） | DAG（TaskFlow 状态依赖）+ Swarm FIFO | 单循环 + 子 Task 嵌套 + 并行 tool-call | **DAG + 并行 + 流水线**（`parallel`/`pipeline` 组合子） | 树状 Lane 分支 + 并发叶 |
| **执行引擎** | `Task` trait + `AppState` 调度 + 异步 spawn | `TeamRunManager` + `Agent::run_to_completion` | `TaskExecutor` + `TaskFlowRegistry` + Worker | `SessionPrompt.loop()` + `SessionProcessor` | **Worker Thread 隔离** + vm 沙箱 + 组合子 runtime | `AgentSession` + `SessionManager` 树 |
| **与 agent loop 关系** | workflow 外层（Coordinator）+ 内层（Worker agent loop） | workflow 外层（Goal/Loop 控制器驱动 agent） | workflow 外层（TaskFlow 编排 SubAgent） | **workflow = agent loop**（loop 即 workflow） | **workflow 即脚本**（脚本调用 agent，agent 是子例程） | workflow 外层（Lane 调度 agent turn） |
| **阶段划分** | Research → Synthesis → Implementation → Verification（Coordinator prompt） | Plan → Build → Verify（GoalPhase + VerifyCadenceHook） | queued → running → succeeded/blocked/failed/cancelled/lost | step 循环 + subtask/compaction 分支 | `phase()` 声明式阶段（仅进度语义，无执行结构） | 无显式阶段，由 Lane 隐式划分 |
| **失败重试** | `SendMessageTool` 续接 worker；Hook 拦截 | `MAX_UNPRODUCTIVE=5` 熔断；CancellationToken | 终态机 + revision 乐观锁 + 重试 | doom_loop 3 次 → 权限请求；retry 指数退避（max 5 次） | 子 agent 失败 → `null`（非致命）；致命错误杀脚本 | LaneBusy 错误；suspended → resume |
| **幂等保证** | Task ID 前缀（b/a/r/t/w/m/d）+ 防暴力破解字母表 | generation 隔离 + 外部 run 代际校验 | idempotencyKey + requestFingerprint | PartID 单调递增 + 快照 hash | 子 agent 发布幂等 + 冷启动重放（replayedSpawnResult） | entry id + parentId 树结构 |
| **与 laew 最相似点** | Coordinator 四阶段 ≈ laew Plan→Main→Sub→QC | Goal 控制器 + VerifyCadence ≈ laew QC 层 | TaskFlow 状态机 ≈ laew WorkFlow 列表 | Agent Loop + 多角色 ≈ laew 多 Agent | **脚本化组合子** ≈ laew Main-Work 的 WorkFlow 理想形态 | Lane 并发 ≈ laew SubAgent 并发 |

### 1.2 定义方式光谱

```
纯代码/状态机                                                  纯 DSL/脚本
←─────────────────────────────────────────────────────────────────────────→
OpenCode(loop)  AtomCode(GoalStruct)  OpenClaw(TaskFlowRecord)  ClaudeCode(Task)  Pi(Lane)  DeepSeek(JS脚本)
```

laew 的 Main-Work WorkFlow 列表位于光谱的**代码/状态机端**（`WorkFlowSpec` 结构体 + JSON 解析），与 OpenClaw TaskFlowRecord 和 AtomCode GoalStruct 同属一类。

### 1.3 编排粒度光谱

```
粗粒度（单循环）                                               细粒度（多层嵌套）
←─────────────────────────────────────────────────────────────────────────→
OpenCode(单loop)  Pi(Lane)  AtomCode(Team)  OpenClaw(TaskFlow)  ClaudeCode(Coordinator)  DeepSeek(脚本组合子)
```

laew 的 Yolo → Plan → MainWork → SubAgent → QC → SessionContext 六层架构位于光谱的**细粒度端**，与 DeepSeek 脚本组合子和 Claude Code Coordinator 同属一类。

### 1.4 执行引擎隔离光谱

```
进程内（共享内存）                                             进程外/沙箱隔离
←─────────────────────────────────────────────────────────────────────────→
OpenCode(Effect)  Pi(类内状态)  AtomCode(tokio Task)  OpenClaw(Worker)  ClaudeCode(子进程)  DeepSeek(Worker Thread + vm)
```

laew 当前为**进程内 async**（tokio），与 AtomCode 和 OpenCode 同属一类。

---

## 2. Claude Code：Task 系统 + Coordinator 编排 + Swarm 扇出

### 2.1 Workflow 定义方式

Claude Code 的 workflow 是**隐式定义**的：没有独立的 workflow 文件，而是通过 Task 对象类型系统 + Coordinator 系统 prompt 中的阶段约定来定义。

**Task 类型系统**（`src/Task.ts`）：

```ts
export type TaskType =
  | 'local_bash'
  | 'local_agent'
  | 'remote_agent'
  | 'in_process_teammate'
  | 'local_workflow'
  | 'monitor_mcp'
  | 'dream'

export type TaskStatus =
  | 'pending' | 'running' | 'completed' | 'failed' | 'killed'
```

每个 Task 有唯一 ID，前缀标识类型（`b`/`a`/`r`/`t`/`w`/`m`/`d`），使用 36 进制 8 位随机后缀（约 2.8 万亿组合，防暴力破解 symlink 攻击）：

```ts
const TASK_ID_ALPHABET = '0123456789abcdefghijklmnopqrstuvwxyz'
export function generateTaskId(type: TaskType): TaskId {
  const prefix = getTaskIdPrefix(type)
  const bytes = randomBytes(8)
  let id = prefix
  for (let i = 0; i < 8; i++) {
    id += TASK_ID_ALPHABET[bytes[i]! % TASK_ID_ALPHABET.length]
  }
  return id
}
```

**Task trait**（`src/Task.ts`）极简，只暴露 `kill`：

```ts
export type Task = {
  name: string
  type: TaskType
  kill(taskId: string, setAppState: SetAppState): Promise<void>
}
```

### 2.2 编排拓扑

Claude Code 的 workflow 编排是**Coordinator 驱动的隐式 DAG**：

```
用户输入 → Coordinator（主 agent loop）
                ├─ AgentTool("research A")  ──→ Worker a1b（子 agent loop）
                ├─ AgentTool("research B")  ──→ Worker a2c（子 agent loop）  ← 并行扇出
                ├─ AgentTool("implement")   ──→ Worker a3d
                └─ AgentTool("verify")      ──→ Worker a4e
```

Coordinator 通过**单次消息中的多个 AgentTool 调用**实现并行扇出（"Parallelism is your superpower"）。Worker 完成后通过 `<task-notification>` XML 消息异步回报。

**Coordinator 四阶段约定**（`src/coordinator/coordinatorMode.ts` 系统 prompt）：

| Phase | Who | Purpose |
|-------|-----|---------|
| Research | Workers (parallel) | Investigate codebase, find files, understand problem |
| Synthesis | **You** (coordinator) | Read findings, craft implementation specs |
| Implementation | Workers | Make targeted changes per spec, commit |
| Verification | Workers | Test changes work |

### 2.3 执行引擎

**Task 调度**（`src/tasks.ts`）：

```ts
export function getAllTasks(): Task[] {
  const tasks: Task[] = [LocalShellTask, LocalAgentTask, RemoteAgentTask, DreamTask]
  if (LocalWorkflowTask) tasks.push(LocalWorkflowTask)
  if (MonitorMcpTask) tasks.push(MonitorMcpTask)
  return tasks
}
```

注意 `LocalWorkflowTask` 受 feature gate `WORKFLOW_SCRIPTS` 控制，说明 Claude Code 也在探索脚本化 workflow。

**Worker 续接机制**：Coordinator 通过 `SendMessageTool` 向已完成的 worker 发送后续消息，利用其已加载的上下文（"Continue vs. Spawn"决策矩阵）：

| 情境 | 机制 | 原因 |
|------|------|------|
| 研究恰好覆盖要编辑的文件 | Continue（SendMessage） | Worker 已有文件上下文 |
| 研究宽泛但实现狭窄 | Spawn fresh（AgentTool） | 避免拖入探索噪声 |
| 纠正失败或扩展近期工作 | Continue | Worker 有错误上下文 |

### 2.4 失败重试与幂等

- **Worker 失败**：通过 `SendMessageTool` 续接同一 worker（保留错误上下文），不发新 worker
- **Worker 停止**：`TaskStopTool` + `task_id` 停止跑偏的 worker，可继续
- **权限处理**：Coordinator worker 的权限走 `coordinatorHandler.ts`（Hook → classifier → 交互对话框三级）；Swarm worker 走 `swarmWorkerHandler.ts`（classifier → 通过 mailbox 转发给 leader）

### 2.5 与 laew 对照

Claude Code 的 Coordinator 四阶段 ≈ laew 的 Plan → Main-Work → SubAgent → QC 链路。关键差异：Claude Code 的阶段是**prompt 约定**（软约束），laew 是**代码编排**（硬约束）。Claude Code 的并行扇出是**模型驱动**（模型决定一次发几个 AgentTool），laew 是**Orchestrator 驱动**（代码控制并发）。

---

## 3. AtomCode：Goal/Loop 控制器 + Team 并发 + Plan/Verify 纪律

### 3.1 Workflow 定义方式

AtomCode 的 workflow 是**代码结构体 + 控制器**模式，核心在 `crates/atomcode-coding/src/controllers.rs`：

```rust
pub(crate) struct GoalState {
    pub id: u64,
    pub condition: String,          // 目标条件（自然语言）
    pub active: bool,
    pub phase: GoalPhase,
    pub terminal: Option<GoalTerminal>,
    pub round: u32,
    pub max_rounds: Option<u32>,
    deadline: Option<Instant>,
    pub unproductive: u32,          // 连续无进展轮次
    pub cancel: CancellationToken,
    progress_recap: Option<String>, // 恢复用进度摘要
    recovery_pause: bool,
}
```

**GoalPhase 状态机**：

```rust
pub enum GoalPhase {
    Pursuing,      // 执行中
    Paused,        // 用户显式暂停
    PausedAtCap,   // 触及上限暂停
    Satisfied,     // 已满足
    Ended,         // 终态
}
```

**GoalTerminal**：`Met | Stopped | Failed | Cancelled`

### 3.2 编排拓扑

AtomCode 的 workflow 是**Goal 控制器驱动的循环**：

```
用户设定 Goal（condition, max_rounds, max_duration）
        ↓
GoalPhase::Pursuing ──→ Agent 执行一轮 ──→ evaluate_goal() 评估
        │                                        │
        │←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←←┘
        ↓（未满足且未超限）
  继续下一轮 / 暂停恢复 / 熔断
```

**Team 并发**（`crates/atomcode-coding/src/team/manager.rs`）：

```rust
pub struct TeamRuntimeConfig {
    pub max_concurrent: usize,    // 默认 3
    pub cancel_grace: Duration,   // 默认 2s
    pub max_result_chars: usize,  // 默认 12_000
    pub max_completed_runs: usize,// 默认 32
}
```

`TeamRunManager::delegate()` 接收 `Vec<TeamTaskSpec>`，为每个 task 构建独立的 `Agent`，通过 `TeamJobFactory` 生成 future，共享一个 `CancellationToken`（generation 隔离）。

**TeamRunner**（`team/runner.rs`）为每个成员构建完整 Agent：

```rust
let mut builder = Agent::builder()
    .provider(provider)
    .tools(tools)
    .persona(team_member_persona(profile, &task.scope))
    .working_dir(self.working_dir.clone())
    .cancel_token(cancel)
    .hook(progress.clone())
    .middleware(Arc::new(DenyTeamBash));
```

### 3.3 执行引擎

**Goal 评估器**（`controllers.rs`）使用独立 LLM 调用做二元判定：

```rust
const EVALUATOR_SYSTEM_PROMPT: &str = r#"You are a strict goal evaluator...
1. Output EXACTLY ONE LINE.
2. The line MUST begin with `Verdict: yes` or `Verdict: no`..."#;
```

评估器接收 `<<<GOAL>>>` 和 `<<<ASSISTANT_LOG>>>` 两个 sentinel 包裹的块，输出 `Verdict: yes/no + 原因`。

**无进展熔断**：`MAX_UNPRODUCTIVE = 5`，连续 5 轮无进展则 `pause_at_cap`。

**恢复机制**：`GoalState::resume()` 重置 round=0、刷新 deadline、清零 unproductive；`resume_paused()` 恢复用户暂停（不刷新预算）。

### 3.4 Plan/Verify 纪律

**PlanModeGate**（`plan_mode.rs`）：plan 模式下 hard-block 内置 mutating 工具（bash/edit/write），MCP read-only 工具放行，其他 MCP 工具 prompt 用户决定。

**VerifyCadenceHook**（`discipline/verify.rs`）：编辑后验证纪律——模型停止时若编辑了代码但未运行检查，注入一次性 nudge：

```rust
const NUDGE: &str = "You made code edits but have not verified them. Run a fast check \
(`cargo check`, `tsc --noEmit`, or the equivalent for this project)...";
```

这是 laew Quality-Check 层的 AtomCode 等价物，但它是**单轮提示注入**，非独立 Agent。

### 3.5 与 laew 对照

AtomCode 的 Goal 控制器 ≈ laew 的 Main-Work + Orchestrator 循环（`max_retry_per_level`）。关键差异：AtomCode 的 Goal 是**自主循环**（控制器持续驱动 agent 直到满足条件），laew 的 Orchestrator 是**单次调度**（失败只回流到 Yolo 重评，不自动续轮）。AtomCode 的 VerifyCadenceHook ≈ laew 的 Quality-Check，但粒度不同（单轮提示 vs 独立 Agent）。

---

## 4. OpenClaw：TaskFlow 注册表 + Swarm 调度 + Worker 执行

### 4.1 Workflow 定义方式

OpenClaw 的 workflow 是**状态机驱动的 TaskFlow 注册表**，核心在 `src/tasks/` 目录。

**TaskFlowRecord**（`src/tasks/task-flow-registry.ts`）：

```ts
type FlowRecordPatch = Omit<Partial<Pick<TaskFlowRecord,
  | 'status' | 'notifyPolicy' | 'goal' | 'currentStep'
  | 'blockedTaskId' | 'blockedSummary' | 'controllerId'
  | 'stateJson' | 'waitJson' | 'cancelRequestedAt'
  | 'updatedAt' | 'endedAt'>>, ...>
```

**TaskFlowStatus** 终态机：

```ts
function isTerminalTaskFlowStatus(status: TaskFlowStatus): boolean {
  return status === "succeeded" || status === "blocked"
      || status === "failed" || status === "cancelled" || status === "lost"
}
```

**TaskRecord 状态**：`queued | running | succeeded | cancelled | lost`，其中 succeeded 可带 `terminalOutcome: blocked`。

### 4.2 编排拓扑

OpenClaw 的 workflow 是**DAG + 托管 runtime**：

```
TaskFlowRecord（goal + currentStep + blockedTaskId）
    ├─ TaskRecord #1（queued → running → succeeded）
    ├─ TaskRecord #2（依赖 #1 输出）
    └─ TaskRecord #3（依赖 #1 + #2）
```

**revision 乐观锁**保证并发安全：

```ts
export type TaskFlowUpdateResult =
  | { applied: true; flow: TaskFlowRecord }
  | { applied: false; reason: "not_found" | "revision_conflict" | "persist_failed"; current?: TaskFlowRecord }
```

**单任务流**（`isOneTaskFlowEligible`）：ACP/SubAgent 运行自动获得一个 flow handle，用于状态和重试表面。

### 4.3 执行引擎

**TaskExecutor**（`src/tasks/task-executor.ts`）是核心执行入口：

```ts
export function createQueuedTaskRunCore(params: DetachedTaskCreateParams): TaskRecord | null {
  const task = createTaskRecord({ ...params, status: "queued" })
  if (!task) return null
  return ensureSingleTaskFlow({ task, requesterOrigin: params.requesterOrigin })
}
```

**Task 完成/失败**：

```ts
export function completeTaskRunByRunIdCore(params: DetachedTaskCompleteParams) {
  return finalizeTaskRunByRunIdCore({ ...params, status: "succeeded" })
}
export function failTaskRunByRunIdCore(params: DetachedTaskFailParams) {
  return finalizeTaskRunByRunIdCore({ ...params, status: params.status ?? "failed" })
}
```

### 4.4 Swarm 调度

**code-mode-swarm**（`src/agents/code-mode-swarm.runtime.ts`）实现 Coordinator 模式的 Swarm：

```ts
export const codeModeSwarmHandlers = {
  agentSpawn: runAgentSpawnBridge,   // 启动子 agent
  agentWait: runAgentWaitBridge,     // 等待完成
  swarmNote: runSwarmNoteBridge,     // 阶段/日志注释
}
```

**幂等 spawn**：通过 `idempotencyKey` + `requestFingerprint`（sha256）保证冷启动重放安全：

```ts
const requestFingerprint = `sha256:${createHash("sha256")
  .update(stableStringify(spawnInput)).digest("hex")}`
const idempotencyKey = `${params.codeModeRunId}:${params.request.id}`
let existing = getSwarmRunByLaunchReplayKey(idempotencyKey, requesterSessionKey, params.ctx.agentId)
if (existing) return replayedSpawnResult(existing)
```

### 4.5 Worker 系统

OpenClaw 的 `src/worker/` 提供完整的 Worker 运行时：
- `worker.runtime.ts`：Worker 生命周期
- `worker-command.runtime.ts`：命令执行
- `worker-connection.ts`：连接管理
- `inference-stream.runtime.ts`：推理流
- `embedded-agent*.runtime.ts`：嵌入式 agent

### 4.6 与 laew 对照

OpenClaw 的 TaskFlow 注册表 ≈ laew 的 WorkFlow 列表理想形态。关键差异：OpenClaw 的 TaskFlow 是**持久化状态机**（SQLite + revision 乐观锁），laew 的 WorkFlow 是**内存态 JSON 解析**（无持久化、无并发控制）。OpenClaw 的 Swarm 幂等 spawn 机制是 laew 可借鉴的模式。

---

## 5. OpenCode：Session Agent Loop + 多 Agent 类型 + 状态机

### 5.1 Workflow 定义方式

OpenCode 的 workflow **就是 agent loop 本身**——没有独立的 workflow 定义层，workflow 由 `SessionPrompt.loop()` 的 step 循环 + Agent 类型 + Todo 列表共同定义。

**Agent 类型系统**（`packages/opencode/src/agent/agent.ts`）：

```ts
export const Info = Schema.Struct({
  name: Schema.String,
  description: Schema.optional(Schema.String),
  mode: Schema_literals(["subagent", "primary", "all"]),
  native: Schema.optional(Schema.Boolean),
  permission: PermissionV1.Ruleset,
  model: Schema.optional(Schema.Struct({ modelID, providerID })),
  prompt: Schema.optional(Schema.String),
  steps: Schema.optional(Schema.Finite),   // 最大步数限制
  ...
})
```

内置 Agent 角色：`build`（主 agent）、`plan`（只读规划）、`general`（通用子 agent）、`explore`（快速搜索）、`compaction`、`title`、`summary`。

### 5.2 编排拓扑

OpenCode 的 workflow 是**单循环 + 分支**：

```
loop(sessionID):
  while (true):
    ├─ 检查 lastAssistant.finish → 无 tool-calls 则 break
    ├─ 检查 subtask 队列 → handleSubtask
    ├─ 检查 compaction → 溢出则压缩
    ├─ 解析 agent + tools + permission
    ├─ processor.create() → handle.process(stream)
    │     └─ 流式处理：reasoning/text/tool-call/tool-result/step-finish
    ├─ 检查 doom_loop（3 次精确重复 → 权限请求）
    ├─ 检查 max_steps → 注入 MAX_STEPS_PROMPT
    └─ result === "stop" 则 break
```

**Todo 列表**（`packages/opencode/src/session/todo.ts`）提供步骤级跟踪：

```ts
export interface Interface {
  readonly update: (input: { sessionID: SessionID; todos: ReadonlyArray<Info> }) => Effect.Effect<void>
  readonly get: (sessionID: SessionID) => Effect.Effect<Info[]>
}
```

### 5.3 执行引擎

**SessionProcessor**（`packages/opencode/src/session/processor.ts`）是核心执行引擎，处理 LLM 流事件：

```ts
case "tool-call": {
  yield* ensureToolCall(value)
  // doom_loop 检测：最近 3 个 part 都是同一 tool 且同一 input
  const recentParts = parts.slice(-DOOM_LOOP_THRESHOLD)
  if (recentParts.length === DOOM_LOOP_THRESHOLD &&
      recentParts.every(part => part.type === "tool" && part.tool === value.name
          && JSON.stringify(part.state.input) === JSON.stringify(input))) {
    yield* permission.ask({ permission: "doom_loop", patterns: [value.name], ... })
  }
}
```

**SessionRunState**（`packages/opencode/src/session/run-state.ts`）管理运行态：

```ts
export interface Interface {
  readonly assertNotBusy: (sessionID: SessionID) => Effect.Effect<void, Session.BusyError>
  readonly cancel: (sessionID: SessionID) => Effect.Effect<void>
  readonly ensureRunning: (sessionID, onInterrupt, work) => Effect.Effect<SessionV1.WithParts>
  readonly startShell: (sessionID, onInterrupt, work, ready?) => Effect.Effect<...>
}
```

### 5.4 失败重试与幂等

**retry 策略**（`packages/opencode/src/session/retry.ts`）：

```ts
export const RETRY_INITIAL_DELAY = 2000
export const RETRY_BACKOFF_FACTOR = 2
export const RETRY_JITTER_FACTOR = 0.25
export const RETRY_MAX_DELAY_NO_HEADERS = 30_000
export const RETRY_MAX_RETRIES = 5
```

可重试模式覆盖：429/5xx、rate limit、overloaded、network error、timeout 等。支持 `retry-after` / `retry-after-ms` 响应头解析。

**doom_loop 检测**：连续 3 次精确重复的 tool-call（同名 + 同 input JSON）触发权限请求，由用户决定是否继续。

**快照 hash 幂等**：每个 step 前后捕获文件系统快照（`snapshot.track()`），step-finish 时计算 patch hash，保证文件变更可追溯。

### 5.5 与 laew 对照

OpenCode 的 agent loop ≈ laew 的 SubAgent-Work 单单元执行。关键差异：OpenCode 的 workflow 是**单进程 Effect 流**（无多 Agent 角色切换），laew 是**多 Agent 编排**（Yolo/Plan/Main/Sub/QC 角色各异）。OpenCode 的 doom_loop 检测是 laew Quality-Check 的轻量替代方案。OpenCode 的 Agent 类型系统（build/plan/explore/general）≈ laew 的 AgentRole 枚举，但 OpenCode 通过 permission ruleset 控制能力，laew 通过工具集控制。

---

## 6. DeepSeek-Harness：脚本化 Workflow 引擎 + 组合子 + Worker 隔离

### 6.1 Workflow 定义方式

DeepSeek-Harness 的 workflow 是**JavaScript 脚本 DSL**，这是 6 个项目中最独特的定义方式。核心在 `packages/workflow/`。

**WorkflowMeta**（`packages/workflow/workflow/src/types.ts`）：

```ts
export interface WorkflowMeta {
  name: string           // kebab-case 名称
  description: string    // 一行描述
  whenToUse?: string     // 使用时机
  phases?: WorkflowPhase[]  // 可选阶段声明
}

export interface WorkflowPhase {
  title: string          // phase() 调用匹配的标题
  detail?: string
  provider?: string      // 信息性 provider 覆盖
  model?: string         // 信息性 model 覆盖
}
```

**脚本钩子**（`packages/workflow/tool-workflow/src/index.ts` 内嵌 DESCRIPTION）：

```
Script-body hooks:
- agent(prompt, opts?): Promise<any> — 运行一个子 agent 到完成
- pipeline(items, ...stages): Promise<any[]> — 每个 item 独立通过各阶段（无跨阶段屏障）
- parallel(thunks): Promise<any[]> — 并发执行零参数函数并等待全部（有屏障）
- phase(title) — 开始进度阶段；log(message) — 叙述进度；args — 工具调用的 args 输入
```

### 6.2 编排拓扑

DeepSeek 的 workflow 是**DAG + 并行 + 流水线**，由脚本作者显式控制：

```js
// 典型 workflow 脚本结构（来自 DESCRIPTION）
const research = await parallel([
  () => agent("研究角度 A"),
  () => agent("研究角度 B"),
  () => agent("研究角度 C"),
]);

const verified = await pipeline(research.filter(Boolean),
  (item) => agent(`验证 ${item.topic}`, { schema: verificationSchema }),
  (item) => agent(`深化 ${item.topic}`)
);

phase("综合结论");
return { summary: verified, count: verified.length };
```

**parallel vs pipeline 语义差异**：
- `parallel(thunks)`：有屏障（`Promise.all`），全部完成才继续，单个失败 → `null`
- `pipeline(items, ...stages)`：**无跨阶段屏障**，每个 item 独立流过所有阶段，单个 item 失败 → 该 item 变 `null`，不影响其他 item

### 6.3 执行引擎

**WorkflowEngine**（`packages/workflow/workflow/src/index.ts`）是抽象服务：

```ts
export abstract class WorkflowEngine extends Service {
  abstract start(request: WorkflowStartRequest): WorkflowRun
  protected emitWorkflowEvent(name: WorkflowEventName, ...args: unknown[]): void { ... }
}
```

**Worker Thread 隔离**（`packages/workflow/workflow-worker-thread/`）：

```ts
// worker.ts 入口
void runWorkerSession(requireParentPort(parentPort), workerData as WorkerInit)
```

**WorkerRun**（`host.ts`）实现 `WorkflowRun`：

```ts
export class WorkerRun implements WorkflowRun {
  readonly result: Promise<WorkflowResult>  // 永不出 reject
  private settled = false
  private terminalClaimed = false
  private worker: Worker
  private children = new Map<number, ChildRecord>()
  ...
}
```

**vm 沙箱**（`runtime.ts`）：脚本在 `node:vm` 上下文中执行，只暴露白名单钩子（`agent`/`parallel`/`pipeline`/`phase`/`log`/`args`），**无文件系统、网络、定时器、Node.js API**：

```ts
// runtime.ts 注入的全局
agent: (prompt, opts?) => this.contain(this.agent(prompt, opts)),
parallel: (thunks) => this.contain(this.parallel(thunks)),
pipeline: (items, ...stages) => this.contain(this.pipeline(items, stages)),
phase: (title) => { this.phase(title) },
```

**realm 隔离**（`realm.ts`）：脚本返回值通过 `materializeFromRealm()` 序列化为纯 JSON，防止泄漏脚本内部引用。

### 6.4 失败处理与幂等

**错误分类**（`WorkflowErrorCode`）：

```ts
export type WorkflowErrorCode =
  | 'SCRIPT_PARSE' | 'META_INVALID' | 'INVALID_ARGUMENT'
  | 'UNSUPPORTED_OPTION' | 'UNSUPPORTED_SCHEMA'
  | 'AGENT_CAP' | 'ITEM_CAP' | 'AGENT_START' | 'AGENT_RESULT'
  | 'RESULT_UNSERIALIZABLE' | 'CANCELLED'
```

**致命 vs 非致命**：`isFatalWorkflowError()` 决定组合子行为——致命错误（参数错误、cap 触发、取消）杀脚本；子 agent 失败和普通 stage 错误 → 该 item 变 `null`：

```ts
export function isFatalWorkflowError(error: unknown): boolean {
  return error instanceof WorkflowError && error.fatal
}
```

**子 agent 幂等**：`agent()` 调用通过子 agent 发布幂等保证，冷启动重放返回缓存结果（`replayedSpawnResult`）。

**取消机制**：`WorkflowRun.cancel(reason?)` + `AbortSignal` 传播，worker 死亡后在 grace 期内 force-settle。

### 6.5 与 laew 对照

DeepSeek 的脚本化 workflow 是 laew Main-Work WorkFlow 列表的**理想进化形态**。关键差异：DeepSeek 的 workflow 是**图灵完备的 JS 脚本**（可表达任意 DAG/循环/条件），laew 的 WorkFlow 是**预定义结构体**（steps/branches/loops 字段固定）。DeepSeek 的 `parallel`/`pipeline` 组合子可直接映射到 laew 的 SubAgent 并发执行模式。DeepSeek 的 Worker Thread + vm 沙箱隔离是 laew 可参考的安全模式（当前 laew SubAgent 无隔离）。

---

## 7. Pi：Lane 并发树 + 队列驱动 + 分支编排

### 7.1 Workflow 定义方式

Pi 的 workflow 是**Lane 树 + 队列驱动**，核心在 `packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts`：

```ts
export interface LaneRow {
  session_id: string
  lane: string           // lane 名称（如 "main"、"fork-1"）
  leaf_id: string | null // 当前叶 entry
  open_operation_id: string | null // 正在运行的 operation
}

export interface LaneMoveRow {
  session_id: string
  seq: number
  lane: string
  leaf_id: string | null
}
```

**Lane 操作 API**：

```ts
export function createInitialLane(db, sessionId, lane = "leafId") {...}
export function createLane(db, sessionId, seq, lane, leafId) {...}
export function moveLane(db, sessionId, seq, lane, leafId) {...}
export function startLaneOperation(db, sessionId, lane, runId) {...}  // 乐观锁
export function finishLaneOperation(db, sessionId, lane, runId) {...}
```

### 7.2 编排拓扑

Pi 的 workflow 是**树状 Lane 分支**：

```
                    main lane (root)
                   /            \
            fork-1 lane      fork-2 lane
            /        \
      fork-1-1    fork-1-2
```

每个 lane 是一个独立的执行分支，拥有自己的叶 entry（会话历史点）。`startLaneOperation` 通过 `open_operation_id` 实现**乐观并发控制**——同一 lane 同时只能有一个 operation：

```ts
export function startLaneOperation(db, sessionId, lane, runId) {
  const result = sql`UPDATE lanes SET open_operation_id = ${runId}
    WHERE session_id = ${sessionId} AND lane = ${lane} AND open_operation_id IS NULL`.run(db)
  if (result.changes === 1) return
  throw new SessionError("storage", `Lane ${lane} already has an open operation`)
}
```

### 7.3 执行引擎

**AgentSession**（`packages/coding-agent/src/core/agent-session.ts`）是核心执行抽象：

```ts
export class AgentSession {
  readonly agent: Agent
  readonly sessionManager: SessionManager
  private _steeringMessages: string[] = []   // 中断队列
  private _followUpMessages: string[] = []   // 等待队列
  private _pendingNextTurnMessages: CustomMessage[] = []  // 附加上下文
  ...
}
```

**三种队列模式**：
- `steer`：中断当前 turn，立即处理
- `followUp`：等当前 turn 完成，附加到下一 turn
- `nextRun`：作为独立上下文注入

**AgentSessionRuntime**（`agent-session-runtime.ts`）管理会话生命周期：

```ts
export class AgentSessionRuntime {
  private _session: AgentSession
  private _services: AgentSessionServices
  async switchSession(sessionPath, options?) {...}   // 切换会话
  async newSession(options?) {...}                   // 新建会话
  async fork(entryId, options?) {...}                // 分叉会话
}
```

### 7.4 失败重试与幂等

**Lane 树持久化**：session 历史存储为 entry 链（`SessionEntry`），每个 entry 有 `id` + `parentId`，形成不可变树。`migrateV1ToV2` / `migrateV2ToV3` 处理版本迁移。

**operation 隔离**：`startLaneOperation` / `finishLaneOperation` 保证同一 lane 的 operation 串行化，防止并发写冲突。

**Session 分叉**：`fork()` 在指定 entry 处创建新 lane，继承父历史，实现时间线分支。

### 7.5 与 laew 对照

Pi 的 Lane 树 ≈ laew 的 Session 历史 + SubAgent 并发。关键差异：Pi 的 Lane 是**持久化树结构**（SQLite entry 链），laew 的 Session 是**内存对话上下文**。Pi 的队列驱动（steer/followUp）是 laew 可借鉴的**用户中断/续接机制**（当前 laew 无此能力）。Pi 的 `fork()` 能力是 laew 缺失的**时间线分支**功能。

---

## 8. 设计模式提炼

### 8.1 模式一：脚本化组合子（DeepSeek）

**描述**：用 `parallel`/`pipeline`/`agent` 三个组合子表达任意 DAG 工作流，脚本即 workflow 定义。

**优势**：
- 图灵完备，可表达任意拓扑
- 脚本与引擎分离，引擎可独立升级（沙箱隔离）
- `parallel` vs `pipeline` 的屏障语义精确

**劣势**：
- 脚本调试困难
- 无静态类型检查
- 需要运行时解析

**适用场景**：大规模扇出（审计、迁移、多角研究）。

### 8.2 模式二：状态机注册表（OpenClaw）

**描述**：TaskFlowRecord + TaskRecord 持久化状态机，revision 乐观锁保证并发安全。

**优势**：
- 持久化，可恢复
- 可观测（每个 task/flow 有状态）
- 支持外部系统查询和干预

**劣势**：
- 状态空间复杂（queued/running/succeeded/blocked/failed/cancelled/lost）
- 需要处理 revision 冲突

**适用场景**：长期运行、多外部系统协作的 workflow。

### 8.3 模式三：控制器驱动循环（AtomCode）

**描述**：GoalState 控制器持续驱动 agent 循环，直到满足条件或触及上限。

**优势**：
- 自主执行，无需人工干预
- 有评估器（LLM）做二元判定
- 恢复机制完善（pause/resume）

**劣势**：
- 评估器可能误判
- 循环次数/时间需要上限保护

**适用场景**：目标明确、可判定的任务（修 bug、加功能）。

### 8.4 模式四：Agent Loop 即 Workflow（OpenCode）

**描述**：workflow 就是 step 循环，每个 step 是一个 LLM 调用 + tool 执行。

**优势**：
- 极简，无额外抽象
- 流式处理，实时反馈
- doom_loop 检测防死循环

**劣势**：
- 无显式阶段划分
- 复杂编排需依赖外部 Todo

**适用场景**：通用对话式任务，无需复杂编排。

### 8.5 模式五：Coordinator 约定（Claude Code）

**描述**：workflow 是 Coordinator 系统 prompt 中的阶段约定，由模型决定如何编排。

**优势**：
- 灵活，模型可动态调整
- 并行扇出自然（单次消息多 tool-call）

**劣势**：
- 软约束，模型可能不遵循
- 不可观测（无持久化状态机）

**适用场景**：研究-实现-验证类任务，需要模型灵活性。

### 8.6 模式六：Lane 树分支（Pi）

**描述**：workflow 是树状 Lane 结构，每个分支是独立执行线。

**优势**：
- 时间线分支自然
- 队列驱动（steer/followUp）支持中断/续接
- 持久化 entry 链

**劣势**：
- 无显式阶段
- 分支管理复杂

**适用场景**：需要分叉探索、对比方案的任务。

### 8.7 模式七：拓扑排序 + 结构体（laew 当前）

**描述**：WorkFlowSpec 结构体 + `depends_on` 字段 + Kahn 算法拓扑排序。

**优势**：
- 显式依赖，可静态分析
- 拓扑排序检测循环依赖
- 结构体可序列化

**劣势**：
- 字段固定（steps/branches/loops），不图灵完备
- 无持久化
- 无运行时状态机

---

## 9. 对 laew 的综合建议

### 9.1 现状差距分析

laew 的 Main-Work WorkFlow 列表（`src/agent/main_work.rs`）当前是**最简形态**：

```rust
pub struct WorkFlowSpec {
    pub id: String,
    pub name: String,
    pub steps: Vec<String>,
    pub branches: Vec<BranchSpec>,   // condition/then
    pub loops: Vec<LoopSpec>,        // condition/over/max_iterations
    pub depends_on: Vec<String>,
    pub acceptance: Vec<String>,
    pub delegate_to: AgentRole,
}
```

**核心差距**：

| 维度 | laew 当前 | 业界最佳实践 | 差距 |
|------|-----------|-------------|------|
| 依赖执行 | 拓扑排序后串行 | DeepSeek `parallel`/`pipeline` 并发 | 无并发执行 |
| 运行时状态 | 内存态，无持久化 | OpenClaw TaskFlowRecord 持久化 | 无状态机 |
| 失败重试 | 整档回流 Yolo | 步骤级重试 + 跳过 | 粒度太粗 |
| 循环表达 | `LoopSpec.max_iterations` | DeepSeek 脚本 `while`/AtomCode Goal 控制器 | 无循环体 |
| 条件分支 | `BranchSpec.then`（字符串） | DeepSeek 脚本 `if`/OpenCode 模型决策 | 无可执行条件 |
| 与 SubAgent 关系 | 1 WorkFlow → 1 SubAgent | DeepSeek 1 脚本 → N agent() 调用 | 扇出能力弱 |

### 9.2 建议路线图

**P0（最小可用）：步骤级重试 + 并行扇出**

1. 为 `WorkFlowSpec` 增加 `max_retries` / `retry_delay_ms` 字段
2. Orchestrator 在 SubAgent 失败时按 `max_retries` 重试（而非整档回流）
3. 拓扑排序后，将 `depends_on` 为空的 WorkFlow 并发派发给多个 SubAgent
4. 增加 `WorkflowStatus` 枚举（pending/running/completed/failed/skipped）

**P1（状态机化）：持久化 + 可观测**

1. 引入 `WorkflowRun` 结构体，记录每个 WorkFlow 的执行状态
2. 持久化到 SQLite `workflow_runs` 表
3. 增加 revision 乐观锁（参考 OpenClaw）
4. 暴露 workflow 状态查询接口（供 TUI 展示进度）

**P2（组合子化）：脚本/表达式驱动**

1. 引入轻量 DSL 或 JSON 表达式描述 workflow 拓扑
2. 支持 `parallel`/`pipeline`/`branch` 三种组合子
3. 可选：集成 Rhai/Lua 脚本引擎（Rust 生态友好）
4. 参考 DeepSeek 的 `agent(prompt, opts)` 钩子设计

**P3（自主循环）：Goal 控制器**

1. 引入 `GoalState` 控制器（参考 AtomCode）
2. 支持目标条件 + 评估器 + 自动续轮
3. 增加 `max_rounds` / `max_duration` / `MAX_UNPRODUCTIVE` 熔断
4. 与 Quality-Check 层集成（每轮结束后 QC）

### 9.3 关键设计原则

1. **渐进增强**：保持 `WorkFlowSpec` 结构体向后兼容，新字段用 `#[serde(default)]`
2. **失败隔离**：步骤级失败不应杀整个 workflow，应支持 skip/retry/fallback
3. **可观测**：每个 WorkFlow 的状态变迁应可查询、可展示
4. **幂等保证**：WorkFlow ID + SubAgent 调用幂等，支持重放
5. **用户控制**：支持中断（steer）、续接（followUp）、分叉（fork）——参考 Pi

---

## 附录 A：laew 当前编排现状

### A.1 MultiAgentOrchestrator 编排链路

`src/agent/orchestrator.rs` 的 `handle()` 方法是总入口：

```rust
pub async fn handle(&self, session: &mut Session) -> Result<OrchestrationOutcome> {
    // 0) 项目上下文首次注入(幂等)
    // 0.1) 历史 Session 摘要注入(幂等)
    // 1) Yolo 入口 → classification
    loop {
        // 2) 调度执行
        let exec_result = match classification.task_level {
            TaskLevel::Simple => self.run_simple(...),   // → SubAgent
            TaskLevel::Medium => self.run_medium(...),   // → Main-Work → SubAgent
            TaskLevel::Hard => self.run_hard(...),       // → Plan → Main-Work → SubAgent
        };
        // 3) SessionContext 收口
    }
}
```

### A.2 Main-Work WorkFlow 列表

`src/agent/main_work.rs` 的 `MainWorkRunner`：

```rust
pub async fn plan_workflows(&self, goal, decomposition, session_id) -> Result<WorkFlowPlan> {
    // 构建 prompt → 调用 LLM → 解析 JSON → WorkFlowPlan
    let (text, usage) = self.agent.run_session(&mut sub_session).await?;
    let plan = parse_workflow_plan(&text).unwrap_or_else(|e| {
        // 解析失败 → 单 WorkFlow 兜底
        WorkFlowPlan { workflows: vec![WorkFlowSpec { id: "wf-1", ... }], ... }
    });
}
```

**拓扑排序**（`topo_sort`）：

```rust
pub fn topo_sort(workflows: &[WorkFlowSpec]) -> Result<Vec<WorkFlowSpec>> {
    // Kahn 算法；检测循环依赖
    // indegree = 该 wf 依赖的 wf 数
}
```

### A.3 失败回流机制

```rust
match exec_result {
    Ok(mut task_result) => {
        // SessionContext 收口 → 返回 Executed
    }
    Err(failure) => {
        if !failure.retryable {
            // 升级到上一层（Yolo 重新评估）
            classification = self.run_yolo_with_failure(...).await?;
            continue;
        }
        // retryable=true 留在当前档位继续重跑
        continue;
    }
}
```

### A.4 当前局限

1. **WorkFlow 解析脆弱**：LLM 输出 JSON 可能格式异常，当前用字符串匹配提取
2. **无并发**：拓扑排序后串行执行，未利用 `depends_on` 为空的并行机会
3. **无持久化**：WorkFlow 状态在内存中，进程丢失即丢失
4. **无步骤级重试**：SubAgent 失败整档回流，无单步重试
5. **无循环体**：`LoopSpec` 只有条件字符串，无可执行循环体
6. **无中断/续接**：用户无法在 WorkFlow 执行中干预

---

## 附录 B：各项目 workflow 定义代码片段速查

### B.1 DeepSeek-Harness：脚本钩子定义

```ts
// packages/workflow/tool-workflow/src/index.ts
const DESCRIPTION = `
Script-body hooks:
- agent(prompt, opts?): Promise<any> — run one subagent to completion.
- pipeline(items, ...stages): Promise<any[]> — run each item through the stages independently.
- parallel(thunks): Promise<any[]> — run zero-argument functions concurrently and await ALL.
- phase(title) — start a progress phase; log(message) — narrate progress; args — the tool call's args.
`
```

### B.2 DeepSeek-Harness：WorkflowEngine 服务契约

```ts
// packages/workflow/workflow/src/index.ts
export abstract class WorkflowEngine extends Service {
  abstract start(request: WorkflowStartRequest): WorkflowRun
}
export interface WorkflowRun {
  readonly id: WorkflowRunId
  readonly meta: WorkflowMeta
  readonly result: Promise<WorkflowResult>  // 永不出 reject
  cancel(reason?: string): void
  dispose(): Promise<void>
}
```

### B.3 DeepSeek-Harness：组合子 runtime

```ts
// packages/workflow/workflow-worker-thread/src/runtime.ts
private async parallel(rawThunks: unknown): Promise<unknown[]> {
  this.assertItemCap(rawThunks.length, 'parallel()')
  return Promise.all(thunks.map(async (thunk) => {
    try { return await thunk() }
    catch (error) {
      if (isFatalWorkflowError(error)) throw error
      return null  // 非致命 → 该 item 变 null
    }
  }))
}
private async pipeline(rawItems, rawStages): Promise<unknown[]> {
  // 每个 item 独立流过所有阶段，无跨阶段屏障
}
```

### B.4 Claude Code：Task 类型与状态

```ts
// src/Task.ts
export type TaskType = 'local_bash' | 'local_agent' | 'remote_agent'
  | 'in_process_teammate' | 'local_workflow' | 'monitor_mcp' | 'dream'
export type TaskStatus = 'pending' | 'running' | 'completed' | 'failed' | 'killed'
export type Task = { name, type, kill(taskId, setAppState): Promise<void> }
```

### B.5 AtomCode：Goal 状态机

```rust
// crates/atomcode-coding/src/controllers.rs
pub enum GoalPhase { Pursuing, Paused, PausedAtCap, Satisfied, Ended }
pub enum GoalTerminal { Met, Stopped, Failed, Cancelled }
pub(crate) const MAX_UNPRODUCTIVE: u32 = 5;
impl GoalState {
    pub fn resume(&mut self, new_max_rounds: u32) {...}      // 刷新预算
    pub fn pause_at_cap(&mut self, note) {...}               // 触及上限
    pub fn pause_for_recovery(&mut self, note) {...}         // 失败恢复
}
```

### B.6 AtomCode：Team 并发配置

```rust
// crates/atomcode-coding/src/team/manager.rs
pub struct TeamRuntimeConfig {
    pub max_concurrent: usize,    // 默认 3
    pub cancel_grace: Duration,   // 默认 2s
    pub max_result_chars: usize,  // 默认 12_000
    pub max_completed_runs: usize,// 默认 32
}
```

### B.7 OpenClaw：TaskFlow 状态机

```ts
// src/tasks/task-flow-registry.ts
type TaskFlowStatus = "queued" | "running" | "succeeded" | "blocked" | "failed" | "cancelled" | "lost"
function isTerminalTaskFlowStatus(status): boolean {
  return status === "succeeded" || status === "blocked" || status === "failed"
      || status === "cancelled" || status === "lost"
}
export type TaskFlowUpdateResult =
  | { applied: true; flow: TaskFlowRecord }
  | { applied: false; reason: "not_found" | "revision_conflict" | "persist_failed" }
```

### B.8 OpenClaw：Swarm 幂等 spawn

```ts
// src/agents/code-mode-swarm.runtime.ts
const requestFingerprint = `sha256:${createHash("sha256")
  .update(stableStringify(spawnInput)).digest("hex")}`
const idempotencyKey = `${params.codeModeRunId}:${params.request.id}`
let existing = getSwarmRunByLaunchReplayKey(idempotencyKey, requesterSessionKey, params.ctx.agentId)
if (existing) return replayedSpawnResult(existing)  // 冷启动重放
```

### B.9 OpenCode：Agent Loop

```ts
// packages/opencode/src/session/prompt.ts
const runLoop = function* (sessionID: SessionID) {
  while (true) {
    // 检查 lastAssistant.finish → 无 tool-calls 则 break
    // 检查 subtask/compaction 队列
    // 解析 agent + tools + permission
    const handle = yield* processor.create({ assistantMessage: msg, sessionID, model })
    const result = yield* handle.process({ user, agent, system, messages, tools, model })
    if (result === "stop") break
  }
}
```

### B.10 OpenCode：doom_loop 检测

```ts
// packages/opencode/src/session/processor.ts
case "tool-call": {
  const recentParts = parts.slice(-DOOM_LOOP_THRESHOLD)  // DOOM_LOOP_THRESHOLD = 3
  if (recentParts.every(part => part.type === "tool" && part.tool === value.name
      && JSON.stringify(part.state.input) === JSON.stringify(input))) {
    yield* permission.ask({ permission: "doom_loop", patterns: [value.name], ... })
  }
}
```

### B.11 OpenCode：retry 策略

```ts
// packages/opencode/src/session/retry.ts
export const RETRY_INITIAL_DELAY = 2000
export const RETRY_BACKOFF_FACTOR = 2
export const RETRY_JITTER_FACTOR = 0.25
export const RETRY_MAX_DELAY_NO_HEADERS = 30_000
export const RETRY_MAX_RETRIES = 5
function exponential(attempt: number, random: number) {
  const base = RETRY_INITIAL_DELAY * Math.pow(RETRY_BACKOFF_FACTOR, attempt - 1)
  return Math.ceil(base + base * RETRY_JITTER_FACTOR * random)
}
```

### B.12 Pi：Lane 存储

```ts
// packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts
export function startLaneOperation(db, sessionId, lane, runId) {
  const result = sql`UPDATE lanes SET open_operation_id = ${runId}
    WHERE session_id = ${sessionId} AND lane = ${lane} AND open_operation_id IS NULL`.run(db)
  if (result.changes === 1) return
  throw new SessionError("storage", `Lane ${lane} already has an open operation`)
}
```

### B.13 Pi：AgentSession 队列

```ts
// packages/coding-agent/src/core/agent-session.ts
export interface PromptOptions {
  streamingBehavior?: "steer" | "followUp";  // steer=中断, followUp=等待
}
private _steeringMessages: string[] = []   // 中断队列
private _followUpMessages: string[] = []   // 等待队列
```

### B.14 laew：WorkFlowSpec 结构体

```rust
// src/agent/main_work.rs
pub struct WorkFlowSpec {
    pub id: String,
    pub name: String,
    pub steps: Vec<String>,
    pub branches: Vec<BranchSpec>,   // { condition, then }
    pub loops: Vec<LoopSpec>,        // { condition, over, max_iterations }
    pub depends_on: Vec<String>,
    pub acceptance: Vec<String>,
    pub delegate_to: AgentRole,
}
```

### B.15 laew：拓扑排序

```rust
// src/agent/main_work.rs
pub fn topo_sort(workflows: &[WorkFlowSpec]) -> Result<Vec<WorkFlowSpec>> {
    // Kahn 算法；indegree = 该 wf 依赖的 wf 数
    // 检测循环依赖 → AgentError::WorkflowTopology
}
```

---

## 附录 C：建议实现方案详设

### C.1 P0 方案：步骤级重试 + 并行扇出

#### C.1.1 数据结构扩展

```rust
// src/agent/main_work.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkFlowSpec {
    pub id: String,
    pub name: String,
    pub steps: Vec<String>,
    pub branches: Vec<BranchSpec>,
    pub loops: Vec<LoopSpec>,
    pub depends_on: Vec<String>,
    pub acceptance: Vec<String>,
    pub delegate_to: AgentRole,
    // === 新增字段 ===
    #[serde(default)]
    pub max_retries: usize,           // 步骤级最大重试次数（默认 2）
    #[serde(default)]
    pub retry_delay_ms: u64,          // 重试间隔（默认 1000ms）
    #[serde(default = "default_skip_on_failure")]
    pub skip_on_failure: bool,        // 失败时是否跳过（默认 false）
    #[serde(default)]
    pub timeout_ms: Option<u64>,      // 单 WorkFlow 超时
}

fn default_skip_on_failure() -> bool { false }

/// WorkFlow 运行时状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkflowStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

/// WorkFlow 执行结果
#[derive(Debug, Clone)]
pub struct WorkflowRunResult {
    pub spec: WorkFlowSpec,
    pub status: WorkflowStatus,
    pub outcome: String,
    pub quality_report: QualityReport,
    pub usage: Usage,
    pub attempts: usize,              // 实际尝试次数
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
}
```

#### C.1.2 Orchestrator 并行调度

```rust
// src/agent/orchestrator.rs
async fn run_workflows_parallel(
    &self,
    workflows: &[WorkFlowSpec],
    classification: &TaskClassification,
    session: &mut Session,
) -> Result<Vec<WorkflowRunResult>, QualityFailure> {
    let ordered = main_work::topo_sort(workflows)?;
    let mut results: Vec<WorkflowRunResult> = Vec::new();
    let mut completed: Vec<String> = Vec::new();

    // 按依赖层级分批执行
    while completed.len() < ordered.len() {
        // 找出当前可执行的（depends_on 全部已完成）
        let ready: Vec<&WorkFlowSpec> = ordered.iter()
            .filter(|w| !completed.contains(&w.id))
            .filter(|w| w.depends_on.iter().all(|d| completed.contains(d)))
            .collect();
        if ready.is_empty() {
            return Err(QualityFailure { reason: "依赖死锁".into(), ... });
        }

        // 并发执行当前批次
        let futures: Vec<_> = ready.iter().map(|wf| {
            self.run_single_workflow(wf, classification, session)
        }).collect();
        let batch_results = join_all(futures).await;

        for result in batch_results {
            match result {
                Ok(r) => {
                    if r.status == WorkflowStatus::Completed {
                        completed.push(r.spec.id.clone());
                    }
                    results.push(r);
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(results)
}
```

#### C.1.3 步骤级重试

```rust
// src/agent/orchestrator.rs
async fn run_single_workflow(
    &self,
    spec: &WorkFlowSpec,
    classification: &TaskClassification,
    session: &mut Session,
) -> Result<WorkflowRunResult, QualityFailure> {
    let mut attempts = 0;
    let max_attempts = 1 + spec.max_retries;

    loop {
        attempts += 1;
        let input = SubFlowInput {
            id: spec.id.clone(),
            description: spec.name.clone(),
            expected_output: spec.acceptance.join("; "),
            depends_on_outputs: vec![],  // 可从前序 WorkFlow 结果收集
            sibling_outputs: vec![],
        };

        match self.sub_agent.run_unit(&input, session.id()).await {
            Ok(outcome) => {
                let qc = self.quality.check_subagent(...).await?;
                if qc.verdict == Verdict::Pass {
                    return Ok(WorkflowRunResult {
                        status: WorkflowStatus::Completed,
                        outcome: outcome.text,
                        quality_report: qc,
                        attempts,
                        ..
                    });
                }
                if attempts >= max_attempts {
                    return Ok(WorkflowRunResult {
                        status: if spec.skip_on_failure { WorkflowStatus::Skipped }
                               else { WorkflowStatus::Failed },
                        ...
                    });
                }
                // 退避等待
                tokio::time::sleep(Duration::from_millis(spec.retry_delay_ms)).await;
            }
            Err(e) => {
                if attempts >= max_attempts {
                    return Err(QualityFailure { ... });
                }
            }
        }
    }
}
```

### C.2 P1 方案：持久化状态机

#### C.2.1 数据库表

```sql
-- 新增 workflow_runs 表
CREATE TABLE IF NOT EXISTS workflow_runs (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    task_level      TEXT NOT NULL,       -- simple/medium/hard
    status          TEXT NOT NULL,       -- pending/running/completed/failed/skipped/cancelled
    spec_json       TEXT NOT NULL,       -- WorkFlowSpec JSON
    outcome_text    TEXT,
    quality_json    TEXT,                -- QualityReport JSON
    attempts        INTEGER DEFAULT 0,
    max_retries     INTEGER DEFAULT 2,
    error_text      TEXT,
    started_at      INTEGER,
    ended_at        INTEGER,
    created_at      INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at      INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE INDEX idx_workflow_runs_session ON workflow_runs(session_id, status);
```

#### C.2.2 状态推进

```rust
// src/config/workflow_run.rs
pub struct WorkflowRunRecord {
    pub id: String,
    pub session_id: String,
    pub status: WorkflowStatus,
    pub spec_json: String,
    pub attempts: usize,
    ...
}

impl WorkflowRunRecord {
    pub fn can_transition_to(&self, new_status: WorkflowStatus) -> bool {
        matches!((&self.status, &new_status),
            (WorkflowStatus::Pending, WorkflowStatus::Running)
          | (WorkflowStatus::Running, WorkflowStatus::Completed)
          | (WorkflowStatus::Running, WorkflowStatus::Failed)
          | (WorkflowStatus::Running, WorkflowStatus::Skipped)
          | (WorkflowStatus::Running, WorkflowStatus::Cancelled)
          | (WorkflowStatus::Failed, WorkflowStatus::Running)  // 重试
        )
    }
}
```

### C.3 P2 方案：轻量 DSL（可选 Rhai 集成）

#### C.3.1 设计原则

- 不引入完整脚本引擎（避免安全风险）
- 用 JSON 表达式描述拓扑
- 条件分支用简单表达式（成功/失败/输出匹配）

#### C.3.2 JSON DSL 示例

```json
{
  "workflows": [
    { "id": "research", "name": "调研", "steps": ["搜索代码库"] }
  ],
  "execution": {
    "phases": [
      { "name": "研究", "parallel": ["research_a", "research_b"] },
      { "name": "实现", "depends_on": ["research"], "steps": ["implement"] },
      { "name": "验证", "depends_on": ["implement"], "steps": ["verify"] }
    ],
    "on_failure": { "retry": 2, "fallback": "skip" }
  }
}
```

#### C.3.3 与现有 WorkFlowSpec 的映射

| DSL 概念 | WorkFlowSpec 字段 |
|---------|------------------|
| parallel | `depends_on = []` 的多个 wf |
| depends_on | `depends_on` 字段 |
| on_failure.retry | `max_retries` |
| on_failure.fallback | `skip_on_failure` |
| phase | 未来扩展字段 |

### C.4 实施优先级总结

| 优先级 | 工作量 | 收益 | 关键改动 |
|--------|--------|------|---------|
| P0 步骤级重试 | 低 | 高 | `WorkFlowSpec` 加字段 + Orchestrator 重试循环 |
| P0 并行扇出 | 中 | 高 | `topo_sort` 后分批 `join_all` |
| P1 持久化 | 中 | 中 | 新建表 + CRUD + 状态机校验 |
| P1 可观测 | 低 | 中 | TUI 展示 workflow 进度 |
| P2 DSL | 高 | 中 | JSON Schema + 解析器 |
| P3 Goal 控制器 | 高 | 低 | 独立控制器 + 评估器 |

---

> **结语**：Workflow 设计是 Agent 架构的「骨架」。laew 当前的 WorkFlow 列表已具备拓扑排序和依赖表达的基础，但在并发执行、步骤级重试、持久化状态机、可中断/续接等方面与业界最佳实践存在差距。建议按 P0→P1→P2→P3 渐进增强，优先实现步骤级重试和并行扇出（投入小、收益大），再逐步补齐持久化和可观测能力。DeepSeek 的脚本化组合子和 OpenClaw 的持久化状态机是两个最值得深入借鉴的参考实现。
