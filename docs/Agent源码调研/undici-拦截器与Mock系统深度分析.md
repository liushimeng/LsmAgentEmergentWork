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
8. [跨模块设计模式总结](#8-跨模块设计模式总结)
9. [对 AI Agent HTTP 测试的借鉴](#9-对-ai-agent-http-测试的借鉴)
10. [laew 借鉴路线图](#10-laew-借鉴路线图)

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
  // 双栈模式：交替选择 IPv4/IPv6
  if (this.dualStack) {
    if (affinity == null) {
      affinity = (hostnameRecords.offset & 1) === 1 ? 6 : 4
    }
    // fallback 到另一个 family
    if (records[affinity]?.ips.length === 0) {
      family = records[affinity === 4 ? 6 : 4]
    }
  }
  // 轮询选择 IP
  const position = family.offset % family.ips.length
  // TTL 过期自动移除
  if (Date.now() - ip.timestamp > ip.ttl) {
    family.ips.splice(position, 1)
    return this.pick(origin, hostnameRecords, affinity)  // 递归
  }
}
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

**文件**：`lib/handler/decorator-handler.js`（72 行）

所有 Handler 的基础装饰器，提供生命周期状态机保护：

```javascript
module.exports = class DecoratorHandler {
  #handler
  #onCompleteCalled = false
  #onErrorCalled = false
  #onResponseStartCalled = false

  onRequestStart (...args) { this.#handler.onRequestStart?.(...args) }
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
    this.#onCompleteCalled = true
    return this.#handler.onResponseEnd?.(...args)
  }
  onResponseError (...args) {
    this.#onErrorCalled = true
    return this.#handler.onResponseError?.(...args)
  }
}
```

**设计要点**：
- 用 `assert` 做运行时状态机校验（开发阶段捕获协议违规）
- 可选链 `?.()` 保护（handler 不一定实现所有回调）
- 已标记 `@deprecated`，推荐新代码直接实现 `DispatchHandler` 接口

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
  // 301/302 + POST → GET
  if ((statusCode === 301 || statusCode === 302) && this.opts.method === 'POST') {
    this.opts.method = 'GET'
    this.opts.body = null
    removeContentHeaders = true
  }
  // 303 → GET (HEAD 除外)
  if (statusCode === 303 && this.opts.method !== 'HEAD') {
    this.opts.method = 'GET'
    this.opts.body = null
  }
  // 重定向循环检测
  for (const historyUrl of this.history) {
    if (historyUrl.toString() === redirectUrlString) {
      throw new InvalidArgumentError('Redirect loop detected')
    }
  }
}
```

#### 4.4.2 Header 清理

`cleanRequestHeaders()` 实现了 RFC 7231 的 header 清理规则：

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

### 4.5 RetryHandler

**文件**：`lib/handler/retry-handler.js`（548 行）

最复杂的 Handler 之一，实现了可恢复的请求重试。

#### 4.5.1 RetryController 代理

```javascript
class RetryController {
  #onAbort
  target = null  // 指向当前活跃连接的 controller

  pause () { this.target?.pause() }
  resume () { this.target?.resume() }
  abort (reason) {
    this.target?.abort(reason)
    this.#onAbort(reason)  // 通知 handler 取消 backoff
  }
}
```

**设计要点**：每次透明重试都是新的 dispatch（新的 controller），但下游 handler 通过 `controllerProxy` 始终操作当前活跃连接。

#### 4.5.2 默认重试策略

```javascript
static [kRetryHandlerDefaultRetry] (err, { state, opts }, cb) {
  // 方法过滤：默认只重试 GET/HEAD/OPTIONS/PUT/DELETE/TRACE/QUERY
  // 状态码过滤：默认 [500, 502, 503, 504, 429]
  // 错误码过滤：ECONNRESET, ECONNREFUSED, ENOTFOUND, etc.
  // 最大重试：默认 5 次

  // 指数退避：min(500ms * 2^(n-1), 30s)
  // 尊重 Retry-After header
  const retryTimeout = retryAfterHeader > 0
    ? Math.min(retryAfterHeader, maxTimeout)
    : Math.min(minTimeout * timeoutFactor ** (counter - 1), maxTimeout)

  return setTimeout(() => cb(null), retryTimeout)
}
```

#### 4.5.3 Range 请求续传

当请求体已部分消费（`headersSent = true`），重试使用 Range 请求续传：

```javascript
retry () {
  if (this.start !== 0) {
    const headers = { range: `bytes=${this.start}-${this.end ?? ''}` }
    // 强 ETag 验证
    if (this.etag != null) {
      headers['if-match'] = this.etag
    }
    this.opts = { ...this.opts, headers: { ...this.opts.headers, ...headers } }
  }
  this.dispatch(this.opts, this)
}
```

#### 4.5.4 Abort 传播

```javascript
#onAbort (reason) {
  if (!this.retryPending) return
  this.aborted = true
  this.retryPending = false
  clearTimeout(this.retryTimer)  // 取消 backoff 定时器
  this.handler.onResponseError?.(this.controllerProxy, reason ?? new RequestAbortedError())
}
```

### 4.6 DeduplicationHandler

**文件**：`lib/handler/deduplication-handler.js`（466 行）

请求去重的 Handler 实现，管理多个等待中的 handler。

#### 4.6.1 WaitingHandler 结构

```javascript
const waitingHandler = {
  handler,              // 下游 handler
  controller,           // 独立 controller（每个 waiting handler 有自己的流控状态）
  bufferedChunks: [],   // 暂停时缓冲的数据
  bufferedBytes: 0,     // 缓冲字节数
  pendingTrailers: null, // 暂停时缓存的 trailers
  done: false           // 是否已完成
}
```

#### 4.6.2 流控机制

每个 waiting handler 有独立的 controller，支持独立的 pause/resume：

```javascript
controller = {
  resume: () => {
    state.paused = false
    this.#flushWaitingHandler(waitingHandler)  // 刷出缓冲数据
    // 如果主响应已完成且缓冲区空，发送 trailers
    if (this.#completed && waitingHandler.pendingTrailers && ...) {
      handler.onResponseEnd?.(controller, waitingHandler.pendingTrailers)
    }
  },
  pause: () => { state.paused = true },
  abort: (reason) => {
    state.aborted = true
    handler.onResponseError?.(controller, reason ?? new RequestAbortedError())
  }
}
```

#### 4.6.3 安全上限

`#bufferWaitingChunk()` 中有 `maxBufferSize`（默认 5MB）保护：

