# Pi 第三轮深挖 —— Server / Session 后端 / Client / Protocol / Evals 模块

> 调研对象: `/usr/local/LsmGitOpenSource/pi`
> 调研日期: 2026-09-05
> 调研深度: 6 个专题, 每个 3+ 处代码定位 + 行号 + 代码片段
> 前置文档: `pi-源码调研.md`(366 行) / `pi-深度分析.md`(518 行) / `pi-核心机制深度分析.md`(1507 行) / `pi-第二轮深度分析.md`(1184 行)
> 本文档基于真实源码 Read, 所有引用均可验证。全文覆盖 30+ 源文件。

---

## 一、全包清单与覆盖状态

| 包名 | 行数级 | 已有文档覆盖 | 本文覆盖 |
|------|--------|-------------|---------|
| `packages/agent/` | 大 | **核心深度** — Harness/Lane/Reducer/Tool/Skill/Compaction | 概述 |
| `packages/ai/` | 大 | **核心深度** — 双后端 API + 11 thinkingFormat + cache | 概述(45 providers) |
| `packages/coding-agent/` | 大 | **第二轮** — SessionManager JSONL + AgentLoop + Extension | 概述 |
| `packages/tui/` | 中 | **核心深度** — Layout/AltScreen/Keybinding | -- |
| `packages/server/` | 中 | 未覆盖 | **重点深挖** |
| `packages/session-backends/` | 中 | 第二轮概述(JSONL + SQLite 双后端) | **重点深挖** |
| `packages/client/` | 中 | 未覆盖 | **重点深挖** |
| `packages/protocol/` | 中 | 未覆盖 | **重点深挖** |
| `packages/evals/` | 中 | 第二轮概述(vitest-evals 框架) | **重点深挖** |
| `packages/telemetry/` | 小 | 未覆盖 | **重点深挖** |

**被遗漏的包**: `scripts/` — 35 个构建/发布/迁移脚本, 本文第五节补全。

---

## 二、Server —— 二进制帧协议 + 生命周期编排

### 2.1 架构总览: PiServer + LiveSessionManager + ServerSnapshotPublisher

Pi 的 server 层是一个**纯协议层**, 不包含任何业务逻辑。它通过 `PiServerService` 接口把 session 创建/打开/模型列表等全部委托给应用层, 自身只负责:
1. 接受字节连接(ByteConnection) → 握手(Hello 交换) → 双向消息分发
2. 管理 session 的多连接 attach/detach 生命周期
3. 向所有就绪连接广播 ServerSnapshot

```typescript
// packages/server/src/server.ts:39-79
export class PiServer {
    readonly id: string;
    private readonly listeners: readonly PiServerListener[];
    private readonly sessions: LiveSessionManager;
    private readonly snapshots: ServerSnapshotPublisher;

    constructor(service: PiServerService, options: PiServerOptions) {
        this.sessions = new LiveSessionManager({
            service,
            isClosing: () => this.closing,
            sendMessage: (connection, message) => this.sendMessage(connection, message),
            closeConnection: (connection) => this.closeConnection(connection),
            disconnect: (connection) => this.disconnect(connection),
            broadcastServerSnapshot: () => void this.snapshots.broadcast(),
            reportError: (error) => this.reportError(error),
        });
    }
}
```

**关键设计**: `LiveSessionManager` 不直接持有数据库或 LLM, 它通过 `PiServerService` 回调与应用层解耦。这意味着 server 包是完全可复用的——任何实现了 `PiServerService` 的后端都能挂载。

### 2.2 连接状态机:五阶段生命周期

每个字节连接经历五个阶段:

```typescript
// packages/server/src/connection.ts:20-36
export type ConnectionStage = "awaitingHello" | "handshaking" | "ready" | "closing" | "closed";

export interface ConnectionState {
    id: string;
    connection: ByteConnection;
    decoder: ClientMessageDecoder;
    sessionIds: Set<string>;     // 该连接 attach 的所有 session
    stage: ConnectionStage;
    disconnected: boolean;
    handshakeComplete: boolean;
    handshake?: Promise<void>;
    handshakeTimeout: NodeJS.Timeout;
}
```

握手流程(`server.ts:221-249`):
1. 客户端发 `ClientHello`(含 `version: PROTOCOL_VERSION`)
2. 服务端校验版本, 构造 `ServerSnapshot`(sessions + models), 返回 `ServerHello`
3. 若握手期间 snapshot revision 已变, 立即补发一次 `server_snapshot` 事件
4. 握手超时(默认 5s) → 发 `hello_error` → 断连

### 2.3 Session 命令编排:8 种 Command

```typescript
// packages/server/src/sessions.ts:47-119
async executeCommand(connection: ConnectionState, command: Command) {
    switch (command.command) {
        case "list":    return { command: "list", sessions: await this.listMetadata() };
        case "create":  // 分配 UUID → acquire(runtime) → attach → broadcast
        case "attach":  // acquire(runtime) → attach → broadcast
        case "detach":  // 从连接中移除 → maybeDispose
        case "prompt":  // requireAttached → runtime.prompt → broadcastSnapshot
        case "steer":   // requireAttached → runtime.steer → broadcastSnapshot
        case "abort":   // requireAttached → runtime.abort → broadcastSnapshot
        case "set_model":    // runtime.setModel
        case "set_thinking": // runtime.setThinking
    }
}
```

**关键设计**: `requireAttached`(`sessions.ts:309-318`) 确保只有 attach 到 session 的连接才能发送 prompt/steer/abort, 防止越权操作。

### 2.4 PiServerService 接口:与应用层的契约

```typescript
// packages/server/src/types.ts:54-60
export interface PiServerService {
    listSessions(): Promise<SessionMetadata[]>;
    listModels(): Promise<ModelMetadata[]>;
    createSession(options: CreateSessionOptions): Promise<PiSessionRuntime>;
    openSession(sessionId: string): Promise<PiSessionRuntime>;
}
```

`PiSessionRuntime` 是单个 session 的运行时抽象(`types.ts:42-52`), 包含 `snapshot()` / `prompt()` / `steer()` / `abort()` / `setModel()` / `setThinking()` / `subscribe()` / `dispose()` 八个方法。这种接口设计让 server 层可以服务于任何 session 实现(目前是 JSONL + SQLite 双后端)。

### 2.5 Unix Socket 传输层:原子绑定 + 探活检测

