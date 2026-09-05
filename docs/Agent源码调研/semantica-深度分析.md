# Semantica 源码深度分析报告

> 本报告基于 Semantica v0.6.7 源码，对 10 个核心维度进行**深度**分析。每个分析点均包含具体的代码路径、函数名、数据结构和代码片段，并附带对 laew 工程的 P0/P1/P2 借鉴建议。

---

## 一、Context Graph 深度分析

### 1.1 ContextNode / ContextEdge 数据结构

**代码路径**：`semantica/context/context_graph.py`（5659 行）

**核心数据类**：

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

@dataclass
class ContextEdge:
    source_id: str
    target_id: str
    edge_type: str
    edge_id: str = ""
    weight: float = 1.0
    family_id: Optional[str] = None
    metadata: Dict[str, Any] = field(default_factory=dict)
    valid_from: Optional[str] = None
    valid_until: Optional[str] = None
```

**设计要点**：
- 节点/边均带**时序有效性窗口**（`valid_from`/`valid_until`），`is_active(at_time)` 方法判断特定时间是否有效
- `family_id` 支持边的版本家族，同族边可追踪演化
- 边 ID 通过 `_resolve_edge_identity()` 用 `uuid.uuid5(NAMESPACE_URL, payload)` 确定性生成，保证幂等
- SKOS 层次边通过 `is_skos_hierarchy_edge()` + `validate_skos_hierarchy()` 维护不变性

**时序解析函数** `_parse_iso_dt()` 支持 4 种格式（年/日期/完整 ISO/带时区），统一转为 tz-naive UTC：

```python
def _parse_iso_dt(value: str) -> Optional[datetime]:
    if _re.fullmatch(r"\d{4}", s): s = f"{s}-01-01"
    s = s.replace("Z", "+00:00")
    dt = datetime.fromisoformat(s)
    if dt.tzinfo is not None:
        dt = dt.astimezone(timezone.utc).replace(tzinfo=None)
    return dt
```

### 1.2 ContextGraph 内部索引结构

```python
self.nodes: Dict[str, ContextNode] = {}
self.edges: List[ContextEdge] = []
self._edge_index: Dict[str, ContextEdge] = {}
self._adjacency: Dict[str, List[ContextEdge]] = defaultdict(list)
self.node_type_index: Dict[str, Set[str]] = defaultdict(set)
self.edge_type_index: Dict[str, List[ContextEdge]] = defaultdict(list)
self._retractions: Dict[Tuple[str,str], Dict] = {}  # 撤回（保留实体）
self._tombstones: Dict[Tuple[str,str], Dict] = {}   # 墓碑（彻底清除）
```

**双删除语义**：retraction 仅关闭实体的 `valid_until` 窗口（可恢复），tombstone 记录彻底清除。键用 `(entity_kind, entity_id)` 元组避免节点/边 ID 冲突。

### 1.3 决策记录与先例搜索

`record_decision()` 记录完整决策上下文（443 行验证逻辑）：

```python
def record_decision(self, category, scenario, reasoning, outcome, 
                    confidence, entities=None, decision_maker=None,
                    metadata=None, valid_from=None, valid_until=None, **kwargs) -> str:
    decision_id = str(uuid.uuid4())
    self._add_decision_to_graph(decision)
    self._decision_index[category].add(decision_id)
    for entity in entities or []:
        self._entity_index[entity].add(decision_id)
    self._temporal_index.append((decision_id, timestamp))
```

**先例搜索** `find_precedents_by_scenario()` 使用**混合相似度**：`combined_sim = 0.7 * content_sim + 0.3 * structural_sim`，结合内容相似度与图结构相似度，支持按时间 `as_of` 过滤。

### 1.4 因果链追踪

**`CausalChainAnalyzer`**（`semantica/context/causal_analyzer.py`，780 行）提供：

- `get_causal_chain()` — BFS 上游/下游追踪，支持 Cypher `MATCH path = (start)-[:CAUSED|:INFLUENCED*1..n]->(end)`
- `trace_at_time()` — **事务时间旅行**，仅使用 `recorded_at <= cutoff` 的边
- `find_causal_loops()` — 环检测（`MATCH path = (d1)-[:CAUSED|:INFLUENCED*2..n]->(d1)`）
- `find_root_causes()` — 根因查找（上游无因节点）
- `interpret_causal_distance()` — 生成结构化因果距离报告

**因果边双词汇系统**：

```python
_CAUSAL_EDGE_TYPES = ("CAUSED", "INFLUENCED", "PRECEDENT_FOR")
_CAUSAL_EDGE_ALIASES = {"CAUSES": "CAUSED", "INFLUENCES": "INFLUENCED", "PRECEDES": "PRECEDENT_FOR"}
_CAUSAL_TRAVERSAL_TYPES = frozenset(_CAUSAL_EDGE_ALIASES) | {"LEADS_TO", "SUPPORTS"}
```

### 1.5 策略引擎

**`PolicyEngine`**（`semantica/context/policy_engine.py`，1044 行）：

- `add_policy()` / `update_policy()` — 策略版本化，新版本通过 `[:VERSION_OF]` 链接旧版
- `check_compliance()` — 规则评估，支持 `min_X`/`max_X`/`required_X`/`allowed_outcomes`/`min_confidence` 等前缀约定
- `analyze_policy_impact()` — What-if 影响分析，返回 `affected_decisions`/`compliance_impact`/`risk_increase`
- `record_exception()` / `record_policy_application()` — 策略例外与合规审计

**规则评估算法** `_evaluate_compliance()`：

```python
if rule_key.startswith("min_"):
    if field_value < rule_value: return False