```javascript
if (waitingHandler.bufferedBytes > this.#maxBufferSize) {
  const err = new RequestAbortedError(`Deduplicated waiting handler exceeded maxBufferSize`)
  this.#errorWaitingHandler(waitingHandler, err)
}
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
    this[kNetConnect] = [...(this[kNetConnect] || []), matcher]
  } else if (matcher === undefined) {
    this[kNetConnect] = true
  }
}

disableNetConnect () {
  this[kNetConnect] = false
}
```

#### 5.1.5 未消费拦截器断言

```javascript
assertNoPendingInterceptors () {
  const pending = this.pendingInterceptors()
  if (pending.length === 0) return
  throw new UndiciError(
    `${pending.length} interceptors are pending:\n\n${formatter.format(pending)}`
  )
}
```

使用 `PendingInterceptorsFormatter`（43 行）生成 `console.table` 格式的报告。

### 5.2 MockClient / MockPool

**文件**：`lib/mock/mock-client.js`（68 行）、`lib/mock/mock-pool.js`（68 行）

两者几乎完全相同，分别继承 `Client` 和 `Pool`：

```javascript
class MockClient extends Client {
  constructor (origin, opts) {
    super(origin, opts)
    this[kDispatches] = []                      // 拦截规则列表
    this[kOriginalDispatch] = this.dispatch     // 保存原始 dispatch
    this.dispatch = buildMockDispatch.call(this) // 替换为 mock dispatch
  }

  intercept (opts) {
    return new MockInterceptor(opts, this[kDispatches])
  }

  cleanMocks () {
    this[kDispatches] = []
  }
}
```

**核心替换**：构造函数中将 `this.dispatch` 替换为 `buildMockDispatch()` 生成的 mock dispatch。

### 5.3 MockInterceptor / MockScope

**文件**：`lib/mock/mock-interceptor.js`（227 行）

#### 5.3.1 MockInterceptor（拦截规则定义）

