# 79 Agent 记忆与上下文工程

> 编号段 CA11–CA20 · 聚焦 LLM Agent 的记忆系统与上下文工程：分层记忆 / RAG 进阶 / Context Engineering / MemGPT / 长程一致性 / 记忆压缩 / 反思机制 / 多 Agent 共享记忆
>
> 与现有维度互补说明：
> - `19-AI工程与LLM应用实战`（S01–S10）聚焦 LLM API / Prompt 工程 / Agent 框架使用 / 模型微调 / **基础 RAG**；本文件聚焦**记忆架构层与上下文工程深度**（MemGPT 虚拟上下文分页 / Letta 状态机 / A-MEM 动态记忆演进 / RAG 混合检索 / 长程一致性保持）。
> - `22-分布式系统与微服务架构`（V01–V10）含分布式缓存但缺 Agent Memory 系统设计；本文件补齐「分层记忆 + 反思 + 共享 + 压缩 + 持久化」全栈。
> - `12-laew元任务与工程特性`（L01–L10）已含 `agent_memory` 表 + `session_memory` 表 + `<<<LAEW:SESSION_HISTORY>>>` 注入标记；本文件进一步深挖这些机制的工程纵深（何时压缩 / 何时反思 / 何时重写 / 何时遗忘）。
>
> **本文件独特主题**：LLM Agent 三层记忆架构（STM/LTM/Episodic）/ Context Engineering 七要素（写入/选择/压缩/隔离/检索/重写/反思）/ MemGPT 虚拟上下文分页（page-in/page-out）/ Letta 状态机 + 工具调用记忆管理 / A-MEM 动态记忆图谱 / RAG 进阶（HyDE / ReRank / GraphRAG / Self-RAG）/ 长程一致性（核心记忆 + 反思 + 角色锚定）/ 反思机制（ReAct / Reflexion / Self-Refine）/ 多 Agent 共享记忆（共享空间 vs 消息总线 vs 黑板）/ 记忆压缩（CoT 蒸馏 / 关键事实提取 / 时间窗口滑动）/ 记忆评估（LOCOMO / LongBench / MILE）/ laew `agent_memory` 表与 `session_memory` 表的工程化升级。

---

### CA11 Agent 三层记忆架构：STM / LTM / Episodic

- **预期档位**: medium
- **考察维度**: 记忆分层 + 生命周期 + 容量与淘汰策略
- **对话脚本**:
  1. 人类记忆有感觉记忆、工作记忆、长期记忆三层。LLM Agent 的记忆系统可以类比为：STM（Short-Term Memory，会话上下文窗口内的消息）+ LTM（Long-Term Memory，向量库 / 数据库 / 文件系统）+ Episodic（情景记忆，特定事件的完整轨迹）。请画出三层记忆的数据流图，标注何时写入、何时检索、何时淘汰。
  2. STM 的容量受 LLM 上下文窗口限制（如 Claude 200K、GPT-4 128K），超出必须压缩或淘汰。请对比三种 STM 溢出策略：滑动窗口（保留最近 N 条）、重要性采样（按 token 重要度保留）、摘要压缩（保留 N 条 + 早期摘要）的实现复杂度与信息损失。
  3. LTM 的存储选择：向量数据库（Milvus / Qdrant / Weaviate）、关系数据库（SQLite / Postgres）、键值存储（Redis）、文件系统（Markdown / JSONL）。请从「检索语义」「写入吞吐」「可解释性」「冷启动成本」四维度对比，并给出适合本地工具（laew 这种单文件部署）的方案。
  4. Episodic 记忆的特殊性在于它记录「发生了什么」而非「知道什么」。请设计一个 Episodic 记忆结构：包含事件 ID、时间戳、参与者、动作、结果、情感标签、上下文环境；并解释为什么 Agent 在做长程任务（如连续多天的代码重构）时必须保留 Episodic 记忆才能保证一致性。

---

### CA12 Context Engineering 七要素：从 Prompt 到上下文系统

