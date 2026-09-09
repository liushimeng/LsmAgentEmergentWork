# 专题-第十七轮-atomcode-深度分析

> **调研日期**：2026-09-09
> **调研范围**：atomcode 崩溃恢复、多租户隔离、RRF检索、LLM网关路由、Pregel图执行、Skill生命周期、Agent预热池、Turn锁、HTTP客户端高级实现、安全加固
> **本轮新增 gap**：L1166-L1252

---

## 一、崩溃恢复与取证（Panic Hook / CrashDump / Watchdog）

### 1.1 工程现状

atomcode 在崩溃恢复方面有以下机制：

**空闲超时看门狗（Idle Timeout Watchdog）**：
- 位于 `crates/atomcode-daemon/src/lib.rs`
- 每 60s tick 检查：`active_connections > 0` 或 `active_chats.has_active_operations()` → 跳过
- `elapsed >= timeout_ms` → `shutdown_tx.send(true)` 触发优雅关闭
- 默认 30 分钟（`DEFAULT_IDLE_TIMEOUT_SECS = 30 * 60`），可通过 `ATOMCODE_DAEMON_IDLE_TIMEOUT` 环境变量覆盖
- 最小值 60s（`raw_timeout.max(60)`）

**进程级诊断追踪（File-Sink Trace）**：
- 位于 `crates/atomcode-daemon/src/trace.rs`
- 通过 `ATOMCODE_TUIX_LOG=/path/to/file` 环境变量启用
- 格式：`+{us} [{CAT}] {tid} {message}`
- 分类：AGT（agent loop）、RNR（turn runner）、TOOL（tool implementations）
- 使用 `O_APPEND` 保证多进程写入原子性

**Telemetry 健康快照**：
- 位于 `crates/atomcode-telemetry/src/sender/mod.rs`
- `persist_health()` 将 `CountersSnapshot` 写入 `telemetry/health.json`
- 包含：`events_tracked`、`events_dropped_mpsc`、`events_dropped_disk`、`segments_posted`、`bytes_sent`、`last_post_unix_ms`

**Uncertain Commit 检测**：
- 位于 `crates/atomcode-capabilities/src/session/snapshot.rs`
- `SnapshotPersistenceStatus` 结构体跟踪 uncertain commit / cost warning / auxiliary warning
- 当快照持久化失败且回滚也失败时，标记为 uncertain commit

### 1.2 关键代码片段

```rust
// daemon 空闲看门狗核心逻辑
fn spawn_idle_timeout_task(
    idle_timeout_secs: u64,
    last_activity: Arc<AtomicI64>,
    active_connections: Arc<AtomicUsize>,
    active_chats: ActiveChatRegistry,
    shutdown_tx: watch::Sender<bool>,
) {
    // 每 60s tick
    // active_connections > 0 → 跳过
    // active_chats.has_active_operations() → 跳过
    // elapsed >= timeout_ms → shutdown_tx.send(true)
}

// Telemetry 健康快照
fn persist_health(&self) {
    let snap = self.counters.snapshot();
    if let Ok(json) = serde_json::to_string(&snap) {
        if let Some(parent) = self.health_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.health_path, json);
    }
}
```

### 1.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                    Daemon Process                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ Idle Watchdog│  │ ActiveChat   │  │ Connection       │  │
│  │ (60s tick)   │  │ Registry     │  │ Counter          │  │
│  └──────┬───────┘  └──────┬───────┘  └────────┬─────────┘  │
│         │                 │                    │            │
│         └─────────────────┴────────────────────┘            │
│                           │                                 │
│                    shutdown_tx.send(true)                    │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │              Telemetry Health Snapshot               │   │
│  │  events_tracked / events_dropped / segments_posted   │   │
│  │  → telemetry/health.json                            │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 1.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 panic hook，进程 panic 时无法生成 CrashDump | P0 |
| L1167 | 崩溃恢复 | 无 watchdog/restart loop，daemon 崩溃后无法自动重启 | P1 |
| L1168 | 崩溃恢复 | 无崩溃取证报告生成机制（类似 DebugReport） | P1 |
| L1169 | 崩溃恢复 | 无进程级 backtrace 捕获与上报 | P1 |

---

## 二、多租户隔离（Multi-Tenant Isolation）

### 2.1 工程现状

atomcode 在多租户隔离方面有以下机制：

**Session 隔离（Project Bucket）**：
- 位于 `crates/atomcode-capabilities/src/session/manager.rs`
- 每个项目通过 `project_hash`（16位十六进制）隔离
- Session 文件存储在 `<sessions_root>/<project_hash>/` 目录下
- `SessionManager::for_project(working_dir)` 创建项目级管理器

**Session Lease 机制**：
- 位于 `crates/atomcode-capabilities/src/session/manager.rs`
- `acquire_lease(session_id)` 获取排他锁
- `SessionLease` 是 RAII guard，drop 时释放
- 防止同一 session 被多个 runtime 同时修改

**Daemon Token 鉴权**：
- 位于 `crates/atomcode-daemon/src/auth_token.rs`
- `WebuiTokenStore` 管理进程内有效 token 集合
- Token 通过 `Authorization: Bearer` 或 HttpOnly Cookie 传递
- Cookie 端口隔离：`atomcode_webui_<port>` 避免多实例共享

**App User ID 校验**：
- 位于 `crates/atomcode-daemon/src/auth_token.rs`
- `require_app_user_id` 中间件验证 `X-Atom-User-Id` 头
- 仅 `/app` 模式启用

### 2.2 关键代码片段