```javascript
class MockInterceptor {
  constructor (opts, mockDispatches) {
    // path 必须定义，method 默认 GET
    // query 参数合并到 path
    this[kDispatchKey] = buildKey(opts)       // { path, method, body, headers, query }
    this[kDispatches] = mockDispatches
  }

  reply (replyOptionsCallbackOrStatusCode) {
    // 支持静态值和回调函数两种形式
    if (typeof replyOptionsCallbackOrStatusCode === 'function') {
      // 回调式：支持异步回调
      const wrappedCallback = (opts) => {
        const resolvedData = replyOptionsCallbackOrStatusCode(opts)
        if (isPromise(resolvedData)) {
          return resolvedData.then(resolveReplyCallbackData)
        }
        return resolveReplyCallbackData(resolvedData)
      }
      const newMockDispatch = addMockDispatch(this[kDispatches], this[kDispatchKey], wrappedCallback)
      return new MockScope(newMockDispatch)
    }
    // 静态值式
    const dispatchData = this.createMockScopeDispatchData(replyParameters)
    const newMockDispatch = addMockDispatch(this[kDispatches], this[kDispatchKey], dispatchData)
    return new MockScope(newMockDispatch)
  }

  replyWithError (error) {
    const newMockDispatch = addMockDispatch(this[kDispatches], this[kDispatchKey], { error })
    return new MockScope(newMockDispatch)
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
  .reply(200, { users: [] }, { headers: { 'content-type': 'application/json' } })
  .delay(100)   // 延迟 100ms
  .times(3)     // 前 3 次使用
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

  mockDispatch.timesInvoked++
  mockDispatch.consumed = !mockDispatch.persist && timesInvoked >= times
  mockDispatch.pending = timesInvoked < times

  // 回调式回复
  if (mockDispatch.data.callback) {
    const callbackResult = mockDispatch.data.callback(opts)
    if (isPromise(callbackResult)) {
      callbackResult.then(resolved => dispatchMockReply(...), err => handler.onResponseError(null, err))
      return true
    }
    return dispatchMockReply(...)
  }

  return dispatchMockReply(...)
}
```

#### 5.4.5 响应模拟

`dispatchMockReply()`（第 358 行）模拟完整的 HTTP 响应生命周期：

```javascript
function dispatchMockReply (mockDispatches, mockDispatch, key, opts, handler, resolvedResponse) {
  const response = resolvedResponse ?? mockDispatch.data

  // 错误模拟
  if (response.error !== null) {
    deleteMockDispatch(mockDispatches, key)
    handler.onResponseError(null, response.error)
    return true
  }

  // 创建 controller
  const controller = { paused: false, pause() {}, resume() {}, abort(reason) { ... } }

  // Request 生命周期
  handler.onRequestStart?.(controller, null)
  dispatchRequestBody(opts.body, handler, controller)  // 处理请求体

  function sendReply () {
    // 延迟回复
    if (delay > 0) {
      timer = setTimeout(() => handleReply(), delay)
    } else {
      handleReply()
    }
  }

  function handleReply () {
    const responseData = getResponseData(body)
    const responseHeaders = generateKeyValues(headers)
    const responseTrailers = generateKeyValues(trailers)

    controller.rawHeaders = responseHeaders
    controller.rawTrailers = responseTrailers

    handler.onResponseStart?.(controller, statusCode, parseHeaders(responseHeaders), getStatusText(statusCode))
    handler.onResponseData?.(controller, Buffer.from(responseData))
    handler.onResponseEnd?.(controller, parseHeaders(responseTrailers))
    deleteMockDispatch(mockDispatches, key)
  }
}
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
          // 未匹配的请求：检查网络连接权限
          const netConnect = agent[kGetNetConnect]()
          if (netConnect === false) {
            throw new MockNotMatchedError('net.connect disabled')
          }
          if (checkNetConnect(netConnect, origin)) {
            // 允许真实网络请求
            originalDispatch.call(this, opts, handler)
          } else {
            throw new MockNotMatchedError('net.connect not enabled for origin')
          }
        }
      }
    } else {
      originalDispatch.call(this, opts, handler)  // mock 未激活，走真实路径
    }
  }
}
```

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

### 5.6 MockErrors / MockSymbols

