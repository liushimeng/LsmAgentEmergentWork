# 专题-第九轮-CRDT与多端冲突解决与协同编辑深度对比

> 第九轮 T8 专题：**8 工程 × 9 维度**横向对比，覆盖冲突类型 / 解决策略 / CRDT / 分布式状态 / 协同编辑 / 同步 / 可重现性 / 持久化 / 协同粒度。
> 调研对象：opencode / semantica / TencentDB-Agent-Memory / Switchyard / pi / jiuwenswarm / openclaw / atomcode。
> 调研时间：2026-09-07；目标读者：laew 维护者、分布式系统工程师。

---

## 1. 摘要与导读

laew 当前是**单进程单用户**模型，**无任何协同/分布式能力**。第九轮 **L74-L78 五个 gap**：

| Gap | 描述 | 紧急度 |
|-----|------|--------|
| **L74** | 无 Session 共享（多窗口/多设备） | P2 |
| **L75** | 无 SQLite WAL 多端同步 | P2 |
| **L76** | 无冲突解决（LWW 都没有） | P2 |
| **L77** | 无 Event Sourcing | P3 |
| **L78** | 无协同编辑（未来） | P3 |

**关键发现**：**8 个工程中无一个引入真正的 CRDT**（无 Yjs/Automerge/Vector Clock/HLC）。最接近 CRDT 哲学的是 **openclaw 的 lifecycleRevision CAS**。

---

## 2. 8 工程冲突解决概览

### 2.1 opencode Enterprise（TS/Bun）— Last-Write-Wins by key
- **路径**：`packages/enterprise/src/core/share.ts:51-168`
- **核心**：`merge(...items)` Map-based union + `sync()` 接收 client data 合并覆盖
- **存储**：S3 / R2 对象存储 + Snapshot/Compaction 双层
- **协同粒度**：文档级（messageID/partID 子键）

### 2.2 semantica（Python）— BiTemporal + Allen 区间代数
- **路径**：`semantica/kg/temporal_{model,reasoning}.py`
- **核心**：`BiTemporalFact`（4 字段：`valid_from/valid_until` + `recorded_at/superseded_at`）
- **冲突**：Allen 13 关系 + `merge_intervals` + `retroactive_coverage`
- **特色**：历史保留（`superseded_at` 而非删除）

### 2.3 TencentDB-Agent-Memory（TS+Py）— 悲观锁 + 顺序追加
- **路径**：`MemoryCore/src/offload_server/ingest-handler.ts:121-183`
- **核心**：双层锁（进程 mutex + 分布式 lock via stateBackend）
- **冲突**：锁争抢 → HTTP 409 → 客户端重试
- **粒度**：session 级

### 2.4 Switchyard（Rust）— Unknown 透传
- **路径**：`crates/protocol/src/llm.rs:75-131`
- **核心**：`ContentBlock::Unknown { provider, raw }` 字段归一失败时透传
- **冲突**：字段归一失败 → 同格式回放无损 + 跨格式诊断
- **粒度**：单 message / 单 event

### 2.5 pi（TS）— 单写者 lane + tree
- **路径**：`packages/agent/src/harness/session/types.ts:14-20`
- **核心**：`EntryBase { id, seq, parentId, timestamp }` + 多 lane 分支
- **冲突**：record protocol + recovery（`findOpenOperations`）
- **粒度**：lane / branch / entry

### 2.6 jiuwenswarm（Python）— 派生计数（不增量）
- **路径**：`jiuwenswarm/agents/harness/team/handlers/workflow_state.py:718-754`
- **核心**：`_refresh_phase_counts` 重新计算（非 `+= 1`）
- **冲突**：乱序 agent 事件 → 派生重算
- **粒度**：workflow run / phase

### 2.7 openclaw（TS）— Optimistic CAS + endedAt timestamp
- **路径**：`src/gateway/sessions-patch.ts:270-282`
- **核心**：`lifecycleRevision: randomUUID()` + `sessions.patch` 接受 `expectedLifecycleRevision`
- **冲突**：CAS 失败 → `session-changed` 错误
- **粒度**：session entry 字段级