```typescript
// packages/server/src/transports/unix/listener.ts:37-99
class UnixListener implements PiServerListener {
    async start(accept: ByteConnectionAcceptor): Promise<void> {
        const ownedBindPath = getOwnedBindPath(this.path);  // `.p-{sha256前8位}`
        await mkdir(dirname(this.path), { recursive: true, mode: 0o700 });
        await removeStaleSocket(this.path);     // 探活: createConnection → 1s 超时
        const server = createServer((socket) => this.acceptSocket(socket));
        await server.listen(ownedBindPath);     // 先绑到私有路径
        await link(ownedBindPath, this.path);   // 原子 link 到公开路径
        await setSocketMode(this.path, this.mode);  // 默认 0o600
    }
}
```

**关键设计**:
- **原子绑定**: 先 `listen` 到 `.p-{hash}` 私有路径, 再 `link` 到公开路径。避免"路径存在但 socket 未就绪"的竞态。
- **探活检测**(`listener.ts:351-376`): `isSocketLive()` 尝试 `createConnection`, 1s 超时。`ECONNREFUSED/ENOENT/EPIPE/ECONNRESET` 判定为死 socket, 允许覆盖。
- **背压控制**(`UnixByteConnection`, `listener.ts:204-305`): `pendingBytes + chunk.byteLength > maxPendingBytes` 时拒绝写入, 防止慢客户端拖垮服务端。默认 `maxPendingBytes = maxFrameLength * 4`。
- **优雅关闭**: `gracefulCloseTimeoutMs`(默认 5s) 后强制 `socket.destroy()`。

### 2.6 PiServerOptions 与工厂函数

```typescript
// packages/server/src/transports/unix/preset.ts:7-23
export function createUnixServer(service: PiServerService, options: UnixServerOptions): PiServer {
    const listener = createUnixListener({ path, mode, maxFrameLength, ... });
    return new PiServer(service, { listeners: [listener], ... });
}
```

**对 laew 借鉴**: 如果 laew 未来要支持服务化(多客户端连接同一个 Agent 后端), `PiServer` 的架构可直接参考: 将 `service` 接口映射到 laew 的 `MultiAgentOrchestrator`, 将 `PiSessionRuntime` 映射到 laew 的 `Session`。

---

## 三、Session Backends —— SQLite 后端完整实现

### 3.1 与 JSONL 后端的关系

第二轮分析已覆盖 JSONL 后端(`session-manager.ts`)的原子发布 + 撕裂尾部修复。本节深挖 **SQLite 后端**(`session-backends/sqlite-node`), 这是一个完整的、可用于生产环境的替代方案。

**核心差异对比**:

| 维度 | JSONL 后端 | SQLite 后端 |
|------|-----------|------------|
| 存储格式 | 一行一 JSON, append-only | WAL 模式, 结构化表 + 索引 |
| 事务性 | 文件级 rename 原子 | `BEGIN IMMEDIATE` + fence 写入者租约 |
| 并发读写 | 单写串行, 无并发保护 | 多读者单写者(WriterLease fence) |
| 查询能力 | 全文扫描 + 内存 filter | SQL WHERE + 索引 + FTS5 trigram |
| 搜索 | 无原生搜索 | `session_search_fts` FTS5 虚拟表 |
| Fork 支持 | 文件复制 | 单事务内批量 INSERT |
| 分支缓存 | 无 | `branch_entries` 表 + 按需重建 |

### 3.2 SQLite Schema:12 张表 + WriterLease

```sql
-- packages/session-backends/sqlite-node/src/sqlite/migrations/001_initial.sql:1-123
CREATE TABLE sessions (
    id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, cwd TEXT NOT NULL,
    parent_session_id TEXT NULL, metadata TEXT NULL
) WITHOUT ROWID;

CREATE TABLE entries (
    session_id TEXT NOT NULL, seq INTEGER NOT NULL, id TEXT NOT NULL,
    parent_id TEXT NULL, type TEXT NOT NULL, timestamp INTEGER NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY (session_id, id), UNIQUE (session_id, seq)
);

CREATE TABLE branch_entries (  -- 分支读缓存
    session_id TEXT NOT NULL, branch_id TEXT NOT NULL, entry_id TEXT NOT NULL,
    entry_seq INTEGER NOT NULL, entry_type TEXT NULL, custom_type TEXT NULL,
    PRIMARY KEY (session_id, branch_id, entry_id)
) WITHOUT ROWID;

CREATE TABLE writer_leases (  -- 写入者 fence
    session_id TEXT PRIMARY KEY, owner_id TEXT NOT NULL,
    fence INTEGER NOT NULL, expires_at_ms INTEGER NOT NULL
) WITHOUT ROWID;
```

**表设计要点**:
- `WITHOUT ROWID` 用于主键即查询维度的表(sessions / session_sequences / session_stats / lanes / writer_leases), 减少一层间接引用。
- `entries` 的 `PRIMARY KEY (session_id, id)` + `UNIQUE (session_id, seq)` 双约束: id 用于跨 session 唯一性, seq 用于顺序读取。
- `branch_entries` 是**派生缓存**, 不是权威数据。权威数据是 `entries.parent_id` 链。缓存通过 `rebuildBranchCache()` 按需重建。
- `records` 表有 6 个索引(`idx_records_session_lane_type_op_kind_seq` 等), 覆盖所有常见查询模式。
- `facts` 表存储 session 级元数据(name / label), 按 `seq` 递增 append, 读取时取 `MAX(seq)`。

### 3.3 WriterLease: fence + 心跳 + 事务内续租

这是 SQLite 后端最精巧的设计, 解决了"多进程写同一个 SQLite 文件"的竞态问题。

```typescript
// packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts:16-31
export function acquireWriterLease(db, sessionId, ownerId, now, expiresAtMs) {
    const row = sql`INSERT INTO writer_leases (session_id, owner_id, fence, expires_at_ms)
        VALUES (${sessionId}, ${ownerId}, 1, ${expiresAtMs})
        ON CONFLICT(session_id) DO UPDATE SET
            owner_id = excluded.owner_id,
            fence = writer_leases.fence + 1,
            expires_at_ms = excluded.expires_at_ms
        WHERE writer_leases.expires_at_ms <= ${now}
        RETURNING owner_id, fence, expires_at_ms`.get(db);
    return row === undefined ? undefined : { ownerId: row.owner_id, fence: row.fence, ... };
}
```

**fence 机制**: 每次抢占写入权时 `fence + 1`。续租时必须匹配 `owner_id + fence`, 这样旧 owner 的过期续租不会误更新新 owner 的租约。

