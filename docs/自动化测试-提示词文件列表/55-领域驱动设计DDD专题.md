# 55 领域驱动设计 DDD 专题

> 编号段 BD01–BD10 · 聚焦 DDD 全栈实践：战略设计 / 战术模式 / 事件风暴 / 上下文映射 / 聚合根设计 / 限界上下文集成
> 与现有 50 维「软件架构模式与设计系统」互补：50 仅 AY02 一节概述 DDD；55 展开为 10 个独立纵深专题
> 与现有 22 维「分布式系统与微服务架构」互补：22 偏分布式基础设施；55 偏领域建模与战略设计落地

---

### BD01 通用语言（Ubiquitous Language）建立与维护
- **预期档位**: medium
- **考察维度**: 跨角色对话 + 术语治理
- **对话脚本**:
  1. 通用语言是什么？为什么要让业务方、产品、开发、测试、运维用同一套词汇？「订单」在客服语境（业务订单）、财务语境（结算单）、仓库语境（出库单）的差异。
  2. 建立通用语言的三步法：术语抽取（业务文档 + 用户访谈）→ 术语对齐（Workshop 共识）→ 术语落地（代码命名 + 文档 + 沟通用语）。
  3. 通用语言的「漂移」问题：开发为省事用技术词（Entity/Service/Manager）替代业务词，半年后业务方听不懂。给出检测与纠正机制。
  4. 给 laew 的多 Agent 体系做通用语言梳理：把 Yolo / Plan / SubAgent / Quality-Check / SessionContext / Debug / Compact 七角色翻译为业务可读的角色名。

### BD02 限界上下文（Bounded Context）划分实战
- **预期档位**: hard
- **考察维度**: BC 边界识别 + 拆分方法
- **对话脚本**:
  1. 限界上下文的四大识别信号：业务能力（Capability）边界、团队结构（Conway 定律）、数据所有权（Master Data）、变更频率（独立演进节奏）。
  2. BC 拆分的反模式：按技术层拆（Controller/Service/DAO 跨 BC 共享）、按数据库表拆（订单表横跨多个 BC）、过早拆分（团队规模不够时拆 10 个 BC）。
  3. 真实案例：从单体电商拆 BC 的过程，订单域、库存域、支付域、营销域、用户域的边界识别与典型争议（订单里的「优惠券抵扣」属于订单还是营销）。
  4. 用「事件风暴（Event Storming）」工作坊为 laew 拆 BC：识别领域事件（用户输入任务 / 任务分类 / 子任务委派 / 上下文压缩）→ 划定聚合 → 落定 BC。

### BD03 上下文映射（Context Map）与集成模式
- **预期档位**: hard
- **考察维度**: Context Map 8 种模式 + 集成 DDD
- **对话脚本**:
  1. Context Map 八种集成模式：Partnership / Shared Kernel / Customer-Supplier / Conformist / Anti-Corruption Layer / Open-Host Service / Published Language / Separate Ways。
  2. 每种模式的核心权衡：团队耦合度（低 → 高）vs 演进自由度（高 → 低）vs 翻译成本（无 → 高）。给一个「中台-业务」架构选 Shared Kernel vs ACL 的真实取舍。
  3. ACL（Anti-Corruption Layer）的工程实现：Translator 模式 + Adapter 模式 + 防腐层的位置（上游还是下游？），给一段 ACL 翻译外部订单模型的伪代码。
  4. 给 laew 设计 Context Map：Yolo 入口层与 Plan 规划层的 ACL 翻译（任务分类标签的语义对齐）、SubAgent ↔ Quality-Check 的 Customer-Supplier 关系。

### BD04 聚合根（Aggregate Root）设计与一致性边界
- **预期档位**: hard
- **考察维度**: 聚合设计 + 不变量保护
- **对话脚本**:
  1. 聚合根的本质：一致性边界 + 事务边界 + 不变量（Invariant）守护者。判断一个实体是否该是聚合根的三个问题：能否独立事务？是否被外部直接引用？修改是否需协调其他对象？
  2. 聚合根设计的常见错误：过大（God Aggregate，把所有相关实体塞进一个根）/ 过小（每个实体一个根，失去聚合意义）/ 跨聚合事务（破坏最终一致性原则）。
  3. 聚合根的引用规则：聚合根之间通过 ID 引用（非对象引用）、聚合内部通过对象引用、跨聚合用领域事件同步状态。
  4. 给 laew 设计聚合根：把「任务执行」建模为 `TaskExecution` 聚合根，内含 `WorkflowStep` 列表与 `SessionContext`，外部只能通过根的领域方法（如 `executeNext()`）变更内部状态。

### BD05 值对象（Value Object）与不可变性
- **预期档位**: medium
- **考察维度**: VO 设计 + 不可变实践
- **对话脚本**:
  1. 值对象 vs 实体的本质差异：无身份标识（Identity）vs 有身份；不可变 vs 可变；可替换（基于属性相等）vs 不可替换（基于 ID 追踪）。
  2. 经典值对象案例：Money（金额 + 币种）、Address（街道 + 城市 + 邮编）、DateRange（开始 + 结束）、Email、PhoneNumber。给一段 Rust 实现的 Money 不可变示例。
  3. 值对象的工程价值：消除「贫血模型」（Primitive Obsession）、提升表达力（`Email` 比 `String` 更安全）、自动验证（构造时校验）。
  4. 给 laew 设计一批值对象：`ContextUsage { used: u32, max: u32 }`、`TokenCost { input: u32, output: u32, cache_read: u32 }`、`TimeRange { start: DateTime, end: DateTime }`，列出它们的使用场景。

