# 232 工作流编排与 Goal 状态机进阶实战

> 编号段 HS01–HS10 · 聚焦「laew 第 10 角色 WorkFlow Agent 的进阶编排能力」:Goal 六态迁移图与合法性 / Phase DAG 拓扑序与失败策略矩阵 / Squad 多角色并行调度(AllMustPass/Quorum/LeaderDecides) / AdaptiveLoop 自适应修复六策略 / QualityGate 四级门禁(Task/Squad/Phase/Goal) / TemplateLibrary 模板分类与参数化渲染 / BatchChannel 批量分片并发限速 / WorkFlow 与 SubAgent/WindowUse 跨角色协作 / 端到端超大任务串联 / 进度报告与失败回流
>
> 与现有维度互补说明:
> - `231-WorkFlow工作流编排与Goal状态机实战`(HR 维)是**基础编排**;本文件是**进阶编排**——测 WorkFlow Agent 在复杂失败场景、跨角色协作、批量任务等高阶能力。
> - `22-分布式系统与微服务架构`(DB 维)是**外部分布式系统**;本文件是**laew 内部第 10 角色**的 WorkFlow 调度引擎。
> - `128-CI/CD流水线工程与GitOps实战`(DI 维)是**外部 CI/CD**;本文件是**laew 内部**的 WorkFlow 编排。
> - `93-包管理与依赖解析工程`(GR 维)是**依赖解析**;本文件是**任务编排**。
> - `209-数字取证与电子证据分析工程`(HQ 维)是**取证分析**;本文件是**工作流引擎**。
>
> **本文件独特主题**:`Goal` 六态(Pending/Pursuing/Blocked/Paused/Satisfied/Failed) / `GoalStore` SQLite 持久化 / `Phase` 依赖图 DAG + 失败策略(Abort/Continue/Retry/Skip) / `Squad` 多 Agent 协同(SquadStrategy:AllMustPass/Quorum/LeaderDecides) / `SquadMember` 角色(Leader/Worker/Verifier) / `AdaptiveLoop` 修复策略(SimplifyScope/ChangeApproach/AddContext/SplitTask/EscalateToPlan/AskUser) / `QualityGate` 四级门禁(Task/Squad/Phase/Goal) / `TemplateLibrary` 模板分类(Refactoring/Testing/Analysis/Migration/Documentation/Custom) / `BatchChannel` 并发限速(batch_chunk_size/max_parallel_batches) / 与 MainWork 的衔接(Yolo→Main→WorkFlow 路由) / 进度报告与失败回流 Yolo。

---

### HS01 Goal 六态迁移图与合法性校验

- **预期档位**: medium
- **考察维度**: GoalState 六态枚举 / 合法迁移图 / 非法迁移拒绝
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs01_goal_fsm.py`:Goal 状态机驱动(参考 `src/agent/workflow/goal.rs` 公开枚举,纯函数实现):(a) 定义六态 `Pending/Pursuing/Blocked/Paused/Satisfied/Failed`;(b) 定义合法迁移图(Pending→{Pursuing};Pursuing→{Blocked,Paused,Satisfied,Failed};Blocked→{Pursuing,Failed};Paused→{Pursuing,Failed};Satisfied/Failed→无出向);(c) 提供 `transition(current, event) -> Result<NewState>` 函数,对非法迁移返回错误且不动 current;(d) 提供 `Goal` 结构含 `id/title/state/priority(1-10)/dependencies[]/acceptance_criteria[]/subgoals[]/retry_count/max_retries(3)/error_text/metadata/created_at/updated_at/completed_at`;(e) 提供 `GoalStore`(内存 HashMap + 序列化到 JSON 的 dump/load)模拟持久化。
  2. Bash:运行 `hs01_goal_fsm.py` 自带 mini-test:每条合法迁移走一次 + 每条非法迁移走一次(Pending→Satisfied/Failed→Pending/Satisfied→Pursuing 等);断言全合法路径成功且 updated_at 单调递增;非法路径返回错误且 current 不变;dump/load 往返一致。
  3. Read 源码 + Bash 断言:含六态枚举、含合法迁移图、含 GoalStore 持久化、含时间戳单调性。

### HS02 Phase DAG 拓扑序与失败策略矩阵

- **预期档位**: hard
- **考察维度**: Phase DAG 拓扑序 Kahn 算法 / PhaseFailurePolicy 四种语义
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs02_phase_dag.py`:Phase 编排器(纯函数,模拟执行):(a) `Phase` 结构含 `id/name/depends_on[]/action/failure_policy`;(b) `PhaseFailurePolicy` 四种:Abort(整 WorkFlow 终止)/Continue(忽略继续下一阶段,下游不依赖则可)/Retry(N 次,指数退避,全失败才按上层策略)/Skip(标记 skipped 不阻塞下游);(c) 拓扑序 Kahn 算法找入度 0 节点入队,出队时把其下游入度减一;(d) 模拟执行器按拓扑序逐阶段跑;某 Phase 失败按其 policy 决定后续;(e) 输出时间线 + 终态(Completed/Partial/Failed)。
  2. Bash:跑 4 个 Phase 拓扑(`P1→P2→P3`、`P1→P3`、`P2,P3 并行`、`P4→P5`)各配不同 policy,断言:拓扑序正确(无依赖先跑);Retry 阶段真的重试 N 次;Abort 阶段一失败后续全跳;Skip 阶段失败后下游若不依赖仍执行。
  3. Read 源码 + Bash 断言:含 Kahn、含四种 policy、含时间线输出。

