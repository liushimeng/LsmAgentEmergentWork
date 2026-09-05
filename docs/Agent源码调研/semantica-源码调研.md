# Semantica 源码深度调研报告

## 工程概述

Semantica 是一个开源的「图原生（Graph-Native）」AI 基础设施框架，定位为 "The Open Source Palantir for AI Agents"。它为 AI Agent 提供语义层、知识图谱、决策智能、溯源追踪（W3C PROV-O）和确定性推理的底层框架。当前版本 0.6.7，使用 Python 3.8+ 编写，基于 MIT 协议。

工程的核心理念是：大多数 AI Agent 运行在 embedding 之上而非语义之上——只有相似度分数没有结构、关系和可解释性。Semantica 作为 LLM、向量存储和 Agent 框架之下的语义/上下文层，提供确定性的基础设施层（图构建、推理、溯源均不需要 LLM），将碎片化的企业数据转化为结构化、可查询的 Context Graph 和知识图谱。

---

### 一、工程结构

**主包组织**：工程采用单包多模块结构，主包 `semantica/` 下包含 27 个子模块，外加 `integrations/`、`explorer/`、`mcp/`、`plugins/`、`cookbook/`、`deploy/` 等扩展目录。

```
semantica/
├── semantica/          # 主包（27 个子模块）
│   ├── core/           # 核心编排器、生命周期、插件注册
│   ├── context/        # 上下文图、决策追踪、策略引擎
│   ├── kg/             # 知识图谱构建、实体解析、时序模型
│   ├── ingest/         # 多源摄取（文件/DB/云/流/Databricks/Snowflake/SAP）
│   ├── parse/          # 文档解析（PDF/DOCX/HTML/Code/Email/Image）
│   ├── semantic_extract/ # 语义抽取（NER/关系/事件/三元组）
│   ├── reasoning/      # 推理引擎（Rete/Datalog/SPARQL）
│   ├── ontology/       # 本体管理（OWL/SHACL/SKOS）
│   ├── provenance/     # 溯源系统（W3C PROV-O）
│   ├── vector_store/   # 向量存储（FAISS/Qdrant/Weaviate/Milvus/PgVector）
│   ├── graph_store/    # 图存储（Neo4j/FalkorDB/Apache AGE/Neptune）
│   ├── triplet_store/  # RDF 存储（Oxigraph/Blazegraph/Jena/RDF4J）
│   ├── llms/           # LLM 适配（9 个 Provider）
│   ├── pipeline/       # 流水线 DSL
│   ├── mcp_server/     # MCP Server 实现
│   ├── conflicts/      # 冲突检测与解决
│   ├── visualization/  # 可视化
│   ├── embeddings/     # 嵌入生成
│   ├── normalize/      # 文本归一化
│   ├── split/          # 分块策略
│   ├── deduplication/  # 去重
│   ├── export/         # 导出（RDF/JSON-LD/Parquet）
│   ├── seed/           # 种子数据管理
│   ├── change_management/ # 变更管理
│   ├── evals/          # 评估
│   ├── utils/          # 工具集
│   ├── explorer/       # Knowledge Explorer（React 前端）
│   ├── cli.py          # CLI 入口（50+ 命令）
│   ├── server.py       # FastAPI REST 服务
│   └── worker.py       # 后台 Worker
├── integrations/       # 多框架集成（agno/crewai/langchain/openclaw）
├── explorer/           # React 前端（Vite + TypeScript）
├── mcp/                # MCP 工具/资源定义
├── plugins/            # IDE 插件（Claude/Cursor/Cline/OpenClaw/VSCode/Windsurf/Codex）
├── cookbook/           # 教程与示例
├── deploy/             # 部署配置（K8s/Helm/Docker/云厂商）
└── tests/              # 测试套件
```

**扩展机制**：`integrations/` 目录提供 4 个框架集成（agno、crewai、langchain、openclaw），每个集成都是自包含的，通过 `optional-dependencies` 独立安装。`plugins/` 目录包含 7 个 IDE 插件（Claude Code、Cursor、Cline、OpenClaw、VSCode、Windsurf、Codex），通过 `.claude-plugin`、`.cursor-plugin` 等标准格式分发。

**入口点**：`pyproject.toml` 定义了 5 个 CLI 入口：
- `semantica` → `semantica.cli:main`（主 CLI）
- `semantica-server` → `semantica.server:main`（REST API）
- `semantica-worker` → `semantica.worker:main`（后台 Worker）
- `semantica-explorer` → `semantica.explorer:main`（Explorer 仪表盘）
- `semantica-mcp` → `semantica.mcp_server:main`（MCP Server）

---

### 二、核心架构

**Semantica 主类**位于 `semantica/core/orchestrator.py`，是整个框架的入口点：

