# 79 Agent 记忆与上下文工程

> 编号段 CA11–CA20 · 聚焦 LLM Agent 的记忆系统与上下文工程：分层记忆 / RAG 进阶 / Context Engineering / MemGPT / 长程一致性 / 记忆压缩 / 反思机制 / 多 Agent 共享记忆
>
> **本批运行环境约束**：无 LLM API Key / 无外部 embedding 服务 / 无向量数据库，**全部以纯 python3 + numpy + sqlite3 等价实现** 跑通。MemGPT 虚拟上下文分页用 LRU + 阈值触发模拟；RAG HyDE / ReRank 用 numpy 向量检索；LOCOMO 评估用脚本生成对话集 + 正确率断言；多 Agent 共享记忆用 sqlite3 表 + ACL 模拟。产物落 `tmpPlan/agent-test/` 沙盒。

---

### CA11 Agent 三层记忆架构：STM / LTM / Episodic

- **测试状态**: 🔄 待重测（2026-09-11 脚本重写；旧版曾通过，记录见 tmpPlan/2026-09-09_08-S01-S10-AI工程LLM应用测试脚本重构方案.md）
- **预期档位**: medium
- **考察维度**: 记忆分层 + 生命周期 + 淘汰策略
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca11/` 下用 Write 写 `three_layer_arch.md`：markdown 画 STM / LTM / Episodic 三层记忆数据流图，标注何时写入、何时检索、何时淘汰；Bash `grep -c '^|' three_layer_arch.md` ≥ 5 + `grep -F 'STM' three_layer_arch.md` 命中。
  2. 写 `stm_overflow.py`：实现 STM 三种溢出策略——滑动窗口（保留最近 10 条）/ 重要性采样（按 token 长度加权）/ 摘要压缩（前 5 条 + 后 5 条）；输入 20 条消息跑 3 种策略，断言每种都保留 10 条且摘要版含 "summary" 关键字；Bash 跑后 `grep -F "STM_OK" stm.out` 命中，写入 `stm.out`。
  3. 写 `ltm_storage.md`：markdown 表 4 行（向量数据库 / 关系数据库 / 键值存储 / 文件系统），列：检索语义、写入吞吐、可解释性、冷启动成本；Bash `grep -c '^|' ltm_storage.md` ≥ 6 + `grep -F '向量数据库' ltm_storage.md` 命中。
  4. 写 `episodic_struct.py`：实现 Episodic 记忆结构——`Event(id, ts, actor, action, result, sentiment, env)`；Bash 跑后断言事件插入与查询正确，写入 `ep.out`。

### CA12 Context Engineering 七要素

- **预期档位**: medium
- **考察维度**: 七要素体系化 + 与 Prompt Engineering 关系
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca12/` 下用 Write 写 `seven_elements.md`：markdown 列七要素（Write / Select / Compress / Isolate / Retrieve / Rewrite / Reflect），每个 ≥ 1 句解释；Bash `grep -c '^[0-9]\.' seven_elements.md` ≥ 7 + `wc -l seven_elements.md` ≥ 15。
  2. 写 `scratchpad.py`：实现 scratchpad 数据结构——`Scratchpad(ts, kind, content)` 支持按时间 / 主题 / 任务 ID 索引；Bash 跑后断言 3 种索引查询正确，写入 `sp.out`。
  3. 写 `isolate_compare.md`：markdown 对比 Multi-Turn 单上下文 vs Multi-Agent 多上下文——并行任务谁更好 / 跨任务共享谁更好；Bash `grep -c '^|' isolate_compare.md` ≥ 4 + `grep -F 'Multi-Agent' isolate_compare.md` 命中。
  4. 写 `reflect_loop.py`：实现反思循环——触发条件（任务结束 / 错误发生 / 用户反馈）+ 反思 prompt "具体哪一步可以改进" + 存储到反思表；Bash 跑后断言反思记录生成正确，写入 `ref.out`。

### CA13 MemGPT 虚拟上下文分页（LRU + 阈值模拟）

