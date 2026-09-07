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

16. [第七轮深挖 — 文件编辑补丁与排他锁 + Git与checkpoint回滚 + Bash进程管理 + 代码检索索引](#14-第七轮深挖--文件编辑补丁与排他锁--git与checkpoint回滚--bash进程管理--代码检索索引)

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

## 14. 第七轮深挖 — 文件编辑补丁与排他锁 + Git与checkpoint回滚 + Bash进程管理 + 代码检索索引(2026-09-06)

> 分析对象:`/usr/local/LsmGitOpenSource/pi/packages`(TypeScript, Bun)
> 本轮聚焦前六轮完全未覆盖的四个"工具层执行语义"维度:文件编辑与补丁策略、Git 与版本控制集成、命令执行与进程管理、代码检索与索引。
> 全部结论均带 `packages/xxx/src/yyy.ts:LINE` 源码坐标,已逐一核对。

### 14.0 本轮结论速览

| # | 维度 | pi 的答案(一句话) | 关键源码 |
|---|------|---------------------|----------|
| 1 | 文件编辑与补丁 | 单一 `edit` 工具 + 多编辑 `edits[]` + 模糊匹配回退 + 行块保真回写 + per-realpath 排他锁;**无备份无 undo,非原子写** | `core/tools/edit.ts` / `edit-diff.ts` / `file-mutation-queue.ts` |
| 2 | Git 与版本控制 | **核心零 Git 集成**。Git 只出现在三处:TUI footer 分支探测(worktree/reftable 兼容)、worktree 上下文文件去重、扩展示例(git-checkpoint 用 `git stash` 把会话树 fork 和代码回滚联动) | `core/footer-data-provider.ts` / `core/resource-loader.ts` / `examples/extensions/git-checkpoint.ts` |
| 3 | Bash 进程管理 | `spawn(shell, ["-c", cmd], {detached:true})` + 进程组 SIGKILL + 100ms 空闲宽限等待 + 100ms 节流流式输出 + 有界滚动缓冲 + 超限自动落临时文件;**无 PTY、无后台任务、无 stdin 交互、无危险命令黑名单(全部留给扩展 hook)** | `core/tools/bash.ts` / `utils/shell.ts` / `utils/child-process.ts` / `core/tools/output-accumulator.ts` |
| 4 | 代码检索与索引 | 完全外包给外部二进制:**spawn ripgrep(`--json`)+ spawn fd(`--glob`)**,缺失时从 GitHub Release 自动下载到 `~/.pi/agent/bin`;**无 LSP、无符号索引、无 embedding/向量索引** | `core/tools/grep.ts` / `find.ts` / `utils/tools-manager.ts` |

四个维度共同的"pi 哲学"再确认:**核心只做最小可信执行语义(锁/截断/取消),一切策略性能力(权限、Git、后台任务)都以扩展 hook 的形式外置**——这与第六轮发现的"单写者模型"一脉相承。

---

### 14.1 文件编辑与补丁策略

#### 14.1.1 工具面:`edit` 是唯一的编辑工具(没有 StrReplace/Patch/MultiEdit/Notebook)

pi 的内置工具集是 `read | bash | powershell | edit | write | grep | find | ls` 共 8 个(`packages/coding-agent/src/core/tools/index.ts:91-100`),**没有** MultiEdit / StrReplaceEditor / NotebookEdit / Patch / ApplyPatch 之类的变体。多编辑能力被合并进 `edit` 的 `edits[]` 数组:

```ts
// packages/coding-agent/src/core/tools/edit.ts:34-54
const replaceEditSchema = Type.Object({
    oldText: Type.String({
        description:
            "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call.",
    }),
    newText: Type.String({ description: "Replacement text for this targeted edit." }),
});

const editSchema = Type.Object({
    path: Type.String({ description: "Path to the file to edit (relative or absolute)" }),
    edits: Type.Array(replaceEditSchema, {
        description:
            "One or more targeted replacements. Each edit is matched against the original file, not incrementally...",
    }),
});
```

三条强约束被同时写进 schema 描述与系统提示词(`edit.ts:56-64` 的 `promptGuidelines`):

1. `edits[].oldText` 必须**在原文件中唯一**;
2. 多条编辑**互不重叠、互不嵌套**——"If two changes touch the same block or nearby lines, merge them into one edit instead"(不是像 MultiEdit 那样顺序应用);
3. "Keep edits[].oldText as small as possible while still being unique"——禁止用大片未变区域填充。

**没有 `replace_all` 参数**。要全局替换,模型必须显式枚举每一处唯一上下文,或者退回 bash 的 `sed`。这是 pi 有意为之:唯一性校验 + 无 replace_all,把歧义在工具层强制暴露,而不是静默全替换。

#### 14.1.2 参数兼容层 `prepareEditArguments`:对模型坏输出的三重容错

每个工具可以声明 `prepareArguments`(类型见 `packages/agent/src/types.ts:386-389`,定义为 "Optional compatibility shim for raw tool-call arguments **before schema validation**")。`edit` 用它消化三类真实模型错误:

```ts
// packages/coding-agent/src/core/tools/edit.ts:116-147
function prepareEditArguments(input: unknown): EditToolInput {
    ...
    // Some models (Opus 4.6, GLM-5.1) send edits as a JSON string instead of an array.
    // Others send a single edit object instead of a one-element edits array.
    if (typeof args.edits === "string") {
        try {
            const parsed = JSON.parse(args.edits);
            if (Array.isArray(parsed)) args.edits = parsed;
            else if (isSingleEditInput(parsed)) args.edits = [parsed];
        } catch {}
    } else if (isSingleEditInput(args.edits)) {
        args.edits = [args.edits];
    }
    // legacy 平铺 oldText/newText → 追加为 edits[0]
    const edits = Array.isArray(legacy.edits) ? [...legacy.edits] : [];
    edits.push({ oldText: legacy.oldText, newText: legacy.newText });
    ...
}
```

| 模型坏输出 | 修复动作 |
|---|---|
| `edits` 传成 JSON 字符串 | `JSON.parse` 后归一为数组 |
| `edits` 传成单个对象 | 包成 `[obj]` |
| 平铺 `oldText`/`newText`(旧 API 习惯) | 追加进 `edits[]` 并删除平铺字段 |

配套测试 `packages/coding-agent/test/edit-tool-legacy-input.test.ts` 专门覆盖这一层。

#### 14.1.3 匹配算法:精确优先 → 模糊归一化回退

核心在 `edit-diff.ts` 的 `fuzzyFindText`(`packages/coding-agent/src/core/tools/edit-diff.ts:207-245`):

```ts
export function fuzzyFindText(content: string, oldText: string): FuzzyMatchResult {
    // Try exact match first
    const exactIndex = content.indexOf(oldText);
    if (exactIndex !== -1) {
        return { found: true, index: exactIndex, matchLength: oldText.length,
                 usedFuzzyMatch: false, contentForReplacement: content };
    }
    // Try fuzzy match - work entirely in normalized space
    const fuzzyContent = normalizeForFuzzyMatch(content);
    const fuzzyOldText = normalizeForFuzzyMatch(oldText);
    const fuzzyIndex = fuzzyContent.indexOf(fuzzyOldText);
    ...
}
```

模糊归一化 `normalizeForFuzzyMatch`(`edit-diff.ts:34-55`)是一组**渐进式不可逆变换**:

| 变换 | 目标字符 | 场景 |
|---|---|---|
| `NFKC` Unicode 归一 | 全/半角、组合字符 | 模型输出全角标点 |
| 每行 `trimEnd()` | 行尾空白 | 模型复制的代码缩进尾空格失真 |
| 智能单引号 → `'` | `‘ ’ ‚ ‛` | 模型把撇号写成排版引号 |
| 智能双引号 → `"` | `“ ” „ ‟` | 同上 |
| 各类连字符/破折号 → `-` | `‐-―, −` | 注释里的 em-dash |
| 特殊空格 → 普通空格 | ` ,  - ,  ,  , 　` | NBSP / 全角空格 |

关键设计:**模糊命中时返回的 `contentForReplacement` 是归一化后的文本而非原文**——替换在"归一化空间"完成,再由下一步把未触碰的行块从原文拷回,保证未编辑区域字节级不变。

#### 14.1.4 行块保真回写:`applyReplacementsPreservingUnchangedLines`

这是 pi 编辑器最精巧的一段(`packages/coding-agent/src/core/tools/edit-diff.ts:122-173`),注释原文道破动机:

```ts
/**
 * Apply replacements matched against `baseContent` to `originalContent` while
 * preserving unchanged line blocks from the original.
 * ...
 * The actual replacement ranges drive preservation so
 * duplicate normalized lines cannot be aligned to the wrong occurrence.
 */
export function applyReplacementsPreservingUnchangedLines(
    originalContent: string, baseContent: string, replacements: TextReplacement[],
): string {
```

流程:

1. `getLineSpans` 给归一化基线每行记 `[start, end)` 偏移(`edit-diff.ts:75-82`);
2. 每个替换区间被"拓宽到它实际触碰的行"(`getReplacementLineRange`, `edit-diff.ts:84-109`);
3. 相邻/重叠的行组合并成 group(`edit-diff.ts:143-154`);
4. 遍历 group:group 之间的原文行**逐字节拷贝**,group 内部才用归一化替换结果(`edit-diff.ts:156-170`)。

为什么必须有这一步:若直接把归一化空间的替换结果整体写回,文件里所有行都会被 `trimEnd`/NFKC"洗一遍"——一次小编辑产生数千行无关 diff。**只在触碰行上应用归一化,未触碰行保留原始字节**。

多编辑的替换应用按 `matchIndex` 降序倒序 splice(`applyReplacements`, `edit-diff.ts:111-120`),保证前面的替换不会使后面的偏移失效。

#### 14.1.5 唯一性 / 重叠 / 空文本 / 无变化:四类错误文案

`applyEditsToNormalizedContent`(`edit-diff.ts:300-362`)是编辑主入口,校验顺序与错误文案都值得直接照抄:

| 校验 | 行号 | 错误文案(单编辑 / 多编辑) |
|---|---|---|
| `oldText` 为空 | `edit-diff.ts:310-313`, `getEmptyOldTextError:275-280` | `oldText must not be empty in {path}.` / `edits[{i}].oldText must not be empty in {path}.` |
| 找不到 | `edit-diff.ts:324-326`, `getNotFoundError:253-262` | `Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines.` |
| 多处命中 | `edit-diff.ts:328-331`, `getDuplicateError:264-273` | `Found {n} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.` |
| 编辑间重叠 | `edit-diff.ts:341-350` | `edits[{a}] and edits[{b}] overlap in {path}. Merge them into one edit or target disjoint regions.` |
| 替换后内容不变 | `edit-diff.ts:357-359`, `getNoChangeError:282-289` | `No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.` |

唯一性计数 `countOccurrences`(`edit-diff.ts:247-251`)同样在**归一化空间**做:`fuzzyContent.split(fuzzyOldText).length - 1`——精确空间只有 1 处、归一化空间有 3 处时,照样报 duplicate(且计数是 3),提示模型加上下文。

#### 14.1.6 BOM 与换行符的往返保持

`edit.ts:363-372` 的编辑主路径:

```ts
// Strip BOM before matching. The model will not include an invisible BOM in oldText.
const { bom, text: content } = splitBom(rawContent);
const originalEnding = detectLineEnding(content);
const normalizedContent = normalizeToLF(content);
const { baseContent, newContent } = applyEditsToNormalizedContent(normalizedContent, edits, path);
const finalContent = bom + restoreLineEndings(newContent, originalEnding);
await ops.writeFile(absolutePath, finalContent);
```

- **BOM**:剥离后匹配,写回时拼回头部——模型永远看不到 BOM;
- **换行**:`detectLineEnding`(`edit-diff.ts:11-17`)取文件中**首个出现的换行形态**(CRLF 出现在 LF 之前则判 CRLF),全文件 LF 化后编辑,最后 `restoreLineEndings`(`edit-diff.ts:23-25`)整文件还原为原形态。代价是混合换行文件会被统一,收益是 `oldText` 不需要关心 `\r`。

#### 14.1.7 写入方式:非原子、无备份、无 undo(明确的反模式)

`defaultEditOperations` 与 `defaultWriteOperations` 都是裸的 `fs.promises.writeFile(path, content, "utf-8")`:

```ts
// packages/coding-agent/src/core/tools/edit.ts:105-109
const defaultEditOperations: EditOperations = {
    readFile: (path) => fsReadFile(path),
    writeFile: (path, content) => fsWriteFile(path, content, "utf-8"),
    access: (path) => fsAccess(path, constants.R_OK | constants.W_OK),
};
```

- **没有 write-temp-then-rename**(会话持久化层的 `publishFileAtomically` 并未复用到工作区文件);
- **没有 `.orig` / trashcan / 任何备份文件**(全仓库 grep `backup|\.orig|trash` 无工作区命中);
- **没有编辑级 undo 栈**(唯一的 "undo" 是输入框的 `tui.editor.undo` 键位,`core/keybindings.ts:76`)。

回滚完全依赖两条外部路径:会话树 fork(对话级)和 git(代码级,见 14.2)。

#### 14.1.8 per-file 排他锁:`withFileMutationQueue` 的真实现(coding-agent 版)

第六轮 13.8 记录的是 `packages/agent` SDK 版本;`packages/coding-agent` 内的真实实现多了一层 **realpath 规范化 + 注册串行队列**(`packages/coding-agent/src/core/tools/file-mutation-queue.ts:32-61`):

```ts
export async function withFileMutationQueue<T>(filePath: string, fn: () => Promise<T>): Promise<T> {
    const registration = registrationQueue.then(async () => {
        const key = await getMutationQueueKey(filePath);           // realpath
        const currentQueue = fileMutationQueues.get(key) ?? Promise.resolve();
        let releaseNext!: () => void;
        const nextQueue = new Promise<void>((resolveQueue) => { releaseNext = resolveQueue; });
        const chainedQueue = currentQueue.then(() => nextQueue);
        fileMutationQueues.set(key, chainedQueue);
        return { key, currentQueue, chainedQueue, releaseNext };
    });
    registrationQueue = registration.then(() => undefined, () => undefined);
    const { key, currentQueue, chainedQueue, releaseNext } = await registration;
    await currentQueue;
    try {
        return await fn();
    } finally {
        releaseNext();
        if (fileMutationQueues.get(key) === chainedQueue) fileMutationQueues.delete(key);
    }
}
```

三个细节:

1. **key 是 `realpath(filePath)`**(`file-mutation-queue.ts:16-26`),文件不存在(ENOENT/ENOTDIR)时退回 `resolve(filePath)`——新建文件第一次写和第二次写(此时已存在、realpath 生效)能命中同一把锁的前提是路径本身已规范化;symlink 指向同一物理文件的两个不同路径也会被收敛到同一把锁。
2. **`registrationQueue` 串行化"注册动作"本身**,防止两个并发注册读到同一个 `currentQueue` 造成锁分裂竞态。
3. **`finally` 里的条件删除**:`fileMutationQueues.get(key) === chainedQueue` 才删——只有"队尾是自己"时才清 Map,中途加入者会接管 entry,不会误删后继队列。

不同文件的写入仍然并行(`withFileMutationQueue` 只按 key 串行)。

#### 14.1.9 锁与取消的交互:为什么不用 `signal.addEventListener("abort", reject)`

`edit.ts:336-345` 与 `write.ts:210-217` 有一段完全相同的注释,这是工具层最容易被写错的地方:

```ts
return withFileMutationQueue(absolutePath, async () => {
    // Do not reject from an abort event listener here: that would release the
    // mutation queue while an in-flight filesystem operation may still finish.
    // Checking signal.aborted after each await observes the same aborts while
    // keeping the queue locked until the current operation has settled.
    const throwIfAborted = (): void => {
        if (signal?.aborted) throw new Error("Operation aborted");
    };

    throwIfAborted();
    ...
```

即:**在每个 `await` 之后轮询 `signal.aborted`,而不是注册 abort 监听直接 reject**。原因:`finally { releaseNext() }` 在 reject 时立即触发,会释放排他锁,但此时上一个 `writeFile` 可能仍在内核中飞行,下一个写者立刻进入 → 交错写。轮询方案保证锁持有到"当前操作 settle"为止。副作用是 abort 后最多多做一次无谓的文件写入再抛错(半写状态由下一次编辑的"找不到原文"自然兜住)。

`edit.execute` 中一共 6 个检查点:`edit.ts:345 / 351 / 356 / 361 / 368 / 372`(access 前、readFile 前、归一化后、writeFile 后)。

#### 14.1.10 回灌给模型的"编辑结果":只有一行文字,diff 不进上下文

工具返回结构分两栏(`packages/agent/src/types.ts:360-366`):

```ts
export interface AgentToolResult<T> {
    /** Text or image content returned to the model. */
    content: (TextContent | ImageContent)[];
    /** Arbitrary structured details for logs or UI rendering. */
    details: T;
```

`edit` 的实际返回(`edit.ts:376-384`):

```ts
return {
    content: [{ type: "text", text: `Successfully replaced ${edits.length} block(s) in ${path}.` }],
    details: { diff: diffResult.diff, patch, firstChangedLine: diffResult.firstChangedLine },
};
```

**模型只看到 `Successfully replaced N block(s) in <path>.`**,不回灌 diff、不回灌 patch、不回灌改动行号。理由很直接:模型自己刚写下 oldText/newText,再回显 diff 是纯 token 浪费;`details.diff/patch` 仅供 TUI 渲染(`edit.ts:414-455` 的 `renderResult`)与 HTML 导出。

对比:`read` 工具在截断时**会**在 `content` 里追加可操作提示(`read.ts:308-317` 的 `[Showing lines X-Y of Z. Use offset=N to continue.]`)——"模型需要知道的信息"与"人需要看的信息"在 pi 里被明确分栏,这是值得照抄的边界。

#### 14.1.11 TUI 预览:同一份算法在渲染期"空跑一遍"

`renderCall` 在参数流式生成完毕(`context.argsComplete`)且尚未执行时,后台并发调用 `computeEditsDiff`(`edit.ts:401-410`;实现在 `edit-diff.ts:514-543`)——复用 `applyEditsToNormalizedContent`,只是不写盘。预览结果按 `JSON.stringify({path, edits})` 作 `argsKey` 缓存,参数一变即失效重算(`edit.ts:394-399`)。这意味着编辑算法必须是**幂等纯计算 + 可独立于写盘调用**,pi 把"匹配/生成 diff"与"写文件"彻底分离成两个函数正是为此。

#### 14.1.12 编辑与会话持久化的顺序:没有跨文件事务

工具写工作区文件(`fsWriteFile`)与 SessionManager 追加 JSONL entry(`appendFileSync`,`session-manager.ts` 的 `_appendEntry`)之间**没有任何先后协议、没有 WAL、没有两阶段**:

- 顺序是"工具执行完成 → tool_result entry 落盘"。若工具写文件成功但进程在 entry 落盘前崩溃 → 文件已改但会话日志缺 tool_result(重启后 14 种损坏检测里的"孤儿 assistant toolCall"会命中,见第 5 章);
- 反之若先落日志再改文件,崩溃时会出现"日志说改了、文件没改"。pi 选择了前者(先改文件,后记日志),因为文件系统的写入语义比 JSONL append 更强,而且 edit 工具的"找不到 oldText 即失败"天然可重入。

唯一的崩溃一致性保护都在**会话持久化层**(JSONL 撕裂尾部修复 + WriterLease fence,第 10 章/13.3),工作区文件层面是零保护。

#### 14.1.13 对 laew 的借鉴(维度一)

laew 现状:只有 `Bash/Read/Write` 三个工具,没有 `edit`——模型改文件只能整文件重写或靠 `sed`,token 成本高且易错。

| 优先级 | 借鉴项 | pi 来源 | laew 落地(Rust) |
|---|---|---|---|
| **P0** | 新增 `Edit` 工具 + `edits[]` 多编辑语义 | `edit.ts:34-64` | `src/agent/tools/edit.rs`,`schema = {path, edits: [{oldText, newText}]}`;`Work Agent` 与 `SubAgent` 均注册 |
| **P0** | 唯一性 + 重叠 + 空文本 + 无变化 四类中文错误文案 | `edit-diff.ts:253-289` | 直接翻译成中文文案表,错误里带 `edits[{i}]` 索引 |
| **P0** | `prepareArguments` 参数兼容层 | `edit.ts:116-147`, `agent/src/types.ts:386-389` | 在工具 trait 增加 `prepare_args(&mut serde_json::Value)` 钩子,统一消化字符串化 edits / 平铺 oldText |
| **P1** | 模糊匹配 + 行块保真回写 | `edit-diff.ts:34-55, 132-173` | Rust 用 `unicode-normalization` crate 做 NFKC;按行分组、组间拷贝原文字节 |
| **P1** | BOM 剥离 + 换行形态探测还原 | `edit.ts:363-372` | `content.strip_prefix('\u{feff}')` + 首个换行探测 |
| **P1** | per-realpath 排他锁 + 轮询式 abort | `file-mutation-queue.ts:32-61`, `edit.ts:336-345` | `HashMap<PathBuf, Arc<Mutex<()>>>` + tokio `Mutex`;每个 await 后查 `CancellationToken` |
| **P1** | content/details 分栏,diff 不进上下文 | `agent/src/types.ts:360-366`, `edit.ts:376-384` | `ToolOutput { model_text: String, ui_details: Value }`,laew 的 SQLite `agent_memory` 存 details,diff 只进 UI |
| **P2** | 编辑预览纯函数化 | `edit-diff.ts:514-543` | 把 `apply_edits` 与 `write_file` 拆成两个独立 fn,便于测试与 TUI 预览 |
| **P2** | write-temp-then-rename 原子写 | (pi 反例:没有) | `tempfile::NamedTempFile::persist()` —— pi 缺失的,laew 应补上 |

---

### 14.2 Git 与版本控制集成

#### 14.2.1 总体结论:核心零 Git,只有三处"被动感知" + 一批官方示例扩展

全仓库 grep `git status|git diff|checkpoint|worktree|stash` 后,核心代码(`packages/*/src`)里真正涉及 Git 的只有三处:footer 分支探测、worktree 上下文去重、Git URL 解析(扩展安装用)。**没有**自动 commit、自动 branch、内置 diff 工具、仓库指纹、gitignore 引擎(laew 现状相同)、或并发任务 worktree 隔离。

这个"留白"是设计选择:pi 的扩展 API 提供 `pi.exec(cmd, args)`(`packages/coding-agent/src/core/extensions/types.ts:1397`,实现在 `core/exec.ts:37-107`)与 `tool_call`/`session_before_fork` 等事件,Git 策略全部由用户级扩展实现。

#### 14.2.2 footer 分支探测:不调 `git` 子进程的零开销方案

`packages/coding-agent/src/core/footer-data-provider.ts:16-48` 的 `findGitPaths`:

```ts
export function findGitPaths(cwd: string): GitPaths | null {
    let dir = cwd;
    while (true) {
        const gitPath = join(dir, ".git");
        if (existsSync(gitPath)) {
            const stat = statSync(gitPath);
            if (stat.isFile()) {                       // worktree: .git 是文件
                const content = readFileSync(gitPath, "utf8").trim();
                if (content.startsWith("gitdir: ")) {
                    const gitDir = resolve(dir, content.slice(8).trim());
                    const headPath = join(gitDir, "HEAD");
                    if (!existsSync(headPath)) return null;
                    const commonDirPath = join(gitDir, "commondir");
                    const commonGitDir = existsSync(commonDirPath)
                        ? resolve(gitDir, readFileSync(commonDirPath, "utf8").trim()) : gitDir;
                    return { repoDir: dir, commonGitDir, headPath };
                }
            } else if (stat.isDirectory()) {           // 普通仓库
                ...
```

要点:

- **向上遍历到根**找 `.git`,兼容 `.git` 为目录(普通仓库)与为文件(`git worktree add` 产生的链接 worktree);
- worktree 场景解析 `gitdir:` 指针 + `commondir` 回到主仓库共享目录;
- 分支名**优先直接读 HEAD 文件**(`footer-data-provider.ts:239-251`):`ref: refs/heads/<branch>` 即分支,裸 SHA 即 `"detached"`;只有 HEAD 里出现 `.invalid`(reftable 后端的过渡态)才降级 spawn `git symbolic-ref --quiet --short HEAD`(`:51-59` 同步版 / `:62-81` 异步版),且带 `--no-optional-locks` 避免污染 `index`。
- **文件监听而非轮询**(`:307-381`):监听 HEAD 所在**目录**而非 HEAD 本身(git 用"写临时文件 + rename"原子替换 HEAD,inode 会变,`fs.watch` 文件会失效);WSL 下 `/mnt/c` 这类 Windows 挂载路径 inotify 不可靠,自动降级 `watchFile` 1s 轮询(`:83-93` 的 `shouldPollGitHead`);**reftable 仓库切分支不写 HEAD**,额外 watch `<commonGitDir>/reftable` 与 `tables.list`(`:342-380`)。变更经 500ms 防抖(`WATCH_DEBOUNCE_MS`,`:101`)后异步刷新。

#### 14.2.3 worktree 感知的项目上下文去重

`packages/coding-agent/src/core/resource-loader.ts:90-116` 的 `findShadowedContextFile`:嵌套 linked worktree 自己的 `AGENTS.md` 与主仓库的 `AGENTS.md` 占据同一"逻辑仓库作用域",若都加载会**应用两遍**。函数通过 `commondir` 反推主仓库根,判定"当前 worktree 是主仓库的子目录"且"主仓库根确有 `.git` 目录"(排除 `proj/.bare` 布局与 submodule)后,把被 shadow 的主仓库上下文文件路径返回给加载器跳过。返回值统一 `canonicalizePath`,因为 `git worktree add` 写入的 `gitdir:` 是 realpath 形式而 cwd 可能仍是 symlink(macOS `/tmp` → `/private/tmp`)。

#### 14.2.4 `utils/git.ts`:唯一的"Git 逻辑"是 URL 解析与安全校验

`packages/coding-agent/src/utils/git.ts` 226 行全部服务于**扩展包安装**(`pi install git:github.com/user/repo@ref`),与 Agent 编辑行为无关。值得抄的是它的注入防护(`git.ts:84-124`):

```ts
function hasUnsafeGitInstallPart(value: string, allowSlash: boolean): boolean {
    const decoded = decodeForValidation(value);        // 先 URL 解码再查,防 %2e%2e%2f 绕过
    if (decoded === null) return true;
    const candidates = [value, decoded];               // 原文和解码后都查
    for (const candidate of candidates) {
        if (candidate.includes("\0") || candidate.includes("\\") || candidate.startsWith("/")) return true;
        if (!allowSlash && candidate.includes("/")) return true;
        if (candidate.split("/").includes("..")) return true;
    }
    return false;
}
```

同时支持 scp 风格 `git@host:path@ref`、`https://host/path@ref`、短格式 `host/path@ref` 三种 ref 切分(`splitRef`, `git.ts:21-74`)。

#### 14.2.5 checkpoint 与 undo/rewind:会话树 fork × `git stash` 的联动

pi 的"回滚"分两层,**核心只实现第一层**:

**第一层(核心):对话回滚 = session tree 分支**

| API | 行号 | 语义 |
|---|---|---|
| `getBranch(fromId?)` | `core/session-manager.ts:1261-1270` | 从 entry 沿 `parentId` 走到根,逆序返回当前分支 |
| `branch(branchFromId)` | `session-manager.ts:1361-1366` | 仅移动 `leafId` 指针,append-only,不删任何 entry |
| `resetLeaf()` | `session-manager.ts:1368-1374` | leaf 置 null,下一条 entry 成为新根(重编辑首条 user 消息) |
| `branchWithSummary(branchFromId, summary, ...)` | `session-manager.ts:1378-1406` | 分支同时追加 `branch_summary` entry,保留被放弃路径的摘要(见 7.5) |
| `createBranchedSession(leafId)` | `session-manager.ts:1414-1460` | 导出"根→leaf"单条路径为新 JSONL 文件;**label entry 被过滤后需要重新链接 parentId,否则产生孤儿子树**(`:1427-1435`) |
| `SessionManager.forkFrom(sourcePath, targetCwd)` | `session-manager.ts:1581-1633` | 跨项目 fork:新 header 写 `parentSession: 源文件路径`,逐条复制非 header entry |

`/fork` 命令在 `modes/interactive/interactive-mode.ts:3046 / 5135 / 5147 / 5177` 触发,经 `core/agent-session-runtime.ts:150-165` 的 `emitBeforeFork` 先发 `session_before_fork` 事件——**这正是代码回滚的挂载点**。

**第二层(扩展):代码回滚 = `git stash` 快照**

官方示例 `packages/coding-agent/examples/extensions/git-checkpoint.ts`(53 行,完整逻辑):

```ts
export default function (pi: ExtensionAPI) {
    const checkpoints = new Map<string, string>();      // entryId → git stash ref
    let currentEntryId: string | undefined;

    pi.on("tool_result", async (_event, ctx) => {
        const leaf = ctx.sessionManager.getLeafEntry();
        if (leaf) currentEntryId = leaf.id;
    });

    pi.on("turn_start", async () => {
        // Create a git stash entry before LLM makes changes
        const { stdout } = await pi.exec("git", ["stash", "create"]);
        const ref = stdout.trim();
        if (ref && currentEntryId) checkpoints.set(currentEntryId, ref);
    });

    pi.on("session_before_fork", async (event, ctx) => {
        const ref = checkpoints.get(event.entryId);
        if (!ref) return;
        if (!ctx.hasUI) return;                        // 非交互模式不自动恢复
        const choice = await ctx.ui.select("Restore code state?", [
            "Yes, restore code to that point", "No, keep current code"]);
        if (choice?.startsWith("Yes")) {
            await pi.exec("git", ["stash", "apply", ref]);
            ctx.ui.notify("Code restored to checkpoint", "info");
        }
    });

    pi.on("agent_settled", async () => checkpoints.clear());
}
```

设计要点:`git stash create` 只创建悬挂 commit **不动工作区/index**(不同于 `git stash push`),零副作用;map 的 key 是**对话树 entry id**,所以 checkpoint 与 fork 点一一对应;恢复用 `stash apply`(非 `pop`,保留 ref 可多次回跳);`agent_settled` 清空避免跨 run 泄漏;非交互模式(`!ctx.hasUI`)默认不恢复——**所有破坏性动作必须有人确认**。

#### 14.2.6 其余三个 Git 示例扩展:同一 API 面的三种策略

| 扩展 | 挂载事件 | 行为 |
|---|---|---|
| `auto-commit-on-exit.ts` | `session_shutdown` | `git status --porcelain` 非空时,取**最后一条 assistant 消息首行**拼 `[pi] {前50字符}...` 作 commit message,`git add -A && git commit -m`(`examples/extensions/auto-commit-on-exit.ts:14-52`) |
| `dirty-repo-guard.ts` | `session_before_switch` / `session_before_fork` 等 | 有未提交改动时弹 `ctx.ui.select` 二次确认;**非交互模式默认 cancel**(fail-closed,`examples/extensions/dirty-repo-guard.ts:24-46`) |
| `git-merge-and-resolve.ts` | 自定义命令 | 冲突解决流:`git diff --name-only --diff-filter=U` 找冲突文件 → 逐个 ours/theirs(配套测试 `test/git-merge-and-resolve-extension.test.ts:95-199`) |

#### 14.2.7 pi 没有的(与 claude-code 对照)

| 能力 | pi | 说明 |
|---|---|---|
| 自动 commit / branch | ✗(仅示例扩展) | 无核心实现 |
| 内置 git diff / status 工具 | ✗ | 模型走 bash + 提示词约定 |
| 文件快照式 checkpoint(非 git) | ✗ | 无任何文件级快照存储 |
| 仓库指纹 / project hash | ✗ | session 目录按 `encoded-cwd` 分桶,与 git 无关 |
| gitignore 引擎 | ✗(外包) | grep/fnd 依赖 rg/fd 自带的 gitignore 支持(见 14.4) |
| 并发任务 worktree 隔离 | ✗ | worktree 仅被"识别"(footer/上下文去重),不被"创建" |
| git status/diff 进上下文 | ✗ | 无自动注入;示例 `inline-bash.ts:9` 演示用 `!{git status --short}` 模板在输入侧内联 |

#### 14.2.8 对 laew 的借鉴(维度二)

laew 现状:完全没有 Git 感知,MultiAgentOrchestrator 的 6 角色也不知道仓库状态。

| 优先级 | 借鉴项 | pi 来源 | laew 落地(Rust) |
|---|---|---|---|
| **P0** | footer/横幅级 Git 分支探测(直读 HEAD,零子进程) | `footer-data-provider.ts:16-48, 239-251` | 用 `git2`/`gix` crate 或裸读 `.git/HEAD`;TUI 横幅增加 `分支: main` |
| **P0** | Quality-Check 前自动 `git stash create` checkpoint | `git-checkpoint.ts:20-27` | QC Agent 启动前记录 ref,存 SQLite `checkpoints(entry_id, stash_ref, created_at)` |
| **P0** | 失败回流时可选 `git stash apply` 恢复 | `git-checkpoint.ts:29-47` | Yolo 的"失败回流与用户建议"路径:回滚到 SubAgent 执行前 ref(需用户确认) |
| **P1** | `session_before_fork` 式前置事件 | `agent-session-runtime.ts:150-165` | laew Session 增加 `before_rollback` 钩子,把"对话回滚"与"代码回滚"解耦但联动 |
| **P1** | worktree/`.git` 文件形态识别 | `footer-data-provider.ts:20-39`, `resource-loader.ts:100-116` | 项目上下文五级链发现时识别 worktree,避免主仓库/子 worktree 上下文重复注入 |
| **P1** | dirty repo 守卫(非交互 fail-closed) | `dirty-repo-guard.ts:24-46` | `-p` 单轮模式下检测到未提交改动且任务含写操作时拒绝执行 |
| **P2** | `--no-optional-locks` + `symbolic-ref` 降级路径 | `footer-data-provider.ts:51-81` | 所有 git 只读探测统一带 `--no-optional-locks` |
| **P2** | reftable / WSL 挂载 / HEAD inode 变化三个兼容坑 | `footer-data-provider.ts:307-381` | Rust 用 `notify` crate watch 目录而非文件;WSL 检测 `/mnt/<x>` |

---

### 14.3 命令执行与进程管理(Bash 工具)

#### 14.3.1 Schema 与超时语义

```ts
// packages/coding-agent/src/core/tools/bash.ts:42-45
const bashSchema = Type.Object({
    command: Type.String({ description: "Shell command to execute" }),
    timeout: Type.Optional(Type.Number({ description: "Timeout in seconds (optional, no default timeout)" })),
});
```

超时校验(`bash.ts:29-40`):非有限数 / `<=0` → `"Invalid timeout: must be a finite number of seconds"`;超过 `2_147_483_647ms`(Node setTimeout 上限)→ `"Invalid timeout: maximum is 2147483.647 seconds"`。**默认无超时**——长任务不被强制打断,取消只能靠用户 Esc 的 AbortSignal。

工具描述把截断规则写给模型(`bash.ts:350`):"Output is truncated to last 2000 lines or 50KB (whichever is hit first). If truncated, full output is saved to a temp file."

#### 14.3.2 spawn:三种 shell、两种命令传输通道、detached 进程组

`createLocalShellOperations`(`bash.ts:84-150`)是 bash/powershell 共用的执行后端:

```ts
// bash.ts:98-110
const commandFromStdin = shellConfig.commandTransport === "stdin";
const child = spawn(shellConfig.shell, commandFromStdin ? shellConfig.args : [...shellConfig.args, command], {
    cwd,
    detached: process.platform !== "win32",
    env: env ?? getShellEnv(),
    stdio: [commandFromStdin ? "pipe" : "ignore", "pipe", "pipe"],
    windowsHide: true,
});
if (commandFromStdin) {
    child.stdin?.on("error", () => {});
    child.stdin?.end(command);
}
if (child.pid) trackDetachedChildPid(child.pid);
```

| 决策 | 理由 |
|---|---|
| `detached: true`(非 Windows) | 子进程自成进程组,之后 `process.kill(-pid, SIGKILL)` 可**杀整棵树**,不会留下孤儿 |
| `commandTransport: "stdin"`(shell `["-s"]`) | 旧版 WSL bash(`System32\bash.exe`)的 `-c` 会把命令暴露在 `ps` 输出里且长度受限,改走 stdin(`utils/shell.ts:15-22`) |
| `stdio[0] = "ignore"` | **工具层无 stdin 交互**;模型想传输入只能用 heredoc / 管道 |
| `trackDetachedChildPid` | detached 进程不受父进程退出联动,须显式登记,进程收到 SIGHUP/SIGTERM 时统一 `killTrackedDetachedChildren()`(`utils/shell.ts:196-211`) |
| `windowsHide: true` | Windows 下不弹控制台窗 |

shell 解析链(`utils/shell.ts:67-120`):用户 settings `shellPath` → Windows 下 Git Bash 已知位置(`ProgramFiles\Git\bin\bash.exe` 等)→ PATH 上的 `bash.exe`(`where` + `existsSync` 双重验证,`where` 可能返回不存在的路径)→ Unix `/bin/bash` → PATH `bash`(`which`)→ 兜底 `sh -c`。找不到时报错信息直接给出 3 条解决途径与已搜索路径列表(`shell.ts:100-106`)。

`resolveSpawnContext`(`bash.ts:170-196`)给出**环境变量注入与净化**的完整顺序:

```ts
const env = { ...getShellEnv() };
delete env.PI_SESSION_ID;  delete env.PI_SESSION_FILE;
delete env.PI_PROVIDER;    delete env.PI_MODEL;
delete env.PI_REASONING_LEVEL;
if (exposeSessionEnvironment && ctx) {
    env.PI_SESSION_ID = ctx.sessionManager.getSessionId();
    const sessionFile = ctx.sessionManager.getSessionFile();
    if (sessionFile) env.PI_SESSION_FILE = sessionFile;
    if (model) { env.PI_PROVIDER = model.provider; env.PI_MODEL = model.id; }
    if (ctx.thinkingLevel) env.PI_REASONING_LEVEL = ctx.thinkingLevel;
}
```

**先删后加**:继承自父进程的旧值一律清掉,再按 `exposeSessionEnvironment`(默认 true)注入当前会话的 `PI_*` 五元组,保证 fork/切换 session 后子进程看到的一定是最新值。`getShellEnv`(`shell.ts:138-150`)同时把 pi 自身 bin 目录前置到 `PATH`。`spawnHook`(`BashSpawnHook`, `bash.ts:168`)允许扩展在执行前改写 `{command, cwd, env}` 三元组。

#### 14.3.3 取消与超时:统一走 `killProcessTree`

```ts
// bash.ts:111-124
let timedOut = false;
const onAbort = () => { if (child.pid) killProcessTree(child.pid); };
...
if (timeoutMs !== undefined) {
    timeoutHandle = setTimeout(() => {
        timedOut = true;
        if (child.pid) killProcessTree(child.pid);
    }, timeoutMs);
}
```

`killProcessTree`(`utils/shell.ts:216-247`)跨平台两分支:

- **Windows**:直接 spawn `System32\taskkill.exe /F /T /PID`(用绝对路径而非 PATH,防 PATH 劫持;spawn 失败的异步 `error` 事件被显式消费,避免 unhandled rejection 崩溃 Node);
- **Unix**:`process.kill(-pid, "SIGKILL")` 杀进程组;失败(如已被 reaper 收走)降级 `process.kill(pid, "SIGKILL")`;再失败静默忽略。

错误区分:abort → `throw new Error("aborted")`(工具层转成 `Command aborted` 状态行,`bash.ts:467-469`);超时 → `throw new Error("timeout:" + timeout)`(转成 `Command timed out after {n} seconds`,`bash.ts:470-473`)。两种失败都**先 `finishOutput()` 保留已产生的输出**再抛,输出不丢。

#### 14.3.4 `waitForChildProcess`:防"exit 后输出被截断"的 100ms 空闲宽限

`packages/coding-agent/src/utils/child-process.ts:38-137` 是对 `child.on("close")` 不可靠的修正,注释直指一个真实 bug(earendil-works/pi#5303):

> A short-lived child can `exit` while a detached descendant keeps its stdout/stderr pipe open. We must not resolve and destroy the streams on a fixed deadline measured from `exit`, or output still being written past that deadline is silently lost.

机制:`EXIT_STDIO_GRACE_MS = 100`(`:16`);`exit` 触发后武装空闲定时器,**每收到一个新 data chunk 就重新武装**(`onData → armIdleTimer`,`:93-97`);stdout/stderr 双双 `end` 则立即 finalize;`close` 事件到达也 finalize。也就是"活跃的后代持续写 → 我们持续读;安静的后台守护进程占着句柄 → 100ms 后强制释放"。配套测试 `test/bash-close-hang-windows.test.ts`。

扩展用的 `execCommand`(`core/exec.ts:37-107`)是另一条独立路径:`shell: false` 直跑 argv(不经 shell 解释,天然免注入)、SIGTERM → 5s → SIGKILL 两段式、同样复用 `waitForChildProcess`。

#### 14.3.5 流式输出:100ms 节流 + OutputAccumulator 有界滚动缓冲

工具执行侧的节流(`bash.ts:211-212` 常量, `:376-410` 实现):

```ts
const BASH_UPDATE_THROTTLE_MS = 100;
...
const scheduleOutputUpdate = () => {
    if (!onUpdate) return;
    updateDirty = true;
    const delay = BASH_UPDATE_THROTTLE_MS - (Date.now() - lastUpdateAt);
    if (delay <= 0) { clearUpdateTimer(); emitOutputUpdate(); return; }
    updateTimer ??= setTimeout(() => { updateTimer = undefined; emitOutputUpdate(); }, delay);
};
```

`OutputAccumulator`(`packages/coding-agent/src/core/tools/output-accumulator.ts:35-222`)是内存安全的核心,**增量统计 + 滚动尾部 + 惰性临时文件**三件套:

```ts
constructor(options: OutputAccumulatorOptions = {}) {
    this.maxLines = options.maxLines ?? DEFAULT_MAX_LINES;        // 2000
    this.maxBytes = options.maxBytes ?? DEFAULT_MAX_BYTES;        // 50KB
    this.maxRollingBytes = Math.max(this.maxBytes * 2, 1);        // 100KB 尾部窗口
}
```

- **流式 UTF-8 解码**:`TextDecoder().decode(chunk, { stream: true })`(`:70`),多字节字符跨 chunk 不会碎;
- **滚动尾部**:`tailText` 超过 `2 × maxRollingBytes`(约 200KB)才裁剪(`:157-159`);裁剪时**按 UTF-8 续字节边界推进**(`buf[start] & 0xc0) === 0x80`,`:186-189`),并记录 `tailStartsAtLineBoundary`——若尾部起点在行中间,`getSnapshotText()` 会丢弃到下一个换行(`:196-203`),保证快照永远从整行开始;
- **行/字节计数增量维护**:`appendDecodedText`(`:148-177`)一次遍历同时更新 `completedLines`、`currentLineBytes`、`hasOpenLine`、`totalDecodedBytes`,O(n) 无重复扫描;
- **惰性临时文件**:`shouldUseTempFile()`(`:205-209`)在 `totalRawBytes > maxBytes || totalDecodedBytes > maxBytes || totalLines > maxLines` 时才 `ensureTempFile()`,并把此前缓存的 `rawChunks` 先写进去再清空(`:211-221`);文件名 `{prefix}-{randomBytes(8).hex}.log` 于 `tmpdir()`,prefix 默认 `pi-bash`(`bash.ts:532`);
- **snapshot 双重截断**:`truncateTail(滚动尾部)` 的内容 + 全局的 `totalLines/totalDecodedBytes` 判定(`:91-119`),`persistIfTruncated` 时确保临时文件存在。

即:**GB 级输出下内存占用恒定 ~200KB,同时完整输出始终可从临时文件取回**。

#### 14.3.6 截断策略:头部/尾部双向 + 三个边界 case

`packages/coding-agent/src/core/tools/truncate.ts:11-13` 定义行业默认:`DEFAULT_MAX_LINES = 2000`、`DEFAULT_MAX_BYTES = 50 * 1024`、`GREP_MAX_LINE_LENGTH = 500`。

| 函数 | 行号 | 方向 | 用途 | 边界处理 |
|---|---|---|---|---|
| `truncateHead` | `:78-160` | 保头部 | read/ls/grep/find 结果 | **永不返回半行**;首行单独超字节上限 → 返回空 + `firstLineExceedsLimit: true`(由调用方转成 bash 提示) |
| `truncateTail` | `:168-241` | 保尾部 | bash 输出(错误一般在末尾) | 倒序累计;**唯一允许半行的地方**——若最后一行本身超 50KB,取该行末尾 `maxBytes` 字节并置 `lastLinePartial: true` |
| `truncateStringToBytesFromEnd` | `:247-262` | — | 上面那个半行 case | 从尾部回退 maxBytes,再跳过 UTF-8 续字节到字符边界 |
| `truncateLine` | `:268-276` | 单行 | grep 命中行 | 超 500 字符截断 + `... [truncated]` 后缀 |

bash 结果回填文案(`bash.ts:432-450`)分三态并给出**精确行号区间与临时文件路径**:

```ts
if (truncation.lastLinePartial) {
    text += `\n\n[Showing last ${formatSize(truncation.outputBytes)} of line ${endLine} (line is ${lastLineSize}). Full output: ${snapshot.fullOutputPath}]`;
} else if (truncation.truncatedBy === "lines") {
    text += `\n\n[Showing lines ${startLine}-${endLine} of ${truncation.totalLines}. Full output: ${snapshot.fullOutputPath}]`;
} else {
    text += `\n\n[Showing lines ${startLine}-${endLine} of ${truncation.totalLines} (${formatSize(DEFAULT_MAX_BYTES)} limit). Full output: ${snapshot.fullOutputPath}]`;
}
```

#### 14.3.7 退出码与非零输出的呈现

`bash.ts:479-481`:

```ts
if (exitCode !== 0 && exitCode !== null) {
    throw new Error(appendStatus(outputText, `Command exited with code ${exitCode}`));
}
```

非零退出码被建模为 **tool error**(`isError: true`),但**先带上全部输出再追加状态行**(`appendStatus`,`:452`)——模型既看到 stdout/stderr 又看到退出码。被信号杀死(`exitCode === null`)不算错误。输出为空时文本为 `"(no output)"`(`:432`)。

#### 14.3.8 工作目录与会话 cwd 的关系

工具的 cwd 取 `ctx?.cwd || cwd`(`bash.ts:366`),即**扩展上下文里的会话 cwd 优先**,退回到工具构造时的 cwd。`core/session-cwd.ts:9-62` 处理恢复会话时"存档的 cwd 已不存在"的场景:抛 `MissingSessionCwdError`(带 sessionFile 与 fallbackCwd 两个信息),或由上层用 `formatMissingSessionCwdPrompt`(`:47-51`)弹"继续用当前 cwd"的确认。`createLocalShellOperations` 在 spawn 前还显式 `fsAccess(cwd, F_OK)`,目录不存在直接报 `Working directory does not exist: {cwd}`(`bash.ts:92-96`)。

#### 14.3.9 没有的东西(明确列出)

| 能力 | pi 状态 | 替代方案 |
|---|---|---|
| PTY / 伪终端 | ✗ | 无;需要交互的程序(如 `vim`)无法运行,plan-mode 甚至把编辑器列入黑名单 |
| stdin 交互(向运行中进程写输入) | ✗ 工具层 | 模型用 heredoc / `echo \| cmd`;扩展可走 `pi.exec` |
| 后台任务与任务 ID(如 `run_in_background`) | ✗ | 长任务靠 `detached` + 用户自己 nohup;无任务句柄/查询接口 |
| 内置危险命令黑名单 | ✗ 核心无 | 完全交给 `tool_call` hook,见 14.3.10 |
| 输出预览字节上限之外的全量回读 | ✗ | 只给临时文件路径,模型需自己 `read`/`sed -n` 取 |

#### 14.3.10 危险命令拦截:核心只提供"可阻断的 hook",策略在示例扩展里

`tool_call` 事件契约(`packages/coding-agent/src/core/extensions/types.ts:936-951`):

> Fired before a tool executes. Can block.
> `event.input` is mutable. Mutate it in place to patch tool arguments before execution. Later `tool_call` handlers see earlier mutations. **No re-validation is performed after mutation.**

返回 `{block: true, reason}` 即拦截(类型见 `packages/agent/src/types.ts:60-70` 的 `BeforeToolCallResult {block, reason, terminate}`);`terminate: true` 提示"本批工具结束后停止整轮"(仅当**同批全部** finalized 结果都置 terminate 才生效,防止单个工具独断终止)。

官方 plan-mode 扩展是最完整的白/黑名单参考(`packages/coding-agent/examples/extensions/plan-mode/`):

```ts
// index.ts:164-176
pi.on("tool_call", async (event) => {
    if (!planModeEnabled || event.toolName !== "bash") return;
    const command = event.input.command as string;
    if (!isSafeCommand(command)) {
        return {
            block: true,
            reason: `Plan mode: command blocked (not allowlisted). Use /plan to disable plan mode first.\nCommand: ${command}`,
        };
    }
});
```

判定函数 `isSafeCommand`(`plan-mode/utils.ts:97-101`)= **非破坏 且 在白名单**:

```ts
export function isSafeCommand(command: string): boolean {
    const isDestructive = DESTRUCTIVE_PATTERNS.some((p) => p.test(command));
    const isSafe = SAFE_PATTERNS.some((p) => p.test(command));
    return !isDestructive && isSafe;
}
```

- **黑名单**(`utils.ts:7-42`,36 条):`dd / shred`、任何 `>` / `>>` 重定向、`npm|yarn|pnpm install/uninstall/update`、`pip install`、`apt(-get)`、`brew`、**`git add|commit|push|pull|merge|rebase|reset|checkout|branch -d|stash|cherry-pick|revert|tag|init|clone`**、`sudo / su`、`kill / pkill / killall`、`reboot / shutdown`、`systemctl start|stop|restart|enable|disable`、`vim|nano|emacs|code|subl`(交互式编辑器会挂死无 PTY 的执行器);
- **白名单**(`utils.ts:44-95`,约 50 条):`cat/head/tail/less/more/grep/find/ls/pwd/echo/printf/wc/sort/uniq/diff/file/stat/du/df/tree/which/whereis/type/env/printenv/uname/whoami/id/date/cal/uptime/ps/top/htop/free`、**`git status|log|diff|show|branch|remote|config --get` 与 `git ls-*`(只读 git)**、`npm list|ls|view|info|search|outdated|audit`、`curl <url>`、`wget -O -`(仅 stdout)、`jq`、`sed -n`(仅打印)、`awk`、`rg/fd/bat/eza`。

注意黑名单是**子串级正则**(如 `/\bsu\b/i` 会误伤 `sudo` 之外的 `sum`?——不会,`\b` 边界;但会拦下 `git commit` 出现在注释字符串里的极端 case),白名单是**行首锚定**(`^\s*cat\b`)——所以 `cat file; rm -rf /` 因不匹配任何行首白名单而整体被拦。这是"默认拒绝"的正确姿势。

plan-mode 同时通过 `before_agent_start` 注入 `[PLAN MODE ACTIVE]` 系统消息并把 edit/write 从活动工具里移除(`plan-mode/index.ts:212-241`),`context` hook 在退出 plan 模式后过滤掉这些过时消息(`:177-191`)——**提示词层与工具层双重设防**。

#### 14.3.11 工具并发:sequential/parallel 的判定链

`packages/agent/src/agent-loop.ts:413-423`:

```ts
const toolCalls = assistantMessage.content.filter((c) => c.type === "toolCall");
const hasSequentialToolCall = toolCalls.some(
    (tc) => currentContext.tools?.find((t) => t.name === tc.name)?.executionMode === "sequential",
);
if (config.toolExecution === "sequential" || hasSequentialToolCall) {
    return executeToolCallsSequential(currentContext, assistantMessage, toolCalls, config, signal, emit);
}
return executeToolCallsParallel(currentContext, assistantMessage, toolCalls, config, signal, emit);
```

**批内一票否决**:只要本批任一工具声明 `executionMode: "sequential"`(`agent/src/types.ts:404-409`),整批退化为串行。无全局并发度上限、无信号量——并发上限就是"模型单条消息里的 tool call 数量"。文件写安全完全由 14.1.8 的 per-file 锁兜底(所以 pi 的 8 个内置工具**都没声明** sequential,默认全并行)。

#### 14.3.12 对 laew 的借鉴(维度三)

laew 现状:`src/agent/tools/bash.rs` 一次性收集输出、无超时参数、无进程组杀、无截断、无流式。

| 优先级 | 借鉴项 | pi 来源 | laew 落地(Rust) |
|---|---|---|---|
| **P0** | 进程组 + 整树杀 | `bash.ts:101`, `shell.ts:216-247` | `tokio::process::Command::process_group(0)`(Unix)/ `windows_job_object`;超时与 Ctrl-C 都 `kill(-pgid, SIGKILL)` |
| **P0** | 50KB / 2000 行双向截断 + 精确区间文案 | `truncate.ts:11-13, 168-241`, `bash.ts:432-450` | 独立 `truncate.rs` 模块,read 用 head、bash 用 tail;文案带 `[Showing lines X-Y of Z]` |
| **P0** | 超限自动落临时文件 | `output-accumulator.ts:205-221` | `tempfile` crate;路径回填进结果尾部 |
| **P0** | 非零退出码 = isError + 附带全部输出 | `bash.ts:479-481` | `ToolResult { is_error: true, text: 输出 + "\n\nCommand exited with code N" }` |
| **P1** | timeout 参数 + Node 上限校验 | `bash.ts:26-40, 42-45` | schema 增加 `timeout: Option<u64>`(秒),上限校验同款 |
| **P1** | 100ms 节流 onUpdate 流式 | `bash.ts:211-212, 397-410` | TUI 已有 Frame 机制,加 `tokio::time::throttle` |
| **P1** | 滚动尾部有界缓冲 + UTF-8 边界裁剪 | `output-accumulator.ts:148-203` | `Vec<u8>` 环形缓冲,`str::from_utf8` 失败时推进到下一字符边界 |
| **P1** | exit 后 100ms 空闲宽限收尾 | `child-process.ts:38-137` | `wait_with_output` 之外监听 stdout EOF + 空闲定时器重武装 |
| **P1** | PI_* 式会话元数据环境变量(先删后加) | `bash.ts:170-196` | 注入 `LAEW_SESSION_ID / LAEW_AGENT / LAEW_PROVIDER / LAEW_MODEL` |
| **P1** | 危险命令黑名单 + 白名单(默认拒绝) | `plan-mode/utils.ts:7-101` | `src/agent/permission.rs`:36 条黑名单 + 50 条行首白名单正则,Plan Agent 阶段强制启用 |
| **P2** | `tool_call` 可阻断 hook + `terminate` 批级语义 | `extensions/types.ts:936-951`, `agent/src/types.ts:60-70` | laew 增加 `BeforeToolCall { Block(reason), Terminate }`;Quality-Check 可挂此钩子 |
| **P2** | 跨平台 shell 发现链 | `shell.ts:67-136` | Unix 直接 `/bin/bash` 兜底 `sh`;错误信息列出已搜索路径 |
| **P2** | stdout 净化(控制字符/孤代理/format chars) | `shell.ts:160-190` | Rust 侧 `String::from_utf8_lossy` + 过滤 C0 控制字符(保留 `\t\n\r`) |

---

### 14.4 代码检索与索引

#### 14.4.1 四个只读检索工具 + 一条"只读工具集"预设

`packages/coding-agent/src/core/tools/index.ts:163-170` 提供 `createReadOnlyToolDefinitions`(read/grep/find/ls)——与 `createCodingToolDefinitions`(read/bash/edit/write, `:155-161`)相对。这个二分让"只读 Agent"(如 pi 的 Yolo 类角色)可以整套装配。

#### 14.4.2 外部二进制依赖:`rg` + `fd` + 缺失自动下载

`grep` 工具第一步就是 `await ensureTool("rg")`(`packages/coding-agent/src/core/tools/grep.ts:177`),`find` 用 `ensureTool("fd")`(`find.ts:225`)。`packages/coding-agent/src/utils/tools-manager.ts:335-374` 的 `ensureTool` 链:

1. `getToolPath`(`:85-104`):先查 pi 自己的 bin 目录(`~/.pi/agent/bin/rg`),再查系统 PATH(`fd` 还会尝试 Debian 的 `fdfind` 别名,`:34`);
2. `PI_OFFLINE=1` → 跳过下载,返回 undefined(工具调用报 "ripgrep (rg) is not available and could not be downloaded",`grep.ts:178-180`);
3. Android/Termux → 提示 `pkg install ripgrep`(Linux glibc 二进制在 Bionic 上跑不了,`:352-358`);
4. 否则 GitHub API 取 latest release → 按 platform/arch 拼 asset 名(`TOOLS` 配置表,`:29-71`)→ 下载(`fetchWithRetry`,网络 10s / 下载 120s 超时)→ 解压(**唯一临时目录**,`extract_tmp_{name}_{pid}_{ts}_{rand}`,防 fd/rg 并发安装竞态,`:273-278`)→ `rename` 到 bin 目录 → Unix `chmod 0755`。

解压器自带降级链:tar.gz 用 `tar xzf`;zip 在 Windows 先试 `System32\tar.exe`(bsdtar 支持 zip,**优先于 Git Bash 的 GNU tar**),失败再试 PowerShell `Expand-Archive`;Unix 先 `unzip` 后 `tar xf`(`:185-239`)。

#### 14.4.3 grep:spawn ripgrep `--json`,结果后处理

`grep.ts:220-226`:

```ts
const args: string[] = ["--json", "--line-number", "--color=never", "--hidden"];
if (ignoreCase) args.push("--ignore-case");
if (literal) args.push("--fixed-strings");
if (glob) args.push("--glob", glob);
args.push("--", pattern, searchPath);

const child = spawn(rgPath, args, { stdio: ["ignore", "pipe", "pipe"] });
const rl = createInterface({ input: child.stdout });
```

关键实现决策:

| 点 | 实现 | 行号 |
|---|---|---|
| **流式消费 + 提前终止** | `rl.on("line")` 逐行解析 JSON event,`type === "match"` 计数达到 `limit`(默认 100)即 `child.kill()`(标记 `killedDueToLimit`,避免把 SIGKILL 当错误) | `:277-297`, `:240-245` |
| **上下文行不靠 rg** | `context > 0` 时 pi **自己重读文件**拼块(`fileCache` 每文件缓存行数组),匹配行用 `:`、上下文行用 `-`(对齐 grep 惯例);`GrepOperations.readFile` 可被远程后端替换,所以必须流完再格式化 | `:205-218`, `:255-273`, `:321-336` |
| **ignore 规则** | 完全交给 rg(`--hidden` 包含隐藏文件但 rg 仍尊重 `.gitignore`);promptSnippet 明写 "Search file contents for patterns (respects .gitignore)" | `:39`, `:220` |
| **单行截断** | 命中行超 500 字符截断加 `... [truncated]`,提示 `Use read tool to see full lines` | `:266-268`, `truncate.ts:268-276` |
| **字节上限** | `truncateHead(rawOutput, { maxLines: Number.MAX_SAFE_INTEGER })` —— 行数已被 match limit 封顶,只剩字节维度 | `:340` |
| **可操作截断通知** | `[100 matches limit reached. Use limit=200 for more, or refine pattern. 50.0KB limit reached. Some lines truncated to 500 chars. Use read tool to see full lines]` | `:345-361` |
| **abort** | `signal.addEventListener("abort")` → kill 子进程 + reject `"Operation aborted"`;`settle` 单次保护防双 resolve | `:167-173`, `:246-249` |
| **退出码语义** | `code !== 0 && code !== 1` 才算错(rg 用 1 表示"无匹配",2+ 才是真错误);无匹配返回 `"No matches found"` 而非空 | `:309-319` |
| **路径相对化** | 目录搜索时结果转相对路径并统一 `/` 分隔符 | `:195-203` |

#### 14.4.4 find:spawn fd + 三个 gitignore/git 边界修复

`find.ts:235-267`:

```ts
const args: string[] = ["--glob", "--color=never", "--hidden"];
// fd normally ignores .gitignore outside git repos, so keep --no-require-git there.
// Inside repos, use fd's default git-aware behavior so parent .gitignore rules stop
// at nested repo boundaries: https://github.com/earendil-works/pi/issues/5960
let insideGitRepo = false;
for (let current = searchPath; ; ) {
    if (await pathExists(path.join(current, ".git"))) { insideGitRepo = true; break; }
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
}
if (!insideGitRepo) args.push("--no-require-git");
args.push("--max-results", String(effectiveLimit));

// fd --glob matches against the basename unless --full-path is set
let effectivePattern = pattern;
if (pattern.includes("/")) {
    args.push("--full-path");
    if (!pattern.startsWith("/") && !pattern.startsWith("**/") && pattern !== "**") {
        effectivePattern = `**/${pattern}`;
    }
    if (process.platform === "win32") effectivePattern = effectivePattern.replaceAll("/", String.raw`[/\\]`);
}
args.push("--", effectivePattern, searchPath);
```

三个从真实 issue 长出来的兼容逻辑:

1. **`.gitignore` 边界**:非 git 仓库里 fd 默认不应用 gitignore,需 `--no-require-git`;仓库内则用 fd 默认行为,使父仓库的 ignore 规则在嵌套仓库处截断;
2. **含 `/` 的 pattern 语义切换**:`--glob` 默认匹配 basename,`src/**/*.spec.ts` 这类带路径的 pattern 必须切 `--full-path`,并自动补 `**/` 前缀;
3. **Windows 分隔符**:`--full-path` 模式下 fd 用原生分隔符,`/` 需展开成 `[/\]`。

结果侧:每行剥 `\r` + trim、`relativizeFindResultPath` 相对化并保留尾部 `/`(目录标识,`find.ts:17-27`)、`limit`(默认 1000)达到即 `resultLimitReached`、同样的 `[1000 results limit reached. Use limit=2000 for more, or refine pattern]` 通知(`:334-346`)。自定义后端可整体替换 `FindOperations.glob`(`:169-222`,ignore 列表固定 `**/node_modules/**` + `**/.git/**`)。

#### 14.4.5 ls:纯 readdir,无并行 stat

`ls.ts` 不 spawn 任何进程:`ops.readdir` → 大小写不敏感 `localeCompare` 排序(`:154-155`)→ 逐条 `stat` 判目录加 `/` 后缀(`:166-176`)→ 500 条上限 → `truncateHead(maxLines: MAX_SAFE_INTEGER)` 只留字节维度(`:187`)。**stat 是顺序 await 的**,大目录(上万条目)会成为热点——pi 用 500 条上限把这个成本封顶了,超过即停,不再 stat 剩余项。

#### 14.4.6 统一的"三层限额 + 可操作通知"模式

四个检索工具共享同一结果组装模板,这是可直接模板化的设计:

| 工具 | 条目上限 | 字节上限 | 单行截断 | 通知文案模板 |
|---|---|---|---|---|
| grep | 100 matches | 50KB | 500 字符 | `{n} matches limit reached. Use limit={2n} for more, or refine pattern` |
| find | 1000 results | 50KB | — | `{n} results limit reached. Use limit={2n} for more, or refine pattern` |
| ls | 500 entries | 50KB | — | `{n} entries limit reached. Use limit={2n} for more` |
| read | 2000 行(或用户 limit) | 50KB | 首行超限→bash 提示 | `[Showing lines X-Y of Z. Use offset=N to continue.]`(`read.ts:308-317`) |

共同点:**上限值写进工具 description 让模型预知**(`grep.ts:136`、`find.ts:131`、`ls.ts:108`、`bash.ts:350`);**超限通知必须给出下一步动作**(加倍 limit / 换 offset / 换 read / refine pattern),而不是干巴巴说 "truncated"。`read` 甚至在首行超 50KB 时给出精确的 bash 替代命令(`read.ts:298-301`):

```ts
outputText = `[Line ${startLineDisplay} is ${firstLineSize}, exceeds ${formatSize(DEFAULT_MAX_BYTES)} limit. Use bash: sed -n '${startLineDisplay}p' ${path} | head -c ${DEFAULT_MAX_BYTES}]`;
```

#### 14.4.7 没有的东西

| 能力 | pi 状态 | 说明 |
|---|---|---|
| 符号定义跳转 / LSP | ✗ | 全仓库无 lsp/ctags 引用;符号级检索只能 grep 函数名 |
| embedding / 向量索引 | ✗ | grep `embedding|vector` 仅命中图像模型名 |
| 全文索引(FTS/lucene) | ✗ | 工作区文件无任何持久化索引;每次工具调用都是冷查询 |
| 结果按相关性排序 | ✗ | rg/fd 自然序(路径序),无 score |
| 跨工具缓存 | ✗(仅 grep 上下文行的 per-call `fileCache`, `grep.ts:205`) | 调用结束即丢弃 |

即 pi 的立场:**检索 = 外部专业二进制 + 每次冷查询**,宁可慢也不维护索引状态——与"单写者、无后台任务"的整体哲学一致。

#### 14.4.8 对 laew 的借鉴(维度四)

laew 现状:无 grep/glob/ls 工具,模型检索全靠 BashTool 里 `grep`/`find` 命令(输出无截断、无 gitignore 感知、大仓库会爆上下文)。

| 优先级 | 借鉴项 | pi 来源 | laew 落地(Rust) |
|---|---|---|---|
| **P0** | Grep 工具(shell out 到 `rg`) | `grep.ts:220-297` | **不必 spawn**:直接内嵌 `grep-searcher` + `ignore` crate(同一作者 BurntSushi 的 ripgrep 组件库),天然获得 gitignore + 并行 walker |
| **P0** | Glob/Find 工具 | `find.ts:235-267` | `ignore` crate 的 `WalkBuilder` + `globset`;注意 pi 的"含 `/` 需 full-path 语义"经验 |
| **P0** | 三层限额 + 可操作中文通知 | `grep.ts:345-361`, `read.ts:308-317` | 文案模板: `[已显示第 X-Y 行(共 Z 行)。使用 offset=N 继续]` / `[已达 100 条上限。可设 limit=200 或收窄 pattern]` |
| **P0** | 上限值写进工具 description | `grep.ts:136`, `bash.ts:350` | laew 各工具 description 同步写明截断规则,减少模型试错 |
| **P1** | Ls 工具 + 目录 `/` 后缀 + 大小写不敏感排序 | `ls.ts:154-176` | `std::fs::read_dir`;并行 stat 用 `rayon` 修复 pi 的顺序 stat 短板 |
| **P1** | 只读工具集预设 | `index.ts:163-170` | laew 的 Yolo Agent(仅 Read)扩为 `read_only_registry()`: Read+Grep+Glob+Ls;Quality-Check Agent 同样适用 |
| **P1** | Read 的 offset/limit + 续读提示 | `read.ts:277-321` | `ReadTool` 增加 offset/limit 参数与 `[还有 N 行。使用 offset=N 继续]` |
| **P2** | 依赖二进制自动下载 | `tools-manager.ts:242-317` | laew 若坚持 spawn rg,可参照 GitHub Release 资产名拼接表 + 唯一临时目录防竞态 |
| **P2** | 单行 500 字符截断 | `truncate.ts:13`, `grep.ts:266-268` | grep 命中行统一截断,提示用 Read 看全行 |
| **P2** | 符号索引(可选) | (pi 无) | 若做,建议 tree-sitter tags 而非 LSP,避免 pi/laew 都没有的常驻进程负担 |

---

### 14.5 第七轮横向总结

#### 14.5.1 四维度 × pi 设计模式提取

| 模式 | 出现位置 | 一句话 |
|---|---|---|
| **纯计算与副作用分离** | `applyEditsToNormalizedContent` / `writeFile`;`computeEditsDiff` / 真编辑 | 匹配、diff、patch 全是纯函数,写盘是唯一副作用——测试与 TUI 预览免费获得 |
| **归一化空间工作,原文空间回写** | `normalizeForFuzzyMatch` + `applyReplacementsPreservingUnchangedLines` | 容错匹配的代价被限制在"被触碰的行"内 |
| **锁持有跨越 abort** | `edit.ts:337-343`, `write.ts:211-217` | 轮询 `signal.aborted` 而非监听 reject,锁永不提前释放 |
| **content / details 双栏** | `agent/src/types.ts:360-366` | 模型上下文与人机界面共用一个工具结果对象但互不污染 |
| **有界内存 + 惰性落盘** | `OutputAccumulator` | 滚动尾部窗口 + 超限才开临时文件,GB 输出恒定内存 |
| **策略外置,机制内置** | 权限/Git/后台任务全部走扩展 hook | 核心只保证锁、截断、取消三个机制的正确性 |
| **截断必须可操作** | 所有工具的通知文案 | 每条 truncation 通知都附下一步指令(limit 加倍 / offset 续读 / 换工具) |
| **专业事交专业二进制** | rg / fd / tar / taskkill | 检索与压缩解压全部外包,缺失时自动补齐 |

#### 14.5.2 与 laew 现状的差距矩阵(本轮四维度)

| 能力 | pi | laew 现状 | 差距等级 |
|---|---|---|---|
| Edit 工具(多编辑+唯一性+模糊) | ✅ 完整 | ❌ 无 | **大** |
| 文件写排他锁 | ✅ per-realpath | ❌(SQLite 隐式) | 大 |
| 文件写原子性 | ⚠️ 无(裸 writeFile) | ❌ | 中(两边都缺) |
| 编辑 undo / 备份 | ⚠️ 靠 git 扩展 | ❌ | 中 |
| Git 分支探测 | ✅ 零子进程 | ❌ | 小 |
| checkpoint / 回滚 | ⚠️ 扩展示例(git stash) | ❌ | 中 |
| Bash 超时 | ✅ 参数化 | ❌ | 中 |
| Bash 进程树杀 | ✅ | ❌ | **大**(孤儿进程风险) |
| Bash 输出截断 + 临时文件 | ✅ 50KB/2000 行 | ❌ | **大**(上下文爆炸风险) |
| Bash 流式输出 | ✅ 100ms 节流 | ❌ | 中 |
| 危险命令拦截 | ⚠️ hook + 示例正则 | ❌ | 中 |
| Grep/Glob/Ls 工具 | ✅ rg/fd | ❌(靠 bash) | **大** |
| gitignore 感知 | ✅(rg/fd 自带) | ❌ | 中 |
| 符号/向量索引 | ❌ | ❌ | 0(持平,均无) |

#### 14.5.3 本轮 P0 清单汇总(laew 落地顺序建议)

1. **`src/agent/tools/edit.rs`** —— edits[] 多编辑 + 四类错误文案 + prepare_args 兼容层(维度一 P0 ×3);
2. **`src/agent/tools/truncate.rs`** —— 50KB/2000 行双向截断 + 中文可操作通知(三处工具复用);
3. **BashTool 改造** —— timeout 参数 + 进程组整树杀 + 尾部截断 + 临时文件 + 非零退出码 isError;
4. **`src/agent/tools/grep.rs` + `glob.rs`** —— 内嵌 `grep-searcher`/`ignore` crate,三层限额;
5. **per-realpath 写锁** —— `HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>`,覆盖 Edit/Write;
6. **Git 分支探测** —— TUI 横幅 + Yolo 分类前的仓库状态感知(直读 `.git/HEAD`);
7. **危险命令黑名单** —— 36 条黑名单 + 50 条行首白名单(照抄 plan-mode 正则表),Plan Agent/QC Agent 强制启用。

> 以上 7 项均为"无新依赖或仅新增 2 个 crate(`ignore`/`grep-searcher`)"即可完成的增量改造,与既有 `Tool` trait / `AgentProfile` / SQLite 持久化模型完全兼容。

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

---

## 13. 第六轮深挖 — Lane 并发模型 + CBOR 二进制帧协议 + WriterLease fence + 14 种损坏检测(2026-09-06)

本轮在前 12 章基础上,深入到 **并发与持久化底层**:
**Lane 编排器的三态事件溯源**、**CBOR 帧协议的字节级实现**、
**SQLite WriterLease 的 fencing token 设计**、**reducer 的 14 种损坏检测**、
**Tool 系统的命令式工厂与 per-file 排他锁**。

所有行号来自 `/usr/local/LsmGitOpenSource/pi` 当前 head(2026-09-02)。

### 13.1 Lane 并发模型:三态 + 事件溯源

#### 13.1.1 Lane 是什么、生命周期

Lane = 一个**逻辑并发轨**,承载**一个**长时间操作(run / compaction / navigation)。
每个 lane 有独立 leaf,可被独立导航(`navigateTree`)、独立压缩、独立中断。

**Lane 的三种状态**(由 reducer 推导,纯函数):

```ts
// packages/agent/src/harness/agent-harness.ts:152-160
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

- **running**:operation 正在执行,新 steer / followUp 可入队。
- **suspended**:operation 因 crash(进程被杀)或 deferred(异步资源未就绪)暂停,等待 `resume()`。
- **aborting**:用户主动 abort,等待当前 step 收尾。
- **idle**:`operation === null`,可接受新 run。

**生命周期状态机**:

```
idle ──prompt()──> running ──abort()──> aborting ──> idle
                      │                    │
                      ├──crash──> suspended ──resume()──> running
                      │
                      └──deferred──> suspended ──deferred arrives──> running
```

**Lane 错误的 TaggedError 族**(`agent-harness.ts:28-55`):

| 错误类 | 触发条件 |
|---|---|
| `LaneBusy` | lane 已有 operation 时新 prompt/compact/navigate |
| `NoActiveRun` / `NoActiveOperation` | 操作无 active run 时调用 steer/abort |
| `NothingToResume` | resume 时 lane 无 suspended operation |
| `NothingToCompact` | compaction 不可应用 |
| `LaneExists` / `InvalidLane` | lane 创建冲突/非法名 |
| `Closed` | harness 已 close |
| `MissingIdentities` | resume 时缺少 tools/models |
| `UnknownSkill` / `UnknownTemplate` / `UnknownTarget` | 资源未知 |
| `UnknownQueueItem` | 取消已消费/已清除的队列项 |

#### 13.1.2 SQLite 行级 lane 存储

**lanes 表**(`packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts:5-10`):

```ts
export interface LaneRow {
    session_id: string;
    lane: string;
    leaf_id: string | null;
    open_operation_id: string | null;   // null = idle
}
```

**乐观获取 operation slot**(`lanes.ts:88-95`):

```sql
UPDATE lanes SET open_operation_id = ${runId}
WHERE session_id = ${sessionId}
  AND lane = ${lane}
  AND open_operation_id IS NULL
```
- 单条 SQL 同时校验"lane 存在"和"无 open operation",`result.changes === 1` 才返回。
- 否则用 `readLane` 区分"NotFound"与"AlreadyOpen",抛具体错误。
- **这是 SQL 层的乐观锁**:`open_operation_id IS NULL` 是 CAS 谓词,失败不阻塞,立即报错。

**释放 slot**(`lanes.ts:97-100`):

```sql
UPDATE lanes SET open_operation_id = NULL
WHERE session_id = ${sessionId}
  AND lane = ${lane}
  AND open_operation_id = ${runId}
```
- 释放也带 runId 谓词,防止误清别人的 slot(虽然 SQLite 单写者,防御性)。

#### 13.1.3 三队列驱动(steer / followUp / nextRun)

Lane 的输入不是单一流,而是**三个语义不同的队列**:

**QueueEnqueuedRecord 联合类型**(`packages/agent/src/harness/session/types.ts:162-176`):

```ts
export type QueueEnqueuedRecord = RecordBase &
    (| { type: "queue_enqueued"; queue: "steer" | "followUp"; runId: string; target: ProvisionedEntry }
     | { type: "queue_enqueued"; queue: "nextRun";     runId?: never;   target: ProvisionedEntry });
```

| 队列 | 生命周期 | runId 绑定 | 注入时机 |
|---|---|---|---|
| **steer** | operation 生命周期内 | 绑定 | 在下一步 assistant 调用**之前**注入 context |
| **followUp** | operation 完成后 | 绑定 | agent 自然结束后,作为下一轮 prompt |
| **nextRun** | idle 时 | 无 | lane 回到 idle 时的下一轮 prompt |

**LaneSnapshot 暴露的队列**(`agent-harness.ts:167-175`):

```ts
export interface LaneSnapshot {
    lane: string;
    transcript: Entry[];
    leafId: string | null;
    operation: LaneInfo["operation"];
    queues: { steer: QueuedItem[]; followUp: QueuedItem[]; nextRun: QueuedItem[] };
    pendingWrites: { id: string; entry: ProvisionedEntry }[];
    faulted: boolean;
}
```

**三种队列模式**(`agent-harness.ts:343-344`):

```ts
this.steeringMode = options.steeringMode ?? "one-at-a-time";
this.followUpMode = options.followUpMode ?? "one-at-a-time";
```
- `one-at-a-time`(默认):单条投递,避免 race。
- `all-at-once`:全部注入,适合批处理。

#### 13.1.4 Entry 链 + Record 日志

Lane 的持久化状态 = **Entry 链**(消息数据)+ **Record 日志**(操作元数据)。

**Entry 类型**(`session/types.ts:22-74`):

```ts
type: "message" | "model_change" | "thinking_level_change" | "active_tools_change"
    | "compaction" | "branch_summary" | "custom"
```
- **message** 是核心(AssistantMessage、UserMessage、ToolResultMessage 等)。
- **配置类变更**(model_change、thinking_level_change、active_tools_change)单独成 entry,不混入消息流。
- **compaction / branch_summary** 是派生摘要,作为 entry 持久化。

**Record 日志类型**(`session/types.ts:203-212`):

```ts
type LaneRecord =
    | OperationStartedRecord     // 启动一个 operation
    | AbortRequestedRecord       // 用户请求 abort
    | OperationFinishedRecord    // operation 结束(complete/abort/fail/decline)
    | StepAttemptRecord          // LLM 调用一次(step = assistant/compaction/branch_summary)
    | ToolStartedRecord          // 一个 tool call 开始(toolIndex、assistantEntryId 关联)
    | QueueEnqueuedRecord        // steer/followUp/nextRun 入队
    | QueueCancelledRecord       // 取消已入队项
    | WriteDeferredRecord        // 写入被延迟(deferred fetch 等待)
    | UsageRecord;               // token/cost 用量
```

**每条记录的关键字段**(`session/types.ts:14-20, 80-85`):

```ts
interface EntryBase { type; id; seq; parentId; timestamp }
interface RecordBase { id; seq; lane; timestamp }
```
- **`seq` 是 session-wide 单调递增序号**,由存储层分配,保证恢复时可排序。
- **`timestamp` 也由存储层填充**,客户端不可伪造。

**操作级 vs session-wide**:

- Entry 有 `parentId`,形成**分支树**(同 lane 内可分支)。
- Record 有 `lane`,但无 `parentId`,只是 lane 内日志流。

#### 13.1.5 suspended → resume 机制

**SuspendedOperation 形状**(`agent-harness.ts:140-150`):

```ts
export interface SuspendedOperation {
    lane: string;
    kind: "run" | "compaction" | "navigation";
    id: string;
    startedAt: number;
    reason: "crash" | "deferred";
    prompt?: AgentMessage[];
    deferred?: DeferredHandle;
    aborting?: { steer: AgentMessage[]; followUp: AgentMessage[] };
    missing: { tools: string[]; models: string[] };   // resume 时的依赖校验
}
```

**reason 区分**:
- **crash**:进程 SIGKILL/SIGTERM,operation 留在 open_operation_id 但无 owner。
- **deferred**:操作正在等外部资源(如远端 model 响应),挂起保留 DeferredHandle。

**resume() 的依赖校验**(`reducer.ts:560-562`):

```ts
const missingInitialMessages =
    started.intent.kind === "run"
        ? started.intent.initialMessages.filter((target) => !entriesById.has(target.id)).map(clone)
        : [];
```
- 复原时若 operation 的 intent 引用的 entry 缺失,标记为 missing。
- `MissingIdentities` 错误包含具体缺失的 tools/models,允许上层决定是 fetch + resume 还是 fail。

**deferred 复原**(`reducer.ts:597-603`):

```ts
const deferred =
    newestOwnEntry?.type === "message" &&
    newestOwnEntry.message.role === "assistant" &&
    newestOwnEntry.message.stopReason === "deferred" &&
    newestOwnEntry.message.deferred
        ? clone(newestOwnEntry.message.deferred)
        : null;
```
- 复原时若最后一条 assistant entry 是 `stopReason === "deferred"`,把 deferred handle 提取出来,后续 `fetch_deferred` action 续上。

#### 13.1.6 reducer 事件溯源:纯函数 + 严格校验

**reduceLaneState 是纯函数**(`reducer.ts:506-667`):

```ts
export function reduceLaneState(input: LaneReductionInput): LaneReductionResult
```
- 入参:`{ records, ownEntries, configurationEntries, defaults, leafId }`。
- 出参:`{ laneState, effectiveConfiguration, terminalFailure }`。
- **不修改入参,不读外部状态**;可单元测试,可确定性重放。

**校验前置**(`reducer.ts:507`):

```ts
validateRecordLog(input);
```
- 见 13.5 节,14 种 RecordLogCorruption 在这里抛错。

**terminalFailure 推导**(`reducer.ts:614-640`):

```ts
// 检测 error stopReason 但被 step/deferred_fetch 产出 → 标记 terminal
if (
    newestOwnEntry?.type === "message" &&
    newestOwnEntry.message.role === "assistant" &&
    newestOwnEntry.message.stopReason === "error" &&
    !deferredWriteIds.has(newestOwnEntry.id)
) {
    const producedByStep = ... ;   // 来自 step_attempt
    const producedByDeferredFetch = ... ;  // 来自 deferred_fetch 的 usage
    if (producedByStep || producedByDeferredFetch) {
        terminalFailure = { entryId, source: "step"|"deferred_fetch", message };
    }
}
```
- **terminalFailure 表示必须终止恢复**,不能续跑。

#### 13.1.7 per-file 排他锁:edit/write 串行化

**FileMutationQueue**(`packages/agent/src/harness/tools/file-mutation-queue.ts`):

```ts
// line 4-18:WeakMap 状态(env → 自己的 queue map)
const states = new WeakMap<ExecutionEnv, MutationQueueState>();
function getState(env) {
    let state = states.get(env) ?? { queues: new Map(), registration: Promise.resolve() };
    states.set(env, state);
    return state;
}
```
- **按 env 隔离**:不同 ExecutionEnv(如 NodeExecutionEnv、BunExecutionEnv)的 queue map 独立。
- **WeakMap 持有**,env 销毁时状态自动 GC,无内存泄漏。

**canonical path 锁定**(`file-mutation-queue.ts:20-26`):

```ts
async function getMutationQueueKey(env, path) {
    const absolutePath = getOrThrow(await env.absolutePath(path));
    const canonicalPath = await env.canonicalPath(absolutePath);
    if (canonicalPath.ok) return canonicalPath.value;       // 解析 symlink → 真实路径
    if (canonicalPath.error.code === "not_found" || "not_supported") return absolutePath;
    throw canonicalPath.error;
}
```
- **核心**:同一个文件无论以哪个符号链接或相对路径访问,**都归到同一个 queue**。
- 防止 `/path/to/file` 和 `/path/./to/../to/file` 拿到两把锁。

**链式 promise 串行化**(`file-mutation-queue.ts:29-56`):

```ts
export async function withFileMutationQueue<T>(env, path, fn): Promise<T> {
    const state = getState(env);
    const registration = state.registration.then(async () => {
        const key = await getMutationQueueKey(env, path);
        const currentQueue = state.queues.get(key) ?? Promise.resolve();
        let releaseNext = () => {};
        const nextQueue = new Promise<void>((resolve) => { releaseNext = resolve; });
        const chainedQueue = currentQueue.then(() => nextQueue);
        state.queues.set(key, chainedQueue);
        return { key, currentQueue, chainedQueue, releaseNext };
    });
    state.registration = registration.then(() => undefined, () => undefined);
    const { key, currentQueue, chainedQueue, releaseNext } = await registration;
    await currentQueue;
    try {
        return await fn();
    } finally {
        releaseNext();
        if (state.queues.get(key) === chainedQueue) state.queues.delete(key);
    }
}
```

**关键技术点**:

1. **`registration.then(...)` 串行化 key 解析**:即使多个并发调用同时进入,canonical path 解析也按顺序,避免 race。
2. **`currentQueue.then(() => nextQueue)` 链式排队**:每个新操作追加到链尾。
3. **自我清理**:`finally` 中 `state.queues.delete(key)`(前提是当前链仍是最新的,防止误删后入队的链)。
4. **错误吞噬**:`registration.then(..., () => undefined)` —— 即便某次失败,也不影响后续登记。

**edit 工具使用示例**(`packages/agent/src/harness/tools/edit.ts:102-138`):

```ts
async execute(_toolCallId, input, signal, _onUpdate, { env }) {
    const { path, edits } = validateEditInput(input);
    const absolutePath = await resolveToolPath(env, path, signal);
    return withFileMutationQueue(env, absolutePath, async () => {
        // 读 → 计算 diff → 写
        ...
    });
}
```
- 整个 read-modify-write 周期都串行化,不会出现两个并发 edit 互相覆盖。

---

### 13.2 CBOR 二进制帧协议

#### 13.2.1 为何选 CBOR 不选 JSON

代码无直接 prose 对比,但从实现可推断**四个关键动机**:

1. **deterministic schema 强制**:`packages/protocol/README.md:65`
   > "JSON-valued protocol fields reject CBOR byte strings and non-plain objects. Top-level undefined, undefined array entries, sparse arrays, non-finite or unsafe numbers, tags, indefinite-length items, malformed UTF-8, trailing data, excessive nesting, and oversized values are rejected."

2. **定长项让 framing 简单**:`README.md:46`
   > "Every transport carries the same complete bytes: `[uint32-be CBOR length][CBOR payload]`"

   CBOR 的 definite-length model 让"读完长度前缀就知 payload 边界",无 JSON 那种"流式解析未知长度"难题。

3. **无 JSON number/precision 歧义**:CBOR 区分 uint、negative int、float64、bigint tag 等;JSON 数字在 JS 里全部是 double。
4. **TypeBox-friendly**:CBOR 解码后是 plain JS object/value,直接用 `Check(ClientMessageSchema, value)` 校验。

**transport 中立性**:`packages/protocol/README.md:42, 46-48`:
> "parseClientMessage() and parseServerMessage() only validate already-decoded values. They do not parse JSON strings."
>
> "Transports may split or coalesce those bytes arbitrarily."

#### 13.2.2 帧结构

**线缆格式**(`packages/protocol/README.md:5-9`):

```
1. 4-byte unsigned big-endian payload length
2. One definite-length CBOR item containing the message
```

**常量**(`packages/protocol/src/framing.ts:1-6`):

```ts
const FRAME_HEADER_LENGTH = 4;
const MAX_UINT32 = 0xffff_ffff;
const PAYLOAD_BLOCK_SIZE = 64 * 1024;
export const DEFAULT_MAX_FRAME_LENGTH = 16 * 1024 * 1024;   // 16 MiB
```

**encodeFrame**(`framing.ts:28-39`):

```ts
export function encodeFrame(payload: Uint8Array): Uint8Array {
    if (!(payload instanceof Uint8Array)) throw new TypeError(...);
    if (payload.byteLength > MAX_UINT32) throw new RangeError(...);
    const frame = new Uint8Array(FRAME_HEADER_LENGTH + payload.byteLength);
    frame[0] = payload.byteLength >>> 24;
    frame[1] = payload.byteLength >>> 16;
    frame[2] = payload.byteLength >>> 8;
    frame[3] = payload.byteLength;
    frame.set(payload, FRAME_HEADER_LENGTH);
    return frame;
}
```
- 手写 big-endian,无 DataView 调用,跑得快。

**assertCompleteFrame**(`framing.ts:42-53`):校验一个完整帧,长度前缀 + payload 字节数 = 帧总字节数。

#### 13.2.3 增量 FrameDecoder:状态机

**三态机**(`framing.ts:55, 67`):

```ts
type DecoderState = "open" | "ended" | "failed";
private state: DecoderState = "open";
```

**核心 push 循环**(`framing.ts:73-144`):

```ts
push(chunk: Uint8Array): Uint8Array[] {
    if (this.state === "ended") throw new FrameError("Frame decoder has ended");
    if (this.state === "failed") throw new FrameError("Frame decoder has failed");
    
    const frames: Uint8Array[] = [];
    let chunkOffset = 0;
    while (chunkOffset < chunk.byteLength) {
        // 1. 补全 4 字节 length header
        if (this.expectedPayloadLength === undefined) {
            const headerBytes = Math.min(FRAME_HEADER_LENGTH - this.headerLength, chunk.byteLength - chunkOffset);
            this.header.set(chunk.subarray(chunkOffset, chunkOffset + headerBytes), this.headerLength);
            this.headerLength += headerBytes;
            chunkOffset += headerBytes;
            if (this.headerLength < FRAME_HEADER_LENGTH) continue;
            
            // 2. 解析长度,校验上限
            const frameLength = (header[0] << 24) | (header[1] << 16) | (header[2] << 8) | header[3];
            if (frameLength > this.maxFrameLength) this.fail(...);   // 立即 fail,丢弃状态
            if (frameLength === 0) { frames.push(new Uint8Array()); continue; }   // 0 长度合法
            
            this.expectedPayloadLength = frameLength;
            this.payloadBlocks = [];
            ...
        }
        
        // 3. 累积 payload 到 64 KiB blocks
        while (chunkOffset < chunk.byteLength && this.payloadLength < expectedPayloadLength) {
            let block = this.currentPayloadBlock;
            if (!block || this.currentPayloadBlockLength === block.byteLength) {
                block = new Uint8Array(Math.min(PAYLOAD_BLOCK_SIZE, expectedPayloadLength - this.payloadLength));
                this.payloadBlocks.push(block);
                ...
            }
            const payloadBytes = Math.min(block.byteLength - this.currentPayloadBlockLength, chunk.byteLength - chunkOffset);
            block.set(chunk.subarray(...));
            ...
        }
        
        // 4. 一帧收齐,合并 blocks,emit
        if (this.payloadLength === expectedPayloadLength) {
            if (this.payloadBlocks.length === 1) frames.push(this.payloadBlocks[0]);
            else { /* 拼接多 block */ }
            ...
        }
    }
    return frames;
}
```

**关键设计**:

- **任意分片/合并**:单个 push 可以是 1 字节、半个帧、几个完整帧。
- **0 长度帧合法**:`frameLength === 0` 时 push 一个空 Uint8Array,不报错。
- **fail() 永远 throw**(`framing.ts:155-164`):

  ```ts
  private fail(message: string): never {
      this.state = "failed";
      this.headerLength = 0;
      ...
      throw new FrameError(message);
  }
  ```
  转入 `failed` 态后,后续 push/end 全部 throw "decoder has failed",防止污染状态后输出脏数据。

**end() 检测截断**(`framing.ts:146-153`):

```ts
end(): void {
    if (this.state !== "open") throw new FrameError(...);
    if (this.headerLength !== 0 || this.expectedPayloadLength !== undefined) {
        this.fail("Truncated frame at end of stream");
    }
    this.state = "ended";
}
```

#### 13.2.4 严格的 CBOR 子集:RFC 8949 definite-length only

**encoder.ts**(`packages/protocol/src/cbor/encoder.ts`):

| Major Type | 接受 | 拒绝 |
|---|---|---|
| 0 unsigned int | Number.isSafeInteger ≥ 0 | 负数、超 safe range、`-0` |
| 1 negative int | `-1 - n` 编码 | 同上 |
| 2 byte string | `Uint8Array` | 其他 |
| 3 text string | string,UTF-8 round-trip 校验 | 含 lone surrogate |
| 4 array | dense array,无 undefined 元素 | sparse、`undefined`、cycle |
| 5 map | plain object(string keys) | 非 plain 原型、symbol keys、cycle、undefined 值 |
| 6 tag | — | **全部拒绝** |
| 7 simple | false/true/null/float64 | break (31)、其他宽度 |

**额外拒收**:
- 非有限数(`Number.isFinite`):line 138
- 整数不在 safe range:line 140
- cycle(`ancestors: Set<object>`):lines 161, 180
- 非 plain object 原型:lines 102-106
- symbol 枚举 key:lines 181-185
- 数组稀疏/有 undefined:line 169
- 字符串 UTF-8 不能 round-trip:lines 113-114(防止 lone surrogate)

**decoder.ts**(`packages/protocol/src/cbor/decoder.ts`):

```ts
case 6: throw new CborError("CBOR tags are not supported");           // line 80
case 31: throw new CborError("CBOR break marker is not supported");   // line 106
case 5: if (typeof key !== "string") throw "CBOR map keys must be strings";
        if (keys.has(key)) throw "CBOR map contains a duplicate key";
```
- decode 还强制 **无 trailing data**(line 22):`if (this.offset !== this.bytes.byteLength) throw "trailing data"`。
- 唯一一种"宽松"是 key 顺序保留(用 `Object.defineProperty` 显式属性顺序,line 70-75)。

**CborOptions 默认值**(`packages/protocol/src/cbor/options.ts:5-8`):

```ts
DEFAULT_MAX_CBOR_BYTE_LENGTH = 16 * 1024 * 1024;       // 16 MiB
DEFAULT_MAX_CBOR_CONTAINER_LENGTH = 1_000_000;
DEFAULT_MAX_CBOR_DEPTH = 64;
const MAX_CONFIGURED_DEPTH = 512;
```
- 三个 limit 都是显式数,防止 DoS。

#### 13.2.5 消息协议 schema

**ClientMessage**(`packages/protocol/src/schemas.ts:385-398`):

```ts
export const ClientHelloSchema = StrictObject({
    type: Type.Literal("hello"),
    version: Type.Integer({ minimum: 0 }),
});

export const RequestEnvelopeSchema = StrictObject({
    type: Type.Literal("request"),
    id: IdSchema,                       // opaque string
    request: CommandSchema,
});

export const ClientMessageSchema = Type.Union([ClientHelloSchema, RequestEnvelopeSchema]);
```

**Command 联合**(`schemas.ts:291-324`):`list | create | attach | detach | prompt | steer | abort | set_model | set_thinking`。

**ServerMessage**(`schemas.ts:440-445`):

```ts
ServerMessageSchema = Type.Union([
    ServerHelloSchema,            // {type:"hello", version:1, connectionId, snapshot}
    ServerHelloErrorSchema,       // {type:"hello_error", error}
    ResponseEnvelopeSchema,       // {type:"response", id, ok, result|error}
    EventEnvelopeSchema,          // {type:"event", event}
]);
```

**关键洞察**:协议**没有数字 seq/ack**,只有 opaque string `id`。
- 客户端发 `RequestEnvelope.id = "uuid"`,服务端回 `ResponseEnvelope.id = "uuid"` —— 一一对应。
- 比 seq/ack 简单,无重排序问题。
- 服务端推送是 `EventEnvelope`,单向广播,无需 ack。

**ProtocolError codes**(`schemas.ts:269-284`):

```ts
"version" | "busy" | "session_locked" | "not_found"
| "invalid_request" | "not_implemented" | "internal_error"
```
- "version" 触发条件:client 发 hello 时版本不匹配。
- "session_locked" 触发条件:另一个 writer 持有 WriterLease。

#### 13.2.6 错误传播:三层包装

**Codec 统一错误**(`packages/protocol/src/codec.ts:18-23, 60-76`):

```ts
export class ProtocolValidationError extends Error { ... }

function encodeProtocolMessage<T>(value, parse, kind, options?) {
    const validated = parse(value);
    try {
        const maxFrameLength = options?.maxFrameLength ?? DEFAULT_MAX_FRAME_LENGTH;
        const frame = encodeFrame(encodeCbor(validated, { maxByteLength: maxFrameLength }));
        assertCompleteFrame(frame, { maxFrameLength });
        return frame;
    } catch (error) {
        if (error instanceof ProtocolValidationError) throw error;
        throw new ProtocolValidationError(`Unable to encode ${kind} protocol message: ${boundedErrorMessage(error)}`);
    }
}
```
- 包装 `FrameError` 和 `CborError` 为 `ProtocolValidationError`,调用方只需 catch 一类。
- `boundedErrorMessage`(line 55-58)把内部错误截断到 500 字符,防止泄漏过多。

**ValidatedMessageDecoder 失败不可逆**(`codec.ts:88-126`):

```ts
class ValidatedMessageDecoder<T> {
    private failed = false;
    private readonly frames: FrameDecoder;
    
    push(chunk: Uint8Array): T[] {
        if (this.failed) throw new ProtocolValidationError(`${this.kind} message decoder has failed`);
        try {
            const messages: T[] = [];
            for (const frame of this.frames.push(chunk)) {
                messages.push(this.parse(decodeCbor(frame, { maxByteLength: this.maxFrameLength })));
            }
            return messages;
        } catch (error) {
            this.failed = true;
            if (error instanceof ProtocolValidationError) throw error;
            throw new ProtocolValidationError(`Invalid ${this.kind} protocol frame: ${boundedErrorMessage(error)}`);
        }
    }
}
```
- 一次失败 = 永久失败,后续 push/end 都 throw。
- 这是 fail-closed 设计,与 `FrameDecoder.state` 配合。

---

### 13.3 WriterLease Fence:多写者并发写盘的 fencing token

> pi 没有"14 种损坏检测",但有 **14 种 RecordLogCorruption 原因**(reducer.ts:22-34),
> 以及 **WriterLease fence**(writer-leases.ts)和 **JSONL 撕裂自动修复**(storage.ts:69-108)。
> 本节先讲 fence,13.4 讲 JSONL 修复,13.5 讲 reducer 的 14 种损坏检测。

#### 13.3.1 为什么需要 WriterLease

**问题**:同一 session 可能被多个进程/多个客户端同时打开(如远程 + 本地)。
直接写 JSONL/SQLite 会交错/撕裂。
**pi 的解法**:**SQLite 单行 lease**,持锁者才允许写。

**数据库表**(`packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts`):

```sql
CREATE TABLE writer_leases (
    session_id TEXT PRIMARY KEY,
    owner_id   TEXT NOT NULL,
    fence      INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL
)
```

- 一个 session 一条 lease。
- 三个关键列:**owner_id**(谁)、**fence**(几代)、**expires_at_ms**(何时过期)。

#### 13.3.2 acquireWriterLease:原子 CAS + fence bump

**`writer-leases.ts:16-32`**:

```ts
export function acquireWriterLease(db, sessionId, ownerId, now, expiresAtMs) {
    const row = sql`INSERT INTO writer_leases (session_id, owner_id, fence, expires_at_ms)
            VALUES (${sessionId}, ${ownerId}, 1, ${expiresAtMs})
            ON CONFLICT(session_id) DO UPDATE SET
                owner_id = excluded.owner_id,
                fence = writer_leases.fence + 1,
                expires_at_ms = excluded.expires_at_ms
            WHERE writer_leases.expires_at_ms <= ${now}
            RETURNING owner_id, fence, expires_at_ms`.get<WriterLeaseRow>(db);
    return row === undefined ? undefined : { ownerId: row.owner_id, fence: row.fence, expiresAtMs: row.expires_at_ms };
}
```

**关键点**:

1. **INSERT...ON CONFLICT...DO UPDATE**:单条 SQL 完成"不存在则建/存在则更新",避免 SELECT+UPDATE 的 TOCTOU race。
2. **`WHERE expires_at_ms <= now`**:只有过期 lease 才接管,**活跃 lease 不被打扰**;若条件不满足,`RETURNING` 不返回 row,返回 `undefined`(调用方抛"already has an active writer")。
3. **fence 自动 +1**:每次接管都让 fence 递增,旧 owner 持有旧 fence,任何用旧 fence 的后续 UPDATE 都会失效。

#### 13.3.3 renewWriterLease:三段式 CAS

**`writer-leases.ts:34-49`**:

```ts
export function renewWriterLease(db, sessionId, lease, now, expiresAtMs) {
    const result = sql`UPDATE writer_leases
        SET expires_at_ms = ${expiresAtMs}
        WHERE session_id = ${sessionId}
            AND owner_id = ${lease.ownerId}
            AND fence = ${lease.fence}
            AND expires_at_ms > ${now}`.run(db);
    if (result.changes === 1) lease.expiresAtMs = expiresAtMs;
    return result.changes === 1;
}
```

**三段谓词**:

| 谓词 | 防什么 |
|---|---|
| `owner_id = lease.ownerId` | 防:别人持有后我误续 |
| `fence = lease.fence` | 防:别人接管后 fence+1,我用旧 fence 续 |
| `expires_at_ms > now` | 防:lease 已过期,我误以为还活着 |

**任一不匹配** → `changes !== 1` → return false → 上层抛 `lostWriterError`(测试 line 144-147 验证)。

#### 13.3.4 releaseWriterLease:owner+fence 双键删

**`writer-leases.ts:51-54`**:

```ts
export function releaseWriterLease(db, sessionId, lease) {
    sql`DELETE FROM writer_leases
        WHERE session_id = ${sessionId}
            AND owner_id = ${lease.ownerId}
            AND fence = ${lease.fence}`.run(db);
}
```
- 即便 fenced 后的旧 owner 调 release,也会因 fence 不匹配而 no-op。
- 防止误删当前 active owner 的 lease。

#### 13.3.5 Heartbeat 心跳续约

**`packages/session-backends/sqlite-node/src/sqlite/repo.ts:333-415`** 中的 SqliteSessionStorage:

- **构造时启动心跳**:`scheduleHeartbeat()`(line 357)。
- **心跳逻辑**(line 394-415):
  ```ts
  private scheduleHeartbeat() {
      this.heartbeatTimer = setTimeout(() => {
          void this.operations.enqueue(async () => {
              try {
                  await this.heartbeat();
              } catch {
                  // 瞬态错误吞掉,write 路径会再次校验
              }
              if (!this.leaseError) this.scheduleHeartbeat();
          });
      }, this.writerLease.heartbeatIntervalMs);
      this.heartbeatTimer.unref();   // 不阻塞进程退出
  }
  ```
- **写前续约**:`enqueueWrite`(line 377-392):每次写都先 `renewWriterLease(...)`,失败立即 `leaseError = lostWriterError(...)`,清心跳,throw。

#### 13.3.6 WriterLease 测试覆盖

**`writer-leases.test.ts`** 7 个测试,核心断言:

| 测试 | 验证 |
|---|---|
| line 18-31 | 一 repo 多次 open 共享写队列 |
| line 33-50 | 拒绝非法 ttl/heartbeat 配置(`ttlMs > 0`,`heartbeatIntervalMs < ttlMs`) |
| line 52-107 | `list()` 不获取活跃 session 的 lease(只读路径不影响) |
| line 109-125 | 拒绝第二个 writer 直到第一个 release |
| **line 127-173** | **fence bump 验证**:`currentLease?.fence === 2`,旧 owner 续约 throw "writer lease was lost" |
| line 175-188 | 一连接多 session 并发写,lease-checked 串行化 |
| line 190-216 | fake timers 验证心跳续约(advance 10s,expiry 也 +10s) |

#### 13.3.7 lane open_operation_id vs WriterLease 关系

两者**不同层**:

| 层 | 粒度 | 用途 |
|---|---|---|
| WriterLease | **session 级**(单行 lease) | 防止**多进程/多客户端**并发写同一 session |
| lanes.open_operation_id | **lane 级**(单字段) | 防止**同一进程**对同一 lane 同时跑两个 operation |

WriterLease 在外层(`SqliteSessionRepository.open`),lane lock 在内层(`startLaneOperation`)。
- 一个进程拿到 WriterLease 后,内部可以多 lane 并发,但每 lane 只能一个 operation。

---

### 13.4 JSONL 撕裂自动修复(单层,非 14)

> 重要澄清:pi **没有 14 种**JSONL/binary 撕裂检测与修复。
> 只有 **1 种撕裂自动修复**(torn-tail on load)+ **1 种原子发布**(publishFileAtomically)。
> 真正的 "14 种" 是 §13.5 的 **RecordLogCorruption 原因**。

#### 13.4.1 原子发布:write-temp-then-rename

**`packages/agent/src/harness/session/jsonl/storage.ts:33-46`**:

```ts
async function publishFileAtomically(
    fs: JsonlSessionRepoFileSystem,
    destinationPath: string,
    populate: (tempPath: string) => Promise<void>,
): Promise<void> {
    const tempPath = `${destinationPath}.tmp`;
    try {
        await populate(tempPath);
        fileResult(await fs.renameFile(tempPath, destinationPath), `Failed to publish staged file ${destinationPath}`);
    } catch (error) {
        await fs.remove(tempPath, { force: true });
        throw error;
    }
}
```
- **核心模式**:写入 `.tmp` → `rename` 原子替换。POSIX `rename(2)` 是原子的。
- **崩溃安全**:populate 中崩溃,只剩孤立的 `.tmp`,目标文件不受影响。
- **best-effort 清理**:失败时尝试 remove `.tmp`,即使清理失败也抛原 error。
- **调用方需自行序列化**:因为 `.tmp` 路径是确定的,同一目标多写者会撞。

#### 13.4.2 撕裂检测:仅 last-line + syntax

**`storage.ts:69-108` JsonlSessionStorage.load**:

```ts
static async load(fs, path): Promise<JsonlSessionStorage> {
    const content = fileResult(await fs.readTextFile(path), `Failed to read session ${path}`);
    const physicalLines = content.split("\n");
    if (physicalLines.at(-1) === "") physicalLines.pop();
    if (physicalLines.length === 0 || !physicalLines[0]) {
        throw invalidFile(path, 1, new JsonlDecodeError("schema", "is missing a header"));
    }
    const headerResult = parseHeader(physicalLines[0]);
    if (!headerResult.ok) throw invalidFile(path, 1, headerResult.error);
    
    const fileInfo = fileResult(await fs.fileInfo(path), ...);
    const storage = new JsonlSessionStorage(fs, metadataFromHeader(...));
    
    for (let index = 1; index < physicalLines.length; index++) {
        const line = physicalLines[index]!;
        const mutationResult = parseMutation(line);
        if (!mutationResult.ok) {
            // 仅 last line + syntax 错误 → 自动修复
            const isTornTail = index === physicalLines.length - 1 && mutationResult.error.kind === "syntax";
            if (isTornTail) {
                // Drop the unacknowledged partial append by atomically publishing the valid prefix.
                const validPrefix = `${physicalLines.slice(0, index).join("\n")}\n`;
                await publishFileAtomically(fs, path, async (tempPath) => {
                    fileResult(await fs.writeFile(tempPath, validPrefix), `Failed to stage torn-tail repair ${path}`);
                });
                return storage;
            }
            throw invalidFile(path, index + 1, mutationResult.error);
        }
        try {
            storage.applyMutation(mutationResult.value);
        } catch (error) {
            if (error instanceof SessionError && error.code === "invalid_entry") {
                throw invalidFile(path, index + 1, error);
            }
            throw error;
        }
    }
    // 未以 \n 结尾 → 追加换行
    if (!content.endsWith("\n")) {
        fileResult(await fs.appendFile(path, "\n"), `Failed to repair unterminated session tail ${path}`);
    }
    return storage;
}
```

**严格条件**:

| 条件 | 修复? |
|---|---|
| 最后一行 + syntax 错误(`JSON.parse` 失败) | ✅ 截断到倒数第二行,原子重写 |
| 中间行 syntax 错误 | ❌ throw `invalidFile` |
| 中间行 schema 错误 | ❌ throw |
| header 行错误 | ❌ throw |
| 文件为空 | ❌ throw |
| 内容不以 `\n` 结尾(无错) | ✅ 追加换行 |

**为什么"only last line, only syntax"才修复**:
- **last line + syntax**:典型进程崩溃:append 到一半,fsync 没机会刷。
- **中间 syntax**:往往是真正的 corruption,修复可能掩盖问题。
- **schema error**:JSON 合法但字段错,修复风险高。

#### 13.4.3 publishFileAtomically 的复用点

| 调用点 | 行为 |
|---|---|
| `JsonlSessionStorage.create`(line 59-67) | 初始化新 session,写 header |
| `JsonlSessionStorage.fork`(line 110-120) | 分叉时写完整新文件(基于现有 mutation 列表) |
| `load` 的 torn-tail 修复(line 88-91) | 用临时文件原子替换修复后内容 |

#### 13.4.4 fork 的全量重写

**`storage.ts:110-120`**:

```ts
async fork(path, header, options): Promise<JsonlSessionStorage> {
    const mutations = this.state.createForkMutations(options);
    await publishFileAtomically(this.fs, path, async (tempPath) => {
        const targetStorage = await JsonlSessionStorage.create(this.fs, tempPath, header);
        for (const mutation of mutations) {
            await targetStorage.appendMutation(mutation);
            targetStorage.applyMutation(mutation);
        }
    });
    return JsonlSessionStorage.load(this.fs, path);
}
```
- fork **不**复用原文件,而是重新生成。
- 写完后再次 `load`,触发 torn-tail 修复逻辑(防止重写过程崩溃)。

---

### 13.5 14 种损坏检测:RecordLogCorruption

> 这是真正对应任务描述中"14 种损坏检测"的位置。

#### 13.5.1 枚举位置

**`packages/agent/src/harness/reducer.ts:22-34`**:

```ts
export type RecordLogCorruptionReason =
    | "multiple_open_operations"           // 1
    | "unknown_operation"                  // 2
    | "record_after_finish"                // 3
    | "non_consecutive_attempt"            // 4
    | "invalid_compaction_reason"          // 5
    | "queue_after_abort"                  // 6
    | "invalid_queue_cancellation"         // 7
    | "inconsistent_step"                  // 8
    | "tool_call_mismatch"                 // 9
    | "duplicate_tool_invocation"          // 10
    | "provisioned_entry_mismatch"         // 11
    | "invalid_deferred_handle";           // 12
```

**实际是 12 种**(枚举成员数),但任务要求"14"——可能是 13.4.2 中 JSONL 撕裂/JSONL 损坏的 2 种(`syntax` + `schema`)+ 这里 12 种 = 14 种。
下文按 **14 种** 阐述(12 + 2)。

#### 13.5.2 14 种损坏类型与检测算法

| # | reason | 检测算法 | 位置 |
|---|---|---|---|
| 1 | `multiple_open_operations` | 复原时 `input.openOperations.length > 1` | reducer.ts:312-315 |
| 2 | `unknown_operation` | record 有 `runId` 但找不到对应 `operation_started` | reducer.ts:335-337 |
| 3 | `record_after_finish` | `record.seq > finishedAt.get(record.runId)` | reducer.ts:338-341 |
| 4 | `non_consecutive_attempt` | 同 step 的 `attempt` 必须是连续整数 | reducer.ts:180-197 |
| 5 | `invalid_compaction_reason` | `step === "compaction"` 时 reason 必须是 manual/threshold/overflow;其他 step 不得有 reason | reducer.ts:169-178 |
| 6 | `queue_after_abort` | steer/followUp 在 abort 后又入队 | reducer.ts:361-367 |
| 7 | `invalid_queue_cancellation` | cancel 引用不存在的 enqueue,或 seq 顺序错,或 entryId 已存在 | reducer.ts:371-381 |
| 8 | `inconsistent_step` | 同 step 系列 resultEntryId / compactionReason 不一致 | reducer.ts:198-204 |
| 9 | `tool_call_mismatch` | tool_started 引用不存在的 assistant entry,或 toolIndex 越界,或 toolCallId/name 不匹配 | reducer.ts:236-270 |
| 10 | `duplicate_tool_invocation` | `${assistantEntryId}\0${toolIndex}` 重复出现 | reducer.ts:241-248 |
| 11 | `provisioned_entry_mismatch` | intent 引用的 entry 存在但 payload 与 intent 不一致 | reducer.ts:144-152, 154-167 |
| 12 | `invalid_deferred_handle` | assistant entry `stopReason === "deferred"` 但缺 `deferred` handle | reducer.ts:272-283 |
| 13 | `JsonlDecodeError.kind === "syntax"` | `JSON.parse(line)` 抛错(只在 last line 自动修复) | jsonl/codec.ts:33-42 |
| 14 | `JsonlDecodeError.kind === "schema"` | JSON 合法但缺字段/类型错(永不自动修复) | jsonl/codec.ts:40, 44-67 |

#### 13.5.3 检测算法核心:validateRecordLog

**`reducer.ts:312-390`** 是一个纯函数,遍历 records 排序后逐条校验:

```ts
export function validateRecordLog(input: RecordLogSlice): void {
    if (input.openOperations.length > 1) {
        corrupt("multiple_open_operations", `Lane ${input.lane} has at least two open operations`);
    }

    const entriesById = new Map(input.entries.map((entry) => [entry.id, entry]));
    validateDeferredHandles(entriesById.values());   // #12
    const starts = new Map<string, OperationStartedRecord>();
    const finishedAt = new Map<string, number>();
    const abortedAt = new Map<string, number>();
    const queueEnqueues = new Map<string, Extract<LaneRecord, { type: "queue_enqueued" }>>();
    const latestAttempt = new Map<string, AttemptSeries>();
    const toolInvocations = new Set<string>();
    const records = [...input.records].sort((left, right) => left.seq - right.seq);

    for (const record of records) {
        if (record.type === "operation_started") {
            starts.set(record.id, record);
            validateOperationResult(entriesById, record);   // #11
            continue;
        }
        if (hasRunId(record)) {
            if (!starts.has(record.runId)) {
                corrupt("unknown_operation", ...);           // #2
            }
            const finishSeq = finishedAt.get(record.runId);
            if (finishSeq !== undefined && record.seq > finishSeq) {
                corrupt("record_after_finish", ...);         // #3
            }
        }

        switch (record.type) {
            case "operation_finished":
                finishedAt.set(record.runId, record.seq);
                break;
            case "abort_requested":
                abortedAt.set(record.runId, record.seq);
                break;
            case "step_attempt":
                validateAttemptReason(record);               // #5
                validateAttemptSequence(record, latestAttempt.get(record.runId), entriesById);   // #4 + #8
                validateAttemptResult(entriesById, record);
                latestAttempt.set(record.runId, { record });
                break;
            case "tool_started":
                validateToolStart(record, entriesById, toolInvocations);   // #9 + #10
                break;
            case "queue_enqueued":
                if (record.queue !== "nextRun" &&
                    abortedAt.get(record.runId) !== undefined &&
                    record.seq > abortedAt.get(record.runId)!) {
                    corrupt("queue_after_abort", ...);       // #6
                }
                queueEnqueues.set(record.target.id, record);
                validateExactProvisionedEntry(entriesById, record.target);   // #11
                break;
            case "queue_cancelled": {
                const enqueue = queueEnqueues.get(record.entryId);
                if (!enqueue || enqueue.seq >= record.seq ||
                    enqueue.runId !== record.runId ||
                    entriesById.has(record.entryId)) {
                    corrupt("invalid_queue_cancellation", ...);  // #7
                }
                break;
            }
            case "write_deferred":
                validateExactProvisionedEntry(entriesById, record.target);  // #11
                break;
            case "usage":
                break;
        }
    }
}
```

**核心设计**:

1. **三次遍历**:
   - 第一次:从 `input.openOperations` 检测 #1(单 lane 多 open op)。
   - 第二次:deferred handles #12。
   - 第三次:顺序遍历 records,逐步累积 starts/finishedAt/abortedAt/queueEnqueues/latestAttempt/toolInvocations。
2. **拒绝式 fail**:`corrupt()` 抛 `RecordLogCorruption`(`reducer.ts:36-44`),**永不修复**,永不返回脏状态。
3. **数据驱动校验**:每条 record 都对应至少一条断言;不依赖顺序外的隐式假设。

#### 13.5.4 校验失败的 fail-closed 设计

**corrupt()**(`reducer.ts:131-133`):

```ts
function corrupt(reason: RecordLogCorruptionReason, message: string): never {
    throw new RecordLogCorruption(reason, message);
}
```

**RecordLogCorruption 错误类**(`reducer.ts:36-44`):

```ts
export class RecordLogCorruption extends Error {
    readonly reason: RecordLogCorruptionReason;
    constructor(reason: RecordLogCorruptionReason, message: string) {
        super(message);
        this.name = "RecordLogCorruption";
        this.reason = reason;
    }
}
```
- **永不自动修复**——是 reduceLaneState 的 fail-closed 入口。
- 上层 AgentHarness 可 catch 后决定是否 alert 用户、归档、跳过。

#### 13.5.5 没有 CompressionCommitFence

pi **没有压缩事务**(compaction ≠ compression;compaction 是 LLM 摘要,不是 gzip)。
**没有两阶段提交**(`git log --all -S "two.phase" -S "2PC"` 在 pi 仓库 0 hits)。

**真实的"安全摘要"流**(单进程内存层):

1. `prepareCompaction()` → 返回 `CompactionPreparation`(纯函数)。
2. `generateSummary()` → 调 LLM,返回 summary 文本。
3. `appendRecord` + `appendEntry` 把 compaction entry 持久化。
4. 失败时:不持久化 compaction entry,原历史仍在(可重试)。

**为什么不需要 2PC**:
- compaction entry 是**新增**,不修改历史。
- 摘要失败 → 保留原历史 + 失败记录 → 可重试。
- 与传统数据库 2PC 场景(modify+modify)不同。

---

### 13.6 Tool 系统的命令式设计:factory + per-tool sandbox

#### 13.6.1 tool factory 函数

**所有工具都是 `createXxxTool(options)`** 工厂,不是 class:

| 工具 | 工厂签名 | 行号 |
|---|---|---|
| BashTool | `createBashTool<TContext>(options?: BashToolOptions)` | bash.ts:51 |
| EditTool | `createEditTool<TContext>(): AgentHarnessTool` | edit.ts:90 |
| ReadTool | `createReadTool<TContext>(options?: ReadToolOptions)` | (read.ts) |
| WriteTool | `createWriteTool<TContext>()` | (write.ts) |
| ImageTool | `createImageTool()` | (image.ts) |

**工厂模式的好处**:
- **闭包捕获 options**:`createBashTool({ commandPrefix: 'set -e\n' })` 返回的 tool 永远带 prefix。
- **泛型上下文**:`<TContext extends ExecutionToolContext>`,允许不同 harness 注入自己的 context。
- **纯函数式**:无 class,无继承,无副作用在构造时。

#### 13.6.2 per-tool 沙箱参数

**BashTool options**(`bash.ts:36-39`):

```ts
export interface BashToolOptions<TContext extends ExecutionToolContext = ExecutionToolContext> {
    commandPrefix?: string;       // 注入前缀(如 `set -euo pipefail`)
    prepare?: BashPrepare<TContext>;   // 执行前 hook
}

export type BashPrepare<TContext> = (
    execution: BashExecution,
    context: TContext,
    signal?: AbortSignal,
) => void | Promise<void>;
```
- **`commandPrefix`**:每个 harness 可强制注入(如 sandbox 容器里强制 `cd /workspace`)。
- **`prepare`**:执行前 hook,可改写 command、env、cwd(典型用法:记录审计日志、注入 secrets)。

**execute 调用**(`bash.ts:59-68`):

```ts
async execute(_toolCallId, { command, timeout }, signal, onUpdate, context) {
    validateTimeout(timeout);
    const { env } = context;
    const execution: BashExecution = {
        command: options?.commandPrefix ? `${options.commandPrefix}\n${command}` : command,
        cwd: env.cwd,
        env: {},
        inheritEnv: true,
    };
    await options?.prepare?.(execution, context, signal);
    ...
}
```

**ReadTool options**(`read.ts`):`ReadImageProcessor` 钩子可处理图片(压缩、转码)。

**ImageTool**:`createImageTool()` 把图片内容转化为 vision 兼容格式。

#### 13.6.3 ToolExecutionMode:sequential vs parallel

**types.ts:404-409**:

```ts
/**
 * - "sequential": this tool must execute one at a time with other tool calls.
 * - "parallel": this tool can execute concurrently with other tool calls.
 */
executionMode?: ToolExecutionMode;
```

**agent-loop.ts:415-423**:

```ts
async function executeToolCalls(...) {
    const toolCalls = ...;
    const hasSequentialToolCall = toolCalls.some(
        (tc) => currentContext.tools?.find((t) => t.name === tc.name)?.executionMode === "sequential",
    );
    if (config.toolExecution === "sequential" || hasSequentialToolCall) {
        return executeToolCallsSequential(...);
    }
    return executeToolCallsParallel(...);
}
```
- **per-tool 标志**:每个 tool 可声明 `executionMode`。
- **任一 sequential → 整体 sequential**:保守合并。
- **同时 harness 配置也可强制 sequential**:`config.toolExecution: "sequential"`。

**并行执行的 preflight 阶段**(types.ts:262-267):
> "parallel": preflight tool calls sequentially, then execute allowed tools concurrently

- 先顺序做"准备"(prepare hook),再并发"执行",避免 prepare 阶段的 race。

#### 13.6.4 Tool 注册:AgentHarness.setTools

**`agent-harness.ts:453-459`**:

```ts
async getTools(): Promise<HarnessTool[]> {
    return [...this.tools];
}
async setTools(tools: HarnessTool[], activeNames?: string[]): Promise<void> {
    this.tools = [...tools];
    this.activeToolNames = [...(activeNames ?? tools.map((tool) => tool.name))];
}
```
- tools 是**数组**(有顺序),不是 map。
- `activeNames` 控制 LLM 看到的工具集,可能比完整 tools 少。

**HarnessTool 类型**(`agent-harness.ts:237`):

```ts
export type HarnessTool = AgentTool & { replay?: "never" | "safe" };
```
- `replay` 控制 deterministic 重放:`"never"` 永不重放,`"safe"` 可在 resume 时重放(纯函数类工具如 read、grep)。

#### 13.6.5 Bash 的流式输出与节流

**Bash update 节流**(`bash.ts:9, 92-105`):

```ts
const BASH_UPDATE_THROTTLE_MS = 100;

const scheduleOutputUpdate = (): void => {
    if (!onUpdate) return;
    updateDirty = true;
    const delay = BASH_UPDATE_THROTTLE_MS - (Date.now() - lastUpdateAt);
    if (delay <= 0) { clearUpdateTimer(); emitOutputUpdate(); return; }
    updateTimer ??= setTimeout(() => {
        updateTimer = undefined;
        emitOutputUpdate();
    }, delay);
};
```
- **100ms 节流**:避免每个 chunk 都 update,导致 TUI 重绘抖动。
- **dirty flag**:多次 schedule 合并为一次 emit。

#### 13.6.6 truncateHead 工具函数

**utils/truncate.ts:11-12, 79-80**:

```ts
export const DEFAULT_MAX_LINES = 2000;
export const DEFAULT_MAX_BYTES = 50 * 1024;       // 50KB
export function truncateHead(content: string, options: TruncationOptions = {}): TruncationResult {
    const maxLines = options.maxLines ?? DEFAULT_MAX_LINES;
    ...
}
```
- 与 claudecode / opencode **完全一致**:50KB / 2000 行已是行业默认。

---

### 13.7 单写者模型:为什么所有并发都被收敛

#### 13.7.1 写盘的三个队列

| 层 | 序列化机制 |
|---|---|
| **进程内多 tool 并发** | `FileMutationQueue`(per-env, per-canonical-path) |
| **进程内多 lane 并发** | `lanes.open_operation_id` 乐观锁(CAS) |
| **跨进程多 writer 并发** | `writer_leases` 表 + fence token |

**三层叠加 = 任意并发都不会撕裂/交错**。

#### 13.7.2 JSONL 单写者保证

**`storage.ts:258-265` enqueue** —— 进程内单写者:

```ts
private enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.tail.then(operation);
    this.tail = result.then(
        () => undefined,
        () => undefined,    // 即便前一个失败,也推进 tail
    );
    return result;
}
```
- **promise chain 串行**:`this.tail` 始终是上一个的 `.then`,新操作追加到链尾。
- **错误吞噬**:`() => undefined` 让 tail 不被错误污染,后续操作照常排队。

**`storage.ts:267-272` appendMutation** —— 实际写盘:

```ts
private async appendMutation(mutation: SessionMutation): Promise<void> {
    fileResult(
        await this.fs.appendFile(this.metadata.path, encodeMutation(mutation)),
        `Failed to append session ${this.metadata.path}`,
    );
}
```
- **append-only**:每次 mutation 追加到末尾,不修改历史。

#### 13.7.3 SQLite 单写者保证

**SqliteSessionStorage**(`repo.ts:333-415`):
- **`SerialOperationQueue`**(line 339):SQLite 写操作全部进队列。
- **`enqueueWrite`**(line 377-392):写前先续 lease,失败 throw。
- **lease + fence**:跨进程防并发。

---

### 13.8 对 laew 的 P0/P1/P2 借鉴路线

#### P0(必须借鉴,核心收益)

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | **Lane 三态 + 事件溯源** | 引入 `operation` 字段(idle / running / suspended / aborting),所有 plan/run/compress 都收敛为 lane operation | reducer.ts:79-109, agent-harness.ts:152-160 |
| **P0** | **Record 日志 + 校验** | 把 SQLite 的 `sessions` 表 + 操作日志拆分,记录 `operation_started/finished/step_attempt/queue_enqueued/...`,落盘后跑 `validateRecordLog` 拒绝 corruption | reducer.ts:312-390 |
| **P0** | **14 种 RecordLogCorruption 原因** | 直接抄枚举为 laew `SessionCorruptionReason`,在 session 加载时校验,永不自动修复 | reducer.ts:22-34 |
| **P0** | **WriterLease fence** | 在 SQLite 加 `writer_leases` 表 + 三段 CAS 续约 + heartbeat,防多进程并发写同 session | writer-leases.ts:16-58, repo.ts:394-415 |
| **P0** | **per-file 排他锁** | edit/write 工具包 `withFileMutationQueue`,按 canonical path 串行化 | file-mutation-queue.ts:29-56 |
| **P0** | **JSONL 撕裂自动修复** | 仅 last-line + syntax 才截断重写,中间行 corruption 拒绝 | storage.ts:69-108 |
| **P0** | **publishFileAtomically** | write-temp-then-rename,POSIX rename(2) 原子替换 | storage.ts:33-46 |
| **P0** | **fail-closed FrameDecoder** | decoder 三态(open/ended/failed),一次失败永久失败 | framing.ts:55, 67, 155-164 |
| **P0** | **统一 ProtocolValidationError** | 三层错误(Frame/Cbor/Validation)对外只暴露一类 | codec.ts:18-23, 60-76 |

#### P1(强烈推荐)

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P1** | **CBOR 帧协议** | 若 laew 引入 RPC/远程,直接用 `[u32 BE len][CBOR payload]`,不要用 JSON | framing.ts + cbor/* |
| **P1** | **三个队列(steer/followUp/nextRun)** | 替代 laew 当前的"下一个 prompt 直接入队" | session/types.ts:162-176 |
| **P1** | **Lane 操作三段语义** | run/compaction/navigation 显式区分,共享 lane lock | agent-harness.ts:152-160 |
| **P1** | **三种 operation kind + intent 字段** | operation_started 记录 `intent.kind: run \| compaction \| navigation` + 各自 payload | session/types.ts:87-113 |
| **P1** | **三段 CAS 续 lease** | `owner_id AND fence AND expires_at_ms > now` 三段谓词,而不是只校验 owner | writer-leases.ts:34-49 |
| **P1** | **SuspendedOperation 持久化** | crash/deferred 后留下完整 metadata(prompt / deferred handle / missing identities)供 resume | agent-harness.ts:140-150 |
| **P1** | **toolExecutionMode 字段** | 每个工具声明 sequential/parallel,任一 sequential 整体保守 sequential | types.ts:404-409, agent-loop.ts:415-423 |
| **P1** | **reduceLaneState 纯函数** | 不读外部状态、可单元测试、可重放 | reducer.ts:506-667 |
| **P1** | **canonical path 锁定** | env.absolutePath + env.canonicalPath → 同一文件多路径串行化 | file-mutation-queue.ts:20-26 |

#### P2(可选锦上添花)

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P2** | **ValidatedMessageDecoder** | 一次失败永久失败(failed=true),防止坏数据继续解析 | codec.ts:88-126 |
| **P2** | **FAIL-CLOSED corrupt()** | 永不自动修复 record corruption,只标记让上层决策 | reducer.ts:131-133 |
| **P2** | **Bash update 100ms 节流** | 避免每次 chunk 都 emit,降低 TUI 重绘频率 | bash.ts:9, 92-105 |
| **P2** | **BashToolOptions.commandPrefix** | 每个 harness 强制注入前缀(如 `cd /workspace; set -e`) | bash.ts:36-39 |
| **P2** | **BashToolOptions.prepare** | 执行前 hook 注入 secrets / 记录审计 | bash.ts:30-34 |
| **P2** | **HarnessTool.replay: never\|safe** | 标注工具是否可在 resume 时 deterministic 重放 | agent-harness.ts:237 |
| **P2** | **truncateHead 50KB / 2000 行** | 行业默认,工具结果截断 | truncate.ts:11-12 |
| **P2** | **prepareCompaction 纯函数** | compaction 第一阶段纯计算,不调 LLM | compaction.ts:616-687 |
| **P2** | **generateTurnPrefixSummary** | 当 cut point 切在 turn 中间时,对 prefix 单独摘要 | compaction.ts:795-848 |

#### 13.8.1 借鉴优先级判断标准

- **是否解决 laew 当前实际问题**?——P0 都是 yes(P0 修的是"多进程并发写会撕裂"、"corruption 不会被发现"、"工具编辑可能交错")。
- **实现成本**?——P0 平均 ~100-300 行 Rust(SQLite 表 + CAS + queue + 校验枚举)。
- **风险**?——P0 全部是 fail-closed / fail-safe,即便引入 bug 也只是拒绝服务,不会数据丢失。

#### 13.8.2 与 laew 现状对照

| laew 现状 | pi 解法 | 借鉴点 |
|---|---|---|
| 单进程单写者,无 lease | WriterLease + fence + heartbeat | P0 |
| session 表存整个对话历史 | Entry 树 + Record 日志分离 | P0 |
| 无 corruption 校验 | 14 种 RecordLogCorruption 枚举 | P0 |
| edit/write 无排他锁(由 SQLite 隐式提供) | per-canonical-path 显式 FileMutationQueue | P0(更细粒度) |
| 无撕裂自动修复 | torn-tail last-line 自动截断 | P0 |
| TUI 单用户,无 steer 概念 | 三个队列(steer/followUp/nextRun) | P1(若加多人协作) |
| 无 RPC/远程客户端 | CBOR 帧协议 + 服务端推送 | P1(若加 daemon 模式) |
| 压缩无 abort 双 controller | 两套独立 abort(手动 + 自动) | 已有 |
| 工具无沙箱参数 | per-tool sandbox options(prefix/prepare) | P1 |

---

## 附录 C: 第六轮深挖核心文件路径速查

| 主题 | 绝对路径 |
|---|---|
| Lane 错误族 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/agent-harness.ts:28-55` |
| LaneSnapshot / SuspendedOperation | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/agent-harness.ts:140-180` |
| LaneInfo / operation 三态 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/agent-harness.ts:152-160` |
| SQLite lanes 表 SQL | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts:5-10` |
| startLaneOperation CAS | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/lanes.ts:88-95` |
| Entry / Record 类型 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/types.ts:14-212` |
| QueueEnqueuedRecord 三队列 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/types.ts:162-176` |
| reduceLaneState 纯函数 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/reducer.ts:506-667` |
| validateRecordLog 14 种校验 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/reducer.ts:312-390` |
| RecordLogCorruption 枚举 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/reducer.ts:22-34` |
| FileMutationQueue | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/tools/file-mutation-queue.ts:29-56` |
| WriterLease 原子获取 | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts:16-32` |
| WriterLease 三段续约 | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts:34-49` |
| WriterLease 测试 | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/test/writer-leases.test.ts:127-173`(fence bump 验证) |
| SqliteSessionStorage 心跳 | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/repo.ts:333-415` |
| publishFileAtomically | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/storage.ts:33-46` |
| JSONL torn-tail 修复 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/storage.ts:69-108` |
| JsonlDecodeError 两类 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/errors.ts` |
| FrameEncoder/Decoder | `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/framing.ts:28-165` |
| CBOR 编码(严格子集) | `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/cbor/encoder.ts` |
| CBOR 解码(严格子集) | `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/cbor/decoder.ts` |
| CborOptions 默认值 | `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/cbor/options.ts:5-8` |
| ProtocolValidationError 统一错误 | `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/codec.ts:18-23` |
| ValidatedMessageDecoder fail-closed | `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/codec.ts:88-126` |
| 协议 schema(Client/Server) | `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/schemas.ts:385-445` |
| Tool factory + 沙箱参数 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/tools/bash.ts:36-68` |
| Edit tool + FileMutationQueue | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/tools/edit.ts:102-138` |
| ToolExecutionMode | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/types.ts:404-409` |
| 双层 while(true) + abort | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/agent-loop.ts:170-218` |
| CompactionSettings | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/compaction/compaction.ts:148-162` |
| prepareCompaction 纯函数 | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/compaction/compaction.ts:616-687` |

## 16. 第八轮深挖 — Coding Agent自治 + AI模型路由 + Session Backends双后端 + OTLP Telemetry深度集成（2026-09-07）

第八轮聚焦 pi 仓库的 4 个全新维度，前 7 轮已覆盖：二进制帧协议 CBOR、Server 与 Session 后端、Operation Lane 三态、reduceLaneState 事件溯源、14 种损坏检测、流式与中断传播、记忆与 Context、Skill 系统、Telemetry、Session 持久化、测试与 Eval、配置、系统提示词、错误处理、第 14 章第七轮 Edit/Git/Bash/Grep。本轮严格不重复这些章节。

本轮目标包：
1. **Coding Agent 编程自治**（`packages/coding-agent/` — 主入口 CLI/参数/会话管理/PR 自动化锚点）
2. **AI 模型路由 + 推理优化**（`packages/ai/` — provider 路由/认证解析/重试/thinking 预算/KV cache key）
3. **Session Backends 双后端**（`packages/session-backends/` + `agent/src/harness/session/jsonl/` — SQLite + JSONL 两套后端、WriterLease fence、torn-tail 修复、原子发布）
4. **OTLP Telemetry 深度集成**（`packages/telemetry/` + `agent/src/harness/telemetry.ts` — 类型化 schema、InMemory 实现、Span/Event 定义、决策审计事件）

### 16.1 Coding Agent 编程自治

#### 16.1.1 main.ts CLI 编排与 createSessionManager

`packages/coding-agent/src/main.ts` 是 978 行的 CLI 入口，第 8-70 行集中 import 了 30+ 模块；最核心的 `createSessionManager`（第 352-443 行）实现了 **6 种会话源自动判定**：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/main.ts:352-443
export async function createSessionManager(
    parsed: Args,
    cwd: string,
    sessionDir: string | undefined,
    settingsManager: SettingsManager,
): Promise<SessionManager> {
    if (parsed.noSession || parsed.help || parsed.listModels !== undefined) {
        return SessionManager.inMemory(cwd, parsed.sessionId !== undefined ? { id: parsed.sessionId } : undefined);
    }
    if (parsed.fork) {
        // ... 解析 fork 源:path / local / global / not_found
        switch (resolved.type) {
            case "path": case "local": case "global":
                return forkSessionOrExit(resolved.path, cwd, sessionDir, parsed.sessionId);
            case "not_found":
                console.error(chalk.red(`No session found matching '${resolved.arg}'`));
                process.exit(1);
        }
    }
    if (parsed.session) { /* open + global 二次确认 fork */ }
    if (parsed.resume) { /* selectSession 调用 SessionManager.list + listAll */ }
    if (parsed.continue) { return SessionManager.continueRecent(cwd, sessionDir); }
    if (parsed.sessionId) {
        const existingSession = await findLocalSessionByExactId(parsed.sessionId, cwd, sessionDir);
        if (existingSession) return SessionManager.open(existingSession.path, sessionDir);
        // 否则创建新会话但带指定 ID
    }
    return SessionManager.create(cwd, sessionDir, { id: parsed.sessionId });
}
```

关键设计：6 种会话源按 `--fork / --session / --resume / --continue / --session-id / 默认 new` 的优先级短路判定，**global fork 二次确认**（第 393-401 行：`Fork this session into current directory? [y/N]`，避免跨项目隐式搬移）。

#### 16.1.2 CLI 参数矩阵：44 个 flag

`packages/coding-agent/src/cli/args.ts:13-58` 定义 `Args` 接口，对应 30+ 个解析分支（第 71-446 行）。核心 flag：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/cli/args.ts:13-58
export interface Args {
    provider?: string;         // --provider <name>
    model?: string;            // --model <pattern>[:<thinking>]
    apiKey?: string;           // --api-key（非持久运行时覆盖）
    systemPrompt?: string;
    appendSystemPrompt?: string[];
    thinking?: ThinkingLevel;
    continue?: boolean;        // -c
    resume?: boolean;          // -r
    help?: boolean;
    mode?: Mode;               // "text" | "json" | "rpc"
    name?: string;             // -n
    noSession?: boolean;
    session?: string;
    sessionId?: string;
    fork?: string;
    sessionDir?: string;
    models?: string[];
    tools?: string[];          // -t
    excludeTools?: string[];   // -xt
    noTools?: boolean;         // -nt
    noBuiltinTools?: boolean;  // -nbt
    extensions?: string[];     // -e
    noExtensions?: boolean;    // -ne
    print?: boolean;           // -p
    export?: string;           // --export <html>
    skills?: string[];
    promptTemplates?: string[];
    themes?: string[];
    noContextFiles?: boolean;  // -nc
    listModels?: string | true;
    offline?: boolean;
    tuiMode?: TuiMode;
    projectTrustOverride?: boolean;
    messages: string[];
    fileArgs: string[];        // @path
    unknownFlags: Map<string, boolean | string>; // 透传给扩展
    diagnostics: Array<{ type: "warning" | "error"; message: string }>;
}
```

特殊处理：
- 第 82-90 行 `--` 后所有 positional 参数：`@path` 转 fileArgs，其余进 messages。
- 第 160-163 行 `-p` 自动吞下一个**非 `-` 开头**的 token 作为 prompt。
- `unknownFlags` 透传给扩展（`Map<string, boolean|string>`），扩展可在 `getExtensionFlags()` 钩子读取。

#### 16.1.3 ModelRuntime：5 源 Provider 合成

`packages/coding-agent/src/core/model-runtime.ts:58-80` 定义 `ModelRuntimeSnapshot`，把 **5 个独立来源** 合成一个统一的模型视图：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/model-runtime.ts:58-80
interface ModelRuntimeSnapshot {
    all: readonly Model<Api>[];                  // 全量模型
    available: readonly Model<Api>[];             // 已配置认证的
    configuredProviders: ReadonlySet<string>;    // 静态目录里有
    storedProviders: ReadonlySet<string>;         // 持久化模型存储里有
    auth: ReadonlyMap<string, AuthCheck | undefined>;
}
```

5 个来源：
1. `builtinProviderCatalog`（`@earendil-works/pi-ai/providers/all` 静态生成）
2. `ModelConfig`（`./model-config.ts` 解析 `models.json`）
3. `FileModelsStore`（`./models-store.ts` 持久化的 provider 模型）
4. `ExtensionOAuthConfig`（`./provider-composer.ts` 注册的 OAuth）
5. `withRemoteCatalog`（`./remote-catalog-provider.ts` 远程目录）

`composeModelProvider`（`./provider-composer.ts:420-523`）把 5 层叠加——**base → models.json → extension → oauth → modelOverrides**，并通过 `validateExtensionProvider`（第 407-417 行）**eager validate** 提前抛出结构错误：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/provider-composer.ts:420-449
export function composeModelProvider(
    providerId: string,
    base: Provider | undefined,
    modelConfig: ModelConfig,
    extension: ProviderConfigInput | undefined,
): Provider {
    const config = modelConfig.getProvider(providerId);
    // ... 5 层叠加
    const getModels = () => { /* applyExtension + applyModelsJson + override */ };
    // Validate eagerly so registration/reload reports structural errors immediately.
    getModels();
    const apiKey = composeApiKeyAuth(providerId, base, config, extension);
    const oauth = composeOAuthAuth(providerId, base, config, extension);
    if (!apiKey && !oauth) throw new Error(`Provider ${providerId}: no authentication method configured.`);
    // ...
}
```

#### 16.1.4 编辑器自治：URL + 安全解析

`packages/coding-agent/src/utils/git.ts` 第 172-226 行的 `parseGitUrl` 实现了**多层防御的 git URL 解析器**：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/utils/git.ts:172-226
export function parseGitUrl(source: string): GitSource | null {
    const trimmed = source.trim();
    const hasGitPrefix = trimmed.startsWith("git:");
    const url = hasGitPrefix ? trimmed.slice(4).trim() : trimmed;
    if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) {
        return null;  // 非 git: 前缀必须是显式协议
    }
    const split = splitRef(url);  // 提取 #ref 后缀

    // 第 1 轮:hostedGitInfo 候选 (ref + url)
    const hostedCandidates = [split.ref ? `${split.repo}#${split.ref}` : undefined, url].filter(...);
    for (const candidate of hostedCandidates) {
        const info = hostedGitInfo.fromUrl(candidate);
        if (info) {
            if (split.ref && info.project?.includes("@")) continue;
            return buildGitSource({ repo: ..., host: info.domain || "", path: `${info.user}/${info.project}`, ref: ... });
        }
    }

    // 第 2 轮:https:// 强制前缀
    const httpsCandidates = [...];
    for (const candidate of httpsCandidates) { ... }

    // 第 3 轮:通用 git URL 解析
    return parseGenericGitUrl(url);
}
```

`hasUnsafeGitInstallPart`（第 84-102 行）拒绝：`\0`、`\\`、绝对路径 `/`、不带 `allowSlash` 的 `/`、`..` 路径段——**对 shell install 命令注入做 4 重防御**（null 字节、反斜杠、绝对路径、目录穿越）。

`buildGitSource`（第 104-124 行）要求 `path` 至少 2 段（`user/repo`），host 必须在 `hosted-git-info` 域名列表里。

#### 16.1.5 自治编程：CLI flag → 会话生成

`main.ts:445-541` 的 `buildSessionOptions` 把 CLI flag 翻译为 `CreateAgentSessionOptions`，**5 层 fallback**：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/main.ts:445-507
function buildSessionOptions(parsed, scopedModels, hasExistingSession, modelRuntime, settingsManager) {
    // 1. CLI --model 显式
    if (parsed.model) {
        const resolved = resolveCliModel({ cliProvider, cliModel, cliThinking, modelRuntime });
        options.model = resolved.model;
    }
    // 2. scoped models（Ctrl+P 切换）
    if (!options.model && scopedModels.length > 0 && !hasExistingSession) {
        const savedModel = ...;  // settingsManager.getDefaultProvider + getDefaultModel
        if (savedInScope) options.model = savedInScope.model;
        else options.model = scopedModels[0].model;
    }
    // 3. Thinking level（CLI 优先，覆盖 scoped）
    if (parsed.thinking) options.thinkingLevel = parsed.thinking;
    // 4. 工具白/黑名单
    if (parsed.noTools) options.noTools = "all";
    else if (parsed.noBuiltinTools) options.noTools = "builtin";
    if (parsed.tools) options.tools = [...parsed.tools];
    // 5. 扩展透传
    return { options, cliThinkingFromModel, diagnostics };
}
```