```rust
// Session Lease 排他锁
pub fn acquire_lease(&self, id: &str) -> Result<SessionLease, SessionStoreError> {
    // 检查 session 是否已被其他 runtime 占用
    // 创建 lease 文件（0600）
    // RAII guard，drop 时释放
}

// Cookie 端口隔离
pub fn webui_cookie_name(port: u16) -> String {
    format!("atomcode_webui_{}", port)
}

// App User ID 校验
pub async fn require_app_user_id(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let expected = &state.app_user_id;
    if expected.is_empty() {
        return Ok(next.run(req).await);
    }
    let actual = req.headers()
        .get(APP_USER_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if actual == *expected {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
```

### 2.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                    Daemon Process                            │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              WebuiTokenStore                         │   │
│  │  HashSet<token> — 进程内有效 token 集合              │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │           require_webui_token 中间件                  │   │
│  │  Authorization: Bearer → token_from_header           │   │
│  │  Cookie: atomcode_webui_<port> → token_from_cookie   │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │           SessionManager (per project)                │   │
│  │  sessions/<project_hash>/<session_id>.{meta,snapshot} │   │
│  │  acquire_lease() → SessionLease (RAII)               │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 2.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1170 | 多租户 | 无用户级数据隔离，所有 session 共享同一文件系统 | P1 |
| L1171 | 多租户 | 无资源配额限制（CPU/内存/存储 per user） | P1 |
| L1172 | 多租户 | 无多租户审计日志 | P2 |
| L1173 | 多租户 | Cookie 端口隔离仅适用于 localhost，无跨主机隔离 | P2 |

---

## 三、RRF检索（混合检索 / 排序融合 / 向量检索）

### 3.1 工程现状

atomcode 在检索方面有以下机制：

**Recall 工具（关键词全文检索）**：
- 位于 `crates/atomcode-capabilities/src/session/recall.rs`
- 读取 `<id>.jsonl` 转录文件（never-compacted ground truth）
- 基于 `RecallIndex` trait 的可插拔排序后端
- v1 实现 `KeywordIndex`：score = 总词频，ties break by recency
- 支持时间过滤（`after`/`before`）和 limit

**CodeIntel 图检索**：
- 位于 `crates/atomcode-capabilities/src/codeintel/graph.rs`
- `CodeGraph` 结构体：symbol nodes + call edges
- BFS 遍历：`trace_callers` / `trace_callees` / `shortest_path`
- `file_dependents` 计算文件级依赖

**Memory 检索**：
- 位于 `crates/atomcode-capabilities/src/memory/store.rs`
- 简单的 bullet list 加载（`- ` 前缀）
- `merged_for_prompt` 合并 global + project memory
- 4000 字符截断

### 3.2 关键代码片段

```rust
// Recall 关键词检索
pub struct KeywordIndex;
impl RecallIndex for KeywordIndex {
    fn search<'a>(&self, records: &'a [TurnRecord], q: &RecallQuery) -> Vec<&'a TurnRecord> {
        let mut scored: Vec<(usize, &'a TurnRecord)> = records
            .iter()
            .filter_map(|r| {
                let score = score_record(r, &q.terms);
                (score > 0).then_some((score, r))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.ts.cmp(&a.1.ts)));
        scored.into_iter().take(q.limit).map(|(_, r)| r).collect()
    }
}

// CodeGraph BFS 遍历
pub fn trace_callers(&self, id: SymbolId, max_depth: usize) -> Vec<(SymbolId, usize)> {
    self.trace(id, max_depth, true)
}
pub fn trace_callees(&self, id: SymbolId, max_depth: usize) -> Vec<(SymbolId, usize)> {
    self.trace(id, max_depth, false)
}
```

### 3.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                    Recall Tool                               │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  RecallIndex trait (可插拔)                          │   │
│  │  ┌──────────────┐  ┌────────────────────────────┐  │   │
│  │  │ KeywordIndex │  │ EmbeddingIndex (future)    │  │   │
│  │  │ score=词频   │  │ 语义向量检索               │  │   │
│  │  │ tie=recency  │  │                            │  │   │
│  │  └──────────────┘  └────────────────────────────┘  │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  CodeGraph                                           │   │
│  │  SymbolNode + Edge(Calls/Imports/Inherits/Refs)      │   │
│  │  BFS: trace_callers / trace_callees / shortest_path  │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 3.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1174 | RRF检索 | 无 RRF（Reciprocal Rank Fusion）混合检索 | P0 |
| L1175 | RRF检索 | 无向量检索后端（EmbeddingIndex 仅为 trait，无实现） | P0 |
| L1176 | RRF检索 | 无 BM25 排序，仅关键词词频 | P1 |
| L1177 | RRF检索 | 无语义相似度检索（cosine similarity / ANN） | P1 |
| L1178 | RRF检索 | 无检索结果重排序（re-ranker） | P2 |

---

## 四、LLM网关路由（模型路由 / 负载均衡 / 故障转移）

### 4.1 工程现状

atomcode 在 LLM 网关路由方面有以下机制：

**Provider Factory 模式**：
- 位于 `crates/atomcode-coding/src/provider_factory.rs`
- `CodingProviderFactory` trait 定义 `build(config, session_id) → LlmProvider`
- `DefaultCodingProviderFactory` 根据 `provider_type` 分发：
  - `"claude" | "anthropic"` → `AnthropicProvider`
  - `"ollama"` → `OllamaProvider`
  - 其他 → `OpenAiCompatProvider`

