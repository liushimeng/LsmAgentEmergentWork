# Agent 架构对比与参考:7 项目横向综合报告

> 输入:12 份源码调研与深度分析(atomcode / claudecode / deepseek-harness / opencode / openclaw / pi ×2)
> 输出日期:2026-09-04
> 目标读者:`LsmAgentEmergentWork`(laew)架构演进决策者
> 行数:800-950 行,聚焦对比与提炼,避免重复输入文件细节

---

## 1. 执行摘要

本轮调研覆盖 6 个主流 LLM Agent 项目 + laew 自身,按**设计范式**可归为三大阵营:

| 范式 | 代表 | 核心信条 | 复杂度 |
|------|------|---------|--------|
| **编排器驱动** | **laew**、atomcode、deepseek-harness | "流程显式可控"——用中央编排器 / 状态机 / 域模型强约束 Agent 行为,角色、质检、分类都是硬编码环节 | 中-高(结构清晰、学习曲线陡) |
| **模型自主** | **claudecode**、opencode、openclaw、pi | "模型即路由"——把决策权交给 LLM,框架只提供工具 / hooks / skills,流程由模型自组织 | 机制多、单文件小、整体复杂度分散 |

### 关键洞察(6 条)

1. **仅 laew 与 atomcode 有"入口层 Yolo/Goal 分类器"**,其余 5 个都把"任务分档"交给模型或用户显式选择。这意味着 laew 的"三步意图识别 + 三档分类"是**少数派设计**,优势在可控性,劣势在每次任务都要额外一次 LLM 调用。claude / opencode / openclaw / pi 都证明了"模型自管理任务"在生产可行,但 laew 的硬编码角色在**高可靠性场景**(如代码质检)仍有独特价值。

2. **仅 laew 有独立 Quality-Check Agent 作为必经门控**,其余用 hooks / guard plugin / permission / doom_loop 等轻量机制替代。这意味着 laew 的 QC 是**最重的质检方案**,适合高可靠性场景,但简单任务也过重。量化来看,laew 的"每单元 QC"在 10 轮任务中可能多消耗 1-2 次 LLM 调用,在简单任务中是显著开销。

3. **MCP 是分水岭**:claudecode(3348 行) / atomcode(完整 trust 体系) / deepseek-harness(重连策略) / opencode(3 传输 + Catalog) / openclaw(双向)都是"双向 MCP 一等公民",pi 明确拒绝,laew 完全自建。**laew 缺失 MCP 是最大生态短板**——一旦用户需要浏览器控制、数据库查询、搜索引擎等成熟 MCP 工具,laew 无解。

4. **Skill 是 MCP 的轻量替代**:6 个外部项目都有 Skill 系统,laew 缺失。Skill 的核心是"Markdown + frontmatter + 延迟加载",比 MCP 更轻、更 token 经济。pi 的作者 Mario Zechner 甚至认为"**No MCP. Build CLI tools with READMEs**"——Skill 优先哲学值得 laew 借鉴。

5. **Context 压缩是标配**:除 laew 外都有多阶段压缩(四级管线 / 三级 ladder / prune + compaction),laew 的"无压缩"是**最大技术债**。量化来看,Claude Sonnet 的 200K 上下文窗口在 30-50 轮工具调用后会溢出,**laew 当前无法处理长任务**。

6. **双层循环 + steering 是主流**:claude / openclaw / pi 都用双层 while + 队列支持"用户中途插入",laew 当前是单线状态。这意味着 laew 在 Agent 运行中无法响应用户中断,**长任务体验差**。

### 对 laew 的整体建议

laew 的"6 角色 + 三档分类 + QC 门控"是**强编排范式**的极端,优势在可控性,劣势在灵活性。本报告第 5 节给出按优先级排序的借鉴路线图,核心思路是**"保留编排骨架,吸收机制级最佳实践"**——不推翻 6 角色,而是在每个角色内部借鉴外部最佳实现。具体策略:

- **保留**:MultiAgentOrchestrator 的三档路由、Quality-Check Agent 的必经门控、Plan Agent 的 Markdown 方案输出
- **吸收**:claude 的四级压缩、openclaw 的 exec auto-reviewer、opencode 的 doom_loop、atomcode 的 tool-loop policy、pi 的 Skill 系统
- **新增**:MCP 客户端(最小可用)、错误分类表驱动、双层循环 + steering

---

## 2. 架构范式对比

### 2.1 基础元信息

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 语言 | Rust 2021 | Rust 2021 | TS(Bun) | TS(pnpm) | TS(pnpm) | TS(Bun + Effect) | TS(npm) |
| 规模(核心) | ~8k 行 | ~150k 行 | ~218k 行 | ~80+ 包 | ~201 万行 | ~18k 行 | ~12 包 |
| 构建 | cargo | cargo workspace | Bun bundle | pnpm + Cordis | tsdown | Bun + esbuild | Bun standalone |
| 二进制 | `laew` | `atomcode` | `claude` | `dsh`(多 profile) | `openclaw` | `opencode` | `pi` |
| 协议 | Anthropic + OpenAI | 中性 + 多 Provider | Anthropic(主) | DeepSeek + pi-ai 通用 | 9 family + 40+ 插件 | 14+ Provider | 30+ Provider |
| MCP | 无 | 完整(双向) | 完整(7 传输) | 完整(stdio+http) | 完整(双向) | 完整(3 传输) | **明确无** |
| Skill | 无 | frontmatter + shell | 16 bundled | layered registry | 52 + workshop 自演化 | 双轨 + 远程拉取 | 一等公民 |
| 许可证 | MIT | MIT | MIT | MIT | MIT | MIT | MIT |

**规模警示**:openclaw(201 万行)与 claude(218k 行)是团队级产物,不可整体照搬;atomcode(150k 行)与 opencode(18k 行)是中型项目,架构最具参考价值;pi(12 包)是极简脚手架,哲学对立但机制可借鉴。laew(8k 行)与 opencode 量级接近,**opencode 是最直接的对标对象**。

### 2.2 设计哲学

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 编排模式 | 中央编排器(MultiAgentOrchestrator) | 三层垂直 + driver 编排 | 分布式(hooks + skills) | Cordis 插件 + 状态机 | Gateway + Harness 抽象 | Effect + Session 状态机 | Harness lane + 扩展 |
| 决策主体 | Yolo Agent(LLM 分类) | 调用方指定 + goal evaluator | 模型自管理 | Goal 域 + round driver | 模型 + swarm scheduler | LLM 选择 Agent | 模型 + 用户 |
| Agent 角色 | 6 固定角色 | Team 多角色(动态) | 动态(内置+用户+插件+MCP) | SubAgent provider 注册表 | 内置/CLI/ACP 三态 | 内置 7 + 用户自定义 | 扩展定义 |
| 任务分类 | 显式三档(simple/medium/hard) | 两档(Simple/Hard)选模型 | 隐式(effort) | Goal 域 maxGoalRounds | 无(单 Agent + Swarm) | 无(Agent 类型) | 无 |
| 质检 | 独立 QC Agent(必经) | VerifyCadenceHook | hooks + skills | guard plugin | exec auto-reviewer | doom_loop + permission | 无内置 |
| Context | 无压缩 | 三级 overflow ladder | 四级压缩管线 | projection unit | 可插拔 ContextEngine | prune + compaction | 压缩 + 分支摘要 |
| 持久化 | SQLite | .snapshot + .jsonl 二级 | 文件 + SQLite | append-only SessionEvent | JSONL + SQLite | SQLite + Drizzle | JSONL / SQLite |
| 项目上下文 | 五级链(CLAUDE→AGENTS→README→生成→空) | 无独立注入 | prependUserContext + MagicDocs | agent-instructions 插件 | context-engine | instruction.ts | resource-loader(候选链) |

