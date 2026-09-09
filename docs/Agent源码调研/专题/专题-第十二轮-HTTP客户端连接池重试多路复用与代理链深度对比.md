# 专题-第十二轮-HTTP客户端连接池重试多路复用与代理链深度对比

> **分析日期**：2026-09-07
> **分析范围**：7 个工程（atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici）× 10 大维度
> **底层栈**：reqwest 0.12 (Rust) / undici 8 (Node/Bun) / hyper 1 + rustls 0.23 / Effect HTTP
> **目标**：为 laew（Rust Agent CLI，reqwest 0.12 + rustls-tls）的 HTTP 客户端升级提供源码级参考

---

## 〇、全局架构对比总览

### 0.1 七工程 HTTP 栈分层对比

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                        laew 现状（基准线）                                        │
│  reqwest::Client::new() [完全默认] → resp.chunk() 循环 → SseStream 解析          │
│  无重试 / 无超时 / 无连接池调优 / 无代理配置 / 无熔断                                │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│  atomcode (Rust) — 同技术栈，最直接的参考                                          │
│  SwappableClient (池毒化热重建) → 5 类错误分类 → 中段流重开 (replay-safe)         │
│  15s POOL_IDLE_TIMEOUT / TLS 1.2 自动降级 / 三层信任根 / ±25% jitter             │
│  关键：reqwest + rustls-tls（与 laew 完全一致）                                    │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│  claudecode (TypeScript/Bun+Node) — 822 行 withRetry                             │
│  undici EnvHttpProxyAgent → 4 层重试 (529/429/fast mode/persistent)              │
│  ECONNRESET → disableKeepAlive 降级 / SSETransport 711 行                         │
│  关键：错误分类最完整、Fast Mode 冷却态                                            │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│  deepseek-harness (TypeScript) — Cordis 事件总线分包                              │
│  6 个独立 package (dsh-timeout / llm-retry / web-fetch-http / api/gateway ...)   │
│  双模式重试 (normal 5 次/always 无限) + 对称抖动 + idleWatchdog                   │
│  关键：分包解耦、idleWatchdog 流式空闲超时                                         │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│  openclaw (TypeScript) — SSRF 防御为核心的多层网络栈                              │
│  fetch-guard.ts 762 行 + ssrf.ts 814 行 + PinnedDispatcherPool LRU 池            │
│  HTTP/1-only 强制 / TLS fingerprint pin / Gateway WebSocket generation 作用域     │
│  event-loop 就绪探测 / full/supported/equal/decorrelated jitter                  │
│  关键：安全最深（DNS pinning + 双层 DNS 校验 + LRU 池）                            │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│  opencode (TypeScript/Bun) — Effect 类型化副作用 4 层架构                         │
│  LLMClient → Transport → Executor → FetchHttpClient (Effect Layer)               │
│  MAX_RETRIES=2 + jitter + Retry-After / 10+ Schema Reason 子类 / Auth 组合子      │
│  关键：类型安全（Effect Schema）、Auth 可组合（andThen/orElse）                    │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│  pi (TypeScript/Bun+Node) — 3 层重试 + CBOR 二进制帧                              │
│  undici allowH2:false + autoSelectFamilyAttemptTimeout:2000ms                    │
│  双正则可重试模式匹配 / WebSocket 会话缓存 (5min 空闲/55min 最大)                   │
│  关键：3 层重试（传输/应用/Agent）、WebSocket 故障转移                              │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│  undici (Node.js 底层库) — 所有 Node 工程的共同底层                                │
│  Dispatcher → compose(8 interceptors) → Agent/Pool/Client 三级                   │
│  RetryHandler 548 行 / FixedQueue O(1) / SessionCache (WeakRef)                  │
│  GOAWAY 请求重排 / CONNECT 隧道 / Socks5ProxyAgent                               │
│  关键：拦截器组合范式、连接池标准实现                                              │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 0.2 十维度宏观对比矩阵

| 维度 | laew 现状 | atomcode | claudecode | deepseek | openclaw | opencode | pi | undici |
|------|----------|----------|-----------|---------|---------|---------|-----|--------|
| 连接池 | reqwest 默认 90s idle | ✅ 15s idle + 池重建 | 委托 undici/Bun | 委托 undici | ✅ LRU + 租约 | 委托运行时 | ✅ 可调 bodyTimeout | ✅ Pool/Client 可调 |
| 重试 | ❌ 无 | ✅ 3 次 + ±25% jitter | ✅ 822 行 withRetry | ✅ 双模式 5/∞ | ✅ 指数+jitter | ✅ 2 次+jitter+Retry-After | ✅ 3 层重试 | ✅ RetryHandler 548 行 |
| HTTP/2 | reqwest 默认开 | 默认开 | 未显式启用 | 默认开 | ❌ 全局禁用 | 委托运行时 | ❌ allowH2:false | ✅ 原生 h2 |
| 代理链 | env 自动 | ✅ 3 模式 + loopback 旁路 | ✅ 4 env + Unix socket | env 自动 | ✅ 完整（含 SOCKS5） | env 自动（Bun） | ✅ 完整 NO_PROXY | ✅ 3 种 Agent |
| TLS | rustls 默认 | ✅ 3 层信任根 + TLS 1.2 降级 | ✅ mTLS + CA bundle | 默认 | ✅ fingerprint pin | 默认 | 默认 | ✅ SessionCache |
| 拦截器 | ❌ 无 | 构建时 proxy policy | axios+fetch wrapper | Cordis waterfall | ✅ fetch-guard 全链路 | ✅ Auth 组合子 | onPayload/onResponse | ✅ 8 个内置 |
| 流式 | ✅ chunk 循环 | ✅ 中段流重开 | ✅ SSETransport 711 行 | ✅ SSE + drain | ✅ TransformStream | ✅ Effect Stream Channel | ✅ EventStream 队列 | ✅ Readable/Writable |
| 超时 | ❌ 无 | per-attempt TTFB | 6 级超时 | idleWatchdog | ✅ 多级 + 可刷新 | SSE chunkTimeout | ✅ 4 级超时 | ✅ 4 种 timeout |
| 健康 | ❌ 无 | stale pool 重建 | ECONNRESET 降级 | 被动重连 | ✅ tick watchdog | ❌ 无 | WS 故障转移 | connectionError 事件 |
| 性能 | 默认 | wire dump | 懒加载 undici 1.5MB | 去重 | ✅ HappyEyeballs + LRU | 录制回放 | zstd 压缩 | llhttp WASM |

---

## 一、连接池实现

### 1.1 laew 现状（基准线）

```rust
// Cargo.toml:23
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }

// src/llm/anthropic.rs:37-45 / src/llm/openai.rs:35-45
pub fn new(end_point: &str, api_key: &str, model: &str, user_agent: &str) -> Result<Self> {
    Ok(Self {
        http: reqwest::Client::new(),  // ← 完全默认，无任何调优
        ...
    })
}
```

**现状**：reqwest 默认连接池（每主机 idle 90s、内部 Arc 共享），未设置 `pool_idle_timeout` / `pool_max_idle_per_host` / `connect_timeout`。

### 1.2 atomcode：15s idle + SwappableClient 池毒化热重建

atomcode 与 laew 同技术栈（reqwest + rustls-tls），是最直接的参考：

```rust
// crates/atomcode-capabilities/src/provider/retry.rs:45-54
/// reqwest's default is 90s; gateway load balancers commonly close idle
/// connections sooner. 30s proved too generous against a real gateway —
/// half-open reuse there surfaced as hyper IncompleteMessage.
/// 15s stays well under observed LB windows while keeping reuse for the
/// back-to-back requests of a tool loop.
pub(crate) const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
```

```rust
// crates/atomcode-capabilities/src/provider/openai_compat.rs:438-480
/// An HTTP client held behind a rebuild seam. `rebuild()` constructs a fresh
/// client — hence a brand-new, EMPTY connection pool — and atomically swaps it in.
/// This is the automatic form of the manual `/login` remedy for the "poisoned pool"
/// failure: once a keep-alive connection is silently half-closed, only a fresh pool recovers.
pub(crate) struct SwappableClient {
    current: std::sync::RwLock<reqwest::Client>,
    build: Box<dyn Fn(bool) -> Result<reqwest::Client, ProviderError> + Send + Sync>,
}
```

**核心机制**：
- `get()` 只是 Arc bump（reqwest::Client 内部 Arc），无锁常态化
- `rebuild()` 构建全新空池 + 原子交换 — 修复"池中毒"（keep-alive 连接被 upstream 半关后，从同一池再取仍是死 socket）
- lock 被 panic 污染时 `unwrap_or_else(|p| p.into_inner())` — 防止"锁中毒→所有请求 panic→彻底卡死"

### 1.3 undici：Pool/Agent/Client 三级架构（Node 工程共同底层）

```js
// lib/dispatcher/pool.js:99-118 — Pool[kGetDispatcher] 调度核心
[kGetDispatcher] () {
  const clientTtlOption = this[kOptions].clientTtl
  for (let i = 0; i < this[kClients].length; i++) {
    const client = this[kClients][i]
    // TTL 过期逐出
    if (clientTtlOption != null && clientTtlOption > 0 && client.ttl
        && ((Date.now() - client.ttl) > clientTtlOption)) {
      this[kRemoveClient](client); i--
    } else if (!client[kNeedDrain]) {
      return client    // 有空闲连接，复用
    }
  }
  // 没有空闲连接且未达上限 → 新建
  if (!this[kConnections] || this[kClients].length < this[kConnections]) {
    const dispatcher = this[kFactory](this[kUrl], this[kOptions])
    this[kAddClient](dispatcher)
    return dispatcher
  }
  return undefined   // 已达上限 → 上层排入 FixedQueue
}
```

```js
// lib/dispatcher/pool-base.js — PoolBase 维护 kClients[] + FixedQueue
get [kFree] () { let ret=0; for (const {[kConnected]:c,[kNeedDrain]:n} of this[kClients]) ret+=c&&!n; return ret }
get [kPending] () { let ret=this[kQueued]; for (const {[kPending]:p} of this[kClients]) ret+=p; return ret }
get [kRunning] () { let ret=0; for (const {[kRunning]:r} of this[kClients]) ret+=r; return ret }
```

### 1.4 openclaw：DNS Pinned Dispatcher LRU 池

```ts
// src/infra/net/pinned-dispatcher-pool.ts:35-163
export class PinnedDispatcherPool {
  private readonly entries = new Map<string, PinnedDispatcherPoolEntry>();
  private readonly maxEntries: number;
  private readonly idleTtlMs: number;

  acquire(params) {
    const existing = this.entries.get(params.key);
    if (existing) {
      if (existing.idleTimer) { clearTimeout(existing.idleTimer); existing.idleTimer = undefined; }
      existing.activeLeases += 1;
      this.entries.delete(existing.key); this.entries.set(existing.key, existing);  // LRU 移到末尾
      return this.createLease(existing, true);   // reused=true
    }
    // LRU 淘汰 + 同 groupKey 退役
    if (this.entries.size >= this.maxEntries) {
      const idleEntry = [...this.entries.values()].find((e) => e.activeLeases === 0);
      if (idleEntry) this.retireEntry(idleEntry);
    }
    const entry = { key, groupKey, dispatcher: params.createDispatcher(), activeLeases: 1 };
    this.entries.set(key, entry);
    return this.createLease(entry, false);   // reused=false
  }
}
```

**池键设计**（`fetch-guard.ts:631-637`）：包含 origin + IP 集 + 超时 + 地址族策略 + SSRF 策略 — 只有完全相同的网络语义才能复用连接。

### 1.5 pi：bodyTimeout/headersTimeout 300s 可调

```typescript
// packages/coding-agent/src/core/http-dispatcher.ts:1-14
export const DEFAULT_HTTP_IDLE_TIMEOUT_MS = 300_000;   // 默认 5 min
export const HTTP_IDLE_TIMEOUT_CHOICES = [
    { label: "30 sec",   timeoutMs: 30_000 },
    { label: "1 min",    timeoutMs: 60_000 },
    { label: "2 min",    timeoutMs: 120_000 },
    { label: "5 min",    timeoutMs: 300_000 },
    { label: "disabled", timeoutMs: 0 },
] as const;
```

