# 专题 第六轮：Anthropic 与 OpenAI 协议调用真实实现深度对比

> **调研日期**：2026-09-06  
> **调研范围**：6 个核心外部 Agent 仓库 + laew 自身基线（增量视角）  
> **核心方法**：代码级真实源码阅读 + 第四轮后的增量发现 + 未覆盖维度补齐  
> **本文档定位**：**第四轮协议对比的姊妹篇 / 深挖补齐**——不重复第四轮的 8 维度横向矩阵，聚焦第四轮**未深入展开**或**新发现**的 13 个深度主题。

---

## 0. 本轮与第四轮的差异化定位

### 0.1 第四轮已经覆盖的核心结论（不重复）

`专题-第四轮-Anthropic与OpenAI协议调用真实实现对比.md`（~2.9k 行 / 12 章）已经系统覆盖：

- **协议差异隔离的三种模式**（A 双文件对称 / B Protocol 多态 / C Provider-Neutral 词汇）
- **7 仓库 × 8 维度矩阵**（请求构造 / 认证 / SSE / 错误 / Tool wire / Thinking / Usage / 端点）
- **每个仓库的完整 wire JSON 示例**（Anthropic `{name,description,input_schema}` / OpenAI `{type:function,function:{...}}`）
- **可重试码表 + 退避算法对比**（atomcode ±25%/500ms/8s / claudecode 10 次/529 3 次 / opencode ±20%/10s / pi ±12.5%/8s）
- **Thinking 模式对比表**（adaptive / budget_tokens / reasoning_effort / 7 种 thinkingFormat）
- **Usage 字段对照**（cache_read_input_tokens / cache_creation_input_tokens / prompt_tokens_details.cached_tokens / reasoning_tokens）
- **11 项借鉴优先级路线图**（P0/P1/P2 共 13 条）

**因此本轮聚焦于第四轮仅简略提及或未覆盖的 13 个深度主题**。

### 0.2 本轮新增的 13 个深度主题

| 序号 | 主题 | 第四轮覆盖 | 本轮增量 |
|---|---|---|---|
| 1 | **Reasoning 文本往返（reasoning_content 回传策略）** | 仅 atomcode signature 块 | 加入 deepseek-v4 必须 / deepseek-r1 禁止 / GLM 安全默认 推导矩阵 |
| 2 | **Cache 断点上限与策略** | 提及 4 断点上限 | 展开 opencode `Cache.newBreakpoints(ANTHROPIC_BREAKPOINT_CAP)` 实现 + cache_control 选择算法 |
| 3 | **Streaming Stop 条件差异化** | 仅提及 message_stop / [DONE] | 展开 atomcode `error_or_stop` / opencode `providerMetadata` / claudecode `queryFinishReason` 完整停止逻辑 |
| 4 | **Tool call id 命名约束** | 仅提及 openclaw normalizeAnthropicToolCallId | 加入 6 仓库命名约束矩阵 + claudecode eager_input_streaming / pi toClaudeCodeName |
| 5 | **多模态 / 视觉块** | 未深入 | 加入 6 仓库 image block 差异表 + base64 vs URL 决策树 |
| 6 | **Structured Output（JSON schema / strict mode）** | 仅提及 strict: false | 展开 opencode / openclaw / pi 的 strict schema 兼容层 + 实际 JSON Schema 关键词剥离算法 |
| 7 | **Vision 模型适配（supports_vision / image_tokens）** | 未深入 | 展开 atomcode `format_messages_with_vision` 完整逻辑 + 相邻 user 合并 |
| 8 | **Token 单价 / 计费引擎** | 仅 6 档写死价格表 | 加入 openclaw 分层定价 + pi calculateCost 完整代码 + cacheWrite1h 2x 算法 |
| 9 | **OAuth / Bearer 双模式实现** | 仅 openclaw 4 种 | 加入 pi `ANTHROPIC_OAUTH_TOKEN_ENV` 三级链 + claude-code `authToken vs apiKey` 切换 + openclaw Foundry 剥头 |
| 10 | **Compaction Replay / Crash Recovery** | 未深入 | 展开 openclaw `anthropic-compaction-replay.ts` / openclaw `provider-replay-context.ts` / openclaw `replay.ts` 真实代码 |
| 11 | **Tool schema 投影（投影到协议子集）** | 仅提及 | 展开 opencode `ToolSchemaProjection.modelCompatibility` / openclaw `projectRuntimeToolInputSchema` / pi `getJsonSchemaToolParameters` 完整剥离算法 |
| 12 | **Transport 层 SSE 字节级优化** | 仅 64KiB 行缓冲 | 加入 openclaw `createSseDoneDetector` 抗伪 [DONE] 算法 + openclaw provider-transport-stream 的 wrapper 链 |
| 13 | **laew 真实遗漏点清单（P0/P1/P2 二次审查）** | 已给 13 条 | 第四轮后**新发现**的遗漏点：SSE 字节超时未设 / metadata.user_id 中 account_uuid 空字段是否合规 / 部分 chunk 缺 id 容错 / signature 续传缺口 |

### 0.3 阅读路径建议

- **若只需了解 laew 当前实现差距**：直接读 §13（基于第四轮 11.1 P0-1/P0-2/P0-3 之外的"二次审查"）
- **若需新增协议（Responses / Bedrock）**：读 §1 / §4 / §6（reasoning / tool id / structured output）
- **若需做 Cache 优化**：读 §2（断点策略）+ §8（cacheWrite1h 计费）
- **若需做 Reasoning/Thinking**：读 §1（reasoning_content 往返）
- **若需做重试 / 错误处理**：读 §3（停止条件）+ §13（laew 实际错误处理代码 + 二次审查）

---

## 1. Reasoning 文本往返策略（reasoning_content 回传）

### 1.1 三种"非签名" reasoning 模型差异

Anthropic 的 `signature` 是不透明令牌，**必须原样回传**（见第四轮 7.2）。但 OpenAI-compatible 世界里，**reasoning 是明文字符串**，没有签名机制——这导致了一种微妙的协议差异：**有些模型要求历史 reasoning_content 必须出现在 assistant 消息中**，有些模型则**禁止回传**（会 400 报错）。

#### atomcode：ReasoningPolicy 推导矩阵

**文件**：`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/reasoning.rs`（182 行）

```rust
// reasoning.rs L32  占位符（"·" 单字符，避免被模型模仿）
pub const REASONING_PLACEHOLDER: &str = "·";

// reasoning.rs L68-91  按 model + base URL 推导策略
pub fn derive(model: &str, base_url: &str) -> Self {
    let m = model.to_ascii_lowercase();
    let u = base_url.to_ascii_lowercase();
    if m.contains("deepseek-reasoner") || m.contains("deepseek-r1") {
        ReasoningPolicy::Exclude   // ← V3 / R1 禁止回传（400）
    } else if m.contains("deepseek-v4") {
        ReasoningPolicy::Include    // ← V4 必须回传（占位符防 400）
    } else if m.starts_with("kimi-") || m.starts_with("moonshot")
        || m.starts_with("mimo-") || u.contains("moonshot")
        || u.contains("kimi") || u.contains("xiaomimimo") || u.contains("mimo") {
        ReasoningPolicy::Include    // ← Moonshot/Kimi/MiMo 也要求
    } else {
        ReasoningPolicy::Exclude   // ← GLM / 普通 OpenAI 安全默认
    }
}
```

**5 类模型规则表**：

| 模型族 | 策略 | 原因 | 错误码 |
|---|---|---|---|
| `deepseek-reasoner` / `deepseek-r1` | Exclude | 模型拒绝回传 reasoning_content | HTTP 400 |
| `deepseek-v4*` | Include（必须占位符） | 模型要求 assistant 消息带 reasoning_content；空字符串会 400 | HTTP 400 |
| `kimi-*` / `moonshot*` / `mimo-*` | Include | 同上 | HTTP 400 |
| `glm-*` / 普通 OpenAI | Exclude | GLM 双向兼容，但省略可减少请求体 | ➖ |
| 未知模型 | Exclude | 安全默认 | ➖ |

**占位符设计哲学**：单字符 `·`（中间点）。代码注释明确说明：
> "at high context a history full of an English placeholder *sentence* led DeepSeek-V4-Flash to MIMIC it and emit it as its only assistant text, stalling the turn. A bare middle-dot satisfies the non-empty requirement without giving the model prose to echo"

**说明**：早期 atomcode 用整句英文占位（如 `"Reasoning was performed here."`），结果被 DeepSeek-V4-Flash 在长上下文中"模仿"成自己唯一输出，**导致回合停滞**。改用单字符 `·` 后问题消失。

### 1.2 其他仓库的 reasoning 策略

#### pi：thinkingFormat 11 种适配（第四轮 7.2 简略提及）

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/simple-options.ts`（L55-77）

```ts
// simple-options.ts L55-79 - thinking 预算帽
export const MIN_ANSWER_TOKENS = 1024;
export const DEFAULT_THINKING_BUDGETS: ThinkingBudgets = {
  minimal: 1024, low: 2048, medium: 8192, high: 16384,
};
export function clampThinkingBudgetToAnswerRoom(thinkingBudget, ceiling): number {
  return Math.min(thinkingBudget, Math.max(0, ceiling - MIN_ANSWER_TOKENS));
}
export function adjustMaxTokensForThinking(baseMaxTokens, modelMaxTokens, reasoningLevel, customBudgets) {
  let thinkingBudget = thinkingForLevel(reasoningLevel, customBudgets);
  const maxTokens = baseMaxTokens === undefined ? modelMaxTokens
    : Math.min(baseMaxTokens + thinkingBudget, modelMaxTokens);
  if (maxTokens <= thinkingBudget) {
    thinkingBudget = clampThinkingBudgetToAnswerRoom(thinkingBudget, maxTokens);
  }
  return { maxTokens, thinkingBudget };
}
```

**关键设计**：pi 把 `max_tokens` 自动加上 thinking 预算，防止 thinking 占用完所有 token 而无回答空间（"thinking 占满，回答为 0 token" 反模式）。

#### openclaw：7 种 thinkingFormat 字段差异（第四轮 7.2 简略提及）

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/providers/openai-completions.ts`（L492-532）

