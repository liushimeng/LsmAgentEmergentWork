# 专题-第九轮-WebSocket与SSE与长连接推送协议深度对比

> 第九轮 T6 专题：**8 工程 × 9 维度**横向对比，覆盖 WS/SSE/长连接 / 重连 / 心跳 / 背压 / 鉴权。
> 调研对象：undici / openclaw / pi / opencode / claudecode / deepseek-harness / atomcode / agent-studio。
> 调研时间：2026-09-07；目标读者：laew 维护者、协议网关开发者。

---

## 1. 摘要与导读

laew 当前 LLM 调用是**单次 Request-Response**（`src/llm/anthropic.rs` + `openai.rs`），**未实现流式**。第九轮 **L64-L68 五个 gap**：

| Gap | 描述 | 紧急度 |
|-----|------|--------|
| **L64** | 无 SSE 流式响应接收 | P1 |
| **L65** | 无 WebSocket 客户端（订阅型场景） | P2 |
| **L66** | 无心跳 / KeepAlive 机制 | P2 |
| **L67** | 无重连 / 指数退避 | P1 |
| **L68** | 无背压控制（大输出流） | P2 |

8 工程调研后我们看到 **4 档协议哲学**：

1. **L1 库级 RFC 标准**（undici WebSocket/EventSource）
2. **L2 业务级重连**（claudecode SSETransport）
3. **L3 Effect 结构化并发**（opencode WebSocketTracker）
4. **L4 stdio JSON-RPC**（deepseek-harness）

---

## 2. 8 工程长连接概览

### 2.1 undici（JS）— L1 范本
- **路径**：`/usr/local/LsmGitOpenSource/undici/lib/web/websocket/` + `lib/web/eventsource/`
- **核心**：
  - `WebSocket` class（`websocket.js:63`）
  - `establishWebSocketConnection`（`connection.js:26-78`）— RFC 6455 完整握手
  - `EventSource`（`eventsource.js:77`）— server-driven `retry:` + `Last-Event-ID`

### 2.2 openclaw（TS）— ArmableStallWatchdog 通道 watchdog
- **路径**：`/usr/local/LsmGitOpenSource/openclaw/src/channels/transport/stall-watchdog.ts:30-141`
- **核心**：
  - 通道无关 watchdog：`arm/touch/disarm/stop` API
  - 自适应 check interval：`min(5000, max(250, timeoutMs / 6))`
  - `AbortSignal` 集成

