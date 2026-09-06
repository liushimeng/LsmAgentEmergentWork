# undici HTTP 传输层深度分析

> **仓库定位**：undici 是 Node.js 官方 HTTP 客户端库（**纯 HTTP 传输层，不实现 LLM 协议**），
> 本次分析聚焦 `lib/` 目录，按"请求构造 / Header 透传 / 流式响应 / 错误体系 / URL 解析"5 个有效维度 + 3 个"不适用"标记维度展开。
>
> 范围：`lib/dispatcher/`、`lib/core/`、`lib/web/`、`lib/interceptor/`、`lib/api/`。

---

## 1. 请求构造（Request Assembly）

### 1.1 入口链路：从 `dispatch` 到 `Request`

文件：`/usr/local/LsmGitOpenSource/undici/lib/dispatcher/client.js`

```js
// lib/dispatcher/client.js:405-424
[kDispatch] (opts, handler) {
  const request = new Request(this[kUrl].origin, opts, handler)

  this[kQueue].push(request)
  if (this[kResuming]) {
    // Do nothing.
  } else if (util.bodyLength(request.body) == null && util.isIterable(request.body)) {
    // Wait a tick in case stream/iterator is ended in the same tick.
    this[kResuming] = 1
    queueMicrotask(() => resume(this))
  } else {
    this[kResume](true)
  }

  if (this[kResuming] && this[kNeedDrain] !== 2 && this[kBusy]) {
    this[kNeedDrain] = 2
  }

  return this[kNeedDrain] < 2
}
```

> **说明**：`Client[kDispatch]` 是请求入场的唯一入口，把 `opts`（调用方传入的 path/method/body/headers）和 `handler`（回调）包装成 `Request` 对象后推入 `kQueue` 队列。

### 1.2 `Request` 构造函数 — 字段校验与 headers 数组化

文件：`/usr/local/LsmGitOpenSource/undici/lib/core/request.js`

```js
// lib/core/request.js:97-280 (节选核心字段)
class Request {
  constructor (origin, {
    path, method, body, headers, query, idempotent, blocking, upgrade,
    headersTimeout, bodyTimeout, reset, expectContinue, servername,
    throwOnError, maxRedirections, typeOfService
  }, handler) {
    // 校验 path 必须以 '/' 或 http:// | https:// 开头（CONNECT 例外）
    if (typeof path !== 'string') {
      throw new InvalidArgumentError('path must be a string')
    } else if (
      path[0] !== '/' &&
      !(path.startsWith('http://') || path.startsWith('https://')) &&
      method !== 'CONNECT'
    ) {
      throw new InvalidArgumentError('path must be an absolute URL or start with a slash')
    }

    this.method = method
    this.path = query ? serializePathWithQuery(path, query) : path
    this.origin = origin
    this.protocol = getProtocolFromUrlString(origin)

    this.idempotent = idempotent == null
      ? method === 'HEAD' || method === 'GET' || method === 'QUERY'
      : idempotent

    this.headers = []    // 偶数 index=key, 奇数 index=value

    if (Array.isArray(headers)) {
      for (let i = 0; i < headers.length; i += 2) {
        processHeader(this, headers[i], headers[i + 1])
      }
    } else if (headers && typeof headers === 'object') {
      const keys = Object.keys(headers)
      for (let i = 0; i < keys.length; ++i) {
        processHeader(this, keys[i], headers[keys[i]])
      }
    }
    this[kHandler] = handler
  }
}
```

> **说明**：`Request` 是协议无关的请求描述体。headers 被统一存入 `this.headers` 扁平数组（`[k0,v0,k1,v1,...]`），由 `processHeader` 逐条校验与归类（详见维度 2）。

### 1.3 H1 写入路径：`writeH1` 组装 HTTP wire 帧

文件：`/usr/local/LsmGitOpenSource/undici/lib/dispatcher/client-h1.js`