```typescript
// repo.ts:394-415 — 心跳续租
private scheduleHeartbeat(): void {
    this.heartbeatTimer = setTimeout(async () => {
        await this.operations.enqueue(() => {
            this.db.transaction(() => {
                const now = Date.now();
                if (!renewWriterLease(this.db, this.metadata.id, this.lease, now, now + this.leaseOptions.ttlMs)) {
                    this.leaseError = lostWriterError(this.metadata.id);
                }
            });
        });
        this.scheduleHeartbeat();  // 递归调度
    }, this.leaseOptions.heartbeatIntervalMs);  // 默认 10s
}
```

**关键设计**:
- **默认 TTL 30s, 心跳 10s**: 每 10s 续一次, 若 30s 未续则其他进程可抢占。
- **事务内续租**: `enqueueWrite()` 在每个写操作开头先续租, 若续租失败(被抢占)立即抛 `lostWriterError`, 后续所有写操作都快速失败。
- **`SerialOperationQueue`**(`repo.ts:139-154`): 类似 Promise 链式队列, 确保所有写操作严格串行, 不会交叉。

### 3.4 SqliteSessionRepository: 创建 / Fork / 删除

```typescript
// repo.ts:720-742 — create
async create(options: SqliteSessionCreateOptions): Promise<Session<SqliteSessionMetadata>> {
    return this.operations.enqueue(async () => {
        const db = await this.getDatabase();
        const id = options.id ?? uuidv7();
        const lease = db.transaction(() => {
            insertSessionRow(db, { id, createdAt: Date.now(), cwd, ... });
            createSequence(db, id);   // session_sequences 表
            createStats(db, id);      // session_stats 表
            createInitialLane(db, id); // lanes 表, 默认 lane = "main"
            return claimWriterLease(db, id, this.leaseOptions);
        });
        return this.sessionFromLease(db, ..., lease);
    });
}
```

**Fork 支持**(`repo.ts:797-909`): 支持两种 scope:
- `scope: "tree"` — 复制整个 entry 树 + 所有 lane + 所有 branch_tip + name/label facts
- `scope: "branch"` — 只复制 main lane 的 main 分支到指定 entry, 可选 `position: "before" | "at"`

Fork 在单个事务中完成: `insertEntryRow` × N + `insertLane` × N + `appendFact` × N + `buildCachedBranch` × N + `claimWriterLease`。任何一步失败整个 fork 回滚。

### 3.5 BranchCache: 分支读缓存 + 增量追加

```typescript
// branch-cache.ts:70-101 — appendEntryToBranchCache
export function appendEntryToBranchCache(db, sessionId, entryId, entrySeq, entryType, customType, parentId) {
    if (parentId === null) {
        // 新根节点, 创建新 branch
        const branchId = uuidv7();
        insertBranchEntry(db, sessionId, branchId, entryId, ...);
        insertBranchTip(db, sessionId, entryId, branchId);
        return;
    }
    const tipBranchId = readBranchTipBranchId(db, sessionId, parentId);
    if (tipBranchId !== undefined) {
        // parent 是某 branch 的 tip, 延伸该 branch
        extendBranch(db, sessionId, tipBranchId, parentId, entryId, ...);
        return;
    }
    // parent 在某个 branch 中间, 复制路径 + 分叉新 branch
    const source = readBranchContainingEntry(db, sessionId, parentId);
    const branchId = uuidv7();
    copyBranchEntriesThroughSeq(db, sessionId, branchId, source.branchId, source.entrySeq);
    insertBranchEntry(db, sessionId, branchId, entryId, ...);
    insertBranchTip(db, sessionId, entryId, branchId);
}
```

**三种情况**:
1. **新根** → 新建 branch
2. **追加到 tip** → 延伸现有 branch(最常见)
3. **分叉** → 复制到分叉点 + 新 branch

**`rebuildBranchCache`**(`branch-cache.ts:19-29`): 从 entries 表重建所有 branch_tip, 用于损坏修复或迁移。

### 3.6 FTS5 全文搜索

```typescript
// search-backend.ts:65-88 — ensureSearchSchema
db.transaction(() => {
    sql`CREATE VIRTUAL TABLE IF NOT EXISTS session_search_fts USING fts5(
        payload, content = 'entries', content_rowid = 'rowid',
        tokenize = 'trigram remove_diacritics 1'
    )`.exec(db);
    // 自动同步触发器: INSERT/DELETE/UPDATE on entries
});
```

**搜索查询**(`search-backend.ts:135-189`): 用 FTS5 `MATCH` + `bm25()` 排序, 支持 `entryTypes` 过滤和 `limit` 截断。搜索结果通过 `AsyncIterable` 流式返回, 每行 yield 前检查 `AbortSignal`。

**对 laew 借鉴**: laew 的 `session_memory` 表只存储 Markdown 摘要。如果改为类似 Pi 的结构化存储, FTS5 trigram 搜索可以让用户在历史会话中全文检索, 比当前的纯文本注入更有价值。

---

## 四、Client —— 命令请求 + Session 租约 + 自动重连

### 4.1 架构:PiClient → Connection → ByteTransport

```
PiClient (业务层: list/create/attach/prompt/steer/abort)
  ├── Connection (协议层: hello 握手 + 帧解码 + 心跳)
  └── ByteTransport (传输层: Unix socket / WebSocket / ...)
```

```typescript
// packages/client/src/transport.ts:1-18
export interface ByteTransport {
    send(chunk: Uint8Array): Promise<void>;
    close(): void;
}
export type ByteTransportFactory = (handlers: ByteTransportHandlers) => ByteTransport | Promise<ByteTransport>;
```

**关键设计**: 传输层完全抽象, `ByteTransportFactory` 返回一个已连接的传输。客户端不关心底层是 Unix socket 还是 WebSocket。

### 4.2 连接状态机: 三阶段

```typescript
// packages/client/src/connection.ts:23-31
type ConnectionLifecycle =
    | { state: "disconnected" }
    | ({ state: "connecting"; handshake: PromiseResolvers<ServerSnapshot> } & ActiveConnection)
    | ({ state: "connected"; transport: ByteTransport; ... } & ActiveConnection);
```

握手流程(`connection.ts:119-140`):
1. `transportFactory(handlers)` 建立传输
2. 发送 `ClientHello { version: PROTOCOL_VERSION }`
3. 等待 `ServerHello` 或 `ServerHelloError`
4. 若成功 → `state: "connected"`, resolve `ServerSnapshot`

