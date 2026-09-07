# DeepSeek-Harness HTTP 客户端高级实现深度分析

> 分析范围：`/usr/local/LsmGitOpenSource/deepseek-harness`（TypeScript monorepo，53 个 package）
> 分析日期：2026-09-07
> 分析维度：连接池 / 重试 / HTTP/2 / 代理 / TLS / 拦截器 / 流式 / 超时 / 健康检查 / 性能优化

---

## 0. 架构总览

deepseek-harness 的 HTTP 客户端基础设施**不是**传统意义上的"统一 HTTP 客户端库"。
它是一组**按能力分包的薄层**，各自解决特定场景的传输问题：

```
┌──────────────────────────────────────────────────────────────────────┐
│                    deepseek-harness HTTP 传输全景                      │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌─────────────┐   ┌──────────────┐   ┌──────────────────────────┐  │
│  │ llm-deepseek │   │ llm-pi-ai    │   │ web-fetch-http           │  │
│  │ (原生 fetch) │   │ (pi-ai 库)   │   │ (undici Agent + DNS pin) │  │
│  │ 707 行       │   │ 420 行       │   │ 252 行                   │  │
│  └──────┬───────┘   └──────┬───────┘   └────────────┬─────────────┘  │
│         │                  │                       │                │
│         ▼                  ▼                       ▼                │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                 llm-retry (Cordis 插件)                       │   │
│  │     normal 模式（5 次上限）+ always 模式（无限重试）           │   │
│  │     provider 路由级策略 + 持久化重试历史                       │   │
│  └──────────────────────────────────────────────────────────────┘   │
│         │                                                          │
│         ▼                                                          │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │              dsh-timeout (@deepseek-ai/dsh-timeout)           │   │
│  │     deadline() / idleWatchdog() / TimeoutReason / clampTimeout│   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │        api/gateway (WebSocket 多路复用 + 心跳 + 重连)          │   │
│  │     RemoteStreamMuxServer / RemoteStreamMuxConnection         │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │        client/connection (HTTP 桥 + 连接控制器)                │   │
│  │     http-bridge (node:http ↔ fetch) + ConnectionController   │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │        mcp/mcp-client (连接监督 + 退避重启)                    │   │
│  │     startConnection + ReconnectConfig (10 次上限 + 退避)      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

**关键设计哲学**：
- **不造统一 HTTP 客户端轮子**：直接依赖 Node 原生 `fetch`（undici 内核）+ 第三方库（pi-ai、eventsource-parser、ws）
- **能力分包**：超时、重试、流式、连接管理各自独立 package，通过 Cordis 事件总线组合
- **provider 路由级策略**：每个 LLM provider 可独立配置重试策略（normal/always 双模式）
- **SSRF 防御**：web-fetch-http 包实现了 DNS 地址钉扎（address-pinning）

---

## 1. 连接池实现

### 1.1 现状总结

| 维度 | 实现情况 | 位置 |
|------|---------|------|
| 最大连接数 | ❌ 未显式配置 | — |
| 连接复用 | ⚠️ 依赖 undici 默认 | 原生 fetch 使用 Node 全局连接池 |
| keep-alive | ⚠️ 依赖 undici 默认（默认开启） | — |
| 连接生命周期 | ⚠️ 每请求独立 Agent（web-fetch-http） | `network.ts:180-191` |
| 连接池隔离 | ✅ 按请求隔离 dispatcher | `network.ts:180-186` |

### 1.2 web-fetch-http 的 per-request Agent 模式

这是项目中**唯一显式使用 undici Agent 的地方**，出于 SSRF 防御目的：

**文件**：`packages/web/web-fetch-http/src/network.ts:170-191`

```typescript
export async function requestPinned(
  url: URL,
  addresses: readonly PublicAddress[],
  headers: Record<string, string>,
  signal: AbortSignal,
): Promise<PinnedResponse> {
  // 动态导入 undici，避免浏览器-worker 启动时加载 Node 专有依赖
  const { Agent, fetch } = await import('undici')
  const dispatcher = new Agent({
    autoSelectFamily: true,           // 自动选择 IPv4/IPv6
    connect: { lookup: createPinnedLookup(addresses) },  // 钉扎 DNS
  })
  try {
    const response = await fetch(url, { method: 'GET', redirect: 'manual', headers, signal, dispatcher })
    return { response, close: async () => { await dispatcher.close() } }
  } catch (error: unknown) {
    await dispatcher.close()          // 失败时立即关闭
    throw error
  }
}
```

**设计要点**：
- 每个请求创建独立 `Agent` 实例，请求完成后 `dispatcher.close()` 释放
- `autoSelectFamily: true` 启用 Happy Eyeballs（RFC 8305）
- `createPinnedLookup()` 返回固定地址集，**禁止 undici 自行 DNS 解析**（SSRF 防御核心）
- 不设置 `maxConnections`、`keepAliveTimeout` 等参数，依赖 undici 默认

### 1.3 llm-deepseek 的全局 fetch 模式

**文件**：`packages/llm/llm-deepseek/src/adapter.ts:643-656`

```typescript
response = await fetch(`${connection.baseURL}/chat/completions`, {
  method: 'POST',
  headers,
  body: payload,
  signal,
})
```

**特点**：
- 直接使用 Node 全局 `fetch`（undici 内核）
- 无自定义 dispatcher/agent → 使用 Node 默认连接池
- undici 默认：`maxConnections: 100`（每个 origin）、keep-alive 默认开启
- 连接生命周期完全委托给 undici 内部管理

### 1.4 pi-ai 的委托模式

**文件**：`packages/llm/llm-pi-ai/src/adapter.ts`（420 行）

pi-ai 适配器将连接管理完全委托给 `@earendil-works/pi-ai` 库：
- pi-ai 内部使用 undici 作为 HTTP 客户端
- 连接池配置由 pi-ai 库控制，harness 层不可见
- harness 只负责流式翻译和错误分类

### 1.5 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| 连接池 | 依赖 undici 默认 | 无（reqwest 默认） | 可配置 `max_idle_per_host` |
| keep-alive | undici 默认开启 | reqwest 默认开启 | ✅ 已满足 |
| 连接钉扎 | ✅ SSRF 防御用 | ❌ 无 | 按需添加 |
| 连接生命周期 | 每请求/全局混用 | 全局 Client | ✅ laew 更优 |

---

## 2. 重试机制

### 2.1 架构概览

deepseek-harness 的重试机制是**项目中最精密的 HTTP 相关基础设施**，分为两层：

```
┌──────────────────────────────────────────────────────────────┐
│                    llm-retry 重试架构                         │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│   ┌─────────────────────────────────────────────────────┐   │
│   │ llm/llm/src/retry-policy.ts (策略层)                 │   │
│   │  • BackoffConfig (initialDelayMs/maxDelayMs/jitter) │   │
│   │  • NormalRetryPolicyConfig (maxRetries + codes)     │   │
│   │  • AlwaysRetryPolicyConfig (无限重试)               │   │
│   │  • resolveRetryPolicy() 配置解析 + 校验              │   │
│   └──────────────────────┬──────────────────────────────┘   │
│                          │                                   │
│                          ▼                                   │
│   ┌─────────────────────────────────────────────────────┐   │
│   │ llm/llm-retry/src/index.ts (执行层)                  │   │
│   │  • Cordis 插件，监听 agent/request-error 事件        │   │
│   │  • localDelay() 指数退避 + 对称抖动                  │   │
│   │  • cancellableDelay() 可取消等待                     │   │
│   │  • backoff() 持久化重试记录到 session                │   │
│   │  • recover() 决策：重试/透传/放弃                    │   │
│   └─────────────────────────────────────────────────────┘   │
│                          │                                   │
│                          ▼                                   │
│   ┌─────────────────────────────────────────────────────┐   │
│   │ Session 持久化                                       │   │
│   │  • 'llm/retry' 事件：重试计划记录                     │   │
│   │  • 'llm/retry-started' 事件：重试执行记录            │   │
│   │  • policyKey 去重：同策略连续重试                    │   │
│   └─────────────────────────────────────────────────────┘   │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 2.2 策略配置层

**文件**：`packages/llm/llm/src/retry-policy.ts`

#### 2.2.1 双模式重试策略

```typescript
// retry-policy.ts:36-57
export interface NormalRetryPolicyConfig {
  mode: 'normal'
  maxRetries?: number              // 默认 5
  retryableCodes?: string[]        // 默认可重试错误码
  backoff?: BackoffConfig
}

export interface AlwaysRetryPolicyConfig {
  mode: 'always'                   // 无限重试，直到成功/取消/销毁
  backoff?: BackoffConfig
}
```

#### 2.2.2 默认重试参数

