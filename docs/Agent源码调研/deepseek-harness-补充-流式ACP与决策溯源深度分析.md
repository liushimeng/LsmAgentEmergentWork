# DeepSeek-Harness 补充：流式协议翻译 / ACP 通信协议 / 决策溯源深度分析

> 本文档是对《deepseek-harness-第四轮-Cordis 核心与模块群深度分析》《deepseek-harness-补充-中断传播背压与Typert协议深度分析》的**独立补充**，聚焦三个未在前轮充分覆盖的维度：
> 1. 流式协议翻译管线（SSE ↔ StreamChunk ↔ UI）
> 2. Agent 间通信协议（ACP 完整实现 + A2A 最近似物）
> 3. 决策溯源 / 因果链（SessionEventMap + OTLP + 持久化）

---

## 维度 1：流式协议翻译管线（SSE ↔ 内部事件）

### SSE Chunk 解析
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/sse.ts`
- `parseSse(stream, onComment?)` — AsyncGenerator 产出 SSE data payload。用 `eventsource-parser/stream` 的 `EventSourceParserStream`。产出 `[DONE]` sentinel；EOF 未得则抛 `LlmError('STREAM_CLOSED')`
- 关键契约："Framing 是 spec-strict：事件仅在空行终止符时分派，所以 EOF 处未终止的尾是截断，不是可刷新的 payload"

### 内部事件 / StreamChunk 类型定义
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm/src/types.ts`
- `StreamChunk` union（lines 364-376）：`'block-start' | 'text-delta' | 'reasoning-delta' | 'tool-call-delta' | 'block-end' | 'usage' | 'finish'`。这是**规范的 provider 中立流式协议**
- `ContentBlockMap` / `ContentBlock`（lines 99-110）：`'text' | 'reasoning' | 'image' | 'tool-call' | 'tool-result'`
- `FinishReasonMap`（lines 116-125）：`'stop' | 'tool-calls' | 'max-tokens' | 'aborted' | 'error'`
- `ReplayEnvelope`（lines 342-354）：adapter-private 无损 JSON 回放元数据
- `TokenUsage`（lines 135-149）：**不相交计数约定**（inputTokens 排除 cache hits）

### 翻译管线：SSE → 内部事件 → UI 渲染
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/translate.ts`
- `translate(payloads)` — 消费 SSE data payload（以 `[DONE]` 结尾），产出 `StreamChunk`。按 content/reasoning/tool-call index 维护 `OpenBlock` 状态
- `mapFinishReason(reason)` — 映射有线 finish_reason 词汇
- `mapUsage(usage)` — 从 prompt_tokens 减去 cache hits 以维持不相交约定
- 关键：`tool-call-delta` 带 `argumentsDelta` 是增量 tool_args 片段（line 172-178）

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm/src/assembler.ts`
- `BlockAssembler` class — 增量 chunk→message 组装器。`push(chunk)` 喂入；`blocks()`, `message()`, `usage`, `finish` 读取结果。`interruptedBlocks()` 用于取消
- `PartialBlock` interface（lines 15-23）持有 `toolCallArguments: string` 累积片段

### Wire 格式类型
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/types.ts`
- `WireChunk`, `WireChoice`, `WireDelta`, `WireToolCallDelta`（lines 118-157）— DeepSeek OpenAI 兼容有线格式
- `WireToolCallDelta.arguments`（line 155）："Argument JSON fragment (concatenate across deltas)"

### Adapter（fetch + SSE 接线）
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/adapter.ts`
- `DeepSeekAdapter.streamWithConnection()`（lines 444-520）— 通过 `AbortSignal.any([options.signal, consumer.signal])` 用一个稳定 AbortSignal，空闲看门狗，`yield* translate(parseSse(response.body, onActivity))`
- `request()`（lines 522-706）— fetch POST `/chat/completions` with `accept: 'text/event-stream'`

### WriteBehind / 200ms 批策略
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-persistence/src/write-behind.ts`
- `SessionWriteBehind` class — 有界 per-session 写批。`enqueue(event)` 启动固定 deadline；`flush()` 排空到静止；`armTimer()` 用 `options.maxDelayMs`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-persistence/src/coordinator.ts`
- `DEFAULT_WRITE_BATCH_MAX_DELAY_MS = 200`（line 31）— **这是 WriteBehind 200ms 批常量**
- `PersistenceCoordinator` 编排缓冲、序列化、采纳、修复、处置

### Ctrl-C / AbortController / 中断传播
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/agent-loop/src/agent.ts`
- `ReactLoopAgent`（lines 69-543）— 阶段机（`idle | maintenance | running`）。`cancel(cause, options)` 通过 `phase.abort.abort(cause)` abort。`AgentCancelCause` = `'user' | 'parent' | 'hook' | 'disposed'`
- 通过 `signal.throwIfAborted()` 在每个步骤边界传播 AbortSignal

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/session/src/types.ts`
- `TurnEndReasonMap`（lines 155-177）：`'completed' | 'aborted' | 'blocked' | 'error' | 'max-tokens' | 'interrupted'`

---