### 4.3 Session 租约: exclusive / shared 模式

```typescript
// packages/client/src/client.ts:381-393
#reserveSessionLease(sessionId: string, mode: SessionLeaseMode): SessionLeaseToken {
    const count = this.#sessionLeaseCounts.get(sessionId) ?? 0;
    if (mode === "exclusive" && count > 0) {
        throw new PiSessionOwnershipError(sessionId, `Session already has an active lease`);
    }
    if (mode === "shared" && this.#exclusiveSessionLeases.has(sessionId)) {
        throw new PiSessionOwnershipError(sessionId, `Session has an exclusive lease`);
    }
    const token: SessionLeaseToken = { mode };
    this.#sessionLeaseCounts.set(sessionId, count + 1);
    if (mode === "exclusive") this.#exclusiveSessionLeases.set(sessionId, token);
    return token;
}
```

**两种模式**:
- `exclusive` — `createSession()` 默认使用, 同一 session 只允许一个 exclusive lease
- `shared` — `attachSession()` 默认使用, 多个 shared lease 共存, 但不能与 exclusive 共存

**断线处理**(`client.ts:321-328`): 断线时清空所有 attach 状态, 递增所有 session 的 generation, 使现有 handle 全部失效(`#invalidateAllSessionLeases`)。重连后需要重新 attach。

### 4.4 SessionHandle: 命令代理 + 状态订阅

```typescript
// packages/client/src/session-handle.ts:47-111
export class SessionHandle implements SessionLease {
    async prompt(text: string): Promise<SessionSnapshot> {
        return (await this.#request({ command: "prompt", sessionId: this.id, text })).session;
    }
    async steer(text: string): Promise<SessionSnapshot> { ... }
    async abort(): Promise<SessionSnapshot> { ... }
    async setModel(model: ModelRef): Promise<SessionSnapshot> { ... }
    async setThinking(thinkingLevel: ThinkingLevel): Promise<SessionSnapshot> { ... }
}
```

**关键设计**: 所有命令都返回 `SessionSnapshot`, 实现了**命令查询分离(CQRS)**——客户端通过 snapshot 获取最新状态, 不需要额外的 GET 请求。

### 4.5 ClientState: revision 去重 + 分层订阅

```typescript
// packages/client/src/state.ts:100-104
applyServerSnapshot(snapshot: ServerSnapshot): void {
    if (this.#snapshot && snapshot.revision < this.#snapshot.revision) return;
    this.#snapshot = snapshot;
    this.#notify(this.#snapshotListeners, snapshot);
}
```

revision 单调递增, 旧 snapshot 被静默丢弃。支持两级订阅:
- `subscribe(listener)` — 全局 ServerSnapshot 变化
- `subscribeSession(sessionId, listener)` — 指定 Session 变化

**对 laew 借鉴**: laew 目前是单进程 CLI, 但如果要做 C/S 架构(如 VS Code 插件连接 laew 后端), Pi 的 Client-Server 分层是现成的参考模板。`SessionLease` 的 exclusive/shared 模式可防止多客户端并发修改同一 session。

---

## 五、Protocol —— CBOR 帧 + TypeBox Schema + 版本化

### 5.1 帧格式:4 字节大端长度前缀 + CBOR payload

```typescript
// packages/protocol/src/framing.ts:1-6
const FRAME_HEADER_LENGTH = 4;
const PAYLOAD_BLOCK_SIZE = 64 * 1024;
export const DEFAULT_MAX_FRAME_LENGTH = 16 * 1024 * 1024;  // 16 MB
```

`encodeFrame`(`framing.ts:28-39`): 将 payload 长度写入前 4 字节(大端), 拼接 payload。

`FrameDecoder`(`framing.ts:58-165`): 增量解码器, 处理任意 chunk 切分。使用 64KB 块链(`payloadBlocks`)避免大帧的连续内存分配。

### 5.2 CBOR 编解码

```typescript
// packages/protocol/src/cbor/index.ts
export { decodeCbor } from "./decoder.ts";
export { encodeCbor } from "./encoder.ts";
```

CBOR (Concise Binary Object Representation) 比 JSON 更紧凑, 适合二进制传输。配合 `maxByteLength` 限制防止超大消息。

### 5.3 消息类型: 双向对称

```typescript
// packages/protocol/src/schemas.ts:284-398
// 客户端 → 服务端
ClientMessage = ClientHello | RequestEnvelope
RequestEnvelope = { type: "request", id, request: Command }
Command = list | create | attach | detach | prompt | steer | abort | set_model | set_thinking

// 服务端 → 客户端
ServerMessage = ServerHello | ServerHelloError | ResponseEnvelope | EventEnvelope
ServerEvent = server_snapshot | session_snapshot | session_progress | session_removed
```

**TranscriptProgress**(`schemas.ts:204-231`): 流式增量推送, 四种类型:
- `item_started` — 新 transcript item 出现
- `assistant_delta` — 流式文本/thinking/toolCall 增量
- `item_updated` — assistant/tool item 中途更新(如 usage 统计)
- `item_finished` — item 终态(complete/error/aborted)

### 5.4 TypeBox Schema:编译期类型安全

```typescript
// packages/protocol/src/schemas.ts:4-8
const StrictObject = <const T>(properties: T) =>
    Type.Object(properties, { additionalProperties: false });
```

所有 schema 用 `typebox` 定义, 导出 `Static<typeof XxxSchema>` 类型。运行时用 `Check()` 验证。`StrictObject` 禁止额外属性, 防止协议漂移。

### 5.5 协议版本:整数递增

```typescript
// packages/protocol/src/schemas.ts:3
export const PROTOCOL_VERSION = 1 as const;
```

版本是整数而非 semver, 客户端握手时发送, 服务端 `isSupportedProtocolVersion()` 精确匹配。不支持"部分兼容"——版本不匹配直接断连。

### 5.6 server/protocol.ts: AI ↔ Protocol 消息桥接

server 包的 `protocol.ts` 是一个极其精细的类型桥接层, 用 TypeScript `Assert` 类型守卫确保 AI 内部模型与 Protocol schema 之间**字段级一一对应**:

```typescript
// packages/server/src/protocol.ts:24-78
type _AiThinkingLevelsFitProtocol = Assert<ModelThinkingLevel extends ThinkingLevel ? true : false>;
type _ProtocolThinkingLevelsFitAi = Assert<ThinkingLevel extends ModelThinkingLevel ? true : false>;
type _AiTextContentFieldsAccountedFor = Assert<ExactKeys<AiTextContent, "type" | "text" | "textSignature">>;
type _AiAssistantMessageFieldsAccountedFor = Assert<
    ExactKeys<AssistantMessage, "role" | "content" | "api" | "provider" | "model" | "responseModel"
        | "diagnostics" | "usage" | "stopReason" | "deferred" | "errorMessage" | "rawStopReason" | "endTurn" | "timestamp">
>;
```

**设计意图**: 如果 AI 层新增一个 `AssistantMessage` 字段(如 `cacheControl`), 但 `protocol.ts` 的 `ExactKeys` 断言没有更新, 编译器会立即报错。这强制开发者在 AI 层改动时同步更新协议层, 不会遗漏。

`toProtocolAssistantMessage`(`protocol.ts:283-337`) 将 AI 的 7 种 `stopReason`(pending/stop/length/toolUse/deferred/error/aborted) 映射到 Protocol 的 5 种 status(streaming/complete/error/aborted + deferred 被拒绝)。`deferred` 消息在 protocol v1 中不支持, 直接 `throw new TypeError`。

`sanitizeProtocolDetails`(`protocol.ts:166-186`) 是一个**防御性深度拷贝**: 将任意 JavaScript 值(lossy)转换为 JSON-safe 子集, 处理 `Date`(→ ISO string)、`bigint`(→ string)、`undefined/function/symbol`(→ undefined)、`Infinity/NaN`(→ string)、循环引用(`→ "[Circular]"`)。这保证了 tool result 的 `details` 字段不会因为包含不可序列化的值而破坏帧编码。

**对 laew 借鉴**: laew 的 `llm/anthropic.rs` 和 `llm/openai.rs` 中的消息转换缺少类似的编译期守卫。可以借鉴 `ExactKeys` 模式, 在 Rust 中用 trait + const assert 确保内部消息模型与协议输出的字段同步。

---

## 六、Evals —— 自动化评估框架

### 6.1 架构:pi-harness + vitest-evals + Reporter

```
pi-harness.ts          — 创建 Pi Agent 评估 harness
  └── vitest-evals/    — 框架扩展
      ├── artifacts.ts    — session.jsonl + source 持久化
      ├── harness-table.ts — baseline/candidate 配对 + 重复实验
      ├── reporter.ts     — Vitest Reporter: runs.jsonl + comparison
      └── summary.ts      — 统计比较: correctness lift / token delta / cost delta
```

### 6.2 PiCodingAgentHarness:完整 Agent 隔离运行

```typescript
// packages/evals/src/pi-harness.ts:109-244
async function runPiCodingAgent(input, signal, setArtifact, options) {
    const root = await mkdtemp(join(tmpdir(), "pi-eval-"));
    const cwd = join(root, "workspace");
    const agentDir = join(root, "agent");
    // 创建隔离的 AgentSessionServices (无真实文件系统)
    const services = await createAgentSessionServices({
        cwd, agentDir, modelRuntime,
        settingsManager: SettingsManager.inMemory(),
    });
    sessionManager = SessionManager.create(cwd, join(root, "sessions"));
    session = await createAgentSessionFromServices({ services, sessionManager, model, thinkingLevel: "off", noTools });
    // 执行多步输入(prompt + reload 交替)
    for (const step of steps) {
        if (step.type === "prompt") response = await promptAgent(session, step.content, signal);
        else await session.reload();
    }
    // 收集 usage / events / timings
    // 清理: rm(root, { recursive: true })
}
```

**关键设计**:
- **临时目录隔离**: 每次 eval 创建 `mkdtemp`, 结束后 `rm -rf`, 不污染宿主文件系统。
- **InMemorySettingsManager**: 不读磁盘配置, 保证 eval 可重现。
- **AbortSignal 支持**: 外部可随时中断 eval, 触发 `session.abort()`。
- **session.jsonl artifact**: 评估完成后把完整的 JSONL session 持久化到 `PI_EVAL_ARTIFACT_DIR`, 供离线分析。

### 6.3 Extension Authoring Eval:baseline vs candidate 对比

```typescript
// packages/evals/src/extensions.eval.ts:100-139
const extensionHarnessTable = evalHarnessTable("Pi extension authoring system prompt", {
    baseline: createExtensionAuthoringHarness("system-prompt-without-docs", excludeGuidelinesAndDocumentation),
    candidate: createExtensionAuthoringHarness("default-system-prompt", prepareDefaultPromptOverride),
});

describe.for(extensionHarnessTable)("$name", ({ harness }) => {
    describeEval("Pi extension authoring system prompt",
        { harness, judges: [ExtensionAuthoringJudge], judgeThreshold: null },
        (it) => {
            it("creates, reloads, and uses a hello extension", async ({ run }) => {
                const result = await run([
                    { type: "prompt", content: "Create a Pi extension with a hello tool..." },
                    { type: "reload" },
                    { type: "prompt", content: "Use the hello tool to greet Bob..." },
                ]);
            });
        },
    );
});
```

**Judge 机制**(`extensions.eval.ts:53-98`): `createJudge` 返回结构化评分:
- 检查 extension source 是否存在 + 是否导入 `@earendil-works/pi-coding-agent`
- 检查不能导入 `@mariozechner/`(旧包名) 或 `@sinclair/typebox`(旧依赖)
- 检查 `hello` tool 是否注册并被调用

**score**: 0 或 1, `judgeThreshold: null` 表示只记录不拦截。

### 6.4 Vitest Reporter: 统计比较引擎

`summary.ts` 实现了完整的**配对统计比较**(paired comparison):

```typescript
// packages/evals/src/vitest-evals/summary.ts:247-282
function summarizeCorrectness(pairs, totalPairs) {
    for (const { baseline, candidate } of pairs) {
        if (baseline.outcome !== "scored" || candidate.outcome !== "scored") continue;
        eligiblePairs += 1;
        const baselinePassed = baseline.score >= 1;
        const candidatePassed = candidate.score >= 1;
        if (baselinePassed) baselinePasses += 1;
        if (candidatePassed) candidatePasses += 1;
        if (baselinePassed === candidatePassed) ties += 1;
        else if (baselinePassed) baselineWins += 1;
        else candidateWins += 1;
    }
    return { totalPairs, eligiblePairs, baselinePassRate, candidatePassRate, lift, baselineWins, candidateWins, ties };
}
```

**lift 计算**: `candidatePassRate - baselinePassRate`, 单位是"百分点"(percentage points)。正数表示 candidate 更好, 负数表示 baseline 更好。