**Tier Provider 路由**：
- 位于 `crates/atomcode-coding/src/subagent_tiers.rs`
- `resolve_tier_keys(config, host_model) → Option<(fast_id, capable_id)>`
- 基于 `capable_model` 排名（higher = more capable）
- fast = 最低排名，capable = 最高排名
- 仅当 host model 参与且 ≥2 参与者时路由

**Request Signing（网关认证）**：
- 位于 `crates/atomcode-auth/src/gateway_crypto.rs`
- `RequestSigner` trait：`sign(SignInput) → SignOutput`
- 通过 `codingplan-crypto` feature gate 控制
- 官方构建启用，source 构建返回 `UnavailableSigner`

**Retry Policy**：
- 位于 `crates/atomcode-capabilities/src/provider/retry.rs`
- `RetryPolicy`：`max_attempts` / `base_delay` / `max_delay`
- 默认 3 次尝试，500ms base，8s cap
- 指数退避 + 抖动

### 4.2 关键代码片段

```rust
// Provider Factory 分发
impl CodingProviderFactory for DefaultCodingProviderFactory {
    fn build(&self, cfg: &CodingAgentConfig, session_id: Option<&str>) 
        -> Result<Arc<dyn LlmProvider>, ProviderBuildError> 
    {
        let provider: Arc<dyn LlmProvider> = match cfg.provider_type.as_str() {
            "claude" | "anthropic" => {
                let mut ac = AnthropicConfig::new(&cfg.api_key, &cfg.base_url, &cfg.model);
                ac.retry = retry_policy_for(cfg.retry_max_attempts)?;
                Arc::new(AnthropicProvider::new(ac)?)
            }
            "ollama" => { /* OllamaProvider */ }
            _ => { /* OpenAiCompatProvider */ }
        };
        Ok(provider)
    }
}

// Tier 路由
pub fn resolve_tier_keys(config: &Config, host_model: &str) -> Option<(String, String)> {
    let host_participates = models.values()
        .any(|m| m.model == host_model && m.capable_model.is_some());
    if !host_participates { return None; }
    let mut ranked: Vec<(&String, i64)> = models.iter()
        .filter_map(|(id, m)| m.capable_model.map(|c| (id, c)))
        .collect();
    if ranked.len() < 2 { return None; }
    ranked.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
    Some((ranked.first().unwrap().0.clone(), ranked.last().unwrap().0.clone()))
}
```

### 4.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                CodingProviderFactory                         │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  provider_type 分发                                  │   │
│  │  ┌────────────┐ ┌────────────┐ ┌────────────────┐  │   │
│  │  │ Anthropic  │ │ OpenAI     │ │ Ollama         │  │   │
│  │  │ Provider   │ │ Compat     │ │ Provider       │  │   │
│  │  └────────────┘ └────────────┘ └────────────────┘  │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Tier Router (subagent_tiers.rs)                     │   │
│  │  host_model → resolve_tier_keys → (fast, capable)    │   │
│  │  capable_model 排名 → 选择 fast/capable 模型         │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Request Signer (gateway_crypto.rs)                  │   │
│  │  is_atomgit_gateway → RealSigner / UnavailableSigner │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 4.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1179 | LLM网关 | 无模型负载均衡（多 endpoint 轮询） | P1 |
| L1180 | LLM网关 | 无故障转移（failover）机制，provider 失败即报错 | P0 |
| L1181 | LLM网关 | 无熔断器（circuit breaker） | P1 |
| L1182 | LLM网关 | 无请求超时动态调整 | P2 |
| L1183 | LLM网关 | 无模型成本优化路由（cost-aware routing） | P2 |

---

## 五、Pregel图执行（图计算 / 消息传递 / 迭代计算）

### 5.1 工程现状

atomcode 在图计算方面有以下机制：

**CodeGraph 图结构**：
- 位于 `crates/atomcode-capabilities/src/codeintel/graph.rs`
- `CodeGraph` 结构体：
  - `nodes: HashMap<SymbolId, SymbolNode>` — 符号节点
  - `edges_out: HashMap<SymbolId, Vec<Edge>>` — 出边（调用者→被调用者）
  - `edges_in: HashMap<SymbolId, Vec<Edge>>` — 入边（反向，存储源）
  - `file_symbols: HashMap<PathBuf, Vec<SymbolId>>` — 文件→符号映射
  - `by_name: HashMap<String, Vec<SymbolId>>` — 名称索引

**BFS 遍历**：
- `trace_callers(id, max_depth)` — 谁调用了 id
- `trace_callees(id, max_depth)` — id 调用了谁
- `shortest_path(from, to)` — 最短调用路径
- `file_dependents(file, max_depth)` — 文件级依赖

**CodeIndex 缓存**：
- 位于 `crates/atomcode-capabilities/src/codeintel/index.rs`
- `CodeIndex` 结构体：`Mutex<Option<(u64, Arc<CodeGraph>)>>`
- 基于文件 mtime 指纹的缓存失效
- `get(root)` 返回缓存图或重建

### 5.2 关键代码片段

```rust
// CodeGraph BFS 遍历
fn trace(&self, id: SymbolId, max_depth: usize, incoming: bool) -> Vec<(SymbolId, usize)> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(id);
    queue.push_back((id, 0));
    let mut result = Vec::new();
    while let Some((cur, depth)) = queue.pop_front() {
        if depth >= max_depth { continue; }
        let edges = if incoming { self.callers(cur) } else { self.callees(cur) };
        if let Some(edges) = edges {
            for e in edges {
                if visited.insert(e.to) {
                    queue.push_back((e.to, depth + 1));
                    result.push((e.to, depth + 1));
                }
            }
        }
    }
    result
}

// CodeIndex 缓存
pub fn get(&self, root: &Path) -> Arc<CodeGraph> {
    let files = collect_files(&root);
    let fp = fingerprint(&files);
    if let Some((cfp, g)) = self.cache.lock().unwrap().as_ref() {
        if *cfp == fp { return g.clone(); }
    }
    let g = Arc::new(build_from_files(&root, files));
    *self.cache.lock().unwrap() = Some((fp, g.clone()));
    g
}
```