### 1.6 连接池对比表

| 工程 | idle 超时 | 最大连接/池 | 连接 TTL | 池毒化恢复 | 等待队列 |
|------|----------|-----------|---------|----------|---------|
| laew | 90s (reqwest 默认) | 默认 | 无 | ❌ | 无 |
| atomcode | **15s** | 默认 | 无 | ✅ SwappableClient 热重建 | 无 |
| claudecode | undici 默认 | 默认 | 无 | ECONNRESET → disableKeepAlive | FixedQueue |
| deepseek | undici 默认 | 默认 | 无 | ❌ | FixedQueue |
| openclaw | undici 默认 + idleTtlMs | LRU maxEntries | ✅ clientTtl | ✅ LRU 淘汰 | FixedQueue |
| opencode | 运行时默认 | 默认 | 无 | ❌ | 无 |
| pi | **300s 可调** | 默认 | ✅ | withUndiciErrorListener | FixedQueue |
| undici | 默认 | kConnections | ✅ clientTtl | connectionError 摘除 | FixedQueue O(1) |

---

## 二、重试机制

### 2.1 laew 现状

```rust
// src/llm/anthropic.rs:276-280 — 最简错误处理，零重试
let status = resp.status();
if !status.is_success() {
    let body_text = resp.text().await.unwrap_or_default();
    return Err(AgentError::Llm(format!("HTTP {status}: {body_text}")));
}
```

### 2.2 atomcode：3 次 + ±25% jitter + 5 类错误分类

```rust
// retry.rs:56-88 — RetryPolicy
impl RetryPolicy {
    pub fn default_policy() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
        }
    }
}
```

```rust
// retry.rs:402-446 — 指数退避 + ±25% jitter
fn compute_backoff_jittered(attempt: u32, policy: &RetryPolicy, jitter: f64) -> Duration {
    let exp = policy.base_delay.saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
    let capped = exp.min(policy.max_delay);
    let capped_ms = capped.as_millis() as u64;
    let window_ms = capped_ms / 2;  // ±25% 窗口
    let offset_ms = (jitter * window_ms as f64) as u64;
    let floor_ms = capped_ms.saturating_sub(window_ms / 2);
    Duration::from_ms(floor_ms + offset_ms)
}
```

**5 类错误分类**（laew 最大差距）：

```rust
// retry.rs:119-228
pub(crate) fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_timeout()
        || err.is_connect()
        || is_stale_connection_error(err)      // transient_io + IncompleteMessage
        || chain_has_tls_corruption(err)        // BadRecordMac / DecryptError
}
// 429 所有权分离：Provider 内部快速重试 vs Kernel 生命周期 hook
pub(crate) fn should_retry_open_status(code: u16, owner: RateLimitRetryOwner) -> bool {
    is_retryable_status(code) && (code != 429 || owner == RateLimitRetryOwner::Provider)
}
```

可重试状态码：`408 | 425 | 429 | 500 | 502 | 503 | 504 | 529`

**中段流重开**（streaming 核心容错）：

```rust
// anthropic.rs:201-296 — 流级 loop，1 initial + up to 2 transparent reopens
const MAX_STREAM_ATTEMPTS: u32 = 3;
// metadata 缓冲 + replay-safe：ResponseId/Model/Usage 缓存，replay-sensitive 事件出现后 flush
// 一旦用户可见输出发出，不再重放 — 防止重复输出/重复工具执行
```

### 2.3 claudecode：822 行 withRetry.ts

```typescript
// src/services/api/withRetry.ts:476-491 — 退避算法
export function getRetryDelay(attempt: number, retryAfterHeader?: string | null, maxDelayMs = 32000): number {
    if (retryAfterHeader) {
        const seconds = parseInt(retryAfterHeader, 10);
        if (!isNaN(seconds)) return seconds * 1000;   // 优先服从服务端
    }
    const baseDelay = Math.min(BASE_DELAY_MS * Math.pow(2, attempt - 1), maxDelayMs);
    const jitter = Math.random() * 0.25 * baseDelay;   // 25% jitter
    return baseDelay + jitter;
}
```

**特色机制**：
- **Fast Mode 冷却态**：429/529 命中 fast mode 时，长延迟切到标准模型避免 cache thrashing，短延迟保持 fast mode 保留 prompt cache
- **Persistent 模式**：无人值守无限重试 + 30s heartbeat
- **自适应 max_tokens**：解析 context overflow 错误 → 调整 max_tokens 立即重试
- **foreground/background 529 分类**：非用户等待的请求在容量级联下不重试

### 2.4 deepseek-harness：双模式 + 对称抖动

| 模式 | 最大重试 | 退避策略 |
|------|---------|---------|
| normal | 5 次 | 指数 + 对称抖动 |
| always | ∞ | 指数 + 对称抖动 |

### 2.5 opencode：Schema `retryable` 类型安全

```ts
// packages/llm/src/route/executor.ts:345-351
const retryDelay = (error: LLMError, attempt: number) => {
  if (error.retryAfterMs !== undefined) return Effect.succeed(Math.min(error.retryAfterMs, MAX_DELAY_MS));
  return Random.nextBetween(
    Math.min(BASE_DELAY_MS * 2 ** attempt * 0.8, MAX_DELAY_MS),
    Math.min(BASE_DELAY_MS * 2 ** attempt * 1.2, MAX_DELAY_MS),  // 0.8-1.2 jitter
  ).pipe(Effect.map((delay) => Math.round(delay)))
}
```

```ts
// packages/llm/src/schema/errors.ts:74-85 — 类型安全的 retryable 标记
export class RateLimitReason extends Schema.Class<RateLimitReason>(...) {
  get retryable() { return true }   // 仅 RateLimit + ProviderInternal 为 true
}
// 不可重试：InvalidRequest / Authentication / QuotaExceeded / ContentPolicy / Transport
```

### 2.6 pi：3 层重试架构

| 层级 | 入口 | 触发条件 | 退避 |
|------|------|---------|------|
| 传输级 | `retryProviderRequest()` | HTTP 状态码 / 网络错误 | retry-after 优先 + 指数+抖动 |
| 应用级 | `retryAssistantCall()` | `AssistantMessage.stopReason === "error"` | 纯指数 |
| Agent 级 | `AgentSession._prepareRetry()` | 上层决策 | 纯指数 |

**双正则模式匹配**（业界独特）：

```typescript
// retry.ts:6-58 — 可重试 / 不可重试 双正则
const NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN = buildProviderErrorPattern([
    "GoUsageLimitError", "FreeUsageLimitError", "quota exceeded", "billing",
]);
const RETRYABLE_PROVIDER_ERROR_PATTERN = buildProviderErrorPattern([
    "overloaded", "rate.?limit", "connection.?refused", "ENOTFOUND", "EAI_AGAIN",
    "socket hang up", "timed? out", "websocket.?closed", "ResourceExhausted",
]);
```

### 2.7 openclaw：4 种 jitter 模式

```ts
// packages/retry/src/index.ts:216-239
// jitter=0 → 无抖动
// jitter∈(0,1) → 对称或正向分数抖动
// jitter="full" → Full Jitter / Equal Jitter / Decorrelated Jitter 复合
// Retry-After hint 使用正向模式（不允许向下抖动规避服务器下限）
```

### 2.8 undici：RetryHandler 548 行

```js
// lib/handler/retry-handler.js:222-283
static [kRetryHandlerDefaultRetry] (err, { state, opts }, cb) {
  const { statusCode, code, headers } = err
  const { maxRetries, minTimeout, maxTimeout, timeoutFactor, statusCodes, errorCodes, methods, retryAfter } = retryOptions
  const { counter } = state
  if (code && code !== 'UND_ERR_REQ_RETRY' && !errorCodes.includes(code)) { cb(err); return }
  if (Array.isArray(methods) && !methods.includes(method)) { cb(err); return }
  if (statusCode != null && Array.isArray(statusCodes) && !statusCodes.includes(statusCode)) { cb(err); return }
  if (counter > maxRetries) { cb(err); return }
  let retryAfterHeader = headers?.['retry-after']
  // ... 指数退避 + Retry-After 优先
  return setTimeout(() => cb(null), retryTimeout)
}
```

**断点续传重试**（`retry-handler.js:453-477`）：206 Partial Content → Range + If-Match(etag)

### 2.9 重试对比表

| 工程 | 最大重试 | 退避策略 | Retry-After | 幂等键 | 特殊机制 |
|------|---------|---------|------------|-------|---------|
| laew | ❌ 无 | — | — | — | — |
| atomcode | 3 | 指数 + ±25% jitter | ✅ 两种格式 | ❌ | 中段流重开、429 所有权分离 |
| claudecode | 有限/∞ | 指数 + 25% jitter | ✅ | ❌ | Fast Mode 冷却、自适应 max_tokens |
| deepseek | 5/∞ 双模式 | 指数 + 对称抖动 | ✅ | ❌ | — |
| openclaw | 可配 | 指数 + 4 种 jitter | ✅ 单独上限 | ❌ | 不可恢复 auth 错误直接停 |
| opencode | 2 | 指数 + 0.8-1.2 jitter | ✅ 3 格式 | ❌ | 类型安全 retryable |
| pi | 3 (3 层) | 指数 + 抖动 | ✅ 3 优先级 | ❌ | 双正则模式匹配 |
| undici | 5 (默认) | 指数 + 抖动 | ✅ | ❌ | 断点续传 |

---

## 三、HTTP/2 多路复用

### 3.1 各工程 HTTP/2 策略

| 工程 | HTTP/2 状态 | 原因 |
|------|-----------|------|
| laew | reqwest 默认开（ALPN 协商） | — |
| atomcode | reqwest 默认开 | 单连接流式复用要求不高 |
| claudecode | 未显式启用 | 低频长响应流式不需要多路复用 |
| deepseek | 默认开 | — |
| **openclaw** | **❌ 全局禁用** (`allowH2: false`) | Undici 8 H2 dispatcher override 不可靠；per-dispatcher pinned DNS 与 H2 多路复用冲突 |
| opencode | 运行时默认 | — |
| **pi** | **❌ 全局禁用** (`allowH2: false`) | Undici 8 HTTP/2 实现存在连接级 race，会 crash Node CLI |
| undici | ✅ 原生支持 | maxConcurrentStreams=100, GOAWAY 请求重排 |

### 3.2 undici HTTP/2 实现（lib/dispatcher/client-h2.js）

```js
// GOAWAY 帧处理 + 请求重排
function onHttp2SessionGoAway (errorCode, lastStreamID) {
  const retriableRequests = []
  for (let i = pendingIdx; i < previousPendingIdx; i++) {
    const request = client[kQueue][i]
    if (canReplayRequest(request) && registerGoAwayRefusal(request)) {
      retriableRequests.push(request)   // 可重试的请求重新排队
    } else {
      util.errorRequest(client, request, err)
    }
  }
  client[kQueue].push(...retriableRequests, ...remainingPendingRequests)
}
```

```js
// SETTINGS_MAX_CONCURRENT_STREAMS=0 超时保护
function setNoStreamsTimeout (session) {
  state.noStreamsTimeout = setTimeout(onNoStreamsTimeout, timeout, session).unref()
}
```

### 3.3 laew 建议

reqwest 默认启用 HTTP/2 对大多数场景工作良好。**仅在遇到连接级 race 或需要 per-host 精细控制时考虑禁用**（参考 openclaw/pi 经验）。

---

## 四、代理链与隧道

### 4.1 laew 现状

reqwest 自动读取 `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` 环境变量，但无显式配置、无 loopback 旁路、无认证处理。

### 4.2 atomcode：3 模式 + loopback 旁路

```rust
// crates/atomcode-config/src/proxy.rs:20-56
pub enum ProxyMode {
    FollowSystem,   // 读取 env + OS 系统代理
    DefaultProxy,   // 固定使用启动时捕获的 env 快照
    NoProxy,        // 强制绕过
}
```

