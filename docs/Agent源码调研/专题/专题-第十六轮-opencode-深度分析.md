# 专题-第十六轮-opencode-深度分析

> **调研日期**：2026-09-09
> **调研范围**：opencode LLM 协议栈完整抽象层（Route 五层/7 协议/Cache Policy/LLMEvent/录制回放）
> **本轮新增 gap**：L1046-L1047 / L1101-L1115+

---

## 一、Route 五层抽象（llm/src/route/client.ts 400 行）

```typescript
export interface Route<Body, Prepared> {
  readonly id: string
  readonly protocol: ProtocolID            // 协议层：语义 API 契约
  readonly endpoint: Endpoint<Body>        // 端点层：URL 构造
  readonly auth: AuthDef                   // 认证层：凭证加载与注入
  readonly transport: Transport<Body, Prepared, unknown>  // 传输层：HTTP/WebSocket
  readonly defaults: RouteDefaults         // 默认值合并
  readonly body: RouteBody<Body>           // 协议体：Schema + 构造函数
  readonly with: (patch) => Route          // 不可变补丁
}
```

**编译管线**：
```
LLMRequest → applyCachePolicy() → resolveRequestOptions() → route.body.from()
  → Schema.decodeUnknownEffect() → route.prepareTransport() → PreparedRequest
```

**设计亮点**：Protocol ≠ Route。DeepSeek/TogetherAI/Cerebras 复用 OpenAIChat.protocol，无需 fork 300 行。

**laew gap L1046 [P0]**：无 Route 五层抽象。

---

## 二、七大协议实现（llm/src/protocols/）

| 协议文件 | ADAPTER ID | 特殊能力 |
|---------|-----------|---------|
| `anthropic-messages.ts` | `anthropic-messages` | 4 断点 cap、server_tool_use、thinking block |
| `openai-chat.ts` | `openai-chat` | reasoning_content、tool_choice |
| `openai-responses.ts` | `openai-responses` | encrypted_content、item_reference、service_tier |
| `bedrock-converse.ts` | `bedrock-converse` | inferenceConfig、CachePointBlock |
| `bedrock-event-stream.ts` | `aws-event-stream` | 二进制帧解码、CRC 校验 |
| `gemini.ts` | `gemini` | thoughtSignature、thinkingConfig |
| `openai-compatible-chat.ts` | `openai-compatible-chat` | 复用 OpenAIChat.protocol |

### Anthropic 4 断点 Cap

```typescript
const ANTHROPIC_BREAKPOINT_CAP = 4
const cacheControl = (breakpoints, cache) => {
  if (breakpoints.remaining <= 0) {
    breakpoints.dropped += 1
    return undefined  // 静默丢弃超出 cap 的断点
  }
  breakpoints.remaining -= 1
  return Cache.ttlBucket(cache.ttlSeconds) === "1h" ? EPHEMERAL_1H : EPHEMERAL_5M
}
```

---

## 三、Cache Policy 自动注入（llm/src/cache-policy.ts 112 行）

```typescript
const AUTO: CachePolicyObject = {
  tools: true,
  system: true,
  messages: "latest-user-message"
}

const RESPECTS_INLINE_HINTS = new Set(["anthropic-messages", "bedrock-converse"])
// OpenAI/Gemini 使用隐式前缀缓存，跳过整个 policy pass
```

**标记放置**：
- `markLastTool(tools, hint)` — 最后一个 tool 定义
- `markLastSystem(system, hint)` — 最后一个 system part
- `markMessages(messages, strategy, hint)` — latest-user-message / latest-assistant / tail-N

**laew gap L1047 [P0]**：无 Cache Policy 自动注入。

---

## 四、LLMEvent 统一事件模型（schema/events.ts）

```typescript
const llmEventTagged = Schema.Union([
  StepStart, TextStart, TextDelta, TextEnd,
  ReasoningStart, ReasoningDelta, ReasoningEnd,
  ToolInputStart, ToolInputDelta, ToolInputEnd,
  ToolCall, ToolResult, ToolError,
  StepFinish, Finish, ProviderErrorEvent
]).pipe(Schema.toTaggedUnion("type"))
```