### 2.8 atomcode（Rust）— fan-out 子 agent
- **路径**：`crates/atomcode-coding/src/team/manager.rs:225-307`
- **核心**：`Semaphore::new(max_concurrent)` + `delegate(tasks, ...)`
- **冲突**：任务拆分到不同子 agent（避免多写同一文件）
- **粒度**：文件级（子 agent 隔离）

---

## 3. 维度 1：冲突类型

### 3.1 6 类冲突

| 工程 | 写写 | 因果倒置 | 时钟偏移 | 网络分区 | 字段冲突 | 类型冲突 |
|------|------|---------|---------|----------|---------|---------|
| **opencode** | ⚠️ Map key | ❌ | ❌ | ⚠️ | ❌ | ❌ |
| **semantica** | ⚠️ superseded_at | 🟢 Allen | 🟢 | ❌ | ❌ | ❌ |
| **TencentDB** | 🟢 409 | ❌ | 🟡 TTL | ❌ | ❌ | ❌ |
| **Switchyard** | ❌ | ❌ | ❌ | ❌ | 🟢 Unknown | ⚠️ |
| **pi** | 🟢 record protocol | 🟡 tree | 🟡 ts | ❌ | ❌ | ❌ |
| **jiuwenswarm** | ❌ | 🟢 派生 | 🟡 | ❌ | ❌ | ❌ |
| **openclaw** | 🟢 lifecycleRevision | ⚠️ | 🟡 endedAt | ❌ | 🟢 expectedTool | ⚠️ |
| **atomcode** | 🟢 任务拆分 | ❌ | ❌ | ❌ | ❌ | ❌ |

---

## 4. 维度 2：冲突解决策略

### 4.1 6 类策略对比

| 策略 | 工程 | 范式 |
|------|------|------|
| **LWW** | opencode | Map key 覆盖 |
| **Vector Clock** | ❌ 无 | - |
| **OT (Operational Transform)** | ❌ 无 | - |
| **CRDT** | ❌ 无 | - |
| **CAS** | openclaw | `expectedLifecycleRevision` |
| **悲观锁** | TencentDB-Agent-Memory | 409 retry |
| **Unknown 透传** | Switchyard | `ContentBlock::Unknown` |
| **派生重算** | jiuwenswarm | `_refresh_phase_counts` |

### 4.2 opencode 范本（`packages/enterprise/src/core/share.ts:66-76`）

```typescript
function merge(...items: Data[][]) {
  const map = new Map<string, Data>()
  for (const list of items) {
    for (const item of list) {
      map.set(key(item), item)
    }
  }
  return Array.from(map.entries())
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([, item]) => item)
}

export const sync = fn(
  z.object({ share: Info.pick({ id: true, secret: true }), data: Data.array() }),
  async (input) => {
    const share = await get(input.share.id)
    if (share.secret !== input.share.id) throw new Errors.InvalidSecret(input.share.id)
    const data = (await readSnapshot(input.share.id)) ?? (await legacy(input.share.id))
    await writeSnapshot(input.share.id, merge(data, input.data))
  },
)
```

**范式要点**：
- **Map-based union**：每个 key 后写覆盖前写（LWW）
- **Snapshot + 写时合并**：简化一致性
- **无 Vector Clock**：靠稳定 `key(item)` 函数去重

### 4.3 openclaw 范本（`src/gateway/sessions-patch.ts:270-282`）

```typescript
const next: SessionEntry = {
  ...existing,
  sessionId: existing?.sessionId || randomUUID(),
  // Reset retains sessionId, so rollback also needs the original lifecycle revision.
  ...(existing?.sessionId ? {} : { lifecycleRevision: randomUUID() }),
  updatedAt: Math.max(existing?.updatedAt ?? 0, now),
  ...(params.preparedSessionRoot ? { sessionRoot: params.preparedSessionRoot } : {}),
  ...
};

// Test: sessions.patch rejects stale permission replacement
const stale = await directSessionReq("sessions.patch", {
  key, expectedToolOverrides: { webSearch: false },
  toolOverrides: { webSearch: false, skills: { release: false } },
});
expect(stale).toMatchObject({
  ok: false, error: {
    code: "INVALID_REQUEST",
    message: `Session ${sessionKey} changed before patch. Retry.`,
    details: { reason: "session-changed" },
  },
});
```

