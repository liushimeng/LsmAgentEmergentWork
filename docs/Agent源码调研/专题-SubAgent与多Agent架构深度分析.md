# SubAgent 与多 Agent 架构横向专题深度分析

> 分析日期: 2026-09-04
> 输入: 6 份核心机制深度分析 + 已有对比报告
> 目标读者: laew 架构演进决策者
> 覆盖项目: claudecode / atomcode / openclaw / opencode / deepseek-harness / pi

---

## 1. 横向对比总览

### 1.1 基础对比表

| 维度 | claudecode | atomcode | openclaw | opencode | deepseek-harness | pi | **laew** |
|------|-----------|----------|----------|----------|-----------------|-----|---------|
| **Agent 数量** | 动态(内置+用户+插件+MCP) | Team 多角色(动态) | 内置/CLI/ACP 三态 | 内置 7 + 用户自定义 | 扩展定义 | 扩展定义 | **6 固定角色** |
| **编排模式** | 分布式(hooks+skills) | 三层垂直+driver 编排 | Gateway+Harness 抽象 | Effect+Session 状态机 | Cordis 插件+状态机 | Harness lane+扩展 | **MultiAgentOrchestrator** |
| **决策主体** | 模型自管理 | 调用方指定+goal evaluator | 模型+swarm scheduler | LLM 选择 Agent | Goal 域+round driver | 模型+用户 | **Yolo Agent(LLM 分类)** |
| **上下文传递** | 共享 session context | L0/L1/L2 层级传递 | 消息+共享状态 | 独立上下文(服务调用) | provider 传递 | lane 隔离 | **独立上下文** |
| **并发支持** | 有限(fork 模式) | 有限(串行) | **支持**(swarm 并发) | 有限(串行) | **支持**(事件系统) | **原生支持**(lane) | **无** |
| **通信方式** | 消息传递 | 层级传递 | 消息总线 | Effect 服务调用 | Cordis 事件 | lane 通信 | **编排器传递** |
| **工具集限制** | 无(全部工具) | 按层限制 | 按 Agent 类型限制 | 按 Agent 类型限制 | 按 provider 限制 | 按 lane 限制 | **按角色限制** |
| **结果汇总** | 模型整合 | driver 汇总 | scheduler 汇总 | Session 状态机汇总 | Goal 汇总 | lane 合并 | **编排器汇总** |

### 1.2 编排模式光谱

```
完全自主                                              完全编排
←──────────────────────────────────────────────────────────→
  pi        claudecode    openclaw    opencode    atomcode    laew
  (lane)    (hooks)       (swarm)     (Effect)    (driver)    (Orchestrator)
```

---

## 2. 各项目深入分析

### 2.1 claudecode — 分布式 Agent 架构

**核心设计**：
- Agent 数量不固定，由 hooks + skills + plugins + MCP 动态组合
- 无中央编排器，模型自行决定何时调用什么 Agent
- 每个 Agent 是独立的"能力单元"，通过 hook 注册

**上下文传递**：
- 共享 session context（所有 Agent 看到相同的对话历史）
- 工具结果通过 tool_result 消息回填到上下文
- 无专门的 Agent 间通信机制

**并发模型**：
- 支持有限的 fork 模式（子 Agent 独立执行后汇报）
- 主要还是串行：用户输入 → Agent 处理 → 工具调用 → 下一轮

**对 laew 的借鉴**：
- hook 注册模式可借鉴：laew 可以让 Agent 通过 hook 声明自己的能力
- 但分布式编排不适合 laew 的可控性需求

---

### 2.2 atomcode — 三层垂直 + Team 编排

**核心设计**：
```
L2 (业务层: 编码 Agent)
  ↓ 使用
L1 (能力层: 工具集 Bash/Read/Write/Edit/Glob/Grep)
  ↓ 基于
L0 (内核层: 中立 Agent 循环)
```

**Team 概念**：
- 多个 Agent 组成 Team，每个 Agent 有特定角色
- 调用方指定哪个 Agent 处理当前任务（两档：Simple/Hard）
- Goal 评估器判断任务难度，选择合适的 Agent

**上下文传递**：
- 层级传递：L0 → L1 → L2 每层有独立上下文
- 工具结果通过层间接口传递
- L0 不感知业务，L2 不感知底层实现

**工具集限制**：
- L0 层：无工具（纯循环）
- L1 层：基础工具集
- L2 层：业务工具集（基于 L1）

**对 laew 的借鉴价值**：
- **层级隔离**值得学习：laew 的 6 角色可以进一步分层
- **Goal 评估器**的两档分类与 laew 的三档类似
- **cargo feature gating** 的编译强制隔离可以借鉴