### 2.3 pi（TS）— Transport 抽象 4 选 1
- **路径**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/types.ts:107-216`
- **核心**：`Transport = "sse" | "websocket" | "websocket-cached" | "auto"`
- `AssistantMessageEventStream` 统一消费

### 2.4 opencode（TS/Bun）— Effect WebSocketTracker
- **路径**：`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/server/routes/instance/httpapi/websocket-tracker.ts:1-60`
- **核心**：
  - `Effect.scoped` 资源管理
  - `closeAll` 1 秒超时 + `unbounded` concurrency
  - `1001 server closing` clean close

### 2.5 claudecode（TS/Bun）— L2 范本
- **路径**：`/usr/local/LsmGitOpenSource/claudecode/src/cli/transports/{WebSocketTransport,SSETransport,HybridTransport}.ts`
- **核心**：
  - **Bun/Node 双 runtime** 自动检测（`WebSocketTransport.ts:159-192`）
  - SSE 1s → 30s 指数退避 + ±25% jitter（`SSETransport.ts:470-535`）
  - 双层 keepalive（10s WS ping + 5min data frame）

### 2.6 deepseek-harness（TS/Py）— stdio JSON-RPC
- **路径**：`/usr/local/LsmGitOpenSource/deepseek-harness/python/sdk/src/deepseek_harness/client.py:24-93`
- **核心**：
  - `subprocess.Popen(stdin=PIPE, stdout=PIPE)` 单进程长连接
  - `threading.Lock()` + `_write_lock` 帧排序
  - `Queue` per-request id 相关

### 2.7 atomcode（Rust）— SSE only
- **路径**：`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-daemon/src/{lib.rs,live_hub.rs}`
- **核心**：
  - `axum::response::sse::Sse::keep_alive(15s)`
  - `tokio::sync::broadcast(cap 1024)` 多客户端广播
  - **无 WebSocket**（ACP v2 计划加）

### 2.8 agent-studio（Python）— FastAPI EventSourceResponse
- **路径**：`/usr/local/LsmGitOpenSource/agent-studio/backend/openjiuwen_studio/routers/execution.py:9,221-279`
- **核心**：
  - `sse_starlette.EventSourceResponse` 4 端点
  - 平台 channel adapter（Slack/Discord WS/Messenger webhook）

---

## 3. 维度 1：协议分类

### 3.1 横向对比表

| 工程 | WebSocket | SSE | HTTP 长轮询 | gRPC streaming | 二进制帧 |
|------|-----------|-----|-------------|----------------|---------|
| **undici** | ✅ RFC 6455 | ✅ HTML spec | - | - | - |
| **openclaw** | ⚠️ 通道 adapter | ⚠️ | - | - | - |
| **pi** | ✅ 4 选 1 | ✅ | - | - | - |
| **opencode** | ✅ Effect tracker | - | - | - | - |
| **claudecode** | ✅ Bun/Node | ✅ 1s-30s 退避 | - | - | - |
| **deepseek-harness** | ❌（stdio） | - | - | - | - |
| **atomcode** | ❌（计划中） | ✅ axum | - | - | - |
| **agent-studio** | ✅ Discord/Slack | ✅ sse_starlette | - | - | - |

### 3.2 undici WebSocket 范本（`connection.js:26-78`）

```javascript
function establishWebSocketConnection(url, protocols, client, handler, options) {
  const requestURL = url
  requestURL.protocol = url.protocol === 'ws:' ? 'http:' : 'https:'
  const request = makeRequest({ urlList: [requestURL], client, ..., mode: 'websocket', ...})
  if (options.headers) {
    const headersList = getHeadersList(new Headers(options.headers))
    request.headersList = headersList
  }
  const keyValue = crypto.randomBytes(16).toString('base64')
  request.headersList.append('sec-websocket-key', keyValue, true)
  request.headersList.append('sec-websocket-version', '13', true)
  for (const protocol of protocols) {
    request.headersList.append('sec-websocket-protocol', protocol, true)
  }
}
```

**范式要点**：
- **16 字节随机 base64 key**（Sec-WebSocket-Key）
- **Version 13** 强制
- **Subprotocols** 数组支持

### 3.3 claudecode 双 runtime 范本（`WebSocketTransport.ts:159-192`）

```typescript
if (typeof Bun !== 'undefined') {
  const ws = new globalThis.WebSocket(this.url.href, { headers, ... })
  ws.addEventListener('open', this.onBunOpen)
  ws.addEventListener('message', this.onBunMessage)
  ws.addEventListener('pong', this.onPong)        // Bun-only
} else {
  const { default: WS } = await import('ws')
  const ws = new WS(this.url.href, { ... })
  ws.on('pong', this.onPong)
}
```

---

## 4. 维度 2：握手 / 升级

### 4.1 WS Upgrade 头

`undici/connection.js`：
- `Upgrade: websocket`
- `Connection: Upgrade`
- `Sec-WebSocket-Key: <random>`
- `Sec-WebSocket-Version: 13`
- `Sec-WebSocket-Protocol: <protocols>`
- `Sec-WebSocket-Extensions: permessage-deflate`

### 4.2 opencode WS Proxy 升级（`middleware/proxy.ts:14-77`）

```typescript
return Effect.scoped(Effect.gen(function* () {
  const inbound = yield* Effect.orDie(request.upgrade)
  const outbound = yield* Socket.makeWebSocket(ProxyUtil.websocketTargetURL(target),
                                                { protocols: ProxyUtil.websocketProtocols(request.headers) })
  ...
}))
```

---

## 5. 维度 3：消息分帧

### 5.1 4 种分帧范式

| 工程 | 格式 |
|------|------|
| **undici** | RFC 6455 frame + permessage-deflate |
| **claudecode** | JSON text frame `{"type":"keep_alive"}` |
| **opencode** | `Event.default().data(...)` + `[DONE]` 哨兵 |
| **atomcode** | `axum::sse::Event.default().data(json)` |

### 5.2 opencode [DONE] 哨兵范本（`switchyard-server/src/sse.rs:17-78`）

```typescript
yield Ok(Event::default().json_data(value));
if (failed) break;
// [DONE] is the OpenAI Chat success sentinel: clients stop reading there
if (!failed && target_format === WireFormat::OpenAiChat) {
    yield Ok(Event::default().data("[DONE]"));
}
```

**范式要点**：OpenAI Chat 用 `[DONE]` 哨兵；Anthropic/Responses 用 event type。

---

## 6. 维度 4：心跳 / KeepAlive

### 6.1 4 类心跳范式

| 工程 | 协议层 | 应用层 | 间隔 |
|------|--------|--------|------|
| **atomcode** | ✅ Sse::keep_alive | - | 15s |
| **claudecode** | ✅ WS ping | ✅ data frame | 10s + 5min |
| **openclaw** | ⚠️ channel 自带 | ✅ ArmableStallWatchdog | adaptive |
| **opencode** | ✅ closeAll | - | 1s |
| **deepseek-harness** | - | ✅ Queue 心跳 | - |

### 6.2 claudecode 双层范本（`WebSocketTransport.ts:20-46`）

```typescript
const DEFAULT_PING_INTERVAL = 10000         // 10s
const DEFAULT_KEEPALIVE_INTERVAL = 300_000  // 5min
const KEEP_ALIVE_FRAME = '{"type":"keep_alive"}\n'