**Loopback 旁路**（真实场景刚需）：

```rust
// proxy.rs:231-258
const LOOPBACK_BYPASS: &[&str] = &["localhost", "127.0.0.1", "::1"];
fn with_loopback_bypass(no_proxy: &Option<String>) -> Option<String> {
    // 本地 Ollama / LM Studio / vLLM 都在 127.0.0.1 监听，
    // 用户配了公司代理后会被错误转发到外部 proxy → 403
}
```

**OS 系统代理跨平台解析**：macOS `scutil --proxy`、Windows 注册表 `Internet Settings`、Linux 仅靠 env。

### 4.3 openclaw：最完整的代理体系

```
代理类型:
  1. HTTP 明文代理 (http://) → Http1ProxyWrapper 改写 URL + host 头
  2. HTTPS 代理 (https://) → CONNECT 隧道 + TLS 升级到代理 + TLS 到目标
  3. SOCKS5 代理 (socks5://) → Socks5ProxyAgent 握手 + CONNECT 命令
  4. 环境变量代理 → EnvHttpProxyAgent 读 HTTP_PROXY/HTTPS_PROXY/NO_PROXY
  5. Managed Proxy (proxyline) → 代理轮换、健康检查、故障转移
```

**自研 dispatch 代理**（禁用原生路由）：

```ts
// src/infra/net/undici-runtime.ts:132-213
// 禁用 EnvHttpProxyAgent 原生路由（httpProxy: "", noProxy: "*"），改为自研 dispatch 代理
// 原因：原生 EnvHttpProxyAgent 不支持 per-hop pinned DNS
```

**NO_PROXY 完整复刻**（`proxy-env.ts:152-303`）：逗号/空白分隔、大小写不敏感、尾部 DNS 点忽略、`*` 通配、精确/前导点/`*.`/子域后缀匹配、`:port` 端口匹配、IPv6 括号/裸字面量、**IPv4 CIDR 扩展**。

### 4.4 pi：Bedrock 代理特例

```typescript
// packages/ai/src/api/bedrock-converse-stream.ts:207-218
// AWS SDK v3.798.0+ 默认用 NodeHttp2Handler（基于 node:http2），不支持 http agent
// 需要代理时强制切回 NodeHttpHandler（HTTP/1.1）
config.requestHandler = new NodeHttpHandler({
    httpAgent: new HttpProxyAgent(proxyUrl),
    httpsAgent: new HttpsProxyAgent(proxyUrl),
});
```

### 4.5 undici：3 种代理 Agent

```js
// lib/dispatcher/proxy-agent.js — 通用代理 + CONNECT 隧道
// lib/dispatcher/env-http-proxy-agent.js — 环境变量代理
// lib/dispatcher/socks5-proxy-agent.js — SOCKS5 代理
```

### 4.6 代理对比表

| 工程 | HTTP 代理 | HTTPS 代理 | SOCKS5 | NO_PROXY | 认证 | 轮换 |
|------|----------|-----------|--------|---------|------|------|
| laew | env 自动 | env 自动 | ❌ | env 自动 | URL 内嵌 | ❌ |
| atomcode | ✅ | ✅ | ❌ | ✅ + loopback | URL 内嵌 | ❌ |
| claudecode | ✅ | ✅ | ❌ | ✅ 自实现 | ✅ | ❌ |
| deepseek | env 自动 | env 自动 | ❌ | env 自动 | ❌ | ❌ |
| openclaw | ✅ | ✅ | ✅ | ✅ 完整 + CIDR | ✅ Basic/Bearer | ✅ Managed Proxy |
| opencode | Bun 自动 | Bun 自动 | ❌ | Bun 支持 | URL 内嵌 | ❌ |
| pi | ✅ | ✅ | ❌ | ✅ 完整 | ✅ | ❌ |
| undici | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |

---

## 五、TLS 配置

### 5.1 laew 现状

```toml
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
```

纯 rustls 默认（系统 CA 链），无证书固定、无自签处理、无客户端证书、无 TLS 1.2 降级。

### 5.2 atomcode：3 层信任根 + TLS 1.2 自动降级

**3 层信任根叠加**（`openai_compat.rs:369-436`）：

```rust
fn add_trusted_roots(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // 1) webpki 基座（reqwest 内置）— 公共 CA 永远存在
    // 2) OS native roots（rustls-native-certs）— 企业 MITM CA
    //    pre-filter：用 rustls::RootCertStore::empty().add() 预先验证每个 OS root
    //    避免"一粒老鼠屎坏一锅粥"
    // 3) SSL_CERT_FILE 显式覆盖
}
```

**Issue #514 Backstop**（`openai_compat.rs:282-319`）：

```rust
// OS/SSL_CERT_FILE 污染 → 放弃自定义 root，仅用 webpki 基座
// 公共 CA 端点仍工作；只丢失企业 MITM 根
match build_http_client_inner(..., trust_os_roots = true) {
    Ok(client) => Ok(client),
    Err(first) => {
        tracing::warn!("http client build failed with OS/SSL_CERT_FILE trust roots; retrying with webpki base only");
        build_http_client_inner(..., trust_os_roots = false)
    }
}
```

**TLS 1.2 自动降级**（`crates/atomcode-config/src/tls.rs`）：

```rust
// 两种触发：1) ATOMCODE_TLS_MAX=1.2 env 2) 首次连接失败 → 降级 TLS 1.2 重试 → 成功后 latch 进程级
// Endpoint-gated：仅限 is_managed_https_url() 匹配的第一方域名
pub fn latch_managed_tls12() { MANAGED_TLS12.store(true, Ordering::Relaxed); }
```

### 5.3 claudecode：mTLS + CA bundle

```typescript
// src/utils/caCerts.ts:34-102 — CA Bundle 多源合并
export const getCACertificates = memoize((): string[] | undefined => {
    if (useSystemCA) {
        const systemCAs = tls.getCACertificates?.('system')  // Bun API
        if (systemCAs?.length) certs.push(...systemCAs)
        else certs.push(...tls.rootCertificates)  // fallback Mozilla 内置
    } else {
        certs.push(...tls.rootCertificates)  // NODE_EXTRA_CA_CERTS 必须带 root bundle
    }
    if (extraCertsPath) certs.push(fs.readFileSync(extraCertsPath))
    return certs.length > 0 ? certs : undefined
})
```

**17 种 SSL 错误码全覆盖**（`errorUtils.ts:6-32`）：`UNABLE_TO_VERIFY_LEAF_SIGNATURE` / `SELF_SIGNED_CERT_IN_CHAIN` / `CERT_HAS_EXPIRED` / `ERR_TLS_CERT_ALTNAME_INVALID` / ...

### 5.4 openclaw：TLS fingerprint pin

```ts
// packages/gateway-client/src/websocket-transport.ts:137-190
export function applyGatewayWebSocketTlsPin(options, expectedFingerprint, normalize) {
  options.rejectUnauthorized = false;   // 关闭默认证书链验证
  options.finishRequest = (request) => {
    request.once("socket", (socket) => {
      const validatePin = () => {
        const canonicalFingerprint = normalizeTlsFingerprint(socket.getPeerCertificate()?.fingerprint256);
        if (fingerprint !== normalize(expectedFingerprint)) {
          request.destroy(new GatewayWebSocketTlsPinError("gateway tls fingerprint mismatch"));
        }
      };
      socket.once("secureConnect", validatePin);
    });
  };
}
```

**SNI 处理**（`undici-dispatcher-options.ts:34-54`）：当 HTTPS 代理目标是 IP 地址时，剥离 SNI servername，避免 IP 字面量被当作 SNI 发送。

### 5.5 undici：SessionCache（WeakRef）

```js
// lib/core/connect.js:15-165
const SessionCache = class WeakSessionCache {
  constructor (maxCachedSessions) {
    this._sessionCache = new Map()
    this._sessionRegistry = new FinalizationRegistry((key) => {
      if (this._sessionCache.size < this._maxCachedSessions) return
      const ref = this._sessionCache.get(key)
      if (ref !== undefined && ref.deref() === undefined) this._sessionCache.delete(key)
    })
  }
}
// TLS 参数：servername (SNI) / session (复用) / ALPNProtocols (h2 vs http/1.1)
```

### 5.6 TLS 对比表

| 工程 | 证书验证 | SNI | ALPN | TLS 1.3 | 证书固定 | 自签 | mTLS |
|------|---------|-----|------|--------|---------|------|------|
| laew | rustls 默认 | ✅ | h2+http/1.1 | ✅ | ❌ | ❌ | ❌ |
| atomcode | ✅ 3 层 + pre-filter | ✅ | h2+http/1.1 | ✅ + 1.2 降级 | ❌ | skip_tls_verify | ❌ |
| claudecode | ✅ system + Mozilla + extra | ✅ | ✅ | ✅ | ❌ | NODE_EXTRA_CA_CERTS | ✅ |
| deepseek | Node 默认 | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| openclaw | ✅ + fingerprint pin | ✅ IP 剥离 | http/1.1 only | ✅ | ✅ Gateway | ❌ | ❌ |
| opencode | 运行时默认 | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| pi | Node 默认 | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| undici | ✅ SessionCache | ✅ | ✅ | ✅ | ❌ | rejectUnauthorized=false | ❌ |

---

## 六、请求/响应拦截器

### 6.1 laew 现状

无拦截器/中间件链。请求构建直接 `.post().headers().json().send()`。

### 6.2 atomcode：构建时 proxy policy + wire dump

```rust
// crates/atomcode-capabilities/src/proxy.rs:63-92
pub(crate) fn apply_async_proxy_policy(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let builder = if proxy_disabled() { builder.no_proxy() } else { builder };
    if force_tls12() { builder.max_tls_version(reqwest::tls::Version::TLS_1_2) } else { builder }
}
```

```rust
// provider/mod.rs:46-93 — BYTE-LEVEL wire dump（env ATOMCODE_WIRE_DUMP=1）
pub(crate) fn wire_dump_request(model: &str, body: &Value) {
    let seq = WIRE_DUMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{seq:06}-{ts}-{safe_model}.req.json"));
    let _ = std::fs::write(&path, serde_json::to_string_pretty(body));
}
```

### 6.3 opencode：Auth 组合子（最优雅）

```ts
// packages/llm/src/route/auth.ts:54-63
const auth = (apply: Auth["apply"]): Auth => {
  const self: Auth = {
    apply,
    andThen: (that) => auth((input) => apply(input).pipe(Effect.flatMap((headers) => that.apply({ ...input, headers })))),
    orElse: (that) => auth((input) => apply(input).pipe(Effect.catch(() => that.apply(input))))  // 降级
  }
  return self
}
```

**三层脱敏**（`executor.ts:60-195`）：Header 脱敏 + URL 脱敏（query 参数）+ Body 脱敏（正则 + 实际 secret 全文替换，防御响应回显）。

### 6.4 openclaw：fetch-guard 全链路拦截

```
请求 ──→ fetch-guard.ts
  ├─ 1. beforeRequest() — 同步前置检查
  ├─ 2. resolveSsrFPolicyForUrl() — 策略合并
  ├─ 3. assertExplicitProxyAllowed() — 代理 SSRF 校验
  ├─ 4. resolvePinnedHostnameWithPolicy() — DNS 校验
  ├─ 5. createPinnedDispatcher() — 创建/复用 dispatcher
  ├─ 6. fetchWithRuntimeDispatcher() — 实际请求
  ├─ 7. captureGuardedFetchExchange() — 抓包审计
  └─ 8. 重定向循环（跨域去 body、去敏感头）
```

### 6.5 undici：8 个内置拦截器

```js
module.exports.interceptors = {
  redirect,      // 重定向跟随
  responseError, // 4xx/5xx 错误封装
  retry,         // 重试
  dump,          // 响应截断（默认 1MB）
  dns,           // DNS 缓存 + 双栈
  cache,         // HTTP 缓存
  decompress,    // gzip/br/deflate/zstd
  deduplicate,   // 请求去重
}
// compose(...interceptors) → Proxy(this) 拦截器链
```