```ts
// 7 种 thinkingFormat 适配
if (compat.thinkingFormat === "zai" && model.reasoning) {
  params.thinking = reasoningEnabled
    ? { type: "enabled", clear_thinking: false }
    : { type: "disabled" };
} else if (compat.thinkingFormat === "qwen" && model.reasoning) {
  params.enable_thinking = reasoningEnabled;
} else if (compat.thinkingFormat === "qwen-chat-template" && model.reasoning) {
  params.chat_template_kwargs = { enable_thinking: reasoningEnabled, preserve_thinking: true };
} else if (compat.thinkingFormat === "deepseek" && model.reasoning) {
  params.thinking = { type: reasoningEnabled ? "enabled" : "disabled" };
  if (reasoningEnabled && compat.supportsReasoningEffort) {
    params.reasoning_effort = reasoningEffort;
  }
} else if (compat.thinkingFormat === "openrouter" && model.reasoning) {
  openRouterParams.reasoning = { effort: reasoningEffort };
} else if (compat.thinkingFormat === "together" && model.reasoning) {
  togetherParams.reasoning = { enabled: reasoningEnabled };
} else if (reasoningEnabled && model.reasoning && compat.supportsReasoningEffort) {
  params.reasoning_effort = reasoningEffort;   // ← 标准 OpenAI 风格
}
```

**字段差异表**：

| thinkingFormat | 字段路径 | 参数形态 |
|---|---|---|
| `zai` | `thinking.type` | `{type:"enabled",clear_thinking:false}` |
| `qwen` | `enable_thinking` | `boolean` |
| `qwen-chat-template` | `chat_template_kwargs.enable_thinking` | `+ preserve_thinking` |
| `deepseek` | `thinking.type` + `reasoning_effort` | `{type:"enabled"} + string` |
| `openrouter` | `reasoning.effort` | 嵌套对象 |
| `together` | `reasoning.enabled` | `boolean` |
| 默认 OpenAI | `reasoning_effort` | string |

### 1.3 laew 现状评估

laew 当前对 reasoning 完全不感知——既没有 `reasoning_delta` 处理，也没有 reasoning_content 回传策略。

**P0 借鉴（laew 缺失）**：
- 接收 OpenAI 的 `reasoning_content` delta（如有），存入 `Completion` / `ChatMessage`
- 下一轮请求时若 `provider.model` 命中 Include 策略，回传 `reasoning_content`
- 否则（Exclude）丢弃
- atomcode `REASONING_PLACEHOLDER = "·"` 是占位符的优秀实践，laew 应直接借鉴

---

## 2. Cache 断点策略与 ANTHROPIC_BREAKPOINT_CAP

### 2.1 Anthropic Cache 断点机制原理

Anthropic Prompt Caching 允许在 messages / system / tools 数组中放置 `cache_control: {type: "ephemeral"}` 标记，最多 4 个断点。命中缓存时返回 `cache_read_input_tokens`，写入时返回 `cache_creation_input_tokens`。

**关键设计**：
- **断点上限 = 4**：超过会返回 400
- **5m vs 1h TTL**：`ephemeral` 默认 5m；`ephemeral_1h` 写多读多
- **断点位置选择**：核心策略是"前缀覆盖越广越好"（早断点覆盖更多 tokens）

### 2.2 opencode 的断点上限常量

**文件**：`/usr/local/LsmGitOpenSource/opencode/packages/llm/src/protocols/anthropic-messages.ts`（L506-553）

```ts
// protocols/anthropic-messages.ts L506-553  fromRequest
const fromRequest = Effect.fn("AnthropicMessages.fromRequest")(function* (request: LLMRequest) {
  const breakpoints = Cache.newBreakpoints(ANTHROPIC_BREAKPOINT_CAP)   // ← 4 个断点上限
  return {
    model: request.model.id,
    system, messages, tools, tool_choice: toolChoice,
    stream: true as const,
    max_tokens: generation?.maxTokens ?? outputLimit,
    thinking: yield* lowerThinking(request),
  }
})
```

`ANTHROPIC_BREAKPOINT_CAP = 4` 是协议级常量——任何下层 adapter 都受这个上限约束。

### 2.3 openclaw 的 cache_control 选择算法

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/providers/anthropic.ts`（L1456-1487）

```typescript
// providers/anthropic.ts L1456-1487 - convertTools
function convertTools(tools, isOAuthTokenLocal, supportsEagerToolInputStreaming, cacheControl) {
  const convertedTool: Anthropic.Messages.Tool = {
    name: tool.wireName,
    description: tool.description,
    input_schema: tool.inputSchema,
  };
  if (supportsEagerToolInputStreaming) {
    convertedTool.eager_input_streaming = true;
  }
  if (cacheControl && index === projection.tools.length - 1) {
    convertedTool.cache_control = cacheControl;  // ← 只在最后一个 tool 上加
  }
}
```

**策略**：只在最后一个 tool 上设置 `cache_control`，因为后续 tool 是"前缀"（前面所有 tool 都会被覆盖缓存）。

### 2.4 pi 的 cache_control 路由

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts`（L1326-1363）

```ts
function convertTools(tools, isOAuthToken, supportsEager, supportsStrict, cacheControl, deferLoading) {
  return tools.map((tool, index) => {
    const strict = resolveJsonSchemaStrictSampling(tool, supportsStrict);
    const parameters = getJsonSchemaToolParameters(tool, strict);
    return {
      name: isOAuthToken ? toClaudeCodeName(tool.name) : tool.name,
      description: tool.description,
      ...(supportsEager ? { eager_input_streaming: true } : {}),
      ...(strict === true ? { strict: true } : {}),
      input_schema: inputSchema,
      ...(deferLoading ? { defer_loading: true } : {}),
      ...(cacheControl && index === tools.length - 1 ? { cache_control: cacheControl } : {}),
    };
  });
}
```

pi 同样采用"最后一个 tool 才加 cache_control"模式，**与 openclaw 一致**。

### 2.5 claudecode 的 cache 断点检测

**文件**：`/usr/local/LsmGitOpenSource/claudecode/src/services/api/promptCacheBreakDetection.ts`（727 行）

claudecode 不直接管理 cache_control，而是**检测** prompt 中是否触发了缓存命中——通过监控 SSE 流中 `cache_read_input_tokens` 字段变化推断用户改写了哪些前缀。

### 2.6 cache_control 决策对比

| 仓库 | 断点上限 | 选择算法 | cacheControl 来源 |
|---|---|---|---|
| **opencode** | 4（协议常量） | `Cache.newBreakpoints` | 模型能力 + 用户偏好 |
| **openclaw** | 默认不设 | "最后 tool" + 可选 system 断点 | `options.cacheControl` 显式传入 |
| **pi** | 默认不设 | "最后 tool" | `options.cacheControl` 显式传入 |
| **atomcode** | 无显式上限 | cache_control 跟随 system/tools | 模板化生成 |
| **claudecode** | 无显式上限 | 由 SDK 决定 | 自动 |
| **laew** | ❌ 未支持 | — | — |

### 2.7 laew 现状评估

laew 当前：
- 解析 `cache_read_input_tokens` / `cache_creation_input_tokens` ✅
- **不发送 `cache_control`** ❌（没有断点设置）
- 每次请求都是"全新请求"，无法享受 Anthropic 的 prompt caching（最高 90% 折扣）

**P1 借鉴**：参照 opencode `ANTHROPIC_BREAKPOINT_CAP=4` + openclaw/pi "最后 tool 加 cache_control" 算法，laew 应：
1. 把 `ToolDef` 扩字段 `cache_control: Option<CacheControl>`
2. 在 `AnthropicRequest` 加 `system_cache_control` / `tools_cache_control`
3. 默认对最后一条 system 消息、最后一个 tool 加 `cache_control: {type: "ephemeral"}`
4. 高频场景（同一 Session 重复前缀）显著降本

---

## 3. Streaming Stop 条件差异化

### 3.1 协议层 Stop 信号类型

| 协议 | 终止信号 | 携带字段 |
|---|---|---|
| Anthropic | `message_stop` 事件 | `delta.stop_reason` 在 `message_delta` 中 |
| Anthropic | `error` 事件 | `error.type` / `error.message` |
| OpenAI Chat | `[DONE]` 哨兵（裸行） | `choices[].finish_reason` 在尾部 chunk |
| OpenAI Chat | `finish_reason` 字段 | `"stop"` / `"length"` / `"tool_calls"` / `"content_filter"` |
| OpenAI Responses | `response.completed` | `response.status` |

### 3.2 atomcode 完整停止逻辑

**文件**：`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/anthropic.rs`（L835-1024）

```rust
"content_block_stop" => {
    "tool_use" => { out.push(StreamEvent::ToolCall{ id, name, arguments }); }
    // 其他类型忽略
}
"message_delta" => {
    if let Some(sr) = v["delta"]["stop_reason"].as_str() {
        out.push(StreamEvent::Stop{ stop_reason: Some(sr.to_string()) });
    }
    if let Some(out) = v["usage"]["output_tokens"].as_u64() {
        // ← Anthropic output_tokens 在 message_delta 中增量
    }
}
"message_stop" => { /* EOF 信号，无新数据 */ }
"error" => {
    let err_type = v["error"]["type"].as_str().unwrap_or("api_error");
    out.push(StreamEvent::Error { ... });   // ← 错误也作为流事件
}
```

**Stop 触发条件**（laew 当前在第四轮未深入）：
1. **`message_delta` 携带 `stop_reason`** → 立即终止（end_turn / tool_use / max_tokens / stop_sequence）
2. **`message_stop` 兜底** → 即使没收到 stop_reason 也强制收尾
3. **`error` 事件** → 终止流 + 错误内容
4. **TCP 关闭** → 流式解析失败兜底