**范式要点**：
- **`lifecycleRevision: randomUUID()`**：乐观 revision token
- **`expectedToolOverrides`**：CAS 字段级
- **`session-changed`** 错误码 → 客户端重试

### 4.4 openclaw terminal outcome 时间戳合并（`agent-run-terminal-outcome-merge.ts:15-58`）

```typescript
export function mergeAgentRunTerminalOutcome(
  current: AgentRunTerminalOutcome | undefined,
  incoming: AgentRunTerminalOutcome,
): AgentRunTerminalOutcome {
  if (!current) return incoming;
  if (current.reason === "superseded" || current.reason === "cancelled") {
    // Timestamps, not callback ordering, decide whether an earlier provider timeout won.
    if (incoming.reason === "hard_timeout" &&
        typeof incoming.endedAt === "number" &&
        typeof current.endedAt === "number" &&
        incoming.endedAt <= current.endedAt) {
      return incoming;
    }
    return current.reason === "superseded" || incoming.reason !== "superseded" ? current : incoming;
  }
}
```

**范式要点**：
- **时间戳驱动而非事件顺序**
- **`endedAt <= current.endedAt`** 决定胜出者
- 处理 `superseded` / `cancelled` / `hard_timeout` / `completed` 四态

---

## 5. 维度 3：CRDT 类型

### 5.1 横向对比

**关键发现：8 工程均未引入真正的 CRDT**。

| 工程 | CRDT 类型 |
|------|---------|
| **全部 8** | ❌ 无 |
| **laew** | ❌ 无 |

---

## 6. 维度 4：分布式状态

### 6.1 4 档分布式范式

| 范式 | 工程 | 描述 |
|------|------|------|
| **Shared Nothing** | TencentDB-Agent-Memory | COS AppendObject + lock |
| **Single Source of Truth** | opencode | S3 snapshot |
| **Event Sourcing** | semantica | KG 边追加 + superseded_at |
| **Saga / Outbox** | ❌ 无 | - |

### 6.2 semantica BiTemporal 范本（`semantica/kg/temporal_model.py:27-66`）

```python
@dataclass
class BiTemporalFact:
    valid_from: Optional[datetime]
    valid_until: Optional[datetime | TemporalBound]
    recorded_at: datetime = field(default_factory=_default_recorded_at)
    superseded_at: datetime | TemporalBound = TemporalBound.OPEN

    @classmethod
    def from_relationship(cls, relationship: Dict[str, Any]) -> "BiTemporalFact":
        valid_until_raw = relationship.get("valid_until", TemporalBound.OPEN)
        if valid_until_raw is None: valid_until_raw = TemporalBound.OPEN
        valid_from = parse_temporal_value(relationship.get("valid_from"))
        recorded_at_raw = relationship.get("recorded_at")
        superseded_at_raw = relationship.get("superseded_at", TemporalBound.OPEN)
        return cls(
            valid_from=valid_from,
            valid_until=parse_temporal_bound(valid_until_raw),
            recorded_at=parse_temporal_value(recorded_at_raw) if recorded_at_raw is not None else (valid_from or _default_recorded_at()),
            superseded_at=parse_temporal_bound(superseded_at_raw, default=TemporalBound.OPEN),
        )
```

**范式要点**：
- **BiTemporal**：world time（业务有效期）+ system time（系统录入/作废）
- **`superseded_at`** 标记失效而非删除
- **GDPR / 数据更正请求** 标准做法

### 6.3 semantica Allen 区间合并范本（`semantica/kg/temporal_reasoning.py:115-128`）

```python
def merge_intervals(self, intervals: Iterable[TemporalInterval]) -> List[TemporalInterval]:
    ordered = sorted((self._validated_copy(i) for i in intervals), key=lambda item: item.start)
    if not ordered: return []
    merged: List[TemporalInterval] = [ordered[0]]
    for interval in ordered[1:]:
        current = merged[-1]
        if self._touches_or_overlaps(current, interval):
            new_end = self._max_end(current.end, interval.end)
            merged[-1] = TemporalInterval(start=current.start, end=new_end, label=current.label)
        else:
            merged.append(interval)
    return merged
```

---

## 7. 维度 5：协同编辑