```typescript
// retry-policy.ts:14-24
const DEFAULT_MAX_RETRIES = 5
const DEFAULT_INITIAL_DELAY_MS = 500
const DEFAULT_MAX_DELAY_MS = 10_000
const DEFAULT_JITTER_RATIO = 0.1
const DEFAULT_RETRYABLE_CODES = Object.freeze([
  EMPTY_RESPONSE_CODE,   // 'EMPTY_RESPONSE'
  'RATE_LIMIT',
  'SERVER',
  'TIMEOUT',
  'TRANSPORT',
])
```

#### 2.2.3 退避配置

```typescript
// retry-policy.ts:27-35
export interface BackoffConfig {
  initialDelayMs?: number    // 初始延迟（默认 500ms）
  maxDelayMs?: number        // 最大延迟（默认 10000ms）
  jitterRatio?: number       // 对称抖动比率（默认 0.1 = ±10%）
}
```

### 2.3 执行层

**文件**：`packages/llm/llm-retry/src/index.ts`

#### 2.3.1 指数退避 + 对称抖动算法

```typescript
// llm-retry/src/index.ts:58-63
function localDelay(config: ResolvedRetryPolicy, retry: number, random: () => number): number {
  const exponent = Math.min(retry - 1, 1024)
  const exponential = Math.min(config.initialDelayMs * 2 ** exponent, config.maxDelayMs)
  const jitter = 1 - config.jitterRatio + 2 * config.jitterRatio * random()
  return Math.min(exponential * jitter, config.maxDelayMs)
}
```

**算法解析**：
- 指数部分：`initialDelayMs * 2^(retry-1)`，上限 `maxDelayMs`
- 抖动部分：`[1-jitterRatio, 1+jitterRatio]` 均匀随机
- 最终延迟：`min(exponential * jitter, maxDelayMs)`
- 示例（默认配置）：
  - 第 1 次重试：500ms × [0.9, 1.1] = [450, 550]ms
  - 第 2 次重试：1000ms × [0.9, 1.1] = [900, 1100]ms
  - 第 3 次重试：2000ms × [0.9, 1.1] = [1800, 2200]ms
  - 第 5 次重试：8000ms × [0.9, 1.1] = [7200, 8800]ms

#### 2.3.2 可取消延迟

```typescript
// llm-retry/src/index.ts:78-91
function cancellableDelay(delayMs: number, signal: AbortSignal): Promise<boolean> {
  if (signal.aborted) return Promise.resolve(false)
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolve(true)           // 延迟完成
    }, delayMs)
    function onAbort(): void {
      clearTimeout(timer)
      resolve(false)          // 被取消
    }
    signal.addEventListener('abort', onAbort, { once: true })
  })
}
```

#### 2.3.3 恢复决策逻辑

```typescript
// llm-retry/src/index.ts:156-208
async function recover(
  { agent, turn, step, provider, failure, retryPolicy: policy, signal },
  next: () => Promise<RequestErrorAction>,
): Promise<RequestErrorAction> {
  if (policy === undefined) return next()
  if (policy.mode === 'always') {
    // always 模式：无限重试，忽略下游决策
    const downstream = await settleDownstream(next)
    if (downstream.type === 'decision' && downstream.decision?.kind === 'retry') {
      return downstream.decision
    }
  } else if (!policy.retryableCodes.includes(failure.code)) {
    return next()  // 不可重试错误码，透传
  }

  // 查找同策略上一次重试记录
  const priorPolicyRetry = agent.session.events.findLast(...)
  const previousRetry = priorPolicyRetry?.data.retry ?? 0
  if (policy.mode === 'normal' && previousRetry >= policy.maxRetries) return next()

  // 计算延迟：优先使用 provider 的 Retry-After
  let delayMs: number
  if (failure.providerRetryAfterMs !== undefined && ... ) {
    if (failure.providerRetryAfterMs > policy.maxDelayMs) {
      if (policy.mode === 'normal') return next()
      delayMs = localDelay(policy, retry, random)
    } else {
      delayMs = failure.providerRetryAfterMs
    }
  } else {
    delayMs = localDelay(policy, retry, random)
  }

  return backoff(...)
}
```

### 2.4 持久化重试历史

**文件**：`packages/llm/llm-retry/src/types.ts:16-48`

```typescript
declare module '@deepseek-ai/dsh-session/types' {
  interface SessionEventMap {
    'llm/retry': LlmRetryEventData         // 重试计划记录
    'llm/retry-started': LlmRetryStartedEventData  // 重试执行记录
  }
}

export type LlmRetryEventData =
  | {
    retryId: RetryId
    turn: number
    step: number
    provider: string
    mode: 'normal'
    policyKey: string      // 策略指纹，用于去重
    retry: number
    maxRetries: number
    delayMs: number
    failure: LlmFailure
  }
  | { /* always 模式，无 maxRetries 字段 */ }
```

**幂等性保证**：
- `policyKey` = JSON.stringify([mode, maxRetries, sorted(retryableCodes), initialDelayMs, maxDelayMs, jitterRatio])
- 同 turn + step + provider + policyKey 的连续重试被识别为同一次"重试序列"
- 跨 turn/step 独立计数

### 2.5 错误码分类

**文件**：`packages/llm/llm/src/error.ts:25-48`

```typescript
export const CONTEXT_WINDOW_EXCEEDED_CODE = 'CONTEXT_WINDOW_EXCEEDED'
export const QUOTA_EXCEEDED_CODE = 'QUOTA'
export const EMPTY_RESPONSE_CODE = 'EMPTY_RESPONSE'
export const INVALID_CREDENTIAL_CODE = 'INVALID_CREDENTIAL'
```

**文件**：`packages/llm/llm-deepseek/src/adapter.ts:332-344`

```typescript
export function httpErrorCode(status: number, error?: WireError['error']): string {
  if (status === 401 || status === 403) return 'AUTH'
  if (status === 413) return 'INVALID_REQUEST'
  const detail = [error?.code, error?.type, error?.message].filter(Boolean).join(' ')
  if (isQuotaExceededError(detail)) return QUOTA_EXCEEDED_CODE
  if (status === 429) return 'RATE_LIMIT'
  if (status === 400) {
    if (isContextWindowExceededError(detail)) return CONTEXT_WINDOW_EXCEEDED_CODE
    return 'INVALID_REQUEST'
  }
  if (status >= 500) return 'SERVER'
  return `HTTP_${status}`
}
```

### 2.6 pi-ai 适配器的错误分类

**文件**：`packages/llm/llm-pi-ai/src/stream.ts:41-67`

```typescript
function classifyPiAiError(message: string): string {
  if (/\b(?:401|403)\b/.test(message)) return 'AUTH'
  if (isQuotaExceededError(message)) return QUOTA_EXCEEDED_CODE
  if (/\b429\b|rate.?limit/i.test(message)) return 'RATE_LIMIT'
  if (/\b413\b|failed to buffer the request body/i.test(message)) return 'INVALID_REQUEST'
  if (/\b400\b|invalid.?request/i.test(message)) return 'INVALID_REQUEST'
  if (/\b5\d\d\b/.test(message)) return 'SERVER'
  if (/\btime(?:d)?\s*out\b|timeout/i.test(message)) return 'TIMEOUT'
  if (/stream ended (?:before|without)\b/i.test(message)) return 'TRANSPORT'
  if (/\b(?:network|connection|socket|fetch)\b|ECONN[A-Z]+|other side closed|terminated|premature close/i.test(message)) {
    return 'TRANSPORT'
  }
  return 'PI_AI_ERROR'
}
```

**注意**：pi-ai 适配器因为上游库将 cause 链展平，只能基于消息文本正则匹配，是不得已的降级方案。

### 2.7 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| 重试模式 | normal + always 双模式 | 无 | 至少实现 normal 模式 |
| 退避算法 | 指数 + 对称抖动 | 无 | `backoff` crate |
| 可重试错误码 | 配置化白名单 | 无 | 默认 [EMPTY, RATE_LIMIT, SERVER, TIMEOUT, TRANSPORT] |
| Retry-After | ✅ 优先使用 | 无 | 解析 retry-after header |
| 持久化重试历史 | ✅ session 事件 | 无 | 可选 |
| 幂等性判断 | ✅ policyKey 去重 | N/A | 请求级去重 |

---

## 3. HTTP/2 多路复用

### 3.1 现状总结

| 维度 | 实现情况 | 位置 |
|------|---------|------|
| 流控制 | ❌ 未显式配置 | — |
| 优先级 | ❌ 未显式配置 | — |
| 服务器推送 | ❌ 未使用 | — |
| 连接合并 | ❌ 未显式配置 | — |
| ALPN | ⚠️ 依赖 undici 默认 | 原生 fetch |

### 3.2 依赖 undici 默认行为

deepseek-harness 的 HTTP/2 行为**完全依赖 undici 的默认配置**：

- **llm-deepseek**：`fetch(url, { signal })` → undici 自动协商 HTTP/2（如果 server 支持 ALPN h2）
- **web-fetch-http**：`new Agent({ autoSelectFamily: true })` → undici 内部处理 HTTP/2
- **pi-ai**：委托给 pi-ai 库，内部使用 undici