### 5.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                CodeGraph                                     │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  SymbolNode                                          │   │
│  │  id / name / kind / visibility / file / signature    │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Edge                                                 │   │
│  │  to / kind(Calls/Imports/Inherits/Impl/Refs) / line   │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  遍历算法                                           │   │
│  │  BFS: trace_callers / trace_callees                  │   │
│  │  shortest_path: 双向 BFS                            │   │
│  │  file_dependents: 文件级依赖                        │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 5.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1184 | Pregel图执行 | 无 Pregel 模型实现（vertex-centric computation） | P1 |
| L1185 | Pregel图执行 | 无消息传递抽象（message passing between vertices） | P1 |
| L1186 | Pregel图执行 | 无迭代计算终止条件（convergence detection） | P2 |
| L1187 | Pregel图执行 | 无图分区（graph partitioning）支持大规模代码库 | P2 |
| L1188 | Pregel图执行 | CodeGraph 仅支持调用图，无数据流图/控制流图 | P2 |

---

## 六、Skill生命周期（注册 / 加载 / 执行 / 卸载）

### 6.1 工程现状

atomcode 拥有完整的 Skill 生命周期管理：

**Skill 定义**：
- 位于 `crates/atomcode-capabilities/src/skills/skill.rs`
- `Skill` 结构体：`name` / `description` / `template` / `allowed_tools` / `user_invocable` / `skill_dir` / `source_path`
- 模板引擎：`$ARGUMENTS[N]` / `$N` / `${CLAUDE_SESSION_ID}` / `${CLAUDE_SKILL_DIR}` / `` !`cmd` ``
- 单次左到右扫描，避免重复展开

**SkillRegistry**：
- 位于 `crates/atomcode-capabilities/src/skills/registry.rs`
- `BTreeMap<String, Arc<Skill>>` 有序存储
- 加载优先级：低→高（后加载的同名 skill 胜出）
- 发现策略：
  - 扁平 `*.md` 文件（仅 depth=0）
  - `*/SKILL.md` 子目录（directory-style skill）
  - 分组目录递归发现（MAX_DEPTH=8）
- 命名空间支持：`{ns}:{name}`

**标准目录链**：
```
~/.claude/skills → ~/.agents/skills → ~/.atomcode/skills
.claude/skills   → .agents/skills   → .atomcode/skills
```

**UseSkillTool / ListSkillsTool**：
- 位于 `crates/atomcode-capabilities/src/skills/use_skill.rs`
- `use_skill`：调用命名 skill，返回展开后的内容
- `list_skills`：列出所有可用 skill

**Plugin Skill 集成**：
- 位于 `crates/atomcode-capabilities/src/plugin/loader.rs`
- `reload_skill_registry`：标准 skill + plugin skill 叠加
- `installed_plugin_skill_dirs`：发现已安装 plugin 的 skill 目录

### 6.2 关键代码片段

```rust
// Skill 模板展开
pub fn expand(&self, arguments: &str, session_id: &str) -> String {
    let positional: Vec<&str> = arguments.split_whitespace().collect();
    // 单次左到右扫描
    while i < t.len() {
        let rest = &t[i..];
        if let Some((value, len)) = match_substitution(rest, &positional, arguments, session_id, skill_dir) {
            result.push_str(value);
            i += len;
        } else {
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    // 无 $ARGUMENTS 时追加
    if !self.template.contains("$ARGUMENTS") && !arguments.trim().is_empty() {
        result = format!("{}\n\nARGUMENTS: {}", result.trim_end(), arguments);
    }
    expand_shell_injections(&result)
}

// SkillRegistry 加载
pub fn load(dirs: &[PathBuf]) -> Self {
    let mut reg = Self::new();
    for dir in dirs {
        reg.load_dir(dir, None);
    }
    reg
}

// Plugin skill 集成
pub fn reload_skill_registry(registry: &mut SkillRegistry, working_dir: &Path) -> Vec<String> {
    let warnings = registry.reload(working_dir);
    for (dir, namespace) in installed_plugin_skill_dirs(working_dir) {
        registry.load_dir(&dir, Some(&namespace));
    }
    warnings
}
```

### 6.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                Skill Lifecycle                               │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Discovery                                           │   │
│  │  标准目录链: .claude → .agents → .atomcode           │   │
│  │  Plugin: installed_plugin_skill_dirs                 │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Load (SkillRegistry)                                │   │
│  │  BTreeMap<name, Arc<Skill>>                          │   │
│  │  命名空间: {ns}:{name}                               │   │
│  │  优先级: 后加载胜出                                  │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Execute (UseSkillTool)                              │   │
│  │  expand(arguments, session_id) → 模板展开            │   │
│  │  expand_for_injection → 注入 system-reminder         │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Unload                                               │   │
│  │  reload() 全量重建                                    │   │
│  │  无热卸载（需重启 session）                           │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 6.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1189 | Skill生命周期 | 无 Skill 热加载/热卸载（需重启 session） | P1 |
| L1190 | Skill生命周期 | 无 Skill 版本管理 | P2 |
| L1191 | Skill生命周期 | 无 Skill 依赖声明与解析 | P2 |
| L1192 | Skill生命周期 | 无 Skill 执行超时控制 | P1 |
| L1193 | Skill生命周期 | 无 Skill 执行结果缓存 | P2 |