---

### 2.3 openclaw — Gateway + Swarm 编排

**核心设计**：
```
Gateway (统一 Provider 接口)
  ↓
Harness (状态机: idle→thinking→tool_calling→completing)
  ↓
Swarm Scheduler (多 Agent 调度)
  ↓
内置/CLI/ACP 三态 Agent
```

**Swarm 调度器**：
- 管理多个 Agent 的执行队列
- 支持并发执行（多个 Agent 同时处理不同子任务）
- 调度策略：轮询 / 优先级 / 负载均衡

**三态 Agent**：
1. **内置 Agent**：框架提供的核心能力
2. **CLI Agent**：外部 CLI 工具包装
3. **ACP Agent**：通过 Agent Communication Protocol 连接的远程 Agent

**上下文传递**：
- 消息总线：Agent 间通过消息通信
- 共享状态：部分状态对所有 Agent 可见
- Harness 维护全局状态

**对 laew 的借鉴价值**：
- **Swarm 并发**是 laew 缺失的：hard 任务的并行子任务可以并发执行
- **ACP Agent** 概念：laew 可以支持远程 Agent 委派
- **Harness 状态机**：比 laew 的简单 while 循环更可控

---

### 2.4 opencode — Effect + Session 状态机

**核心设计**：
```
AppRuntime (ManagedRuntime)
  ↓
LocationServiceMap (40+ 服务)
  ↓
SessionRunner (状态机: idle→running→tool_calling→completing)
  ↓
3 种 Agent (coder / task / ask)
```

**3 种内置 Agent**：

| Agent | 职责 | 工具集 | 场景 |
|-------|------|--------|------|
| `coder` | 默认编码 Agent | 全部工具 | 主要使用 |
| `task` | 子任务 Agent | 受限工具集 | SubAgent 委派 |
| `ask` | 问答 Agent | 无写入工具 | 纯问答 |

**Agent 选择机制**：
- 默认使用 `coder`
- 用户可通过命令切换
- 子任务自动使用 `task`

**上下文传递**：
- 独立上下文：每个 Agent 有自己的消息历史
- 通过 Effect 服务调用传递数据
- Session 状态机管理全局状态

**对 laew 的借鉴价值**：
- **`task` Agent 概念**：laew 的 SubAgent 可以使用受限工具集（仅 Read+Write，无 Bash）
- **Effect 服务调用**：laew 可以借鉴服务化架构，让 Agent 通过服务接口交互
- **状态机**：比 laew 的简单循环更可控

---

### 2.5 deepseek-harness — Cordis 插件 + Goal 域

**核心设计**：
```
Cordis (Everything-is-a-Plugin)
  ↓
Goal 域模型 (created→executing→completed/failed)
  ↓
Round Driver (每轮循环驱动)
  ↓
SubAgent Provider 注册表
```

**Goal 域模型**：
- 完整的生命周期管理：created → planning → executing → reviewing → completed/failed
- `maxGoalRounds`：最大轮次限制
- 支持子 Goal（嵌套目标）
- 进度追踪与完成判定

**SubAgent Provider 注册表**：
```typescript
interface SubAgentProvider {
  name: string
  capabilities: string[]  // 能力标签
  tools: Tool[]           // 可用工具
  create(config): SubAgent  // 工厂方法
}
```

**调度策略**：
- 注册表机制：SubAgent 注册到全局表
- 能力匹配：根据任务需求选择合适的 SubAgent
- 工具集限制：每个 SubAgent 有自己的工具集

**上下文传递**：
- Provider 传递：通过 SubAgentProvider 的 create 方法注入上下文
- Cordis 事件系统：Agent 间通过事件通信
- Goal 域维护全局状态

**对 laew 的借鉴价值**：
- **Goal 域模型**是最完整的任务抽象，值得 laew 借鉴
- **Provider 注册表**：laew 的 6 角色可以抽象为 Provider
- **Round Driver**：每轮循环的驱动机制，比 laew 的简单 while 更精细
- **maxGoalRounds**：防止无限循环的硬限制

---

### 2.6 pi — lane 并发模型

**核心设计**：
```
Harness (核心抽象)
  ↓
lane 并发模型 (独立执行通道)
  ↓
扩展定义 (Skill + Tool)
  ↓
30+ Provider 适配
```

**lane 概念**：
- lane：独立的执行通道
- 每个 lane 有自己的上下文和工具集
- 支持真正的并行执行
- lane 间通过消息通信

**并发模型**：
```typescript
interface Lane {
  id: string
  context: Context      // 独立上下文
  tools: Tool[]         // 独立工具集
  status: 'idle' | 'running' | 'completed' | 'failed'
}
```