### 3.3 无 HTTP/2 特定代码

搜索 `HTTP/2`、`h2`、`stream.priority`、`PING frame`、`SETTINGS`、`WINDOW_UPDATE`、`PUSH_PROMISE` 等关键词，在整个代码库中**零命中**。

### 3.4 WebSocket 多路复用（替代方案）

**文件**：`packages/api/gateway/src/stream-server.ts`

虽然 HTTP/2 多路复用未实现，但项目在 WebSocket 层实现了**应用层多路复用**：

```typescript
// stream-server.ts:86-116
class RemoteStreamMuxConnection {
  private readonly streams = new Map<string, ActiveStream>()
  private writes = Promise.resolve()  // 串行化写入

  private receive(text: string): void {
    const message = parseRemoteStreamClientMessage(text)
    if (message.type === 'cancel') {
      this.streams.get(message.streamId)?.abort.abort(new Error('Remote stream cancelled'))
      return
    }
    const abort = new AbortController()
    const active: ActiveStream = { abort, done: Promise.resolve() }
    this.streams.set(message.streamId, active)
    const done = this.pump(message.streamId, message.endpoint, message.payload, active)
    active.done = done
  }
}
```

**特点**：
- 单一 WebSocket 连接承载多个逻辑流（streamId 标识）
- 支持取消帧（`{ type: 'cancel', streamId }`）
- 心跳机制（`heartbeatIntervalMs`）
- 串行化写入（`this.writes = delivery.catch(...)`）

### 3.5 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| HTTP/2 协商 | 依赖 undici 默认 | reqwest 默认开启 | ✅ 已满足 |
| 流优先级 | ❌ | ❌ | 低优先级 |
| 连接级流控 | ❌ | ❌ | 低优先级 |
| 应用层多路复用 | ✅ WebSocket | ❌ | 按需添加 |

---

## 4. 代理链与隧道

### 4.1 现状总结

| 维度 | 实现情况 | 位置 |
|------|---------|------|
| SOCKS5 代理 | ❌ 未实现 | — |
| HTTPS 代理 | ❌ 未实现 | — |
| CONNECT 隧道 | ❌ 未实现 | — |
| 代理认证 | ❌ 未实现 | — |
| 代理轮换 | ❌ 未实现 | — |
| 环境变量代理 | ⚠️ 依赖 undici 默认 | 原生 fetch |

### 4.2 无代理相关代码

搜索 `ProxyAgent`、`HttpProxyAgent`、`SOCKS`、`CONNECT`、`tunnel`、`proxy` 等关键词：
- `ProxyAgent`：0 命中
- `HTTPS_PROXY`：0 命中
- `SOCKS`：0 命中

### 4.3 undici 的默认代理行为

undici 从 v5.7+ 开始支持 `EnvHttpProxyAgent`，会读取 `HTTP_PROXY`、`HTTPS_PROXY`、`NO_PROXY` 环境变量。但 deepseek-harness **没有显式配置**，完全依赖运行时环境。

### 4.4 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| HTTP 代理 | 依赖环境变量 | reqwest 支持 | ✅ 已满足 |
| SOCKS5 | ❌ | reqwest 支持 | ✅ 已满足 |
| 代理认证 | ❌ | reqwest 支持 | ✅ 已满足 |
| 代理轮换 | ❌ | ❌ | 低优先级 |

---

## 5. TLS 配置

### 5.1 现状总结

| 维度 | 实现情况 | 位置 |
|------|---------|------|
| 证书验证 | ⚠️ 依赖 Node 默认（开启） | — |
| SNI | ⚠️ 依赖 undici 默认（自动） | — |
| ALPN | ⚠️ 依赖 undici 默认 | — |
| 证书固定 | ❌ 未实现 | — |
| TLS 1.3 | ⚠️ 依赖 Node 默认（支持） | — |
| 自签证书 | ❌ 不支持 | — |
| mTLS | ❌ 未实现 | — |

### 5.2 无 TLS 相关代码

搜索 `tls.`、`https.`、`certificate`、`ca`、`cert`、`key`、`passphrase`、`secureProtocol`、`checkServerIdentity` 等关键词，在整个代码库中**零命中**（除了 node:https 的类型引用）。

### 5.3 地址钉扎（SSRF 防御）

虽然不是 TLS 配置，但 web-fetch-http 的**地址钉扎**机制间接影响 TLS 连接：

**文件**：`packages/web/web-fetch-http/src/network.ts:74-109`

```typescript
export async function resolvePublicAddresses(
  hostname: string,
  signal: AbortSignal,
  resolver: AddressResolver = systemLookup,
): Promise<PublicAddress[]> {
  const resolved = await raceWithSignal(
    resolver(unbracketed, { all: true, order: 'verbatim' }), signal
  )
  // 验证每个地址都是公网单播地址
  for (const entry of resolved) {
    if (!isPublicIpAddress(entry.address)) {
      throw new WebError(`URL hostname "${hostname}" resolves to a non-public IP address`, 'WEB_BLOCKED_URL')
    }
  }
  return addresses
}
```

**作用**：
- 在 DNS 解析阶段阻止私有地址（10.x、172.16-31.x、192.168.x、fd00::/8 等）
- 防止 DNS 重绑定攻击（先返回公网 IP 通过校验，连接时返回私网 IP）
- 通过 `createPinnedLookup()` 将已验证地址集注入 undici connect.lookup

### 5.4 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| 证书验证 | Node 默认 | rustls 默认 | ✅ 已满足 |
| SNI | undici 自动 | rustls 自动 | ✅ 已满足 |
| 证书固定 | ❌ | ❌ | 低优先级 |
| 自签证书 | ❌ | 可配置 accept_invalid_certs | ✅ laew 已满足 |
| mTLS | ❌ | ❌ | 按需添加 |
| SSRF 防御 | ✅ 地址钉扎 | ❌ | 按需添加 |

---

## 6. 请求/响应拦截器

### 6.1 现状总结

| 维度 | 实现情况 | 位置 |
|------|---------|------|
| 中间件链 | ✅ Cordis 事件系统 | `llm/src/index.ts:67` |
| 请求转换 | ✅ waterfall 拦截 | `llm/stream` 事件 |
| 响应解耦 | ✅ 流式 chunk 协议 | `types.ts:364-376` |
| 错误标准化 | ✅ LlmError + code | `error.ts:13-22` |

### 6.2 Cordis Waterfall 拦截器

**文件**：`packages/llm/llm/src/index.ts:49-70`

```typescript
declare module '@deepseek-ai/cordis' {
  interface Events {
    /**
     * Waterfall around every streaming model call (retry, replay, routing).
     * Bound to the {@link LlmRuntime}; call `next()` to reach the resolved
     * adapter's stream, or yield your own chunks to short-circuit.
     * @mode waterfall
     */
    'llm/stream'(this: LlmRuntime, options: GenerateOptions, next: () => AsyncIterable<StreamChunk>): AsyncIterable<StreamChunk>
  }
}
```

**waterfall 语义**：
- 多个监听器按注册顺序执行
- 每个监听器可选择：
  - 调用 `next()` 继续下一个监听器
  - 自行 yield chunks 短路后续
  - 修改 options（只读推荐）

### 6.3 拦截器注册示例

**文件**：`packages/llm/llm-retry/src/index.ts:210-219`

```typescript
const disposeListener = ctx.on('agent/request-error', (
  payload,
  next: () => Promise<RequestErrorAction>,
) => {
  if (lifetime.signal.aborted) return Promise.resolve<RequestErrorAction>(undefined)
  return track(recover(payload, next))
})
```

### 6.4 错误标准化

**文件**：`packages/llm/llm/src/error.ts:13-22`

```typescript
export class HarnessError extends Error {
  readonly code: string    // 稳定机器路由码

  constructor(message: string, code: string, options?: ErrorOptions) {
    super(message, options)
    this.code = code
    this.name = new.target.name
  }
}
```

**文件**：`packages/llm/llm/src/adapter-failure.ts:16-28`

```typescript
export function normalizeLlmFailure(value: unknown): LlmFailure {
  const error = value instanceof Error
    ? value
    : new HarnessError(thrownMessage(value), 'UNKNOWN', { cause: value })
  // 跨包副本保留自有数据但不保留类身份
  const carried = ownFailureSnapshot(error)
  if (carried !== undefined && carried.code === ownErrorCode(error)) return carried
  return Object.freeze({
    message: errorMessage(error),
    code: harnessErrorCode(error),
  })
}
```

### 6.5 错误链渲染

**文件**：`packages/llm/llm/src/error.ts:114-154`

