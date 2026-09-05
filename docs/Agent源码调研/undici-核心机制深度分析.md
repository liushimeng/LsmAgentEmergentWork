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

---

### 核心模块深度专题（第三轮深化）

13. [errors.js 完整解析 — 26 种错误类谱系](#13-errorsjs-完整解析--26-种错误类谱系)
14. [retry-handler.js 深度解析 — 重试控制器与部分恢复](#14-retry-handlerjs-深度解析--重试控制器与部分恢复)
15. [redirect-handler.js 深度解析 — 重定向状态机](#15-redirect-handlerjs-深度解析--重定向状态机)
16. [diagnostics.js 完整诊断钩子清单](#16-diagnosticsjs-完整诊断钩子清单)
17. [buildConnector 连接管理 — TCP/TLS/Unix 统一连接器](#17-buildconnector-连接管理--tcpunix-统一连接器)
18. [Request 核心类完整解析 — 构造/Header/Body 生命周期](#18-request-核心类完整解析--构造headerbody-生命周期)
19. [FastTimer 与时间管理 — 自研低精度定时器全解](#19-fasttimer-与时间管理--自研低精度定时器全解)
20. [FixedQueue 数据结构 — Node.js 级高性能环形缓冲](#20-fixedqueue-数据结构--nodejs-级高性能环形缓冲)
21. [TernarySearchTree 三叉搜索树 — 大小写不敏感 Header 查找](#21-ternarysearchtree-三叉搜索树--大小写不敏感-header-查找)
22. [constants.js 与 symbols.js — 常量体系与内部 Symbol 清单](#22-constantsjs-与-symbolsjs--常量体系与内部-symbol-清单)
23. [容错与超时体系总览 — 四维超时配置与实现](#23-容错与超时体系总览--四维超时配置与实现)

---

24. [对 laew 的借鉴价值与路线图](#24-对-laew-的借鉴价值与路线图)

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

## 13. errors.js 完整解析 — 26 种错误类谱系

`lib/core/errors.js` 共 497 行，是 undici 统一错误体系的唯一入口。本节给出 26 个错误类的完整类图、触发条件、属性与使用场景。

### 13.1 基类防伪设计

```javascript
// lib/core/errors.js:3-18
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
  get [kUndiciError] () { return true }
}
```

**防伪三要素**：
1. `Symbol.for()` 全局符号 — 跨 Realm/VM 上下文唯一
2. getter 返回字面量 true — 适用于任何对象，含字面量/反序列化对象
3. 静态 `[Symbol.hasInstance]` — 覆盖 `instanceof` 默认行为，即使跨 bundler 拷贝也保持识别

### 13.2 完整 ASCII 继承树

```
UndiciError (UND_ERR)                              ← 所有 undici 错误的基类
 │
 ├─ 超时类 (Timeout)
 │   ├─ ConnectTimeoutError          UND_ERR_CONNECT_TIMEOUT  连接建立超时
 │   ├─ HeadersTimeoutError          UND_ERR_HEADERS_TIMEOUT  等待响应头超时
 │   ├─ HeadersOverflowError         UND_ERR_HEADERS_OVERFLOW 响应头尺寸超限
 │   └─ BodyTimeoutError             UND_ERR_BODY_TIMEOUT     等待 Body 超时
 │
 ├─ 参数/校验类 (Validation)
 │   ├─ InvalidArgumentError         UND_ERR_INVALID_ARG       参数非法
 │   └─ InvalidReturnValueError      UND_ERR_INVALID_RETURN_VALUE  返回值非法
 │
 ├─ 生命周期类 (Lifecycle)
 │   ├─ AbortError                   UND_ERR_ABORT             操作被中止
 │   │   └─ RequestAbortedError      UND_ERR_ABORTED           请求被中止
 │   ├─ InformationalError           UND_ERR_INFO              非致命信息错误
 │   ├─ ClientDestroyedError         UND_ERR_DESTROYED         Client 已销毁
 │   ├─ ClientClosedError            UND_ERR_CLOSED            Client 已关闭
 │   └─ SocketError                  UND_ERR_SOCKET            Socket 错误
 │
 ├─ 内容协议类 (Content)
 │   ├─ RequestContentLengthMismatchError   UND_ERR_REQ_CONTENT_LENGTH_MISMATCH
 │   ├─ ResponseContentLengthMismatchError  UND_ERR_RES_CONTENT_LENGTH_MISMATCH
 │   ├─ ResponseExceededMaxSizeError        UND_ERR_RES_EXCEEDED_MAX_SIZE
 │   └─ RequestRetryError                   UND_ERR_REQ_RETRY
 │
 ├─ 代理类 (Proxy)
 │   ├─ SecureProxyConnectionError   UND_ERR_PRX_TLS    TLS 代理握手失败
 │   ├─ ProxyConnectionError         UND_ERR_PRX_CONN   SOCKS4/普通代理连接失败
 │   └─ Socks5ProxyError             UND_ERR_SOCKS5     SOCKS5 协议错误
 │
 ├─ 特殊类 (Special)
 │   ├─ NotSupportedError            UND_ERR_NOT_SUPPORTED     特性不支持
 │   ├─ BalancedPoolMissingUpstreamError  UND_ERR_BPL_MISSING_UPSTREAM
 │   ├─ MaxOriginsReachedError       UND_ERR_MAX_ORIGINS_REACHED
 │   ├─ MessageSizeExceededError     UND_ERR_WS_MESSAGE_SIZE_EXCEEDED
 │   └─ ResponseError               UND_ERR_RESPONSE           response-error 拦截器抛出
 │
 └─ HTTPParserError                  HPE_*                     ← 唯一非 Error 子类
       直接继承 Error，携带 HPE_ 前缀代码 + data
```

### 13.2 错误类详解表

| 错误类 | 默认消息 | 特殊属性 | 典型触发场景 |
|--------|---------|---------|-------------|
| `ConnectTimeoutError` | `Connect Timeout Error` | 无 | `connectTimeout` 到期、TLS 握手超时 |
| `HeadersTimeoutError` | `Headers Timeout Error` | 无 | `headersTimeout` 到期，服务器发送头太慢 |
| `HeadersOverflowError` | `Headers Overflow Error` | 无 | 响应头超过 `maxResponseSize` |
| `BodyTimeoutError` | `Body Timeout Error` | 无 | `bodyTimeout` 到期，body 传输太慢 |
| `InvalidArgumentError` | `Invalid Argument Error` | 无 | 构造 Request 参数非法 |
| `InvalidReturnValueError` | `Invalid Return Value Error` | 无 | handler/拦截器返回值非法 |
| `AbortError` | `The operation was aborted` | 无 | `AbortController.abort()` |
| `RequestAbortedError` | `Request aborted` | 无 | 请求级中止（继承 AbortError） |
| `InformationalError` | `Request information` | 无 | "reset"/"upgrade"/"idle timeout" 等正常生命周期 |
| `RequestContentLengthMismatchError` | `Request body length does not match content-length header` | 无 | 发送 body 与 Content-Length 不符 |
| `ResponseContentLengthMismatchError` | `Response body length does not match content-length header` | 无 | 接收 body 与 Content-Length 不符 |
| `ClientDestroyedError` | `The client is destroyed` | 无 | 在已销毁 Client 上 dispatch |
| `ClientClosedError` | `The client is closed` | 无 | 在已关闭 Client 上 dispatch |
| `SocketError` | `Socket error` | `socket` | 底层 socket 错误，携带 socket 引用 |
| `NotSupportedError` | `Not supported error` | 无 | 如 `Expect` 请求头 |
| `BalancedPoolMissingUpstreamError` | `No upstream has been added to the BalancedPool` | 无 | 空 BalancedPool dispatch |
| `ResponseExceededMaxSizeError` | `Response content exceeded max size` | 无 | 响应超过 `maxResponseSize` |
| `RequestRetryError` | `Request retry error` | `statusCode, headers, data` | 重试拦截器抛出，携带 count |
| `ResponseError` | `Response error` | `statusCode, headers, body` | response-error 拦截器抛出 |
| `SecureProxyConnectionError` | `Secure Proxy Connection failed` | `cause` | HTTPS 代理 TLS 失败，携带原始 cause |
| `ProxyConnectionError` | `Proxy Connection failed` | `cause` | HTTP 代理 CONNECT 失败 |
| `MaxOriginsReachedError` | `Maximum allowed origins reached` | 无 | Agent origin 数超限 |
| `Socks5ProxyError` | `SOCKS5 proxy error` | `code` | SOCKS5 协议级错误 |
| `MessageSizeExceededError` | `Max decompressed message size exceeded` | 无 | WebSocket 解压消息超限 |
| `HTTPParserError` | (自定义) | `code: HPE_*`, `data` | llhttp 解析错误，非 UndiciError |

### 13.3 错误分组体系

```
┌─────────────────────────────────────────────────────┐
│                  errors.js 分组                      │
├──────────────┬──────────────────────────────────────┤
│ 客户端错误    │ InvalidArgumentError                 │
│ (输入校验)    │ InvalidReturnValueError             │
│              │ BalancedPoolMissingUpstreamError     │
│              │ MaxOriginsReachedError               │
│              │ NotSupportedError                    │
├──────────────┼──────────────────────────────────────┤
│ 连接错误      │ ConnectTimeoutError                 │
│ (网络/传输层) │ SocketError                          │
│              │ SecureProxyConnectionError           │
│              │ ProxyConnectionError                 │
│              │ Socks5ProxyError                     │
├──────────────┼──────────────────────────────────────┤
│ 超时错误      │ ConnectTimeoutError                 │
│ (时间约束)    │ HeadersTimeoutError                 │
│              │ HeadersOverflowError                │
│              │ BodyTimeoutError                    │
├──────────────┼──────────────────────────────────────┤
│ 内容错误      │ RequestContentLengthMismatchError   │
│ (协议层)      │ ResponseContentLengthMismatchError  │
│              │ ResponseExceededMaxSizeError         │
│              │ HTTPParserError                      │
│              │ MessageSizeExceededError             │
├──────────────┼──────────────────────────────────────┤
│ 生命周期错误  │ AbortError / RequestAbortedError    │
│ (状态约束)    │ InformationalError                   │
│              │ ClientDestroyedError                 │
│              │ ClientClosedError                    │
├──────────────┼──────────────────────────────────────┤
│ 重试/响应错误 │ RequestRetryError                   │
│ (业务层)      │ ResponseError                        │
└──────────────┴──────────────────────────────────────┘
```

### 13.4 InformationalError 的特殊语义

`InformationalError`（code = `UND_ERR_INFO`）在 `client.js:473` 被特殊处理：不触发队列清理。用于表示"非致命"的"正常结束"事件，避免误杀其他等待中的请求。

典型用途：
- `ECONNRESET` — 远端重置
- `upgrade` — 协议升级（WebSocket）
- `socket idle timeout` — 空闲回收

```javascript
// lib/dispatcher/client.js:473 简示
if (err.code !== 'UND_ERR_INFO') {
  // 仅非信息错误才清理队列
}
```

### 13.5 HTTPParserError — 唯一非继承类

```javascript
// lib/core/errors.js:309-325
class HTTPParserError extends Error {   // ← 直接继承 Error
  constructor (message, code, data) {
    super(message)
    this.name = 'HTTPParserError'
    this.code = code ? `HPE_${code}` : undefined   // HPE_ 前缀
    this.data = data ? data.toString() : undefined
  }
}
```

直接继承 Error 而非 UndiciError，因为这是 llhttp 底层错误，与 undici 应用层错误属于不同域。`HPE_*` 代码来自 llhttp 枚举（如 `HPE_INVALID_METHOD`、`HPE_INVALID_CONSTANT`、`HPE_INVALID_HEADER_TOKEN` 等）。

---

## 14. retry-handler.js 深度解析 — 重试控制器与部分恢复

`lib/handler/retry-handler.js` 共 548 行，是 undici 拦截器体系中最复杂的 Handler。实现了指数退避、重试条件判定、部分内容恢复、ETag 校验、消费者中止联动。

### 14.1 RetryController 代理模式

```javascript
// lib/handler/retry-handler.js:48-69
class RetryController {
  #onAbort   // 私有字段，abort 回调

  constructor (onAbort) {
    this.#onAbort = onAbort
    this.target = null   // ← 指向当前活跃连接的 controller
  }

  pause () { this.target?.pause() }
  resume () { this.target?.resume() }

  abort (reason) {
    this.target?.abort(reason)
    this.#onAbort(reason)
  }

  get paused () { return this.target?.paused ?? false }
  get aborted () { return this.target?.aborted ?? false }
  get reason () { return this.target?.reason ?? null }
  get rawHeaders () { return this.target?.rawHeaders ?? null }
  get rawTrailers () { return this.target?.rawTrailers ?? null }
}
```

**设计关键**：每次 retry/resume 都是独立的 dispatch，产生**新**连接的 controller。稳定代理对象转发到当前活跃 controller，避免下游 body 控制的 controller 和接收数据的 controller 不一致导致背压死锁。

```
消费者 ──▶ RetryController ──┬──▶ controller (conn 1) — 首次请求
                            └──▶ controller (conn 2) — 第 1 次重试
```

### 14.2 默认重试配置

```javascript
// lib/handler/retry-handler.js:92-117
this.retryOpts = {
  retry: retryFn ?? RetryHandler[kRetryHandlerDefaultRetry],
  retryAfter: retryAfter ?? true,                // 遵守 Retry-After
  maxTimeout: maxTimeout ?? 30 * 1000,           // 单次退避上限 30s
  minTimeout: minTimeout ?? 500,                 // 起始退避 500ms
  timeoutFactor: timeoutFactor ?? 2,             // 指数因子
  maxRetries: maxRetries ?? 5,                   // 最多重试 5 次
  methods: methods ?? ['GET','HEAD','OPTIONS','PUT','DELETE','TRACE','QUERY'],
  statusCodes: statusCodes ?? [500,502,503,504,429],
  errorCodes: errorCodes ?? [
    'ECONNRESET','ECONNREFUSED','ENOTFOUND',
    'ENETDOWN','ENETUNREACH','EHOSTDOWN',
    'EHOSTUNREACH','EPIPE','UND_ERR_SOCKET'
  ]
}
```

### 14.3 默认重试策略详解

```javascript
// lib/handler/retry-handler.js:222-283
static [kRetryHandlerDefaultRetry] (err, { state, opts }, cb) {
  const { statusCode, code, headers } = err
  const { counter } = state

  // 1. 错误码不在白名单 → 不重试
  if (code && code !== 'UND_ERR_REQ_RETRY' && !errorCodes.includes(code)) {
    cb(err); return
  }

  // 2. 方法不在白名单 → 不重试
  if (Array.isArray(methods) && !methods.includes(method)) {
    cb(err); return
  }

  // 3. 状态码不在白名单 → 不重试
  if (statusCode != null && Array.isArray(statusCodes) && !statusCodes.includes(statusCode)) {
    cb(err); return
  }

  // 4. 超过最大重试次数 → 不重试
  if (counter > maxRetries) {
    cb(err); return
  }

  // 5. 计算退避时间（Retry-After 优先）
  let retryAfterHeader = retryAfter === false ? undefined : headers?.['retry-after']
  if (retryAfterHeader) {
    retryAfterHeader = Number(retryAfterHeader)
    retryAfterHeader = Number.isNaN(retryAfterHeader)
      ? calculateRetryAfterHeader(headers['retry-after'])  // HTTP-date 格式
      : retryAfterHeader * 1e3                               // 秒数
  }

  const retryTimeout =
    retryAfterHeader === 0 ? 0 :
    retryAfterHeader > 0
      ? Math.min(retryAfterHeader, maxTimeout)
      : Math.min(minTimeout * timeoutFactor ** (counter - 1), maxTimeout)
              // 500ms * 2^(counter-1), 上限 30s

  return setTimeout(() => cb(null), retryTimeout)   // 返回 timer，支持 abort 取消
}
```

**退避序列**（counter 从 1 起）：500ms → 1s → 2s → 4s → 8s → 16s → 30s（封顶）

### 14.4 重试判定流程

```
                 err / statusCode >= 300
                        │
                        ▼
           ┌──── 错误码在白名单？──── No ──▶ 传播错误
           │ Yes
           ▼
           ┌──── 方法在白名单？──── No ──▶ 传播错误
           │ Yes
           ▼
           ┌──── 状态码在白名单？──── No ──▶ 传播错误
           │ Yes
           ▼
           ┌──── counter > maxRetries？── Yes ──▶ 传播错误
           │ No
           ▼
     计算退避时间（Retry-After 优先）
           │
           ▼
     setTimeout 等待 ──▶ cb(null) ──▶ retry()
           ▲                │
           │    abort 取消 timer
           └────────────────┘
```

### 14.5 部分内容恢复（Partial Content Resume）

```javascript
// lib/handler/retry-handler.js:453-477
retry () {
  if (this.start !== 0) {
    const headers = { range: `bytes=${this.start}-${this.end ?? ''}` }

    // 弱 etag 不参与匹配（以 W/ 开头）
    if (this.etag != null) {
      headers['if-match'] = this.etag
    }

    this.opts = {
      ...this.opts,
      headers: { ...this.opts.headers, ...headers }
    }
  }

  try {
    this.retryCountCheckpoint = this.retryCount
    this.dispatch(this.opts, this)
  } catch (err) {
    this.handler.onResponseError?.(this.controllerProxy, err)
  }
}
```

**内容恢复机制**：
- `this.start` 记录已接收字节数
- 重试时添加 `Range: bytes={start}-{end}` 请求头
- 若服务器之前返回了 ETag，加 `If-Match` 校验一致性
- `onResponseStart` 中校验 `Content-Range` + `Content-Length` + ETag 三重一致性
- `W/` 前缀的弱 etag 在 `onResponseStart:397-403` 被清空，不参与校验

### 14.6 消费者中止联动

```javascript
// lib/handler/retry-handler.js:531-545
#onAbort (reason) {
  if (!this.retryPending) { return }

  this.aborted = true
  this.retryPending = false
  clearTimeout(this.retryTimer)    // 取消等待中的退避定时器
  this.retryTimer = null
  this.handler.onResponseError?.(this.controllerProxy, reason ?? new RequestAbortedError())
}
```

**retryPending 标志**：当 retry 策略正在决定（通常持有 setTimeout），消费者 abort 可立即取消等待，避免请求一直挂到退避时间耗尽。

### 14.7 retryCount vs retryCountCheckpoint

```javascript
// lib/handler/retry-handler.js:511-518
if (this.retryCount - this.retryCountCheckpoint > 0) {
  // 网络错误 + 服务器错误混合场景的对账
  this.retryCount =
    this.retryCountCheckpoint +
    (this.retryCount - this.retryCountCheckpoint)
} else {
  this.retryCount += 1
}
```

`retryCountCheckpoint` 在每次 `retry()` 时记录，用于网络错误和服务器错误混合场景的计数对账。

---

## 15. redirect-handler.js 深度解析 — 重定向状态机

`lib/handler/redirect-handler.js` 共 229 行，实现 RFC 7231 重定向语义。

### 15.1 重定向状态码

```javascript
// lib/handler/redirect-handler.js:7
const redirectableStatusCodes = [300, 301, 302, 303, 307, 308]
```

### 15.2 方法转换规则

```javascript
// lib/handler/redirect-handler.js:63-86
let removeContentHeaders = statusCode === 303

// 301/302 + POST → GET
if ((statusCode === 301 || statusCode === 302) && this.opts.method === 'POST') {
  this.opts.method = 'GET'
  if (util.isStream(this.opts.body)) {
    util.destroy(this.opts.body.on('error', noop))
  }
  this.opts.body = null
  removeContentHeaders = true
}

// 303（非 HEAD）→ GET
if (statusCode === 303 && this.opts.method !== 'HEAD') {
  this.opts.method = 'GET'
  if (util.isStream(this.opts.body)) {
    util.destroy(this.opts.body.on('error', noop))
  }
  this.opts.body = null
}
```

**状态码 × 方法矩阵**：

| 状态码 | POST | GET/HEAD/其他 |
|--------|------|---------------|
| 300 | 不转 | 不转 |
| 301 | → GET | 不转 |
| 302 | → GET | 不转 |
| 303 | → GET | → GET（非 HEAD） |
| 307 | 不转 | 不转 |
| 308 | 不转 | 不转 |

### 15.3 Location 解析与 URL 重建

```javascript
// lib/handler/redirect-handler.js:88-121
this.location = this.history.length >= this.maxRedirections
  || util.isDisturbed(this.opts.body)
  || redirectableStatusCodes.indexOf(statusCode) === -1
  ? null
  : headers.location

if (this.opts.origin) {
  this.history.push(new URL(this.opts.path, this.opts.origin))
}

// ... 后续
const { origin, pathname, search } = util.parseURL(
  new URL(this.location, this.opts.origin && new URL(this.opts.path, this.opts.origin))
)
const path = search ? `${pathname}${search}` : pathname
```

**Redirect 决策三条件**（任一为 true 则不重定向）：
1. `history.length >= maxRedirections` — 超过最大重定向次数
2. `isDisturbed(body)` — body 已消费（无法重新发送）
3. `statusCode` 不在 `redirectableStatusCodes` 中

### 15.4 循环检测

```javascript
// lib/handler/redirect-handler.js:104-112
const redirectUrlString = `${origin}${path}`
for (const historyUrl of this.history) {
  if (historyUrl.toString() === redirectUrlString) {
    throw new InvalidArgumentError(
      `Redirect loop detected. Cannot redirect to ${origin}. ...`
    )
  }
}
```

在 history 数组中查找重复 URL，发现循环立即抛出。这是 Client/Pool 跨 origin 重定向的兜底保护。

### 15.5 Header 清理规则

```javascript
// lib/handler/redirect-handler.js:169-184
function shouldRemoveHeader (header, removeContent, unknownOrigin, stripHeaders, stripHeadersOnCrossOrigin) {
  const name = util.headerNameToString(header)
  if (name === 'host') { return true }                    // 始终移除 Host
  if (stripHeaders?.has(name)) { return true }             // 用户自定义移除列表
  if (unknownOrigin && stripHeadersOnCrossOrigin?.has(name)) { return true }
  if (removeContent && name.startsWith('content-')) { return true }  // Content-* 在 POST→GET
  if (unknownOrigin) {                                    // 跨 origin 移除敏感头
    return name === 'authorization' || name === 'cookie' || name === 'proxy-authorization'
  }
  return false
}
```

**清理优先级**：
1. `Host` 始终移除（跟随新 origin）
2. 用户自定义 `stripHeaders` 列表
3. 用户自定义 `stripHeadersOnCrossOrigin` 列表（仅跨 origin 时）
4. `303` 或 `301/302 + POST` → 移除所有 `Content-*`
5. 跨 origin → 移除 `Authorization` / `Cookie` / `Proxy-Authorization`

### 15.6 Body 忽略语义

```javascript
// lib/handler/redirect-handler.js:123-145
onResponseData (controller, chunk) {
  if (this.location) {
    // 丢弃所有 3xx response body
  } else {
    this.handler.onResponseData?.(controller, chunk)
  }
}
```

undici 始终忽略 3xx 响应体，即使 RFC 允许 body 存在。这是安全+简化权衡。

### 15.7 递归 dispatch

```javascript
// lib/handler/redirect-handler.js:147-161
onResponseEnd (controller, trailers) {
  if (this.location) {
    this.dispatch(this.opts, this)   // ← 递归，当前 handler 继续复用
  } else {
    this.handler.onResponseEnd(controller, trailers)
  }
}
```

重定向时 `dispatch(this.opts, this)`，当前 RedirectHandler 继续作为 handler，保持 history 状态。

---

## 16. diagnostics.js 完整诊断钩子清单

`lib/core/diagnostics.js` 共 228 行，使用 Node.js `diagnostics_channel` 模块（`node:diagnostics_channel`）提供 5 组 16 个 channel。

### 16.1 Channel 全表

| 组 | Channel 名 | 触发时机 | 携带数据 |
|----|-----------|---------|---------|
| Client | `undici:client:beforeConnect` | TCP/TLS 连接建立前 | `{ connectParams: { version, protocol, port, host } }` |
| Client | `undici:client:connected` | 连接建立成功 | `{ connectParams: { version, protocol, port, host } }` |
| Client | `undici:client:connectError` | 连接失败 | `{ connectParams: {...}, error }` |
| Client | `undici:client:sendHeaders` | HTTP 头发送前 | `{ request: { method, path, origin } }` |
| Request | `undici:request:create` | Request 对象构造完成 | `{ request }` |
| Request | `undici:request:bodySent` | 请求体发送完成 | `{ request }` |
| Request | `undici:request:bodyChunkSent` | 每个 body 块发送 | `{ request, chunk }` |
| Request | `undici:request:bodyChunkReceived` | 每个 body 块接收 | `{ request, chunk }` |
| Request | `undici:request:headers` | 响应头接收 | `{ request, response: { statusCode, headers, statusText } }` |
| Request | `undici:request:trailers` | trailer 接收 | `{ request, trailers }` |
| Request | `undici:request:error` | 请求错误 | `{ request, error }` |
| WebSocket | `undici:websocket:open` | WS 连接打开 | `{ address: { address, port } }` 或空 |
| WebSocket | `undici:websocket:close` | WS 连接关闭 | `{ websocket, code, reason }` |
| WebSocket | `undici:websocket:socket_error` | WS socket 错误 | `error` |
| WebSocket | `undici:websocket:ping` | 收到 ping | 无 |
| WebSocket | `undici:websocket:pong` | 收到 pong | 无 |
| Proxy | `undici:proxy:connected` | 代理隧道建立 | `{ response, socket, head }` |

### 16.2 防重复订阅机制

```javascript
// lib/core/diagnostics.js:36-49
function trackClientEvents (debugLog = undiciDebugLog) {
  if (isTrackingClientEvents) { return }         // 本进程已订阅

  if (channels.beforeConnect.hasSubscribers || channels.connected.hasSubscribers ||
      channels.connectError.hasSubscribers || channels.sendHeaders.hasSubscribers) {
    isTrackingClientEvents = true               // 别人已订阅，标记为已跟踪但不重复订阅
    return
  }

  isTrackingClientEvents = true
  // ... 实际订阅
}
```

**双保险**：既防止同一进程内重复订阅，又防止 Node.js 内置 undici 和 npm 安装的 undici 共存时双重订阅。

### 16.3 分域 debug log

```javascript
// lib/core/diagnostics.js:6-8
const undiciDebugLog = util.debuglog('undici')
const fetchDebuglog = util.debuglog('fetch')
const websocketDebuglog = util.debuglog('websocket')
```

环境变量控制：
- `NODE_DEBUG=undici` → Client + Request 事件
- `NODE_DEBUG=fetch` → Client + Request 事件（使用 fetch 专用 logger）
- `NODE_DEBUG=websocket` → Client + WebSocket 事件

### 16.4 外部订阅示例

```javascript
const diagnosticsChannel = require('node:diagnostics_channel')

diagnosticsChannel.subscribe('undici:client:beforeConnect', (evt) => {
  console.log('即将连接到', evt.connectParams.host)
})

diagnosticsChannel.subscribe('undici:request:headers', (evt) => {
  console.log(`收到响应 ${evt.response.statusCode} ${evt.request.method} ${evt.request.origin}${evt.request.path}`)
})
```

---

## 17. buildConnector 连接管理 — TCP/TLS/Unix 统一连接器

`lib/core/connect.js` 共 193 行，导出 `buildConnector` 工厂函数，统一创建 TCP/TLS/Unix socket 连接器。

### 17.1 整体架构

```
buildConnector(options) → connect(params, callback) → socket

  protocol === 'https:'  ──▶  tls.connect(...)  +  SessionCache
  protocol === 'http:'   ──▶  net.connect(...)   +  keepAlive
  socketPath            ──▶  net.connect({ path })（Unix socket）
```

### 17.2 SessionCache（TLS Session 复用）

```javascript
// lib/core/connect.js:15-60
const SessionCache = class WeakSessionCache {
  constructor (maxCachedSessions) {
    this._maxCachedSessions = maxCachedSessions
    this._sessionCache = new Map()
    this._sessionRegistry = new FinalizationRegistry((key) => {
      // GC 回调：当 session 被回收，清理对应 Map 条目
      if (this._sessionCache.size < this._maxCachedSessions) return
      const ref = this._sessionCache.get(key)
      if (ref !== undefined && ref.deref() === undefined) {
        this._sessionCache.delete(key)
      }
    })
  }

  get (sessionKey) {
    const ref = this._sessionCache.get(sessionKey)
    return ref ? ref.deref() : null
  }

  set (sessionKey, session) {
    if (this._maxCachedSessions === 0) return
    // LRU-like 淘汰
    if (this._sessionCache.size >= this._maxCachedSessions) {
      for (const [key, ref] of this._sessionCache) {
        if (ref.deref() === undefined) {
          this._sessionCache.delete(key); return
        }
      }
      const oldest = this._sessionCache.keys().next()
      if (!oldest.done) { this._sessionCache.delete(oldest.value) }
    }
    this._sessionCache.set(sessionKey, new WeakRef(session))
    this._sessionRegistry.register(session, sessionKey)  // 注册 FinalizationRegistry
  }
}
```

**设计要点**：
- `WeakRef` 避免缓存阻止 TLS Session 被 GC
- `FinalizationRegistry` 在 session 被 GC 时自动清理 Map 条目
- 默认 `maxCachedSessions=100`

### 17.3 TLS 连接分支

```javascript
// lib/core/connect.js:73-102
if (protocol === 'https:') {
  if (!tls) { tls = require('node:tls') }   // 条件加载，非必需环境跳过
  servername = servername || options.servername || util.getServerName(host) || null

  const sessionKey = servername || hostname
  const session = customSession || sessionCache.get(sessionKey) || null

  port = port || 443

  socket = tls.connect({
    highWaterMark: 16384,        // TLS 不能更大
    ...options,
    servername,                  // SNI
    session,                     // TLS Session 复用
    localAddress,
    ALPNProtocols: allowH2
      ? (preferH2 ? ['h2', 'http/1.1'] : ['http/1.1', 'h2'])
      : ['http/1.1'],
    socket: httpSocket,          // 升级已有 socket（代理隧道）
    port,
    host: hostname
  })

  socket.on('session', function (session) {
    sessionCache.set(sessionKey, session)  // 缓存新 session
  })
}
```

**关键参数**：
- `servername`：SNI 字段，优先级 > options.servername > host 解析
- `ALPNProtocols`：`allowH2` 控制是否协商 H2，`preferH2` 控制优先级
- `session`：TLS Session 复用，减少握手开销
- `httpSocket`：代理隧道场景下的升级

### 17.4 明文 TCP 分支

```javascript
// lib/core/connect.js:103-132
} else {
  assert(!httpSocket, 'httpSocket can only be sent on TLS update')
  port = port || 80

  const connectOptions = {
    highWaterMark: 64 * 1024,    // 与 Node.js fs stream 一致
    ...options,
    localAddress,
    port,
    host: hostname
  }

  // servername ≠ hostname 时（SNI/CDN 场景），自定义 lookup
  const family = net.isIP(hostname)
  if (family !== 0 && servername && servername !== hostname) {
    connectOptions.host = servername
    connectOptions.lookup = (_hostname, lookupOptions, cb) => {
      // 直接返回原始 hostname，跳过 DNS
      if (lookupOptions.all) {
        cb(null, [{ address: hostname, family }])
      } else { cb(null, hostname, family) }
    }
  }

  socket = net.connect(connectOptions)
  if (useH2c === true) { socket.alpnProtocol = 'h2' }  // 强制 H2C
}
```

### 17.5 Keep-Alive 与超时

```javascript
// lib/core/connect.js:134-163
// TCP Keep-Alive
if (options.keepAlive == null || options.keepAlive) {
  const keepAliveInitialDelay = options.keepAliveInitialDelay === undefined
    ? 60e3 : options.keepAliveInitialDelay
  socket.setKeepAlive(true, keepAliveInitialDelay)
}

// 连接超时
const clearConnectTimeout = util.setupConnectTimeout(
  new WeakRef(socket), { timeout, hostname, port }
)

socket
  .setNoDelay(true)              // 禁用 Nagle
  .once(protocol === 'https:' ? 'secureConnect' : 'connect', function () {
    queueMicrotask(clearConnectTimeout)
    if (callback) {
      const cb = callback; callback = null
      cb(null, this)
    }
  })
  .on('error', function (err) {
    queueMicrotask(clearConnectTimeout)
    if (callback) {
      const cb = callback; callback = null
      cb(maybeNormalizeConnectError(err, this, { timeout, hostname, port }))
    }
  })
```

### 17.6 Windows 特殊处理

```javascript
// lib/core/connect.js:863-900（位于 lib/core/util.js）
const setupConnectTimeout = process.platform === 'win32'
  ? (socketWeakRef, opts) => {
      const fastTimer = timers.setFastTimeout(() => {
        s1 = setImmediate(() => {
          s2 = setImmediate(() => onConnectTimeout(socketWeakRef.deref(), opts))
        })
      }, opts.timeout)
      return () => {
        timers.clearFastTimeout(fastTimer)
        clearImmediate(s1); clearImmediate(s2)
      }
    }
  : (socketWeakRef, opts) => {
      const fastTimer = timers.setFastTimeout(() => {
        s1 = setImmediate(() => onConnectTimeout(socketWeakRef.deref(), opts))
      }, opts.timeout)
      return () => {
        timers.clearFastTimeout(fastTimer)
        clearImmediate(s1)
      }
    }
```

Windows 需要额外一层 `setImmediate` 嵌套，因为 Windows socket 实现差异。

### 17.7 AggregateError 归一化

```javascript
// lib/core/connect.js:172-190
function maybeNormalizeConnectError (err, socket, opts) {
  if (
    err instanceof AggregateError &&
    (err.code === 'ETIMEDOUT' || err.errors.some((e) => e?.code === 'ETIMEDOUT'))
  ) {
    let message = 'Connect Timeout Error'
    if (Array.isArray(socket.autoSelectFamilyAttemptedAddresses)) {
      message += ` (attempted addresses: ${socket.autoSelectFamilyAttemptedAddresses.join(', ')},`
    } else {
      message += ` (attempted address: ${opts.hostname}:${opts.port},`
    }
    message += ` timeout: ${opts.timeout}ms)`
    const wrapped = new ConnectTimeoutError(message)
    wrapped.cause = err
    return wrapped
  }
  return err
}
```

Node.js `autoSelectFamily` 失败时抛出 `AggregateError`，undici 将其归一化为 `ConnectTimeoutError`，保持错误语义一致。

---

## 18. Request 核心类完整解析 — 构造/Header/Body 生命周期

`lib/core/request.js` 共 547 行，是 undici 请求模型的核心。

### 18.1 Request 构造函数签名

```javascript
// lib/core/request.js:98-115
constructor (origin, {
  path, method, body, headers, query,
  idempotent, blocking, upgrade,
  headersTimeout, bodyTimeout, reset, expectContinue,
  servername, throwOnError, maxRedirections, typeOfService
}, handler)
```

### 18.2 参数校验矩阵

| 参数 | 校验规则 | 错误类型 |
|------|---------|---------|
| `path` | 必须为字符串；以 `/` 开头或绝对 URL 或 CONNECT | `InvalidArgumentError` |
| `method` | 必须为字符串；合法 HTTP token 或已知方法 | `InvalidArgumentError` |
| `upgrade` | 若存在必须为字符串 + 合法 header value | `InvalidArgumentError` |
| `headersTimeout` | 若存在必须是非负有限数 | `InvalidArgumentError` |
| `bodyTimeout` | 若存在必须是非负有限数 | `InvalidArgumentError` |
| `reset` | 若存在必须是 boolean | `InvalidArgumentError` |
| `expectContinue` | 若存在必须是 boolean | `InvalidArgumentError` |
| `throwOnError` | 若存在必须是 boolean | `InvalidArgumentError` |
| `maxRedirections` | 必须为 0 或 null（由拦截器处理） | `InvalidArgumentError` |
| `typeOfService` | 0-255 整数 | `InvalidArgumentError` |

### 18.3 Body 类型分发

```javascript
// lib/core/request.js:180-213
if (body == null) {
  this.body = null
} else if (isStream(body)) {
  this.body = body
  // 非 autoDestroy 的 stream 注册 end → destroy
  const rState = this.body._readableState
  if (!rState || !rState.autoDestroy) {
    this.endHandler = function autoDestroy () { destroy(this) }
    this.body.on('end', this.endHandler)
  }
  // 错误转发
  this.errorHandler = err => {
    if (this.abort) { this.abort(err) } else { this.error = err }
  }
  this.body.on('error', this.errorHandler)
} else if (isBuffer(body)) {
  this.body = body.byteLength ? body : null
} else if (ArrayBuffer.isView(body)) {
  this.body = body.buffer.byteLength ? Buffer.from(body.buffer, body.byteOffset, body.byteLength) : null
} else if (body instanceof ArrayBuffer) {
  this.body = body.byteLength ? Buffer.from(body) : null
} else if (typeof body === 'string') {
  this.body = body.length ? Buffer.from(body) : null
} else if (isFormDataLike(body) || isIterable(body) || isBlobLike(body)) {
  this.body = body
} else {
  throw new InvalidArgumentError('body must be a string, a Buffer, a Readable stream, an iterable, or an async iterable')
}
```

**Body 类型处理**：

| 输入类型 | 处理 | 备注 |
|---------|------|------|
| `null/undefined` | `this.body = null` | 无 body |
| `Readable stream` | 直接引用 + 注册 end/error 监听 | 流式发送 |
| `Buffer` | 空 buffer → null | 直接发送 |
| `TypedArray/ArrayBuffer` | 转 Buffer | 空 → null |
| `string` | `Buffer.from(string)` | 空字符串 → null |
| `FormData/Iterable/Blob` | 直接引用 | 由拦截器处理 |

### 18.4 Header 处理 — processHeader

```javascript
// lib/core/request.js:446-544
function processHeader (request, key, val) {
  // 1. 值校验
  if (val && (typeof val === 'object' && !Array.isArray(val))) {
    throw new InvalidArgumentError(`invalid ${key} header`)
  }

  // 2. 名称规范化（查表 → toLowerCase → 校验 HTTP token）
  let headerName = headerNameLowerCasedRecord[key]
  if (headerName === undefined) {
    headerName = key.toLowerCase()
    if (headerNameLowerCasedRecord[headerName] === undefined && !isValidHTTPToken(headerName)) {
      throw new InvalidArgumentError('invalid header key')
    }
  }

  // 3. 值规范化（数组/字符串/null/原始类型）
  if (Array.isArray(val)) { /* 逐项校验 */ }
  else if (typeof val === 'string') { /* 校验 */ }
  else if (val === null) { val = '' }
  else { val = `${val}`; /* 校验 */ }

  // 4. 特殊头处理
  if (headerName === 'host') {
    if (request.host !== null) throw new InvalidArgumentError('duplicate host header')
    request.host = val
  } else if (headerName === 'content-length') {
    if (request.contentLength !== null) throw new InvalidArgumentError('duplicate content-length header')
    request.contentLength = parseInt(val, 10)
  } else if (request.contentType === null && headerName === 'content-type') {
    request.contentType = val
    request.headers.push(key, val)
  } else if (headerName === 'transfer-encoding' || headerName === 'keep-alive' || headerName === 'upgrade') {
    throw new InvalidArgumentError(`invalid ${headerName} header`)   // 禁止用户设置
  } else if (headerName === 'connection') {
    // 解析 connection tokens
    for (const token of value.toLowerCase().split(',')) {
      if (trimmed === 'close') { request.reset = true }
    }
  } else if (headerName === 'expect') {
    throw new NotSupportedError('expect header not supported')
  } else {
    request.headers.push(key, val)
  }
}
```

**特殊 Header 处理**：

| Header | 行为 |
|--------|------|
| `Host` | 存入 `request.host`，不进入 headers 数组；禁止重复 |
| `Content-Length` | 解析为整数存入 `request.contentLength`；禁止重复 |
| `Content-Type` | 首次设置存入 `request.contentType`，同时 push 到 headers |
| `Transfer-Encoding` / `Keep-Alive` / `Upgrade` | **禁止用户设置**，抛 InvalidArgumentError |
| `Connection` | 解析 tokens；`close` → `request.reset = true` |
| `Expect` | 抛 NotSupportedError |

### 18.5 RequestController — 背压/中止控制

```javascript
// lib/core/request.js:50-95
class RequestController {
  #paused = false
  #reason = null
  #aborted = false
  #abort

  [kResume] = null    // 外部注入的 resume 回调
  rawHeaders = null
  rawTrailers = null

  pause () { this.#paused = true }

  resume () {
    if (this.#paused) {
      this.#paused = false
      this[kResume]?.()   // 触发外部 resume
    }
  }

  abort (reason) {
    if (!this.#aborted) {
      this.#aborted = true
      this.#reason = reason
      this.#abort(reason)
    }
  }

  get aborted () { return this.#aborted }
  get reason () { return this.#reason }
  get paused () { return this.#paused }
}
```

### 18.6 Request 生命周期回调

```javascript
// lib/core/request.js:282-438
onBodySent (chunk)         → channels.bodyChunkSent + handler.onBodySent
onRequestSent ()           → channels.bodySent + handler.onRequestSent
onRequestStart (abort, ctx)→ 创建 RequestController + handler.onRequestStart
onResponseStarted ()       → handler.onResponseStarted
onResponseStart (...)      → channels.headers + 注入 resume/rawHeaders + handler.onResponseStart
onResponseData (chunk)     → channels.bodyChunkReceived + handler.onResponseData
onRequestUpgrade (...)     → handler.onRequestUpgrade
onResponseEnd (trailers)   → onFinally + channels.trailers + handler.onResponseEnd
onResponseError (error)    → onFinally + channels.error + handler.onResponseError
onFinally ()               → 移除 body 的 error/end 监听器
```

**关键设计**：
- 每个回调都先发布 diagnostics_channel，再调用 handler
- `onResponseStart` 注入 `controller[kResume] = resume`，使 handler 能控制背压
- `onFinally` 清理 body 监听器，避免内存泄漏
- `assert(!this.aborted)` / `assert(!this.completed)` 防止状态机越界

### 18.7 属性总览

| 属性 | 类型 | 说明 |
|------|------|------|
| `method` | string | HTTP 方法 |
| `path` | string | 路径（含 query） |
| `origin` | string | 协议+主机+端口 |
| `protocol` | string | `http:` 或 `https:` |
| `headers` | Array | `[name, value, name, value, ...]` |
| `body` | Buffer/Stream/... | 请求体 |
| `host` | string/null | 解析后的 Host 头 |
| `contentLength` | number/null | 解析后的 Content-Length |
| `contentType` | string/null | 解析后的 Content-Type |
| `idempotent` | boolean | 是否幂等（默认 HEAD/GET/QUERY） |
| `blocking` | boolean | 是否阻塞（默认非 HEAD） |
| `reset` | boolean/null | Connection: close |
| `upgrade` | string/null | 协议升级 |
| `servername` | string/null | SNI |
| `headersTimeout` | number | 头超时 |
| `bodyTimeout` | number | 体超时 |
| `expectContinue` | boolean | H2 用 |
| `typeOfService` | number | IP TOS |
| `abort` | function/null | 中止回调 |
| `completed` | boolean | 请求完成 |
| `aborted` | boolean | 请求中止 |

---

## 19. FastTimer 与时间管理 — 自研低精度定时器全解

`lib/util/timers.js` 共 425 行，实现低精度、事件循环友好的定时器。

### 19.1 设计动机

Node.js 原生 `setTimeout` 在事件循环阻塞时会"追赶"——阻塞 5s 后到期立即触发。undici 的 headers/body 超时**不希望**这种行为：事件循环阻塞时，连接可能仍然正常，只是处理慢，不应误杀。

### 19.2 核心参数

```javascript
// lib/util/timers.js:22-40
let fastNow = 0              // 内部时钟，自增不依赖系统时间
const RESOLUTION_MS = 1e3    // 目标分辨率 1000ms
const TICK_MS = (RESOLUTION_MS >> 1) - 1   // = 499ms
```

### 19.3 4 态状态机

```javascript
// lib/util/timers.js:78-108
const NOT_IN_LIST = -2       // 不在 fastTimers 数组中
const TO_BE_CLEARED = -1     // 待清除（下一 tick 移除）
const PENDING = 0            // 刚 refresh，下一 tick 设置 _idleStart
const ACTIVE = 1             // 活跃，等待到期
```

```
                  refresh()
   ┌──────────┐ ──────▶ ┌─────────┐
   │NOT_IN_LIST│         │ PENDING │
   └──────────┘         └─────────┘
        ▲                     │ onTick 设置 _idleStart
        │                     ▼
   ┌──────────┐         ┌────────┐
   │TO_BE_    │ ◀────── │ ACTIVE │  (fastNow >= _idleStart + _idleTimeout)
   │CLEARED   │ onTick  └────────┘
   └──────────┘              │
        │                    │ 到期 → _onTimeout()
        └────── 移除 ────────┘
```

### 19.4 onTick 批量处理

```javascript
// lib/util/timers.js:115-188
function onTick () {
  fastNow += TICK_MS           // 自增时钟，不依赖系统时间

  let idx = 0, len = fastTimers.length
  while (idx < len) {
    const timer = fastTimers[idx]

    if (timer._state === PENDING) {
      timer._idleStart = fastNow - TICK_MS
      timer._state = ACTIVE
    } else if (timer._state === ACTIVE &&
               fastNow >= timer._idleStart + timer._idleTimeout) {
      timer._state = TO_BE_CLEARED
      timer._idleStart = -1
      timer._onTimeout(timer._timerArg)
    }

    if (timer._state === TO_BE_CLEARED) {
      timer._state = NOT_IN_LIST
      if (--len !== 0) {
        fastTimers[idx] = fastTimers[len]   // 尾部替换法 O(1) 删除
      }
    } else { ++idx }
  }

  fastTimers.length = len

  if (fastTimers.length !== 0) {
    refreshTimeout()   // 继续调度
  }
}
```

**关键设计**：
- 单个 `setTimeout(onTick, 499)` 驱动所有 FastTimer
- 尾部替换法 O(1) 删除，避免数组移动
- 无活跃 timer 时停止调度，节省资源

### 19.5 FastTimer 类

```javascript
// lib/util/timers.js:213-320
class FastTimer {
  [kFastTimer] = true
  _state = NOT_IN_LIST
  _idleTimeout = -1
  _idleStart = -1
  _onTimeout
  _timerArg

  constructor (callback, delay, arg) {
    this._onTimeout = callback
    this._idleTimeout = delay
    this._timerArg = arg
    this.refresh()
  }

  refresh () {
    if (this._state === NOT_IN_LIST) {
      fastTimers.push(this)
    }
    if (!fastNowTimeout || fastTimers.length === 1) {
      refreshTimeout()
    }
    this._state = PENDING
  }

  clear () {
    this._state = TO_BE_CLEARED
    this._idleStart = -1
  }
}
```

### 19.6 导出 API

```javascript
// lib/util/timers.js:326-425
module.exports = {
  setTimeout (callback, delay, arg) {
    // delay <= 1000ms 用原生 setTimeout，否则用 FastTimer
    return delay <= RESOLUTION_MS
      ? setTimeout(callback, delay, arg)
      : new FastTimer(callback, delay, arg)
  },
  clearTimeout (timeout) {
    if (timeout[kFastTimer]) { timeout.clear() }
    else { clearTimeout(timeout) }
  },
  setFastTimeout (callback, delay, arg) { return new FastTimer(callback, delay, arg) },
  clearFastTimeout (timeout) { timeout.clear() },
  now () { return fastNow },
  tick (delay = 0) { /* 测试用 */ },
  reset () { /* 测试用 */ },
  kFastTimer
}
```

### 19.7 与原生 setTimeout 对比

| 特性 | 原生 setTimeout | FastTimer |
|------|----------------|-----------|
| 精度 | ~1-4ms | ~500ms |
| 事件循环阻塞 | 阻塞后"追赶" | 阻塞期间时钟不前进 |
| 系统时钟跳变 | 受影响 | 不受影响 |
| 适用场景 | 精确定时 | headers/body 超时 |
| 资源开销 | 每个 timer 一个 Timeout | 共享一个 setTimeout |

---

## 20. FixedQueue 数据结构 — Node.js 级高性能环形缓冲

`lib/dispatcher/fixed-queue.js` 共 135 行，是 Node.js `internal/fixed_queue.js` 的移植。

### 20.1 整体架构

```
head                                                       tail
  │                                                         │
  v                                                         v
+-----------+ <-----\       +-----------+ <------\         +-----------+
|  [null]   |        \----- |   next    |         \------- |   next    |
+-----------+               +-----------+                  +-----------+
|   item    | <-- bottom    |   item    | <-- bottom       | undefined |
|   item    |               |   item    |                  | undefined |
|   item    |               |   item    |                  | undefined |
|   item    |               |   item    |                  | undefined |
|   item    |               |   item    |       bottom --> |   item    |
|   item    |               |   item    |                  |   item    |
|    ...    |               |    ...    |                  |    ...    |
|   item    |               |   item    |                  |   item    |
|   item    |               |   item    |                  |   item    |
| undefined | <-- top       |   item    |                  |   item    |
| undefined |               |   item    |                  |   item    |
| undefined |               | undefined | <-- top  top --> | undefined |
+-----------+               +-----------+                  +-----------+
```

### 20.2 核心常量

```javascript
// lib/dispatcher/fixed-queue.js:5-7
const kSize = 2048              // 每个环形缓冲区槽位数
const kMask = kSize - 1         // 位掩码，用于取模
```

**为什么是 2048**：V8 6.0-6.6 测试最佳值，必须为 2 的幂以便位运算取模。

### 20.3 FixedCircularBuffer

```javascript
// lib/dispatcher/fixed-queue.js:61-98
class FixedCircularBuffer {
  bottom = 0
  top = 0
  list = new Array(kSize).fill(undefined)
  next = null

  isEmpty () { return this.top === this.bottom }

  isFull () { return ((this.top + 1) & kMask) === this.bottom }

  push (data) {
    this.list[this.top] = data
    this.top = (this.top + 1) & kMask
  }

  shift () {
    const nextItem = this.list[this.bottom]
    if (nextItem === undefined) { return null }
    this.list[this.bottom] = undefined   // ← 允许 GC
    this.bottom = (this.bottom + 1) & kMask
    return nextItem
  }
}
```

**空/满判定**：
- 空：`top === bottom`
- 满：`(top + 1) & mask === bottom`（浪费一个槽位换取 O(1) 判定）

### 20.4 FixedQueue 主类

```javascript
// lib/dispatcher/fixed-queue.js:103-135
module.exports = class FixedQueue {
  constructor () {
    this.head = this.tail = new FixedCircularBuffer()
  }

  isEmpty () { return this.head.isEmpty() }

  push (data) {
    if (this.head.isFull()) {
      // head 满：创建新 buffer，链接到 next
      this.head = this.head.next = new FixedCircularBuffer()
    }
    this.head.push(data)
  }

  shift () {
    const tail = this.tail
    const next = tail.shift()
    if (tail.isEmpty() && tail.next !== null) {
      // tail 空且有下一个：推进 tail
      this.tail = tail.next
      tail.next = null
    }
    return next
  }
}
```

### 20.5 摊还 O(1) 分析

| 操作 | 最坏 | 摊还 |
|------|------|------|
| `push` | O(kSize)（分配新 buffer） | O(1) |
| `shift` | O(1) | O(1) |
| `isEmpty` | O(1) | O(1) |

**摊还关键**：每 2048 次 push 才分配一次新 buffer，均摊到每次 push 是 O(1)。

### 20.6 在 undici 中的应用

`FixedQueue` 在 `lib/dispatcher/client.js` 中作为请求队列：

```javascript
// lib/dispatcher/client.js:339-354
this[kQueue] = new FixedQueue()
this[kRunningIdx] = 0
this[kPendingIdx] = 0
```

三段式队列：`| complete | running | pending |`，通过 `kRunningIdx` 和 `kPendingIdx` 游标划分。

---

## 21. TernarySearchTree 三叉搜索树 — 大小写不敏感 Header 查找

`lib/core/tree.js` 共 160 行，实现 ASCII 大小写不敏感的三叉搜索树。

### 21.1 数据结构

```javascript
// lib/core/tree.js:8-38
class TstNode {
  value = null
  left = null      // 小于当前字符
  middle = null    // 等于当前字符，下一位置
  right = null     // 大于当前字符
  code             // 当前节点字符的 ASCII 码

  constructor (key, value, index) {
    const code = this.code = key.charCodeAt(index)
    if (code > 0x7F) { throw new TypeError('key must be ascii string') }
    if (key.length !== ++index) {
      this.middle = new TstNode(key, value, index)
    } else {
      this.value = value
    }
  }
}
```

### 21.2 插入操作

```javascript
// lib/core/tree.js:45-85
add (key, value) {
  let index = 0
  let node = this
  while (true) {
    const code = key.charCodeAt(index)
    if (code > 0x7F) { throw new TypeError('key must be ascii string') }

    if (node.code === code) {
      if (length === ++index) { node.value = value; break }
      else if (node.middle !== null) { node = node.middle }
      else { node.middle = new TstNode(key, value, index); break }
    } else if (node.code < code) {
      if (node.left !== null) { node = node.left }
      else { node.left = new TstNode(key, value, index); break }
    } else {
      if (node.right !== null) { node = node.right }
      else { node.right = new TstNode(key, value, index); break }
    }
  }
}
```

### 21.3 搜索操作（大小写折叠）

```javascript
// lib/core/tree.js:91-121
search (key) {
  const keylength = key.length
  let index = 0
  let node = this
  while (node !== null && index < keylength) {
    let code = key[index]
    // 大小写折叠：A-Z → a-z
    if (code <= 0x5a && code >= 0x41) {
      code |= 32    // 0x41-0x5A → 0x61-0x7A，仅翻转第 5 位
    }
    while (node !== null) {
      if (code === node.code) {
        if (keylength === ++index) { return node }
        node = node.middle
        break
      }
      node = node.code < code ? node.left : node.right
    }
  }
  return null
}
```

**大小写折叠原理**：ASCII 编码中 `A-Z` = 0x41-0x5A，`a-z` = 0x61-0x7A，差异仅在第 5 位（0x20）。`code |= 32` 单条位操作完成转换。

### 21.4 TernarySearchTree 包装

```javascript
// lib/core/tree.js:124-148
class TernarySearchTree {
  node = null

  insert (key, value) {
    if (this.node === null) {
      this.node = new TstNode(key, value, 0)
    } else {
      this.node.add(key, value)
    }
  }

  lookup (key) {
    return this.node?.search(key)?.value ?? null
  }
}
```

### 21.5 预填充已知 Header

```javascript
// lib/core/tree.js:150-160
const tree = new TernarySearchTree()

for (let i = 0; i < wellknownHeaderNames.length; ++i) {
  const key = headerNameLowerCasedRecord[wellknownHeaderNames[i]]
  tree.insert(key, key)
}
```

启动时将 100+ 个已知 HTTP 头（小写形式）预插入树。

### 21.6 使用场景

```javascript
// lib/core/util.js:405-418
function headerNameToString (value) {
  return typeof value === 'string'
    ? headerNameLowerCasedRecord[value] ?? value.toLowerCase()
    : tree.lookup(value) ?? value.toString('latin1').toLowerCase()
}

function bufferToLowerCasedHeaderName (value) {
  return tree.lookup(value) ?? value.toString('latin1').toLowerCase()
}
```

**用途**：llhttp 解析器返回的 header name 是 `Buffer`，通过 `tree.lookup(buffer)` 快速匹配已知头，避免逐字节 `toLowerCase()`。

### 21.7 性能特征

| 操作 | 复杂度 | 说明 |
|------|--------|------|
| 插入 | O(L log Σ) | L=key 长度，Σ=字符集 |
| 查找 | O(L log Σ) | 优于哈希表的最坏情况 |
| 空间 | O(N × L) | N=key 数 |

**优势**：直接在 `Uint8Array` 上操作，跳过 UTF-8 解码；大小写折叠零开销。

---

## 22. constants.js 与 symbols.js — 常量体系与内部 Symbol 清单

### 22.1 constants.js — 已知 HTTP 头常量

```javascript
// lib/core/constants.js:6-102
const wellknownHeaderNames = [
  'Accept', 'Accept-Encoding', 'Accept-Language', 'Accept-Ranges',
  'Access-Control-Allow-Credentials', 'Access-Control-Allow-Headers',
  // ... 共 100 个标准头
  'X-XSS-Protection'
]
```

**100 个标准头**：覆盖 RFC 7230-7235、CORS、CSP、HSTS、WebSocket 等。

```javascript
// lib/core/constants.js:104-115
const headerNameLowerCasedRecord = {}
Object.setPrototypeOf(headerNameLowerCasedRecord, null)   // null 原型防污染

for (let i = 0; i < wellknownHeaderNames.length; ++i) {
  const key = wellknownHeaderNames[i]
  const lowerCasedKey = key.toLowerCase()
  headerNameLowerCasedRecord[key] = headerNameLowerCasedRecord[lowerCasedKey] = lowerCasedKey
}
```

**双向映射**：`Accept` → `accept`，`accept` → `accept`。null 原型防止 `Object.prototype` 污染。

### 22.2 symbols.js — 内部 Symbol 清单

`lib/core/symbols.js` 共 77 行，导出 50+ 个内部 Symbol，避免属性名冲突。

**按用途分组**：

| 类别 | Symbol | 用途 |
|------|--------|------|
| **生命周期** | `kClose`, `kDestroy`, `kConnecting`, `kConnected`, `kClosed`, `kDestroyed` | 连接/Client 状态 |
| **队列** | `kQueue`, `kRunning`, `kPending`, `kRunningIdx`, `kPendingIdx`, `kSize` | 请求队列 |
| **调度** | `kDispatch`, `kResume`, `kConnect`, `kBusy`, `kNeedDrain` | 调度控制 |
| **协议** | `kHTTP2Session`, `kHTTP2Options`, `kHTTP2SessionState`, `kHTTP2Stream`, `kHTTPContext` | H2 相关 |
| **超时** | `kHeadersTimeout`, `kBodyTimeout`, `kKeepAlive`, `kKeepAliveDefaultTimeout`, `kKeepAliveMaxTimeout`, `kKeepAliveTimeoutThreshold`, `kKeepAliveTimeoutValue` | 超时配置 |
| **连接** | `kSocket`, `kConnector`, `kClients`, `kClient`, `kParser`, `kCounter` | 连接管理 |
| **请求** | `kUrl`, `kWriting`, `kHost`, `kServerName`, `kLocalAddress`, `kHostHeader` | 请求属性 |
| **Body** | `kBody`, `kBodyUsed`, `kBlocking`, `kReset` | Body 状态 |
| **统计** | `kFree`, `kQueued`, `kConnected`, `kMaxRequests`, `kMaxResponseSize`, `kMaxHeadersSize` | 池统计 |
| **代理** | `kProxy`, `kNoProxyAgent`, `kHttpProxyAgent`, `kHttpsProxyAgent`, `kSocks5ProxyAgent` | 代理类型 |
| **H2 扩展** | `kEnableConnectProtocol`, `kRemoteSettings`, `kMaxConcurrentStreams`, `kHTTP2InitialWindowSize`, `kHTTP2ConnectionWindowSize`, `kPingInterval`, `kHostAuthority` | H2 高级 |
| **其他** | `kRetryHandlerDefaultRetry`, `kConstruct`, `kListeners`, `kOnError`, `kOnDestroyed`, `kStrictContentLength`, `kMaxRedirections`, `kNoRef`, `kPipelining` | 杂项 |

**关键设计**：
- 使用 `Symbol()` 而非字符串键，避免与用户数据冲突
- `kDestroyed` 使用 `Symbol.for('nodejs.stream.destroyed')` 与 Node.js 共享
- 集中管理，便于重构和调试

---

## 23. 容错与超时体系总览 — 四维超时配置与实现

### 23.1 四维超时

| 超时 | 默认值 | 作用域 | 定时器类型 | 错误类 |
|------|--------|--------|----------|--------|
| `connectTimeout` | 10,000ms | 连接建立 | FastTimer | `ConnectTimeoutError` |
| `headersTimeout` | 30,000ms | 等待响应头 | FastTimer | `HeadersTimeoutError` |
| `bodyTimeout` | 30,000ms | 等待响应体 | FastTimer | `BodyTimeoutError` |
| `socketConnectTimeout` / `idleTimeout` | 由 keepAlive 控制 | 空闲回收 | 原生定时器 | `InformationalError` |

### 23.2 超时位掩码

```javascript
// lib/dispatcher/client-h1.js:198-208
const USE_NATIVE_TIMER = 0
const USE_FAST_TIMER = 1

const TIMEOUT_HEADERS = 2 | USE_FAST_TIMER     // = 3
const TIMEOUT_BODY = 4 | USE_FAST_TIMER        // = 5
const TIMEOUT_KEEP_ALIVE = 8 | USE_NATIVE_TIMER // = 8
```

`type & USE_FAST_TIMER` 快速判断是否使用 FastTimer。

### 23.3 超时配置传递链

```
Client/Pool 构造
    │
    ▼
connect.js: buildConnector({ timeout: connectTimeout })
    │
    ▼
util.setupConnectTimeout(socket, { timeout })  ← connectTimeout
    │
    ▼
Request 构造
    │
    ├─ headersTimeout ──▶ Parser.setTimeout(TIMEOUT_HEADERS, headersTimeout)
    ├─ bodyTimeout    ──▶ Parser.setTimeout(TIMEOUT_BODY, bodyTimeout)
    └─ reset          ──▶ Connection: close
```

### 23.4 Parser 超时处理

```javascript
// lib/dispatcher/client-h1.js:833（简化）
function onParserTimeout () {
  const socket = this[kSocket]
  if (!socket || socket.destroyed) return

  const timer = this[kTimeoutTimer]
  if (timer) {
    this[kTimeoutTimer] = null
    if (timer & USE_FAST_TIMER) {
      timers.clearFastTimeout(timer)
    } else {
      clearTimeout(timer)
    }
  }

  const timeoutType = this[kTimeoutType]
  switch (timeoutType) {
    case TIMEOUT_HEADERS:
      destroy(socket, new HeadersTimeoutError())
      break
    case TIMEOUT_BODY:
      destroy(socket, new BodyTimeoutError())
      break
    case TIMEOUT_KEEP_ALIVE:
      destroy(socket, new InformationalError('socket idle timeout'))
      break
  }
}
```

### 23.5 超时与事件循环的关系

```
事件循环正常：
  时间轴 ─────────────────────────────────────────▶
  connectTimeout ────────▶ 触发 ConnectTimeoutError

事件循环阻塞 5s：
  时间轴 ─────────────────────────────────────────▶
  wall clock   0s ──────────────▶ 5s ──────────▶ 10s
  fastNow      0s ─────▶ 2s ─────▶ 4s ────────▶ 6s
                        (阻塞期间 fastNow 不前进)
  connectTimeout ────────▶ 实际触发在 wall clock 15s
```

**设计意图**：headers/body 超时感知事件循环延迟，避免误杀正常连接。keep-alive 超时使用墙钟时间，精确回收空闲连接。

### 23.6 超时配置示例

```javascript
import { Client } from 'undici'

const client = new Client('https://api.example.com', {
  connectTimeout: 5000,      // 5s 连接超时
  headersTimeout: 10000,    // 10s 等待响应头
  bodyTimeout: 30000,       // 30s 等待响应体
  keepAliveTimeout: 60000,  // 60s 空闲回收
  keepAliveMaxTimeout: 600000,  // 最大 10 分钟
  keepAliveTimeoutThreshold: 1000,  // 1s 阈值
  pipelining: 1,            // H1 pipelining
  maxResponseSize: 10 * 1024 * 1024  // 10MB
})
```

### 23.7 错误恢复策略

| 错误 | 自动恢复 | 重试条件 |
|------|---------|---------|
| `ConnectTimeoutError` | retry 拦截器 | 错误码白名单 |
| `HeadersTimeoutError` | retry 拦截器 | 错误码白名单 |
| `BodyTimeoutError` | retry 拦截器 | 错误码白名单 |
| `SocketError` | retry 拦截器 | 错误码白名单 |
| `InformationalError` | 不重试 | 非致命，仅关闭当前连接 |
| `ClientDestroyedError` | 不重试 | 客户端已销毁 |
| `ClientClosedError` | 不重试 | 客户端已关闭 |

---

## 24. 对 laew 的借鉴价值与路线图

本节基于前 23 节的全面分析（尤其第 13-23 节核心模块深度专题），给出 laew 可落地的借鉴方案。

### P0（立即借鉴 — 高价值、低成本）

| 维度 | undici 机制 | laew 落地 | 参考章节 |
|------|-----------|---------|---------|
| **错误体系** | `Symbol.hasInstance` 防伪 + 26 种错误类分组 | Rust `thiserror` + `ErrorKind` enum + `From` trait；分超时/连接/内容/生命周期 4 组 | §13 errors.js |
| **超时分层** | headers/body 用 FastTimer，keep-alive 用原生 timer | LLM 请求用 cooperative cancel，心跳用 tokio::time wall clock | §19 FastTimer, §23 超时体系 |
| **compose 拦截器** | `compose + Proxy dispatch` 中间件链 | Rust trait object + `Vec<Box<dyn Handler>>`，可组合校验/日志/重试 | §6 拦截器体系 |
| **FixedQueue** | 2048 槽位循环缓冲 + 摊还 O(1) | laew 工具调用队列用环形缓冲，支持背压 | §20 FixedQueue |
| **参数校验** | `processHeader` 的严格模式校验矩阵 | Bash/Read/Write 工具的 `input_schema` 严格模式校验 | §18 Request |

### P1（短期借鉴 — 1-2 周）

| 维度 | undici 机制 | laew 落地 | 参考章节 |
|------|-----------|---------|---------|
| **TernarySearchTree** | 字节级大小写不敏感查找 | laew 斜杠命令匹配（`/help`/`/provider` 等） | §21 TST |
| **RetryController 代理** | 稳定代理转发到活跃连接 | laew 工具调用重试的取消/暂停语义 | §14 retry-handler |
| **FastTimer 批量处理** | 单个 setTimeout 驱动 N 个定时器 | tokio 心跳管理用一个 interval 驱动多个超时检查 | §19 FastTimer |
| **容错分组** | 5 类错误分组（客户端/连接/超时/内容/生命周期） | laew 工具错误分组 + 对应恢复策略 | §13.3 分组体系 |
| **Header 常量预填充** | 100 个已知头预插 TST | laew 工具名/命令名预构建匹配树 | §22 constants.js |

### P2（中期借鉴 — 1-2 月）

| 维度 | undici 机制 | laew 落地 | 参考章节 |
|------|-----------|---------|---------|
| **diagnostics_channel** | 5 组 16 channel 分层追踪 | laew connection/request/tool_call 三层 tracing | §16 diagnostics |
| **重定向语义** | 状态码 × 方法矩阵 + 循环检测 | laew 多步骤工作流的失败回流/重路由 | §15 redirect-handler |
| **部分内容恢复** | Range + If-Match 续传 | laew 大文件/流式输出的断点续传 | §14.5 部分恢复 |
| **SessionCache** | WeakRef + FinalizationRegistry | laew LLM 连接池的 session 复用 + GC 清理 | §17.2 SessionCache |
| **Symbol 隔离** | 50+ 内部 Symbol 避免属性冲突 | laew 内部状态与用户数据隔离 | §22 symbols.js |
| **Body 类型分发** | 6 种 Body 类型统一接口 | laew 工具输入的多态分发（文件/文本/流） | §18.3 Body 分发 |

### P3（长期跟踪）

| 维度 | undici 机制 | laew 评估 |
|------|-----------|---------|
| **HTTP/2 多路复用** | client-h2.js 完整实现 | laew 暂不需要（单 LLM 连接足够） |
| **Mock 录制回放** | Snapshot 系统 | laew 端到端测试可考虑 |
| **Socks5 代理** | socks5-client.js 6 态状态机 | laew 代理需求有限 |
| **WebIDL 校验** | 完整类型转换矩阵 | laew 用 Rust 类型系统替代 |

### laew 错误体系落地方案（基于 §13）

```rust
// 参照 undici 26 种错误类的分组设计
#[derive(Debug, thiserror::Error)]
pub enum LaewError {
    // 超时组
    #[error("连接超时: {message}")]
    ConnectTimeout { message: String },

    #[error("LLM 响应超时")]
    HeadersTimeout,

    #[error("工具执行超时")]
    BodyTimeout,

    // 连接组
    #[error("Socket 错误: {source}")]
    Socket {
        #[from]
        source: std::io::Error,
    },

    // 参数组
    #[error("无效参数: {0}")]
    InvalidArgument(String),

    // 内容组
    #[error("Content-Length 不匹配")]
    ContentLengthMismatch,

    // 生命周期组
    #[error("操作已中止")]
    Abort,

    #[error("Session 已关闭")]
    Closed,
}

// 关键：错误码分级，决定是否重试
impl LaewError {
    fn retryable(&self) -> bool {
        matches!(self,
            LaewError::ConnectTimeout { .. } |
            LaewError::HeadersTimeout |
            LaewError::BodyTimeout |
            LaewError::Socket { .. }
        )
    }
}
```

### laew 超时体系落地方案（基于 §19/§23）

```rust
// 参照 undici 四维超时
struct LaewTimeouts {
    connect: Duration,      // TCP 连接超时
    headers: Duration,      // 等待 LLM 首 token 超时
    body: Duration,         // 完整响应超时
    idle: Duration,         // keep-alive 空闲回收
}

// 参照 FastTimer 的 cooperative cancel
async fn with_cooperative_timeout<F, T>(
    future: F,
    timeout: Duration,
) -> Result<T, LaewError>
where
    F: Future<Output = Result<T, LaewError>>,
{
    // 使用 tokio 的 timeout 机制，但配置事件循环感知模式
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| LaewError::HeadersTimeout)?
}
```

### laew 工具重试落地方案（基于 §14）

```rust
// 参照 RetryController 代理模式
struct RetryController {
    target: Option<AbortHandle>,
}

impl RetryController {
    fn pause(&self) { /* 暂停当前工具执行 */ }
    fn resume(&self) { /* 恢复 */ }
    fn abort(&self, reason: &str) {
        if let Some(handle) = &self.target {
            handle.abort();
        }
    }
}

// 参照默认重试策略的退避序列
fn default_backoff(attempt: u32) -> Duration {
    let base = Duration::from_millis(500);
    let max = Duration::from_secs(30);
    let factor = 2u32.pow(attempt.min(10));
    std::cmp::min(base * factor, max)
}
```

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
