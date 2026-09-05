# undici 核心机制深度分析

> 项目：undici（https://github.com/nodejs/undici）
> 定位：Node.js 官方 HTTP/1.1 + HTTP/2 客户端库，Node.js 内置 `fetch()` 的底层实现
> 语言：JavaScript（Node.js）
> 源码规模：109 个 JS 文件，~22,079 行
> 分析日期：2026-09-05

---

## 目录

1. [错误处理与容错机制](#1-错误处理与容错机制)
2. [连接池与并发控制](#2-连接池与并发控制)
3. [性能优化设计](#3-性能优化设计)
4. [HTTP/1.1 与 HTTP/2 双协议实现](#4-http11-与-http2-双协议实现)
5. [流式 Body 传输与背压](#5-流式-body-传输与背压)
6. [拦截器体系](#6-拦截器体系)
7. [缓存系统](#7-缓存系统)
8. [诊断与可观测性](#8-诊断与可观测性)
9. [代理体系](#9-代理体系)
10. [Mock 测试系统](#10-mock-测试系统)
11. [Web 标准实现](#11-web-标准实现)
12. [跨模块设计模式总结](#12-跨模块设计模式总结)
13. [对 laew 的借鉴价值与路线图](#13-对-laew-的借鉴价值与路线图)

---

## 1. 错误处理与容错机制

### 1.1 设计概述

undici 构建了一套层次分明、覆盖全面的错误体系，共 26 个错误类，全部继承自 `UndiciError` 基类。核心设计亮点是 `Symbol.hasInstance` 防伪模式：每个错误类都通过全局唯一的 `Symbol.for()` 键绑定实例标记，使 `instanceof` 检查不依赖原型链，即使跨 Realm 传递或序列化/反序列化也能正确识别。

超时体系覆盖连接建立、头部解析、Body 传输、Keep-Alive 空闲四个阶段。headers/body 超时使用 FastTimer（自研低精度定时器，~500ms 分辨率），keep-alive 超时使用原生定时器。这种区分是刻意为之——headers/body 超时需要感知事件循环延迟（当事件循环阻塞时 FastTimer 内部时钟不前进，避免误杀正常连接），keep-alive 超时需要精确墙钟时间。

### 1.2 错误类层次结构

```
UndiciError (基类, lib/core/errors.js)
  ├── ConnectTimeoutError          (UND_ERR_CONNECT_TIMEOUT)
  ├── HeadersTimeoutError          (UND_ERR_HEADERS_TIMEOUT)
  ├── HeadersOverflowError         (UND_ERR_HEADERS_OVERFLOW)
  ├── BodyTimeoutError             (UND_ERR_BODY_TIMEOUT)
  ├── InvalidArgumentError         (UND_ERR_INVALID_ARG)
  ├── InvalidReturnValueError      (UND_ERR_INVALID_RETURN_VALUE)
  ├── AbortError                   (UND_ERR_ABORT)
  │   └── RequestAbortedError      (UND_ERR_ABORTED)
  ├── InformationalError           (UND_ERR_INFO)        -- 非致命信息性错误
  ├── RequestContentLengthMismatchError
  ├── ResponseContentLengthMismatchError
  ├── ClientDestroyedError         (UND_ERR_DESTROYED)
  ├── ClientClosedError            (UND_ERR_CLOSED)
  ├── SocketError                  (UND_ERR_SOCKET)
  ├── NotSupportedError            (UND_ERR_NOT_SUPPORTED)
  ├── BalancedPoolMissingUpstreamError
  ├── ResponseExceededMaxSizeError
  ├── RequestRetryError
  ├── ResponseError
  ├── SecureProxyConnectionError   (UND_ERR_PRX_TLS)
  ├── ProxyConnectionError         (UND_ERR_PRX_CONN)
  ├── MaxOriginsReachedError
  ├── Socks5ProxyError
  ├── MessageSizeExceededError     (UND_ERR_WS_MESSAGE_SIZE_EXCEEDED)
  └── HTTPParserError              (继承自 Error, 非 UndiciError)
```

### 1.3 Symbol.hasInstance 防伪模式

`lib/core/errors.js:4-18`：

```javascript
const kUndiciError = Symbol.for('undici.error.UND_ERR')
class UndiciError extends Error {
  constructor (message, options) {
    super(message, options)
    this.name = 'UndiciError'
    this.code = 'UND_ERR'
  }
  static [Symbol.hasInstance] (instance) {
    return instance && instance[kUndiciError] === true
  }
  get [kUndiciError] () {
    return true
  }
}
```

每个子类使用 `Symbol.for('undici.error.XXX')` 全局符号，配合 getter 模式，即使字面量对象也能被正确识别。解决了跨模块/跨版本/跨 bundler 的错误识别问题。

### 1.4 超时体系

**位掩码超时分类**（`lib/dispatcher/client-h1.js:198-208`）：

```javascript
const USE_NATIVE_TIMER = 0
const USE_FAST_TIMER = 1

const TIMEOUT_HEADERS = 2 | USE_FAST_TIMER     // = 3, fast timer
const TIMEOUT_BODY = 4 | USE_FAST_TIMER        // = 5, fast timer
const TIMEOUT_KEEP_ALIVE = 8 | USE_NATIVE_TIMER // = 8, native timer
```

`type & USE_FAST_TIMER` 快速判断是否使用 FastTimer。`Parser.setTimeout()`（`lib/dispatcher/client-h1.js:246`）根据类型选择定时器实现。

**超时处理函数**：
- `onParserTimeout()` — `lib/dispatcher/client-h1.js:833`
- `setupConnectTimeout()` — `lib/core/util.js:863`
- `setNoStreamsTimeout()` — `lib/dispatcher/client-h2.js:446`

### 1.5 GOAWAY 恢复与重放预算

`lib/dispatcher/client-h2.js:58-205`：

```javascript
const MAX_GOAWAY_REPLAY_ATTEMPTS = 1

function registerGoAwayRefusal (request) {
  const attempts = (request[kGoAwayReplayAttempts] ?? 0) + 1
  request[kGoAwayReplayAttempts] = attempts
  return attempts <= MAX_GOAWAY_REPLAY_ATTEMPTS
}
```

`canReplayRequest` 检查 body 可重放性（null/Buffer/Blob），REFUSED_STREAM 使用独立的 `kRefusedStreamRetry` 布尔标记，不消耗 GOAWAY 重放预算。session 级和 request 级双重防护。

**GOAWAY 两阶段分离**（`onHttp2SessionGoAway`）：先 `detachRequestStreamForClose()` 分离请求（解绑 close 监听器），再 `closeStream()` 关闭流，避免"关闭流 A → frameError on 流 B → 误杀"竞态。

### 1.6 Idle Socket Validation

`lib/dispatcher/client-h1.js:1062-1081`：

```javascript
function scheduleIdleSocketValidation (client, socket) {
  socket[kIdleSocketValidation] = 1
  // ref'd setImmediate — 既保持 pending 请求存活，又让 poll 立即返回
  socket[kIdleSocketValidationTimeout] = setImmediate(() => {
    socket[kIdleSocketValidationTimeout] = null
    socket[kIdleSocketValidation] = 2
    if (client[kSocket] === socket && !socket.destroyed) {
      client[kResume]()
    }
  })
}
```

三态（0=未验证, 1=验证中, 2=验证完成），在 keep-alive 复用前先 `readMore()` 探测 socket 状态。GHSA-35p6-xmwp-9g52 安全修复。

### 1.7 InformationalError 非致命设计

`code === 'UND_ERR_INFO'` 的错误在 `onError` 中被特殊跳过（`lib/dispatcher/client.js:473`），不触发队列清理。"reset"/"upgrade"/"socket idle timeout" 等正常生命周期事件不会误杀等待中的请求。

---

## 2. 连接池与并发控制

### 2.1 继承树

```
Dispatcher (抽象基类, lib/dispatcher/dispatcher.js)
  └── DispatcherBase (状态机 + close/destroy, lib/dispatcher/dispatcher-base.js)
        ├── Client (单连接 H1/H2, lib/dispatcher/client.js)
        ├── Pool (多连接池, lib/dispatcher/pool.js)
        ├── PoolBase (池基类, lib/dispatcher/pool-base.js)
        │     ├── BalancedPool (加权负载均衡)
        │     └── RoundRobinPool (轮询负载均衡)
        └── Agent (多 origin 路由, lib/dispatcher/agent.js)
```

### 2.2 Client 三段式队列

`lib/dispatcher/client.js:339-354`：

```javascript
// |   complete   |   running   |   pending   |
//                ^ kRunningIdx ^ kPendingIdx ^ kQueue.length
// 摊还时间复杂度 O(1)，已完成区域超 256 时批量回收
this[kQueue] = []
this[kRunningIdx] = 0
this[kPendingIdx] = 0
```

**协议感知并发上限**（`lib/dispatcher/client.js:92-97`）：

```javascript
function getMaxConcurrent (client) {
  if (client[kHTTPContext]?.version === 'h2') {
    return client[kMaxConcurrentStreams]  // 由服务器 SETTINGS 确认
  }
  return getPipelining(client)
}
```

**H2 busy 信号抑制**：`kPending > 0` 对 H2 多路复用被抑制，避免不必要连接创建。

### 2.3 PoolBase 队列调度

`lib/dispatcher/pool-base.js:30-49`：

```javascript
[kOnDrain] (client, origin, targets) {
  const queue = this[kQueue]
  let needDrain = false
  while (!needDrain) {
    const item = queue.shift()
    if (!item) break
    this[kQueued]--
    needDrain = !client.dispatch(item.opts, item.handler)
  }
  client[kNeedDrain] = needDrain
  if (!needDrain && this[kNeedDrain]) {
    this[kNeedDrain] = false
    this.emit('drain', origin, [this, ...targets])  // 级联传播
  }
}
```

### 2.4 BalancedPool 加权轮询

`lib/dispatcher/balanced-pool.js` 使用经典 GCD 算法实现平滑加权轮询：

- `getGreatestCommonDivisor()` — 欧几里得 GCD
- `errorPenalty`（默认 15）— 连接错误时降权
- `maxWeightPerServer`（默认 100）— 最大权重上限
- `addUpstream/removeUpstream` — 动态管理后端

`addUpstream` 时注册 `connect/connectionError/disconnect` 事件自动调整权重。

### 2.5 Agent 多 origin 路由

`lib/dispatcher/agent.js:82-84`：

```javascript
const allowH2 = opts.allowH2 ?? this[kOptions].allowH2
const key = allowH2 === false ? `${origin}#http1-only` : origin
```

HTTP/1.1 only 请求使用独立 key，避免与 H2 dispatcher 混用。`closeClientIfUnused` 在 disconnect/connectionError 时自动清理空闲 Pool，检查 `kConnected > 0 || kBusy || kPending > 0` 防止 GOAWAY 重连中误删。

### 2.6 FixedQueue 高性能队列

`lib/dispatcher/fixed-queue.js`：单链表循环缓冲区，每个节点 2048 槽位（V8 测试最佳值，必须为 2 的幂）。空槽位用 `undefined` 标记，shift 时置 `undefined` 允许 GC。

---

## 3. 性能优化设计

### 3.1 llhttp WASM 解析器

`lib/dispatcher/client-h1.js:68-182`：

**SIMD 自动降级**：先尝试 SIMD 版 WASM，失败后静默降级到标准版。ppc64 自动禁用。环境变量 `UNDICI_NO_WASM_SIMD` 手动控制。

**JS-WASM 桥零拷贝回调**（第 97-182 行）：

```javascript
wasm_on_status: (p, at, len) => {
  assert(currentParser.ptr === p)
  const start = at - currentBufferPtr + currentBufferRef.byteOffset
  return currentParser.onStatus(new FastBuffer(currentBufferRef.buffer, start, len))
},
```

全局模块变量（`currentParser`/`currentBufferRef`/`currentBufferPtr`/`currentBuffer`）消除回调参数传递开销。

**4096 字节对齐分配**（第 315-339 行）：

```javascript
currentBufferSize = Math.ceil(chunk.length / 4096) * 4096
currentBufferPtr = llhttp.malloc(currentBufferSize)
```

缓存 `currentBuffer` 避免重复创建 `Uint8Array` 视图。`FastBuffer = Buffer[Symbol.species]` 即 `Uint8Array`，不做 zero-fill。

### 3.2 TernarySearchTree 头部查找

`lib/core/tree.js:91-121`：

```javascript
search (key) {
  let node = this
  while (node !== null && index < keylength) {
    let code = key[index]
    if (code <= 0x5a && code >= 0x41) {
      code |= 32  // 大写转小写，单条位操作
    }
    // ... 三叉树遍历
  }
}
```

直接在 `Uint8Array` 上操作，跳过 UTF-8 解码。`code |= 32` 利用 ASCII 编码特性：A-Z(0x41-0x5A) 与 a-z(0x61-0x7A) 差异仅在第 5 位。

### 3.3 FastTimer 低精度定时器

`lib/util/timers.js:115-188`：

```javascript
function onTick () {
  fastNow += TICK_MS  // 自增时钟，不依赖系统时间
  let idx = 0, len = fastTimers.length
  while (idx < len) {
    const timer = fastTimers[idx]
    if (timer._state === PENDING) {
      timer._idleStart = fastNow - TICK_MS
      timer._state = ACTIVE
    } else if (timer._state === ACTIVE && fastNow >= timer._idleStart + timer._idleTimeout) {
      timer._state = TO_BE_CLEARED
      timer._onTimeout(timer._timerArg)
    }
    if (timer._state === TO_BE_CLEARED) {
      timer._state = NOT_IN_LIST
      if (--len !== 0) fastTimers[idx] = fastTimers[len]  // 尾部替换法 O(1) 删除
    } else { ++idx }
  }
  fastTimers.length = len
}
```

4 种状态机：NOT_IN_LIST(-2) / TO_BE_CLEARED(-1) / PENDING(0) / ACTIVE(1)。`RESOLUTION_MS=1000`, `TICK_MS=499`。完全不受系统时钟跳变和 NTP 调整影响。

### 3.4 其他性能优化

- **URL 协议缓存**（`lib/core/util.js:927-946`）：`getProtocolFromUrlString` 单条目缓存，"最近一次"命中率极高
- **null 原型防污染**（`lib/core/util.js:1001-1002`）：`Object.setPrototypeOf(normalizedMethodRecords, null)`
- **socket.cork/uncork 批量写入**：将 HTTP 头部和首个 body 合并为单个 TCP 段
- **`#shouldSkipDecompression` 提前短路**：在 decompress 拦截器中快速跳过不需解压的响应

---

## 4. HTTP/1.1 与 HTTP/2 双协议实现

### 4.1 统一接口

Client 通过 ALPN 协商自动选择协议，两条路径返回相同的接口对象：

```javascript
// lib/dispatcher/client.js:554-556
client[kHTTPContext] = socket.alpnProtocol === 'h2'
  ? connectH2(client, socket)
  : connectH1(client, socket)
```

接口：`{ version, write, resume, destroy, destroyed, busy }`，Client 的 resume 循环完全不感知协议差异。

### 4.2 HTTP/1.1 Parser 状态机

`lib/dispatcher/client-h1.js` 的 8 回调：

1. `onStatus(buf)` — 解析状态文本
2. `onMessageBegin()` — 消息开始，验证 running > 0
3. `onHeaderField(buf)` — 头部名，奇数长度拼接 Buffer
4. `onHeaderValue(buf)` — 头部值，识别 keep-alive/connection/content-length
5. `onHeadersComplete(statusCode, upgrade, shouldKeepAlive)` — 动态调整 keep-alive timeout、背压暂停/恢复
6. `onBody(buf)` — 响应体，maxResponseSize 检查
7. `onMessageComplete()` — 完成，Content-Length 校验、keep-alive 决策
8. `onUpgrade(head)` — 协议升级（CONNECT/WebSocket）

### 4.3 HTTP/2 多路复用

`lib/dispatcher/client-h2.js`：

- **stream 生命周期**：`openStream()` → 事件绑定 → `writeBodyH2()` → `onEnd()`/`onResponse()` → `completeRequestStream()` → `closeStreamSession()`
- **GOAWAY 恢复**：两阶段分离 + 可重放 body 检查 + 重放预算
- **Extended CONNECT (RFC-8441)**：WebSocket over HTTP/2，检查 `kEnableConnectProtocol`
- **Ping 保活**：`onHttp2SendPing()` + `pingInterval` 配置
- **NO_STREAMS 超时**：`setNoStreamsTimeout()` 防止 `MAX_CONCURRENT_STREAMS=0` 死锁
- **ref/unref 缓存**：`state.refed` 避免重复系统调用

### 4.4 H1 Body 5 种写入策略

```javascript
if (!body || bodyLength === 0) writeBuffer(...)
else if (util.isBuffer(body)) writeBuffer(...)
else if (util.isBlobLike(body)) { body.stream ? writeIterable(...) : writeBlob(...) }
else if (util.isStream(body)) writeStream(...)
else if (util.isIterable(body)) writeIterable(...)
```

### 4.5 H2CClient 明文 HTTP/2

`lib/dispatcher/h2c-client.js`：`allowH2: true` + `useH2c: true`，仅支持 `http:` 协议。

---

## 5. 流式 Body 传输与背压

### 5.1 AsyncWriter 分块写入

`lib/dispatcher/client-h1.js:1645-1799`：

```javascript
write (chunk) {
  socket.cork()
  if (bytesWritten === 0) {
    if (contentLength === null) {
      socket.write(`${header}transfer-encoding: chunked\r\n`, 'latin1')
    } else {
      socket.write(`${header}content-length: ${contentLength}\r\n\r\n`, 'latin1')
    }
  }
  if (contentLength === null) {
    socket.write(`\r\n${len.toString(16)}\r\n`, 'latin1')  // chunked 前缀
  }
  this.bytesWritten += len
  const ret = socket.write(chunk)
  socket.uncork()
  return ret
}
```

### 5.2 Promise 化背压等待

```javascript
const waitForDrain = () => new Promise((resolve, reject) => {
  if (socket[kError]) reject(socket[kError])
  else callback = resolve
})
for await (const chunk of body) {
  if (!writer.write(chunk)) await waitForDrain()
}
```

`close` 事件也触发 `onDrain`，确保 socket 关闭时 Promise 能被释放。

### 5.3 BodyReadable 注入模式

`lib/api/readable.js:26-340`：

```javascript
constructor ({ resume, abort, contentType, contentLength, highWaterMark }) {
  super({ autoDestroy: true, read: resume, highWaterMark })
  this[kAbort] = abort
}
```

不继承特定协议层，通过构造函数注入 `resume`/`abort` 回调，同时用于 H1/H2。

**kReading 热路径优化**：没有 `data`/`readable` 监听器时，`push()` 跳过事件分发直接返回 true。

### 5.4 ReadableStreamFrom 字节流

`lib/core/util.js:651-683`：

```javascript
function ReadableStreamFrom (iterable) {
  let iterator
  return new ReadableStream({
    start () { iterator = iterable[Symbol.asyncIterator]() },
    pull (controller) {
      return iterator.next().then(({ done, value }) => {
        if (done) { controller.close(); controller.byobRequest?.respond(0); return }
        const buf = Buffer.isBuffer(value) ? value : Buffer.from(value)
        if (buf.byteLength) controller.enqueue(new Uint8Array(buf))
        else return this.pull(controller)  // 跳过空块
      })
    },
    type: 'bytes'
  })
}
```

---

## 6. 拦截器体系

### 6.1 compose 柯里化组合

`lib/dispatcher/dispatcher.js:18-51`：

```javascript
compose (...args) {
  const interceptors = Array.isArray(args[0]) ? args[0] : args
  let dispatch = this.dispatch.bind(this)
  for (const interceptor of interceptors) {
    if (interceptor == null) continue
    dispatch = interceptor(dispatch)
  }
  // Proxy 仅覆盖 dispatch 属性
  return new Proxy(this, {
    get: (target, key) => key === 'dispatch' ? dispatch : target[key]
  })
}
```

### 6.2 cache 拦截器

实现 RFC 9111 完整 HTTP 缓存语义。三层架构：拦截器入口（决策逻辑）→ CacheHandler（读写逻辑）→ Store（存储后端）。

**核心流程**：方法过滤 → Origin 白名单 → Cache-Control 解析 → 缓存查找 → stale 判断（max-stale/min-fresh）→ stale-while-revalidate 非阻塞重验证 → Heuristic Caching（10% 规则）→ Revalidation-only entries（24h 保留）

**stale-while-revalidate**：先立即返回陈旧缓存，再通过 `queueMicrotask` 后台重验证。

### 6.3 retry 拦截器

**RetryController 代理模式**（`lib/handler/retry-handler.js:48-69`）：

```javascript
class RetryController {
  pause () { this.target?.pause() }
  resume () { this.target?.resume() }
  abort (reason) { this.target?.abort(reason); this.#onAbort(reason) }
}
```

稳定代理对象，`this.target` 始终指向当前活跃连接的 controller。

**指数退避**：`minTimeout=500ms` → `maxTimeout=30s`, factor=2。公式 `500ms * 2^(counter-1)`。

**可重试条件**：
- 方法：GET/HEAD/OPTIONS/PUT/DELETE/TRACE/QUERY
- 状态码：500/502/503/504/429
- 错误码：ECONNRESET/ECONNREFUSED/ENOTFOUND 等 9 种

**部分内容恢复**：连接中断时已传输部分 body，重试添加 `Range: bytes=...` + `If-Match: etag` 请求续传。

### 6.4 redirect 拦截器

支持 300/301/302/303/307/308。POST→GET 规则：301/302 仅 POST 转 GET，303 全部转 GET，307/308 保持方法。循环检测通过 `history` 数组。跨域自动剥离 authorization/cookie。

### 6.5 dns 拦截器

DNSInstance + DNSStorage 架构。Dual-Stack 故障转移（`firstTry` 标志防无限循环）。IP 轮转（round-robin + offset 计数器）。SNI 处理保留 `servername`。

### 6.6 decompress 拦截器

支持 gzip/deflate/br/zstd。CVE 防护 `maxContentEncodings=5`。链式解压管道 `pipeline`。skipStatusCodes=[204, 304]。

### 6.7 deduplicate 拦截器

DeduplicationHandler 合并并发 GET 请求。`maxBufferSize=5MB` 流式缓冲。body 流已开始后安全降级为独立发送。

### 6.8 dump 拦截器

Content-Length 预检查（`maxSize=1MB`）。劫持 controller abort 方法记录状态。

### 6.9 response-error 拦截器

仅对 `statusCode >= 400` 收集 body。处理 `application/json` 和 `text/plain`。创建 ResponseError 时临时 `Error.stackTraceLimit = 0` 避免栈追踪开销。

---

## 7. 缓存系统

### 7.1 Cache-Control 解析引擎

`lib/util/cache.js`（716 行）：手工状态机处理带引号的值，容错 malformed quotes、trailing whitespace。数值指令 max-age/s-maxage/stale-while-revalidate 等在缺失/非数字/有前导空格时标记无效。

### 7.2 MemoryCacheStore

`lib/cache/memory-cache-store.js`：`Map<string, Entry[]>` 结构。

- `maxCount=1024`, `maxSize=100MB`, `maxEntrySize=5MB`
- 近似 LRU 淘汰：删除每 key 一半条目（`Math.ceil(entries.length / 2)`）
- `maxSizeExceeded` 事件只发射一次直到缓存降到阈值以下

### 7.3 SqliteCacheStore

`lib/cache/sqlite-cache-store.js`：`node:sqlite` `DatabaseSync`。

- Schema V3，WAL 模式 + NORMAL 同步
- Prepared Statements 预编译
- 两阶段 Prune：先删过期，再删最老 10%
- `maxEntrySize=2GB`, `maxCount=Infinity`

### 7.4 HTTP 日期解析

`lib/util/date.js`（671 行）：三种格式（IMF-fixdate/asctime/RFC 850），完全 `charCodeAt` + 位置索引，零正则。`makeDate` 反向验证 7 个字段。

### 7.5 CacheRevalidationHandler

RFC 5861 stale-if-error 扩展：5xx 响应视为重验证成功。连接错误也触发 stale-if-error。

---

## 8. 诊断与可观测性

### 8.1 diagnostics_channel 全覆盖

`lib/core/diagnostics.js`（228 行），16 个 channel：

| 组别 | Channel | 含义 |
|------|---------|------|
| Client | beforeConnect, connected, connectError, sendHeaders | 连接生命周期 |
| Request | create, bodySent, bodyChunkSent, bodyChunkReceived, headers, trailers, error | 请求生命周期 |
| WebSocket | open, close, socketError, ping, pong | WebSocket 生命周期 |
| Proxy | proxyConnected | 代理隧道建立 |

### 8.2 防重复订阅

```javascript
if (channels.beforeConnect.hasSubscribers || ...) {
  isTrackingClientEvents = true
  return
}
```

解决 Node.js 内置 undici 和 npm 安装共存的重复订阅问题。

### 8.3 分域调试日志

- `NODE_DEBUG=undici` → `undiciDebugLog`
- `NODE_DEBUG=fetch` → `fetchDebuglog`
- `NODE_DEBUG=websocket` → `websocketDebuglog`

---

## 9. 代理体系

### 9.1 ProxyAgent

`lib/dispatcher/proxy-agent.js`（379 行）：

- **HTTP CONNECT 隧道**：CONNECT → 200 验证 → TLS 升级
- **Http1ProxyWrapper**：非隧道，改写 path 为绝对路径
- **安全防护**：`throwIfProxyAuthIsSent` 防止请求级凭据泄露

### 9.2 Socks5ProxyAgent

`lib/core/socks5-client.js`（423 行）：

Socks5Client 6 态状态机：INITIAL → HANDSHAKING → AUTHENTICATING → AUTHENTICATED → CONNECTING → CONNECTED

协议：SOCKS5(0x05) + 认证方法（NO_AUTH/USERNAME_PASSWORD）+ CONNECT 命令 + 地址类型（IPv4/Domain/IPv6）

Per-Origin Pool 确保不同主机不混用连接。

### 9.3 EnvHttpProxyAgent

`lib/dispatcher/env-http-proxy-agent.js`（176 行）：

- `http_proxy`/`https_proxy`/`no_proxy` 环境变量（小写优先）
- `*` 通配符全局排除
- IPv6 方括号处理
- 尾部点号 FQDN 处理
- 动态 NO_PROXY 检测

---

## 10. Mock 测试系统

### 10.1 MockAgent 架构

继承 Dispatcher，内部包装真实 Agent。`buildMockDispatch` 闭包工厂根据 `isMockActive` 状态分流：激活走 mock 路径，关闭走真实网络。

`disableNetConnect()` / `enableNetConnect(matcher)` 控制网络访问策略。

### 10.2 四维匹配引擎

path → method → body → headers，按序过滤。`matchValue` 三态：string 精确、RegExp 正则、function 谓词。

URL 归一化：`safeUrl` query 排序、`normalizeSearchParams` 非标准参数处理。

### 10.3 MockCallHistory

MockCallHistoryLog 9 字段结构。`filterCalls` 支持函数/正则/对象三种 criteria，OR/AND 组合操作符。

### 10.4 Snapshot 快照系统

三层架构：SnapshotAgent（入口，3 模式）→ SnapshotRecorder（录制/存储）→ snapshot-utils（工具）

**3 模式**：record（走真实网络录制）、playback（从快照回放）、update（先回放后录制补充）

**请求哈希**：SHA-256（有 crypto）或 Base64url（fallback）。Header 三级过滤：excludeHeaders（安全考虑）、ignoreHeaders（匹配忽略）、matchHeaders（白名单）。

---

## 11. Web 标准实现

### 11.1 EventSource/SSE

EventSource 继承 EventTarget，readyState 三态（CONNECTING/OPEN/CLOSED），`reconnectionTime=3000ms`。自动重连 + `Last-Event-ID`。

EventSourceStream 是 SSE 协议核心解析引擎：Buffer 字节层面操作，`data/event/id/retry` 字段解析，BOM 检测跳过，`maxEventSize` 限制，零拷贝缓冲区管理（chunks 数组 + 游标 + subarray）。

### 11.2 Cookies

`parseSetCookie` 严格对齐 RFC 6265bis 8 步算法。CTL 字符检查、name-value-pair 分割、maxNameValuePairSize=4096。属性解析支持 expires/maxAge/domain/path/secure/httpOnly/sameSite。

### 11.3 WebIDL 类型系统

- `brandCheck`：`Function.prototype[Symbol.hasInstance]` 防跨 realm 问题
- `ConvertToInt`：完整 WebIDL Section 3.2.10（EnforceRange/Clamp/模运算）
- `converters` 矩阵：DOMString/USVString/ByteString/sequence/record/Dictionary/Interface/nullable/TypedArray
- `errors` 工厂：exception/conversionFailed/invalidArgument

### 11.4 WHATWG Infra

- `collectASequenceOfCodePointsFast`：`indexOf` 替代逐字符循环 O(n*k) → O(n)
- `forgivingBase64`：容错 Base64 解码
- `isomorphicDecode`：分批 32K `String.fromCharCode.apply`

### 11.5 SRI 子资源完整性

`validSRIHashAlgorithmTokenSet`（sha256/sha384/sha512）强度排序。`bytesMatch` 最强算法选择 + 宽容比较（忽略尾部 `=` padding，容忍 base64url 编码）。

---

## 12. 跨模块设计模式总结

| 模式 | 应用 | 示例 |
|------|------|------|
| **装饰器** | Handler 层叠 | DecoratorHandler → CacheHandler/DeduplicationHandler/DumpHandler |
| **策略** | 连接池调度 | Pool/BalancedPool/RoundRobinPool 各自 kGetDispatcher |
| **工厂** | Agent 创建 | kFactory 默认 Client/Pool 工厂 |
| **代理** | 重试 Controller | RetryController 转发到活跃连接 |
| **观察者** | 事件驱动 | drain/connect/disconnection/connectionError |
| **组合** | 拦截器链 | compose + Proxy dispatch 覆盖 |
| **状态机** | 连接/定时器/协议 | Socks5Client 6 态 / FastTimer 4 态 / EventSource 3 态 |
| **快照** | 测试录制回放 | SnapshotRecorder → SnapshotAgent |

---

## 13. 对 laew 的借鉴价值与路线图

### P0（立即借鉴）

1. **错误体系**：`Symbol.hasInstance` 防伪 → Rust `thiserror` + `ErrorKind` enum + `From` trait
2. **超时分层**：headers/body 用 cooperative cancel，keep-alive 用 wall clock
3. **compose 拦截器**：Rust trait object + `Vec<Box<dyn Handler>>` 可组合中间件链
4. **FixedQueue**：Rust 手写高性能循环缓冲队列，2048 槽位设计启发

### P1（短期借鉴）

1. **TernarySearchTree**：Rust `aho-corasick` crate 或字节数组 trie
2. **FastTimer 批量处理**：tokio 心跳管理用一个定时器驱动多个超时
3. **RetryController 代理**：laew 工具调用重试的取消/暂停语义
4. **DeduplicationHandler**：相同请求合并减少 LLM API 调用
5. **Snapshot 测试系统**：record/playback + SHA-256 哈希 + header 归一化

### P2（中期借鉴）

1. **DNS 缓存拦截器**：减少 DNS 查询开销
2. **缓存系统**：Memory + SQLite 双后端 + `maxCount/maxSize/maxEntrySize` 三维限制
3. **diagnostics_channel 分层**：connection/request/tool_call 三层 tracing
4. **WebIDL 参数校验**：工具注册和调用入口的严格类型/参数校验
5. **SSE 解析器优化**：零拷贝缓冲区管理，chunks 数组 + 游标 + subarray

### 关键文件索引

| 文件 | 行数 | 核心内容 |
|------|------|---------|
| `lib/dispatcher/client-h1.js` | 1,801 | H1 Parser + 8 回调 + AsyncWriter |
| `lib/dispatcher/client-h2.js` | 1,781 | H2 多路复用 + GOAWAY + Extended CONNECT |
| `lib/dispatcher/client.js` | 741 | Client 三段式队列 + 协议感知并发 |
| `lib/core/errors.js` | 497 | 26 种错误类 + Symbol.hasInstance 防伪 |
| `lib/core/util.js` | 1,049 | 工具函数集 + ReadableStreamFrom |
| `lib/util/timers.js` | 425 | FastTimer 4 态状态机 |
| `lib/core/tree.js` | 160 | TernarySearchTree 头部查找 |
| `lib/interceptor/cache.js` | 618 | HTTP 缓存拦截器 |
| `lib/handler/retry-handler.js` | 548 | RetryController 代理 + 指数退避 |
| `lib/mock/snapshot-recorder.js` | 623 | 快照录制 SHA-256 哈希 |
| `lib/mock/mock-utils.js` | 720 | 四维匹配引擎 |
| `lib/cache/memory-cache-store.js` | 279 | LRU 淘汰 + 三维限制 |
| `lib/cache/sqlite-cache-store.js` | 469 | SQLite 持久化 + Prepared Statements |