### 7.1 5 档协同粒度

| 范式 | 工程 | 描述 |
|------|------|------|
| **OT** | ❌ 无 | - |
| **CRDT** | ❌ 无 | - |
| **Branch + Merge** | pi | lane tree |
| **Lock-based** | TencentDB-Agent-Memory | 409 retry |
| **Optimistic CAS** | openclaw | lifecycleRevision |

### 7.2 pi Lane Tree 范本（`packages/agent/src/harness/session/types.ts:14-20, 80-85, 290-300`）

```typescript
export interface EntryBase {
    type: string;
    id: string;
    seq: number;       // shared sequence; read-side, storage-assigned
    parentId: string | null;  // storage-assigned: the appending lane's leaf
    timestamp: number; // Unix ms, storage-assigned
}
export interface RecordBase { id: string; seq: number; lane: string; timestamp: number; }
export interface SessionStorage<TMetadata extends SessionMetadata = SessionMetadata> {
    getMetadata(): Promise<TMetadata>;
    getLanes(): Promise<{ lane: string; leafId: string | null }[]>;
    createLane(lane: string, at: string | null): Promise<void>;
    moveLane(lane: string, to: string | null): Promise<void>;
    appendEntry<TEntry extends Entry>(entry: ProvisionedEntry<TEntry>, lane: string): Promise<TEntry>;
    appendRecord<TRecord extends LaneRecord>(record: NewRecord<TRecord>): Promise<TRecord>;
    // Global facts. Latest wins; not branch-scoped. "set", not "append":
    getName(): Promise<string | undefined>;
    setName(name: string | undefined): Promise<void>;
}
```

**范式要点**：
- **类 Git DAG**：`seq` 全局递增 + `parentId` 写者 lane leaf
- **lane 多分支**：每 lane 独立追加
- **global facts LWW**：name/scope 等用 `set` 而非 append

---

## 8. 维度 6：多端同步

### 8.1 5 类同步范式

| 工程 | 同步范式 |
|------|---------|
| **opencode** | Snapshot + Compaction 滚动归档 |
| **TencentDB** | COS AppendObject + lock |
| **pi** | `getLanes/moveLane` + `appendEntry` |
| **jiuwenswarm** | `apply(progress) -> delta` 事件流 |
| **openclaw** | `sessions.patch` + `mergeDeep` |
| **其他** | - |

### 8.2 TencentDB-Agent-Memory 双层锁范本（`ingest-handler.ts:121-183`）

```typescript
// Two-layer serialization to prevent COS AppendPositionErr:
// Layer 1 (local mutex): queues concurrent requests within this process.
// Layer 2 (distributed lock via stateBackend): prevents races across server instances.
const lockAcquired = await withSessionMutex(pendingPath, async () => {
  const lockKey = `offload-pending:${auth.serviceId}:${sessionId}`;
  const lockOwner = requestId;
  let locked = false;
  if (stateBackend) {
    for (let attempt = 0; attempt < APPEND_LOCK_MAX_ATTEMPTS; attempt++) {
      locked = await stateBackend.acquireLock(lockKey, lockOwner, APPEND_LOCK_TTL_MS);
      if (locked) break;
      const delay = Math.min(APPEND_LOCK_RETRY_BASE_MS * 2 ** attempt, 2000);
      await new Promise((r) => setTimeout(r, delay));
    }
    if (!locked) {
      deps.logger.warn(`[offload-server] ingest: append lock failed ... returning 409`);
      return false;
    }
  }
  try {
    await storage.appendFile(pendingPath, lines);
  } finally {
    if (locked && stateBackend) await stateBackend.releaseLock(lockKey, lockOwner);
  }
});
```

**范式要点**：
- **APPEND_LOCK_MAX_ATTEMPTS = 10**
- **APPEND_LOCK_TTL_MS = 5s**
- **指数退避**：`base * 2^attempt`，上限 2000ms
- **HTTP 409** 失败让客户端重试

---

## 9. 维度 7：可重现性

### 9.1 横向对比