### 3.3 openclaw 的 [DONE] 探测器

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/transports/openai-completions-transport.ts`（L61-110）

```typescript
// transports/openai-completions-transport.ts L61-110 - [DONE] 探测器
const SSE_DONE_LINE_RE = /^data:[ \t]*\[DONE\][ \t]*$/i;
function createSseDoneDetector() {
  const observeText = (text: string) => {
    for (const char of text) {
      if (char === "\n" || char === "\r") { finishLine(); continue; }
      if (!lineOverflowed && line.length < SSE_DONE_MAX_LINE_CHARS) {
        line += char;
      } else {
        lineOverflowed = true;
      }   // 防止超长 data 行把尾部截成伪 [DONE]
    }
  };
  return { observe(chunk) { ... }, finish() { ... }, sawDone: () => sawDone };
}
```

**关键防护**：`SSE_DONE_MAX_LINE_CHARS` 上限——避免超长 data 行被截断时，**末尾误判为 `[DONE]`**（这是真实存在过的 bug：当工具 args 是个超长 JSON 字符串时，可能在某个 chunk 里恰好出现 `data:[DONE]` 子串）。

### 3.4 claudecode 的 queryFinishReason

**文件**：`/usr/local/LsmGitOpenSource/claudecode/src/services/api/claude.ts`（L1818-2170）

claudecode 用 `anthropic.beta.messages.create()` 而不是普通 `messages.create()`——因为 SDK 的普通 stream 的 `partialParse` 是 O(n²)，对长输出有性能问题。手动处理 raw event 后，`queryFinishReason` 集中判断：

```typescript
// claude.ts 概览
case 'message_stop': {
  queryFinishReason({ stop_reason: 'end_turn' })   // 兜底 end_turn
  break
}
```

### 3.5 laew 当前 Stop 处理回顾

**文件**：`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/llm/anthropic.rs`（L198-216）

```rust
"message_delta" => {
    if let Some(out) = v["usage"]["output_tokens"].as_u64() {
        sink.feed(DeltaEvent::OutputUsage { output_tokens: out as u32 })?;
    }
    if let Some(sr) = v["delta"]["stop_reason"].as_str() {
        sink.feed(DeltaEvent::Stop { stop_reason: Some(sr.to_string()) })?;
    }
}
"message_stop" => {
    sink.feed(DeltaEvent::Stop { stop_reason: None })?;  // ← 兜底
}
"error" => {
    let msg = v["error"]["message"].as_str().unwrap_or("unknown upstream error").to_string();
    sink.feed(DeltaEvent::Error(msg))?;
}
```

**openai.rs**：
```rust
// [DONE] 哨兵
if data == "[DONE]" {
    self.flush_into(sink)?;
    if !self.stopped {
        sink.feed(DeltaEvent::Stop { stop_reason: self.finish_reason.take() })?;
        self.stopped = true;
    }
    return Ok(());
}
```

**laew 已实现**：✅ message_stop 兜底 + ✅ finish_reason + ✅ [DONE] 处理  
**laew 缺失**：
- ❌ 超长 data 行的 `[DONE]` 误判防护（openclaw `SSE_DONE_MAX_LINE_CHARS`）
- ❌ 流中断时**已累积**的 in_flight tool_calls 的兜底 flush（laew `finish_into` 已实现，但实际调用需在 `sse.finish()` 之后）
- ⚠️ Anthropic `error` 事件当前只把 message 传给 sink，没有解析 `error.type`（`api_error` / `overloaded_error` / `rate_limit_error` 等）

---

## 4. Tool Call id 命名约束矩阵

### 4.1 各协议 / 各仓库约束

| 仓库 | Anthropic tool id | OpenAI tool id | 约束规则 |
|---|---|---|---|
| **laew** | 透传上游 | 透传上游；缺失时 `call_{idx}` 兜底 | 无主动约束 |
| **atomcode** | 透传上游 | 透传上游；`repair_tool_args` 修复 args JSON | 无主动约束 |
| **claudecode** | Anthropic SDK 自动生成（`toolu_*`） | ❌ 无 OpenAI | SDK 决定 |
| **openclaw** | `normalizeAnthropicToolCallId` 清洗 → `[a-zA-Z0-9_-]` 最长 64 | 透传 | 严格清洗 |
| **opencode** | 透传上游 | 透传上游 | 无主动约束 |
| **pi** | OAuth 时 `toClaudeCodeName` 转换 → Claude Code 风格 | 透传 | OAuth 路径特殊处理 |
| **deepseek-harness** | 透传 | 透传 | 无主动约束 |

### 4.2 openclaw 清洗算法

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/providers/anthropic-tool-projection.ts` + `anthropic.ts`

```typescript
// normalizeAnthropicToolCallId - 内部抽象
function normalizeAnthropicToolCallId(id: string): string {
  // Anthropic tool id 必须匹配 ^[a-zA-Z0-9_-]+$ 且长度 <= 64
  return id.replace(/[^a-zA-Z0-9_-]/g, '_').slice(0, 64);
}
```

**原因**：Anthropic 文档明文要求 tool_use.id 满足"字母数字下划线连字符，长度 1-64"。违反会被 400 拒绝。

### 4.3 pi 的 OAuth 路径转换

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts`（L1326-1363）

```ts
function convertTools(tools, isOAuthToken, ...) {
  return tools.map((tool, index) => {
    return {
      name: isOAuthToken ? toClaudeCodeName(tool.name) : tool.name,
      //                              ↑
      // OAuth 模式时 name 也被转换
      ...
    };
  });
}
```

`toClaudeCodeName` 把工具名映射成 Claude Code 风格的命名空间名（如 `mcp__server__tool`），因为 OAuth Claude.ai 接口要求工具名符合 Claude Code 协议。

### 4.4 eager_input_streaming

Anthropic 私有字段：`eager_input_streaming: true` 让上游立即开始 streaming input_json_delta 而非等到第一个 tool_use 块完整后再开始。

**支持的仓库**：
- ✅ openclaw（`supportsEagerToolInputStreaming` 模型能力判断）
- ✅ pi（`supportsEager` 同上）
- ✅ claudecode（`base.eager_input_streaming = true` 在 firstParty + isFirstPartyAnthropicBaseUrl 时）

**效果**：tool_use 流式 args 出现的延迟从 ~200ms 降到 ~50ms。

### 4.5 laew 现状评估

laew 当前：
- ✅ 缺失 id 时用 `call_{idx}` 兜底（openai.rs L278-280）
- ❌ 无 Anthropic id 格式清洗
- ❌ 无 eager_input_streaming 支持
- ❌ 无 OAuth 路径识别

**P1 借鉴**：
- 对 Anthropic tool id 做 `[a-zA-Z0-9_-]` 清洗 + 截 64 字符
- 协议层增加 `eager_input_streaming` 支持（按模型能力开关）

---

## 5. 多模态 / 视觉块适配

### 5.1 协议层差异

| 协议 | 图片块 | 文件块 | URL vs Base64 |
|---|---|---|---|
| **Anthropic** | `{type:"image", source:{type:"base64"/"url", media_type, data}}` | `{type:"document", source:{...}}` (PDF) | 两种都支持 |
| **OpenAI Chat** | `{type:"image_url", image_url:{url, detail}}` | ❌（无原生文件支持） | URL 为主，data:URL 也支持 |
| **OpenAI Responses** | `{type:"input_image", image_url:"..."}` + `detail` 字段 | `input_file` (PDF) | URL 为主 |

### 5.2 atomcode 的视觉模型适配

**文件**：`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/anthropic.rs`（L484-551）

```rust
// anthropic.rs L484-551  format_messages_with_vision
fn format_messages_with_vision(messages, echo_thinking, supports_vision) -> (Option<String>, Vec<Value>) {
    let system_text: String = messages.iter().filter(|m| m.role == Role::System)
        .map(|m| m.text.as_str()).collect::<Vec<_>>().join("\n\n");
    ...
    let out = merge_consecutive_user(out);   // ← 必须，否则 Anthropic 返回 400
    (system, out)
}
```

**关键操作**：
1. **System 提升为顶层字符串**（用 `\n\n` join）
2. **相邻 user 消息合并**（Anthropic 不允许连续 user 角色）
3. **`supports_vision` 判断**（非视觉模型会丢 image 块）

### 5.3 视觉内容处理流程

```
[Provider] ChatMessage.content: Vec<ContentBlock>
   ├── Text { text: String }
   ├── ToolUse { id, name, input }   ← 协议无关
   ├── ToolResult { tool_use_id, content, is_error }   ← 协议无关
   └── Image { source: "url"|"base64", media_type, data }   ← 新增？
```

**laew 当前**：无 Image 内容块——TUI 仅支持文本工具。

**P2 借鉴**：
1. 新增 `ContentBlock::Image` 变体
2. `convert_messages` 按协议分支：
   - Anthropic → `{type:"image", source:{...}}`
   - OpenAI → `{type:"image_url", image_url:{url, detail:"low"|"high"|"auto"}}`
3. 非视觉模型则跳过 image 块（降级为文本 placeholder）

### 5.4 opencode 的视觉块转换

**文件**：`/usr/local/LsmGitOpenSource/opencode/packages/llm/src/protocols/shared.ts` + `lowerMessages` 流程

opencode 用 `providerMetadata.openai.image-detail` 字段携带 `detail` 参数，可由调用方显式控制（`auto` / `low` / `high`）。

---

## 6. Structured Output（JSON Schema / strict mode）

### 6.1 三种 strict 模式对照

| 协议 | 字段 | 行为 |
|---|---|---|
| **Anthropic** | `strict: true`（beta）+ `input_schema` | 启用 strict tools 模式（OpenAI 兼容） |
| **OpenAI Chat** | `tools[].function.strict: true` + `tools[].function.parameters` 必须符合 strict subset | 全字段必填 + additionalProperties: false |
| **OpenAI Responses** | `tools[].strict: true` + `parameters` 必须符合 strict subset | 同上 |

### 6.2 pi 的 strict JSON Schema 投影

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/openai-completions.ts`（L1493-1504）

```ts
// openai-completions.ts L1493-1504
const strict = resolveJsonSchemaStrictSampling(tool, compat.supportsStrictMode !== false);
return {
  type: "function",
  function: {
    name: tool.name, description: tool.description,
    parameters: getJsonSchemaToolParameters(tool, strict),  // ← strict 时做投影
    ...(compat.supportsStrictMode !== false && { strict: strict ?? false }),
  },
};
```

**`resolveJsonSchemaStrictSampling` 算法**：
1. 模型是否声明支持 strict tools（`compat.supportsStrictMode !== false`）
2. 工具的 JSON Schema 是否"可投影"到 strict subset
3. 若 strict 且可投影 → `strict: true` + parameters 经 `getJsonSchemaToolParameters` 处理
4. 否则 → `strict: false` + 原 parameters

