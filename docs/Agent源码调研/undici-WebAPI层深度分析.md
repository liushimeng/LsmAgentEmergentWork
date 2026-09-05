# Undici Web API 层深度分析

> 分析日期: 2026-09-05 | 仓库: `/usr/local/LsmGitOpenSource/undici` | 分支: main

## 目录

1. [概览与架构总览](#1-概览与架构总览)
2. [Fetch API 全链路深度分析](#2-fetch-api-全链路深度分析)
3. [WebSocket 协议实现深度分析](#3-websocket-协议实现深度分析)
4. [EventSource / SSE 流式解析深度分析](#4-eventsource--sse-流式解析深度分析)
5. [Cache API 深度分析](#5-cache-api-深度分析)
6. [Cookies 模块深度分析](#6-cookies-模块深度分析)
7. [WebIDL 类型系统深度分析](#7-webidl-类型系统深度分析)
8. [基础设施模块 (infra / encoding / data-url)](#8-基础设施模块)
9. [FormData 模块深度分析](#9-formdata-模块深度分析)
10. [自定义事件体系 (MessageEvent / CloseEvent / ErrorEvent)](#10-自定义事件体系)
11. [对 AI Agent HTTP 传输层的借鉴](#11-对-ai-agent-http-传输层的借鉴)
12. [跨模块设计模式汇总](#12-跨模块设计模式汇总)
13. [laew 借鉴路线图](#13-laew-借鉴路线图)

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
├── websocket/           # WebSocket RFC 6455 (~2,000+ 行)
│   ├── websocket.js     # WebSocket 主类 (781 行)
│   ├── connection.js    # 握手建立连接 (330 行)
│   ├── frame.js         # 帧编解码 (128 行)
│   ├── receiver.js      # 接收解析器 (508 行)
│   ├── sender.js        # 发送队列 (110 行)
│   ├── events.js        # MessageEvent/CloseEvent/ErrorEvent (332 行)
│   ├── constants.js     # WebSocket 常量 (127 行)
│   ├── permessage-deflate.js  # 压缩扩展 (100 行)
│   └── stream/          # WebSocketStream API
│       ├── websocketstream.js   # WebSocketStream (498 行)
│       └── websocketerror.js    # WebSocketError (104 行)
├── eventsource/         # EventSource SSE (~580 行)
│   ├── eventsource.js   # EventSource 主类 (493 行)
│   ├── eventsource-stream.js  # SSE 流解析器 (521 行)
│   └── util.js          # CORS 工具 (60 行)
├── cache/               # Cache API (~1,060 行)
│   ├── cache.js         # Cache 类 (862 行)
│   ├── cachestorage.js  # CacheStorage (152 行)
│   └── util.js          # URL 比较 + Vary 处理 (45 行)
├── cookies/             # Cookie 处理 (~420 行)
│   ├── index.js         # getCookies/setCookie/deleteCookie (199 行)
│   ├── parse.js         # Set-Cookie 解析器 (317 行)
│   ├── util.js          # 验证 + 序列化 (353 行)
│   └── constants.js     # Cookie 大小限制 (12 行)
├── webidl/              # WebIDL 类型系统 (~1,000 行)
│   └── index.js         # 转换器 + brand check (1,004 行)
├── infra/               # WHATWG Infra 规范 (~230 行)
│   └── index.js         # 序列收集 + Base64 + 同构编解码
└── encoding/            # WHATWG Encoding 规范 (~34 行)
    └── index.js         # UTF-8 解码
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
| WebSocket | 10 | ~2,800 | `WebSocket`, `ByteParser`, `WebsocketFrameSend`, `SendQueue`, `PerMessageDeflate`, `WebSocketStream` |
| EventSource | 3 | ~580 | `EventSource`, `EventSourceStream` |
| Cache | 3 | ~260 | `Cache`, `CacheStorage` |
| Cookies | 4 | ~420 | `getCookies()`, `setCookie()`, `parseSetCookie()`, `stringify()` |
| WebIDL | 1 | ~1,004 | `brandCheck()`, `ConvertToInt()`, `dictionaryConverter()`, `sequenceConverter()` |
| Infra/Encoding | 2 | ~264 | `collectASequenceOfCodePoints()`, `forgivingBase64()`, `utf8DecodeBytes()` |
| FormData | 1 | ~278 | `FormData`, `makeEntry()` |
| Data URL | 1 | ~596 | `dataURLProcessor()`, `parseMIMEType()` |
| **总计** | **36** | **~14,060** | |


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


---

## 3. WebSocket 协议实现深度分析

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

## 4. EventSource / SSE 流式解析深度分析

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

## 5. Cache API 深度分析

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

## 6. Cookies 模块深度分析

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