```python
class Semantica:
    def __init__(self, config=None, **kwargs):
        self.config_manager = ConfigManager()
        self.lifecycle_manager = LifecycleManager()
        self.plugin_registry = PluginRegistry()
        # 惰性加载各模块
        self._modules: Dict[str, Any] = {}
```

**编排器模式**：`Semantica` 类通过 `@property` 装饰器实现模块的惰性加载（lazy loading），仅在首次访问时实例化各子模块（`embedding_generator`、`reasoner`、`graph_builder`、`document_parser`、`file_ingestor`、`pipeline_builder`）。这种设计避免了启动时的重量级依赖加载。

**生命周期管理**：`LifecycleManager`（`semantica/core/lifecycle.py`）管理系统的完整生命周期：

```python
class SystemState(str, Enum):
    UNINITIALIZED = "uninitialized"
    INITIALIZING = "initializing"
    READY = "ready"
    RUNNING = "running"
    STOPPING = "stopping"
    STOPPED = "stopped"
    ERROR = "error"
```

核心方法包括 `startup()`（按优先级执行启动钩子）、`shutdown()`（优雅关闭）、`health_check()`（组件健康检查）、`register_startup_hook()`/`register_shutdown_hook()`（优先级排序的钩子系统）。

**插件注册**：`PluginRegistry`（`semantica/core/plugin_registry.py`）提供动态插件发现、依赖解析和生命周期管理。插件必须实现 `initialize()` 和 `execute()` 方法，支持从文件系统路径自动发现。

**配置管理**：`ConfigManager`（`semantica/core/config_manager.py`）支持多层配置加载（环境变量、配置文件、运行时参数），通过 `get()` 方法支持点号路径访问嵌套配置。

---

### 三、Context Graph

Context Graph 模块（`semantica/context/`）是 Semantica 最核心的创新，提供 Agent 上下文管理和决策智能能力。

**核心类**：
- `ContextGraph`（`context_graph.py`，227K）：内存图存储，支持 KG 算法集成和全面决策管理
- `ContextNode`/`ContextEdge`：图数据结构，支持时序有效性（`valid_from`/`valid_until`）
- `AgentContext`（`agent_context.py`，95K）：高层接口，集成 KG、向量存储和决策追踪
- `AgentMemory`（`agent_memory.py`，87K）：持久化 Agent 记忆，集成 RAG

**决策记录**：`DecisionRecorder`（`decision_recorder.py`）负责记录决策的完整上下文：

```python
def record_decision(self, decision: Decision, entities: List[str], 
                    source_documents: List[str]) -> str:
    # 生成 embedding → 存储决策节点 → 链接实体 → 追踪溯源
```

**因果链分析**：`CausalChainAnalyzer`（`causal_analyzer.py`）通过图遍历追踪决策因果关系，支持上游（`upstream`）和下游（`downstream`）双向追踪，使用 BFS 算法在决策图中寻找因果链。

**策略引擎**：`PolicyEngine`（`policy_engine.py`）管理策略的版本、合规检查和影响分析。策略以 `Policy` 数据类表示，支持规则评估、违规检测和审批链管理。

**时序模型**：`ContextNode` 和 `ContextEdge` 都支持 `valid_from`/`valid_until` 时间戳，`is_active()` 方法判断节点/边在特定时间是否有效。`_parse_iso_dt()` 函数支持多种时间格式（年、日期、完整 ISO、带时区）。

**实体链接**：`EntityLinker`（`entity_linker.py`）跨源链接实体并分配 URI，支持实体消歧和归一化。

**上下文检索**：`ContextRetriever`（`context_retriever.py`，118K）提供混合检索（向量 + 图 + 记忆），支持语义搜索、结构相似度和时间过滤。

---

### 四、知识图谱

KG 模块（`semantica/kg/`）提供知识图谱构建、实体解析、时序模型和图分析能力。

**GraphBuilder**（`graph_builder.py`，62K）：知识图谱构建器，支持时序知识图谱、实体解析、冲突检测。关键参数包括 `merge_entities`（实体合并）、`entity_resolution_strategy`（fuzzy/exact/ml-based）、`enable_temporal`（时序启用）、`temporal_granularity`（时间粒度）。

**实体解析**：`EntityResolver`（`entity_resolver.py`）三步流程：检测重复组 → 合并重复项 → 合并非重复项。底层使用 `DuplicateDetector` 和 `EntityMerger`。

**时序模型**：`BiTemporalFact`（`temporal_model.py`）实现双时序事实模型，包含四个时间维度：
- `valid_from`/`valid_until`：业务有效性时间
- `recorded_at`：记录时间
- `superseded_at`：取代时间

`TemporalBound.OPEN` 哨兵值表示开放区间。