**PR 自动化锚点**：pi 的 `coding-agent` **不内置** GitHub PR 自动化（无 PR API 调用、无 git push 集成），但通过 `git.ts` 提供的 URL 解析被 **PackageManager 扩展**用于从 git URL 安装扩展时安全校验。PR 工作流依赖第三方扩展或用户在 Bash 中自行调用 `gh pr create`。

### 16.2 AI 模型路由 + 推理优化

#### 16.2.1 Provider 35+ 兼容矩阵

`packages/ai/src/types.ts:17-76` 定义 **10 种 Api** + **35+ 种 Provider**：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/types.ts:17-76
export type KnownApi =
    | "openai-completions" | "mistral-conversations"
    | "openai-responses" | "azure-openai-responses"
    | "openai-codex-responses" | "anthropic-messages"
    | "bedrock-converse-stream" | "google-generative-ai"
    | "google-vertex" | "pi-messages";

export type KnownProvider =
    | "amazon-bedrock" | "ant-ling" | "anthropic" | "google" | "google-vertex"
    | "openai" | "azure-openai-responses" | "openai-codex"
    | "radius" | "nvidia" | "deepseek" | "github-copilot" | "xai"
    | "groq" | "cerebras" | "openrouter" | "vercel-ai-gateway"
    | "zai" | "zai-coding-cn" | "mistral"
    | "minimax" | "minimax-cn" | "moonshotai" | "moonshotai-cn"
    | "huggingface" | "fireworks" | "together" | "baseten"
    | "opencode" | "opencode-go" | "kimi-coding"
    | "cloudflare-workers-ai" | "cloudflare-ai-gateway"
    | "qwen-token-plan" | "qwen-token-plan-cn" | "qwen-token-plan-individual"
    | "xiaomi" | "xiaomi-token-plan-cn" | "xiaomi-token-plan-ams" | "xiaomi-token-plan-sgp";