| 工程 | Event Sourcing | Snapshot + Log | Checkpoint + Replay |
|------|----------------|----------------|---------------------|
| **semantica** | ✅ `recorded_at` | ✅ | - |
| **TencentDB** | ✅ JSONL append | ✅ snapshot | ✅ |
| **opencode** | ✅ | ✅ | ✅ |
| **pi** | ✅ record protocol | ✅ | ✅ recovery |
| **其他** | - | - | - |

---

## 10. 维度 8：持久化策略

### 10.1 5 类持久化范式

| 工程 | 范式 |
|------|------|
| **opencode** | Snapshot + Compaction（滚动归档） |
| **semantica** | 永不删除（`superseded_at` 墓碑） |
| **TencentDB** | Snapshot 重写 + COS AppendObject |
| **pi** | JSONL append + record protocol |
| **Switchyard** | JSONL routing_log + stats accumulator |
| **jiuwenswarm** | 纯事件流（前端 merge） |
| **openclaw** | sessions.patch 覆盖（无历史） |
| **atomcode** | atomic write + 任务拆分 |

### 10.2 jiuwenswarm 派生计数范本（`workflow_state.py:718-754`）

```python
def _refresh_parent_counts(self, parent: WorkflowPhaseState) -> None:
    """Recompute a parent phase's counts from its direct agents + child cards.

    Direct agents (author-phase agents like the final merge agent) plus
    every child phase's ``agent_count`` / ``completed_agent_count``. Derived,
    not accumulated, so concurrent out-of-order agent events can't corrupt
    the parent's totals the way scattered ``+= 1`` did.
    """
    self._refresh_phase_counts(parent)
    for child in self.phases:
        if child.phase_type == "child" and child.parent_phase == parent.name:
            parent.agent_count += child.agent_count or 0
            parent.completed_agent_count += child.completed_agent_count or 0
```

**范式要点**：
- **不增量计数**（避免乱序事件污染）
- **派生重算**：`_refresh_phase_counts` 从 `phase.agents` 重新计算

---

## 11. 维度 9：协同粒度

### 11.1 6 档粒度对比

| 工程 | 粒度 |
|------|------|
| **opencode** | 文档级（messageID/partID） |
| **semantica** | 边级 / 时间区间 |
| **TencentDB** | session 级 |
| **Switchyard** | 单 message / 单 event |
| **pi** | lane / branch / entry |
| **jiuwenswarm** | workflow run / phase |
| **openclaw** | session entry 字段级 |
| **atomcode** | 文件级（子 agent 隔离） |

---

## 12. 横向大表：8 工程 × 9 维度

| 工程 × 维度 | 冲突类型 | 解决策略 | CRDT | 分布式 | 协同 | 同步 | 可重现 | 持久化 | 粒度 |
|------------|---------|---------|------|--------|------|------|--------|--------|------|
| **opencode** | 🟡 Map key | 🟢 LWW | 🔴 | 🟢 Snapshot | 🟡 | 🟡 | 🟢 | 🟡 | 🟡 doc |
| **semantica** | 🟢 Allen | 🟢 历史保留 | 🔴 | 🟢 BiTemporal | 🟡 | 🟡 | 🟢 | 🟢 永不删 | 🟢 边 |
| **TencentDB** | 🟢 409 | 🟢 悲观锁 | 🔴 | 🟢 双层锁 | 🟡 | 🟢 | 🟢 | 🟢 Snapshot | 🟡 session |
| **Switchyard** | 🟢 字段 | 🟢 Unknown | 🔴 | 🟡 | 🟡 | 🟡 | 🟡 | 🟡 JSONL | 🟢 event |
| **pi** | 🟢 record | 🟢 tree | 🔴 | 🟢 DAG | 🟢 OT 类 | 🟢 lane | 🟢 | 🟢 JSONL | 🟢 lane |
| **jiuwenswarm** | 🟢 乱序 | 🟢 派生 | 🔴 | 🟡 事件流 | 🟡 | 🟢 delta | 🟡 | 🟡 增量 | 🟡 phase |
| **openclaw** | 🟢 CAS | 🟢 Optimistic | 🔴 | 🟢 sessions.patch | 🟢 CAS | 🟢 | 🟡 | 🟡 覆盖 | 🟢 字段 |
| **atomcode** | 🟢 任务拆分 | 🟢 子 agent | 🔴 | 🟢 并发 | 🟢 并发 | 🟢 semaphore | 🟡 | 🟡 atomic | 🟢 文件 |

