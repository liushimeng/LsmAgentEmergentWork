# Semantica 核心机制深度分析报告

> 本报告基于 Semantica v0.6.7 真实源码，对 7 大核心机制进行**逐函数级别**的深度分析。
> 每个分析点都包含真实的代码路径、函数签名、关键代码片段，以及对 laew 工程的具体借鉴建议。
> 全部内容基于源码实际行号，可在 `/usr/local/LsmGitOpenSource/semantica/semantica/` 目录交叉验证。

---

## 目录

1. [Context Graph 核心代码路径](#一-context-graph-核心代码路径)
2. [Rete 推理引擎核心代码路径](#二-rete-推理引擎核心代码路径)
3. [Datalog 推理核心代码路径](#三-datalog-推理核心代码路径)
4. [W3C PROV-O 溯源核心代码路径](#四-w3c-prov-o-溯源核心代码路径)
5. [摄取管线核心代码路径](#五-摄取管线核心代码路径)
6. [语义抽取核心代码路径](#六-语义抽取核心代码路径)
7. [冲突检测核心代码路径](#七-冲突检测核心代码路径)
8. [对 laew 工程的 P0/P1/P2 借鉴路线图](#八-对-laew-工程的-p0p1p2-借鉴路线图)

---

## 一、Context Graph 核心代码路径

### 1.1 ContextNode / ContextEdge 数据结构

**代码路径**：`semantica/context/context_graph.py`（5659 行）

**ContextNode 数据类**（约第 380-413 行）：

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
        """判断节点在指定时间是否有效（基于 valid_from / valid_until）"""
        ...
```

**ContextEdge 数据类**（约第 430-476 行）：

```python
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

**关键设计要点**：

- **时序有效性窗口**：`is_active(at_time)` 通过比较 `at_time` 与 `valid_from`/`valid_until` 判断节点/边在特定时间是否有效
- **`family_id` 版本家族**：同族边可追踪演化关系（用于边 ID 重命名后追溯原家族）
- **边 ID 确定性生成**：`_resolve_edge_identity()` 使用 `uuid.uuid5(NAMESPACE_URL, payload)` 从 (source, target, type, weight, valid_from, valid_until, metadata) 哈希生成确定性 ID

### 1.2 时序解析函数 `_parse_iso_dt()`

**代码路径**：`semantica/context/context_graph.py:203-231`

```python
def _parse_iso_dt(value: str) -> Optional[datetime]:
    """Parse an ISO datetime string into a tz-naive UTC datetime.

    Supported formats (in priority order):
        - Year-only shorthand:  "1990"  → "1990-01-01"
        - Date-only:            "1990-06-15"
        - Full ISO (with tz):   "1990-06-15T00:00:00+00:00" / "...Z"
        - Full ISO (naive):     "1990-06-15T00:00:00"

    Returns None on failure; callers must treat the node as Always-Active.
    """
    import logging
    import re as _re
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

**设计要点**：失败时返回 None，调用方将其视为"始终有效"——即**优雅降级**而非崩溃。

### 1.3 ContextGraph 内部索引结构

**代码路径**：`semantica/context/context_graph.py:569-590`

```python
self.nodes: Dict[str, ContextNode] = {}
self.edges: List[ContextEdge] = []
self._edge_index: Dict[str, ContextEdge] = {}

self._adjacency: Dict[str, List[ContextEdge]] = defaultdict(list)
self.node_type_index: Dict[str, Set[str]] = defaultdict(set)
self.edge_type_index: Dict[str, List[ContextEdge]] = defaultdict(list)

# 双删除语义：
# - retraction: 仅关闭实体的 valid_until 窗口（可恢复，保留实体）
# - tombstone: 彻底清除（保留清除记录，不保留内容）
self._retractions: Dict[Tuple[str, str], Dict] = {}  # (entity_kind, entity_id)
self._tombstones: Dict[Tuple[str, str], Dict] = {}
```

**设计要点**：键用 `(entity_kind, entity_id)` 元组避免节点/边 ID 冲突（因 edge_id 是 UUID、node_id 是用户字符串，二者命名空间不同）。

### 1.4 决策记录 `record_decision()`

**代码路径**：`semantica/context/context_graph.py:4285-4432`

```python
def record_decision(
    self,
    category: str,
    scenario: str,
    reasoning: str,
    outcome: str,
    confidence: float,
    entities: Optional[List[str]] = None,
    decision_maker: Optional[str] = None,
    metadata: Optional[Dict[str, Any]] = None,
    valid_from: Optional[Union[str, int, float, datetime]] = None,
    valid_until: Optional[Union[str, int, float, datetime]] = None,
    **kwargs
) -> str:
    """443 行中含完整输入验证（category/scenario/reasoning/outcome 长度上限、类型校验）"""
    
    # 输入校验（4330-4378 行）
    if not isinstance(category, str) or not category.strip():
        raise ValueError("Category must be a non-empty string")
    if len(category.strip()) > 100:
        raise ValueError("Category must be 100 characters or less")
    # ... 更多校验
    
    decision_id = str(uuid.uuid4())
    timestamp = datetime.now().timestamp()
    
    decision = {
        "id": decision_id,
        "category": category,
        "scenario": scenario,
        "reasoning": reasoning,
        "outcome": outcome,
        "confidence": confidence,
        "entities": entities,
        "decision_maker": decision_maker,
        "timestamp": timestamp,
        "recorded_at": datetime.utcnow().isoformat(),
        "valid_from": normalized_valid_from,
        "valid_until": normalized_valid_until,
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

**设计要点**：443 行中含**严密的输入验证**（每个字段类型+长度约束），并将决策存入**三个独立索引**（category 索引、entity 反向索引、temporal 索引），支撑后续混合检索。

### 1.5 因果边双词汇系统

**代码路径**：`semantica/context/context_graph.py:500-517`

```python
#: 词汇归一化:本模块使用过去时, analyzer 使用现在时; 存储前归一化到过去时
_CAUSAL_EDGE_TYPES = ("CAUSED", "INFLUENCED", "PRECEDENT_FOR")

_CAUSAL_EDGE_ALIASES = {
    "CAUSES": "CAUSED",
    "CAUSED": "CAUSED",
    "INFLUENCES": "INFLUENCED",
    "INFLUENCED": "INFLUENCED",
    "PRECEDES": "PRECEDENT_FOR",
    "PRECEDENT_FOR": "PRECEDENT_FOR",
}
_CAUSAL_TRAVERSAL_TYPES = frozenset(_CAUSAL_EDGE_ALIASES) | {
    "LEADS_TO", "LEAD_TO", "SUPPORTS", "SUPPORT",
}
```

### 1.6 `add_causal_relationship()` 因果关系记录

**代码路径**：`semantica/context/context_graph.py:3836-3880`

```python
def add_causal_relationship(
    self,
    source_decision_id: str,
    target_decision_id: str,
    relationship_type: str
) -> None:
    """Add causal relationship between decisions."""
    
    # 词汇归一化:causes/caused → CAUSED
    if not isinstance(relationship_type, str):
        raise ValueError(f"Relationship type must be one of: {_CAUSAL_EDGE_TYPES}")
    relationship_type = _CAUSAL_EDGE_ALIASES.get(relationship_type.strip().upper())
    if relationship_type is None:
        raise ValueError(f"Relationship type must be one of: {_CAUSAL_EDGE_TYPES}")
    
    # 节点存在性 + 类型校验
    if source_decision_id not in self.nodes or target_decision_id not in self.nodes:
        return  # 静默跳过:不存在的节点
    source_node = self.nodes[source_decision_id]
    target_node = self.nodes[target_decision_id]
    if (source_node.node_type.lower() != "decision" or 
        target_node.node_type.lower() != "decision"):
        return  # 静默跳过:非 decision 类型
    
    edge = ContextEdge(
        source_id=source_decision_id,
        target_id=target_decision_id,
        edge_type=relationship_type,
        weight=1.0,
        metadata={"recorded_at": datetime.utcnow().isoformat()},
    )
    self._add_internal_edge(edge)
```

### 1.7 `get_causal_chain()` BFS 因果链追踪

**代码路径**：`semantica/context/context_graph.py:3882-3964`

```python
def get_causal_chain(
    self,
    decision_id: str,
    direction: str = "upstream",
    max_depth: int = 10
) -> List["Decision"]:
    if direction not in ["upstream", "downstream"]:
        raise ValueError("Direction must be 'upstream' or 'downstream'")
    
    # BFS 遍历
    visited = set()
    queue = deque([(decision_id, 0)])
    decisions = []
    
    while queue:
        current_id, depth = queue.popleft()
        if current_id in visited or depth > max_depth:
            continue
        visited.add(current_id)
        # ... 收集 decision 对象（含 causal_distance 元数据）
        
        # 沿 edge 遍历
        for edge in self.edges:
            if direction == "upstream":
                if edge.target_id == current_id and edge.edge_type.upper() in _CAUSAL_TRAVERSAL_TYPES:
                    if edge.source_id not in visited and depth < max_depth:
                        queue.append((edge.source_id, depth + 1))
            else:  # downstream
                if edge.source_id == current_id and edge.edge_type.upper() in _CAUSAL_TRAVERSAL_TYPES:
                    if edge.target_id not in visited and depth < max_depth:
                        queue.append((edge.target_id, depth + 1))
    
    # 按深度排序
    if direction == "upstream":
        decisions.sort(key=lambda d: d.metadata.get("causal_distance", 0), reverse=True)
    return decisions
```

### 1.8 `find_precedents_by_scenario()` 先例搜索（混合相似度算法）

**代码路径**：`semantica/context/context_graph.py:4434-4510`

```python
def find_precedents_by_scenario(
    self,
    scenario: str,
    category: Optional[str] = None,
    limit: int = 10,
    similarity_threshold: float = 0.5,
    include_superseded: bool = False,
    as_of: Optional[Union[str, int, float, datetime]] = None,
    **filters
) -> List[Dict[str, Any]]:
    """Find similar decisions (precedents) using hybrid search."""
    
    candidates = set()
    if category:
        candidates.update(self._decision_index.get(category, set()))
    else:
        candidates.update(self._decisions.keys())
    
    # 实体过滤
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
        
        # 内容相似度（embedding 余弦 / 字符串）
        content_sim = self._calculate_decision_content_similarity(scenario, decision)
        
        # 结构相似度（图结构）
        structural_sim = 0.0
        if self.config.get("advanced_analytics"):
            structural_sim = self._calculate_structural_similarity_for_decision(decision_id, scenario)
        
        # 混合相似度公式: content_sim 主导, structural_sim 辅助
        combined_sim = 0.7 * content_sim + 0.3 * structural_sim
        
        if combined_sim >= similarity_threshold:
            precedents.append({
                "decision": decision,
                "similarity": combined_sim,
                "content_similarity": content_sim,
                "structural_similarity": structural_sim,
            })
    
    precedents.sort(key=lambda x: x["similarity"], reverse=True)
    return precedents[:limit]
```

**关键公式**：`combined_sim = 0.7 × content_sim + 0.3 × structural_sim`. 内容相似度（embedding 余弦 / 字符串相似度）+ 图结构相似度（共享实体、共享边类型）的加权融合。

### 1.9 `trace_decision_causality()` 复杂因果链递归追踪

**代码路径**：`semantica/context/context_graph.py:4660-4820`

```python
def trace_decision_causality(
    self, decision_id: str, max_depth: int = 5, max_chains: Optional[int] = 10000
) -> List[Dict[str, Any]]:
    """DFS 递归追踪, 同时支持显式因果边(add_causal_relationship)和隐式因果(共享实体+时序)"""
    
    causal_chain = []
    chain_limit = float("inf") if max_chains is None else max_chains
    truncated = False
    
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
        current_decision = self._decisions[current_id]
        
        # 1. 显式因果(优先, add_causal_relationship 记录的)
        explicit_causes = incoming_causal_edges.get(current_id, [])
        explicit_cause_ids = {edge.source_id for edge in explicit_causes}
        for edge in explicit_causes:
            cause_id = edge.source_id
            hop = {
                "from": cause_id, "to": current_id,
                "type": edge.edge_type, "edge_weight": edge_weight,
            }
            cause_path = path + [hop]
            if not record_chain(cause_path):
                return
            trace_recursive(cause_id, depth + 1, cause_path, path_ids)
        
        # 2. 隐式因果(共享实体 + 时序先后, 跳过显式已覆盖)
        for entity in current_decision["entities"]:
            for other_decision_id in self._entity_index.get(entity, set()):
                if other_decision_id != current_id and other_decision_id not in explicit_cause_ids:
                    other_decision = self._decisions[other_decision_id]
                    if other_decision["timestamp"] < current_decision["timestamp"]:
                        # 添加为隐式影响边
                        ...
```

**设计要点**：
- **per-path 环检测**：同一决策可通过不同分支重新访问，全局 visited 会丢失合法链
- **显式因果优先于隐式因果**：用户记录的因果是 ground truth
- **链数上限 + truncated 标记**：防止密集图组合爆炸
- **反向索引一次构建**：避免每次递归都扫描整个边列表

### 1.10 双删除语义：`retract_node()` vs `purge_node()`

**代码路径**：`semantica/context/context_graph.py:2644-2911`

```python
def retract_node(self, node_id, reason=None, at=None, cascade=True) -> bool:
    """Retract a node: no longer active, but still visible in history.
    Closes the node's validity window rather than deleting it..."""
    at_iso = _normalize_temporal_input(at) or datetime.now(timezone.utc).isoformat()
    with self._lock:
        node = self.nodes.get(node_id)
        if node is None or ("node", node_id) in self._retractions:
            return False
        node.valid_until = _closing_valid_until(node.valid_until, at_iso)
        record = {"entity_id": node_id, "retracted_at": at_iso, "reason": reason}
        self._retractions[("node", node_id)] = record
        # 级联: 同时撤回所有 incident 边
        for edge in self._incident_edges(node_id):
            edge.valid_until = _closing_valid_until(edge.valid_until, at_iso)
            self._retractions[("edge", edge.edge_id)] = {...}
        self._emit_mutation("UPDATE_NODE", node_id, node_payload)
```

**关键设计**：
- **retraction**：仅关闭 `valid_until`，保留实体（`state_at(before_at)` 仍能查到），保证**可解释性**（决策历史不消失）
- **tombstone**：彻底清除，仅保留 `{entity_id, purged_at, reason}` 记录（不保留内容）
- **级联一致性**：retract 节点时同时 retract 所有 incident 边，避免"节点已失效但边仍显示 active"的悬挂状态

### 对 laew 的借鉴

**P0 — 决策追踪**：
- laew 的 SessionContext Agent 仅生成 Markdown 摘要，缺乏结构化决策记录
- 建议在 SQLite 新增 `decisions` 表，字段：id, category, scenario, reasoning, outcome, confidence, entities(JSON), decision_maker, timestamp, valid_from, valid_until
- 每次 Yolo 分类/工具选择都记录一条 Decision，作为审计依据

**P1 — 因果链追踪**：
- 借鉴 `trace_decision_causality()` 的"显式+隐式双轨"机制
- "为什么选 Bash 而不是 Read"作为显式因果边
- "共享会话上下文+时间接近"作为隐式因果推断

---

## 二、Rete 推理引擎核心代码路径

**代码路径**：`semantica/reasoning/rete_engine.py`（642 行）

### 2.1 节点类层次

```python
#: Line 154
@dataclass
class Token:
    """Token: 在 Rete 网络中流动的部分匹配"""
    facts: List[Fact] = field(default_factory=list)
    bindings: Dict[str, str] = field(default_factory=dict)

#: Line 172
@dataclass
class Match:
    """Pattern match"""
    rule: Rule
    facts: List[Fact]
    bindings: Dict[str, Any] = field(default_factory=dict)
    confidence: float = 1.0

#: Line 182
class ReteNode:
    def __init__(self, node_id: str):
        self.node_id = node_id
        self.children: List[ReteNode] = []
```

### 2.2 AlphaNode（单条件匹配）

**代码路径**：`semantica/reasoning/rete_engine.py:190-257`

```python
class AlphaNode(ReteNode):
    """Alpha node for single condition matching."""

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
            logger.warning(
                "AlphaNode %r failed to compile condition %r: %s; "
                "node will never match",
                node_id, pattern, e,
            )

    def add_fact(self, fact: Fact) -> Optional[Token]:
        bindings = self._matches(fact)
        if bindings is not None:
            token = Token(facts=[fact], bindings=dict(bindings))
            self.tokens.append(token)
            return token
        return None

    def _matches(self, fact: Fact) -> Optional[Dict[str, str]]:
        """使用预编译正则, RETE 会对每个 alpha node 评估大量 fact"""
        if self._compiled is None:
            return None
        fact_str = str(fact)
        try:
            match = self._compiled.match(fact_str)
        except Exception as e:
            logger.warning(...)
            return None
        if not match:
            return None
        return match.groupdict()
```

**关键优化**：**预编译正则**——Rete 会用同一 AlphaNode 评估大量 fact，每次重新编译代价高昂。

### 2.3 BetaNode（连接操作）

**代码路径**：`semantica/reasoning/rete_engine.py:260-288`

```python
class BetaNode(ReteNode):
    """Beta node for joining conditions."""

    def __init__(self, node_id: str, left: ReteNode, right: ReteNode):
        super().__init__(node_id)
        self.left = left
        self.right = right
        # 双侧 token 记忆: 新到达 token 与对侧所有已有 token 尝试连接
        self.left_tokens: List[Token] = []
        self.right_tokens: List[Token] = []

    def join(self, left_token: Token, right_token: Token) -> Optional[Token]:
        """Join a left token with a right token.
        Returns a new merged Token (facts concatenated in condition order,
        bindings unified) when the two tokens are consistent,
        otherwise None on a binding conflict."""
        merged = dict(left_token.bindings)
        for var, value in right_token.bindings.items():
            if var in merged and merged[var] != value:
                return None  # Binding conflict — cannot join.
            merged[var] = value
        return Token(
            facts=list(left_token.facts) + list(right_token.facts),
            bindings=merged,
        )
```

**关键设计**：
- **双侧 token 记忆**：实现经典 Rete 的"增量连接"——新 token 到达一侧时，与另一侧所有已有 token 尝试 join
- **Binding 冲突检测**：同一变量在不同 token 中绑定不同值时返回 None（合一失败）
- **facts 列表追加**：合并后 facts 按条件顺序排列，便于解释生成

### 2.4 TerminalNode（规则激活）

**代码路径**：`semantica/reasoning/rete_engine.py:291-301`

```python
class TerminalNode(ReteNode):
    """Terminal node representing rule activation."""

    def __init__(self, node_id: str, rule: Rule):
        super().__init__(node_id)
        self.rule = rule
        self.activations: List[Match] = []

    def activate(self, match: Match) -> None:
        """Activate rule."""
        self.activations.append(match)
```

### 2.5 正则合一算法 `_build_condition_regex()`

**代码路径**：`semantica/reasoning/rete_engine.py:47-85`

```python
def _build_condition_regex(
    pattern: str,
    initial_bindings: Optional[Dict[str, str]] = None,
) -> str:
    """Build an anchored regex string for a condition pattern.
    Splits the pattern on ?var placeholders, escaping the literal segments
    so surrounding parentheses/commas match literally. Variables become
    named groups (or backreferences when repeated); variables already
    present in initial_bindings are inlined as their literal value."""
    bindings = initial_bindings or {}
    segments = re.split(r"(\?\w+)", pattern)
    seen_vars: Set[str] = set()
    p_regex = ""
    for seg in segments:
        if seg.startswith("?"):
            var_name = seg[1:]
            if var_name in bindings:
                # 已绑定: 字面量匹配
                p_regex += re.escape(bindings[var_name])
            elif var_name in seen_vars:
                # 重复变量: 反向引用
                p_regex += f"(?P={var_name})"
            else:
                # 新变量: 命名捕获
                p_regex += f"(?P<{var_name}>.+?)"
                seen_vars.add(var_name)
        else:
            p_regex += re.escape(seg)
    return f"^{p_regex}$"
```

**示例**：
- 模式 `"Person(?x)"` → 正则 `^Person(?P<x>.+?)$`
- 模式 `"knows(?x, ?x)"` → 正则 `^knows(?P<x>.+?),\s*(?P=x)$`（反向引用确保同一人）
- 模式 `"Person(?x)"` + bindings={x: "Alice"} → `^PersonAlice$`（字面量）

### 2.6 `unify_condition()` 合一函数

**代码路径**：`semantica/reasoning/rete_engine.py:88-151`

```python
def unify_condition(
    condition: Any,
    fact: Fact,
    initial_bindings: Optional[Dict[str, str]] = None,
) -> Optional[Dict[str, str]]:
    """Unify a condition pattern against a fact."""
    bindings = dict(initial_bindings or {})
    pattern = condition if isinstance(condition, str) else str(condition)
    fact_str = str(fact)

    # 构建锚定正则(已绑定变量内联为字面量)
    p_regex = _build_condition_regex(pattern, bindings)

    try:
        match = re.match(p_regex, fact_str)
    except re.error as e:
        logger.warning(...)
        return None
    
    if not match:
        return None

    for var, value in match.groupdict().items():
        if var in bindings and bindings[var] != value:
            return None  # Binding conflict
        bindings[var] = value
    return bindings
```

### 2.7 Rete 网络构建 `_add_rule_to_network()`

**代码路径**：`semantica/reasoning/rete_engine.py:392-426`

```python
def _add_rule_to_network(self, rule: Rule) -> None:
    """Add rule to Rete network."""
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
            # BetaNode 同时作为两侧的 child, 确保 token 双向可达
            current.children.append(beta_node)
            alpha_nodes[i].children.append(beta_node)
            current = beta_node
        final_node = current
    else:
        final_node = alpha_nodes[0] if alpha_nodes else None

    # 3. 末尾创建 TerminalNode (规则激活)
    if final_node:
        node_id = f"terminal_{self.node_counter}"
        self.node_counter += 1
        terminal_node = TerminalNode(node_id, rule)
        final_node.children.append(terminal_node)
        self.network[node_id] = terminal_node
```

### 2.8 `_propagate_to_beta()` Token 传播

**代码路径**：`semantica/reasoning/rete_engine.py:470-495`

```python
def _propagate_to_beta(self, beta: BetaNode, source: ReteNode, token: Token) -> None:
    """Attempt joins at beta for a token arriving from one side.
    The incoming token is stored in the corresponding side's memory,
    then joined against every token already recorded on the opposite side."""
    if source is beta.left:
        beta.left_tokens.append(token)
        # 新 left token 与所有已有 right token 尝试连接
        for right_token in list(beta.right_tokens):
            merged = beta.join(token, right_token)
            if merged is not None:
                self._propagate_token(beta, merged)
    elif source is beta.right:
        beta.right_tokens.append(token)
        # 新 right token 与所有已有 left token 尝试连接
        for left_token in list(beta.left_tokens):
            merged = beta.join(left_token, token)
            if merged is not None:
                self._propagate_token(beta, merged)
```

### 2.9 `execute_matches()` 规则执行（带去重）

**代码路径**：`semantica/reasoning/rete_engine.py:545-609`

```python
def execute_matches(self, matches=None) -> List[Any]:
    matches = matches or self.match_patterns()
    results = []
    for match in matches:
        results.append(match.rule.conclusion)
        try:
            # 通过绑定的 Reasoner 触发规则的 actions (产生与 forward_chain() 一致的副作用)
            if self.reasoner is not None and (
                match.rule.actions or match.rule.handler is not None
            ):
                # 关键: _executed_activations 去重, 同一绑定不重复触发
                activation_key = _make_activation_key(
                    match.rule.rule_id,
                    match.bindings,
                    [(fact.fact_id, fact.predicate, fact.arguments) for fact in match.facts],
                )
                if activation_key not in self._executed_activations:
                    self._executed_activations.add(activation_key)
                    self.reasoner._fire_actions(match.rule, match.bindings)
        except Exception as e:
            self.logger.error(f"Error executing match: {e}")
    return results
```

### 对 laew 的借鉴

**P1 — 工具选择规则引擎**：
- laew 当前所有工具选择都由 LLM 决策
- 引入轻量 Rete 引擎（仅 642 行），处理简单规则如：
  - "Bash 命令包含 rm → 必须先 QC"
  - "工作目录在 git repo 内 → 允许 Bash 执行 git 命令"
  - "文件 > 10MB → 阻止 Read 直接读取"
- 这些规则无需 LLM 决策，毫秒级响应，降低成本

**P1 — 解释生成器**：
- BetaNode.join() 合并 facts 列表的设计天然支持"为什么选择这个工具"的解释生成
- laew 的 Quality-Check Agent 可用同一机制生成决策依据

---

## 三、Datalog 推理核心代码路径

**代码路径**：`semantica/reasoning/datalog_reasoner.py`（431 行）

### 3.1 数据结构

```python
@dataclass(frozen=True)
class DatalogFact:
    """Represents a ground truth fact."""
    predicate: str
    args: Tuple[str, ...]

class BodyAtom(NamedTuple):
    """Represents a single predicate condition in a rule's body."""
    predicate: str
    args: Tuple[str, ...]

@dataclass
class DatalogRule:
    """Represents a Horn clause rule."""
    head_predicate: str
    head_args: Tuple[str, ...]
    body: List[BodyAtom]
```

**设计要点**：`DatalogFact` 使用 `@dataclass(frozen=True)`——事实不可变，可哈希，自然用于 Set 去重。

### 3.2 变量识别 `_is_variable()`

**代码路径**：`semantica/reasoning/datalog_reasoner.py:180-182`

```python
def _is_variable(self, term: str) -> bool:
    """Variables strictly start with an uppercase letter."""
    return bool(term and term[0].isupper())
```

**简洁约定**：变量大写开头，常量小写开头。这是 Datalog 经典语法。

### 3.3 合一算法 `_unify()`（优化版）

**代码路径**：`semantica/reasoning/datalog_reasoner.py:184-215`

```python
def _unify(
    self,
    pattern_args: Tuple[str, ...],
    fact_args: Tuple[str, ...],
    bindings: Dict[str, str]
) -> Optional[Dict[str, str]]:
    """Unifies a rule atom's pattern with a concrete fact.
    Optimized to prevent unnecessary dictionary allocations."""
    if len(pattern_args) != len(fact_args):
        return None
    
    new_additions = {}  # 暂存新绑定, 仅在确实有新绑定时才合并
    
    for p_arg, f_arg in zip(pattern_args, fact_args):
        if self._is_variable(p_arg):
            if p_arg in bindings:
                # 已绑定: 必须一致
                if bindings[p_arg] != f_arg:
                    return None
            elif p_arg in new_additions:
                # 本轮新绑定: 必须一致
                if new_additions[p_arg] != f_arg:
                    return None
            else:
                # 全新变量: 记录
                new_additions[p_arg] = f_arg
        else:
            # 常量: 必须相等
            if p_arg != f_arg:
                return None
    
    # 性能优化: 仅在新绑定时分配新字典
    if new_additions:
        return {**bindings, **new_additions}
    return bindings  # 无新绑定, 返回原 dict 节省分配
```

**关键优化**：**避免不必要的字典分配**——如果合一未产生任何新绑定，返回原 dict 而非新 dict（Python 不可变 dict 优化 + 减少 GC 压力）。

### 3.4 `_parse_fact_string()` 事实解析

**代码路径**：`semantica/reasoning/datalog_reasoner.py:129-145`

```python
def _parse_fact_string(self, s: str) -> DatalogFact:
    """Parse 'predicate(arg1, arg2)' into a DatalogFact."""
    match = re.match(r'^\s*([a-zA-Z0-9_]+)\s*\(\s*([^)]+)\s*\)\s*\.?\s*$', s.strip())
    if not match:
        raise ValueError(f"Invalid fact syntax: {s}")
    
    predicate = match.group(1)
    args_str = match.group(2)
    args = tuple(arg.strip() for arg in args_str.split(','))
    
    # 关键: 事实必须是常量, 不能含变量(否则是查询/规则)
    for arg in args:
        if not arg:
            raise ValueError(f"Empty argument found in fact: {s}")
        if arg[0].isupper():
            raise ValueError(
                f"Facts must be constants only (no variables). "
                f"Found variable '{arg}' in {s}"
            )
    
    return DatalogFact(predicate, args)
```

### 3.5 `_parse_rule_string()` 规则解析

**代码路径**：`semantica/reasoning/datalog_reasoner.py:147-175`

```python
def _parse_rule_string(self, s: str) -> DatalogRule:
    """Parse 'head(X, Y) :- body1(X, Z), body2(Z, Y).' into a DatalogRule."""
    s = s.strip()
    if ":-" not in s:
        raise ValueError(f"Invalid rule syntax (missing ':-'): {s}")
    
    head_str, body_str = s.split(":-", 1)
    head_str = head_str.strip()
    body_str = body_str.strip().rstrip('.')
    
    head_match = re.match(r'^([a-zA-Z0-9_]+)\s*\(\s*([^)]+)\s*\)$', head_str)
    if not head_match:
        raise ValueError(f"Invalid rule head syntax: {head_str}")
    head_pred = head_match.group(1)
    head_args = tuple(arg.strip() for arg in head_match.group(2).split(','))
    
    body = []
    atom_matches = re.findall(r'([a-zA-Z0-9_]+)\s*\(\s*([^)]+)\s*\)', body_str)
    if not atom_matches:
        raise ValueError(f"No valid body atoms found in rule: {s}")
    for pred, args_str in atom_matches:
        args = tuple(arg.strip() for arg in args_str.split(','))
        body.append(BodyAtom(pred, args))
    
    return DatalogRule(head_pred, head_args, body)
```

### 3.6 半朴素不动点求值 `derive_all()`

**代码路径**：`semantica/reasoning/datalog_reasoner.py:242-293`

```python
def derive_all(self) -> List[str]:
    """Executes bottom-up semi-naive evaluation until fixpoint is reached."""
    if self._derived:
        return [f"{f.predicate}({', '.join(f.args)})" for f in self._all_facts]
    
    tracking_id = self.progress_tracker.start_tracking(
        module="reasoning", submodule="DatalogReasoner",
        message="Starting semi-naive fixpoint evaluation"
    )
    
    iteration = 0
    newly_derived_count = 0
    try:
        # 初始 delta = 所有事实
        self._delta_new = self._all_facts.copy()
        
        while self._delta_new:
            iteration += 1
            # 滚动 delta
            self._delta_old = self._delta_new
            self._delta_new = set()
            
            # 关键: 按 predicate 建索引, 加速 _apply_rule 中的查找
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
    finally:
        self.progress_tracker.stop_tracking(
            tracking_id, status="completed",
            message=f"Fixpoint reached in {iteration} iterations. {newly_derived_count} new facts derived."
        )
    
    return [f"{f.predicate}({', '.join(f.args)})" for f in self._all_facts]
```

### 3.7 `_apply_rule()` 半朴素策略（核心优化）

**代码路径**：`semantica/reasoning/datalog_reasoner.py:295-339`

```python
def _apply_rule(
    self, rule: DatalogRule, delta_index: Optional[Dict[str, Set[DatalogFact]]] = None
) -> Set[DatalogFact]:
    """Evaluates a single rule.
    Uses semi-naive strategy if delta_index is provided,
    otherwise falls back to naive evaluation."""
    results = set()
    
    if not rule.body:
        # 空 body: 直接实例化 head
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
            
            # 关键: 增量位置用 delta, 其他位置用全量
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
                break  # 早退: 当前路径无解
        
        for final_bindings in bindings_list:
            head_fact = self._instantiate_fact(
                rule.head_predicate, rule.head_args, final_bindings
            )
            if head_fact:
                results.add(head_fact)
    
    return results
```

**半朴素求值策略**：
- 每轮只考虑**至少一个新事实**参与推导
- 假设规则 body 有 N 个原子，则依次将每个位置作为"增量位置"，其他位置使用全量 fact 索引
- 保证：每轮至少有一个体原子使用新事实，避免重复推导；同时保证终止性

### 3.8 `query()` 查询接口

**代码路径**：`semantica/reasoning/datalog_reasoner.py:344-402`

```python
def query(self, pattern: str, bindings: dict = None) -> List[dict]:
    """Queries the derived fact set. Automatically runs derive_all() if rules exist."""
    if self._rules and not self._derived:
        self.derive_all()
    
    match = re.match(r'^\s*([a-zA-Z0-9_]+)\s*\(\s*([^)]+)\s*\)\s*\.?\s*$', pattern.strip())
    if not match:
        raise ValueError(f"Invalid query syntax: {pattern}")
    
    pred = match.group(1)
    raw_args = tuple(arg.strip() for arg in match.group(2).split(','))
    
    query_vars = {}  # 记录 query 中哪些位置是变量
    pattern_args = []
    for i, arg in enumerate(raw_args):
        if arg.startswith('?'):
            var_name = arg[1:]
            # 关键: ?x 和 X 等价, 内部统一为大写开头
            internal_var = var_name[0].upper() + var_name[1:]
            query_vars[i] = (var_name, internal_var)
            pattern_args.append(internal_var)
        elif self._is_variable(arg):
            query_vars[i] = (arg, arg)
            pattern_args.append(arg)
        else:
            pattern_args.append(arg)
    
    # 处理初始绑定(已知值)
    initial_bindings = {}
    for k, v in (bindings or {}).items():
        internal_k = k[0].upper() + k[1:] if k and not k[0].isupper() else k
        initial_bindings[internal_k] = v
    
    # 将已绑定变量替换为字面量
    for i, arg in enumerate(pattern_args):
        if self._is_variable(arg) and arg in initial_bindings:
            pattern_args[i] = initial_bindings[arg]
    
    results = []
    candidates = self._fact_index.get(pred, set())
    for fact in candidates:
        match_bindings = self._unify(tuple(pattern_args), fact.args, {})
        if match_bindings is not None:
            result_row = {}
            for idx, (orig_var, internal_var) in query_vars.items():
                if internal_var in match_bindings:
                    result_row[orig_var] = match_bindings[internal_var]
                elif internal_var in initial_bindings:
                    result_row[orig_var] = initial_bindings[internal_var]
            if result_row and result_row not in results:
                results.append(result_row)
    return results
```

**对 laew 的借鉴**：

**P1 — 任务分类规则化**：
- 当前 laew 完全用 LLM 分类任务为 simple/medium/hard
- 用 Datalog 表达简单规则，如：
  ```prolog
  hard_task(T) :- file_count(T, N), N > 10.
  hard_task(T) :- has_keywords(T, "重构";"迁移";"架构").
  simple_task(T) :- not has_keywords(T, _), length(T) < 50.
  ```
- 简单分类走规则（毫秒级，零成本），复杂情况才走 LLM

---

## 四、W3C PROV-O 溯源核心代码路径

### 4.1 `ProvenanceEntry` 数据类

**代码路径**：`semantica/provenance/schemas.py:36-200`

```python
@dataclass
class ProvenanceEntry:
    """W3C PROV-O compliant provenance entry."""
    
    # W3C PROV-O core entities
    entity_id: str
    entity_type: str
    activity_id: str
    agent_id: str = "semantica"
    
    # Accountability-scoped agent typing
    agent_type: str = "software_agent"  # "person" | "software_agent" | "organization"
    is_automated: bool = True
    role: Optional[str] = None  # prov:hadRole
    
    # Audit-grade source tracking
    source_document: str = ""
    source_location: Optional[str] = None
    source_quote: Optional[str] = None
    
    # Temporal tracking
    timestamp: str = field(default_factory=lambda: utc_now_iso())
    first_seen: Optional[str] = None
    last_updated: Optional[str] = None
    
    # Quality metrics
    confidence: float = 1.0
    checksum: Optional[str] = None
    
    # Hash-chain linkage
    sequence_id: Optional[int] = None
    previous_checksum: Optional[str] = None
    
    # W3C PROV-O chain of custody
    parent_entity_id: Optional[str] = None
    used_entities: List[str] = field(default_factory=list)
    
    # Versioning vs. derivation (双重版本语义)
    previous_version_id: Optional[str] = None
    derived_from_id: Optional[str] = None
    
    # Activity timing
    activity_started_at_time: Optional[str] = None
    activity_ended_at_time: Optional[str] = None
    
    # Delegation
    acted_on_behalf_of: Optional[str] = None
    informed_by_activities: List[str] = field(default_factory=list)
    
    # Bitemporal
    valid_from: Optional[str] = None
    valid_until: Optional[str] = None
    revision_type: Optional[str] = None
    supersedes: Optional[str] = None
    
    # Bundle membership
    bundle_id: Optional[str] = None
    
    # Invalidation (tombstone)
    invalidated: bool = False
    invalidated_at_time: Optional[str] = None
    invalidated_by: Optional[str] = None
    invalidation_reason: Optional[str] = None
    
    # ... 其他字段(40+ 字段)
```

**W3C PROV-O 映射表**：

| 字段 | PROV-O 映射 |
|------|-------------|
| `entity_id` | `prov:Entity` |
| `activity_id` | `prov:Activity` |
| `agent_id`/`agent_type` | `prov:Agent` (Person\|SoftwareAgent\|Organization) |
| `role` | `prov:hadRole` (via prov:qualifiedAssociation) |
| `parent_entity_id` | `prov:wasDerivedFrom` (legacy) |
| `derived_from_id` | `prov:wasDerivedFrom` (true cross-source) |
| `previous_version_id` | prior version of same fact (correction/versioning) |
| `used_entities` | `prov:used` |
| `timestamp` | `prov:generatedAtTime` |
| `invalidated`/`invalidated_at_time` | `prov:Invalidation` |
| `activity_started_at_time`/`activity_ended_at_time` | `prov:startedAtTime`/`prov:endedAtTime` |
| `acted_on_behalf_of` | `prov:actedOnBehalfOf` |
| `bundle_id` | `prov:Collection`/`prov:Bundle` |

### 4.2 SHA-256 哈希链 `compute_checksum()`

**代码路径**：`semantica/provenance/integrity.py:27-116`

```python
def compute_checksum(entry: Any) -> str:
    """Compute SHA-256 checksum for a provenance entry.
    Creates a deterministic checksum based on critical provenance fields
    to detect any tampering or corruption of provenance data.
    
    Includes previous_checksum (issue #825, Part A item 2), which chains
    each entry to the prior entry in insertion order. Wholesale deletion
    of a row breaks the chain for the entry that used to follow it.
    Also includes agent_id/agent_type and lineage-link fields.
    
    Deliberately excludes entity_id: entity_id is the storage primary key,
    and track_entity()'s versioning archives a prior value by copying it
    to a new entity_id (e.g. "X" -> "X:v:..."). If entity_id were hashed,
    that relabeling would change the archived copy's checksum, permanently
    orphaning any later entry whose previous_checksum had already chained
    from the pre-relabel value — a false-positive "broken chain" for a
    legitimate rename, not tampering."""
    
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
    else:
        used_entities = getattr(entry, "used_entities", None) or []
        data = (
            f"{entry.entity_type}"
            f"{entry.activity_id}"
            f"{getattr(entry, 'agent_id', '') or ''}"
            # ... 同样的字段
        )
    return hashlib.sha256(data.encode('utf-8')).hexdigest()
```

**哈希字段设计要点**：

| 包含字段 | 原因 |
|---------|------|
| `entity_type`, `activity_id`, `agent_id`/`agent_type` | 身份字段：识别 entry 来源 |
| `source_document`, `source_location`, `source_quote` | 审计级源追踪 |
| `timestamp`, `confidence` | 时序与质量 |
| `parent_entity_id`, `previous_version_id`, `derived_from_id` | lineage 字段 |
| `used_entities` | 派生源 |
| `previous_checksum` | 哈希链 |
| `invalidated`/`invalidated_at_time`/`invalidated_by`/`invalidation_reason` | 失效追踪 |
| **故意排除 `entity_id`** | 版本归档重命名时(`X` → `X:v:...`)不破坏哈希链 |

### 4.3 `track_entity()` 原子事务与版本归档

**代码路径**：`semantica/provenance/manager.py:268-420`

```python
def track_entity(
    self, entity_id: str, source: str, metadata=None, _conn=None, **kwargs
) -> Optional[ProvenanceEntry]:
    """Atomic transaction (issue #807):
    One connection scoped to the full duration of track_entity so all
    retrieve and store operations share a single transaction. With BEGIN
    IMMEDIATE, concurrent calls serialize during retrieval so no intervening
    versions are lost. If any step raises, the whole operation rolls back."""
    
    existing = None
    entry = None
    try:
        with self._get_or_create_transaction(_conn) as conn:
            existing = self.storage._retrieve_with_conn(conn, entity_id)
            parent_id = kwargs.get("parent_entity_id")
            
            # 跨源派生父节点推断
            if not parent_id and source and isinstance(source, str):
                try:
                    source_entity = self.storage._retrieve_with_conn(conn, source)
                    if source_entity:
                        parent_id = source
                except Exception:
                    pass
            
            explicit_parent_supplied = parent_id is not None
            archived_history_id = None
            
            if existing:
                # 关键: 版本归档 (复制旧条目, 主键重命名为 f"{entity_id}:v:{last_updated}")
                history_entry = copy.deepcopy(existing)
                base_history_id = f"{entity_id}:v:{existing.last_updated}"
                history_id = base_history_id
                counter = 1
                while self.storage._retrieve_with_conn(conn, history_id):
                    history_id = f"{base_history_id}:{counter}"
                    counter += 1
                history_entry.entity_id = history_id
                # 关键: 纯重命名, checksum/sequence_id/previous_checksum 不变
                # (compute_checksum 排除 entity_id, 所以安全)
                self.storage._store_with_conn(conn, history_entry)
                archived_history_id = history_id
                if not explicit_parent_supplied:
                    parent_id = history_id
            
            entry = ProvenanceEntry(
                entity_id=entity_id,
                entity_type=kwargs.get("entity_type", "entity"),
                # ... 字段填充
            )
            
            # 双版本语义:
            # previous_version_id: 修正/版本化(总是设置当有归档)
            entry.previous_version_id = archived_history_id
            # derived_from_id: 跨源派生(仅当显式 parent)
            if explicit_parent_supplied:
                entry.derived_from_id = parent_id
            
            if archived_history_id and explicit_parent_supplied:
                entry.used_entities.append(archived_history_id)
            
            self._save_entry(entry, _conn=conn, _raise_on_error=True)
    except Exception as e:
        # 事务回滚; 已有 entry 时返回 deepcopy(existing) (恢复状态)
        if _conn is not None:
            raise
        if existing is not None:
            return copy.deepcopy(existing)
        return None
    return entry
```

**关键设计**：
- **BEGIN IMMEDIATE 事务**：写锁而非读锁，避免并发插入产生间隙
- **归档时仅重命名 entity_id**：checksum/sequence_id/previous_checksum 完全保留，保证哈希链不中断
- **双版本语义独立**：`previous_version_id`（修正旧版本）vs `derived_from_id`（跨源派生），互不混淆

### 4.4 `verify_chain()` 哈希链验证

**代码路径**：`semantica/provenance/manager.py:1450-1520`

```python
def verify_chain(self) -> Dict[str, Any]:
    """Verify the hash chain across all provenance entries.
    
    Sorts entries by sequence_id (global insertion order) and checks
    three things:
    1. each entry's own checksum matches its content;
    2. each entry's previous_checksum matches the checksum of the entry that
       precedes it;
    3. sequence_id is exactly the predecessor's plus one (no gap, no duplicate)."""
    
    entries = sorted(
        (e for e in self.storage.retrieve_all() if e.sequence_id is not None),
        key=lambda e: e.sequence_id,
    )
    
    broken_links: List[Dict[str, Any]] = []
    expected_previous: Optional[str] = None
    expected_sequence: Optional[int] = None
    
    for entry in entries:
        if not verify_checksum(entry):
            broken_links.append({
                "entity_id": entry.entity_id,
                "sequence_id": entry.sequence_id,
                "reason": "checksum_mismatch",
            })
        else:
            sequence_gap = (
                expected_sequence is not None
                and entry.sequence_id != expected_sequence + 1
            )
            checksum_break = entry.previous_checksum != expected_previous
            if sequence_gap or checksum_break:
                broken_links.append({
                    "entity_id": entry.entity_id,
                    "sequence_id": entry.sequence_id,
                    "reason": "chain_break",
                    "expected_previous_checksum": expected_previous,
                    "actual_previous_checksum": entry.previous_checksum,
                    "expected_sequence_id": (
                        expected_sequence + 1 if expected_sequence is not None else None
                    ),
                })
        
        # 关键: 即使 entry 被标记为 broken, 仍然用它推进 expected_*
        # 否则单个损坏 entry 会导致后续所有 entry 都被错误标记
        expected_previous = entry.checksum
        expected_sequence = entry.sequence_id
    
    return {
        "valid": len(broken_links) == 0,
        "total_entries": len(entries),
        "broken_links": broken_links,
    }
```

**三重检测**：
1. **checksum_mismatch**：单条 entry 内容被篡改
2. **sequence_gap**：sequence_id 不连续（说明有行被硬删除）
3. **checksum_break**：previous_checksum 与前一行的 checksum 不匹配（哈希链断裂）

**对 laew 的借鉴**：

**P0 — 工具调用溯源**：
- 当前 laew 完全无溯源机制
- 在 `agent/tools/` 层包装每个工具调用，自动记录 `ProvenanceEntry`：
  - `entity_id`：文件名/URL/命令
  - `activity_id`：read/write/bash
  - `agent_id`：当前 agent 名称（如 `LsmAgentEmergentWork-Main-Work`）
  - `source_document`：调用的源 prompt
  - `used_entities`：输入参数
- 实现 `compute_checksum()` SHA-256 链，保证审计完整性
- 利用 SQLite 即可，无需新基础设施

**P0 — 双删除语义**：
- 当前 laew 的 SQLite 没有"撤回"概念，删了就删了
- 引入 retraction（仅设置 valid_until）支持撤销工具结果而不丢失历史

---

## 五、摄取管线核心代码路径

### 5.1 统一摄取分发函数 `ingest()`

**代码路径**：`semantica/ingest/methods.py:1491-1644`

```python
def ingest(
    sources: Union[List[Union[str, Path]], str, Path],
    source_type: Optional[str] = None,
    method: Optional[str] = None,
    **kwargs,
) -> Dict[str, Any]:
    """统一摄取入口; 自动检测 source_type 或按指定类型分发"""
    
    # 自动检测 source_type
    if not source_type:
        if isinstance(sources, (str, Path)):
            source_str = str(sources).lower()
            if source_str.startswith(("http://", "https://")):
                # URL: 进一步判断是 feed 还是 web
                if any(ext in source_str for ext in [".xml", "/feed", "/rss", "/atom"]):
                    source_type = "feed"
                else:
                    source_type = "web"
            elif source_str.startswith(("postgresql://", "mysql://", "sqlite://", ...)):
                source_type = "db"
            elif source_str.startswith(("https://github.com", "https://gitlab.com")):
                source_type = "repo"
            elif source_str.endswith((".ttl", ".owl", ".rdf", ".jsonld", ".n3", ".nt")):
                source_type = "ontology"
            elif source_str.endswith((".parquet", ".pq")):
                source_type = "parquet"
            elif source_str.endswith((".arrow", ".feather", ".ipc")):
                source_type = "arrow"
            elif source_str.endswith(".xml"):
                source_type = "xml"
            else:
                source_type = "file"
        # ... 列表检测分支
    
    # 分发到具体摄取器
    if source_type == "file":
        return {"files": ingest_file(sources, method=method or "file", **kwargs)}
    elif source_type == "web":
        return {"content": ingest_web(sources, method=method or "url", **kwargs)}
    elif source_type == "feed":
        return {"feeds": ingest_feed(sources, method=method or "rss", **kwargs)}
    elif source_type == "stream":
        if isinstance(sources, dict):
            return {"processor": ingest_stream(sources, method=method or "kafka", **kwargs)}
        else:
            raise ProcessingError("Stream ingestion requires configuration dictionary")
    # ... 其他分支
```

**统一接口设计**：通过 `source_type` 字符串分发，支持 12+ 种数据源。

### 5.2 `FileTypeDetector` 三法检测

**代码路径**：`semantica/ingest/file_ingestor.py:81-220`

```python
class FileTypeDetector:
    """File type detection using three methods:
    1. File extension analysis (fastest)
    2. MIME type detection (if file exists)
    3. Magic number (file signature) analysis (most reliable)"""

    def detect_type(
        self, file_path: Union[str, Path], content: Optional[bytes] = None
    ) -> str:
        file_path = Path(file_path)
        
        # Method 1: 扩展名检测 (最快)
        extension = file_path.suffix.lstrip(".").lower()
        if extension:
            self.logger.debug(f"Detected file type by extension: {extension}")
            return extension
        
        # Method 2: MIME 类型检测 (文件存在时)
        if file_path.exists():
            mime_type, _ = mimetypes.guess_type(str(file_path))
            if mime_type:
                ext = mimetypes.guess_extension(mime_type)
                if ext:
                    detected_type = ext.lstrip(".").lower()
                    return detected_type
        
        # Method 3: 魔数分析 (最可靠, 需 content)
        if content:
            file_type = self._detect_by_magic_numbers(content)
            if file_type:
                return file_type
        
        return "unknown"

    def _detect_by_magic_numbers(self, content: bytes) -> Optional[str]:
        """通过文件头字节序列识别格式"""
        if len(content) < 4:
            return None
        
        magic_numbers = {
            b"\x25\x50\x44\x46": "pdf",  # PDF
            b"%PDF": "pdf",              # PDF (文本头)
            b"\x50\x4b\x03\x04": "zip",  # ZIP/DOCX/XLSX/PPTX
            b"\x89\x50\x4e\x47": "png",  # PNG
            b"\xff\xd8\xff": "jpg",      # JPEG
            b"\x47\x49\x46\x38": "gif",  # GIF
            b"PAR1": "parquet",          # Apache Parquet
            b"ARROW1\x00\x00": "arrow",  # Apache Arrow IPC
        }
        
        for magic_bytes, file_type in magic_numbers.items():
            if content.startswith(magic_bytes):
                return file_type
        return None
```

**三级回退策略**：
1. **扩展名**：最快但易被伪造（用户改后缀名绕过）
2. **MIME 类型**：基于文件内容嗅探（需读文件头）
3. **魔数分析**：最可靠（基于文件头字节特征）

### 5.3 `FileIngestor.ingest_directory()` 批量摄取

**代码路径**：`semantica/ingest/file_ingestor.py:468-551`

```python
def ingest_directory(
    self, directory_path: Union[str, Path], recursive: bool = True, **filters
) -> List[FileObject]:
    directory_path = Path(directory_path)
    
    tracking_id = self.progress_tracker.start_tracking(
        file=str(directory_path), module="ingest", submodule="FileIngestor",
        message=f"Directory: {directory_path.name}",
    )
    
    try:
        if not directory_path.exists():
            raise ValidationError(f"Directory not found: {directory_path}")
        if not directory_path.is_dir():
            raise ValidationError(f"Path is not a directory: {directory_path}")
        
        # 1. 扫描目录
        files = self.scan_directory(directory_path, recursive=recursive, **filters)
        
        file_objects = []
        total_files = len(files)
        self.progress_tracker.update_tracking(tracking_id, message=f"Processing {total_files} files")
        
        # 2. 逐文件处理
        for idx, file_info in enumerate(files, 1):
            try:
                file_obj = self.ingest_file(file_info["path"], **file_info)
                file_objects.append(file_obj)
                
                self.progress_tracker.update_progress(
                    tracking_id, processed=idx, total=total_files,
                    message=f"Processing file {idx}/{total_files}: {Path(file_info['path']).name}"
                )
                
                if self._progress_callback:
                    self._progress_callback(idx, total_files, file_obj)
            except Exception as e:
                self.logger.error(f"Failed to ingest file {file_info['path']}: {e}")
                if self.config.get("fail_fast", False):
                    raise ProcessingError(f"Failed to ingest file: {e}")
        
        self.progress_tracker.stop_tracking(tracking_id, status="completed", message=f"Ingested {len(file_objects)} files")
        return file_objects
    except Exception as e:
        self.progress_tracker.stop_tracking(tracking_id, status="failed", message=str(e))
        raise
```

**特性**：
- **`fail_fast` 选项**：默认宽容（失败跳过继续），可配置严格（任一失败立即抛错）
- **进度回调**：支持 `set_progress_callback()` 自定义进度报告
- **进度跟踪**：通过 `progress_tracker` 集成全局进度系统

### 5.4 摄取器清单（17+ 种）

| 摄取器 | 文件路径 | 数据源 |
|--------|---------|--------|
| `FileIngestor` | `ingest/file_ingestor.py` | 本地文件 + S3/GCS/Azure |
| `WebIngestor` | `ingest/web_ingestor.py` | 网页（robots.txt/sitemap） |
| `RESTIngestor` | `ingest/api_ingestor.py` | REST API |
| `PublicAPIIngestor` | `ingest/public_api_ingestor.py` | 无认证公共 API |
| `FeedIngestor` | `ingest/feed_ingestor.py` | RSS/Atom |
| `StreamIngestor` | `ingest/stream_ingestor.py` | Kafka/RabbitMQ/Kinesis/Pulsar |
| `RepoIngestor` | `ingest/repo_ingestor.py` | Git 仓库 |
| `EmailIngestor` | `ingest/email_ingestor.py` | IMAP/POP3 |
| `DBIngestor` | `ingest/db_ingestor.py` | PostgreSQL/MySQL/SQLite/Oracle |
| `SnowflakeIngestor` | `ingest/snowflake_ingestor.py` | Snowflake |
| `DatabricksIngestor` | `ingest/databricks_ingestor.py` | Unity Catalog + Delta Lake |
| `SAPIngestor` | `ingest/sap_ingestor.py` | SAP OData |
| `SalesforceIngestor` | `ingest/salesforce_ingestor.py` | Salesforce CRM |
| `ParquetIngestor` | `ingest/parquet_ingestor.py` | Apache Parquet |
| `ArrowIngestor` | `ingest/arrow_ingestor.py` | Arrow IPC/Feather |
| `XMLIngestor` | `ingest/xml_ingestor.py` | XML 文件 |
| `MCPIngestor` | `ingest/mcp_ingestor.py` | MCP 资源/工具 |

**对 laew 的借鉴**：

**P2 — 文件类型检测**：
- laew 当前用扩展名识别文件类型，可魔数分析加固（防伪装 .txt 实为 .pdf）

**P2 — 增量模式**：
- Semantica 通过 `PipelineStep.delta_mode` + `base_version_id`/`target_version_id` 支持增量处理
- laew 的 SessionContext 可借鉴：基于版本号仅注入增量变更

---

## 六、语义抽取核心代码路径

### 6.1 `NERExtractor` 多方法回退链

**代码路径**：`semantica/semantic_extract/ner_extractor.py:87-167`

```python
class NERExtractor:
    def __init__(
        self,
        method: Union[str, List[str]] = "ml",  # 支持单方法或多方法列表
        entity_types: Optional[List[str]] = None,
        **config
    ):
        # method 支持的取值:
        # "pattern" - 基于正则模式
        # "regex" - 通用正则
        # "rules" - 基于语言规则
        # "ml" - spaCy (默认)
        # "huggingface" - HuggingFace NER 模型
        # "llm" - LLM 驱动抽取
        # 也可传 list, 形成回退链
        
        self.method = method if isinstance(method, list) else [method]
        
        # 关键: ML 方法在初始化时验证 spaCy 运行时
        self._ml_runtime_usable = True
        if "ml" in self.method and SPACY_AVAILABLE:
            try:
                from .methods import load_spacy_model
                load_spacy_model(self.model_name)  # 加载并缓存
            except OSError:
                self.logger.warning(f"spaCy model {self.model_name} not found. ML method will fallback.")
            except Exception as exc:
                self._ml_runtime_usable = False
```

**回退链设计**：`["llm", "ml", "rules"]` 表示先试 LLM，失败回退到 ML，再失败回退到规则。

### 6.2 `extract()` 批量并行处理

**代码路径**：`semantica/semantic_extract/ner_extractor.py:167-310`

```python
def extract(self, text, pipeline_id=None, **kwargs):
    """批量抽取入口; 支持 list[str] / list[dict]"""
    
    if isinstance(text, list):
        tracking_id = self.progress_tracker.start_tracking(...)
        
        # 根据方法自动选择 worker 数
        from .config import resolve_max_workers
        max_workers = resolve_max_workers(
            explicit=kwargs.get("max_workers"),
            local_config=self.config,
            methods=self.method,
        )
        
        # 关键: ThreadPoolExecutor 并行执行
        with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
            future_to_idx = {
                executor.submit(process_item, idx, item): idx
                for idx, item in enumerate(text)
            }
            
            for future in concurrent.futures.as_completed(future_to_idx):
                idx, entities = future.result()
                results[idx] = entities
                # ... 进度更新
```

**自适应并发**：`resolve_max_workers()` 根据 method 类型（LLM 调 API → 少 worker；spaCy 本地 → 多 worker）自动调整。

### 6.3 加权置信度公式 `calculate_weighted_confidence()`

**代码路径**：`semantica/semantic_extract/methods.py:601-648`

```python
def calculate_weighted_confidence(
    item_type: str,
    original_confidence: float,
    valid_types: Optional[List[str]] = None,
    item_text: Optional[str] = None,
    weight_method: float = 0.5,
    weight_similarity: float = 0.5
) -> float:
    """Final Score = (w_m * original_confidence) + (w_s * max(label_sim, content_sim))
    
    Args:
        weight_method: 方法置信度权重 (默认 0.5)
        weight_similarity: 相似度权重 (默认 0.5)
    """
    if not valid_types:
        return original_confidence
    
    # 相似度 1: 标签 vs 有效类型 (e.g., "PERSON" vs "Artist")
    label_similarity = calculate_similarity(item_type, valid_types)
    
    # 相似度 2: 内容 vs 有效类型 (e.g., "Picasso" vs "Artist")
    content_similarity = 0.0
    if item_text:
        content_similarity = calculate_similarity(item_text, valid_types)
    
    # 取最佳相似度
    best_similarity = max(label_similarity, content_similarity)
    
    # 归一化权重(确保和为 1)
    total_weight = weight_method + weight_similarity
    if total_weight <= 0:
        return original_confidence
    
    w_m = weight_method / total_weight
    w_s = weight_similarity / total_weight
    
    final_score = (w_m * original_confidence) + (w_s * best_similarity)
    return max(0.0, min(1.0, final_score))
```

**公式**：`Score = (w_m × Method_Confidence) + (w_s × max(Label_Sim, Content_Sim))`
**默认权重**：method=0.5, similarity=0.5（可配置偏向）

### 6.4 相似度五级回退 `find_best_match_index()`

**代码路径**：`semantica/semantic_extract/methods.py:320-485`

```python
def find_best_match_index(text: str, candidates: List[str]) -> Tuple[int, float]:
    """Hybrid similarity: Exact -> Synonym -> Substring -> Embeddings -> Vector -> Fuzzy."""
    
    text_lower = text.lower().strip()
    candidates_lower = [c.lower().strip() for c in candidates]
    
    # 1. 精确匹配 (最快, 分数 1.0)
    try:
        idx = candidates_lower.index(text_lower)
        return idx, 1.0
    except ValueError:
        pass
    
    # 1b. 同义词匹配 (快速启发式, 分数 0.95)
    synonyms = _ENTITY_SYNONYMS
    if text_lower in synonyms:
        for syn in synonyms[text_lower]:
            if syn in candidates_lower:
                return candidates_lower.index(syn), 0.95
    
    # 2. 子串匹配 (快速, 分数 0.88-1.0)
    for i, cand in enumerate(candidates_lower):
        if text_lower in cand or cand in text_lower:
            ratio = min(len(text_lower), len(cand)) / max(len(text_lower), len(cand))
            score = 0.9 * ratio + 0.1  # base + length bonus
            # word boundary 匹配再加分
            if word_pat and word_pat.search(cand):
                score = max(score, 0.88)
    
    # 3. 文本嵌入 (高精度语义, 仅当 <0.85 时调用)
    if best_score >= 0.85:
        return best_idx, float(best_score)
    
    embedder = get_text_embedder()
    if embedder:
        # 批量嵌入: [text, cand1, cand2, ...]
        texts_to_embed = [text] + [c for c, i in valid_cands_with_idx]
        embeddings = list(embedder.embed_batch(texts_to_embed))
        # 向量化余弦相似度
        text_norm = np.linalg.norm(text_emb)
        cand_matrix = np.array(cand_embs)
        cand_norms = np.linalg.norm(cand_matrix, axis=1)
        sims = np.dot(cand_matrix, text_emb) / (cand_norms * text_norm)
        max_sim = float(np.max(sims))
    
    # 4. spaCy 向量相似度 (legacy fallback)
    if best_score < 0.9:
        nlp = get_nlp_model()
        doc = nlp(text)
        for i, candidate in enumerate(candidates):
            cand_doc = nlp(candidate)
            score = doc.similarity(cand_doc)
    
    # 5. Fuzzy 字符串匹配 (difflib, 终极兜底)
    if best_score < 0.9:
        for i, cand in enumerate(candidates_lower):
            score = difflib.SequenceMatcher(None, text_lower, cand).ratio()
    
    return best_idx, float(best_score)
```

**五级回退设计**：
1. **精确匹配**（1.0）→ 2. **同义词**（0.95）→ 3. **子串**（0.88-1.0）→ 4. **嵌入余弦**→ 5. **spaCy 向量**→ 6. **Fuzzy**

**性能优化**：每级计算后立即判断是否满足阈值，提前退出避免后续昂贵计算。

### 6.5 LLM 提示词模板 `extract_entities_llm()`

**代码路径**：`semantica/semantic_extract/methods.py:1092-1115`

```python
prompt = f"""Extract named entities from the provided text.
Return the result as a JSON object with an "entities" key containing the list of entities.
Each entity should have 'text', 'label', and 'confidence' fields.

IMPORTANT: 
- Return a FLAT LIST of entities. 
- DO NOT group entities by type.
- The output structure must exactly match: {{ "entities": [ {{ "text": "...", "label": "...", "confidence": ... }}, ... ] }}

Example output (JSON format only):
{{
  "entities": [
    {{"text": "Entity Name", "label": "CATEGORY", "confidence": 0.95}},
    {{"text": "Another Entity", "label": "OTHER_CATEGORY", "confidence": 0.90}}
  ]
}}

Instructions:
1. Extract entities ONLY from the text provided below.
2. Do not include any entities from the example above.
3. {entity_types_instruction}

Text to extract from:
{text}"""

# 使用 Pydantic schema + structured output
result_obj = llm.generate_typed(prompt, schema=EntitiesResponse, **kwargs)
```

### 6.6 长文本分块 `_extract_entities_chunked()`

**代码路径**：`semantica/semantic_extract/methods.py:1169-1228`

```python
def _extract_entities_chunked(
    text: str, provider: str, model: Optional[str], silent_fail: bool,
    max_text_length: int, structured_output_mode: str = "typed", **kwargs
) -> List[Entity]:
    from ..split import TextSplitter
    
    # 关键: 10% 重叠分块, 避免边界实体丢失
    splitter = TextSplitter(
        method="recursive",
        chunk_size=max_text_length,
        chunk_overlap=int(max_text_length * 0.1)  # 10% overlap
    )
    chunks = splitter.split(text)
    
    all_entities = []
    max_workers = resolve_max_workers(explicit=kwargs.get("max_workers"))
    
    with ThreadPoolExecutor(max_workers=max_workers) as executor:
        future_to_chunk = {}
        for i, chunk in enumerate(chunks):
            future = executor.submit(
                extract_entities_llm,
                chunk.text, provider=provider, model=model,
                silent_fail=False,
                max_text_length=len(chunk.text) + 1,  # 关键: 避免递归分块
                structured_output_mode=structured_output_mode,
                **kwargs
            )
            future_to_chunk[future] = (i, chunk)
        
        for future in as_completed(future_to_chunk):
            i, chunk = future_to_chunk[future]
            try:
                chunk_entities = future.result()
                # 关键: 调整字符偏移, 还原到原始文本位置
                for entity in chunk_entities:
                    entity.start_char += chunk.start_index
                    entity.end_char += chunk.start_index
                    all_entities.append(entity)
            except Exception as e:
                if not silent_fail:
                    logger.error(f"Chunk {i+1} failed: {e}")
                    raise
                logger.warning(f"Chunk {i+1} failed (silent): {e}")
    
    return all_entities
```

**设计要点**：
- **10% 重叠**：防止跨块边界处的实体被切断
- **`max_text_length=len(chunk.text)+1`**：防止递归触发分块（chunk 大小 < 限制）
- **start_char/end_char 偏移调整**：分块抽取后需还原到原文位置

### 对 laew 的借鉴

**P1 — 任务分类置信度加权**：
- 当前 laew 完全信任 LLM 分类结果
- 引入 `calculate_weighted_confidence` 类似机制：综合 LLM 置信度 + 规则匹配相似度
- 例如：`final_confidence = 0.6 × llm_confidence + 0.4 × rule_match_score`

**P2 — LLM 提示词模板库**：
- Semantica 的提示词模板值得借鉴——明确 "IMPORTANT" 部分（防止 LLM 输出非 JSON 结构）
- 在 `agent/system_prompt/` 中沉淀类似模板

---

## 七、冲突检测核心代码路径

### 7.1 `ConflictType` 与 `Conflict` 数据类

**代码路径**：`semantica/conflicts/conflict_detector.py:68-93`

```python
class ConflictType(str, Enum):
    """Conflict type enumeration."""
    VALUE_CONFLICT = "value_conflict"
    TYPE_CONFLICT = "type_conflict"
    RELATIONSHIP_CONFLICT = "relationship_conflict"
    TEMPORAL_CONFLICT = "temporal_conflict"
    LOGICAL_CONFLICT = "logical_conflict"

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

### 7.2 `ResolutionStrategy` 七种解决策略

**代码路径**：`semantica/conflicts/conflict_resolver.py:108-148`（间接通过 `ResolutionStrategy`）

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

### 7.3 `detect_value_conflicts()` 值冲突检测

**代码路径**：`semantica/conflicts/conflict_detector.py:136-573`

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
    # 2. 每组内检查值冲突
    for entity_id, entity_list in entity_groups.items():
        if len(entity_list) < 2:
            continue  # 至少 2 个源才有冲突
        
        values, sources = [], []
        for entity in entity_list:
            if property_name in entity:
                values.append(entity[property_name])
                sources.append({...})
        
        # 去重后值数量 > 1 → 冲突
        unique_values = list(set(str(v) for v in values))
        if len(unique_values) <= 1:
            continue
        
        conflict = Conflict(
            conflict_id=str(uuid.uuid4()),
            conflict_type=ConflictType.VALUE_CONFLICT,
            entity_id=entity_id,
            property_name=property_name,
            conflicting_values=values,
            sources=sources,
            severity=self._calculate_severity(property_name, values),
            recommended_action=self._recommend_action(property_name, values),
        )
        conflicts.append(conflict)
    return conflicts
```

### 7.4 严重度计算 `_calculate_severity()`

**代码路径**：`semantica/conflicts/conflict_detector.py:575-592`

```python
def _calculate_severity(self, property_name: str, values: List[Any]) -> str:
    """多因素严重度评分"""
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
    
    # 3. 默认 medium
    return "medium"
```

### 7.5 `_resolve_by_voting()` 投票算法

**代码路径**：`semantica/conflicts/conflict_resolver.py:355-383`

```python
def _resolve_by_voting(self, conflict: Conflict) -> ResolutionResult:
    """多数投票: 票数最多的值胜出"""
    if not conflict.conflicting_values:
        return ResolutionResult(
            conflict_id=conflict.conflict_id, resolved=False,
            resolution_notes="No conflicting values to resolve",
        )
    
    # Counter 计票
    value_counts = Counter(conflict.conflicting_values)
    most_common_value, count = value_counts.most_common(1)[0]
    
    # 置信度 = 票数 / 总票数
    total_votes = len(conflict.conflicting_values)
    confidence = count / total_votes if total_votes > 0 else 0.0
    
    sources_used = [s.get("document", "unknown") for s in conflict.sources]
    
    return ResolutionResult(
        conflict_id=conflict.conflict_id,
        resolved=True,
        resolved_value=most_common_value,
        confidence=confidence,
        sources_used=sources_used,
        resolution_notes=f"Resolved by voting: {count}/{total_votes} votes for this value",
    )
```

### 7.6 `_resolve_by_credibility()` 可信度加权算法

**代码路径**：`semantica/conflicts/conflict_resolver.py:385-435`

```python
def _resolve_by_credibility(self, conflict: Conflict) -> ResolutionResult:
    """可信度加权投票"""
    value_weights: Dict[Any, float] = {}
    
    for i, value in enumerate(conflict.conflicting_values):
        source = conflict.sources[i] if i < len(conflict.sources) else {}
        document = source.get("document", "unknown")
        source_confidence = source.get("confidence", 0.5)
        # 关键: 权重 = 源置信度 × 源历史可信度
        credibility = self.source_tracker.get_source_credibility(document)
        weight = source_confidence * credibility
        
        if value not in value_weights:
            value_weights[value] = 0.0
        value_weights[value] += weight
    
    if not value_weights:
        return ResolutionResult(...)
    
    # 最高权重值胜出
    resolved_value = max(value_weights.items(), key=lambda x: x[1])[0]
    total_weight = sum(value_weights.values())
    confidence = (
        value_weights[resolved_value] / total_weight if total_weight > 0 else 0.0
    )
    
    return ResolutionResult(
        conflict_id=conflict.conflict_id,
        resolved=True,
        resolved_value=resolved_value,
        confidence=confidence,
        sources_used=[s.get("document", "unknown") for s in conflict.sources],
        resolution_notes=(
            f"Resolved by credibility-weighted voting "
            f"(weight: {value_weights[resolved_value]:.2f})"
        ),
    )
```

### 7.7 `resolve_conflict()` 分发器

**代码路径**：`semantica/conflicts/conflict_resolver.py:181-272`

```python
def resolve_conflict(
    self, conflict: Conflict, strategy: Union[str, ResolutionStrategy, None] = None
) -> ResolutionResult:
    normalized_strategy = self._normalize_strategy(strategy)
    
    # 关键: 按 entity_id.property_name 查找属性特定规则
    if strategy is None:
        if conflict.property_name:
            rule_key = f"{conflict.entity_id}.{conflict.property_name}"
            if rule_key in self.resolution_rules:
                normalized_strategy = self.resolution_rules[rule_key]
    
    strategy = normalized_strategy
    
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
    # ... 元数据填充
    self.resolution_history.append(result)
    return result
```

### 7.8 `InvestigationGuideGenerator` 调查指南生成

**代码路径**：`semantica/conflicts/investigation_guide.py:138-179`

```python
def generate_guide(
    self, conflict: Conflict, additional_context: Optional[Dict[str, Any]] = None
) -> InvestigationGuide:
    """为冲突生成结构化调查指南"""
    
    guide = InvestigationGuide(
        conflict_id=conflict.conflict_id,
        conflict_summary=self._generate_summary(conflict),
        severity=conflict.severity,
        conflicting_sources=conflict.sources,
        investigation_steps=self._generate_investigation_steps(conflict),
        recommended_actions=self._generate_recommended_actions(conflict),
        context=additional_context or {},
    )
    return guide

def _generate_summary(self, conflict: Conflict) -> str:
    """生成冲突摘要: Conflict Type | Entity | Property | Values | Severity | Sources"""
    parts = [f"Conflict Type: {conflict.conflict_type.value}"]
    if conflict.entity_id:
        parts.append(f"Entity: {conflict.entity_id}")
    if conflict.property_name:
        parts.append(f"Property: {conflict.property_name}")
    if conflict.conflicting_values:
        unique_values = list(set(str(v) for v in conflict.conflicting_values))
        parts.append(f"Conflicting Values: {', '.join(unique_values)}")
    parts.append(f"Severity: {conflict.severity}")
    if conflict.sources:
        source_docs = list(set(s.get("document", "unknown") for s in conflict.sources))
        parts.append(f"Sources: {', '.join(source_docs)}")
    return " | ".join(parts)
```

**6 步调查模板**：
1. Review conflict details and context
2. Identify all source documents
3. Compare conflicting information across sources
4. Assess source credibility and reliability
5. Determine resolution approach
6. Document resolution decision

**严重度推荐**：
- `critical` → URGENT: escalate to domain expert
- `high` → Review within 24 hours, consult SME
- 按冲突类型（VALUE/TYPE/TEMPORAL）提供具体建议

### 对 laew 的借鉴

**P1 — 多策略冲突解决**：
- 当前 laew 的 Quality-Check Agent 仅做单次检查（pass/fail）
- 引入 7 种策略（投票/可信度加权/最新值/最高置信度/人工/专家）
- 例如：3 个 SubAgent 都产出相似但不同的输出 → 用 voting 决定；置信度差异大 → 用 highest_confidence

**P2 — 调查指南生成**：
- laew 的 QC 失败时仅输出错误
- 引入 `InvestigationGuideGenerator` 生成结构化排查步骤，辅助用户理解失败原因

---

## 八、对 laew 工程的 P0/P1/P2 借鉴路线图

### P0（高优先级 — 立即产生价值）

#### 1. 工具调用溯源（P0）
**现状**：laew 完全无溯源机制
**方案**：
- 在 `src/agent/tools/` 层包装 Bash/Read/Write 工具调用
- 每次调用自动记录 `ProvenanceEntry` 到 SQLite 新表 `provenance`
- 关键字段：`entity_id`（文件路径/URL）、`activity_id`（read/write/bash）、`agent_id`、`source_document`（触发该调用的 user prompt 摘要）
- 实现 `compute_checksum()` SHA-256 哈希链，参照 Semantica 的 17 字段设计
- **预计代码量**：~300 行 Rust
- **价值**：完整审计日志，满足合规要求；调试时定位"为什么这次失败了"

#### 2. 双时序决策追踪（P0）
**现状**：Yolo 分类仅在每轮 SessionContext 中记录为 Markdown 摘要
**方案**：
- 借鉴 `BiTemporalFact` 模型，新增 SQLite 表 `decisions`：
  ```sql
  CREATE TABLE decisions (
      id TEXT PRIMARY KEY,
      category TEXT,           -- 'task_classification'|'tool_selection'|'plan_choice'
      scenario TEXT,
      reasoning TEXT,
      outcome TEXT,
      confidence REAL,
      entities TEXT,           -- JSON list
      valid_from TEXT,         -- 业务时间
      valid_until TEXT,        -- 业务时间
      recorded_at TEXT,        -- 系统记录时间
      superseded_at TEXT,      -- 被取代时间
      metadata TEXT            -- JSON
  );
  ```
- `record_decision()` 在 Yolo/Plan/QC 关键节点调用
- 借鉴 `find_precedents_by_scenario()`，在 Yolo 处理时自动注入最近 N 条同类型决策
- **预计代码量**：~500 行 Rust
- **价值**：Yolo 决策可解释、可审计；相似历史决策自动参考

#### 3. 哈希链完整性验证（P0）
**现状**：SQLite 数据可被外部篡改
**方案**：
- 在 `decisions` 表和 `provenance` 表添加 `checksum` 和 `previous_checksum` 字段
- 实现 `verify_chain()` 函数，定期扫描检测异常
- 检测到 chain_break 时通过 TUI 横幅告警
- **预计代码量**：~200 行 Rust
- **价值**：检测 SQLite 篡改，满足企业合规

### P1（中优先级 — 显著增强）

#### 4. Datalog 规则引擎替换简单分类（P1）
**现状**：所有任务分类都依赖 LLM
**方案**：
- 集成 431 行 DatalogReasoner 到 Rust 代码（可翻译为 Rust 实现或 FFI 调用）
- 规则示例：
  ```prolog
  hard_task(T) :- file_count(T, N), N > 10.
  simple_task(T) :- not has_keywords(T, _).
  ```
- 规则明确的任务走 Datalog（毫秒级、零成本），规则模糊的才走 LLM
- **预计代码量**：~1500 行 Rust（Datalog 引擎翻译）
- **价值**：降本 30-50%（简单任务零 LLM 调用）；规则可读可审计

#### 5. 7 策略冲突解决器（P1）
**现状**：Quality-Check Agent 仅单次检查
**方案**：
- 在 Quality-Check 后增加 `ConflictResolver`
- 当多个 SubAgent 输出不一致时，按预设策略解决
- 例如：`task_complexity > high` → `voting`；否则 `highest_confidence`
- **预计代码量**：~600 行 Rust
- **价值**：QC 通过率提升；降低人工干预频率

#### 6. 混合检索 RRF（P1）
**现状**：SessionContext 仅按时间取最近 N 条
**方案**：
- 实现 `HybridSearch.reciprocal_rank_fusion()`：`score = Σ 1/(k + rank)`，k=60
- 融合 3 个检索源：向量相似度 + 时间衰减 + 任务类别匹配
- **预计代码量**：~400 行 Rust
- **价值**：历史摘要检索更精准，减少无关上下文注入

#### 7. Rete 工具选择规则（P1）
**现状**：所有工具选择由 LLM 决策
**方案**：
- 实现 642 行 ReteEngine 的 Rust 版本（核心算法不变）
- 简单规则：
  - `Bash(rm, -rf, ?path) → must_have_approval`
  - `Read(?file) AND file_size(?file, >10MB) → block`
- **预计代码量**：~1800 行 Rust
- **价值**：降低工具选择的 LLM 调用频率和延迟

### P2（低优先级 — 长期演进）

#### 8. PipelineBuilder 风格的工作流 DSL（P2）
**现状**：MultiAgentOrchestrator 是硬编码 if/else
**方案**：迁移到流畅 DSL，允许用户配置：
```rust
let pipeline = PipelineBuilder::new()
    .add_step("yolo_classify", StepType::Yolo)
    .add_step("plan", StepType::Plan).depends_on(&["yolo_classify"])
    .add_step("main", StepType::MainWork).depends_on(&["plan"]).parallel_safe(true)
    .add_step("qc", StepType::QualityCheck).depends_on(&["main"])
    .build();
```
- **预计代码量**：~1000 行 Rust

#### 9. 本体管理（OWL/SHACL）（P2）
**现状**：无本体层
**方案**：为 Agent 工具/任务类型建立轻量 OWL 本体，支持语义推理
- **预计代码量**：~3000 行 Rust（含推理引擎）

#### 10. 17+ 摄取器适配器（P2）
**现状**：仅支持文件 Read 工具
**方案**：为 DB/Web/Repo/Stream 等数据源实现统一接口
- **预计代码量**：~5000+ 行 Rust

#### 11. 调查指南生成器（P2）
**现状**：QC 失败仅给错误信息
**方案**：在 `tui/error_screen` 增加 6 步排查模板，引导用户定位问题

---

## 总结

Semantica 的核心机制体现了**"图原生 + 确定性强 + 可审计"**的设计哲学：

| 机制 | 核心价值 | 可移植性 | laew 借鉴优先级 |
|------|---------|---------|----------------|
| Context Graph | 决策追踪 + 因果链 | 中（数据模型需改造） | P0 |
| Rete 引擎 | 确定性规则匹配 | 高（仅 642 行 Python） | P1 |
| Datalog 引擎 | 半朴素求值 + 多跳推理 | 高（仅 431 行 Python） | P1 |
| W3C PROV-O 溯源 | 审计级数据来源 | 高（237 行完整性 + 1521 行 manager） | P0 |
| 摄取管线 | 17+ 数据源统一接口 | 低（依赖过多 Python 库） | P2 |
| 语义抽取加权置信度 | 多方法融合 | 高（单函数） | P1 |
| 冲突检测 7 策略 | 多场景解决 | 高（574 行 resolver） | P1 |

**最值得借鉴的 3 个组件**（代码量可控、价值密度高）：

1. **`compute_checksum()` + `verify_chain()`**（237 行）—— 直接可移植到 Rust，零依赖
2. **`DatalogReasoner`**（431 行）—— 算法清晰，移植后用于任务分类
3. **`ConflictResolver` 七策略**（574 行）—— 独立组件，集成到 QC 流程

**3 个不适合直接借鉴**：
- 摄取管线（依赖 boto3/beautifulsoup4/pyarrow 等重依赖）
- Explorer 前端（React/Vite，独立产品）
- 多框架集成（LangChain/CrewAI 等的胶水代码）

Semantica 的设计为 laew 提供了**完整的参考实现路径**，特别是决策可解释性、审计完整性、规则化推理这三个维度，可逐步集成到 laew 的 MultiAgent 架构中。