elif rule_key.startswith("max_"):
    if field_value > rule_value: return False
elif rule_key.startswith("required_"):
    if not all(item in field_value for item in rule_value): return False
```

---

## 二、知识图谱深度分析

### 2.1 GraphBuilder 构建流程

**代码路径**：`semantica/kg/graph_builder.py`（1396 行）

`build()` 是核心入口，处理多源输入（文本/实体/关系字典）：

```python
def build(self, sources, second_arg=None, pipeline_id=None, **options) -> Dict[str, Any]:
    # 1. 多态输入处理 _process_item()
    #    - str → _extract_from_text()
    #    - Entity/Relation 对象 → dict 标准化
    #    - dict → 递归处理 entities/relationships
    # 2. 文本抽取: NERExtractor → RelationExtractor → TripletExtractor
    # 3. 实体合并 entity_resolver.resolve()
    # 4. 关系端点重映射 _remap_relationship_endpoints()
    # 5. 冲突检测 conflict_detector.detect_conflicts()
```

**抽取器缓存**避免 spaCy 模型重复加载：

```python
def _get_extractor(self, kind, extractor_cls, method):
    key = (kind, tuple(method) if isinstance(method, list) else method)
    if key not in self._extractor_cache:
        self._extractor_cache[key] = extractor_cls(method=method, **self.config)
    return self._extractor_cache[key]
```

### 2.2 EntityResolver 实体解析

**代码路径**：`semantica/kg/entity_resolver.py`

三步流程：检测重复组（`DuplicateDetector`）→ 合并重复项（`EntityMerger`）→ 合并非重复项。策略参数 `entity_resolution_strategy` 支持 `fuzzy`/`exact`/`ml-based`。合并后记录 `merged_from` 源 ID 列表，供关系端点重映射使用。

### 2.3 BiTemporalFact 双时序模型

**代码路径**：`semantica/kg/temporal_model.py`（175 行）

```python
@dataclass
class BiTemporalFact:
    valid_from: Optional[datetime]              # 业务有效性开始
    valid_until: Optional[datetime | TemporalBound]  # 业务有效性结束
    recorded_at: datetime = field(default_factory=_default_recorded_at)  # 记录时间
    superseded_at: datetime | TemporalBound = TemporalBound.OPEN        # 取代时间
```

**四时间维度**语义：
- **业务时间**（valid_from/until）：事实本身何时有效
- **记录时间**（recorded_at）：系统何时记录
- **取代时间**（superseded_at）：何时被新事实替代
- `TemporalBound.OPEN` 哨兵值表示开放区间

配套 `parse_temporal_value()`/`serialize_temporal_value()` 统一处理 datetime/epoch/ISO 字符串，始终转为 UTC。

---

## 三、推理引擎深度分析

### 3.1 Rete 前向链算法

**代码路径**：`semantica/reasoning/rete_engine.py`（642 行）

**网络结构**：

```python
class AlphaNode(ReteNode):   # 单条件匹配
    condition: Any
    tokens: List[Token]
    _compiled: Optional[re.Pattern]  # 预编译正则

class BetaNode(ReteNode):    # 连接操作
    left: ReteNode
    right: ReteNode
    left_tokens: List[Token]
    right_tokens: List[Token]
    
    def join(self, left_token, right_token) -> Optional[Token]:
        merged = dict(left_token.bindings)
        for var, value in right_token.bindings.items():
            if var in merged and merged[var] != value:
                return None  # 绑定冲突
        return Token(facts=list(left_token.facts)+list(right_token.facts), bindings=merged)