```js
// lib/dispatcher/client-h1.js:1178-1368 (节选 wire 帧拼接)
function writeH1 (client, request) {
  const { method, path, host, upgrade, blocking, reset } = request
  let { body, headers, contentLength } = request

  const expectsPayload = (method === 'PUT' || method === 'POST' || method === 'PATCH' || ...)

  // formdata/blob 补充 content-type
  if (util.isFormDataLike(body)) {
    const [bodyStream, contentType] = extractBody(body)
    if (request.contentType == null) headers.push('content-type', contentType)
    body = bodyStream.stream; contentLength = bodyStream.length
  }

  ...
  let header = `${method} ${path} HTTP/1.1\r\n`          // 请求行
  if (typeof host === 'string') {
    header += `host: ${host}\r\n`
  } else {
    header += client[kHostHeader]                       // 默认 "host: authority\r\n"
  }

  if (upgrade) {
    header += `connection: upgrade\r\nupgrade: ${upgrade}\r\n`
  } else if (client[kPipelining] && !socket[kReset]) {
    header += 'connection: keep-alive\r\n'
  } else {
    header += 'connection: close\r\n'
  }

  if (Array.isArray(headers)) {
    for (let n = 0; n < headers.length; n += 2) {
      const key = headers[n + 0]; const val = headers[n + 1]
      if (Array.isArray(val)) {
        for (let i = 0; i < val.length; i++) header += `${key}: ${val[i]}\r\n`
      } else {
        header += `${key}: ${val}\r\n`
      }
    }
  }

  // 分 body 类型派发写入器
  if (!body || bodyLength === 0)       writeBuffer(...)
  else if (util.isBuffer(body))        writeBuffer(...)
  else if (util.isStream(body))        writeStream(...)
  else if (util.isIterable(body))      writeIterable(...)
  return true
}
```

> **说明**：`writeH1` 负责把 `Request` 序列化成 HTTP/1.1 wire 格式（请求行 + host/connection + 调用方 headers + 空行 + body）。**调用方 headers 被原样拼入**——undici 不根据协议注入任何 LLM 认证头。

### 1.4 队列调度：`resume` → `_resume` 写入 socket

```js
// lib/dispatcher/client.js:660-739
function _resume (client, sync) {
  while (true) {
    ...
    if (!client[kHTTPContext]) { connect(client); return }
    if (client[kHTTPContext].busy(request)) return
    if (!request.aborted && client[kHTTPContext].write(request)) {
      client[kPendingIdx]++     // 推进 pending 指针
    } else {
      client[kQueue].splice(client[kPendingIdx], 1)
    }
  }
}
```

> **说明**：`_resume` 是核心调度环，维护 `kRunningIdx/kPendingIdx/kQueue` 三段队列；`kHTTPContext.write` 即 `writeH1`（h1）或 h2 写入器，二者由 `connectH1/connectH2` 挂载到 socket 上。

---

## 2. 认证头 / Header 透传（Header Pass-through）

### 2.1 `processHeader` 校验规则 — 不做协议认证，仅语法校验

文件：`/usr/local/LsmGitOpenSource/undici/lib/core/request.js`

```js
// lib/core/request.js:446-544
function processHeader (request, key, val) {
  if (val && (typeof val === 'object' && !Array.isArray(val))) {
    throw new InvalidArgumentError(`invalid ${key} header`)
  } else if (val === undefined) return

  let headerName = headerNameLowerCasedRecord[key]
  if (headerName === undefined) {
    headerName = key.toLowerCase()
    if (!isValidHTTPToken(headerName)) throw new InvalidArgumentError('invalid header key')
  }

  if (Array.isArray(val)) {             // 多值合并成数组
    const arr = []
    for (let i = 0; i < val.length; i++) {
      if (typeof val[i] === 'string') {
        if (!isValidHeaderValue(val[i])) throw new InvalidArgumentError(`invalid ${key} header`)
        arr.push(val[i])
      } else if (val[i] === null) arr.push('')
      else { val = `${val[i]}`; ... }
    }
    val = arr
  } else if (typeof val === 'string') {
    if (!isValidHeaderValue(val)) throw new InvalidArgumentError(`invalid ${key} header`)
  }

  if (headerName === 'host') { request.host = val }            // host 单独存
  else if (headerName === 'content-length') {
    if (request.contentLength !== null) throw new InvalidArgumentError('duplicate content-length header')
    request.contentLength = parseInt(val, 10)
  } else if (request.contentType === null && headerName === 'content-type') {
    request.contentType = val
    request.headers.push(key, val)
  } else if (headerName === 'transfer-encoding' || headerName === 'keep-alive' || headerName === 'upgrade') {
    throw new InvalidArgumentError(`invalid ${headerName} header`)
  } else if (headerName === 'connection') {                    // 解析 close 标记
    for (const token of value.toLowerCase().split(',')) {
      if (trimmed === 'close') request.reset = true
    }
  } else if (headerName === 'expect') {
    throw new NotSupportedError('expect header not supported')
  } else {
    request.headers.push(key, val)                             // ← 普通 header 原样入队
  }
}
```