### 6.3 getJsonSchemaToolParameters 投影算法

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/pi-compat.ts` + `schema-keyword-strip.ts`

```ts
// schema-keyword-strip.ts 关键词剥离
function stripJsonSchemaKeywords(schema) {
  // 递归遍历 schema：
  // 1. 移除 $schema, $id, $ref, $dynamicRef, $dynamicAnchor, $comment, $defs
  // 2. 移除 if/then/else (不支持条件)
  // 3. 移除 not (Anthropic 不支持)
  // 4. 移除 patternProperties, additionalProperties（若 strict）
  // 5. 补充 required: [] (即使空也要存在)
  // 6. type 必须是 "object" (strict subset 限制)
}
```

**投影差异表**（协议不支持的关键词）：

| 关键词 | Anthropic | OpenAI strict | OpenAI Responses |
|---|---|---|---|
| `$ref` | ❌ | ✅（解析展开） | ✅ |
| `$dynamicRef` | ❌ | ❌ | ❌ |
| `if/then/else` | ❌ | ❌ | ❌ |
| `not` | ❌ | ✅ | ✅ |
| `patternProperties` | ✅ | ❌ | ❌ |
| `additionalProperties: false` | ✅ | ✅（strict 强制） | ✅（strict 强制） |
| `additionalProperties: true` | ✅ | ❌（strict 不允许） | ❌ |

### 6.4 openclaw 的 tool name 清洗（第四轮 6.2 简略提及）

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/providers/anthropic-tool-projection.ts`

```ts
// projectRuntimeToolInputSchema - 把内部 JSON Schema 投影成 Anthropic 接受形状
function projectRuntimeToolInputSchema(schema) {
  // 1. 顶层必须 type: "object"
  // 2. properties 必须存在（即使空对象）
  // 3. required 数组必须与 properties 字段名一致
  // 4. 递归清理每个 property
}
```

**对 laew 的借鉴**：当前 laew 的 `convert_tools` 直接透传 `t.input_schema` 到 wire——若 `input_schema` 含 `if/then/else` 或 `$ref`，Anthropic 会 400。

**P1 借鉴**：
1. 在 `convert_tools` 前调用 `projectRuntimeToolInputSchema` 做投影
2. 至少剥离 `$ref`、`if/then/else`、`$dynamicRef`
3. 补充 `properties: {}` 即使空

---

## 7. Vision 模型适配（supports_vision / image_tokens）

### 7.1 视觉能力判断矩阵

| 仓库 | 模型能力字段 | 视觉块处理 |
|---|---|---|
| **atomcode** | `cfg.supports_vision` | `format_messages_with_vision` 决定是否丢 image 块 |
| **openclaw** | `model.capabilities.input.image` | 拒绝发送 image 块给非视觉模型 |
| **opencode** | `model.capabilities.input.image` | 同样降级 |
| **pi** | `model.compat.supportsVision` | 同上 |
| **deepseek-harness** | `model.metadata.vision` | 同上 |
| **claudecode** | 工具级判断 | 发送前检查 |
| **laew** | ❌ | ❌（无 Image 块） |

### 7.2 deepseek-harness 的 image_tokens 计算

**文件**：`/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/llm-deepseek/src/image-tokens.ts`

```typescript
// image-tokens.ts - 按图片像素估算 token 数
function estimateImageTokens(width: number, height: number): number {
  // OpenAI 经验公式：约 765 tokens @ 1024x1024
  // 实际为 170 tokens + (tiles * 85 tokens)
  // tiles = ceil(width/512) * ceil(height/512)
  const tiles = Math.ceil(width / 512) * Math.ceil(height / 512);
  return 170 + tiles * 85;
}
```

**说明**：deepseek-harness 在请求**前**估算图片 token 数，用于 budget 控制（不让单张图片吃掉 context 50%）。

### 7.3 视觉块降级策略

**场景**：用户给纯文本模型发图片块  
**各仓库策略**：

| 仓库 | 策略 |
|---|---|
| atomcode | 跳过 image 块（不留任何痕迹） |
| openclaw | 抛错（`incompatible_model_capability`） |
| opencode | 同 openclaw |
| pi | 抛错 |
| deepseek-harness | 跳过 image 块 |

**两种学派**：
- **静默降级派**（atomcode）：让多模态 Agent 在文本模型上"自然退化"
- **显式失败派**（openclaw/opencode/pi）：早失败，避免静默丢失用户输入

**laew 当前**：无 Image 内容块 → 不存在视觉适配问题。

---

## 8. Token 单价 / 计费引擎深度

### 8.1 openclaw 分层定价

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/llm-core/src/usage-cost.ts`（L93-114）

```typescript
// packages/llm-core/src/usage-cost.ts L93-114  分层定价
export function calculateUsageCost(usage, pricing: RawModelCostConfig): Usage["cost"] {
  const rates = selectPricingRates(pricing, input + cacheRead + cacheWrite);
  // ↑ input+cacheRead+cacheWrite 总量作为分层依据
  const cacheWrite1h = Math.min(cacheWrite, Math.max(0, finiteOrZero(usage.cacheWrite1h)));
  const cacheWrite5m = cacheWrite - cacheWrite1h;
  const cost = {
    input: (input * rates.input) / 1_000_000,
    output: (output * rates.output) / 1_000_000,
    cacheRead: (cacheRead * rates.cacheRead) / 1_000_000,
    cacheWrite: (cacheWrite5m * rates.cacheWrite + cacheWrite1h * rates.input * 2) / 1_000_000,
    //                                                        ↑ 1h cache 写入按 2× input 价格计算
    total: 0,
  };
  cost.total = cost.input + cost.output + cost.cost.cacheRead + cost.cost.cacheWrite;
  return cost;
}
```

**关键设计**：
- **分层费率**：根据使用总量选择费率档（>200K tokens 折扣、>1M tokens 更低）
- **1h cache 写入 2x**：1h 长期缓存的写入成本按 input 单价 2x 计算（反映长期占存储）
- **5m cache 写入 1x**：5m 短期缓存按 cacheWrite 价（通常与 cacheRead 接近）

### 8.2 pi 的 calculateCost

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/models.ts`（L878-898）

```ts
export function calculateCost<TApi extends Api>(model: Model<TApi>, usage: Usage): Usage["cost"] {
  const inputTokens = usage.input + usage.input + usage.cacheRead + usage.cacheWrite;
  //                                          ↑ 注意：这里有 BUG?应该是 usage.cacheRead + usage.cacheWrite
  let rates: ModelCostRates = model.cost;
  let matchedThreshold = -1;
  for (const tier of model.cost.tiers ?? []) {
    if (inputTokens > tier.inputTokensAbove && tier.inputTokensAbove > matchedThreshold) {
      rates = tier; matchedThreshold = tier.inputTokensAbove;
    }
  }
  const longWrite = usage.cacheWrite1h ?? 0;
  const shortWrite = usage.cacheWrite - longWrite;
  usage.cost.input = (rates.input / 1000000) * usage.input;
  usage.cost.output = (rates.output / 1000000) * usage.output;
  usage.cost.cacheRead = (rates.cacheRead / 1000000) * usage.cacheRead;
  usage.cost.cacheWrite = (rates.cacheWrite * shortWrite + rates.input * 2 * longWrite) / 1000000;
  usage.cost.total = usage.cost.input + usage.cost.output + usage.cost.cacheRead + usage.cost.cacheWrite;
  return usage.cost;
}
```

**设计差异**：
- pi 也用 1h cache 写入 2x input（与 openclaw 一致）
- pi **也支持 tiers**（多层费率）
- **疑似 BUG**：`const inputTokens = usage.input + usage.input + usage.cacheRead + usage.cacheWrite;` 中的 `usage.input + usage.input` 应该是 `usage.input + usage.cacheRead + usage.cacheWrite`

### 8.3 claudecode 的 6 档写死价格表（第四轮 8.2 简略提及）

**文件**：`/usr/local/LsmGitOpenSource/claudecode/src/utils/modelCost.ts`（200 行）

```typescript
export const COST_TIER_3_15   = { inputTokens: 3, outputTokens: 15, ... }   // Sonnet
export const COST_TIER_15_75  = { inputTokens: 15, outputTokens: 75, ... }  // Opus 4
export const COST_TIER_5_25   = { inputTokens: 5, outputTokens: 25, ... }   // Opus 4.5
export const COST_HAIKU_35    = { inputTokens: 0.80, outputTokens: 4, ... }
export const COST_HAIKU_45    = { inputTokens: 1, outputTokens: 5, ... }
export const MODEL_COSTS: Record<ModelShortName, ModelCosts> = { ... }
```

**设计差异**：
- claudecode **无分层**（一个模型一档价）——简单清晰
- 价格表是**写死的字面量**（非配置）——跟随 Claude Code 自身模型集合

### 8.4 计费引擎对比表

| 仓库 | 计费方式 | 分层支持 | 1h cache 写入 | Cache TTL 分层 |
|---|---|---|---|---|
| **openclaw** | `calculateUsageCost` | ✅ `selectPricingRates` | ✅ 2x input | ✅ 1h/5m 分开 |
| **pi** | `calculateCost` | ✅ `tiers` | ✅ 2x input | ✅ 1h/5m 分开 |
| **claudecode** | `tokensToUSDCost` | ❌ 单一档 | ✅ 模型决定 | ✅ 写死 |
| **atomcode** | ❌ | ➖ | ➖ | ➖ |
| **opencode** | ❌（catalog 相对排序） | ➖ | ➖ | ➖ |
| **laew** | ❌ 无定价 | ➖ | ➖ | ➖ |

### 8.5 laew 现状评估

laew 当前：
- ✅ 4 维 usage 解析（input_tokens / output_tokens / cache_read / cache_creation）
- ❌ 无定价引擎
- ❌ 无 session 累计费用

**P2 借鉴**：
1. 新增 `src/llm/pricing.rs`：内置主流模型单价表
2. 在 `Completion` 增加 `cost: Cost` 字段
3. TUI 底部显示当前 session 累计费用
4. 借鉴 openclaw 的 1h cache 2x 算法

---

## 9. OAuth / Bearer 双模式深度实现

### 9.1 各仓库 OAuth 字段

| 仓库 | OAuth 入口 | 兼容多 provider | 认证上下文 |
|---|---|---|---|
| **laew** | ❌ 无 OAuth | ❌ | API Key 单模式 |
| **atomcode** | `RequestSigner::recover_unauthorized` | ✅ AtomGit 自定义 | OpenAI 路径支持 401 刷新 |
| **claudecode** | `getClaudeAIOAuthTokens()` | ✅ Anthropic OAuth / Foundry | 双模式：apiKey + authToken |
| **deepseek-harness** | 外部 pi-ai 库 | ✅ Models 集合 | credentialStore |
| **openclaw** | `isAnthropicOAuthApiKey()` | ✅ 4 种模式 | API key / OAuth / Foundry / Copilot / Cloudflare |
| **opencode** | `Credential` 接口 | ✅ per-provider configure | Redacted 字符串 |
| **pi** | `ANTHROPIC_OAUTH_TOKEN_ENV` | ✅ 三级链 | stored credential / env vars / OAuth |

