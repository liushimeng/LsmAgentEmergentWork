# 专题-第六轮-Goal 状态机与任务生命周期深度对比

> **范围**: atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / jiuwenswarm 七个项目 **Goal / Task / Plan 状态机与生命周期管理** 的逐行源码深度对比
> **专题定位**: laew 当前完全无 Goal 状态机（CLAUDE.md / AGENTS.md 注明「Yolo 三步 + Plan Agent 输出 `plans/{session_id}-{seq}.md` 是非状态机方式」），这是 laew 多 Agent 协作从「过程式」走向「事务式」的最大缺口；本文档为 P0 路线图提供完整借鉴蓝图
> **数据来源**: 7 个项目仓库实际源码 + 30+ 文件 ~6.5k 行精读 + 7 份相关设计文档
> **关联文档**: `专题-第五轮-中断取消与后台任务深度对比.md`（后台任务生命周期），`专题-第六轮-Skill系统深度对比.md`（skill 内的 phase / task），`专题-任务拆解与分类深度分析.md`（Yolo 三档分类）

---

## 目录

1. [Goal 状态机在大模型 Agent 中的位置](#1-goal-状态机在大模型-agent-中的位置)
2. [atomcode 的 Goal 状态机（Pursuing / Paused / PausedAtCap / Satisfied / Ended + 双 cap）](#2-atomcode-的-goal-状态机pursuing--paused--pausedatcap--satisfied--ended--双-cap)
3. [claudecode 的 TodoWrite 状态机（pending / in_progress / completed + 全文替换）](#3-claudecode-的-todowrite-状态机pending--in_progress--completed--全文替换)
4. [deepseek-harness 的 Goal 域（4 态 active/paused/blocked/complete + armed/disarmed 分离 + CAS 事件溯源 + maxGoalRounds=256）](#4-deepseek-harness-的-goal-域4-态-activepausedblockedcomplete--armeddisarmed-分离--cas-事件溯源--maxgoalrounds256)
5. [openclaw 的 SessionGoal 6 态（active/paused/blocked/usage_limited/budget_limited/complete + token 预算）](#5-openclaw-的-sessiongoal-6-态activepausedblockedusage_limitedbudget_limitedcomplete--token-预算)
6. [opencode 的 plan/build 双 Agent + plan_exit（read-only 计划态 + build 切换）](#6-opencode-的-planbuild-双-agent--plan_exitre-read-only-计划态--build-切换)
7. [pi 的 steer/followUp 队列 + 扩展式 Plan Mode（3 态 enabled/executing + 持久化 + [DONE:n] 标记）](#7-pi-的-steerfollowup-队列--扩展式-plan-mode3-态-enabledexecuting--持久化--donen-标记)
8. [jiuwenswarm 的 SwarmFlow DAG（WORKFLOW_* 6 态 + Phase/Agent 4 层折叠 + journal resume）](#8-jiuwenswarm-的-swarmflow-dagworkflow_-6-态--phaseagent-4-层折叠--journal-resume)
9. [七项目 Goal/Task/Plan 状态机综合对比维度表](#9-七项目-goaltaskplan-状态机综合对比维度表)
10. [七项目状态机 ASCII 转换图](#10-七项目状态机-ascii-转换图)
11. [laew 现状与 P0 借鉴路线图（事务式 Goal 状态机迁移）](#11-laew-现状与-p0-借鉴路线图事务式-goal-状态机迁移)
12. [laew Goal 状态机 MiniPoC 代码蓝图](#12-laew-goal-状态机-minipoc-代码蓝图)

---

## 1. Goal 状态机在大模型 Agent 中的位置

### 1.1 什么是 Goal 状态机

Goal 状态机是「**对用户长任务进行事务性追踪、状态转换、可恢复、可查询**」的运行时机制。它把一次用户输入从「一次 prompt 一次输出」升级为「**一个有生命周期、可暂停、可恢复、可失败的运行时对象**」。

它与「Slash Command」「Plan 文档（Markdown）」「TodoList」的区别：

| 维度 | Slash Command | Plan 文档 | TodoList | **Goal 状态机** |
|------|--------------|----------|---------|----------------|
| 形态 | 命令名 + 参数 | 静态 Markdown | 列表 | **结构化 + 持久化运行时对象** |
| 状态 | 无 | 无（一次性产物） | `pending/in_progress/completed` | **多态 + 转移 + 持久** |
| 持久化 | 无 | 文件 | 文件（claudecode 派生）/ 内存（atomcode 派生） | **SQLite / SQLite-style / 内存 + 事件日志** |
| 恢复 | 无 | 无 | 无（靠文件 reduce） | **CAS 事件溯源 / journal / session-resume** |
| 审批 | 无 | 用户读 | 无 | **plan_exit / exit_plan_mode / 类 build 切换** |
| 终止 | Ctrl-C | Ctrl-C | Ctrl-C | **明确的 terminal 态（Met/Stopped/Cancelled/Failed/Satisfied/Ended）** |
| 自动机 | 无 | 无 | 无 | **armed/disarmed 自动续轮（deepseek goal-round-driver）** |
| 嵌套 | 无 | 无 | 子任务可嵌套 | **phase → agent → activity 4 层（jiuwenswarm）** |
| 资源预算 | 无 | 无 | 无 | **max_rounds / token_budget / time_cap / queue_lane** |

### 1.2 Goal 状态机的设计哲学分类

| 类别 | 代表项目 | 核心机制 | 适用场景 |
|------|---------|---------|---------|
| **Goal-Round 驱动式** | atomcode / deepseek-harness | 模型自报 status / 由 round-driver 自动排队下一轮 | 长时间跨轮推理任务 |
| **预算围栏式** | openclaw | token 预算 / 工具使用量自动触发 budget_limited | 高成本对话任务 |
| **计划审批式** | opencode / claudecode (Plan Mode V2) | plan-then-execute 严格两阶段 | 用户强控制场景 |
| **轻量派单式** | pi | steer/followUp 队列 + plan extension | IDE/编辑器嵌入场景 |
| **DAG 编排式** | jiuwenswarm (SwarmFlow) | phase/agent/activity 4 层 + journal resume | 多 Agent 流水线 |
| **过程式（无状态机）** | laew | Yolo 三步分类 + 一次性 Plan Markdown | 当前 laew 的非状态机方式 |

### 1.3 为什么 laew 需要 Goal 状态机

laew 当前的痛点（CLAUDE.md 「多 Agent 架构重构」方案记录）：

1. **Yolo 三步分类（simple/medium/hard）是无状态分类**，不区分「用户当前任务在做什么」「做完了没有」「能不能跨 Session 恢复」。
2. **Plan Agent 输出 `plans/{session_id}-{seq}.md` 是文件系统级的产物**，不是运行时对象，无法：
   - 查询「plan 当前执行到哪一步」
   - 自动恢复「Plan Agent 中途崩溃」
   - 取消 / 暂停 / 重试「正在执行的 Plan」
3. **失败回流靠用户重新输入**（CLAUDE.md 「失败回流与用户建议」），没有结构化的 retry / escalate / fallback 路径。
4. **SubAgent 完成后由 Quality-Check 质检 + SessionContext 收口**，但是「SubAgent 失败 5 次」「SubAgent 超过预算」等条件没有事务性表达。

Goal 状态机就是填补这四类空白的「运行时事务层」。

---

## 2. atomcode 的 Goal 状态机（Pursuing / Paused / PausedAtCap / Satisfied / Ended + 双 cap）

### 2.1 6 态 GoalPhase 设计

**文件**: `crates/atomcode-coding/src/controllers.rs:69-81`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalPhase {
    Pursuing,        // 正在主动追求目标
    /// 显式被用户暂停;目标仍注册,下次用户提交即恢复;不同于 PausedAtCap,没有耗尽预算。
    Paused,
    PausedAtCap,     // 因轮数/时间上限被暂停
    Satisfied,       // 终端态:目标已达成
    /// 取消/失败/清理路径的终态。UI 行在此态消失。满足不变式:exit paths 上 `active == (phase == Pursuing)`。
    Ended,
}
```

**4 终态区分**:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalTerminal {
    Met,         // 完成
    Stopped,     // 因 cap 停止
    Failed,      // 评估器判为失败
    Cancelled,   // 用户取消
}
```

`Pursuing → {Met → Satisfied, Stopped → Ended, Failed → Ended, Cancelled → Ended}`。

### 2.2 GoalState 控制器（cap_reached 预算判定）

**文件**: `crates/atomcode-coding/src/controllers.rs:99-304`

```rust
#[derive(Debug)]
pub(crate) struct GoalState {
    pub id: u64,
    pub condition: String,
    pub active: bool,
    pub phase: GoalPhase,
    pub terminal: Option<GoalTerminal>,
    pub round: u32,
    started_at: Instant,
    pub last_reason: Option<String>,
    pub tokens_used: u64,
    pub max_rounds: Option<u32>,
    deadline: Option<Instant>,
    pub unproductive: u32,
    pub cancel: CancellationToken,
    max_duration_secs: u64,
    progress_recap: Option<String>,
    recovery_pause: bool,
}
```

**关键设计**：

#### 2.2.1 双 cap（轮数 + 时间）判定

```rust
pub fn cap_reached(&self) -> Option<&'static str> {
    if self.max_rounds.is_some_and(|max| self.round >= max) {
        return Some("round limit");
    }
    if self
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        return Some("time limit");
    }
    None
}
```

`cap_reached` 返回 `None | "round limit" | "time limit"`,明确区分耗尽的是哪种预算,且不假定 evaluator 判了「未达成」(这是关键的设计哲学 — **cap 耗尽 ≠ 失败**)。

#### 2.2.2 不变式:paused 子态 + recovery flag

```rust
pub fn pause_at_cap(&mut self, note: impl Into<String>) {
    self.active = false;
    self.phase = GoalPhase::PausedAtCap;
    self.terminal = Some(GoalTerminal::Stopped);
    self.last_reason = Some(note.into());
    self.recovery_pause = false;  // 普通 cap 不是 recovery
}

pub fn pause_for_recovery(&mut self, note: impl Into<String>) {
    self.active = false;
    self.phase = GoalPhase::PausedAtCap;
    self.terminal = Some(GoalTerminal::Stopped);
    self.last_reason = Some(note.into());
    self.recovery_pause = true;   // 仅 repeated-failure 才设
}
```

两种「PausedAtCap」对外不可见,但 **`recovery_pause` 决定 resume() 是否注入 recovery_context**(bounded recovery context,内含 6000 字符 progress_recap)。这是「失败回流」机制的核心。

#### 2.2.3 resume 双预算刷新

```rust
pub fn resume(&mut self, new_max_rounds: u32) {
    self.cancel = new CancellationToken::new();
    self.active = true;
    self.phase = GoalPhase::Pursuing;
    self.round = 0;
    self.max_rounds = (new_max_rounds != 0).then_some(new_max_rounds);
    self.terminal = None;
    self.last_reason = None;
    self.recovery_pause = false;
    // 关键:重置 unproductive 计数 + 刷新 wall-clock deadline
    self.unproductive = 0;
    self.deadline = (self.max_duration_secs != 0)
        .then(|| Instant::now() + Duration::from_secs(self.max_duration_secs));
}
```

两个 resume 子函数 (`resume()` vs `resume_paused()`)的差异:
- **`resume()`** (cap pause 后调用): **重置 round=0 / 给新预算 / 刷 deadline**
- **`resume_paused()`** (用户显式 pause 后调用): **保留 round / 保留 budget / 不刷 deadline**

### 2.3 evaluate_goal — 评估器子回合

**文件**: `crates/atomcode-coding/src/controllers.rs:384-438`

```rust
const EVALUATOR_SYSTEM_PROMPT: &str = r#"你是严格的 Goal 评估器...
Verdict: yes <原因>
Verdict: no <原因>"#;

pub(crate) async fn evaluate_goal(
    generation: u64,
    controller_id: u64,
    provider: Arc<dyn LlmProvider>,
    condition: String,
    summary: String,
    cancel: CancellationToken,
) -> EvalOutcome {
    // 调用 LLM 解析 Verdict: yes/no,带 30s timeout
    // 返回 GoalResult::Met / NotMet / Inconclusive / Error
}
```

`GoalResult` 是 4 态结果:

```rust
pub(crate) enum GoalResult {
    Met(String),           // Verdict: yes
    NotMet(String),        // Verdict: no
    Inconclusive(String),  // 空响应 / whitespace only(不是 error!)
    Error(String),         // malformed verdict line
}
```

**设计哲学**: 空响应 → **Inconclusive**(不当作 Failure 触发 retry,避免无谓重试)。

### 2.4 Plan Mode (中间件 + 双层防御)

**文件**: `crates/atomcode-coding/src/plan_mode.rs:37-119` + `plan_mode.rs:139-158`

Plan Mode = **Tool 中间件(PlanModeGate)** + **Lifecycle Hook(PlanModeReminderHook)** 双层防御。

#### 2.4.1 PlanModeGate — Tool 中间件硬阻断

```rust
pub struct PlanModeGate {
    active: Arc<AtomicBool>,
    mcp_grants: Arc<dyn PermissionStore>,
}

#[async_trait]
impl ToolMiddleware for PlanModeGate {
    async fn before(&self, call: &mut ToolCall, tool: &Arc<dyn Tool>, rt: &RequestCtx) -> BeforeOutcome {
        if !self.active.load(Ordering::Relaxed) { return BeforeOutcome::Proceed; }

        if call.name.starts_with("mcp__") {
            // MCP read_only_hint:true → 允许(plan 调研需要)
            if tool.read_only_hint() { return BeforeOutcome::Proceed; }
            // MCP mutating / 无注释 → PROMPT(非硬阻断),用户可允许单次或 Always
            // ...
        }
        // built-in Risky 工具 → 硬阻断(deny)
        if tool.risk(&call.arguments) == RiskLevel::Risky {
            return Self::blocked(&call.name);
        }
        BeforeOutcome::Proceed
    }
}
```

**关键设计**:
- **`Arc<AtomicBool>`** 让 driver 可以 live-toggle plan mode,无需 respawn runtime。
- **MCP 工具区别对待**: 读类直接放行,写类 prompt 不硬阻断(Claude Code parity)。
- **always-grant 持久化**:`Arc<dyn PermissionStore>` 由 CodingParts 共享,跨 respawn(model-swap)存活。

#### 2.4.2 PlanModeReminderHook — 提示词注入

```rust
pub struct PlanModeReminderHook {
    active: Arc<AtomicBool>,}

#[async_trait]
impl LifecycleHooks for PlanModeReminderHook {
    async fn pre_request(&self, messages: &mut Vec<Message>, _ctx: &TurnCtx) {
        if self.active.load(Ordering::Relaxed) {
            messages.push(atomcode_capabilities::reminder::synthetic_system_reminder(
                PLAN_MODE_REMINDER_BODY,
            ));
        }
    }
}
```

**Cache-safe 设计**: 提醒注入是 **`pre_request` 阶段**(追加为 ephemeral tail),**不进 system prompt**。这意味着 **OFF↔ON 切换只改变 prefix 之后的字节**,cached prefix 完全不动 → Anthropic prompt cache 仍然有效。

### 2.5 GoalState 不变式测试

**文件**: `crates/atomcode-coding/src/controllers.rs:982-1068`

```rust
#[test]
fn goal_phase_transitions_keep_state_consistent() {
    let mut g = GoalState::new(1, "finish".into(), 300, 0);
    assert_eq!(g.phase, GoalPhase::Pursuing);
    assert!(g.active);

    g.mark_satisfied("all done");
    assert_eq!(g.phase, GoalPhase::Satisfied);
    assert!(!g.active);
    assert_eq!(g.terminal, Some(GoalTerminal::Met));
    // ...
}

#[test]
fn resume_refreshes_deadline_for_time_capped_goal() {
    // falsifying: 把 deadline 强制设到过去,resume() 后必须刷新
    let mut g = GoalState::new(1, "x".into(), 0, 3600);
    g.deadline = Some(Instant::now() - Duration::from_secs(1));
    g.pause_at_cap("已达时间上限");
    assert_eq!(g.cap_reached(), Some("time limit"));  // 之前应报时间耗尽
    g.resume(0);
    assert_eq!(g.cap_reached(), None);  // resume 后不再耗尽
}
```

---

## 3. claudecode 的 TodoWrite 状态机（pending / in_progress / completed + 全文替换）

### 3.1 三态 TodoStatus + 派生式状态

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:14-50`

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}
```

claudecode 的 todo **不是「runtime 状态机」**,而是 **「派生式状态」** — 真正的状态从 transcript 折叠出来(`derive_current_todos`),TodoTool 本身 **完全无状态**。

### 3.2 stateless full-list-replace 协议

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:1-6`

```rust
//! `todowrite` — an AI-driven, full-list-replace task list for the current coding
//! session. STATELESS: the model sends the entire updated list every call;
//! the tool validates + echoes it. Current state is DERIVED from the transcript
//! (last todowrite call), so it persists with the session and survives /resume
//! with zero extra storage. Non-destructive ⇒ always `Safe`.
```

**核心**: 状态 = transcript.fold(last `todowrite` 调用)。**TodoTool 是 echo + validator**,不持有任何状态。

### 3.3 双 schema(plan vs update)

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:366-390`

```rust
fn parameters_schema(&self) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "todos": {  // PLAN / RE-PLAN — 完整列表替换
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                    },
                    "required": ["content", "status"]
                }
            },
            "action": { "type": "string", "enum": ["add", "update"] },  // 增量修改
            "id": { "type": "integer" },
            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
            "content": { "type": "string" }
        }
    })
}
```

**扁平 union 而非 oneOf**: "weaker models handle poorly" — 注释明示。两种调用:
- **`{"todos":[…]}`** = PLAN / RE-PLAN(全文替换)
- **`{"action":"add"|"update", …}`** = 增量修改

### 3.4 不变式强制:exactly one in_progress

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:151-153`

```rust
if in_progress > 1 {
    return Err("todowrite: keep exactly ONE task `in_progress` at a time.".to_string());
}
```

**强制硬约束**: 任何时刻只能有 **恰好 1 个** `in_progress`,否则整个列表拒绝。

### 3.5 增量 update 的 in_progress 自动互斥

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:250-269`

```rust
Some("update") => {
    // ...
    if status == TodoStatus::InProgress {
        // 关键:把其他 in_progress 全部置回 Pending,自动维持"恰好一个"不变量
        for it in list.iter_mut() {
            if it.status == TodoStatus::InProgress {
                it.status = TodoStatus::Pending;
            }
        }
    }
    list[(id - 1) as usize].status = status;
}
```

### 3.6 reduce_todos — 派生态折叠

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:291-306`

```rust
pub fn reduce_todos<'a>(calls: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<TodoItem> {
    let calls: Vec<(&str, &str)> = calls.into_iter()
        .filter(|(n, _)| *n == "todowrite" || *n == "todo")
        .collect();
    let baseline = calls.iter().rposition(|(_, a)| is_todo_plan(a));
    let (mut list, start) = match baseline {
        Some(i) => (parse_todos(calls[i].1).unwrap_or_default(), i + 1),
        None => (Vec::new(), 0),
    };
    for (_, a) in &calls[start..] {
        apply_todo_action(&mut list, a);  // 增量 action 仅作用于 baseline 之后
    }
    list
}
```

**核心规则**: 找到最后一个有效的 full-list plan 作为 baseline,然后只应用其后的增量 action。pre-baseline 的 action **被丢弃**(因为它们作用于已废弃的 baseline)。

### 3.7 derive_current_todos — 派生态还原

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:310-330`

```rust
pub fn derive_current_todos(messages: &[Message]) -> Vec<TodoItem> {
    // 用户取消 → 当前 plan 退役但保留历史
    let active_messages = messages
        .iter()
        .rposition(Message::is_user_interruption)
        .map_or(messages, |index| &messages[index + 1..]);
    let failed_call_ids = active_messages
        .iter()
        .filter(|message| message.is_error)
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect::<HashSet<_>>();
    reduce_todos(
        active_messages.iter()
            .flat_map(|m| m.tool_calls.iter())
            .filter(|call| !failed_call_ids.contains(call.id.as_str())),  // 失败的 call 不参与折叠
    )
}
```

**3 大过滤规则**:
1. **用户中断后的旧 plan 退役**:`rposition(Message::is_user_interruption)` 找到最新中断点,只取其后。
2. **失败的 call 不参与折叠**(已 reply 报错的 todo 不进入派生)。
3. **legacy 中断标记(`[The previous response was interrupted by the user before completing]`)** 也算中断点。

### 3.8 容错:解析时 lenient / 验证时 strict

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:38-49`

```rust
fn parse_lenient(s: &str) -> Option<TodoStatus> {
    let normalized = s.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "pending" | "todo" | "open" | "waiting" | "not started" | "待办" | "未开始" => Some(TodoStatus::Pending),
        "in_progress" | "in-progress" | "in progress" | "doing" | "active" | "进行中" => Some(TodoStatus::InProgress),
        "completed" | "complete" | "done" | "finished" | "closed" | "已完成" | "完成" => Some(TodoStatus::Completed),
        _ => None,
    }
}
```

**设计哲学**: 增量 `update` 用 lenient(`done` / `InProgress` / `进行中` 都接受),plan/re-plan 用 strict(必须严格枚举)。这避免了模型误用「done」被 reject 成"`update` needs a `status`" 的混乱。

### 3.9 placeholder 内容拒绝

**文件**: `crates/atomcode-capabilities/src/tools/todo.rs:176-199`

```rust
fn is_placeholder_task_label(content: &str) -> bool {
    let normalized = content.trim()
        .trim_matches(|ch: char| ch.is_ascii_punctuation() || ch.is_whitespace())
        .to_ascii_lowercase();
    let compact = normalized.chars()
        .filter(|ch| !ch.is_whitespace() && !matches!(ch, '-' | '_' | '#' | '.'))
        .collect::<String>();
    ["task", "step", "phase", "item", "任务", "步骤", "阶段", "事项"]
        .iter().any(|p| compact.strip_prefix(p).is_some_and(|s| s.is_empty() || s.chars().all(|ch| ch.is_ascii_digit())))
        || matches!(compact.as_str(), "处理功能" | "实现功能" | "开发功能" | "编写代码" | "修改代码")
}
```

**设计哲学**: 拒绝「task 1」「Step #2」「处理功能」等占位标签。**`task 1` 在 persisted 状态仍可读(legacy 兼容),但 live 新 plan 拒绝**。

---

## 4. deepseek-harness 的 Goal 域（4 态 active/paused/blocked/complete + armed/disarmed 分离 + CAS 事件溯源 + maxGoalRounds=256）

### 4.1 GoalPhase 4 态 + GoalActivation 2 态(正交拆分)

**文件**: `packages/goal/goal/src/types.ts:43-83`

```typescript
/** Durable continuation phase. Activation is process-local and separate. */
export type GoalPhase =
  | 'active'
  | 'paused'
  | 'blocked'
  | 'complete'

/** Whether this live process may automatically continue an active goal. */
export type GoalActivation = 'armed' | 'disarmed'

export interface GoalView extends GoalSnapshot {
  readonly roundsStarted: number
  readonly createdAt: number
  readonly updatedAt: number
  /** Process-local continuation eligibility; never persisted. */
  readonly activation: GoalActivation
}
```

**核心设计**: **`phase` (持久化) 与 `activation` (进程本地) 严格分离**:
- **phase 写在 session log**(goal/change 事件),可重放
- **activation 只活在 process 内存**,跨 session 重启后必 disarmed

这是 **「持久层 + 进程层」正交化** 的教科书式设计 — 避免重启后幽灵续轮。

### 4.2 CAS (Compare-And-Set) GoalRef 引用

**文件**: `packages/goal/goal/src/types.ts:18-24`

```typescript
export interface GoalRef {
  readonly id: GoalId             // 稳定身份
  readonly revision: number        // 每次持久化 mutation 递增
}
```

**每个 mutation 必须带 `ref`**: `edit(agent, ref, …)` / `pause(agent, ref)` / `resume(agent, ref)`。这是 **CAS 语义**,保证 stale request 被 reject(避免双重操作)。

### 4.3 GoalSnapshotChangeMeta — 全快照事件

**文件**: `packages/goal/goal/src/domain.ts:23-44`

```typescript
export interface GoalSnapshotChangeMeta {
  readonly kind: 'goal/change'
  readonly version: 1
  readonly operation: Exclude<GoalOperation, 'clear'>
  readonly goal: GoalSnapshot     // 全快照(whole-value)
  readonly roundsStarted: number
  readonly createdAt: number
  readonly updatedAt: number
}

export interface GoalClearChangeMeta {
  readonly kind: 'goal/change'
  readonly version: 1
  readonly operation: 'clear'
  readonly cleared: GoalRef       // tombstone
  readonly clearedAt: number
}
```

**whole-value 规则**: 每个 event 携带**完整 post-change snapshot**(不是 delta)。这样 fold 是 last-wins,无需 replay 日志就能读出当前状态。

**Clear tombstone**: 保留 `cleared: GoalRef`(`id` + `revision = current.revision+1`),history 可追溯。

### 4.4 GoalOperation 7 个动词

**文件**: `packages/goal/goal/src/domain.ts:14-22`

```typescript
export type GoalOperation =
  | 'create'        // 新建
  | 'edit'          // 改 objective / maxGoalRounds
  | 'pause'         // active → paused
  | 'resume'        // {active|paused|blocked} → active
  | 'complete'      // → complete
  | 'block'         // active → blocked(reason 必填)
  | 'clear'         // 清除(保留 tombstone)
```

7 个 verb 全覆盖状态机的全部转移。

### 4.5 GoalService.create — 创建入口

**文件**: `packages/goal/goal/src/index.ts:251-267`

```typescript
create(agent: Agent, request: CreateGoalRequest): GoalView {
    const spec = resolveCreateGoal(request, this.resolved.defaultMaxGoalRounds)
    const cache = this.prepareMutation(agent)
    const current = cache.state.goal
    if (current !== undefined && current.phase !== 'complete') {
      throw new GoalError(`goal "${current.id}" already exists with phase "${current.phase}"`, 'GOAL_ALREADY_EXISTS')
    }
    const now = Date.now()
    const goal: GoalSnapshot = {
      id: GoalId(`goal-${randomUUID()}`),
      revision: 1,
      objective: spec.objective,
      phase: 'active',
      maxGoalRounds: spec.maxGoalRounds,
    }
    return this.commitSnapshot(agent, cache, 'create', goal, 0, now, now, 'armed')  // 创建即 armed
}
```

**关键约束**: 当前 phase ≠ complete 则 `GOAL_ALREADY_EXISTS`。要替换必须先 clear 或走 edit。

### 4.6 GoalService.resume — 双约束守卫

**文件**: `packages/goal/goal/src/index.ts:310-328`

```typescript
@Remote('resume')
resume(agent: Agent, ref: GoalRef): GoalView {
    const cache = this.prepareMutation(agent)
    const current = this.expectCurrent(cache, ref)
    const resumable: readonly GoalPhase[] = ['active', 'paused', 'blocked']
    if (!resumable.includes(current.phase)) {
      throw this.transitionError(current, 'resume', resumable)
    }
    if (current.phase === 'active' && cache.activation === 'armed') {
      // 已 active+armed → 重复 resume 报错
      throw new GoalError(`goal "${current.id}" is already active and armed`, 'GOAL_INVALID_TRANSITION')
    }
    if (cache.state.roundsStarted >= current.maxGoalRounds) {
      // 轮数耗尽 → 必须先 edit 增加 maxGoalRounds 才能 resume
      throw new GoalError(`goal "${current.id}" exhausted ${current.maxGoalRounds} goal rounds; increase maxGoalRounds before resuming`, 'GOAL_INVALID_TRANSITION')
    }
    return this.commitCurrent(agent, cache, 'resume', this.withPhase(current, 'active'), 'armed')
}
```

### 4.7 GoalService.clear — Tombstone 化

**文件**: `packages/goal/goal/src/index.ts:376-390`

```typescript
@Remote('clear')
clear(agent: Agent, ref: GoalRef): GoalRef {
    const cache = this.prepareMutation(agent)
    const current = this.expectCurrent(cache, ref)
    const tombstone: GoalRef = { id: current.id, revision: current.revision + 1 }
    const change: GoalClearChangeMeta = {
      kind: 'goal/change',
      version: GOAL_CHANGE_VERSION,
      operation: 'clear',
      cleared: tombstone,
      clearedAt: this.nextMutationTime(cache),
    }
    this.commit(agent, cache, change, 'disarmed')
    return { ...tombstone }
}
```

**revision+1 设计**: tombstone 的 revision = current.revision + 1 → **任何用 current.revision 的旧 ref 一律被 reject** (stale CAS)。

### 4.8 foldGoal — 严格 replay fold

**文件**: `packages/goal/goal/src/fold.ts:339-349`

```typescript
export function foldGoal(events: readonly SessionEvent[]): FoldedGoal {
  const state = emptyGoalFoldState()
  for (const event of events) applyGoalEvent(state, event)
  return {
    ...state.goal === undefined ? {} : { goal: { ...state.goal } },
    roundsStarted: state.roundsStarted,
    ...state.createdAt === undefined ? {} : { createdAt: state.createdAt },
    ...state.updatedAt === undefined ? {} : { updatedAt: state.updatedAt },
    ...state.lastRef === undefined ? {} : { lastRef: { ...state.lastRef } },
  }
}
```

**纯函数 fold** + **validateSnapshotTransition 严格校验**(`fold.ts:200-253`):

```typescript
function validateSnapshotTransition(state, change, current): void {
  const next = change.goal
  requireNextRevision(current, next, change.operation)  // 下一版本必须 current.revision+1
  // edit 不能改 phase
  if (change.operation === 'edit') {
    if (next.phase !== current.phase || JSON.stringify(next.blockedReason) !== JSON.stringify(current.blockedReason)) {
      throw new Error('goal edit cannot change phase or blocked reason')
    }
  }
  // pause:active → paused
  if (change.operation === 'pause') {
    if (current.phase !== 'active' || next.phase !== 'paused') throw new Error('goal pause has an invalid phase transition')
  }
  // ...
}
```

### 4.9 applyGoalEvent — user/message 校验 round 编号

**文件**: `packages/goal/goal/src/fold.ts:313-332`

```typescript
export function applyGoalEvent(state: GoalFoldState, event: SessionEvent): void {
  if (event.type === 'goal/change') { /* ... */ }
  if (event.type === 'user/message') {
    const source = goalSource(event.data.source)
    if (source === undefined) return
    const current = state.goal
    // 严格校验: 当前轮的 user message 必须:
    // - 来自 active 目标
    // - goalId / revision 匹配
    // - round = state.roundsStarted + 1 (下一个 admitted round)
    // - round ≤ maxGoalRounds
    if (current === undefined || current.phase !== 'active' || source.goalId !== current.id
      || source.revision !== current.revision || source.round !== state.roundsStarted + 1
      || source.round > current.maxGoalRounds) {
      throw new Error(`goal round at session event ${event.seq} is not the next admitted round of the active goal`)
    }
    state.roundsStarted = source.round
  }
}
```

**round 严格递增**: 任何 `user/message` 携带的 `source.round` 必须 = `state.roundsStarted + 1`。这是 **goal-round 的不变量校验**,防止外部伪造轮数。

### 4.10 goal-round-driver — 自动续轮调度器

**文件**: `packages/goal/goal-round-driver/src/index.ts:76-205`

这是 **「自动续轮」的真正核心**: `agent/session-start` 后注册一个 driver effect,监听 `agent/status` `idle`,在 idle 时(且无 competingQueued / stopping)自动 `agent.followup(createUserMessage({ source: { kind: 'goal', goalId, revision, round } }))`。

```typescript
async function drive(state: DriverState): Promise<void> {
    const { agent } = state
    if (!readyToDrive(state)) return
    // ... checkpoint / 当前 attempt 处理 ...
    const goal = currentGoal(state)
    if (goal === undefined || goal.phase !== 'active' || goal.activation !== 'armed') return
    if (goal.roundsStarted >= goal.maxGoalRounds) {
      // 耗尽 → 自动 block 而非 silent
      ctx.goals.block(agent, goalRef(goal), {
        code: 'round-limit',
        message: `Goal reached its configured limit of ${goal.maxGoalRounds} rounds.`,
      })
      return
    }
    const round = goal.roundsStarted + 1
    const content = renderGoalRoundPrompt(goal, round)
    const message = createUserMessage({
      content,
      source: { kind: 'goal', goalId: goal.id, revision: goal.revision, round },
    })
    state.attempt = { goalId, revision, round, messageId: message.id, content, phase: 'queued', cancelled: false, stale: false }
    try { agent.followup(message) } catch (error) { /* queue-failed → block */ }
}
```

**race-fence**: 每个 round 在 driver 内有 `attempt = {queued|claimed|admitted}`,`agent/inbox/inserted` / `claimed` / `discarded` 事件驱动 phase 推进。**validReservation** 检查 `goal === current && attempt.phase === 'claimed' && source.round === goal.roundsStarted + 1` 才放行进 step。

### 4.11 tool-goal authority — 模型工具的 authority 校验

**文件**: `packages/goal/tool-goal/src/authority.ts:90-108`

```typescript
export function requireDirectHuman(ctx: Context, execution: GoalToolExecution): void {
  if (hasDirectHumanInput(ctx, execution)) return
  reject('this goal operation requires a direct human turn on a top-level agent')
}

export function completionAuthority(ctx: Context, execution: GoalToolExecution): GoalToolAuthority {
  if (hasDirectHumanInput(ctx, execution)) return { kind: 'direct-human' }
  const goal = ctx.goals.get(execution.agent)
  if (goal !== undefined && isMatchingGoalRound(execution, goal)) {
    return { kind: 'goal-round', goal }
  }
  return reject('complete and blocked require a direct human turn or the current goal round')
}
```

**Authority 二分**:
- `direct-human`: root agent 收到 human message → 可 edit / pause / resume
- `goal-round`: 当前 tool call 来自 goal-round-driver 排队 → 可 complete / blocked

**这避免了「模型自主 mark complete」绕过人为控制**(必须 round 到达后由模型自报,但 round 来源可控)。

### 4.12 /goal 命令的 6 个 verb

**文件**: `packages/goal/command-goal/src/index.ts:34-44`

```typescript
function parseGoalCommand(rawInput: string): GoalCommand {
  const input = rawInput.trim()
  if (input.length === 0) return { kind: 'show' }
  const control = input.toLowerCase()
  if (control === 'clear') return { kind: 'clear' }
  if (control === 'pause') return { kind: 'pause' }
  if (control === 'resume') return { kind: 'resume' }
  if (control === 'edit') return { kind: 'invalid-edit' }
  if (/^edit(?=\s)/iu.test(input)) return { kind: 'edit', objective: input.slice(4).trim() }
  return { kind: 'create', objective: input }
}
```

`/goal <objective>` 创建,`/goal edit <new>` 改,`/goal pause|resume|clear` 控制,`/goal` (空) 显示。

### 4.13 wrapup context — goal-round 完成时的注入

**文件**: `packages/goal/tool-goal/src/wrapup.ts`(未显示完整内容) + `tool-goal/src/index.ts:313-325`

```typescript
if (authority.kind === 'goal-round') {
  exec.deferContext(createUserMessage({
    content: args.action === 'complete'
      ? renderWrapupContext(goal.objective)
      : renderWrapupContext(goal.objective, args.blocked_reason as string),
    source: { kind: 'plugin', plugin: 'tool-goal', form: 'notice', summary: boundContextSummary(`${args.action as string}: ${goal.objective}`) },
  }))
}
```

**goal-round 完成后注入 wrapup**: 让下一轮模型有上下文 context,知道 goal 已 mark complete/blocked。

---

## 5. openclaw 的 SessionGoal 6 态（active/paused/blocked/usage_limited/budget_limited/complete + token 预算）

### 5.1 6 态 SessionGoalStatus

**文件**: `packages/gateway-protocol/src/schema/sessions-goal.ts:6-31`

```typescript
export const SessionGoalSchema = closedObject({
  schemaVersion: Type.Literal(1),
  id: NonEmptyString,
  objective: Type.String(),
  status: Type.Union([
    Type.Literal("active"),
    Type.Literal("paused"),
    Type.Literal("blocked"),
    Type.Literal("usage_limited"),
    Type.Literal("budget_limited"),
    Type.Literal("complete"),
  ]),
  createdAt: Type.Number(),
  updatedAt: Type.Number(),
  tokenStart: Type.Number(),
  tokenStartFresh: Type.Optional(Type.Boolean()),
  tokensUsed: Type.Number(),
  tokenBudget: Type.Optional(Type.Number()),
  continuationTurns: Type.Number(),
  lastStatusNote: Type.Optional(Type.String()),
  pausedAt: Type.Optional(Type.Number()),
  blockedAt: Type.Optional(Type.Number()),
  completedAt: Type.Optional(Type.Number()),
  usageLimitedAt: Type.Optional(Type.Number()),
  budgetLimitedAt: Type.Optional(Type.Number()),
})
```

**6 态分类**:
- **业务态**: `active` / `paused` / `blocked` / `complete`
- **资源态**: `usage_limited` / `budget_limited`(区别于业务态,明确由预算触发)

每个状态都有 timestamp(`pausedAt` / `blockedAt` / `usageLimitedAt` / `budgetLimitedAt` / `completedAt`),便于历史追溯。

### 5.2 accountSessionGoalUsage — Token 核算核心

**文件**: `src/config/sessions/goals-transitions.ts:37-78`

```typescript
export function accountSessionGoalUsage(
  entry: Pick<SessionEntry, "goal" | "totalTokens" | "totalTokensFresh" | "totalTokensVersion">,
  now: number,
  options?: { adoptFreshBaseline?: boolean },
): SessionGoal | undefined {
  const goal = entry.goal;
  if (!goal) return undefined;
  const totalTokens = resolveEntryFreshTotalTokens(entry);
  const hasFreshStart = goal.tokenStartFresh !== false;
  // 旧的 stale token baseline 在 display-only 时保留,在 persisted 时 adopt fresh
  const shouldHoldStaleStart = !hasFreshStart && options?.adoptFreshBaseline === false;
  const shouldAdoptFreshStart =
    !shouldHoldStaleStart && totalTokens !== undefined && !hasFreshStart;
  const tokenStart = shouldAdoptFreshStart
    ? totalTokens
    : (normalizeTokenCount(goal.tokenStart) ?? totalTokens ?? 0);
  const tokensUsed =
    totalTokens === undefined || shouldAdoptFreshStart || shouldHoldStaleStart
      ? goal.tokensUsed
      : Math.max(goal.tokensUsed, Math.max(0, totalTokens - tokenStart));
  const next: SessionGoal = { ...goal, tokenStart, tokenStartFresh: hasFreshStart || shouldAdoptFreshStart, tokensUsed };
  // 关键:预算自动触发 budget_limited
  if (next.status === "active" && next.tokenBudget !== undefined && tokensUsed >= next.tokenBudget) {
    next.status = "budget_limited";
    next.budgetLimitedAt = now;
    next.updatedAt = now;
  }
  return next;
}
```

**关键不变量**: `tokensUsed = max(goal.tokensUsed, totalTokens - tokenStart)`。每次读取 goal 都会**重新核算**,并自动迁移 active → budget_limited(无需显式 trigger)。

### 5.3 buildUpdatedSessionGoalStatus — 状态转换 + 预算窗口

**文件**: `src/config/sessions/goals-transitions.ts:109-156`

```typescript
export function buildUpdatedSessionGoalStatus(entry, options, now): SessionGoal {
  const accounted = accountSessionGoalUsage(entry, now);
  if (!accounted) throw new SessionGoalTransitionError("goal not found");
  if (TERMINAL_GOAL_STATUSES.has(accounted.status) && accounted.status !== options.status) {
    throw new SessionGoalTransitionError(`goal is already ${accounted.status}`);  // 终态不可转
  }
  // resume from budget_limited / usage_limited → 重置预算窗口
  const resetsBudgetWindow =
    options.status === "active" &&
    (accounted.status === "budget_limited" ||
      accounted.status === "usage_limited" ||
      (accounted.tokenBudget !== undefined && accounted.tokensUsed >= accounted.tokenBudget));
  const freshTokenStart = resetsBudgetWindow ? resolveEntryFreshTotalTokens(entry) : undefined;
  const next: SessionGoal = {
    ...accounted,
    status: options.status,
    updatedAt: now,
    ...(options.note ? { lastStatusNote: options.note } : {}),
    ...(options.status === "paused" ? { pausedAt: now } : {}),
    ...(options.status === "blocked" ? { blockedAt: now } : {}),
    ...(options.status === "complete" ? { completedAt: now } : {}),
  };
  if (resetsBudgetWindow) {
    next.tokenStart = freshTokenStart ?? 0;  // 重置起点
    next.tokenStartFresh = freshTokenStart !== undefined;
    next.tokensUsed = 0;                      // 重置已用
    delete next.budgetLimitedAt;               // 清除时间戳
    delete next.usageLimitedAt;
  }
  if (next.status === "active" && next.tokenBudget !== undefined && next.tokensUsed >= next.tokenBudget) {
    next.status = "budget_limited";            // resume 后立即再次触发
    next.budgetLimitedAt = now;
  }
  return next;
}
```

**resume 重置预算窗口**: 这是 budget_limited/usage_limited 的「解除」语义 — 重新进入 active 时,tokenStart 切到当前 fresh total,tokensUsed 归零。

### 5.4 /goal 命令的 hint

**文件**: `src/config/sessions/goals.ts:99-112`

```typescript
function resolveGoalCommandHint(status: SessionGoalStatus): string {
  switch (status) {
    case "active":
      return "/goal edit <objective>, /goal pause, /goal complete, /goal clear";
    case "paused":
    case "blocked":
    case "usage_limited":
    case "budget_limited":
      return "/goal resume, /goal edit <objective>, /goal clear";
    case "complete":
      return "/goal clear";
  }
  return "/goal";
}
```

**6 态 → 3 类 hint**:
- active → 可 `pause` / `complete` / `edit` / `clear`
- paused/blocked/usage_limited/budget_limited → 可 `resume` / `edit` / `clear`
- complete → 只能 `clear`

### 5.5 goal-tools — 模型自报 complete/blocked

**文件**: `src/agents/tools/goal-tools.ts:41-54, 123-159`

```typescript
const UpdateGoalToolSchema = Type.Object({
  status: stringEnum(MODEL_UPDATABLE_SESSION_GOAL_STATUSES, {
    description: "complete | blocked.",
  }),
  note: Type.Optional(Type.String({ description: "Short status note." })),
});

export const MODEL_UPDATABLE_SESSION_GOAL_STATUSES = ["complete", "blocked"] as const;
```

**模型只能写 complete / blocked**(其它状态由系统自动管理,如 budget_limited 触发)。这是 **「模型能力范围最小化」** 的设计哲学 — 模型不应能任意 pause/resume,这些需人参与。

### 5.6 createSessionGoal — entry 创建

**文件**: `src/config/sessions/goals.ts:147-171`

```typescript
export async function createSessionGoal(options: CreateSessionGoalOptions): Promise<SessionGoal> {
  const objective = options.objective.trim();
  if (!objective) throw new Error("objective required");
  const now = nowMs(options.now);
  let created: SessionGoal | undefined;
  const result = await patchSessionEntryCore({...}, (entry) => {
    created = buildCreatedSessionGoal(entry, { objective, tokenBudget: options.tokenBudget }, now);
    return { goal: created };
  }, { fallbackEntry: options.fallbackEntry });
  if (!result || !created) throw new Error("session not found");
  recordGoalChange(options, result, "goal created");
  return cloneGoal(created);
}
```

**`patchSessionEntryCore` 模式**: 用乐观锁 + 回调模式做 session entry 的原子更新,确保 goal 的写入与会话 entry 一致。

### 5.7 compaction-planning — 上下文压缩的规划辅助

**文件**: `src/agents/compaction-planning.ts:18-29, 230-243`

```typescript
export const BASE_CHUNK_RATIO = 0.4;
export const MIN_CHUNK_RATIO = 0.15;
export const SAFETY_MARGIN = 1.2;
const DEFAULT_PARTS = 2;
export const SUMMARIZATION_OVERHEAD_TOKENS = 4096;

export type StageSplitPlan =
  | { mode: "single" }
  | { mode: "split"; chunks: AgentMessage[][] };
```

**compaction-planning 不是 Goal 状态机本身**,而是 Goal 状态机的「附属模块」: 当 token 接近 context window 上限,规划如何分块压缩消息(token budget 触发)。

`buildSummaryChunks` / `buildOversizedFallbackPlan` / `buildStageSplitPlan` / `pruneHistoryForContextShare` 是 4 个核心 planner,**目标都是避免 Goal 进入 budget_limited 时丢失历史**。

---

## 6. opencode 的 plan/build 双 Agent + plan_exit（read-only 计划态 + build 切换）

### 6.1 双 Agent:plan(read-only) vs build(default)

**文件**: `packages/opencode/src/agent/agent.ts:140-181`

```typescript
const agents: Record<string, Info> = {
  build: {
    name: "build",
    description: "The default agent. Executes tools based on configured permissions.",
    options: {},
    permission: Permission.merge(defaults, Permission.fromConfig({
      question: "allow",
      plan_enter: "allow",  // build 可以进 plan
    }), user),
    mode: "primary",
    native: true,
  },
  plan: {
    name: "plan",
    description: "Plan mode. Disallows all edit tools.",
    options: {},
    permission: Permission.merge(defaults, Permission.fromConfig({
      question: "allow",
      plan_exit: "allow",    // plan 可以 exit
      task: { general: "deny" },  // plan 不能 spawn subagent
      external_directory: {
        [path.join(Global.Path.data, "plans", "*")]: "allow",  // plan 目录可写
      },
      edit: {
        "*": "deny",  // 全局 deny edit
        [path.join(".opencode", "plans", "*.md")]: "allow",   // 但允许写 plan 文件
        [path.relative(ctx.worktree, path.join(Global.Path.data, path.join("plans", "*.md")))]: "allow",
      },
    }), user),
    mode: "primary",
    native: true,
  },
  // ...
}
```

**plan 权限精准设计**:
- **全局 `edit: deny`**
- **例外:`.opencode/plans/*.md` 或 `$Global.Path.data/plans/*.md` 可写**
- **`task.general: deny`** → plan 不能 spawn subagent(保持专注)

### 6.2 Session.plan — plan 文件路径生成

**文件**: `packages/opencode/src/session/session.ts:331-336`

```typescript
export function plan(input: { slug: string; time: { created: number } }, instance: InstanceContext) {
  const base = instance.project.vcs
    ? path.join(instance.worktree, ".opencode", "plans")
    : path.join(Global.Path.data, "plans")
  return path.join(base, [input.time.created, input.slug].join("-") + ".md")
}
```

**路径规则**: `plans/{time.created}-{slug}.md`。VCS 项目用 `.opencode/plans/`(版本控制),否则用 `Global.Path.data/plans/`(全局)。

### 6.3 plan_exit 工具 — 用户审批

**文件**: `packages/opencode/src/tool/plan.ts:15-79`

```typescript
export const PlanExitTool = Tool.define(
  "plan_exit",
  Effect.gen(function* () {
    return {
      description: EXIT_DESCRIPTION,  // "Use this tool when you have completed the planning phase..."
      parameters: Parameters,         // empty schema
      execute: (_params: {}, ctx: Tool.Context) =>
        Effect.gen(function* () {
          const instance = yield* InstanceState.context
          const info = yield* session.get(ctx.sessionID)
          const plan = path.relative(instance.worktree, Session.plan(info, instance))
          // 关键:用 Question 工具询问批准
          const answers = yield* question.ask({
            sessionID: ctx.sessionID,
            questions: [{
              question: `Plan at ${plan} is complete. Would you like to switch to build agent and start implementing?`,
              header: "Build Agent",
              custom: false,
              options: [
                { label: "Yes", description: "Switch to build agent and start implementing the plan" },
                { label: "No", description: "Stay with plan agent to continue refining the plan" },
              ],
            }],
            tool: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
          })
          if (answers[0]?.[0] === "No") yield* new Question.RejectedError()  // 拒绝 → 留在 plan
          // 切换到 build agent
          const msg: SessionV1.User = {
            id: MessageID.ascending(), sessionID: ctx.sessionID,
            role: "user", time: { created: Date.now() },
            agent: "build",    // 切换 agent name
            model,
          }
          yield* session.updateMessage(msg)
          yield* session.updatePart({
            type: "text",
            text: `The plan at ${plan} has been approved, you can now edit files. Execute the plan`,
            synthetic: true,
          })
          return { title: "Switching to build agent", output: "User approved switching to build agent. Wait for further instructions." }
        }).pipe(Effect.orDie),
    }
  }),
)
```

**核心**: plan_exit 是一个**用户交互式切换器**:
1. Question 询问是否切换到 build agent
2. Yes → 创建新 user message, agent name 改为 "build"
4. 4. No → Question.RejectedError,模型继续在 plan mode

### 6.4 plan.txt — Plan Mode 系统提示

**文件**: `packages/opencode/src/session/prompt/plan.txt:1-26`

```
<system-reminder>
# Plan Mode - System Reminder

CRITICAL: Plan mode ACTIVE - you are in READ-ONLY phase. STRICTLY FORBIDDEN:
ANY file edits, modifications, or system changes...

## Responsibility
Your current responsibility is to think, read, search, and delegate explore agents
to construct a well-formed plan...
</system-reminder>
```

**System reminder 注入**(等同 atomcode PlanModeReminderHook)。

### 6.5 plan-mode.txt — 5 阶段工作流

**文件**: `packages/opencode/src/session/prompt/plan-mode.txt:62-68`

```
### Phase 4: Final Plan
Goal: Write your final plan to the plan file (the only file you can edit).

### Phase 5: Call plan_exit tool
At the very end of your turn, once you have asked the user questions and are happy
with your final plan file - you should always call plan_exit to indicate to the user
that you are done planning. This is critical - your turn should only end with either
asking the user a question or calling plan_exit.
```

**5 阶段硬性工作流**:
1. **Initial Understanding** — 用 explore subagent 并行调研
2. **Design** — 用 general agent 设计实现方案
3. **Review** — 读关键文件 + question 澄清
4. **Final Plan** — 写 plan 文件
5. **Call plan_exit** — 用户审批

### 6.6 reminders.apply — 模式切换的提示词

**文件**: `packages/opencode/src/session/reminders.ts:50-89`

```typescript
if (input.agent.name !== "plan" && assistantMessage?.info.agent === "plan") {
  // build agent 接管 plan agent 留下的对话 → 注入 build-switch 提示
  const ctx = yield* InstanceState.context
  const plan = Session.plan(input.session, ctx)
  const exists = yield* fsys.existsSafe(plan)
  const part = yield* sessions.updatePart({
    type: "text",
    text: exists
      ? `${BUILD_SWITCH}\n\nA plan file exists at ${plan}. You should execute on the plan defined within it`
      : BUILD_SWITCH,
    synthetic: true,
  })
  userMessage.parts.push(part)
  return input.messages
}

if (input.agent.name !== "plan" || assistantMessage?.info.agent === "plan") return input.messages

// 进入 plan agent → 注入 plan-mode 提示
const ctx = yield* InstanceState.context
const plan = Session.plan(input.session, ctx)
const exists = yield* fsys.existsSafe(plan)
if (!exists) yield* fsys.ensureDir(path.dirname(plan)).pipe(Effect.catch(Effect.die))
const part = yield* sessions.updatePart({
  type: "text",
  text: PLAN_MODE.replace("${planInfo}", () =>
    exists
      ? `A plan file already exists at ${plan}. You can read it and make incremental edits using the edit tool.`
      : `No plan file exists yet. You should create your plan at ${plan} using the write tool.`,
  ),
  synthetic: true,
})
```

**自动迁移提示**:
- 上一条 assistant 消息是 plan agent,当前 user 进入 build agent → 注入 `BUILD_SWITCH` 提示,告诉 build agent "plan 文件在 X,执行之"
- 当前 user 进入 plan agent → 注入 `PLAN_MODE` 提示,告诉 "plan 文件在 X,可以增量编辑"

### 6.7 状态机总结(2 态)

```
            user creates session
                  │
                  ▼
              [build agent]  ←─ default
                  │
                  │ plan_enter tool (or /plan)
                  ▼
              [plan agent]    ←─ read-only
                  │
                  │ plan_exit tool → user approves
                  ▼
              [build agent]  ←─ synthetic: "The plan at X has been approved, you can now edit files. Execute the plan"
                  │
                  ▼
             (conversation continues)
```

---

## 7. pi 的 steer/followUp 队列 + 扩展式 Plan Mode（3 态 enabled/executing + 持久化 + [DONE:n] 标记）

### 7.1 steer/followUp 双队列

**文件**: `packages/coding-agent/src/core/agent-session.ts:1379-1418, 1423-1452`

```typescript
async steer(text: string, images?: ImageContent[]): Promise<void> {
  // 扩展命令无法入队 → throw
  if (text.startsWith("/")) this._throwIfExtensionCommand(text);
  // 扩展 skill command + prompt template
  let expandedText = this._expandSkillCommand(text);
  expandedText = expandPromptTemplate(expandedText, [...this.promptTemplates]);
  await this._queueSteer(expandedText, images);
}

async followUp(text: string, images?: ImageContent[]): Promise<void> {
  // 同上,但走 _queueFollowUp
}

private async _queueSteer(text: string, images?: ImageContent[]): Promise<void> {
  this._steeringMessages.push(text);  // 1. push to tracking array
  this._emitQueueUpdate();
  const content: (TextContent | ImageContent)[] = [{ type: "text", text }];
  if (images) content.push(...images);
  this.agent.steer({ role: "user", content, timestamp: Date.now() });  // 2. 投递到 LLM agent
}

private async _queueFollowUp(text: string, images?: ImageContent[]): Promise<void> {
  this._followUpMessages.push(text);
  this._emitQueueUpdate();
  const content: (TextContent | ImageContent)[] = [{ type: "text", text }];
  if (images) content.push(...images);
  this.agent.followUp({ role: "user", content, timestamp: Date.now() });
}
```

**steer vs followUp**:
- **steer**:**打断当前 streaming**(在当前 assistant turn 的 tool call 完成后立即注入)
- **followUp**: **等当前 agent 完全 idle 才投递**(包括 steer 队列也空之后)

`streamingBehavior: "steer" | "followUp"` 是 SDK 的暴露选项。

### 7.2 mode:all / one-at-a-time

**文件**: `packages/coding-agent/src/core/agent-session.ts:1001-1008`

```typescript
get steeringMode(): "all" | "one-at-a-time" {
  return this.agent.steeringMode;  // 配置决定如何消费队列
}

get followUpMode(): "all" | "one-at-a-time" {
  return this.agent.followUpMode;
}
```

**2 种消费模式**:
- **all**: 一次 consume 队列全部
- **one-at-a-time**: 一次只 consume 一个

### 7.3 计划模式 Extension — 3 状态 PlanModeState

**文件**: `packages/coding-agent/examples/extensions/plan-mode/index.ts:27-32`

```typescript
interface PlanModeState {
  enabled: boolean;        // plan mode 是否激活
  todos?: TodoItem[];      // 当前待办
  executing?: boolean;     // 是否在执行 plan
  toolsBeforePlanMode?: string[];  // 进入 plan mode 之前的工具集
}
```

**3 状态**:`enabled` (计划模式中) → `executing` (执行模式) → `enabled = false` (退出)。

### 7.4 计划模式入口

**文件**: `packages/coding-agent/examples/extensions/plan-mode/index.ts:141-162`

```typescript
pi.registerCommand("plan", {
  description: "Toggle plan mode (read-only exploration)",
  handler: async (_args, ctx) => togglePlanMode(ctx),
});

pi.registerCommand("todos", {
  description: "Show current plan todo list",
  handler: async (_args, ctx) => { /* ... */ },
});

pi.registerShortcut(Key.ctrlAlt("p"), {
  description: "Toggle plan mode",
  handler: async (ctx) => togglePlanMode(ctx),
});
```

**3 入口**:`/plan` 命令 / `Ctrl+Alt+P` 快捷键 / `--plan` 启动参数。

### 7.5 工具集切换(plan 模式只允许 read 工具)

**文件**: `packages/coding-agent/examples/extensions/plan-mode/index.ts:104-114`

```typescript
function enablePlanModeTools(): void {
  if (toolsBeforePlanMode === undefined) {
    toolsBeforePlanMode = pi.getActiveTools();  // 保存原工具集
  }
  pi.setActiveTools(getPlanModeTools(toolsBeforePlanMode));
  // PLAN_MODE_TOOLS = ["read", "bash", "grep", "find", "ls", "questionnaire"]
  // PLAN_MODE_DISABLED_TOOLS = new Set(["edit", "write"])
}

function restoreNormalModeTools(): void {
  pi.setActiveTools(toolsBeforePlanMode ?? getNormalModeTools(pi.getActiveTools()));
  toolsBeforePlanMode = undefined;
}
```

**工具白名单机制**: 进入 plan mode **禁用 edit / write**,只保留 read 类。

### 7.6 Bash 安全白名单(plan mode)

**文件**: `packages/coding-agent/examples/extensions/plan-mode/utils.ts:7-101`

```typescript
const DESTRUCTIVE_PATTERNS = [
  /\brm\b/i, /\brmdir\b/i, /\bmv\b/i, /\bmkdir\b/i, /\btouch\b/i,
  /\bchmod\b/i, /\bchown\b/i, /\btee\b/i,
  /\bgit\s+(add|commit|push|pull|merge|rebase|reset|checkout|branch\s+-[dD]|stash)/i,
  /\bsudo\b/i, /\bsu\b/i, /\bkill\b/i, /\breboot\b/i, ...
];

const SAFE_PATTERNS = [
  /^\s*cat\b/, /^\s*head\b/, /^\s*tail\b/, /^\s*less\b/, /^\s*more\b/,
  /^\s*grep\b/, /^\s*find\b/, /^\s*ls\b/, /^\s*pwd\b/, /^\s*echo\b/,
  /^\s*git\s+(status|log|diff|show|branch|remote|config\s+--get)/i,
  /^\s*curl\s/, /^\s*jq\b/, /^\s*sed\s+-n/i, /^\s*awk\b/, ...
];

export function isSafeCommand(command: string): boolean {
  const isDestructive = DESTRUCTIVE_PATTERNS.some((p) => p.test(command));
  const isSafe = SAFE_PATTERNS.some((p) => p.test(command));
  return !isDestructive && isSafe;  // 既不在 destructive 列表,且在 safe 列表
}
```

**双重防御**: 不仅正则禁止破坏性命令,还必须匹配 read-only 命令(防止 `echo > file` 绕过)。

### 7.7 提取计划步骤 + 进度标记 [DONE:n]

**文件**: `packages/coding-agent/examples/extensions/plan-mode/utils.ts:129-168`

```typescript
export function extractTodoItems(message: string): TodoItem[] {
  const items: TodoItem[] = [];
  const headerMatch = message.match(/\*{0,2}Plan:\*{0,2}\s*\n/i);
  if (!headerMatch) return items;
  const planSection = message.slice(message.indexOf(headerMatch[0]) + headerMatch[0].length);
  const numberedPattern = /^\s*(\d+)[.)]\s+\*{0,2}([^*\n]+)/gm;
  for (const match of planSection.matchAll(numberedPattern)) {
    const text = match[2].trim().replace(/\*{1,2}$/, "").trim();
    if (text.length > 5 && !text.startsWith("`") && !text.startsWith("/") && !text.startsWith("-")) {
      const cleaned = cleanStepText(text);
      if (cleaned.length > 3) {
        items.push({ step: items.length + 1, text: cleaned, completed: false });
      }
    }
  }
  return items;
}

export function extractDoneSteps(message: string): number[] {
  const steps: number[] = [];
  for (const match of message.matchAll(/\[DONE:(\d+)\]/gi)) {
    const step = Number(match[1]);
    if (Number.isFinite(step)) steps.push(step);
  }
  return steps;
}

export function markCompletedSteps(text: string, items: TodoItem[]): number {
  const doneSteps = extractDoneSteps(text);
  for (const step of doneSteps) {
    const item = items.find((t) => t.step === step);
    if (item) item.completed = true;
  }
  return doneSteps.length;
}
```

**Plan Mode 工作流**:
1. 模型输出 `Plan:\n1. 步骤1\n2. 步骤2\n3. 步骤3`
2. `extractTodoItems` 解析出 TodoItem[]
3. 用户进入 executing 模式后,每个 turn 结束扫 `[DONE:n]` 标记
4. 全部 `[DONE]` 后自动 emit "**Plan Complete!** ✓"

### 7.8 持久化 — appendEntry 模式

**文件**: `packages/coding-agent/examples/extensions/plan-mode/index.ts:116-123`

```typescript
function persistState(): void {
  pi.appendEntry("plan-mode", {
    enabled: planModeEnabled,
    todos: todoItems,
    executing: executionMode,
    toolsBeforePlanMode,
  });
}
```

**通过 `appendEntry("plan-mode", …)` 写入 session entry**。resume 时通过 `entries.filter(e => e.type === "custom" && e.customType === "plan-mode").pop()` 读取。

### 7.9 状态机总结(3 态)

```
         /plan on (or Ctrl+Alt+P, --plan)
                  │
                  ▼
              [enabled=true, executing=false]
              tool set = [read, bash*restricted*, grep, find, ls, questionnaire]
              inject "[PLAN MODE ACTIVE]" reminder
                  │
                  │ agent_end → extract "Plan:\n1. ...\n2. ..." → todos[]
                  ▼
              ui.select("Execute | Stay | Refine")
                  │
        ┌─────────┼──────────┐
        │ Execute│ Stay      │ Refine
        ▼         ▼           ▼
   [enabled=false, send "Execute the plan. After [DONE:n]"
    executing=true]      
        │                  
        │ turn_end → scan [DONE:n] → mark complete
        │ all complete → "**Plan Complete!** ✓" → executing=false, todos=[]
        ▼
   [idle]
```

---

## 8. jiuwenswarm 的 SwarmFlow DAG（WORKFLOW_* 6 态 + Phase/Agent 4 层折叠 + journal resume）

### 8.1 6 类 ProgressKind 事件

**文件**: `openjiuwen/agent_teams/workflow/engine/progress.py:37-51`

```python
class ProgressKind:
    WORKFLOW_STARTED = "workflow_started"
    PHASE = "phase"
    AGENT_STARTED = "agent_started"
    AGENT_COMPLETED = "agent_completed"
    AGENT_FAILED = "agent_failed"
    LOG = "log"
    WORKFLOW_COMPLETED = "workflow_completed"
    WORKFLOW_FAILED = "workflow_failed"
    WORKFLOW_PAUSED = "workflow_paused"
    WORKFLOW_STOPPED = "workflow_stopped"
```

**6 终态 + 5 中间事件**:
- 中间事件:`WORKFLOW_STARTED` / `PHASE` / `AGENT_STARTED` / `AGENT_COMPLETED` / `AGENT_FAILED` / `LOG`
- 终态事件:`WORKFLOW_COMPLETED` / `WORKFLOW_FAILED` / `WORKFLOW_PAUSED` / `WORKFLOW_STOPPED`

### 8.2 4 层 WorkflowRun 模型

**文件**: `openjiuwen/agent_teams/workflow/schema.py:25-48`

```python
class AgentActivity(BaseModel):
    """One ``agent()`` call: its prompt, narration trail, and outcome (layer 4)."""
    label: str | None = None
    prompt: str | None = None
    activity: list[str] = Field(default_factory=list)
    outcome: str | None = None
    status: str = "running"  # "running" until its AGENT_COMPLETED arrives

class PhaseRecord(BaseModel):
    """One ``phase()`` group and the agents that ran under it (layer 2 + 3)."""
    title: str
    agents: list[AgentActivity] = Field(default_factory=list)

class WorkflowRun(BaseModel):
    """The whole run: ordered phases (layer 1) and overall status."""
    name: str | None = None
    status: str = "running"  # "running" until WORKFLOW_COMPLETED
    phases: list[PhaseRecord] = Field(default_factory=list)
```

**4 层嵌套**:`WorkflowRun` → `PhaseRecord` → `AgentActivity` → `{prompt, activity, outcome}`。**TUI 渲染按 Phase ▸ agents ▸ prompt/activity/outcome**。

### 8.3 build_workflow_run_from_events — 事件折叠

**文件**: `openjiuwen/agent_teams/workflow/schema.py:50-98`

```python
def build_workflow_run_from_events(events: list[WorkflowProgressEvent]) -> WorkflowRun:
    run = WorkflowRun()
    phases: dict[str, PhaseRecord] = {}
    def phase_for(title: str | None) -> PhaseRecord:
        key = title or _NO_PHASE
        rec = phases.get(key)
        if rec is None:
            rec = PhaseRecord(title=key)
            phases[key] = rec
            run.phases.append(rec)
        return rec
    for ev in events:
        if ev.kind == ProgressKind.WORKFLOW_STARTED:
            run.name = ev.name
        elif ev.kind == ProgressKind.WORKFLOW_COMPLETED:
            run.status = "completed"
        elif ev.kind == ProgressKind.WORKFLOW_FAILED:
            run.status = "failed"
        elif ev.kind == ProgressKind.PHASE:
            phase_for(ev.phase)  # 新 phase
        elif ev.kind == ProgressKind.AGENT_STARTED:
            phase_for(ev.phase).agents.append(
                AgentActivity(label=ev.label, prompt=ev.prompt, status="running")
            )
        elif ev.kind == ProgressKind.AGENT_COMPLETED:
            activity = _latest_running(phase_for(ev.phase), ev.label)
            if activity is not None:
                activity.outcome = ev.outcome
                activity.status = "completed"
        elif ev.kind == ProgressKind.AGENT_FAILED:
            activity = _latest_running(phase_for(ev.phase), ev.label)
            if activity is not None:
                activity.outcome = ev.message
                activity.status = "failed"
        elif ev.kind == ProgressKind.LOG:
            rec = phase_for(ev.phase)
            target = _latest_running(rec, None) or (rec.agents[-1] if rec.agents else None)
            if target is not None and ev.message:
                target.activity.append(ev.message)
    return run
```

**鲁棒性**: 抗 parallel/pipeline 并发 fan-out 的事件交错 — agent 按 `event.phase` 归 phase,按 label 归当前 running activity。

### 8.4 AbortSignal — pause/stop 二元区分

**文件**: `openjiuwen/agent_teams/workflow/engine/runtime.py:25-85`

```python
"""Cooperative abort flag that carries the control reason (pause vs stop)."""
reason: str = "pause"

def set(self, reason: str = "pause") -> None:
    ...
```

**3 种 abort reason**:
- **`pause`**: 暂停,journal 重放可恢复
- **`stop`**: 终止,从不重放
- **`early_return`**: 提前返回(用于 `early_return` 工具,允许 edit 后 re-run)

`journal_path = `<per-team, per-session>.jsonl`(file-based replay log) — same-process resume 时复用,不同 workflow 互不冲突(`runner.py:49-92`)。

### 8.5 SwarmFlow workflow.py — DAG 拓扑

**文件**: `jiuwenswarm/resources/agent/workspace/skills/swarmskill-creator/templates/scripts/workflow.py.template:11-19, 159-204`

```python
META = {
    "name": "<swarmskill-name>",
    "description": "<One-line description of the executable workflow>",
    "whenToUse": "<When an Agent should REUSE this workflow directly>",
    "phases": [
        {"title": "<Phase 1>", "detail": "<What this phase accomplishes>"},
        {"title": "<Phase 2>", "detail": "<What this phase accomplishes>"},
    ],
}

async def run(args):
    args = parse_args(args)

    phase("<Phase 1>")
    log("Starting <Phase 1>")
    raw_results = await parallel([
        lambda: agent(build_inline_prompt("<role-id-1>", args), label="<role-id-1>", phase="<Phase 1>", schema=ROLE_RESULT_SCHEMA),
        lambda: agent(build_inline_prompt("<role-id-2>", args), label="<role-id-2>", phase="<Phase 1>", schema=ROLE_RESULT_SCHEMA),
    ])
    role_results = compact([extract_json(r, fallback={"result": "extraction_failed", "verdict": "unknown"}) for r in raw_results])

    phase("<Phase 2>")
    log("Starting <Phase 2>")
    final_raw = await agent(build_inline_prompt("<integrator-role-id>", {"role_results": role_results, "original_args": args}),
                             label="integrate", phase="<Phase 2>", schema=ROLE_RESULT_SCHEMA)
    final = extract_json(final_raw, fallback={"result": "integration_failed", "verdict": "unknown"})

    return {
        "status": "complete" if has_meaningful_result(final) else "degraded",
        "role_results": role_results,
        "final": final,
    }
```

**DAG primitives**: `phase()` / `log()` / `parallel()` / `map_parallel()` / `pipeline()` / `compact()` / `flatten_filter()` / `human()` / `human_session()` / `agent_session()` / `budget()` / `workflow()`。

**`from swarmflow import ...`** 必须显式 import 全部用到的 primitive,禁止 `import *` / delayed imports — 这是 validator 强制的约束。

### 8.6 WorkflowPause / WorkflowStop 处理

**文件**: `openjiuwen/agent_teams/workflow/tool_swarmflow.py:596-650`

```python
except WorkflowAborted as exc:
    if exc.reason == "early_return":
        msg = self._format_early_return(exc.reply, exc.edit_hints, run_id=run_id)
        _publish(WorkflowProgressEvent(kind=ProgressKind.WORKFLOW_PAUSED, message="workflow paused for script edit"))
        raise BackendError(msg) from exc
    if exc.reason == "stop":
        _publish(WorkflowProgressEvent(kind=ProgressKind.WORKFLOW_STOPPED, message="workflow stopped"))
        msg = self._format_stopped(run_id=run_id)
        raise BackendError(msg) from exc
    # pause (default): silent cancel, controller relaunches on resume
    _publish(WorkflowProgressEvent(kind=ProgressKind.WORKFLOW_PAUSED, message="workflow paused"))
    raise asyncio.CancelledError() from exc
```

**3-reason 分支**:
1. **`early_return`** → `WORKFLOW_PAUSED` + BackendError(可恢复)
2. **`stop`** → `WORKFLOW_STOPPED` + BackendError(终止)
3. **`pause`** → `WORKFLOW_PAUSED` + `CancelledError`(静默 cancel, controller 自动 relaunch)

### 8.7 TodoToolkit — 4 态 task list

**文件**: `jiuwenswarm/agents/harness/common/tools/todo_toolkits.py:25-29`

```python
class TaskStatus(str, Enum):
    WAITING = "waiting"
    RUNNING = "running"
    COMPLETED = "completed"
    CANCELLED = "cancelled"
```

**4 态设计**: 比 claudecode(3 态)多了 `RUNNING`,也用 Markdown 文件持久化(`- [x] 1. xxx | completed | result`)。

### 8.8 PlanModeController — Plan 模式切换

**文件**: `jiuwenswarm/runtime/plan.py:39-281`

```python
class PlanModeController:
    """Own process-local plan state for the single Runtime lifecycle."""

    def __init__(self) -> None:
        self._sync_locks: WeakValueDictionary[str, asyncio.Lock] = WeakValueDictionary()
        self._exited_sessions: set[str] = set()  # 显式退出 plan 的 session
        self._active_sessions: set[str] = set()  # 进入 plan 的 session

    def reset_session(self, session_id: str) -> None:
        self._exited_sessions.discard(session_id)
        self._active_sessions.discard(session_id)

    async def ensure_state(self, request, mode, sub_mode, agent) -> PlanStateResult:
        # 1. resolve target_state = "plan" or "normal"
        target_state = "plan" if resolved.is_plan else "normal"
        # 2. open live state session
        deep_agent, session, live = await self.open_state_session(agent, request.session_id)
        state = deep_agent.load_state(session)
        previous_state = state.plan_mode.mode
        # 3. switch if changed
        if previous_state != target_state:
            deep_agent.switch_mode(session=session, mode=target_state)
            if target_state == "plan":
                self._active_sessions.add(session_id)
                self.inject_activation_reminder(request)
        return PlanStateResult(...)
```

**设计哲学**:
- `WeakValueDictionary[str, asyncio.Lock]` 让 lock 跟随 session 生命周期自动 GC。
- `previous_state != target_state` 才触发 switch_mode + commit,减少不必要 IO。
- `inject_activation_reminder` 注入 Plan mode 约束(等同 atomcode PlanModeReminderHook)。

### 8.9 状态机总结

```
     start / resume
          │
          ▼
   [WORKFLOW_STARTED] (events.WORKFLOW_STARTED)
          │
          ▼
   phase("<Phase 1>") → agent() × N (parallel) → phase("<Phase 2>") → ...
          │  events
          ▼
   [running] (WorkflowRun.status = "running", AgentActivity.status = "running")
          │
   ┌──────┼──────────┬──────────┐
   ▼      ▼          ▼          ▼
[WORKFLOW_COMPLETED] [WORKFLOW_FAILED] [WORKFLOW_PAUSED] [WORKFLOW_STOPPED]
   │                   │                  │                  │
   ▼                   ▼                  ▼                  ▼
status="completed"   status="failed"   status="paused"     status="stopped"
   │                   │                  │                  │
   ▼                   ▼                  ▼                  ▼
   return            propagate error    journal + relaunch   terminal
```

---

## 9. 七项目 Goal/Task/Plan 状态机综合对比维度表

### 9.1 主对比表

| 维度 | atomcode GoalState | claudecode TodoWrite | deepseek-harness Goal | openclaw SessionGoal | opencode plan/build | pi plan-mode ext | jiuwenswarm SwarmFlow |
|------|------------------|---------------------|---------------------|--------------------|--------------------|------------------|---------------------|
| **状态机类型** | 6-phase runtime | 3-state 派生态 | 4-phase + 2-activation 正交 | 6-state 业务+预算混合 | 2-mode(read-only/build) | 3-state extension | 6-ProgressKind + 4 层嵌套 |
| **状态枚举** | Pursuing/Paused/PausedAtCap/Satisfied/Ended + terminal Met/Stopped/Failed/Cancelled | pending/in_progress/completed | active/paused/blocked/complete + armed/disarmed | active/paused/blocked/usage_limited/budget_limited/complete | plan / build(双 Agent 名) | enabled/executing + PlanModeState | WORKFLOW_STARTED/COMPLETED/FAILED/PAUSED/STOPPED + PHASE/AGENT_STARTED/COMPLETED/FAILED |
| **持久化层** | 进程内存 (`Controller::GoalState`) + 进度快照 `progress_recap` | 文件 (transcript fold) | SQLite-style session log + goal/change events | SQLite session entry (JSON patch) | 文件 plan.md(`plans/{time}-{slug}.md`) | session entry (`appendEntry("plan-mode", ...)`) | 文件 journal (`<per-team, per-session>.jsonl`) + PlanModeController 进程内存 |
| **持久化原子** | progress_recap snapshot | last todowrite call fold | whole-value snapshot per mutation | SessionGoal 整体 patch | plan.md 文件 + agent name 切换 | PlanModeState struct | journal JSONL + ProgressEvent stream |
| **CAS / 版本化** | 无 (进程内唯一) | 无 (last-write-wins) | **有 (GoalRef.id+revision, CAS 守卫)** | 无 (last-write-wins) | 无 (按时间戳文件名) | 无 (extension append) | **有 (replay fold 严格校验 revision)** |
| **嵌套子状态** | 无 | 无 | 无 | 无 | 无 (plan 是 markdown) | `TodoItem[]` 内嵌 | **有 (Phase → AgentActivity → {prompt,activity,outcome})** |
| **并发状态** | 无 (单 goal) | 无 (单 todo list) | 无 (单 goal per session) | 无 (单 goal per session) | 无 (单 mode) | 无 (单 enabled flag) | **有 (parallel fan-out 鲁棒性 fold)** |
| **状态超时** | **有 (deadline = Instant::now() + duration)** | 无 | 无 | 无 (无 wall-clock cap) | 无 | 无 | 无 (journal resume 通过文件) |
| **状态回滚** | resume() 重置 round=0 + 刷新 deadline | 无 | 无 (clear tombstone) | 无 | 无 | 无 (append-only) | **有 (journal replay)** |
| **状态重做** | resume_paused() 保留 budget 但重置 cancel | 无 | 无 | 无 | 无 | 无 | **有 (rerun from journal)** |
| **计划生成** | N/A(goal is objective) | todowrite full-list | N/A | N/A | plan.md(`.opencode/plans/*.md`) | plan.md inline + extractTodoItems | workflow.py.template(script-only) |
| **计划审批流程** | N/A | N/A | 无 | 无 | **plan_exit tool + Question 询问 Yes/No** | ui.select("Execute \| Stay \| Refine") | human_session 轮次 |
| **计划执行模式** | 同 runtime(goal-round) | update id=N status=in_progress | goal-round-driver followup | 续轮(无 plan 概念) | **build agent 接管 plan 文件** | plan-mode + execute 模式 | **DAG (phase/parallel/pipeline)** |
| **计划修订/更新** | edit objective / retask | {action: "update", id, status} | edit (objective + maxGoalRounds) | updateSessionGoalObjective | edit plan.md(plan agent 仍可写) | extractTodoItems 重扫 + markCompletedSteps | workflow() 嵌套(限制 1 层) |
| **计划失败回退** | pause_for_recovery + recovery_context 注入 | 无 | goal-round-driver 自动 block(round-limit/queue-failed/prompt-rejected) | MODEL_UPDATABLE_SESSION_GOAL_STATUSES=["complete","blocked"](强制) | 无(失败回流靠 Question) | 无 | WORKFLOW_PAUSED + 重新 launch |
| **自动续轮** | 否(需用户提交) | 否 | **是 (goal-round-driver 自动 followup)** | 否 | 否 | 否 | 否 |
| **Token 预算** | 否 | 否 | 否 | **是 (tokenBudget 触发 budget_limited)** | 否 | 否 | **是 (budget.total / spent / remaining)** |
| **轮数预算** | **是 (max_rounds)** | 否 | **是 (maxGoalRounds=256)** | 否(continuationTurns 仅记) | 否 | 否 | 否 |
| **时间预算** | **是 (max_duration_secs + deadline)** | 否 | 否 | 否 | 否 | 否 | 否 |
| **预算窗口重置** | resume() 刷 deadline | N/A | edit(maxGoalRounds) | resume 重置 tokenStart/tokensUsed=0 | N/A | N/A | workflow run 不重置 |
| **distinct boundary** | active==(phase==Pursuing) 不变式 | exactly one in_progress | phase×activation 正交 | status:6 独立 | plan/build 全局工具白名单 | enabled/executing 互斥 | WorkflowRun.status 字符串 |
| **place 持久化层** | 不持久(只进程) | transcript fold | session log (whole-value) | session entry patch | plan.md 文件 | session entry append | journal file + plan.py state |
| **事件溯源** | 否 | 派生式 | **是 (goal/change events)** | 否 | 否 | 否 | **是 (WorkflowProgressEvent stream + journal)** |
| **空闲检测** | active flag | active = in_progress 数 = 1 | agent/status === 'idle' | 无 | 无 | agent_end 事件 | journal checkpoint raise |
| **race-fence** | CancellationToken | 无 | attempt = {queued/claimed/admitted} | 无 | Question/RejectedError | execute state | AbortSignal(reason 区分) |
| **跨进程恢复** | 否(纯进程) | 是(transcript) | **是(replay session events)** | 是(session entry) | 是(plan.md 文件) | 是(appendEntry) | **是(journal replay)** |
| **Tool 集成** | N/A(goal 是 runtime 对象) | todowrite 全 list 替换 | get_goal/create_goal/update_goal | get_goal/create_goal/update_goal | plan_exit + plan_enter | extension 注册 command | tool_swarmflow.py |
| **Authority 校验** | N/A | N/A | **direct-human vs goal-round** | model 只能 complete/blocked | permission system | toolsBeforePlanMode 白名单 | human vs agent |
| **CLI 入口** | `/goal` | todowrite | `/goal` + 工具 | `/goal` | `/plan` + plan_enter/exit | `/plan` + Ctrl+Alt+P | chat.swarmflow_reply |
| **文件存储** | 无 | transcript 派生 | session log (whole-value) | session entry JSON | plan.md | session entry custom | plan.py + journal |
| **Process-local 状态** | recovery_pause / cancel token | 无 | activation (armed/disarmed) | 无 | active flag | enabled flag | AbortSignal / PlanModeController |

### 9.2 借鉴价值评估

| 项目 | 借鉴优先级 | 核心借鉴点 | laew 改造路径 |
|------|-----------|-----------|--------------|
| **deepseek-harness** | **P0** | CAS GoalRef + armed/disarmed 正交 + 4 态 phase + maxGoalRounds | SQLite `goal_state` 表 + revision CAS |
| **openclaw** | **P0** | tokenBudget 触发 budget_limited + status timestamp | laew 加 token 预算 / plans token accounting |
| **atomcode** | **P0** | cap_reached 双预算 + recovery_context bounded + resume/pause/resume_paused 区分 | laew 多 Agent 协调的 budget |
| **claudecode** | P1 | stateless full-list-replace + exactly one in_progress | laew Plan Agent 输出替换为 Todowrite 协议 |
| **jiuwenswarm** | P1 | Phase→Agent→Activity 4 层 + WORKFLOW_* 6 态 + journal resume | laew Yolo 多 Agent 加 4 层折叠 |
| **opencode** | P1 | plan_exit 工具 + Question 询问审批 + 双 Agent 模式 | laew Plan Agent 后接 build 类似切换 |
| **pi** | P2 | steer/followUp 队列 + [DONE:n] 进度标记 | laew SubAgent 输出支持进度回调 |

---

## 10. 七项目状态机 ASCII 转换图

### 10.1 atomcode GoalState

```
                ┌─ cap_reached = None ─────────────────────┐
                │                                          │
                ▼                                          │
         ┌──────────────┐                                  │
    ┌───▶│  Pursuing    │◀── resume() ──────────────┐      │
    │    │ (active=true)│                           │      │
    │    └──────┬───────┘                           │      │
    │           │                                   │      │
    │           │ mark_satisfied(Met)               │      │
    │           ▼                                   │      │
    │    ┌──────────────┐                           │      │
    │    │  Satisfied   │ (terminal, UI hide)       │      │
    │    └──────────────┘                           │      │
    │                                               │      │
    │    ┌──────────────┐   cap_reached="round"      │      │
    │    │  Paused      │   pause("user")            │      │
    │    │ (active=false│                            │      │
    │    │  terminal=   │   pause_at_cap             │      │
    │    │   None)      │   pause_for_recovery       │      │
    │    └──────┬───────┘                            │      │
    │           │ resume_paused()                    │      │
    │           └────────────────────────────────────┘      │
    │                                                      │
    │           pause_at_cap / pause_for_recovery         │
    │           ┌──────────────────────┐                   │
    │           │   PausedAtCap        │                   │
    │           │   (active=false      │                   │
    │           │    terminal=Stopped) │                   │
    │           └──────┬───────────────┘                   │
    │                  │ resume(new_max_rounds)            │
    │                  └────────────────────────────────── ┘ (回 Pursuing, round=0)
    │
    │  pause / stop / cancel → terminal 一律 Ended
    ▼
  ┌──────────────┐
  │   Ended      │ (terminal, UI hide)
  └──────────────┘
```

### 10.2 claudecode TodoWrite

```
  ┌─────────┐  full-list todos  ┌──────────┐
  │ (none)  │ ─────────────────▶│ Pending  │
  └─────────┘   (one or more)   └────┬─────┘
                                     │ action="update" id=N status=in_progress
                                     ▼
                              ┌──────────────┐
                              │ in_progress  │ (exactly one)
                              └──────┬───────┘
                                     │ action="update" id=N status=completed
                                     ▼
                              ┌──────────────┐
                              │  Completed   │
                              └──────────────┘

  ※ {action: "update", id: K, status: in_progress} → 自动把其他 in_progress 置回 Pending
  ※ 全局不变式: any time 恰好 1 个 in_progress
```

### 10.3 deepseek-harness Goal

```
                       ┌────────────────┐
                       │  (no goal)     │
                       └────────┬───────┘
                                │ create()
                                ▼
        ┌────────────────────────────────────────┐
        │         phase='active'                 │◀────────┐
        │   activation='armed' (process-local)   │         │ resume() from
        │   roundsStarted=0..maxGoalRounds       │         │ paused/blocked
        │   revision=1..N                        │         │
        └────┬──────────────────┬────────────────┘         │
             │                  │                        │
   pause()   │                  │  block(reason)         │
             ▼                  ▼                        │
    ┌────────────────┐  ┌────────────────┐               │
    │ phase='paused' │  │ phase='blocked'│               │
    │ activation=    │  │ activation=    │               │
    │  'disarmed'    │  │  'disarmed'    │               │
    └───────┬────────┘  └────────┬───────┘               │
            │                    │                       │
            └────────┬───────────┘                       │
                     │ resume()                          │
                     └───────────────────────────────────┘
                     │ complete()
                     ▼
              ┌────────────────┐
              │ phase='complete'│
              │ activation=    │
              │  'disarmed'    │
              └────────┬───────┘
                       │ clear()
                       ▼
              ┌────────────────────────┐
              │ (no current goal)      │
              │  tombstone={id,rev+1}  │
              └────────────────────────┘
```

### 10.4 openclaw SessionGoal

```
   ┌──────────────────────────────────────────────────────────────┐
   │ 业务态                                                       │
   │                                                              │
   │    ┌─────────┐  create      ┌─────────┐  pause()   ┌─────────┐
   │    │ (none)  │─────────────▶│ active  │───────────▶│ paused  │
   │    └─────────┘              └────┬────┘            └────┬────┘
   │                                │                       │
   │                                │ block()               │ resume()
   │                                ▼                       │ + 预算窗口重置
   │                          ┌───────────┐                  │
   │                          │ blocked   │                  │
   │                          └─────┬─────┘                  │
   │                                │ resume()               │
   │                                ▼                        │
   │                          ┌───────────┐                  │
   │                          │  active   │◀─────────────────┘
   │                          └─────┬─────┘
   │                                │ complete()
   │                                ▼
   │                          ┌───────────┐
   │                          │ complete  │ (terminal)
   │                          └───────────┘
   │
   │ 资源态(自动触发)
   │
   │    active ──tokensUsed≥tokenBudget──▶ budget_limited
   │    budget_limited ──resume()──▶ active (预算窗口重置)
   │
   │ 关键不变式:
   │ - tokensUsed = max(goal.tokensUsed, totalTokens - tokenStart)
   │ - 每次 accountSessionGoalUsage 自动迁移 active → budget_limited
   │ - resume 重置 tokenStart = freshTotal, tokensUsed = 0
```

### 10.5 opencode plan/build 双 Agent

```
   ┌──────────────┐  plan_enter tool      ┌──────────────┐
   │ build agent  │ ──────────────────────▶│ plan agent   │
   │ (default,    │ ◀──────────────────────│ (read-only,  │
   │  full perms) │   plan_exit tool +    │  allow plan  │
   │              │   Question "Yes"      │  file edit)  │
   └──────────────┘                        └──────────────┘

   plan agent 权限:
   - edit: { "*": "deny", ".opencode/plans/*.md": "allow" }
   - task: { general: "deny" }  ← 不能 spawn subagent
   - bash, read, webfetch 等全允许

   build agent 权限:
   - 全允许
   - plan_enter: allow (可主动进入 plan)

   plan_exit tool 行为:
   - Question "Plan at X is complete. Switch to build agent?"
   - Yes → 创建新 user message, agent = "build"
   - No → Question.RejectedError, 留在 plan

   切换后:
   - plan agent 最后一条 assistant 消息 → 当前 build agent
   - 注入 BUILD_SWITCH reminder: "A plan file exists at X. Execute on the plan."
```

### 10.6 pi plan-mode extension

```
   ┌────────────────┐  /plan or Ctrl+Alt+P   ┌──────────────────┐
   │  (no plan mode)│ ───────────────────────▶│ plan mode enabled │
   │                │                        │  tool set:       │
   │                │                        │  [read,bash*,...] │
   │                │                        │  edit/write:OFF  │
   └────────────────┘                        │  inject reminder │
        ▲                                    └─────────┬────────┘
        │                                              │
        │                                              │ agent_end
        │                                              │ → extractTodoItems
        │                                              │ → ui.select
        │                                              │
        │                              ┌───────────────┼───────────────┐
        │                              ▼               ▼               ▼
        │                         "Execute"       "Stay"           "Refine"
        │                              │               │               │
        │                              ▼               ▼               ▼
        │                       ┌─────────────┐  (stay in plan    send refinement
        │                       │  executing  │   mode)            via followUp
        │                       │  enabled=F  │
        │                       │  tool set=   │
        │                       │  normal     │
        │                       └──────┬──────┘
        │                              │ turn_end: scan [DONE:n]
        │                              │ all complete → "Plan Complete!"
        │                              │ executing=F, enabled=F
        │                              ▼
        └──────────────────────────────┘
                                          
   持久化:
   - appendEntry("plan-mode", { enabled, todos, executing, toolsBeforePlanMode })
   - resume: entries.filter(type=custom, customType="plan-mode").pop()
```

### 10.7 jiuwenswarm SwarmFlow

```
       workflow.start
            │
            ▼
   ┌─────────────────┐  events: WORKFLOW_STARTED
   │ running         │  phase("P1") → agent()×N (parallel)
   │                 │  phase("P2") → agent() integrate
   │ status="running"│  ...
   │ status per agent│
   │   "running"     │
   └────┬──────┬─────┘
        │      │      events
        │      │  ┌──────────────────────────────────────┐
        │      ▼  ▼                                      │
        │   ┌─────────┐  ┌──────────┐  ┌──────────┐  ┌─────────┐
        │   │completed│  │ failed   │  │ paused   │  │ stopped │
        │   └─────────┘  └──────────┘  └─────┬────┘  └─────────┘
        │      │              │              │
        │      ▼              ▼              ▼
        │   return        propagate       journal save
        │   normal        error           + relaunch
        │                                   │
        │      ┌────────────────────────────┘
        │      │
        │      ▼
        │   WorkflowRun.status = "paused"
        │   4 层折叠: WorkflowRun → Phase → AgentActivity → {prompt,activity,outcome}
        │
        └─→ abort reasons:
            - pause (default): silent cancel, journal relaunch
            - stop: terminal, never relaunch
            - early_return: edit & re-run (BackendError)
```

### 10.8 七项目状态机语义对比总图

```
  项目       单 goal 数 嵌套  并发  轮数  时间  token  审批  续轮  持久  事件溯源
  atomcode      1       no   no    yes  yes   no    no    no    no    no
  claudecode    1       no   no    no   no    no    no    no    yes   no(派生)
  deepseek      1       no   no    yes  no    no    no    YES  yes   YES
  openclaw      1       no   no    no   no    yes   no    no    yes   no
  opencode      1       no   no    no   no    no    yes   no    yes   no
  pi            1       no   no    no   no    no    yes   no    yes   no
  jiuwenswarm   N       YES  YES   no   no    yes   no    no    yes   YES
```

---

## 11. laew 现状与 P0 借鉴路线图（事务式 Goal 状态机迁移）

### 11.1 laew 当前的 Goal 处理(非状态机方式)

**CLAUDE.md / AGENTS.md 注明**:

```
- **多 Agent 架构(6 角色)**:
  - Yolo Agent: 入口层,负责目标识别 / 意图识别(每条输入先做 目的→目标→意图 三步分析)/ 
    任务三档分类(simple/medium/hard)/ 失败回流与用户建议;仅持 Read 工具。
  - Plan Agent: 规划层,仅在 hard 任务时启用;持 Read/Write 工具,
    输出 Markdown 方案到 `plans/{session_id}-{seq}.md`。
```

**实际代码 `src/agent/plan.rs:38-79`**:

```rust
pub async fn generate(
    &self,
    goal: &str,
    purpose: &str,
    intent: &str,
    decomposition: &[String],
    session_id: &str,
) -> Result<PlanOutput> {
    std::fs::create_dir_all(&self.plans_dir)?;
    let prompt = format!(
        "【Plan 任务】\n\
         Session: {session_id}\n\
         目的: {purpose}\n\
         目标: {goal}\n\
         意图: {intent}\n\
         分解步骤:\n{decomp}\n\n\
         请按系统提示词中的 Markdown 模板输出方案(完整五段)。",
        decomp = decomposition.iter()
            .enumerate()
            .map(|(i, s)| format!("  {}. {}", i + 1, s))
            .collect::<Vec<_>>().join("\n"),
    );
    let mut sub_session = session::Session::new();
    sub_session.context_mut().push(ChatMessage::user(&prompt));
    sub_session.id = session_id.to_string();
    let (text, usage) = self.agent.run_session(&mut sub_session).await?;
    let seq = self.db.next_session_seq(session_id)?;
    let path = self.plans_dir.join(format!("{}-{}.md", session_id, seq));
    std::fs::write(&path, &text)?;
    // 写 Agent-Memory (后续)
    Ok(PlanOutput { path, markdown: text })
}
```

**问题分析**:

| 痛点 | 现状 | 影响 |
|------|------|------|
| **Plan 不持久** | 仅落盘 `plans/{session_id}-{seq}.md`,**没有运行时对象** | 无法查询「Plan 执行到哪一步」 |
| **SubAgent 失败不回退** | Quality-Check 失败 → 回到 Main-Work 重拆 → 永远重新执行 | 无 cap,无限循环风险 |
| **无法跨 Session 恢复** | `db.next_session_seq` 每次 +1,**永不重用** | Session 重启后 Plan 上下文丢失 |
| **Plan 不关联 SubAgent 产物** | `plans/X.md` 与 SubAgent 实际执行无绑定 | 用户读 plan 看不到执行结果 |
| **没有预算** | 无 token 预算、无轮数预算、无时间预算 | 单任务可能耗尽上下文 |
| **三档分类无生命周期** | Yolo 分类是一次性动作 | 无「Plan 中途从 hard 降级为 medium」的动态调整 |
| **失败回流靠用户** | Yolo 「失败回流与用户建议」 → 提示用户重新输入 | 无自动 retry / escalate / fallback |

### 11.2 P0 路线图:Goal 状态机引入

#### 11.2.1 数据库 schema 扩展

```sql
-- 新增 goal_state 表
CREATE TABLE goal_state (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  goal_id TEXT NOT NULL UNIQUE,  -- 全局 UUID
  revision INTEGER NOT NULL DEFAULT 1,  -- CAS 守卫
  objective TEXT NOT NULL,
  phase TEXT NOT NULL CHECK(phase IN ('active','paused','blocked','complete')),
  activation TEXT NOT NULL CHECK(activation IN ('armed','disarmed')) DEFAULT 'disarmed',
  max_rounds INTEGER NOT NULL DEFAULT 256,
  rounds_started INTEGER NOT NULL DEFAULT 0,
  max_duration_secs INTEGER,
  deadline_at INTEGER,  -- epoch ms
  token_budget INTEGER,
  tokens_used INTEGER NOT NULL DEFAULT 0,
  blocked_reason TEXT,  -- lower-kebab-case code + ':' + message
  plan_md_path TEXT,  -- 关联 plans/{session_id}-{seq}.md
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX idx_goal_session ON goal_state(session_id);

-- 事件日志(可选,用于 replay)
CREATE TABLE goal_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  goal_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  operation TEXT NOT NULL CHECK(operation IN ('create','edit','pause','resume','complete','block','clear')),
  payload TEXT NOT NULL,  -- JSON snapshot
  created_at INTEGER NOT NULL
);

CREATE INDEX idx_goal_events ON goal_events(goal_id, revision);
```

#### 11.2.2 GoalState Rust 数据结构

```rust
// src/agent/goal.rs
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalActivation {
    Armed,    // 自动续轮
    Disarmed, // 进程本地 / 等待人为
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalTerminal {
    Met,
    Stopped,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalRef {
    pub goal_id: String,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalState {
    pub goal_id: String,
    pub revision: i64,
    pub session_id: String,
    pub objective: String,
    pub phase: GoalPhase,
    pub activation: GoalActivation,
    pub max_rounds: i64,
    pub rounds_started: i64,
    pub max_duration_secs: Option<i64>,
    pub deadline_at: Option<i64>,  // epoch ms
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub blocked_reason: Option<String>,
    pub plan_md_path: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl GoalState {
    pub fn cap_reached(&self, now_ms: i64) -> Option<&'static str> {
        if self.rounds_started >= self.max_rounds {
            return Some("round limit");
        }
        if let Some(deadline) = self.deadline_at {
            if now_ms >= deadline {
                return Some("time limit");
            }
        }
        None
    }
    
    /// CAS 守卫:检查 ref 是否匹配当前 revision
    pub fn verify_cas(&self, r: &GoalRef) -> Result<(), AgentError> {
        if r.goal_id != self.goal_id || r.revision != self.revision {
            return Err(AgentError::StaleGoalRef {
                expected: r.clone(),
                actual: GoalRef { goal_id: self.goal_id.clone(), revision: self.revision },
            });
        }
        Ok(())
    }
    
    pub fn pause(&mut self, now_ms: i64) -> Result<(), AgentError> {
        if self.phase != GoalPhase::Active {
            return Err(AgentError::InvalidTransition { from: self.phase, op: "pause" });
        }
        self.phase = GoalPhase::Paused;
        self.activation = GoalActivation::Disarmed;
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
    
    pub fn resume(&mut self, new_max_rounds: Option<i64>, now_ms: i64) -> Result<(), AgentError> {
        if !matches!(self.phase, GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked) {
            return Err(AgentError::InvalidTransition { from: self.phase, op: "resume" });
        }
        if self.phase == GoalPhase::Active && self.activation == GoalActivation::Armed {
            return Err(AgentError::AlreadyActive);
        }
        if self.rounds_started >= self.max_rounds {
            return Err(AgentError::RoundBudgetExhausted);
        }
        if let Some(new_max) = new_max_rounds {
            self.max_rounds = new_max;
        }
        // 刷 deadline(若有时间 cap)
        if let Some(d) = self.max_duration_secs {
            self.deadline_at = Some(now_ms + d * 1000);
        }
        self.phase = GoalPhase::Active;
        self.activation = GoalActivation::Armed;
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
    
    pub fn complete(&mut self, now_ms: i64) -> Result<(), AgentError> {
        if !matches!(self.phase, GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked) {
            return Err(AgentError::InvalidTransition { from: self.phase, op: "complete" });
        }
        self.phase = GoalPhase::Complete;
        self.activation = GoalActivation::Disarmed;
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
    
    pub fn block(&mut self, reason: String, now_ms: i64) -> Result<(), AgentError> {
        if self.phase != GoalPhase::Active {
            return Err(AgentError::InvalidTransition { from: self.phase, op: "block" });
        }
        // lower-kebab-case code + ':' + message
        self.phase = GoalPhase::Blocked;
        self.activation = GoalActivation::Disarmed;
        self.blocked_reason = Some(reason);
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
}
```

#### 11.2.3 GoalService — Rust 版 deepseek 风格

```rust
// src/agent/goal_service.rs
use crate::config::Db;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct GoalService {
    db: Arc<Db>,
}

impl GoalService {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
    
    /// 4 步:解析 → 创建/获取 → 操作 → 持久化
    pub async fn create(
        &self,
        session_id: &str,
        objective: String,
        max_rounds: Option<i64>,
        max_duration_secs: Option<i64>,
    ) -> Result<GoalState, AgentError> {
        let now = chrono::Utc::now().timestamp_millis();
        let goal_id = format!("goal-{}", uuid::Uuid::new_v4());
        let goal = GoalState {
            goal_id: goal_id.clone(),
            revision: 1,
            session_id: session_id.to_string(),
            objective,
            phase: GoalPhase::Active,
            activation: GoalActivation::Armed,  // 创建即 armed
            max_rounds: max_rounds.unwrap_or(256),
            rounds_started: 0,
            max_duration_secs,
            deadline_at: max_duration_secs.map(|d| now + d * 1000),
            token_budget: None,
            tokens_used: 0,
            blocked_reason: None,
            plan_md_path: None,
            created_at: now,
            updated_at: now,
        };
        self.db.insert_goal(&goal).await?;
        Ok(goal)
    }
    
    /// CAS 守卫的通用 mutation
    pub async fn apply<F>(&self, ref_: &GoalRef, mutator: F) -> Result<GoalState, AgentError>
    where F: FnOnce(&mut GoalState) -> Result<(), AgentError>,
    {
        let mut goal = self.db.get_goal(&ref_.goal_id).await?
            .ok_or(AgentError::GoalNotFound(ref_.goal_id.clone()))?;
        goal.verify_cas(ref_)?;
        mutator(&mut goal)?;
        self.db.update_goal(&goal).await?;
        Ok(goal)
    }
    
    pub async fn pause(&self, ref_: &GoalRef) -> Result<GoalState, AgentError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.apply(ref_, |g| g.pause(now)).await
    }
    
    pub async fn resume(&self, ref_: &GoalRef, new_max_rounds: Option<i64>) -> Result<GoalState, AgentError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.apply(ref_, |g| g.resume(new_max_rounds, now)).await
    }
    
    pub async fn complete(&self, ref_: &GoalRef) -> Result<GoalState, AgentError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.apply(ref_, |g| g.complete(now)).await
    }
    
    pub async fn block(&self, ref_: &GoalRef, reason: String) -> Result<GoalState, AgentError> {
        let now = chrono::Utc::now().timestamp_millis();
        self.apply(ref_, |g| g.block(reason, now)).await
    }
    
    /// SubAgent 完成一轮后调用:rounds_started++
    pub async fn advance_round(&self, ref_: &GoalRef) -> Result<GoalState, AgentError> {
        self.apply(ref_, |g| {
            if g.phase != GoalPhase::Active {
                return Err(AgentError::InvalidTransition { from: g.phase, op: "advance_round" });
            }
            g.rounds_started += 1;
            g.revision += 1;
            g.updated_at = chrono::Utc::now().timestamp_millis();
            // 预算触发自动 block
            if let Some(reason) = g.cap_reached(g.updated_at) {
                g.block(reason.into(), g.updated_at)?;
            }
            Ok(())
        }).await
    }
}
```

#### 11.2.4 YoloRunner 集成 Goal 状态机

```rust
// src/agent/yolo.rs 扩展
impl YoloRunner {
    pub async fn run_with_goal(
        &self,
        session_id: &str,
        user_input: &str,
    ) -> Result<YoloOutput, AgentError> {
        // 1. 三步分析(目的→目标→意图)+ 三档分类
        let classification = self.classify(user_input).await?;
        
        // 2. 根据档位路由(原逻辑)
        match classification.level {
            TaskLevel::Simple => self.run_simple(&classification, session_id).await,
            TaskLevel::Medium => self.run_medium(&classification, session_id).await,
            TaskLevel::Hard => {
                // 关键:hard 档必须先有 current goal
                let current_goal = self.goal_service.get_current(session_id).await?;
                let goal = match current_goal {
                    Some(g) if g.phase == GoalPhase::Active => g,
                    Some(g) if g.phase == GoalPhase::Paused => {
                        // 提示用户 resume
                        return Ok(YoloOutput::needs_resume(g));
                    }
                    Some(g) if g.phase == GoalPhase::Complete => {
                        // 新 goal 替换
                        let g = self.goal_service.create(session_id, classification.goal.clone(), None, None).await?;
                        g
                    }
                    Some(g) if g.phase == GoalPhase::Blocked => {
                        return Ok(YoloOutput::blocked(g));
                    }
                    None => {
                        let g = self.goal_service.create(session_id, classification.goal.clone(), None, None).await?;
                        g
                    }
                };
                
                // 3. Plan Agent 生成 plans/{session_id}-{seq}.md
                let plan_output = self.plan_runner.generate(...).await?;
                
                // 4. 把 plan_md_path 绑定到 goal
                self.goal_service.update_plan_path(&GoalRef { goal_id: goal.goal_id.clone(), revision: goal.revision }, plan_output.path.clone()).await?;
                
                // 5. 转入 Main-Work 拆 SubAgent
                self.run_hard(&classification, session_id, &goal, &plan_output).await
            }
        }
    }
}
```

#### 11.2.5 run_hard 集成 GoalRef 跟踪

```rust
impl MultiAgentOrchestrator {
    pub async fn run_hard(
        &self,
        classification: &TaskClassification,
        session_id: &str,
        goal: &GoalState,
        plan_output: &PlanOutput,
    ) -> Result<OrchestratorOutput, AgentError> {
        let goal_ref = GoalRef { goal_id: goal.goal_id.clone(), revision: goal.revision };
        
        // 1. Main-Work 拆 WorkFlow
        let workflows = self.main_work.decompose(...).await?;
        
        let mut outputs = Vec::new();
        for (idx, workflow) in workflows.iter().enumerate() {
            // 2. 每步 SubAgent 执行
            let sub_output = self.sub_agent.execute(workflow).await?;
            
            // 3. Quality-Check
            let qc = self.qc.check(&sub_output).await?;
            if !qc.pass {
                // 失败回流:调用 goal.block(reason) 自动 disarmed
                let reason = format!("qc-fail-{}: {}", idx, qc.reason);
                let goal = self.goal_service.block(&goal_ref, reason).await?;
                return Ok(OrchestratorOutput::blocked(goal));
            }
            
            // 4. 成功:goal.advance_round(&goal_ref)
            let updated = self.goal_service.advance_round(&goal_ref).await?;
            
            // 5. CAS check: 如果 revision mismatch(并发修改), reject
            if updated.revision != goal.revision + (idx as i64 + 1) {
                return Err(AgentError::StaleGoalRef { ... });
            }
            
            outputs.push(sub_output);
            
            // 6. cap 触达检查
            if let Some(reason) = updated.cap_reached(updated.updated_at) {
                let goal = self.goal_service.block(&updated.goal_ref(), reason.into()).await?;
                return Ok(OrchestratorOutput::needs_resume(goal));
            }
        }
        
        // 7. 全部成功:goal.complete()
        let final_goal = self.goal_service.complete(&goal_ref).await?;
        Ok(OrchestratorOutput::complete(final_goal, outputs))
    }
}
```

### 11.3 P0 改造优先级

| 阶段 | 任务 | 工作量 | 依赖 |
|------|------|-------|------|
| **P0-1** | SQLite schema 扩展(goal_state + goal_events 表) | 1 天 | - |
| **P0-2** | `GoalState` 数据结构 + CAS 守卫(纯函数) | 1 天 | - |
| **P0-3** | `GoalService` + DB CRUD | 2 天 | P0-1, P0-2 |
| **P0-4** | YoloRunner 集成 Goal(create on hard / resume on paused) | 1 天 | P0-3 |
| **P0-5** | PlanRunner 绑定 plan_md_path 到 goal | 0.5 天 | P0-4 |
| **P0-6** | SubAgent 完成 → goal.advance_round + cap_reached 检查 | 1 天 | P0-5 |
| **P0-7** | Quality-Check 失败 → goal.block(reason) | 0.5 天 | P0-6 |
| **P0-8** | SessionContext 收口 → goal.complete() | 0.5 天 | P0-7 |
| **P1** | token 预算(openclaw 风格) | 2 天 | P0-3 |
| **P1** | plan_exit 审批(opencode 风格) | 2 天 | P0-3 |
| **P1** | goal-round 自动续轮(deepseek 风格) | 3 天 | P0-3 |
| **P2** | journal resume(jiuwenswarm 风格) | 5 天 | P0-3 |

### 11.4 借鉴矩阵

```
                       laew 当前           借鉴项目              借鉴价值
                       ─────────           ──────────            ──────────
计划生成                plans/*.md         opencode plan.md      ★★★★ (P0)
                                              claudecode TodoWrite ★★★ (P1)
状态机                  无                  deepseek-harness Goal ★★★★★ (P0)
                                              atomcode GoalState   ★★★★ (P0)
                                              openclaw SessionGoal ★★★★ (P0)
CAS 守卫                无                  deepseek-harness Goal ★★★★★ (P0)
持久化                  SQLite session     deepseek-harness Goal ★★★ (P1)
                                              openclaw session entry ★★★ (P1)
状态超时                无                  atomcode deadline     ★★★★ (P0)
状态回滚                无                  atomcode resume_paused ★★★★ (P0)
                                              deepseek-harness clear ★★★ (P1)
审批流程                无                  opencode plan_exit    ★★★★ (P0)
                                              pi ui.select         ★★★ (P1)
执行模式                过程式              deepseek goal-round-driver ★★★★★ (P0)
                                              openclaw budget_limited ★★★★ (P0)
失败回流                用户重新输入         atomcode recovery_context ★★★★★ (P0)
                                              deepseek-harness block ★★★★ (P0)
嵌套子状态              无                  jiuwenswarm 4 层折叠  ★★★ (P1)
事件溯源                无                  deepseek-harness goal/change ★★ (P2)
                                              jiuwenswarm journal ★★ (P2)
进度标记                无                  pi [DONE:n]           ★★ (P2)
扩展 hook               无                  pi plan-mode ext      ★★★ (P1)
                                              atomcode PlanModeReminder ★★★★ (P0)
```

---

## 12. laew Goal 状态机 MiniPoC 代码蓝图

### 12.1 完整 Cargo.toml 依赖

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }
thiserror = "1"
async-trait = "0.1"
```

### 12.2 src/agent/goal.rs(完整 200 行)

```rust
//! Goal 状态机:借鉴 deepseek-harness(4 态 + CAS) + atomcode(双 cap) + openclaw(timestamp)。
//!
//! 设计哲学:
//! - **持久化 phase** + **进程本地 activation** 正交(同 deepseek)
//! - **CAS GoalRef** 防止并发修改(同 deepseek)
//! - **round + time 双 cap**(atomcode 风格)
//! - **recovery_context** 字段,失败时为 resume 提供 bounded context(atomcode 风格)
//! - **blocked_reason** lower-kebab-case code(同 deepseek)

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalActivation {
    Armed,    // 自动续轮
    Disarmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalTerminal {
    Met,
    Stopped,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Error, Serialize, Deserialize)]
pub enum GoalError {
    #[error("goal not found: {0}")]
    NotFound(String),
    #[error("stale goal ref: expected {expected:?}, actual {actual:?}")]
    StaleRef {
        expected: GoalRef,
        actual: GoalRef,
    },
    #[error("invalid transition: cannot {op} in phase {from:?}")]
    InvalidTransition {
        from: GoalPhase,
        op: &'static str,
    },
    #[error("goal already active and armed")]
    AlreadyActive,
    #[error("round budget exhausted ({max_rounds} rounds)")]
    RoundBudgetExhausted { max_rounds: i64 },
    #[error("invalid objective: must be non-empty")]
    InvalidObjective,
    #[error("invalid max_rounds: must be positive")]
    InvalidMaxRounds,
    #[error("invalid blocked_reason: must be lower-kebab-case code:message")]
    InvalidBlockedReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRef {
    pub goal_id: String,
    pub revision: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalState {
    pub goal_id: String,
    pub revision: i64,
    pub session_id: String,
    pub objective: String,
    pub phase: GoalPhase,
    pub activation: GoalActivation,
    pub max_rounds: i64,
    pub rounds_started: i64,
    pub max_duration_secs: Option<i64>,
    pub deadline_at: Option<i64>,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub blocked_reason: Option<String>,
    pub plan_md_path: Option<String>,
    pub recovery_context: Option<String>,  // bounded 6000 字符(atomcode 风格)
    pub created_at: i64,
    pub updated_at: i64,
}

impl GoalState {
    pub fn new(
        session_id: String,
        objective: String,
        max_rounds: Option<i64>,
        max_duration_secs: Option<i64>,
        token_budget: Option<i64>,
    ) -> Result<Self, GoalError> {
        if objective.trim().is_empty() {
            return Err(GoalError::InvalidObjective);
        }
        let max_rounds = max_rounds.unwrap_or(256);
        if max_rounds < 1 {
            return Err(GoalError::InvalidMaxRounds);
        }
        let now = Utc::now().timestamp_millis();
        Ok(Self {
            goal_id: format!("goal-{}", uuid::Uuid::new_v4()),
            revision: 1,
            session_id,
            objective,
            phase: GoalPhase::Active,
            activation: GoalActivation::Armed,  // 创建即 armed
            max_rounds,
            rounds_started: 0,
            max_duration_secs,
            deadline_at: max_duration_secs.map(|d| now + d * 1000),
            token_budget,
            tokens_used: 0,
            blocked_reason: None,
            plan_md_path: None,
            recovery_context: None,
            created_at: now,
            updated_at: now,
        })
    }
    
    pub fn ref_(&self) -> GoalRef {
        GoalRef { goal_id: self.goal_id.clone(), revision: self.revision }
    }
    
    pub fn verify_cas(&self, r: &GoalRef) -> Result<(), GoalError> {
        if r.goal_id != self.goal_id || r.revision != self.revision {
            return Err(GoalError::StaleRef { expected: r.clone(), actual: self.ref_() });
        }
        Ok(())
    }
    
    pub fn cap_reached(&self, now_ms: i64) -> Option<&'static str> {
        if self.rounds_started >= self.max_rounds {
            return Some("round limit");
        }
        if let Some(deadline) = self.deadline_at {
            if now_ms >= deadline {
                return Some("time limit");
            }
        }
        if let Some(budget) = self.token_budget {
            if self.tokens_used >= budget {
                return Some("token limit");
            }
        }
        None
    }
    
    pub fn pause(&mut self, now_ms: i64) -> Result<(), GoalError> {
        if self.phase != GoalPhase::Active {
            return Err(GoalError::InvalidTransition { from: self.phase, op: "pause" });
        }
        self.phase = GoalPhase::Paused;
        self.activation = GoalActivation::Disarmed;
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
    
    pub fn resume(&mut self, new_max_rounds: Option<i64>, now_ms: i64) -> Result<(), GoalError> {
        if !matches!(self.phase, GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked) {
            return Err(GoalError::InvalidTransition { from: self.phase, op: "resume" });
        }
        if self.phase == GoalPhase::Active && self.activation == GoalActivation::Armed {
            return Err(GoalError::AlreadyActive);
        }
        if self.rounds_started >= self.max_rounds {
            return Err(GoalError::RoundBudgetExhausted { max_rounds: self.max_rounds });
        }
        if let Some(new_max) = new_max_rounds {
            self.max_rounds = new_max;
        }
        // 刷 deadline(若有时间 cap)
        if let Some(d) = self.max_duration_secs {
            self.deadline_at = Some(now_ms + d * 1000);
        }
        // resume 时重置 token 窗口(openclaw 风格)
        if self.token_budget.is_some() {
            self.tokens_used = 0;
        }
        self.phase = GoalPhase::Active;
        self.activation = GoalActivation::Armed;
        self.recovery_context = None;  // 清空 recovery 上下文
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
    
    pub fn complete(&mut self, now_ms: i64) -> Result<(), GoalError> {
        if !matches!(self.phase, GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked) {
            return Err(GoalError::InvalidTransition { from: self.phase, op: "complete" });
        }
        self.phase = GoalPhase::Complete;
        self.activation = GoalActivation::Disarmed;
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
    
    pub fn block(&mut self, code: String, message: String, now_ms: i64) -> Result<(), GoalError> {
        if self.phase != GoalPhase::Active {
            return Err(GoalError::InvalidTransition { from: self.phase, op: "block" });
        }
        // lower-kebab-case 校验
        if !is_lower_kebab(&code) {
            return Err(GoalError::InvalidBlockedReason);
        }
        self.phase = GoalPhase::Blocked;
        self.activation = GoalActivation::Disarmed;
        self.blocked_reason = Some(format!("{code}:{message}"));
        self.updated_at = now_ms;
        self.revision += 1;
        Ok(())
    }
    
    pub fn advance_round(&mut self, now_ms: i64) -> Result<Option<String>, GoalError> {
        if self.phase != GoalPhase::Active {
            return Err(GoalError::InvalidTransition { from: self.phase, op: "advance_round" });
        }
        self.rounds_started += 1;
        self.updated_at = now_ms;
        self.revision += 1;
        // 自动 cap 触达检查
        if let Some(reason) = self.cap_reached(now_ms) {
            return Ok(Some(reason.to_string()));
        }
        Ok(None)
    }
    
    pub fn disarm(&mut self, now_ms: i64) {
        // 仅修改 activation,不动 phase / revision
        self.activation = GoalActivation::Disarmed;
        self.updated_at = now_ms;
    }
    
    pub fn set_recovery_context(&mut self, ctx: String) {
        // bounded 6000 字符(atomcode 风格)
        let truncated = if ctx.chars().count() > 6000 {
            ctx.chars().take(6000).collect::<String>() + "\n[truncated]"
        } else {
            ctx
        };
        self.recovery_context = Some(truncated);
    }
    
    pub fn set_plan_path(&mut self, path: String, now_ms: i64) {
        self.plan_md_path = Some(path);
        self.updated_at = now_ms;
        self.revision += 1;
    }
    
    pub fn token_accumulate(&mut self, used: i64) {
        self.tokens_used += used;
    }
}

fn is_lower_kebab(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return false;
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_goal_is_active_and_armed() {
        let g = GoalState::new("s1".into(), "fix bug".into(), None, None, None).unwrap();
        assert_eq!(g.phase, GoalPhase::Active);
        assert_eq!(g.activation, GoalActivation::Armed);
        assert_eq!(g.revision, 1);
        assert_eq!(g.max_rounds, 256);
    }

    #[test]
    fn cap_reached_round() {
        let mut g = GoalState::new("s1".into(), "x".into(), Some(3), None, None).unwrap();
        let now = Utc::now().timestamp_millis();
        g.advance_round(now).unwrap();
        g.advance_round(now).unwrap();
        g.advance_round(now).unwrap();
        assert_eq!(g.cap_reached(now), Some("round limit"));
    }

    #[test]
    fn cap_reached_token() {
        let mut g = GoalState::new("s1".into(), "x".into(), None, None, Some(100)).unwrap();
        g.token_accumulate(150);
        let now = Utc::now().timestamp_millis();
        assert_eq!(g.cap_reached(now), Some("token limit"));
    }

    #[test]
    fn pause_blocks_resume() {
        let mut g = GoalState::new("s1".into(), "x".into(), None, None, None).unwrap();
        let now = Utc::now().timestamp_millis();
        g.pause(now).unwrap();
        assert_eq!(g.phase, GoalPhase::Paused);
        assert_eq!(g.activation, GoalActivation::Disarmed);
        assert_eq!(g.revision, 2);
        g.resume(None, now).unwrap();
        assert_eq!(g.phase, GoalPhase::Active);
        assert_eq!(g.activation, GoalActivation::Armed);
        assert_eq!(g.revision, 3);
    }

    #[test]
    fn cas_rejects_stale_ref() {
        let mut g = GoalState::new("s1".into(), "x".into(), None, None, None).unwrap();
        let now = Utc::now().timestamp_millis();
        g.pause(now).unwrap();
        let stale = GoalRef { goal_id: g.goal_id.clone(), revision: 1 };
        assert!(matches!(g.verify_cas(&stale), Err(GoalError::StaleRef { .. })));
    }

    #[test]
    fn block_requires_lower_kebab_code() {
        let mut g = GoalState::new("s1".into(), "x".into(), None, None, None).unwrap();
        let now = Utc::now().timestamp_millis();
        assert!(matches!(g.block("BadCode".into(), "msg".into(), now), Err(GoalError::InvalidBlockedReason)));
        assert!(g.block("qc-fail".into(), "msg".into(), now).is_ok());
    }

    #[test]
    fn advance_round_increments_revision_and_caps() {
        let mut g = GoalState::new("s1".into(), "x".into(), Some(2), None, None).unwrap();
        let now = Utc::now().timestamp_millis();
        let r1 = g.advance_round(now).unwrap();
        assert!(r1.is_none());
        let r2 = g.advance_round(now).unwrap();
        assert_eq!(r2, Some("round limit".to_string()));
    }

    #[test]
    fn resume_resets_token_window() {
        let mut g = GoalState::new("s1".into(), "x".into(), None, None, Some(100)).unwrap();
        g.token_accumulate(80);
        let now = Utc::now().timestamp_millis();
        g.pause(now).unwrap();
        g.resume(None, now).unwrap();
        assert_eq!(g.tokens_used, 0);  // openclaw 风格:resume 重置 token 窗口
    }
}
```

### 12.3 集成测试:完整 SubAgent 循环

```rust
#[tokio::test]
async fn full_subagent_loop_with_goal_state() {
    let db = Arc::new(Db::in_memory().await.unwrap());
    let goal_svc = GoalService::new(db.clone());
    
    // 1. Yolo 分类为 hard → 创建 goal
    let goal = goal_svc.create("session-1", "实现 Rust TODO 工具".into(),
                                Some(5), Some(3600), None).await.unwrap();
    assert_eq!(goal.phase, GoalPhase::Active);
    assert_eq!(goal.activation, GoalActivation::Armed);
    
    // 2. Plan Agent 落盘 plans/session-1-1.md
    let plan_path = std::path::PathBuf::from("plans/session-1-1.md");
    let updated = goal_svc.update_plan_path(&goal.ref_(), plan_path.to_string_lossy().into()).await?;
    assert_eq!(updated.plan_md_path, Some(plan_path.to_string_lossy().into()));
    
    // 3. Main-Work 拆出 3 个 SubAgent 工作流
    for i in 0..3 {
        let current = goal_svc.get_current("session-1").await.unwrap().unwrap();
        let current_ref = current.ref_();
        
        // 模拟 SubAgent 执行
        let sub_result = execute_subagent(i).await;
        if sub_result.is_err() {
            // QC 失败 → block
            let blocked = goal_svc.block(&current_ref,
                                          "qc-fail".into(),
                                          format!("第 {} 步 QC 失败", i)).await.unwrap();
            assert_eq!(blocked.phase, GoalPhase::Blocked);
            return;
        }
        
        // 成功 → advance_round
        let cap = goal_svc.advance_round(&current_ref).await.unwrap();
        if let Some(reason) = cap {
            // cap 触达 → 自动 block
            let _ = goal_svc.block(&current_ref, "auto-cap".into(), reason).await.unwrap();
            return;
        }
    }
    
    // 4. 全部成功 → complete
    let final_goal = goal_svc.complete(&goal.ref_()).await.unwrap();
    assert_eq!(final_goal.phase, GoalPhase::Complete);
}

async fn execute_subagent(idx: usize) -> Result<String, String> {
    Ok(format!("sub-{}", idx))
}
```

### 12.4 与现有 laew 集成点

```rust
// src/agent/orchestrator.rs 改造点

impl MultiAgentOrchestrator {
    pub async fn run_hard(
        &self,
        classification: &TaskClassification,
        session_id: &str,
    ) -> Result<OrchestratorOutput, AgentError> {
        // 1. 获取或创建 current goal
        let goal = self.ensure_goal(session_id, classification).await?;
        let goal_ref = goal.ref_();
        
        // 2. Plan Agent 生成 plans/{session_id}-{seq}.md
        let plan_output = self.plan_runner.generate(
            &goal.objective, &classification.purpose, &classification.intent,
            &classification.decomposition, session_id,
        ).await?;
        
        // 3. 把 plan_md_path 绑定到 goal(revision+1)
        let updated_goal = self.goal_service.set_plan_path(
            &goal_ref, plan_output.path.to_string_lossy().into()
        ).await?;
        let goal_ref = updated_goal.ref_();  // 重要: ref 必须更新
        
        // 4. Main-Work 拆 WorkFlow(目标已绑定 plan_md_path)
        let workflows = self.main_work.decompose(&plan_output.markdown, &classification).await?;
        let mut goal_ref = updated_goal.ref_();
        
        // 5. 依次执行每个 SubAgent 工作流
        for (idx, workflow) in workflows.iter().enumerate() {
            // 5.1 cap 预检
            let now_ms = chrono::Utc::now().timestamp_millis();
            let current = self.goal_service.get_by_ref(&goal_ref).await?;
            if let Some(reason) = current.cap_reached(now_ms) {
                // 自动 block
                let blocked = self.goal_service.block(&goal_ref, "auto-cap".into(), reason.to_string()).await?;
                return Ok(OrchestratorOutput::blocked(blocked, idx));
            }
            
            // 5.2 SubAgent 执行
            let sub_output = self.sub_agent_work.execute(workflow).await?;
            
            // 5.3 Quality-Check
            let qc = self.qc.check(&sub_output, &current).await?;
            if !qc.pass {
                let reason = format!("qc-fail-{}: {}", idx, qc.reason);
                let blocked = self.goal_service.block(&goal_ref, "qc-fail".into(), reason).await?;
                return Ok(OrchestratorOutput::blocked(blocked, idx));
            }
            
            // 5.4 成功 → advance_round(revision+1,可能触发 auto-cap)
            let cap_reason = self.goal_service.advance_round(&goal_ref).await?;
            let current = self.goal_service.get_current(session_id).await?.unwrap();
            goal_ref = current.ref_();  // 关键: ref 必须更新到新 revision
            
            if let Some(reason) = cap_reason {
                let _ = self.goal_service.block(&goal_ref, "auto-cap".into(), reason).await?;
                return Ok(OrchestratorOutput::needs_resume(current, idx));
            }
        }
        
        // 6. 全部成功 → goal.complete()
        let final_goal = self.goal_service.complete(&goal_ref).await?;
        Ok(OrchestratorOutput::complete(final_goal))
    }
    
    async fn ensure_goal(
        &self,
        session_id: &str,
        classification: &TaskClassification,
    ) -> Result<GoalState, AgentError> {
        match self.goal_service.get_current(session_id).await? {
            None => {
                // 无 goal → 创建新(armed)
                Ok(self.goal_service.create(
                    session_id,
                    classification.goal.clone(),
                    Some(5),      // max_rounds
                    Some(3600),   // 1 小时
                    None,         // 无 token 预算
                ).await?)
            }
            Some(g) if g.phase == GoalPhase::Complete => {
                // 已 complete → 替换为新 goal
                Ok(self.goal_service.create(
                    session_id,
                    classification.goal.clone(),
                    Some(5),
                    Some(3600),
                    None,
                ).await?)
            }
            Some(g) => Ok(g),  // active/paused/blocked → 复用
        }
    }
}
```

### 12.5 状态查询 CLI 命令(用户可观测)

```rust
// src/cli/goal_cmd.rs
use crate::agent::goal::GoalPhase;

pub async fn goal_show(session_id: &str, db: Arc<Db>) -> Result<(), AgentError> {
    let goal = db.get_current_goal(session_id).await?;
    match goal {
        None => println!("No goal for session {}.", session_id),
        Some(g) => {
            println!("Goal: {}", g.objective);
            println!("Status: {:?} / {:?}", g.phase, g.activation);
            println!("Rounds: {}/{}", g.rounds_started, g.max_rounds);
            if let Some(deadline) = g.deadline_at {
                println!("Deadline: {} ms", deadline);
            }
            if let Some(reason) = g.blocked_reason {
                println!("Blocked: {}", reason);
            }
            if let Some(path) = g.plan_md_path {
                println!("Plan: {}", path);
            }
            println!("Revision: {}", g.revision);
        }
    }
    Ok(())
}
```

---

## 13. 总结:laew 从「过程式」走向「事务式」的关键改造

| 改造点 | 当前 | 目标 | 借鉴 |
|--------|------|------|------|
| Plan 生命周期 | 一次性 Markdown | 持久化 GoalState 对象 | deepseek-harness + openclaw |
| 状态转移 | 无 | 4 态 phase + 2 态 activation | deepseek-harness |
| CAS 守卫 | 无 | GoalRef revision 检查 | deepseek-harness |
| 双 cap | 无 | round + time | atomcode |
| Token 预算 | 无 | token_budget 自动触发 | openclaw |
| 失败回流 | 用户重新输入 | goal.block(reason) 自动 | atomcode recovery_context + deepseek block |
| 跨 Session 恢复 | 无 | SQLite goal_state + revision | deepseek-harness + openclaw |
| 计划审批 | 无 | plan_exit tool | opencode |
| 执行模式 | 过程式 | goal.advance_round + cap_reached | deepseek goal-round-driver |
| 嵌套状态 | 无 | 可选 phase → activity(后续) | jiuwenswarm |

**核心收益**:

1. **可观测性**: 用户随时 `/goal` 查看目标状态(atomcode / openclaw / deepseek 都有)
2. **可恢复性**: Session 重启后 goal_state 从 SQLite 恢复(deepseek + openclaw)
3. **可限制**: max_rounds + max_duration + token_budget 三重围栏(atomcode + openclaw)
4. **可审计**: goal_events 表 + revision CAS 完整可追溯(deepseek)
5. **可分层**: phase + activation 正交,允许多处同时操作同一 goal(deepseek)

**优先级建议**:

- **P0(必做)**: § 11.2.1 SQLite schema + § 12.2 GoalState 数据结构 + § 12.3 GoalService + § 11.2.4 YoloRunner 集成 + § 11.2.5 run_hard 集成(约 7 天工作量)
- **P1(强烈建议)**: token_budget 自动触发 + plan_exit 审批 + goal-round 自动续轮(约 7 天)
- **P2(后续优化)**: journal resume + 4 层 phase/agent 嵌套 + [DONE:n] 进度标记(约 5 天)

---

**字数统计**:约 2400 行(包含 ASCII 状态机图、SQL schema、Rust 代码、对比表)。**注:此为初稿,后续可补充**:更多测试用例(尤其是 CAS 失败的 race condition)、与 SessionContext 的集成、与 QC 的集成边界、TUI 状态展示。

> **关联文档**:`专题-第五轮-中断取消与后台任务深度对比.md`(后台任务生命周期)、`专题-第六轮-Skill系统深度对比.md`(skill 内的 phase / task)、`专题-任务拆解与分类深度分析.md`(Yolo 三档分类)