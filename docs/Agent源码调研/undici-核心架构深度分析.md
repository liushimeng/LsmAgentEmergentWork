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

### 5.4 双协议关键差异对比

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

### 8.1 ProxyAgent — HTTP CONNECT 隧道

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

### 5 种 API 对比表

| API | 请求体 | 响应体 | 返回值 | 适用场景 |
|-----|--------|--------|--------|---------|
| `request` | opts.body | Readable 流 | Promise/callback | 通用 HTTP 请求 |
| `stream` | opts.body | factory 返回的 Writable | Promise/callback | 流式写入（写文件等） |
| `pipeline` | Duplex 写入端 | handler 返回的 Readable | Duplex 流 | 管道化处理 |
| `connect` | 无 | 原始 socket | Promise/callback | HTTP CONNECT 隧道 |
| `upgrade` | 无 | 原始 socket | Promise/callback | WebSocket 升级 |

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

**通道列表**：

```javascript
const channels = {
  // Client 级别
  beforeConnect: channel('undici:client:beforeConnect'),    // 连接前
  connected: channel('undici:client:connected'),             // 连接成功
  connectError: channel('undici:client:connectError'),       // 连接失败
  sendHeaders: channel('undici:client:sendHeaders'),         // 发送请求头

  // Request 级别
  create: channel('undici:request:create'),                   // 请求创建
  bodySent: channel('undici:request:bodySent'),               // 请求体发送完成
  bodyChunkSent: channel('undici:request:bodyChunkSent'),     // 请求体块发送
  bodyChunkReceived: channel('undici:request:bodyChunkReceived'), // 响应体块接收
  headers: channel('undici:request:headers'),                 // 响应头接收
  trailers: channel('undici:request:trailers'),               // trailer 接收
  error: channel('undici:request:error'),                     // 请求错误

  // WebSocket 级别
  open: channel('undici:websocket:open'),
  close: channel('undici:websocket:close'),
  socketError: channel('undici:websocket:socket_error'),
  ping: channel('undici:websocket:ping'),
  pong: channel('undici:websocket:pong'),

  // Proxy 级别
  proxyConnected: channel('undici:proxy:connected')           // 代理隧道建立
}
```

**懒加载订阅**：

```javascript
if (undiciDebugLog.enabled || fetchDebuglog.enabled) {
  trackClientEvents()
  trackRequestEvents()
}
```

只有在 `NODE_DEBUG=undici` 或 `NODE_DEBUG=fetch` 环境变量启用时才订阅诊断通道，零开销。

**订阅去重**：

```javascript
if (channels.beforeConnect.hasSubscribers) {
  isTrackingClientEvents = true
  return  // 已有订阅者，不重复订阅
}
```

防止 Node.js 内置 undici 和 npm 安装的 undici 重复订阅。

---

## 14. FixedQueue 高性能队列

`lib/dispatcher/fixed-queue.js` 是从 Node.js 内部提取的高性能队列实现。

**数据结构**：单向链表 + 固定大小环形缓冲区

```
  head (写入端)                                  tail (读取端)
    |                                               |
    v                                               v
 +-----------+       +-----------+       +-----------+
 | Buffer N  | ----> | Buffer N-1| ----> | Buffer 0  |
 | (newest)  |       |           |       | (oldest)  |
 +-----------+       +-----------+       +-----------+
```

每个 `FixedCircularBuffer` 大小为 2048（V8 优化测试得出的最佳大小，必须是 2 的幂）。

**push（入队）**：

```javascript
push (data) {
  if (this.head.isFull()) {
    this.head = this.head.next = new FixedCircularBuffer()
  }
  this.head.push(data)
}
```

当前缓冲区满时自动创建新的缓冲区链接到链表末尾。

**shift（出队）**：

```javascript
shift () {
  const tail = this.tail
  const next = tail.shift()
  if (tail.isEmpty() && tail.next !== null) {
    this.tail = tail.next  // 回收空缓冲区
    tail.next = null
  }
  return next
}
```

出队时如果当前 tail 缓冲区为空且有下一个，自动前进并释放旧缓冲区。

**性能优势**：
- 数组固定大小，V8 可以优化为连续内存
- 环形缓冲区避免了数组的 shift/unshift（O(n)）操作
- 链表分段避免了单个大数组的 GC 压力
- 2048 大小经过 V8 基准测试优化

---

## 15. TernarySearchTree 头部快速查找

`lib/core/tree.js` 实现了三叉搜索树（Ternary Search Tree, TST），用于 HTTP 头部名称的快速查找。

**用途**：将头部名称 Buffer（来自 llhttp 解析）快速映射为小写字符串，避免逐字节 `toLowerCase()` 和对象查找。

**search 方法**：

```javascript
search (key) {  // key: Uint8Array
  let index = 0
  let node = this
  while (node !== null && index < keylength) {
    let code = key[index]
    // 内联大写转小写：A-Z (0x41-0x5A) → a-z (0x61-0x7A)
    if (code <= 0x5a && code >= 0x41) code |= 32
    while (node !== null) {
      if (code === node.code) {
        if (keylength === ++index) return node
        node = node.middle
        break
      }
      node = node.code < code ? node.left : node.right
    }
  }
  return null
}
```

**优化点**：
- 使用位运算 `|= 32` 代替条件判断做大小写转换
- 三叉搜索树相比 HashMap 对短字符串（头部名称通常 < 30 字符）有更好的缓存局部性
- 树中存储了约 100 个 well-known 头部名称

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

> 文档完成日期：2026-09-05
> 分析文件数：40+ 源码文件
> 文档总行数：4000+
> 章节数：36 个编号章节 + 目录 + 3 个附录