**图分析算法**：
- `CentralityCalculator`：度中心性、介数中心性、接近中心性、特征向量中心性、PageRank
- `CommunityDetector`：Louvain、Leiden、K-clique 社区检测
- `ConnectivityAnalyzer`：连通分量、桥接检测
- `PathFinder`：BFS 最短路径、全对最短路径
- `LinkPredictor`：关系预测
- `SimilarityCalculator`：多类型相似度
- `NodeEmbedder`：Node2Vec 节点嵌入

**时序查询**：`TemporalGraphQuery`（`temporal_query.py`，71K）支持时间点查询、时间范围查询、时序模式检测、图演化分析和时序路径查找。

---

### 五、摄取管线

Ingest 模块（`semantica/ingest/`）提供多源数据摄取能力，支持 15+ 种数据源。

**核心类**：
- `FileIngestor`（`file_ingestor.py`）：本地文件 + 云存储（S3/GCS/Azure）
- `WebIngestor`（`web_ingestor.py`）：网页抓取，支持 robots.txt、限速、sitemap
- `FeedIngestor`（`feed_ingestor.py`）：RSS/Atom 订阅
- `StreamIngestor`（`stream_ingestor.py`）：Kafka/RabbitMQ/Kinesis/Pulsar
- `RepoIngestor`（`repo_ingestor.py`）：Git 仓库摄取
- `EmailIngestor`（`email_ingestor.py`）：IMAP/POP3 邮件
- `DBIngestor`（`db_ingestor.py`）：SQL 数据库（PostgreSQL/MySQL/SQLite/Oracle）
- `SnowflakeIngestor`（`snowflake_ingestor.py`）：Snowflake 数仓
- `DatabricksIngestor`（`databricks_ingestor.py`）：Databricks Unity Catalog + Delta Lake
- `SAPIngestor`（`sap_ingestor.py`）：SAP OData（Business Partners、Sales Orders）
- `SalesforceIngestor`（`salesforce_ingestor.py`）：Salesforce CRM
- `ParquetIngestor`（`parquet_ingestor.py`）：Apache Parquet
- `ArrowIngestor`（`arrow_ingestor.py`）：Apache Arrow IPC/Feather
- `MCPIngestor`（`mcp_ingestor.py`）：MCP 资源摄取

**统一摄取函数**：`ingest()` 方法通过 `source_type` 参数分发到具体摄取器。

**文件类型检测**：`FileTypeDetector` 使用三种方法（扩展名、MIME 类型、魔数分析）检测文件类型，支持文档、图像、音频、视频格式。

**SSRF 防护**：`ssrf.py` 提供 SSRF（服务端请求伪造）防护，对 Web 摄取的 URL 进行安全检查。

**MCP 客户端**：`mcp_client.py` 实现 MCP 客户端，支持从 MCP 服务器摄取资源和工具。

---

### 六、文档解析

Parse 模块（`semantica/parse/`）处理多种文档格式解析。

**核心类**：
- `DocumentParser`（`document_parser.py`）：PDF/DOCX/HTML/TXT 解析
- `WebParser`（`web_parser.py`）：HTML/XML/JavaScript 渲染内容
- `StructuredDataParser`（`structured_data_parser.py`）：JSON/CSV/XML/YAML
- `EmailParser`（`email_parser.py`）：邮件头/正文/附件/线程
- `CodeParser`（`code_parser.py`）：多语言 AST 解析、注释提取、依赖分析
- `MediaParser`（`media_parser.py`）：图像 OCR、音频/视频元数据
- `DoclingParser`（`docling_parser.py`）：Docling 增强解析（可选依赖）

**PDF 解析**：`PDFParser` 基于 pdfplumber，支持文本提取、表格提取、图像提取、元数据提取、页级处理。

**DOCX 解析**：`DOCXParser` 基于 python-docx，支持段落、表格、章节/标题检测、核心属性。

**代码解析**：`CodeParser` 使用 `ast.parse()` 进行 Python AST 遍历，支持函数/类提取、导入语句解析、注释提取、依赖分析。

**图像 OCR**：`ImageParser` 集成 Tesseract OCR，支持文本提取、置信度评分、边界框提取。

**Docling 集成**：`DoclingParser` 是可选依赖，支持 PDF/DOCX/PPTX/XLSX/HTML/图像，提供增强的表格提取和文档结构理解。

---

### 七、语义抽取

Semantic Extract 模块（`semantica/semantic_extract/`）提供 NER、关系抽取、事件检测、三元组抽取等能力。