class TerminalNode(ReteNode): # 规则激活
    rule: Rule
    activations: List[Match]
```

**合一算法** `unify_condition()` 使用正则变量绑定：

```python
def _build_condition_regex(pattern, initial_bindings=None):
    segments = re.split(r"(\?\w+)", pattern)
    for seg in segments:
        if seg.startswith("?"):
            if var_name in bindings:
                p_regex += re.escape(bindings[var_name])  # 已绑定→字面量
            elif var_name in seen_vars:
                p_regex += f"(?P={var_name})"              # 重复变量→反向引用
            else:
                p_regex += f"(?P<{var_name}>.+?)"          # 新变量→命名捕获
    return f"^{p_regex}$"
```

**传播机制** `_propagate_to_beta()` 实现链式连接：新 token 到达一侧时，与另一侧所有已有 token 尝试连接，成功则向下游传播。

### 3.2 Datalog 半朴素不动点

**代码路径**：`semantica/reasoning/datalog_reasoner.py`（431 行）

```python
@dataclass(frozen=True)
class DatalogFact:
    predicate: str
    args: Tuple[str, ...]

class DatalogRule:
    head_predicate: str
    head_args: Tuple[str, ...]
    body: List[BodyAtom]
```

**半朴素求值** `derive_all()`：

```python
def derive_all(self) -> List[str]:
    self._delta_new = self._all_facts.copy()
    while self._delta_new:
        self._delta_old = self._delta_new
        self._delta_new = set()
        delta_index = defaultdict(set)
        for f in self._delta_old:
            delta_index[f.predicate].add(f)
        for rule in self._rules:
            new_facts = self._apply_rule(rule, delta_index)
            for fact in new_facts:
                if fact not in self._all_facts:
                    self._delta_new.add(fact)
                    self._all_facts.add(fact)
```

**半朴素策略** `_apply_rule()`：对每个规则，依次将每个体原子位置 `i` 作为"增量位置"，仅该位置使用 `delta_index`（新事实），其余位置使用完整 `_fact_index`。这保证每轮至少有一个体原子使用新事实，避免重复推导，同时保证终止性。

**合一** `_unify()` 优化：仅在产生新绑定时分配新字典：

```python
def _unify(self, pattern_args, fact_args, bindings):
    new_additions = {}
    for p_arg, f_arg in zip(pattern_args, fact_args):
        if self._is_variable(p_arg):
            if p_arg in bindings:
                if bindings[p_arg] != f_arg: return None
            elif p_arg in new_additions:
                if new_additions[p_arg] != f_arg: return None
            else:
                new_additions[p_arg] = f_arg
    if new_additions: return {**bindings, **new_additions}
    return bindings
```

### 3.3 SPARQL 推理

**代码路径**：`semantica/reasoning/sparql_reasoner.py`，基于 RDFLib 的 SPARQL 1.1 实现。

---

## 四、溯源系统深度分析

### 4.1 W3C PROV-O 标准实现

**代码路径**：`semantica/provenance/schemas.py`（498 行）

`ProvenanceEntry` 数据类完整映射 W3C PROV-O：

| 字段 | PROV-O 映射 |
|------|-------------|
| `entity_id` | `prov:Entity` |
| `activity_id` | `prov:Activity` |
| `agent_id`/`agent_type` | `prov:Agent` (Person/SoftwareAgent/Organization) |
| `role` | `prov:hadRole` |
| `parent_entity_id` | `prov:wasDerivedFrom`（遗留） |
| `derived_from_id` | `prov:wasDerivedFrom`（跨源派生） |
| `previous_version_id` | 同事实前一版本（修正） |
| `used_entities` | `prov:used` |
| `timestamp` | `prov:generatedAtTime` |
| `invalidated`/`invalidated_at_time` | `prov:Invalidation` |
| `activity_started_at_time` | `prov:startedAtTime` |
| `acted_on_behalf_of` | `prov:actedOnBehalfOf` |
| `bundle_id` | `prov:Collection`/`prov:Bundle` |

**审计级源追踪**：`source_document`（DOI/URL）+ `source_location`（页码/图表）+ `source_quote`（直接引用）三元组支持精确溯源。

### 4.2 哈希链完整性

**代码路径**：`semantica/provenance/integrity.py`（237 行）

```python
def compute_checksum(entry) -> str:
    data = (
        f"{entry.entity_type}{entry.activity_id}{entry.agent_id}"
        f"{entry.agent_type}{entry.source_document}{entry.timestamp}"
        f"{entry.confidence}{entry.parent_entity_id}"
        f"{entry.previous_version_id}{entry.derived_from_id}"
        f"{','.join(used_entities)}{entry.previous_checksum}"
        f"{bool(entry.invalidated)}{entry.invalidated_at_time}"
        ...
    )
    return hashlib.sha256(data.encode('utf-8')).hexdigest()