### 6.6 拦截器对比表

| 工程 | 中间件链 | 请求转换 | 响应解耦 | 错误标准化 | 脱敏 |
|------|---------|---------|---------|----------|------|
| laew | ❌ | ❌ | ❌ | ❌ | mask_key |
| atomcode | 构建时 proxy policy | wire dump | 多 shape 错误提取 | ✅ 中文友好 | proxy URL |
| claudecode | axios+fetch wrapper | client-request-id | ✅ | ✅ 17 SSL 错误码 | ❌ |
| deepseek | Cordis waterfall | onPayload | onResponse | ✅ | ❌ |
| openclaw | ✅ fetch-guard 全链路 | 跨域重定向安全 | release dispatcher | ✅ | ✅ |
| opencode | ✅ Auth 组合子 | Body Overlay | Framing → Protocol | ✅ 10+ Reason | ✅ 3 层 |
| pi | onPayload/onResponse | ❌ | ❌ | ✅ normalizeProviderError | ❌ |
| undici | ✅ 8 个内置 | ❌ | ❌ | ❌ | ❌ |

---

## 七、流式处理（背压与取消）

> 第十一轮已覆盖 SSE 解析细节，本节聚焦背压控制与取消传播。

### 7.1 laew 现状

```rust
// src/llm/anthropic.rs:282-303 — 手动 chunk 循环
let mut sse = SseStream::new();
let mut parser = AnthropicParser::new();
let mut resp = resp;
loop {
    match resp.chunk().await {
        Ok(Some(bytes)) => { let evs = sse.push(&bytes)?; for ev in evs { parser.feed(&ev, &mut sink)?; } }
        Ok(None) => break,
        Err(e) => return Err(AgentError::Llm(format!("SSE 传输中断: {e}"))),
    }
}
```

无背压控制、无取消传播、无 idle 超时。

### 7.2 atomcode：per-attempt TTFB watchdog + 中段流重开

```rust
// openai_compat.rs:744-938 — TTFB watchdog
let sent = match tokio::time::timeout(open_timeout, req.send()).await {
    Ok(r) => r,
    Err(_elapsed) => {
        if attempt < policy.max_attempts {
            tokio::time::sleep(retry::compute_backoff(attempt, policy)).await;
            attempt += 1; continue;
        }
        return Err(...);
    }
}
// send() 在 response HEAD 到达时 resolve → 只限制"等首字节"时间，不截断慢 body
```

### 7.3 claudecode：SSETransport 711 行

```typescript
// src/cli/transports/SSETransport.ts:77-106 — 增量帧解析器
export function parseSSEFrames(buffer: string): { frames: SSEFrame[]; remaining: string } {
  while ((idx = buffer.indexOf('\n\n', pos)) !== -1) {  // SSE 双换行分隔
    // 注释帧也保留（重置 liveness）
  }
}
// Liveness 心跳：服务端 15s keepalive，客户端 45s 判定死亡
const LIVENESS_TIMEOUT_MS = 45_000
// 主动超时看门狗：90s 无 chunk → abort
const STREAM_IDLE_TIMEOUT_MS = 90_000
```

### 7.4 opencode：Effect Stream Channel 背压

```ts
// packages/llm/src/protocols/shared.ts:242-249
export const sseFraming = (bytes: Stream.Stream<Uint8Array, LLMError>): Stream.Stream<string, LLMError> =>
  bytes.pipe(
    Stream.decodeText(),
    Stream.pipeThroughChannel(Sse.decode()),  // ← 独立 fiber，背压通过 Channel awaitRead 自动传递
    Stream.catchTag("Retry", () => Stream.empty),
  )
// WebSocket 消息背压上限 128
const messages = yield* Queue.bounded<string | Uint8Array, LLMError | Cause.Done<void>>(128)
```

### 7.5 openclaw：TransformStream + 可刷新超时

```ts
// packages/ai/src/transports/openai-completions-transport.ts:206-232
const transformed = response.body.pipeThrough(
  new TransformStream({
    transform(chunk, controller) { doneDetector.observe(chunk); controller.enqueue(chunk); },  // chunk 直通
  }),
)
// 超时刷新：每收到一个 SSE chunk 调用 refresh() 重置计时器
return { response, refreshTimeout: refresh, release, dispatcherReused: dispatcherLease?.reused };
```

### 7.6 流式对比表

| 工程 | 背压机制 | 取消传播 | idle 超时 | 首包超时 | 重连 |
|------|---------|---------|----------|---------|------|
| laew | ❌ | ❌ | ❌ | ❌ | ❌ |
| atomcode | ❌ | ❌ | ❌ | ✅ TTFB watchdog | ✅ 中段流重开 3 次 |
| claudecode | ❌ | ✅ AbortSignal | ✅ 90s | ❌ | ✅ SSETransport 指数退避 |
| deepseek | drain | ❌ | ✅ idleWatchdog | ❌ | ❌ |
| openclaw | ✅ TransformStream | ✅ AbortSignal.any | ✅ 30min 可刷新 | ✅ stream-first-event | ❌ |
| opencode | ✅ Channel awaitRead | ✅ fiber cancel | ❌ | ✅ chunkTimeout | ❌ |
| pi | ✅ EventStream 队列 | ✅ AbortSignal | ✅ 300s bodyTimeout | ❌ | ✅ WS 故障转移 |
| undici | ✅ Readable/Writable | ✅ abort | ✅ bodyTimeout | ✅ headersTimeout | ❌ |

---

## 八、超时与取消

### 8.1 laew 现状

**完全无超时**：无 `connect_timeout` / `read_timeout` / `request_timeout` → 网络抖动时可能永久阻塞。

### 8.2 claudecode：6 级超时

| 层级 | 配置 | 默认值 | 位置 |
|------|------|--------|------|
| SDK 总超时 | `API_TIMEOUT_MS` | 600,000ms (10min) | `services/api/client.ts:144` |
| WebFetch 单次 | `FETCH_TIMEOUT_MS` | 60,000ms | `WebFetchTool/utils.ts:116` |
| SSE 空闲 | `CLAUDE_STREAM_IDLE_TIMEOUT_MS` | 90,000ms | `claude.ts:1870` |
| SSE Liveness | `LIVENESS_TIMEOUT_MS` | 45,000ms | `SSETransport.ts:20` |
| POST 单次 | `POST_TIMEOUT_MS` | 15,000ms | `HybridTransport.ts:12` |
| Domain 检查 | `DOMAIN_CHECK_TIMEOUT_MS` | 10,000ms | `WebFetchTool/utils.ts:119` |

**WeakRef 子 AbortController**（防泄漏）：

```typescript
// src/utils/abortController.ts:77-98
export function createChildAbortController(parent: AbortController): AbortController {
  const child = createAbortController()
  const weakChild = new WeakRef(child)   // parent 不强引用 abandoned child
  const handler = propagateAbort.bind(weakParent, weakChild)
  parent.signal.addEventListener('abort', handler, { once: true })
  child.signal.addEventListener('abort', removeAbortHandler.bind(weakParent, new WeakRef(handler)), { once: true })
  return child
}
```

### 8.3 pi：4 级超时

| 超时类型 | 配置 | 默认值 |
|---------|------|--------|
| 请求超时 | `timeoutMs` | 无（SDK 默认 10 min） |
| 响应头超时 | `headersTimeout` (undici) | = bodyTimeout |
| 体空闲超时 | `bodyTimeout` (undici) | 300s |
| WebSocket 连接超时 | `websocketConnectTimeoutMs` | 15000ms |

### 8.4 openclaw：多级超时 + 事件循环饥饿检测

```ts
// src/utils/fetch-timeout.ts:66-106
function abortDueToTimeout(controller, timeoutMs, startedAtMs, operation, url, combinedSignal) {
  const elapsedMs = Date.now() - startedAtMs;
  const delayMs = elapsedMs - timeoutMs;
  const eventLoopDelayHint = delayMs >= Math.max(1000, timeoutMs * 0.5)
    ? `timer delayed ${delayMs}ms, likely event-loop starvation` : null;
  controller.abort(new TimeoutError("request timed out"));
}
```

### 8.5 undici：4 种 timeout

```js
// lib/core/request.js
this[kHeadersTimeout] = opts.headersTimeout ?? ...  // 响应头等待
this[kBodyTimeout] = opts.bodyTimeout ?? ...        // 响应体 chunk 等待
this[kResetTimeout] = opts.resetTimeout ?? ...      // 连接 reset 超时
this[kAbortTimeout] = opts.abortTimeout ?? ...      // abort 超时
```

### 8.6 超时对比表

| 工程 | 连接超时 | 读取超时 | 请求总超时 | 全局超时 | 可刷新 |
|------|---------|---------|----------|---------|-------|
| laew | ❌ | ❌ | ❌ | ❌ | — |
| atomcode | ✅ connect_timeout | ❌ | ❌ | ❌ | — |
| claudecode | ✅ | ✅ | ✅ 10min | ❌ | ❌ |
| deepseek | ❌ | ✅ idleWatchdog | ❌ | ❌ | — |
| openclaw | ✅ | ✅ | ✅ | ✅ 30min | ✅ refresh() |
| opencode | ❌ | ✅ chunkTimeout | ✅ AISDK timeout | ❌ | ❌ |
| pi | ✅ | ✅ bodyTimeout 300s | ✅ | ❌ | ❌ |
| undici | ✅ connectTimeout | ✅ bodyTimeout | ❌ | ❌ | ❌ |

---

## 九、连接健康检查

### 9.1 laew 现状

无主动/被动健康检查、无熔断、无故障转移。

### 9.2 atomcode：stale pool 重建

```rust
// 一旦 keep-alive 连接被 upstream 半关 → SwappableClient.rebuild() 拿新池
// 触发条件：is_stale_connection_error (transient_io + IncompleteMessage) / TLS corruption
if try_tls12 || retry::is_stale_connection_error(&e) || tls_corruption {
    client.rebuild(capped)?;
}
```

### 9.3 claudecode：ECONNRESET → disableKeepAlive

```typescript
// src/utils/proxy.ts:22-31 — 全局一次性开关
let keepAliveDisabled = false
export function disableKeepAlive(): void { keepAliveDisabled = true }
// 不区分"坏连接"与"好连接"，一刀切 disable keep-alive
// 牺牲连接复用，规避 stale pooled socket 反复 ECONNRESET
```

### 9.4 openclaw：tick watchdog + event-loop 就绪探测

```ts
// packages/gateway-client/src/event-loop-ready.ts:32-120
export async function waitForEventLoopReady(options = {}) {
  const driftThresholdMs = 200;        // 连续 2 次低于 200ms
  const consecutiveReadyChecks = 2;
  // 避免 Node.js 启动阶段（模块加载、JIT 编译）event-loop 抖动导致 WebSocket 握手超时
}
// Gateway Tick Watchdog：30s 无活动 → 判定连接僵死 → 触发重连
this.tickIntervalMs = helloOk.policy?.tickIntervalMs ?? 30_000;
```

### 9.5 pi：WebSocket 故障转移

```typescript
// packages/ai/src/api/openai-codex-responses.ts:930-945
function recordWebSocketFailure(sessionId: string, error: unknown): void {
  websocketSseFallbackSessions.add(sessionId);   // 标记 session 进入 fallback
  stats.websocketFailures++;
}
// 故障转移流程：WS 失败 → 标记 session → 后续请求强制走 SSE
```

### 9.6 健康检查对比表

| 工程 | 主动健康 | 被动健康 | 熔断 | 故障转移 |
|------|---------|---------|------|---------|
| laew | ❌ | ❌ | ❌ | ❌ |
| atomcode | ❌ | ✅ stale pool 重建 | ❌ | ❌ |
| claudecode | ✅ WS ping 30s | ✅ ECONNRESET 降级 | ❌ | ❌ |
| deepseek | ❌ | ✅ 被动重连 | ❌ | ❌ |
| openclaw | ✅ tick watchdog + event-loop 就绪 | ✅ | ✅ shouldPauseGatewayReconnect | ✅ 模型 fallback |
| opencode | ❌ | ❌ | ❌ | ❌ |
| pi | ✅ WS ping | ✅ WS 故障标记 | ❌ | ✅ WS→SSE fallback |
| undici | ❌ | ✅ connectionError 事件 | ❌ | ❌ |