> **说明**：**`authorization`、`x-api-key`、`anthropic-version` 等任何协议认证头都走最后一支 `request.headers.push(key, val)` 原样透传**——undici 完全不知道也不构造任何 LLM 协议头。唯一做特殊处理的是 `host`、`content-length`、`content-type`、`connection` 这几个 HTTP 语义头。

### 2.2 Fetch 层 `HeadersList.append` — 同形合并

文件：`/usr/local/LsmGitOpenSource/undici/lib/web/fetch/headers.js`

```js
// lib/web/fetch/headers.js:236-258
append (name, value, isLowerCase) {
  this.sortedMap = null
  const lowercaseName = isLowerCase ? name : name.toLowerCase()
  const exists = this.headersMap.get(lowercaseName)

  if (exists) {
    const delimiter = lowercaseName === 'cookie' ? '; ' : ', '
    this.headersMap.set(lowercaseName, {
      name: exists.name,
      value: `${exists.value}${delimiter}${value}`
    })
  } else {
    this.headersMap.set(lowercaseName, { name, value })
  }

  if (lowercaseName === 'set-cookie') (this.cookies ??= []).push(value)
}
```

> **说明**：Web Fetch 层使用 `HeadersList` 内部 map，普通头多值以 `, ` 合并（`cookie` 用 `; `）。同样**没有**任何协议特有的认证头注入逻辑。

### 2.3 结论：对 laew 的映射

- undici 是纯 HTTP 传输层：调用者给什么 header 就发什么 header，不感知 LLM 协议语义。
- laew 的双协议（Anthropic / OpenAI）header 差异（`x-api-key` / `Authorization: Bearer` / `anthropic-version` / `metadata.user_id`）**完全由 laew 在调用 `client.dispatch` 之前构造好再交给 undici**，undici 不会代劳。

---

## 3. 流式响应解析（Readable / AsyncIterator）

### 3.1 顶层 API：`api-request.js` 把 handler 桥接成 Node Readable

文件：`/usr/local/LsmGitOpenSource/undici/lib/api/api-request.js`

```js
// lib/api/api-request.js:96-166
onResponseStart (controller, statusCode, headers, statusText) {
  ...
  const res = new Readable({
    resume: () => controller.resume(),
    abort: (reason) => controller.abort(reason),
    contentType,
    contentLength: this.method !== 'HEAD' && contentLength ? Number(contentLength) : null,
    highWaterMark
  })
  this.callback = null
  this.res = res
  if (callback !== null) {
    this.runInAsyncScope(callback, null, null, {
      statusCode, statusText, headers: responseHeaderData,
      trailers: this.trailers, opaque, body: res, context
    })
  }
}

onResponseData (controller, chunk) {
  if (!this.res) return
  if (this.res.push(chunk) === false) controller.pause()   // 背压
}
```

> **说明**：对外的 `undici.request()` 把 `RequestHandler` 回调式接口桥接成 Node `Readable` 流；`onResponseData` 把 chunk push 入 Readable，背压通过 `controller.pause()` 反馈到 socket 解析器。

### 3.2 Fetch 层 `Response.body` — Web ReadableStream

文件：`/usr/local/LsmGitOpenSource/undici/lib/web/fetch/response.js`

```js
// lib/web/fetch/response.js:215-225
get body () {
  webidl.brandCheck(this, Response)
  return this.#state.body ? this.#state.body.stream : null     // ReadableStream
}

get bodyUsed () {
  webidl.brandCheck(this, Response)
  return !!this.#state.body && util.isDisturbed(this.#state.body.stream)
}
```

> **说明**：`Response.body` 暴露 Web `ReadableStream`，来自 `extractBody` 时用 `stream/source/length` 三元组构造。

### 3.3 `extractBody` + `consumeBody` — 消费 Web Stream