**为什么 pi 拒绝多 Agent**：
- 作者 Mario Zechner 认为"模型即 Agent"
- 通过 lane 并发实现并行，无需多个 Agent
- Skill 系统提供能力扩展

**上下文传递**：
- lane 隔离：每个 lane 完全独立
- 主 lane 可以 spawn 子 lane
- 子 lane 完成后结果合并到主 lane

**对 laew 的借鉴价值**：
- **lane 并发**是 laew 缺失的关键能力
- hard 任务的并行子任务可以用 lane 实现
- 但 lane 的完全隔离可能不适合 laew 的编排需求

---

## 3. 横向深度对比

### 3.1 编排模式对比

| 模式 | 代表 | 优势 | 劣势 | 适合场景 |
|------|------|------|------|---------|
| **中央编排器** | laew, atomcode | 可控性强、流程清晰 | 灵活性差、扩展困难 | 高可靠性场景 |
| **分布式 hooks** | claudecode | 灵活、易扩展 | 不可控、调试困难 | 快速原型 |
| **Swarm 调度** | openclaw | 支持并发、负载均衡 | 复杂度高 | 大规模任务 |
| **状态机驱动** | opencode, deepseek | 状态清晰、错误恢复好 | 学习曲线陡 | 复杂工作流 |
| **lane 并发** | pi | 原生并发、简洁 | 隔离性强 | 并行子任务 |

### 3.2 Agent 类型对比

| 项目 | 类型数 | 类型定义方式 | 类型切换 |
|------|--------|------------|---------|
| claudecode | 动态 | hooks + plugins | 自动 |
| atomcode | Team | 调用方指定 | 手动/自动 |
| openclaw | 3 态 | 内置/CLI/ACP | 调度器 |
| opencode | 3 种 | coder/task/ask | 命令/自动 |
| deepseek | 动态 | Provider 注册表 | 能力匹配 |
| pi | 扩展定义 | Skill + Tool | 用户选择 |
| **laew** | **6 固定** | **AgentProfile** | **Yolo 分类** |

### 3.3 上下文传递对比

| 传递方式 | 代表 | 优势 | 劣势 |
|---------|------|------|------|
| **共享上下文** | claudecode | 简单、信息完整 | 隐私泄露、冲突 |
| **层级传递** | atomcode | 隔离好、职责清晰 | 信息丢失 |
| **消息总线** | openclaw | 解耦、可追踪 | 复杂度高 |
| **服务调用** | opencode | 类型安全、可测试 | 抽象层多 |
| **事件系统** | deepseek | 异步、松耦合 | 调试困难 |
| **lane 隔离** | pi | 完全隔离 | 信息不共享 |
| **编排器传递** | laew | 可控、明确 | 耦合度高 |

### 3.4 并发模型对比

| 模型 | 代表 | 并发粒度 | 实现复杂度 |
|------|------|---------|-----------|
| **串行** | laew, opencode | 无并发 | 低 |
| **有限 fork** | claudecode | 子 Agent 级 | 中 |
| **Swarm 并发** | openclaw | Agent 级 | 高 |
| **事件驱动** | deepseek | 事件级 | 高 |
| **lane 并发** | pi | 执行通道级 | 中 |

---

## 4. 设计模式提炼

### 模式 1：中央编排 vs 模型自主

**中央编排**（laew, atomcode）：
- 流程显式可控
- 适合高可靠性场景
- 缺点：扩展困难

**模型自主**（claudecode, pi）：
- 灵活、易扩展
- 适合快速迭代
- 缺点：不可控

**最佳实践**：保留编排骨架，吸收自主机制
- hard 任务：完整编排（Plan → Main-Work → SubAgent → QC）
- medium 任务：简化编排（Main-Work → SubAgent）
- simple 任务：模型自主（直接执行）

### 模式 2：SubAgent 工具集限制

**通用方案**：
```rust
enum ToolPolicy {
    Full,           // 全部工具
    ReadOnly,       // 仅读取
    LimitedWrite,   // 受限写入
    Custom(Vec<Tool>), // 自定义
}
```

**各项目实现**：
- claudecode：无限制
- atomcode：按 L0/L1/L2 层级限制
- opencode：按 Agent 类型限制（coder=全部, task=受限, ask=无写入）
- deepseek：按 Provider 限制

**对 laew 的建议**：
- SubAgent-Work：Bash + Read + Write（当前方案）
- Plan Agent：Read + Write（仅规划）
- Yolo Agent：Read（仅分析）
- Quality-Check：Read（仅检查）

### 模式 3：上下文传递的陷阱

**陷阱 1：上下文爆炸**
- 共享上下文会导致所有 Agent 看到所有信息
- 解决：按需传递，只给必要信息