```

**关键设计**：
- **排除 `entity_id`**：entity_id 是主键，版本归档时重命名（`X` → `X:v:...`）不应破坏链
- **包含 `previous_checksum`**：形成哈希链，删除一行会破坏后续所有链
- **包含 lineage 字段**：篡改 attribution/派生关系可被检测

### 4.3 ProvenanceManager 事务与版本

**代码路径**：`semantica/provenance/manager.py`（1521 行）

`track_entity()` 使用原子事务（`BEGIN IMMEDIATE`）：

```python
def track_entity(self, entity_id, source, metadata=None, **kwargs):
    with self._get_or_create_transaction(_conn) as conn:
        existing = self._retrieve_with_conn(conn, entity_id)
        if existing:
            history_entry = copy.deepcopy(existing)
            history_entry.entity_id = f"{entity_id}:v:{existing.last_updated}"
            self._store_with_conn(conn, history_entry)  # 归档旧版
        entry = ProvenanceEntry(...)
        self._save_entry(entry, _conn=conn)
```

**双版本语义**：`previous_version_id`（同事实修正）vs `derived_from_id`（跨源派生），两者独立，不混淆。

---

## 五、摄取管线深度分析

### 5.1 15+ 摄取器统一抽象

**代码路径**：`semantica/ingest/`

| 摄取器 | 文件 | 数据源 |
|--------|------|--------|
| `FileIngestor` | `file_ingestor.py` | 本地文件 + S3/GCS/Azure |
| `WebIngestor` | `web_ingestor.py` | 网页（robots.txt/sitemap） |
| `FeedIngestor` | `feed_ingestor.py` | RSS/Atom |
| `StreamIngestor` | `stream_ingestor.py` | Kafka/RabbitMQ/Kinesis/Pulsar |
| `RepoIngestor` | `repo_ingestor.py` | Git 仓库 |
| `EmailIngestor` | `email_ingestor.py` | IMAP/POP3 |
| `DBIngestor` | `db_ingestor.py` | PostgreSQL/MySQL/SQLite/Oracle |
| `SnowflakeIngestor` | `snowflake_ingestor.py` | Snowflake |
| `DatabricksIngestor` | `databricks_ingestor.py` | Unity Catalog + Delta Lake |
| `SAPIngestor` | `sap_ingestor.py` | SAP OData |
| `SalesforceIngestor` | `salesforce_ingestor.py` | Salesforce CRM |
| `ParquetIngestor` | `parquet_ingestor.py` | Apache Parquet |
| `ArrowIngestor` | `arrow_ingestor.py` | Arrow IPC/Feather |
| `MCPIngestor` | `mcp_ingestor.py` | MCP 资源 |

### 5.2 FileTypeDetector 三法检测

```python
class FileTypeDetector:
    # 1. 扩展名检测
    # 2. MIME 类型检测
    # 3. 魔数分析（magic bytes）
```

### 5.3 增量模式

`PipelineStep.delta_mode` + `base_version_id`/`target_version_id` 支持增量处理，仅处理版本窗口内的数据。

---

## 六、语义抽取深度分析

### 6.1 NER 多方法回退链

**代码路径**：`semantica/semantic_extract/ner_extractor.py`（643 行）

```python
class NERExtractor:
    def __init__(self, method: Union[str, List[str]] = "ml", ...):
        self.method = method if isinstance(method, list) else [method]
        # "pattern" → "regex" → "rules" → "ml"(spaCy) → "huggingface" → "llm"
    
    def extract_entities(self, text, **options):
        for method_name in methods:  # 回退链
            method_func = get_entity_method(method_name)
            entities = method_func(text, **all_options)
```

**批量并行**：`extract()` 使用 `ThreadPoolExecutor`，`max_workers` 由 `resolve_max_workers()` 配置。

### 6.2 加权置信度评分

**代码路径**：`semantica/semantic_extract/methods.py` `calculate_weighted_confidence()`

```python
def calculate_weighted_confidence(item_type, original_confidence, valid_types=None, item_text=None,
                                   weight_method=0.5, weight_similarity=0.5) -> float:
    label_similarity = calculate_similarity(item_type, valid_types)
    content_similarity = calculate_similarity(item_text, valid_types) if item_text else 0.0
    best_similarity = max(label_similarity, content_similarity)
    final_score = (w_m * original_confidence) + (w_s * best_similarity)
    return max(0.0, min(1.0, final_score))
