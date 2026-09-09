# 自动化测试提示词 — AI 工程与 LLM 应用实战（S01–S10）

> 使用说明见同目录 `README.md`。每条提示词在同一 Session 内按轮次顺序喂入。

## 维度说明

本维度考察 Agent 在 **LLM 应用工程化、Prompt 工程、RAG 检索增强生成、
Agent 框架实现、模型微调与评估、多模态、Token 成本控制、向量化与向量库、
LLM 网关与故障转移、MLOps** 等"AI 工程"垂直领域中的实战表现，
与 E 维度（通用编码调试）、Q 维度（编程范式与垂直领域）、R 维度（Web 全栈）互补：
E 考察"常规软件开发"，Q 考察"编程范式与特定垂直领域（游戏/编译器/嵌入式）"，
R 考察"浏览器端与全栈 Web 工程实现"，
本维度考察"基于大模型的 AI 应用从原型到生产"的工程实现。
所有主题都以"动手写 AI 应用的代码或方案"为核心，强调 AI 工程实战能力。

---

### S01 用 OpenAI / Anthropic API 开发对话机器人
- **预期档位**: medium
- **考察维度**: LLM API 集成 + 流式响应 + 多轮上下文
- **对话脚本**:
  1. 我想用 Python 集成 OpenAI 和 Anthropic 两家 API 做对话机器人，先设计统一的 `LlmClient` 抽象接口（屏蔽协议差异）。
  2. 实现核心功能：流式响应（SSE chunk 解析）、多轮对话上下文管理（消息数组 + token 估算）、错误重试（指数退避 + 429 限流处理）。
  3. 加上 tool calling：用 JSON Schema 定义两个工具（`get_weather` / `search_db`），让 LLM 自动调用并把结果回填到上下文。
  4. 讨论：Anthropic（`tools[].input_schema`）和 OpenAI（`function.parameters`）的工具定义协议在嵌套 `$ref`、`anyOf`、`enum` 上有哪些差异？怎么用 Pydantic 自动生成两套 Schema？

### S02 RAG 系统：从文档到向量检索全链路
- **预期档位**: hard
- **考察维度**: RAG 架构 + 文档分块 + Embedding + 向量检索
- **对话脚本**:
  1. 我想用 RAG（检索增强生成）做一个企业知识库问答系统，先设计整体架构（文档摄入 → 分块 → Embedding → 检索 → 生成）。
  2. 实现核心功能：用 `langchain` 或纯手写实现文档加载（PDF/Markdown/HTML）→ 按 512 token 滑窗分块（带 64 token 重叠）→ 调用 Embedding API → 存入本地向量库（如 `chromadb` 或 `qdrant`）。
  3. 加上混合检索：BM25 关键词检索 + 向量语义检索，用 RRF（Reciprocal Rank Fusion）公式 `score = Σ 1/(k+rank_i)`（k=60）合并两路结果。
  4. 讨论：分块策略（固定大小 / 按段落 / 按语义）对召回质量影响多大？怎么评测 RAG 系统的端到端效果？

### S03 Prompt 工程：从基础到高级技巧
- **预期档位**: medium
- **考察维度**: Prompt 设计 + Few-shot + Chain-of-Thought + ReAct
- **对话脚本**:
  1. 解释 Prompt 工程的核心原则（角色设定、上下文约束、输出格式指令、负面约束），并给一个"差→好"的对比示例。
  2. 用 Few-shot（3 个示例）让 LLM 完成"从自然语言提取结构化信息"任务（如从商品评论里抽 `商品/优点/缺点/评分`），对比 zero-shot 和 few-shot 的输出差异。
  3. 用 Chain-of-Thought（CoT）+ ReAct 框架让 LLM 解决"数学应用题"：先思考步骤，再选择"调用计算器工具"或"直接回答"，给出完整 prompt 模板。
  4. 讨论：Prompt 注入（Prompt Injection）攻击有哪些形态？怎么在系统提示词里设计防御（输入净化、工具调用沙箱、权限分级）？

### S04 Agent 框架设计：从 ReAct 到多 Agent 协作
- **预期档位**: hard
- **考察维度**: Agent 循环 + 工具注册 + 状态管理 + 多 Agent 通信
- **对话脚本**:
  1. 用 Python 从零实现一个最小 Agent 循环（ReAct 范式）：LLM 思考 → 调用工具 → 观察结果 → 继续思考，直到给出最终答案。
  2. 实现工具注册表（`ToolRegistry`）：支持动态注册、JSON Schema 校验、超时控制、并发执行（asyncio.gather）。
  3. 加上多 Agent 协作：Planner Agent 拆解任务 → Worker Agent 执行子任务 → Reviewer Agent 质检，最终汇总结果。设计 Agent 之间的通信协议（消息队列 + 状态传递）。
  4. 讨论：单 Agent 加更多工具 vs 多 Agent 分工，各自的边界在哪？什么场景下必须用多 Agent？