**文件**：`lib/mock/mock-errors.js`（29 行）、`lib/mock/mock-symbols.js`（32 行）

#### 5.6.1 MockNotMatchedError

```javascript
class MockNotMatchedError extends UndiciError {
  constructor (message) {
    super(message)
    this.code = 'UND_MOCK_ERR_MOCK_NOT_MATCHED'
  }
  // Symbol.hasInstance 支持：instanceof 检查
  static [Symbol.hasInstance] (instance) {
    return instance && instance[kMockNotMatchedError] === true
  }
}
```

#### 5.6.3 MockSymbols

定义了 31 个 Symbol，用于隐藏内部状态：

```javascript
module.exports = {
  kAgent: Symbol('agent'),
  kOptions: Symbol('options'),
  kFactory: Symbol('factory'),
  kDispatches: Symbol('dispatches'),
  kDispatchKey: Symbol('dispatch key'),
  kDefaultHeaders: Symbol('default headers'),
  kDefaultTrailers: Symbol('default trailers'),
  kContentLength: Symbol('content length'),
  kMockAgent: Symbol('mock agent'),
  kMockAgentSet: Symbol('mock agent set'),
  kMockAgentGet: Symbol('mock agent get'),
  kMockDispatch: Symbol('mock dispatch'),
  kClose: Symbol('close'),
  kOriginalClose: Symbol('original agent close'),
  kOriginalDispatch: Symbol('original dispatch'),
  kOrigin: Symbol('origin'),
  kIsMockActive: Symbol('is mock active'),
  kNetConnect: Symbol('net connect'),
  kGetNetConnect: Symbol('get net connect'),
  kConnected: Symbol('connected'),
  kIgnoreTrailingSlash: Symbol('ignore trailing slash'),
  kMockAgentMockCallHistoryInstance: Symbol('mock agent mock call history name'),
  kMockAgentRegisterCallHistory: Symbol('mock agent register mock call history'),
  kMockAgentAddCallHistoryLog: Symbol('mock agent add call history log'),
  kMockAgentIsCallHistoryEnabled: Symbol('mock agent is call history enabled'),
  kMockAgentAcceptsNonStandardSearchParameters: Symbol('mock agent accepts non standard search parameters'),
  kMockCallHistoryAddLog: Symbol('mock call history add log'),
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

  const recordingHandler = {
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
      self[kSnapshotRecorder].record(opts, {
        statusCode: responseData.statusCode,
        headers: responseData.headers,
        body: Buffer.concat(responseData.body),
        trailers: responseData.trailers
      }).then(() => handler.onResponseEnd(controller, trailers))
    }
  }

  return agent.dispatch(opts, recordingHandler)
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
  const headerKeys = Object.keys(formattedRequest.headers).sort()
  for (const key of headerKeys) {
    parts.push(key)
    for (const value of values.sort()) {
      parts.push(String(value))
    }
  }

  parts.push(formattedRequest.body)
  return hashId(parts.join('|'))  // SHA-256 或 base64url
}
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

#### 6.2.5 Header 过滤

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

## 8. 跨模块设计模式总结

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
| `lib/interceptor/cache.js` | `sendCachedValue()` | 293 | 发送缓存响应 |
| `lib/interceptor/cache.js` | `isStale()` | 180 | 过期判断 |
| `lib/interceptor/cache.js` | `needsRevalidation()` | 80 | 验证需求判断 |
| `lib/interceptor/cache.js` | `withinStaleWhileRevalidateWindow()` | 214 | SWR 窗口判断 |
| `lib/interceptor/cache.js` | `makeRevalidationHeaders()` | 153 | 条件请求头构造 |
| `lib/interceptor/retry.js` | `module.exports` | 4 | Retry 拦截器工厂 |
| `lib/interceptor/redirect.js` | `createRedirectInterceptor()` | 5 | Redirect 拦截器工厂 |
| `lib/interceptor/dns.js` | `module.exports` | 458 | DNS 拦截器工厂 |
| `lib/interceptor/dns.js` | `DNSInstance.runLookup()` | 156 | DNS 查询入口 |
| `lib/interceptor/dns.js` | `DNSDispatchHandler.onResponseError()` | 404 | 双栈故障转移 |
| `lib/interceptor/decompress.js` | `createDecompressInterceptor()` | 270 | 解压拦截器工厂 |
| `lib/interceptor/decompress.js` | `DecompressHandler.#createDecompressionChain()` | 68 | 多级解压链创建 |
| `lib/interceptor/deduplicate.js` | `module.exports` | 14 | 去重拦截器工厂 |
| `lib/interceptor/dump.js` | `createDumpInterceptor()` | 96 | Dump 拦截器工厂 |
| `lib/interceptor/response-error.js` | `module.exports` | 89 | Response-Error 拦截器工厂 |

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
| `lib/mock/mock-agent.js` | `MockAgent` | 31 | Mock 系统总入口 |
| `lib/mock/mock-agent.js` | `MockAgent.dispatch()` | 72 | dispatch 路由 |
| `lib/mock/mock-agent.js` | `MockAgent.assertNoPendingInterceptors()` | 229 | 未消费断言 |
| `lib/mock/mock-client.js` | `MockClient` | 23 | Mock Client |
| `lib/mock/mock-pool.js` | `MockPool` | 23 | Mock Pool |
| `lib/mock/mock-interceptor.js` | `MockInterceptor` | 65 | 拦截规则定义 |
| `lib/mock/mock-interceptor.js` | `MockInterceptor.reply()` | 121 | 定义回复 |
| `lib/mock/mock-interceptor.js` | `MockScope` | 25 | 回复行为配置 |
| `lib/mock/mock-utils.js` | `matchValue()` | 22 | 值匹配（string/regex/function） |
| `lib/mock/mock-utils.js` | `matchKey()` | 142 | 完整请求匹配 |
| `lib/mock/mock-utils.js` | `getMockDispatch()` | 171 | 匹配 dispatch 查找 |
| `lib/mock/mock-utils.js` | `addMockDispatch()` | 211 | 注册 mock |
| `lib/mock/mock-utils.js` | `mockDispatch()` | 303 | 核心 dispatch 函数 |
| `lib/mock/mock-utils.js` | `dispatchMockReply()` | 358 | 响应模拟 |
| `lib/mock/mock-utils.js` | `dispatchRequestBody()` | 536 | 请求体处理 |
| `lib/mock/mock-utils.js` | `buildMockDispatch()` | 627 | dispatch 替换引擎 |
| `lib/mock/mock-call-history.js` | `MockCallHistory` | 127 | 调用历史 |
| `lib/mock/mock-call-history.js` | `MockCallHistoryLog` | 74 | 调用日志条目 |
| `lib/mock/mock-call-history.js` | `filterCalls()` | 157 | 多维过滤 |
| `lib/mock/mock-errors.js` | `MockNotMatchedError` | 10 | 未匹配错误 |
| `lib/mock/mock-symbols.js` | 全部 31 个 Symbol | 3-32 | 内部状态隐藏 |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent` | 19 | 快照 Agent |
| `lib/mock/snapshot-agent.js` | `SnapshotAgent.dispatch()` | 83 | 三模式 dispatch |
| `lib/mock/snapshot-agent.js` | `#recordAndReplay()` | 134 | 录制并回放 |
| `lib/mock/snapshot-agent.js` | `#replaySnapshot()` | 196 | 快照回放 |
| `lib/mock/snapshot-recorder.js` | `SnapshotRecorder` | 246 | 快照录制器 |
| `lib/mock/snapshot-recorder.js` | `record()` | 314 | 录制请求 |
| `lib/mock/snapshot-recorder.js` | `findSnapshot()` | 384 | 查找快照 |
| `lib/mock/snapshot-recorder.js` | `createRequestHash()` | 214 | 请求哈希 |
| `lib/mock/snapshot-recorder.js` | `formatRequestKey()` | 124 | 请求格式化 |
| `lib/mock/snapshot-utils.js` | `hashId()` | 43 | SHA-256/base64url 哈希 |
| `lib/mock/snapshot-utils.js` | `normalizeHeaders()` | 104 | Header 归一化 |
| `lib/mock/snapshot-utils.js` | `isUrlExcludedFactory()` | 68 | URL 排除工厂 |
| `lib/mock/pending-interceptors-formatter.js` | `PendingInterceptorsFormatter` | 12 | 格式化输出 |

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