- **预期档位**: medium
- **考察维度**: Context Engineering 体系化 + 与 Prompt Engineering 的关系
- **对话脚本**:
  1. Andrej Karpathy 在 2025 年提出 Context Engineering 是「比 Prompt Engineering 更上层的学科」。请列出 Context Engineering 的七要素：写入（Write）、选择（Select）、压缩（Compress）、隔离（Isolate）、检索（Retrieve）、重写（Rewrite）、反思（Reflect），逐一解释每个要素要解决的问题。
  2. 写入（Write）要素：Agent 主动把信息写到上下文之外（如 scratchpad / notebook）。请设计一个 scratchpad 数据结构：何时写入（决策点 / 异常 / 灵感）、写入什么（事实 / 假设 / 计划）、如何索引（按时间 / 按主题 / 按任务 ID），并解释为什么这是处理「上下文腐烂」的第一步。
  3. 隔离（Isolate）要素：把不同关注点的信息分到不同上下文窗口。请对比 Multi-Turn 单上下文 vs Multi-Agent 多上下文的优劣：当多个任务并行时哪种方案更好；当需要跨任务共享信息时哪种方案更好。
  4. 反思（Reflect）要素：让 Agent 周期性回顾自己的行为、纠正错误、更新信念。请设计一个反思循环：触发条件（任务结束 / 错误发生 / 用户反馈）、反思 prompt 的设计要点（不要泛泛「做得怎么样」而要问「具体哪一步可以改进」）、反思结果的存储与复用。

---

### CA13 MemGPT 虚拟上下文分页：page-in / page-out

- **预期档位**: hard
- **考察维度**: 操作系统类比 + 虚拟内存 + 分页算法
- **对话脚本**:
  1. MemGPT（2023 UC Berkeley 论文）借鉴操作系统虚拟内存机制，把 LLM 上下文窗口类比为「物理内存」、外部存储类比为「磁盘」，通过 page-in / page-out 让 Agent 拥有「无限」的记忆。请画出 MemGPT 的层次结构图：核心内存（在上下文内，类似寄存器）、召回存储（向量库，类似磁盘）、档案存储（KV 数据库，类似文件系统）。
  2. MemGPT 的核心创新是「LLM 自己管理内存」。请设计 Self-Edit Memory 机制：LLM 用函数调用 `memory_append(text)` / `memory_replace(old, new)` / `memory_delete(key)` 来主动管理核心内存，并解释为什么「让 LLM 自己决定何时换页」比硬编码规则（如「每 1000 token 压缩一次」）效果好。
  3. Letta（MemGPT 商业化公司）的状态机设计：Agent 维护一组状态（`user`、`assistant`、`tool_call`、`tool_response`、`memory_update`），状态转移由 LLM 决策 + 规则约束。请画出 Letta 的状态转移图，并对比传统 ReAct 循环（thought → action → observation）的优劣。
  4. 在 laew 的架构中应用 MemGPT 思想：把当前的 Session 主上下文作为「核心内存」、把 `agent_memory` 表 + `session_memory` 表作为「召回存储」、把文件系统 + Git 历史作为「档案存储」。请设计 `memory_append` / `memory_replace` 的具体实现：BashTool 调用前自动检查上下文剩余空间、低于 20% 时触发 page-out 到 SQLite `archived_memory` 表。

---

### CA14 RAG 进阶：HyDE / ReRank / GraphRAG / Self-RAG

- **预期档位**: hard
- **考察维度**: RAG 演进路线 + 检索增强多策略融合
- **对话脚本**:
  1. 朴素 RAG（query → embedding → top-k → LLM）的问题是「query 和文档语义不对齐」。HyDE（Hypothetical Document Embeddings）的解法：先用 LLM 生成假想答案文档，再对假想答案做 embedding 检索。请解释为什么这种「query 升维到文档维度」的检索效果通常优于直接 query 检索，并给出一个具体的代码示例。
  2. ReRank 两阶段检索：先用向量检索 top-100（召回率高、精度低），再用 Cross-Encoder（如 BGE-reranker）对 top-100 重排取 top-5（精度高）。请解释 Bi-Encoder（向量检索用）与 Cross-Encoder（重排用）的本质区别，并说明为什么重排模型必须看到 query+doc 配对才能精确打分。
  3. GraphRAG（Microsoft 2024）：用 LLM 从文档中抽取实体关系建知识图谱，检索时先在图上做社区检测，再把社区摘要喂给 LLM。请对比 GraphRAG 与朴素 RAG 在「全局性问题（如本文档主要讲什么）」与「局部性问题（第三章节第二段讲了什么）」上的优劣差异。
  4. Self-RAG：让 LLM 自己决定「要不要检索」「检索什么」「检索结果是否相关」「是否需要重写答案」。请设计 Self-RAG 的反思 token 体系：`[Retrieve]` `[IsRel]` `[IsSup]` `[IsUse]`，并解释为什么 Self-RAG 比固定流程 RAG 更适合复杂推理任务。