文件：`/usr/local/LsmGitOpenSource/undici/lib/web/fetch/body.js`

```js
// lib/web/fetch/body.js:40-244 (节选)
function extractBody (object, keepalive = false) {
  let stream = null; let controller = null
  if (webidl.is.ReadableStream(object)) {
    stream = object
  } else if (webidl.is.Blob(object)) {
    stream = object.stream()
  } else {
    stream = new ReadableStream({
      pull () {}, start (c) { controller = c }, cancel () {}, type: 'bytes'
    })
  }
  let action = null, source = null, length = null, type = null
  // ... 按 string / URLSearchParams / BufferSource / FormData / Blob / asyncIterator 分支
  if (typeof source === 'string' || isUint8Array(source)) {
    action = () => { length = ...; return source }
  }
  if (action != null) {
    ;(async () => {
      const result = action()
      const iterator = result?.[Symbol.asyncIterator]?.()
      if (iterator) {
        for await (const bytes of iterator) {
          if (isErrored(stream)) break
          if (bytes.length) controller.enqueue(new Uint8Array(bytes))
        }
      } else if (result?.length && !isErrored(stream)) {
        controller.enqueue(typeof result === 'string' ? textEncoder.encode(result) : new Uint8Array(result))
      }
      queueMicrotask(() => readableStreamClose(controller))
    })()
  }
  const body = { stream, source, length }
  return [body, type]
}

// lib/web/fetch/body.js:456-502
function consumeBody (object, convertBytesToJSValue, instance, getInternalState) {
  object = getInternalState(object)
  if (bodyUnusable(object)) return Promise.reject(new TypeError('Body is unusable: Body has already been read'))
  const promise = Promise.withResolvers()
  if (object.body == null) { successSteps(Buffer.allocUnsafe(0)); return promise.promise }
  fullyReadBody(object.body, successSteps, errorSteps)
  return promise.promise
}
```

> **说明**：`extractBody` 把所有 Web BodyInit 形态归一化为 `{stream: ReadableStream, source, length}`；`consumeBody` 是 `Response.text()/.json()/.blob()/.arrayBuffer()/.formData()` 的统一消费入口，最终调用 `fullyReadBody`。

### 3.4 `fullyReadBody` + `readAllBytes` — 底层 reader 消费

文件：`/usr/local/LsmGitOpenSource/undici/lib/web/fetch/util.js`

```js
// lib/web/fetch/util.js:940-1019
function fullyReadBody (body, processBody, processBodyError) {
  try {
    const reader = body.stream.getReader()
    readAllBytes(reader, successSteps, errorSteps)
  } catch (e) {
    errorSteps(e)
  }
}

async function readAllBytes (reader, successSteps, failureSteps) {
  try {
    const bytes = []; let byteLength = 0
    do {
      const { done, value: chunk } = await reader.read()
      if (done) { successSteps(Buffer.concat(bytes, byteLength)); return }
      if (!isUint8Array(chunk)) { failureSteps(new TypeError('Received non-Uint8Array chunk')); return }
      bytes.push(chunk); byteLength += chunk.length
    } while (true)
  } catch (e) { failureSteps(e) }
}
```

> **说明**：`readAllBytes` 是最底层 reader 循环，把 Web ReadableStream 收敛成 `Buffer`。对 LLM SSE 长响应而言，**laew 不会走 `consumeBody`（它要的是增量 chunk）**——应使用 `for await (const chunk of res.body)` 或 `Readable` API 的 `onResponseData` 增量回调。

### 3.5 结论：对 laew 的映射

| 场景 | 推荐消费路径 |
|------|-------------|
| `undici.request()` 回调式 | `body` 是 Node `Readable`，`body.on('data', ...)` 即可拿到增量 chunk（SSE 解析的最佳入口）|
| Fetch API `Response` | `Response.body` 是 Web `ReadableStream`，用 `getReader()` 或 `for await` 增量消费 |
| 一次性拿完整 JSON | `Response.json()` / `Response.text()` 内部走 `consumeBody`（不适合流式）|

---

## 4. 错误码映射与 26 种错误类

文件：`/usr/local/LsmGitOpenSource/undici/lib/core/errors.js`

### 4.1 错误类继承层次