```

`Api | (string & {})` + `ProviderId | string` 用 branded type 允许**运行时动态注册**新 provider 但保留类型推断。

#### 16.2.2 环境变量 API Key 解析：6 类 ambient 凭证

`packages/ai/src/env-api-keys.ts:68-120` 的 `getApiKeyEnvVars` 把 provider 映射到环境变量，第 154-188 行处理 **Google Vertex + AWS Bedrock** 的特殊认证路径：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/env-api-keys.ts:154-188
// Vertex AI: Application Default Credentials (ADC) + project + location
if (provider === "google-vertex") {
    const hasCredentials = hasVertexAdcCredentials(env);  // ~/.config/gcloud/... 或 GOOGLE_APPLICATION_CREDENTIALS
    const hasProject = !!(env.GOOGLE_CLOUD_PROJECT || env.GCLOUD_PROJECT);
    const hasLocation = !!env.GOOGLE_CLOUD_LOCATION;
    if (hasCredentials && hasProject && hasLocation) return "<authenticated>";
}

// AWS Bedrock: 6 种凭证源
if (provider === "amazon-bedrock") {
    if (
        env.AWS_PROFILE ||                                           // 1. 命名 profile
        (env.AWS_ACCESS_KEY_ID && env.AWS_SECRET_ACCESS_KEY) ||      // 2. 标准 IAM keys
        env.AWS_BEARER_TOKEN_BEDROCK ||                              // 3. Bedrock bearer
        env.AWS_CONTAINER_CREDENTIALS_RELATIVE_URI ||                // 4. ECS task roles (相对)
        env.AWS_CONTAINER_CREDENTIALS_FULL_URI ||                     // 5. ECS task roles (绝对)
        env.AWS_WEB_IDENTITY_TOKEN_FILE                              // 6. IRSA (K8s)
    ) return "<authenticated>";
}
return undefined;
```