**Usage 不变量**：
- `nonCachedInputTokens + cacheReadInputTokens + cacheWriteInputTokens = inputTokens`
- `reasoningTokens ≤ outputTokens`
- 每个 protocol mapper 计算 provider 不提供的另一侧，`Math.max(0, …)` 夹紧

---

## 五、LLMError 错误分类体系（schema/errors.ts）

| _tag | retryable | 说明 |
|------|-----------|------|
| `InvalidRequest` | false | 请求格式错误 |
| `NoRoute` | false | 无匹配路由 |
| `Authentication` | false | 认证失败（missing/invalid/expired） |
| `RateLimit` | true | 限流（含 retryAfterMs） |
| `QuotaExceeded` | false | 配额耗尽 |
| `ContentPolicy` | false | 内容违规 |
| `ProviderInternal` | true | 服务端错误 |
| `TransportReason` | false | 传输层错误 |
| `InvalidProviderOutput` | false | 输出解析失败 |

---

## 六、Context Overflow 检测（provider-error.ts 44 行）

```typescript
const patterns = [
  /prompt is too long/i, /request_too_large/i,
  /exceeds the context window/i, /maximum context length is \d+/i,
  /reduce the length of the messages/i, /context length exceeded/i,
  /request entity too large/i, /too many tokens/i, ...
]
const exclusions = [/^(throttling error|service unavailable):/i, /rate limit/i]
```

---

## 七、WebSocket Transport（route/transport/websocket.ts 200+ 行）

```typescript
export interface WebSocketConnection {
  readonly sendText: (message: string) => Effect<void, LLMError>
  readonly messages: Stream<string | Uint8Array, LLMError>
  readonly close: Effect<void, never>
}

// Queue-based 流式
Queue.bounded<string | Uint8Array, LLMError | Cause.Done<void>>(128)
```

**waitOpen 实现**：通过 Effect.callback 注册 open/error/close 事件，signal.addEventListener("abort") 处理取消。

---

## 八、录制回放测试（packages/http-recorder/ + packages/llm/test/）

```typescript
export const HttpRecorder = { http, socket } as const

// Recorded Test 工厂
export const recordedTests = (options) =>
  recordedEffectGroup({
    cassetteExists: (cassette) => HttpRecorderInternal.hasCassetteSync(cassette, { directory: FIXTURES_DIR }),
    layer: ({ cassette, metadata, options, caseOptions, recording }) => {
      const mode = recording ? "record" : "replay"
      // ...
    }
  })
```

- **总测试文件**：660 个 `*.test.ts`
- **录制 fixtures**：40 个 cassette JSON
- **覆盖协议**：anthropic-messages / openai-chat / openai-responses / bedrock-converse / gemini

---

## 九、新增 gap 清单

| 编号 | 维度 | 描述 | 优先级 |
|------|------|------|--------|
| L1046 | 协议栈 | 无 Route 五层抽象 | P0 |
| L1047 | 缓存策略 | 无 Cache Policy 自动注入 | P0 |
| L1101 | 协议 | 无 Anthropic 4 断点 cap 处理 | P1 |
| L1102 | 协议 | 无 OpenAI Responses 双传输共享 | P1 |
| L1103 | 协议 | 无 Bedrock 二进制帧解码 | P1 |
| L1104 | 协议 | 无 Gemini thoughtSignature 往返 | P1 |
| L1105 | 事件 | 无 LLMEvent 统一事件模型（17 类型） | P1 |
| L1106 | 错误 | 无 LLMError 10 种 _tag 分类 | P1 |
| L1107 | 传输 | 无 WebSocket Queue-based 流式 | P1 |
| L1108 | 测试 | 无录制回放系统（HttpRecorder） | P1 |
| L1109 | 结构化 | 无 generateObject 强制合成 tool call | P2 |
| L1110 | 溢出 | 无 Context Overflow 28 种正则检测 | P1 |

---

**报告完成日期**：2026-09-09
**分析基础**：opencode packages/llm/src/route/ + protocols/ + schema/ + packages/http-recorder/ 逐行分析
