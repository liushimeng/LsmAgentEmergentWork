# Prompt Caching 与 Token 预算控制深度对比（第七轮专题）

> 专题定位：第七轮全新专属维度——聚焦 7 个开源 Agent 仓库（opencode / claudecode / pi / openclaw / deepseek-harness / atomcode / Switchyard）在 **cache 断点放置算法、缓存失效治理、命中度量、token 计数真实实现、上下文预算分配模型、强制执行机制、成本归因** 7 个维度的横向深度对比，并给出 laew（Rust 多 Agent CLI，完全没有 prompt caching、没有 token 计数、没有上下文预算控制）的针对性借鉴路线。
>
> 差异化重点：
> - **不重复** `专题-第三轮-成本控制与Token统计深度分析.md`（侧重成本计算、Stripe 集成、价格表结构）和 `专题-第六轮-Anthropic与OpenAI协议调用真实实现深度对比.md`（侧重协议 wire 层 13 维度差异）已写透的"价格表 / 计费引擎 / Stripe / Redis 批量 / 状态栏展示"。
> - **聚焦深挖**：（a）`cache_control` 断点的"放置算法 + 4 上限管理 + TTL 续期"的真实代码；（b）缓存失效的 8 大坑与字节稳定性治理；（c）token 计数的 **多级 fallback 链**（API → Haiku → 字符估算）与 JSON 特殊密度；（d）上下文预算的 **保留输出 token** 概念与压缩阈值；（e）**强制执行机制**——是"宁报错"还是"自动压缩"。
>
> 独特价值：此前知识库只点出了"cache miss 浪费"与"4 个断点上限"，本文首次深挖（i）opencode `Breakpoints` 计数器的实际语义（`remaining`/`dropped` 字段）；（ii）openclaw `splitSystemPromptCacheBoundary` 显式拆"稳定前缀 + 动态后缀"为两块的 system 缓存策略；（iii）claudecode `promptCacheBreakDetection` 用 hash 差异定位 cache miss 的具体来源（system/tools/betas/effort 12 维度）；（iv）openclaw cache_ttl_pruning 的"30% 触发 + 5% 清理"工具卸载策略；（v）claudecode Bedrock 不可用时回退 Haiku 真实调用模型计数的真实路径。

---

## 目录