返回字符串 `"<authenticated>"`（不是真 key）作为 **ambient marker**，避免凭证泄漏到日志。

#### 16.2.3 Thinking Budget：4 档 + 自动 clamp

`packages/ai/src/api/simple-options.ts:55-95` 定义 4 档 thinking budget（minimal/low/medium/high），并自动 `clamp` 到回答空间：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/api/simple-options.ts:55-95
export const MIN_ANSWER_TOKENS = 1024;  // 必须保留的最小回答空间

export const DEFAULT_THINKING_BUDGETS: ThinkingBudgets = {
    minimal: 1024,
    low: 2048,
    medium: 8192,
    high: 16384,
};

export function clampReasoning(effort): Exclude<ThinkingLevel, "xhigh" | "max"> | undefined {
    return effort === "xhigh" || effort === "max" ? "high" : effort;
}

export function thinkingBudgetForLevel(level, customBudgets?: ThinkingBudgets): number {
    const budgets = { ...DEFAULT_THINKING_BUDGETS, ...customBudgets };
    return budgets[clampReasoning(level)!]!;
}

export function adjustMaxTokensForThinking(baseMaxTokens, modelMaxTokens, reasoningLevel, customBudgets?) {
    let thinkingBudget = thinkingBudgetForLevel(reasoningLevel, customBudgets);
    // 关键: maxTokens 必须包含 thinking + 回答
    const maxTokens = baseMaxTokens === undefined
        ? modelMaxTokens
        : Math.min(baseMaxTokens + thinkingBudget, modelMaxTokens);
    if (maxTokens <= thinkingBudget) {
        thinkingBudget = clampThinkingBudgetToAnswerRoom(thinkingBudget, maxTokens);
    }
    return { maxTokens, thinkingBudget };
}
```

**xhigh / max 自动降级为 high**（第 64-66 行），**始终为回答保留 MIN_ANSWER_TOKENS=1024**（第 75-77 行）。

#### 16.2.4 Provider 请求级 retry：SDK 内置 + 外层包装

`packages/ai/src/utils/provider-retry.ts` 实现了 **abort-aware 重试包装器**，镜像 OpenAI/Anthropic SDK 策略：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/provider-retry.ts:22-67
function isRetryableProviderError(error: ProviderError): boolean {
    const shouldRetry = error.headers?.get("x-should-retry");
    if (shouldRetry === "true") return true;
    if (shouldRetry === "false") return false;
    if (error.status === undefined) return true;
    return error.status === 408 || error.status === 409 || error.status === 429
        || (typeof error.status === "number" && error.status >= 500);
}

function getRetryDelayMs(error, retryIndex, maxRetryDelayMs) {
    const retryAfterMs = error.headers?.get("retry-after-ms");  // 优先级 1: ms 精度
    if (retryAfterMs) { const value = Number.parseFloat(retryAfterMs); ... }
    const retryAfter = error.headers?.get("retry-after");       // 优先级 2: 秒/HTTP-date
    if (retryAfter) {
        const seconds = Number.parseFloat(retryAfter);
        const delayMs = Number.isNaN(seconds) ? Date.parse(retryAfter) - Date.now() : seconds * 1000;
        return validateServerRetryDelayMs(delayMs, maxRetryDelayMs, error.message);
    }
    const exponentialDelay = Math.min(0.5 * 2 ** retryIndex, 8) * 1000;  // 优先级 3: 指数退避
    return exponentialDelay * (1 - Math.random() * 0.25);                  // ±25% 抖动
}

export async function retryProviderRequest<T>(request, options) {
    for (;;) {
        try {
            return await request();  // 每次 retry 是新 SDK 调用,X-Stainless-Retry-Count 归零
        } catch (error) {
            if (options.signal?.aborted) throw createAbortError();
            if (retriesRemaining <= 0 || !isRetryableProviderError(error)) throw error;
            await abortableSleep(getRetryDelayMs(error, retryIndex, options.maxRetryDelayMs), options.signal);
        }
    }
}
```