**核心类**：
- `NamedEntityRecognizer`（`named_entity_recognizer.py`）：NER 协调器
- `NERExtractor`（`ner_extractor.py`）：核心 NER 实现（spaCy/HuggingFace/LLM/Pattern）
- `RelationExtractor`（`relation_extractor.py`）：关系抽取
- `EventDetector`（`event_detector.py`）：事件检测和分类
- `CoreferenceResolver`（`coreference_resolver.py`）：共指消解
- `TripletExtractor`（`triplet_extractor.py`）：RDF 三元组抽取
- `SemanticAnalyzer`（`semantic_analyzer.py`）：语义分析
- `SemanticNetworkExtractor`（`semantic_network_extractor.py`）：语义网络构建
- `LLMExtraction`（`llm_extraction.py`）：LLM 增强抽取

**三元组抽取方法**：
- `pattern`：基于模式匹配（默认）
- `rules`：基于语言规则
- `huggingface`：自定义 HuggingFace 模型
- `llm`：LLM 驱动抽取

**加权置信度评分**：`Score = (0.5 * Method_Confidence) + (0.5 * Type_Similarity_Score)`，其中 Type_Similarity_Score 支持精确匹配（1.0）、同义词（0.95）、嵌入余弦相似度。

**Provider 抽象**：`providers.py`（70K）定义 `BaseProvider` 和具体实现（`OpenAIProvider`、`GeminiProvider`、`AnthropicProvider`、`GroqProvider`、`OllamaProvider`、`HuggingFaceLLMProvider`），通过 `create_provider()` 工厂函数创建。

---

### 八、推理引擎

Reasoning 模块（`semantica/reasoning/`）提供多种推理能力。

**Rete 引擎**（`rete_engine.py`，23K）：实现 Rete 算法用于高效规则匹配：

```python
class ReteEngine:
    def add_rule(self, rule: Rule): ...
    def add_fact(self, fact: Fact): ...
    def get_matches(self) -> List[Match]: ...
```

网络节点包括 `AlphaNode`（单条件匹配）、`BetaNode`（连接操作）、`TerminalNode`（规则激活）。`unify_condition()` 函数实现模式匹配，使用正则表达式进行变量绑定。

**Datalog 推理**（`datalog_reasoner.py`）：原生 Datalog 引擎，使用自底向上半朴素不动点求值（bottom-up semi-naive fixpoint evaluation），支持递归规则和多跳推理。

```python
class DatalogReasoner:
    def add_fact(self, fact: Any) -> None: ...  # 支持字符串和字典
    def evaluate(self) -> Set[DatalogFact]: ...  # 不动点求值
```

**SPARQL 推理**（`sparql_reasoner.py`）：基于 RDFLib 的 SPARQL 推理。

**解释生成**（`explanation_generator.py`）：生成推理路径和证明（`Explanation`、`Justification`、`ReasoningPath`）。

**时序推理**（`temporal_reasoning.py`）：`TemporalReasoningEngine` 支持区间关系（Allen 区间代数）。

---

### 九、本体管理

Ontology 模块（`semantica/ontology/`）提供 6 阶段本体生成流水线：

1. **语义网络解析**：从实体/关系提取领域概念
2. **YAML 到定义**：概念转化为类定义
3. **定义到类型**：映射到 OWL 类型
4. **层次生成**：构建分类结构，DFS 检测循环依赖
5. **TTL 生成**：使用 rdflib 生成 OWL/Turtle 语法
6. **符号验证**：HermiT/Pellet 推理机一致性检查

**核心类**：
- `OntologyGenerator`（`ontology_generator.py`，57K）：6 阶段流水线主类
- `ClassInferrer`（`class_inferrer.py`）：类发现和层次构建
- `PropertyGenerator`（`property_generator.py`）：属性推断和数据类型
- `OWLGenerator`（`owl_generator.py`）：OWL/RDF 生成
- `OntologyValidator`（`ontology_validator.py`）：本体验证
- `SHACLGenerator`：SHACL 形状生成
- `NamespaceManager`（`namespace_manager.py`）：命名空间和 IRI 管理
- `NamingConventions`（`naming_conventions.py`）：命名规范（PascalCase/camelCase）
- `RequirementsSpecManager`：需求规范和能力问题管理
- `ReuseManager`：本体复用管理
- `OntologyEvaluator`：本体质量评估
- `OntologyQualityGate`（`quality_gate.py`，31K）：质量门禁

**LLM 增强生成**：`LLMOntologyGenerator` 使用 LLM 辅助本体生成。

---

### 十、溯源系统

Provenance 模块（`semantica/provenance/`）实现 W3C PROV-O 标准的溯源追踪。

**核心类**：
- `ProvenanceManager`（`manager.py`，63K）：统一溯源管理器
- `ProvenanceEntry`（`schemas.py`）：W3C PROV-O 兼容的溯源条目
- `ProvenanceStorage`（`storage.py`）：存储抽象
- `InMemoryStorage` / `SQLiteStorage`：内存/SQLite 存储后端

