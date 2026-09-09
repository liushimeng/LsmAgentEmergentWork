# 第十八轮 undici 深度分析 — Agent 富内容获取的 HTTP 底座

> **轮次**：第十八轮（2026-09-09） · **主题**：Agent 富内容获取的 HTTP 底座 · **维度**：N1 响应体内容解码管线 / N2 内容类型协商与 MIME 处理 / N3 大响应的流式消费与背压 / N4 网页获取的场景化封装 / N5 对 laew WebFetch 工具的 Rust 映射
>
> **本轮不重复声明**：undici 已被深挖 15+ 轮，连接池/拦截器/HTTP2/GOAWAY/重试/代理链/TLS/Cookie/重定向/Mock 录制回放/SSE 状态机/WebSocket cork/AbortSignal/HTTP3 QUIC 等已全部覆盖。本轮聚焦 **Agent 把网页/文档取回并解析为 LLM 可用富文本时，HTTP 客户端层需要什么能力** —— 这是 laew WebFetch 工具的底层支撑维度。
>
> **新增 gap**：**L1576-L1590**（共 15 个新 gap，聚焦「响应体解码 / 字符集嗅探 / 大响应截断 / Web 场景化封装」等本轮独有能力）

---

## 目录

- [一、N1 响应体内容解码管线](#一n1-响应体内容解码管线)
- [二、N2 内容类型协商与 MIME 处理](#二n2-内容类型协商与-mime-处理)
- [三、N3 大响应的流式消费与背压](#三n3-大响应的流式消费与背压)
- [四、N4 网页获取的场景化封装](#四n4-网页获取的场景化封装)
- [五、N5 对 laew WebFetch 工具的 Rust 映射](#五n5-对-laew-webfetch-工具的-rust-映射)
- [六、gap 汇总](#六gap-汇总)

---

## 一、N1 响应体内容解码管线

### 1.1 源码定位

- **Accept-Encoding 自动注入**：`lib/web/fetch/index.js:1563-1576`
- **Content-Encoding 解码链**：`lib/web/fetch/index.js:2268-2318`
- **deflate Raw/Zlib 自动判定**：`lib/web/fetch/util.js:1243-1277`
- **UTF-8 BOM 剥离**：`lib/encoding/index.js:9-29`
- **Decompress 拦截器**：`lib/interceptor/decompress.js:1-160`

### 1.2 机制剖析

#### 1.2.1 Accept-Encoding 注入策略

undici 的 fetch 入口（`httpFetch`）在缺少 `Accept-Encoding` 头时按 scheme 注入不同声明：

```javascript
// lib/web/fetch/index.js:1569-1576
if (!httpRequest.headersList.contains('accept-encoding', true)) {
  if (urlHasHttpsScheme(requestCurrentURL(httpRequest))) {
    httpRequest.headersList.append('accept-encoding', 'br, gzip, deflate, zstd', true)
  } else {
    httpRequest.headersList.append('accept-encoding', 'gzip, deflate', true)
  }
}
```

**设计巧妙点**：HTTPS 才声明 `br + zstd`（部分代理如 CloudFront 旧版不会对 `br` 做正确 pass-through，明文 HTTP 走精简声明规避中间件兼容问题）。如果用户已声明 `Range`，强制覆盖为 `identity`（避免下载片段时服务端预压缩造成 Range 错位）。

#### 1.2.2 Content-Encoding 解码链（CVE 修复）

`onResponseStart` 阶段构造 `decoders[]` 数组（栈式 Transform 管道），按 **编码逆序** 压栈、`pipeline()` 串接：

```javascript
// lib/web/fetch/index.js:2268-2318
const decoders = []

if (request.method !== 'HEAD' && request.method !== 'CONNECT' &&
    !nullBodyStatus.includes(status) && !willFollow) {
  const contentEncoding = headersList.get('content-encoding', true)
  const codings = contentEncoding ? contentEncoding.toLowerCase().split(',') : []

  // Limit the number of content-encodings to prevent resource exhaustion.
  // CVE fix similar to urllib3 (GHSA-gm62-xv2j-4w53) and curl (CVE-2022-32206).
  const maxContentEncodings = 5
  if (codings.length > maxContentEncodings) {
    reject(new Error(`too many content-encodings in response: ${codings.length}, maximum allowed is ${maxContentEncodings}`))
    return
  }

  for (let i = codings.length - 1; i >= 0; --i) {
    const coding = codings[i].trim()
    if (coding === 'x-gzip' || coding === 'gzip') {
      decoders.push(zlib.createGunzip({
        flush: zlib.constants.Z_SYNC_FLUSH,
        finishFlush: zlib.constants.Z_SYNC_FLUSH
      }))
    } else if (coding === 'deflate') {
      decoders.push(createInflate({
        flush: zlib.constants.Z_SYNC_FLUSH,
        finishFlush: zlib.constants.Z_SYNC_FLUSH
      }))
    } else if (coding === 'br') {
      decoders.push(zlib.createBrotliDecompress({
        flush: zlib.constants.BROTLI_OPERATION_FLUSH,
        finishFlush: zlib.constants.BROTLI_OPERATION_FLUSH
      }))
    } else if (coding === 'zstd') {
      decoders.push(zlib.createZstdDecompress({
        flush: zlib.constants.ZSTD_e_continue,
        finishFlush: zlib.constants.ZSTD_e_end
      }))
    } else {
      decoders.length = 0   // 未知编码 → 跳过整条解码链
      break
    }
  }
}

resolve({
  // ...
  body: decoders.length
    ? pipeline(this.body, ...decoders, (err) => {
      if (err) this.onResponseError(controller, err)
    }).on('error', onError)
    : this.body.on('error', onError)
})
```

**设计巧妙点**：

1. **CVE 2022 风格攻击防护**：`maxContentEncodings = 5` 拒绝 `content-encoding: gzip, gzip, gzip, gzip, gzip, gzip, ...` 无限压栈 DoS 攻击（urllib3/curl 同款修复）。
2. **逐编码逆序压栈**（`i = codings.length - 1; i >= 0`）：HTTP 规范 `content-encoding: gzip, br` 表示先 gzip 后 br，客户端解码必须先 br 后 gzip —— 与逆向 `for` 循环自然契合，`pipeline()` 按数组顺序消费。
3. **未知编码熔断**：`decoders.length = 0; break` 跳过整条链让上层直接处理原始字节 —— 避免「猜错编码」导致 OOM 或乱码传递。
4. **always-Z_SYNC_FLUSH**（注释明示对齐 cURL）：容忍非标准 `gzip` 响应（缺尾 flush），让 LAEW 这类通用 LLM 客户端在抓取 web 时零配置。
5. **HEAD/CONNECT 跳过**：HEAD 不带 body，CONNECT 走隧道 —— 不需要解码链。

#### 1.2.3 deflate Raw/Zlib 自动嗅探

HTTP `deflate` 在历史上同时表示 zlib wrapper 与 raw deflate，undici 自研 `InflateStream` 在首个 chunk 到达时按首字节嗅探：

```javascript
// lib/web/fetch/util.js:1252-1268
_transform (chunk, encoding, callback) {
  if (!this._inflateStream) {
    if (chunk.length === 0) {
      callback()
      return
    }
    // If the lower byte of the first byte is 0x08, then the stream is
    // interpreted as a zlib stream, otherwise it's interpreted as a
    // raw deflate stream.
    this._inflateStream = (chunk[0] & 0x0F) === 0x08
      ? zlib.createInflate(this.#zlibOptions)
      : zlib.createInflateRaw(this.#zlibOptions)
    // ...
  }
}
```

**设计巧妙点**：CMF byte 低 4 位 `0x08` 是 zlib header 的强指示（CMF * FLG 末位组合恒为 `0x78`），首字节决策一次性、惰性建流 —— 兼得 zlib 与 raw deflate 兼容性而不需要预先知道服务端规范。

#### 1.2.4 BOM 剥离与 UTF-8 解码

```javascript
// lib/encoding/index.js:1-29
const textDecoder = new TextDecoder()  // 单例:fatal=false, 默认 UTF-8

function utf8DecodeBytes (buffer) {
  if (buffer.length === 0) return ''
  // 剥离 0xEF 0xBB 0xBF BOM
  if (buffer[0] === 0xEF && buffer[1] === 0xBB && buffer[2] === 0xBF) {
    buffer = buffer.subarray(3)
  }
  return textDecoder.decode(buffer)
}
```

`TextDecoder` 默认 `fatal: false` —— 无效 UTF-8 序列替换为 U+FFFD 而不是抛异常。这是 **Web API 强一致性**（与浏览器一致），但 **不可逆**（LLM 若看到 `���` 大量出现时需要靠 charset 嗅探补救）。

#### 1.2.5 Decompress 拦截器（API 模式）

undici 还提供 `decompress` 拦截器（`createDecompressInterceptor`）给 `request()` API：

```javascript
// lib/interceptor/decompress.js:73-95
#shouldSkipDecompression (contentEncoding, statusCode) {
  if (!contentEncoding || statusCode < 200) return true
  if (this.#skipStatusCodes.includes(statusCode)) return true
  if (this.#skipErrorResponses && statusCode >= 400) return true
  return false
}
```

**默认跳过 204/304**（这两个状态本来就没有 body），**默认跳过 4xx/5xx**（错误页即使压缩也按原文抛给调用方，便于调试）—— 与 fetch 路径默认行为一致，但提供 `skipErrorResponses=false` 选项供调试压缩流。

### 1.3 laew gap 表

| 编号 | gap 描述 | 推荐 Rust crate / 方案 |
|------|---------|------------------------|
| **L1576** | 无 `content-encoding: br/zstd` 解码（reqwest 默认 feature 包含 `gzip/deflate/brotli`，但 zstd 不含） | reqwest 启用 `zstd` feature（Rust 1.81+ 走 `zstd-rs`），或在 WebFetch 工具侧补 `zstd` 二次解压 |
| **L1577** | 无 `Content-Encoding` 链级 DoS 防护（无限 `gzip, gzip, ...`） | 自研 `decode_chain()`：超过 N 层（建议 5）直接拒绝；与 undici `maxContentEncodings` 对齐 |
| **L1578** | 无 deflate Raw/Zlib 自动嗅探 | `async-compression` 的 `DeflateDecoder` 配置 `raw: false`（默认 zlib wrapper）+ 检测首字节 CMF 0x78 切换 `raw: true` |
| **L1579** | 无 BOM 剥离（UTF-8 BOM 0xEF 0xBB 0xBF 会污染文本首字） | `encoding_rs::UTF_8.decode_with_bom_removal(bytes)` 或自写 3 字节 peek |
| **L1580** | 无「未知 encoding 熔断」机制（解压失败直接 500） | 解码器链返回 `InvalidData` → 降级直接使用原始字节（与 undici `decoders.length = 0` 语义一致）|

---

## 二、N2 内容类型协商与 MIME 处理

### 2.1 源码定位

- **Accept 头自动注入**：`lib/web/fetch/index.js:515-534`
- **MIME 解析**：`lib/web/fetch/data-url.js:100-200`（`parseMIMEType`）
- **MIME 提取**（含 charset 嗅探）：`lib/web/fetch/util.js:1289-1342`（`extractMimeType`）
- **JSON 嗅探**：`lib/web/fetch/data-url.js:556-571`
- **FormData multipart**：`lib/web/fetch/formdata-parser.js:1-50`

### 2.2 机制剖析

#### 2.2.1 Accept 头按 destination 注入

```javascript
// lib/web/fetch/index.js:515-534
if (!request.headersList.contains('accept', true)) {
  // 1. Let value be `*/*`.
  const value = '*/*'

  // 2. A user agent should set value to the first matching statement,
  //    switching on request's destination:
  //    "document"/"frame"/"iframe" → `text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8`
  //    "image"                      → `image/png,image/svg+xml,image/*;q=0.8,*/*;q=0.5`
  //    "style"                      → `text/css,*/*;q=0.1`
  // TODO

  // 3. Append `Accept`/value to request's header list.
  request.headersList.append('accept', value, true)
}
```

**当前实现**：`destination` 字段已经入参但分支仍 TODO，统一发 `*/*` —— undici 的策略选择是「不替用户做 Content Negotiation」。**对 Agent 场景的隐含后果**：WebFetch 抓 HTML 时拿不到 `text/html` 优先级权（拿到的是 `*/*`），多数服务器会按 `Content-Type: text/html` 返回 —— 影响不大；但抓 API 返回的 JSON 时如果服务端做了 `Accept` 过滤，可能拿到 XML 或 HTML fallback。

#### 2.2.2 MIME 解析与 charset 嗅探（fetch 规范 §2.7）

```javascript
// lib/web/fetch/util.js:1291-1342
function extractMimeType (headers) {
  let charset = null
  let essence = null
  let mimeType = null

  // 4. Let values be the result of getting, decoding, and splitting `Content-Type` from headers.
  const values = getDecodeSplit('content-type', headers)
  if (values === null) return 'failure'

  for (const value of values) {
    const temporaryMimeType = parseMIMEType(value)

    // 6.2. If temporaryMimeType is failure or its essence is "*/*", then continue.
    if (temporaryMimeType === 'failure' || temporaryMimeType.essence === '*/*') {
      continue
    }

    mimeType = temporaryMimeType

    // 6.4. If mimeType's essence is not essence, then:
    if (mimeType.essence !== essence) {
      charset = null  // 重置
      if (mimeType.parameters.has('charset')) {
        charset = mimeType.parameters.get('charset')
      }
      essence = mimeType.essence
    } else if (!mimeType.parameters.has('charset') && charset !== null) {
      // 6.5. 后继 Content-Type 同 essence 时,继承前值 charset
      mimeType.parameters.set('charset', charset)
    }
  }
  // ...
}
```

**设计巧妙点**：

1. **`getDecodeSplit('content-type', headers)`**：先按 RFC 7230 §3.2.2 解码 quoted-string 转义，再按 `,` 分多值 —— 同时支持 `text/html; charset="UTF-8"` 这种带引号形式。
2. **多 Content-Type 后值继承前值 charset**：如果服务端发 `text/html;charset=UTF-8, text/html`（奇葩但合法），第二个值会从第一个继承 charset —— 业界少见但规范要求。
3. **`*/*` 视为 failure**：多值中跳过 `*/*`，避免被全局 Accept 头污染（这是请求头而不是响应头，但解析路径共用同一函数）。

#### 2.2.3 解析路径：parseMIMEType

`data-url.js` 中的 `parseMIMEType`（RFC 7231 §3.1.1.1）状态机：

- 识别 `type/subtype` 主体 → essence
- 解析 `;` 后参数表（`key=value` / `quoted-string` / token）
- 处理 RFC 2045 注释 `()` 与 RFC 7230 OWS

输出结构：

```javascript
{ essence: 'text/html', parameters: Map { 'charset' => 'UTF-8' } }
```

**`JSON MIME type 嗅探`**：`isJSONMimeType(essence)` 检查 `essence === 'application/json'` 或 `+json` 后缀（RFC 8259 §6）。

### 2.3 laew gap 表

| 编号 | gap 描述 | 推荐 Rust crate / 方案 |
|------|---------|------------------------|
| **L1581** | 无 `Accept` 头按 destination 自动协商（HTML 抓取未声明 `text/html` 优先级） | WebFetch 工具在构造 request 时强制 `accept: text/html, application/json, */*;q=0.8`（按 LLM 期望内容类型区分） |
| **L1582** | 无 `Content-Type: text/html; charset=GBK` 等非 UTF-8 字符集解码 | `encoding_rs::Encoding::for_label_no_replacement(bytes)` 探测 → 二次 fallback `chardetng` / `charset` crate |
| **L1583** | 无 `<meta charset="...">` HTML 内嵌声明嗅探（页面缺 Content-Type 头时的最终回退） | WebFetch 解析 HTML 时正则提取 `<meta charset="([^"]+)">` / `<meta http-equiv="content-type">` 二次纠偏 |
| **L1584** | 无 `+json` MIME 后缀识别（RFC 8259 §6） | WebFetch 工具检测 `essence.ends_with("+json")` 走 `serde_json::from_slice` 而非默认 text |
| **L1585** | 无 Multipart FormData 解析（上传/抓取场景） | `reqwest::multipart` 上传 + `multer` 解析响应；本轮优先级低（WebFetch 单向下行为主）|

---

## 三、N3 大响应的流式消费与背压

### 3.1 源码定位

- **maxResponseSize 连接层强制**：`lib/dispatcher/client-h1.js:745-752`、`client-h2.js:1331-1345`
- **maxResponseSize 配置与校验**：`lib/dispatcher/client.js:163-321`
- **Response 阶段 fetchParams 截断**：`lib/web/fetch/index.js:2230-2245`
- **BodyReadable.dump 截断**：`lib/api/readable.js:265-320`
- **SSE 事件级 maxEventSize**：`lib/web/eventsource/eventsource-stream.js:151`
- **Fetch 流式 body 拉取**：`lib/web/fetch/index.js:2088-2140`

### 3.2 机制剖析

#### 3.2.1 maxResponseSize 协议层强制（H1/H2 双实现）

```javascript
// lib/dispatcher/client.js:163-214
if (maxHeaderSize != null) {
  if (!Number.isInteger(maxHeaderSize) || maxHeaderSize < 1) {
    throw new InvalidArgumentError('invalid maxHeaderSize')
  }
  maxHeaderSize = getDefaultNodeMaxHeaderSize()
}

// lib/dispatcher/client.js:197
if (bodyTimeout != null && (!Number.isInteger(bodyTimeout) || bodyTimeout < 0)) {
  throw new InvalidArgumentError('bodyTimeout must be a positive integer or zero')
}

// lib/dispatcher/client.js:213-214
if (maxResponseSize != null && (!Number.isInteger(maxResponseSize) || maxResponseSize < -1)) {
  throw new InvalidArgumentError('maxResponseSize must be a positive number')
}
```

**H1 路径**（`client-h1.js:745-752`）：

```javascript
if (maxResponseSize > -1 && this.bytesRead + buf.length > maxResponseSize) {
  util.destroy(socket, new ResponseExceededMaxSizeError())
  return -1
}
this.bytesRead += buf.length
```

**H2 路径**（`client-h2.js:1337-1343`）：

```javascript
if (maxResponseSize > -1 && state.bytesRead + chunk.length > maxResponseSize) {
  // Unlike HTTP/1.1, which destroys the socket because it cannot abandon one
  // response without losing framing, resetting the offending stream leaves
  // the session usable for its siblings.
  state.abort(new ResponseExceededMaxSizeError())
  return
}
state.bytesRead += chunk.length
```

**设计巧妙点**：

1. **`-1` 哨兵 = 无限制**：避免 0 与「未设置」语义混淆（0 是合法的「拒绝任何响应」，`undefined` 才是「未限制」）。
2. **H1 destroy socket vs H2 RST_STREAM**：HTTP/1.1 一旦开始接收响应，无法在帧内断开（无 multiplex），只能销毁整个 socket；但 H2 单个 stream RST_STREAM 不影响同连接其他 stream（多路复用优势）—— **协议层差异化的截断语义**。
3. **`bytesRead + buf.length > maxResponseSize`**：累加器模式而不是单 chunk 比较，避免「最后一个 chunk 跨越阈值」误判（单 chunk 可能达 64KB highWaterMark）。

#### 3.2.2 fetchParams.controller.dump 模式（fetch 路径）

```javascript
// lib/web/fetch/index.js:2336-2358
onResponseData (controller, chunk) {
  if (fetchParams.controller.dump) {
    return    // 全 dump 模式:不向 body 推任何数据
  }
  // ...
  timingInfo.encodedBodySize += bytes.byteLength
  if (this.body.push(bytes) === false) {
    controller.pause()    // 背压:暂停下层 socket
  }
}
```

**`fetchParams.controller.dump`** 是 fetch 层的流控开关 —— true 时**完全丢弃** body chunk，但 socket 仍在读取直到 EOF（让 server 不会因客户端不读而触发 TCP 窗口归零）。

**`controller.pause()`**：Node.js Readable 标准的背压原语 —— `push() === false` 表示下游队列已满，暂停上游数据推送，等 `readable` 事件触发时再 resume。

#### 3.2.3 BodyReadable.dump（request API）

`request()` API（区别于 `fetch()`）提供 dump 方法带 `limit` 选项：

```javascript
// lib/api/readable.js:265-320
const limit = opts?.limit && Number.isFinite(opts.limit)
  ? opts.limit
  : 128 * 1024   // 默认 128KB

if (signal?.aborted) {
  return Promise.reject(signal.reason ?? new AbortError())
}

if (this._readableState.closeEmitted) {
  return Promise.resolve(null)
}

return new Promise((resolve, reject) => {
  if (
    (this[kContentLength] && (this[kContentLength] > limit)) ||
    this[kBytesRead] > limit
  ) {
    this.destroy(new AbortError())
  }

  if (signal) {
    const onAbort = () => {
      this.destroy(signal.reason ?? new AbortError())
    }
    const abortListener = addAbortListener(signal, onAbort)
    this.on('close', function () {
      abortListener[Symbol.dispose]()
      if (signal.aborted) reject(signal.reason ?? new AbortError())
      else resolve(null)
    })
  } else {
    this.on('close', resolve)
  }

  this.on('error', noop)
    .on('data', () => {
      if (this[kBytesRead] > limit) {
        this.destroy()
      }
    })
    .resume()
})
```

**设计巧妙点**：

1. **双重预检**：先看 `Content-Length` 头，超过直接拒绝（避免下载大文件浪费带宽）；再累加 `kBytesRead`，覆盖「无 Content-Length 或 chunked」场景。
2. **AbortSignal + destroy**：通过 `addAbortListener` 把外部 signal 桥接到 stream destroy，**用户取消立即生效**（不等下一个 chunk）。
3. **默认 128KB**：对比 industry 经验值（claudecode 50KB 工具结果 / atomcode 30KB bash 输出），128KB 偏激进 —— undici 倾向 Web 文档抓取（大 HTML/JS bundle），WebFetch 工具侧应二次截断到 256KB-2MB。

#### 3.2.4 SSE 事件级 maxEventSize

```javascript
// lib/web/eventsource/eventsource-stream.js:151
this.maxEventSize = options.maxEventSize ?? defaultMaxEventSize  // kStringMaxLength (~512MB)
```

**事件级而非字节级**：SSE 单个 `data:` 字段可能跨多个 chunk 累积，事件维度（`eventDataSize` 累加器）截断更合理 —— 防止单事件占用过多内存拖垮整个流。

### 3.3 laew gap 表

| 编号 | gap 描述 | 推荐 Rust crate / 方案 |
|------|---------|------------------------|
| **L1586** | 无 `maxResponseSize` 连接层强制（无 Content-Length 时的累加器截断） | reqwest 不直接支持；中间件用 `tower::steer::Take` 限制 body 字节数，或在 `Body` 包装 `Limited::new(bytes, N)`（hyper 1.x `Body` trait `poll_frame` 内累加） |
| **L1587** | 无 fetchParams.controller.dump 等价机制（HEAD/探测请求的零拷贝放行） | 自实现 `webfetch` 工具时区分 `head_only: true`（探测 Content-Type/Length，不下载 body）与 `full: true`（按需上限下载） |
| **L1588** | 无 128KB 默认 dump 上限（llm 喂食的安全网缺失） | WebFetch 工具强制 `dump_limit = 256KB`（HTML 摘要场景）/ `4MB`（完整文档场景），超出则落盘 + 返回 `[TRUNCATED]` 标记 |
| **L1589** | 无 AbortSignal + bytes 累加双重预检（Content-Length 头存在时仍下载大文件） | WebFetch 入口处 `if content_length > limit { return Err("too large") }`，再走下载 + 累加器二次截断 |

---

## 四、N4 网页获取的场景化封装

### 4.1 源码定位

- **重定向策略**：`lib/web/fetch/index.js:1214-1350`（fetch 规范）/`lib/handler/redirect-handler.js:1-229`（request API）
- **Redirect 状态集合**：`lib/web/fetch/index.js:2265`（`redirectStatusSet`）
- **referer/origin 控制**：`lib/web/fetch/index.js:1499-1503`/`lib/web/fetch/request.js:639-657`
- **Cookie 会话保持**：`lib/web/fetch/index.js:2065-2076`（TODO 标注）
- **AbortSignal 集成**：`lib/web/fetch/index.js:180-188`
- **timeout 集成**：`lib/dispatcher/client.js:316-317`

### 4.2 机制剖析

#### 4.2.1 重定向策略（fetch 规范）

```javascript
// lib/web/fetch/index.js:1214-1242
if (redirectStatusSet.has(actualResponse.status)) {
  // 1. H2 场景:对带 body 的非 303 重定向,user agent may RST_STREAM
  if (request.redirect !== 'manual') {
    fetchParams.controller.connection.destroy(undefined, false)
  }

  // 2. Switch on request's redirect mode:
  if (request.redirect === 'error') {
    response = makeNetworkError('unexpected redirect')
  } else if (request.redirect === 'manual') {
    response = actualResponse  // 透传,不跟随
  } else if (request.redirect === 'follow') {
    response = await httpRedirectFetch(fetchParams, response)
  }
}
```

`httpRedirectFetch`（`index.js:1252-1360`）的 13 步算法：

```javascript
// lib/web/fetch/index.js:1287-1290
if (request.redirectCount === 20) {
  return Promise.resolve(makeNetworkError('redirect count exceeded'))
}
request.redirectCount += 1
```

**设计巧妙点**：

1. **重定向次数硬上限 20**：与 WHATWG fetch 规范一致，防止循环重定向（恶意服务端或配置错误）。
2. **POST→GET 自动降级**（`index.js:1330-1345`）：301/302 + POST 自动改 GET 并剥 `Content-*` 头（对齐 RFC 7231 §6.4.2）；303 + 非 GET/HEAD 也强制 GET —— **SPA 抓取注意**：登录后 302 跳到 dashboard 会丢 POST body，**对 WebFetch 应配置 `redirect: 'manual'` 让 Agent 显式决策**。
3. **跨域 CORS 删头**（`index.js:1347-1355`）：跨 origin 时按 CORS 规范删除非 wildcard 头 —— 防止 token 跨站泄漏。
4. **重定向 body 完全丢弃**（`redirect-handler.js:123-145`）：注释明示「undici always ignores 3xx response bodies」，因为按 RFC 7231 重定向 body 仅含人类可读 hint，机器客户端无需解析。

#### 4.2.2 referer 与 origin

```javascript
// lib/web/fetch/index.js:1499-1503
//    11. If httpRequest's referrer is a URL, then append
//    `Referer`/httpRequest's referrer, serialized and isomorphic encoded,
//    to httpRequest's header list.
if (webidl.is.URL(httpRequest.referrer)) {
  httpRequest.headersList.append('referer', isomorphicEncode(httpRequest.referrer.href), true)
}
```

`referrerPolicy` 在 `request.js:663` 暴露（`strict-origin-when-cross-origin` 等 8 种策略），由 `determineRequestsReferrer`（`index.js:600`）按策略计算最终 URL —— **WebFetch 默认应禁用 referer**（`referrerPolicy: 'no-referrer'`），避免把 agent 任务名/上下文泄漏给目标站。

#### 4.2.3 Cookie 会话保持（明确未实现）

```javascript
// lib/web/fetch/index.js:2071-2076
//    3. If includeCredentials is true and the user agent is not configured
//    to block cookies for request (see section 7 of [COOKIES]), then run the
//    "set-cookie-string" parsing algorithm (see section 5.2 of [COOKIES]) on
//    the value of each header whose name is a byte-case-insensitive match for
//    `Set-Cookie` in response's header list, if any, and request's current URL.
//    TODO
```

**注释 `TODO`**：fetch 路径**完全不持久化 Cookie**，每次 fetch 都是 fresh session。Cookie 持久化只在 `cookies/` 模块的独立 API（`parseSetCookie`）中实现 —— 用户必须自行维护 jar。

**对 Agent 场景的隐含后果**：WebFetch 抓需要登录的页面（公众号、Notion 公开页等）会失败 —— laew 需要自实现 cookie jar。

#### 4.2.4 AbortSignal + timeout 组合

```javascript
// lib/web/fetch/index.js:180-188
if (requestObject.signal.aborted) {
  abortFetch(p, request, null, requestObject.signal.reason, null)
  return p.promise
}
```

```javascript
// lib/dispatcher/client.js:316-317
this[kBodyTimeout] = bodyTimeout != null ? bodyTimeout : 300e3      // 300s
this[kHeadersTimeout] = headersTimeout != null ? headersTimeout : 300e3
```

**设计巧妙点**：

1. **signal 早检**：在 fetch 入口先看 `signal.aborted`（无 IO 调用），避免发起 socket 后才取消浪费握手 —— 与 React `useEffect cleanup` 模式契合。
2. **headers/body timeout 分离**：headersTimeout 防止「服务端接受连接但不响应头」（slowloris 攻击），bodyTimeout 防止「传输卡死」（chunks 之间间隔过大）。
3. **默认 300s 偏激进**：Agent WebFetch 场景建议 30s headers / 60s body —— web 内容大部分 3s 内返回。

### 4.3 laew gap 表

| 编号 | gap 描述 | 推荐 Rust crate / 方案 |
|------|---------|------------------------|
| **L1590** | 无 WebFetch 重定向策略显式控制（POST→GET 自动降级对 SPA 登录态丢失） | reqwest `redirect::Policy::limited(5)` + 自实现「POST + Location ≠ GET 不跟随」分支；或每次 WebFetch 默认 `redirect::Policy::none`，让 agent 决策 |
| **L1591**（本轮保留，挪至下一轮）| 无 Cookie jar 持久化（fetch TODO） | `reqwest::cookie::Jar` (内置) + `Arc<Jar>` 跨请求复用 |
| **L1592**（本轮保留，挪至下一轮）| 无 referer 抑制（默认 no-referrer 缺失） | reqwest `header::HeaderMap::insert("Referer", "")` 或显式禁用 |
| **L1593**（本轮保留，挪至下一轮）| 无 headers/body timeout 分离（slowloris 防护） | reqwest `.timeout(Duration::from_secs(30))` 单值；自实现需在中间件中按「已收到 first byte」分两阶段计时 |

> **说明**：L1591/L1592/L1593 三项超出本轮 L1576-L1590 区间上限（15 个），作为下轮 WebFetch 重构待办；本轮专注 N1-N5 主线 15 个 gap。

---

## 五、N5 对 laew WebFetch 工具的 Rust 映射

### 5.1 Rust crate 矩阵

| 能力维度 | undici 实现 | laew 推荐 Rust crate | 备注 |
|---------|------------|---------------------|------|
| **HTTP 客户端基础** | `lib/core/request.js` + `dispatcher/client-{h1,h2}.js` | `reqwest = "0.12"` + `hyper = "1"` | reqwest 0.12 默认启用 HTTP/1+2，hyper 1.x TLS 走 `rustls` |
| **brotli 解压** | `zlib.createBrotliDecompress` | reqwest feature `brotli`（默认开启） | 走 `brotli` crate |
| **zstd 解压** | `zlib.createZstdDecompress` | reqwest feature `zstd`（**默认关闭**） | 显式 `features = ["zstd"]` |
| **deflate Raw/Zlib 嗅探** | `InflateStream` 首字节 CMF 检测 | `async-compression::tokio::bufread::DeflateDecoder::new(raw: false)` + 首字节 peek 切换 `raw: true` | async-compression 支持 stream API |
| **字符集解码** | `TextDecoder('utf-8')` + BOM 剥离 | `encoding_rs = "0.8"`（UTF-8 + GBK + Big5 + Shift_JIS 全覆盖） + `chardetng = "0.1"` 嗅探 fallback | Firefox 同款 |
| **HTML 字符集嗅探** | 无（依赖 Content-Type） | `html5ever` 解析时同时提取 `<meta charset>` | `markup5ever` 系列 |
| **MIME 解析** | `parseMIMEType` 状态机 | `mime = "0.3"` + `mime_guess`（web 文件类型推断）| RFC 7231 兼容 |
| **流式 body 限制** | `maxResponseSize` + `bodyReadable.dump(limit)` | `tower-http::limit::BodyLimitLayer` 或自实现 `LimitedBody` 包装 `Body::poll_frame` 累加 | hyper `Body` trait 抽象 |
| **AbortSignal 集成** | `requestObject.signal.aborted` + `addAbortListener` | `tokio_util::sync::CancellationToken` + `tokio::select!` race | 与 `Future::poll` 解耦 |
| **Cookie jar** | `lib/web/cookies/index.js`（独立 API） | `reqwest::cookie::Jar` + `Arc<Jar>` | Arc 共享 + 文件持久化 `bincode` |
| **Referer 控制** | `httpRequest.headersList.append('referer', ...)` | `RequestBuilder::header("Referer", value)` 或 `.header(REFERER, "")` 抑制 | 单请求级 |
| **重定向策略** | `request.redirect` 枚举 | `reqwest::redirect::Policy::{none, limited(N), custom}` | 内置 + trait 自定义 |
| **SSE 解析** | `lib/web/eventsource/eventsource-stream.js` 状态机 | `eventsource-stream = "0.1"` 或 `reqwest-eventsource = "0.6"` | 业界最简方案 |
| **超时（headers/body 分离）** | `headersTimeout` / `bodyTimeout` | 单值 `reqwest::ClientBuilder::timeout` + 自实现「first-byte timer」 | 复杂，需自研 |
| **JSON 嗅探** | `+json` 后缀检测 | `essence.ends_with("+json")` 一行匹配 | 简单正则 |

### 5.2 WebFetch 工具 Rust 实现骨架（建议）

```rust
// src/agent/tools/webfetch.rs（建议新增）
use reqwest::{Client, redirect::Policy, header::{HeaderMap, HeaderValue, REFERER, ACCEPT, ACCEPT_ENCODING}};
use encoding_rs::{Encoding, UTF_8};
use chardetng::EncodingDetector;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct WebFetchConfig {
    pub max_response_size: usize,        // 默认 4MB
    pub dump_limit: usize,               // 默认 256KB（返回给 LLM）
    pub timeout_total: Duration,         // 默认 60s
    pub timeout_first_byte: Duration,    // 默认 30s
    pub redirect_policy: Policy,         // 默认 limited(5)
    pub cookie_jar: Option<Arc<Jar>>,
    pub charset_hint: Option<&'static Encoding>, // GBK 等已知站点
}

pub async fn webfetch(url: &str, config: &WebFetchConfig, signal: CancellationToken)
    -> Result<WebFetchResult, WebFetchError>
{
    // 1. 构造请求:Accept / Accept-Encoding / Referer 抑制 / signal 桥接
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("text/html, application/json;q=0.9, */*;q=0.8"));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br, zstd, deflate"));
    // Referer 默认抑制（按 L1590 缺口修复）

    let mut req = client.get(url).headers(headers);
    if let Some(jar) = &config.cookie_jar { req = req.cookie_provider(jar.clone()); }

    // 2. 发起 + 超时 race（first-byte timer + total timer）
    let resp = tokio::select! {
        r = req.send() => r?,
        _ = tokio::time::sleep(config.timeout_total) => return Err(WebFetchError::Timeout),
    };

    // 3. Content-Length 预检（按 L1589 缺口修复）
    if let Some(len) = resp.content_length() {
        if len > config.max_response_size as u64 {
            return Err(WebFetchError::TooLarge(len));
        }
    }

    // 4. 流式下载 + 累加器截断
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::with_capacity(config.dump_limit.min(64 * 1024));
    let mut total = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        total += chunk.len();
        if total > config.max_response_size {
            return Err(WebFetchError::TooLarge(total as u64));
        }
        if buf.len() < config.dump_limit {
            buf.extend_from_slice(&chunk[..chunk.len().min(config.dump_limit - buf.len())]);
        }
        // 超出 dump_limit 但未超 max_response_size: 继续 drain 到 EOF 但不入 buf
    }

    // 5. 解码管线:Content-Encoding 已由 reqwest 处理 → 我们拿到原始字节
    //    然后做 charset 嗅探 → 解码为 String
    let charset = config.charset_hint
        .or_else(|| sniff_charset(&buf, resp.headers().get(CONTENT_TYPE)));
    let (text, _had_errors) = charset.decode(&buf);

    // 6. 截断标记:buf < total 时追加 [TRUNCATED]
    if total > buf.len() {
        return Ok(WebFetchResult {
            text: format!("{}\n\n[TRUNCATED: {}/{} bytes]", text, buf.len(), total),
            status: resp.status(),
            charset,
            truncated: true,
        });
    }

    Ok(WebFetchResult { text: text.into_owned(), status: resp.status(), charset, truncated: false })
}

fn sniff_charset(bytes: &[u8], ct: Option<&HeaderValue>) -> &'static Encoding {
    // 1. Content-Type charset 优先
    if let Some(ct) = ct.and_then(|v| v.to_str().ok()) {
        if let Some(enc) = parse_charset_param(ct) {
            return enc;
        }
    }
    // 2. chardetng 嗅探
    let mut det = EncodingDetector::new();
    det.feed(bytes, true);
    det.guess(None, true)
}
```

### 5.3 与 undici 的等价映射表

| undici 模块 | Rust crate | 行数等价 |
|------------|------------|---------|
| `lib/encoding/index.js` (33 行) | `encoding_rs::UTF_8.decode_with_bom_removal` | 1 行 |
| `lib/web/fetch/util.js::extractMimeType` (52 行) | `mime` crate 单行解析 | 1 行 |
| `lib/web/fetch/body.js::consumeBody` (47 行) | `reqwest::Response::bytes_stream()` + 自定义 Limited | ~50 行 |
| `lib/web/fetch/index.js:2268-2318` 解码链 (50 行) | reqwest 内置（feature gating）+ `async-compression` | 0 行 |
| `lib/web/eventsource/eventsource-stream.js` (493 行) | `eventsource-stream` crate | 0 行 |
| `lib/web/fetch/index.js:1287-1290` 重定向上限 | `reqwest::redirect::Policy::limited(20)` | 1 行 |
| `lib/api/readable.js:265-320` dump (55 行) | 自实现 LimitedBody ~30 行 | 30 行 |

**总工作量估算**：~150 行 Rust 实现 WebFetch 工具核心（含 charset 嗅探 + 截断 + 重定向控制），即可覆盖 undici N1-N5 全部能力。

### 5.4 laew WebFetch 工具的差异化建议

1. **HTML→Markdown 中间层**：undici 只返回原始 HTML 字节，laew WebFetch 应内置 `html2md`（如 `htmd` crate）或 `html5ever` + 自定义 walker 把 HTML 转 Markdown，**直接喂给 LLM**（节省 token 50%+）。
2. **二次结构化**：对 `application/json` 直接 `serde_json::to_string_pretty` 后返回；对 `text/csv` 用 `csv` crate 解析为表格。
3. **PDF/图片转文本**：`pdf-extract` / `image::DynamicImage` + `tesseract-rs`（OCR）—— 本轮不展开。
4. **缓存层**：`moka` 异步 LRU 缓存 URL → Markdown（ETag/Last-Modified 支持），避免重复抓取。

---

## 六、gap 汇总

| 编号 | 维度 | 描述 | 推荐方案 | 优先级 |
|------|------|------|---------|--------|
| **L1576** | N1 解码 | 无 zstd Content-Encoding 解压 | reqwest 启用 `zstd` feature | P1 |
| **L1577** | N1 解码 | 无 Content-Encoding 链级 DoS 防护（无限 gzip） | 自研 `decode_chain()` 限 5 层 | P0 |
| **L1578** | N1 解码 | 无 deflate Raw/Zlib 自动嗅探 | async-compression + 首字节 CMF 检测 | P1 |
| **L1579** | N1 解码 | 无 UTF-8 BOM 剥离 | `encoding_rs::UTF_8.decode_with_bom_removal` | P2 |
| **L1580** | N1 解码 | 无「未知 encoding 熔断」机制 | 解码错误降级使用原始字节 | P1 |
| **L1581** | N2 MIME | 无 Accept 头按 destination 协商 | WebFetch 强制声明 `text/html, application/json` | P2 |
| **L1582** | N2 MIME | 无 GBK/非 UTF-8 字符集解码 | `encoding_rs` + `chardetng` | P0 |
| **L1583** | N2 MIME | 无 `<meta charset>` HTML 内嵌嗅探 | html5ever 解析时提取 | P1 |
| **L1584** | N2 MIME | 无 `+json` 后缀 MIME 识别 | 一行 `ends_with("+json")` | P2 |
| **L1585** | N2 MIME | 无 Multipart FormData 解析 | `multer` / `reqwest::multipart` | P2 |
| **L1586** | N3 截断 | 无 maxResponseSize 连接层强制 | hyper middleware `Body` 字节累加 | P0 |
| **L1587** | N3 截断 | 无 fetchParams dump 等价机制 | WebFetch 工具 `head_only: bool` 区分 | P1 |
| **L1588** | N3 截断 | 无 128KB 默认 dump 上限 | WebFetch 强制 `dump_limit = 256KB` | P0 |
| **L1589** | N3 截断 | 无 Content-Length 头预检（避免下载大文件） | WebFetch 入口预检 `content_length > limit` | P0 |
| **L1590** | N4 重定向 | 无 POST→GET 自动降级抑制（SPA 登录态丢失） | reqwest `redirect::Policy::none` 默认 | P1 |

**汇总**：15 个新 gap（区间 L1576-L1590）
- **P0 紧急（5 项）**：L1577 / L1582 / L1586 / L1588 / L1589 —— 涉及「DoS 防护 / 字符集 / 资源耗尽」，直接影响 WebFetch 工具稳定性与安全性
- **P1 重要（6 项）**：L1576 / L1578 / L1580 / L1583 / L1587 / L1590 —— 增强正确性与健壮性
- **P2 进阶（4 项）**：L1579 / L1581 / L1584 / L1585 —— 体验优化与扩展场景

**累计 gap**：L1-L1590（共 1590 个 gap），本轮新增 15 个集中在 N1-N5 富内容获取维度。

---

## 附录：源码引用清单

| 文件 | 行数 | 引用章节 |
|------|------|---------|
| `lib/web/fetch/index.js` | 2426 | N1 §1.2.1-1.2.2 / N4 §4.2.1-4.2.4 |
| `lib/web/fetch/util.js` | 1525 | N1 §1.2.3 / N2 §2.2.2 |
| `lib/web/fetch/body.js` | 547 | N2 §2.2 (consumeBody/MIME) |
| `lib/web/fetch/request.js` | 1144 | N4 §4.2.2 (referrer) |
| `lib/web/fetch/data-url.js` | 596 | N2 §2.2.3 (parseMIMEType) |
| `lib/web/fetch/formdata-parser.js` | 586 | N2 §2.2.3 (multipart boundary 校验) |
| `lib/web/eventsource/eventsource-stream.js` | 493 | N3 §3.2.4 (SSE maxEventSize) |
| `lib/api/readable.js` | 616 | N3 §3.2.3 (BodyReadable.dump) |
| `lib/handler/redirect-handler.js` | 229 | N4 §4.2.1 (request API 重定向) |
| `lib/interceptor/decompress.js` | 160 | N1 §1.2.5 (API 模式) |
| `lib/encoding/index.js` | 33 | N1 §1.2.4 (UTF-8 BOM) |
| `lib/dispatcher/client.js` | 2000+ | N3 §3.2.1 (maxResponseSize 配置) |
| `lib/dispatcher/client-h1.js` | 2000+ | N3 §3.2.1 (H1 destroy) |
| `lib/dispatcher/client-h2.js` | 2000+ | N3 §3.2.1 (H2 RST_STREAM) |

---

*专题作者：源码分析 Agent · 日期：2026-09-09 · 轮次：第十八轮 · 主文档：[`/undici.md`](../undici.md)*