```typescript
export function errorChain(value: unknown): string {
  const path = new Set<unknown>()
  const render = (current: unknown): string => {
    if (path.has(current)) return '<circular cause>'
    path.add(current)
    try {
      if (!(current instanceof Error)) { /* ... */ }
      const message = current.message === '' ? current.name : current.message
      const members = current instanceof AggregateError && current.errors.length > 0
        ? ` [${current.errors.map(render).join('; ')}]`
        : ''
      const causeText = current.cause === undefined ? '' : render(current.cause)
      const cause = causeText === '' || causeText === message ? '' : `: ${causeText}`
      return `${message}${members}${cause}`
    } catch {
      return '<unrenderable value>'
    } finally {
      path.delete(current)
    }
  }
  return render(value)
}
```

**特别处理**：
- 循环引用检测（`path` Set）
- AggregateError 成员展开
- 包装器消息去重（避免 `new HarnessError(String(value), code, { cause: value })` 重复渲染）
- 恶意 getter 防护（try/catch 包裹）

### 6.6 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| 中间件链 | Cordis waterfall | 无 | 可选 |
| 错误标准化 | ✅ code + message | thiserror | ✅ 已满足 |
| 错误链 | ✅ 递归渲染 | anyhow::Error chain | ✅ 已满足 |
| 跨包错误保留 | ✅ ownFailureSnapshot | ❌ | 单 crate 不需要 |

---

## 7. 流式处理

### 7.1 架构概览

```
┌──────────────────────────────────────────────────────────────┐
│                    流式处理架构                               │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ llm-deepseek SSE 解析                                  │ │
│  │  ReadableStream → TextDecoderStream → EventSourceParser│ │
│  │  → parseSse() → translate() → StreamChunk              │ │
│  └────────────────────────────────────────────────────────┘ │
│                          │                                   │
│                          ▼                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ llm-pi-ai 事件翻译                                     │ │
│  │  AssistantMessageEvent → StreamChunk                   │ │
│  │  text_delta / thinking_delta / toolcall_delta          │ │
│  └────────────────────────────────────────────────────────┘ │
│                          │                                   │
│                          ▼                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ web-fetch-http 字节流控                                │ │
│  │  ReadableStream → readCapped(maxResponseBytes)         │ │
│  │  → Uint8Array 聚合 → TextDecoder                       │ │
│  └────────────────────────────────────────────────────────┘ │
│                          │                                   │
│                          ▼                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ http-bridge 背压                                       │ │
│  │  res.write(chunk) → false → await 'drain'              │ │
│  └────────────────────────────────────────────────────────┘ │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 7.2 SSE 解析（llm-deepseek）

**文件**：`packages/llm/llm-deepseek/src/sse.ts:28-40`

```typescript
export async function* parseSse(
  stream: ReadableStream<BufferSource>,
  onComment?: (comment: string) => void,
): AsyncGenerator<string> {
  const events = stream
    .pipeThrough(new TextDecoderStream())           // UTF-8 解码
    .pipeThrough(new EventSourceParserStream({ onComment }))  // SSE 帧解析
  for await (const { data } of events) {
    yield data
    if (data === DONE) return
  }
  throw new LlmError('SSE stream ended without [DONE]', 'STREAM_CLOSED')
}
```

**关键设计**：
- 使用 `eventsource-parser/stream`（npm 库）处理 SSE 帧边界
- 严格模式：未终止的尾部视为截断（不 flush）
- `[DONE]` 哨兵作为正常结束标志
- 注释通过 `onComment` 回调报告（用于 idle watchdog 心跳）

### 7.3 字节流控（web-fetch-http）

**文件**：`packages/web/web-fetch-http/src/provider.ts:168-221`

```typescript
private async readCapped(response: Response, signal: AbortSignal): Promise<{ bytes: Uint8Array; truncatedByBytes: boolean }> {
  const declared = response.headers.get('content-length')
  if (declared !== null) {
    const length = Number(declared)
    if (Number.isFinite(length) && length > this.limits.maxResponseBytes) {
      await response.body?.cancel()
      throw new WebError(`response exceeds the maximum of ${this.limits.maxResponseBytes} bytes`, 'WEB_FETCH_TOO_LARGE')
    }
  }

  const chunks: Uint8Array[] = []
  let total = 0
  let truncatedByBytes = false
  const reader = response.body.getReader() as ReadableStreamDefaultReader<Uint8Array>
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      const remaining = this.limits.maxResponseBytes - total
      if (value.byteLength > remaining) {
        chunks.push(value.subarray(0, remaining))  // 只取剩余容量
        total += remaining
        truncatedByBytes = true
        break
      }
      chunks.push(value)
      total += value.byteLength
    }
  } finally {
    await reader.cancel().catch(() => {})  // 尽力取消
  }

  // 聚合 chunks 到单一 Uint8Array
  const bytes = new Uint8Array(total)
  let offset = 0
  for (const chunk of chunks) {
    bytes.set(chunk, offset)
    offset += chunk.byteLength
  }
  return { bytes, truncatedByBytes }
}
```

**背压策略**：
- 读取上限：`maxResponseBytes`（默认 5MB）
- 超过上限时**截断**而非拒绝（Content-Length 未声明时）
- Content-Length 超过上限时**立即拒绝**（避免无意义下载）
- 读取完成后取消底层流（释放 socket）

### 7.4 HTTP 桥背压

**文件**：`packages/client/connection/src/http-bridge.ts:80-98`

```typescript
for await (const chunk of response.body) {
  // Backpressure: a false return means the socket buffer is full — wait for drain
  if (!res.write(chunk)) {
    await new Promise<void>((resolve) => {
      const done = (): void => {
        res.off('drain', done)
        res.off('close', done)
        resolve()
      }
      res.once('drain', done)
      res.once('close', done)
    })
  }
}
res.end()
```

**背压机制**：
- `res.write()` 返回 `false` 表示内核缓冲区满
- 等待 `drain` 事件后再继续写入
- `close` 事件也能解除等待（防止永久阻塞）

### 7.5 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| SSE 解析 | eventsource-parser | 手动解析 | 可考虑 sse-stream crate |
| 字节上限 | ✅ 5MB 默认 | 无 | 按需添加 |
| 背压 | ✅ drain 事件 | tokio 异步背压 | ✅ 已满足 |
| 流取消 | ✅ reader.cancel() | Drop 取消 | ✅ 已满足 |

---

## 8. 超时与取消

### 8.1 架构概览

**文件**：`packages/util/timeout/src/index.ts`（190 行，核心基础设施）

```
┌──────────────────────────────────────────────────────────────┐
│                 dsh-timeout 超时基础设施                      │
├──────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ TimeoutReason (Error 子类)                             │ │
│  │  • code: 能力拥有者代码（如 'BASH_TIMEOUT'）           │ │
│  │  • timeoutMs: 实际超时毫秒数                           │ │
│  └────────────────────────────────────────────────────────┘ │
│                          │                                   │
│                          ▼                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ clampTimeout(requested, def, max, name)                │ │
│  │  • 验证正有限数                                         │ │
│  │  • 应用默认值 + 上限                                    │ │
│  │  • 上限 MAX_TIMER_DELAY_MS = 2,147,483,647 (24.8 天)   │ │
│  └────────────────────────────────────────────────────────┘ │
│                          │                                   │
│                          ▼                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ deadline(upstream, timeoutMs, code) → Deadline         │ │
│  │  • 融合上游取消 + 超时                                  │ │
│  │  • AbortSignal.any 竞争（先触发者胜）                   │ │
│  │  • Symbol.dispose 清理定时器                            │ │
│  └────────────────────────────────────────────────────────┘ │
│                          │                                   │
│                          ▼                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ idleWatchdog(upstream, timeoutMs, code) → IdleWatchdog │ │
│  │  • 可重武装空闲计时器                                   │ │
│  │  • next(iterator) 武装 → 等待 → 解除                   │ │
│  │  • pulse() 重武装（传输活动回调）                       │ │
│  └────────────────────────────────────────────────────────┘ │
│                          │                                   │
│                          ▼                                   │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ timeoutOf(signal, code?) → TimeoutReason | undefined   │ │
│  │  • 从 AbortSignal.reason 恢复超时原因                   │ │
│  │  • 区分本层超时 vs 上游取消 vs 嵌套超时                 │ │
│  └────────────────────────────────────────────────────────┘ │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 8.2 Deadline（绝对超时）

**文件**：`packages/util/timeout/src/index.ts:91-113`

```typescript
export function deadline(
  upstream: AbortSignal | undefined,
  timeoutMs: number,
  code: string,
): Deadline {
  if (timeoutMs <= 0) {
    // 无超时（后台工作）：仅转发上游信号
    return { signal: upstream ?? new AbortController().signal, [Symbol.dispose]() {} }
  }

  assertTimerDelay(timeoutMs, 'deadline timeoutMs')

  const timer = new AbortController()
  const id = setTimeout(() => { timer.abort(new TimeoutReason(code, timeoutMs)) }, timeoutMs)
  return {
    // AbortSignal.any 采用先触发者的 reason
    signal: upstream !== undefined ? AbortSignal.any([upstream, timer.signal]) : timer.signal,
    [Symbol.dispose]() { clearTimeout(id) },
  }
}
```