```

**公式**：`Score = (w_m × Method_Confidence) + (w_s × max(Label_Sim, Content_Sim))`

**类型相似度层级**：精确匹配（1.0）→ 同义词（0.95）→ 嵌入余弦相似度。

### 6.3 LLM 提示词设计

**NER 抽取提示词**（`extract_entities_llm()`）：

```python
prompt = f"""Extract named entities from the provided text.
Return the result as a JSON object with an "entities" key...
IMPORTANT: 
- Return a FLAT LIST of entities. 
- DO NOT group entities by type.
- The output structure must exactly match: {{ "entities": [ {{ "text": "...", "label": "...", "confidence": ... }}, ... ] }}
...
Text to extract from:
{text}"""
```

**长文本分块回退**：当 LLM 输出因 token 限制截断时，`_extract_entities_chunked()` 将文本分块（10% 重叠），并行抽取后调整 `start_char`/`end_char` 偏移。

---

## 七、本体管理深度分析

### 7.1 6 阶段生成流水线

**代码路径**：`semantica/ontology/ontology_generator.py`（1394 行）

```python
class OntologyGenerator:
    def generate_ontology(self, data, **options) -> Dict[str, Any]:
        # Stage 1: 语义网络解析 — 从实体/关系提取领域概念
        # Stage 2: YAML 到定义 — 概念转化为类定义
        # Stage 3: 定义到类型 — 映射到 OWL 类型
        # Stage 4: 层次生成 — 构建分类结构，DFS 检测循环依赖
        # Stage 5: TTL 生成 — 使用 rdflib 生成 OWL/Turtle
        # Stage 6: 符号验证 — HermiT/Pellet 推理机一致性检查
```

### 7.2 SHACL 生成器

```python
class SHACLGenerator:
    """6-stage internal pipeline:
    1. Index building — 构建 OWL 索引
    2. Shape inference — 推断 NodeShape/PropertyShape
    3. Constraint generation — 生成 sh:datatype/sh:minCount 等约束
    4. Serialization — Turtle 序列化
    5. Validation — pySHACL 验证
    """
    def generate(self, ontology, **options) -> SHACLGraph:
        # SHACLGraph 包含 NodeShape 和 PropertyShape 列表
```

---

## 八、流水线 DSL 深度分析

### 8.1 PipelineBuilder API

**代码路径**：`semantica/pipeline/pipeline_builder.py`

```python
@dataclass
class PipelineStep:
    name: str
    step_type: str
    config: Dict[str, Any] = field(default_factory=dict)
    dependencies: List[str] = field(default_factory=list)
    handler: Optional[Callable] = None
    status: StepStatus = StepStatus.PENDING
    delta_mode: bool = False
    base_version_id: Optional[str] = None
    target_version_id: Optional[str] = None
    parallel_safe: bool = False  # 并行安全标记
```

**流畅 DSL**：

```python
builder = PipelineBuilder()
pipeline = (builder
    .add_step("ingest", "file_ingest")
    .add_step("parse", "document_parse", dependencies=["ingest"])
    .add_step("extract", "ner_extract", parallel_safe=True)
    .connect_steps("parse", "extract")
    .set_parallelism(4)
    .build(name="doc_pipeline"))
```

### 8.2 失败处理

**代码路径**：`semantica/pipeline/failure_handler.py`

支持 `RetryStrategy`（重试策略）、`FallbackHandler`（回退处理）、`ErrorRecovery`（错误恢复）。`StepStatus` 枚举：`PENDING`/`RUNNING`/`COMPLETED`/`FAILED`/`SKIPPED`。

---

## 九、冲突检测深度分析

### 9.1 7 种解决策略

**代码路径**：`semantica/conflicts/conflict_resolver.py`

```python
class ResolutionStrategy(str, Enum):
    VOTING = "voting"                   # 多数投票
    CREDIBILITY_WEIGHTED = "credibility_weighted"  # 可信度加权
    MOST_RECENT = "most_recent"         # 最新值
    FIRST_SEEN = "first_seen"           # 最早值
    HIGHEST_CONFIDENCE = "highest_confidence"  # 最高置信度
    MANUAL_REVIEW = "manual_review"     # 人工审核
    EXPERT_REVIEW = "expert_review"     # 专家审核