---

## 七、Agent预热池（Agent池化 / 预热 / 复用）

### 7.1 工程现状

atomcode 在 Agent 池化方面有以下机制：

**SubAgent 驱动**：
- 位于 `crates/atomcode-capabilities/src/subagent/`
- `SubagentBackend` trait：`name()` / `kind()` / `capabilities()` / `run(req)`
- 支持外部 agent 驱动：Claude Code / Codex
- `ExternalSubagentProfile`：name / kind / model / permission / timeout

**ManagedChild 进程管理**：
- 位于 `crates/atomcode-capabilities/src/subagent/proc.rs`
- `ManagedChild` 包装 `tokio::process::Child`
- 进程树收割（Unix: setsid + killpg，Windows: Job Object）
- `wait_or_kill(timeout, cancel)` 超时/取消语义

**Team Agent 调度**：
- 位于 `crates/atomcode-coding/src/team/manager.rs`
- `TeamRunManager`：`max_concurrent` / `cancel_grace` / `max_result_chars`
- Semaphore 并发控制
- CancellationToken 传播

### 7.2 关键代码片段

```rust
// SubagentBackend trait
#[async_trait]
pub trait SubagentBackend: Send + Sync {
    fn name(&self) -> &str;
    fn kind(&self) -> SubagentKind;
    fn capabilities(&self) -> SubagentCapabilities;
    async fn run(&self, req: SubagentRun) -> Result<SubagentResult, SubagentError>;
}

// ManagedChild 进程树收割
impl ManagedChild {
    pub fn spawn(spec: ChildSpec) -> Result<Self, SubagentError> {
        // Unix: setsid in pre_exec → child leads its own pgroup
        // killpg(pid) reaches grandchildren
        // Windows: Job Object → kill-on-close
    }
}

// Team Agent 并发控制
pub struct TeamRuntimeConfig {
    pub max_concurrent: usize,  // 默认 3
    pub cancel_grace: Duration, // 默认 2s
    pub max_result_chars: usize, // 默认 12_000
    pub max_completed_runs: usize, // 默认 32
}
```

### 7.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                Agent Pool / SubAgent                         │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  SubagentBackend trait                               │   │
│  │  ┌──────────────┐  ┌──────────────┐                 │   │
│  │  │ ClaudeCode   │  │ Codex        │                 │   │
│  │  │ Backend      │  │ Backend      │                 │   │
│  │  └──────────────┘  └──────────────┘                 │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  ManagedChild                                        │   │
│  │  spawn → setsid/Job Object → process tree            │   │
│  │  wait_or_kill(timeout, cancel) → WaitOutcome         │   │
│  │  Drop → kill_tree (killpg/taskkill /T)               │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  TeamRunManager                                      │   │
│  │  Semaphore(max_concurrent=3)                         │   │
│  │  CancellationToken 传播                              │   │
│  │  max_completed_runs=32                               │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 7.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1194 | Agent预热池 | 无 Agent 预热池（每次请求新建 Agent） | P1 |
| L1195 | Agent预热池 | 无 Agent 复用机制（session 间无法共享 Agent） | P1 |
| L1196 | Agent预热池 | 无 Agent 池大小动态调整 | P2 |
| L1197 | Agent预热池 | 无 Agent 健康检查与自动替换 | P2 |
| L1198 | Agent预热池 | 无 Agent 预热策略（lazy/eager warmup） | P2 |

---

## 八、Turn锁（并发控制 / Turn排他性 / Lease机制）

### 8.1 工程现状

atomcode 在 Turn 锁方面有以下机制：

**Session Lease**：
- 位于 `crates/atomcode-capabilities/src/session/manager.rs`
- `acquire_lease(session_id) → SessionLease`
- 排他锁：同一 session 同时只能有一个 runtime
- RAII guard：drop 时释放

**ActiveChatRegistry 单飞准入**：
- 位于 `crates/atomcode-daemon/src/lib.rs`
- `admit()` 在单次 write lock 下：
  1. 检查 session_id / request_id 是否已存在 alias
  2. 生成 uuid operation_id
  3. 创建新的 CancellationToken
  4. 同时插入 operations 和 aliases 表
- `stop_alias(alias)` → 标记 stopped=true → `cancellation.cancel()`

**Turn-level 并发控制**：
- 位于 `crates/atomcode-kernel/src/agent.rs`
- `AgentHandle` 通过 `mpsc::UnboundedSender<AgentCommand>` 串行处理
- `SendMessage` / `SendSyntheticMessage` / `Shutdown` 等命令 FIFO 处理

### 8.2 关键代码片段

```rust
// Session Lease 排他锁
pub fn acquire_lease(&self, id: &str) -> Result<SessionLease, SessionStoreError> {
    // 检查 session 是否已被其他 runtime 占用
    // 创建 lease 文件（0600）
    // RAII guard，drop 时释放
}

// ActiveChatRegistry 单飞准入
struct ActiveChatOperation {
    session_id: Option<String>,
    aliases: Vec<String>,
    cancellation: CancellationToken,
    stopped: bool,
}

// AgentHandle 串行命令处理
pub struct AgentHandle {
    commands: UnboundedSender<AgentCommand>,
    events: UnboundedReceiver<AgentEvent>,
    task: JoinHandle<()>,
}
```