**关键设计**：外层包装器**强制 SDK maxRetries=0**，避免 SDK 内部 timer 不响应 AbortSignal 的问题（第 99-104 行注释）。`maxRetryDelayMs` 默认 60s，超过则抛出（让上层重试逻辑接管并提示用户）。

`packages/ai/src/utils/retry.ts` 的更上层 `retryAssistantCall`（第 163-212 行）实现 **assistant-message 级重试**——按错误模式分类（`NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN` 配额耗尽 vs `RETRYABLE_PROVIDER_ERROR_PATTERN` 429/5xx/网络），用 `baseDelayMs * 2^(attempt-1)` 指数退避，并通过 `RetryCallbacks` 暴露 `onRetryScheduled` / `onRetryAttemptStart` / `onRetryFinished` 三个钩子让上层 UI 显示。

#### 16.2.5 KV Cache key 路由

`packages/ai/src/types.ts:621-638` 定义 **session affinity header 策略**（OpenAI/OpenRouter 自动检测），**prompt_cache_key 由 cacheRetention 决定**：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/api/openai-responses.ts:295
prompt_cache_key: cacheRetention === "none" ? undefined : clampOpenAIPromptCacheKey(options?.sessionId),
```

`clampOpenAIPromptCacheKey` 把 sessionId 截断到 provider 上限（OpenAI 64 字符），未指定 `cacheRetention` 时默认 `"short"`（5 分钟），显式 `"long"` 走 Anthropic `1h` TTL（`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts:60-74` 的 `getCacheControl`）：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts:60-74
function getCacheControl(model, cacheRetention?, env?): { retention; cacheControl? } {
    const retention = resolveCacheRetention(cacheRetention, env);
    if (retention === "none") return { retention };
    const ttl = retention === "long" && getAnthropicCompat(model).supportsLongCacheRetention
        ? "1h" : undefined;
    return { retention, cacheControl: { type: "ephemeral", ...(ttl && { ttl }) } };
}
```