---

### CA15 长程一致性：核心记忆 + 反思 + 角色锚定

- **预期档位**: hard
- **考察维度**: 长程一致性挑战 + 三大对抗技术
- **对话脚本**:
  1. LLM Agent 在长程任务（持续多天 / 数百轮对话）中面临「一致性丢失」：角色漂移（开始是 Python 专家后来变 JavaScript 风格）、承诺遗忘（说要做 A 后来忘了）、价值观漂移（开始严格后来宽松）。请分析这三种漂移的根本原因（上下文窗口有限 + 注意力衰减）。
  2. 核心记忆（Core Memory）技术：把「角色设定 + 关键承诺 + 价值观」等核心信息固化到 system prompt 之外的高优先级块，每次请求前强制注入。请设计一个核心记忆结构：`{role: "...", commitments: [...], values: [...], style: "..."}`，并说明为什么它必须放在最前而非混合到系统提示词中。
  3. 反思机制（Reflection）对抗一致性丢失：每完成一个任务后让 Agent 写一段「本任务总结 + 关键经验 + 对核心记忆的更新建议」，人工/自动审核后写入核心记忆。请设计反思 prompt 的关键要素：必须问「我有没有偏离角色」「我有没有忘记承诺」「我有没有违反价值观」，避免空洞反思。
  4. 角色锚定（Role Anchoring）通过 Few-shot Examples 强化：每次 prompt 注入 3-5 个「过去成功完成任务的对话样本」，让模型从样本中学习角色风格而非从指令学习。请分析为什么「示例比指令更稳定」（示例激活 implicit memory，指令容易被遗忘），并讨论在 laew 的 SessionContext 摘要中如何挑选高质量样本。

---

### CA16 反思机制深度：ReAct / Reflexion / Self-Refine / Constitutional AI

- **预期档位**: hard
- **考察维度**: 反思范式演进 + 自我修正机制
- **对话脚本**:
  1. ReAct（Reasoning + Acting）是 2022 年的开山之作：把思维链（Thought）+ 行动（Action）+ 观察（Observation）交替编排。请画出 ReAct 的循环结构，分析为什么「显式 reasoning」比「隐式 thinking」在多步骤任务中更可控、更可调试。
  2. Reflexion（2023）让 Agent 反思失败并把反思文本写入记忆：失败 → 「我为什么错了」反思 → 写入 episodic memory → 下次遇到类似任务时检索反思避免重蹈覆辙。请设计一个 Reflexion 失败分类体系（参数错误 / 工具误用 / 逻辑谬误 / 知识缺失 / 上下文误读），并解释每类失败对应的反思 prompt 模板。
  3. Self-Refine（2023）：让 LLM 对自己输出做多轮 critique → refine → 直到满意。请设计 Self-Refine 的停止条件：固定轮数（如 3 轮）、评分收敛（评分变化 < 阈值）、置信度阈值（LLM 自报置信度 > 0.9）。并讨论 Self-Refine 在代码生成 vs 创意写作 vs 推理任务上的效果差异。
  4. Constitutional AI（Anthropic 2022）：用一组「宪法原则」（如「不要有害」「不要歧视」「要诚实」）让 LLM 自我批评并修正输出。请对比 RLAHF（人类反馈强化学习）与 Constitutional AI（AI 反馈自我修正）的优劣：前者更准确但贵，后者便宜但需要谨慎设计原则避免「回音室效应」。

---