```
Error
└── UndiciError                      UND_ERR                         (base, 用 Symbol.for('undici.error.UND_ERR') 标识)
    ├── ConnectTimeoutError           UND_ERR_CONNECT_TIMEOUT
    ├── HeadersTimeoutError           UND_ERR_HEADERS_TIMEOUT
    ├── HeadersOverflowError          UND_ERR_HEADERS_OVERFLOW
    ├── BodyTimeoutError              UND_ERR_BODY_TIMEOUT
    ├── InvalidArgumentError          UND_ERR_INVALID_ARG
    ├── InvalidReturnValueError       UND_ERR_INVALID_RETURN_VALUE
    ├── AbortError                    UND_ERR_ABORT
    │   └── RequestAbortedError       UND_ERR_ABORTED
    ├── InformationalError            UND_ERR_INFO
    ├── RequestContentLengthMismatchError  UND_ERR_REQ_CONTENT_LENGTH_MISMATCH
    ├── ResponseContentLengthMismatchError UND_ERR_RES_CONTENT_LENGTH_MISMATCH
    ├── ClientDestroyedError          UND_ERR_DESTROYED
    ├── ClientClosedError             UND_ERR_CLOSED
    ├── SocketError                   UND_ERR_SOCKET
    ├── NotSupportedError             UND_ERR_NOT_SUPPORTED
    ├── BalancedPoolMissingUpstreamError  UND_ERR_BPL_MISSING_UPSTREAM
    ├── ResponseExceededMaxSizeError  UND_ERR_RES_EXCEEDED_MAX_SIZE
    ├── RequestRetryError             UND_ERR_REQ_RETRY     (带 statusCode/headers/data)
    ├── ResponseError                 UND_ERR_RESPONSE      (带 statusCode/headers/body)
    ├── SecureProxyConnectionError    UND_ERR_PRX_TLS       (带 cause)
    ├── ProxyConnectionError          UND_ERR_PRX_CONN      (带 cause)
    ├── MaxOriginsReachedError        UND_ERR_MAX_ORIGINS_REACHED
    ├── Socks5ProxyError              UND_ERR_SOCKS5
    └── MessageSizeExceededError      UND_ERR_WS_MESSAGE_SIZE_EXCEEDED

Error
└── HTTPParserError                   HPE_<llhttp code>              (独立分支, 带 data)
```

### 4.2 典型定义 — 双重判别式（继承 + Symbol tag）

```js
// lib/core/errors.js:4-18 (base)
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

// lib/core/errors.js:128-144 (AbortError 示例)
const kAbortError = Symbol.for('undici.error.UND_ERR_ABORT')
class AbortError extends UndiciError {
  constructor (message) {
    super(message)
    this.name = 'AbortError'
    this.message = message || 'The operation was aborted'
    this.code = 'UND_ERR_ABORT'
  }
  get [kAbortError] () { return true }
}

// lib/core/errors.js:309-325 (HTTPParserError — 独立继承 Error, 非 UndiciError)
class HTTPParserError extends Error {
  constructor (message, code, data) {
    super(message)
    this.name = 'HTTPParserError'
    this.code = code ? `HPE_${code}` : undefined
    this.data = data ? data.toString() : undefined
  }
}
```

> **说明**：每个错误类都用 `Symbol.for('undici.error.UND_ERR_*)` 暴露一个 getter，既支持 `instanceof` 又支持跨 realm 的 `Symbol.hasInstance` 判定；`HTTPParserError` 独立继承 `Error`（因为它是 llhttp 解析器错误，不是 undici 业务错误）。

### 4.3 错误类的`module.exports`清单（共 25 个）

```js
// lib/core/errors.js:470-497
module.exports = {
  AbortError, HTTPParserError, UndiciError, HeadersTimeoutError, HeadersOverflowError,
  BodyTimeoutError, RequestContentLengthMismatchError, ConnectTimeoutError,
  InvalidArgumentError, InvalidReturnValueError, RequestAbortedError,
  ClientDestroyedError, ClientClosedError, InformationalError, SocketError,
  NotSupportedError, ResponseContentLengthMismatchError, BalancedPoolMissingUpstreamError,
  ResponseExceededMaxSizeError, RequestRetryError, ResponseError,
  SecureProxyConnectionError, ProxyConnectionError, MaxOriginsReachedError,
  Socks5ProxyError, MessageSizeExceededError
}
```