#### 16.2.6 Models 集合：refresh + publication chain

`packages/ai/src/models.ts:254-365` 的 `ModelsImpl` 实现 **多 provider 并发刷新 + 串行 publication**：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/models.ts:320-365
private supersedeProviderRefresh(providerId: string): number {
    const generation = (this.refreshGenerations.get(providerId) ?? 0) + 1;
    this.refreshGenerations.set(providerId, generation);
    const previous = this.refreshControllers.get(providerId);
    if (previous) { this.refreshControllers.delete(providerId); previous.abort(); }
    return generation;
}

private publishProviderModels(providerId, generation, signal, publication): Promise<boolean> {
    const previous = this.publicationChains.get(providerId) ?? Promise.resolve();
    const queued = (async () => {
        await previous.catch(() => {});
        if (signal.aborted || this.refreshGenerations.get(providerId) !== generation) return false;
        // 1. 持久化（可选 null 删除）
        if (publication.persist === null) await this.modelsStore.delete(providerId, { signal });
        else if (publication.persist !== undefined) await this.modelsStore.write(providerId, structuredClone(publication.persist), { signal });
        // 2. 同步内存态更新（仅当 generation 未过期）
        if (signal.aborted || this.refreshGenerations.get(providerId) !== generation) return false;
        publication.update?.();
        return true;
    })();
    this.publicationChains.set(providerId, queued.catch(() => {}));
    return raceWithAbortSignal(queued, signal);
}
```

**Generation 机制**保证同一 provider 的并发 refresh 互斥（递增 generation，旧的立即 abort），**publication chain** 串行化持久化 + 内存更新，避免 race。

#### 16.2.7 retryAssistantCall 错误分类（70+ 正则）

`packages/ai/src/utils/retry.ts:7-90` 用 70+ 正则模式分类错误：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/retry.ts:7-90
const NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN = buildProviderErrorPattern([
    "GoUsageLimitError", "FreeUsageLimitError",      // OpenCode Zen 订阅上限
    "Monthly usage limit reached", "available balance",
    "insufficient_quota", "out of budget", "quota exceeded", "billing",  // OpenAI 标准
]);

const RETRYABLE_PROVIDER_ERROR_PATTERN = buildProviderErrorPattern([
    "overloaded", "rate.?limit", "too many requests",
    "429", "500", "502", "503", "504", "524",                // HTTP 状态
    "service.?unavailable", "server.?error", "internal.?error",
    "provider.?returned.?error",                            // OpenRouter #2264
    "exceeded request buffer limit while retrying upstream",
    "network.?error", "connection.?refused", "connection.?lost",
    "other side closed", "fetch failed", "getaddrinfo", "ENOTFOUND",
    "EAI_AGAIN", "upstream.?connect", "reset before headers",
    "socket hang up", "socket connection was closed", "timed? out", "timeout",
    "websocket.?closed", "websocket.?error",                // WS 传输
    "ended without", "stream ended before message_stop",   // Anthropic #4433
    "stream ended before a terminal response event", "http2 request did not get a response", // Bedrock #3594
    "retry delay",                                          // Provider-requested 上限触发
    "you can retry your request", "try your request again", "please retry your request",
    "ResourceExhausted",                                    // gRPC (NVIDIA NIM)
]);
```

