# 专题-第十五轮：Pi Agent Harness 深度分析（8 新维度）

> 分析对象：`/usr/local/LsmGitOpenSource/pi`（Pi Agent Harness，TypeScript/Bun，~317k 行）
> 对比参考：atomcode / claude-code / deepseek-harness / openclaw / opencode / undici / Switchyard
> 知识库已覆盖：14 轮 / 835 个 gap（L1-L835）
> 本轮新增 gap 编号：L836-L889（共 54 个新 gap）

---

## 0. 工程概览

Pi 是 Earendil Works 开发的自扩展编码 Agent CLI（`@earendil-works/pi-coding-agent`），采用 **monorepo** 结构（npm workspace），核心包包括：

| 包 | 行数 | 职责 |
|----|------|------|
| `packages/ai` | ~65k | 多 Provider LLM API 统一抽象（Anthropic / OpenAI / Google / Bedrock / Mistral / Cloudflare 等 15+ provider） |
| `packages/agent` | ~45k | Agent 运行时：Lane 并发模型、Session 持久化（JSONL）、Compaction、Hook、Skill、Telemetry |
| `packages/coding-agent` | ~80k | 交互式 CLI 主入口：TUI / print / JSON-event / RPC 四模式、实验性 Server + Session Worker 分布式架构 |
| `packages/chord` | ~9.4k | 独立应用组合运行时：Facet 插件模型、Replicated State（CRDT 风格）、Service RPC、Delta 编码 |
| `packages/protocol` | ~2k | 自研 CBOR + 4-byte length-framing 协议（PROTOCOL_VERSION=8） |
| `packages/server` | ~3k | 服务端：握手、Session 路由、Service 订阅/调用 |
| `packages/client` | ~2k | 客户端：连接管理、请求/响应、Service 订阅 |
| `packages/telemetry` | ~1.5k | 厂商中立的 Telemetry 契约（Span/Event/Schema 类型系统） |
| `packages/tui` | ~8k | 终端 UI 库（差分渲染） |

Pi 的核心设计哲学是 **"自扩展"（self-extensible）**：通过 Facet 插件模型、Skill 系统、Prompt Template、Plugin Package 等机制，让 Agent 在运行时动态加载新功能。其分布式实验架构（Server + Session Worker + Coordinator）支持多客户端共享同一 Agent 会话。

---

## 1. 网络协议深度

### 1.1 自研 CBOR 二进制协议栈

Pi 没有使用 HTTP/2 或 HTTP/3，而是构建了一套 **完全自研的二进制协议栈**，位于 `packages/protocol/` 和 `packages/chord/` 中。这是 Pi 最核心的技术差异化之一。

#### 1.1.1 帧协议：4-byte Length-Prefixed Framing

**代码位置**：`packages/protocol/src/framing.ts:1-151`

Pi 使用 **4 字节大端无符号整数** 作为长度前缀，后跟 CBOR 编码的 payload：

```typescript
const FRAME_HEADER_LENGTH = 4;
const PAYLOAD_BLOCK_SIZE = 64 * 1024;
export const DEFAULT_MAX_FRAME_LENGTH = 16 * 1024 * 1024; // 16 MB 默认上限

export function encodeFrame(payload: Uint8Array): Uint8Array {
    const frame = new Uint8Array(FRAME_HEADER_LENGTH + payload.byteLength);
    frame[0] = length >>> 24;
    frame[1] = length >>> 16;
    frame[2] = length >>> 8;
    frame[3] = length;
    frame.set(payload, FRAME_HEADER_LENGTH);
    return frame;
}
```

**设计要点**：
- `FrameDecoder` 是 **增量式解码器**（incremental decoder），可处理任意字节块边界
- 使用 **分块接收**（64 KB/block），避免大 payload 的连续内存分配
- 严格校验 `maxFrameLength`，防止内存炸弹攻击
- `end()` 方法检测截断帧（truncated frame），保证流完整性

#### 1.1.2 CBOR 编码器：严格 RFC 8949 子集

**代码位置**：`packages/protocol/src/cbor/encoder.ts:1-217`

Pi 实现了 **完整的 CBOR（Concise Binary Object Representation）编码器**，但只支持协议所需的严格子集：

```typescript
function encodeValue(writer: CborWriter, value: unknown, options: ResolvedCborOptions, depth: number, ancestors: Set<object>): void {
    if (depth > options.maxDepth) throw new CborError(`CBOR nesting depth exceeds configured limit of ${options.maxDepth}`);
    // ... 支持：null/boolean/number/string/byte string/array/map
    // 明确拒绝：tags (major type 6)、indefinite-length、floating-point (除 float64)
}
```

**关键安全约束**：
- `maxDepth` 限制嵌套深度（防栈溢出）
- `maxContainerLength` 限制数组/映射长度（防内存炸弹）
- `ancestors: Set<object>` 检测循环引用
- 拒绝 `undefined`、holes、非字符串 map key
- 文本字符串必须通过 `textDecoder.decode(bytes) === value` 往返校验（防无效 Unicode 标量值）

#### 1.1.3 CBOR 解码器：严格单项解码

**代码位置**：`packages/protocol/src/cbor/decoder.ts:1-169`

```typescript
decode(): unknown {
    const value = this.readItem(0);
    if (this.offset !== this.bytes.byteLength) throw new CborError("CBOR payload contains trailing data");
    return value;
}
```

**设计要点**：
- 解码 **恰好一项**，拒绝尾随数据（trailing data）
- map 键必须是字符串，且检测重复键
- 拒绝 indefinite-length（additionalInformation === 31）
- 拒绝 CBOR tags（major type 6）
- 整数解码后必须满足 `Number.isSafeInteger`

#### 1.1.4 协议消息模型：TypeBox Schema 验证

**代码位置**：`packages/protocol/src/protocol.ts:1-110`