---

## 十、性能优化

### 10.1 laew 现状

reqwest 内置基础优化（hyper 连接池、HTTP/2 ALPN），无 DNS 预解析、无 TCP Fast Open、无零拷贝。

### 10.2 claudecode：懒加载 undici 1.5MB

```typescript
// src/utils/proxy.ts:3
// undici is lazy-required inside getProxyAgent/configureGlobalAgents to defer
// ~1.5MB when no HTTPS_PROXY/mTLS env vars are set (the common case).
```

### 10.3 openclaw：HappyEyeballs + LRU 池 + timer.unref

```ts
// src/infra/net/undici-family-policy.ts:6-44
const AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT_MS = 300;
export function resolveUndiciAutoSelectFamily() {
  if (systemDefault && isWSL2Sync()) return false;   // WSL2 强制 IPv4
  return systemDefault;
}
// IPv4 优先排序
function dedupeAndPreferIpv4(results) { return [...ipv4, ...otherFamilies]; }
// timer.unref() 防阻塞退出（sleepWithAbort / buildTimeoutAbortSignal / PinnedDispatcherPool）
```

### 10.4 pi：zstd 请求体压缩

```typescript
// packages/ai/src/api/openai-codex-responses.ts:51
const REQUEST_COMPRESSION_ZSTD_LEVEL = 3;
const compressedBody = compressRequestBodyZstd(bodyJson);
if (compressedBody) sseHeaders.set("content-encoding", "zstd");
```

### 10.5 undici：llhttp WASM + SessionCache

```js
// lib/core/llhttp/ — lazyllhttp WASM 解析器
// lib/core/connect.js — SessionCache (WeakRef + FinalizationRegistry, 100 条)
// lib/dispatcher/fixed-queue.js — FixedQueue O(1) 均摊环形队列
```

### 10.6 性能优化对比表

| 工程 | DNS 预解析 | TCP Fast Open | 零拷贝 | 缓冲区 | 压缩 | 懒加载 |
|------|-----------|-------------|-------|-------|------|-------|
| laew | ❌ | ❌ | ❌ | 默认 | ❌ | — |
| atomcode | ❌ | ❌ | ❌ | 默认 | ❌ | — |
| claudecode | ❌ | ❌ | ❌ | 默认 | ❌ | ✅ undici 1.5MB |
| deepseek | ❌ | ❌ | ❌ | 默认 | ❌ | ❌ |
| openclaw | ✅ DNS pinning | ❌ | ✅ TransformStream 直通 | 默认 | ❌ | ✅ undici |
| opencode | ❌ | ❌ | ✅ Channel | 默认 | ❌ | ❌ |
| pi | ❌ | ❌ | ❌ | 默认 | ✅ zstd | ❌ |
| undici | ✅ dns interceptor | ❌ | ❌ | FixedQueue O(1) | ✅ decompress | ✅ llhttp WASM |

---

## 十一、laew 差距分析与改造路线图

### 11.1 laew HTTP 客户端基准线（源码级）

```toml
# Cargo.toml:23
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
```

```rust
// src/llm/anthropic.rs:37-45 / src/llm/openai.rs:35-45
http: reqwest::Client::new(),  // 完全默认
```

```rust
// src/llm/anthropic.rs:276-280 — 最简错误处理
if !status.is_success() {
    return Err(AgentError::Llm(format!("HTTP {status}: {body_text}")));
}
```

### 11.2 十维度差距清单

| 维度 | 当前状态 | 业界标杆 | 差距等级 | 推荐参考 |
|------|---------|---------|---------|---------|
| 连接池 | reqwest 默认 90s idle | atomcode 15s + 池重建 | 🟡 中 | atomcode |
| 重试 | ❌ 无 | atomcode 3 次 + 5 类分类 | 🔴 P0 | atomcode |
| HTTP/2 | reqwest 默认开 | openclaw/pi 禁用 | 🟢 低 | 保持现状 |
| 代理 | env 自动 | atomcode 3 模式 + loopback | 🟡 中 | atomcode |
| TLS | rustls 默认 | atomcode 3 层 + 1.2 降级 | 🟡 中 | atomcode |
| 拦截器 | ❌ 无 | opencode Auth 组合子 | 🟢 低 | 可选 |
| 流式 | ✅ chunk 循环 | atomcode 中段流重开 | 🟡 中 | atomcode |
| 超时 | ❌ 无 | claudecode 6 级 | 🔴 P0 | claudecode |
| 健康 | ❌ 无 | atomcode stale pool 重建 | 🟡 中 | atomcode |
| 性能 | 默认 | openclaw HappyEyeballs | 🟢 低 | 可选 |

### 11.3 P0 改造（紧急）

#### P0-1：超时设置（对标 claudecode 6 级）

```rust
// src/llm/anthropic.rs / src/llm/openai.rs — Client 构造
let http = reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(30))     // 连接建立
    .timeout(Duration::from_secs(600))             // 请求总超时（10min，对标 claudecode API_TIMEOUT_MS）
    .pool_idle_timeout(Duration::from_secs(15))    // 对标 atomcode POOL_IDLE_TIMEOUT
    .build()?;
```

#### P0-2：重试 + 退避 + 错误分类（对标 atomcode）

```rust
// 新增 src/llm/retry.rs
pub struct RetryPolicy {
    pub max_attempts: u32,     // 默认 3
    pub base_delay: Duration,  // 500ms
    pub max_delay: Duration,   // 8s
}

// 可重试状态码
fn is_retryable_status(code: u16) -> bool {
    matches!(code, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
}

// 可重试传输错误（source chain 挖掘）
fn is_retryable_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
    || /* transient_io: ConnectionReset/BrokenPipe/UnexpectedEof */
    || /* hyper IncompleteMessage */
}

// 指数退避 + ±25% jitter
fn compute_backoff(attempt: u32, policy: &RetryPolicy) -> Duration {
    let exp = policy.base_delay.saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
    let capped = exp.min(policy.max_delay);
    let jitter = rand::random::<f64>() * 0.5 - 0.25;  // ±25%
    capped + Duration::from_secs_f64(capped.as_secs_f64() * jitter)
}
```

#### P0-3：连接池调优（对标 atomcode）

```rust
.pool_idle_timeout(Duration::from_secs(15))   // 对标 atomcode POOL_IDLE_TIMEOUT
// 可选：.pool_max_idle_per_host(10)          // 限制每主机最大空闲连接
```

### 11.4 P1 改造（重要）

#### P1-1：TLS 3 层信任根（对标 atomcode）

```rust
fn add_trusted_roots(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // 1) webpki 基座（reqwest 内置）
    // 2) OS native roots（rustls-native-certs）— 企业 MITM CA
    // 3) SSL_CERT_FILE 显式覆盖
    builder
}
```

#### P1-2：代理 3 模式 + loopback 旁路（对标 atomcode）

```rust
pub enum ProxyMode { FollowSystem, DefaultProxy, NoProxy }
const LOOPBACK_BYPASS: &[&str] = &["localhost", "127.0.0.1", "::1"];
```

#### P1-3：中段流重开（对标 atomcode）

```rust
// 流级 loop：1 initial + up to 2 transparent reopens
const MAX_STREAM_ATTEMPTS: u32 = 3;
// 流传输错误 + !emitted_replay_sensitive → reopen
```

#### P1-4：可中断 sleep + AbortSignal 透传

```rust
// 所有 sleep 接 CancellationToken，避免重试阻塞进程退出
async fn sleep_with_cancellation(delay: Duration, token: &CancellationToken) { ... }
```

### 11.5 P2 改造（进阶）

#### P2-1：熔断器（对标 failsafe）

```rust
// 连续 N 次失败 → 熔断 → 快速失败
use failsafe::futures::CircuitBreaker;
let circuit_breaker = CircuitBreaker::builder()
    .failure_threshold(5)
    .half_open_max_calls(1)
    .build();
```

#### P2-2：DNS pinning + HappyEyeballs（对标 openclaw）

```rust
// trust-dns-resolver + hyper::client::connect::HttpConnector::new_with_resolver()
```

#### P2-3：Wire dump 诊断（对标 atomcode）

```rust
// env LAEW_WIRE_DUMP=1 → 写 <config_dir>/wire-dump/<seq>-<ts>-<model>.req.json
```

### 11.6 推荐 Rust crate 清单

| 需求 | 推荐 crate | 理由 |
|------|-----------|------|
| 超时 | reqwest 内置 `.timeout()` / `.connect_timeout()` | 已依赖 |
| 重试 + 退避 | `backoff` + `backoff-reqwest` | 标准退避库 |
| 熔断器 | `failsafe` | 三态熔断，futures 支持 |
| 代理 | reqwest 内置 `.proxy()` | 已依赖 |
| TLS 根扩展 | `rustls-native-certs` | atomcode 同方案 |
| DNS 预解析 | `trust-dns-resolver` | 异步 DNS |
| 连接池监控 | `metrics` + hyper pool 事件 | 可观测 |
| 随机抖动 | `rand` | 标准库 |
| 可取消 sleep | tokio 内置 `tokio::time::timeout` | 已依赖 |
| SSE 解析 | `sse-stream` | 可选（当前自实现够用） |

---

## 十二、总结

### 12.1 七工程 HTTP 成熟度排名

| 排名 | 工程 | 成熟度 | 核心亮点 | laew 可借鉴 |
|------|------|--------|---------|-----------|
| 1 | **atomcode** | ⭐⭐⭐⭐⭐ | 同技术栈、SwappableClient、5 类错误分类、中段流重开 | ★★★★★ 最直接 |
| 2 | **openclaw** | ⭐⭐⭐⭐⭐ | SSRF 防御最深、DNS pinning、LRU 池、event-loop 就绪 | ★★★★ 安全维度 |
| 3 | **claudecode** | ⭐⭐⭐⭐ | 822 行 withRetry、Fast Mode 冷却、6 级超时 | ★★★★ 重试维度 |
| 4 | **pi** | ⭐⭐⭐⭐ | 3 层重试、双正则模式匹配、WS 故障转移 | ★★★ 重试分层 |
| 5 | **opencode** | ⭐⭐⭐ | Effect 类型安全、Auth 组合子、Schema Reason | ★★★ 类型安全 |
| 6 | **deepseek** | ⭐⭐⭐ | 双模式重试、idleWatchdog、分包解耦 | ★★ 分包架构 |
| 7 | **undici** | ⭐⭐⭐ | 底层库、拦截器组合、标准实现 | ★★ 拦截器范式 |

### 12.2 laew 改造优先级

```
P0（紧急，1-2 周）：
  ├── 超时设置（connect_timeout + timeout + pool_idle_timeout）
  ├── 重试机制（3 次 + 指数退避 + ±25% jitter）
  └── 错误分类（可重试状态码 + 传输错误 source chain 挖掘）

P1（重要，2-4 周）：
  ├── TLS 3 层信任根（webpki + OS native + SSL_CERT_FILE）
  ├── 代理 3 模式 + loopback 旁路
  ├── 中段流重开（MAX_STREAM_ATTEMPTS=3）
  └── 可中断 sleep + CancellationToken 透传

P2（进阶，1-2 月）：
  ├── 熔断器（failsafe）
  ├── DNS pinning + HappyEyeballs
  └── Wire dump 诊断
```

### 12.3 关键洞察

1. **atomcode 是 laew 最直接的参考**：同 reqwest + rustls-tls 技术栈，SwappableClient、5 类错误分类、中段流重开、TLS 1.2 降级均可直接移植。

2. **重试是最大差距**：6/7 个工程都有完整重试机制，laew 为零。Anthropic 429/529 overloaded_error 直接报错严重影响用户体验。

3. **超时是 P0**：无超时在网络抖动时可能永久阻塞，对标 claudecode 6 级超时体系。