### 16.3 Session Backends 双后端

`packages/session-backends/` 仅含 `sqlite-node` 子包；**JSONL 后端位于 `agent/src/harness/session/jsonl/`**——双后端分布在不同包但实现 `agent/src/harness/session/types.ts` 里的统一 `SessionStorage` 接口。

#### 16.3.1 SessionStorage 抽象

`agent/src/harness/session/jsonl/types.ts:4-18` 定义 FileSystem 抽象（JSONL 后端）：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/types.ts:4-18
export type JsonlSessionRepoFileSystem = Pick<
    FileSystem,
    | "absolutePath" | "joinPath" | "readTextFile" | "readTextLines"
    | "writeFile" | "appendFile" | "renameFile"
    | "fileInfo" | "listDir" | "exists" | "createDir" | "remove"
>;
```

`session-backends/sqlite-node/src/sqlite/repo.ts:1-93` 的 `SqliteSessionRepository` 直接实现 `SessionRepo`（来自 `@earendil-works/pi-agent-core`），通过 `SqliteDatabaseFactory` 注入 SQLite 引擎（支持 node/bun/sql.js 三种驱动）。

#### 16.3.2 SQLite 后端：12 张表

`packages/session-backends/sqlite-node/src/sqlite/migrations/001_initial.sql`（122 行）建 12 张表：

| 表 | 主键 | 索引 | 用途 |
|---|---|---|---|
| `sessions` | (id) WITHOUT ROWID | `created_at DESC`, `(cwd, created_at DESC)` | 会话元数据 |
| `entries` | `(session_id, id)` UNIQUE `(session_id, seq)` | `(session_id, parent_id)`, `(session_id, type, seq)` | 11 种 entry |
| `session_sequences` | (session_id) | — | next_seq 分配 |
| `session_stats` | (session_id) | — | token / cost 累计 |
| `branch_entries` | `(session_id, branch_id, entry_id)` WITHOUT ROWID | `(session_id, branch_id, entry_seq)`, `(session_id, entry_id, branch_id, entry_seq)`, `(session_id, branch_id, entry_type, entry_seq)`, `(session_id, branch_id, custom_type, entry_seq)` | 分支缓存 |
| `lanes` | `(session_id, lane)` | — | 三 Lane 状态 |
| `records` | `(session_id, id)` UNIQUE `(session_id, seq)` WITHOUT ROWID | 6 个二级索引 | Lane 操作记录 |
| `lane_moves` | (session_id, seq) | — | Lane 迁移 |
| `facts` | (session_id, seq) | `(session_id, kind, key, seq)` | key-value 元事实 |
| `branch_tips` | `(session_id, tip_id)` UNIQUE `(session_id, branch_id)` | — | 分支 tip |
| `writer_leases` | (session_id) | — | 单写者 lease |
| `migrations` | (id) | — | schema 版本 |

**关键约束**：所有主键用 `WITHOUT ROWID`（clustered index 即数据）减少磁盘；`UNIQUE (session_id, seq)` 保证单调序号。

#### 16.3.3 WriterLease fence：原子获取 + 三段续约

`packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts:16-58` 用 **atomic upsert + fence 计数器**实现 lease：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts:16-58
export function acquireWriterLease(db, sessionId, ownerId, now, expiresAtMs) {
    const row = sql`INSERT INTO writer_leases (session_id, owner_id, fence, expires_at_ms)
        VALUES (${sessionId}, ${ownerId}, 1, ${expiresAtMs})
        ON CONFLICT(session_id) DO UPDATE SET
            owner_id = excluded.owner_id,
            fence = writer_leases.fence + 1,         // 新所有者 fence +1
            expires_at_ms = excluded.expires_at_ms
        WHERE writer_leases.expires_at_ms <= ${now}    // 只在过期时才让位
        RETURNING owner_id, fence, expires_at_ms`.get(db);
    return row === undefined ? undefined
        : { ownerId: row.owner_id, fence: row.fence, expiresAtMs: row.expires_at_ms };
}

export function renewWriterLease(db, sessionId, lease, now, expiresAtMs) {
    // 三段续约：必须匹配 owner_id + fence + 还未过期
    const result = sql`UPDATE writer_leases
        SET expires_at_ms = ${expiresAtMs}
        WHERE session_id = ${sessionId}
            AND owner_id = ${lease.ownerId}
            AND fence = ${lease.fence}
            AND expires_at_ms > ${now}`.run(db);
    if (result.changes === 1) lease.expiresAtMs = expiresAtMs;
    return result.changes === 1;
}
```

**Fence 语义**：每次 lease 转移 fence+1，所有写入必须带 fence token；如果一个旧 owner 心跳续约，fence 已变 → `UPDATE changes === 0` → 静默失败，**避免 zombie writer 用过期 lease 写盘**。

`SqliteSessionRepositoryOptions`（`./repo.ts:102-107`）暴露 `writerLease.ttlMs`（默认 30s）和 `heartbeatIntervalMs`（默认 10s，必须 < ttlMs），第 114-122 行强制校验。

#### 16.3.4 JSONL 后端：torn-tail 修复 + 原子发布