```

**投票算法** `_resolve_by_voting()`：

```python
value_counts = Counter(conflict.conflicting_values)
most_common_value, count = value_counts.most_common(1)[0]
confidence = count / total_votes
```

**可信度加权** `_resolve_by_credibility()`：

```python
weight = source_confidence * credibility  # 源置信度 × 源可信度历史
value_weights[value] += weight
resolved_value = max(value_weights.items(), key=lambda x: x[1])[0]
```

### 9.2 冲突类型

```python
class ConflictType(str, Enum):
    VALUE_CONFLICT = "value_conflit"        # 值冲突
    TYPE_CONFLICT = "type_conflict"          # 类型冲突
    RELATIONSHIP_CONFLICT = "relationship_conflict"  # 关系冲突
    TEMPORAL_CONFLICT = "temporal_conflict"  # 时序冲突
    LOGICAL_CONFLICT = "logical_conflict"    # 逻辑冲突
```

### 9.3 调查指南生成

**代码路径**：`semantica/conflicts/investigation_guide.py` — `InvestigationGuideGenerator` 为冲突生成结构化调查步骤，指导人工审核。

---

## 十、存储抽象深度分析

### 10.1 VectorStore 统一接口

**代码路径**：`semantica/vector_store/vector_store.py`

```python
class VectorStore:
    def add_documents(self, documents, **options) -> ...
    def search(self, query, limit=10, **options) -> List[Dict]
    def search_vectors(self, query_vector, limit=10, **options) -> List[Dict]
    def search_hybrid(self, query, limit=10, **options) -> List[Dict]
    def search_similar(self, vector_id, limit=10, **options) -> List[Dict]
    def search_by_metadata(self, filters, limit=10, **options) -> List[Dict]
    def delete_vectors(self, vector_ids, **options) -> bool
```

**6 种后端**：`FAISSStore`/`QdrantStore`/`WeaviateStore`/`MilvusStore`/`PineconeStore`/`PgVectorStore`/`SQLiteVecStore`

### 10.2 HybridSearch RRF 融合

**代码路径**：`semantica/vector_store/hybrid_search.py`

```python
class SearchRanker:
    def reciprocal_rank_fusion(self, results: List[List[Dict]], k: int = 60) -> List[Dict]:
        scores: Dict[str, float] = {}
        for result_list in results:
            for rank, result in enumerate(result_list, start=1):
                result_id = result.get("id", str(id(result)))
                score = 1.0 / (k + rank)  # RRF 公式
                scores[result_id] = scores.get(result_id, 0.0) + score
        ranked = sorted(scores.items(), key=lambda x: x[1], reverse=True)
```

**RRF 公式**：`score = Σ 1/(k + rank)`，k 默认 60。同时支持 `weighted_average` 加权平均融合。

### 10.3 GraphStore 接口

**代码路径**：`semantica/graph_store/graph_store.py`

```python
class GraphStore:
    def add_nodes(self, nodes, **options) -> int
    def add_edges(self, edges, **options) -> int
    def delete_node(self, node_id, **options) -> bool
    def delete_relationship(self, rel_id, **options) -> bool
    def query(self, query, **options) -> List[Dict]  # Cypher
```

**4 种后端**：`Neo4jStore`/`FalkorDBStore`/`ApacheAgeStore`/`AmazonNeptuneStore`

**查询消毒**：`query_sanitize.py` 提供 Cypher 注入防护。

### 10.4 TripletStore 接口

**代码路径**：`semantica/triplet_store/triplet_store.py`

```python
class TripletStore:
    def add_triplet(self, triplet: Triplet, **options) -> Dict
    def add_triplets(self, triplets: List[Triplet], **options) -> Dict
    def delete_triplet(self, triplet: Triplet, **options) -> Dict
    def add_skos_concept(self, concept, **options) -> Dict
```

**5 种后端**：`OxigraphStore`/`BlazegraphStore`/`JenaStore`/`RDF4JStore`/`AnzoStore`

**Oxigraph 集成**：嵌入式 SPARQL 1.1，支持命名图、事务批量加载、显式 `flush()` 保证持久性。

---

## 数据模型图（文字描述）

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
│  │ (邻接表)     │ │ index        │ │ (边类型→边列表)       │    │
│  └──────────────┘ └──────────────┘ └──────────────────────┘    │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────────────┐    │
│  │ _decisions   │ │ _decision_   │ │ _entity_index        │    │
│  │ (决策存储)   │ │ index        │ │ (实体→决策)           │    │
│  └──────────────┘ └──────────────┘ └──────────────────────┘    │
│  ┌──────────────┐ ┌──────────────┐                              │
│  │ _retractions │ │ _tombstones  │  (双删除语义)               │
│  └──────────────┘ └──────────────┘                              │
└─────────────────────────────────────────────────────────────────┘

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
│  (entity_id 故意排除，支持版本归档重命名)                         │
└─────────────────────────────────────────────────────────────────┘

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

## 流程图（文字描述）

### 决策记录与因果追踪流程

```
用户决策输入
    │
    ▼