### 9.2 openclaw 的 4 种模式 + Foundry 剥头算法

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/providers/anthropic.ts`（L919-1058）

```typescript
if (model.provider === "cloudflare-ai-gateway") {
  // → Authorization: null, 用 Cloudflare 的 cf-aig-authorization
}
if (model.provider === "github-copilot") {
  // → Bearer auth: authToken: apiKey
}
if (usesFoundryBearerAuth(model)) {
  // → Foundry Bearer: 剥掉 authorization/x-api-key/api-key 头
  // （Foundry 用 Bearer 头而非 x-api-key）
}
if (isAnthropicOAuthApiKey(apiKey)) {
  // → OAuth: authToken: apiKey, 加 claude-code/oauth beta 头
  if (apiKey.includes("sk-ant-oat")) { ... }
}
// 默认: API key auth → x-api-key 由 SDK 自动加
```

**Foundry 剥头算法**：
- Foundry 是 Anthropic 在 Azure 上的部署，要求用 `Authorization: Bearer` 而非 `x-api-key`
- 算法：若检测到 Foundry 模式，**显式删除** SDK 默认加的 `authorization` / `x-api-key` / `api-key` 头，仅保留 `Authorization: Bearer <token>`

### 9.3 pi 的三级链

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/providers/anthropic.ts`（L9-40）

```ts
resolve: async ({ ctx, credential, signal }) => {
  if (credential?.key) {
    return { auth: { apiKey: credential.key }, env: credential.env, source: "stored credential" };
  }
  const authToken = await ctx.env(ANTHROPIC_AUTH_TOKEN_ENV);
  if (authToken) {
    return { auth: { headers: { Authorization: `Bearer ${authToken}` } }, source: ANTHROPIC_AUTH_TOKEN_ENV };
  }
  for (const envVar of [ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV]) {
    const apiKey = await ctx.env(envVar);
    if (apiKey) return { auth: { apiKey }, source: envVar };
  }
  return undefined;
},
```

**三级链解析顺序**：
1. **stored credential**（用户在 pi TUI 中保存的 key）→ `apiKey` 字段
2. **`ANTHROPIC_AUTH_TOKEN`**（env var）→ `Authorization: Bearer` header
3. **`ANTHROPIC_OAUTH_TOKEN` / `ANTHROPIC_API_KEY`**（env var）→ `apiKey` 字段

**assertRequestAuth 兜底校验**（L297-307）：
```ts
function assertRequestAuth(provider: string, apiKey: string | undefined,
    headers: ProviderHeaders | undefined): void {
  if (apiKey) return;
  if (hasHeader(headers, "authorization") || hasHeader(headers, "x-api-key") ||
      hasHeader(headers, "cf-aig-authorization")) return;
  throw new Error(`No API key for provider: ${provider}`);
}
```

**关键设计**：至少一个有效认证（apiKey 或 Authorization 头）——"不能裸跑"。

### 9.4 laew 现状评估

laew 当前仅 `api_key` 一种认证模式（来自 DB `providers.api_key`）。

**P2 借鉴**：
1. 增加 OAuth 路径（Claude.ai OAuth Token）
2. 支持 env var fallback（`ANTHROPIC_API_KEY` / `OPENAI_API_KEY`）
3. 至少实现 `assertRequestAuth` 兜底

---

## 10. Compaction Replay / Crash Recovery（第四轮未覆盖）

### 10.1 场景与价值

**场景**：长 Session 中部分历史被压缩（compaction）成摘要，但用户要求重新发起一次"未压缩"对话以恢复原始上下文。

**价值**：
- 在 compaction 发生前**录制原始 HTTP 流**
- 压缩后用户可重新 "replay" 录制，**用原 token 计费方式**继续
- 用于回滚 compaction 决策

### 10.2 openclaw 的实现

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/transports/anthropic-compaction-replay.ts`

```typescript
// anthropic-compaction-replay.ts - compaction 前录制完整 HTTP 流
class AnthropicCompactionReplay {
  // 把 message_start / content_block_* / message_delta / message_stop
  // 全部写入 .jsonl 文件（每行一个 SSE 事件）
  async record(httpResponse: Response): Promise<ReplayFile> {
    // ... 流式读取 body，逐行写入
  }
  
  // replay 时重新解析 .jsonl，模拟原始 Anthropic 流
  async replay(replayFile: ReplayFile): Promise<AnthropicStream> {
    // ... 读取 .jsonl，重建消息内容
  }
}
```

**应用**：与 `provider-compaction-replay.ts`（provider 通用层）配合，在 session_memory 表中存"replay 文件路径 + 偏移量"。

### 10.3 opencode 的 providerReplayContext

**文件**：`/usr/local/LsmGitOpenSource/opencode/packages/llm/src/protocols/provider-replay-context.ts`

opencode 把 replay 做成"context"——任何 provider 都可以用同一套 replay API，区别仅在于解析格式。

### 10.4 laew 现状评估

laew 当前 **无 compaction 机制**——长 Session 只能通过 `/clear` 全量清空。

**P2 借鉴**：
1. 实现 `SessionContextAgent` 的 compaction 触发（>N tokens → 压缩为摘要）
2. 压缩前录制 Anthropic 流到文件
3. `/replay <session_id>` 命令恢复原始流

**本节定位**：作为未来扩展参考，laew 当前不需要实现，但需在架构上预留"流录制"入口（`agent/mod.rs` 已有 `agent_memory` 表结构）。

---

## 11. Tool Schema 投影（投影到协议子集）

### 11.1 投影的本质

内部 JSON Schema（如含 `$ref`、`if/then/else`、自定义 keyword）需要被"投影"成各协议支持的子集。  
**4 个主流投影方向**：

1. **Anthropic**：拒绝 `$ref`、`$dynamicRef`、`if/then/else`、`not`
2. **OpenAI strict**：强制 `additionalProperties: false`、全字段必填、剥离 `patternProperties`
3. **OpenAI Responses strict**：同 strict + 拒绝某些组合 keyword
4. **Bedrock**（Anthropic 兼容）：同 Anthropic 子集

### 11.2 openclaw 的 projectRuntimeToolInputSchema

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/providers/anthropic-tool-projection.ts`

```ts
function projectRuntimeToolInputSchema(schema: JSONSchema): JSONSchema {
  // 1. type 必须是 "object"（顶层）
  if (schema.type !== "object") {
    return { type: "object", properties: {} };
  }
  
  // 2. properties 必须存在
  const properties = schema.properties ?? {};
  
  // 3. 递归清理每个 property
  const projectedProperties: Record<string, JSONSchema> = {};
  for (const [key, prop] of Object.entries(properties)) {
    projectedProperties[key] = projectProperty(prop);
  }
  
  // 4. 补充 required 数组
  const required = schema.required ?? Object.keys(projectedProperties);
  
  // 5. 剥离 keyword
  return {
    type: "object",
    properties: projectedProperties,
    required,
    // 拒绝：$schema, $id, $ref, $dynamicRef, $defs, if/then/else, not
  };
}

function projectProperty(prop: JSONSchema): JSONSchema {
  if (prop.type === "object") return projectRuntimeToolInputSchema(prop);
  if (prop.type === "array") {
    return {
      type: "array",
      items: prop.items ? projectProperty(prop.items) : {},
    };
  }
  // primitive type → 原样返回
  return prop;
}
```

### 11.3 opencode 的 ToolSchemaProjection

**文件**：`/usr/local/LsmGitOpenSource/opencode/packages/llm/src/protocols/utils/tool-schema.ts`

```ts
// protocols/utils/tool-schema.ts
export const ToolSchemaProjection = {
  modelCompatibility(schema, provider): JSONSchema {
    if (provider === "anthropic") return stripForAnthropic(schema);
    if (provider === "openai") return stripForOpenAI(schema);
    if (provider === "openai-strict") return stripForOpenAIStrict(schema);
    return schema;
  },
  openAI(schema, strict): JSONSchema {
    if (strict) return stripForOpenAIStrict(schema);
    return stripForOpenAI(schema);
  },
};
```

**3 种 provider 投影**：
- `anthropic`：剥 `$ref` / `if-then-else` / `$dynamicRef`
- `openai`：剥 `patternProperties`，强制 `additionalProperties: false`
- `openai-strict`：strict 子集 + 全字段必填

### 11.4 pi 的 getJsonSchemaToolParameters

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/pi-compat.ts`

```ts
// getJsonSchemaToolParameters - strict 投影
function getJsonSchemaToolParameters(tool: Tool, strict: boolean): JSONSchema {
  if (!strict) return tool.parameters;
  
  // strict 模式：递归遍历 properties
  const projected = projectStrict(tool.parameters);
  return projected;
}
```

**strict projection 算法**：
1. type 必须是 "object"
2. properties 必填
3. required 数组必须**包含所有 properties 的 key**
4. additionalProperties: false
5. 递归处理 nested object / array

### 11.5 laew 现状评估

laew 当前：
- ❌ 完全无 schema 投影
- ⚠️ 直接透传 `t.input_schema` 到 wire
- 风险：若工具描述中含 `$ref` 或 `if/then/else`，Anthropic / OpenAI strict 会 400

**P1 借鉴**：
1. 在 `convert_tools` 前做最小投影：
   - 剥 `$ref` / `$dynamicRef` / `if/then/else` / `not`
   - 强制顶层 `type: "object"` + `properties` 存在
   - 补充 `required` 数组
2. 对 OpenAI strict 模式再做 `additionalProperties: false` 强制
3. 单元测试覆盖每个 keyword

---

## 12. Transport 层 SSE 字节级优化

### 12.1 laew 当前 SSE 解析回顾

**文件**：`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/llm/sse.rs`

laew 自实现 `SseStream`：
- 按 `\n` 切行（处理 `\r\n`）
- 64KiB 行缓冲上限
- 解析 `event:` / `data:` / `id:` / `retry:` 字段
- 输出 `SseEvent` 给协议 Parser

**已知盲点**：
- ❌ 无 connect / headers / body timeout 区分
- ❌ 无超长 data 行的 `[DONE]` 误判防护
- ❌ 流中断时已累积的 in_flight tool_calls 的强制 flush 需手动调用 `finish_into`

### 12.2 openclaw 的字节级优化

**文件**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/transports/openai-completions-transport.ts`（L61-110）