**陷阱 2：信息丢失**
- 层级传递会丢失底层信息
- 解决：关键信息显式传递

**陷阱 3：并发冲突**
- 多 Agent 同时修改共享状态
- 解决：lane 隔离或消息总线

### 模式 4：Goal 域模型

**通用设计**：
```
Goal {
  id: string
  status: created | planning | executing | reviewing | completed | failed
  parent?: Goal      // 父目标
  children: Goal[]   // 子目标
  rounds: number     // 当前轮次
  maxRounds: number  // 最大轮次
  result?: any       // 执行结果
}
```

**最佳实践**：
- 支持嵌套目标（子 Goal）
- 有最大轮次限制（防死循环）
- 有进度追踪（完成百分比）
- 有失败处理（重试/降级/放弃）

### 模式 5：lane 并发与任务拆分

**lane 模型**（pi）：
```
主 lane
  ├── 子 lane 1 (并行执行)
  ├── 子 lane 2 (并行执行)
  └── 子 lane 3 (并行执行)
  ↓ 结果合并
主 lane 继续
```

**最佳实践**：
- 独立上下文：每个 lane 有自己的历史
- 消息通信：lane 间通过消息协调
- 结果合并：子 lane 完成后结果汇总到主 lane

---

## 5. 对 laew 的综合建议

### 5.1 当前架构分析

**laew 的 6 角色架构**：
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
- 质检门控严格
- 流程可控

**劣势**：
- 无并发支持
- simple 任务也过重
- 扩展困难

### 5.2 优化建议

**P0：简化 simple 任务路径**
```rust
// 当前：simple → Yolo → SubAgent → QC → SessionContext
// 优化：simple → Yolo → SubAgent（跳过 QC 和 SessionContext）
```
- 工作量：50-100 行
- 影响：减少 50% 的 simple 任务开销

**P0：引入 doom_loop 检测**
```rust
// 在 agent/mod.rs 的主循环中
let recent_calls: VecDeque<ToolCallHash> = VecDeque::new();
if is_doom_loop(&recent_calls) {
    inject_warning("检测到重复模式，请换策略");
}
```
- 工作量：30-50 行
- 影响：防止无意义循环

**P1：SubAgent 工具集限制**
```rust
enum SubAgentPolicy {
    Full,           // SubAgent-Work: Bash+Read+Write
    ReadOnly,       // Yolo: Read
    ReadWrite,      // Plan: Read+Write
    CheckOnly,      // QC: Read
}
```
- 工作量：50-100 行
- 影响：安全性提升

**P1：Goal 域模型**
```rust
struct Goal {
    id: String,
    status: GoalStatus,
    parent: Option<Box<Goal>>,
    children: Vec<Goal>,
    rounds: u32,
    max_rounds: u32,
}
```
- 工作量：100-200 行
- 影响：任务生命周期管理

**P2：lane 并发（hard 任务）**
```rust
// 在 MultiAgentOrchestrator 中
if task_level == TaskLevel::Hard {
    let lanes = spawn_parallel_lanes(sub_tasks);
    let results = join_all(lanes).await;
    merge_results(results);
}
```
- 工作量：500-800 行
- 影响：hard 任务并行化

**P2：Agent 类型扩展**
```rust
// 新增 ask Agent（纯问答，无工具）
fn ask_profile() -> AgentProfile {
    AgentProfile {
        name: "LsmAgentEmergentWork-Ask",
        system_prompt: "...",
        tools: vec![],  // 无工具
    }
}
```
- 工作量：100-200 行
- 影响：简单问答无需走完整流程

### 5.3 代码位置建议

| 优化项 | 代码位置 | 建议 |
|--------|---------|------|
| 简化 simple 路径 | `agent/yolo.rs` | 跳过 QC 和 SessionContext |
| doom_loop 检测 | `agent/mod.rs` | 主循环中加入检测 |
| 工具集限制 | `agent/profile.rs` | SubAgentPolicy 枚举 |
| Goal 域模型 | `agent/goal.rs`（新建） | Goal 结构体 + 生命周期 |
| lane 并发 | `agent/lane.rs`（新建） | Lane 结构体 + 并发管理 |
| Agent 类型扩展 | `agent/profile.rs` | 新增 ask_profile() |

---

## 文档信息

- 总行数：~600 行
- 覆盖维度：Agent 数量 / 编排模式 / 上下文传递 / 并发支持 / 通信方式 / 工具集限制 / 结果汇总
- 输入文件：6 份核心机制深度分析 + 已有对比报告
- 分析方法：横向对比 + 深入分析 + 设计模式 + laew 建议