### 8.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                Turn Lock / Lease                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  SessionLease (RAII)                                 │   │
│  │  acquire_lease(id) → lease file (0600)               │   │
│  │  Drop → release lease                                │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  ActiveChatRegistry                                  │   │
│  │  admit() → 单飞准入 + CancellationToken              │   │
│  │  stop_alias() → cancellation.cancel()                │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  AgentHandle (mpsc 串行)                             │   │
│  │  SendMessage → FIFO 处理                             │   │
│  │  SendSyntheticMessage → 队列 FIFO                    │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 8.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1199 | Turn锁 | 无 Turn 级超时 lease（lease 可永久持有） | P1 |
| L1200 | Turn锁 | 无 Turn 锁优先级调度 | P2 |
| L1201 | Turn锁 | 无 Turn 锁公平性保证（FIFO/公平锁） | P2 |
| L1202 | Turn锁 | 无分布式 Turn 锁（跨进程/跨节点） | P2 |

---

## 九、HTTP客户端高级实现（连接池 / 重试 / 多路复用 / 代理链）

### 9.1 工程现状

atomcode 在 HTTP 客户端方面有以下机制：

**Retry Policy**：
- 位于 `crates/atomcode-capabilities/src/provider/retry.rs`
- `RetryPolicy`：`max_attempts` / `base_delay` / `max_delay`
- 默认 3 次尝试，500ms base，8s cap
- 指数退避 + 抖动

**连接池管理**：
- `POOL_IDLE_TIMEOUT = 15s`（避免网关 LB 关闭空闲连接）
- reqwest 默认 90s → atomcode 缩短到 15s

**错误分类**：
- `is_retryable_status(code)`：408/425/429/500/502/503/504/529
- `is_retryable_reqwest_error(&e)`：连接重置/超时/不完整消息
- `is_stale_connection_error(&e)`：网关 LB 关闭空闲连接

**代理支持**：
- `apply_blocking_proxy_policy` / `apply_async_proxy_policy`
- 支持 `no_proxy` 模式
- TLS 1.2 降级（`force_tls12`）

**流式重连**：
- `MAX_STREAM_ATTEMPTS = 3`（1 initial + 2 transparent reopens）
- 仅在无 replay-sensitive event 时重连

### 9.2 关键代码片段

```rust
// Retry Policy
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl RetryPolicy {
    pub fn default_policy() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
        }
    }
}

// 连接池空闲超时
pub(crate) const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

// 流式重连
const MAX_STREAM_ATTEMPTS: u32 = 3;
let mut stream_attempt = 1u32;
let mut reconnect_attempts = 0u32;
'reopen: loop {
    let mut dec = SseDecoder::new();
    let mut emitted_replay_sensitive = false;
    // ...
}

// 代理策略
fn apply_blocking_proxy_policy(
    builder: reqwest::blocking::ClientBuilder,
    force_tls12: bool,
) -> reqwest::blocking::ClientBuilder {
    // no_proxy 模式
    // TLS 1.2 降级
}
```

### 9.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                HTTP Client Stack                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Retry Layer                                         │   │
│  │  RetryPolicy: 3 attempts, 500ms base, 8s cap        │   │
│  │  is_retryable_status: 408/425/429/500/502/503/504   │   │
│  │  is_retryable_reqwest_error: connect/timeout/reset   │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Connection Pool                                     │   │
│  │  POOL_IDLE_TIMEOUT = 15s                             │   │
│  │  reqwest default → atomcode 15s                      │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Stream Recovery                                     │   │
│  │  MAX_STREAM_ATTEMPTS = 3                             │   │
│  │  replay-sensitive detection → no retry               │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Proxy + TLS                                         │   │
│  │  no_proxy mode / TLS 1.2 fallback                    │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 9.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1203 | HTTP客户端 | 无 HTTP/2 多路复用显式配置 | P2 |
| L1204 | HTTP客户端 | 无连接池大小动态调整 | P2 |
| L1205 | HTTP客户端 | 无代理链（proxy chain）支持 | P2 |
| L1206 | HTTP客户端 | 无请求优先级调度 | P2 |
| L1207 | HTTP客户端 | 无请求去重（idempotency key） | P2 |

---

## 十、安全加固（威胁模型 / Prompt注入防护 / 密钥管理）

### 10.1 工程现状

atomcode 在安全加固方面有以下机制：

**Approval Middleware**：
- 位于 `crates/atomcode-capabilities/src/tools/approval.rs`
- `ApprovalMiddleware`：风险工具调用审批门
- `PermissionStore` trait：session-scoped grant cache
- `PermissionDecision`：AllowOnce / AllowAlways / Deny
- 审批 key 策略：edit_file 工具级，bash 命令级

**SensitivePathGate**：
- 位于 `crates/atomcode-capabilities/src/tools/sensitive_path.rs`
- 敏感路径检测：`/.ssh` / `id_rsa` / `/.aws` / `.env` 等
- 配置跟随：`$ATOMCODE_HOME` 变化时自动更新标记
- `.env` 特殊处理：仅匹配文件名，排除 `environment`

**JSON Repair**：
- 位于 `crates/atomcode-capabilities/src/tools/repair.rs`
- 修复 LLM 输出的 malformed JSON
- 修复链：direct parse → repair_json → tool-specific extractor → generic KV
- Windows 路径预转义

**Telemetry Scrub**：
- 位于 `crates/atomcode-telemetry/src/scrub.rs`
- `redact_secrets(s)`：GitHub/GitLab/AWS/Google/Slack/OpenAI 风格 token
- `scrub_path(s, home, cwd)`：路径脱敏
- `truncate_head(s, max_chars)`：字符串截断

**Gateway Crypto**：
- 位于 `crates/atomcode-auth/src/gateway_crypto.rs`
- `RequestSigner` trait：请求签名
- `codingplan-crypto` feature gate
- 官方构建启用，source 构建返回 UnavailableSigner