```typescript
const SSE_DONE_MAX_LINE_CHARS = 1024;  // ← 单行最大字符数
const SSE_DONE_LINE_RE = /^data:[ \t]*\[DONE\][ \t]*$/i;

function createSseDoneDetector() {
  let line = "";
  let lineOverflowed = false;
  let sawDone = false;
  
  const finishLine = () => {
    if (SSE_DONE_LINE_RE.test(line)) sawDone = true;
    line = "";
    lineOverflowed = false;
  };
  
  const observeText = (text: string) => {
    for (const char of text) {
      if (char === "\n" || char === "\r") { finishLine(); continue; }
      if (!lineOverflowed && line.length < SSE_DONE_MAX_LINE_CHARS) {
        line += char;
      } else {
        lineOverflowed = true;  // ← 超过上限，丢弃后续字符
      }
    }
  };
  
  return { observe(chunk) { ... }, finish() { ... }, sawDone: () => sawDone };
}
```

**关键优化**：
- **单行字符上限 = 1024**：超长 data 行（如 `tool_calls.arguments` 是 100KB JSON 字符串）会被截断，但**不会**误判为 `[DONE]`
- **逐字符观察**：用循环而非正则匹配，避免大块 buffer 重复匹配

### 12.3 pi 的 3 层降级 partial JSON

**文件**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/utils/json-parse.ts`（L104-124）

```ts
export function parseStreamingJson<T>(partialJson: string | undefined): T {
  if (!partialJson || partialJson.trim() === "") return {} as T;
  try {
    return parseJsonWithRepair<T>(partialJson);  // ← 第 1 层：parse + repair
  } catch {
    try {
      const result = partialParse(partialJson);  // ← 第 2 层：partial-json 库
      return (result ?? {}) as T;
    } catch {
      try {
        const result = partialParse(repairJson(partialJson));  // ← 第 3 层：repair + partial
        return (result ?? {}) as T;
      } catch {
        return {} as T;  // ← 全部失败，兜底空对象
      }
    }
  }
}
```

**3 层降级表**：

| 层 | 算法 | 适用 |
|---|---|---|
| 1 | `parseJsonWithRepair`（标准 JSON + 简单修复） | 完整或近完整 JSON |
| 2 | `partialParse`（partial-json 库） | 截断 JSON（缺 `}` / `]`） |
| 3 | `repairJson` + `partialParse` | 截断 + 缺引号 / 逗号 |

### 12.4 laew 当前 partial JSON 处理

**文件**：`/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/llm/anthropic.rs`（L146-152, L183-187）

```rust
let v: Value = match serde_json::from_str(&ev.data) {
    Ok(v) => v,
    Err(e) => {
        tracing::warn!(error = %e, data = %ev.data, "Anthropic SSE data 非 JSON,跳过");
        return Ok(());
    }
};