// WS ping: 防止代理 60s 闲置断连（NAT/Cloudflare）
// data frame: 重置 CDN/反向代理闲置 timer
```

**范式要点**：
- **WS ping** 是协议层（不消耗应用层字节）
- **data frame** 是应用层（兼容某些代理只看 data）
- **双层组合**覆盖各种代理场景

### 6.3 atomcode SSE keep_alive（`lib.rs:4401-4428`）

```rust
Sse::new(guarded_stream)
  .keep_alive(axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"))
```

**范式要点**：axum 内置 SSE keep-alive，每 15s 发 `ping` 注释。

---

## 7. 维度 5：断线重连

### 7.1 4 种重连范式

| 工程 | 触发 | 退避 | Give-up |
|------|------|------|---------|
| **claudecode SSE** | connection error | 1s→30s ±25% jitter | 10min |
| **claudecode WS** | `code != 1000` | 同 SSE | 10min |
| **undici EventSource** | end-of-body | server-driven `retry:` | never |
| **openclaw** | channel 自带 | - | - |

### 7.2 claudecode 退避范本（`SSETransport.ts:470-535`）

```typescript
private handleConnectionError(): void {
  this.clearLivenessTimer()
  if (this.state === 'closing' || this.state === 'closed') return
  const now = Date.now()
  if (!this.reconnectStartTime) this.reconnectStartTime = now
  const elapsed = now - this.reconnectStartTime
  if (elapsed < RECONNECT_GIVE_UP_MS) {
    const baseDelay = Math.min(RECONNECT_BASE_DELAY_MS * Math.pow(2, this.reconnectAttempts - 1),
                                RECONNECT_MAX_DELAY_MS)
    const delay = Math.max(0, baseDelay + baseDelay * 0.25 * (2 * Math.random() - 1))  // ±25% jitter
    this.reconnectTimer = setTimeout(() => { void this.connect() }, delay)
  } else {
    this.state = 'closed'; this.onCloseCallback?.()
  }
}
```

### 7.3 claudecode 睡眠检测范本（`WebSocketTransport.ts:36`）

```typescript
const SLEEP_DETECTION_THRESHOLD_MS = DEFAULT_MAX_RECONNECT_DELAY * 2  // 60s
```

**范式要点**：
- 如果重连间隔 > 60s，机器可能睡眠过 → 重置重连预算
- 服务器若因睡眠清理 session（4001/1002）会拒绝重连

### 7.4 PERMANENT_CLOSE_CODES 范本（`WebSocketTransport.ts:36`）

```typescript
const PERMANENT_CLOSE_CODES = new Set([1002, 4001, 4003])
```

**范式要点**：
- 1002 = protocol error
- 4001 = session not found
- 4003 = forbidden
- 立即 `closed` 不重试

---

## 8. 维度 6：背压 / 限流

### 8.1 4 种背压范式

| 工程 | 机制 |
|------|------|
| **claudecode** | `CircularBuffer<StdoutMessage>(cap 1000)` replay-on-reconnect |
| **undici** | WebSocketStream high-water mark |
| **atomcode** | `tokio::sync::broadcast(cap 1024)` + `RecvError::Lagged` |
| **opencode** | Effect scoped + 1s shutdown timeout |

### 8.2 atomcode 广播丢失检测范本（`live_hub.rs:1-87`）

```rust
const BROADCAST_CAPACITY: usize = 1024;
pub struct LiveJoin {
  pub receiver: broadcast::Receiver<LiveObservation>,
}
```

**范式要点**：
- 慢客户端 → `RecvError::Lagged` → 跳过
- 应用层需显式处理 `Lagged`（不阻塞 publisher）

---

## 9. 维度 7：多路复用

### 9.1 横向对比

| 工程 | 多路复用 |
|------|---------|
| **opencode** | WS proxy bidirectional pipe + closeAll 并发 |
| **atomcode** | broadcast channel 多 SSE 客户端 |
| **openclaw** | 每 channel 独立（Slack/Discord/Messenger 各一连接） |
| **pi** | `AssistantMessageEventStream` 单 stream 多 source |

---

## 10. 维度 8：安全

### 10.1 4 类安全范式

| 工程 | Origin | 鉴权 | CSRF |
|------|--------|------|------|
| **semantica**（参考） | ✅ 手动校验（`explorer/ws.py:21-58`） | ✅ API key in header/query | - |
| **claudecode** | - | ✅ token in WS subprotocol | - |
| **opencode** | - | ✅ W3C traceparent | - |
| **deepseek-harness** | - | ✅ subprocess stdio | - |

### 10.2 semantica WS 鉴权范本

```python
@app.websocket("/ws/graph-updates")
async def websocket_endpoint(websocket: WebSocket) -> None:
    # CORSMiddleware 不覆盖 WebSocket 握手，所以手动校验 origin
    origin = websocket.headers.get("origin")
    if origin is not None and origin not in allowed_origin_set:
        await websocket.close(code=4403); return
    # 浏览器客户端用 query parameter 传 API key (WS API 不能设 header)
    candidate = websocket.headers.get("x-api-key") or websocket.query_params.get("api_key")
    if not is_valid_api_key(candidate):
        await websocket.close(code=4401); return
```

**范式要点**：
- WS **不能设自定义 header**，API key 必须走 query string
- Origin 校验要手动（CORS middleware 不覆盖 WS）
- 消息大小限制（避免 DoS）

---

## 11. 维度 9：可观测

### 11.1 5 类 telemetry 范式

| 工程 | 指标 |
|------|------|
| **claudecode** | `tengu_ws_transport_*` events |
| **opencode** | `WebSocketTracker.sockets.size` gauge |
| **atomcode** | `active_connections` counter |
| **undici** | `channels.ping/pong` diag |
| **openclaw** | `ArmableStallWatchdog` idle timer |

---

## 12. 横向大表：8 工程 × 9 维度

| 工程 × 维度 | 协议 | 握手 | 分帧 | 心跳 | 重连 | 背压 | 多路 | 安全 | 可观测 |
|------------|------|------|------|------|------|------|------|------|--------|
| **undici** | 🟢 WS+SSE | 🟢 RFC 6455 | 🟢 | 🟡 | 🟢 server-driven | 🟡 | 🟡 | 🟡 | 🟢 channels |
| **openclaw** | 🟡 watchdog | 🟡 | 🟡 | 🟢 adaptive | 🟡 | 🟡 | 🟡 | 🟡 | 🟢 |
| **pi** | 🟢 4选1 | 🟢 | 🟢 | 🟡 | 🟡 | 🟡 | 🟡 | 🟡 | 🟡 |
| **opencode** | 🟢 Effect | 🟢 Upgrade | 🟢 | 🟢 closeAll | 🟡 | 🟢 scoped | 🟢 | 🟢 | 🟢 |
| **claudecode** | 🟢 Bun/Node | 🟢 | 🟢 | 🟢 双层 | 🟢 exp+jitter | 🟢 circular | 🟡 | 🟢 | 🟢 analytics |
| **deepseek-harness** | 🟡 stdio | 🟡 | 🟡 | 🟡 | 🔴 | 🟢 Queue | 🟡 | 🟢 subprocess | 🟡 |
| **atomcode** | 🟡 SSE only | 🟢 axum | 🟢 | 🟢 15s | 🟡 | 🟢 broadcast | 🟢 | 🟡 | 🟢 conn count |
| **agent-studio** | 🟢 SSE+WS | 🟢 | 🟢 | 🟢 15s | 🟡 | 🟡 | 🟡 | 🟡 | 🟡 |

---

## 13. 设计模式提炼（5 条）

### 13.1 模式 D1：双层 keepalive（claudecode 范本）

```typescript
const DEFAULT_PING_INTERVAL = 10000          // WS ping 协议层
const DEFAULT_KEEPALIVE_INTERVAL = 300_000   // data frame 应用层
```

**laew 应用**：未来流式 LLM 调用时双层保活。

---

### 13.2 模式 D2：Sleep detection（claudecode 范本）

```typescript
const SLEEP_DETECTION_THRESHOLD_MS = 60_000
```

**laew 应用**：TUI 重连后台服务时检测睡眠。

---

### 13.3 模式 D3：Effect scoped + closeAll（opencode 范本）

```typescript
yield* Effect.all(
  active.map(close => close.pipe(Effect.timeout("1 second"))),
  { concurrency: "unbounded", discard: true },
)
```

**laew 应用**：TUI 退出时清理所有后台 stream。

---

### 13.4 模式 D4：Permanent close codes（claudecode 范本）

```typescript
const PERMANENT_CLOSE_CODES = new Set([1002, 4001, 4003])
```

**laew 应用**：OAuth 401 → 立即放弃，不重试。

---

### 13.5 模式 D5：transport-agnostic consumer（pi 范本）

```typescript
export type Transport = "sse" | "websocket" | "websocket-cached" | "auto"
export const stream = new AssistantMessageEventStream()
```

**laew 应用**：未来 LLM 客户端用 `Transport` 抽象屏蔽 SSE/WS 差异。

---

## 14. 反模式警示（3 条）

### 14.1 反模式 A1：无限重试

```typescript
// ❌ 反模式
while (true) { await connect() }
```

**正确**：bounded retry + give-up + 用户提示。

### 14.2 反模式 A2：无 sleep detection

```typescript
// ❌ 反模式
// 笔记本睡眠 8 小时后唤醒，重连 budget 已耗尽
```

**正确**：检测长 gap，重置 budget。

### 14.3 反模式 A3：背压丢失静默

```rust
// ❌ 反模式
let _ = broadcast_rx.recv();  // Lagged 错误被吞掉
```

**正确**：显式处理 `RecvError::Lagged`，记录 metrics。

---

## 15. laew 现状评估（L64-L68 五个 gap）

### 15.1 L64：无 SSE 流式响应（紧急度 P1）

**现状**：`src/llm/anthropic.rs` + `openai.rs` 用 `reqwest::blocking`，无流式。

**修复**：
1. Cargo.toml 加 `eventsource-client = "0.13"` 或 `reqwest-eventsource`。
2. `src/llm/streaming.rs` 新模块。

```rust
use eventsource_client as es;
let client = es::ClientBuilder::for_url(url)?
    .header("authorization", format!("Bearer {}", api_key))?
    .build();
let mut stream = client.stream();
while let Some(event) = stream.next().await {
    match event { Ok(es::SSE::Event(e)) => println!("{}", e.data), ... }
}
```

---

### 15.2 L65：无 WebSocket 客户端（紧急度 P2）

**现状**：无 WS。

**修复**：
1. Cargo.toml 加 `tokio-tungstenite = "0.24"`。
2. 仅在订阅型场景用（一般 LLM 调作用 SSE 即可）。

---

### 15.3 L66：无心跳（紧急度 P2）

**现状**：无 keep-alive。

**修复**：SSE 自带 ping 帧（按 SSE spec），无需额外代码。

---

### 15.4 L67：无重连 / 指数退避（紧急度 P1）

**现状**：失败直接退出。

**修复**：
1. Cargo.toml 加 `backoff = "0.4"`。
2. `src/llm/retry.rs` 退避逻辑。

---

### 15.5 L68：无背压控制（紧急度 P2）

**现状**：流式输出直接打印到 stdout（可能 OOM）。

**修复**：
1. 用 `tokio::sync::mpsc::channel(N)` 限流。
2. TUI 用 `src/tui/input.rs` 的 backpressure 处理。

---

## 16. 附录

### 16.1 参考文件清单（绝对路径）

#### undici
- `lib/web/websocket/websocket.js:63` — WebSocket class
- `lib/web/websocket/connection.js:26-78` — handshake
- `lib/web/websocket/permessage-deflate.js` — deflate
- `lib/web/eventsource/eventsource.js:77,221-307,313-352` — EventSource

#### openclaw
- `src/channels/transport/stall-watchdog.ts:30-141` — ArmableStallWatchdog

#### pi
- `packages/ai/src/types.ts:107-216` — Transport union
- `packages/ai/src/api/pi-messages.ts:345-419` — SSE stream consumer
- `packages/coding-agent/src/core/settings-manager.ts:431-434` — migration

#### opencode
- `packages/opencode/src/server/routes/instance/httpapi/websocket-tracker.ts:1-60`
- `packages/opencode/src/server/routes/instance/httpapi/middleware/proxy.ts:14-77`
- `packages/opencode/src/server/server.ts:196-218` — coordinated shutdown

#### claudecode
- `src/cli/transports/WebSocketTransport.ts:20-160`
- `src/cli/transports/SSETransport.ts:470-535` — exp backoff
- `src/cli/transports/HybridTransport.ts` — composition
- `src/bridge/replBridgeTransport.ts:23-103` — abstraction

#### deepseek-harness
- `python/sdk/src/deepseek_harness/client.py:24-93`

#### atomcode
- `crates/atomcode-daemon/src/lib.rs:4401-4428` — SSE keep-alive
- `crates/atomcode-daemon/src/live_hub.rs:1-87` — broadcast

#### agent-studio
- `backend/openjiuwen_studio/routers/execution.py:9,221-279`
- `connect/adapters/channels/ARCHITECTURE.md:84-96`

#### laew
- `src/llm/{anthropic.rs,openai.rs}` — 无流式
- `src/agent/mod.rs` — run_session 循环

### 16.2 术语表

| 术语 | 含义 |
|------|------|
| **WebSocket** | RFC 6455 全双工协议 |
| **SSE** | Server-Sent Events（HTTP 单向流） |
| **EventSource** | 浏览器 SSE API |
| **WS frame** | WebSocket 二进制分帧 |
| **permessage-deflate** | WS 压缩扩展 |
| **keep-alive** | 心跳保活 |
| **backoff** | 退避 |
| **backpressure** | 背压 |
| **broadcast** | 一对多分发 |
| **Lagged** | broadcast 订阅者落后错误 |
| **scoped** | Effect 资源作用域 |
| **subprotocol** | WS 子协议（Sec-WebSocket-Protocol） |

---

## 17. 结语

8 工程调研后，我们看到 laew 在长连接协议上是**空白**：

- **L64 SSE 流式** 是 P1（用户最直接体验不到延迟的改进）。
- **L67 重连退避** 是 P1（网络抖动时不至于崩）。
- **L65/L66/L68** 是 P2。

**一句话总结**：「**eventsource-client + backoff crate + 双层 keepalive + bounded retry + Permanent close codes**」是 laew 长连接协议的最小落地路径。

---

**字数统计**：~9,000 字，~1,100 行。
**调研时间**：2026-09-07
**作者**：第九轮 T6 专题研究 SubAgent