4. **HTTP/2 默认开即可**：仅 openclaw/pi 因 undici 8 bug 禁用，reqwest 默认工作良好。

5. **安全可渐进**：SSRF 防御（openclaw）和 TLS fingerprint pin 是 P2，先解决重试和超时。

---

> **报告完**。本文档基于 7 个工程源码级分析，覆盖 10 大维度 × 7 项目 × ~200 处关键代码片段，为 laew HTTP 客户端从 PoC 升级到生产级 Agent CLI 提供完整参考。

---

## 附录 A：各工程核心源码片段精选

> 本附录收录 7 个工程中最关键、最值得 laew 参考的源码片段，按维度组织。

### A.1 连接池 — atomcode SwappableClient（最完整实现）

```rust
// crates/atomcode-capabilities/src/provider/openai_compat.rs:438-480
/// An HTTP client held behind a rebuild seam. `get()` hands out the current client
/// (cheap: `reqwest::Client` is `Arc` inside); `rebuild()` constructs a fresh client
/// — hence a brand-new, EMPTY connection pool — and atomically swaps it in. This is
/// the automatic form of the manual `/login` remedy for the "poisoned pool" failure:
/// once a keep-alive connection is silently half-closed, only a fresh pool recovers,
/// because every reuse of the old pool re-hands-out the dead socket.
pub(crate) struct SwappableClient {
    current: std::sync::RwLock<reqwest::Client>,
    build: Box<dyn Fn(bool) -> Result<reqwest::Client, ProviderError> + Send + Sync>,
}

impl SwappableClient {
    pub(crate) fn get(&self) -> reqwest::Client {
        // Poison-tolerant: lock 被 panic 污染时取出内部值
        self.current.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub(crate) fn rebuild(&self, force_tls12: bool) -> Result<reqwest::Client, ProviderError> {
        let fresh = (self.build)(force_tls12)?;
        *self.current.write().unwrap_or_else(|p| p.into_inner()) = fresh.clone();
        Ok(fresh)
    }
}
```

### A.2 连接池 — undici Pool[kGetDispatcher]（标准实现）

```js
// lib/dispatcher/pool.js:99-118
[kGetDispatcher] () {
  const clientTtlOption = this[kOptions].clientTtl
  for (let i = 0; i < this[kClients].length; i++) {
    const client = this[kClients][i]
    // TTL 过期逐出
    if (clientTtlOption != null && clientTtlOption > 0 && client.ttl
        && ((Date.now() - client.ttl) > clientTtlOption)) {
      this[kRemoveClient](client); i--
    } else if (!client[kNeedDrain]) {
      return client    // 有空闲连接，复用
    }
  }
  // 没有空闲连接且未达上限 → 新建
  if (!this[kConnections] || this[kClients].length < this[kConnections]) {
    const dispatcher = this[kFactory](this[kUrl], this[kOptions])
    this[kAddClient](dispatcher)
    return dispatcher
  }
  return undefined   // 已达上限 → 上层排入 FixedQueue
}
```

### A.3 重试 — atomcode 5 类错误分类（最完整）

```rust
// crates/atomcode-capabilities/src/provider/retry.rs:119-228
pub(crate) fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_timeout()
        || err.is_connect()
        || is_stale_connection_error(err)
        || chain_has_tls_corruption(err)
}

// (a) 瞬时 IO 错误
pub(crate) fn chain_has_transient_io(err: &(dyn std::error::Error + 'static)) -> bool {
    use std::io::ErrorKind::*;
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if matches!(io.kind(),
                ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof | NotConnected | TimedOut
            ) { return true; }
        }
        cur = e.source();
    }
    false
}

// (b) TLS 记录损坏
pub(crate) fn chain_has_tls_corruption(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        let msg = e.to_string();
        if msg.contains("BadRecordMac") || msg.contains("DecryptError")
            || msg.contains("cannot decrypt peer's message") { return true; }
        cur = e.source();
    }
    false
}

// (c) hyper IncompleteMessage
pub(crate) fn chain_has_incomplete_message(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(h) = e.downcast_ref::<hyper::Error>() {
            if h.is_incomplete_message() { return true; }
        }
        cur = e.source();
    }
    false
}
```

### A.4 重试 — claudecode withRetry 主循环

```typescript
// src/services/api/withRetry.ts:256-297 — Fast Mode 冷却态
if (wasFastModeActive && !isPersistentRetryEnabled() &&
    error instanceof APIError && (error.status === 429 || is529Error(error))) {
  const retryAfterMs = getRetryAfterMs(error)
  if (retryAfterMs !== null && retryAfterMs < SHORT_RETRY_THRESHOLD_MS) {
    // <20s: 短时等待，保持 fast mode 不动（prompt cache 复用）
    await sleep(retryAfterMs, options.signal, { abortError })
    continue
  }
  // 长或未知 → 进 cooldown，切到标准模型避免 cache thrashing
  const cooldownMs = Math.max(retryAfterMs ?? DEFAULT_FAST_MODE_FALLBACK_HOLD_MS, MIN_COOLDOWN_MS)
  triggerFastModeCooldown(Date.now() + cooldownMs, is529Error(error) ? 'overloaded' : 'rate_limit')
  retryContext.fastMode = false
  continue
}
```

### A.5 重试 — opencode Schema retryable（类型安全）

```ts
// packages/llm/src/schema/errors.ts:74-85
export class RateLimitReason extends Schema.Class<RateLimitReason>("LLM.Error.RateLimit")({
  _tag: Schema.tag("RateLimit"),
  message: Schema.String,
  retryAfterMs: Schema.optional(Schema.Number),
  rateLimit: Schema.optional(HttpRateLimitDetails),
}) {
  get retryable() { return true }   // 仅 RateLimit + ProviderInternal 为 true
}

// packages/llm/src/route/executor.ts:345-351 — 指数退避 + jitter
const retryDelay = (error: LLMError, attempt: number) => {
  if (error.retryAfterMs !== undefined) return Effect.succeed(Math.min(error.retryAfterMs, MAX_DELAY_MS))
  return Random.nextBetween(
    Math.min(BASE_DELAY_MS * 2 ** attempt * 0.8, MAX_DELAY_MS),
    Math.min(BASE_DELAY_MS * 2 ** attempt * 1.2, MAX_DELAY_MS),
  ).pipe(Effect.map((delay) => Math.round(delay)))
}
```

### A.6 代理 — atomcode loopback 旁路

```rust
// crates/atomcode-config/src/proxy.rs:231-258
const LOOPBACK_BYPASS: &[&str] = &["localhost", "127.0.0.1", "::1"];

fn with_loopback_bypass(no_proxy: &Option<String>) -> Option<String> {
    let mut entries: Vec<String> = no_proxy.as_deref().unwrap_or_default()
        .split(',').map(str::trim).filter(|e| !e.is_empty()).map(str::to_owned).collect();
    for host in LOOPBACK_BYPASS {
        if !entries.iter().any(|e| e.eq_ignore_ascii_case(host)) {
            entries.push((*host).to_owned);
        }
    }
    Some(entries.join(","))
}
```

### A.7 代理 — openclaw 自研 dispatch 代理

```ts
// src/infra/net/undici-runtime.ts:132-213
export function createHttp1EnvHttpProxyAgent(options, timeoutMs, managedTlsEnv) {
  const proxies = new Map();
  for (const uri of [httpProxy, httpsProxy]) {
    if (uri && !proxies.has(uri)) {
      proxies.set(uri, createHttp1ProxyAgent({ ...options, uri }, timeoutMs, managedTlsEnv));
    }
  }
  const dispatcher = new EnvHttpProxyAgent({
    ...buildHttp1AgentOptions(options, timeoutMs),
    httpProxy: "", httpsProxy: "", noProxy: "*",   // 禁用原生路由
  });
  return new Proxy(dispatcher, {
    get(target, property) {
      if (property === "dispatch") {
        return (request, handler) => {
          const uri = origin.protocol === "https:" ? httpsProxy : httpProxy;
          const proxy = proxies.get(uri);
          return proxy && origin && !matchesNoProxy(origin, bypassEnv)
            ? proxy.dispatch(request, handler)
            : target.dispatch(request, handler);
        };
      }
    },
  });
}
```

### A.8 TLS — atomcode 3 层信任根 + issue #514 backstop

```rust
// openai_compat.rs:369-436
fn add_trusted_roots(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // 1) OS native roots（rustls-native-certs）— 企业 MITM CA
    let native = rustls_native_certs::load_native_certs();
    // PRE-FILTER: 用 rustls::RootCertStore::empty().add() 预先验证每个 OS root
    let mut rejected = 0usize;
    for der in native.certs {
        if rustls::RootCertStore::empty().add(der.clone()).is_err() { rejected += 1; continue; }
        if let Ok(cert) = reqwest::Certificate::from_der(der.as_ref()) {
            builder = builder.add_root_certificate(cert);
        }
    }
    // 2) SSL_CERT_FILE override
    if let Some(path) = std::env::var_os("SSL_CERT_FILE").filter(|p| !p.is_empty()) {
        let pem = std::fs::read(&path)?;
        for cert in reqwest::Certificate::from_pem_bundle(&pem)? {
            builder = builder.add_root_certificate(cert);
        }
    }
    builder
}

// openai_compat.rs:282-319 — Issue #514 Backstop
fn build_http_client(...) -> Result<reqwest::Client, ProviderError> {
    match build_http_client_inner(..., trust_os_roots = true) {
        Ok(client) => Ok(client),
        Err(first) => {
            // OS/SSL_CERT_FILE 污染 → 放弃自定义 root，仅用 webpki 基座
            tracing::warn!("http client build failed with OS/SSL_CERT_FILE trust roots; retrying with webpki base only");
            build_http_client_inner(..., trust_os_roots = false)
        }
    }
}
```

### A.9 TLS — openclaw fingerprint pin

```ts
// packages/gateway-client/src/websocket-transport.ts:137-190
export function applyGatewayWebSocketTlsPin(options, expectedFingerprint, normalize) {
  options.rejectUnauthorized = false;   // 关闭默认证书链验证
  options.finishRequest = (request) => {
    request.once("socket", (socket) => {
      const validatePin = () => {
        const canonicalFingerprint = normalizeTlsFingerprint(socket.getPeerCertificate()?.fingerprint256);
        if (fingerprint !== normalize(expectedFingerprint)) {
          request.destroy(new GatewayWebSocketTlsPinError("gateway tls fingerprint mismatch"));
        }
      };
      socket.once("secureConnect", validatePin);
    });
  };
}
```

### A.10 拦截器 — opencode Auth 组合子

```ts
// packages/llm/src/route/auth.ts:54-63
const auth = (apply: Auth["apply"]): Auth => {
  const self: Auth = {
    apply,
    andThen: (that) => auth((input) => apply(input).pipe(Effect.flatMap((headers) =>
      that.apply({ ...input, headers })))),
    orElse: (that) => auth((input) => apply(input).pipe(Effect.catch(() => that.apply(input))))  // 降级
  }
  return self
}

// 使用：config("ANTHROPIC_API_KEY").bearer().orElse(config("ANTHROPIC_TOKEN").header("x-api-key"))
```

### A.11 拦截器 — undici compose 拦截器链

```js
// lib/dispatcher/dispatcher.js:1-54
class Dispatcher extends EventEmitter {
  compose (...args) {
    const interceptors = Array.isArray(args[0]) ? args[0] : args
    let dispatch = this.dispatch.bind(this)
    for (const interceptor of interceptors) {
      if (interceptor == null) continue
      dispatch = interceptor(dispatch)
    }
    return new Proxy(this, { get: (target, key) => key === 'dispatch' ? dispatch : target[key] })
  }
}

// 使用：dispatcher.compose(redirect(), retry(), dump(), dns(), cache(), decompress())
```

### A.12 流式 — atomcode 中段流重开（replay-safe）