### 10.2 关键代码片段

```rust
// Approval Middleware
pub struct ApprovalMiddleware {
    store: Arc<dyn PermissionStore>,
    kind: String,
}

impl ApprovalMiddleware {
    pub fn grant_key(call: &ToolCall, tool: &Arc<dyn Tool>) -> String {
        // edit_file: 工具级（忽略 args）
        // bash: 命令级（包含 args）
    }
}

// SensitivePathGate
const SENSITIVE_MARKERS: &[&str] = &[
    "/.ssh", "id_rsa", "id_ed25519", "/.aws", "/.gnupg",
    "/.kube", ".netrc", ".git-credentials", "/.atomcode/auth.toml",
    ".npmrc", ".pypirc", ".pem", ".p12", ".pfx", ".keystore",
];

// Telemetry Scrub
pub fn redact_secrets(s: &str) -> String {
    // 1. key/value credentials → keep key, redact value
    // 2. known standalone token shapes → redact whole token
    // GitHub/GitLab/AWS/Google/Slack/OpenAI/JWT
}

// Gateway Crypto
pub trait RequestSigner: Send + Sync {
    fn sign(&self, req: SignInput<'_>) -> Result<SignOutput, SignError>;
    fn algorithm_version(&self) -> u8;
}
```

### 10.3 架构图