**哲学光谱**:从"完全编排"(laew)到"完全自主"(pi),中间是各种混合。laew 的独特价值在于**角色隔离 + 质量门控 + 确定性路由**,适合对可靠性要求高的场景;劣势是**灵活性不足**,简单任务也过重。建议走"**编排为主、自主为辅**"的中间路线:保留 hard 任务的完整编排,simple/medium 任务允许模型自主决策。

### 2.3 分层架构

| 项目 | 分层 | 依赖方向 | 编译强制 |
|------|------|---------|---------|
| **laew** | 扁平模块:`agent/`(orchestrator/plan/quality/subagent/yolo/profile/tools) + `llm/`(双协议) + `tui/` | 无编译强制,边界由模块注释维持 | 无 |
| **atomcode** | **L0 kernel**(中立循环) → **L1 capabilities**(可复用能力) → **L2 coding**(业务) → **cli/tui**(前端) | cargo feature gating 编译强制,capabilities 绝不依赖 core | **是** |
| **claudecode** | 单层扁平 + feature gate(`REACTIVE_COMPACT` / `COORDINATOR_MODE` / `CONTEXT_COLLAPSE`) | Bun `feature()` 编译期 DCE | 是(编译期 DCE) |
| **deepseek-harness** | **Everything-is-a-Plugin**:core / llm / mcp / skill / subagent / session / context 都是 Cordis 插件 | Cordis `ctx.effect()` 注册,Scope 链路隔离 | 否(运行时插件) |
| **openclaw** | **渠道 → auto-reply → Gateway → Agent Harness**(builtin/CLI/ACP) → agent-loop → StreamFn | packages/ 契约层与 src/ 解耦 | 是(project-references) |
| **opencode** | **core**(Effect + Schema) → **llm**(协议无关) → **opencode**(运行时) → **tui**(Solid + OpenTUI) | Effect Layer DI 全栈化 | 是(Effect Schema) |
| **pi** | **pi-ai**(Provider) → **agent-core**(循环 + Harness) → **coding-agent**(CLI/Modes) → **pi-tui**(差异渲染) | 严格分层,lane 并发模型 | 否 |

**对 laew 的启示**:atomcode 的"三层 + cargo feature gating"是**编译强制分层**的样板,laew 可借鉴把 `agent/` 拆为 `kernel/`(循环) + `capabilities/`(工具/MCP/Skill) + `coding/`(业务),用 feature flag 选编译。当前 laew 的扁平模块在 8k 行时尚可,但增长到 20k+ 行后边界会模糊。

### 2.4 架构决策树(帮助选型)

```
你的 Agent 是编码专用还是通用?
  ├─ 编码专用 → laew / atomcode / opencode / pi
  └─ 通用助手 → openclaw / claude

是否需要接第三方工具生态(MCP Server)?
  ├─ 是 → claude / atomcode / opencode / openclaw / deepseek-harness
  └─ 否 → pi / laew(当前)

是否需要长会话(>50 轮)?
  ├─ 是 → 必须有压缩:claude / atomcode / opencode / openclaw / pi
  └─ 否 → laew(当前可接受) / 其他

是否需要多 Agent 协作?
  ├─ 强编排(角色隔离 + QC) → laew / atomcode
  ├─ 模型自组织 → claude / opencode / openclaw
  └─ 极简扩展 → pi

你的团队规模?
  ├─ 1-3 人 → pi / laew / opencode
  ├─ 5-10 人 → atomcode / claude
  └─ 10+ 人 → openclaw / deepseek-harness
```

---

## 3. 8 维度横向对比

### 3.1 多轮对话

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 主循环位置 | `agent/mod.rs` run_session | `kernel/agent.rs:1418` session_loop | `query.ts:241` queryLoop | `agent-loop/agent.ts` ReactLoopAgent | `agent-core/agent-loop.ts:295` runLoop | `session/prompt.ts:1081` runLoop | `agent/agent-loop.ts:156` runLoop |
| 循环模型 | 单循环 + 多 Agent 接力 | turn → round 嵌套 | while(true) + 7 个 continue 站点 | 三相位状态机(idle/maintenance/running) | 双层 while + steering | while(true) + Effect | 双层 while + steering/followUp |
| Turn 模型 | 无显式 turn | TurnCtx(turn_id/round/request_id) | State.transition 记录为何继续 | turn/step 三层边界 | turnTainted 机制 | finish reason 判断 | turn_start/turn_end 事件 |
| Steering | 无 | SteerBuf 折叠到下一轮 | 命令队列 drain | inbox 双队列(next-turn/next-step) | steering 队列 + checkpoint | 无 | PendingMessageQueue |
| 取消 | CancellationToken | per-turn token + keep_interrupted | 协作式 + transition 记录 | wakeRequested 锁存 | turn-interruption + tainted | Effect.onInterrupt | abort signal |
| 失败回流 | Yolo 处理 | tool-loop / repeat-loop / overflow ladder | 错误暂扣(withholding) + 恢复路径 | sticky turn end reason | tool-loop 检测 + 恢复 | doom_loop 检测 | 无 |
| 最大轮次 | max_rounds | MAX_REPEAT_ROUNDS=6 + MAX_RATE_LIMIT_WAITS=5 | 无显式(靠 transition) | maxGoalRounds=256 | 无显式 | agent.steps ?? Infinity | 无 |
| 中断语义 | 无 | keep_interrupted_context | transition 记录 | wakeRequested 锁存 | turn-interruption + tainted | Effect.onInterrupt | DeferredHandle |

**关键差异**:claude 的"错误暂扣 + 7 个 continue 站点"是**最精细的恢复路径**——当 API 返回 prompt-too-long 时,先尝试 context collapse(廉价),再 reactive compact(重量),最后才 surface 给用户。laew 的"max-rounds 一刀切"最粗糙,一旦超限直接失败。

**对 laew 的启示**:P0-3(doom_loop)是最小可用改进;P1-3(双层循环 + steering)是中期目标;P2-3(三相位状态机)是长期重构。当前最紧迫的是 **P1-1(压缩管线)**——没有它,长会话必然失败,其他改进都是空中楼阁。