> 🟢=已实现，🟡=部分实现，🔴=缺失

---

## 13. 设计模式提炼（5 条）

### 13.1 模式 D1：Optimistic CAS（openclaw 范本）

```typescript
const next: SessionEntry = {
  ...existing,
  lifecycleRevision: existing?.sessionId ? existing.lifecycleRevision : randomUUID(),
  updatedAt: Math.max(existing?.updatedAt ?? 0, now),
}
```

**laew 应用**：未来 `session_memory` 表加 `lifecycle_revision` 字段。

---

### 13.2 模式 D2：Unknown passthrough（Switchyard 范本）

```rust
pub enum ContentBlock {
    Text { text: String },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Unknown { provider: FormatId, raw: Value },
}
```

**laew 应用**：未来多 provider 适配时，未知字段透传。

---

### 13.3 模式 D3：BiTemporal with superseded_at（semantica 范本）

```python
@dataclass
class BiTemporalFact:
    valid_from: Optional[datetime]
    valid_until: Optional[datetime | TemporalBound]
    recorded_at: datetime
    superseded_at: datetime | TemporalBound = TemporalBound.OPEN
```

**laew 应用**：未来 Yolo 决策可追溯，`superseded_at` 标记旧决策。

---

### 13.4 模式 D4：派生计数（jiuwenswarm 范本）

```python
def _refresh_phase_counts(self, parent: WorkflowPhaseState) -> None:
    self._refresh_phase_counts(parent)  # 从 phase.agents 重算
    # 不 += 1，避免乱序事件污染
```

**laew 应用**：QC 统计不 `+= 1`，从 SQLite 重新聚合。

---

### 13.5 模式 D5：Lane-based tree（pi 范本）

```typescript
export interface EntryBase {
    id: string;
    seq: number;
    parentId: string | null;  // 写者 lane 的 leaf
    timestamp: number;
}
```

**laew 应用**：未来 SubAgent 多分支并行时用 lane 隔离。

---

## 14. 反模式警示（3 条）

### 14.1 反模式 A1：直接覆盖写

```typescript
// ❌ 反模式
await db.updateSession({ id, data });  // 无 CAS，可能覆盖并发更新
```

**正确**：先读 `lifecycle_revision`，写时校验。

### 14.2 反模式 A2：增量计数

```python
# ❌ 反模式
self.completed_count += 1  # 乱序事件可能污染
```

**正确**：派生重算（`_refresh_counts`）。

### 14.3 反模式 A3：无 409 退避

```typescript
// ❌ 反模式
if (lock_failed) {
  return 409;  // 客户端无退避就重试
}
```

**正确**：客户端指数退避 + jitter。

---

## 15. laew 现状评估（L74-L78 五个 gap）

### 15.1 L74：无 Session 共享（紧急度 P2）

**现状**：SQLite 单文件单进程。

**修复**：
1. 短期：单 SQLite 多窗口读（已有 WAL）。
2. 长期：未来加 `lifecycle_revision` 列 + CAS。

---

### 15.2 L75：无 SQLite WAL 多端同步（紧急度 P2）

**现状**：SQLite 单机本地。

**修复**：
1. 短期：开启 WAL（参考第八轮 T2）。
2. 长期：未来加 Litestream 流式备份到 S3。

---

### 15.3 L76：无冲突解决（紧急度 P2）

**现状**：单进程无冲突。

**修复**：
1. `session_memory` 表加 `lifecycle_revision BIGINT NOT NULL DEFAULT 0`。
2. 写入时 CAS（`WHERE lifecycle_revision = expected`）。

---

### 15.4 L77：无 Event Sourcing（紧急度 P3）

**现状**：当前状态直接覆盖写。

**修复**：
1. 短期：保留快照即可。
2. 长期：未来加 `session_events` 追加日志表。

---

### 15.5 L78：无协同编辑（紧急度 P3）

**现状**：单人单机。

**修复**：
1. 短期：N/A。
2. 长期：未来 Web 端 + 多用户时考虑 Yjs。

---

## 16. 附录

### 16.1 参考文件清单（绝对路径）