**设计要点**：
- `timeoutMs <= 0` 是内部"无超时"哨兵（非公共 API）
- `AbortSignal.any` 竞争语义：先触发者决定 reason
- `timeoutOf()` 后续可识别是超时胜出还是上游取消胜出
- `Symbol.dispose` 支持 `using` 声明（TC39 Explicit Resource Management）

### 8.3 IdleWatchdog（空闲看门狗）

**文件**：`packages/util/timeout/src/index.ts:126-173`

```typescript
export function idleWatchdog(
  upstream: AbortSignal | undefined,
  timeoutMs: number,
  code: string,
): IdleWatchdog {
  assertTimerDelay(timeoutMs, 'idleWatchdog timeoutMs')
  const timeout = new AbortController()
  const signal = upstream === undefined
    ? timeout.signal
    : AbortSignal.any([upstream, timeout.signal])
  let timer: ReturnType<typeof setTimeout> | undefined
  let outstanding = false
  let disposed = false

  const arm = (): void => {
    if (timer !== undefined) clearTimeout(timer)
    timer = setTimeout(() => {
      timeout.abort(new TimeoutReason(code, timeoutMs))
    }, timeoutMs)
  }

  return {
    signal,
    async next<T>(iterator: AsyncIterator<T>): Promise<IteratorResult<T>> {
      if (disposed) throw new Error('idleWatchdog is disposed')
      if (outstanding) throw new Error('idleWatchdog next is already outstanding')
      outstanding = true
      arm()   // 武装计时器
      try {
        return await iterator.next()
      } finally {
        clearTimeout(timer)  // 解除
        timer = undefined
        outstanding = false
      }
    },
    pulse(): void {
      if (disposed || !outstanding) return
      arm()   // 重武装
    },
    [Symbol.dispose](): void {
      if (disposed) return
      disposed = true
      if (timer !== undefined) clearTimeout(timer)
      timer = undefined
    },
  }
}
```

**关键设计**：
- 计时器**仅在 next() 调用期间武装**，消费者思考时间不计入空闲
- `pulse()` 在传输活动时重武装（如收到 SSE comment）
- 状态机：`outstanding` 防止并发 next() 调用

### 8.4 使用示例（llm-deepseek）

**文件**：`packages/llm/llm-deepseek/src/adapter.ts:473-520`

```typescript
const consumer = new AbortController()
const upstream = options.signal === undefined
  ? consumer.signal
  : AbortSignal.any([options.signal, consumer.signal])
using watchdog = idleWatchdog(upstream, connection.streamIdleTimeoutMs, STREAM_IDLE_TIMEOUT_CODE)
const iterator = this.request(
  options, watchdog.signal, connection, apiKey, userId, attachments,
  () => { watchdog.pulse() },  // SSE 活动回调
)[Symbol.asyncIterator]()
try {
  while (true) {
    const result = await watchdog.next(iterator)  // 每次 next 武装计时器
    if (result.done) { exhausted = true; return }
    yield result.value
  }
} catch (error: unknown) {
  if (timeoutOf(watchdog.signal, STREAM_IDLE_TIMEOUT_CODE) !== undefined) {
    throw new LlmError(
      `DeepSeek stream idle timeout after ${connection.streamIdleTimeoutMs}ms`,
      'TIMEOUT',
      { cause: error },
    )
  }
  if (options.signal?.aborted) {
    throw new LlmError('DeepSeek request aborted by caller', 'ABORTED', { cause: error })
  }
  throw new LlmError(`DeepSeek API stream from ${connection.baseURL} failed`, 'TRANSPORT', { cause: error })
} finally {
  consumer.abort('DeepSeek stream consumer stopped')
  if (!exhausted && iterator.return !== undefined) {
    try { await iterator.return() } catch (_abortedTransportTeardown) { /* noop */ }
  }
}
```

### 8.5 使用示例（web-fetch-http）

**文件**：`packages/web/web-fetch-http/src/provider.ts:55-62`

```typescript
async fetch(request: WebFetchRequest, signal?: AbortSignal): Promise<WebFetchResult> {
  if (signal?.aborted) throw new WebError('web fetch aborted', 'WEB_ABORTED')

  // 一个信号同时停止请求和 body 读取
  using d = deadline(signal, this.limits.timeoutMs, 'WEB_FETCH_TIMEOUT')
  return await this.followAndRead(request.url, d.signal)
}
```

### 8.6 超时分类机制

**文件**：`packages/web/web-fetch-http/src/provider.ts:249-254`

```typescript
function translateAbortOrNetwork(error: unknown, signal: AbortSignal): WebError {
  const timeout = timeoutOf(signal, 'WEB_FETCH_TIMEOUT')
  if (timeout !== undefined) return new WebError('web fetch timed out', 'WEB_FETCH_TIMEOUT', { cause: timeout })
  if (signal.aborted) return new WebError('web fetch aborted', 'WEB_ABORTED', { cause: error })
  return new WebError(`web fetch failed: ${String(error)}`, 'WEB_PROVIDER_ERROR', { cause: error })
}
```

**三种结局**：
1. `WEB_FETCH_TIMEOUT`：本 provider 超时触发
2. `WEB_ABORTED`：上游取消（调用方 signal）
3. `WEB_PROVIDER_ERROR`：传输/网络故障（signal 未 abort）

### 8.7 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| 绝对超时 | ✅ deadline() | tokio::time::timeout | ✅ 已满足 |
| 空闲超时 | ✅ idleWatchdog | ❌ | 高价值（流式场景） |
| 超时分类 | ✅ timeoutOf + code | ❌ | 高价值 |
| 信号融合 | ✅ AbortSignal.any | tokio::select! | ✅ 已满足 |
| 资源清理 | ✅ using/Symbol.dispose | Drop | ✅ 已满足 |

---

## 9. 连接健康检查

### 9.1 现状总结

| 维度 | 实现情况 | 位置 |
|------|---------|------|
| 主动健康检查 | ❌ 未实现 | — |
| 被动健康检查 | ⚠️ 错误驱动（重试/重连） | 多处 |
| 熔断器 | ❌ 未实现 | — |
| 故障转移 | ⚠️ provider 路由切换 | llm-retry |
| 连接保活 | ✅ WebSocket 心跳 | gateway |

### 9.2 WebSocket 心跳（Gateway）

**文件**：`packages/api/gateway/src/stream-server.ts:69-78`

```typescript
private startHeartbeat(): void {
  if (this.heartbeatTimer !== undefined) return
  this.heartbeatTimer = setInterval(() => {
    for (const socket of this.server.clients) {
      if (socket.readyState === WebSocket.OPEN) socket.ping()
    }
  }, this.heartbeatIntervalMs)
  this.heartbeatTimer.unref()  // 不阻止进程退出
}
```

**特点**：
- 首次升级后启动，跨空客户端周期持续
- `unref()` 确保不阻止 Node 进程退出
- 仅发送 Ping 帧，不等待 Pong（fire-and-forget）

### 9.3 连接控制器（client/connection）

**文件**：`packages/client/connection/src/client/connection.ts:78-214`

```typescript
export class ConnectionController {
  private generation = 0
  private attempt = 0
  private current: AbortController | null = null
  private running = false
  private lastState: ConnectionState | null = null
  private readonly config: Required<ConnectionConfig>

  private backoffDelay(attempt: number): number {
    const { backoffBaseMs, backoffFactor, backoffMaxMs } = this.config
    const cap = Math.min(backoffMaxMs, backoffBaseMs * backoffFactor ** Math.max(0, attempt - 1))
    return cap / 2 + Math.random() * (cap / 2)  // [cap/2, cap] 均匀随机
  }

  private async loop(): Promise<void> {
    while (this.running) {
      const gen = ++this.generation
      const ac = new AbortController()
      this.current = ac
      // ... source 启动 ...
      try {
        const host = await Promise.race([
          waitForReady(ready, this.config.generationReadyTimeoutMs, ac.signal),
          sourceLost,
        ])
        this.attempt = 0
        this.emitState('connected')
      } catch {
        if (!ac.signal.aborted) ac.abort()
      }
      await failed
      if (!this.isRunning()) return
      this.emitState('reconnecting')
      this.attempt += 1
      const idle = new AbortController()
      await sleep(this.backoffDelay(this.attempt), idle.signal)
    }
  }
}
```

**默认配置**：
- `backoffBaseMs: 500`
- `backoffFactor: 2`
- `backoffMaxMs: 10_000`
- `generationReadyTimeoutMs: 3_000`

### 9.4 MCP 连接监督

**文件**：`packages/mcp/mcp-client/src/connection.ts:28-90`

```typescript
export interface ReconnectConfig {
  enabled?: boolean            // 默认 true
  initialDelayMs?: number      // 默认 500
  maxDelayMs?: number          // 默认 30_000
  maxAttempts?: number         // 默认 10
}

export const RECONNECT_DEFAULTS: Required<ReconnectConfig> = Object.freeze({
  enabled: true,
  initialDelayMs: 500,
  maxDelayMs: 30_000,
  maxAttempts: 10,
})
```