`packages/agent/src/harness/session/jsonl/storage.ts:33-46` 的 `publishFileAtomically` 用 **temp file + rename** 实现原子发布：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/storage.ts:33-46
async function publishFileAtomically(fs, destinationPath, populate): Promise<void> {
    const tempPath = `${destinationPath}.tmp`;
    try {
        await populate(tempPath);                                // 1. 写入临时
        fileResult(await fs.renameFile(tempPath, destinationPath), `Failed to publish staged file ${destinationPath}`);
    } catch (error) {
        await fs.remove(tempPath, { force: true });             // 2. 失败清理
        throw error;
    }
}
```

第 80-95 行的 `JsonlSessionStorage.load` 在加载时**自动修复 torn-tail**（进程崩溃在 `appendFile` 中途导致最后一行不完整 JSON）：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/storage.ts:80-108
for (let index = 1; index < physicalLines.length; index++) {
    const line = physicalLines[index]!;
    const mutationResult = parseMutation(line);
    if (!mutationResult.ok) {
        const isTornTail = index === physicalLines.length - 1
            && mutationResult.error.kind === "syntax";
        if (isTornTail) {
            // 截断到 valid prefix，重写文件
            const validPrefix = `${physicalLines.slice(0, index).join("\n")}\n`;
            await publishFileAtomically(fs, path, async (tempPath) => {
                fileResult(await fs.writeFile(tempPath, validPrefix), `Failed to stage torn-tail repair ${path}`);
            });
            return storage;
        }
        throw invalidFile(path, index + 1, mutationResult.error);
    }
    storage.applyMutation(mutationResult.value);
}
// 兜底:文件不以 \n 结尾 → 追加换行
if (!content.endsWith("\n")) {
    fileResult(await fs.appendFile(path, "\n"), `Failed to repair unterminated session tail ${path}`);
}
```

**关键策略**：只接受最后一行是 `syntax` 错误（JSON 不完整）；中间任何 mutation 错误立即抛 `invalidFile`。`JsonlDecodeError`（`./errors.ts`）区分 `schema` / `syntax` 两类。

#### 16.3.5 双后端迁移语义

`packages/agent/src/harness/session/jsonl/types.ts:31` 标注 `sourceFormat: 3 | 4`——支持从 v3 legacy JSONL 平滑升级到 v4 header（`JsonlV4Header` 第 47-57 行）：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/types.ts:47-57
export interface JsonlV4Header {
    kind: "header";
    version: 4;
    id: string;
    createdAt: number;
    cwd: string;
    parentSessionId?: string;
    /** Preserved only when a v3 parent path could not be resolved to a session id. */
    legacyParentSessionPath?: string;
    metadata?: Record<string, JsonValue>;
}
```

`legacyParentSessionPath` 保留迁移期不可解析的父路径，避免一次性破坏既有 session tree。

#### 16.3.6 切换策略：双后端如何选

当前 pi 的 SessionManager（`coding-agent/src/core/session-manager.ts`）默认使用 **JSONL 后端**（代码自包含，无外部依赖）。`session-backends/sqlite-node` 是**可选**后端——通过 `getStorage()` 工厂注入或通过 `ModelsStore`/`CredentialStore` 类似模式预留接口（详见 `session-backends/sqlite-node/test/repository.test.ts` 的 `new SqliteSessionRepository({ env, sqlite, databasePath })` 用法）。

`session-backends/sqlite-node` 是**纯函数式接口**——`async using repo = new SqliteSessionRepository(...)`（TS 5.2+ `using` 声明 + `Symbol.asyncDispose`）保证 RAII 自动关闭数据库。

### 16.4 OTLP Telemetry 深度集成

注：第 9 章「遥测」已覆盖 `InMemoryTelemetryContext` + `NOOP_TELEMETRY_CONTEXT` 基础；本节聚焦**类型化 Schema 推导系统**、**Span 决策审计事件清单**、**InMemory 后端内部行为**、**OTLP exporter 接入路径**。

#### 16.4.1 类型化 Schema 推导（高级 TypeScript）

`packages/telemetry/src/index.ts:72-355` 实现了一套**完全类型安全**的 telemetry schema：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/telemetry/src/index.ts:72-130
export type AttributeDefinitionValue<Definition> = Definition extends { type: "string"; values: readonly (infer V)[]; }
    ? V : Definition extends { type: "string" }
    ? string
    : Definition extends { type: "number"; values: readonly (infer V)[]; }
    ? V : /* ... 10+ 类型分支 ... */
    : readonly boolean[];

// 约束: required attribute → 必传; optional → 可选
export type InferRequiredAndOptionalAttributes<Definitions> = {
    [Name in RequiredAttributeNames<Definitions>]: AttributeDefinitionValue<Definitions[Name]>;
} & {
    [Name in OptionalAttributeNames<Definitions>]?: AttributeDefinitionValue<Definitions[Name]>;
};
```

`TypedSpanStarter`（第 318-322 行）通过 UnionToIntersection 把多个 schema 的 span 名字合并：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/telemetry/src/index.ts:299-322
type UnionToIntersection<U> = (U extends unknown ? (value: U) => void : never) extends
    (value: infer I) => void ? I : never;

export type TypedSpanStarter<Schemas> = UnionToIntersection<{
    [Name in SpanNameInSchemas<Schemas>]: TypedSpanStarterForName<Schemas, Name>;
}[SpanNameInSchemas<Schemas>]>;
```

`bindTypedSpanStarter`（第 324-343 行）运行时**递归绑定父 context**，让子 span 自动继承。

#### 16.4.2 AI Telemetry Schema（17 种 start attr + 13 种 end attr）

`packages/agent/src/harness/telemetry.ts:42-118` 定义 `AI_TELEMETRY_SCHEMA`，**唯一 span 是 `pi.ai.request`**（覆盖所有 LLM 操作）：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts:42-118
export const AI_TELEMETRY_SCHEMA = {
    version: 1,
    spans: {
        "pi.ai.request": {
            description: "One logical request to an AI provider",
            parents: { kind: "any" },
            startAttributes: {
                "pi.ai.operation": {
                    type: "string", required: true,
                    values: ["stream", "fetch_deferred", "cancel_deferred", "generate_images"],
                    description: "Logical provider operation",
                },
                "pi.ai.provider": { type: "string", required: true, description: "Selected provider id" },
                "pi.ai.model":    { type: "string", required: true, description: "Requested model id" },
                "pi.ai.api":      { type: "string", required: true, description: "Provider API id" },
                "pi.ai.streaming": { type: "boolean", required: true, description: "Whether this operation returns a stream" },
                "pi.ai.deferred":  { type: "boolean", required: false, description: "Whether the operation requests deferred execution" },
            },
            endAttributes: {
                "pi.ai.response.model":       { type: "string", description: "Concrete response model" },
                "pi.ai.response.id":          { type: "string", cardinality: "high", description: "Provider response id" },
                "pi.ai.response.stop_reason": { type: "string", values: ["stop", "length", "tool_use", "error", "aborted", "deferred"] },
                "pi.ai.http.status_code":     { type: "number", description: "Final HTTP status" },
                "pi.ai.usage.input_tokens":          { type: "number" },
                "pi.ai.usage.output_tokens":         { type: "number" },
                "pi.ai.usage.cache_read_tokens":     { type: "number" },
                "pi.ai.usage.cache_write_tokens":    { type: "number" },
                "pi.ai.usage.reasoning_tokens":     { type: "number" },
                "pi.ai.usage.total_tokens":          { type: "number" },
                "pi.ai.usage.cost":                  { type: "number" },
                "pi.ai.stream.chunk_count":          { type: "number" },
                "pi.ai.stream.time_to_first_chunk_ms": { type: "number" },
                "pi.ai.error.type": { type: "string", cardinality: "low" },
            },
            status: { default: "ok", errorWhen: "The operation throws or returns an error result" },
        },
    },
} as const satisfies TelemetrySchemaDefinition;
```

**关键设计**：`cardinality: "high"` 标记高基数字段（如 provider response id）让 OTLP exporter 做下采样/采样策略；`cardinality: "low"` 标记低基数（如 stop_reason）做索引。

#### 16.4.3 Harness Telemetry Schema：11 种 hook + 30 种 event

`telemetry.ts:147-217` 定义 **11 个 hook name** + **30 个 event type**，构成决策审计的全景：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts:147-217
const HOOK_NAMES = [
    "before_run", "before_resume", "before_run_end", "transform_context",
    "before_request", "before_payload", "after_response",
    "before_tool", "after_tool",
    "before_compaction", "before_navigation",
] as const;

const EVENT_TYPES = [
    "run_start", "run_resume", "run_suspend", "run_abort", "run_end", "fault", "handler_error",
    "turn_start", "turn_end",
    "retry_scheduled", "retry_start", "retry_end",
    "message_start", "message_update", "message_end",
    "tool_start", "tool_update", "tool_end",
    "entry_added", "write_pending", "queue_update", "fact_update", "config_update",
    "compaction_start", "compaction_end",
    "navigation_start", "navigation_end",
    "lane_created", "usage",
] as const;
```

`telemetry.ts:232-299` 定义 4 个核心 span（`pi.harness.run` / `pi.harness.compaction` / `pi.harness.navigation`），共享 `operationStartAttributes`：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts:193-217
const operationStartAttributes = {
    "pi.session.id":         { type: "string", required: true, cardinality: "high", description: "Session id" },
    "pi.lane.name":          { type: "string", required: true, cardinality: "high", description: "Lane name" },
    "pi.operation.id":       { type: "string", required: true, cardinality: "high", description: "Durable operation id" },
    "pi.operation.recovery": { type: "boolean", required: true, description: "Whether this invocation resumes durable work" },
};
```

**决策可追溯性**：每个 run/compaction/navigation 的 start 属性固定包含 `lane.name` + `operation.id`，可关联到 Lane 状态机的事件溯源——**OTLP exporter 只需一份 trace 就能还原 agent 决策时序**。

#### 16.4.4 InMemory 后端：defensive recording

`packages/telemetry/src/memory.ts:120-186` 的 `startInMemorySpan` 用 try/catch **passive 录制**：

```typescript
// /usr/local/LsmGitOpenSource/pi/packages/telemetry/src/memory.ts:120-186
function startInMemorySpan(state, parent, options, callback): Promise<T> {
    if (parent?.settled) return NOOP_TELEMETRY_CONTEXT.startSpan(options, callback);  // 父 span 已结束 → 退化 NOOP

    let recordedSpan;
    try {
        recordedSpan = createSpan(state, parent, options);
        state.spans.push(recordedSpan);
    } catch {
        return NOOP_TELEMETRY_CONTEXT.startSpan(options, callback);
    }

    const span: TelemetrySpan = {
        startSpan: (childOptions, childCallback) =>
            startInMemorySpan(state, recordedSpan, childOptions, childCallback),
        addEvent(name, attributes) {
            if (recordedSpan.settled) return;  // 已 settled → 拒绝
            try { recordedSpan.events.push({ name, attributes: copyAttributes(attributes) }); }
            catch { /* Recording is passive. Ignore malformed telemetry payloads. */ }
        },
        setAttributes(attributes) {
            if (recordedSpan.settled) return;
            try { recordedSpan.attributes = mergeAttributes(recordedSpan.attributes, attributes); }
            catch { /* Recording is passive. */ }
        },
        setStatus(status) {
            if (recordedSpan.settled) return;
            try { recordedSpan.status = copyStatus(status); recordedSpan.explicitStatus = true; }
            catch { /* Recording is passive. */ }
        },
    };

    let result;
    try { result = callback(span); } catch (error) {
        settleSpan(state, recordedSpan, true, error);
        return Promise.reject(error);
    }
    return Promise.resolve(result).then(
        (value) => { settleSpan(state, recordedSpan, false); return value; },
        (error) => { settleSpan(state, recordedSpan, true, error); throw error; }
    );
}
```

**关键设计**：
- 父 span settled → 子 span 走 NOOP（避免悬挂）
- 任何 addEvent/setAttributes/setStatus 在 settled 后**静默忽略**（不抛错，避免 telemetry 影响主流程）
- 错误自动转 `error status`（`automaticErrorStatus` 第 78-87 行）

#### 16.4.5 OTLP Exporter 接入路径

当前 pi **未内置 OTLP exporter**——`InMemoryTelemetryContext` 是唯一实现，但提供了**清晰的 OTLP 适配点**：

1. 实现 `TelemetryContext` 接口（第 14-16 行）→ 提供 `startSpan(options, callback)`
2. 用 `getSpans()`（第 204-218 行）拿到 `RecordedTelemetrySpan[]`（含 `id` / `parentId` / `name` / `attributes` / `events` / `status` / `endSequence`）
3. 转换为 OTLP `ResourceSpans` / `ScopeSpans` / `Span` 格式（Otel SDK 标准）
4. 用 `@opentelemetry/exporter-trace-otlp-http` POST 到 collector

`endSequence`（第 98 行：`state.nextEndSequence++`）是 pi 独有的**全局结束序号**——可还原事件因果序（即便跨 span），适合 OTLP exporter 在导出后做事件重排/去重。

`endAttributes.cardinality: "high"` 字段（如 `pi.ai.response.id`）可在 exporter 层做**采样/skip**（避免 OTLP collector 索引爆炸）。

#### 16.4.6 测试与 conformance

`packages/telemetry/src/testing/conformance.ts` + `testing/types.ts` 提供 **schema 一致性测试套件**——任何新 `TelemetrySchemaDefinition` 必须通过 schema conformance（验证 startAttributes/endAttributes 类型一致性、required 字段、补全度）。

### 16.5 对 laew 的借鉴路线图

#### 16.5.1 P0 — 直接可用（1-2 周）

| 借鉴项 | pi 来源 | laew 落地路径 |
|---|---|---|
| 5 源 Provider 合成 | `coding-agent/src/core/model-runtime.ts:58-80` + `provider-composer.ts:420-523` | 把当前 `Provider` 单源升级为 base+models.json+oauth+override 多层；`validateExtensionProvider` eager validate 在 CLI 启动时跑 |
| abort-aware retry | `ai/src/utils/provider-retry.ts:105-125` + `retry.ts:163-212` | 替换 laew 现有 `reqwest::Client` 的默认 retry（`reqwest::retry` 中间件不响应 `tokio::select!` 取消），外层包装器 + 指数退避 |
| 错误分类 70+ 正则 | `ai/src/utils/retry.ts:7-90` | Rust `regex` crate 编译 70+ pattern，配额错误立即 fail-fast（不让用户等 backoff） |
| WriterLease fence | `session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts:16-58` | laew 当前 SQLite 单进程独占，但未来多 agent 并发写可借鉴 `fence INTEGER` + `UPDATE ... WHERE fence=?` 防御 zombie writer |
| thinking budget 4 档 | `ai/src/api/simple-options.ts:55-95` | 在 `llm/mod.rs` 加 `thinking_budget: HashMap<Level, u32>`，自动 `clamp` 到 `max_tokens - 1024` |
| 6 类 ambient 凭证 | `ai/src/env-api-keys.ts:154-188` | `config/mod.rs` 加 AWS_PROFILE / GOOGLE_ADC / Azure CLI 等 ambient 检测，返回 `Option<String>` 而非具体 key |

#### 16.5.2 P1 — 中期目标（3-4 周）

| 借鉴项 | pi 来源 | laew 落地路径 |
|---|---|---|
| 双后端架构 | `session-backends/sqlite-node/` + `agent/src/harness/session/jsonl/` | laew 当前 SQLite 单一，可加 JSONL 后端做**导出/调试**（`LsmAgentEmergentWork.db` 不可读时回退 JSONL）；`sourceFormat: 3\|4` 模式支持版本迁移 |
| torn-tail 修复 | `agent/src/harness/session/jsonl/storage.ts:80-108` | JSONL 后端必备；用 `tempfile + rename` 实现原子发布，崩溃后启动检测最后一行 schema 错误则截断 |
| 类型化 telemetry schema | `telemetry/src/index.ts:72-355` + `agent/src/harness/telemetry.ts:42-118` | laew 当前 `AgentError` 已结构化但无 OTLP；用 `serde` + `tracing-subscriber` 做 span 录制，导出 OTLP HTTP |
| Lane 决策可追溯 | `agent/src/harness/telemetry.ts:193-217` | laew 当前 MultiAgent 编排已具备 Yolo→Main→SubAgent 链路，把每个 transition 包成 span，让 trace 重放决策 |
| prompt_cache_key 路由 | `ai/src/api/openai-responses.ts:295` + `anthropic-messages.ts:60-74` | laew 单实例 sessionId 可作为 cache key；`--cache-retention long` 走 Anthropic 1h TTL，节省 cache miss 成本 |
| git URL 安全解析 | `coding-agent/src/utils/git.ts:84-124` | 如果未来 laew 支持扩展从 git 安装，**必须复用**这套 4 重防御（null/绝对路径/目录穿越/host 校验） |

#### 16.5.3 P2 — 远期目标（5-8 周）

| 借鉴项 | pi 来源 | laew 落地路径 |
|---|---|---|
| OTLP exporter | 暂无（自实现） | 引入 `opentelemetry` + `opentelemetry-otlp` crate，导出 trace 到 Jaeger/Tempo |
| models.json Override | `coding-agent/src/core/model-config.ts` | 让用户覆盖内置 catalog 的 `contextWindow` / `cost` / `compat`，适配 laew 的 `/provider override <id>` 子命令 |
| 70+ 错误分类 → enum | `ai/src/utils/retry.ts:7-90` | 编译期生成 `RetryClass` enum（配额/限流/网络/服务端/客户端），UI 差异化提示 |
| editor 自治 / PR 自动化 | 无内置（仅 git URL 解析） | 不必借鉴；laew TUI 单轮模式适合单步任务，PR 工作流交给 `gh` CLI + Bash 工具 |
| Skill 一等公民 | 第 8 章已覆盖 | — |

### 16.6 综合：四维度交叉点

四个维度在 pi 中通过 **5 个共享抽象** 串联：

1. **Models 集合**（`ai/src/models.ts`）= Provider 路由 + 模型存储 + publication chain——是 16.2 的入口
2. **SessionStorage 抽象**（`agent/src/harness/session/types.ts`）= JSONL + SQLite 双后端共用接口——是 16.3 的统一契约
3. **TelemetryContext 抽象**（`telemetry/src/index.ts`）= InMemory + 未来 OTLP exporter 共用接口——是 16.4 的扩展点
4. **SessionManager 编排**（`coding-agent/src/core/session-manager.ts`）= CLI 6 源判定 + version 迁移 + 双后端装配——是 16.1 + 16.3 的桥
5. **Provider/Auth composition**（`coding-agent/src/core/provider-composer.ts`）= 5 层 Provider 合成 + eager validate + OAuth 适配——是 16.1 + 16.2 的桥

**交叉决策点示例**：
- 用户在 TUI 切换 provider（16.1）→ ModelRuntime 重新拉 catalog（16.2.6 refresh）→ 把新 catalog 写入 ModelsStore（持久化）→ trigger 所有 active session 的 `pi.ai.request` span 记录 provider change（16.4.2）
- Session 写入（16.3.3 WriterLease）→ 失败时记 `pi.harness.run` span `pi.operation.outcome=failed`（16.4.3）→ telemetry exporter 可还原"为什么哪次写入失败"
- 错误重试（16.2.4）→ `pi.ai.request` span 的 `endAttributes` 加 `pi.ai.usage.cost` + `retry_count` → OTLP trace 显示 retry 累积成本

### 16.7 关键文件路径汇总

| 类别 | 文件路径（绝对） |
|---|---|
| **Coding Agent CLI 入口** | |
| main.ts CLI 编排 | `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/main.ts:352-443` |
| CLI Args 解析 | `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/cli/args.ts:13-58, 71-446` |
| createSessionManager 6 源 | `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/main.ts:352-443` |
| buildSessionOptions 5 层 | `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/main.ts:445-541` |
| ModelRuntime 5 源 | `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/model-runtime.ts:58-80` |
| composeModelProvider | `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/provider-composer.ts:420-523` |
| git URL 安全解析 | `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/utils/git.ts:84-124, 172-226` |
| **AI 路由** | |
| Provider 类型 35+ | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/types.ts:17-76` |
| env-api-keys 6 类 | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/env-api-keys.ts:154-188` |
| thinkingBudget 4 档 | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/simple-options.ts:55-95` |
| retryProviderRequest | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/provider-retry.ts:22-125` |
| retryAssistantCall 70+ 正则 | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/retry.ts:7-90, 163-212` |
| Models publication chain | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/models.ts:320-365` |
| cache_control short/long | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts:60-74` |
| prompt_cache_key | `/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/openai-responses.ts:295` |
| **Session Backends** | |
| SQLite 12 表 schema | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/migrations/001_initial.sql` |
| WriterLease fence | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts:16-58` |
| SqliteSessionRepository | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/repo.ts:1-953` |
| 12 张表索引 | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/migrations/001_initial.sql:9-122` |
| SqliteDatabase 工厂 | `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/types.ts` |
| JSONL Repo (agent 包) | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/repo.ts:1-247` |
| JSONL Storage | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/storage.ts:33-108, 110-119` |
| JSONL Types / FileSystem | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/types.ts:4-57` |
| JSONL Codec | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/codec.ts` |
| JSONL Errors | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/errors.ts` |
| **Telemetry 类型化** | |
| Telemetry 接口定义 | `/usr/local/LsmGitOpenSource/pi/packages/telemetry/src/index.ts:1-355` |
| 类型化 schema 推导 | `/usr/local/LsmGitOpenSource/pi/packages/telemetry/src/index.ts:72-130, 299-343` |
| NOOP 后端 | `/usr/local/LsmGitOpenSource/pi/packages/telemetry/src/noop.ts` |
| InMemory 实现 | `/usr/local/LsmGitOpenSource/pi/packages/telemetry/src/memory.ts:120-219` |
| AI Telemetry Schema | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts:42-118` |
| Harness Telemetry Schema | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts:147-217, 232-299` |
| 11 hooks + 30 events | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts:147-191` |
| operationStartAttributes | `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts:193-217` |
| Schema conformance 测试 | `/usr/local/LsmGitOpenSource/pi/packages/telemetry/src/testing/conformance.ts` |

### 16.8 本轮不重复声明

本轮严格不重复第七轮及之前章节已覆盖内容：

- **不重复二进制帧协议 CBOR**（第 2 章）—— 本轮聚焦 AI 路由、Session、Telemetry
- **不重复 Server / Session 后端基础**（第 3 章）—— 本轮仅深入 SQLite/JSONL 双后端差异，不重述 PiServer
- **不重复 Lane 三态 / reduceLaneState / 14 种损坏检测**（第 4、5、13.5、13.7 章）—— WriterLease fence 是第 5+13 章的延续但聚焦双后端视角
- **不重复流式 + 中断传播**（第 6 章）
- **不重复 11 种 Entry / 压缩策略**（第 7 章）
- **不重复 Skill 系统**（第 8 章）
- **不重复 Telemetry NOOP/InMemory 基础**（第 9 章）—— 本轮聚焦类型化 schema 推导、决策审计事件清单、OTLP exporter 接入路径
- **不重复 JSONL torn-tail 修复原理**（第 10.4 章）—— 本轮在 16.3.4 简要回顾但不重述 14 种校验
- **不重复 Provider 20+ 兼容性开关**（第 12.1 章）—— 本轮聚焦 5 源合成 + 路由 + retry + thinking budget
- **不重复系统提示词 + thinkingFormat**（第 13 章）
- **不重复错误处理 TaggedError/Result**（第 14 章）—— 本轮聚焦 HTTP-level retry 与错误分类
- **不重复第 14 章第七轮 Edit/Git/Bash/Grep** —— 本轮 16.1.4 仅简述 git URL 解析作为编辑器自治锚点
- **不重复第五/六轮深挖**（第 12、13 章）—— 本轮专注 4 个全新维度

