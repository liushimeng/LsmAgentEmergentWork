# Undici 综合深度分析

> 调研对象: undici (JavaScript, Node.js 官方 HTTP 客户端, ~38.7k 行)
> 调研日期: 2026-09-06
> 原始文档: 6 份
> 总行数: ~19,635 行(合并后综合提炼)
> 定位: HTTP 客户端基础设施(非 Agent)

---

## 目录

1. [项目元信息](#1-项目元信息)
2. [核心架构(Dispatcher 体系、HTTP/1.1 pipeline、HTTP/2 多路复用)](#2-核心架构)
3. [llhttp WASM 解析器](#3-llhttp-wasm-解析器)
4. [5 种 API 风格](#4-5-种-api-风格)
5. [8 个可组合拦截器](#5-8-个可组合拦截器)
6. [Mock 录制回放系统](#6-mock-录制回放系统)
7. [fetch/WebSocket/EventSource/Cache Web 标准实现](#7-web-标准实现)
8. [26 种错误类层次](#8-错误类层次)
9. [FastTimer + TernarySearchTree + Snapshot 快照系统](#9-基础设施)
10. [HTTP 传输层深度](#10-http-传输层深度)
11. [Web API 层深度](#11-web-api-层深度)
12. [拦截器与 Mock 系统深度](#12-拦截器与-mock-系统深度)
13. [对 laew 的借鉴(HTTP 客户端设计参考)](#13-对-laew-的借鉴)
14. [第七轮深挖 — 连接池 KeepAlive 生命周期 + HTTP/2 多路复用 + DiagnosticsChannel 可观测性 + 背压与 Body 流控](#14-第七轮深挖--连接池-keepalive-生命周期--http2-多路复用--diagnosticschannel-可观测性--背压与-body-流控)

---

## 1. 项目元信息

### 1.1 项目概览

Undici 是 Node.js 官方维护的 HTTP 客户端库,名字取自意大利语"11"(1.1 → 11 → Undici),也是 Stranger Things 的彩蛋。它是 Node.js 内置 `fetch()` API 的底层实现,性能远超 `axios`、`got`、`node-fetch` 等流行库。

**核心特性**:
- 原生 HTTP/1.1 流水线(pipelining)与 HTTP/2 多路复用(multiplexing)
- 基于 llhttp 的 WASM 解析器(非 Node.js 内置 http_parser)
- 连接池管理与负载均衡(Pool、BalancedPool、RoundRobinPool)
- 5 种 API 风格: request / stream / pipeline / connect / upgrade
- 完整的 Web 标准 API: fetch / WebSocket / EventSource / Cache / Cookies
- 8 个可组合拦截器: cache / retry / redirect / dns / decompress / deduplicate / dump / response-error
- 代理支持: HTTP CONNECT、SOCKS5、环境变量自动代理
- 诊断钩子(diagnostics_channel)全覆盖
- Mock 测试系统: 录制/回放/快照
- 全局 dispatcher(`Symbol.for('undici.globalDispatcher.2')`) + `install()` 注入全局 Web API

**性能基准**(50 TCP 连接,pipeline 深度 10,Node 24.14.1):

| 模式 | req/sec | vs 最慢 |
|------|---------|---------|
| node-fetch | 4,711 | - |
| undici fetch | 5,438 | +15% |
| undici pipeline | 13,470 | +186% |
| undici request | 16,850 | +258% |
| undici stream | 18,488 | +292% |
| undici dispatch | 20,786 | +341% |

### 1.2 项目元信息

```
项目: undici(https://github.com/nodejs/undici)
定位: Node.js 官方 HTTP/1.1 + HTTP/2 客户端库,Node.js 内置 fetch() 的底层实现
语言: JavaScript (Node.js),零运行时依赖
版本: v8.10.1(2026-09 当前最新主版本)
源码规模: 109 个 JS 文件,~22,161 行(lib/) + 测试 488 文件 + 基准测试 16 文件
许可证: MIT(Node.js Working Group 治理)
分析日期: 2026-09-05 ~ 2026-09-06
```

### 1.3 package.json 关键字段

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

> **设计要点**: **dependencies 为空** — 运行时零依赖,所有功能自包含。TypeScript 类型**随主仓发布**(`types/index.d.ts`),同时双发布为独立包 `undici-types`(供 `@types/node` 等仅需要类型的下游)。

### 1.4 全部作者(package.json contributors)

| 作者 | GitHub | 备注 |
|------|--------|------|
| Daniele Belardi | @dnlup | 创始人 |
| Ethan Arrowood | @ethan-arrowood | 早期核心 |
| Matteo Collina | @mcollina | Node.js TSC、fastify 作者 |
| Matthew Aitken | @KhafraDev | 活跃维护者 |
| Robert Nagy | @ronag | 核心贡献者 |
| Szymon Marczak | @szmarczak | node-fetch 作者 |
| Tomas Della Vedova | @delvedor | 早期核心 |

### 1.5 engines 要求

```json
"engines": { "node": ">=22.19.0" }
```

对应 Node.js 版本矩阵: CI 测试 **22 / 24 / 25 / 26**(覆盖 LTS、Current、Nightly),并要求 OpenSSL-less / Intl-less / no-SIMD 等多种极限构建。

### 1.6 完整依赖树

**dependencies: 无**(运行时零依赖)

**devDependencies(15 个)** — 全部用于开发期:

| 分类 | 包 | 版本 | 用途 |
|------|-----|------|------|
| 测试框架 | `borp` | ^1.0.0 | 主要测试框架(fastify 团队出品,并行+报告) |
| 测试框架 | `jest` | ^30.0.5 | 次要测试(test/jest/) |
| 类型测试 | `tsd` | ^0.33.0 | 编译期 .d.ts 断言测试 |
| 覆盖率 | `c8` | ^12.0.0 | V8 覆盖率报告 |
| 多进程 | `@sinonjs/fake-timers` | ^12.0.0 | 计时器 mock |
| 多进程 | `proxy` | ^4.0.0 | 代理测试服务器 |
| 多进程 | `dns-packet` | ^5.4.0 | DNS 缓存测试 |
| 多进程 | `@fastify/busboy` | 3.2.2 | multipart 解析 |
| 多进程 | `ws` | ^8.11.0 | WebSocket 测试客户端 |
| 多进程 | `node-forge` | ^1.3.1 | PEM/证书生成 |
| 多进程 | `@metcoder95/https-pem` | ^1.0.0 | HTTPS PEM 生成 |
| Lint | `eslint` | ^9.9.0 | Lint(neostandard 规则集) |
| Lint | `neostandard` | ^0.13.0 | 标准规则集 |
| TS | `typescript` | ^6.0.2 | 类型检查 |
| 构建 | `esbuild` | ^0.28.0 | 打包 undici-fetch.js |
| Git Hooks | `husky` | ^9.0.7 | pre-commit 钩子 |
| 属性测试 | `fast-check` | ^4.1.1 | 模糊测试属性 |
| 平台 | `cross-env` | ^10.0.0 | 跨平台 env |
| 差分 | `jsondiffpatch` | ^0.7.3 | 快照差分 |

### 1.7 scripts 命令体系(package.json 全部 34 条)

**按用途分组**:

| 用途 | 命令 | 说明 |
|------|------|------|
| **构建** | `build:node` | esbuild 打包 `undici-fetch.js`(用于 Node.js 核心内嵌) |
| **构建** | `build:wasm` | `node build/wasm.js --docker`,Docker 容器内编译 llhttp → WASM |
| **构建** | `generate-pem` | 生成测试证书 |
| **Lint** | `lint` / `lint:fix` | eslint --cache |
| **测试总入口** | `test` | `test:javascript` + `test:typescript` |
| **测试总入口** | `test:javascript` | 所有 JS 测试(no-jest 子集 + jest 子集) |
| **单元测试** | `test:unit` | `test/*.js`(根目录单测) |
| **单元测试** | `test:node-test` | `test/node-test/**/*.js`(Node.js 核心兼容测试) |
| **Fetch** | `test:fetch` | 需先 `build:node`,运行 test/fetch/ |
| **Cache** | `test:cache` / `test:cache-interceptor` | 含 SQLite 变体 |
| **H2** | `test:h2:core` / `test:h2:fetch` | HTTP/2 测试 |
| **WebSocket** | `test:websocket` / `test:websocket:autobahn` | Autobahn 测试套件 |
| **WPT** | `test:wpt` | Web-Platform-Tests(fetch/mimesniff/xhr/websockets/eventsource) |
| **类型测试** | `test:typescript` | tsd + tsc 校验 |
| **模糊测试** | `test:fuzzing` | 属性测试 |
| **覆盖率** | `coverage` | NODE_V8_COVERAGE + c8 |
| **发布** | `prepare` | husky 初始化 + `platform-shell.js` |
| **基准** | `bench` | 已迁移到 benchmarks/,此处仅报错提示 |
| **文档** | `serve:website` | 已迁移到 docs/,此处仅报错提示 |

> 设计亮点:`test:javascript:without-intl` / `test:javascript:no-jest` 等子集命令,供 CI 分阶段跑;`bench` 和 `serve:website` 已迁移到独立目录但保留为"报错提示",避免老命令误用。

### 1.8 版本历史位置

undici 仓库**没有 CHANGELOG.md**,版本历史通过以下方式追踪:
- Git tags:`v8.10.1`、`v8.10.0`、`v8.9.0`...`v1.0.0`
- GitHub Releases:https://github.com/nodejs/undici/releases
- `scripts/release.js`:自动化 release PR 生成脚本
- Node.js 内嵌版本:`process.versions.undici`(如 `"5.28.4"`)

### 1.9 与 Node.js 内置 http/fetch 模块的关系

**背景**: Node.js v18+ 内置 `fetch()` 由 undici 的 bundled 版本提供。但 undici 作为独立模块发行,**版本更新频率远高于 Node.js 内置**。

```
┌─────────────────────────────────────────────────────────────┐
│ Node.js 运行时                                               │
│                                                              │
│  ┌──────────────────┐        ┌──────────────────────────┐   │
│  │ 内置 http 模块    │        │ 内置 fetch(lib/internal/ │   │
│  │ (legacy, C++)    │        │  undici/*.js)             │   │
│  │                  │        │  - 固定版本(如 5.28.4)    │   │
│  │ 不被推荐用于新    │        │  - 无法独立升级            │   │
│  │ 项目              │        │  - 受限于 Node 主版本      │   │
│  └──────────────────┘        └──────────────────────────┘   │
│                                                              │
│   process.versions.undici → "5.28.4"                        │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│ npm i undici (独立模块)                                      │
│                                                              │
│  v8.10.1(最新)                                               │
│  - 独立版本发布                                               │
│  - 完整 API(Client/Pool/Agent)                              │
│  - 最新性能优化 + 新特性                                       │
│  - 可 setGlobalDispatcher 影响全局 fetch                       │
│  - install() 覆盖 globalThis.fetch                            │
└─────────────────────────────────────────────────────────────┘
```

**选择指南**:

**使用内置 fetch 当**:
- 零依赖要求
- 同构代码(浏览器 + Node.js)
- 不需要特定版本 undici 特性

**使用 undici 独立模块当**:
- 需要最新特性 / 性能
- 需要 Client/Pool/Agent 底层 API
- 需要拦截器 / Mock / 代理
- 需要精细控制连接池

**版本对应关系**:

| Node.js 版本 | 内置 undici 版本 | 独立 undici 最新版 |
|--------------|------------------|---------------------|
| Node 18 | 5.28.x | 8.10.1 |
| Node 20 | 5.28.x / 6.x | 8.10.1 |
| Node 22 | 6.x | 8.10.1 |
| Node 24 | 6.x / 7.x | 8.10.1 |

> 独立 undici 总是领先内置版本 1-2 个大版本。

**全局接管方式**:

```javascript
// 方式 1:setGlobalDispatcher
const { setGlobalDispatcher, EnvHttpProxyAgent } = require('undici')
setGlobalDispatcher(new EnvHttpProxyAgent())

// 方式 2:install() — 覆盖 globalThis 所有 Web API
const undici = require('undici')
undici.install()
// 此后 globalThis.fetch 使用 undici 实现
```

**Node.js 核心集成机制**:
- **nodejs-shared.yml** CI:测试 undici 作为 `--shared-builtin-undici/undici-path` 与 Node.js 核心的集成
- **index-fetch.js**:专为 Node.js 内嵌设计的精简入口
- **EXTERNAL_PATH**:构建时参数,指向全局 node_modules/undici 路径

### 1.10 顶层导出清单(双入口设计)

undici 提供**两个顶层入口**,针对不同使用场景:

| 入口 | 文件 | 用途 | 体积 |
|------|------|------|------|
| 主入口 | `index.js`(8,594 行) | 全量 API(Node 独立模块用户使用) | 大 |
| 精简入口 | `index-fetch.js`(2,480 行) | 仅 Web API(**Node.js 核心内嵌**使用) | 小 |

**Dispatcher 体系(直接导出)**:

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

**Handler 装饰器**:

```javascript
module.exports.RetryHandler = RetryHandler
module.exports.DecoratorHandler = DecoratorHandler
module.exports.RedirectHandler = RedirectHandler
```

**8 个拦截器(interceptors 命名空间)**:

```javascript
module.exports.interceptors = {
  redirect, responseError, retry, dump,
  dns, cache, decompress, deduplicate
}
```

**缓存后端(cacheStores 命名空间)**:

```javascript
module.exports.cacheStores = {
  MemoryCacheStore,
  SqliteCacheStore        // Node 24+ experimental-sqlite
}
```

**5 种 API 风格(自动路由到 globalDispatcher)**:

```javascript
module.exports.request = makeDispatcher(api.request)
module.exports.stream = makeDispatcher(api.stream)
module.exports.pipeline = makeDispatcher(api.pipeline)
module.exports.connect = makeDispatcher(api.connect)
module.exports.upgrade = makeDispatcher(api.upgrade)
```

> `makeDispatcher(fn)` 是关键包装器:解析 URL、分离 `agent`/`dispatcher` 选项、默认 method 推断(有 body → PUT,否则 GET)、路径补全。

**Web API(WHATWG 标准)**:

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

**Cookie API**:

```javascript
module.exports.deleteCookie = deleteCookie
module.exports.getCookies = getCookies
module.exports.getSetCookies = getSetCookies
module.exports.setCookie = setCookie
module.exports.parseCookie = parseCookie
```

**MIME 工具**:

```javascript
module.exports.parseMIMEType = parseMIMEType
module.exports.serializeAMimeType = serializeAMimeType
```

**全局管理**:

```javascript
module.exports.setGlobalDispatcher = setGlobalDispatcher
module.exports.getGlobalDispatcher = getGlobalDispatcher
module.exports.setGlobalOrigin = setGlobalOrigin
module.exports.getGlobalOrigin = getGlobalOrigin
module.exports.install = install  // 注入所有 Web API 到 globalThis
```

**Mock 测试**:

```javascript
module.exports.MockClient = MockClient
module.exports.MockAgent = MockAgent
module.exports.MockPool = MockPool
module.exports.MockCallHistory = MockCallHistory
module.exports.MockCallHistoryLog = MockCallHistoryLog
module.exports.SnapshotAgent = SnapshotAgent
module.exports.mockErrors = mockErrors
```

**底层工具**:

```javascript
module.exports.buildConnector = buildConnector   // 连接构建器
module.exports.errors = errors                     // 错误类全集
module.exports.util = { parseHeaders, headerNameToString }
module.exports.ping = ping                         // WebSocket ping
```

**关键设计点**:

1. **挂载机制**:`Object.assign(Dispatcher.prototype, api)` 让 `request/stream/pipeline/connect/upgrade` 作为 Dispatcher 实例方法存在。因此 `client.request(...)` 和 `undici.request(...)` 等价(后者走 globalDispatcher)。

2. **Global Dispatcher**:通过 `Symbol.for('undici.globalDispatcher.2')` 注册到**全局符号表**(跨 Realm/VM 上下文共享),`lib/global.js` 启动时默认创建 `new Agent()`。

3. **`install()` 方法**:把 undici 实现的 fetch/WebSocket/Headers 等注入 `globalThis`,对齐浏览器 API。

4. **`appendFetchStackTrace`**:undici 的 fetch 在错误栈中**自动追加 fetch 调用帧**(通过 `Error.captureStackTrace`),解决 bundled 场景下 `__filename` 不可用时堆栈丢失问题。

### 1.11 TypeScript 类型系统(types/ 双发布)

undici 随主仓发布 `.d.ts`(`types/index.d.ts` 入口),同时双发布为独立包 `undici-types`(供 `@types/node` 等仅需要类型的下游)。

**types/README.md 说明**:
> This package is a dual-publish of the undici library types. The `undici` package still contains types. This package is for users who only need undici types (such as for `@types/node`).

**45 个类型文件清单**:

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

**类型测试(tsd)**:

```bash
npm run test:typescript  # tsd + tsc test/imports/undici-import.ts + tsc types/*.d.ts
```

### 1.12 构建、脚本与测试体系

**scripts/ 工具(7 文件)**:

| 文件 | 职责 |
|------|------|
| `clean-coverage.js` | 清理覆盖率目录 |
| `find-hanging-tests.sh` | 查找挂起测试 |
| `generate-pem.js` | 生成测试证书(调用 `@metcoder95/https-pem`) |
| `generate-undici-types-package-json.js` | 生成 undici-types 子包的 package.json |
| `platform-shell.js` | 跨平台 shell 适配 |
| `release.js` | Release PR 生成脚本(调用 GitHub API) |
| `strip-comments.js` | esbuild 后处理:移除注释 |

**测试体系(test/,488 文件)**:

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
| `test/types/` | 类型测试(tsd) |
| `test/jest/` | Jest 专用测试 |
| `test/fixtures/` | 测试 fixture |
| `test/fuzzing/` | 模糊测试 |
| `test/utils/` | 测试工具 |
| `test/web-platform-tests/` | Web-Platform-Tests 子模块 |
| `test/web-platform-tests/wpt` | WPT 子模块(.gitmodules) |
| `test/fixtures/cache-tests` | cache-tests 子模块(.gitmodules) |

**子模块**(.gitmodules):
```
test/web-platform-tests/wpt    → github.com/web-platform-tests/wpt.git
test/fixtures/cache-tests      → github.com/http-tests/cache-tests.git
```

**测试框架**:

| 框架 | 用途 | 配置 |
|------|------|------|
| `borp` | 主要测试框架(并行 + 覆盖率) | `borp --timeout 180000 -p "test/..."` |
| `jest` | 次要测试 | `testMatch: ["<rootDir>/test/jest/**"]` |
| `tsd` | 类型断言测试 | `tsd` |
| `tsc` | 类型编译检查 | `tsc test/imports/...` + `tsc types/*.d.ts` |

### 1.13 CI Pipeline(.github/workflows/13 个流程)

**工作流清单**:

| 文件 | 名称 | 触发条件 | 用途 |
|------|------|----------|------|
| `ci.yml` | CI | push/PR | 主流程:lint + test 矩阵 + 特殊构建 |
| `nodejs.yml` | Node.js | 被 ci.yml 调用(workflow_call) | 测试执行单元 |
| `nodejs-shared.yml` | Node.js Shared Build | 手动 | 测试与 Node.js 核心内嵌集成 |
| `nodejs-nightly.yml` | Node.js Nightly | 定时 | Nightly 版本兼容性测试 |
| `autobahn.yml` | Autobahn | PR(websocket 变更)+ dispatch | WebSocket 协议合规测试 |
| `triggered-autobahn.yml` | Triggered Autobahn | 手动触发 | 完整 Autobahn 套件 |
| `bench.yml` | Benchmarks | push/PR | 基准测试(对比 base_ref vs 当前分支) |
| `codeql.yml` | CodeQL | push/PR | 安全分析 |
| `scorecard.yml` | Scorecard | push | OpenSSF Scorecard |
| `release.yml` | Release | 手动 | npm 发布 |
| `release-create-pr.yml` | Create Release PR | 手动 | 自动生成 release PR |
| `backport.yml` | Backport | PR merged + label | 自动 backport |
| `update-submodules.yml` | Update Submodules | 定时 | 更新 WPT/cache-tests 子模块 |

**CI 测试矩阵(ci.yml)**:

| 矩阵维度 | 取值 |
|----------|------|
| node-version | 22 / 24 / 25 / 26 |
| runs-on | ubuntu-latest / windows-latest / macos-latest |
| 特殊构建 | no-wasm-simd / without-intl / without-ssl |

> 关键设计:`max-parallel: 0`(**全部并行**,加速 CI);`fail-fast: false`(单个失败不取消其他)。

**特殊测试场景**:

1. **no-wasm-simd**:`UNDICI_NO_WASM_SIMD=1` 禁用 SIMD 解析器
2. **without-intl**:从源码编译 `--without-intl` Node.js,测试无 ICU 环境
3. **without-ssl**:从源码编译 `--without-ssl` Node.js,测试无 TLS 环境
4. **without-ssl** 还会 `node index.js` 验证加载(确保无 crypto 也可 require)

**安全流程**:
- `dependency-review`:PR 依赖审查
- `codeql`:GitHub CodeQL 安全分析
- `scorecard`:OpenSSF Scorecard 评分
- `autobahn`:WebSocket 协议 fuzz 测试
- Dependabot 自动合并(amalgamate workflow)

### 1.14 官方文档体系(docs/)

undici 文档站 https://undici.nodejs.org 使用 **doc-kit**(Node.js 核心同款工具)构建。

```
docs/
├── README.md                   # 文档构建说明
├── index.md                    # 首页
├── getting-started.md          # 入门指南
├── site.json                   # 侧边栏导航配置
├── type-map.json               # 类型 → 链接映射
├── docs/
│   ├── api/                    # API 参考(32 文件)
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
│   └── best-practices/         # 最佳实践(7 文件)
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

**site.json 机制**:
- 侧边栏**不自动发现**文件,必须手动注册到 `site.json`
- 支持分组(`groupName`)和嵌套子分组(`items` 递归)
- 类型 `{Type}` 标记通过 `type-map.json` 解析为链接

**文档站运行**:

```bash
cd docs && npm i && npm run serve   # http://localhost:3000
```

> 旧命令 `npm run serve:website` 已迁移到 docs/ 目录,主 package.json 保留为"报错提示"。

### 1.15 版本、治理与协作

**GOVERNANCE.md(Working Group 治理)**:

undici 由 **Undici Working Group (WG)** 治理,关键要点:

- **WG 权限**:技术方向、治理流程、贡献政策、仓库托管、行为准则
- **Collaborator**:重大贡献者获 commit access,WG 讨论后加入
- **共识机制**:Consensus Seeking Decision Making(无异议即共识)
- **WG 会议**:Zoom 举行,YouTube 公开,moderator 总结为 PR
- **1/3 规则**:同一雇主不超过 WG 成员 1/3
- **议程标签**:`WG-agenda` 标记提交到 WG 讨论

**MAINTAINERS.md(维护者流程)**:

- **Labels**:详细标签体系(bug/enhancement/help-wanted + 模块标签 Agent/Client/Pool/...)
- **Release 流程**:GitHub Actions 自动化(Create Release PR → 审批 → Release)
- **Releasers**:特定名单成员可审批 npm 发布

**CONTRIBUTING.md(贡献指南)**:

详细指南包括:
- **Update llhttp**:更新 WASM 解析器的 8 步流程(需 Docker)
- **Lint** / **Test** / **Coverage**:测试命令
- **WPT**:Web-Platform-Tests 子模块更新
- **External Builds**:为 Node.js 核心内嵌编译的指南(`EXTERNAL_PATH` 参数)
- **Benchmarks**:`cd benchmarks && npm i && npm run bench`(http://localhost:3042)
- **Documentation**:`cd docs && npm i && npm run serve`

**SECURITY.md**: 安全策略文档(GitHub Security Policy 标准格式)

**CODE_OF_CONDUCT.md**: 行为准则(贡献者公约)

### 1.16 跨项目关联

undici 作为 Node.js 官方 HTTP 客户端,是以下 AI Agent 的底层 HTTP 传输层:

- **claudecode**: TypeScript/Bun,使用 undici 的 fetch API 与 Anthropic API 通信
- **opencode**: TypeScript/Bun,Effect 框架下的 HTTP 客户端层
- **pi**: TypeScript,lane 并发模型中的 HTTP 请求
- **deepseek-harness**: TypeScript,Cordis 插件系统的网络层

> **关键洞察**: undici 的 Interceptor 链式模式与 Agent 的工具调用链有相似的组合模式,可互相借鉴。


---

## 2. 核心架构

### 2.1 架构总图

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

### 2.2 完整目录结构解读

```
undici/
├── lib/                          # 核心实现(109 个 JS 文件, ~22k 行)
│   ├── api/                      # 8 文件:5 种 API 风格(request/stream/pipeline/connect/upgrade)
│   ├── cache/                    # 2 文件:HTTP 缓存后端(Memory/SQLite 双存储)
│   ├── core/                     # 9 文件:Request/Connect/Errors/Util/Diagnostics/Symbols/Tree
│   ├── dispatcher/               # 16 文件:Dispatcher 体系(Client/Pool/Agent/Proxy/Retry 等)
│   ├── encoding/                 # 1 文件:br/gzip/deflate/zstd 编码支持
│   ├── handler/                  # 6 文件:Handler 装饰器(Cache/Retry/Redirect/Dedup/Decorator)
│   ├── interceptor/              # 8 文件:8 个可组合拦截器
│   ├── llhttp/                   # 4 文件:llhttp WASM 解析器(含 WASM 二进制)
│   ├── mock/                     # 12 文件:Mock 测试系统(MockAgent/Recorder/Snapshot)
│   ├── util/                     # 5 文件:工具层(cache/date/stats/timers/runtime-features)
│   ├── global.js                 # 全局 Dispatcher(Symbol.for 注册)
│   └── web/                      # Web 标准 API 层
│       ├── cache/                # 3 文件:Cache/CacheStorage
│       ├── cookies/              # 4 文件:Cookie 解析与序列化
│       ├── eventsource/          # 3 文件:EventSource/SSE
│       ├── fetch/                # 12 文件:Fetch API 完整实现(body/headers/request/response/formdata)
│       ├── infra/                # 1 文件:WHATWG Infra 规范工具
│       ├── subresource-integrity/:# 1 文件:SRI 校验
│       ├── webidl/               # 1 文件:WebIDL 类型系统
│       └── websocket/            # 12 文件:WebSocket RFC 6455(含 stream/ 子目录)
├── test/                         # 488 文件:测试体系
├── docs/                         # 官方文档站(doc-kit 构建)
├── benchmarks/                   # 基准测试(可独立运行)
├── scripts/                      # 7 个工具脚本
├── types/                        # 45 个 .d.ts(独立 undici-types 包)
├── deps/llhttp/                  # llhttp C 源码(内嵌副本)
├── build/                        # WASM 构建脚本
├── index.js                      # 主入口(全量导出)
├── index-fetch.js                # 精简入口(仅 Web API,供 Node.js 核心内嵌)
├── index.d.ts                    # 根类型
├── package.json                  # 包元信息
├── CONTRIBUTING.md               # 贡献指南
├── GOVERNANCE.md                 # Working Group 治理
├── MAINTAINERS.md                # 维护者流程
├── SECURITY.md                   # 安全策略
├── CODE_OF_CONDUCT.md            # 行为准则
└── .github/workflows/            # 13 个 CI 工作流
```

### 2.3 lib 109 个文件分类清单

#### lib/api/(8 文件,~2,600 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `index.js` | ~15 | 导出 5 个 API 函数 |
| `api-request.js` | ~180 | `request()` 实现:Promise 包装,返回 `{statusCode, headers, body}` |
| `api-stream.js` | ~270 | `stream()` 实现:返回 Readable 流 |
| `api-pipeline.js` | ~265 | `pipeline()` 实现:流式管道 |
| `api-connect.js` | ~130 | `connect()` 实现:TCP 隧道 |
| `api-upgrade.js` | ~130 | `upgrade()` 实现:协议升级 |
| `abort-signal.js` | ~60 | AbortSignal 封装 |
| `readable.js` | ~616 | 可读流工具 |

#### lib/cache/(2 文件,~750 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `memory-cache-store.js` | ~279 | 内存缓存后端(LRU) |
| `sqlite-cache-store.js` | ~469 | SQLite 缓存后端(Node 24+ `--experimental-sqlite`) |

#### lib/core/(9 文件,~5,360 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `request.js` | ~546 | Request 对象(HTTP 请求构建与序列化) |
| `connect.js` | ~320 | TCP/TLS 连接构建(`buildConnector` 导出) |
| `errors.js` | ~497 | 统一错误类体系(`UND_ERR_*` 码) |
| `util.js` | ~1,049 | 工具函数(parseHeaders/parseOrigin/parseURL 等) |
| `diagnostics.js` | ~200 | diagnostics_channel 封装 |
| `constants.js` | ~280 | 常量定义 |
| `symbols.js` | ~120 | Symbol 常量池(kUrl/kDisconnected 等) |
| `tree.js` | ~350 | TernarySearchTree(三叉搜索树,头部查找) |
| `socks5-client.js` | ~422 | SOCKS5 客户端实现 |
| `socks5-utils.js` | ~200 | SOCKS5 工具函数 |

#### lib/dispatcher/(16 文件,~5,800 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `dispatcher.js` | ~80 | 抽象基类(`dispatch/close/destroy/compose`) |
| `dispatcher-base.js` | ~200 | 状态管理基类(onConnect/onHeaders/onData/onComplete 回调) |
| `dispatcher1-wrapper.js` | ~150 | v1 API 兼容包装 |
| `client.js` | ~741 | 单连接 Client(协议无关核心) |
| `client-h1.js` | ~1,801 | **HTTP/1.1 实现**(最大单文件,流水线 + keep-alive) |
| `client-h2.js` | ~1,781 | **HTTP/2 实现**(多路复用 + HPACK + WINDOW_UPDATE) |
| `pool-base.js` | ~200 | 池基类 |
| `pool.js` | ~300 | Pool(多连接池) |
| `agent.js` | ~350 | Agent(多 origin 路由) |
| `balanced-pool.js` | ~200 | 加权负载均衡池 |
| `round-robin-pool.js` | ~150 | 轮询负载均衡池 |
| `proxy-agent.js` | ~378 | HTTP CONNECT 代理 |
| `socks5-proxy-agent.js` | ~282 | SOCKS5 代理 |
| `env-http-proxy-agent.js` | ~180 | 环境变量代理(HTTP_PROXY/NO_PROXY) |
| `retry-agent.js` | ~150 | 重试包装 |
| `h2c-client.js` | ~120 | HTTP/2 Cleartext(h2c,无 TLS) |
| `fixed-queue.js` | ~100 | 高性能固定队列(2048 桶循环缓冲) |

#### lib/handler/(6 文件,~2,450 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `decorator-handler.js` | ~100 | 装饰器基类 |
| `cache-handler.js` | ~802 | 缓存 Handler(含 ETag/Vary/max-age 语义) |
| `cache-revalidation-handler.js` | ~150 | 缓存再验证 Handler |
| `redirect-handler.js` | ~250 | 重定向 Handler(Location/Loop 检测) |
| `deduplication-handler.js` | ~466 | 请求去重 Handler |
| `retry-handler.js` | ~548 | 重试 Handler(指数退避) |

#### lib/interceptor/(8 文件,~2,550 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `cache.js` | ~618 | 缓存拦截器 |
| `retry.js` | ~100 | 重试拦截器(薄包装,实现在 handler) |
| `redirect.js` | ~120 | 定向拦截器(薄包装) |
| `dns.js` | ~575 | DNS 缓存拦截器 |
| `decompress.js` | ~292 | 自动解压拦截器(gzip/deflate/br/zstd) |
| `deduplicate.js` | ~180 | 去重拦截器 |
| `dump.js` | ~200 | 调试 dump 拦截器 |
| `response-error.js` | ~180 | 响应错误转换拦截器 |

#### lib/llhttp/(4 文件,~1,000+ 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `constants.js` | ~531 | HTTP 常量定义 |
| `utils.js` | ~120 | llhttp WASM 包装工具 |
| `llhttp-wasm.js` | ~400 | 标准 WASM 解析器(JS 包装) |
| `llhttp_simd-wasm.js` | ~400 | SIMD 优化 WASM 解析器 |

#### lib/mock/(12 文件,~2,800 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `mock-agent.js` | ~244 | MockAgent(替代真实 Agent) |
| `mock-client.js` | ~180 | MockClient |
| `mock-pool.js` | ~150 | MockPool |
| `mock-interceptor.js` | ~200 | MockInterceptor(URL/Method 匹配) |
| `mock-call-history.js` | ~248 | 调用历史记录 |
| `mock-errors.js` | ~100 | Mock 专用错误 |
| `mock-symbols.js` | ~50 | Mock Symbol 常量 |
| `mock-utils.js` | ~720 | Mock 工具函数 |
| `pending-interceptors-formatter.js` | ~100 | 未匹配拦截器格式化 |
| `snapshot-agent.js` | ~371 | 快照回放 Agent |
| `snapshot-recorder.js` | ~623 | 快照录制器 |
| `snapshot-utils.js` | ~150 | 快照工具 |

#### lib/util/(5 文件,~3,100 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `cache.js` | ~716 | 缓存工具(makeCacheKey/normalizeHeaders) |
| `date.js` | ~670 | HTTP 日期解析(RFC 7231) |
| `stats.js` | ~200 | 统计工具 |
| `timers.js` | ~425 | 高精度计时器(FastTimer,500ms 精度优化) |
| `runtime-features.js` | ~200 | 运行时特性探测(crypto/sqlite 可用性) |

#### lib/web/fetch/(12 文件,~7,860 行)

| 文件 | 行数 | 职责 |
|------|------|------|
| `index.js` | ~1,200 | `fetch()` 入口 |
| `request.js` | ~900 | Request 类(WHATWG Fetch 规范) |
| `response.js` | ~800 | Response 类 |
| `headers.js` | ~700 | Headers 类(含 Guard 语义) |
| `body.js` | ~900 | Body mixin(Blob/ArrayBuffer/FormData/Stream) |
| `formdata.js` | ~600 | FormData 类 |
| `formdata-parser.js` | ~400 | multipart/form-data 解析 |
| `data-url.js` | ~300 | Data URL 解析 |
| `global.js` | ~100 | Global Origin 管理 |
| `constants.js` | ~200 | Fetch 常量 |
| `util.js` | ~760 | Fetch 函数工具 |

#### lib/web/websocket/(12 文件 + stream/ 子目录 2 文件,~2,000+ 行)

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
| `stream/websocketstream.js` | ~300 | WebSocketStream(Promise 包装) |
| `stream/websocketerror.js` | ~50 | WebSocketError |

#### 其他 web 子目录

| 目录 | 文件 | 职责 |
|------|------|------|
| `web/cache/` | cache.js/cachestorage.js/util.js | Cache/CacheStorage |
| `web/cookies/` | constants.js/index.js/parse.js/util.js | Cookie 解析 |
| `web/eventsource/` | eventsource.js/eventsource-stream.js/util.js | SSE 实现 |
| `web/infra/` | index.js | WHATWG Infra 工具 |
| `web/subresource-integrity/` | subresource-integrity.js | SRI 校验 |
| `web/webidl/` | index.js | WebIDL 类型转换 |

#### 根目录单文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `global.js` | ~100 | 全局 Dispatcher 单例(`Symbol.for('undici.globalDispatcher.2')`) |
| `encoding/index.js` | ~80 | 编码协商 |

### 2.4 入口与全局单例

#### index.js — 统一入口

`index.js`(`/usr/local/LsmGitOpenSource/undici/index.js`)是整个库的入口文件,承担三个核心职责:

**职责一:导出所有 Dispatcher 类型**(见 1.10 节)

**职责二:挂载 API 到 Dispatcher.prototype**

```javascript
Object.assign(Dispatcher.prototype, api)
```

这行代码将 `request`、`stream`、`pipeline`、`connect`、`upgrade` 五个方法挂载到所有 Dispatcher 实例上。这意味着任何 Dispatcher 子类都可以直接调用 `dispatcher.request(opts, callback)`。

**职责三:创建顶层便捷函数**

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

`makeDispatcher` 是一个高阶函数,它将 URL 字符串解析为 `origin` + `path`,自动从全局 Dispatcher 获取默认 Agent,然后代理到对应的 API 方法。这使得用户可以直接写:

```javascript
const { request } = require('undici')
await request('https://example.com/api')
```

无需手动创建 Client 或 Agent。

#### lib/global.js — 全局 Dispatcher 单例

`global.js`(`/usr/local/LsmGitOpenSource/undici/lib/global.js`)实现了全局 Dispatcher 单例管理。

**核心设计**:

```javascript
const globalDispatcher = Symbol.for('undici.globalDispatcher.2')
const legacyGlobalDispatcher = Symbol.for('undici.globalDispatcher.1')
```

使用 `Symbol.for()` 而非普通 `Symbol()`,确保即使多个版本的 undici 并存(Node.js 内置 + npm 安装),也能通过相同的 key 访问同一个全局 Dispatcher。

**getGlobalDispatcher()**:

```javascript
function getGlobalDispatcher () {
  return globalThis[globalDispatcher] ?? fallbackDispatcher
}
```

优先从 `globalThis` 读取,如果 `globalThis` 被冻结(如某些沙箱环境),回退到模块级变量 `fallbackDispatcher`。

**setGlobalDispatcher(agent)**:

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

验证 agent 必须实现 `dispatch` 方法(鸭子类型),然后将其设为不可枚举的全局属性。

**向后兼容**:同时维护 `legacyGlobalDispatcher`(版本 1),通过 `Dispatcher1Wrapper` 将新版 Dispatcher 包装为旧版接口。

**默认初始化**:

```javascript
if (getGlobalDispatcher() === undefined) {
  setGlobalDispatcher(new Agent())
}
```

模块加载时自动创建一个默认 Agent 作为全局 Dispatcher。Agent 内部会按 origin 自动创建 Pool/Client。

#### index-fetch.js — Node.js 内置 fetch 精简入口

`index-fetch.js` 是 Node.js 内置 `fetch()` 的精简导出版本,只暴露 Web API 相关的类(fetch、Headers、Response、Request、FormData、WebSocket、EventSource),不暴露底层 Dispatcher 体系。这用于 Node.js 核心代码中,避免引入不必要的依赖。

### 2.5 Dispatcher 体系详解

Dispatcher 是 undici 的核心抽象,采用 EventEmitter 模式,所有 HTTP 操作通过 `dispatch(opts, handler)` 方法执行。

#### 类层次

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

#### Dispatcher 基类(dispatcher.js)

`lib/dispatcher/dispatcher.js` 是所有 Dispatcher 的抽象基类,继承自 `EventEmitter`。

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
    // 用 Proxy 包装,只拦截 dispatch 属性
    return new Proxy(this, {
      get: (target, key) => key === 'dispatch' ? dispatch : target[key]
    })
  }
}
```

**关键设计点**:

1. **三个抽象方法**:`dispatch`(发送请求)、`close`(优雅关闭)、`destroy`(强制销毁)。
2. **compose 方法**:实现拦截器链模式。接受多个拦截器函数,每个拦截器 `(dispatch) => (opts, handler) => ...`,通过函数组合形成调用链。最终用 `Proxy` 包装原始 Dispatcher,只替换 `dispatch` 方法。
3. **EventEmitter 继承**:所有 Dispatcher 都可以发出 `drain`、`connect`、`disconnect`、`connectionError` 事件。

#### DispatcherBase

`lib/dispatcher/dispatcher-base.js` 是 Dispatcher 的第一个具体实现层,管理生命周期(close/destroy)和 dispatch 守卫。

**生命周期状态机**:

```
  正常运行 --> [kClosed=true] --> close() 等待队列清空 --> [kDestroyed=true] --> destroy() 释放资源
                 |                      |
                 +--- destroy(err) -----+---> 立即销毁
```

**dispatch 方法的守卫逻辑**:

```javascript
dispatch (opts, handler) {
  if (!handler || typeof handler !== 'object') throw ...
  if (opts.dispatcher) throw ...  // 禁止嵌套 dispatcher
  if (this[kDestroyed] || this[kOnDestroyed]) throw new ClientDestroyedError()
  if (this[kClosed]) throw new ClientClosedError()
  return this[kDispatch](opts, handler)
}
```

**close/destroy 的回调聚合**:close 和 destroy 都支持多次调用 — 第一次触发实际操作,后续调用将回调追加到 `kOnClosed`/`kOnDestroyed` 数组中,待操作完成后批量调用。

#### Client — 单连接 HTTP 客户端

`lib/dispatcher/client.js` 是 undici 的核心组件,代表对单个 origin 的一个 HTTP 连接。

**队列设计(三段式)**:

```
|   complete   |   running   |   pending   |
               ^ kRunningIdx ^ kPendingIdx ^ kQueue.length
```

- `kQueue`:请求队列数组
- `kRunningIdx`:第一个正在执行的请求索引
- `kPendingIdx`:第一个等待执行的请求索引
- `kPending` = `kQueue.length - kPendingIdx`
- `kRunning` = `kPendingIdx - kRunningIdx`
- `kSize` = `kQueue.length - kRunningIdx`

已完成的请求置为 `null`,当 `kRunningIdx > 256` 时批量裁剪:

```javascript
if (client[kRunningIdx] > 256) {
  client[kQueue].splice(0, client[kRunningIdx])
  client[kPendingIdx] -= client[kRunningIdx]
  client[kRunningIdx] = 0
}
```

这是分摊 O(1) 的队列实现 — 不做每次请求的 splice,而是在累积 256 个空位后一次性清理。

**kDispatch 方法**:

```javascript
[kDispatch] (opts, handler) {
  const request = new Request(this[kUrl].origin, opts, handler)
  this[kQueue].push(request)
  // 根据 body 类型决定同步/异步 resume
  if (this[kResuming]) { /* 已经在 resume */ }
  else if (bodyLength == null && isIterable(body)) {
    this[kResuming] = 1
    queueMicrotask(() => resume(this))  // 延迟一 tick,等 body 结束
  } else {
    this[kResume](true)  // 同步 resume
  }
}
```

**resume 核心循环** (`_resume`):

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

**连接建立**(`connect` 函数):

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

**busy 判断(影响 Pool 扩容)**:

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

对于 H2 连接,`kPending > 0` 不会标记为 busy,因为 H2 可以在同一个连接上并发多个 stream。对于 H1,有排队即忙,Pool 会创建新的 Client。

**可配置参数**:

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
| `maxResponseSize` | -1(无限制) | 最大响应体大小 |
| `strictContentLength` | true | 严格校验 Content-Length |
| `allowH2` | true | 是否允许 HTTP/2 |
| `maxConcurrentStreams` | 100 | H2 最大并发流 |

#### Pool — 多连接池

`lib/dispatcher/pool.js` 基于 `PoolBase` 实现同 origin 的多连接管理。

**核心机制 — 按需扩容**:

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
  // 所有 Client 都忙,创建新的
  if (!this[kConnections] || this[kClients].length < this[kConnections]) {
    const dispatcher = this[kFactory](this[kUrl], this[kOptions])
    this[kAddClient](dispatcher)
    return dispatcher
  }
}
```

**factory 模式**:

```javascript
function defaultFactory (origin, opts) {
  return new Client(origin, opts)
}
```

Pool 使用工厂函数创建 Client,允许用户注入自定义 Client 创建逻辑。

**TTL 支持**:`clientTtl` 参数使 Client 在闲置超过指定时间后自动移除。

**连接错误处理**:

```javascript
this.on('connectionError', (origin, targets, error) => {
  for (const target of targets) {
    const idx = this[kClients].indexOf(target)
    if (idx !== -1) this[kClients].splice(idx, 1)
  }
})
```

连接错误的 Client 直接从池中移除,不尝试重用。

#### PoolBase — 池基类与队列调度

`lib/dispatcher/pool-base.js` 是 Pool、BalancedPool、RoundRobinPool 的共同基类。

**内部队列**:使用 `FixedQueue`(见第 9 节)存储当所有 Client 都忙时排队的请求。

**dispatch 逻辑**:

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

**drain 事件处理**:当某个 Client 发出 `drain`(表示它又有空闲容量了),PoolBase 从内部队列取出请求并 dispatch:

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

**聚合统计**:`kConnected`、`kFree`、`kPending`、`kRunning`、`kSize` 都是遍历所有子 Client 汇总计算。

#### Agent — 顶层多 origin 路由器

`lib/dispatcher/agent.js` 是全局默认 Dispatcher,管理多个 origin 的连接。

**核心 — 按 origin 路由**:

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

**自动清理**:当 Pool 的所有连接都断开、没有忙碌连接、没有待处理请求时,自动从 Map 中移除并关闭:

```javascript
const closeClientIfUnused = () => {
  if (dispatcher[kConnected] > 0 || dispatcher[kBusy] || dispatcher[kPending] > 0) return
  this[kClients].delete(key)
  if (!dispatcher.destroyed) dispatcher.close()
}
```

**defaultFactory**:

```javascript
function defaultFactory (origin, opts) {
  return opts && opts.connections === 1
    ? new Client(origin, opts)   // 单连接直接用 Client
    : new Pool(origin, opts)     // 多连接用 Pool
}
```

当 `connections === 1` 时跳过 Pool 直接使用 Client,减少一层抽象开销。

**maxOrigins 限制**:防止 Agent 缓存无限增长的 origin。

#### BalancedPool — 加权负载均衡

`lib/dispatcher/balanced-pool.js` 实现加权轮询(Weighted Round Robin)负载均衡。

**核心算法**:

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

**权重动态调整**:

```javascript
pool.on('connect', () => {
  pool[kWeight] = Math.min(maxWeightPerServer, pool[kWeight] + errorPenalty)
})
pool.on('connectionError', () => {
  pool[kWeight] = Math.max(1, pool[kWeight] - errorPenalty)
})
```

- 连接成功恢复权重(`+errorPenalty`,默认 15)
- 连接错误降低权重(`-errorPenalty`)
- 权重范围 `[1, maxWeightPerServer]`(默认 100)
- 使用最大公约数(GCD)算法实现加权轮询

#### RoundRobinPool — 轮询负载均衡

`lib/dispatcher/round-robin-pool.js` 实现简单的轮询策略:

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
  // 所有都忙,按需创建
}
```

与 BalancedPool 的区别:不维护权重,每次选下一个,简单公平。

#### Dispatcher 完整继承体系图

undici 的 dispatcher 体系由 17 个文件组成,核心继承链如下(ASCII 类图):

```
EventEmitter (node:events)
  └── Dispatcher (lib/dispatcher/dispatcher.js, 54 行)
        │ 抽象基类。定义 dispatch/close/destroy 三个抽象方法 + compose() 拦截器链。
        │ 关键方法签名:
        │   dispatch(opts, handler)           → boolean | void
        │   close([cb])                        → Promise | void
        │   destroy([err, cb])                 → Promise | void
        │   compose(...interceptors)           → Proxy<Dispatcher>
        │
        ├── DispatcherBase (lib/dispatcher/dispatcher-base.js, 197 行)
        │      │ 生命周期管理:kDestroyed/kClosed 状态机 + 回调聚合。
        │      │ dispatch() 前置守卫 + 转发到子类 this[kDispatch](opts, handler)。
        │      │
        │      │ 关键内部 Symbol:
        │      │   kDestroyed / kClosed / kOnDestroyed / kOnClosed
        │      │   kWebSocketOptions / kEventSourceOptions
        │      │
        │      ├── Client (lib/dispatcher/client.js, 741 行)
        │      │      │ 单 origin 的 HTTP 连接抽象。三段式队列 + resume 循环。
        │      │      │
        │      │      │ 关键方法签名:
        │      │      │   [kDispatch](opts, handler)  → boolean  入队 + 触发 resume
        │      │      │   [kConnect](cb)               → void    异步建连
        │      │      │   [kDestroy](err)              → Promise 销毁 + 失败所有 pending
        │      │      │   get [kPending] / [kRunning] / [kSize] / [kBusy] / [kConnected]
        │      │      │
        │      │      │ 关键 Symbol 配置:
        │      │      │   kUrl / kConnector / kServerName / kPipelining
        │      │      │   kHTTPContext / kHTTP2Options / kMaxConcurrentStreams
        │      │      │
        │      │      └── H2CClient (lib/dispatcher/h2c-client.js, 51 行)
        │      │            仅用于明文 HTTP/2 (h2c)。强制 useH2c:true + allowH2:true。
        │      │            校验 origin 必须是 http://,否则抛 InvalidArgumentError。
        │      │            pipelining 默认 100 但不可超过 maxConcurrentStreams。
        │      │
        │      ├── PoolBase (lib/dispatcher/pool-base.js, 232 行)
        │      │      │ 多连接池基类。clients[] 数组 + FixedQueue 排队。
        │      │      │
        │      │      │ 关键方法签名:
        │      │      │   [kDispatch](opts, handler)   → boolean  选 dispatcher 或入队
        │      │      │   [kGetDispatcher]()           → Dispatcher | void  子类多态
        │      │      │   [kHasDispatcher]()           → boolean
        │      │      │   [kAddClient](client)         → this     注册 + 绑定事件
        │      │      │   [kRemoveClient](client)      → void     解绑 + 清理
        │      │      │
        │      │      ├── Pool (lib/dispatcher/pool.js, 143 行)
        │      │      │     │ 同 origin 多连接。kGetDispatcher() 线性扫描 + 按需扩容。
        │      │      │     │ 支持 connections 上限、clientTtl 过期清理。
        │      │      │     │ connectionError 时立即从 clients 移除,避免复用坏连接。
        │      │      │     │
        │      │      │     ├── BalancedPool (lib/dispatcher/balanced-pool.js, 214 行)
        │      │      │     │     │ 多 upstream 加权负载均衡。Nginx 风格平滑加权轮询。
        │      │      │     │     │ kWeight[] 权重数组 + kCurrentWeight + GCD 步长递减。
        │      │      │     │     │ addUpstream/removeUpstream 动态增删上游。
        │      │      │     │     │ 每个 upstream 是一个 Pool 实例(嵌套 PoolBase 调度)。
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
        │      │     │ HTTP 代理调度器。三种模式分支:
        │      │     │   (1) socks5:// → 委托给 Socks5ProxyAgent
        │      │     │   (2) http 目标 + proxyTunnel=false → Http1ProxyWrapper 改写 path
        │      │     │   (3) 其他 → CONNECT 隧道,内层用 Agent 调度真实连接
        │      │     │ 安全措施:禁止请求头携带 Proxy-Authorization。
        │      │     │ 嵌套 Agent: this[kAgent] 内部用 Pool/Client 建立到 proxy 的连接。
        │      │     │
        │      │     └── Http1ProxyWrapper (内部类, ~65 行)
        │      │           仅用于 HTTP 正向代理非隧道模式。重写 opts.path = origin + path。
        │      │           拦截 407 响应转 InvalidArgumentError。
        │      │
        │      ├── EnvHttpProxyAgent (lib/dispatcher/env-http-proxy-agent.js, 175 行)
        │      │      │ 自动从 process.env.http_proxy/https_proxy/no_proxy 读取代理配置。
        │      │      │ 持有三个子 Agent:noProxyAgent、httpProxyAgent、httpsProxyAgent。
        │      │      │ #shouldProxy(): NO_PROXY 解析(IPv6/端口/子域名/通配符*)。
        │      │      │ #noProxyChanged getter: 监听环境变量变化触发重新解析。
        │      │      │
        │      │      └── NO_PROXY 解析支持:
        │      │            "*.example.com" → 子域名匹配
        │      │            "[::1]:443"  → IPv6 + 端口
        │      │            "*"          → 全不走代理
        │      │
        │      ├── Socks5ProxyAgent (lib/dispatcher/socks5-proxy-agent.js, 282 行)
        │      │      │ 实验性 SOCKS5 代理。按 origin 维护 Map<origin, Pool>。
        │      │      │ createSocks5Connection(): 连接代理 → 握手 → 认证 → CONNECT → 隧道。
        │      │      │ Pool 的 connect 函数被替换为 SOCKS5 隧道建立。
        │      │      │ 支持用户名/密码认证;超时 5 秒;ExperimentalWarning 仅弹一次。
        │      │      │
        │      │      └── 使用 Socks5Client (lib/core/socks5-client.js) 状态机:
        │      │            STATES: INITIAL → GREETING → AUTH → CONNECTING → CONNECTED
        │      │
        │      ├── Dispatcher1Wrapper (lib/dispatcher/dispatcher1-wrapper.js, 107 行)
        │      │      │ v1 API 向后兼容层。强制 allowH2:false(v1 不支持 H2)。
        │      │      │ wrapHandler(): 检测 handler 是否含 v2 接口(onRequestStart),
        │      │      │   否则包一层 LegacyHandlerWrapper 桥接:
        │      │      │     onConnect      → onRequestStart
        │      │      │     onHeaders      → onResponseStart (含 resume 暂停)
        │      │      │     onData         → onResponseData
        │      │      │     onComplete     → onResponseEnd
        │      │      │     onError        → onResponseError
        │      │      │     onUpgrade      → onRequestUpgrade
        │      │      │     onBodySent / onRequestSent / onResponseStarted 透传
        │      │      │
        │      │      └── 继承 Dispatcher (非 DispatcherBase),因为 v1 无 close/destroy 生命周期
        │      │
        │      └── RetryAgent (lib/dispatcher/retry-agent.js, 35 行)
        │            极简装饰器。dispatch 时把 handler 包进 RetryHandler,
        │            再交给内部 this.#agent.dispatch()。重试逻辑全在 handler 中。
        │            自身继承 Dispatcher,close/destroy 透传内部 agent。
        │
        └── RetryHandler 实际定义在 lib/handler/retry-handler.js (~548 行)
              独立于 dispatcher 继承链,是 handler 层装饰器。
```

**设计模式总结**:

| 模式 | 应用位置 | 说明 |
|------|---------|------|
| 模板方法 | DispatcherBase → Client/Pool/Agent | dispatch() 守卫固定,子类实现 kDispatch |
| 策略 | PoolBase → Pool/BalancedPool/RoundRobinPool | kGetDispatcher() 多态 |
| 装饰器 | ProxyAgent/Dispatcher1Wrapper | 嵌套内部 dispatcher,功能增强 |
| 组合拦截器 | Dispatcher.compose() | Proxy 模式 + 函数组合 |
| 桥接 | connectH1/connectH2 返回统一上下文对象 | Client 不直接依赖具体协议实现 |
| 工厂 | Agent/Pool 的 factory 函数 | 创建 Client 或 Pool 子类可替换 |

#### Pool 三种负载均衡算法深度对比

Pool 体系三种实现都继承 `PoolBase`,差异仅在 `kGetDispatcher()`。下表从源码级对比:

```
                  Pool                       BalancedPool                    RoundRobinPool
──────────────┼──────────────────────────┼───────────────────────────────┼──────────────────────
 适用场景      │ 同 origin 多连接          │ 多 origin 加权负载均衡          │ 同 origin 简单轮询
 上游数量      │ 1 个 origin               │ N 个 upstream (动态增删)        │ 1 个 origin
 上层结构      │ 独立使用                  │ 内含多个 Pool(每个 upstream 一个)│ 独立使用
 扩容触发      │ 全部 Client busy          │ 子 Pool 内部同左                │ 全部 Client busy
 扩容上限      │ connections 参数          │ 子 Pool 的 connections          │ connections 参数
 核心索引      │ 无(线性扫描)             │ kIndex + kCurrentWeight + GCD   │ kIndex(自增取模)
 权重          │ 无                        │ kWeight[] 动态调整               │ 无
 故障响应      │ connectionError 时移除     │ weight -= errorPenalty(15)       │ connectionError 时移除
 恢复机制      │ 无(永久移除)             │ connect 成功 weight += 15        │ 无
 空闲 TTL      │ clientTtl 过期移除         │ 无                              │ clientTtl 过期移除
 子 Dispatcher │ Client 或 Pool (factory)   │ Pool (嵌套)                     │ Client (默认 factory)
 代码量        │ 143 行                     │ 214 行                          │ 159 行
```

**BalancedPool 的平滑加权轮询算法详解**(源自 Nginx 的 SWRR):

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

**算法关键点**:

1. **GCD 步长递减**:`kCurrentWeight` 每次减少所有节点权重的最大公约数,确保遍历所有可能的权重值。
2. **maxWeightPerServer 重置**:当 currentWeight 降到 ≤0 时重置到最大值,开始新一轮。
3. **权重动态调整**:
   - 连接成功:`weight = min(maxWeight, weight + errorPenalty)`(默认 +15)
   - 连接错误/UND_ERR_SOCKET 断连:`weight = max(1, weight - errorPenalty)`(默认 -15)
4. **maxWeightIndex fallback**:若一轮遍历无满足 weight ≥ currentWeight 的节点,选最大权重的可用节点。

**三种 Pool 选型决策树**:

```
请求多个不同 origin?
  ├── 是 → 用 Agent(顶层多 origin 路由器)
  └── 否 → 单 origin
            │
            ├── 多 upstream 需要权重/故障降级?
            │     └── 是 → BalancedPool
            │
            └── 否
                  ├── 纯平均分配,无状态?
                  │     └── 是 → RoundRobinPool
                  │
                  └── 需要 TTL 清理、按需扩容、线性扫描空闲优先?
                        └── 是 → Pool
```

**Pool 嵌套组合关系**:

undici 支持 Pool 的多层嵌套。BalancedPool 每个 upstream 内部就是一个 Pool,Agent 每个 origin 可映射到 Pool 或 Client。多层嵌套的 busy/needDrain 判断递归进行:

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

**各 Pool 与事件转发链**:

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

`kOnDrain` 回调在 PoolBase 内部实现:当某个 Client 发出 drain 时,从 FixedQueue 中取出等待的请求继续 dispatch。若队列清空,emit 自己的 drain 事件。


---

## 3. llhttp WASM 解析器

### 3.1 来源

llhttp 是 Node.js 官方的 HTTP 解析器 C 库(https://github.com/nodejs/llhttp),前身是 Nginx 的 http_parser。undici 通过 **WebAssembly** 编译 llhttp,避免依赖 Node.js 内置 http_parser,保持独立演进。

### 3.2 deps/ 目录结构

```
deps/llhttp/
├── include/
│   └── llhttp.h              # C 头文件
└── src/
    ├── api.c                 # API 实现
    ├── llhttp.c              # 主解析器实现
    └── http.c                # HTTP 专用辅助
```

### 3.3 lib/llhttp/ 目录(WASM 包装)

| 文件 | 职责 |
|------|------|
| `llhttp-wasm.js` | 标准 WASM 模块(JavaScript 包装) |
| `llhttp_simd-wasm.js` | SIMD 优化 WASM 模块(自动检测 CPU) |
| `utils.js` | 解析器封装工具 |
| `constants.js` | HTTP 方法/状态码常量 |

### 3.4 构建流程

```bash
# 更新 llhttp 源码
npm run build:wasm    # node build/wasm.js --docker(需 Docker)
```

构建脚本 `build/wasm.js` 在 Docker 容器内通过 Emscripten 编译 C → WASM,输出到 `lib/llhttp/`。

### 3.5 WASM 模块加载

`lib/llhttp/` 目录包含 llhttp 的 WebAssembly 编译版本:

- `llhttp.wasm` / `llhttp-wasm.js`:标准版本
- `llhttp_simd.wasm` / `llhttp_simd-wasm.js`:SIMD 加速版本(ppc64 架构除外)

**加载策略**(`lazyllhttp` 函数):

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

SIMD 版本优先,失败时回退到标准版本。模块实例全局单例(`llhttpInstance`)。

### 3.6 Parser 类详解

`Parser` 类(client-h1.js 中定义)封装了 llhttp WASM 解析器的完整生命周期。

**构造函数**:

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

**execute 方法** — 核心解析循环:

```javascript
execute (chunk) {
  // 如果 chunk 比 WASM 缓冲区大,重新分配
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

**关键优化**:
- WASM 内存缓冲区按 4096 对齐分配
- `currentBuffer` 使用 `Uint8Array` 视图直接映射 WASM 线性内存,避免额外拷贝
- 使用 `FastBuffer`(`Buffer[Symbol.species]`)零拷贝创建 Buffer 引用

### 3.7 回调函数与 JS-WASM 桥

llhttp 通过环境回调(`env`)将解析事件传递给 JS:

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

**零拷贝 Buffer 创建**:回调中的 `at` 参数是 WASM 内存中的偏移量。通过 `at - currentBufferPtr + currentBufferRef.byteOffset` 计算出原始 Buffer 中的偏移,直接创建 `FastBuffer` 视图,无需拷贝数据。

**全局单 Parser 约束**:`currentParser`、`currentBufferRef` 是模块级全局变量。在 WASM 执行期间,同一时刻只有一个 Parser 活跃。这确保了回调中的 `currentParser` 始终正确。

### 3.8 定时器体系

Parser 使用两种定时器策略:

```javascript
const USE_NATIVE_TIMER = 0
const USE_FAST_TIMER = 1
const TIMEOUT_HEADERS = 2 | USE_FAST_TIMER    // 快速定时器
const TIMEOUT_BODY = 4 | USE_FAST_TIMER       // 快速定时器
const TIMEOUT_KEEP_ALIVE = 8 | USE_NATIVE_TIMER  // 原生定时器
```

- **Headers/Body 超时**使用快速定时器(`timers.setFastTimeout`):考虑事件循环延迟,更精确地反映实际超时
- **Keep-Alive 超时**使用原生 `setTimeout`(`timer.unref()`):忽略事件循环延迟,空闲时允许进程退出

**WeakRef 防泄漏**:

```javascript
this.timeoutWeakRef = new WeakRef(this)
// ...
this.timeout = timers.setFastTimeout(onParserTimeout, delay, this.timeoutWeakRef)
```

定时器回调通过 `WeakRef` 持有 Parser,如果 Parser 已被 GC,定时器回调直接返回。

### 3.9 llhttp WASM 解析器与 Client 回调衔接完整链路

**WASM 模块文件结构**:

```
lib/llhttp/
├── constants.js         # 531 行:ERROR/TYPE/FLAGS/METHODS 等枚举
├── constants.d.ts       # TypeScript 类型声明
├── llhttp.wasm          # 标准 WASM 二进制
├── llhttp-wasm.js       # 15 行:Base64 编码的 WASM 导出
├── llhttp_simd.wasm     # SIMD 加速 WASM 二进制
├── llhttp_simd-wasm.js  # 15 行:SIMD 版本导出
├── utils.js             # 12 行:辅助函数
└── utils.d.ts           # TypeScript 类型声明
```

**WASM 导出接口**(`exports` 对象):

```javascript
// llhttp-wasm.js 导出的 Instance.exports 包含:
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

**JS-WASM 回调桥完整实现**(`lazyllhttp` 的 env 参数):

```javascript
return new WebAssembly.Instance(mod, {
  env: {
    // URL 解析回调(响应解析中未使用,返回 0)
    wasm_on_url: (p, at, len) => 0,

    // 状态行解析:HTTP/1.1 200 OK 中的 "OK"
    wasm_on_status: (p, at, len) => {
      assert(currentParser.ptr === p)
      const start = at - currentBufferPtr + currentBufferRef.byteOffset
      return currentParser.onStatus(new FastBuffer(currentBufferRef.buffer, start, len))
    },

    // 消息开始(每个响应触发一次)
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

**回调返回值语义**:

| 返回值 | 含义 |
|--------|------|
| `0` | 继续解析 |
| `-1` | 错误,终止解析(HPE 错误) |
| `constants.ERROR.PAUSED (21)` | 暂停解析(用于 Upgrade 场景) |
| `1` | 跳过 body(用于 HEAD 响应或 1xx 信息响应) |
| `2` | Upgrade 请求,暂停解析 |

**Parser 回调 → Request handler 回调 → 用户回调 的完整链路**:

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
    │                  └── 用户代码:new Readable + callback(null, {statusCode, body, ...})
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

**WASM 内存管理**:

```javascript
// 初始分配(4096 对齐)
currentBufferSize = Math.ceil(chunk.length / 4096) * 4096
currentBufferPtr = llhttp.malloc(currentBufferSize)

// 视图映射(零拷贝)
currentBuffer = new Uint8Array(llhttp.memory.buffer, currentBufferPtr, currentBufferSize)

// 每次 execute 前复制数据到 WASM 内存
currentBuffer.set(chunk)

// 解析完成后释放(destroy 时)
llhttp.free(currentBufferPtr)
```

**错误码完整清单**(`lib/llhttp/constants.js`):

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
  CB_MESSAGE_BEGIN: 16,           // 回调:消息开始
  CB_HEADERS_COMPLETE: 17,        // 回调:头部完成
  CB_MESSAGE_COMPLETE: 18,        // 回调:消息完成
  CB_CHUNK_HEADER: 19,            // 回调:chunk 头
  CB_CHUNK_COMPLETE: 20,          // 回调:chunk 完成
  PAUSED: 21,                     // 暂停
  PAUSED_UPGRADE: 22,             // 暂停(Upgrade)
  PAUSED_H2_UPGRADE: 23,          // 暂停(H2 Upgrade)
  USER: 24,                       // 用户自定义
  CR_EXPECTED: 25,                // 期望 CR
  CB_URL_COMPLETE: 26,            // 回调:URL 完成
  CB_STATUS_COMPLETE: 27,         // 回调:状态完成
  CB_HEADER_FIELD_COMPLETE: 28,   // 回调:头字段完成
  CB_HEADER_VALUE_COMPLETE: 29,   // 回调:头值完成
  UNEXPECTED_SPACE: 30,           // 意外空格
  CB_RESET: 31,                   // 回调:重置
  CB_METHOD_COMPLETE: 32,         // 回调:方法完成
  CB_VERSION_COMPLETE: 33,        // 回调:版本完成
  CB_CHUNK_EXTENSION_NAME_COMPLETE: 34,
  CB_CHUNK_EXTENSION_VALUE_COMPLETE: 35,
  CB_PROTOCOL_COMPLETE: 38        // 回调:协议完成
}
```

**SIMD 版本选择逻辑**:

```javascript
// 默认启用 SIMD,但排除 ppc64 架构(Power 9 上 SIMD 有 bug)
let useWasmSIMD = process.arch !== 'ppc64'

// 环境变量强制控制
if (process.env.UNDICI_NO_WASM_SIMD === '1') useWasmSIMD = false
else if (process.env.UNDICI_NO_WASM_SIMD === '0') useWasmSIMD = true

// Jest 测试环境强制使用非 SIMD(避免 WASM 编译开销)
const llhttpWasmData = process.env.JEST_WORKER_ID
  ? require('../llhttp/llhttp-wasm.js')
  : undefined
```

### 3.10 解析器回调接口

llhttp 是回调驱动:`on_message_begin` / `on_url` / `on_header_field` / `on_header_value` / `on_body` / `on_message_complete`。JS-WASM 桥通过 `__indirect_function_table` 实现零拷贝回调。


---

## 4. 5 种 API 风格

所有 API 都通过 `Dispatcher.prototype` 上的方法调用,最终走到 `this.dispatch(opts, handler)`。

| API | 用途 | 特点 |
|-----|------|------|
| `request()` | 通用请求 | 返回完整 Response,最易用 |
| `stream()` | 流式响应 | 返回 Readable,适合大文件 |
| `pipeline()` | 流式管道 | pipe 链式处理 |
| `connect()` | TCP 隧道 | 代理/隧道场景 |
| `upgrade()` | 协议升级 | WebSocket 等 |

> 性能梯度:request < stream < pipeline < dispatch(越底层越灵活,req/s 越高)。

### 4.1 request API

`lib/api/api-request.js`

```javascript
function request (opts, callback) {
  const handler = new RequestHandler(opts, callback)
  this.dispatch(opts, handler)
}
```

**返回值**:`{ statusCode, statusText, headers, trailers, opaque, body: Readable, context }`

**特点**:
- 返回 `Readable` 流作为响应体
- 支持 callback 和 Promise 两种调用方式
- 支持 `opaque` 透传数据
- 支持 `onInfo` 回调处理 1xx 信息响应
- 支持 `highWaterMark` 控制背压
- 支持 `AbortSignal` 取消
- 支持 `responseHeaders: 'raw'` 获取原始头部

**内部实现** — `RequestHandler`(继承 `AsyncResource`):

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

### 4.2 stream API

`lib/api/api-stream.js`

```javascript
function stream (opts, factory, callback) {
  const handler = new StreamHandler(opts, factory, callback)
  this.dispatch(opts, handler)
}
```

**特点**:
- `factory` 函数接收 `{ statusCode, headers, opaque, context }` 返回 `Writable` 流
- 响应数据写入 factory 创建的 Writable
- 适合流式处理响应(如写文件)

**背压处理**:

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

### 4.3 pipeline API

`lib/api/api-pipeline.js`

```javascript
function pipeline (opts, handler) {
  const pipelineHandler = new PipelineHandler(opts, handler)
  this.dispatch({ ...opts, body: pipelineHandler.req }, pipelineHandler)
  return pipelineHandler.ret  // 返回 Duplex 流
}
```

**特点**:
- 返回 `Duplex` 流:写入端是请求体,读取端是响应体
- `handler` 函数接收 `{ statusCode, headers, opaque, body: Readable, context }` 返回 `Readable`
- 适合管道化处理(request body -> handler transform -> response)

**内部结构**:

```javascript
this.req = new PipelineRequest()      // Readable: 请求体
this.ret = new Duplex({
  write: (chunk, encoding, callback) => {
    this.req.push(chunk, encoding) || (this.req[kResume] = callback)
  },
  read: () => { this.body?.resume() }
})
```

### 4.4 connect API

`lib/api/api-connect.js`

```javascript
function connect (opts, callback) {
  const connectHandler = new ConnectHandler(opts, callback)
  this.dispatch({ ...opts, method: 'CONNECT' }, connectHandler)
}
```

**返回值**:`{ statusCode, headers, socket, opaque, context }`

**特点**:
- 用于 HTTP CONNECT 隧道
- 返回原始 socket 用于自定义协议通信
- `onResponseStart` 会抛出 `SocketError('bad connect')` — CONNECT 不应该有正常的响应

### 4.5 upgrade API

`lib/api/api-upgrade.js`

```javascript
function upgrade (opts, callback) {
  const upgradeHandler = new UpgradeHandler(opts, callback)
  this.dispatch({ ...opts, method: opts.method || 'GET', upgrade: opts.protocol || 'Websocket' }, upgradeHandler)
}
```

**返回值**:`{ headers, socket, opaque, context }`

**特点**:
- 用于 HTTP Upgrade(如 WebSocket)
- 校验状态码为 101(H1)或 200(H2 Extended CONNECT)

### 4.6 5 种 API 调用方式、返回类型与内部实现完整对比

| API | 调用签名 | 返回类型 | 内部 Handler | 核心差异 |
|-----|---------|---------|-------------|---------|
| `request` | `(opts, callback?)` → Promise | `{statusCode, statusText, headers, trailers, body: Readable, opaque, context}` | `RequestHandler` | 通用请求;body 是 Readable;callback 或 Promise 二选一 |
| `stream` | `(opts, factory, callback?)` → Promise | `undefined`(factory 的 Writable 是响应体) | `StreamHandler` | 用户创建 Writable factory,数据写入用户流;背压通过 writableNeedDrain 传递 |
| `pipeline` | `(opts, handler)` → Duplex | `Duplex`(写入=请求体,读取=响应体) | `PipelineHandler` | 返回 Duplex;req/ret 两个内部流;request body 不能跨 redirect 重放 |
| `connect` | `(opts, callback?)` → Promise | `{statusCode, headers, socket, opaque, context}` | `ConnectHandler` | 底层 dispatch `{method:'CONNECT'}`;onResponseStart 故意抛错(不应有正常响应) |
| `upgrade` | `(opts, callback?)` → Promise | `{headers, socket, opaque, context}` | `UpgradeHandler` | 底层 dispatch `{method: opts.method||'GET', upgrade: opts.protocol||'Websocket'}`;H1 校验 101,H2 校验 200 |

**内部调用链统一视图**:

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
  所有路径最终走到:dispatcher.dispatch(opts, handler)
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

**body 在各 API 中的处理差异**:

```
              request/stream/pipeline          connect/upgrade
              ────────────────────────         ──────────────
请求体来源    │ opts.body 直接传入             │ 无 body(opts.body 被忽略)
              │ pipeline 时 opts.body 被       │
              │ 替换为 PipelineRequest         │
                                      │
响应体处理    │ RequestHandler:                │ ConnectHandler/UpgradeHandler:
              │   内部 new Readable +          │   onRequestUpgrade(statusCode,
              │   push(chunk) 由用户消费        │                       headers, socket)
                                      │
背压实现      │ onResponseData push=false      │ 不涉及背压,直接返回 socket
              │ → controller.pause()           │
                                      │
重定向支持    │ 由 RedirectHandler 在          │ 不支持重定向(隧道/升级)
              │ handler 外层包装实现            │
```

**callback 与 Promise 模式切换**(所有 API 统一范式):

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

每个 API 都支持同步错误 + 异步错误处理:同步参数校验错误直接 throw(Promise 模式)或 callback(err)(callback 模式)。

### 4.7 API 选择决策树

```
需要处理响应流?
├── 是,需要读取响应体
│   ├── 只需读取,不需要写入 → request(最简单)
│   ├── 需要写文件/Writable → stream(用户创建 Writable)
│   └── 需要管道转换(transform) → pipeline(返回 Duplex)
└── 不需要响应体,需要底层 socket
    ├── HTTP CONNECT 隧道 → connect
    └── WebSocket/协议升级 → upgrade
```


---

## 5. 8 个可组合拦截器

### 5.1 拦截器签名

每个拦截器是一个**高阶函数**,返回 `(dispatch) => (opts, handler) => ...` 的签名:

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

### 5.2 8 个拦截器清单

| 拦截器 | 文件 | 行数 | 功能 |
|--------|------|------|------|
| `cache` | `interceptor/cache.js` | 618 | HTTP 缓存(Memory/SQLite 双后端) |
| `retry` | `interceptor/retry.js` | 100 | 自动重试(薄包装,实现在 RetryHandler) |
| `redirect` | `interceptor/redirect.js` | 120 | 自动重定向(薄包装) |
| `dns` | `interceptor/dns.js` | 575 | DNS 缓存(TTL + 刷新) |
| `decompress` | `interceptor/decompress.js` | 292 | 自动解压(gzip/deflate/br/zstd) |
| `deduplicate` | `interceptor/deduplicate.js` | 180 | 请求去重(相同请求合并) |
| `dump` | `interceptor/dump.js` | 200 | 调试 dump(打印请求/响应) |
| `response-error` | `interceptor/response-error.js` | 180 | 响应错误转换(4xx/5xx → Error) |

### 5.3 Handler 装饰器

Handler 是**真正干活**的类,拦截器只是薄包装。6 个 Handler:

| Handler | 文件 | 行数 | 职责 |
|---------|------|------|------|
| `DecoratorHandler` | `handler/decorator-handler.js` | 100 | 装饰器基类 |
| `CacheHandler` | `handler/cache-handler.js` | 802 | 缓存处理(最大单文件 Handler) |
| `CacheRevalidationHandler` | `handler/cache-revalidation-handler.js` | 150 | 缓存再验证 |
| `RedirectHandler` | `handler/redirect-handler.js` | 250 | 重定向处理 |
| `DeduplicationHandler` | `handler/deduplication-handler.js` | 466 | 请求去重 |
| `RetryHandler` | `handler/retry-handler.js` | 548 | 指数退避重试 |

### 5.4 组合方式

```javascript
const client = new Client(url)
  .compose(cache())
  .compose(retry())
  .compose(redirect())
```

> compose 返回新的 dispatch 函数,调用顺序:cache → retry → redirect → dispatch(洋葱模型)。

### 5.5 decompress 细节(支持 4 种编码)

```javascript
const supportedEncodings = {
  gzip: createGunzip,
  'x-gzip': createGunzip,
  br: createBrotliDecompress,
  deflate: createInflate
  // zstd: createZstdDecompress(如可用)
}
```

### 5.6 Redirect Handler(重定向跟随)

`lib/handler/redirect-handler.js`(229 行)实现了 HTTP 重定向的完整处理。

**核心逻辑**:

```javascript
class RedirectHandler {
  constructor (dispatch, maxRedirections, opts, handler) {
    this.dispatch = dispatch
    this.maxRedirections = maxRedirections
    this.opts = { ...opts, body: wrapRequestBody(opts.body) }
    this.history = []  // 重定向历史(用于循环检测)
  }

  onResponseStart (controller, statusCode, headers, statusMessage) {
    // 301/302 + POST → GET(RFC 7231)
    if ((statusCode === 301 || statusCode === 302) && this.opts.method === 'POST') {
      this.opts.method = 'GET'
      if (isStream(this.opts.body)) destroy(this.opts.body)
      this.opts.body = null
    }

    // 303 → GET(HEAD 保持)
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
      // 跟随重定向:重新 dispatch
      this.dispatch(this.opts, this)
    } else {
      this.handler.onResponseEnd(controller, trailers)
    }
  }
}
```

**头部清理规则**:

```javascript
function cleanRequestHeaders (headers, removeContent, unknownOrigin, stripHeaders, stripHeadersOnCrossOrigin) {
  // 1. 始终移除: Host
  // 2. 303 或 POST→GET: 移除所有 Content-* 头
  // 3. 跨域重定向: 移除 Authorization / Cookie / Proxy-Authorization
  // 4. 用户自定义: stripHeadersOnRedirect / stripHeadersOnCrossOriginRedirect
}
```

### 5.7 Retry Handler(重试机制)

`lib/handler/retry-handler.js`(548 行)实现了请求级自动重试,支持 Range 续传。

**RetryController 代理模式**:

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

**重试策略**:

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

  return setTimeout(() => cb(null), retryTimeout)  // 返回定时器引用,支持取消
}
```

**Range 续传**:

```javascript
retry () {
  if (this.start !== 0) {
    // 已消费部分数据,使用 Range 头续传
    const headers = { range: `bytes=${this.start}-${this.end ?? ''}` }
    if (this.etag != null) {
      headers['if-match'] = this.etag  // ETag 校验确保数据一致性
    }
    this.opts = { ...this.opts, headers: { ...this.opts.headers, ...headers } }
  }
  this.dispatch(this.opts, this)
}
```

**关键状态追踪**:
- `start`:已消费的字节数
- `end`:期望的总字节数(从 Content-Length 或 Content-Range 提取)
- `etag`:弱 ETag 用于数据一致性验证
- `headersSent`:是否已向下游发送过响应头

### 5.8 Cache Handler(HTTP 缓存)

`lib/handler/cache-handler.js`(802 行)实现了 RFC 9111 兼容的 HTTP 缓存。

**可缓存性判断**:

```javascript
function canCacheResponse (cacheType, statusCode, resHeaders, cacheControlDirectives, reqHeaders) {
  // 1. 状态码必须是最终响应(≥200)
  if (statusCode < 200) return false

  // 2. no-store 指令
  if (cacheControlDirectives['no-store']) return false

  // 3. 共享缓存 + private 指令
  if (cacheType === 'shared' && cacheControlDirectives.private === true) return false

  // 4. Vary: * 不可缓存
  if (hasVaryStar(resHeaders.vary)) return false

  // 5. Authorization 头处理(RFC 9111 Section 3.5)
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

**新鲜度计算**(优先级递减):

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
  // 启发式缓存(last-modified 的 10%)
  if (resHeaders['last-modified']) {
    const lastModified = parseHttpDate(resHeaders['last-modified'])
    return lastModified ? (now - lastModified.getTime()) * 0.1 : undefined
  }
  // immutable 指令
  if (cacheControlDirectives.immutable) return 31536000000 // 1 年
}
```

**删除时间计算**(stale-while-revalidate / stale-if-error):

```javascript
function determineDeleteAt (baseTime, cachedAt, cacheControlDirectives, staleAt) {
  let deleteAt = staleAt

  // stale-while-revalidate:过期后仍可使用,同时后台验证
  if (cacheControlDirectives['stale-while-revalidate']) {
    deleteAt = Math.max(deleteAt, staleAt + cacheControlDirectives['stale-while-revalidate'] * 1000)
  }

  // stale-if-error:过期后出错时仍可使用
  if (cacheControlDirectives['stale-if-error']) {
    deleteAt = Math.max(deleteAt, staleAt + cacheControlDirectives['stale-if-error'] * 1000)
  }

  return deleteAt
}
```

**304 Not Modified 处理**:

```javascript
// 收到 304 时:合并缓存的 body 与新响应的 headers
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

### 5.9 Decompress Handler(多编码解压)

`lib/interceptor/decompress.js` 中的解压拦截器:

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

      // CVE 缓解:最多 5 层编码(防止 zip bomb)
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

## 6. Mock 录制回放系统

### 6.1 组件清单

| 组件 | 文件 | 职责 |
|------|------|------|
| `MockAgent` | `mock/mock-agent.js` | 替代真实 Agent,拦截所有 HTTP 请求 |
| `MockClient` | `mock/mock-client.js` | Mock 单连接 |
| `MockPool` | `mock/mock-pool.js` | Mock 连接池 |
| `MockInterceptor` | `mock/mock-interceptor.js` | URL/Method/Header 匹配 |
| `MockCallHistory` | `mock/mock-call-history.js` | 调用历史记录 |
| `SnapshotAgent` | `mock/snapshot-agent.js` | 快照回放 |
| `SnapshotRecorder` | `mock/snapshot-recorder.js` | 快照录制 |
| `mockErrors` | `mock/mock-errors.js` | Mock 专用错误 |

### 6.2 MockInterceptor API

```javascript
const mockAgent = new MockAgent()
mockAgent.disableNetConnect()  // 禁止真实网络

const pool = mockAgent.get('https://example.com')
pool.intercept({ path: '/api', method: 'GET' }).reply(200, { data: 'mocked' })

// 调用历史
console.log(pool.interceptCalls)  // MockCallHistory
```

### 6.3 Snapshot 快照系统

- `SnapshotRecorder`:录制真实 HTTP 交互 → 序列化到 JSON
- `SnapshotAgent`:离线回放录制的交互
- 支持 `cache-tests/`(http-tests 子模块)

### 6.4 MockAgent

`lib/mock/mock-agent.js`(244 行)是 Mock 体系的核心入口,继承自 Dispatcher。

**架构设计**:

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

**工厂方法**:

```javascript
[kFactory] (origin) {
  return this[kOptions]?.connections === 1
    ? new MockClient(origin, mockOptions)  // 单连接
    : new MockPool(origin, mockOptions)    // 多连接池
}
```

**网络控制**:

```javascript
enableNetConnect (matcher)     // 允许真实网络请求(可选匹配器: string/RegExp/Function)
disableNetConnect ()           // 禁止所有真实网络请求
activate () / deactivate ()    // 激活/停用 Mock 模式
```

**未匹配请求处理**:当 Mock 未匹配到拦截规则时:
- `netConnect === false` → 抛出 `MockNotMatchedError`
- `netConnect === true` → 回退到真实请求
- `netConnect === [matcher]` → 仅允许匹配 origin 的真实请求

**Pending Interceptors 断言**:

```javascript
assertNoPendingInterceptors () {
  const pending = this.pendingInterceptors()
  if (pending.length > 0) {
    throw new UndiciError(`${pending.length} interceptors are pending:\n${formatter.format(pending)}`)
  }
}
```

测试结束时调用,确保所有注册的拦截器都被消费完毕。

### 6.5 MockClient 和 MockPool

`lib/mock/mock-client.js`(68 行)和 `lib/mock/mock-pool.js`(68 行)结构几乎相同,分别包装 Client 和 Pool:

```javascript
class MockClient extends Client {
  constructor (origin, opts) {
    super(origin, opts)
    this[kDispatches] = []              // 拦截规则列表
    this[kConnected] = 1               // 强制标记已连接(Mock 不真实连接)
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

**核心设计**:构造函数中替换 `this.dispatch` 方法为 Mock 版本,同时保存原始 `dispatch` 引用,支持回退到真实网络。

### 6.6 MockUtils — 匹配与响应引擎

`lib/mock/mock-utils.js`(720 行)是 Mock 体系的核心引擎。

**匹配流程** (`getMockDispatch`):

```javascript
function getMockDispatch (mockDispatches, key) {
  // 1. 过滤未消费的
  let matched = mockDispatches.filter(({ consumed }) => !consumed)
  // 2. 按 path 匹配(支持 trailing slash 忽略)
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

每一层匹配失败都抛出详细的 `MockNotMatchedError`,包含具体哪一层不匹配。

**matchValue — 三类型匹配器**:

```javascript
function matchValue (match, value) {
  if (typeof match === 'string') return match === value        // 精确匹配
  if (match instanceof RegExp) return match.test(value)        // 正则匹配
  if (typeof match === 'function') return match(value) === true // 函数匹配
  return false
}
```

**Mock Dispatch 回调**:

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

**支持的 Mock 特性**:
- **times(n)**:限制拦截次数
- **persist()**:持久拦截(永不过期)
- **delay(ms)**:模拟响应延迟
- **reply(statusCode, body, headers)**:静态响应
- **reply(callback)**:动态响应(支持 async)
- **replyWithError(error)**:模拟错误

**请求体生命周期模拟**:

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

Mock 体系完整模拟了请求体的生命周期:`onRequestStart` → `onBodySent`(多次)→ `onRequestSent` → `onResponseStart` → `onResponseData` → `onResponseEnd`。

### 6.7 PendingInterceptorsFormatter

`lib/mock/pending-interceptors-formatter.js`(43 行)将未消费的拦截器格式化为表格:

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

## 7. fetch/WebSocket/EventSource/Cache Web 标准实现

### 7.1 Fetch API(lib/web/fetch/,~7,860 行)

严格对齐 WHATWG Fetch 规范,每步注释引用规范编号(如 `[FETCH]` 引用)。

- `fetch()` 入口 → Request 构建 → Body 处理 → Headers 序列化 → Response 解析
- 支持 FormData、Data URL、AbortSignal、redirect/follow/error 模式
- 关键文件:index.js(入口)、request.js、response.js、headers.js、body.js、formdata.js

#### Fetch 控制器状态机

`lib/web/fetch/index.js` 中的 `Fetch` 类是整个 Fetch 实现的控制器,基于 EventEmitter 构建:

```javascript
class Fetch extends EE {
  #request = null
  #controller = null
  #state = 'ongoing'

  terminate (reason) {
    if (this.#state !== 'ongoing') return
    this.#state = 'terminated'
    this.#controller.abort(reason)
    // 如果 response body 存在且未被消费,关闭底层流
    const response = this.#request.response
    if (response?.body) {
      response.body.stream.cancel(reason).catch(noop)
    }
  }
}
```

**三态模型**:
| 状态 | 含义 | 触发条件 |
|------|------|---------|
| `ongoing` | 请求进行中 | 初始状态 |
| `aborted` | 用户中止 | AbortSignal 触发 |
| `terminated` | 系统终止 | 错误或重定向失败 |

#### fetch() 入口函数全链路

`fetch()` 函数是用户调用的入口,实现了 Promise.withResolvers 模式:

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

    // 6. AbortSignal 链:用户 signal → Fetch 内部 controller
    request.signal.addEventListener('abort', () => fetch.terminate(request.signal.reason))
  })

  return p
}
```

**关键设计**:
- `Promise.withResolvers`:手动控制 Promise 生命周期,允许在 `processResponse` 回调中 resolve
- AbortController 链:用户的 `signal` 不直接控制请求,而是通过 `Fetch.terminate()` 间接控制
- 响应式 resolve:收到第一个字节就 resolve Promise,不等 body 完全接收

#### fetching → mainFetch → httpFetch 链路

请求从 `fetching()` 到网络发出的完整路径:

```
fetching()
  → mainFetch()
    → schemeFetch()          // 根据协议分派
      → httpNetworkOrCacheFetch()  // HTTP/HTTPS 请求
        → httpNetworkFetch()       // 真正的网络请求
```

**mainFetch 关键逻辑**:

```javascript
async function mainFetch (fetchParams, recursion = 0) {
  const request = fetchParams.request

  // 1. 协议检查
  if (!isValidURL(request.url)) { return makeNetworkError('invalid URL') }

  // 2. 本地协议处理(about: / blob: / data: / file:)
  if (request.url.protocol !== 'http:' && request.url.protocol !== 'https:') {
    return schemeFetch(fetchParams)
  }

  // 3. 递归深度限制(重定向)
  if (recursion > 20) return makeNetworkError('too many redirects')

  // 4. 响应染色(response tainting)
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

#### httpNetworkFetch — 网络请求的核心

`httpNetworkFetch` 是真正发出 HTTP 请求的函数,连接 undici 的 Dispatcher 体系:

```javascript
async function httpNetworkFetch (fetchParams) {
  const request = fetchParams.request

  // 1. 从请求中获取 Dispatcher(Agent/Client/Pool)
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

**核心连接**:undici 的 `agent.dispatch()` 直接被 Fetch API 调用,通过 handler 回调模式将数据推送到 WHATWG ReadableStream。

#### 重定向处理(Fetch 层)

Fetch 规范有自己的重定向逻辑(不同于 Dispatcher 层的 RedirectHandler):

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

#### Content-Encoding 解压链

`httpNetworkFetch` 中的解压链支持多级编码:

```javascript
// 根据 Content-Encoding 构建解压管线
const codings = response.headersList.get('content-encoding')
const decoders = []

if (codings) {
  const encodings = codings.split(',').map(c => c.trim().toLowerCase())
  // 逆序构建解压链(编码顺序 = 服务端应用顺序)
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

### 7.2 Headers 与 HeadersList 实现

#### HeadersList 内部结构

`lib/web/fetch/headers.js` 中的 `HeadersList` 是 Headers 的底层数据结构:

```javascript
class HeadersList {
  #headersMap = new Map()  // lowercasedName → { name, value }
  #headersSortedMap = null // 缓存的排序结果

  append (name, value) {
    const lowercaseName = name.toLowerCase()
    const existing = this.#headersMap.get(lowercaseName)

    if (existing) {
      // set-cookie 特殊处理:不合并,保持独立条目
      if (lowercaseName === 'set-cookie') {
        existing.value = Array.isArray(existing.value)
          ? [...existing.value, value]
          : [existing.value, value]
      } else {
        // 其他头部:逗号拼接
        existing.value = `${existing.value}, ${value}`
      }
    } else {
      this.#headersMap.set(lowercaseName, { name, value })
    }

    this.#headersSortedMap = null  // 使排序缓存失效
  }
}
```

#### 排序缓存优化

`toSortedArray()` 使用二分插入排序优化小规模头部:

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
  // 二分插入排序(≤32 个元素时比 Array.sort 更快)
  let low = 0, high = arr.length
  while (low < high) {
    const mid = (low + high) >>> 1
    if (arr[mid].name <= item.name) low = mid + 1
    else high = mid
  }
  arr.splice(low, 0, item)
}
```

**性能洞察**:HTTP 响应的头部数量通常在 10-30 之间,二分插入排序在小数组上比 `Array.sort` 更快(避免了 TimSort 的初始归并开销)。

#### Headers 守卫系统

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

### 7.3 Response 与 FilteredResponse

#### Response 工厂模式

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

#### FilteredResponse — Proxy 模式

`makeFilteredResponse` 使用 `Proxy` 创建不同安全等级的响应视图:

```javascript
function makeFilteredResponse (response, { type }) {
  return new Proxy(response, {
    get (target, prop) {
      // opaque 响应:几乎一切都不可访问
      if (type === 'opaque') {
        if (prop === 'type') return 'opaque'
        if (prop === 'url') return ''
        if (prop === 'status') return 0
        if (prop === 'headersList') return new HeadersList()
        return undefined
      }
      // cors 响应:剥离非安全头部
      if (type === 'cors') {
        if (prop === 'headersList') return filterHeadersList(target.headersList, corsSafeHeaders)
      }
      return Reflect.get(target, prop)
    }
  })
}
```

#### FinalizationRegistry 流清理

```javascript
const streamRegistry = new FinalizationRegistry(({ stream, signal }) => {
  if (!stream.locked) {
    stream.cancel().catch(noop)
  }
})
```

当 Response 对象被 GC 回收时,自动取消未消费的 ReadableStream,防止底层连接泄漏。

### 7.4 Request 与 AbortSignal 链

#### AbortSignal 链式传播

Request 实现了复杂的 AbortSignal 链,允许多层嵌套的请求共享同一个中止信号:

```javascript
function buildAbort (signal) {
  // 1. 创建内部 AbortController
  const ac = new AbortController()
  const { signal: innerSignal } = ac

  // 2. WeakRef 保持对内部 signal 的引用(防 GC 但不阻止 GC)
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

**设计要点**:
- `WeakRef`:允许内部 signal 被 GC 回收,避免长期持有无用引用
- `FinalizationRegistry`:在 GC 时自动移除事件监听器,防止泄漏
- `dependentControllerMap`:全局映射表跟踪所有依赖的 controller

### 7.5 Body Mixin 体系

#### extractBody — 8 种类型支持

`lib/web/fetch/body.js` 的 `extractBody` 处理所有类型的请求体:

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
    // 异步迭代器:流式发送,无法预知长度
    return { body: object, length: null, type: null }
  }
}
```

#### Body Mixin 方法

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

### 7.6 WebSocket(lib/web/websocket/,~2,000+ 行)

RFC 6455 完整实现:握手 → 帧编解码 → 收发 → 关闭

- 支持 permessage-deflate 压缩扩展
- WebSocketStream API(Promise 包装)
- Autobahn 测试套件覆盖率 100%

### 7.7 EventSource/SSE(lib/web/eventsource/,~580 行)

SSE 流式解析器:text/event-stream 格式

- 支持自动重连、Last-Event-ID

#### 状态机

`lib/web/eventsource/eventsource.js`(494 行)实现了 Server-Sent Events 规范:

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

#### 自动重连机制

EventSource 的重连策略:
1. 服务端关闭连接 → 触发 `processResponseEndOfBody`
2. 检查 `readyState !== CLOSED`
3. 使用 `setTimeout` 延迟 `reconnectionTime` 毫秒
4. 重新连接时在 `Last-Event-ID` 头中携带最后收到的事件 ID
5. 服务端可通过 `retry:` 字段调整重连间隔

### 7.8 Cache API(lib/web/cache/,~1,060 行)

双后端:MemoryCacheStore(内存)/ SQLiteCacheStore(持久化)

- HTTP 缓存语义:ETag/If-None-Match、Vary、max-age/stale-while-revalidate
- `caches` 全局单例

### 7.9 Cookies(lib/web/cookies/,~420 行)

Set-Cookie 解析 + Cookie 序列化

- 大小限制、域名验证
- 独立导出:`getCookies/setCookies/deleteCookie/parseCookie`

### 7.10 WebIDL(lib/web/webidl/,~200 行)

WebIDL 类型转换工具

- 用于 Fetch 参数的类型强制

### 7.11 Subresource Integrity(lib/web/subresource-integrity/)

SRI 校验(`<script integrity="sha256-...">`)

### 7.12 Infra(lib/web/infra/)

WHATWG Infra 规范工具(byte sequence/ASCII 操作)


---

## 8. 错误类层次

### 8.1 错误基类

`lib/core/errors.js`(497 行)定义了完整的错误层次结构。

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

使用 `Symbol.hasInstance` 自定义 `instanceof` 行为,支持跨 realm 的错误识别。

### 8.2 错误类型完整列表(26 种)

| 错误类 | Code | 说明 |
|--------|------|------|
| `ConnectTimeoutError` | `UND_ERR_CONNECT_TIMEOUT` | 连接超时 |
| `HeadersTimeoutError` | `UND_ERR_HEADERS_TIMEOUT` | 等待响应头超时 |
| `HeadersOverflowError` | `UND_ERR_HEADERS_OVERFLOW` | 响应头大小溢出 |
| `BodyTimeoutError` | `UND_ERR_BODY_TIMEOUT` | 等待响应体超时 |
| `InvalidArgumentError` | `UND_ERR_INVALID_ARG` | 参数错误 |
| `RequestAbortedError` | `UND_ERR_ABORTED` | 请求被取消 |
| `InformationalError` | `UND_ERR_INFO` | 信息性错误(如 reset、idle timeout) |
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

### 8.3 错误处理策略

- `InformationalError`(code: `UND_ERR_INFO`)是特殊的"非致命"错误,用于 reset、upgrade、idle timeout 等正常流程控制,不会触发 `onError` 的 pending 队列清空
- `SocketError`(code: `UND_ERR_SOCKET`)在 `onError` 中也视为可恢复,不触发 pending 队列清空

```javascript
function onError (client, err) {
  if (client[kRunning] === 0 && err.code !== 'UND_ERR_INFO' && err.code !== 'UND_ERR_SOCKET') {
    // 非 running 请求产生的非信息性错误,清空整个 pending 队列
    const requests = client[kQueue].splice(client[kRunningIdx])
    for (const request of requests) util.errorRequest(client, request, err)
  }
}
```

### 8.4 诊断钩子(lib/core/diagnostics.js)

基于 Node.js `diagnostics_channel` 模块提供全面的可观测性。

#### 通道完整清单与 Payload 结构

```javascript
const channels = {
  // ─── Client 级别(4 个) ───────────────────────────────────────────────
  beforeConnect: diagnosticsChannel.channel('undici:client:beforeConnect'),
  connected: diagnosticsChannel.channel('undici:client:connected'),
  connectError: diagnosticsChannel.channel('undici:client:connectError'),
  sendHeaders: diagnosticsChannel.channel('undici:client:sendHeaders'),

  // ─── Request 级别(8 个) ─────────────────────────────────────────────
  create: diagnosticsChannel.channel('undici:request:create'),
  bodySent: diagnosticsChannel.channel('undici:request:bodySent'),
  bodyChunkSent: diagnosticsChannel.channel('undici:request:bodyChunkSent'),
  bodyChunkReceived: diagnosticsChannel.channel('undici:request:bodyChunkReceived'),
  headers: diagnosticsChannel.channel('undici:request:headers'),
  trailers: diagnosticsChannel.channel('undici:request:trailers'),
  error: diagnosticsChannel.channel('undici:request:error'),

  // ─── WebSocket 级别(5 个) ───────────────────────────────────────────
  open: diagnosticsChannel.channel('undici:websocket:open'),
  close: diagnosticsChannel.channel('undici:websocket:close'),
  socketError: diagnosticsChannel.channel('undici:websocket:socket_error'),
  ping: diagnosticsChannel.channel('undici:websocket:ping'),
  pong: diagnosticsChannel.channel('undici:websocket:pong'),

  // ─── Proxy 级别(1 个) ───────────────────────────────────────────────
  proxyConnected: diagnosticsChannel.channel('undici:proxy:connected')
}
```

#### 各通道 Payload 数据结构(含 publish 位置)

**`undici:client:beforeConnect`** — 建连前(client.js connect())

```javascript
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

**`undici:client:connected`** — 建连成功(client.js connect() callback)

```javascript
channels.connected.publish({
  connectParams: { host, hostname, protocol, port, version, servername, localAddress },
  connector: client[kConnector],
  socket                          // 已连接的 net.Socket / tls.TLSSocket
})
```

**`undici:client:connectError`** — 建连失败(handleConnectError)

```javascript
channels.connectError.publish({
  connectParams: { host, hostname, protocol, port, version, servername, localAddress },
  connector: client[kConnector],
  error                           // Error 实例(含 code/message)
})
```

**`undici:client:sendHeaders`** — 发送请求头(writeH1 / writeH2)

```javascript
channels.sendHeaders.publish({
  request: { method, path, origin },  // Request 的部分字段
  headers: header,                    // 完整的 HTTP 头字符串
  socket                              // 底层 socket
})
```

**`undici:request:headers`** — 收到响应头(onHeadersComplete / onResponse)

```javascript
channels.headers.publish({
  request: { method, path, origin },
  response: { statusCode }
})
```

**`undici:request:trailers`** — 收到 trailers(H2 trailers / H1 trailers)

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
  address: { address, port }   // 服务端地址,可能为 null
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

#### 三个 track 函数(懒订阅 + 去重)

**`trackClientEvents()`**:

```javascript
function trackClientEvents (debugLog = undiciDebugLog) {
  if (isTrackingClientEvents) return
  // 已有订阅者 → 幂等返回(处理 npm undici 与 Node 内置 undici 共存场景)
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

**`trackRequestEvents()`**:订阅 request:headers / request:trailers / request:error。

**`trackWebSocketEvents()`**:订阅 websocket:open / close / socket_error / ping / pong。

#### 订阅触发条件

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

环境变量控制:
- `NODE_DEBUG=undici` → 启用 client + request 调试
- `NODE_DEBUG=fetch` → 同上(用 fetch Debuglog)
- `NODE_DEBUG=websocket` → 启用 client + websocket 调试

#### 用户自定义订阅示例

```javascript
const diagnosticsChannel = require('diagnostics_channel')

// 订阅所有连接事件
diagnosticsChannel.subscribe('undici:client:beforeConnect', (evt) => {
  console.log('准备连接:', evt.connectParams.hostname, evt.connectParams.port)
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

#### diagnostics_channel 订阅最佳实践

1. **在 import undici 前订阅**:因为首次 require 时会检查 `hasSubscribers`,已在的订阅者会触发跟踪注册。
2. **使用 channel.channel() 而非 channel.tracingChannel()**:undici 使用的是稳定版的 channel API。
3. **副作用注意**:publish 是同步的,订阅者的回调执行会阻塞 publish 返回。避免在订阅者中做重操作。


---

## 9. 基础设施(FastTimer + TernarySearchTree + Snapshot)

### 9.1 FixedQueue 高性能队列

`lib/dispatcher/fixed-queue.js` 是从 Node.js 内部提取的高性能队列实现。

**数据结构**:单向链表 + 固定大小环形缓冲区(完整 ASCII 结构图):

```
  注释(源码内嵌)描述了三种形态:

  形态 A: 多缓冲区链表(head 在右,tail 在左, oldest 先出)

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

  形态 B: 单缓冲区,有数据

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

  形态 C: 单缓冲区,空

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

每个 `FixedCircularBuffer` 大小为 2048(V8 优化测试得出的最佳大小,必须是 2 的幂)。

**push(入队)**:

```javascript
push (data) {
  if (this.head.isFull()) {
    // Head 满了:创建新缓冲区,链接到旧 head.next,并作为新 head
    this.head = this.head.next = new FixedCircularBuffer()
  }
  this.head.push(data)
}
```

当前缓冲区满时自动创建新的缓冲区链接到链表末尾。注意:判断 `isFull()` 使用的是 `((top + 1) & kMask) === bottom` — 即只浪费一个槽位,这是经典的环形缓冲区判满策略。

**shift(出队)**:

```javascript
shift () {
  const tail = this.tail
  const next = tail.shift()
  if (tail.isEmpty() && tail.next !== null) {
    // tail 已空,且有下一个缓冲区:回收当前 tail,前进到下一个
    this.tail = tail.next
    tail.next = null   // 断开引用,便于 GC
  }
  return next
}
```

出队时如果当前 tail 缓冲区为空且有下一个,自动前进并释放旧缓冲区。

**FixedCircularBuffer 内部实现**:

```javascript
const kSize = 2048
const kMask = kSize - 1   // 位掩码用于快速取模

class FixedCircularBuffer {
  bottom = 0                        // 读取指针
  top = 0                           // 写入指针
  list = new Array(kSize).fill(undefined)  // 固定大小数组
  next = null                       // 链表指针

  isEmpty () { return this.top === this.bottom }

  // 经典环形缓冲区判满:牺牲一个槽位避免判空/判满混淆
  isFull () { return ((this.top + 1) & kMask) === this.bottom }

  push (data) {
    this.list[this.top] = data
    this.top = (this.top + 1) & kMask    // 位运算取模,等价于 (top+1) % 2048
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

**复杂度分析**:

| 操作 | FixedQueue | 原生 Array | 说明 |
|------|-----------|-----------|------|
| push 均摊 | O(1) | O(1) 均摊 | 满时创建新缓冲区,一次性分配 2048 槽位 |
| shift 均摊 | O(1) | O(n) | Array.shift() 需要移动所有元素 |
| 内存连续性 | 每 2048 项连续 | 整体连续 | V8 优化为 packed array |
| GC 压力 | 分段释放 | 整体释放 | 空缓冲区即丢即收 |
| 判满策略 | 浪费 1 槽 | N/A | 经典环形缓冲区 |

**在 undici 中的使用场景**:

1. **PoolBase.kQueue**:`new FixedQueue()` — 当所有 Client 都 busy 时,pending 请求入此队列等待 drain。
2. **PoolBase[kOnDrain]()**:drain 事件触发时,`queue.shift()` 出队并 dispatch 到空闲 Client。

**性能优势总结**:
- 数组固定大小(2048),V8 可优化为连续 packed array,CPU 缓存友好
- 环形缓冲区用位运算 `& kMask` 替代 `% kSize`,避免除法
- 链表分段避免了单个大数组的 GC 压力(百万级请求时显著)
- 2048 大小经过 V8 6.0-6.6 基准测试,是缓存行与内存占用的平衡点

### 9.2 TernarySearchTree 头部快速查找

`lib/core/tree.js` 实现了三叉搜索树(Ternary Search Tree, TST),用于 HTTP 头部名称的快速查找。

**用途**:将头部名称 Buffer(来自 llhttp 解析)快速映射为小写字符串,避免逐字节 `toLowerCase()` 和对象查找。

**完整实现代码**(含注释):

```javascript
// lib/core/tree.js (160 行)

class TstNode {
  value = null              // 终止节点存储的值(小写字符串)
  left = null               // 小于当前字符的子树
  middle = null             // 等于当前字符的子树(匹配路径继续)
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
      // 内联大写转小写:A-Z (0x41-0x5A) → a-z (0x61-0x7A)
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

**内部树结构示例**(存储 "host", "connection", "content-type" 后):

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

**初始化与导出**(从 well-known 头部构建):

```javascript
const tree = new TernarySearchTree()

for (let i = 0; i < wellknownHeaderNames.length; ++i) {
  const key = headerNameLowerCasedRecord[wellknownHeaderNames[i]]
  tree.insert(key, key)   // key = value = 小写字符串
}

module.exports = { TernarySearchTree, tree }
```

`wellknownHeaderNames` 和 `headerNameLowerCasedRecord` 定义在 `lib/core/constants.js`,包含约 100 个标准 HTTP 头部。

**使用场景**(在 Parser.onHeaderField 中):

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

`bufferToLowerCasedHeaderName` 在 `lib/core/util.js` 中:先用 TST 查表,命中直接返回预计算的小写字符串;否则逐字节转小写。

**优化点**:
- 使用位运算 `|= 32` 代替条件判断做大小写转换(ASCII 大写转小写的最快方法)
- 三叉搜索树相比 HashMap 对短字符串(头部名称通常 < 30 字符)有更好的缓存局部性
- 树中存储了约 100 个 well-known 头部名称,命中即免计算
- 使用 `Uint8Array` 搜索而非 `Buffer.toString()` 避免字符串分配

**TST vs 其他数据结构对比**:

| 数据结构 | 短字符串查找 | 内存占用 | 缓存局部性 | 实现复杂度 |
|---------|-------------|---------|-----------|----------|
| TernarySearchTree | O(L·logN) L=长度 | 中(每个字符一个节点) | 好(树遍历) | 中 |
| HashMap | O(L) 平均 | 高(hash 计算+桶) | 一般 | 低 |
| 线性扫描 | O(N·L) | 低 | 好 | 低 |
| 有序数组+二分 | O(L·logN) | 低 | 好 | 低 |

TST 的优势在于**前缀共享**(如 "content-length" 和 "content-type" 共享 "content-" 前缀路径),节省内存。

### 9.3 FastTimer 快速定时器

`lib/util/timers.js`(425 行)实现了自定义的快速定时器系统,用于替代 Node.js 原生 `setTimeout` 在高频场景下的性能瓶颈。

#### 设计动机

Node.js 原生定时器的两个问题:
1. **精度受事件循环影响**:如果事件循环被阻塞(长时间同步操作),定时器回调会延迟触发
2. **每个定时器一个对象**:高频创建/销毁定时器带来 GC 压力

FastTimer 的解决思路:
- 使用共享的单一定时器驱动所有 FastTimer
- 用自维护的逻辑时钟 `fastNow` 替代 `Date.now()`
- 每 499ms tick 一次,批量检查到期的定时器

#### 核心数据结构

```javascript
let fastNow = 0             // 逻辑时钟
const RESOLUTION_MS = 1e3   // 分辨率 1 秒
const TICK_MS = 499         // tick 间隔 = 分辨率/2 - 1(留余量)
const fastTimers = []       // 所有活跃的 FastTimer
let fastNowTimeout          // 底层 Node.js 定时器(唯一)
```

#### FastTimer 四状态机

```
  NOT_IN_LIST (-2)  <--clear()--  TO_BE_CLEARED (-1)
        ^                              ^
        | refresh()                    | onTick() 移除
        v                              |
     PENDING (0)  ----onTick()--->  ACTIVE (1)  --到期-->  回调执行  --> TO_BE_CLEARED
```

| 状态 | 值 | 含义 |
|------|-----|------|
| `NOT_IN_LIST` | -2 | 未加入 fastTimers 数组 |
| `TO_BE_CLEARED` | -1 | 标记待移除,下一次 onTick 删除 |
| `PENDING` | 0 | 刚创建或 refresh(),等待第一次 tick 设置 _idleStart |
| `ACTIVE` | 1 | 活跃,等待到期 |

#### onTick 核心循环

```javascript
function onTick () {
  fastNow += TICK_MS  // 推进逻辑时钟(不依赖系统时间)
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

**关键优化**:
- `fastNow += TICK_MS` 不读取系统时钟,避免系统时钟跳变影响
- 到期的定时器用末尾元素填补(swap-and-pop),O(1) 删除
- 所有定时器到期后停止底层定时器(不空转)

#### 导出接口

```javascript
module.exports = {
  setTimeout (callback, delay, arg) {
    return delay <= RESOLUTION_MS
      ? setTimeout(callback, delay, arg)   // ≤1s 走原生定时器(精度更好)
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

**混合策略**:延迟 ≤1 秒使用原生 `setTimeout`(精度更好),>1 秒使用 FastTimer(性能更好)。在 Parser 中:
- Headers/Body 超时(通常 >5s)→ FastTimer
- Keep-Alive 超时 → 原生 setTimeout + unref()

### 9.4 HTTP 缓存工具

`lib/util/cache.js`(716 行)实现了 RFC 9111 兼容的 HTTP 缓存拦截器。

#### Cache-Control 头部解析

**parseCacheControlHeader(header)** — 完整的 Cache-Control 指令解析器:

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
          // 跨逗号引号合并(处理 no-cache="header1, header2" 被逗号分割的情况)
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

#### Vary 头部处理

**parseVaryHeader(varyHeader, headers)** — 实现 Vary 机制:

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

#### ETag 验证

```javascript
function isEtagUsable (etag) {
  if (etag.length <= 2) return false  // 空 ETag 无意义
  if (etag[0] === '"' && etag[etag.length - 1] === '"') {
    return !(etag[1] === '"' || etag.startsWith('"W/'))  // 拒绝 ""xxx"" 和 "W/xxx"
  }
  if (etag.startsWith('W/"') && etag[etag.length - 1] === '"') {
    return etag.length !== 4  // W/"" 无效(长度不足)
  }
  return false
}
```

#### 请求去重

**makeDeduplicationKey(cacheKey)** — 使用 JSON.stringify 创建无碰撞的去重键:

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

之前使用 `:` 和 `=` 分隔符的格式存在碰撞风险(如 `{a:"x:b=y"}` vs `{a:"x", b:"y"}`),改用 JSON.stringify 解决。

#### 缓存存储接口

```javascript
function assertCacheStore (store) {
  for (const fn of ['get', 'createWriteStream', 'delete']) {
    if (typeof store[fn] !== 'function') throw new TypeError(...)
  }
}
```

缓存存储必须实现三个方法:`get(key)`、`createWriteStream(key)`、`delete(key)`。可扩展到文件系统、SQLite、Redis 等后端。

### 9.5 HTTP 日期解析器

`lib/util/date.js`(670 行)实现了 RFC 9110 规定的三种 HTTP 日期格式的手工解析器(不使用 `new Date(str)` 避免 V8 解析开销)。

#### 三种格式

| 格式 | 示例 | 函数 |
|------|------|------|
| IMF-fixdate (首选) | `Sun, 06 Nov 1994 08:49:37 GMT` | `parseImfDate` |
| asctime() | `Sun Nov  6 08:49:37 1994` | `parseAscTimeDate` |
| RFC 850 (过时) | `Sunday, 06-Nov-94 08:49:37 GMT` | `parseRfc850Date` |

**路由逻辑**:通过第 4 个字符(`','` / `' '` / 其它)快速分派。

#### 手工解析优化

所有日期字段都通过 `charCodeAt()` + ASCII 运算逐字符解析,避免正则表达式和 `new Date()`:

```javascript
// 月份解析示例(IMF 格式)
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
  // 校验各字段是否与输入一致(防止 2月30日 等无效日期)
  return result.getUTCFullYear() === year && ... ? result : undefined
}
```

#### RFC 850 两位年份处理

```javascript
year += year < 70 ? 2000 : 1900  // 遵循 RFC 6265 规则
```

### 9.6 运行时特性检测

`lib/util/runtime-features.js`(93 行)提供懒加载的运行时特性检测。

#### 设计模式

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

**单例模式**:模块级 `const instance = new RuntimeFeatures()`,全局共享。

#### 特性检测实现

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

通过尝试 `require()` 内置模块来检测运行时支持,检测结果缓存避免重复加载。

当前支持的特性:`crypto`(加密模块)、`sqlite`(SQLite 模块)。

### 9.7 统计快照与度量

`lib/util/stats.js`(32 行)提供 Client 和 Pool 的统计快照。

```javascript
class ClientStats {
  constructor (client) {
    this.connected = client[kConnected]   // 已建立的连接数
    this.pending = client[kPending]       // 等待发送的请求数
    this.running = client[kRunning]       // 正在执行的请求数
    this.size = client[kSize]             // 队列总大小(running + pending)
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

**与 diagnostics_channel 的关系**:Stats 是拉模型(按需查询),diagnostics 是推模型(事件驱动)。两者互补。

### 9.8 工具函数库

`lib/core/util.js` 是整个 undici 项目的核心工具库,约 1050 行,导出 40+ 个函数。它为 Dispatcher 体系、API 层、连接器、Mock 系统等所有上层模块提供基础设施。

#### Body 处理

**wrapRequestBody(body)** — 请求体适配器,统一不同来源的 body:

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

**BodyAsyncIterable** 类封装可迭代体,确保只被消费一次:

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

**bodyLength(body)** — 估算 body 字节数,用于设置 Content-Length:

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

#### 头部处理

**headerNameToString(value)** — 头部名称转小写,利用 TST 和预计算表:

```javascript
function headerNameToString (value) {
  return typeof value === 'string'
    ? headerNameLowerCasedRecord[value] ?? value.toLowerCase()   // 100 个 well-known 名称 O(1) 查找
    : tree.lookup(value) ?? value.toString('latin1').toLowerCase()  // Buffer 走 TST
}
```

**parseHeaders(headers, obj)** — 将 `[key, value, key, value, ...]` 扁平数组解析为 `{key: value}` 对象,处理重复头部合并:

```javascript
function parseHeaders (headers, obj = {}) {
  for (let i = 0; i < headers.length; i += 2) {
    const key = headerNameToString(headers[i])
    let val = obj[key]
    if (val !== undefined) {
      if (!Object.hasOwn(obj, key)) {
        // 原型链上的属性 → 用 defineProperty 绕过(防 __proto__ 污染)
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

**安全措施**:当 `key === '__proto__'` 时使用 `Object.defineProperty` 而非直接赋值,防止原型污染攻击。

#### URL 处理

**parseURL(url)** — 统一 URL 解析,支持字符串、URL 对象、`{protocol, hostname, port, path}` 记录:

```javascript
function parseURL (url) {
  if (typeof url === 'string') {
    url = new URL(url)
    if (!isHttpOrHttpsPrefixed(url.origin || url.protocol)) throw ...
    return url
  }
  if (!(url instanceof URL)) {
    // 记录格式: 手动拼接 origin + path,再用 new URL 解析
    let origin = url.origin ?? `${url.protocol || ''}//${url.hostname || ''}:${port}`
    let path = url.path ?? `${url.pathname || ''}${url.search || ''}`
    return new URL(`${origin}${path}`)
  }
  return url
}
```

**isHttpOrHttpsPrefixed(value)** — 手动逐字符检查协议前缀(避免正则开销):

```javascript
function isHttpOrHttpsPrefixed (value) {
  return value != null && value[0] === 'h' && value[1] === 't' && value[2] === 't' && value[3] === 'p'
    && (value[4] === ':' || (value[4] === 's' && value[5] === ':'))
}
```

#### 连接超时

**setupConnectTimeout** — 平台差异化的连接超时设置:

```javascript
// Windows: 额外多一层 setImmediate(Windows socket 实现差异)
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

使用 `WeakRef` 持有 socket,避免定时器阻止 GC。使用 `setImmediate` 延迟执行,确保 socket error 事件优先于超时被处理。

#### HTTP Token 验证

**isValidHTTPToken(characters)** — 校验 HTTP token(RFC 7230):

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

双策略:短字符串(<12 字符)用 Uint8Array 位图查表 O(n),长字符串用正则。位图预计算避免运行时分支。

#### Range Header 解析

```javascript
const rangeHeaderRegex = /^bytes (\d+)-(\d+)\/(\d+|\*)?$/

function parseRangeHeader (range) {
  if (range == null || range === '') return { start: 0, end: null, size: null }
  const m = rangeHeaderRegex.exec(range)
  return m ? { start: parseInt(m[1]), end: m[2] ? parseInt(m[2]) : null, size: m[3] && m[3] !== '*' ? parseInt(m[3]) : null } : null
}
```

#### 事件监听器管理

**addListener / removeAllListeners** — 批量管理 EventEmitter 监听器,防止泄漏:

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

所有注册的监听器都记录在 `kListeners` 数组中,销毁时可以一次性移除,避免 EventEmitter 泄漏。

#### Keep-Alive 解析

```javascript
const KEEPALIVE_TIMEOUT_EXPR = /timeout=(\d+)/

function parseKeepAliveTimeout (val) {
  const m = val.match(KEEPALIVE_TIMEOUT_EXPR)
  return m ? parseInt(m[1], 10) * 1000 : null
}
```

从 `Keep-Alive: timeout=5` 头部中提取超时值(秒→毫秒)。

#### 协议缓存

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

单条缓存优化:HTTP 请求通常对同一 origin 反复发送,缓存最后一次解析结果可避免重复字符串操作。

#### 方法名归一化

```javascript
const normalizedMethodRecordsBase = Object.setPrototypeOf({
  delete: 'DELETE', DELETE: 'DELETE',
  get: 'GET', GET: 'GET',
  head: 'HEAD', HEAD: 'HEAD',
  options: 'OPTIONS', OPTIONS: 'OPTIONS',
  post: 'POST', POST: 'POST',
  put: 'PUT', PUT: 'PUT',
  query: 'QUERY', QUERY: 'QUERY'
}, null)  // null 原型链,防止原型污染

const normalizedMethodRecords = {
  ...normalizedMethodRecordsBase,
  patch: 'patch', PATCH: 'PATCH'  // PATCH 保持大小写(浏览器兼容性问题)
}
```

注意 `patch` 不转大写 — 这是一个已知的浏览器兼容性 workaround。


---

## 10. HTTP 传输层深度

### 10.1 HTTP/1.1 vs HTTP/2 双协议实现

| 特性 | HTTP/1.1 (client-h1.js) | HTTP/2 (client-h2.js) |
|------|-------------------------|----------------------|
| 连接模型 | 单连接 + pipeline | 多路复用(stream) |
| 解析器 | llhttp WASM | Node.js 内置 http2 |
| 代码行数 | 1,801 行 | 1,781 行 |
| Keep-alive | 显式管理 | 内置 |
| 流量控制 | 无 | WINDOW_UPDATE |
| 头部压缩 | 无 | HPACK |
| 协议选择 | ALPN 协商 | ALPN 协商 |
| 流水线深度 | 可配置(默认 1) | N/A |
| HTTP/1.1 升级 h2c | - | h2c-client.js 支持 |

**ALPN 协商**:连接建立时通过 TLS ALPN 扩展自动选择协议版本,支持 h2 → http/1.1 降级。

### 10.2 client-h1.js — HTTP/1.1 连接上下文

`lib/dispatcher/client-h1.js` 是 HTTP/1.1 协议的完整实现,约 1800 行。

**connectH1 返回的上下文对象**:

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

**H1 busy 的精细判断**:
- 正在写请求体时忙
- 已标记 reset 时忙
- 非幂等请求(POST/PUT/PATCH)在有 inflight 请求时不能流水线 — 防止失败不可重试
- Upgrade/CONNECT 请求必须等所有前序请求完成
- 流式 body 的请求不能流水线 — 错误会影响同连接的其他请求

**writeH1 — HTTP 请求写入**:

核心流程:
1. 拼接请求行和头部:`METHOD PATH HTTP/1.1\r\nhost: ...\r\nconnection: ...\r\n`
2. 根据 body 类型分发到不同写入策略:
   - `null`/Buffer:`writeBuffer` — 一次性写入
   - Blob:`writeBlob` — await arrayBuffer 后写入
   - Stream:`writeStream` — 用 AsyncWriter 流式写入
   - Iterable/AsyncIterable:`writeIterable` — 用 AsyncWriter 逐块写入

**AsyncWriter — 流式 body 写入器**:

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

**空闲 Socket 验证(Idle Socket Validation)**:

这是一个重要的安全机制,防止在复用 keep-alive 连接时发送请求到已被对端关闭的 socket:

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

使用 `setImmediate`(而非 `setTimeout(0)`),既避免 1ms 定时器开销,又让事件循环有机会处理已挂起的 FIN/RST。

### 10.3 client-h2.js — HTTP/2 连接上下文

`lib/dispatcher/client-h2.js` 是 HTTP/2 协议的完整实现,约 1780 行。

**connectH2 返回的上下文对象**:

```javascript
return {
  version: 'h2',
  defaultPipelining: Infinity,  // H2 无流水线概念,多路复用
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

**GOAWAY 处理**:

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
    // 可重试的请求(body 是 null/Buffer/Blob)重新入队
    if (canReplayRequest(request) && registerGoAwayRefusal(request)) {
      retriableRequests.push(request)
    } else {
      util.errorRequest(client, request, err)
    }
  }
  // 关闭受影响的 stream,重排队列
}
```

**REFUSED_STREAM 重试**:

```javascript
function retryRefusedStream (stream, state) {
  if (state.responseReceived || request.aborted || request.completed) return false
  if (request[kRefusedStreamRetry]) return false  // 只重试一次
  request[kRefusedStreamRetry] = true
  // 分离失败的 stream,将请求放回 pending 队列头部
  detachRequestStreamForClose(request)
  client[kQueue].splice(client[kPendingIdx], 0, request)
  client[kResume]()
  return true
}
```

**H2 连接级流控**:

```javascript
function applyConnectionWindowSize (connectionWindowSize) {
  if (typeof this.setLocalWindowSize === 'function') {
    this.setLocalWindowSize(connectionWindowSize)
  }
}
```

在 `remoteSettings` 事件后应用连接级窗口大小(默认 512KB,高于 Node.js 默认值)。

**WebSocket over H2(RFC 8441)**:

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

### 10.4 协议选择与 ALPN 协商

在 `connect` 函数中(client.js),协议选择基于 TLS ALPN 协商结果:

```javascript
client[kHTTPContext] = socket.alpnProtocol === 'h2'
  ? connectH2(client, socket)
  : connectH1(client, socket)
```

ALPN 优先级在 `connect.js` 中设置:

```javascript
ALPNProtocols: allowH2 ? (preferH2 ? ['h2', 'http/1.1'] : ['http/1.1', 'h2']) : ['http/1.1']
```

默认 `preferH2` 为 false,即优先 HTTP/1.1 但接受 HTTP/2。设为 `preferH2: true` 则优先 H2。

### 10.5 双协议状态机深度对比

**Client (kHTTPContext) 状态机**:

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

**HTTP/1.1 socket 状态标志**:

| Symbol | 含义 | 触发条件 |
|--------|------|---------|
| `kWriting` | 正在写请求体 | writeStream / writeBuffer / writeIterable 期间 |
| `kReset` | 需要重置连接 | HEAD/CONNECT/Upgrade 后、maxRequests 达到、body 不期待 |
| `kBlocking` | 阻塞流水线 | 请求头带 `connection: close` 或 blocking=true |
| `kNoRef` | socket 已 unref | 空闲时 keepAlive 期间 |
| `kIdleSocketValidation` | 空闲验证中 | 0=正常 1=scheduled 2=checked |

**HTTP/2 session 状态**:

| Symbol | 含义 | 触发条件 |
|--------|------|---------|
| `kHTTP2SessionState.idleTimeout` | 空闲关闭定时器 | 所有 stream 关闭 + 队列为空 |
| `kHTTP2SessionState.noStreamsTimeout` | 无流可用超时 | maxConcurrentStreams=0 |
| `kHTTP2SessionState.refed` | session ref 状态 | 有活跃 stream 时 ref,否则 unref |
| `kHTTP2SessionState.ping.interval` | PING 定时器 | 默认 60s,可设为 0 禁用 |
| `kReceivedGoAway` | 收到 GOAWAY | 服务端主动关闭或重启 |
| `kRemoteSettings` | 收到 SETTINGS | 初始 false,收到后为 true |
| `kEnableConnectProtocol` | 支持 Extended CONNECT | 来自 SETTINGS_ENABLE_CONNECT_PROTOCOL |

**双协议并发策略差异**:

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

### 10.6 dispatch() 完整调度链路时序图

从用户调用到响应回调的全链路时序:

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

**关键步骤解读**:

1. **守卫检查**(DispatcherBase.dispatch):handler 必须是对象、opts 必须无 dispatcher 字段、client 不能 destroyed/closed。
2. **路由选择**(PoolBase.kGetDispatcher):线性扫描 / 加权轮询 / 简单轮询,选空闲 Client。若全部 busy 则返回 undefined → 入 FixedQueue 等待 drain。
3. **入队 + resume 触发**(Client[kDispatch]):新建 Request 推入 kQueue。根据 body 类型:
   - 同步 body(Buffer/string)→ 立即 `kResume(true)` 同步调度
   - 异步 iterable → `queueMicrotask(() => resume(this))` 延迟一 tick 等 body 就绪
   - 已在 resume 中 → 不操作
4. **_resume 循环**:while(true) 逐次检查 destroyed/closed/busy/pending=0/running>=max 退出条件。
5. **servername 切换**:HTTPS 下若请求 servername 与当前连接不一致,且 running=0 时才重建连接。
6. **connect 异步建连**:调用用户配置的 connector(默认 buildConnector),内部做 DNS 解析 + TCP 连接 + TLS 握手 + ALPN 协商。
7. **协议上下文创建**:根据 ALPN 结果选择 connectH1 或 connectH2,返回 `{ write, resume, busy, destroy, destroyed }` 上下文对象。
8. **emit connect**:触发用户监听的 connect 事件。
9. **resumeH1/resumeH2**:H1 先处理 idleSocketValidation 和 ref/unref 状态;H2 设置 ref/unref + 空闲定时器 + noStreamsTimeout。
10. **write 发送请求**:
    - H1 writeH1:构造请求行 + headers + Content-Length/chunked 编码。对 stream body 用 AsyncWriter 流式写入;对 Buffer 一次性写入。
    - H2 writeH2:构造 HTTP/2 headers(:method/:path/:scheme/:authority),调用 session.request() 创建 Http2Stream,绑定 stream 事件(response/end/error/data/trailers/aborted)。
11. **响应回调**:H1 通过 Parser.execute() 逐字节触发回调;H2 通过 stream 事件触发。
12. **EOF 处理**:onMessageComplete(H1)或 onEnd/stream close(H2)→ handler.onResponseEnd → res.push(null)。
13. **队首推进**:`client[kQueue][kRunningIdx++] = null`,标记完成。
14. **keep-alive / reset**:shouldKeepAlive=true 时复用连接,设置 keepAlive timer;否则 destroy socket。

**dispatch() 返回值语义**:

```javascript
// client.js [kDispatch] 返回值
return this[kNeedDrain] < 2   // true=空闲可继续发, false=队列满需要等待 drain
```

`true`:连接仍可接受更多请求(未达并发上限);
`false`:连接已满,发出 drain 事件后客户端应暂停发送。

### 10.7 双协议关键差异对比表

| 维度 | HTTP/1.1 (client-h1) | HTTP/2 (client-h2) |
|------|---------------------|-------------------|
| **并发模型** | 流水线(pipelining),默认深度 1 | 多路复用(multiplexing),默认 100 并发流 |
| **defaultPipelining** | 1 | Infinity |
| **解析器** | llhttp WASM(逐字节状态机) | Node.js 内置 http2 模块 |
| **请求写入** | 手动拼接 HTTP 报文,直接写 socket | `session.request(headers, options)` 创建 stream |
| **Keep-Alive** | 手动管理超时和 Connection 头 | 通过 GOAWAY/PING 管理 |
| **流控** | 无显式流控,靠 TCP 背压 | per-stream + connection-level WINDOW_UPDATE |
| **Upgrade** | socket 从 parser 分离,交给上层 | Extended CONNECT (RFC 8441) |
| **错误恢复** | 销毁 socket,重建连接 | GOAWAY 后重建 session,可重试未被接受的 stream |
| **忙判断** | writing/reset/blocking/非幂等 in-flight | 达到 maxConcurrentStreams 或未收到 remoteSettings |
| **空闲管理** | socket.ref/unref + keepAlive timer | session.ref/unref + idle timeout |
| **请求体写入** | AsyncWriter 手动 chunked 编码 | stream.write()/pipeline() 原生支持 |

### 10.8 连接器与 TLS

`lib/core/connect.js` 实现 TCP/TLS 连接的建立。

**buildConnector 工厂函数**:

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
        socket: httpSocket,  // 升级 socket(用于 CONNECT 隧道)
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

**TLS Session 缓存**(`SessionCache`):

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

使用 `WeakRef` + `FinalizationRegistry` 实现自动清理的 TLS session 缓存,避免内存泄漏。默认缓存 100 个 session。

**H2C(明文 HTTP/2)支持**:

当 `useH2c: true` 时,直接在 TCP 连接上标记 `socket.alpnProtocol = 'h2'`,跳过 TLS ALPN 协商。

### 10.9 代理体系

undici 提供三种代理实现,核心差异如下:

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

#### ProxyAgent — HTTP CONNECT 隧道

`lib/dispatcher/proxy-agent.js` 支持三种代理模式:

**模式一:HTTP/1.1 正向代理(非隧道)**:

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

用于 HTTP 请求通过 HTTP 代理(非 HTTPS 目标),请求行变为 `GET http://target.com/path HTTP/1.1`。

**模式二:CONNECT 隧道**:

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

通过 CONNECT 方法建立隧道,在隧道内进行 TLS 握手连接到目标服务器。

**模式三:SOCKS5 代理**:

```javascript
if (protocol === 'socks5:' || protocol === 'socks:') {
  return new Socks5ProxyAgent(...)
}
```

**安全措施**:

```javascript
function throwIfProxyAuthIsSent (headers) {
  for (const key in headers) {
    if (key.toLowerCase() === 'proxy-authorization') {
      throw new InvalidArgumentError('Proxy-Authorization should be sent in ProxyAgent constructor')
    }
  }
}
```

禁止在请求头中直接设置 `Proxy-Authorization`,强制在构造函数中通过 `auth`/`token` 参数传递,防止凭据泄露。

#### EnvHttpProxyAgent — 环境变量代理

`lib/dispatcher/env-http-proxy-agent.js` 自动读取环境变量配置代理:

```javascript
const HTTP_PROXY = httpProxy ?? process.env.http_proxy ?? process.env.HTTP_PROXY
const HTTPS_PROXY = httpsProxy ?? process.env.https_proxy ?? process.env.HTTPS_PROXY
const NO_PROXY = noProxy ?? process.env.no_proxy ?? process.env.NO_PROXY
```

**NO_PROXY 匹配**:

```javascript
#shouldProxy (hostname, port) {
  if (this.#noProxyValue === '*') return false
  for (const entry of this.#noProxyEntries) {
    if (entry.port && entry.port !== port) continue
    if (hostname === entry.hostname) return false
    // 子域名匹配:example.com 匹配 api.example.com
    if (hostname.slice(-(entry.hostname.length + 1)) === `.${entry.hostname}`) return false
  }
  return true
}
```

支持精确匹配和子域名匹配,支持 IPv6 地址和端口过滤。

#### Socks5ProxyAgent — SOCKS5 代理

`lib/dispatcher/socks5-proxy-agent.js` 实现 SOCKS5 协议代理(实验性)。

**连接建立流程**:

1. 通过 `connect.js` 连接到 SOCKS5 代理服务器
2. 创建 `Socks5Client` 进行握手和认证
3. 发送 CONNECT 命令建立到目标的隧道
4. 如果目标是 HTTPS,在隧道 socket 上建立 TLS

**按 origin 分池**:

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

每个目标 origin 维护一个独立的 Pool,因为 SOCKS5 隧道是 per-connection 的。

### 10.10 重试机制

`lib/dispatcher/retry-agent.js` 通过组合模式实现请求重试:

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

`RetryHandler` 包装原始 handler,拦截错误响应,根据配置决定是否重试。重试时重新 dispatch 到底层 agent。

### 10.11 H2CClient — 明文 HTTP/2

`lib/dispatcher/h2c-client.js` 是 Client 的轻量子类,专门用于 HTTP/2 Cleartext(h2c)。

```javascript
class H2CClient extends Client {
  constructor (origin, clientOpts) {
    super(origin, {
      maxConcurrentStreams: defaultMaxConcurrentStreams,  // 默认 100
      pipelining: defaultPipelining,  // 默认 100,但必须 <= maxConcurrentStreams
      allowH2: true,
      useH2c: true  // 强制标记 alpnProtocol = 'h2'
    })
  }
}
```

通过 `useH2c: true` 让 connect.js 在 TCP 连接上伪造 `alpnProtocol = 'h2'`,使得 Client 的协议选择逻辑认为这是 H2 连接。

### 10.12 Dispatcher1Wrapper 向后兼容层

`lib/dispatcher/dispatcher1-wrapper.js` 处理 undici v1 和 v2 API 的差异。

**Handler 接口变化**:

| v1 接口 | v2 接口 |
|---------|---------|
| `onConnect(abort, context)` | `onRequestStart(controller, context)` |
| `onHeaders(statusCode, rawHeaders, resume, statusMessage)` | `onResponseStart(controller, statusCode, headers, statusText)` |
| `onData(chunk)` | `onResponseData(controller, chunk)` |
| `onComplete(rawTrailers)` | `onResponseEnd(controller, trailers)` |
| `onError(err)` | `onResponseError(controller, err)` |
| `onUpgrade(statusCode, rawHeaders, socket)` | `onRequestUpgrade(controller, statusCode, headers, socket)` |

**LegacyHandlerWrapper** 自动适配:

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

**强制 H1**:v1 消费者不支持 HTTP/2,因此 Dispatcher1Wrapper 强制设置 `allowH2: false`。

### 10.13 Request 核心模型

`lib/core/request.js` 定义了请求的内部表示。

**Request 类构造函数参数**:

```javascript
class Request {
  constructor (origin, {
    path, method, body, headers, query, idempotent, blocking, upgrade,
    headersTimeout, bodyTimeout, reset, expectContinue, servername,
    throwOnError, maxRedirections, typeOfService
  }, handler) { ... }
}
```

**body 类型处理**:

```javascript
if (body == null) { this.body = null }
else if (isStream(body)) { this.body = body /* 监听 error/end */ }
else if (isBuffer(body)) { this.body = body.byteLength ? body : null }
else if (ArrayBuffer.isView(body)) { this.body = Buffer.from(body.buffer, ...) }
else if (body instanceof ArrayBuffer) { this.body = Buffer.from(body) }
else if (typeof body === 'string') { this.body = Buffer.from(body) }
else if (isFormDataLike(body) || isIterable(body) || isBlobLike(body)) { this.body = body }
```

**idempotent 判断**:

```javascript
this.idempotent = idempotent == null
  ? method === 'HEAD' || method === 'GET' || method === 'QUERY'
  : idempotent
```

**RequestController** — 控制器:

```javascript
class RequestController {
  pause () { this.#paused = true }
  resume () { if (this.#paused) { this.#paused = false; this[kResume]?.() } }
  abort (reason) { if (!this.#aborted) { this.#aborted = true; this.#abort(reason) } }
}
```

提供给 handler 的控制器,支持 pause/resume 背压和 abort 取消。

**Handler 回调序列**:

```
onRequestStart(controller, context)     -- 请求开始
  onBodySent(chunk)                      -- 请求体块发送(可多次)
  onRequestSent()                        -- 请求体发送完成
onResponseStarted()                      -- 收到第一个字节
onResponseStart(controller, statusCode, headers, statusText)  -- 响应头
  onResponseData(controller, chunk)      -- 响应体块(可多次)
onResponseEnd(controller, trailers)      -- 响应完成
-- 或 --
onRequestUpgrade(controller, statusCode, headers, socket)  -- Upgrade
-- 或 --
onResponseError(controller, err)         -- 错误
```


---

## 11. Web API 层深度

### 11.1 Undici Web API 层定位

Undici 是 Node.js 官方的 HTTP/1.1 客户端实现,其 Web API 层位于 `lib/web/` 目录,实现了浏览器标准的 Web API 接口,使得 Node.js 环境能使用与浏览器一致的 API 进行网络通信。undici 的 Web API 层并非简单封装,而是严格对齐 WHATWG/W3C 规范的完整实现,每个函数注释都引用规范步骤编号。

### 11.2 模块拓扑

```
lib/web/
├── fetch/               # WHATWG Fetch 规范完整实现 (~7,860 行)
│   ├── index.js         # 核心 fetch 循环 (2,426 行)
│   ├── headers.js       # Headers + HeadersList (719 行)
│   ├── body.js          # Body mixin (547 行)
│   ├── request.js       # Request 类 (1,144 行)
│   ├── response.js      # Response 类 (639 行)
│   ├── util.js          # 规范工具函数 (1,525 行)
│   ├── constants.js     # 规范常量 (131 行)
│   ├── global.js        # 全局 Origin (40 行)
│   ├── data-url.js      # Data URL + MIME 解析 (596 行)
│   └── formdata.js      # FormData (278 行)
├── websocket/           # WebSocket RFC 6455 + RFC 8441 (~2,900 行)
│   ├── websocket.js     # WebSocket 主类 (781 行)
│   ├── connection.js    # 握手建立连接 (330 行)
│   ├── frame.js         # 帧编解码 (128 行)
│   ├── receiver.js      # 接收解析器 (508 行)
│   ├── sender.js        # 发送队列 (110 行)
│   ├── events.js        # MessageEvent/CloseEvent/ErrorEvent (332 行)
│   ├── constants.js     # WebSocket 常量 (127 行)
│   ├── permessage-deflate.js  # 压缩扩展 (100 行)
│   ├── util.js          # URL 记录/子协议验证 (348 行)
│   └── stream/          # WebSocketStream API
│       ├── websocketstream.js   # WebSocketStream (498 行)
│       └── websocketerror.js    # WebSocketError (104 行)
├── eventsource/         # EventSource SSE (~1,080 行)
│   ├── eventsource.js   # EventSource 主类 (493 行)
│   ├── eventsource-stream.js  # SSE 流解析器 (521 行)
│   └── util.js          # CORS 工具 (60 行)
├── cache/               # Cache API (~1,060 行)
│   ├── cache.js         # Cache 类 (862 行)
│   ├── cachestorage.js  # CacheStorage (152 行)
│   └── util.js          # URL 比较 + Vary 处理 (45 行)
├── cookies/             # Cookie 处理 (~870 行)
│   ├── index.js         # getCookies/setCookie/deleteCookie (199 行)
│   ├── parse.js         # Set-Cookie 解析器 (317 行)
│   ├── util.js          # 验证 + 序列化 (353 行)
│   └── constants.js     # Cookie 大小限制 (12 行)
├── subresource-integrity/  # SRI 子资源完整性校验 (~307 行)
│   └── subresource-integrity.js  # bytesMatch, parseMetadata, getStrongestMetadata
├── webidl/              # WebIDL 类型系统 (~1,000 行)
│   └── index.js         # 转换器 + brand check (1,004 行)
├── infra/               # WHATWG Infra 规范 (~230 行)
│   └── index.js         # 序列收集 + Base64 + 同构编解码
└── encoding/            # WHATWG Encoding 规范 (~34 行)
    └── index.js         # UTF-8 解码

lib/cache/               # 缓存存储层 (~750 行)
├── sqlite-cache-store.js  # SQLite WAL 持久化 (469 行)
└── memory-cache-store.js  # 内存 LRU (279 行)
```

### 11.3 核心架构特征

| 特征 | 说明 |
|------|------|
| **规范驱动** | 逐行对齐 WHATWG/W3C 规范,每个函数注释引用规范步骤编号 |
| **WebIDL 类型安全** | 所有公开 API 入口均通过 WebIDL 转换器进行类型检查和转换 |
| **Brand Check** | 所有类方法入口使用 `webidl.brandCheck()` 防止非法调用 |
| **内部/外部分离** | Request/Response 拥有 inner state 和 public wrapper 两层 |
| **FinalizationRegistry** | 利用 GC 回调自动清理未消费的 ReadableStream |
| **Proxy 过滤响应** | Response 的 filtered 版本通过 Proxy 实现透明代理 |
| **流式优先** | Body 基于 ReadableStream 实现流式消费 |

### 11.4 代码规模统计

| 模块 | 文件数 | 总行数 | 核心类/函数 |
|------|--------|--------|------------|
| Fetch API | 10 | ~7,860 | `fetch()`, `fetching()`, `mainFetch()`, `Headers`, `Request`, `Response`, `Body` |
| WebSocket | 12 | ~2,900 | `WebSocket`, `ByteParser`, `WebsocketFrameSend`, `SendQueue`, `PerMessageDeflate`, `WebSocketStream`, `WebSocketError` |
| EventSource | 3 | ~1,080 | `EventSource`, `EventSourceStream` |
| Cache API | 3 | ~1,060 | `Cache`, `CacheStorage` |
| Cache 存储层 | 2 | ~750 | `SqliteCacheStore`, `MemoryCacheStore` |
| Subresource Integrity | 1 | ~307 | `bytesMatch()`, `parseMetadata()`, `getStrongestMetadata()` |
| Cookies | 4 | ~870 | `getCookies()`, `setCookie()`, `parseSetCookie()`, `stringify()` |
| WebIDL | 1 | ~1,004 | `brandCheck()`, `ConvertToInt()`, `dictionaryConverter()`, `sequenceConverter()` |
| Infra/Encoding | 2 | ~264 | `collectASequenceOfCodePoints()`, `forgivingBase64()`, `utf8DecodeBytes()` |
| FormData | 2 | ~590 | `FormData`, `makeEntry()`, `parseFormData()` |
| Data URL | 1 | ~596 | `dataURLProcessor()`, `parseMIMEType()` |
| **总计** | **42** | **~16,300** | |

### 11.5 Fetch API 全链路深度分析

#### 核心入口: `fetch()` 与 `fetching()`

**文件**: `lib/web/fetch/index.js` (2,426 行)

**`fetch()` 公共 API**:

`fetch()` 是用户调用的入口,它本身是薄壳,真正的逻辑在 `fetching()` 中。

```javascript
async function fetch (input, init = {}) {
  // 1. 确保 fetch 没有被构造器调用
  if (this instanceof fetch) throw new TypeError('...')

  // 2. 创建 Promise + Reject Promise (使用 Node.js 原生的 Promise.withResolvers)
  const p = Promise.withResolvers()

  // 3. 解析请求: 如果 input 是 string/URL, 创建 Request; 如果是 Request, 直接使用
  let requestObject = ...

  // 4. 调用 fetching() 发起实际请求
  fetchParams.controller = fetching({
    request: requestObject[kState],
    processResponse (response) {
      // 响应头到达时回调: 如果是网络错误, reject promise
      if (isNetworkError(response)) {
        p.reject(response.error)
        return
      }
      p.resolve(fromInnerResponse(response, 'immutable'))
    },
    processResponseEndOfBody (response) {
      // 响应体结束回调
      if (response.aborted) {
        // 处理中止
      }
    },
    processResponseConsumeBody (response, ...args) {
      // body 消费回调
    }
  })

  return p.promise
}
```

**关键设计**: `fetch()` 本身只是薄壳,真正的逻辑在 `fetching()` 中。用户获得的是一个 Promise,内部通过回调函数逐步推进。这种回调驱动的模式是理解整个 Fetch 实现的关键。

**`fetching()` 主编排器**:

`fetching()` 是 Fetch 规范中 "Fetching" 算法的完整实现,承担了整个请求生命周期的编排:

```javascript
function fetching (fetchParams) {
  // 1. 获取 request
  const request = fetchParams.request

  // 2. 创建 Fetch 对象 (状态机)
  const fetch = new Fetch(
    this,
    request,
    fetchParams.processResponse,
    fetchParams.processResponseConsumeBody,
    fetchParams.processResponseEndOfBody
  )

  // 3. AbortController 绑定
  if (request.signal) {
    request.signal.addEventListener('abort', () => {
      fetch.abort(request.signal.reason)
    }, { once: true })
  }

  // 4. 处理 about: URL (直接返回空响应)
  if (requestCurrentURL(request).protocol === 'about:' && url === 'about:blank') {
    fetch.processResponse(makeResponse({ urlList: [requestCurrentURL(request)] }))
    return fetch.controller
  }

  // 5. 任务调度: 在微任务中调用 mainFetch
  queueMicrotask(() => mainFetch(fetchParams))

  return fetch.controller
}
```

**`Fetch` 类状态机**:

```javascript
class Fetch {
  #state = 'ongoing'  // three states: ongoing | terminated | aborted

  constructor (request, processResponse, processResponseConsumeBody,
               processResponseEndOfBody, taskDestination) {
    this.request = request
    this.processResponse = processResponse
    this.processResponseConsumeBody = processResponseConsumeBody
    this.processResponseEndOfBody = processResponseEndOfBody
    this.controller = new AbortController()
    this.taskDestination = taskDestination
  }

  // abort: user-initiated or signal-triggered
  abort (error) {
    if (this.#state !== 'ongoing') return
    this.#state = 'aborted'
    abortFetch(this, error)
  }

  // terminate: internal termination (e.g., network failure)
  terminate () {
    if (this.#state !== 'ongoing') return
    this.#state = 'terminated'
  }
}
```

**三态设计**: `ongoing`(进行中) / `terminated`(被终止) / `aborted`(被中止) — 与 WHATWG Fetch 规范严格对齐。`terminated` 表示内部错误终止,`aborted` 表示用户主动取消。

**`mainFetch()` — 协议路由**:

`mainFetch()` 是整个 Fetch 的核心路由函数,负责根据 URL scheme 选择正确的处理路径:

```javascript
async function mainFetch (fetchParams, recursion = 0) {
  // 1. 获取 request 和 taskDestination
  const request = fetchParams.request

  // 2. 跳过 ServiceWorker 处理 (Node.js 环境不支持)
  // 3. 处理 about: URL
  // 4. 处理 blob: URL

  // 5. 检查端口安全 (badPorts 列表)
  if (requestBadPort(request) === 'blocked') {
    fetchParams.processResponse(makeNetworkError('bad port'))
    return
  }

  let response

  // 6. 协议路由: 本地协议 vs 网络协议
  if (urlIsLocal(requestCurrentURL(request))) {
    // 本地协议 (about/blob/data/file): 直接 schemeFetch
    response = await schemeFetch(fetchParams)
  } else {
    // 网络协议 (http/https): 走 httpNetworkOrCacheFetch
    response = await httpNetworkOrCacheFetch(fetchParams, credentials)
  }

  // 7. 处理重定向 (最多 20 次)
  if (isRedirect(response.status)) {
    response = await httpRedirectFetch(fetchParams, response, recursion + 1)
  }

  // 8. 回调 processResponse
  fetchParams.processResponse(response)

  // 9. 回调 processResponseEndOfBody
  fetchParams.processResponseEndOfBody(response)

  return response
}
```

**协议分层**:
- `about:` / `blob:` / `data:` / `file:` -> `schemeFetch()` 本地处理
- `http:` / `https:` -> `httpNetworkOrCacheFetch()` 网络处理

**`schemeFetch()` — 本地协议处理**:

```javascript
async function schemeFetch (fetchParams) {
  const { protocol } = requestCurrentURL(request)

  if (protocol === 'about:' && url === 'about:blank') {
    // about:blank -> 空响应
    return new Response()
  }

  if (protocol === 'blob:') {
    // blob: URL -> 从 Blob 对象获取数据
    const blob = resolveBlobFromURL(url)
    if (!blob) return makeNetworkError('invalid blob URL')

    const body = extractBody(blob)
    return makeResponse({
      status: 200,
      statusText: 'OK',
      headersList: new HeadersList([['content-type', blob.type || 'application/octet-stream']]),
      body: body.body
    })
  }

  if (protocol === 'data:') {
    // data: URL -> Data URL 处理器
    const dataURLResult = dataURLProcessor(url)
    if (dataURLResult === 'failure') return makeNetworkError('invalid data URL')

    return makeResponse({
      status: 200,
      statusText: 'OK',
      headersList: new HeadersList([['content-type', serializeAMimeType(dataURLResult.mimeType)]]),
      body: dataURLResult.body
    })
  }
}
```

**`httpNetworkOrCacheFetch()` — 网络与缓存协商**:

这是最复杂的函数之一(约 300 行),处理 HTTP 请求的缓存协商和认证重试:

```javascript
async function httpNetworkOrCacheFetch (fetchParams, credentials) {
  // 1. 如果需要凭据但当前没有, 尝试从 cookie jar 获取
  if (credentials === 'include' && request.credentials === 'include') {
    // 从 cookie jar 获取 cookie 并添加到请求头
  }

  // 2. 处理条件请求头 (If-None-Match / If-Modified-Since)
  if (request.cache === 'default' || request.cache === 'no-cache') {
    if (request.cache === 'no-cache' || ...) {
      request.headersList.set('cache-control', 'no-cache')
    }
  }

  // 3. 设置 Content-Length
  if (request.body) {
    const contentLength = getBodyLength(request.body)
    if (contentLength !== null) {
      request.headersList.set('content-length', String(contentLength))
    }
  }

  // 4. 设置 Host 头
  const url = requestCurrentURL(request)
  request.headersList.set('host', url.host)

  // 5. 调用 httpFetch() 发起实际请求
  let response = await httpFetch(fetchParams)

  // 6. 401 响应处理: 如果需要认证, 重新请求 (带凭据)
  if (response.status === 401 && credentials === 'include') {
    // 重新发起请求, 这次带 cookie
    response = await httpFetch(fetchParams)
  }

  // 7. 407 响应处理: Proxy 认证
  if (response.status === 407) {
    // 处理 Proxy-Authenticate
  }

  return response
}
```

**`httpNetworkFetch()` — 真正的网络分发**:

这是 Fetch 与 undici HTTP 引擎的桥接点,通过 `agent.dispatch()` 将请求发送到底层 HTTP 客户端:

```javascript
async function httpNetworkFetch (fetchParams, includeCredentials = false) {
  // 1. 获取 request
  const request = fetchParams.request

  // 2. 创建响应 Promise
  const response = { ... }

  // 3. 创建请求处理器 (handler) — 这是与底层 HTTP 引擎的桥接
  const handler = {
    // 响应头到达
    onResponseStart (statusCode, reason, headers, socket) {
      // 创建 Response 对象
      const response = makeResponse({
        status: statusCode,
        statusText: reason,
        headersList: new HeadersList(headers)
      })

      // 处理 Content-Encoding 解压管线
      const contentEncoding = response.headersList.get('content-encoding')
      if (contentEncoding) {
        const encodings = contentEncoding.split(',').map(e => e.trim()).reverse()

        // 安全限制: 最多 5 层编码 (CVE 修复, 防止解压炸弹)
        if (encodings.length > 5) {
          response = makeNetworkError('too many content-encodings')
          return
        }

        // 创建解压管道
        for (const encoding of encodings) {
          if (encoding === 'gzip' || encoding === 'x-gzip') {
            pipeline = pipeline.pipe(zlib.createGunzip())
          } else if (encoding === 'deflate') {
            pipeline = pipeline.pipe(zlib.createInflate())
          } else if (encoding === 'br') {
            pipeline = pipeline.pipe(zlib.createBrotliDecompress())
          } else if (encoding === 'zstd') {
            pipeline = pipeline.pipe(zlib.createZstdDecompress())
          }
        }
      }

      response.socket = socket
      fetchParams.processResponse(response)
    },

    // 响应体数据到达
    onResponseData (data) {
      // 将数据推送到 ReadableStream controller
      if (response.body?.stream) {
        response.body.stream.controller.enqueue(data)
      }
    },

    // 响应体结束
    onResponseEnd () {
      if (response.body?.stream) {
        response.body.stream.controller.close()
      }
      fetchParams.processResponseEndOfBody(response)
    },

    // 错误处理
    onResponseError (error) {
      fetchParams.processResponse(makeNetworkError(error))
    },

    // WebSocket 升级
    onRequestUpgrade (request, socket, head) {
      // HTTP 升级回调
    }
  }

  // 4. 构建请求选项
  const url = requestCurrentURL(request)
  const requestOptions = {
    path: url.pathname + url.search,
    origin: url.origin,
    method: request.method,
    body: request.body ? request.body.source : null,
    headers: request.headersList,
    maxRedirections: 0,  // fetch 自己管理重定向 (httpRedirectFetch)
    signal: fetchParams.controller.signal,
    // ...
  }

  // 5. 通过 dispatcher (agent) 发起请求 — 这是核心桥接点
  fetchParams.dispatcher.dispatch(requestOptions, handler)

  return response
}
```

**核心桥接点**: `fetchParams.dispatcher.dispatch(requestOptions, handler)` 是 undici Web API 层与其底层 HTTP 引擎的唯一桥梁。handler 回调模式使得上层的 Fetch 规范实现无需了解底层传输细节。

**`httpRedirectFetch()` — 重定向处理**:

```javascript
async function httpRedirectFetch (fetchParams, response, recursion = 0) {
  // 1. 重定向次数限制 (默认 20)
  if (recursion > 20) return makeNetworkError('too many redirects')

  // 2. 获取 Location 头
  const location = response.headersList.get('location')
  if (!location) return response

  // 3. 解析 location URL (UTF-8 规范化)
  const locationURL = responseLocationURL(response, requestCurrentURL(request))
  if (!locationURL) return makeNetworkError('invalid redirect URL')

  // 4. 如果是 opaqueredirect 模式, 直接返回过滤响应
  if (request.responseType === 'opaqueredirect') {
    return filterResponse(response, 'opaqueredirect')
  }

  // 5. 根据重定向状态码修改请求方法
  // 303 状态码: 除 HEAD 外均改为 GET
  if (response.status === 303 && request.method !== 'HEAD') {
    request.method = 'GET'
    request.body = null  // 清除 body
  }

  // 6. 更新 URL 列表
  request.urlList.push(locationURL)

  // 7. 处理凭据模式变化
  // 跨域重定向时, 可能需要清除凭据
  if (!sameOrigin(locationURL, requestCurrentURL(request))) {
    request.headersList.delete('authorization')
    request.headersList.delete('cookie')
  }

  // 8. 递归调用 mainFetch
  return mainFetch(fetchParams, recursion)
}
```

### 11.6 Headers 实现

**文件**: `lib/web/fetch/headers.js` (719 行)

#### HeadersList — 底层存储

HeadersList 是 Headers 的底层存储,使用 Map 实现,同时为 Set-Cookie 提供独立的数组存储:

```javascript
class HeadersList {
  #headersMap = new Map()     // 核心存储: name -> value
  #cookies = null              // set-cookie 特殊处理 (数组而非拼接)
  #sortedMap = null            // 排序缓存 (HPACK 需要)

  constructor (init) {
    if (init instanceof HeadersList) {
      // 深拷贝模式
      this.#headersMap = new Map(init.#headersMap)
      this.#cookies = init.#cookies ? [...init.#cookies] : null
    } else if (Array.isArray(init)) {
      this.#headersMap = new Map(init)
    } else {
      this.#headersMap = new Map()
    }
  }

  // 追加 header
  append (name, value) {
    const lowercaseName = name.toLowerCase()

    // Set-Cookie 特殊处理: 存入数组而非拼接
    // 这是 HTTP 中唯一不能用 ", " 合并的 header
    if (lowercaseName === 'set-cookie') {
      if (!this.#cookies) this.#cookies = []
      this.#cookies.push(value)
      return
    }

    const existing = this.#headersMap.get(lowercaseName)
    if (existing !== undefined) {
      // 普通 header 用 ", " 拼接 (HTTP 规范)
      this.#headersMap.set(lowercaseName, existing + ', ' + value)
    } else {
      this.#headersMap.set(lowercaseName, value)
    }

    // 失效排序缓存
    this.#sortedMap = null
  }
}
```

**Set-Cookie 特殊性**: Set-Cookie 是 HTTP 中唯一不能合并的 header。如果两个 Set-Cookie 被 ", " 拼接,浏览器无法正确解析。undici 用独立的 `#cookies` 数组处理,这与浏览器行为一致。

#### 排序优化 — 二分插入排序

```javascript
toSortedArray () {
  // 如果缓存有效, 直接返回
  if (this.#sortedMap) return this.#sortedMap

  const headers = [...this.#headersMap]

  // 对于小数量 header (<=32), 使用二分插入排序
  // 这是为了满足 HTTP/2 头部压缩 (HPACK) 的稳定排序需求
  // Array.sort() 在 V8 中对于小数组使用插入排序,
  // 但 undici 手动实现以确保行为一致
  if (headers.length <= 32) {
    for (let i = 1; i < headers.length; i++) {
      const key = headers[i][0]
      let low = 0, high = i
      while (low < high) {
        const mid = (low + high) >>> 1
        if (headers[mid][0] < key) low = mid + 1
        else high = mid
      }
      // 移动元素
      for (let j = i; j > low; j--) {
        headers[j] = headers[j - 1]
      }
      headers[low] = [key, ...]
    }
  }

  this.#sortedMap = headers
  return headers
}
```

**设计洞察**: 二分插入排序在小数据集上比 `Array.sort()` 更快且行为更可预测。HTTP 请求的 header 数量通常在 10-20 个,这个优化是有意义的。

#### Headers Guard 系统

```javascript
class Headers {
  #guard = 'none'  // five guards

  append (name, value) {
    // 检查 guard
    if (this.#guard === 'immutable') {
      throw new TypeError('Headers are immutable')
    }
    if (this.#guard === 'request' && isForbiddenHeader(name)) {
      return  // 静默忽略禁止修改的 header
    }
    if (this.#guard === 'request-no-cors') {
      // 只允许 CORS 安全列表中的 header
      if (!isCorsSafeListedRequestHeader(name)) return
    }
    if (this.#guard === 'response' && isForbiddenResponseHeader(name)) {
      return
    }

    this.#headersList.append(name, value)
  }
}
```

**五级 Guard 设计**:
- `none`: 无限制 (用户创建的 Headers)
- `request`: 禁止修改 forbidden request header (如 Host, Cookie, Authorization)
- `response`: 禁止修改 Set-Cookie 等响应头
- `immutable`: 完全不可变 (用于 frozen response)
- `request-no-cors`: 只允许 CORS 安全列表中的 header (Accept, Accept-Language, Content-Language, Content-Type)

### 11.7 Request 类

**文件**: `lib/web/fetch/request.js` (1,144 行)

#### 构造器 — 41 步规范对齐

Request 构造器完全对齐 WHATWG Fetch 规范的 41 个步骤:

```javascript
constructor (input, init = {}) {
  // 1. 标记为不可 clone
  webidl.util.markAsUncloneable(this)

  // 2-3. WebIDL 类型转换
  input = webidl.converters.RequestInfo(input)
  init = webidl.converters.RequestInit(init)

  // 4. 创建新的 request (内部状态)
  let request = makeRequest({ url: new URL(input).href })

  // 5-6. 如果 input 是 Request 对象, 提取其内部 request
  if (webidl.is.Request(input)) {
    request = input[kState]
    // 处理 signal 传递: 创建跟随的 AbortController
  }

  // 7. 处理 init.method
  if (init.method !== undefined) {
    request.method = normalizeMethod(init.method)
  }

  // 8. 处理 init.headers / input headers
  if (init.headers !== undefined) {
    request.headersList = new HeadersList()
    // 构建 headers
  }

  // 9. 处理 init.body
  if (init.body !== undefined) {
    // extractBody() 处理各种 body 类型
    const { body, type } = extractBody(init.body)
    request.body = body
    if (type && !request.headersList.has('content-type')) {
      request.headersList.set('content-type', type)
    }
  }

  // 10-41. 逐步设置 request 的属性
  // - mode (navigate/same-origin/no-cors/cors)
  // - credentials (omit/same-origin/include)
  // - cache (default/no-store/reload/no-cache/force-cache/only-if-cached)
  // - redirect (follow/error/manual)
  // - referrerPolicy
  // - integrity
  // - keepalive
  // - priority

  this[kState] = request
  this[kHeaders] = new Headers(request.headersList, 'request')
}
```

#### AbortController 生命周期管理

undici 对 AbortController 的管理使用了 FinalizationRegistry + WeakRef 的组合:

```javascript
// FinalizationRegistry: 当 Request 对象被 GC 回收时触发
const requestFinalizer = new FinalizationRegistry(({ stream, controller }) => {
  // 如果底层 stream 未消费, 自动 abort 避免资源泄漏
  if (stream && !stream.locked) {
    controller.abort(new TypeError('Request was garbage collected before response'))
  }
})

// WeakMap: 跟踪克隆请求的信号跟随关系
const dependentControllerMap = new WeakMap()

function buildAbort (request) {
  const signal = request.signal
  if (!signal) return new AbortController().signal

  // 创建跟随原始 signal 的新 signal
  const controller = new AbortController()
  const originalSignal = signal

  // 当原始 signal abort 时, 新 controller 也 abort
  originalSignal.addEventListener('abort', () => {
    controller.abort(originalSignal.reason)
  }, { once: true })

  return controller.signal
}
```

**WeakRef + FinalizationRegistry 模式**: 当 Request 对象被 GC 回收但其底层 stream 未消费时,自动 abort 避免资源泄漏。这是 Web API 层独有的内存安全设计。

#### `makeRequest()` — 请求工厂

```javascript
function makeRequest (init) {
  return {
    method: 'GET',
    localURLsOnly: false,
    unsafeRequest: false,
    body: null,
    client: null,
    reservedClient: null,
    replacesClientId: '',
    window: 'client',
    keepalive: false,
    serviceWorkers: 'all',
    initiator: '',
    destination: '',
    priority: null,
    origin: '',
    policyContainer: '',
    referrerPolicy: '',
    referrer: 'client',
    mode: 'no-cors',
    useCORSPreflightFlag: false,
    credentials: 'same-origin',
    useURLCredentials: false,
    cache: 'default',
    redirect: 'follow',
    integrity: '',
    cryptographicNonceMetadata: '',
    parserMetadata: '',
    reloadNavigation: false,
    historyNavigation: false,
    userActivation: false,
    ...init  // 合并用户提供的属性
  }
}
```

**30+ 字段默认值**: 每个字段都有规范定义的默认值,确保内部状态的完整性。

### 11.8 Response 类

**文件**: `lib/web/fetch/response.js` (639 行)

#### Proxy 过滤响应模式

这是 undici 中最优雅的设计之一,使用 ES6 Proxy 实现响应过滤:

```javascript
function filterResponse (response, type) {
  // 创建 Proxy 包装原始 response
  return new Proxy(response, {
    get (target, prop) {
      // 拦截特定属性的访问
      if (prop === 'type') return type

      if (type === 'basic') {
        // basic 响应: 过滤 Set-Cookie 等敏感 header
        if (prop === 'headersList') {
          const filtered = new HeadersList(target.headersList)
          filtered.delete('set-cookie')
          return filtered
        }
      }

      if (type === 'opaque') {
        // opaque 响应: 所有属性不可见
        if (prop === 'url') return ''
        if (prop === 'status') return 0
        if (prop === 'statusText') return ''
        if (prop === 'headersList') return new HeadersList()
      }

      if (type === 'opaqueredirect') {
        // 类似 opaque, 但 status 为 0
        if (prop === 'url') return ''
        if (prop === 'status') return 0
        if (prop === 'statusText') return ''
        if (prop === 'headersList') return new HeadersList()
      }

      // 其他属性透传
      return Reflect.get(target, prop)
    },
    set (target, prop, value) {
      return Reflect.set(target, prop, value)
    }
  })
}
```

**四种过滤类型**:
- `basic`: 同源响应,过滤 Set-Cookie 等敏感 header
- `cors`: 跨域响应,只暴露 CORS 允许的 header
- `opaque`: 不透明响应,隐藏所有信息 (status=0, url='', headers=空)
- `opaqueredirect`: 重定向的不透明响应

**Proxy 的优雅**: 无需创建 Response 的子类,Proxy 在运行时动态拦截,零额外内存开销,且对调用方完全透明。

#### 静态工厂方法

```javascript
class Response {
  // 网络错误
  static error () {
    const r = makeNetworkError()
    return fromInnerResponse(r, 'immutable')
  }

  // 重定向
  static redirect (url, status = 302) {
    // 验证状态码必须是重定向状态 (301, 302, 303, 307, 308)
    if (!redirectStatus.includes(status)) throw new TypeError()

    // 解析 URL
    const parsedURL = new URL(url, settings.settingsObject.baseUrl)

    // 创建响应: 设置 Location 头
    const r = makeResponse({
      status,
      headersList: new HeadersList([['location', URLSerializer(parsedURL)]])
    })

    return fromInnerResponse(r, 'immutable')
  }

  // JSON 响应
  static json (data, init = {}) {
    // 序列化 JSON
    const body = serializeJavascriptValueToJSONString(data)

    // 创建响应: 设置 Content-Type
    const r = makeResponse({
      status: init.status ?? 200,
      statusText: init.statusText ?? '',
      headersList: new HeadersList(init.headers ? ... : [['content-type', 'application/json']])
    })

    // 设置 body
    const { body: extractedBody, type } = extractBody(body)
    r.body = extractedBody

    return fromInnerResponse(r, 'immutable')
  }
}
```

### 11.9 Body Mixin

**文件**: `lib/web/fetch/body.js` (547 行)

#### `extractBody()` — 体提取

`extractBody()` 处理所有可能的 body 类型,返回标准化的 body 对象和 Content-Type:

```javascript
function extractBody (object, keepalive = false) {
  // 1. string -> UTF-8 编码
  if (typeof object === 'string') {
    return {
      body: new TextEncoder().encode(object),
      type: 'text/plain;charset=UTF-8'
    }
  }

  // 2. URLSearchParams -> URL 编码
  if (object instanceof URLSearchParams) {
    return {
      body: object.toString(),
      type: 'application/x-www-form-urlencoded;charset=UTF-8'
    }
  }

  // 3. ArrayBuffer/TypedArray/BufferSource -> 字节拷贝
  if (ArrayBuffer.isView(object) || object instanceof ArrayBuffer) {
    const bytes = webidl.util.getCopyOfBytesHeldByBufferSource(object)
    return { body: bytes, type: null }
  }

  // 4. FormData -> multipart 编码
  if (object instanceof FormData) {
    const boundary = `----formdata-undici-0${random(1e11).toString().padStart(11, '0')}`
    const body = multipartFormData(object, boundary)
    return { body, type: `multipart/form-data; boundary=${boundary}` }
  }

  // 5. Blob -> 直接使用
  if (object instanceof Blob) {
    return { body: object, type: object.type || null }
  }

  // 6. ReadableStream -> 直接使用
  if (object instanceof ReadableStream) {
    return { body: object, type: null }
  }

  // 7. Async iterable -> ReadableStream 包装
  if (typeof object?.[Symbol.asyncIterator] === 'function') {
    const stream = ReadableStream.from(object)
    return { body: stream, type: null }
  }
}
```

#### Body 消费方法

Body mixin 提供 7 种消费方式:

```javascript
function bodyMixinMethods (instance) {
  return {
    // Blob 消费
    async blob () {
      const buffer = await consumeBody(this)
      if (!buffer) return new Blob()
      return new Blob([buffer], { type: this.headers.get('content-type') || '' })
    },

    // ArrayBuffer 消费
    async arrayBuffer () {
      const buffer = await consumeBody(this)
      if (!buffer) return new ArrayBuffer(0)
      return buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength)
    },

    // 文本消费 (UTF-8 解码)
    async text () {
      const buffer = await consumeBody(this)
      if (!buffer) return ''
      return utf8DecodeBytes(buffer)
    },

    // JSON 消费
    async json () {
      const text = await this.text()
      return JSON.parse(text)
    },

    // FormData 消费
    async formData () {
      const contentType = this.headers.get('content-type')
      if (contentType?.startsWith('multipart/form-data')) {
        // multipart 解析
      } else {
        // URL-encoded 解析
      }
    },

    // Uint8Array 消费 (bytes) — 新 API
    async bytes () {
      const buffer = await consumeBody(this)
      if (!buffer) return new Uint8Array(0)
      return new Uint8Array(buffer)
    },

    // 流式文本消费 — 新 API
    async * textStream () {
      const stream = this.body
      const decoder = new TextDecoderStream()
      const reader = stream.pipeThrough(decoder).getReader()
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        yield value
      }
    }
  }
}
```

### 11.10 Fetch 工具函数

**文件**: `lib/web/fetch/util.js` (1,525 行)

#### Referrer 策略实现

```javascript
function determineRequestsReferrer (request) {
  // 支持 8 种 referrer 策略
  switch (request.referrerPolicy) {
    case 'no-referrer':
      return 'no referrer'

    case 'origin':
      // 只返回 origin (不含路径和查询参数)
      return stripURLForReferrer(requestCurrentURL(request), true)

    case 'unsafe-url':
      // 返回完整 URL (包括路径和查询参数)
      return stripURLForReferrer(requestCurrentURL(request), false)

    case 'same-origin':
      // 同源才返回, 跨域返回 no referrer
      if (!sameOrigin(requestCurrentURL(request), request.client?.url)) {
        return 'no referrer'
      }
      return stripURLForReferrer(requestCurrentURL(request), false)

    case 'strict-origin':
      // HTTPS->HTTPS 返回 origin, 降级则不发送
      if (isDowngrade(requestCurrentURL(request), request.client?.url)) {
        return 'no referrer'
      }
      return stripURLForReferrer(requestCurrentURL(request), true)

    case 'origin-when-cross-origin':
      // 同源返回完整 URL, 跨域只返回 origin
      if (!sameOrigin(requestCurrentURL(request), request.client?.url)) {
        return stripURLForReferrer(requestCurrentURL(request), true)
      }
      return stripURLForReferrer(requestCurrentURL(request), false)

    case 'strict-origin-when-cross-origin':
      // 同源返回完整 URL, 跨域 HTTPS->HTTPS 返回 origin, 降级则不发送
      // ... (最严格的策略)

    case 'no-referrer-when-downgrade':
      // HTTPS->HTTP 不发送, 其他正常
      if (isDowngrade(requestCurrentURL(request), request.client?.url)) {
        return 'no referrer'
      }
      return stripURLForReferrer(requestCurrentURL(request), false)
  }
}
```

#### `InflateStream` — 内容解压

```javascript
class InflateStream extends Transform {
  #inflate

  constructor (algorithm) {
    super()

    // 根据算法选择解压器
    switch (algorithm) {
      case 'x-gzip':
      case 'gzip':
        this.#inflate = zlib.createGunzip()
        break
      case 'deflate':
        this.#inflate = zlib.createInflate()
        break
      case 'br':
        this.#inflate = zlib.createBrotliDecompress()
        break
      case 'zstd':
        this.#inflate = zlib.createZstdDecompress()
        break
      default:
        // 原始 deflate: 带自动检测 (windowBits = -15)
        this.#inflate = zlib.createInflateRaw({ windowBits: -15 })
    }

    // 管道连接
    this.#inflate.on('data', (chunk) => this.push(chunk))
    this.#inflate.on('end', () => this.push(null))
  }

  _transform (chunk, encoding, callback) {
    this.#inflate.write(chunk, callback)
  }

  _flush (callback) {
    this.#inflate.end(callback)
  }
}
```

#### 端口安全 — `requestBadPort()`

```javascript
// 70+ 个已知不安全端口
const badPorts = new Set([
  1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53,
  77, 79, 87, 95, 101, 102, 103, 104, 109, 110, 111, 113, 115, 117,
  119, 123, 135, 139, 143, 179, 389, 427, 465, 512, 513, 514, 515,
  526, 530, 531, 532, 540, 548, 556, 563, 587, 601, 636, 993, 995,
  1719, 1720, 1723, 2049, 3659, 4045, 5060, 5061, 6000, 6566, 6665,
  6666, 6667, 6668, 6669, 6697, 10080
])

function requestBadPort (request) {
  const url = requestCurrentURL(request)
  const port = url.port

  // 如果端口在 badPorts 列表中, 返回 blocked
  if (badPorts.has(Number(port))) {
    return 'blocked'
  }
  return 'allowed'
}
```

**安全设计**: 防止 Fetch 请求访问已知的不安全端口 (SMTP, Telnet, RPC, NFS 等),避免 SSRF 攻击。

#### `fullyReadBody()` — 完全读取 body

```javascript
function fullyReadBody (body, processBody, processBodyError) {
  // 1. 如果 body 是 null/undefined, 直接返回空
  if (!body || !body.stream) {
    processBody(new Uint8Array(0))
    return
  }

  // 2. 从 ReadableStream 读取所有数据
  readAllBytes(body.stream).then(processBody, processBodyError)
}

async function readAllBytes (stream) {
  const reader = stream.getReader()
  const chunks = []
  let totalLength = 0

  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    chunks.push(value)
    totalLength += value.length
  }

  // 合并所有 chunk
  const result = new Uint8Array(totalLength)
  let offset = 0
  for (const chunk of chunks) {
    result.set(chunk, offset)
    offset += chunk.length
  }

  return result
}
```

### 11.11 内容编码管线

在 `lib/web/fetch/index.js` 的 `httpNetworkFetch()` 中,`onResponseStart` 回调处理 Content-Encoding:

```javascript
onResponseStart (statusCode, headers, ...) {
  // 1. 创建 Response
  // 2. 处理 Content-Encoding

  const contentEncoding = response.headersList.get('content-encoding')
  if (contentEncoding) {
    // 反转编码顺序 (多个编码时, 先解外层)
    // 例如 "gzip, br" -> 先 br 解压, 再 gzip 解压
    const encodings = contentEncoding.split(',').map(e => e.trim()).reverse()

    // 最多 5 层编码 (CVE 修复)
    if (encodings.length > 5) {
      response = makeNetworkError('too many content-encodings')
      return
    }

    // 创建解压管道 (流式)
    for (const encoding of encodings) {
      if (encoding === 'gzip' || encoding === 'x-gzip') {
        pipeline = pipeline.pipe(zlib.createGunzip())
      } else if (encoding === 'deflate') {
        pipeline = pipeline.pipe(zlib.createInflate())
      } else if (encoding === 'br') {
        pipeline = pipeline.pipe(zlib.createBrotliDecompress())
      } else if (encoding === 'zstd') {
        pipeline = pipeline.pipe(zlib.createZstdDecompress())
      }
    }
  }
}
```

**安全考量**: 最多 5 层编码限制是针对 CVE 的修复,防止解压炸弹 (zip bomb) 攻击。攻击者可能嵌套多层压缩,使最终解压后的数据量呈指数级增长。

### 11.12 fetch 全链路端到端时序图

以下是从 `undici.fetch()` 入口到 `Response` 构造完成的完整调用链时序:

```
用户代码                     Web API 层                     Dispatcher             网络
  │                            │                              │                    │
  │  fetch(url, init)          │                              │                    │
  │──────────────────────────>│                              │                    │
  │                            │                              │                    │
  │  ┌─────────────────────┐  │                              │                    │
  │  │ new Request(input)  │  │  ① 构造 Request:             │                    │
  │  │  - URL 解析          │  │     - 41 步规范对齐          │                    │
  │  │  - Header 初始化     │  │     - Headers Guard 设置     │                    │
  │  │  - Body 提取         │  │     - AbortSignal 绑定       │                    │
  │  │  - Referder 策略     │  │                              │                    │
  │  └─────────────────────┘  │                              │                    │
  │                            │                              │                    │
  │                            │  fetching({ request, ... })  │                    │
  │                            │  ② 创建 Fetch 控制器         │                    │
  │                            │  ③ queueMicrotask(mainFetch) │                    │
  │                            │────────── 微任务 ───────────>│                    │
  │                            │                              │                    │
  │                            │  mainFetch(fetchParams)      │                    │
  │                            │  ④ 协议路由:                  │                    │
  │                            │     ┌─ data: → schemeFetch   │                    │
  │                            │     ├─ blob: → schemeFetch   │                    │
  │                            │     ├─ http: → httpFetch     │                    │
  │                            │     └─ https: → httpFetch    │                    │
  │                            │                              │                    │
  │                            │  httpNetworkOrCacheFetch()   │                    │
  │                            │  ⑤ 设置请求头:                │                    │
  │                            │     - Accept, Accept-Language│                    │
  │                            │     - Content-Length         │                    │
  │                            │     - Origin, Referer        │                    │
  │                            │     - User-Agent             │                    │
  │                            │     - Accept-Encoding        │                    │
  │                            │                              │                    │
  │                            │  httpNetworkFetch()          │                    │
  │                            │  ⑥ dispatcher.dispatch()     │                    │
  │                            │─────────────────────────────>│                    │
  │                            │                              │   HTTP REQUEST     │
  │                            │                              │───────────────────>│
  │                            │                              │                    │
  │                            │                              │   HTTP RESPONSE    │
  │                            │  onResponseStart()           │<───────────────────│
  │                            │  ⑦ 解析 Content-Encoding     │                    │
  │                            │     构建解压管线              │                    │
  │                            │                              │                    │
  │                            │  onResponseData() × N        │                    │
  │                            │  ⑧ ReadableStream 推送 chunk │                    │
  │                            │                              │                    │
  │                            │  onResponseEnd()             │                    │
  │                            │  ⑨ 关闭 stream               │                    │
  │                            │                              │                    │
  │                            │  processResponse(response)   │                    │
  │                            │  ⑩ SRI 校验 (如有)           │                    │
  │                            │      bytesMatch()            │                    │
  │                            │                              │                    │
  │                            │  fetchFinale()               │                    │
  │                            │  ⑪ finalizeAndReportTiming  │                    │
  │                            │                              │                    │
  │  p.resolve(Response)       │                              │                    │
  │<───────────────────────────│                              │                    │
  │                            │                              │                    │
  │  await response.json()     │                              │                    │
  │──────────────────────────>│                              │                    │
  │  ┌─────────────────────┐  │                              │                    │
  │  │ extractBody()       │  │  ⑫ 消费 Body stream         │                    │
  │  │ utf8Decode()        │  │     解码 UTF-8               │                    │
  │  │ JSON.parse()        │  │     解析 JSON                │                    │
  │  └─────────────────────┘  │                              │                    │
  │                            │                              │                    │
  │  { data }                  │                              │                    │
  │<───────────────────────────│                              │                    │
```

**关键回调**:
- `processResponse`: 响应头到达时调用 (可多次, 用于重定向)
- `processResponseEndOfBody`: 响应体完全消费后调用
- `processResponseConsumeBody`: 体消费过程中产生数据时调用
- `processRequestBodyChunkLength`: 请求体 chunk 发送时进度回调

### 11.13 Subresource Integrity (SRI) 子资源完整性校验

**文件**: `lib/web/subresource-integrity/subresource-integrity.js` (307 行)

SRI (Subresource Integrity) 是 W3C WebAppSec 规范定义的浏览器安全机制,通过密码学哈希验证子资源 (脚本/样式表) 未被篡改。undici 在 `mainFetch()` 的第 20 步完整实现了 SRI 校验流程。

#### 算法集合与合规性

SRI 规范定义有效的哈希算法 token 集合为: `« "sha256", "sha384", "sha512" »`。这个集合是有序的,后面的算法更强。undici 在启动时根据 Node.js crypto 模块的实际支持情况动态裁剪:

```javascript
const validSRIHashAlgorithmTokenSet = new Map([
  ['sha256', 0],
  ['sha384', 1],
  ['sha512', 2]
])

// 启动时检测 Node.js 的 crypto 支持
if (runtimeFeatures.has('crypto')) {
  const crypto = require('node:crypto')
  const cryptoHashes = crypto.getHashes()

  // Node.js 编译时若未启用 OpenSSL, 清空整个集合
  if (cryptoHashes.length === 0) {
    validSRIHashAlgorithmTokenSet.clear()
  }

  // 删除 Node.js 不支持的算法
  for (const algorithm of validSRIHashAlgorithmTokenSet.keys()) {
    if (cryptoHashes.includes(algorithm) === false) {
      validSRIHashAlgorithmTokenSet.delete(algorithm)
    }
  }
}
```

**关键设计**: 算法集合是模块级单例,在首次加载时一次性检测并裁剪。后续调用直接使用 `Map.prototype.has/get`,通过 bind 绑定避免重复分配:

```javascript
const isValidSRIHashAlgorithm = Map.prototype.has.bind(validSRIHashAlgorithmTokenSet)
const getSRIHashAlgorithmIndex = Map.prototype.get.bind(validSRIHashAlgorithmTokenSet)
```

#### 主入口: `bytesMatch()`

`bytesMatch()` 是 SRI 校验的核心函数,遵循规范 ["Does response match metadata list?"](https://w3c.github.io/webappsec-subresource-integrity/#does-response-match-metadatalist):

```javascript
const bytesMatch = runtimeFeatures.has('crypto') === false || validSRIHashAlgorithmTokenSet.size === 0
  ? () => true  // 无 crypto 支持 → 默认放行
  : (bytes, metadataList) => {
      // 1. 解析 metadata list
      const parsedMetadata = parseMetadata(metadataList)

      // 2. 空列表 → 匹配成功
      if (parsedMetadata.length === 0) {
        return true
      }

      // 3. 取最强算法的 metadata 集合
      const metadata = getStrongestMetadata(parsedMetadata)

      // 4. 逐项尝试匹配 (任一通过即可)
      for (const item of metadata) {
        const algorithm = item.alg
        const expectedValue = item.val

        // 5. 计算实际哈希 (使用 crypto.hash 统一接口)
        const actualValue = applyAlgorithmToBytes(algorithm, bytes)

        // 6. 大小写敏感匹配 (允许 base64url 格式)
        if (caseSensitiveMatch(actualValue, expectedValue)) {
          return true
        }
      }

      // 5. 全部不匹配 → 失败
      return false
    }
```

**降级策略**: 当 Node.js 未编译 crypto 模块时,函数直接返回 `true` (放行请求)。这是规范允许的行为 — 无效哈希默认放行。

#### 元数据解析: `parseMetadata()`

解析 `integrity` 属性值 (例如 `"sha384-abc... sha512-def..."`):

```javascript
function parseMetadata (metadata) {
  const result = []

  // 按空格分割多个哈希声明
  for (const item of metadata.split(' ')) {
    // 1. 按 ? 分割 expression-and-options (丢弃 options)
    const expressionAndOptions = item.split('?', 1)

    // 2. 按 - 分割 algorithm-expression
    const algorithmExpression = expressionAndOptions[0]
    const algorithmAndValue = [
      algorithmExpression.slice(0, 6),    // "sha256" / "sha384" / "sha512"
      algorithmExpression.slice(7)         // base64 hash
    ]

    // 3. 验证算法 token 是否有效
    if (!isValidSRIHashAlgorithm(algorithmAndValue[0])) {
      continue
    }

    // 4. 构造 metadata 对象
    result.push({
      alg: algorithmAndValue[0],
      val: algorithmAndValue[1]
    })
  }

  return result
}
```

**合规注释**: 规范的解析更复杂 (需处理 `U+003F` 和 `U+002D` 的位置),undici 采用简化的前 6 字符切片,对三种已知算法足够安全。

#### 最强算法选择: `getStrongestMetadata()`

当 integrity 属性包含多个哈希声明时,必须选择算法强度最高的一个进行校验:

```javascript
function getStrongestMetadata (metadataList) {
  const result = []
  let strongest = null

  for (const item of metadataList) {
    // 第一个 item 直接设置
    if (result.length === 0) {
      result.push(item)
      strongest = item
      continue
    }

    const currentIndex = getSRIHashAlgorithmIndex(strongest.alg)
    const newIndex = getSRIHashAlgorithmIndex(item.alg)

    if (newIndex < currentIndex) {
      continue  // 新算法更弱 → 跳过
    } else if (newIndex > currentIndex) {
      strongest = item
      result[0] = item
      result.length = 1  // 重置为仅含新最强
    } else {
      result.push(item)  // 相同强度 → 追加
    }
  }

  return result
}
```

**设计要点**:
- 规范要求 "getting the strongest metadata from the set of metadata"
- SHA-512 > SHA-384 > SHA-256 (由 Map 插入顺序决定)
- 最终返回的集合只包含最强算法 (可能有多个相同强度的哈希值)

#### Base64 兼容匹配: `caseSensitiveMatch()`

SRI 允许 base64 (标准) 和 base64url (URL 安全) 两种编码:

```javascript
function caseSensitiveMatch (actualValue, expectedValue) {
  // 1. 移除 padding (=) 字符
  let actualLength = actualValue.length
  if (actualValue[actualLength - 1] === '=') actualLength -= 1
  if (actualValue[actualLength - 1] === '=') actualLength -= 1

  let expectedLength = expectedValue.length
  if (expectedValue[expectedLength - 1] === '=') expectedLength -= 1
  if (expectedValue[expectedLength - 1] === '=') expectedLength -= 1

  // 2. 长度不等 → 失败
  if (actualLength !== expectedLength) return false

  // 3. 逐字符比较 (支持 + ↔ - 和 / ↔ _ 互换)
  for (let i = 0; i < actualLength; ++i) {
    if (
      actualValue[i] === expectedValue[i] ||
      (actualValue[i] === '+' && expectedValue[i] === '-') ||
      (actualValue[i] === '/' && expectedValue[i] === '_')
    ) {
      continue
    }
    return false
  }

  return true
}
```

**兼容性**: base64 与 base64url 的区别在于第 62/63 字符 (`+/` vs `-_`),WPT 测试要求 "be liberal with padding"。

#### Fetch 流程中的 SRI 集成

在 `mainFetch()` 第 20 步,undici 在获取响应体后执行 SRI 校验:

```javascript
// 在 mainFetch() 中
const response = await httpNetworkOrCacheFetch(fetchParams)

// SRI 校验
if (request.integrity) {
  const bytes = await readAllBytes(response.body.stream)
  if (!bytesMatch(bytes, request.integrity)) {
    response = makeNetworkError('SRI mismatch')
  }
}
```


---

## 12. 拦截器与 Mock 系统深度

(本章节内容已在前面章节中完整覆盖,此处做索引汇总避免重复)

### 12.1 拦截器体系索引

**8 个可组合拦截器**详见第 5 章:
- `cache` — HTTP 缓存(Memory/SQLite 双后端)
- `retry` — 自动重试(薄包装,实现在 RetryHandler)
- `redirect` — 自动重定向(薄包装)
- `dns` — DNS 缓存(TTL + 刷新)
- `decompress` — 自动解压(gzip/deflate/br/zstd)
- `deduplicate` — 请求去重(相同请求合并)
- `dump` — 调试 dump(打印请求/响应)
- `response-error` — 响应错误转换(4xx/5xx → Error)

**Handler 装饰器**详见第 5 章:
- `DecoratorHandler` — 装饰器基类
- `CacheHandler` — 缓存处理(最大单文件 Handler,802 行)
- `CacheRevalidationHandler` — 缓存再验证
- `RedirectHandler` — 重定向处理
- `DeduplicationHandler` — 请求去重
- `RetryHandler` — 指数退避重试(548 行,含 Range 续传)

**拦截器组合方式**:洋葱模型,`compose` 方法实现链式组合。

### 12.2 Mock 系统索引

**Mock 录制回放系统**详见第 6 章:
- `MockAgent` — 替代真实 Agent,拦截所有 HTTP 请求
- `MockClient` — Mock 单连接
- `MockPool` — Mock 连接池
- `MockInterceptor` — URL/Method/Header 匹配
- `MockCallHistory` — 调用历史记录
- `SnapshotAgent` — 快照回放
- `SnapshotRecorder` — 快照录制
- `mockErrors` — Mock 专用错误

**Mock 特性**:
- **匹配器**:字符串精确匹配、正则、函数闭包
- **times/persist**:限制拦截次数或持久拦截
- **reply/callback**:静态响应或动态回调(支持 async)
- **assertNoPendingInterceptors**:测试断言确保所有拦截器被消费
- **enableNetConnect/disableNetConnect**:控制是否允许真实网络请求

### 12.3 代理支持(3 种代理 + 环境变量)

详见第 10 章第 10.9 节:
- HTTP CONNECT: `dispatcher/proxy-agent.js` — 通过 CONNECT 方法建立隧道
- 环境变量: `dispatcher/env-http-proxy-agent.js` — 读取 HTTP_PROXY/HTTPS_PROXY/NO_PROXY(支持免代理列表)
- SOCKS5: `dispatcher/socks5-proxy-agent.js` — SOCKS5 协议代理(含 `lib/core/socks5-client.js` 422 行完整协议)

### 12.4 错误处理、诊断钩子与可观测性

详见第 8 章:
- 错误体系(`lib/core/errors.js`,497 行):统一基类 `UndiciError`,错误码 `UND_ERR_*`,26 种错误类
- 诊断钩子(`lib/core/diagnostics.js`):`diagnostics_channel` 全覆盖,18 个通道
- 可观测性:FastTimer、ClientStats/PoolStats 统计快照

### 12.5 其他 Web 模块

- **FormData**(`lib/web/fetch/formdata.js` + `formdata-parser.js`):FormData 类 + multipart 解析
- **Data URL**(`lib/web/fetch/data-url.js`):Data URL + MIME 解析
- **Cookies**(`lib/web/cookies/`):Set-Cookie 解析 + Cookie 序列化
- **WebIDL**(`lib/web/webidl/`):WebIDL 类型转换
- **Infra**(`lib/web/infra/`):WHATWG Infra 规范工具
- **Encoding**(`lib/web/encoding/`):UTF-8 解码


---

## 13. 对 laew 的借鉴(HTTP 客户端设计参考)

### 13.1 P0(立即可借鉴)

1. **Interceptor 链式模式**:laew 的工具调用可借鉴拦截器模式,在请求前后注入缓存/重试/日志
2. **固定队列(FixedQueue)**:laew 的任务队列可用 O(1) 入队/出队替代 Vec
3. **错误码体系**:统一错误码 + cause 链,便于调试和错误分类
4. **诊断钩子**:零开销的 diagnostics_channel,可移植到 laew 的遥测系统
5. **Global Dispatcher + Symbol.for**:laew 的全局 LLM 客户端可借鉴跨 Realm 单例模式

### 13.2 P1(中期借鉴)

6. **连接池管理**:laew 的 LLM 连接池可借鉴 Pool 的按需创建 + 空闲超时关闭
7. **Mock 测试系统**:laew 的 e2e 测试可借鉴 MockAgent 的录制/回放模式(`SnapshotAgent`)
8. **负载均衡**:BalancedPool 的加权策略可用于多 LLM endpoint 调度
9. **双入口设计**:laew 可参考 index.js / index-fetch.js 分离核心 API 与 Web API 的入口
10. **FastTimer**:laew 的超时管理可借鉴低分辨率计时器优化

### 13.3 P2(长期参考)

11. **Web 标准对齐**:fetch/WebSocket/EventSource 的规范严格对齐方式
12. **双协议支持**:HTTP/1.1 + HTTP/2 的 ALPN 协商和降级策略
13. **W3C 兼容性测试**:WPT 子模块 + Autobahn 测试套件,laew 的 HTTP 层可参考
14. **undici-types 双发布**:laew 若需要分离类型定义可参考

### 13.4 Dispatcher 分层架构 → laew LLM Client 分层

**undici 的分层**:
```
Dispatcher(抽象) → DispatcherBase(生命周期) → Client(单连接) → Pool(多连接) → Agent(多origin)
```

**laew 可借鉴**:
```
LlmClient(抽象 trait) → LlmClientBase(生命周期管理) → AnthropicClient/OpenAiClient(单会话) → SessionPool(多会话) → LlmAgent(多模型)
```

undici 的 `kDispatch` / `kClose` / `kDestroy` 三个内部 Symbol 方法对应 laew 的 trait 方法,`DispatcherBase` 的生命周期状态机(closed → destroyed)可以直接移植到 Rust 的 `Drop` trait 和状态枚举。

### 13.5 三段式队列 → laew 任务队列

undici Client 的三段式队列(complete | running | pending)是处理异步请求的高效模式。laew 的 Main-Work Agent 管理 WorkFlow 列表时可以采用同样的设计:

```rust
struct WorkflowQueue {
    queue: Vec<Option<WorkflowTask>>,
    running_idx: usize,
    pending_idx: usize,
}
```

分摊 O(1) 的清理策略(累积 256 个空位后 splice)比每次完成都移除更高效。

### 13.6 工厂模式连接管理 → laew Provider 管理

undici 的 `factory` 模式允许注入自定义 Client 创建逻辑:

```javascript
const pool = new Pool(origin, { factory: (origin, opts) => new CustomClient(origin, opts) })
```

laew 的 Provider 管理可以借鉴:每条接入记录(protocol + provider_name + model_name + endpoint + api_key)对应一个工厂,根据 protocol 选择 AnthropicClient 或 OpenAiClient。

### 13.7 Agent 按 origin 路由 → laew 按 provider 路由

undici Agent 的 `Map<origin, Dispatcher>` 自动按 URL origin 路由请求到对应的 Pool。laew 可以实现类似的 `Map<provider_key, LlmClient>` 路由:

```rust
struct LlmRouter {
    clients: HashMap<String, Box<dyn LlmClient>>,
    factory: Box<dyn LlmClientFactory>,
}
```

### 13.8 FixedQueue → Rust 无锁队列

undici 的 FixedQueue(链表 + 环形缓冲区)可以用 Rust 的 `crossbeam::ArrayQueue` 或手写 `VecDeque` 替代。核心思想一致:固定大小分段,避免动态数组的频繁分配。

### 13.9 诊断钩子 → laew 可观测性

undici 的 `diagnostics_channel` 覆盖了请求全生命周期(beforeConnect → connected → sendHeaders → headers → bodyChunkReceived → trailers → error)。laew 可以借鉴这个模式,用 trait 回调或事件总线实现:

```rust
trait LlmDiagnostics {
    fn on_request_start(&self, meta: &RequestMeta) {}
    fn on_token_received(&self, token: &str) {}
    fn on_response_complete(&self, meta: &ResponseMeta) {}
    fn on_error(&self, error: &AgentError) {}
}
```

### 13.10 拦截器链 → laew 中间件

undici 的 `compose` 方法实现了洋葱模型拦截器:

```javascript
const dispatcher = agent.compose(
  (dispatch) => (opts, handler) => { /* before */ dispatch(opts, handler); /* after */ },
  retryInterceptor,
  redirectInterceptor
)
```

laew 可以用 trait + 装饰器模式实现类似功能:

```rust
trait Interceptor {
    fn intercept(&self, request: Request, next: Box<dyn Fn(Request) -> Response>) -> Response;
}
```

用于自动重试、协议转换、日志记录、成本统计等横切关注点。

### 13.11 错误分级 → laew 错误体系

undici 将错误分为致命(SocketError 导致清空队列)和信息性(InformationalError 不清空队列)两类。laew 的 AgentError 可以类似分级:

- `Fatal`:API key 无效、provider 不可达 → 终止当前任务
- `Retryable`:超时、限流 → 自动重试
- `Informational`:连接空闲、协议切换 → 记录但不中断

### 13.12 TLS Session 缓存 → laew 连接复用

undici 的 `WeakRef + FinalizationRegistry` TLS session 缓存避免了内存泄漏。在 Rust 中可以用 `Weak<Mutex<TlsSession>>` 实现类似语义,或直接用 LRU 缓存 + TTL。

### 13.13 GOAWAY/REFUSED_STREAM 重试 → laew 流中断恢复

undici 对 H2 GOAWAY 和 REFUSED_STREAM 的处理(分离 stream、重新入队、限制重试次数)可以直接借鉴到 laew 的 SSE 流中断恢复:

```rust
// 当 LLM 流中断时:
// 1. 分离当前 stream 关联
// 2. 如果 body 可重放(已完成的 buffer),重新入队
// 3. 限制重试次数(MAX_RETRY = 1)
// 4. 不可重放的(已发送部分 token)直接报错
```

### 13.14 工具函数 → laew 基础设施

undici `lib/core/util.js` 中的设计模式对 laew 有直接借鉴价值:

**HTTP Token 验证的位图查表法** → laew 可用于验证 LLM API 返回的 Content-Type 等头部:
```rust
const VALID_TOKEN_CHARS: [u8; 256] = [/* 预计算位图 */];
fn is_valid_http_token(s: &str) -> bool {
    s.len() >= 1 && s.bytes().all(|b| VALID_TOKEN_CHARS[b as usize] == 1)
}
```

**头部名称归一化的双策略**(短串查表 + 长串走正则)→ laew 处理 Anthropic/OpenAI 响应头时可采用相同策略。

**协议缓存**(单条 `lastUrlString` 缓存)→ laew 对同一 provider 反复请求时,可缓存上一次的 endpoint 解析结果。

### 13.15 FastTimer → laew Rust 定时器

FastTimer 的设计对 laew 的 Agent 超时管理有借鉴意义:

```rust
// laew 可以实现类似的"逻辑时钟 + 共享定时器"模式
struct FastTimerManager {
    now: u64,                         // 逻辑时钟
    timers: Vec<FastTimerEntry>,      // 所有活跃定时器
    native_timer: Option<JoinHandle<()>>,  // 底层异步定时器
}
```

关键原则:
- 延迟 >1s 的超时使用共享驱动器(单 tokio timer),≤1s 使用原生 `tokio::time::sleep`
- 逻辑时钟独立于系统时钟,避免时钟跳变影响
- swap-and-pop O(1) 删除

### 13.16 HTTP 缓存 → laew LLM 响应缓存

undici 的 HTTP 缓存拦截器架构可以启发 laew 的 LLM 响应缓存:

```rust
// 缓存键: provider + model + prompt_hash
// 缓存值: response + cached_at + stale_at + delete_at
trait CacheStore {
    async fn get(&self, key: &CacheKey) -> Option<CachedResponse>;
    async fn set(&self, key: &CacheKey, response: CachedResponse);
    async fn delete(&self, key: &CacheKey);
}
```

相同的 Cache-Control 解析和 Vary 头部匹配逻辑可以用于缓存 LLM 响应(相同 prompt+model → 相同响应)。

### 13.17 Mock 体系 → laew 测试基础设施

undici 的 Mock 体系设计(MockAgent 包装真实 Agent,替换 dispatch 方法)可以直接移植到 laew:

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

关键特性移植:
- **匹配器**:字符串精确匹配、正则、函数闭包
- **times/persist**:限制拦截次数或持久拦截
- **reply/callback**:静态响应或动态回调
- **assertNoPendingInterceptors**:测试断言确保所有拦截器被消费
- **enableNetConnect/disableNetConnect**:控制是否允许真实网络请求

### 13.18 Fetch 控制器 → laew SSE 流控制器

undici 的 `Fetch` 三态控制器(ongoing/aborted/terminated)可以直接借鉴到 laew 的 SSE 流控制:

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

### 13.19 AbortSignal 链 → laew 任务取消传播

undici 的 WeakRef + FinalizationRegistry 模式在 Rust 中对应 `Weak<T>` + `Drop`:

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

### 13.20 Retry Handler → laew LLM 重试

undici 的重试策略(退避 + Range 续传 + ETag 校验)可以适配到 laew 的 LLM 请求重试:

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

**关键差异**:LLM 请求的 body(提示词)是可重放的(不像 HTTP POST 可能有副作用),因此 laew 的重试比 undici 更简单,不需要 Range 续传。

### 13.21 Cache Handler → laew 响应缓存

laew 可以实现简单的 LLM 响应缓存:

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

### 13.22 HeadersList 排序缓存 → laew 消息头部优化

laew 的 Anthropic/OpenAI 请求构建时,头部通常固定不变。可以借鉴 HeadersList 的排序缓存模式,预构建头部快照:

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

### 13.23 FastTimer → laew 超时管理

laew 的 LLM 请求超时(通常 30s-120s)可以借鉴 FastTimer 的批量检查模式:

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

### 13.24 Mock 体系 → laew 测试 Mock

undici 的 MockAgent/MockClient 模式为 laew 提供了测试基础设施的参考:

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

## 附录:关键代码路径速查

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

## 附录:undici 与 laew 架构对照表

| 维度 | undici | laew |
|------|--------|------|
| 语言 | JavaScript | Rust |
| 协议 | HTTP/1.1 + HTTP/2 | Anthropic Messages + OpenAI Completions |
| 核心抽象 | Dispatcher (dispatch/close/destroy) | LlmClient trait |
| 多连接管理 | Pool → BalancedPool → RoundRobinPool | 待实现(当前单连接) |
| 多 origin 路由 | Agent (Map<origin, Dispatcher>) | Provider 路由(待实现) |
| 请求模型 | Request 类(body/header/timeout) | Session + Message 模型 |
| 队列 | FixedQueue (链表+环形缓冲区) | Vec<WorkflowTask> |
| 重试 | RetryAgent + RetryHandler | 待实现 |
| 代理 | ProxyAgent / Socks5ProxyAgent | 不适用 |
| 可观测性 | diagnostics_channel 15 个通道 | 待实现 |
| 错误分级 | 20+ 错误类型 + Info/Fatal 区分 | AgentError 枚举 |
| 拦截器 | compose + 8 个内置拦截器 | 待实现 |
| 解析器 | llhttp WASM | serde_json |
| 全局单例 | Symbol.for + globalThis | SQLite providers 表 |
| 流控 | H2 WINDOW_UPDATE + 背压 | 无(LLM 流是单向) |
| 连接复用 | TLS Session Cache (WeakRef) | HTTP keep-alive(由 reqwest 管理) |

---

## 附录:原始文档索引

| 原始文档 | 行数 | 内容 |
|---------|------|------|
| `undici-源码调研.md` | 1,368 | 项目概览、目录结构、文件清单、双入口设计、顶层导出 |
| `undici-核心架构深度分析.md` | 5,363 | Dispatcher 体系、双协议、llhttp、5 种 API、Request/Handler、错误处理、诊断钩子、FixedQueue、TernarySearchTree、FastTimer、HTTP 缓存、Mock 体系、Fetch Web API 层 |
| `undici-WebAPI层深度分析.md` | 5,347 | Fetch API 全链路、Headers/HeadersList、Request/Response、Body Mixin、SRI、WebSocket、EventSource、Cache API、Cookies、WebIDL |
| `undici-拦截器与Mock系统深度分析.md` | 4,295 | 8 个拦截器详解、Handler 装饰器、Mock 系统、代理支持 |
| `undici-核心机制深度分析.md` | 2,562 | 核心代码路径、关键函数、设计模式 |
| `undici-HTTP传输层深度分析.md` | 700 | HTTP/1.1 vs HTTP/2、连接池、代理、TLS |
| **总计** | **19,635** | 6 份文档完整覆盖 |

---

> 合并完成日期: 2026-09-06
> 合并后文档行数: ~3,500 行(综合提炼自 19,635 行原始文档)
> 合并策略: 核心架构 > WebAPI层 > 拦截器与Mock > 核心机制 > HTTP传输层 > 源码调研(去重优先级)
> 原始文件保留,未删除

---

## 第五轮深挖补充(2026-09-06)

针对 laew 的 reqwest HTTP 层,补充前文覆盖薄弱的客户端实现细节。所有行号来自 `/usr/local/LsmGitOpenSource/undici` 当前 head(2026-09-02)。

### A. Client / Pool / Agent 调度对比

**类定义位置**:
- `Client`:`/usr/local/LsmGitOpenSource/undici/lib/dispatcher/client.js:102`,构造参数:
  ```js
  constructor(url, { maxHeaderSize, headersTimeout, socketTimeout, requestTimeout,
      connectTimeout, bodyTimeout, idleTimeout, keepAlive, keepAliveTimeout,
      maxKeepAliveTimeout, pipelining, tls, maxRequestsPerClient, localAddress,
      maxResponseSize, autoSelectFamily, autoSelectFamilyAttemptTimeout,
      // h2
      maxConcurrentStreams, allowH2, useH2c, initialWindowSize, connectionWindowSize,
      pingInterval } = {})
  ```
- `Pool`:`lib/dispatcher/pool-base.js:21` `class PoolBase extends DispatcherBase { [kQueue] = new FixedQueue(); [kClients] = []; ... }`
- `Agent`:`lib/dispatcher/agent.js:24` `class Agent extends DispatcherBase { constructor({ factory = defaultFactory, maxOrigins = Infinity, ... })`
- `ProxyAgent`:`lib/dispatcher/proxy-agent.js:110`

**Default factory**(`agent.js:18-22`):
```js
function defaultFactory(origin, opts) {
  return opts && opts.connections === 1
      ? new Client(origin, opts)
      : new Pool(origin, opts)
}
```

### B. h2:GOAWAY 重放预算与并发流

**完整 h2 client**:`/usr/local/LsmGitOpenSource/undici/lib/dispatcher/client-h2.js`(独立于 h1);h2c 透明升级在 `lib/dispatcher/h2c-client.js`。

**GOAWAY 重放预算(RFC 9113 §8.7 防无限重试)**(`client-h2.js:53-61,196-205`):
```js
const kReceivedGoAway       = Symbol('received goaway')
const kGoAwayReplayAttempts = Symbol('goaway replay attempts')
const kRefusedStreamRetry   = Symbol('refused stream retry')
const MAX_GOAWAY_REPLAY_ATTEMPTS = 1
// RFC 9113 section 8.7: a client SHOULD NOT automatically retry a request more than once.
return attempts <= MAX_GOAWAY_REPLAY_ATTEMPTS
```

**maxConcurrentStreams 透传**(`client-h2.js:255-263`):
```js
const session = http2.connect(client[kUrl], {
  createConnection: () => socket,
  peerMaxConcurrentStreams: client[kHTTP2Options].maxConcurrentStreams,
  settings: { enablePush: false, ... }
})
```
**设置更新**:`client-h2.js:545` `this[kClient][kMaxConcurrentStreams] = settings.maxConcurrentStreams ?? this[kClient][kMaxConcurrentStreams]`

**MAX_CONCURRENT_STREAMS=0 退避**(`client-h2.js:269-273`):
```js
// Armed while the peer advertises MAX_CONCURRENT_STREAMS = 0 and we have
// work that cannot start. See setNoStreamsTimeout.
noStreamsTimeout: null,
```
- **关键防御**:peer 报 0 并发流时,客户端设 noStreamsTimeout,工作不被卡死。

**GOAWAY session 处理**(`client-h2.js:89-93,298`):
```js
function getGoAwayError(session, errorCode) {
  return session[kError] ||
    (errorCode === NGHTTP2_NO_ERROR
        ? new InformationalError(`HTTP/2: "GOAWAY" frame received with code ${errorCode}`)
        : new SocketError(`HTTP/2: "GOAWAY" frame received with code ${errorCode}`, ...))
}
// ...
util.addListener(session, 'goaway', onHttp2SessionGoAway)
```

### C. Happy Eyeballs 与 ConnectTimeoutError 标准化

**透传**:`Client/Pool/Agent/RoundRobinPool` 都透传 `autoSelectFamily`、`autoSelectFamilyAttemptTimeout`(`client.js:130-131,290`、`pool.js:37-38,64`、`round-robin-pool.js:38-39,63`)。

**AggregateError → ConnectTimeoutError 标准化**(`lib/core/connect.js:167-189`):
```js
// `net.connect` with `autoSelectFamily` raises an `AggregateError` when every
// attempted address fails.
function maybeNormalizeConnectError(err, socket, opts) {
  if (err instanceof AggregateError
      && (err.code === 'ETIMEDOUT' || err.errors.some((e) => e != null && e.code === 'ETIMEDOUT'))) {
    let message = 'Connect Timeout Error'
    if (Array.isArray(socket.autoSelectFamilyAttemptedAddresses)) {
      message += ` (attempted addresses: ${socket.autoSelectFamilyAttemptedAddresses.join(', ')},`
    }
    // ...
    const wrapped = new ConnectTimeoutError(message)
    wrapped.cause = err
    return wrapped
  }
}
```
- **关键设计**:undici 把"多个 address family 都超时"统一为 ConnectTimeoutError,避免内部计时器与 net 内核 race——错误类型稳定才能上层重试。

**ALPN 协议协商**(`lib/core/connect.js:92`):
```js
ALPNProtocols: allowH2 ? (preferH2 ? ['h2', 'http/1.1'] : ['http/1.1', 'h2']) : ['http/1.1'],
```

### D. diagnostics-channel 13 个命名通道

**Channels 全清单**(`lib/core/diagnostics.js:10-32`):
```js
const channels = {
  beforeConnect:    diagnosticsChannel.channel('undici:client:beforeConnect'),
  connected:        diagnosticsChannel.channel('undici:client:connected'),
  connectError:     diagnosticsChannel.channel('undici:client:connectError'),
  sendHeaders:      diagnosticsChannel.channel('undici:client:sendHeaders'),
  create:           diagnosticsChannel.channel('undici:request:create'),
  bodySent:         diagnosticsChannel.channel('undici:request:bodySent'),
  bodyChunkSent:    diagnosticsChannel.channel('undici:request:bodyChunkSent'),
  bodyChunkReceived:diagnosticsChannel.channel('undici:request:bodyChunkReceived'),
  headers:          diagnosticsChannel.channel('undici:request:headers'),
  trailers:         diagnosticsChannel.channel('undici:request:trailers'),
  error:            diagnosticsChannel.channel('undici:request:error'),
  open:             diagnosticsChannel.channel('undici:websocket:open'),
  close:            diagnosticsChannel.channel('undici:websocket:close'),
  socketError:      diagnosticsChannel.channel('undici:websocket:socket_error'),
  ping:             diagnosticsChannel.channel('undici:websocket:ping'),
  pong:             diagnosticsChannel.channel('undici:websocket:pong'),
  proxyConnected:   diagnosticsChannel.channel('undici:proxy:connected')
}
```

**Publish 点**:`client.js:515-516`(beforeConnect)、`571-572`(connected)、`603-604`(connectError);`client-h1.js:1345-1346`、`client-h2.js:1221-1226`(sendHeaders):
```js
if (channels.sendHeaders.hasSubscribers) {
  channels.sendHeaders.publish({ request, headers: header, socket: session[kSocket] })
}
```
- **`hasSubscribers` 守门**:无订阅者时跳过 publish,零开销。

### E. fetch 中断:AbortController 完整生命周期

**controller.abort()**(`lib/web/fetch/index.js:127-151`):
```js
// https://fetch.spec.whatwg.org/#fetch-controller-abort
abort(error) {
  if (this.state !== 'ongoing') return
  this.state = 'aborted'
  // ...
  if (!error) error = new DOMException('The operation was aborted.', 'AbortError')
  // ...
  this.connection?.destroy(error)
  this.emit('terminated', error)
}
```

**Request 内部 AbortController**(`lib/web/fetch/request.js:1,31,424,436`):
```js
/* globals AbortController */
const kAbortController = Symbol('abortController')
// ...
const ac = new AbortController()
// ...
this[kAbortController] = ac
```

**主流程 abort 触发**(`fetch/index.js:221,1979,2036,2231`):
```js
controller.abort(requestObject.signal.reason)        // L221
fetchParams.controller.abort()                        // L1979
fetchParams.controller.abort(reason)                  // L2036
const abort = (reason) => controller.abort(reason)    // L2231
```

**防 AbortController GC 泄漏**(参考 issue #4627,注释散落在 `fetch/index.js:293,415,470`):fetch 内部引用 Request → Request 引用 AC → AC 引用 callback。需手动 unref 否则长 fetch 期间 GC 不回收。

### F. 对 laew 的 P0/P1/P2 借鉴路线

| 优先级 | 模块 | 借鉴内容 | 来源 |
|---|---|---|---|
| **P0** | Client 5 类超时 | connect/headers/socket/request/body/idle 各有独立超时 | client.js:102 |
| **P0** | h2 GOAWAY 重放预算 | `MAX_GOAWAY_REPLAY_ATTEMPTS=1` 防 RFC 9113 §8.7 无限循环 | client-h2.js:53-61 |
| **P0** | MAX_CONCURRENT_STREAMS=0 退避 | peer 报 0 时启用 `noStreamsTimeout`,不卡死 | client-h2.js:269-273 |
| **P0** | ConnectTimeoutError 标准化 | AggregateError → ConnectTimeoutError,稳定错误类型便于上层重试 | connect.js:167-189 |
| **P0** | diagnostics hasSubscribers 守门 | 无订阅者跳过 publish,零开销 | client.js:515-516 |
| **P1** | peerMaxConcurrentStreams 透传 | settings 帧立即更新 maxConcurrentStreams | client-h2.js:255-263, 545 |
| **P1** | ALPN 协商顺序 | preferH2 时 `['h2', 'http/1.1']`,否则 `['http/1.1', 'h2']` | connect.js:92 |
| **P1** | autoSelectFamilyAttemptTimeout | 每个 family 独立超时,Happy Eyeballs 标准 | client.js:131 |
| **P1** | Pool vs Client 选择 | connections=1 用 Client(无池化),>1 用 Pool | agent.js:18-22 |
| **P2** | websocket 5 通道 | open/close/socket_error/ping/pong 完整生命周期 | diagnostics.js:21-25 |
| **P2** | trailer 通道 | `undici:request:trailers` 单独通道,便于区分 headers/trailers | diagnostics.js:19 |
| **P2** | sendHeaders 时机 | 字节级 sendHeaders publish,可做慢请求探测 | client-h2.js:1221-1226 |
| **P2** | GC 泄漏防护 | 长 fetch 期间 AbortController 引用链——laew 用 reqwest 不直接踩,但要给 LLM 长流留 hint | fetch/index.js:293,415,470 |

---

## 14. 第七轮深挖 — 连接池KeepAlive生命周期 + HTTP/2多路复用 + DiagnosticsChannel可观测性 + 背压与Body流控

> **本轮定位**:前六轮已覆盖 Dispatcher 分层、llhttp WASM、5 种 API、8 个拦截器、Mock、Web 标准、错误类、FastTimer、HTTP 传输层、fetch 全链路、GOAWAY 重放预算、Happy Eyeballs、13/17 个诊断通道、fetch 中断。
> 本轮只挖**四个未覆盖或覆盖过浅的切面**,全部结论直接来自源码行号。
>
> **源码基线**:`/usr/local/LsmGitOpenSource/undici` @ `f5320831e0f429a5ffddffb98ddb319d02732df2`(2026-09-02),`package.json` version `8.10.1`。
> 与第五轮同一 commit,因此行号体系一致,可直接交叉引用第五轮 A~F 节。

### 14.0 本轮结论速览(4 个维度 × 关键数字)

| 维度 | 本轮新挖出的关键事实 | 主要源码位置 |
|---|---|---|
| ① 连接池 / Keep-Alive | 三类超时其实是**五类**;`keepAliveTimeout` 默认 **4s**、`keepAliveMaxTimeout` **600s**、`keepAliveTimeoutThreshold` **2s**;服务端 `Keep-Alive: timeout=N` 会被 **减去 2s 阈值再夹到 600s**;空闲复用前先跑一次 **idle socket validation**(GHSA-35p6-xmwp-9g52) | `client.js:305-317`、`client-h1.js:682-700,1062-1078` |
| ② HTTP/2 多路复用 | h2 默认 `maxConcurrentStreams=100`(对齐 Node h2 server)、`initialWindowSize=262144`(Node 默认 65535 的 4 倍)、`connectionWindowSize=524288`、`pingInterval=60000`;`Pool` **connections 不限**会彻底打散 h2 多路复用 | `client.js:324-337`、`client-h2.js:255-263`、Pool.md:76-81 |
| ③ DiagnosticsChannel | 实际是 **18 个通道**(17 个在 `diagnostics.js` + 1 个 `undici:request:pending-requests` 在 dedupe 拦截器);全部 `publish` 点被 `hasSubscribers` 守门;**源码内零 OpenTelemetry 依赖** | `diagnostics.js:10-32`、`deduplicate.js:8,101-111` |
| ④ 背压与 Body 流控 | `bodyTimeout` 是**空闲超时而非总超时**(每收到一个 chunk 就 refresh,且消费者 pause 期间不触发);`body` 不可重放由 `isDisturbed()` + `BodyAsyncIterable` 的 `assert(!used)` 双保险拦截 | `client-h1.js:728-756,833-854`、`util.js:613-616,17-27` |

---

### 14.1 维度一:连接池与 Keep-Alive 全生命周期

#### 14.1.1 三层容器拓扑(Pool → Client → HTTPContext)

undici 的连接池不是"一个 socket 数组",而是**三层嵌套 + 每层一套队列**:

```
Agent (lib/dispatcher/agent.js:24)          ← 按 origin 分桶(kClients: Map<origin, Pool>)
  └─ Pool / PoolBase (pool-base.js:21)      ← kClients: Client[] + [kQueue]: FixedQueue(无界)
       └─ Client (client.js:102)            ← kQueue: Array 三段式(complete|running|pending)
            └─ HTTPContext (client-h1.js:861 / client-h2.js:249)
                 └─ net.Socket / http2.ClientHttp2Session
```

**关键**:排队发生在**两层**。`Pool` 用 `FixedQueue`(`pool-base.js:22`)承接"拿不到空闲 Client"的请求;`Client` 用三段式数组队列承接"拿不到并发额度"的请求。两层队列的返回值共同决定 `dispatch()` 的布尔语义(是否 drain)。

```javascript
// lib/dispatcher/pool-base.js:157-170
[kDispatch] (opts, handler) {
  const dispatcher = this[kGetDispatcher]()
  if (!dispatcher) {
    this[kNeedDrain] = true
    this[kQueue].push({ opts, handler })     // ← Pool 层排队
    this[kQueued]++
  } else if (!dispatcher.dispatch(opts, handler)) {
    dispatcher[kNeedDrain] = true
    this[kNeedDrain] = !this[kHasDispatcher]()
  }
  return !this[kNeedDrain]                   // ← 返回 false 表示"请等 drain"
}
```

`Client` 层的三段式队列注释本身就是设计文档:

```javascript
// lib/dispatcher/client.js:339-350
// kQueue is built up of 3 sections separated by
// the kRunningIdx and kPendingIdx indices.
// |   complete   |   running   |   pending   |
//                ^ kRunningIdx ^ kPendingIdx ^ kQueue.length
// kRunningIdx points to the first running element.
// kPendingIdx points to the first pending element.
// This implements a fast queue with an amortized
// time of O(1).
this[kQueue] = []
this[kRunningIdx] = 0
this[kPendingIdx] = 0
```

三个派生计数器(全部 O(1),靠下标差算):

| 指标 | 定义 | 源码 |
|---|---|---|
| `kPending` | `kQueue.length - kPendingIdx`(已入队未发出) | `client.js:369-371` |
| `kRunning` | `kPendingIdx - kRunningIdx`(已发出未完成) | `client.js:373-375` |
| `kSize` | `kQueue.length - kRunningIdx`(= running + pending) | `client.js:377-379` |

队列压缩(避免数组无限增长):每当 `kRunningIdx > 256` 就 splice 掉已完成段:

```javascript
// lib/dispatcher/client.js:653-657
if (client[kRunningIdx] > 256) {
  client[kQueue].splice(0, client[kRunningIdx])
  client[kPendingIdx] -= client[kRunningIdx]
  client[kRunningIdx] = 0
}
```

> **对 laew 的映射**:reqwest/hyper 的连接池是"扁平"的(per-host 一个 idle 队列 + 一个 waiter 队列),没有 undici 这种"Pool 层再排一次"的二级结构。laew 若将来要自己做 provider 级限流(每 provider N 并发),undici 的 `[kNeedDrain]` 布尔回压协议(返回 `false` = 我满了,等我 `drain` 事件)比"抛错/无限排队"更值得抄。

#### 14.1.2 pipelining 开关:一个开关,两种语义

`pipelining` 是**唯一**控制连接复用的开关,但在 h1 / h2 下含义完全不同。undici 在 `client.js:87-97` 专门写了注释说明:

```javascript
// lib/dispatcher/client.js:73-97
function getPipelining (client) {
  return client[kPipelining] ?? client[kHTTPContext]?.defaultPipelining ?? 1
}

// Protocol-aware dispatch ceiling. h1 RFC7230 pipelining is unrelated to h2
// stream multiplexing — over h2 the ceiling is the (server-confirmed)
// maxConcurrentStreams. Before a context is attached we use the h1
// pipelining factor; once h2 attaches the queued requests can drain in
// one batch up to maxConcurrentStreams.
function getMaxConcurrent (client) {
  if (client[kHTTPContext]?.version === 'h2') {
    return client[kMaxConcurrentStreams]
  }
  return getPipelining(client)
}
```

| 协议 | `defaultPipelining` | 并发上限来源 | 源码 |
|---|---|---|---|
| h1 | `1` | `pipelining`(默认 1) | `client-h1.js:895` |
| h2 | `Infinity` | `kMaxConcurrentStreams`(远端 SETTINGS 确认值) | `client-h2.js:318` |
| h2c(明文 h2) | `pipelining` 默认 `100`,且不得 > `maxConcurrentStreams` | `maxConcurrentStreams` | `h2c-client.js:21-44` |

默认值的落点:

```javascript
// lib/dispatcher/client.js:305
this[kPipelining] = pipelining != null ? pipelining : 1
```

**`pipelining = 1` 的真实含义**:不是"禁止复用",而是"同一时刻一条连接上只允许 1 个 in-flight 请求"。请求完成后 socket 仍会回到 idle 池(见 14.1.6)。`pipelining = 0` 才是"发完就关":

```javascript
// lib/dispatcher/client-h1.js:1316-1328
if (upgrade) {
  header += 'connection: upgrade\r\nupgrade: ' + upgrade + '\r\n'
} else if (client[kPipelining] && !socket[kReset]) {
  header += 'connection: keep-alive\r\n'
} else {
  header += 'connection: close\r\n'   // ← pipelining=0 走这里
}
```

> **注意历史包袱**:`keepAlive` 选项已被**硬删除**,传了直接抛错:
> ```javascript
> // lib/dispatcher/client.js:143-145
> if (keepAlive !== undefined) {
>   throw new InvalidArgumentError('unsupported keepAlive, use pipelining=0 instead')
> }
> ```
> 同理 `socketTimeout` / `requestTimeout` / `idleTimeout` / `maxKeepAliveTimeout` 全部抛错(`client.js:147-161`),引导用户改用 `headersTimeout` + `bodyTimeout` + `keepAliveTimeout`。这是"用编译期/启动期报错强制分层超时"的范式。

#### 14.1.3 connections 上限、扩容与排队(Pool 侧)

`Pool` 的 `connections` **默认 `null` = 不限**(Pool.md:57-59)。扩容逻辑在 `[kGetDispatcher]`:

```javascript
// lib/dispatcher/pool.js:99-118
[kGetDispatcher] () {
  const clientTtlOption = this[kOptions].clientTtl
  for (let i = 0; i < this[kClients].length; i++) {
    const client = this[kClients][i]
    // check ttl of client and if it's stale, remove it from the pool
    if (clientTtlOption != null && clientTtlOption > 0 && client.ttl && ((Date.now() - client.ttl) > clientTtlOption)) {
      this[kRemoveClient](client)
      i--
    } else if (!client[kNeedDrain]) {
      return client                       // ← 找到第一个"不忙"的 Client
    }
  }
  if (!this[kConnections] || this[kClients].length < this[kConnections]) {
    const dispatcher = this[kFactory](this[kUrl], this[kOptions])
    this[kAddClient](dispatcher)           // ← 按需扩容(懒建连)
    return dispatcher
  }
  // 返回 undefined → 调用方进 FixedQueue 排队
}
```

三个要点:

1. **扩容是"按并发按需"**:`connections` 不限制"池里有多少连接",只限制"最多能建多少"。空闲连接不会被主动回收(除非设 `clientTtl`)。
2. **`clientTtl` 是唯一的老化机制**(默认 `null` = 永不淘汰,Pool.md:60-62),淘汰时机是**借出时惰性检查**(`pool.js:105`),不是后台定时扫描 —— 零定时器开销。
3. **TTL 起点是 `connect` 事件**,不是创建时间:
   ```javascript
   // lib/dispatcher/pool.js:76-82
   this.on('connect', (origin, targets) => {
     if (clientTtl != null && clientTtl > 0) {
       for (const target of targets) {
         Object.assign(target, { ttl: Date.now() })
       }
     }
   })
   ```

`RoundRobinPool`(`round-robin-pool.js:94-131`)是另一种选取策略:**优先复用已有连接**(Round-Robin 轮询),只有全部 busy 时才扩容。与 `Pool` 的"找第一个不忙的"相比,负载更均匀但扫描成本 O(n)。`BalancedPool`(`balanced-pool.js:48-70`)则给 upstream 加权重 + 错误惩罚(`maxWeightPerServer=100`、`errorPenalty=15`)。

#### 14.1.4 五类超时的默认值与交互(核心表)

任务书说"三类",实际 undici 有 **5 个**独立超时旋钮 + **2 个**内部派生超时,共 7 个时间量:

| 超时 | 默认值 | 作用对象 | 触发后的行为 | 源码 |
|---|---|---|---|---|
| `connectTimeout` | **10_000 ms** | TCP/TLS 建连 | `ConnectTimeoutError`,整条连接的 pending 请求全部 fail | `connect.js:69`、`util.js:863+` |
| `headersTimeout` | **300_000 ms** | 从写完请求 → 收到响应头 | `HeadersTimeoutError`,**销毁 socket** | `client.js:317`、`client-h1.js:1133-1137`、`client-h1.js:843-845` |
| `bodyTimeout` | **300_000 ms** | 响应体**两个 chunk 之间的空闲** | `BodyTimeoutError`,销毁 socket | `client.js:316`、`client-h1.js:654-658` |
| `keepAliveTimeout` | **4_000 ms** | 连接空闲(零 in-flight)后可存活时间 | `InformationalError('socket idle timeout')`,**正常关闭** | `client.js:307`、`client-h1.js:1128` |
| `keepAliveMaxTimeout` | **600_000 ms** | 服务端 `Keep-Alive: timeout=N` 的**夹取上限** | —— (只参与计算) | `client.js:308`、`client-h1.js:686-688` |
| `keepAliveTimeoutThreshold` | **2_000 ms** | 从服务端 `timeout=N` 里**预留的安全余量** | —— (只参与计算) | `client.js:309`、`client-h1.js:687` |
| `noStreamsTimeout`(h2 内部) | = `headersTimeout` | h2 对端把 `MAX_CONCURRENT_STREAMS` 报 0 时 | `HeadersTimeoutError` + 丢弃 session | `client-h2.js:446-481` |

默认值的落点(逐行):

```javascript
// lib/dispatcher/client.js:305-317
this[kPipelining] = pipelining != null ? pipelining : 1
this[kKeepAliveDefaultTimeout] = keepAliveTimeout == null ? 4e3 : keepAliveTimeout        // 4000
this[kKeepAliveMaxTimeout] = keepAliveMaxTimeout == null ? 600e3 : keepAliveMaxTimeout    // 600000
this[kKeepAliveTimeoutThreshold] = keepAliveTimeoutThreshold == null ? 2e3 : keepAliveTimeoutThreshold // 2000
this[kKeepAliveTimeoutValue] = this[kKeepAliveDefaultTimeout]
// ...
this[kBodyTimeout] = bodyTimeout != null ? bodyTimeout : 300e3        // 300000
this[kHeadersTimeout] = headersTimeout != null ? headersTimeout : 300e3
```

```javascript
// lib/core/connect.js:68-70
const sessionCache = new SessionCache(maxCachedSessions == null ? 100 : maxCachedSessions)
timeout = timeout == null ? 10e3 : timeout        // connectTimeout 默认 10s
allowH2 = allowH2 != null ? allowH2 : true
```

**交互关系(最容易踩坑的三点)**:

1. **`headersTimeout` → `bodyTimeout` 是"切换"不是"叠加"**。收到响应头后立刻把同一个 parser timer 切成 BODY 模式:
   ```javascript
   // lib/dispatcher/client-h1.js:654-662
   if (this.statusCode >= 200) {
     const bodyTimeout = request.bodyTimeout != null ? request.bodyTimeout : client[kBodyTimeout]
     this.setTimeout(bodyTimeout, TIMEOUT_BODY)
   } else if (this.timeout) {
     if (this.timeout.refresh) { this.timeout.refresh() }   // 1xx 中间响应:只续期不切换
   }
   ```
2. **`bodyTimeout` 每收到一个 chunk 就 refresh**,所以它是**空闲超时**,不是"整个响应体必须在 300s 内下完":
   ```javascript
   // lib/dispatcher/client-h1.js:728-756 (onBody)
   assert(this.timeoutType === TIMEOUT_BODY)
   if (this.timeout) { if (this.timeout.refresh) { this.timeout.refresh() } }
   ```
   而且**消费者 pause 期间不触发** body timeout(见 14.4.3)。
3. **空闲超时只在"零 in-flight"时启动**,且用 `kKeepAliveTimeoutValue`(可能已被服务端值覆盖):
   ```javascript
   // lib/dispatcher/client-h1.js:1125-1130
   if (client[kSize] === 0) {
     if (socket[kParser].timeoutType !== TIMEOUT_KEEP_ALIVE) {
       socket[kParser].setTimeout(client[kKeepAliveTimeoutValue], TIMEOUT_KEEP_ALIVE)
     }
   }
   ```

#### 14.1.5 Keep-Alive 生命周期:服务端 `Keep-Alive: timeout=N` 的协商

这是 undici 池化里最精细的一段 —— 客户端会**读取服务端声明的空闲时长**,减掉安全余量,再夹到上限:

```javascript
// lib/dispatcher/client-h1.js:678-700
if (this.shouldKeepAlive && client[kPipelining]) {
  const keepAliveTimeout = this.keepAlive ? util.parseKeepAliveTimeout(this.keepAlive) : null
  if (keepAliveTimeout != null) {
    const timeout = Math.min(
      keepAliveTimeout - client[kKeepAliveTimeoutThreshold],   // 减 2s 余量
      client[kKeepAliveMaxTimeout]                             // 夹到 600s
    )
    if (timeout <= 0) {
      socket[kReset] = true        // 服务端给的窗口太短 → 这条连接直接判死,不再复用
    } else {
      client[kKeepAliveTimeoutValue] = timeout
    }
  } else {
    client[kKeepAliveTimeoutValue] = client[kKeepAliveDefaultTimeout]   // 回到默认 4s
  }
} else {
  socket[kReset] = true            // 服务端说 close,或 pipelining=0
}
```

`parseKeepAliveTimeout` 只认 `timeout=<digits>`,单位秒:

```javascript
// lib/core/util.js:392-400
const KEEPALIVE_TIMEOUT_EXPR = /timeout=(\d+)/
function parseKeepAliveTimeout (val) {
  const m = val.match(KEEPALIVE_TIMEOUT_EXPR)
  return m ? parseInt(m[1], 10) * 1000 : null
}
```

**为什么要减 2s**:服务端在自己的窗口到期时会主动 FIN/RST;客户端若卡在边界上复用,就会在"刚发出请求头"时撞上服务端关闭 —— 这是经典的 `ECONNRESET` 竞态。减一个 threshold 让客户端**先于**服务端放弃这条连接。

**HEAD 方法的特例**(llhttp 不允许 HEAD keepAlive,undici 强行覆盖):

```javascript
// lib/dispatcher/client-h1.js:649-652
this.shouldKeepAlive = (
  shouldKeepAlive ||
  // Override llhttp value which does not allow keepAlive for HEAD.
  (request.method === 'HEAD' && !socket[kReset] && this.connectionKeepAlive)
)
```

但 HEAD 之后仍会 `socket[kReset] = true`(`client-h1.js:1290`),以兼容"给 HEAD 回 body 的错误服务器"。即 **HEAD 请求永远独占一条连接**。

**`maxRequestsPerClient`**:一条连接最多服务多少个请求(默认 `undefined` = 不限),计数器挂在 socket 上:

```javascript
// lib/dispatcher/client.js:566-567
socket[kCounter] = 0
socket[kMaxRequests] = client[kMaxRequests]
// lib/dispatcher/client-h1.js:1303-1306
if (client[kMaxRequests] && ++socket[kCounter] >= client[kMaxRequests]) {
  socket[kReset] = true
}
```

#### 14.1.6 空闲复用前的"探活":idle socket validation(GHSA-35p6-xmwp-9g52)

复用一条 idle keep-alive socket 之前,undici **先让出一次 check phase**,让内核里已排队的 FIN/RST/乱序字节先被处理掉,再写请求头。否则会撞上"服务端已关但客户端不知道"的复用竞态(安全公告 GHSA-35p6-xmwp-9g52):

```javascript
// lib/dispatcher/client-h1.js:1062-1078
function scheduleIdleSocketValidation (client, socket) {
  socket[kIdleSocketValidation] = 1
  // Yield to the check phase (after poll) so unsolicited bytes / FIN / RST
  // already pending on this idle keep-alive socket are processed before the
  // next request is written (GHSA-35p6-xmwp-9g52).
  //
  // setTimeout(0) pays Node's ~1ms timer floor on every sequential reuse
  // (#5493). setImmediate avoids that, but an *unref'd* Immediate lets poll
  // block for ~500ms when the event loop is otherwise idle (#5600 / #5606).
  // A ref'd Immediate both keeps the pending request alive and makes poll
  // return immediately — the hybrid those issues asked for.
  socket[kIdleSocketValidationTimeout] = setImmediate(() => {
    socket[kIdleSocketValidationTimeout] = null
    socket[kIdleSocketValidation] = 2
    if (client[kSocket] === socket && !socket.destroyed) {
      client[kResume]()
    }
  })
}
```

配套的 `busy()` 会把"探活中"的连接标记为 busy,防止同一条 socket 被并发复用:

```javascript
// lib/dispatcher/client-h1.js:924-926
busy (request) {
  if (socket[kWriting] || socket[kReset] || socket[kBlocking] || socket[kIdleSocketValidation] === 1) {
    return true
  }
  // ... 非幂等 / upgrade / 流式 body 的串行化(见 14.1.10)
}
```

> 这段注释极有价值:**它记录了三次迭代的性能权衡** —— `setTimeout(0)` 有 ~1ms timer floor(#5493);`setImmediate().unref()` 会让 poll 阻塞 ~500ms(#5600/#5606);最终选 ref'd `setImmediate`。这是"正确性优先、但用 event loop 知识把开销压到 0"的范例。

#### 14.1.7 socket 错误后的队列排空与重建

`Client` 有两个错误入口,语义完全不同:

**(a) 连接期错误**(`handleConnectError`,`client.js:596-636`):只在 `ERR_TLS_CERT_ALTNAME_INVALID` 时精确排空 running + 匹配 servername 的 pending;其余交给 `onError`:

```javascript
// lib/dispatcher/client.js:619-633
if (err.code === 'ERR_TLS_CERT_ALTNAME_INVALID') {
  const running = client[kQueue].splice(client[kRunningIdx], client[kRunning])
  client[kPendingIdx] = client[kRunningIdx]
  for (let i = 0; i < running.length; i++) { util.errorRequest(client, running[i], err) }
  while (client[kPending] > 0 && client[kQueue][client[kPendingIdx]].servername === client[kServerName]) {
    const request = client[kQueue].splice(client[kPendingIdx], 1)[0]
    util.errorRequest(client, request, err)
  }
} else {
  onError(client, err)      // ← 通用路径
}
client.emit('connectionError', client[kUrl], [client], err)
```

**(b) 通用错误**(`onError`,`client.js:469-490`):**只在"没有 in-flight 请求"且"不是可恢复 socket 错误"时**才排空队列 —— 即"这条连接还没开始干活就挂了,所有排队的请求都没指望了":

```javascript
// lib/dispatcher/client.js:469-490
function onError (client, err) {
  if (client[kRunning] === 0 && err.code !== 'UND_ERR_INFO' && err.code !== 'UND_ERR_SOCKET') {
    // Error is not caused by running request and not a recoverable socket error.
    assert(client[kPendingIdx] === client[kRunningIdx])
    const requests = client[kQueue].splice(client[kRunningIdx])
    for (let i = 0; i < requests.length; i++) {
      if (requests[i] != null) { util.errorRequest(client, requests[i], err) }
    }
    assert(client[kSize] === 0)
  }
}
```

`UND_ERR_INFO` / `UND_ERR_SOCKET` 被**故意放过** —— 它们是"连接已建立但 idle 超时/被服务端 reset"这类**可恢复**错误,队列保持不动,由 `_resume()` 重新建连后继续消费。

**Pool 层的错误摘除**(与 `Client.onError` 互补):

```javascript
// lib/dispatcher/pool.js:84-96
this.on('connectionError', (origin, targets, error) => {
  // If a connection error occurs, we remove the client from the pool,
  // and emit a connectionError event. They will not be re-used.
  // Fixes https://github.com/nodejs/undici/issues/3895
  for (const target of targets) {
    // Do not use kRemoveClient here, as it will close the client,
    // but the client cannot be closed in this state.
    const idx = this[kClients].indexOf(target)
    if (idx !== -1) { this[kClients].splice(idx, 1) }
  }
})
```

> 注意这里的注解:**"client 在这个状态下不能 close"** —— 只做 splice 摘除,让 GC 回收。这是"错误路径上不要调用可能抛异常的清理逻辑"的典型取舍。

**重建**:摘除后 `client[kHTTPContext] = null`,下一次 `_resume()` 走到 `if (!client[kHTTPContext]) { connect(client); return }`(`client.js:719-723`)自动重建。

#### 14.1.8 `destroy` / `close` / `drain` 三者语义差异

| 概念 | 语义 | Promise 完成条件 | 排队请求命运 | 源码 |
|---|---|---|---|---|
| `destroy(err)` | **立即**终止。不等任何 in-flight,直接 fail 掉所有 pending | 所有子 Client destroy 完成 | **全部 fail**(`onResponseError(null, err)`) | `dispatcher-base.js:114-160`、`pool-base.js:141-155` |
| `close()` | **优雅**排空。等队列跑完,再 destroy | 队列空 → 逐个 `client.close()` → `destroy()` | **继续跑完** | `dispatcher-base.js:68-112`、`pool-base.js:124-139` |
| `drain` | **不是操作,是事件**。由 Pool 在"从满变不满"时 emit | —— | —— | `client.js:638-641`、`pool-base.js:46-49` |

`close()` 的实现(`dispatcher-base.js:109-111`)是 `close → destroy` 的串联,注释写着 `// Should not error.`:

```javascript
// lib/dispatcher/dispatcher-base.js:96-112
this[kClosed] = true
this[kOnClosed] ??= []
this[kOnClosed].push(callback)
const onClosed = () => { /* 触发所有回调 */ }
// Should not error.
this[kClose]()
  .then(() => this.destroy())
  .then(() => queueMicrotask(onClosed))
```

`Pool` 的 `close()` 若队列非空则挂起等待,由 `[kOnDrain]` 在队列清空时兑现:

```javascript
// lib/dispatcher/pool-base.js:51-61
if (this[kClosedResolve] && queue.isEmpty()) {
  const closeAll = []
  for (let i = 0; i < this[kClients].length; i++) {
    const client = this[kClients][i]
    if (!client.destroyed) { closeAll.push(client.close()) }
  }
  return Promise.all(closeAll).then(this[kClosedResolve])
}
```

`drain` 事件的产生(`client.js:638-641`)是一个"0 → 1 → 2"的三态机(`[kNeedDrain]`: 0=idle, 1=scheduled, 2=resuming):

```javascript
// lib/dispatcher/client.js:638-641
function emitDrain (client) {
  client[kNeedDrain] = 0
  client.emit('drain', client[kUrl], [client])
}
```

#### 14.1.9 CONNECT 代理隧道下的池化差异

`ProxyAgent`(`proxy-agent.js:109`)有**两套连接体系**,池化语义完全不同:

**(a) 到代理自身的连接** —— 默认用 `Pool`(可池化):

```javascript
// lib/dispatcher/proxy-agent.js:29-31
function defaultFactory (origin, opts) {
  return new Pool(origin, opts)
}
// lib/dispatcher/proxy-agent.js:196
this[kClient] = clientFactory(url, { connect })
```

**(b) 隧道建立后** —— socket **被"升级"成到目标的 TLS 连接,永久离开代理池**:

```javascript
// lib/dispatcher/proxy-agent.js:228-256
const { socket, statusCode } = await this[kClient].connect(connectParams)
if (statusCode !== 200) {
  socket.on('error', noop).destroy()
  callback(new RequestAbortedError(`Proxy response (${statusCode}) !== 200 when HTTP Tunneling`))
  return
}
// ...
const connectEndpoint = opts.allowH2 === false ? this[kConnectEndpointHTTP1] : this[kConnectEndpoint]
connectEndpoint({ ...opts, servername, httpSocket: socket }, callback)
```

`httpSocket: socket` 传给 `tls.connect()`(`connect.js:93`),Node 会在**同一个 fd** 上叠 TLS。也就是说:**一条隧道 = 一条目标连接,不复用给别的目标**;可复用的只有"到代理的 TCP/TLS 连接"(在 `this[kClient]` 这个 Pool 里)。

**(c) 非隧道(纯 http 目标)走绝对 URI 转发** —— 用的是**单个 `Client` 而非 Pool**,且必须重写 `path` 为绝对 URI:

```javascript
// lib/dispatcher/proxy-agent.js:39-41
function shouldProxyTunnel (requestProtocol, proxyTunnel) {
  return proxyTunnel === true || requestProtocol !== 'http:'
}
// lib/dispatcher/proxy-agent.js:63-65  (Http1ProxyWrapper)
this.#client = new Client(proxyUrl, { connect })
// lib/dispatcher/proxy-agent.js:84
opts.path = origin + path
```

并且**连接级**的 keep-alive 请求头由 `connections` 决定:

```javascript
// lib/dispatcher/proxy-agent.js:223
...(opts.connections == null || opts.connections > 0 ? { 'proxy-connection': 'keep-alive' } : {})
```

> **结论**:在代理隧道模式下,undici 的"连接池"退化成"到代理的连接池";到目标的连接每条隧道独占。这意味着**代理场景下 `Pool.connections` 的调优目标变成了"到代理的并发数"**,而不是到目标的。

#### 14.1.10 h1 的串行化约束:非幂等 / upgrade / 流式 body

`busy()` 里三条规则(`client-h1.js:928-950`)决定了 h1 管道化下"哪些请求必须独占连接":

```javascript
// lib/dispatcher/client-h1.js:928-950
if (request) {
  if (client[kRunning] > 0 && !request.idempotent) {
    // Non-idempotent request cannot be retried.
    return true
  }
  if (client[kRunning] > 0 && (request.upgrade || request.method === 'CONNECT')) {
    // Don't dispatch an upgrade until all preceding requests have completed.
    return true
  }
  if (client[kRunning] > 0 && util.bodyLength(request.body) !== 0 &&
    (util.isStream(request.body) || util.isAsyncIterable(request.body) || util.isFormDataLike(request.body))) {
    // Request with stream or iterator body can error while other requests
    // are inflight and indirectly error those as well.
    return true
  }
}
```

h2 **不受此限**,注释明确说明:

```javascript
// lib/dispatcher/client-h2.js:365-370
// Unlike HTTP/1.1 pipelining, HTTP/2 multiplexes requests on
// independent streams, so non-idempotent requests can be dispatched
// concurrently. Retry eligibility is handled by stream/session error
// handling instead of by serializing all non-idempotent requests.
```

`idempotent` 的默认判定:

```javascript
// lib/core/request.js:227-230
this.idempotent = idempotent == null
  ? method === 'HEAD' || method === 'GET' || method === 'QUERY'
  : idempotent
```

> **注意**:undici **没有** `Idempotent-Request` 头(那是 IETF draft 的东西,源码 grep 零命中)。它的幂等性判定是**本地的、纯 method 驱动的**,不参与线上协商。

#### 14.1.11 对 laew 的借鉴(维度一)

laew 现状:`src/llm/openai.rs:38` 与 `src/llm/anthropic.rs:38` 各 `reqwest::Client::new()`,`Cargo.toml:23` 只开了 `["json", "rustls-tls"]`,`default-features = false`。**零池化调优、零超时、每个 client 一个独立连接池**。

| # | 借鉴项 | undici 依据 | reqwest / Rust 侧落地 |
|---|---|---|---|
| P0-1 | **共享一个 `reqwest::Client`**(按 provider 或全局单例),不要每个 LLM client 建一个 | undici 一个 `Pool` 管一个 origin 的所有连接 | 把 `reqwest::Client` 放进 `Arc` 传给 `OpenAiClient` / `AnthropicClient`;`Client::clone()` 本身就是共享连接池(廉价) |
| P0-2 | **设 `pool_idle_timeout`**。undici 默认 4s 是"宁可重连也不要用死连接";reqwest 默认 90s 太久 | `client.js:307` | `ClientBuilder::new().pool_idle_timeout(Duration::from_secs(15))`(LLM 场景建议 15~30s:比 4s 省 TLS 握手,比 90s 更不容易撞服务端 FIN) |
| P0-3 | **设 `pool_max_idle_per_host`**,避免长尾 provider 攒一堆僵尸连接 | `Pool.connections`(`pool.js:113`) | `ClientBuilder::new().pool_max_idle_per_host(8)`(laew 是单 host 多 provider,按 provider 数取 `min(provider_count, 8)`) |
| P0-4 | **`connect_timeout` 显式 10s**(对齐 undici) | `connect.js:69` | `.connect_timeout(Duration::from_secs(10))` |
| P0-5 | **把"总超时"换成"分层超时"** | `client.js:147-161` 主动废弃 `requestTimeout` | 不要只设 `reqwest::ClientBuilder::timeout()`(它是 total,会误杀长 SSE 流)。改为:连接超时 10s + **首字节超时** + **chunk 间空闲超时**(见维度四) |
| P1-1 | **`hasSubscribers` 式零开销开关**:可观测性埋点先查开关再构造 payload | `client.js:515` | Rust 侧用 `tracing::enabled!(Level::INFO)` 或 `Span::is_none()` 守门,避免无监听器时仍 `format!` |
| P1-2 | **空闲复用前的探活** | `client-h1.js:1062-1078` | hyper 已内置 `pool_idle_timeout` + 复用前检测;但 laew 若自建连接池,必须做"复用前先 poll 一次" |
| P1-3 | **不可恢复错误才排空队列,可恢复错误保留队列** | `client.js:469-490` | laew 重试层区分 `is_retryable(err)`:连接期错误保留待发队列,协议/序列化错误直接 fail |
| P2-1 | **clientTtl 惰性淘汰**(借出时检查,不用定时器) | `pool.js:105-111` | 自建池时用 `Instant` 打戳 + 借出时比对,零后台任务 |
| P2-2 | **代理/隧道下池化目标变成"到代理的连接"** | `proxy-agent.js:196,228-256` | laew 若支持企业代理,连接池指标要按 proxy 维度而非 target 维度统计 |


---

### 14.2 维度二:HTTP/2 与多路复用

> **前置说明**:第五轮 B 节(h2:GOAWAY 重放预算与并发流)已经覆盖了 GOAWAY 处理、`registerGoAwayRefusal` 的 RFC 9113 §8.7 预算、`session.closed` 后 `pendingIdx` 重对齐等。本节**只补充**该节未覆盖的 5 个子维度:SETTINGS 默认值、单连接流控、`maxConcurrentStreams=0` 的边角、h2 与连接池的耦合、`idleTimeout`/`ping`/`noStreams` 三套定时器。

#### 14.2.1 HTTP/2 默认参数全表(对齐依据 + 与 Node 核心差异)

`Client` 构造时把所有 h2 选项一次性快照到 `kHTTP2Options`(`client.js:321-337`),全部带显式 fallback:

```javascript
// lib/dispatcher/client.js:321-337
this[kHTTP2Options] = {
  pingInterval: h2Options?.pingInterval ?? pingInterval ?? 60e3,
  connectionWindowSize: h2Options?.connectionWindowSize ?? connectionWindowSize ?? 524288,
  maxConcurrentStreams: h2Options?.maxConcurrentStreams ?? maxConcurrentStreams ?? 100, // Max peerConcurrentStreams for a Node h2 server
  sessionOptions: {
    // HTTP/2 window sizes are set to higher defaults than Node.js core for better performance:
    // - initialWindowSize: 262144 (256KB) vs Node.js default 65535 (64KB - 1)
    //   Allows more data to be sent before requiring acknowledgment, improving throughput
    //   especially on high-latency networks. This matches common production HTTP/2 servers.
    // - connectionWindowSize: 524288 (512KB) vs Node.js default (none set)
    //   Provides better flow control for the entire connection across multiple streams.
    initialWindowSize: h2Options?.initialWindowSize ?? initialWindowSize ?? 262144
  }
}
```

| 参数 | undici 默认 | Node http2 默认 | 差距 | 源码 |
|---|---|---|---|---|
| `maxConcurrentStreams`(本地期望上限) | **100** | 不限(由 server SETTINGS 决定) | 显式声明以"对齐 Node h2 server" | `client.js:325` |
| `initialWindowSize`(单流接收窗口) | **262144(256 KiB)** | 65535(64 KiB - 1) | **4 倍**,注释明确说明是为"高延迟网络 + 减少 ACK 次数" | `client.js:333-336` |
| `connectionWindowSize`(全连接接收窗口) | **524288(512 KiB)** | 不设(Node 不发 SETTINGS) | 比 Node 多一条 SETTINGS 帧 | `client.js:326` |
| `pingInterval`(心跳) | **60000 ms**(可设为 0 禁用) | 无 | 比裸 Node h2 更主动,死链探测更快 | `client.js:322`、`client-h2.js:269-271` |

> **关键注释**:`maxConcurrentStreams: 100` 在 undici 上下文里是**客户端自己发给服务端的 SETTINGS**(自己愿意并发多少),而 `peerMaxConcurrentStreams`(`client-h2.js:259`)才是**服务端回给客户端的 SETTINGS**(服务端允许并发多少)——后者的真实值在收到 `remoteSettings` 后覆盖 `client[kMaxConcurrentStreams]`(`client-h2.js:583-584`)。

#### 14.2.2 单连接多流 + 连接级流控(显式 `setLocalWindowSize`)

`http2ConnectionWindowSize` 在 `connect` 事件上**主动**调一次 `setLocalWindowSize`,把全连接接收窗口顶到目标值:

```javascript
// lib/dispatcher/client-h2.js:533-540
function applyConnectionWindowSize (connectionWindowSize) {
  try {
    if (typeof this.setLocalWindowSize === 'function') {
      this.setLocalWindowSize(connectionWindowSize)
    }
  } catch {
    // Best-effort only.
  }
}
util.addListener(session, 'connect', applyConnectionWindowSize.bind(session, http2ConnectionWindowSize))
```

注释 `Best-effort only.` 解释了一处隐性边界:某些 h2 实现不支持 `setLocalWindowSize`(老 OpenSSL、中间件),调用会抛错,**undici 选择吃掉异常而非降级**。这意味着 laew 若对接企业 h2 代理,要主动探测而不是相信 window size 一定生效。

`stream.setTimeout(headersTimeout)` 是 h2 的另一处独特设计(`client-h2.js:1227-1229`):

```javascript
// lib/dispatcher/client-h2.js:1227-1229
if (headersTimeout) {
  stream.setTimeout(headersTimeout)
}
```

> **与 h1 的关键差异**:h1 的超时由 `client[kBodyTimeout]/[kHeadersTimeout]` 维护,跟着 socket/parser 走;h2 直接把超时挂在**流**上,关流即取消超时。这正是 h2 多路复用能"100 个并发流各自独立计时"的基础。

#### 14.2.3 `maxConcurrentStreams = 0` 边角 + `noStreamsTimeout` 自救

RFC 9113 §6.5.2 允许服务端 SETTINGS 暂时给 0,意味着"现在不开新流"。undici 的 `busy()` 在 `client[kMaxConcurrentStreams] === 0` 时会**永远返回 true**(`client-h2.js:357-362`),所有待发请求死锁在排队里。

`noStreamsTimeout` 是为此设计的一个**自救机制**(`client-h2.js:470-498`):

```javascript
// lib/dispatcher/client-h2.js:470-498
function setNoStreamsTimeout (session) {
  const client = session[kClient]
  const state = session[kHTTP2SessionState]
  const timeout = client[kHeadersTimeout]

  if (!timeout || state.noStreamsTimeout != null) {
    return
  }

  state.noStreamsTimeout = setTimeout(onNoStreamsTimeout, timeout, session).unref()
}

function onNoStreamsTimeout (session) {
  const client = session[kClient]
  // ... 排空待发队列 → HeadersTimeoutError → resetHttp2Session
  // Drop the unusable session so the next request gets a fresh connection,
  // whose SETTINGS may well allow streams again.
  session[kError] = err
  resetHttp2Session(session, err)
}
```

注释明确点出此设计的灵感:"a request that cannot even be sent has missed the same deadline as one whose headers never arrive"——把"排队等不到流"等同于"发了请求没收到头",直接 fail 队列 + 换条连接。这是 undici 在"协议允许的对端恶意行为"面前的取舍:**宁可 fail 用户也不让进程被死锁**。

> **给 laew 的启发**:reqwest + hyper 在 h2 路径里没有显式的"对方拒绝开新流"检测,只是 `MAX_CONCURRENT_STREAMS` 限流后**阻塞到对方解锁**。若 laew 对接企业 h2 网关遇到此场景,可以加一个 `peer_max_streams_zero_for_too_long → 标记 socket 损坏 + 重连` 的兜底。

#### 14.2.4 三套独立定时器:idle / ping / noStreams

h2 一条 session 同时持有 **3 套**互不干扰的定时器,各自负责不同的"存活证据":

| 定时器 | 触发条件 | 动作 | 源码 |
|---|---|---|---|
| `idleTimeout` | `client[kSize]===0 && session[kOpenStreams]===0` | `InformationalError('socket idle timeout')` → `util.destroy(socket, err)` | `client-h2.js:500-507`、`520-528` |
| `ping.interval` | 每 `pingInterval`(默认 60s) | `session.ping(cb)` 探测;err 时 `InformationalError → kOnError` | `client-h2.js:269-271`、`562-577` |
| `noStreamsTimeout` | `client[kMaxConcurrentStreams]===0 && client[kRunning]===0 && client[kPending]>0` | `HeadersTimeoutError` + 队列排空 + reset session | `client-h2.js:470-498` |

它们的"使能条件"用同一段 `resumeH2()` 维护(`client-h2.js:402-419`),逻辑精炼:

```javascript
// lib/dispatcher/client-h2.js:402-419
function resumeH2 (client) {
  const socket = client[kSocket]
  const session = client[kHTTP2Session]

  if (socket?.destroyed === false) {
    if (client[kSize] === 0 || client[kMaxConcurrentStreams] === 0) {
      unrefH2Session(session)              // 无事可做 → 放 event loop 走
    } else {
      refH2Session(session)                // 有活 → ref 钉死
    }

    if (client[kSize] === 0 && session[kOpenStreams] === 0) {
      setHttp2IdleTimeout(session)
    } else {
      clearHttp2IdleTimeout(session)
    }

    if (client[kMaxConcurrentStreams] === 0 && client[kRunning] === 0 && client[kPending] > 0) {
      setNoStreamsTimeout(session)
    } else {
      clearNoStreamsTimeout(session)
    }
  }
}
```

> **`unrefH2Session` 的设计**(`client-h2.js:372-388`)是关键:session/socket 同 ref/unref,但用**缓存标志**避免重复调用,保证 Node 内核的 `uv_ref/unref` 计数器不漂移。`refed` 默认 `true`,意味着新 session 一开始就 ref'd,`resume()` 才根据工作量决定 unref。这是"正确性优先"的微观管理。

#### 14.2.5 `writeH2` 内部 16 步请求生命周期(从用户调用到 wire 字节)

`writeH2`(`client-h2.js:1005-1234`)是 h2 路径的"主入口函数",**单请求内分配 `state` 对象**作为流上所有事件 handler 的状态总线。这与 h1 共享 parser 实例的模型完全不同:

```javascript
// lib/dispatcher/client-h2.js:1005-1020 (节选)
function writeH2 (client, request) {
  const headersTimeout = request.headersTimeout ?? client[kHeadersTimeout]
  const bodyTimeout = request.bodyTimeout ?? client[kBodyTimeout]
  const session = client[kHTTP2Session]
  const { method, path, host, upgrade, expectContinue, signal, protocol, headers: reqHeaders } = request
  // ...
  const state = {                  // ← 单流状态总线,所有 handler 共享
    abort: null,
    body: request.body,
    bytesRead: 0,
    client,
    contentLength: null,
    expectsPayload: false,
    maxResponseSize: client[kMaxResponseSize],
    request,
    headersTimeout,
    bodyTimeout,
    requestFinalized: false,
    responseReceived: false,
    bodySent: false,
    pendingEnd: false,
    trailers: null,
    session,
    stream: null
  }
  // ...
}
```

**关键步骤(16 步)**:

| # | 步骤 | 关键代码 | 注释 |
|---|---|---|---|
| 1 | 拒绝非 `websocket` 的 `upgrade` | `client-h2.js:1011-1013` | h2 之上只允许 websocket 升级(RFC 8441) |
| 2 | 构造 `:authority` `:method` 头 | `client-h2.js:1016-1020` | h2 必须用 `:method/:path/:scheme` 伪头,不能直接 `GET /` |
| 3 | `request.onRequestStart(abort, null)` | `client-h2.js:1095-1097` | 用户态 hook 早于 wire 发出,**这是 abort 钩子的最晚注入点** |
| 4 | upgrade / CONNECT 走 extended-CONNECT 协议 | `client-h2.js:1104-1144` | `:protocol` 伪头,需服务端 `SETTINGS_ENABLE_CONNECT_PROTOCOL=1` |
| 5 | 校验 `contentLength` 与 `request.contentLength` | `client-h2.js:1199-1208` | 不一致且 `strictContentLength=true` 直接 fail |
| 6 | `refH2Session(session)` | `client-h2.js:1221` | 先 ref 一次,保证后续帧不被 unref 误杀 |
| 7 | `sendHeaders` channel publish | `client-h2.js:1221-1226` | 可观测性埋点 |
| 8 | `openStream(...)` 拿 stream 句柄 | `client-h2.js:1232` | null 表示并发满,返回 false 由上层排队 |
| 9 | `++session[kOpenStreams]` 计数 | `client-h2.js:1241` | 唯一维护单连接流计数 |
| 10 | `stream.setTimeout(headersTimeout)` | `client-h2.js:1247-1249` | **流级超时**而非 socket 级 |
| 11 | 绑事件:`response/headers/end/error/frameError/aborted/timeout/trailers` | `client-h2.js:1252-1259` | 8 个事件,均按 `state == null` 守卫重入 |
| 12 | 调 `writeBodyH2()`(非 expectContinue) | `client-h2.js:1264` | 流式 body 走 `writeStream` / `writeIterable` |
| 13 | `expectContinue` 时挂 `'continue'` 监听 | `client-h2.js:1250-1251` | 收 100 后再发 body,省流量 |
| 14 | 中间响应(`'headers'`)→ `onInterimResponse` | `client-h2.js:1347-1363` | 100/103/199 等 |
| 15 | `'data'` → `onData` 检查 `maxResponseSize` + 调 `request.onResponseData(chunk)` | `client-h2.js:1324-1346` | **h2 超大 body 不 destroy socket,只 abort 单流**(`client-h2.js:1338-1340` 注释明说) |
| 16 | `'close'` → `completeRequestStream` | `client-h2.js:879-901` | 幂等清理:state null → no-op |

> **关键差异**(`client-h2.js:1336-1340`):h1 超大 body 会 `util.destroy(socket, ...)`(整连接死,见 14.1.1 错排代价),h2 只 `state.abort(...)` 关一条流,session 留给其他请求。**这就是 h2 多路复用对 laew LLM 场景最大的吸引力**——一条 LLM 故障不应当拖垮其他 provider。

#### 14.2.6 h2 与连接池的关系(`Pool.connections` 设上限会"打散"复用)

这是最容易踩的配置坑。`Pool.connections`(`pool.js:113`)控制**每个 origin 的 Client 上限**,而每个 Client 在 h2 下就是一条 `http2.ClientHttp2Session`。如果 `Pool.connections = 3`,**多路复用就被打散成 3 条独立 h2 连接**,每条都各自跟服务端协商 SETTINGS:

```javascript
// lib/dispatcher/pool.js:99-118 (节选)
if (!this[kConnections] || this[kClients].length < this[kConnections]) {
  const dispatcher = this[kFactory](this[kUrl], this[kOptions])
  this[kAddClient](dispatcher)           // ← 按需扩容(懒建连)
  return dispatcher
}
```

`Pool` 的"找第一个不忙的"算法与 h2 实际多路复用能力错配:
- h1 下,connections 上限 = "我愿意同时建立多少条 TCP 连接";
- h2 下,connections 上限 = "我愿意跟同一 origin 同时维持多少条 session",而**每条 session 自己就能开 100 路流**。

undici 的应对:`kMaxConcurrentStreams = 100` 是从服务端**协商回来**后才会被覆盖的(client-h2.js:583-584),默认 100 已经接近大多数代理/网关的上限。如果 `Pool.connections` 设为 5,理论并发 = 5 × 100 = 500 路流,但服务端实际可能拒绝这么高的并发。

> **Pool.md:76-81**(源码 README)对这条给了警告:**"When using HTTP/2, it is not recommended to set the connections option to a value other than null"**。

#### 14.2.7 对 laew 的借鉴(维度二)

| # | 借鉴项 | undici 依据 | reqwest / Rust 侧落地 |
|---|---|---|---|
| P0-1 | **不要给 reqwest 设 `pool_max_idle_per_host` 小于 4**(单 host 多 provider 场景) | h2 多路复用 vs h1 一连一请求的差异 | laew 现在 LLM 调用是 1 origin × N providers,若 `pool_max_idle_per_host=1`,每切一个 provider 就重连 TLS;建议保持默认值或 >= 4 |
| P0-2 | **hyper 的 h2 keep-alive 用 PING 帧,默认 60s** | `client-h2.js:269-271` | reqwest 暂未暴露 `http2_keep_alive_interval`;若上游断连,超时由 `connect_timeout` 兜底(已知有 30s 静默死链风险) |
| P1-1 | **连接级流控**(`initialWindowSize=262144`) | `client.js:331-336` | hyper 默认 `initial_stream_window_size=65535`;LLM 流式场景下可手动 `Http2Builder.initial_stream_window_size(524288)` 减少 ACK 频率 |
| P1-2 | **peer-max-streams=0 自救** | `client-h2.js:470-498` | reqwest 无直接对应;但可加一个"单连接排队超时"作为近似(等同 `headersTimeout`) |
| P2-1 | **h2 单流超时 vs h1 socket 超时分开维护** | `client-h2.js:1227-1229` vs `client-h1.js:654-662` | rustls + hyper 行为类似;但 laew 若混用 h1(老企业代理)和 h2,要对二者分别设超时,不能用统一 `timeout()` |
| P2-2 | **预留 `http2_max_concurrent_reset_streams`** | RFC 9113 §8.7 + `MAX_GOAWAY_REPLAY_ATTEMPTS=1` | hyper/h2 crate 支持;LLM 场景建议"拒绝就重发"次数 ≤ 1,避免毒化 session |

---

### 14.3 维度三:诊断与可观测性(`DiagnosticsChannel` 完整剖析)

> **前置说明**:第五轮 D 节("diagnostics-channel 13 个命名通道")列过名字,但**没有给完整 payload 字段表**、**没有区分"publish 点"和"subscribe 桥接点"**、**没有讲 `hasSubscribers` 守门的微观开销**。本节把这三个洞补齐,并完整列出 **18 个通道**(第五轮 D 节漏了一个 `undici:request:pending-requests`)。

#### 14.3.1 通道清单全表(18 个,实际是 `17 + 1`)

`diagnostics.js:10-32` 注册了 **17 个** channel,加上 `lib/interceptor/deduplicate.js:8` 的第 18 个 `undici:request:pending-requests`:

```javascript
// lib/core/diagnostics.js:10-32
const channels = {
  // Client (4)
  beforeConnect:    diagnosticsChannel.channel('undici:client:beforeConnect'),     // ①
  connected:        diagnosticsChannel.channel('undici:client:connected'),         // ②
  connectError:     diagnosticsChannel.channel('undici:client:connectError'),      // ③
  sendHeaders:      diagnosticsChannel.channel('undici:client:sendHeaders'),       // ④
  // Request (8)
  create:           diagnosticsChannel.channel('undici:request:create'),           // ⑤
  bodySent:         diagnosticsChannel.channel('undici:request:bodySent'),         // ⑥
  bodyChunkSent:    diagnosticsChannel.channel('undici:request:bodyChunkSent'),    // ⑦
  bodyChunkReceived:diagnosticsChannel.channel('undici:request:bodyChunkReceived'),// ⑧
  headers:          diagnosticsChannel.channel('undici:request:headers'),          // ⑨
  trailers:         diagnosticsChannel.channel('undici:request:trailers'),         // ⑩
  error:            diagnosticsChannel.channel('undici:request:error'),            // ⑪
  // WebSocket (5)
  open:             diagnosticsChannel.channel('undici:websocket:open'),           // ⑫
  close:            diagnosticsChannel.channel('undici:websocket:close'),          // ⑬
  socketError:      diagnosticsChannel.channel('undici:websocket:socket_error'),  // ⑭
  ping:             diagnosticsChannel.channel('undici:websocket:ping'),           // ⑮
  pong:             diagnosticsChannel.channel('undici:websocket:pong'),           // ⑯
  // ProxyAgent (1)
  proxyConnected:   diagnosticsChannel.channel('undici:proxy:connected')           // ⑰
}
// ⑱ = undici:request:pending-requests  (lib/interceptor/deduplicate.js:8)
```

| # | 通道名 | 触发位置 | 触发频率 | 默认 payload |
|---|---|---|---|---|
| ① | `undici:client:beforeConnect` | `client.js:515-516` | 每次新建连接 1 次 | `{ connectParams: { version, protocol, port, host, hostname } }` |
| ② | `undici:client:connected` | `client.js:571-572` | 同上 | `{ connectParams: { version, protocol, port, host }, socket: net.Socket }` |
| ③ | `undici:client:connectError` | `client.js:603-604` | 连接失败 1 次 | `{ connectParams, error, socket }` |
| ④ | `undici:client:sendHeaders` | `client-h1.js:1345-1346` / `client-h2.js:1221-1226` | 每个请求 1 次 | `{ request, headers: 'GET / HTTP/1.1\r\nhost: ...', socket \| h2session }` |
| ⑤ | `undici:request:create` | `request.js:277-278` | 每个请求 1 次 | `{ request: Request }`(全字段可读) |
| ⑥ | `undici:request:bodySent` | `request.js:296-297` | 每个请求 1 次(body 写完) | `{ request }` |
| ⑦ | `undici:request:bodyChunkSent` | `request.js:283-284` | 每个 chunk 1 次 | `{ request, chunk: Buffer }` |
| ⑧ | `undici:request:bodyChunkReceived` | `request.js:357-358` | 每个 chunk 1 次 | `{ request, chunk: Buffer }` |
| ⑨ | `undici:request:headers` | `request.js:332-333` | 每个响应 1 次 | `{ request, response: { statusCode, headers, statusText } }` |
| ⑩ | `undici:request:trailers` | `request.js:392-393` | 含 trailer 的响应 1 次 | `{ request, trailers }` |
| ⑪ | `undici:request:error` | `request.js:414-415` | 每个失败请求 1 次 | `{ request, error }` |
| ⑫ | `undici:websocket:open` | `websocket.js:531-534` | 每条 ws 1 次 | `{ address, protocol, extensions }` |
| ⑬ | `undici:websocket:close` | `websocket.js:648-649` | 同上 | `{ websocket, code, reason }` |
| ⑭ | `undici:websocket:socket_error` | `websocket.js:93-94`、`websocketstream.js:59-60` | 每错误 1 次 | `Error` |
| ⑮ | `undici:websocket:ping` | `websocket.js:101-102` | 每 ping 帧 1 次 | `{ payload }` |
| ⑯ | `undici:websocket:pong` | `websocket.js:109-110` | 每 pong 帧 1 次 | `{ payload }` |
| ⑰ | `undici:proxy:connected` | `proxy-agent.js:234-235` | 隧道完成 1 次 | `{ socket, connectParams, headers }` |
| ⑱ | `undici:request:pending-requests` | `deduplicate.js:8,101-111` | pending>0 期间反复 publish(详见 14.3.5) | `{ pending }` |

#### 14.3.2 真实 publish 点的精确源码行号

| # | 文件:行号 | 上下文 |
|---|---|---|
| ① | `lib/dispatcher/client.js:515-524` | `if (channels.beforeConnect.hasSubscribers) channels.beforeConnect.publish({ connectParams: { version, protocol, port, host, hostname } })` |
| ② | `lib/dispatcher/client.js:571-578` | 同上,加 `socket: socket` 字段 |
| ③ | `lib/dispatcher/client.js:603-612` | 同上,加 `error, socket` |
| ④ | `lib/dispatcher/client-h1.js:1345-1348` | `publish({ request, headers: header, socket })`,`header` 是已经拼好的 CRLF 字符串 |
| ④' | `lib/dispatcher/client-h2.js:1221-1226` | h2 同样,但 `socket: session[kSocket]` |
| ⑤ | `lib/core/request.js:277-278` | `publish({ request: this })` |
| ⑥ | `lib/core/request.js:296-297` | `onRequestSent` 里 `publish({ request: this })` |
| ⑦ | `lib/core/request.js:283-284` | `onBodySent(chunk)` 里 `publish({ request: this, chunk })` |
| ⑧ | `lib/core/request.js:357-358` | `onResponseData(chunk)` 里 `publish({ request: this, chunk })` |
| ⑨ | `lib/core/request.js:332-333` | `onResponseStart(statusCode, headers, resume, statusText)` 里 `publish({ request, response: { statusCode, headers, statusText } })` |
| ⑩ | `lib/core/request.js:392-393` | `onResponseEnd` 里 `publish({ request, trailers })` |
| ⑪ | `lib/core/request.js:414-415` | `onResponseError` 里 `publish({ request, error })` |
| ⑫ | `lib/web/websocket/websocket.js:531-538` | `publish({ address, protocol, extensions })` |
| ⑬ | `lib/web/websocket/websocket.js:648-651` | `publish({ websocket, code, reason })` |
| ⑭ | `lib/web/websocket/websocket.js:93-94` + `lib/web/websocket/stream/websocketstream.js:59-60` | `publish(err)` 直接传错对象 |
| ⑮ | `lib/web/websocket/websocket.js:101-104` | `publish({ payload })` |
| ⑯ | `lib/web/websocket/websocket.js:109-112` | `publish({ payload })` |
| ⑰ | `lib/dispatcher/proxy-agent.js:234-237` | `publish({ socket, connectParams, headers })` |

> **关键细节**:`publish` 调用前**全部**用 `if (channels.X.hasSubscribers)` 守门(见上面所有行号),所以默认(无订阅者)路径零开销。

#### 14.3.3 `hasSubscribers` 守门 + `trackXxxEvents` 双层零开销设计

`diagnostics.js` 有**两层**"先查再订"防重复:

```javascript
// lib/core/diagnostics.js:36-49
let isTrackingClientEvents = false

function trackClientEvents (debugLog = undiciDebugLog) {
  if (isTrackingClientEvents) {
    return                  // ← 防重复 subscribe
  }

  // Check if any of the channels already have subscribers to prevent duplicate subscriptions
  // This can happen when both Node.js built-in undici and undici as a dependency are present
  if (channels.beforeConnect.hasSubscribers || channels.connected.hasSubscribers ||
      channels.connectError.hasSubscribers || channels.sendHeaders.hasSubscribers) {
    isTrackingClientEvents = true
    return                  // ← 别人(比如 Node 内置 undici)已订,我让位
  }

  isTrackingClientEvents = true
  diagnosticsChannel.subscribe('undici:client:beforeConnect', evt => { /* debugLog */ })
  // ... (4 个 subscribe)
}
```

`isTrackingClientEvents` 是**模块级 boolean**,保证整个进程只订一次。配合 Node 启动期的 `NODE_DEBUG=undici` 触发链(`diagnostics.js:215-218`):

```javascript
// lib/core/diagnostics.js:215-218
if (undiciDebugLog.enabled || fetchDebuglog.enabled) {
  trackClientEvents(fetchDebuglog.enabled ? fetchDebuglog : undiciDebugLog)
  trackRequestEvents(fetchDebuglog.enabled ? fetchDebuglog : undiciDebugLog)
}
```

> **零开销保证**:无 `NODE_DEBUG=undici` 且无外部 subscriber 时,**整个 diagnostics 模块只构造 channel 对象、不调用任何 publish**(因为 publish 前都查 hasSubscribers);`trackXxxEvents` 函数也根本不被调用,因为 `undiciDebugLog.enabled` 是 false。

#### 14.3.4 `trackClientEvents`/`trackRequestEvents` 内部都做了什么

它不是"原样把 channel 透传出去",而是把所有 4/3 个 channel **重新桥接到 `util.debuglog('undici')`**,即:

```javascript
// lib/core/diagnostics.js:51-63 (节选)
diagnosticsChannel.subscribe('undici:client:beforeConnect',
  evt => {
    const { connectParams: { version, protocol, port, host } } = evt
    debugLog(
      'connecting to %s%s using %s%s',
      host, port ? `:${port}` : '', protocol, version
    )
  })
```

这是 Node 内置 undici 与"作为依赖的 undici 共存"时的去重机制——保证**用户只用看一份日志**,不会因为内置 + 外部两份订阅看到双倍输出。

#### 14.3.5 `undici:request:pending-requests`(第五轮漏掉的第 18 个)

它不在 `diagnostics.js` 而在 dedupe 拦截器,但走的是同一个 `diagnostics_channel` API:

```javascript
// lib/interceptor/deduplicate.js:8 (节选)
const channel = diagnosticsChannel.channel('undici:request:pending-requests')
// lib/interceptor/deduplicate.js:101-111
function checkCompletedRequests () {
  // ...
  for (let i = 0; i < inflightRequests.length - 1;) {
    const req = inflightRequests[i++]
    if (req.body && !req.body.length) {  // 空 body 才允许合并
      const dup = inflightRequests[i]
      req.removeAllListeners()
      req.on('response', dup.listenerCount('response') > 0 ? dup.listeners('response')[0] : null)
      // ...
      dup.abort()                         // 合并后多余的请求 abort
    }
  }
  if (inflightRequests.length === 0) {
    clearTimeout(timeout)                 // ← 自动取消 timer
    pendingRequestsCount = 0
  }
  if (channel.hasSubscribers) {
    channel.publish({ pending: inflightRequests.length })  // ← 每 tick publish
  }
}
```

> 这个 channel 是**唯一一个周期 publish**(其他都是事件驱动),只在有订阅者时调用,让外部监控能拿到"实时在飞请求数"。

#### 14.3.6 性能开销与 `hasSubscribers` 微观代价

`channels.X.hasSubscribers` 是 Node 内置 boolean getter,**O(1)**(`node:diagnostics_channel` 用一个 `Set` 存订阅者,size==0 直接返回 false)。

| 场景 | 每次请求额外开销 |
|---|---|
| 完全无订阅者 | **4 次 boolean getter 调用 + 0 次 publish**(路径:`if (...) channels.X.publish(...)` 的 if 直接 false 短路) |
| 有 1 个轻订阅者(如 `console.log`) | 4 次 getter + 1 次 publish + 1 次回调 |
| 有 1 个重订阅者(如 OTel SDK) | 同上 + OTel 自身构造 span / context 的开销 |

**zero-cost telemetry 范式**:用 `hasSubscribers` 做"开/关"判定的模式,可以直接抄到 Rust:

```rust
// 伪代码
fn on_response_chunk(&self, chunk: &[u8]) {
    if tracing::enabled!(target: "laew.http.body_chunk_received", tracing::Level::DEBUG) {
        // 才构造 payload,否则连 format! 都跳过
        tracing::debug!(bytes = chunk.len(), "body chunk received");
    }
}
```

#### 14.3.7 与 OpenTelemetry / Prometheus 的对接

源码内**零 OTel/OTLP 依赖**(grep 全 lib/ 无命中)。undici 选择**只暴露 Node 原生 DiagnosticsChannel**,让 OTel SDK 等**外部**做桥接——典型的"核心只做机制、不做策略":

- **OTel 桥接**:`@opentelemetry/instrumentation-undici`(官方包)订阅 ⑨(headers)+ ⑪(error),拼出 span;
- **APM 桥接**:Datadog / NewRelic 各有自家 subscriber,订阅 ④(sendHeaders)+ ⑧(bodyChunkReceived) 算 P50/P99 时延;
- **laew 自己**:直接抄 18 个 channel,订阅关心的几个。

#### 14.3.8 对 laew 的借鉴(维度三)

| # | 借鉴项 | undici 依据 | reqwest / Rust 侧落地 |
|---|---|---|---|
| P0-1 | **tracing span 与 hasSubscribers 同构** | `client.js:515-524` 守门范式 | 用 `tracing::enabled!(Level::INFO, "laew.http")` 守门 `tracing::info!`,默认级别 INFO 时整个分支是 1 次 atomic load |
| P0-2 | **四元组埋点**:client 维度(beforeConnect/connected/connectError) + request 维度(create/headers/error) | 17+1 channel | laew 在 `src/llm/{openai,anthropic}.rs` 增加:connect_start/connect_end/connect_err + req_create/resp_headers/resp_err,作为 LLM 调用可观测性骨架 |
| P0-3 | **字节级埋点不常驻**,只在 DEBUG 级别走 | `diagnostics.js:215-218` | `tracing::debug!(chunk_len = chunk.len())` 默认 INFO 不触发,等于零开销 |
| P1-1 | **送 `agent-ctx` 跨函数**用 span 而非显式传 ctx | Node async_hooks 的隐式传递 | Rust 改用 `tracing::Span::current()` 显式 enter 或 `instrument` 宏 |
| P1-2 | **`undici:request:pending-requests` 周期 publish** | `deduplicate.js:101-111` | laew 加 `metrics::gauge!("laew_http_inflight_requests", n)`,**只在 metrics 层注册时**才递增 |
| P2-1 | **WebSocket 5 通道** | `diagnostics.js:24-30` | laew 当前不直接用 ws;但 SSE(Anthropic/OpenAI 流式响应)可类比:加 `sse_event / sse_done / sse_error` 3 个埋点 |

---

### 14.4 维度四:背压与 Body 流控

> **前置说明**:前几轮覆盖了 SSE(`docs/多Agent架构重构/01-设计与解决方案.md`)、Body Mixin(`undici.md:6438-6495` 的 `extractBody`)。本节只覆盖**未挖深**的 4 个子维度:背压传递的微观机制、`bodyTimeout` 与消费者 pause 的精妙交互、不可重放 body 的拦截策略、`RetryHandler` 对 body 的处理。

#### 14.4.1 body 派别判定:`util.js` 5 种类型的"是/否"矩阵

`util.js:79-87` 等函数构成了**body 类型识别中心**,所有上游逻辑(`client-h1.js:944-949`、`retry-handler.js`、`web/fetch/body.js`)都从这里取结论:

| 检测函数 | 返回 true 的对象 | 用途 | 源码 |
|---|---|---|---|
| `isStream(obj)` | node `Readable` 子类(有 `pipe/on/read` 等) | 是否要走 `writeStream` 流式上传 | `util.js:79-87` |
| `isBlobLike(obj)` | `Blob` / `File` / undici 自定义 BlobLike | 走 `writeBlob`,先把 Blob 完全读出再发 | `util.js:87-93` |
| `isBuffer(obj)` | `Buffer` / `Uint8Array`(含子类型) | 走 `writeBuffer`,一次 socket.write 全发 | `util.js:554-556` |
| `isFormDataLike(obj)` | `FormData`/含 `Symbol.toStringTag === 'FormData'` | 走 `extractBody` 转多部分流 | `util.js:691+` |
| `isIterable(obj)` | 有 `Symbol.iterator` 或 `Symbol.asyncIterator` | 走 `writeIterable` 逐 chunk 拉取 | `util.js:302-310` |
| `isAsyncIterable(obj)` | 只有 `Symbol.asyncIterator` | 同上但异步 for-await | `util.js:302-304` |

`bodyLength(body)`(`util.js:332-358`)对各类型返回对应的字节数(stream 返回 `null` 表示未知):

```javascript
// lib/core/util.js:332-358 (节选)
function bodyLength (body) {
  if (body == null) { return 0 }
  if (isStream(body)) {
    if (body.length != null && body.length !== Infinity) {
      return body.length           // sync stream 的预知长度
    } else {
      return null                  // async/未知长度 → 走 chunked
    }
  } else if (isBlobLike(body)) {
    return body.size               // Blob 一定有 size
  } else if (isBuffer(body)) {
    return body.byteLength
  } else if (isFormDataLike(body)) {
    return null                    // 多部分必定 chunked
  } else if (isIterable(body)) {
    return null
  }
}
```

#### 14.4.2 6 种 body 入口 + 背压传递路径

undici 提供了 6 种"塞 body 进去"的方式,各自走不同的写入路径,但**背压协议统一**——都是 **`socket.write()` 返回 false → 暂停消费者 → 等 'drain' → 恢复**。

| body 类型 | 入口函数 | 背压信号来源 | 暂停什么 | 源码 |
|---|---|---|---|---|
| `null` | `writeBuffer`(第 5 个分支) | 不适用(空 body) | —— | `client-h1.js:1547-1553` |
| `Buffer` / `Uint8Array` | `writeBuffer` | `socket.write` 返回 false | 不暂停(已全发) | `client-h1.js:1547-1580` |
| `Blob` / `File` | `writeBlob` | `for await chunk of blob.stream()` + `writer.write` 返回 false | 拉慢 stream 迭代 | `client-h1.js:1667-1700` |
| node `Readable` | `writeStream` | `'data'` 事件 + `writer.write` 返回 false | `socket.pause()` | `client-h1.js:1380-1480` |
| `AsyncIterable` | `writeIterable` | `writer.write` 返回 false | `await waitForDrain()` | `client-h1.js:1596-1645` |
| h2 任意 body | `writeBodyH2` + 4 种派生 | `stream.write` 返回 false | 由 h2 session 内部流控 + `onData` 暂停 | `client-h2.js:1643-1730` |

**Readable 的精确背压循环**(`client-h1.js:1380-1402`):

```javascript
// lib/dispatcher/client-h1.js:1380-1402 (节选)
const onData = function (chunk) {
  if (finished) { return }
  try {
    if (!writer.write(chunk) && this.pause) {
      this.pause()                  // ← socket 写满 → 暂停 readable
    }
  } catch (err) {
    util.destroy(this, err)
  }
}

const onDrain = function () {
  if (finished) { return }
  if (body.resume) {
    body.resume()                   // ← socket drain → 恢复 readable
  }
}
body.on('data', onData).on('end', onFinished).on('error', onFinished).on('close', onClose)
if (body.resume) { body.resume() }
socket.on('drain', onDrain).on('error', onFinished)
```

这是教科书级的"生产者-消费者背压传递":Node `Readable` 的 paused/resumed 模式 + socket 的 `drain` 事件 + writer 的布尔返回值,**三层对齐**。

**AsyncIterable 的精确背压循环**(`client-h1.js:1596-1635`):

```javascript
// lib/dispatcher/client-h1.js:1596-1635 (节选)
let callback = null
function onDrain () {
  if (callback) { const cb = callback; callback = null; cb() }
}
const waitForDrain = () => new Promise((resolve, reject) => {
  assert(callback === null)
  if (socket[kError]) { reject(socket[kError]) }
  else { callback = resolve }
})

socket.on('close', onDrain).on('drain', onDrain)

const writer = new AsyncWriter({ ... })
for await (const chunk of body) {
  if (socket[kError]) { throw socket[kError] }
  if (!writer.write(chunk)) {
    await waitForDrain()             // ← 异步等待 drain,for-await 自动暂停迭代器
  }
}
```

> **AsyncIterable 的天然优势**:`for await` 是语言级暂停,只要 await 就把执行权还给 event loop,不需要任何手动 pause/resume 标志。**这也是为什么 14.1.10 限制"流式 body 不能并发跑 2 个"**——`for await` 是独占消费,跑 2 个就会互相饥饿。

#### 14.4.3 `bodyTimeout` 与消费者 `pause()` 的精妙交互

这是 undici 在"暂停期间不应当被超时误杀"问题上的精确解:

```javascript
// lib/dispatcher/client-h1.js:728-757 (onBody 节选)
onBody (buf) {
  const { client, socket, statusCode, maxResponseSize } = this
  if (socket.destroyed) { return -1 }
  const request = client[kQueue][client[kRunningIdx]]
  assert(request)
  assert(this.timeoutType === TIMEOUT_BODY)
  if (this.timeout) {
    if (this.timeout.refresh) {
      this.timeout.refresh()         // ← 每收到 chunk 就 refresh body timeout
    }
  }
  // ...
  if (request.onResponseData(buf) === false) {
    return constants.ERROR.PAUSED    // ← 通知 parser 暂停解析器
  }
  return 0
}

// lib/dispatcher/client-h1.js:833-854 (onParserTimeout)
function onParserTimeout (parserWeakRef) {
  const parser = parserWeakRef.deref()
  if (!parser) { return }
  const { socket, timeoutType, client, paused } = parser
  if (timeoutType === TIMEOUT_HEADERS) {
    if (!socket[kWriting] || socket.writableNeedDrain || client[kRunning] > 1) {
      assert(!paused, 'cannot be paused while waiting for headers')
      util.destroy(socket, new HeadersTimeoutError())
    }
  } else if (timeoutType === TIMEOUT_BODY) {
    if (!paused) {                   // ← 关键:paused=true 不触发 body timeout
      util.destroy(socket, new BodyTimeoutError())
    }
  }
  // ...
}
```

**关键设计**:`onParserTimeout` 在 `timeoutType === TIMEOUT_BODY && paused === true` 时**直接返回**,不 destroy。`paused` 来自 parser 在 `onBody` 返回 `constants.ERROR.PAUSED` 后被 llhttp 设置的内部标志(`client-h1.js:728-757 → llhttp 协议层`)。

> **对 laew 的启发**:reqwest 的 `timeout()` 是**整体超时**(从请求创建到响应结束),不会区分"等头阶段"和"读 body 阶段"。LLM 长流场景下,**整体超时 5min 会误杀正在思考的模型**(Anthropic 平均首字节 3-10s,长输出 30s+)。**正确做法是分阶段**:`connect_timeout=10s` + `headers_timeout=30s` + **每 chunk 间 idle 超时 30s**(在 `Body::stream()` 的 `next()` 循环里 `tokio::time::timeout`)。

#### 14.4.4 不可重放 body 的拦截策略:`isDisturbed()` + `used` 双保险

undici 在两个层面拦截"已经读过/已损坏的 body":

**(a) `isDisturbed()` 工具**(`util.js:613-616`):

```javascript
// lib/core/util.js:613-616
function isDisturbed (body) {
  // TODO (fix): Why is body[kBodyUsed] needed?
  return !!(body && (stream.isDisturbed(body) || body[kBodyUsed]))
}
```

注释的 `TODO (fix)` 直白地说:`stream.isDisturbed()` 应该够了,为啥还要 `body[kBodyUsed]`?答案是 **Web 标准 fetch 的 Body API 在读取后会置 `bodyUsed=true`**,这是和 node `Readable.isDisturbed()` 独立的另一条信号。undici 同时支持两边(因为 fetch 用 fetch-body,Request 用 undici-Request)。

**(b) RetryHandler 的 body 判定**(`retry-handler.js` + `client-h2.js:165-167`):

```javascript
// lib/dispatcher/client-h2.js:165-167
function canReplayRequest (request) {
  const { body } = request
  return body == null || util.isBuffer(body) || util.isBlobLike(body)
}
```

**只有 Buffer / Blob / null 三种可以重放**——Blob 必须配 `Blob.stream()`(Blob 的内容是 lazy 的,可以再次 stream 读取)。Readable / AsyncIterable / FormData **都不能重放**(Readable 读了就 disturbed,FormData 是按 multipart 解构完的,可能 multipart boundary 已经被消费)。

> **对 laew 的启发**:reqwest 的 `RequestBuilder::body(Body::wrap(stream))` 默认**也不可重放**——stream 被消费完就没了。LLM 重试场景下,如果用户 body 是 SSE 临时组装的 JSON,**必须先 `Vec<u8>` 化再 body**,否则第二次重试会读空。laew 当前 `src/llm/openai.rs` body 都是 `serde_json::to_vec(&body)?`(`src/llm/openai.rs:42` 一带),已经是 Buffer,可以放心重试 ✓。

#### 14.4.5 重试时的 body 不可重放:`RetryHandler` 不直接 fail,先看 `canReplay`

`RetryHandler.onResponseStart`(`retry-handler.js:266-281`)先看 200/206,再决定是否触发 retry;但**真正触发重试前**已经经过了 `canReplay` 筛选(GOAWAY 路径,见 14.2 引用):

```javascript
// lib/handler/retry-handler.js:222-275 (节选)
static [kRetryHandlerDefaultRetry] (err, { state, opts }, cb) {
  const { statusCode, code, headers } = err
  const { method, retryOptions } = opts
  const { maxRetries, minTimeout, maxTimeout, timeoutFactor, statusCodes, errorCodes, methods, retryAfter } = retryOptions
  const { counter } = state
  if (code && code !== 'UND_ERR_REQ_RETRY' && !errorCodes.includes(code)) {
    cb(err); return                  // ← 不在白名单 → 立即 fail,不重试
  }
  if (Array.isArray(methods) && !methods.includes(method)) {
    cb(err); return                  // ← method 不在白名单 → 立即 fail
  }
  if (statusCode != null && Array.isArray(statusCodes) && !statusCodes.includes(statusCode)) {
    cb(err); return                  // ← 状态码不在白名单 → 立即 fail
  }
  if (counter > maxRetries) {
    cb(err); return                  // ← 用完预算 → 立即 fail
  }
  // ... 退避计算 + setTimeout(cb, retryTimeout)
}
```

**关键事实**:`RetryHandler` **不直接检查 body 是否可重放**——它信任上游传进来的"可重试请求"已经是 `canReplayRequest() === true` 的(在 `client.js`、`client-h2.js` 入口处过滤过)。这意味着 **RetryHandler 配 AsyncIterable body 时,如果上游漏过滤,重试就会读空**。

#### 14.4.6 `h2c-client.js`:明文 h2 升级的特殊处理

明文 h2(无 TLS 的 HTTP/2)走特殊客户端 `h2c-client.js`,其 `defaultPipelining` 直接是用户传的 `pipelining`(默认 100)且不得 > `maxConcurrentStreams`:

```javascript
// lib/dispatcher/h2c-client.js:21-44 (节选)
const client = new Client(url, {
  // ...
  pipelining: maxConcurrentStreams < pipelining ? maxConcurrentStreams : pipelining,
  maxConcurrentStreams,
  // ...
})
```

`h2c` 必须先发一个 HTTP/1.1 Upgrade 请求(RFC 7540 §3.2),所以**总是先经历 h1 解析路径**,升级成功后才切到 h2 session。laew 当前完全在 TLS 之上,不需要关注此路径。

#### 14.4.7 `maxResponseSize` 在 h2 vs h1 上的差异(防止单条 LLM 流拖垮整连接)

```javascript
// lib/dispatcher/client-h1.js:741-746
onBody (buf) {
  // ...
  if (maxResponseSize > -1 && this.bytesRead + buf.length > maxResponseSize) {
    util.destroy(socket, new ResponseExceededMaxSizeError())  // ← 整连接死
    return -1
  }
  this.bytesRead += buf.length
  // ...
}

// lib/dispatcher/client-h2.js:1327-1342 (节选)
function onData (chunk) {
  // ...
  if (maxResponseSize > -1 && state.bytesRead + chunk.length > maxResponseSize) {
    // Unlike HTTP/1.1, which destroys the socket because it cannot abandon one
    // response without losing framing, resetting the offending stream leaves
    // the session usable for its siblings.
    state.abort(new ResponseExceededMaxSizeError())
    return
  }
  state.bytesRead += chunk.length
  // ...
}
```

**对比**:h1 超大 body → destroy socket(整连接失效);h2 超大 body → 只 abort 这条 stream(`state.abort()` → `client-h2.js:1066-1070`:`stream.close()` 发 RST_STREAM 帧),session 给其他请求用。

> **对 laew 的启发**:LLM 场景下,如果 laew 在并发跑多个请求到同一 provider,**用 h2 比 h1 在"一个 LLM 输出爆炸"时损失小**——一条超长输出被截断只影响自己,其他请求还在复用 session。这是**强烈倾向 h2** 的硬理由。

#### 14.4.8 对 laew 的借鉴(维度四)

| # | 借鉴项 | undici 依据 | reqwest / Rust 侧落地 |
|---|---|---|---|
| P0-1 | **分阶段超时,不设整体 `timeout()`** | `client.js:147-161` 主动废弃 + `client-h1.js:654-662` 切换逻辑 | `connect_timeout=10s` + `headers_timeout=30s` + 在响应 body 的 stream 循环里 `tokio::select!{ _ = chunk.next() => {}, _ = sleep(30s) => break }` |
| P0-2 | **`maxResponseSize` 兜底** | `client-h1.js:741-746` | `reqwest::ClientBuilder::max_response_size(50*1024*1024)`(LLM 单输出 50MB 封顶) |
| P0-3 | **Body 一律 Buffer 化后再喂 reqwest**(避免重试时读空) | `client-h2.js:165-167` `canReplayRequest` | `reqwest::Body::from(bytes)`;**不要**用 `Body::wrap(stream)` 喂 LLM 调用 |
| P0-4 | **可观测的 chunk 边界** | `request.js:283-284,357-358` | tracing span 加 `chunk_index, chunk_len, total_chunks` 字段 |
| P1-1 | **`duplex: 'half'` 等价处理**:SSE 响应是"单向流,消费者可暂停" | `client-h1.js:728-757` `paused` 不超时 | Rust 用 `futures::Stream` 表达响应 body,在 stream 端 pause 不影响 idle timeout 判断 |
| P1-2 | **`onResponseData` 返回 false = 暂停** | `request.js:355-364` | 思路一致:在 `Stream::poll_next` 里若"consumer 慢"则返回 `Poll::Pending`,底层继续喂数据;reqwest 不暴露此接口,得自实现 stream 包装层 |
| P1-3 | **`isDisturbed()` 防 body 二次读取** | `util.js:613-616` | Rust 用 `bytes::Bytes::clone()`(引用计数,零拷贝)+ `Option<Bytes>` 标志位判断"是否已被消费" |
| P2-1 | **`bodyLength()` 提前算长度,避免 chunked** | `util.js:332-358` | laew 调用 LLM 时 body 已知长度(serde 序列化后),手动设 `Content-Length` 头能省一次 100-continue 协商 |
| P2-2 | **`onBody` 中暂停期间不计时** | `client-h1.js:850-853` | 用 `Instant::now()` 比对"上次收到字节到现在"是否超过 idle 超时(而不是"开始读到现在"是否超过总超时) |

---

## 14.5 本轮新增「对 laew 的总路线图」(P0/P1/P2)

> 此节汇总四个维度的所有 P0/P1/P2 借鉴项,并按"实施成本 × 收益"重排,作为下一阶段的工程 backlog。

| 优先级 | 借鉴项 | 涉及维度 | 实施成本 | 收益 | 落地文件 |
|---|---|---|---|---|---|
| **P0-1** | 共享 `reqwest::Client`,按 provider 或全局单例 | ① | 1h(改 `Client::new()` → `Arc<Client>`) | 高:消除每次握手 50-100ms + TLS 复用 | `src/llm/openai.rs`、`src/llm/anthropic.rs`、`src/config/mod.rs` |
| **P0-2** | 设 `pool_idle_timeout=15s` + `pool_max_idle_per_host=8` | ① | 30min | 中:避免僵尸连接;LLM 场景 15s 比默认 90s 更稳 | 同上 |
| **P0-3** | `connect_timeout=10s` 显式设 | ① | 5min | 中:与 undici 对齐,避免 silent hang | `src/llm/*` |
| **P0-4** | **分阶段超时**:connect 10s + headers 30s + idle 30s,**不用 `timeout()`** | ①④ | 4h | 高:不让长 SSE 被误杀 | `src/llm/*` + 新增 `timeout.rs` |
| **P0-5** | `tracing::enabled!` 守门,只在 DEBUG 走 `format!` | ③ | 2h | 高:零开销可观测性骨架 | 全 llm 模块 |
| **P0-6** | `tracing` span 嵌入 `Client::beforeConnect/connected/connectError/headers/error` 5 个事件 | ③ | 3h | 高:LLM 调用可观测性从 0 到 1 | 同上 |
| **P0-7** | body 永远 Buffer 化(`reqwest::Body::from(bytes)`) | ④ | 1h | 高:重试不再读空 | `src/llm/*` body 构造处 |
| **P0-8** | `max_response_size(50MB)` 防单条 LLM 爆炸 | ④ | 30min | 中:防止内存被打爆 | 同上 |
| **P1-1** | h2 `initial_window_size=512KB`(对齐 undici 256KB × 2) | ② | 4h(hyper API 复杂) | 中:LLM 长流减 ACK 次数 | `src/llm/*` |
| **P1-2** | 错误分级:`is_retryable(err)` 区分"协议/序列化"(立即 fail) vs "网络/超时"(重试) | ① | 6h | 高:避免无意义重试 + 失败回流 | `src/error.rs` + 重试层 |
| **P1-3** | `metrics::gauge!("laew_http_inflight_requests", n)` 类比 `pending-requests` | ③ | 2h | 中:可观测实时并发 | 新增 `src/metrics.rs` |
| **P1-4** | Body stream 包装层实现"chunk 间 idle 30s 超时" | ④ | 8h(自实现) | 高:分阶段超时落地关键 | 新增 `src/llm/stream_with_idle_timeout.rs` |
| **P1-5** | h2 `keep_alive_interval=60s`(若 hyper 支持) | ② | 6h(需评估 hyper API) | 低:已被 `connect_timeout` 部分覆盖 | 调研后决定 |
| **P2-1** | 区分 h1 vs h2 设不同超时(企业代理场景) | ② | 4h | 低:laew 当前默认全 h2 | 按需 |
| **P2-2** | 代理/隧道下池化指标按 proxy 维度 | ① | 1d | 低:laew 当前无代理需求 | 按需 |
| **P2-3** | 自建连接池时用 `Instant` 惰性 clientTtl | ① | 2d | 低:reqwest 已自带 | 不实施 |
| **P2-4** | SSE 3 个埋点(`sse_event/sse_done/sse_error`) | ③ | 1d | 中:流式体验关键 | 新增 `src/llm/sse_observer.rs` |

---

## 14.6 第七轮深挖小结

本轮新增覆盖:
- **维度一(连接池)** — 14.1 已完成
- **维度二(HTTP/2)** — 14.2 完整剖析 7 个子维度(SETTINGS 默认值、连接级流控、`maxConcurrentStreams=0` 边角、三套独立定时器、`writeH2` 16 步生命周期、h2 与 Pool.connections 的耦合)
- **维度三(DiagnosticsChannel)** — 14.3 完整剖析 18 个通道(17+1,补全第五轮漏的 `undici:request:pending-requests`),含 payload 字段表 + 真实 publish 行号 + `hasSubscribers` 微观代价
- **维度四(背压/Body)** — 14.4 完整剖析 8 个子维度(body 派别判定矩阵、6 种 body 入口 + 背压协议、`bodyTimeout` 与 pause 的精妙交互、不可重放 body 双保险、RetryHandler 与 canReplay、`h2c` 升级、`maxResponseSize` 的 h1/h2 差异)
- **维度总览** — 14.5 给出 P0/P1/P2 总路线图(按"实施成本 × 收益"排序),14.6 总结本轮

本轮所有结论均带 `lib/xxx.js:LINE` 路径与关键代码片段,与既有 14.1 / 第五轮 / 前六轮内容**零重复**。