### HS03 Squad 多角色并行调度策略

- **预期档位**: hard
- **考察维度**: SquadStrategy 三种 / SquadMember 角色组合(Leader/Worker/Verifier) / 并发限速
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs03_squad_dispatch.py`:Squad 调度器(纯模拟,各成员用 `time.sleep` 模拟耗时):(a) `SquadMember{member_id, role∈{Leader,Worker,Verifier}, task, expected_output, status, result, error}`;(b) `SquadStrategy` 三种:AllMustPass(全员通过才算完成)/Quorum(n)(多数通过即可)/LeaderDecides(Leader 汇总判定);(c) 并发限速:`max_concurrent=3` 用 `Semaphore` 模拟;(d) LeaderDecides 策略下 Leader 任务在最后跑且合并所有 Worker/Verifier 结果;(e) `SquadDispatchResult{squad_id, success, member_results, summary}`。
  2. Bash:三策略各跑一个 5 成员 squad(estimated_seconds 固定 1s),断言:AllMustPass 全员成功才 success=true;Quorum(3) 下 3/5 通过即 success=true;LeaderDecides 下 Leader 汇总后判定;且 `max_concurrent=3` 时并行数从未超过 3。
  3. Read 源码 + Bash 断言:含三种 strategy、含 Semaphore 限速、含角色枚举。

### HS04 AdaptiveLoop 自适应修复六策略

- **预期档位**: hard
- **考察维度**: RepairStrategy 触发条件 / 升级与终止 / 循环预算
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs04_adaptive_loop.py`:AdaptiveLoop(纯函数 + 模拟子任务):(a) `RepairStrategy` 六种:SimplifyScope(缩小范围)/ChangeApproach(换思路)/AddContext(补充上下文)/SplitTask(拆子任务)/EscalateToPlan(升级回 Plan)/Abort(终止并回流 Yolo);(b) 升级策略:同一 Phase 连续失败 → 按 SimplifyScope→ChangeApproach→AddContext→SplitTask→EscalateToPlan→Abort 链升级;(c) 全局循环预算 `MAX_ADAPTIVE_ATTEMPTS=5`,超过强制 Abort;(d) `MAX_UNPRODUCTIVE=3` 连续无进展也触发升级;(e) 注入可控失败的 mock 子任务,精确观察升级路径。
  2. Bash:三组场景(总是失败 → Abort/第一次失败第二次成功 → SimplifyScope 即过/Phase 拆解后才成功 → SplitTask),断言每组最终态与循环次数。
  3. Read 源码 + Bash 断言:含六级升级链、含 MAX_ADAPTIVE_ATTEMPTS 常量、含 SplitTask 拆分逻辑。

### HS05 QualityGate 四级门禁与分层检查

- **预期档位**: medium
- **考察维度**: QualityGate 四级(Task/Squad/Phase/Goal) / 通过/降级/阻断 / strict_mode
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs05_quality_gate.py`:QualityGate 评估器(纯函数):(a) `QualityLevel` 四级:Task(单任务)/Squad(小队)/Phase(阶段)/Goal(目标);(b) `QualityGate{level, enabled, strict_mode}`;(c) `check(output, expected) -> QualityGateResult{passed, level, issues[], suggestion}`;(d) 默认按 Task→Squad→Phase→Goal 顺序,任一阻断级失败立即终止;(e) strict_mode 下检查期望关键词 + 失败措辞。
  2. Bash:三组 artifact(全部通过/Goal 级失败阻断/Task 级 strict_mode 失败),断言:全通过结果 PASS;Goal fail 立即终止(Task/Squad/Phase 已跑但后续未跑);Task strict_mode 失败结果带 issues。
  3. Read 源码 + Bash 断言:含四级定义、含阻断与降级语义、含评估顺序。

### HS06 TemplateLibrary 模板分类与参数化渲染

- **预期档位**: medium
- **考察维度**: TemplateCategory 分类 / 模板选择 / 参数化渲染 / usage_count
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs06_template_lib.py`:TemplateLibrary(纯内存实现):(a) `TemplateCategory` 枚举:Refactoring/Testing/Analysis/Migration/Documentation/Custom(String);(b) 内置 6 个模板,每个含 `id/name/description/category/phase_names[]/default_config/tags[]/usage_count`;(c) `select(category, query) -> Template?`(按关键词匹配 name/description/tags);(d) `render(template, params) -> Goal` 渲染参数到 Phase 列表(如 `{file_pattern}` 替换为真实 glob);(e) 参数缺失或类型不匹配返回明确错误;选中后 `usage_count` 自增。
  2. Bash:三组调用(选 Refactoring 模板并填参数成功/参数缺值报错/分类无匹配返回 None),断言渲染产物包含用户参数、缺值错误信息明确、无匹配返回 None 不崩溃、选中后 usage_count 自增。
  3. Read 源码 + Bash 断言:含六分类、含 select/render 两函数、含参数校验、含 usage_count 自增。