**W3C PROV-O 映射**：
- `entity_id` → `prov:Entity`
- `activity_id` → `prov:Activity`
- `agent_id`/`agent_type` → `prov:Agent`（Person|SoftwareAgent|Organization）
- `parent_entity_id` → `prov:wasDerivedFrom`
- `timestamp` → `prov:generatedAtTime`
- `invalidated` → `prov:Invalidation`

**审计级溯源**：支持 DOI + 页码 + 直接引用的审计级源追踪，`source_document`、`source_location`、`source_quote` 字段记录完整来源信息。

**哈希链**：`previous_checksum` 字段链接到前一条目，支持 `verify_chain()` 验证完整性，检测批量删除。

**桥接公理**：`bridge_axiom.py` 实现 L1→L2→L3 翻译链，支持跨层级溯源。

---

### 十一、向量存储

Vector Store 模块（`semantica/vector_store/`）支持 6 种向量存储后端。

**核心类**：
- `VectorStore`（`vector_store.py`，63K）：主接口
- `FAISSStore`（`faiss_store.py`）：FAISS 本地向量存储
- `QdrantStore`（`qdrant_store.py`）：Qdrant 向量数据库
- `WeaviateStore`（`weaviate_store.py`）：Weaviate 向量数据库
- `MilvusStore`（`milvus_store.py`）：Milvus 向量数据库
- `PineconeStore`（`pinecone_store.py`）：Pinecone 托管服务
- `PgVectorStore`（`pgvector_store.py`）：PostgreSQL pgvector
- `SQLiteVecStore`（`sqlite_vec_store.py`）：SQLite 向量扩展

**混合搜索**：`HybridSearch`（`hybrid_search.py`）结合向量相似度和元数据过滤，使用 RRF（Reciprocal Rank Fusion）算法融合多源结果：

```python
# RRF 公式: score = sum(1 / (k + rank))，k 通常取 60
```

**元数据过滤**：`MetadataFilter` 支持等值、不等式、成员、包含过滤，AND/OR 条件组合。

**命名空间管理**：`NamespaceManager`（`namespace_manager.py`）支持多租户隔离，向量到命名空间的字典映射。

**决策嵌入**：`DecisionEmbeddingPipeline`（`decision_embedding_pipeline.py`，40K）提供决策的嵌入生成和相似度搜索。

---

### 十二、图存储

Graph Store 模块（`semantica/graph_store/`）支持 4 种图数据库后端。

**核心类**：
- `GraphStore`（`graph_store.py`，38K）：统一图存储接口
- `Neo4jStore`（`neo4j_store.py`）：Neo4j 集成（Bolt 协议）
- `FalkorDBStore`（`falkordb_store.py`）：FalkorDB 集成（Redis 基础）
- `ApacheAgeStore`（`age_store.py`，45K）：Apache AGE（PostgreSQL 扩展）
- `AmazonNeptuneStore`（`amazon_neptune.py`，61K）：Amazon Neptune

**子组件**：
- `NodeManager`：节点 CRUD
- `RelationshipManager`：关系 CRUD
- `QueryEngine`：Cypher 查询执行和优化
- `GraphAnalytics`：图算法（中心性、社区、路径）

**查询消毒**：`query_sanitize.py` 提供 Cypher 注入防护。

**Neptune 认证**：`NeptuneAuthTokenManager` 管理 IAM 认证令牌。

---

### 十三、RDF 存储

Triplet Store 模块（`semantica/triplet_store/`）支持 5 种 RDF 存储后端。

**核心类**：
- `TripletStore`（`triplet_store.py`，27K）：统一三元组存储接口
- `OxigraphStore`（`oxigraph_store.py`）：嵌入式 Oxigraph（内存/磁盘）
- `BlazegraphStore`（`blazegraph_store.py`）：Blazegraph
- `JenaStore`（`jena_store.py`）：Apache Jena
- `RDF4JStore`（`rdf4j_store.py`）：Eclipse RDF4J
- `AnzoStore`（`anzo_store.py`）：Altair Anzo

**Oxigraph 集成**：`OxigraphStore` 使用 PyOxigraph 提供嵌入式 SPARQL 1.1 存储，支持命名图、事务批量加载、显式 `flush()` 保证持久性。

**查询引擎**：`QueryEngine`（`query_engine.py`）提供 SPARQL 查询执行和优化。

**批量加载**：`BulkLoader`（`bulk_loader.py`）支持高容量数据加载，分块处理。

**SPARQL 转义**：`sparql_escaping.py` 提供 SPARQL 注入防护。

---

### 十四、LLM 适配