**稳定性窗口机制**：
- 连接持续 `maxDelayMs`（30s）视为"稳定"，重置 attempt 预算
- 崩溃循环（反复断连）会耗尽 `maxAttempts`（10 次）
- 耗尽后注销工具并停止，仅 disposal/HMR 可恢复

### 9.5 无熔断器

搜索 `circuit`、`breaker`、`half-open`、`failure threshold`、`recovery threshold` 等关键词，在整个代码库中**零命中**。

### 9.6 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| 主动健康检查 | ❌ | ❌ | 低优先级 |
| 被动重连 | ✅ 指数退避 | ❌ | 中优先级 |
| 熔断器 | ❌ | ❌ | `failsafe` crate |
| 故障转移 | ⚠️ provider 路由 | ❌ | 中优先级 |
| 连接保活 | ✅ WebSocket ping | ❌ | 按需添加 |

---

## 10. 性能优化

### 10.1 现状总结

| 维度 | 实现情况 | 位置 |
|------|---------|------|
| DNS 预解析 | ❌ 未实现 | — |
| TCP Fast Open | ❌ 未实现 | — |
| 零拷贝 | ❌ 未实现 | — |
| 缓冲区管理 | ⚠️ chunk 聚合 | web-fetch-http |
| 连接复用 | ⚠️ 依赖 undici 默认 | — |
| 请求去重 | ✅ 文件上传去重 | file-store.ts |
| 响应缓存 | ⚠️ 文件 ID 缓存 | upload-index.ts |

### 10.2 缓冲区管理（web-fetch-http）

**文件**：`packages/web/web-fetch-http/src/provider.ts:214-220`

```typescript
const bytes = new Uint8Array(total)
let offset = 0
for (const chunk of chunks) {
  bytes.set(chunk, offset)
  offset += chunk.byteLength
}
return { bytes, truncatedByBytes }
```

**策略**：
- 先收集 chunks 到数组（多次分配）
- 最后聚合到单一 Uint8Array（一次大分配）
- 避免频繁小分配，但峰值内存 = 2× 响应体（chunks + bytes）

### 10.3 请求去重（文件上传）

**文件**：`packages/llm/llm-deepseek/src/file-store.ts:142-176`

```typescript
ensureUploaded(version, connection, policy, signal): Promise<DeepSeekFileReference> {
  const key = `${scope}\0${version.variantId}`
  let active = this.inflight.get(key)
  if (active?.controller.signal.aborted) {
    this.inflight.delete(key)
    active = undefined
  }
  if (active !== undefined) return waitForUpload(active, signal)  // 复用进行中上传
  const controller = new AbortController()
  const shared: SharedUpload = { controller, settled: false, waiters: 0, promise: undefined as never }
  shared.promise = this.ensureUploadedOnce(version, connection, policy, controller.signal)
  this.inflight.set(key, shared)
  void shared.promise.finally(() => {
    if (this.inflight.get(key) === shared) this.inflight.delete(key)
  }).catch(() => {})
  return waitForUpload(shared, signal)
}
```

**并发控制**：
- 相同 `variantId` 的并发上传共享一个 AbortController
- 多个 waiter 通过 Promise 链等待同一上传
- 最后一个 waiter 取消时中止上传（`operation.waiters === 0`）

### 10.4 文件 ID 缓存

**文件**：`packages/llm/llm-deepseek/src/upload-index.ts`（225 行）

- 上传文件 ID 持久化到索引（内存/SQLite）
- 刷新边际（`refreshMarginSeconds`，默认 1 小时）内复用
- 过期前主动刷新（避免使用中文件失效）

### 10.5 无高级性能优化

搜索 `prefetch`、`preconnect`、`tcp fast open`、`TCP_FASTOPEN`、`splice`、`sendfile`、`zero-copy`、`buffer pool`、`object pool` 等关键词，在整个代码库中**零命中**。

### 10.6 laew 对比启示

| 能力 | deepseek-harness | laew 现状 | 建议 |
|------|-----------------|-----------|------|
| DNS 预解析 | ❌ | ❌ | 低优先级 |
| TCP Fast Open | ❌ | ❌ | 低优先级 |
| 零拷贝 | ❌ | ❌ | 低优先级 |
| 缓冲区池 | ❌ | ❌ | 低优先级 |
| 请求去重 | ✅ 文件上传 | ❌ | 按需添加 |
| 响应缓存 | ⚠️ 文件 ID | ❌ | 按需添加 |

---

## 11. 综合对比与 laew Gap 分析

### 11.1 能力矩阵

| 能力维度 | deepseek-harness | laew (Rust) | 差距等级 |
|---------|-----------------|-------------|---------|
| 连接池 | ⚠️ undici 默认 | ⚠️ reqreq 默认 | 🟢 持平 |
| 重试机制 | ✅ 双模式 + 退避 + 抖动 | ❌ 无 | 🔴 P0 差距 |
| HTTP/2 | ⚠️ undici 默认 | ⚠️ reqwest 默认 | 🟢 持平 |
| 代理 | ⚠️ 环境变量 | ✅ reqwest 支持 | 🟢 laew 优 |
| TLS | ⚠️ Node 默认 | ✅ rustls 可配 | 🟢 laew 优 |
| 拦截器 | ✅ Cordis waterfall | ❌ 无 | 🟡 可选 |
| 流式 | ✅ SSE + 背压 | ⚠️ 基础 SSE | 🟡 可增强 |
| 超时 | ✅ deadline + idleWatchdog | ⚠️ 基础超时 | 🟡 可增强 |
| 健康检查 | ⚠️ 被动重连 | ❌ 无 | 🟡 中优先级 |
| 性能优化 | ⚠️ 基础 | ⚠️ 基础 | 🟢 持平 |

### 11.2 laew 优先改造清单

#### P0（紧急）

1. **重试机制**（对标 `llm-retry`）
   - 实现 normal 模式（5 次上限 + 可重试错误码白名单）
   - 指数退避 + 对称抖动（`backoff` crate）
   - 解析 `Retry-After` header
   - 默认可重试码：`[EMPTY_RESPONSE, RATE_LIMIT, SERVER, TIMEOUT, TRANSPORT]`

2. **空闲超时**（对标 `idleWatchdog`）
   - 流式响应场景：两次 chunk 间隔超过阈值则中止
   - 可重武装（收到 chunk 时重置计时器）
   - 与调用方取消信号融合

#### P1（重要）

3. **超时分类**（对标 `timeoutOf`）
   - 区分：连接超时 / 读取超时 / 请求超时 / 全局超时 / 调用方取消
   - 错误码映射到 `AgentError` 变体

4. **连接重连**（对标 `ConnectionController`）
   - WebSocket 场景：断连后指数退避重连
   - 状态机：connected → reconnecting → connected

#### P2（进阶）

5. **SSRF 防御**（对标 `network.ts` 地址钉扎）
   - 可选功能：解析 DNS 后验证非私有地址
   - 防止 DNS 重绑定（钉扎已验证地址集）

6. **应用层多路复用**（对标 `stream-server.ts`）
   - 单连接多流（streamId 标识）
   - 心跳保活 + 取消帧

### 11.3 推荐 Rust Crates

| 功能 | 推荐 crate | 说明 |
|------|-----------|------|
| 重试 + 退避 | `backoff` | 指数退避 + 抖动 |
| 熔断器 | `failsafe` | 三态熔断（Closed/Open/HalfOpen） |
| 超时 | `tokio::time::timeout` | 基础超时 |
| SSE 解析 | `sse-stream` / `eventsource` | SSE 帧解析 |
| HTTP 客户端 | `reqwest` | 已使用，支持 HTTP/2 + 代理 |
| TLS 配置 | `rustls` | 已使用，支持证书固定 |
| 连接池 | `reqwest` 内置 | 已使用 |
| 并发去重 | `tokio::sync::Mutex` + HashMap | 文件上传去重模式 |

---

## 12. 关键文件索引

