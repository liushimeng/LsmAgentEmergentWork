# Undici Web API 层深度分析

> 分析日期: 2026-09-05 | 仓库: `/usr/local/LsmGitOpenSource/undici` | 分支: main

## 目录

1. [概览与架构总览](#1-概览与架构总览)
2. [Fetch API 全链路深度分析](#2-fetch-api-全链路深度分析)
3. [Subresource Integrity (SRI) 子资源完整性校验](#3-subresource-integrity-sri-子资源完整性校验)
4. [WebSocket 协议实现深度分析](#4-websocket-协议实现深度分析)
5. [EventSource / SSE 流式解析深度分析](#5-eventsource--sse-流式解析深度分析)
6. [Cache API 深度分析](#6-cache-api-深度分析)
7. [Cache 存储层 (SQLite / Memory)](#7-cache-存储层-sqlite--memory)
8. [Cookies 模块深度分析](#8-cookies-模块深度分析)
9. [WebIDL 类型系统深度分析](#9-webidl-类型系统深度分析)
10. [基础设施模块 (infra / encoding / data-url)](#10-基础设施模块)
11. [FormData 模块深度分析](#11-formdata-模块深度分析)
12. [自定义事件体系 (MessageEvent / CloseEvent / ErrorEvent)](#12-自定义事件体系)
13. [对 AI Agent HTTP 传输层的借鉴](#13-对-ai-agent-http-传输层的借鉴)
14. [跨模块设计模式汇总](#14-跨模块设计模式汇总)
15. [laew 借鉴路线图](#15-laew-借鉴路线图)

---

## 1. 概览与架构总览

### 1.1 Undici Web API 层定位

Undici 是 Node.js 官方的 HTTP/1.1 客户端实现，其 Web API 层位于 `lib/web/` 目录，实现了浏览器标准的 Web API 接口，使得 Node.js 环境能使用与浏览器一致的 API 进行网络通信。undici 的 Web API 层并非简单封装，而是严格对齐 WHATWG/W3C 规范的完整实现，每个函数注释都引用规范步骤编号。

### 1.2 模块拓扑

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

### 1.3 核心架构特征

| 特征 | 说明 |
|------|------|
| **规范驱动** | 逐行对齐 WHATWG/W3C 规范，每个函数注释引用规范步骤编号 |
| **WebIDL 类型安全** | 所有公开 API 入口均通过 WebIDL 转换器进行类型检查和转换 |
| **Brand Check** | 所有类方法入口使用 `webidl.brandCheck()` 防止非法调用 |
| **内部/外部分离** | Request/Response 拥有 inner state 和 public wrapper 两层 |
| **FinalizationRegistry** | 利用 GC 回调自动清理未消费的 ReadableStream |
| **Proxy 过滤响应** | Response 的 filtered 版本通过 Proxy 实现透明代理 |
| **流式优先** | Body 基于 ReadableStream 实现流式消费 |

### 1.4 代码规模统计

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


---

## 2. Fetch API 全链路深度分析

### 2.1 核心入口: `fetch()` 与 `fetching()`

**文件**: `lib/web/fetch/index.js` (2,426 行)

#### 2.1.1 `fetch()` 公共 API

`fetch()` 是用户调用的入口，它本身是薄壳，真正的逻辑在 `fetching()` 中。

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

**关键设计**: `fetch()` 本身只是薄壳，真正的逻辑在 `fetching()` 中。用户获得的是一个 Promise，内部通过回调函数逐步推进。这种回调驱动的模式是理解整个 Fetch 实现的关键。

#### 2.1.2 `fetching()` 主编排器

`fetching()` 是 Fetch 规范中 "Fetching" 算法的完整实现，承担了整个请求生命周期的编排：

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

#### 2.1.3 `Fetch` 类状态机

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

**三态设计**: `ongoing`(进行中) / `terminated`(被终止) / `aborted`(被中止) -- 与 WHATWG Fetch 规范严格对齐。`terminated` 表示内部错误终止，`aborted` 表示用户主动取消。

#### 2.1.4 `mainFetch()` -- 协议路由

`mainFetch()` 是整个 Fetch 的核心路由函数，负责根据 URL scheme 选择正确的处理路径：

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

#### 2.1.5 `schemeFetch()` -- 本地协议处理

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

#### 2.1.6 `httpNetworkOrCacheFetch()` -- 网络与缓存协商

这是最复杂的函数之一（约 300 行），处理 HTTP 请求的缓存协商和认证重试：

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

#### 2.1.7 `httpNetworkFetch()` -- 真正的网络分发

这是 Fetch 与 undici HTTP 引擎的桥接点，通过 `agent.dispatch()` 将请求发送到底层 HTTP 客户端：

```javascript
async function httpNetworkFetch (fetchParams, includeCredentials = false) {
  // 1. 获取 request
  const request = fetchParams.request

  // 2. 创建响应 Promise
  const response = { ... }

  // 3. 创建请求处理器 (handler) -- 这是与底层 HTTP 引擎的桥接
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

  // 5. 通过 dispatcher (agent) 发起请求 -- 这是核心桥接点
  fetchParams.dispatcher.dispatch(requestOptions, handler)

  return response
}
```

**核心桥接点**: `fetchParams.dispatcher.dispatch(requestOptions, handler)` 是 undici Web API 层与其底层 HTTP 引擎的唯一桥梁。handler 回调模式使得上层的 Fetch 规范实现无需了解底层传输细节。

#### 2.1.8 `httpRedirectFetch()` -- 重定向处理

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

### 2.2 Headers 实现

**文件**: `lib/web/fetch/headers.js` (719 行)

#### 2.2.1 HeadersList -- 底层存储

HeadersList 是 Headers 的底层存储，使用 Map 实现，同时为 Set-Cookie 提供独立的数组存储：

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

**Set-Cookie 特殊性**: Set-Cookie 是 HTTP 中唯一不能合并的 header。如果两个 Set-Cookie 被 ", " 拼接，浏览器无法正确解析。undici 用独立的 `#cookies` 数组处理，这与浏览器行为一致。

#### 2.2.2 排序优化 -- 二分插入排序

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

**设计洞察**: 二分插入排序在小数据集上比 `Array.sort()` 更快且行为更可预测。HTTP 请求的 header 数量通常在 10-20 个，这个优化是有意义的。

#### 2.2.3 Headers Guard 系统

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

### 2.3 Request 类

**文件**: `lib/web/fetch/request.js` (1,144 行)

#### 2.3.1 构造器 -- 41 步规范对齐

Request 构造器完全对齐 WHATWG Fetch 规范的 41 个步骤：

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

#### 2.3.2 AbortController 生命周期管理

undici 对 AbortController 的管理使用了 FinalizationRegistry + WeakRef 的组合：

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

**WeakRef + FinalizationRegistry 模式**: 当 Request 对象被 GC 回收但其底层 stream 未消费时，自动 abort 避免资源泄漏。这是 Web API 层独有的内存安全设计。

#### 2.3.3 `makeRequest()` -- 请求工厂

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

**30+ 字段默认值**: 每个字段都有规范定义的默认值，确保内部状态的完整性。

### 2.4 Response 类

**文件**: `lib/web/fetch/response.js` (639 行)

#### 2.4.1 Proxy 过滤响应模式

这是 undici 中最优雅的设计之一，使用 ES6 Proxy 实现响应过滤：

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
- `basic`: 同源响应，过滤 Set-Cookie 等敏感 header
- `cors`: 跨域响应，只暴露 CORS 允许的 header
- `opaque`: 不透明响应，隐藏所有信息 (status=0, url='', headers=空)
- `opaqueredirect`: 重定向的不透明响应

**Proxy 的优雅**: 无需创建 Response 的子类，Proxy 在运行时动态拦截，零额外内存开销，且对调用方完全透明。

#### 2.4.2 静态工厂方法

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

### 2.5 Body Mixin

**文件**: `lib/web/fetch/body.js` (547 行)

#### 2.5.1 `extractBody()` -- 体提取

`extractBody()` 处理所有可能的 body 类型，返回标准化的 body 对象和 Content-Type：

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

#### 2.5.2 Body 消费方法

Body mixin 提供 7 种消费方式：

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

    // Uint8Array 消费 (bytes) -- 新 API
    async bytes () {
      const buffer = await consumeBody(this)
      if (!buffer) return new Uint8Array(0)
      return new Uint8Array(buffer)
    },

    // 流式文本消费 -- 新 API
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

### 2.6 Fetch 工具函数

**文件**: `lib/web/fetch/util.js` (1,525 行)

#### 2.6.1 Referrer 策略实现

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

#### 2.6.2 `InflateStream` -- 内容解压

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

#### 2.6.3 端口安全 -- `requestBadPort()`

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

**安全设计**: 防止 Fetch 请求访问已知的不安全端口 (SMTP, Telnet, RPC, NFS 等)，避免 SSRF 攻击。

#### 2.6.4 `fullyReadBody()` -- 完全读取 body

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

### 2.7 内容编码管线

在 `lib/web/fetch/index.js` 的 `httpNetworkFetch()` 中，`onResponseStart` 回调处理 Content-Encoding：

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

**安全考量**: 最多 5 层编码限制是针对 CVE 的修复，防止解压炸弹 (zip bomb) 攻击。攻击者可能嵌套多层压缩，使最终解压后的数据量呈指数级增长。

### 2.8 fetch 全链路端到端时序图

以下是从 `undici.fetch()` 入口到 `Response` 构造完成的完整调用链时序：

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

---

## 3. Subresource Integrity (SRI) 子资源完整性校验

**文件**: `lib/web/subresource-integrity/subresource-integrity.js` (307 行)

SRI (Subresource Integrity) 是 W3C WebAppSec 规范定义的浏览器安全机制，通过密码学哈希验证子资源 (脚本/样式表) 未被篡改。undici 在 `mainFetch()` 的第 20 步完整实现了 SRI 校验流程。

### 3.1 算法集合与合规性

SRI 规范定义有效的哈希算法 token 集合为: `« "sha256", "sha384", "sha512" »`。这个集合是有序的，后面的算法更强。undici 在启动时根据 Node.js crypto 模块的实际支持情况动态裁剪：

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

  // Node.js 编译时若未启用 OpenSSL，清空整个集合
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

**关键设计**: 算法集合是模块级单例，在首次加载时一次性检测并裁剪。后续调用直接使用 `Map.prototype.has/get`，通过 bind 绑定避免重复分配：

```javascript
const isValidSRIHashAlgorithm = Map.prototype.has.bind(validSRIHashAlgorithmTokenSet)
const getSRIHashAlgorithmIndex = Map.prototype.get.bind(validSRIHashAlgorithmTokenSet)
```

### 3.2 主入口: `bytesMatch()`

`bytesMatch()` 是 SRI 校验的核心函数，遵循规范 ["Does response match metadata list?"](https://w3c.github.io/webappsec-subresource-integrity/#does-response-match-metadatalist)：

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

**降级策略**: 当 Node.js 未编译 crypto 模块时，函数直接返回 `true` (放行请求)。这是规范允许的行为 — 无效哈希默认放行。

### 3.3 元数据解析: `parseMetadata()`

解析 `integrity` 属性值 (例如 `"sha384-abc... sha512-def..."`)：

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

**合规注释**: 规范的解析更复杂 (需处理 `U+003F` 和 `U+002D` 的位置)，undici 采用简化的前 6 字符切片，对三种已知算法足够安全。

### 3.4 最强算法选择: `getStrongestMetadata()`

当 integrity 属性包含多个哈希声明时，必须选择算法强度最高的一个进行校验：

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

### 3.5 Base64 兼容匹配: `caseSensitiveMatch()`

SRI 允许 base64 (标准) 和 base64url (URL 安全) 两种编码：

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

**兼容性**: base64 与 base64url 的区别在于第 62/63 字符 (`+/` vs `-_`)，WPT 测试要求 "be liberal with padding"。

### 3.6 Fetch 流程中的 SRI 集成

在 `mainFetch()` 第 20 步，undici 在获取响应体后执行 SRI 校验：

```javascript
// lib/web/fetch/index.js 第 770-805 行
if (request.integrity) {
  const processBodyError = (reason) =>
    fetchFinale(fetchParams, makeNetworkError(reason))

  // opaque 响应或无 body → 直接失败
  if (request.responseTainting === 'opaque' || response.body == null) {
    processBodyError(response.error)
    return
  }

  const processBody = (bytes) => {
    // 关键: 调用 bytesMatch 进行 SRI 校验
    if (!bytesMatch(bytes, request.integrity)) {
      processBodyError('integrity mismatch')
      return
    }

    // 校验通过: 用字节数组构造 body
    response.body = safelyExtractBody(bytes)[0]
    fetchFinale(fetchParams, response)
  }

  // 读取全部 body 后执行校验
  fullyReadBody(response.body, processBody, processBodyError)
} else {
  fetchFinale(fetchParams, response)
}
```

### 3.7 SRI 校验完整流程图

```
Request.integrity = "sha384-abc... sha512-def..."
         │
         ▼
   response 到达 mainFetch()
         │
         ▼
   request.integrity 非空?
    │           │
   Yes          No → fetchFinale() 直接返回
    │
    ▼
   responseTainting === 'opaque'?
    │           │
   Yes          No
    │           │
    ▼           ▼
   FAIL    fullyReadBody() 读取所有字节
                │
                ▼
          parseMetadata("sha384-abc... sha512-def...")
                │
                ▼ 解析结果:
          [{alg:'sha384',val:'abc...'},
           {alg:'sha512',val:'def...'}]
                │
                ▼
          getStrongestMetadata()
                │
                ▼ 选择最强 (sha512):
          [{alg:'sha512',val:'def...'}]
                │
                ▼
          for each metadata:
            applyAlgorithmToBytes('sha512', bytes)
                │
                ▼
            crypto.hash('sha512', bytes, 'base64')
                │
                ▼
            caseSensitiveMatch(actual, expected)
                │
           ┌────┴────┐
          Match    No Match
           │          │
           ▼          ▼
        PASS       FAIL → fetchFinale(networkError)
```

### 3.8 对 laew 的借鉴

SRI 的 "内容完整性校验" 思路可应用于 Agent 场景:

1. **工具输出校验**: Bash 工具执行结果可通过哈希校验确保未被中间人篡改
2. **模型响应签名**: 对 LLM 返回的关键 JSON 字段计算哈希，Write 工具写入时校验
3. **配置完整性**: CLAUDE.md / AGENTS.md 加载时校验哈希，防止恶意篡改

---

## 4. WebSocket 协议实现深度分析

### 3.1 WebSocket 主类

**文件**: `lib/web/websocket/websocket.js` (781 行)

#### 3.1.1 私有 Handler 模式

WebSocket 使用私有 handler 对象集中管理所有回调和状态，避免 WebSocket 实例与底层传输的紧耦合：

```javascript
class WebSocket extends EventTarget {
  #handler = {
    // 回调函数
    onConnectionEstablished: (response, extensions) => ...,
    onMessage: (opcode, data) => ...,
    onParserError: (err) => ...,
    onParserDrain: () => ...,
    onSocketData: (chunk) => ...,
    onSocketError: (err) => ...,
    onSocketClose: () => ...,
    onPing: () => ...,
    onPong: () => ...,

    // 状态
    readyState: states.CONNECTING,  // 0=CONNECTING, 1=OPEN, 2=CLOSING, 3=CLOSED
    socket: null,
    closeState: new Set(),
    controller: null,
    wasEverConnected: false
  }
}
```

**设计特点**: handler 对象作为中间层，将 WebSocket 公共 API 与底层 socket 通信解耦。所有 socket 回调都通过 handler 转发，使得 WebSocket 类本身只需要处理高层逻辑。

#### 3.1.2 构造器

```javascript
constructor (url, protocols = []) {
  super()

  // 1. 标记不可构造
  webidl.util.markAsUncloneable(this)

  // 2. URL 解析和验证
  const urlRecord = new URL(url, settings.settingsObject.baseUrl)
  if (urlRecord.protocol !== 'ws:' && urlRecord.protocol !== 'wss:') {
    throw new DOMException('Invalid protocol', 'SyntaxError')
  }

  // 3. Protocol 去重检查
  const protocolSet = new Set(protocols)
  if (protocolSet.size !== protocols.length) {
    throw new DOMException('Duplicate protocol', 'SyntaxError')
  }

  // 4. 建立连接
  this.#handler.readyState = states.CONNECTING
  this.#handler.controller = establishWebSocketConnection(
    urlRecord,
    protocols,
    settings.settingsObject,
    this.#handler
  )

  // 5. 绑定 signal
  if (options?.signal) {
    options.signal.addEventListener('abort', () => {
      this.#handler.controller.abort()
    }, { once: true })
  }
}
```

#### 3.1.3 `send()` -- 消息发送

```javascript
send (data) {
  // 1. readyState 必须是 OPEN
  if (this.#handler.readyState !== states.OPEN) {
    throw new DOMException('WebSocket is not open', 'InvalidStateError')
  }

  // 2. 根据类型分发
  if (typeof data === 'string') {
    // 字符串 -> TEXT 帧 (opcode=0x01)
    const frame = new WebsocketFrameSend(Buffer.from(data, 'utf-8'))
    this.#handler.sendQueue.add(
      { opcode: opcodes.TEXT, data: frame },
      (err) => { if (err) this.#handler.onParserError(err) }
    )
  } else if (data instanceof Blob) {
    // Blob -> 异步读取后 BINARY 帧
    data.arrayBuffer().then(ab => {
      const frame = new WebsocketFrameSend(new Uint8Array(ab))
      this.#handler.sendQueue.add({ opcode: opcodes.BINARY, data: frame })
    })
  } else if (ArrayBuffer.isView(data)) {
    // TypedArray -> BINARY 帧 (opcode=0x02)
    const ab = new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
    const frame = new WebsocketFrameSend(ab)
    this.#handler.sendQueue.add({ opcode: opcodes.BINARY, data: frame })
  } else if (data instanceof ArrayBuffer) {
    // ArrayBuffer -> BINARY 帧
    const frame = new WebsocketFrameSend(new Uint8Array(data))
    this.#handler.sendQueue.add({ opcode: opcodes.BINARY, data: frame })
  }
}
```

#### 3.1.4 `#onConnectionEstablished()` -- 连接建立

```javascript
#onConnectionEstablished (response, extensions) {
  this.#handler.socket = response.socket
  this.#handler.readyState = states.OPEN

  // 创建 ByteParser (接收解析器)
  const parser = new ByteParser(this.#handler, extensions)
  parser.on('drain', () => this.#handler.onParserDrain())
  parser.on('error', (err) => this.#handler.onParserError(err))
  this.#handler.parser = parser

  // 创建 SendQueue (发送队列)
  this.#handler.sendQueue = new SendQueue(this.#handler.socket)

  // 触发 open 事件
  this.dispatchEvent(new Event('open'))
}
```

#### 3.1.5 `#onMessage()` -- 消息接收

```javascript
#onMessage (type, data) {
  let message

  if (type === opcodes.TEXT) {
    // 文本帧 -> UTF-8 解码
    message = utf8Decode(data)
    // 如果解码失败, 触发连接失败
    if (message === undefined) {
      failWebsocketConnection(this.#handler, 1007, 'Invalid UTF-8')
      return
    }
  } else if (type === opcodes.BINARY) {
    // 二进制帧 -> 根据 binaryType 决定格式
    if (this.#binaryType === 'blob') {
      message = new Blob([data])
    } else {
      message = new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
    }
  }

  // 触发 message 事件
  this.dispatchEvent(new MessageEvent('message', { data: message }))
}
```

#### 3.1.6 `#onSocketClose()` -- 连接关闭

```javascript
#onSocketClose (code, reason) {
  // 1. 更新状态
  this.#handler.readyState = states.CLOSED

  // 2. 确定关闭码
  // 如果服务端发送了 close frame, 使用其 code
  // 否则使用默认值:
  // - 有 socket error -> 1006 (Abnormal Closure)
  // - 正常关闭 -> 1005 (No Status Received)
  if (this.#handler.closeState.has(sentCloseFrameState.RECEIVED)) {
    // 正常关闭流程
  } else {
    // 异常关闭
    code = 1006
  }

  // 3. 触发 close 事件
  this.dispatchEvent(new CloseEvent('close', {
    wasClean: this.#handler.closeState.has(sentCloseFrameState.RECEIVED),
    code: code ?? 1005,
    reason: reason ?? ''
  }))

  // 4. 清理资源
  if (this.#handler.socket && !this.#handler.socket.destroyed) {
    this.#handler.socket.destroy()
  }
}
```

### 3.2 WebSocket 连接建立

**文件**: `lib/web/websocket/connection.js` (330 行)

#### 3.2.1 `establishWebSocketConnection()` -- 完整握手

```javascript
function establishWebSocketConnection (urlRecord, protocols, client, handler, options) {
  // 1. 创建 nonce key (16 字节随机, Base64 编码)
  const key = crypto.randomBytes(16).toString('base64')

  // 2. 构建请求
  const request = makeRequest({
    urlList: [urlRecord],
    client,
    serviceWorkers: 'none',
    mode: 'websocket',         // WebSocket 专用模式
    credentials: 'include',    // 默认发送凭据
    cache: 'no-store',         // 不使用缓存
    redirect: 'error'          // 不允许重定向
  })

  // 3. 设置 WebSocket 特有头 (RFC 6455)
  request.headersList.set('upgrade', 'websocket')
  request.headersList.set('connection', 'Upgrade')
  request.headersList.set('sec-websocket-key', key)
  request.headersList.set('sec-websocket-version', '13')

  if (protocols.length > 0) {
    request.headersList.set('sec-websocket-protocol', protocols.join(', '))
  }

  // 4. 处理 permessage-deflate 扩展
  if (options?.perMessageDeflate) {
    request.headersList.set('sec-websocket-extensions',
      'permessage-deflate; client_max_window_bits')
  }

  // 5. 通过 fetching() 发起请求
  const controller = fetching({
    request,
    dispatcher: options?.dispatcher,

    processResponse (response) {
      // 验证握手响应 (RFC 6455 Section 4.2.2)

      // 1. 状态码必须是 101 Switching Protocols
      if (response.status !== 101) {
        failWebsocketConnection(handler, 1002, 'Expected 101')
        return
      }

      // 2. 验证 Upgrade: websocket (不区分大小写)
      const upgrade = response.headersList.get('upgrade')
      if (!upgrade || upgrade.toLowerCase() !== 'websocket') {
        failWebsocketConnection(handler, 1002, 'Invalid Upgrade header')
        return
      }

      // 3. 验证 Connection: Upgrade (不区分大小写)
      const connection = response.headersList.get('connection')
      if (!connection || connection.toLowerCase() !== 'upgrade') {
        failWebsocketConnection(handler, 1002, 'Invalid Connection header')
        return
      }

      // 4. 验证 Sec-WebSocket-Accept (SHA-1 哈希)
      const accept = response.headersList.get('sec-websocket-accept')
      const expectedAccept = crypto
        .createHash('sha1')
        .update(key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')
        .digest('base64')

      if (accept !== expectedAccept) {
        failWebsocketConnection(handler, 1002, 'Invalid Sec-WebSocket-Accept')
        return
      }

      // 5. 处理 Sec-WebSocket-Extensions (permessage-deflate)
      // 6. 处理 Sec-WebSocket-Protocol (子协议协商)

      handler.wasEverConnected = true
      handler.onConnectionEstablished(response, parsedExtensions)
    },

    onRequestUpgrade (request, socket, head) {
      // HTTP 升级回调: 保存 socket 引用
      handler.socket = socket
    }
  })

  return controller
}
```

**WebSocket UID**: `258EAFA5-E914-47DA-95CA-C5AB0DC85B11` 是 RFC 6455 定义的固定 GUID，用于 Sec-WebSocket-Accept 的 SHA-1 计算。握手的安全性基于这个 GUID 的不可预测性（攻击者无法伪造正确的 Accept 值）。

#### 3.2.2 HTTP/2 WebSocket 协商 (RFC 8441)

undici 完整支持 RFC 8441 (Bootstrapping WebSockets with HTTP/2)，使用 Extended CONNECT Protocol：

```javascript
// connection.js processResponse 中的 HTTP/2 特殊处理
if (response.type === 'error' || response.status !== 101) {
  // HTTP/1.1 期望 101 Switching Protocols
  if (response.socket?.session == null) {
    failWebsocketConnection(handler, 1002, 'Received network error or non-101')
    return
  }

  // HTTP/2 期望 200 (CONNECT 成功)
  if (response.status !== 200) {
    failWebsocketConnection(handler, 1002, 'Received network error or non-200')
    return
  }
}
```

**降级策略**: 当 HTTP/2 服务器不支持 Extended CONNECT 时，`onResponseError` 回调会捕获 `UND_ERR_INFO: HTTP/2: Extended CONNECT protocol not supported by server` 错误并自动降级到 HTTP/1.1:

```javascript
// httpNetworkFetch → dispatch 中的 onResponseError
if (
  request.mode === 'websocket' &&
  allowH2 !== false &&
  error?.code === 'UND_ERR_INFO' &&
  error?.message === 'HTTP/2: Extended CONNECT protocol not supported by server'
) {
  // 降级到 HTTP/1.1 重试
  resolve(dispatchWithProtocolPreference(body, false))
  return
}
```

#### 3.2.3 握手验证细节 (RFC 6455 Section 4.2.2)

服务器握手验证包含 6 步 (见 connection.js):

| 步骤 | 验证项 | 失败码 |
|------|--------|--------|
| 1 | 状态码 = 101 (H1) 或 200 (H2) | 1002 |
| 2 | `Upgrade: websocket` (不区分大小写) | 1002 |
| 3 | `Connection: Upgrade` (不区分大小写) | 1002 |
| 4 | `Sec-WebSocket-Accept` = SHA-1(key + GUID) base64 | 1002 |
| 5 | `Sec-WebSocket-Extensions` 必须包含客户端请求的扩展 | 1002 |
| 6 | `Sec-WebSocket-Protocol` 必须是客户端请求的子集 | 1002 |

```javascript
// 4. Sec-WebSocket-Accept 验证
const secWSAccept = response.headersList.get('Sec-WebSocket-Accept')
const digest = crypto.hash('sha1', keyValue + uid, 'base64')  // uid = GUID
if (secWSAccept !== digest) {
  failWebsocketConnection(handler, 1002, 'Incorrect hash')
  return
}

// 5. Extension 验证
const secExtension = response.headersList.get('Sec-WebSocket-Extensions')
if (secExtension !== null) {
  extensions = parseExtensions(secExtension)
  if (!extensions.has('permessage-deflate')) {
    failWebsocketConnection(handler, 1002, 'Extensions mismatch')
    return
  }
}

// 6. Protocol 验证
const secProtocol = response.headersList.get('Sec-WebSocket-Protocol')
if (secProtocol !== null) {
  const requestProtocols = getDecodeSplit('sec-websocket-protocol', request.headersList)
  if (!requestProtocols.includes(secProtocol)) {
    failWebsocketConnection(handler, 1002, 'Protocol mismatch')
    return
  }
}
```

#### 3.2.4 关闭握手状态机

WebSocket 关闭使用双向 "closing handshake"，通过 `sentCloseFrameState` 集合追踪状态:

```javascript
// constants.js
const sentCloseFrameState = {
  SENT: 1,      // 本端已发送 Close 帧
  RECEIVED: 2   // 对端已发送 Close 帧 (本端已接收)
}
```

`closeWebSocketConnection()` 实现 (connection.js):

```javascript
function closeWebSocketConnection (object, code, reason, validate = false) {
  // 1. 验证 close code
  if (validate) validateCloseCodeAndReason(code, reason)

  // 2. 根据当前状态分发
  if (isClosed(object.readyState) || isClosing(object.readyState)) {
    // 已在关闭中 → 不操作
  } else if (!isEstablished(object.readyState)) {
    // 连接未建立 → 失败并转 CLOSING
    failWebsocketConnection(object)
    object.readyState = states.CLOSING
  } else if (!closeState.has(SENT) && !closeState.has(RECEIVED)) {
    // 首次关闭: 发送 Close 帧
    if (reason.length !== 0 && code === null) {
      code = 1000  // 有 reason 但无 code → 默认 1000 Normal Closure
    }

    // 构造 Close 帧 payload
    if (code === null && reason.length === 0) {
      frame.frameData = emptyBuffer  // 无 body
    } else if (code !== null && reason === null) {
      frame.frameData = Buffer.allocUnsafe(2)
      frame.frameData.writeUInt16BE(code, 0)
    } else {
      frame.frameData = Buffer.allocUnsafe(2 + Buffer.byteLength(reason))
      frame.frameData.writeUInt16BE(code, 0)
      frame.frameData.write(reason, 2, 'utf-8')
    }

    object.socket.write(frame.createFrame(opcodes.CLOSE))
    object.closeState.add(SENT)
    object.readyState = states.CLOSING
  } else {
    // 对端已发送 Close 或本端已发送 → 仅标记 CLOSING
    object.readyState = states.CLOSING
  }
}
```

**关闭状态图**:

```
            Client                Server
              │                     │
              │──── Close Frame ───>│    Client 主动关闭
              │                     │
              │<─── Close Frame ────│    Server 回复 (echo code)
              │                     │
              │    TCP Close        │
              │<───────────────────>│
              │                     │

wasClean = closeState.has(SENT) && closeState.has(RECEIVED)
```

接收端收到 Close 帧后自动回复 (receiver.js `parseControlFrame`):

```javascript
if (opcode === opcodes.CLOSE) {
  // 自动回复 Close (echo code)
  if (!closeState.has(SENT) && !closeState.has(RECEIVED)) {
    const body = code ? Buffer.from(code.toString()) : emptyBuffer
    const closeFrame = new WebsocketFrameSend(body)
    socket.write(closeFrame.createFrame(opcodes.CLOSE))
    closeState.add(SENT)
  }

  readyState = states.CLOSING
  closeState.add(RECEIVED)
  return false  // 停止解析
}
```

### 3.3 帧编解码

**文件**: `lib/web/websocket/frame.js` (128 行)

#### 3.3.1 帧发送 -- `WebsocketFrameSend`

```javascript
class WebsocketFrameSend {
  #data

  constructor (data) {
    this.#data = data
  }

  createFrame (opcode) {
    const data = this.#data
    // 4 字节随机 mask key (客户端必须 mask)
    const mask = crypto.randomFillSync(Buffer.allocUnsafe(4))

    // 计算帧头长度
    let headerLength = 2 // 最小帧头 (2 字节)

    if (data.length > 125) {
      if (data.length > 65535) {
        headerLength += 8 // 64 位长度
      } else {
        headerLength += 2 // 16 位长度
      }
    }
    // +4 for mask key
    const head = Buffer.allocUnsafe(headerLength + 4)

    // 第 1 字节: FIN=1 + opcode
    head[0] = 0x80 | opcode  // FIN bit 置 1

    // 第 2 字节: MASK=1 + payload length
    let offset = 2
    if (data.length > 65535) {
      head[1] = 0x80 | 127  // MASK=1 + 127 表示后续 64 位长度
      // 写入 64 位长度 (大端序, 高 32 位通常为 0)
      head.writeUInt32BE(0, 2)
      head.writeUInt32BE(data.length, 6)
      offset = 10
    } else if (data.length > 125) {
      head[1] = 0x80 | 126  // MASK=1 + 126 表示后续 16 位长度
      head.writeUInt16BE(data.length, 2)
      offset = 4
    } else {
      head[1] = 0x80 | data.length  // MASK=1 + 直接长度 (7 位)
    }

    // 写入 4 字节 mask key
    mask.copy(head, offset)

    // 对 payload 应用 mask (XOR 运算)
    const maskedData = Buffer.allocUnsafe(data.length)
    for (let i = 0; i < data.length; i++) {
      maskedData[i] = data[i] ^ mask[i % 4]
    }

    return Buffer.concat([head, maskedData])
  }
}
```

#### 3.3.2 快速文本帧 -- `createFastTextFrame()`

```javascript
function createFastTextFrame (data) {
  // 优化路径: 返回 [head, buffer] 元组, 避免 Buffer.concat
  const mask = crypto.randomFillSync(Buffer.allocUnsafe(4))

  // ... 计算帧头 (与 createFrame 类似)

  // 应用 mask
  const maskedData = Buffer.allocUnsafe(data.length)
  for (let i = 0; i < data.length; i++) {
    maskedData[i] = data[i] ^ mask[i % 4]
  }

  return [head, maskedData]
  // 使用时:
  // socket.cork()
  // socket.write(head)
  // socket.write(maskedData)
  // socket.uncork()
}
```

**性能优化**: 避免 `Buffer.concat()` 的额外拷贝，利用 TCP cork/uncork 合并小写入。cork 告诉内核暂缓发送，直到 uncork 时一次性发送所有数据，减少系统调用次数。

### 3.4 接收解析器

**文件**: `lib/web/websocket/receiver.js` (508 行)

#### 3.4.1 ByteParser 状态机

ByteParser 是整个 WebSocket 接收的核心，使用 4 状态状态机逐字节解析帧：

```javascript
class ByteParser extends Writable {
  #state = parserStates.INFO  // INFO | PAYLOADLENGTH_16 | PAYLOADLENGTH_64 | READ_DATA

  // 帧解析字段
  #info = {}        // FIN, RSV1/2/3, opcode
  #masked = false
  #payloadLength = 0
  #maskKey = null
  #fragmented = Buffer.alloc(0)

  // 缓冲区管理
  #buffers = []
  #bufferedBytes = 0
  #fragments = []

  _write (chunk, encoding, callback) {
    // 将新数据加入缓冲区
    this.#buffers.push(chunk)
    this.#bufferedBytes += chunk.length

    // 主解析循环: 持续解析直到数据不足
    while (true) {
      if (this.#state === parserStates.INFO) {
        // 需要至少 2 字节 (帧头最小长度)
        if (!this.#consume(2)) break

        const [first, second] = this.#consumeBytes(2)

        // 第 1 字节: FIN + RSV + opcode
        this.#info.fin = (first & 0x80) !== 0
        this.#info.rsv1 = (first & 0x40) !== 0
        this.#info.rsv2 = (first & 0x20) !== 0
        this.#info.rsv3 = (first & 0x10) !== 0
        this.#info.opcode = first & 0x0F

        // 第 2 字节: MASK + payload length
        this.#masked = (second & 0x80) !== 0
        this.#payloadLength = second & 0x7F

        // RSV 位验证
        if (this.#info.rsv1 || this.#info.rsv2 || this.#info.rsv3) {
          // 如果没有协商 permessage-deflate, rsv 位必须为 0
          if (!this.#extensions) {
            callback(new Error('RSV bit set without extension'))
            return
          }
        }

        // 根据 payload length 决定下一步状态
        if (this.#payloadLength === 126) {
          this.#state = parserStates.PAYLOADLENGTH_16
        } else if (this.#payloadLength === 127) {
          this.#state = parserStates.PAYLOADLENGTH_64
        } else {
          this.#state = parserStates.READ_DATA
        }
      }

      if (this.#state === parserStates.PAYLOADLENGTH_16) {
        // 需要 2 字节 16 位长度 + 4 字节 mask key
        if (!this.#consume(6)) break
        const lengthBytes = this.#consumeBytes(2)
        this.#payloadLength = lengthBytes.readUInt16BE(0)
        if (this.#masked) this.#maskKey = this.#consumeBytes(4)
        this.#state = parserStates.READ_DATA
      }

      if (this.#state === parserStates.PAYLOADLENGTH_64) {
        // 需要 8 字节 64 位长度 + 4 字节 mask key
        if (!this.#consume(12)) break
        const lengthBytes = this.#consumeBytes(8)
        // 高 32 位应为 0 (Node.js buffer 最大 2GB)
        this.#payloadLength = lengthBytes.readUInt32BE(4)
        if (this.#masked) this.#maskKey = this.#consumeBytes(4)
        this.#state = parserStates.READ_DATA
      }

      if (this.#state === parserStates.READ_DATA) {
        // 等待足够的数据
        if (!this.#consume(this.#payloadLength)) break

        const data = this.#consumeBytes(this.#payloadLength)

        // 应用 mask (客户端数据必须被 mask)
        if (this.#masked && this.#maskKey) {
          for (let i = 0; i < data.length; i++) {
            data[i] ^= this.#maskKey[i % 4]
          }
        }

        // 根据帧类型分发
        if (isControlFrame(this.#info.opcode)) {
          this.#parseControlFrame()
        } else {
          this.#parseDataFrame(data)
        }

        this.#state = parserStates.INFO
      }
    }

    callback()
  }
}
```

#### 3.4.2 `consume()` -- 零拷贝缓冲区消费

```javascript
// 检查是否有足够的数据
#consume (n) {
  if (this.#bufferedBytes < n) return false
  this.#bufferedBytes -= n
  return true
}

// 消费 n 字节
#consumeBytes (n) {
  // 快速路径: 单个 buffer 足够, 使用 subarray (零拷贝)
  if (this.#buffers[0].length >= n) {
    const result = this.#buffers[0].subarray(0, n)
    this.#buffers[0] = this.#buffers[0].subarray(n)
    if (this.#buffers[0].length === 0) this.#buffers.shift()
    return result
  }

  // 慢速路径: 跨多个 buffer, 需要拼接
  const result = Buffer.allocUnsafeSlow(n)
  let offset = 0

  while (offset < n) {
    const buf = this.#buffers[0]
    const remaining = n - offset

    if (buf.length <= remaining) {
      buf.copy(result, offset)
      offset += buf.length
      this.#buffers.shift()
    } else {
      buf.copy(result, offset, 0, remaining)
      this.#buffers[0] = buf.subarray(remaining)
      offset = n
    }
  }

  return result
}
```

**零拷贝优化**: 单 buffer 场景使用 `subarray()` 避免内存分配; 跨 buffer 场景使用 `allocUnsafeSlow()` 避免内存池管理开销。`allocUnsafeSlow` 不会从 Node.js 的预分配内存池中分配，适合大块内存。

#### 3.4.3 数据帧处理 -- 分片与合并

```javascript
#parseDataFrame (data) {
  const { fin, opcode } = this.#info

  if (opcode === opcodes.CONTINUATION) {
    // CONTINUATION 帧: 追加到分片缓冲区
    this.#fragmented = Buffer.concat([this.#fragmented, data])

    if (fin) {
      // FIN=1: 分片结束, 分发完整消息
      this.#handler.onMessage(this.#fragmentedType, this.#fragmented)
      this.#fragmented = Buffer.alloc(0)
      this.#fragmentedType = null
    }
    return
  }

  if (opcode === opcodes.TEXT || opcode === opcodes.BINARY) {
    if (fin) {
      // FIN=1: 完整消息, 直接分发
      this.#handler.onMessage(opcode, data)
    } else {
      // FIN=0: 开始新的分片序列
      this.#fragmented = data
      this.#fragmentedType = opcode
    }
  }
}
```

#### 3.4.4 控制帧处理

```javascript
#parseControlFrame () {
  const { opcode, payloadLength } = this.#info

  // 控制帧限制: payload <= 125 字节, 不允许分片
  if (payloadLength > 125) {
    this.#fail(new Error('Control frame payload too large'))
    return
  }

  if (opcode === opcodes.CLOSE) {
    // CLOSE 帧: 解析关闭码和原因
    const body = this.#consumeBytes(payloadLength)
    const { code, reason } = this.#parseCloseBody(body)

    // Echo 回关闭帧 (如果是收到的第一个 close)
    this.#closeState.add(sentCloseFrameState.RECEIVED)
    this.#handler.onSocketClose(code, reason)
  }

  if (opcode === opcodes.PING) {
    // PING -> 自动回复 PONG (相同的 payload)
    const data = this.#consumeBytes(payloadLength)
    const frame = new WebsocketFrameSend(data)
    this.#handler.socket.write(frame.createFrame(opcodes.PONG))
    this.#handler.onPing(data)
  }

  if (opcode === opcodes.PONG) {
    // PONG -> 通知上层
    const data = this.#consumeBytes(payloadLength)
    this.#handler.onPong(data)
  }
}
```

### 3.5 发送队列

**文件**: `lib/web/websocket/sender.js` (110 行)

```javascript
class SendQueue extends FixedQueue {
  #socket

  constructor (socket) {
    super()
    this.#socket = socket
  }

  add (data, cb) {
    // 快速路径: 文本帧使用 cork/uncork 优化
    if (data.opcode === opcodes.TEXT) {
      const [head, masked] = createFastTextFrame(data.data)

      this.#socket.cork()
      this.#socket.write(head)
      this.#socket.write(masked, () => {
        this.#socket.uncork()
        cb?.()
      })
      return
    }

    // 慢速路径: 二进制帧 (可能需要处理 Blob)
    if (data.data instanceof Blob) {
      data.data.arrayBuffer().then(ab => {
        const frame = createFrame(opcodes.BINARY, new Uint8Array(ab))
        this.#socket.write(frame, cb)
      })
      return
    }

    // 通用路径
    const frame = createFrame(data.opcode, data.data)
    this.#socket.write(frame, cb)
  }
}
```

### 3.6 permessage-deflate 压缩扩展

**文件**: `lib/web/websocket/permessage-deflate.js` (100 行)

```javascript
class PerMessageDeflate {
  #inflate = null
  #options = {}
  #maxPayloadSize = 0

  constructor (extensions, options) {
    // 解析扩展参数
    this.#options.serverNoContextTakeover = extensions.has('server_no_context_takeover')
    this.#options.serverMaxWindowBits = extensions.get('server_max_window_bits')
    this.#maxPayloadSize = options.maxPayloadSize
  }

  decompress (chunk, fin, callback) {
    if (!this.#inflate) {
      // 创建 inflate 流
      let windowBits = Z_DEFAULT_WINDOWBITS
      if (this.#options.serverMaxWindowBits) {
        windowBits = Number.parseInt(this.#options.serverMaxWindowBits)
      }
      this.#inflate = createInflateRaw({ windowBits })
      this.#inflate[kBuffer] = []
      this.#inflate[kLength] = 0

      // 收集解压数据
      this.#inflate.on('data', (data) => {
        this.#inflate[kLength] += data.length

        // 大小限制检查 (防止解压炸弹)
        if (this.#maxPayloadSize > 0 && this.#inflate[kLength] > this.#maxPayloadSize) {
          callback(new MessageSizeExceededError())
          this.#inflate.removeAllListeners()
          this.#inflate = null
          return
        }

        this.#inflate[kBuffer].push(data)
      })
    }

    // 写入压缩数据
    this.#inflate.write(chunk)

    // 最后一个分片: 追加 tail bytes (RFC 7692)
    // 0x00 0x00 0xFF 0xFF 是 deflate 的同步标记
    if (fin) {
      this.#inflate.write(Buffer.from([0x00, 0x00, 0xFF, 0xFF]))
    }

    // 刷新并回调
    this.#inflate.flush(() => {
      const full = Buffer.concat(this.#inflate[kBuffer], this.#inflate[kLength])
      this.#inflate[kBuffer].length = 0
      this.#inflate[kLength] = 0
      callback(null, full)
    })
  }
}
```

**RFC 7692 关键点**: `0x00 0x00 0xFF 0xFF` 是 deflate 同步标记 (empty stored block), 用于表示消息边界。WebSocket 的 permessage-deflate 扩展通过追加这个标记来指示一条消息的压缩数据结束。

### 3.7 WebSocketStream API

**文件**: `lib/web/websocket/stream/websocketstream.js` (498 行)

WebSocketStream 是基于 WHATWG Streams API 的新 WebSocket 接口，将 WebSocket 包装为标准的 ReadableStream + WritableStream：

```javascript
class WebSocketStream {
  #url
  #openedPromise = Promise.withResolvers()
  #closedPromise = Promise.withResolvers()
  #readableStream
  #writableStream
  #handshakeAborted = false
  #handler = { ... }

  constructor (url, options) {
    // 1. URL 验证
    // 2. Protocol 去重检查
    // 3. AbortSignal 处理
    // 4. 建立连接
    this.#handler.controller = establishWebSocketConnection(
      urlRecord, protocols, client, this.#handler, options
    )
  }

  // 连接建立后创建 ReadableStream + WritableStream
  #onConnectionEstablished (response, extensions) {
    // ReadableStream: 接收消息
    const readable = new ReadableStream({
      start: (controller) => {
        this.#readableStreamController = controller
      },
      cancel: (reason) => {
        closeWebSocketConnection(this.#handler, 1000, reason?.message)
      }
    })

    // WritableStream: 发送消息
    const writable = new WritableStream({
      write: (chunk) => {
        return this.#write(chunk)  // 异步发送
      },
      close: () => {
        closeWebSocketConnection(this.#handler, null, null)
      },
      abort: (reason) => {
        this.#closeUsingReason(reason)
      }
    })

    // 解析 opened promise: 返回流和协商结果
    this.#openedPromise.resolve({
      extensions,
      protocol,
      readable,
      writable
    })
  }

  // 写入处理: 区分 BufferSource 和 string
  #write (chunk) {
    if (typeof chunk === 'string') {
      // 字符串 -> TEXT 帧
      const encoder = new TextEncoder()
      return this.#send(encoder.encode(chunk), opcodes.TEXT)
    } else {
      // BufferSource -> BINARY 帧
      return this.#send(
        new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength),
        opcodes.BINARY
      )
    }
  }

  // Promise 接口
  get opened () { return this.#openedPromise.promise }
  get closed () { return this.#closedPromise.promise }

  close (closeInfo) {
    closeWebSocketConnection(
      this.#handler,
      closeInfo.closeCode,
      closeInfo.reason,
      true
    )
  }
}
```

**设计亮点**: 将 WebSocket 包装为标准的 ReadableStream + WritableStream，可与 `pipeTo()` / `pipeFrom()` 等 Stream API 集成，支持背压控制。


---

## 5. EventSource / SSE 流式解析深度分析

### 4.1 EventSource 主类

**文件**: `lib/web/eventsource/eventsource.js` (493 行)

#### 4.1.1 构造器

```javascript
class EventSource extends EventTarget {
  #url
  #withCredentials = false
  #readyState = CONNECTING  // 0=CONNECTING, 1=OPEN, 2=CLOSED
  #request = null
  #controller = null
  #state = {
    lastEventId: '',
    reconnectionTime: 3000,  // 默认重连时间 3000ms
    origin: ''
  }

  constructor (url, eventSourceInitDict = {}) {
    super()

    // 1. URL 解析
    const urlRecord = new URL(url, settings.settingsObject.baseUrl)
    this.#url = urlRecord.href

    // 2. CORS 属性状态
    let corsAttributeState = ANONYMOUS
    if (eventSourceInitDict.withCredentials) {
      corsAttributeState = USE_CREDENTIALS
      this.#withCredentials = true
    }

    // 3. 创建潜在 CORS 请求
    const request = createPotentialCORSRequest(
      urlRecord, '', corsAttributeState
    )

    // 4. 设置 Accept: text/event-stream
    request.headersList.set('Accept', 'text/event-stream')

    // 5. 设置缓存模式: no-store (SSE 不缓存)
    request.cache = 'no-store'

    this.#request = request

    // 6. 发起连接
    this.#connect()
  }
}
```

#### 4.1.2 `#connect()` -- 连接与流式解析

```javascript
#connect () {
  if (this.#readyState === CLOSED) return
  this.#readyState = CONNECTING

  const fetchParams = {
    request: this.#request,
    dispatcher: this.#dispatcher,

    // 响应体结束回调: 重连
    processResponseEndOfBody: (response) => {
      if (!isNetworkError(response)) {
        // 正常结束: 触发重连
        return this.#reconnect()
      }
    },

    // 响应头回调: 验证并开始流式解析
    processResponse: (response) => {
      if (isNetworkError(response)) {
        // 网络错误: 区分中止和普通错误
        if (response.aborted) {
          this.close()
          this.dispatchEvent(new Event('error'))
        } else {
          this.#reconnect()
        }
        return
      }

      // 验证状态码: 必须是 200
      if (response.status !== 200) {
        this.close()
        this.dispatchEvent(new Event('error'))
        return
      }

      // 验证 Content-Type: 必须是 text/event-stream
      const contentType = response.headersList.get('content-type')
      const mimeType = parseMIMEType(contentType)
      if (!mimeType || mimeType.essence !== 'text/event-stream') {
        this.close()
        this.dispatchEvent(new Event('error'))
        return
      }

      // 连接成功
      this.#readyState = OPEN
      this.dispatchEvent(new Event('open'))

      // 创建 EventSourceStream 并 pipe 响应体
      const eventSourceStream = new EventSourceStream({
        eventSourceSettings: this.#state,
        maxEventSize: this.#dispatcher?.eventSourceOptions?.maxEventSize,
        push: (event) => {
          // 将解析后的事件分发给用户
          this.dispatchEvent(
            createFastMessageEvent(event.type, event.options)
          )
        }
      })

      // 流式管道: 响应体 -> EventSourceStream
      pipeline(response.body.stream, eventSourceStream, (error) => {
        if (error?.aborted === false) {
          this.close()
          this.dispatchEvent(new Event('error'))
        }
      })
    }
  }

  this.#controller = fetching(fetchParams)
}
```

#### 4.1.3 `#reconnect()` -- 自动重连

```javascript
#reconnect () {
  if (this.#readyState === CLOSED) return
  this.#readyState = CONNECTING

  // 触发 error 事件
  this.dispatchEvent(new Event('error'))

  // 等待重连时间后重连
  setTimeout(() => {
    if (this.#readyState !== CONNECTING) return

    // 设置 Last-Event-ID 头 (断点续传)
    if (this.#state.lastEventId.length) {
      this.#request.headersList.set(
        'last-event-id', this.#state.lastEventId, true
      )
    }

    this.#connect()
  }, this.#state.reconnectionTime)?.unref()
}
```

**设计要点**:
- 重连时间默认 3000ms (与 Chrome 一致，Deno 用 5000ms)
- `.unref()` 防止定时器阻止 Node.js 进程退出
- 服务端可通过 `retry:` 字段动态调整重连时间
- 每次重连携带 `Last-Event-ID` 头实现断点续传

### 4.2 EventSourceStream -- SSE 流解析器

**文件**: `lib/web/eventsource/eventsource-stream.js` (521 行)

这是整个 SSE 解析的核心，基于 Transform Stream 实现逐字节解析。

#### 4.2.1 架构设计

```javascript
class EventSourceStream extends Transform {
  // 事件源设置
  #state                      // eventSourceSettings (lastEventId, reconnectionTime, origin)

  // BOM 和行结束检测
  #checkBOM = true            // BOM 检测标志
  #crlfCheck = false          // CRLF 检测标志
  #eventEndCheck = false      // 事件结束检测 (空行)

  // 缓冲区管理 (支持跨 chunk 行读取)
  #chunks = []                // 缓冲区: Buffer[]
  #chunkIndex = 0             // 当前 chunk 索引
  #pos = 0                    // 当前 chunk 内位置
  #lineChunkIndex = 0         // 行起始 chunk 索引
  #linePos = 0                // 行起始位置

  // 事件大小保护
  #eventDataSize = 0          // 当前事件 data 累计大小
  #maxEventSize               // 最大事件大小限制

  // 当前事件状态
  #event = {
    data: undefined,
    event: undefined,
    id: undefined,
    retry: undefined
  }
}
```

#### 4.2.2 `_transform()` -- 主解析循环

```javascript
_transform (chunk, _encoding, callback) {
  if (chunk.length === 0) { callback(); return }

  // 将新 chunk 加入缓冲区
  this.#chunks.push(chunk)

  // 1. BOM 处理: UTF-8 BOM (0xEF 0xBB 0xBF)
  if (this.#checkBOM) {
    if (this.#handleBOM()) { callback(); return }
  }

  // 2. 主解析循环: 逐字节处理
  while (this.#hasCurrentByte()) {
    const byte = this.#currentByte()

    // === 事件结束检测 (连续空行) ===
    if (this.#eventEndCheck) {
      if (this.#crlfCheck) {
        // 上一个是 CR, 检查是否是 CRLF
        if (byte === LF) {
          this.#crlfCheck = false
          this.#consumeCurrentByte()
          continue
        }
        this.#crlfCheck = false
      }

      if (byte === LF || byte === CR) {
        if (byte === CR) this.#crlfCheck = true
        this.#consumeCurrentByte()

        // 空行: 当前事件结束
        if (this.#hasPendingEvent()) {
          this.#processEvent(this.#event)
        }
        this.#clearEvent()
        continue
      }
      this.#eventEndCheck = false
      continue
    }

    // === 行结束检测 (CR, LF, CRLF) ===
    if (byte === LF || byte === CR) {
      if (byte === CR) this.#crlfCheck = true

      // 读取当前行并解析
      try {
        this.#parseLine(this.#readLine(), this.#event)
      } catch (error) {
        callback(error)
        return
      }
      this.#consumeCurrentByte()
      this.#eventEndCheck = true
      continue
    }

    // 普通字符: 推进光标
    this.#advanceCursor()
  }

  callback()
}
```

#### 4.2.3 `#parseLine()` -- 行解析

```javascript
#parseLine (line, event) {
  if (line.length === 0) return  // 空行 -> 事件结束 (由调用方处理)

  // 注释行: 以 : 开头 (心跳或注释)
  const colonPosition = line.indexOf(COLON)
  if (colonPosition === 0) return

  // 分离字段名和值
  let fieldLength = line.length
  let valueStart = line.length

  if (colonPosition !== -1) {
    fieldLength = colonPosition
    valueStart = colonPosition + 1
    // 跳过值前的空格 (仅第一个空格)
    if (line[valueStart] === SPACE) ++valueStart
  }

  // === data 字段 ===
  if (isFieldName(line, fieldLength, DATA)) {
    const valueBytes = line.length - valueStart
    const eventDataSize = this.#eventDataSize +
      (event.data === undefined ? 0 : 1) + valueBytes  // +1 for \n

    // 大小限制检查
    if (this.#maxEventSize > 0 && eventDataSize > this.#maxEventSize) {
      throw createMaxEventSizeExceededError()
    }

    const value = line.toString('utf8', valueStart)
    if (event.data === undefined) {
      event.data = value
    } else {
      event.data += `\n${value}`  // 多行 data 用 \n 拼接
    }
    this.#eventDataSize = eventDataSize
    return
  }

  // === retry 字段 ===
  if (isFieldName(line, fieldLength, RETRY)) {
    if (isASCIINumberBytes(line, valueStart)) {
      event.retry = line.toString('utf8', valueStart)
    }
    return
  }

  // === id 字段 ===
  if (isFieldName(line, fieldLength, ID)) {
    if (isValidLastEventIdBytes(line, valueStart)) {
      event.id = line.toString('utf8', valueStart)
    }
    return
  }

  // === event 字段 ===
  if (isFieldName(line, fieldLength, EVENT)) {
    const value = line.toString('utf8', valueStart)
    if (value.length > 0) event.event = value
  }

  // 未知字段: 静默忽略 (规范要求)
}
```

**字段名匹配优化**: `isFieldName()` 使用 Buffer 比较而非字符串比较，直接在 Buffer 上操作避免了 UTF-8 解码开销。

#### 4.2.4 `#processEvent()` -- 事件分发

```javascript
#processEvent (event) {
  // 1. 更新重连时间 (如果 retry 字段存在)
  if (event.retry && isASCIINumber(event.retry)) {
    this.#state.reconnectionTime = parseInt(event.retry, 10)
  }

  // 2. 更新 lastEventId (如果 id 字段存在且有效)
  if (event.id !== undefined && isValidLastEventId(event.id)) {
    this.#state.lastEventId = event.id
  }

  // 3. 分发事件 (只有 data 存在时才分发)
  if (event.data !== undefined) {
    this.push({
      type: event.event || 'message',  // 默认类型为 'message'
      options: {
        data: event.data,
        lastEventId: this.#state.lastEventId,
        origin: this.#state.origin
      }
    })
  }
}
```

#### 4.2.5 BOM 处理

```javascript
#handleBOM () {
  const first = this.#peekBufferedByte(0)
  const second = this.#peekBufferedByte(1)
  const third = this.#peekBufferedByte(2)

  // UTF-8 BOM: 0xEF 0xBB 0xBF
  if (first === 0xEF && second === 0xBB && third === 0xBF) {
    this.#discardLeadingBytes(3)
  }

  this.#checkBOM = false
  return !this.#hasCurrentByte()  // 如果只有 BOM 没有其他数据, 返回 true
}
```

#### 4.2.6 跨 chunk 行读取

```javascript
#readLine () {
  // 快速路径: 行在单个 chunk 内 (使用 subarray, 零拷贝)
  if (this.#lineChunkIndex === this.#chunkIndex) {
    return this.#chunks[this.#chunkIndex].subarray(this.#linePos, this.#pos)
  }

  // 慢速路径: 行跨多个 chunk (需要拼接)
  const chunks = []
  let length = 0

  for (let i = this.#lineChunkIndex; i <= this.#chunkIndex; i++) {
    const chunk = this.#chunks[i]
    const start = i === this.#lineChunkIndex ? this.#linePos : 0
    const end = i === this.#chunkIndex ? this.#pos : chunk.length
    const slice = chunk.subarray(start, end)
    length += slice.length
    chunks.push(slice)
  }

  return Buffer.concat(chunks, length)
}
```

**优化策略**: 快速路径使用 `subarray()` (零拷贝), 慢速路径才 `Buffer.concat()`。大多数 SSE 行都在单个 chunk 内，快速路径是常态。

#### 4.2.7 Chunk 内存管理

```javascript
#dropConsumedChunks () {
  // 丢弃已经被行起始指针越过的 chunk
  while (this.#lineChunkIndex > 0) {
    this.#chunks.shift()
    this.#lineChunkIndex--
    this.#chunkIndex--
  }

  // 所有 chunk 都已消费: 清理
  if (this.#chunkIndex === this.#chunks.length) {
    this.#chunks.length = 0
    this.#chunkIndex = 0
    this.#pos = 0
    this.#lineChunkIndex = 0
    this.#linePos = 0
  }
}
```

**内存管理**: 通过 `lineChunkIndex` 追踪已消费位置，及时丢弃已处理的 chunk，避免长时间运行的 SSE 连接内存泄漏。

### 4.3 EventSource 工具函数

**文件**: `lib/web/eventsource/util.js` (60 行)

#### 4.3.1 `createPotentialCORSRequest()`

```javascript
function createPotentialCORSRequest (url, destination, corsAttributeState) {
  // 1. 确定 mode
  let mode = corsAttributeState === 'no cors' ? 'no-cors' : 'cors'

  // 2. 确定 credentialsMode
  let credentialsMode = 'include'
  if (corsAttributeState === 'anonymous') {
    credentialsMode = 'same-origin'
  }

  // 3. 创建请求
  return makeRequest({
    urlList: [url],
    destination,
    mode,
    credentials: credentialsMode,
    useURLCredentials: true
  })
}
```

#### 4.3.2 `isValidLastEventId()`

```javascript
function isValidLastEventId (id) {
  // Last-Event-ID 不能包含 NULL 字符 (0x00)
  for (let i = 0; i < id.length; ++i) {
    if (id.charCodeAt(i) === 0x00) return false
  }
  return true
}

function isASCIINumber (value) {
  // 只包含 ASCII 十进制数字
  for (let i = 0; i < value.length; ++i) {
    const charCode = value.charCodeAt(i)
    if (charCode < 0x30 || charCode > 0x39) return false
  }
  return value.length > 0
}
```

---

## 6. Cache API 深度分析

### 5.1 Cache 类

**文件**: `lib/web/cache/cache.js` (862 行)

#### 5.1.1 存储结构

```javascript
class Cache {
  #relevantRequestResponseList  // [request, response][] 数组

  constructor () {
    if (arguments[0] !== kConstruct) {
      webidl.illegalConstructor()  // 禁止直接构造
    }
    this.#relevantRequestResponseList = arguments[1]
  }
}
```

**设计**: Cache 不直接暴露构造器，只能通过 `CacheStorage.open()` 创建。内部使用 `[request, response]` 元组数组存储。这个设计确保了 Cache 实例与 CacheStorage 的生命周期绑定。

**重要发现**: undici 的 Cache API 是纯内存实现，没有 SQLite 后端。所有数据存储在 `#relevantRequestResponseList` 数组中，进程退出后数据丢失。这与浏览器实现不同（浏览器通常有持久化存储）。

#### 5.1.2 `match()` -- 缓存匹配

```javascript
async match (request, options = {}) {
  const p = this.#internalMatchAll(request, options, 1)
  return p.length === 0 ? undefined : p[0]
}

#internalMatchAll (request, options, maxResponses = Infinity) {
  let r = null

  // 1. 解析 request
  if (request !== undefined) {
    if (webidl.is.Request(request)) {
      r = getRequestState(request)
      // 非 GET 请求 + 非 ignoreMethod -> 不匹配
      if (r.method !== 'GET' && !options.ignoreMethod) return []
    } else if (typeof request === 'string') {
      r = getRequestState(new Request(request))
    }
  }

  // 2. 查询缓存
  const responses = []
  if (request === undefined) {
    // 无 request 参数: 返回所有缓存项
    for (const [_, response] of this.#relevantRequestResponseList) {
      responses.push(response)
    }
  } else {
    // 按 URL + Vary 匹配
    const requestResponses = this.#queryCache(r, options)
    for (const [_, response] of requestResponses) {
      responses.push(response)
    }
  }

  // 3. 克隆响应并返回
  const responseList = []
  for (const response of responses) {
    responseList.push(fromInnerResponse(cloneResponse(response), 'immutable'))
    if (responseList.length >= maxResponses) break
  }

  return Object.freeze(responseList)
}
```

#### 5.1.3 `#queryCache()` -- 缓存查询算法

```javascript
#queryCache (requestQuery, options, targetStorage) {
  const resultList = []
  const storage = targetStorage ?? this.#relevantRequestResponseList

  for (const requestResponse of storage) {
    const [cachedRequest, cachedResponse] = requestResponse
    if (this.#requestMatchesCachedItem(
      requestQuery, cachedRequest, cachedResponse, options
    )) {
      resultList.push(requestResponse)
    }
  }

  return resultList
}

#requestMatchesCachedItem (requestQuery, request, response, options) {
  const queryURL = new URL(requestQuery.url)
  const cachedURL = new URL(request.url)

  // ignoreSearch: 忽略查询参数
  if (options?.ignoreSearch) {
    cachedURL.search = ''
    queryURL.search = ''
  }

  // URL 比较 (可选排除 fragment)
  if (!urlEquals(queryURL, cachedURL, true)) return false

  // 无 response 或 ignoreVary 或无 Vary 头 -> 直接匹配
  if (response == null || options?.ignoreVary ||
      !response.headersList.contains('vary')) {
    return true
  }

  // Vary 头匹配: 逐个字段比较
  const fieldValues = getFieldValues(response.headersList.get('vary'))
  for (const fieldValue of fieldValues) {
    if (fieldValue === '*') return false  // Vary: * 永远不匹配

    const requestValue = request.headersList.get(fieldValue)
    const queryValue = requestQuery.headersList.get(fieldValue)
    if (requestValue !== queryValue) return false
  }

  return true
}
```

#### 5.1.4 `addAll()` -- 批量添加 (带并发 fetch)

```javascript
async addAll (requests) {
  const fetchControllers = []
  const responsePromises = []
  const requestList = []

  for (const request of requests) {
    const r = getRequestState(new Request(request))

    // 验证: 必须是 HTTP(S) + GET
    if (!urlIsHttpHttpsScheme(r.url) || r.method !== 'GET') {
      throw webidl.errors.exception(...)
    }

    r.initiator = 'fetch'
    r.destination = 'subresource'
    requestList.push(r)

    const responsePromise = Promise.withResolvers()

    fetchControllers.push(fetching({
      request: r,
      processResponse (response) {
        // 验证状态码: 必须是 2xx (非 206)
        if (response.type === 'error' || response.status === 206 || ...) {
          responsePromise.reject(...)
          return
        }

        // 验证 Vary: * 不允许
        if (response.headersList.contains('vary')) {
          const fieldValues = getFieldValues(response.headersList.get('vary'))
          for (const fieldValue of fieldValues) {
            if (fieldValue === '*') {
              responsePromise.reject(new DOMException('Vary: *', 'InvalidStateError'))
              // 中止所有其他 fetch
              for (const controller of fetchControllers) controller.abort()
              return
            }
          }
        }
      },
      processResponseEndOfBody (response) {
        if (response.aborted) {
          responsePromise.reject(new DOMException('aborted', 'AbortError'))
        } else {
          responsePromise.resolve(response)
        }
      }
    }))

    responsePromises.push(responsePromise.promise)
  }

  // 等待所有 fetch 完成
  const responses = await Promise.all(responsePromises)

  // 批量写入缓存
  const operations = responses.map((response, index) => ({
    type: 'put',
    request: requestList[index],
    response
  }))

  this.#batchCacheOperations(operations)
}
```

**并发 fetch**: `addAll()` 并行发起所有 fetch 请求，然后在全部完成后原子性写入缓存。使用 `Promise.all()` 等待，确保要么全部成功要么全部失败。

#### 5.1.5 `#batchCacheOperations()` -- 原子批量操作

```javascript
#batchCacheOperations (operations) {
  const cache = this.#relevantRequestResponseList
  const backupCache = [...cache]  // 备份用于回滚
  const addedItems = []

  try {
    for (const operation of operations) {
      // 检查写入冲突
      if (this.#queryCache(
        operation.request, operation.options, addedItems
      ).length) {
        throw new DOMException('Write conflict', 'InvalidStateError')
      }

      if (operation.type === 'delete') {
        // 删除匹配的条目
        const requestResponses = this.#queryCache(
          operation.request, operation.options
        )
        for (const requestResponse of requestResponses) {
          const idx = cache.indexOf(requestResponse)
          cache.splice(idx, 1)
        }
      } else if (operation.type === 'put') {
        // 删除旧条目并添加新条目
        const existingItems = this.#queryCache(operation.request)
        for (const existing of existingItems) {
          const idx = cache.indexOf(existing)
          cache.splice(idx, 1)
        }
        cache.push([operation.request, operation.response])
        addedItems.push([operation.request, operation.response])
      }
    }

    return resultList
  } catch (e) {
    // 回滚: 恢复备份
    this.#relevantRequestResponseList.length = 0
    Object.assign(this.#relevantRequestResponseList, backupCache)
    throw e
  }
}
```

**事务性**: 批量操作在失败时自动回滚，保证缓存状态的一致性。使用简单的数组快照备份/恢复策略。

### 5.2 CacheStorage 类

**文件**: `lib/web/cache/cachestorage.js` (152 行)

```javascript
class CacheStorage {
  #caches = new Map()  // name -> [request, response][]

  async match (request, options = {}) {
    if (options.cacheName != null) {
      // 在指定缓存中查找
      if (this.#caches.has(options.cacheName)) {
        const cache = new Cache(kConstruct, this.#caches.get(options.cacheName))
        return await cache.match(request, options)
      }
    } else {
      // 遍历所有缓存查找 (按插入顺序)
      for (const cacheList of this.#caches.values()) {
        const cache = new Cache(kConstruct, cacheList)
        const response = await cache.match(request, options)
        if (response !== undefined) return response
      }
    }
  }

  async open (cacheName) {
    if (this.#caches.has(cacheName)) {
      return new Cache(kConstruct, this.#caches.get(cacheName))
    }
    const cache = []
    this.#caches.set(cacheName, cache)
    return new Cache(kConstruct, cache)
  }

  async delete (cacheName) {
    return this.#caches.delete(cacheName)
  }

  async has (cacheName) {
    return this.#caches.has(cacheName)
  }

  async keys () {
    return [...this.#caches.keys()]
  }
}
```

---

## 7. Cache 存储层 (SQLite / Memory)

**文件**:
- `lib/cache/sqlite-cache-store.js` (469 行)
- `lib/cache/memory-cache-store.js` (279 行)
- `lib/util/cache.js` (工具函数, 提供 `assertCacheKey` / `assertCacheValue`)

undici 的 cache 拦截器 (`DispatchInterceptor`) 支持可插拔的存储后端，内置两种实现：

| 存储后端 | 持久化 | 默认位置 | 适用场景 |
|----------|--------|----------|----------|
| `SqliteCacheStore` | ✅ WAL 模式 | `:memory:` 或磁盘文件 | 生产环境，大缓存量 |
| `MemoryCacheStore` | ❌ 纯内存 | `Map` | 测试、短生命周期 |

两个后端都实现相同的 `CacheStore` 接口 (定义于 `types/cache-interceptor.d.ts`)。

### 7.1 公共接口 `CacheStore`

```typescript
interface CacheStore {
  get(key: CacheKey): GetResult | undefined
  set(key: CacheKey, value: CacheValue): void
  createWriteStream(key: CacheKey, value: CacheValue): Writable | undefined
  delete(key: CacheKey): void
  close?(): void
}

interface CacheKey {
  origin: string
  path: string
  method: string
  headers?: Record<string, string>
}

interface CacheValue {
  body: Buffer | null
  statusCode: number
  statusMessage: string
  headers?: string        // JSON
  vary?: string           // JSON
  etag?: string
  cacheControlDirectives?: string  // JSON
  cachedAt: number        // Date.now()
  staleAt: number         // 过期开始时间
  deleteAt: number        // 必须删除时间
}
```

### 7.2 SqliteCacheStore -- 持久化缓存

**文件**: `lib/cache/sqlite-cache-store.js` (469 行)

使用 Node.js v22.5+ 内置的 `node:sqlite` (编译时 SQLite)，不需要外部依赖。

#### 7.2.1 数据库初始化

```javascript
const VERSION = 3         // schema 版本号
const MAX_ENTRY_SIZE = 2 * 1000 * 1000 * 1000  // 2GB

class SqliteCacheStore {
  #maxEntrySize = MAX_ENTRY_SIZE
  #maxCount = Infinity
  #db: DatabaseSync

  constructor (opts) {
    // 参数校验
    // opts.maxEntrySize, opts.maxCount, opts.location

    this.#db = new DatabaseSync(opts?.location ?? ':memory:')

    // 1. PRAGMA 优化
    this.#db.exec(`
      PRAGMA journal_mode = WAL;        -- 写前日志: 读写并发
      PRAGMA synchronous = NORMAL;      -- 同步模式: 性能与安全平衡
      PRAGMA temp_store = memory;       -- 临时表在内存
      PRAGMA optimize;                  -- 自动索引优化
    `)

    // 2. 建表
    this.#db.exec(`
      CREATE TABLE IF NOT EXISTS cacheInterceptorV${VERSION} (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        url TEXT NOT NULL,
        method TEXT NOT NULL,

        body BUF NULL,                    -- 响应体 (可为空)
        deleteAt INTEGER NOT NULL,        -- 必须删除时间
        statusCode INTEGER NOT NULL,
        statusMessage TEXT NOT NULL,
        headers TEXT NULL,                -- JSON 序列化
        cacheControlDirectives TEXT NULL, -- JSON
        etag TEXT NULL,
        vary TEXT NULL,                   -- JSON
        cachedAt INTEGER NOT NULL,        -- 缓存写入时间
        staleAt INTEGER NOT NULL,         -- 开始过期时间
      );

      -- 索引: 按 url + method + deleteAt 查询
      CREATE INDEX IF NOT EXISTS ..._getValuesQuery
        ON cacheInterceptorV${VERSION}(url, method, deleteAt);

      -- 索引: 按 deleteAt 删除过期数据
      CREATE INDEX IF NOT EXISTS ..._deleteByUrlQuery
        ON cacheInterceptorV${VERSION}(deleteAt);
    `)

    // 3. 预编译语句 (StatementSync)
    this.#getValuesQuery = this.#db.prepare(`SELECT ... WHERE url = ? AND method = ? ORDER BY deleteAt ASC`)
    this.#insertValueQuery = this.#db.prepare(`INSERT INTO ...`)
    this.#updateValueQuery = this.#db.prepare(`UPDATE ... WHERE id = ?`)
    this.#deleteByUrlQuery = this.#db.prepare(`DELETE ... WHERE url = ?`)
    this.#deleteExpiredValuesQuery = this.#db.prepare(`DELETE ... WHERE deleteAt <= ?`)
    this.#deleteOldValuesQuery = this.#db.prepare(`DELETE ... ORDER BY cachedAt ASC LIMIT ?`)
  }
}
```

#### 7.2.2 Vary 匹配查询

S `#findValue()` 方法在 SQLite 之上实现了 `Vary` 响应头的匹配逻辑:

```javascript
#findValue (key, canBeExpired = false) {
  const url = `${key.origin}/${key.path}`
  const values = this.#getValuesQuery.all(url, method)

  const now = Date.now()
  for (const value of values) {
    // 1. 删除时间过滤
    if (now >= value.deleteAt && !canBeExpired) {
      continue
    }

    // 2. Vary 匹配
    if (value.vary) {
      const vary = JSON.parse(value.vary)
      for (const header in vary) {
        if (!headerValueEquals(headers[header], vary[header])) {
          matches = false
          break
        }
      }
    }

    if (matches) return value
  }
}
```

**Vary 匹配语义**: `Vary: Accept-Encoding, User-Agent` 表示根据这两个请求头区分缓存版本。`headerValueEquals` 支持 null 等价性比较。

#### 7.2.3 流式写入: `createWriteStream()`

缓存响应体可能很大，不能一次性读到内存。`createWriteStream()` 返回一个 Writable，在 `final` 回调中调用 `set`:

```javascript
createWriteStream (key, value) {
  let size = 0
  const body = []

  return new Writable({
    write (chunk, encoding, callback) {
      size += chunk.byteLength

      // 超过 maxEntrySize → 丢弃
      if (size > this.#maxEntrySize) {
        this.destroy()
      } else {
        body.push(chunk)
      }

      callback()
    },
    final (callback) {
      // 写入完成 → 持久化到 SQLite
      this.set(key, { ...value, body: Buffer.concat(body) })
      callback()
    }
  })
}
```

#### 7.2.4 LRU 式剪枝: `#prune()`

每次插入新数据后调用 `#prune()`，按以下顺序执行:

```javascript
#prune () {
  // 1. 未超限 → 不操作
  if (Number.isFinite(this.#maxCount) && this.size <= this.#maxCount) {
    return 0
  }

  // 2. 先删过期数据
  const removed = this.#deleteExpiredValuesQuery.run(Date.now()).changes
  if (removed) return removed

  // 3. 删除最老的 10% 条目
  const removed = this.#deleteOldValuesQuery.run(
    Math.max(Math.floor(this.#maxCount * 0.1), 1)
  ).changes
  return removed
}
```

**剪枝策略**: SQLite 版本没有严格 LRU，而是采用 "删过期 → 删最老 10%" 的混合策略，减少查询次数。

### 7.3 MemoryCacheStore -- 内存缓存

**文件**: `lib/cache/memory-cache-store.js` (279 行)

纯内存实现，使用 `Map<topLevelKey, entry[]>` 存储，适用于测试和短生命周期场景。

```javascript
class MemoryCacheStore extends EventEmitter {
  #maxCount = 1024                // 最大条目数
  #maxSize = 104857600            // 100MB 总大小
  #maxEntrySize = 5242880         // 单条 5MB

  #size = 0                       // 当前总字节
  #count = 0                      // 当前条目数
  #entries = new Map()            // Map<string, entry[]>
  #hasEmittedMaxSizeEvent = false
}
```

#### 7.3.1 写入与驱逐

MemoryCacheStore 的驱逐策略更加激进:

```javascript
createWriteStream (key, val) {
  return new Writable({
    write (chunk, encoding, callback) {
      entry.size += chunk.byteLength
      if (entry.size > store.#maxEntrySize) {
        this.destroy()
      } else {
        entry.body.push(chunk)
      }
      callback(null)
    },
    final (callback) {
      // 更新 size/count 计数器
      store.#size += entry.size
      store.#count += 1

      // 超限 → 驱逐
      if (store.#size > store.#maxSize || store.#count > store.#maxCount) {
        // 1. 触发 maxSizeExceeded 事件 (仅首次)
        store.emit('maxSizeExceeded', { ... })

        // 2. 对所有 key 驱逐一半条目
        for (const [key, entries] of store.#entries) {
          for (const entry of entries.splice(0, Math.ceil(entries.length / 2))) {
            store.#size -= entry.size
            store.#count -= 1
          }
        }
      }
    }
  })
}
```

**驱逐策略**: 当缓存超限时，**所有 key 的条目都减半**。这是一种全局性的激进驱逐，适用于内存受限场景。

#### 7.3.2 Vary 匹配

```javascript
function findEntry (key, entries, now) {
  for (const entry of entries) {
    // 1. deleteAt 过滤
    // 2. method 匹配
    // 3. vary 匹配
    if (entry.deleteAt > now &&
        entry.method === key.method &&
        varyMatches(key, entry)) {
      return entry
    }
  }
}

function varyMatches (key, entry) {
  if (entry.vary == null) return true

  for (const headerName in entry.vary) {
    if (!headerValueEquals(key.headers?.[headerName], entry.vary[headerName])) {
      return false
    }
  }
  return true
}
```

### 7.4 缓存拦截器集成

undici 通过 `CacheInterceptor` (内部模块) 将存储后端集成到 dispatcher 中:

```javascript
// lib/interceptors/cache.js (简化)
function cacheInterceptor (dispatcher, opts) {
  const store = opts.store ?? new SqliteCacheStore(opts)

  return (dispatch) => {
    return function CacheDispatch (opts, handler) {
      const key = makeCacheKey(opts)
      const cached = store.get(key)

      // 命中: 直接返回缓存
      if (cached && cached.staleAt > Date.now()) {
        return dispatch(opts, createCachedHandler(cached, handler))
      }

      // 未命中: 透传并缓存响应
      return dispatch(opts, wrapHandler(handler, (response) => {
        store.set(key, makeCacheValue(response))
      }))
    }
  }
}

// 使用示例
const dispatcher = new Agent().compose(
  cacheInterceptor(new ProxyAgent(), {
    store: new SqliteCacheStore({
      location: './undici-cache.db',
      maxCount: 10000
    })
  })
)

const response = await fetch(url, { dispatcher })
```

### 7.5 配置选项对比

| 选项 | SqliteCacheStore | MemoryCacheStore |
|------|:----------------:|:----------------:|
| `location` | 文件路径 / `:memory:` | N/A |
| `maxEntrySize` | 默认 2GB | 默认 5MB |
| `maxCount` | 默认 ∞ | 默认 1024 |
| `maxSize` | N/A | 默认 100MB |
| 持久化 | ✅ WAL | ❌ |
| Vary 匹配 | ✅ | ✅ |
| 流式写入 | ✅ | ✅ |
| 删除过期 | ✅ | ✅ |

### 7.6 对 laew 的借鉴

1. **会话记忆缓存**: 使用 MemoryCacheStore 缓存最近 N 个 Session 的上下文摘要，避免 SQLite 读写延迟
2. **模型响应缓存**: 对相同 prompt 的 LLM 响应缓存 1 小时 (带 `staleAt` 过期)
3. **工具输出缓存**: Bash 命令结果按 `(cwd, command)` 做 key 缓存，TTL 30s

---

## 8. Cookies 模块深度分析

### 6.1 Cookie API

**文件**: `lib/web/cookies/index.js` (199 行)

#### 6.1.1 `getCookies()` -- 获取请求 Cookie

```javascript
function getCookies (headers) {
  const cookie = headers.get('cookie')
  const out = {}

  if (!cookie) return out

  // 解析: "name1=value1; name2=value2"
  // 注意: 值中可能包含 =, 所以用 slice 而非 split
  for (const piece of cookie.split(';')) {
    const [name, ...value] = piece.split('=')
    out[name.trim()] = value.join('=')
  }

  return out
}
```

#### 6.1.2 `setCookie()` -- 设置响应 Cookie

```javascript
function setCookie (headers, cookie) {
  cookie = webidl.converters.Cookie(cookie)

  const str = stringify(cookie)
  if (str) {
    // 注意: append 第三个参数 true 表示允许 Set-Cookie 重复
    headers.append('set-cookie', str, true)
  }
}
```

#### 6.1.3 `getSetCookies()` -- 获取 Set-Cookie 列表

```javascript
function getSetCookies (headers) {
  // 利用 HeadersList 的特殊 cookies 数组
  const cookies = headers.getSetCookie()
  if (!cookies) return []
  return cookies.map(pair => parseSetCookie(pair))
}
```

#### 6.1.4 `deleteCookie()` -- 删除 Cookie

```javascript
function deleteCookie (headers, name, attributes) {
  // 设置过期时间为 epoch 0 来 "删除"
  setCookie(headers, {
    name,
    value: '',
    expires: new Date(0),
    ...attributes
  })
}
```

**删除机制**: HTTP Cookie 没有直接的删除操作。通过设置 `Expires` 为过去时间，浏览器会自动删除该 Cookie。

### 6.2 Set-Cookie 解析器

**文件**: `lib/web/cookies/parse.js` (317 行)

#### 6.2.1 `parseSetCookie()` -- RFC 6265bis 对齐

```javascript
function parseSetCookie (header) {
  // 1. CTL 字符检查 (排除 HTAB)
  if (isCTLExcludingHtab(header)) return null

  let nameValuePair = ''
  let unparsedAttributes = ''

  // 2. 分离 name=value 和 unparsed-attributes (以 ; 分割)
  if (header.includes(';')) {
    const position = { position: 0 }
    nameValuePair = collectASequenceOfCodePointsFast(';', header, position)
    unparsedAttributes = header.slice(position.position)
  } else {
    nameValuePair = header
  }

  // 3. 分离 name 和 value (以第一个 = 分割)
  let name, value
  if (!nameValuePair.includes('=')) {
    value = nameValuePair
  } else {
    const position = { position: 0 }
    name = collectASequenceOfCodePointsFast('=', nameValuePair, position)
    value = nameValuePair.slice(position.position + 1)
  }

  name = name.trim()
  value = value.trim()

  // 4. 大小限制: name + value <= 4096 字节
  if (name.length + value.length > maxNameValuePairSize) return null

  // 5. 解析 unparsed-attributes (递归)
  return { name, value, ...parseUnparsedAttributes(unparsedAttributes) }
}
```

#### 6.2.2 `parseUnparsedAttributes()` -- 属性递归解析

```javascript
function parseUnparsedAttributes (unparsedAttributes, cookieAttributeList = {}) {
  // 递归终止条件
  if (unparsedAttributes.length === 0) return cookieAttributeList

  // 丢弃前导 ;
  unparsedAttributes = unparsedAttributes.slice(1)

  // 提取当前 cookie-av (以 ; 或末尾为边界)
  let cookieAv = ''
  if (unparsedAttributes.includes(';')) {
    cookieAv = collectASequenceOfCodePointsFast(';', unparsedAttributes, { position: 0 })
    unparsedAttributes = unparsedAttributes.slice(cookieAv.length)
  } else {
    cookieAv = unparsedAttributes
    unparsedAttributes = ''
  }

  // 分离 attribute-name 和 attribute-value
  let attributeName, attributeValue
  if (cookieAv.includes('=')) {
    const position = { position: 0 }
    attributeName = collectASequenceOfCodePointsFast('=', cookieAv, position)
    attributeValue = cookieAv.slice(position.position + 1)
  } else {
    attributeName = cookieAv
    attributeValue = ''
  }

  attributeName = attributeName.trim()
  attributeValue = attributeValue.trim()

  // 属性值长度限制 (1024 字节)
  if (attributeValue.length > maxAttributeValueSize) {
    return parseUnparsedAttributes(unparsedAttributes, cookieAttributeList)
  }

  // 按属性名分发处理
  const attributeNameLowercase = attributeName.toLowerCase()

  if (attributeNameLowercase === 'expires') {
    const expiryTime = new Date(attributeValue)
    if (!Number.isNaN(expiryTime.getTime())) {
      cookieAttributeList.expires = expiryTime
    }
  } else if (attributeNameLowercase === 'max-age') {
    // 验证: 首字符是数字或负号, 其余是数字
    const charCode = attributeValue.charCodeAt(0)
    if ((charCode >= 48 && charCode <= 57) || attributeValue[0] === '-') {
      if (!/[^\d]/.test(attributeValue.slice(1))) {
        cookieAttributeList.maxAge = Number(attributeValue)
      }
    }
  } else if (attributeNameLowercase === 'domain') {
    let cookieDomain = attributeValue
    // 去掉前导点 (RFC 6265bis)
    if (cookieDomain[0] === '.') cookieDomain = cookieDomain.slice(1)
    cookieAttributeList.domain = cookieDomain.toLowerCase()
  } else if (attributeNameLowercase === 'path') {
    // 路径必须以 / 开头, 否则使用默认 /
    cookieAttributeList.path = attributeValue[0] === '/' ? attributeValue : '/'
  } else if (attributeNameLowercase === 'secure') {
    cookieAttributeList.secure = true
  } else if (attributeNameLowercase === 'httponly') {
    cookieAttributeList.httpOnly = true
  } else if (attributeNameLowercase === 'samesite') {
    const v = attributeValue.toLowerCase()
    if (v === 'none') cookieAttributeList.sameSite = 'None'
    else if (v === 'strict') cookieAttributeList.sameSite = 'Strict'
    else if (v === 'lax') cookieAttributeList.sameSite = 'Lax'
    // 未知值不设置 (等效于 Lax)
  } else {
    // 未识别的属性存入 unparsed
    cookieAttributeList.unparsed ??= []
    cookieAttributeList.unparsed.push(`${attributeName}=${attributeValue}`)
  }

  // 递归解析下一个属性
  return parseUnparsedAttributes(unparsedAttributes, cookieAttributeList)
}
```

### 6.3 Cookie 验证与序列化

**文件**: `lib/web/cookies/util.js` (353 行)

#### 6.3.1 Cookie Name 验证

```javascript
function validateCookieName (name) {
  for (let i = 0; i < name.length; ++i) {
    const code = name.charCodeAt(i)
    // Cookie name 必须是 "token" 字符 (RFC 2616)
    // 排除: CTL, SP, HT, 以及分隔符 " ( ) < > @ , ; : \ / [ ] ? = { }
    if (
      code < 0x21 || code > 0x7E ||
      code === 0x22 || code === 0x28 || code === 0x29 ||
      code === 0x3C || code === 0x3E || code === 0x40 ||
      code === 0x2C || code === 0x3B || code === 0x3A ||
      code === 0x5C || code === 0x2F || code === 0x5B ||
      code === 0x5D || code === 0x3F || code === 0x3D ||
      code === 0x7B || code === 0x7D
    ) {
      throw new Error('Invalid cookie name')
    }
  }
}
```

#### 6.3.2 Cookie Domain 验证

```javascript
function validateCookieDomain (domain) {
  if (domain === ' ') return
  if (domain.length > 255) throw new Error('Invalid cookie domain: too long')

  let labelLength = 0

  for (let i = 0; i < domain.length; ++i) {
    const code = domain.charCodeAt(i)

    if (code === 0x2E) {  // . (label separator)
      if (labelLength === 0) throw new Error('Empty label')
      if (domain.charCodeAt(i - 1) === 0x2D) throw new Error('Label ends with -')
      labelLength = 0
      continue
    }

    // Label 必须以字母或数字开头
    if (labelLength === 0 && !isLetterOrDigit(code)) {
      throw new Error('Label starts with non-alphanumeric')
    }
    // Label 只包含字母、数字、连字符
    if (!isLetterOrDigit(code) && code !== 0x2D) {
      throw new Error('Invalid character in label')
    }
    // Label 最大 63 字符 (RFC 1034)
    if (++labelLength > 63) throw new Error('Label too long')
  }

  // 不能以空 label 或连字符结尾
  if (labelLength === 0 || domain.charCodeAt(domain.length - 1) === 0x2D) {
    throw new Error('Invalid domain end')
  }
}
```

#### 6.3.3 `stringify()` -- Cookie 序列化

```javascript
function stringify (cookie) {
  if (cookie.name.length === 0) return null

  validateCookieName(cookie.name)
  validateCookieValue(cookie.value)

  const out = [`${cookie.name}=${cookie.value}`]

  // __Secure- 前缀: 强制 Secure
  if (cookie.name.startsWith('__Secure-')) {
    cookie.secure = true
  }

  // __Host- 前缀: 强制 Secure + Path=/ + 无 Domain
  if (cookie.name.startsWith('__Host-')) {
    cookie.secure = true
    cookie.domain = null
    cookie.path = '/'
  }

  if (cookie.secure) out.push('Secure')
  if (cookie.httpOnly) out.push('HttpOnly')
  if (typeof cookie.maxAge === 'number') {
    validateCookieMaxAge(cookie.maxAge)
    out.push(`Max-Age=${cookie.maxAge}`)
  }
  if (cookie.domain) {
    validateCookieDomain(cookie.domain)
    out.push(`Domain=${cookie.domain}`)
  }
  if (cookie.path) {
    validateCookiePath(cookie.path)
    out.push(`Path=${cookie.path}`)
  }
  if (cookie.expires && cookie.expires.toString() !== 'Invalid Date') {
    out.push(`Expires=${toIMFDate(cookie.expires)}`)
  }
  if (cookie.sameSite) {
    out.push(`SameSite=${cookie.sameSite}`)
  }

  // 处理 unparsed 自定义属性
  for (const part of cookie.unparsed) {
    const [key, ...value] = part.split('=')
    const trimmedKey = key.trim()
    const joinedValue = value.join('=')
    validateCookieName(trimmedKey)
    validateCookieValue(joinedValue)
    out.push(`${trimmedKey}=${joinedValue}`)
  }

  return out.join('; ')
}
```

#### 6.3.4 IMF 日期格式

```javascript
// 预计算查找表 (避免运行时格式化开销)
const IMFPaddedNumbers = Array.from({ length: 61 }, (_, i) =>
  i.toString().padStart(2, '0')
)

const IMFDays = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat']
const IMFMonths = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
                    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

function toIMFDate (date) {
  if (typeof date === 'number') date = new Date(date)

  // RFC 7231 Section 7.1.1.1 IMF-fixdate
  // 例: "Sun, 06 Nov 1994 08:49:37 GMT"
  return `${IMFDays[date.getUTCDay()]}, ` +
    `${IMFPaddedNumbers[date.getUTCDate()]} ` +
    `${IMFMonths[date.getUTCMonth()]} ` +
    `${date.getUTCFullYear()} ` +
    `${IMFPaddedNumbers[date.getUTCHours()]}:` +
    `${IMFPaddedNumbers[date.getUTCMinutes()]}:` +
    `${IMFPaddedNumbers[date.getUTCSeconds()]} GMT`
}
```

**性能优化**: 使用预计算的查找表 (`IMFPaddedNumbers[0..60]`, `IMFDays`, `IMFMonths`) 避免运行时格式化开销。这种微优化在高频场景 (大量 Set-Cookie) 下有意义。


---

## 9. WebIDL 类型系统深度分析

**文件**: `lib/web/webidl/index.js` (1,004 行)

### 7.1 架构概览

undici 的 WebIDL 实现是一个精简但完整的类型系统，包含：
- **类型转换器** (`converters.*`): JavaScript 值 -> WebIDL 类型
- **类型检查器** (`is.*`): 值是否是某种类型
- **工具函数** (`util.*`): 类型判定、整数转换等
- **错误工厂** (`errors.*`): 标准化的类型错误
- **属性标志** (`attributes.*`): Clamp/EnforceRange/AllowShared 等

### 7.2 Brand Check

Brand check 是 Web API 安全的基础，防止用户将一个类的实例传给另一个类的方法：

```javascript
// 单类型 brand check
webidl.brandCheck = function (V, I) {
  // 使用 Symbol.hasInstance (Function.prototype[Symbol.hasInstance])
  if (!FunctionPrototypeSymbolHasInstance(I, V)) {
    const err = new TypeError('Illegal invocation')
    err.code = 'ERR_INVALID_THIS'  // Node.js 兼容错误码
    throw err
  }
}

// 多类型 brand check (任一类型匹配即可)
webidl.brandCheckMultiple = function (List) {
  const prototypes = List.map(c => webidl.util.MakeTypeAssertion(c))
  return (V) => {
    if (prototypes.every(typeCheck => !typeCheck(V))) {
      const err = new TypeError('Illegal invocation')
      err.code = 'ERR_INVALID_THIS'
      throw err
    }
  }
}
```

**设计意义**: `brandCheck` 通过 `Symbol.hasInstance` 实现精确的实例类型检查，比 `instanceof` 更严格 (不易被原型链篡改绕过)。

### 7.3 基础类型转换器

```javascript
// DOMString: 类型安全的字符串
webidl.converters.DOMString = function (V) {
  // [LegacyNullToEmptyString] 扩展属性: null -> ''
  if (V === null && HasFlag(flags, attributes.LegacyNullToEmptyString)) {
    return ''
  }
  if (typeof V === 'symbol') throw new TypeError('Cannot convert a Symbol value to a string')
  return String(V)
}

// USVString: Unicode 标量值字符串
webidl.converters.USVString = function (value) {
  if (typeof value === 'string') return value.toWellFormed()
  return `${value}`.toWellFormed()  // 替换不配对的代理项
}

// ByteString: 字节字符串 (所有字符 <= 0xFF)
webidl.converters.ByteString = function (V) {
  const x = String(V)
  for (let i = 0; i < x.length; i++) {
    if (x.charCodeAt(i) > 255) {
      throw new TypeError('Argument is not a valid ByteString')
    }
  }
  return x
}

// boolean
webidl.converters.boolean = function (V) {
  return Boolean(V)
}

// 整数类型
webidl.converters['long long'] = function (V) {
  return webidl.util.ConvertToInt(V, 64, 'signed')
}

webidl.converters['unsigned long'] = function (V) {
  return webidl.util.ConvertToInt(V, 32, 'unsigned')
}

webidl.converters['unsigned long long'] = function (V) {
  return webidl.util.ConvertToInt(V, 64, 'unsigned')
}

webidl.converters['unsigned short'] = function (V, prefix, argument, flags) {
  return webidl.util.ConvertToInt(V, 16, 'unsigned', flags)
}
```

### 7.4 `ConvertToInt()` -- 整数转换核心

```javascript
webidl.util.ConvertToInt = function (V, bitLength, signedness, flags) {
  let upperBound, lowerBound

  if (bitLength === 64) {
    // 64 位: 使用 JS 安全整数范围 (2^53 - 1)
    upperBound = Math.pow(2, 53) - 1
    lowerBound = signedness === 'unsigned' ? 0 : -Math.pow(2, 53) + 1
  } else if (signedness === 'unsigned') {
    lowerBound = 0
    upperBound = Math.pow(2, bitLength) - 1
  } else {
    lowerBound = -Math.pow(2, bitLength - 1)
    upperBound = Math.pow(2, bitLength - 1) - 1
  }

  let x = Number(V)
  if (x === 0) x = 0  // -0 -> +0

  // === [EnforceRange]: 严格范围检查 ===
  if (webidl.util.HasFlag(flags, webidl.attributes.EnforceRange)) {
    if (Number.isNaN(x) || !Number.isFinite(x)) {
      throw TypeError('Value is not a finite number')
    }
    x = webidl.util.IntegerPart(x)
    if (x < lowerBound || x > upperBound) {
      throw TypeError('Value is outside the valid range')
    }
    return x
  }

  // === [Clamp]: 钳位到范围 ===
  if (!Number.isNaN(x) && webidl.util.HasFlag(flags, webidl.attributes.Clamp)) {
    x = Math.min(Math.max(x, lowerBound), upperBound)
    // Banker's rounding (四舍六入五成双)
    if (Math.floor(x) % 2 === 0) x = Math.floor(x)
    else x = Math.ceil(x)
    return x
  }

  // === 默认: 模运算 ===
  if (Number.isNaN(x) || x === 0 || !Number.isFinite(x)) return 0
  x = webidl.util.IntegerPart(x)
  x = x % Math.pow(2, bitLength)
  if (signedness === 'signed' && x >= Math.pow(2, bitLength - 1)) {
    x -= Math.pow(2, bitLength)
  }
  return x
}
```

**三种整数处理模式**: EnforceRange(严格，越界抛异常)、Clamp(钳位到边界 + Banker's rounding)、默认(模运算溢出回绕) -- 与 WebIDL 规范完全对齐。

### 7.5 BufferSource 处理

```javascript
webidl.util.getCopyOfBytesHeldByBufferSource = function (bufferSource) {
  let jsArrayBuffer = bufferSource
  let offset = 0, length = 0

  // TypedArray/DataView: 提取底层 buffer + offset + length
  if (types.isTypedArray(bufferSource) || types.isDataView(bufferSource)) {
    jsArrayBuffer = bufferSource.buffer
    offset = bufferSource.byteOffset
    length = bufferSource.byteLength
  } else {
    // ArrayBuffer: 直接使用
    length = bufferSource.byteLength
  }

  // 已分离的 buffer 返回空
  if (jsArrayBuffer.detached) return new Uint8Array(0)

  // 拷贝字节 (安全: 防止原始 buffer 后续被修改)
  const bytes = new Uint8Array(length)
  const view = new Uint8Array(jsArrayBuffer, offset, length)
  bytes.set(view)
  return bytes
}
```

**安全设计**: 始终拷贝字节而非引用，防止原始 buffer 被分离(detached)或修改影响已提取的数据。

### 7.6 序列与记录转换器

```javascript
// 序列转换器: iterable -> 类型化数组
webidl.sequenceConverter = function (converter) {
  return (V, prefix, argument, Iterable) => {
    if (webidl.util.Type(V) !== OBJECT) throw TypeError(...)

    // 获取迭代器
    const method = typeof Iterable === 'function' ? Iterable() : V?.[Symbol.iterator]?.()
    if (method === undefined || typeof method.next !== 'function') throw TypeError(...)

    // 迭代并转换每个元素
    const seq = []
    let index = 0
    while (true) {
      const { done, value } = method.next()
      if (done) break
      seq.push(converter(value, prefix, `${argument}[${index++}]`))
    }
    return seq
  }
}

// 记录转换器: object -> 键值对记录
webidl.recordConverter = function (keyConverter, valueConverter) {
  return (O, prefix, argument) => {
    if (webidl.util.Type(O) !== OBJECT) throw TypeError(...)

    const result = {}

    // 区分 Proxy 和普通对象的键获取方式
    if (!types.isProxy(O)) {
      const keys = [
        ...Object.getOwnPropertyNames(O),
        ...Object.getOwnPropertySymbols(O)
      ]
      for (const key of keys) {
        result[keyConverter(key, ...)] = valueConverter(O[key], ...)
      }
    } else {
      // Proxy 对象: 使用 Reflect API 避免触发副作用
      const keys = Reflect.ownKeys(O)
      for (const key of keys) {
        const desc = Reflect.getOwnPropertyDescriptor(O, key)
        if (desc?.enumerable) {
          result[keyConverter(key, ...)] = valueConverter(O[key], ...)
        }
      }
    }

    return result
  }
}
```

### 7.7 字典转换器

```javascript
webidl.dictionaryConverter = function (converters) {
  // WebIDL 规范要求字典键按字典序排序
  converters.sort((a, b) => (a.key > b.key) - (a.key < b.key))

  return (dictionary, prefix, argument) => {
    const dict = {}

    // 验证输入是对象
    if (dictionary != null && webidl.util.Type(dictionary) !== OBJECT) {
      throw webidl.errors.exception(...)
    }

    for (const options of converters) {
      const { key, defaultValue, required, converter, allowedValues } = options

      // 必需键检查
      if (required && (dictionary == null || !Object.hasOwn(dictionary, key))) {
        throw webidl.errors.exception({
          header: prefix,
          message: `Missing required key "${key}".`
        })
      }

      let value = dictionary?.[key]

      // 默认值处理 (工厂函数)
      if (defaultValue !== undefined && value === undefined) {
        value = defaultValue()
      }

      // 类型转换
      if (required || defaultValue !== undefined || value !== undefined) {
        value = converter(value, prefix, `${argument}.${key}`)

        // 允许值枚举检查
        if (allowedValues && !allowedValues.includes(value)) {
          throw webidl.errors.exception(...)
        }

        dict[key] = value
      }
    }

    return dict
  }
}
```

**字典序排序**: WebIDL 规范要求字典成员按字典序处理，这确保了不同实现间的确定性行为。

### 7.8 属性标志系统

```javascript
webidl.attributes = {
  Clamp: 1 << 0,                    // 0b00001
  EnforceRange: 1 << 1,             // 0b00010
  AllowShared: 1 << 2,              // 0b00100
  AllowResizable: 1 << 3,           // 0b01000
  LegacyNullToEmptyString: 1 << 4   // 0b10000
}

webidl.util.HasFlag = function (flags, attributes) {
  return typeof flags === 'number' && (flags & attributes) === attributes
}
```

**位标志设计**: 使用位运算高效检查多个属性标志，比对象属性查找更快。单次检查只需一次 AND 运算。

---

## 10. 基础设施模块

### 8.1 WHATWG Infra 规范

**文件**: `lib/web/infra/index.js` (230 行)

#### 8.1.1 序列收集

```javascript
// 通用版本: 条件函数驱动
function collectASequenceOfCodePoints (condition, input, position) {
  let result = ''
  while (position.position < input.length &&
         condition(input[position.position])) {
    result += input[position.position]
    position.position++
  }
  return result
}

// 快速版本: 单字符比较 (使用 String.indexOf)
function collectASequenceOfCodePointsFast (char, input, position) {
  const idx = input.indexOf(char, position.position)
  const start = position.position

  if (idx === -1) {
    position.position = input.length
    return input.slice(start)
  }

  position.position = idx
  return input.slice(start, idx)
}
```

**性能优化**: `Fast` 版本使用 `String.indexOf()` 替代逐字符比较，对于简单分隔符场景快 10-100 倍。`String.indexOf` 内部使用 SIMD 优化的 memchr 实现。

#### 8.1.2 Forgiving Base64

```javascript
function forgivingBase64 (data) {
  // 1. 移除 ASCII 空白 (TAB, LF, CR, SPACE)
  data = data.replace(/[\t\n\r ]/g, '')

  let dataLength = data.length

  // 2. 处理 padding (=)
  if (dataLength % 4 === 0) {
    if (data.charCodeAt(dataLength - 1) === 0x003D) {
      --dataLength
      if (data.charCodeAt(dataLength - 1) === 0x003D) --dataLength
    }
  }

  // 3. 长度校验: 去掉 padding 后长度不能是 4n+1
  if (dataLength % 4 === 1) return 'failure'

  // 4. 字符校验: 只允许 + / 0-9 A-Z a-z
  if (/[^+/0-9A-Za-z]/.test(data.slice(0, dataLength))) return 'failure'

  // 5. 解码
  const buffer = Buffer.from(data, 'base64')
  return new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength)
}
```

**"Forgiving"**: 与标准 Base64 的区别在于允许忽略 padding 和空白，这是 Web 标准中常见的容错设计。

#### 8.1.3 同构编解码

```javascript
// 同构解码: 字节 -> 字符串 (每个字节映射到同值的码点)
function isomorphicDecode (input) {
  const length = input.length
  // 批量处理 (每次 65535 字节, 避免 apply 参数过多)
  const batchSize = (2 << 15) - 1  // 65535

  if (batchSize > length) {
    return String.fromCharCode.apply(null, input)
  }

  let result = ''
  let i = 0
  while (i < length) {
    let end = i + batchSize
    if (end > length) end = length
    result += String.fromCharCode.apply(null, input.subarray(i, end))
    i = end
  }
  return result
}

// 同构编码: 字符串 -> 字节 (每个码点映射到同值的字节)
function isomorphicEncode (input) {
  // 断言所有字符在 0x00-0xFF 范围内
  for (let i = 0; i < input.length; i++) {
    if (input.charCodeAt(i) > 0xFF) throw new TypeError('...')
  }
  return input
}
```

### 8.2 WHATWG Encoding

**文件**: `lib/encoding/index.js` (34 行)

```javascript
const textDecoder = new TextDecoder()  // 复用单个实例

function utf8DecodeBytes (buffer) {
  if (buffer.length === 0) return ''

  // 跳过 UTF-8 BOM (0xEF 0xBB 0xBF)
  if (buffer[0] === 0xEF && buffer[1] === 0xBB && buffer[2] === 0xBF) {
    buffer = buffer.subarray(3)
  }

  return textDecoder.decode(buffer)
}
```

**设计**: 复用单个 `TextDecoder` 实例避免重复创建。`TextDecoder` 内部维护编码状态，对于 UTF-8 来说是无状态的，可以安全复用。

### 8.3 Data URL 与 MIME 解析

**文件**: `lib/web/fetch/data-url.js` (596 行)

#### 8.3.1 Data URL 处理器

```javascript
function dataURLProcessor (dataURL) {
  // 1. 序列化 URL (排除 fragment)
  let input = URLSerializer(dataURL, true)

  // 2. 去掉 "data:" 前缀
  input = input.slice(5)

  // 3. 提取 MIME 类型 (逗号前的部分)
  const position = { position: 0 }
  let mimeType = collectASequenceOfCodePointsFast(',', input, position)
  mimeType = removeASCIIWhitespace(mimeType, true, true)  // 去首尾空白

  // 4. 提取编码后的 body (逗号后的部分)
  const encodedBody = input.slice(mimeType.length + 1)
  let body = stringPercentDecode(encodedBody)

  // 5. Base64 检测: MIME 类型以 ;base64 结尾
  if (/;(?: *)base64$/ui.test(mimeType)) {
    const stringBody = isomorphicDecode(body)
    body = forgivingBase64(stringBody)
    if (body === 'failure') return 'failure'

    // 清理 MIME 类型中的 ;base64 后缀
    mimeType = mimeType.slice(0, -6).replace(/( +)$/u, '').slice(0, -1)
  }

  // 6. 默认 MIME 类型: 如果以 ; 开头, 补充 text/plain
  if (mimeType.startsWith(';')) mimeType = 'text/plain' + mimeType

  // 7. 解析 MIME 类型
  let mimeTypeRecord = parseMIMEType(mimeType)
  if (mimeTypeRecord === 'failure') {
    // 默认: text/plain;charset=US-ASCII
    mimeTypeRecord = parseMIMEType('text/plain;charset=US-ASCII')
  }

  return { mimeType: mimeTypeRecord, body }
}
```

#### 8.3.2 MIME 类型解析

```javascript
function parseMIMEType (input) {
  input = removeHTTPWhitespace(input, true, true)
  const position = { position: 0 }

  // 提取 type (slash 之前的部分)
  const type = collectASequenceOfCodePointsFast('/', input, position)
  if (type.length === 0 || !HTTP_TOKEN_CODEPOINTS.test(type)) return 'failure'

  position.position++  // 跳过 /

  // 提取 subtype (分号或末尾之前的部分)
  let subtype = collectASequenceOfCodePointsFast(';', input, position)
  subtype = removeHTTPWhitespace(subtype, false, true)
  if (subtype.length === 0 || !HTTP_TOKEN_CODEPOINTS.test(subtype)) return 'failure'

  // 创建 MIME 记录
  const mimeType = {
    type: type.toLowerCase(),
    subtype: subtype.toLowerCase(),
    parameters: new Map(),
    essence: `${type.toLowerCase()}/${subtype.toLowerCase()}`
  }

  // 解析参数 (分号分隔)
  while (position.position < input.length) {
    position.position++  // 跳过 ;

    // 跳过空白
    collectASequenceOfCodePoints(
      char => HTTP_WHITESPACE_REGEX.test(char), input, position
    )

    // 参数名 (到 = 或 ; 或末尾)
    let parameterName = collectASequenceOfCodePoints(
      char => char !== ';' && char !== '=', input, position
    ).toLowerCase()

    if (position.position < input.length) {
      if (input[position.position] === ';') continue  // 空参数值
      position.position++  // 跳过 =
    }

    // 参数值 (可能带引号)
    let parameterValue
    if (input[position.position] === '"') {
      // 引号值: 支持转义
      parameterValue = collectAnHTTPQuotedString(input, position, true)
      collectASequenceOfCodePointsFast(';', input, position)
    } else {
      // 非引号值
      parameterValue = collectASequenceOfCodePointsFast(';', input, position)
      parameterValue = removeHTTPWhitespace(parameterValue, false, true)
    }

    // 去重: 同名参数只保留第一个
    if (parameterName.length !== 0 && !mimeType.parameters.has(parameterName)) {
      mimeType.parameters.set(parameterName, parameterValue)
    }
  }

  return mimeType
}
```

#### 8.3.3 HTTP 引用字符串收集

```javascript
function collectAnHTTPQuotedString (input, position, extractValue = false) {
  const positionStart = position.position
  let value = ''

  assert(input[position.position] === '"')
  position.position++  // 跳过开头引号

  while (true) {
    // 收集非 " 和 \ 的字符
    value += collectASequenceOfCodePoints(
      char => char !== '"' && char !== '\\', input, position
    )

    if (position.position >= input.length) break

    const quoteOrBackslash = input[position.position]
    position.position++

    if (quoteOrBackslash === '\\') {
      // 转义字符
      if (position.position >= input.length) { value += '\\'; break }
      value += input[position.position]  // 取转义后的字符
      position.position++
    } else {
      break  // 关闭引号
    }
  }

  // extractValue: true 返回值, false 返回完整引号字符串
  return extractValue ? value : input.slice(positionStart, position.position)
}
```

---

## 11. FormData 模块深度分析

**文件**: `lib/web/fetch/formdata.js` (278 行)

### 9.1 FormData 类

```javascript
class FormData {
  #state = []    // { name, value }[] 数组

  append (name, value, filename) {
    name = webidl.converters.USVString(name)

    if (webidl.is.Blob(value)) {
      // Blob/File 值
      value = webidl.converters.Blob(value)
      if (filename !== undefined) {
        filename = webidl.converters.USVString(filename)
      }
    } else {
      // 字符串值
      value = webidl.converters.USVString(value)
    }

    const entry = makeEntry(name, value, filename)
    this.#state.push(entry)
  }

  delete (name) {
    name = webidl.converters.USVString(name)
    this.#state = this.#state.filter(entry => entry.name !== name)
  }

  get (name) {
    name = webidl.converters.USVString(name)
    const idx = this.#state.findIndex(entry => entry.name === name)
    return idx === -1 ? null : this.#state[idx].value
  }

  getAll (name) {
    name = webidl.converters.USVString(name)
    return this.#state
      .filter(entry => entry.name === name)
      .map(entry => entry.value)
  }

  has (name) {
    name = webidl.converters.USVString(name)
    return this.#state.some(entry => entry.name === name)
  }

  set (name, value, filename) {
    name = webidl.converters.USVString(name)
    // ... WebIDL 转换

    const entry = makeEntry(name, value, filename)
    const idx = this.#state.findIndex(entry => entry.name === name)

    if (idx !== -1) {
      // 替换第一个匹配项, 删除其余同名项
      this.#state = [
        ...this.#state.slice(0, idx),
        entry,
        ...this.#state.slice(idx + 1).filter(e => e.name !== name)
      ]
    } else {
      this.#state.push(entry)
    }
  }
}
```

### 9.2 Entry 创建与边界生成

```javascript
function makeEntry (name, value, filename) {
  if (typeof value === 'string') {
    // 字符串值: 直接使用
  } else {
    // Blob/File 值
    if (!webidl.is.File(value)) {
      // 非 File 的 Blob 包装为 File (名称 "blob")
      value = new File([value], 'blob', { type: value.type })
    }
    if (filename !== undefined) {
      // 用指定文件名创建新 File
      value = new File([value], filename, {
        type: value.type,
        lastModified: value.lastModified
      })
    }
  }
  return { name, value }
}

// Boundary 生成: "----formdata-undici-0" + 11 位随机数
static getFormDataBoundary (formData) {
  const boundary = formData.#boundary
  if (boundary != null) return boundary

  return formData.#boundary =
    `----formdata-undici-0${random(1e11).toString().padStart(11, '0')}`
}
```

### 9.3 迭代器支持

```javascript
iteratorMixin('FormData', FormData, getFormDataState, 'name', 'value')
```

这使得 `FormData` 支持 `for...of` 迭代:
```javascript
for (const [name, value] of formData) {
  console.log(name, value)
}
```

`iteratorMixin` 是 undici 的通用迭代器混入工具，为任何类添加 `entries()`, `keys()`, `values()`, `forEach()`, `Symbol.iterator` 方法。

---

## 12. 自定义事件体系

**文件**: `lib/web/websocket/events.js` (332 行)

### 10.1 MessageEvent

```javascript
class MessageEvent extends Event {
  #eventInit

  constructor (type, eventInitDict = {}) {
    // 快速构造路径 (内部使用): 跳过 WebIDL 验证
    if (type === kConstruct) {
      super(arguments[1], arguments[2])
      return
    }

    // 正常构造路径
    super(type, eventInitDict)
    this.#eventInit = eventInitDict
  }

  get data () { return this.#eventInit.data }
  get origin () { return this.#eventInit.origin ?? '' }
  get lastEventId () { return this.#eventInit.lastEventId ?? '' }
  get source () { return this.#eventInit.source ?? null }
  get ports () {
    if (!Object.isFrozen(this.#eventInit.ports)) {
      Object.freeze(this.#eventInit.ports)  // 惰性冻结
    }
    return this.#eventInit.ports ?? Object.freeze([])
  }

  // 高速工厂方法: 跳过 WebIDL 类型检查
  static createFastMessageEvent (type, init) {
    const messageEvent = new MessageEvent(kConstruct, type, init)
    messageEvent.#eventInit = init
    return messageEvent
  }
}
```

**性能优化**: `createFastMessageEvent()` 通过 `kConstruct` 哨兵值跳过构造器中的 WebIDL 验证，用于高频的内部事件分发。在 SSE 解析器的热路径中，每条消息都会创建 MessageEvent，这个优化很重要。

### 10.2 CloseEvent

```javascript
class CloseEvent extends Event {
  #eventInit

  constructor (type, eventInitDict = {}) {
    super(type, eventInitDict)
    this.#eventInit = eventInitDict
  }

  get wasClean () { return this.#eventInit.wasClean ?? false }
  get code () { return this.#eventInit.code ?? 0 }
  get reason () { return this.#eventInit.reason ?? '' }
}
```

### 10.3 ErrorEvent

```javascript
class ErrorEvent extends Event {
  #eventInit

  constructor (type, eventInitDict = {}) {
    super(type, eventInitDict)
    this.#eventInit = eventInitDict
  }

  get message () { return this.#eventInit.message ?? '' }
  get filename () { return this.#eventInit.filename ?? '' }
  get lineno () { return this.#eventInit.lineno ?? 0 }
  get colno () { return this.#eventInit.colno ?? 0 }
  get error () { return this.#eventInit.error ?? undefined }
}
```

**共同设计模式**: 所有自定义事件都使用 `#eventInit` 私有字段存储初始化字典，getter 方法提供安全的默认值。这避免了直接暴露可变状态。


---

## 13. 对 AI Agent HTTP 传输层的借鉴

### 11.1 Fetch 流式回调模式 -> laew Agent 循环

undici 的 `fetching()` 使用回调驱动的异步模式，而不是简单的 `await`:

```javascript
// undici 模式: 回调驱动
fetching({
  request,
  processResponse (response) { ... },      // 响应头到达
  processResponseEndOfBody (response) { ... }, // 响应体完成
  processResponseConsumeBody (response, data) { ... } // 响应体数据
})
```

**laew 借鉴**: Agent 循环可以采用类似的回调驱动模式:

```
// laew 可以借鉴的模式
runAgentLoop({
  onTaskStart (task) { ... },       // 任务开始
  onLLMResponse (response) { ... }, // LLM 响应头/首 token
  onToolCall (tool) { ... },        // 工具调用
  onToolResult (result) { ... },    // 工具结果
  onTaskComplete (result) { ... }   // 任务完成
})
```

回调驱动模式的优势是将异步流程分解为多个阶段，每个阶段可以独立处理错误和状态转换。

### 11.2 Content-Encoding 管线 -> Agent 响应解压

undici 支持 5 种内容编码 (gzip, deflate, br, zstd)，通过流式管线解压:

```javascript
pipeline = response.body.stream
pipeline = pipeline.pipe(zlib.createGunzip())
pipeline = pipeline.pipe(zlib.createBrotliDecompress())
```

**laew 借鉴**: 如果 LLM API 返回压缩响应 (如 Anthropic 支持 br)，可以复用 undici 的解压管线设计。

### 11.3 Headers Guard -> Agent 权限控制

undici 的 Headers Guard 系统可以借鉴到 Agent 的请求权限控制:

```
AgentRequest {
  guard: 'none' | 'restricted' | 'immutable'
  // restricted: 禁止修改 Authorization 等敏感 header
  // immutable: 子 Agent 不能修改父 Agent 的请求头
}
```

### 11.4 Proxy 过滤响应 -> Agent 响应脱敏

undici 的 `filterResponse()` 使用 Proxy 实现透明的响应过滤:

```javascript
const filtered = new Proxy(response, {
  get (target, prop) {
    if (prop === 'status') return type === 'opaque' ? 0 : target.status
    return Reflect.get(target, prop)
  }
})
```

**laew 借鉴**: 可以用 Proxy 对 LLM 响应进行脱敏处理，过滤敏感信息后再传递给上层 Agent。例如，SubAgent 的响应可以过滤掉包含 API key 的内容。

### 11.5 WebSocketStream -> Agent 双向通信

WebSocketStream 将 WebSocket 包装为标准的 ReadableStream + WritableStream:

```javascript
const { readable, writable } = await wsStream.opened
// 可以与 pipeTo/pipeFrom 集成
readable.pipeTo(transformStream).pipeTo(writable)
```

**laew 借鉴**: 如果需要与 LLM 建立持久连接 (如 WebSocket API)，可以用类似的 Stream 包装模式。

### 11.6 FinalizationRegistry -> Agent 资源清理

undici 使用 `FinalizationRegistry` 自动清理未消费的请求:

```javascript
const requestFinalizer = new FinalizationRegistry(({ stream, controller }) => {
  if (stream && !stream.locked) {
    controller.abort(new TypeError('Request was garbage collected'))
  }
})
```

**laew 借鉴**: Agent 的 AbortController 和 Session 资源可以用类似模式自动清理，防止内存泄漏。

### 11.7 SSE 流式解析 -> LLM 流式响应

undici 的 `EventSourceStream` 是一个完整的 SSE 解析器:

- BOM 处理
- 逐字节解析
- 跨 chunk 行读取
- 自动重连 + Last-Event-ID 断点续传

**laew 借鉴**: LLM 的流式响应 (SSE) 解析可以直接复用 `EventSourceStream`，或者参考其设计实现专用的 SSE 解析器。关键优化点:
- 零拷贝 `subarray()` 用于单 chunk 行
- `Buffer` 比较避免 UTF-8 解码开销
- 及时释放已消费 chunk 防止内存泄漏

### 11.8 permessage-deflate -> Agent 消息压缩

undici 的 WebSocket 压缩扩展:

```javascript
class PerMessageDeflate {
  decompress (chunk, fin, callback) {
    this.#inflate.write(chunk)
    if (fin) this.#inflate.write(Buffer.from([0x00, 0x00, 0xFF, 0xFF]))
    this.#inflate.flush(() => callback(null, Buffer.concat(...)))
  }
}
```

**laew 借鉴**: 对于大规模 Agent 协作场景，消息压缩可以显著减少传输开销。

### 11.9 Cookie 管理 -> Agent Session 持久化

undici 的 Cookie 管理模式:

```javascript
getCookies(headers)
setCookie(headers, { name, value, expires, maxAge, domain, path, secure, httpOnly, sameSite })
deleteCookie(headers, name)
```

**laew 借鉴**: Agent 的 Session 状态可以借鉴 Cookie 的过期策略 (expires/maxAge) 和域隔离 (domain/path) 机制。

### 11.10 kConstruct 快速路径 -> Agent 内部高性能通道

undici 使用 `kConstruct` 哨兵值区分内部和外部构造:

```javascript
// 构造器: 检查哨兵值
constructor (type, eventInitDict) {
  if (type === kConstruct) {
    super(arguments[1], arguments[2])
    return  // 跳过 WebIDL 验证
  }
  // ... 正常构造 (带 WebIDL 验证)
}

// 内部工厂: 传入哨兵值
static createFast (type, init) {
  return new MyClass(kConstruct, type, init)  // 跳过验证
}
```

**laew 借鉴**: Agent 内部消息传递可以使用类似的模式，区分公开 API (严格验证) 和内部通道 (快速路径)。

---

## 14. 跨模块设计模式汇总

### 12.1 规范步骤注释

undici 的每个函数都严格按规范步骤注释:

```javascript
// 1. Let ev be a new EventSource object.
// 2. Let settings be ev's relevant settings object.
// 3. Let urlRecord be the result of encoding-parsing a URL given url.
// 4. If urlRecord is failure, then throw a "SyntaxError" DOMException.
// 5. Set ev's url to urlRecord.
```

这种注释方式使得代码审查和规范更新追踪变得容易。当规范更新时，只需找到对应步骤的代码进行修改。

### 12.2 Inner/Outer 分离

Request 和 Response 都有 inner state 和 outer wrapper 两层:

- `makeRequest()` / `fromInnerRequest()`: 创建内部状态 / 从内部状态创建公开对象
- `getRequestState()` / `getResponseState()`: 获取内部状态 (通过 `kState` 符号)

这种分离使得:
1. 内部操作不需要 WebIDL 验证 (直接操作 inner state)
2. 公开 API 可以设置不同的 Guard (request/response/immutable)
3. 克隆操作只影响内部状态，不影响持有引用的代码

### 12.3 kConstruct 快速路径

```javascript
// 区分公开构造和内部构造
if (type === kConstruct) {
  super(arguments[1], arguments[2])
  return  // 跳过 WebIDL 验证
}
```

**性能收益**: 内部高频路径 (如 SSE 事件创建) 跳过 WebIDL 验证，减少对象创建开销。

### 12.4 FinalizationRegistry + WeakRef

用于自动资源清理:
- `requestFinalizer`: Request 被 GC 时自动 abort
- `streamRegistry`: Response 被 GC 时自动取消未消费的 stream
- `dependentControllerMap`: WeakMap 跟踪克隆关系

### 12.5 Proxy 过滤模式

Response 的 filtered 版本使用 Proxy，无需创建子类:
- `basic`: 过滤 Set-Cookie
- `cors`: 只暴露 CORS 允许的 header
- `opaque`: 隐藏所有信息

### 12.6 位标志属性

WebIDL 属性使用位运算:
```javascript
Clamp: 1 << 0, EnforceRange: 1 << 1, AllowShared: 1 << 2
HasFlag = (flags, attr) => (flags & attr) === attr
```

### 12.7 预计算查找表

Cookie 模块使用预计算的查找表:
```javascript
const IMFPaddedNumbers = Array.from({ length: 61 }, (_, i) =>
  i.toString().padStart(2, '0')
)
```

### 12.8 零拷贝 Buffer 操作

多个模块使用 `subarray()` 避免内存分配:
- ByteParser: 单 buffer 场景使用 `subarray()`
- EventSourceStream: 单 chunk 行使用 `subarray()`
- FormData: entries 使用 `subarray()`

### 12.9 条件函数 vs 快速路径

infra 模块提供两个版本的序列收集:
- `collectASequenceOfCodePoints(condition, ...)`: 通用版本
- `collectASequenceOfCodePointsFast(char, ...)`: 快速版本 (indexOf)

根据场景选择合适的版本。

### 12.10 事件工厂方法

所有自定义事件都提供静态工厂方法:
```javascript
static createFastMessageEvent (type, init) {
  return new MessageEvent(kConstruct, type, init)  // 跳过验证
}
```

---

## 15. laew 借鉴路线图

### P0 (立即可做)

| 编号 | 借鉴项 | 来源模块 | 说明 |
|------|--------|---------|------|
| P0-1 | SSE 流式解析器 | EventSourceStream | 复用或参考其 SSE 解析器处理 LLM 流式响应 |
| P0-2 | WebIDL brand check | webidl/index.js | 为 Agent 公开 API 添加类型安全检查 |
| P0-3 | Inner/Outer 分离 | request.js / response.js | Agent 内部状态与外部 API 分离 |
| P0-4 | Content-Encoding 管线 | fetch/index.js | 支持 gzip/br/zstd 响应解压 |
| P0-5 | 零拷贝 Buffer 处理 | receiver.js / eventsource-stream.js | 工具输出的 Buffer 处理优化 |

### P1 (近期规划)

| 编号 | 借鉴项 | 来源模块 | 说明 |
|------|--------|---------|------|
| P1-1 | 回调驱动异步模式 | fetching() | Agent 循环用回调替代阻塞 await |
| P1-2 | Proxy 响应过滤 | response.js | LLM 响应脱敏 |
| P1-3 | Headers Guard | headers.js | Agent 请求权限控制 |
| P1-4 | FinalizationRegistry | request.js / body.js | Agent 资源自动清理 |
| P1-5 | kConstruct 快速路径 | events.js | 内部高频路径跳过验证 |
| P1-6 | 预计算查找表 | cookies/util.js | IMF 日期/常量表预计算 |

### P2 (长期规划)

| 编号 | 借鉴项 | 来源模块 | 说明 |
|------|--------|---------|------|
| P2-1 | WebSocketStream 包装 | stream/websocketstream.js | Agent 双向持久通信 |
| P2-2 | permessage-deflate | permessage-deflate.js | Agent 消息压缩 |
| P2-3 | Cookie 过期策略 | cookies/ | Session 状态过期管理 |
| P2-4 | Cache 事务性操作 | cache/cache.js | Agent 结果缓存 (带原子批量操作) |
| P2-5 | Port 安全列表 | fetch/constants.js | Agent 网络访问控制 |
| P2-6 | 字典转换器 | webidl/index.js | Agent 配置验证 |

---

## 附录 A: 文件索引

| 文件路径 | 行数 | 职责 |
|---------|------|------|
| `lib/web/fetch/index.js` | 2,426 | Fetch 核心循环 (fetching/mainFetch/schemeFetch/httpFetch) |
| `lib/web/fetch/util.js` | 1,525 | 规范工具函数 (referrer/MIME/InflateStream/端口检查) |
| `lib/web/fetch/request.js` | 1,144 | Request 类 (41 步构造器/AbortController/克隆) |
| `lib/web/fetch/response.js` | 639 | Response 类 (Proxy 过滤/静态工厂) |
| `lib/web/fetch/headers.js` | 719 | Headers + HeadersList (Guard/二分排序/cookie 特殊) |
| `lib/web/fetch/body.js` | 547 | Body mixin (extractBody/consumeBody/7 种消费方式) |
| `lib/web/fetch/data-url.js` | 596 | Data URL 处理 + MIME 类型解析 |
| `lib/web/fetch/formdata.js` | 278 | FormData (append/delete/get/set + iterator) |
| `lib/web/fetch/constants.js` | 131 | 规范常量 (CORS/重定向/禁止端口/策略) |
| `lib/web/fetch/global.js` | 40 | 全局 Origin 管理 |
| `lib/web/websocket/websocket.js` | 781 | WebSocket 主类 (send/close/handler) |
| `lib/web/websocket/connection.js` | 330 | WebSocket 握手 (nonce/SHA-1 验证) |
| `lib/web/websocket/frame.js` | 128 | 帧编解码 (masking/快速文本帧) |
| `lib/web/websocket/receiver.js` | 508 | 接收解析器 (状态机/零拷贝/控制帧) |
| `lib/web/websocket/sender.js` | 110 | 发送队列 (cork/uncork 优化) |
| `lib/web/websocket/permessage-deflate.js` | 100 | 压缩扩展 (RFC 7692) |
| `lib/web/websocket/events.js` | 332 | MessageEvent/CloseEvent/ErrorEvent |
| `lib/web/websocket/constants.js` | 127 | WebSocket 常量 |
| `lib/web/websocket/stream/websocketstream.js` | 498 | WebSocketStream (Readable+Writable) |
| `lib/web/websocket/stream/websocketerror.js` | 104 | WebSocketError (closeCode/reason) |
| `lib/web/eventsource/eventsource.js` | 493 | EventSource 主类 (连接/重连/事件分发) |
| `lib/web/eventsource/eventsource-stream.js` | 521 | SSE 流解析器 (逐字节/BOM/跨 chunk) |
| `lib/web/eventsource/util.js` | 60 | CORS 请求创建 |
| `lib/web/cache/cache.js` | 862 | Cache 类 (匹配/批量操作/事务) |
| `lib/web/cache/cachestorage.js` | 152 | CacheStorage (open/delete/keys) |
| `lib/web/cache/util.js` | 45 | URL 比较 + Vary 处理 |
| `lib/web/subresource-integrity/subresource-integrity.js` | 307 | SRI 校验 (bytesMatch/parseMetadata/getStrongestMetadata) |
| `lib/web/cookies/index.js` | 199 | Cookie API (get/set/delete) |
| `lib/web/cookies/parse.js` | 317 | Set-Cookie 解析器 (RFC 6265bis) |
| `lib/web/cookies/util.js` | 353 | Cookie 验证 + 序列化 + IMF 日期 |
| `lib/web/cookies/constants.js` | 12 | Cookie 大小限制 |
| `lib/web/webidl/index.js` | 1,004 | WebIDL 类型系统 (转换/检查/属性) |
| `lib/web/infra/index.js` | 230 | WHATWG Infra (序列收集/Base64/同构) |
| `lib/encoding/index.js` | 34 | UTF-8 解码 |
| `lib/cache/sqlite-cache-store.js` | 469 | SQLite WAL 持久化缓存 |
| `lib/cache/memory-cache-store.js` | 279 | 内存 LRU 缓存 |

**总计**: ~16,300+ 行代码，覆盖 Fetch API / WebSocket / EventSource / Cache API / Cache 存储层 / SRI / Cookies / WebIDL / Infra / Encoding 10 大模块。

---

## 附录 B: 关键函数名速查

### Fetch API
- `fetch()` -- 公共入口
- `fetching()` -- 主编排器
- `Fetch` -- 内部状态机类 (ongoing/terminated/aborted)
- `mainFetch()` -- 协议路由
- `schemeFetch()` -- 本地协议处理 (about/blob/data/file)
- `httpFetch()` -- HTTP 请求处理
- `httpRedirectFetch()` -- 重定向处理 (最多 20 次)
- `httpNetworkOrCacheFetch()` -- 网络/缓存协商
- `httpNetworkFetch()` -- 网络分发 (agent.dispatch)
- `abortFetch()` -- 中止处理
- `finalizeAndReportTiming()` -- 性能计时

### Headers
- `HeadersList.append()` -- 追加 header
- `HeadersList.toSortedArray()` -- 二分排序 (HPACK)
- `Headers.getSetCookie()` -- 获取 Set-Cookie 列表
- `filterHeadersList()` -- 过滤响应头

### Body
- `extractBody()` -- 提取 body (7 种类型)
- `consumeBody()` -- 消费 body
- `fullyReadBody()` -- 完全读取 body
- `readAllBytes()` -- 读取所有字节
- `bodyMixinMethods()` -- 7 种消费方法

### Request/Response
- `makeRequest()` -- 创建内部请求 (30+ 默认字段)
- `fromInnerRequest()` / `fromInnerResponse()` -- 从内部状态创建公开对象
- `cloneRequest()` / `cloneResponse()` -- 克隆请求/响应
- `filterResponse()` -- Proxy 过滤响应
- `makeNetworkError()` -- 创建网络错误响应
- `makeResponse()` -- 创建响应

### WebSocket
- `establishWebSocketConnection()` -- 建立连接 (握手)
- `closeWebSocketConnection()` -- 关闭连接
- `failWebsocketConnection()` -- 连接失败处理
- `ByteParser` -- 帧解析状态机 (INFO/PAYLOADLENGTH_16/PAYLOADLENGTH_64/READ_DATA)
- `WebsocketFrameSend` -- 帧发送 (mask + FIN + opcode)
- `SendQueue` -- 发送队列 (cork/uncork)
- `PerMessageDeflate.decompress()` -- 压缩解压 (RFC 7692 tail bytes)

### EventSource
- `EventSource.#connect()` -- 连接
- `EventSource.#reconnect()` -- 重连 (reconnectionTime + Last-Event-ID)
- `EventSourceStream._transform()` -- SSE 主解析循环
- `EventSourceStream.#parseLine()` -- 行解析 (DATA/EVENT/ID/RETRY)
- `EventSourceStream.#processEvent()` -- 事件分发
- `EventSourceStream.#handleBOM()` -- BOM 处理
- `EventSourceStream.#readLine()` -- 跨 chunk 行读取

### Subresource Integrity
- `bytesMatch()` -- SRI 主入口 (metadata 匹配)
- `parseMetadata()` -- integrity 属性解析
- `getStrongestMetadata()` -- 最强算法选择
- `caseSensitiveMatch()` -- Base64/Base64url 兼容匹配
- `applyAlgorithmToBytes()` -- 哈希计算 (crypto.hash)

### Cache
- `Cache.#queryCache()` -- 缓存查询
- `Cache.#batchCacheOperations()` -- 原子批量操作 (备份/回滚)
- `Cache.#requestMatchesCachedItem()` -- 匹配算法 (URL + Vary)
- `Cache.addAll()` -- 并发 fetch + 批量写入

### Cache 存储层
- `SqliteCacheStore.get/set/createWriteStream/delete` -- SQLite CRUD
- `SqliteCacheStore.#findValue()` -- Vary 匹配查询
- `SqliteCacheStore.#prune()` -- LRU 剪枝 (过期 → 最老 10%)
- `MemoryCacheStore.createWriteStream()` -- 流式写入 + 大小检查
- `headerValueEquals()` -- Vary 头比较 (null 等价)

### Cookies
- `parseSetCookie()` -- Set-Cookie 解析 (RFC 6265bis)
- `parseUnparsedAttributes()` -- 属性递归解析
- `stringify()` -- Cookie 序列化 (含 __Secure-/__Host- 前缀处理)
- `validateCookieName()` / `validateCookieValue()` / `validateCookieDomain()` -- 验证
- `toIMFDate()` -- RFC 7231 日期格式 (预计算查找表)

### WebIDL
- `webidl.brandCheck()` -- 品牌检查 (Symbol.hasInstance)
- `webidl.converters.*` -- 类型转换器 (DOMString/USVString/boolean/整数/...)
- `webidl.util.ConvertToInt()` -- 整数转换 (EnforceRange/Clamp/默认)
- `webidl.dictionaryConverter()` -- 字典转换器 (字典序排序)
- `webidl.sequenceConverter()` -- 序列转换器 (Symbol.iterator)
- `webidl.recordConverter()` -- 记录转换器 (Proxy 感知)
- `webidl.util.getCopyOfBytesHeldByBufferSource()` -- BufferSource 字节拷贝
- `webidl.attributes.*` -- 属性位标志

### Infra
- `collectASequenceOfCodePoints()` / `collectASequenceOfCodePointsFast()` -- 序列收集
- `forgivingBase64()` -- Base64 解码 (容错)
- `isomorphicDecode()` / `isomorphicEncode()` -- 同构编解码
- `parseMIMEType()` -- MIME 类型解析 (type/subtype/parameters)
- `dataURLProcessor()` -- Data URL 处理
- `collectAnHTTPQuotedString()` -- HTTP 引用字符串收集
- `utf8DecodeBytes()` -- UTF-8 解码 (BOM 剥离)

---

## 附录 C: 协议规范对齐索引

| 模块 | 对齐规范 | 关键条款 |
|------|---------|---------|
| Fetch API | WHATWG Fetch Standard | Section 2: Fetching, Section 5: Fetch API |
| Headers | WHATWG Fetch Standard | Section 2.1: Headers class |
| Request | WHATWG Fetch Standard | Section 2.2: Request class (41 步构造器) |
| Response | WHATWG Fetch Standard | Section 2.3: Response class |
| WebSocket | RFC 6455 | Section 4: Opening Handshake, Section 5: Data Framing |
| WebSocket H2 | RFC 8441 | Bootstrapping WebSockets with HTTP/2 (Extended CONNECT) |
| WebSocketStream | W3C WebSocketStream | Stream-based WebSocket API |
| SRI | W3C WebAppSec SRI | Algorithm selection, strongest metadata |
| EventSource | WHATWG HTML Standard | Section 9.2.6: Server-sent events |
| Cache API | W3C Service Worker | Section 4: Cache API |
| Cookies | RFC 6265bis | Section 5.4: Set-Cookie, Section 5.5: Cookie |
| WebIDL | W3C WebIDL | Section 3.2: Types, Section 3.3: Dictionary |
| Infra | WHATWG Infra Standard | Section 4: Byte sequences, Section 8: Encoding |
| MIME | WHATWG MIME Sniffing | Section 2: MIME Type |

---

## 附录 D: 安全设计汇总

| 安全机制 | 来源模块 | 说明 |
|---------|---------|------|
| 端口安全列表 | fetch/constants.js | 70+ 已知不安全端口 (SMTP/Telnet/RPC/NFS) |
| 内容编码层数限制 | fetch/index.js | 最多 5 层 (CVE 修复, 防止解压炸弹) |
| CORS 机制 | fetch/index.js | 三种模式 (no-cors/cors/same-origin) |
| Referrer 策略 | fetch/util.js | 8 种策略 (no-referrer -> unsafe-url) |
| Header Guard | fetch/headers.js | 5 级 guard (none/request/response/immutable/request-no-cors) |
| Brand Check | webidl/index.js | Symbol.hasInstance 精确类型检查 |
| BufferSource 拷贝 | webidl/index.js | 始终拷贝字节, 防止后续修改 |
| WebSocket Mask | websocket/frame.js | 4 字节随机 mask (防缓存投毒) |
| Sec-WebSocket-Accept | websocket/connection.js | SHA-1 + 固定 UID (防握手伪造) |
| SRI 校验 | subresource-integrity/ | SHA-256/384/512 哈希校验 (防篡改) |
| permessage-deflate 大小限制 | websocket/permessage-deflate.js | maxPayloadSize 防止解压炸弹 |
| Cookie CTL 检查 | cookies/parse.js | 控制字符过滤 |
| Cookie 大小限制 | cookies/parse.js | name+value <= 4096 字节 |
| Cookie Domain 验证 | cookies/util.js | RFC 1034/1123 标签规则 |
| Cookie 安全前缀 | cookies/util.js | __Secure- 和 __Host- 前缀强制安全属性 |
| Data URL BOM | eventsource/eventsource-stream.js | BOM 自动剥离 |
| Event ID NULL 检查 | eventsource/util.js | Last-Event-ID 不允许 NULL 字符 |

