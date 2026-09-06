# 专题：6 个 Agent 项目的目标/意图识别与规划横向分析

> **元信息**
> - 生成日期：2026-09-04
> - 分析范围：deepseek-harness / openclaw / claudecode / atomcode / opencode / pi
> - 输入文档：`docs/Agent源码调研/` 下 6 份「核心机制深度分析」
> - 对标基准：laew Yolo 三步分析（`src/agent/yolo.rs`）+ Plan Agent（`src/agent/plan.rs`）
> - 定位：聚焦「目标识别 / 意图识别 / 目标规划 / 目标拆分 / 目标生命周期」五个上游环节，与既有「任务拆解与分类」专题互补——本专题关注「目标如何被识别、建模、规划、拆分、驱动执行并闭环」，而非「任务如何被拆解为 WorkFlow 步骤」。

## 目录

- [1. 横向对比总览表](#1-横向对比总览表)
- [2. DeepSeek-Harness：最完整的 Goal 状态机 + Plan 协作态双轨模型](#2-deepseek-harness最完整的-goal-状态机--plan-协作态双轨模型)
- [3. OpenClaw：会话级 Goal 事务 + 压缩规划双轨](#3-openclaw会话级-goal-事务--压缩规划双轨)
- [4. Claude Code：Effort 档位 + 五阶段 Plan Mode V2](#4-claudecodeeffort-档位--五阶段-plan-mode-v2)
- [5. AtomCode：TeamDifficulty 二档 + PlanModeGate 中间件](#5-atomcode-teamdifficulty-二档--planmodegate-中间件)
- [6. OpenCode：plan/build 双 Agent 切换 + plan_exit 工具](#6-opencodeplanbuild-双-agent-切换--plan_exit-工具)
- [7. Pi：steer/followUp 队列 + 扩展式 Plan Mode](#7-pisteerfollowup-队列--扩展式-plan-mode)
- [8. 设计模式提炼](#8-设计模式提炼)
- [9. 对 laew 的综合建议](#9-对-laew-的综合建议)
- [附录 A：各项目 goal/plan 代码片段速查](#附录-a各项目-goalplan-代码片段速查)
- [附录 B：laew 当前 Yolo/Plan 现状速览](#附录-blaew-当前-yoloplan-现状速览)
- [附录 C：建议实现方案详设](#附录-c建议实现方案详设)

---

## 1. 横向对比总览表

### 1.1 五维对比总表

| 维度 | DeepSeek-Harness | OpenClaw | Claude Code | AtomCode | OpenCode | Pi | **laew(对标)** |
|------|------------------|----------|-------------|----------|----------|-----|---------------|
| **目标模型** | 显式 Goal 状态机（`GoalPhase` 4 态） | 显式 `SessionGoal`（6 态 + token 预算） | 无显式 Goal，靠 Plan Mode + Effort | 无显式 Goal，靠 TeamDifficulty 二档 | 无显式 Goal，靠 plan/build Agent 切换 | 无显式 Goal，靠 steer 队列 | 显式 `TaskClassification`（三档） |
| **意图识别** | 无独立意图层；Goal objective 由人/模型写入 | 无独立意图层 | Effort 档位（low/medium/high/max）最接近意图强度 | 无独立意图层；role 即意图 | 无独立意图层 | 无独立意图层 | **目的→目标→意图** 三步分析 |
| **目标生命周期** | `active→paused→blocked→complete` + `armed/disarmed` + round 计数 | `active→paused→blocked→complete` + `budget_limited/usage_limited` | Plan Mode 进入/退出；无持久 Goal | PlanModeGate 开/关 | plan↔build 切换 | plan mode 开/关 + execution mode | 无持久生命周期（单次分类） |
| **规划生成** | Plan Mode：LLM 输出 Markdown 到 plan 文件，5 阶段工作流 | 无任务级 Plan Mode；有压缩规划（`compaction-planning`） | Plan Mode V2：5 阶段（理解→设计→评审→定稿→Exit），写 plan 文件 | PlanModeGate：只读探索 + 口头方案，不落盘 | plan Agent 写 `.opencode/plans/*.md`，`plan_exit` 切 build | 扩展式 Plan Mode：提取编号步骤 + `[DONE:n]` 标记 | Plan Agent 输出 `plans/{session_id}-{seq}.md`（五段式） |
| **目标拆分** | Goal 是单目标；拆分靠 round 驱动（`goal-round-driver`） | 无子目标拆分 | Plan Phase 2 可并行多个 Plan Agent | `TeamTool.delegate` 拆多任务（14 角色） | 无子目标 | 无子目标 | `decomposition_plan: Vec<String>` |
| **重规划** | Goal `edit` + `goal-round-driver` 自动续 round | Goal `edit`/`resume` | Plan 被拒 → 修订再提 | 无 | 无 | steer 注入新指令 | 失败回流 → 修订 plan |
| **持久化** | Session 事件日志（`goal/change` whole-value） | SQLite（`session_entry.goal` + 操作收据） | plan 文件落盘（`~/.claude/plans/`） | 无持久化 | plan 文件落盘 | 扩展状态（`appendEntry`） | plan 文件落盘 + `agent_memory` |
| **执行耦合** | Goal 是状态，调度靠 `goal-round-driver`（opt-in） | Goal 不驱动执行；执行靠 turn 循环 | Plan → ExitPlanMode → 执行 | TeamTool 直接派生子 Agent | plan_exit 切 build Agent 执行 | execution mode 执行编号步骤 | Plan → Main-Work → SubAgent |

### 1.2 目标/规划光谱定位图

```
隐式意图 ←──────────────────────────────────────────────→ 显式 Goal 模型

  opencode     pi       claudecode       atomcode       openclaw       deepseek       laew
  (无Goal)   (无Goal)   (Effort+Plan)   (Difficulty)   (SessionGoal)   (GoalPhase)   (TaskLevel)
                                                          ↑                              ↑
                                                    最强持久化                    最强意图识别(三步)

弱持久化 ←──────────────────────────────────────────────→ 强持久化

  atomcode     pi       claudecode       opencode       laew         openclaw       deepseek
  (内存态)   (扩展态)   (plan文件)      (plan文件)    (plan+内存)   (SQLite+收据)   (事件日志+fold)
```

### 1.3 规划生成方式定位

| 规划形态 | 项目 | 消费方式 |
|----------|------|----------|
| **一次性长程计划**（写文件，审批后执行） | deepseek-harness Plan Mode、claudecode Plan Mode V2、opencode plan Agent、laew Plan Agent | 人工审批 → 解析 → 执行 |
| **逐步短程计划**（每 round 续） | deepseek-harness `goal-round-driver` | 自动续 round，每轮重读目标 |
| **无任务计划，有压缩规划** | OpenClaw `compaction-planning` | 上下文溢出时触发，非用户任务 |
| **无计划，靠队列驱动** | pi steer/followUp | 运行时注入 |

---

## 2. DeepSeek-Harness：最完整的 Goal 状态机 + Plan 协作态双轨模型

> 本节是全文重点。DeepSeek-Harness 的 `packages/goal` 与 `packages/plan` 是 6 个项目里**唯一**把「目标」做成**显式、持久、可重放状态机**的实现，且与 laew Yolo 的命名同源（Yolo/Goal 评估器概念即源自此处）。

### 2.1 架构总览

```
用户/模型
    │
    ├─ /goal 命令 ──────────► command-goal ──► ctx.goals.create/edit/pause/resume/clear
    │
    ├─ get_goal/create_goal/update_goal 工具 ──► tool-goal ──► ctx.goals.*
    │                                                │
    │                                          authority.ts（权限校验）
    │
    └─ goal-round-driver ──► 自动续 round（armed → followup goal_round 消息）
                                │
                                ▼
                          session 事件日志（goal/change whole-value）
                                │
                                ▼
                          foldGoal() 纯回放折叠 → GoalProjection
```

包结构（`packages/goal/`）：

| 包 | 角色 | ctx key |
|----|------|---------|
| `goal` | 持久 Goal 服务：create/edit/pause/resume/complete/block/clear | `ctx.goals` |
| `tool-goal` | 模型工具 `get_goal`/`create_goal`/`update_goal` | `ctx.tools` |
| `command-goal` | 人 `/goal` 命令 | `ctx.commands` |
| `goal-round-driver` | 自动续 round 驱动器 | 无服务 key |

### 2.2 Goal 状态机（核心）

**文件**：`packages/goal/goal/src/types.ts`

```typescript
/** Durable continuation phase. Activation is process-local and separate. */
export type GoalPhase = 'active' | 'paused' | 'blocked' | 'complete'

export type GoalActivation = 'armed' | 'disarmed'

export interface GoalSnapshot extends GoalRef {
  readonly objective: string          // 人/模型写的完成目标
  readonly phase: GoalPhase
  readonly blockedReason?: GoalBlockReason
  readonly maxGoalRounds: number      // round 上限
}

export interface GoalView extends GoalSnapshot {
  readonly roundsStarted: number      // 已执行 round 数
  readonly createdAt: number
  readonly updatedAt: number
  readonly activation: GoalActivation // 进程本地，不持久
}
```

**状态转移**（`packages/goal/goal/src/fold.ts` → `validateSnapshotTransition`）：

```
create ──► active ──► paused ──► active(resume)
   │          │
   │          ├──► blocked ──► active(resume)
   │          │
   │          └──► complete(终态)
   │
   clear ──► 墓碑（tombstone）
```

关键约束（`fold.ts`）：
- `create` 必须是 `revision===1 && phase==='active' && roundsStarted===0`
- `pause` 只能从 `active→paused`
- `resume` 只能从 `paused/blocked→active`，且 `roundsStarted < maxGoalRounds`
- `block` 只能从 `active→blocked`，必须带 `blockedReason`
- `complete` 只能从非 complete→complete
- 所有非 create 操作必须 `revision = current.revision + 1`（CAS）

### 2.3 持久化：事件日志 + 纯折叠

**文件**：`packages/goal/goal/src/domain.ts`

```typescript
export type GoalOperation =
  | 'create' | 'edit' | 'pause' | 'resume' | 'complete' | 'block' | 'clear'

export interface GoalSnapshotChangeMeta {
  readonly kind: 'goal/change'
  readonly version: 1
  readonly operation: Exclude<GoalOperation, 'clear'>
  readonly goal: GoalSnapshot
  readonly roundsStarted: number
  readonly createdAt: number
  readonly updatedAt: number
}
```

设计要点：
- **whole-value 规则**：每次 `goal/change` 事件携带完整 post-change 状态，fold 是 last-wins
- **消息归因**：`GoalMessageSource { kind:'goal', goalId, revision, round }` 标记自动续 round 的用户消息
- **纯折叠**：`foldGoal(events)` 从事件日志重放，无独立存储
- **不变量守护**：`packages/goal/goal/src/invariant.ts` 在 `session/event` 发布前校验

### 2.4 模型工具：get_goal / create_goal / update_goal

**文件**：`packages/goal/tool-goal/src/index.ts`

```typescript
// create_goal 描述（节选）
const CREATE_DESCRIPTION =
  'Create one persisted same-session completion goal when the current direct human request ' +
  'is a long-running objective that should continue across autonomous goal rounds. You may ' +
  'infer that intent without requiring the user to say "create a goal". Do not use this for ' +
  'trivial single-turn work.'

// update_goal action 枚举
type UpdateAction = 'edit' | 'pause' | 'resume' | 'complete' | 'blocked'
```

**权限模型**（`packages/goal/tool-goal/src/authority.ts`）：
- `edit/pause/resume` → 必须 `requireDirectHuman`（仅人可直接操作）
- `complete/blocked` → 人直接操作 **或** 当前 goal round（`completionAuthority`）
- `blocked` 阈值：`blockedAfterConsecutiveRounds`（默认 3），不到轮次拒绝
- `wrapup.ts`：complete/blocked 后注入 `<goal_complete>`/`<goal_blocked>` 收尾提示

### 2.5 自动续 round：goal-round-driver

**文件**：`packages/goal/goal-round-driver/src/index.ts`

```typescript
// 续 round 提示（prompt.ts）
export function renderGoalRoundPrompt(goal: GoalView, round: number): ContentBlock[] {
  return [{
    type: 'text',
    text: '<goal_round>\n'
      + `Objective: ${JSON.stringify(goal.objective)}\n`
      + `Round: ${round}/${goal.maxGoalRounds}\n\n`
      + 'Continue working toward the objective in this same session. ...'
      + '</goal_round>',
  }]
}
```

驱动逻辑：
- 监听 `agent/status idle` → `drive()` → 若 goal `active+armed` 且 `roundsStarted < maxGoalRounds` → `agent.followup(goal_round 消息)`
- 竞态防护：`RoundAttempt { phase: 'queued'|'claimed'|'admitted' }` + `validReservation` 校验
- round 上限触发：`ctx.goals.block(..., { code:'round-limit', ... })`

### 2.6 Plan Mode：与 Goal 独立的协作态

**文件**：`packages/plan/plan-mode/src/index.ts`

```typescript
export class PlanModeController extends Service {
  // plan/mode 事件：whole-value replace，last-wins
  // /plan 命令：进入/退出
  // exit_plan_mode 工具：提审批（Approve / Keep planning）
}
```

关键差异（vs Goal）：
- Plan Mode 是**协作态**（guidance section），不是状态机
- 工具全部可用（不限制），靠 `plan:policy` section 引导
- `exit_plan_mode` 触发用户审批，可带反馈（`Keep planning` + custom feedback）
- `PlanProjection { active, pending }` 是纯事件折叠（`foldPlanMode`）

### 2.7 对 laew 的启示

1. **Goal 状态机**：laew 当前 `TaskClassification` 是单次分类，无持久生命周期。可借鉴 `GoalPhase` 四态 + `armed/disarmed`。
2. **whole-value 事件 + 纯折叠**：laew 的 `agent_memory` 是追加日志，可借鉴 last-wins whole-value 简化 fold。
3. **自动续 round**：laew 的 Plan→Main→SubAgent 是单次流水线，可借鉴 `goal-round-driver` 做「目标驱动的多轮自动执行」。
4. **blocked 阈值**：laew 的失败回流可借鉴 `blockedAfterConsecutiveRounds` 做「N 次连续失败才真正 blocked」。

---

## 3. OpenClaw：会话级 Goal 事务 + 压缩规划双轨

### 3.1 架构总览

OpenClaw 的 Goal 是**会话级持久状态**，带**事务收据**（operation receipt）防重放；规划能力不在任务层，而在**上下文压缩层**（`compaction-planning`）。

```
模型 ──► get_goal / create_goal / update_goal 工具
              │
              ▼
        goals-operations.ts（mutateSessionGoal + SQLite 收据）
              │
              ▼
        session_entry.goal（持久）
```

### 3.2 SessionGoal 状态机

**文件**：`src/config/sessions/goals-transitions.ts`

```typescript
export function buildCreatedSessionGoal(entry, options, now): SessionGoal {
  return {
    schemaVersion: 1,
    id: crypto.randomUUID(),
    objective,
    status: 'active',
    createdAt: now, updatedAt: now,
    tokenStart: ..., tokenStartFresh: ...,
    tokensUsed: 0,
    ...(tokenBudget ? { tokenBudget } : {}),
    continuationTurns: 0,
  }
}

export function buildUpdatedSessionGoalStatus(entry, options, now): SessionGoal {
  // active/paused/blocked/complete
  // resetsBudgetWindow：从 limited 恢复时重置 token 窗口
}
```

**状态集合**：`active | paused | blocked | complete | budget_limited | usage_limited`

**独特设计**：
- **token 预算**：`tokenBudget` + `tokensUsed`，超预算自动转 `budget_limited`
- **预算窗口重置**：从 `budget_limited/usage_limited` resume 时重置 `tokenStart`
- **continuationTurns**：续 turn 计数

### 3.3 操作事务与防重放

**文件**：`src/config/sessions/goals-operations.ts`

```typescript
export type SessionGoalOperation = SessionGoalOperationIdentity & (
  | { action: 'start'; objective: string; tokenBudget?: number }
  | { action: 'edit'; goalId: string; objective: string }
  | { action: 'resume' | 'pause' | 'block' | 'complete'; goalId: string; note?: string }
  | { action: 'clear'; goalId: string }
)

// 操作身份
type SessionGoalOperationIdentity = {
  operationId: string
  issuedAtMs: number
  requestFingerprint: string   // 完整请求哈希
}
```

防重放机制：
- `OPERATION_VALIDITY_MS = 24h`（操作有效期）
- `OPERATION_FUTURE_SKEW_MS = 5m`（未来时间戳容差）
- `MAX_SESSION_RECEIPTS = 4096`（收据上限）
- `writeSessionGoalOperationReceipt`：写入 SQLite 收据表
- `lookupSessionGoalOperation`：先查收据再执行（幂等）

### 3.4 模型工具

**文件**：`src/agents/tools/goal-tools.ts`

```typescript
export function createGetGoalTool(options): AnyAgentTool {
  return {
    name: 'get_goal',
    description: 'Get thread goal, status, token usage.',
    execute: async () => jsonResult(await getSessionGoal({ ...scope, persist: false })),
  }
}

export function createUpdateGoalTool(options): AnyAgentTool {
  // 仅允许 complete | blocked（MODEL_UPDATABLE_SESSION_GOAL_STATUSES）
  // complete only achieved; blocked only same blocker 3+ consecutive goal turns
}
```

**权限**：模型只能 `complete/blocked`，不能 `edit/pause/resume`（仅人）。

### 3.5 压缩规划（compaction-planning）

**文件**：`src/agents/compaction-planning.ts`

```typescript
export type StageSplitPlan = { mode: 'single' } | { mode: 'split'; chunks: AgentMessage[][] }
export type OversizedFallbackPlan = { smallMessages: AgentMessage[]; oversizedNotes: string[] }

export function buildStageSplitPlan(params): StageSplitPlan { ... }
export function buildOversizedFallbackPlan(params): OversizedFallbackPlan { ... }
export function buildSummaryChunks(params): AgentMessage[][] { ... }
```

**注意**：这是**上下文压缩规划**，不是任务规划。大历史溢出时触发，用 worker 线程（`compaction-planning-worker`）做 CPU 密集型分块。

### 3.6 对 laew 的启示

1. **token 预算**：laew 的 Plan Agent 可借鉴 `tokenBudget` 做「规划阶段 token 上限」。
2. **操作收据**：laew 的 `agent_memory` 是追加日志，可借鉴收据表做「操作幂等 + 防重放」。
3. **预算窗口重置**：laew 的失败回流可借鉴「从 limited 恢复时重置计数」。

---

## 4. Claude Code：Effort 档位 + 五阶段 Plan Mode V2

### 4.1 架构总览

Claude Code 无显式 Goal 模型，靠两套机制覆盖「目标/规划」：
- **Effort 档位**：最接近「意图强度」的显式维度
- **Plan Mode V2**：5 阶段规划工作流，写 plan 文件 + 审批

```
用户输入
    │
    ├─ Effort 档位（low/medium/high/max）──► 影响模型思考时长
    │
    └─ EnterPlanMode ──► Plan Mode V2
                          │
                          ├─ Phase 1: Initial Understanding（Explore Agent 并行）
                          ├─ Phase 2: Design（Plan Agent 并行）
                          ├─ Phase 3: Review
                          ├─ Phase 4: Final Plan（写 plan 文件）
                          └─ Phase 5: ExitPlanMode（审批）
```

### 4.2 Effort 档位：最接近意图强度的维度

**文件**：`src/utils/effort.ts`

```typescript
export const EFFORT_LEVELS = ['low', 'medium', 'high', 'max'] as const
export type EffortLevel = 'low' | 'medium' | 'high' | 'max'
export type EffortValue = EffortLevel | number

export function resolveAppliedEffort(model, appStateEffortValue): EffortValue | undefined {
  const envOverride = getEffortEnvOverride()
  if (envOverride === null) return undefined
  const resolved = envOverride ?? appStateEffortValue ?? getDefaultEffortForModel(model)
  if (resolved === 'max' && !modelSupportsMaxEffort(model)) return 'high'
  return resolved
}
```

**优先级链**：`env CLAUDE_CODE_EFFORT_LEVEL → appState.effortValue → model default`

**Agent 级覆盖**（`src/tools/AgentTool/runAgent.ts`）：

```typescript
const effortValue =
  agentDefinition.effort !== undefined
    ? agentDefinition.effort      // Agent 定义可覆盖
    : state.effortValue
```

**Effort 描述**：
- `low`：Quick, straightforward implementation with minimal overhead
- `medium`：Balanced approach with standard implementation and testing
- `high`：Comprehensive implementation with extensive testing and documentation
- `max`：Maximum capability with deepest reasoning (Opus 4.6 only)

### 4.3 Plan Mode V2：5 阶段工作流

**文件**：`src/utils/messages.ts`（`getPlanModeV2Instructions`）

```
Phase 1: Initial Understanding
  - 仅用 Explore Agent
  - 并行 1~3 个 Explore Agent 探索代码库

Phase 2: Design
  - 用 Plan Agent（可并行 1~3 个）
  - 提供 Phase 1 背景 + 需求约束

Phase 3: Review
  - 审 plan，对齐用户意图
  - AskUserQuestion 澄清

Phase 4: Final Plan（写 plan 文件）
  - 4 个实验臂：control/trim/cut/cap（pewter_ledger 实验）
  - cap 臂：硬限 40 行，禁止 prose

Phase 5: ExitPlanMode
  - 必须调用 ExitPlanModeV2Tool
```

**Plan 文件**（`src/utils/plans.ts`）：
- 路径：`~/.claude/plans/{slug}.md`（可配置 `plansDirectory`）
- `getPlanSlug`：惰性生成唯一 word slug
- `getPlanFilePath(agentId?)`：子 Agent 用 `{slug}-agent-{agentId}.md`

### 4.4 EnterPlanMode / ExitPlanMode 工具

**文件**：`src/tools/EnterPlanModeTool/EnterPlanModeTool.ts`

```typescript
export const EnterPlanModeTool: Tool<InputSchema, Output> = buildTool({
  name: 'EnterPlanMode',
  async call(_input, context) {
    handlePlanModeTransition(appState.toolPermissionContext.mode, 'plan')
    context.setAppState(prev => ({
      ...prev,
      toolPermissionContext: applyPermissionUpdate(
        prepareContextForPlanMode(prev.toolPermissionContext),
        { type: 'setMode', mode: 'plan', destination: 'session' },
      ),
    }))
    return { data: { message: 'Entered plan mode...' } }
  },
})
```

**文件**：`src/tools/ExitPlanModeTool/ExitPlanModeV2Tool.ts`

```typescript
export const ExitPlanModeV2Tool: Tool<InputSchema, Output> = buildTool({
  name: 'ExitPlanModeV2',
  async validateInput(_input, { getAppState, options }) {
    // 校验 plan 存在 + 非空
  },
})
```

### 4.5 对 laew 的启示

1. **Effort 档位**：laew 的三档（simple/medium/hard）最接近 Effort，但 Effort 是**连续强度**而非离散分类。可借鉴「Agent 级 effort 覆盖」。
2. **5 阶段 Plan**：laew 的 Plan Agent 是单步生成，可借鉴「理解→设计→评审→定稿→审批」多阶段。
3. **plan 文件实验臂**：laew 的 `validate_plan_markdown` 可借鉴 A/B 实验不同 plan 结构。
4. **Explore Agent 并行**：laew 的 Plan Agent 是单 Agent，可借鉴并行多 Agent 探索。

---

## 5. AtomCode：TeamDifficulty 二档 + PlanModeGate 中间件

### 5.1 架构总览

AtomCode 无显式 Goal 模型，靠两套机制：
- **TeamDifficulty 二档**：任务难度 → 模型路由（fast vs capable）
- **PlanModeGate 中间件**：Plan 模式只读门控

```
用户输入
    │
    ├─ TeamTool.delegate ──► 多角色子 Agent（14 角色）
    │                          │
    │                          └─ TeamDifficulty → 模型路由
    │                              Simple → fast_cell
    │                              Hard   → capable_cell
    │
    └─ PlanModeGate 中间件 ──► 只读探索（Risky 工具硬阻塞）
```

### 5.2 TeamDifficulty 二档

**文件**：`crates/atomcode-capabilities/src/team.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamDifficulty {
    Simple,
    Hard,
}

pub struct TeamRoleProfile {
    pub id: TeamRoleId,
    pub display_name: &'static str,
    pub permission: TeamPermission,  // Explore | Worker
    pub difficulty: TeamDifficulty,
    pub persona: &'static str,
    pub when_to_use: &'static str,
}
```

**14 内置角色**（`BUILT_IN_ROLES`）：

| 角色 | 权限 | 难度 |
|------|------|------|
| Planner | Explore | Hard |
| Architect | Explore | Hard |
| Explorer | Explore | Simple |
| Implementer | Worker | Hard |
| Rust | Worker | Hard |
| TuiUx | Worker | Hard |
| Reviewer | Explore | Hard |
| Tester | Worker | Hard |
| ... | ... | ... |

**模型路由**（`crates/atomcode-coding/src/parts.rs`）：

```rust
let providers = Arc::new(move |difficulty| {
    let tier = match difficulty {
        TeamDifficulty::Simple => fast_cell.as_ref(),
        TeamDifficulty::Hard => capable_cell.as_ref(),
    };
    tier.and_then(|cell| cell.get()).unwrap_or_else(|| provider_slot.clone())
})
```

### 5.3 TeamTool：目标拆分

**文件**：`crates/atomcode-coding/src/team/tool.rs`

```rust
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum TeamArgs {
    Delegate { tasks: Vec<DelegateTask> },
    Status { run_id: Option<String> },
    Wait { run_id: String, timeout_secs: u64 },
    Result { run_id: String },
    Stop { run_id: String },
}

struct DelegateTask {
    description: String,
    prompt: String,
    role: TeamRoleId,
    scope: Vec<String>,
}
```

**执行**：`manager.delegate(tasks, jobs, models)` → 并行子 Agent，每个角色带独立 persona + 模型。

### 5.4 PlanModeGate 中间件

**文件**：`crates/atomcode-coding/src/plan_mode.rs`

```rust
pub struct PlanModeGate {
    active: Arc<AtomicBool>,
    mcp_grants: Arc<dyn PermissionStore>,
}

#[async_trait]
impl ToolMiddleware for PlanModeGate {
    async fn before(&self, call, tool, rt) -> BeforeOutcome {
        if !self.active.load(Ordering::Relaxed) { return BeforeOutcome::Proceed; }
        if call.name.starts_with("mcp__") {
            if tool.read_only_hint() { return BeforeOutcome::Proceed; }
            // 其他 MCP 工具 → prompt（不硬阻塞）
            return match rt.request(APPROVAL_KIND, payload).await { ... }
        }
        // 内置 Risky 工具 → 硬阻塞
        if tool.risk(&call.arguments) == RiskLevel::Risky {
            return Self::blocked(&call.name);
        }
        BeforeOutcome::Proceed
    }
}
```

**PlanModeReminderHook**：注入 `PLAN_MODE_REMINDER_BODY` 到尾消息：

```
PLAN MODE is active. Do NOT create, edit, or delete files, and do NOT write out the
implementation — not even as code blocks in your reply. Investigate with read-only tools,
then present a concise implementation plan and STOP, waiting for the user to review and
switch to build mode.
```

### 5.5 对 laew 的启示

1. **Difficulty → 模型路由**：laew 的三档可借鉴「不同档用不同模型/provider」。
2. **PlanModeGate 中间件**：laew 的 Plan Agent 是独立 Agent，可借鉴「中间件门控」做更轻量的 Plan 模式。
3. **多角色并行**：laew 的 Plan Agent 是单 Agent，可借鉴 14 角色并行探索。

---

## 6. OpenCode：plan/build 双 Agent 切换 + plan_exit 工具

### 6.1 架构总览

OpenCode 无显式 Goal 模型，靠 **Agent 切换** 实现规划/执行分离：

```
用户输入
    │
    ├─ plan_enter 提示 ──► 切到 plan Agent（只读）
    │                        │
    │                        └─ 写 .opencode/plans/*.md
    │
    └─ plan_exit ──► 切到 build Agent（执行）
```

### 6.2 plan/build 双 Agent

**文件**：`packages/opencode/src/agent/agent.ts`

```typescript
const agents: Record<string, Info> = {
  build: {
    name: "build",
    description: "The default agent. Executes tools based on configured permissions.",
    permission: Permission.merge(defaults, { question: "allow", plan_enter: "allow" }, user),
    mode: "primary",
    native: true,
  },
  plan: {
    name: "plan",
    description: "Plan mode. Disallows all edit tools.",
    permission: Permission.merge(defaults, {
      question: "allow",
      plan_exit: "allow",
      task: { general: "deny" },
      edit: {
        "*": "deny",
        [path.join(".opencode", "plans", "*.md")]: "allow",  // 仅可写 plan 文件
      },
    }, user),
    mode: "primary",
    native: true,
  },
  // general, explore, compaction...
}
```

**关键**：plan Agent 的 `edit` 权限是 `*:deny` + `plans/*.md:allow`，与 laew Plan Agent 的「Write 仅限 plans/目录」异曲同工。

### 6.3 plan_exit 工具

**文件**：`packages/opencode/src/tool/plan.ts`

```typescript
export const PlanExitTool = Tool.define("plan_exit", Effect.gen(function* () {
  // 1. 问用户：切到 build Agent？
  const answers = yield* question.ask({
    questions: [{
      question: `Plan at ${plan} is complete. Would you like to switch to the build agent and start implementing?`,
      options: [
        { label: "Yes", description: "Switch to build agent and start implementing the plan" },
        { label: "No", description: "Stay with plan agent to continue refining the plan" },
      ],
    }],
  })
  // 2. Yes → 创建 synthetic user message，agent: "build"
  const msg: SessionV1.User = {
    id: MessageID.ascending(),
    sessionID: ctx.sessionID,
    role: "user",
    agent: "build",    // ← 切到 build Agent
    model,
  }
  yield* session.updateMessage(msg)
  yield* session.updatePart({
    type: "text",
    text: `The plan at ${plan} has been approved, you can now edit files. Execute the plan`,
    synthetic: true,
  })
}))
```

**plan 文件路径**（`packages/opencode/src/session/session.ts`）：

```typescript
export function plan(input: { slug: string; time: { created: number } }, instance: InstanceContext) {
  const base = instance.project.vcs
    ? path.join(instance.worktree, ".opencode", "plans")
    : path.join(Global.Path.data, "plans")
  return path.join(base, [input.time.created, input.slug].join("-") + ".md")
}
```

### 6.4 对 laew 的启示

1. **Agent 切换**：laew 的 Plan Agent → Main-Work → SubAgent 是编排器驱动，可借鉴「Agent 身份切换」做更轻量切换。
2. **plan_exit 审批**：laew 的 QC 校验可借鉴「用户审批 + 反馈」闭环。
3. **synthetic user message**：laew 可借鉴 synthetic 消息做 Agent 间handoff。

---

## 7. Pi：steer/followUp 队列 + 扩展式 Plan Mode

### 7.1 架构总览

Pi 无显式 Goal 模型，靠 **steer/followUp 队列** 做运行时目标修正，靠**扩展式 Plan Mode** 做规划：

```
用户输入
    │
    ├─ steer(message) ──► 注入到下一 assistant turn 前
    │
    ├─ followUp(message) ──► 注入到 agent 本应停止后
    │
    └─ Plan Mode 扩展 ──► 只读探索 + 编号步骤提取 + [DONE:n] 标记
```

### 7.2 steer/followUp 队列

**文件**：`packages/agent/src/agent.ts`

```typescript
export class Agent {
  private readonly steeringQueue: PendingMessageQueue;
  private readonly followUpQueue: PendingMessageQueue;

  steer(message: AgentMessage): void {
    this.steeringQueue.enqueue(message);
  }

  followUp(message: AgentMessage): void {
    this.followUpQueue.enqueue(message);
  }
}

class PendingMessageQueue {
  public mode: QueueMode;  // "one-at-a-time" | "all"

  drain(): AgentMessage[] {
    if (this.mode === "all") {
      const drained = this.messages.slice();
      this.messages = [];
      return drained;
    }
    // one-at-a-time：只取第一条
    const first = this.messages[0];
    this.messages = this.messages.slice(1);
    return [first];
  }
}
```

**队列模式**：
- `one-at-a-time`：每 turn 只注入一条（默认）
- `all`：每 turn 注入全部

**prepareNextTurnWithContext**：每 turn 间可修改 context + model + reasoning。

### 7.3 扩展式 Plan Mode

**文件**：`packages/coding-agent/examples/extensions/plan-mode/index.ts`

```typescript
export default function planModeExtension(pi: ExtensionAPI): void {
  // 1. /plan 命令切换
  pi.registerCommand("plan", { handler: async (_args, ctx) => togglePlanMode(ctx) })

  // 2. Plan 模式：禁用 edit/write，bash 限 allowlist
  function enablePlanModeTools(): void {
    pi.setActiveTools(getPlanModeTools(toolsBeforePlanMode))
  }

  // 3. 注入 plan-mode-context
  pi.on("before_agent_start", async () => {
    if (planModeEnabled) {
      return {
        message: {
          customType: "plan-mode-context",
          content: `[PLAN MODE ACTIVE]
You are in plan mode - a read-only exploration mode for safe code analysis.
...
Create a detailed numbered plan under a "Plan:" header:
Plan:
1. First step description
2. Second step description
...`,
        },
      }
    }
    if (executionMode && todoItems.length > 0) {
      return {
        message: {
          customType: "plan-execution-context",
          content: `[EXECUTING PLAN - Full tool access enabled]
Remaining steps:
${todoList}
Execute each step in order.
After completing a step, include a [DONE:n] tag in your response.`,
        },
      }
    }
  })

  // 4. turn_end 检测 [DONE:n] 标记
  pi.on("turn_end", async (event, ctx) => {
    if (markCompletedSteps(text, todoItems) > 0) {
      updateStatus(ctx)
    }
  })
}
```

**Plan 模式双阶段**：
1. **Plan 模式**（只读）→ 生成编号步骤
2. **Execution 模式**（全工具）→ 按步骤执行，`[DONE:n]` 标记完成

### 7.4 对 laew 的启示

1. **steer 队列**：laew 的失败回流可借鉴「运行时注入修正指令」。
2. **[DONE:n] 标记**：laew 的 WorkFlow 步骤可借鉴「步骤完成标记」做进度追踪。
3. **双阶段 Plan**：laew 的 Plan Agent → Main-Work 可借鉴「Plan/Execution 模式切换」。

---

## 8. 设计模式提炼

### 8.1 模式一：显式 Goal 状态机（DeepSeek-Harness / OpenClaw）

**描述**：将「目标」建模为显式、持久、可转移的状态机，而非隐式意图。

**共同要素**：
- 状态集合：`active | paused | blocked | complete`（+ `budget_limited`）
- 转移规则：CAS revision + 合法转移校验
- 持久化：事件日志（deepseek）/ SQLite（openclaw）
- 归因：操作来源（人/模型）

**差异**：
- DeepSeek：whole-value 事件 + 纯折叠，无独立存储
- OpenClaw：操作收据 + 防重放 + token 预算

**适用场景**：长程目标、跨 turn/跨 session 持续追踪。

### 8.2 模式二：Plan 协作态（DeepSeek-Harness / Claude Code / OpenCode）

**描述**：规划是**独立阶段**，产出 plan 文件，审批后执行。

**共同要素**：
- Plan 文件落盘（Markdown）
- 审批/反馈闭环
- Plan 阶段限制写工具

**差异**：
- DeepSeek：Plan Mode 是协作态（不限制工具），靠 guidance section
- Claude Code：Plan Mode V2 是 5 阶段工作流，Explore/Plan Agent 并行
- OpenCode：plan/build Agent 切换，synthetic message handoff

### 8.3 模式三：Effort/Difficulty → 模型路由（Claude Code / AtomCode）

**描述**：任务难度/强度 → 不同模型/provider 路由。

**差异**：
- Claude Code：Effort 是连续强度（low/medium/high/max），影响思考时长
- AtomCode：Difficulty 是离散二档（Simple/Hard），影响 fast/capable 模型

### 8.4 模式四：自动续 round（DeepSeek-Harness）

**描述**：Goal active + armed → 驱动器自动注入 goal_round 消息，驱动多轮执行。

**独特**：6 个项目里唯一实现「目标驱动的多轮自动执行」。

### 8.5 模式五：运行时 steer（Pi）

**描述**：通过队列注入修正指令，运行时改变目标/方向。

**独特**：6 个项目里唯一实现「运行时目标修正」。

### 8.6 模式六：多角色并行拆分（AtomCode）

**描述**：父目标拆子任务，每个子任务委派一个角色（persona + 模型 + 权限）。

**独特**：14 内置角色，每个角色带独立 persona + 难度 → 模型路由。

### 8.7 模式七：操作收据 + 防重放（OpenClaw）

**描述**：每个 Goal 操作带唯一 operationId + 指纹 + 有效期，SQLite 收据防重放。

**独特**：6 个项目里唯一实现「操作幂等 + 防重放」。

### 8.8 模式八：失败回流 + 用户建议（laew 独有）

**描述**：Yolo 分类输出 `user_suggestion_if_fail`，失败时给备选建议。

**独特**：6 个项目里唯一在分类阶段就预置失败建议。

### 8.9 设计模式对比表

| 模式 | DeepSeek | OpenClaw | Claude | AtomCode | OpenCode | Pi | laew |
|------|----------|----------|--------|----------|----------|-----|------|
| 显式 Goal 状态机 | ✅ 4 态 | ✅ 6 态 | ✗ | ✗ | ✗ | ✗ | ✗（仅分类） |
| Plan 协作态 | ✅ 审批 | ✗ | ✅ 5 阶段 | ✗ | ✅ 切换 | ✗ | ✅ 单步 |
| Effort→模型路由 | ✗ | ✗ | ✅ | ✅ | ✗ | ✗ | ✗ |
| 自动续 round | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 运行时 steer | ✗ | ✗ | ✗ | ✗ | ✗ | ✅ | ✗ |
| 多角色并行 | ✗ | ✗ | ✗ | ✅ | ✗ | ✗ | ✗ |
| 操作收据 | ✗ | ✅ | ✗ | ✗ | ✗ | ✗ | ✗ |
| 失败回流建议 | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✅ |

---

## 9. 对 laew 的综合建议

### 9.1 当前差距分析

| 维度 | laew 现状 | 业界最佳实践 | 差距 |
|------|-----------|--------------|------|
| **目标模型** | `TaskClassification` 单次分类，无持久生命周期 | DeepSeek `GoalPhase` 4 态 + OpenClaw 6 态 | 高 |
| **意图识别** | 目的→目标→意图三步（独有优势） | Claude Effort 最接近 | 持平 |
| **目标持久化** | 无（分类后丢弃） | DeepSeek 事件日志 + OpenClaw SQLite | 高 |
| **自动续 round** | 无（单次流水线） | DeepSeek `goal-round-driver` | 高 |
| **Plan 生成** | 单步 Markdown 五段 | Claude 5 阶段 + 并行 Agent | 中 |
| **Plan 审批** | QC 校验（无用户审批） | DeepSeek/Claude 用户审批 | 中 |
| **目标拆分** | `decomposition_plan: Vec<String>` | AtomCode 14 角色并行 | 中 |
| **失败回流** | `user_suggestion_if_fail`（独有） | 无 | 领先 |

### 9.2 P0 建议：引入轻量 Goal 状态机

**目标**：让 hard 任务的目标可跨 turn 持续追踪。

**方案**：
1. 在 `src/agent/` 新增 `goal.rs`，定义 `GoalPhase { Active, Paused, Blocked, Complete }`
2. `TaskClassification` 新增 `goal_id: Option<String>`，hard 任务自动 create_goal
3. Plan Agent 生成前 get_goal，生成后 update_goal
4. 失败回流时：N 次连续失败 → block（借鉴 DeepSeek `blockedAfterConsecutiveRounds`）

**落地位置**：`src/agent/goal.rs` + 扩展 `yolo.rs` 的 `TaskClassification`

### 9.3 P1 建议：Plan 多阶段化

**目标**：单步 Plan → 多阶段 Plan。

**方案**：
1. Plan Agent 拆为 3 阶段：Explore（只读探索）→ Design（方案）→ Review（QC）
2. 借鉴 Claude Code 5 阶段：理解→设计→评审→定稿→审批
3. Plan 文件落盘后增加用户审批环节（借鉴 DeepSeek `exit_plan_mode`）

**落地位置**：`src/agent/plan.rs` + `src/agent/orchestrator.rs::run_hard`

### 9.4 P1 建议：Difficulty → 模型路由

**目标**：不同难度档用不同 provider/model。

**方案**：
1. `TaskLevel` 映射到 provider 配置（simple→fast, hard→capable）
2. 借鉴 AtomCode `TeamProviderFactory`：`Fn(TeamDifficulty) -> Arc<dyn LlmProvider>`
3. 在 `MultiAgentOrchestrator` 构造时按难度选 provider

**落地位置**：`src/agent/orchestrator.rs` + `src/llm/`

### 9.5 P2 建议：运行时 steer

**目标**：执行中可注入修正指令。

**方案**：
1. 借鉴 Pi `steeringQueue`：`Agent` 增加 `steer(message)` API
2. Orchestrator 暴露 `steer(session_id, message)` 接口
3. TUI 支持 `/steer <message>` 命令

**落地位置**：`src/agent/orchestrator.rs` + `src/tui/`

### 9.6 P2 建议：操作收据

**目标**：Plan 生成等关键操作幂等 + 防重放。

**方案**：
1. 借鉴 OpenClaw `SessionGoalOperation`：关键操作带 operationId + 指纹
2. `agent_memory` 表增加 `operation_id` 字段
3. 重放时先查收据

**落地位置**：`src/config/` + `src/agent/memory.rs`

### 9.7 不建议照搬

1. **DeepSeek 自动续 round**：laew 是 CLI 工具，非服务化场景，自动续 round 场景有限。
2. **OpenClaw token 预算**：laew 的 Plan Agent 是单步，token 预算收益有限。
3. **AtomCode 14 角色**：laew 当前 6 角色已够用，14 角色过度设计。

---

## 附录 A：各项目 goal/plan 代码片段速查

### A.1 DeepSeek-Harness

| 关注点 | 文件 | 关键符号 |
|--------|------|----------|
| Goal 状态机 | `packages/goal/goal/src/types.ts` | `GoalPhase`, `GoalSnapshot`, `GoalView` |
| Goal 转移 | `packages/goal/goal/src/fold.ts` | `validateSnapshotChange`, `foldGoal` |
| Goal 事件 | `packages/goal/goal/src/domain.ts` | `GoalOperation`, `GoalSnapshotChangeMeta` |
| 模型工具 | `packages/goal/tool-goal/src/index.ts` | `get_goal`, `create_goal`, `update_goal` |
| 权限 | `packages/goal/tool-goal/src/authority.ts` | `completionAuthority`, `requireDirectHuman` |
| 收尾提示 | `packages/goal/tool-goal/src/wrapup.ts` | `renderWrapupContext` |
| 人命令 | `packages/goal/command-goal/src/index.ts` | `parseGoalCommand`, `executeGoalCommand` |
| 自动续 round | `packages/goal/goal-round-driver/src/index.ts` | `drive`, `requestDrive` |
| 续 round 提示 | `packages/goal/goal-round-driver/src/prompt.ts` | `renderGoalRoundPrompt` |
| Plan Mode | `packages/plan/plan-mode/src/index.ts` | `PlanModeController`, `exit_plan_mode` |
| Plan 折叠 | `packages/plan/plan-mode/src/index.ts` | `foldPlanMode`, `PlanProjection` |
| 系统提示序 | `packages/core/system-prompt/src/index.ts` | `FIRST_PARTY_SECTION_ORDER` |

### A.2 OpenClaw

| 关注点 | 文件 | 关键符号 |
|--------|------|----------|
| Goal 状态 | `src/config/sessions/goals-transitions.ts` | `buildCreatedSessionGoal`, `buildUpdatedSessionGoalStatus` |
| Goal 操作 | `src/config/sessions/goals-operations.ts` | `mutateSessionGoal`, `applySessionGoalOperation` |
| 操作类型 | `src/config/sessions/goals-operations.types.ts` | `SessionGoalOperation`, `SessionGoalOperationResult` |
| 模型工具 | `src/agents/tools/goal-tools.ts` | `createGetGoalTool`, `createCreateGoalTool`, `createUpdateGoalTool` |
| 压缩规划 | `src/agents/compaction-planning.ts` | `buildStageSplitPlan`, `buildSummaryChunks` |
| 压缩 worker | `src/agents/compaction-planning-worker.ts` | `buildStageSplitPlanWithWorker` |

### A.3 Claude Code

| 关注点 | 文件 | 关键符号 |
|--------|------|----------|
| Effort 档位 | `src/utils/effort.ts` | `EFFORT_LEVELS`, `resolveAppliedEffort`, `getDefaultEffortForModel` |
| EnterPlanMode | `src/tools/EnterPlanModeTool/EnterPlanModeTool.ts` | `EnterPlanModeTool` |
| Enter 提示 | `src/tools/EnterPlanModeTool/prompt.ts` | `getEnterPlanModeToolPrompt` |
| ExitPlanMode | `src/tools/ExitPlanModeTool/ExitPlanModeV2Tool.ts` | `ExitPlanModeV2Tool` |
| Plan 5 阶段 | `src/utils/messages.ts` | `getPlanModeV2Instructions`, `getPlanPhase4Section` |
| Plan 文件 | `src/utils/plans.ts` | `getPlanSlug`, `getPlanFilePath`, `getPlansDirectory` |
| Plan V2 配置 | `src/utils/planModeV2.ts` | `getPlanModeV2AgentCount`, `isPlanModeInterviewPhaseEnabled` |
| Agent effort | `src/tools/AgentTool/runAgent.ts` | `effortValue = agentDefinition.effort ?? state.effortValue` |

### A.4 AtomCode

| 关注点 | 文件 | 关键符号 |
|--------|------|----------|
| 难度枚举 | `crates/atomcode-capabilities/src/team.rs` | `TeamDifficulty::Simple/Hard` |
| 角色配置 | `crates/atomcode-capabilities/src/team.rs` | `TeamRoleProfile`, `BUILT_IN_ROLES` |
| 任务规格 | `crates/atomcode-capabilities/src/team.rs` | `TeamTaskSpec` |
| Team 工具 | `crates/atomcode-coding/src/team/tool.rs` | `TeamTool`, `TeamArgs::Delegate` |
| PlanMode 门控 | `crates/atomcode-coding/src/plan_mode.rs` | `PlanModeGate`, `PlanModeReminderHook` |
| 模型路由 | `crates/atomcode-coding/src/parts.rs` | `providers = Arc::new(move \|difficulty\| ...)` |

### A.5 OpenCode

| 关注点 | 文件 | 关键符号 |
|--------|------|----------|
| Agent 定义 | `packages/opencode/src/agent/agent.ts` | `agents.plan`, `agents.build` |
| plan_exit | `packages/opencode/src/tool/plan.ts` | `PlanExitTool` |
| plan 路径 | `packages/opencode/src/session/session.ts` | `plan(input, instance)` |
| plan 提示 | `packages/opencode/src/tool/plan-enter.txt`, `plan-exit.txt` | 工具描述文本 |

### A.6 Pi

| 关注点 | 文件 | 关键符号 |
|--------|------|----------|
| Agent 类 | `packages/agent/src/agent.ts` | `steer()`, `followUp()`, `steeringQueue` |
| 队列 | `packages/agent/src/agent.ts` | `PendingMessageQueue`, `QueueMode` |
| Agent 循环 | `packages/agent/src/agent-loop.ts` | `runLoop`, `prepareNextTurn` |
| Plan 扩展 | `packages/coding-agent/examples/extensions/plan-mode/index.ts` | `planModeExtension`, `[DONE:n]` |

---

## 附录 B：laew 当前 Yolo/Plan 现状速览

### B.1 Yolo（`src/agent/yolo.rs`）

```rust
pub enum TaskLevel { Simple, Medium, Hard }

pub struct TaskClassification {
    pub task_level: TaskLevel,
    pub purpose: String,           // 三步之一：目的
    pub goal_summary: String,      // 三步之二：目标
    pub intent: String,            // 三步之三：意图
    pub agent_role: Option<AgentRole>,
    pub decomposition_plan: Vec<String>,
    pub direct_answer: Option<String>,
    pub user_suggestion_if_fail: String,  // 失败备选建议
}

pub struct YoloRunner { yolo_agent: Agent }

impl YoloRunner {
    pub async fn classify(&self, context) -> Result<(TaskClassification, String, Usage)>
}
```

**执行流**（`src/agent/orchestrator.rs`）：

```rust
match classification.task_level {
    TaskLevel::Simple => self.run_simple(&classification, session).await,
    TaskLevel::Medium => self.run_medium(&classification, session).await,
    TaskLevel::Hard   => self.run_hard(&classification, session).await,
}
```

### B.2 Plan（`src/agent/plan.rs`）

```rust
pub struct PlanRunner { agent: Agent, db: Arc<Db>, plans_dir: PathBuf }

impl PlanRunner {
    pub async fn generate(
        &self, goal: &str, purpose: &str, intent: &str,
        decomposition: &[String], session_id: &str,
    ) -> Result<PlanOutput>
}

pub fn validate_plan_markdown(content: &str) -> Result<()> {
    let required = ["目标", "WorkFlow 拆解", "关键决策", "风险", "验收总览"];
    ...
}
```

**执行流**（`src/agent/orchestrator.rs::run_hard`）：

```rust
// 1) Plan 生成
let plan_output = self.plan.generate(&c.goal_summary, &c.purpose, &c.intent, &c.decomposition_plan, session.id()).await?;
// 2) Quality 校验 Plan
let qc_plan = self.quality.check_plan(&plan_output.markdown, session.id()).await?;
// 3) Main-Work 解析 Plan → WorkFlow
let plan = self.main_work.parse_plan(&plan_output.path)?;
```

### B.3 当前局限

1. **无持久 Goal**：`TaskClassification` 分类后即丢弃，无跨 turn 生命周期。
2. **Plan 单步生成**：无多阶段、无并行 Agent、无用户审批。
3. **无自动续 round**：Plan→Main→SubAgent 是单次流水线。
4. **无 Difficulty→模型路由**：三档不映射到不同 provider。
5. **无运行时 steer**：执行中无法注入修正指令。

---

## 附录 C：建议实现方案详设

### C.1 轻量 Goal 状态机（P0）

**新增文件**：`src/agent/goal.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalPhase { Active, Paused, Blocked, Complete }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub objective: String,
    pub phase: GoalPhase,
    pub blocked_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub rounds_started: u32,
    pub max_rounds: u32,
}

impl Goal {
    pub fn create(objective: String, max_rounds: u32) -> Self { ... }
    pub fn block(&mut self, reason: String) -> Result<()> { ... }
    pub fn complete(&mut self) -> Result<()> { ... }
    pub fn resume(&mut self) -> Result<()> { ... }
}
```

**扩展 `TaskClassification`**：

```rust
pub struct TaskClassification {
    // ... 现有字段 ...
    pub goal_id: Option<String>,  // hard 任务自动创建
}
```

**扩展 `run_hard`**：

```rust
async fn run_hard(&self, c: &TaskClassification, session: &Session) -> Result<TaskResult, QualityFailure> {
    // 0) 创建/恢复 Goal
    let goal_id = match &c.goal_id {
        Some(id) => id.clone(),
        None => self.db.create_goal(&c.goal_summary, 5)?,
    };
    // 1) Plan 生成（带 goal_id 上下文）
    // 2) Quality 校验
    // 3) Main-Work 解析
    // 4) 执行成功 → goal.complete()
    // 5) 执行失败 → goal.block(reason)，N 次后真正 blocked
}
```

### C.2 Plan 多阶段化（P1）

**改造 `src/agent/plan.rs`**：

```rust
pub enum PlanStage { Explore, Design, Review }

pub struct PlanRunner { ... }

impl PlanRunner {
    pub async fn generate_multistage(&self, goal: &Goal, session_id: &str) -> Result<PlanOutput> {
        // Stage 1: Explore（只读探索，可选）
        // Stage 2: Design（生成方案）
        // Stage 3: Review（QC 校验）
    }
}
```

### C.3 Difficulty → 模型路由（P1）

**新增**：`src/llm/routing.rs`

```rust
pub fn select_provider(difficulty: TaskLevel, config: &LlmConfig) -> Arc<dyn LlmClient> {
    match difficulty {
        TaskLevel::Simple => config.fast_provider.clone(),
        TaskLevel::Medium => config.default_provider.clone(),
        TaskLevel::Hard   => config.capable_provider.clone(),
    }
}
```

### C.4 运行时 steer（P2）

**扩展 `src/agent/orchestrator.rs`**：

```rust
pub struct MultiAgentOrchestrator {
    // ... 现有字段 ...
    steering_queues: HashMap<String, VecDeque<String>>,
}

impl MultiAgentOrchestrator {
    pub fn steer(&mut self, session_id: &str, message: String) {
        self.steering_queues.entry(session_id.to_string()).or_default().push_back(message);
    }
}
```

**TUI 命令**：`/steer <message>`

### C.5 实施路线图

| 阶段 | 内容 | 预估工作量 |
|------|------|-----------|
| P0-a | `goal.rs` Goal 状态机 + 单元测试 | 1 天 |
| P0-b | `TaskClassification.goal_id` + `run_hard` 集成 | 0.5 天 |
| P0-c | `agent_memory` 表扩展 goal 字段 | 0.5 天 |
| P1-a | Plan 多阶段化（Explore/Design/Review） | 2 天 |
| P1-b | Difficulty→模型路由 | 1 天 |
| P2-a | 运行时 steer 队列 | 1 天 |
| P2-b | 操作收据（可选） | 1 天 |

---

> **结语**：6 个外部项目在「目标/意图识别与规划」维度呈现明显分层——DeepSeek-Harness 和 OpenClaw 走「显式 Goal 状态机」路线，Claude Code 走「Effort + Plan Mode」路线，AtomCode 走「Difficulty + 多角色」路线，OpenCode/Pi 走「轻量 Plan 模式」路线。laew 当前的「目的→目标→意图三步分析 + 三档分类」在**意图识别**维度有独有优势，但在**目标持久化、生命周期管理、自动续 round** 三个维度差距最大。建议按 P0→P1→P2 顺序补齐，优先引入轻量 Goal 状态机，再逐步做多阶段 Plan 和 Difficulty→模型路由。