| 文件路径 | 行数 | 核心能力 |
|---------|------|---------|
| `packages/llm/llm/src/retry-policy.ts` | 195 | 重试策略配置（双模式 + 退避 + 抖动） |
| `packages/llm/llm-retry/src/index.ts` | 226 | 重试执行（Cordis 插件 + 持久化历史） |
| `packages/llm/llm-retry/src/types.ts` | 48 | 重试事件类型定义 |
| `packages/llm/llm-retry/src/history.ts` | 33 | 重试历史查询 |
| `packages/util/timeout/src/index.ts` | 190 | 超时基础设施（deadline/idleWatchdog/TimeoutReason） |
| `packages/web/web-fetch-http/src/network.ts` | 252 | undici Agent + DNS 钉扎 + SSRF 防御 |
| `packages/web/web-fetch-http/src/provider.ts` | 254 | HTTP 获取（超时 + 重定向 + 字节上限） |
| `packages/web/web-fetch-http/src/policy.ts` | 118 | URL 验证 + Content-Type 分类 |
| `packages/llm/llm-deepseek/src/adapter.ts` | 707 | DeepSeek 适配器（fetch + SSE + idleWatchdog） |
| `packages/llm/llm-deepseek/src/sse.ts` | 40 | SSE 流解析 |
| `packages/llm/llm-deepseek/src/translate.ts` | 194 | SSE payload → StreamChunk 翻译 |
| `packages/llm/llm-deepseek/src/file-store.ts` | 331 | 文件上传去重 + 配额恢复 |
| `packages/llm/llm-deepseek/src/files-api.ts` | 257 | Files API 客户端 |
| `packages/llm/llm-pi-ai/src/stream.ts` | 231 | pi-ai 流翻译 + 错误分类 |
| `packages/llm/llm/src/error.ts` | 163 | 错误基类 + 错误链渲染 + 错误码分类 |
| `packages/llm/llm/src/adapter-failure.ts` | 104 | 跨包错误标准化 |
| `packages/llm/llm/src/index.ts` | 1091 | LlmRuntime + 适配器注册 + waterfall |
| `packages/llm/llm/src/types.ts` | 429 | 消息/流式/错误类型定义 |
| `packages/api/gateway/src/stream-server.ts` | 208 | WebSocket 多路复用 + 心跳 |
| `packages/api/gateway/src/client/stream-client.ts` | 348 | WebSocket 客户端 + 重连退避 |
| `packages/client/connection/src/http-bridge.ts` | 99 | node:http ↔ fetch 桥 + 背压 |
| `packages/client/connection/src/client/connection.ts` | 242 | 连接控制器 + 指数退避重连 |
| `packages/mcp/mcp-client/src/connection.ts` | 351 | MCP 连接监督 + 稳定性窗口 |

---

## 13. 总结

deepseek-harness 的 HTTP 客户端基础设施呈现以下特征：

1. **不造轮子**：依赖 Node 原生 fetch（undici 内核）+ 第三方库，不实现底层 HTTP 协议逻辑
2. **能力分包**：超时（dsh-timeout）、重试（llm-retry）、流式（llm-deepseek/llm-pi-ai）、连接（client/connection）各自独立
3. **provider 路由级策略**：每个 LLM provider 可独立配置重试策略，通过 Cordis 事件总线组合
4. **SSRF 防御**：web-fetch-http 的地址钉扎机制是安全亮点
5. **精密的重试基础设施**：双模式（normal/always）+ 指数退避 + 对称抖动 + 持久化历史 + Retry-After 优先
6. **先进的超时机制**：deadline + idleWatchdog + TimeoutReason 分类 + AbortSignal 融合
7. **缺失的能力**：HTTP/2 流控、代理链、TLS 配置、熔断器、主动健康检查、高级性能优化

对于 laew 的改造启示：
- **最应该借鉴**：重试机制（双模式 + 退避 + 抖动）、空闲超时（idleWatchdog）、超时分类（timeoutOf）
- **可以跳过**：HTTP/2 流控、代理链、TLS 配置（reqwest/rustls 已满足）
- **按需添加**：SSRF 防御（如果 laew 有 WebFetch 工具）、应用层多路复用（如果有多流场景）

---

*分析完成。共覆盖 23 个核心文件，约 6,500 行代码。*

---

## 附录 A：完整代码片段精选

### A.1 cancellableDelay + localDelay（重试核心算法）

**文件**：`packages/llm/llm-retry/src/index.ts:58-91`

```typescript
/**
 * 计算下次重试的延迟毫秒数
 * 公式：min(initialDelayMs * 2^(retry-1), maxDelayMs) * jitter
 * jitter 范围：[1 - jitterRatio, 1 + jitterRatio] 均匀随机
 */
function localDelay(config: ResolvedRetryPolicy, retry: number, random: () => number): number {
  const exponent = Math.min(retry - 1, 1024)
  const exponential = Math.min(config.initialDelayMs * 2 ** exponent, config.maxDelayMs)
  const jitter = 1 - config.jitterRatio + 2 * config.jitterRatio * random()
  return Math.min(exponential * jitter, config.maxDelayMs)
}

/**
 * 可取消的延迟等待
 * @returns true=延迟正常完成, false=被取消
 */
function cancellableDelay(delayMs: number, signal: AbortSignal): Promise<boolean> {
  if (signal.aborted) return Promise.resolve(false)
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolve(true)
    }, delayMs)
    function onAbort(): void {
      clearTimeout(timer)
      resolve(false)
    }
    signal.addEventListener('abort', onAbort, { once: true })
  })
}
```

### A.2 createPinnedLookup（DNS 钉扎）

**文件**：`packages/web/web-fetch-http/src/network.ts:211-236`

```typescript
/**
 * 构建连接器 lookup 回调，返回固定的已验证地址集
 * 注意：不做任何网络 DNS 解析！
 */
export function createPinnedLookup(addresses: readonly PublicAddress[]): (
  hostname: string,
  options: LookupOptions,
  callback: LookupCallback,
) => void {
  return (hostname: string, options: LookupOptions, callback: LookupCallback): void => {
    const family = typeof options.family === 'number'
      ? options.family
      : options.family === 'IPv4' ? 4 : options.family === 'IPv6' ? 6 : 0
    const eligible = family === 0 ? addresses : addresses.filter(address => address.family === family)
    const selected = eligible[0]
    if (selected === undefined) {
      const error = Object.assign(new Error(`no validated address for ${hostname} in family ${family}`), {
        code: 'ENOTFOUND',
        hostname,
      })
      callback(error, options.all === true ? [] : '', family)
      return
    }
    if (options.all === true) {
      callback(null, eligible.map(address => ({ ...address })))
      return
    }
    callback(null, selected.address, selected.family)
  }
}
```

### A.3 normalizeLlmFailure（跨包错误标准化）

**文件**：`packages/llm/llm/src/adapter-failure.ts:16-28`

```typescript
/**
 * 从适配器抛出的任意值中提取可序列化的 provider 失败事实
 * 跨包副本保留自有数据但不保留类身份
 */
export function normalizeLlmFailure(value: unknown): LlmFailure {
  const error = value instanceof Error
    ? value
    : new HarnessError(thrownMessage(value), 'UNKNOWN', { cause: value })
  // 信任携带的事实仅当自有属性一致且通过验证
  const carried = ownFailureSnapshot(error)
  if (carried !== undefined && carried.code === ownErrorCode(error)) return carried
  return Object.freeze({
    message: errorMessage(error),
    code: harnessErrorCode(error),
  })
}

/** 读取外部错误自有数据-backed code（不调用 accessor） */
function ownErrorCode(error: Error): unknown {
  try {
    const descriptor = Object.getOwnPropertyDescriptor(error, 'code')
    return descriptor !== undefined && 'value' in descriptor ? descriptor.value : undefined
  } catch (_sdkPropertyTrap) {
    return undefined
  }
}

/** 验证并分离任意可序列化失败载荷 */
function failureSnapshot(value: unknown): LlmFailure | undefined {
  if (typeof value !== 'object' || value === null) return undefined
  try {
    const candidate = value as Partial<LlmFailure>
    const message = candidate.message
    const code = candidate.code
    const status = candidate.status
    const providerRetryAfterMs = candidate.providerRetryAfterMs
    const requestId = candidate.requestId
    // 严格类型和范围校验
    if (typeof message !== 'string' || message.length === 0
      || typeof code !== 'string' || code.length === 0
      || (status !== undefined && (!Number.isInteger(status) || status < 100 || status > 599))
      || (providerRetryAfterMs !== undefined
        && (!Number.isFinite(providerRetryAfterMs) || providerRetryAfterMs <= 0))
      || (requestId !== undefined && (typeof requestId !== 'string' || requestId.length === 0))) return undefined
    return Object.freeze({
      message,
      code,
      ...status === undefined ? {} : { status },
      ...providerRetryAfterMs === undefined ? {} : { providerRetryAfterMs },
      ...requestId === undefined ? {} : { requestId },
    })
  } catch (_sdkFailureGetter) {
    return undefined
  }
}
```

### A.4 httpErrorCode（HTTP → 错误码映射）

**文件**：`packages/llm/llm-deepseek/src/adapter.ts:332-344`

```typescript
/**
 * 将 HTTP 状态映射到稳定的 LlmError 代码
 */
export function httpErrorCode(status: number, error?: WireError['error']): string {
  if (status === 401 || status === 403) return 'AUTH'
  if (status === 413) return 'INVALID_REQUEST'
  const detail = [error?.code, error?.type, error?.message].filter(Boolean).join(' ')
  if (isQuotaExceededError(detail)) return QUOTA_EXCEEDED_CODE
  if (status === 429) return 'RATE_LIMIT'
  if (status === 400) {
    if (isContextWindowExceededError(detail)) return CONTEXT_WINDOW_EXCEEDED_CODE
    return 'INVALID_REQUEST'
  }
  if (status >= 500) return 'SERVER'
  return `HTTP_${status}`
}
```

