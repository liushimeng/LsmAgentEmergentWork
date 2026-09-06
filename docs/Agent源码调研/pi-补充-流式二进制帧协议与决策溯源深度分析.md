# Pi 补充：流式协议翻译 / 二进制帧协议 / 决策溯源深度分析

> 本文档是对《pi-第四轮-codingAgent与evals深度分析》的**独立补充**，聚焦三个未在前轮充分覆盖的维度：
> 1. 流式协议翻译管线（SSE ↔ AssistantMessageEvent ↔ AgentEvent）
> 2. 客户端-服务器二进制帧协议（CBOR + 4 字节 length-prefixed）
> 3. 决策溯源 / 因果链（双后端 JSONL+SQLite + WriterLease fence + schema-typed telemetry）

---

## 维度 1：流式协议翻译管线（SSE ↔ 内部事件）

### SSE chunk 解析
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts`
- `decodeSseLine(line, state)` — 解析一行 SSE（event/data/comment 处理）
- `nextLineBreakIndex(text)`, `consumeLine(text)` — 按 `\r`, `\n`, `\r\n` 分行
- `iterateSseMessages(body, signal)` — `AsyncGenerator<ServerSentEvent>` 通过 `TextDecoder({ stream: true })` 读 `ReadableStream<Uint8Array>`，带内部 buffer
- `iterateAnthropicEvents(response, signal)` — 从 SSE 解码 `RawMessageStreamEvent`
```ts
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
  ...
}
```

### 内部事件类型定义
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/ai/src/types.ts`
- `AssistantMessageEvent` — union：`start | text_start | text_delta | text_end | thinking_start | thinking_delta | thinking_end | toolcall_start | toolcall_delta | toolcall_end | done | error`
- `Transport = "sse" | "websocket" | "websocket-cached" | "auto"`

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/event-stream.ts`
- `EventStream<T, R>` — 通用 async-iterable queue，带 waiter-based backpressure（push/end/result）
- `AssistantMessageEventStream extends EventStream<AssistantMessageEvent, AssistantMessage>`

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/types.ts`
- `AgentEvent` — union：`agent_start | agent_end | turn_start | turn_end | message_start | message_update | message_end | tool_execution_start | tool_execution_update | tool_execution_end`

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/agent-session.ts`
- `AgentSessionEvent` — 扩展 `AgentEvent` 加 `agent_settled | queue_update | compaction_start | entry_appended | session_info_changed | thinking_level_changed | compaction_end | auto_retry_start | ...`

### 翻译管线（SSE → 内部事件 → UI 渲染）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/agent-loop.ts`
- `streamAssistantResponse(context, config, signal, emit, streamFunction)` — 把 `AssistantMessageEvent` 通过 `emit({ type: "message_update", assistantMessageEvent: event, message })` 转为 `AgentEvent`
- `runLoop(...)` — 内/外 loop 分派 tool calls

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/modes/json-event.ts`
- `toJsonEvent(event)` — 剥离累积 `partial` snapshot；保留 delta + usage
- `JsonAgentSessionEvent` — JSON 有线事件形状

### Partial JSON / 增量 tool_args 解析
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/json-parse.ts`
- `parseStreamingJson<T>(partialJson)` — 尝试 `parseJsonWithRepair` → `partialParse`（from `partial-json` lib） → `partialParse(repairJson(...))`
- `repairJson(json)` — 转义控制字符、修复坏转义
```ts
export function parseStreamingJson<T>(partialJson): T {
  if (!partialJson || partialJson.trim() === "") return {} as T;
  try { return parseJsonWithRepair<T>(partialJson); }
  catch { try { return partialParse(partialJson) ?? {}; }
  catch { try { return partialParse(repairJson(partialJson)) ?? {}; }
  catch { return {} as T; }}}
}
```
**文件（Anthropic SSE）：** `anthropic-messages.ts` — `input_json_delta` 累积 `block.partialJson += event.delta.partial_json; block.arguments = parseStreamingJson(block.partialJson);` 然后 emit `toolcall_delta`