`summarizeMetric`(`summary.ts:212-245`) 对 token/latency/cost 三个维度做配对均值差计算, 用 `preciseDifference` 避免浮点精度问题。

**diagnostic 报告**(`summary.ts:164-194`): 5 种诊断原因:
- `missing-observation` — 某 harness 在某 group 完全没有运行
- `duplicate-observation` — 某 harness 在某 group 运行了多次
- `harness-error` — harness 运行出错
- `missing-score` — 运行成功但没有 score
- `unscorable-outcome` — outcome 不是 scored(如 skipped/pending)

这些诊断帮助开发者快速定位 eval 不可信的原因, 而不是默默给出错误的比较结论。

### 6.4 harness-table:baseline/candidate 配对 + 重复

```typescript
// packages/evals/src/vitest-evals/harness-table.ts:157-193
export function evalHarnessTable(evalSet, options) {
    const repetitions = options.repetitions ?? 1;
    const harnesses = [options.baseline, ...candidates];
    for (let repetition = 1; repetition <= repetitions; repetition += 1) {
        for (const harness of harnesses) {
            rows.push({ harness: withIterationArtifact(harness, plan), name: harness.name, repetition });
        }
    }
    return rows;
}
```

每个 harness 运行时附带 `EvalHarnessIterationArtifact`(含 `evalSet` / `groupKey` / `baseline` / `candidates` / `repetition`), 供 Reporter 做配对比较。

### 6.5 Reporter:runs.jsonl + 比较报告

```typescript
// packages/evals/src/vitest-evals/reporter.ts:14-49
async function appendHarnessRunReport(test: TestCase) {
    const record = {
        schemaVersion: 1, runId,
        test: { id, file, name, fullName, status },
        harness: harness.name,
        usage: run.usage,
        timings: run.timings,
        errors: run.errors,
        artifacts: await persistEvalArtifactReferences(test.artifacts(), runId, artifactDirectory),
    };
    await appendFile(join(artifactDirectory, "runs.jsonl"), `${JSON.stringify(record)}\n`);
}
```

**比较报告**(`summary.ts:300-328`): `summarizeHarnessComparisons` 输出:
- `CorrectnessLiftSummary` — baseline/candidate pass rate + lift(百分点差)
- `PairedMetricSummary` — token / latency / cost 的配对均值差
- `HarnessComparisonDiagnostic` — 缺失/重复/错误/无分/不可评 的诊断

报告格式化为带颜色的终端输出(`formatHarnessComparisonReport`, `summary.ts:374-438`)。

### 6.5 Artifact 持久化: session.jsonl + source

```typescript
// packages/evals/src/vitest-evals/artifacts.ts:87-113
export async function persistEvalArtifactReferences(artifacts, runId, artifactDirectory) {
    for (const artifact of artifacts) {
        const category = artifact.type === "@earendil-works/pi-evals:session" ? "sessions" : "sources";
        const directory = join(artifactDirectory, category, createHash("sha256").update(runId).digest("hex"));
        await mkdir(directory, { recursive: true, mode: 0o700 });
        const path = join(directory, name);
        await writeFile(path, attachment.body, { encoding: "utf8", mode: 0o600 });
    }
}
```

**两种 artifact 类型**:
- `@earendil-works/pi-evals:session` — 完整的 session.jsonl, 用于回放 agent 的完整推理过程
- `@earendil-works/pi-evals:source` — eval 生成的源文件(如 hello.ts extension), 用于验证代码质量

`setup.ts` 用 `afterEach` hook 自动记录 session artifact, 无需 eval case 手动处理。

**对 laew 借鉴**: laew 完全没有 Eval 基础设施。可借鉴:
1. `pi-harness.ts` 的隔离运行模式 — 创建临时 cwd + InMemory 配置 + AbortSignal
2. `evalHarnessTable` 的 baseline/candidate 配对 — 比较不同系统提示词或不同模型的效果
3. `runs.jsonl` 结构 — 供 CI 追踪 eval 趋势
4. `persistEvalArtifactReferences` 的 artifact 持久化 — 供离线分析 agent 推理过程

---

## 七、Telemetry —— Schema 驱动的 span 追踪

### 7.1 接口层:TelemetryContext + TelemetrySpan

```typescript
// packages/telemetry/src/index.ts:14-22
export interface TelemetryContext {
    startSpan<T>(options: SpanOptions, callback: (span: TelemetrySpan) => T | Promise<T>): Promise<T>;
}
export interface TelemetrySpan extends TelemetryContext {
    addEvent(name: string, attributes?: SpanAttributes): void;
    setAttributes(attributes: SpanAttributes): void;
    setStatus(status: SpanStatus): void;
}
```

**回调式 API**: `startSpan` 接受一个 callback, callback 内的操作自动被 span 包裹。callback 结束后 span 自动 settle(无论成功还是失败)。这比 OpenTelemetry 的 `span.end()` 手动模式更安全——不会忘记结束 span。

### 7.2 NOOP 实现:零开销默认

```typescript
// packages/telemetry/src/noop.ts:3-20
const noopTelemetrySpan: TelemetrySpan = {
    startSpan: startNoopSpan,
    addEvent: () => {}, setAttributes: () => {}, setStatus: () => {},
};
Object.freeze(noopTelemetrySpan);
export const NOOP_TELEMETRY_CONTEXT: TelemetryContext = noopTelemetrySpan;
```

`NOOP_TELEMETRY_CONTEXT` 是全局单例, 所有方法都是 no-op。当应用不配置 telemetry 时使用, 零开销。

### 7.3 InMemory 实现:测试用 span 录制

```typescript
// packages/telemetry/src/memory.ts:89-99
function settleSpan(state, span, failed, error?) {
    if (span.settled) return;
    if (failed && !span.explicitStatus) span.status = automaticErrorStatus(error);
    span.settled = true;
    span.endSequence = state.nextEndSequence++;
}
```

**关键设计**:
- `explicitStatus` 标志: 若 callback 内已手动 `setStatus`, 则错误时不覆盖。
- `endSequence`: 同一 parent 下, 先结束的 child 有更小的 sequence, 可用于判断执行顺序。
- **passive recording**: 所有 `setAttributes` / `addEvent` / `setStatus` 调用都 try-catch, 即使传入不可读的 Proxy 对象也不影响 callback 执行。

### 7.4 TelemetrySchemaDefinition:类型安全的 span 契约

