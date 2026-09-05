# Undici 核心架构深度分析

> 项目：undici（https://github.com/nodejs/undici）
> 定位：Node.js 官方 HTTP/1.1 + HTTP/2 客户端库，Node.js 内置 `fetch()` 的底层实现
> 语言：JavaScript（Node.js）
> 分析日期：2026-09-05

---

## 目录

1. [项目概览](#1-项目概览)
2. [架构总图](#2-架构总图)
3. [入口与全局单例](#3-入口与全局单例)
4. [Dispatcher 体系详解](#4-dispatcher-体系详解)
   - 4.1 Dispatcher 基类
   - 4.2 DispatcherBase
   - 4.3 Client — 单连接 HTTP 客户端
   - 4.4 Pool — 多连接池
   - 4.5 PoolBase — 池基类与队列调度
   - 4.6 Agent — 顶层多 origin 路由器
   - 4.7 BalancedPool — 加权负载均衡
   - 4.8 RoundRobinPool — 轮询负载均衡
5. [HTTP/1.1 vs HTTP/2 双协议实现](#5-http11-vs-http2-双协议实现)
   - 5.1 client-h1.js — HTTP/1.1 连接上下文
   - 5.2 client-h2.js — HTTP/2 连接上下文
   - 5.3 协议选择与 ALPN 协商
   - 5.4 双协议关键差异对比
6. [llhttp WASM 解析器](#6-llhttp-wasm-解析器)
   - 6.1 WASM 模块加载
   - 6.2 Parser 类详解
   - 6.3 回调函数与 JS-WASM 桥
   - 6.4 定时器体系
7. [连接器与 TLS](#7-连接器与-tls)
8. [代理体系](#8-代理体系)
   - 8.1 ProxyAgent — HTTP CONNECT 隧道
   - 8.2 EnvHttpProxyAgent — 环境变量代理
   - 8.3 Socks5ProxyAgent — SOCKS5 代理
9. [重试机制](#9-重试机制)
10. [5 种 API 风格对比](#10-5-种-api-风格对比)
   - 10.1 request API
   - 10.2 stream API
   - 10.3 pipeline API
   - 10.4 connect API
   - 10.5 upgrade API
11. [Request 核心模型](#11-request-核心模型)
12. [错误处理体系](#12-错误处理体系)
13. [诊断钩子（diagnostics_channel）](#13-诊断钩子)
14. [FixedQueue 高性能队列](#14-fixedqueue-高性能队列)
15. [TernarySearchTree 头部快速查找](#15-ternarysearchtree-头部快速查找)
16. [Dispatcher1Wrapper 向后兼容层](#16-dispatcher1wrapper-向后兼容层)
17. [H2CClient — 明文 HTTP/2](#17-h2cclient--明文-http2)
18. [工具函数库 core/util.js](#18-工具函数库)
19. [FastTimer 快速定时器](#19-fasttimer-快速定时器)
20. [HTTP 缓存拦截器](#20-http-缓存拦截器)
21. [HTTP 日期解析器](#21-http-日期解析器)
22. [运行时特性检测](#22-运行时特性检测)
23. [Mock 测试体系](#23-mock-测试体系)
24. [统计快照与度量](#24-统计快照与度量)
25. [对 laew（Rust Agent CLI）的借鉴价值](#25-对-laew-的借鉴价值)

---

## 1. 项目概览

Undici 是 Node.js 官方维护的 HTTP 客户端库，名字取自意大利语"11"，代表 HTTP/1.1。它是 Node.js 内置 `fetch()` API 的底层实现，也被广泛用作 `axios`、`got` 等流行 HTTP 库的高性能替代方案。

**核心特性**：
- 原生 HTTP/1.1 流水线（pipelining）与 HTTP/2 多路复用（multiplexing）
- 基于 llhttp 的 WASM 解析器（非 Node.js 内置 http_parser）
- 连接池管理与负载均衡（Pool、BalancedPool、RoundRobinPool）
- 5 种 API 风格：request / stream / pipeline / connect / upgrade
- 代理支持：HTTP CONNECT、SOCKS5、环境变量自动代理
- 诊断钩子（diagnostics_channel）全覆盖
- 全局单例 Dispatcher 模式

**入口文件**：`index.js` 导出全部公开 API，`index-fetch.js` 为 Node.js 内置 fetch 的精简导出。

---

## 2. 架构总图

```
                        +------------------+
                        |     index.js     |  <-- 公开 API 入口
                        +--------+---------+
                                 |
              +------------------+------------------+
              |                  |                  |
    +---------v------+  +--------v--------+  +------v---------+
    |   API 层        |  | Dispatcher 体系 |  | Web API 层      |
    | (request/stream/ |  |                 |  | (fetch/Headers/ |
    |  pipeline/connect/|  |                 |  |  Response/WebSocket|
    |  upgrade)        |  |                 |  |  EventSource)   |
    +--------+---------+  +--------+--------+  +--------+-------+
             |                     |                     |
             |          +----------v-----------+         |
             +--------->|  Dispatcher (基类)    |<--------+
                        |  EventEmitter        |
                        +----------+-----------+
                                   |
                        +----------v-----------+
                        |   DispatcherBase      |
                        |  close/destroy/dispatch|
                        +----------+-----------+
                                   |
              +--------------------+--------------------+
              |                    |                     |
    +---------v------+   +--------v--------+   +--------v---------+
    |    Client       |   |     Pool        |   |     Agent         |
    | (单连接)         |   | (多连接池)       |   | (多origin路由器)  |
    |                 |   |                 |   |                  |
    | + client-h1.js  |   | PoolBase        |   | Map<origin, Pool>|
    | + client-h2.js  |   | FixedQueue      |   | factory 模式     |
    +---------+-------+   +-----------------+   +------------------+
              |
    +---------v---------+
    | connect.js        |
    | (TCP/TLS连接器)    |
    +-------------------+

    连接池变体:
    +-----------------+  +-------------------+  +------------------+
    | BalancedPool    |  | RoundRobinPool    |  | Pool (默认)       |
    | 加权负载均衡     |  | 轮询负载均衡       |  | 首个空闲分发      |
    +-----------------+  +-------------------+  +------------------+

    代理变体:
    +------------------+  +--------------------+  +-------------------+
    | ProxyAgent       |  | EnvHttpProxyAgent  |  | Socks5ProxyAgent  |
    | HTTP CONNECT隧道 |  | 环境变量自动代理    |  | SOCKS5协议代理     |
    +------------------+  +--------------------+  +-------------------+

    重试:
    +------------------+
    | RetryAgent       |
    | RetryHandler包装  |
    +------------------+

    核心层:
    +----------+  +----------+  +----------+  +----------+  +----------+
    | request.js|  | connect.js|  | errors.js|  | symbols.js|  | diagnostics|
    | (请求模型) |  | (连接器)  |  | (错误体系) |  | (Symbol常量)|  | (钩子)    |
    +----------+  +----------+  +----------+  +----------+  +----------+

    llhttp WASM:
    +--------------------+
    | llhttp-wasm.js     |
    | llhttp_simd-wasm.js|
    | Parser (JS层)      |
    +--------------------+
```

---

## 3. 入口与全局单例

### 3.1 index.js — 统一入口

`index.js`（`/usr/local/LsmGitOpenSource/undici/index.js`）是整个库的入口文件，承担三个核心职责：

**职责一：导出所有 Dispatcher 类型**

```javascript
module.exports.Dispatcher = Dispatcher
module.exports.Client = Client
module.exports.Pool = Pool
module.exports.BalancedPool = BalancedPool
module.exports.RoundRobinPool = RoundRobinPool
module.exports.Agent = Agent
module.exports.ProxyAgent = ProxyAgent
module.exports.Socks5ProxyAgent = Socks5ProxyAgent
module.exports.EnvHttpProxyAgent = EnvHttpProxyAgent
module.exports.RetryAgent = RetryAgent
module.exports.H2CClient = H2CClient
```

**职责二：挂载 API 到 Dispatcher.prototype**

```javascript
Object.assign(Dispatcher.prototype, api)
```

这行代码将 `request`、`stream`、`pipeline`、`connect`、`upgrade` 五个方法挂载到所有 Dispatcher 实例上。这意味着任何 Dispatcher 子类都可以直接调用 `dispatcher.request(opts, callback)`。

**职责三：创建顶层便捷函数**

```javascript
function makeDispatcher (fn) {
  return (url, opts, handler) => {
    // 解析 URL, 提取 origin/path, 设置默认 method
    const { agent, dispatcher = getGlobalDispatcher(), ...restOpts } = opts
    return fn.call(dispatcher, { ...restOpts, origin, path, method }, handler)
  }
}

module.exports.request = makeDispatcher(api.request)
module.exports.stream = makeDispatcher(api.stream)
module.exports.pipeline = makeDispatcher(api.pipeline)
module.exports.connect = makeDispatcher(api.connect)
module.exports.upgrade = makeDispatcher(api.upgrade)
```

`makeDispatcher` 是一个高阶函数，它将 URL 字符串解析为 `origin` + `path`，自动从全局 Dispatcher 获取默认 Agent，然后代理到对应的 API 方法。这使得用户可以直接写：

```javascript
const { request } = require('undici')
await request('https://example.com/api')
```

无需手动创建 Client 或 Agent。

### 3.2 lib/global.js — 全局 Dispatcher 单例

`global.js`（`/usr/local/LsmGitOpenSource/undici/lib/global.js`）实现了全局 Dispatcher 单例管理。

**核心设计**：

```javascript
const globalDispatcher = Symbol.for('undici.globalDispatcher.2')
const legacyGlobalDispatcher = Symbol.for('undici.globalDispatcher.1')
```

使用 `Symbol.for()` 而非普通 `Symbol()`，确保即使多个版本的 undici 并存（Node.js 内置 + npm 安装），也能通过相同的 key 访问同一个全局 Dispatcher。

**getGlobalDispatcher()**：

```javascript
function getGlobalDispatcher () {
  return globalThis[globalDispatcher] ?? fallbackDispatcher
}
```

优先从 `globalThis` 读取，如果 `globalThis` 被冻结（如某些沙箱环境），回退到模块级变量 `fallbackDispatcher`。

**setGlobalDispatcher(agent)**：

```javascript
function setGlobalDispatcher (agent) {
  if (!agent || typeof agent.dispatch !== 'function') {
    throw new InvalidArgumentError('Argument agent must implement Agent')
  }
  Object.defineProperty(globalThis, globalDispatcher, {
    value: agent,
    writable: true, enumerable: false, configurable: false
  })
}
```

验证 agent 必须实现 `dispatch` 方法（鸭子类型），然后将其设为不可枚举的全局属性。

**向后兼容**：同时维护 `legacyGlobalDispatcher`（版本 1），通过 `Dispatcher1Wrapper` 将新版 Dispatcher 包装为旧版接口。

**默认初始化**：

```javascript
if (getGlobalDispatcher() === undefined) {
  setGlobalDispatcher(new Agent())
}
```

模块加载时自动创建一个默认 Agent 作为全局 Dispatcher。Agent 内部会按 origin 自动创建 Pool/Client。

### 3.3 index-fetch.js — Node.js 内置 fetch 精简入口

`index-fetch.js` 是 Node.js 内置 `fetch()` 的精简导出版本，只暴露 Web API 相关的类（fetch、Headers、Response、Request、FormData、WebSocket、EventSource），不暴露底层 Dispatcher 体系。这用于 Node.js 核心代码中，避免引入不必要的依赖。

---

## 4. Dispatcher 体系详解

### 4.1 Dispatcher 基类

`lib/dispatcher/dispatcher.js` 是所有 Dispatcher 的抽象基类，继承自 `EventEmitter`。

```javascript
class Dispatcher extends EventEmitter {
  dispatch () { throw new Error('not implemented') }
  close () { throw new Error('not implemented') }
  destroy () { throw new Error('not implemented') }

  compose (...args) {
    const interceptors = Array.isArray(args[0]) ? args[0] : args
    let dispatch = this.dispatch.bind(this)
    for (const interceptor of interceptors) {
      dispatch = interceptor(dispatch)
    }
    // 用 Proxy 包装，只拦截 dispatch 属性
    return new Proxy(this, {
      get: (target, key) => key === 'dispatch' ? dispatch : target[key]
    })
  }
}
```

**关键设计点**：

1. **三个抽象方法**：`dispatch`（发送请求）、`close`（优雅关闭）、`destroy`（强制销毁）。
2. **compose 方法**：实现拦截器链模式。接受多个拦截器函数，每个拦截器 `(dispatch) => (opts, handler) => ...`，通过函数组合形成调用链。最终用 `Proxy` 包装原始 Dispatcher，只替换 `dispatch` 方法。
3. **EventEmitter 继承**：所有 Dispatcher 都可以发出 `drain`、`connect`、`disconnect`、`connectionError` 事件。

### 4.2 DispatcherBase

`lib/dispatcher/dispatcher-base.js` 是 Dispatcher 的第一个具体实现层，管理生命周期（close/destroy）和 dispatch 守卫。

**生命周期状态机**：

```
  正常运行 --> [kClosed=true] --> close() 等待队列清空 --> [kDestroyed=true] --> destroy() 释放资源
                 |                      |
                 +--- destroy(err) -----+---> 立即销毁
```

**dispatch 方法的守卫逻辑**：

```javascript
dispatch (opts, handler) {
  if (!handler || typeof handler !== 'object') throw ...
  if (opts.dispatcher) throw ...  // 禁止嵌套 dispatcher
  if (this[kDestroyed] || this[kOnDestroyed]) throw new ClientDestroyedError()
  if (this[kClosed]) throw new ClientClosedError()
  return this[kDispatch](opts, handler)
}
```

**close/destroy 的回调聚合**：

close 和 destroy 都支持多次调用——第一次触发实际操作，后续调用将回调追加到 `kOnClosed`/`kOnDestroyed` 数组中，待操作完成后批量调用。

### 4.3 Client — 单连接 HTTP 客户端

`lib/dispatcher/client.js` 是 undici 的核心组件，代表对单个 origin 的一个 HTTP 连接。

**队列设计（三段式）**：

```
|   complete   |   running   |   pending   |
               ^ kRunningIdx ^ kPendingIdx ^ kQueue.length
```

- `kQueue`：请求队列数组
- `kRunningIdx`：第一个正在执行的请求索引
- `kPendingIdx`：第一个等待执行的请求索引
- `kPending` = `kQueue.length - kPendingIdx`
- `kRunning` = `kPendingIdx - kRunningIdx`
- `kSize` = `kQueue.length - kRunningIdx`

已完成的请求置为 `null`，当 `kRunningIdx > 256` 时批量裁剪：

```javascript
if (client[kRunningIdx] > 256) {
  client[kQueue].splice(0, client[kRunningIdx])
  client[kPendingIdx] -= client[kRunningIdx]
  client[kRunningIdx] = 0
}
```

这是分摊 O(1) 的队列实现——不做每次请求的 splice，而是在累积 256 个空位后一次性清理。

**kDispatch 方法**：

```javascript
[kDispatch] (opts, handler) {
  const request = new Request(this[kUrl].origin, opts, handler)
  this[kQueue].push(request)
  // 根据 body 类型决定同步/异步 resume
  if (this[kResuming]) { /* 已经在 resume */ }
  else if (bodyLength == null && isIterable(body)) {
    this[kResuming] = 1
    queueMicrotask(() => resume(this))  // 延迟一 tick，等 body 结束
  } else {
    this[kResume](true)  // 同步 resume
  }
}
```

**resume 核心循环** (`_resume`)：

```javascript
function _resume (client, sync) {
  while (true) {
    if (client.destroyed) return
    if (client[kClosedResolve] && !client[kSize]) { resolve(); return }
    if (client[kHTTPContext]) client[kHTTPContext].resume()
    if (client[kBusy]) { /* 设置 needDrain */ }
    if (client[kPending] === 0) return
    if (client[kRunning] >= getMaxConcurrent(client)) return
    const request = client[kQueue][client[kPendingIdx]]
    // servername 变化时重建连接
    if (!client[kHTTPContext]) { connect(client); return }
    if (client[kHTTPContext].write(request)) client[kPendingIdx]++
  }
}
```

**连接建立**（`connect` 函数）：

```javascript
function connect (client) {
  client[kConnecting] = true
  client[kConnector]({ host, hostname, protocol, port, servername, localAddress },
    (err, socket) => {
      // 根据 ALPN 协议选择 H1 或 H2 上下文
      client[kHTTPContext] = socket.alpnProtocol === 'h2'
        ? connectH2(client, socket)
        : connectH1(client, socket)
      client.emit('connect', client[kUrl], [client])
      client[kResume]()
    })
}
```

**busy 判断（影响 Pool 扩容）**：

```javascript
get [kBusy] () {
  const allowsMux = this[kHTTPContext]?.version === 'h2'
  return Boolean(
    this[kHTTPContext]?.busy(null) ||
    (this[kSize] >= (getMaxConcurrent(this) || 1)) ||
    (this[kPending] > 0 && !allowsMux)  // H2 不触发 "有排队即忙"
  )
}
```

对于 H2 连接，`kPending > 0` 不会标记为 busy，因为 H2 可以在同一个连接上并发多个 stream。对于 H1，有排队即忙，Pool 会创建新的 Client。

**可配置参数**：

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `pipelining` | 1 | HTTP/1.1 流水线深度 |
| `keepAliveTimeout` | 4000ms | Keep-Alive 空闲超时 |
| `keepAliveMaxTimeout` | 600000ms | Keep-Alive 最大超时 |
| `keepAliveTimeoutThreshold` | 2000ms | Keep-Alive 超时阈值 |
| `headersTimeout` | 300000ms | 等待响应头超时 |
| `bodyTimeout` | 300000ms | 等待响应体超时 |
| `maxHeaderSize` | `http.maxHeaderSize` | 最大头部大小 |
| `maxRequestsPerClient` | 无限制 | 单连接最大请求数 |
| `maxResponseSize` | -1（无限制） | 最大响应体大小 |
| `strictContentLength` | true | 严格校验 Content-Length |
| `allowH2` | true | 是否允许 HTTP/2 |
| `maxConcurrentStreams` | 100 | H2 最大并发流 |

### 4.4 Pool — 多连接池

`lib/dispatcher/pool.js` 基于 `PoolBase` 实现同 origin 的多连接管理。

**核心机制 — 按需扩容**：

```javascript
[kGetDispatcher] () {
  for (let i = 0; i < this[kClients].length; i++) {
    const client = this[kClients][i]
    // TTL 检查
    if (clientTtlOption && (Date.now() - client.ttl) > clientTtlOption) {
      this[kRemoveClient](client)
      i--
    } else if (!client[kNeedDrain]) {
      return client  // 找到空闲 Client
    }
  }
  // 所有 Client 都忙，创建新的
  if (!this[kConnections] || this[kClients].length < this[kConnections]) {
    const dispatcher = this[kFactory](this[kUrl], this[kOptions])
    this[kAddClient](dispatcher)
    return dispatcher
  }
}
```

**factory 模式**：

```javascript
function defaultFactory (origin, opts) {
  return new Client(origin, opts)
}
```

Pool 使用工厂函数创建 Client，允许用户注入自定义 Client 创建逻辑。

**TTL 支持**：`clientTtl` 参数使 Client 在闲置超过指定时间后自动移除。

**连接错误处理**：

```javascript
this.on('connectionError', (origin, targets, error) => {
  for (const target of targets) {
    const idx = this[kClients].indexOf(target)
    if (idx !== -1) this[kClients].splice(idx, 1)
  }
})
```

连接错误的 Client 直接从池中移除，不尝试重用。

### 4.5 PoolBase — 池基类与队列调度

`lib/dispatcher/pool-base.js` 是 Pool、BalancedPool、RoundRobinPool 的共同基类。

**内部队列**：使用 `FixedQueue`（见第 14 节）存储当所有 Client 都忙时排队的请求。

**dispatch 逻辑**：

```javascript
[kDispatch] (opts, handler) {
  const dispatcher = this[kGetDispatcher]()  // 由子类实现
  if (!dispatcher) {
    this[kNeedDrain] = true
    this[kQueue].push({ opts, handler })  // 入队等待
    this[kQueued]++
  } else if (!dispatcher.dispatch(opts, handler)) {
    dispatcher[kNeedDrain] = true
    this[kNeedDrain] = !this[kHasDispatcher]()
  }
  return !this[kNeedDrain]
}
```

**drain 事件处理**：当某个 Client 发出 `drain`（表示它又有空闲容量了），PoolBase 从内部队列取出请求并 dispatch：

```javascript
[kOnDrain] (client, origin, targets) {
  while (!needDrain) {
    const item = queue.shift()
    if (!item) break
    this[kQueued]--
    needDrain = !client.dispatch(item.opts, item.handler)
  }
  client[kNeedDrain] = needDrain
}
```

**聚合统计**：`kConnected`、`kFree`、`kPending`、`kRunning`、`kSize` 都是遍历所有子 Client 汇总计算。

### 4.6 Agent — 顶层多 origin 路由器

`lib/dispatcher/agent.js` 是全局默认 Dispatcher，管理多个 origin 的连接。

**核心 — 按 origin 路由**：

```javascript
[kDispatch] (opts, handler) {
  const origin = String(opts.origin)
  const allowH2 = opts.allowH2 ?? this[kOptions].allowH2
  const key = allowH2 === false ? `${origin}#http1-only` : origin

  let dispatcher = this[kClients].get(key)
  if (!dispatcher) {
    dispatcher = this[kFactory](opts.origin, this[kOptions])
    // 监听事件并自动清理空闲 Pool
    dispatcher.on('disconnect', () => closeClientIfUnused())
    dispatcher.on('connectionError', () => closeClientIfUnused())
    this[kClients].set(key, dispatcher)
  }
  return dispatcher.dispatch(opts, handler)
}
```

**自动清理**：当 Pool 的所有连接都断开、没有忙碌连接、没有待处理请求时，自动从 Map 中移除并关闭：

```javascript
const closeClientIfUnused = () => {
  if (dispatcher[kConnected] > 0 || dispatcher[kBusy] || dispatcher[kPending] > 0) return
  this[kClients].delete(key)
  if (!dispatcher.destroyed) dispatcher.close()
}
```

**defaultFactory**：

```javascript
function defaultFactory (origin, opts) {
  return opts && opts.connections === 1
    ? new Client(origin, opts)   // 单连接直接用 Client
    : new Pool(origin, opts)     // 多连接用 Pool
}
```

当 `connections === 1` 时跳过 Pool 直接使用 Client，减少一层抽象开销。

**maxOrigins 限制**：防止 Agent 缓存无限增长的 origin。

### 4.7 BalancedPool — 加权负载均衡

`lib/dispatcher/balanced-pool.js` 实现加权轮询（Weighted Round Robin）负载均衡。

**核心算法**：

```javascript
[kGetDispatcher] () {
  let counter = 0
  let maxWeightIndex = -1

  while (counter++ < this[kClients].length) {
    this[kIndex] = (this[kIndex] + 1) % this[kClients].length
    const pool = this[kClients][this[kIndex]]

    if (this[kIndex] === 0) {
      this[kCurrentWeight] -= this[kGreatestCommonDivisor]
      if (this[kCurrentWeight] <= 0) this[kCurrentWeight] = this[kMaxWeightPerServer]
    }

    if (pool[kNeedDrain] || pool.closed || pool.destroyed) continue

    if (maxWeightIndex === -1 || pool[kWeight] > this[kClients][maxWeightIndex][kWeight]) {
      maxWeightIndex = this[kIndex]
    }

    if (pool[kWeight] >= this[kCurrentWeight]) return pool
  }
  // 回退到权重最大的
  return this[kClients][maxWeightIndex]
}
```

**权重动态调整**：

```javascript
pool.on('connect', () => {
  pool[kWeight] = Math.min(maxWeightPerServer, pool[kWeight] + errorPenalty)
})
pool.on('connectionError', () => {
  pool[kWeight] = Math.max(1, pool[kWeight] - errorPenalty)
})
```

- 连接成功恢复权重（`+errorPenalty`，默认 15）
- 连接错误降低权重（`-errorPenalty`）
- 权重范围 `[1, maxWeightPerServer]`（默认 100）
- 使用最大公约数（GCD）算法实现加权轮询

### 4.8 RoundRobinPool — 轮询负载均衡

`lib/dispatcher/round-robin-pool.js` 实现简单的轮询策略：

```javascript
[kGetDispatcher] () {
  let checked = 0
  while (checked < this[kClients].length) {
    this[kIndex] = (this[kIndex] + 1) % this[kClients].length
    const client = this[kClients][this[kIndex]]
    // TTL 检查
    if (/* TTL expired */) { this[kRemoveClient](client); this[kIndex]--; continue }
    if (!client[kNeedDrain]) return client
    checked++
  }
  // 所有都忙，按需创建
}
```

与 BalancedPool 的区别：不维护权重，每次选下一个，简单公平。

### 4.9 Dispatcher 完整继承体系图

undici 的 dispatcher 体系由 17 个文件组成，核心继承链如下（ASCII 类图）：

```
EventEmitter (node:events)
  └── Dispatcher (lib/dispatcher/dispatcher.js, 54 行)
        │ 抽象基类。定义 dispatch/close/destroy 三个抽象方法 + compose() 拦截器链。
        │ 关键方法签名：
        │   dispatch(opts, handler)           → boolean | void
        │   close([cb])                        → Promise | void
        │   destroy([err, cb])                 → Promise | void
        │   compose(...interceptors)           → Proxy<Dispatcher>
        │
        ├── DispatcherBase (lib/dispatcher/dispatcher-base.js, 197 行)
        │      │ 生命周期管理：kDestroyed/kClosed 状态机 + 回调聚合。
        │      │ dispatch() 前置守卫 + 转发到子类 this[kDispatch](opts, handler)。
        │      │
        │      │ 关键内部 Symbol：
        │      │   kDestroyed / kClosed / kOnDestroyed / kOnClosed
        │      │   kWebSocketOptions / kEventSourceOptions
        │      │
        │      ├── Client (lib/dispatcher/client.js, 741 行)
        │      │      │ 单 origin 的 HTTP 连接抽象。三段式队列 + resume 循环。
        │      │      │
        │      │      │ 关键方法签名：
        │      │      │   [kDispatch](opts, handler)  → boolean  入队 + 触发 resume
        │      │      │   [kConnect](cb)               → void    异步建连
        │      │      │   [kDestroy](err)              → Promise 销毁 + 失败所有 pending
        │      │      │   get [kPending] / [kRunning] / [kSize] / [kBusy] / [kConnected]
        │      │      │
        │      │      │ 关键 Symbol 配置：
        │      │      │   kUrl / kConnector / kServerName / kPipelining
        │      │      │   kHTTPContext / kHTTP2Options / kMaxConcurrentStreams
        │      │      │
        │      │      └── H2CClient (lib/dispatcher/h2c-client.js, 51 行)
        │      │            仅用于明文 HTTP/2 (h2c)。强制 useH2c:true + allowH2:true。
        │      │            校验 origin 必须是 http://，否则抛 InvalidArgumentError。
        │      │            pipelining 默认 100 但不可超过 maxConcurrentStreams。
        │      │
        │      ├── PoolBase (lib/dispatcher/pool-base.js, 232 行)
        │      │      │ 多连接池基类。clients[] 数组 + FixedQueue 排队。
        │      │      │
        │      │      │ 关键方法签名：
        │      │      │   [kDispatch](opts, handler)   → boolean  选 dispatcher 或入队
        │      │      │   [kGetDispatcher]()           → Dispatcher | void  子类多态
        │      │      │   [kHasDispatcher]()           → boolean
        │      │      │   [kAddClient](client)         → this     注册 + 绑定事件
        │      │      │   [kRemoveClient](client)      → void     解绑 + 清理
        │      │      │
        │      │      ├── Pool (lib/dispatcher/pool.js, 143 行)
        │      │      │     │ 同 origin 多连接。kGetDispatcher() 线性扫描 + 按需扩容。
        │      │      │     │ 支持 connections 上限、clientTtl 过期清理。
        │      │      │     │ connectionError 时立即从 clients 移除，避免复用坏连接。
        │      │      │     │
        │      │      │     ├── BalancedPool (lib/dispatcher/balanced-pool.js, 214 行)
        │      │      │     │     │ 多 upstream 加权负载均衡。Nginx 风格平滑加权轮询。
        │      │      │     │     │ kWeight[] 权重数组 + kCurrentWeight + GCD 步长递减。
        │      │      │     │     │ addUpstream/removeUpstream 动态增删上游。
        │      │      │     │     │ 每个 upstream 是一个 Pool 实例（嵌套 PoolBase 调度）。
        │      │      │     │
        │      │      │     └── RoundRobinPool (lib/dispatcher/round-robin-pool.js, 159 行)
        │      │      │           纯轮询 + 跳过 busy 客户端 + 全部 busy 时按需扩容。
        │      │      │           kIndex 全局自增取模。无权重、无故障降级。
        │      │      │
        │      │      └── Agent (lib/dispatcher/agent.js, 177 行)
        │      │            顶层多 origin 路由器。Map<origin, dispatcher> 按 origin 选择。
        │      │            maxOrigins 限制 + 自动 GC 空闲 origin。
        │      │            allowH2=false 时 origin key 追加 "#http1-only" 隔离。
        │      │            disconnect 事件触发 closeClientIfUnused 清理。
        │      │
        │      ├── ProxyAgent (lib/dispatcher/proxy-agent.js, 378 行)
        │      │     │ HTTP 代理调度器。三种模式分支：
        │      │     │   (1) socks5:// → 委托给 Socks5ProxyAgent
        │      │     │   (2) http 目标 + proxyTunnel=false → Http1ProxyWrapper 改写 path
        │      │     │   (3) 其他 → CONNECT 隧道，内层用 Agent 调度真实连接
        │      │     │ 安全措施：禁止请求头携带 Proxy-Authorization。
        │      │     │ 嵌套 Agent: this[kAgent] 内部用 Pool/Client 建立到 proxy 的连接。
        │      │     │
        │      │     └── Http1ProxyWrapper (内部类, ~65 行)
        │      │           仅用于 HTTP 正向代理非隧道模式。重写 opts.path = origin + path。
        │      │           拦截 407 响应转 InvalidArgumentError。
        │      │
        │      ├── EnvHttpProxyAgent (lib/dispatcher/env-http-proxy-agent.js, 175 行)
        │      │      │ 自动从 process.env.http_proxy/https_proxy/no_proxy 读取代理配置。
        │      │      │ 持有三个子 Agent：noProxyAgent、httpProxyAgent、httpsProxyAgent。
        │      │      │ #shouldProxy(): NO_PROXY 解析（IPv6/端口/子域名/通配符*）。
        │      │      │ #noProxyChanged getter: 监听环境变量变化触发重新解析。
        │      │      │
        │      │      └── NO_PROXY 解析支持：
        │      │            "*.example.com" → 子域名匹配
        │      │            "[::1]:443"  → IPv6 + 端口
        │      │            "*"          → 全不走代理
        │      │
        │      ├── Socks5ProxyAgent (lib/dispatcher/socks5-proxy-agent.js, 282 行)
        │      │      │ 实验性 SOCKS5 代理。按 origin 维护 Map<origin, Pool>。
        │      │      │ createSocks5Connection(): 连接代理 → 握手 → 认证 → CONNECT → 隧道。
        │      │      │ Pool 的 connect 函数被替换为 SOCKS5 隧道建立。
        │      │      │ 支持用户名/密码认证；超时 5 秒；ExperimentalWarning 仅弹一次。
        │      │      │
        │      │      └── 使用 Socks5Client (lib/core/socks5-client.js) 状态机：
        │      │            STATES: INITIAL → GREETING → AUTH → CONNECTING → CONNECTED
        │      │
        │      ├── Dispatcher1Wrapper (lib/dispatcher/dispatcher1-wrapper.js, 107 行)
        │      │      │ v1 API 向后兼容层。强制 allowH2:false（v1 不支持 H2）。
        │      │      │ wrapHandler(): 检测 handler 是否含 v2 接口(onRequestStart)，
        │      │      │   否则包一层 LegacyHandlerWrapper 桥接：
        │      │      │     onConnect      → onRequestStart
        │      │      │     onHeaders      → onResponseStart (含 resume 暂停)
        │      │      │     onData         → onResponseData
        │      │      │     onComplete     → onResponseEnd
        │      │      │     onError        → onResponseError
        │      │      │     onUpgrade      → onRequestUpgrade
        │      │      │     onBodySent / onRequestSent / onResponseStarted 透传
        │      │      │
        │      │      └── 继承 Dispatcher (非 DispatcherBase)，因为 v1 无 close/destroy 生命周期
        │      │
        │      └── RetryAgent (lib/dispatcher/retry-agent.js, 35 行)
        │            极简装饰器。dispatch 时把 handler 包进 RetryHandler，
        │            再交给内部 this.#agent.dispatch()。重试逻辑全在 handler 中。
        │            自身继承 Dispatcher，close/destroy 透传内部 agent。
        │
        └── RetryHandler 实际定义在 lib/handler/retry-handler.js (~548 行)
              独立于 dispatcher 继承链，是 handler 层装饰器。
```

**设计模式总结**：

| 模式 | 应用位置 | 说明 |
|------|---------|------|
| 模板方法 | DispatcherBase → Client/Pool/Agent | dispatch() 守卫固定，子类实现 kDispatch |
| 策略 | PoolBase → Pool/BalancedPool/RoundRobinPool | kGetDispatcher() 多态 |
| 装饰器 | ProxyAgent/Dispatcher1Wrapper | 嵌套内部 dispatcher，功能增强 |
| 组合拦截器 | Dispatcher.compose() | Proxy 模式 + 函数组合 |
| 桥接 | connectH1/connectH2 返回统一上下文对象 | Client 不直接依赖具体协议实现 |
| 工厂 | Agent/Pool 的 factory 函数 | 创建 Client 或 Pool 子类可替换 |

### 4.10 Pool 三种负载均衡算法深度对比

Pool 体系三种实现都继承 `PoolBase`，差异仅在 `kGetDispatcher()`。下表从源码级对比：

```
                  Pool                       BalancedPool                    RoundRobinPool
──────────────┼──────────────────────────┼───────────────────────────────┼──────────────────────
 适用场景      │ 同 origin 多连接          │ 多 origin 加权负载均衡          │ 同 origin 简单轮询
 上游数量      │ 1 个 origin               │ N 个 upstream (动态增删)        │ 1 个 origin
 上层结构      │ 独立使用                  │ 内含多个 Pool（每个 upstream 一个）│ 独立使用
 扩容触发      │ 全部 Client busy          │ 子 Pool 内部同左                │ 全部 Client busy
 扩容上限      │ connections 参数          │ 子 Pool 的 connections          │ connections 参数
 核心索引      │ 无（线性扫描）             │ kIndex + kCurrentWeight + GCD   │ kIndex（自增取模）
 权重          │ 无                        │ kWeight[] 动态调整               │ 无
 故障响应      │ connectionError 时移除     │ weight -= errorPenalty(15)       │ connectionError 时移除
 恢复机制      │ 无（永久移除）             │ connect 成功 weight += 15        │ 无
 空闲 TTL      │ clientTtl 过期移除         │ 无                              │ clientTtl 过期移除
 子 Dispatcher │ Client 或 Pool (factory)   │ Pool (嵌套)                     │ Client (默认 factory)
 代码量        │ 143 行                     │ 214 行                          │ 159 行
```

**BalancedPool 的平滑加权轮询算法详解**（源自 Nginx 的 SWRR）：

```javascript
[kGetDispatcher] () {
  if (this[kClients].length === 0) throw new BalancedPoolMissingUpstreamError()

  let counter = 0
  let maxWeightIndex = -1

  while (counter++ < this[kClients].length) {
    // 1. 轮转索引
    this[kIndex] = (this[kIndex] + 1) % this[kClients].length
    const pool = this[kClients][this[kIndex]]

    // 2. 每轮起始降低 currentWeight
    if (this[kIndex] === 0) {
      this[kCurrentWeight] -= this[kGreatestCommonDivisor]  // GCD 步长
      if (this[kCurrentWeight] <= 0) {
        this[kCurrentWeight] = this[kMaxWeightPerServer]     // 重置为 max
      }
    }

    // 3. 跳过不可用
    if (pool[kNeedDrain] || pool.closed || pool.destroyed) continue

    // 4. 记录最大权重 fallback
    if (maxWeightIndex === -1 || pool[kWeight] > this[kClients][maxWeightIndex][kWeight]) {
      maxWeightIndex = this[kIndex]
    }

    // 5. 权重 ≥ 当前权重 → 选中
    if (pool[kWeight] >= this[kCurrentWeight]) return pool
  }

  // 6. fallback 到最大权重
  if (maxWeightIndex !== -1) {
    this[kCurrentWeight] = this[kClients][maxWeightIndex][kWeight]
    return this[kClients][maxWeightIndex]
  }
}
```

**算法关键点**：

1. **GCD 步长递减**：`kCurrentWeight` 每次减少所有节点权重的最大公约数，确保遍历所有可能的权重值。
2. **maxWeightPerServer 重置**：当 currentWeight 降到 ≤0 时重置到最大值，开始新一轮。
3. **权重动态调整**：
   - 连接成功：`weight = min(maxWeight, weight + errorPenalty)`（默认 +15）
   - 连接错误/UND_ERR_SOCKET 断连：`weight = max(1, weight - errorPenalty)`（默认 -15）
4. **maxWeightIndex fallback**：若一轮遍历无满足 weight ≥ currentWeight 的节点，选最大权重的可用节点。

**三种 Pool 选型决策树**：

```
请求多个不同 origin？
  ├── 是 → 用 Agent（顶层多 origin 路由器）
  └── 否 → 单 origin
            │
            ├── 多 upstream 需要权重/故障降级？
            │     └── 是 → BalancedPool
            │
            └── 否
                  ├── 纯平均分配，无状态？
                  │     └── 是 → RoundRobinPool
                  │
                  └── 需要 TTL 清理、按需扩容、线性扫描空闲优先？
                        └── 是 → Pool
```

**Pool 嵌套组合关系**：

undici 支持 Pool 的多层嵌套。BalancedPool 每个 upstream 内部就是一个 Pool，Agent 每个 origin 可映射到 Pool 或 Client。多层嵌套的 busy/needDrain 判断递归进行：

```
BalancedPool (多 upstream)
  ├── Pool("https://api-us.example.com")    ← 每个 upstream 一个 Pool
  │     ├── Client(socket-1)
  │     ├── Client(socket-2)
  │     └── Client(socket-3)
  └── Pool("https://api-eu.example.com")
        ├── Client(socket-4)
        └── Client(socket-5)
```

**各 Pool 与事件转发链**：

```
Client (emit drain/connect/disconnect/connectionError)
  ↑ kAddClient 时绑定事件
  │
PoolBase (emit 同名事件)
  ↑ kAddClient 时绑定事件
  │
Pool / BalancedPool / RoundRobinPool / Agent (emit 同名事件)
  ↑
用户代码 listener
```

`kOnDrain` 回调在 PoolBase 内部实现：当某个 Client 发出 drain 时，从 FixedQueue 中取出等待的请求继续 dispatch。若队列清空，emit 自己的 drain 事件。

---

## 5. HTTP/1.1 vs HTTP/2 双协议实现

### 5.1 client-h1.js — HTTP/1.1 连接上下文

`lib/dispatcher/client-h1.js` 是 HTTP/1.1 协议的完整实现，约 1800 行。

**connectH1 返回的上下文对象**：

```javascript
return {
  version: 'h1',
  defaultPipelining: 1,
  write (request) { return writeH1(client, request) },
  resume () { resumeH1(client) },
  destroy (err, callback) { socket.destroy(err).on('close', callback) },
  get destroyed () { return socket.destroyed },
  busy (request) {
    if (socket[kWriting] || socket[kReset] || socket[kBlocking]) return true
    if (request) {
      if (client[kRunning] > 0 && !request.idempotent) return true
      if (client[kRunning] > 0 && (request.upgrade || request.method === 'CONNECT')) return true
      if (client[kRunning] > 0 && hasStreamBody(request)) return true
    }
    return false
  }
}
```

**H1 busy 的精细判断**：
- 正在写请求体时忙
- 已标记 reset 时忙
- 非幂等请求（POST/PUT/PATCH）在有 inflight 请求时不能流水线——防止失败不可重试
- Upgrade/CONNECT 请求必须等所有前序请求完成
- 流式 body 的请求不能流水线——错误会影响同连接的其他请求

**writeH1 — HTTP 请求写入**：

核心流程：
1. 拼接请求行和头部：`METHOD PATH HTTP/1.1\r\nhost: ...\r\nconnection: ...\r\n`
2. 根据 body 类型分发到不同写入策略：
   - `null`/Buffer：`writeBuffer` — 一次性写入
   - Blob：`writeBlob` — await arrayBuffer 后写入
   - Stream：`writeStream` — 用 AsyncWriter 流式写入
   - Iterable/AsyncIterable：`writeIterable` — 用 AsyncWriter 逐块写入

**AsyncWriter — 流式 body 写入器**：

```javascript
class AsyncWriter {
  write (chunk) {
    if (bytesWritten === 0) {
      if (contentLength === null) {
        socket.write(`transfer-encoding: chunked\r\n`, 'latin1')
      } else {
        socket.write(`content-length: ${contentLength}\r\n\r\n`, 'latin1')
      }
    }
    if (contentLength === null) {
      socket.write(`\r\n${len.toString(16)}\r\n`, 'latin1')  // chunked 编码
    }
    return socket.write(chunk)
  }
}
```

**空闲 Socket 验证（Idle Socket Validation）**：

这是一个重要的安全机制，防止在复用 keep-alive 连接时发送请求到已被对端关闭的 socket：

```javascript
function scheduleIdleSocketValidation (client, socket) {
  socket[kIdleSocketValidation] = 1
  socket[kIdleSocketValidationTimeout] = setImmediate(() => {
    socket[kIdleSocketValidation] = 2
    if (client[kSocket] === socket && !socket.destroyed) {
      client[kResume]()
    }
  })
}
```

使用 `setImmediate`（而非 `setTimeout(0)`），既避免 1ms 定时器开销，又让事件循环有机会处理已挂起的 FIN/RST。

### 5.2 client-h2.js — HTTP/2 连接上下文

`lib/dispatcher/client-h2.js` 是 HTTP/2 协议的完整实现，约 1780 行。

**connectH2 返回的上下文对象**：

```javascript
return {
  version: 'h2',
  defaultPipelining: Infinity,  // H2 无流水线概念，多路复用
  write (request) { return writeH2(client, request) },
  resume () { resumeH2(client) },
  destroy (err, callback) { socket.destroy(err).on('close', callback) },
  get destroyed () { return socket.destroyed },
  busy (request) {
    if (session[kRemoteSettings] === false && client[kRunning] > 0) return true
    if (client[kRunning] >= client[kMaxConcurrentStreams]) return true
    // Upgrade/CONNECT 需要等 remoteSettings
    return false
  }
}
```

**GOAWAY 处理**：

```javascript
function onHttp2SessionGoAway (errorCode, lastStreamID) {
  this[kReceivedGoAway] = true
  const err = getGoAwayError(this, errorCode)
  const pendingIdx = getGoAwayPendingIdx(client, lastStreamID)
  const retriableRequests = []

  for (let i = pendingIdx; i < previousPendingIdx; i++) {
    const request = client[kQueue][i]
    // 分离 stream 关联
    streamsToClose.push(detachRequestStreamForClose(request))
    // 可重试的请求（body 是 null/Buffer/Blob）重新入队
    if (canReplayRequest(request) && registerGoAwayRefusal(request)) {
      retriableRequests.push(request)
    } else {
      util.errorRequest(client, request, err)
    }
  }
  // 关闭受影响的 stream，重排队列
}
```

**REFUSED_STREAM 重试**：

```javascript
function retryRefusedStream (stream, state) {
  if (state.responseReceived || request.aborted || request.completed) return false
  if (request[kRefusedStreamRetry]) return false  // 只重试一次
  request[kRefusedStreamRetry] = true
  // 分离失败的 stream，将请求放回 pending 队列头部
  detachRequestStreamForClose(request)
  client[kQueue].splice(client[kPendingIdx], 0, request)
  client[kResume]()
  return true
}
```

**H2 连接级流控**：

```javascript
function applyConnectionWindowSize (connectionWindowSize) {
  if (typeof this.setLocalWindowSize === 'function') {
    this.setLocalWindowSize(connectionWindowSize)
  }
}
```

在 `remoteSettings` 事件后应用连接级窗口大小（默认 512KB，高于 Node.js 默认值）。

**WebSocket over H2（RFC 8441）**：

```javascript
if (upgrade === 'websocket') {
  if (session[kEnableConnectProtocol] === false) {
    // 不支持 extended CONNECT
    return false
  }
  headers[HTTP2_HEADER_METHOD] = 'CONNECT'
  headers[HTTP2_HEADER_PROTOCOL] = 'websocket'
  headers[HTTP2_HEADER_PATH] = path
  headers[HTTP2_HEADER_SCHEME] = protocol === 'ws:' ? 'http' : 'https'
}
```

### 5.3 协议选择与 ALPN 协商

在 `connect` 函数中（client.js），协议选择基于 TLS ALPN 协商结果：

```javascript
client[kHTTPContext] = socket.alpnProtocol === 'h2'
  ? connectH2(client, socket)
  : connectH1(client, socket)
```

ALPN 优先级在 `connect.js` 中设置：

```javascript
ALPNProtocols: allowH2 ? (preferH2 ? ['h2', 'http/1.1'] : ['http/1.1', 'h2']) : ['http/1.1']
```

默认 `preferH2` 为 false，即优先 HTTP/1.1 但接受 HTTP/2。设为 `preferH2: true` 则优先 H2。

### 5.4 双协议状态机深度对比

**Client (kHTTPContext) 状态机**：

```
                   ┌──────────────────────────────────┐
                   │          Client 状态机             │
                   └──────────────────────────────────┘

  ┌──────────┐  dispatch()   ┌──────────────┐   connect()   ┌─────────────┐
  │  IDLE    │ ────────────> │  RESUMING    │ ────────────> │  CONNECTING │
  │ kQueue=0 │               │ kResuming=2  │               │ kConnecting │
  └──────────┘               └──────────────┘               └──────┬──────┘
       ^                           │  ▲                            │
       │                           │  │                            │
       │                     write │  │ idle                       │ connect cb
       │                     busy  │  │                            ▼
       │                           ▼  │                     ┌─────────────┐
       │                     ┌──────────────┐   alpn=h2      │  CONNECTED  │
       └──────────────────── │   BUSY       │ ─────────────>  │  kHTTPContext│
            kNeedDrain=0     │ kNeedDrain=2 │                 └──────┬──────┘
                             └──────────────┘                        │
                                         │                     h1 ╱   ╲ h2
                                         ▼                     ╱       ║
                                   ┌──────────────┐    ┌────────┐  ┌────────┐
                                   │   DRAINING   │    │  H1    │  │  H2    │
                                   │ 等所有请求完成  │    │Context │  │Session │
                                   └──────────────┘    └────────┘  └────────┘
```

**HTTP/1.1 socket 状态标志**：

| Symbol | 含义 | 触发条件 |
|--------|------|---------|
| `kWriting` | 正在写请求体 | writeStream / writeBuffer / writeIterable 期间 |
| `kReset` | 需要重置连接 | HEAD/CONNECT/Upgrade 后、maxRequests 达到、body 不期待 |
| `kBlocking` | 阻塞流水线 | 请求头带 `connection: close` 或 blocking=true |
| `kNoRef` | socket 已 unref | 空闲时 keepAlive 期间 |
| `kIdleSocketValidation` | 空闲验证中 | 0=正常 1=scheduled 2=checked |

**HTTP/2 session 状态**：

| Symbol | 含义 | 触发条件 |
|--------|------|---------|
| `kHTTP2SessionState.idleTimeout` | 空闲关闭定时器 | 所有 stream 关闭 + 队列为空 |
| `kHTTP2SessionState.noStreamsTimeout` | 无流可用超时 | maxConcurrentStreams=0 |
| `kHTTP2SessionState.refed` | session ref 状态 | 有活跃 stream 时 ref，否则 unref |
| `kHTTP2SessionState.ping.interval` | PING 定时器 | 默认 60s，可设为 0 禁用 |
| `kReceivedGoAway` | 收到 GOAWAY | 服务端主动关闭或重启 |
| `kRemoteSettings` | 收到 SETTINGS | 初始 false，收到后为 true |
| `kEnableConnectProtocol` | 支持 Extended CONNECT | 来自 SETTINGS_ENABLE_CONNECT_PROTOCOL |

**双协议并发策略差异**：

```
            HTTP/1.1 (client-h1)                     HTTP/2 (client-h2)
      ┌────────────────────────────┐         ┌────────────────────────────────┐
      │    单个 TCP Socket          │         │     单个 TCP Socket             │
      │                            │         │                                │
      │  Req1 ──> Req2 ──> Req3   │         │  Stream1 ─┐                    │
      │  (严格顺序,默认深度1)        │         │  Stream2 ─┼── 多路复用并发       │
      │                            │         │  Stream3 ─┘   (默认100路)       │
      │  响应必须按请求顺序          │         │  响应可以乱序到达               │
      │  队头阻塞(HOL blocking)     │         │  无队头阻塞                    │
      └────────────────────────────┘         └────────────────────────────────┘

  判断 busy:                                 判断 busy:
  socket[kWriting]                         session[kRemoteSettings]=false && running>0
  || socket[kReset]                        || running >= maxConcurrentStreams
  || socket[kBlocking]                     || (upgrade/CONNECT) && !remoteSettings
  || (running>0 && !idempotent)
  || (running>0 && upgrade)
  || (running>0 && stream body)

  扩容条件 (Pool 创建新 Client):            扩容条件 (Pool 创建新 Client):
  kPending > 0 (H1 视排队为 busy)          kSize >= maxConcurrentStreams
  kSize >= pipelining (默认1)
```

### 5.5 dispatch() 完整调度链路时序图

从用户调用到响应回调的全链路时序：

```
用户代码                 API层                Dispatcher                 Client                协议层
   │                      │                      │                        │                    │
   │ undici.request(opts) │                      │                        │                    │
   │─────────────────────>│                      │                        │                    │
   │                      │                      │                        │                    │
   │                      │ new RequestHandler   │                        │                    │
   │                      │ dispatch(opts,h)     │                        │                    │
   │                      │─────────────────────>│                        │                    │
   │                      │                      │                        │                    │
   │                      │                      │ 1. 守卫检查             │                    │
   │                      │                      │   (handler/destroyed/   │                    │
   │                      │                      │    closed 校验)         │                    │
   │                      │                      │                        │                    │
   │                      │                      │ 2. ProxyAgent/Pool 分支 │                    │
   │                      │                      │   ┌─── kGetDispatcher ─┤                    │
   │                      │                      │   │   (选 Client)       │                    │
   │                      │                      │   │                     │                    │
   │                      │                      │   │ 3. Client[kDispatch]│                    │
   │                      │                      │───┼────────────────────>│                    │
   │                      │                      │   │                     │                    │
   │                      │                      │   │  new Request(origin, │                    │
   │                      │                      │   │    opts, handler)   │                    │
   │                      │                      │   │                     │                    │
   │                      │                      │   │  kQueue.push(request)│                   │
   │                      │                      │   │                     │                    │
   │                      │                      │   │  [kResume](sync)    │                    │
   │                      │                      │<──┼─────────────────────│                    │
   │                      │                      │   │                     │                    │
   │                      │                      │ 4. _resume() 循环      │                    │
   │                      │                      │   ┌─ destroyed? return │                    │
   │                      │                      │   ├─ kClosedResolve?   │                    │
   │                      │                      │   │   resolve & return │                    │
   │                      │                      │   ├─ kBusy? needDrain  │                    │
   │                      │                      │   ├─ kPending=0? return │                    │
   │                      │                      │   ├─ kRunning>=max?    │                    │
   │                      │                      │   │   return            │                    │
   │                      │                      │   │                     │                    │
   │                      │                      │   │ 5. !kHTTPContext?   │                    │
   │                      │                      │   │    connect(client)  │                    │
   │                      │                      │   │    (异步 TLS/TCP)   │                    │
   │                      │                      │   │         │           │                    │
   │                      │                      │   │         │           │ connect(cb)        │
   │                      │                      │   │         │           │───────────────────>│
   │                      │                      │   │         │           │                    │
   │                      │                      │   │         │           │  6. TCP+TLS 握手   │
   │                      │                      │   │         │           │  ALPN 协商         │
   │                      │                      │   │         │           │                    │
   │                      │                      │   │         │           │  callback(err,     │
   │                      │                      │   │         │           │          socket)   │
   │                      │                      │   │         │           │<───────────────────│
   │                      │                      │   │         │           │                    │
   │                      │                      │   │    7. socket.alpnProtocol          │
   │                      │                      │   │    ┌─── h2? connectH2()             │
   │                      │                      │   │    │    : connectH1()               │
   │                      │                      │   │    │                                 │
   │                      │                      │   │    │  kHTTPContext = {              │
   │                      │                      │   │    │    write(), resume(),          │
   │                      │                      │   │    │    busy(), destroy()           │
   │                      │                      │   │    │  }                             │
   │                      │                      │   │    │                                 │
   │                      │                      │   │ 8. emit('connect')                 │
   │                      │                      │   │    client[kResume]()                │
   │                      │                      │   │         │                            │
   │                      │                      │   │ 9. HTTP 上下文 resume()             │
   │                      │                      │   │    (h1: resumeH1 /                  │
   │                      │                      │   │     h2: resumeH2)                   │
   │                      │                      │   │         │                            │
   │                      │                      │   │ 10. 重新进入 _resume()              │
   │                      │                      │   │    取 request = kQueue[kPendingIdx]│
   │                      │                      │   │         │                            │
   │                      │                      │   │ 11. kHTTPContext.write(request)     │
   │                      │                      │   │────────────────────────────────────>│
   │                      │                      │   │         │                            │
   │                      │                      │   │     12. H1: writeH1()               │
   │                      │                      │   │         拼接 request line + headers │
   │                      │                      │   │         分发 body 写入策略           │
   │                      │                      │   │                            │         │
   │                      │                      │   │        或 H2: writeH2()               │
   │                      │                      │   │         构造 HTTP/2 headers           │
   │                      │                      │   │         session.request(headers)       │
   │                      │                      │   │         创建 stream                    │
   │                      │                      │   │         stream.write(body)              │
   │                      │                      │   │                            │              │
   │                      │                      │   │  13. 返回 true/false            │              │
   │                      │                      │   │<───────────────────────────────────────────│
   │                      │                      │   │         │                            │
   │                      │                      │   │  14. kPendingIdx++               │
   │                      │                      │   │         │                            │
   │                      │                      │   │  ┌─── 网络传输...                  │
   │                      │                      │   │  │                                  │
   │                      │                      │   │  │  H1: Parser.execute() 逐字节解析  │
   │                      │                      │   │  │  H2: Node http2 模块内部解析      │
   │                      │                      │   │  │                                  │
   │                      │                      │   │  │  ┌─── 响应到达 ──────────────┐  │
   │                      │                      │   │  │  │ H1: parser callback       │  │
   │                      │                      │   │  │  │ H2: stream.emit('response')│  │
   │                      │                      │   │  │  └───────────────────────────┘  │
   │                      │                      │   │  │                                  │
   │                      │                      │   │  15. handler.onResponseStart()       │
   │                      │                      │   │     (statusCode, headers, resume)   │
   │                      │                      │   │     │                                │
   │                      │                      │   │     │  RequestHandler.onResponseStart    │
   │                      │                      │   │     │  new Readable + callback(null,     │
   │                      │                      │   │     │    {statusCode, body, ...})        │
   │                      │                      │   │     │                                │
   │                      │<─────────────────────│<────┼─────│                                │
   │                      │                      │     │     │                                │
   │  Promise  resolve    │                      │     │ 16. handler.onResponseData()       │
   │  {statusCode, body}  │                      │     │     │  res.push(chunk)                  │
   │<─────────────────────│                      │     │     │  (用户消费 Readable)               │
   │                      │                      │     │     │                                │
   │  for await(chunk)    │                      │     │ 17. handler.onResponseEnd()        │
   │<─────────────────────│                      │     │     │  res.push(null)  // EOF          │
   │                      │                      │     │     │  client[kQueue][kRunningIdx++]=null│
   │                      │                      │     │     │                                │
```

**关键步骤解读**：

1. **守卫检查**（DispatcherBase.dispatch）：handler 必须是对象、opts 必须无 dispatcher 字段、client 不能 destroyed/closed。
2. **路由选择**（PoolBase.kGetDispatcher）：线性扫描 / 加权轮询 / 简单轮询，选空闲 Client。若全部 busy 则返回 undefined → 入 FixedQueue 等待 drain。
3. **入队 + resume 触发**（Client[kDispatch]）：新建 Request 推入 kQueue。根据 body 类型：
   - 同步 body（Buffer/string） → 立即 `kResume(true)` 同步调度
   - 异步 iterable → `queueMicrotask(() => resume(this))` 延迟一 tick 等 body 就绪
   - 已在 resume 中 → 不操作
4. **_resume 循环**：while(true) 逐次检查 destroyed/closed/busy/pending=0/running>=max 退出条件。
5. **servername 切换**：HTTPS 下若请求 servername 与当前连接不一致，且 running=0 时才重建连接。
6. **connect 异步建连**：调用用户配置的 connector（默认 buildConnector），内部做 DNS 解析 + TCP 连接 + TLS 握手 + ALPN 协商。
7. **协议上下文创建**：根据 ALPN 结果选择 connectH1 或 connectH2，返回 `{ write, resume, busy, destroy, destroyed }` 上下文对象。
8. **emit connect**：触发用户监听的 connect 事件。
9. **resumeH1/resumeH2**：H1 先处理 idleSocketValidation 和 ref/unref 状态；H2 设置 ref/unref + 空闲定时器 + noStreamsTimeout。
10. **write 发送请求**：
    - H1 writeH1：构造请求行 + headers + Content-Length/chunked 编码。对 stream body 用 AsyncWriter 流式写入；对 Buffer 一次性写入。
    - H2 writeH2：构造 HTTP/2 headers（:method/:path/:scheme/:authority），调用 session.request() 创建 Http2Stream，绑定 stream 事件（response/end/error/data/trailers/aborted）。
11. **响应回调**：H1 通过 Parser.execute() 逐字节触发回调；H2 通过 stream 事件触发。
12. **EOF 处理**：onMessageComplete（H1）或 onEnd/stream close（H2）→ handler.onResponseEnd → res.push(null)。
13. **队首推进**：`client[kQueue][kRunningIdx++] = null`，标记完成。
14. **keep-alive / reset**：shouldKeepAlive=true 时复用连接，设置 keepAlive timer；否则 destroy socket。

**dispatch() 返回值语义**：

```javascript
// client.js [kDispatch] 返回值
return this[kNeedDrain] < 2   // true=空闲可继续发, false=队列满需要等待 drain
```

`true`：连接仍可接受更多请求（未达并发上限）；
`false`：连接已满，发出 drain 事件后客户端应暂停发送。

### 5.6 双协议关键差异对比表

| 维度 | HTTP/1.1 (client-h1) | HTTP/2 (client-h2) |
|------|---------------------|-------------------|
| **并发模型** | 流水线（pipelining），默认深度 1 | 多路复用（multiplexing），默认 100 并发流 |
| **defaultPipelining** | 1 | Infinity |
| **解析器** | llhttp WASM（逐字节状态机） | Node.js 内置 http2 模块 |
| **请求写入** | 手动拼接 HTTP 报文，直接写 socket | `session.request(headers, options)` 创建 stream |
| **Keep-Alive** | 手动管理超时和 Connection 头 | 通过 GOAWAY/PING 管理 |
| **流控** | 无显式流控，靠 TCP 背压 | per-stream + connection-level WINDOW_UPDATE |
| **Upgrade** | socket 从 parser 分离，交给上层 | Extended CONNECT (RFC 8441) |
| **错误恢复** | 销毁 socket，重建连接 | GOAWAY 后重建 session，可重试未被接受的 stream |
| **忙判断** | writing/reset/blocking/非幂等 in-flight | 达到 maxConcurrentStreams 或未收到 remoteSettings |
| **空闲管理** | socket.ref/unref + keepAlive timer | session.ref/unref + idle timeout |
| **请求体写入** | AsyncWriter 手动 chunked 编码 | stream.write()/pipeline() 原生支持 |

---

## 6. llhttp WASM 解析器

### 6.1 WASM 模块加载

`lib/llhttp/` 目录包含 llhttp 的 WebAssembly 编译版本：

- `llhttp.wasm` / `llhttp-wasm.js`：标准版本
- `llhttp_simd.wasm` / `llhttp_simd-wasm.js`：SIMD 加速版本（ppc64 架构除外）

**加载策略**（`lazyllhttp` 函数）：

```javascript
function lazyllhttp () {
  let useWasmSIMD = process.arch !== 'ppc64'
  if (process.env.UNDICI_NO_WASM_SIMD === '1') useWasmSIMD = false
  else if (process.env.UNDICI_NO_WASM_SIMD === '0') useWasmSIMD = true

  let mod
  if (useWasmSIMD) {
    try { mod = new WebAssembly.Module(require('../llhttp/llhttp_simd-wasm.js')) } catch {}
  }
  if (!mod) {
    mod = new WebAssembly.Module(require('../llhttp/llhttp-wasm.js'))
  }
  return new WebAssembly.Instance(mod, { env: { ... } })
}
```

SIMD 版本优先，失败时回退到标准版本。模块实例全局单例（`llhttpInstance`）。

### 6.2 Parser 类详解

`Parser` 类（client-h1.js 中定义）封装了 llhttp WASM 解析器的完整生命周期。

**构造函数**：

```javascript
class Parser {
  constructor (client, socket, { exports }) {
    this.llhttp = exports
    this.ptr = this.llhttp.llhttp_alloc(constants.TYPE.RESPONSE)
    this.client = client
    this.socket = socket
    this.timeout = null
    this.statusCode = 0
    this.headers = []
    this.headersSize = 0
    this.shouldKeepAlive = false
    this.paused = false
    this.bytesRead = 0
    this.contentLength = -1
  }
}
```

`llhttp_alloc(TYPE.RESPONSE)` 在 WASM 内存中分配一个响应解析器实例。

**execute 方法** — 核心解析循环：

```javascript
execute (chunk) {
  // 如果 chunk 比 WASM 缓冲区大，重新分配
  if (chunk.length > currentBufferSize) {
    if (currentBufferPtr) llhttp.free(currentBufferPtr)
    currentBufferSize = Math.ceil(chunk.length / 4096) * 4096
    currentBufferPtr = llhttp.malloc(currentBufferSize)
  }
  // 复制 chunk 到 WASM 线性内存
  currentBuffer.set(chunk)
  // 调用 WASM 解析函数
  let ret = llhttp.llhttp_execute(this.ptr, currentBufferPtr, chunk.length)
  if (ret !== constants.ERROR.OK) {
    if (ret === constants.ERROR.PAUSED_UPGRADE) this.onUpgrade(data)
    else if (ret === constants.ERROR.PAUSED) { this.paused = true; socket.unshift(data) }
    else throw this.createError(ret, data)
  }
}
```

**关键优化**：
- WASM 内存缓冲区按 4096 对齐分配
- `currentBuffer` 使用 `Uint8Array` 视图直接映射 WASM 线性内存，避免额外拷贝
- 使用 `FastBuffer`（`Buffer[Symbol.species]`）零拷贝创建 Buffer 引用

### 6.3 回调函数与 JS-WASM 桥

llhttp 通过环境回调（`env`）将解析事件传递给 JS：

```javascript
wasm_on_status: (p, at, len) => {
  const start = at - currentBufferPtr + currentBufferRef.byteOffset
  return currentParser.onStatus(new FastBuffer(currentBufferRef.buffer, start, len))
},
wasm_on_header_field: (p, at, len) => { ... },
wasm_on_header_value: (p, at, len) => { ... },
wasm_on_headers_complete: (p, statusCode, upgrade, shouldKeepAlive) => { ... },
wasm_on_body: (p, at, len) => { ... },
wasm_on_message_complete: (p) => { ... }
```

**零拷贝 Buffer 创建**：回调中的 `at` 参数是 WASM 内存中的偏移量。通过 `at - currentBufferPtr + currentBufferRef.byteOffset` 计算出原始 Buffer 中的偏移，直接创建 `FastBuffer` 视图，无需拷贝数据。

**全局单 Parser 约束**：`currentParser`、`currentBufferRef` 是模块级全局变量。在 WASM 执行期间，同一时刻只有一个 Parser 活跃。这确保了回调中的 `currentParser` 始终正确。

### 6.4 定时器体系

Parser 使用两种定时器策略：

```javascript
const USE_NATIVE_TIMER = 0
const USE_FAST_TIMER = 1
const TIMEOUT_HEADERS = 2 | USE_FAST_TIMER    // 快速定时器
const TIMEOUT_BODY = 4 | USE_FAST_TIMER       // 快速定时器
const TIMEOUT_KEEP_ALIVE = 8 | USE_NATIVE_TIMER  // 原生定时器
```

- **Headers/Body 超时**使用快速定时器（`timers.setFastTimeout`）：考虑事件循环延迟，更精确地反映实际超时
- **Keep-Alive 超时**使用原生 `setTimeout`（`timer.unref()`）：忽略事件循环延迟，空闲时允许进程退出

**WeakRef 防泄漏**：

```javascript
this.timeoutWeakRef = new WeakRef(this)
// ...
this.timeout = timers.setFastTimeout(onParserTimeout, delay, this.timeoutWeakRef)
```

定时器回调通过 `WeakRef` 持有 Parser，如果 Parser 已被 GC，定时器回调直接返回。

### 6.5 llhttp WASM 解析器与 Client 回调衔接完整链路

**WASM 模块文件结构**：

```
lib/llhttp/
├── constants.js         # 531 行：ERROR/TYPE/FLAGS/METHODS 等枚举
├── constants.d.ts       # TypeScript 类型声明
├── llhttp.wasm          # 标准 WASM 二进制
├── llhttp-wasm.js       # 15 行：Base64 编码的 WASM 导出
├── llhttp_simd.wasm     # SIMD 加速 WASM 二进制
├── llhttp_simd-wasm.js  # 15 行：SIMD 版本导出
├── utils.js             # 12 行：辅助函数
└── utils.d.ts           # TypeScript 类型声明
```

**WASM 导出接口**（`exports` 对象）：

```javascript
// llhttp-wasm.js 导出的 Instance.exports 包含：
{
  llhttp_alloc: (type) => ptr,           // 分配解析器实例
  llhttp_free: (ptr) => void,            // 释放解析器实例
  llhttp_execute: (ptr, ptr, len) => err,// 执行解析
  llhttp_finish: (ptr) => err,           // 通知 EOF
  llhttp_resume: (ptr) => void,          // 恢复暂停的解析器
  llhttp_get_error_pos: (ptr) => pos,    // 获取错误位置
  llhttp_get_error_reason: (ptr) => ptr, // 获取错误原因字符串
  llhttp_settings_init: (ptr) => void,   // 初始化 settings
  malloc: (size) => ptr,                 // WASM 内存分配
  free: (ptr) => void,                   // WASM 内存释放
  memory: WebAssembly.Memory             // WASM 线性内存
}
```

**JS-WASM 回调桥完整实现**（`lazyllhttp` 的 env 参数）：

```javascript
return new WebAssembly.Instance(mod, {
  env: {
    // URL 解析回调（响应解析中未使用，返回 0）
    wasm_on_url: (p, at, len) => 0,

    // 状态行解析：HTTP/1.1 200 OK 中的 "OK"
    wasm_on_status: (p, at, len) => {
      assert(currentParser.ptr === p)
      const start = at - currentBufferPtr + currentBufferRef.byteOffset
      return currentParser.onStatus(new FastBuffer(currentBufferRef.buffer, start, len))
    },

    // 消息开始（每个响应触发一次）
    wasm_on_message_begin: (p) => {
      assert(currentParser.ptr === p)
      return currentParser.onMessageBegin()
    },

    // 头部字段名
    wasm_on_header_field: (p, at, len) => {
      assert(currentParser.ptr === p)
      const start = at - currentBufferPtr + currentBufferRef.byteOffset
      return currentParser.onHeaderField(new FastBuffer(currentBufferRef.buffer, start, len))
    },

    // 头部值
    wasm_on_header_value: (p, at, len) => {
      assert(currentParser.ptr === p)
      const start = at - currentBufferPtr + currentBufferRef.byteOffset
      return currentParser.onHeaderValue(new FastBuffer(currentBufferRef.buffer, start, len))
    },

    // 头部解析完成
    wasm_on_headers_complete: (p, statusCode, upgrade, shouldKeepAlive) => {
      assert(currentParser.ptr === p)
      return currentParser.onHeadersComplete(statusCode, upgrade === 1, shouldKeepAlive === 1)
    },

    // 响应体数据块
    wasm_on_body: (p, at, len) => {
      assert(currentParser.ptr === p)
      const start = at - currentBufferPtr + currentBufferRef.byteOffset
      return currentParser.onBody(new FastBuffer(currentBufferRef.buffer, start, len))
    },

    // 消息完成
    wasm_on_message_complete: (p) => {
      assert(currentParser.ptr === p)
      return currentParser.onMessageComplete()
    }
  }
})
```

**回调返回值语义**：

| 返回值 | 含义 |
|--------|------|
| `0` | 继续解析 |
| `-1` | 错误，终止解析（HPE 错误） |
| `constants.ERROR.PAUSED (21)` | 暂停解析（用于 Upgrade 场景） |
| `1` | 跳过 body（用于 HEAD 响应或 1xx 信息响应） |
| `2` | Upgrade 请求，暂停解析 |

**Parser 回调 → Request handler 回调 → 用户回调 的完整链路**：

```
socket 收到数据
    │
    ▼
onHttpSocketReadable() → parser.readMore()
    │
    ▼
parser.execute(chunk)
    │ 调用 WASM: llhttp_execute(ptr, bufferPtr, chunk.length)
    │
    ▼ (WASM 内部逐字节状态机)
    │
    ├── onMessageBegin() ──→ request.onResponseStarted()
    │
    ├── onHeaderField(buf) ──→ parser.headers.push(buf)
    │
    ├── onHeaderValue(buf) ──→ parser.headers.push(buf)
    │                          + 解析 keep-alive / connection / content-length
    │
    ├── onHeadersComplete(statusCode, upgrade, keepAlive)
    │      │
    │      ├── 设置 keepAlive 超时
    │      ├── 切换到 TIMEOUT_BODY
    │      └── request.onResponseStart(statusCode, headers, resume, statusText)
    │            │
    │            └── handler.onResponseStart(controller, statusCode, headers, statusText)
    │                  │
    │                  └── 用户代码：new Readable + callback(null, {statusCode, body, ...})
    │
    ├── onBody(buf) ──→ request.onResponseData(buf)
    │                     │
    │                     └── handler.onResponseData(controller, chunk)
    │                           │
    │                           └── res.push(chunk)  ← 用户消费
    │                               若 push 返回 false → controller.pause()
    │
    └── onMessageComplete()
           │
           ├── 校验 contentLength === bytesRead
           ├── request.onResponseEnd(headers)
           │     │
           │     └── handler.onResponseEnd(controller, trailers)
           │           │
           │           └── res.push(null)  ← EOF
           │
           ├── client[kQueue][kRunningIdx++] = null  ← 推进队首
           └── 设置 keep-alive timer 或 destroy socket
```

**WASM 内存管理**：

```javascript
// 初始分配（4096 对齐）
currentBufferSize = Math.ceil(chunk.length / 4096) * 4096
currentBufferPtr = llhttp.malloc(currentBufferSize)

// 视图映射（零拷贝）
currentBuffer = new Uint8Array(llhttp.memory.buffer, currentBufferPtr, currentBufferSize)

// 每次 execute 前复制数据到 WASM 内存
currentBuffer.set(chunk)

// 解析完成后释放（destroy 时）
llhttp.free(currentBufferPtr)
```

**错误码完整清单**（`lib/llhttp/constants.js`）：

```javascript
exports.ERROR = {
  OK: 0,                          // 成功
  INTERNAL: 1,                    // 内部错误
  STRICT: 2,                      // 严格模式错误
  LF_EXPECTED: 3,                 // 期望 LF
  UNEXPECTED_CONTENT_LENGTH: 4,   // 意外的 Content-Length
  CLOSED_CONNECTION: 5,           // 连接关闭
  INVALID_METHOD: 6,              // 无效方法
  INVALID_URL: 7,                 // 无效 URL
  INVALID_CONSTANT: 8,            // 无效常量
  INVALID_VERSION: 9,             // 无效版本
  INVALID_HEADER_TOKEN: 10,       // 无效头部 token
  INVALID_CONTENT_LENGTH: 11,     // 无效 Content-Length
  INVALID_CHUNK_SIZE: 12,         // 无效 chunk 大小
  INVALID_STATUS: 13,             // 无效状态码
  INVALID_EOF_STATE: 14,          // 无效 EOF 状态
  INVALID_TRANSFER_ENCODING: 15,  // 无效传输编码
  CB_MESSAGE_BEGIN: 16,           // 回调：消息开始
  CB_HEADERS_COMPLETE: 17,        // 回调：头部完成
  CB_MESSAGE_COMPLETE: 18,        // 回调：消息完成
  CB_CHUNK_HEADER: 19,            // 回调：chunk 头
  CB_CHUNK_COMPLETE: 20,          // 回调：chunk 完成
  PAUSED: 21,                     // 暂停
  PAUSED_UPGRADE: 22,             // 暂停（Upgrade）
  PAUSED_H2_UPGRADE: 23,          // 暂停（H2 Upgrade）
  USER: 24,                       // 用户自定义
  CR_EXPECTED: 25,                // 期望 CR
  CB_URL_COMPLETE: 26,            // 回调：URL 完成
  CB_STATUS_COMPLETE: 27,         // 回调：状态完成
  CB_HEADER_FIELD_COMPLETE: 28,   // 回调：头字段完成
  CB_HEADER_VALUE_COMPLETE: 29,   // 回调：头值完成
  UNEXPECTED_SPACE: 30,           // 意外空格
  CB_RESET: 31,                   // 回调：重置
  CB_METHOD_COMPLETE: 32,         // 回调：方法完成
  CB_VERSION_COMPLETE: 33,        // 回调：版本完成
  CB_CHUNK_EXTENSION_NAME_COMPLETE: 34,
  CB_CHUNK_EXTENSION_VALUE_COMPLETE: 35,
  CB_PROTOCOL_COMPLETE: 38        // 回调：协议完成
}
```

**SIMD 版本选择逻辑**：

```javascript
// 默认启用 SIMD，但排除 ppc64 架构（Power 9 上 SIMD 有 bug）
let useWasmSIMD = process.arch !== 'ppc64'

// 环境变量强制控制
if (process.env.UNDICI_NO_WASM_SIMD === '1') useWasmSIMD = false
else if (process.env.UNDICI_NO_WASM_SIMD === '0') useWasmSIMD = true

// Jest 测试环境强制使用非 SIMD（避免 WASM 编译开销）
const llhttpWasmData = process.env.JEST_WORKER_ID
  ? require('../llhttp/llhttp-wasm.js')
  : undefined
```

---

## 7. 连接器与 TLS

`lib/core/connect.js` 实现 TCP/TLS 连接的建立。

**buildConnector 工厂函数**：

```javascript
function buildConnector ({ allowH2, preferH2, useH2c, maxCachedSessions,
                           socketPath, timeout, session: customSession, ...opts }) {
  const sessionCache = new SessionCache(maxCachedSessions ?? 100)
  timeout = timeout ?? 10e3
  return function connect ({ hostname, host, protocol, port, servername, localAddress, httpSocket }, callback) {
    if (protocol === 'https:') {
      socket = tls.connect({
        highWaterMark: 16384,
        servername,
        session: customSession || sessionCache.get(sessionKey),
        ALPNProtocols: allowH2 ? ['http/1.1', 'h2'] : ['http/1.1'],
        socket: httpSocket,  // 升级 socket（用于 CONNECT 隧道）
        port, host: hostname
      })
      socket.on('session', (session) => sessionCache.set(sessionKey, session))
    } else {
      socket = net.connect({ highWaterMark: 64 * 1024, port, host: hostname })
      if (useH2c === true) socket.alpnProtocol = 'h2'  // 强制标记 H2C
    }
    socket.setNoDelay(true).setKeepAlive(true, 60e3)
  }
}
```

**TLS Session 缓存**（`SessionCache`）：

```javascript
class WeakSessionCache {
  constructor (maxCachedSessions) {
    this._sessionCache = new Map()
    this._sessionRegistry = new FinalizationRegistry((key) => {
      // 当 WeakRef 指向的 session 被 GC 时自动清理 Map 条目
      const ref = this._sessionCache.get(key)
      if (ref !== undefined && ref.deref() === undefined) {
        this._sessionCache.delete(key)
      }
    })
  }
  get (sessionKey) { return this._sessionCache.get(sessionKey)?.deref() ?? null }
  set (sessionKey, session) {
    this._sessionCache.set(sessionKey, new WeakRef(session))
    this._sessionRegistry.register(session, sessionKey)
  }
}
```

使用 `WeakRef` + `FinalizationRegistry` 实现自动清理的 TLS session 缓存，避免内存泄漏。默认缓存 100 个 session。

**H2C（明文 HTTP/2）支持**：

当 `useH2c: true` 时，直接在 TCP 连接上标记 `socket.alpnProtocol = 'h2'`，跳过 TLS ALPN 协商。

---

## 8. 代理体系

### 8.1 代理体系完整对比

undici 提供三种代理实现，核心差异如下：

```
                ProxyAgent                EnvHttpProxyAgent             Socks5ProxyAgent
──────────────┼─────────────────────────┼────────────────────────────┼──────────────────────
 配置方式      │ 构造函数参数 uri         │ 环境变量 / 构造函数覆盖      │ 构造函数参数 uri
 协议支持      │ HTTP/HTTPS/SOCKS5       │ HTTP/HTTPS                 │ SOCKS5
 隧道模式      │ CONNECT / 正向(非隧道)   │ CONNECT                    │ SOCKS5 CONNECT
 NO_PROXY     │ 不支持                  │ 完整支持                    │ 不支持
 认证方式      │ auth/token/URL 用户名密码 │ 同左                       │ 用户名/密码
 嵌套结构      │ 内嵌 Agent+Client       │ 内嵌 3 个子 Agent           │ 按 origin 分 Pool
 代码量        │ 378 行                  │ 175 行                      │ 282 行
 适用场景      │ 显式指定代理             │ 透明读取环境代理配置         │ SOCKS5 代理需求
```

### 8.2 ProxyAgent — HTTP CONNECT 隧道

`lib/dispatcher/proxy-agent.js` 支持三种代理模式：

**模式一：HTTP/1.1 正向代理（非隧道）**：

```javascript
class Http1ProxyWrapper extends DispatcherBase {
  [kDispatch] (opts, handler) {
    // 改写 path 为完整 URL
    opts.path = origin + path
    opts.headers = { ...this[kProxyHeaders], ...headers }
    return this.#client[kDispatch](opts, handler)
  }
}
```

用于 HTTP 请求通过 HTTP 代理（非 HTTPS 目标），请求行变为 `GET http://target.com/path HTTP/1.1`。

**模式二：CONNECT 隧道**：

```javascript
connect: async (opts, callback) => {
  const { socket, statusCode } = await this[kClient].connect({
    path: requestedPath,
    headers: { ...this[kProxyHeaders], host: opts.host, 'proxy-connection': 'keep-alive' }
  })
  if (statusCode !== 200) { /* 错误处理 */ }
  if (opts.protocol !== 'https:') {
    callback(null, socket)  // HTTP 目标直接使用隧道 socket
  } else {
    // HTTPS 目标需要在隧道 socket 上建立 TLS
    connectEndpoint({ ...opts, servername, httpSocket: socket }, callback)
  }
}
```

通过 CONNECT 方法建立隧道，在隧道内进行 TLS 握手连接到目标服务器。

**模式三：SOCKS5 代理**：

```javascript
if (protocol === 'socks5:' || protocol === 'socks:') {
  return new Socks5ProxyAgent(...)
}
```

**安全措施**：

```javascript
function throwIfProxyAuthIsSent (headers) {
  for (const key in headers) {
    if (key.toLowerCase() === 'proxy-authorization') {
      throw new InvalidArgumentError('Proxy-Authorization should be sent in ProxyAgent constructor')
    }
  }
}
```

禁止在请求头中直接设置 `Proxy-Authorization`，强制在构造函数中通过 `auth`/`token` 参数传递，防止凭据泄露。

### 8.2 EnvHttpProxyAgent — 环境变量代理

`lib/dispatcher/env-http-proxy-agent.js` 自动读取环境变量配置代理：

```javascript
const HTTP_PROXY = httpProxy ?? process.env.http_proxy ?? process.env.HTTP_PROXY
const HTTPS_PROXY = httpsProxy ?? process.env.https_proxy ?? process.env.HTTPS_PROXY
const NO_PROXY = noProxy ?? process.env.no_proxy ?? process.env.NO_PROXY
```

**NO_PROXY 匹配**：

```javascript
#shouldProxy (hostname, port) {
  if (this.#noProxyValue === '*') return false
  for (const entry of this.#noProxyEntries) {
    if (entry.port && entry.port !== port) continue
    if (hostname === entry.hostname) return false
    // 子域名匹配：example.com 匹配 api.example.com
    if (hostname.slice(-(entry.hostname.length + 1)) === `.${entry.hostname}`) return false
  }
  return true
}
```

支持精确匹配和子域名匹配，支持 IPv6 地址和端口过滤。

### 8.3 Socks5ProxyAgent — SOCKS5 代理

`lib/dispatcher/socks5-proxy-agent.js` 实现 SOCKS5 协议代理（实验性）。

**连接建立流程**：

1. 通过 `connect.js` 连接到 SOCKS5 代理服务器
2. 创建 `Socks5Client` 进行握手和认证
3. 发送 CONNECT 命令建立到目标的隧道
4. 如果目标是 HTTPS，在隧道 socket 上建立 TLS

**按 origin 分池**：

```javascript
[kDispatch] (opts, handler) {
  let pool = this[kPools].get(originKey)
  if (!pool || pool.destroyed || pool.closed) {
    pool = new Pool(origin, {
      connect: async (connectOpts, callback) => {
        const socket = await this.createSocks5Connection(targetHost, targetPort)
        // HTTPS: 在隧道上建 TLS
        if (url.protocol === 'https:') {
          finalSocket = tls.connect({ socket, servername: targetHost })
        }
        callback(null, finalSocket)
      }
    })
    this[kPools].set(originKey, pool)
  }
  return pool[kDispatch](opts, handler)
}
```

每个目标 origin 维护一个独立的 Pool，因为 SOCKS5 隧道是 per-connection 的。

---

## 9. 重试机制

`lib/dispatcher/retry-agent.js` 通过组合模式实现请求重试：

```javascript
class RetryAgent extends Dispatcher {
  dispatch (opts, handler) {
    const retry = new RetryHandler({
      ...opts,
      retryOptions: this.#options
    }, {
      dispatch: this.#agent.dispatch.bind(this.#agent),
      handler
    })
    return this.#agent.dispatch(opts, retry)
  }
}
```

`RetryHandler` 包装原始 handler，拦截错误响应，根据配置决定是否重试。重试时重新 dispatch 到底层 agent。

---

## 10. 5 种 API 风格对比

所有 API 都通过 `Dispatcher.prototype` 上的方法调用，最终走到 `this.dispatch(opts, handler)`。

### 10.1 request API

`lib/api/api-request.js`

```javascript
function request (opts, callback) {
  const handler = new RequestHandler(opts, callback)
  this.dispatch(opts, handler)
}
```

**返回值**：`{ statusCode, statusText, headers, trailers, opaque, body: Readable, context }`

**特点**：
- 返回 `Readable` 流作为响应体
- 支持 callback 和 Promise 两种调用方式
- 支持 `opaque` 透传数据
- 支持 `onInfo` 回调处理 1xx 信息响应
- 支持 `highWaterMark` 控制背压
- 支持 `AbortSignal` 取消
- 支持 `responseHeaders: 'raw'` 获取原始头部

**内部实现** — `RequestHandler`（继承 `AsyncResource`）：

```javascript
class RequestHandler extends AsyncResource {
  onRequestStart (controller, context) {
    this.abort = (reason) => controller.abort(reason)
  }
  onResponseStart (controller, statusCode, headers, statusText) {
    const res = new Readable({ resume: () => controller.resume(), ... })
    this.runInAsyncScope(callback, null, null, { statusCode, headers, body: res, ... })
  }
  onResponseData (controller, chunk) {
    if (this.res.push(chunk) === false) controller.pause()  // 背压
  }
  onResponseEnd (_controller, trailers) {
    this.res?.push(null)  // EOF
  }
}
```

### 10.2 stream API

`lib/api/api-stream.js`

```javascript
function stream (opts, factory, callback) {
  const handler = new StreamHandler(opts, factory, callback)
  this.dispatch(opts, handler)
}
```

**特点**：
- `factory` 函数接收 `{ statusCode, headers, opaque, context }` 返回 `Writable` 流
- 响应数据写入 factory 创建的 Writable
- 适合流式处理响应（如写文件）

**背压处理**：

```javascript
onResponseStart (controller, statusCode, headers) {
  const res = this.runInAsyncScope(factory, null, { statusCode, headers, opaque, context })
  res.on('drain', () => controller.resume())
  if (res.writableNeedDrain) controller.pause()
}
onResponseData (controller, chunk) {
  if (this.res.write(chunk) === false) controller.pause()
}
```

### 10.3 pipeline API

`lib/api/api-pipeline.js`

```javascript
function pipeline (opts, handler) {
  const pipelineHandler = new PipelineHandler(opts, handler)
  this.dispatch({ ...opts, body: pipelineHandler.req }, pipelineHandler)
  return pipelineHandler.ret  // 返回 Duplex 流
}
```

**特点**：
- 返回 `Duplex` 流：写入端是请求体，读取端是响应体
- `handler` 函数接收 `{ statusCode, headers, opaque, body: Readable, context }` 返回 `Readable`
- 适合管道化处理（request body -> handler transform -> response）

**内部结构**：

```javascript
this.req = new PipelineRequest()      // Readable: 请求体
this.ret = new Duplex({
  write: (chunk, encoding, callback) => {
    this.req.push(chunk, encoding) || (this.req[kResume] = callback)
  },
  read: () => { this.body?.resume() }
})
```

### 10.4 connect API

`lib/api/api-connect.js`

```javascript
function connect (opts, callback) {
  const connectHandler = new ConnectHandler(opts, callback)
  this.dispatch({ ...opts, method: 'CONNECT' }, connectHandler)
}
```

**返回值**：`{ statusCode, headers, socket, opaque, context }`

**特点**：
- 用于 HTTP CONNECT 隧道
- 返回原始 socket 用于自定义协议通信
- `onResponseStart` 会抛出 `SocketError('bad connect')`——CONNECT 不应该有正常的响应

### 10.5 upgrade API

`lib/api/api-upgrade.js`

```javascript
function upgrade (opts, callback) {
  const upgradeHandler = new UpgradeHandler(opts, callback)
  this.dispatch({ ...opts, method: opts.method || 'GET', upgrade: opts.protocol || 'Websocket' }, upgradeHandler)
}
```

**返回值**：`{ headers, socket, opaque, context }`

**特点**：
- 用于 HTTP Upgrade（如 WebSocket）
- 校验状态码为 101（H1）或 200（H2 Extended CONNECT）

### 10.6 5 种 API 调用方式、返回类型与内部实现完整对比

| API | 调用签名 | 返回类型 | 内部 Handler | 核心差异 |
|-----|---------|---------|-------------|---------|
| `request` | `(opts, callback?)` → Promise | `{statusCode, statusText, headers, trailers, body: Readable, opaque, context}` | `RequestHandler` | 通用请求；body 是 Readable；callback 或 Promise 二选一 |
| `stream` | `(opts, factory, callback?)` → Promise | `undefined`（factory 的 Writable 是响应体） | `StreamHandler` | 用户创建 Writable factory，数据写入用户流；背压通过 writableNeedDrain 传递 |
| `pipeline` | `(opts, handler)` → Duplex | `Duplex`（写入=请求体，读取=响应体） | `PipelineHandler` | 返回 Duplex；req/ret 两个内部流；request body 不能跨 redirect 重放 |
| `connect` | `(opts, callback?)` → Promise | `{statusCode, headers, socket, opaque, context}` | `ConnectHandler` | 底层 dispatch `{method:'CONNECT'}`；onResponseStart 故意抛错（不应有正常响应） |
| `upgrade` | `(opts, callback?)` → Promise | `{headers, socket, opaque, context}` | `UpgradeHandler` | 底层 dispatch `{method: opts.method\|\|'GET', upgrade: opts.protocol\|\|'Websocket'}`；H1 校验 101，H2 校验 200 |

**内部调用链统一视图**：

```
request(opts, cb)          stream(opts, factory, cb)       pipeline(opts, handler)
     │                           │                               │
     ▼                           ▼                               ▼
new RequestHandler          new StreamHandler              new PipelineHandler
     │                           │                               │
     ▼                           ▼                               ▼
this.dispatch(opts, h)      this.dispatch(opts, h)         this.dispatch(
     │                           │                            {...opts,
     │                           │                             body: handler.req},
     │                           │                            handler)
     │                           │                               │
     │                           │                               ▼
     │                           │                          return handler.ret
     │                           │                          (Duplex)
     ▼                           ▼
  所有路径最终走到：dispatcher.dispatch(opts, handler)
     │
     ▼
  ProxyAgent / Agent / PoolBase 路由
     │
     ▼
  Client[kDispatch] → _resume() → write → HTTP 上下文
     │
     ▼
  协议回调 → handler.onResponseStart / onResponseData / onResponseEnd
     │
     ▼
  RequestHandler:   StreamHandler:       PipelineHandler:
  new Readable →    factory() →          user handler(res) →
  callback(null,    writable.write(chunk)  ret.push(transformed)
  {body: res})      drain → resume
```

**body 在各 API 中的处理差异**：

```
              request/stream/pipeline          connect/upgrade
              ────────────────────────         ──────────────
请求体来源    │ opts.body 直接传入             │ 无 body（opts.body 被忽略）
              │ pipeline 时 opts.body 被       │
              │ 替换为 PipelineRequest         │
                                      │
响应体处理    │ RequestHandler:                │ ConnectHandler/UpgradeHandler:
              │   内部 new Readable +          │   onRequestUpgrade(statusCode,
              │   push(chunk) 由用户消费        │                       headers, socket)
                                      │
背压实现      │ onResponseData push=false      │ 不涉及背压，直接返回 socket
              │ → controller.pause()           │
                                      │
重定向支持    │ 由 RedirectHandler 在          │ 不支持重定向（隧道/升级）
              │ handler 外层包装实现            │
```

**callback 与 Promise 模式切换**（所有 API 统一范式）：

```javascript
function xxx (opts, callback) {
  if (callback === undefined) {
    return new Promise((resolve, reject) => {
      xxx.call(this, opts, (err, data) => {
        return err ? reject(err) : resolve(data)
      })
    })
  }
  // ... callback 逻辑
}
```

每个 API 都支持同步错误 + 异步错误处理：同步参数校验错误直接 throw（Promise 模式）或 callback(err)（callback 模式）。

### 10.7 API 选择决策树

```
需要处理响应流？
├── 是，需要读取响应体
│   ├── 只需读取，不需要写入 → request（最简单）
│   ├── 需要写文件/Writable → stream（用户创建 Writable）
│   └── 需要管道转换（transform）→ pipeline（返回 Duplex）
└── 不需要响应体，需要底层 socket
    ├── HTTP CONNECT 隧道 → connect
    └── WebSocket/协议升级 → upgrade
```

---

## 11. Request 核心模型

`lib/core/request.js` 定义了请求的内部表示。

**Request 类构造函数参数**：

```javascript
class Request {
  constructor (origin, {
    path, method, body, headers, query, idempotent, blocking, upgrade,
    headersTimeout, bodyTimeout, reset, expectContinue, servername,
    throwOnError, maxRedirections, typeOfService
  }, handler) { ... }
}
```

**body 类型处理**：

```javascript
if (body == null) { this.body = null }
else if (isStream(body)) { this.body = body /* 监听 error/end */ }
else if (isBuffer(body)) { this.body = body.byteLength ? body : null }
else if (ArrayBuffer.isView(body)) { this.body = Buffer.from(body.buffer, ...) }
else if (body instanceof ArrayBuffer) { this.body = Buffer.from(body) }
else if (typeof body === 'string') { this.body = Buffer.from(body) }
else if (isFormDataLike(body) || isIterable(body) || isBlobLike(body)) { this.body = body }
```

**idempotent 判断**：

```javascript
this.idempotent = idempotent == null
  ? method === 'HEAD' || method === 'GET' || method === 'QUERY'
  : idempotent
```

**RequestController** — 控制器：

```javascript
class RequestController {
  pause () { this.#paused = true }
  resume () { if (this.#paused) { this.#paused = false; this[kResume]?.() } }
  abort (reason) { if (!this.#aborted) { this.#aborted = true; this.#abort(reason) } }
}
```

提供给 handler 的控制器，支持 pause/resume 背压和 abort 取消。

**Handler 回调序列**：

```
onRequestStart(controller, context)     -- 请求开始
  onBodySent(chunk)                      -- 请求体块发送（可多次）
  onRequestSent()                        -- 请求体发送完成
onResponseStarted()                      -- 收到第一个字节
onResponseStart(controller, statusCode, headers, statusText)  -- 响应头
  onResponseData(controller, chunk)      -- 响应体块（可多次）
onResponseEnd(controller, trailers)      -- 响应完成
-- 或 --
onRequestUpgrade(controller, statusCode, headers, socket)  -- Upgrade
-- 或 --
onResponseError(controller, err)         -- 错误
```

---

## 12. 错误处理体系

`lib/core/errors.js` 定义了完整的错误层次结构。

**错误基类**：

```javascript
class UndiciError extends Error {
  constructor (message, options) {
    super(message, options)
    this.code = 'UND_ERR'
  }
  static [Symbol.hasInstance] (instance) {
    return instance && instance[kUndiciError] === true
  }
}
```

使用 `Symbol.hasInstance` 自定义 `instanceof` 行为，支持跨 realm 的错误识别。

**错误类型完整列表**：

| 错误类 | Code | 说明 |
|--------|------|------|
| `ConnectTimeoutError` | `UND_ERR_CONNECT_TIMEOUT` | 连接超时 |
| `HeadersTimeoutError` | `UND_ERR_HEADERS_TIMEOUT` | 等待响应头超时 |
| `HeadersOverflowError` | `UND_ERR_HEADERS_OVERFLOW` | 响应头大小溢出 |
| `BodyTimeoutError` | `UND_ERR_BODY_TIMEOUT` | 等待响应体超时 |
| `InvalidArgumentError` | `UND_ERR_INVALID_ARG` | 参数错误 |
| `RequestAbortedError` | `UND_ERR_ABORTED` | 请求被取消 |
| `InformationalError` | `UND_ERR_INFO` | 信息性错误（如 reset、idle timeout） |
| `RequestContentLengthMismatchError` | `UND_ERR_REQ_CONTENT_LENGTH_MISMATCH` | 请求 Content-Length 不匹配 |
| `ResponseContentLengthMismatchError` | `UND_ERR_RES_CONTENT_LENGTH_MISMATCH` | 响应 Content-Length 不匹配 |
| `ClientDestroyedError` | `UND_ERR_DESTROYED` | Client 已销毁 |
| `ClientClosedError` | `UND_ERR_CLOSED` | Client 已关闭 |
| `SocketError` | `UND_ERR_SOCKET` | Socket 错误 |
| `HTTPParserError` | `HPE_*` | HTTP 解析错误 |
| `ResponseExceededMaxSizeError` | `UND_ERR_RES_EXCEEDED_MAX_SIZE` | 响应体超过最大大小 |
| `BalancedPoolMissingUpstreamError` | `UND_ERR_BPL_MISSING_UPSTREAM` | 无可用上游 |
| `RequestRetryError` | `UND_ERR_REQ_RETRY` | 重试错误 |
| `ResponseError` | `UND_ERR_RESPONSE` | 响应错误 |
| `SecureProxyConnectionError` | `UND_ERR_PRX_TLS` | 代理 TLS 连接失败 |
| `ProxyConnectionError` | `UND_ERR_PRX_CONN` | 代理连接失败 |
| `MaxOriginsReachedError` | `UND_ERR_MAX_ORIGINS_REACHED` | origin 数量超限 |
| `Socks5ProxyError` | `UND_ERR_SOCKS5` | SOCKS5 代理错误 |
| `MessageSizeExceededError` | `UND_ERR_WS_MESSAGE_SIZE_EXCEEDED` | WebSocket 消息过大 |

**错误处理策略**：

- `InformationalError`（code: `UND_ERR_INFO`）是特殊的"非致命"错误，用于 reset、upgrade、idle timeout 等正常流程控制，不会触发 `onError` 的 pending 队列清空
- `SocketError`（code: `UND_ERR_SOCKET`）在 `onError` 中也视为可恢复，不触发 pending 队列清空

```javascript
function onError (client, err) {
  if (client[kRunning] === 0 && err.code !== 'UND_ERR_INFO' && err.code !== 'UND_ERR_SOCKET') {
    // 非 running 请求产生的非信息性错误，清空整个 pending 队列
    const requests = client[kQueue].splice(client[kRunningIdx])
    for (const request of requests) util.errorRequest(client, request, err)
  }
}
```

---

## 13. 诊断钩子

`lib/core/diagnostics.js` 基于 Node.js `diagnostics_channel` 模块提供全面的可观测性。

### 13.1 通道完整清单与 Payload 结构

```javascript
const channels = {
  // ─── Client 级别（4 个） ───────────────────────────────────────────────
  beforeConnect: diagnosticsChannel.channel('undici:client:beforeConnect'),
  connected: diagnosticsChannel.channel('undici:client:connected'),
  connectError: diagnosticsChannel.channel('undici:client:connectError'),
  sendHeaders: diagnosticsChannel.channel('undici:client:sendHeaders'),

  // ─── Request 级别（8 个） ─────────────────────────────────────────────
  create: diagnosticsChannel.channel('undici:request:create'),
  bodySent: diagnosticsChannel.channel('undici:request:bodySent'),
  bodyChunkSent: diagnosticsChannel.channel('undici:request:bodyChunkSent'),
  bodyChunkReceived: diagnosticsChannel.channel('undici:request:bodyChunkReceived'),
  headers: diagnosticsChannel.channel('undici:request:headers'),
  trailers: diagnosticsChannel.channel('undici:request:trailers'),
  error: diagnosticsChannel.channel('undici:request:error'),

  // ─── WebSocket 级别（5 个） ───────────────────────────────────────────
  open: diagnosticsChannel.channel('undici:websocket:open'),
  close: diagnosticsChannel.channel('undici:websocket:close'),
  socketError: diagnosticsChannel.channel('undici:websocket:socket_error'),
  ping: diagnosticsChannel.channel('undici:websocket:ping'),
  pong: diagnosticsChannel.channel('undici:websocket:pong'),

  // ─── Proxy 级别（1 个） ───────────────────────────────────────────────
  proxyConnected: diagnosticsChannel.channel('undici:proxy:connected')
}
```

### 13.2 各通道 Payload 数据结构（含 publish 位置）

**`undici:client:beforeConnect`** — 建连前（client.js connect()）

```javascript
// lib/dispatcher/client.js  line ~516
channels.beforeConnect.publish({
  connectParams: {
    host, hostname, protocol, port,        // URL 解析结果
    version: client[kHTTPContext]?.version, // 'h1' | 'h2' | undefined
    servername: client[kServerName],        // TLS SNI
    localAddress: client[kLocalAddress]     // 绑定的本地 IP
  },
  connector: client[kConnector]             // 连接工厂函数
})
```

**`undici:client:connected`** — 建连成功（client.js connect() callback）

```javascript
// lib/dispatcher/client.js  line ~572
channels.connected.publish({
  connectParams: { host, hostname, protocol, port, version, servername, localAddress },
  connector: client[kConnector],
  socket                          // 已连接的 net.Socket / tls.TLSSocket
})
```

**`undici:client:connectError`** — 建连失败（handleConnectError）

```javascript
// lib/dispatcher/client.js  line ~605
channels.connectError.publish({
  connectParams: { host, hostname, protocol, port, version, servername, localAddress },
  connector: client[kConnector],
  error                           // Error 实例（含 code/message）
})
```

**`undici:client:sendHeaders`** — 发送请求头（writeH1 / writeH2）

```javascript
// lib/dispatcher/client-h1.js  line ~1345
channels.sendHeaders.publish({
  request: { method, path, origin },  // Request 的部分字段
  headers: header,                    // 完整的 HTTP 头字符串
  socket                              // 底层 socket
})

// lib/dispatcher/client-h2.js  line ~1221
channels.sendHeaders.publish({
  request: { method, path, origin },
  headers: header,                    // 拼接后的头字符串
  socket: session[kSocket]
})
```

**`undici:request:headers`** — 收到响应头（onHeadersComplete / onResponse）

```javascript
// 通过 lib/core/request.js 内部触发
channels.headers.publish({
  request: { method, path, origin },
  response: { statusCode }
})
```

**`undici:request:trailers`** — 收到 trailers（H2 trailers / H1 trailers）

```javascript
channels.trailers.publish({
  request: { method, path, origin }
})
```

**`undici:request:error`** — 请求错误

```javascript
channels.error.publish({
  request: { method, path, origin },
  error     // Error 实例
})
```

**`undici:websocket:open`** — WebSocket 连接打开

```javascript
channels.open({
  address: { address, port }   // 服务端地址，可能为 null
})
```

**`undici:websocket:close`** — WebSocket 连接关闭

```javascript
channels.close({
  websocket,   // WebSocket 实例
  code,        // 关闭码 (1000=normal, 1006=abnormal)
  reason       // 关闭原因字符串
})
```

**`undici:websocket:socket_error`** — WebSocket 底层 socket 错误

```javascript
channels.socketError(err)   // Error 实例
```

**`undici:websocket:ping` / `undici:websocket:pong`** — WebSocket 心跳

```javascript
channels.ping()   // 收到 ping
channels.pong()   // 收到 pong
```

**`undici:proxy:connected`** — 代理隧道建立

```javascript
channels.proxyConnected({
  // 代理连接信息
})
```

### 13.3 三个 track 函数（懒订阅 + 去重）

**`trackClientEvents()`**：

```javascript
function trackClientEvents (debugLog = undiciDebugLog) {
  if (isTrackingClientEvents) return
  // 已有订阅者 → 幂等返回（处理 npm undici 与 Node 内置 undici 共存场景）
  if (channels.beforeConnect.hasSubscribers || channels.connected.hasSubscribers ||
      channels.connectError.hasSubscribers || channels.sendHeaders.hasSubscribers) {
    isTrackingClientEvents = true
    return
  }
  isTrackingClientEvents = true

  diagnosticsChannel.subscribe('undici:client:beforeConnect',
    evt => { debugLog('connecting to %s%s using %s%s', host, port, protocol, version) })
  diagnosticsChannel.subscribe('undici:client:connected',
    evt => { debugLog('connected to %s%s using %s%s', ...) })
  diagnosticsChannel.subscribe('undici:client:connectError',
    evt => { debugLog('connection to %s%s using %s%s errored - %s', ..., error.message) })
  diagnosticsChannel.subscribe('undici:client:sendHeaders',
    evt => { debugLog('sending request to %s %s%s', method, origin, path) })
}
```

**`trackRequestEvents()`**：订阅 request:headers / request:trailers / request:error。

**`trackWebSocketEvents()`**：订阅 websocket:open / close / socket_error / ping / pong。

### 13.4 订阅触发条件

```javascript
if (undiciDebugLog.enabled || fetchDebuglog.enabled) {
  trackClientEvents(fetchDebuglog.enabled ? fetchDebuglog : undiciDebugLog)
  trackRequestEvents(fetchDebuglog.enabled ? fetchDebuglog : undiciDebugLog)
}

if (websocketDebuglog.enabled) {
  trackClientEvents(undiciDebugLog.enabled ? undiciDebugLog : websocketDebuglog)
  trackWebSocketEvents(websocketDebuglog)
}
```

环境变量控制：
- `NODE_DEBUG=undici` → 启用 client + request 调试
- `NODE_DEBUG=fetch` → 同上（用 fetch Debuglog）
- `NODE_DEBUG=websocket` → 启用 client + websocket 调试

### 13.5 用户自定义订阅示例

```javascript
const diagnosticsChannel = require('diagnostics_channel')

// 订阅所有连接事件
diagnosticsChannel.subscribe('undici:client:beforeConnect', (evt) => {
  console.log('准备连接:', evt.connectParams.hostname, evt.connectParams.port)
  // 可以修改 evt.connectParams（虽然这里只是 observable）
})

// 订阅请求错误
diagnosticsChannel.subscribe('undici:request:error', (evt) => {
  telemetry.recordError({
    origin: evt.request.origin,
    path: evt.request.path,
    error: evt.error.message,
    code: evt.error.code
  })
})

// 订阅 WebSocket 事件
diagnosticsChannel.subscribe('undici:websocket:close', (evt) => {
  console.log('WebSocket 关闭:', evt.code, evt.reason)
})
```

### 13.6 diagnostics_channel 订阅最佳实践

1. **在 import undici 前订阅**：因为首次 require 时会检查 `hasSubscribers`，已在的订阅者会触发跟踪注册。
2. **使用 channel.channel() 而非 channel.tracingChannel()**：undici 使用的是稳定版的 channel API。
3. **副作用注意**：publish 是同步的，订阅者的回调执行会阻塞 publish 返回。避免在订阅者中做重操作。

---

## 14. FixedQueue 高性能队列

`lib/dispatcher/fixed-queue.js` 是从 Node.js 内部提取的高性能队列实现。

**数据结构**：单向链表 + 固定大小环形缓冲区（完整 ASCII 结构图）：

```
  注释（源码内嵌）描述了三种形态：

  形态 A: 多缓冲区链表（head 在右，tail 在左， oldest 先出）

   head                                                        tail
     |                                                           |
     v                                                           v
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
  | undefined |               | undefined | <-- top  top -->| undefined |
  +-----------+               +-----------+                  +-----------+

  形态 B: 单缓冲区，有数据

   head   tail
     |     |
     v     v
  +-----------+
  |  [null]   |
  +-----------+
  | undefined |
  | undefined |
  |   item    | <-- bottom            top --> | undefined |
  |   item    |                               | undefined |
  | undefined | <-- top            bottom --> |   item    |
  | undefined |                               |   item    |
  +-----------+

  形态 C: 单缓冲区，空

   head   tail
     |     |
     v     v
  +-----------+
  |  [null]   |
  +-----------+
  | undefined |                               | undefined |
  | undefined |                               | undefined |
  | undefined | <-- bottom            top --> | undefined |
  | undefined |                               | undefined |
  | undefined | <-- top            bottom --> | undefined |
  | undefined |                               | undefined |
  +-----------+
```

每个 `FixedCircularBuffer` 大小为 2048（V8 优化测试得出的最佳大小，必须是 2 的幂）。

**push（入队）**：

```javascript
push (data) {
  if (this.head.isFull()) {
    // Head 满了：创建新缓冲区，链接到旧 head.next，并作为新 head
    this.head = this.head.next = new FixedCircularBuffer()
  }
  this.head.push(data)
}
```

当前缓冲区满时自动创建新的缓冲区链接到链表末尾。注意：判断 `isFull()` 使用的是 `((top + 1) & kMask) === bottom`——即只浪费一个槽位，这是经典的环形缓冲区判满策略。

**shift（出队）**：

```javascript
shift () {
  const tail = this.tail
  const next = tail.shift()
  if (tail.isEmpty() && tail.next !== null) {
    // tail 已空，且有下一个缓冲区：回收当前 tail，前进到下一个
    this.tail = tail.next
    tail.next = null   // 断开引用，便于 GC
  }
  return next
}
```

出队时如果当前 tail 缓冲区为空且有下一个，自动前进并释放旧缓冲区。

**FixedCircularBuffer 内部实现**：

```javascript
const kSize = 2048
const kMask = kSize - 1   // 位掩码用于快速取模

class FixedCircularBuffer {
  bottom = 0                        // 读取指针
  top = 0                           // 写入指针
  list = new Array(kSize).fill(undefined)  // 固定大小数组
  next = null                       // 链表指针

  isEmpty () { return this.top === this.bottom }

  // 经典环形缓冲区判满：牺牲一个槽位避免判空/判满混淆
  isFull () { return ((this.top + 1) & kMask) === this.bottom }

  push (data) {
    this.list[this.top] = data
    this.top = (this.top + 1) & kMask    // 位运算取模，等价于 (top+1) % 2048
  }

  shift () {
    const nextItem = this.list[this.bottom]
    if (nextItem === undefined) return null
    this.list[this.bottom] = undefined    // 释放引用
    this.bottom = (this.bottom + 1) & kMask
    return nextItem
  }
}
```

**复杂度分析**：

| 操作 | FixedQueue | 原生 Array | 说明 |
|------|-----------|-----------|------|
| push 均摊 | O(1) | O(1) 均摊 | 满时创建新缓冲区，一次性分配 2048 槽位 |
| shift 均摊 | O(1) | O(n) | Array.shift() 需要移动所有元素 |
| 内存连续性 | 每 2048 项连续 | 整体连续 | V8 优化为 packed array |
| GC 压力 | 分段释放 | 整体释放 | 空缓冲区即丢即收 |
| 判满策略 | 浪费 1 槽 | N/A | 经典环形缓冲区 |

**在 undici 中的使用场景**：

1. **PoolBase.kQueue**：`new FixedQueue()` — 当所有 Client 都 busy 时，pending 请求入此队列等待 drain。
2. **PoolBase[kOnDrain]()**：drain 事件触发时，`queue.shift()` 出队并 dispatch 到空闲 Client。

**性能优势总结**：
- 数组固定大小（2048），V8 可优化为连续 packed array，CPU 缓存友好
- 环形缓冲区用位运算 `& kMask` 替代 `% kSize`，避免除法
- 链表分段避免了单个大数组的 GC 压力（百万级请求时显著）
- 2048 大小经过 V8 6.0-6.6 基准测试，是缓存行与内存占用的平衡点

---

## 15. TernarySearchTree 头部快速查找

`lib/core/tree.js` 实现了三叉搜索树（Ternary Search Tree, TST），用于 HTTP 头部名称的快速查找。

**用途**：将头部名称 Buffer（来自 llhttp 解析）快速映射为小写字符串，避免逐字节 `toLowerCase()` 和对象查找。

**完整实现代码**（含注释）：

```javascript
// lib/core/tree.js (160 行)

class TstNode {
  value = null              // 终止节点存储的值（小写字符串）
  left = null               // 小于当前字符的子树
  middle = null             // 等于当前字符的子树（匹配路径继续）
  right = null              // 大于当前字符的子树
  code                     // 当前节点存储的字符 charCode

  constructor (key, value, index) {
    if (index === undefined || index >= key.length) throw new TypeError('Unreachable')
    const code = this.code = key.charCodeAt(index)
    if (code > 0x7F) throw new TypeError('key must be ascii string')
    if (key.length !== ++index) {
      this.middle = new TstNode(key, value, index)   // 递归创建子节点
    } else {
      this.value = value                              // 终止节点存储值
    }
  }

  add (key, value) {
    let index = 0
    let node = this
    while (true) {
      const code = key.charCodeAt(index)
      if (code > 0x7F) throw new TypeError('key must be ascii string')
      if (node.code === code) {
        if (length === ++index) { node.value = value; break }  // 覆盖
        else if (node.middle !== null) { node = node.middle }
        else { node.middle = new TstNode(key, value, index); break }
      } else if (node.code < code) {
        if (node.left !== null) { node = node.left }
        else { node.left = new TstNode(key, value, index); break }
      } else {  // node.code > code
        if (node.right !== null) { node = node.right }
        else { node.right = new TstNode(key, value, index); break }
      }
    }
  }

  search (key) {
    const keylength = key.length
    let index = 0
    let node = this
    while (node !== null && index < keylength) {
      let code = key[index]
      // 内联大写转小写：A-Z (0x41-0x5A) → a-z (0x61-0x7A)
      if (code <= 0x5a && code >= 0x41) code |= 32
      while (node !== null) {
        if (code === node.code) {
          if (keylength === ++index) return node   // 完全匹配
          node = node.middle                       // 继续匹配下一个字符
          break
        }
        node = node.code < code ? node.left : node.right
      }
    }
    return null
  }
}

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

**内部树结构示例**（存储 "host", "connection", "content-type" 后）：

```
                     h (0x68)
                    / \
                   /   \
                 a(0x61) e(0x65)
                /        |
              s(0x73)    a(0x65)
              /          |
            t(0x74)      d(0x64)
            value:        |
          "host"        e(0x65)
                        |
                      r(0x72)
                    value:
                   "header"
```

**初始化与导出**（从 well-known 头部构建）：

```javascript
const tree = new TernarySearchTree()

for (let i = 0; i < wellknownHeaderNames.length; ++i) {
  const key = headerNameLowerCasedRecord[wellknownHeaderNames[i]]
  tree.insert(key, key)   // key = value = 小写字符串
}

module.exports = { TernarySearchTree, tree }
```

`wellknownHeaderNames` 和 `headerNameLowerCasedRecord` 定义在 `lib/core/constants.js`，包含约 100 个标准 HTTP 头部。

**使用场景**（在 Parser.onHeaderField 中）：

```javascript
// lib/dispatcher/client-h1.js
onHeaderField (buf) {
  const len = this.headers.length
  if ((len & 1) === 0) {
    this.headers.push(buf)
  } else {
    this.headers[len - 1] = Buffer.concat([this.headers[len - 1], buf])
  }
  this.trackHeader(buf.length)
  return 0
}

onHeaderValue (buf) {
  // ...
  const key = this.headers[len - 2]
  if (key.length === 10) {
    const headerName = util.bufferToLowerCasedHeaderName(key)  // ← 使用 tree
    if (headerName === 'keep-alive') { ... }
  }
  // ...
}
```

`bufferToLowerCasedHeaderName` 在 `lib/core/util.js` 中：先用 TST 查表，命中直接返回预计算的小写字符串；否则逐字节转小写。

**优化点**：
- 使用位运算 `|= 32` 代替条件判断做大小写转换（ASCII 大写转小写的最快方法）
- 三叉搜索树相比 HashMap 对短字符串（头部名称通常 < 30 字符）有更好的缓存局部性
- 树中存储了约 100 个 well-known 头部名称，命中即免计算
- 使用 `Uint8Array` 搜索而非 `Buffer.toString()` 避免字符串分配

**TST vs 其他数据结构对比**：

| 数据结构 | 短字符串查找 | 内存占用 | 缓存局部性 | 实现复杂度 |
|---------|-------------|---------|-----------|----------|
| TernarySearchTree | O(L·logN) L=长度 | 中（每个字符一个节点） | 好（树遍历） | 中 |
| HashMap | O(L) 平均 | 高（hash 计算+桶） | 一般 | 低 |
| 线性扫描 | O(N·L) | 低 | 好 | 低 |
| 有序数组+二分 | O(L·logN) | 低 | 好 | 低 |

TST 的优势在于**前缀共享**（如 "content-length" 和 "content-type" 共享 "content-" 前缀路径），节省内存。

---

## 16. Dispatcher1Wrapper 向后兼容层

`lib/dispatcher/dispatcher1-wrapper.js` 处理 undici v1 和 v2 API 的差异。

**Handler 接口变化**：

| v1 接口 | v2 接口 |
|---------|---------|
| `onConnect(abort, context)` | `onRequestStart(controller, context)` |
| `onHeaders(statusCode, rawHeaders, resume, statusMessage)` | `onResponseStart(controller, statusCode, headers, statusText)` |
| `onData(chunk)` | `onResponseData(controller, chunk)` |
| `onComplete(rawTrailers)` | `onResponseEnd(controller, trailers)` |
| `onError(err)` | `onResponseError(controller, err)` |
| `onUpgrade(statusCode, rawHeaders, socket)` | `onRequestUpgrade(controller, statusCode, headers, socket)` |

**LegacyHandlerWrapper** 自动适配：

```javascript
class LegacyHandlerWrapper {
  onRequestStart (controller, context) {
    this.#handler.onConnect?.((reason) => controller.abort(reason), context)
  }
  onResponseStart (controller, statusCode, headers, statusMessage) {
    const rawHeaders = controller?.rawHeaders ?? toRawHeaders(headers)
    if (this.#handler.onHeaders?.(statusCode, rawHeaders, () => controller.resume(), statusMessage) === false) {
      controller.pause()
    }
  }
  // ...
}
```

**强制 H1**：v1 消费者不支持 HTTP/2，因此 Dispatcher1Wrapper 强制设置 `allowH2: false`。

---

## 17. H2CClient — 明文 HTTP/2

`lib/dispatcher/h2c-client.js` 是 Client 的轻量子类，专门用于 HTTP/2 Cleartext（h2c）。

```javascript
class H2CClient extends Client {
  constructor (origin, clientOpts) {
    super(origin, {
      maxConcurrentStreams: defaultMaxConcurrentStreams,  // 默认 100
      pipelining: defaultPipelining,  // 默认 100，但必须 <= maxConcurrentStreams
      allowH2: true,
      useH2c: true  // 强制标记 alpnProtocol = 'h2'
    })
  }
}
```

通过 `useH2c: true` 让 connect.js 在 TCP 连接上伪造 `alpnProtocol = 'h2'`，使得 Client 的协议选择逻辑认为这是 H2 连接。

---

## 18. 工具函数库

`lib/core/util.js` 是整个 undici 项目的核心工具库，约 1050 行，导出 40+ 个函数。它为 Dispatcher 体系、API 层、连接器、Mock 系统等所有上层模块提供基础设施。

### 18.1 Body 处理

**wrapRequestBody(body)** — 请求体适配器，统一不同来源的 body：

```javascript
function wrapRequestBody (body) {
  if (isStream(body)) {
    // Stream: 监听 data 事件标记 kBodyUsed
    body[kBodyUsed] = false
    EE.prototype.on.call(body, 'data', function () { this[kBodyUsed] = true })
    return body
  } else if (body && typeof body.pipeTo === 'function') {
    // ReadableStream (Web Streams API): 包装为 BodyAsyncIterable
    return new BodyAsyncIterable(body)
  } else if (body && isFormDataLike(body)) {
    return body  // FormData: 直接返回
  } else if (body && typeof body !== 'string' && !ArrayBuffer.isView(body) && isIterable(body)) {
    // Iterable/AsyncIterable: 包装为 BodyAsyncIterable
    return new BodyAsyncIterable(body)
  } else {
    return body  // string/Buffer/ArrayBuffer/其它: 原样返回
  }
}
```

**BodyAsyncIterable** 类封装可迭代体，确保只被消费一次：

```javascript
class BodyAsyncIterable {
  constructor (body) {
    this[kBody] = body
    this[kBodyUsed] = false
  }
  async * [Symbol.asyncIterator] () {
    assert(!this[kBodyUsed], 'disturbed')
    this[kBodyUsed] = true
    yield * this[kBody]
  }
}
```

**bodyLength(body)** — 估算 body 字节数，用于设置 Content-Length：

```javascript
function bodyLength (body) {
  if (body == null) return 0
  else if (isStream(body)) {
    const state = body._readableState
    return state && state.objectMode === false && state.ended === true && Number.isFinite(state.length)
      ? state.length : null
  } else if (isBlobLike(body)) return body.size ?? null
  else if (isBuffer(body)) return body.byteLength
  return null
}
```

### 18.2 头部处理

**headerNameToString(value)** — 头部名称转小写，利用 TST 和预计算表：

```javascript
function headerNameToString (value) {
  return typeof value === 'string'
    ? headerNameLowerCasedRecord[value] ?? value.toLowerCase()   // 100 个 well-known 名称 O(1) 查找
    : tree.lookup(value) ?? value.toString('latin1').toLowerCase()  // Buffer 走 TST
}
```

**parseHeaders(headers, obj)** — 将 `[key, value, key, value, ...]` 扁平数组解析为 `{key: value}` 对象，处理重复头部合并：

```javascript
function parseHeaders (headers, obj = {}) {
  for (let i = 0; i < headers.length; i += 2) {
    const key = headerNameToString(headers[i])
    let val = obj[key]
    if (val !== undefined) {
      if (!Object.hasOwn(obj, key)) {
        // 原型链上的属性 → 用 defineProperty 绕过（防 __proto__ 污染）
        Object.defineProperty(obj, key, { value: headersValue, enumerable: true, configurable: true, writable: true })
      } else {
        // 重复头部 → 转为数组
        if (typeof val === 'string') { val = [val]; obj[key] = val }
        val.push(headers[i + 1].toString('latin1'))
      }
    } else {
      obj[key] = headersValue
    }
  }
  return obj
}
```

**安全措施**：当 `key === '__proto__'` 时使用 `Object.defineProperty` 而非直接赋值，防止原型污染攻击。

### 18.3 URL 处理

**parseURL(url)** — 统一 URL 解析，支持字符串、URL 对象、`{protocol, hostname, port, path}` 记录：

```javascript
function parseURL (url) {
  if (typeof url === 'string') {
    url = new URL(url)
    if (!isHttpOrHttpsPrefixed(url.origin || url.protocol)) throw ...
    return url
  }
  if (!(url instanceof URL)) {
    // 记录格式: 手动拼接 origin + path，再用 new URL 解析
    let origin = url.origin ?? `${url.protocol || ''}//${url.hostname || ''}:${port}`
    let path = url.path ?? `${url.pathname || ''}${url.search || ''}`
    return new URL(`${origin}${path}`)
  }
  return url
}
```

**isHttpOrHttpsPrefixed(value)** — 手动逐字符检查协议前缀（避免正则开销）：

```javascript
function isHttpOrHttpsPrefixed (value) {
  return value != null && value[0] === 'h' && value[1] === 't' && value[2] === 't' && value[3] === 'p'
    && (value[4] === ':' || (value[4] === 's' && value[5] === ':'))
}
```

### 18.4 连接超时

**setupConnectTimeout** — 平台差异化的连接超时设置：

```javascript
// Windows: 额外多一层 setImmediate（Windows socket 实现差异）
const setupConnectTimeout = process.platform === 'win32'
  ? (socketWeakRef, opts) => {
      const fastTimer = timers.setFastTimeout(() => {
        s1 = setImmediate(() => {
          s2 = setImmediate(() => onConnectTimeout(socketWeakRef.deref(), opts))
        })
      }, opts.timeout)
      return () => { timers.clearFastTimeout(fastTimer); clearImmediate(s1); clearImmediate(s2) }
    }
  : (socketWeakRef, opts) => {
      const fastTimer = timers.setFastTimeout(() => {
        s1 = setImmediate(() => onConnectTimeout(socketWeakRef.deref(), opts))
      }, opts.timeout)
      return () => { timers.clearFastTimeout(fastTimer); clearImmediate(s1) }
    }
```

使用 `WeakRef` 持有 socket，避免定时器阻止 GC。使用 `setImmediate` 延迟执行，确保 socket error 事件优先于超时被处理。

### 18.5 HTTP Token 验证

**isValidHTTPToken(characters)** — 校验 HTTP token（RFC 7230）：

```javascript
const validTokenChars = new Uint8Array([/* 256 字节的 0/1 位图 */])

function isValidHTTPToken (characters) {
  if (characters.length >= 12) return tokenRegExp.test(characters)  // 长串走正则
  if (characters.length === 0) return false
  for (let i = 0; i < characters.length; i++) {
    if (validTokenChars[characters.charCodeAt(i)] !== 1) return false
  }
  return true
}
```

双策略：短字符串（<12 字符）用 Uint8Array 位图查表 O(n)，长字符串用正则。位图预计算避免运行时分支。

### 18.6 Range Header 解析

```javascript
const rangeHeaderRegex = /^bytes (\d+)-(\d+)\/(\d+|\*)?$/

function parseRangeHeader (range) {
  if (range == null || range === '') return { start: 0, end: null, size: null }
  const m = rangeHeaderRegex.exec(range)
  return m ? { start: parseInt(m[1]), end: m[2] ? parseInt(m[2]) : null, size: m[3] && m[3] !== '*' ? parseInt(m[3]) : null } : null
}
```

### 18.7 事件监听器管理

**addListener / removeAllListeners** — 批量管理 EventEmitter 监听器，防止泄漏：

```javascript
function addListener (obj, name, listener) {
  const listeners = (obj[kListeners] ??= [])
  listeners.push([name, listener])
  obj.on(name, listener)
  return obj
}

function removeAllListeners (obj) {
  if (obj[kListeners] != null) {
    for (const [name, listener] of obj[kListeners]) {
      obj.removeListener(name, listener)
    }
    obj[kListeners] = null
  }
  return obj
}
```

所有注册的监听器都记录在 `kListeners` 数组中，销毁时可以一次性移除，避免 EventEmitter 泄漏。

### 18.8 Keep-Alive 解析

```javascript
const KEEPALIVE_TIMEOUT_EXPR = /timeout=(\d+)/

function parseKeepAliveTimeout (val) {
  const m = val.match(KEEPALIVE_TIMEOUT_EXPR)
  return m ? parseInt(m[1], 10) * 1000 : null
}
```

从 `Keep-Alive: timeout=5` 头部中提取超时值（秒→毫秒）。

### 18.9 协议缓存

```javascript
let lastUrlString = null
let lastProtocol = null

function getProtocolFromUrlString (urlString) {
  if (urlString === lastUrlString) return lastProtocol
  const protocol = getProtocolFromUrlStringSlow(urlString)
  lastUrlString = urlString
  lastProtocol = protocol
  return protocol
}
```

单条缓存优化：HTTP 请求通常对同一 origin 反复发送，缓存最后一次解析结果可避免重复字符串操作。

### 18.10 方法名归一化

```javascript
const normalizedMethodRecordsBase = Object.setPrototypeOf({
  delete: 'DELETE', DELETE: 'DELETE',
  get: 'GET', GET: 'GET',
  head: 'HEAD', HEAD: 'HEAD',
  options: 'OPTIONS', OPTIONS: 'OPTIONS',
  post: 'POST', POST: 'POST',
  put: 'PUT', PUT: 'PUT',
  query: 'QUERY', QUERY: 'QUERY'
}, null)  // null 原型链，防止原型污染

const normalizedMethodRecords = {
  ...normalizedMethodRecordsBase,
  patch: 'patch', PATCH: 'PATCH'  // PATCH 保持大小写（浏览器兼容性问题）
}
```

注意 `patch` 不转大写——这是一个已知的浏览器兼容性 workaround。

---

## 19. FastTimer 快速定时器

`lib/util/timers.js`（425 行）实现了自定义的快速定时器系统，用于替代 Node.js 原生 `setTimeout` 在高频场景下的性能瓶颈。

### 19.1 设计动机

Node.js 原生定时器的两个问题：
1. **精度受事件循环影响**：如果事件循环被阻塞（长时间同步操作），定时器回调会延迟触发
2. **每个定时器一个对象**：高频创建/销毁定时器带来 GC 压力

FastTimer 的解决思路：
- 使用共享的单一定时器驱动所有 FastTimer
- 用自维护的逻辑时钟 `fastNow` 替代 `Date.now()`
- 每 499ms tick 一次，批量检查到期的定时器

### 19.2 核心数据结构

```javascript
let fastNow = 0             // 逻辑时钟
const RESOLUTION_MS = 1e3   // 分辨率 1 秒
const TICK_MS = 499         // tick 间隔 = 分辨率/2 - 1（留余量）
const fastTimers = []       // 所有活跃的 FastTimer
let fastNowTimeout          // 底层 Node.js 定时器（唯一）
```

### 19.3 FastTimer 四状态机

```
  NOT_IN_LIST (-2)  <--clear()--  TO_BE_CLEARED (-1)
        |                              ^
        | refresh()                    | onTick() 移除
        v                              |
     PENDING (0)  ----onTick()--->  ACTIVE (1)  --到期-->  回调执行  --> TO_BE_CLEARED
```

| 状态 | 值 | 含义 |
|------|-----|------|
| `NOT_IN_LIST` | -2 | 未加入 fastTimers 数组 |
| `TO_BE_CLEARED` | -1 | 标记待移除，下一次 onTick 删除 |
| `PENDING` | 0 | 刚创建或 refresh()，等待第一次 tick 设置 _idleStart |
| `ACTIVE` | 1 | 活跃，等待到期 |

### 19.4 onTick 核心循环

```javascript
function onTick () {
  fastNow += TICK_MS  // 推进逻辑时钟（不依赖系统时间）
  let idx = 0, len = fastTimers.length

  while (idx < len) {
    const timer = fastTimers[idx]
    if (timer._state === PENDING) {
      timer._idleStart = fastNow - TICK_MS
      timer._state = ACTIVE
    } else if (timer._state === ACTIVE && fastNow >= timer._idleStart + timer._idleTimeout) {
      timer._state = TO_BE_CLEARED
      timer._idleStart = -1
      timer._onTimeout(timer._timerArg)  // 触发回调
    }
    if (timer._state === TO_BE_CLEARED) {
      timer._state = NOT_IN_LIST
      if (--len !== 0) fastTimers[idx] = fastTimers[len]  // 末尾元素填补空位
    } else {
      ++idx
    }
  }
  fastTimers.length = len
  if (fastTimers.length !== 0) refreshTimeout()
}
```

**关键优化**：
- `fastNow += TICK_MS` 不读取系统时钟，避免系统时钟跳变影响
- 到期的定时器用末尾元素填补（swap-and-pop），O(1) 删除
- 所有定时器到期后停止底层定时器（不空转）

### 19.5 导出接口

```javascript
module.exports = {
  setTimeout (callback, delay, arg) {
    return delay <= RESOLUTION_MS
      ? setTimeout(callback, delay, arg)   // ≤1s 走原生定时器（精度更好）
      : new FastTimer(callback, delay, arg) // >1s 走 FastTimer
  },
  clearTimeout (timeout) {
    if (timeout[kFastTimer]) timeout.clear()
    else clearTimeout(timeout)
  },
  setFastTimeout (callback, delay, arg) { return new FastTimer(callback, delay, arg) },
  clearFastTimeout (timeout) { timeout.clear() },
  now () { return fastNow }
}
```

**混合策略**：延迟 ≤1 秒使用原生 `setTimeout`（精度更好），>1 秒使用 FastTimer（性能更好）。在 Parser 中：
- Headers/Body 超时（通常 >5s）→ FastTimer
- Keep-Alive 超时 → 原生 setTimeout + unref()

---

## 20. HTTP 缓存拦截器

`lib/util/cache.js`（716 行）实现了 RFC 9111 兼容的 HTTP 缓存拦截器。

### 20.1 Cache-Control 头部解析

**parseCacheControlHeader(header)** — 完整的 Cache-Control 指令解析器：

```javascript
function parseCacheControlHeader (header) {
  const output = {}
  const directives = splitCacheControlHeaderValue(Array.isArray(header) ? header.join(',') : header)

  for (const directiveRecord of directives) {
    const directive = directiveRecord.value.toLowerCase()
    const keyValueDelimiter = directive.indexOf('=')
    // ... 解析 key/value

    switch (key) {
      case 'min-fresh': case 'max-stale': case 'max-age':
      case 's-maxage': case 'stale-while-revalidate': case 'stale-if-error':
        // 数值型指令: 验证格式, 去重取最值
        const parsedValue = Math.min(parseInt(value, 10), MAX_DELTA_SECONDS)
        if (key === 'min-fresh') output[key] = Math.max(output[key], parsedValue)  // 取最大
        else output[key] = Math.min(output[key], parsedValue)  // 取最小
        break

      case 'private': case 'no-cache':
        // 可带值的布尔型: no-cache="header1, header2"
        if (value && value[0] === '"') {
          // 跨逗号引号合并（处理 no-cache="header1, header2" 被逗号分割的情况）
          for (let j = i + 1; j < directives.length; j++) {
            // ... 合并引用列表
          }
        }
        break

      case 'public': case 'must-revalidate': case 'proxy-revalidate':
      case 'immutable': case 'no-transform': case 'must-understand':
      case 'only-if-cached':
        // 不带值的布尔型
        if (value !== undefined) { delete output[key]; continue }  // 有值则无效
        output[key] = true
        break

      case 'no-store':
        output[key] = true
        break
    }
  }
  return output
}
```

### 20.2 Vary 头部处理

**parseVaryHeader(varyHeader, headers)** — 实现 Vary 机制：

```javascript
function parseVaryHeader (varyHeader, headers) {
  if (hasVaryStar(varyHeader)) return headers  // Vary: * 匹配所有请求头

  const output = {}
  const varyingHeaders = splitVaryHeader(varyHeader)
  for (const header of varyingHeaders) {
    const trimmedHeader = trimOWS(header).toLowerCase()
    if (!isValidHTTPToken(trimmedHeader)) return undefined  // 无效头部名 → 不可缓存
    output[trimmedHeader] = headers[trimmedHeader] ?? null
  }
  return output
}
```

### 20.3 ETag 验证

```javascript
function isEtagUsable (etag) {
  if (etag.length <= 2) return false  // 空 ETag 无意义
  if (etag[0] === '"' && etag[etag.length - 1] === '"') {
    return !(etag[1] === '"' || etag.startsWith('"W/'))  // 拒绝 ""xxx"" 和 "W/xxx"
  }
  if (etag.startsWith('W/"') && etag[etag.length - 1] === '"') {
    return etag.length !== 4  // W/"" 无效（长度不足）
  }
  return false
}
```

### 20.4 请求去重

**makeDeduplicationKey(cacheKey)** — 使用 JSON.stringify 创建无碰撞的去重键：

```javascript
function makeDeduplicationKey (cacheKey, excludeHeaders) {
  const headers = {}
  if (cacheKey.headers) {
    for (const header of Object.keys(cacheKey.headers).sort()) {
      if (excludeHeaders?.has(header.toLowerCase())) continue
      headers[header] = cacheKey.headers[header]
    }
  }
  return JSON.stringify([cacheKey.origin, cacheKey.method, cacheKey.path, headers])
}
```

之前使用 `:` 和 `=` 分隔符的格式存在碰撞风险（如 `{a:"x:b=y"}` vs `{a:"x", b:"y"}`），改用 JSON.stringify 解决。

### 20.5 缓存存储接口

```javascript
function assertCacheStore (store) {
  for (const fn of ['get', 'createWriteStream', 'delete']) {
    if (typeof store[fn] !== 'function') throw new TypeError(...)
  }
}
```

缓存存储必须实现三个方法：`get(key)`、`createWriteStream(key)`、`delete(key)`。可扩展到文件系统、SQLite、Redis 等后端。

---

## 21. HTTP 日期解析器

`lib/util/date.js`（670 行）实现了 RFC 9110 规定的三种 HTTP 日期格式的手工解析器（不使用 `new Date(str)` 避免 V8 解析开销）。

### 21.1 三种格式

| 格式 | 示例 | 函数 |
|------|------|------|
| IMF-fixdate (首选) | `Sun, 06 Nov 1994 08:49:37 GMT` | `parseImfDate` |
| asctime() | `Sun Nov  6 08:49:37 1994` | `parseAscTimeDate` |
| RFC 850 (过时) | `Sunday, 06-Nov-94 08:49:37 GMT` | `parseRfc850Date` |

**路由逻辑**：通过第 4 个字符（`','` / `' '` / 其它）快速分派。

### 21.2 手工解析优化

所有日期字段都通过 `charCodeAt()` + ASCII 运算逐字符解析，避免正则表达式和 `new Date()`：

```javascript
// 月份解析示例（IMF 格式）
if (date[8] === 'J' && date[9] === 'a' && date[10] === 'n') monthIdx = 0      // Jan
else if (date[8] === 'F' && date[9] === 'e' && date[10] === 'b') monthIdx = 1 // Feb
// ... 12 个月逐一匹配

// 年份解析
const year = (yearDigit1 - 48) * 1000 + (yearDigit2 - 48) * 100
           + (yearDigit3 - 48) * 10 + (yearDigit4 - 48)

// 日期验证: 用 Date.UTC 后反向校验各字段
function makeDate (year, monthIdx, day, hour, minute, second, weekday) {
  const result = new Date(Date.UTC(year, monthIdx, day, hour, minute, second))
  if (year >= 0 && year <= 99) result.setUTCFullYear(year)  // 修正 2 位年份
  // 校验各字段是否与输入一致（防止 2月30日 等无效日期）
  return result.getUTCFullYear() === year && ... ? result : undefined
}
```

### 21.3 RFC 850 两位年份处理

```javascript
year += year < 70 ? 2000 : 1900  // 遵循 RFC 6265 规则
```

---

## 22. 运行时特性检测

`lib/util/runtime-features.js`（93 行）提供懒加载的运行时特性检测。

### 22.1 设计模式

```javascript
class RuntimeFeatures {
  #map = new Map()  // 缓存检测结果

  has (feature) {
    return this.#map.get(feature) ?? this.#detectRuntimeFeature(feature)
  }

  #detectRuntimeFeature (feature) {
    const result = detectRuntimeFeature(feature)
    this.#map.set(feature, result)  // 缓存
    return result
  }
}
```

**单例模式**：模块级 `const instance = new RuntimeFeatures()`，全局共享。

### 22.2 特性检测实现

```javascript
const lazyLoaders = {
  'node:crypto': () => require('node:crypto'),
  'node:sqlite': () => require('node:sqlite')
}

function detectRuntimeFeatureByNodeModule (moduleName) {
  try {
    lazyLoaders[moduleName]()
    return true
  } catch (err) {
    if (err.code !== 'ERR_UNKNOWN_BUILTIN_MODULE' && err.code !== 'ERR_NO_CRYPTO') throw err
    return false
  }
}
```

通过尝试 `require()` 内置模块来检测运行时支持，检测结果缓存避免重复加载。

当前支持的特性：`crypto`（加密模块）、`sqlite`（SQLite 模块）。

---

## 23. Mock 测试体系

undici 的 Mock 体系（`lib/mock/`）提供了完整的 HTTP 请求模拟能力，用于单元测试。

### 23.1 MockAgent

`lib/mock/mock-agent.js`（244 行）是 Mock 体系的核心入口，继承自 Dispatcher。

**架构设计**：

```javascript
class MockAgent extends Dispatcher {
  constructor (opts) {
    // 内部封装一个真实的 Agent
    const agent = opts?.agent ? opts.agent : new Agent(opts)
    this[kAgent] = agent
    this[kClients] = agent[kClients]  // 共享 clients Map
  }

  dispatch (opts, handler) {
    const mockDispatcher = this.get(opts.origin)
    this[kMockAgentAddCallHistoryLog](opts)  // 记录调用历史
    return this[kAgent].dispatch(opts, handler)  // 委托给内部 Agent
  }

  get (origin) {
    // 按 origin 获取或创建 MockDispatcher
    let dispatcher = this[kMockAgentGet](origin)
    if (!dispatcher) {
      dispatcher = this[kFactory](origin)
      this[kMockAgentSet](origin, dispatcher)
    }
    return dispatcher
  }
}
```

**工厂方法**：

```javascript
[kFactory] (origin) {
  return this[kOptions]?.connections === 1
    ? new MockClient(origin, mockOptions)  // 单连接
    : new MockPool(origin, mockOptions)    // 多连接池
}
```

**网络控制**：

```javascript
enableNetConnect (matcher)     // 允许真实网络请求（可选匹配器: string/RegExp/Function）
disableNetConnect ()           // 禁止所有真实网络请求
activate () / deactivate ()    // 激活/停用 Mock 模式
```

**未匹配请求处理**：当 Mock 未匹配到拦截规则时：
- `netConnect === false` → 抛出 `MockNotMatchedError`
- `netConnect === true` → 回退到真实请求
- `netConnect === [matcher]` → 仅允许匹配 origin 的真实请求

**Pending Interceptors 断言**：

```javascript
assertNoPendingInterceptors () {
  const pending = this.pendingInterceptors()
  if (pending.length > 0) {
    throw new UndiciError(`${pending.length} interceptors are pending:\n${formatter.format(pending)}`)
  }
}
```

测试结束时调用，确保所有注册的拦截器都被消费完毕。

### 23.2 MockClient 和 MockPool

`lib/mock/mock-client.js`（68 行）和 `lib/mock/mock-pool.js`（68 行）结构几乎相同，分别包装 Client 和 Pool：

```javascript
class MockClient extends Client {
  constructor (origin, opts) {
    super(origin, opts)
    this[kDispatches] = []              // 拦截规则列表
    this[kConnected] = 1               // 强制标记已连接（Mock 不真实连接）
    this[kOriginalDispatch] = this.dispatch
    this.dispatch = buildMockDispatch.call(this)  // 替换 dispatch 方法
  }

  intercept (opts) {
    return new MockInterceptor(opts, this[kDispatches])  // 注册拦截器
  }

  async [kClose] () {
    await promisify(this[kOriginalClose])()
    this[kConnected] = 0
    this[kMockAgent][Symbols.kClients].delete(this[kOrigin])  // 从 Agent 清理
  }
}
```

**核心设计**：构造函数中替换 `this.dispatch` 方法为 Mock 版本，同时保存原始 `dispatch` 引用，支持回退到真实网络。

### 23.3 MockUtils — 匹配与响应引擎

`lib/mock/mock-utils.js`（720 行）是 Mock 体系的核心引擎。

**匹配流程** (`getMockDispatch`)：

```javascript
function getMockDispatch (mockDispatches, key) {
  // 1. 过滤未消费的
  let matched = mockDispatches.filter(({ consumed }) => !consumed)
  // 2. 按 path 匹配（支持 trailing slash 忽略）
  matched = matched.filter(({ path, ignoreTrailingSlash }) =>
    ignoreTrailingSlash
      ? matchValue(removeTrailingSlash(safeUrl(path)), resolvedPathWithoutTrailingSlash)
      : matchValue(safeUrl(path), resolvedPath))
  // 3. 按 method 匹配
  matched = matched.filter(({ method }) => matchValue(method, key.method))
  // 4. 按 body 匹配
  matched = matched.filter(({ body }) => typeof body !== 'undefined' ? matchValue(body, key.body) : true)
  // 5. 按 headers 匹配
  matched = matched.filter((mockDispatch) => matchHeaders(mockDispatch, key.headers))
  return matched[0]
}
```

每一层匹配失败都抛出详细的 `MockNotMatchedError`，包含具体哪一层不匹配。

**matchValue — 三类型匹配器**：

```javascript
function matchValue (match, value) {
  if (typeof match === 'string') return match === value        // 精确匹配
  if (match instanceof RegExp) return match.test(value)        // 正则匹配
  if (typeof match === 'function') return match(value) === true // 函数匹配
  return false
}
```

**Mock Dispatch 回调**：

```javascript
function mockDispatch (opts, handler) {
  const mockDispatch = getMockDispatch(this[kDispatches], key)
  mockDispatch.timesInvoked++
  mockDispatch.consumed = !mockDispatch.persist && timesInvoked >= times
  mockDispatch.pending = timesInvoked < times

  // 支持动态回调
  if (mockDispatch.data.callback) {
    const callbackResult = callback(opts)
    if (isPromise(callbackResult)) {
      // 异步回调: 等待结果
      callbackResult.then(data => dispatchMockReply(...), err => handler.onResponseError(...))
      return true
    }
    return dispatchMockReply(...)
  }
  return dispatchMockReply(...)
}
```

**支持的 Mock 特性**：
- **times(n)**：限制拦截次数
- **persist()**：持久拦截（永不过期）
- **delay(ms)**：模拟响应延迟
- **reply(statusCode, body, headers)**：静态响应
- **reply(callback)**：动态响应（支持 async）
- **replyWithError(error)**：模拟错误

**请求体生命周期模拟**：

```javascript
function dispatchRequestBody (body, handler, controller, isAborted) {
  if (typeof handler.onBodySent !== 'function' && typeof handler.onRequestSent !== 'function')
    return body  // 无 body 钩子时跳过

  if (body && typeof body[Symbol.asyncIterator] === 'function') {
    return dispatchAsyncIterableBody(body, handler, controller, isAborted)  // 异步迭代体
  }
  if (isIterableBody(body)) {
    // 同步迭代体: 逐块回调 onBodySent
    for (const chunk of body) {
      chunks.push(chunk)
      if (!callOnBodySent(handler, controller, chunk)) return requestAborted
    }
  }
  // 普通体: 单次回调
  callOnBodySent(handler, controller, body)
  callOnRequestSent(handler, controller, isAborted)
}
```

Mock 体系完整模拟了请求体的生命周期：`onRequestStart` → `onBodySent`（多次）→ `onRequestSent` → `onResponseStart` → `onResponseData` → `onResponseEnd`。

### 23.4 PendingInterceptorsFormatter

`lib/mock/pending-interceptors-formatter.js`（43 行）将未消费的拦截器格式化为表格：

```javascript
class PendingInterceptorsFormatter {
  format (pendingInterceptors) {
    const withPrettyHeaders = pendingInterceptors.map(
      ({ method, path, data: { statusCode }, persist, times, timesInvoked, origin }) => ({
        Method: method, Origin: origin, Path: path,
        'Status code': statusCode,
        Persistent: persist ? '✅' : '❌',
        Invocations: timesInvoked,
        Remaining: persist ? Infinity : times - timesInvoked
      }))
    this.logger.table(withPrettyHeaders)  // 使用 console.table
    return this.transform.read().toString()
  }
}
```

利用 `Console` + `Transform` 流将 `console.table()` 输出捕获为字符串。

---

## 24. 统计快照与度量

`lib/util/stats.js`（32 行）提供 Client 和 Pool 的统计快照。

```javascript
class ClientStats {
  constructor (client) {
    this.connected = client[kConnected]   // 已建立的连接数
    this.pending = client[kPending]       // 等待发送的请求数
    this.running = client[kRunning]       // 正在执行的请求数
    this.size = client[kSize]             // 队列总大小（running + pending）
  }
}

class PoolStats {
  constructor (pool) {
    this.connected = pool[kConnected]     // 总连接数
    this.free = pool[kFree]               // 空闲连接数
    this.pending = pool[kPending]         // 等待发送的请求数
    this.queued = pool[kQueued]           // 排队等待分配连接的请求数
    this.running = pool[kRunning]         // 正在执行的请求数
    this.size = pool[kSize]               // 总大小
  }
}
```

**与 diagnostics_channel 的关系**：Stats 是拉模型（按需查询），diagnostics 是推模型（事件驱动）。两者互补。

---

## 25. 对 laew（Rust Agent CLI）的借鉴价值

### 25.1 Dispatcher 分层架构 → laew LLM Client 分层

**undici 的分层**：
```
Dispatcher(抽象) → DispatcherBase(生命周期) → Client(单连接) → Pool(多连接) → Agent(多origin)
```

**laew 可借鉴**：
```
LlmClient(抽象 trait) → LlmClientBase(生命周期管理) → AnthropicClient/OpenAiClient(单会话) → SessionPool(多会话) → LlmAgent(多模型)
```

undici 的 `kDispatch` / `kClose` / `kDestroy` 三个内部 Symbol 方法对应 laew 的 trait 方法，`DispatcherBase` 的生命周期状态机（closed → destroyed）可以直接移植到 Rust 的 `Drop` trait 和状态枚举。

### 25.2 三段式队列 → laew 任务队列

undici Client 的三段式队列（complete | running | pending）是处理异步请求的高效模式。laew 的 Main-Work Agent 管理 WorkFlow 列表时可以采用同样的设计：

```rust
struct WorkflowQueue {
    queue: Vec<Option<WorkflowTask>>,
    running_idx: usize,
    pending_idx: usize,
}
```

分摊 O(1) 的清理策略（累积 256 个空位后 splice）比每次完成都移除更高效。

### 25.3 工厂模式连接管理 → laew Provider 管理

undici 的 `factory` 模式允许注入自定义 Client 创建逻辑：

```javascript
const pool = new Pool(origin, { factory: (origin, opts) => new CustomClient(origin, opts) })
```

laew 的 Provider 管理可以借鉴：每条接入记录（protocol + provider_name + model_name + endpoint + api_key）对应一个工厂，根据 protocol 选择 AnthropicClient 或 OpenAiClient。

### 25.4 Agent 按 origin 路由 → laew 按 provider 路由

undici Agent 的 `Map<origin, Dispatcher>` 自动按 URL origin 路由请求到对应的 Pool。laew 可以实现类似的 `Map<provider_key, LlmClient>` 路由：

```rust
struct LlmRouter {
    clients: HashMap<String, Box<dyn LlmClient>>,
    factory: Box<dyn LlmClientFactory>,
}
```

### 25.5 FixedQueue → Rust 无锁队列

undici 的 FixedQueue（链表 + 环形缓冲区）可以用 Rust 的 `crossbeam::ArrayQueue` 或手写 `VecDeque` 替代。核心思想一致：固定大小分段，避免动态数组的频繁分配。

### 25.6 诊断钩子 → laew 可观测性

undici 的 `diagnostics_channel` 覆盖了请求全生命周期（beforeConnect → connected → sendHeaders → headers → bodyChunkReceived → trailers → error）。laew 可以借鉴这个模式，用 trait 回调或事件总线实现：

```rust
trait LlmDiagnostics {
    fn on_request_start(&self, meta: &RequestMeta) {}
    fn on_token_received(&self, token: &str) {}
    fn on_response_complete(&self, meta: &ResponseMeta) {}
    fn on_error(&self, error: &AgentError) {}
}
```

### 25.7 拦截器链 → laew 中间件

undici 的 `compose` 方法实现了洋葱模型拦截器：

```javascript
const dispatcher = agent.compose(
  (dispatch) => (opts, handler) => { /* before */ dispatch(opts, handler); /* after */ },
  retryInterceptor,
  redirectInterceptor
)
```

laew 可以用 trait + 装饰器模式实现类似功能：

```rust
trait Interceptor {
    fn intercept(&self, request: Request, next: Box<dyn Fn(Request) -> Response>) -> Response;
}
```

用于自动重试、协议转换、日志记录、成本统计等横切关注点。

### 25.8 错误分级 → laew 错误体系

undici 将错误分为致命（SocketError 导致清空队列）和信息性（InformationalError 不清空队列）两类。laew 的 AgentError 可以类似分级：

- `Fatal`：API key 无效、provider 不可达 → 终止当前任务
- `Retryable`：超时、限流 → 自动重试
- `Informational`：连接空闲、协议切换 → 记录但不中断

### 25.9 TLS Session 缓存 → laew 连接复用

undici 的 `WeakRef + FinalizationRegistry` TLS session 缓存避免了内存泄漏。在 Rust 中可以用 `Weak<Mutex<TlsSession>>` 实现类似语义，或直接用 LRU 缓存 + TTL。

### 25.10 GOAWAY/REFUSED_STREAM 重试 → laew 流中断恢复

undici 对 H2 GOAWAY 和 REFUSED_STREAM 的处理（分离 stream、重新入队、限制重试次数）可以直接借鉴到 laew 的 SSE 流中断恢复：

```rust
// 当 LLM 流中断时：
// 1. 分离当前 stream 关联
// 2. 如果 body 可重放（已完成的 buffer），重新入队
// 3. 限制重试次数（MAX_RETRY = 1）
// 4. 不可重放的（已发送部分 token）直接报错
```

### 25.11 工具函数 → laew 基础设施

undici `lib/core/util.js` 中的设计模式对 laew 有直接借鉴价值：

**HTTP Token 验证的位图查表法** → laew 可用于验证 LLM API 返回的 Content-Type 等头部：
```rust
const VALID_TOKEN_CHARS: [u8; 256] = [/* 预计算位图 */];
fn is_valid_http_token(s: &str) -> bool {
    s.len() >= 1 && s.bytes().all(|b| VALID_TOKEN_CHARS[b as usize] == 1)
}
```

**头部名称归一化的双策略**（短串查表 + 长串走正则）→ laew 处理 Anthropic/OpenAI 响应头时可采用相同策略。

**协议缓存**（单条 `lastUrlString` 缓存）→ laew 对同一 provider 反复请求时，可缓存上一次的 endpoint 解析结果。

### 25.12 FastTimer → laew Rust 定时器

FastTimer 的设计对 laew 的 Agent 超时管理有借鉴意义：

```rust
// laew 可以实现类似的"逻辑时钟 + 共享定时器"模式
struct FastTimerManager {
    now: u64,                         // 逻辑时钟
    timers: Vec<FastTimerEntry>,      // 所有活跃定时器
    native_timer: Option<JoinHandle<()>>,  // 底层异步定时器
}
```

关键原则：
- 延迟 >1s 的超时使用共享驱动器（单 tokio timer），≤1s 使用原生 `tokio::time::sleep`
- 逻辑时钟独立于系统时钟，避免时钟跳变影响
- swap-and-pop O(1) 删除

### 25.13 HTTP 缓存 → laew LLM 响应缓存

undici 的 HTTP 缓存拦截器架构可以启发 laew 的 LLM 响应缓存：

```rust
// 缓存键: provider + model + prompt_hash
// 缓存值: response + cached_at + stale_at + delete_at
trait CacheStore {
    async fn get(&self, key: &CacheKey) -> Option<CachedResponse>;
    async fn set(&self, key: &CacheKey, response: CachedResponse);
    async fn delete(&self, key: &CacheKey);
}
```

相同的 Cache-Control 解析和 Vary 头部匹配逻辑可以用于缓存 LLM 响应（相同 prompt+model → 相同响应）。

### 25.14 Mock 体系 → laew 测试基础设施

undici 的 Mock 体系设计（MockAgent 包装真实 Agent，替换 dispatch 方法）可以直接移植到 laew：

```rust
struct MockLlmClient {
    inner: Box<dyn LlmClient>,
    intercepts: Vec<MockIntercept>,
}

impl LlmClient for MockLlmClient {
    async fn send_message(&self, req: &Request) -> Result<Response> {
        if let Some(intercept) = self.find_matching_intercept(req) {
            return intercept.reply(req).await;
        }
        self.inner.send_message(req).await  // 回退到真实请求
    }
}
```

关键特性移植：
- **匹配器**：字符串精确匹配、正则、函数闭包
- **times/persist**：限制拦截次数或持久拦截
- **reply/callback**：静态响应或动态回调
- **assertNoPendingInterceptors**：测试断言确保所有拦截器被消费
- **enableNetConnect/disableNetConnect**：控制是否允许真实网络请求

---

## 附录：关键代码路径速查

| 功能 | 文件 | 关键函数/类 |
|------|------|------------|
| 入口 | `index.js` | `makeDispatcher`, `Object.assign(Dispatcher.prototype, api)` |
| 全局单例 | `lib/global.js` | `getGlobalDispatcher`, `setGlobalDispatcher` |
| Dispatcher 基类 | `lib/dispatcher/dispatcher.js` | `Dispatcher.compose()` |
| 生命周期管理 | `lib/dispatcher/dispatcher-base.js` | `DispatcherBase.dispatch/close/destroy` |
| 单连接客户端 | `lib/dispatcher/client.js` | `Client.[kDispatch]`, `_resume`, `connect` |
| H1 协议 | `lib/dispatcher/client-h1.js` | `connectH1`, `writeH1`, `Parser`, `AsyncWriter` |
| H2 协议 | `lib/dispatcher/client-h2.js` | `connectH2`, `writeH2`, `onHttp2SessionGoAway` |
| 多连接池 | `lib/dispatcher/pool.js` | `Pool.[kGetDispatcher]` |
| 池基类 | `lib/dispatcher/pool-base.js` | `PoolBase.[kDispatch]`, `[kOnDrain]` |
| 多 origin 路由 | `lib/dispatcher/agent.js` | `Agent.[kDispatch]`, `defaultFactory` |
| 加权负载均衡 | `lib/dispatcher/balanced-pool.js` | `BalancedPool.[kGetDispatcher]` |
| 轮询负载均衡 | `lib/dispatcher/round-robin-pool.js` | `RoundRobinPool.[kGetDispatcher]` |
| HTTP 代理 | `lib/dispatcher/proxy-agent.js` | `ProxyAgent`, `Http1ProxyWrapper` |
| 环境变量代理 | `lib/dispatcher/env-http-proxy-agent.js` | `EnvHttpProxyAgent.#shouldProxy` |
| SOCKS5 代理 | `lib/dispatcher/socks5-proxy-agent.js` | `Socks5ProxyAgent.createSocks5Connection` |
| 重试 | `lib/dispatcher/retry-agent.js` | `RetryAgent.dispatch` |
| 请求模型 | `lib/core/request.js` | `Request`, `RequestController` |
| 连接器 | `lib/core/connect.js` | `buildConnector`, `SessionCache` |
| 错误体系 | `lib/core/errors.js` | `UndiciError` + 20+ 子类 |
| Symbol 常量 | `lib/core/symbols.js` | 77 个 Symbol 常量 |
| 诊断钩子 | `lib/core/diagnostics.js` | `channels`, `trackClientEvents` |
| 头部快速查找 | `lib/core/tree.js` | `TernarySearchTree.search` |
| well-known 头部 | `lib/core/constants.js` | `wellknownHeaderNames`, `headerNameLowerCasedRecord` |
| llhttp 常量 | `lib/llhttp/constants.js` | `ERROR`, `METHODS`, `STATUSES`, `FLAGS` |
| 高性能队列 | `lib/dispatcher/fixed-queue.js` | `FixedQueue`, `FixedCircularBuffer` |
| v1 兼容层 | `lib/dispatcher/dispatcher1-wrapper.js` | `Dispatcher1Wrapper`, `LegacyHandlerWrapper` |
| H2C 客户端 | `lib/dispatcher/h2c-client.js` | `H2CClient` |
| request API | `lib/api/api-request.js` | `RequestHandler`, `request` |
| stream API | `lib/api/api-stream.js` | `StreamHandler`, `stream` |
| pipeline API | `lib/api/api-pipeline.js` | `PipelineHandler`, `pipeline` |
| connect API | `lib/api/api-connect.js` | `ConnectHandler`, `connect` |
| upgrade API | `lib/api/api-upgrade.js` | `UpgradeHandler`, `upgrade` |
| 核心工具库 | `lib/core/util.js` | `parseURL`, `headerNameToString`, `parseHeaders`, `bodyLength`, `isValidHTTPToken`, `setupConnectTimeout` |
| 快速定时器 | `lib/util/timers.js` | `FastTimer`, `setTimeout`, `setFastTimeout`, `onTick` |
| 缓存拦截器 | `lib/util/cache.js` | `parseCacheControlHeader`, `parseVaryHeader`, `makeCacheKey`, `makeDeduplicationKey` |
| HTTP 日期解析 | `lib/util/date.js` | `parseHttpDate`, `parseImfDate`, `parseAscTimeDate`, `parseRfc850Date` |
| 运行时特性检测 | `lib/util/runtime-features.js` | `RuntimeFeatures.has()`, `detectRuntimeFeature` |
| 统计快照 | `lib/util/stats.js` | `ClientStats`, `PoolStats` |
| Mock Agent | `lib/mock/mock-agent.js` | `MockAgent`, `get`, `dispatch`, `enableNetConnect`, `assertNoPendingInterceptors` |
| Mock Client | `lib/mock/mock-client.js` | `MockClient`, `intercept`, `cleanMocks` |
| Mock Pool | `lib/mock/mock-pool.js` | `MockPool`, `intercept`, `cleanMocks` |
| Mock 工具 | `lib/mock/mock-utils.js` | `mockDispatch`, `buildMockDispatch`, `getMockDispatch`, `matchValue`, `addMockDispatch` |
| Mock 格式化 | `lib/mock/pending-interceptors-formatter.js` | `PendingInterceptorsFormatter.format` |

---

## 附录：undici 与 laew 架构对照表

| 维度 | undici | laew |
|------|--------|------|
| 语言 | JavaScript | Rust |
| 协议 | HTTP/1.1 + HTTP/2 | Anthropic Messages + OpenAI Completions |
| 核心抽象 | Dispatcher (dispatch/close/destroy) | LlmClient trait |
| 多连接管理 | Pool → BalancedPool → RoundRobinPool | 待实现（当前单连接） |
| 多 origin 路由 | Agent (Map<origin, Dispatcher>) | Provider 路由（待实现） |
| 请求模型 | Request 类（body/header/timeout） | Session + Message 模型 |
| 队列 | FixedQueue (链表+环形缓冲区) | Vec<WorkflowTask> |
| 重试 | RetryAgent + RetryHandler | 待实现 |
| 代理 | ProxyAgent / Socks5ProxyAgent | 不适用 |
| 可观测性 | diagnostics_channel 15 个通道 | 待实现 |
| 错误分级 | 20+ 错误类型 + Info/Fatal 区分 | AgentError 枚举 |
| 拦截器 | compose + 8 个内置拦截器 | 待实现 |
| 解析器 | llhttp WASM | serde_json |
| 全局单例 | Symbol.for + globalThis | SQLite providers 表 |
| 流控 | H2 WINDOW_UPDATE + 背压 | 无（LLM 流是单向） |
| 连接复用 | TLS Session Cache (WeakRef) | HTTP keep-alive（由 reqwest 管理） |

---

## 26. WHATWG Fetch Web API 层深度分析

undici 除了作为底层 HTTP 客户端，还实现了完整的 WHATWG Fetch 规范（`lib/web/fetch/`），提供浏览器兼容的 `fetch()` / `Request` / `Response` / `Headers` / `FormData` API。

### 26.1 Fetch 控制器状态机

`lib/web/fetch/index.js` 中的 `Fetch` 类是整个 Fetch 实现的控制器，基于 EventEmitter 构建：

```javascript
class Fetch extends EE {
  #request = null
  #controller = null
  #state = 'ongoing'

  terminate (reason) {
    if (this.#state !== 'ongoing') return
    this.#state = 'terminated'
    this.#controller.abort(reason)
    // 如果 response body 存在且未被消费，关闭底层流
    const response = this.#request.response
    if (response?.body) {
      response.body.stream.cancel(reason).catch(noop)
    }
  }
}
```

**三态模型**：
| 状态 | 含义 | 触发条件 |
|------|------|---------|
| `ongoing` | 请求进行中 | 初始状态 |
| `aborted` | 用户中止 | AbortSignal 触发 |
| `terminated` | 系统终止 | 错误或重定向失败 |

### 26.2 fetch() 入口函数全链路

`fetch()` 函数是用户调用的入口，实现了 Promise.withResolvers 模式：

```javascript
async function fetch (input, init = {}) {
  // 1. Promise.withResolvers — 手动控制 resolve/reject
  const p = new Promise((resolve, reject) => {
    // 2. 构建 Request 对象
    const request = new Request(input, init)

    // 3. AbortController 链 — 用户 signal → 内部 AC
    if (request.signal.aborted) {
      reject(request.signal.reason)
      return
    }

    // 4. 创建 Fetch 控制器
    const fetch = new Fetch({
      request,
      controller: new AbortController(),
      // 5. processResponse 回调 — 收到第一个字节时 resolve
      processResponse (response) {
        if (response.aborted) { reject(new AbortError()); return }
        if (response.type === 'error') { reject(new TypeError('fetch failed')); return }
        resolve(new Response(response.body, response))
      },
      processResponseEndOfBody () { /* 清理 */ },
      processResponseConsumeBody () { /* 体消费 */ }
    })

    // 6. AbortSignal 链：用户 signal → Fetch 内部 controller
    request.signal.addEventListener('abort', () => fetch.terminate(request.signal.reason))
  })

  return p
}
```

**关键设计**：
- `Promise.withResolvers`：手动控制 Promise 生命周期，允许在 `processResponse` 回调中 resolve
- AbortController 链：用户的 `signal` 不直接控制请求，而是通过 `Fetch.terminate()` 间接控制
- 响应式 resolve：收到第一个字节就 resolve Promise，不等 body 完全接收

### 26.3 fetching → mainFetch → httpFetch 链路

请求从 `fetching()` 到网络发出的完整路径：

```
fetching()
  → mainFetch()
    → schemeFetch()          // 根据协议分派
      → httpNetworkOrCacheFetch()  // HTTP/HTTPS 请求
        → httpNetworkFetch()       // 真正的网络请求
```

**mainFetch 关键逻辑**：

```javascript
async function mainFetch (fetchParams, recursion = 0) {
  const request = fetchParams.request

  // 1. 协议检查
  if (!isValidURL(request.url)) { return makeNetworkError('invalid URL') }

  // 2. 本地协议处理（about: / blob: / data: / file:）
  if (request.url.protocol !== 'http:' && request.url.protocol !== 'https:') {
    return schemeFetch(fetchParams)
  }

  // 3. 递归深度限制（重定向）
  if (recursion > 20) return makeNetworkError('too many redirects')

  // 4. 响应染色（response tainting）
  const responseTainting = request.responseTainting

  // 5. CORS 检查
  if (request.mode === 'cors') {
    // ... CORS 预检逻辑
  }

  // 6. 发起网络请求
  const response = await httpNetworkOrCacheFetch(fetchParams)

  // 7. 重定向处理
  if (isRedirect(response.status)) {
    return httpRedirectFetch(fetchParams, response, recursion + 1)
  }

  return response
}
```

### 26.4 httpNetworkFetch — 网络请求的核心

`httpNetworkFetch` 是真正发出 HTTP 请求的函数，连接 undici 的 Dispatcher 体系：

```javascript
async function httpNetworkFetch (fetchParams) {
  const request = fetchParams.request

  // 1. 从请求中获取 Dispatcher（Agent/Client/Pool）
  const agent = request.dispatcher ?? getGlobalDispatcher()

  // 2. 构建 dispatch 选项
  const dispatchOpts = {
    path: request.url.pathname + request.url.search,
    method: request.method,
    origin: request.url.origin,
    headers: request.headersList.entries,
    body: request.body ? request.body.source : null,
    maxRedirections: 0,  // Fetch 自己处理重定向
    upgrade: request.mode === 'websocket' ? 'websocket' : undefined
  }

  // 3. 发起请求 — 通过 handler 回调接收数据
  return new Promise((resolve, reject) => {
    agent.dispatch(dispatchOpts, {
      // 请求开始
      onRequestStart (controller, context) {
        fetchParams.controller = controller
      },

      // 响应头到达
      onResponseStart (controller, statusCode, headers, statusMessage) {
        const response = makeResponse({
          status: statusCode,
          statusText: statusMessage,
          headersList: new HeadersList()
        })
        // 解析头部
        for (let i = 0; i < headers.length; i += 2) {
          response.headersList.append(headers[i], headers[i + 1])
        }
        resolve(response)
      },

      // 响应体数据块
      onResponseData (controller, chunk) {
        // 推入 ReadableStream
        if (pullAlgorithm) pullAlgorithm(chunk)
      },

      // 响应结束
      onResponseEnd (controller, trailers) {
        // 关闭 ReadableStream
        if (pullAlgorithm) pullAlgorithm(null)
      },

      // 错误
      onResponseError (controller, error) {
        reject(error)
      }
    })
  })
}
```

**核心连接**：undici 的 `agent.dispatch()` 直接被 Fetch API 调用，通过 handler 回调模式将数据推送到 WHATWG ReadableStream。

### 26.5 重定向处理（Fetch 层）

Fetch 规范有自己的重定向逻辑（不同于 Dispatcher 层的 RedirectHandler）：

```javascript
async function httpRedirectFetch (fetchParams, response, recursion) {
  if (recursion > 20) return makeNetworkError('too many redirects')

  const request = fetchParams.request
  const locationURL = responseLocationURL(response)

  // 303 See Other → 转为 GET
  if (response.status === 303 && request.method !== 'HEAD') {
    request.method = 'GET'
    request.body = null
  }

  // 跨域凭证剥离
  if (!sameOrigin(request.url, locationURL)) {
    request.headersList.delete('authorization')
    request.headersList.delete('cookie')
    request.headersList.delete('cookie2')
  }

  // 更新 URL
  request.url = locationURL
  request.redirectCount++

  // 重新发起请求
  return mainFetch(fetchParams, recursion + 1)
}
```

### 26.6 Content-Encoding 解压链

`httpNetworkFetch` 中的解压链支持多级编码：

```javascript
// 根据 Content-Encoding 构建解压管线
const codings = response.headersList.get('content-encoding')
const decoders = []

if (codings) {
  const encodings = codings.split(',').map(c => c.trim().toLowerCase())
  // 逆序构建解压链（编码顺序 = 服务端应用顺序）
  for (let i = encodings.length - 1; i >= 0; i--) {
    switch (encodings[i]) {
      case 'gzip': case 'x-gzip':
        decoders.push(zlib.createGunzip()); break
      case 'deflate': case 'x-compress':
        decoders.push(zlib.createInflate()); break
      case 'br':
        decoders.push(zlib.createBrotliDecompress()); break
      case 'zstd':
        decoders.push(zlib.createZstdDecompress()); break
    }
  }
}
```

---

## 27. Headers 与 HeadersList 实现

### 27.1 HeadersList 内部结构

`lib/web/fetch/headers.js` 中的 `HeadersList` 是 Headers 的底层数据结构：

```javascript
class HeadersList {
  #headersMap = new Map()  // lowercasedName → { name, value }
  #headersSortedMap = null // 缓存的排序结果

  append (name, value) {
    const lowercaseName = name.toLowerCase()
    const existing = this.#headersMap.get(lowercaseName)

    if (existing) {
      // set-cookie 特殊处理：不合并，保持独立条目
      if (lowercaseName === 'set-cookie') {
        existing.value = Array.isArray(existing.value)
          ? [...existing.value, value]
          : [existing.value, value]
      } else {
        // 其他头部：逗号拼接
        existing.value = `${existing.value}, ${value}`
      }
    } else {
      this.#headersMap.set(lowercaseName, { name, value })
    }

    this.#headersSortedMap = null  // 使排序缓存失效
  }
}
```

### 27.2 排序缓存优化

`toSortedArray()` 使用二分插入排序优化小规模头部：

```javascript
toSortedArray () {
  if (this.#headersSortedMap) return this.#headersSortedMap

  const headers = [...this.#headersMap.values()]
  const result = []

  for (let i = 0; i < headers.length; i++) {
    const { name, value } = headers[i]

    // set-cookie 展开为独立条目
    if (Array.isArray(value)) {
      for (let j = 0; j < value.length; j++) {
        sortedInsert(result, { name, value: value[j] })
      }
    } else {
      sortedInsert(result, { name, value })
    }
  }

  this.#headersSortedMap = result
  return result
}

function sortedInsert (arr, item) {
  // 二分插入排序（≤32 个元素时比 Array.sort 更快）
  let low = 0, high = arr.length
  while (low < high) {
    const mid = (low + high) >>> 1
    if (arr[mid].name <= item.name) low = mid + 1
    else high = mid
  }
  arr.splice(low, 0, item)
}
```

**性能洞察**：HTTP 响应的头部数量通常在 10-30 之间，二分插入排序在小数组上比 `Array.sort` 更快（避免了 TimSort 的初始归并开销）。

### 27.3 Headers 守卫系统

```javascript
class Headers {
  #guard    // none | request | request-no-cors | response | immutable
  #headersList

  append (name, value) {
    if (this.#guard === 'immutable') throw new TypeError('immutable')
    if (this.#guard === 'request' && isForbiddenHeader(name)) return  // 静默拒绝
    if (this.#guard === 'request-no-cors' && !isNoCORSSafelisted(name)) return
    this.#headersList.append(name, value)
  }
}
```

---

## 28. Response 与 FilteredResponse

### 28.1 Response 工厂模式

```javascript
function makeResponse (init = {}) {
  return {
    status: init.status ?? 200,
    statusText: init.statusText ?? '',
    type: init.type ?? 'default',
    headersList: init.headersList ?? new HeadersList(),
    urlList: init.urlList ?? [],
    body: init.body ?? null,
    aborted: false
  }
}

function makeNetworkError (reason) {
  return { ...makeResponse(), type: 'error', status: 0, body: null }
}
```

### 28.2 FilteredResponse — Proxy 模式

`makeFilteredResponse` 使用 `Proxy` 创建不同安全等级的响应视图：

```javascript
function makeFilteredResponse (response, { type }) {
  return new Proxy(response, {
    get (target, prop) {
      // opaque 响应：几乎一切都不可访问
      if (type === 'opaque') {
        if (prop === 'type') return 'opaque'
        if (prop === 'url') return ''
        if (prop === 'status') return 0
        if (prop === 'headersList') return new HeadersList()
        return undefined
      }
      // cors 响应：剥离非安全头部
      if (type === 'cors') {
        if (prop === 'headersList') return filterHeadersList(target.headersList, corsSafeHeaders)
      }
      return Reflect.get(target, prop)
    }
  })
}
```

### 28.3 FinalizationRegistry 流清理

```javascript
const streamRegistry = new FinalizationRegistry(({ stream, signal }) => {
  if (!stream.locked) {
    stream.cancel().catch(noop)
  }
})
```

当 Response 对象被 GC 回收时，自动取消未消费的 ReadableStream，防止底层连接泄漏。

---

## 29. Request 与 AbortSignal 链

### 29.1 AbortSignal 链式传播

Request 实现了复杂的 AbortSignal 链，允许多层嵌套的请求共享同一个中止信号：

```javascript
function buildAbort (signal) {
  // 1. 创建内部 AbortController
  const ac = new AbortController()
  const { signal: innerSignal } = ac

  // 2. WeakRef 保持对内部 signal 的引用（防 GC 但不阻止 GC）
  const weakRef = new WeakRef(innerSignal)

  // 3. FinalizationRegistry 清理
  const registry = new FinalizationRegistry((abortFn) => abortFn())
  registry.register(innerSignal, () => {
    signal.removeEventListener('abort', onAbort)
  })

  // 4. 信号传播
  function onAbort () {
    ac.abort(signal.reason)
  }

  if (signal.aborted) {
    ac.abort(signal.reason)
  } else {
    signal.addEventListener('abort', onAbort, { once: true })
  }

  return { signal: innerSignal, controller: ac }
}
```

**设计要点**：
- `WeakRef`：允许内部 signal 被 GC 回收，避免长期持有无用引用
- `FinalizationRegistry`：在 GC 时自动移除事件监听器，防止泄漏
- `dependentControllerMap`：全局映射表跟踪所有依赖的 controller

### 29.2 makeRequest 工厂

```javascript
function makeRequest (init = {}) {
  return {
    method: init.method ?? 'GET',
    localURLsOnly: init.localURLsOnly ?? false,
    headerList: init.headerList ?? new HeadersList(),
    url: init.url,
    urlList: init.urlList ?? [],
    body: init.body ?? null,
    client: init.client ?? null,
    serviceWorkers: init.serviceWorkers ?? 'all',
    initiator: init.initiator ?? '',
    destination: init.destination ?? '',
    mode: init.mode ?? 'no-cors',
    credentials: init.credentials ?? 'same-origin',
    useCORSPreflightFlag: init.useCORSPreflightFlag ?? false,
    responseTainting: init.responseTainting ?? 'basic',
    redirectCount: init.redirectCount ?? 0,
    // ... ~40 个字段
  }
}
```

---

## 30. Body Mixin 体系

### 30.1 extractBody — 8 种类型支持

`lib/web/fetch/body.js` 的 `extractBody` 处理所有类型的请求体：

```javascript
function extractBody (object, keepalive = false) {
  if (object instanceof Blob) {
    return { body: object.stream(), length: object.size, type: object.type }
  }

  if (object instanceof ArrayBuffer || ArrayBuffer.isView(object)) {
    const body = Buffer.from(object.buffer, object.byteOffset, object.byteLength)
    return { body, length: body.byteLength, type: null }
  }

  if (object instanceof URLSearchParams) {
    const body = Buffer.from(object.toString())
    return { body, length: body.byteLength, type: 'application/x-www-form-urlencoded;charset=UTF-8' }
  }

  if (typeof object === 'string') {
    const body = Buffer.from(object)
    return { body, length: body.byteLength, type: 'text/plain;charset=UTF-8' }
  }

  if (object instanceof FormData) {
    const boundary = FormData.getFormDataBoundary(object)
    const body = multipartEncode(object, boundary)
    return { body, length: body.byteLength, type: `multipart/form-data; boundary=${boundary}` }
  }

  if (isAsyncIterable(object)) {
    // 异步迭代器：流式发送，无法预知长度
    return { body: object, length: null, type: null }
  }
}
```

### 30.2 Body Mixin 方法

```javascript
function mixinBody (prototype) {
  // blob()
  prototype.blob = asyncBody(function () {
    return consumeBody(this, (bytes) => new Blob([bytes], { type: this.contentType }))
  })

  // arrayBuffer()
  prototype.arrayBuffer = asyncBody(function () {
    return consumeBody(this, (bytes) => bytes.buffer)
  })

  // text()
  prototype.text = asyncBody(function () {
    return consumeBody(this, (bytes) => new TextDecoder().decode(bytes))
  })

  // json()
  prototype.json = asyncBody(function () {
    return consumeBody(this, (bytes) => JSON.parse(new TextDecoder().decode(bytes)))
  })

  // formData()
  prototype.formData = asyncBody(function () {
    return consumeBody(this, (bytes, contentType) => {
      if (contentType?.essence === 'multipart/form-data') {
        return multipartFormDataParser(bytes, contentType)
      }
      return urlEncodedParse(bytes)
    })
  })
}
```

---

## 31. FormData 实现

### 31.1 FormData 类

`lib/web/fetch/formdata.js` 实现了标准的 FormData API：

```javascript
class FormData {
  #state = []      // [{name, value}]
  #boundary = null // 延迟生成的 boundary

  append (name, value, filename) {
    // Blob → File 转换
    if (value instanceof Blob && !(value instanceof File)) {
      value = new File([value], 'blob', { type: value.type })
    }
    if (filename !== undefined && value instanceof File) {
      value = new File([value], filename, { type: value.type, lastModified: value.lastModified })
    }
    this.#state.push({ name, value })
  }

  set (name, value, filename) {
    // 替换第一个同名条目，删除其余
    const idx = this.#state.findIndex(e => e.name === name)
    if (idx !== -1) {
      this.#state = [
        ...this.#state.slice(0, idx),
        makeEntry(name, value, filename),
        ...this.#state.slice(idx + 1).filter(e => e.name !== name)
      ]
    } else {
      this.#state.push(makeEntry(name, value, filename))
    }
  }

  // Boundary 生成（延迟 + 懒初始化）
  static getFormDataBoundary (formData) {
    if (formData.#boundary) return formData.#boundary
    return formData.#boundary = `----formdata-undici-0${`${random(1e11)}`.padStart(11, '0')}`
  }
}
```

### 31.2 Multipart 解析器

`lib/web/fetch/formdata-parser.js` 实现了完整的 multipart/form-data 解析：

```javascript
function multipartFormDataParser (input, mimeType) {
  const boundaryString = mimeType.parameters.get('boundary')
  if (!boundaryString) throw parsingError('missing boundary')

  const boundary = Buffer.from(`--${boundaryString}`, 'utf8')
  const entryList = []
  const position = { position: 0 }

  // 跳过 preamble（RFC 2046 Section 5.1.1）
  const firstBoundaryIndex = input.indexOf(boundary)
  position.position = firstBoundaryIndex

  while (true) {
    // 1. 匹配 boundary
    if (!input.subarray(position.position, position.position + boundary.length).equals(boundary)) {
      throw parsingError('expected boundary')
    }
    position.position += boundary.length

    // 2. 检测结束标记 (-- 后跟 boundary)
    if (bufferStartsWith(input, dd, position)) return entryList

    // 3. 跳过 CRLF
    position.position += 2

    // 4. 解析 headers（Content-Disposition / Content-Type / Content-Transfer-Encoding）
    const { name, filename, contentType, encoding } = parseMultipartFormDataHeaders(input, position)
    position.position += 2

    // 5. 提取 body（找到下一个 boundary）
    const boundaryIndex = input.indexOf(boundary.subarray(2), position.position)
    let body = input.subarray(position.position, boundaryIndex - 4)
    if (encoding === 'base64') body = Buffer.from(body.toString(), 'base64')

    // 6. 构建 File 或 string
    const value = filename !== null
      ? new File([body], filename, { type: contentType ?? 'text/plain' })
      : decoderIgnoreBOM.decode(Buffer.from(body))

    entryList.push(makeEntry(name, value, filename))
    position.position += body.length + 2 // body + CRLF
  }
}
```

**Content-Disposition 属性解析**支持三种格式：
- `name="value"` — 标准引号字符串
- `filename*=utf-8''encoded` — RFC 5987 扩展编码（支持非 ASCII 文件名）
- `name=token` — 无引号的 token 值

### 31.3 Boundary 验证

```javascript
function validateBoundary (boundary) {
  // 长度约束：27-70 字符
  if (boundary.length < 27 || boundary.length > 70) return false
  // 字符约束：ASCII 字母数字 + ' - _
  for (let i = 0; i < boundary.length; i++) {
    const cp = boundary.charCodeAt(i)
    if (!((cp >= 0x30 && cp <= 0x39) || (cp >= 0x41 && cp <= 0x5a) ||
          (cp >= 0x61 && cp <= 0x7a) || cp === 0x27 || cp === 0x2d || cp === 0x5f)) {
      return false
    }
  }
  return true
}
```

---

## 32. 拦截器体系详解

undici 的拦截器通过 `Dispatcher.compose()` 实现洋葱模型，每个拦截器是一个 `(dispatch) => (opts, handler) => void` 函数。

### 32.1 Redirect Handler（重定向跟随）

`lib/handler/redirect-handler.js`（229 行）实现了 HTTP 重定向的完整处理。

**核心逻辑**：

```javascript
class RedirectHandler {
  constructor (dispatch, maxRedirections, opts, handler) {
    this.dispatch = dispatch
    this.maxRedirections = maxRedirections
    this.opts = { ...opts, body: wrapRequestBody(opts.body) }
    this.history = []  // 重定向历史（用于循环检测）
  }

  onResponseStart (controller, statusCode, headers, statusMessage) {
    // 301/302 + POST → GET（RFC 7231）
    if ((statusCode === 301 || statusCode === 302) && this.opts.method === 'POST') {
      this.opts.method = 'GET'
      if (isStream(this.opts.body)) destroy(this.opts.body)
      this.opts.body = null
    }

    // 303 → GET（HEAD 保持）
    if (statusCode === 303 && this.opts.method !== 'HEAD') {
      this.opts.method = 'GET'
      this.opts.body = null
    }

    // 确定是否继续重定向
    this.location = (this.history.length >= this.maxRedirections ||
                     isDisturbed(this.opts.body) ||
                     !redirectableStatusCodes.includes(statusCode))
      ? null
      : headers.location

    // 循环检测
    if (this.opts.origin) {
      const redirectUrl = new URL(this.opts.path, this.opts.origin)
      for (const historyUrl of this.history) {
        if (historyUrl.toString() === redirectUrl.toString()) {
          throw new InvalidArgumentError('Redirect loop detected')
        }
      }
      this.history.push(redirectUrl)
    }
  }

  onResponseEnd (controller, trailers) {
    if (this.location) {
      // 跟随重定向：重新 dispatch
      this.dispatch(this.opts, this)
    } else {
      this.handler.onResponseEnd(controller, trailers)
    }
  }
}
```

**头部清理规则**：

```javascript
function cleanRequestHeaders (headers, removeContent, unknownOrigin, stripHeaders, stripHeadersOnCrossOrigin) {
  // 1. 始终移除: Host
  // 2. 303 或 POST→GET: 移除所有 Content-* 头
  // 3. 跨域重定向: 移除 Authorization / Cookie / Proxy-Authorization
  // 4. 用户自定义: stripHeadersOnRedirect / stripHeadersOnCrossOriginRedirect
}
```

### 32.2 Retry Handler（重试机制）

`lib/handler/retry-handler.js`（548 行）实现了请求级自动重试，支持 Range 续传。

**RetryController 代理模式**：

```javascript
class RetryController {
  constructor (onAbort) {
    this.#onAbort = onAbort
    this.target = null  // 指向当前活跃的连接控制器
  }

  pause () { this.target?.pause() }
  resume () { this.target?.resume() }
  abort (reason) {
    this.target?.abort(reason)
    this.#onAbort(reason)  // 通知 handler 取消 backoff 定时器
  }
}
```

**重试策略**：

```javascript
static [kRetryHandlerDefaultRetry] (err, { state, opts }, cb) {
  const { counter } = state
  const { maxRetries, minTimeout, maxTimeout, timeoutFactor } = opts.retryOptions

  // 1. 不可重试的错误码
  if (code && code !== 'UND_ERR_REQ_RETRY' && !errorCodes.includes(code)) {
    cb(err); return
  }

  // 2. 方法不在允许列表
  if (!methods.includes(method)) { cb(err); return }

  // 3. 状态码不在允许列表
  if (statusCode && !statusCodes.includes(statusCode)) { cb(err); return }

  // 4. 超过最大重试次数
  if (counter > maxRetries) { cb(err); return }

  // 5. 计算退避时间
  let retryTimeout
  if (retryAfterHeader > 0) {
    retryTimeout = Math.min(retryAfterHeader, maxTimeout)  // 尊重 Retry-After
  } else {
    retryTimeout = Math.min(minTimeout * timeoutFactor ** (counter - 1), maxTimeout)
  }

  return setTimeout(() => cb(null), retryTimeout)  // 返回定时器引用，支持取消
}
```

**Range 续传**：

```javascript
retry () {
  if (this.start !== 0) {
    // 已消费部分数据，使用 Range 头续传
    const headers = { range: `bytes=${this.start}-${this.end ?? ''}` }
    if (this.etag != null) {
      headers['if-match'] = this.etag  // ETag 校验确保数据一致性
    }
    this.opts = { ...this.opts, headers: { ...this.opts.headers, ...headers } }
  }
  this.dispatch(this.opts, this)
}
```

**关键状态追踪**：
- `start`：已消费的字节数
- `end`：期望的总字节数（从 Content-Length 或 Content-Range 提取）
- `etag`：弱 ETag 用于数据一致性验证
- `headersSent`：是否已向下游发送过响应头

### 32.3 Cache Handler（HTTP 缓存）

`lib/handler/cache-handler.js`（802 行）实现了 RFC 9111 兼容的 HTTP 缓存。

**可缓存性判断**：

```javascript
function canCacheResponse (cacheType, statusCode, resHeaders, cacheControlDirectives, reqHeaders) {
  // 1. 状态码必须是最终响应（≥200）
  if (statusCode < 200) return false

  // 2. no-store 指令
  if (cacheControlDirectives['no-store']) return false

  // 3. 共享缓存 + private 指令
  if (cacheType === 'shared' && cacheControlDirectives.private === true) return false

  // 4. Vary: * 不可缓存
  if (hasVaryStar(resHeaders.vary)) return false

  // 5. Authorization 头处理（RFC 9111 Section 3.5）
  if (reqHeaders?.authorization) {
    if (!cacheControlDirectives.public &&
        !cacheControlDirectives['s-maxage'] &&
        !cacheControlDirectives['must-revalidate']) {
      return false
    }
  }

  return true
}
```

**新鲜度计算**（优先级递减）：

```javascript
function determineStaleAt (cacheType, now, age, resHeaders, responseDate, cacheControlDirectives, hasValidator) {
  // 共享缓存优先 s-maxage
  if (cacheType === 'shared' && cacheControlDirectives['s-maxage'] !== undefined) {
    return cacheControlDirectives['s-maxage'] * 1000
  }
  // max-age
  if (cacheControlDirectives['max-age'] !== undefined) {
    return cacheControlDirectives['max-age'] * 1000
  }
  // Expires 头
  if (resHeaders.expires) {
    const expiresDate = parseHttpDate(resHeaders.expires)
    return expiresDate ? expiresDate.getTime() - now : 0
  }
  // 启发式缓存（last-modified 的 10%）
  if (resHeaders['last-modified']) {
    const lastModified = parseHttpDate(resHeaders['last-modified'])
    return lastModified ? (now - lastModified.getTime()) * 0.1 : undefined
  }
  // immutable 指令
  if (cacheControlDirectives.immutable) return 31536000000 // 1 年
}
```

**删除时间计算**（stale-while-revalidate / stale-if-error）：

```javascript
function determineDeleteAt (baseTime, cachedAt, cacheControlDirectives, staleAt) {
  let deleteAt = staleAt

  // stale-while-revalidate：过期后仍可使用，同时后台验证
  if (cacheControlDirectives['stale-while-revalidate']) {
    deleteAt = Math.max(deleteAt, staleAt + cacheControlDirectives['stale-while-revalidate'] * 1000)
  }

  // stale-if-error：过期后出错时仍可使用
  if (cacheControlDirectives['stale-if-error']) {
    deleteAt = Math.max(deleteAt, staleAt + cacheControlDirectives['stale-if-error'] * 1000)
  }

  return deleteAt
}
```

**304 Not Modified 处理**：

```javascript
// 收到 304 时：合并缓存的 body 与新响应的 headers
if (statusCode === 304) {
  const cachedValue = store.get(cacheKey)
  if (cachedValue) {
    value.statusCode = cachedValue.statusCode
    value.statusMessage = cachedValue.statusMessage
    value.headers = { ...cachedValue.headers, ...strippedHeaders }
    // 将缓存的 body 复制到新的写入流
    writeStream = store.createWriteStream(cacheKey, value)
    for (const chunk of cachedValue.body) {
      writeStream.write(chunk)
      handler.onResponseData(controller, chunk)
    }
  }
}
```

### 32.4 Decompress Handler（多编码解压）

`lib/interceptor/decompress.js` 中的解压拦截器：

```javascript
class DecompressHandler extends DecoratorHandler {
  constructor (opts, handler) {
    super(handler)
    this.encodings = []  // 解压编码栈
    this.skipForStatus = opts?.skipForStatus ?? (s => s >= 400)
  }

  onResponseStart (controller, statusCode, headers, statusMessage) {
    // 跳过 HEAD、204、304 和可选的 4xx/5xx
    if (statusCode === 204 || statusCode === 304 || this.skipForStatus(statusCode)) {
      return super.onResponseStart(controller, statusCode, headers, statusMessage)
    }

    const contentEncoding = headers['content-encoding']
    if (contentEncoding) {
      const encodings = contentEncoding.split(',').map(e => e.trim().toLowerCase())

      // CVE 缓解：最多 5 层编码（防止 zip bomb）
      if (encodings.length > 5) {
        throw new Error('Too many content-encodings')
      }

      // 构建解压链
      this.decompressors = encodings.reverse().map(encoding => createDecompressor(encoding))
      this.pipeline = pipeline(...this.decompressors, noop)

      // 移除 content-encoding 和 content-length
      delete headers['content-encoding']
      delete headers['content-length']
    }

    super.onResponseStart(controller, statusCode, headers, statusMessage)
  }

  onResponseData (controller, chunk) {
    if (this.decompressors) {
      this.decompressors[0].write(chunk)  // 写入第一个解压器
    } else {
      super.onResponseData(controller, chunk)
    }
  }
}
```

---

## 33. EventSource 实现

### 33.1 状态机

`lib/web/eventsource/eventsource.js`（494 行）实现了 Server-Sent Events 规范：

```javascript
class EventSource extends EventTarget {
  #state = 0  // CONNECTING
  #reconnectionTime = 3000  // 默认 3 秒重连
  #lastEventId = ''

  constructor (url, eventSourceInitDict = {}) {
    super()
    this.#reconnectionTime = eventSourceInitDict.eventSourceInitDict?.reconnectionTime ?? 3000

    // 启动第一次连接
    this.#connect()
  }

  #connect () {
    this.#state = 0  // CONNECTING
    this.dispatchEvent(new Event('connecting'))

    // 使用 Fetch API 发起请求
    fetching({
      request: makeRequest({
        url: this.#url,
        method: 'GET',
        headersList: new HeadersList([
          ['accept', 'text/event-stream'],
          ['last-event-id', this.#lastEventId]  // 重连时携带 Last-Event-ID
        ])
      }),
      processResponse (response) {
        if (response.status !== 200) {
          this.#state = 2  // CLOSED
          this.dispatchEvent(new Event('error'))
          return
        }
        if (!response.headersList.get('content-type')?.startsWith('text/event-stream')) {
          this.#state = 2
          return
        }
        this.#state = 1  // OPEN
        this.dispatchEvent(new Event('open'))
      },
      processResponseEndOfBody () {
        // 连接关闭 → 自动重连
        if (this.#state !== 2) {
          setTimeout(() => this.#connect(), this.#reconnectionTime)
        }
      }
    })
  }

  close () {
    this.#state = 2  // CLOSED
  }
}
```

### 33.2 自动重连机制

EventSource 的重连策略：
1. 服务端关闭连接 → 触发 `processResponseEndOfBody`
2. 检查 `readyState !== CLOSED`
3. 使用 `setTimeout` 延迟 `reconnectionTime` 毫秒
4. 重新连接时在 `Last-Event-ID` 头中携带最后收到的事件 ID
5. 服务端可通过 `retry:` 字段调整重连间隔

---

## 34. MemoryCacheStore 内存缓存

### 34.1 三级限制

```javascript
class MemoryCacheStore extends EventEmitter {
  #maxCount = 1024        // 最大条目数
  #maxSize = 104857600    // 最大总大小 100MB
  #maxEntrySize = 5242880 // 单条最大 5MB
  #entries = new Map()     // origin:path → [{...entry}]

  // 逐条写入流
  createWriteStream (key, val) {
    const entry = { ...key, ...val, body: [], size: 0 }

    return new Writable({
      write (chunk, encoding, callback) {
        entry.size += chunk.byteLength
        if (entry.size > this.#maxEntrySize) {
          this.destroy()  // 超过单条限制 → 销毁流
        } else {
          entry.body.push(chunk)
        }
        callback(null)
      },
      final (callback) {
        // 写入完成 → 存储
        const entries = this.#entries.get(topLevelKey) ?? []
        entries.push(entry)
        this.#entries.set(topLevelKey, entries)
        this.#size += entry.size

        // 驱逐策略：超出限制时删除每个 key 的前半部分
        if (this.#size > this.#maxSize || this.#count > this.#maxCount) {
          for (const [key, entries] of this.#entries) {
            for (const entry of entries.splice(0, Math.ceil(entries.length / 2))) {
              this.#size -= entry.size
              this.#count -= 1
            }
            if (entries.length === 0) this.#entries.delete(key)
          }
        }
      }
    })
  }
}
```

### 34.2 Vary 匹配

```javascript
function varyMatches (key, entry) {
  if (entry.vary == null) return true

  for (const headerName in entry.vary) {
    if (!headerValueEquals(key.headers?.[headerName], entry.vary[headerName])) {
      return false
    }
  }
  return true
}

function headerValueEquals (lhs, rhs) {
  if (lhs == null && rhs == null) return true
  if (lhs == null || rhs == null) return false
  if (Array.isArray(lhs) && Array.isArray(rhs)) {
    if (lhs.length !== rhs.length) return false
    return lhs.every((v, i) => v === rhs[i])
  }
  return lhs === rhs
}
```

---

## 35. 对 laew 借鉴价值（补充）

### 35.1 Fetch 控制器 → laew SSE 流控制器

undici 的 `Fetch` 三态控制器（ongoing/aborted/terminated）可以直接借鉴到 laew 的 SSE 流控制：

```rust
enum StreamState {
    Ongoing,
    Aborted(String),    // 用户中止 + 原因
    Terminated(String), // 系统终止 + 原因
}

struct StreamController {
    state: StreamState,
    request: LlmRequest,
}
```

### 35.2 AbortSignal 链 → laew 任务取消传播

undici 的 WeakRef + FinalizationRegistry 模式在 Rust 中对应 `Weak<T>` + `Drop`：

```rust
use std::sync::{Arc, Weak};

struct AbortSignal {
    cancelled: Arc<AtomicBool>,
    parent: Option<Weak<AbortSignal>>,
}

impl AbortSignal {
    fn propagate_abort(&self) {
        if let Some(parent) = &self.parent {
            if let Some(parent) = parent.upgrade() {
                parent.cancelled.store(true, Ordering::Release);
            }
        }
    }
}

impl Drop for AbortSignal {
    fn drop(&mut self) {
        // Rust 的 Drop 替代 FinalizationRegistry
    }
}
```

### 35.3 Retry Handler → laew LLM 重试

undici 的重试策略（退避 + Range 续传 + ETag 校验）可以适配到 laew 的 LLM 请求重试：

```rust
struct RetryPolicy {
    max_retries: u32,         // 默认 5
    min_timeout: Duration,    // 默认 500ms
    max_timeout: Duration,    // 默认 30s
    timeout_factor: f64,      // 默认 2.0
    retryable_codes: Vec<u16>, // [429, 500, 502, 503, 504]
}

impl RetryPolicy {
    fn backoff_duration(&self, attempt: u32) -> Duration {
        let base = self.min_timeout.as_millis() as f64 * self.timeout_factor.powi(attempt as i32);
        Duration::from_millis(base.min(self.max_timeout.as_millis() as u64))
    }
}
```

**关键差异**：LLM 请求的 body（提示词）是可重放的（不像 HTTP POST 可能有副作用），因此 laew 的重试比 undici 更简单，不需要 Range 续传。

### 35.4 Cache Handler → laew 响应缓存

laew 可以实现简单的 LLM 响应缓存：

```rust
struct LlmCache {
    entries: HashMap<CacheKey, CacheEntry>,
    max_size: usize,
}

struct CacheEntry {
    response: LlmResponse,
    cached_at: Instant,
    ttl: Duration,
}

// Cache Key = hash(system_prompt + messages + model + temperature)
impl LlmCache {
    fn get(&self, key: &CacheKey) -> Option<&LlmResponse> {
        self.entries.get(key).and_then(|entry| {
            if entry.cached_at.elapsed() < entry.ttl {
                Some(&entry.response)
            } else {
                None
            }
        })
    }
}
```

### 35.5 HeadersList 排序缓存 → laew 消息头部优化

laew 的 Anthropic/OpenAI 请求构建时，头部通常固定不变。可以借鉴 HeadersList 的排序缓存模式，预构建头部快照：

```rust
struct RequestHeaders {
    entries: Vec<(String, String)>,
    sorted: Option<Vec<(String, String)>>,  // 延迟排序缓存
}

impl RequestHeaders {
    fn to_sorted(&mut self) -> &[(String, String)] {
        if self.sorted.is_none() {
            let mut sorted = self.entries.clone();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            self.sorted = Some(sorted);
        }
        self.sorted.as_ref().unwrap()
    }
}
```

### 35.6 FastTimer → laew 超时管理

laew 的 LLM 请求超时（通常 30s-120s）可以借鉴 FastTimer 的批量检查模式：

```rust
use tokio::time::{Instant, Duration};

struct TimerWheel {
    timers: Vec<TimerEntry>,
    tick_interval: Duration,
}

struct TimerEntry {
    deadline: Instant,
    callback: Box<dyn FnOnce()>,
}

impl TimerWheel {
    async fn run(&mut self) {
        loop {
            tokio::time::sleep(self.tick_interval).await;
            let now = Instant::now();
            // swap-and-pop 删除到期定时器
            let mut i = 0;
            while i < self.timers.len() {
                if self.timers[i].deadline <= now {
                    let entry = self.timers.swap_remove(i);
                    (entry.callback)();
                } else {
                    i += 1;
                }
            }
        }
    }
}
```

### 35.7 Mock 体系 → laew 测试 Mock

undici 的 MockAgent/MockClient 模式为 laew 提供了测试基础设施的参考：

```rust
trait MockLlmClient: LlmClient {
    fn intercept(&mut self, matcher: RequestMatcher, response: MockResponse);
    fn assert_no_pending(&self);
}

struct MockAgent {
    interceptors: Vec<MockInterceptor>,
    real_client: Option<Box<dyn LlmClient>>,
}
```

---

## 36. 关键代码路径速查（补充）

| 功能 | 文件 | 关键函数/类 |
|------|------|------------|
| Fetch 入口 | `lib/web/fetch/index.js` | `fetch()`, `Fetch` class, `fetching()` |
| Fetch 链路 | `lib/web/fetch/index.js` | `mainFetch()`, `schemeFetch()`, `httpFetch()`, `httpNetworkFetch()` |
| Body 处理 | `lib/web/fetch/body.js` | `extractBody()`, `consumeBody()`, `mixinBody()` |
| Headers | `lib/web/fetch/headers.js` | `HeadersList`, `Headers`, `toSortedArray()` |
| Response | `lib/web/fetch/response.js` | `makeResponse()`, `makeFilteredResponse()`, `streamRegistry` |
| Request | `lib/web/fetch/request.js` | `makeRequest()`, `buildAbort()`, AbortSignal 链 |
| Fetch 工具 | `lib/web/fetch/util.js` | `fullyReadBody()`, `determineRequestsReferrer()`, `InflateStream` |
| FormData | `lib/web/fetch/formdata.js` | `FormData`, `makeEntry()` |
| FormData 解析 | `lib/web/fetch/formdata-parser.js` | `multipartFormDataParser()`, `validateBoundary()` |
| EventSource | `lib/web/eventsource/eventsource.js` | `EventSource`, 自动重连, Last-Event-ID |
| Cache API | `lib/web/cache/cache.js` | `Cache`, `#batchCacheOperations()`, `#queryCache()` |
| 重定向 Handler | `lib/handler/redirect-handler.js` | `RedirectHandler`, `cleanRequestHeaders()` |
| 重试 Handler | `lib/handler/retry-handler.js` | `RetryHandler`, `RetryController`, Range 续传 |
| 缓存 Handler | `lib/handler/cache-handler.js` | `CacheHandler`, `canCacheResponse()`, `determineStaleAt()` |
| 内存缓存 | `lib/cache/memory-cache-store.js` | `MemoryCacheStore`, `varyMatches()` |
| HTTP 缓存工具 | `lib/util/cache.js` | `parseCacheControlHeader()`, `parseVaryHeader()`, `isEtagUsable()` |
| HTTP 日期 | `lib/util/date.js` | `parseImfDate()`, `parseRfc850Date()`, `parseAscTimeDate()` |
| 快速定时器 | `lib/util/timers.js` | `FastTimer`, `onTick()`, 混合策略 |
| 特性检测 | `lib/util/runtime-features.js` | `RuntimeFeatures`, 懒加载检测 |
| Mock 体系 | `lib/mock/mock-agent.js` | `MockAgent`, `enableNetConnect()`, `assertNoPendingInterceptors()` |
| Mock 引擎 | `lib/mock/mock-utils.js` | `getMockDispatch()`, `matchValue()`, `mockDispatch()` |
| Mock 格式化 | `lib/mock/pending-interceptors-formatter.js` | `PendingInterceptorsFormatter` |
| 统计 | `lib/util/stats.js` | `ClientStats`, `PoolStats` |


---

> 文档完成日期：2026-09-05（初版），2026-09-06（深化补全）
> 分析文件数：40+ 源码文件
> 文档总行数：5300+（从 4127 行深化至 5363 行，新增 1236 行）
> 章节数：36 个编号章节 + 目录 + 3 个附录