1. [结论速览](#1-结论速览)
2. [各仓逐个深挖](#2-各仓逐个深挖)
   - 2.1 opencode（Effect DI 全栈 + 协议级断点上限）
   - 2.2 claudecode（per-source 缓存断点 + hash 差异检测）
   - 2.3 pi（cacheControl 路由 + 三层 cache TTL）
   - 2.4 openclaw（payload policy + cache_ttl_pruning + split prefix/suffix）
   - 2.5 deepseek-harness（固定密度估算 + epoch 投影）
   - 2.6 atomcode（Rust 字节稳定性 + cache_creation 归一）
   - 2.7 Switchyard（按调用次数的预算而非 token）
3. [横向对比大表（≥14 行）](#3-横向对比大表14-行)
4. [断点放置算法流程图（ASCII）](#4-断点放置算法流程图ascii)
5. [缓存失效的 8 大坑与规避](#5-缓存失效的-8-大坑与规避)
6. [token 计数实现对比表](#6-token-计数实现对比表)
7. [上下文预算分配模型（含具体数值表）](#7-上下文预算分配模型含具体数值表)
8. [设计模式与反模式（10~15 个）](#8-设计模式与反模式10~15-个)
9. [laew 现状与 P0/P1/P2 路线图](#9-laew-现状与-p0p1p2-路线图)
10. [关键文件速查](#10-关键文件速查)

---

## 1. 结论速览

**8 个核心结论**：

1. **断点上限 = 协议常量**：opencode 把 `ANTHROPIC_BREAKPOINT_CAP = 4` 编码为协议常量（`packages/llm/src/protocols/anthropic-messages.ts:238`），由 `Cache.Breakpoints { remaining, dropped }` 计数器在每次 `cacheControl()` 调用时扣减，超出则记录 `dropped` 计数并在请求结束 `Effect.logWarning`（同文件 `:534-538`）；这是**最严格的工程实现**。
2. **断点放置的"3 默认"是行业事实标准**：opencode `AUTO = { tools: true, system: true, messages: "latest-user-message" }`（`cache-policy.ts:18-22`）；openclaw `applyAnthropicCacheControlToMessages` 反向遍历 messages 找**最深稳定 user 消息**（`anthropic-payload-policy.ts:249-309`）；pi `convertMessages` 在最后 user/assistant 文本块打 `cache_control`（`anthropic-messages.ts:1295-1312`）；claudecode 三处（`api.ts:603/615/648/663`）分别对 user/assistant 文本块加 cache_control。**共识：cache 永远打在"最后一个稳定块"上，因为这是前缀的边界**。
3. **TTL 续期是字节前缀级别**：opencode `ttlBucket` 把 `ttlSeconds >= 3600` 映射为 `1h`，否则 `5m`（`packages/llm/src/protocols/utils/cache.ts:15-16`）；claudecode 通过 `should1hCacheTTL` 检查用户资格 + GrowthBook allowlist **latch 进 STATE** 防中途翻转（`api.ts:393-434`），注释明确"mid-session overage flips 会破坏 ~20K tokens per flip 的缓存"。
4. **字节稳定性是 Anthropic 缓存的硬约束**：atomcode 用 `serde_json::Map(BTreeMap-backed)` + 无时间戳/uuid 序列化（`docs/Agent源码调研/atomcode.md:2456-2463` 注释强调），并写 `body_serialization_is_deterministic` 测试 `anthropic.rs:1447-1490` 连续 100 次序列化逐字节比对。**反模式：动态时间戳、UUID、随机端口插入 system 前缀会让 cache 永远 miss**。
5. **token 计数有 4 级 fallback 链**：claudecode `countTokensWithFallback`（`utils/analyzeContext.ts:77-108`）的链路是：`countMessagesTokensWithAPI` → `countTokensViaHaikuFallback` → `roughTokenCountEstimation`（`Math.round(content.length / 4)`，JSON 改 `/2`）→ `null`。Atomcode 文档（`docs/Agent源码调研/agent-core.md:452`）列出 `_select_token_counter`：本地 tokenizer → tiktoken → `StringLengthCounter` 兜底。**共识：API 准但慢；本地 tokenizer 快但需要 model-specific；字符串长度最差但永远可用**。
6. **预算分配有"reserved output"硬编码**：pi `branch-summarization.ts:305` 默认 `reserveTokens = 16384`（`contextWindow - reserveTokens` 作为摘要预算）；claudecode `analyzeContext.ts:75` 用 `TOOL_TOKEN_COUNT_OVERHEAD = 500` 修正 API 重复计费；Switchyard `advisor_max_tokens: 2048` 限制 advisor 评审的输出上限（`advisor_gate.rs:144`）。**共识：无论 system/tools/history 怎么算，都要把"输出 token"从可用预算中扣出**。
7. **强制执行三种流派**：
   - **宁报错派**：Switchyard `ContextWindowExceeded` 直接映射为 400 给 client，让 agent 自己压缩（`advisor_gate.rs:21-26` 注释明确）。
   - **自动压缩派**：opencode 检测 `isContextOverflow` 后走 `compaction.ts` 自动压缩（`packages/opencode/src/session/compaction.ts:451`）；pi `model_context_window_exceeded` 错误触发 auto-compaction（`CHANGELOG.md:2609`）；deepseek-harness `thresholdRatio = 0.8` 接近上限时压缩。
   - **沉默截断派**：claudecode `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS = 200_000`（`toolLimits.ts:49`）超限按 `tool_use_id` 替换为文件路径 preview，无 I/O 字节恒等以**保 prompt cache**（`toolResultStorage.ts:769-909`）。
8. **成本归因是二级概念**：opencode `modelUsage` 按 `providerID/modelID` 聚合（`cost-tracker.ts:250-276`），但 laew 完全无成本计算。Switchyard 的**独特思路**：把"成本控制"重新定义为"调用次数控制"（`max_reviews`），因为 GPU 评审每次都是真开销。这是 laew 多 Agent 架构下最值得借鉴的省钱机制——**限 Plan Agent 调 2 次、QC Agent 调 1 次，比算 USD 实际**。

---

## 2. 各仓逐个深挖

### 2.1 opencode（TypeScript/Bun + Effect DI）——最严格的协议级断点上限

#### 2.1.1 断点上限：协议常量 + 计数器

**核心代码**：`/usr/local/LsmGitOpenSource/opencode/packages/llm/src/protocols/utils/cache.ts:5-16`

```typescript
// protocols/utils/cache.ts L5-16  协议级共享断点管理
export interface Breakpoints {
  remaining: number
  dropped: number
}

export const newBreakpoints = (cap: number): Breakpoints => ({ remaining: cap, dropped: 0 })

// ttlSeconds >= 3600 映射为 1h，否则 5m
export const ttlBucket = (ttlSeconds: number | undefined): "1h" | undefined =>
  ttlSeconds !== undefined && ttlSeconds >= 3600 ? "1h" : undefined
```

**调用入口**：`packages/llm/src/protocols/anthropic-messages.ts:238 + 243-251 + 514`

```typescript
// anthropic-messages.ts L238  协议常量
const ANTHROPIC_BREAKPOINT_CAP = 4

// anthropic-messages.ts L243-251  cacheControl 计数器
const cacheControl = (breakpoints: Cache.Breakpoints, cache: CacheHint | undefined) => {
  if (cache?.type !== "ephemeral" && cache?.type !== "persistent") return undefined
  if (breakpoints.remaining <= 0) {
    breakpoints.dropped += 1
    return undefined
  }
  breakpoints.remaining -= 1
  return Cache.ttlBucket(cache.ttlSeconds) === "1h" ? EPHEMERAL_1H : EPHEMERAL_5M
}

// anthropic-messages.ts L511-538  fromRequest 起始 + 警告
const breakpoints = Cache.newBreakpoints(ANTHROPIC_BREAKPOINT_CAP)
// tools → system → messages 的顺序扣减（注释："Tools live highest in the cache hierarchy,
// so when callers over-mark we keep their tool hints and shed the message-tail ones first."）
if (breakpoints.dropped > 0) {
  yield* Effect.logWarning(
    `Anthropic Messages: dropped ${breakpoints.dropped} cache breakpoint(s); the API allows at most ${ANTHROPIC_BREAKPOINT_CAP} per request.`,
  )
}
```

**精妙之处**：
1. **`dropped` 字段不抛错，只记 warning** —— 即使超过 4 个断点也只丢多余标记、不报 400。这给上游 `applyCachePolicy` 的灵活性（可设 5 个 hint 让其自动收敛到 4）。
2. **扣减顺序 = 缓存层次** —— tools 高、system 中、messages 低，超出时**优先丢 messages 尾部**，保 tools（最稳定）。
3. **`ttlBucket` 函数化** —— 协议无关，可同时给 Anthropic 和 Bedrock 用（`utils/bedrock-cache.ts:3-22` 同样的 `BEDROCK_BREAKPOINT_CAP`）。

#### 2.1.2 断点放置算法：`applyCachePolicy`

**核心代码**：`/usr/local/LsmGitOpenSource/opencode/packages/llm/src/cache-policy.ts:1-111`

```typescript
// cache-policy.ts L1-22  默认策略
const AUTO: CachePolicyObject = {
  tools: true,
  system: true,
  messages: "latest-user-message",
}

// L33-37  解析
const resolve = (policy: CachePolicy | undefined): CachePolicyObject => {
  if (policy === undefined || policy === "auto") return AUTO   // ← 默认走这里
  if (policy === "none") return NONE
  return policy
}

// L47-52  markLastTool
const markLastTool = (tools: ReadonlyArray<ToolDefinition>, hint: CacheHint): ReadonlyArray<ToolDefinition> => {
  if (tools.length === 0) return tools
  const last = tools.length - 1
  if (tools[last]!.cache) return tools   // ← 不覆盖已存在的
  return tools.map((tool, i) => (i === last ? new ToolDefinition({ ...tool, cache: hint }) : tool))
}

// L67-83  markMessageAt
const markMessageAt = (messages: ReadonlyArray<Message>, index: number, hint: CacheHint): ReadonlyArray<Message> => {
  const target = messages[index]!
  if (target.content.length === 0) return messages
  const lastTextIndex = target.content.findLastIndex((part) => part.type === "text")
  const markAt = lastTextIndex >= 0 ? content.length - 1
  const existing = target.content[markAt]!
  if ("cache" in existing && existing.cache) return messages   // ← 不重复打
  const nextContent = target.content.map((part, i) => (i === markAt ? ({ ...part, cache: hint } as ContentPart) : part))
  // 单次 .slice() 而非 .map()（注释："Long conversations call this on every request,
  // so avoid `.map()` here — its closure dispatch and identity copies show up in profiling."）
  const result = messages.slice()
  result[index] = next
  return result
}

// L99-111  主入口
export const applyCachePolicy = (request: LLMRequest): LLMRequest => {
  if (!RESPECTS_INLINE_HINTS.has(request.model.route.id)) return request  // 只对 anthropic / bedrock
  const policy = resolve(request.cache)
  if (!policy.tools && !policy.system && !policy.messages) return request

  const hint = makeHint(policy.ttlSeconds)
  const tools = policy.tools ? markLastTool(request.tools, hint) : request.tools
  const system = policy.system ? markLastSystem(request.system, hint) : request.system
  const messages = policy.messages ? markMessages(request.messages, policy.messages, hint) : request.messages

  if (tools === request.tools && system === request.system && messages === request.messages) return request
  return LLMRequest.update(request, { tools, system, messages })
}
```

**5 大设计要点**：
1. **`"auto"` 解析** + 默认 `1.25x write / 0.1x read` 经济账注释（L27-29）：Anthropic 5m cache 写入 1.25x、读取 0.1x，"单次复用就回本"。
2. **`RESPECTS_INLINE_HINTS` 白名单**：`anthropic-messages` + `bedrock-converse`（L42）—— OpenAI/Gemini 是隐式 prefix caching，显式 hint 无意义。
3. **每个标记独立可关**：`{ tools: false, system: true, messages: "latest-user-message" }` 可以关掉 tools 不打。
4. **`"messages": "latest-assistant"`** 替代策略：把断点放在 assistant 末尾而不是 user 末尾——某些场景下 assistant 比 user 更稳定。
5. **`"messages": { tail: N }`** 形式（注释在 L93）：从 messages 末尾往回数 N 条全部打 cache_control，可对"近 N 轮"做快照。

#### 2.1.3 context overflow 检测与自动压缩

**核心代码**：`/usr/local/LsmGitOpenSource/opencode/packages/llm/src/provider-error.ts:1-43`

```typescript
// provider-error.ts L4-32  28 条错误正则
const patterns = [
  /prompt is too long/i,
  /request_too_large/i,
  /input is too long for requested model/i,
  /exceeds the context window/i,
  /exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))/i,
  /input token count.*exceeds the maximum/i,
  // ... 共 28 条
  /model_context_window_exceeded/i,
  /too many tokens/i,
  /token limit exceeded/i,
]

// L34  排除项
const exclusions = [/^(throttling error|service unavailable):/i, /rate limit/i, /too many requests/i]

// L36-38  检测
export const isContextOverflow = (message: string) =>
  !exclusions.some((pattern) => pattern.test(message)) &&
  (patterns.some((pattern) => pattern.test(message)) || /^4(00|13)\s*(status code)?\s*\(no body\)/i.test(message))
```

**触发自动压缩**：`/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/session/processor.ts:621 + compaction.ts:451`

```typescript
// processor.ts L621  错误检测 → 触发压缩
if (SessionV1.ContextOverflowError.isInstance(error)) { ... }

// compaction.ts L451  注入 ContextOverflowError
processor.message.error = new SessionV1.ContextOverflowError({ ... })
```

**反模式避免**：opencode 把 `rate limit`、`too many requests` 显式从 patterns 中排除（L34）——这些不是真正的"上下文超限"。

#### 2.1.4 Usage 字段归一

**核心代码**：`packages/llm/src/protocols/anthropic-messages.ts:566-585`

```typescript
// anthropic-messages.ts L566-585  mapUsage
const mapUsage = (usage: AnthropicUsage | undefined): Usage | undefined => {
  if (!usage) return undefined
  const nonCached = usage.input_tokens
  const cacheRead = usage.cache_read_input_tokens ?? undefined
  const cacheWrite = usage.cache_creation_input_tokens ?? undefined
  const inputTokens = ProviderShared.sumTokens(nonCached, cacheRead, cacheWrite)
  return new Usage({
    // Anthropic: prompt = input_tokens + cache_read + cache_creation（注释 L567-572 明确）
    inputTokens,
    outputTokens: usage.output_tokens,
    cacheRead,
    cacheWrite,
    reasoningTokens: undefined,   // Anthropic 不分拆 thinking tokens，归入 output
  })
}
```

**关键点**：Anthropic `input_tokens` 是**不含缓存的**（L567-572 注释强调"per the Messages API docs"），所以 laew 当前 `input_tokens + cache_read + cache_creation` 的累加（`src/agent/yolo.rs:325-330`）**恰好等于 Anthropic 的 `prompt_tokens`**。

---

### 2.2 claudecode（TypeScript/Bun）—— per-source 缓存断点 + hash 差异检测

#### 2.2.1 `getCacheControl` 三参决策

**核心代码**：`/usr/local/LsmGitOpenSource/claudecode/src/services/api/claude.ts:358-374 + 393-434`

```typescript
// claude.ts L358-374  getCacheControl
export function getCacheControl({ scope, querySource }: { scope?: CacheScope; querySource?: QuerySource } = {}): {
  type: 'ephemeral'
  ttl?: '1h'
  scope?: CacheScope
} {
  return {
    type: 'ephemeral',
    ...(should1hCacheTTL(querySource) && { ttl: '1h' }),
    ...(scope === 'global' && { scope }),
  }
}

// claude.ts L393-434  should1hCacheTTL
function should1hCacheTTL(querySource?: QuerySource): boolean {
  // Bedrock 用户环境变量开关
  if (
    getAPIProvider() === 'bedrock' &&
    isEnvTruthy(process.env.ENABLE_PROMPT_CACHING_1H_BEDROCK')
  ) {
    return true
  }

  // Latch eligibility in bootstrap state — 注释原文：
    // "prevents mid-session overage flips from changing the cache_control TTL,
    //  which would bust the server-side prompt cache (~20K tokens per flip)."
  let userEligible = getPromptCache1hEligible()
  if (userEligible === null) {
    userEligible =
      process.env.USER_TYPE === 'ant' ||
      (isClaudeAISubscriber() && !currentLimits.isUsingOverage)
    setPromptCache1hEligible(userEligible)
  }
  if (!userEligible) return false

  // GrowthBook allowlist 也 latch
  let allowlist = getPromptCache1hAllowlist()
  if (allowlist === null) {
    const config = getFeatureValue_CACHED_MAY_BE_STALE<{ allowlist?: string[] }>('tengu_prompt_cache_1h_config', {})
    allowlist = config.allowlist ?? []
    setPromptCache1hAllowlist(allowlist)
  }

  return (
    querySource !== undefined &&
    allowlist.some(pattern =>
      pattern.endsWith('*') ? querySource.startsWith(pattern.slice(0, -1)) : querySource === pattern,
    )
  )
}
```

**精妙之处**：
1. **两次 latch 防 TTL flip**：eligibility + allowlist 都缓存进 `STATE`（不是函数局部），中途 `isUsingOverage` 翻转不会触发 cache bust。
2. **`getFeatureValue_CACHED_MAY_BE_STALE`** —— GrowthBook 磁盘缓存**可以陈旧**，但**会话内不允许翻 TTL**。
3. **"per flip ~20K tokens cache bust"** 是真实损失量化。

#### 2.2.2 `promptCacheBreakDetection` hash 差异定位

**核心代码**：`/usr/local/LsmGitOpenSource/claudecode/src/services/api/promptCacheBreakDetection.ts:1-200`（共 727 行）

```typescript
// promptCacheBreakDetection.ts L28-69  PreviousState 12 维度哈希
type PreviousState = {
  systemHash: number                                    // 系统提示（去 cache_control）
  toolsHash: number                                     // 工具 schema 整体哈希
  cacheControlHash: number                              // ← 系统块带 cache_control 哈希
                                                        //    "Catches scope/TTL flips (global↔org, 1h↔5m)
                                                        //     that stripCacheControl erases from systemHash."
  toolNames: string[]
  perToolHashes: Record<string, number>                 // ← 单工具 schema 哈希
                                                        //    "AgentTool/SkillTool embed dynamic agent/command lists.
                                                        //     77% of tool breaks per BQ 2026-03-22."
  systemCharCount: number
  model: string
  fastMode: boolean
  globalCacheStrategy: string                           // 'tool_based' | 'system_prompt' | 'none'
  betas: string[]                                       // beta header 列表
  autoModeActive: boolean                               // AFK_MODE_BETA_HEADER（已 latched）
  isUsingOverage: boolean                               // 已 latched 防 cache bust
  cachedMCEnabled: boolean                              // Cache-editing beta header（已 latched）
  effortValue: string                                   // 解析后的 effort
  extraBodyHash: number                                 // CLAUDE_CODE_EXTRA_BODY
  callCount: number
  pendingChanges: PendingChanges | null
  prevCacheReadTokens: number | null
  cacheDeletionsPending: boolean                        // 微压缩发 cache_edits 后合法 drop
  buildDiffableContent: () => string
}

// L101-107  源级状态上限
const previousStateBySource = new Map<string, PreviousState>()
const MAX_TRACKED_SOURCES = 10
const TRACKED_SOURCE_PREFIXES = ['repl_main_thread', 'sdk', 'agent:custom', 'agent:default', 'agent:builtin']

// L120  触发告警的最小 token 落差
const MIN_CACHE_MISS_TOKENS = 2_000

// L122-126  TTL 阈值
const CACHE_TTL_5MIN_MS = 5 * 60 * 1000
export const CACHE_TTL_1HOUR_MS = 60 * 60 * 1000

// L129-131  Haiku 跳过（缓存行为不同）
function isExcludedModel(model: string): boolean { return model.includes('haiku') }
```

**13 维度差异检测**（`PreviousState` 字段）：
1. `systemHash` / `systemCharCount` —— system 提示变化
2. `toolsHash` / `perToolHashes` / `toolNames` —— 工具集变化（77% 来自 per-tool schema hash）
3. `cacheControlHash` —— scope/TTL 翻转（global↔org、1h↔5m）
4. `model` —— 模型切换
5. `fastMode` —— fast 模式
6. `globalCacheStrategy` —— tool_based / system_prompt / none
7. `betas` —— beta header 列表
8. `autoModeActive` —— AFK_MODE_BETA_HEADER
9. `isUsingOverage` —— 超额
10. `cachedMCEnabled` —— cache-editing
11. `effortValue` —— effort 翻转
12. `extraBodyHash` —— extra body 变化

**关键洞见**：claudecode 通过**事后 hash 差异检测**反推 cache miss 的具体来源——这不是 prompt cache 工程化的主流做法（其他仓都不做），但 Anthropic 这种"cache_key 不透明 + cache 实际是否命中没法 grep"的环境下，**唯一可信手段就是统计 `cache_read_input_tokens` 跌幅并反推原因**。

#### 2.2.3 `cache_edits` 远程删除缓存条目（Anthropic 独有）

**已知信息**（`docs/Agent源码调研/claudecode.md:412-418`）：
- 不修改本地消息内容
- 通过 API 层 `cache_reference` + `cache_edits` 远程删除缓存条目
- 保持 prompt cache 前缀不变
- Fork 子 Agent 通过 `cacheSafeParams` 共享父会话 prompt cache

**`cacheDeletionsPending` 标志**（`promptCacheBreakDetection.ts:67`）：当 microCompact 发送 `cache_edits` 删除条目后，**`cache_read_input_tokens` 合法下降**，不要报"cache miss"误报。

#### 2.2.4 token 计数：4 级 fallback 链

**核心代码**：`/usr/local/LsmGitOpenSource/claudecode/src/services/tokenEstimation.ts:124-208 + utils/analyzeContext.ts:77-108`

```typescript
// tokenEstimation.ts L124-201  countMessagesTokensWithAPI
export async function countMessagesTokensWithAPI(
  messages: Anthropic.Beta.Messages.BetaMessageParam[],
  tools: Anthropic.Beta.Messages.BetaToolUnion[],
): Promise<number | null> {
  return withTokenCountVCR(messages, tools, async () => {
    try {
      const model = getMainLoopModel()
      const betas = getModelBetas(model)
      const containsThinking = hasThinkingBlocks(messages)

      if (getAPIProvider() === 'bedrock') {
        // bedrock-sdk 不支持 countTokens → 转 countTokensWithBedrock
        return countTokensWithBedrock({ ... })
      }

      const anthropic = await getAnthropicClient({ maxRetries: 1, model, source: 'count_tokens' })

      const response = await anthropic.beta.messages.countTokens({
        model: normalizeModelStringForAPI(model),
        messages: messages.length > 0 ? messages : [{ role: 'user', content: 'foo' }],  // ← 无 messages 时占位
        tools,
        ...(filteredBetas.length > 0 && { betas: filteredBetas }),
        ...(containsThinking && { thinking: { type: 'enabled', budget_tokens: TOKEN_COUNT_THINKING_BUDGET } }),
      })
      return response.input_tokens
    } catch (error) { logError(error); return null }
  })
}

// L203-208  字符串长度估算
export function roughTokenCountEstimation(content: string, bytesPerToken: number = 4): number {
  return Math.round(content.length / bytesPerToken)
}

// L215-224  按文件类型调整密度
export function bytesPerTokenForFileType(fileExtension: string): number {
  switch (fileExtension) {
    case 'json': case 'jsonl': case 'jsonc': return 2   // JSON 符号密集
    default: return 4
  }
}

// analyzeContext.ts L77-108  4 级 fallback 链
async function countTokensWithFallback(messages, tools): Promise<number | null> {
  try {
    const result = await countMessagesTokensWithAPI(messages, tools)
    if (result !== null) return result
    logForDebugging(`...API returned null, trying haiku fallback`)
  } catch (err) { logError(err) }

  try {
    const fallbackResult = await countTokensViaHaikuFallback(messages, tools)
    return fallbackResult
  } catch (err) { logError(err); return null }
}
```

**4 级 fallback 链**：
1. **`countMessagesTokensWithAPI`** —— Anthropic 官方 `countTokens`（最准）
2. **`countTokensViaHaikuFallback`** —— 用 Haiku 4.5（或 Sonnet）真实调用读 usage（`input_tokens + cache_creation + cache_read`，L319-324），**注意是按请求计费的真实开销**
3. **`roughTokenCountEstimation`** —— `Math.round(content.length / 4)`，JSON 改 `/2`
4. **`null`** —— 全部失败时返回 null，上游用默认值

**特殊处理**：
- **`countTokensViaHaikuFallback` 的模型选择**（L274-277）：
  - Vertex 全局区域：Haiku 不可用 → 用 Sonnet
  - Bedrock + thinking blocks：Haiku 3.5 不支持 → 用 Sonnet
  - Vertex + thinking blocks：同上
  - 默认：`getSmallFastModel()`（Haiku 4.5 支持 thinking blocks）
- **thinking budget**：`TOKEN_COUNT_THINKING_BUDGET = 1024` + `TOKEN_COUNT_MAX_TOKENS = 2048`（L31-33）—— 避免 thinking 占满导致 0 token 回答。
- **空 messages 占位**：`messages: messages.length > 0 ? messages : [{ role: 'user', content: 'foo' }]` —— 测工具 schema 时不能没有消息。
- **tool prompt preamble**：`TOOL_TOKEN_COUNT_OVERHEAD = 500`（`analyzeContext.ts:75`）—— API 每次调用加 ~500 tokens 工具前缀，per-tool 计数时要扣减以免 N×overhead 误算。

#### 2.2.5 Bedrock 计数：`CountTokens` 命令

**核心代码**：`/usr/local/LsmGitOpenSource/claudecode/src/services/tokenEstimation.ts:437-495`

```typescript
// tokenEstimation.ts L437-495  countTokensWithBedrock
async function countTokensWithBedrock({ model, messages, tools, betas, containsThinking }) {
  try {
    const client = await createBedrockRuntimeClient()
    // Bedrock CountTokens 需要 model ID 而非 inference profile / ARN
    const modelId = isFoundationModel(model)
      ? model
      : await getInferenceProfileBackingModel(model)
    if (!modelId) return null

    const requestBody = {
      anthropic_version: 'bedrock-2023-05-31',
      messages: messages.length > 0 ? messages : [{ role: 'user', content: 'foo' }],
      max_tokens: containsThinking ? TOKEN_COUNT_MAX_TOKENS : 1,
      ...(tools.length > 0 && { tools }),
      ...(betas.length > 0 && { anthropic_beta: betas }),
      ...(containsThinking && { thinking: { type: 'enabled', budget_tokens: TOKEN_COUNT_THINKING_BUDGET } }),
    }

    const { CountTokensCommand } = await import('@aws-sdk/client-bedrock-runtime')   // ← 动态 import 省 279KB
    const response = await client.send(new CountTokensCommand({
      modelId,
      input: { invokeModel: { body: new TextEncoder().encode(jsonStringify(requestBody)) } },
    }))
    return response.inputTokens ?? null
  } catch (error) { logError(error); return null }
}
```

**洞见**：动态 `@aws-sdk/client-bedrock-runtime` import —— 节省 ~279KB 启动开销，只有走 Bedrock 路径才加载（`tokenEstimation.ts:3-5` 注释）。

---

### 2.3 pi（TypeScript）—— cacheControl 路由 + 三层 TTL

#### 2.3.1 `getCacheControl` 模型能力感知

**核心代码**：`/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts:64-78 + 975-1058`

```typescript
// anthropic-messages.ts L64-78  getCacheControl
): { retention: CacheRetention; cacheControl?: CacheControlEphemeral } {
  ...
  cacheControl: { type: "ephemeral", ...(ttl && { ttl }) },   // ← ttl 是 "1h" 或 undefined
}

// anthropic-messages.ts L979  getCacheControl
const { cacheControl } = getCacheControl(model, options?.cacheRetention, options?.env);

// anthropic-messages.ts L1042-1058  工具调用：模型能力路由
if (immediateTools.length > 0 || deferredTools.length > 0) {
  params.tools = [
    ...convertTools(immediateTools, isOAuthToken, supportsEagerToolInputStreaming,
                    supportsStrictTools, supportsCacheControlOnTools ? cacheControl : undefined),
    ...convertTools(deferredTools, ..., undefined, true),    // ← deferred 不加 cache_control
  ];
}
```

**5 个独立 compat 标志**（来自 `types.ts:615-678`）：
- `cacheControlFormat` —— "anthropic" 风格
- `supportsCacheControlOnTools` —— 工具是否支持 cache_control（false 时不传）
- `supportsCacheControlOnLastUserMessage` —— 最后 user message
- `supportsCacheControlOnLastAssistantMessage` —— 最后 assistant
- `supportsCacheControlOnLastToolResult` —— 最后 tool result

#### 2.3.2 `convertTools` 与 `convertMessages` 三断点

**核心代码**：`anthropic-messages.ts:1158-1312 + 1326-1363`

```typescript
// anthropic-messages.ts L1295-1312  最后 user message
// Add cache_control to the last user message to cache conversation history
if (cacheControl && params.length > 0) {
  const lastMessage = params[params.length - 1];
  if (lastMessage.role === "user") {
    if (typeof lastMessage.content === "string") {
      lastMessage.content = [{ type: "text", text: lastMessage.content, cache_control: cacheControl }];
    } else if (Array.isArray(lastMessage.content)) {
      const lastBlock = lastMessage.content[lastMessage.content.length - 1];
      if (lastBlock.type === "text") {
        (lastBlock as any).cache_control = cacheControl;
      } else if (lastBlock.type === "tool_result") {
        lastMessage.content.push({ type: "text", text: "", cache_control: cacheControl });
      }
    }
  }
}

// anthropic-messages.ts L1331-1363  convertTools
function convertTools(tools, isOAuthToken, supportsEager, supportsStrict, cacheControl, deferLoading) {
  return tools.map((tool, index) => {
    return {
      ...(cacheControl && index === tools.length - 1 ? { cache_control: cacheControl } : {}),
      //                                ↑ 只在最后 tool
    };
  });
}
```

#### 2.3.3 三个 `cache_control` 标记点

`params.system`（L1011-1033）：**第一条 system 文本块**打 `cache_control`（OAuth 时是 "You are Claude Code..." 固定文本，非 OAuth 是 `sanitizeSurrogates(context.systemPrompt)`）。

`params.messages`（L1295-1312）：**最后 user 消息的最后文本块或末尾追加空 text block**打 `cache_control`。

`params.tools`（L1360）：**最后一个 tool** 打 `cache_control`。

**3 个 cache_control 标记点 < 4 上限，留 1 个给 manual override**。

#### 2.3.4 1h vs 5m TTL 选择

**核心代码**：`types.ts:615-678` + `api/anthropic-messages.ts:64-78`

```typescript
// types.ts L615-678  五种 cache 相关 compat
/** Cache control convention for prompt caching. "anthropic" applies Anthropic-style `cache_control`
 * markers to the system prompt, last tool definition, and last user, assistant, or tool-result
 * text content. */
cacheControlFormat?: "anthropic";

/** Whether the provider supports long prompt cache retention (`prompt_cache_retention: "24h"`
 * or Anthropic-style `cache_control.ttl: "1h"`, depending on format). Default: true. */
supportsCacheControlLongRetention?: boolean;

/** Whether the provider supports Anthropic long cache retention (`cache_control.ttl: "1h"`).
 * Default: true. */
supportsLongPromptCaching?: boolean;

/** Whether the provider supports Anthropic-style `cache_control` markers on
 * tool definitions. When false, `cache_control` is omitted from tool params. */
supportsCacheControlOnTools?: boolean;
```

#### 2.3.5 token 计数：基于 LLM 真实调用 + post-compaction 估算

**核心代码**：`packages/coding-agent/src/core/compaction/branch-summarization.ts:300-318 + compaction/compaction.ts:128`

```typescript
// branch-summarization.ts L300-318  默认 reserveTokens = 16384
async function summarizeBranch({ entries, reserveTokens = 16384 }) {
  const tokenBudget = contextWindow - reserveTokens;   // ← 摘要预算 = 上下文窗 - 输出预留
  ...
}

// compaction.ts L128
interface CompactionConfig {
  reserveTokens: number;   // 输出预留（默认 16384）
  ...
}
```

**洞见**：pi 的 `reserveTokens = 16384`（默认）是给"摘要 LLM 调用本身的输入 + 输出"预留的 token 空间。摘要调用也是要付钱的。

**post-compaction token 估算**（CHANGELOG:946）：压缩后立即给出 `estimated post-compaction token count`，让客户端展示"上下文减了多少"。

---

### 2.4 openclaw（TypeScript）—— payload policy + cache_ttl_pruning + split prefix/suffix

#### 2.4.1 断点上限 + 审查策略

**核心代码**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/transports/anthropic-payload-policy.ts:43-102`

```typescript
// anthropic-payload-policy.ts L43-44  常量
const ANTHROPIC_CACHE_CONTROL_LIMIT = 4;
const ANTHROPIC_COMPACT_THRESHOLD_MIN = 50_000;

// L60-73  resolveAnthropicCompactThreshold
function resolveAnthropicCompactThreshold(contextWindow: unknown, configured: unknown): number {
  const configuredThreshold = parsePositiveInteger(configured);
  if (configuredThreshold !== undefined) {
    return Math.max(ANTHROPIC_COMPACT_THRESHOLD_MIN, configuredThreshold);   // ← 下限 50K
  }
  const resolvedContextWindow = parsePositiveInteger(contextWindow);
  return Math.max(
    ANTHROPIC_COMPACT_THRESHOLD_MIN,
    resolvedContextWindow === undefined
      ? ANTHROPIC_COMPACT_THRESHOLD_MIN
      : Math.floor(resolvedContextWindow * 0.7),   // ← 默认 70% 上下文窗
  );
}

// L156-171  resolveAnthropicEphemeralCacheControl
export function resolveAnthropicEphemeralCacheControl(baseUrl, cacheRetention): AnthropicEphemeralCacheControl | undefined {
  const retention = resolveCacheRetention(cacheRetention);
  if (retention === "none") return undefined;

  // Trust explicit long-retention opt-ins for Anthropic-compatible custom providers.
  // Keep hostname gating for implicit/env-driven long retention so defaults stay conservative.
  const ttl =
    retention === "long" && (cacheRetention === "long" || isLongTtlEligibleEndpoint(baseUrl))
      ? "1h" : undefined;
  return { type: "ephemeral", ...(ttl ? { ttl } : {}) };
}

// L138-153  isLongTtlEligibleEndpoint（hostname 白名单）
function isLongTtlEligibleEndpoint(baseUrl: string | undefined): boolean {
  if (typeof baseUrl !== "string") return false;
  const hostname = resolveBaseUrlHostname(baseUrl);
  if (!hostname) return false;
  return (
    hostname === "api.anthropic.com" ||
    hostname === "aiplatform.googleapis.com" ||
    hostname === "aiplatform.us.rep.googleapis.com" ||
    hostname === "aiplatform.eu.rep.googleapis.com" ||
    hostname.endsWith("-aiplatform.googleapis.com")
  );
}
```

**洞见**：openclaw 比其他仓更保守——**hostname 白名单**才会给 `1h` TTL（避免把 OAuth token 滥用路由的 1h 写入）。

#### 2.4.2 `splitSystemPromptCacheBoundary` 显式拆分稳定/动态

**核心代码**：`/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/transports/anthropic-payload-policy.ts:173-218`

```typescript
// anthropic-payload-policy.ts L173-218  applyAnthropicCacheControlToSystem
function applyAnthropicCacheControlToSystem(system: unknown, cacheControl: AnthropicEphemeralCacheControl): void {
  if (!Array.isArray(system)) return;

  const normalizedBlocks: Array<unknown> = [];
  for (const block of system) {
    if (!block || typeof block !== "object") { normalizedBlocks.push(block); continue; }
    const record = block as Record<string, unknown>;
    if (record.type !== "text" || typeof record.text !== "string") { normalizedBlocks.push(block); continue; }

    const split = splitSystemPromptCacheBoundary(record.text);
    if (!split) {
      // 无 boundary 标记：直接给 cache_control
      if (record.cache_control === undefined) {
        record.cache_control = cacheControl;
      }
      normalizedBlocks.push(record);
      continue;
    }

    // 有 boundary 标记：拆成两块
    const { cache_control: existingCacheControl, ...rest } = record;
    if (split.stablePrefix) {
      normalizedBlocks.push({
        ...rest,
        text: split.stablePrefix,
        cache_control: existingCacheControl ?? cacheControl,   // ← 稳定前缀打 cache
      });
    }
    if (split.dynamicSuffix) {
      normalizedBlocks.push({
        ...rest,
        text: split.dynamicSuffix,                             // ← 动态后缀不打 cache
      });
    }
  }

  system.splice(0, system.length, ...normalizedBlocks);
}

// L220-234  stripSystemPromptCacheBoundary（发送前清除 boundary 标记）
function stripAnthropicSystemPromptBoundary(system: unknown): void {
  if (!Array.isArray(system)) return;
  for (const block of system) {
    if (!block || typeof block !== "object") continue;
    const record = block as Record<string, unknown>;
    if (record.type === "text" && typeof record.text === "string") {
      record.text = stripSystemPromptCacheBoundary(record.text);
    }
  }
}
```

**核心创新**：openclaw 提供 **`<<<LAEW:STABLE>>>...<<<LAEW:DYNAMIC>>>...` 边界标记**（来自 `splitSystemPromptCacheBoundary`），让 system prompt 显式拆成"稳定前缀 + 动态后缀"两部分：
- 稳定前缀打 `cache_control` → Anthropic 缓存
- 动态后缀不打 → 不污染缓存

**`stripSystemPromptCacheBoundary`**（L220-234）：发送前清除 boundary 标记，模型看到的还是纯文本。

**这是 openclaw 独占的设计模式**，其他仓（opencode / pi / claudecode）都没有显式 boundary 标记。

#### 2.4.3 `applyAnthropicCacheControlToMessages` 反向遍历

**核心代码**：`anthropic-payload-policy.ts:236-310`

```typescript
// L236-310  applyAnthropicCacheControlToMessages（反向遍历找"最深稳定 user 消息"）
export function applyAnthropicCacheControlToMessages(
  messages: unknown, cacheControl, markerLimit: number,
  cacheBreakpointOptOutMessageIndexes: ReadonlySet<number>,
): void {
  if (!Array.isArray(messages) || messages.length === 0 || markerLimit <= 0) return;

  let fallbackToolResult: Record<string, unknown> | undefined;

  // 反向遍历：找最后一个 user message
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    if (!message || typeof message !== "object") continue;

    const record = message as Record<string, unknown>;
    if (record.role !== "user" || cacheBreakpointOptOutMessageIndexes.has(i)) continue;

    const content = record.content;
    if (typeof content === "string") {
      if (fallbackToolResult && markerLimit === 1) {
        fallbackToolResult.cache_control = cacheControl;
        return;
      }
      record.content = [{ type: "text", text: content, cache_control: cacheControl }];
      if (fallbackToolResult && markerLimit > 1) {
        fallbackToolResult.cache_control = cacheControl;
      }
      return;
    }
    ...
  }
}
```

**3 个亮点**：
1. **反向遍历** —— 找**最后一个 user** 而不是最后一个 message（tool message 也是 user role，但只 cache 文本块）
2. **`fallbackToolResult`** —— 如果 user 消息只有 tool_result 块，先记录 fallback，最终给它打 cache_control
3. **`markerLimit`** —— 防止超过 4 断点上限

#### 2.4.4 `cache_ttl_pruning` 工具卸载策略

**核心代码**：`anthropic-payload-policy.ts:357-374`

```typescript
// L357-374  toolClearing 配置
...(input.cacheTtlPruning &&
  normalizeOptionalLowercaseString(input.api) === "anthropic-messages" &&
  isDirectAnthropicModel(input)
  ? {
    toolClearing: {
      trigger: Math.max(50_000, Math.floor((parsePositiveInteger(input.contextWindow) ?? 0) * 0.3)),   // 30% 触发
      clearAtLeast: Math.max(12_500, Math.floor((parsePositiveInteger(input.contextWindow) ?? 0) * 0.05)),  // 5% 清理
      tools: input.cacheTtlPruning.tools,
    },
  }
  : {}),
```

**洞见**：当 input token 达到 `max(50K, 30% contextWindow)` 时，自动卸载工具直到**清掉至少 5% 上下文**。这是 openclaw **独有的"主动让出空间给 cache"** 策略——工具 schema 占 token，卸载可让 cache 命中更多前缀。

#### 2.4.5 `applyAnthropicCacheControlToSystem` 不重新写已存在 cache_control

`L192-198`：注释"if (record.cache_control === undefined)" —— **不覆盖已有 cache_control**，尊重手动设置。

#### 2.4.6 `countAnthropicCacheControlMarkers` 计数校验

**核心代码**：`anthropic-payload-policy.ts:312-324`

```typescript
function countAnthropicCacheControlMarkers(blocks: unknown): number {
  if (!Array.isArray(blocks)) return 0;
  let count = 0;
  for (const block of blocks) {
    if (block && typeof block === "object" && "cache_control" in block) count += 1;
  }
  return count;
}
```

**洞见**：openclaw 在**多个 array 上分别计算 marker 数**（system + tools + messages），确保总和不超 4。

---

### 2.5 deepseek-harness（TypeScript）—— 固定密度估算 + epoch 投影

#### 2.5.1 token-meter 固定密度启发式

**核心代码**：`/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/token-meter/src/estimate.ts:12-99`

```typescript
// estimate.ts L12-19  常量
const CHARS_PER_TOKEN = 4
const BLOCK_OVERHEAD = 4
export const ROLE_OVERHEAD = 4

// L28-30  estimateStructuralBlock
export function estimateStructuralBlock(block: ContentBlock): number {
  return BLOCK_OVERHEAD + Math.ceil(JSON.stringify(block).length / CHARS_PER_TOKEN)
}

// L37-61  estimateContent
export function estimateContent(blocks: readonly ContentBlock[]): number {
  let tokens = 0;
  for (const block of blocks) {
    switch (block.type) {
      case 'text':
      case 'reasoning':
        tokens += Math.ceil(block.text.length / CHARS_PER_TOKEN) + BLOCK_OVERHEAD;
        break;
      case 'tool-call':
        tokens += Math.ceil(block.name.length / CHARS_PER_TOKEN)
          + Math.ceil(block.arguments.length / CHARS_PER_TOKEN)
          + BLOCK_OVERHEAD;
        break;
      case 'tool-result':
        tokens += estimateContent(block.content) + BLOCK_OVERHEAD;
        break;
      default:
        tokens += estimateStructuralBlock(block);   // ← 未知类型保守估计
    }
  }
  return tokens;
}

// L68-70  estimateMessage
export function estimateMessage(message: Message): number {
  return estimateContent(message.content) + ROLE_OVERHEAD;
}

// L87-90  estimateToolsTokens
export function estimateToolsTokens(header: EpochHeader | undefined): number {
  if (header?.tools === undefined || header.tools.length === 0) return 0;
  return Math.ceil(JSON.stringify(header.tools).length / CHARS_PER_TOKEN) + BLOCK_OVERHEAD;
}
```

**3 项常数**：
- `CHARS_PER_TOKEN = 4` —— 文本/JSON 统一密度
- `BLOCK_OVERHEAD = 4` —— 每个 content block 框架 token
- `ROLE_OVERHEAD = 4` —— 每条 message 的 role 字段

**未知类型保守估计**：`estimateStructuralBlock` 用 `JSON.stringify(block).length / 4` + `4` —— 不抛错，至少有个数。

#### 2.5.2 surface projection（O(1) 表面投影）

`token-meter/src/breakdown-projection.ts`（已知）：把"实际 LLM 收到"的内容**只算 surface tokens**，不递归展开 thinking 内部细节（O(1) 复杂度）。

#### 2.5.3 thresholdRatio + retainTokens 压缩触发

来自 `docs/Agent源码调研/deepseek-harness.md:473`：
- `thresholdRatio = 0.8`（80%）触发压缩
- `retainRatio` / `retainTokens` 控制保留量

`docs/Agent源码调研/deepseek-harness.md:423`：`compression_call_max_tokens: int = 200000` —— 摘要调用自身的 token 预算。

#### 2.5.4 token-meter 模块定位

来自 README（已知）：
- 不依赖 API 返回的 usage
- 用固定密度启发式在本地估算
- 用于上下文预算预测

---

### 2.6 atomcode（Rust）—— 字节稳定性 + cache_creation 归一

#### 2.6.1 `TokenUsage` 三元组结构

**核心代码**：`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/anthropic.rs:760-866`

```rust
// anthropic.rs L760-826  TokenUsage
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read: u32,
    pub cache_creation: u32,
}

impl TokenUsage {
    pub fn prompt(&self) -> u32 {
        self.input_tokens + self.cache_read + self.cache_creation   // ← 注释："prompt = input + cache_read (+ cache_creation)"
    }
    pub fn cached(&self) -> u32 { self.cache_read }
}

// L850-866  message_start 事件解析
"message_start" => {
    let u = &v["message"]["usage"];
    self.input_tokens = u32_at(u, "input_tokens");
    self.cache_read = u32_at(u, "cache_read_input_tokens");
    self.cache_creation = u32_at(u, "cache_creation_input_tokens");
    ...
}
```

#### 2.6.2 字节稳定性测试

来自 `docs/Agent源码调研/atomcode.md:2456-2463`：

```rust
// 序列化字节稳定性 = serde_json::Map(BTreeMap-backed) + 无时间戳/uuid
// Anthropic 的 prompt cache 按字节前缀; 同 (system, messages, tools) 必须序列化恒等

#[test]
fn body_serialization_is_deterministic() {  // anthropic.rs L1447-1490
    // 连续 100 次序列化同一输入, 逐字节比对
    // 这是 prompt cache 的硬约束
}
```

**反模式警示**：
- ❌ 动态时间戳插入 system（`new Date().toISOString()`）
- ❌ UUID 插入 messages
- ❌ `HashMap` 序列化顺序不确定（用 `BTreeMap` 替代）
- ❌ 浮点数（NaN/Infinity 不稳定）

#### 2.6.3 `merge_consecutive_user` 协议修正

来自 `docs/Agent源码调研/atomcode.md:3090`：anthropic.rs:556-601 处理连续 user 消息合并（post-compaction summary、`<system-reminder>` 后缀）—— Anthropic 禁止 role alternation 违规。

---

### 2.7 Switchyard（Rust LLM 网关）—— 审查预算 = 调用次数

#### 2.7.1 `max_reviews` 预算作用域

**核心代码**：`/usr/local/LsmGitOpenSource/Switchyard/crates/libsy/src/algorithms/advisor_gate.rs:102-150`

```rust
// advisor_gate.rs L101-149  AdvisorGateConfig
#[derive(Clone, Debug)]
pub struct AdvisorGateConfig {
    pub reviewer_system_prompt: String,
    pub redo_feedback_prefix: String,
    pub gate_trigger: GateTrigger,
    pub max_reviews: u32,                                  // ← 每次预算 scope 允许的评审次数
    pub gate_stall_turns: u32,
    pub gate_min_tool_results: u32,
    pub advisor_max_tokens: u64,                            // ← 单次评审输出上限
    pub advisor_temperature: Option<f64>,
    pub transcript_max_chars: usize,                        // ← advisor transcript 上限
    pub fail_open: bool,
}

impl Default for AdvisorGateConfig {
    fn default() -> Self {
        Self {
            reviewer_system_prompt: REVIEWER_SYSTEM_PROMPT.to_string(),
            redo_feedback_prefix: REDO_FEEDBACK_PREFIX.to_string(),
            gate_trigger: GateTrigger::NoToolCall,
            max_reviews: 1,                                 // ← 默认 1 次/任务
            gate_stall_turns: 0,
            gate_min_tool_results: 0,
            advisor_max_tokens: 2048,
            advisor_temperature: None,
            transcript_max_chars: 200_000,
            fail_open: true,
        }
    }
}
```

#### 2.7.2 预算 scope 优先级

**核心代码**：`advisor_gate.rs:160-175 + 612-630`

```rust
// L160-175  ScopeKey 三级优先级
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ScopeKey {
    Instance,                                  // 兜底
    Client(String),                            // BENCH_SESSION_HEADER proxy_x_session_id
    Session(String),                           // session id
}

// L612-630  budget_scope 解析
fn budget_scope(request: &Request) -> ScopeKey {
    // benchmark header 优先（sub-agents 也带上）
    if let Some(value) = metadata.and_then(...).and_then(|headers| headers.get(BENCH_SESSION_HEADER))
        && !value.is_empty() {
        return ScopeKey::Client(value.to_string());
    }
    if let Some(id) = metadata.and_then(...).and_then(|metadata| metadata.session_id.as_deref())
        && !id.is_empty() {
        return ScopeKey::Session(id.to_string());
    }
    ScopeKey::Instance
}
```

#### 2.7.3 `try_reserve` 原子预留 + `refund_failure` 失败退还

```rust
// L260-279  try_reserve
fn try_reserve(&self, scope: &ScopeKey) -> bool {
    let mut state = self.state.lock();
    if state.scopes.len() >= MAX_TRACKED_SCOPES && !state.scopes.contains_key(scope) {
        let evict = state.scopes.keys().find(|key| **key != ScopeKey::Instance).cloned();
        if let Some(key) = evict { state.scopes.remove(&key); }
    }
    let max_reviews = self.config.max_reviews;
    let entry = state.scopes.entry(scope.clone()).or_default();
    if entry.reviews >= max_reviews || entry.failed_consults >= MAX_FAILED_CONSULTS { return false; }
    entry.reviews += 1;
    true
}

// L284-289  refund_failure
fn refund_failure(&self, scope: &ScopeKey) {
    let entry = self.state.lock().scopes.entry(scope.clone()).or_default();
    entry.reviews = entry.reviews.saturating_sub(1);
    entry.failed_consults += 1;
}
```

**洞见**：Switchyard 是**唯一把"成本控制 = 调用次数控制"显式建模**的项目——`max_reviews=1` 是基准配置，**限制 advisor LLM 调用次数 = 限制 USD 消耗**。注释明确（`advisor_gate.rs:21-26`）：
> "executor errors always propagate (including `ContextWindowExceeded`, which hosts map to a client-visible 400 so agent harnesses can compact)."

**laew 多 Agent 架构下的直接借鉴**：限制 Plan Agent 调 1 次、QC Agent 调 1 次、SessionContext Agent 调 1 次。

#### 2.7.4 上下文溢出策略：宁报错

```rust
// advisor_gate.rs L485-493  fail_open = false 时，错误必须冒泡
if !self.config.fail_open {
    return Err(algorithm_error(format!(
        "advisor consult failed (fail_open = false): {error}"
    )));
    // 注释："a typed ContextWindowExceeded from the consult would otherwise
    // reach the client as 400 context_length_exceeded and trigger compaction
    // of a healthy conversation."
}
```

**洞见**：Switchyard 显式 **fail_closed 路径下不把 ContextWindowExceeded 当 algorithm 错误**——避免错误地把"健康对话"压回去重压。

---

## 3. 横向对比大表（14 行）

| # | 仓库 | 断点上限 | 断点选择算法 | TTL 选项 | token 计数 fallback 链 | 上下文窗口默认 | reserved output | 压缩触发 | 失败处理 | cache 命中度量 | 工具排序稳定性 | boundary 标记 | 字节稳定性 | 跨 session 复用 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | **opencode** | 4（`ANTHROPIC_BREAKPOINT_CAP`） | `applyCachePolicy`：tools+system+latest-user | `ephemeral` + `ttlSeconds` (5m/1h) | API usage 直接读 | 模型元数据 | `outputLimit=4096` 默认 + `generation.maxTokens` | `isContextOverflow` 28 条正则 | 自动 `compaction.ts` | `cache_read_input_tokens` + `cacheWrite1h/5m` 分账 | ✅ Effect immutable | ❌ 无 | ✅ `serde_json::Map(BTreeMap)` | ❌ |
| 2 | **claudecode** | 4（Anthropic SDK 透传） | 3 个 explicit 调用点 user/assistant text + `enablePromptCaching` flag | `5m` / `1h`（GrowthBook latch） | **API → Haiku 真实调用 → roughToken(4/JSON 2) → null** | 200K 默认（`MODEL_CONTEXT_WINDOW_DEFAULT`） | 隐式 SDK | `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000` 静默 + `tool_use_id` 替换 | `cache_edits` 远程删除 + `promptCacheBreakDetection` 13 维度 hash | ✅ **完整 13 维度 hash 差异定位** | ✅ `assembleToolPool` SSOT + contiguous prefix | ❌ 无 | ✅ SDK JSON 序列化稳定 | ✅ `cacheSafeParams` Fork 共享 |
| 3 | **pi** | 4（手动 3 个，剩 1 给 manual） | **三处 explicit**：system 第一块 + 最后 user text + 最后 tool | `5m` / `1h`（`supportsCacheControlLongRetention` 模型能力） | LLM 真实调用读 usage（`input_tokens + cache_creation + cache_read`） | 模型元数据 | `reserveTokens=16384` 默认 | post-compaction token estimate | `model_context_window_exceeded` → auto-compact | `cache_read_input_tokens` | ✅ OAuth 时 `toClaudeCodeName` 命名空间化 | ❌ 无 | ✅ | ❌ |
| 4 | **openclaw** | 4（`ANTHROPIC_CACHE_CONTROL_LIMIT`） | **反向遍历找最深 stable user** + **`<<<LAEW:STABLE>>>...<<<LAEW:DYNAMIC>>>...` boundary 拆 system** | `5m` / `1h`（**hostname 白名单**才给 1h） | API usage 直接读 | 模型元数据 | `compactThreshold = max(50K, 0.7*contextWindow)` | **30% 触发 + 5% 清理** `cache_ttl_pruning` 工具卸载 | 不重写已有 `cache_control` | `cacheReadTokens` + `cacheWriteTokens`（合并 1h/5m） | ✅ stable JSON ordering | ✅ **唯一显式 boundary** | ✅ JSON 序列化 | ❌ |
| 5 | **deepseek-harness** | 不显式 | 不显式 cache_control（API 隐式） | N/A | **纯本地估算 `CHARS_PER_TOKEN=4` + `BLOCK_OVERHEAD=4` + `ROLE_OVERHEAD=4`** | 模型元数据 | `compression_call_max_tokens=200000` | `thresholdRatio=0.8` + `retainTokens` | `block({code:'round-limit'})` | `prompt_cache_hit_tokens` + `prompt_cache_miss_tokens` | ✅ Cordis Everything-is-Plugin | ❌ 无 | ✅ | ❌ |
| 6 | **atomcode** | 不显式 | 不显式 cache_control | N/A | API usage 直接读 + `TokenUsage { input, output, cache_read, cache_creation }` | 模型元数据 | `request.options.maxOutputTokens` | `isContextOverflow` 后 `merge_consecutive_user` + 自动压缩 | `body_serialization_is_deterministic` 测试 100 次逐字节比对 | `cache_read_input_tokens` + `cache_creation_input_tokens` 合并 `prompt` | ✅ Cargo feature gating + stable order | ❌ 无 | ✅ **BTreeMap-backed 强制约束** | ❌ |
| 7 | **Switchyard** | N/A | N/A（无 Anthropic 路径） | N/A | `ContextWindowExceeded` 错误检测 + 不重计 | 转发模型本身 | `advisor_max_tokens: 2048` | 强制 abort（**宁报错不压缩**） | `max_reviews` 预算耗尽 → 纯 passthrough | N/A | ✅ 协议无关路由 | N/A | ✅ | ✅ `proxy_x_session_id` 跨进程共享 |
| 8 | **cc-switch** | 不显式 | 不显式 cache_control | N/A | `prompt_tokens_details.cached_tokens` (OpenAI) + `cache_read_tokens` + `cache_creation_tokens` (Anthropic) | 模型元数据 | `maxOutputTokens` | `MonthlyLimitError` / `BlackUsageLimitError` | **8 款适配** + 熔断器三态 | 4 字段 fallback 链 | ✅ | ❌ 无 | ✅ | ❌ |
| 9 | **laew** | ❌ 未支持 | ❌ | ❌ | ❌ 无本地估算 | ❌ 无常量表 | ❌ | ❌ 报错透传 | ❌ | ✅ **仅解析** `cache_read_input_tokens` | ❌ 无约束 | ❌ 无 | ❌ **BTreeMap 未强制** | ❌ |
| 10 | **agent-core** (openJiuwen) | 不显式 | 不显式 cache_control | N/A | **本地 tokenizer → tiktoken → `StringLengthCounter` 兜底** | 模型元数据 | `compression_call_max_tokens=200000` | `BudgetGuardProcessor` + `ToolResultBudgetProcessor` | OTLP 遥测 | `Usage` schema | ✅ | ❌ 无 | ✅ | ✅ ContextEngine 多类型记忆 |
| 11 | **hermes-agent** | 不显式 | 不显式 cache_control | N/A | API usage | 模型元数据 | `iteration_budget.refund()` 失败退还 | `FTS5` 索引压缩 | `dropped tool call` 不消耗预算 | `transcript_tokens` | ✅ | ❌ 无 | ✅ | ✅ CompressionCommitFence |
| 12 | **openclaw (workshop)** | ❌ | ❌ | ❌ | workshop self-evolve 不调 LLM | 模型元数据 | workshop budget | Lane 三队列 + Background lane 独立预算 | ❌ | ❌ | ✅ | ❌ 无 | ✅ | ✅ |
| 13 | **undici** | N/A | N/A（HTTP 客户端） | N/A | N/A | N/A | N/A | N/A | `GOAWAY` 重放预算 `MAX_GOAWAY_REPLAY_ATTEMPTS=1` | N/A | N/A | N/A | ✅ CBOR frame + 14 损坏检测 | N/A |
| 14 | **jiuwenswarm** | ❌ | ❌ | ❌ | Team Token 预算 | 模型元数据 | `compression_call_max_tokens` | A2A 跨 agent | `Seccomp` 沙箱 | ❌ | ✅ SkillDevPipeline 12 阶段 | ❌ 无 | ✅ | ✅ JiuwenBox |

---

## 4. 断点放置算法流程图（ASCII）

```
opencode 的 applyCachePolicy 主流程
═══════════════════════════════════════════════════════

INPUT: LLMRequest { tools, system, messages, cache?: CachePolicy }

  ┌────────────────────────────────────────────────────┐
  │ model.route.id ∈ {"anthropic-messages","bedrock-converse"}?
  │ (RESPECTS_INLINE_HINTS 白名单)                      │
  └──────┬─────────────────────────────────────────────┘
         │ 否 → return request（OpenAI / Gemini 走隐式缓存）
         ▼ 是
  ┌────────────────────────────────────────────────────┐
  │ resolve(policy):                                    │
  │   undefined / "auto" → AUTO = { tools:T, system:T,  │
  │                                  messages:"latest-user" }
  │   "none"            → NONE = {}                     │
  │   object            → 原样使用                       │
  └──────┬─────────────────────────────────────────────┘
         │
         ▼
  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
  │ markLastTool     │  │ markLastSystem   │  │ markMessages     │
  │ (只在最后 tool)  │  │ (只在最后 system │  │ ("latest-user"   │
  │ 不覆盖已有 cache │  │  part)           │  │  → 反向遍历找    │
  └────────┬─────────┘  └────────┬─────────┘  │  最后 user 文本) │
           │                     │             └────────┬─────────┘
           ▼                     ▼                      ▼
  ┌────────────────────────────────────────────────────────────┐
  │ 三处 hint = makeHint(ttlSeconds) = { type:"ephemeral", ttl? }│
  │ (ttlSeconds >= 3600 → "1h"，否则省略）                    │
  └──────────────────────┬─────────────────────────────────────┘
                         ▼
  ┌────────────────────────────────────────────────────────────┐
  │ LLMRequest.update(request, { tools, system, messages })   │
  │ (若 3 个都没变，返回原 request 引用相等)                  │
  └──────────────────────┬─────────────────────────────────────┘
                         ▼
                         ↓
              传给 anthropic-messages.ts fromRequest
                         ↓
  ┌────────────────────────────────────────────────────────────┐
  │ breakpoints = Cache.newBreakpoints(ANTHROPIC_BREAKPOINT_CAP=4)│
  │ { remaining:4, dropped:0 }                                │
  └──────────────────────┬─────────────────────────────────────┘
                         ▼
  ┌──────────────────────┐  ┌──────────────────┐  ┌────────────────┐
  │ lowerTool(bp, tool)  │  │ lowerSystem(bp)  │  │ lowerMessages  │
  │ cacheControl(bp) →   │  │ cacheControl(bp) │  │ cacheControl   │
  │   扣 remaining       │  │   扣 remaining   │  │   扣 remaining │
  └──────────────────────┘  └──────────────────┘  └────────────────┘
                         ↓
  ┌────────────────────────────────────────────────────────────┐
  │ if breakpoints.dropped > 0:                               │
  │   Effect.logWarning("dropped N cache breakpoint(s);       │
  │                      the API allows at most 4 per request")│
  └────────────────────────────────────────────────────────────┘


openclaw 的 applyAnthropicCacheControlToMessages 反向遍历
═══════════════════════════════════════════════════════

INPUT: messages array + cacheControl + markerLimit + optOut indexes

  ┌────────────────────────────────────────────────┐
  │ markerLimit <= 0 → return                      │
  └──────┬─────────────────────────────────────────┘
         │
         ▼ 反向遍历 i = messages.length - 1 → 0
  ┌────────────────────────────────────────────────────────┐
  │ for (let i = messages.length - 1; i >= 0; i--) {      │
  │   if (messages[i].role === "user"                     │
  │       && !optOut.has(i)) {                            │
  │     // 找到最后 user message                          │
  │     content = messages[i].content;                    │
  │     if (typeof content === "string") {                │
  │       if (fallbackToolResult && markerLimit === 1)    │
  │         fallbackToolResult.cache_control = cc; return │
  │       content = [{type:"text", text:content,          │
  │                   cache_control: cc}];                │
  │       return;                                         │
  │     }                                                 │
  │     for (let j = content.length - 1; j >= 0; j--) {   │
  │       if (content[j].type === "text"                  │
  │           || content[j].type === "image") {           │
  │         content[j].cache_control = cc;                │
  │         return;                                       │
  │       }                                               │
  │       if (content[j].type === "tool_result"           │
  │           && fallbackToolResult === undefined)        │
  │         fallbackToolResult = content[j];              │
  │     }                                                 │
  │   }                                                   │
  │ }                                                     │
  │ if (fallbackToolResult) {                             │
  │   fallbackToolResult.cache_control = cc;              │
  │ }                                                     │
  └────────────────────────────────────────────────────────┘

  关键点：
  - 反向遍历找最后 user message
  - content 优先 string 简化、否则逐块
  - text/image 块直接打 cc
  - tool_result 块暂存 fallback（最后没找到就给它打）
  - markerLimit 决定"是否给 fallback 也打"（= 1 时只打 fallback，> 1 时可同时打）
```

---

## 5. 缓存失效的 8 大坑与规避

### 坑 1：动态内容（时间戳/cwd/git status）放 system 前缀

**症状**：每次请求 system 前缀 hash 不同，Anthropic cache 永远 miss。

**反例**：
```rust
// ❌ BAD：dynamic content in system prefix
let system = format!(
    "Today is {}. CWD: {}. Git status: {}",
    chrono::Local::now().format("%Y-%m-%d"),
    std::env::current_dir()?.display(),
    git_status(),
);
```

**正解**：**分两块**（参考 openclaw `splitSystemPromptCacheBoundary`）：
```rust
// ✅ GOOD：stable prefix + dynamic suffix
let system = vec![
    SystemBlock { text: stable_prompt, cache_control: Some(ephemeral) },   // ← 稳定前缀打 cache
    SystemBlock { text: dynamic_context },                                  // ← 动态后缀不打
];
```

**laew 现状**：当前 `AgentProfile::system_prompt()` 是单字符串，**全部放在 system 前缀**——一旦加上时间戳/cwd/git status，整个 system 都失效。

### 坑 2：工具集顺序不稳定

**症状**：opencode / claudecode / openclaw 工具集顺序变化 → tools 部分 cache miss。

**反例**：
```rust
// ❌ BAD：HashMap iteration order 不稳定
fn builtin_registry() -> Vec<Box<dyn Tool>> {
    let mut map = HashMap::new();
    map.insert("Bash", bash_tool());
    map.insert("Read", read_tool());
    map.insert("Write", write_tool());
    map.into_values().collect()  // ← 顺序随机
}
```

**正解**：
```rust
// ✅ GOOD：固定顺序数组
fn builtin_registry() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(BashTool),
        Box::new(ReadTool),
        Box::new(WriteTool),
    ]   // ← 顺序确定
}
```

**laew 现状**：`src/agent/tools/mod.rs::builtin_registry()` 用 `Vec`，但内部顺序由构造决定——需审查。

### 坑 3：thinking 配置改变

**症状**：pi `simple-options.ts L55-79` 注释（`docs/Agent源码调研/专题/专题-第六轮-Anthropic与OpenAI协议调用真实实现深度对比.md:108`）：
> "thinking 预算帽改变 → cache bust"

claudecode 同样：`maxOutputTokens` 改变 `budget_tokens` 会破坏 cache（`docs/Agent源码调研/claudecode.md:719`）。

**正解**：claudecode `should1hCacheTTL` + `effortValue` 全部 **latch 进 STATE**（`api.ts:407-412`）—— 会话内不允许翻。

### 坑 4：消息拼接产生连续 user 消息

**症状**：Anthropic 禁止连续 user role，atomcode `merge_consecutive_user`（anthropic.rs:556-601）专门修这个。

**反例**：post-compaction summary 后追加 user 消息 → 两条连续 user → Anthropic 返回 400。

**laew 现状**：`SessionContext` 写完 summary 后直接 push user 消息——需审查。

### 坑 5：dynamic content 挪到 user 尾部

**症状**：user 消息尾部有动态内容（时间戳、git status、cwd），导致 messages 部分 cache miss。

**正解**：openclaw `cacheBreakpointOptOutMessageIndexes` 显式 opt-out（`anthropic-payload-policy.ts:241`）—— 把动态内容 message 排除在 cache_control 之外。

### 坑 6：tool schema 顺序 + 内部字段顺序

**症状**：claudecode 注释 `promptCacheBreakDetection.ts:33-37`："77% of tool breaks per BQ 2026-03-22" 来自 per-tool schema hash 变化。AgentTool/SkillTool 嵌入动态 agent/command 列表 → 每次 schema 不同。

**正解**：claudecode `perToolHashes` 记录每个 tool 的独立 hash，便于事后差异检测（**事后补救**）。

**事前预防**：动态 agent/command list 挪到 system 末尾而不是 tool schema 内部。

### 坑 7：cache_control TTL 中途翻转

**症状**：claudecode `should1hCacheTTL` 注释："mid-session overage flips from changing the cache_control TTL, which would bust the server-side prompt cache (~20K tokens per flip)"。

**正解**：latch 进 STATE（`setPromptCache1hEligible` / `setPromptCache1hAllowlist`），**只读不写**直到新 session。

### 坑 8：MCP tool 动态发现

**症状**：`globalCacheStrategy: 'tool_based' | 'system_prompt' | 'none'` —— MCP tools 发现/移除时策略翻转（`promptCacheBreakDetection.ts:42-45`）。

**正解**：laew **目前没有 MCP**，无需处理；但应预留 `cache_strategy` enum 字段。

---

## 6. token 计数实现对比表

| 仓库 | 实现方式 | 真实代码 | 准确度 | 速度 | JSON 特殊密度 | 用途 |
|---|---|---|---|---|---|---|
| **opencode** | API usage 直接读 | `mapUsage` (anthropic-messages.ts:573-585) | 100%（API 真实） | 0 延迟（响应中带回） | N/A（不重新计） | 计费、压缩触发 |
| **claudecode** | **4 级 fallback** | `countTokensWithFallback` (analyzeContext.ts:77-108) | 100% / 95% / 80% / null | API 慢 → 估算快 | ✅ JSON `/2` | 上下文分析、压缩决策 |
| **claudecode (Bedrock)** | AWS SDK CountTokens | `countTokensWithBedrock` (tokenEstimation.ts:437-495) | 100% | AWS 网络开销 | ✅ 4 | 上下文分析 |
| **pi** | LLM 真实调用读 usage | `cacheWrite1h`/`cacheWrite5m` 分别计（anthropic-messages.ts:606） | 100% | LLM 调用延迟 | N/A | post-compaction 估算 |
| **openclaw** | API usage 直接读 | `applyAnthropicCacheControlToMessages` | 100% | 0 延迟 | N/A | 计费、压缩 |
| **deepseek-harness** | **纯本地估算** | `estimate.ts:12-99` `CHARS_PER_TOKEN=4` | ~80% | O(n) 即时 | ❌ 统一 4 | 上下文预算预测 |
| **agent-core** | **3 级 fallback** | `_select_token_counter`：本地 tokenizer → tiktoken → `StringLengthCounter` | 100% / 95% / 80% | tokenizer 快 | ✅ | 精确窗口控制 |
| **hermes-agent** | API usage | `transcript_tokens` (agent/conversation_loop.py) | 100% | 0 延迟 | N/A | iteration_budget.refund |
| **atomcode** | API usage 直接读 | `TokenUsage` (anthropic.rs:760-866) | 100% | 0 延迟 | N/A | 计费 |
| **laew** | ❌ 无 | 仅读 API 返回的 cache_read/cache_creation | 100%（解析） | 0 | N/A | **仅展示**（`src/main.rs:187-193`） |

### token 计数公式对比

```typescript
// claudecode (tokenEstimation.ts L203-208)
export function roughTokenCountEstimation(content: string, bytesPerToken: number = 4): number {
  return Math.round(content.length / bytesPerToken)
}
// JSON: bytesPerToken = 2
// 文本: bytesPerToken = 4
```

```typescript
// deepseek-harness (estimate.ts L12-19)
const CHARS_PER_TOKEN = 4
const BLOCK_OVERHEAD = 4
const ROLE_OVERHEAD = 4
// estimateMessage = sum(estimateContent(blocks)) + ROLE_OVERHEAD
```

```rust
// laew 当前（无实现，需新增）
// 推荐实现（参考 claudecode）：
pub fn rough_token_count(text: &str) -> u32 {
    (text.len() as f64 / 4.0).round() as u32
}
```

**CJK 特殊密度**：所有仓库都**不处理 CJK**（`text.length` 是 UTF-8 字节数，但中文字符是 3 字节 ≈ 0.75 token 而非 0.25 token）。**这是 laew 中文场景下必须解决的问题**——建议：
- 简单方案：`text.chars().count() / 1.5`（char count / 1.5，CJK 字符约 1.5 token）
- 完整方案：引入 `tiktoken-rs` + Anthropic 官方 o200k_base

---

## 7. 上下文预算分配模型（含具体数值表）

### 预算分配对比表

| 仓库 | contextWindow 默认 | reserved output | system | tools | history | tool_result 单条 | 压缩阈值 | 触发压缩 |
|---|---|---|---|---|---|---|---|---|
| **opencode** | 模型元数据 | `outputLimit=4096` 默认 + `generation.maxTokens` | full | full | full | full（不单算） | 28 条 `isContextOverflow` 正则 | `compaction.ts` 自动 |
| **claudecode** | `MODEL_CONTEXT_WINDOW_DEFAULT=200_000` | 隐式 SDK | full | `TOOL_TOKEN_COUNT_OVERHEAD=500` 修正 | full | `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000` 单 message 聚合 | 92%（隐式） | `MAX_TOOL_RESULTS` 静默截断 + `/compact` 命令 |
| **claudecode** | Sonnet 4.6 实验性 1M | 1M beta | full | full | full | `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000` | 92% | microCompact |
| **pi** | 模型元数据 | `reserveTokens=16384` 默认 | full | full | full | full | 隐式 | `model_context_window_exceeded` → auto-compact |
| **openclaw** | 模型元数据 | `compactThreshold = max(50K, 0.7*contextWindow)` | full | full | full | full | **70%** | `cache_ttl_pruning` 工具卸载 |
| **deepseek-harness** | 模型元数据 | `compression_call_max_tokens=200000` | full | full | full | full | `thresholdRatio=0.8` | `retainRatio` / `retainTokens` |
| **atomcode** | 模型元数据 | `request.options.maxOutputTokens` | full | full | full | full | 28 条正则 | auto-compact |
| **Switchyard** | 转发模型本身 | `advisor_max_tokens: 2048` | full | full | `transcript_max_chars: 200_000` | full | 强制 | **宁报错不压缩** |
| **agent-core** | 模型元数据 | `compression_call_max_tokens=200000` | full | full | full | `ToolResultBudgetProcessor` | `BudgetGuardProcessor` | `BudgetGuardProcessor` |
| **laew** | ❌ 无常量 | ❌ 无 | full | full | full | ❌ 无 | ❌ 无 | ❌ 报错透传 |

### 压缩策略细节

| 策略 | opencode | pi | claudecode | openclaw |
|---|---|---|---|---|
| **触发** | API 返回 overflow | `model_context_window_exceeded` 错误 | `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS` 超限 | 30% contextWindow |
| **方法** | `compaction.ts` 自动 LLM 摘要 | auto-compact + branch summarization | `/compact` 命令 + microCompact | `cache_ttl_pruning` 卸载工具 |
| **保留最后 N 条** | unknown | `keepRecent: 20000` tokens | unknown | 不保留 |
| **保留 user 最后消息** | 是 | 是 | 是 | 是 |
| **压缩后 token 估算** | ✅ post-compaction | ✅ | ✅ microCompact output | N/A |

### 工具结果截断规则

| 仓库 | 单条阈值 | 单 message 阈值 | 超限行为 | 保 prompt cache 措施 |
|---|---|---|---|---|
| **claudecode** | Bash 30k / Edit 50k 默认 | `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000` | 按 `tool_use_id` 替换为文件路径 preview | 字节恒等（`mustReapply` 重放 cached preview 无 I/O） |
| **openclaw** | unknown | unknown | unknown | stable JSON ordering |
| **pi** | unknown | unknown | `cacheWrite1h`/`cacheWrite5m` 分账 | post-compaction 估算 |
| **laew** | ❌ 无 | ❌ 无 | 报错透传 | N/A |

### reserveTokens 概念详解

```rust
// pi (branch-summarization.ts L305)
const tokenBudget = contextWindow - reserveTokens;   // reserveTokens=16384 默认
//                   ↑ 摘要 LLM 的可用输入 token
```

```typescript
// Switchyard (advisor_gate.rs L144)
advisor_max_tokens: 2048,   // 单次 advisor 评审的 max output
```

```typescript
// opencode (anthropic-messages.ts L510)
const outputLimit = request.model.defaults?.limits?.output ?? request.model.route.defaults.limits?.output ?? 4096
```

**共识**：所有仓库都把"输出 token"从可用预算中扣除——典型值 4096（opencode）/ 16384（pi）/ 2048（Switchyard advisor）/ 8000~16000（OpenAI/Anthropic 主流模型 max_tokens）。

**laew 当前**：`AnthropicClient::DEFAULT_MAX_TOKENS = 8192`（`src/llm/anthropic.rs:24`），但**不参与预算分配**——无压缩触发机制。

---

## 8. 设计模式与反模式（10~15 个）

### 设计模式

#### M1：协议级断点上限 + dropped 字段

**opencode** (`protocols/anthropic-messages.ts:238` + `protocols/utils/cache.ts:5-16`)：
```typescript
const ANTHROPIC_BREAKPOINT_CAP = 4
export interface Breakpoints { remaining: number; dropped: number }
```
**优点**：超过 4 断点不抛错，只记 warning + 静默丢多余。给上游 `applyCachePolicy` 的灵活性（设 5 个 hint 让其自动收敛到 4）。

#### M2：扣减顺序 = 缓存层次

**opencode** 注释明确（`anthropic-messages.ts:511-513`）：
> "Tools live highest in the cache hierarchy, so when callers over-mark we keep their tool hints and shed the message-tail ones first."

**应用**：laew 应在 `protocols/anthropic.rs::convert_tools` 后才扣 `cache_control`。

#### M3：单次 `.slice()` 而非 `.map()`

**opencode** (`cache-policy.ts:79-83`) 注释：
> "Long conversations call this on every request, so avoid `.map()` here — its closure dispatch and identity copies show up in profiling."

**应用**：laew 在修改 message 时用 `slice()` + 索引赋值，避免整表 `.map()`。

#### M4：boundary 标记拆分 system

**openclaw** (`anthropic-payload-policy.ts:173-218`)：`<<<LAEW:STABLE>>>...<<<LAEW:DYNAMIC>>>...` 让用户标记 cache 拆分点。

**应用**：laew 在 `AgentProfile::system_prompt()` 提供 `stable_prompt()` + `dynamic_context()` 双段 API。

#### M5：latch 防止 mid-session flip

**claudecode** (`claude.ts:393-434`)：`setPromptCache1hEligible` / `setPromptCache1hAllowlist` 缓存进 STATE，会话内只读不写。

**应用**：laew `CacheState` 结构体（`src/agent/cache_state.rs`），`enabled: bool` + `ttl: "5m"|"1h"` 启动时锁定。

#### M6：4 级 fallback 链

**claudecode** (`analyzeContext.ts:77-108`)：
```
API → Haiku 真实调用 → roughToken(4/JSON 2) → null
```

**应用**：laew 实现 `TokenCounter::count()` 三级链：API → `tiktoken-rs` → `rough_token_count(text.chars().count() / 1.5)`。

#### M7：reserved output 预算分配

**pi** (`branch-summarization.ts:305`)：`tokenBudget = contextWindow - reserveTokens`，`reserveTokens=16384` 默认。

**应用**：laew 在 `src/agent/budget.rs::ContextBudget`：
```rust
pub struct ContextBudget {
    pub context_window: u32,
    pub reserved_output: u32,   // 默认 16384
    pub reserved_system: u32,
    pub reserved_tools: u32,
}
```

#### M8：hostname 白名单决定 TTL

**openclaw** (`anthropic-payload-policy.ts:138-153`)：只对 `api.anthropic.com` / `aiplatform.googleapis.com` 等白名单端点给 `1h` TTL。

**应用**：laew `CachePolicy::resolve_ttl(end_point: &str)` 检查 hostname。

#### M9：cache_ttl_pruning 主动让出空间

**openclaw** (`anthropic-payload-policy.ts:357-374`)：当 input token 达 30% 上下文窗时，**主动卸载工具**直到清掉 5%。

**应用**：laew 多 Agent 架构下，Main-Work → SubAgent 时可"卸载部分工具"。

#### M10：按调用次数而非 token

**Switchyard** (`advisor_gate.rs:101-149`)：`max_reviews` 控制 advisor 评审次数 = 直接控制 USD 成本。

**应用**：laew 多 Agent 架构下：
```rust
const PLAN_AGENT_MAX_INVOCATIONS: u32 = 1;     // Plan Agent 每会话最多 1 次
const QC_AGENT_MAX_INVOCATIONS: u32 = 1;       // QC Agent 每任务最多 1 次
const SESSION_CONTEXT_MAX_INVOCATIONS: u32 = 1;
```

#### M11：动态 import 省启动开销

**claudecode** (`tokenEstimation.ts:477`)：`const { CountTokensCommand } = await import('@aws-sdk/client-bedrock-runtime')` —— 节省 ~279KB 启动开销。

**应用**：laew `tiktoken-rs` 可放在 `lazy_static!` / `once_cell` 延迟加载。

#### M12：占位消息避免 API 报错

**claudecode** (`tokenEstimation.ts:177`): `messages: messages.length > 0 ? messages : [{ role: 'user', content: 'foo' }]` —— 测工具 schema 时必须有消息占位。

**应用**：laew `count_tokens(messages, tools)` 时若 messages 空，必须加占位。

#### M13：post-compaction token 估算

**pi** (CHANGELOG:946)：压缩后立即给客户端 `estimated post-compaction token count`。

**应用**：laew `SessionContext` 写摘要时附带 `original_tokens: u32` + `summarized_tokens: u32` 两列。

#### M14：字节稳定性 = cache 命中前提

**atomcode** (`anthropic.rs:1447-1490`)：连续 100 次序列化同一输入逐字节比对。

**应用**：laew 在 `src/llm/anthropic.rs` 加 `body_serialization_is_deterministic` 测试。

#### M15：per-source cache state 隔离

**claudecode** (`promptCacheBreakDetection.ts:107-115`)：`MAX_TRACKED_SOURCES=10`，subagent 用 `agentId` 隔离。

**应用**：laew 多 Agent 架构下，Yolo / Plan / Main-Work / SubAgent / QC 各自独立 cache state。

### 反模式警示

#### R1：把 cache_creation 当 input 计价

**claudecode** (`docs/Agent源码调研/专题/专题-第三轮-成本控制与Token统计深度分析.md:889`)：cacheWrite 1.25x input 价格，分账不区分会高估 25%。

**laew 现状**：`src/main.rs:187-193` 仅打印 cache_read，**不打印 cache_creation**，更没计价。

#### R2：忽略"prompt = input + cache_read + cache_creation"

**atomcode** (`anthropic.rs:822` 注释 + opencode `anthropic-messages.ts:566-572`)：Anthropic `input_tokens` 不含 cache，需手动 sum。

**laew 现状**：`src/agent/yolo.rs:325-330` 已正确 sum = `input + cache_read + cache_creation` ✅。

#### R3：动态时间戳/cwd 插入 system 前缀

**openclaw** (`splitSystemPromptCacheBoundary`) 反例 → 必须用 boundary 拆开。

**laew 现状**：当前 `system_prompt` 单字符串 + 无 boundary 概念。

#### R4：工具集 HashMap 顺序不稳定

**opencode** (`serde_json::Map(BTreeMap)`) 反例 → 必须用 BTreeMap 或显式 Vec。

**laew 现状**：`src/agent/tools/mod.rs::builtin_registry()` 是 Vec，但需审查顺序确定性。

#### R5：mid-session TTL flip

**claudecode** (`should1hCacheTTL`) 反例 → 必须 latch 进 STATE。

**laew 现状**：完全无 cache 概念，**埋下未来隐患**。

#### R6：cache miss 不定位

**claudecode** (`promptCacheBreakDetection` 13 维度 hash) 反例 → 必须做 hash 差异。

**laew 现状**：无差异检测手段。

#### R7：连续 user 消息

**atomcode** (`merge_consecutive_user`) 反例 → Anthropic 拒绝连续 user role。

**laew 现状**：`SessionContext` 写完 summary 直接 push user 消息，需审查。

#### R8：CJK 字符不处理

**所有仓库**都按 `text.length / 4` —— **CJK 字符 0.75 token 而非 0.25 token**。

**laew 现状**：完全无 CJK 处理。

#### R9：openai 工具结果不显式截断

**claudecode** (`MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000`) 反例 → 必须单 message 聚合上限。

**laew 现状**：`BashTool` 无输出截断，`ReadTool` / `WriteTool` 无大小检查。

#### R10：JSON 序列化顺序不确定

**atomcode** (`BTreeMap-backed`) 反例 → 必须用 BTreeMap。

**laew 现状**：`serde_json` 默认 `Map` 行为需审查。

---

## 9. laew 现状与 P0/P1/P2 路线图

### 9.1 现状盘点

#### ✅ 已解析但未利用

| 能力 | 现状 | 位置 |
|---|---|---|
| Anthropic `cache_read_input_tokens` | ✅ 解析 | `src/llm/anthropic.rs:159` (`cache_read_input_tokens`) |
| Anthropic `cache_creation_input_tokens` | ✅ 解析 | `src/llm/anthropic.rs:160` |
| OpenAI `prompt_tokens_details.cached_tokens` | ✅ 解析 | `src/llm/openai.rs:217` |
| Usage 累加（Yolo/Main-Work/Plan/SubAgent/QC） | ✅ 实现 | `src/agent/yolo.rs:325-330` + `src/agent/orchestrator.rs:632-634` |
| `-p` 单轮模式打印 cache_read | ✅ 实现 | `src/main.rs:187-193` |
| TUI 状态栏 input/output/cache_read | ✅ 显示 | 已知（`docs/Agent源码调研/专题/专题-第三轮-成本控制与Token统计深度分析.md:706`） |

#### ❌ 完全缺失

| 能力 | 缺失位置 | 优先级 |
|---|---|---|
| **发送 `cache_control`** | `src/llm/anthropic.rs::convert_tools` / `convert_messages` 无 `cache_control` 字段 | P0 |
| **`max_tokens` 显式分配** | `AnthropicRequest` 无 `max_tokens` 字段（默认 8192 hardcoded） | P0 |
| **本地 token 估算** | 完全无 | P0 |
| **上下文预算分配** | 完全无 | P1 |
| **压缩触发机制** | 完全无 | P1 |
| **强制执行（超窗兜底）** | 完全无 | P1 |
| **成本计算** | 完全无 | P2 |
| **`token_usage` SQLite 表** | 完全无 | P2 |
| **跨会话 cache 复用** | 完全无 | P2 |
| **多 Agent 调用预算** | 完全无 | P2 |
| **CJK 字符密度修正** | 完全无 | P2 |

### 9.2 P0 路线（2~4 周）—— cache_control + max_tokens + token 计数

#### P0-1：发送 `cache_control` 三断点

**目标**：在 `src/llm/anthropic.rs::convert_tools` 与 `convert_messages` 后注入 `cache_control: { type: "ephemeral" }`。

**设计**：
```rust
// src/llm/anthropic.rs 新增
const ANTHROPIC_CACHE_CONTROL_LIMIT: usize = 4;

const CACHE_CONTROL_EPHEMERAL: &str = "ephemeral";

/// Apply cache_control to the last tool, last system block, and last user message.
/// Returns (remaining, dropped) for logging.
fn apply_cache_control(
    system: &mut Option<String>,
    tools: &mut Vec<Value>,
    messages: &mut Vec<Value>,
) -> (usize, usize) {
    let mut remaining = ANTHROPIC_CACHE_CONTROL_LIMIT;
    let mut dropped = 0usize;

    // 1. last tool
    if let Some(last_tool) = tools.last_mut() {
        if remaining > 0 {
            last_tool["cache_control"] = json!({ "type": CACHE_CONTROL_EPHEMERAL });
            remaining -= 1;
        } else { dropped += 1; }
    }

    // 2. system: if multi-block array, last block
    //    laew 当前 system 是单字符串 → 暂不打，TODO 待支持多 block

    // 3. last user message 的最后 text block
    if let Some(last_msg) = messages.iter_mut().rev().find(|m| m["role"] == "user") {
        if remaining > 0 {
            if let Some(content) = last_msg["content"].as_array_mut() {
                if let Some(last_text) = content.iter_mut().rev().find(|c| c["type"] == "text") {
                    last_text["cache_control"] = json!({ "type": CACHE_CONTROL_EPHEMERAL });
                    remaining -= 1;
                }
            }
        } else { dropped += 1; }
    }

    if dropped > 0 {
        tracing::warn!("dropped {dropped} cache breakpoint(s); Anthropic allows at most {ANTHROPIC_CACHE_CONTROL_LIMIT}");
    }
    (remaining, dropped)
}
```

**调用位置**：`from_request` 序列化前（`src/llm/anthropic.rs::complete` 调用前）。

#### P0-2：`max_tokens` 显式分配

**目标**：把 `AnthropicRequest` 的 `max_tokens` 从 hardcoded `DEFAULT_MAX_TOKENS = 8192` 改为按模型元数据动态分配。

**设计**：
```rust
// src/llm/anthropic.rs 新增
const fn max_tokens_for_model(model: &str) -> u32 {
    // 来自 pi/atomcode/claudecode 的真实常数表
    if model.contains("opus-4-7") { return 32000; }      // Opus 4.7+
    if model.contains("opus-4-6") { return 32000; }      // Opus 4.6+
    if model.contains("opus") { return 8192; }
    if model.contains("sonnet") { return 8192; }
    if model.contains("haiku") { return 8192; }
    DEFAULT_MAX_TOKENS
}
```

#### P0-3：本地 token 估算（3 级 fallback）

**目标**：`src/agent/budget.rs::TokenCounter` 实现三级链。

**设计**：
```rust
// src/agent/budget.rs 新增
pub struct TokenCounter;

impl TokenCounter {
    /// 三级 fallback 链：
        // 1. 真实调用 /count_tokens（可选，外部 LLM API）
        // 2. tiktoken-rs（o200k_base）
        // 3. 字符串长度估算（CJK 修正）
    pub fn count(text: &str) -> u32 {
        // 第 3 级：CJK 字符密度修正
        let cjk_count = text.chars().filter(|c| {
            let cp = *c as u32;
            (0x4E00..=0x9FFF).contains(&cp)              // CJK Unified Ideographs
                || (0x3040..=0x309F).contains(&cp)         // Hiragana
                || (0x30A0..=0x30FF).contains(&cp)         // Katakana
                || (0xAC00..=0xD7AF).contains(&cp)         // Hangul
        }).count();
        let ascii_count = text.chars().count() - cjk_count;
        // CJK ~1.5 token/char, ASCII ~0.25 token/char
        (cjk_count as f64 * 1.5 + ascii_count as f64 * 0.25).ceil() as u32
    }

    /// 第 1 级：调用 Anthropic count_tokens API（可选）
    pub async fn count_with_api(text: &str, end_point: &str, api_key: &str, model: &str) -> Result<u32> {
        // POST {end_point}/v1/messages/count_tokens
        // ...
    }
}
```

**Cargo.toml 新增依赖**：
```toml
[dependencies]
tiktoken-rs = "0.7"   # 可选，第 2 级（如果 o200k_base 可用）
```

### 9.3 P1 路线（4~8 周）—— 预算分配 + 压缩触发 + 强制执行

#### P1-1：上下文预算分配常量表

**目标**：`src/agent/budget.rs::ContextBudget` 引入 model → contextWindow 映射。

**设计**：
```rust
// src/agent/budget.rs 新增
pub struct ContextBudget {
    pub context_window: u32,
    pub reserved_output: u32,           // 默认 16384
    pub reserved_system_estimate: u32,  // 默认 8192（按 system_prompt 长度估算）
    pub reserved_tools_estimate: u32,   // 默认 4096（按 builtin tools 估算）
    pub reserved_history_estimate: u32, // 当前 history 已用
    pub reserved_tool_result_estimate: u32,
}

impl ContextBudget {
    pub fn for_model(model: &str) -> Self {
        let context_window = Self::context_window_for(model);
        Self {
            context_window,
            reserved_output: 16384,
            ..Default::default()
        }
    }

    /// 模型 → 上下文窗 映射（参考 pi CHANGELOG.md 的真实数值）
    pub fn context_window_for(model: &str) -> u32 {
        if model.contains("opus-4-6") && (model.contains("1m") || model.contains("1M")) { return 1_000_000; }
        if model.contains("opus") { return 200_000; }
        if model.contains("sonnet") && model.contains("1m") { return 1_000_000; }   // Sonnet 4.6 实验性 1M
        if model.contains("sonnet") { return 200_000; }
        if model.contains("haiku") { return 200_000; }
        // OpenAI
        if model.contains("gpt-5-codex") { return 272_000; }   // Codex 后端 272K
        if model.contains("gpt-5") { return 272_000; }
        if model.contains("o3") || model.contains("o4") { return 200_000; }
        if model.contains("gpt-4") { return 128_000; }
        // 默认
        200_000
    }

    /// 是否超窗（宁报错兜底）
    pub fn is_overflow(&self) -> bool {
        self.reserved_system_estimate
            + self.reserved_tools_estimate
            + self.reserved_history_estimate
            + self.reserved_tool_result_estimate
            + self.reserved_output
            >= self.context_window
    }

    /// 触发压缩阈值（pi 0.8，openclaw 0.7）
    pub fn needs_compact(&self) -> bool {
        self.is_overflow() || self.usage_ratio() >= 0.92
    }

    fn usage_ratio(&self) -> f64 {
        let used = self.reserved_system_estimate
            + self.reserved_tools_estimate
            + self.reserved_history_estimate
            + self.reserved_tool_result_estimate;
        used as f64 / self.context_window as f64
    }
}
```

#### P1-2：工具结果截断

**目标**：参考 claudecode `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS = 200_000`。

**设计**：
```rust
// src/agent/tools/mod.rs 新增
pub const MAX_TOOL_RESULT_BYTES: usize = 30_000;       // 单条工具结果上限
pub const MAX_TOOL_RESULTS_PER_MESSAGE_BYTES: usize = 200_000;   // 单 message 聚合上限

/// 工具结果截断：超限按 tool_use_id 替换为文件路径 preview
pub fn truncate_tool_result(tool_use_id: &str, content: &str) -> String {
    if content.len() <= MAX_TOOL_RESULT_BYTES { return content.to_string(); }
    // 持久化到 ~/.laew/<session_id>/tool-results/<tool_use_id>.txt
    let path = persistence_path_for(tool_use_id);
    std::fs::write(&path, content).ok();
    format!(
        "[Tool result truncated. Full output saved to {}]\nPreview: {}...",
        path.display(),
        &content[..MAX_TOOL_RESULT_BYTES.min(content.len())]
    )
}
```

#### P1-3：压缩触发机制

**目标**：当 `ContextBudget::needs_compact()` 为 true 时，调用 SessionContext Agent 生成摘要。

**设计**：
```rust
// src/agent/session_context.rs 新增
impl SessionContext {
    pub async fn compact_if_needed(&self, budget: &ContextBudget) -> Result<Option<Usage>> {
        if !budget.needs_compact() { return Ok(None); }
        // 调用 SessionContext Agent 生成摘要
        let summary = self.summarize().await?;
        Ok(Some(summary.usage))
    }
}
```

#### P1-4：强制执行（宁报错 vs 自动压缩）

**目标**：根据 `provider_record.protocol` 决定：Anthropic → 自动压缩；OpenAI → 宁报错透传（参考 Switchyard）。

**设计**：
```rust
// src/agent/mod.rs 新增
pub async fn enforce_budget(
    budget: &ContextBudget,
    overflow_strategy: OverflowStrategy,
) -> Result<()> {
    if !budget.is_overflow() { return Ok(()); }
    match overflow_strategy {
        OverflowStrategy::AutoCompact => {
            session_context::compact_if_needed(budget).await?;
            Ok(())
        }
        OverflowStrategy::FailLoud => Err(AgentError::Llm("Context window exceeded; please /compact or /new".into())),
    }
}

pub enum OverflowStrategy {
    AutoCompact,    // Anthropic / pi 默认
    FailLoud,       // Switchyard 风格
}
```

### 9.4 P2 路线（8~16 周）—— 成本归因 + SQLite + 多 Agent 预算

#### P2-1：`token_usage` SQLite 表

**目标**：每次 LLM 调用的 usage 落到 SQLite，按 model/provider/session 聚合。

**设计**：
```sql
-- migration: add token_usage table
CREATE TABLE token_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,           -- Yolo / Plan / Main-Work / SubAgent / QC / SessionContext
    provider_name TEXT NOT NULL,
    model_name TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd_micros INTEGER,            -- 微美分
    created_at INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);
CREATE INDEX idx_token_usage_session ON token_usage(session_id);
CREATE INDEX idx_token_usage_model ON token_usage(model_name, created_at);
```

**参考**：opencode `modelUsage` 聚合（`cost-tracker.ts:250-276`）+ cc-switch `proxy/usage/logger.rs:172-176` SQLite 持久化。

#### P2-2：多 Agent 调用次数预算（学 Switchyard）

**目标**：限制 Plan Agent 调 1 次、QC Agent 调 1 次、SessionContext 调 1 次。

**设计**：
```rust
// src/agent/orchestrator.rs 新增
const PLAN_AGENT_MAX_INVOCATIONS_PER_SESSION: u32 = 1;
const QC_AGENT_MAX_INVOCATIONS_PER_TASK: u32 = 1;
const SESSION_CONTEXT_MAX_INVOCATIONS: u32 = 1;

pub struct AgentBudgetTracker {
    scopes: HashMap<ScopeKey, ScopeState>,
}

impl AgentBudgetTracker {
    pub fn try_reserve(&mut self, agent: AgentName, scope: ScopeKey) -> bool {
        let max = match agent {
            AgentName::Plan => PLAN_AGENT_MAX_INVOCATIONS_PER_SESSION,
            AgentName::QualityCheck => QC_AGENT_MAX_INVOCATIONS_PER_TASK,
            AgentName::SessionContext => SESSION_CONTEXT_MAX_INVOCATIONS,
            _ => u32::MAX,
        };
        let entry = self.scopes.entry(scope).or_default();
        if entry.invocations >= max { return false; }
        entry.invocations += 1;
        true
    }
}
```

#### P2-3：成本计算 + 状态栏展示

**目标**：参考 opencode `centsToMicroCents` + `formatTotalCost()`，laew 在 TUI 状态栏显示本次会话累计成本。

**设计**：
```rust
// src/llm/pricing.rs 新增（按 model 查单价表）
pub fn model_cost(model: &str, usage: &Usage) -> f64 {
    let rates = rates_for(model);   // 输入/输出/cache_read/cache_creation 单价
    (usage.input_tokens as f64 / 1_000_000.0) * rates.input
        + (usage.output_tokens as f64 / 1_000_000.0) * rates.output
        + (usage.cache_read_input_tokens as f64 / 1_000_000.0) * rates.cache_read
        + (usage.cache_creation_input_tokens as f64 / 1_000_000.0) * rates.cache_creation
}
```

#### P2-4：CJK 字符密度修正 + `tiktoken-rs`

**目标**：引入 `tiktoken-rs`（或 `tokenizers`）做精确计数；CJK 修正字符密度。

**设计**：
```rust
// src/agent/budget.rs 新增
pub struct PreciseTokenCounter {
    encoder: Option<tiktoken_rs::CoreBPE>,   // o200k_base
}

impl PreciseTokenCounter {
    pub fn new() -> Self {
        let encoder = tiktoken_rs::o200k_base().ok();   // 失败回退到字符串估算
        Self { encoder }
    }

    pub fn count(&self, text: &str) -> u32 {
        match &self.encoder {
            Some(enc) => enc.encode_ordinary(text).len() as u32,
            None => TokenCounter::count(text),   // fallback
        }
    }
}
```

### 9.5 关键决策点

| 决策点 | 选项 | 推荐 | 理由 |
|---|---|---|---|
| Cache TTL | `5m` 默认 vs `1h` 用户可选 | **5m 默认** + 用户可开关 | laew 单次会话较短，5m 足够覆盖；1h 需 hostname 白名单 |
| 压缩触发 | 92% vs 95% vs 80% | **92%** | pi 0.8 太激进；Switchyard 100% 太晚；claudecode 92% 平衡 |
| overflow 处理 | AutoCompact vs FailLoud | **Anthropic AutoCompact / OpenAI FailLoud** | Anthropic 1m 上下文便宜可压缩；OpenAI 按 token 贵，宁早报错 |
| Token 计数 | 估算 vs API vs tiktoken-rs | **估算 (P0) → tiktoken-rs (P2)** | 估算零依赖立刻上线；tiktoken-rs 需 ~1MB 二进制 |
| 多 Agent 预算 | 计数限制 vs 时间限制 | **计数限制** | Switchyard 验证；laew 多 Agent 适合直接搬 |

### 9.6 反模式警示（laew 专项）

| 反模式 | 风险 | 规避 |
|---|---|---|
| 在 system 提示词中加时间戳/cwd | cache 永远 miss | 用 `dynamic_context()` 单独 API |
| `Vec<ToolDef>` 顺序不稳定 | cache 永远 miss | 在 `builtin_registry()` 内显式 Vec + 注释保证顺序 |
| `serde_json::Map` 默认行为 | HashMap iteration 不确定 → cache miss | 改 `serde_json::Map<BTreeMap>` 或 `Vec<(String, Value)>` |
| `cache_control` 重复打 | `dropped` 计数器累加 | opencode 风格：先 check `'cache' in existing` |
| `cache_control` TTL 中途翻 | mid-session flip 损坏 cache | latch 进 STATE（`src/agent/cache_state.rs`） |
| 工具结果不截断 | 单条超 30k → 单 message 聚合超 200k → 422 | 参考 claudecode `MAX_TOOL_RESULT_BYTES=30_000` |
| 中文字符按 `/4` 算 | 实际偏高估 ~6x | CJK 修正：`text.chars().count() / 1.5` |

---

## 10. 关键文件速查

### 10.1 真实源码路径

#### opencode（TypeScript/Bun + Effect DI）
- `/usr/local/LsmGitOpenSource/opencode/packages/llm/src/cache-policy.ts` —— **断点放置算法（最完整）**
- `/usr/local/LsmGitOpenSource/opencode/packages/llm/src/protocols/utils/cache.ts` —— **协议级断点上限 + TTL bucket**
- `/usr/local/LsmGitOpenSource/opencode/packages/llm/src/protocols/anthropic-messages.ts:238 + 514 + 573-585` —— 协议常量 + `mapUsage` 归一
- `/usr/local/LsmGitOpenSource/opencode/packages/llm/src/provider-error.ts:4-43` —— **28 条 context overflow 正则**
- `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/session/compaction.ts:451` —— 压缩触发

#### claudecode（TypeScript）
- `/usr/local/LsmGitOpenSource/claudecode/src/services/api/claude.ts:358-434` —— `getCacheControl` + `should1hCacheTTL` latch
- `/usr/local/LsmGitOpenSource/claudecode/src/services/api/promptCacheBreakDetection.ts:1-200` —— **13 维度 hash 差异检测**
- `/usr/local/LsmGitOpenSource/claudecode/src/services/tokenEstimation.ts:124-495` —— **4 级 token 计数 fallback**
- `/usr/local/LsmGitOpenSource/claudecode/src/utils/analyzeContext.ts:77-108` —— fallback 链 + `TOOL_TOKEN_COUNT_OVERHEAD=500`
- `/usr/local/LsmGitOpenSource/claudecode/src/constants/toolLimits.ts:49` —— `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS=200_000`

#### pi（TypeScript）
- `/usr/local/LsmGitOpenSource/pi/packages/ai/src/api/anthropic-messages.ts:64-78 + 975-1363` —— **3 断点 explicit + 模型能力路由**
- `/usr/local/LsmGitOpenSource/pi/packages/ai/src/types.ts:615-678` —— **5 个 cache 相关 compat 标志**
- `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/compaction/branch-summarization.ts:300-318` —— `reserveTokens=16384`
- `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/compaction/compaction.ts:128` —— `CompactionConfig`

#### openclaw（TypeScript）
- `/usr/local/LsmGitOpenSource/openclaw/packages/ai/src/transports/anthropic-payload-policy.ts:43-374` —— **完整 payload policy（boundary + 计数 + pruning）**
  - `:138-153` `isLongTtlEligibleEndpoint` —— **hostname 白名单**
  - `:156-171` `resolveAnthropicEphemeralCacheControl` —— TTL 选择
  - `:173-218` `applyAnthropicCacheControlToSystem` —— **boundary 拆分**
  - `:220-234` `stripSystemPromptCacheBoundary` —— 发送前清除
  - `:236-310` `applyAnthropicCacheControlToMessages` —— 反向遍历
  - `:312-324` `countAnthropicCacheControlMarkers` —— 计数校验
  - `:357-374` `cache_ttl_pruning` 工具卸载

#### deepseek-harness（TypeScript）
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/token-meter/src/estimate.ts:12-99` —— **固定密度启发式（CHARS_PER_TOKEN=4）**
- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/llm/token-meter/src/breakdown-projection.ts` —— O(1) 表面投影

#### atomcode（Rust）
- `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/anthropic.rs:760-866` —— `TokenUsage` + `mapUsage`
- `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/anthropic.rs:556-601` —— `merge_consecutive_user` 协议修正
- `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/provider/anthropic.rs:1447-1490` —— `body_serialization_is_deterministic` 测试

#### Switchyard（Rust LLM 网关）
- `/usr/local/LsmGitOpenSource/Switchyard/crates/libsy/src/algorithms/advisor_gate.rs:102-149` —— `max_reviews` 配置
- `/usr/local/LsmGitOpenSource/Switchyard/crates/libsy/src/algorithms/advisor_gate.rs:160-175 + 612-630` —— 预算 scope 优先级
- `/usr/local/LsmGitOpenSource/Switchyard/crates/libsy-llm-client/src/client.rs` —— cache_control 注入

#### cc-switch（Tauri 2 + Rust）
- `/usr/local/LsmGitOpenSource/cc-switch/src-tauri/src/proxy/usage/logger.rs:22-209` —— **4 字段 fallback + SQLite 持久化**
- `/usr/local/LsmGitOpenSource/cc-switch/src-tauri/src/provider.rs:470-471 + 988` —— `costMultiplier` 字段

### 10.2 laew 当前源码

- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/llm/anthropic.rs:24` —— `DEFAULT_MAX_TOKENS = 8192`
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/llm/anthropic.rs:154-162` —— `message_start` 解析 cache_read / cache_creation
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/llm/openai.rs:213-218` —— OpenAI `prompt_tokens_details.cached_tokens` 解析
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/llm/sse.rs:177-247` —— `DeltaEvent::InputUsage { input_tokens, cache_read, cache_creation }` + `ParseSink`
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/agent/yolo.rs:323-330` —— Yolo Usage 累加
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/agent/orchestrator.rs:632-634` —— Main-Work Usage 累加
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/main.rs:187-193` —— `-p` 模式打印 cache_read
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/agent/tools/mod.rs` —— 工具注册（需审查顺序稳定性）

### 10.3 本专题关键路径（一句话总览）

- **断点上限常量**：`opencode::packages/llm/src/protocols/utils/cache.ts:10` (`newBreakpoints(4)`)
- **断点放置主入口**：`opencode::packages/llm/src/cache-policy.ts:99` (`applyCachePolicy`)
- **TTL 选择**：`opencode::packages/llm/src/protocols/utils/cache.ts:15` (`ttlBucket`)
- **boundary 拆分**：`openclaw::packages/ai/src/transports/anthropic-payload-policy.ts:173` (`applyAnthropicCacheControlToSystem`)
- **latch 防 flip**：`claudecode::src/services/api/claude.ts:393` (`should1hCacheTTL`)
- **hash 差异检测**：`claudecode::src/services/api/promptCacheBreakDetection.ts:28-69` (`PreviousState`)
- **4 级 token fallback**：`claudecode::src/utils/analyzeContext.ts:77` (`countTokensWithFallback`)
- **固定密度估算**：`deepseek-harness::packages/llm/token-meter/src/estimate.ts:12` (`CHARS_PER_TOKEN=4`)
- **预算分配常量**：`pi::packages/coding-agent/src/core/compaction/branch-summarization.ts:305` (`reserveTokens=16384`)
- **压缩触发阈值**：`openclaw::packages/ai/src/transports/anthropic-payload-policy.ts:44` (`ANTHROPIC_COMPACT_THRESHOLD_MIN=50_000`)
- **调用次数预算**：`Switchyard::crates/libsy/src/algorithms/advisor_gate.rs:113` (`max_reviews`)
- **字节稳定性测试**：`atomcode::crates/atomcode-capabilities/src/provider/anthropic.rs:1447` (`body_serialization_is_deterministic`)

---

## 附录：专题与已有文档差异化对照

| 已有专题 | 已覆盖 | 本专题新增 |
|---|---|---|
| 专题-第三轮-成本控制与Token统计深度分析.md | 价格表结构、四类 token 区分、缓存折扣价、分层定价、cache miss 浪费量化、审查预算 | **断点放置算法 + 字节稳定性 + 4 级 token fallback + boundary 拆分 + TTL latch** |
| 专题-第六轮-Anthropic与OpenAI协议调用真实实现深度对比.md | Cache 断点上限（仅一段） | **完整 14 行横向对比表 + 8 大失效坑 + 字节稳定性 + 13 维度 hash 检测 + reserved output 模型** |
| 专题-Context上下文管理深度分析.md | 4 级压缩管线 | **断点放置 vs 压缩触发的交互 + budget 分配数值表 + openclaw 工具卸载策略** |
| 专题-第五轮-工具结果回填与消息组装深度分析.md | 工具结果截断 | **单 message 聚合上限 + 持久化路径 + mustReapply 字节恒等** |
| 专题-第三轮-系统提示词工程真实对比深度分析.md | DYNAMIC_BOUNDARY 概念 | **openclaw 显式 boundary 标记实现代码 + CJK 修正 + 多 Agent 预算** |
| claudecode.md / atomcode.md / openclaw.md / pi.md | 各仓综合 | **纯 P0/P1/P2 路线图 + Rust `tiktoken-rs` 设计 + laew 字段级埋点代码** |

**总计约 2100 行**，完整覆盖本专题定位的 7 个分析维度与 15 个真实源码锚点。