# 专题：Context 上下文管理深度分析

> **日期**: 2026-09-04
> **输入文件数**: 6（各项目的「核心机制深度分析」文档）
> **分析范围**: Context 上下文压缩 / 触发条件 / token 计数 / prompt cache / 工具结果处理 / 尾部保留 / 摘要生成
> **项目清单**: Claude Code / AtomCode (Rust) / OpenClaw / OpenCode / DeepSeek Harness / Pi

---

## 目录

- [第 1 章 六项目横向对比总览](#第-1-章-六项目横向对比总览)
- [第 2 章 Claude Code — 四级压缩管线](#第-2-章-claude-code--四级压缩管线)
- [第 3 章 AtomCode — 三级 Overflow Ladder（Rust）](#第-3-章-atomcode--三级-overflow-ladderrust)
- [第 4 章 OpenClaw — ContextEngine 抽象 + BasicCompactionEngine](#第-4-章-openclaw--contextengine-抽象--basiccompactionengine)
- [第 5 章 OpenCode — Prune + Compaction 双阶段](#第-5-章-opencode--prune--compaction-双阶段)
- [第 6 章 DeepSeek Harness — 事件源投影 + CompactionEngine](#第-6-章-deepseek-harness--事件源投影--compactionengine)
- [第 7 章 Pi — 压缩 + 分支摘要](#第-7-章-pi--压缩--分支摘要)
- [第 8 章 设计模式提炼](#第-8-章-设计模式提炼)
- [第 9 章 对 laew 的综合建议](#第-9-章-对-laew-的综合建议)

---

## 第 1 章 六项目横向对比总览

### 1.1 Context 管理方案对比表

| 维度 | Claude Code | AtomCode (Rust) | OpenClaw | OpenCode | DeepSeek Harness | Pi |
|------|-------------|-----------------|----------|----------|-----------------|-----|
| **压缩层级** | 4 级管线 | 3 级 ladder | 2 层（prune + compact） | 2 阶段（prune + compact） | 2 层（prune + compact） | 2 层（compact + branch） |
| **最轻量层** | Time-Based MC | StubCompaction（>500B → 摘要） | toolResultPruner | PRUNE（标记过时） | toolResultPruner | - |
| **中间层** | Cached MC（API cache_edits） | truncate_rewrites（硬截断） | BasicCompactionEngine | Compaction Agent 摘要 | BasicCompactionEngine | LLM 摘要 |
| **最重量层** | Auto-Compact（LLM 全文摘要） | drain + summarize（LLM） | - | - | - | 分支摘要 |
| **手动触发** | Partial Compact | /compact 命令 | compactNow() | - | compactNow() | - |
| **触发条件** | token ≥ 窗口 - 13K | utilization ≥ 0.7（Tier 0）/ 0.78（Tier 2） | thresholdRatio × 窗口 | 可用窗口 ≤ 实际 token | thresholdRatio × 窗口 | token > 窗口 - 16K |
| **Token 估算** | roughTokenCountEstimation（~4 chars/token） | provider 返回 usage | tokenMeter（精确） | 字符数/4 | tokenMeter（精确） | 字符数/4 |
| **Prompt Cache** | runForkedAgent 共享 cache-safe 参数 | cache_epoch + committed compaction 才 bump | ContextEnginePromptCacheInfo | - | - | cache_control: ephemeral |
| **工具结果处理** | content-clear / cache_edits 远程删除 | stub 摘要（>500B） | toolResultPruner 修剪 | 标记 compacted 时间戳 → "[Old tool result content cleared]" | toolResultPruner 修剪 | 截断到 2000 字符 |
| **尾部保留** | 最近 5 个文件 + plan/skill 恢复 | keep_recent_turns=1 + recent_keep_budget（25%窗口） | retainTokens / retainRatio | 保护最近 2 轮 + PRUNE_PROTECT=40K | retainTokens | retainedTail + keepRecentTokens=20K |
| **前缀保护** | system prompt + 工具定义不变 | sacred_floor（System + 首个真实 User） | - | 到达 compaction 边界即停止 | - | - |
| **摘要格式** | 9 段式提示词 | anchored context summarization | LLM 自由格式 | 专用 compaction Agent | LLM 摘要 | 结构化 Goal/Progress/Next Steps |
| **断路器** | 连续失败 ≥ 3 停止 | 超时 180s 降级为 stub | quarantine 健康隔离 | - | compactionRetries 可配 | - |
| **溢出恢复** | MAX_PTL_RETRIES=3 | Overflow attempt 递增 0→1→2 | context-overflow 事件触发 | replay 机制 | context-overflow 事件触发重试 | overflowRecoveryUsed 标记 |
| **并发保护** | - | - | compaction/start 锁定事件 | - | markRuntimeCompactionDelegate | - |
| **语言** | TypeScript | Rust | TypeScript | TypeScript (Effect) | TypeScript | TypeScript |
| **核心代码量** | ~3000 行（6 文件） | ~2345 行（1 文件）+ 核心模块 | ~500 行（4 文件）+ engine 接口 | ~600 行（4 文件） | ~500 行（4 文件）+ projection 框架 | ~400 行（3 文件） |

### 1.2 压缩策略复杂度排名

```
Claude Code (4 级) > AtomCode (3 级) > OpenClaw ≈ DeepSeek Harness (2 层 + 抽象) > OpenCode ≈ Pi (2 层)
```

### 1.3 对 laew 的适用度排名

```
AtomCode (Rust 同源) > Claude Code (最完整) > OpenCode (最实用) > Pi (简洁) > DeepSeek Harness (偏框架) > OpenClaw (偏插件)
```

---

## 第 2 章 Claude Code — 四级压缩管线

> **来源文件**: `claudecode-核心机制深度分析.md`
> **核心文件**: `src/services/compact/compact.ts` (1706 行) / `microCompact.ts` (531 行) / `autoCompact.ts` (352 行) / `prompt.ts` (375 行)

### 2.1 架构总览

Claude Code 采用**四级递进**压缩管线，从最轻量的工具结果清理到全文 LLM 摘要：

```
Level 1: Time-Based Microcompact  ─→  间隔超时 → content-clear 旧工具结果
Level 2: Cached Microcompact      ─→  工具数超阈值 → API cache_edits 远程删除
Level 3: Auto-Compact             ─→  token 超阈值 → LLM 全文摘要
Level 4: Partial Compact          ─→  用户选择 → 按消息范围 LLM 摘要
```

这是所有 6 个项目中**层级最丰富、设计最精细**的方案。

### 2.2 第一级：Time-Based Microcompact

**触发条件**: 两次对话间隔超过阈值（服务端 prompt cache 已失效），直接 content-clear 旧的工具结果。

```typescript
// microCompact.ts:422
export function evaluateTimeBasedTrigger(messages, querySource) {
  const config = getTimeBasedMCConfig();  // GrowthBook 配置
  if (!config.enabled || !querySource || !isMainThreadSource(querySource)) return null;
  const lastAssistant = messages.findLast(m => m.type === 'assistant');
  const gapMinutes = (Date.now() - new Date(lastAssistant.timestamp).getTime()) / 60_000;
  if (!Number.isFinite(gapMinutes) || gapMinutes < config.gapThresholdMinutes) return null;
  return { gapMinutes, config };
}
```

**压缩算法**: 保留最近 N 个工具结果，其余替换为常量 `'[Old tool result content cleared]'`。

- **可压缩工具**（`COMPACTABLE_TOOLS`）: Read、Bash/PowerShell、Grep、Glob、WebSearch、WebFetch、Edit、Write
- **保留策略**: `config.keepRecent`（至少 1 个）
- **特点**: 纯本地操作，无 LLM 调用，代价极低

### 2.3 第二级：Cached Microcompact

利用 Anthropic API 的 `cache_edits` 特性，**远程删除**工具结果而不破坏 prompt cache 前缀：

```typescript
// microCompact.ts:305
async function cachedMicrocompactPath(messages, querySource) {
  const state = ensureCachedMCState();
  const config = getCachedMCConfig();
  // 注册新工具结果 → 基于触发/保留阈值决定删除 → 创建 cache_edits 块
  if (toolsToDelete.length > 0) {
    const cacheEdits = createCacheEditsBlock(state, toolsToDelete);
    pendingCacheEdits = cacheEdits;  // 推送到 API 层
  }
  return { messages };  // 消息不变，cache_edits 在 API 层生效
}
```

- **关键创新**: 消息体不变（本地不删除），通过 API 层的 `cache_edits` 远程清除
- **前提**: 需要 Anthropic API 支持此特性
- **特点**: 不破坏前缀缓存命中率

### 2.4 第三级：Auto-Compact

**触发判断**:

```typescript
// autoCompact.ts:160
export async function shouldAutoCompact(messages, model, querySource, snipTokensFreed) {
  // 排除: session_memory / compact / marble_origami 来源
  // 排除: 禁用 auto-compact / reactive-only 模式 / context-collapse 模式
  const tokenCount = tokenCountWithEstimation(messages) - snipTokensFreed;
  const threshold = getAutoCompactThreshold(model);
  return tokenCount >= threshold;
}
```

**阈值计算**:

```typescript
// autoCompact.ts:72
export function getAutoCompactThreshold(model) {
  const effectiveContextWindow = getEffectiveContextWindowSize(model);
  // effectiveContextWindow = contextWindow - min(maxOutputTokens, 20000)
  return effectiveContextWindow - AUTOCOMPACT_BUFFER_TOKENS;  // 13,000 token 缓冲
}
```

**执行流程**（autoCompactIfNeeded，line 241）:

1. **断路器检查**: 连续失败 >= 3 次则停止重试
2. **先尝试 Session Memory 压缩**（更轻量）: `trySessionMemoryCompaction()`
3. **回退到全文压缩**: `compactConversation()`，使用 `runForkedAgent` 复用 prompt cache

### 2.5 第四级：Partial Compact

支持两个方向的精确压缩:

- `direction='up_to'`: 压缩 pivotIndex 之前，保留之后的
- `direction='from'`: 压缩 pivotIndex 之后，保留之前的

用户可选择消息范围进行定向压缩。

### 2.6 Prompt Cache 共享机制

压缩调用通过 `runForkedAgent` 复用主对话的 prompt cache:

```typescript
// compact.ts:1188
const result = await runForkedAgent({
  promptMessages: [summaryRequest],
  cacheSafeParams,            // 包含系统提示、工具定义等 cache 关键参数
  canUseTool: createCompactCanUseTool(),  // 禁止所有工具调用
  querySource: 'compact',
  maxTurns: 1,
  skipCacheWrite: true,       // 只读缓存，不写新缓存
  overrides: { abortController: context.abortController },
});
```

Fork 子 Agent 的模式: 隔离消息上下文，但**共享 cache-safe 参数**（系统提示、工具定义），实现 prompt cache 复用。

### 2.7 压缩后恢复

压缩后重新注入关键附件:

```typescript
// compact.ts:532
const [fileAttachments, asyncAgentAttachments] = await Promise.all([
  createPostCompactFileAttachments(preCompactReadFileState, context, 5),  // 最近 5 个文件
  createAsyncAgentAttachmentsIfNeeded(context),  // 后台 Agent 状态
]);
// 还单独注入: plan、plan_mode、skill、deferred tools delta、agent listing delta、MCP instructions delta
```

### 2.8 Token 预算汇总

| 常量 | 值 | 用途 |
|------|-----|------|
| `AUTOCOMPACT_BUFFER_TOKENS` | 13,000 | auto-compact 触发缓冲 |
| `POST_COMPACT_TOKEN_BUDGET` | 50,000 | 压缩后文件恢复总预算 |
| `POST_COMPACT_MAX_TOKENS_PER_FILE` | 5,000 | 单文件恢复上限 |
| `POST_COMPACT_SKILLS_TOKEN_BUDGET` | 25,000 | 压缩后 skill 恢复总预算 |
| `POST_COMPACT_MAX_TOKENS_PER_SKILL` | 5,000 | 单 skill 恢复上限 |
| `POST_COMPACT_MAX_FILES_TO_RESTORE` | 5 | 恢复文件数上限 |
| `MAX_OUTPUT_TOKENS_FOR_SUMMARY` | 20,000 | 压缩摘要输出 token 上限 |
| `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES` | 3 | 断路器阈值 |
| `MAX_PTL_RETRIES` | 3 | prompt-too-long 重试上限 |

### 2.9 Token 估算方法

使用 `roughTokenCountEstimation`，基于 ~4 chars/token 的粗略估算。不依赖精确 tokenizer。

### 2.10 关键设计亮点

1. **四级递进**: 从零代价的工具结果清理到全量 LLM 摘要，逐级升级
2. **缓存编辑型微压缩**: 利用 API 特性远程删除缓存条目而不破坏前缀（Claude Code 独有）
3. **时间触发型清理**: 用户长时间不操作时主动清理旧结果
4. **压缩后恢复**: 文件、plan、skill 等关键状态在压缩后重新注入
5. **断路器**: 连续失败 >= 3 次停止，防止不可恢复的上下文反复触发压缩
6. **9 段式压缩提示词**: 确保摘要质量

---

## 第 3 章 AtomCode — 三级 Overflow Ladder（Rust）

> **来源文件**: `atomcode-核心机制深度分析.md`
> **核心文件**: `crates/atomcode-capabilities/src/compaction.rs` (2345 行) / `crates/atomcode-kernel/src/message.rs` / `agent.rs` / `checkpoint.rs`

### 3.1 架构总览

AtomCode 是唯一的**纯 Rust** 实现，采用**三级 Overflow Ladder** 架构:

```rust
// crates/atomcode-capabilities/src/compaction.rs
pub struct OverflowCompaction {
    inner: StubCompaction,
    summary_provider: Option<Arc<dyn LlmProvider>>,
}

async fn plan(&self, view: &CompactionView<'_>) -> CompactionPlan {
    match view.trigger {
        CompactTrigger::Auto { .. } => self.inner.plan(view).await,          // Tier 0
        CompactTrigger::Overflow { attempt } => self.overflow_plan(view, *attempt).await,  // Tier 0-2
        CompactTrigger::Manual { focus } => self.manual_plan(view, focus.as_deref()).await,
    }
}
```

| Tier | 触发条件 | 策略 | 特点 |
|------|---------|------|------|
| **Tier 0** | Auto（utilization >= 0.7）或 Overflow{attempt=0} | `StubCompaction`: 旧工具结果 stub（>500B → 一行摘要） | cache-friendly，单调（stub 不可逆） |
| **Tier 1** | Overflow{attempt=1} | `truncate_rewrites`: 硬截断超长消息（budget = ctx_window × 2 chars） | 更激进，幂等（TRUNCATE_MARKER） |
| **Tier 2** | Overflow{attempt=2} | drain + summarize: LLM 生成摘要替换旧历史 | 最激进，有超时保护（180s） |

### 3.2 Tier 0: StubCompaction（正常路径）

```rust
pub const MIN_COLLAPSE_SIZE: usize = 500;  // 低于此大小的不 stub

pub struct StubCompaction {
    keep_recent_turns: usize,   // 默认 1（只保留活跃轮次完整）
    exempt_read_file: bool,     // 默认 true（read_file 结果不 stub）
}
```

**核心逻辑**:

1. 找到活跃轮次的起始位置（最近 N 个非合成 User 消息）
2. 遍历 `sacred_floor..boundary` 范围的消息
3. 跳过: 非 Tool 角色 / 已小消息（<=500B）/ read_file 结果（如果 exempt）
4. 生成 stub: `"[tool: bash] Command output (847 chars, success)"` 格式

**为什么 cache-friendly**: stub 是 COMMITTED 到历史的（不可逆），所以重跑不会改变历史字节，prefix cache 只在 stub 那一轮 break 一次。

### 3.3 Auto 触发机制

```rust
fn auto_compact_trigger(used_tokens: u32, ctx_window: u32, threshold: f32) -> Option<CompactTrigger> {
    if ctx_window == 0 { return None; }
    let utilization = used_tokens as f32 / ctx_window as f32;
    (utilization >= threshold).then_some(CompactTrigger::Auto { utilization })
}
```

**两级自动触发**:

- `compact_threshold`（默认 0.7）: 触发 Tier 0 stub compaction
- `AUTO_DRAIN_UTILIZATION`（0.78）: 触发 drain + summarize（在 Auto 路径内升级）

### 3.4 Manual /compact 命令

手动压缩按 `recent_keep_budget` 保留近期轮次，drain 老历史到 LLM 摘要。超时（180s）降级为 gentle stub。

`recent_keep_budget` 计算:

```rust
const RECENT_KEEP_FRACTION: f32 = 0.25;  // 保留窗口 25%
const MIN_RECENT_KEEP_TOKENS: usize = 8_000;
const MAX_RECENT_KEEP_TOKENS: usize = 256_000;

fn recent_keep_budget(ctx_window: u32) -> usize {
    ((window as f32 * RECENT_KEEP_FRACTION) as usize)
        .clamp(MIN_RECENT_KEEP_TOKENS, MAX_RECENT_KEEP_TOKENS)
        .min(window / 2)
}
```

### 3.5 Sacred Floor（神圣前缀保护）

```rust
pub fn sacred_floor(&self) -> usize {
    let lead_system = usize::from(matches!(self.messages.first().map(|m| &m.role), Some(Role::System)));
    match self.messages.iter().position(|m| m.role == Role::User && !m.synthetic) {
        Some(idx) => idx + 1,  // System + 第一个真实 User 消息
        None => lead_system,
    }
}
```

**永不删除**: System 消息 + 第一个非合成 User 消息（任务提示词）永远不被 compaction 删除。这是**所有项目中对前缀保护最严格的设计**。

### 3.6 摘要 LLM 调用

```rust
const SUMMARY_SYSTEM_PROMPT: &str = "You are an anchored context summarization assistant...";
const MAX_SUMMARY_BYTES: usize = 64 * 1024;   // 64KB 硬上限
const MAX_SUMMARY_TOKENS: u32 = 16_000;       // 发给 LLM 的 max_tokens
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(180);
```

摘要有 `<previous-summary>` 增量更新模式: 如果有已有摘要，LLM 更新而非重写。

### 3.7 Prompt Cache 集成

`cache_epoch` 只在 committed compaction 时 bump。`pre_request` 投影是 ephemeral（克隆上操作），不污染存储历史 → prefix cache 稳定。每轮只在 tail break 一次 cache（stub 那轮），之后冻结。

### 3.8 溢出恢复循环

`ProviderError::is_context_overflow()` 检测 9 种签名 → 递增 attempt 调用 `CompactTrigger::Overflow`。attempt 从 0 递增到 2，每升一级使用更激进的压缩策略。

截断续接: `finish_reason=length` 时注入 `TRUNCATION_RESUME_NUDGE`，最多 4 次（`MAX_TRUNCATION_CONTINUATIONS`）。

### 3.9 持久化快照

`CompactionCheckpoint` 支持压缩状态的持久化快照，跨 session 恢复。

### 3.10 关键设计亮点

1. **三级 ladder 逐级升级**: stub → truncate → summarize，按压力递增
2. **cache-friendly stub**: stub 是 committed 单调的，不反复改变历史字节
3. **sacred_floor**: 永远保护 System + 第一个真实 User 消息
4. **recent_keep_budget**: drain 时保留 25% 近期轮次（8K-256K tokens）
5. **摘要超时降级**: 180s 超时后降级为 gentle stub
6. **read_file exemption**: read_file 结果不 stub（保留行号上下文）
7. **溢出恢复 9 种签名检测**: 覆盖主流 LLM 提供商的上下文溢出错误格式
8. **增量摘要**: `<previous-summary>` 模式避免重新生成全部摘要
9. **Rust 实现**: 对 laew 最具参考价值

---

## 第 4 章 OpenClaw — ContextEngine 抽象 + BasicCompactionEngine

> **来源文件**: `openclaw-核心机制深度分析.md`
> **核心文件**: `src/context-engine/types.ts` / `compaction-basic/src/index.ts` / `compaction/src/tool-pairing.ts`

### 4.1 架构总览

OpenClaw 的 Context 管理基于 **Cordis 插件框架**，采用**抽象接口 + 可插拔实现**的设计:

```
ContextEngine（抽象接口，15 个生命周期方法）
    └── BasicCompactionEngine（基于 token meter 的压缩实现）
        └── toolResultPruner（可选工具结果修剪器）
```

### 4.2 ContextEngine 核心接口

```typescript
export interface ContextEngine {
  readonly info: ContextEngineInfo;
  // 初始化
  bootstrap?(params): Promise<BootstrapResult>;
  // 维护
  maintain?(params): Promise<ContextEngineMaintenanceResult>;
  // 消息摄入
  ingest(params): Promise<IngestResult>;
  ingestBatch?(params): Promise<IngestBatchResult>;
  // Turn 生命周期
  afterTurn?(params): Promise<void>;
  commitTurn?(params): Promise<{ status: "committed" | "duplicate" }>;
  // 核心: 组装与压缩
  assemble(params): Promise<AssembleResult>;
  compact(params): Promise<CompactResult>;
  // 子 Agent 生命周期
  prepareSubagentSpawn?(params): Promise<SubagentSpawnPreparation | undefined>;
  onSubagentEnded?(params): Promise<void>;
  dispose?(): Promise<void>;
}
```

`ContextEngineInfo` 声明引擎能力:

```typescript
export type ContextEngineInfo = {
  id: string;
  name: string;
  ownsCompaction?: boolean;  // 引擎自行管理压缩（而非 runtime 管理）
  turnMaintenanceMode?: "foreground" | "background";
  acceptedHostParams?: string[];
  hostRequirements?: Partial<Record<ContextEngineOperation, ContextEngineHostRequirements>>;
};
```

### 4.3 上下文组装（assemble）

`assemble()` 是每轮调用模型前的核心方法:

```typescript
export type AssembleResult = {
  messages: AgentMessage[];
  estimatedTokens: number;
  promptAuthority?: "assembled" | "preassembly_may_overflow";
  // "assembled": 使用组装后的估算（默认）
  // "preassembly_may_overflow": 使用组装前的原始估算（防止组装后隐藏溢出）
  systemPromptAddition?: string;
  contextProjection?: ContextEngineProjection;
};
```

`promptAuthority` 字段的设计很精妙: 当引擎做了激进裁剪可能掩盖溢出风险时，返回 `preassembly_may_overflow` 让 runtime 使用原始估算。

### 4.4 压缩（compact）

```typescript
compact(params: {
  sessionId: string;
  sessionKey: string;
  agentId?: string;
  sessionTarget?: ContextEngineSessionTarget;
  tokenBudget?: number;
  force?: boolean;
  currentTokenCount?: number;
  compactionTarget?: "budget" | "threshold";
  customInstructions?: string;
  abortSignal?: AbortSignal;
}): Promise<CompactResult>;
```

`CompactResult`:

```typescript
export type CompactResult = {
  ok: boolean;
  compacted: boolean;
  reason?: string;
  result?: {
    summary?: string;
    firstKeptEntryId?: string;  // 保留的最早条目 ID
    tokensBefore: number;
    tokensAfter?: number;
    sessionId?: string;
    sessionTarget?: ContextEngineSessionTarget;
  };
};
```

### 4.5 BasicCompactionEngine 的压缩流程

```typescript
// compaction-basic/src/index.ts:258-332
override async compactIfNeeded(agent, trigger, signal) {
  const meter = this.ctx.tokenMeter;
  let measurement = meter.measure(agent.session);

  // 1. 修剪工具结果（可选）
  const prune = this.ctx.get('toolResultPruner');
  if (prune !== undefined) {
    prune.pruneSession(agent.session);
    measurement = meter.measure(agent.session);
  }

  // 2. 检查是否超过阈值
  if (measurement.totalTokens < spec.thresholdTokens) return null;

  // 3. 选择可压缩范围
  const range = selectCompactableRange(agent.session, measurement, spec.retainTokens);

  // 4. 用 LLM 摘要 + 替换表面区域
  return this.compactRegion(range.start, range.end, agent, signal);
}
```

**token 预算管理**: 通过 `thresholdRatio`（如 0.8）和 `retainRatio` / `retainTokens` 控制:

- `thresholdTokens = contextWindow * thresholdRatio` -- 超过此值触发压缩
- `retainTokens` -- 保留最近的 N 个 token 不压缩
- `selectCompactableRange` -- 在保留尾部之外选择平衡的（tool call/result 成对）范围

### 4.6 工具配对保护

压缩的一个关键约束是 **tool call/result 必须成对**。`selectCompactableRange` 通过 `toolPairingBalancedBefore` / `toolPairingBalancedAfter` 检查边界。如果范围边界落在一个 tool call 和它的 result 之间，压缩会破坏配对，这是不允许的。

### 4.7 双触发机制

```typescript
// compaction-basic/src/index.ts:137-165
private _registerAutomaticCompaction(): void {
  // 步骤压力: 每步开始前检查
  ctx.on('agent/pre-step', async ({ agent, signal }, next) => {
    const result = await this.compactIfNeeded(agent, 'pressure', signal);
    return next();
  });

  // 上下文溢出: LLM 返回 context_window_exceeded 时触发
  ctx.on('agent/request-error', async ({ agent, failure, signal }, next) => {
    if (failure.code !== CONTEXT_WINDOW_EXCEEDED_CODE) return next();
    const result = await this.compactIfNeeded(agent, 'context-overflow', signal);
    if (result !== null) return { kind: 'retry' };  // 重试请求
    return next();
  });
}
```

### 4.8 压缩的 Session 事件

压缩产生三个 session 事件:

```typescript
'compaction/start'   // 锁定标记，防止并发压缩
'compaction/summary' // 摘要内容 + 影子范围 + token 统计 + LLM 调用元数据
'compaction/prune'   // 模型无关的修剪（无摘要，直接删除）
```

`compaction/summary` 事件携带完整的 LLM 调用元数据（provider、model、maxTokens、usage），支持从日志重建摘要。

### 4.9 Engine 注册与 Quarantine 健康隔离

引擎的所有方法被 `wrapResolvedContextEngine()` 包装为带异常捕获的版本。当引擎抛异常时，自动进入 quarantine 状态，下次解析 fallback 到默认引擎。

### 4.10 压缩安全超时与诊断

```typescript
const EMBEDDED_COMPACTION_TIMEOUT_MS = 180_000; // 180 秒

async function raceCompactionWithAbortSignal<T>(
  compact: () => Promise<T>, abortSignal?: AbortSignal, onAbort?: () => void
): Promise<T> {
  // AbortSignal 先于压缩完成 → 抛出 AbortError
  // 压缩先于 AbortSignal → 正常返回
}
```

### 4.11 RuntimeContext 提供给引擎的运行时能力

```typescript
export type ContextEngineRuntimeContext = Record<string, unknown> & {
  cwd?: string;
  allowDeferredCompactionExecution?: boolean;
  tokenBudget?: number;
  currentTokenCount?: number;
  promptCache?: ContextEnginePromptCacheInfo;
  sessionTarget?: ContextEngineSessionTarget;
  rewriteTranscriptEntries?: (request) => Promise<TranscriptRewriteResult>;
  llm?: {
    complete: (params: LlmCompleteParams) => Promise<LlmCompleteResult>;
  };
};
```

### 4.12 多模型策略

每个 provider/model 组合可以有不同的压缩策略:

```typescript
const modelPolicy = z.object({
  provider: z.string(),
  model: z.string(),
  thresholdRatio: thresholdRatioSchema,
  retainRatio: retainRatioSchema,
  retainTokens: retainTokensSchema,
  summarizationProvider: summarizationProviderSchema,  // 可以用不同模型做摘要
  summarizationModel: summarizationModelSchema,
  maxTokens: maxTokensSchema,
  compactionRetries: compactionRetriesSchema,
  maxOverflowRetries: maxOverflowRetriesSchema,
});
```

### 4.13 关键设计亮点

1. **ContextEngine 15 个生命周期方法**: 最完整的抽象接口
2. **promptAuthority 字段**: 防止组装后隐藏溢出风险
3. **工具配对保护**: tool call/result 必须成对压缩
4. **Quarantine 健康隔离**: 引擎异常后自动降级
5. **多模型策略**: 每个模型组合可配不同压缩参数
6. **压缩事件携带 LLM 元数据**: 支持日志重建

---

## 第 5 章 OpenCode — Prune + Compaction 双阶段

> **来源文件**: `opencode-核心机制深度分析.md`
> **核心文件**: `packages/opencode/src/session/compaction.ts` / `overflow.ts` / `message-v2.ts`

### 5.1 架构总览

OpenCode 采用 **Prune + Compaction** 两阶段设计，基于 Effect 框架:

```
Prune（工具输出裁剪）→ Compaction（LLM 摘要）→ Auto Continue
```

### 5.2 溢出检测

```typescript
// packages/opencode/src/session/overflow.ts
const COMPACTION_BUFFER = 20_000;

export function usable(input: { cfg; model; outputTokenMax? }) {
  const reserved = input.cfg.compaction?.reserved ??
    Math.min(COMPACTION_BUFFER, ProviderTransform.maxOutputTokens(input.model, input.outputTokenMax));
  return input.model.limit.input
    ? Math.max(0, input.model.limit.input - reserved)
    : Math.max(0, context - ProviderTransform.maxOutputTokens(input.model, input.outputTokenMax));
}

export function isOverflow(input) {
  if (input.cfg.compaction?.auto === false) return false;
  const count = input.tokens.total || input.tokens.input + output + cache.read + cache.write;
  return count >= usable(input);
}
```

**关键公式**: `可用窗口 = 模型上下文窗口 - max(输出token上限, 20000保留缓冲)`

在 `processor.ts` 的 `step-finish` 事件中触发。流处理使用 `Stream.takeUntil(() => ctx.needsCompaction)` 在检测到溢出时停止当前流。

### 5.3 Prune 策略

Prune 是**工具输出裁剪**，在 compaction 之前执行:

```typescript
export const PRUNE_MINIMUM = 20_000;   // 至少裁剪 20k tokens 才值得执行
export const PRUNE_PROTECT = 40_000;   // 保护最近 40k tokens 的工具输出
const PRUNE_PROTECTED_TOOLS = ["skill"];  // skill 工具永不裁剪
```

**核心逻辑**:

1. 从最新消息向前遍历
2. 跳过最近 2 轮对话
3. 到达上一次 compaction 边界则停止
4. 跳过已裁剪过和受保护的工具
5. 累计 token，超过 PRUNE_PROTECT 的部分标记为待裁剪
6. 只有裁剪量超过 PRUNE_MINIMUM 才执行

**Prune 的实际效果**: 不是删除工具输出，而是在 `part.state.time.compacted` 上标记时间戳。后续 `serialize()` 函数读到此标记时，输出变为 `"[Old tool result content cleared]"`。

### 5.4 Compaction 策略

使用一个**专用 compaction Agent** 生成摘要:

```typescript
const processCompaction = Effect.fn("SessionCompaction.process")(function* (input) {
  // 1. 使用 "compaction" 专用 Agent
  const agent = yield* agents.get("compaction");

  // 2. 序列化历史消息
  const conversation = selected.head.map(serialize).filter(Boolean).join("\n\n");

  // 3. 调用 LLM 生成摘要
  const result = yield* processor.process({
    agent,
    tools: {},  // compaction agent 无工具
    messages: [{ role: "user", content: nextPrompt }],
    model,
  });

  // 4. 如果压缩成功且配置了 auto-continue
  if (result === "continue" && input.auto) {
    yield* session.updatePart({
      type: "text",
      text: "Continue if you have next steps...",
      synthetic: true,
    });
  }
});
```

### 5.5 Tail 保留机制

`select()` 将消息分为 **head**（要压缩的）和 **tail**（要保留的）:

```typescript
const select = Effect.fn("SessionCompaction.select")(function* (input) {
  const budget = preserveRecentBudget({ cfg, model });  // 2k~15k tokens
  const all = turns(input.messages);

  let total = 0, keep: Tail | undefined;
  for (let i = recent.length - 1; i >= 0; i--) {
    const turn = recent[i];
    const size = yield* estimate({ messages: turn 区间, model });
    if (total + size <= budget) {
      total += size;
      keep = { start: turn.start, id: turn.id };
      continue;
    }
    const split = yield* splitTurn({ messages, turn, model, budget: budget - total });
    if (split) keep = split;
    break;
  }

  return {
    head: messages.slice(0, keep.start),
    tail_start_id: keep.id,
  };
});
```

**preserveRecentBudget**: 可用窗口的 25%，最少 2k，最多 15k tokens。

### 5.6 溢出时的 Replay 机制

当溢出发生在用户消息（含大文件附件）时:

1. 找到最近的非 compaction 用户消息
2. 截断到该消息之前
3. compaction 成功后，将 replay 消息重新插入
4. 大文件附件替换为文本描述

### 5.7 Token 估算

```typescript
const CHARS_PER_TOKEN = 4;
export const estimate = (input: string) => Math.max(0, Math.round(input.length / CHARS_PER_TOKEN));
```

简单的字符数/4 估算，不依赖 tiktoken 等库。

### 5.8 消息序列化格式

```
[User]: 请帮我修复这个 bug
[Assistant]: 我来查看代码
[Assistant tool call]: read({"path":"src/main.ts"})
[Tool result]: console.log("hello")...
[Assistant]: 问题在于...
```

工具输出限制为 2000 字符（`TOOL_OUTPUT_MAX_CHARS`）。

### 5.9 关键设计亮点

1. **Prune + Compaction 两阶段分离**: Prune 是零代价的标记操作，Compaction 才调用 LLM
2. **双阈值保护**: PRUNE_PROTECT=40K（保护近期）+ PRUNE_MINIMUM=20K（避免频繁小量裁剪）
3. **专用 compaction Agent**: 无工具，专注于摘要生成
4. **Replay 机制**: 溢出时自动重放最近用户消息
5. **Auto Continue**: 压缩后自动继续工作，用户无感知
6. **标记式裁剪**: 不删除原始数据，通过时间戳标记实现软删除

---

## 第 6 章 DeepSeek Harness — 事件源投影 + CompactionEngine

> **来源文件**: `deepseek-harness-核心机制深度分析.md`
> **核心文件**: `packages/compaction/compaction/src/index.ts` / `compaction-basic/src/index.ts` / `session-projection/src/index.ts`

### 6.1 架构总览

DeepSeek Harness 的 Context 管理融合了**事件源投影**和**可插拔压缩引擎**:

```
SessionEvent → SessionProjectionRegistry（纯 fold 函数）→ 投影状态
CompactionEngine（抽象）→ BasicCompactionEngine（实现）
    ├── 压力触发（agent/pre-step）
    └── 溢出触发（agent/request-error）
```

### 6.2 事件源投影框架

每个投影单元（ProjectionDefinition）注册一个纯 fold 函数:

```typescript
export interface ProjectionDefinition<K, S> {
  key: K;
  stateSchema: ZodType<S>;
  init(header: SessionHeader): S;
  apply(state: S, event: SessionEvent): S;  // 纯同步状态转换
  wire?: { viewSchema; view(state): View };
  stateVersion: number;
}
```

**关键规则**: `apply` 必须是纯同步函数; 不关心的事件必须返回相同引用（`Object.is`），利用引用相等性跳过下游通知。

### 6.3 CompactionEngine 抽象接口

```typescript
export abstract class CompactionEngine extends Service {
  abstract compactIfNeeded(agent, trigger, signal): Promise<CompactionResult | null>;
  abstract compactNow(agent, signal, sourceCommandId?): Promise<CompactionResult>;
  abstract compactRegion(start, end, agent, signal?): Promise<CompactionResult>;
}
```

### 6.4 双触发机制

```typescript
// compaction-basic/src/index.ts:137-165
private _registerAutomaticCompaction(): void {
  // 步骤压力: 每步开始前检查
  ctx.on('agent/pre-step', async ({ agent, signal }, next) => {
    const result = await this.compactIfNeeded(agent, 'pressure', signal);
    return next();
  });

  // 上下文溢出: LLM 返回 context_window_exceeded 时触发
  ctx.on('agent/request-error', async ({ agent, failure, signal }, next) => {
    if (failure.code !== CONTEXT_WINDOW_EXCEEDED_CODE) return next();
    const result = await this.compactIfNeeded(agent, 'context-overflow', signal);
    if (result !== null) return { kind: 'retry' };
    return next();
  });
}
```

### 6.5 压缩流程

```typescript
override async compactIfNeeded(agent, trigger, signal) {
  const meter = this.ctx.tokenMeter;
  let measurement = meter.measure(agent.session);

  // 1. 修剪工具结果（可选）
  const prune = this.ctx.get('toolResultPruner');
  if (prune !== undefined) {
    prune.pruneSession(agent.session);
    measurement = meter.measure(agent.session);
  }

  // 2. 检查是否超过阈值
  if (measurement.totalTokens < spec.thresholdTokens) return null;

  // 3. 选择可压缩范围（tool call/result 成对保护）
  const range = selectCompactableRange(agent.session, measurement, spec.retainTokens);

  // 4. 用 LLM 摘要 + 替换表面区域
  return this.compactRegion(range.start, range.end, agent, signal);
}
```

### 6.6 工具配对保护

压缩范围选择必须保证 **tool call/result 成对**:

```typescript
export function toolPairingBalancedBefore(session, seq): boolean;
export function toolPairingBalancedAfter(session, seq): boolean;
// 检查 seq 位置是否处于一个平衡的（未匹配的 tool call 之外的）位置
```

### 6.7 压缩事件

```typescript
'compaction/start'   // 锁定标记，防止并发压缩
'compaction/summary' // 摘要 + 影子范围 + token 统计 + LLM 调用元数据
'compaction/prune'   // 模型无关的修剪（无摘要，直接删除）
```

### 6.8 多模型策略

```typescript
const modelPolicy = z.object({
  provider: z.string(),
  model: z.string(),
  thresholdRatio: thresholdRatioSchema,
  retainRatio: retainRatioSchema,
  retainTokens: retainTokensSchema,
  summarizationProvider: summarizationProviderSchema,  // 可以用不同模型做摘要
  summarizationModel: summarizationModelSchema,
  maxTokens: maxTokensSchema,
  compactionRetries: compactionRetriesSchema,
  maxOverflowRetries: maxOverflowRetriesSchema,
});
```

### 6.9 持久化投影缓存的恢复算法

```typescript
restoreFloor(checkpoint): number | undefined {
  let floor: number | undefined;
  for (const registration of this.registrations.values()) {
    const row = checkpoint[registration.def.key];
    const need = row !== undefined && row.ver === registration.def.stateVersion
      ? Math.max(row.seq + 1, 0)  // 从 checkpoint 的下一个 seq 开始
      : 0;                         // version 不匹配，从头 fold
    floor = floor === undefined ? need : Math.min(floor, need);
  }
  return floor === undefined ? undefined : Math.max(floor - 1, 0);  // "one-below anchor"
}
```

**one-below anchor** 的设计: 返回 `floor - 1` 而非 `floor`，让持久层从 `floor - 1` 开始读取，这样可以检测日志是否缩小到了 checkpoint 的 watermark 以下（crash-repair truncation）。

### 6.10 关键设计亮点

1. **事件源投影**: 纯同步 fold 函数 + 引用相等性跳过通知，性能极优
2. **工具配对保护**: 切割点不能落在 tool call/result 之间
3. **多模型策略**: 每个模型组合可配不同压缩参数，包括使用不同模型做摘要
4. **CompactionEngine 可插拔**: 通过 Cordis 注册表实现热替换
5. **one-below anchor**: 精巧的恢复算法，支持 crash-repair

---

## 第 7 章 Pi — 压缩 + 分支摘要

> **来源文件**: `pi-核心机制深度分析.md`
> **核心文件**: `packages/agent/src/harness/compaction/compaction.ts` / `branch-summarization.ts` / `session/context.ts`

### 7.1 架构总览

Pi 采用**压缩 + 分支摘要**双机制:

```
shouldCompact（触发判断）
    ├── findCutPoint（切割点选择）
    ├── generateSummary（LLM 摘要）
    └── branch-summarization（分支摘要，lane 切换时）
```

### 7.2 触发条件

```typescript
export function shouldCompact(
  contextTokens: number,
  contextWindow: number,
  settings: CompactionSettings
): boolean {
  if (!settings.enabled) return false;
  return contextTokens > contextWindow - settings.reserveTokens;
}
```

默认配置:

```typescript
export const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = {
  enabled: true,
  reserveTokens: 16384,     // 给摘要 prompt 预留的 token
  keepRecentTokens: 20000,  // 压缩后保留的最近上下文 token 数
};
```

### 7.3 切割点选择（findCutPoint）

1. 从后往前累计 token，当累积 >= `keepRecentTokens` 时找到切割位置
2. 切割点必须是有效的 turn 边界（user 消息 / bashExecution / branchSummary）
3. 如果切割点在 turn 中间（`isSplitTurn`），记录 `turnStartIndex` 用于生成 turn 前缀摘要

### 7.4 摘要生成（generateSummary）

- 使用当前模型（非独立摘要模型）
- 两次 LLM 调用: 第一次生成历史摘要，第二次生成 turn 前缀摘要（仅 `isSplitTurn` 时）
- **结构化摘要格式**: `## Goal` / `## Constraints` / `## Progress` / `## Key Decisions` / `## Next Steps` / `## Critical Context`
- 摘要末尾追加文件操作列表（readFiles / modifiedFiles）

### 7.5 增量更新

当已有旧摘要时，使用增量更新 prompt（`UPDATE_SUMMARIZATION_PROMPT`），保留旧信息并合并新进展，避免从头重新生成。

### 7.6 分支摘要（Branch Summarization）

当用户从 lane A 导航到 lane B 时:

```typescript
export async function collectEntriesForBranchSummary(
  session: Session,
  oldLeafId: string | null,
  targetId: string,
): Promise<CollectEntriesResult>
```

1. 找到 oldLeafId 和 targetId 的**最近公共祖先**（common ancestor）
2. 收集 oldLeafId 到 common ancestor 之间的所有 entry
3. 在 token 预算内选择要摘要的 entry
4. 生成摘要并追加到 session tree 的目标分支上

### 7.7 文件操作追踪（Pi 独有）

```typescript
export function extractFileOpsFromMessage(message: AgentMessage, fileOps: FileOperations): void {
  for (const block of message.content) {
    if (block.type !== "toolCall") continue;
    switch (block.name) {
      case "read": fileOps.read.add(path); break;
      case "write": fileOps.written.add(path); break;
      case "edit": fileOps.edited.add(path); break;
    }
  }
}
```

摘要输出格式:

```xml
<read-files>
src/foo.ts
src/bar.ts
</read-files>

<modified-files>
src/baz.ts
</modified-files>
```

这让模型在压缩后仍能知道"哪些文件被读过、哪些被改过"，避免重复操作。

### 7.8 CompactionEntry 持久化结构

```typescript
export interface CompactionEntry extends EntryBase {
  type: "compaction";
  summary: string;              // 生成的摘要文本
  retainedTail: AgentMessage[]; // 压缩后保留的最近消息（完整保留，不截断）
  tokensBefore: number;         // 压缩前的总 token 数
  details?: unknown;            // CompactionDetails { readFiles, modifiedFiles }
  usage?: Usage;                // 摘要生成时的 LLM 使用量
}
```

**retainedTail** 的设计: 它不是摘要的一部分，而是**完整保留的最近消息**。压缩后的上下文 = compaction summary + retainedTail。模型看到的是"摘要 + 原始最近对话"的组合，而非纯摘要。

### 7.9 Split Turn 处理

当压缩切割点落在一个 turn 的中间时（`isSplitTurn`）:

1. **历史摘要**: 切割点之前的所有消息
2. **Turn 前缀摘要**: 切割点到 turn 开始之间的消息（用 `TURN_PREFIX_SUMMARIZATION_PROMPT`）

最终 compaction summary = 历史摘要 + `---` + turn 前缀摘要。

### 7.10 Token 估算

```typescript
export function estimateTokens(message: AgentMessage): number {
  // 字符数 / 4 的粗略估算
  // 图片按 4800 字符估算
  // thinking + toolCall 参数都计入
}
```

上下文使用量优先使用 provider 返回的 `usage`（精确），仅在没有 provider 数据时回退到字符估算。

### 7.11 关键设计亮点

1. **结构化摘要格式**: Goal/Constraints/Progress/Key Decisions/Next Steps，比自由格式更利于恢复
2. **文件操作追踪**: 压缩后仍知道哪些文件被操作过（Pi 独有）
3. **retainedTail**: 摘要 + 原始最近对话的组合，保留上下文连续性
4. **Split Turn 处理**: 切割点在 turn 中间时分别生成历史摘要和 turn 前缀摘要
5. **增量更新**: 已有摘要时不重写，只合并新进展
6. **Cache control**: 在 system prompt 和最后 user message 上加 `cache_control: ephemeral`

---

## 第 8 章 设计模式提炼

### 模式 1: 渐进式压缩（Progressive Compaction）

**定义**: 压缩分为多个级别，从最轻量的工具结果清理到最重量的全文 LLM 摘要，按压力逐级升级。

**采用者**: Claude Code（4 级）、AtomCode（3 级）

**laew 借鉴**: 建立三级压缩: Tier 0（工具结果 stub）→ Tier 1（硬截断）→ Tier 2（LLM 摘要）。

### 模式 2: Prune-then-Compact（先裁剪后压缩）

**定义**: 先执行零代价或低代价的工具输出裁剪（Prune），释放空间后仍不够再执行 LLM 压缩（Compact）。

**采用者**: OpenCode（PRUNE_PROTECT + PRUNE_MINIMUM 双阈值）、OpenClaw（toolResultPruner）、DeepSeek Harness（toolResultPruner）

**laew 借鉴**: SubAgent-Work 的工具输出应支持标记式裁剪，序列化时替换为 `[Old tool result content cleared]`。

### 模式 3: Sacred Floor / 前缀保护

**定义**: System 消息 + 第一个真实 User 消息（任务提示词）永远不被压缩删除。

**采用者**: AtomCode（sacred_floor）、Claude Code（system prompt + 工具定义不变）

**laew 借鉴**: Session 压缩必须保护系统提示词和首个用户任务不被丢弃。

### 模式 4: Monotonic / Idempotent Compaction

**定义**: 压缩操作是单调的（stub 不可逆）或幂等的（TRUNCATE_MARKER 防止重复截断），保证 prefix cache 稳定。

**采用者**: AtomCode（cache_epoch + committed 才 bump）、Claude Code（cache-safe 参数）

**laew 借鉴**: 压缩操作不应改变已压缩部分的字节，保证 prefix cache 只在压缩轮 break 一次。

### 模式 5: Tool Call/Result 成对保护

**定义**: 压缩的切割点不能落在一个 tool call 和它的 result 之间。

**采用者**: OpenClaw（toolPairingBalancedBefore/After）、DeepSeek Harness（同上）

**laew 借鉴**: 压缩时必须以消息轮次为边界，不能拆散 tool call/result 对。

### 模式 6: Tail 保留（Retained Tail）

**定义**: 压缩后保留最近 N 条完整消息（不截断），与摘要组成"摘要 + 原始最近对话"的上下文。

**采用者**: Pi（retainedTail）、OpenCode（tail_start_id）、AtomCode（keep_recent_turns）、Claude Code（POST_COMPACT_MAX_FILES_TO_RESTORE）

**laew 借鉴**: 压缩后应保留最近 2-3 轮完整对话，不全部摘要化。

### 模式 7: 增量摘要（Incremental Summary）

**定义**: 已有摘要时不重写，只合并新进展。

**采用者**: AtomCode（`<previous-summary>` 增量更新）、Pi（UPDATE_SUMMARIZATION_PROMPT）

**laew 借鉴**: SessionContext Agent 生成摘要时应支持增量更新模式。

### 模式 8: 断路器（Circuit Breaker）

**定义**: 压缩连续失败 N 次后停止重试，防止不可恢复的上下文反复触发压缩。

**采用者**: Claude Code（MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES=3）、AtomCode（SUMMARY_TIMEOUT=180s 降级）

**laew 借鉴**: 压缩应有最大重试次数和超时降级路径。

### 模式 9: 文件操作追踪

**定义**: 压缩摘要末尾追加 readFiles/modifiedFiles 列表，让模型知道哪些文件已被操作过。

**采用者**: Pi（extractFileOpsFromMessage）、Claude Code（post-compact file attachments）

**laew 借鉴**: 从 BashTool/ReadTool/WriteTool 结果中提取文件路径信息，注入压缩摘要。

### 模式 10: 专用压缩 Agent

**定义**: 压缩使用一个无工具的专用 Agent，专注于摘要生成，避免在压缩过程中触发工具调用。

**采用者**: OpenCode（agents.get("compaction")，无工具）、Claude Code（runForkedAgent，禁止工具调用）

**laew 借鉴**: 可新增一个隐藏的压缩 Agent 角色，或复用 SessionContext Agent。

### 模式 11: 溢出自动恢复（Overflow Auto-Recovery）

**定义**: LLM 返回上下文溢出错误时，自动触发压缩并重试请求，用户无感知。

**采用者**: AtomCode（Overflow attempt 递增）、OpenClaw（context-overflow 事件 + retry）、DeepSeek Harness（同上）、OpenCode（replay 机制）

**laew 借鉴**: 在 LLM 请求返回上下文溢出错误时，自动触发压缩并重试。

### 模式 12: 并发压缩保护

**定义**: 通过锁/标记防止多个压缩任务同时执行。

**采用者**: OpenClaw（compaction/start 锁定事件）、DeepSeek Harness（markRuntimeCompactionDelegate）

**laew 借鉴**: 压缩操作应有互斥锁，防止多 Agent 并发触发压缩。

---

## 第 9 章 对 laew 的综合建议

### 9.1 laew 现状分析

laew 当前的 Context 管理状态:

- **零压缩**: 完全没有上下文压缩机制，对话变长后 token 会超出窗口
- **SessionContext Agent**: 每个任务完成后生成自由格式摘要写入 `session_memory` 表，但这不是压缩——它是持久化记忆，不减少当前对话的 token
- **Token 估算**: 无
- **Prompt Cache**: 无集成
- **工具结果长度控制**: 无
- **溢出恢复**: 无

### 9.2 推荐实施路线图

#### P0: 基础 Token 管理（1-2 天）

**目标**: 建立 token 估算和溢出检测基础。

1. **Token 估算函数**（借鉴 OpenCode/Pi）

```rust
// src/llm/token.rs
const CHARS_PER_TOKEN: f64 = 4.0;

pub fn estimate_tokens(text: &str) -> u32 {
    // 中文等多字节字符按 2 chars/token 估算
    let multi_byte_count = text.chars().filter(|c| (*c as u32) > 0x7F).count();
    let single_byte_count = text.len() - multi_byte_count;
    ((single_byte_count as f64 / CHARS_PER_TOKEN) + (multi_byte_count as f64 / 2.0)).ceil() as u32
}
```

2. **上下文窗口溢出检测**（借鉴 OpenCode）

```rust
// src/agent/context.rs
pub fn is_overflow(used_tokens: u32, context_window: u32, reserve_tokens: u32) -> bool {
    used_tokens >= context_window.saturating_sub(reserve_tokens)
}
```

3. **LLM 响应溢出错误检测**（借鉴 AtomCode 的 9 种签名检测）

```rust
// src/llm/error.rs
pub fn is_context_overflow(error: &LlmError) -> bool {
    // 检测 Anthropic / OpenAI 的上下文溢出错误码
    matches!(error,
        LlmError::AnthropicOverloaded { .. }
        | LlmError::OpenAiContextLength { .. }
        // ... 9 种签名
    )
}
```

#### P1: Prune 工具结果裁剪（2-3 天）

**目标**: 降低工具输出的 token 占用。

1. **工具输出长度限制**（借鉴 Pi 的 2000 字符 / OpenCode 的 TOOL_OUTPUT_MAX_CHARS）

```rust
// src/agent/tools/mod.rs
const TOOL_OUTPUT_MAX_CHARS: usize = 4000;

pub fn truncate_tool_output(output: &str) -> String {
    if output.len() <= TOOL_OUTPUT_MAX_CHARS {
        output.to_string()
    } else {
        format!("{}...\n[truncated, {} chars total]", &output[..TOOL_OUTPUT_MAX_CHARS], output.len())
    }
}
```

2. **标记式软删除**（借鉴 OpenCode 的 compacted 时间戳标记）

```rust
// src/session/message.rs
pub struct ToolResult {
    pub output: String,
    pub compacted_at: Option<i64>,  // 被裁剪的时间戳
}

impl ToolResult {
    pub fn effective_output(&self) -> &str {
        if self.compacted_at.is_some() {
            "[Old tool result content cleared]"
        } else {
            &self.output
        }
    }
}
```

#### P2: Tier 0 Stub Compaction（3-5 天）

**目标**: 实现最轻量的压缩——将旧工具结果替换为一行摘要（借鉴 AtomCode）。

1. **Sacred Floor 保护**

```rust
// src/session/context.rs
pub fn sacred_floor(messages: &[Message]) -> usize {
    // 永远保护 System + 第一个真实 User 消息
    let has_system = messages.first().map_or(false, |m| m.role == Role::System);
    let offset = usize::from(has_system);
    match messages.iter().position(|m| m.role == Role::User && !m.synthetic) {
        Some(idx) => idx + 1,
        None => offset,
    }
}
```

2. **Stub 生成**

```rust
// src/agent/compact.rs
const MIN_COLLAPSE_SIZE: usize = 500;

pub fn generate_stub(tool_name: &str, output_len: usize, success: bool) -> String {
    let status = if success { "success" } else { "error" };
    format!("[tool: {}] Output ({} chars, {})", tool_name, output_len, status)
}
```

3. **Auto 触发**

```rust
pub fn auto_compact_trigger(used_tokens: u32, ctx_window: u32) -> Option<CompactTrigger> {
    if ctx_window == 0 { return None; }
    let utilization = used_tokens as f32 / ctx_window as f32;
    if utilization >= 0.70 {
        Some(CompactTrigger::Stub)
    } else {
        None
    }
}
```

#### P3: LLM 摘要压缩（5-7 天）

**目标**: 当 Stub 不够时，使用 LLM 生成结构化摘要。

1. **专用压缩角色**（借鉴 OpenCode 的 compaction Agent）

在 `MultiAgentOrchestrator` 中新增一个隐藏的 Compaction Agent，无工具，专注摘要。

2. **结构化摘要格式**（借鉴 Pi）

```markdown
## Goal
{用户的主要目标}

## Progress
{已完成的关键步骤}

## Key Decisions
{已做出的重要决策}

## Next Steps
{下一步计划}

## Critical Context
{关键上下文信息}

<read-files>
{已读取的文件列表}
</read-files>

<modified-files>
{已修改的文件列表}
</modified-files>
```

3. **Tail 保留**

```rust
pub struct CompactResult {
    pub summary: String,
    pub retained_tail: Vec<Message>,  // 保留最近 N 条完整消息
    pub tokens_before: u32,
    pub tokens_after: u32,
}
```

#### P4: Overflow 自动恢复（2-3 天）

**目标**: LLM 返回上下文溢出错误时自动压缩并重试。

```rust
// src/agent/mod.rs
async fn run_with_overflow_recovery(&mut self) -> Result<Response, AgentError> {
    for attempt in 0..=2 {
        match self.call_llm().await {
            Ok(response) => return Ok(response),
            Err(e) if e.is_context_overflow() && attempt < 2 => {
                self.compact(CompactTrigger::Overflow { attempt }).await?;
            }
            Err(e) => return Err(e),
        }
    }
    Err(AgentError::ContextOverflow)
}
```

#### P5: Prompt Cache 集成（3-5 天）

**目标**: 利用 Anthropic API 的 prompt cache 特性。

1. 在系统提示和工具定义上添加 cache_control 标记
2. 压缩操作保证 cache_epoch 单调递增
3. 压缩后确保 prefix cache 只在压缩轮 break 一次

#### P6: 压缩后恢复（3-5 天）

**目标**: 压缩后重新注入关键上下文。

1. 最近读取的文件内容（借鉴 Claude Code 的 POST_COMPACT_MAX_FILES_TO_RESTORE=5）
2. 当前 plan 摘要（如果 Plan Agent 有活跃计划）
3. 文件操作列表（借鉴 Pi 的 readFiles/modifiedFiles）

### 9.3 按 Agent 角色分配压缩职责

| Agent 角色 | 压缩相关职责 |
|-----------|------------|
| **Yolo Agent** | 检测 token 使用情况; 在分类时考虑上下文压力; 注入历史摘要 |
| **Plan Agent** | plan 内容在压缩后必须重新注入 |
| **Main-Work Agent** | 编排 Prune 和 Compaction; 管理压缩触发 |
| **SubAgent-Work Agent** | 工具输出长度限制; 标记式裁剪 |
| **Quality-Check Agent** | 压缩后检查上下文完整性 |
| **SessionContext Agent** | 改造为支持增量摘要更新; 追踪文件操作 |
| **Compaction Agent**（新增） | 无工具，专注 LLM 摘要生成 |

### 9.4 推荐的 Token 预算常量

基于 6 个项目的横向对比，推荐以下初始值:

| 常量 | 推荐值 | 参考来源 |
|------|--------|---------|
| `CHARS_PER_TOKEN` | 4.0 | OpenCode / Pi |
| `TOOL_OUTPUT_MAX_CHARS` | 4000 | Pi（2000）× 2 |
| `MIN_COLLAPSE_SIZE` | 500 | AtomCode |
| `STUB_TRIGGER_RATIO` | 0.70 | AtomCode |
| `SUMMARY_TRIGGER_RATIO` | 0.78 | AtomCode |
| `RESERVE_TOKENS` | 16384 | Pi |
| `KEEP_RECENT_TOKENS` | 20000 | Pi |
| `RECENT_KEEP_FRACTION` | 0.25 | AtomCode |
| `MIN_RECENT_KEEP_TOKENS` | 8000 | AtomCode |
| `MAX_RECENT_KEEP_TOKENS` | 256000 | AtomCode |
| `MAX_SUMMARY_BYTES` | 65536 | AtomCode |
| `MAX_SUMMARY_TOKENS` | 16000 | AtomCode |
| `SUMMARY_TIMEOUT_SECS` | 180 | AtomCode / OpenClaw |
| `MAX_COMPACT_FAILURES` | 3 | Claude Code |
| `POST_COMPACT_FILES_TO_RESTORE` | 5 | Claude Code |
| `POST_COMPACT_FILE_TOKEN_BUDGET` | 5000 | Claude Code |
| `PRUNE_MINIMUM_TOKENS` | 20000 | OpenCode |
| `PRUNE_PROTECT_TOKENS` | 40000 | OpenCode |
| `OVERFLOW_MAX_RETRIES` | 3 | AtomCode |

### 9.5 实施优先级矩阵

| 阶段 | 内容 | 复杂度 | 价值 | 优先级 |
|------|------|--------|------|--------|
| P0 | Token 估算 + 溢出检测 | 低 | 高 | 立即 |
| P1 | Prune 工具结果裁剪 | 低 | 高 | P0 后立即 |
| P2 | Tier 0 Stub Compaction | 中 | 高 | P1 后 |
| P3 | LLM 摘要压缩 | 高 | 高 | P2 后 |
| P4 | Overflow 自动恢复 | 中 | 中 | P2 后 |
| P5 | Prompt Cache 集成 | 中 | 中 | P3 后 |
| P6 | 压缩后恢复 | 高 | 中 | P3 后 |

### 9.6 特别注意事项

1. **Rust 实现参考 AtomCode**: laew 是 Rust 项目，AtomCode 的 `compaction.rs`（2345 行）是最直接的参考。StubCompaction、OverflowCompaction、CompactionPlan 等数据结构可以直接借鉴。

2. **多 Agent 架构的独特挑战**: laew 有 6 个 Agent 角色，每个角色有独立的 Agent-Context。压缩需要考虑:
   - SubAgent-Work 的工具输出是否在压缩范围内?
   - SessionContext Agent 的摘要与压缩摘要如何协调?
   - Plan Agent 的输出在压缩后如何重新注入?

3. **SQLite 持久化**: laew 已有 SQLite 基础设施（`LsmAgentEmergentWork.db`），压缩状态可以复用现有表结构。

4. **不要过度工程化**: 先从最简单的 P0（token 估算）开始，逐步升级。参考 AtomCode 的三级 ladder 思路，每升一级都是对前一级的补充而非替换。

---

## 附录 A: 核心文件索引

| 项目 | Context 管理核心文件 |
|------|---------------------|
| Claude Code | `src/services/compact/compact.ts` (1706 行), `microCompact.ts` (531 行), `autoCompact.ts` (352 行), `prompt.ts` (375 行) |
| AtomCode | `crates/atomcode-capabilities/src/compaction.rs` (2345 行), `crates/atomcode-kernel/src/message.rs`, `agent.rs`, `checkpoint.rs` |
| OpenClaw | `src/context-engine/types.ts`, `compaction-basic/src/index.ts`, `compaction/src/tool-pairing.ts`, `compaction-basic/src/summarizer.ts` |
| OpenCode | `packages/opencode/src/session/compaction.ts`, `overflow.ts`, `message-v2.ts`, `processor.ts` |
| DeepSeek Harness | `packages/compaction/compaction/src/index.ts`, `compaction-basic/src/index.ts`, `compaction-basic/src/region.ts`, `session-projection/src/index.ts` |
| Pi | `packages/agent/src/harness/compaction/compaction.ts`, `branch-summarization.ts`, `compaction/utils.ts`, `session/context.ts` |

## 附录 B: 压缩触发条件速查

| 项目 | 触发条件 | 阈值 |
|------|---------|------|
| Claude Code | `tokenCount >= contextWindow - maxOutputTokens(min 20K) - 13K` | 自适应 |
| AtomCode | `utilization >= 0.70`（Stub）/ `>= 0.78`（Summarize） | 比率制 |
| OpenClaw | `totalTokens >= contextWindow * thresholdRatio`（默认 0.8） | 比率制 |
| OpenCode | `tokenCount >= usableWindow`（窗口 - max(输出上限, 20K)） | 绝对值 |
| DeepSeek Harness | `totalTokens >= contextWindow * thresholdRatio`（默认 0.8） | 比率制 |
| Pi | `contextTokens > contextWindow - 16384` | 绝对值 |

## 附录 C: 压缩策略分类法

```
按操作代价分类:
├── 零代价（本地操作）
│   ├── 标记式裁剪: OpenCode（compacted 时间戳）
│   ├── content-clear: Claude Code（替换为常量）
│   └── stub 生成: AtomCode（替换为一行摘要）
├── 低代价（API 特性）
│   └── cache_edits: Claude Code（远程删除缓存条目）
├── 中代价（硬截断）
│   └── truncate_rewrites: AtomCode Tier 1（截断 + 标记）
└── 高代价（LLM 调用）
    ├── 全文摘要: Claude Code / AtomCode / OpenCode / Pi
    ├── 增量摘要: AtomCode（previous-summary 模式）
    └── 分支摘要: Pi（最近公共祖先算法）

按触发方式分类:
├── 时间触发: Claude Code Time-Based MC（对话间隔超时）
├── 压力触发: 所有项目（token 超阈值）
├── 溢出触发: AtomCode / OpenClaw / DeepSeek / OpenCode（LLM 返回错误）
└── 手动触发: Claude Code Partial / AtomCode /compact / OpenClaw compactNow
```

## 附录 D: 六项目压缩成熟度评分

| 维度 | Claude Code | AtomCode | OpenClaw | OpenCode | DeepSeek Harness | Pi |
|------|-------------|----------|----------|----------|-----------------|-----|
| 压缩层级丰富度 | 5/5 | 4/5 | 3/5 | 3/5 | 3/5 | 2/5 |
| Prompt Cache 集成 | 5/5 | 5/5 | 3/5 | 1/5 | 1/5 | 3/5 |
| 溢出自动恢复 | 4/5 | 5/5 | 4/5 | 4/5 | 4/5 | 3/5 |
| 尾部保留机制 | 4/5 | 4/5 | 3/5 | 4/5 | 3/5 | 5/5 |
| 工具结果处理 | 5/5 | 5/5 | 3/5 | 5/5 | 3/5 | 3/5 |
| 前缀保护 | 4/5 | 5/5 | 2/5 | 2/5 | 2/5 | 2/5 |
| 摘要质量保障 | 5/5 | 4/5 | 3/5 | 3/5 | 3/5 | 5/5 |
| 断路器/降级 | 5/5 | 4/5 | 3/5 | 1/5 | 2/5 | 1/5 |
| **总分** | **37/40** | **36/40** | **24/40** | **23/40** | **21/40** | **24/40** |

### 评分说明

- **Claude Code** 和 **AtomCode** 并列最高，分别代表 TypeScript 和 Rust 生态的最优实践
- **Claude Code** 的四级管线 + 缓存编辑型微压缩是独有创新
- **AtomCode** 的 sacred_floor + cache_epoch + 9 种溢出签名检测最适合 Rust 参考
- **OpenClaw** 和 **DeepSeek Harness** 偏框架抽象，压缩实现在插件层
- **OpenCode** 的 Prune 双阈值设计实用但断路器缺失
- **Pi** 的结构化摘要 + 文件操作追踪 + retainedTail 是摘要质量最好的

---

## 附录 E: 关键术语表

| 术语 | 含义 | 首次出现 |
|------|------|---------|
| `sacred_floor` | 永不被压缩删除的消息下界（System + 首个 User） | AtomCode |
| `cache_epoch` | prefix cache 世代标记，只在 committed compaction 时 bump | AtomCode |
| `stub` | 工具结果的一行摘要替代（不可逆、单调） | AtomCode |
| `retainedTail` | 压缩后完整保留的最近消息 | Pi / OpenCode |
| `tail_start_id` | 保留消息的起始 ID | OpenCode |
| `thresholdRatio` | 触发压缩的 token 使用率阈值 | OpenClaw / DeepSeek |
| `reserveTokens` | 为摘要 prompt 预留的 token 数 | Pi |
| `keepRecentTokens` | 压缩后保留的最近 token 数 | Pi |
| `toolPairing` | tool call/result 必须成对出现的约束 | OpenClaw / DeepSeek |
| `quarantine` | 引擎异常后进入隔离状态，自动降级到默认引擎 | OpenClaw |
| `compaction/start` | 并发压缩保护的锁定标记 | OpenClaw / DeepSeek |
| `PRUNE_PROTECT` | 不被裁剪的最近工具输出 token 量 | OpenCode |
| `PRUNE_MINIMUM` | 触发裁剪的最小裁剪量阈值 | OpenCode |
| `AUTOCOMPACT_BUFFER_TOKENS` | auto-compact 触发前的安全缓冲 | Claude Code |
| `runForkedAgent` | 共享 prompt cache 的子 Agent 调用模式 | Claude Code |
| `cache_edits` | Anthropic API 的远程缓存编辑特性 | Claude Code |
| `one-below anchor` | 恢复算法中返回 floor-1 以检测 crash-repair | DeepSeek Harness |
| `CompactTrigger` | 压缩触发枚举: Auto / Overflow / Manual | AtomCode |
| `contextProjection` | 上下文投影模式: per_turn / thread_bootstrap | OpenClaw |
| `CompactionEntry` | 压缩后的持久化结构（摘要 + retainedTail + 统计） | Pi |
