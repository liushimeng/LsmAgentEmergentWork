# Semantica 综合深度分析

> 调研对象:Semantica(Python,图原生 AI 基础设施)
> 调研日期:2026-09-05
> 原始文档:3 份(源码调研 + 深度分析 + 核心机制深度分析)
> 总行数:~2,500 行(合并后,去重精简)

---

## 目录

1. [项目元信息](#一项目元信息)
2. [Context Graph](#二context-graph)
3. [决策因果链](#三决策因果链)
4. [Rete 网络](#四rete-网络)
5. [Datalog 半朴素不动点](#五datalog-半朴素不动点)
6. [SPARQL](#六sparql)
7. [W3C PROV-O 溯源](#七w3c-prov-o-溯源)
8. [BiTemporalFact 双时序模型](#八bitemporal-双时序模型)
9. [冲突检测 7 种策略](#九冲突检测-7-种策略)
10. [对 laew 的借鉴](#十对-laew-的借鉴)

---

## 一、项目元信息

### 1.1 工程概述

Semantica 是一个开源的「图原生(Graph-Native)」AI 基础设施框架,定位为 "The Open Source Palantir for AI Agents"。它为 AI Agent 提供语义层、知识图谱、决策智能、溯源追踪(W3C PROV-O)和确定性推理的底层框架。当前版本 0.6.7,使用 Python 3.8+ 编写,基于 MIT 协议。

核心理念:大多数 AI Agent 运行在 embedding 之上而非语义之上——只有相似度分数没有结构、关系和可解释性。Semantica 作为 LLM、向量存储和 Agent 框架之下的语义/上下文层,提供确定性的基础设施层(图构建、推理、溯源均不需要 LLM),将碎片化的企业数据转化为结构化、可查询的 Context Graph 和知识图谱。

### 1.2 工程结构

**主包组织**:工程采用单包多模块结构,主包 `semantica/` 下包含 27 个子模块,外加 `integrations/`、`explorer/`、`mcp/`、`plugins/`、`cookbook/`、`deploy/` 等扩展目录。

```
semantica/
├── semantica/          # 主包(27 个子模块)
│   ├── core/           # 核心编排器、生命周期、插件注册
│   ├── context/        # 上下文图、决策追踪、策略引擎
│   ├── kg/             # 知识图谱构建、实体解析、时序模型
│   ├── ingest/         # 多源摄取(文件/DB/云/流/Databricks/Snowflake/SAP)
│   ├── parse/          # 文档解析(PDF/DOCX/HTML/Code/Email/Image)
│   ├── semantic_extract/ # 语义抽取(NER/关系/事件/三元组)
│   ├── reasoning/      # 推理引擎(Rete/Datalog/SPARQL)
│   ├── ontology/       # 本体管理(OWL/SHACL/SKOS)
│   ├── provenance/     # 溯源系统(W3C PROV-O)
│   ├── vector_store/   # 向量存储(FAISS/Qdrant/Weaviate/Milvus/PgVector)
│   ├── graph_store/    # 图存储(Neo4j/FalkorDB/Apache AGE/Neptune)
│   ├── triplet_store/  # RDF 存储(Oxigraph/Blazegraph/Jena/RDF4J)
│   ├── llms/           # LLM 适配(9 个 Provider)
│   ├── pipeline/       # 流水线 DSL
│   ├── mcp_server/     # MCP Server 实现
│   ├── conflicts/      # 冲突检测与解决
│   ├── visualization/  # 可视化
│   ├── embeddings/     # 嵌入生成
│   ├── normalize/      # 文本归一化
│   ├── split/          # 分块策略
│   ├── deduplication/  # 去重
│   ├── export/         # 导出(RDF/JSON-LD/Parquet)
│   ├── seed/           # 种子数据管理
│   ├── change_management/ # 变更管理
│   ├── evals/          # 评估
│   ├── utils/          # 工具集
│   ├── explorer/       # Knowledge Explorer(React 前端)
│   ├── cli.py          # CLI 入口(50+ 命令)
│   ├── server.py       # FastAPI REST 服务
│   └── worker.py       # 后台 Worker
├── integrations/       # 多框架集成(agno/crewai/langchain/openclaw)
├── explorer/           # React 前端(Vite + TypeScript)
├── mcp/                # MCP 工具/资源定义
├── plugins/            # IDE 插件(Claude/Cursor/Cline/OpenClaw/VSCode/Windsurf/Codex)
├── cookbook/           # 教程与示例
├── deploy/             # 部署配置(K8s/Helm/Docker/云厂商)
└── tests/              # 测试套件
```

**入口点**(`pyproject.toml` 定义 5 个 CLI 入口):
- `semantica` → `semantica.cli:main`(主 CLI)
- `semantica-server` → `semantica.server:main`(REST API)
- `semantica-worker` → `semantica.worker:main`(后台 Worker)
- `semantica-explorer` → `semantica.explorer:main`(Explorer 仪表盘)
- `semantica-mcp` → `semantica.mcp_server:main`(MCP Server)

### 1.3 核心架构

**Semantica 主类**位于 `semantica/core/orchestrator.py`:

```python
class Semantica:
    def __init__(self, config=None, **kwargs):
        self.config_manager = ConfigManager()
        self.lifecycle_manager = LifecycleManager()
        self.plugin_registry = PluginRegistry()
        self._modules: Dict[str, Any] = {}  # 惰性加载各模块
```

**编排器模式**:通过 `@property` 装饰器实现模块惰性加载,避免启动时重量级依赖加载。

**生命周期管理**(`LifecycleManager`):

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

核心方法:`startup()`(按优先级执行启动钩子)、`shutdown()`(优雅关闭)、`health_check()`(组件健康检查)、`register_startup_hook()`/`register_shutdown_hook()`(优先级排序的钩子系统)。

**插件注册**(`PluginRegistry`):提供动态插件发现、依赖解析和生命周期管理。插件必须实现 `initialize()` 和 `execute()` 方法。

**配置管理**(`ConfigManager`):支持多层配置加载(环境变量、配置文件、运行时参数),通过 `get()` 方法支持点号路径访问嵌套配置。

### 1.4 其他模块概览

| 模块 | 核心能力 |
|------|---------|
| **Parse** | PDF/DOCX/HTML/TXT/Email/Code/Image/Media/Docling 多格式解析 |
| **Semantic Extract** | NER/关系抽取/事件检测/三元组/共指消解/语义网络/LLM 增强抽取 |
| **Ontology** | 6 阶段本体生成流水线,OWL/SHACL/SKOS 支持,LLM 辅助生成 |
| **Vector Store** | 7 种后端(FAISS/Qdrant/Weaviate/Milvus/Pinecone/PgVector/SQLiteVec),RRF 混合检索 |
| **Graph Store** | 4 种后端(Neo4j/FalkorDB/Apache AGE/Neptune),Cypher 查询消毒 |
| **Triplet Store** | 5 种后端(Oxigraph/Blazegraph/Jena/RDF4J/Anzo),SPARQL 1.1 |
| **LLMs** | 9 个 Provider(Groq/OpenAI/HuggingFace/LiteLLM/Anthropic/Gemini/Ollama/DeepSeek/Novita) |
| **Pipeline** | 流畅 DSL,支持依赖声明、并行标记、增量模式、失败重试 |
| **MCP Server** | 15 个 MCP 工具(extract/record/query/reason/analytics/export 等) |
| **Integrations** | agno/crewai/langchain/openclaw 四框架集成 |
| **Visualization** | KG/Ontology/Embedding/SemanticNetwork/Analytics/Temporal 可视化 |

### 1.5 架构总图

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Sources (多源数据)                            │
│  Files │ Web │ DB │ Cloud │ Streams │ Git │ Email │ MCP │ Parquet   │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Ingest (摄取管线,17+ 摄取器)                     │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Parse (文档解析,10+ 格式)                        │
└──────────────────────────────────────────────────────────────────────┘
                                 ▼ Normalize → Split → Semantic Extract
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Conflicts (冲突检测 5 种类型)                    │
│                      Deduplication (去重)                             │
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
│                      Storage (存储层,多后端抽象)                      │
│  Vector(7) │ Graph(4) │ Triplet(5) │ 统一接口 + NamespaceManager      │
└──────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────────────────────────────────────────────────────────┐
│                      Outputs (输出层)                                 │
│  Export(RDF/JSON-LD/Parquet) │ Visualization │ REST API │ MCP        │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 二、Context Graph

Context Graph 模块(`semantica/context/`)是 Semantica 最核心的创新,提供 Agent 上下文管理和决策智能能力。代码路径:`semantica/context/context_graph.py`(5659 行)。

### 2.1 ContextNode / ContextEdge 数据结构

**核心数据类**:

```python
@dataclass
class ContextNode:
    node_id: str
    node_type: str
    content: str
    metadata: Dict[str, Any] = field(default_factory=dict)
    properties: Dict[str, Any] = field(default_factory=dict)
    valid_from: Optional[str] = None   # ISO datetime
    valid_until: Optional[str] = None  # ISO datetime

    def is_active(self, at_time: Optional[datetime] = None) -> bool:
        """判断节点在指定时间是否有效"""

@dataclass
class ContextEdge:
    source_id: str
    target_id: str
    edge_type: str
    edge_id: str = ""
    weight: float = 1.0
    family_id: Optional[str] = None   # 版本家族
    metadata: Dict[str, Any] = field(default_factory=dict)
    valid_from: Optional[str] = None
    valid_until: Optional[str] = None
```

**设计要点**:
- 节点/边均带**时序有效性窗口**(`valid_from`/`valid_until`)
- `family_id` 支持边的版本家族,同族边可追踪演化
- 边 ID 通过 `_resolve_edge_identity()` 用 `uuid.uuid5(NAMESPACE_URL, payload)` 确定性生成,保证幂等
- SKOS 层次边通过 `is_skos_hierarchy_edge()` + `validate_skos_hierarchy()` 维护不变性

### 2.2 时序解析函数 `_parse_iso_dt()`

**代码路径**:`context_graph.py:203-231`

```python
def _parse_iso_dt(value: str) -> Optional[datetime]:
    """Parse an ISO datetime string into a tz-naive UTC datetime.
    支持 4 种格式(优先级):
        - Year-only:  "1990"  → "1990-01-01"
        - Date-only:  "1990-06-15"
        - Full ISO:   "1990-06-15T00:00:00+00:00" / "...Z"
        - Full ISO naive: "1990-06-15T00:00:00"
    失败返回 None,调用方视为"始终有效"(优雅降级)
    """
    if not value:
        return None
    s = str(value).strip()
    if _re.fullmatch(r"\d{4}", s):
        s = f"{s}-01-01"
    s = s.replace("Z", "+00:00")
    try:
        dt = datetime.fromisoformat(s)
        if dt.tzinfo is not None:
            dt = dt.astimezone(timezone.utc).replace(tzinfo=None)
        return dt
    except (ValueError, AttributeError) as e:
        logging.getLogger("semantica.context").warning(
            "Malformed temporal value %r — treating node as Always-Active. (%s)", value, e
        )
        return None
```

### 2.3 ContextGraph 内部索引结构

```python
self.nodes: Dict[str, ContextNode] = {}
self.edges: List[ContextEdge] = []
self._edge_index: Dict[str, ContextEdge] = {}

self._adjacency: Dict[str, List[ContextEdge]] = defaultdict(list)
self.node_type_index: Dict[str, Set[str]] = defaultdict(set)
self.edge_type_index: Dict[str, List[ContextEdge]] = defaultdict(list)

# 双删除语义
self._retractions: Dict[Tuple[str, str], Dict] = {}  # (entity_kind, entity_id) 可恢复
self._tombstones: Dict[Tuple[str, str], Dict] = {}   # 彻底清除
```

**双删除语义**:`retraction` 仅关闭实体的 `valid_until` 窗口(可恢复),`tombstone` 记录彻底清除。键用 `(entity_kind, entity_id)` 元组避免节点/边 ID 冲突。

### 2.4 决策记录 `record_decision()`

**代码路径**:`context_graph.py:4285-4432`

```python
def record_decision(
    self, category: str, scenario: str, reasoning: str, outcome: str,
    confidence: float, entities: Optional[List[str]] = None,
    decision_maker: Optional[str] = None, metadata: Optional[Dict[str, Any]] = None,
    valid_from=None, valid_until=None, **kwargs
) -> str:
    """443 行中含完整输入验证(category/scenario/reasoning/outcome 长度上限、类型校验)"""
    # 输入校验(4330-4378 行)
    if not isinstance(category, str) or not category.strip():
        raise ValueError("Category must be a non-empty string")
    if len(category.strip()) > 100:
        raise ValueError("Category must be 100 characters or less")
    # ... 更多校验

    decision_id = str(uuid.uuid4())
    timestamp = datetime.now().timestamp()
    decision = {
        "id": decision_id, "category": category, "scenario": scenario,
        "reasoning": reasoning, "outcome": outcome, "confidence": confidence,
        "entities": entities, "decision_maker": decision_maker,
        "timestamp": timestamp, "recorded_at": datetime.utcnow().isoformat(),
        "valid_from": normalized_valid_from, "valid_until": normalized_valid_until,
        "metadata": metadata or {},
    }
    self._add_decision_to_graph(decision)
    self._decisions[decision_id] = decision
    self._decision_index[category].add(decision_id)
    for entity in entities or []:
        self._entity_index[entity].add(decision_id)
    self._temporal_index.append((decision_id, timestamp))
    self._temporal_index.sort(key=lambda x: x[1], reverse=True)
    return decision_id
```

**设计要点**:严密输入验证,决策存入三个独立索引(category 索引、entity 反向索引、temporal 索引)支撑混合检索。

### 2.5 先例搜索(混合相似度算法)

**代码路径**:`context_graph.py:4434-4510`

```python
def find_precedents_by_scenario(
    self, scenario: str, category: Optional[str] = None, limit: int = 10,
    similarity_threshold: float = 0.5, include_superseded: bool = False, as_of=None, **filters
) -> List[Dict[str, Any]]:
    candidates = set()
    if category:
        candidates.update(self._decision_index.get(category, set()))
    else:
        candidates.update(self._decisions.keys())

    if "entities" in filters:
        entity_candidates = set()
        for entity in filters["entities"]:
            entity_candidates.update(self._entity_index.get(entity, set()))
        candidates = candidates.intersection(entity_candidates)

    precedents = []
    for decision_id in candidates:
        decision = self._decisions[decision_id]
        if not self._decision_matches_temporal_filters(...):
            continue
        content_sim = self._calculate_decision_content_similarity(scenario, decision)
        structural_sim = 0.0
        if self.config.get("advanced_analytics"):
            structural_sim = self._calculate_structural_similarity_for_decision(decision_id, scenario)
        combined_sim = 0.7 * content_sim + 0.3 * structural_sim
        if combined_sim >= similarity_threshold:
            precedents.append({
                "decision": decision, "similarity": combined_sim,
                "content_similarity": content_sim, "structural_similarity": structural_sim,
            })
    precedents.sort(key=lambda x: x["similarity"], reverse=True)
    return precedents[:limit]
```

**关键公式**:`combined_sim = 0.7 × content_sim + 0.3 × structural_sim`

### 2.6 双删除语义:retract vs purge

**代码路径**:`context_graph.py:2644-2911`

```python
def retract_node(self, node_id, reason=None, at=None, cascade=True) -> bool:
    """Retract: 不再活跃但仍可见于历史;关闭 valid_until 窗口"""
    at_iso = _normalize_temporal_input(at) or datetime.now(timezone.utc).isoformat()
    with self._lock:
        node = self.nodes.get(node_id)
        if node is None or ("node", node_id) in self._retractions:
            return False
        node.valid_until = _closing_valid_until(node.valid_until, at_iso)
        record = {"entity_id": node_id, "retracted_at": at_iso, "reason": reason}
        self._retractions[("node", node_id)] = record
        # 级联撤回所有 incident 边
        for edge in self._incident_edges(node_id):
            edge.valid_until = _closing_valid_until(edge.valid_until, at_iso)
            self._retractions[("edge", edge.edge_id)] = {...}
        self._emit_mutation("UPDATE_NODE", node_id, node_payload)
```

**关键设计**:
- **retraction**:仅关闭 `valid_until`,保留实体(可解释性)
- **tombstone**:彻底清除,仅保留 `{entity_id, purged_at, reason}` 记录
- **级联一致性**:retract 节点同时 retract 所有 incident 边

### 2.7 策略引擎

**`PolicyEngine`**(`policy_engine.py`,1044 行):
- `add_policy()`/`update_policy()` — 策略版本化
- `check_compliance()` — 规则评估(`min_X`/`max_X`/`required_X`/`allowed_outcomes` 前缀约定)
- `analyze_policy_impact()` — What-if 影响分析
- `record_exception()`/`record_policy_application()` — 策略例外与合规审计

### 2.8 ContextGraph 数据模型图

```
┌─────────────────────────────────────────────────────────────────┐
│                     ContextGraph 核心结构                         │
│                                                                 │
│  ContextNode (node_id, node_type, content,                      │
│              metadata, properties, valid_from, valid_until)      │
│       │                                                         │
│       │ 1:N                                                     │
│       ▼                                                         │
│  ContextEdge (source_id, target_id, edge_type,                  │
│               edge_id, weight, family_id,                       │
│               metadata, valid_from, valid_until)                 │
│                                                                 │
│  内部索引:                                                       │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────────────┐    │
│  │ _adjacency   │ │ node_type_   │ │ edge_type_index      │    │
│  └──────────────┘ └──────────────┘ └──────────────────────┘    │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────────────┐    │
│  │ _decisions   │ │ _decision_   │ │ _entity_index        │    │
│  └──────────────┘ └──────────────┘ └──────────────────────┘    │
│  ┌──────────────┐ ┌──────────────┐                              │
│  │ _retractions │ │ _tombstones  │  (双删除语义)               │
│  └──────────────┘ └──────────────┘                              │
└─────────────────────────────────────────────────────────────────┘
```

---

## 三、决策因果链

### 3.1 因果边双词汇系统

**代码路径**:`context_graph.py:500-517`

```python
#: 本模块使用过去时, analyzer 使用现在时; 存储前归一化到过去时
_CAUSAL_EDGE_TYPES = ("CAUSED", "INFLUENCED", "PRECEDENT_FOR")

_CAUSAL_EDGE_ALIASES = {
    "CAUSES": "CAUSED", "CAUSED": "CAUSED",
    "INFLUENCES": "INFLUENCED", "INFLUENCED": "INFLUENCED",
    "PRECEDES": "PRECEDENT_FOR", "PRECEDENT_FOR": "PRECEDENT_FOR",
}
_CAUSAL_TRAVERSAL_TYPES = frozenset(_CAUSAL_EDGE_ALIASES) | {
    "LEADS_TO", "LEAD_TO", "SUPPORTS", "SUPPORT",
}
```

### 3.2 `add_causal_relationship()` 因果关系记录

**代码路径**:`context_graph.py:3836-3880`

```python
def add_causal_relationship(
    self, source_decision_id: str, target_decision_id: str, relationship_type: str
) -> None:
    # 词汇归一化
    relationship_type = _CAUSAL_EDGE_ALIASES.get(relationship_type.strip().upper())
    if relationship_type is None:
        raise ValueError(f"Relationship type must be one of: {_CAUSAL_EDGE_TYPES}")
    # 节点存在性 + 类型校验
    if source_decision_id not in self.nodes or target_decision_id not in self.nodes:
        return
    source_node = self.nodes[source_decision_id]
    target_node = self.nodes[target_decision_id]
    if source_node.node_type.lower() != "decision" or target_node.node_type.lower() != "decision":
        return
    edge = ContextEdge(
        source_id=source_decision_id, target_id=target_decision_id,
        edge_type=relationship_type, weight=1.0,
        metadata={"recorded_at": datetime.utcnow().isoformat()},
    )
    self._add_internal_edge(edge)
```

### 3.3 `get_causal_chain()` BFS 因果链追踪

**代码路径**:`context_graph.py:3882-3964`

```python
def get_causal_chain(
    self, decision_id: str, direction: str = "upstream", max_depth: int = 10
) -> List["Decision"]:
    # BFS 遍历
    visited = set()
    queue = deque([(decision_id, 0)])
    decisions = []
    while queue:
        current_id, depth = queue.popleft()
        if current_id in visited or depth > max_depth:
            continue
        visited.add(current_id)
        for edge in self.edges:
            if direction == "upstream":
                if edge.target_id == current_id and edge.edge_type.upper() in _CAUSAL_TRAVERSAL_TYPES:
                    if edge.source_id not in visited and depth < max_depth:
                        queue.append((edge.source_id, depth + 1))
            else:
                if edge.source_id == current_id and edge.edge_type.upper() in _CAUSAL_TRAVERSAL_TYPES:
                    if edge.target_id not in visited and depth < max_depth:
                        queue.append((edge.target_id, depth + 1))
    return decisions
```

### 3.4 `trace_decision_causality()` 复杂因果链递归追踪

**代码路径**:`context_graph.py:4660-4820`

```python
def trace_decision_causality(
    self, decision_id: str, max_depth: int = 5, max_chains: Optional[int] = 10000
) -> List[Dict[str, Any]]:
    """DFS 递归追踪, 同时支持显式因果边和隐式因果(共享实体+时序)"""
    # 构建反向索引(一次构建, 避免重复扫描边列表)
    incoming_causal_edges = defaultdict(list)
    for edge_type, edges in self.edge_type_index.items():
        if edge_type.upper() not in _CAUSAL_TRAVERSAL_TYPES:
            continue
        for edge in edges:
            if edge.source_id in self._decisions:
                incoming_causal_edges[edge.target_id].append(edge)

    def trace_recursive(current_id, depth, path, path_ids):
        # 关键: 环检测是 per-path 而非全局
        if truncated or depth >= max_depth or current_id in path_ids:
            return
        path_ids = path_ids | {current_id}
        # 1. 显式因果(优先)
        explicit_causes = incoming_causal_edges.get(current_id, [])
        for edge in explicit_causes:
            # ...记录链 + 递归
        # 2. 隐式因果(共享实体 + 时序先后)
        for entity in current_decision["entities"]:
            for other_decision_id in self._entity_index.get(entity, set()):
                if other_decision["timestamp"] < current_decision["timestamp"]:
                    # 添加为隐式影响边
                    ...
```

**设计要点**:
- **per-path 环检测**:同一决策可通过不同分支重新访问
- **显式因果优先于隐式因果**
- **链数上限 + truncated 标记**:防止密集图组合爆炸
- **反向索引一次构建**:避免重复扫描

### 3.5 决策记录与因果追踪流程图

```
用户决策输入
    │
    ▼
record_decision(category, scenario, reasoning, outcome, confidence, entities)
    │
    ├─→ 输入验证(443 行验证逻辑)
    ├─→ _add_decision_to_graph() → ContextNode(type="decision")
    ├─→ _decisions[decision_id] = decision
    ├─→ _decision_index[category].add(decision_id)
    ├─→ _entity_index[entity].add(decision_id)
    └─→ _temporal_index.append((decision_id, timestamp))
    │
    ▼
add_causal_relationship(source_id, target_id, relationship_type)
    │
    ├─→ 词汇归一化(CAUSES→CAUSED)
    ├─→ 验证两节点存在且为 decision 类型
    └─→ _add_internal_edge(ContextEdge(type=CAUSED/INFLUENCED/PRECEDENT_FOR))
    │
    ▼
trace_decision_causality(decision_id, max_depth=5)
    │
    ├─→ 构建 incoming_causal_edges 反向索引
    ├─→ 递归 DFS 追踪
    │   ├─→ 显式因果边优先
    │   ├─→ 隐式因果(共享实体 + 时间顺序)
    │   └─→ 环检测(per-path path_ids)
    └─→ 返回因果链报告
```

---

## 四、Rete 网络

**代码路径**:`semantica/reasoning/rete_engine.py`(642 行)

### 4.1 节点类层次

```python
@dataclass
class Token:
    """在 Rete 网络中流动的部分匹配"""
    facts: List[Fact] = field(default_factory=list)
    bindings: Dict[str, str] = field(default_factory=dict)

@dataclass
class Match:
    rule: Rule
    facts: List[Fact]
    bindings: Dict[str, Any] = field(default_factory=dict)
    confidence: float = 1.0

class ReteNode:
    def __init__(self, node_id: str):
        self.node_id = node_id
        self.children: List[ReteNode] = []
```

### 4.2 AlphaNode(单条件匹配)

```python
class AlphaNode(ReteNode):
    def __init__(self, node_id: str, condition: Any):
        super().__init__(node_id)
        self.condition = condition
        self.tokens: List[Token] = []
        # 关键: 预编译正则一次, 避免每次 fact 到达重新编译
        pattern = condition if isinstance(condition, str) else str(condition)
        self._compiled: Optional[re.Pattern] = None
        try:
            self._compiled = re.compile(_build_condition_regex(pattern))
        except re.error as e:
            logger.warning("AlphaNode %r failed to compile...", node_id, pattern, e)

    def add_fact(self, fact: Fact) -> Optional[Token]:
        bindings = self._matches(fact)
        if bindings is not None:
            token = Token(facts=[fact], bindings=dict(bindings))
            self.tokens.append(token)
            return token
        return None
```

**关键优化**:**预编译正则**——Rete 会用同一 AlphaNode 评估大量 fact,每次重新编译代价高昂。

### 4.3 BetaNode(连接操作)

```python
class BetaNode(ReteNode):
    def __init__(self, node_id: str, left: ReteNode, right: ReteNode):
        super().__init__(node_id)
        self.left = left
        self.right = right
        self.left_tokens: List[Token] = []   # 双侧 token 记忆
        self.right_tokens: List[Token] = []

    def join(self, left_token: Token, right_token: Token) -> Optional[Token]:
        """Join: 绑定冲突检测 + facts 合并"""
        merged = dict(left_token.bindings)
        for var, value in right_token.bindings.items():
            if var in merged and merged[var] != value:
                return None  # Binding conflict
            merged[var] = value
        return Token(
            facts=list(left_token.facts) + list(right_token.facts),
            bindings=merged,
        )
```

**关键设计**:
- **双侧 token 记忆**:经典 Rete 增量连接
- **Binding 冲突检测**:同一变量在不同 token 中绑定不同值时返回 None
- **facts 列表追加**:按条件顺序排列,便于解释生成

### 4.4 TerminalNode(规则激活)

```python
class TerminalNode(ReteNode):
    def __init__(self, node_id: str, rule: Rule):
        super().__init__(node_id)
        self.rule = rule
        self.activations: List[Match] = []

    def activate(self, match: Match) -> None:
        self.activations.append(match)
```

### 4.5 正则合一算法 `_build_condition_regex()`

**代码路径**:`rete_engine.py:47-85`

```python
def _build_condition_regex(
    pattern: str, initial_bindings: Optional[Dict[str, str]] = None
) -> str:
    """构建锚定正则:
    - 已绑定变量 → 字面量匹配
    - 重复变量 → 反向引用 (?P=name)
    - 新变量 → 命名捕获 (?P<name>.+?)
    """
    bindings = initial_bindings or {}
    segments = re.split(r"(\?\w+)", pattern)
    seen_vars: Set[str] = set()
    p_regex = ""
    for seg in segments:
        if seg.startswith("?"):
            var_name = seg[1:]
            if var_name in bindings:
                p_regex += re.escape(bindings[var_name])
            elif var_name in seen_vars:
                p_regex += f"(?P={var_name})"
            else:
                p_regex += f"(?P<{var_name}>.+?)"
                seen_vars.add(var_name)
        else:
            p_regex += re.escape(seg)
    return f"^{p_regex}$"
```

**示例**:
- `"Person(?x)"` → `^Person(?P<x>.+?)$`
- `"knows(?x, ?x)"` → `^knows(?P<x>.+?),\s*(?P=x)$`(反向引用)

### 4.6 `unify_condition()` 合一函数

**代码路径**:`rete_engine.py:88-151`

```python
def unify_condition(
    condition: Any, fact: Fact, initial_bindings: Optional[Dict[str, str]] = None
) -> Optional[Dict[str, str]]:
    bindings = dict(initial_bindings or {})
    pattern = condition if isinstance(condition, str) else str(condition)
    fact_str = str(fact)
    p_regex = _build_condition_regex(pattern, bindings)
    try:
        match = re.match(p_regex, fact_str)
    except re.error as e:
        return None
    if not match:
        return None
    for var, value in match.groupdict().items():
        if var in bindings and bindings[var] != value:
            return None  # Binding conflict
        bindings[var] = value
    return bindings
```

### 4.7 Rete 网络构建

```python
def _add_rule_to_network(self, rule: Rule) -> None:
    # 1. 每个条件创建一个 AlphaNode
    alpha_nodes = []
    for condition in rule.conditions:
        node_id = f"alpha_{self.node_counter}"
        self.node_counter += 1
        alpha_node = AlphaNode(node_id, condition)
        alpha_nodes.append(alpha_node)
        self.network[node_id] = alpha_node

    # 2. 多于 1 个 AlphaNode 时, 链式创建 BetaNode 连接
    if len(alpha_nodes) > 1:
        current = alpha_nodes[0]
        for i in range(1, len(alpha_nodes)):
            node_id = f"beta_{self.node_counter}"
            self.node_counter += 1
            beta_node = BetaNode(node_id, current, alpha_nodes[i])
            self.network[node_id] = beta_node
            current.children.append(beta_node)
            alpha_nodes[i].children.append(beta_node)
            current = beta_node
        final_node = current
    else:
        final_node = alpha_nodes[0] if alpha_nodes else None

    # 3. 末尾创建 TerminalNode
    if final_node:
        node_id = f"terminal_{self.node_counter}"
        self.node_counter += 1
        terminal_node = TerminalNode(node_id, rule)
        final_node.children.append(terminal_node)
        self.network[node_id] = terminal_node
```

### 4.8 Token 传播

```python
def _propagate_to_beta(self, beta: BetaNode, source: ReteNode, token: Token) -> None:
    if source is beta.left:
        beta.left_tokens.append(token)
        for right_token in list(beta.right_tokens):
            merged = beta.join(token, right_token)
            if merged is not None:
                self._propagate_token(beta, merged)
    elif source is beta.right:
        beta.right_tokens.append(token)
        for left_token in list(beta.left_tokens):
            merged = beta.join(left_token, token)
            if merged is not None:
                self._propagate_token(beta, merged)
```

### 4.9 规则执行(带去重)

```python
def execute_matches(self, matches=None) -> List[Any]:
    matches = matches or self.match_patterns()
    results = []
    for match in matches:
        results.append(match.rule.conclusion)
        if self.reasoner is not None and (match.rule.actions or match.rule.handler is not None):
            # 关键: _executed_activations 去重
            activation_key = _make_activation_key(
                match.rule.rule_id, match.bindings,
                [(fact.fact_id, fact.predicate, fact.arguments) for fact in match.facts],
            )
            if activation_key not in self._executed_activations:
                self._executed_activations.add(activation_key)
                self.reasoner._fire_actions(match.rule, match.bindings)
    return results
```

### 4.10 Rete 网络结构图

```
┌─────────────────────────────────────────────────────────────────┐
│                    Rete 网络结构                                  │
│                                                                 │
│  Fact → AlphaNode(条件1) ─┐                                     │
│                           BetaNode(连接) ─→ TerminalNode(规则1) │
│  Fact → AlphaNode(条件2) ─┘         │                          │
│                                      │                          │
│  Fact → AlphaNode(条件3) ─→ BetaNode ┘                          │
│                                                                 │
│  Token(facts, bindings) 在网络中流动                             │
│  BetaNode.join() 检查绑定冲突                                    │
└─────────────────────────────────────────────────────────────────┘
```

---

## 五、Datalog 半朴素不动点

**代码路径**:`semantica/reasoning/datalog_reasoner.py`(431 行)

### 5.1 数据结构

```python
@dataclass(frozen=True)
class DatalogFact:
    """事实不可变,可哈希,自然用于 Set 去重"""
    predicate: str
    args: Tuple[str, ...]

class BodyAtom(NamedTuple):
    predicate: str
    args: Tuple[str, ...]

@dataclass
class DatalogRule:
    head_predicate: str
    head_args: Tuple[str, ...]
    body: List[BodyAtom]
```

### 5.2 变量识别

```python
def _is_variable(self, term: str) -> bool:
    """变量大写开头,常量小写开头(Datalog 经典语法)"""
    return bool(term and term[0].isupper())
```

### 5.3 合一算法(优化版)

```python
def _unify(
    self, pattern_args: Tuple[str, ...], fact_args: Tuple[str, ...], bindings: Dict[str, str]
) -> Optional[Dict[str, str]]:
    """优化: 避免不必要的字典分配"""
    if len(pattern_args) != len(fact_args):
        return None
    new_additions = {}  # 暂存新绑定
    for p_arg, f_arg in zip(pattern_args, fact_args):
        if self._is_variable(p_arg):
            if p_arg in bindings:
                if bindings[p_arg] != f_arg:
                    return None
            elif p_arg in new_additions:
                if new_additions[p_arg] != f_arg:
                    return None
            else:
                new_additions[p_arg] = f_arg
        else:
            if p_arg != f_arg:
                return None
    # 仅在新绑定时分配新字典
    if new_additions:
        return {**bindings, **new_additions}
    return bindings  # 无新绑定, 返回原 dict
```

### 5.4 事实/规则解析

```python
def _parse_fact_string(self, s: str) -> DatalogFact:
    """Parse 'predicate(arg1, arg2)' into a DatalogFact"""
    match = re.match(r'^\s*([a-zA-Z0-9_]+)\s*\(\s*([^)]+)\s*\)\s*\.?\s*$', s.strip())
    if not match:
        raise ValueError(f"Invalid fact syntax: {s}")
    predicate = match.group(1)
    args = tuple(arg.strip() for arg in match.group(2).split(','))
    # 事实必须是常量, 不能含变量
    for arg in args:
        if arg[0].isupper():
            raise ValueError(f"Facts must be constants only. Found variable '{arg}' in {s}")
    return DatalogFact(predicate, args)

def _parse_rule_string(self, s: str) -> DatalogRule:
    """Parse 'head(X, Y) :- body1(X, Z), body2(Z, Y).' into a DatalogRule"""
    head_str, body_str = s.split(":-", 1)
    head_pred = ...
    body = []
    atom_matches = re.findall(r'([a-zA-Z0-9_]+)\s*\(\s*([^)]+)\s*\)', body_str)
    for pred, args_str in atom_matches:
        args = tuple(arg.strip() for arg in args_str.split(','))
        body.append(BodyAtom(pred, args))
    return DatalogRule(head_pred, head_args, body)
```

### 5.5 半朴素不动点求值 `derive_all()`

```python
def derive_all(self) -> List[str]:
    """自底向上半朴素求值直到不动点"""
    if self._derived:
        return [f"{f.predicate}({', '.join(f.args)})" for f in self._all_facts]

    iteration = 0
    newly_derived_count = 0
    # 初始 delta = 所有事实
    self._delta_new = self._all_facts.copy()

    while self._delta_new:
        iteration += 1
        self._delta_old = self._delta_new
        self._delta_new = set()
        # 按 predicate 建索引
        delta_index = defaultdict(set)
        for f in self._delta_old:
            delta_index[f.predicate].add(f)

        for rule in self._rules:
            new_facts = self._apply_rule(rule, delta_index)
            for fact in new_facts:
                if fact not in self._all_facts:
                    self._delta_new.add(fact)
                    self._all_facts.add(fact)
                    self._fact_index[fact.predicate].add(fact)
                    newly_derived_count += 1

    self._derived = True
    return [f"{f.predicate}({', '.join(f.args)})" for f in self._all_facts]
```

### 5.6 `_apply_rule()` 半朴素策略(核心优化)

```python
def _apply_rule(
    self, rule: DatalogRule, delta_index: Optional[Dict[str, Set[DatalogFact]]] = None
) -> Set[DatalogFact]:
    results = set()
    if not rule.body:
        fact = self._instantiate_fact(rule.head_predicate, rule.head_args, {})
        if fact:
            results.add(fact)
        return results

    is_seminaive = delta_index is not None
    # 关键: 对每个 body 位置依次作为"增量位置"
    evaluation_paths = range(len(rule.body)) if is_seminaive else [0]

    for delta_index_pos in evaluation_paths:
        bindings_list = [{}]
        for i, atom in enumerate(rule.body):
            new_bindings_list = []
            # 增量位置用 delta, 其他位置用全量
            if is_seminaive and i == delta_index_pos:
                candidate_facts = delta_index.get(atom.predicate, set())
            else:
                candidate_facts = self._fact_index.get(atom.predicate, set())

            for bindings in bindings_list:
                for fact in candidate_facts:
                    merged_bindings = self._unify(atom.args, fact.args, bindings)
                    if merged_bindings is not None:
                        new_bindings_list.append(merged_bindings)
            bindings_list = new_bindings_list
            if not bindings_list:
                break  # 早退

        for final_bindings in bindings_list:
            head_fact = self._instantiate_fact(rule.head_predicate, rule.head_args, final_bindings)
            if head_fact:
                results.add(head_fact)
    return results
```

**半朴素求值策略**:
- 每轮只考虑**至少一个新事实**参与推导
- 假设规则 body 有 N 个原子,依次将每个位置作为"增量位置",其他位置使用全量 fact 索引
- 保证:每轮至少有一个体原子使用新事实,避免重复推导;同时保证终止性

### 5.7 `query()` 查询接口

```python
def query(self, pattern: str, bindings: dict = None) -> List[dict]:
    """查询派生事实集,自动运行 derive_all()"""
    if self._rules and not self._derived:
        self.derive_all()
    # 解析 pattern, 处理变量(?x 和 X 等价)
    # 将已绑定变量替换为字面量
    # 对候选 fact 执行 _unify
    # 返回结果行列表
```

---

## 六、SPARQL

**代码路径**:`semantica/reasoning/sparql_reasoner.py`,基于 RDFLib 的 SPARQL 1.1 实现。

### 6.1 SPARQL 集成

- 基于 RDFLib 的 SPARQL 1.1 完整支持
- 与 Oxigraph 嵌入式存储深度集成
- 支持命名图、事务批量加载
- `sparql_escaping.py` 提供 SPARQL 注入防护

### 6.2 推理能力

- 支持 SELECT/CONSTRUCT/ASK/DESCRIBE 查询
- 与 Datalog 引擎互补:Datalog 用于自定义规则推理,SPARQL 用于标准 RDF 查询
- 支持 OWL/RDFS 推理机集成

---

## 七、W3C PROV-O 溯源

### 7.1 `ProvenanceEntry` 数据类

**代码路径**:`semantica/provenance/schemas.py:36-200`

```python
@dataclass
class ProvenanceEntry:
    """W3C PROV-O compliant provenance entry (40+ 字段)"""
    entity_id: str
    entity_type: str
    activity_id: str
    agent_id: str = "semantica"
    agent_type: str = "software_agent"  # "person" | "software_agent" | "organization"
    is_automated: bool = True
    role: Optional[str] = None
    source_document: str = ""
    source_location: Optional[str] = None
    source_quote: Optional[str] = None
    timestamp: str = field(default_factory=lambda: utc_now_iso())
    first_seen: Optional[str] = None
    last_updated: Optional[str] = None
    confidence: float = 1.0
    checksum: Optional[str] = None
    sequence_id: Optional[int] = None
    previous_checksum: Optional[str] = None
    parent_entity_id: Optional[str] = None
    used_entities: List[str] = field(default_factory=list)
    previous_version_id: Optional[str] = None
    derived_from_id: Optional[str] = None
    activity_started_at_time: Optional[str] = None
    activity_ended_at_time: Optional[str] = None
    acted_on_behalf_of: Optional[str] = None
    informed_by_activities: List[str] = field(default_factory=list)
    valid_from: Optional[str] = None
    valid_until: Optional[str] = None
    revision_type: Optional[str] = None
    supersedes: Optional[str] = None
    bundle_id: Optional[str] = None
    invalidated: bool = False
    invalidated_at_time: Optional[str] = None
    invalidated_by: Optional[str] = None
    invalidation_reason: Optional[str] = None
```

**W3C PROV-O 映射表**:

| 字段 | PROV-O 映射 |
|------|-------------|
| `entity_id` | `prov:Entity` |
| `activity_id` | `prov:Activity` |
| `agent_id`/`agent_type` | `prov:Agent` (Person\|SoftwareAgent\|Organization) |
| `role` | `prov:hadRole` |
| `parent_entity_id` | `prov:wasDerivedFrom` (legacy) |
| `derived_from_id` | `prov:wasDerivedFrom` (true cross-source) |
| `previous_version_id` | prior version of same fact |
| `used_entities` | `prov:used` |
| `timestamp` | `prov:generatedAtTime` |
| `invalidated`/`invalidated_at_time` | `prov:Invalidation` |
| `activity_started_at_time`/`activity_ended_at_time` | `prov:startedAtTime`/`prov:endedAtTime` |
| `acted_on_behalf_of` | `prov:actedOnBehalfOf` |
| `bundle_id` | `prov:Collection`/`prov:Bundle` |

### 7.2 SHA-256 哈希链 `compute_checksum()`

**代码路径**:`semantica/provenance/integrity.py:27-116`

```python
def compute_checksum(entry: Any) -> str:
    """SHA-256 校验和, 故意排除 entity_id(支持版本归档重命名)"""
    if isinstance(entry, dict):
        used_entities = entry.get("used_entities") or []
        data = (
            f"{entry.get('entity_type') or ''}"
            f"{entry.get('activity_id') or ''}"
            f"{entry.get('agent_id') or ''}"
            f"{entry.get('agent_type') or ''}"
            f"{entry.get('source_document') or ''}"
            f"{entry.get('timestamp') or ''}"
            f"{entry.get('confidence') if entry.get('confidence') is not None else 1.0}"
            f"{entry.get('parent_entity_id') or ''}"
            f"{entry.get('previous_version_id') or ''}"
            f"{entry.get('derived_from_id') or ''}"
            f"{','.join(used_entities)}"
            f"{entry.get('previous_checksum') or ''}"
            f"{bool(entry.get('invalidated'))}"
            f"{entry.get('invalidated_at_time') or ''}"
            f"{entry.get('invalidated_by') or ''}"
            f"{entry.get('invalidation_reason') or ''}"
        )
    return hashlib.sha256(data.encode('utf-8')).hexdigest()
```

**哈希字段设计要点**:

| 包含字段 | 原因 |
|---------|------|
| `entity_type`, `activity_id`, `agent_id`/`agent_type` | 身份字段 |
| `source_document`, `source_location`, `source_quote` | 审计级源追踪 |
| `timestamp`, `confidence` | 时序与质量 |
| `parent_entity_id`, `previous_version_id`, `derived_from_id` | lineage 字段 |
| `used_entities` | 派生源 |
| `previous_checksum` | 哈希链 |
| `invalidated`/`invalidated_at_time` | 失效追踪 |
| **故意排除 `entity_id`** | 版本归档重命名(`X` → `X:v:...`)不破坏哈希链 |

### 7.3 `track_entity()` 原子事务与版本归档

**代码路径**:`semantica/provenance/manager.py:268-420`

```python
def track_entity(
    self, entity_id: str, source: str, metadata=None, _conn=None, **kwargs
) -> Optional[ProvenanceEntry]:
    """原子事务(BEGIN IMMEDIATE): 所有 retrieve/store 共享单事务"""
    try:
        with self._get_or_create_transaction(_conn) as conn:
            existing = self.storage._retrieve_with_conn(conn, entity_id)
            # 跨源派生父节点推断
            if not parent_id and source and isinstance(source, str):
                try:
                    source_entity = self.storage._retrieve_with_conn(conn, source)
                    if source_entity:
                        parent_id = source
                except Exception:
                    pass

            if existing:
                # 版本归档: 复制旧条目, 主键重命名为 f"{entity_id}:v:{last_updated}"
                history_entry = copy.deepcopy(existing)
                base_history_id = f"{entity_id}:v:{existing.last_updated}"
                history_id = base_history_id
                counter = 1
                while self.storage._retrieve_with_conn(conn, history_id):
                    history_id = f"{base_history_id}:{counter}"
                    counter += 1
                history_entry.entity_id = history_id
                # 纯重命名, checksum/sequence_id/previous_checksum 不变
                self.storage._store_with_conn(conn, history_entry)

            entry = ProvenanceEntry(entity_id=entity_id, ...)
            # 双版本语义
            entry.previous_version_id = archived_history_id
            if explicit_parent_supplied:
                entry.derived_from_id = parent_id
            self._save_entry(entry, _conn=conn, _raise_on_error=True)
    except Exception as e:
        # 事务回滚; 已有 entry 时返回 deepcopy(existing)
        if existing is not None:
            return copy.deepcopy(existing)
        return None
    return entry
```

**关键设计**:
- **BEGIN IMMEDIATE 事务**:写锁避免并发插入产生间隙
- **归档时仅重命名 entity_id**:checksum/sequence_id/previous_checksum 完全保留
- **双版本语义独立**:`previous_version_id`(修正旧版本) vs `derived_from_id`(跨源派生)

### 7.4 `verify_chain()` 哈希链验证

**代码路径**:`semantica/provenance/manager.py:1450-1520`

```python
def verify_chain(self) -> Dict[str, Any]:
    """三重检测: checksum_mismatch / sequence_gap / checksum_break"""
    entries = sorted(
        (e for e in self.storage.retrieve_all() if e.sequence_id is not None),
        key=lambda e: e.sequence_id,
    )
    broken_links = []
    expected_previous = None
    expected_sequence = None

    for entry in entries:
        if not verify_checksum(entry):
            broken_links.append({"entity_id": entry.entity_id, "reason": "checksum_mismatch"})
        else:
            sequence_gap = (
                expected_sequence is not None
                and entry.sequence_id != expected_sequence + 1
            )
            checksum_break = entry.previous_checksum != expected_previous
            if sequence_gap or checksum_break:
                broken_links.append({"entity_id": entry.entity_id, "reason": "chain_break", ...})
        # 即使 entry 被标记为 broken, 仍然推进 expected_*
        expected_previous = entry.checksum
        expected_sequence = entry.sequence_id

    return {"valid": len(broken_links) == 0, "total_entries": len(entries), "broken_links": broken_links}
```

**三重检测**:
1. **checksum_mismatch**:单条 entry 内容被篡改
2. **sequence_gap**:sequence_id 不连续(有行被硬删除)
3. **checksum_break**:previous_checksum 与前一行的 checksum 不匹配

### 7.5 Provenance 哈希链结构图

```
┌─────────────────────────────────────────────────────────────────┐
│                   Provenance 哈希链结构                           │
│                                                                 │
│  Entry_1 → Entry_2 → Entry_3 → ... → Entry_n                   │
│  (seq=1)   (seq=2)   (seq=3)          (seq=n)                  │
│  prev=None  prev=H1    prev=H2         prev=Hn-1               │
│  checksum=H1 checksum=H2 checksum=H3    checksum=Hn             │
│                                                                 │
│  H_i = SHA256(entity_type + activity_id + agent_id +            │
│              source_document + timestamp + confidence +          │
│              parent_entity_id + previous_checksum + ...)         │
│  (entity_id 故意排除, 支持版本归档重命名)                         │
└─────────────────────────────────────────────────────────────────┘
```

---

## 八、BiTemporal 双时序模型

### 8.1 BiTemporalFact 数据结构

**代码路径**:`semantica/kg/temporal_model.py`(175 行)

```python
@dataclass
class BiTemporalFact:
    valid_from: Optional[datetime]              # 业务有效性开始
    valid_until: Optional[datetime | TemporalBound]  # 业务有效性结束
    recorded_at: datetime = field(default_factory=_default_recorded_at)  # 记录时间
    superseded_at: datetime | TemporalBound = TemporalBound.OPEN        # 取代时间
```

### 8.2 四时间维度语义

| 时间维度 | 字段 | 含义 |
|---------|------|------|
| **业务时间** | `valid_from`/`valid_until` | 事实本身何时有效 |
| **记录时间** | `recorded_at` | 系统何时记录 |
| **取代时间** | `superseded_at` | 何时被新事实替代 |
| **开放区间** | `TemporalBound.OPEN` | 哨兵值表示无限 |

### 8.3 配套函数

```python
def parse_temporal_value(value) -> datetime:
    """统一处理 datetime/epoch/ISO 字符串, 始终转为 UTC"""

def serialize_temporal_value(dt) -> str:
    """序列化为 ISO 8601 字符串"""
```

### 8.4 时序查询

`TemporalGraphQuery`(`temporal_query.py`,71K)支持:
- 时间点查询(as_of)
- 时间范围查询(valid_from/until 窗口)
- 时序模式检测
- 图演化分析
- 时序路径查找

### 8.5 时间旅行

`CausalChainAnalyzer.trace_at_time()` 实现**事务时间旅行**,仅使用 `recorded_at <= cutoff` 的边,支持查询特定时间点的上下文状态。

---

## 九、冲突检测 7 种策略

### 9.1 冲突类型

**代码路径**:`semantica/conflicts/conflict_detector.py:68-93`

```python
class ConflictType(str, Enum):
    VALUE_CONFLICT = "value_conflict"           # 值冲突
    TYPE_CONFLICT = "type_conflict"              # 类型冲突
    RELATIONSHIP_CONFLICT = "relationship_conflict"  # 关系冲突
    TEMPORAL_CONFLICT = "temporal_conflict"      # 时序冲突
    LOGICAL_CONFLICT = "logical_conflict"        # 逻辑冲突

@dataclass
class Conflict:
    conflict_id: str
    conflict_type: ConflictType
    entity_id: Optional[str] = None
    property_name: Optional[str] = None
    relationship_id: Optional[str] = None
    conflicting_values: List[Any] = field(default_factory=list)
    sources: List[Dict[str, Any]] = field(default_factory=list)
    confidence: float = 1.0
    severity: str = "medium"  # low, medium, high, critical
    recommended_action: Optional[str] = None
    metadata: Dict[str, Any] = field(default_factory=dict)
```

### 9.2 七种解决策略

**代码路径**:`semantica/conflicts/conflict_resolver.py:108-148`

```python
class ResolutionStrategy(str, Enum):
    VOTING = "voting"                        # 多数投票
    CREDIBILITY_WEIGHTED = "credibility_weighted"  # 可信度加权
    MOST_RECENT = "most_recent"              # 最新值
    FIRST_SEEN = "first_seen"                # 最早值
    HIGHEST_CONFIDENCE = "highest_confidence"  # 最高置信度
    MANUAL_REVIEW = "manual_review"          # 人工审核
    EXPERT_REVIEW = "expert_review"          # 专家审核
```

### 9.3 值冲突检测

```python
def detect_value_conflicts(
    self, entities, property_name: str, entity_type: Optional[str] = None
) -> List[Conflict]:
    # 1. 按 entity_id 分组(同实体来自不同源)
    entity_groups: Dict[str, List[Dict[str, Any]]] = {}
    for entity in entities:
        entity_id = entity.get("id") or entity.get("entity_id")
        if entity_id not in entity_groups:
            entity_groups[entity_id] = []
        entity_groups[entity_id].append(entity)

    conflicts = []
    for entity_id, entity_list in entity_groups.items():
        if len(entity_list) < 2:
            continue
        values, sources = [], []
        for entity in entity_list:
            if property_name in entity:
                values.append(entity[property_name])
                sources.append({...})
        unique_values = list(set(str(v) for v in values))
        if len(unique_values) <= 1:
            continue
        conflict = Conflict(
            conflict_id=str(uuid.uuid4()),
            conflict_type=ConflictType.VALUE_CONFLICT,
            entity_id=entity_id, property_name=property_name,
            conflicting_values=values, sources=sources,
            severity=self._calculate_severity(property_name, values),
            recommended_action=self._recommend_action(property_name, values),
        )
        conflicts.append(conflict)
    return conflicts
```

### 9.4 严重度计算

```python
def _calculate_severity(self, property_name: str, values: List[Any]) -> str:
    # 1. 关键字段 → critical
    critical_fields = ["id", "name", "type", "founded_year", "revenue"]
    if property_name.lower() in critical_fields:
        return "critical"
    # 2. 数值差异大 → high
    try:
        numeric_values = [float(v) for v in values if v is not None]
        if numeric_values:
            value_range = max(numeric_values) - min(numeric_values)
            if value_range > 1000:
                return "high"
    except (ValueError, TypeError):
        pass
    return "medium"
```

### 9.5 投票算法

```python
def _resolve_by_voting(self, conflict: Conflict) -> ResolutionResult:
    """多数投票: 票数最多的值胜出"""
    value_counts = Counter(conflict.conflicting_values)
    most_common_value, count = value_counts.most_common(1)[0]
    total_votes = len(conflict.conflicting_values)
    confidence = count / total_votes if total_votes > 0 else 0.0
    return ResolutionResult(
        conflict_id=conflict.conflict_id, resolved=True,
        resolved_value=most_common_value, confidence=confidence,
        resolution_notes=f"Resolved by voting: {count}/{total_votes} votes",
    )
```

### 9.6 可信度加权算法

```python
def _resolve_by_credibility(self, conflict: Conflict) -> ResolutionResult:
    """权重 = 源置信度 × 源历史可信度"""
    value_weights: Dict[Any, float] = {}
    for i, value in enumerate(conflict.conflicting_values):
        source = conflict.sources[i] if i < len(conflict.sources) else {}
        document = source.get("document", "unknown")
        source_confidence = source.get("confidence", 0.5)
        credibility = self.source_tracker.get_source_credibility(document)
        weight = source_confidence * credibility
        value_weights[value] = value_weights.get(value, 0.0) + weight

    resolved_value = max(value_weights.items(), key=lambda x: x[1])[0]
    total_weight = sum(value_weights.values())
    confidence = value_weights[resolved_value] / total_weight if total_weight > 0 else 0.0
    return ResolutionResult(
        conflict_id=conflict.conflict_id, resolved=True,
        resolved_value=resolved_value, confidence=confidence,
        resolution_notes=f"Resolved by credibility-weighted voting (weight: {value_weights[resolved_value]:.2f})",
    )
```

### 9.7 分发器

```python
def resolve_conflict(
    self, conflict: Conflict, strategy: Union[str, ResolutionStrategy, None] = None
) -> ResolutionResult:
    # 按 entity_id.property_name 查找属性特定规则
    if strategy is None and conflict.property_name:
        rule_key = f"{conflict.entity_id}.{conflict.property_name}"
        if rule_key in self.resolution_rules:
            normalized_strategy = self.resolution_rules[rule_key]

    if strategy == ResolutionStrategy.VOTING:
        result = self._resolve_by_voting(conflict)
    elif strategy == ResolutionStrategy.CREDIBILITY_WEIGHTED:
        result = self._resolve_by_credibility(conflict)
    elif strategy == ResolutionStrategy.MOST_RECENT:
        result = self._resolve_by_recency(conflict)
    elif strategy == ResolutionStrategy.FIRST_SEEN:
        result = self._resolve_by_first_seen(conflict)
    elif strategy == ResolutionStrategy.HIGHEST_CONFIDENCE:
        result = self._resolve_by_confidence(conflict)
    elif strategy == ResolutionStrategy.MANUAL_REVIEW:
        result = self._flag_for_manual_review(conflict)
    elif strategy == ResolutionStrategy.EXPERT_REVIEW:
        result = self._flag_for_expert_review(conflict)
    else:
        result = self._resolve_by_voting(conflict)
    result.resolution_strategy = strategy.value
    self.resolution_history.append(result)
    return result
```

### 9.8 调查指南生成

**代码路径**:`semantica/conflicts/investigation_guide.py:138-179`

```python
def generate_guide(self, conflict: Conflict, additional_context=None) -> InvestigationGuide:
    """为冲突生成结构化调查指南"""
    return InvestigationGuide(
        conflict_id=conflict.conflict_id,
        conflict_summary=self._generate_summary(conflict),
        severity=conflict.severity,
        conflicting_sources=conflict.sources,
        investigation_steps=self._generate_investigation_steps(conflict),
        recommended_actions=self._generate_recommended_actions(conflict),
        context=additional_context or {},
    )
```

**6 步调查模板**:
1. Review conflict details and context
2. Identify all source documents
3. Compare conflicting information across sources
4. Assess source credibility and reliability
5. Determine resolution approach
6. Document resolution decision

---

## 十、对 laew 的借鉴

### P0(高优先级 — 立即产生价值)

#### 1. 工具调用溯源(P0)

**现状**:laew 完全无溯源机制

**方案**:
- 在 `src/agent/tools/` 层包装 Bash/Read/Write 工具调用
- 每次调用自动记录 `ProvenanceEntry` 到 SQLite 新表 `provenance`
- 关键字段:`entity_id`(文件路径/URL)、`activity_id`(read/write/bash)、`agent_id`、`source_document`(触发该调用的 user prompt 摘要)
- 实现 `compute_checksum()` SHA-256 哈希链,参照 Semantica 的 17 字段设计
- **预计代码量**:~300 行 Rust
- **价值**:完整审计日志,满足合规要求;调试时定位"为什么这次失败了"

#### 2. 双时序决策追踪(P0)

**现状**:Yolo 分类仅在每轮 SessionContext 中记录为 Markdown 摘要

**方案**:
- 借鉴 `BiTemporalFact` 模型,新增 SQLite 表 `decisions`:
  ```sql
  CREATE TABLE decisions (
      id TEXT PRIMARY KEY,
      category TEXT,           -- 'task_classification'|'tool_selection'|'plan_choice'
      scenario TEXT, reasoning TEXT, outcome TEXT,
      confidence REAL, entities TEXT,  -- JSON list
      valid_from TEXT, valid_until TEXT,    -- 业务时间
      recorded_at TEXT, superseded_at TEXT, -- 系统时间
      metadata TEXT
  );
  ```
- `record_decision()` 在 Yolo/Plan/QC 关键节点调用
- 借鉴 `find_precedents_by_scenario()`,在 Yolo 处理时自动注入最近 N 条同类型决策
- **预计代码量**:~500 行 Rust
- **价值**:Yolo 决策可解释、可审计;相似历史决策自动参考

#### 3. 哈希链完整性验证(P0)

**现状**:SQLite 数据可被外部篡改

**方案**:
- 在 `decisions` 表和 `provenance` 表添加 `checksum` 和 `previous_checksum` 字段
- 实现 `verify_chain()` 函数,定期扫描检测异常
- 检测到 chain_break 时通过 TUI 横幅告警
- **预计代码量**:~200 行 Rust
- **价值**:检测 SQLite 篡改,满足企业合规

### P1(中优先级 — 显著增强)

#### 4. Datalog 规则引擎替换简单分类(P1)

**现状**:所有任务分类都依赖 LLM

**方案**:
- 集成 431 行 DatalogReasoner 到 Rust 代码(可翻译为 Rust 实现或 FFI 调用)
- 规则示例:
  ```prolog
  hard_task(T) :- file_count(T, N), N > 10.
  simple_task(T) :- not has_keywords(T, _).
  ```
- 规则明确的任务走 Datalog(毫秒级、零成本),规则模糊的才走 LLM
- **预计代码量**:~1500 行 Rust(Datalog 引擎翻译)
- **价值**:降本 30-50%(简单任务零 LLM 调用);规则可读可审计

#### 5. 7 策略冲突解决器(P1)

**现状**:Quality-Check Agent 仅单次检查

**方案**:
- 在 Quality-Check 后增加 `ConflictResolver`
- 当多个 SubAgent 输出不一致时,按预设策略解决
- 例如:`task_complexity > high` → `voting`;否则 `highest_confidence`
- **预计代码量**:~600 行 Rust
- **价值**:QC 通过率提升;降低人工干预频率

#### 6. 混合检索 RRF(P1)

**现状**:SessionContext 仅按时间取最近 N 条

**方案**:
- 实现 `HybridSearch.reciprocal_rank_fusion()`:`score = Σ 1/(k + rank)`,k=60
- 融合 3 个检索源:向量相似度 + 时间衰减 + 任务类别匹配
- **预计代码量**:~400 行 Rust
- **价值**:历史摘要检索更精准,减少无关上下文注入

#### 7. Rete 工具选择规则(P1)

**现状**:所有工具选择由 LLM 决策

**方案**:
- 实现 642 行 ReteEngine 的 Rust 版本(核心算法不变)
- 简单规则:
  - `Bash(rm, -rf, ?path) → must_have_approval`
  - `Read(?file) AND file_size(?file, >10MB) → block`
- **预计代码量**:~1800 行 Rust
- **价值**:降低工具选择的 LLM 调用频率和延迟

#### 8. 任务分类置信度加权(P1)

**现状**:当前 laew 完全信任 LLM 分类结果

**方案**:
- 引入 `calculate_weighted_confidence` 类似机制:综合 LLM 置信度 + 规则匹配相似度
- 例如:`final_confidence = 0.6 × llm_confidence + 0.4 × rule_match_score`
- **预计代码量**:~200 行 Rust

### P2(低优先级 — 长期演进)

#### 9. PipelineBuilder 风格的工作流 DSL(P2)

**现状**:MultiAgentOrchestrator 是硬编码 if/else

**方案**:迁移到流畅 DSL,允许用户配置:
```rust
let pipeline = PipelineBuilder::new()
    .add_step("yolo_classify", StepType::Yolo)
    .add_step("plan", StepType::Plan).depends_on(&["yolo_classify"])
    .add_step("main", StepType::MainWork).depends_on(&["plan"]).parallel_safe(true)
    .add_step("qc", StepType::QualityCheck).depends_on(&["main"])
    .build();
```
- **预计代码量**:~1000 行 Rust

#### 10. 本体管理(OWL/SHACL)(P2)

**现状**:无本体层

**方案**:为 Agent 工具/任务类型建立轻量 OWL 本体,支持语义推理
- **预计代码量**:~3000 行 Rust(含推理引擎)

#### 11. 17+ 摄取器适配器(P2)

**现状**:仅支持文件 Read 工具

**方案**:为 DB/Web/Repo/Stream 等数据源实现统一接口
- **预计代码量**:~5000+ 行 Rust

#### 12. 调查指南生成器(P2)

**现状**:QC 失败仅给错误信息

**方案**:在 `tui/error_screen` 增加 6 步排查模板,引导用户定位问题

#### 13. 插件注册机制(P2)

**现状**:工具注册是静态的 `builtin_registry()`

**方案**:
- `PluginRegistry` 动态发现 + 依赖解析
- 插件版本管理和兼容性检查
- 插件隔离和错误处理
- **预计代码量**:~800 行 Rust

#### 14. 生命周期管理(P2)

**方案**:
- 组件健康检查(`health_check()`)
- 优雅关闭序列(资源清理、状态持久化)
- 错误状态恢复

#### 15. MCP Server 暴露(P2)

**方案**:
- 将 Yolo 分类、任务执行、Session 管理暴露为 MCP 工具
- 支持 `SEMANTICA_KG_PATH` 类似的持久化机制
- 提供 stdio 传输模式供 IDE 集成

### 借鉴优先级总表

| 机制 | 核心价值 | 可移植性 | laew 借鉴优先级 | 预计代码量 |
|------|---------|---------|----------------|-----------|
| W3C PROV-O 溯源 | 审计级数据来源 | 高(237 行完整性 + 1521 行 manager) | **P0** | ~300 行 Rust |
| 双时序决策追踪 | 决策可解释性 | 中(数据模型需改造) | **P0** | ~500 行 Rust |
| 哈希链完整性 | 防篡改 | 高(单函数) | **P0** | ~200 行 Rust |
| Datalog 引擎 | 半朴素求值 + 多跳推理 | 高(仅 431 行 Python) | **P1** | ~1500 行 Rust |
| Rete 引擎 | 确定性规则匹配 | 高(仅 642 行 Python) | **P1** | ~1800 行 Rust |
| 冲突检测 7 策略 | 多场景解决 | 高(574 行 resolver) | **P1** | ~600 行 Rust |
| 混合检索 RRF | 多源融合 | 高(单函数) | **P1** | ~400 行 Rust |
| 语义抽取加权置信度 | 多方法融合 | 高(单函数) | **P1** | ~200 行 Rust |
| 流水线 DSL | 工作流可配置 | 中 | **P2** | ~1000 行 Rust |
| 本体管理 | 语义推理 | 低(依赖重) | **P2** | ~3000 行 Rust |
| 摄取管线 | 多源数据 | 低(依赖 Python 库) | **P2** | ~5000+ 行 Rust |

**最值得借鉴的 3 个组件**(代码量可控、价值密度高):

1. **`compute_checksum()` + `verify_chain()`**(237 行)—— 直接可移植到 Rust,零依赖
2. **`DatalogReasoner`**(431 行)—— 算法清晰,移植后用于任务分类
3. **`ConflictResolver` 七策略**(574 行)—— 独立组件,集成到 QC 流程

**3 个不适合直接借鉴**:
- 摄取管线(依赖 boto3/beautifulsoup4/pyarrow 等重依赖)
- Explorer 前端(React/Vite,独立产品)
- 多框架集成(LangChain/CrewAI 等的胶水代码)

---

## 总结

Semantica 是一个设计精良的图原生 AI 基础设施框架,其核心优势在于:

1. **完整的决策智能链路**:ContextGraph + DecisionRecorder + CausalChainAnalyzer + PolicyEngine 形成闭环
2. **确定性推理能力**:Rete(642 行)+ Datalog(431 行)+ SPARQL 三引擎不依赖 LLM
3. **W3C 标准兼容**:PROV-O 溯源(哈希链完整性)、OWL/SHACL/SKOS 本体、SPARQL 查询
4. **多后端抽象**:向量(7 种)、图(4 种)、RDF(5 种)统一接口
5. **企业级特性**:审计级溯源、7 种冲突解决策略、策略版本管理、双时序模型

Semantica 的设计哲学体现了**"图原生 + 确定性强 + 可审计"**,为 laew 提供了完整的参考实现路径,特别是决策可解释性、审计完整性、规则化推理这三个维度,可逐步集成到 laew 的 MultiAgent 架构中。