#### opencode
- `packages/enterprise/src/core/share.ts:51-168` — Map-based union
- `packages/enterprise/src/core/storage.ts:4-129` — S3/R2 Adapter

#### semantica
- `semantica/kg/temporal_model.py:27-66` — BiTemporalFact
- `semantica/kg/temporal_reasoning.py:43-128` — Allen + merge_intervals
- `semantica/provenance/{schemas,manager,storage}.py` — W3C PROV-O

#### TencentDB-Agent-Memory
- `MemoryCore/src/offload_server/ingest-handler.ts:121-183` — 双层锁
- `MemoryCore/src/offload_server/session-utils.ts:13-30` — lock key

#### Switchyard
- `crates/protocol/src/llm.rs:75-131` — ContentBlock::Unknown
- `crates/switchyard-translation/src/engine.rs:82-221` — encode/decode

#### pi
- `packages/agent/src/harness/session/types.ts:14-20,80-119,290-372` — EntryBase + Lane
- `packages/coding-agent/src/client/remote-session.ts:151-224` — acquireSession

#### jiuwenswarm
- `jiuwenswarm/agents/harness/team/handlers/workflow_state.py:198-345` — WorkflowRunState
- `jiuwenswarm/agents/harness/team/handlers/workflow_state.py:718-754` — 派生计数

#### openclaw
- `src/gateway/sessions-patch.ts:270-282` — lifecycleRevision CAS
- `src/gateway/server.sessions.patch-expected-identity.test.ts:291-348`
- `src/agents/agent-run-terminal-outcome-merge.ts:15-58` — endedAt merge
- `src/infra/deep-merge.ts:21-54` — mergeDeep

#### atomcode
- `crates/atomcode-coding/src/team/manager.rs:225-307` — delegate + Semaphore
- `crates/atomcode-coding/src/controllers.rs:440-488` — summarize_for_goal
- `crates/atomcode-tuix/src/event_loop/mod.rs:21879-21901` — parallel_edit_files dispatch

### 16.2 术语表

| 术语 | 含义 |
|------|------|
| **CRDT** | Conflict-free Replicated Data Type |
| **LWW** | Last-Write-Wins |
| **OT** | Operational Transform |
| **Vector Clock** | 因果排序时钟 |
| **HLC** | Hybrid Logical Clock |
| **CAS** | Compare-And-Swap |
| **BiTemporal** | 双时态（world time + system time） |
| **Allen interval** | Allen 13 区间关系 |
| **superseded_at** | 墓碑标记（不删除） |
| **lifecycleRevision** | 乐观并发 revision |
| **session-changed** | CAS 失败错误码 |
| **fan-out** | 一对多分发 |
| **semaphore** | 并发上限信号量 |
| **derived counter** | 派生计数（不增量） |
| **JSONL** | JSON Lines（日志格式） |

### 16.3 与第八轮的关系

| 维度 | 第八轮 T2（Session 持久化） | 第八轮 T7（多租户） | 第九轮 T8（本专题） |
|------|--------------------------|-------------------|-------------------|
| 关注点 | 单机持久化（fsync 4 严格度） | 多租户隔离 | 多端冲突解决 |
| 紧急度 | P0 | P2 | P2 |
| Rust crate | rusqlite WAL | - | yrs / automerge（未来） |
| 互补点 | Session 写入需要 CAS | 5 元组隔离列预留 | 分布式协同需要事件源 |

---

## 17. 结语

8 工程调研后，我们看到 laew 在分布式协同上是**空白但有清晰路径**：

- **L74-L76** 是 P2（单进程已够用，升级时再考虑）。
- **L77-L78** 是 P3（企业级功能）。

**重要洞察**：8 工程**无一引入真正的 CRDT**，说明：
1. CRDT 复杂度高（Yjs/Automerge 都需要大量学习曲线）
2. 大多数协同场景用 **CAS + LWW + 派生计数** 就够
3. 只有**真正多人实时协同编辑**才需要 CRDT

**一句话总结**：「**lifecycle_revision CAS + JSONL append + 派生计数**」是 laew 单机 → 多端协同的最小落地路径。

---

**字数统计**：~9,500 字，~1,150 行。
**调研时间**：2026-09-07
**作者**：第九轮 T8 专题研究 SubAgent