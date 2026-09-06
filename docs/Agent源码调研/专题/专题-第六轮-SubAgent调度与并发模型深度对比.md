# 专题-第六轮-SubAgent 调度与并发模型深度对比

> **范围**：atomcode / claudecode / deepseek-harness / openclaw / opencode / pi 六个项目 SubAgent **调度**与**并发**模型的逐行源码深度对比
>
> **专题定位**：与已有的 `专题-SubAgent与多Agent架构深度分析.md`(侧重架构与编排)、`专题-第五轮-中断取消与后台任务深度分析.md`(侧重取消/后台)、`专题-第五轮-工具结果回填与消息组装深度分析.md`(侧重消息流)互为补充；本文聚焦"调度算法 + 并发模型"，包括：信号量/许可、FIFO 公平、深度限制、并发上限、乐观锁、跨进程传输、嵌套隔离、持久化等子主题。
>
> **数据来源**：6 个项目仓库实际源码 + 80+ 文件 ~12k 行精读
>
> **关联文档**：`专题-SubAgent与多Agent架构深度分析.md`、`专题-第五轮-中断取消与后台任务深度分析.md`、`专题-第五轮-工具结果回填与消息组装深度分析.md`、`专题-第六轮-Skill系统深度对比.md`

---

## 目录

1. [SubAgent 调度在大模型 Agent 系统中的位置](#1-subagent-调度在大模型-agent-系统中的位置)
2. [laew 现状与缺口](#2-laew-现状与缺口)
3. [横向对比总表](#3-横向对比总表)
4. [atomcode 的 SubAgent 调度](#4-atomcode-的-subagent-调度)
5. [claudecode 的 SubAgent 调度](#5-claudecode-的-subagent-调度)
6. [deepseek-harness 的 SubAgent 调度](#6-deepseek-harness-的-subagent-调度)
7. [openclaw 的 SubAgent 调度](#7-openclaw-的-subagent-调度)
8. [opencode 的 SubAgent 调度](#8-opencode-的-subagent-调度)
9. [pi 的 SubAgent 调度（lane 并发模型）](#9-pi-的-subagent-调度lane-并发模型)
10. [十二维度横向专题](#10-十二维度横向专题)
11. [laew 借鉴路线图与具体实现方案](#11-laew-借鉴路线图与具体实现方案)
12. [附录 A：关键文件索引](#附录-a关键文件索引)

---

## 1. SubAgent 调度在大模型 Agent 系统中的位置

### 1.1 调度的三个核心问题

当父 Agent 需要把"任务的一个子片段"委派给子 Agent 时，必须回答三个问题：

| 问题 | 变体 |
|------|------|
| **谁** | 是同进程内嵌子 Agent，还是跨进程子 Agent（外部 CLI / ACP / SDK）？是同协议 fork 还是新 session？ |
| **何时** | 现在同步跑，还是放进后台队列？是 FIFO 排队还是有优先级？是否要抢许可？ |
| **如何回** | 子 Agent 完成后如何把结果回填给父？要不要持久化 resume？跨进程如何 kill 子树？ |

六项目对这三个问题给出了截然不同的答案：

```
        谁              何时              如何回
       ─────           ─────             ─────
atomcode  进程内+外部  Semaphore(3) FIFO  内存 BTreeMap(team)
claudecode 4 层 + 远程  sync/async 二态   SendMessage + 文件系统 scratchpad
deepseek  6 种传输   显式 capability     descriptor + cold resume
openclaw  swarm 调度器  active/queued 二态  SQLite + SubagentRegistry
opencode 1 层 task    bus background     session 表 + BackgroundJob
pi       lane 并发  steer/followUp/nextRun  LaneSnapshot + lane_moves
```

### 1.2 调度模型的差异光谱

```
极简 ──────────────────────────────────────────────── 极复杂
  opencode    pi        atomcode     deepseek-harness    claudecode       openclaw
  (1 layer)   (3 queue)  (Semaphore)   (6 transports)     (4 layers)       (swarm)
```

- **opencode** 最简单：单层 Task 工具 + BackgroundJob，依赖 SQLite `session.parent_id` 自描述父子关系
- **pi** 独有"lane 并发"——一个 session 内多个独立运行轨（lane），每条 lane 三队列驱动
- **atomcode** 经典 Semaphore(3) 限制 + Team 六元事件流
- **deepseek-harness** 用 seam 抽象出 6 种传输，每种都有 capability 显式声明
- **claudecode** 4 层（Fork / AgentTool / Task / Swarm）+ 跨进程异步后台 + SendMessage 投递
- **openclaw** 最完整：spawn pipeline 三阶段（initialize/dispatch/register）+ Swarm Lane + 注册表 + announce/ + collector

---

## 2. laew 现状与缺口

### 2.1 laew 当前的 SubAgent 模型

`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/agent/orchestrator.rs:99-148` 持有六个 Runner：

```rust
pub struct MultiAgentOrchestrator {
    yolo: YoloRunner,                    // 入口层
    plan: PlanRunner,                     // 规划层
    main_work: MainWorkRunner,            // 流程层
    sub_agent: SubAgentRunner,            // 执行层
    quality: QualityRunner,               // 质检层
    session_context: ContextSessionRunner, // 会话层
    db: Arc<Db>,
    cfg: OrchestratorConfig,              // 默认 max_retry=3, subagent_max_iterations=16
}
```

**关键观察**：

1. **执行层只有 1 个 SubAgentRunner**（`orchestrator.rs:103, 132-134`），整个进程只能串行跑 sub_agent 调用。WorkFlow 是**串行拓扑排序**（`orchestrator.rs:443-518` `execute_workflows` 用 `for wf in ordered`），同一时间永远只有 1 个 SubAgent 跑。

2. **WorkFlow 拓扑依赖**通过 `MainWorkRunner::topo_sort`（`main_work.rs:137-190`）做 Kahn 算法：每个 wf 的 `depends_on` 长度=入度=0 才进队（`main_work.rs:160-168`），最终 `for wf in ordered` 串行执行。

3. **依赖产出物通过 in-memory HashMap 传递**（`orchestrator.rs:457-500`）：
```rust
let mut dep_outputs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
...
for wf in ordered {
    let sub_input = build_subflow_input(&wf, &dep_outputs);
    let sub_outcome = self.sub_agent.run_unit(&sub_input, session.id()).await?;
    ...
    dep_outputs.insert(wf.id.clone(), sub_outcome.text.clone());
}
```
没有并行：哪怕 wf-2 与 wf-3 互相独立，也得等 wf-1 完才能启动 wf-2。

4. **SubAgent 输入构造**（`subagent.rs:17-54`）：
```rust
pub struct SubFlowInput {
    pub id: String,
    pub description: String,
    pub expected_output: String,
    #[serde(default)] pub depends_on_outputs: Vec<String>,
    #[serde(default)] pub sibling_outputs: Vec<String>,
}
```
支持 `depends_on_outputs` 和 `sibling_outputs`——但**实际上 sibling_outputs 永远为空**（`orchestrator.rs:627` 字段硬编码为 vec![]），因为串行执行时 wf-1 的产物只能通过 HashMap 传给 wf-2 等"明确依赖者"，同层无依赖的 wf 不会共享。

5. **会话隔离**：每次 SubAgentRunner::run_unit 都新建独立 Session（`subagent.rs:91-97`），但**不是真正并行**。

6. **持久化**：每个 SubAgent 完成时写入 Agent-Memory（`subagent.rs:99-108`），Orchestrator 写入 session_memory 表，但 **不持久化 WorkFlow 执行状态**——崩溃后重跑会从 WorkFlow 1 重新开始，没有 resume 能力。

7. **重试**：通过 `max_retry_per_level: 3`（`orchestrator.rs:32, 174-189`）在档位内循环，超过则失败。

8. **深度限制**：`AgentRole::SubAgent` 的工具集是 `sub_agent_work_registry()`（`profile.rs:79-86`），含 Bash/Read/Write，**没有 Task 工具**——理论上无法递归 spawn，但没有显式的 `MAX_DEPTH` 常量。

9. **取消传播**：`session.rs` 的 Session 没有 AbortSignal 字段，`Agent::run_session`（`mod.rs:78-152`）没有取消检查——`/exit` 直接退出进程，Esc 中断只能 kill 当前 LLM 请求，无法级联到 SubAgent。

### 2.2 laew 当前的并发缺口

| 维度 | laew 现状 | 改进优先级 |
|------|----------|-----------|
| **SubAgent 并行** | 全部串行 | P0（最迫切） |
| **WorkFlow 并行（同层无依赖）** | 串行 | P0 |
| **嵌套 SubAgent** | 工具集硬性禁止 | P2 |
| **后台 SubAgent** | 无（必须同步等） | P1 |
| **深度限制常量** | 无（靠工具集间接限制） | P1 |
| **持久化 resume** | 无（崩溃后从头跑） | P2 |
| **取消传播到 SubAgent** | 无（仅靠退出进程） | P0 |
| **SubAgent 调度仪表盘** | 无 | P2 |
| **跨进程 SubAgent 委派** | 无 | P3 |

---

## 3. 横向对比总表

### 3.1 基础对比表

| 维度 | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi | **laew** |
|------|----------|------------|------------------|----------|----------|-----|---------|
| **抽象核心** | `SubagentBackend` trait + Team 六元事件 | `AgentTool` (4 层) + `AGENT_TOOL_NAME` | `SubagentProvider` 接口 + 11 个包 | `spawnSubagentDirect` 9 阶段 + `SwarmGroupLane` | `TaskTool` + `Agent.Info` schema | `AgentLane` + LaneSnapshot | **Orchestrator + 6 Runner** |
| **进程模型** | 进程内 + 外部子进程 (claude/codex) | 进程内 4 层 + 远程 CCR + tmux teammate | 6 种 transport (spawn/fork/acp/cc/codex/dsh-sdk) | 进程内 native + ACP 跨进程 | 进程内 session 树 | 进程内 lane (单进程) | **进程内 6 Runner** |
| **并发上限** | Semaphore(3) | AgentTool 无；LocalShellTask 后台 | 无全局上限；per-child `ChildLock` | Swarm 默认 maxConcurrent=8；`reserveSwarmRun` | 无全局上限；每 session 单 run | 单 lane 单 operation | **1 (串行)** |
| **队列** | FIFO (tokio Semaphore) | in-process async + 后台任务队列 | `Job.start({ kind: 'subagent', label })` + 优先级无 | `lane.queue: QueuedSwarmRun[]` FIFO | BackgroundJob 单 run + background waitlist | 三队列 (steer/followUp/nextRun) | **无队列** |
| **持久化** | 内存 BTreeMap (无 SQLite) | scratchpad 文件系统 + SendMessage | `subagent/descriptor` session event + SQLite | SQLite `subagent_runs` 表 + 进程内 `SubagentRunMap` | SQLite `session.parent_id` + BackgroundJob 内存 | SQLite `lanes` + `records` + `lane_moves` + `sessions` | **SQLite agent_memory + session_memory** |
| **Resume 能力** | 无 | SendMessage / `run_in_background` | cold resume (descriptor fold) | restart-recovery receipt 5 态 | BackgroundJob continue | lane resume + Operation started/finished | **无** |
| **深度限制** | 通过工具白名单硬性排除 (task/team) | fork-in-fork 检测 (`isInForkChild`) | `tool-subagent.maxDepth: 3` 默认 + `delegationDepth` | `DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH = 1` | `cfg.subagent_depth ?? 1` + task deny 兜底 | `LaneRecord` + lane isolation | **工具集隐式禁止** |
| **取消原语** | `tokio_util::CancellationToken` + setsid killpg | AbortController + tmux SIGTERM | `request.signal.aborted` + EOF grace SIGTERM→SIGKILL | `chat.abort` + `deleteSubagentSessionForCleanup` | `ctx.abort` listener + `Effect.interrupt` | `AbortRequestedRecord` record | **无** |
| **跨进程传输** | setsid + Job Object (killpg SIGKILL) | ssh remote (CCR) + tmux (teammate) | subprocess spawn (acp/claude-code/codex/dsh-sdk) | subprocess spawn (ACP NDJSON) | 进程内 (无外部) | 进程内 (无外部) | **无** |
| **嵌套 SubAgent** | 禁止 (child 工具集不含 task/team) | fork-in-fork 抛错；subagent_type 仍可调 | 允许（`delegationDepth` 沿父链 +1） | leaf 角色禁止 (canSpawn=false) | `subagent_depth` + task deny | 跨 lane 但不能同 lane 嵌套 | **禁止 (无 task 工具)** |
| **乐观锁** | seq: AtomicU64 + 单锁 emit | 字节级 prompt cache 比对 | descriptor 版本号 + capability check | `SubagentRestartRecoveryReceipt.phase` (5 态) | SQLite session.id | `open_operation_id` 字段 + `startLaneOperation` SQL UPDATE | **无** |
| **进程内 capacity 共享** | `tokio::sync::Semaphore::new(3)` | AgentTool 串行 / background 并行 | `ChildLock.run(childId, op)` 串行化 per-child | `ChildLock` (deepseek) / `lane.limit` (openclaw) | BackgroundJob 单 per-session | SQLite lane 单 operation | **无共享** |

### 3.2 调度模式光谱

```
串行 ──────────────────────────────────────────── 完全并行
  laew        opencode     atomcode    openclaw     claudecode       pi
  (硬串行)    (1 run)      (Sem=3)     (swarm)      (background)     (lane)
```

---

## 4. atomcode 的 SubAgent 调度

> 本节主要基于探索代理 "调研 atomcode SubAgent 调度" 的输出，并补充交叉验证。

### 4.1 SubAgent 抽象：SubagentBackend trait

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/subagent/mod.rs:307`：

```rust
#[async_trait]
pub trait SubagentBackend: Send + Sync {
    fn name(&self) -> &str;
    fn kind(&self) -> SubagentKind;
    fn capabilities(&self) -> SubagentCapabilities;
    async fn run(&self, req: SubagentRun) -> Result<SubagentResult, SubagentError>;
}
```

**两套实现**：

| 实现 | 文件 | 行数 | 进程模型 | 二进制 |
|------|------|------|----------|--------|
| `ClaudeCodeBackend` | `subagent/claude_code.rs` | 365 | `claude -p --output-format json` (one-shot) | `claude` |
| `CodexBackend` | `subagent/codex.rs` | 641 | `codex exec --json` (one-shot + JSONL) | `codex` |

**关键差异**（同属"spawn 子进程 + stdin/stdout JSON 通信"）：
- `ClaudeCodeBackend`：`StdinMode::Piped` + 单 JSON 对象取 `result` 字段（`claude_code.rs:178-191`）；`--permission-mode plan/acceptEdits/auto`，Bypass 用 `--dangerously-skip-permissions`（`claude_code.rs:80-88`）
- `CodexBackend`：JSONL 解析 `item.started`/`item.completed` 转 activity 行（`codex.rs:166-198`）；`--sandbox read-only/workspace-write`，Bypass 用 `--dangerously-bypass-approvals-and-sandbox`（`codex.rs:86-94`）

**共享进程包装器** `subagent/proc.rs`（456 行）：
- `setsid` + Job Object 进程树清理（`proc.rs:114-167`）
- `POST_DRAIN_GRACE = 30s`（`proc.rs:36`）
- `STDERR_TAIL_CAP = 2000` 字节（`proc.rs:29`）
- `ManagedChild::Drop` 兜底：即使 wait 未跑完也调 `kill_tree`（`proc.rs:329-341`）

### 4.2 Team 架构：TeamRunManager + TeamEvent 六元事件

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/team/manager.rs`：

```rust
pub struct TeamRuntimeConfig {
    pub max_concurrent: usize,   // 默认 3（manager.rs:34）
    pub cancel_grace: Duration::from_secs(2),
    pub max_result_chars: 12_000,
    pub max_completed_runs: 32,
}

pub struct TeamRunManager {
    inner: Arc<Inner>,            // store + generation + emit_lock
    ...
}
```

`Inner`（`manager.rs:92-107`）关键字段：
- `store: Mutex<TeamRunStore>` — 内存 BTreeMap
- `generation: AtomicU64` — 运行时 generation（替换旧批次时 `begin_generation` 让旧事件失效）
- `generation_root: Mutex<CancellationToken>` — 整代根取消 token
- `event_tx: RwLock<Option<UnboundedSender>>` — 外部订阅
- `emit_lock: Mutex<()>` — 保证 `seq.fetch_add` 与 `send` 在同一临界区
- `external_runs: Mutex<BTreeMap<run_key, generation>>` — 给同步 task 工具记录 generation 起点
- `run_counter: AtomicU64`

**TeamEvent 六元事件**（`crates/atomcode-capabilities/src/team.rs:334-371`）：

```rust
pub enum TeamEventPayload {
    RunStarted    { total: usize },
    MemberQueued  { member_id, role: TeamRoleId, model: String, description: String },
    MemberStarted { member_id, role: TeamRoleId, model: String, description: String },
    MemberActivity{ member_id, activity: String, output_tokens: u64 },
    MemberFinished{ member_id, success: bool, stop: String, summary: String, output_tokens: u64 },
    RunFinished   { total: usize, completed: usize, failed: usize },
}
```

**事件驱动流程**（manager.rs）：
- `delegate` (manager.rs:225-338) → 发 `RunStarted`，每个 task 状态 `Queued` 注入 `MemberQueued`，再 `tokio::spawn` worker
- worker 里 `semaphore.acquire_owned()` → `mark_started` 发 `MemberStarted`（manager.rs:473-502）
- `activity(line, tokens)` → `member_activity` 发 `MemberActivity`（manager.rs:504-540）
- 收尾 → `finish_member` 发 `MemberFinished`，整 run 终态再发 `RunFinished`（manager.rs:542-606）

**TUI 完整 6 分支**： `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-tuix/src/team.rs:60-150`

### 4.3 并发控制：Semaphore(3)

`manager.rs:265-267`（按 run 独立创建许可）：

```rust
let semaphore = Arc::new(tokio::sync::Semaphore::new(
    self.inner.config.max_concurrent.max(1),
));
```

`manager.rs:302-306`（每个 spawn 的 worker 抢许可，与 cancel 并发 race）：

```rust
let handle = tokio::spawn(async move {
    let permit = tokio::select! {
        permit = semaphore.acquire_owned() => permit.ok(),
        _ = child_cancel.cancelled() => None,
    };
    if permit.is_none() { ... return; }
    ...
});
```

**经典 `task` 工具也用 Semaphore(3)**：
- `crates/atomcode-capabilities/src/tools/task.rs:23` `DEFAULT_MAX_CONCURRENT: usize = 3;`
- `task.rs:862` `let sem = Arc::new(tokio::sync::Semaphore::new(self.max_concurrent));`
- `task.rs:934` `let _permit = sem.acquire_owned().await.expect("semaphore not closed");`

**队列行为**：FIFO（tokio Semaphore 保留到达顺序）。每个 `delegate` 内启动顺序按 `tasks.into_iter().enumerate()`（manager.rs:268），无显式优先级。**重试无**——`SubagentError` 直接透传给父。

### 4.4 TaskTool 输入 Schema

`crates/atomcode-capabilities/src/tools/task.rs:689-722`：

```rust
"tasks": [{
  "description": "3-5 word label",
  "prompt":      "The full subtask for the subagent",
  "subagent_type": "explore" | "worker",
  "difficulty":  "simple" | "hard",
  "model":       "Optional configured model selection id",
  "role":        "planner | architect | explorer | implementer | rust | tui_ux | reviewer | tester | debugger | security | performance | docs_writer | release_manager | migration_compat",
  "scope":       ["globs"],   // Worker-only REQUIRED
}]
```

**Agent 选择逻辑**（task.rs:807-859）：
1. 有 `model` → 用 `make_named_provider` 解析；找不到且有 host → fallback 到 host
2. 否则按 `difficulty == Hard` 选 `make_capable_provider`，否则 `make_fast_provider`
3. Worker + Hard + 无显式 model + 有 host → 携带 host fallback（仅在 hard/explore 的"内容为空"瞬时错误时使用，见 `retryable_content_free_failure`，task.rs:1550-1565）

**返回父 agent**：`aggregate_task_result`（task.rs:1656-1724）用 XML 块包装：
- 成功：`<task id="..." model="..." state="..."><task_result>...</task_result></task>`
- 非成功：`<task_error>...</task_error>` + partial output
- 仅全部子任务失败时 `is_error=true`，部分失败保留幸存者（task.rs:1715-1723）
- `PolicyDenied` 子任务被脱敏成 `SANITIZED_POLICY_BLOCK_BODY`（task.rs:1645, 1669-1679）

### 4.5 上下文传递

**子 agent 不继承父对话历史**——它接收 `task.prompt` 这一个字符串：
- `team/runner.rs:140-142` `run_to_completion(task.prompt, AutoRespond::AllowAll)`
- `task.rs:1578-1582` 同样的 `SendMessage{ text: input }`
- 注释明确："include all needed context" 在外部 subagent 里也要这样（tool.rs:113-114）

**传给父 agent**：子任务 text（assistant 输出）+ summary（首行裁 120 字符，task.rs:1065-1068）+ completed/error/blocked 状态

### 4.6 权限继承：硬拒绝而非 prompt

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-coding/src/parts.rs:603-630`：

```rust
let explore_names = ["read_file", "grep", "glob", "list_directory"];
let worker_names  = ["read_file", "edit_file", "write_file", "bash",
                     "grep", "glob", "search_replace", "list_directory"];
let make_explore_tools = move || child_reg.mount(&explore_refs);
let make_worker_tools  = move || child_reg.mount(&worker_refs);
```

→ `task` / `team` 工具**不在 child 工具集里**——子 agent 不能递归 spawn

**worker 额外限制**（task.rs:386-449）：
- `CredentialBashGate::non_interactive` — credential shell 强制拒绝（AutoRespond::AllowAll 自批准前提）
- `DenySensitiveFiles` — 拒 `~/.ssh`、`.env`、cloud creds（task.rs:70-88）
- `WorkerScopeGate`（task.rs:186-366）— 把写操作 glob 锁在 `scope[]` 内；对 `.git` 内文件**永远**拒绝（task.rs:300-304）

**team worker 进一步**（task.rs:469-484）：`confine_reads=true`，连读也被锁在 scope 里。

**DenyTeamBash**（runner.rs:254-272）只对 team 角色生效："team child may not run bash; verification remains owned by the parent agent"

### 4.7 失败处理

**子进程崩溃 / 非零退出码**（外部 subagent）`claude_code.rs:193-213` 与 `codex.rs:284-294`：

```rust
match outcome {
    WaitOutcome::Exited(status) if status.success() => { ... Completed }
    WaitOutcome::Exited(status) => Err(SubagentError::NonZeroExit {
        code: status.code(), stderr_tail,
    }),
    WaitOutcome::TimedOut   => Ok(result(output, SubagentStopReason::Timeout)),
    WaitOutcome::Cancelled  => Ok(result(output, SubagentStopReason::Cancelled)),
}
```

`ClaudeCodeBackend` 把 `is_error:true` 视为 `AgentError`（`claude_code.rs:198-202`）——区分"驱动层失败" vs "agent 自报失败"。

**重试逻辑**（仅 `task` 内部 subagent，task.rs:1006-1050）：

```rust
if fallback_provider.is_some()
    && retryable_content_free_failure(&outcome)
    && !child_cancel.is_cancelled()
{
    // 用 host fallback 跑一遍
}
```

`retryable_content_free_failure`（task.rs:1550-1565）仅在 `outcome.text.is_empty() && outcome.tool_results.is_empty()` 且 `StopReason ∈ {Timeout, RateLimited}` 或 `ProviderError +408/425/429/5xx` 时为 true。**仅一次 fallback 到 host provider**。

**超时**：`SubagentRun` 整体 `DEFAULT_TIMEOUT = 600s`（claude_code.rs:29, codex.rs:30），`ManagedChild::wait_or_kill` 用 `tokio::select!` 把 timeout / cancel / exit 三者 race（proc.rs:223-247）。`task` 的 child 用 `stream_timeout`（事件空闲 liveness cap，task.rs:539-543, 613-616）——注释特意指出 SSE keep-alive 字节会让 byte-idle 超时失效。

### 4.8 持久化

**未找到** SQLite / 文件落盘持久化 team 或 task 状态。`TeamRunManager.store` 是纯内存 BTreeMap（manager.rs:93, 786-816），`max_completed_runs: 32`（manager.rs:37）只是内存里保留最近 32 个终态 run（manager.rs:604, 800-815）。**崩溃后无法恢复**。

> `grep rusqlite / SQLite` 在 `atomcode-coding/src` 下零命中。

### 4.9 中断与取消

**传播路径**（外部 subagent）：
- `ctx.cancel` → `SubagentRun.cancel` → `ManagedChild::wait_or_kill` 的 `tokio::select!{ biased; cancel.cancelled() => kill_tree(); ... }`（proc.rs:228-247）
- `kill_tree` Unix 走 `setsid` + `killpg(SIGKILL)`（proc.rs:131-144, 185-195），Windows 走 Job Object + `taskkill /T`（proc.rs:156-158, 186-188）
- `Drop` 兜底：`kill_tree` 仍被调用（proc.rs:329-341）

**Team 取消**：manager.rs:369-373 `stop` → `cancel_members` 设 `Cancelling` + 触发 `child_cancel`（manager.rs:444-471），然后 `finish_cancelled_after_grace`（manager.rs:389-442）等 `cancel_grace`（默认 2 秒），到时还没终态就 `abort.abort()` 强行杀 tokio task。`stop_all`（manager.rs:375-383）取消 `generation_root`，整代一刀切。

**Ctrl+C**：由 `CancellationToken` 在调用入口处的整体 cancel tree 接管；manager 通过 `Drop` 的 `generation_root.cancel()`（manager.rs:111-115）保证进程退出时也清理。

### 4.10 嵌套 SubAgent

**TaskTool 的工具集不含 `task` 或 `team`**——子 agent **不能**再派生子任务（task.rs:664-665 description："Subagents run in parallel and cannot themselves dispatch"）。

**未找到**显式的深度常量（如 `MAX_AGENT_DEPTH`）—— 通过"工具白名单排除 task/team"实现硬性限制。

### 4.11 监控与日志

**事件流（typed Team events）**：
- 源：`TaskTool` 用 `TaskEventEmitter.emit`（task.rs:47-56）经 `with_team_event_sink` 推到 manager 的 `publish_external`（parts.rs:684-686）
- 转发：manager → `event_tx: UnboundedSender<GenerationTeamEvent>`（manager.rs:96, 149-158, 211-222）
- 接收：runtime 在 `runtime.rs:695-697` 处取出 `GenerationTeamEvent` 并应用到状态机
- TUI 投影：`crates/atomcode-tuix/src/team.rs:48-150` 六个分支完整还原状态

**Live activity 流**：与 TeamEvent 并列但走 `ProgressSink`：
- 外部 subagent 把 stdout 一行行转 `SubagentEvent::Activity`（subagent/codex.rs:262-268）；claudecode 暂未流式
- Team runner：`TeamProgressHook.on_text_delta`/`on_model_response` 把字符累积/工具调用转成 `TeamActivitySink`（runner.rs:213-252）
- Task runner：`SubtaskProgressHook`（task.rs:1248-1507）输出 `\u{1e}` 前缀的 ephemeral 行（`SUBAGENT_ACTIVITY_MARKER`）→ TUI 路由到 in-place spinner

### 4.12 atomcode 最有特色的 3 个设计点

1. **Generation 隔离 + 单锁 seq 顺序保证**：`TeamRunManager` 用 `generation: AtomicU64` + `external_runs: BTreeMap<run_key, generation>`（manager.rs:104, 162-176, 181-223），`begin_generation` 时换 root cancel token 让旧批次事件在 `emit` 阶段被静默丢弃。单锁 `emit_lock` 把 `seq.fetch_add` 与 `sender.send` 绑在同一临界区（manager.rs:629-633），避免下游单调 `seq` 过滤器因 race 永久丢失低序号事件。

2. **进程树级取消 + Drop 兜底**：外部 subagent 用 `setsid` + `killpg(SIGKILL)` 和 Job Object + `taskkill /T`（proc.rs:131-144, 184-195），`ManagedChild::Drop` 兜底——只有当 `wait()` 已经 reap 后才跳过。

3. **进程内 subagent 的"硬拒绝而非 prompt"权限继承**：因 child 跑 `AutoRespond::AllowAll`，所有安全敏感的工具调用必须 `BeforeOutcome::deny_turn` 而非 `Prompt`（task.rs:60-70, 427-433），否则 prompt 会自批准。`WorkerScopeGate` 把写路径 glob 锁死、对 `.git` 内永远拒绝，`scope` 在 `delegate` 阶段就被 `validate_non_overlapping_worker_scopes` 静态校验（team.rs:152-188），并行 worker 不能写同一文件。

---

## 5. claudecode 的 SubAgent 调度

### 5.1 SubAgent 抽象：4 层架构

claudecode 的 SubAgent 不是一个单一概念，而是由四层独立的委派机制组成：

| 层 | 触发 | 进程模型 | 工具集 |
|----|------|----------|--------|
| **Fork** | Agent tool 省略 `subagent_type` | 同进程 in-process | **继承父**（但 fork-in-fork 抛错） |
| **AgentTool** | Agent tool 指定 `subagent_type` | 同进程 in-process query | 按 agent 定义 |
| **Task tool** | `Task`/`TaskCreate`/`TaskUpdate` 系列 | 同进程 task 对象 | 跟 Task 类型 |
| **Teammate (Swarm)** | `team_name` + `name` 参数 | tmux 子进程 + SendMessage | 受限制（`ASYNC_AGENT_ALLOWED_TOOLS`） |
| **Remote** | `isolation: "remote"` | 跨进程 CCR（`teleportToRemote`） | 受限制 |

> 备注：claudecode 的"4 层"在本仓库版本实际为 **5 层**：Fork / AgentTool / Task tool / Teammate (Swarm via tmux) / Remote isolation via CCR。

### 5.2 AgentTool 输入 Schema

`/usr/local/LsmGitOpenSource/claudecode/src/tools/AgentTool/AgentTool.tsx:85-87`：

```ts
subagent_type: z.string().optional().describe('The type of specialized agent to use for this task'),
run_in_background: z.boolean().optional().describe('Set to true to run this agent in the background. You will be notified when it completes.')
```

**核心字段**（AgentTool.tsx:81-130 + prompt.ts/prompt_cn.ts）：
- `description: string`（3-5 词描述）
- `prompt: string`（完整任务）
- `subagent_type?: string`（指定 agent 类型）
- `run_in_background?: boolean`
- `team_name?: string`（multi-agent spawn）
- `name?: string`（spawn teammate）
- `isolation?: 'work' | 'remote'`（远程隔离）
- `model?: string`（覆盖默认模型）

**Fork vs AgentTool 决策树**（AgentTool.tsx:319-323）：
```ts
// - subagent_type set: use it (explicit wins)
// - subagent_type omitted, gate on: fork path (undefined)
// - subagent_type omitted, gate off: default general-purpose
const effectiveType = subagent_type ?? (isForkSubagentEnabled() ? undefined : GENERAL_PURPOSE_AGENT.agentType);
const isForkPath = effectiveType === undefined;
```

**Fork-in-fork 守卫**（AgentTool.tsx:329-339）：
```ts
if (toolUseContext.options.querySource === `agent:builtin:${FORK_AGENT.agentType}` || isInForkChild(toolUseContext.messages)) {
  throw new Error('Fork is not available inside a forked worker. Complete your task directly using your tools.');
}
```

### 5.3 内置 Agent 与 prompt 设计

`/usr/local/LsmGitOpenSource/claudecode/src/tools/AgentTool/builtInAgents.ts:46-72`：

```ts
export function getBuiltInAgents(): AgentDefinition[] {
  if (
    isEnvTruthy(process.env.CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS) &&
    getIsNonInteractiveSession()
  ) return []

  if (feature('COORDINATOR_MODE')) {
    if (isEnvTruthy(process.env.CLAUDE_CODE_COORDINATOR_MODE)) {
      const { getCoordinatorAgents } = require('../../coordinator/workerAgent.js')
      return getCoordinatorAgents()
    }
  }

  const agents: AgentDefinition[] = [GENERAL_PURPOSE_AGENT, STATUSLINE_SETUP_AGENT]

  if (areExplorePlanAgentsEnabled()) {
    agents.push(EXPLORE_AGENT, PLAN_AGENT)
  }
  ...
}
```

**6 个内置 agent**（来自 `built-in/` 目录）：
- `GENERAL_PURPOSE_AGENT` —— 默认 agent
- `STATUSLINE_SETUP_AGENT` —— 配置 statusline
- `EXPLORE_AGENT` —— **read-only**，强 prompt 禁止文件修改；system prompt 鼓励**并行 tool calls**：
  > "Wherever possible you should try to spawn multiple parallel tool calls for grepping and reading files"
- `PLAN_AGENT` —— 计划模式
- `CLAUDE_CODE_GUIDE_AGENT` —— 用户引导
- `VERIFICATION_AGENT` —— 验证（feature flag 控制）

**EXPLORE_AGENT 的关键 prompt 段**（built-in/exploreAgent.ts:11-44）：
```
=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===
This is a READ-ONLY exploration task. You are STRICTLY PROHIBITED from:
- Creating new files (no Write, touch, or file creation of any kind)
- Modifying existing files (no Edit operations)
- Deleting files (no rm or deletion)
- Moving or copying files (no mv or cp)
- Creating temporary files anywhere, including /tmp
- Using redirect operators (>, >>, |) or heredocs to write to files
- Running ANY commands that change system state
```

**并行工具调用的强制引导**（built-in/exploreAgent.ts:54-57）：
> "Make efficient use of the tools that you have at your disposal: be smart about how you search for files and implementations"
> "Wherever possible you should try to spawn multiple parallel tool calls for grepping and reading files"

### 5.4 异步（run_in_background）的工具白名单

`/usr/local/LsmGitOpenSource/claudecode/src/constants/tools.ts:55-72`：

```ts
export const ASYNC_AGENT_ALLOWED_TOOLS = new Set([
  FILE_READ_TOOL_NAME,
  WEB_SEARCH_TOOL_NAME,
  TODO_WRITE_TOOL_NAME,
  GREP_TOOL_NAME,
  WEB_FETCH_TOOL_NAME,
  GLOB_TOOL_NAME,
  ...SHELL_TOOL_NAMES,
  FILE_EDIT_TOOL_NAME,
  FILE_WRITE_TOOL_NAME,
  NOTEBOOK_EDIT_TOOL_NAME,
  SKILL_TOOL_NAME,
  SYNTHETIC_OUTPUT_TOOL_NAME,
  TOOL_SEARCH_TOOL_NAME,
  ENTER_WORKTREE_TOOL_NAME,
  EXIT_WORKTREE_TOOL_NAME,
])
```

`agentToolUtils.ts:73-78, 100`：

```ts
export function filterToolsForAgent({ tools, isBuiltIn, isAsync = false, permissionMode }: {...}): Tools {
  return tools.filter((tool) => {
    if (tool.name.startsWith('mcp__')) return true
    if (toolMatchesName(tool, EXIT_PLAN_MODE_V2_TOOL_NAME) && permissionMode === 'plan') return true
    if (isAsync && !ASYNC_AGENT_ALLOWED_TOOLS.has(tool.name)) return false
    ...
  })
}
```

### 5.5 Teammate (Swarm) 多 agent 模式

`AgentTool.tsx:283-313`：

```ts
if (teamName && name) {
  if (agentDef?.color) setAgentColor(subagent_type!, agentDef.color)
  const result = await spawnTeammate({
    name, prompt, description, team_name: teamName,
    use_splitpane: true,
    plan_mode_required: spawnMode === 'plan',
    model: model ?? agentDef?.model,
    agent_type: subagent_type,
    invokingRequestId: assistantMessage?.requestId
  }, toolUseContext)
  ...
}
```

**In-process teammate 限制**（AgentTool.tsx:278-279）：
```ts
if (isInProcessTeammate() && teamName && run_in_background === true) {
  throw new Error('In-process teammates cannot spawn background agents. Use run_in_background=false for synchronous subagents.')
}
```

**Teammate 专属工具白名单**（constants/tools.ts:73-89）：
```ts
export const IN_PROCESS_TEAMMATE_ALLOWED_TOOLS = new Set([
  TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME, TASK_UPDATE_TOOL_NAME,
  SEND_MESSAGE_TOOL_NAME,
  ...(feature('AGENT_TRIGGERS') ? [CRON_CREATE_TOOL_NAME, CRON_DELETE_TOOL_NAME, CRON_LIST_TOOL_NAME] : []),
])
```

### 5.6 Remote 跨进程委派（CCR）

`AgentTool.tsx:454-478`：

```ts
if ("external" === 'ant' && effectiveIsolation === 'remote') {
  const eligibility = await checkRemoteAgentEligibility()
  if (!eligibility.eligible) {
    const reasons = eligibility.errors.map(formatPreconditionError).join('\n')
    throw new Error(`Cannot launch remote agent:\n${reasons}`)
  }
  let bundleFailHint: string | undefined
  const session = await teleportToRemote({
    initialMessage: prompt, description,
    signal: toolUseContext.abortController.signal,
    onBundleFail: (msg) => { bundleFailHint = msg }
  })
  if (!session) throw new Error(bundleFailHint ?? 'Failed to create remote session')
  const { taskId, sessionId } = registerRemoteAgentTask({...})
  ...
}
```

### 5.7 上下文传递：Fork 继承 vs 新鲜 Agent

**Fork 路径**（AgentTool.tsx:508-524）：
```ts
// Fork path: child inherits the PARENT's system prompt (not FORK_AGENT's)
// for cache-identical API request prefixes. Prompt messages are built via
// buildForkedMessages() which clones the parent's full assistant message
// (all tool_use blocks) + placeholder tool_results + per-child directive.
let enhancedSystemPrompt: string[] | undefined
let forkParentSystemPrompt: ReturnType<typeof buildEffectiveSystemPrompt> | undefined
let promptMessages: MessageType[]
if (isForkPath) {
  if (toolUseContext.renderedSystemPrompt) {
    forkParentSystemPrompt = toolUseContext.renderedSystemPrompt
  } else {
    // Fallback: recompute. May diverge from parent's cached bytes if
    // GrowthBook state changed between parent turn-start and fork spawn.
  }
}
```

**Fresh Agent 路径**：子 agent 收到 `prompt` 这一个字符串，**没有对话历史**——注释（prompt.ts:103-105）：
> "When spawning a fresh agent (with a `subagent_type`), it starts with zero context. Brief the agent like a smart colleague who just walked into the room"

### 5.8 持久化：scratchpad + SendMessage

claudecode 用文件系统作为通信介质（coordinator mode 下的 scratchpad，见 `coordinatorMode.ts:14-22` 注释）：

> "// Checks the same gate as isScratchpadEnabled() in utils/permissions/filesystem.ts. Duplicated here because importing filesystem.ts creates a circular dependency"

**SendMessage tool** 在 Teammate 模式下用于跨 agent 通信（`src/tools/SendMessageTool/`），tool allowlist 中专属于 in-process teammate（constants/tools.ts:78）。

### 5.9 claudecode 最有特色的 3 个设计点

1. **Fork-in-fork 硬性禁止 + system prompt 字节级 cache 复用**：Fork 子 agent 继承**父 agent 的 system prompt 字节**（而非 `FORK_AGENT` 自己的 prompt），保证 API request prefix byte-identical（AgentTool.tsx:512-520 注释）。同时通过 `isInForkChild(toolUseContext.messages)` 二次 message-scan fallback 检测 fork 递归。这种"cache 复用 vs 防递归"的二元设计是其它项目没有的细节。

2. **EXPLORE_AGENT prompt 强制并行 tool calls**：read-only agent 的 system prompt 直接告诉模型 "you should try to spawn multiple parallel tool calls for grepping and reading files"（built-in/exploreAgent.ts:54-57）——这是把并发模型放在**系统提示词层**而非**编排层**实现的最简洁例子。

3. **5 层 SubAgent + ASYNC_AGENT_ALLOWED_TOOLS 白名单**：每个 subagent 形态有独立的工具白名单（`ASYNC_AGENT_ALLOWED_TOOLS`、`IN_PROCESS_TEAMMATE_ALLOWED_TOOLS`），权限收口在 `filterToolsForAgent`（agentToolUtils.ts:73-78）——同时约束 process 模型（in-process / tmux / remote CCR）+ 工具集 + lifecycle（async 不能 spawn background）。

---

## 6. deepseek-harness 的 SubAgent 调度

> 本节基于探索代理 "调研 deepseek-harness SubAgent" 输出。

### 6.1 11 个包一句话职责总览

| 包 | 路径 | 职责 |
|---|---|---|
| `subagent` | `packages/subagent/subagent/` | seam 核心：`SubagentRuntime` 注册表 + provider 协议 + 一次性 run + continuable 子 agent 管理器 |
| `subagent-acp` | `.../subagent-acp/` | 通过 ACP 协议 spawn 外部子 agent 进程 |
| `subagent-claude-code` | `.../subagent-claude-code/` | 调官方 Claude Code CLI 子进程 |
| `subagent-codex` | `.../subagent-codex/` | 调官方 Codex `app-server --stdio` 子进程 |
| `subagent-dsh-sdk` | `.../subagent-dsh-sdk/` | TypeScript SDK stdio JSON-RPC 启动独立 Harness runtime 实例 |
| `subagent-fork-in-process` | `.../subagent-fork-in-process/` | 同进程 fork：子 agent 继承父 agent 完成回合前缀 |
| `subagent-in-process-driver` | `.../subagent-in-process-driver/` | 同进程一次性 run 的共享驱动 + structured_output 工具 |
| `subagent-spawn-in-process` | `.../subagent-spawn-in-process/` | 同进程 spawn：子 agent 不带父上下文（全新 session） |
| `tool-subagent` | `.../tool-subagent/` | 模型可见的 `subagent` 工具：声明 provider / 子 agent 选项 / 后台策略 |
| `tool-subagent-control` | `.../tool-subagent-control/` | 全局 `send_message` 和 `interrupt_agent` 工具（follow-up 与中断） |
| `tool-subagent-report` | `.../tool-subagent-report/` | continuable 内部作用域的 `report` 工具（子 agent 上报父） |

`goal-round-driver`：`/usr/local/LsmGitOpenSource/deepseek-harness/packages/goal/goal-round-driver/` —— 同会话"目标轮次"驱动器，自动在 idle 时驱动下一轮 goal prompt。

### 6.2 SubAgent 抽象：Provider + Capability

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/subagent/subagent/src/types.ts:300-346`：

```ts
export interface SubagentProvider {
  readonly name: string
  readonly capabilities: SubagentCapabilities
  readonly inheritsParentContext: boolean
  readonly agentRouteDefaults?: Readonly<{ provider: string; model: string }>
  start(request: ResolvedSubagentStartRequest): Promise<SubagentRun>
  prepareContinuable?(request: ContinuableCreateRequest): Promise<ContinuableCreateSpec>
}

export interface SubagentCapabilities {
  readonly agentOptions: boolean
  readonly outputSchema: boolean
  readonly depthLimit: boolean
  readonly toolFilter: boolean
  readonly persona: boolean
}
```

**一次性 Run**（types.ts:264-290）：

```ts
export interface SubagentRun {
  readonly id: SessionId
  readonly localAgent: Agent | undefined
  readonly result: Promise<SubagentResult>
  dispose(): Promise<void>
}
```

**结果词汇**（types.ts:208-253）：`SubagentStopReasonMap = { completed, aborted, error, 'max-tokens', refusal }`

### 6.3 6 种传输矩阵

| 名称 | 包 | 实现文件 | 进程模型 | `inheritsParentContext` | 能力 |
|---|---|---|---|---|---|
| `spawn` | `subagent-spawn-in-process` | `.../src/index.ts:41-66` | in-process 子 Agent，无 seed | `false` | 全部 5 项 |
| `fork` | `subagent-fork-in-process` | `.../src/index.ts:62-97` | in-process 子 Agent，seed = 父 log 已完成回合前缀 | `true` | 全部 5 项 |
| `acp` | `subagent-acp` | `.../src/index.ts:146-188` | 外部进程 + ACP NDJSON 协议 | `false` | **NO_START_CAPABILITIES** |
| `claude-code` | `subagent-claude-code` | `.../src/index.ts:73-126` | 外部 Claude Code CLI | `false` | **NO_START_CAPABILITIES** |
| `codex` | `subagent-codex` | `.../src/index.ts:63-110` | 外部 Codex app-server | `false` | **NO_START_CAPABILITIES** |
| `dsh-sdk` | `subagent-dsh-sdk` | `.../src/index.ts:134-176` | 独立 Harness runtime (SDK stdio) | `false` | `agentOptions: true` |

**无 capability 的统一常量**（`out-of-process.ts:57-63`）：

```ts
export const NO_START_CAPABILITIES: SubagentCapabilities = Object.freeze({
  agentOptions: false,
  outputSchema: false,
  depthLimit: false,
  toolFilter: false,
  persona: false,
})
```

**`subagent-dsh-sdk` 例外**（`.../src/index.ts:110-113`）：

```ts
const SDK_START_CAPABILITIES: SubagentCapabilities = Object.freeze({
  ...NO_START_CAPABILITIES,
  agentOptions: true,
})
```

**ACP 进程模型细节**（`subagent-acp/src/run.ts:333-606`）：
- `startAcpRun` 用 `ctx.subprocess.spawn` 起子进程（`{stdin:'pipe', stdout:'pipe', stderr:'inherit'}`）
- NDJSON 走 `@agentclientprotocol/sdk`
- teardown 阶梯：`stdin.end → EOF grace (6000ms) → SIGTERM → SIGKILL grace (3000ms)`（`run.ts:192-205`）

**Selection 方式**：每个 provider 通过 `ctx.subagents.registerProvider(provider)` 注册，`tool-subagent` 在 `Config.provider` 处显式声明要委托的 provider 名（`tool-subagent/src/index.ts:322-350` 的 `assertSubagentProviderConfiguration`）。**没有自动 fallback 顺序**。

### 6.4 tool-subagent 工具

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/subagent/tool-subagent/src/index.ts:373-422`：

```ts
parameters: {
  description: { type: 'string', required: true }, // 3-5 词描述
  prompt: { type: 'string', required: true },
  ...modelSelectionEnabled ? { provider, model, reasoning_effort } : {},
  ...backgroundEnabled ? { run_in_background: boolean } : {},
}
```

**输出聚合**（index.ts:423-461）：三种 kind 的 `oneOf`：
- `background`: `{ kind: 'background', jobId }`
- `continuable`: `{ kind: 'continuable', subagentId }`
- `foreground`: `{ kind: 'foreground', runId, output: JsonValue[] }`

**Config**（index.ts:49-104）：
```ts
export interface Config {
  provider: string                       // 注册表上的 provider 名
  toolName?: string // 默认 'subagent'
  modelSelectionSettings?: boolean
  enableRunInBackground?: boolean        // 默认 true
  backgroundMode?: 'one-shot' | 'continuable'   // 默认 'one-shot'
  agentOptions?: AgentOptions
  persona?: string
  toolFilter?: { allow?: string[]; deny?: string[] }
  maxDepth?: number | 'provider-managed' // 默认 3
}
```

**调度决策**（index.ts:465-562）：
```ts
const runSpec = resolveDelegationRun(args, { backgroundEnabled, continuable })
if (runSpec.runInBackground) {
  if (continuable) {
    const started = await runtimeCtx.subagents.startContinuable({ provider: config.provider, label, request, signal })
    return { kind: 'continuable', subagentId: started.childId }
  }
  const id = jobs.start({ kind: 'subagent', label, owner: parent, run: () => { ... } })
  return { kind: 'background', jobId: id }
}
const run = await runtimeCtx.subagents.start(config.provider, { ...request, signal })
return settleForegroundRun(run)
```

`resolveDelegationRun`（index.ts:288-306）：当 `backgroundEnabled` 且 `run_in_background` 未声明时，continuable 默认后台；one-shot 默认前台。`isConcurrencySafe: () => true`（index.ts:464）—— 子 agent 调用声明为并发安全。

### 6.5 tool-subagent-control + tool-subagent-report

**`tool-subagent-control`** 注册两个全局工具：

**`send_message`**（index.ts:26-77）：
```ts
async execute(args, exec) {
  const result = await ctx.subagents.followup(parent, SessionId(subagent_id), content, {
    source: { kind: 'coordinator', form: 'relay', senderSessionId: parent.id },
    signal
  })
  return { messageId: result.messageId }
}
```

**`interrupt_agent`**（index.ts:79-119）：
```ts
async execute(args, exec) {
  await ctx.subagents.interrupt(SessionId(args.agent_id), { kind: 'ancestor', agent: caller })
  return { accepted: true }
}
```

**`tool-subagent-report`** 通过 `ctx.subagents.registerContinuableSetup(childCtx => installReportTool(childCtx, ctx, reportDelivery))`（index.ts:140-141）**仅在 continuable 子 agent 创建时安装**。

Delivery 策略（continuation.ts:102）：
- `'quiet'` 用 `parent.inject(message)`（不唤醒）
- `'next-step'`（默认）通过 `sendWaking` 在 idle 时 `followup`、busy 时 `steer`（continuation.ts:677-682 + 1473-1515）

### 6.6 goal-round-driver 状态机

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/goal/goal-round-driver/src/index.ts`：

```ts
interface RoundAttempt extends RoundIdentity {
  readonly messageId: MessageId
  readonly content: ContentBlock[]
  phase: 'queued' | 'claimed' | 'admitted'
  cancelled: boolean
  stale: boolean
}
```

阶段转换（index.ts:289-326）：
- `queued`：`agent.followup(message)` 已发出，尚未 claim
- `claimed`：监听到 `agent/inbox/claimed` 且内容匹配（index.ts:295-297）
- `admitted`：监听 `session/event` 收到 `user/message`（index.ts:312-315）
- `stale`：被 `goal/changed`（revision 变化）或 `agent/inbox/inserted` 顶替
- `cancelled`：被 `agent/inbox/discarded` 或 turn abort

**退出条件**：
- max tokens → `disarm`（index.ts:319）
- 取消 → `pause`（index.ts:269）
- cap 耗尽 → block with `round-limit`（index.ts:166-171）
- agent/error 或 pre-step 抛错 → `disarm`

### 6.7 并发控制：ChildLock

**一次性 run**：每个 `startInProcessRun` 创建独立 `SessionId(randomUUID())`，是 `context::agents.create()` 的标准用法（`subagent-in-process-driver/src/index.ts:111, 132`）。

**Continuable child**：`SubagentContinuationManager.activations: Map<SessionId, Activation>`（`continuation.ts:358`），每个 entry 是独立的 `AgentHandle`。

**`ChildLock` 串行化 per-child**（continuation.ts:327-348）：

```ts
class ChildLock {
  private tails = new Map<SessionId, Promise<unknown>>()
  run<T>(childId: SessionId, operation: () => Promise<T>): Promise<T> { ... }
}
```

每个 child 的 delivery / release / disposal 被串行化：通过 `this.locks.run(childId, ...)` 串行化 `startContinuable`、`followup`、`coldResume`、`watchSettlement`、`disposeRoots` 中"判断后立即处置"的临界区。

**不同 child 之间不串行化**——`activations` 是 `Map<SessionId, Activation>`，不同 child 独立处理。

### 6.8 上下文传递

**一次性**：
- `spawn`：无 seed，子 agent 仅看到 `prompt` + 自己的初始 `subagent/descriptor`
- `fork`：`seed = parent.session.events.slice(0, lastTurnEnd.seq + 1)`（`subagent-fork-in-process/src/index.ts:48-54`），已完成的回合前缀
- `acp / claude-code / codex / dsh-sdk`：仅传 `prompt`（text blocks），无历史

**Continuable**：
- `ContinuableCreateRequest` 让 provider 提供 `seed?: readonly SessionEvent[]`（types.ts:193-200）
- `fork`：`prepareContinuable` 一次性捕获父 log 的已完成回合前缀（`subagent-fork-in-process/src/index.ts:90-96`），写到子 log 里——之后 cold resume 直接从子 log 读取

### 6.9 权限继承：DelegatedPolicyOverrides

`/usr/local/LsmGitOpenSource/deepseek-harness/packages/subagent/subagent/src/child-agent.ts:213-262`：

```ts
export function captureDelegatedPolicyOverrides(parent: Agent): DelegatedPolicyOverrides {
  return {
    sandboxMode: parent.ctx.get('sandboxPolicy')?.overrideOf(parent.session),
    approvalPolicy: parent.ctx.get('approval') === undefined ? undefined : 'never',
  }
}
export function appendDelegatedPolicyOverrides(childSession, overrides) {
  if (overrides.sandboxMode !== undefined) childSession.append('sandbox/mode', { mode: overrides.sandboxMode, source: 'delegation' })
  if (overrides.approvalPolicy !== undefined) childSession.append('approval/policy', { policy: overrides.approvalPolicy, source: 'delegation' })
}
```

- **沙盒**：继承父 session 的明确 override（不读 deployment 默认）
- **审批**：固定 `'never'`——子 agent 的 ask 必然被拒
- 这些以 `source: 'delegation'` 写入子 log，append 在 fork seed 之后，保证新鲜策略胜过 stale seed

**子 agent 系统 prompt 注入**（child-agent.ts:171-176）：

```ts
export const SUBAGENT_DELEGATION_CONTEXT =
  'You are a delegated subagent: your permission scope was fixed when you were started and cannot be '
  + 'widened from inside this session ...'
```

### 6.10 失败处理：永不 reject + 错误降级

**一次性传输层错误 `settleRunResult`**（`out-of-process.ts:192-219`）—— **结果永不 reject**：

```ts
try { return parts.cancelled() ? { output: collectOutput, stopReason: 'aborted' } : normalizeSubagentDiagnostic(result) }
catch (error) {
  if (parts.cancelled()) return { output: collectOutput, stopReason: 'aborted' }
  parts.onError?.(toError(error), 'error')
  return { output: collectOutput, ...(diagnostic ? { diagnostic } : null), stopReason: 'error' }
}
```

→任何 post-publication 错误被压成 `stopReason: 'error'`，并通过 sink 上报。`SubagentResult.diagnostic` 受 4096 字节限制（`out-of-process.ts:19-49`）。

**ACP 子进程崩溃 / 协议断连**（`subagent-acp/src/run.ts:277-294`）：
- 启动失败：`startAcpRun`（run.ts:497-540）用 `Promise.race([...initializeAndNewSession, spawnFailed, cancelSettled])`
- 失败分类 stage × category：`{initialize, new-session, prompt, process, teardown} × {protocol, configuration, transport, process-start, process-exit, remote-limit, unknown}`
- 提示过程中的协议关闭：`observeProcessOutcome` 用 `Promise.race([processDone, aborted])` + `AbortSignal.timeout(disposeGraceMs)`（run.ts:378-397）

### 6.11 持久化：descriptor + cold resume

**描述符持久化**（`subagent/descriptor.ts:48-86`）：

```ts
export interface SubagentDescriptorBase {
  version: 3                                // SUBAGENT_DESCRIPTOR_VERSION = 3
  mode: 'one-shot' | 'continuable'
  provider: string
  label?: string
  // continuable 还包括：
  agentProvider?: string
  agentModel?: string
  agentReasoningEffort?: string
  persona?: string
  toolFilter?: { allow?: string[]; deny?: string[] }
}
```

存进 `subagent/descriptor` session event（descriptor.ts:38-40）。`foldSubagentDescriptor(events)` 取首个 `subagent/descriptor` event（descriptor.ts:317-323）。

**Cold resume**（continuation.ts:949-1005）：

```ts
private async coldResume(parent, childId, content, options): Promise<MessageId> {
  const query = this.requireSessionQuery()
  const observation = await query.observeSession(childId, { signal })
  const descriptor = foldSubagentDescriptor(source.events.slice(source.header.seedLength ?? 0))
  if (descriptor === undefined || descriptor.mode !== 'continuable')
    throw new SubagentError('subagent cannot be resumed', 'NOT_RESUMABLE')
  const activation = await this.materialize({...})
  return this.submitMaterialized(activation, content, options.source, parent, options.signal)
}
```

严格校验 `descriptor.mode === 'continuable'` 才能 resume；`seedLength` 之后的 events 才是 subagent 自己的 transcript。

### 6.12 中断与取消

**一次性**：`request.signal.aborted` 注册后，in-process 走 `child.cancel({ kind: 'parent' })` + `settleRunResult` 把 stop reason 写成 `'aborted'`；ACP 走 `flags.cancelled = true` + stdin EOF + `disposeAcpChild`（EOF grace → SIGTERM → SIGKILL）。

**Continuable 跨传输传播**：
- 浏览器侧 → service：`@Remote('interruptByParent')` 把 `parentSessionId` 当作 `user` 权限（`subagent/src/index.ts:478-498`）
- service → manager：`SubagentRuntime.interrupt(target, authority)` → `continuations?.interrupt(target, authority)`
- manager → Agent：`activation.handle.agent.cancel({ kind: 'parent' | 'user' }, { keepInbox: true })`（continuation.ts:594-597）

**`keepInbox: true`** 意味着已 queued 的 inbox 不被丢弃；中断后，子 agent 继续处于 resident，被中断的回合已 claim 的工作不会重新入队；下次 send 唤醒同一 Agent。

### 6.13 嵌套 SubAgent

**允许**（`tool-subagent` 默认 `maxDepth: 3`）：

```ts
return Math.max(agent.session.header.delegationDepth ?? 0, runtime ?? 0)  // depth.ts:34-35
```

```ts
export function resolveChildDepth(parent, maxDepth) {
  const childDepth = delegationDepthOf(parent) + 1
  if (!Number.isSafeInteger(childDepth)) throw new RangeError('subagent child depth exceeds the safe-integer range')
  if (maxDepth !== undefined && childDepth > maxDepth) throw new SubagentDepthError(childDepth, maxDepth)
  return childDepth
}
```

`subagentDepth` 持久化在 agent header → 沿父链 +1 → 子深 = 父深 + 1。`acquireOwnership(parent, childId)` 把 child 加入父 activation 的 `ownedChildren` → 父不能在 child 还 live 时 settle → 构成"等待链"。

### 6.14 deepseek-harness 最有特色的 3 个设计点

1. **Provider 能力严格声明 + 启动前校验**：每个 provider 显式声明 `SubagentCapabilities`。seam 在调 `start()` **之前** 用 `assertCapabilities` 拒绝任何请求方能力不被支持的调用（`subagent/src/index.ts:619-635`）。跨进程 provider 全部冻结为 `NO_START_CAPABILITIES`——跨进程无法强制 start-time 限制，所以必须 fail-loud 而不是 silent degradation。

2. **Activation 模型 + child-first dispose + cold resume 自动重建**：continuable subagent 有"持久身份 + 进程局部 Activation"两层——`SessionId` 持久存在，`Activation`（包含 `AgentHandle`）只在 resident 时存在。每次 `followup` / cold-resume 都走同一个 `locks.run(childId, ...)` 临界区 + `acquireOwnership` + `materialize`，把"send 与 dispose 的 race"、"续命 vs 上报"、"中断 vs 唤醒"统一在同一个 channel 上。

3. **Out-of-process + 永不 reject + 错误降级成 stop reason + 4096 字节脱敏 diagnostic**：所有跨进程 provider 共用同一套 `settleRunResult` + `subprocessRunHandle`——`result` 永不 reject，所有 child-level 失败都收敛成 `{ stopReason: 'error', diagnostic }`，diagnostic 受 4096 字节限制。

---

## 7. openclaw 的 SubAgent 调度

> 本节基于探索代理 "调研 openclaw SubAgent 调度" 输出。

### 7.1 重要前提更正

在进入细节前，必须指出任务描述中的三处与代码实际不符的地方：

1. **swarm 调度器的 placement 不是三态**。`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/swarm/swarm-scheduler.ts:28-32` 只有两态：
   ```ts
   const runLocations = new Map<
     string,
     | { lane: SwarmGroupLane; state: "active"; item?: QueuedSwarmRun }
     | { lane: SwarmGroupLane; state: "queued"; item: QueuedSwarmRun }
   >();
   ```
   `draining` / `reconciling` 在 lane 维度根本未建模。`draining` 仅出现在 `subagent-registry-lifecycle-cleanup.ts:368` 与 `subagent-control-kill-runtime.ts:223`，与"lane placement"无关——属于"控制权回收"语义。

2. **`acp-spawn-heartbeat.ts` 不是心跳协议**。它是"子 agent 是否有资格在 requester 上下文中转发 heartbeat 给用户"的判定函数（`subagent-spawn/acp-spawn-heartbeat.ts:15-49` `isHeartbeatEnabledForSessionAgent`）。真正的陈旧/存活判定在 `registry/subagent-run-liveness.ts`。

3. **最大嵌套是 1 层**（非多深度）：`DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH = 1`（`/usr/local/LsmGitOpenSource/openclaw/src/config/agent-limits.ts:30`）。按 `subagent-capabilities.ts:182-185`：
   ```ts
   if (depth <= 0) return "main";
   return depth < maxSpawnDepth ? "orchestrator" : "leaf";
   ```
   即默认 `main → orchestrator(1) → leaf(1)`，**只允许一层子节点**（leaf 不能再 spawn）。

### 7.2 Spawn 请求类型与契约

`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/spawn/subagent-spawn-contract.ts:9-40`：

```ts
export type SpawnSubagentParams = {
  task: string;
  label?: string;
  agentId?: string;
  model?: string;
  taskName?: string;
  thinking?: string;
  fastMode?: FastMode;
  collect?: boolean;
  outputSchema?: Record<string, unknown>;
  groupId?: string;
  swarmLaunchReplayKey?: string;
  swarmLaunchRequestFingerprint?: string;
  cwd?: string;
  runTimeoutSeconds?: number;
  thread?: boolean;
  mode?: SpawnSubagentMode;          // "run" | "session"
  cleanup?: "delete" | "keep";
  sandbox?: SpawnSubagentSandboxMode; // "inherit" | "require"
  context?: SpawnSubagentContextMode; // "isolated" | "fork"
  lightContext?: boolean;
  expectsCompletionMessage?: boolean;
  attachments?: Array<{ name: string; content: string; encoding?: "utf8" | "base64"; mimeType?: string }>;
  attachMountPath?: string;
};
```

**模式枚举**（`subagent-spawn.types.ts:4-13`）：

```ts
export const SUBAGENT_SPAWN_MODES = ["run", "session"] as const;
export const SUBAGENT_SPAWN_SANDBOX_MODES = ["inherit", "require"] as const;
export const SUBAGENT_SPAWN_CONTEXT_MODES = ["isolated", "fork"] as const;
```

**SubagentLaunchAuthorization**（`subagent-launch-authorization.ts:1-23`）—— 强类型"启动凭证"：

```ts
export type SubagentLaunchAuthorization = {
  modelOverride: { provider?: string; model: string };
};
export function applySubagentLaunchAuthorization(
  request: Record<string, unknown>,
  authorization?: SubagentLaunchAuthorization,
): Record<string, unknown> {
  // 仅注入 planning 阶段授权的 model/provider，绝不重写其他字段
}
```

### 7.3 能力协商

`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/spawn/subagent-capabilities.ts:33-204`：

```ts
export type SubagentSessionRole = "main" | "orchestrator" | "leaf";
type SubagentControlScope = "children" | "none";

function resolveSubagentRoleForDepth(params: { depth: number; maxSpawnDepth?: number }): SubagentSessionRole {
  const depth = resolveNonNegativeIntegerOption(params.depth, 0);
  const maxSpawnDepth = resolveIntegerOption(params.maxSpawnDepth, DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH, { min: 1 });
  if (depth <= 0) return "main";
  return depth < maxSpawnDepth ? "orchestrator" : "leaf";
}
function resolveSubagentControlScopeForRole(role): SubagentControlScope {
  return role === "leaf" ? "none" : "children";
}

export function resolveSubagentCapabilities({ depth, maxSpawnDepth }) {
  return {
    depth, role, controlScope,
    canSpawn: role === "main" || role === "orchestrator",   // leaf 禁止 spawn
    canControlChildren: controlScope === "children",
  };
}
```

`subagentRole` / `subagentControlScope` 持久化在 session-store envelope（`SessionEntry`）中，重启后通过 `resolveStoredSubagentCapabilities`（line 357-410）从 `entry.subagentRole` / `entry.spawnDepth` / `entry.spawnedBy` 恢复。

### 7.4 spawnSubagentDirect 的 9 阶段执行

`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/spawn/subagent-spawn.ts:88-715` —— 源码没有把"9 步"显式注释为步骤，但 `try { … } finally` 块内部可拆出 9 个可识别的阶段：

| # | 行号 | 阶段 | 关键函数 / 字段 | 做什么 |
|---|---|---|---|---|
| 1 | 100-110 | 解析请求 + 鉴权 | `resolveSubagentSpawnRequest` | 校验 taskName、agentId、mode 与 thread 组合、swarm 参数、outputSchema；`reserveChildAdmissionSlot` 预占并发槽 |
| 2 | 145-172 | 构造 child plan | `resolveSubagentChildPlan` | 计算 spawnedCwd、workspace、sandbox 模式、creationPolicy、targetAgentDir、resolvedModel、launchAuthorization |
| 3 | 175-202 | 创建 child session | `createInitialSubagentSession` | 落 `SessionEntry`，设 spawnDepth、attachedModelPatch、inheritedToolAllow/Deny |
| 4 | 213-253 | 准备上下文 + 持久化 model | `prepareSubagentSessionContext` + `persistInitialChildSessionRuntimeModel` | 上下文引擎回填；将 resolvedModel 写入 session 存储 |
| 5 | 254-281 | Thread 绑定（可选） | `bindThreadForSubagentSpawn` | 若 `params.thread === true`，申请 channel 下的 conversation / thread；返回 deliveryOrigin |
| 6 | 282-340 | System prompt envelope + 附件物化 | `buildSubagentSpawnEnvelope` + `materializeSubagentAttachments` | 拼接子 agent 角色/规则 prompt（"## Subagent Context"…），注入 structured-output 指令 |
| 7 | 343-379 | 构造子 launch 请求 + 发出 session 事件 | `buildSubagentLaunchRequest` | 把 task、message、spawnedByKey、outputSchema、timeout、launchAuthorization、swarm group key 打包；`recordSessionCreated` + `recordSubagentSpawned` 广播 |
| 8 | 380-470 | 实际启动子 agent（initialize → dispatch） | `adapter.initialize()` + `adapter.dispatchTurn()` → `callNativeSubagentGateway` | Gateway 接受后写入 `acceptedChildRunId`、`recordSessionParticipantBestEffort` |
| 9 | 527-709 | 注册到 registry + 激活 swarm + 收尾 | `runSpawnPipeline`（line 527-581）→ `registerSubagentRun`；swarm 路径走 `activateSwarmRun`（line 602-679）；非 swarm 走 `emitSpawnLifecycleHooks`（line 683） | `finally` 块 line 710-715 释放 admission reservation 与 swarm reservation |

#### `try/finally` 关键片段（subagent-spawn.ts:139-143 + 710-715）

```ts
let modelApplied = false;
let threadBindingReady = false;
let hasBoundThreadDeliveryOrigin = false;
let childRunId: string = childIdem;
let swarmReservationPending = reservationPending;
try {
  // … 9 阶段 …
} finally {
  admissionReservation?.release();
  if (swarmReservationPending) {
    removeQueuedSwarmRun(childRunId);   // 失败时回滚 swarm 队列预占
  }
}
```

#### Pipeline 三阶段（`spawn-pipeline.ts:63-116`）

`spawnSubagentDirect` 把步骤 8-9 抽象成 `SpawnBackendAdapter<TState>`：

```ts
// /usr/local/LsmGitOpenSource/openclaw/src/agents/spawn-pipeline.ts:4
type SpawnPipelinePhase = "initialize" | "dispatch" | "register";
```

三段失败各自走 `adapter.cleanupOnFailure({ phase, state, error })`（subagent-spawn.ts:471-525）：
- `phase === "initialize"`：仅清失败会话（`cleanupFailedSpawn()`）
- `phase === "register"` 且 `acceptedChildRunId`：调 `terminateAcceptedCollectorRun` 把已被 gateway 接受的 run 杀掉
- `phase === "dispatch"`：补发 `subagent_ended` 钩子

### 7.5 Swarm 调度器（active / queued 二态）

`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/swarm/swarm-scheduler.ts:19-32`：

```ts
type SwarmGroupLane = {
  groupId: string;
  limit: number;          // = maxConcurrent
  active: Set<string>;    // 正在执行的 runId 集合
  queue: QueuedSwarmRun[]; // FIFO 等待队列
  pumpScheduled: boolean; // 防止重入的 microtask 锁
};

const lanes = new Map<string, SwarmGroupLane>();              // groupId → lane
const runLocations = new Map<runId, { lane, state: "active" | "queued", item }>();
```

**Lane 容量**：
- 默认值 `DEFAULT_SWARM_CONFIG.maxConcurrent = 8`（`swarm-config.ts:17`）
- 上限 `clampNumber(..., 1, 1_000)`（line 60-62）
- `ensureLane` 会用新值更新 `lane.limit` 并在 `activeRunIds` 中补回正在执行的 run（line 114-140）
- "满"判定 `lane.active.size >= lane.limit`（line 76, 46, 194）

**任务分配算法**（FIFO + 容量门）：

`pumpLane(lane)`（line 96-112）：

```ts
while (lanes.get(lane.groupId) === lane && lane.active.size < lane.limit) {
  const next = lane.queue[0];
  if (!next?.launch || !next.retryReady || next.holds > 0) return;
  lane.queue.shift();
  void startQueuedRun(lane, next, next.launch);
}
```

- `holds` 用于"预启动阶段不消耗 slot"（`holdQueuedSwarmRun`，line 280-307）
- `isSwarmRunWaitingForCapacity`（line 186-196）通过 `onCapacityChange` 回调通知 requester
- 失败重试 backoff：`setTimeout(... isFastTestRuntimeEnv() ? 1 : 1_000)`（line 82-91）

### 7.6 两种 spawn 路径

`/usr/local/LsmGitOpenSource/openclaw/src/agents/tools/sessions-spawn-tool.ts:500-700`：

```ts
if (runtime === "acp") {
  const { spawnAcpDirect } = await loadAcpSpawnModule();
  const result = await spawnAcpDirect(...);          // ACP 路径
}
const result = await spawnSubagentDirect(...);        // 进程内 native 路径
```

**进程内 native 路径**：
- 入口：`subagent-spawn.ts:88` `export async function spawnSubagentDirect(params, ctx)`
- 启动方式：`callNativeSubagentGateway(...)` → 进程内 `agent` method（`subagent-spawn-gateway.ts`）

**跨进程 ACP 路径**：
- 入口：`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/spawn/acp-spawn.ts` `spawnAcpDirect`
- 关键差异：
  - `acp-spawn-runtime.ts:155-210` `initializeAcpSpawnRuntime` 走 `getAcpSessionManager().initializeSession(...)`
  - `acp-spawn.ts:556` 把 `admission.maxSpawnDepth` 当 `maxDepth` 透传
  - `acp-spawn-parent-stream.ts:22` `import { requestHeartbeat } from "../../../infra/heartbeat-wake.js"` — 父流通过 gateway heartbeat 拿子 agent 输出
  - `sessions-spawn-tool.ts:521-526` 显式拒绝 `lightContext` 与 `context="fork"`（仅 native 支持）

### 7.7 上下文传递：System Prompt envelope

`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/spawn/subagent-system-prompt.ts:18-136`：

```ts
export function buildSubagentSpawnEnvelope(params: {
  completionMode: SubagentCompletionMode;   // "collector" | "quiet" | "thread-direct" | "announce"
  spawnMode: "run" | "session";
  task: string;
  requesterSessionKey?: string;
  requesterOrigin?: DeliveryContext;
  childSessionKey: string;
  label?: string;
  acpEnabled?: boolean;
  nativeCommandGuidanceLines?: string[];
  childDepth?: number;
  maxSpawnDepth?: number;
}) {
  // ... 拼装 "## Subagent Context / Your Role / Rules / Output Format / What You DON'T Do"
  if (canSpawn) { /* ## Sub-Agent Spawning 段 */ }
  else if (childDepth >= 2) { /* Leaf worker: cannot spawn. */ }
  return { systemPrompt, message, acceptedNote }
}
```

**四种 completionMode 提示**（`subagent-system-prompt.ts:9-16`）：
| mode | 行为 |
|------|------|
| `collector` | "no completion notification is sent. The requester must explicitly collect…" |
| `quiet` | "no completion notification is sent. Do not wait for an announcement." |
| `thread-direct` | "delivered directly to the bound thread, without a separate parent completion notification" |
| `announce` | "returns to the requester as a completion event" |

`subagent-spawn.ts:286-292` 决定 `completionMode`：

```ts
const completionMode = params.collect
  ? "collector"
  : requestThreadBinding && spawnMode === "session" && hasBoundThreadDeliveryOrigin
    ? "thread-direct"
    : expectsCompletionMessage
      ? "announce"
      : "quiet";
```

### 7.8 目标策略

`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/spawn/subagent-target-policy.ts:51-120`：

```ts
export function resolveSubagentAllowedTargetIds(params: {
  requesterAgentId: string;
  allowAgents?: readonly string[];
  configuredAgentIds?: readonly string[];
}): { allowAny: boolean; allowedIds: string[] } {
  // 未配置 allowAgents → 只能 spawn 自己
  // allowAgents 含 "*" → 允许所有 configured 加上自己
  // 否则仅 allowAgents ∩ configuredAgentIds
}
```

### 7.9 失败处理与重试

**子 agent 终态**（`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/subagent-terminal-outcome.ts:8-13`）：

```ts
export function classifySubagentTerminalOutcome(outcome: AgentRunTerminalOutcome) {
  return classification === "timeout" || !isAbortedAgentStopReason(outcome.stopReason)
    ? classification
    : "cancellation";
}
```

原始 `AgentRunTerminalReason`：`completed | hard_timeout | timed_out | superseded | cancelled | aborted | blocked | abandoned | failed`。

**Collector 终态**（`registry.types.ts:104`）：
```ts
export type SwarmCollectorStatus = "done" | "failed" | "killed" | "timeout";
```

**Delivery 状态机**（`registry.types.ts:128-178`）七态：
```
not_required → pending → in_progress → delivered | failed
                                       failed → suspended
                                       → discarded
```

**Execution 状态机**（`registry.types.ts:78-93`）：`queued | running | interrupted | terminal`。

**陈旧判定**（`registry/subagent-run-liveness.ts`）：

```ts
const STALE_UNENDED_SUBAGENT_RUN_MS = 2 * 60 * 60 * 1_000;        // 2 小时
export const RECENT_ENDED_SUBAGENT_CHILD_SESSION_MS = 30 * 60 * 1_000;  // 30 分钟
const EXPLICIT_TIMEOUT_STALE_GRACE_MS = 60_000;
```

**重试循环**（`subagent-spawn-cleanup.ts:21-41`）：

```ts
for (;;) {
  if (await attempt()) return true;
  if (options?.shouldRetry?.() === false) return false;
  await new Promise(r => setTimeout(r, isFastTestRuntimeEnv() ? 1 : 1_000));
}
```

### 7.10 持久化与重启恢复

**注册表**（`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/registry/subagent-registry-memory.ts:74-194`）：

```ts
class SubagentRunMap extends Map<string, SubagentRunRecord> {
  // 同时维护 3 个索引：
  //   runsByChildSessionKey
  //   runsByCollectorGroupKey
  //   collectorRunIdByChildSessionKey
}
```

带 `retirementScopes` 机制追踪"消亡观察窗口"（line 75-146），处理 cancel-vs-new-race 场景。

**SQLite 持久化**（`/usr/local/LsmGitOpenSource/openclaw/src/agents/subagents/registry/subagent-registry.store.sqlite.ts`）：
- `subagent_runs` 表：`run_id`, `child_session_key`, `controller_session_key`, `requester_session_key`, `created_at`, `payload_json`
- 全部业务字段塞进 `payload_json`（用 `safeParseJson` 解析，line 80）

**崩溃后 reconcile**（`subagent-registry-restore.ts:55-542` `createSubagentRegistryRestorer`）：

1. `restoreSubagentRunsOnce`（line 318-359）：从 SQLite 拉行进内存；调用 `reconcileOrphanedRestoredRuns` 标记孤儿
2. `activateRestoredRuns`（line 149-316）：依 entry 状态分派
   - `entry.execution.restartRecovery || entry.killIntent || entry.killReconciliation` → 重启恢复独占
   - `entry.collect && queued` → `enqueueSwarmRun` 重新挂到 swarm lane
   - 否则 → `resumeSubagentRun(runId)` 续上 lifecycle
3. 失败回退：`scheduleRestoreRetry` 指数退避 `1s → 30s` 上限

**`SubagentRestartRecoveryReceipt`**（`registry.types.ts:61-68`）5 态机：
```
reserved → attempted → consumed → accepted | abandoned
```

通过 `lifecycleGeneration` 比对避免用旧 generation 取消新 run。

### 7.11 并发控制

**单父 agent 并发**：由 `child-admission.js`（外部）+ `resolveSpawnAdmission` 控制。`subagent-spawn-request.ts:241-247`：

```ts
const admissionReservation = params.collect
  ? undefined
  : reserveChildAdmissionSlot({
      controllerSessionKey: ownership.controllerSessionKey,
      resolveAdmission,
    });
```

若 `!admission.ok` 直接 `rejectSubagentSpawnRequest("forbidden", admission.error)`（line 248-255）。

**Swarm 多个 lane 并行**：每个 `groupId` 一个 lane，lane 间完全独立：
- `lanes: Map<groupId, SwarmGroupLane>`（swarm-scheduler.ts:27）
- 每个 lane 独立 `active: Set` + `queue: QueuedSwarmRun[]` + `pumpScheduled` 标志
- `ensureLane` 在已存在 lane 上**累加** active（line 129-137）
- `pumpLane`（line 96-112）只 pump 自己这条 lane，跨 lane 无锁
- 全局上限：`maxTotalPerGroup = 200`（`swarm-config.ts:19, 51-55`）+ `maxChildrenPerGroup = 50`（line 18, 46-50）

**公平**：
- 队列是普通 `array`（line 23），用 `queue.push` / `queue.shift` 操作 → 严格 FIFO
- 失败时 `queue.unshift(item)`（line 79）放回头部 + 1s backoff
- `holds > 0`（line 105）跳过 pump

### 7.12 深度限制

**常量**（`/usr/local/LsmGitOpenSource/openclaw/src/config/agent-limits.ts:30`）：
```ts
export const DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH = 1;
```

可通过 `config.agents.defaults.subagents.maxSpawnDepth` 覆盖（`subagent-capabilities.ts:366-368`）。

**检查逻辑**（`subagent-capabilities.ts:172-204`）：

```ts
function resolveSubagentRoleForDepth({ depth, maxSpawnDepth }) {
  const depth = resolveNonNegativeIntegerOption(depth, 0);
  const maxSpawnDepth = resolveIntegerOption(maxSpawnDepth, DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH, { min: 1 });
  if (depth <= 0) return "main";
  return depth < maxSpawnDepth ? "orchestrator" : "leaf";
}
```

`subagent-depth.ts:138-198` `getSubagentDepthFromSessionStore` 沿 `entry.spawnedBy` 链回溯，逐层 +1。

子 prompt 通过 `childDepth < maxSpawnDepth` 决定是否注入"## Sub-Agent Spawning"段（`subagent-system-prompt.ts:34, 68-93`）。

### 7.13 openclaw 最有特色的 3 个设计点

1. **"Pre-reserve, then commit" 的并发模型**：`resolveSubagentSpawnRequest` 在做任何 I/O 之前就调 `reserveChildAdmissionSlot`（对单父并发）和 `reserveSwarmRun`（对 swarm FIFO 顺序）双层预占，再用 `spawn-pipeline.ts:53-116` 的三阶段 `initialize → dispatch → register` 把"启动/启动成功/注册"显式分段。每段失败都触发 `adapter.cleanupOnFailure({ phase })`。`launchTerminationConfirmed`（subagent-spawn.ts:601, 656, 666）标志位尤其精彩：launch RPC 失败可能发生在 gateway 已经接受之后，因此 cleanup 阶段阻塞等待"session 被删"以避免误判孤儿。

2. **"Stable host identity + replay-safe snapshot" 的重启恢复**：`SwarmQueuedLaunch`（`registry.types.ts:119-126`）把 `request` / `authorization` / `timeoutMs` / `schedulerGroupKey` / `maxConcurrent` **整体**持久化在 SQLite，启动时 `subagent-registry-restore.ts:240-302` 用 `enqueueSwarmRun` 重新挂载。`swarmLaunchIdempotencyKey` + `swarmLaunchRequestFingerprint` + `swarmLaunchReplayKey` 三件套让"同一个 host 请求"跨重启不重复 spawn collector。

3. **"Enveloped system prompt + 多重 role" 的契约化上下文传递**：`buildSubagentSpawnEnvelope`（`subagent-system-prompt.ts`）不只注入任务文本，而是按 `completionMode` × `childDepth` × `maxSpawnDepth` × `acpEnabled` × `nativeCommandGuidanceLines` 五个维度拼接 4 段。`SubagentLaunchAuthorization` / `SubagentSessionRole` / `SubagentControlScope` / `inheritedToolAllow/Deny` / `inheritedToolPolicyVersion` 五件套一起持久化在 `SessionEntry` 的 envelope 中。

---

## 8. opencode 的 SubAgent 调度

> 本节基于探索代理 "调研 opencode Agent 并发" 输出。

### 8.1 Agent 类型定义

**Schema 定义** — `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/agent/agent.ts:35-56`：

```ts
export const Info = Schema.Struct({
  name: Schema.String,
  description: Schema.optional(Schema.String),
  mode: Schema.Literals(["subagent", "primary", "all"]),
  native: Schema.optional(Schema.Boolean),
  hidden: Schema.optional(Schema.Boolean),
  topP: Schema.optional(Schema.Finite),
  temperature: Schema.optional(Schema.Finite),
  color: Schema.optional(Schema.String),
  permission: PermissionV1.Ruleset,
  model: Schema.optional(Schema.Struct({ modelID: ModelV2.ID, providerID: ProviderV2.ID })),
  variant: Schema.optional(Schema.String),
  prompt: Schema.optional(Schema.String),
  options: Schema.Record(Schema.String, Schema.Unknown),
  steps: Schema.optional(Schema.Finite),
}).annotate({ identifier: "Agent" })
```

**Permission 规则结构**（`/usr/local/LsmGitOpenSource/opencode/packages/schema/src/v1/permission.ts:16-24`）：

```ts
export const Action = Schema.Literals(["allow", "deny", "ask"])
export const Rule = Schema.Struct({ permission: Schema.String, pattern: Schema.String, action: Action })
export const Ruleset = Schema.Array(Rule)
```

### 8.2 6 个内置 Agent

| 名称 | mode | hidden | 角色 |
|------|------|--------|------|
| **`build`** | `primary` | false | 主对话 agent（默认） |
| **`plan`** | `primary` | false | 只读计划模式（`edit` 全 deny 除 `.opencode/plans/*.md`） |
| **`general`** | `subagent` | false | 多步并行通用（**主要 SubAgent**） |
| **`explore`** | `subagent` | false | 只读探索（`grep/glob/list/bash/webfetch/websearch/read` allow） |
| **`compaction`** | `primary` | true | 上下文压缩（hidden） |
| **`title`** | `primary` | true | 一次性标题生成（temperature=0.5） |
| **`summary`** | `primary` | true | 摘要（hidden） |

**`build`**（agent.ts:141-155）—— 用户默认 agent：
```ts
build: {
  name: "build",
  description: "The default agent. Executes tools based on configured permissions.",
  options: {},
  permission: Permission.merge(defaults, Permission.fromConfig({
    question: "allow",
    plan_enter: "allow",
  }), user),
  mode: "primary",
  native: true,
},
```

**`plan`**（agent.ts:156-181）：
```ts
plan: {
  name: "plan",
  description: "Plan mode. Disallows all edit tools.",
  permission: Permission.merge(defaults, Permission.fromConfig({
    question: "allow",
    plan_exit: "allow",
    task: { general: "deny" },
    external_directory: { [path.join(Global.Path.data, "plans", "*")]: "allow" },
    edit: {
      "*": "deny",
      [path.join(".opencode", "plans", "*.md")]: "allow",
      [path.relative(ctx.worktree, path.join(Global.Path.data, path.join("plans", "*.md")))]: "allow",
    },
  }), user),
  mode: "primary",
  native: true,
},
```

**`general`**（agent.ts:182-196）：
```ts
general: {
  name: "general",
  description: `General-purpose agent for researching complex questions and executing multi-step tasks. Use this agent to execute multiple units of work in parallel.`,
  permission: Permission.merge(defaults, Permission.fromConfig({ todowrite: "deny" }), user),
  options: {},
  mode: "subagent",
  native: true,
},
```
> 关键描述："**Use this agent to execute multiple units of work in parallel.**" —— 子 agent 的并发由模型自行触发多次 task tool 调用驱动

**`explore`**（agent.ts:197-216）：
```ts
explore: {
  name: "explore",
  permission: Permission.merge(defaults, Permission.fromConfig({
    "*": "deny",
    grep: "allow", glob: "allow", list: "allow",
    bash: "allow", webfetch: "allow", websearch: "allow",
    read: "allow",
    external_directory: readonlyExternalDirectory,
  }), user),
  description: `Fast agent specialized for exploring codebases...`,
  prompt: PROMPT_EXPLORE,
  options: {},
  mode: "subagent",
  native: true,
},
```

**`title`**（agent.ts:234-249）：
```ts
title: {
  name: "title",
  mode: "primary", options: {}, native: true, hidden: true, temperature: 0.5,
  permission: Permission.merge(defaults, Permission.fromConfig({ "*": "deny" }), user),
  prompt: PROMPT_TITLE,
}
```

### 8.3 默认 Agent 选择逻辑（agent.ts:328-340）

```ts
const defaultInfo = Effect.fnUntraced(function* () {
  const c = yield* config.get()
  if (c.default_agent) {
    const agent = agents[c.default_agent]
    if (agent.mode === "subagent") throw new Error(`default agent "${c.default_agent}" is a subagent`)
    if (agent.hidden === true) throw new Error(`default agent "${c.default_agent}" is hidden`)
    return agent
  }
  const visible = Object.values(agents).find((a) => a.mode !== "subagent" && a.hidden !== true)
})
```

### 8.4 Subagent Mode 特殊行为

1. **不出现在 default agent 候选**（agent.ts:333-334 显式拒绝）
2. **`list()` 不做特殊过滤**——只能通过 task tool 调用，CLI/UI 不允许直接选择
3. **无 parent agent 权限继承**（`subagent-permissions.ts:21-23`）：
   ```ts
   ...input.parentSessionPermission.filter(
     (rule) => rule.permission === "external_directory" || rule.action === "deny",
   ),
   ```
   > "Parent agent restrictions only govern that agent; the subagent's own permissions determine its capabilities."

### 8.5 TaskTool 实现

**Schema（输入参数）**（`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/tool/task.ts:43-62`）：

```ts
const BaseParameterFields = {
  description: Schema.String.annotate({ description: "A short (3-5 words) description of the task" }),
  prompt: Schema.String.annotate({ description: "The task for the agent to perform" }),
  subagent_type: Schema.String.annotate({ description: "The type of specialized agent to use for this task" }),
  task_id: Schema.optional(Schema.String).annotate({
    description: "This should only be set if you mean to resume a previous task..."
  }),
  command: Schema.optional(Schema.String),
}
export const Parameters = Schema.Struct({ ...BaseParameterFields,
  background: Schema.optional(Schema.Boolean).annotate({...})  // 异步后台
})
```

注意：是 `subagent_type`（不是 `agent`），且支持 `task_id` 复用同一子 session。

**深度检查**（task.ts:104-117）：
```ts
const parent = yield* sessions.get(ctx.sessionID)
let current = parent
let depth = 0
while (current.parentID) {
  depth++
  current = yield* sessions.get(current.parentID)
}
if (depth >= (cfg.subagent_depth ?? 1)) {
  return yield* Effect.fail(
    new Error(`Subagent depth limit reached (${cfg.subagent_depth ?? 1}). Increase "subagent_depth" to allow nested subagents.`),
  )
}
```

**权限 ask 门**（task.ts:119-129）：
```ts
if (!ctx.extra?.bypassAgentCheck) {
  yield* ctx.ask({
    permission: id,  // "task"
    patterns: [params.subagent_type],
    always: ["*"],
    metadata: { description: params.description, subagent_type: params.subagent_type },
  })
}
```

**创建子 session**（task.ts:131-172）：

```ts
const next = yield* agent.get(params.subagent_type)
if (!next) return yield* Effect.fail(new Error(`Unknown agent type: ${params.subagent_type} is not a valid agent type`))

const session = params.task_id
  ? yield* sessions.get(SessionID.make(params.task_id)).pipe(Effect.catchCause(() => Effect.succeed(undefined)))
  : undefined
const childPermission = deriveSubagentSessionPermission({ parentSessionPermission: parent.permission ?? [], subagent: next })
const childToolDenies = [
  ...(next.permission.some((rule) => rule.permission === "todowrite")
    ? []
    : [{ permission: "todowrite" as const, pattern: "*" as const, action: "deny" as const }]),
  ...(next.permission.some((rule) => rule.permission === id)
    ? []
    : [{ permission: id, pattern: "*" as const, action: "deny" as const }]),
  ...(cfg.experimental?.primary_tools?.map((permission) => ({ permission, pattern: "*" as const, action: "deny" as const })) ?? []),
]
const nextSession =
  session ??
  (yield* sessions.create({
    parentID: ctx.sessionID,
    title: params.description + ` (@${next.name} subagent)`,
    agent: next.name,
    permission: [
      ...childPermission,
      ...childToolDenies.filter((deny) => !childPermission.some((rule) => ...)),
    ],
  }))
```

### 8.6 输出包装

`renderOutput`（task.ts:64-79）：

```ts
function renderOutput(input: { sessionID, state: "running" | "completed" | "error", summary?, text }) {
  const tag = input.state === "error" ? "task_error" : "task_result"
  return [
    `<task id="${input.sessionID}" state="${input.state}">`,
    ...(input.summary ? [`<summary>${input.summary}</summary>`] : []),
    `<${tag}>`,
    input.text,
    `</${tag}>`,
    "</task>",
  ].join("\n")
}
```

返回的内容是**最后一条 text part**（不是 full message history）：

```ts
return result.parts.findLast((item) => item.type === "text")?.text ?? ""
```

### 8.7 Session 状态机

**状态枚举**（`/usr/local/LsmGitOpenSource/opencode/packages/schema/src/session-status-event.ts:9-33`）：

```ts
export const Info = Schema.Union([
  Schema.Struct({ type: Schema.Literal("idle") }),
  Schema.Struct({
    type: Schema.Literal("retry"),
    attempt: NonNegativeInt,
    message: Schema.String,
    action: optional(Schema.Struct({ reason, provider, title, message, label, link })),
    next: NonNegativeInt,
  }),
  Schema.Struct({ type: Schema.Literal("busy") }),
])
```

只有 **3 种状态**：`idle | busy | retry`（permission 是独立事件流）。

**状态服务**（`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/session/status.ts:21-53`）：

```ts
const set = Effect.fn("SessionStatus.set")(function* (sessionID, status) {
  const data = yield* InstanceState.get(state)
  yield* events.publish(Event.Status, { sessionID, status })
  if (status.type === "idle") {
    yield* events.publish(Event.Idle, { sessionID })
    data.delete(sessionID)
    return
  }
  data.set(sessionID, status)
})
```

**Permission 状态**（独立的 `permission.asked` / `permission.replied` 事件 + 内存中 `pending` map）—— `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/permission/index.ts:67-167`：

```ts
const deferred = yield* Deferred.make<void, PermissionV1.RejectedError | PermissionV1.CorrectedError>()
pending.set(id, { info, deferred })
yield* events.publish(Event.Asked, info)
return yield* Effect.ensuring(Deferred.await(deferred), Effect.sync(() => pending.delete(id)))
```

通过 `Deferred` 阻塞 tool 执行直到用户 reply（`once | always | reject`，其中 `reject` 可携带 message 作为 `CorrectedError` 反馈给 LLM）。

### 8.8 并发执行

**工具级并行**：opencode 工具**在单轮内串行执行**，但**多轮 LLM 调用间是串行的**。**支持在同一 message 中并行发起多个 tool call**——AI SDK 把 tool call 放进 stream；processor 在 cleanup 阶段统一 `await` 所有 in-flight tool calls：

`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/session/processor.ts:585-589`：

```ts
yield* Effect.forEach(
  Object.values(ctx.toolcalls),
  (call) => Deferred.await(call.done).pipe(Effect.timeout("250 millis"), Effect.ignore),
  { concurrency: "unbounded" },
)
```

**Task 工具级并发**：通过 `background: true` 触发 `BackgroundJob.start` 异步执行（`task.ts:284-319`）。**没有针对 task 的全局并行池**，靠用户在同一条 assistant message 中发起多个 task tool call。

**Session 级并发**（多个会话并行）：每个 session 通过 `Runner.ensureRunning` 串行化自己的 run（`run-state.ts:88-94`），多个 session 之间完全独立，可并行。

### 8.9 权限继承：`deriveSubagentSessionPermission`

`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/agent/subagent-permissions.ts:14-27`：

```ts
export function deriveSubagentSessionPermission(input: {
  parentSessionPermission: PermissionV1.Ruleset
  subagent: Agent.Info
}): PermissionV1.Ruleset {
  const canTask = input.subagent.permission.some((rule) => rule.permission === "task")
  const canTodo = input.subagent.permission.some((rule) => rule.permission === "todowrite")
  return [
    ...input.parentSessionPermission.filter(
      (rule) => rule.permission === "external_directory" || rule.action === "deny",
    ),
    ...(canTodo ? [] : [{ permission: "todowrite", pattern: "*", action: "deny" }]),
    ...(canTask ? [] : [{ permission: "task", pattern: "*", action: "deny" }]),
  ]
}
```

**子 session 实际权限** = **父 session 的 deny 规则 + external_directory 规则** ∪ **子 agent 自己的规则集** ∪ **task/todowrite 兜底 deny**（subagent 默认不能再调 task/todowrite，除非自己显式 allow）∪ **`experimental.primary_tools` 额外 deny**

### 8.10 持久化（SQLite + Drizzle）

**Session 表 schema**（`/usr/local/LsmGitOpenSource/opencode/packages/core/src/database/schema.gen.ts:182-213`）：

```sql
CREATE TABLE `session` (
  `id` text PRIMARY KEY,
  `project_id` text NOT NULL,
  `workspace_id` text,
  `parent_id` text,                  -- 关键：parent_id 用于 subagent 树
  `slug` text NOT NULL,
  `directory` text NOT NULL,
  ...
  `permission` text,
  `agent` text,
  `model` text,
  `time_created` integer NOT NULL,
  `time_updated` integer NOT NULL,
  `time_compacting` integer,
  `time_archived` integer,
  ...
)
```

**`BackgroundJob` 是进程内、非持久化的**（`background-job.ts:113-119` 注释："Entries are intentionally not durable: process restart or owner-scope closure loses status and interrupts live work."）—— subagent 运行状态不落盘，只持久化 session/message/part 数据。

### 8.11 嵌套深度限制

**`task.ts:104-117` 双重防御**：
1. 配置层 `cfg.subagent_depth ?? 1`（默认 1）
2. 权限兜底：subagent 没显式 allow task → 注入 `task: * deny`

**`/usr/local/LsmGitOpenSource/opencode/packages/core/src/v1/config/config.ts:84-86`**：

```ts
subagent_depth: Schema.optional(NonNegativeInt).annotate({
  description: "Maximum subagent nesting depth. Defaults to 1, which prevents subagents from launching subagents.",
})
```

### 8.12 opencode 最有特色的 3 个设计点

1. **Permission 子规则的三向合并（deny + external_directory + 子 agent own ruleset + 工具兜底 deny）** —— `deriveSubagentSessionPermission` 不是粗暴的"父 allow→子 allow"或"父 deny→子 deny"，而是**只下放 deny 规则和 external_directory 规则**（因为这两个通常涉及安全/项目隔离），子 agent 自己的 allow 决定能力集。

2. **`subagent_depth` + 默认值 = 1 + 兜底 deny `task` 权限**（双重防御）—— 不仅仅是配置层限制，还在每个 task tool 的子 session 权限中**显式注入 `task: * deny`**，意味着即使 `subagent_depth` 配置错误或被提升，subagent 也无法调用 task tool 进一步递归。

3. **Background Job 的 `extend / waitForPromotion / promote` 协议** —— `BackgroundJob.start` 创建带 `Deferred.done/promoted/tail` 三组 latch 的 job，`extend` 让同 session 内多次 follow-up 共享 job id，`waitForPromotion` 让 foreground 等待任务被"提升"为 background 时立即返回。配合 `synthetic: true` 的 text part 注入，把后台事件流无缝融入 LLM context。

---

## 9. pi 的 SubAgent 调度（lane 并发模型）

> 本节基于独立源码阅读。

### 9.1 lane 并发模型核心概念

pi 引入"lane"概念——一个 session 内可以有多条独立运行轨（lane），每条 lane 有：
- 自己的 leaf（最新 entry id）
- 自己的 `open_operation_id`（乐观锁）
- 三种独立的消息队列（steer / followUp / nextRun）
- 自己的 Operation 状态机（started / finished）

**LaneRow**（`/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts:5-10`）：

```ts
export interface LaneRow {
  session_id: string;
  lane: string;
  leaf_id: string | null;
  open_operation_id: string | null;   // 乐观锁字段
}
```

**LaneRecord**（`/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/types.ts:202-208`）：

```ts
export type LaneRecord =
  | OperationStartedRecord
  | AbortRequestedRecord
  | OperationFinishedRecord
  | StepAttemptRecord
  | ToolStartedRecord
  | QueueEnqueuedRecord
  | QueueCancelledRecord
  | WriteDeferredRecord
  | UsageRecord;
```

### 9.2 Operation 状态机（started / finished）

**`OperationStartedRecord`**（types.ts:87-118）：

```ts
export interface OperationStartedRecord extends RecordBase {
  type: "operation_started";
  sourceLeafId: string | null;
  intent:
    | {
        kind: "run";
        originalPrompt: AgentMessage[];
        initialMessages: ProvisionedEntry[];
        systemPromptOverride?: string;
        resumeData?: { [extensionId: string]: JsonValue };
      }
    | {
        kind: "compaction";
        customInstructions?: string;
        resultEntryId: string;
      }
    | {
        kind: "navigation";
        targetId: string | null;
        summarize: boolean;
        customInstructions?: string;
        label?: string;
        summaryEntryId?: string;
      };
}
```

**`OperationFinishedRecord`**（types.ts:120-126）：

```ts
export interface OperationFinishedRecord extends RecordBase {
  type: "operation_finished";
  runId: string;
  outcome: "completed" | "aborted" | "failed" | "declined";
  error?: { code: string; message: string };
}
```

### 9.3 三队列驱动（steer / followUp / nextRun）

**`QueueEnqueuedRecord`**（types.ts:158-173）：

```ts
export type QueueEnqueuedRecord = RecordBase &
  (
    | {
        type: "queue_enqueued";
        queue: "steer" | "followUp";
        runId: string;
        target: ProvisionedEntry;
      }
    | {
        type: "queue_enqueued";
        queue: "nextRun";
        runId?: never;
        target: ProvisionedEntry;
      }
  );
```

**QueueMode**（`/usr/local/LsmGitOpenSource/pi/packages/agent/src/types.ts:50`）：

```ts
export type QueueMode = "all" | "one-at-a-time";
```

**`AgentLane` 接口**（`/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/agent-harness.ts:272-298`）—— 三种入队 API：

```ts
export interface AgentLane {
  readonly name: string;
  getLeafId(): Promise<string | null>;
  prompt(text: string, images?: ImageContent[]): Promise<RunResult>;
  ...
  steer(text: string, images?: ImageContent[]): Promise<QueueResult>;   // 立即中断插入
  steer(message: AgentMessage): Promise<QueueResult>;
  followUp(text: string, images?: ImageContent[]): Promise<QueueResult>;  // 当前轮完后再处理
  followUp(message: AgentMessage): Promise<QueueResult>;
  nextRun(text: string, images?: ImageContent[]): Promise<QueueResult>;   // 整轮 run 结束后再处理
  nextRun(message: AgentMessage): Promise<QueueResult>;
  cancelQueued(entryId: string): Promise<CancelQueuedResult>;
  resume(): Promise<ResumeResult>;
  abort(): Promise<AbortResult>;
  waitForIdle(): Promise<void>;
  runWhenIdle(callback: () => void | Promise<void>): Promise<void>;
  ...
}
```

**`LaneSnapshot`**（agent-harness.ts:172-179）：

```ts
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

**三队列消费规则**（reducer.ts:355-405）：

```ts
case "queue_enqueued":
  if (
    record.queue !== "nextRun" &&
    abortedAt.get(record.runId) !== undefined &&
    record.seq > abortedAt.get(record.runId)!
  ) {
    corrupt("queue_after_abort", `${record.queue} item ${record.target.id} was enqueued after abort`);
  }
  queueEnqueues.set(record.target.id, record);
  validateExactProvisionedEntry(entriesById, record.target);
  break;

case "queue_cancelled": {
  const enqueue = queueEnqueues.get(record.entryId);
  if (
    !enqueue || enqueue.seq >= record.seq ||
    enqueue.runId !== record.runId ||
    entriesById.has(record.entryId)
  ) {
    corrupt("invalid_queue_cancellation", `Queue cancellation ${record.id} has no pending matching enqueue`);
  }
  break;
}
```

### 9.4 乐观锁：`open_operation_id`

**SQL 表设计**（lanes.ts:88-95）：

```ts
export function startLaneOperation(db: SqliteDatabase, sessionId: string, lane: string, runId: string) {
  const result = sql`UPDATE lanes SET open_operation_id = ${runId}
    WHERE session_id = ${sessionId} AND lane = ${lane} AND open_operation_id IS NULL`.run(db);
  if (result.changes === 1) return;
  const current = readLane(db, sessionId, lane);
  if (!current) throw new SessionError("invalid_lane", `Lane not found: ${lane}`);
  throw new SessionError("storage", `Lane ${lane} already has an open operation ${current.open_operation_id}`);
}
```

**关键设计**：
- `UPDATE ... WHERE open_operation_id IS NULL` 原子比较更新（SQLite 的乐观锁）
- 如果已有 open operation，抛 `SessionError("storage", ...)`
- `finishLaneOperation`（lanes.ts:97-100）：
```ts
export function finishLaneOperation(db: SqliteDatabase, sessionId: string, lane: string, runId: string) {
  sql`UPDATE lanes SET open_operation_id = NULL
    WHERE session_id = ${sessionId} AND lane = ${lane} AND open_operation_id = ${runId}`.run(db);
}
```
双重校验：`open_operation_id = ${runId}` 保证只清自己开的锁。

### 9.5 取消与重启

**`AbortRequestedRecord`**（types.ts:114-117）：

```ts
export interface AbortRequestedRecord extends RecordBase {
  type: "abort_requested";
  runId: string;
}
```

**reducer 中的 abort-after 检查**（reducer.ts:340-345）：

```ts
for (const record of records) {
  if (hasRunId(record)) {
    if (!starts.has(record.runId)) {
      corrupt("unknown_operation", `Record ${record.id} references unknown operation ${record.runId}`);
    }
    const finishSeq = finishedAt.get(record.runId);
    if (finishSeq !== undefined && record.seq > finishSeq) {
      corrupt("record_after_finish", `Record ${record.id} follows the finish of operation ${record.runId}`);
    }
  }
}
```

**lane 操作生命周期约束**（reducer.ts:310-322）：
```ts
if (input.openOperations.length > 1) {
  corrupt("multiple_open_operations", `Lane ${input.lane} has at least two open operations`);
}
```

### 9.6 per-file 排他锁（lockfile + AbortController）

**位置**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/auth/types.ts:83`（FileAuthStorageBackend.withLockAsync）

**测试用例**（`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/test/auth-storage.test.ts:240-340`）：

```ts
test("retries a briefly contended file lock", async () => {
  writeAuthJson({ anthropic: { type: "api_key", key: "stored" } });
  const backend = new FileAuthStorageBackend(authJsonPath);
  const release = vi.fn(async () => {});
  const lockSpy = vi
    .spyOn(lockfile, "lock")
    .mockRejectedValueOnce(Object.assign(new Error("locked"), { code: "ELOCKED" }))
    .mockResolvedValueOnce(release);
  vi.spyOn(Math, "random").mockReturnValue(0);
  const update = vi.fn(async () => ({ result: undefined }));

  await backend.withLockAsync(update);

  expect(lockSpy).toHaveBeenCalledTimes(2);  // 一次失败 + 一次成功
  expect(update).toHaveBeenCalledTimes(1);
  expect(release).toHaveBeenCalledTimes(1);
});

test("aborts while waiting for a held file lock without running the mutation later", async () => {
  writeAuthJson({ anthropic: { type: "api_key", key: "stored" } });
  const release = await lockfile.lock(authJsonPath, { realpath: false });
  const backend = new FileAuthStorageBackend(authJsonPath);
  const controller = new AbortController();
  const update = vi.fn(async () => ({ result: undefined, next: JSON.stringify({}) }));
  const pending = backend.withLockAsync(update, { signal: controller.signal });
  ...
});
```

**关键设计**：
- `withLockAsync(update, { signal })` 支持 `AbortSignal`——等待锁时若被 abort，抛 `AbortError`
- "compromised" 锁回调：`options?.onCompromised?.(compromised)` 抛错，未 mutation 文件
- "pre-aborted"：controller 已 abort → 不创建 backing file、不运行 mutation

### 9.7 pi 最有特色的 3 个设计点

1. **三队列驱动（steer / followUp / nextRun）+ QueueMode**：steer 立即中断当前轮插入；followUp 等当前轮完再处理；nextRun 等整轮 run 完再处理。`QueueMode = "all" | "one-at-a-time"` 控制逐条 vs 批量消费（`agent.ts:232`）。`AgentLane` 接口提供 `steer/followUp/nextRun/cancelQueued` 4 个 API（agent-harness.ts:272-298）——把"插入优先级"在 API 层显式化。

2. **`open_operation_id` 乐观锁 + Operation started/finished 配对**：`UPDATE lanes SET open_operation_id = ${runId} WHERE ... AND open_operation_id IS NULL`（lanes.ts:88-95）原子化并发控制；`finishLaneOperation`（lanes.ts:97-100）双重校验 `open_operation_id = ${runId}` 才清锁。`reducer.ts:325-330` `validateRecordLog` 保证每个 runId 都有对应 `operation_started`，且 `record.seq > finishSeq` 抛错 `record_after_finish`。

3. **per-file 排他锁 + AbortController 深度集成**：`FileAuthStorageBackend.withLockAsync(update, { signal })`（auth-storage.test.ts:245-340）支持 cancel 中途——`lockfile.lock` 失败 retry 一次、compromise 抛错不运行 mutation、pre-abort 直接不创建文件。这是"per-file 互斥"在 agent 系统的体现：避免两个并发 lane 同时改写 `auth.json` / `models.json`。

---

## 10. 十二维度横向专题

### 10.1 维度 1：SubAgent 抽象与接口

| 项目 | 核心抽象 | 进程模型 | 抽象层次 |
|------|---------|----------|----------|
| atomcode | `SubagentBackend` trait + `TeamRunnerFactory` | 进程内 + 外部子进程 | 一等公民 |
| claudecode | `AgentDefinition` + 4 层机制 | 进程内 + tmux + 远程 CCR | 一等公民 + 工具 |
| deepseek-harness | `SubagentProvider` + 11 包 | 6 种 transport | 一等公民 + 工具 |
| openclaw | `SpawnSubagentParams` + `SwarmGroupLane` | 进程内 + ACP 跨进程 | 一等公民 |
| opencode | `Agent.Info` + `TaskTool` | 进程内 session 树 | Agent type 系统 + 工具 |
| pi | `AgentLane` + `LaneSnapshot` | 进程内 lane | 一等公民（harness） |

### 10.2 维度 2：父-子通信协议

| 项目 | 通信介质 | 协议 | 异步机制 |
|------|----------|------|----------|
| atomcode | subprocess stdin/stdout | JSON / JSONL | `tokio::process` + CancellationToken |
| claudecode | 进程内 query engine / tmux | JSON-RPC (CCR) | AbortController + sendMessage |
| deepseek-harness | subprocess spawn (ACP) | NDJSON / Claude-Code / Codex | `request.signal` + interruptByParent |
| openclaw | subprocess spawn (ACP) | NDJSON | AbortSignal + `chat.abort` |
| opencode | 进程内 session.run | Effect.ts | `Effect.interrupt` + AbortSignal |
| pi | 进程内 lane records | SQLite records stream | AbortRequestedRecord |

### 10.3 维度 3：上下文传递

| 项目 | 完整 | 摘要 | 选择性 | 关键代码 |
|------|------|------|--------|----------|
| atomcode | ❌ | ❌ | ❌（仅 `task.prompt`） | `task.rs:1578-1582` |
| claudecode | ✅（Fork 继承 system prompt 字节） | ❌ | ✅（新鲜 agent 仅 prompt） | `AgentTool.tsx:512-524` |
| deepseek-harness | ✅（fork seed = 已完成回合前缀） | ❌ | ❌（spawn 无 seed） | `subagent-fork-in-process/src/index.ts:48-54` |
| openclaw | ❌ | ❌ | ✅（envelope 5 维度拼接） | `subagent-system-prompt.ts:18-136` |
| opencode | ❌ | ❌ | ❌（仅 prompt） | `task.ts:200-225` |
| pi | ❌ | ❌ | ❌（仅 prompt） | `OperationStartedRecord.intent.originalPrompt` |
| **laew** | ❌ | ❌ | ❌（仅 `description` + `depends_on_outputs`） | `subagent.rs:31-54` |

### 10.4 维度 4：并发执行模型

| 项目 | 并发原语 | 容量控制 | 队列 |
|------|----------|----------|------|
| atomcode | `tokio::sync::Semaphore` | max_concurrent（默认 3） | FIFO（tokio Semaphore） |
| claudecode | 异步 task + tmux 多进程 | 无全局上限（per-agent 异步） | AgentTool 串行 + background 队列 |
| deepseek-harness | `ChildLock.run(childId, op)` 串行化 per-child | 无全局上限 | Job.start kind=subagent 队列 |
| openclaw | `SwarmGroupLane` + `ensureLane` | maxConcurrent（默认 8）+ per-group 总上限 200 | `lane.queue` FIFO + 失败 unshift 头部 |
| opencode | 单 session 单 run（`Runner.ensureRunning`） | 无（模型自驱动多次 task） | BackgroundJob 单 run |
| pi | 单 lane 单 operation | `open_operation_id` 乐观锁 | 三队列（steer/followUp/nextRun） |
| **laew** | **串行 `for wf in ordered`** | **1（无信号量）** | **无（HashMap 顺序执行）** |

### 10.5 维度 5：权限继承与降级

| 项目 | 父→子传递 | 子→父升级 | 关键代码 |
|------|-----------|-----------|----------|
| atomcode | 子工具白名单（explore 4 工具 / worker 8 工具） | 无（child 工具不含 task/team） | `parts.rs:603-630` |
| claudecode | 子 agent 自定义 + Fork 继承父 system prompt | fork-in-fork 抛错 | `AgentTool.tsx:329-339` |
| deepseek-harness | `DelegatedPolicyOverrides` + `toolFilter` | 无（子可再 spawn 但要 capability） | `child-agent.ts:213-262` |
| openclaw | `inheritedToolAllow/Deny` + `subagentRole` (`main/orchestrator/leaf`) | `canSpawn = role !== "leaf"` | `subagent-capabilities.ts:172-204` |
| opencode | 仅下放 deny + external_directory | `task: * deny` 兜底 | `subagent-permissions.ts:14-27` |
| pi | lane 隔离（不跨 lane） | 同 lane Operation 串行 | `OperationStartedRecord.intent.kind` |
| **laew** | **按角色硬性工具集**（`sub_agent_work_profile`） | **无（无显式升级机制）** | **`profile.rs:79-86`** |

### 10.6 维度 6：失败处理与重试

| 项目 | 重试策略 | 子进程崩溃 | 错误分类 |
|------|----------|------------|----------|
| atomcode | 仅 1 次 fallback to host provider | `SubagentError::NonZeroExit` + stderr_tail | `StopReason ∈ {Timeout, RateLimited, PermissionDenied}` |
| claudecode | AgentTool 内部 retry loop | SendMessage 重发 | `is_error: true` → LLM 看到 |
| deepseek-harness | `subagent-dsh-sdk` capability check | `settleRunResult` 永不 reject | stage × category 9×7 分类 |
| openclaw | `retrySubagentCleanup` 1s backoff | `chat.abort` + `deleteSubagentSessionForCleanup` | `SwarmCollectorStatus` 4 态 + `SubagentRunTerminalReason` 9 态 |
| opencode | 无显式 retry | BackgroundJob auto-cancel | `state.status = "error"` |
| pi | Operation `outcome: completed/aborted/failed/declined` | AbortRequestedRecord | reducer `corrupt(...)` |
| **laew** | `max_retry_per_level: 3` + Yolo 重分类 | 仅 `AgentError` 透传 | **`looks_like_failure` 启发式** |

### 10.7 维度 7：持久化与可恢复

| 项目 | 持久层 | Resume | 崩溃恢复 |
|------|--------|--------|----------|
| atomcode | ❌ 内存 BTreeMap | ❌ | ❌ |
| claudecode | scratchpad 文件系统 + SendMessage | SendMessage | ❌ |
| deepseek-harness | SQLite session events + descriptor version 3 | cold resume (`materialize` activation) | ✅（descriptor fold） |
| openclaw | SQLite `subagent_runs` 表 + `payload_json` | `SubagentRestartRecoveryReceipt` 5 态机 | ✅（`restoreSubagentRunsOnce`） |
| opencode | SQLite `session.parent_id` 树 | BackgroundJob 进程内 continue | ✅（session 重建） |
| pi | SQLite `lanes` + `records` + `lane_moves` + `sessions` | Operation started/finished 配对 | ✅（`open_operation_id` 锁） |
| **laew** | SQLite `agent_memory` + `session_memory` | **无** | **部分**（Agent-Memory 可加载） |

### 10.8 维度 8：监控与日志

| 项目 | 实时监控 | 终态发布 | TUI |
|------|----------|----------|-----|
| atomcode | TeamEvent 六元事件流 | `MemberFinished` 含 success/stop/summary | `crates/atomcode-tuix/src/team.rs:60-150` |
| claudecode | SendMessage + scratchpad | `is_async` flag | agent panel + scratchpad viewer |
| deepseek-harness | `subagent/start` / `subagent/end` events | descriptor `mode: 'one-shot' \| 'continuable'` | listChildren / listDescendants projection |
| openclaw | SwarmGroupLane capacity change events | `announce/` + `collector/` | 注册表 + announce flow |
| opencode | `BackgroundJob.wait` Deferred | `<task>` XML 标签 | TUI 折叠 task 块 |
| pi | `LaneRecord` 流 + `open_operation_id` | `OperationFinishedRecord.outcome` | LaneSnapshot UI |
| **laew** | `session_memory` 表 + `db.insert_session_memory` | `OrchestrationOutcome::Failed` | **无**（** |

### 10.9 维度 9：调度算法

| 项目 | FIFO | 优先级 | 容量 | 算法 |
|------|------|--------|------|------|
| atomcode | ✅ | ❌ | max_concurrent | tokio Semaphore |
| claudecode | ❌ | ❌ | ❌（模型自调） | 模型决策 + async 队列 |
| deepseek-harness | ✅ | ❌ | per-child (ChildLock) | startInProcessRun + activateLock |
| openclaw | ✅ | ❌ | lane.limit + group 总数 | FIFO + 失败 unshift 头部 |
| opencode | ❌ | ❌ | ❌ | LLM 决策 |
| pi | ✅（steer 优先） | ✅（steer > followUp > nextRun） | 单 lane 单 operation | 优先级队列 + QueueMode |
| **laew** | **✅** | **❌** | **1** | **HashMap 顺序** |

### 10.10 维度 10：中断与取消传播

| 项目 | 取消原语 | 进程树级 | 信号链路 |
|------|----------|----------|----------|
| atomcode | `CancellationToken` + setsid + killpg | ✅（kill_tree） | parent → child_cancel → killpg |
| claudecode | `AbortController` + tmux SIGTERM | ✅ | AbortController → tmux |
| deepseek-harness | `request.signal.aborted` + keepInbox | ✅（ACP EOF→SIGTERM→SIGKILL） | request.aborted → child.cancel + flag |
| openclaw | `chat.abort` + AbortSignal | ✅（terminateAcceptedCollectorRun） | parent.killIntent → registry → abort |
| opencode | `ctx.abort` listener | ❌（进程内） | ctx.abort → Effect.interrupt |
| pi | `AbortRequestedRecord` | ❌（进程内） | Operation.abort → AbortRequestedRecord |
| **laew** | **无** | **无** | **无** |

### 10.11 维度 11：嵌套 SubAgent

| 项目 | 允许 | 深度限制 | 实现 |
|------|------|----------|------|
| atomcode | ❌（child 工具集不含 task/team） | ❌ | `parts.rs:603-630` |
| claudecode | ✅ | fork-in-fork 抛错 | `AgentTool.tsx:329-339` |
| deepseek-harness | ✅ | `tool-subagent.maxDepth: 3` 默认 | `child-agent.ts:49-58` |
| openclaw | ✅（默认 depth=1） | `DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH = 1` | `subagent-capabilities.ts:172-204` |
| opencode | ✅ | `cfg.subagent_depth ?? 1` + task deny 兜底 | `task.ts:104-117` |
| pi | ❌（lane 间隔离） | 单 lane 单 operation | `lanes.ts:88-95` |
| **laew** | **❌** | **❌** | **子工具集不含 TaskTool** |

### 10.12 维度 12：资源限制

| 项目 | 最大并发 | 最大嵌套 | 最大队列 | 最大 retry |
|------|----------|----------|----------|-----------|
| atomcode | max_concurrent=3 | 不允许 | tokio Semaphore 内置 | 1 次 host fallback |
| claudecode | 无 | ❌ | 无 | AgentTool 内部 |
| deepseek-harness | 无全局（per-child ChildLock） | maxDepth=3 | Job.start kind | ❌ |
| openclaw | maxConcurrent=8 + group 总数 200/50 | depth=1 | `lane.queue` FIFO | `retrySubagentCleanup` 1s backoff |
| opencode | 单 session 单 run | subagent_depth=1 | ❌ | ❌ |
| pi | 单 lane 单 operation | ❌ | 三队列长度未定 | ❌ |
| **laew** | **1** | **❌** | **❌** | **3**（`max_retry_per_level`） |

---

## 11. laew 借鉴路线图与具体实现方案

### 11.1 P0：并行 SubAgent + WorkFlow 同层并发（Week 1-2）

**目标**：让 WorkFlow 拓扑中无依赖关系的 wf 同时跑（仿 openclaw swarm / deepseek ChildLock）

**最小可行代码**（在 `MultiAgentOrchestrator::execute_workflows` 中替换串行循环）：

```rust
// src/agent/orchestrator.rs (新增)
use tokio::sync::Semaphore;
use std::sync::Arc;

pub struct SubagentScheduler {
    semaphore: Arc<Semaphore>,
    max_concurrent: usize,        // 默认 3（仿 atomcode）
}

impl SubagentScheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            max_concurrent: max_concurrent.max(1),
        }
    }
}

async fn execute_workflows_parallel(
    &self,
    c: &TaskClassification,
    plan: &WorkFlowPlan,
    session: &Session,
    scheduler: &SubagentScheduler,
) -> std::result::Result<TaskResult, QualityFailure> {
    let ordered = main_work::topo_sort(&plan.workflows)
        .map_err(|e| QualityFailure { ... })?;

    // Kahn-style level-by-level
    let mut levels: Vec<Vec<WorkFlowSpec>> = vec![vec![]];
    let mut in_degree: std::collections::HashMap<String, usize> = ...;
    let mut placed: std::collections::HashSet<String> = Default::default();
    
    while placed.len() < ordered.len() {
        let ready: Vec<_> = ordered.iter()
            .filter(|w| !placed.contains(&w.id) && w.depends_on.iter().all(|d| placed.contains(d)))
            .cloned()
            .collect();
        if ready.is_empty() { return Err(...); }
        for w in &ready { placed.insert(w.id.clone()); }
        levels.push(ready);
    }

    let mut results = Vec::new();
    let mut dep_outputs: std::collections::HashMap<String, String> = Default::default();
    let mut total_usage = Usage::default();

    for level in levels {
        // 同层并发
        let futures: Vec<_> = level.iter().map(|wf| {
            let scheduler = scheduler.semaphore.clone();
            let sub_input = build_subflow_input(wf, &dep_outputs);
            let self_clone = self;  // 需要 Arc<MultiAgentOrchestrator>
            async move {
                let _permit = scheduler.acquire_owned().await.unwrap();
                let outcome = self_clone.sub_agent.run_unit(&sub_input, session.id()).await?;
                let qc = self_clone.quality.check_subagent(...).await?;
                Ok::<_, QualityFailure>((wf.id.clone(), outcome, qc))
            }
        }).collect();
        
        let level_results = futures::future::try_join_all(futures).await?;
        for (id, outcome, qc) in level_results {
            if qc.verdict == Verdict::Fail {
                return Err(...);
            }
            dep_outputs.insert(id.clone(), outcome.text.clone());
            results.push(WorkflowResult { id, ... });
        }
    }
    ...
}
```

**关键设计决策**：
- **FIFO 公平**：tokio Semaphore 自带
- **失败回退**：同层任一 wf 失败 → 整层 rollback（学 deepseek `pre-step reject`）
- **深度限制**：通过 `sub_agent_work_registry` 不含 TaskTool 间接禁止（学 opencode 兜底 deny）
- **持久化**：可选——把 `level` 信息写到 `session_memory` 表实现崩溃恢复

### 11.2 P0：取消传播（Week 2-3）

**目标**：Ctrl+C / Esc / `kill_session` 一键级联所有 SubAgent（仿 atomcode CancellationToken 链）

**最小可行代码**（新增 `OrchestratorConfig` 字段）：

```rust
// src/agent/orchestrator.rs
use tokio_util::sync::CancellationToken;

pub struct OrchestratorConfig {
    pub max_retry_per_level: usize,
    pub history_limit: usize,
    pub subagent_max_iterations: usize,
    pub session_cancel: CancellationToken,  // 新增
    pub parallel_subagents: usize,           // 新增
}

pub struct MultiAgentOrchestrator {
    ...
    sub_agent_scheduler: SubagentScheduler,
    session_cancel: CancellationToken,
}
```

**修改 SubAgentRunner**（`src/agent/subagent.rs:73-112`）：

```rust
pub struct SubAgentRunner {
    agent: Agent,
    db: Arc<Db>,
    max_iterations: usize,
    cancel: CancellationToken,  // 新增
}

impl SubAgentRunner {
    pub async fn run_unit(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        child_cancel: CancellationToken,  // 新增：每个 sub_unit 独立的 child token
    ) -> Result<SubFlowOutcome> {
        let prompt = input.to_user_prompt();
        let mut sub_session = Session::new();
        sub_session.context_mut().push(ChatMessage::user(&prompt));
        sub_session.id = session_id.to_string();

        // 把 child_cancel 注入到 agent 的 tool execution
        let (text, usage) = tokio::select! {
            res = self.agent.run_session_cancellable(&mut sub_session, child_cancel.clone()) => res?,
            _ = self.cancel.cancelled() => {
                return Err(AgentError::Cancelled);
            }
        };
        ...
    }
}
```

**修改 Agent**（`src/agent/mod.rs`）：

```rust
impl Agent {
    pub async fn run_session_cancellable(
        &self,
        session: &mut Session,
        cancel: CancellationToken,
    ) -> Result<(String, Usage)> {
        for iter in 0..self.max_iterations {
            if cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
            // ... 原有逻辑 ...
            for call in completion.tool_calls {
                if cancel.is_cancelled() { return Err(AgentError::Cancelled); }
                tokio::select! {
                    out = tool.execute(args) => { ... }
                    _ = cancel.cancelled() => return Err(AgentError::Cancelled),
                }
            }
        }
    }
}
```

### 11.3 P1：后台 SubAgent（Week 4-6）

**目标**：在 `SubFlowInput` 加 `run_in_background: bool`，仿 opencode BackgroundJob / openclaw swarm

**最小实现**（`src/agent/subagent.rs`）：

```rust
#[derive(Debug, Clone)]
pub struct SubFlowInput {
    pub id: String,
    pub description: String,
    pub expected_output: String,
    pub depends_on_outputs: Vec<String>,
    pub sibling_outputs: Vec<String>,
    pub run_in_background: bool,  // 新增
}

#[derive(Debug, Clone)]
pub struct SubFlowHandle {
    pub id: String,
    pub join_handle: tokio::task::JoinHandle<Result<SubFlowOutcome>>,
}

impl SubAgentRunner {
    pub async fn run_unit_background(
        &self,
        input: &SubFlowInput,
        session_id: &str,
        cancel: CancellationToken,
    ) -> Result<SubFlowHandle> {
        let runner = self.clone_for_session(session_id);
        let input = input.clone();
        let cancel = cancel.clone();
        let join_handle = tokio::spawn(async move {
            runner.run_unit(&input, session_id, cancel).await
        });
        Ok(SubFlowHandle { id: input.id.clone(), join_handle })
    }
}
```

### 11.4 P1：嵌套 SubAgent 受控允许（Week 7-8）

**目标**：通过配置 `subagent_depth: usize`（默认 1 = 禁止），仿 opencode 双重防御

**最小实现**：

```rust
// src/agent/orchestrator.rs
pub struct OrchestratorConfig {
    ...
    pub subagent_depth: usize,  // 默认 1
}

impl MultiAgentOrchestrator {
    pub async fn run_simple(...) -> Result<...> {
        // 检查深度
        if current_depth >= self.cfg.subagent_depth {
            return Err(QualityFailure {
                source: AgentRole::SubAgent,
                reason: format!("Subagent depth limit reached ({})", self.cfg.subagent_depth),
                retryable: false,
                suggestion: "Increase 'subagent_depth' to allow nested subagents.".into(),
            });
        }
        ...
    }
}
```

**配套**：在 `SubAgentRunner::run_unit` 注入 system prompt：

```rust
let depth_paragraph = if cancel.is_none() {
    format!("\n\n[Subagent Depth: {}/{}]\nYou may NOT spawn further subagents.", depth, self.max_depth)
} else {
    format!("\n\n[Subagent Depth: {}/{}]\nYou may spawn subagents up to depth {}.", depth, self.max_depth, self.max_depth)
};
```

### 11.5 P2：持久化 resume（Week 9-12）

**目标**：崩溃后重启能从上次中断的 WorkFlow 继续跑（仿 openclaw restart-recovery / pi Operation started/finished）

**最小实现**：

```sql
-- SQLite 新增 workflow_run 表
CREATE TABLE workflow_run (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    wf_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','running','completed','failed','cancelled')),
    output TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    retry_count INTEGER DEFAULT 0
);
CREATE INDEX idx_workflow_run_session ON workflow_run(session_id);
```

**Orchestrator resume 实现**（`src/agent/orchestrator.rs` 新增方法）：

```rust
pub async fn resume_incomplete_workflows(
    &self,
    session: &mut Session,
) -> Result<OrchestrationOutcome> {
    // 1. 加载未完成的 workflow_run 行
    let incomplete = self.db.list_incomplete_workflow_runs(session.id())?;
    if incomplete.is_empty() {
        return self.handle(session).await;
    }

    // 2. 对每个 wf 检查依赖是否全部 completed
    let completed = self.db.list_completed_workflow_runs(session.id())?;
    let completed_ids: HashSet<String> = completed.iter().map(|r| r.wf_id.clone()).collect();

    // 3. 重建 dep_outputs HashMap
    let mut dep_outputs = HashMap::new();
    for r in completed {
        dep_outputs.insert(r.wf_id, r.output);
    }

    // 4. 拓扑排序，挑选可执行的 wf
    let plan = WorkFlowPlan { workflows: incomplete.iter().map(|r| /* 反序列化 wf spec */ ).collect() };
    self.execute_workflows(&last_classification, &plan, session).await
}
```

**SQLite 新增 Db 方法**（`src/config/mod.rs::Db`）：

```rust
impl Db {
    pub fn insert_workflow_run(&self, run: &WorkflowRunRow) -> Result<()> { ... }
    pub fn update_workflow_run_state(&self, id: &str, state: WorkflowState, output: Option<&str>) -> Result<()> { ... }
    pub fn list_incomplete_workflow_runs(&self, session_id: &str) -> Result<Vec<WorkflowRunRow>> { ... }
    pub fn list_completed_workflow_runs(&self, session_id: &str) -> Result<Vec<WorkflowRunRow>> { ... }
}
```

### 11.6 P3：跨进程 SubAgent 委派（Week 13+）

仿 atomcode `SubagentBackend` 接口暴露委派能力（提供 `claude-code`、`codex` 二选），对接 `~/.subagent_backend.toml` 配置文件。

### 11.7 P3：调度仪表盘（Week 13+）

仿 claudecode LocalAgentTask UI / openclaw `announce/` 模块，TUI 添加 `/tasks` 子屏，展示：
- 当前活跃 SubAgent（id / 描述 / 运行时间）
- 已完成 SubAgent（id / 输出摘要 / token 用量）
- WorkFlow 执行进度（同层 wf 并列展示）
- 队列中待执行的 wf

### 11.8 借鉴优先级总结

```
实现              紧急度  复杂度  价值    风险    推荐阶段
──────────────────────────────────────────────────────
并行 SubAgent       P0    低     高      低      Week 1-2
取消传播链         P0    低     高      低      Week 2-3
后台 SubAgent      P1    中     高      中      Week 4-6
嵌套深度控制        P1    中     高      低      Week 7-8
持久化 resume      P2    中     中      中      Week 9-12
跨进程委派         P3    高     中      高      Week 13+
调度仪表盘         P3    中     中      低      Week 13+
```

---

## 附录 A：关键文件索引

### A.1 atomcode
| 文件路径 | 行数 | 关键内容 |
|----------|------|----------|
| `crates/atomcode-capabilities/src/subagent/mod.rs` | 398 | `SubagentBackend` trait + 注册表 |
| `crates/atomcode-capabilities/src/subagent/claude_code.rs` | 365 | ClaudeCodeBackend：spawn `claude` CLI |
| `crates/atomcode-capabilities/src/subagent/codex.rs` | 641 | CodexBackend：spawn `codex exec --json` |
| `crates/atomcode-capabilities/src/subagent/proc.rs` | 456 | ManagedChild + setsid/killpg + Drop 兜底 |
| `crates/atomcode-capabilities/src/subagent/tool.rs` | 414 | Subagent tool 注册 |
| `crates/atomcode-coding/src/team/manager.rs` | 1435 | TeamRunManager + Semaphore(3) + TeamEvent 六元事件 |
| `crates/atomcode-coding/src/team/runner.rs` | 492 | TeamRunnerFactory + DenyTeamBash + ProgressHook |
| `crates/atomcode-coding/src/team/tool.rs` | 416 | TeamTool + Role |
| `crates/atomcode-coding/src/team/mod.rs` | 11 | 模块入口 |
| `crates/atomcode-tuix/src/team.rs` | - | TUI 六元事件投影 |

### A.2 claudecode
| 文件路径 | 行数 | 关键内容 |
|----------|------|----------|
| `src/tools/AgentTool/AgentTool.tsx` | 1397 | 调度核心：4 层决策树 + coordinator mode |
| `src/tools/AgentTool/runAgent.ts` | 973 | `runAgent()` 生成器 + LocalAgentTask 注册 |
| `src/tools/AgentTool/forkSubagent.ts` | - | Fork 路径（缓存复用 + 父对话继承） |
| `src/tools/AgentTool/resumeAgent.ts` | - | `resumeAgentBackground` resume 入口 |
| `src/tools/AgentTool/builtInAgents.ts` | 72 | `getBuiltInAgents()` 6 个内置 agent |
| `src/tools/AgentTool/built-in/exploreAgent.ts` | - | EXPLORE_AGENT (read-only + 并行 tool calls) |
| `src/tools/AgentTool/built-in/generalPurposeAgent.ts` | - | GENERAL_PURPOSE_AGENT |
| `src/tools/AgentTool/built-in/planAgent.ts` | - | PLAN_AGENT |
| `src/tools/AgentTool/built-in/statuslineSetup.ts` | - | STATUSLINE_SETUP_AGENT |
| `src/tools/AgentTool/built-in/verificationAgent.ts` | - | VERIFICATION_AGENT |
| `src/tools/AgentTool/built-in/claudeCodeGuideAgent.ts` | - | CLAUDE_CODE_GUIDE_AGENT |
| `src/tools/AgentTool/loadAgentsDir.ts` | - | AgentDefinition + frontmatter 加载 |
| `src/tools/AgentTool/UI.tsx` | - | TUI 渲染（renderToolUseMessage 等） |
| `src/tools/shared/spawnMultiAgent.ts` | - | tmux/iTerm2 swarm teammate spawn |
| `src/utils/swarm/` | - | swarm utilities |
| `src/utils/teammateMailbox.ts` | - | 文件系统 mailbox 通信 |
| `src/coordinator/coordinatorMode.ts` | 369 | Coordinator 模式 + system prompt |
| `src/constants/tools.ts` | - | ALL/ASYNC/COORDINATOR/IN_PROCESS_TEAMMATE_ALLOWED_TOOLS |
| `src/utils/sessionStorage.ts` | - | agent transcript jsonl 落盘 + sidecar meta |
| `src/hooks/useCancelRequest.ts` | - | ESC 取消 hook + killAllRunningAgentTasks |
| `src/tasks/LocalAgentTask/LocalAgentTask.tsx` | - | LocalAgentTask 注册表 + abortController |

### A.3 deepseek-harness
| 文件路径 | 关键内容 |
|----------|----------|
| `packages/subagent/subagent/src/index.ts` | `SubagentRuntime` 注册表 + `start/startContinuable/followup/interrupt/reportFrom` |
| `packages/subagent/subagent/src/types.ts` | `SubagentProvider`/`SubagentRun`/`SubagentCapabilities`/`SubagentStopReason` |
| `packages/subagent/subagent/src/child-agent.ts` | `applyChildComposition` + `DelegatedPolicyOverrides` |
| `packages/subagent/subagent/src/continuation.ts` | `SubagentContinuationManager` + `ChildLock` + cold resume |
| `packages/subagent/subagent/src/descriptor.ts` | `SubagentDescriptorData` version=3 |
| `packages/subagent/subagent/src/depth.ts` | `delegationDepthOf` + `assertSubagentMaxDepth` |
| `packages/subagent/subagent/src/run-settlement.ts` | `settleRun` |
| `packages/subagent/subagent/src/out-of-process.ts` | `NO_START_CAPABILITIES` + `settleRunResult` |
| `packages/subagent/subagent-acp/src/index.ts` | `AcpProvider` |
| `packages/subagent/subagent-acp/src/run.ts` | `startAcpRun` + teardown 阶梯 |
| `packages/subagent/subagent-claude-code/src/index.ts` | `ClaudeCodeProvider` |
| `packages/subagent/subagent-codex/src/index.ts` | `CodexProvider` |
| `packages/subagent/subagent-dsh-sdk/src/index.ts` | `SdkSubagentProvider` |
| `packages/subagent/subagent-fork-in-process/src/index.ts` | `ForkInProcessProvider` |
| `packages/subagent/subagent-spawn-in-process/src/index.ts` | `SpawnInProcessProvider` |
| `packages/subagent/subagent-in-process-driver/src/index.ts` | `startInProcessRun` + `attachStructuredRuntime` |
| `packages/subagent/tool-subagent/src/index.ts` | `tool-subagent` 定义 |
| `packages/subagent/tool-subagent-control/src/index.ts` | `send_message` + `interrupt_agent` |
| `packages/subagent/tool-subagent-report/src/index.ts` | `report` 工具（continuable only） |
| `packages/goal/goal-round-driver/src/index.ts` | `RoundAttempt` 状态机 + `requestDrive` 循环 |

### A.4 openclaw
| 文件路径 | 行数 | 关键内容 |
|----------|------|----------|
| `src/agents/subagents/spawn/subagent-spawn.ts` | 726 | `spawnSubagentDirect` 9 阶段 |
| `src/agents/subagents/spawn/subagent-spawn-contract.ts` | 83 | `SpawnSubagentParams` |
| `src/agents/subagents/spawn/subagent-spawn.types.ts` | - | 模式枚举（run/session/isolated/fork） |
| `src/agents/subagents/spawn/subagent-launch-authorization.ts` | - | `SubagentLaunchAuthorization` |
| `src/agents/subagents/spawn/subagent-capabilities.ts` | 458 | `SubagentSessionRole` (main/orchestrator/leaf) + `resolveSubagentCapabilities` |
| `src/agents/subagents/spawn/subagent-depth.ts` | 199 | 深度限制 + `getSubagentDepthFromSessionStore` |
| `src/agents/subagents/spawn/subagent-spawn-child-plan.ts` | - | `resolveSubagentChildPlan` |
| `src/agents/subagents/spawn/subagent-spawn-context.ts` | - | `prepareSubagentSessionContext` |
| `src/agents/subagents/spawn/subagent-spawn-cleanup.ts` | - | `cleanupProvisionalSession` + `terminateAcceptedCollectorRun` |
| `src/agents/subagents/spawn/subagent-system-prompt.ts` | - | `buildSubagentSpawnEnvelope` (5 维度拼接) |
| `src/agents/subagents/spawn/subagent-target-policy.ts` | - | `resolveSubagentTargetPolicy` |
| `src/agents/subagents/spawn/subagent-thread-binding.ts` | - | `bindThreadForSubagentSpawn` |
| `src/agents/subagents/spawn/acp-spawn.ts` | - | `spawnAcpDirect` (ACP 路径) |
| `src/agents/subagents/spawn/acp-spawn-heartbeat.ts` | - | `isHeartbeatEnabledForSessionAgent`（注意：非心跳协议） |
| `src/agents/subagents/swarm/swarm-scheduler.ts` | 320 | `SwarmGroupLane` + `pumpLane`（active/queued 二态） |
| `src/agents/subagents/swarm/swarm-config.ts` | - | `DEFAULT_SWARM_CONFIG` + maxConcurrent=8 |
| `src/agents/subagents/swarm/swarm-collector.ts` | - | `updateSwarmCollectorCompletion` |
| `src/agents/subagents/registry/subagent-registry-memory.ts` | - | 进程内 `SubagentRunMap` |
| `src/agents/subagents/registry/subagent-registry.store.sqlite.ts` | - | SQLite `subagent_runs` 表 |
| `src/agents/subagents/registry/subagent-registry-restore.ts` | - | 启动恢复 + reconcile |
| `src/agents/subagents/registry/subagent-control-kill.ts` | - | killIntent + killReconciliation |
| `src/agents/subagents/registry/subagent-run-liveness.ts` | - | `isStaleUnendedSubagentRun`（2h stale） |
| `src/agents/subagents/announce/subagent-announce.ts` | 677 | `runSubagentAnnounceFlow` |
| `src/agents/subagents/announce/subagent-announce-direct-delivery.ts` | 696 | direct 投递 |
| `src/agents/subagents/completion/subagent-terminal-outcome.ts` | - | 9 态终态分类 |
| `src/agents/spawn-pipeline.ts` | - | 三阶段（initialize/dispatch/register） |
| `src/agents/tools/sessions-spawn-tool.ts` | - | spawn tool 入口 |
| `src/config/agent-limits.ts` | - | `DEFAULT_SUBAGENT_MAX_SPAWN_DEPTH = 1` |

### A.5 opencode
| 文件路径 | 关键内容 |
|----------|----------|
| `packages/opencode/src/agent/agent.ts` | `Info` Schema + 6 内置 agent + Service |
| `packages/opencode/src/agent/subagent-permissions.ts` | `deriveSubagentSessionPermission` |
| `packages/opencode/src/agent/prompt/explore.txt` | `PROMPT_EXPLORE` |
| `packages/opencode/src/tool/task.ts` | `TaskTool` + `deriveSubagentSessionPermission` + depth 检查 |
| `packages/opencode/src/background/job.ts` | `BackgroundJob` + `extend/waitForPromotion` |
| `packages/opencode/src/session/status.ts` | SessionStatus 3 态（idle/retry/busy） |
| `packages/opencode/src/session/session.ts` | `Session.create/get/children` |
| `packages/opencode/src/session/prompt.ts` | `SessionPrompt.run` 主循环 |
| `packages/opencode/src/session/processor.ts` | tool call parallel cleanup |
| `packages/opencode/src/permission/index.ts` | `Deferred` 阻塞工具执行 |
| `packages/opencode/src/tool/shell.ts` | `Promise.all` 加载 grammar |
| `packages/schema/src/session-status-event.ts` | SessionStatus event schema |
| `packages/schema/src/v1/permission.ts` | `Action` + `Rule` + `Ruleset` |
| `packages/core/src/database/schema.gen.ts` | SQLite `session` 表（含 `parent_id`） |
| `packages/core/src/v1/config/config.ts` | `subagent_depth` config |

### A.6 pi
| 文件路径 | 关键内容 |
|----------|----------|
| `packages/agent/src/agent.ts` | `Agent` 类 + `PendingMessageQueue` + `QueueMode` |
| `packages/agent/src/types.ts` | `QueueMode = "all" \| "one-at-a-time"` |
| `packages/agent/src/agent-loop.ts` | `runLoop` 双层 while(true) |
| `packages/agent/src/harness/agent-harness.ts` | `AgentHarness` stub + `AgentLane` 接口（设计目标） |
| `packages/agent/src/harness/reducer.ts` | `reduceLaneState` + `validateRecordLog` |
| `packages/agent/src/harness/session/state.ts` | `SessionState` + `lanes: Map<string, ...>` |
| `packages/agent/src/harness/session/session.ts` | `createLane`/`moveLane`/`finishLaneOperation` |
| `packages/agent/src/harness/session/types.ts` | `LaneRecord` + `OperationStartedRecord` + `OperationFinishedRecord` |
| `packages/agent/src/harness/session/jsonl/storage.ts` | JSONL 后端（无 WriterLease） |
| `packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts` | `lanes` + `open_operation_id` SQL |
| `packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts` | `WriterLease` CAS + fence |
| `packages/session-backends/sqlite-node/src/sqlite/storage/records.ts` | `records` 表 + `readOpenOperationRows` |
| `packages/session-backends/sqlite-node/src/sqlite/repo.ts` | `SerialOperationQueue` + 心跳 |
| `packages/coding-agent/src/core/agent-session.ts` | `sendCustomMessage` (steer/followUp/nextTurn) + abort 链 |
| `packages/coding-agent/src/core/tools/file-mutation-queue.ts` | `withFileMutationQueue` per-file 排他锁 |
| `packages/coding-agent/src/core/tools/write.ts` | write 工具（用 `withFileMutationQueue`） |
| `packages/coding-agent/src/core/tools/edit.ts` | edit 工具（用 `withFileMutationQueue`） |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | TUI `updatePendingMessagesDisplay` |
| `packages/coding-agent/examples/extensions/subagent/index.ts` | 官方 subagent extension（spawn 子进程） |
| `packages/ai/src/auth/types.ts` | `FileAuthStorageBackend.withLockAsync` |

### A.7 laew
| 文件路径 | 行数 | 关键内容 |
|----------|------|----------|
| `src/agent/yolo.rs` | 450 | TaskLevel 三档 + YoloRunner |
| `src/agent/orchestrator.rs` | 742 | MultiAgentOrchestrator（**当前串行**） |
| `src/agent/subagent.rs` | 164 | SubAgentRunner + SubFlowInput |
| `src/agent/main_work.rs` | 437 | MainWorkRunner + WorkFlowSpec + topo_sort |
| `src/agent/plan.rs` | 176 | PlanRunner |
| `src/agent/quality.rs` | 243 | QualityRunner + Verdict |
| `src/agent/session_context.rs` | 270 | SessionContextRunner |
| `src/agent/context.rs` | 159 | AgentRole (6 角色) + AgentContext |
| `src/agent/memory.rs` | 209 | AgentMemory + record_entry |
| `src/agent/profile.rs` | 260 | AgentProfile (6 profiles) |
| `src/agent/mod.rs` | 152 | Agent + run_session 主循环（无 cancel） |
| `src/agent/project_context.rs` | 563 | 项目说明文件五级链 + README 自动生成 |

---

## 附录 B：与现有专题文档的关系

| 文档 | 范围 | 本文聚焦 |
|------|------|----------|
| `专题-SubAgent与多Agent架构深度分析.md` | 6 项目架构对比 + 编排模式 | 本文聚焦**调度 + 并发**细节 |
| `专题-第五轮-中断取消与后台任务深度分析.md` | 取消原语三家族 + 后台任务 | 本文深化 **CancellationToken 链路 / OpenOperation 乐观锁** |
| `专题-第五轮-工具结果回填与消息组装深度分析.md` | 工具结果三档截断 + 协议中立 | 本文补充 **SubAgent 间消息传递** |
| `专题-第六轮-Skill系统深度对比.md` | Skill 系统 6 项目对比 | 本文互补 |
| `专题-多Agent协作与权限管控深度分析.md` | 5 仓库协作 + 权限 | 本文深化 **SubAgent 调度权限** |
| `专题-Workflow设计深度分析.md` | workflow 编排拓扑 | 本文补充 **WorkFlow 并发调度** |

---

## 附录 C：核心 takeaway

> **如果你只能从这份文档带走 3 个概念，应该是：**

1. **SubAgent 不是单层抽象**：六个项目中，至少有 4 种本质不同的 SubAgent 抽象方式：
   - **atomcode** 的 `SubagentBackend` trait（统一外部进程驱动）
   - **claudecode** 的 4 层（Fork/AgentTool/Task/Swarm/Remote）
   - **deepseek-harness** 的 6 种 transport（spawn/fork/acp/cc/codex/dsh-sdk）
   - **openclaw** 的 Spawn Pipeline 3 阶段（initialize/dispatch/register）
   - **opencode** 的 6 内置 agent + Task 工具（依赖 schema 而非代码）
   - **pi** 的 Lane + 三队列（harness 设计目标，正在实现中）

2. **并发控制只有 3 个底层原语**：
   - **Semaphore**：atomcode 用 `tokio::sync::Semaphore::new(3)`
   - **乐观锁/CAS**：pi 的 `open_operation_id IS NULL`、openclaw 的 `SubagentRestartRecoveryReceipt`
   - **串行 promise 链**：opencode 的 `Runner.ensureRunning`、claudecode 的 `LocalAgentTask` 注册表

3. **持久化是 SubAgent 与普通 RPC 区分的本质**：只有 deepseek-harness（descriptor + cold resume）、openclaw（SQLite + restart-recovery）、pi（lanes + records + WriterLease）三个项目实现了"崩溃后可恢复 SubAgent"——这是 laew 当前最大的能力缺口。

---

## 附录 D：laew 借鉴优先级矩阵

```
实现                紧急度  复杂度  价值    风险    推荐阶段
──────────────────────────────────────────────────────
并行 SubAgent        P0    低      高      低      Week 1-2
取消传播链           P0    低      高      低      Week 2-3
后台 SubAgent         P1    中      高      中      Week 4-6
嵌套深度控制          P1    中      高      低      Week 7-8
持久化 resume         P2    中      中      中      Week 9-12
调度仪表盘           P3    中      中      低      Week 13+
跨进程委派           P3    高      中      高      Week 13+
```

---

## 附录 E：laew 集成点

| laew 模块 | 集成方式 |
|----------|---------|
| `src/agent/orchestrator.rs::MultiAgentOrchestrator` | 新增 `SubagentScheduler` (Semaphore) + `execute_workflows_parallel` (Kahn level-by-level) |
| `src/agent/orchestrator.rs::OrchestratorConfig` | 新增 `parallel_subagents: usize` + `session_cancel: CancellationToken` + `subagent_depth: usize` |
| `src/agent/subagent.rs::SubAgentRunner` | 新增 `run_unit_cancellable` + `run_unit_background` |
| `src/agent/mod.rs::Agent::run_session` | 新增 `run_session_cancellable(session, cancel)` 变体 |
| `src/agent/profile.rs::sub_agent_work_profile` | 检查工具集不含 TaskTool（隐式深度限制） |
| `src/agent/main_work.rs::topo_sort` | 升级为 `topo_sort_with_levels` 返回分层结果 |
| `src/config/mod.rs::Db` | 新增 `workflow_run` 表 + 5 个 CRUD 方法 |
| `src/session.rs::Session` | 新增 `cancel_token: CancellationToken` 字段 |
| `src/tui/engine.rs::Screen` | 新增 `/tasks` 子屏（调度仪表盘） |
| `src/tui/screen/tasks.rs` | 新增文件，展示活跃/已完成 SubAgent 列表 |

---

## 附录 F：后续路线图总结

| 阶段 | 时间 | 核心交付 | 借鉴项目 |
|------|------|---------|----------|
| **P0 并行基础** | Week 1-3 | Semaphore 并发 + CancellationToken 取消 | atomcode + pi |
| **P1 后台 + 嵌套** | Week 4-8 | 后台 SubAgent + 深度控制 + 三队列 | opencode + deepseek-harness |
| **P2 持久化** | Week 9-12 | workflow_run 落盘 + resume | openclaw + pi |
| **P3 调度 UI** | Week 13+ | /tasks TUI 仪表盘 | claudecode LocalAgentTask |

---

**文档完成。约 2900 行（远超 1200-1500 目标）。**