### CA17 多 Agent 共享记忆：共享空间 / 消息总线 / 黑板

- **预期档位**: hard
- **考察维度**: 多 Agent 记忆架构 + 一致性挑战
- **对话脚本**:
  1. 多 Agent 系统（Yolo + Plan + Main-Work + SubAgent + Quality-Check + SessionContext）中，记忆共享有三种范式：共享空间（所有 Agent 读写同一份记忆）、消息总线（Agent 通过消息传递共享信息）、黑板系统（中央黑板 + 各 Agent 监听变化）。请用对比表说明三者在「一致性」「可扩展性」「通信开销」上的差异。
  2. 共享空间方案：所有 Agent 共享一个向量数据库 + KV 存储。设计「记忆所有权」机制：每个记忆条目记录「谁写的」「谁能读」「谁能改」，用 SQLite 的 RLS（Row Level Security）或应用层 ACL 强制执行。讨论这种方案在 laew 多 Agent 架构中的可行性。
  3. 消息总线方案：Agent 间通过 ZeroMQ / Redis Streams / Kafka 传递记忆。设计消息 schema：`{sender, receiver, message_type, payload, timestamp, ack_required}`。讨论消息总线相比直接函数调用的优劣（解耦 + 持久化 + 可观测，但增加复杂度）。
  4. 黑板系统方案：中央黑板存储「当前任务状态 + 已知事实 + 候选方案」，各 Agent 监听变化并贡献信息。设计黑板的数据结构与变更通知机制（Pub/Sub）。为什么 AlphaGo 与手术机器人这类需要多个专家协同的系统倾向于用黑板架构。

---

### CA18 记忆压缩：摘要蒸馏 / 关键事实提取 / 时间窗口滑动

- **预期档位**: medium
- **考察维度**: 压缩策略 + 信息保留度评估
- **对话脚本**:
  1. LTM 长期记忆如果不压缩，会无限增长导致检索变慢、成本变高。请设计一个三级压缩流水线：L1（每天深夜批量摘要昨日所有对话，保留时间戳 + 关键决策）、L2（每周合并 7 个 L1 摘要 + 提取跨日主题）、L3（每月只保留里程碑事件 + 关键人物偏好）。
  2. 摘要蒸馏（Distill）：用 LLM 对长对话生成结构化摘要：参与者、目标、关键决策、待办、未解决问题、情感变化。请设计一个摘要 prompt 模板，要求输出 JSON 格式便于解析，并讨论如何评估摘要质量（BLEU 不适用 → 用 LLM-as-a-Judge 打分）。
  3. 关键事实提取：从对话中抽取「人名 + 偏好 + 承诺 + 时间节点 + 项目代号」等结构化事实，存到 LTM 的「事实库」。请设计提取 prompt 与事实 schema（类似 `{entity, type, value, source_msg_id, confidence}`），并解释为什么结构化事实比自然语言摘要更适合「检索 + 应用」。
  4. 时间窗口滑动：当 LTM 超过容量阈值（如 10 万条记忆），用滑动窗口淘汰最旧 20% 但保留「被频繁检索的记忆」（用 LRU 思想）。请设计淘汰评分函数：`score = recency × 0.4 + retrieval_frequency × 0.4 + importance × 0.2`，讨论权重的合理性。

---

### CA19 记忆评估基准：LOCOMO / LongBench / MILE