- **预期档位**: hard
- **考察维度**: OS 类比 + Self-Edit Memory + page-in/page-out
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca13/` 下用 Write 写 `memgpt_arch.md`：markdown 画 MemGPT 层次结构图——核心内存（in-context）/ 召回存储（向量库）/ 档案存储（KV DB）；Bash `grep -c '^|' memgpt_arch.md` ≥ 5 + `grep -F 'page-in' memgpt_arch.md` 命中。
  2. 写 `paging_sim.py`：实现虚拟上下文分页——`CoreMemory(capacity=8)` 满了触发 page-out 到 `ArchivalStore`（sqlite3 表），最近访问 page-in；Bash 跑 `python paging_sim.py` 后断言 page-in/page-out 事件数 ≥ 1，写入 `pg.out`。
  3. 写 `self_edit.py`：实现 `memory_append/replace/delete`——LLM 模拟（基于关键词触发）；Bash 跑后断言三种操作正确执行，写入 `se.out`。
  4. 写 `letta_fsm.py`：实现 Letta 风格状态机——状态 `user/assistant/tool_call/tool_response/memory_update`；Bash 跑后断言状态转移合法序列，写入 `letta.out`。

### CA14 RAG 进阶：HyDE / ReRank / GraphRAG / Self-RAG（numpy 等价）

- **预期档位**: hard
- **考察维度**: RAG 演进路线 + 检索增强多策略
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca14/` 下用 Write 写 `hyde_sim.py`：实现 HyDE 检索——给定查询 "如何训练神经网络"，先用 LLM 模拟器（基于关键词 "训练" 生成 5 句"假想答案"）生成假想答案，再做 embedding 检索；Bash 跑后断言 HyDE top-1 命中率 ≥ 直接查询（启发式判断），写入 `hyde.out`。
  2. 写 `rerank_sim.py`：实现两阶段检索——Bi-Encoder 向量召回 top-10 + Cross-Encoder 风格重排（用余弦相似度差值 + 长度归一化模拟）取 top-3；Bash 跑后断言重排后相关性分数 > 原始，写入 `rerank.out`。
  3. 写 `graphrag_sim.py`：模拟 GraphRAG——从 5 段文本中提取实体-关系三元组（启发式：用句号分割+提取含 "是"/"有" 的句子的主谓宾）建图，再做社区检测（简化版：连通分量）；Bash 跑后断言提取三元组数 ≥ 5，写入 `gr.out`。
  4. 写 `self_rag.py`：实现 Self-RAG 反思 token——`[Retrieve]/[IsRel]/[IsSup]/[IsUse]` 决策门控；Bash 跑后断言无相关结果时不返回答案（`[IsUse]=no`），写入 `sr.out`。

### CA15 长程一致性：核心记忆 + 反思 + 角色锚定

- **预期档位**: hard
- **考察维度**: 长程一致性 + 三大对抗技术
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca15/` 下用 Write 写 `drift_analysis.md`：markdown 分析三种漂移（角色漂移 / 承诺遗忘 / 价值观漂移）的根本原因——上下文窗口有限 + 注意力衰减；Bash `grep -c '^### ' drift_analysis.md` ≥ 3 + `wc -l drift_analysis.md` ≥ 15。
  2. 写 `core_memory.py`：实现核心记忆结构——`{role, commitments, values, style}` 注入到 system prompt 之前；Bash 跑后断言 core_memory 字段齐全，写入 `cm.out`。
  3. 写 `reflection_prompt.md`：markdown 设计反思 prompt 模板——必须问"我有没有偏离角色/忘记承诺/违反价值观"，避免空洞；Bash `grep -c '^- ' reflection_prompt.md` ≥ 3 + `grep -F '角色' reflection_prompt.md` 命中。
  4. 写 `role_anchoring.py`：模拟 few-shot 角色锚定——每次 prompt 注入 3 个成功对话样本（从历史 SQLite 取）；Bash 跑后断言样本注入位置正确（system prompt 之后，user 之前），写入 `ra.out`。

### CA16 反思机制深度：ReAct / Reflexion / Self-Refine / Constitutional AI

- **预期档位**: hard
- **考察维度**: 反思范式 + 自我修正
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca16/` 下用 Write 写 `react_loop.py`：实现最小 ReAct 循环——Thought → Action → Observation 交替；mock LLM 根据 thought 关键词返回 action；Bash 跑后断言循环在 ≤ 5 步内终止，写入 `react.out`。
  2. 写 `reflexion.py`：实现 Reflexion——失败分类（参数错误 / 工具误用 / 逻辑谬误 / 知识缺失）+ 反思 prompt 模板；Bash 跑后断言反思文本写入 episodic memory，写入 `rf.out`。
  3. 写 `self_refine.py`：实现 Self-Refine 多轮 critique→refine——停止条件：固定 3 轮 或 评分变化 < 0.05；Bash 跑后断言 refine 后分数 ≥ refine 前，写入 `srf.out`。
  4. 写 `constitutional_ai.md`：markdown 对比 RLAHF vs Constitutional AI——前者准确但贵，后者便宜但有回音室风险；Bash `grep -c '^|' constitutional_ai.md` ≥ 5 + `grep -F 'Constitutional' constitutional_ai.md` 命中。

### CA17 多 Agent 共享记忆（sqlite + ACL 模拟）

