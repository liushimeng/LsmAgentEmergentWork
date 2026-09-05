# undici 拦截器与 Mock 系统深度分析

> 源码版本：undici (Node.js 官方 HTTP 客户端)
> 分析范围：interceptor 体系(8 个拦截器) + handler 体系(6 个处理器) + Mock 系统(12 个模块) + 工具层(5 个模块)
> 总计分析代码行数：~8,867 行
> 分析日期：2026-09-05

---

## 目录

1. [架构总览](#1-架构总览)
2. [Interceptor 链式调用模式](#2-interceptor-链式调用模式)
3. [核心拦截器逐一分析](#3-核心拦截器逐一分析)
   - 3.1 Cache 拦截器
   - 3.2 Retry 拦截器
   - 3.3 Redirect 拦截器
   - 3.4 DNS 拦截器
   - 3.5 Decompress 拦截器
   - 3.6 Deduplicate 拦截器
   - 3.7 Dump 拦截器
   - 3.8 Response-Error 拦截器
4. [Handler 装饰器模式](#4-handler-装饰器模式)
   - 4.1 DecoratorHandler 基类
   - 4.2 CacheHandler
   - 4.3 CacheRevalidationHandler
   - 4.4 RedirectHandler
   - 4.5 RetryHandler
   - 4.6 DeduplicationHandler
5. [Mock 系统架构](#5-mock-系统架构)
   - 5.1 MockAgent 总入口
   - 5.2 MockClient / MockPool
   - 5.3 MockInterceptor / MockScope
   - 5.4 MockUtils 核心引擎
   - 5.5 MockCallHistory 调用历史
   - 5.6 MockErrors / MockSymbols
6. [Snapshot 快照系统](#6-snapshot-快照系统)
   - 6.1 SnapshotAgent
   - 6.2 SnapshotRecorder
   - 6.3 SnapshotUtils
7. [工具层分析](#7-工具层分析)
8. [拦截器组合最佳实践与陷阱](#8-拦截器组合最佳实践与陷阱)
9. [跨模块设计模式总结](#9-跨模块设计模式总结)
10. [对 AI Agent HTTP 测试的借鉴](#10-对-ai-agent-http-测试的借鉴)
11. [laew 借鉴路线图](#11-laew-借鉴路线图)

---

## 1. 架构总览

undici 的拦截器与 Mock 系统采用三层架构：

```
┌─────────────────────────────────────────────────┐
│              用户层 (Client / Pool / Agent)       │
├─────────────────────────────────────────────────┤
│         Interceptor 层 (拦截器链)                 │
│  cache → retry → redirect → dns → decompress    │
│  deduplicate → dump → response-error             │
├─────────────────────────────────────────────────┤
│         Handler 层 (装饰器 / 处理器)              │
│  DecoratorHandler ← CacheHandler                 │
│                   ← CacheRevalidationHandler      │
│                   ← RedirectHandler               │
│                   ← RetryHandler                  │
│                   ← DeduplicationHandler           │
│                   ← DumpHandler                    │
│                   ← ResponseErrorHandler           │
│                   ← DecompressHandler              │
├─────────────────────────────────────────────────┤
│         Mock 层 (录制 / 回放 / 快照)              │
│  MockAgent → MockClient / MockPool                │
│  MockInterceptor → MockScope                      │
│  MockUtils (核心匹配引擎)                         │
│  MockCallHistory (调用历史)                        │
│  SnapshotAgent → SnapshotRecorder                 │
├─────────────────────────────────────────────────┤
│         工具层                                    │
│  cache.js (Cache-Control 解析)                    │
│  date.js (HTTP 日期解析)                          │
│  timers.js (低精度快速定时器)                      │
│  stats.js (连接统计)                              │
│  runtime-features.js (运行时特性检测)              │
└─────────────────────────────────────────────────┘
```

**核心设计原则**：

1. **拦截器是纯函数组合**：每个拦截器都是 `dispatch => (opts, handler) => ...` 的柯里化函数，通过 `compose` 链式组合。
2. **Handler 是装饰器模式**：所有 Handler 继承 `DecoratorHandler`，包装原始 handler 并在回调链中注入行为。
3. **Mock 系统是 Agent 替换**：`MockAgent` 继承 `Dispatcher`，通过 `buildMockDispatch` 替换真实 dispatch 为 mock dispatch。
4. **Snapshot 是录制/回放模式**：`SnapshotAgent` 继承 `MockAgent`，支持 record/playback/update 三种模式。

---

## 2. Interceptor 链式调用模式

### 2.0 八大拦截器全面对比

| 维度 | cache | retry | redirect | dns | decompress | deduplicate | dump | response-error |
|------|-------|-------|----------|-----|------------|-------------|------|----------------|
| **文件行数** | 618 | 19 | 21 | 575 | 292 | 117 | 112 | 95 |
| **Handler 行数** | 802+134 | 548 | 229 | 0（内嵌） | 内嵌 | 466 | 内嵌 | 内嵌 |
| **策略** | 替换 handler | 替换 handler | 替换 handler | 异步修改 origin | 替换 handler | 替换 handler | 替换 handler | 替换 handler |
| **核心能力** | HTTP 缓存 RFC 9111 | 重试 + Range 续传 | 3xx 跟随 | DNS 双栈 | 自动解压 | 请求合并 | 大小限制 | 4xx/5xx → Error |
| **是否短路** | 是（缓存命中） | 否 | 是（0 次） | 是（IP 跳过） | 是（HEAD） | 是（非安全方法） | 否 | 否 |
| **异步** | 否 | 是（backoff） | 否 | 是（DNS 查询） | 否 | 否 | 否 | 否 |
| **状态存储** | CacheStore | retryCount | history[] | DNSStorage | 解压流 | pendingRequests Map | size | statusCode/body |
| **协议合规** | RFC 9111 | - | RFC 7231 | - | - | - | - | - |
| **CVE 修复** | - | - | - | - | ✅ 编码层数限制 | - | - | - |

### 2.1 核心签名

每个拦截器遵循统一的函数签名：

```javascript
// 类型签名
type Interceptor = (dispatch: DispatchFn) => (opts: DispatchOptions, handler: DispatchHandler) => void
type DispatchFn = (opts: DispatchOptions, handler: DispatchHandler) => void
```

**文件路径**：所有拦截器位于 `lib/interceptor/*.js`

### 2.2 链式组合机制

拦截器通过 `Dispatcher.compose()` 方法链式组合，本质上是函数嵌套：

```javascript
// 组合伪代码
const composed = compose(cacheInterceptor, retryInterceptor, redirectInterceptor)
// 等价于:
// (dispatch) => cacheInterceptor(retryInterceptor(redirectInterceptor(dispatch)))
```

每一层拦截器可以：
- **短路跳过**：直接调用 `dispatch(opts, handler)` 跳过当前拦截
- **替换 handler**：创建新的 handler 装饰器包裹原始 handler
- **修改 opts**：变换请求参数（如 DNS 拦截器修改 origin）
- **异步延迟**：DNS 拦截器异步解析后才 dispatch

### 2.3 拦截器分类

| 拦截器 | 文件 | 行数 | 策略 | 说明 |
|--------|------|------|------|------|
| cache | `lib/interceptor/cache.js` | 618 | 替换 handler | HTTP 缓存，RFC 9111 完整实现 |
| retry | `lib/interceptor/retry.js` | 19 | 替换 handler | 委托给 RetryHandler |
| redirect | `lib/interceptor/redirect.js` | 21 | 替换 handler | 委托给 RedirectHandler |
| dns | `lib/interceptor/dns.js` | 575 | 异步修改 origin | DNS 预解析 + 双栈切换 |
| decompress | `lib/interceptor/decompress.js` | 292 | 替换 handler | 自动解压 gzip/br/deflate/zstd |
| deduplicate | `lib/interceptor/deduplicate.js` | 117 | 去重并发请求 | 同 key 请求共享响应 |
| dump | `lib/interceptor/dump.js` | 112 | 替换 handler | 大小限制 + 丢弃 body |
| response-error | `lib/interceptor/response-error.js` | 95 | 替换 handler | 4xx/5xx 转 Error |

---

## 3. 核心拦截器逐一分析

### 3.1 Cache 拦截器

**文件**：`lib/interceptor/cache.js`（618 行）

这是最复杂的拦截器，实现了完整的 HTTP 缓存（RFC 9111）。

#### 3.1.1 入口函数

```javascript
// lib/interceptor/cache.js 第 501 行
module.exports = (opts = {}) => {
  const {
    store = new MemoryCacheStore(),    // 缓存存储（默认内存）
    methods = ['GET'],                  // 可缓存的方法
    cacheByDefault = undefined,         // 默认缓存时长
    type = 'shared',                    // 缓存类型 shared|private
    origins = undefined                 // origin 白名单
  } = opts
  // ...
  return dispatch => {
    return (opts, handler) => {
      // 拦截逻辑
    }
  }
}
```

#### 3.1.2 核心决策流程

`handleResult()` 函数（第 379 行）是缓存命中与否的核心决策：

```
请求到达
  ├─ 缓存不存在 → handleUncachedResponse() → 包装 CacheHandler 转发
  ├─ 缓存已过期 (deleteAt) → 直接转发，不包装
  ├─ 需要验证 (stale/revalidate)
  │   ├─ stale-while-revalidate 窗口内 → 立即返回缓存 + 后台验证
  │   ├─ stale-if-error 窗口内 → 发送条件请求 + CacheRevalidationHandler
  │   └─ 其他 → 发送条件请求 + CacheRevalidationHandler
  └─ 缓存新鲜 → sendCachedValue() 直接返回缓存响应
```

#### 3.1.3 关键函数

| 函数 | 行号 | 用途 |
|------|------|------|
| `needsRevalidation()` | 80 | 判断是否需要验证（no-cache / conditional headers） |
| `isStale()` | 180 | 判断是否过期（含 max-stale / min-fresh） |
| `withinStaleWhileRevalidateWindow()` | 214 | RFC 5861 stale-while-revalidate 窗口判断 |
| `sendCachedValue()` | 293 | 构造虚拟 controller + Readable stream 发送缓存 |
| `handleUncachedResponse()` | 233 | 处理 only-if-cached 指令返回 504 |
| `makeRevalidationHeaders()` | 153 | 构造 If-Modified-Since / If-None-Match |

#### 3.1.4 缓存键生成

```javascript
// lib/util/cache.js 第 150 行
function makeCacheKey(opts) {
  return {
    origin: opts.origin?.toString() || '',
    method: opts.method,
    path: fullPath,        // path + query
    headers: opts.headers
  }
}
```

#### 3.1.5 stale-while-revalidate 实现

这是最精妙的部分（第 414-441 行）：

```javascript
if (!revalidate && withinStaleWhileRevalidateWindow(result, globalOpts.type)) {
  // 立即返回过期缓存
  sendCachedValue(handler, opts, result, age, null, true)
  // 后台 fire-and-forget 验证
  queueMicrotask(() => {
    const headers = makeRevalidationHeaders(opts, result)
    dispatch({ ...opts, headers }, new CacheHandler(globalOpts, cacheKey, {
      // 静默 handler - 只更新缓存，不返回给用户
      onRequestStart() {}, onResponseStart() {},
      onResponseData() {}, onResponseEnd() {}, onResponseError() {}
    }))
  })
  return true
}
```

**设计要点**：用 `queueMicrotask` 实现后台验证，使用空 handler 静默丢弃验证响应，仅更新缓存存储。

#### 3.1.6 sendCachedValue 虚拟响应构造

`sendCachedValue()`（第 293 行）构造一个虚拟的 controller + Readable stream 发送缓存内容：

```javascript
function sendCachedValue (handler, opts, result, age, readStream, stale) {
  // 创建虚拟 controller
  const controller = {
    pause () {},
    resume () {},
    abort (reason) {
      if (readStream) readStream.destroy()
    },
    rawHeaders: result.rawHeaders
  }

  // 构造响应头（包含 Age 头）
  const headers = { ...result.headers }
  if (age > 0) {
    headers.age = Math.floor(age / 1000).toString()
  }
  if (stale) {
    headers.warning = '110 - "Response is Stale"'
  }

  // 发送响应
  handler.onResponseStart(controller, result.statusCode, headers, result.statusMessage)

  // 创建 Readable stream 发送缓存 body
  const body = new Readable({ read() {} })
  handler.onResponseData(controller, body)

  // 异步读取缓存 store
  result.store.read(cacheKey, (err, storedBody) => {
    if (err) {
      handler.onResponseError(controller, err)
      return
    }
    body.push(storedBody)
    body.push(null)  // 结束
    handler.onResponseEnd(controller, result.trailers)
  })
}
```

#### 3.1.7 完整 handleResult 决策流程

```javascript
function handleResult (request, opts, result, context) {
  const { cacheControlDirectives } = result

  // 1. 缓存未命中
  if (result.statusCode === 0 && !cacheControlDirectives) {
    if (request.headers['only-if-cached']) {
      // only-if-cached 指令 + 无缓存 → 504
      handler.onResponseStart(controller, 504, ...)
      handler.onResponseData(controller, Buffer.from('Gateway Timeout'))
      handler.onResponseEnd(controller, {})
    }
    return handleUncachedResponse(...)
  }

  // 2. 缓存已过期（deleteAt 已过）
  if (Date.now() > result.deleteAt) {
    return handleUncachedResponse(...)
  }

  // 3. 需要验证（no-cache / conditional headers）
  const revalidate = needsRevalidation(result, cacheControlDirectives, request)
  if (revalidate) {
    // 发送条件请求
    const headers = makeRevalidationHeaders(opts, result)
    return dispatch({ ...opts, headers }, new CacheRevalidationHandler(...))
  }

  // 4. stale-while-revalidate 窗口内
  if (withinStaleWhileRevalidateWindow(result, globalOpts.type)) {
    sendCachedValue(handler, opts, result, age, null, true)
    queueMicrotask(() => { /* 后台验证 */ })
    return true
  }

  // 5. stale-if-error 窗口内
  if (withinStaleIfErrorWindow(result)) {
    // 发送真实请求 + CacheRevalidationHandler
    // 失败时使用缓存
    return dispatch(opts, new CacheRevalidationHandler(..., true))
  }

  // 6. 缓存新鲜
  return sendCachedValue(handler, opts, result, age, result.store.read(cacheKey), false)
}
```

#### 3.1.8 配置选项

```javascript
const cacheInterceptor = cache({
  store: new MemoryCacheStore(),   // 缓存存储（可替换为 Redis 等）
  methods: ['GET'],                 // 可缓存的方法
  cacheByDefault: undefined,        // 默认缓存时长（秒）
  type: 'shared',                   // 缓存类型 shared|private
  origins: ['http://api.example.com'] // origin 白名单（可选）
})
```

**store 接口**：

```javascript
interface CacheStore {
  get(key: CacheKey): Promise<GetResult>
  set(key: CacheKey, entry: CacheEntry): Promise<void>
  delete(key: CacheKey): Promise<boolean>
}
```

### 3.2 Retry 拦截器

**文件**：`lib/interceptor/retry.js`（19 行）

这是最简洁的拦截器，仅作为 RetryHandler 的入口包装：

```javascript
// lib/interceptor/retry.js 第 4 行
module.exports = globalOpts => {
  return dispatch => {
    return function retryInterceptor (opts, handler) {
      return dispatch(opts, new RetryHandler(
        { ...opts, retryOptions: { ...globalOpts, ...opts.retryOptions } },
        { handler, dispatch }
      ))
    }
  }
}
```

所有逻辑都在 `RetryHandler`（548 行）中。

### 3.3 Redirect 拦截器

**文件**：`lib/interceptor/redirect.js`（21 行）

同样简洁，委托给 RedirectHandler：

```javascript
// lib/interceptor/redirect.js 第 5 行
function createRedirectInterceptor ({
  maxRedirections, throwOnMaxRedirect,
  stripHeadersOnRedirect, stripHeadersOnCrossOriginRedirect
} = {}) {
  return (dispatch) => {
    return function Intercept (opts, handler) {
      if (maxRedirections == null || maxRedirections === 0) {
        return dispatch(opts, handler)  // 短路
      }
      const redirectHandler = new RedirectHandler(dispatch, maxRedirections, dispatchOpts, handler)
      return dispatch(dispatchOpts, redirectHandler)
    }
  }
}
```

### 3.4 DNS 拦截器

**文件**：`lib/interceptor/dns.js`（575 行）

这是唯一采用**异步修改 origin** 策略的拦截器，不替换 handler。

#### 3.4.1 核心架构

```
DNSInstance
  ├── DNSStorage          // 缓存层（Map，可替换为 Redis 等）
  ├── runLookup()         // 查询入口
  ├── #defaultLookup()    // 默认 DNS 查询（node:dns.lookup）
  ├── #defaultPick()      // 默认 IP 选择算法
  ├── pickFamily()        // 按 family 选择（双栈回退用）
  └── setRecords()        // 设置 DNS 记录（含 TTL）
```

#### 3.4.2 双栈故障转移

`DNSDispatchHandler`（第 386 行）实现了 IPv4/IPv6 双栈自动切换：

```javascript
onResponseError (controller, err) {
  switch (err.code) {
    case 'ETIMEDOUT':
    case 'ECONNREFUSED':
      if (this.#state.dualStack && this.#firstTry) {
        this.#firstTry = false
        // 从另一个 IP family 选择地址
        const otherFamily = this.#newOrigin.hostname[0] === '[' ? 4 : 6
        const ip = this.#state.pickFamily(this.#origin, otherFamily)
        if (ip == null) { super.onResponseError(controller, err); return }
        // 重新 dispatch 到另一个 family 的地址
        this.#dispatch(dispatchOpts, this)
        return
      }
      break
    case 'ENOTFOUND':
      this.#state.deleteRecords(this.#origin)  // 清除过期缓存
      break
  }
}
```

#### 3.4.3 IP 轮询算法

`#defaultPick()`（第 267 行）实现了加权轮询：

```javascript
#defaultPick (origin, hostnameRecords, affinity) {
  let ip = null
  const { records, offset } = hostnameRecords

  let family
  if (this.dualStack) {
    if (affinity == null) {
      // 交替选择：根据 offset 奇偶性
      if (offset == null || offset === maxInt) {
        hostnameRecords.offset = 0
        affinity = 4
      } else {
        hostnameRecords.offset++
        affinity = (hostnameRecords.offset & 1) === 1 ? 6 : 4
      }
    }
    if (records[affinity] != null && records[affinity].ips.length > 0) {
      family = records[affinity]
    } else {
      family = records[affinity === 4 ? 6 : 4]  // fallback
    }
  } else {
    family = records[affinity]
  }

  if (family == null || family.ips.length === 0) return ip

  if (family.offset == null || family.offset === maxInt) {
    family.offset = 0
  } else {
    family.offset++
  }

  const position = family.offset % family.ips.length
  ip = family.ips[position] ?? null

  if (ip == null) return ip

  // TTL 过期自动移除（毫秒级）
  if (Date.now() - ip.timestamp > ip.ttl) {
    family.ips.splice(position, 1)
    return this.pick(origin, hostnameRecords, affinity)  // 递归
  }

  return ip
}
```

#### 3.4.4 DNSStorage 缓存层

```javascript
class DNSStorage {
  #maxItems = 0
  #records = new Map()

  constructor (opts) {
    this.#maxItems = opts.maxItems
  }

  get size () { return this.#records.size }

  get (hostname) { return this.#records.get(hostname) ?? null }

  set (hostname, records) { this.#records.set(hostname, records) }

  delete (hostname) { this.#records.delete(hostname) }

  // 满了之后不再接受新查询（回退到原始 origin）
  full () { return this.size >= this.#maxItems }
}
```

#### 3.4.5 完整入口函数

```javascript
module.exports = interceptorOpts => {
  // 参数校验
  if (interceptorOpts?.maxTTL != null && (typeof interceptorOpts?.maxTTL !== 'number' || interceptorOpts?.maxTTL < 0)) {
    throw new InvalidArgumentError('Invalid maxTTL. Must be a positive number')
  }
  if (interceptorOpts?.maxItems != null && (typeof interceptorOpts?.maxItems !== 'number' || interceptorOpts?.maxItems < 1)) {
    throw new InvalidArgumentError('Invalid maxItems. Must be a positive number and greater than zero')
  }
  if (interceptorOpts?.affinity != null && interceptorOpts?.affinity !== 4 && interceptorOpts?.affinity !== 6) {
    throw new InvalidArgumentError('Invalid affinity. Must be either 4 or 6')
  }
  // ... 更多校验

  const dualStack = interceptorOpts?.dualStack ?? true
  let affinity
  if (dualStack) {
    affinity = interceptorOpts?.affinity ?? null
  } else {
    affinity = interceptorOpts?.affinity ?? 4
  }

  const opts = {
    maxTTL: interceptorOpts?.maxTTL ?? 10e3,  // 默认 10 秒
    lookup: interceptorOpts?.lookup ?? null,
    pick: interceptorOpts?.pick ?? null,
    dualStack,
    affinity,
    maxItems: interceptorOpts?.maxItems ?? Infinity,
    storage: interceptorOpts?.storage
  }

  const instance = new DNSInstance(opts)

  return dispatch => {
    return function dnsInterceptor (origDispatchOpts, handler) {
      if (origDispatchOpts.origin == null) {
        return dispatch(origDispatchOpts, handler)
      }

      const origin = origDispatchOpts.origin.constructor === URL
        ? origDispatchOpts.origin
        : new URL(origDispatchOpts.origin)

      // 已经是 IP 地址，跳过
      if (isIP(origin.hostname) !== 0) {
        return dispatch(origDispatchOpts, handler)
      }

      // 异步 DNS 查询
      instance.runLookup(origin, origDispatchOpts, (err, newOrigin) => {
        if (err) return handler.onResponseError(null, err)

        const dispatchOpts = {
          ...origDispatchOpts,
          servername: origin.hostname,  // 用于 SNI（TLS 握手）
          origin: newOrigin.origin,
          headers: withHostHeader(origin.host, origDispatchOpts.headers)  // 自动加 Host
        }

        dispatch(
          dispatchOpts,
          instance.getHandler({ origin, dispatch, handler, newOrigin }, origDispatchOpts)
        )
      })

      return true
    }
  }
}
```

#### 3.4.6 自定义 lookup/pick/storage

```javascript
// 自定义 DNS lookup（例如使用 doh）
const dnsInterceptor = dns({
  lookup: (hostname, options, callback) => {
    // 自定义查询逻辑
    dohLookup(hostname).then(addrs => callback(null, addrs)).catch(callback)
  },
  // 自定义 IP 选择
  pick: (origin, records, affinity) => {
    // 总是选择延迟最低的
    return records[affinity].ips.sort((a, b) => a.latency - b.latency)[0]
  },
  // 自定义存储（例如使用 Redis）
  storage: {
    get: (hostname) => redis.get(`dns:${hostname}`),
    set: (hostname, records) => redis.set(`dns:${hostname}`, records, 'EX', 60),
    full: () => false,
    delete: (hostname) => redis.del(`dns:${hostname}`)
  }
})
```

### 3.5 Decompress 拦截器

**文件**：`lib/interceptor/decompress.js`（292 行）

自动解压缩拦截器，支持 7 种编码：

```javascript
// lib/interceptor/decompress.js 第 12 行
const supportedEncodings = {
  gzip: createGunzip,
  'x-gzip': createGunzip,
  br: createBrotliDecompress,
  deflate: createInflate,
  compress: createInflate,
  'x-compress': createInflate,
  zstd: createZstdDecompress
}
```

#### 3.5.1 多级解压链

`#createDecompressionChain()`（第 68 行）处理 `Content-Encoding: gzip, br` 这种多级编码：

```javascript
#createDecompressionChain (encodings) {
  const parts = encodings.split(',')
  // CVE 修复：限制最多 5 层编码
  if (parts.length > maxContentEncodings) {
    throw new Error(`too many content-encodings in response: ${parts.length}`)
  }
  // 逆序创建解压流（最后编码先解压）
  for (let i = parts.length - 1; i >= 0; i--) {
    decompressors.push(supportedEncodings[encoding]())
  }
  return decompressors
}
```

#### 3.5.2 单解压器 vs 多解压器

- 单解压器：直接监听 `readable` 和 `end` 事件
- 多解压器：使用 `pipeline()` 串联，仅最后一个解压器监听输出

#### 3.5.3 安全措施

- CVE 修复：限制 Content-Encoding 层数（最多 5 层）
- 跳过 204/304 等无 body 状态码
- 跳过 HEAD 请求
- 跳过错误响应（status >= 400）
- 实验性功能发出 `ExperimentalWarning`

#### 3.5.4 DecompressHandler 完整实现

```javascript
class DecompressHandler extends DecoratorHandler {
  #decompressors = []
  #trailers
  #skipStatusCodes
  #skipErrorResponses

  constructor (handler, { skipStatusCodes = defaultSkipStatusCodes, skipErrorResponses = true } = {}) {
    super(handler)
    this.#skipStatusCodes = skipStatusCodes
    this.#skipErrorResponses = skipErrorResponses
  }

  #shouldSkipDecompression (contentEncoding, statusCode) {
    if (!contentEncoding || statusCode < 200) return true
    if (this.#skipStatusCodes.includes(statusCode)) return true
    if (this.#skipErrorResponses && statusCode >= 400) return true
    return false
  }

  onResponseStart (controller, statusCode, headers, statusMessage) {
    const contentEncoding = headers['content-encoding']

    if (this.#shouldSkipDecompression(contentEncoding, statusCode)) {
      return super.onResponseStart(controller, statusCode, headers, statusMessage)
    }

    const decompressors = this.#createDecompressionChain(contentEncoding.toLowerCase())
    if (decompressors.length === 0) {
      this.#cleanupDecompressors()
      return super.onResponseStart(controller, statusCode, headers, statusMessage)
    }

    this.#decompressors = decompressors

    // 移除压缩相关 header
    const { 'content-encoding': _, 'content-length': __, ...newHeaders } = headers

    // 同步更新 rawHeaders
    if (controller?.rawHeaders) {
      const rawHeaders = controller.rawHeaders
      if (Array.isArray(rawHeaders)) {
        const filteredHeaders = []
        for (let i = 0; i < rawHeaders.length; i += 2) {
          const headerName = rawHeaders[i]
          const name = Buffer.isBuffer(headerName) ? headerName.toString('latin1') : `${headerName}`
          const lowerName = name.toLowerCase()
          if (lowerName === 'content-encoding' || lowerName === 'content-length') continue
          filteredHeaders.push(rawHeaders[i], rawHeaders[i + 1])
        }
        controller.rawHeaders = filteredHeaders
      }
    }

    if (this.#decompressors.length === 1) {
      this.#setupSingleDecompressor(controller)
    } else {
      this.#setupMultipleDecompressors(controller)
    }

    return super.onResponseStart(controller, statusCode, newHeaders, statusMessage)
  }

  onResponseData (controller, chunk) {
    if (this.#decompressors.length > 0) {
      this.#decompressors[0].write(chunk)  // 写入第一个解压器
      return
    }
    super.onResponseData(controller, chunk)
  }

  onResponseEnd (controller, trailers) {
    if (this.#decompressors.length > 0) {
      this.#trailers = trailers
      this.#decompressors[0].end()  // 结束第一个解压器
      this.#cleanupDecompressors()
      return
    }
    super.onResponseEnd(controller, trailers)
  }

  onResponseError (controller, err) {
    if (this.#decompressors.length > 0) {
      for (const decompressor of this.#decompressors) {
        decompressor.destroy(err)  // 销毁所有解压器
      }
      this.#cleanupDecompressors()
    }
    super.onResponseError(controller, err)
  }
}
```

#### 3.5.5 单解压器 vs 多解压器

```
单解压器 (gzip):
  chunk → gunzip → onResponseData

多解压器 (gzip, br):
  chunk → gunzip → brotliDecompress → onResponseData
         └─ pipeline() 串联 ─┘
```

```javascript
#setupSingleDecompressor (controller) {
  const decompressor = this.#decompressors[0]
  this.#setupDecompressorEvents(decompressor, controller)
  decompressor.on('end', () => {
    super.onResponseEnd(controller, this.#trailers)
  })
}

#setupMultipleDecompressors (controller) {
  const lastDecompressor = this.#decompressors[this.#decompressors.length - 1]
  this.#setupDecompressorEvents(lastDecompressor, controller)
  pipeline(this.#decompressors, (err) => {
    if (err) super.onResponseError(controller, err)
    else super.onResponseEnd(controller, this.#trailers)
  })
}
```

#### 3.5.6 入口函数

```javascript
function createDecompressInterceptor (options = {}) {
  // 只发出一次实验性警告
  if (!warningEmitted) {
    process.emitWarning(
      'DecompressInterceptor is experimental and subject to change',
      'ExperimentalWarning'
    )
    warningEmitted = true
  }

  return (dispatch) => {
    return (opts, handler) => {
      if (opts.method === 'HEAD') {
        return dispatch(opts, handler)  // HEAD 请求跳过
      }
      const decompressHandler = new DecompressHandler(handler, options)
      return dispatch(opts, decompressHandler)
    }
  }
}
```

### 3.6 Deduplicate 拦截器

**文件**：`lib/interceptor/deduplicate.js`（117 行）

请求去重拦截器，确保相同请求不会重复发送：

```javascript
// lib/interceptor/deduplicate.js 第 58 行
const pendingRequests = new Map()  // dedupeKey → DeduplicationHandler

return dispatch => {
  return (opts, handler) => {
    const cacheKey = makeCacheKey(opts)
    const dedupeKey = makeDeduplicationKey(cacheKey, excludeHeaderNamesSet)

    const pendingHandler = pendingRequests.get(dedupeKey)
    if (pendingHandler) {
      // 已有相同请求在途，加入等待队列
      if (pendingHandler.addWaitingHandler(handler)) {
        return true  // 共享响应
      }
      return dispatch(opts, handler)  // body 已开始流式传输，独立发送
    }

    // 创建新的去重 handler
    const deduplicationHandler = new DeduplicationHandler(handler, () => {
      pendingRequests.delete(dedupeKey)  // 完成后清理
    }, maxBufferSize)
    pendingRequests.set(dedupeKey, deduplicationHandler)
    return dispatch(opts, deduplicationHandler)
  }
}
```

**去重键生成**（`lib/util/cache.js` 第 681 行）：

```javascript
function makeDeduplicationKey (cacheKey, excludeHeaders) {
  // 使用 JSON.stringify 避免碰撞
  // 排除指定 header
  return JSON.stringify([cacheKey.origin, cacheKey.method, cacheKey.path, headers])
}
```

**diagnostics_channel 集成**：通过 `undici:request:pending-requests` 通道发布去重状态变化。

### 3.7 Dump 拦截器

**文件**：`lib/interceptor/dump.js`（112 行）

大小限制 + body 丢弃拦截器：

```javascript
class DumpHandler extends DecoratorHandler {
  #maxSize = 1024 * 1024  // 默认 1MB
  #dumped = false
  #size = 0

  onResponseStart (controller, statusCode, headers, statusMessage) {
    // Content-Length 预检
    if (contentLength != null && contentLength > this.#maxSize) {
      throw new RequestAbortedError(`Response size (${contentLength}) larger than maxSize`)
    }
  }

  onResponseData (controller, chunk) {
    this.#size += chunk.length
    if (this.#size >= this.#maxSize) {
      this.#dumped = true
      // 直接结束或报错
      super.onResponseEnd(controller, {})
    }
    return true  // 不传递数据给下游
  }
}
```

**abort 支持**：通过覆盖 `controller.abort` 实现取消时的优雅处理。

### 3.8 Response-Error 拦截器

**文件**：`lib/interceptor/response-error.js`（95 行）

将 HTTP 4xx/5xx 错误转为 Error 对象：

```javascript
class ResponseErrorHandler extends DecoratorHandler {
  onResponseStart (controller, statusCode, headers, statusMessage) {
    this.#statusCode = statusCode
    if (this.#statusCode < 400) {
      return super.onResponseStart(...)  // 正常响应，透传
    }
    // 错误响应：收集 body 并解码
    if (this.#checkContentType('application/json') || this.#checkContentType('text/plain')) {
      this.#decoder = new TextDecoder('utf-8')
    }
  }

  onResponseEnd (controller, trailers) {
    if (this.#statusCode >= 400) {
      // JSON 解析
      if (this.#checkContentType('application/json')) {
        this.#body = JSON.parse(this.#body)
      }
      // 创建 ResponseError（避免栈追踪开销）
      Error.stackTraceLimit = 0
      const err = new ResponseError('Response Error', this.#statusCode, {
        body: this.#body, headers: this.#headers
      })
      super.onResponseError(controller, err)
    }
  }
}
```

**性能优化**：创建错误时临时设置 `Error.stackTraceLimit = 0` 避免栈追踪开销。

---

## 4. Handler 装饰器模式

### 4.1 DecoratorHandler 基类

**文件**：`lib/handler/decorator-handler.js`（73 行）

所有 Handler 的基础装饰器，提供生命周期状态机保护。已标记 `@deprecated`，推荐新代码直接实现 `DispatchHandler` 接口。

#### 4.1.1 完整源码

```javascript
module.exports = class DecoratorHandler {
  #handler
  #onCompleteCalled = false
  #onErrorCalled = false
  #onResponseStartCalled = false

  constructor (handler) {
    if (typeof handler !== 'object' || handler === null) {
      throw new TypeError('handler must be an object')
    }
    this.#handler = handler
  }

  onRequestStart (...args) {
    return this.#handler.onRequestStart?.(...args)
  }

  onRequestUpgrade (...args) {
    assert(!this.#onCompleteCalled)
    assert(!this.#onErrorCalled)
    return this.#handler.onRequestUpgrade?.(...args)
  }

  onResponseStart (...args) {
    assert(!this.#onCompleteCalled)     // 防止完成后调用
    assert(!this.#onErrorCalled)        // 防止错误后调用
    assert(!this.#onResponseStartCalled) // 防止重复调用
    this.#onResponseStartCalled = true
    return this.#handler.onResponseStart?.(...args)
  }

  onResponseData (...args) {
    assert(!this.#onCompleteCalled)
    assert(!this.#onErrorCalled)
    return this.#handler.onResponseData?.(...args)
  }

  onResponseEnd (...args) {
    assert(!this.#onCompleteCalled)
    assert(!this.#onErrorCalled)
    this.#onCompleteCalled = true
    return this.#handler.onResponseEnd?.(...args)
  }

  onResponseError (...args) {
    this.#onErrorCalled = true
    return this.#handler.onResponseError?.(...args)
  }

  // @deprecated - 直接透传
  onBodySent (...args) {
    return this.#handler.onBodySent?.(...args)
  }

  onRequestSent (...args) {
    return this.#handler.onRequestSent?.(...args)
  }
}
```

#### 4.1.2 Handler 生命周期状态机

```
                    ┌──────────────────────────────┐
                    │        onRequestStart         │ ← 无状态校验
                    └──────────────┬───────────────┘
                                   │
                    ┌──────────────▼───────────────┐
                    │       onRequestUpgrade        │ ← assert(!onComplete, !onError)
                    └──────────────┬───────────────┘
                                   │
                    ┌──────────────▼───────────────┐
                    │   onBodySent / onRequestSent  │ ← @deprecated 直接透传
                    └──────────────┬───────────────┘
                                   │
                    ┌──────────────▼───────────────┐
                    │       onResponseStart         │ ← assert(!onComplete, !onError, !onResponseStartCalled)
                    └──────────────┬───────────────┘  onResponseStartCalled = true
                                   │
                    ┌──────────────▼───────────────┐
                    │       onResponseData          │ ← assert(!onComplete, !onError)
                    └──────────────┬───────────────┘
                                   │
                 ┌─────────────────┴─────────────────┐
                 │                                   │
      ┌──────────▼──────────┐            ┌───────────▼──────────┐
      │    onResponseEnd     │            │   onResponseError    │
      │  onCompleteCalled=true│            │   onErrorCalled=true │
      └─────────────────────┘            └──────────────────────┘
```

**设计要点**：
- 用 `assert` 做运行时状态机校验（开发阶段捕获协议违规）
- 可选链 `?.()` 保护（handler 不一定实现所有回调）
- 状态单向流动：responseStart → responseData → responseEnd/error（不可逆）
- `onResponseStart` 通过 `onResponseStartCalled` 防止同一 handler 被重复触发

#### 4.1.3 子类继承模式

所有 Handler 子类通过 `extends DecoratorHandler` 继承，在覆盖方法中选择性地调用 `super.xxx()` 保持装饰链。例如：

```javascript
class DumpHandler extends DecoratorHandler {
  onResponseData (controller, chunk) {
    this.#size += chunk.length
    if (this.#size >= this.#maxSize) {
      this.#dumped = true
      super.onResponseEnd(controller, {})  // 提前终止
    }
    return true  // 不传递数据给下游
  }
}
```

**关键约定**：子类返回 `true` 表示拦截数据（不再调用 super）；返回 `super.xxx()` 表示透传给下游。

### 4.2 CacheHandler

**文件**：`lib/handler/cache-handler.js`（802 行）

这是最复杂的 Handler，负责缓存写入和 304 重验证处理。

#### 4.2.1 核心职责

1. **响应缓存写入**：`onResponseStart` 中判断是否可缓存，创建 `writeStream`
2. **数据双写**：`onResponseData` 中同时写入缓存 store 和下游 handler
3. **304 处理**：重用缓存 body，更新元数据
4. **不安全方法缓存失效**：POST/PUT/DELETE 成功时删除相关缓存

#### 4.2.2 可缓存性判断

`canCacheResponse()`（第 500 行）实现了 RFC 9111 的完整规则：

```javascript
function canCacheResponse (cacheType, statusCode, resHeaders, cacheControlDirectives, reqHeaders) {
  // 状态码必须 >= 200 且已理解
  if (statusCode < 200 || NOT_UNDERSTOOD_STATUS_CODES.includes(statusCode)) return false
  // 必须有缓存指示或可启发式缓存的状态码
  if (!HEURISTICALLY_CACHEABLE_STATUS_CODES.includes(statusCode) && !expires && !cacheControl) return false
  // no-store 指令
  if (cacheControlDirectives['no-store']) return false
  // shared 缓存不缓存 private 响应
  if (cacheType === 'shared' && cacheControlDirectives.private === true) return false
  // Vary: * 不缓存
  if (resHeaders.vary && hasVaryStar(resHeaders.vary)) return false
  // Authorization 请求头的特殊处理
  if (reqHeaders?.authorization && !cacheControlDirectives.public && !s-maxage && !must-revalidate)
    return false
}
```

#### 4.2.3 新鲜度计算

`determineStaleAt()`（第 621 行）按优先级计算：

```
shared 缓存: s-maxage > max-age > Expires > 启发式(last-modified * 0.1) > immutable > no-cache+validator
private 缓存: max-age > Expires > 启发式 > immutable > no-cache+validator
```

#### 4.2.4 删除时间计算

`determineDeleteAt()`（第 720 行）考虑：

```javascript
deleteAt = max(
  staleAt,
  staleAt + stale-while-revalidate * 1000,
  staleAt + stale-if-error * 1000,
  cachedAt + 31536000000  // immutable: 1 year
)
// 无 stale 指令时，额外加一个 freshness lifetime 作为 revalidation 缓冲
```

#### 4.2.5 304 响应处理

`onResponseStart` 中检测到 `statusCode === 304` 时：

```javascript
// 304 表示缓存验证成功
if (statusCode === 304) {
  // 重用缓存中的响应体
  const cachedBody = this.#readStream
  // 更新响应头（新的 Date、Age 等）
  for (const [key, value] of Object.entries(headers)) {
    this.#storedResponse.headers[key] = value
  }
  // 计算新的新鲜度
  const freshnessLifetime = determineFreshnessLifetime(...)
  this.#storedResponse.staleAt = cachedAt + freshnessLifetime * 1000
}
```

#### 4.2.6 不安全方法缓存失效

当 `POST`/`PUT`/`DELETE` 等不安全方法成功时，会删除同一 URL 的缓存：

```javascript
function onResponseEnd () {
  if (isUnsafeMethod(this.#opts.method)) {
    // 删除同一 URL 的所有缓存
    this.#store.delete(this.#cacheKey)
    // 同时删除 Vary 匹配的变体
    if (this.#varyEntries) {
      for (const varyKey of this.#varyEntries) {
        this.#store.delete(varyKey)
      }
    }
  }
}
```

#### 4.2.7 流式双写机制

`onResponseData` 中的双写：

```javascript
onResponseData (controller, chunk) {
  if (this.#canCache) {
    // 写入缓存存储（异步）
    this.#writeStream.write(chunk)
  }
  // 同时转发给下游 handler
  super.onResponseData(controller, chunk)
}
```

**关键设计**：数据被"分叉"——一份写入缓存供后续命中使用，一份立即转发给用户。

#### 4.2.8 stripNecessaryHeaders

`stripNecessaryHeaders()`（第 765 行）在缓存存储前剥离敏感/临时性 header：

```javascript
// 移除的 headers：
// - connection、keep-alive 等连接相关
// - 任何 connection 指定的 hop-by-hop headers
// - content-length（因为解压后长度可能变化）
```

### 4.3 CacheRevalidationHandler

**文件**：`lib/handler/cache-revalidation-handler.js`（134 行）

处理条件请求（If-Modified-Since / If-None-Match）的 304 响应：

```javascript
class CacheRevalidationHandler {
  onResponseStart (controller, statusCode, headers, statusMessage) {
    // 304 = 验证成功，或 5xx + allowErrorStatusCodes（stale-if-error）
    this.#successful = statusCode === 304 ||
      (this.#allowErrorStatusCodes && statusCode >= 500 && statusCode <= 504)

    // 通知缓存拦截器验证结果
    this.#callback(this.#successful, this.#context, statusCode, headers)
    this.#callback = null

    if (this.#successful) {
      return true  // 拦截数据流（使用缓存的 body）
    }
    // 验证失败，透传新响应给 CacheHandler 处理
    this.#handler.onResponseStart?.(controller, statusCode, headers, statusMessage)
  }

  onResponseError (controller, err) {
    // 连接错误时，如果允许 stale-if-error，也返回缓存
    if (this.#callback && this.#allowErrorStatusCodes) {
      this.#successful = true
      this.#callback(true, this.#context)  // 标记成功 → 使用缓存
      return
    }
  }
}
```

**关键设计**：`#callback` 用于回调通知上层（cache 拦截器的 `handleResult`），实现了两层 Handler 之间的协调。

### 4.4 RedirectHandler

**文件**：`lib/handler/redirect-handler.js`（229 行）

处理 HTTP 重定向（301/302/303/307/308）：

#### 4.4.1 重定向策略

```javascript
onResponseStart (controller, statusCode, headers, statusMessage) {
  if (this.opts.throwOnMaxRedirect && this.history.length >= this.maxRedirections) {
    throw new Error('max redirects')
  }

  let removeContentHeaders = statusCode === 303

  // 301/302 + POST → GET (RFC 7231 §6.4.2)
  if ((statusCode === 301 || statusCode === 302) && this.opts.method === 'POST') {
    this.opts.method = 'GET'
    if (util.isStream(this.opts.body)) {
      util.destroy(this.opts.body.on('error', noop))
    }
    this.opts.body = null
    removeContentHeaders = true
  }

  // 303 → GET/HEAD (RFC 7231 §6.4.4)
  if (statusCode === 303 && this.opts.method !== 'HEAD') {
    this.opts.method = 'GET'
    if (util.isStream(this.opts.body)) {
      util.destroy(this.opts.body.on('error', noop))
    }
    this.opts.body = null
  }

  // 判断是否需要重定向
  this.location = this.history.length >= this.maxRedirections ||
                  util.isDisturbed(this.opts.body) ||
                  redirectableStatusCodes.indexOf(statusCode) === -1
    ? null
    : headers.location

  if (this.opts.origin) {
    this.history.push(new URL(this.opts.path, this.opts.origin))
  }

  if (!this.location) {
    this.handler.onResponseStart?.(controller, statusCode, headers, statusMessage)
    return
  }

  const { origin, pathname, search } = util.parseURL(
    new URL(this.location, this.opts.origin && new URL(this.opts.path, this.opts.origin))
  )
  const path = search ? `${pathname}${search}` : pathname

  // 重定向循环检测
  const redirectUrlString = `${origin}${path}`
  for (const historyUrl of this.history) {
    if (historyUrl.toString() === redirectUrlString) {
      throw new InvalidArgumentError(
        `Redirect loop detected. Cannot redirect to ${origin}. This typically happens when using a Client or Pool with cross-origin redirects. Use an Agent for cross-origin redirects.`
      )
    }
  }

  // Header 清理
  this.opts.headers = cleanRequestHeaders(this.opts.headers, removeContentHeaders, this.opts.origin !== origin, this.stripHeadersOnRedirect, this.stripHeadersOnCrossOriginRedirect)
  this.opts.path = path
  this.opts.origin = origin
  this.opts.query = null
}
```

#### 4.4.2 Header 清理

`cleanRequestHeaders()` 实现了 RFC 7231 的 header 清理规则：

```javascript
function shouldRemoveHeader (header, removeContent, unknownOrigin, stripHeaders, stripHeadersOnCrossOrigin) {
  const name = util.headerNameToString(header)
  if (name === 'host') return true  // 始终移除
  if (stripHeaders?.has(name) || (unknownOrigin && stripHeadersOnCrossOrigin?.has(name))) return true
  if (removeContent && name.startsWith('content-')) return true
  if (unknownOrigin) {
    return name === 'authorization' || name === 'cookie' || name === 'proxy-authorization'
  }
  return false
}

function cleanRequestHeaders (headers, removeContent, unknownOrigin, stripHeaders, stripHeadersOnCrossOrigin) {
  const ret = []
  if (Array.isArray(headers)) {
    for (let i = 0; i < headers.length; i += 2) {
      if (!shouldRemoveHeader(headers[i], removeContent, unknownOrigin, stripHeaders, stripHeadersOnCrossOrigin)) {
        ret.push(headers[i], headers[i + 1])
      }
    }
  } else if (headers && typeof headers === 'object') {
    const entries = util.hasSafeIterator(headers) ? headers : Object.entries(headers)
    for (const [key, value] of entries) {
      if (!shouldRemoveHeader(key, removeContent, unknownOrigin, stripHeaders, stripHeadersOnCrossOrigin)) {
        ret.push(key, value)
      }
    }
  }
  return ret
}
```

**清理规则**：
- 始终移除 `Host`
- 303 或 POST→GET：移除所有 `Content-*`
- 跨域：移除 `Authorization`、`Cookie`、`Proxy-Authorization`
- 可配置：`stripHeadersOnRedirect`、`stripHeadersOnCrossOriginRedirect`

#### 4.4.3 自引用

重定向时 handler 重新 dispatch 给自己：

```javascript
onResponseEnd (controller, trailers) {
  if (this.location) {
    this.dispatch(this.opts, this)  // 自引用：dispatch(opts, redirectHandler)
  }
}
```

#### 4.4.4 静态 buildDispatch 方法

```javascript
static buildDispatch (dispatcher, maxRedirections) {
  if (maxRedirections != null && (!Number.isInteger(maxRedirections) || maxRedirections < 0)) {
    throw new InvalidArgumentError('maxRedirections must be a positive number')
  }

  const dispatch = dispatcher.dispatch.bind(dispatcher)
  return (opts, originalHandler) => dispatch(opts, new RedirectHandler(dispatch, maxRedirections, opts, originalHandler))
}
```

### 4.5 RetryHandler

**文件**：`lib/handler/retry-handler.js`（548 行）

最复杂的 Handler 之一，实现了可恢复的请求重试。

#### 4.5.1 RetryController 代理

**文件位置**：`lib/handler/retry-handler.js` 第 48-69 行

```javascript
class RetryController {
  #onAbort

  constructor (onAbort) {
    this.#onAbort = onAbort
    this.target = null  // 指向当前活跃连接的 controller
  }

  pause () { this.target?.pause() }
  resume () { this.target?.resume() }
  abort (reason) {
    this.target?.abort(reason)
    this.#onAbort(reason)  // 通知 handler 取消 backoff
  }

  get paused () { return this.target?.paused ?? false }
  get aborted () { return this.target?.aborted ?? false }
  get reason () { return this.target?.reason ?? null }
  get rawHeaders () { return this.target?.rawHeaders ?? null }
  get rawTrailers () { return this.target?.rawTrailers ?? null }
}
```

**设计要点**：

```
下游 handler 持有 controllerProxy (稳定引用)
         │
         │ pause()/resume()/abort()
         ▼
    RetryController (代理)
         │
         │ target 指向
         ▼
  ┌──────────────────┐     ┌──────────────────┐     ┌──────────────────┐
  │ 连接 A (旧)      │ ──► │ 连接 B (新)      │ ──► │ 连接 C (新)      │
  │ dispatch #1      │     │ dispatch #2      │     │ dispatch #3      │
  └──────────────────┘     └──────────────────┘     └──────────────────┘
```

每次透明重试都是新的 dispatch（新的 controller），但下游 handler 通过 `controllerProxy` 始终操作当前活跃连接。**关键问题**：如果不使用代理，当下游调用 `resume()` 时会操作旧（已死）的 controller，导致新连接永久 stalled。

#### 4.5.2 构造函数与配置

```javascript
constructor (opts, { dispatch, handler }) {
  const { retryOptions, ...dispatchOpts } = opts
  const {
    retry: retryFn,
    maxRetries,
    maxTimeout,
    minTimeout,
    timeoutFactor,
    methods,
    errorCodes,
    retryAfter,
    statusCodes,
    throwOnError
  } = retryOptions ?? {}

  this.retryOpts = {
    throwOnError: throwOnError ?? true,
    retry: retryFn ?? RetryHandler[kRetryHandlerDefaultRetry],
    retryAfter: retryAfter ?? true,
    maxTimeout: maxTimeout ?? 30 * 1000,   // 30s
    minTimeout: minTimeout ?? 500,          // 500ms
    timeoutFactor: timeoutFactor ?? 2,
    maxRetries: maxRetries ?? 5,
    methods: methods ?? ['GET', 'HEAD', 'OPTIONS', 'PUT', 'DELETE', 'TRACE', 'QUERY'],
    statusCodes: statusCodes ?? [500, 502, 503, 504, 429],
    errorCodes: errorCodes ?? [
      'ECONNRESET', 'ECONNREFUSED', 'ENOTFOUND',
      'ENETDOWN', 'ENETUNREACH', 'EHOSTDOWN',
      'EHOSTUNREACH', 'EPIPE', 'UND_ERR_SOCKET'
    ]
  }

  this.retryCount = 0
  this.retryCountCheckpoint = 0    // 用于 Range 续传的检查点
  this.headersSent = false
  this.start = 0
  this.end = null
  this.etag = null
  this.controllerProxy = new RetryController(reason => this.#onAbort(reason))
  this.retryPending = false        // 等待 retry 策略决策中
  this.retryTimer = null           // backoff 定时器引用
  this.aborted = false             // abort 已传播到下游
}
```

#### 4.5.3 默认重试策略

`RetryHandler[kRetryHandlerDefaultRetry]`（第 222 行）是静态符号方法：

```javascript
static [kRetryHandlerDefaultRetry] (err, { state, opts }, cb) {
  const { statusCode, code, headers } = err
  const { method, retryOptions } = opts
  const { maxRetries, minTimeout, maxTimeout, timeoutFactor, statusCodes, errorCodes, methods, retryAfter } = retryOptions
  const { counter } = state

  // 1. 错误码过滤 - 非 UND_ERR_REQ_RETRY 且不在 errorCodes 列表则不重试
  if (code && code !== 'UND_ERR_REQ_RETRY' && !errorCodes.includes(code)) {
    cb(err)  // 直接回调错误
    return
  }

  // 2. 方法过滤 - 不在 methods 列表则不重试
  if (Array.isArray(methods) && !methods.includes(method)) {
    cb(err)
    return
  }

  // 3. 状态码过滤 - 不在 statusCodes 列表则不重试
  if (statusCode != null && Array.isArray(statusCodes) && !statusCodes.includes(statusCode)) {
    cb(err)
    return
  }

  // 4. 最大重试次数过滤
  if (counter > maxRetries) {
    cb(err)
    return
  }

  // 5. 计算退避时间
  let retryAfterHeader = retryAfter === false ? undefined : headers?.['retry-after']
  if (retryAfterHeader) {
    retryAfterHeader = Number(retryAfterHeader)
    retryAfterHeader = Number.isNaN(retryAfterHeader)
      ? calculateRetryAfterHeader(headers['retry-after'])  // 解析 HTTP Date
      : retryAfterHeader * 1e3  // Retry-After 单位为秒
  }

  const retryTimeout =
    retryAfterHeader === 0
      ? 0
      : retryAfterHeader > 0
        ? Math.min(retryAfterHeader, maxTimeout)   // 尊重服务器值
        : Math.min(minTimeout * timeoutFactor ** (counter - 1), maxTimeout)  // 指数退避

  // 6. 返回 setTimeout 以便 abort 可以取消
  return setTimeout(() => cb(null), retryTimeout)
}
```

**退避公式**：`minTimeout * timeoutFactor^(counter-1)` 上限 `maxTimeout`

| 重试次数 | 计算 | 实际值 |
|---------|------|--------|
| 1 | 500 * 2^0 | 500ms |
| 2 | 500 * 2^1 | 1s |
| 3 | 500 * 2^2 | 2s |
| 4 | 500 * 2^3 | 4s |
| 5 | 500 * 2^4 | 16s |
| ... | ... | 30s (cap) |

#### 4.5.4 Range 请求续传

当请求体已部分消费（`headersSent = true`），重试使用 Range 请求续传：

```javascript
retry () {
  if (this.start !== 0) {
    const headers = { range: `bytes=${this.start}-${this.end ?? ''}` }
    // 强 ETag 验证 - 防止服务器资源在重试过程中变更
    if (this.etag != null) {
      headers['if-match'] = this.etag
    }
    this.opts = { ...this.opts, headers: { ...this.opts.headers, ...headers } }
  }
  try {
    this.retryCountCheckpoint = this.retryCount
    this.dispatch(this.opts, this)  // 自引用 dispatch
  } catch (err) {
    this.handler.onResponseError?.(this.controllerProxy, err)
  }
}
```

**206 Partial Content 处理**：

```javascript
onResponseStart (controller, statusCode, headers, statusMessage) {
  if (this.headersSent) {
    // 部分消费后收到 2xx，验证 Content-Range
    if (statusCode !== 206 && (this.start > 0 || statusCode !== 200)) {
      throw new RequestRetryError('server does not support range header...')
    }
    const contentRange = parseRangeHeader(headers['content-range'])
    if (this.etag != null && this.etag !== headers.etag) {
      throw new RequestRetryError('ETag mismatch', statusCode, ...)
    }
    validatePartialResponseContentLength(headers, contentRange, statusCode, this.retryCount)
    return
  }
  // 第一次响应：记录 start、end、etag
  if (statusCode === 206) {
    const range = parseRangeHeader(headers['content-range'])
    this.start = range.start
    this.end = range.end
  }
  this.resume = true
  this.etag = headers.etag
  // 忽略弱 etag (W/"...")
  if (this.etag != null && this.etag[0] === 'W' && this.etag[1] === '/') {
    this.etag = null
  }
  this.headersSent = true
  this.handler.onResponseStart?.(this.controllerProxy, statusCode, headers, statusMessage)
}
```

#### 4.5.5 Abort 传播

```javascript
#onAbort (reason) {
  if (!this.retryPending) return  // 没有待决策的 retry，无需处理
  this.aborted = true
  this.retryPending = false
  clearTimeout(this.retryTimer)  // 取消 backoff 定时器
  this.retryTimer = null
  this.handler.onResponseError?.(this.controllerProxy, reason ?? new RequestAbortedError())
}
```

**关键流程**：

```
用户 abort()
    │
    ▼
controllerProxy.abort()
    ├─► target?.abort()       // 中止当前连接
    └─► #onAbort()
         ├─► clearTimeout(retryTimer)  // 取消 backoff 等待
         └─► handler.onResponseError()  // 传播错误到下游
```

#### 4.5.6 retry interceptor 与 RetryHandler 的关系

| 层次 | 文件 | 职责 |
|------|------|------|
| Interceptor | `lib/interceptor/retry.js` (19 行) | 入口包装，注入 `retryOptions` 到 opts |
| Handler | `lib/interceptor/retry-handler.js` (548 行) | 完整重试逻辑 |

**retry 拦截器源码**：

```javascript
module.exports = globalOpts => {
  return dispatch => {
    return function retryInterceptor (opts, handler) {
      return dispatch(
        opts,
        new RetryHandler(
          { ...opts, retryOptions: { ...globalOpts, ...opts.retryOptions } },
          { handler, dispatch }  // 传递 handler 和 dispatch 引用
        )
      )
    }
  }
}
```

**设计要点**：Interceptor 仅 19 行，纯委托。所有复杂逻辑都在 Handler 中，因为重试需要管理多个连接的状态（controller、retryCount、backoff 定时器），这些超出了 Interceptor 的简单替换 handler 的能力。

### 4.6 DeduplicationHandler

**文件**：`lib/handler/deduplication-handler.js`（466 行）

请求去重的 Handler 实现，管理多个等待中的 handler。

#### 4.6.1 WaitingHandler 结构

```javascript
const waitingHandler = {
  handler,               // 下游 handler
  controller,            // 独立 controller（每个 waiting handler 有自己的流控状态）
  bufferedChunks: [],    // 暂停时缓冲的数据
  bufferedBytes: 0,      // 缓冲字节数
  pendingTrailers: null, // 暂停时缓存的 trailers
  done: false            // 是否已完成
}
```

#### 4.6.2 流控机制

每个 waiting handler 有独立的 controller，支持独立的 pause/resume：

```javascript
controller = {
  resume: () => {
    if (state.aborted) return
    state.paused = false
    this.#flushWaitingHandler(waitingHandler)  // 刷出缓冲数据
    // 如果主响应已完成且缓冲区空，发送 trailers
    if (
      this.#completed &&
      waitingHandler.pendingTrailers &&
      waitingHandler.bufferedChunks.length === 0 &&
      !state.paused &&
      !state.aborted
    ) {
      waitingHandler.handler.onResponseEnd?.(waitingHandler.controller, waitingHandler.pendingTrailers)
      waitingHandler.pendingTrailers = null
      waitingHandler.done = true
    }
    this.#pruneDoneWaitingHandlers()
  },
  pause: () => {
    if (!state.aborted) state.paused = true
  },
  abort: (reason) => {
    if (state.aborted) return
    state.aborted = true
    waitingHandler.done = true
    waitingHandler.pendingTrailers = null
    waitingHandler.bufferedChunks = []
    waitingHandler.bufferedBytes = 0
    handler.onResponseError?.(waitingHandler.controller, state.reason ?? new RequestAbortedError())
  }
}
```

#### 4.6.3 安全上限

`#bufferWaitingChunk()` 中有 `maxBufferSize`（默认 5MB）保护：

```javascript
#bufferWaitingChunk (waitingHandler, chunk) {
  if (waitingHandler.done || waitingHandler.controller.aborted) {
    waitingHandler.done = true
    waitingHandler.bufferedChunks = []
    waitingHandler.bufferedBytes = 0
    return
  }
  const bufferedChunk = Buffer.from(chunk)
  waitingHandler.bufferedChunks.push(bufferedChunk)
  waitingHandler.bufferedBytes += bufferedChunk.length

  if (waitingHandler.bufferedBytes > this.#maxBufferSize) {
    const err = new RequestAbortedError(
      `Deduplicated waiting handler exceeded maxBufferSize (${this.#maxBufferSize} bytes) while paused`
    )
    this.#errorWaitingHandler(waitingHandler, err)
  }
}
```

#### 4.6.4 主/从 Handler 广播机制

```
主 Handler (primaryHandler) ─── 直接发送到第一个请求的 handler
     │
     ├── waitingHandler #1 (独立 controller + buffer)
     ├── waitingHandler #2 (独立 controller + buffer)
     └── waitingHandler #3 (独立 controller + buffer)
```

```javascript
// onResponseData 广播到所有 waiting handlers
onResponseData (controller, chunk) {
  if (this.#aborted || this.#completed) return
  this.#responseDataStarted = true
  this.#primaryHandler.onResponseData?.(controller, chunk)

  for (const waitingHandler of this.#waitingHandlers) {
    const { handler, controller: waitingController } = waitingHandler
    if (waitingHandler.done || waitingController.aborted) {
      waitingHandler.done = true
      continue
    }
    if (waitingController.paused) {
      this.#bufferWaitingChunk(waitingHandler, chunk)  // 暂停时缓冲
      continue
    }
    try {
      handler.onResponseData?.(waitingController, chunk)
    } catch {
      // 忽略 waiting handler 的错误
    }
    if (waitingController.aborted) {
      waitingHandler.done = true
      waitingHandler.bufferedChunks = []
      waitingHandler.bufferedBytes = 0
    }
  }
  this.#pruneDoneWaitingHandlers()  // 清理已完成的 handler
}
```

#### 4.6.5 diagnostics_channel 集成

```javascript
const pendingRequestsChannel = diagnosticsChannel.channel('undici:request:pending-requests')

// 新增 pending 请求时
if (pendingRequestsChannel.hasSubscribers) {
  pendingRequestsChannel.publish({ size: pendingRequests.size, key: dedupeKey, type: 'added' })
}

// 完成移除时
if (pendingRequestsChannel.hasSubscribers) {
  pendingRequestsChannel.publish({ size: pendingRequests.size, key: dedupeKey, type: 'removed' })
}
```

#### 4.6.6 配置选项

| 选项 | 默认值 | 说明 |
|------|--------|------|
| `methods` | `['GET']` | 可去重的方法（必须是安全方法） |
| `skipHeaderNames` | `[]` | 存在这些 header 时跳过去重 |
| `excludeHeaderNames` | `[]` | 生成去重键时排除的 header |
| `maxBufferSize` | `5 * 1024 * 1024` | 暂停时单个 waiting handler 的最大缓冲 |

**配置示例**：

```javascript
dispatch = deduplicate({
  methods: ['GET', 'HEAD'],
  excludeHeaderNames: ['authorization'],  // 不同 auth 不去重
  maxBufferSize: 10 * 1024 * 1024         // 10MB 缓冲
})(dispatch)
```

---

## 5. Mock 系统架构

### 5.1 MockAgent 总入口

**文件**：`lib/mock/mock-agent.js`（244 行）

MockAgent 是整个 Mock 系统的顶层入口，继承自 `Dispatcher`。

#### 5.1.1 核心设计

```javascript
class MockAgent extends Dispatcher {
  constructor (opts = {}) {
    super(opts)
    this[kIsMockActive] = true
    this[kNetConnect] = true           // 网络连接控制
    this[kMockAgentIsCallHistoryEnabled] = opts.enableCallHistory ?? false

    // 内部包含真实 Agent
    const agent = opts?.agent ? opts.agent : new Agent(opts)
    this[kAgent] = agent
    this[kClients] = agent[kClients]   // 共享客户端池
  }
}
```

#### 5.1.2 Origin 匹配工厂

```javascript
[kFactory] (origin) {
  // connections === 1 时用 MockClient，否则用 MockPool
  return this[kOptions]?.connections === 1
    ? new MockClient(origin, mockOptions)
    : new MockPool(origin, mockOptions)
}
```

#### 5.1.3 正则 Origin 匹配

`[kMockAgentGet]()`（第 192 行）支持正则和函数匹配：

```javascript
[kMockAgentGet] (origin) {
  // 先精确匹配
  const dispatcher = this[kClients].get(origin)
  if (dispatcher) return dispatcher

  // 正则/函数模糊匹配
  for (const [keyMatcher, nonExplicitDispatcher] of Array.from(this[kClients])) {
    if (typeof keyMatcher !== 'string' && matchValue(keyMatcher, origin)) {
      const dispatcher = this[kFactory](origin)
      this[kMockAgentSet](origin, dispatcher)
      dispatcher[kDispatches] = nonExplicitDispatcher[kDispatches]  // 共享拦截规则
      return dispatcher
    }
  }
}
```

#### 5.1.4 网络连接控制

```javascript
enableNetConnect (matcher) {
  // true = 全部允许
  // false = 全部禁止
  // [string|RegExp|Function] = 白名单
  if (typeof matcher === 'string' || typeof matcher === 'function' || matcher instanceof RegExp) {
    if (Array.isArray(this[kNetConnect])) {
      this[kNetConnect].push(matcher)  // 追加到白名单
    } else {
      this[kNetConnect] = [matcher]
    }
  } else if (typeof matcher === 'undefined') {
    this[kNetConnect] = true
  } else {
    throw new InvalidArgumentError('Unsupported matcher')
  }
}

disableNetConnect () {
  this[kNetConnect] = false
}
```

#### 5.1.5 dispatch 路由完整流程

`dispatch()`（第 72 行）是 Mock 系统的核心路由：

```javascript
dispatch (opts, handler) {
  opts.origin = normalizeOrigin(opts.origin)  // 1. 标准化 origin

  const mockDispatcher = this.get(opts.origin)  // 2. 获取 MockClient/MockPool

  this[kMockAgentAddCallHistoryLog](opts)  // 3. 记录调用历史

  const acceptNonStandardSearchParameters = this[kMockAgentAcceptsNonStandardSearchParameters]
  const dispatchOpts = { ...opts }

  // 4. 处理 allowH2=false（HTTP/1.1 only 模式）
  //    Agent 对 HTTP/1.1 使用独立 key，需要镜像 mock dispatches
  if (dispatchOpts.allowH2 === false) {
    const http1OnlyKey = `${dispatchOpts.origin}#http1-only`
    if (!this[kClients].has(http1OnlyKey)) {
      const http1OnlyDispatcher = this[kFactory](dispatchOpts.origin)
      http1OnlyDispatcher[kDispatches] = mockDispatcher[kDispatches]  // 共享拦截规则
      this[kMockAgentSet](http1OnlyKey, http1OnlyDispatcher)
    }
  }

  // 5. 非标准搜索参数处理
  if (acceptNonStandardSearchParameters && dispatchOpts.path) {
    const [path, searchParams] = dispatchOpts.path.split('?')
    const normalizedSearchParams = normalizeSearchParams(searchParams, acceptNonStandardSearchParameters)
    dispatchOpts.path = `${path}?${normalizedSearchParams}`
  }

  return this[kAgent].dispatch(dispatchOpts, handler)  // 6. 委托给真实 Agent
}
```

#### 5.1.6 Origin 标准化

`normalizeOrigin()` 确保 origin 为小写字符串：

```javascript
function normalizeOrigin (origin) {
  if (typeof origin !== 'string' && !(origin instanceof URL)) {
    return origin
  }
  if (origin instanceof URL) {
    return origin.origin
  }
  return origin.toLowerCase()
}
```

**注意**：URL 会被转为 `origin` 形式（仅 protocol+host+port），丢弃 path 和 query。

#### 5.1.7 未消费拦截器断言

```javascript
pendingInterceptors () {
  const mockAgentClients = this[kClients]
  return Array.from(mockAgentClients.entries())
    .flatMap(([origin, dispatcher]) =>
      dispatcher[kDispatches].map(dispatch => ({ ...dispatch, origin }))
    )
    .filter(({ pending }) => pending)  // 只返回未消费的
}

assertNoPendingInterceptors ({ pendingInterceptorsFormatter = new PendingInterceptorsFormatter() } = {}) {
  const pending = this.pendingInterceptors()
  if (pending.length === 0) return
  throw new UndiciError(
    pending.length === 1
      ? `1 interceptor is pending:\n\n${pendingInterceptorsFormatter.format(pending)}`.trim()
      : `${pending.length} interceptors are pending:\n\n${pendingInterceptorsFormatter.format(pending)}`.trim()
  )
}
```

`PendingInterceptorsFormatter`（43 行）生成 `console.table` 格式的报告：

```javascript
format (pendingInterceptors) {
  const withPrettyHeaders = pendingInterceptors.map(
    ({ method, path, data: { statusCode }, persist, times, timesInvoked, origin }) => ({
      Method: method,
      Origin: origin,
      Path: path,
      'Status code': statusCode,
      Persistent: persist ? PERSISTENT : NOT_PERSISTENT,
      Invocations: timesInvoked,
      Remaining: persist ? Infinity : times - timesInvoked
    })
  )
  this.logger.table(withPrettyHeaders)
  return this.transform.read().toString()
}
```

#### 5.1.8 MockAgent 完整配置

```javascript
const mockAgent = new MockAgent({
  agent: customAgent,        // 自定义真实 Agent（用于 record 模式）
  connections: 1,            // 1=MockClient, >1=MockPool
  enableCallHistory: true,   // 启用调用历史
  ignoreTrailingSlash: true, // 匹配时忽略尾部 /
  acceptNonStandardSearchParameters: true  // 接受非标准搜索参数
})
```

#### 5.1.9 调用历史 API

```javascript
// 启用/禁用调用历史
mockAgent.enableCallHistory()   // 链式调用
mockAgent.disableCallHistory()

// 获取调用历史
const history = mockAgent.getCallHistory()
history.firstCall()             // 第一次调用
history.lastCall()              // 最后一次调用
history.nthCall(3)              // 第 3 次调用（非零基索引）
history.filterCallsByMethod('GET')
history.filterCalls({ path: '/api', method: 'GET' }, { operator: 'AND' })

// 清理
mockAgent.clearCallHistory()
```

### 5.2 MockClient / MockPool

**文件**：`lib/mock/mock-client.js`（68 行）、`lib/mock/mock-pool.js`（68 行）

两者几乎完全相同，分别继承 `Client` 和 `Pool`。

#### 5.2.1 MockClient 完整实现

```javascript
class MockClient extends Client {
  constructor (origin, opts) {
    if (!opts || !opts.agent || typeof opts.agent.dispatch !== 'function') {
      throw new InvalidArgumentError('Argument opts.agent must implement Agent')
    }

    super(origin, opts)

    this[kMockAgent] = opts.agent                    // 反向引用 MockAgent
    this[kOrigin] = origin
    this[kIgnoreTrailingSlash] = opts.ignoreTrailingSlash ?? false
    this[kDispatches] = []                           // 拦截规则列表
    this[kConnected] = 1
    this[kOriginalDispatch] = this.dispatch           // 保存原始 dispatch
    this[kOriginalClose] = this.close.bind(this)     // 保存原始 close

    this.dispatch = buildMockDispatch.call(this)     // 替换为 mock dispatch
    this.close = this[kClose]                        // 替换 close
  }

  get [Symbols.kConnected] () {
    return this[kConnected]
  }

  intercept (opts) {
    return new MockInterceptor(
      opts && { ignoreTrailingSlash: this[kIgnoreTrailingSlash], ...opts },
      this[kDispatches]
    )
  }

  cleanMocks () {
    this[kDispatches] = []
  }

  async [kClose] () {
    await promisify(this[kOriginalClose])()
    this[kConnected] = 0
    this[kMockAgent][Symbols.kClients].delete(this[kOrigin])  // 清理引用
  }
}
```

**核心替换**：构造函数中将 `this.dispatch` 替换为 `buildMockDispatch()` 生成的 mock dispatch。这种"猴子补丁"模式是 undici Mock 系统的核心。

#### 5.2.2 dispatch 替换机制

```
构造时:
  this.dispatch ──────────────► Client.prototype.dispatch (原始)
  this[kOriginalDispatch] ────► 保存原始引用
  this.dispatch = buildMockDispatch() ──► 替换为新的 dispatch 函数

运行时:
  this.dispatch(opts, handler)
       │
       ▼
  buildMockDispatch 返回的函数
       │
       ├─ agent.isMockActive === false ──► originalDispatch(opts, handler)  // 真实请求
       │
       └─ agent.isMockActive === true ──► mockDispatch(opts, handler)
              │
              ├─ 匹配成功 ──► 返回 mock 响应
              │
              └─ MockNotMatchedError ──► checkNetConnect()
                    │
                    ├─ 允许 ──► originalDispatch(opts, handler)  // 回退真实请求
                    └─ 拒绝 ──► 抛出错误
```

#### 5.2.3 与真实 Agent 的关系

```
MockAgent
  ├── kAgent ──────────────► 真实 Agent
  │                           ├── kClients (Map<origin, dispatcher>)
  │                           │   ├── "http://api.example.com" ──► MockClient
  │                           │   └── "http://cdn.example.com" ──► MockPool
  │                           └── dispatch()
  │
  ├── kIsMockActive ───────► true/false 控制开关
  └── kNetConnect ─────────► 网络连接白名单/黑名单
```

### 5.3 MockInterceptor / MockScope

**文件**：`lib/mock/mock-interceptor.js`（227 行）

#### 5.3.1 MockInterceptor（拦截规则定义）

**文件**：`lib/mock/mock-interceptor.js`（227 行）

```javascript
class MockInterceptor {
  constructor (opts, mockDispatches) {
    if (typeof opts !== 'object') {
      throw new InvalidArgumentError('opts must be an object')
    }
    if (typeof opts.path === 'undefined') {
      throw new InvalidArgumentError('opts.path must be defined')
    }
    if (typeof opts.method === 'undefined') {
      opts.method = 'GET'
    }
    // path 处理：合并 query，解析 URL
    if (typeof opts.path === 'string') {
      if (opts.query) {
        opts.path = serializePathWithQuery(opts.path, opts.query)
      } else {
        const parsedURL = new URL(opts.path, 'data://')
        opts.path = parsedURL.pathname + parsedURL.search
      }
    }
    if (typeof opts.method === 'string') {
      opts.method = opts.method.toUpperCase()
    }

    this[kDispatchKey] = buildKey(opts)       // { path, method, body, headers, query }
    this[kDispatches] = mockDispatches
    this[kIgnoreTrailingSlash] = opts.ignoreTrailingSlash ?? false
    this[kDefaultHeaders] = {}
    this[kDefaultTrailers] = {}
    this[kContentLength] = false
  }

  createMockScopeDispatchData ({ statusCode, data, responseOptions }) {
    const responseData = getResponseData(data)
    const contentLength = this[kContentLength] ? { 'content-length': responseData.length } : {}
    const headers = { ...this[kDefaultHeaders], ...contentLength, ...responseOptions.headers }
    const trailers = { ...this[kDefaultTrailers], ...responseOptions.trailers }
    return { statusCode, data, headers, trailers }
  }

  // 支持三种调用形式：
  // .reply(statusCode)                      - 只有状态码
  // .reply(statusCode, data)               - 状态码 + body
  // .reply(statusCode, data, options)      - 完整配置
  // .reply(callback)                       - 回调函数（动态响应）
  reply (replyOptionsCallbackOrStatusCode) {
    if (typeof replyOptionsCallbackOrStatusCode === 'function') {
      // 回调式：支持异步回调
      const resolveReplyCallbackData = (resolvedData) => {
        if (typeof resolvedData !== 'object' || resolvedData === null) {
          throw new InvalidArgumentError('reply options callback must return an object')
        }
        const replyParameters = { data: '', responseOptions: {}, ...resolvedData }
        this.validateReplyParameters(replyParameters)
        return { ...this.createMockScopeDispatchData(replyParameters) }
      }
      const wrappedDefaultsCallback = (opts) => {
        const resolvedData = replyOptionsCallbackOrStatusCode(opts)
        if (isPromise(resolvedData)) {
          return resolvedData.then(resolveReplyCallbackData)  // 支持异步
        }
        return resolveReplyCallbackData(resolvedData)
      }
      const newMockDispatch = addMockDispatch(this[kDispatches], this[kDispatchKey], wrappedDefaultsCallback, { ignoreTrailingSlash: this[kIgnoreTrailingSlash] })
      return new MockScope(newMockDispatch)
    }
    // 静态值式
    const replyParameters = {
      statusCode: replyOptionsCallbackOrStatusCode,
      data: arguments[1] === undefined ? '' : arguments[1],
      responseOptions: arguments[2] === undefined ? {} : arguments[2]
    }
    this.validateReplyParameters(replyParameters)
    const dispatchData = this.createMockScopeDispatchData(replyParameters)
    const newMockDispatch = addMockDispatch(this[kDispatches], this[kDispatchKey], dispatchData, { ignoreTrailingSlash: this[kIgnoreTrailingSlash] })
    return new MockScope(newMockDispatch)
  }

  replyWithError (error) {
    const newMockDispatch = addMockDispatch(this[kDispatches], this[kDispatchKey], { error }, { ignoreTrailingSlash: this[kIgnoreTrailingSlash] })
    return new MockScope(newMockDispatch)
  }

  defaultReplyHeaders (headers) {
    this[kDefaultHeaders] = headers
    return this  // 链式
  }

  defaultReplyTrailers (trailers) {
    this[kDefaultTrailers] = trailers
    return this
  }

  replyContentLength () {
    this[kContentLength] = true
    return this
  }
}
```

#### 5.3.2 MockScope（回复行为配置）

```javascript
class MockScope {
  delay (waitInMs) {
    this[kMockDispatch].delay = waitInMs   // 延迟回复
    return this                             // 链式调用
  }
  persist () {
    this[kMockDispatch].persist = true     // 永不过期
    return this
  }
  times (repeatTimes) {
    this[kMockDispatch].times = repeatTimes // 使用 N 次后消费
    return this
  }
}
```

**链式 API 设计**：

```javascript
mockPool.intercept({ path: '/api/users', method: 'GET' })
  .defaultReplyHeaders({ 'x-powered-by': 'test' })
  .replyContentLength()
  .reply(200, { users: [] }, { headers: { 'content-type': 'application/json' } })
  .delay(100)   // 延迟 100ms
  .times(3)     // 前 3 次使用
  .persist()    // 永不过期
```

**高级用法**：

```javascript
// 异步回调
mockPool.intercept({ path: '/api/users' })
  .reply((opts) => {
    if (opts.headers?.authorization) {
      return { statusCode: 200, data: mockUsers }
    }
    return { statusCode: 401, data: { error: 'Unauthorized' } }
  })

// 错误模拟
mockPool.intercept({ path: '/api/fail' })
  .replyWithError(new Error('Connection refused'))

// 异步回调
mockPool.intercept({ path: '/api/async' })
  .reply(async (opts) => {
    const result = await fetchRealData(opts)
    return { statusCode: 200, data: result }
  })
```

### 5.4 MockUtils 核心引擎

**文件**：`lib/mock/mock-utils.js`（720 行）

这是 Mock 系统的核心引擎，包含匹配、dispatch、body 处理等。

#### 5.4.1 匹配函数

```javascript
// 值匹配：支持 string/RegExp/Function
function matchValue (match, value) {
  if (typeof match === 'string') return match === value
  if (match instanceof RegExp) return match.test(value)
  if (typeof match === 'function') return match(value) === true
  return false
}

// 完整请求匹配
function matchKey (mockDispatch, { path, method, body, headers }) {
  return matchValue(mockDispatch.path, path)
    && matchValue(mockDispatch.method, method)
    && (typeof mockDispatch.body !== 'undefined' ? matchValue(mockDispatch.body, body) : true)
    && matchHeaders(mockDispatch, headers)
}
```

#### 5.4.2 Header 匹配

```javascript
function matchHeaders (mockDispatch, headers) {
  // 函数式匹配
  if (typeof mockDispatch.headers === 'function') {
    return mockDispatch.headers(lowerCaseEntries(headers))
  }
  // 未定义 = 匹配所有
  if (mockDispatch.headers === undefined) return true
  // 对象式匹配：每个 mock header 都必须匹配
  for (const [name, value] of Object.entries(mockDispatch.headers)) {
    if (!matchValue(value, getHeaderByName(headers, name))) return false
  }
  return true
}
```

#### 5.4.3 Mock Dispatch 查找

`getMockDispatch()`（第 171 行）实现多级过滤：

```javascript
function getMockDispatch (mockDispatches, key) {
  // 1. 过滤已消费的
  let matched = mockDispatches.filter(({ consumed }) => !consumed)
  // 2. 匹配 path（支持 ignoreTrailingSlash）
  matched = matched.filter(({ path }) => matchValue(path, resolvedPath))
  // 3. 匹配 method
  matched = matched.filter(({ method }) => matchValue(method, key.method))
  // 4. 匹配 body
  matched = matched.filter(({ body }) => matchValue(body, key.body))
  // 5. 匹配 headers
  matched = matched.filter((mock) => matchHeaders(mock, key.headers))
  return matched[0]  // 取第一个匹配
}
```

**错误报告**：每级过滤失败都抛出 `MockNotMatchedError`，包含精确的失败原因。

#### 5.4.4 mockDispatch 主函数

`mockDispatch()`（第 303 行）是 mock 系统的核心 dispatch 函数：

```javascript
function mockDispatch (opts, handler) {
  const key = buildKey(opts)
  const mockDispatch = getMockDispatch(this[kDispatches], key)
  const mockDispatches = this[kDispatches]

  mockDispatch.timesInvoked++

  const { timesInvoked, times } = mockDispatch

  // 标记消费状态
  mockDispatch.consumed = !mockDispatch.persist && timesInvoked >= times
  mockDispatch.pending = timesInvoked < times

  const hasBodyHooks = typeof handler.onBodySent === 'function' ||
    typeof handler.onRequestSent === 'function'

  // 回调式回复（且无 body hooks 或 body 为空时立即调用）
  if (mockDispatch.data.callback && (!hasBodyHooks || opts.body == null)) {
    const { callback, ...responseDefaults } = mockDispatch.data
    const callbackResult = callback(opts)

    if (isPromise(callbackResult)) {
      callbackResult.then(
        (resolvedData) => {
          if (resolvedData == null || typeof resolvedData !== 'object') {
            handler.onResponseError(null, new InvalidArgumentError('reply options callback must return an object'))
            return
          }
          dispatchMockReply(mockDispatches, mockDispatch, key, opts, handler, { ...responseDefaults, ...resolvedData })
        },
        (error) => handler.onResponseError(null, error)
      )
      return true
    }

    if (callbackResult == null || typeof callbackResult !== 'object') {
      throw new InvalidArgumentError('reply options callback must return an object')
    }

    return dispatchMockReply(mockDispatches, mockDispatch, key, opts, handler, { ...responseDefaults, ...callbackResult })
  }

  return dispatchMockReply(mockDispatches, mockDispatch, key, opts, handler)
}
```

#### 5.4.5 响应模拟

`dispatchMockReply()`（第 358 行）模拟完整的 HTTP 响应生命周期：

```javascript
function dispatchMockReply (mockDispatches, mockDispatch, key, opts, handler, resolvedResponse) {
  const { data: responseData, delay } = mockDispatch
  const response = resolvedResponse ?? responseData

  // 1. 错误模拟
  if (response.error !== null) {
    deleteMockDispatch(mockDispatches, key)
    handler.onResponseError(null, response.error)
    return true
  }

  let aborted = false
  let timer = null

  // 2. 创建 controller（提前创建以便 abort 使用）
  const controller = {
    paused: false,
    rawHeaders: null,
    rawTrailers: null,
    pause () { this.paused = true },
    resume () { this.paused = false },
    abort: (reason) => {
      if (aborted) return
      aborted = true
      if (timer !== null) {
        clearTimeout(timer)
        timer = null
      }
      handler.onResponseError?.(controller, reason)
    }
  }

  let replyOpts = opts
  handler.onRequestStart?.(controller, null)

  if (aborted) return true

  // 3. 处理请求体（触发 body hooks）
  const requestBody = dispatchRequestBody(opts.body, handler, controller, () => aborted)

  if (isPromise(requestBody)) {
    requestBody.then((body) => {
      if (body === requestAborted) return
      if (body !== opts.body) replyOpts = { ...opts, body }
      sendReply()
    }, (error) => controller.abort(error))
    return true
  }

  if (requestBody === requestAborted) return true
  if (requestBody !== opts.body) replyOpts = { ...opts, body: requestBody }

  sendReply()

  function sendReply () {
    if (response.callback) {
      const { callback, ...responseDefaults } = response
      let callbackResult
      try {
        callbackResult = callback(replyOpts)
      } catch (err) {
        deleteMockDispatch(mockDispatches, key)
        handler.onResponseError(null, err)
        return
      }

      if (isPromise(callbackResult)) {
        callbackResult.then(
          (resolvedData) => handleReply(dispatches, { ...responseDefaults, ...resolvedData }),
          (err) => handler.onResponseError(null, err)
        )
        return
      }

      handleReply(dispatches, { ...responseDefaults, ...callbackResult })
      return
    }

    // 延迟回复
    if (typeof delay === 'number' && delay > 0) {
      timer = setTimeout(() => {
        timer = null
        handleReply(dispatches)
      }, delay)
    } else {
      handleReply(dispatches)
    }
  }

  function handleReply (mockDispatches, _response = response) {
    if (aborted) return

    const { statusCode, data, headers, trailers } = _response

    // 支持 data 为函数（动态生成 body）
    const optsHeaders = Array.isArray(opts.headers) ? buildHeadersFromArray(opts.headers) : opts.headers
    const body = typeof data === 'function' ? data({ ...replyOpts, headers: optsHeaders }) : data

    if (isPromise(body)) {
      return body.then((newData) => handleReply(mockDispatches, { ..._response, data: newData }))
    }

    if (aborted) return

    const responseData = getResponseData(body)
    const responseHeaders = generateKeyValues(headers ?? {})
    const responseTrailers = generateKeyValues(trailers ?? {})

    controller.rawHeaders = responseHeaders
    controller.rawTrailers = responseTrailers

    // 模拟 HTTP 响应生命周期
    handler.onResponseStart?.(controller, statusCode, parseHeaders(responseHeaders), getStatusText(statusCode))
    handler.onResponseData?.(controller, Buffer.from(responseData))
    handler.onResponseEnd?.(controller, parseHeaders(responseTrailers))
    deleteMockDispatch(mockDispatches, key)
  }

  return true
}
```

**完整生命周期时序**：

```
Mock 响应时序:
┌─────────────────────────────────────────────────────────┐
│ mockDispatch()                                          │
│   │                                                     │
│   ├─ getMockDispatch()  // 查找匹配的拦截规则          │
│   │                                                     │
│   ├─ handler.onRequestStart(controller)                │
│   │                                                     │
│   ├─ dispatchRequestBody()                             │
│   │   ├─ handler.onBodySent(chunk)  // 如果有          │
│   │   └─ handler.onRequestSent()                       │
│   │                                                     │
│   ├─ sendReply() (可选 delay)                          │
│   │   ├─ callback(opts)  // 动态响应                  │
│   │   │   └─ 如果是 Promise，等待 resolve              │
│   │   │                                                │
│   │   └─ handleReply()                                 │
│   │       ├─ handler.onResponseStart(code, headers)    │
│   │       ├─ handler.onResponseData(body)              │
│   │       └─ handler.onResponseEnd(trailers)           │
│   │                                                     │
│   └─ deleteMockDispatch()  // 清理已消费的规则         │
└─────────────────────────────────────────────────────────┘
```

#### 5.4.6 请求体处理

`dispatchRequestBody()`（第 536 行）处理 3 种请求体类型：

```javascript
function dispatchRequestBody (body, handler, controller, isAborted) {
  // 无 body hooks → 直接返回
  if (!hasBodyHooks) return body

  // null body → 只调 onRequestSent
  if (body == null) return callOnRequestSent(handler, controller)

  // AsyncIterable body → 异步迭代
  if (body[Symbol.asyncIterator]) return dispatchAsyncIterableBody(body, handler, controller)

  // Iterable body → 同步迭代
  if (isIterableBody(body)) {
    for (const chunk of body) {
      chunks.push(chunk)
      callOnBodySent(handler, controller, chunk)
    }
    callOnRequestSent(handler, controller)
    return chunks
  }

  // 普通 body
  callOnBodySent(handler, controller, body)
  callOnRequestSent(handler, controller)
  return body
}
```

#### 5.4.7 buildMockDispatch 替换引擎

`buildMockDispatch()`（第 627 行）是连接 MockAgent 和原始 dispatch 的桥梁：

```javascript
function buildMockDispatch () {
  const agent = this[kMockAgent]
  const origin = this[kOrigin]
  const originalDispatch = this[kOriginalDispatch]

  return function dispatch (opts, handler) {
    if (agent.isMockActive) {
      try {
        mockDispatch.call(this, opts, handler)
      } catch (error) {
        if (error.code === 'UND_MOCK_ERR_MOCK_NOT_MATCHED') {
          const netConnect = agent[kGetNetConnect]()
          const totalInterceptsCount = this[kDispatches][kTotalDispatchCount] || this[kDispatches].length
          const pendingInterceptsCount = this[kDispatches].filter(({ consumed }) => !consumed).length
          const interceptsMessage = `, ${pendingInterceptsCount} interceptor(s) remaining out of ${totalInterceptsCount} defined`
          if (netConnect === false) {
            throw new MockNotMatchedError(
              `${error.message}: subsequent request to origin ${origin} was not allowed (net.connect disabled)${interceptsMessage}`
            )
          }
          if (checkNetConnect(netConnect, origin)) {
            originalDispatch.call(this, '__mockAgentBodyForDispatch' in opts
              ? { ...opts, body: opts.__mockAgentBodyForDispatch }
              : opts, handler)
          } else {
            throw new MockNotMatchedError(
              `${error.message}: subsequent request to origin ${origin} was not allowed (net.connect is not enabled for this origin)${interceptsMessage}`
            )
          }
        } else {
          throw error
        }
      }
    } else {
      originalDispatch.call(this, opts, handler)  // mock 未激活，走真实路径
    }
  }
}
```

**设计要点**：
- 捕获 `MockNotMatchedError` 后回退到真实网络
- 错误信息包含剩余拦截器数量，便于调试
- `__mockAgentBodyForDispatch` 是内部标记，用于 body 修复

#### 5.4.8 checkNetConnect 白名单检查

```javascript
function checkNetConnect (netConnect, origin) {
  const url = new URL(origin)
  if (netConnect === true) {
    return true
  } else if (Array.isArray(netConnect) && netConnect.some((matcher) => matchValue(matcher, url.host))) {
    return true  // 匹配 host
  }
  return false
}
```

#### 5.4.9 getResponseData 数据类型处理

```javascript
function getResponseData (data) {
  if (Buffer.isBuffer(data)) return data
  else if (data instanceof Uint8Array) return data
  else if (data instanceof ArrayBuffer) return data
  else if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
  else if (typeof data === 'object') return JSON.stringify(data)
  else if (data) return data.toString()
  else return ''
}
```

#### 5.4.10 dispatchRequestBody 完整流程

```javascript
function dispatchRequestBody (body, handler, controller, isAborted) {
  // 1. 无 body hooks → 直接返回
  if (typeof handler.onBodySent !== 'function' && typeof handler.onRequestSent !== 'function') {
    return body
  }

  // 2. null body → 只调 onRequestSent
  if (body == null) {
    return callOnRequestSent(handler, controller, isAborted) ? body : requestAborted
  }

  // 3. AsyncIterable body → 异步迭代
  if (body && typeof body[Symbol.asyncIterator] === 'function') {
    return dispatchAsyncIterableBody(body, handler, controller, isAborted)
  }

  // 4. Iterable body → 同步迭代
  if (isIterableBody(body)) {
    const chunks = []
    for (const chunk of body) {
      if (isAborted()) return requestAborted
      chunks.push(chunk)
      if (!callOnBodySent(handler, controller, chunk) || isAborted()) {
        return requestAborted
      }
    }
    return callOnRequestSent(handler, controller, isAborted) ? chunks : requestAborted
  }

  // 5. 普通 body
  if (isAborted()) return requestAborted
  if (!callOnBodySent(handler, controller, body)) return requestAborted
  return callOnRequestSent(handler, controller, isAborted) ? body : requestAborted
}

// 判断是否为可迭代 body（排除 string/Buffer/TypedArray）
function isIterableBody (body) {
  return typeof body !== 'string' &&
    !Buffer.isBuffer(body) &&
    !ArrayBuffer.isView(body) &&
    typeof body[Symbol.iterator] === 'function'
}
```

#### 5.4.11 请求体发送钩子

```javascript
function callOnBodySent (handler, controller, chunk) {
  try {
    handler.onBodySent?.(chunk)
    return true
  } catch (error) {
    controller.abort(error)  // 错误时中止请求
    return false
  }
}

function callOnRequestSent (handler, controller, isAborted) {
  try {
    handler.onRequestSent?.()
    return !isAborted()
  } catch (error) {
    controller.abort(error)
    return false
  }
}
```

**关键设计**：钩子中的错误会触发 `controller.abort(error)`，让错误冒泡到 response error 回调。

### 5.5 MockCallHistory 调用历史

**文件**：`lib/mock/mock-call-history.js`（248 行）

#### 5.5.1 MockCallHistoryLog

```javascript
class MockCallHistoryLog {
  constructor (requestInit) {
    this.body = requestInit.body
    this.headers = requestInit.headers
    this.method = requestInit.method

    const url = computeUrlWithMaybeSearchParameters(requestInit)
    this.fullUrl = url.toString()
    this.origin = url.origin
    this.path = url.pathname
    this.searchParams = Object.fromEntries(url.searchParams)
    this.protocol = url.protocol
    this.host = url.host
    this.port = url.port
    this.hash = url.hash
  }
}
```

#### 5.5.2 多维过滤系统

```javascript
class MockCallHistory {
  logs = []

  // 便捷方法
  calls ()    { return this.logs }
  firstCall() { return this.logs.at(0) }
  lastCall()  { return this.logs.at(-1) }
  nthCall(n)  { return this.logs.at(n - 1) }  // 非零基索引

  // 多维过滤
  filterCalls (criteria, options) {
    if (typeof criteria === 'function') return this.logs.filter(criteria)
    if (criteria instanceof RegExp) return this.logs.filter(log => criteria.test(log.toString()))
    if (typeof criteria === 'object') {
      const finalOptions = { operator: 'OR', ...options }
      let filtered = finalOptions.operator === 'AND' ? this.logs : []

      // 8 个维度的过滤
      if ('protocol' in criteria) filtered = handleFilterCallsWithOptions(...)
      if ('host' in criteria) filtered = handleFilterCallsWithOptions(...)
      if ('port' in criteria) filtered = handleFilterCallsWithOptions(...)
      if ('origin' in criteria) filtered = handleFilterCallsWithOptions(...)
      if ('path' in criteria) filtered = handleFilterCallsWithOptions(...)
      if ('hash' in criteria) filtered = handleFilterCallsWithOptions(...)
      if ('fullUrl' in criteria) filtered = handleFilterCallsWithOptions(...)
      if ('method' in criteria) filtered = handleFilterCallsWithOptions(...)

      return [...new Set(filtered)]  // 去重
    }
  }

  // 8 个独立过滤器
  filterCallsByProtocol = makeFilterCalls('protocol')
  filterCallsByHost = makeFilterCalls('host')
  // ... 以此类推
}
```

**OR/AND 操作符**：支持组合条件的逻辑操作。

#### 5.5.3 MockCallHistoryLog 完整结构

```javascript
class MockCallHistoryLog {
  constructor (requestInit = {}) {
    this.body = requestInit.body
    this.headers = requestInit.headers
    this.method = requestInit.method

    const url = computeUrlWithMaybeSearchParameters(requestInit)

    this.fullUrl = url.toString()
    this.origin = url.origin
    this.path = url.pathname
    this.searchParams = Object.fromEntries(url.searchParams)
    this.protocol = url.protocol
    this.host = url.host
    this.port = url.port
    this.hash = url.hash
  }

  toMap () {
    return new Map([
      ['protocol', this.protocol],
      ['host', this.host],
      ['port', this.port],
      ['origin', this.origin],
      ['path', this.path],
      ['hash', this.hash],
      ['searchParams', this.searchParams],
      ['fullUrl', this.fullUrl],
      ['method', this.method],
      ['body', this.body],
      ['headers', this.headers]
    ])
  }

  toString () {
    const options = { betweenKeyValueSeparator: '->', betweenPairSeparator: '|' }
    let result = ''
    this.toMap().forEach((value, key) => {
      if (typeof value === 'string' || value === undefined || value === null) {
        result = `${result}${key}${options.betweenKeyValueSeparator}${value}${options.betweenPairSeparator}`
      }
      if ((typeof value === 'object' && value !== null) || Array.isArray(value)) {
        result = `${result}${key}${options.betweenKeyValueSeparator}${JSON.stringify(value)}${options.betweenPairSeparator}`
      }
    })
    return result.slice(0, -1)  // 移除最后一个分隔符
  }
}
```

#### 5.5.4 完整过滤 API

```javascript
const history = mockAgent.getCallHistory()

// 便捷方法
history.calls()         // 所有日志
history.firstCall()      // 第一条
history.lastCall()       // 最后一条
history.nthCall(3)       // 第 3 条（非零基索引）

// 8 个单维度过滤器
history.filterCallsByProtocol('https:')
history.filterCallsByHost('api.example.com')
history.filterCallsByPort('443')
history.filterCallsByOrigin('https://api.example.com')
history.filterCallsByPath('/v1/users')
history.filterCallsByHash('#section')
history.filterCallsByFullUrl('https://api.example.com/v1/users')
history.filterCallsByMethod('GET')

// 多维度组合过滤
history.filterCalls({
  method: 'POST',
  path: '/api/users'
}, { operator: 'OR' })  // 满足任一条件

history.filterCalls({
  method: 'GET',
  host: 'api.example.com'
}, { operator: 'AND' })  // 同时满足

// 函数过滤
history.filterCalls((log) => log.path.startsWith('/api/'))

// RegExp 过滤
history.filterCalls(/\/v1\/users\?page=\d+/)
```

#### 5.5.5 实现原理

```javascript
class MockCallHistory {
  logs = []

  filterCalls (criteria, options) {
    if (this.logs.length === 0) return this.logs
    if (typeof criteria === 'function') return this.logs.filter(criteria)
    if (criteria instanceof RegExp) return this.logs.filter(log => criteria.test(log.toString()))
    if (typeof criteria === 'object' && criteria !== null) {
      if (Object.keys(criteria).length === 0) return this.logs
      const finalOptions = { operator: 'OR', ...buildAndValidateFilterCallsOptions(options) }
      let maybeDuplicatedLogsFiltered = finalOptions.operator === 'AND' ? this.logs : []

      if ('protocol' in criteria) maybeDuplicatedLogsFiltered = handleFilterCallsWithOptions(...)
      if ('host' in criteria) maybeDuplicatedLogsFiltered = handleFilterCallsWithOptions(...)
      // ... 8 个维度

      return [...new Set(maybeDuplicatedLogsFiltered)]  // 去重
    }
  }
}

function makeFilterCalls (parameterName) {
  return (parameterValue, logs = this.logs) => {
    if (typeof parameterValue === 'string' || parameterValue == null) {
      return logs.filter(log => log[parameterName] === parameterValue)
    }
    if (parameterValue instanceof RegExp) {
      return logs.filter(log => parameterValue.test(log[parameterName]))
    }
    throw new InvalidArgumentError('...')
  }
}
```

### 5.6 MockErrors / MockSymbols

**文件**：`lib/mock/mock-errors.js`（29 行）、`lib/mock/mock-symbols.js`（32 行）

#### 5.6.1 MockNotMatchedError

```javascript
const kMockNotMatchedError = Symbol.for('undici.error.UND_MOCK_ERR_MOCK_NOT_MATCHED')

class MockNotMatchedError extends UndiciError {
  constructor (message) {
    super(message)
    this.name = 'MockNotMatchedError'
    this.message = message || 'The request does not match any registered mock dispatches'
    this.code = 'UND_MOCK_ERR_MOCK_NOT_MATCHED'
  }

  // Symbol.hasInstance 支持：instanceof 检查
  static [Symbol.hasInstance] (instance) {
    return instance && instance[kMockNotMatchedError] === true
  }

  get [kMockNotMatchedError] () {
    return true
  }
}
```

**设计要点**：
- 使用 `Symbol.for()` 注册全局 Symbol，支持跨 realm 检查
- 通过 `Symbol.hasInstance` 自定义 `instanceof` 行为
- 错误码 `UND_MOCK_ERR_MOCK_NOT_MATCHED` 用于在 `buildMockDispatch` 中识别

#### 5.6.2 MockSymbols

定义了 31 个 Symbol，用于隐藏内部状态。Symbol 是最佳内部状态隐藏方式：

| 分类 | Symbol | 用途 |
|------|--------|------|
| 核心 | `kAgent` | 真实 Agent 引用 |
| 核心 | `kOptions` | MockAgent 配置 |
| 核心 | `kFactory` | 创建 MockClient/MockPool 的工厂 |
| 拦截 | `kDispatches` | MockClient 上的拦截规则列表 |
| 拦截 | `kDispatchKey` | 拦截匹配键 |
| 拦截 | `kMockDispatch` | MockScope 上的 dispatch 数据 |
| 拦截 | `kDefaultHeaders` | 默认响应头 |
| 拦截 | `kDefaultTrailers` | 默认响应 trailers |
| 拦截 | `kContentLength` | 是否自动计算 Content-Length |
| Mock | `kMockAgent` | MockClient 反向引用 MockAgent |
| Mock | `kMockAgentSet` | 设置 client 到 clients Map |
| Mock | `kMockAgentGet` | 获取 client（支持模糊匹配） |
| Mock | `kClose` | Mock 专用 close |
| Mock | `kOriginalClose` | 原始 close |
| Mock | `kOriginalDispatch` | 原始 dispatch |
| Mock | `kOrigin` | origin |
| Mock | `kIsMockActive` | mock 激活状态 |
| Mock | `kNetConnect` | 网络连接控制 |
| Mock | `kGetNetConnect` | 获取网络连接控制 |
| Mock | `kConnected` | 连接状态 |
| Mock | `kIgnoreTrailingSlash` | 忽略尾部斜杠 |
| 历史 | `kMockAgentMockCallHistoryInstance` | 调用历史实例 |
| 历史 | `kMockAgentRegisterCallHistory` | 注册调用历史 |
| 历史 | `kMockAgentAddCallHistoryLog` | 添加调用日志 |
| 历史 | `kMockAgentIsCallHistoryEnabled` | 调用历史启用状态 |
| 历史 | `kMockCallHistoryAddLog` | 调用历史添加 |
| 搜索 | `kMockAgentAcceptsNonStandardSearchParameters` | 接受非标准搜索参数 |
| 统计 | `kTotalDispatchCount` | 注册的 dispatch 总数 |

```javascript
module.exports = {
  kAgent: Symbol('agent'),
  kOptions: Symbol('options'),
  kFactory: Symbol('factory'),
  kDispatches: Symbol('dispatches'),
  kDispatchKey: Symbol('dispatch key'),
  // ... 共 31 个
  kTotalDispatchCount: Symbol('total dispatch count')
}
```

---

## 6. Snapshot 快照系统

### 6.1 SnapshotAgent

**文件**：`lib/mock/snapshot-agent.js`（371 行）

SnapshotAgent 继承 MockAgent，支持 HTTP 交互的录制/回放。

#### 6.1.1 三种模式

```javascript
const validSnapshotModes = ['record', 'playback', 'update']

// record:   录制所有请求到快照文件
// playback: 从快照文件回放，无快照则报错
// update:   优先回放，无快照时录制新请求
```

#### 6.1.2 Dispatch 路由

```javascript
dispatch (opts, handler) {
  // URL 排除检查
  if (this[kSnapshotRecorder].isUrlExcluded(opts)) {
    return this[kRealAgent].dispatch(opts, handler)  // 直接走真实请求
  }

  if (mode === 'playback' || mode === 'update') {
    const snapshot = this[kSnapshotRecorder].findSnapshot(opts)
    if (snapshot) {
      return this.#replaySnapshot(snapshot, handler)  // 同步回放
    } else if (mode === 'update') {
      return this.#recordAndReplay(opts, handler)     // 录制并回放
    }
  } else if (mode === 'record') {
    return this.#recordAndReplay(opts, handler)        // 录制并回放
  }
}
```

#### 6.1.3 录制并回放

`#recordAndReplay()`（第 134 行）通过 handler 拦截收集响应数据：

```javascript
#recordAndReplay (opts, handler) {
  const responseData = { statusCode: null, headers: {}, trailers: {}, body: [] }

  const self = this  // 捕获 this 用于嵌套回调

  const recordingHandler = {
    onRequestStart (controller, context) {
      return handler.onRequestStart(controller, { ...context, history: this.history })
    },
    onRequestUpgrade (controller, statusCode, headers, socket) {
      return handler.onRequestUpgrade(controller, statusCode, headers, socket)
    },
    onResponseStart (controller, statusCode, headers, statusMessage) {
      responseData.statusCode = statusCode
      responseData.headers = headers
      return handler.onResponseStart(controller, statusCode, headers, statusMessage)
    },
    onResponseData (controller, chunk) {
      responseData.body.push(chunk)
      return handler.onResponseData(controller, chunk)
    },
    onResponseEnd (controller, trailers) {
      responseData.trailers = trailers
      // 异步录制（fire and forget）
      const responseBody = Buffer.concat(responseData.body)
      self[kSnapshotRecorder].record(opts, {
        statusCode: responseData.statusCode,
        headers: responseData.headers,
        body: responseBody,
        trailers: responseData.trailers
      })
        .then(() => handler.onResponseEnd(controller, trailers))
        .catch((error) => handler.onResponseError(controller, error))
    },
    onResponseError (controller, error) {
      return handler.onResponseError(controller, error)
    }
  }

  const agent = this[kRealAgent]
  return agent.dispatch(opts, recordingHandler)
}
```

**设计要点**：
- 录制是异步的（fire-and-forget），不阻塞响应返回给用户
- `handler.onResponseEnd` 等待录制完成后再调用，确保数据落盘

#### 6.1.4 URL 排除机制

```javascript
dispatch (opts, handler) {
  // URL 排除检查 - 直接走真实请求
  if (this[kSnapshotRecorder].isUrlExcluded(opts)) {
    return this[kRealAgent].dispatch(opts, handler)
  }
  // ... 正常录制/回放逻辑
}
```

```javascript
// SnapshotRecorder.isUrlExcluded
isUrlExcluded (requestOpts) {
  const url = new URL(requestOpts.path, requestOpts.origin).toString()
  return this.#isUrlExcluded(url)
}

// snapshot-utils.js
function isUrlExcludedFactory (excludePatterns = []) {
  if (excludePatterns.length === 0) return () => false

  return function isUrlExcluded (url) {
    let urlLowerCased
    for (const pattern of excludePatterns) {
      if (typeof pattern === 'string') {
        if (!urlLowerCased) urlLowerCased = url.toLowerCase()
        if (urlLowerCased.includes(pattern.toLowerCase())) return true
      } else if (pattern instanceof RegExp) {
        if (pattern.test(url)) return true
      }
    }
    return false
  }
}
```

#### 6.1.4 快照回放

```javascript
#replaySnapshot (snapshot, handler) {
  const { response } = snapshot

  const controller = {
    rawHeaders: util.toRawHeaders(response.headers),
    rawTrailers: util.toRawHeaders(response.trailers),
    pause () {}, resume () {},
    abort (reason) { this.aborted = true; this.reason = reason },
    aborted: false, paused: false
  }

  handler.onRequestStart(controller)
  handler.onResponseStart(controller, response.statusCode, response.headers, response.statusMessage)

  // body 始终以 base64 存储
  const body = Buffer.from(response.body, 'base64')
  handler.onResponseData(controller, body)
  handler.onResponseEnd(controller, response.trailers)
}
```

#### 6.1.5 Playback 模式 Mock 拦截器设置

```javascript
#setupMockInterceptors () {
  for (const snapshot of this[kSnapshotRecorder].getSnapshots()) {
    const { request, responses, response } = snapshot
    const url = new URL(request.url)
    const mockPool = this.get(url.origin)
    const responseData = responses ? responses[0] : response

    mockPool.intercept({
      path: url.pathname + url.search,
      method: request.method,
      headers: request.headers,
      body: request.body
    }).reply(responseData.statusCode, responseData.body, {
      headers: responseData.headers,
      trailers: responseData.trailers
    }).persist()
  }
}
```

### 6.2 SnapshotRecorder

**文件**：`lib/mock/snapshot-recorder.js`（623 行）

#### 6.2.1 请求哈希

`createRequestHash()`（第 214 行）生成请求指纹：

```javascript
function createRequestHash (formattedRequest) {
  const parts = [formattedRequest.method, formattedRequest.url]

  // 确定性 header 排序
  if (formattedRequest.headers && typeof formattedRequest.headers === 'object') {
    const headerKeys = Object.keys(formattedRequest.headers).sort()
    for (const key of headerKeys) {
      const values = Array.isArray(formattedRequest.headers[key])
        ? formattedRequest.headers[key]
        : [formattedRequest.headers[key]]

      parts.push(key)
      // 值也排序，保证一致性
      for (const value of values.sort()) {
        parts.push(String(value))
      }
    }
  }

  parts.push(formattedRequest.body)
  const content = parts.join('|')
  return hashId(content)  // SHA-256 或 base64url
}
```

**哈希算法选择**：

```javascript
// snapshot-utils.js
const crypto = runtimeFeatures.has('crypto')
  ? require('node:crypto')
  : null

const hashId = crypto?.hash
  ? (value) => crypto.hash('sha256', value, 'base64url')  // Node.js crypto
  : (value) => Buffer.from(value).toString('base64url')    // 降级方案
```

#### 6.2.2 请求格式化

`formatRequestKey()`（第 124 行）标准化请求：

```javascript
function formatRequestKey (opts, headerFilters, matchOptions) {
  const url = new URL(opts.path, opts.origin)

  return {
    method: opts.method || 'GET',
    url: normalizeUrlForMatching(url, matchOptions.matchQuery, matchOptions.normalizeQuery),
    headers: filterHeadersForMatching(normalizedHeaders, headerFilters, matchOptions),
    body: normalizeBodyForMatching(opts.body, matchOptions.matchBody, matchOptions.normalizeBody)
  }
}
```

#### 6.2.3 顺序响应支持

快照支持多次调用返回不同响应（用于模拟状态变化的 API）：

```javascript
// record() 中
const existingSnapshot = this.#snapshots.get(hash)
if (existingSnapshot && existingSnapshot.responses) {
  existingSnapshot.responses.push(responseData)  // 追加新响应
} else {
  this.#snapshots.set(hash, {
    request, responses: [responseData], callCount: 0, timestamp
  })
}

// findSnapshot() 中
const currentCallCount = snapshot.callCount || 0
const responseIndex = Math.min(currentCallCount, snapshot.responses.length - 1)
snapshot.callCount = currentCallCount + 1
return { ...snapshot, response: snapshot.responses[responseIndex] }
```

#### 6.2.4 自动刷新

```javascript
#scheduleFlush () {
  this.#flushTimeout = setTimeout(() => {
    this.saveSnapshots().catch(() => {})  // 静默失败
    if (this.#autoFlush) {
      this.#flushTimeout?.refresh()  // 自动续期
    }
  }, 1000)  // 1 秒防抖
}
```

#### 6.2.5 持久化格式

快照保存为 JSON 文件：

```javascript
// saveSnapshots()
async saveSnapshots (filePath) {
  const path = filePath || this.#snapshotPath
  const resolvedPath = resolve(path)

  // 确保目录存在
  await mkdir(dirname(resolvedPath), { recursive: true })

  // Map → Array 序列化
  const data = Array.from(this.#snapshots.entries()).map(([hash, snapshot]) => ({
    hash,
    snapshot
  }))

  await writeFile(resolvedPath, JSON.stringify(data, null, 2), { flush: true })
}

// 文件格式示例:
// [
//   {
//     "hash": "base64url-hash",
//     "snapshot": {
//       "request": { "method": "GET", "url": "http://api.example.com/v1/users", ... },
//       "responses": [
//         { "statusCode": 200, "headers": {...}, "body": "base64...", "trailers": {} }
//       ],
//       "callCount": 0,
//       "timestamp": "2024-01-01T00:00:00.000Z"
//     }
//   }
// ]
```

#### 6.2.6 加载快照

```javascript
async loadSnapshots (filePath) {
  const path = filePath || this.#snapshotPath
  const data = await readFile(resolve(path), 'utf8')
  const parsed = JSON.parse(data)

  if (Array.isArray(parsed)) {
    this.#snapshots.clear()
    for (const { hash, snapshot } of parsed) {
      this.#snapshots.set(hash, snapshot)
    }
  } else {
    // Legacy object format
    this.#snapshots = new Map(Object.entries(parsed))
  }
}
```

#### 6.2.7 Header 过滤

三种 Header 过滤策略：

```javascript
// 匹配用：只保留 matchHeaders 中列出的 header
function filterHeadersForMatching (headers, headerFilters, matchOptions) {
  for (const [key, value] of Object.entries(headers)) {
    if (exclude.has(headerKey)) continue    // 安全排除（如 Authorization）
    if (ignore.has(headerKey)) continue     // 忽略（如 Date）
    if (match.size !== 0 && !match.has(headerKey)) continue  // 白名单匹配
    filtered[headerKey] = value
  }
}

// 存储用：只排除安全敏感 header
function filterHeadersForStorage (headers, headerFilters, matchOptions) {
  for (const [key, value] of Object.entries(headers)) {
    if (excludeSet.has(headerKey)) continue  // 只排除敏感 header
    filtered[headerKey] = value
  }
}
```

### 6.3 SnapshotUtils

**文件**：`lib/mock/snapshot-utils.js`（158 行）

#### 6.3.1 Header 过滤缓存

```javascript
function createHeaderFilters (matchOptions) {
  return {
    ignore: new Set(ignoreHeaders.map(h => caseSensitive ? h : h.toLowerCase())),
    exclude: new Set(excludeHeaders.map(h => caseSensitive ? h : h.toLowerCase())),
    match: new Set(matchHeaders.map(h => caseSensitive ? h : h.toLowerCase()))
  }
}
```

#### 6.3.2 Hash 函数

```javascript
const hashId = crypto?.hash
  ? (value) => crypto.hash('sha256', value, 'base64url')  // Node.js crypto
  : (value) => Buffer.from(value).toString('base64url')    // 降级方案
```

#### 6.3.3 URL 排除工厂

```javascript
function isUrlExcludedFactory (excludePatterns = []) {
  if (excludePatterns.length === 0) return () => false

  return function isUrlExcluded (url) {
    for (const pattern of excludePatterns) {
      if (typeof pattern === 'string') {
        if (url.toLowerCase().includes(pattern.toLowerCase())) return true
      } else if (pattern instanceof RegExp) {
        if (pattern.test(url)) return true
      }
    }
    return false
  }
}
```

---

## 7. 工具层分析

### 7.1 Cache-Control 解析器

**文件**：`lib/util/cache.js`（716 行）

#### 7.1.1 核心解析函数

`parseCacheControlHeader()`（第 314 行）是 RFC 9111 Cache-Control 的完整实现：

```javascript
function parseCacheControlHeader (header) {
  const output = {}
  const directives = splitCacheControlHeaderValue(header)

  for (const directiveRecord of directives) {
    switch (key) {
      // 数值型指令：max-age, s-maxage, min-fresh, max-stale, stale-while-revalidate, stale-if-error
      case 'max-age': {
        const parsedValue = Math.min(parseInt(value, 10), MAX_DELTA_SECONDS)
        output[key] = parsedValue  // max-age 取最小值
        break
      }
      case 'min-fresh': {
        output[key] = parsedValue  // min-fresh 取最大值
        break
      }
      // 布尔型指令：public, no-store, must-revalidate, etc.
      case 'no-store': output[key] = true; break
      // 带参数的指令：private, no-cache（可带 header 列表）
      case 'private':
      case 'no-cache': {
        // 处理 private="header1, header2" 跨逗号解析
        // ...
      }
    }
  }
  return output
}
```

#### 7.1.2 跨逗号引号解析

`splitCacheControlHeaderValue()`（第 53 行）处理 `no-cache="header1, header2"` 这种跨逗号引号：

```javascript
function splitCacheControlHeaderValue (value) {
  let inQuote = false
  let escaped = false
  for (let i = 0; i < value.length; i++) {
    if (inQuote) {
      if (escaped) { escaped = false }
      else if (value[i] === '\\') { escaped = true }
      else if (value[i] === '"') { inQuote = false }
    } else if (value[i] === '"') {
      inQuote = true
    } else if (value[i] === ',' && !inQuote) {
      directives.push(value.substring(start, i))
      start = i + 1
    }
  }
}
```

#### 7.1.3 Header 归一化

`normalizeHeaders()`（第 185 行）处理 3 种 header 格式：

```javascript
function normalizeHeaders (opts) {
  // 1. 对象格式：{ 'content-type': 'application/json' }
  // 2. 平坦数组格式：['content-type', 'application/json', 'accept', '*/*']
  // 3. 嵌套数组格式：[['content-type', 'application/json'], ['accept', '*/*']]
  // 4. 可迭代格式（Map 等）
}
```

#### 7.1.4 ETag 可用性检查

```javascript
function isEtagUsable (etag) {
  if (etag.length <= 2) return false  // 空 etag
  if (etag[0] === '"' && etag[etag.length - 1] === '"') {
    return !(etag[1] === '"' || etag.startsWith('"W/'))  // 双引号嵌套
  }
  if (etag.startsWith('W/"') && etag[etag.length - 1] === '"') {
    return etag.length !== 4  // W/"" 无效
  }
  return false
}
```

### 7.2 HTTP 日期解析器

**文件**：`lib/util/date.js`（670 行）

实现了 RFC 9110 定义的三种 HTTP 日期格式：

```javascript
function parseHttpDate (date) {
  switch (date[3]) {
    case ',': return parseImfDate(date)     // "Sun, 06 Nov 1994 08:49:37 GMT"
    case ' ': return parseAscTimeDate(date) // "Sun Nov  6 08:49:37 1994"
    default: return parseRfc850Date(date)   // "Sunday, 06-Nov-94 08:49:37 GMT"
  }
}
```

**设计要点**：
- 完全手写解析器，避免 `new Date()` 的 V8 开销
- 逐字符验证（`charCodeAt`），精确到每个字段
- 周几一致性校验（`makeDate` 中 `getUTCDay() === weekday`）
- RFC 6265 年份处理（两位年份 < 70 → 2000+，>= 70 → 1900+）

### 7.3 快速定时器

**文件**：`lib/util/timers.js`（425 行）

#### 7.3.1 设计理念

低精度定时器（精度 ~500ms），适用于 > 1s 的延迟场景，减少 `setTimeout` 的开销：

```javascript
const RESOLUTION_MS = 1e3      // 1 秒精度
const TICK_MS = (RESOLUTION_MS >> 1) - 1  // 499ms tick 间隔
```

#### 7.3.2 FastTimer 状态机

```
NOT_IN_LIST (-2)  →  PENDING (0)  →  ACTIVE (1)  →  TO_BE_CLEARED (-1)  →  NOT_IN_LIST
      ↑                    |                |                                    |
      └──── refresh() ─────┘                └── timer expired ──────────────────┘
```

- `NOT_IN_LIST`：不在活跃列表中
- `PENDING`：刚创建或刷新，等待下一次 tick 设置 `_idleStart`
- `ACTIVE`：等待过期
- `TO_BE_CLEARED`：已清除，等待从数组移除

#### 7.3.3 onTick 处理

```javascript
function onTick () {
  fastNow += TICK_MS  // 固定增量，不依赖系统时钟

  let idx = 0, len = fastTimers.length
  while (idx < len) {
    const timer = fastTimers[idx]

    if (timer._state === PENDING) {
      timer._idleStart = fastNow - TICK_MS
      timer._state = ACTIVE
    } else if (timer._state === ACTIVE && fastNow >= timer._idleStart + timer._idleTimeout) {
      timer._state = TO_BE_CLEARED
      timer._onTimeout(timer._timerArg)  // 触发回调
    }

    if (timer._state === TO_BE_CLEARED) {
      timer._state = NOT_IN_LIST
      if (--len !== 0) fastTimers[idx] = fastTimers[len]  // 交换移除
    } else {
      ++idx
    }
  }
  fastTimers.length = len  // 截断数组
}
```

**性能优化**：
- 使用数组而非 Map/Set（CPU 缓存友好）
- 交换移除（O(1) 而非 O(n)）
- `unref()` 允许进程退出

#### 7.3.4 自动降级

```javascript
setTimeout (callback, delay, arg) {
  return delay <= RESOLUTION_MS
    ? setTimeout(callback, delay, arg)    // 短延迟用原生
    : new FastTimer(callback, delay, arg) // 长延迟用 FastTimer
}
```

### 7.4 运行时特性检测

**文件**：`lib/util/runtime-features.js`（93 行）

```javascript
class RuntimeFeatures {
  #map = new Map()

  has (feature) {
    return this.#map.get(feature) ?? this.#detectRuntimeFeature(feature)
  }

  #detectRuntimeFeature (feature) {
    try {
      require(`node:${feature}`)
      return true
    } catch (err) {
      if (err.code !== 'ERR_UNKNOWN_BUILTIN_MODULE' && err.code !== 'ERR_NO_CRYPTO') throw err
      return false
    }
  }
}
```

目前支持：`crypto`、`sqlite`。

### 7.5 连接统计

**文件**：`lib/util/stats.js`（32 行）

```javascript
class ClientStats {
  constructor (client) {
    this.connected = client[kConnected]
    this.pending = client[kPending]
    this.running = client[kRunning]
    this.size = client[kSize]
  }
}

class PoolStats {
  constructor (pool) {
    this.connected = pool[kConnected]
    this.free = pool[kFree]
    this.pending = pool[kPending]
    this.queued = pool[kQueued]
    this.running = pool[kRunning]
    this.size = pool[kSize]
  }
}
```

---

## 8. 拦截器组合最佳实践与陷阱（新增章节）

### 8.1 拦截器执行顺序

拦截器通过 `Dispatcher.compose()` 链式组合，**外层先执行**：

```javascript
// 执行顺序：retry → redirect → dns → decompress → dispatch
const dispatcher = dispatcher.compose(
  retry(),
  redirect(),
  dns(),
  decompress()
)
```

```
请求流向 (从外到内):
  retry (最外层)
    ↓
  redirect
    ↓
  dns (修改 origin)
    ↓
  decompress
    ↓
  dispatch (实际发送)
```

**执行顺序规则**：
1. `onRequestStart`：从外到内（retry → redirect → dns → decompress）
2. `onResponseStart`：从内到外（decompress → dns → redirect → retry）
3. `onResponseData`：从内到外
4. `onResponseEnd`：从内到外

### 8.2 推荐的拦截器顺序

```javascript
// 推荐顺序（从外到内）
const dispatcher = compose(
  dump({ maxSize: 10 * 1024 * 1024 }),  // 1. 大小限制（最外层）
  responseError(),                        // 2. 错误转换
  cache({ store }),                       // 3. 缓存（跳过已缓存的）
  retry({ maxRetries: 3 }),              // 4. 重试
  redirect({ maxRedirections: 5 }),      // 5. 重定向
  dns({ dualStack: true }),              // 6. DNS
  decompress(),                           // 7. 解压（最内层，先处理响应）
  deduplicate()                           // 8. 去重（可选）
)
```

**顺序原则**：
- **越外层的拦截器越先处理请求，越后处理响应**
- `dump`/`responseError` 放在最外层，确保捕获所有错误
- `cache` 放前面，避免重复请求进入重试/重定向
- `decompress` 放最内层，先解压再给上层处理

### 8.3 拦截器互斥与协作

#### 8.3.1 Cache + Revalidation 协作

```
请求到达
  │
  ├─ 缓存拦截器 (cache.js)
  │   ├─ 未命中 → 包装 CacheHandler 转发
  │   ├─ 命中新鲜 → 直接返回缓存
  │   └─ 命中过期 → 发送条件请求 + CacheRevalidationHandler
  │
  └─ CacheRevalidationHandler 回调
      ├─ 304 → 使用缓存 body，更新元数据
      └─ 200 → 使用新响应，更新缓存
```

#### 8.3.2 Retry + Range 协作

当重试时 body 已部分消费，RetryHandler 使用 Range 请求续传：

```
第一次请求:
  GET /file
  ↓ (连接断开，已接收 1000/5000 bytes)

重试请求:
  GET /file
  Range: bytes=1000-4999
  If-Match: "etag-value"  (如果服务器支持 ETag)
  ↓
  206 Partial Content
  Content-Range: bytes 1000-4999/5000
```

### 8.4 常见陷阱

#### 8.4.1 陷阱一：拦截器顺序错误

```javascript
// ❌ 错误：decompress 在 retry 外
const bad = compose(
  decompress(),  // 先解压
  retry()        // 重试时需要重新解压，但旧连接已销毁
)

// ✅ 正确：retry 在 decompress 外
const good = compose(
  retry(),       # 重试整个连接
  decompress()   # 解压在重试成功后
)
```

#### 8.4.2 陷阱二：MockAgent 忘记 activate/deactivate

```javascript
const mockAgent = new MockAgent()
mockAgent.disableNetConnect()

const client = new Client('http://localhost', { agent: mockAgent })

// ❌ 忘记设置拦截器就发送请求
await client.request({ path: '/' })
// 抛出 MockNotMatchedError

// ✅ 先设置拦截器
mockAgent.get('http://localhost')
  .intercept({ path: '/' })
  .reply(200, 'ok')

// ✅ 或者在测试结束时断言
mockAgent.assertNoPendingInterceptors()
```

#### 8.4.3 陷阱三：RetryHandler 的 body 流式问题

```javascript
// ❌ 错误：body 是 stream，重试时无法回退
dispatch({
  method: 'POST',
  body: createReadStream('file.txt')  // 流式 body
})

// ✅ 使用 wrapRequestBody 包装可重试 body
const body = require('../core/util').wrapRequestBody(streamOrBuffer)
```

#### 8.4.4 陷阱四：CacheHandler 的 no-cache vs no-store

| 指令 | 行为 |
|------|------|
| `no-store` | 完全不缓存（skip） |
| `no-cache` | 缓存但每次验证（304） |
| `max-age=0` | 同 no-cache |
| `must-revalidate` | 新鲜时直接用过期时必须验证 |

#### 8.4.5 陷阱五：DeduplicationHandler 的 buffer 溢出

```javascript
// 暂停状态下的 waiting handler 会缓冲数据
// 如果消费太慢，缓冲超过 maxBufferSize (5MB) 会抛错

// ✅ 消费端保持消费
for await (const chunk of response.body) {
  // 立即处理
}

// ❌ 暂停太久不消费
response.body.pause()
// ... 长时间不 resume
```

### 8.5 Handler 返回值约定

| 回调 | 返回值含义 |
|------|-----------|
| `onRequestStart` | 无返回值 |
| `onResponseStart` | `true` 表示拦截数据（不传给下游） |
| `onResponseData` | `true` 表示拦截数据（不传给下游） |
| `onResponseEnd` | 无返回值 |
| `onResponseError` | 无返回值 |

### 8.6 Symbol 命名约定

undici 使用多种 Symbol 策略：

```javascript
// 1. 实例 Symbol - 每个实例唯一（默认）
const kMyState = Symbol('my state')

// 2. 全局 Symbol - 跨 realm 共享
const kShared = Symbol.for('undici.shared')

// 3. 静态符号方法 - 类级别
class RetryHandler {
  static [kRetryHandlerDefaultRetry] = (err, ctx, cb) => { ... }
}
```

---

## 9. 跨模块设计模式总结（原第8节重编号）

### 8.1 模式一：柯里化拦截器组合

**位置**：所有 `lib/interceptor/*.js`

```javascript
// 统一签名
const interceptor = (options) => (dispatch) => (opts, handler) => {
  // 可以修改 opts
  // 可以替换 handler
  // 可以短路跳过
  // 可以异步延迟
  return dispatch(opts, newHandler)
}
```

**借鉴价值**：laew 的工具链可以采用相同的柯里化模式，实现工具的灵活组合。

### 8.2 模式二：Handler 装饰器链

**位置**：`lib/handler/decorator-handler.js` 及其子类

```
原始 Handler
  ← DecoratorHandler（生命周期保护）
    ← CacheHandler（缓存写入）
      ← CacheRevalidationHandler（条件请求协调）
    ← RetryHandler（重试 + Range 续传）
      ← RetryController（controller 代理）
    ← RedirectHandler（重定向 + header 清理）
    ← DeduplicationHandler（去重 + 多 handler 广播）
    ← DumpHandler（大小限制）
    ← ResponseErrorHandler（错误码转 Error）
    ← DecompressHandler（自动解压）
```

**借鉴价值**：laew 的 SubAgent 工具执行可以采用装饰器模式，在执行前后注入日志、限流、重试等横切关注点。

### 8.3 模式三：dispatch 替换

**位置**：`lib/mock/mock-client.js`、`mock-pool.js`

```javascript
constructor () {
  this[kOriginalDispatch] = this.dispatch
  this.dispatch = buildMockDispatch.call(this)
}
```

**借鉴价值**：laew 的 LlmClient 可以用类似方式替换 dispatch，实现请求拦截/录制/回放。

### 8.4 模式四：Symbol 隐藏内部状态

**位置**：`lib/mock/mock-symbols.js`（31 个 Symbol）

所有 Mock 系统的内部状态都通过 Symbol 属性存储，避免用户代码意外访问。

**借鉴价值**：laew 的 Agent 内部状态（context、memory、tools）可以用 Symbol 保护。

### 8.5 模式五：同结构代理（Controller Proxy）

**位置**：`lib/handler/retry-handler.js` - `RetryController`

```javascript
class RetryController {
  target = null  // 指向当前活跃的 controller
  pause () { this.target?.pause() }
  resume () { this.target?.resume() }
  abort (reason) { this.target?.abort(reason); this.#onAbort(reason) }
}
```

**借鉴价值**：laew 的多 Agent 协作中，当 SubAgent 需要切换执行环境时，可以用代理模式保持稳定的引用。

### 8.6 模式六：快照录制/回放

**位置**：`lib/mock/snapshot-agent.js` + `snapshot-recorder.js`

```
record 模式：真实请求 → 拦截响应 → 计算哈希 → 存储 JSON
playback 模式：请求 → 计算哈希 → 查找快照 → 模拟响应
update 模式：请求 → 查找快照 → 命中则回放，未命中则录制
```

**借鉴价值**：laew 的 E2E 测试可以采用 Snapshot 模式录制真实 LLM 交互，后续测试回放。

### 8.7 模式七：匹配器多态

**位置**：`lib/mock/mock-utils.js` - `matchValue()`

```javascript
function matchValue (match, value) {
  if (typeof match === 'string') return match === value
  if (match instanceof RegExp) return match.test(value)
  if (typeof match === 'function') return match(value) === true
  return false
}
```

**借鉴价值**：laew 的任务路由、工具匹配可以用同样的多态匹配器。

### 8.8 模式八：diagnostics_channel 集成

**位置**：`lib/interceptor/deduplicate.js`

```javascript
const pendingRequestsChannel = diagnosticsChannel.channel('undici:request:pending-requests')
if (pendingRequestsChannel.hasSubscribers) {
  pendingRequestsChannel.publish({ size, key, type: 'added' })
}
```

**借鉴价值**：laew 可以用 `diagnostics_channel` 实现跨进程的 Agent 遥测。

### 8.9 模式九：低精度批量定时器

**位置**：`lib/util/timers.js` - `FastTimer`

用单个 `setTimeout` 驱动 N 个定时器，精度换取吞吐量。

**借鉴价值**：laew 的 Session 超时、缓存过期可以使用类似的批量定时器。

### 8.10 模式十：防抖自动持久化

**位置**：`lib/mock/snapshot-recorder.js` - `#scheduleFlush()`

```javascript
#scheduleFlush () {
  this.#flushTimeout = setTimeout(() => {
    this.saveSnapshots().catch(() => {})
    if (this.#autoFlush) this.#flushTimeout?.refresh()
  }, 1000)  // 1 秒防抖
}
```

**借鉴价值**：laew 的 `agent_memory` / `session_memory` 可以用防抖写入减少 SQLite I/O。

---

## 9. 对 AI Agent HTTP 测试的借鉴

### 9.1 laew 当前 HTTP 测试现状

laew 当前的 HTTP 测试使用自定义的 mock LLM 方案，通过环境变量控制返回固定响应。与 undici 的 Mock 系统相比，存在以下不足：

| 维度 | laew 现状 | undici Mock |
|------|-----------|-------------|
| 请求匹配 | 固定路由 | 多维匹配（path/method/body/headers，支持 RegExp/Function） |
| 响应配置 | 硬编码 | 链式 API（reply/delay/persist/times） |
| 错误模拟 | 简单错误 | replyWithError + 完整生命周期 |
| 调用历史 | 无 | MockCallHistory（8 维过滤 + OR/AND） |
| 未消费断言 | 无 | assertNoPendingInterceptors |
| 录制/回放 | 无 | SnapshotAgent（3 模式 + 顺序响应） |
| 网络控制 | 无 | enableNetConnect/disableNetConnect |

### 9.2 具体借鉴建议

#### 9.2.1 Mock LLM Client

借鉴 MockAgent 的 `dispatch` 替换模式：

```rust
// laew 设想：MockLlmClient
struct MockLlmClient {
    original: Box<dyn LlmClient>,
    dispatches: Vec<MockDispatch>,
    is_active: bool,
}

impl LlmClient for MockLlmClient {
    async fn complete(&self, messages: &[Message], opts: &RequestMeta) -> Result<Response> {
        if self.is_active {
            let matched = self.find_match(opts)?;
            return matched.reply(opts).await;
        }
        self.original.complete(messages, opts).await
    }
}
```

#### 9.2.2 多维请求匹配

借鉴 `matchValue` + `matchKey` 的多态匹配：

```rust
enum Matcher<T> {
    Exact(T),
    Regex(Regex),
    Function(Box<dyn Fn(&T) -> bool>),
}

struct MockDispatch {
    path: Matcher<String>,
    method: Matcher<String>,
    body: Matcher<Option<String>>,
    headers: HashMap<String, Matcher<String>>,
}
```

#### 9.2.3 调用历史记录

借鉴 MockCallHistory 的多维过滤：

```rust
struct CallHistory {
    logs: Vec<CallLog>,
}

struct CallLog {
    model: String,
    endpoint: String,
    method: String,
    headers: HashMap<String, String>,
    body: String,
    timestamp: SystemTime,
}

impl CallHistory {
    fn filter_calls(&self, criteria: FilterCriteria) -> Vec<&CallLog> { ... }
    fn first_call(&self) -> Option<&CallLog> { ... }
    fn nth_call(&self, n: usize) -> Option<&CallLog> { ... }
}
```

#### 9.2.4 LLM 交互快照

借鉴 SnapshotAgent 的录制/回放：

```rust
// 第一次运行：录制真实 LLM 交互
let agent = SnapshotAgent::new("record", "./fixtures/llm-snapshots.json");
agent.complete(messages, opts).await?;  // 自动录制

// 后续测试：回放快照
let agent = SnapshotAgent::new("playback", "./fixtures/llm-snapshots.json");
agent.complete(messages, opts).await?;  // 从快照回放，无需网络
```

#### 9.2.5 链式拦截器

借鉴 Interceptor 链式组合，为 LLM 请求注入日志、限流、重试：

```rust
let client = LlmClient::new(config)
    .with_interceptor(logging_interceptor())
    .with_interceptor(retry_interceptor(RetryOptions { max_retries: 3 }))
    .with_interceptor(rate_limit_interceptor(100));  // 100 req/min
```

---

## 10. laew 借鉴路线图

### P0（立即借鉴）

1. **Mock LLM Client 实现**：参考 `MockAgent` + `buildMockDispatch`，创建 `MockLlmClient` 替换真实 LLM 调用
2. **多维请求匹配器**：参考 `matchValue` + `matchKey`，支持 string/regex/function 匹配
3. **链式 Mock 配置 API**：参考 `MockInterceptor` + `MockScope`，提供 `.reply().delay().times()` 链式 API

### P1（近期借鉴）

4. **调用历史记录**：参考 `MockCallHistory`，记录所有 LLM 交互并支持多维过滤
5. **未消费断言**：参考 `assertNoPendingInterceptors`，测试结束时断言所有 mock 都被消费
6. **防抖持久化**：参考 `SnapshotRecorder.#scheduleFlush()`，优化 `agent_memory` 写入

### P2（中期借鉴）

7. **LLM 交互快照**：参考 `SnapshotAgent`，实现录制/回放模式的 E2E 测试
8. **拦截器链**：参考 Interceptor 柯里化组合，为 LLM 请求注入日志/限流/重试
9. **Controller 代理**：参考 `RetryController`，在多 Agent 协作中保持稳定的 controller 引用

### P3（远期借鉴）

10. **diagnostics_channel 集成**：参考 deduplicate 拦截器，实现跨进程 Agent 遥测
11. **低精度批量定时器**：参考 `FastTimer`，优化 Session 超时和缓存过期
12. **HTTP Cache-Control 解析**：参考 `lib/util/cache.js`，为 laew 的 HTTP 工具提供缓存能力

---

## 附录：关键函数索引

### Interceptor 层

| 文件 | 函数/类 | 行号 | 用途 |
|------|---------|------|------|
| `lib/interceptor/cache.js` | `module.exports` (入口) | 501 | Cache 拦截器工厂 |
| `lib/interceptor/cache.js` | `handleResult()` | 379 | 缓存命中/未命中决策 |
| `lib/interceptor/cache.js` | `sendCachedValue()` | 293 | 发送缓存响应（构造虚拟 controller + Readable） |
| `lib/interceptor/cache.js` | `isStale()` | 180 | 过期判断 |
| `lib/interceptor/cache.js` | `needsRevalidation()` | 80 | 验证需求判断 |
| `lib/interceptor/cache.js` | `withinStaleWhileRevalidateWindow()` | 214 | SWR 窗口判断 |
| `lib/interceptor/cache.js` | `makeRevalidationHeaders()` | 153 | 条件请求头构造 |
| `lib/interceptor/retry.js` | `module.exports` | 4 | Retry 拦截器工厂（19 行纯委托） |
| `lib/interceptor/redirect.js` | `createRedirectInterceptor()` | 5 | Redirect 拦截器工厂 |
| `lib/interceptor/dns.js` | `module.exports` | 458 | DNS 拦截器工厂（575 行） |
| `lib/interceptor/dns.js` | `DNSInstance.runLookup()` | 156 | DNS 查询入口 |
| `lib/interceptor/dns.js` | `DNSInstance.#defaultLookup()` | 241 | 默认 DNS 查询 |
| `lib/interceptor/dns.js` | `DNSInstance.#defaultPick()` | 267 | IP 轮询算法 |
| `lib/interceptor/dns.js` | `DNSInstance.setRecords()` | 353 | 设置 DNS 记录（含 TTL） |
| `lib/interceptor/dns.js` | `DNSDispatchHandler.onResponseError()` | 404 | 双栈故障转移 |
| `lib/interceptor/decompress.js` | `createDecompressInterceptor()` | 270 | 解压拦截器工厂（292 行） |
| `lib/interceptor/decompress.js` | `DecompressHandler.#createDecompressionChain()` | 68 | 多级解压链创建 |
| `lib/interceptor/decompress.js` | `DecompressHandler.#setupSingleDecompressor()` | 123 | 单解压器事件 |
| `lib/interceptor/decompress.js` | `DecompressHandler.#setupMultipleDecompressors()` | 137 | 多解压器 pipeline |
| `lib/interceptor/deduplicate.js` | `module.exports` | 14 | 去重拦截器工厂 |
| `lib/interceptor/dump.js` | `createDumpInterceptor()` | 96 | Dump 拦截器工厂 |
| `lib/interceptor/response-error.js` | `module.exports` | 89 | Response-Error 拦截器工厂 |
| `lib/interceptor/response-error.js` | `ResponseErrorHandler.onResponseStart()` | 32 | 状态码判断 + 初始化 decoder |
| `lib/interceptor/response-error.js` | `ResponseErrorHandler.onResponseEnd()` | 54 | 错误转 Error + stackTraceLimit 优化 |

### Handler 层

| 文件 | 函数/类 | 行号 | 用途 |
|------|---------|------|------|
| `lib/handler/decorator-handler.js` | `DecoratorHandler` | 8 | 装饰器基类 |
| `lib/handler/cache-handler.js` | `CacheHandler` | 126 | 缓存写入 Handler |
| `lib/handler/cache-handler.js` | `canCacheResponse()` | 500 | 可缓存性判断 |
| `lib/handler/cache-handler.js` | `determineStaleAt()` | 621 | 新鲜度计算 |
| `lib/handler/cache-handler.js` | `determineDeleteAt()` | 720 | 删除时间计算 |
| `lib/handler/cache-handler.js` | `stripNecessaryHeaders()` | 765 | 缓存 Header 清理 |
| `lib/handler/cache-revalidation-handler.js` | `CacheRevalidationHandler` | 18 | 304 处理 Handler |
| `lib/handler/redirect-handler.js` | `RedirectHandler` | 11 | 重定向 Handler |
| `lib/handler/redirect-handler.js` | `cleanRequestHeaders()` | 207 | Header 清理 |
| `lib/handler/retry-handler.js` | `RetryHandler` | 71 | 重试 Handler |
| `lib/handler/retry-handler.js` | `RetryController` | 48 | Controller 代理 |
| `lib/handler/retry-handler.js` | `RetryHandler[kRetryHandlerDefaultRetry]` | 222 | 默认重试策略 |
| `lib/handler/deduplication-handler.js` | `DeduplicationHandler` | 27 | 去重 Handler |
| `lib/handler/deduplication-handler.js` | `addWaitingHandler()` | 106 | 添加等待 handler |

### Mock 层

| 文件 | 函数/类 | 行号 | 用途 |
|------|---------|------|------|
| `lib/mock/mock-agent.js` | `MockAgent` | 31 | Mock 系统总入口（244 行） |
| `lib/mock/mock-agent.js` | `MockAgent.dispatch()` | 72 | dispatch 路由（origin 标准化 + 历史记录） |
| `lib/mock/mock-agent.js` | `MockAgent.get()` | 58 | origin 标准化 + 模糊匹配 |
| `lib/mock/mock-agent.js` | `MockAgent[kMockAgentGet]()` | 192 | 正则/函数 origin 匹配 |
| `lib/mock/mock-agent.js` | `MockAgent[kFactory]()` | 185 | 创建 MockClient/MockPool |
| `lib/mock/mock-agent.js` | `MockAgent[kMockAgentAddCallHistoryLog]()` | 171 | 记录调用历史 |
| `lib/mock/mock-agent.js` | `MockAgent.enableNetConnect()` | 119 | 启用网络白名单 |
| `lib/mock/mock-agent.js` | `MockAgent.disableNetConnect()` | 133 | 禁止网络连接 |
| `lib/mock/mock-agent.js` | `MockAgent.pendingInterceptors()` | 221 | 获取未消费拦截器 |
| `lib/mock/mock-agent.js` | `MockAgent.assertNoPendingInterceptors()` | 229 | 未消费断言 |
| `lib/mock/mock-client.js` | `MockClient` | 23 | Mock Client（68 行） |
| `lib/mock/mock-client.js` | `MockClient.intercept()` | 50 | 创建 MockInterceptor |
| `lib/mock/mock-client.js` | `MockClient[kClose]()` | 61 | Mock 专用 close |
| `lib/mock/mock-pool.js` | `MockPool` | 23 | Mock Pool（68 行） |
| `lib/mock/mock-interceptor.js` | `MockInterceptor` | 65 | 拦截规则定义（227 行） |
| `lib/mock/mock-interceptor.js` | `MockInterceptor.reply()` | 121 | 定义回复（支持同步/异步回调） |
| `lib/mock/mock-interceptor.js` | `MockInterceptor.replyWithError()` | 184 | 定义错误回复 |
| `lib/mock/mock-interceptor.js` | `MockInterceptor.defaultReplyHeaders()` | 196 | 设置默认回复头 |
| `lib/mock/mock-interceptor.js` | `MockInterceptor.replyContentLength()` | 220 | 自动计算 Content-Length |
| `lib/mock/mock-interceptor.js` | `MockScope` | 25 | 回复行为配置 |
| `lib/mock/mock-interceptor.js` | `MockScope.delay()` | 32 | 延迟回复 |
| `lib/mock/mock-interceptor.js` | `MockScope.persist()` | 44 | 永不过期 |
| `lib/mock/mock-interceptor.js` | `MockScope.times()` | 52 | 使用 N 次后消费 |
| `lib/mock/mock-utils.js` | `matchValue()` | 22 | 值匹配（string/regex/function） |
| `lib/mock/mock-utils.js` | `matchKey()` | 142 | 完整请求匹配 |
| `lib/mock/mock-utils.js` | `matchHeaders()` | 73 | Header 匹配（支持函数） |
| `lib/mock/mock-utils.js` | `getMockDispatch()` | 171 | 匹配 dispatch 查找（逐级过滤） |
| `lib/mock/mock-utils.js` | `addMockDispatch()` | 211 | 注册 mock（含 kTotalDispatchCount） |
| `lib/mock/mock-utils.js` | `deleteMockDispatch()` | 221 | 删除已消费的 dispatch |
| `lib/mock/mock-utils.js` | `buildKey()` | 254 | 构造匹配键 |
| `lib/mock/mock-utils.js` | `getResponseData()` | 150 | 数据类型转换（Buffer/JSON/string） |
| `lib/mock/mock-utils.js` | `normalizeOrigin()` | 672 | Origin 标准化（小写） |
| `lib/mock/mock-utils.js` | `buildAndValidateMockOptions()` | 684 | MockAgent 配置校验 |
| `lib/mock/mock-utils.js` | `mockDispatch()` | 303 | 核心 dispatch 函数 |
| `lib/mock/mock-utils.js` | `dispatchMockReply()` | 358 | 响应模拟（完整生命周期） |
| `lib/mock/mock-utils.js` | `dispatchRequestBody()` | 536 | 请求体处理（4 种类型） |
| `lib/mock/mock-utils.js` | `dispatchAsyncIterableBody()` | 576 | 异步迭代 body |
| `lib/mock/mock-utils.js` | `callOnBodySent()` | 600 | 触发 onBodySent 钩子 |
| `lib/mock/mock-utils.js` | `callOnRequestSent()` | 610 | 触发 onRequestSent 钩子 |
| `lib/mock/mock-utils.js` | `buildMockDispatch()` | 627 | dispatch 替换引擎（含 netConnect 回退） |
| `lib/mock/mock-utils.js` | `checkNetConnect()` | 662 | 网络连接白名单检查 |
| `lib/mock/mock-call-history.js` | `MockCallHistory` | 127 | 调用历史（248 行） |
| `lib/mock/mock-call-history.js` | `MockCallHistoryLog` | 74 | 调用日志条目 |
| `lib/mock/mock-call-history.js` | `MockCallHistory.filterCalls()` | 157 | 多维过滤（支持 OR/AND） |
| `lib/mock/mock-call-history.js` | `MockCallHistory.nthCall()` | 142 | 第 N 次调用（非零基） |
| `lib/mock/mock-call-history.js` | `MockCallHistoryLog.toMap()` | 92 | 转为 Map |
| `lib/mock/mock-call-history.js` | `MockCallHistoryLog.toString()` | 108 | 字符串表示 |
| `lib/mock/mock-errors.js` | `MockNotMatchedError` | 10 | 未匹配错误（29 行） |
| `lib/mock/mock-symbols.js` | 全部 31 个 Symbol | 3-32 | 内部状态隐藏 |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent` | 19 | 快照 Agent（371 行） |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.dispatch()` | 83 | 三模式 dispatch |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.#asyncDispatch()` | 126 | 异步 dispatch（先加载快照） |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.#recordAndReplay()` | 134 | 录制并回放（recordingHandler） |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.#replaySnapshot()` | 196 | 快照回放 |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.#setupMockInterceptors()` | 270 | 回退到 MockAgent 拦截器 |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.loadSnapshots()` | 237 | 加载快照文件 |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.saveSnapshots()` | 253 | 保存快照文件 |
| `lib/mock/snapshot-recorder.js` | `SnapshotRecorder` | 246 | 快照录制器（623 行） |
| `lib/mock/snapshot-recorder.js` | `record()` | 314 | 录制请求（支持顺序响应） |
| `lib/mock/snapshot-recorder.js` | `findSnapshot()` | 384 | 查找快照（顺序响应支持） |
| `lib/mock/snapshot-recorder.js` | `createRequestHash()` | 214 | 请求哈希（SHA-256 base64url） |
| `lib/mock/snapshot-recorder.js` | `formatRequestKey()` | 124 | 请求格式化 |
| `lib/mock/snapshot-recorder.js` | `filterHeadersForMatching()` | 148 | Header 匹配过滤 |
| `lib/mock/snapshot-recorder.js` | `filterHeadersForStorage()` | 185 | Header 存储过滤 |
| `lib/mock/snapshot-recorder.js` | `loadSnapshots()` | 417 | 加载快照文件 |
| `lib/mock/snapshot-recorder.js` | `saveSnapshots()` | 453 | 保存快照文件（JSON 格式） |
| `lib/mock/snapshot-recorder.js` | `#scheduleFlush()` | 583 | 防抖自动刷新 |
| `lib/mock/snapshot-utils.js` | `hashId()` | 43 | SHA-256/base64url 哈希 |
| `lib/mock/snapshot-utils.js` | `normalizeHeaders()` | 104 | Header 归一化（支持数组/对象） |
| `lib/mock/snapshot-utils.js` | `isUrlExcludedFactory()` | 68 | URL 排除工厂 |
| `lib/mock/snapshot-utils.js` | `createHeaderFilters()` | 19 | 创建 Header 过滤缓存 |
| `lib/mock/snapshot-utils.js` | `validateSnapshotMode()` | 145 | 快照模式校验 |
| `lib/mock/pending-interceptors-formatter.js` | `PendingInterceptorsFormatter` | 12 | 格式化输出（43 行） |

### 工具层

| 文件 | 函数/类 | 行号 | 用途 |
|------|---------|------|------|
| `lib/util/cache.js` | `parseCacheControlHeader()` | 314 | Cache-Control 解析 |
| `lib/util/cache.js` | `makeCacheKey()` | 150 | 缓存键生成 |
| `lib/util/cache.js` | `normalizeHeaders()` | 185 | Header 归一化 |
| `lib/util/cache.js` | `makeDeduplicationKey()` | 681 | 去重键生成 |
| `lib/util/cache.js` | `isEtagUsable()` | 614 | ETag 验证 |
| `lib/util/cache.js` | `parseVaryHeader()` | 570 | Vary 解析 |
| `lib/util/date.js` | `parseHttpDate()` | 9 | HTTP 日期解析 |
| `lib/util/date.js` | `parseImfDate()` | 47 | IMF-fixdate 解析 |
| `lib/util/date.js` | `parseAscTimeDate()` | 256 | asctime 解析 |
| `lib/util/date.js` | `parseRfc850Date()` | 460 | RFC 850 解析 |
| `lib/util/timers.js` | `FastTimer` | 213 | 快速定时器 |
| `lib/util/timers.js` | `onTick()` | 115 | Tick 处理 |
| `lib/util/timers.js` | `setTimeout()` | 338 | 智能 setTimeout |
| `lib/util/runtime-features.js` | `RuntimeFeatures` | 47 | 运行时特性检测 |
| `lib/util/stats.js` | `ClientStats` / `PoolStats` | 13/22 | 连接统计 |

---

> 本文档基于 undici 源码逐文件阅读分析，共覆盖 31 个源文件、约 8,867 行代码。
> 所有函数名、行号、代码路径均来自实际源码。