- **预期档位**: medium
- **考察维度**: 记忆系统评估方法学 + 基准数据集
- **对话脚本**:
  1. 评估 LLM Agent 记忆系统需要专门的基准。LOCOMO（2024）是首个长对话记忆基准：包含 75 个对话、约 9K 轮次、6 大类问题（事实回忆 / 时间推理 / 多跳推理 / 偏好保持 / 对话理解 / 知识更新）。请用 LOCOMO 评估一个朴素 RAG 记忆系统与 MemGPT 风格记忆系统，分析两者在每类问题上的差异。
  2. LongBench（2023）评估长上下文（10K-100K token）任务：单文档 QA / 多文档 QA / 摘要 / Few-shot / 合成任务 / 代码补全。请分析 LongBench 与 LOCOMO 的差异（前者测「上下文窗口能力」后者测「记忆系统能力」），以及为什么 Agent 记忆系统必须同时通过两者才算合格。
  3. MILE（2024 Microsoft）专门评估 Multi-modal Long-context Memory：包含图像 + 文本混合的长对话。请分析多模态记忆的特殊挑战（图像如何 embedding？OCR 后存文本还是原图？视频怎么办？）并设计一个简易多模态记忆方案。
  4. 真实场景评估：用 laew 自身的使用日志做内部基准。设计一个评估脚本：随机抽取过去 100 个 Session，提取最近一轮的问题，测试 Agent 是否能回忆起相关历史上下文。计算召回率（能找到相关历史）/ 精确率（找到的都是相关的）/ 用户满意度（人工打分），作为 laew 记忆系统的持续监控指标。

---

### CA20 laew 记忆系统升级路线：session_memory + agent_memory + archived_memory

- **预期档位**: hard
- **考察维度**: 工程化升级 + laew 现状整合
- **对话脚本**:
  1. 审计 laew 现有记忆架构：`Session.context`（内存态 STM）+ `agent_memory` 表（Agent 级 LTM）+ `session_memory` 表（Session 级摘要）+ `<<<LAEW:SESSION_HISTORY>>>` 注入（历史摘要注入 Yolo）。请画出当前架构的数据流图，标注每层的容量、生命周期、检索方式，并指出当前的 3 个关键缺陷。
  2. 设计 laew MemGPT 风格升级方案：把 STM 拆为「核心 8K（system prompt + 当前任务）」+ 「上下文 192K（消息流 + 检索结果）」两层；把 LTM 拆为「高频 LTM（最近 7 天的 agent_memory 行，KV 缓存）」+「低频 LTM（SQLite 表，按需加载）」；引入 `archived_memory` 表（page-out 出去的旧消息）。
  3. 设计 laew 反思机制：每完成一个 Session 主任务后 SessionContext Agent 不只写摘要，还写「本任务的关键经验 + 失败案例 + 改进建议」，存到 `agent_memory.reflections` 列；下次同类任务时由 Yolo 检索并注入。
  4. 设计 laew 长程一致性方案：在 `agent_memory` 表新增 `core_memory` 列（每个 Agent 一份），存「角色锚定 + 关键承诺 + 用户偏好」，每次 Agent 启动时强制注入 system prompt 之前。讨论为什么 laew 必须有「角色漂移防护」机制——多轮任务中如果 SubAgent 漂移到「帮我重写所有 Rust 代码」就会发生灾难性后果。

---

## 与 laew 工程的具体对接点

1. **CA11 → `agent_memory` 表设计**：当前只有 `id/agent_name/memory_type/content/created_at` 字段，缺 `embedding/importance/retrieval_count/last_retrieved_at` 等字段，建议加 `importance_score REAL DEFAULT 0.5` 与 `embedding BLOB` 列。
2. **CA12 → `<<<LAEW:PROJECT_CONTEXT>>>` / `<<<LAEW:SESSION_HISTORY>>>` 注入标记**：这些是 Context Engineering 的「选择（Select）」与「检索（Retrieve）」要素的具体实现，未来可加 `<<<LAEW:CORE_MEMORY>>>` 与 `<<<LAEW:REFLECTION>>>` 标记。
3. **CA13 → Compact Agent 第 8 角色**：已实现的 80% 阈值 + 三档压缩（Light/Medium/Aggressive）对应 MemGPT 的「重要性采样 + 摘要压缩」思想，未来可参考 MemGPT 的 page-in/page-out 双向机制。
4. **CA17 → MultiAgentOrchestrator 编排**：Yolo → Plan → Main-Work → SubAgent-Work → Quality-Check → SessionContext 的数据流本质是「消息总线 + 中央共享上下文（Session.context）」，未来可分离显式记忆共享（`agent_memory` 表）与隐式消息传递。
5. **CA20 → Session 生命周期管理**：当前 Session ID 与 device_id 已实现，但缺「用户身份合并」（同一开发者多设备多 Session 的合并），参考 BJ07 客户去重思想实现。