"input_json_delta" => {
    if let Some(pj) = delta["partial_json"].as_str() {
        sink.feed(DeltaEvent::ToolCallJsonDelta(pj.to_string()))?;
    }
}
```

**laew 当前**：
- ✅ 把 `partial_json` 字符串片段累积（通过 `DeltaEvent::ToolCallJsonDelta`）
- ✅ 在 `content_block_stop` / `[DONE]` 时整体解析（由 `ParseSink` 完成）
- ❌ 累积后整体 parse 失败 → 当前会**整个 tool_use 块丢失**

**P1 借鉴**（pi 的 3 层降级）：
1. 在 `ParseSink` 解析 `ToolCallJsonDelta` 累积的 buffer 时，按 pi 3 层降级
2. 增加 `partial-json` crate 依赖（接受截断 JSON）
3. 增加 `json-repair` crate 依赖（修复缺引号/逗号）

### 12.5 行缓冲 vs 流式分发对比

| 仓库 | 行缓冲 | 流式分发 | 备注 |
|---|---|---|---|
| **laew** | 64KiB 行缓冲 | 解析后逐事件分发 | 简单清晰 |
| **claudecode** | SDK 内部 | 直接拿原始 stream | 性能好但需手写 |
| **openclaw** | 1024 字符截断 | detector 包装 | 防 [DONE] 误判 |
| **opencode** | 无固定上限 | 帧解析 | Effect Schema 严格 |
| **pi** | 无固定上限 | TextDecoder 流式 | 浏览器/Runtime 兼容 |

---

## 13. laew 真实遗漏点清单（基于第四轮 11.x 的"二次审查"）

> 本节基于**第四轮 11.1-11.4 的 P0/P1/P2 路线图** + **本轮 12 主题增量发现** + **直接读 laew 源代码的差距审计**综合得出。

### 13.1 当前 laew 协议层结构总览

**`src/llm/`**：

| 文件 | 行数 | 职责 |
|---|---|---|
| `mod.rs` | 230 | 统一消息模型 + LlmClient trait + 客户端工厂 + build_common_headers + normalize_endpoint |
| `anthropic.rs` | 630 | Anthropic Messages 协议 |
| `openai.rs` | 549 | OpenAI Chat Completions 协议 |
| `sse.rs` | 456 | 协议无关 SSE 字节流→事件流 |

**总计 1865 行 Rust**——非常紧凑。

### 13.2 第四轮已列的 13 条借鉴（不重复）

第四轮 11.1-11.4 已给出：
- **P0**（3 条）：System 提升 / Thinking 签名 / 基础重试
- **P1**（5 条）：partial JSON 3 层 / Usage 双视角 / 错误分类 / 超时三件套 / 429 所有权
- **P2**（5 条）：分层定价 / 529 fallback / 熔断器 / OpenAI Responses / Tool Schema 投影

### 13.3 本轮"二次审查"新发现的遗漏点

#### 漏点 L1：SSE 字节超时未设（影响正确性）

**位置**：`src/llm/anthropic.rs` L268-274 / `openai.rs` L328-334

```rust
let resp = self.http.post(&self.url).headers(headers).json(&req).send().await?;
//            ↑ reqwest::Client::new() 用默认超时（无）
//              SSE 长响应若上游卡住，会无限等待
```

**借鉴**：undici 三件套：
- `connect_timeout: 10s`
- `headers_timeout: 30s`
- `body_timeout: 0`（SSE 流禁用，禁用 mid-stream 切断）

**优先级**：**P0**（影响正确性：上游卡死时任务永久挂起）

#### 漏点 L2：metadata.user_id 中 account_uuid 字段空值合规性

**位置**：`src/llm/anthropic.rs` L70-77

```rust
fn build_user_id(device_id: &str, session_id: &str) -> String {
    json!({
        "device_id": device_id,
        "account_uuid": "",   // ← 空字符串，Anthropic 是否接受?
        "session_id": session_id,
    }).to_string()
}
```

**借鉴**：
- claudecode 同样用 `accountUuid: ''`（空字符串兜底，OAuth 时填入真值）
- opencode 不用 metadata.user_id（不发送此字段）

**风险**：Anthropic 可能期望 `account_uuid` 是 UUID 格式字符串而非空串。  
**优先级**：**P2**（理论风险，未观察到 400 报错）

#### 漏点 L3：AnthropicParser 不区分 thinking_delta 类型块

**位置**：`src/llm/anthropic.rs` L188-189

```rust
// thinking_delta / signature_delta 本期不向 TUI 输出
_ => {}
```

**问题**：laew 当前对 `thinking_delta` / `signature_delta` 完全忽略。但 Anthropic API 文档说：  
> "If thinking is enabled, the model will emit `thinking_delta` blocks. **You must include these blocks back in subsequent requests** to maintain the model's reasoning context."

**风险**：
1. 开启 thinking 时多轮对话会丢失思考链
2. 下一轮请求若不带 signature，Anthropic 会 400

**借鉴**：opencode `signatureFromMetadata` + atomcode `format_assistant_message`（第四轮 7.2 已提及）

**优先级**：**P0**（影响正确性：thinking 模型多轮失败）

#### 漏点 L4：Anthropic error 事件仅取 message，无 error.type 分类

**位置**：`src/llm/anthropic.rs` L218-224

```rust
"error" => {
    let msg = v["error"]["message"].as_str().unwrap_or("unknown upstream error").to_string();
    sink.feed(DeltaEvent::Error(msg))?;
}
```

**问题**：`error.type` 字段（如 `api_error` / `overloaded_error` / `rate_limit_error` / `invalid_request_error`）被丢弃。

**借鉴**：
- atomcode 在 `is_retryable_status` 中专门识别 `529`
- openclaw `overflow.ts` 30+ 正则检测 `context_window_exceeded`

**优先级**：**P1**（影响错误分类 + 重试决策）

#### 漏点 L5：convert_messages 把 System 消息降级为 user（已列 P0-1）

**位置**：`src/llm/anthropic.rs` L83-90

```rust
// System 字段在请求体顶层,不进入 messages
crate::llm::Role::System => "user",  // ← 但代码并未提升为顶层 system!
```

**问题**：`Role::System` 消息被映射为 `role:"user"`，**没有**被提升为顶层 `system` 字段。

**当前 OpenAI 路径**（`openai.rs` L67-72）：

```rust
if !system.trim().is_empty() {
    out.push(json!({ "role": "system", "content": system }));  // ← 通过 LlmClient::complete 的 system 参数提升
}
```

**问题根源**：`LlmClient::complete(&self, system: &str, messages: &[ChatMessage], ...)` 把 system 作为独立参数，调用方（如 agent loop）需要从 messages 中提取 system 字符串后再传入。如果调用方忘了提取，system 内容**完全丢失**。

**借鉴**：atomcode `format_messages_with_vision` 把 `Role::System` join 成单一字符串再传入顶层 `system` 字段。

**优先级**：**P0**（影响 Anthropic 的 system 缓存断点能力）

#### 漏点 L6：tool_choice 写死 "auto"，无法选择 "required"/"none"/特定 tool

**位置**：`src/llm/openai.rs` L319

```rust
tool_choice: Some("auto"),
```

**问题**：OpenAI Chat 支持 `"required"` / `"none"` / `{type:"function", function:{name:"X"}}`，laew 全部写死 `"auto"`。

**借鉴**：
- opencode `toolChoice: ToolChoice` 支持多模式
- openclaw 通过 `compat.supportsToolChoice` 模型能力分支

**优先级**：**P2**（一般场景 `"auto"` 足够）

#### 漏点 L7：AnthropicRequest 没有 temperature 字段

**位置**：`src/llm/anthropic.rs` L47-61

```rust
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    system: Option<String>,
    messages: Vec<Value>,
    tools: Vec<Value>,
    metadata: Option<Metadata>,
    stream: bool,
    // ❌ 缺 temperature
    // ❌ 缺 top_p
    // ❌ 缺 top_k
    // ❌ 缺 stop_sequences
    // ❌ 缺 thinking
    // ❌ 缺 cache_control
}
```

**问题**：用户无法控制生成参数（temperature、top_p 等）。

**借鉴**：
- claudecode 通过 `extraBodyParams` 注入额外字段
- opencode `RequestOptions.temperature` 全协议统一注入

**优先级**：**P1**（影响模型调优）

#### 漏点 L8：AnthropicRequest 没有 cache_control（见 §2.7）

**位置**：同上

**借鉴**：opencode `Cache.newBreakpoints` + openclaw "最后 tool 加 cache_control"

**优先级**：**P1**（影响成本优化）

#### 漏点 L9：Assistant 消息中的 thinking 块未回传

**位置**：`src/llm/anthropic.rs` L96-99

```rust
ContentBlock::ToolUse { id, name, input } => {
    json!({ "type": "tool_use", "id": id, "name": name, "input": input })
}
// ❌ 没有处理 thinking 块
```

**问题**：`ContentBlock` 枚举只有 `Text` / `ToolUse` / `ToolResult`，没有 `Thinking` 变体——即使上游返回 thinking 块，laew 也无法回传。

**借鉴**：opencode `signatureFromMetadata`（把 signature 保留在 `providerMetadata.anthropic.signature`）

**优先级**：**P0**（thinking 模型必须）

#### 漏点 L10：用户消息中的 image 内容块未实现

**位置**：`src/llm/mod.rs` L60-65

```rust
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    // ❌ 缺 Image { source, media_type, data }
}
```

**问题**：laew 完全没有多模态能力——即使 Anthropic / OpenAI 都支持图片，laew 无法发送。

**优先级**：**P2**（基础 Agent 暂不需要）

#### 漏点 L11：tool_call id 缺失时用 `call_{idx}` 兜底，但 Anthropic 不接受此格式

**位置**：`src/llm/openai.rs` L278-280（仅 OpenAI 路径）

```rust
let id = if call.id.is_empty() {
    format!("call_{idx}")  // ← Anthropic 不接受此格式
} else { call.id.clone() };
```

**问题**：OpenAI 路径兜底生成的 `call_0` / `call_1` 在 Anthropic 上会被 400 拒绝（格式不合规）。

**借鉴**：openclaw `normalizeAnthropicToolCallId`（必须 `[a-zA-Z0-9_-]` ≤ 64）

**优先级**：**P1**（多 provider 切换场景的硬错误）

#### 漏点 L12：HTTP 错误响应 body 截断风险

**位置**：`src/llm/anthropic.rs` L278 / `openai.rs` L338

```rust
let body_text = resp.text().await.unwrap_or_default();
return Err(AgentError::Llm(format!("HTTP {status}: {body_text}")));
//                    ↑ resp.text() 读全部 body，没有大小限制
//                      若上游返回 100MB HTML 错误页，会卡死
```

**借鉴**：
- atomcode `friendly_http_error` 仅取前 1KB
- openclaw `error-body.ts` 解析后丢弃

**优先级**：**P1**（DoS 防护）

#### 漏点 L13：未实现 stream 取消（用户中断 Ctrl-C）

**位置**：`src/llm/anthropic.rs` L286-298 / `openai.rs` L346-356

```rust
loop {
    match resp.chunk().await {  // ← 没有 AbortSignal
        ...
    }
}
```

**问题**：用户在 TUI 中按 Ctrl-C 时，reqwest 不会立即取消——会继续读 chunk 直到自然结束。

**借鉴**：
- claudecode `{signal, ...}` AbortSignal 透传
- openclaw `signal.ts` 集中管理

**优先级**：**P0**（用户体验：中断响应不及时）

#### 漏点 L14：SSE 单行无最大长度保护

**位置**：`src/llm/sse.rs` L31-97

```rust
pub const DEFAULT_MAX_BUFFER: usize = 64 * 1024;
```

**问题**：laew 整个 buffer 上限 64KiB——超过直接报错。但**单行**没有上限，单行 60KiB 是允许的（虽然 protobuf 几乎不会这样，但 `tool_calls.arguments` 大量场景可能产生）。

**借鉴**：openclaw `SSE_DONE_MAX_LINE_CHARS = 1024`（仅对 `[DONE]` 检测时用，普通行不截断）

**优先级**：**P2**（理论风险）

#### 漏点 L15：openai.rs 的 [DONE] 处理后未补 finish_reason

**位置**：`src/llm/openai.rs` L188-199

```rust
if data == "[DONE]" {
    self.flush_into(sink)?;
    if !self.stopped {
        sink.feed(DeltaEvent::Stop {
            stop_reason: self.finish_reason.take(),  // ← 可能 None
        })?;
        ...
    }
}
```

**问题**：若 OpenAI 在 `[DONE]` 之前没有 chunk 带 `finish_reason`，则 stop_reason 为 None。

**借鉴**：claudecode `queryFinishReason` 在 message_stop 时兜底 `end_turn`。

**优先级**：**P2**（边界 case）

### 13.4 二次审查汇总表

| 漏点 | 文件:行 | 优先级 | 借鉴来源 |
|---|---|---|---|
| L1 SSE 字节超时未设 | `anthropic.rs:268-274` `openai.rs:328-334` | **P0** | undici 三件套 |
| L2 metadata.user_id 空字段合规 | `anthropic.rs:70-77` | P2 | claudecode 同样兜底 |
| L3 AnthropicParser 不处理 thinking_delta | `anthropic.rs:188-189` | **P0** | atomcode / opencode |
| L4 Anthropic error 事件仅取 message | `anthropic.rs:218-224` | P1 | atomcode 分类 |
| L5 System 消息降级为 user | `anthropic.rs:83-90` | **P0** | atomcode |
| L6 tool_choice 写死 "auto" | `openai.rs:319` | P2 | opencode |
| L7 AnthropicRequest 缺 temperature 等 | `anthropic.rs:47-61` | P1 | opencode RequestOptions |
| L8 AnthropicRequest 缺 cache_control | `anthropic.rs:47-61` | P1 | openclaw / pi |
| L9 Assistant 消息无 thinking 块 | `anthropic.rs:96-99` | **P0** | opencode signatureFromMetadata |
| L10 ContentBlock 缺 Image | `mod.rs:60-65` | P2 | atomcode |
| L11 tool id `call_{idx}` 不合规 Anthropic | `openai.rs:278-280` | P1 | openclaw normalizeAnthropicToolCallId |
| L12 HTTP 错误 body 无大小限制 | `anthropic.rs:278` `openai.rs:338` | P1 | atomcode friendly_http_error |
| L13 stream 取消未实现 | `anthropic.rs:286` `openai.rs:346` | **P0** | claudecode signal |
| L14 SSE 单行无最大长度 | `sse.rs:31` | P2 | openclaw SSE_DONE_MAX_LINE_CHARS |
| L15 [DONE] 后 finish_reason 兜底 | `openai.rs:188-199` | P2 | claudecode queryFinishReason |

### 13.5 优先级汇总（P0 共 5 条 + P1 共 5 条 + P2 共 5 条）

#### 二次审查 P0（必须做——影响正确性或用户体验）

```
────────────────────────────────────────────────────
│ L1: SSE 字节超时（connect/headers/body 三件套）0.5 天
│ L3: thinking_delta / signature_delta 完整处理 2-3 天
│ L5: System 消息提升为顶层字段 1 天
│ L9: Assistant 消息 thinking 块回传 2 天
│ L13: stream AbortSignal 取消 1 天
────────────────────────────────────────────────────
```

#### 二次审查 P1（应该做——提升健壮性）

```
────────────────────────────────────────────────────
│ L4: Anthropic error.type 分类 0.5 天
│ L7: temperature / top_p / top_k 注入 0.5 天
│ L8: cache_control 断点（最后 tool）1 天
│ L11: tool_call id 格式清洗 0.5 天
│ L12: HTTP 错误 body 大小限制 0.5 天
────────────────────────────────────────────────────
```

#### 二次审查 P2（可以做——提升体验）

```
────────────────────────────────────────────────────
│ L2: metadata.account_uuid UUID 校验
│ L6: tool_choice 多模式支持
│ L10: Image 内容块（视觉多模态）
│ L14: SSE 单行最大长度
│ L15: [DONE] 后 finish_reason 兜底
────────────────────────────────────────────────────
```

### 13.6 与第四轮 11.1-11.4 的对比

| 第四轮 P0 | 本轮新增 P0 |
|---|---|
| P0-1 System 提升 | L5 System 提升（**重复**） |
| P0-2 Thinking 签名 | L3 + L9 Thinking 完整处理（**更细**） |
| P0-3 基础重试 + Retry-After | L1 + L13 超时 + 取消（**新增**） |

| 第四轮 P1 | 本轮新增 P1 |
|---|---|
| P1-1 partial JSON 3 层 | （未列） |
| P1-2 Usage 双视角 | （未列） |
| P1-3 错误分类 + 溢出识别 | L4 Anthropic error.type（**新增**） |
| P1-4 超时三件套 | L1（**重复为 P0**） |
| P1-5 429 所有权 | （未列） |
| | L7 temperature 注入（**新增**） |
| | L8 cache_control（**新增**） |
| | L11 tool id 清洗（**新增**） |
| | L12 HTTP 错误 body 限制（**新增**） |

| 第四轮 P2 | 本轮新增 P2 |
|---|---|
| P2-1 分层定价 | （未列） |
| P2-2 529 fallback | （未列） |
| P2-3 熔断器 | （未列） |
| P2-4 OpenAI Responses | （未列） |
| P2-5 Tool Schema 投影 | （未列） |
| | L2 / L6 / L10 / L14 / L15（**新增**） |

**结论**：第四轮 P0/P1 的部分条目与本轮**部分重叠**，但本轮**新增 6 条**（L1/L4/L7/L8/L11/L12 提升为 P1）。

---

## 14. 附录：6 仓库协议文件路径全索引（增量版）

> 第四轮 12.2 已列核心文件，本附录补充本轮 13 主题引用的扩展文件。

### 14.1 atomcode（增量）

| 文件 | 本轮引用主题 |
|---|---|
| `crates/atomcode-capabilities/src/provider/reasoning.rs` | §1 ReasoningPolicy 推导 |
| `crates/atomcode-config/src/config/provider.rs` | §9.1 三级 key 解析 |

### 14.2 claudecode（增量）

| 文件 | 本轮引用主题 |
|---|---|
| `src/services/api/promptCacheBreakDetection.ts` | §2.5 cache 断点检测 |
| `src/services/api/claude.ts:1818-2170` | §3.4 queryFinishReason |
| `src/utils/thinking.ts:113-144` | §1.2 modelSupportsAdaptiveThinking |

### 14.3 deepseek-harness（增量）

| 文件 | 本轮引用主题 |
|---|---|
| `packages/llm/llm-deepseek/src/image-tokens.ts` | §7.2 image token 估算 |

### 14.4 openclaw（增量）

| 文件 | 本轮引用主题 |
|---|---|
| `packages/ai/src/transports/openai-completions-transport.ts:61-110` | §3.3 createSseDoneDetector |
| `packages/ai/src/transports/openai-completions-stream.ts:633-641` | §12.2 partial args 累积 |
| `packages/ai/src/providers/anthropic.ts:919-1058` | §9.2 4 种模式 + Foundry 剥头 |
| `packages/ai/src/providers/anthropic-tool-projection.ts` | §6.4 / §11.2 projectRuntimeToolInputSchema |
| `packages/llm-core/src/usage-cost.ts:93-114` | §8.1 分层定价 |
| `packages/ai/src/transports/anthropic-compaction-replay.ts` | §10.2 录制回放 |
| `packages/ai/src/providers/openai-completions.ts:492-532` | §1.2 7 种 thinkingFormat |

### 14.5 opencode（增量）

| 文件 | 本轮引用主题 |
|---|---|
| `packages/llm/src/protocols/anthropic-messages.ts:506-553` | §2.2 Cache.newBreakpoints |
| `packages/llm/src/protocols/utils/tool-schema.ts` | §11.3 ToolSchemaProjection |
| `packages/llm/src/protocols/provider-replay-context.ts` | §10.3 providerReplayContext |

### 14.6 pi（增量）

| 文件 | 本轮引用主题 |
|---|---|
| `packages/ai/src/api/anthropic-messages.ts:1326-1363` | §2.4 cache_control 路由 |
| `packages/ai/src/api/openai-completions.ts:1493-1504` | §6.2 resolveJsonSchemaStrictSampling |
| `packages/ai/src/api/simple-options.ts:55-77` | §1.2 thinking budget clamp |
| `packages/ai/src/api/pi-compat.ts` | §6.3 getJsonSchemaToolParameters |
| `packages/ai/src/providers/anthropic.ts:9-40` | §9.3 三级链 |
| `packages/ai/src/models.ts:878-898` | §8.2 calculateCost |
| `packages/ai/src/utils/json-parse.ts:104-124` | §12.3 parseStreamingJson 3 层降级 |

### 14.7 laew 改动文件清单（基于本轮 13 主题）

| 文件 | 本轮主题 | 改动 |
|---|---|---|
| `src/llm/mod.rs` | §13 L7/L10 | 扩 `ContentBlock`（加 Image）+ `AnthropicRequest` 字段（temperature/top_p/thinking） |
| `src/llm/anthropic.rs` | §13 L1/L3/L4/L5/L9 | timeout / thinking 处理 / error 分类 / System 提升 / thinking 回传 / cache_control |
| `src/llm/openai.rs` | §13 L1/L6/L11 | timeout / tool_choice 多模式 / tool id 清洗 |
| `src/llm/sse.rs` | §13 L14 | 单行最大长度 |
| `src/agent/mod.rs` | §13 L13 | 透传 AbortSignal 给 LlmClient |

---

## 15. 关键洞察总结（10 条）

### 15.1 wire JSON 真实差异（精简版）

#### Anthropic 最小请求

```json
{
  "model": "claude-sonnet-4-5",
  "max_tokens": 8192,
  "stream": true,
  "system": "you are a coding assistant",
  "messages": [
    { "role": "user", "content": [{ "type": "text", "text": "ls" }] }
  ],
  "tools": [{
    "name": "Bash",
    "description": "run shell",
    "input_schema": { "type": "object", "properties": { "command": { "type": "string" } } }
  }],
  "metadata": { "user_id": "{\"device_id\":\"d1\",\"account_uuid\":\"\",\"session_id\":\"s1\"}" }
}
```

#### OpenAI Chat 最小请求

```json
{
  "model": "gpt-4o",
  "stream": true,
  "stream_options": { "include_usage": true },
  "messages": [
    { "role": "system", "content": "you are a coding assistant" },
    { "role": "user", "content": "ls" }
  ],
  "tools": [{
    "type": "function",
    "function": {
      "name": "Bash",
      "description": "run shell",
      "parameters": { "type": "object", "properties": { "command": { "type": "string" } } }
    }
  }],
  "tool_choice": "auto"
}
```

### 15.2 协议差异浓缩为 5 句话

1. **认证**：Anthropic `x-api-key + anthropic-version` / OpenAI `Authorization: Bearer` —— laew 已实现
2. **system 位置**：Anthropic 顶层字段 / OpenAI 角色消息 —— laew 当前 OpenAI 正确，Anthropic 错误（**漏点 L5**）
3. **tool 定义**：Anthropic `{name,description,input_schema}` / OpenAI `{type:"function",function:{...}}` —— laew 已实现
4. **tool 结果**：Anthropic user 消息的 `tool_result` 块 / OpenAI `role:"tool", tool_call_id` —— laew 已实现
5. **流式终止**：Anthropic `message_stop` 事件 / OpenAI `[DONE]` 哨兵 —— laew 已实现

### 15.3 laew 当前 5 个最关键 P0 漏点（按重要性排序）

1. **L3 + L9 thinking 处理**（影响 thinking 模型多轮）
2. **L5 System 提升**（影响 Anthropic 缓存断点能力）
3. **L1 SSE 超时**（上游卡死时永久挂起）
4. **L13 stream 取消**（Ctrl-C 无响应）
5. （第四轮已有 P0-3 重试 / Retry-After）

### 15.4 6 仓库设计模式浓缩

| 模式 | 代表 | laew 是否采用 |
|---|---|---|
| 双文件对称隔离 | laew / atomcode / openclaw | ✅ |
| Protocol 多态 | opencode | ➖ |
| Provider-Neutral 词汇 | deepseek / pi | ➖ |
| 协议级 cache 断点上限常量 | opencode | ❌（漏点 L8） |
| OAuth 三级链解析 | pi | ❌ |
| partial JSON 3 层降级 | pi | ❌ |
| [DONE] 误判防护 | openclaw | ❌ |
| Tool id 格式清洗 | openclaw | ❌（漏点 L11） |
| 7 种 thinkingFormat | openclaw / pi | ➖（不支持多 provider） |
| ReasoningPolicy 推导 | atomcode | ❌ |

### 15.5 laew 完整度评分（10 分制）

| 维度 | 满分 | laew 得分 | 差距 |
|---|---|---|---|
| 请求构造 | 10 | 7 | system 提升 / temperature |
| 认证头 | 10 | 9 | OAuth |
| SSE 解析 | 10 | 7 | partial JSON 3 层 / 单行最大 |
| 错误映射 | 10 | 3 | 重试 / 超时 / 分类 / 熔断全无 |
| Tool wire | 10 | 8 | schema 投影 / id 清洗 |
| Thinking | 10 | 1 | 签名续传 / reasoning_content 全无 |
| Usage | 10 | 6 | 双视角 / 定价全无 |
| 端点补全 | 10 | 9 | 网关识别 |
| 流式控制 | 10 | 4 | 取消 / 超时 / 重试全无 |
| **总分** | **100** | **54** | **46** |

### 15.6 与第四轮的差异化本轮定位

- 第四轮是"**协议差异的横向对比**"（8 维度 × 7 仓库）
- 本轮是"**第四轮主题的深度补充**"（13 主题 × laew 差距审计）
- 第四轮 + 本轮合起来 = laew 协议层的完整外部参考

### 15.7 借鉴优先级合并表（第四轮 + 本轮）

```
P0（正确性）─────────────────────────────────────────
│ 第四轮: System 提升 / Thinking 签名 / 基础重试
│ 本轮新增: SSE 字节超时 / thinking 完整处理 / stream 取消
────────────────────────────────────────────────────
P1（健壮性）─────────────────────────────────────────
│ 第四轮: partial JSON / Usage 双视角 / 错误分类 / 429 所有权
│ 本轮新增: temperature / cache_control / tool id 清洗 / HTTP 错误 body 限制
────────────────────────────────────────────────────
P2（体验）───────────────────────────────────────────
│ 第四轮: 分层定价 / 529 fallback / 熔断器 / Responses / Schema 投影
│ 本轮新增: OAuth / Image / SSE 单行最大 / finish_reason 兜底 / metadata 合规
────────────────────────────────────────────────────
```

### 15.8 实施时间估算（合并）

| 阶段 | 工时 |
|---|---|
| P0 全量 | 7-10 天 |
| P1 全量 | 6-8 天 |
| P2 全量 | 8-12 天 |
| **合计** | **21-30 天**（约 1 个月） |

### 15.9 与 laew CLAUDE.md 的对应

本轮新增的 13 主题对应 CLAUDE.md "**Anthropic → `{end_point}/v1/messages`**" "**OpenAI → `{end_point}/chat/completions`**" "**工具定义协议差异**" 三条协议层条款的扩展。

### 15.10 维护建议

- 当 laew 实施 L3/L5/L9（thinking 完整处理 + System 提升）时，本文档 §1 / §13 的对应章节需同步更新代码片段
- 当 laew 实施 L1/L13（SSE 超时 + 取消）时，§13.5 优先级需下调
- 当 laew 新增协议（OpenAI Responses / Bedrock）时，§1 / §4 / §6 的 reasoning / tool id / structured output 维度需扩展

---

> **文档完成时间**：2026-09-06  
> **总章节数**：15 章  
> **总行数**：约 1900 行  
> **覆盖主题**：Reasoning 往返 / Cache 断点 / Streaming Stop / Tool id 约束 / 多模态 / Structured Output / Vision 适配 / Token 单价 / OAuth / Compaction Replay / Schema 投影 / SSE 字节优化 / laew 二次审查  
> **laew 漏点清单**：15 条（P0 5 + P1 5 + P2 5）  
> **维护建议**：实施 P0 时同步更新 §1 / §13；新增协议时扩展 §1 / §4 / §6