# 231 WorkFlow 工作流编排与 Goal 状态机实战

> 编号段 HR01–HR10 · 聚焦「laew 第 10 角色 WorkFlow Agent 的超大型复杂任务编排」:Goal 状态机持久化与迁移 / Phase 阶段编排与失败策略 / Squad 多角色并行调度 / AdaptiveLoop 自适应修复 / QualityGate 五级门禁 / TemplateLibrary 模板复用 / BatchChannel 批量任务 / WorkFlow 与 SubAgent / WindowUse 跨角色协作 / 端到端超大任务串联
>
> 与现有维度互补说明:
> - `57-经典小游戏复刻与游戏编程实战`(CG 维)用游戏讲工程;本文件用工程讲工程 —— 测的是 WorkFlow Agent 自身(第 10 角色)。
> - `33-嵌入式系统与物联网开发`(DF 维)嵌入式流水线;本文件软件流水线(Goal → Phase → Squad → QualityGate)。
> - `209-数字取证与电子证据分析工程`(HQ 维)平行编号但完全不同领域 —— 本文件测 WorkFlow Agent 实现细节,非取证。
> - `93-包管理与依赖解析工程`(GR 维)与 `55-领域驱动设计DDD专题`(BW 维)是**建模维度**;本文件是**编排引擎维度**。
> - `128-CI/CD流水线工程与GitOps实战`(DI 维)是**外部 CI/CD**(Jenkins/GitHub Actions);本文件是**laew 内部第 10 角色**的 WorkFlow 调度。
> - 既有 `docs/WorkFlowAgent设计与实现/` 是设计文档;本文件是**面向用户的测试提示词**,验证 WorkFlow 在真实多轮对话中的可观测行为。
>
> **本文件独特主题**:`Goal` 状态机(Pending/Active/Paused/Completed/Failed/Cancelled 六态)/ `GoalStore` SQLite 持久化 / `Phase` 依赖图 DAG + 失败策略(Abort/Continue/Retry/Skip)/ `Squad` 多 Agent 协同(SquadStrategy:FanOut/Pipeline/LeaderFollower)/ `SquadMember` 角色(Squad/SubAgent/WindowUse/Skill)/ `AdaptiveLoop` 修复策略(Retry/Replan/Decompose/Abort)/ `QualityGate` 五级门禁(Schema/Semantic/Functional/Performance/Safety)/ `TemplateLibrary` 模板分类 / `BatchChannel` 并发限速 / 与 MainWork 的衔接(Yolo→Main→WorkFlow 路由)。

---

### HR01 Goal 状态机持久化与状态迁移

- **预期档位**: medium
- **考察维度**: GoalStore CRUD / 状态机合法性 / 时间戳完整性
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr01_goal.py`:Goal 状态机驱动(参考 `src/agent/workflow/goal.rs` 公开枚举,纯函数实现,不动真实 SQLite):(a) 定义六态 `Pending/Active/Paused/Completed/Failed/Cancelled`;(b) 定义合法迁移图(Pending→{Active,Cancelled};Active→{Paused,Completed,Failed};Paused→{Active,Cancelled};Completed/Failed→无出向;Cancelled→无出向);(c) 提供 `transition(current, event) -> Result<NewState>` 函数,对非法迁移返回错误且不动 current;(d) 提供 `Goal` 结构含 `id/title/state/created_at/updated_at/priority`,每次合法迁移更新 `updated_at`;(e) 提供 `GoalStore`(内存 HashMap + 序列化到 JSON 的 dump/load)模拟持久化。
  2. Bash:运行 `hr01_goal.py` 自带 mini-test:每条合法迁移走一次 + 每条非法迁移走一次(Pending→Completed/Failed→Active/Cancelled→Active 等);断言全合法路径成功且 updated_at 单调递增;非法路径返回错误且 current 不变;dump/load 往返一致。
  3. Read 源码 + Bash 断言:含六态枚举、含合法迁移图、含 GoalStore 持久化、含时间戳单调性。

### HR02 Phase 阶段编排与失败策略矩阵

- **预期档位**: hard
- **考察维度**: Phase DAG 拓扑序 / PhaseFailurePolicy 四种语义
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr02_phase.py`:Phase 编排器(纯函数,模拟执行):(a) `Phase` 结构含 `id/name/depends_on[]/action/failure_policy`;(b) `PhaseFailurePolicy` 四种:Abort(整 WorkFlow 终止)/Continue(忽略继续下一阶段,下游不依赖则可)/Retry(N 次,指数退避,全失败才按上层策略)/Skip(标记 skipped 不阻塞下游);(c) 拓扑序 Kahn 算法找入度 0 节点入队,出队时把其下游入度减一;(d) 模拟执行器按拓扑序逐阶段跑;某 Phase 失败按其 policy 决定后续;(e) 输出时间线 + 终态(Completed/Partial/Failed)。
  2. Bash:跑 4 个 Phase 拓扑(`P1→P2→P3`、`P1→P3`、`P2,P3 并行`、`P4→P5`)各配不同 policy,断言:拓扑序正确(无依赖先跑);Retry 阶段真的重试 N 次;Abort 阶段一失败后续全跳;Skip 阶段失败后下游若不依赖仍执行。
  3. Read 源码 + Bash 断言:含 Kahn、含四种 policy、含时间线输出。