### 3.2 Context 管理

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 压缩 | 无 | 三级 ladder(stub→truncate→drain+LLM) | 四级管线(micro/auto/reactive/sessionMemory) | projection unit 可插拔 | 内置 + 可插拔 ContextEngine | prune + compaction + tail budget | 压缩 + 分支摘要 |
| Token 估算 | 无 | used_tokens + 重算利用率 | usage + chars 回退 | 无 | usage + chars/4 启发式 | 真实 usage + 估算 | 真实 usage + chars/4 |
| 触发 | N/A | 任务边界 + 紧急溢出 + 手动 | autoCompact 阈值 + reactive | 无 | shouldCompact(阈值) | overflow 检测 | shouldCompact(阈值) |
| 切点保护 | N/A | sacred_floor(首 System + 首 User) | API 不变量(tool_use/tool_result 对) | 无 | turn 边界 + tool-result 配对 | 只切 user/assistant | 只切 user/assistant |
| 工具结果截断 | 无 | cap_tool_result + self_bounds_output | tool result budget | 无 | tool-result-truncation(1370 行) | truncate.output() 统一 | 无 |
| 摘要格式 | N/A | LLM 自由生成 | 结构化(Goal/Progress/...) | 无 | 结构化(Goal/Constraints/Progress/...) | 无固定格式 | 结构化(Goal/Constraints/...) |
| 增量更新 | N/A | 无 | 无 | 无 | UPDATE_SUMMARIZATION_PROMPT | 无 | UPDATE_SUMMARIZATION_PROMPT |
| 分支摘要 | N/A | 无 | 无 | 无 | branch-summarization.ts | 无 | branch-summarization.ts |
| 文件操作追踪 | N/A | 无 | 无 | 无 | CompactionDetails(readFiles/modifiedFiles) | 无 | CompactionDetails |

**关键差异**:claude 的"四级管线"是**业界最复杂**,opencode 的"prune + compaction 双阶段"是**最轻量有效**(608 行可实现核心)。两者的共同点是**双轨 token 估算**(真实 usage + chars/4 启发式),这是防止误判的关键。

**对 laew 的启示**:P1-1 应优先借鉴 opencode 的"prune + compaction"方案,核心步骤:
1. **prune**:从后往前擦除旧 tool output(跳过最近 2 轮 + skill 工具)
2. **compaction**:超阈值时调 LLM 生成结构化摘要(Goal/Constraints/Progress/...)
3. **tail budget**:保留最近 20K token 不被压缩
4. **增量更新**:有 previousSummary 时走 update prompt,避免信息丢失

### 3.3 Yolo / 任务分类

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 入口 Agent | **Yolo Agent**(三步意图识别) | 无(goal-mode followup 分类) | 无(模型即路由) | 无(Goal 域) | 无(Yolo=exec 模式) | 无 | 无 |
| 分类档位 | **三档**(simple/medium/hard) | 两档(Simple/Hard) | 无(effort 声明) | maxGoalRounds 数值 | 无 | 无 | 无 |
| 分类主体 | LLM(JSON 解析) | 调用方指定 | 模型自管理 | 用户显式创建 | 模型 + Swarm | LLM 选择 Agent | 用户/模型 |
| 分类时机 | 每条用户输入 | goal-mode 下 | 无 | 显式 goals.create | 无 | 无 | 无 |
| 失败回流 | Yolo 建议 | goal_cap_stop_note | stop hooks | GOAL_INVALID_TRANSITION | tool-loop 终止 | doom_loop 询问 | 无 |
| 实现成本 | 高(每次任务多一次 LLM 调用) | 中(evaluator 调用) | 零 | 低(域模型) | 零 | 零 | 零 |
| 输出格式 | JSON TaskClassification | 无 | effort 字段 | Goal 快照 | 无 | Agent 类型 | 无 |

**关键洞察**:laew 的 Yolo 是**唯一把"任务分类"作为独立 Agent 角色**的设计。优势在"失败回流 + 用户建议"(其他项目没有),劣势在"每次任务多一次 LLM 调用"。如果保留,可借鉴 atomcode 的"evaluator LLM 调一次判 Verdict: yes/no"简化三步分析;如果要替换,可借鉴 deepseek-harness 的"Goal 域 + maxGoalRounds 配额"。

**对 laew 的决策建议**:
- **保留 Yolo 的场景**:需要"失败回流 + 用户建议"的高可靠性场景
- **替换为 Goal 域的场景**:需要"显式可解释资源配额"的长期任务
- **移除 Yolo 的场景**:简单/中等任务占多数,可让模型自主决策(可配置开关)

### 3.4 质检检查

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 质检角色 | **Quality-Check Agent**(必经) | VerifyCadenceHook(编辑后验证) | hooks + skills + verification agent | guard plugin(advisory) | exec auto-reviewer(执行前) | doom_loop + permission | 无内置 |
| 质检时机 | 每个执行单元完成后 | edit 后未跑构建 | stop hooks / post-sampling | tools/post-execute | 命令执行前 | 工具执行时 | N/A |
| 固定 cadence | 有(每单元) | 一次性 nudge | 无(事件驱动) | 无(观察驱动) | 无 | doom_loop=3 | N/A |
| 失败阻断 | 可回流重做 | attended 模式抑制 | stop hook 可阻塞 | 不阻断(advisory) | 转人工审批 | 触发权限询问 | N/A |
| 实现方式 | LLM judge | 工具名键控 + bash denylist | 无独立 | canonical 参数比对 | 独立小模型 | 连续 N 次相同调用 | N/A |
| 语言相关性 | 通用 | 编码领域 | 通用 | 通用 | 通用 | 通用 | N/A |
| 实现成本 | 高(每次单元完成后 LLM 调用) | 中(一次性) | 低(hooks) | 低(观察) | 中(小模型) | 低(计数) | 零 |

**关键洞察**:laew 的 QC Agent 是**最重的质检方案**,适合高可靠性场景。但 openclaw 的"exec auto-reviewer"(执行前)与 laew 的"QC Agent"(执行后)是**正交**的,两者结合可形成"前置风控 + 后置质检"闭环。量化来看,laew 的 QC 在 10 个执行单元中会多消耗 10 次 LLM 调用,而 openclaw 的审查员只对 bash 命令执行前调用,**成本更低、覆盖面更集中**。

**对 laew 的启示**:P0-2(执行前审查员)应与现有 QC Agent 共存,形成双层质检:
- **执行前**:bash 命令用小模型审查(低开销、高覆盖)
- **执行后**:Quality-Check Agent 做通用 QC(高开销、仅 hard 任务启用)

### 3.5 任务拆解

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 规划层 | Plan Agent(仅 hard,输出 Markdown) | Team 多角色 / goal continuation | Plan Agent(只读) + Plan Mode | PlanModeController + exit_plan_mode | 无(模型自拆) | plan Agent(禁 edit) | plan-mode 扩展 |
| 执行层 | Main-Work → SubAgent-Work | TeamRunner | AgentTool → runAgent | SubagentRuntime(N provider) | sessions_spawn + registry | task 工具 + subagent_type | subagent 扩展(子进程) |
| 多工人编排 | 无 | TeamManager | Coordinator + Team + SendMessage | 无(experimental) | Swarm scheduler(限流) | 无 | 无 |
| 依赖图 | 无 | 无 | TaskCreate/Get/List | 无 | 无 | 无 | 无 |
| 子 Agent 隔离 | Agent 切换 | Team scope 隔离 | forkContextMessages | 独立 Session | isolated/fork 模式 | 独立 session | 独立子进程 |
| 深度限制 | 无 | 无 | 无 | maxDepth + provider-managed | 无 | subagent_depth 默认 1 | MAX_CONCURRENCY=4 |
| 恢复机制 | 无 | 无 | resumeAgent | ContinuableManager | 无 | task_id 恢复 | 无 |