### Ctrl-C / AbortController / 中断传播
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/utils/abort.ts`
- `operationSignal(signal)`, `raceWithAbortSignal(operation, signal)` — 通过 `signal.reason` abort
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/agent-loop.ts` — `executeToolCallsSequential`, `executePreparedToolCall`, `prepareToolCall` 中的 `signal?.aborted` 检查
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts` — `iterateSseMessages` 每次读检查 `signal?.aborted`；`stream` 在循环后检查 `options?.signal?.aborted`

### 背压 / 缓冲策略
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/output-guard.ts`
- `writeRawStdout`, `waitForRawStdoutBackpressure`, `flushRawStdout` — promise-chained stdout 带 ENOBUFS/EAGAIN/EWOULDBLOCK 重试
- `takeOverStdout()` / `restoreStdout()` — 把 `process.stdout.write` 重定向到 stderr
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts` — `unsubscribeBackpressure = session.agent.subscribe(async () => { await waitForRawStdoutBackpressure(); ... })`
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/modes/print-mode.ts` — 相同背压模式
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/event-stream.ts` — `EventStream` 用 `queue[]` + `waiting[]` resolver list（pull-based backpressure）

---

## 维度 2：客户端-服务器二进制帧协议

（注意：本仓库**无** `A2A`/`ACP`/`E2A`/`A2UI` 术语 — 这些名称不存在。实际存在的是健壮的**客户端-服务器二进制帧协议**。）

### 帧格式 — length-prefixed, 4 字节 big-endian 头
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/framing.ts`
- `FRAME_HEADER_LENGTH = 4`, `PAYLOAD_BLOCK_SIZE = 64 * 1024`, `DEFAULT_MAX_FRAME_LENGTH = 16 * 1024 * 1024`
- `encodeFrame(payload)` — 用 uint32 BE length 作前缀
- `FrameDecoder` — 增量 chunk→frame 分离器；`push(chunk): Uint8Array[]`, `end()`
- `FrameError`, `assertCompleteFrame`
```ts
export function encodeFrame(payload: Uint8Array): Uint8Array {
  const frame = new Uint8Array(FRAME_HEADER_LENGTH + payload.byteLength);
  frame[0] = length >>> 24; frame[1] = length >>> 16; frame[2] = length >>> 8; frame[3] = length;
  frame.set(payload, FRAME_HEADER_LENGTH);
  return frame;
}
```

### 序列化 — 严格 RFC 8949 CBOR
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/cbor/encoder.ts`, `decoder.ts`, `options.ts`
- `encodeCbor(value, options)`, `decodeCbor(bytes, options)` — definite-length 子集；限制：`maxByteLength`, `maxContainerLength`, `maxDepth`
- `CborError`, `CborOptions`

### Codec — 有验证的帧协议消息
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/codec.ts`
- `encodeClientMessage`, `encodeServerMessage` — 通过 TypeBox schema + CBOR + frame 验证
- `ClientMessageDecoder`, `ServerMessageDecoder` — 增量验证解码器
- `ProtocolValidationError`, `parseClientMessage`, `parseServerMessage`, `isSupportedProtocolVersion`