### 4.4 关键错误码速查

| 错误码 | 触发场景 |
|--------|----------|
| `UND_ERR_CONNECT_TIMEOUT` | TCP/TLS 建连超时 |
| `UND_ERR_HEADERS_TIMEOUT` | 等待响应头超时 |
| `UND_ERR_BODY_TIMEOUT` | 等待 body chunk 超时 |
| `UND_ERR_SOCKET` | socket 底层错误（`socketError`）|
| `UND_ERR_ABORT` | 用户 abort |
| `UND_ERR_REQ_RETRY` | 可重试响应（如 429/5xx，由 retry 拦截器抛出，带 statusCode）|
| `UND_ERR_RES_EXCEEDED_MAX_SIZE` | 响应体超过 `maxResponseSize` |
| `UND_ERR_DESTROYED / CLOSED` | Client 已销毁/关闭 |
| `HPE_*` | llhttp 解析失败（如 `HPE_INVALID_STATUS`）|

> **说明**：laew 的 `agent/mod.rs` 在收到这些错误时应映射到自家 `AgentError`——尤其是 `UND_ERR_SOCKET` / `UND_ERR_CONNECT_TIMEOUT` / `UND_ERR_BODY_TIMEOUT` 对应网络层可重试；`UND_ERR_ABORTED` 是用户主动取消；`UND_ERR_*_TIMEOUT` 需要区分"headers 阶段"和"body 阶段"（SSE 长响应的 bodyTimeout 应设 0 = 禁用）。

---

## 5. Tool Wire 格式

> **标记：不适用（HTTP 传输层）**。
>
> undici 是 HTTP 客户端库，不涉及 LLM 工具调用的 wire 格式（tool_use / tool_result 等概念属于 Anthropic/OpenAI 应用层协议）。
> 但是 undici 自身的"协议格式"是 HTTP/1.1 和 HTTP/2 wire 帧：
> - H1：文本请求行 + `\r\n` 分隔的 headers + `\r\n\r\n` + body，chunked 用 `${len.toString(16)}\r\n` 分块
> - H2：二进制帧（HEADERS + DATA），由 Node.js `http2` 模块实现，`client-h2.js` 仅做 stream 调度

---

## 6. Thinking / Reasoning

> **标记：不适用（HTTP 传输层）**。
>
> `thinking` / `reasoning` 是 Claude 等 LLM 在响应中内嵌的推理块，由应用层 SDK（`@anthropic-ai/sdk`）解析。
> undici 只负责把 SSE chunk（`data: {...}\n\n`）以字节形式推给调用者，**不解析 JSON、不识别 `content_block` 类型**。

---

## 7. Usage / Token 统计

> **标记：不适用（HTTP 传输层）**。
>
> Token 统计是 LLM 应用层功能（解析 `usage.input_tokens/output_tokens`）。
> undici 没有 token 概念，也没有响应体 JSON schema 感知。调用者需要自己：
> 1. 通过 `onResponseData` 或 `Readable` 拿到每个 SSE chunk 的原始字节
> 2. 自己做 `data: ` 行解析 + `JSON.parse`
> 3. 自己累加 `usage` 字段

---

## 8. URL 解析（URL / URLSearchParams）

### 8.1 `parseURL` — 严格 http/https 校验

文件：`/usr/local/LsmGitOpenSource/undici/lib/core/util.js`

