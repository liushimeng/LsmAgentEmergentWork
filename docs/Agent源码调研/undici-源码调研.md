# undici 源码调研

> 项目：undici（https://github.com/nodejs/undici）
> 定位：Node.js 官方 HTTP/1.1 + HTTP/2 客户端库，Node.js 内置 `fetch()` 的底层实现
> 语言：JavaScript（Node.js）
> 版本：v8.10.1
> 源码规模：109 个 JS 文件，~38,703 行
> 分析日期：2026-09-05

---

## 目录

1. [项目概览](#1-项目概览)
2. [架构总图](#2-架构总图)
3. [核心模块一览](#3-核心模块一览)
4. [Dispatcher 体系](#4-dispatcher-体系)
5. [HTTP/1.1 与 HTTP/2 双协议](#5-http11-与-http2-双协议)
6. [连接池与负载均衡](#6-连接池与负载均衡)
7. [llhttp WASM 解析器](#7-llhttp-wasm-解析器)
8. [5 种 API 风格](#8-5-种-api-风格)
9. [Web API 层](#9-web-api-层)
10. [Interceptor 拦截器体系](#10-interceptor-拦截器体系)
11. [Mock 测试系统](#11-mock-测试系统)
12. [代理支持](#12-代理支持)
13. [错误处理与诊断](#13-错误处理与诊断)
14. [对 laew 的借鉴价值](#14-对-laew-的借鉴价值)
15. [详细分析文档索引](#15-详细分析文档索引)

---

## 1. 项目概览

Undici 是 Node.js 官方维护的 HTTP 客户端库，名字取自意大利语"11"（1.1 → 11 → Undici），也是 Stranger Things 的彩蛋。它是 Node.js 内置 `fetch()` API 的底层实现，性能远超 `axios`、`got`、`node-fetch` 等流行库。

**核心特性**：
- 原生 HTTP/1.1 流水线（pipelining）与 HTTP/2 多路复用（multiplexing）
- 基于 llhttp 的 WASM 解析器（非 Node.js 内置 http_parser）
- 连接池管理与负载均衡（Pool、BalancedPool、RoundRobinPool）
- 5 种 API 风格：request / stream / pipeline / connect / upgrade
- 完整的 Web 标准 API：fetch / WebSocket / EventSource / Cache / Cookies
- 8 个可组合拦截器：cache / retry / redirect / dns / decompress / deduplicate / dump / response-error
- 代理支持：HTTP CONNECT、SOCKS5、环境变量自动代理
- 诊断钩子（diagnostics_channel）全覆盖
- Mock 测试系统：录制/回放/快照

**性能基准**（50 TCP 连接，pipeline 深度 10，Node 24.14.1）：

| 模式 | req/sec | vs 最慢 |
|------|---------|---------|
| node-fetch | 4,711 | - |
| undici fetch | 5,438 | +15% |
| undici pipeline | 13,470 | +186% |
| undici request | 16,850 | +258% |
| undici stream | 18,488 | +292% |
| undici dispatch | 20,786 | +341% |

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
              +--------------------+--------------------+
              |                    |                    |
    +---------v------+  +---------v------+  +----------v-------+
    | Client (单连接) |  | Pool (多连接)   |  | Agent (多origin) |
    | client-h1.js   |  | pool-base.js   |  | agent.js         |
    | client-h2.js   |  | pool.js        |  +--------+---------+
    +---------+------+  +---------+------+           |
              |                   |         +--------v---------+
              |                   |         | BalancedPool     |
              |                   |         | RoundRobinPool   |
              |                   |         +------------------+
              |                   |
    +---------v-------------------v---------+
    |          llhttp WASM 解析器            |
    |  (llhttp-wasm.js / llhttp_simd-wasm.js)|
    +-------------------+-------------------+
                        |
    +-------------------v-------------------+
    |          Interceptor 层               |
    |  cache → retry → redirect → dns       |
    |  → decompress → deduplicate → dump    |
    |  → response-error                     |
    +-------------------+-------------------+
                        |
    +-------------------v-------------------+
    |          Handler 层 (装饰器)           |
    |  CacheHandler / RetryHandler          |
    |  RedirectHandler / DeduplicationHandler|
    +---------------------------------------+
```

---

## 3. 核心模块一览

| 模块 | 文件数 | 说明 |
|------|--------|------|
| `lib/dispatcher/` | 16 | Dispatcher 体系：Client/Pool/Agent/Proxy/Retry |
| `lib/core/` | 9 | 核心：Request/Connect/Errors/Util/Diagnostics |
| `lib/api/` | 8 | 5 种 API 风格：request/stream/pipeline/connect/upgrade |
| `lib/web/fetch/` | 11 | Fetch API 完整实现 |
| `lib/web/websocket/` | 12 | WebSocket RFC 6455 实现 |
| `lib/web/eventsource/` | 3 | EventSource/SSE 实现 |
| `lib/web/cache/` | 3 | Cache API 实现 |
| `lib/web/cookies/` | 4 | Cookie 处理 |
| `lib/web/webidl/` | 1 | WebIDL 类型系统 |
| `lib/web/infra/` | 1 | WHATWG Infra 规范工具 |
| `lib/web/subresource-integrity/` | 1 | SRI 校验 |
| `lib/interceptor/` | 8 | 8 个可组合拦截器 |
| `lib/handler/` | 6 | Handler 装饰器 |
| `lib/mock/` | 12 | Mock 测试系统（录制/回放/快照） |
| `lib/cache/` | 2 | HTTP 缓存后端（Memory/SQLite） |
| `lib/llhttp/` | 4 | llhttp WASM 解析器 |
| `lib/encoding/` | 1 | 编码支持 |
| `lib/util/` | 5 | 工具层 |

---

## 4. Dispatcher 体系

Dispatcher 是 undici 的核心抽象，采用 EventEmitter 模式，所有 HTTP 操作通过 `dispatch(opts, handler)` 方法执行。

### 4.1 类层次

```
EventEmitter
  └── Dispatcher (抽象基类)
        ├── DispatcherBase (状态管理 + 队列)
        │     ├── Client (单连接)
        │     │     ├── client-h1.js (HTTP/1.1 连接上下文)
        │     │     └── client-h2.js (HTTP/2 连接上下文)
        │     ├── Pool (多连接)
        │     │     └── pool-base.js (池基类)
        │     └── Agent (多 origin 路由)
        ├── BalancedPool (加权负载均衡)
        ├── RoundRobinPool (轮询负载均衡)
        ├── RetryAgent (重试包装)
        ├── ProxyAgent (HTTP CONNECT 代理)
        ├── EnvHttpProxyAgent (环境变量代理)
        └── Socks5ProxyAgent (SOCKS5 代理)
```

### 4.2 关键设计

- **dispatch(opts, handler)**：核心方法，opts 包含 origin/method/path/headers/body，handler 是回调接口
- **状态机**：CLOSED → CONNECTING → CONNECTED → BUSY → IDLE → CLOSED
- **FixedQueue**：高性能固定大小队列，用于请求排队
- **TernarySearchTree**：三叉搜索树，用于 HTTP 头部快速查找

---

## 5. HTTP/1.1 与 HTTP/2 双协议

| 特性 | HTTP/1.1 (client-h1.js) | HTTP/2 (client-h2.js) |
|------|-------------------------|----------------------|
| 连接模型 | 单连接 + pipeline | 多路复用（stream） |
| 解析器 | llhttp WASM | Node.js 内置 http2 |
| Keep-alive | 显式管理 | 内置 |
| 流量控制 | 无 | WINDOW_UPDATE |
| 头部压缩 | 无 | HPACK |
| 协议选择 | ALPN 协商 | ALPN 协商 |

**ALPN 协商**：连接建立时通过 TLS ALPN 扩展自动选择协议版本，支持 h2 → http/1.1 降级。

---

## 6. 连接池与负载均衡

### Pool
- 基于 PoolBase，维护多个 Client 实例
- 按需创建连接，空闲超时自动关闭
- 支持 `connections` 参数限制最大连接数

### BalancedPool
- 加权负载均衡，支持 `addUpstream/removeUpstream`
- 权重动态调整，失败自动降权

### RoundRobinPool
- 轮询调度，简单公平
- 适合同构后端

---

## 7. llhttp WASM 解析器

undici 自带 llhttp 的 WASM 版本，不依赖 Node.js 内置 http_parser：

- `llhttp-wasm.js`：标准 WASM 模块
- `llhttp_simd-wasm.js`：SIMD 优化版本（自动检测 CPU 支持）
- 回调驱动：on_message_begin / on_url / on_header_field / on_header_value / on_body / on_message_complete
- JS-WASM 桥：通过 `__indirect_function_table` 实现零拷贝回调

---

## 8. 5 种 API 风格

| API | 用途 | 特点 |
|-----|------|------|
| `request()` | 通用请求 | 返回完整 Response，最易用 |
| `stream()` | 流式响应 | 返回 Readable，适合大文件 |
| `pipeline()` | 流式管道 | pipe 链式处理 |
| `connect()` | TCP 隧道 | 代理/隧道场景 |
| `upgrade()` | 协议升级 | WebSocket 等 |

---

## 9. Web API 层

### Fetch API（~7,860 行）
- 严格对齐 WHATWG Fetch 规范，每步注释引用规范编号
- `fetch()` 入口 → Request 构建 → Body 处理 → Headers 序列化 → Response 解析
- 支持 FormData、Data URL、AbortSignal、redirect/follow/error 模式

### WebSocket（~2,000+ 行）
- RFC 6455 完整实现：握手 → 帧编解码 → 收发 → 关闭
- 支持 permessage-deflate 压缩扩展
- WebSocketStream API（Promise 包装）

### EventSource/SSE（~580 行）
- SSE 流式解析器：text/event-stream 格式
- 支持自动重连、Last-Event-ID

### Cache API（~1,060 行）
- 双后端：MemoryCacheStore（内存）/ SQLiteCacheStore（持久化）
- HTTP 缓存语义：ETag/If-None-Match、Vary、max-age/stale-while-revalidate

### Cookies（~420 行）
- Set-Cookie 解析 + Cookie 序列化
- 大小限制、域名验证

---

## 10. Interceptor 拦截器体系

8 个可组合拦截器，通过装饰器模式链式调用：

| 拦截器 | 功能 |
|--------|------|
| `cache` | HTTP 缓存（Memory/SQLite 双后端） |
| `retry` | 自动重试（指数退避、可配置重试条件） |
| `redirect` | 自动重定向（可配置最大次数） |
| `dns` | DNS 缓存（减少 DNS 查询） |
| `decompress` | 自动解压（gzip/deflate/br） |
| `deduplicate` | 请求去重（相同请求合并） |
| `dump` | 调试 dump（打印请求/响应） |
| `response-error` | 响应错误转换（4xx/5xx → Error） |

**组合方式**：
```javascript
const client = new Client(url)
  .compose(cache())
  .compose(retry())
  .compose(redirect())
```

---

## 11. Mock 测试系统

### MockAgent
- 替代真实 Agent，拦截所有 HTTP 请求
- 支持 `get()` / `disableNetConnect()` / `enableNetConnect()`

### MockInterceptor
- 按 URL/Method/Header 匹配
- 支持 `.reply()` / `.replyWithError()` / `.persist()`
- 调用历史记录（MockCallHistory）

### Snapshot 快照系统
- `SnapshotRecorder`：录制真实 HTTP 交互
- `SnapshotAgent`：回放录制的交互
- 支持序列化到文件、离线测试

---

## 12. 代理支持

| 代理类型 | 实现 | 说明 |
|----------|------|------|
| HTTP CONNECT | ProxyAgent | 通过 CONNECT 方法建立隧道 |
| 环境变量 | EnvHttpProxyAgent | 读取 HTTP_PROXY/HTTPS_PROXY/NO_PROXY |
| SOCKS5 | Socks5ProxyAgent | SOCKS5 协议代理 |

---

## 13. 错误处理与诊断

### 错误体系
- `lib/core/errors.js`：统一错误类 UndiciError
- 错误码：UND_ERR_*
- 每个错误包含 `code`、`message`、`cause`

### 诊断钩子
- `diagnostics_channel` 全覆盖
- 钩子点：client.send / client.headers / client.trailers / client.bodyComplete / client.error
- 零开销：未订阅时不执行

---

## 14. 对 laew 的借鉴价值

### P0（立即可借鉴）

1. **Interceptor 链式模式**：laew 的工具调用可借鉴拦截器模式，在请求前后注入缓存/重试/日志
2. **固定队列（FixedQueue）**：laew 的任务队列可用 O(1) 入队/出队替代 Vec
3. **错误码体系**：统一错误码 + cause 链，便于调试和错误分类
4. **诊断钩子**：零开销的 diagnostics_channel，可移植到 laew 的遥测系统

### P1（中期借鉴）

5. **连接池管理**：laew 的 LLM 连接池可借鉴 Pool 的按需创建 + 空闲超时关闭
6. **Mock 测试系统**：laew 的 e2e 测试可借鉴 MockAgent 的录制/回放模式
7. **负载均衡**：BalancedPool 的加权策略可用于多 LLM endpoint 调度
8. **WASM 解析器**：llhttp 的 WASM + SIMD 模式，laew 可考虑用于 HTTP 响应解析

### P2（长期参考）

9. **Web 标准对齐**：fetch/WebSocket/EventSource 的规范严格对齐方式
10. **双协议支持**：HTTP/1.1 + HTTP/2 的 ALPN 协商和降级策略

---

## 15. 详细分析文档索引

| 文档 | 行数 | 内容 |
|------|------|------|
| [undici-核心架构深度分析.md](undici-核心架构深度分析.md) | 4,127 | Dispatcher 体系、双协议、连接池、llhttp、API 层、错误处理 |
| [undici-WebAPI层深度分析.md](undici-WebAPI层深度分析.md) | 2,018 | Fetch/WebSocket/EventSource/Cache/Cookies/WebIDL |
| [undici-拦截器与Mock系统深度分析.md](undici-拦截器与Mock系统深度分析.md) | 2,252 | 8 个拦截器、Handler 装饰器、Mock 系统、Snapshot 快照 |

**总计：8,397 行深度分析文档**

---

## 跨项目关联

undici 作为 Node.js 官方 HTTP 客户端，是以下 AI Agent 的底层 HTTP 传输层：

- **claudecode**：TypeScript/Bun，使用 undici 的 fetch API 与 Anthropic API 通信
- **opencode**：TypeScript/Bun，Effect 框架下的 HTTP 客户端层
- **pi**：TypeScript，lane 并发模型中的 HTTP 请求
- **deepseek-harness**：TypeScript，Cordis 插件系统的网络层

**关键洞察**：undici 的 Interceptor 链式模式与 Agent 的工具调用链有相似的组合模式，可互相借鉴。