### Schema — 完整协议消息词汇
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/protocol/src/schemas.ts`
- `PROTOCOL_VERSION = 1`
- **Client→Server:** `ClientHello { type:"hello", version }`, `RequestEnvelope { type:"request", id, request }`, `ClientMessage = ClientHello | RequestEnvelope`
- **Server→Client:** `ServerHello`, `ServerHelloError`, `ResponseEnvelope { type:"response", id, ok, result|error }`, `EventEnvelope { type:"event", event }`, `ServerMessage = ServerHello | ServerHelloError | ResponseEnvelope | EventEnvelope`
- **Commands:** `list | create | attach | detach | prompt | steer | abort | set_model | set_thinking`
- **Server events:** `server_snapshot | session_snapshot | session_progress | session_progress | session_removed`
- **Transcript:** `TranscriptItem = UserTranscriptItem | AssistantTranscriptItem | ToolTranscriptItem` with statuses `streaming | complete | error | aborted | running`
- `SessionPhase = "idle" | "turn" | "compaction" | "branch_summary" | "retry"`
- `ThinkingLevel`, `ModelRef`, `ModelMetadata`, `Usage`, `JsonValue`, `SessionSnapshot`, `ServerSnapshot`

### Server 端生命周期（5 阶段连接状态）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/server/src/connection.ts`
```ts
export type ConnectionStage = "awaitingHello" | "handshaking" | "ready" | "closing" | "closed";
export interface ConnectionState { id; connection: ByteConnection; decoder: ClientMessageDecoder; sessionIds; stage; disconnected; handshakeComplete; handshake?; handshakeTimeout; }
```
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/server/src/server.ts`
- `PiServer.accept()` → `receive()` → `dispatchMessage()` → `finishHandshake()` → `handleRequest()`
- `sendMessage()` 编码 `ServerMessage` 并调用 `connection.send(frame)`
- `failProtocol()` 发送 `ServerHelloError` 然后关闭

### Client 端连接生命周期
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/client/src/connection.ts`
- `ConnectionLifecycle`: `disconnected | connecting | connected`
- `Connection.connect()` 打开 transport，发送 `hello`，等待 `ServerHello`
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/client/src/client.ts`
- `PiClient` — 通过 `#pendingRequests` 的 request/response 关联、session leases、event 订阅
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/client/src/session-handle.ts` — `PiSessionHandle` 带 shared/exclusive lease 模式

### Server→AI 协议适配器（AI messages → protocol TranscriptItems）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/server/src/protocol.ts`
- `toProtocolUserMessage`, `toProtocolAssistantMessage`, `toProtocolToolResultMessage`, `toProtocolUsage`, `toProtocolModelMetadata`, `toProtocolJsonValue`, `sanitizeProtocolDetails`

### RPC mode（headless JSON-line protocol）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/modes/rpc/rpc-types.ts` — `RpcCommand`, `RpcResponse`, `RpcSessionState`, `RpcExtensionUIRequest`, `RpcExtensionUIResponse`
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/modes/rpc/jsonl.ts` — `serializeJsonLine`, `attachJsonlLineReader`（严格 LF-only JSONL 帧，非 Node readline）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/modes/rpc/rpc-mode.ts` — 把 `AgentSession` 事件接线到 JSON-line 响应 + 背压

---

## 维度 3：决策溯源 / 因果链

### LLM 调用输入/输出日志
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts`
- `stream` 捕获 `responseId`, `model`, `responseModel`, `usage`, `stopReason`, `rawStopReason`, `errorMessage`, `diagnostics`, `thinkingSignature`
- `onPayload`, `onResponse` 回调用于 request/response 检查

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/messages.ts` — harness 消息类型