```typescript
// packages/telemetry/src/index.ts:57-69
export interface TelemetrySpanDefinition {
    description: string;
    parents: TelemetryParentDefinition;
    startAttributes: Record<string, TelemetryStartAttributeDefinition>;
    endAttributes: Record<string, TelemetryAttributeDefinition>;
    events?: Record<string, TelemetryEventDefinition>;
    status: { default: "ok"; errorWhen: string };
}
export interface TelemetrySchemaDefinition {
    version: number;
    spans: Record<string, TelemetrySpanDefinition>;
}
```

**类型推导链**: `TelemetrySchemaDefinition` → `TelemetrySchemaSpanName` → `TelemetrySchemaSpanStartAttributes` → `InferStartAttributes` → `ExactTelemetryAttributes`。编译期强制每个 span 的属性名和类型符合 schema。

`createTypedSpanStarter`(`index.ts:349-354`) 绑定一个或多个 schema, 返回类型安全的 `TypedSpanStarter`, 调用时只能传已定义的 span name + 已定义的属性。`UniqueTelemetrySchemas` 约束(`index.ts:293-297`) 在编译期检查多个 schema 之间没有重复的 span name, 防止运行时歧义。

```typescript
// packages/telemetry/src/index.ts:317-343
export type TypedSpanStarter<Schemas extends TelemetrySchemaTuple> = UnionToIntersection<
    { [Name in SpanNameInSchemas<Schemas>]: TypedSpanStarterForName<Schemas, Name> }[...]
>;
```

`UnionToIntersection` 将所有 span name 的重载签名交叉合并, 使得 `startSpan("llm_request", { provider: "anthropic", ... })` 在编译期就校验 provider 必须是已声明的字面量类型。

### 7.5 Conformance 测试套件

```typescript
// packages/telemetry/src/testing/conformance.ts:61-315
export function createTelemetryAdapterConformance(factory) {
    return [
        createCase(factory, "callback lifecycle", "admits once synchronously ...", ...),
        createCase(factory, "callback lifecycle", "preserves synchronous and asynchronous rejection ...", ...),
        createCase(factory, "status", "uses last explicit status without automatic overwrite", ...),
        createCase(factory, "recording", "merges attributes and records ordered events", ...),
        createCase(factory, "recording", "ignores failed attribute calls atomically", ...),
        createCase(factory, "recording", "makes calls after settlement inert", ...),
        createCase(factory, "parentage", "records nested and concurrent child relationships", ...),
        createCase(factory, "passivity", "suppresses unreadable telemetry payload failures", ...),
    ];
}
```

**8 个 conformance case**, 覆盖: 同步/异步回调、错误传播、状态覆盖规则、属性合并、settlement 后惰性、嵌套 parent 关系、不可读 payload 容错。任何实现 `TelemetryContext` 的适配器都可以复用这套测试。

**对 laew 借鉴**: laew 没有任何 telemetry 基础设施。Pi 的 `NOOP → InMemory → Schema 驱动 → Conformance 测试` 四层设计是很好的渐进式引入模式:
1. P0: 引入 `TelemetryContext` 接口 + NOOP 实现(零代码改动)
2. P1: 用 `InMemoryTelemetryContext` 在测试中追踪 Agent 循环的 span
3. P2: 定义 schema, 让每个 Agent 角色的 span 属性有编译期保障
4. P3: 接入真实后端(OTLP/Jaeger)

---

## 八、遗漏包补全

### 8.1 packages/ai/ —— 45 Provider + 双后端 API

已有文档覆盖: 双协议(anthropic-messages / openai-completions)、11 thinkingFormat、cache miss 量化。

**本文补充**: 45 个 provider 声明(`providers/` 目录), 覆盖: Anthropic / OpenAI / Google / Azure / Bedrock / DeepSeek / Groq / Mistral / XAI / 月之暗面 / 阿里通义 / 百度 / 小米 / Cerebras / Together / Fireworks / OpenRouter / Cloudflare / NVIDIA / Baseten / HuggingFace / Vercel / Radius 等。

```typescript
// packages/ai/src/api/ — 14 个 API 实现文件
anthropic-messages.ts | openai-completions.ts | openai-responses.ts
openai-codex-responses.ts | azure-openai-responses.ts | bedrock-converse-stream.ts
google-generative-ai.ts | google-vertex.ts | mistral-conversations.ts
cloudflare.ts | pi-messages.ts | openrouter-images.ts
```

每个 API 实现对应一种 LLM wire 协议, `providers/*.ts` 声明式注册模型列表 + API 映射。

### 8.2 packages/coding-agent/ —— SessionManager JSONL + AgentLoop

已有文档覆盖较充分(Lane 并发、AgentLoop、Extension Runner)。

**本文补充**: `session-manager.ts` 是 JSONL 后端的核心, 定义了 11 种 session entry type:

```typescript
// packages/coding-agent/src/core/session-manager.ts:143-150
export type SessionEntry =
    | SessionMessageEntry | ThinkingLevelChangeEntry | ModelChangeEntry
    | CompactionEntry | BranchSummaryEntry | CustomEntry
    | LabelEntry | SessionInfoEntry | CustomMessageEntry
    | ActiveToolsChangeEntry;
```

**`CustomMessageEntry`**(与 `CustomEntry` 的区别): `CustomEntry` 不参与 LLM 上下文(纯持久化), `CustomMessageEntry` 会被注入 LLM 上下文(作为 user message)。

### 8.3 packages/agent/ —— AgentHarness 基类

已有文档覆盖: `AgentHarness` 是空壳(`unavailable()` 保护), 真实实现在 `coding-agent/server/create-harness.ts`。

### 8.4 scripts/ —— 35 个构建/发布/迁移脚本

```
scripts/
├── build-binaries.sh         # 多平台二进制构建
├── release.mjs / local-release.mjs / publish.mjs  # 发布管线
├── publish-model-catalog.mjs # 模型目录发布
├── generate-thinking-capabilities.mjs  # 生成 thinking 能力表
├── publish-release-announcement.mjs    # 发布公告
├── sync-versions.js          # 多包版本同步
├── cost.ts / stats.ts / tool-stats.ts  # 统计分析
├── session-transcripts.ts    # session 转录导出
├── session-context-stats.ts  # 上下文统计
├── generate-coding-agent-shrinkwrap.mjs  # 依赖锁定
└── ... (35 个脚本)
```

**对 laew 借鉴**: `generate-thinking-capabilities.mjs` 用于自动生成每个模型支持的 thinking level 列表, 避免手动维护。laew 可参考此模式自动生成 provider 配置。