### HS07 BatchChannel 批量任务并发限速

- **预期档位**: medium
- **考察维度**: 批量任务分片 / 并发限速 / 进度回报 / 失败重试
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs07_batch_channel.py`:BatchChannel(纯模拟):(a) `BatchTask` trait:`task_id()` + `task_description()`;(b) `BatchResult{total, succeeded, failed, skipped, failures: Vec<(task_id, error)>}` + `merge()` + `success_rate()` + `all_succeeded()`;(c) `BatchChannel{config}`:`chunk_tasks(tasks)` 按 `batch_chunk_size` 分片 + `process(tasks)` 并行处理各批次;(d) 并发限速:`max_parallel_batches=3` 用 `Semaphore` 模拟;(e) 失败任务收集到 `failures` 不中断后续。
  2. Bash:跑 100 个 mock 任务(其中 5 个故意失败),`batch_chunk_size=10`,`max_parallel_batches=3`,断言:total=100, succeeded=95, failed=5, success_rate=0.95, all_succeeded=false, failures 列表含 5 条。
  3. Read 源码 + Bash 断言:含分片逻辑、含并发限速、含 BatchResult 统计。

### HS08 WorkFlow 与 SubAgent 跨角色协作

- **预期档位**: hard
- **考察维度**: WorkFlow 调度 SubAgent 执行 Phase / 结果回传 / 失败回流
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs08_wf_subagent.py`:WorkFlow→SubAgent 协作模拟(纯函数):(a) `WorkFlowRunner` 持 `SubAgentRunner`;(b) Phase 执行时调用 `sub_agent.run_unit(input, session_id) -> SubFlowOutcome`;(c) SubFlowOutcome 成功 → Phase 标记 Completed;失败 → 按 PhaseFailurePolicy 处理;(d) 模拟 3 个 Phase(P1 成功/P2 失败重试后成功/P3 失败触发 Abort),断言:P1 Completed, P2 Retry 后 Completed, P3 触发 Abort 后 WorkFlow 终态 Failed。
  2. Bash:跑上述场景,断言 WorkFlowResult{goal_state, phases_completed, total_phases, tasks_executed, tasks_succeeded, summary, error} 各字段正确。
  3. Read 源码 + Bash 断言:含 WorkFlowRunner 持 SubAgentRunner、含 Phase 失败回流、含 WorkflowResult 结构。

### HS09 WorkFlow 与 WindowUse 跨角色协作

- **预期档位**: hard
- **考察维度**: WorkFlow 调度 WindowUse 操控桌面 / AgentMessage 消息桥 / pending_data 传递
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs09_wf_windowuse.py`:WorkFlow→WindowUse 协作模拟(纯函数):(a) WorkFlow Phase 含 `delegate_to="windowuse"` 字段;(b) WindowUse 读窗口文本后通过 `AgentMessage{from_role=WindowUse, to_role=SubAgentWork, payload=WindowTextRead{...}}` 发消息;(c) SubAgent 处理后通过 `AgentMessage{from_role=SubAgentWork, to_role=WindowUse, payload=TextToWindow{...}}` 回写;(d) 模拟 2 个 Phase(P1: WindowUse 读 → SubAgent 处理 → WindowUse 写; P2: 纯 SubAgent 计算),断言:AgentMessage 表含 2 条消息(1 WindowTextRead + 1 TextToWindow),consumed 标记正确。
  2. Bash:跑上述场景,断言消息序列正确、consumed 幂等、pending_data 跨 Phase 传递。
  3. Read 源码 + Bash 断言:含 delegate_to 路由、含 AgentMessage 消息桥、含 pending_data 传递。

### HS10 端到端超大任务串联与进度报告

- **预期档位**: hard
- **考察维度**: Yolo→Plan→WorkFlow→SubAgent→QC→SessionContext 全链路 / 进度报告 / 失败回流
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hs10_e2e_workflow.py`:端到端超大任务模拟(纯函数):(a) Yolo 分类 hard → 激活 Plan Agent 生成方案;(b) Plan 拆解为 5 个 Phase(P1→P2→P3, P4→P5 并行);(c) WorkFlow 调度 Squad 执行;(d) 每个 Phase 完成后 QualityGate 检查;(e) 全部完成后 SessionContext 生成摘要;(f) 模拟 P3 失败 → AdaptiveLoop 升级 → 重试成功;(g) 输出完整时间线 + 终态报告。
  2. Bash:跑上述场景,断言:Plan 生成 5 Phase、WorkFlow 完成 5/5、QualityGate 全 Pass、SessionContext 摘要非空、终态报告含 goal_state=Satisfied。
  3. Read 源码 + Bash 断言:含全链路串联、含进度报告、含失败回流与重试。