### BD06 领域服务（Domain Service）与应用服务分层
- **预期档位**: medium
- **考察维度**: 领域服务 vs 应用服务职责边界
- **对话脚本**:
  1. 领域服务的判定三问：操作是否跨多个聚合根？是否承载领域逻辑而非基础设施？是否属于业务概念而非技术概念？三条都满足 → 领域服务。
  2. 应用服务 vs 领域服务 vs 基础设施服务的三层分工：应用服务（用例编排 + 事务边界 + DTO 转换）→ 领域服务（领域逻辑 + 不变量保护）→ 基础设施服务（DB/HTTP/MQ）。
  3. 「贫血模型」反模式：所有逻辑塞进 Service、实体只有 getter/setter。给出从贫血模型到充血模型的迁移路径。
  4. 给 laew 分层：`TaskClassifier`（应用服务，编排分类流程）→ `TaskDifficultyEvaluator`（领域服务，3 档分类业务逻辑）→ `LlmClient`（基础设施，HTTP 调用）。

### BD07 领域事件（Domain Event）与解耦
- **预期档位**: hard
- **考察维度**: 领域事件建模 + 可靠发布
- **对话脚本**:
  1. 领域事件的本质：过去时态命名（OrderPlaced / UserRegistered / TaskCompleted）、携带必要数据（ID + 业务字段）、携带元数据（时间戳、actor、tenant）。
  2. 领域事件的工程实现：内存事件总线（同进程）、Outbox 模式（可靠发布到 MQ）、Eventual Consistency（最终一致性）、CDC（变更数据捕获）四种实现路径对比。
  3. 事件命名与版本演进：`OrderPlaced` → `OrderPlacedV2` 的迁移策略，事件 schema 的 upcasting 与死信处理。
  4. 给 laew 设计领域事件流：`TaskClassified(level: TaskLevel)` → `WorkflowPlanned(workflows: Vec<Workflow>)` → `SubTaskCompleted(task_id, output)` → `QualityCheckPassed`，画出事件订阅关系图。

### BD08 仓储（Repository）模式与持久化
- **预期档位**: medium
- **考察维度**: 仓储抽象 + 持久化无关
- **对话脚本**:
  1. 仓储模式的本质：聚合根级别的「内存集合」抽象，对上层隐藏持久化细节，让领域代码不依赖 ORM/数据库。
  2. 仓储与 DAO 的区别：DAO 是表级别的、仓储是聚合根级别的；仓储返回领域对象、DAO 返回数据结构。
  3. 仓储设计原则：接口定义在领域层、实现放在基础设施层、聚合根只暴露仓储接口、避免「通用仓储」反模式（CRUD 通用方法）。
  4. 给 laew 设计仓储：`TaskExecutionRepository` 接口（find_by_id / save / find_incomplete）定义在 domain 层，SqliteTaskExecutionRepository 实现放在 infra 层，给出 trait 定义与实现分离的代码。

### BD09 事件风暴（Event Storming）工作坊
- **预期档位**: medium
- **考察维度**: 工作坊流程 + 协作引导
- **对话脚本**:
  1. 事件风暴三色便利贴：橙色（领域事件）/ 蓝色（命令）/ 黄色（外部系统）/ 绿色（读模型）/ 粉色（热点问题），贴出来的就是一张「业务流程大图」。
  2. 工作坊的标准流程：邀请（业务 + 开发 + 测试 + UX，10-15 人）→ 事件识别 → 命令识别 → 聚合划分 → BC 边界 → 上下文映射 → 输出待办。
  3. 远程协作的事件风暴工具：Miro / Mural / FigJam / Whimsical，对比实时性、白板体验、便利贴拖拽、模板支持。
  4. 设计一场为 laew 团队做的事件风暴：3 小时工作坊议程、参与者角色分工、产出物清单（领域事件列表 + BC 草图 + Context Map + 待澄清问题）。

### BD10 DDD 落地陷阱与团队转型
- **预期档位**: medium
- **考察维度**: 落地方法论 + 团队演进
- **对话脚本**:
  1. DDD 落地的七大陷阱：教条化（把模式当教条而非工具）、过度设计（CRUD 强行上聚合根）、名词驱动（只翻译术语不识别边界）、文档驱动（架构图代替对话）、单兵作战（缺跨角色协同）、回避妥协（不愿写 ACL）。
  2. DDD 转型的三阶段：探索阶段（单 BC 试点 + 事件风暴）→ 推广阶段（识别核心域 + 拆分 BC）→ 收敛阶段（持续集成 + 上下文映射治理），每阶段 3-6 个月。
  3. 「遗留系统」的 DDD 化：绞杀者模式（Strangler Fig）逐步替换、Anti-Corruption Layer 隔离、技术债分阶段偿还，给出 18 个月迁移路线图。
  4. laew 作为新项目从 Day 1 引入 DDD 的最佳实践：先建通用语言词典、再用事件风暴识别 4-5 个聚合根、采用战术模式（VO/Entity/Domain Service）落地代码，最后按 BC 拆分模块。