```rust
// anthropic.rs:201-296
let s = async_stream::stream! {
    const MAX_STREAM_ATTEMPTS: u32 = 3;
    let mut stream_attempt = 1u32;
    let mut resp = resp;
    'reopen: loop {
        let mut dec = AnthropicSseDecoder::new();
        let mut emitted_replay_sensitive = false;
        let mut pending_metadata = Vec::new();
        let byte_stream = resp.bytes_stream();
        futures::pin_mut!(byte_stream);
        loop {
            match tokio::time::timeout(idle, byte_stream.next()).await {
                Ok(Some(Ok(chunk))) => {
                    for ev in dec.feed(chunk.as_ref()) {
                        // metadata 缓存，replay-sensitive 事件出现后再 flush
                        if !emitted_replay_sensitive && retry::is_attempt_metadata_event(&ev) {
                            pending_metadata.push(ev); continue;
                        }
                        if retry::is_replay_sensitive_event(&ev) {
                            for metadata in pending_metadata.drain(..) { yield metadata; }
                        }
                        emitted_replay_sensitive |= retry::is_replay_sensitive_event(&ev);
                        yield ev;
                    }
                }
                Ok(Some(Err(e))) => {
                    if !emitted_replay_sensitive && stream_attempt < MAX_STREAM_ATTEMPTS {
                        // 短暂退避后再 open（避免立即重试冲击正在恢复的网关）
                        tokio::time::sleep(retry::compute_backoff(stream_attempt, &policy)).await;
                        if let Ok(fresh) = open_stream(...).await {
                            stream_attempt += 1; resp = fresh; continue 'reopen;
                        }
                    }
                    yield StreamEvent::Error(...); return;
                }
            }
        }
    }
};
```

### A.13 流式 — opencode Effect Stream Channel 背压

```ts
// packages/llm/src/protocols/shared.ts:242-249
export const sseFraming = (bytes: Stream.Stream<Uint8Array, LLMError>): Stream.Stream<string, LLMError> =>
  bytes.pipe(
    Stream.decodeText(),
    Stream.pipeThroughChannel(Sse.decode()),  // ← 独立 fiber，背压通过 Channel awaitRead 自动传递
    Stream.catchTag("Retry", () => Stream.empty),
    Stream.filter((event) => event.data.length > 0 && event.data !== "[DONE]"),
    Stream.map((event) => event.data),
  )

// packages/llm/src/route/transport/websocket.ts:144 — WebSocket 消息背压上限
const messages = yield* Queue.bounded<string | Uint8Array, LLMError | Cause.Done<void>>(128)
```

### A.14 超时 — claudecode 6 级超时

```typescript
// src/services/api/client.ts:144 — SDK 总超时
const API_TIMEOUT_MS = parseInt(process.env.API_TIMEOUT_MS || '', 10) || 600_000

// src/services/api/claude.ts:1866-1874 — SSE 空闲看门狗
const STREAM_IDLE_TIMEOUT_MS = parseInt(process.env.CLAUDE_STREAM_IDLE_TIMEOUT_MS || '', 10) || 90_000
const STREAM_IDLE_WARNING_MS = STREAM_IDLE_TIMEOUT_MS / 2

// src/cli/transports/SSETransport.ts:18-20 — Liveness 心跳
const LIVENESS_TIMEOUT_MS = 45_000  // 服务端 15s keepalive，客户端 45s 判定死亡

// src/utils/abortController.ts:77-98 — WeakRef 子 AbortController
export function createChildAbortController(parent: AbortController): AbortController {
  const child = createAbortController()
  const weakChild = new WeakRef(child)   // parent 不强引用 abandoned child
  parent.signal.addEventListener('abort', propagateAbort.bind(weakParent, weakChild), { once: true })
  child.signal.addEventListener('abort', removeAbortHandler.bind(weakParent, new WeakRef(handler)), { once: true })
  return child
}
```

### A.15 超时 — openclaw 可刷新超时 + 事件循环饥饿检测

```ts
// src/utils/fetch-timeout.ts:112-164
export function buildTimeoutAbortSignal(params) {
  const controller = new AbortController();
  const signal = parentSignal ? AbortSignal.any([parentSignal, controller.signal]) : controller.signal;
  const scheduleTimeout = () => {
    timeoutId = setTimeout(abortDueToTimeout, normalizedTimeoutMs, controller, normalizedTimeoutMs, Date.now(), params.operation, params.url, signal);
  };
  scheduleTimeout();
  return {
    signal,
    refresh: () => { /* 重置超时计时器 */ },
    cleanup: () => { /* 清理定时器 */ },
  };
}

function abortDueToTimeout(controller, timeoutMs, startedAtMs, operation, url, combinedSignal) {
  const elapsedMs = Date.now() - startedAtMs;
  const delayMs = elapsedMs - timeoutMs;
  const eventLoopDelayHint = delayMs >= Math.max(1000, timeoutMs * 0.5)
    ? `timer delayed ${delayMs}ms, likely event-loop starvation` : null;
  controller.abort(new TimeoutError("request timed out"));
}
```

### A.16 健康检查 — openclaw event-loop 就绪探测

```ts
// packages/gateway-client/src/event-loop-ready.ts:32-120
export async function waitForEventLoopReady(options = {}) {
  const maxWaitMs = 10_000;        // 默认 10s
  const intervalMs = 1;            // 默认 1ms
  const driftThresholdMs = 200;    // 连续 2 次低于 200ms
  const consecutiveReadyChecks = 2;
  return await new Promise((resolve) => {
    const scheduleNext = () => {
      timer = setTimeout(() => {
        const driftMs = Date.now() - scheduledAt - delayMs;
        if (driftMs > driftThresholdMs) readyChecks = 0;
        else readyChecks += 1;
        if (readyChecks >= consecutiveReadyChecks) { finish(true); return; }
        scheduleNext();
      }, Math.min(intervalMs, maxWaitMs - elapsedMs));
    };
    scheduleNext();
  });
}
```

### A.17 健康检查 — pi WebSocket 故障转移

```typescript
// packages/ai/src/api/openai-codex-responses.ts:829-855
const SESSION_WEBSOCKET_CACHE_TTL_MS = 5 * 60 * 1000;    // 空闲 5 min 关闭
const SESSION_WEBSOCKET_MAX_AGE_MS = 55 * 60 * 1000;    // 最大存活 55 min

function isWebSocketReusable(socket: WebSocketLike): boolean {
  const readyState = getWebSocketReadyState(socket);
  return readyState === undefined || readyState === 1;   // 1 = OPEN
}

function recordWebSocketFailure(sessionId: string, error: unknown): void {
  websocketSseFallbackSessions.add(sessionId);   // 标记 session 进入 fallback
  stats.websocketFailures++;
}
// 故障转移：WS 失败 → 标记 session → 后续请求强制走 SSE
```

### A.18 性能 — openclaw HappyEyeballs + IPv4 优先

```ts
// src/infra/net/undici-family-policy.ts:6-44
const AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT_MS = 300;
export function resolveUndiciAutoSelectFamily() {
  const systemDefault = net.getDefaultAutoSelectFamily();
  if (systemDefault && isWSL2Sync()) return false;   // WSL2 强制 IPv4
  return systemDefault;
}

// src/infra/net/ssrf.ts:591-607 — IPv4 优先排序
function dedupeAndPreferIpv4(results) {
  const ipv4 = [], otherFamilies = [];
  for (const entry of results) {
    if (entry.family === 4) ipv4.push(entry.address);
    else otherFamilies.push(entry.address);
  }
  return [...ipv4, ...otherFamilies];   // IPv4 排前面
}
```

### A.19 性能 — pi zstd 请求体压缩

```typescript
// packages/ai/src/api/openai-codex-responses.ts:51, 367-374
const REQUEST_COMPRESSION_ZSTD_LEVEL = 3;
const bodyJson = JSON.stringify(body);
const compressedBody = compressRequestBodyZstd(bodyJson);
if (compressedBody) sseHeaders.set("content-encoding", "zstd");
const sseBody: Uint8Array | string = compressedBody ?? bodyJson;
```

### A.20 性能 — undici FixedQueue O(1) 均摊

```js
// lib/dispatcher/fixed-queue.js:1-135
const kSize = 2048
const kMask = kSize - 1
// 单链表串联多个固定大小环形缓冲区
// head → [circular buffer] → [circular buffer] → tail
class FixedCircularBuffer {
  bottom = 0; top = 0
  list = new Array(kSize).fill(undefined)
  push (data) { this.list[this.top] = data; this.top = (this.top + 1) & kMask }
  shift () {
    const nextItem = this.list[this.bottom]
    if (nextItem === undefined) return null
    this.list[this.bottom] = undefined
    this.bottom = (this.bottom + 1) & kMask
    return nextItem
  }
}
```

---

## 附录 B：laew 改造参考实现

> 本附录提供可直接用于 laew 的 Rust 代码参考，基于 atomcode（同技术栈）和 claudecode 的最佳实践。

### B.1 超时设置

```rust
// src/llm/mod.rs — Client 构造改造
pub fn build_http_client(connect_timeout_secs: u64, total_timeout_secs: u64) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(connect_timeout_secs))  // 默认 30s
        .timeout(Duration::from_secs(total_timeout_secs))            // 默认 600s (10min)
        .pool_idle_timeout(Duration::from_secs(15))                  // 对标 atomcode
        .user_agent(build_user_agent())
        .build()
        .map_err(|e| AgentError::Llm(format!("HTTP client 构建失败: {e}")))
}
```

### B.2 重试 + 退避 + 错误分类

```rust
// src/llm/retry.rs（新增模块）
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 3, base_delay: Duration::from_millis(500), max_delay: Duration::from_secs(8) }
    }
}

/// 可重试 HTTP 状态码（对标 atomcode）
pub fn is_retryable_status(code: u16) -> bool {
    matches!(code, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
}

/// 可重试传输错误（source chain 挖掘）
pub fn is_retryable_error(err: &reqwest::Error) -> bool {
    if err.is_timeout() || err.is_connect() { return true; }
    // 挖掘 source chain 找 io::Error
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind::*;
            if matches!(io.kind(), ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof | TimedOut) {
                return true;
            }
        }
        // hyper IncompleteMessage
        if let Some(h) = e.downcast_ref::<hyper::Error>() {
            if h.is_incomplete_message() { return true; }
        }
        cur = e.source();
    }
    false
}

/// 指数退避 + ±25% jitter（对标 atomcode）
pub fn compute_backoff(attempt: u32, policy: &RetryPolicy) -> Duration {
    let exp = policy.base_delay.saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
    let capped = exp.min(policy.max_delay);
    let jitter = rand::random::<f64>() * 0.5 - 0.25;  // ±25%
    capped + Duration::from_secs_f64(capped.as_secs_f64() * jitter.max(-0.25).min(0.25))
}

/// Retry-After 解析（对标 atomcode）
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    if let Ok(secs) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // HTTP-date 格式（可选，需要 http-date crate）
    None
}
```

### B.3 重试主循环

```rust
// src/llm/anthropic.rs — complete() 改造
async fn complete_with_retry(&self, req: &AnthropicRequest, meta: &RequestMeta) -> Result<Completion> {
    let policy = RetryPolicy::default();
    let mut attempt = 1;
    loop {
        let result = self.http.post(&self.url).headers(headers).json(req).send().await;
        match result {
            Ok(resp) if resp.status().is_success() => { /* 处理流 */ }
            Ok(resp) if is_retryable_status(resp.status().as_u16()) && attempt < policy.max_attempts => {
                let wait = parse_retry_after(resp.headers())
                    .unwrap_or_else(|| compute_backoff(attempt, &policy));
                tokio::time::sleep(wait).await;
                attempt += 1; continue;
            }
            Ok(resp) => { return Err(AgentError::Llm(format!("HTTP {}: {}", resp.status(), resp.text().await.unwrap_or_default()))); }
            Err(e) if is_retryable_error(&e) && attempt < policy.max_attempts => {
                tokio::time::sleep(compute_backoff(attempt, &policy)).await;
                attempt += 1; continue;
            }
            Err(e) => { return Err(AgentError::Llm(format!("HTTP 请求失败: {e}"))); }
        }
    }
}
```

### B.4 代理 3 模式 + loopback 旁路