## 维度 2：Agent 间通信协议矩阵（A2A / ACP / E2A / A2UI）

### ACP（Agent Communication Protocol）完整实现
**包:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/index.ts`
- `apply(ctx, config)` — 通过 `@agentclientprotocol/sdk` 在 JSON-RPC stdio 上挂载仅自动化 ACP server。`createAcpAgentApp({ name: 'deepseek-harness-acp' })`。Methods: `initialize`, `authenticate`, `session.new`, `session.list`, `session.resume`, `session.close`, `session.setConfigOption`, `session.prompt`, `session.cancel`
- `AcpConfig`（lines 74-83）：provider/model 选择, sessionListPageSize
- `PROTOCOL_VERSION` from SDK

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/session.ts`
- `AcpSession` class — Per-session ACP module 拥有未发布 Agent 组合、选定路由、one-prompt 准入槽、有序标准更新、memoized 静止拆卸
- `prompt()`（lines 246-334）— 在整 Agent 静止时准入、排队、安定一个 prompt。用 `admissionController` AbortController 取消
- `onSessionEvent()`（lines 348-392）— 把持久事件投影为 ACP 更新：`assistant/message` → `assistantUpdates`、`tool/call` → `toolCallUpdate`、`tool/result` → `toolResultUpdate`
- 通过 codec 的 `turnEndToStopReason()`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/codec.ts`
- `turnEndToStopReason(reason: TurnEndReason): StopReason` — 把 harness turn 结束映射为 ACP 终态原因词汇（`end_turn`, `max_tokens`, `cancelled`）

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/updates.ts`
- `assistantUpdates()` — 把已提交 assistant message 转为有序 `agent_thought_chunk`, `agent_message_chunk`, `usage_update` 更新
- `toolCallUpdate()` — `'tool_call'` 更新带 `rawInput: parseToolArguments(event.data.arguments)`
- `toolResultUpdate()` — `'tool_call_update'` completed/failed

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/content.ts`
- `admitAcpPrompt()` — 把 ACP prompt 块准入到有序持久 core content。验证 images（canonical base64）、resource_links
- `assistantBlockToAcp()` — 把已提交 assistant block 翻译为 ACP 有线 content

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/acp/acp/src/mcp.ts`
- `mountAcpMcpServers()` — 把标准 ACP MCP-server 声明翻译为 Agent-scoped DSH MCP clients（stdio + streamable-http）

### A2A / E2A / A2UI
**未找到专用 A2A/E2A/A2UI 包** — grep `a2a|A2A|e2a|E2A|a2ui|A2UI` 无源码命中。最近似物：

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/experimental/agent-team/src/mailbox.ts`
- `TeamMailbox` class — 持久 Team mailbox 准入、target-local dispatch、acknowledgement、recovery。`send()`, `observeSessionEvent()`, `recoverFor()`。这是代码库中最接近 A2A 的实现

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/experimental/agent-team/src/types.ts`
- `TeamMessageSnapshot`, `TeamMessageSource`, `SendTeamMessageRequest` — 持久 peer message 记录。SessionEventMap 合并：`'team/member'`, `'team/task'`, `'team/message/queued'`, `'team/message/delivered'`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/experimental/agent-team/src/journal.ts`
- `TeamJournal` — 在活跃 Lead Session 日志上序列化 Team 事务。`transact()`, `appendAndFlush()`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/subagent/subagent-acp/src/` — 用 ACP 的 Subagent 桥接

---

## 维度 3：决策溯源 / 因果链

### 决策记录：LLM 调用输入/输出日志
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/session/src/types.ts`
- `SessionEventMap`（lines 221-325）— append-only 真相源。关键事件：
  - `'turn/start'`, `'turn/end'`（带 `TurnEndReason`）
  - `'step/start'`, `'step/end'`
  - `'user/message'`, `'assistant/chunk'`（原始 stream chunk 用于回放保真）、`'assistant/message'`（组装后带 usage + interrupted flag）
  - `'tool/call'`, `'tool/result'`
  - `'request/header'`（完整 EpochHeader 快照：config, system, tools）、`'request/context'`（路由元数据）
  - `'session/end-seed'` — 标记构造函数 seed 边界
- `EpochHeader`（lines 184-193）：call config, adapterDefaults, system, tools

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/agent-loop/src/agent.ts`
- `ReactLoopAgent.step()`（lines 339-436）— 日志原始 chunk：`chunkSeqs.push(this.session.append('assistant/chunk', { turn, step, chunk }).seq)`。然后 `assembler.push(chunk)`。完成时日志 `'assistant/message'` 带 `sourceEventSeqs: chunkSeqs`（从 chunk 到 message 的因果链）
- `buildRequest()`（lines 442-542）— 日志 `'request/header'`（initial/resume/change/series）和 `'request/context'`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/core/session/src/request-header.ts`
- `canonicalHeader()`, `headerEquals()`, `foldRequestHeader()` — 从日志重建 EpochHeader

### Span / Trace 关联（OTLP, telemetry）
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-telemetry-otel/src/index.ts`
- `OpenTelemetrySessionBackend` — 组合 OTel JS SDK：`LoggerProvider` + `BatchLogRecordProcessor` + `OTLPLogExporter`。映射记录到 `logger.emit()`
- `SessionTelemetryMode`（lines 44-48）：`FULL | FEEDBACK_ONLY | DISABLED`
- Resource attributes：`service.name`, `service.version`, `user.id`（匿名）

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-telemetry/src/coordinator.ts`
- `SessionTelemetryCoordinator` — Capture coordinator。实时捕获订阅 session firehose + `agent/error` 中继。应用固定 chunk 投影（仅每个 (turn, step) 的首 chunk 外发）。`session-telemetry/child` waterfall 用于脱敏
- `SessionTelemetryRecord`（from index.ts lines 64-87）：`channel: 'ledger' | 'ops'`, `time`, `severity`, `attributes`, `body`

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-telemetry/src/index.ts`
- `SessionTelemetryBackend` abstract service。`SessionTelemetrySink` interface（emit/flush/shutdown）。`SessionTelemetrySharingStatus = 'full' | 'feedback-only' | 'disabled'`