**关键洞察**:laew 的"Plan → Main → SubAgent"是**最重的拆解流程**,仅 hard 任务启用。openclaw 的"sessions_spawn + Swarm"是**最灵活的并行编排**(FIFO 限流 + 结果收集),opencode 的"task 工具 + task_id 恢复"是**最轻量的子 Agent 机制**。deepseek-harness 的"ContinuableManager"是**唯一支持"持续型子 Agent"**(一次创建、多轮推进)的实现。

**对 laew 的启示**:当前 laew 的拆解流程已足够,可优化点是:
1. 加入 `maxDepth` 限制(防无限递归)
2. 加入 `task_id` 恢复机制(复用旧 session)
3. 借鉴 openclaw 的 Swarm 限流(如果未来需要并行子 Agent)

### 3.6 任务分类

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 显式层级 | Goal → Task → Step | 无 | 无(模型自管理 Task) | Goal 四 phase 状态机 | 无 | 无 | 无 |
| 复杂度输入 | Yolo 三档 | TeamDifficulty | effort 声明 | maxGoalRounds | 无 | 无 | 无 |
| 编排方式 | 中央编排器 | TeamRunner / goal evaluator | 分布式(hooks + skills) | Goal 域 + Round Driver | 分布式(swarm) | Agent 选择 | 扩展 |
| 状态机 | 无 | GoalPhase + GoalProgress | 无 | GoalSnapshot(四 phase:active/paused/blocked/complete) | 无 | 无 | 无 |
| 修订号 | 无 | 无 | 无 | CAS 修订号(乐观并发) | 无 | 无 | 无 |
| 配额 | 无 | 无 | 无 | defaultMaxGoalRounds=256 | 无 | 无 | 无 |

**关键洞察**:deepseek-harness 的"Goal 域 + 四 phase 状态机 + CAS 修订号"是**最严谨的任务状态管理**,laew 可借鉴替代当前的"无状态任务"。特别是 CAS 修订号,可防止并发提交产生 revision 漂移。

### 3.7 工具调用

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 工具数 | 3(Bash/Read/Write) | 10+ | 40+ | 10+ | 10+ | 15+ | 10+ |
| 注册中心 | ToolRegistry(有序) | ToolRegistry(Arc<RwLock>) | tools.ts(排序合并) | ctx.tools.register | agent-tools 装配 | ToolRegistry + Effect | 扩展注册 |
| 并发 | 顺序 | 三阶段(Classify/Execute/Apply) + Semaphore(4) | StreamingToolExecutor(兄弟 abort) | executeToolCalls 并行 | parallel/sequential 分组 | Effect 并发 | parallel/sequential |
| 循环检测 | 基本 max-rounds | tool_loop_policy(3 warn / 4 stop) | 无独立 | guard/repeat-tool-reminder | 6 种检测器 | DOOM_LOOP_THRESHOLD=3 | 无 |
| 权限 | 无 | middleware before/after 链 | 多模式(default/auto/ask/yolo/plan) | 无 | batch admission + reviewer | permission 三档(allow/ask/deny) | beforeToolCall/afterToolCall |
| Schema | Rust 定义 | JSON Schema | Zod | JSON Schema | typebox | effect/Schema | TypeBox |
| 流式 tool_use | 无 | ToolCallDelta(展示) | StreamingToolUse | 无 | 无 | tool-input-start/delta/end | 无 |
| 工具搜索 | 无 | mount(names)选择子集 | 无 | 无 | tool_search/tool_describe/tool_call | 无 | 无 |
| Schema 校验 | 无 | middleware | Zod 校验 | 无 | typebox 校验 | Schema.decode | TypeBox 校验 |

**关键洞察**:atomcode 的"三阶段(Classify/Execute/Apply) + RwLock gate + Semaphore"是**最工程化的工具调度**,laew 当前是"简单顺序执行",可借鉴加入并发 + 循环检测。openclaw 的"6 种工具循环检测器"(generic_repeat / argument_churn / unknown_tool_repeat / known_poll_no_progress / global_circuit_breaker / ping_pong)是**最全面的循环防护**,laew 可借鉴其中的 `global_circuit_breaker`(全局熔断)。

**对 laew 的启示**:
- P0-3:加入 `DOOM_LOOP_THRESHOLD=3` 检测(最简单)
- P0-4:加入 `cap_tool_result` 截断(防撑爆)
- 长期:借鉴 atomcode 的三阶段调度(如果需要并行工具)