- **预期档位**: hard
- **考察维度**: 多 Agent 记忆架构 + 一致性
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca17/` 下用 Write 写 `multi_arch_compare.md`：markdown 表 3 行（共享空间 / 消息总线 / 黑板系统），列：一致性、可扩展性、通信开销；Bash `grep -c '^|' multi_arch_compare.md` ≥ 5 + `grep -F '黑板' multi_arch_compare.md` 命中。
  2. 写 `shared_space.py`：实现共享空间 sqlite 表 `memories(id, owner, readers, writers, content)` + ACL 过滤；Bash 跑后断言无权限 Agent 读取时抛 PermissionError，写入 `ss.out`。
  3. 写 `message_bus.py`：实现消息总线模拟——`mp.Queue` + `producer/consumer` + schema `{sender, receiver, type, payload, ts}`；Bash 跑后断言消息按序到达正确 receiver，写入 `mb.out`。
  4. 写 `blackboard.py`：实现黑板——中心 dict + 各 Agent 监听 key 变化（用回调注册）；Bash 跑后断言任一 Agent 写入后其他监听者收到通知，写入 `bb.out`。

### CA18 记忆压缩：摘要蒸馏 / 关键事实提取 / 时间窗口滑动

- **预期档位**: medium
- **考察维度**: 压缩策略 + 信息保留度
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca18/` 下用 Write 写 `three_level_compress.md`：markdown 描述 L1（每日摘要）/ L2（每周合并）/ L3（每月里程碑）三级压缩；Bash `grep -c '^[0-9]\.' three_level_compress.md` ≥ 3 + `grep -F 'L1' three_level_compress.md` 命中。
  2. 写 `summary_distill.py`：实现摘要蒸馏——给定 20 条对话生成结构化摘要 `{participants, goals, decisions, todos, open_questions, sentiment}`；Bash 跑后断言输出 dict 6 个字段齐全，写入 `sd.out`。
  3. 写 `fact_extract.py`：实现关键事实提取——从对话中抽取 `{entity, type, value, source_msg_id, confidence}`；Bash 跑后断言事实数 ≥ 3 且 confidence ∈ [0, 1]，写入 `fe.out`。
  4. 写 `slide_window.py`：实现滑动窗口淘汰——`score = recency*0.4 + retrieval_freq*0.4 + importance*0.2`；淘汰 score 最低 20%；Bash 跑后断言淘汰后剩余数 ≈ 80%，写入 `sw.out`。

### CA19 记忆评估基准：LOCOMO / LongBench / MILE（脚本生成对话集）

- **预期档位**: medium
- **考察维度**: 记忆系统评估方法学
- **工具链**: Write → Bash → Write → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca19/` 下用 Write 写 `locomo_gen.py`：用脚本生成 LOCOMO 风格对话集——3 人轮流聊天 50 轮 + 6 类问题（事实回忆 / 时间推理 / 多跳 / 偏好 / 理解 / 知识更新）；Bash 跑后断言对话数 ≥ 50、问题数 ≥ 6，写入 `locomo.out`。
  2. 写 `eval_memory.py`：实现朴素 RAG vs MemGPT 风格评估——给定生成的对话集，跑两种检索方式统计正确率；Bash 跑后断言 MemGPT 风格正确率 ≥ 朴素 RAG（启发式），写入 `eval.out`。
  3. 写 `longbench_compare.md`：markdown 对比 LongBench vs LOCOMO——前者测"上下文窗口能力"后者测"记忆系统能力"；Bash `grep -c '^|' longbench_compare.md` ≥ 4 + `grep -F 'LongBench' longbench_compare.md` 命中。
  4. 写 `multimodal_sim.md`：markdown 描述多模态记忆挑战——图像 embedding（用 PIL resize + numpy flatten）/ OCR 文本 / 视频抽帧；Bash `grep -c '^- ' multimodal_sim.md` ≥ 3 + `grep -F 'embedding' multimodal_sim.md` 命中。

### CA20 laew 记忆系统升级路线（SQLite schema 模拟）

- **预期档位**: hard
- **考察维度**: 工程化升级 + laew 现状整合
- **工具链**: Write → Bash → Read → Bash
- **对话脚本**:
  1. 在 `tmpPlan/agent-test/ca20/` 下用 Write 写 `audit_current.md`：markdown 审计当前 laew 记忆架构——`Session.context` / `agent_memory` 表 / `session_memory` 表 / `<<<LAEW:SESSION_HISTORY>>>` 注入；Bash `grep -c '^## ' audit_current.md` ≥ 4 + `grep -F '缺陷' audit_current.md` 命中。
  2. 写 `upgrade_schema.sql`：升级 sqlite schema——新增 `core_memory TEXT` / `archived_memory TEXT` / `reflections TEXT` / `importance_score REAL` / `retrieval_count INTEGER` 列；Bash 跑 `python -c "import sqlite3; c=sqlite3.connect(':memory:'); c.executescript(open('upgrade_schema.sql').read()); print('OK')"` 后断言 schema 解析无语法错。
  3. 写 `pageout_trigger.py`：实现 page-out 触发器——`current_tokens / context_max >= 0.8` 触发 page-out 到 archived_memory；Bash 跑后断言阈值 0.8 触发正确，写入 `po.out`。
  4. 写 `reflection_writer.py`：实现 SessionContext 反思写入——任务结束写"关键经验+失败+建议"到 reflections 列；Bash 跑后断言反射文本非空且含 ≥ 3 个关键词（"教训"/"改进"/"下次"），写入 `rw.out`。
