# 成本控制与 Token 统计深度分析（第三轮专题）

> 专题定位：全新专属维度——聚焦 7 个开源 Agent 仓库（opencode / claudecode / pi / openclaw / deepseek-harness / atomcode / Switchyard）在 **Token 计数、价格数据源、成本计算、上下文预算、预算限额、成本呈现 UI、省钱机制** 7 个维度的横向深度对比，并给出 laew（Rust 多 Agent CLI）的针对性借鉴路线。
>
> 独特价值：知识库此前仅泛提「Cache Control 自动注入可省 50-80%」，本文深挖各仓**成本计算代码本身**——价格表结构、四类 token 区分、缓存折扣价、分层定价、cache miss 浪费量化、审查预算控成本等全新细节。

---

## 目录

1. [各仓逐个深挖](#1-各仓逐个深挖)
   - 1.1 opencode（最完整的 Stripe 计费体系）
   - 1.2 claudecode（写死价格表 + 上下文百分比）
   - 1.3 pi（cache miss 浪费量化 + token-plan）
   - 1.4 openclaw（分层定价 + cacheWrite1h 子集）
   - 1.5 deepseek-harness（纯本地 token 估算 + 表面投影）
   - 1.6 atomcode（三元组 token 统计，零成本计算）
   - 1.7 Switchyard（审查预算 = 成本控制）
2. [横向对比总表](#2-横向对比总表)
3. [可复用设计模式（8 个）](#3-可复用设计模式8-个)
4. [laew 借鉴路线](#4-laew-借鉴路线)
5. [总结与反模式警示](#5-总结与反模式警示)

---

## 1. 各仓逐个深挖

### 1.1 opencode（TypeScript/Bun）——最完整的 Stripe 计费体系

opencode 拥有本次调研中**最完整的端到端成本系统**：从远程模型目录获取价格 → 四类 token 区分 → 分层/峰值定价 → 微美分精度计费 → Redis 批量写入 → Stripe 支付扣款 → 多 provider 预算追踪 → 按模型/按周聚合统计。

#### 1.1.1 Token 计数：四类 token 严格区分

`packages/console/core/src/schema/billing.sql.ts:114-136` 的 `UsageTable` 定义了完整的 token 字段：

```sql
input_tokens            int  -- 普通输入
output_tokens           int  -- 输出
reasoning_tokens        int  -- 推理（独立字段）
cache_read_tokens       int  -- 缓存读取（折扣价）
cache_write_5m_tokens   int  -- 5 分钟缓存写入
cache_write_1h_tokens   int  // 1 小时缓存写入
cost                    bigint -- 微美分计价值
```

**关键设计**：`cacheWrite5m` 与 `cacheWrite1h` 分开统计——1h 缓存的价格是 5m 的 2 倍（见 1.1.3），必须区分。

#### 1.1.2 价格数据源：远程 models.dev 目录 + 峰值定价

`packages/console/core/src/model.ts:14-50` 的 `ModelCostSchema` 定义价格结构：

```typescript
const ModelCostSchema = z.object({
  input: z.number(),
  output: z.number(),
  cacheRead: z.number().optional(),
  cacheWrite5m: z.number().optional(),
  cacheWrite1h: z.number().optional(),
})
```

**价格来源**：`packages/console/core/src/models-dev.ts:160-165` 从远程 `https://models.opencode.ai/api.json` 拉取，本地缓存 5 分钟 TTL，每 60 分钟后台刷新：

```typescript
const source = Flag.OPENCODE_MODELS_URL || "https://models.opencode.ai"
const ttl = Duration.minutes(5)
// Schedule.spaced runs the effect once, then waits between completions.
yield* Effect.forkScoped(refresh().pipe(Effect.repeat(Schedule.spaced("60 minutes")), Effect.ignore))
```

**峰值定价**：`packages/console/app/src/routes/zen/util/pricing.ts:1-11` 实现 DeepSeek 中国时区峰值加价：

```typescript
export function isPeakPricing(date: Date) {
  // DeepSeek peak pricing in China Standard Time (UTC+8):
  // - Weekdays only; 9 AM to noon; 2 PM to 6 PM
  const dateCN = new Date(date.getTime() + 8 * 3_600 * 1_000)
  const dayCN = dateCN.getUTCDay()
  if (dayCN === 0 || dayCN === 6) return false
  const hourCN = dateCN.getUTCHours()
  return (hourCN >= 9 && hourCN < 12) || (hourCN >= 14 && hourCN < 18)
}
```

#### 1.1.3 成本计算：微美分精度 + 分层定价

`packages/console/app/src/routes/zen/util/handler.ts:1002-1041` 的 `calculateCost` 是核心：

```typescript
function calculateCost(modelInfo: ModelInfo, usageInfo: UsageInfo) {
  const { inputTokens, outputTokens, reasoningTokens, cacheReadTokens, cacheWrite5mTokens, cacheWrite1hTokens } = usageInfo

  const modelCost =
    modelInfo.costPeak && isPeakPricing(new Date())
      ? modelInfo.costPeak
      : modelInfo.cost200K &&
          inputTokens + (cacheReadTokens ?? 0) + (cacheWrite5mTokens ?? 0) + (cacheWrite1hTokens ?? 0) > 200_000
        ? modelInfo.cost200K
        : modelInfo.cost

  const inputCost = modelCost.input * inputTokens * 100
  const outputCost = modelCost.output * outputTokens * 100
  const cacheReadCost = modelCost.cacheRead * cacheReadTokens * 100
  const cacheWrite5mCost = modelCost.cacheWrite5m * cacheWrite5mTokens * 100
  const cacheWrite1hCost = modelCost.cacheWrite1h * cacheWrite1hTokens * 100
  const totalCostInCent = inputCost + outputCost + (cacheReadCost ?? 0) + (cacheWrite5mCost ?? 0) + (cacheWrite1hCost ?? 0)
  return { totalCostInCent, inputCost, outputCost, cacheReadCost, cacheWrite5mCost, cacheWrite1hCost }
}
```

**精度链**：`packages/console/core/src/util/price.ts` 用**微美分**（1 美元 = 10^8 微美分）避免浮点误差：

```typescript
export function centsToMicroCents(amount: number) {
  return Math.round(amount * 1000000)
}
export function microCentsToCents(amount: number) {
  return Math.round(amount / 1000000)
}
```

**分层定价**：当 `inputTokens + cacheTokens > 200K` 时切换到 `cost200K` 档位——大上下文通常单价更低。

#### 1.1.4 预算与限额：多 provider 优先级预算

`packages/console/app/src/routes/zen/util/providerBudgetTracker.ts:14-150` 实现**按 provider + 优先级 + 分钟级**的预算追踪：

```typescript
// Per-provider, per-minute budget with priorities.
// priority 1 ("always") routes unconditionally
// higher priorities ("fill") only route while under budget
export function createProviderBudgetTracker(providers: {
  id: string; budget?: number; budgetContribution?: number; budgetPriority?: number
}[]) { ... }
```

- `check()`：读 Redis 当前分钟 + 上一分钟花费，计算 `effectiveBudget = budget - 上一分钟高优先级已用`
- `track()`：按 `budgetContribution` 比例累加，Redis pipeline incrby + expire(120s)

**限额层级**（`handler.ts:811-989`）：workspace 月限额 → user 月限额 → subscription 周/滚动限额 → lite 周/月/滚动限额，超限抛 `MonthlyLimitError` / `BlackUsageLimitError` 等。

#### 1.1.5 省钱机制：Redis 批量写入 + 自动 cache 注入

**Redis 批量**：`packages/console/app/src/routes/zen/util/usageBatcher.ts:12-31` 对热点 workspace 用 Redis 累加，~1% 概率刷库：

```typescript
const FLUSH_PROBABILITY = 1 / 100  // ~1% of requests write
await Promise.all([redis.incrby(wKey, workspaceCost), redis.incrby(uKey, userCost)])
if (Math.random() > FLUSH_PROBABILITY) return null
const [workspaceTotal, userTotal] = await Promise.all([redis.getdel(wKey), redis.getdel(uKey)])
```

**自动 cache 注入**：`packages/llm/src/cache-policy.ts:18-42` 默认 `"auto"` 策略在**最后一个 tool 定义、最后一个 system part、最新 user message** 三个断点注入 `cache_control`：

```typescript
const AUTO: CachePolicyObject = {
  tools: true, system: true, messages: "latest-user-message",
}
// 解析：Anthropic 5m-cache write 是 1.25x base, read 是 0.1x,
// 所以 a single reuse within 5 minutes already wins.
```

#### 1.1.6 成本呈现：/stats 命令 + 按模型/按工具聚合

`packages/opencode/src/cli/cmd/stats.ts:292-384` 的 `displayStats` 输出完整成本表：

```
┌────────────────────────────────────────────────────────┐
│                    COST & TOKENS                       │
├────────────────────────────────────────────────────────┤
│Total Cost            $12.34                            │
│Avg Cost/Day          $1.23                             │
│Input                 1.2M                              │
│Cache Read            800.0K                            │
└────────────────────────────────────────────────────────┘
```

支持 `--days` / `--tools` / `--models` 过滤，按 `providerID/modelID` 聚合 modelUsage。

**运维统计脚本**：`packages/console/core/script/black-stats.ts` 按 Black 订阅 plan 输出 CSV，含每周 sub/ppu 花费 + input/cache/output token 三列。

---

### 1.2 claudecode（TypeScript）——写死价格表 + 上下文百分比

claudecode 的成本系统**简洁实用**：写死价格表 + 按模型聚合 + 上下文百分比提示 + 退出时持久化。

#### 1.2.1 价格数据源：按 tier 分组的写死常量

`src/utils/modelCost.ts:36-126` 定义 6 个价格 tier：

```typescript
// Standard pricing tier for Sonnet: $3 input / $15 output per Mtok
export const COST_TIER_3_15 = {
  inputTokens: 3, outputTokens: 15,
  promptCacheWriteTokens: 3.75, promptCacheReadTokens: 0.3,
  webSearchRequests: 0.01,
}
export const COST_TIER_15_75 = { inputTokens: 15, outputTokens: 75, ... }   // Opus 4/4.1
export const COST_TIER_5_25  = { inputTokens: 5, outputTokens: 25, ... }    // Opus 4.5
export const COST_TIER_30_150 = { inputTokens: 30, outputTokens: 150, ... } // Opus 4.6 fast
export const COST_HAIKU_35  = { inputTokens: 0.8, outputTokens: 4, ... }    // Haiku 3.5
export const COST_HAIKU_45  = { inputTokens: 1, outputTokens: 5, ... }      // Haiku 4.5

export const MODEL_COSTS: Record<ModelShortName, ModelCosts> = {
  "claude-3-5-sonnet": COST_TIER_3_15,
  "claude-sonnet-4-5":  COST_TIER_3_15,
  "claude-opus-4":      COST_TIER_15_75,
  "claude-haiku-4-5":    COST_HAIKU_45,
  ...
}
```

**关键细节**：
- `promptCacheReadTokens = 0.3`（基础价 3 的 1/10）——缓存读取折扣
- `promptCacheWriteTokens = 3.75`（基础价 3 的 1.25 倍）——缓存写入溢价
- `webSearchRequests: 0.01`——Web 搜索单独计费
- **未知模型兜底**：`DEFAULT_UNKNOWN_MODEL_COST = COST_TIER_5_25`，并设 `hasUnknownModelCost` 标志

#### 1.2.2 成本计算：按 Mtok 计价

`src/utils/modelCost.ts:131-142`：

```typescript
function tokensToUSDCost(modelCosts: ModelCosts, usage: Usage): number {
  return (
    (usage.input_tokens / 1_000_000) * modelCosts.inputTokens +
    (usage.output_tokens / 1_000_000) * modelCosts.outputTokens +
    ((usage.cache_read_input_tokens ?? 0) / 1_000_000) * modelCosts.promptCacheReadTokens +
    ((usage.cache_creation_input_tokens ?? 0) / 1_000_000) * modelCosts.promptCacheWriteTokens +
    (usage.server_tool_use?.web_search_requests ?? 0) * modelCosts.webSearchRequests
  )
}
```

#### 1.2.3 Token 计数：API usage 字段直接累加

`src/cost-tracker.ts:250-276` 的 `addToTotalModelUsage` 把每次 API 返回的 `BetaUsage` 累加到 per-model 聚合：

```typescript
modelUsage.inputTokens += usage.input_tokens
modelUsage.outputTokens += usage.output_tokens
modelUsage.cacheReadInputTokens += usage.cache_read_input_tokens ?? 0
modelUsage.cacheCreationInputTokens += usage.cache_creation_input_tokens ?? 0
modelUsage.webSearchRequests += usage.server_tool_use?.web_search_requests ?? 0
modelUsage.costUSD += cost
```

**Advisor 递归计费**：`cost-tracker.ts:304-321` 处理 advisor 工具内嵌套调用的 token——递归调用 `addToTotalSessionCost`，并以 `cost_usd_micros` 上报分析事件。

#### 1.2.4 上下文预算：百分比计算 + 自动压缩联动

`src/utils/context.ts:118-144`：

```typescript
export function calculateContextPercentages(
  currentUsage: { input_tokens: number; cache_creation_input_tokens: number; cache_read_input_tokens: number } | null,
  contextWindowSize: number,
): { used: number | null; remaining: number | null } {
  const totalInputTokens =
    currentUsage.input_tokens + currentUsage.cache_creation_input_tokens + currentUsage.cache_read_input_tokens
  const usedPercentage = Math.round((totalInputTokens / contextWindowSize) * 100)
  return { used: clampedUsed, remaining: 100 - clampedUsed }
}
```

**上下文窗口**：默认 200K（`MODEL_CONTEXT_WINDOW_DEFAULT = 200_000`），支持 `[1m]` 后缀 / 1M beta / Sonnet 4.6 实验性 1M。

#### 1.2.5 成本呈现：退出时持久化 + 状态栏

`src/costHook.ts:6-22` 在进程退出时打印总花费并持久化到项目配置：

```typescript
export function useCostSummary(getFpsMetrics?: () => FpsMetrics | undefined): void {
  useEffect(() => {
    const f = () => {
      if (hasConsoleBillingAccess()) {
        process.stdout.write('\n' + formatTotalCost() + '\n')
      }
      saveCurrentSessionCosts(getFpsMetrics?.())
    }
    process.on('exit', f)
    return () => { process.off('exit', f) }
  }, [])
}
```

`formatTotalCost()` 输出：`Total cost: $X.XX` + `Total duration (API/Wall)` + `Total code changes` + `Usage by model`（按短名聚合）。

---

### 1.3 pi（TypeScript）——cache miss 浪费量化 + token-plan

pi 的成本系统**最具诊断特色**：不只算花了多少钱，还量化「cache miss 浪费了多少钱」。

#### 1.3.1 Token 计数：Usage 标准结构

`packages/coding-agent/src/core/usage-totals.ts:4-10`：

```typescript
export interface UsageTotals {
  input: number; output: number; cacheRead: number; cacheWrite: number; cost: number;
}
```

#### 1.3.2 成本计算：按模型分组 + 工具/摘要单独桶

`usage-totals.ts:37-70` 的 `getUsageCostBreakdown` 把 usage 按 `provider/responseModel` 分组，**工具调用和摘要合并到 "Tools/summaries" 桶**：

```typescript
if (entry.type === "message" && entry.message.role === "assistant") {
  key = `${entry.message.provider}/${entry.message.responseModel ?? entry.message.model}`
  usage = entry.message.usage
} else if (entry.type === "message" && entry.message.role === "toolResult" && entry.message.usage) {
  key = "Tools/summaries"
  usage = entry.message.usage
}
```

#### 1.3.3 省钱机制：cache miss 浪费量化（独特）

`packages/coding-agent/src/core/cache-stats.ts:56-90` 的 `detectMiss` 是本次调研最精巧的省钱诊断：

```typescript
function detectMiss(prev: PreviousRequest | undefined, message: AssistantMessage, models: ModelPriceSource): CacheMiss | undefined {
  const usage = message.usage
  const promptTokens = usage.input + usage.cacheRead + usage.cacheWrite
  if (!prev || promptTokens <= 0 || (usage.cacheRead + usage.cacheWrite === 0 && !prev.reportedCache)) return undefined

  const missedTokens = Math.min(prev.promptTokens, promptTokens) - usage.cacheRead
  if (missedTokens <= NOISE_FLOOR_TOKENS) return undefined  // 1024 token 噪声地板

  // 额外成本 = 错过的 token 按实际支付价（非缓存价）计费
  const paidTokens = usage.input + usage.cacheWrite
  const paidPerToken = paidTokens > 0 ? (usage.cost.input + usage.cost.cacheWrite) / paidTokens : 0
  const readPerToken = usage.cacheRead > 0
    ? usage.cost.cacheRead / usage.cacheRead
    : (models.getModel(message.provider, message.model)?.cost.cacheRead ?? 0) / 1_000_000

  return {
    missedTokens,
    missedCost: missedTokens * Math.max(0, paidPerToken - readPerToken),
    idleMs: Math.max(0, message.timestamp - prev.timestamp),
    modelChanged: `${message.provider}/${message.model}` !== prev.modelKey,
  }
}
```

**诊断价值**：
- `missedCost`：本次 cache miss 多花了多少钱（支付价 vs 缓存价之差）
- `idleMs`：距上次请求的空闲时间（> 5 分钟 = TTL 过期 = miss 合理）
- `modelChanged`：模型切换导致的全量重计费（应计数）
- `NOISE_FLOOR_TOKENS = 1024`：过滤断点粒度噪声

**设置联动**：`settings-manager.ts:108` 的 `showCacheMissNotices?: boolean` 控制是否显示 cache miss + 压缩成本通知。

#### 1.3.4 上下文预算：压缩预留 token

`settings-manager.ts:15-20, 842-853`：

```typescript
reserveTokens?: number;    // default: 16384（为 prompt + LLM 响应预留）
keepRecentTokens?: number; // default: 20000（保留最近 token）
getCompactionReserveTokens(): number { return this.settings.compaction?.reserveTokens ?? 16384 }
```

#### 1.3.5 价格数据源：token-plan 套餐概念

`packages/coding-agent/src/core/model-resolver.ts:54-60` 出现 **token-plan** 概念：

```typescript
"qwen-token-plan": "qwen3.7-max",
"qwen-token-plan-cn": "qwen3.7-max",
"qwen-token-plan-individual": "qwen3.8-max",
"xiaomi-token-plan-cn": "mimo-v2.5-pro",
```

这表明 pi 把「付费套餐 → 可用模型」映射内置在模型解析层——不同套餐路由到不同模型。

---

### 1.4 openclaw（TypeScript）——分层定价 + cacheWrite1h 子集

openclaw 的 `usage-cost.ts` 是本次调研中**定价模型最灵活**的实现：支持按 prompt token 数分层定价。

#### 1.4.1 价格数据源：snapshot 式分层定价

`packages/llm-core/src/types.ts:311-323`：

```typescript
export type PricingTier = ModelCostRates & {
  range: [number, number]  // [start, end) token 范围
}
export type ModelCostConfig = ModelCostRates & { tieredPricing?: PricingTier[] }
export type RawModelCostConfig = ModelCostRates & { tieredPricing?: RawPricingTier[] }
```

**snapshot 语义**：`usage-cost.ts:10-12` 注释明确——

> Pricing is a model snapshot. Weak keys release the sorted schedule with its owner; a config/catalog reload supplies a new schedule rather than mutating active tiers.

#### 1.4.2 成本计算：分层选择 + cacheWrite1h 子集定价

`packages/llm-core/src/usage-cost.ts:93-114`：

```typescript
export function calculateUsageCost(
  usage: Partial<Pick<Usage, "input" | "output" | "cacheRead" | "cacheWrite" | "cacheWrite1h">>,
  pricing: RawModelCostConfig,
): Usage["cost"] {
  const rates = selectPricingRates(pricing, input + cacheRead + cacheWrite)
  const cacheWrite1h = Math.min(cacheWrite, Math.max(0, finiteOrZero(usage.cacheWrite1h)))
  const cacheWrite5m = cacheWrite - cacheWrite1h
  const cost = {
    input: (input * rates.input) / 1_000_000,
    output: (output * rates.output) / 1_000_000,
    cacheRead: (cacheRead * rates.cacheRead) / 1_000_000,
    // One-hour writes are a subset, priced at twice the selected input rate.
    cacheWrite: (cacheWrite5m * rates.cacheWrite + cacheWrite1h * rates.input * 2) / 1_000_000,
    total: 0,
  }
  cost.total = cost.input + cost.output + cost.cacheRead + cost.cacheWrite
  return cost
}
```

**分层选择**：`selectPricingRates` 按 `promptTokens` 匹配 `[start, end)` 区间，取最近较低 tier（不跨桶混合）。

**cacheWrite1h 子集**：1h 缓存是 5m 缓存的子集，价格 = 2 倍 input 率（比 cacheWrite 率更高）。

#### 1.4.3 Usage 完整结构

`packages/llm-core/src/types.ts:281-308`：

```typescript
export interface Usage {
  input: number; output: number; cacheRead: number; cacheWrite: number;
  cacheWrite1h?: number;  // cacheWrite 的子集（1 小时保留）
  contextUsage?: { input: number; output: number; cacheRead: number; cacheWrite: number }
  cost: { input: number; output: number; cacheRead: number; cacheWrite: number; total: number; provenance: ... }
}
```

---

### 1.5 deepseek-harness（TypeScript）——纯本地 token 估算 + 表面投影

deepseek 的 `token-meter` 包**不依赖 API 返回的 usage**，而是用固定密度启发式在本地估算 token——用于上下文预算预测。

#### 1.5.1 Token 估算：chars/4 + 结构开销

`packages/llm/token-meter/src/estimate.ts:13-20`：

```typescript
const CHARS_PER_TOKEN = 4          // 固定文本密度
const BLOCK_OVERHEAD = 4            // JSON 框架 + 类型标签
export const ROLE_OVERHEAD = 4      // 角色字段框架
```

**分类型估算**：`estimate.ts:37-61`：

```typescript
export function estimateContent(blocks: readonly ContentBlock[]): number {
  let tokens = 0
  for (const block of blocks) {
    switch (block.type) {
      case 'text': case 'reasoning':
        tokens += Math.ceil(block.text.length / CHARS_PER_TOKEN) + BLOCK_OVERHEAD; break
      case 'tool-call':
        tokens += Math.ceil(block.name.length / CHARS_PER_TOKEN)
          + Math.ceil(block.arguments.length / CHARS_PER_TOKEN) + BLOCK_OVERHEAD; break
      case 'tool-result':
        tokens += estimateContent(block.content) + BLOCK_OVERHEAD; break
      default:
        tokens += estimateStructuralBlock(block)  // 未知块保留结构 JSON 价
    }
  }
  return tokens
}
```

#### 1.5.2 上下文预算：O(1) 表面投影 + shadow-price 协议

`packages/llm/token-meter/src/surface-projection.ts` 维护一个**有界**的 running total：

```typescript
// 有界状态：只保留 running total + 最多一个 pending claim
export function foldSurfaceProjection(claim: ShadowPriceClaim | undefined, event: SessionEvent): SurfaceTokensFold {
  if (event.type === 'compaction/summary' || event.type === 'compaction/prune') {
    const { shadowedRange, shadowedTokenCount } = event.data
    return { deltaTokens: 0, claim: { start, end, tokens: shadowedTokenCount } }  // 武装 claim
  }
  if (!isSurfaceEvent(event)) return { deltaTokens: 0, claim: undefined }
  const tokens = estimateMessage(message)
  if (op === 'append') return { deltaTokens: tokens, claim: undefined }
  // replace 消费 claim，计算 delta = 新 - 旧
  return { deltaTokens: tokens - claim.tokens, claim: undefined }
}
```

**shadow-price 协议**：压缩事件前的 metering 事件声明被替换范围的 heuristic token 数，使 fold 无需保留每节点价格即可精确计算 delta。

#### 1.5.3 路由感知图像定价

`packages/llm/token-meter/src/route-pricing.ts:32-68` 把图像 token 定价交给路由层：

```typescript
export function priceSurface(nodes: readonly MeterSurfaceNode[], pricing: LlmImageRequestPricing | undefined): PricedSurface {
  if (pricing === undefined || images.length === 0) { /* 保留固定启发式 */ }
  const prices = pricing.priceImages(images)
  if (prices.length !== images.length) throw new Error(`misalignment would silently misprice nodes`)
  // 每个图像 = visualTokens + 其中文本的估算
  tokens += price.visualTokens + estimateContent([{ type: 'text', text: price.text }])
}
```

---

### 1.6 atomcode（Rust）——三元组 token 统计，零成本计算

atomcode **没有成本计算**，只有 token 统计——代表「CLI 工具不收费，仅透明展示」的极简路线。

#### 1.6.1 Token 计数：三元组结构

`crates/atomcode-kernel/src/stream.rs:6-10`：

```rust
pub struct TokenUsage {
    pub prompt: u32,      // 输入
    pub completion: u32,  // 输出
    pub cached: u32,      // 缓存命中
}
```

**Field-wise MAX 合并**：`stream.rs:13-21` 处理同一 round 多个 `StreamEvent::Usage`：

```rust
/// OpenAI-style: ONE cumulative Usage at the end → merging is a no-op
/// Anthropic-style: input tokens in message_start, running output in each message_delta
/// Field-wise MAX is correct for both
```

#### 1.6.2 成本呈现：trace summary 展示

`crates/atomcode-clix/src/main.rs:421-425`：

```rust
if let Some(u) = run.usage {
    eprintln!("— tokens — prompt {} / completion {} / cached {}", u.prompt, u.completion, u.cached)
}
```

**无价格表、无成本计算、无预算**——纯展示 token 用量。

#### 1.6.3 遥测：LlmChat 事件上报

`crates/atomcode-coding/src/telemetry.rs:612-644` 的 `Event::LlmChat` 携带 `input_tokens / output_tokens / cached_tokens / duration_ms / context_window`，但**不含 cost**。

---

### 1.7 Switchyard（Rust LLM 网关）——审查预算 = 成本控制

Switchyard 不直接计费，但通过**审查预算**间接控制成本：限制 LLM 评审次数 = 控制 API 调用开销。

#### 1.7.1 审查预算机制

`crates/libsy/src/algorithms/advisor_gate.rs:101-150`：

```rust
pub struct AdvisorGateConfig {
    pub max_reviews: u32,          // 每个预算 scope 允许的评审次数（默认 1）
    pub gate_stall_turns: u32,     // 停滞检测：N 轮无工具调用时触发评审
    pub gate_min_tool_results: u32,// 最少工具结果数才评审
    pub advisor_max_tokens: u64,   // 每次评审输出上限（默认 2048）
    pub transcript_max_chars: usize, // 交给 advisor 的转录上限（默认 200K）
    pub fail_open: bool,           // advisor 失败时降级为 APPROVE
}
```

**预算 scope**：`advisor_gate.rs:161-166` 按 `Instance → Client → Session` 三级优先级：

```rust
enum ScopeKey { Instance, Client(String), Session(String) }
struct ScopeState { reviews: u32; failed_consults: u32; exhaustion_logged: bool }
```

**预算耗尽行为**：`advisor_gate.rs:238-251`——`reviews >= max_reviews` 后纯 passthrough（不再调用 advisor），日志一次。

**设计洞察**：每次 advisor 调用都是一次完整 LLM 请求（带 200K 转录上下文），`max_reviews = 1` 意味着「每个 Session 最多花 1 次额外 API 调用做质检」——这是**以调用次数为粒度的成本控制**。

---

## 2. 横向对比总表

| 维度 | opencode | claudecode | pi | openclaw | deepseek-harness | atomcode | Switchyard |
|------|----------|------------|-----|----------|------------------|----------|------------|
| **Token 计数** | 6 类（input/output/reasoning/cacheRead/cacheWrite5m/cacheWrite1h） | 5 类（input/output/cacheRead/cacheCreation/webSearch） | 5 类（input/output/cacheRead/cacheWrite/cost） | 5 类（input/output/cacheRead/cacheWrite/cacheWrite1h） | 本地估算（chars/4 + 结构开销） | 3 类（prompt/completion/cached） | 无 |
| **价格数据源** | 远程 models.dev（5min TTL）+ 峰值定价 | 写死常量（6 tier） | 内置（token-plan 映射） | snapshot 分层定价（catalog 重载） | 无（仅估算） | 无 | 无 |
| **成本精度** | 微美分（1$=10^8） | 美元浮点 | 美元浮点 | 美元浮点 | 无 | 无 | 无 |
| **缓存折扣** | cacheRead=0.1x, cacheWrite5m=1.25x, cacheWrite1h=2x | cacheRead=0.1x, cacheWrite=1.25x | cacheRead 单独计价 | cacheRead=0.1x, cacheWrite1h=2x input | 无 | 无 | 无 |
| **分层定价** | cost200K 档位（>200K token 切换） | 按模型 tier | 无 | 按 promptTokens 范围分层 | 无 | 无 | 无 |
| **上下文预算** | 压缩配置（auto/prune/keep/buffer） | 200K 默认 + 百分比计算 | reserveTokens 16384 + keepRecent 20000 | 无显式 | O(1) 表面投影 + shadow-price | ctx_window 遥测 | 无 |
| **预算限额** | workspace 月限 + user 月限 + sub 周/滚动限 | 无 | 无 | 无 | 无 | 无 | max_reviews 调用次数限 |
| **成本呈现** | /stats 命令 + 按模型/工具聚合 + CSV 运维脚本 | 退出时 formatTotalCost + 状态栏 | cache miss 诊断通知 | 无显式 | 无 | trace summary token 展示 | 无 |
| **省钱机制** | 自动 cache 断点注入 + Redis 批量写入 + provider 优先级预算 | 无 | cache miss 浪费量化 + 5 分钟 TTL 诊断 | 无 | 无 | 无 | 审查预算（限调用次数） |
| **支付集成** | Stripe（充值/订阅/优惠券） | 无 | 无 | 无 | 无 | 无 | 无 |
| **聚合粒度** | 按 workspace/user/provider/model/week | 按模型短名 | 按 provider/model + Tools 桶 | 无 | 无 | 按 run | 按 Session/Client/Instance |

### 关键差异一览

- **最完整**：opencode（支付 + 计费 + 预算 + 统计全链路）
- **最诊断**：pi（cache miss 浪费量化，告诉你「多花了多少钱」）
- **最灵活**：openclaw（分层定价 + cacheWrite1h 子集）
- **最轻量**：atomcode（仅展示 token，零成本计算）
- **最独特**：Switchyard（以调用次数为粒度的审查预算）
- **最预测**：deepseek（纯本地估算，不依赖 API usage 字段）

---

## 3. 可复用设计模式（8 个）

### 模式 M1：四类 token 分桶计价

**描述**：将 token 拆为 `input / output / cacheRead / cacheWrite` 四个独立桶，分别适用不同单价。

**出处**：opencode（`handler.ts:1002-1041`）、claudecode（`modelCost.ts:131-142`）、openclaw（`usage-cost.ts:93-114`）。

**价格关系**（以 Sonnet 为例）：
- `input = $3/Mtok`
- `output = $15/Mtok`
- `cacheRead = $0.3/Mtok`（input 的 1/10）
- `cacheWrite = $3.75/Mtok`（input 的 1.25 倍）

**laew 适用性**：★★★★★（直接复用，Anthropic/OpenAPI 都已返回这 4 类数据）

### 模式 M2：微美分整数计价

**描述**：用整数微美分（1 美元 = 10^8 微美分）累加，避免浮点误差。

**出处**：opencode（`price.ts` 的 `centsToMicroCents`）。

**laew 适用性**：★★★★☆（Rust 整数运算天然适合，但需权衡复杂度）

### 模式 M3：cache miss 浪费量化

**描述**：对比相邻两轮的 prompt token 重叠度，量化「本应命中缓存但被全量重计费」的浪费金额。

**出处**：pi（`cache-stats.ts:56-90`）。

**关键公式**：`missedCost = missedTokens × (paidPerToken - readPerToken)`

**laew 适用性**：★★★★★（纯本地计算，无需价格表，仅需相邻两轮 usage）

### 模式 M4：自动 cache 断点注入

**描述**：在最后一个 tool 定义、最后一个 system part、最新 user message 三个断点自动注入 `cache_control: { type: "ephemeral" }`。

**出处**：opencode（`cache-policy.ts:18-42`）。

**经济依据**：`read = 0.1x, write = 1.25x` → 5 分钟内复用一次即回本。

**laew 适用性**：★★★★★（Anthropic 协议已支持，laew 当前未利用）

### 模式 M5：按模型分层的写死价格表

**描述**：以 `Record<ModelShortName, ModelCosts>` 写死价格，未知模型兜底到默认 tier。

**出处**：claudecode（`modelCost.ts:104-126`）。

**laew 适用性**：★★★★★（最简方案，SQLite providers 表旁加 pricing 表）

### 模式 M6：上下文百分比 + 压缩联动

**描述**：`(input + cacheRead + cacheCreation) / contextWindow × 100%`，超阈值触发自动压缩。

**出处**：claudecode（`context.ts:118-144`）。

**laew 适用性**：★★★★☆（laew 有压缩机制，但未与成本联动）

### 模式 M7：审查预算（调用次数限）

**描述**：以「每个 Session 最多 N 次额外 LLM 调用」为粒度控制成本。

**出处**：Switchyard（`advisor_gate.rs:101-150`）。

**laew 适用性**：★★★★☆（多 Agent 架构中，Quality-Check / Plan Agent 的调用次数直接影响成本）

### 模式 M8：退出时持久化 + 状态栏展示

**描述**：进程退出时打印 `Total cost: $X.XX` + `Usage by model`，并持久化到项目配置。

**出处**：claudecode（`costHook.ts:6-22`、`cost-tracker.ts:228-244`）。

**laew 适用性**：★★★★★（TUI 状态栏加一行成本显示，改动最小）

---

## 4. laew 借鉴路线

### 4.1 laew 现状盘点

| 能力 | 现状 | 证据 |
|------|------|------|
| Token 计数 | ✅ 已解析 4 类 token | `src/llm/sse.rs:209-254` ParseSink 收集 input/output/cacheRead/cacheCreation |
| 累计用量 | ✅ 单轮循环累计 | `src/agent/mod.rs:94-99` total_usage 累加 |
| 状态栏展示 | ✅ 显示原始 token | `src/tui/mod.rs:151-214` 显示 input/output/cache_read |
| 价格表 | ❌ 无 | — |
| 成本计算 | ❌ 无 | — |
| 预算/限额 | ❌ 无 | — |
| cache 注入 | ❌ 未利用 | 请求头已带 cache 能力但未注入 cache_control |
| 成本 UI | ❌ 无 /cost 命令 | — |

**结论**：laew 已具备 token 计数的**数据基础**（API usage 字段已解析），缺的是「价格表 → 成本计算 → 预算 → UI」这一层。

### 4.2 P0：最小可用成本统计（1-2 周）

#### P0-1：SQLite pricing 表 + 写死价格

在 `src/config/mod.rs` 的 `providers` 表旁新增 `pricing` 表：

```sql
CREATE TABLE pricing (
  provider_name TEXT NOT NULL,  -- 匹配 providers.provider_name
  model_name    TEXT NOT NULL,
  input_cost    INTEGER NOT NULL,  -- 每 token 价格（微美分）
  output_cost   INTEGER NOT NULL,
  cache_read_cost  INTEGER,
  cache_write_cost INTEGER,
  context_window   INTEGER,  -- 上下文窗口大小（用于百分比计算）
  PRIMARY KEY (provider_name, model_name)
);
```

**设计理由**：
- 与 `providers` 表解耦：同一模型可有多条 pricing 记录（不同时期价格不同，加 `effective_from`）
- 微美分整数：避免浮点误差（学 opencode）
- 启动时全量加载到内存 `HashMap<(provider, model), PriceConfig>`

#### P0-2：agent 循环内成本计算

在 `src/agent/mod.rs:94-99` 的 total_usage 累加后，增加成本计算：

```rust
// 现有
total_usage.input_tokens = total_usage.input_tokens.saturating_add(completion.usage.input_tokens);
// 新增
if let Some(pricing) = price_table.lookup(provider, model) {
    let cost = pricing.calculate(&completion.usage);
    total_cost.add(cost);  // 微美分累加
}
```

#### P0-3：TUI 状态栏 + /cost 命令

**状态栏**：在 `src/tui/mod.rs:151-214` 的 token 显示行增加成本：

```
Tokens: input=1234 output=567 cache_read=890 | Cost: $0.0234
```

**/cost 命令**：新增斜杠命令，输出：

```
┌─────────────────────────────────────────┐
│              COST SUMMARY               │
├─────────────────────────────────────────┤
│ Total Cost        $1.234                │
│ Input Tokens      123.4K                │
│ Output Tokens     45.6K                 │
│ Cache Read        78.9K                 │
│ Cache Hit Rate    85.2%                 │
└─────────────────────────────────────────┘
```

### 4.3 P1：省钱机制（2-4 周）

#### P1-1：自动 cache 断点注入（学 opencode）

在 `src/llm/anthropic.rs` 的 `convert_tools` / system / messages 阶段注入 cache_control：

```rust
// 最后一个 tool 定义
if i == tools.len() - 1 {
    tool_obj["cache_control"] = json!({ "type": "ephemeral" });
}
// 最后一个 system part / 最新 user message 同理
```

**预期收益**：多轮对话场景下 cache hit 率从 ~30% 提升到 ~85%，成本降低 50-80%。

#### P1-2：cache miss 浪费量化（学 pi）

在 `src/agent/mod.rs` 记录上一轮 prompt token 数，本轮对比计算 `missedTokens` 和 `missedCost`，在 TUI 显示：

```
⚡ Cache miss: 2.3K tokens re-billed (+$0.0023) — idle 6.2min (TTL expired)
```

#### P1-3：上下文百分比 + 压缩联动

在 `src/tui/mod.rs` 显示上下文使用率：

```
Context: 145.2K / 200K (72.6%)  ⚠️ >80% will auto-compact
```

### 4.4 P2：预算与限额（4-8 周）

#### P2-1：单次会话花费上限

在 pricing 表加 `max_session_cost` 字段，agent 循环检查：

```rust
if total_cost > pricing.max_session_cost {
    yield AgentEvent::BudgetExceeded { limit: pricing.max_session_cost, actual: total_cost };
    break;  // 或询问用户是否继续
}
```

#### P2-2：多 Agent 调用次数预算（学 Switchyard）

对 Plan Agent / Quality-Check Agent 的调用次数设限：

```rust
struct AgentCallBudget {
    max_reviews: u32,  // 默认 1
    reviews_used: u32,
}
if budget.reviews_used >= budget.max_reviews {
    // 跳过 QC，直接通过
}
```

#### P2-3：按模型聚合统计（学 opencode /stats）

新增 `/cost --models` 输出：

```
┌────────────────────────────────────────────────────┐
│                   MODEL USAGE                      │
├────────────────────────────────────────────────────┤
│ claude-sonnet-4-5                                    │
│   Messages 12  Input 89.3K  Output 12.4K  Cost $0.45│
│ claude-haiku-4-5                                     │
│   Messages 3   Input 5.2K    Output 1.1K   Cost $0.02│
└────────────────────────────────────────────────────┘
```

### 4.5 实施优先级矩阵

| 优先级 | 改动 | 依赖 | 预期收益 |
|--------|------|------|----------|
| P0-1 | pricing 表 | 无 | 数据基础 |
| P0-2 | 成本计算 | P0-1 | 知道花了多少钱 |
| P0-3 | 状态栏 + /cost | P0-2 | 用户可见 |
| P1-1 | cache 注入 | 无 | 成本降低 50-80% |
| P1-2 | cache miss 量化 | P0-2 | 诊断省钱 |
| P1-3 | 上下文百分比 | P0-1 | 预防超限 |
| P2-1 | 会话花费上限 | P0-2 | 防止破产 |
| P2-2 | Agent 调用预算 | 无 | 多 Agent 成本控制 |
| P2-3 | 按模型聚合 | P0-2 | 优化模型选择 |

---

## 5. 总结与反模式警示

### 5.1 核心发现

1. **opencode 的微美分 + Redis 批量 + provider 预算**是生产级计费标杆，但其复杂度（Stripe 集成、多 workspace 隔离）远超 CLI 工具需求——laew 应学其「精度 + 批量」而非「支付」。

2. **claudecode 的写死价格表 + 未知模型兜底**是最适合 laew 的起步方案：6 个 tier 覆盖全模型，`hasUnknownModelCost` 标志保证透明。

3. **pi 的 cache miss 浪费量化**是独特诊断视角——告诉你「为什么多花了钱」而不只是「花了多少钱」，对优化 cache 策略极具价值。

4. **openclaw 的分层定价 + cacheWrite1h 子集**是最灵活的定价模型，但 laew 起步阶段无需此复杂度。

5. **Switchyard 的审查预算**揭示「成本控制 = 调用次数控制」——多 Agent 架构中，限制 Plan/QC Agent 调用次数是最直接的省钱手段。

### 5.2 反模式警示

- **反模式 R1：浮点累加成本**——直接用 `f64` 累加美元，长期运行产生误差。应学 opencode 用整数微美分。
- **反模式 R2：忽略 cache_creation/input 区分**——把 cacheWrite 当 input 计价会高估成本 25%。
- **反模式 R3：价格表与 provider 耦合**——价格应按 `(provider, model)` 独立管理，而非嵌入 provider 配置。
- **反模式 R4：无兜底策略**——遇到未知模型时静默忽略成本（学 claudecode 设 `hasUnknownModelCost` 标志）。
- **反模式 R5：成本展示与 token 展示分离**——应在同一区域展示（学 claudecode 的 `formatTotalCost` 一体化输出）。

### 5.3 一句话总结

> laew 已具备 token 计数的完整数据基础（API usage 字段已解析到 ParseSink），**缺的是「价格表 → 成本计算 → 预算 → UI」一层**。最小改动 = SQLite 加 pricing 表 + agent 循环加成本累加 + TUI 状态栏加一行成本显示；最大收益 = 自动 cache 注入（降本 50-80%）+ 多 Agent 调用预算（限次数）。

---

## 附录 A：关键文件索引

| 仓库 | 文件 | 核心内容 |
|------|------|----------|
| opencode | `packages/console/core/src/schema/billing.sql.ts:114-136` | UsageTable 六类 token 定义 |
| opencode | `packages/console/core/src/model.ts:14-50` | ModelCostSchema 价格结构 |
| opencode | `packages/console/app/src/routes/zen/util/handler.ts:1002-1041` | calculateCost 核心计价 |
| opencode | `packages/console/app/src/routes/zen/util/providerBudgetTracker.ts:14-150` | 多 provider 预算追踪 |
| opencode | `packages/console/app/src/routes/zen/util/usageBatcher.ts:12-31` | Redis 批量写入 |
| opencode | `packages/llm/src/cache-policy.ts:18-42` | 自动 cache 断点注入 |
| opencode | `packages/console/core/src/util/price.ts` | 微美分转换 |
| opencode | `packages/opencode/src/cli/cmd/stats.ts:292-384` | /stats 成本展示 |
| claudecode | `src/utils/modelCost.ts:36-126` | 6 个价格 tier + MODEL_COSTS |
| claudecode | `src/utils/modelCost.ts:131-180` | tokensToUSDCost + calculateUSDCost |
| claudecode | `src/cost-tracker.ts:250-323` | addToTotalModelUsage + Advisor 递归 |
| claudecode | `src/utils/context.ts:118-144` | calculateContextPercentages |
| claudecode | `src/costHook.ts:6-22` | 退出时持久化 + 展示 |
| pi | `packages/coding-agent/src/core/usage-totals.ts:37-70` | 按模型分组成本 |
| pi | `packages/coding-agent/src/core/cache-stats.ts:56-90` | cache miss 浪费量化 |
| pi | `packages/coding-agent/src/core/settings-manager.ts:842-853` | 压缩预留 token |
| openclaw | `packages/llm-core/src/usage-cost.ts:93-114` | 分层定价 + cacheWrite1h 子集 |
| openclaw | `packages/llm-core/src/types.ts:281-308` | Usage 完整结构 |
| deepseek | `packages/llm/token-meter/src/estimate.ts:13-20` | chars/4 + 结构开销 |
| deepseek | `packages/llm/token-meter/src/surface-projection.ts:66-94` | O(1) 表面投影 |
| atomcode | `crates/atomcode-kernel/src/stream.rs:6-10` | TokenUsage 三元组 |
| Switchyard | `crates/libsy/src/algorithms/advisor_gate.rs:101-150` | 审查预算配置 |
| laew | `src/llm/sse.rs:209-254` | ParseSink usage 收集 |
| laew | `src/agent/mod.rs:94-99` | total_usage 累加 |
| laew | `src/tui/mod.rs:151-214` | TUI token 展示 |

## 附录 B：价格速查表（2026-09 参考）

| 模型 | input ($/Mtok) | output ($/Mtok) | cacheRead | cacheWrite | 来源 |
|------|---------------|-----------------|-----------|------------|------|
| Claude Sonnet 4.5 | 3 | 15 | 0.3 | 3.75 | claudecode COST_TIER_3_15 |
| Claude Opus 4 | 15 | 75 | 1.5 | 18.75 | claudecode COST_TIER_15_75 |
| Claude Opus 4.5 | 5 | 25 | 0.5 | 6.25 | claudecode COST_TIER_5_25 |
| Claude Haiku 4.5 | 1 | 5 | 0.1 | 1.25 | claudecode COST_HAIKU_45 |
| Claude Haiku 3.5 | 0.80 | 4 | 0.08 | 1 | claudecode COST_HAIKU_35 |
| Web Search | — | — | — | — | $0.01/request |

> 注：opencode 的价格从远程 models.dev 动态获取，上表仅作参考。laew 实施 P0 时应以 Anthropic/OpenAI 官方价格表为准，并预留更新机制。