```js
// lib/core/util.js:161-233
function parseURL (url) {
  if (typeof url === 'string') {
    url = new URL(url)
    if (!isHttpOrHttpsPrefixed(url.origin || url.protocol)) {
      throw new InvalidArgumentError('Invalid URL protocol: the URL must start with `http:` or `https:`.')
    }
    return url
  }

  if (!url || typeof url !== 'object') {
    throw new InvalidArgumentError('Invalid URL: The URL argument must be a non-null object.')
  }

  if (!(url instanceof URL)) {
    // 对 Record 形态逐字段校验
    if (url.port != null && url.port !== '' && isValidPort(url.port) === false) {
      throw new InvalidArgumentError('Invalid URL: port must be a valid integer ...')
    }
    if (url.path != null && typeof url.path !== 'string') { throw ... }
    if (url.pathname != null && typeof url.pathname !== 'string') { throw ... }
    if (url.hostname != null && typeof url.hostname !== 'string') { throw ... }
    if (url.origin != null && typeof url.origin !== 'string') { throw ... }
    if (!isHttpOrHttpsPrefixed(url.origin || url.protocol)) {
      throw new InvalidArgumentError('Invalid URL protocol: the URL must start with `http:` or `https:`.')
    }
    const port = url.port != null ? url.port : (url.protocol === 'https:' ? 443 : 80)
    let origin = url.origin != null
      ? url.origin
      : `${url.protocol || ''}//${url.hostname || ''}:${port}`
    let path = url.path != null ? url.path : `${url.pathname || ''}${url.search || ''}`
    if (origin[origin.length - 1] === '/') origin = origin.slice(0, origin.length - 1)
    if (path && path[0] !== '/') path = `/${path}`
    return new URL(`${origin}${path}`)
  }

  // URL 实例直接返回
  if (!isHttpOrHttpsPrefixed(url.origin || url.protocol)) {
    throw new InvalidArgumentError('Invalid URL protocol: the URL must start with `http:` or `https:`.')
  }
  return url
}
```

### 8.2 `parseOrigin` — 只取 origin（无 path）

```js
// lib/core/util.js:239-247
function parseOrigin (url) {
  url = parseURL(url)
  if (url.pathname !== '/' || url.search || url.hash) {
    throw new InvalidArgumentError('invalid url')
  }
  return url
}
```

> **说明**：`Client` 构造函数调用 `this[kUrl] = util.parseOrigin(url)` 把 origin 存成 URL 实例；`dispatch(opts)` 时再把 `opts.path` + `opts.query` 拼到 origin 上发出。

### 8.3 `getProtocolFromUrlString` — 协议快速判断 + 单值缓存

```js
// lib/core/util.js:934-968
function getProtocolFromUrlString (urlString) {
  if (urlString === lastUrlString) return lastProtocol
  const protocol = getProtocolFromUrlStringSlow(urlString)
  lastUrlString = urlString
  lastProtocol = protocol
  return protocol
}

function getProtocolFromUrlStringSlow (urlString) {
  if (urlString[0] === 'h' && urlString[1] === 't' && urlString[2] === 't' && urlString[3] === 'p') {
    switch (urlString[4]) {
      case ':':  return 'http:'
      case 's':  if (urlString[5] === ':') return 'https:'
    }
  }
  return 'http:'   // 默认
}
```

> **说明**：因为请求总是打向同一个 origin，用 `lastUrlString/lastProtocol` 两个变量缓存最近一次解析结果，避免重复构造 `URL` 对象。

### 8.4 结论：对 laew 的映射

- `parseURL` 严格限制 `http/https` 协议——laew 在传给 undici 之前必须保证 `end_point` 已清洗成 `https://...` 形态。
- `parseOrigin` 强制 path 为空——laew 应把 base URL 和 path 分开传入（`Client` 构造传 origin，`dispatch` 时传 path），避免在 origin 里残留 path。

---

## 9. 综合：对 laew 的映射与借鉴要点

| 维度 | undici 做法 | laew 现状/应借鉴 |
|------|------------|-----------------|
| 请求构造 | `Request(origin, opts, handler)` + `processHeader` 校验 | laew 的 `llm/anthropic.rs` 和 `llm/openai.rs` 在 dispatch 前构造 header 时，可借鉴 `processHeader` 对 host/content-length/connection 的严格校验 |
| Header 透传 | 不感知协议语义，普通 header 原样 `push` | **与 laew 设计一致**——laew 负责构造 Anthropic/OpenAI 协议头（`x-api-key`、`Authorization: Bearer`、`anthropic-version`），undici 只做传输 |
| 流式响应 | 双通道：Readable 回调 + Web ReadableStream | laew 用 `undici.request()` 的 `body` (Readable) + `body.on('data', ...)` 拿 SSE chunk 是最佳路径 |
| 错误体系 | 25 个细粒度类 + `UND_ERR_*` code | laew 可把 `UND_ERR_*` 映射到自家 `AgentError::Network` / `Timeout` / `Retryable` 子类，便于重试决策 |
| URL 解析 | `parseOrigin` 严格限制 http/https + path 为空 | laew 在 `provider add` 写入 DB 前就应做一次 `parseOrigin` 校验，避免脏数据 |
| 拦截器 | `Dispatcher.compose(...interceptors)` 洋葱模型 | 未来 laew 想做"请求签名 / 限流 / 重试"可借鉴该洋葱模式，但 laew 自身不直接消费 undici，依赖上层 SDK |