### A.5 backoffDelay（连接重连退避）

**文件**：`packages/client/connection/src/client/connection.ts:108-112`

```typescript
private backoffDelay(attempt: number): number {
  const { backoffBaseMs, backoffFactor, backoffMaxMs } = this.config
  const cap = Math.min(backoffMaxMs, backoffBaseMs * backoffFactor ** Math.max(0, attempt - 1))
  return cap / 2 + Math.random() * (cap / 2)
}
```

### A.6 resolvePublicAddresses（SSRF 防御核心）

**文件**：`packages/web/web-fetch-http/src/network.ts:74-109`

```typescript
export async function resolvePublicAddresses(
  hostname: string,
  signal: AbortSignal,
  resolver: AddressResolver = systemLookup,
): Promise<PublicAddress[]> {
  const unbracketed = stripIpv6Brackets(hostname)
  const literalFamily = isIP(unbracketed)
  const resolved = literalFamily === 0
    ? await raceWithSignal(resolver(unbracketed, { all: true, order: 'verbatim' }), signal)
    : [{ address: unbracketed, family: literalFamily }]

  if (resolved.length === 0) {
    throw new WebError(`hostname "${hostname}" resolved to no addresses`, 'WEB_PROVIDER_ERROR')
  }

  const hasIpv6 = resolved.some(entry => entry.family === 6 && isIP(entry.address) === 6)
  const nat64Prefixes = hasIpv6
    ? await discoverNat64Prefixes(signal, resolver)
    : []

  const addresses: PublicAddress[] = []
  for (const entry of resolved) {
    if ((entry.family !== 4 && entry.family !== 6) || isIP(entry.address) !== entry.family) {
      throw new WebError(`hostname "${hostname}" resolved to an invalid IP address`, 'WEB_PROVIDER_ERROR')
    }
    if (!isPublicIpAddress(entry.address)) {
      throw new WebError(`URL hostname "${hostname}" resolves to a non-public IP address`, 'WEB_BLOCKED_URL')
    }
    const translatedIpv4 = translatedIpv4Address(entry.address, nat64Prefixes)
    if (translatedIpv4 !== undefined && !isPublicIpAddress(translatedIpv4)) {
      throw new WebError(`URL hostname "${hostname}" resolves through NAT64 to a non-public IPv4 address`, 'WEB_BLOCKED_URL')
    }
    addresses.push({ address: entry.address, family: entry.family })
  }
  return addresses
}
```

---

## 附录 B：错误码完整枚举

| 错误码 | 来源 | 含义 | 默认可重试 |
|-------|------|------|-----------|
| `EMPTY_RESPONSE` | llm/src/error.ts:39 | 模型返回零内容完成 | ✅ |
| `RATE_LIMIT` | llm-deepseek/src/adapter.ts:337 | HTTP 429 | ✅ |
| `SERVER` | llm-deepseek/src/adapter.ts:342 | HTTP 5xx | ✅ |
| `TIMEOUT` | adapter-failure.ts | 超时错误 | ✅ |
| `TRANSPORT` | adapter-failure.ts | 网络传输错误 | ✅ |
| `AUTH` | llm-deepseek/src/adapter.ts:333 | HTTP 401/403 | ❌ |
| `INVALID_REQUEST` | llm-deepseek/src/adapter.ts:334 | HTTP 400/413 | ❌ |
| `QUOTA` | llm/src/error.ts:28 | 账户配额耗尽 | ❌ |
| `CONTEXT_WINDOW_EXCEEDED` | llm/src/error.ts:25 | 上下文窗口溢出 | ❌ |
| `INVALID_CREDENTIAL` | llm/src/error.ts:48 | 凭证格式错误 | ❌ |
| `ABORTED` | adapter-failure.ts | 调用方取消 | ❌ |
| `UNKNOWN` | adapter-failure.ts | 未知错误 | ❌ |
| `STREAM_CLOSED` | sse.ts:39 | SSE 流异常关闭 | ❌ |
| `MALFORMED_RESPONSE` | translate.ts:133 | JSON 解析失败 | ❌ |
| `PI_AI_ERROR` | llm-pi-ai/src/stream.ts:66 | pi-ai 库错误 | ❌ |

---

## 附录 C：配置默认值速查

| 配置项 | 默认值 | 位置 |
|-------|-------|------|
| `maxRetries` | 5 | retry-policy.ts:14 |
| `initialDelayMs` (重试) | 500ms | retry-policy.ts:15 |
| `maxDelayMs` (重试) | 10000ms | retry-policy.ts:16 |
| `jitterRatio` | 0.1 | retry-policy.ts:17 |
| `streamIdleTimeoutMs` | 300_000 (5 分钟) | adapter.ts:138 |
| `filesApiTimeoutMs` | 60_000 | adapter.ts:158 |
| `fetchTimeoutMs` (web) | 30_000 | index.ts:48 |
| `maxResponseBytes` (web) | 5_000_000 | index.ts:46 |
| `maxBodyChars` (web) | 100_000 | index.ts:47 |
| `maxRedirects` (web) | 5 | index.ts:49 |
| `backoffBaseMs` (连接) | 500ms | connection.ts:29 |
| `backoffFactor` | 2 | connection.ts:30 |
| `backoffMaxMs` | 10_000ms | connection.ts:31 |
| `generationReadyTimeoutMs` | 3_000ms | connection.ts:32 |
| `maxAttempts` (MCP) | 10 | mcp-client/src/connection.ts:44 |
| `maxDelayMs` (MCP) | 30_000ms | mcp-client/src/connection.ts:43 |
| `MAX_TIMER_DELAY_MS` | 2_147_483,647 | timeout/src/index.ts:25 |

---

## 附录 D：Cordis 事件流中的 HTTP 请求生命周期

```
用户输入
   │
   ▼
┌──────────────────────────────────────────────────────────────┐
│ 1. agent/turn-start                                          │
└──────────────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────────────┐
│ 2. agent/step-start { turn, step }                           │
└──────────────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────────────┐
│ 3. request/header { provider, model, ... }                   │
│    (记录请求头，用于后续重试历史查询)                         │
└──────────────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────────────┐
│ 4. llm/stream (waterfall 事件)                               │
│    ├─ 监听器 1: 日志记录                                      │
│    ├─ 监听器 2: 指标采集                                      │
│    ├─ 监听器 3: 缓存命中检查                                  │
│    └─ 最终: adapter.stream() 实际请求                        │
└──────────────────────────────────────────────────────────────┘
   │
   ▼
┌──────────────────────────────────────────────────────────────┐
│ 5. HTTP 请求执行 (fetch / pi-ai)                             │
│    ├─ 成功 → 流式 chunk → 持续 yield                         │
│    ├─ 失败 → 抛出异常                                        │
│    └─ 取消 → AbortSignal 传播                                │
└──────────────────────────────────────────────────────────────┘
   │
   ├── 成功路径 ────────────────────────────────────────────┐
   │                                                        │
   ▼                                                        ▼
┌────────────────────────────────┐      ┌─────────────────────────────────┐
│ 6a. agent/step-end (成功)      │      │ 6b. agent/request-error (失败)   │
└────────────────────────────────┘      └─────────────────────────────────┘
                                      │
                                      ▼
                                   ┌──────────────────────────────────────┐
                                   │ 7. llm-retry 插件决策                 │
                                   │    ├─ 不可重试 → next() 透传         │
                                   │    ├─ 可重试 + 未达上限 → 等待重试   │
                                   │    └─ 已达上限 → next() 放弃         │
                                   └──────────────────────────────────────┘
                                      │
                                      ├── 重试 ──────────────────────┐
                                      │                               │
                                      ▼                               ▼
                                   ┌────────────────┐      ┌─────────────────────┐
                                   │ 8. llm/retry   │      │ 9. llm/retry-started│
                                   │    事件记录    │      │    事件记录         │
                                   └────────────────┘      └─────────────────────┘
                                      │
                                      ▼
                                   ┌──────────────────────────────────────┐
                                   │ 10. cancellableDelay (等待)           │
                                   │     ├─ 完成 → 回到步骤 4              │
                                   │     └─ 取消 → 放弃                    │
                                   └──────────────────────────────────────┘
```

---

*报告完成。deepseek-harness 的 HTTP 客户端基础设施以"不造轮子 + 能力分包"为核心哲学，在重试、超时、流式处理方面表现出色，但在 HTTP/2 流控、代理链、TLS 配置、熔断器方面完全依赖运行时默认。对于 laew 的改造，最应借鉴的是其重试机制（双模式 + 退避 + 抖动 + 持久化历史）和空闲超时（idleWatchdog）设计。*