```
┌─────────────────────────────────────────────────────────────┐
│                Security Hardening                            │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Approval Middleware                                 │   │
│  │  RiskLevel::Safe → 直接通过                          │   │
│  │  RiskLevel::Risky → 审批轮询                         │   │
│  │  PermissionStore: AllowOnce / AllowAlways / Deny     │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  SensitivePathGate                                   │   │
│  │  SENSITIVE_MARKERS + configured_credential_markers   │   │
│  │  .env 特殊处理（排除 environment）                   │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  JSON Repair                                         │   │
│  │  direct parse → repair_json → tool-specific → KV     │   │
│  │  Windows 路径预转义                                  │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Telemetry Scrub                                     │   │
│  │  redact_secrets + scrub_path + truncate_head         │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                 │
│  ┌────────────────────────┴────────────────────────────┐   │
│  │  Gateway Crypto                                      │   │
│  │  RequestSigner + codingplan-crypto feature gate      │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 10.4 新增 gap

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1208 | 安全加固 | 无 Prompt 注入防护（prompt injection detection） | P0 |
| L1209 | 安全加固 | 无密钥轮换机制 | P1 |
| L1210 | 安全加固 | 无威胁模型文档 | P1 |
| L1211 | 安全加固 | 无输入验证框架（input validation framework） | P1 |
| L1212 | 安全加固 | 无安全审计日志 | P2 |
| L1213 | 安全加固 | 无沙箱隔离（tool sandboxing） | P0 |
| L1214 | 安全加固 | 无 API Key 加密存储（明文存储在 config.toml） | P0 |

---

## 十一、其他关键发现

### 11.1 Plugin 系统架构

**Plugin Manifest**：
- 位于 `crates/atomcode-capabilities/src/plugin/manifest.rs`
- `PluginManifest`：name / version / description / skills / commands / hooks
- `PathOrList` 枚举：支持单路径或 CC 风格数组
- `PluginSource`：Inline / External / Unknown

**Plugin Loader**：
- 位于 `crates/atomcode-capabilities/src/plugin/loader.rs`
- `iter_installed_plugin_assets(working_dir)` → `Vec<InstalledPluginAssets>`
- 支持 User / Project / Local 三种 scope
- CC hooks 解析：`hooks/hooks.json` / `hooks.json`

### 11.2 OAuth 状态机

**LoginState**：
- 位于 `crates/atomcode-daemon/src/login_state.rs`
- 状态转换：
  ```
  [Pending] --begin_poll--> [Polling {generation}]
  [Polling] --Pending--> [Pending]
  [Polling] --AuthorizationReady--> [Persisting]
  [Persisting] --begin_poll--> [Committing {generation}]
  [Committing] --Authorized--> [Authorized] (terminal)
  [Pending/Polling/Persisting] --cancel--> [Cancelled] (terminal)
  [any] --Failed--> [Failed] (terminal)
  [terminal] --60s--> removable
  ```
- 关键常量：LOGIN_TTL=600s，TERMINAL_RETENTION=60s，LOGIN_RETRY_AFTER_MS=2000

### 11.3 Rewind 机制

**Workspace Checkpoint**：
- 位于 `crates/atomcode-capabilities/src/session/rewind.rs`
- 独立 Git 目录存储 workspace 快照
- `--git-dir` / `--work-tree` 隔离用户分支
- `before_tree` / `after_tree` 可选（v5.0.5+）
- 事务支持：prepare → commit / compensate / recover

### 11.4 Memory 系统

**MemoryStore**：
- 位于 `crates/atomcode-capabilities/src/memory/store.rs`
- 全局 + 项目两级 memory
- 64KB 尾部读取上限
- 4000 字符截断
- 去重：ASCII 大小写不敏感

### 11.5 CodeIntel 索引

**CodeIndex**：
- 位于 `crates/atomcode-capabilities/src/codeintel/index.rs`
- 基于 tree-sitter 的符号提取
- 语言检测：`Lang::detect(path)`
- 调用边解析：`extract_calls(source, lang, syms)`
- mtime 指纹缓存失效

---

## 十二、新增 gap 汇总

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1166 | 崩溃恢复 | 无 panic hook，进程 panic 时无法生成 CrashDump | P0 |
| L1167 | 崩溃恢复 | 无 watchdog/restart loop，daemon 崩溃后无法自动重启 | P1 |
| L1168 | 崩溃恢复 | 无崩溃取证报告生成机制 | P1 |
| L1169 | 崩溃恢复 | 无进程级 backtrace 捕获与上报 | P1 |
| L1170 | 多租户 | 无用户级数据隔离，所有 session 共享同一文件系统 | P1 |
| L1171 | 多租户 | 无资源配额限制（CPU/内存/存储 per user） | P1 |
| L1172 | 多租户 | 无多租户审计日志 | P2 |
| L1173 | 多租户 | Cookie 端口隔离仅适用于 localhost | P2 |
| L1174 | RRF检索 | 无 RRF 混合检索 | P0 |
| L1175 | RRF检索 | 无向量检索后端 | P0 |
| L1176 | RRF检索 | 无 BM25 排序 | P1 |
| L1177 | RRF检索 | 无语义相似度检索 | P1 |
| L1178 | RRF检索 | 无检索结果重排序 | P2 |
| L1179 | LLM网关 | 无模型负载均衡 | P1 |
| L1180 | LLM网关 | 无故障转移机制 | P0 |
| L1181 | LLM网关 | 无熔断器 | P1 |
| L1182 | LLM网关 | 无请求超时动态调整 | P2 |
| L1183 | LLM网关 | 无模型成本优化路由 | P2 |
| L1184 | Pregel图执行 | 无 Pregel 模型实现 | P1 |
| L1185 | Pregel图执行 | 无消息传递抽象 | P1 |
| L1186 | Pregel图执行 | 无迭代计算终止条件 | P2 |
| L1187 | Pregel图执行 | 无图分区支持 | P2 |
| L1188 | Pregel图执行 | CodeGraph 仅支持调用图 | P2 |
| L1189 | Skill生命周期 | 无 Skill 热加载/热卸载 | P1 |
| L1190 | Skill生命周期 | 无 Skill 版本管理 | P2 |
| L1191 | Skill生命周期 | 无 Skill 依赖声明与解析 | P2 |
| L1192 | Skill生命周期 | 无 Skill 执行超时控制 | P1 |
| L1193 | Skill生命周期 | 无 Skill 执行结果缓存 | P2 |
| L1194 | Agent预热池 | 无 Agent 预热池 | P1 |
| L1195 | Agent预热池 | 无 Agent 复用机制 | P1 |
| L1196 | Agent预热池 | 无 Agent 池大小动态调整 | P2 |
| L1197 | Agent预热池 | 无 Agent 健康检查与自动替换 | P2 |
| L1198 | Agent预热池 | 无 Agent 预热策略 | P2 |
| L1199 | Turn锁 | 无 Turn 级超时 lease | P1 |
| L1200 | Turn锁 | 无 Turn 锁优先级调度 | P2 |
| L1201 | Turn锁 | 无 Turn 锁公平性保证 | P2 |
| L1202 | Turn锁 | 无分布式 Turn 锁 | P2 |
| L1203 | HTTP客户端 | 无 HTTP/2 多路复用显式配置 | P2 |
| L1204 | HTTP客户端 | 无连接池大小动态调整 | P2 |
| L1205 | HTTP客户端 | 无代理链支持 | P2 |
| L1206 | HTTP客户端 | 无请求优先级调度 | P2 |
| L1207 | HTTP客户端 | 无请求去重 | P2 |
| L1208 | 安全加固 | 无 Prompt 注入防护 | P0 |
| L1209 | 安全加固 | 无密钥轮换机制 | P1 |
| L1210 | 安全加固 | 无威胁模型文档 | P1 |
| L1211 | 安全加固 | 无输入验证框架 | P1 |
| L1212 | 安全加固 | 无安全审计日志 | P2 |
| L1213 | 安全加固 | 无沙箱隔离 | P0 |
| L1214 | 安全加固 | 无 API Key 加密存储 | P0 |

---

## 十三、与 laew 的对比

| 维度 | atomcode | laew | 差距 |
|------|----------|------|------|
| 崩溃恢复 | 空闲看门狗 + Telemetry 健康快照 | 无 | laew 需添加 panic hook |
| 多租户 | Project Bucket + Session Lease | 无 | laew 需添加 session 隔离 |
| RRF检索 | 关键词全文检索 + CodeGraph BFS | 无 | laew 需添加 recall 工具 |
| LLM网关 | Provider Factory + Tier Router | 双协议 | laew 需添加 failover |
| Pregel图执行 | CodeGraph + BFS 遍历 | 无 | laew 需添加代码图索引 |
| Skill生命周期 | 完整生命周期（发现→加载→执行） | 无 | laew 需添加 skill 系统 |
| Agent预热池 | SubAgent 驱动 + Team 调度 | 无 | laew 需添加 Agent 池 |
| Turn锁 | Session Lease + ActiveChatRegistry | 无 | laew 需添加 Turn 排他 |
| HTTP客户端 | Retry + 连接池 + 流式重连 | reqwest | laew 需添加 retry policy |
| 安全加固 | Approval + SensitivePath + Scrub | 无 | laew 需添加审批门 |

---

**报告完成日期**：2026-09-09
**分析基础**：atomcode crates/ 全量源码逐行分析（atomcode-kernel / atomcode-coding / atomcode-capabilities / atomcode-daemon / atomcode-auth / atomcode-telemetry / atomcode-codingplan / atomcode-codingplan-crypto / atomcode-config / atomcode-tuix / atomcode-clix / atomcode-review / atomcode-updater）