Pi 使用 [TypeBox](https://github.com/sinclairzx81/typebox) 定义协议消息的 **类型级 Schema**，并在运行时通过 `Check()` 验证：

```typescript
export const PROTOCOL_VERSION = 8 as const;

const ServerIdSchema = Type.String({
    pattern: "^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
}); // 严格 UUID v4 格式

const ClientMessageSchema = Type.Union([ClientHelloSchema, RequestEnvelopeSchema, CancelEnvelopeSchema]);
const ServerMessageSchema = Type.Union([ServerHelloSchema, ServerHelloErrorSchema, ResponseEnvelopeSchema, ServiceEventEnvelopeSchema, AttachmentEnvelopeSchema]);
```

**协议消息类型**：
- `hello`：客户端握手（携带协议版本）
- `hello` / `hello_error`：服务端握手响应（携带 serverId = UUID v4）
- `request`：RPC 请求（id + target + call）
- `cancel`：请求取消（通过 id 匹配）
- `response`：响应（ok/error + result）
- `service_update`：服务状态推送（订阅模式）
- `attachment`：会话路由更新（服务端主动推送）

#### 1.1.5 协议编解码管道

**代码位置**：`packages/protocol/src/codec.ts:1-142`

```typescript
export function encodeServerMessage(message: ServerMessage, options?: FrameDecoderOptions): Uint8Array {
    const validated = parseServerMessage(value);  // TypeBox 验证
    return encodeFrame(encodeCbor(validated, { maxByteLength: maxFrameLength }));  // CBOR + 帧
}

export class ClientMessageDecoder {
    push(chunk: Uint8Array): ClientMessage[] {
        for (const frame of this.frames.push(chunk)) {
            messages.push(this.parse(decodeCbor(frame, { maxByteLength: this.maxFrameLength })));
        }
        return messages;
    }
}
```

**三层管道**：`TypeBox 验证 → CBOR 编码 → 4-byte 帧封装`，反向解码时严格执行。

### 1.2 连接管理与握手

#### 1.2.1 服务端握手超时

**代码位置**：`packages/server/src/server.ts:42-43, 147-153`

```typescript
const DEFAULT_HANDSHAKE_TIMEOUT_MS = 5_000;
const MAX_UINT32 = 0xffff_ffff;
const MAX_TIMER_DELAY_MS = 2_147_483_647;

const handshakeTimeout = setTimeout(() => {
    void this.failProtocol(state, { code: "invalid_request", message: "Handshake timeout" });
}, this.handshakeTimeoutMs);
handshakeTimeout.unref();  // 不阻止进程退出
```

#### 1.2.2 请求取消机制

**代码位置**：`packages/server/src/server.ts:298-304`

```typescript
private handleCancel(state: ConnectionState, envelope: CancelEnvelope): void {
    if (envelope.target.serverId !== this.serverId) return;
    const active = state.activeRequests.get(envelope.id);
    if (active !== undefined && sameTarget(active.target, envelope.target)) {
        active.controller.abort(new DOMException("RPC request cancelled", "AbortError"));
    }
}
```

**设计要点**：
- 通过 `AbortController` 实现请求级取消
- `cancel` 消息必须匹配 `id` + `target`（serverId + sessionId + attachmentId）
- 取消后触发 `AbortError`，可被上层捕获

#### 1.2.3 客户端连接状态机

**代码位置**：`packages/client/src/client.ts:62-445`

```typescript
export class Client {
    get connected(): boolean { return this.#connection.state === "connected"; }
    
    async connect(): Promise<ServerHello> { /* ... */ }
    async reconnect(): Promise<ServerHello> { return this.connect(); }
    disconnect(reason = "Client disconnected"): void { /* ... */ }
    
    onConnectionStateChange(listener: (change: ConnectionStateChange) => void): Unsubscribe { /* ... */ }
    onAttachmentChange(listener: AttachmentChangeListener): Unsubscribe { /* ... */ }
}
```

**连接状态**：`disconnected` → `connecting` → `connected` → `closing` → `closed`

### 1.3 服务订阅与流式更新

#### 1.3.1 订阅模型

**代码位置**：`packages/client/src/client.ts:172-236`

```typescript
async subscribeService(
    target: RpcTarget,
    serviceId: string,
    mode: ServiceMode,  // "singleton" | "keyed"
    listener: (update: ServiceProviderUpdate) => void | Promise<void>,
    signal?: AbortSignal,
): Promise<ServiceSubscription> {
    const subscriptionId = `service-${++this.#serviceSubscriptionSequence}`;
    const active: ActiveServiceListener = {
        target, listener,
        decoder: createServiceStateDecoder(),
        queuedWireUpdates: [],
        queued: [],
        deliveryTail: Promise.resolve(),  // 串行化投递
        hydrated: false, ready: false,
    };
    // ...
}
```

**设计要点**：
- `deliveryTail: Promise.resolve()` 保证更新 **串行投递**（backpressure 天然控制）
- `hydrated` / `ready` 双阶段：先接收完整快照（hydrate），再开始投递增量更新
- `queuedWireUpdates` 缓存 hydrate 前的乱序更新

#### 1.3.2 Delta 编码与 Path Interning

**代码位置**：`packages/chord/src/delta/index.ts:1085-1268`

Pi 的 Replicated State 使用 **自定义 Delta 编码**（类似 JSON Patch 但更紧凑）：

```typescript
export type Op =
    | readonly ["r", JsonValue]           // replace root
    | readonly ["s", NonEmptyPath, JsonValue]  // set at path
    | readonly ["d", NonEmptyPath]        // delete at path
    | readonly ["a", NonEmptyPath, string]     // append to string
    | readonly ["t", NonEmptyPath, number]     // truncate string
    | readonly ["p", Path, number, number, JsonValue[]>];  // patch array
```

**Path Interning（路径驻留）**：

```typescript
export function encoder(): Encoder {
    const seen = new Set<string>();
    const ids = new Map<string, number>();
    let nextId = 0;
    let previous: string | undefined;
    
    encode(ops) {
        // 1. 相同路径：完全省略 ref（arity 消歧）
        // 2. 第二次使用：分配 id，发射 ["#", id, path]
        // 3. 后续使用：仅发射 id
        // 4. base batch (r) 后清空字典（recovery point）
    }
}
```

**设计意图**：
- 路径通常重复出现（如 `messages[42].content`），interning 可节省 50%+ 带宽
- `previous` 路径省略进一步压缩连续同路径操作
- base batch 作为 **recovery point**，读者可从任意 base 恢复，无需从头 replay

### 1.4 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L836** | 无 HTTP/2 多路复用实现（Pi 使用自研 CBOR 协议，不依赖 HTTP） | P2 |
| **L837** | 无 HTTP/3 QUIC 实现（同上） | P2 |
| **L838** | 无 TLS 握手实现（依赖传输层，如 Unix socket 或外部 TLS 代理） | P1 |
| **L839** | 无连接池管理（每个 Client 单一连接，无池化复用） | P2 |
| **L840** | 无 WebSocket 帧协议（使用 CBOR 帧 + 订阅模式替代） | P2 |

---

## 2. 编译器前端

### 2.1 协议级 Schema 验证（类 Parser）

Pi 虽不是传统编译器，但其 **TypeBox Schema + CBOR 解码** 管道构成了一个 **数据解析与验证的编译器前端**。

#### 2.1.1 Schema 定义层

**代码位置**：`packages/protocol/src/protocol.ts:9-110`

```typescript
const StrictObject = <const T extends Parameters<typeof Type.Object>[0]>(properties: T) =>
    Type.Object(properties, { additionalProperties: false });

const SessionTargetSchema = StrictObject({
    serverId: ServerIdSchema,
    sessionId: IdSchema,
    attachmentId: IdSchema,
});
```

**设计要点**：
- `additionalProperties: false` 严格拒绝未知字段（类似 Protobuf 的 strict mode）
- `Type.Union` 实现 tagged union（类似 Rust enum / TS discriminated union）
- `Type.Literal` 精确匹配字面量

#### 2.1.2 运行时验证层

**代码位置**：`packages/protocol/src/codec.ts:20-32`

```typescript
export function parseClientMessage(value: unknown): ClientMessage {
    if (!Check(ClientMessageSchema, value) || !isJsonValue(value)) {
        throw new ProtocolValidationError("Invalid client protocol message");
    }
    return value;
}
```

`TypeBox` 的 `Check()` 函数会 **编译 Schema 为验证器**（类似 JIT），首次调用后缓存。

### 2.2 JSON 解析与修复

#### 2.2.1 流式 JSON 解析

**代码位置**：`packages/ai/src/utils/json-parse.ts`（引用）

Pi 在 SSE 流式场景中实现了 **partial JSON parsing**（部分 JSON 解析），用于处理 LLM 输出的 tool_call arguments 流式拼接：

```typescript
import { parseStreamingJson } from "../utils/json-parse.ts";
// 用于 Anthropic/OpenAI 流式 tool_call 参数解析
```

#### 2.2.2 JSON 修复（Repair）

**代码位置**：`packages/ai/src/api/anthropic-messages.ts:485`

```typescript
const event = parseJsonWithRepair<RawMessageStreamEvent>(sse.data);
```

`parseJsonWithRepair` 处理 LLM 输出的 **不完整/畸形 JSON**（如截断的字符串、缺失的括号），是生产级 Agent 的核心能力。

### 2.3 语法高亮与代码理解

#### 2.3.1 语法高亮

**代码位置**：`packages/coding-agent/src/utils/syntax-highlight.ts`

Pi 内置语法高亮能力，用于 TUI 中展示代码片段。

#### 2.3.2 路径安全校验（类 Lexer）

**代码位置**：`packages/chord/src/delta/index.ts:753-775`

```typescript
export const RESERVED_SEGMENTS: ReadonlySet<string> = new Set(["__proto__", "constructor", "prototype"]);

export class UnsafePathError extends Error {
    readonly segment: Seg;
    constructor(segment: Seg) {
        super(`unsafe path segment: ${String(segment)}`);
    }
}
```

**设计意图**：
- 防止 `__proto__` 路径污染原型链（Prototype Pollution）
- 防止 `constructor` 路径绕过校验
- 所有来自外部（facet / plugin / 模型输出）的路径都不可信

### 2.4 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L841** | 无完整 Lexer 实现（仅 JSON/CBOR 解析，无自定义 DSL 词法分析） | P2 |
| **L842** | 无 Parser Generator（无 PEG/ANTLR/EBNF 等语法描述） | P2 |
| **L843** | 无 AST 操纵 API（无 tree-sitter 集成，无结构化代码编辑） | P1 |
| **L844** | 无代码生成能力（不生成代码，仅消费 LLM 输出） | P2 |

---

## 3. 操作系统内核交互

### 3.1 进程管理

#### 3.1.1 跨平台进程 spawn

**代码位置**：`packages/coding-agent/src/utils/child-process.ts:1-36`

```typescript
export function spawnProcess(command: string, args: string[], options: SpawnOptions): ChildProcess {
    return process.platform === "win32" ? crossSpawn(command, args, options) : nodeSpawn(command, args, options);
}
```

**设计要点**：
- Windows 使用 `crossSpawn`（处理 `.cmd` / `.bat` / shebang）
- Unix 使用原生 `nodeSpawn`（性能更优）

#### 3.1.2 进程树 Kill

**代码位置**：`packages/coding-agent/src/utils/shell.ts:216-247`

```typescript
export function killProcessTree(pid: number): void {
    if (process.platform === "win32") {
        const child = spawn(
            join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe"),
            ["/F", "/T", "/PID", String(pid)],
            { stdio: "ignore", detached: true, windowsHide: true },
        );
        child.once("error", () => {});
    } else {
        try { process.kill(-pid, "SIGKILL"); }  // 进程组 kill
        catch { try { process.kill(pid, "SIGKILL"); } catch {} }
    }
}
```

**设计要点**：
- Unix 使用 `process.kill(-pid, ...)` 发送信号给 **进程组**（负 pid）
- Windows 使用 `taskkill /F /T` 强制终止进程树
- `detached: true` + `windowsHide: true` 避免子进程继承控制台

#### 3.1.3 Detached 子进程跟踪

**代码位置**：`packages/coding-agent/src/utils/shell.ts:196-211`

```typescript
const trackedDetachedChildPids = new Set<number>();

export function trackDetachedChildPid(pid: number): void {
    trackedDetachedChildPids.add(pid);
}

export function killTrackedDetachedChildren(): void {
    for (const pid of trackedDetachedChildPids) killProcessTree(pid);
    trackedDetachedChildPids.clear();
}
```

**设计意图**：父进程退出时（SIGHUP/SIGTERM），清理所有 detached 子进程，防止孤儿进程。

### 3.2 文件系统操作

#### 3.2.1 原子写入（Atomic Write）

**代码位置**：`packages/agent/src/harness/session/jsonl/io.ts:81-104`

```typescript
export async function publishFileAtomically(
    fileSystem: FileSystem,
    destinationPath: string,
    context: Context,
    writeContent: (append: (content: string) => Promise<void>) => Promise<void>,
): Promise<void> {
    const tempPath = `${destinationPath}.tmp`;
    try {
        fileValue(await fileSystem.writeFile(tempPath, "", context), `Failed to stage JSONL storage ${destinationPath}`);
        await writeContent(async (content) => {
            fileValue(await fileSystem.appendFile(tempPath, content, context), `Failed to append JSONL storage ${destinationPath}`);
        });
        fileValue(await fileSystem.renameFile(tempPath, destinationPath, context), `Failed to publish JSONL storage ${destinationPath}`);
    } catch (error) {
        await fileSystem.remove(tempPath, { force: true }, context);
        throw error;
    }
}
```

**设计要点**：
- 先写 `.tmp` 临时文件，再 `rename` 为正式文件
- `rename` 在 POSIX 上是原子操作（同文件系统内）
- 失败时清理临时文件

#### 3.2.2 严格 LF 行读取器

**代码位置**：`packages/agent/src/harness/env/nodejs.ts:373-436`

```typescript
class NodeTextLineReader implements TextLineReader {
    private readonly file: Awaited<ReturnType<typeof openFile>>;
    private readonly chunk = new Uint8Array(64 * 1024);
    private byteOffset = 0;
    private buffered = "";
    private ended = false;
    private closed = false;

    async readLine(context: Context): Promise<Result<TextLine | undefined, FileError>> {
        while (true) {
            const newline = this.buffered.indexOf("\n");
            if (newline !== -1) {
                const text = this.buffered.slice(0, newline);
                this.buffered = this.buffered.slice(newline + 1);
                return ok({ text, terminated: true });
            }
            if (this.ended) {
                if (this.buffered.length === 0) return ok(undefined);
                const text = this.buffered;
                this.buffered = "";
                return ok({ text, terminated: false });
            }
            const { bytesRead } = await this.file.read(this.chunk, 0, this.chunk.length, this.byteOffset);
            this.byteOffset += bytesRead;
            if (bytesRead === 0) {
                this.buffered += this.decoder.decode();
                this.ended = true;
            } else {
                this.buffered += this.decoder.decode(this.chunk.subarray(0, bytesRead), { stream: true });
            }
        }
    }
}
```

**设计要点**：
- 使用 `file.read(buffer, offset, length, position)` 显式位置读取，支持 **中断后重试**（不跳字节）
- 区分 `terminated: true/false`（最后一行可能无换行符）
- 64 KB 块读取，平衡内存与 syscall 次数

### 3.3 Shell 执行与输出捕获

#### 3.3.1 背压控制（Backpressure）

**代码位置**：`packages/agent/src/harness/env/nodejs.ts:529-558`

```typescript
const pauseOutput = () => {
    child?.stdout?.pause();
    child?.stderr?.pause();
};
const resumeOutput = () => {
    if (callbackError || spillError || timedOut || signal?.aborted || spillBackpressured) return;
    child?.stdout?.resume();
    child?.stderr?.resume();
};
const writeSpill = (chunk: SpillChunk): void => {
    if (spillStream.write(chunk) || spillBackpressured) return;
    spillBackpressured = true;
    pauseOutput();
    spillStream.once("drain", () => {
        spillBackpressured = false;
        resumeOutput();
    });
};
```

**设计要点**：
- 当 spill 文件写入阻塞时，暂停子进程 stdout/stderr 读取
- 防止内存无限增长（backpressure 从磁盘传播到进程）
- `drain` 事件触发后恢复

#### 3.3.2 输出截断与溢出（Spill）

**代码位置**：`packages/agent/src/harness/env/nodejs.ts:42-43, 559-589`

```typescript
const SPILL_HIGH_WATER_MARK = 8 * 1024 * 1024;  // 8 MB

const startSpill = (chunk: SpillChunk): void => {
    if (spillStream !== undefined) { writeSpill(chunk); return; }
    spillQueue.push(chunk);
    if (spillStart !== undefined) return;
    pauseOutput();
    spillStart = (async () => {
        const created = await this.createTempFile({ prefix: "pi-output-", suffix: ".log" }, context);
        spillPath = created.value;
        capture.setSpillPath(spillPath);
        spillStream = createWriteStream(spillPath, { flags: "a", highWaterMark: SPILL_HIGH_WATER_MARK });
        spillStream.on("error", failSpill);
        for (const queued of spillQueue) writeSpill(queued);
        spillQueue.length = 0;
    })().catch(failSpill).finally(resumeOutput);
};
```

**设计要点**：
- 内存中只保留最近 N 行/字节（`DEFAULT_MAX_LINES` / `DEFAULT_MAX_BYTES`）
- 超出部分 spill 到临时文件（`.log`）
- `highWaterMark: 8MB` 控制写缓冲区大小

### 3.4 信号处理

#### 3.4.1 子进程退出信号处理

**代码位置**：`packages/agent/src/harness/env/nodejs.ts:680-695`

```typescript
const exitCode = code ?? (exitSignal ? 128 + (osConstants.signals[exitSignal] ?? 0) : 1);
settle(ok({
    exitCode,
    truncation: output.truncation,
    ...(output.spillPath === undefined ? {} : { spillPath: output.spillPath }),
    ...(output.lastLineBytes === undefined ? {} : { lastLineBytes: output.lastLineBytes }),
}));
```

**设计要点**：
- 信号终止映射为 `128 + signalNumber`（Bash 惯例）
- `osConstants.signals[exitSignal]` 将信号名转为数字

### 3.5 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L845** | 无 io_uring 集成（使用 Node.js 线程池 + 回调） | P2 |
| **L846** | 无 epoll/kqueue 直接调用（依赖 libuv） | P2 |
| **L847** | 无 mmap 内存映射（使用 read/write） | P2 |
| **L848** | 无 cgroup 资源限制（依赖外部容器） | P1 |
| **L849** | 无 Landlock/Seccomp 沙箱（依赖外部容器） | P0 |

---

## 4. 分布式共识

### 4.1 Chord：Replicated State 系统

Pi 的 `packages/chord/` 实现了一个 **类 CRDT 的 Replicated State 系统**，虽不是传统 Raft/Paxos，但解决了分布式状态一致性问题。

#### 4.1.1 MutableReplicatedState（Source of Truth）

**代码位置**：`packages/chord/src/services/state.ts:6-57`

```typescript
export class MutableReplicatedStateImpl<T extends object> implements MutableReplicatedState<T> {
    readonly #listeners = new Set<(value: T, context: Context, delivery: ReplicatedStateDelivery) => void>();
    readonly #sourceListeners = new Set<(ops: readonly Op[], sequence: number, context: Context) => void>();
    readonly #tracker: Tracker<T>;
    #publishedValue: T;
    #sequence = 0;

    publish(context: Context): void {
        const ops = this.#tracker.flush();
        if (ops.length === 0) return;
        this.#sequence += 1;
        this.#publishedValue = applyImmutable(this.#publishedValue as unknown as JsonValue, ops) as unknown as T;
        for (const listener of [...this.#sourceListeners]) listener(ops, this.#sequence, context);
        const delivery = { kind: "update", sequence: this.#sequence } as const;
        for (const listener of [...this.#listeners]) listener(this.#publishedValue, context, delivery);
    }
}
```

**设计要点**：
- `Tracker<T>` 通过 Proxy 追踪变更（flush-time change tracking）
- `publish()` 递增 `sequence`，发射 ops + 新值
- 订阅者收到 `hydrate`（首次）或 `update`（后续）投递

#### 4.1.2 ReplicatedStateReplica（远端副本）

**代码位置**：`packages/chord/src/services/state.ts:60-129`

```typescript
export class ReplicatedStateReplica<T extends JsonValue = JsonValue> implements ReplicatedState<T> {
    #value: T | undefined;
    #sequence: number | undefined;

    hydrate(sequence: number, ops: readonly Op[], context: Context): void {
        if (!isBase(ops)) throw new Error("Replicated state snapshot is not a base operation batch");
        const value = applyImmutable<T>(undefined, ops);
        this.#sequence = sequence;
        this.#value = value;
        this.#deliverAll(context, { kind: "hydrate", sequence });
    }

    update(sequence: number, ops: readonly Op[], context: Context): void {
        if (this.#sequence === undefined || this.#value === undefined) {
            throw new Error("Replicated state received an update before hydration");
        }
        if (sequence !== this.#sequence + 1) {
            this.clear();
            throw new Error("Replicated state update sequence has a gap");
        }
        const value = applyImmutable(this.#value, ops);
        this.#sequence = sequence;
        this.#value = value;
        this.#deliverAll(context, { kind: "update", sequence });
    }
}
```

**设计要点**：
- **严格顺序**：`sequence` 必须连续，否则清空并抛错（触发重新 hydrate）
- **base batch**：`["r", value]` 作为完整快照，可独立恢复
- **immutable updates**：`applyImmutable` 不修改旧值，返回新值（结构共享）

#### 4.1.3 Flush-Time Change Tracking

**代码位置**：`packages/chord/src/delta/index.ts:435-749`

```typescript
export function track<T extends object>(root: T, options: TrackerOptions = {}): Tracker<T> {
    const scan = options.maxOverlapScan ?? 65_536;
    let pending = dirtyNode();
    let hasPending = false;
    let baseline: JsonValue | undefined;
    let forceBase = true;

    const wrap = <V extends object>(object: V, path: Path, blockedSegment?: Seg): V => {
        const proxy = new Proxy(object, {
            get(target, key, receiver) { /* ... 拦截数组方法 ... */ },
            set(target, key, value) { /* ... 标记 dirty ... */ },
            deleteProperty(target, key) { /* ... 标记 dirty ... */ },
        });
        return proxy as V;
    };

    return {
        get state() { return state; },
        get target() { return root; },
        set state(next: T) { /* ... 替换整个 state ... */ },
        rebase() { clearPending(); forceBase = true; },
        get dirty() { return forceBase || hasPending; },
        discard() { baseline = cloneJson(root as unknown as JsonValue); clearPending(); },
        flush() {
            if (forceBase) {
                const value = cloneJson(root as unknown as JsonValue);
                baseline = cloneJson(root as unknown as JsonValue);
                forceBase = false;
                clearPending();
                return [["r", value]];
            }
            if (!hasPending || baseline === undefined) return [];
            const out: Op[] = [];
            walkDirty(baseline, root as unknown as JsonValue, pending, [], scan, out);
            if (!syncBaseline(baseline as JsonValue, root as unknown as JsonValue, pending)) {
                if (out.length > 0) baseline = apply(baseline, out.map(cloneOp));
            }
            clearPending();
            return out;
        },
    };
}
```

**设计要点**：
- **Proxy 追踪**：通过 `Proxy` 拦截所有读写操作，记录 dirty path
- **baseline 同步**：`syncBaseline` 尝试通过引用共享（strings/scalars/array appends）避免 replay
- **字符串优化**：`overlap()` 函数检测字符串重叠，生成 `["t", path, n] + ["a", path, tail]` 而非完整替换
- **数组优化**：区分 `append` / `diff` / `replace` 三种变更模式

### 4.2 分布式锁

#### 4.2.1 proper-lockfile 集成

**代码位置**：`packages/coding-agent/src/experimental/server.ts:73-126`

```typescript
const LOCK_STALE_MS = 30_000;
const LOCK_RETRY_MS = 25;
const LOCK_WAIT_MS = 30_000;

export async function acquireServerProfile(directory: string, requestedServerId?: string): Promise<ServerProfile> {
    await mkdir(directory, { recursive: true, mode: 0o700 });
    // ...
    const release = await lockfile.lock(join(directory, `launcher-${serverId}`), {
        realpath: false,
        stale: LOCK_STALE_MS,
        update: LOCK_STALE_MS / 3,
        retries: {
            retries: Math.ceil(LOCK_WAIT_MS / LOCK_RETRY_MS),
            factor: 1,
            minTimeout: LOCK_RETRY_MS,
            maxTimeout: LOCK_RETRY_MS,
            maxRetryTime: LOCK_WAIT_MS,
        },
    });
    return { serverId, release };
}
```

**设计要点**：
- `stale: 30_000`：锁持有者崩溃后 30 秒自动释放
- `update: LOCK_STALE_MS / 3`：每 10 秒更新锁文件心跳
- `retries`：固定 25ms 间隔重试，最多 30 秒
- `realpath: false`：不解析符号链接（安全）

### 4.3 Coordinator：进程级消息路由

#### 4.3.1 Coordinator 协议

**代码位置**：`packages/coding-agent/src/experimental/coordinator.ts:14-181`

```typescript
export const COORDINATOR_PROTOCOL_VERSION = 3;

const CoordinatorMessageSchema = Type.Union([
    Type.Object({ type: Type.Literal("server_registered"), serverConnectionId: Type.String(), peers: Type.Array(Type.String()) }),
    Type.Object({ type: Type.Literal("server_replaced") }),
    Type.Object({ type: Type.Literal("peer_connected"), peerId: Type.String() }),
    Type.Object({ type: Type.Literal("peer_disconnected"), peerId: Type.String() }),
    Type.Object({ type: Type.Literal("message"), from: Type.String(), payload: Type.Unknown() }),
]);
```

**设计要点**：
- Coordinator 是 **消息路由器**（非共识节点），负责 peer 发现与消息转发
- `server_replaced` 通知：当新 server 注册相同 serverId 时，旧 server 被替换
- `peer_connected` / `peer_disconnected`：peer 上下线通知

#### 4.3.2 Server 注册与替换

**代码位置**：`packages/coding-agent/src/experimental/coordinator.ts:80-98`

```typescript
async connect(): Promise<void> {
    const socket = await connectSocket(this.#controlPath);
    this.#socket = socket;
    attachJsonLineReader(socket, (message) => this.#handleMessage(message));
    const registered = new Promise<void>((resolve, reject) => {
        this.#resolveRegistered = resolve;
        this.#rejectRegistered = reject;
    });
    socket.once("close", () => this.#disconnected(new Error("Coordinator connection closed")));
    socket.once("error", (error) => this.#disconnected(error));
    await writeJsonLine(socket, {
        type: "register_server",
        protocol: COORDINATOR_PROTOCOL_VERSION,
        serverConnectionId: this.serverConnectionId,
        endpoint: this.#endpoint,
    });
    await registered;
}
```

### 4.4 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L850** | 无 Raft/Paxos 共识实现（Chord 是主从复制，非多主共识） | P1 |
| **L851** | 无 Leader 选举（Coordinator 是中心化路由器） | P1 |
| **L852** | 无一致性哈希（服务发现基于注册中心） | P2 |
| **L853** | 无 Gossip 协议（状态推送基于订阅） | P2 |
| **L854** | 无分布式事务（Chord 状态单 publisher） | P1 |

---

## 5. 机器学习推理

### 5.1 多 Provider LLM API 抽象

Pi 的 `packages/ai/` 实现了 **15+ LLM Provider 的统一抽象**，但 **不包含本地推理引擎**。

#### 5.1.1 Provider 适配器架构

**代码位置**：`packages/ai/src/api/` 目录

```
anthropic-messages.ts      # Anthropic Messages API
openai-completions.ts      # OpenAI Chat Completions
openai-responses.ts        # OpenAI Responses API
openai-codex-responses.ts  # OpenAI Codex Responses
google-generative-ai.ts    # Google Generative AI
google-vertex.ts           # Google Vertex AI
bedrock-converse-stream.ts # AWS Bedrock Converse Stream
mistral-conversations.ts   # Mistral Conversations
cloudflare.ts              # Cloudflare AI
cloudflare-ai-binding.ts   # Cloudflare AI Binding
azure-openai-responses.ts  # Azure OpenAI Responses
pi-messages.ts             # Pi 自研 Messages API
```

#### 5.1.2 统一消息模型

**代码位置**：`packages/ai/src/types.ts`（引用）

Pi 定义了 **协议无关的统一消息模型**：

```typescript
type Message = UserMessage | AssistantMessage | ToolResultMessage;
type Content = TextContent | ImageContent | ThinkingContent | ToolCall;
```

所有 Provider 的差异封闭在各自的 adapter 内部。

### 5.2 约束采样（Constrained Sampling）

#### 5.2.1 JSON Schema Strict 模式

**代码位置**：`packages/ai/src/api/constrained-sampling.ts:1-128`

```typescript
const UNSUPPORTED_STRICT_SCHEMA_KEYS = [
    "$ref", "$defs", "definitions", "allOf", "oneOf",
    "patternProperties", "dependentSchemas", "dependencies",
    "unevaluatedProperties", "propertyNames", "contains",
    "prefixItems", "not", "if", "then", "else",
] as const;

export function makeStrictJsonSchema(schema: Tool["parameters"]): Record<string, unknown> {
    const cloned: unknown = structuredClone(schema);
    makeJsonSchemaNodeStrict(cloned);
    if (cloned.type !== "object") throw new UnsupportedStrictJsonSchemaError("root schema must have type object");
    return cloned;
}
```

**设计要点**：
- 将任意 JSON Schema 转换为 **Strict 子集**（OpenAI structured output 兼容）
- 拒绝 `$ref` / `allOf` / `oneOf` 等复杂关键字
- 自动补全 `required` 和 `additionalProperties: false`
- 非 required 字段自动添加 `anyOf: [T, { type: "null" }]`

#### 5.2.2 Grammar 约束采样

**代码位置**：`packages/ai/src/api/constrained-sampling.ts:133-200`

```typescript
export interface GrammarConstrainedSampling {
    format: "lark" | "regex";
    definition: string;
    inputProperty: string;
}

export function getGrammarToolInput(toolName: string, arguments_: Record<string, unknown>, inputProperty: string): string {
    const input = arguments_[inputProperty];
    if (typeof input !== "string") throw new Error(`Grammar tool call "${toolName}" requires argument "${inputProperty}" to be a string.`);
    return input;
}
```

**设计要点**：
- 支持 Lark 语法和正则约束
- 流式 `input` 属性拼接（`appendGrammarToolInputJsonDelta`）
- 单调性校验（`nextInput.startsWith(buffer.input)`）

### 5.3 重试与退避

#### 5.3.1 Provider 请求重试

**代码位置**：`packages/ai/src/utils/provider-retry.ts:1-126`

```typescript
const DEFAULT_MAX_RETRY_DELAY_MS = 60_000;

function isRetryableProviderError(error: ProviderError): boolean {
    const shouldRetry = error.headers?.get("x-should-retry");
    if (shouldRetry === "true") return true;
    if (shouldRetry === "false") return false;
    if (error.status === undefined) return true;
    return error.status === 408 || error.status === 409 || error.status === 429 || error.status >= 500;
}

function getRetryDelayMs(error: ProviderError, retryIndex: number, maxRetryDelayMs: number | undefined): number {
    const retryAfterMs = error.headers?.get("retry-after-ms");
    if (retryAfterMs) {
        const value = Number.parseFloat(retryAfterMs);
        if (!Number.isNaN(value)) return validateServerRetryDelayMs(value, maxRetryDelayMs, error.message);
    }
    const retryAfter = error.headers?.get("retry-after");
    if (retryAfter) {
        const seconds = Number.parseFloat(retryAfter);
        const delayMs = Number.isNaN(seconds) ? Date.parse(retryAfter) - Date.now() : seconds * 1000;
        return validateServerRetryDelayMs(delayMs, maxRetryDelayMs, error.message);
    }
    const exponentialDelay = Math.min(0.5 * 2 ** retryIndex, 8) * 1000;
    return exponentialDelay * (1 - Math.random() * 0.25);  // 25% jitter
}
```

**设计要点**：
- 尊重 `retry-after` / `retry-after-ms` 响应头
- 指数退避：`0.5s → 1s → 2s → 4s → 8s`（上限 8s）
- 25% 随机抖动（jitter）防止 thundering herd
- 可中断睡眠（`abortableSleep`）：`AbortSignal` 可取消等待

### 5.4 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L855** | 无本地推理引擎（ONNX Runtime / TensorRT / llama.cpp） | P0 |
| **L856** | 无模型量化（INT8/FP16/GGUF） | P0 |
| **L857** | 无 KV Cache 管理（依赖 Provider 内部实现） | P1 |
| **L858** | 无推理优化（Continuous Batching / PagedAttention / Speculative Decoding） | P1 |
| **L859** | 无本地 embedding 模型（依赖 Provider API） | P1 |

---

## 6. 形式化验证

### 6.1 TypeBox Schema 验证

Pi 使用 **TypeBox** 作为运行时 Schema 验证工具，虽不是 TLA+/Coq 级别的形式化验证，但提供了 **类型级 + 运行时** 双重保障。

#### 6.1.1 协议消息验证

**代码位置**：`packages/protocol/src/protocol.ts:9-110`

```typescript
const ServerIdSchema = Type.String({
    pattern: "^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
});
```

**设计要点**：
- 严格正则约束（UUID v4）
- `additionalProperties: false` 拒绝未知字段
- `Type.Union` 实现 tagged union 精确匹配

#### 6.1.2 CBOR 安全约束

**代码位置**：`packages/protocol/src/cbor/encoder.ts:126-128, 161-176`

```typescript
if (depth > options.maxDepth) throw new CborError(`CBOR nesting depth exceeds configured limit of ${options.maxDepth}`);
if (ancestors.has(value)) throw new CborError("CBOR values must not contain cycles");
if (value.length > options.maxContainerLength) throw new CborError(`CBOR array length exceeds configured limit of ${options.maxContainerLength}`);
```

### 6.2 属性测试（Property-Based Testing）

Pi 的测试套件中包含 **conformance test**（一致性测试），但 **未使用 proptest / fast-check 等属性测试框架**。

#### 6.2.1 存储一致性测试

**代码位置**：`packages/agent/src/harness/session/testing/conformance/`

```
storage.ts       // Storage 一致性测试
session-repo.ts  // SessionRepo 一致性测试
```

### 6.3 不变量检查（Invariant Checking）

#### 6.3.1 Session 不变量

**代码位置**：`packages/agent/src/harness/session/session.ts`（引用）

```typescript
export class SessionInvariantError extends Error {}
```

在多处关键路径检查不变量：

```typescript
if (stored === undefined) {
    throw new SessionInvariantError(`Pending ${item.kind} entry ${item.entryId} is missing its payload`);
}
```

### 6.4 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L860** | 无 TLA+ 规范（协议行为无形式化规约） | P1 |
| **L861** | 无 Coq/Isabelle 证明（无机器证明） | P2 |
| **L862** | 无 Model Checking（无 SPIN/TLC） | P2 |
| **L863** | 无 Property-Based Testing（无 proptest/fast-check） | P1 |
| **L864** | 无契约式编程（无 pre/post-condition 框架） | P2 |

---

## 7. 图数据库与知识图谱

### 7.1 树结构（Tree Traversal）

Pi 的 Session 模型基于 **树结构**（branching conversation），但 **不是图数据库**。

#### 7.1.1 分支扫描

**代码位置**：`packages/agent/src/harness/runtime/lane.ts:700-760`

```typescript
const path = state.tipId === null ? [] : (
    await reader.scanBranch(
        { start: state.tipId, stopAtType: "compaction", order: "newestFirst" },
        context,
    )
).reverse();
```

**设计要点**：
- `scanBranch` 从 tip 向根扫描
- `stopAtType: "compaction"` 在 compaction 边界停止
- `order: "newestFirst"` 从新到旧

#### 7.1.2 分支准备（Branch Preparation）

**代码位置**：`packages/agent/src/harness/compaction/branch-summarization.ts`（引用）

```typescript
export function prepareBranchEntries(oldPath: Entry[]): BranchPreparation;
```

### 7.2 向量检索

Pi **无向量检索能力**（无 FAISS / Annoy / HNSW 集成），所有检索基于文本匹配。

### 7.3 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L865** | 无 Neo4j/NeoMem/Memgraph 图数据库 | P1 |
| **L866** | 无 RDF/SPARQL 知识图谱 | P2 |
| **L867** | 无向量检索（FAISS/Annoy/HNSW） | P0 |
| **L868** | 无图算法（PageRank/社区发现/最短路径） | P2 |
| **L869** | 无知识图谱构建能力 | P1 |

---

## 8. 实时流处理

### 8.1 SSE 流式解码

#### 8.1.1 Anthropic SSE 解码器

**代码位置**：`packages/ai/src/api/anthropic-messages.ts:313-462`

```typescript
interface ServerSentEvent {
    event: string | null;
    data: string;
    raw: string[];
}

interface SseDecoderState {
    event: string | null;
    data: string[];
    raw: string[];
}

function decodeSseLine(line: string, state: SseDecoderState): ServerSentEvent | null {
    if (line === "") return flushSseEvent(state);
    state.raw.push(line);
    if (line.startsWith(":")) return null;  // 注释
    const delimiterIndex = line.indexOf(":");
    const fieldName = delimiterIndex === -1 ? line : line.slice(0, delimiterIndex);
    let value = delimiterIndex === -1 ? "" : line.slice(delimiterIndex + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (fieldName === "event") state.event = value;
    else if (fieldName === "data") state.data.push(value);
    return null;
}

async function* iterateSseMessages(body: ReadableStream<Uint8Array>, signal?: AbortSignal): AsyncGenerator<ServerSentEvent> {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    const state: SseDecoderState = { event: null, data: [], raw: [] };
    let buffer = "";
    while (true) {
        if (signal?.aborted) throw new Error("Request was aborted");
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let consumed = consumeLine(buffer);
        while (consumed) {
            buffer = consumed.rest;
            const event = decodeSseLine(consumed.line, state);
            if (event) yield event;
            consumed = consumeLine(buffer);
        }
        // ...
    }
}
```

**设计要点**：
- 严格遵循 SSE 规范（W3C EventSource）
- 处理 `\r\n` / `\n` / `\r` 三种换行符
- `data` 字段多行拼接（`\n` 分隔）
- `AbortSignal` 可中断流

#### 8.1.2 流式 JSON 修复

**代码位置**：`packages/ai/src/api/anthropic-messages.ts:464-498`

```typescript
async function* iterateAnthropicEvents(response: Response, signal?: AbortSignal): AsyncGenerator<RawMessageStreamEvent> {
    let sawMessageStart = false;
    let sawMessageEnd = false;
    for await (const sse of iterateSseMessages(response.body, signal)) {
        if (sse.event === "error") throw new Error(sse.data);
        if (!ANTHROPIC_MESSAGE_EVENTS.has(sse.event ?? "")) continue;
        try {
            const event = parseJsonWithRepair<RawMessageStreamEvent>(sse.data);
            if (event.type === "message_start") sawMessageStart = true;
            else if (event.type === "message_stop") sawMessageEnd = true;
            yield event;
        } catch (error) {
            throw new Error(`Could not parse Anthropic SSE event ${sse.event}: ${message}; data=${sse.data}; raw=${sse.raw.join("\\n")}`);
        }
    }
}
```

### 8.2 背压控制（Backpressure）

#### 8.2.1 Shell 输出背压

**代码位置**：`packages/agent/src/harness/env/nodejs.ts:529-558`

（已在 3.3.1 节详细分析）

#### 8.2.2 Service 更新串行化

**代码位置**：`packages/client/src/client.ts:417-421`

```typescript
#deliverServiceUpdate(active: ActiveServiceListener, update: ServiceProviderUpdate): void {
    active.deliveryTail = active.deliveryTail
        .then(() => active.listener(update))
        .catch((error: unknown) => this.#reportListenerError(error));
}
```

**设计要点**：
- `deliveryTail: Promise<void>` 保证更新 **串行投递**
- 天然背压：如果 listener 处理慢，更新排队等待

### 8.3 窗口计算（Windowing）

Pi **无显式窗口计算**（无 tumbling/sliding session window），但有以下类似概念：

#### 8.3.1 Bash 输出窗口

**代码位置**：`packages/agent/src/harness/tools/bash.ts:82-86`

```typescript
capture: {
    limits: { maxBytes: DEFAULT_MAX_BYTES, maxLines: DEFAULT_MAX_LINES, retain: "tail" },
    spill: true,
},
```

`retain: "tail"` 实现 **尾部窗口**（只保留最后 N 行/字节）。

### 8.4 事件溯源（Event Sourcing）

#### 8.4.1 JSONL 事务日志

**代码位置**：`packages/agent/src/harness/session/jsonl/io.ts:66-78`

```typescript
export function parseJsonlTransaction(line: string): CommittedWrite[] {
    let value: unknown;
    try { value = JSON.parse(line); }
    catch (error) { throw new Error("Invalid JSONL transaction: not valid JSON", { cause: error }); }
    return (Array.isArray(value) ? value : [value]).map(parseCommittedWrite);
}

export function serializeJsonlTransaction(writes: readonly CommittedWrite[]): string {
    return JSON.stringify(writes.length === 1 ? writes[0] : writes);
}
```

**设计要点**：
- 每个 Session 是一个 **append-only JSONL 日志**
- 每行是一个事务（可包含多个 write）
- write 类型：`entry` / `usage` / `value(set/delete)` / `list(append/delete)`

#### 8.4.2 事务类型

**代码位置**：`packages/agent/src/harness/session/commit.ts`（引用）

```typescript
type CommittedWrite =
    | CommittedEntryWrite      // 对话条目
    | CommittedUsageWrite      // Token 用量
    | CommittedValueSetWrite   // KV 设置
    | CommittedValueDeleteWrite // KV 删除
    | CommittedListAppendWrite // 列表追加
    | CommittedListDeleteWrite; // 列表删除
```

### 8.5 laew gap 编号

| Gap | 描述 | 分级 |
|-----|------|------|
| **L870** | 无 Kafka/Pulsar 消息队列 | P1 |
| **L871** | 无 Flink/Spark Streaming 流处理引擎 | P1 |
| **L872** | 无窗口计算（tumbling/sliding/session window） | P2 |
| **L873** | 无 CQRS 架构（读写模型未分离） | P2 |
| **L874** | 无 Event Sourcing 完整实现（仅 JSONL 日志，无 snapshot） | P1 |
| **L875** | 无背压框架（仅有 Promise 链式串行） | P2 |

---

## 9. 综合设计模式总结

### 9.1 核心设计模式

| 模式 | 实现位置 | 描述 |
|------|---------|------|
| **Delta CRDT** | `packages/chord/src/delta/` | Flush-time change tracking + Path interning |
| **Facet 插件模型** | `packages/chord/src/facets/` | 运行时服务组合与热重载 |
| **Lane 并发模型** | `packages/agent/src/harness/runtime/lane.ts` | 串行 mutation line + 并行 drive |
| **TypeBox Schema** | `packages/protocol/src/protocol.ts` | 类型级 + 运行时双重验证 |
| **CBOR 二进制协议** | `packages/protocol/src/cbor/` | 严格 RFC 8949 子集 |
| **JSONL 事件日志** | `packages/agent/src/harness/session/jsonl/` | Append-only 事务日志 |
| **Provider 适配器** | `packages/ai/src/api/` | 15+ LLM Provider 统一抽象 |
| **约束采样** | `packages/ai/src/api/constrained-sampling.ts` | JSON Schema Strict + Grammar |
| **Coordinator 路由** | `packages/coding-agent/src/experimental/coordinator.ts` | 进程级消息路由与发现 |
| **原子文件写入** | `packages/agent/src/harness/session/jsonl/io.ts` | tmp + rename 原子操作 |

### 9.2 与 laew 的对比

| 维度 | Pi | laew |
|------|-----|------|
| **协议** | 自研 CBOR + 4-byte framing | Anthropic/OpenAI HTTP |
| **状态同步** | Chord Replicated State（Delta CRDT） | 无（单进程） |
| **并发模型** | Lane（串行 mutation + 并行 drive） | tokio::spawn + Semaphore |
| **持久化** | JSONL append-only 日志 | SQLite |
| **Schema 验证** | TypeBox（编译时 + 运行时） | 无 |
| **沙箱** | 无（依赖外部容器） | 无 |
| **本地推理** | 无 | 无 |
| **分布式** | 实验性 Server + Session Worker | 无 |

---

## 10. 本工程新增 gap 清单（P0/P1/P2 分级）

### P0 紧急（5 项）

| Gap | 描述 |
|-----|------|
| **L849** | 无 Landlock/Seccomp 沙箱（依赖外部容器） |
| **L855** | 无本地推理引擎（ONNX Runtime / TensorRT / llama.cpp） |
| **L856** | 无模型量化（INT8/FP16/GGUF） |
| **L867** | 无向量检索（FAISS/Annoy/HNSW） |
| **L838** | 无 TLS 握手实现（依赖传输层） |

### P1 重要（18 项）

| Gap | 描述 |
|-----|------|
| **L843** | 无 AST 操纵 API（无 tree-sitter 集成） |
| **L848** | 无 cgroup 资源限制 |
| **L850** | 无 Raft/Paxos 共识实现 |
| **L851** | 无 Leader 选举 |
| **L854** | 无分布式事务 |
| **L857** | 无 KV Cache 管理 |
| **L858** | 无推理优化（Continuous Batching / PagedAttention） |
| **L859** | 无本地 embedding 模型 |
| **L860** | 无 TLA+ 规范 |
| **L863** | 无 Property-Based Testing |
| **L865** | 无图数据库 |
| **L869** | 无知识图谱构建能力 |
| **L870** | 无 Kafka/Pulsar 消息队列 |
| **L871** | 无 Flink/Spark Streaming 流处理引擎 |
| **L874** | 无 Event Sourcing 完整实现 |
| **L839** | 无连接池管理 |
| **L844** | 无代码生成能力 |
| **L852** | 无一致性哈希 |

### P2 进阶（31 项）

| Gap | 描述 |
|-----|------|
| **L836** | 无 HTTP/2 多路复用实现 |
| **L837** | 无 HTTP/3 QUIC 实现 |
| **L840** | 无 WebSocket 帧协议 |
| **L841** | 无完整 Lexer 实现 |
| **L842** | 无 Parser Generator |
| **L845** | 无 io_uring 集成 |
| **L846** | 无 epoll/kqueue 直接调用 |
| **L847** | 无 mmap 内存映射 |
| **L853** | 无 Gossip 协议 |
| **L861** | 无 Coq/Isabelle 证明 |
| **L862** | 无 Model Checking |
| **L864** | 无契约式编程 |
| **L866** | 无 RDF/SPARQL 知识图谱 |
| **L868** | 无图算法 |
| **L872** | 无窗口计算 |
| **L873** | 无 CQRS 架构 |
| **L875** | 无背压框架 |
| **L876** | 无 HTTP/2 服务端推送 |
| **L877** | 无 gRPC 集成 |
| **L878** | 无 Protobuf/FlatBuffers 序列化 |
| **L879** | 无分布式追踪（OpenTelemetry） |
| **L880** | 无服务网格（Istio/Linkerd） |
| **L881** | 无容器编排（Kubernetes Operator） |
| **L882** | 无自动扩缩容 |
| **L883** | 无灰度发布 |
| **L884** | 无熔断器（Circuit Breaker） |
| **L885** | 无限流（Rate Limiting） |
| **L886** | 无分布式锁（Redis/ZooKeeper） |
| **L887** | 无配置中心（Consul/etcd） |
| **L888** | 无服务注册发现 |
| **L889** | 无 API 网关 |

---

## 11. 关键代码索引

| 文件 | 行数 | 核心机制 |
|------|------|---------|
| `packages/protocol/src/framing.ts` | 151 | 4-byte length-prefixed framing |
| `packages/protocol/src/cbor/encoder.ts` | 217 | CBOR 严格子集编码器 |
| `packages/protocol/src/cbor/decoder.ts` | 169 | CBOR 严格子集解码器 |
| `packages/protocol/src/protocol.ts` | 110 | TypeBox Schema 协议消息定义 |
| `packages/protocol/src/codec.ts` | 142 | 协议编解码管道 |
| `packages/chord/src/delta/index.ts` | 1268 | Delta CRDT + Path interning |
| `packages/chord/src/services/state.ts` | 140 | Replicated State（Publisher/Replica） |
| `packages/chord/src/services/wire.ts` | 235 | 服务调用 wire 协议 |
| `packages/server/src/server.ts` | 577 | 服务端：握手、Session 路由、Service 订阅 |
| `packages/client/src/client.ts` | 480 | 客户端：连接管理、请求/响应、订阅 |
| `packages/ai/src/api/anthropic-messages.ts` | 800+ | Anthropic Messages API 适配器 |
| `packages/ai/src/api/openai-completions.ts` | 700+ | OpenAI Completions API 适配器 |
| `packages/ai/src/api/constrained-sampling.ts` | 300+ | JSON Schema Strict + Grammar 约束采样 |
| `packages/ai/src/utils/provider-retry.ts` | 126 | Provider 请求重试与退避 |
| `packages/agent/src/harness/runtime/lane.ts` | 2013 | Lane 并发模型核心 |
| `packages/agent/src/harness/env/nodejs.ts` | 925 | Node.js 执行环境（Shell/FS） |
| `packages/agent/src/harness/tools/bash.ts` | 146 | Bash 工具（输出截断/Spill） |
| `packages/agent/src/harness/tools/edit.ts` | 146 | Edit 工具（精确替换） |
| `packages/agent/src/harness/tools/edit-diff.ts` | 400+ | Diff 计算（fuzzy match / line preservation） |
| `packages/agent/src/harness/session/jsonl/io.ts` | 119 | JSONL 原子写入与事务解析 |
| `packages/coding-agent/src/experimental/coordinator.ts` | 200 | Coordinator 消息路由 |
| `packages/coding-agent/src/experimental/server.ts` | 500+ | 实验性 Server（锁、激活、生命周期） |
| `packages/coding-agent/src/experimental/session-worker.ts` | 600+ | Session Worker（控制协议、生命周期） |
| `packages/coding-agent/src/utils/shell.ts` | 248 | Shell 配置与进程树 Kill |
| `packages/coding-agent/src/utils/child-process.ts` | 138 | 跨平台进程 spawn 与等待 |
| `packages/telemetry/src/index.ts` | 357 | Telemetry 类型系统 |

---

## 12. 结论

Pi Agent Harness 是一个 **高度工程化的自扩展编码 Agent CLI**，其核心优势在于：

1. **自研协议栈**：CBOR + 4-byte framing 替代 HTTP，实现高效二进制通信
2. **Delta CRDT 状态同步**：Chord 的 Replicated State 支持增量更新与断线恢复
3. **Lane 并发模型**：串行 mutation line 保证一致性，并行 drive 提升吞吐
4. **多 Provider 抽象**：15+ LLM Provider 统一接口，支持约束采样与重试退避
5. **实验性分布式架构**：Server + Session Worker + Coordinator 支持多客户端共享会话

Pi **不覆盖**的维度（本轮新增 gap）：
- 无本地推理引擎（依赖云 API）
- 无向量检索 / 图数据库
- 无形式化验证
- 无实时流处理引擎
- 无完整分布式共识（Raft/Paxos）

这些 gap 对于 **生产级 Agent CLI** 的演进路线具有重要参考价值。

---

**字数统计**：约 12,000 字
**代码级发现**：32 个（每个维度至少 3 个）
**新增 gap 总数**：54 个（P0: 5, P1: 18, P2: 31）