### Span / trace 关联（类型化 telemetry）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/telemetry/src/index.ts`
- `TelemetryContext.startSpan(options, callback)`, `TelemetrySpan`（`addEvent`, `setAttributes`, `setStatus`）
- `createTypedSpanStarter<Schemas>(telemetryContext, schemas)` — schema-bound typed span starter
- `TelemetrySchemaDefinition`, `TelemetrySpanDefinition`, `TelemetryEventDefinition`, `TelemetryParentDefinition`（`any | root_or_external | spans`）

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/telemetry/src/memory.ts`
- `InMemoryTelemetryContext` — 记录 span；`RecordedTelemetrySpan` 带 `id`, `parentId`, `name`, `attributes`, `events`, `status`, `settled`, `endSequence`

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/telemetry.ts`
- `AI_TELEMETRY_SCHEMA` — `pi.ai.request` span：`pi.ai.operation, provider, model, api, streaming, deferred, response.*, usage.*, stream.chunk_count, stream.time_to_first_chunk_ms, error.type`
- `HARNESS_TELEMETRY_SCHEMA` — spans：`pi.harness.run`, `pi.harness.compaction`, `pi.harness.navigation`, `pi.harness.checkpoint`, `pi.harness.turn`, `pi.harness.step`, `pi.harness.tool`, `pi.harness.hook`, `pi.harness.sleep`, `pi.harness.event_handler`, `pi.session.write`
- Parent links：`pi.harness.turn` → `pi.harness.run`；`pi.harness.step` → `turn|checkpoint|compaction|navigation`；`pi.harness.tool` → `turn|run`；`pi.harness.checkpoint` → `run`
- Event types 枚举：`run_start, run_resume, run_suspend, run_abort, run_end, fault, handler_error, turn_start, turn_end, retry_scheduled, retry_start, retry_end, message_start, message_update, message_end, tool_start, tool_update, tool_end, entry_added, write_pending, queue_update, fact_update, config_update, compaction_start, compaction_end, navigation_start, navigation_end, lane_created, usage`
- Hook names：`before_run, before_resume, before_run_end, transform_context, before_request, before_payload, after_response, before_tool, after_tool, before_compaction, before_navigation`

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/scripts/generate-telemetry-docs.ts` — 从 schema 生成文档

### 审计日志 / 不可篡改存储 — 双后端（JSONL + SQLite）

#### JSONL 后端
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/storage.ts`
- `JsonlSessionStorage` — append-only JSONL；`publishFileAtomically()` 做 tmp-write + atomic rename；`enqueue()` 序列化写入；load 时 torn-tail repair
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/codec.ts`
- `encodeHeader(header) = \`${JSON.stringify(header)}\n\``, `encodeMutation(mutation)` — JSONL codec
- `parseHeader`, `parseMutation` — 验证 `kind: "header"`, `version: 4`, entry/record/lane/fact mutation kinds
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/types.ts`
- `JsonlV4Header { kind:"header", version:4, id, createdAt, cwd, parentSessionId?, legacyParentSessionPath?, metadata? }`
- `JsonlSessionMetadata { id, createdAt, cwd, path, modifiedAt, sourceFormat: 3|4, ... }`
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/jsonl/repo.ts`
- `JsonlSessionRepo` — `create/open/list/delete/fork`；`claimCreateDestination` 阻止同进程 create 竞争

#### SQLite 后端 + WriterLease fence
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts`
```ts
export interface WriterLease { ownerId: string; fence: number; expiresAtMs: number; }
export function acquireWriterLease(db, sessionId, ownerId, now, expiresAtMs)  // INSERT ... ON CONFLICT ... WHERE expires_at_ms <= now
export function renewWriterLease(db, sessionId, lease, now, expiresAtMs)       // UPDATE ... WHERE owner_id AND fence AND expires_at_ms > now
export function releaseWriterLease(db, sessionId, lease)                       // DELETE WHERE session_id AND owner_id AND fence
export function deleteWriterLease(db, sessionId)
```
- 单调 `fence` 在每次 re-claim 时递增 → stale writer 被 fence off

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/session-backends/sqlite-node/src/sqlite/repo.ts`
- `SqliteWriterLeaseOptions { ttlMs?, heartbeatIntervalMs? }`（默认 ttl 30s, heartbeat 10s）
- `SqliteSessionStorage` — `SerialOperationQueue` + `claimWriterLease` + 定期 `scheduleHeartbeat()`；每个 `enqueueWrite` 在事务内 renew lease，renew 失败抛 `lostWriterError`
- `configureSqliteDatabase`：`PRAGMA journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=5000`