LLMs 模块（`semantica/llms/`）提供 9 个 LLM Provider 封装。

**支持的 Provider**：
- `Groq`：Groq API 快速推理
- `OpenAI`：OpenAI API（GPT-3.5/GPT-4）
- `HuggingFaceLLM`：HuggingFace Transformers 本地推理
- `LiteLLM`：统一接口访问 100+ LLM（OpenAI/Anthropic/Groq/Azure/Bedrock/Vertex AI）
- `Anthropic`：Anthropic Claude API
- `Gemini`：Google Gemini API
- `Ollama`：本地模型服务
- `DeepSeek`：DeepSeek OpenAI 兼容 API
- `Novita`：Novita AI OpenAI 兼容 API

**统一接口**：所有 Provider 都提供 `generate(prompt, **kwargs) -> str` 方法，部分支持 `generate_structured()` 生成结构化 JSON。

**LiteLLM 集成**：`LiteLLM` 类使用 `provider/model-name` 格式（如 `openai/gpt-4o`、`anthropic/claude-sonnet-5`），通过 LiteLLM 库统一调用。

---

### 十五、流水线 DSL

Pipeline 模块（`semantica/pipeline/`）提供流水线构建和执行能力。

**核心类**：
- `PipelineBuilder`（`pipeline_builder.py`）：流水线构建 DSL
- `Pipeline` / `PipelineStep`：流水线和步骤数据类
- `ExecutionEngine`（`execution_engine.py`，32K）：执行引擎
- `FailureHandler`（`failure_handler.py`）：错误处理和重试
- `ParallelismManager`（`parallelism_manager.py`）：并行执行管理
- `ResourceScheduler`（`resource_scheduler.py`）：资源分配调度
- `PipelineValidator`（`pipeline_validator.py`）：流水线验证

**DSL 示例**：
```python
builder = PipelineBuilder()
pipeline = (builder
    .add_step("ingest", "file_ingest")
    .add_step("parse", "document_parse")
    .add_step("extract", "ner_extract")
    .build())
```

**失败处理**：`FailureHandler` 支持重试策略（`RetryStrategy`）、回退处理（`FallbackHandler`）、错误恢复（`ErrorRecovery`）。

**并行执行**：`ParallelismManager` 支持任务并行执行，`parallel_safe` 标记步骤可并行。

**增量模式**：`PipelineStep.delta_mode` 支持增量处理，`base_version_id`/`target_version_id` 指定版本范围。

---

### 十六、MCP Server

MCP Server 模块（`semantica/mcp_server/`）将 Semantica 能力暴露为 MCP 工具。

**工具定义**（`__init__.py` 中的 `TOOLS` 列表）：
1. `extract_entities`：命名实体抽取
2. `extract_relations`：关系和三元组抽取
3. `record_decision`：记录决策
4. `query_decisions`：查询决策
5. `find_precedents`：查找先例
6. `get_causal_chain`：获取因果链
7. `add_entity`：添加实体
8. `add_relationship`：添加关系
9. `run_reasoning`：运行推理规则
10. `get_graph_analytics`：图分析
11. `export_graph`：导出图
12. `get_graph_summary`：图摘要
13. `query_graph`：查询图
14. `update_node`：更新节点
15. `delete_node`：归档节点

**持久化**：通过 `SEMANTICA_KG_PATH` 环境变量指定持久化路径，mutation 操作自动保存到文件，支持原子写入失败回滚。

**懒加载**：`_get_graph()` 函数实现图的懒加载，首次调用时初始化 `ContextGraph`。

**进度条禁用**：MCP Server 强制设置 `SEMANTICA_DISABLE_PROGRESS=1`，避免进度条输出干扰 JSON-RPC 流。

---

### 十七、多框架集成

Integrations 模块提供 4 个 Agent 框架集成。

**Agno 集成**（`integrations/agno/`）：
- `KnowledgeGraph`：知识图谱工具包
- `Toolkit`：KG 工具集
- `ContextStore`：上下文存储
- `DecisionKit`：决策工具包
- `SharedContext`：共享上下文

**CrewAI 集成**（`integrations/crewai/`）：
- `KGTool`：KG 工具（BaseTool 子类）
- `DecisionTool`：决策工具
- `KnowledgeSource`：知识源（BaseKnowledgeSource 子类）

**LangChain 集成**（`integrations/langchain/`）：
- `Retriever`：检索器
- `Tools`：工具集
- `VectorStore`：向量存储

**OpenClaw 集成**（`integrations/openclaw/`）：
- `MCPTool`：MCP 工具封装

---

### 十八、冲突检测

Conflicts 模块（`semantica/conflicts/`）提供冲突检测和解决能力。