record_decision(category, scenario, reasoning, outcome, confidence, entities)
    │
    ├─→ 输入验证（443 行验证逻辑）
    ├─→ _add_decision_to_graph() → ContextNode(type="decision")
    ├─→ _decisions[decision_id] = decision
    ├─→ _decision_index[category].add(decision_id)
    ├─→ _entity_index[entity].add(decision_id)
    └─→ _temporal_index.append((decision_id, timestamp))
    │
    ▼
add_causal_relationship(source_id, target_id, relationship_type)
    │
    ├─→ 词汇归一化（CAUSES→CAUSED, INFLUENCES→INFLUENCED）
    ├─→ 验证两节点存在且为 decision 类型
    └─→ _add_internal_edge(ContextEdge(type=CAUSED/INFLUENCED/PRECEDENT_FOR))
    │
    ▼
trace_decision_causality(decision_id, max_depth=5)
    │
    ├─→ 构建 incoming_causal_edges 反向索引
    ├─→ 递归 DFS 追踪
    │   ├─→ 显式因果边优先（add_causal_relationship 记录的）
    │   ├─→ 隐式因果（共享实体 + 时间顺序）
    │   └─→ 环检测（per-path path_ids）
    └─→ 返回因果链报告（含 confidence_decay, weakest_link）
```

### 知识图谱构建流程

```
sources (文本/实体/关系)
    │
    ▼
_process_item() 多态处理
    │ str → _extract_from_text()
    │ Entity/Relation 对象 → dict 标准化
    │ dict → 递归处理 entities/relationships
    │
    ▼
_extract_from_text(text)
    │
    ├─→ NERExtractor.extract_entities(text) → entities
    ├─→ RelationExtractor.extract_relations(text, entities) → relations
    └─→ TripletExtractor.extract_triplets(text, entities, relations) → triplets
    │
    ▼
entity_resolver.resolve(entities) → merged_entities
    │ fuzzy/exact/ml-based 策略
    │ 记录 merged_from 源 ID
    ▼
_remap_relationship_endpoints(entities, relationships)
    │ 将旧实体 ID 映射到合并后的规范 ID
    ▼
conflict_detector.detect_conflicts()
    │ VALUE/TYPE/RELATIONSHIP/TEMPORAL/LOGICAL 5 种冲突
    ▼
conflict_resolver.resolve_conflicts()
    │ voting/credibility_weighted/most_recent/first_seen/highest_confidence
    ▼