### Session 模型（因果链）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/types.ts`
- `Entry = MessageEntry | ModelChangeEntry | ThinkingLevelEntry | ActiveToolsEntry | CompactionEntry | BranchSummaryEntry | CustomEntry` — 每个带 `id, seq, parentId, timestamp`
- `LaneRecord = OperationStartedRecord | AbortRequestedRecord | OperationFinishedRecord | StepAttemptRecord | ToolStartedRecord | QueueEnqueuedRecord | QueueCancelledRecord | WriteDeferredRecord | UsageRecord` — 持久因果记录
- `SessionStorage` interface：`appendEntry`, `appendRecord`, `findEntries`, `findEntriesOnBranch`, `findRecords`, `findOpenOperations`, `getLog`

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/session/session.ts`
- `Session` — `view(lane)` 返回作用域到 branch 的 `SessionTree`；`assertJsonSerializable` 强制 plain JSON；`commitEntry`/`commitRecord` 持久追加

**文件:** `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/agent-harness.ts`
- `AgentHarness` — `RunOutcome`, `CompactionOutcome`, `NavigationOutcome`, `SuspendedOperation`, `LaneSnapshot`, `SessionSnapshot`, `ActionInfo`（因果 action 枚举）
- `LaneBusy`, `NoActiveRun`, `HarnessFault`, `HarnessClosed` errors

### 可视化生成
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/export-html/index.ts`
- `exportSessionToHtml(sm, state, options)`, `exportFromFile(inputPath, options)` — 生成自包含 HTML，base64 编码 session 数据、theme vars、Marked + Highlight.js
- `preRenderCustomTools` — 预渲染 extension tools 到 HTML
- `generateHtml(sessionData, themeName)` — 注入 CSS/JS/SESSION_DATA 到模板
- `parseColor`, `getLuminance`, `adjustBrightness`, `deriveExportColors` — theme-aware 颜色推导
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/export-html/tool-renderer.ts` — `createToolHtmlRenderer`
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/export-html/ansi-to-html.ts` — ANSI→HTML 转换
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/modes/interactive/components/mermaid.ts` — Mermaid 图渲染（code-fence 检测带 backtick run-length）
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/tui/src/components/markdown.ts` — markdown TUI 渲染
**文件:** `/usr/local/LsmGitOpenSource/pi/packages/tui/src/components/editor.ts` — editor 组件

---

## 关键发现小结

- **无 A2A/ACP/E2A/A2UI 术语** — 实际进程间协议是 **pi 二进制帧协议**（CBOR + 4 字节 length prefix）在 `PiClient` ↔ `PiServer` 之间，加上 headless **RPC JSON-line mode**（`rpc-mode.ts`）
- 因果链通过**双后端 append-only sessions**（JSONL + SQLite）实现，带 **WriterLease fencing**（`fence` counter, TTL, heartbeat）阻止 split-brain writers
- Telemetry 是 **schema-typed**（`TelemetrySchemaDefinition`），带显式 parent-span 链接形成 trace tree
- SSE 翻译是 provider-specific（Anthropic 在 `anthropic-messages.ts`），归一化为统一 `AssistantMessageEventStream` 被 agent loop 消费

---

## 关键文件路径索引

- `ai/src/api/anthropic-messages.ts`、`ai/src/types.ts`、`ai/src/utils/{event-stream,json-parse}.ts`
- `agent/src/agent-loop.ts`、`agent/src/types.ts`、`agent/src/harness/{messages,telemetry}.ts`
- `agent/src/harness/session/{types,session}.ts`、`agent/src/harness/agent-harness.ts`
- `agent/src/harness/session/jsonl/{storage,codec,types,repo}.ts`
- `protocol/src/{framing,codec,schemas}.ts`、`protocol/src/cbor/{encoder,decoder,options}.ts`
- `server/src/{connection,server,protocol}.ts`
- `client/src/{connection,client,session-handle}.ts`
- `session-backends/sqlite-node/src/sqlite/storage/writer-leases.ts`、`session-backends/sqlite-node/src/sqlite/repo.ts`
- `coding-agent/src/core/agent-session.ts`、`coding-agent/src/utils/abort.ts`、`coding-agent/src/core/output-guard.ts`
- `coding-agent/src/modes/json-event.ts`、`coding-agent/src/modes/rpc/{rpc-types,jsonl,rpc-mode}.ts`、`coding-agent/src/modes/print-mode.ts`
- `coding-agent/src/core/export-html/{index,tool-renderer,ansi-to-html}.ts`、`coding-agent/src/modes/interactive/components/mermaid.ts`
- `telemetry/src/{index,memory}.ts`、`agent/scripts/generate-telemetry-docs.ts`
- `tui/src/components/{markdown,editor}.ts`