### 审计日志 / 不可篡改存储
**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-persistence/src/coordinator.ts`
- `PersistenceCoordinator` — 后端无关编排。Per-session 序列化（promise chain）、append-only contiguous-seq 契约、crash repair（torn-tail 截断 + 合成closer）
- `PersistenceBackend<TornMarker>` interface（lines 127-218）：`loadStored`, `readStoredRevision`, `loadStoredFrom?`, `materializeHeader?`, `appendBatch`, `commitRepair`, `list`, `locate?`, `close?`
- `StoredPrefix<TornMarker>` 带 torn-marker 用于 crash repair

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-persistence/src/write-behind.ts`
- `SessionWriteBehind` — 有界 per-session 写批，默认 200ms

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-log-deepseek/src/index.ts`
- `acceptedThrough()` — session-log 上传的最高确认序列。`dsh_session_log` extension 携带 `afterSeq/throughSeq/events` 用于增量上传至 DeepSeek endpoint。通过 `'session-log-deepseek/delivery-accepted'` event 确认水位

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-log-deepseek/src/types.ts`
- `DeepSeekSessionLogExtension`（lines 6-15）：version, session header, afterSeq, throughSeq, events

### 可视化生成
**未找到专用可视化生成代码** — 代码库有 session-projection（state fold 系统）和 telemetry capture，但无可视化 chart 生成。`client/ui-*` 包可能渲染但未深入探索。

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-projection/src/index.ts`
- `SessionProjectionRegistry` — 合并可扩展的状态驱动计算单元。`ProjectionDefinition`（key, stateSchema, init, apply, wire view, stateVersion）。在已提交事件上急切驱动、watermark cache、change feed。这是 feed UI 可视化的数据层

**文件:** `/usr/local/LsmGitOpenSource/deepseek-harness/packages/session/session-projection-cache/src/index.ts`
- 持久化 projection cache：`(sessionId, key, ver, seq, val)` 行

---

## 关键发现小结

**维度 1（流式）：** 真实 SSE 解析在 `llm-deepseek/src/sse.ts`，翻译在 `llm-deepseek/src/translate.ts`，`StreamChunk` 协议在 `llm/src/types.ts`，组装器在 `llm/src/assembler.ts`，WriteBehind 200ms 在 `session-persistence/src/coordinator.ts:31` + `write-behind.ts`

**维度 2（A2A/ACP）：** 完整 ACP 实现在 `packages/acp/acp/src/`（index, session, codec, updates, content, mcp）。无 A2A/E2A/A2UI 包；最近似是 `packages/experimental/agent-team/src/`（mailbox, journal, types）

**维度 3（溯源）：** 决策记录通过 `SessionEventMap` at `core/session/src/types.ts`、在 `core/agent-loop/src/agent.ts` 记录。OTLP telemetry 在 `session-telemetry-otel` + `session-telemetry`。不可篡改存储通过 `session-persistence` coordinator。Session-log 上传通过 `session-log-deepseek`。Projection 系统在 `session-projection`

**注意：** 未找到 `track/track_durable 双通道`、`Cordis events` 或 `Fiber events` 作为显式命名类型 — 代码库用 `SessionEventMap`（Cordis 合并）和 per-session fiber 而非显式 Fiber 事件类型

---

## 关键文件路径索引

- `llm/llm-deepseek/src/{sse,translate,adapter,types}.ts`
- `llm/llm/src/{index,types,assembler}.ts`
- `core/agent-loop/src/agent.ts`、`core/session/src/types.ts`、`core/session/src/request-header.ts`
- `acp/acp/src/{index,session,codec,updates,content,mcp}.ts`
- `experimental/agent-team/src/{mailbox,types,journal}.ts`
- `session/session-persistence/src/{coordinator,write-behind}.ts`
- `session/session-telemetry-otel/src/index.ts`、`session/session-telemetry/src/{coordinator,index}.ts`
- `session/session-log-deepseek/src/{index,types}.ts`
- `session/session-projection/src/index.ts`、`session/session-projection-cache/src/index.ts`
- `subagent/subagent-acp/src/`