**核心类**：
- `ConflictDetector`（`conflict_detector.py`，58K）：多源冲突检测
- `ConflictResolver`（`conflict_resolver.py`）：冲突解决
- `ConflictAnalyzer`（`conflict_analyzer.py`）：冲突模式分析
- `SourceTracker`（`source_tracker.py`）：源追踪和溯源
- `InvestigationGuideGenerator`（`investigation_guide.py`）：调查指南生成

**冲突类型**：
```python
class ConflictType(str, Enum):
    VALUE_CONFLICT = "value_conflit"
    TYPE_CONFLICT = "type_conflict"
    RELATIONSHIP_CONFLICT = "relationship_conflict"
    TEMPORAL_CONFLICT = "temporal_conflict"
    LOGICAL_CONFLICT = "logical_conflict"
```

**解决策略**：
- `VOTING`：多数投票
- `CREDIBILITY_WEIGHTED`：可信度加权
- `MOST_RECENT`：最新值
- `FIRST_SEEN`：最早值
- `HIGHEST_CONFIDENCE`：最高置信度
- `MANUAL_REVIEW`：人工审核
- `EXPERT_REVIEW`：专家审核

**严重度计算**：多因素评分，结合属性重要性、值差异幅度、源数量。

---

### 十九、可视化

Visualization 模块（`semantica/visualization/`）提供全面的可视化能力。

**核心类**：
- `KGVisualizer`（`kg_visualizer.py`）：知识图谱网络和社区可视化
- `OntologyVisualizer`（`ontology_visualizer.py`，36K）：本体层次和结构可视化
- `EmbeddingVisualizer`（`embedding_visualizer.py`）：向量嵌入投影和聚类
- `SemanticNetworkVisualizer`（`semantic_network_visualizer.py`）：语义网络结构
- `AnalyticsVisualizer`（`analytics_visualizer.py`）：图分析和中心性排名
- `TemporalVisualizer`（`temporal_visualizer.py`）：时间线和演化可视化

**布局算法**：
- 力导向布局（NetworkX spring_layout）
- 层次树布局（BFS 遍历）
- 圆形布局

**降维算法**：UMAP、t-SNE、PCA 用于嵌入可视化。

**导出格式**：HTML（Plotly 交互式）、PNG、SVG、PDF、JSON。

**Explorer 前端**：`explorer/` 目录是 React + TypeScript + Vite 前端应用，提供交互式知识图谱浏览器。

---

## 架构图（文字描述）

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Sources (多源数据)                            │
│  Files │ Web │ DB │ Cloud │ Streams │ Git │ Email │ MCP │ Parquet   │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Ingest (摄取管线)                                │
│  FileIngestor │ WebIngestor │ DBIngestor │ StreamIngestor │ ...      │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Parse (文档解析)                                 │
│  PDF │ DOCX │ HTML │ Code │ Email │ Image │ JSON │ XML │ CSV         │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Normalize (文本归一化)                           │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Split (分块策略)                                 │
│  entity_aware │ relation_aware │ graph_based │ ontology_aware         │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Semantic Extract (语义抽取)                      │
│  NER │ Relations │ Events │ Triplets │ Coreference                  │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Conflicts (冲突检测)                             │
│  ConflictDetector │ ConflictResolver │ SourceTracker                 │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Deduplication (去重)                             │
│  DuplicateDetector │ EntityMerger                                     │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      KG Construction (知识图谱构建)                    │
│  GraphBuilder │ EntityResolver │ TemporalModel │ BiTemporalFact       │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Intelligence Layer (智能层)                      │
│  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐       │
│  │  Ontology  │ │ Reasoning  │ │ Provenance │ │  Context   │       │
│  │  OWL/SHACL │ │ Rete/Datalog│ │ W3C PROV-O│ │  Graph     │       │
│  │  /SKOS     │ │ /SPARQL    │ │            │ │ Decisions  │       │
│  └────────────┘ └────────────┘ └────────────┘ └────────────┘       │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Storage (存储层)                                 │
│  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐       │
│  │  Vector Store   │ │  Graph Store    │ │  Triplet Store  │       │
│  │ FAISS/Qdrant/   │ │ Neo4j/FalkorDB/ │ │ Oxigraph/Blaze- │       │
│  │ Weaviate/Milvus │ │ AGE/Neptune     │ │ graph/Jena/RDF4J│       │
│  └─────────────────┘ └─────────────────┘ └─────────────────┘       │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Outputs (输出层)                                 │
│  Export (RDF/JSON-LD/Parquet) │ Visualization │ REST API │ MCP      │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 对 laew 工程的借鉴建议

### 1. 决策追踪与溯源机制（高优先级）

