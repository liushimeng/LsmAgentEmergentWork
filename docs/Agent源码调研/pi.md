# Pi 综合深度分析

> 调研对象:pi(TypeScript,~12 包,lane 并发+一等公民 Skill)
> 调研日期:2026-09-04 ~ 2026-09-06
> 原始文档:7 份(含 1 份补充)
> 总行数:~3800 行(合并后)

---

## 目录

1. [项目元信息](#1-项目元信息)
2. [二进制帧协议](#2-二进制帧协议)
3. [Server 与 Session 后端](#3-server-与-session-后端)
4. [Operation Lane 三态 + reduceLaneState 事件溯源](#4-operation-lane-三态--reducelanestate-事件溯源)
5. [14 种损坏检测](#5-14-种损坏检测)
6. [流式与中断传播](#6-流式与中断传播)
7. [记忆与 Context](#7-记忆与-context)
8. [Skill 系统](#8-skill-系统)
9. [遥测](#9-遥测)
10. [会话持久化](#10-会话持久化)
11. [测试与 Eval](#11-测试与-eval)
12. [配置系统](#12-配置系统)
13. [系统提示词与 thinkingFormat](#13-系统提示词与-thinkingformat)
14. [错误处理与容错](#14-错误处理与容错)
15. [对 laew 的借鉴](#15-对-laew-的借鉴)

---

## 1. 项目元信息

| 项 | 值 |
|---|---|
| 仓库名 | `pi`(earendil-works 自研) |
| 根 `package.json` | `pi-monorepo`,`private: true`,`type: module`,`version 0.0.3` |
| 语言 | TypeScript(Node ≥ 22.19) |
| 包管理 | npm + workspaces(`save-exact=true`,`min-release-age=2`) |
| 单测 | Vitest + Node `--test` |
| Lint/Format | Biome 2.3.5 |
| 类型检查 | `tsgo --noEmit`(TypeScript 7.0 native preview) |
| Release 工具 | Bun 编译 standalone |
| 依赖锁定 | `package-lock.json` 是 ground truth |
| 顶层入口 | `packages/coding-agent/src/main.ts`(CLI),`packages/coding-agent/src/index.ts`(SDK) |
| 仓库类型 | npm workspaces(monorepo) |

构建脚本按依赖顺序编译:`tui → telemetry → ai → agent → session-backends/sqlite-node → protocol → client → server → coding-agent`,共 12 个子包。

### 1.1 目录树(顶层包)

```
pi/
├── packages/
│   ├── ai/                       @earendil-works/pi-ai          多 Provider LLM 统一 API
│   ├── agent/                    @earendil-works/pi-agent-core  Agent 运行时
│   ├── coding-agent/             @earendil-works/pi-coding-agent 交互式 CLI
│   ├── tui/                      @earendil-works/pi-tui         TUI 渲染库(差异渲染)
│   ├── telemetry/                @earendil-works/pi-telemetry   Telemetry contracts
│   ├── protocol/                 JSONL / RPC 协议(CBOR 帧)
│   ├── client/                   RPC client
│   ├── server/                   RPC server(对应 client)
│   ├── session-backends/
│   │   └── sqlite-node/          SQLite session 后端
│   └── evals/                    @earendil-works/pi-evals       评测
├── scripts/                      构建/发布/check 脚本
└── .pi/                          仓库自身的 pi 配置
```

### 1.2 架构分层

```
┌──────────────────────────────────────────────────────────┐
│ coding-agent   (CLI / Modes: interactive / rpc / print) │
│   ↳ AgentSession (~350 行)  生命周期/会话/事件           │
├──────────────────────────────────────────────────────────┤
│ agent-core     (Agent / AgentLoop / Harness)             │
│   ↳ agent.ts / agent-loop.ts  纯循环                     │
│   ↳ harness/   并发 lane、compaction、skill、session     │
├──────────────────────────────────────────────────────────┤
│ pi-ai          (Models.streamSimple / 30+ Providers)     │
│   ↳ EventStream(AssistantMessageEventStream)             │
├──────────────────────────────────────────────────────────┤
│ pi-tui         (差异渲染 TUI 库)                          │
└──────────────────────────────────────────────────────────┘
```

### 1.3 核心特征

| 能力 | 支持 | 位置 / 说明 |
|---|---|---|
| 多 Agent | 部分 | `examples/extensions/subagent/` 演示通过外部 `pi` 子进程隔离上下文(每个子 agent 独立进程),**不内置** |
| 任务分类 | 无 | LLM 自由决定,不显式分类 |
| MCP | **明确不支持** | `coding-agent/README.md:499` 作者 Mario Zechner 反对 MCP |
| Skill | 支持,一等公民 | `harness/skills.ts`(386 行)+ `core/skills.ts`(507 行) |
| Session | 强大 | 树状条目、JSONL 持久化、可恢复、可分支、SQLite backend 可选 |
| 多 Provider | 广泛 | 30+ 内置,统一 `Models.streamSimple` |
| Lane 并发 | 是 | `AgentHarness` 内 lane 模型,run/compact/navigation 三类操作 |
| 扩展 | 是 | `core/extensions/` loader/runner/types/wrapper,内置 50+ 示例扩展 |

---

## 2. 二进制帧协议

Pi server ↔ client 通过**二进制帧协议**通信,格式为 **4 字节大端长度前缀 + CBOR payload**。

### 2.1 帧格式

```typescript
// packages/protocol/src/framing.ts
const FRAME_HEADER_LENGTH = 4;
const PAYLOAD_BLOCK_SIZE = 64 * 1024;
export const DEFAULT_MAX_FRAME_LENGTH = 16 * 1024 * 1024;  // 16 MB

export function encodeFrame(payload: Uint8Array): Uint8Array {
  const frame = new Uint8Array(FRAME_HEADER_LENGTH + payload.byteLength);
  frame[0] = length >>> 24; frame[1] = length >>> 16;
  frame[2] = length >>> 8;  frame[3] = length;
  frame.set(payload, FRAME_HEADER_LENGTH);
  return frame;
}
```

`FrameDecoder`(`framing.ts:58`) 是增量解码器: `push(chunk): Uint8Array[]`,使用 64KB 块链避免大帧的连续内存分配。`end()` 检测截断帧。

### 2.2 CBOR 编解码

```typescript
// packages/protocol/src/cbor/
export { decodeCbor } from "./decoder.ts";
export { encodeCbor } from "./encoder.ts";
```

CBOR (Concise Binary Object Representation) 比 JSON 更紧凑,配合 `maxByteLength` 限制防止超大消息。

### 2.3 消息 Schema

```typescript
// packages/protocol/src/schemas.ts
export const PROTOCOL_VERSION = 1 as const;

// 客户端 → 服务端
ClientMessage = ClientHello | RequestEnvelope
ClientHello { type:"hello", version: integer }
RequestEnvelope { type:"request", id, request: Command }

// 服务端 → 客户端
ServerMessage = ServerHello | ServerHelloError | ResponseEnvelope | EventEnvelope
ServerHello { type:"hello", version, connectionId, snapshot: ServerSnapshot }
ResponseEnvelope { type:"response", id, ok: true, result } | { ok:false, error }
EventEnvelope { type:"event", event: ServerEvent }

// 9 种 Command
Command = list | create | attach | detach | prompt | steer | abort | set_model | set_thinking

// 4 种 ServerEvent
ServerEvent = server_snapshot | session_snapshot | session_progress | session_removed

// 5 种 SessionPhase
SessionPhase = "idle" | "turn" | "compaction" | "branch_summary" | "retry"
```

所有 schema 用 `typebox` 定义,`StrictObject` 禁止额外属性,防止协议漂移。`sanitizeProtocolDetails` 是防御性深度拷贝,将任意 JavaScript 值转换为 JSON-safe 子集,处理 `Date`、`bigint`、`undefined`、循环引用。

### 2.4 Server→AI 协议桥接

```typescript
// packages/server/src/protocol.ts
type _AiThinkingLevelsFitProtocol = Assert<ModelThinkingLevel extends ThinkingLevel ? true : false>;
type _ProtocolThinkingLevelsFitAi = Assert<ThinkingLevel extends ModelThinkingLevel ? true : false>;
type _AiAssistantMessageFieldsAccountedFor = Assert<
    ExactKeys<AssistantMessage, "role" | "content" | "api" | "provider" | "model" | "responseModel"
        | "diagnostics" | "usage" | "stopReason" | "deferred" | "errorMessage" | "rawStopReason" | "endTurn" | "timestamp">
>;
```

如果 AI 层新增字段但 `protocol.ts` 的 `ExactKeys` 断言没有更新,编译器会立即报错。这强制开发者在 AI 层改动时同步更新协议层。

---

## 3. Server 与 Session 后端

### 3.1 PiServer 架构

```typescript
// packages/server/src/server.ts
export class PiServer {
    readonly id: string;
    private readonly listeners: readonly PiServerListener[];
    private readonly sessions: LiveSessionManager;
    private readonly snapshots: ServerSnapshotPublisher;
}
```

`LiveSessionManager` 不直接持有数据库或 LLM,通过 `PiServerService` 回调与应用层解耦。任何实现了 `PiServerService` 的后端都能挂载。

### 3.2 5 阶段连接状态机

```typescript
// packages/server/src/connection.ts
export type ConnectionStage = "awaitingHello" | "handshaking" | "ready" | "closing" | "closed";
```

握手流程:
1. 客户端发 `ClientHello`(含 `version: PROTOCOL_VERSION`)
2. 服务端校验版本,返回 `ServerHello`
3. 若握手期间 snapshot revision 已变,立即补发 `server_snapshot`
4. 握手超时(默认 5s) → 发 `hello_error` → 断连

### 3.3 8 种 Session Command

```typescript
// packages/server/src/sessions.ts
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

`requireAttached` 确保只有 attach 到 session 的连接才能发送 prompt/steer/abort,防止越权操作。

### 3.4 Unix Socket 传输层

```typescript
// packages/server/src/transports/unix/listener.ts
class UnixListener implements PiServerListener {
    async start(accept: ByteConnectionAcceptor): Promise<void> {
        const ownedBindPath = getOwnedBindPath(this.path);  // `.p-{sha256前8位}`
        await removeStaleSocket(this.path);     // 探活: createConnection → 1s 超时
        await server.listen(ownedBindPath);     // 先绑到私有路径
        await link(ownedBindPath, this.path);   // 原子 link 到公开路径
    }
}
```

关键设计:
- **原子绑定**: 先 `listen` 到 `.p-{hash}` 私有路径,再 `link` 到公开路径,避免竞态
- **探活检测**: `isSocketLive()` 尝试 `createConnection`, 1s 超时
- **背压控制**: `pendingBytes + chunk.byteLength > maxPendingBytes` 时拒绝写入
- **优雅关闭**: `gracefulCloseTimeoutMs`(默认 5s) 后强制 `socket.destroy()`

### 3.5 Session 租约: exclusive / shared 模式

```typescript
// packages/client/src/client.ts
#reserveSessionLease(sessionId: string, mode: SessionLeaseMode): SessionLeaseToken {
    const count = this.#sessionLeaseCounts.get(sessionId) ?? 0;
    if (mode === "exclusive" && count > 0) {
        throw new PiSessionOwnershipError(sessionId, `Session already has an active lease`);
    }
    if (mode === "shared" && this.#exclusiveSessionLeases.has(sessionId)) {
        throw new PiSessionOwnershipError(sessionId, `Session has an exclusive lease`);
    }
}
```

两种模式:
- `exclusive` — `createSession()` 默认使用
- `shared` — `attachSession()` 默认使用,多个 shared lease 共存

---

## 4. Operation Lane 三态 + reduceLaneState 事件溯源

### 4.1 Lane 概念

Lane 是 pi 的**并发执行单元**,本质是 session tree 上的一个命名分支指针。每个 lane 拥有独立的 `leafId`、运行队列、操作状态。

```typescript
// packages/agent/src/harness/agent-harness.ts
export interface LaneInfo {
    name: string;
    leafId: string | null;
    operation: null | {
        id: string;
        kind: "run" | "compaction" | "navigation";
        status: "running" | "suspended" | "aborting";
    };
}
```

三态 status 对应不同的 UI 渲染策略和事件流分叉。

### 4.2 OperationStartedRecord 三意图

```typescript
// packages/agent/src/harness/session/types.ts
export interface OperationStartedRecord extends RecordBase {
    type: "operation_started";
    sourceLeafId: string | null;
    intent:
        | { kind: "run"; originalPrompt: AgentMessage[]; initialMessages: ProvisionedEntry[]; ... }
        | { kind: "compaction"; customInstructions?: string; resultEntryId: string; }
        | { kind: "navigation"; targetId: string | null; summarize: boolean; ... };
}
```

三种 intent 是**操作模式分类**:`run` 是常规对话循环,`compaction` 是显式压缩,`navigation` 是 session tree 分支跳转。`resultEntryId` 预分配是个巧妙设计——LLM 还没返回结果时,占位 ID 已经写入 JSONL,崩溃恢复时能精确定位。

### 4.3 reduceLaneState 事件溯源

```typescript
// packages/agent/src/harness/reducer.ts
export function reduceLaneState(input: LaneReductionInput): LaneReductionResult {
    validateRecordLog(input);          // 先验证记录一致性
    const records = bySequence(input.records);
    const ownEntries = bySequence(input.ownEntries);
    // ... 重建 LaneState
}
```

Pi 没有"内存中保存 lane 状态",而是从持久化记录**纯函数重建**整个 lane 运行时状态:
- **崩溃后恢复**: 从 JSONL 重读 → 重建 LaneState → 决定 resume / suspend / declined
- **多端同步**: 服务端记录 logs,客户端用同一函数重建 UI
- **测试可重现**: 固定输入 → 固定输出,无需 mock 运行时

### 4.4 Deferred Handle 异步结果

```typescript
export interface DeferredHandle {
    provider: string;
    modelId: string;
    api: string;
    id: string;          // provider-specific token
    expiresAt?: number;
    pollAfterMs?: number;
    data?: JsonValue;
}
```

当 LLM 返回 `stopReason: "deferred"` 时(Anthropic batch API),lane 进入 `suspended` 状态,稍后通过 `resume()` 恢复并 fetch deferred 结果。

---

## 5. 14 种损坏检测

`validateRecordLog` 执行严格的协议一致性检查,防止损坏的持久化数据导致运行时错误:

| # | reason | 含义 |
|---|--------|------|
| 1 | `multiple_open_operations` | 单 lane 同时存在 ≥2 个 open operation |
| 2 | `unknown_operation` | record 引用了不存在的 runId |
| 3 | `record_after_finish` | record 出现在 operation_finished 之后 |
| 4 | `non_consecutive_attempt` | attempt 编号不连续(跳号) |
| 5 | `invalid_compaction_reason` | compaction 缺少 reason,或非 compaction 携带 reason |
| 6 | `queue_after_abort` | abort 之后仍 enqueue steering |
| 7 | `invalid_queue_cancellation` | 取消不存在的队列项,或目标 entry 已存在 |
| 8 | `inconsistent_step` | 同 step 的多次 attempt 的 resultEntryId/compactionReason 不一致 |
| 9 | `tool_call_mismatch` | tool_started 与 assistant message 的 toolCall 不匹配 |
| 10 | `duplicate_tool_invocation` | 同一 invocation 被记录两次 |
| 11 | `provisioned_entry_mismatch` | provisioned id 存在但内容不同 |
| 12 | `invalid_deferred_handle` | deferred assistant message 缺少 handle |

`corrupt()` 直接抛 `SessionError`,上层可捕获并提示用户"会话损坏"。这种防御性设计确保即使数据库被手动修改或进程被 kill -9,恢复时也能检测并拒绝进入不一致状态。

---

## 6. 流式与中断传播

### 6.1 SSE → 内部事件翻译管线

```typescript
// packages/ai/src/api/anthropic-messages.ts
async function* iterateSseMessages(body, signal): AsyncGenerator<ServerSentEvent> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const state = { event: null, data: [], raw: [] };
  let buffer = "";
  while (true) {
    if (signal?.aborted) throw new Error("Request was aborted");
    const { value, done } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let consumed = consumeLine(buffer);
    while (consumed) { buffer = consumed.rest; const event = decodeSseLine(consumed.line, state); if (event) yield event; consumed = consumeLine(buffer); }
  }
}
```

### 6.2 统一内部事件

```typescript
// packages/ai/src/types.ts
AssistantMessageEvent = start | text_start | text_delta | text_end
    | thinking_start | thinking_delta | thinking_end
    | toolcall_start | toolcall_delta | toolcall_end
    | done | error

// packages/agent/src/types.ts
AgentEvent = agent_start | agent_end | turn_start | turn_end
    | message_start | message_update | message_end
    | tool_execution_start | tool_execution_update | tool_execution_end
```

`AssistantMessageEventStream` 是 push 协议的 async iterable,11 种细粒度事件让 TUI 可以做增量渲染。`result()` 返回最终 AssistantMessage,允许消费者"先订阅事件、后取最终结果"。

### 6.3 增量 JSON 解析

```typescript
// packages/ai/src/utils/json-parse.ts
export function parseStreamingJson<T>(partialJson): T {
  if (!partialJson || partialJson.trim() === "") return {} as T;
  try { return parseJsonWithRepair<T>(partialJson); }
  catch { try { return partialParse(partialJson) ?? {}; }
  catch { try { return partialParse(repairJson(partialJson)) ?? {}; }
  catch { return {} as T; }}}
}
```

`input_json_delta` 累积 `block.partialJson += event.delta.partial_json; block.arguments = parseStreamingJson(block.partialJson);` 然后 emit `toolcall_delta`。

### 6.4 统一取消原语

```typescript
// packages/ai/src/utils/abort.ts
export function raceWithAbortSignal<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
    if (signal.aborted) {
        void operation.catch(() => {});  // 消费未处理 rejection
        return Promise.reject(abortReason(signal));
    }
    return new Promise<T>((resolve, reject) => {
        let settled = false;
        const onAbort = () => { if (!settled) { settled = true; cleanup(); reject(abortReason(signal)); } };
        signal.addEventListener("abort", onAbort, { once: true });
        void operation.then(
            (value) => { if (!settled) { settled = true; cleanup(); resolve(value); } },
            (error) => { if (!settled) { settled = true; cleanup(); reject(error); } },
        );
    });
}
```

每个 LLM 调用、工具执行、OAuth refresh 都通过此函数包裹,确保:
1. **未捕获拒绝**: `void operation.catch(() => {})` 避免 unhandledRejection
2. **settled 标志**: 成功后即使后续 abort 触发,不会 double-resolve/reject
3. **reason 透传**: AbortSignal 的 reason 被原样传递

`combineAbortSignals` 提供多信号合并:`AbortSignal.any([callerSignal, controller.signal])`。

### 6.5 背压 / 缓冲策略

```typescript
// packages/coding-agent/src/core/output-guard.ts
writeRawStdout, waitForRawStdoutBackpressure, flushRawStdout — promise-chained stdout 带 ENOBUFS/EAGAIN/EWOULDBLOCK 重试
takeOverStdout() / restoreStdout() — 把 process.stdout.write 重定向到 stderr
```

`EventStream` 用 `queue[]` + `waiting[]` resolver list 实现 pull-based backpressure。

---

## 7. 记忆与 Context

### 7.1 树状 Session 与 11 种 Entry

```typescript
// packages/agent/src/harness/session/types.ts
export interface EntryBase {
    type: string;
    id: string;
    seq: number;        // 共享序号,storage-assigned
    parentId: string | null;  // storage-assigned
    timestamp: number;
}

Entry = MessageEntry | ModelChangeEntry | ThinkingLevelEntry | ActiveToolsEntry
      | CompactionEntry | BranchSummaryEntry | CustomEntry | LabelEntry
      | NoteEntry | CustomMessageEntry | SessionInfoEntry
```

- `seq` 跨 lane 共享,是 session 全局的单调递增序号
- `parentId` 指向"append 时该 lane 的 leaf",同一条 entry 可能从多个 lane 看到(分支)
- `CompactionEntry` 的 `retainedTail` 设计——完整保留的最近消息,压缩后的上下文 = summary + retainedTail

### 7.2 压缩策略

```typescript
// packages/agent/src/harness/compaction/compaction.ts
export function shouldCompact(contextTokens: number, contextWindow: number, settings: CompactionSettings): boolean {
    if (!settings.enabled) return false;
    return contextTokens > contextWindow - settings.reserveTokens;  // 默认 reserve 16384
}

export const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = {
    enabled: true,
    reserveTokens: 16384,    // 给摘要 prompt 预留的 token
    keepRecentTokens: 20000,  // 压缩后保留的最近上下文 token 数
};
```

**结构化摘要**(Goal / Constraints / Progress / Key Decisions / Next Steps / Critical Context)比自由格式更利于恢复。

### 7.3 文件操作追踪

```typescript
// packages/agent/src/harness/compaction/utils.ts
export function extractFileOpsFromMessage(message: AgentMessage, fileOps: FileOperations): void {
    if (message.role !== "assistant") return;
    for (const block of message.content) {
        if (block.type !== "toolCall") continue;
        const path = typeof args.path === "string" ? args.path : undefined;
        if (!path) continue;
        switch (block.name) {
            case "read": fileOps.read.add(path); break;
            case "write": fileOps.written.add(path); break;
            case "edit": fileOps.edited.add(path); break;
        }
    }
}
```

摘要末尾追加 `<read-files>` / `<modified-files>`,让模型知道"哪些文件被读过、哪些被改过"。

### 7.4 Token 估算

```typescript
export function estimateTokens(message: AgentMessage): number {
    // 字符数 / 4 的粗略估算
    // 图片按 4800 字符估算
}
```

优先使用 provider 返回的 `usage`(精确),仅在没有 provider 数据时回退到字符估算。

### 7.5 分支摘要

当用户从 lane A 导航到 lane B 时,lane A 的工作需要被摘要保存:

```typescript
// packages/agent/src/harness/compaction/branch-summarization.ts
export async function collectEntriesForBranchSummary(
    session: Session, oldLeafId: string | null, targetId: string
): Promise<CollectEntriesResult> {
    // 1. 找到 oldLeafId 和 targetId 的最近公共祖先
    // 2. 收集 oldLeafId 到 common ancestor 之间的所有 entry
    // 3. 在 token 预算内选择要摘要的 entry
    // 4. 生成摘要并追加到 session tree 的目标分支上
}
```

### 7.6 项目上下文注入

```xml
<project_context>
Project-specific instructions and guidelines:
<project_instructions path="...">content</project_instructions>
</project_context>
```

候选链:`AGENTS.override.md` → `AGENTS.md` → `AGENTS.MD` → `CLAUDE.md` → `CLAUDE.MD`。全局 `~/.pi/agent/`,祖先链从 cwd 向上遍历到根目录。

---

## 8. Skill 系统

Pi 明确拒绝 MCP,把 Skill 作为**纯文本指令注入**机制。Skill 文件 = Markdown + YAML frontmatter,运行时只把 name/description/location 注入系统提示词,模型按需用 `read` 工具读取完整内容。

### 8.1 Skill 类型

```typescript
// packages/agent/src/harness/types.ts
export interface Skill {
    name: string;
    description: string;
    content: string;       // Markdown body(完整指令)
    filePath: string;      // 绝对路径
    disableModelInvocation?: boolean; // 从系统提示词中隐藏
}
```

Skill 不是 tool,没有 `parameters` / `execute` 字段。它是**纯文本指令**,注入系统提示词供模型自行决定是否读取和遵循。

### 8.2 文件格式约定

- **`SKILL.md`**: 每个目录显式声明一个 skill(优先级高,递归时遇到即停止继续扫描子目录)
- **`*.md`**: 根目录下带 frontmatter 的普通 Markdown 文件(需有 `description` 字段才会被识别)

### 8.3 名称强校验

```typescript
// packages/agent/src/harness/skills.ts
function validateName(name: string, parentDirName: string): string[] {
    if (name !== parentDirName)
        errors.push(`name "${name}" does not match parent directory "${parentDirName}"`);
    if (name.length > MAX_NAME_LENGTH)  // 64
        errors.push(`name exceeds ${MAX_NAME_LENGTH} characters`);
    if (!/^[a-z0-9-]+$/.test(name))
        errors.push("name contains invalid characters");
    if (name.startsWith("-") || name.endsWith("-"))
        errors.push("name must not start or end with a hyphen");
    if (name.includes("--"))
        errors.push("name must not contain consecutive hyphens");
}
```

名称必须与父目录名一致(强约束),符合 [agentskills.io](https://agentskills.io/) 规范。

### 8.4 Scope 三档

- **user**:`~/.pi/agent/skills/` 全局可见
- **project**:`<cwd>/.pi/skills/` 仅当前项目可见
- **path**:命令行显式 `--skill-path` 指定的路径,优先级最高

collision 检测不静默覆盖,产生 diagnostic 记录 winnerPath/loserPath。

### 8.5 延迟加载

```typescript
// packages/coding-agent/src/core/skills.ts
export function formatSkillsForPrompt(skills: Skill[]): string {
    const visibleSkills = skills.filter((s) => !s.disableModelInvocation);
    if (visibleSkills.length === 0) return "";
    const lines = [
        "The following skills provide specialized instructions for specific tasks.",
        "Use the read tool to load a skill's file when the task matches its description.",
        "When a skill file references a relative path, resolve it against the skill directory...",
        "",
        "<available_skills>",
    ];
    for (const skill of visibleSkills) {
        lines.push("  <skill>");
        lines.push(`    <name>${escapeXml(skill.name)}</name>`);
        lines.push(`    <description>${escapeXml(skill.description)}</description>`);
        lines.push(`    <location>${escapeXml(skill.filePath)}</location>`);
        lines.push("  </skill>");
    }
    lines.push("</available_skills>");
    return lines.join("\n");
}
```

只包含 name + description + location(都是 XML 转义后的),**不包含 content**——skill 内容按需 `read` 工具加载,节省 90%+ token。`escapeXml` 防止特殊字符导致 XML 注入。

### 8.6 显式调用

```typescript
// packages/agent/src/harness/skills.ts
export function formatSkillInvocation(skill: Skill, additionalInstructions?: string): string {
    const skillBlock = `<skill name="${skill.name}" location="${skill.filePath}">
References are relative to ${dirnameEnvPath(skill.filePath)}.
${skill.content}
</skill>`;
    return additionalInstructions ? `${skillBlock}\n\n${additionalInstructions}` : skillBlock;
}
```

`/skill:name args` 完整注入为带 `<skill>` 标签的 user 消息,触发一轮新 run。

### 8.7 对比三种范式

| 特性 | MCP Tool | pi Skill | laew Tool |
|------|----------|----------|-----------|
| 定义格式 | JSON Schema | Markdown + YAML | Rust struct + impl Tool |
| 发现方式 | 服务端注册 | 文件系统扫描 | builtin_registry() |
| 调用方式 | JSON-RPC tool call | 模型自行读取遵循 | LLM tool call → execute() |
| 参数传递 | JSON arguments | 无（纯文本） | JSON → serde 反序列化 |
| 运行时执行 | MCP Server 进程 | 无（模型用现有工具） | Rust async fn |

---

## 9. 遥测

### 9.1 接口层

```typescript
// packages/telemetry/src/index.ts
export interface TelemetryContext {
    startSpan<T>(options: SpanOptions, callback: (span: TelemetrySpan) => T | Promise<T>): Promise<T>;
}
export interface TelemetrySpan extends TelemetryContext {
    addEvent(name: string, attributes?: SpanAttributes): void;
    setAttributes(attributes: SpanAttributes): void;
    setStatus(status: SpanStatus): void;
}
```

回调式 API: `startSpan` 接受一个 callback,callback 内的操作自动被 span 包裹。callback 结束后 span 自动 settle(无论成功还是失败)。这比 OpenTelemetry 的 `span.end()` 手动模式更安全。

### 9.2 NOOP 实现:零开销默认

```typescript
// packages/telemetry/src/noop.ts
const noopTelemetrySpan: TelemetrySpan = {
    startSpan: startNoopSpan,
    addEvent: () => {}, setAttributes: () => {}, setStatus: () => {},
};
Object.freeze(noopTelemetrySpan);
export const NOOP_TELEMETRY_CONTEXT: TelemetryContext = noopTelemetrySpan;
```

当应用不配置 telemetry 时使用,零开销。

### 9.3 InMemory 实现

```typescript
// packages/telemetry/src/memory.ts
function settleSpan(state, span, failed, error?) {
    if (span.settled) return;
    if (failed && !span.explicitStatus) span.status = automaticErrorStatus(error);
    span.settled = true;
    span.endSequence = state.nextEndSequence++;
}
```

- `explicitStatus` 标志: 若 callback 内已手动 `setStatus`,则错误时不覆盖
- `endSequence`: 同一 parent 下,先结束的 child 有更小的 sequence
- **passive recording**: 所有调用都 try-catch,即使传入不可读的 Proxy 对象也不影响 callback

### 9.4 类型化 Schema 推导

```typescript
// packages/telemetry/src/index.ts
export interface TelemetrySpanDefinition {
    description: string;
    parents: TelemetryParentDefinition;
    startAttributes: Record<string, TelemetryStartAttributeDefinition>;
    endAttributes: Record<string, TelemetryAttributeDefinition>;
    events?: Record<string, TelemetryEventDefinition>;
    status: { default: "ok"; errorWhen: string };
}

export interface TelemetryAttributeMetadata {
    description: string;
    sensitive?: boolean;      // ← 隐私脱敏标记
    cardinality?: "low" | "high";
}
```

类型推导工具链:
- `TelemetrySchemaSpanStartAttributes<Schema, Name>` — 推导某 span 的 start 属性
- `TelemetrySchemaSpanEndAttributes<Schema, Name>` — 推导 end 属性
- `createTypedSpanStarter(telemetryContext, schemas)` — 绑定到具体 Context

`sensitive?: boolean` 是隐私脱敏的关键字段——标记为 sensitive 的属性在导出/日志时可由后端遮蔽。

### 9.5 AI / Harness Telemetry Schema

```typescript
// packages/agent/src/harness/telemetry.ts
AI_TELEMETRY_SCHEMA — `pi.ai.request` span: provider, model, api, streaming, deferred, usage, stream.chunk_count, stream.time_to_first_chunk_ms, error.type
HARNESS_TELEMETRY_SCHEMA — spans: pi.harness.run, compaction, navigation, checkpoint, turn, step, tool, hook, sleep, event_handler, pi.session.write
```

Parent links: `pi.harness.turn` → `pi.harness.run`; `pi.harness.step` → `turn|checkpoint|compaction|navigation`; `pi.harness.tool` → `turn|run`。

---

## 10. 会话持久化

### 10.1 双后端架构

| 维度 | JSONL 后端 | SQLite 后端 |
|------|-----------|------------|
| 存储格式 | 一行一 JSON, append-only | WAL 模式, 结构化表 + 索引 |
| 事务性 | 文件级 rename 原子 | `BEGIN IMMEDIATE` + fence 写入者租约 |
| 并发读写 | 单写串行 | 多读者单写者(WriterLease fence) |
| 查询能力 | 全文扫描 + 内存 filter | SQL WHERE + 索引 + FTS5 trigram |
| 搜索 | 无原生搜索 | `session_search_fts` FTS5 虚拟表 |
| Fork 支持 | 文件复制 | 单事务内批量 INSERT |

### 10.2 SQLite Schema: 12 张表

```sql
-- packages/session-backends/sqlite-node/src/sqlite/migrations/001_initial.sql
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

`WITHOUT ROWID` 用于主键即查询维度的表,减少一层间接引用。`branch_entries` 是派生缓存,通过 `rebuildBranchCache()` 按需重建。

### 10.3 WriterLease fence

```typescript
// packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts
export function acquireWriterLease(db, sessionId, ownerId, now, expiresAtMs) {
    const row = sql`INSERT INTO writer_leases (session_id, owner_id, fence, expires_at_ms)
        VALUES (${sessionId}, ${ownerId}, 1, ${expiresAtMs})
        ON CONFLICT(session_id) DO UPDATE SET
            owner_id = excluded.owner_id,
            fence = writer_leases.fence + 1,
            expires_at_ms = excluded.expires_at_ms
        WHERE writer_leases.expires_at_ms <= ${now}
        RETURNING owner_id, fence, expires_at_ms`.get(db);
    return row === undefined ? undefined : { ... };
}
```

- **fence 机制**: 每次抢占写入权时 `fence + 1`。续租时必须匹配 `owner_id + fence`,这样旧 owner 的过期续租不会误更新新 owner 的租约。
- **默认 TTL 30s, 心跳 10s**: 每 10s 续一次,若 30s 未续则其他进程可抢占。
- **事务内续租**: `enqueueWrite()` 在每个写操作开头先续租,若续租失败立即抛 `lostWriterError`。
- **`SerialOperationQueue`**: 类似 Promise 链式队列,确保所有写操作严格串行。

### 10.4 JSONL 撕裂尾部修复

```typescript
// packages/agent/src/harness/session/jsonl/storage.ts
for (let index = 1; index < physicalLines.length; index++) {
    const line = physicalLines[index]!;
    const mutationResult = parseMutation(line);
    if (!mutationResult.ok) {
        const isTornTail = index === physicalLines.length - 1 && mutationResult.error.kind === "syntax";
        if (isTornTail) {
            // 丢弃未确认的部分追加,通过原子发布有效前缀
            const validPrefix = `${physicalLines.slice(0, index).join("\n")}\n`;
            await publishFileAtomically(fs, path, async (tempPath) => {
                await fs.writeFile(tempPath, validPrefix);
            });
            return storage;
        }
        throw invalidFile(path, index + 1, mutationResult.error);
    }
}
```

**原子发布**: 写临时文件 → rename 覆盖。**撕裂尾部修复**: JSONL 文件可能因进程被 kill -9 而在最后一行被截断,加载时检测"最后一行是 syntax error"→ 截断到倒数第二行有效边界 → 原子发布修复后的版本。

### 10.5 串行写入队列

```typescript
// packages/agent/src/harness/session/jsonl/storage.ts
private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.tail.then(operation);
    this.tail = result.then(
        () => undefined,
        () => undefined,  // 即使失败也消费 promise
    );
    return result;
}
```

所有写操作都通过 `enqueue` 排队,保证**同一文件的写操作严格串行**——不会出现两条 mutation 交叉写入。

### 10.6 FTS5 全文搜索

```typescript
// packages/session-backends/sqlite-node/src/sqlite/search-backend.ts
db.transaction(() => {
    sql`CREATE VIRTUAL TABLE IF NOT EXISTS session_search_fts USING fts5(
        payload, content = 'entries', content_rowid = 'rowid',
        tokenize = 'trigram remove_diacritics 1'
    )`.exec(db);
});
```

用 FTS5 `MATCH` + `bm25()` 排序,支持 `entryTypes` 过滤和 `limit` 截断。

---

## 11. 测试与 Eval

### 11.1 架构

```
pi-harness.ts          — 创建 Pi Agent 评估 harness
  └── vitest-evals/    — 框架扩展
      ├── artifacts.ts    — session.jsonl + source 持久化
      ├── harness-table.ts — baseline/candidate 配对 + 重复实验
      ├── reporter.ts     — Vitest Reporter: runs.jsonl + comparison
      └── summary.ts      — 统计比较: correctness lift / token delta / cost delta
```

### 11.2 PiCodingAgentHarness

```typescript
// packages/evals/src/pi-harness.ts
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
    // 清理: rm(root, { recursive: true })
}
```

关键设计:
- **临时目录隔离**: 每次 eval 创建 `mkdtemp`,结束后 `rm -rf`
- **InMemorySettingsManager**: 不读磁盘配置,保证 eval 可重现
- **AbortSignal 支持**: 外部可随时中断 eval
- **session.jsonl artifact**: 评估完成后把完整的 JSONL session 持久化

### 11.3 createJudge

```typescript
// packages/evals/src/extensions.eval.ts
const ExtensionAuthoringJudge = createJudge<PiCodingAgentInput, ExtensionAuthoringOutput>(
    "ExtensionAuthoringJudge",
    ({ output, toolCalls }) => {
        const failures: string[] = [];
        if (!output.extensionSource.includes("@earendil-works/pi-coding-agent"))
            failures.push("extension does not import the canonical package");
        if (!toolCalls.some(call => call.name === "hello" && call.result === "Hello, Bob!"))
            failures.push("no successful hello call");
        return { score: failures.length === 0 ? 1 : 0, metadata: { rationale: ... } };
    }
);
```

Judge 返回 `{score, metadata}`,score ∈ [0,1]。`judgeThreshold: null` 表示仅用于统计。

### 11.4 baseline-candidate 配对统计

```typescript
// packages/evals/src/vitest-evals/summary.ts
function summarizeCorrectness(pairs, totalPairs) {
    for (const { baseline, candidate } of pairs) {
        if (baseline.outcome !== "scored" || candidate.outcome !== "scored") continue;
        const baselinePassed = baseline.score >= 1;
        const candidatePassed = candidate.score >= 1;
        if (baselinePassed === candidatePassed) ties += 1;
        else if (baselinePassed) baselineWins += 1;
        else candidateWins += 1;
    }
    return { totalPairs, eligiblePairs, baselinePassRate, candidatePassRate, lift, baselineWins, candidateWins, ties };
}
```

**lift 计算**: `candidatePassRate - baselinePassRate`,单位是"百分点"。`summarizeMetric` 对 token/latency/cost 三个维度做配对均值差计算,用 `preciseDifference` 避免浮点精度问题。

### 11.5 Artifact 持久化

两种 artifact 类型:
- `@earendil-works/pi-evals:session` — 完整的 session.jsonl,用于回放 agent 推理过程
- `@earendil-works/pi-evals:source` — eval 生成的源文件,用于验证代码质量

---

## 12. 配置系统

### 12.1 Provider 20+ 兼容性开关

```typescript
// packages/ai/src/types.ts
export interface OpenAICompletionsCompat {
    supportsStore?: boolean;                   // OpenAI 特有
    supportsDeveloperRole?: boolean;           // developer vs system
    supportsReasoningEffort?: boolean;         // reasoning_effort 参数
    supportsUsageInStreaming?: boolean;        // stream_options.include_usage
    supportsFinishReason?: boolean;            // 流式 finish_reason
    maxTokensField?: "max_completion_tokens" | "max_tokens";
    requiresToolResultName?: boolean;          // tool result 需要 name
    requiresAssistantAfterToolResult?: boolean;
    requiresThinkingAsText?: boolean;          // thinking 转 <thinking> 标签
    thinkingFormat?:                          // 11 种格式之一
        | "openai" | "openrouter" | "deepseek" | "together" | "baseten"
        | "zai" | "qwen" | "chat-template" | "qwen-chat-template"
        | "string-thinking" | "ant-ling";
    supportsThinkingTokenBudget?: boolean;
    supportsOpenAIGrammarTools?: boolean;      // Lark/regex 语法约束
    supportsStrictMode?: boolean;
    cacheControlFormat?: "anthropic";
    sendSessionAffinityHeaders?: boolean;
    // ... 更多
}
```

这套 20+ 开关矩阵让同一份代码兼容 vLLM、SGLang、llama.cpp、Ollama、OpenRouter、Together、Baseten、DeepSeek、Qwen、Z.ai、Ant Ling 等十几种"自称 OpenAI 兼容"的服务器。每个开关都有"auto-detect from URL"默认行为,先用 URL 启发式猜测,再被 model.compat 显式配置覆盖。

### 12.2 动态模型刷新 + ETag

```typescript
// packages/ai/src/models.ts
async refresh(options: ModelsRefreshOptions = {}): Promise<ModelsRefreshResult> {
    const refresh = Promise.all(
        refreshable.map(async (provider) => {
            // 阶段 1:用 stored credential 恢复缓存(allowNetwork=false)
            await this.runProviderRefreshPhase(provider, storedCredential, false, ...);
            if (!allowNetwork || signal.aborted) return;
            // 阶段 2:解析凭证 + 网络刷新
            const credential = await this.resolveRefreshCredential(provider, storedCredential, signal);
            if (!credential) return;
            await this.runProviderRefreshPhase(provider, credential, true, options.force, generation, signal);
        })
    );
}
```

**两阶段刷新**——先离线恢复缓存(快速),再在线刷新(可能慢)。确保即使网络不可用,应用也能用上次缓存的模型列表启动。

### 12.3 Generation 检查

```typescript
private supersedeProviderRefresh(providerId: string): number {
    const generation = (this.refreshGenerations.get(providerId) ?? 0) + 1;
    this.refreshGenerations.set(providerId, generation);
    const previous = this.refreshControllers.get(providerId);
    if (previous) {
        this.refreshControllers.delete(providerId);
        previous.abort();  // 取消之前的刷新
    }
    return generation;
}
```

每次 `setProvider` / `deleteProvider` 都会递增 generation 并 abort 旧刷新。`publishProviderModels` 在写入前检查 generation 是否仍匹配,**防止旧刷新覆盖新数据**。

---

## 13. 系统提示词与 thinkingFormat

### 13.1 buildSystemPrompt

`core/system-prompt.ts` 的 `buildSystemPrompt` 负责:

1. **customPrompt 路径**: 追加 `<project_context>` 块(含多个 `<project_instructions path="...">`)+ skills 格式化(`formatSkillsForPrompt`)+ cwd
2. **默认路径**: 从 `config.ts` 的 `getReadmePath()` / `getDocsPath()` / `getExamplesPath()` 读取文档,构建工具列表(`read/bash/edit/write` 可见性由 `toolSnippets` 决定)+ guidelines 列表

### 13.2 promptSnippet + promptGuidelines 联动

```typescript
// packages/coding-agent/src/server/create-harness.ts
export function buildCodingAgentHarnessSystemPrompt(options): string {
    const activeTools = options.activeToolNames.flatMap((name) => {
        const tool = options.tools.find((candidate) => candidate.name === name);
        return tool ? [tool] : [];
    });
    const toolSnippets = Object.fromEntries(
        activeTools.flatMap((tool) => {
            const promptSnippet = tool.promptSnippet
                ?.replace(/[\r\n]+/g, " ")
                .replace(/\s+/g, " ")
                .trim();
            return promptSnippet ? [[tool.name, promptSnippet]] : [];
        }),
    );
    const promptGuidelines = activeTools.flatMap((tool) => tool.promptGuidelines ?? []);
    return buildSystemPrompt({ ...options.systemPromptOptions, selectedTools, toolSnippets, promptGuidelines });
}
```

每个工具可携带 `promptSnippet`(一句话描述)和 `promptGuidelines`(使用指南数组),**新工具加入 → 系统提示词自动更新**。

### 13.3 thinkingFormat 11 种

```typescript
// packages/ai/src/types.ts
thinkingFormat: "openai" | "openrouter" | "deepseek" | "together" | "baseten"
    | "zai" | "qwen" | "chat-template" | "qwen-chat-template"
    | "string-thinking" | "ant-ling"
```

每个 Provider 的"thinking"参数位置和格式都不一样:
- OpenAI → `reasoning_effort: "high"`
- DeepSeek → `{ thinking: { type: "enabled" }, reasoning_effort: "high" }`
- Qwen → `enable_thinking: true`(顶层)
- Qwen-Chat-Template → `chat_template_kwargs: { enable_thinking: true, preserve_thinking: true }`
- Ant Ling → `{ reasoning: { effort: "high" } }`
- Z.ai → `{ thinking: { type: "enabled" } }`
- Together → `{ reasoning: { enabled: true } }`

模型层只需表达"thinking: high",不需要知道目标 Provider 用什么参数名。

---

## 14. 错误处理与容错

### 14.1 截断消息保护

```typescript
// packages/agent/src/agent-loop.ts
const executedToolBatch = message.stopReason === "length"
    ? await failToolCallsFromTruncatedMessage(toolCalls, emit)
    : await executeToolCalls(...);
```

当 `stopReason === "length"`(token 限制截断)时,所有 tool call 的参数都可能不完整,**所有 tool call 直接失败**,不执行——因为截断的 JSON 参数可能静默不完整。

### 14.2 TaggedError 模式

```typescript
// packages/agent/src/harness/agent-harness.ts
export class LaneBusy extends TaggedError("LaneBusy")<{
    lane: string;
    operationId: string;
    operationKind: "run" | "compaction" | "navigation";
    message: string;
}> {}
```

`TaggedError` 是一个高阶函数,返回一个继承自 `Error` 的类,带有 `tag` 字段(字符串字面量)用于 runtime 类型判断。

### 14.3 Result 类型

```typescript
export type Result<TValue, TError> =
    | { ok: true; value: TValue }
    | { ok: false; error: TError };
```

配合 `ok()` / `err()` / `getOrThrow()` / `getOrUndefined()` 工具函数,形成函数式错误处理风格。`FileSystem` 和 `Shell` 接口的所有方法都返回 `Result`,永不抛出异常。

### 14.4 BashTool 实时输出 + 截断

```typescript
// packages/agent/src/harness/tools/bash.ts
const BASH_UPDATE_THROTTLE_MS = 100;

const scheduleOutputUpdate = (): void => {
    updateDirty = true;
    const delay = BASH_UPDATE_THROTTLE_MS - (Date.now() - lastUpdateAt);
    if (delay <= 0) {
        clearUpdateTimer();
        emitOutputUpdate();
        return;
    }
    updateTimer ??= setTimeout(() => { emitOutputUpdate(); }, delay);
};
```

100ms 节流的 `onUpdate` 回调实现实时输出流。输出截断策略:默认最大 500 行或 32KB(先达到的为准),截断后保存完整输出到临时文件,返回末尾附加 `[Showing lines X-Y of Z. Full output: /tmp/xxx]`。

### 14.5 文件写入串行化

```typescript
// packages/agent/src/harness/tools/file-mutation-queue.ts
export async function withFileMutationQueue<T>(env: ExecutionEnv, path: string, fn: () => Promise<T>) {
    const state = getState(env);
    const key = await getMutationQueueKey(env, path);  // canonicalPath 作为 key
    const currentQueue = state.queues.get(key) ?? Promise.resolve();
    let releaseNext = () => {};
    const nextQueue = new Promise<void>((resolve) => { releaseNext = resolve; });
    const chainedQueue = currentQueue.then(() => nextQueue);
    state.queues.set(key, chainedQueue);
    await currentQueue;  // 等待前一个操作完成
    try {
        return await fn();  // 执行当前操作
    } finally {
        releaseNext();  // 释放下一个操作
    }
}
```

经典的 **per-key Promise 链式锁** 模式,确保 read-modify-write 原子性。

---

## 15. 对 laew 的借鉴

### 15.1 P0 — 直接可用(1-2 周)

| 借鉴项 | pi 实现 | laew 落地建议 |
|--------|---------|--------------|
| **截断 tool call 保护** | `failToolCallsFromTruncatedMessage`(`agent-loop.ts:379`) | 在 `agent/mod.rs` 的 `complete` 路径增加 `stopReason === "length"` 时全部 tool call 失败逻辑 |
| **NOOP Telemetry 接口** | `telemetry/src/noop.ts` | 在 `agent/mod.rs` 引入 `TelemetryContext` trait + NOOP 实现 |
| **事件溯源 reducer** | `reduceLaneState` 纯函数 + `validateRecordLog` | 为 session 历史增加 append-only record log + 启动时校验 |
| **类型化遥测 schema** | `TelemetrySchemaDefinition` + `sensitive` 标记 | 为 Bash/Read/Write 工具调用增加 span + 敏感信息脱敏 |
| **Skill 文件格式 SKILL.md** | `packages/coding-agent/src/core/skills.ts` | 新建 `src/agent/skills/` 模块,`Skill { name, description, content, file_path, source, disable_model_invocation }` |
| **延迟 Skill 注入** | `formatSkillsForPrompt` 只注入 metadata | 系统提示词只含 name/description/location,不注入 content |

### 15.2 P1 — 中期目标(3-4 周)

| 借鉴项 | pi 实现 | laew 落地建议 |
|--------|---------|--------------|
| **WriterLease fence** | `acquire/renew/release` + fence 单调递增 | 为 laew 的 SQLite 多进程访问增加租约保护 |
| **baseline-candidate eval** | `evalHarnessTable` + `summarizeHarnessComparisons` | 为 laew 建立 eval 框架,对比不同 system prompt / 模型效果 |
| **beforeToolCall 权限钩子** | `BeforeToolCallResult {block, reason, terminate}` | 在 Bash 工具增加命令黑名单 + 用户确认 |
| **Compaction 管线** | 三阶段 + file op tracking | 为 laew 增加上下文溢出时的自动摘要压缩 |
| **Provider 20+ compat 开关** | `OpenAICompletionsCompat` | 引入 `ProviderCompat` 扩展到 Ollama / DeepSeek / Qwen |
| **碰撞检测 + diagnostic** | collision: { winnerPath, loserPath } | 后加载的不覆盖前者,产生 diagnostic |
| **`.gitignore` 兼容** | `ignore` crate | 目录含 SKILL.md 不再递归;递归扫根目录 *.md |

### 15.3 P2 — 远期目标(5-8 周)

| 借鉴项 | pi 实现 | laew 落地建议 |
|--------|---------|--------------|
| **CBOR 帧协议** | `FrameDecoder` + `encodeFrame` | 若做 server 模式(远程 TUI),可参考帧协议设计 |
| **类型化 span 推导** | `createTypedSpanStarter` 编译期推导 | Rust 可用 enum + match 替代,但 schema 版本管理思路可借鉴 |
| **多 lane 分支** | `SessionTree` + `createLane` + `navigateTree` | 考虑对话分支/回溯能力 |
| **PromptTemplate** | `$@` 参数替换 | `-f` 文件提示词支持参数化 |
| **扩展热加载** | `ExtensionRunner` + `.pi/extensions/*.ts` | 插件生态可参考 |
| **C/S 架构(PiServer)** | `server/src/server.ts` | 供 VS Code 插件/远程终端连接 laew 后端 |

### 15.4 Skill 对 laew 的落地路线图

#### P0(必修,1 周内可落地)

1. **引入 Skill 文件格式 `SKILL.md`**
   - 位置:新建 `src/agent/skills/`
   - 三档发现路径:`~/.pi/agent/skills/` → `<cwd>/.pi/skills/` → `--skill-path`
   - 强名称约束:name 必须 = parentDirName

2. **延迟注入**
   - 位置:`src/agent/system_prompt/mod.rs` 新增 `format_skills_for_prompt`
   - 格式:agentskills.io 的 XML 块,用 `escape_xml` 防注入

3. **`/skill:<name> <args>` 显式触发 + 模型自动发现双模式**

#### P1(重要,2-4 周可落地)

4. **collision 检测 + diagnostic**
5. **`.gitignore` 兼容 + 跳过 `node_modules` / 隐藏目录**
6. **promptSnippet + promptGuidelines 联动**
7. **Edit/Bash 工具结果截断**

#### P2(增强,1-2 月可探索)

8. **PromptTemplate 与 Skill 平行**
9. **Skill 数量统计注入 system prompt**
10. **Skill 版本 + scope 优先级矩阵**
11. **Skill Marketplace / Index(可选)**
12. **每个 Agent 维护独立 Skill 集合**

### 15.5 Pi 的三个核心设计决策

1. **Skill 是文本注入,不是工具调用**: 用最少的抽象(Markdown)覆盖知识管理场景,避免 MCP 的协议复杂度
2. **Lane 是 session tree 的命名分支**: 不是独立的并行执行线程,而是共享同一个 session tree 的不同视角
3. **Harness 是状态容器,不是编排函数**: 面向对象的 Harness 持有 tools/models/resources/session 等全部状态,支持运行时动态修改

这三个决策共同支撑了 pi 的核心特征:**极简 Skill + 并发 Lane + 可组合 Harness**。

### 15.6 Pi 的反模式与局限性

1. **AgentHarness 基类是空壳**: 几乎所有方法都调用 `this.unavailable()`,抛出 `HarnessNotImplemented`。真正的实现在更上层
2. **Hooks 是占位符**: `AgentHarness.hooks` 被赋值为 `UnavailableRegistry`
3. **Tool result 截断硬编码**: 2000 字符限制,对于需要精确上下文的场景可能导致信息丢失
4. **Token 估算粗糙**: `字符数 / 4`,对中文/日文等多字节语言不准确
5. **OAuth 身份伪装有风险**: 伪装为 Claude Code CLI 可能违反 Anthropic 服务条款

---

## 附录 A: 核心源码文件索引

| 文件 | 行数 | 内容 |
|------|------|------|
| `packages/coding-agent/src/core/agent-session.ts` | ~350 | AgentSession 生命周期 |
| `packages/coding-agent/src/core/system-prompt.ts` | ~200 | buildSystemPrompt + 项目上下文注入 |
| `packages/coding-agent/src/core/tools/index.ts` | ~150 | 8 大工具 + ToolName 类型 |
| `packages/coding-agent/src/core/compaction/compaction.ts` | ~300 | Compaction 三阶段 |
| `packages/agent/src/agent-loop.ts` | 804 | 双层 while 循环、tool call 执行 |
| `packages/agent/src/agent.ts` | 593 | 有状态 Agent、PendingMessageQueue |
| `packages/agent/src/types.ts` | 445 | AgentEvent/AgentTool/ThinkingLevel |
| `packages/agent/src/harness/reducer.ts` | 667 | reduceLaneState + 14 种损坏检测 |
| `packages/agent/src/harness/agent-harness.ts` | 508 | AgentHarness + Lane 接口 |
| `packages/agent/src/harness/session/types.ts` | 393 | Entry/LaneRecord 类型 |
| `packages/agent/src/harness/session/jsonl/storage.ts` | 277 | JSONL 撕裂尾部修复 |
| `packages/coding-agent/src/core/skills.ts` | 507 | Skill 加载与 slash 命令集成 |
| `packages/agent/src/harness/skills.ts` | 386 | Harness 侧 Skill 加载 |
| `packages/protocol/src/framing.ts` | 165 | FrameDecoder + encodeFrame |
| `packages/protocol/src/codec.ts` | 173 | 验证 + CBOR 编解码 |
| `packages/protocol/src/schemas.ts` | 451 | 完整消息 schema |
| `packages/telemetry/src/index.ts` | 358 | TelemetrySchemaDefinition |
| `packages/telemetry/src/memory.ts` | 219 | InMemoryTelemetryContext |
| `packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts` | 59 | WriterLease fence |
| `packages/ai/src/types.ts` | ~600 | 11 Api + 30+ Provider + AssistantMessageEvent |
| `packages/ai/src/api/anthropic-messages.ts` | ~1300 | Anthropic 完整适配 |
| `packages/ai/src/api/openai-completions.ts` | ~1700 | OpenAI 完整适配 |
| `packages/ai/src/utils/event-stream.ts` | 89 | EventStream + AssistantMessageEventStream |
| `packages/client/src/client.ts` | ~400 | PiClient + session lease |
| `packages/server/src/server.ts` | ~300 | PiServer 类 |
| `packages/server/src/connection.ts` | ~200 | ConnectionState 五阶段状态机 |
| `packages/server/src/sessions.ts` | ~200 | LiveSessionManager |
| `packages/server/src/protocol.ts` | ~400 | AI ↔ Protocol 消息转换 |
| `packages/evals/src/pi-harness.ts` | 258 | createPiCodingAgentHarness |
| `packages/evals/src/vitest-evals/summary.ts` | 439 | summarizeHarnessComparisons |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | ~300 | BoundedTerminalWriter |
| `packages/agent/test/harness/reducer.test.ts` | 1127 | 14 种损坏检测测试用例 |

---

## 附录 B: Pi vs Laew 架构对照速查

| 维度 | pi | laew |
|------|----|------|
| Skill/指令管理 | Markdown + frontmatter → 系统提示词注入 | SystemPrompt 静态拼接 + project_context 五级链 |
| 并发模型 | Lane(session tree 分支) | SubAgent(一次性执行单元) |
| 运行时容器 | AgentHarness(OOP 状态容器) | MultiAgentOrchestrator(编排函数) |
| 上下文压缩 | shouldCompact + findCutPoint + LLM 摘要 | 无(SessionContext 自由格式摘要) |
| 工具钩子 | beforeToolCall / afterToolCall | 无(直接执行) |
| 协议适配 | 10+ API,20+ 兼容性开关 | Anthropic + OpenAI 双协议 |
| 文件操作保护 | withFileMutationQueue(per-file 排他锁) | 无 |
| 崩溃恢复 | Record Log + reduceLaneState 事件溯源 | 无 |
| 流式输出 | AssistantMessageEventStream(细粒度事件) | 统一消息模型后进入 agent 循环 |
| 认证管理 | API Key + OAuth + AWS SigV4 | API Key only |
| 错误处理 | Result 类型(不抛异常) | Result + AgentError enum |
| 参数验证 | TypeBox 运行时验证 + prepareArguments | serde 静态反序列化 |
| 会话持久化 | JSONL + SQLite(session-backends) | SQLite(session_memory 表) |
| TUI 集成 | drive: "manual" 逐步推进 | 阻塞式等待 agent 完成 |
| 测试策略 | vitest + harness conformance test | cargo test + run_e2e.sh tmux |

---

*文档生成时间: 2026-09-06 | 基于 pi monorepo @ main 分支源码 | 综合合并自 7 份原始文档*

---

## 12. 第五轮深挖补充(2026-09-06)

补充前 11 章覆盖薄弱的代码级事实。所有行号来自 `/usr/local/LsmGitOpenSource/pi` 当前 head(2026-09-02)。

### 12.1 coding-agent 双层 while(true) + abort

**外/内双层 while**(`packages/agent/src/agent-loop.ts:170-175`):
```ts
// Outer loop: continues when queued follow-up messages arrive after agent would stop
while (true) {
    let hasMoreToolCalls = true;
    // Inner loop: process tool calls and steering messages
    while (hasMoreToolCalls || pendingMessages.length > 0) {
```
**中止短路**(`agent-loop.ts:215-218`):
```ts
if (message.stopReason === "error" || message.stopReason === "aborted") {
    await emit({ type: "turn_end", message, toolResults: [] });
    await emit({ type: "agent_end", messages: newMessages });
    return;
}
```
**无 maxTurns 上限**,循环纯由 stopReason/queue 退出。

**运行时 abort**(`packages/agent/src/agent.ts:319-320`):
```ts
abort(): void {
    this.activeRun?.abortController.abort();
}
```
**每次 run 新建 AbortController**(`agent.ts:491-505`):
```ts
const abortController = new AbortController();
// ...
await executor(abortController.signal);
// ...
await this.handleRunFailure(error, abortController.signal.aborted);
```

### 12.2 内置工具的截断与 abort 监听

**read 工具**(`packages/coding-agent/src/core/tools/read.ts:19,218,232-241`):
```ts
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, formatSize, type TruncationResult, truncateHead } from "./truncate.ts";
// ...
if (signal?.aborted) { reject(new Error("Operation aborted")); ... }
const aborted = false; ... signal?.addEventListener("abort", onAbort, { once: true });
```
- **once: true**:abort listener 只触发一次,避免泄漏。

**truncate 常量**(`packages/coding-agent/src/core/tools/truncate.ts:11-12,79-80`):
```ts
export const DEFAULT_MAX_LINES = 2000;
export const DEFAULT_MAX_BYTES = 50 * 1024; // 50KB
export function truncateHead(content: string, options: TruncationOptions = {}): TruncationResult {
    const maxLines = options.maxLines ?? DEFAULT_MAX_LINES;
```
- 与 opencode / claudecode 完全一致 —— 50KB / 2000 行已是行业默认。

**bash 工具**(`packages/coding-agent/src/core/tools/bash.ts:350,384`):
```ts
description: `Execute a ${config.shellName} command... Output is truncated to last ${DEFAULT_MAX_LINES} lines or ${DEFAULT_MAX_BYTES / 1024}KB (whichever is hit first)...`
truncation: snapshot.truncation.truncated ? snapshot.truncation : undefined,
```
- **head 截断** 与 claudecode / opencode 一致。

**edit / write 工具**:同目录(`packages/coding-agent/src/core/tools/{edit.ts, write.ts}`)。

### 12.3 压缩:CompactionSettings + RPC

**配置**(`packages/coding-agent/src/core/compaction/compaction.ts:126-136`):
```ts
export interface CompactionSettings {
    enabled: boolean;
    reserveTokens: number;     // 16384
    keepRecentTokens: number;  // 20000
}
export const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = {
    enabled: true, reserveTokens: 16384, keepRecentTokens: 20000,
};
```

**触发**(`compaction.ts:235-238`):
```ts
export function shouldCompact(contextTokens, contextWindow, settings) {
    if (!settings.enabled) return false;
    return contextTokens > contextWindow - settings.reserveTokens;
}
```
- **公式**:`contextTokens > window - reserve` —— 给输出预留 `reserve` token 缓冲。

**Session 入口持久化结构**(`packages/coding-agent/src/core/compaction/index.ts:36-50`):导出 `compact` 函数族。

**RPC 类型**(`packages/coding-agent/src/modes/rpc/rpc-types.ts:47-48`):
```ts
| { id?: string; type: "compact"; customInstructions?: string }
| { id?: string; type: "set_auto_compaction"; enabled: boolean }
```
- 用户可通过 RPC 触发手动压缩(`customInstructions`)或开关自动压缩。

**Abort 控制**(`packages/coding-agent/src/core/agent-session.ts:2092-2094`):
```ts
abortCompaction(): void {
    this._compactionAbortController?.abort();
    this._autoCompactionAbortController?.abort();
}
```
- **两个独立的 abort controller** —— 手动与自动压缩互不干扰。

### 12.4 Provider 适配层 87 provider

**API 适配目录**(`packages/ai/src/api/`):
- `anthropic-messages.ts`、`anthropic-messages.lazy.ts`
- `openai-responses.ts`、`openai-responses.lazy.ts`、`openai-completions.ts`
- `bedrock-converse-stream.lazy.ts`、`google-vertex.lazy.ts`、`cloudflare.ts`

**Providers 目录**(`packages/ai/src/providers/`):87 个 .ts,含 anthropic/openai/bedrock/google-vertex/together/nvidia/opencode/qwen/xiaomi/cerebras/fireworks/zai-coding 等。

**模型目录**:
- `packages/ai/src/model-catalog.ts`
- `packages/ai/src/models.ts`、`models.generated.ts`
- `packages/ai/src/images-models.ts`、`image-models.generated.ts`

**streamSimple 派发**(`models.ts:690-703, 830-831`):
```ts
streamSimple(model: Model<Api>, context, options?): AssistantMessageEventStream {
    // ...
    return provider.streamSimple(requestModel, context, requestOptions);
}
// 动态派发
streamSimple: (model, context, options) =>
    dispatch(model, (streams) => streams.streamSimple(model, context, options)),
```

### 12.5 对 laew 的 P0/P1/P2 借鉴路线

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | once: true abort listener | 信号只触发一次,避免 listener 泄漏 | read.ts:232-241 |
| **P0** | RPC compact 协议 | 支持手动触发 + 自定义 instructions + 远程控制开关 | rpc-types.ts:47-48 |
| **P0** | 压缩 abort 双 controller | 手动/自动各持独立 controller,互不干扰 | agent-session.ts:2092-2094 |
| **P1** | CompactionSettings 三字段 | enabled/reserveTokens/keepRecentTokens —— 比 atomcode 的常量更可配置 | compaction.ts:126-136 |
| **P1** | 触发公式 | `tokens > window - reserve` 比"阈值比例"更直观 | compaction.ts:235-238 |
| **P1** | 87 provider 动态派发 | dispatch(model, streams => streams.streamSimple(...)) —— 单点适配 | models.ts:830-831 |
| **P1** | 双层 while(true) | 外层等 follow-up / 内层处理 tool+steer —— 与 atomcode 同思路 | agent-loop.ts:170-175 |
| **P2** | truncateHead 工具函数 | head 截断独立工具,可复用 | truncate.ts:79-80 |
| **P2** | models.generated 自动生成 | 模型目录由 build script 生成,人工不维护 | models.generated.ts |
| **P2** | formatSize + truncation 报告 | 工具结果自带截断元信息,可向模型报告 | bash.ts:384 |
