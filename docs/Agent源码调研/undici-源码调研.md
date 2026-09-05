# undici 源码调研

> 项目：undici（https://github.com/nodejs/undici）
> 定位：Node.js 官方 HTTP/1.1 + HTTP/2 客户端库，Node.js 内置 `fetch()` 的底层实现
> 语言：JavaScript（Node.js），零运行时依赖
> 版本：v8.10.1（2026-09 当前最新主版本）
> 源码规模：109 个 JS 文件，~22,161 行（lib/）+ 测试 488 文件 + 基准测试 16 文件
> 许可证：MIT（Node.js Working Group 治理）
> 分析日期：2026-09-05

---

## 目录

1. [项目概览](#1-项目概览)
2. [项目元信息与依赖树](#2-项目元信息与依赖树)
3. [架构总图](#3-架构总图)
4. [完整目录结构解读](#4-完整目录结构解读)
5. [lib 109 个文件分类清单](#5-lib-109-个文件分类清单)
6. [顶层导出清单（双入口设计）](#6-顶层导出清单双入口设计)
7. [Dispatcher 体系](#7-dispatcher-体系)
8. [HTTP/1.1 与 HTTP/2 双协议](#8-http11-与-http2-双协议)
9. [连接池与负载均衡](#9-连接池与负载均衡)
10. [llhttp WASM 解析器与 deps/ 源码](#10-llhttp-wasm-解析器与-deps-源码)
11. [5 种 API 风格](#11-5-种-api-风格)
12. [Web API 层（7 个子目录详解）](#12-web-api-层7-个子目录详解)
13. [Interceptor 拦截器体系与 Handler 装饰器](#13-interceptor-拦截器体系与-handler-装饰器)
14. [Mock 测试系统](#14-mock-测试系统)
15. [代理支持（3 种代理 + 环境变量）](#15-代理支持3-种代理--环境变量)
16. [错误处理、诊断钩子与可观测性](#16-错误处理诊断钩子与可观测性)
17. [TypeScript 类型系统（types/ 双发布）](#17-typescript-类型系统types-双发布)
18. [构建、脚本与测试体系](#18-构建脚本与测试体系)
19. [CI 流水线（.github/workflows/ 13 个流程）](#19-cipipelinegithubworkflows-13-个流程)
20. [官方文档体系（docs/）](#20-官方文档体系docs)
21. [版本、治理与协作](#21-版本治理与协作)
22. [与 Node.js 内置 http/fetch 模块的关系](#22-与-nodejs-内置-httpfetch-模块的关系)
23. [对 laew 的借鉴价值](#23-对-laew-的借鉴价值)
24. [详细分析文档索引](#24-详细分析文档索引)

---

## 1. 项目概览

Undici 是 Node.js 官方维护的 HTTP 客户端库，名字取自意大利语"11"（1.1 → 11 → Undici），也是 Stranger Things 的彩蛋。它是 Node.js 内置 `fetch()` API 的底层实现，性能远超 `axios`、`got`、`node-fetch` 等流行库。

**核心特性**：
- 原生 HTTP/1.1 流水线（pipelining）与 HTTP/2 多路复用（multipelxing）
- 基于 llhttp 的 WASM 解析器（非 Node.js 内置 http_parser）
- 连接池管理与负载均衡（Pool、BalancedPool、RoundRobinPool）
- 5 种 API 风格：request / stream / pipeline / connect / upgrade
- 完整的 Web 标准 API：fetch / WebSocket / EventSource / Cache / Cookies
- 8 个可组合拦截器：cache / retry / redirect / dns / decompress / deduplicate / dump / response-error
- 代理支持：HTTP CONNECT、SOCKS5、环境变量自动代理
- 诊断钩子（diagnostics_channel）全覆盖
- Mock 测试系统：录制/回放/快照
- 全局 dispatcher（`Symbol.for('undici.globalDispatcher.2')`）+ `install()` 注入全局 Web API

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

## 2. 项目元信息与依赖树

### 2.1 package.json 关键字段

```json
{
  "name": "undici",
  "version": "8.10.1",
  "description": "An HTTP/1.1 client, written from scratch for Node.js",
  "main": "index.js",
  "types": "index.d.ts",
  "engines": { "node": ">=22.19.0" },
  "license": "MIT"
}
```

> **设计要点**：**dependencies 为空**——运行时零依赖，所有功能自包含。TypeScript 类型**随主仓发布**（`types/index.d.ts`），同时双发布为独立包 `undici-types`（供 `@types/node` 等仅需要类型的下游）。

### 2.2 全部作者（package.json contributors）

| 作者 | GitHub | 备注 |
|------|--------|------|
| Daniele Belardi | @dnlup | 创始人 |
| Ethan Arrowood | @ethan-arrowood | 早期核心 |
| Matteo Collina | @mcollina | Node.js TSC、fastify 作者 |
| Matthew Aitken | @KhafraDev | 活跃维护者 |
| Robert Nagy | @ronag | 核心贡献者 |
| Szymon Marczak | @szmarczak | node-fetch 作者 |
| Tomas Della Vedova | @delvedor | 早期核心 |

### 2.3 engines 要求

```json
"engines": { "node": ">=22.19.0" }
```

对应 Node.js 版本矩阵：CI 测试 **22 / 24 / 25 / 26**（覆盖 LTS、Current、Nightly），并要求 OpenSSL-less / Intl-less / no-SIMD 等多种极限构建。

### 2.4 完整依赖树

**dependencies：无**（运行时零依赖）

**devDependencies（15 个）**——全部用于开发期：

| 分类 | 包 | 版本 | 用途 |
|------|-----|------|------|
| 测试框架 | `borp` | ^1.0.0 | 主要测试框架（fastify 团队出品，并行+报告） |
| 测试框架 | `jest` | ^30.0.5 | 次要测试（test/jest/） |
| 类型测试 | `tsd` | ^0.33.0 | 编译期 .d.ts 断言测试 |
| 覆盖率 | `c8` | ^12.0.0 | V8 覆盖率报告 |
| 多进程 | `@sinonjs/fake-timers` | ^12.0.0 | 计时器 mock |
| 多进程 | `proxy` | ^4.0.0 | 代理测试服务器 |
| 多进程 | `dns-packet` | ^5.4.0 | DNS 缓存测试 |
| 多进程 | `@fastify/busboy` | 3.2.2 | multipart 解析 |
| 多进程 | `ws` | ^8.11.0 | WebSocket 测试客户端 |
| 多进程 | `node-forge` | ^1.3.1 | PEM/证书生成 |
| 多进程 | `@metcoder95/https-pem` | ^1.0.0 | HTTPS PEM 生成 |
| Lint | `eslint` | ^9.9.0 | Lint（neostandard 规则集） |
| Lint | `neostandard` | ^0.13.0 | 标准规则集 |
| TS | `typescript` | ^6.0.2 | 类型检查 |
| 构建 | `esbuild` | ^0.28.0 | 打包 undici-fetch.js |
| Git Hooks | `husky` | ^9.0.7 | pre-commit 钩子 |
| 属性测试 | `fast-check` | ^4.1.1 | 模糊测试属性 |
| 平台 | `cross-env` | ^10.0.0 | 跨平台 env |
| 差分 | `jsondiffpatch` | ^0.7.3 | 快照差分 |

### 2.5 scripts 命令体系（package.json 全部 34 条）

**按用途分组**：

| 用途 | 命令 | 说明 |
|------|------|------|
| **构建** | `build:node` | esbuild 打包 `undici-fetch.js`（用于 Node.js 核心内嵌） |
| **构建** | `build:wasm` | `node build/wasm.js --docker`，Docker 容器内编译 llhttp → WASM |
| **构建** | `generate-pem` | 生成测试证书 |
| **Lint** | `lint` / `lint:fix` | eslint --cache |
| **测试总入口** | `test` | `test:javascript` + `test:typescript` |
| **测试总入口** | `test:javascript` | 所有 JS 测试（no-jest 子集 + jest 子集） |
| **单元测试** | `test:unit` | `test/*.js`（根目录单测） |
| **单元测试** | `test:node-test` | `test/node-test/**/*.js`（Node.js 核心兼容测试） |
| **Fetch** | `test:fetch` | 需先 `build:node`，运行 test/fetch/ |
| **Cache** | `test:cache` / `test:cache-interceptor` | 含 SQLite 变体 |
| **H2** | `test:h2:core` / `test:h2:fetch` | HTTP/2 测试 |
| **WebSocket** | `test:websocket` / `test:websocket:autobahn` | Autobahn 测试套件 |
| **WPT** | `test:wpt` | Web-Platform-Tests（fetch/mimesniff/xhr/websockets/eventsource） |
| **类型测试** | `test:typescript` | tsd + tsc 校验 |
| **模糊测试** | `test:fuzzing` | 属性测试 |
| **覆盖率** | `coverage` | NODE_V8_COVERAGE + c8 |
| **发布** | `prepare` | husky 初始化 + `platform-shell.js` |
| **基准** | `bench` | 已迁移到 benchmarks/，此处仅报错提示 |
| **文档** | `serve:website` | 已迁移到 docs/，此处仅报错提示 |

> 设计亮点：`test:javascript:without-intl` / `test:javascript:no-jest` 等子集命令，供 CI 分阶段跑；`bench` 和 `serve:website` 已迁移到独立目录但保留为"报错提示"，避免老命令误用。

### 2.6 版本历史位置

undici 仓库**没有 CHANGELOG.md**，版本历史通过以下方式追踪：
- Git tags：`v8.10.1`、`v8.10.0`、`v8.9.0`...`v1.0.0`
- GitHub Releases：https://github.com/nodejs/undici/releases
- `scripts/release.js`：自动化 release PR 生成脚本
- Node.js 内嵌版本：`process.versions.undici`（如 `"5.28.4"`）

---

## 3. 架构总图

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

## 4. 完整目录结构解读

```
undici/
├── lib/                          # 核心实现（109 个 JS 文件, ~22k 行）
│   ├── api/                      # 8 文件：5 种 API 风格（request/stream/pipeline/connect/upgrade）
│   ├── cache/                    # 2 文件：HTTP 缓存后端（Memory/SQLite 双存储）
│   ├── core/                     # 9 文件：Request/Connect/Errors/Util/Diagnostics/Symbols/Tree
│   ├── dispatcher/               # 16 文件：Dispatcher 体系（Client/Pool/Agent/Proxy/Retry 等）
│   ├── encoding/                 # 1 文件：br/gzip/deflate/zstd 编码支持
│   ├── handler/                  # 6 文件：Handler 装饰器（Cache/Retry/Redirect/Dedup/Decorator）
│   ├── interceptor/              # 8 文件：8 个可组合拦截器
│   ├── llhttp/                   # 4 文件：llhttp WASM 解析器（含 WASM 二进制）
│   ├── mock/                     # 12 文件：Mock 测试系统（MockAgent/Recorder/Snapshot）
│   ├── util/                     # 5 文件：工具层（cache/date/stats/timers/runtime-features）
│   ├── global.js                 # 全局 Dispatcher（Symbol.for 注册）
│   └── web/                      # Web 标准 API 层
│       ├── cache/                # 3 文件：Cache/CacheStorage
│       ├── cookies/              # 4 文件：Cookie 解析与序列化
│       ├── eventsource/          # 3 文件：EventSource/SSE
│       ├── fetch/                # 12 文件：Fetch API 完整实现（body/headers/request/response/formdata）
│       ├── infra/                # 1 文件：WHATWG Infra 规范工具
│       ├── subresource-integrity/：# 1 文件：SRI 校验
│       ├── webidl/               # 1 文件：WebIDL 类型系统
│       └── websocket/            # 12 文件：WebSocket RFC 6455（含 stream/ 子目录）
├── test/                         # 488 文件：测试体系
├── docs/                         # 官方文档站（doc-kit 构建）
├── benchmarks/                   # 基准测试（可独立运行）
├── scripts/                      # 7 个工具脚本
├── types/                        # 45 个 .d.ts（独立 undici-types 包）
├── deps/llhttp/                  # llhttp C 源码（内嵌副本）
├── build/                        # WASM 构建脚本
├── index.js                      # 主入口（全量导出）
├── index-fetch.js                # 精简入口（仅 Web API，供 Node.js 核心内嵌）
├── index.d.ts                    # 根类型
├── package.json                  # 包元信息
├── CONTRIBUTING.md               # 贡献指南
├── GOVERNANCE.md                 # Working Group 治理
├── MAINTAINERS.md                # 维护者流程
├── SECURITY.md                   # 安全策略
├── CODE_OF_CONDUCT.md            # 行为准则
└── .github/workflows/            # 13 个 CI 工作流
```

### 各子模块详细职责

| 模块 | 路径 | 文件数 | 职责 |
|------|------|--------|------|
| `api` | `lib/api/` | 8 | 5 种 API 风格入口（通过 `Object.assign(Dispatcher.prototype, api)` 挂载为方法） |
| `cache` | `lib/cache/` | 2 | 缓存后端实现，供 cache interceptor 使用 |
| `core` | `lib/core/` | 9 | 连接、请求、错误、符号、工具、诊断、三叉树、SOCKS5 协议 |
| `dispatcher` | `lib/dispatcher/` | 16 | **核心调度层**：Client/Pool/Agent/Proxy/Retry/Balanced/RoundRobin/H2C |
| `encoding` | `lib/encoding/` | 1 | 编码协商（Accept-Encoding） |
| `handler` | `lib/handler/` | 6 | 装饰器模式：为 dispatcher 包裹缓存/重试/定向/去重逻辑 |
| `interceptor` | `lib/interceptor/` | 8 | 拦截器工厂函数（每个返回 `(dispatch) => (opts, handler) => ...` 签名） |
| `llhttp` | `lib/llhttp/` | 4 | HTTP/1.1 WASM 解析器（含 SIMD 变体） |
| `mock` | `lib/mock/` | 12 | Mock 系统（MockAgent/Interceptor/Client/Pool/Snapshot） |
| `util` | `lib/util/` | 5 | 缓存键、日期、计时器、统计、运行时特性探测 |
| `web/cache` | `lib/web/cache/` | 3 | Cache/CacheStorage Web API |
| `web/cookies` | `lib/web/cookies/` | 4 | Cookie 解析与序列化 |
| `web/eventsource` | `lib/web/eventsource/` | 3 | SSE 流式解析 |
| `web/fetch` | `lib/web/fetch/` | 12 | Fetch API 完整实现 |
| `web/infra` | `lib/web/infra/` | 1 | WHATWG Infra 工具 |
| `web/subresource-integrity` | `lib/web/subresource-integrity/` | 1 | SRI 校验 |
| `web/webidl` | `lib/web/webidl/` | 1 | WebIDL 类型转换 |
| `web/websocket` | `lib/web/websocket/` | 12 + 2(stream) | WebSocket 协议实现 |

---

## 5. lib 109 个文件分类清单

### 5.1 lib/api/（8 文件，~2,600 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `index.js` | ~15 | 导出 5 个 API 函数 |
| `api-request.js` | ~180 | `request()` 实现：Promise 包装，返回 `{statusCode, headers, body}` |
| `api-stream.js` | ~270 | `stream()` 实现：返回 Readable 流 |
| `api-pipeline.js` | ~265 | `pipeline()` 实现：流式管道 |
| `api-connect.js` | ~130 | `connect()` 实现：TCP 隧道 |
| `api-upgrade.js` | ~130 | `upgrade()` 实现：协议升级 |
| `abort-signal.js` | ~60 | AbortSignal 封装 |
| `readable.js` | ~616 | 可读流工具 |

### 5.2 lib/cache/（2 文件，~750 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `memory-cache-store.js` | ~279 | 内存缓存后端（LRU） |
| `sqlite-cache-store.js` | ~469 | SQLite 缓存后端（Node 24+ `--experimental-sqlite`） |

### 5.3 lib/core/（9 文件，~5,360 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `request.js` | ~546 | Request 对象（HTTP 请求构建与序列化） |
| `connect.js` | ~320 | TCP/TLS 连接构建（`buildConnector` 导出） |
| `errors.js` | ~497 | 统一错误类体系（`UND_ERR_*` 码） |
| `util.js` | ~1,049 | 工具函数（parseHeaders/parseOrigin/parseURL 等） |
| `diagnostics.js` | ~200 | diagnostics_channel 封装 |
| `constants.js` | ~280 | 常量定义 |
| `symbols.js` | ~120 | Symbol 常量池（kUrl/kDisconnected 等） |
| `tree.js` | ~350 | TernarySearchTree（三叉搜索树，头部查找） |
| `socks5-client.js` | ~422 | SOCKS5 客户端实现 |
| `socks5-utils.js` | ~200 | SOCKS5 工具函数 |

### 5.4 lib/dispatcher/（16 文件，~5,800 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `dispatcher.js` | ~80 | 抽象基类（`dispatch/close/destroy/compose`） |
| `dispatcher-base.js` | ~200 | 状态管理基类（onConnect/onHeaders/onData/onComplete 回调） |
| `dispatcher1-wrapper.js` | ~150 | v1 API 兼容包装 |
| `client.js` | ~741 | 单连接 Client（协议无关核心） |
| `client-h1.js` | ~1,801 | **HTTP/1.1 实现**（最大单文件，流水线 + keep-alive） |
| `client-h2.js` | ~1,781 | **HTTP/2 实现**（多路复用 + HPACK + WINDOW_UPDATE） |
| `pool-base.js` | ~200 | 池基类 |
| `pool.js` | ~300 | Pool（多连接池） |
| `agent.js` | ~350 | Agent（多 origin 路由） |
| `balanced-pool.js` | ~200 | 加权负载均衡池 |
| `round-robin-pool.js` | ~150 | 轮询负载均衡池 |
| `proxy-agent.js` | ~378 | HTTP CONNECT 代理 |
| `socks5-proxy-agent.js` | ~282 | SOCKS5 代理 |
| `env-http-proxy-agent.js` | ~180 | 环境变量代理（HTTP_PROXY/NO_PROXY） |
| `retry-agent.js` | ~150 | 重试包装 |
| `h2c-client.js` | ~120 | HTTP/2 Cleartext（h2c，无 TLS） |
| `fixed-queue.js` | ~100 | 高性能固定队列（2048 桶循环缓冲） |

### 5.5 lib/handler/（6 文件，~2,450 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `decorator-handler.js` | ~100 | 装饰器基类 |
| `cache-handler.js` | ~802 | 缓存 Handler（含 ETag/Vary/max-age 语义） |
| `cache-revalidation-handler.js` | ~150 | 缓存再验证 Handler |
| `redirect-handler.js` | ~250 | 重定向 Handler（Location/Loop 检测） |
| `deduplication-handler.js` | ~466 | 请求去重 Handler |
| `retry-handler.js` | ~548 | 重试 Handler（指数退避） |

### 5.6 lib/interceptor/（8 文件，~2,550 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `cache.js` | ~618 | 缓存拦截器 |
| `retry.js` | ~100 | 重试拦截器（薄包装，实现在 handler） |
| `redirect.js` | ~120 | 定向拦截器（薄包装） |
| `dns.js` | ~575 | DNS 缓存拦截器 |
| `decompress.js` | ~292 | 自动解压拦截器（gzip/deflate/br/zstd） |
| `deduplicate.js` | ~180 | 去重拦截器 |
| `dump.js` | ~200 | 调试 dump 拦截器 |
| `response-error.js` | ~180 | 响应错误转换拦截器 |

### 5.7 lib/llhttp/（4 文件，~1,000+ 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `constants.js` | ~531 | HTTP 常量定义 |
| `utils.js` | ~120 | llhttp WASM 包装工具 |
| `llhttp-wasm.js` | ~400 | 标准 WASM 解析器（JS 包装） |
| `llhttp_simd-wasm.js` | ~400 | SIMD 优化 WASM 解析器 |

### 5.8 lib/mock/（12 文件，~2,800 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `mock-agent.js` | ~244 | MockAgent（替代真实 Agent） |
| `mock-client.js` | ~180 | MockClient |
| `mock-pool.js` | ~150 | MockPool |
| `mock-interceptor.js` | ~200 | MockInterceptor（URL/Method 匹配） |
| `mock-call-history.js` | ~248 | 调用历史记录 |
| `mock-errors.js` | ~100 | Mock 专用错误 |
| `mock-symbols.js` | ~50 | Mock Symbol 常量 |
| `mock-utils.js` | ~720 | Mock 工具函数 |
| `pending-interceptors-formatter.js` | ~100 | 未匹配拦截器格式化 |
| `snapshot-agent.js` | ~371 | 快照回放 Agent |
| `snapshot-recorder.js` | ~623 | 快照录制器 |
| `snapshot-utils.js` | ~150 | 快照工具 |

### 5.9 lib/util/（5 文件，~3,100 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `cache.js` | ~716 | 缓存工具（makeCacheKey/normalizeHeaders） |
| `date.js` | ~670 | HTTP 日期解析（RFC 7231） |
| `stats.js` | ~200 | 统计工具 |
| `timers.js` | ~425 | 高精度计时器（FastTimer，500ms 精度优化） |
| `runtime-features.js` | ~200 | 运行时特性探测（crypto/sqlite 可用性） |

### 5.10 lib/web/fetch/（12 文件，~7,860 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `index.js` | ~1,200 | `fetch()` 入口 |
| `request.js` | ~900 | Request 类（WHATWG Fetch 规范） |
| `response.js` | ~800 | Response 类 |
| `headers.js` | ~700 | Headers 类（含 Guard 语义） |
| `body.js` | ~900 | Body mixin（Blob/ArrayBuffer/FormData/Stream） |
| `formdata.js` | ~600 | FormData 类 |
| `formdata-parser.js` | ~400 | multipart/form-data 解析 |
| `data-url.js` | ~300 | Data URL 解析 |
| `global.js` | ~100 | Global Origin 管理 |
| `constants.js` | ~200 | Fetch 常量 |
| `util.js` | ~760 | Fetch 工具函数 |
| `LICENSE` | - | llhttp 许可证 |

### 5.11 lib/web/websocket/（12 文件 + stream/ 子目录 2 文件，~2,000+ 行）

| 文件 | 行数 | 职责 |
|------|------|------|
| `websocket.js` | ~500 | WebSocket 入口 |
| `connection.js` | ~300 | 连接管理 |
| `events.js` | ~200 | CloseEvent/ErrorEvent/MessageEvent |
| `frame.js` | ~250 | 帧编解码 |
| `receiver.js` | ~200 | 接收器 |
| `sender.js` | ~200 | 发送器 |
| `permessage-deflate.js` | ~250 | 压缩扩展 |
| `constants.js` | ~100 | WebSocket 常量 |
| `util.js` | ~150 | WebSocket 工具 |
| `stream/websocketstream.js` | ~300 | WebSocketStream（Promise 包装） |
| `stream/websocketerror.js` | ~50 | WebSocketError |

### 5.12 其他 web 子目录

| 目录 | 文件 | 职责 |
|------|------|------|
| `web/cache/` | cache.js/cachestorage.js/util.js | Cache/CacheStorage |
| `web/cookies/` | constants.js/index.js/parse.js/util.js | Cookie 解析 |
| `web/eventsource/` | eventsource.js/eventsource-stream.js/util.js | SSE 实现 |
| `web/infra/` | index.js | WHATWG Infra 工具 |
| `web/subresource-integrity/` | subresource-integrity.js | SRI 校验 |
| `web/webidl/` | index.js | WebIDL 类型系统 |

### 5.13 根目录单文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `global.js` | ~100 | 全局 Dispatcher 单例（`Symbol.for('undici.globalDispatcher.2')`） |
| `encoding/index.js` | ~80 | 编码协商 |

---

## 6. 顶层导出清单（双入口设计）

### 6.1 双入口策略

undici 提供**两个顶层入口**，针对不同使用场景：

| 入口 | 文件 | 用途 | 体积 |
|------|------|------|------|
| 主入口 | `index.js`（8,594 行） | 全量 API（Node 独立模块用户使用） | 大 |
| 精简入口 | `index-fetch.js`（2,480 行） | 仅 Web API（**Node.js 核心内嵌**使用） | 小 |

### 6.2 index.js 完整导出清单

**Dispatcher 体系（直接导出）**：

```javascript
module.exports.Dispatcher = Dispatcher
module.exports.Client = Client
module.exports.Pool = Pool
module.exports.BalancedPool = BalancedPool
module.exports.RoundRobinPool = RoundRobinPool
module.exports.Agent = Agent
module.exports.Dispatcher1Wrapper = Dispatcher1Wrapper   // v1 API 兼容
module.exports.ProxyAgent = ProxyAgent
module.exports.Socks5ProxyAgent = Socks5ProxyAgent
module.exports.EnvHttpProxyAgent = EnvHttpProxyAgent
module.exports.RetryAgent = RetryAgent
module.exports.H2CClient = H2CClient                     // HTTP/2 Cleartext
```

**Handler 装饰器**：

```javascript
module.exports.RetryHandler = RetryHandler
module.exports.DecoratorHandler = DecoratorHandler
module.exports.RedirectHandler = RedirectHandler
```

**8 个拦截器（interceptors 命名空间）**：

```javascript
module.exports.interceptors = {
  redirect, responseError, retry, dump,
  dns, cache, decompress, deduplicate
}
```

**缓存后端（cacheStores 命名空间）**：

```javascript
module.exports.cacheStores = {
  MemoryCacheStore,
  SqliteCacheStore        // Node 24+ experimental-sqlite
}
```

**5 种 API 风格（自动路由到 globalDispatcher）**：

```javascript
module.exports.request = makeDispatcher(api.request)
module.exports.stream = makeDispatcher(api.stream)
module.exports.pipeline = makeDispatcher(api.pipeline)
module.exports.connect = makeDispatcher(api.connect)
module.exports.upgrade = makeDispatcher(api.upgrade)
```

> `makeDispatcher(fn)` 是关键包装器：解析 URL、分离 `agent`/`dispatcher` 选项、默认 method 推断（有 body → PUT，否则 GET）、路径补全。

**Web API（WHATWG 标准）**：

```javascript
module.exports.fetch = function fetch(init, options)  // 带 stack trace 增强
module.exports.Headers = Headers
module.exports.Response = Response
module.exports.Request = Request
module.exports.FormData = FormData
module.exports.WebSocket = WebSocket
module.exports.CloseEvent = CloseEvent
module.exports.ErrorEvent = ErrorEvent
module.exports.MessageEvent = MessageEvent
module.exports.EventSource = EventSource
module.exports.WebSocketStream = WebSocketStream
module.exports.WebSocketError = WebSocketError
module.exports.caches = new CacheStorage(kConstruct)  // 全局 Cache 单例
```

**Cookie API**：

```javascript
module.exports.deleteCookie = deleteCookie
module.exports.getCookies = getCookies
module.exports.getSetCookies = getSetCookies
module.exports.setCookie = setCookie
module.exports.parseCookie = parseCookie
```

**MIME 工具**：

```javascript
module.exports.parseMIMEType = parseMIMEType
module.exports.serializeAMimeType = serializeAMimeType
```

**全局管理**：

```javascript
module.exports.setGlobalDispatcher = setGlobalDispatcher
module.exports.getGlobalDispatcher = getGlobalDispatcher
module.exports.setGlobalOrigin = setGlobalOrigin
module.exports.getGlobalOrigin = getGlobalOrigin
module.exports.install = install  // 注入所有 Web API 到 globalThis
```

**Mock 测试**：

```javascript
module.exports.MockClient = MockClient
module.exports.MockAgent = MockAgent
module.exports.MockPool = MockPool
module.exports.MockCallHistory = MockCallHistory
module.exports.MockCallHistoryLog = MockCallHistoryLog
module.exports.SnapshotAgent = SnapshotAgent
module.exports.mockErrors = mockErrors
```

**底层工具**：

```javascript
module.exports.buildConnector = buildConnector   // 连接构建器
module.exports.errors = errors                     // 错误类全集
module.exports.util = { parseHeaders, headerNameToString }
module.exports.ping = ping                         // WebSocket ping
```

### 6.3 index-fetch.js 导出清单（精简版）

专为 **Node.js 核心内嵌**设计（通过 `libnode.so` 配置 `--shared-builtin-undici/undici-path`）：

```javascript
module.exports.fetch
module.exports.FormData, Headers, Response, Request
module.exports.WebSocket, CloseEvent, ErrorEvent, MessageEvent, createFastMessageEvent
module.exports.EventSource
module.exports.EnvHttpProxyAgent
module.exports.getGlobalDispatcher, setGlobalDispatcher
// 注意：不导出 Client/Pool/Agent/Mock 等底层 API
```

### 6.4 关键设计点

**1. 挂载机制**：`Object.assign(Dispatcher.prototype, api)` 让 `request/stream/pipeline/connect/upgrade` 作为 Dispatcher 实例方法存在。因此 `client.request(...)` 和 `undici.request(...)` 等价（后者走 globalDispatcher）。

**2. Global Dispatcher**：通过 `Symbol.for('undici.globalDispatcher.2')` 注册到**全局符号表**（跨 Realm/VM 上下文共享），`lib/global.js` 启动时默认创建 `new Agent()`。

**3. `install()` 方法**：把 undici 实现的 fetch/WebSocket/Headers 等注入 `globalThis`，对齐浏览器 API。

**4. `appendFetchStackTrace`**：undici 的 fetch 在错误栈中**自动追加 fetch 调用帧**（通过 `Error.captureStackTrace`），解决 bundled 场景下 `__filename` 不可用时堆栈丢失问题。

---

## 7. Dispatcher 体系

Dispatcher 是 undici 的核心抽象，采用 EventEmitter 模式，所有 HTTP 操作通过 `dispatch(opts, handler)` 方法执行。

### 7.1 类层次

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

### 7.2 Dispatcher 基类（dispatcher.js）

```javascript
class Dispatcher extends EventEmitter {
  dispatch () { throw new Error('not implemented') }
  close () { throw new Error('not implemented') }
  destroy () { throw new Error('not implemented') }

  compose (...args) {
    // 支持 [interceptor1, interceptor2] 或 interceptor1, interceptor2, ...
    const interceptors = Array.isArray(args[0]) ? args[0] : args
    let dispatch = this.dispatch.bind(this)
    for (const interceptor of interceptors) {
      if (interceptor == null) continue
      if (typeof interceptor !== 'function') throw new TypeError(...)
      dispatch = interceptor(dispatch)   // 层层包裹
    }
    return dispatch
  }
}
```

> **compose 是拦截器链的核心**：返回新的 dispatch 函数，每层拦截器包裹内层。

### 7.3 关键设计

- **dispatch(opts, handler)**：核心方法，opts 包含 origin/method/path/headers/body，handler 是回调接口
- **状态机**：CLOSED → CONNECTING → CONNECTED → BUSY → IDLE → CLOSED
- **FixedQueue**：高性能固定大小队列（2048 桶循环缓冲），用于请求排队
- **TernarySearchTree**：三叉搜索树，用于 HTTP 头部快速查找

---

## 8. HTTP/1.1 与 HTTP/2 双协议

| 特性 | HTTP/1.1 (client-h1.js) | HTTP/2 (client-h2.js) |
|------|-------------------------|----------------------|
| 连接模型 | 单连接 + pipeline | 多路复用（stream） |
| 解析器 | llhttp WASM | Node.js 内置 http2 |
| 代码行数 | 1,801 行 | 1,781 行 |
| Keep-alive | 显式管理 | 内置 |
| 流量控制 | 无 | WINDOW_UPDATE |
| 头部压缩 | 无 | HPACK |
| 协议选择 | ALPN 协商 | ALPN 协商 |
| 流水线深度 | 可配置（默认 1） | N/A |
| HTTP/1.1 升级 h2c | - | h2c-client.js 支持 |

**ALPN 协商**：连接建立时通过 TLS ALPN 扩展自动选择协议版本，支持 h2 → http/1.1 降级。

---

## 9. 连接池与负载均衡

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

### FixedQueue（高性能队列）

```javascript
// 来自 lib/dispatcher/fixed-queue.js（提取自 node/lib/internal/fixed_queue.js）
const kSize = 2048
const kMask = kSize - 1
// 单链表 + 固定大小循环缓冲，O(1) 入队/出队，无 GC 压力
```

---

## 10. llhttp WASM 解析器与 deps/ 源码

### 10.1 来源

llhttp 是 Node.js 官方的 HTTP 解析器 C 库（https://github.com/nodejs/llhttp），前身是 Nginx 的 http_parser。undici 通过 **WebAssembly** 编译 llhttp，避免依赖 Node.js 内置 http_parser，保持独立演进。

### 10.2 deps/ 目录结构

```
deps/llhttp/
├── include/
│   └── llhttp.h              # C 头文件
└── src/
    ├── api.c                 # API 实现
    ├── llhttp.c              # 主解析器实现
    └── http.c                # HTTP 专用辅助
```

### 10.3 lib/llhttp/ 目录（WASM 包装）

| 文件 | 职责 |
|------|------|
| `llhttp-wasm.js` | 标准 WASM 模块（JavaScript 包装） |
| `llhttp_simd-wasm.js` | SIMD 优化 WASM 模块（自动检测 CPU） |
| `utils.js` | 解析器封装工具 |
| `constants.js` | HTTP 方法/状态码常量 |

### 10.4 构建流程

```bash
# 更新 llhttp 源码
npm run build:wasm    # node build/wasm.js --docker（需 Docker）
```

构建脚本 `build/wasm.js` 在 Docker 容器内通过 Emscripten 编译 C → WASM，输出到 `lib/llhttp/`。

### 10.5 解析器回调接口

llhttp 是回调驱动：`on_message_begin` / `on_url` / `on_header_field` / `on_header_value` / `on_body` / `on_message_complete`。JS-WASM 桥通过 `__indirect_function_table` 实现零拷贝回调。

### 10.6 SIMD 自动检测

undici 运行时检测 CPU 是否支持 WASM SIMD 指令集，自动选择 `llhttp_simd-wasm.js`（快）或标准版。CI 有 `test-with-no-wasm-simd` 任务确保非 SIMD 兼容性。

---

## 11. 5 种 API 风格

| API | 用途 | 特点 |
|-----|------|------|
| `request()` | 通用请求 | 返回完整 Response，最易用 |
| `stream()` | 流式响应 | 返回 Readable，适合大文件 |
| `pipeline()` | 流式管道 | pipe 链式处理 |
| `connect()` | TCP 隧道 | 代理/隧道场景 |
| `upgrade()` | 协议升级 | WebSocket 等 |

> 性能梯度：request < stream < pipeline < dispatch（越底层越灵活，req/s 越高）。

---

## 12. Web API 层（7 个子目录详解）

### 12.1 Fetch API（lib/web/fetch/，~7,860 行）

- 严格对齐 WHATWG Fetch 规范，每步注释引用规范编号（如 `[FETCH]` 引用）
- `fetch()` 入口 → Request 构建 → Body 处理 → Headers 序列化 → Response 解析
- 支持 FormData、Data URL、AbortSignal、redirect/follow/error 模式
- 关键文件：index.js（入口）、request.js、response.js、headers.js、body.js、formdata.js

### 12.2 WebSocket（lib/web/websocket/，~2,000+ 行）

- RFC 6455 完整实现：握手 → 帧编解码 → 收发 → 关闭
- 支持 permessage-deflate 压缩扩展
- WebSocketStream API（Promise 包装）
- Autobahn 测试套件覆盖率 100%

### 12.3 EventSource/SSE（lib/web/eventsource/，~580 行）

- SSE 流式解析器：text/event-stream 格式
- 支持自动重连、Last-Event-ID

### 12.4 Cache API（lib/web/cache/，~1,060 行）

- 双后端：MemoryCacheStore（内存）/ SQLiteCacheStore（持久化）
- HTTP 缓存语义：ETag/If-None-Match、Vary、max-age/stale-while-revalidate
- `caches` 全局单例

### 12.5 Cookies（lib/web/cookies/，~420 行）

- Set-Cookie 解析 + Cookie 序列化
- 大小限制、域名验证
- 独立导出：`getCookies/setCookies/deleteCookie/parseCookie`

### 12.6 WebIDL（lib/web/webidl/，~200 行）

- WebIDL 类型转换工具
- 用于 Fetch 参数的类型强制

### 12.7 Subresource Integrity（lib/web/subresource-integrity/）

- SRI 校验（`<script integrity="sha256-...">`）

### 12.8 Infra（lib/web/infra/）

- WHATWG Infra 规范工具（byte sequence/ASCII 操作）

---

## 13. Interceptor 拦截器体系与 Handler 装饰器

### 13.1 拦截器签名

每个拦截器是一个**高阶函数**，返回 `(dispatch) => (opts, handler) => ...` 的签名：

```javascript
module.exports = (opts = {}) => {
  return (dispatch) => {
    return function Intercept (opts, handler) {
      // 前置逻辑
      const result = dispatch(modifiedOpts, wrappedHandler)
      // 后置逻辑
      return result
    }
  }
}
```

### 13.2 8 个拦截器清单

| 拦截器 | 文件 | 行数 | 功能 |
|--------|------|------|------|
| `cache` | `interceptor/cache.js` | 618 | HTTP 缓存（Memory/SQLite 双后端） |
| `retry` | `interceptor/retry.js` | 100 | 自动重试（薄包装，实现在 RetryHandler） |
| `redirect` | `interceptor/redirect.js` | 120 | 自动重定向（薄包装） |
| `dns` | `interceptor/dns.js` | 575 | DNS 缓存（TTL + 刷新） |
| `decompress` | `interceptor/decompress.js` | 292 | 自动解压（gzip/deflate/br/zstd） |
| `deduplicate` | `interceptor/deduplicate.js` | 180 | 请求去重（相同请求合并） |
| `dump` | `interceptor/dump.js` | 200 | 调试 dump（打印请求/响应） |
| `response-error` | `interceptor/response-error.js` | 180 | 响应错误转换（4xx/5xx → Error） |

### 13.3 Handler 装饰器

Handler 是**真正干活**的类，拦截器只是薄包装。6 个 Handler：

| Handler | 文件 | 行数 | 职责 |
|---------|------|------|------|
| `DecoratorHandler` | `handler/decorator-handler.js` | 100 | 装饰器基类 |
| `CacheHandler` | `handler/cache-handler.js` | 802 | 缓存处理（最大单文件 Handler） |
| `CacheRevalidationHandler` | `handler/cache-revalidation-handler.js` | 150 | 缓存再验证 |
| `RedirectHandler` | `handler/redirect-handler.js` | 250 | 重定向处理 |
| `DeduplicationHandler` | `handler/deduplication-handler.js` | 466 | 请求去重 |
| `RetryHandler` | `handler/retry-handler.js` | 548 | 指数退避重试 |

### 13.4 组合方式

```javascript
const client = new Client(url)
  .compose(cache())
  .compose(retry())
  .compose(redirect())
```

> compose 返回新的 dispatch 函数，调用顺序：cache → retry → redirect → dispatch（洋葱模型）。

### 13.5 decompress 细节（支持 4 种编码）

```javascript
const supportedEncodings = {
  gzip: createGunzip,
  'x-gzip': createGunzip,
  br: createBrotliDecompress,
  deflate: createInflate
  // zstd: createZstdDecompress（如可用）
}
```

---

## 14. Mock 测试系统

### 14.1 组件清单

| 组件 | 文件 | 职责 |
|------|------|------|
| `MockAgent` | `mock/mock-agent.js` | 替代真实 Agent，拦截所有 HTTP 请求 |
| `MockClient` | `mock/mock-client.js` | Mock 单连接 |
| `MockPool` | `mock/mock-pool.js` | Mock 连接池 |
| `MockInterceptor` | `mock/mock-interceptor.js` | URL/Method/Header 匹配 |
| `MockCallHistory` | `mock/mock-call-history.js` | 调用历史记录 |
| `SnapshotAgent` | `mock/snapshot-agent.js` | 快照回放 |
| `SnapshotRecorder` | `mock/snapshot-recorder.js` | 快照录制 |
| `mockErrors` | `mock/mock-errors.js` | Mock 专用错误 |

### 14.2 MockInterceptor API

```javascript
const mockAgent = new MockAgent()
mockAgent.disableNetConnect()  // 禁止真实网络

const pool = mockAgent.get('https://example.com')
pool.intercept({ path: '/api', method: 'GET' }).reply(200, { data: 'mocked' })

// 调用历史
console.log(pool.interceptCalls)  // MockCallHistory
```

### 14.3 Snapshot 快照系统

- `SnapshotRecorder`：录制真实 HTTP 交互 → 序列化到 JSON
- `SnapshotAgent`：离线回放录制的交互
- 支持 `cache-tests/`（http-tests 子模块）

---

## 15. 代理支持（3 种代理 + 环境变量）

| 代理类型 | 实现文件 | 说明 |
|----------|----------|------|
| HTTP CONNECT | `dispatcher/proxy-agent.js` | 通过 CONNECT 方法建立隧道 |
| 环境变量 | `dispatcher/env-http-proxy-agent.js` | 读取 HTTP_PROXY/HTTPS_PROXY/NO_PROXY（支持免代理列表） |
| SOCKS5 | `dispatcher/socks5-proxy-agent.js` | SOCKS5 协议代理（含 `lib/core/socks5-client.js` 422 行完整协议） |
| SOCKS5 工具 | `lib/core/socks5-client.js` | SOCKS5 握手/认证/连接 |
| SOCKS5 工具 | `lib/core/socks5-utils.js` | SOCKS5 辅助函数 |

---

## 16. 错误处理、诊断钩子与可观测性

### 16.1 错误体系（lib/core/errors.js，497 行）

- 统一基类：`UndiciError extends Error`
- 错误码：`UND_ERR_*`（枚举常量）
- 每个错误包含 `code`、`message`、`cause`（错误链）
- 关键错误类：`InvalidArgumentError`、`ConnectTimeoutError`、`RequestAbortedError`、`ResponseError`、`InformationalError`

### 16.2 诊断钩子（lib/core/diagnostics.js）

- `diagnostics_channel` 全覆盖
- 钩子点：`client.send` / `client.headers` / `client.trailers` / `client.bodyComplete` / `client.error`
- 零开销：未订阅时 channel 回调不执行

### 16.3 可观测性

- **lib/util/timers.js**（425 行）：`FastTimer` 实现，低分辨率（500ms 精度）优化，适用 1s+ 超时
- **lib/util/runtime-features.js**（200 行）：运行时特性探测（crypto/sqlite 可用性）
- 类型：`diagnostics-channel.d.ts`、`client-stats.d.ts`、`pool-stats.d.ts` 等

---

## 17. TypeScript 类型系统（types/ 双发布）

### 17.1 types/ 目录结构（45 个 .d.ts 文件）

undici 随主仓发布 `.d.ts`（`types/index.d.ts` 入口），同时双发布为独立包 `undici-types`（供 `@types/node` 等仅需要类型的下游）。

**types/README.md 说明**：
> This package is a dual-publish of the undici library types. The `undici` package still contains types. This package is for users who only need undici types (such as for `@types/node`).

### 17.2 45 个类型文件清单

| 分类 | 文件 |
|------|------|
| 核心 | `index.d.ts`、`dispatcher.d.ts`、`handlers.d.ts`、`errors.d.ts`、`util.d.ts` |
| Dispatcher 体系 | `client.d.ts`、`pool.d.ts`、`agent.d.ts`、`balanced-pool.d.ts`、`round-robin-pool.d.ts`、`proxy-agent.d.ts`、`env-http-proxy-agent.d.ts`、`socks5-proxy-agent.d.ts`、`retry-agent.d.ts`、`h2c-client.d.ts`、`dispatcher1-wrapper.d.ts` |
| 统计 | `client-stats.d.ts`、`pool-stats.d.ts` |
| 拦截器 | `interceptors.d.ts`、`cache-interceptor.d.ts` |
| 缓存 | `cache.d.ts` |
| Web API | `fetch.d.ts`、`websocket.d.ts`、`eventsource.d.ts`、`formdata.d.ts`、`cookies.d.ts`、`webidl.d.ts` |
| Mock | `mock-agent.d.ts`、`mock-client.d.ts`、`mock-pool.d.ts`、`mock-interceptor.d.ts`、`mock-call-history.d.ts`、`mock-errors.d.ts`、`snapshot-agent.d.ts` |
| 连接 | `connector.d.ts`、`header.d.ts`、`content-type.d.ts` |
| 全局 | `global-dispatcher.d.ts`、`global-origin.d.ts`、`patch.d.ts`、`utility.d.ts` |
| 诊断 | `diagnostics-channel.d.ts` |
| 其他 | `readable.d.ts`、`retry-handler.d.ts`、`api.d.ts` |

### 17.3 类型测试（tsd）

```bash
npm run test:typescript  # tsd + tsc test/imports/undici-import.ts + tsc types/*.d.ts
```

---

## 18. 构建、脚本与测试体系

### 18.1 scripts/ 工具（7 文件）

| 文件 | 职责 |
|------|------|
| `clean-coverage.js` | 清理覆盖率目录 |
| `find-hanging-tests.sh` | 查找挂起测试 |
| `generate-pem.js` | 生成测试证书（调用 `@metcoder95/https-pem`） |
| `generate-undici-types-package-json.js` | 生成 undici-types 子包的 package.json |
| `platform-shell.js` | 跨平台 shell 适配 |
| `release.js` | Release PR 生成脚本（调用 GitHub API） |
| `strip-comments.js` | esbuild 后处理：移除注释 |

### 18.2 测试体系（test/，488 文件）

**test/ 子目录结构**：

| 目录 | 用途 |
|------|------|
| `test/` | 根目录单元测试 |
| `test/node-test/` | Node.js 核心兼容性测试 |
| `test/fetch/` | Fetch API 测试 |
| `test/cache/` | Cache API 测试 |
| `test/cache-interceptor/` | Cache Interceptor 测试 |
| `test/interceptors/` | 拦截器测试 |
| `test/cookie/` | Cookie 测试 |
| `test/eventsource/` | EventSource 测试 |
| `test/websocket/` | WebSocket 测试 |
| `test/autobahn/` | Autobahn WebSocket 测试套件 |
| `test/mock/` | Mock 系统测试 |
| `test/node-fetch/` | node-fetch 兼容测试 |
| `test/infra/` | WHATWG Infra 测试 |
| `test/webidl/` | WebIDL 测试 |
| `test/subresource-integrity/` | SRI 测试 |
| `test/busboy/` | Busboy 解析测试 |
| `test/imports/` | 导入兼容性测试 |
| `test/types/` | 类型测试（tsd） |
| `test/jest/` | Jest 专用测试 |
| `test/fixtures/` | 测试 fixture |
| `test/fuzzing/` | 模糊测试 |
| `test/utils/` | 测试工具 |
| `test/web-platform-tests/` | Web-Platform-Tests 子模块 |
| `test/web-platform-tests/wpt` | WPT 子模块（.gitmodules） |
| `test/fixtures/cache-tests` | cache-tests 子模块（.gitmodules） |

**子模块**（.gitmodules）：
```
test/web-platform-tests/wpt    → github.com/web-platform-tests/wpt.git
test/fixtures/cache-tests      → github.com/http-tests/cache-tests.git
```

### 18.3 测试框架

| 框架 | 用途 | 配置 |
|------|------|------|
| `borp` | 主要测试框架（并行 + 覆盖率） | `borp --timeout 180000 -p "test/..."` |
| `jest` | 次要测试 | `testMatch: ["<rootDir>/test/jest/**"]` |
| `tsd` | 类型断言测试 | `tsd` |
| `tsc` | 类型编译检查 | `tsc test/imports/...` + `tsc types/*.d.ts` |

---

## 19. CI Pipeline（.github/workflows/ 13 个流程）

### 19.1 工作流清单

| 文件 | 名称 | 触发条件 | 用途 |
|------|------|----------|------|
| `ci.yml` | CI | push/PR | 主流程：lint + test 矩阵 + 特殊构建 |
| `nodejs.yml` | Node.js | 被 ci.yml 调用（workflow_call） | 测试执行单元 |
| `nodejs-shared.yml` | Node.js Shared Build | 手动 | 测试与 Node.js 核心内嵌集成 |
| `nodejs-nightly.yml` | Node.js Nightly | 定时 | Nightly 版本兼容性测试 |
| `autobahn.yml` | Autobahn | PR（websocket 变更）+ dispatch | WebSocket 协议合规测试 |
| `triggered-autobahn.yml` | Triggered Autobahn | 手动触发 | 完整 Autobahn 套件 |
| `bench.yml` | Benchmarks | push/PR | 基准测试（对比 base_ref vs 当前分支） |
| `codeql.yml` | CodeQL | push/PR | 安全分析 |
| `scorecard.yml` | Scorecard | push | OpenSSF Scorecard |
| `release.yml` | Release | 手动 | npm 发布 |
| `release-create-pr.yml` | Create Release PR | 手动 | 自动生成 release PR |
| `backport.yml` | Backport | PR merged + label | 自动 backport |
| `update-submodules.yml` | Update Submodules | 定时 | 更新 WPT/cache-tests 子模块 |

### 19.2 CI 测试矩阵（ci.yml）

| 矩阵维度 | 取值 |
|----------|------|
| node-version | 22 / 24 / 25 / 26 |
| runs-on | ubuntu-latest / windows-latest / macos-latest |
| 特殊构建 | no-wasm-simd / without-intl / without-ssl |

> 关键设计：`max-parallel: 0`（**全部并行**，加速 CI）；`fail-fast: false`（单个失败不取消其他）。

### 19.3 特殊测试场景

1. **no-wasm-simd**：`UNDICI_NO_WASM_SIMD=1` 禁用 SIMD 解析器
2. **without-intl**：从源码编译 `--without-intl` Node.js，测试无 ICU 环境
3. **without-ssl**：从源码编译 `--without-ssl` Node.js，测试无 TLS 环境
4. **without-ssl** 还会 `node index.js` 验证加载（确保无 crypto 也可 require）

### 19.4 安全流程

- `dependency-review`：PR 依赖审查
- `codeql`：GitHub CodeQL 安全分析
- `scorecard`：OpenSSF Scorecard 评分
- `autobahn`：WebSocket 协议 fuzz 测试
- Dependabot 自动合并（amalgamate workflow）

---

## 20. 官方文档体系（docs/）

### 20.1 文档站结构（doc-kit 构建）

undici 文档站 https://undici.nodejs.org 使用 **doc-kit**（Node.js 核心同款工具）构建。

```
docs/
├── README.md                   # 文档构建说明
├── index.md                    # 首页
├── getting-started.md          # 入门指南
├── site.json                   # 侧边栏导航配置
├── type-map.json               # 类型 → 链接映射
├── docs/
│   ├── api/                    # API 参考（32 文件）
│   │   ├── Agent.md
│   │   ├── BalancedPool.md
│   │   ├── CacheStorage.md
│   │   ├── Client.md
│   │   ├── ClientStats.md
│   │   ├── Connector.md
│   │   ├── ContentType.md
│   │   ├── Cookies.md
│   │   ├── Debug.md
│   │   ├── DiagnosticsChannel.md
│   │   ├── Dispatcher.md
│   │   ├── EnvHttpProxyAgent.md
│   │   ├── Errors.md
│   │   ├── EventSource.md
│   │   ├── Fetch.md
│   │   ├── GlobalInstallation.md
│   │   ├── H2CClient.md
│   │   ├── Interceptors.md
│   │   ├── MockAgent.md
│   │   ├── MockCallHistory.md
│   │   ├── MockClient.md
│   │   ├── MockErrors.md
│   │   ├── MockPool.md
│   │   ├── Pool.md
│   │   ├── PoolStats.md
│   │   ├── ProxyAgent.md
│   │   ├── RedirectHandler.md
│   │   ├── RetryAgent.md
│   │   ├── RetryHandler.md
│   │   ├── RoundRobinPool.md
│   │   ├── SnapshotAgent.md
│   │   ├── Socks5ProxyAgent.md
│   │   ├── Util.md
│   │   ├── WebSocket.md
│   │   └── api-lifecycle.md
│   └── best-practices/         # 最佳实践（7 文件）
│       ├── client-certificate.md
│       ├── crawling.md
│       ├── migrating-from-v7-to-v8.md
│       ├── mocking-request.md
│       ├── proxy.md
│       ├── undici-vs-builtin-fetch.md
│       └── writing-tests.md
└── examples/                   # 使用示例
    └── README.md
```

### 20.2 site.json 机制

- 侧边栏**不自动发现**文件，必须手动注册到 `site.json`
- 支持分组（`groupName`）和嵌套子分组（`items` 递归）
- 类型 `{Type}` 标记通过 `type-map.json` 解析为链接

### 20.3 文档站运行

```bash
cd docs && npm i && npm run serve   # http://localhost:3000
```

> 旧命令 `npm run serve:website` 已迁移到 docs/ 目录，主 package.json 保留为"报错提示"。

---

## 21. 版本、治理与协作

### 21.1 GOVERNANCE.md（Working Group 治理）

undici 由 **Undici Working Group (WG)** 治理，关键要点：

- **WG 权限**：技术方向、治理流程、贡献政策、仓库托管、行为准则
- **Collaborator**：重大贡献者获 commit access，WG 讨论后加入
- **共识机制**：Consensus Seeking Decision Making（无异议即共识）
- **WG 会议**：Zoom 举行，YouTube 公开，moderator 总结为 PR
- **1/3 规则**：同一雇主不超过 WG 成员 1/3
- **议程标签**：`WG-agenda` 标记提交到 WG 讨论

### 21.2 MAINTAINERS.md（维护者流程）

- **Labels**：详细标签体系（bug/enhancement/help-wanted + 模块标签 Agent/Client/Pool/...）
- **Release 流程**：GitHub Actions 自动化（Create Release PR → 审批 → Release）
- **Releasers**：特定名单成员可审批 npm 发布

### 21.3 CONTRIBUTING.md（贡献指南）

详细指南包括：
- **Update llhttp**：更新 WASM 解析器的 8 步流程（需 Docker）
- **Lint** / **Test** / **Coverage**：测试命令
- **WPT**：Web-Platform-Tests 子模块更新
- **External Builds**：为 Node.js 核心内嵌编译的指南（`EXTERNAL_PATH` 参数）
- **Benchmarks**：`cd benchmarks && npm i && npm run bench`（http://localhost:3042）
- **Documentation**：`cd docs && npm i && npm run serve`

### 21.4 SECURITY.md

安全策略文档（GitHub Security Policy 标准格式）。

### 21.5 CODE_OF_CONDUCT.md

行为准则（贡献者公约）。

---

## 22. 与 Node.js 内置 http/fetch 模块的关系

### 22.1 背景

Node.js v18+ 内置 `fetch()` 由 undici 的 bundled 版本提供。但 undici 作为独立模块发行，**版本更新频率远高于 Node.js 内置**。

### 22.2 关系图

```
┌─────────────────────────────────────────────────────────────┐
│ Node.js 运行时                                               │
│                                                              │
│  ┌──────────────────┐        ┌──────────────────────────┐   │
│  │ 内置 http 模块    │        │ 内置 fetch (lib/internal/ │   │
│  │ (legacy, C++)    │        │  undici/*.js)             │   │
│  │                  │        │  - 固定版本（如 5.28.4）    │   │
│  │ 不被推荐用于新    │        │  - 无法独立升级            │   │
│  │ 项目              │        │  - 受限于 Node 主版本      │   │
│  └──────────────────┘        └──────────────────────────┘   │
│                                                              │
│   process.versions.undici → "5.28.4"                        │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ npm i undici (独立模块)                                      │
│                                                              │
│  v8.10.1（最新）                                              │
│  - 独立版本发布                                               │
│  - 完整 API（Client/Pool/Agent）                              │
│  - 最新性能优化 + 新特性                                       │
│  - 可 setGlobalDispatcher 影响全局 fetch                       │
│  - install() 覆盖 globalThis.fetch                            │
└─────────────────────────────────────────────────────────────┘
```

### 22.3 选择指南

**使用内置 fetch 当**：
- 零依赖要求
- 同构代码（浏览器 + Node.js）
- 不需要特定版本 undici 特性

**使用 undici 独立模块当**：
- 需要最新特性 / 性能
- 需要 Client/Pool/Agent 底层 API
- 需要拦截器 / Mock / 代理
- 需要精细控制连接池

### 22.4 版本对应关系

| Node.js 版本 | 内置 undici 版本 | 独立 undici 最新版 |
|--------------|------------------|---------------------|
| Node 18 | 5.28.x | 8.10.1 |
| Node 20 | 5.28.x / 6.x | 8.10.1 |
| Node 22 | 6.x | 8.10.1 |
| Node 24 | 6.x / 7.x | 8.10.1 |

> 独立 undici 总是领先内置版本 1-2 个大版本。

### 22.5 全局接管方式

```javascript
// 方式 1：setGlobalDispatcher
const { setGlobalDispatcher, EnvHttpProxyAgent } = require('undici')
setGlobalDispatcher(new EnvHttpProxyAgent())

// 方式 2：install() — 覆盖 globalThis 所有 Web API
const undici = require('undici')
undici.install()
// 此后 globalThis.fetch 使用 undici 实现
```

### 22.6 Node.js 核心集成机制

- **nodejs-shared.yml** CI：测试 undici 作为 `--shared-builtin-undici/undici-path` 与 Node.js 核心的集成
- **index-fetch.js**：专为 Node.js 内嵌设计的精简入口
- **EXTERNAL_PATH**：构建时参数，指向全局 node_modules/undici 路径

---

## 23. 对 laew 的借鉴价值

### P0（立即可借鉴）

1. **Interceptor 链式模式**：laew 的工具调用可借鉴拦截器模式，在请求前后注入缓存/重试/日志
2. **固定队列（FixedQueue）**：laew 的任务队列可用 O(1) 入队/出队替代 Vec
3. **错误码体系**：统一错误码 + cause 链，便于调试和错误分类
4. **诊断钩子**：零开销的 diagnostics_channel，可移植到 laew 的遥测系统
5. **Global Dispatcher + Symbol.for**：laew 的全局 LLM 客户端可借鉴跨 Realm 单例模式

### P1（中期借鉴）

6. **连接池管理**：laew 的 LLM 连接池可借鉴 Pool 的按需创建 + 空闲超时关闭
7. **Mock 测试系统**：laew 的 e2e 测试可借鉴 MockAgent 的录制/回放模式（`SnapshotAgent`）
8. **负载均衡**：BalancedPool 的加权策略可用于多 LLM endpoint 调度
9. **双入口设计**：laew 可参考 index.js / index-fetch.js 分离核心 API 与 Web API 的入口
10. **FastTimer**：laew 的超时管理可借鉴低分辨率计时器优化

### P2（长期参考）

11. **Web 标准对齐**：fetch/WebSocket/EventSource 的规范严格对齐方式
12. **双协议支持**：HTTP/1.1 + HTTP/2 的 ALPN 协商和降级策略
13. **W3C 兼容性测试**：WPT 子模块 + Autobahn 测试套件，laew 的 HTTP 层可参考
14. **undici-types 双发布**：laew 若需要分离类型定义可参考

---

## 24. 详细分析文档索引

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