### 3.8 MCP / Skill

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| MCP 客户端 | 无 | 完整(stdio+http+OAuth+trust) | 完整(7 传输+OAuth+SdkControl) | 完整(stdio+http+重连) | 完整(双向+OAuth) | 完整(3 传输+OAuth+Catalog) | **无** |
| MCP 服务端 | 无 | 无 | 无 | 无 | 完整(tool-stdio-server) | 无 | 无 |
| 传输 | N/A | stdio + HTTP(SSE) | 7 种(stdio/sse/ws/http/sdk/...) | stdio + Streamable HTTP | stdio + SSE + Streamable HTTP | Stdio + SSE + Streamable HTTP | N/A |
| 重连 | N/A | 无 | 无 | 指数退避(500ms→30s,10 次) | 失败退避(3 次冷却 60s) | 无 | N/A |
| 工具名空间 | N/A | mcp__<server>__<tool> | <server>__<tool> | mcp__<server>__<tool> | sanitizeServerName | McpCatalog.sanitize | N/A |
| 信任模型 | N/A | project trust + server trust + per-tool | 无 | 无 | 无 | 无 | N/A |
| Skill | 无 | frontmatter + shell 注入 + use_skill | 16 bundled + loadSkillsDir | layered registry + rank + source | 52 + workshop 自演化 | 双轨 + 远程拉取 + frontmatter | 一等公民 + 延迟加载 |
| Skill 格式 | N/A | *.md + frontmatter | bundled/*.ts | SKILL.md + frontmatter | SKILL.md + frontmatter | SKILL.md + frontmatter | SKILL.md + frontmatter |
| 加载策略 | N/A | use_skill 工具 | 自动注册 | layered registry | 自动 + 按需 | 延迟加载(只注入元数据) | 延迟加载 |
| 自演化 | N/A | 无 | 无 | 无 | workshop(唯一) | 无 | 无 |
| 来源 | N/A | ~/.atomcode/skills / project | bundled + ~/.claude/skills | layered providers | ~/.claude/skills / project / plugin | .claude/skills / opencode/skills / agents/skills | ~/.pi/skills / .pi/skills |

**关键洞察**:MCP 是**生态标准**,laew 缺失是最大短板;Skill 是**轻量替代**,6 个项目都有,laew 应优先实现 Skill(P1-2),再实现 MCP 客户端(P2-1)。pi 的"延迟加载 + disable-model-invocation"是**最 token 经济**的 Skill 加载策略,laew 可直接借鉴。

**对 laew 的启示**:
- P1-2:Skill 系统,核心是 SKILL.md 发现 + frontmatter 解析 + 延迟加载(只注入 name/description/location,模型按需 read)
- P2-1:MCP 客户端,最小可用 = stdio + StreamableHTTP + 工具同步桥 + 指数退避重连
- 不实现:MCP 服务端(openclaw 独有,laew 不需要暴露自身工具)

---

## 4. 设计模式提炼

从 7 个项目中共归纳 **15 个跨项目通用设计模式**,每个给出定义、代表实现、适用场景、laew 借鉴方式。

### 模式 1:双层循环 + Steering

- **定义**:外层 while 处理 follow-up(子 Agent 完成 / 外部事件),内层 while 处理 tool_calls;steering 队列允许用户中途插入消息。
- **代表**:claude(7 个 continue 站点)、openclaw(双层 + checkpoint)、pi(steering/followUp 队列)、atomcode(SteerBuf 折叠)。
- **适用**:需要"用户可中断 / 子 Agent 可续接"的长任务场景。
- **laew 借鉴**:P1-3,重构主循环为双层 while + PendingMessageQueue。具体做法是在 `agent/mod.rs` 中把 `run_session` 拆为 `outer_loop`(处理 follow-up)和 `inner_loop`(处理 tool_calls),加入 `steering: Vec<UserMessage>` 队列,在每轮开始前 drain。

### 模式 2:四级压缩管线

- **定义**:microCompact(轻量) → autoCompact(阈值触发) → reactiveCompact(错误恢复) → sessionMemoryCompact(长期记忆),叠加 contextCollapse 读时投影。
- **代表**:claude(业界最复杂,2400+ 行)、opencode(prune + compaction + tail budget)、atomcode(三级 overflow ladder)。
- **适用**:长会话 / 大代码库 / 多轮工具调用场景,laew 当前缺失,是 P0 改进点。
- **laew 借鉴**:P1-1,优先借鉴 opencode 的"prune + compaction 双阶段"(608 行可实现核心)。核心步骤:
  1. **prune**:从后往前擦除旧 tool output(跳过最近 2 轮 + skill 工具),轻量同步
  2. **compaction**:超阈值时调 LLM 生成结构化摘要,重量异步
  3. **tail budget**:保留最近 20K token 不被压缩
  4. **双轨 token 估算**:真实 usage + chars/4 启发式回退

### 模式 3:独立审查员模型

- **定义**:用独立小模型(廉价、快速)在命令执行前做准入判断,输出 `{decision, risk, rationale}`,与主 Agent 解耦。
- **代表**:openclaw(exec-auto-reviewer,472 行,带 prompt 注入攻击防护)、claude(verification agent,feature gate)。
- **适用**:需要"执行前风控"的编码 Agent,与 laew 的"执行后 QC"互补。
- **laew 借鉴**:P0-2,在 BashTool 执行前加小模型审查。核心实现:
  - 系统提示词要求模型忽略命令内嵌指令(防 prompt 注入)
  - 输入用 `UNTRUSTED_EXEC_REQUEST_JSON_BEGIN/END` 标记边界
  - 输出 `{decision: "allow"|"ask", risk: "low"|"medium"|"high"|"unknown", rationale: string}`
  - 超时 30s,失败兜底为 `ask`

### 模式 4:三相位状态机

- **定义**:Agent 主循环不是简单 while,而是 `idle / maintenance / running` 三态,所有切换集中发布状态事件,解决 cancel 重入。
- **代表**:deepseek-harness(ReactLoopAgent,status 只读 + setPhase 集中推进)。
- **适用**:需要"可取消 / 可恢复 / 可并发"的 Agent,laew 当前是单线状态。
- **laew 借鉴**:P2-3,重构核心循环。核心类型:
  ```rust
  enum Phase {
      Idle { last_turn: u64 },
      Maintenance { abort: AbortController, wake_requested: bool },
      Running { abort: AbortController, turn: u64, step: u64 },
  }
  ```
  所有状态切换走 `set_phase()`,集中发布 `agent/status` 事件。

### 模式 5:Inbox 双队列

- **定义**:维护 `next-turn` 与 `next-step` 两个有序队列,`followup` 进 turn、`steer / inject` 进 step,先 durable commit 再 live mutate。
- **代表**:deepseek-harness(inbox.ts,220 行,持久化事件投影)。
- **适用**:需要"消息边界清晰 / 可 fork / 可 resume"的 Agent。
- **laew 借鉴**:P2-3,与三相位状态机一起实现。核心 API:
  - `followup(input)`:进 next-turn 队列,唤醒 driver
  - `steer(input)`:进 next-step 队列,唤醒 driver
  - `inject(input)`:进 next-step 队列,不唤醒
  - `claim(target)`:取出队列消息

### 模式 6:Capability Seam

- **定义**:每个子能力(子代理 / 工具 / 技能)都拆成 **Service Definition / Provider / Consumer** 三角色,provider 声明 capabilities,registry 做能力校验。
- **代表**:deepseek-harness(subagent 五项能力 + assertCapabilities)、openclaw(AgentHarness 三态:builtin/CLI/ACP)。
- **适用**:多 provider / 多后端 / 可插拔架构。
- **laew 借鉴**:P1-4,抽出 AgentHarness trait。核心接口:
  ```rust
  trait AgentHarness {
      async fn run(&self, task: Task) -> Result<Output>;
      async fn cancel(&self, run_id: &str) -> Result<()>;
      fn capabilities(&self) -> HarnessCapabilities;
  }
  ```
  让"调用外部 CLI Agent(claude/codex)"成为可插拔后端。

### 模式 7:错误分类表驱动 + Stream 不抛异常

- **定义**:Provider 错误(auth/billing/rate-limit/overflow)固化为 `*.cases.ts` 测试表,stream 永不 throw,失败编码进终态消息。
- **代表**:openclaw(failover/classify.ts,437 行)、claude(categorizeRetryableAPIError)。
- **适用**:多 Provider / 自动故障转移 / 可解释的错误文案。
- **laew 借鉴**:P0-1,重构 `src/llm/` 错误路径。核心枚举:
  ```rust
  enum LlmError {
      Auth { provider: String, message: String },
      Billing { provider: String, message: String },
      RateLimit { provider: String, retry_after: Option<Duration> },
      OverFlow { provider: String, context_used: u32, context_limit: u32 },
      Overloaded { provider: String },
      // ...
  }
  ```
  每个错误类型配 `*.cases.rs` 测试表,stream 失败编码进 `StopReason::Error(LlmError)`。

### 模式 8:可插拔 Context Engine + 隔离降级

- **定义**:上下文管理不是硬编码,而是注册表 + 契约(assemble/ingest/bootstrap/maintenance),引擎抛错时被 quarantine,降到 fallback 模式。
- **代表**:openclaw(context-engine/registry.ts,796 行,quarantine-health.ts)。
- **适用**:需要"第三方可替换上下文策略 / 故障不致命"的 Agent。
- **laew 借鉴**:P1-5,抽象 ContextEngine 契约。核心接口:
  ```rust
  trait ContextEngine {
      async fn assemble(&self, budget: u32) -> Result<Assembly>;
      async fn ingest(&self, message: Message) -> Result<()>;
      async fn compact(&self) -> Result<()>;
  }
  ```
  引擎抛错时记录 quarantine,降到 fallback engine(默认无压缩)。

### 模式 9:渐进式工具披露(Tool Search)

- **定义**:工具数量爆炸(50+)时不全塞 schema,只暴露 `tool_search / tool_describe / tool_call` 三个元工具,模型按需检索。
- **代表**:openclaw(tool-search.ts,421 行 + code mode 子进程)。
- **适用**:接 MCP 后工具数 > 30 的场景,laew 当前不需要但应预留。
- **laew 借鉴**:P2-1 之后,工具数 > 30 时启用。核心是三个元工具:
  - `tool_search(query)`:文本搜索 + 参数元数据索引
  - `tool_describe(name)`:查看工具详情(含 parameters/outputSchema)
  - `tool_call(name, args)`:通过 name 调用工具

### 模式 10:Skill 即模板 + 延迟加载

- **定义**:Skill = Markdown + YAML frontmatter,只注入 name/description/location 元数据,模型按需 `read` 加载完整内容,节省 token。
- **代表**:pi(延迟加载 + disable-model-invocation)、atomcode(use_skill 工具 + SkillCatalogHook)、opencode(双轨 + 远程拉取)。
- **适用**:知识库 / 最佳实践 / 可复用提示的轻量封装,是 MCP 的"重协议"替代。
- **laew 借鉴**:P1-2,实现 SKILL.md 发现 + frontmatter 解析 + 延迟加载。核心格式:
  ```markdown
  ---
  name: my-skill
  description: 简短描述(≤1024 字符)
  ---
  # 技能正文(Markdown)
  ```
  注入到 system prompt 的格式:
  ```xml
  <available_skills>
    <skill>
      <name>my-skill</name>
      <description>简短描述</description>
      <location>/path/to/SKILL.md</location>
    </skill>
  </available_skills>
  ```
  模型按需 `read` 加载完整内容。

### 模式 11:Skill Workshop 自演化

- **定义**:Agent 扫描历史会话 → 发现改进机会 → 生成 proposal → review → apply/rollback,全程带目标锁与 SQLite 事务。
- **代表**:openclaw(src/skills/workshop/,~40 文件,唯一实现)。
- **适用**:长期运行 / 经验沉淀 / 自我改进,作为 laew 的长期方向。
- **laew 借鉴**:P2-4,长期方向。核心流程:
  1. **History Scan**:扫描会话历史,发现技能改进机会
  2. **Proposal Generation**:生成技能修改提案(去重 + 哈希)
  3. **Review**:后台 agent 评审提案
  4. **Apply/Rollback**:自动应用或回滚,带目标锁与 SQLite 事务

### 模式 12:类型图驱动的 RPC(Typert)

- **定义**:服务端与客户端共享"类型图",`@Remote` 装饰器把 service method 投影成 wire-format callable,编译期约束 schema 漂移。
- **代表**:deepseek-harness(typert/generator + protocol,唯一实现)。
- **适用**:跨进程 / 跨语言 / 类型安全的 RPC,laew 当前不需要(单进程)。
- **laew 借鉴**:P2-5,仅当需要跨进程。核心是 `@Remote` 装饰器 + 类型图生成器,编译期保证 RPC + wire 一致。

### 模式 13:Projection Unit(事件折叠)

- **定义**:Session 是 append-only 事件流,所有状态(goal / plan / inbox)都从事件 fold 出来,pending 纯 fold 设计避免 live mirror 双源不一致。
- **代表**:deepseek-harness(plan-mode pending 纯 fold、goal projection unit)。
- **适用**:需要"可重放 / 可恢复 / 多视图"的持久化会话。
- **laew 借鉴**:P2-3,与三相位状态机一起实现。核心是 `SessionEvent` 流 + `ProjectionUnit` trait:
  ```rust
  trait ProjectionUnit<State, Event> {
      fn init() -> State;
      fn apply(state: State, event: Event) -> State;
  }
  ```
  所有状态(goal / plan / inbox)都从事件 fold 出来,无 live mirror。

### 模式 14:Lane 并发模型

- **定义**:把"运行 / 压缩 / 导航"抽象成并发 lane,每条 lane 持有叶节点,操作在 lane 内串行、跨 lane 并行,共享 seq 序号。
- **代表**:pi(harness/agent-harness.ts + reducer.ts,667 行状态机)。
- **适用**:多任务并行 / 分支会话 / 压缩与运行交错。
- **laew 借鉴**:P2-3,可选,与三相位状态机二选一。核心是 `Lane` 类型 + `reducer` 纯函数状态机。

### 模式 15:错误暂扣(Withholding) + 恢复路径

- **定义**:API 错误(prompt-too-long / max-output-tokens / media-size)不立刻 surface,而是先尝试恢复路径(reactive compact / collapse drain / max_output_tokens_escalate),用尽后才真正报错。
- **代表**:claude(query.ts:175-179,7 个 continue 站点)。
- **适用**:需要"自动恢复 / 用户无感"的长任务场景。
- **laew 借鉴**:P1-1(压缩管线)的一部分。核心是 `is_withheld()` 判断 + 恢复路径队列:
  ```rust
  enum RecoverableError {
      PromptTooLong { context_used: u32 },
      MaxOutputTokens { output_used: u32 },
      MediaSize { size_bytes: u64 },
  }
  ```
  错误暂扣后尝试:context collapse → reactive compact → max_output_tokens_escalate → surface。

---

## 5. laew 借鉴路线图

按 **P0(立即可做) / P1(中期演进) / P2(长期方向)** 分级,每个建议包含借鉴来源、具体做法、预期收益、落地成本。

### P0:立即可做(1-2 周)

| # | 建议 | 借鉴来源 | 具体做法 | 预期收益 | 成本 |
|---|------|---------|---------|---------|------|
| P0-1 | **错误分类表驱动** | openclaw | 重构 `src/llm/` 错误路径,把 Anthropic/OpenAPI 错误码分类为 enum + `*.cases.rs` 测试表,stream 失败编码进终态 | 可解释的错误文案 + 自动重试/降级决策 | 中:需重构错误路径 |
| P0-2 | **执行前审查员模型** | openclaw | 在 BashTool 执行前,用廉价小模型审查命令,输出 `{decision, risk, rationale}`,高风险转人工确认 | 补齐"前置风控",与 QC 形成闭环 | 低:一个 prompt + 一次小模型调用 |
| P0-3 | **doom_loop 检测** | opencode | 在 `agent/mod.rs` 主循环中加连续 N 次相同工具 + 相同输入检测,触发权限询问或终止 | 防止 LLM 死循环,替代部分 QC 场景 | 低:30 行代码 |
| P0-4 | **工具结果截断** | atomcode | 在 `tools/mod.rs` 加 `cap_tool_result`,超大结果截断 + stub 替换 | 防止单工具结果撑爆 context | 低 |

### P1:中期演进(1-2 月)

| # | 建议 | 借鉴来源 | 具体做法 | 预期收益 | 成本 |
|---|------|---------|---------|---------|------|
| P1-1 | **四级压缩管线** | claude + opencode | 实现 `microCompact`(工具结果擦除) + `autoCompact`(阈值触发) + `reactiveCompact`(错误恢复) + tail budget 保留 | 长会话不再溢出,支持大代码库 | 高:需新增 compaction 模块 |
| P1-2 | **Skill 系统(轻量)** | pi + atomcode | 实现 `SKILL.md` 发现 + frontmatter 解析 + 延迟加载(只注入元数据,按需 read) | 知识库 / 最佳实践复用,替代部分 MCP 场景 | 中:需新增 skill 模块 |
| P1-3 | **双层循环 + Steering** | pi + openclaw | 重构主循环为双层 while + PendingMessageQueue,支持用户中途插入消息 | 用户可中断 / 子 Agent 可续接 | 中:需重构主循环 |
| P1-4 | **Agent Harness trait 抽象** | openclaw | 抽出 `AgentHarness` trait,让"调用外部 CLI Agent(claude/codex)"成为可插拔后端 | 不必自研全部能力,可外包给成熟 Agent | 中:需重构 orchestrator |
| P1-5 | **可插拔 Context Engine** | openclaw | 定义 `ContextEngine` 契约(assemble/ingest/compact),支持第三方替换 + quarantine 隔离 | 故障不致命 + 可扩展 RAG/向量召回 | 高:需抽象上下文层 |

### P2:长期方向(3-6 月)

| # | 建议 | 借鉴来源 | 具体做法 | 预期收益 | 成本 |
|---|------|---------|---------|---------|------|
| P2-1 | **MCP 客户端(最小可用)** | deepseek-harness + opencode | 实现 stdio + StreamableHTTP 双传输 + 工具同步桥 + 指数退避重连 | 接入生态工具(浏览器 / DB / 搜索) | 高:需新增 mcp 模块 |
| P2-2 | **Goal 域 + Round Driver** | deepseek-harness | 把 Yolo 三档分类替换为显式 Goal 域(maxGoalRounds 配额 + 四 phase 状态机) | 比 LLM 分类更可控、可解释 | 高:需重构任务分类层 |
| P2-3 | **三相位状态机 + Inbox** | deepseek-harness | 重构 Agent 主循环为 idle/maintenance/running + 持久化 inbox 双队列 | 解决 cancel 重入 + resume 零成本还原 | 高:需重构核心循环 |
| P2-4 | **Skill Workshop 自演化** | openclaw | 实现 history scan → proposal → review → apply/rollback 闭环 | Agent 自我改进 + 经验沉淀 | 高:长期方向 |
| P2-5 | **类型图驱动的 RPC** | deepseek-harness | 如走跨进程架构,引入类型图保证 wire 一致性 | 编译期 schema 安全 | 高:仅当需要跨进程 |

### 优先级决策树

```
是否需要长会话(>20 轮)?
  ├─ 是 → P1-1(压缩管线) 优先
  └─ 否 → P0-1(错误分类) + P0-2(审查员) 起步

是否需要接第三方工具生态?
  ├─ 是 → P2-1(MCP 客户端)
  └─ 否 → P1-2(Skill 系统) 替代

是否需要多 Agent 协作?
  ├─ 是 → P1-3(双层循环) + P1-4(Harness 抽象)
  └─ 否 → 保持 6 角色,优化单角色体验

是否需要用户中途中断?
  ├─ 是 → P1-3(双层循环 + steering)
  └─ 否 → 保持单线状态
```

### 落地节奏建议

| 阶段 | 时间 | 目标 | 关键里程碑 |
|------|------|------|-----------|
| **Phase 1** | 第 1-2 周 | P0 四项落地 | 错误分类 + 审查员 + doom_loop + 工具截断 |
| **Phase 2** | 第 3-6 周 | P1-1 + P1-2 落地 | 压缩管线 + Skill 系统 |
| **Phase 3** | 第 7-10 周 | P1-3 + P1-4 落地 | 双层循环 + Harness 抽象 |
| **Phase 4** | 第 11-14 周 | P1-5 + P2-1 落地 | Context Engine + MCP 客户端 |
| **Phase 5** | 第 15-20 周 | P2-2 + P2-3 落地 | Goal 域 + 三相位状态机 |
| **Phase 6** | 第 21-24 周 | P2-4 + P2-5 可选 | Skill Workshop + 类型图 RPC |

---

## 6. 反模式警示

从 6 个 Agent 中识别出 **6 个值得警惕的反模式**,每个给出定义、代表、危害、规避。

### 反模式 1:文件粒度过细

- **定义**:把一个 run / attempt 拆成 40+ 个 500-800 行文件,每个文件一个阶段。
- **代表**:openclaw(`attempt-setup / attempt-session-prepare / attempt-prompt-build / ...`,每个 500-800 行)。
- **危害**:对 < 10k 行的项目属于严重过度工程,认知负担高、重构困难、git 冲突频繁。
- **规避**:laew 当前 ~8k 行,保持"一模块一职责",单文件 < 1500 行。新增功能优先扩展现有模块,不到万不得已不拆新文件。

### 反模式 2:测试量级失控

- **定义**:测试代码量远超生产代码(3:1),是团队规模产物。
- **代表**:openclaw(301 万行测试 vs 201 万行生产)。
- **危害**:不可作为中小项目的参考,维护成本极高,CI 时间爆炸。
- **规避**:laew 保持测试覆盖核心路径(解析 / 转换 / 工具 / 协议),不追求 100% 文件覆盖率。测试代码量控制在生产代码的 0.5-1 倍。

### 反模式 3:巨型接口 + 上帝对象

- **定义**:Tool trait 40+ 方法、ToolUseContext 50+ 字段,一个对象贯穿全局。
- **代表**:claude(`Tool.ts` 的 ~40 方法接口 + `buildTool` 默认值,`ToolUseContext` 的 ~50 字段)。
- **危害**:新工具只需覆盖关心的方法,但阅读者必须理解整个接口;修改上帝对象影响全局;测试需要 mock 大量无关方法。
- **规避**:laew 的 `Tool` trait 保持精简(name/description/schema/execute),扩展用组合(middleware / hook)而非继承。上下文对象按职责拆分,不搞"上帝 Context"。

### 反模式 4:Everything-is-a-Plugin 的隐性成本

- **定义**:包括 Agent 循环本身在内所有能力都是插件,无特权内核。
- **代表**:deepseek-harness(247 包 / 51 组,Cordis 时间空间组合)。
- **危害**:学习曲线极陡,调试困难(事件 waterfall 链路长),性能开销(每次调用走插件链),文档负担重。
- **规避**:laew 保持"核心循环硬编码 + 能力可插拔",不要为了插件化而插件化。只有真正需要第三方替换的部分(Provider / ContextEngine)才抽象为插件。

### 反模式 5:拒绝 MCP 的生态孤岛

- **定义**:明确拒绝 MCP,用 Skill + Extension 自建生态。
- **代表**:pi(`README.md:499`:"**No MCP.** Build CLI tools with READMEs")。
- **危害**:无法接入浏览器 / DB / 搜索等成熟 MCP 服务器,重复造轮子,用户迁移成本高。
- **规避**:laew 应在 P2 阶段支持 MCP 客户端,Skill 作为轻量补充而非替代。MCP 是生态标准,拒绝它等于拒绝整个工具生态。

### 反模式 6:中央编排器的刚性瓶颈

- **定义**:6 个固定角色 + 三档硬编码分类,所有任务必须走 MultiAgentOrchestrator。
- **代表**:laew(当前架构)。
- **危害**:简单任务也过重(必须走 Yolo → SubAgent → QC),无法灵活应对新场景,用户无法跳过不需要的环节。
- **借鉴**:吸收 claude / opencode 的"模型自组织"思想,在 hard 任务保留编排,simple/medium 任务允许模型自主决策(可配置开关)。提供"快速模式"跳过 Yolo 分类和 QC。

---

## 7. 性能与可观测性对比(补充)

| 维度 | laew | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi |
|------|------|----------|------------|------------------|----------|----------|-----|
| 构建优化 | cargo release | opt-level=z + lto + strip + panic=abort | Bun bundle + feature() DCE | Cordis 插件懒加载 | tsdown(rolldown) | esbuild | Bun standalone |
| 启动速度 | 中 | 快(极致小体积) | 快(fast-path + 动态 import) | 中(插件加载) | 中 | 中 | 快 |
| 二进制大小 | 中 | 极小(panic=abort + strip) | 小(Bun 打包) | 大(多插件) | 大 | 中 | 小 |
| 遥测 | 无 | atomcode-telemetry | cost-tracker + metricsOptOut | session-telemetry + otel | OpenTelemetry | @effect/opentelemetry | pi-telemetry |
| 可视化 | TUI 横幅 | TUI + WebUI + Daemon | ContextVisualization.tsx | Web UI(40 包) | Control UI(82 万行) | Desktop + Web | TUI |
| 日志 | 无 | datalog.rs(turn 级 markdown + JSONL) | 无 | SessionEvent 流 | trajectory/ audit/ | EventBus + SSE | 无 |
| 调试 | 打印 | /snapshot + AgentCommand | --dump-system-prompt | snapshots/ 录制回放 | 无 | Heap Snapshot | 无 |
| 成本追踪 | 无 | 无 | cost-tracker + costHook | token-meter | 无 | 无 | 无 |

**对 laew 的启示**:
- **构建优化**:借鉴 atomcode 的 `opt-level=z + lto + strip + panic=abort`,可显著减小二进制体积
- **启动速度**:借鉴 claude 的 fast-path + 动态 import,对 `--version` / `--help` 等命令做快速通道
- **遥测**:P2 阶段可加入 OpenTelemetry,但当前不是优先项
- **成本追踪**:claude 的 cost-tracker 是用户刚需,laew 应在 P1 阶段加入

---

## 附录 A:关键文件索引(供跳读)

| 项目 | 核心循环 | 工具系统 | Context | MCP | Skill |
|------|---------|---------|---------|-----|-------|
| laew | `src/agent/mod.rs` | `src/agent/tools/` | 无 | 无 | 无 |
| atomcode | `kernel/agent.rs:1418` | `kernel/tool.rs:277` | `capabilities/compaction.rs` | `capabilities/mcp/` | `capabilities/skills/` |
| claude | `src/query.ts:241` | `src/Tool.ts:362` | `services/compact/` | `services/mcp/client.ts` | `skills/loadSkillsDir.ts` |
| dsh | `core/agent-loop/agent.ts` | `core/tools/` | `core/system-prompt/` | `mcp/mcp-client/` | `skill/skill/` |
| openclaw | `agent-core/agent-loop.ts` | `agents/agent-tools.ts` | `context-engine/registry.ts` | `agents/agent-bundle-mcp-runtime.ts` | `skills/workshop/` |
| opencode | `session/prompt.ts:1081` | `tool/tool.ts:151` | `session/compaction.ts` | `mcp/index.ts` | `skill/index.ts` |
| pi | `agent/agent-loop.ts:156` | `core/tools/` | `core/compaction/` | 无 | `core/skills.ts` |

## 附录 B:术语对照表

| laew 术语 | 外部对应 |
|-----------|---------|
| Yolo Agent | claude 的 auto-mode 分类器、dsh 的 Goal 域、openclaw 的 exec reviewer |
| MultiAgentOrchestrator | atomcode 的 CodingRuntime、dsh 的 Cordis 编排、openclaw 的 Gateway |
| Quality-Check Agent | atomcode 的 VerifyCadenceHook、claude 的 stop hooks、dsh 的 guard plugin、openclaw 的 exec auto-reviewer |
| Plan Agent | claude 的 Plan Agent + Plan Mode、opencode 的 plan Agent、dsh 的 PlanModeController |
| SubAgent-Work | claude 的 AgentTool / runAgent、dsh 的 SubagentRuntime、openclaw 的 sessions_spawn、opencode 的 task 工具、pi 的 subagent 扩展 |
| 项目上下文注入 | claude 的 prependUserContext + MagicDocs、opencode 的 instruction.ts、pi 的 resource-loader.ts |
| SessionContext 摘要 | claude 的 SessionMemory + extractMemories、dsh 的 projection unit、opencode 的 summary.ts |

## 附录 C:调研范围与局限

**输入范围**:12 份报告,覆盖 6 个项目 ×2(源码调研 + 深度分析)。

**局限**:
1. claude 代码库是"源码库视图"(无 package.json / git 历史),部分模块可能缺失。
2. openclaw 与 deepseek-harness 是巨型项目(200+ 万行 / 247 包),仅覆盖核心模块。
3. 所有报告基于 2026-09-04 快照,后续版本可能变化。
4. 性能数据(启动速度 / 内存)为定性对比,无基准测试。
5. 未覆盖的项目:Codex(OpenAI)、Hermes、OpenCode(Go)、WorkBuddy 等。

**建议**:本报告作为架构决策参考,具体实现前应重新拉取目标项目最新代码验证。特别是 P2-1(MCP 客户端)应参考 `@modelcontextprotocol/sdk` 最新文档。

---

**报告完成**。共 7 个项目、12 份输入、15 个设计模式、14 条借鉴建议(P0=4 / P1=5 / P2=5)、6 个反模式、8 个核心维度 + 2 个补充维度(架构决策树 / 性能与可观测性)。建议按 P0 → P1 → P2 顺序落地,优先补齐"错误分类 + 审查员 + doom_loop + 工具截断"四个低成本高收益项,再推进"压缩管线 + Skill 系统"两个中期目标,最后根据生态需求决定 MCP 客户端与三相位状态机的优先级。