Semantica 的 `ContextGraph` + `DecisionRecorder` + `ProvenanceManager` 形成完整的决策追踪链路，这正是 laew 当前缺失的。laew 的 SessionContext Agent 可以借鉴：

- **决策记录**：每次 Agent 决策（任务分类、工具调用选择）记录为 `Decision` 节点，包含 category、scenario、reasoning、outcome、confidence
- **因果链追踪**：`CausalChainAnalyzer` 的图遍历算法可用于追踪"为什么选择这个工具"的因果链
- **W3C PROV-O 溯源**：`ProvenanceManager` 的 `ProvenanceEntry` 模式可用于记录每条消息/工具调用的完整来源

### 2. 插件注册机制

Semantica 的 `PluginRegistry` 提供动态插件发现、依赖解析和生命周期管理。laew 当前的工具注册是静态的（`builtin_registry()`），可借鉴：

- 动态插件发现（从文件系统路径加载）
- 插件依赖自动解析
- 插件版本管理和兼容性检查
- 插件隔离和错误处理

### 3. 流水线 DSL

Semantica 的 `PipelineBuilder` 提供流畅的 DSL 构建复杂工作流。laew 的 MultiAgentOrchestrator 可借鉴：

- 步骤依赖声明（`dependencies`）
- 并行执行标记（`parallel_safe`）
- 增量处理模式（`delta_mode`）
- 失败重试和回退策略（`RetryStrategy`、`FallbackHandler`）

### 4. 冲突检测与解决

Semantica 的 `ConflictDetector` 提供多源冲突检测和多种解决策略。laew 的 Quality-Check Agent 可借鉴：

- 多策略解决（投票、可信度加权、最新值、人工审核）
- 严重度计算（多因素评分）
- 调查指南生成（`InvestigationGuideGenerator`）

### 5. 时序知识图谱

Semantica 的 `BiTemporalFact` 双时序模型（业务时间 + 记录时间）可用于 laew 的 Session 管理：

- 对话历史的时间旅行（查询特定时间点的上下文状态）
- 实体有效性窗口（某些知识只在特定时间段有效）
- 版本快照和演化分析

### 6. 混合检索策略

Semantica 的 `HybridSearch` 结合向量相似度和元数据过滤，使用 RRF 算法融合。laew 的 SessionContext 历史注入可借鉴：

- 语义相似度 + 时间衰减 + 结构关系的混合检索
- 多源结果融合（RRF 算法）
- 元数据过滤（按任务类型、难度分级过滤历史）

### 7. 生命周期管理

Semantica 的 `LifecycleManager` 提供优先级排序的启动/关闭钩子系统。laew 的 TUI 可借鉴：

- 组件健康检查（`health_check()`）
- 优雅关闭序列（资源清理、状态持久化）
- 错误状态恢复

### 8. MCP Server 暴露能力

Semantica 的 MCP Server 将核心能力暴露为 15 个 MCP 工具。laew 可借鉴：

- 将 Yolo 分类、任务执行、Session 管理暴露为 MCP 工具
- 支持 `SEMANTICA_KG_PATH` 类似的持久化机制
- 提供 stdio 传输模式供 IDE 集成

### 9. 多框架集成策略

Semantica 的 `integrations/` 目录提供 agno/crewai/langchain/openclaw 集成。laew 可借鉴：

- 提供 laew 作为知识层/决策层的集成接口
- 将 Agent 循环暴露为其他框架可调用的工具
- 支持作为 CrewAI 的 BaseTool 或 LangChain 的 Retriever

### 10. 确定性推理引擎

Semantica 的 Rete/Datalog/SPARQL 推理引擎是确定性的（不依赖 LLM）。laew 可借鉴：

- 规则引擎用于任务分类（替代或辅助 LLM 分类）
- 前向链推理用于工具选择决策
- 解释生成器提供可解释的决策路径

---

## 总结

Semantica 是一个设计精良的图原生 AI 基础设施框架，其核心优势在于：

1. **完整的决策智能链路**：从摄取→解析→抽取→冲突检测→KG构建→决策记录→溯源追踪，形成闭环
2. **确定性推理能力**：Rete/Datalog/SPARQL 推理引擎不依赖 LLM，提供可解释的推理路径
3. **W3C 标准兼容**：PROV-O 溯源、OWL/SHACL/SKOS 本体、SPARQL 查询，确保互操作性
4. **多后端抽象**：向量存储（6 种）、图存储（4 种）、RDF 存储（5 种）的统一接口
5. **企业级特性**：审计级溯源、冲突检测、策略引擎、版本管理

对于 laew 工程，最值得借鉴的是其决策追踪与溯源机制、插件注册系统、流水线 DSL 和混合检索策略。这些能力可以显著提升 laew 在 Agent 决策可解释性、工具扩展性和工作流编排方面的能力。