```rust
// src/config/proxy.rs（新增模块）
pub enum ProxyMode { FollowSystem, DefaultProxy, NoProxy }

const LOOPBACK_BYPASS: &[&str] = &["localhost", "127.0.0.1", "::1"];

pub fn build_no_proxy(user_no_proxy: Option<&str>) -> String {
    let mut entries: Vec<String> = user_no_proxy.unwrap_or_default()
        .split(',').map(str::trim).filter(|e| !e.is_empty()).map(str::to_owned).collect();
    for host in LOOPBACK_BYPASS {
        if !entries.iter().any(|e| e.eq_ignore_ascii_case(host)) {
            entries.push(host.to_owned);
        }
    }
    entries.join(",")
}
```

### B.5 TLS 3 层信任根

```rust
// src/llm/tls.rs（新增模块）
fn add_trusted_roots(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // OS native roots（企业 MITM CA）
    let native = rustls_native_certs::load_native_certs();
    for der in native.certs {
        if let Ok(cert) = reqwest::Certificate::from_der(der.as_ref()) {
            builder = builder.add_root_certificate(cert);
        }
    }
    // SSL_CERT_FILE override
    if let Some(path) = std::env::var_os("SSL_CERT_FILE").filter(|p| !p.is_empty()) {
        if let Ok(pem) = std::fs::read(&path) {
            if let Ok(certs) = reqwest::Certificate::from_pem_bundle(&pem) {
                for cert in certs { builder = builder.add_root_certificate(cert); }
            }
        }
    }
    builder
}
```

### B.6 可中断 sleep + CancellationToken

```rust
// src/util/sleep.rs（新增模块）
pub async fn sleep_cancellable(delay: Duration, token: &tokio_util::sync::CancellationToken) {
    tokio::select! {
        _ = tokio::time::sleep(delay) => {},
        _ = token.cancelled() => {},
    }
}
```

---

## 附录 C：横向对比详细表

### C.1 连接池参数对比

| 参数 | laew | atomcode | claudecode | openclaw | pi | undici |
|------|------|----------|-----------|---------|-----|--------|
| idle 超时 | 90s (默认) | **15s** | undici 默认 | undici 默认 | **300s 可调** | 默认 |
| 最大连接/主机 | 默认 | 默认 | 默认 | LRU maxEntries | 默认 | kConnections |
| 连接 TTL | 无 | 无 | 无 | ✅ clientTtl | 无 | ✅ clientTtl |
| 池毒化恢复 | ❌ | ✅ SwappableClient | disableKeepAlive | LRU 淘汰 | withUndiciErrorListener | connectionError 摘除 |
| 等待队列 | 无 | 无 | FixedQueue | FixedQueue | FixedQueue | FixedQueue O(1) |
| keep-alive | 默认开 | 默认开 | 默认开 | 默认开 | 默认开 | 默认开 |

### C.2 重试参数对比

| 参数 | laew | atomcode | claudecode | deepseek | openclaw | opencode | pi | undici |
|------|------|----------|-----------|---------|---------|---------|-----|--------|
| 最大重试 | 0 | 3 | 有限/∞ | 5/∞ | 可配 | 2 | 3 (3 层) | 5 |
| 退避基数 | — | 500ms | 500ms | — | initialMs | 500ms | — | 500ms |
| 退避上限 | — | 8s | 32s | — | maxMs | 10s | — | 30s |
| jitter | — | ±25% | 25% | 对称 | 4 种模式 | 0.8-1.2 | 抖动 | 抖动 |
| Retry-After | — | ✅ 2 格式 | ✅ | ✅ | ✅ 单独上限 | ✅ 3 格式 | ✅ 3 优先级 | ✅ |
| 可重试状态码 | — | 408/425/429/500-504/529 | 429/529/5xx | — | — | 429/503/504/529 | 408/409/429/5xx | 500/502/503/504/429 |
| 可重试错误码 | — | 5 类 | — | — | 17 种 | — | 双正则 | 9 种 |
| 幂等键 | — | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| 中断取消 | — | ❌ | ✅ AbortSignal | — | ✅ | ✅ fiber | ✅ AbortSignal | ✅ abort |

### C.3 代理能力对比

| 能力 | laew | atomcode | claudecode | openclaw | pi | undici |
|------|------|----------|-----------|---------|-----|--------|
| HTTP_PROXY | env 自动 | ✅ | ✅ | ✅ | ✅ | ✅ |
| HTTPS_PROXY | env 自动 | ✅ | ✅ | ✅ | ✅ | ✅ |
| ALL_PROXY | env 自动 | ✅ | ❌ | ❌ | ✅ | ❌ |
| NO_PROXY | env 自动 | ✅ + loopback | ✅ 自实现 | ✅ 完整 + CIDR | ✅ 完整 | ✅ |
| SOCKS5 | ❌ | ❌ | ❌ | ✅ | ❌ | ✅ |
| 代理认证 | URL 内嵌 | URL 内嵌 | ✅ | ✅ Basic/Bearer | ✅ | ✅ |
| 代理轮换 | ❌ | ❌ | ❌ | ✅ Managed Proxy | ❌ | ❌ |
| OS 系统代理 | ❌ | ✅ macOS/Win | ❌ | ❌ | ❌ | ❌ |
| loopback 旁路 | ❌ | ✅ | ❌ | ❌ | ❌ | ❌ |

### C.4 TLS 能力对比

| 能力 | laew | atomcode | claudecode | openclaw | undici |
|------|------|----------|-----------|---------|--------|
| 证书验证 | rustls 默认 | ✅ 3 层 + pre-filter | ✅ system + Mozilla + extra | ✅ + fingerprint pin | ✅ SessionCache |
| SNI | ✅ | ✅ | ✅ | ✅ IP 剥离 | ✅ |
| ALPN | h2+http/1.1 | h2+http/1.1 | ✅ | http/1.1 only | ✅ |
| TLS 1.3 | ✅ | ✅ + 1.2 降级 | ✅ | ✅ | ✅ |
| 证书固定 | ❌ | ❌ | ❌ | ✅ Gateway | ❌ |
| 自签证书 | ❌ | skip_tls_verify | NODE_EXTRA_CA_CERTS | ❌ | rejectUnauthorized=false |
| mTLS | ❌ | ❌ | ✅ | ❌ | ❌ |
| 会话复用 | 默认 | 默认 | 默认 | 默认 | ✅ WeakRef 100 条 |

### C.5 流式处理对比

| 能力 | laew | atomcode | claudecode | opencode | openclaw | pi | undici |
|------|------|----------|-----------|---------|---------|-----|--------|
| SSE 解析 | ✅ 自实现 | ✅ 自实现 | ✅ SSETransport 711 行 | ✅ Effect Channel | ✅ TransformStream | ✅ EventStream | ✅ Readable |
| 背压控制 | ❌ | ❌ | ❌ | ✅ Channel awaitRead | ✅ TransformStream | ✅ 队列+等待者 | ✅ Readable/Writable |
| 取消传播 | ❌ | ❌ | ✅ AbortSignal | ✅ fiber cancel | ✅ AbortSignal.any | ✅ AbortSignal | ✅ abort |
| idle 超时 | ❌ | ❌ | ✅ 90s | ❌ | ✅ 30min 可刷新 | ✅ 300s | ✅ bodyTimeout |
| 首包超时 | ❌ | ✅ TTFB watchdog | ❌ | ✅ chunkTimeout | ✅ stream-first-event | ❌ | ✅ headersTimeout |
| 重连/重开 | ❌ | ✅ 中段流重开 3 次 | ✅ SSETransport 指数退避 | ❌ | ❌ | ✅ WS 故障转移 | ❌ |
| 断点续传 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ Range + If-Match |

---

## 附录 D：第十一轮与第十二轮关系

> 第十一轮已覆盖 SSE 解析、协议流式翻译、决策溯源、可观测性、测试体系、配置系统、插件生态、系统提示词工程。

### D.1 第十一轮 vs 第十二轮覆盖维度

| 维度 | 第十一轮 | 第十二轮 |
|------|---------|---------|
| SSE 解析 | ✅ 完整覆盖 | ❌ 不重复（聚焦背压/取消） |
| 协议流式翻译 | ✅ SSE↔内部事件 | ❌ |
| 连接池 | ❌ | ✅ 完整覆盖 |
| 重试机制 | ❌ | ✅ 完整覆盖 |
| HTTP/2 多路复用 | ❌ | ✅ 完整覆盖 |
| 代理链与隧道 | ❌ | ✅ 完整覆盖 |
| TLS 配置 | ❌ | ✅ 完整覆盖 |
| 拦截器/中间件 | ❌ | ✅ 完整覆盖 |
| 流式处理（背压） | ❌ | ✅ 完整覆盖 |
| 超时与取消 | ❌ | ✅ 完整覆盖 |
| 连接健康检查 | ❌ | ✅ 完整覆盖 |
| 性能优化 | ❌ | ✅ 完整覆盖 |

### D.2 laew gap 累计（L1-L280 + 第十二轮新增）

第十一轮结束时累计 **280 个 laew gap**（L1-L280）。

第十二轮新增 **HTTP 客户端专项 gap**（编号 H1-H20）：

| 编号 | gap | 等级 | 参考工程 |
|------|-----|------|---------|
| H1 | 无超时设置（connect/read/request） | P0 | claudecode 6 级 ✅ **已实现(2026-09-08)**:connect 10s + SSE idle 90s + 单次尝试总 600s,见 `llm/mod.rs::build_http_client` / `llm/sse.rs::stream_chunks` |
| H2 | 无重试机制 | P0 | atomcode 3 次 + 5 类分类 ✅ **已实现(2026-09-08)**:`llm/resilient.rs::ResilientLlmClient`(基数 500ms/上限 8s/±25% jitter/最多 3 次重试,Retry-After 优先 60s 封顶) |
| H3 | 无错误分类（可重试 vs 不可重试） | P0 | atomcode 5 类 ✅ **已实现(2026-09-08)**:`AgentError::LlmHttp/LlmNetwork/LlmStream` + `resilient::is_retryable`(408/425/429/500/502/503/504/529 + 网络类白名单) |
| H4 | 无连接池调优（idle 超时、最大连接） | P1 | atomcode 15s |
| H5 | 无代理 3 模式 + loopback 旁路 | P1 | atomcode |
| H6 | 无 TLS 3 层信任根 | P1 | atomcode |
| H7 | 无中段流重开 | P1 | atomcode MAX_STREAM_ATTEMPTS=3 |
| H8 | 无背压控制 | P2 | opencode Channel |
| H9 | 无取消传播（CancellationToken） | P1 | claudecode WeakRef |
| H10 | 无 idle 超时 watchdog | P1 | deepseek idleWatchdog ✅ **已实现(2026-09-08)**:`llm/sse.rs::stream_chunks`(tokio timeout 包裹 chunk 间读取,超时报可重试 `LlmNetwork`) |
| H11 | 无首包超时 | P2 | openclaw stream-first-event |
| H12 | 无连接健康检查 | P2 | atomcode stale pool 重建 |
| H13 | 无熔断器 | P2 | failsafe crate ✅ **已实现(2026-09-09)**:`llm/resilient.rs` 自研三态熔断(连续 5 次重试耗尽失败 / Open 30s / HalfOpen 单探测),不引入新 crate |
| H14 | 无 DNS pinning | P2 | openclaw |
| H15 | 无 HappyEyeballs | P2 | openclaw/pi |
| H16 | 无请求体压缩 | P2 | pi zstd |
| H17 | 无断点续传 | P2 | undici RetryHandler |
| H18 | 无请求去重 | P2 | undici deduplicate |
| H19 | 无 HTTP 缓存 | P2 | undici cache interceptor |
| H20 | 无 wire dump 诊断 | P2 | atomcode ATOMCODE_WIRE_DUMP |

---

> **全文完**。本文档共 7 个工程 × 10 维度 × ~200 处关键代码片段 + 3 附录（源码精选 + 改造参考 + 详细对比表），为 laew HTTP 客户端升级提供完整参考。