### HR03 Squad 多角色并行调度策略

- **预期档位**: hard
- **考察维度**: SquadStrategy 三种 / SquadMember 角色组合 / 并发限速
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr03_squad.py`:Squad 调度器(纯模拟,各成员用 `time.sleep` 模拟耗时):(a) `SquadMember{id, role∈{Squad/SubAgent/WindowUse/Skill}, estimated_seconds}`;(b) `SquadStrategy` 三种:FanOut(全员并行,总耗时 = max)/Pipeline(分阶段流水线,每阶段 FanOut,总耗时 = Σ stage_max)/LeaderFollower(Leader 派单,Followers 抢任务,Leader 汇总);(c) 并发限速:`max_concurrent=3` 用 `Semaphore` 模拟(可简化为计数信号量);(d) LeaderFollower 策略下 Leader 任务在最后跑且合并所有 Follower 结果。
  2. Bash:三策略各跑一个 5 成员 squad(estimated_seconds 固定 1s),断言:FanOut 耗时 ≈ 1s、Pipeline(2 阶段各 3 成员)耗时 ≈ 2s、LeaderFollower 耗时 ≈ 6s(1+1+1+1+1+Leader 汇总),且 `max_concurrent=3` 时并行数从未超过 3。
  3. Read 源码 + Bash 断言:含三种 strategy、含 Semaphore 限速、含角色枚举。

### HR04 AdaptiveLoop 自适应修复四策略

- **预期档位**: hard
- **考察维度**: RepairStrategy 触发条件 / 升级与终止 / 循环预算
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr04_adapt.py`:AdaptiveLoop(纯函数 + 模拟子任务):(a) `RepairStrategy` 四种:Retry(重试当前 Phase,带抖动)/Replan(回到 Plan 阶段重新拆解 Phase)/Decompose(把失败 Phase 拆成更细子 Phase)/Abort(终止并回流 Yolo);(b) 升级策略:同一 Phase 连续 Retry 2 次失败 → 升级 Replan;Replan 后仍同 Phase 失败 → 升级 Decompose;Decompose 后仍失败 → Abort;(c) 全局循环预算 `MAX_LOOP=8`,超过强制 Abort;(d) 注入可控失败的 mock 子任务,精确观察升级路径。
  2. Bash:三组场景(总是失败 → Abort/第一次失败第二次成功 → Retry 即过/Phase 拆解后才成功 → Decompose),断言每组最终态与循环次数。
 3. Read 源码 + Bash 断言:含四级升级链、含 MAX_LOOP 常量、含 Decompose 拆分逻辑。

### HR05 QualityGate 五级门禁与流水线复用

- **预期档位**: medium
- **考察维度**: QualityGate 五级(Semantic/Functional/Schema/Performance/Safety)/ 通过/降级/阻断
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr05_gate.py`:QualityGate 评估器(纯函数):(a) `QualityGateLevel` 五种,每级定义一组 mock 评估规则(用 JSON 描述断言条件,如 Semantic:产出物字段名匹配预期列表;Functional:模拟跑 main 函数返回 0;Schema:JSON Schema 校验;Performance:耗时 < 阈值;Safety:无敏感词);(b) `evaluate(gate, artifact) -> QualityGateResult{passed, level, details}`;(c) 默认按 Semantic→Functional→Schema→Performance→Safety 顺序,任一阻断级(Schema/Safety)失败立即终止;(d) 通过级可降级(Performance 未达标但未超 1.5 倍阈值 → warn 而非 fail)。
  2. Bash:三组 artifact(全部通过/Safety 失败阻断/Performance 超 1.5 倍降级),断言:全通过结果 PASS;Safety fail 立即终止(Schema 与之前的级已跑但后续未跑);Performance 超标但 ≤ 1.5 倍结果带 WARN。
  3. Read 源码 + Bash 断言:含五级定义、含阻断与降级语义、含评估顺序。

### HR06 TemplateLibrary 模板分类与复用

- **预期档位**: medium
- **考察维度**: TemplateCategory 分类 / 模板选择 / 参数化渲染
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr06_template.py`:TemplateLibrary(纯内存实现):(a) `TemplateCategory` 枚举:Refactor/Migration/CodeGen/Review/AutoFix/Deploy;(b) 内置 6 个模板,每个含 `id/name/category/phases[]/parameters{}`(parameters 为参数 Schema);(c) `select(category, query) -> Template?`(按关键词匹配 name/description);(d) `render(template, params) -> Goal` 渲染参数到 Phase 列表(如 `{file_pattern}` 替换为真实 glob);(e) 参数缺失或类型不匹配返回明确错误。
  2. Bash:三组调用(选 Refactor 模板并填参数成功/参数缺值报错/分类无匹配返回 None),断言渲染产物包含用户参数、缺值错误信息明确、无匹配返回 None 不崩溃。
  3. Read 源码 + Bash 断言:含六分类、含 select/render 两函数、含参数校验。