GraphStore.add_nodes() / add_edges()
```

---

## 对 laew 工程的深度借鉴建议

### P0（高优先级 — 核心价值）

#### 1. 决策追踪与因果链机制

**现状**：laew 的 SessionContext Agent 仅生成 Markdown 摘要写入 `session_memory` 表，缺乏结构化决策追踪和因果分析。

**借鉴**：
- 引入 `Decision` 数据类（category/scenario/reasoning/outcome/confidence），每次 Yolo 分类、工具选择均记录为决策节点
- 实现 `add_causal_relationship()` 记录"为什么选择这个工具"的因果链
- 使用 `find_precedents_by_scenario()` 在 Yolo 处理时注入相似历史决策

**落地路径**：在 SQLite 新增 `decisions` 表，`agent/mod.rs` 的 `run_session` 循环中插入决策记录点。

#### 2. W3C PROV-O 溯源机制

**现状**：laew 无溯源追踪，工具调用链不可审计。

**借鉴**：
- 引入 `ProvenanceEntry` 模式，每条消息/工具调用记录 `entity_id`/`activity_id`/`agent_id`/`source_document`
- 实现哈希链（`previous_checksum`）保证审计完整性
- 双删除语义（retraction vs tombstone）支持错误恢复

**落地路径**：在 `agent/tools/` 层包装每个工具调用，自动记录溯源条目。

#### 3. 混合检索策略（RRF）

**现状**：laew 的 SessionContext 历史注入仅按时间取最近 N 条。

**借鉴**：
- 实现 `HybridSearch.reciprocal_rank_fusion()` 算法
- 语义相似度 + 时间衰减 + 结构关系（决策图）三源融合
- `MetadataFilter` 按任务类型/难度分级过滤历史

**落地路径**：在 `session.rs` 中引入向量存储（SQLite-vec 或 FAISS），历史摘要生成 embedding 后 RRF 融合检索。

### P1（中优先级 — 显著增强）

#### 4. 确定性推理引擎

**现状**：laew 的任务分类完全依赖 LLM，存在不确定性和成本。

**借鉴**：
- 引入轻量 Datalog 引擎（431 行，无依赖），用于规则明确的分类
- Rete 引擎处理工具选择决策（规则匹配）
- `Explanation` 生成器提供可解释的决策路径

**落地路径**：在 `agent/yolo.rs` 中增加规则引擎层，简单模式匹配走规则，复杂情况走 LLM。

#### 5. 冲突检测与解决

**现状**：laew 的 Quality-Check Agent 仅做单次检查，无多策略解决。

**借鉴**：
- 7 种解决策略（投票/可信度加权/最新值/人工审核）
- 严重度计算（多因素评分）
- 调查指南生成

**落地路径**：在 `Quality-Check Agent` 中引入 `ConflictDetector` + `ConflictResolver`。

#### 6. 策略引擎

**现状**：laew 无策略合规机制。

**借鉴**：
- `PolicyEngine` 的策略版本化 + 合规检查
- `analyze_policy_impact()` What-if 分析
- 策略例外记录与审计

**落地路径**：为 Bash 工具引入策略引擎，检查命令合规性（黑名单/白名单）。

#### 7. 双时序模型

**现状**：laew 的 Session 管理无时间旅行能力。

**借鉴**：
- `BiTemporalFact` 四时间维度
- `trace_at_time()` 事务时间旅行
- 实体有效性窗口

**落地路径**：在 `session.rs` 中为对话历史增加 `valid_from`/`valid_until`，支持查询特定时间点的上下文状态。

### P2（低优先级 — 长期演进）

#### 8. 流水线 DSL

**现状**：laew 的 MultiAgentOrchestrator 是硬编码流程。

**借鉴**：
- `PipelineBuilder` 流畅 DSL
- `parallel_safe` 标记 + `ParallelismManager`
- `delta_mode` 增量处理

**落地路径**：将 Yolo→Plan→Main→SubAgent→QC→SessionContext 流程重构为 Pipeline。

#### 9. 插件注册机制

**现状**：laew 的工具注册是静态的 `builtin_registry()`。

**借鉴**：
- `PluginRegistry` 动态发现 + 依赖解析
- 插件版本管理和兼容性检查
- 插件隔离和错误处理

**落地路径**：支持从文件系统路径加载新工具，无需修改源码。

#### 10. 本体管理

**现状**：laew 无本体层。

**借鉴**：
- 6 阶段本体生成流水线
- OWL/SHACL/SKOS 标准兼容
- LLM 辅助本体生成

**落地路径**：长期可考虑为 Agent 工具/任务定义本体，支持语义推理。

#### 11. 存储抽象

**现状**：laew 仅使用 SQLite。

**借鉴**：
- `VectorStore`/`GraphStore`/`TripletStore` 统一接口
- 多后端可插拔（FAISS/Qdrant/Neo4j）
- `NamespaceManager` 多租户隔离

**落地路径**：抽象存储层，支持用户配置不同后端。

#### 12. MCP Server 暴露

**现状**：laew 无 MCP 接口。

**借鉴**：
- Semantica 的 MCP Server 将核心能力暴露为 15 个工具
- `SEMANTICA_KG_PATH` 持久化机制
- stdio 传输模式

**落地路径**：将 Yolo 分类、任务执行、Session 管理暴露为 MCP 工具，供 IDE 集成。

---

## 总结

Semantica 是一个设计精良的图原生 AI 基础设施框架，其核心优势在于：

1. **完整的决策智能链路**：ContextGraph + DecisionRecorder + CausalChainAnalyzer + PolicyEngine 形成闭环
2. **确定性推理能力**：Rete（642 行）+ Datalog（431 行）+ SPARQL 三引擎不依赖 LLM
3. **W3C 标准兼容**：PROV-O 溯源（哈希链完整性）、OWL/SHACL/SKOS 本体、SPARQL 查询
4. **多后端抽象**：向量（7 种）、图（4 种）、RDF（5 种）统一接口
5. **企业级特性**：审计级溯源、7 种冲突解决策略、策略版本管理、双时序模型

对于 laew 工程，**最值得借鉴**的是其决策追踪与溯源机制（P0）、混合检索策略（P0）和确定性推理引擎（P1）。这些能力可以显著提升 laew 在 Agent 决策可解释性、工具扩展性和工作流编排方面的能力。Semantica 的 `ContextNode`/`ContextEdge` 时序模型、`ProvenanceEntry` 哈希链、`DatalogReasoner` 半朴素不动点等实现，均为**可直接移植的轻量级组件**（单文件 400-600 行 Python），适合逐步集成到 laew 的 Rust 代码库中。