---

## 九、对 laew 综合借鉴路线

### P0 — 直接可用(1-2 周)

| 借鉴项 | 来源 | laew 现状 | 实施建议 |
|--------|------|----------|---------|
| NOOP Telemetry 接口 | `telemetry/src/noop.ts` | 无 telemetry | 在 `agent/mod.rs` 引入 `TelemetryContext` trait + NOOP 实现 |
| WriterLease fence 模式 | `session-backends/sqlite-node/.../writer-leases.ts` | 单进程 SQLite | 若未来支持多进程, 用 fence 防竞态 |
| Eval 隔离运行模式 | `evals/src/pi-harness.ts` | 无 eval | 临时 cwd + mock LLM + AbortSignal |

### P1 — 中期目标(3-4 周)

| 借鉴项 | 来源 | laew 现状 | 实施建议 |
|--------|------|----------|---------|
| baseline/candidate eval 对比 | `evals/src/vitest-evals/harness-table.ts` | 无 | 比较不同系统提示词/模型的效果 |
| 结构化 session 存储 | `session-backends/sqlite-node/.../001_initial.sql` | `session_memory` 表(纯 Markdown) | 改为 entries + lanes + facts 结构化存储 |
| FTS5 全文搜索 | `session-backends/sqlite-node/.../search-backend.ts` | 无搜索 | 在 session_memory 上建 FTS5 trigram 索引 |
| Telemetry Schema 驱动 | `telemetry/src/index.ts` | 无 | 为每个 Agent 角色定义 span schema |

### P2 — 远期目标(5-8 周)

| 借鉴项 | 来源 | laew 现状 | 实施建议 |
|--------|------|----------|---------|
| C/S 架构(PiServer) | `server/src/server.ts` | 单进程 CLI | `PiServerService` 接口映射到 `MultiAgentOrchestrator` |
| Client SDK | `client/src/client.ts` | 无 | 供 VS Code 插件/远程终端连接 laew 后端 |
| CBOR 二进制协议 | `protocol/src/cbor/` | JSON 字符串传输 | 提升 LLM 响应解析性能 |
| Conformance 测试套件 | `telemetry/src/testing/conformance.ts` | 无 | 为 Telemetry 实现提供标准化测试 |

---

## 附录:源文件索引(30+ 文件)

| 文件路径 | 核心内容 |
|---------|---------|
| `packages/server/src/server.ts` | PiServer 类: 连接管理 + 握手 + 消息分发 |
| `packages/server/src/sessions.ts` | LiveSessionManager: 8 种 Command + 多连接 attach/detach |
| `packages/server/src/types.ts` | PiServerService / PiSessionRuntime 接口 |
| `packages/server/src/connection.ts` | ConnectionState: 五阶段状态机 |
| `packages/server/src/snapshots.ts` | ServerSnapshotPublisher: revision 广播队列 |
| `packages/server/src/protocol.ts` | AI ↔ Protocol 消息转换(type-safety 断言) |
| `packages/server/src/errors.ts` | PiServerError: 5 种错误码 |
| `packages/server/src/transports/unix/listener.ts` | UnixListener: 原子绑定 + 探活 + 背压 |
| `packages/server/src/transports/unix/preset.ts` | createUnixServer 工厂 |
| `packages/session-backends/sqlite-node/src/sqlite/migrations/001_initial.sql` | 12 张表 + 索引完整 DDL |
| `packages/session-backends/sqlite-node/src/sqlite/migrations.ts` | 迁移框架: migrations 表 + applyMigrations |
| `packages/session-backends/sqlite-node/src/sqlite/repo.ts` | SqliteSessionRepository: create/open/fork/delete |
| `packages/session-backends/sqlite-node/src/sqlite/branch-cache.ts` | 分支读缓存: 三种追加路径 |
| `packages/session-backends/sqlite-node/src/sqlite/search-backend.ts` | FTS5 trigram 全文搜索 |
| `packages/session-backends/sqlite-node/src/sqlite/types.ts` | SqliteDatabase / SqliteStatement 接口 |
| `packages/session-backends/sqlite-node/src/sqlite/sql.ts` | sql tagged template: 参数化查询构建器 |
| `packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts` | acquireWriterLease: fence + UPsert |
| `packages/session-backends/sqlite-node/src/sqlite/storage/sessions.ts` | Session CRUD + name JOIN 查询 |
| `packages/session-backends/sqlite-node/src/sqlite/storage/entries.ts` | Entry CRUD + 分页 + 类型过滤 |
| `packages/session-backends/sqlite-node/src/index.ts` | Node:DatabaseSync 适配层 |
| `packages/client/src/client.ts` | PiClient: 命令请求 + Session 租约管理 |
| `packages/client/src/connection.ts` | Connection: 三阶段状态机 + 传输层抽象 |
| `packages/client/src/session-handle.ts` | SessionHandle: prompt/steer/abort + AsyncDisposable |
| `packages/client/src/state.ts` | ClientState: revision 去重 + 分层订阅 |
| `packages/client/src/transport.ts` | ByteTransport / ByteTransportFactory 接口 |
| `packages/protocol/src/schemas.ts` | TypeBox Schema: 450 行完整协议定义 |
| `packages/protocol/src/codec.ts` | 编解码: CBOR + Frame + TypeBox Check |
| `packages/protocol/src/framing.ts` | FrameDecoder: 4 字节大端 + 64KB 块链 |
| `packages/evals/src/pi-harness.ts` | PiCodingAgentHarness: 隔离运行 + 多步输入 |
| `packages/evals/src/extensions.eval.ts` | Extension Authoring eval + Judge |
| `packages/evals/src/vitest-evals/harness-table.ts` | evalHarnessTable: baseline/candidate 配对 |
| `packages/evals/src/vitest-evals/reporter.ts` | EvalHarnessReporter: runs.jsonl + comparison |
| `packages/evals/src/vitest-evals/summary.ts` | summarizeHarnessComparisons: correctness lift + cost delta |
| `packages/evals/src/vitest-evals/artifacts.ts` | persistEvalArtifactReferences: session.jsonl 持久化 |
| `packages/telemetry/src/index.ts` | TelemetryContext + Schema 驱动类型安全 |
| `packages/telemetry/src/memory.ts` | InMemoryTelemetryContext: span 录制 |
| `packages/telemetry/src/noop.ts` | NOOP_TELEMETRY_CONTEXT: 零开销默认 |
| `packages/telemetry/src/testing/conformance.ts` | 8 个 conformance case |