### HR07 BatchChannel 批量任务并发限速

- **预期档位**: medium
- **考察维度**: 批量任务分片 / 并发限速 / 进度回报
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr07_batch.py`:BatchChannel(纯模拟):(a) `BatchTask{id, payload}` 列表 N 个,(b) `BatchChannel(concurrency=4, queue)` 用 Semaphore 限速;(c) 提供 `submit_all(tasks) -> BatchResult` 返回 `{success[], failed[], elapsed_seconds}`;(d) 进度回报:`progress_callback(done_count, total)` 每完成一个回调;(e) 失败任务不阻塞其他任务,失败详情包含 task_id 与 error。
  2. Bash:跑 20 个 mock 任务(各 0.5s,其中 2 个会失败),断言:总耗时 ≈ ceil(20/4)*0.5 = 3.0s;success=18、failed=2;progress_callback 被调用 20 次且 done_count 单调递增至 20。
  3. Read 源码 + Bash 断言:含并发 4、含进度回调、含失败隔离。

### HR08 WorkFlow 与 WindowUse 跨角色协作

- **预期档位**: hard
- **考察维度**: WorkFlow 阶段委派 WindowUse / pending_data 经 AgentMessage 桥接
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr08_bridge.py`:WorkFlow→WindowUse 桥接模拟(纯函数,模拟消息总线):(a) 模拟 Goal「读取桌面配置文件并校验」,Phase1 `SubAgent`(列文件路径)→ Phase2 `WindowUse`(打开应用读取文本)→ Phase3 `SubAgent`(校验 JSON Schema)→ Phase4 `QualityGate`(五级门禁);(b) 模拟 `AgentMessage` 总线:每个 Phase 完成后发 `ProcessedResult{value}` 给下一 Phase;(c) Phase2 失败时回退(模拟 WindowAction 返回 NOT_FOUND,按 AdaptiveLoop 升级 Replan);(d) 输出整 WorkFlow 时间线 + 每阶段消息序列。
  2. Bash:跑正常流 + 注入 Phase2 失败,断言:正常流 4 阶段全成功且 Phase3 收到的 content 与 Phase2 读取一致;失败流触发 Replan 并最终 Phase2' 成功完成整 WorkFlow。
  3. Read 源码 + Bash 断言:含四阶段顺序、含 AgentMessage 桥接、含 Replan 回退。

### HR09 WorkFlow 模板驱动端到端重构工作流

- **预期档位**: hard
- **考察维度**: TemplateLibrary + GoalStore + Phase + Squad + QualityGate 端到端串联
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr09_e2e.py`:端到端 WorkFlow 模拟器(组合前 8 个模块):(a) 用户输入「重构 src/lib.rs 把函数 foo 全部加 error context」;(b) TemplateLibrary.select(Refactor, "refactor functions") 命中模板;(c) 渲染模板得 4 Phase(分析 AST → Squad 并行改造 → QualityGate 验证 → 总结报告);(d) Phase1 SubAgent 模拟 AST 扫描返回函数列表;(e) Phase2 Squad FanOut 并行改造 5 个函数;(f) Phase3 QualityGate 五级评估(Semantic 字段命名/Functional 模拟编译/Schema 不适用跳过/Performance 不适用跳过/Safety 无敏感词);(g) 终态:Goal Completed + 报告含改造清单。
  2. Bash:跑模拟器断言:Phase2 实际并发运行(各函数完成时间重叠);Phase3 Schema/Performance 跳过有明确日志(SKIPPED 标记);终态 Completed;报告含改造前后对比段。
  3. Read 源码 + Bash 断言:含模板选择、含 FanOut 并行、含 SKIPPED 标记、含报告输出。

### HR10 WorkFlow 异常熔断与回流 Yolo

- **预期档位**: hard
- **考察维度**: Abort 路径 / Decompose 失败后回流 / 错误报告完整性
- **工具链**: Write → Bash → Read
- **对话脚本**:
  1. Write `tmpPlan/agent-test/hr10_abort.py`:熔断与回流模拟器(纯函数):(a) 模拟 Goal 在 Phase3 一直 Decompose 失败(MAX_LOOP=8 内仍失败);(b) Abort 后生成回流 Yolo 的错误报告:`{goal_id, failed_phase, attempts, last_error, suggested_user_action}`;(c) 报告必须含 `user_facing_summary`(给用户看的一句话)与 `technical_detail`(给 Yolo 看的诊断);(d) GoalStore 持久化终态为 Failed 且不可迁移;(e) 同一 session 下用户重新发同类任务时,Yolo 看到的 recent_failures 包含此条,避免重复尝试同一路径。
  2. Bash:跑熔断场景断言:Phase3 attempts ≥ 3(Decompose 多次);回流报告两段齐全;GoalStore 终态 Failed 且再 transition 返回错误;recent_failures 列表新增一条。
  3. Read 源码 + Bash 断言:含 Abort 触发条件、含双段错误报告、含回流接口。