### S05 模型微调：LoRA + QLoRA 实战
- **预期档位**: hard
- **考察维度**: 大模型微调 + 显存优化 + 训练框架
- **对话脚本**:
  1. 解释全量微调（Full Fine-tuning）、LoRA、QLoRA 三者的区别，画一张"显存占用 vs 效果"的取舍图。
  2. 用 `peft` + `bitsandbytes` 在单张 24GB 显卡上 QLoRA 微调 Llama-3-8B：4-bit 量化基座 + LoRA adapter，训练数据是 1000 条客服对话。
  3. 加上评估：训练完成后在 200 条测试集上对比 base model 和 QLoRA 模型的回复质量（人工打分 + LLM-as-Judge）。
  4. 讨论：LoRA 的 rank（r=8 / 16 / 64）怎么选？为什么 LoRA 微调后合并权重可能丢失一部分能力？merge 策略有哪些？

### S06 多模态应用：图文理解与生成
- **预期档位**: medium
- **考察维度**: 多模态 API + Vision + 图片处理 + Prompt 设计
- **对话脚本**:
  1. 用 Anthropic Claude 或 OpenAI GPT-4V 实现一个"看图问答"应用：上传一张图，提问关于图的内容，模型给出回答。
  2. 实现图片预处理：上传前自动 resize 到最长边 1568px、压缩 JPEG 质量到 85%，避免触发 token 上限。先解释 Vision API 的 token 计费规则（图片按 512×512 块切分）。
  3. 加上多图对比：上传 2~3 张图，让模型对比差异（如"这两张户型图哪个采光更好"），并把对比结果导出为结构化 JSON。
  4. 讨论：Vision API 在 OCR、图表理解、UI 截图分析上的成功率差异有多大？哪些场景必须用专门的 OCR 模型（如表格、票据）？

### S07 Token 成本控制与缓存策略
- **预期档位**: medium
- **考察维度**: Token 计量 + 成本优化 + Prompt Caching + 上下文压缩
- **对话脚本**:
  1. 解释 LLM API 的计费规则：输入 / 输出 / cache_hit 三档价格差异（以 Claude Sonnet 为例：3 / 15 / 0.3 美元每百万 token），为什么输出比输入贵 5 倍？
  2. 实现一个 Token 预算管理器：每次对话前估算 token 数（用 `tiktoken` 或近似公式 `字符数/4`），超过单次窗口 80% 时自动触发上下文压缩。
  3. 加上 Prompt Caching：把系统提示词和大文档标记为 cache 断点（`cache_control: {type: "ephemeral"}`），观察 cache hit 率和成本节省比例。
  4. 讨论：除了缓存，还有哪些降本手段（模型路由 / 任务分级 / 输出限制 / 摘要复用）？给一个"日均 100 万 token 消费"的成本优化清单。

### S08 向量数据库选型与混合检索
- **预期档位**: hard
- **考察维度**: 向量库架构 + 索引算法 + 混合检索 + 工程权衡
- **对话脚本**:
  1. 对比 5 个主流向量数据库（ChromaDB / Qdrant / Milvus / Weaviate / pgvector）的特性差异：单机 vs 分布式、索引算法（HNSW / IVF）、metadata 过滤能力、性能。
  2. 用 Qdrant 实现一个产品搜索系统：入库 10 万条商品向量（512 维），支持按类目 metadata 过滤 + 余弦相似度检索，QPS 压测到 1000+。
  3. 加上混合检索：BM25（关键词）+ Dense Vector（语义）+ Cross-Encoder Reranker 三阶段流水线，对比纯向量检索的 Top-5 准确率提升。
  4. 讨论：向量量化为啥能省内存？INT8 / 二进制量化 / Product Quantization 的精度损失和压缩率对比？什么场景下不值得量化？

### S09 LLM 网关与多 Provider 故障转移
- **预期档位**: hard
- **考察维度**: LLM Gateway + 路由算法 + 熔断器 + 配额管理
- **对话脚本**:
  1. 设计一个 LLM 网关架构：统一接入 OpenAI / Anthropic / 自托管 Ollama 多家 Provider，按路由策略（成本 / 延迟 / 任务类型）分发请求。
  2. 实现核心功能：协议转换层（把 Anthropic 格式转 OpenAI 格式，反之亦然），让上层 Agent 不感知 Provider 差异。
  3. 加上熔断器（3 态：Closed / Open / Half-Open）和故障转移：某 Provider 连续 5 次 5xx 错误就熔断 60 秒，期间流量自动切到备选 Provider。
  4. 讨论：多 Provider 路由的关键指标有哪些（成功率 / P99 延迟 / 每千 token 成本）？怎么设计灰度发布和金丝雀切流？

### S10 LLM 应用评估：自动化测试与质量监控
- **预期档位**: hard
- **考察维度**: Eval 体系 + 自动化评测 + LLM-as-Judge + 监控告警
- **对话脚本**:
  1. 设计一个 LLM 应用评估体系：分单元测试（单轮回复质量）、集成测试（多轮对话连贯性）、端到端测试（用户任务完成率）三层。
  2. 用 LLM-as-Judge 实现自动评分：让 GPT-4 当评委，按 5 个维度（准确性 / 相关性 / 流畅性 / 安全性 / 有用性）给被测模型回复打分（1-5 分）。
  3. 加上回归测试集：构建 200 条覆盖 8 类典型场景（闲聊 / 知识问答 / 代码生成 / 翻译 / 摘要 / 推理 / 多轮 / 工具调用）的金标准测试集，每次模型升级都跑一遍。
  4. 讨论：LLM-as-Judge 本身的偏差（位置偏差 / 长度偏差 / 自我偏好）怎么校正？生产环境怎么监控线上回复质量（抽样人工评分 + 用户反馈信号）？