### 9.1 对 laew 落地的具体建议

1. **header 构造前移**：laew 在 `llm/anthropic.rs` 里构造完 header 数组后，可直接借用 undici `processHeader` 思路（代码不可直接复制因为语言不同）做一次 `isValidHTTPToken/isValidHeaderValue` 校验，拦截非法 token。
2. **超时三件套**：laew 应暴露 `connectTimeout / headersTimeout / bodyTimeout`，对 SSE 长响应把 `bodyTimeout` 设 0（禁用），避免 mid-stream 被 `BodyTimeoutError` 切断。
3. **错误重试分级**：
   - 可重试：`UND_ERR_CONNECT_TIMEOUT` / `UND_ERR_SOCKET` / `UND_ERR_BODY_TIMEOUT` / `RequestRetryError(429/502/503)`
   - 不可重试：`UND_ERR_ABORTED` / `UND_ERR_INVALID_ARG` / `HTTPParserError`
4. **严格 URL 校验**：`provider add` 写入 SQLite 前就用类 `parseOrigin` 逻辑校验——必须是 `http://` 或 `https://` 开头、无 path 残留。

---

## 10. 关键文件路径速查（绝对路径）

| 文件 | 职责 |
|------|------|
| `/usr/local/LsmGitOpenSource/undici/lib/dispatcher/client.js` | `Client` 类，`kDispatch` 入口 + 队列调度 |
| `/usr/local/LsmGitOpenSource/undici/lib/dispatcher/client-h1.js` | H1 写入路径：`writeH1` / `writeBuffer` / `writeStream` / `writeIterable` / `AsyncWriter` |
| `/usr/local/LsmGitOpenSource/undici/lib/dispatcher/client-h2.js` | H2 写入路径（maxConcurrentStreams 多路复用）|
| `/usr/local/LsmGitOpenSource/undici/lib/dispatcher/dispatcher.js` | `Dispatcher` 基类 + `compose` 拦截器洋葱 |
| `/usr/local/LsmGitOpenSource/undici/lib/dispatcher/dispatcher-base.js` | `DispatcherBase`：`dispatch` / `close` / `destroy` |
| `/usr/local/LsmGitOpenSource/undici/lib/core/request.js` | `Request` 类 + `processHeader` header 校验 |
| `/usr/local/LsmGitOpenSource/undici/lib/core/errors.js` | 25 个错误类定义 |
| `/usr/local/LsmGitOpenSource/undici/lib/core/util.js` | `parseURL` / `parseOrigin` / `getProtocolFromUrlString` |
| `/usr/local/LsmGitOpenSource/undici/lib/web/fetch/response.js` | `Response` 类 + `makeNetworkError` / `filterResponse` |
| `/usr/local/LsmGitOpenSource/undici/lib/web/fetch/body.js` | `extractBody` / `consumeBody` / `bodyMixinMethods` |
| `/usr/local/LsmGitOpenSource/undici/lib/web/fetch/util.js` | `fullyReadBody` / `readAllBytes` |
| `/usr/local/LsmGitOpenSource/undici/lib/web/fetch/headers.js` | `Headers` / `HeadersList`（append/set/delete/get）|
| `/usr/local/LsmGitOpenSource/undici/lib/api/api-request.js` | 顶层 `request()` + `RequestHandler`（Readable 桥接）|
| `/usr/local/LsmGitOpenSource/undici/lib/interceptor/dump.js` | `dump` 拦截器（消费响应并丢弃）|
| `/usr/local/LsmGitOpenSource/undici/lib/interceptor/retry.js` | 重试拦截器（抛出 `RequestRetryError`）|

---

*文档生成时间：2026-09-06*
*分析范围：undici `lib/` 目录，聚焦 dispatcher / core / web / interceptor 四个子树*
*用途：laew 的 HTTP 传输层知识库参考，指导 Anthropic/OpenAI 双协议客户端接入*
