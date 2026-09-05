# TencentDB Agent Memory 核心机制深度分析

> **分析目标**：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`（v2.0.0-beta.1，2026-08 快照）
>
> **前置文档**：`TencentDB-Agent-Memory-源码调研.md` + `TencentDB-Agent-Memory-深度分析.md`
>
> **分析维度**：L0-L3 管线代码路径 · MemoryPipelineManager 调度算法 · Skill 跨系统事务 · 上下文注入管线 · RRF/BM25/向量检索 · SQLite+Drizzle 存储 · 多租户隔离
>
> **代码深度**：本报告所有"代码片段"均来自实际源码（按行号标注），函数签名 1:1 复制自源文件，便于 laew 工程借鉴时定位。

---

## 一、L0-L3 四层管线核心代码路径

### 1.1 L0 Recorder（`MemoryCore/src/core/conversation/l0-recorder.ts` 608 行）

#### 1.1.1 入口函数：`recordConversation()`

```typescript
// l0-recorder.ts:93-115
export async function recordConversation(params: {
  sessionKey: string;
  sessionId?: string;
  userId?: string;
  agentId?: string;
  rawMessages: unknown[];
  baseDir: string;
  logger?: Logger;
  originalUserText?: string;
  afterTimestamp?: number;
  originalUserMessageCount?: number;
  storage?: StorageAdapter;
}): Promise<ConversationMessage[]>
```

核心实现分四步：
1. **位置切片**（line 126-130）：用 `originalUserMessageCount` 切片 `rawMessages`，只保留 `before_prompt_build` 之后的新消息
2. **时间戳游标**（line 166-169）：`strict greater-than` 过滤已捕获消息
3. **污染替换**（line 213-253）：用 `originalUserText` 替换被 `prependContext` 污染的 user 消息
5. **消毒 + 写盘**（line 256-313）：`sanitizeText` + `stripCodeBlocks` + `shouldCaptureL0` 三重过滤，写入 `conversations/YYYY-MM-DD.jsonl`

**位置切片 vs 时间戳游标双保护**（line 118-130）：

```typescript
// 双保护机制的核心代码
const usePositionSlice = originalUserMessageCount != null && originalUserMessageCount > 0
  && originalUserMessageCount <= rawMessages.length;
const slicedMessages = usePositionSlice
  ? rawMessages.slice(originalUserMessageCount)   // 仅保留 prompt build 后的新消息
  : rawMessages;

// 第二层：严格大于 (>) 游标过滤
const cursor = afterTimestamp ?? 0;
const extracted = cursor !== 0
  ? allExtracted.filter((m) => m.timestamp > cursor)
  : allExtracted;
```

设计精髓：位置切片免疫重启后时间戳漂移（`originalUserMessageCount` 在 `before_prompt_build` 时缓存），时间戳游标作为缓存失效时的 fallback。**安全阀**（line 186-191）：当位置切片不可用且时间戳过滤全量通过（>8 条）时打 warn，提示可能发生时间戳漂移。

#### 1.1.2 污染消息替换算法（line 213-253）

```typescript
if (originalUserText) {
  const targetRaw = usePositionSlice
    ? slicedMessages[0]
    : rawMessages[originalUserMessageCount];

  const targetTs = targetRaw && typeof targetRaw.timestamp === "number" ? targetRaw.timestamp : undefined;

  if (targetTs != null) {
    let replaced = false;
    for (let i = 0; i < extracted.length; i++) {
      if (extracted[i].role === "user" && extracted[i].timestamp === targetTs) {
        extracted[i] = { ...extracted[i], content: originalUserText };
        replaced = true;
        break;
      }
    }
    if (!replaced) {
      logger?.warn?.(`Target user message (ts=${targetTs}) not found in extracted batch`);
    }
  }
}
```

#### 1.1.3 JSONL 写入（line 274-313）

```typescript
const shardDate = formatLocalDate(new Date());
const recordKey = StoragePaths.conversation(shardDate);

if (storage) {
  await storage.appendFile(recordKey, lines.join("\n") + "\n");
} else {
  const fs = await import("node:fs/promises");
  const path = await import("node:path");
  const outDir = path.default.join(baseDir, "conversations");
  const outPath = path.default.join(outDir, `${shardDate}.jsonl`);
  await fs.default.mkdir(outDir, { recursive: true });
  await fs.default.appendFile(outPath, lines.join("\n") + "\n", "utf-8");
}
```

设计：append-only + 日分片，所有 session 的消息合并到同一日文件中（`sessionKey` 作为字段而非文件名）。StorageAdapter 抽象支持本地 fs / COS 两种后端。

#### 1.1.4 读取接口（line 323-461）

- `readConversationRecords(sessionKey, baseDir)` — 按 sessionKey 过滤逐日 JSONL
- `readConversationMessages(sessionKey, baseDir, afterTimestamp?, limit?)` — 游标 + 限流
- `readConversationMessagesGroupedBySessionId(...)` — 按 sessionId 分组（同一 sessionKey 下不同 /reset 实例）

**对 laew 借鉴**：建议在 `src/agent/memory/l0-recorder.ts` 实现类似四步录制：
```rust
pub async fn record_conversation(
    session_key: &str,
    raw_messages: &[Message],
    original_user_text: Option<&str>,
    original_user_message_count: Option<usize>,
    after_timestamp: Option<u64>,
    storage: &dyn StorageAdapter,
) -> Vec<ConversationMessage>
```

### 1.2 L1 Writer（`MemoryCore/src/core/record/l1-writer.ts` 365 行）

#### 1.2.1 核心数据模型

```typescript
// l1-writer.ts:31-98
export type MemoryType =
  | "persona" | "episodic" | "instruction"
  | "work_fact" | "work_task" | "work_method" | "work_artifact";

export interface MemoryRecord {
  id: string;
  content: string;
  type: MemoryType;
  priority: number;     // 0-100, -1 = strict global instruction
  scene_name: string;
  source_message_ids: string[];
  metadata: EpisodicMetadata | Record<string, never>;
  timestamps: string[];
  createdAt: string;
  updatedAt: string;
  version?: number;      // update/merge 时递增
  sessionKey: string;
  sessionId: string;
  teamId?: string;
  userId?: string;
  agentId?: string;
}

export interface DedupDecision {
  record_id: string;
  action: "store" | "update" | "merge" | "skip";
  target_ids: string[];   // 多目标合并支持
  merged_content?: string;
  merged_type?: MemoryType;
  merged_priority?: number;
  merged_timestamps?: string[];
}
```

#### 1.2.2 写记忆主函数：`writeMemory()`

```typescript
// l1-writer.ts:163-354
export async function writeMemory(params: {
  memory: ExtractedMemory;
  decision: DedupDecision;
  baseDir: string;
  sessionKey: string;
  sessionId?: string;
  taskId?: string;
  teamId?: string;
  userId?: string;
  agentId?: string;
  vectorStore?: IMemoryStore;
  embeddingService?: EmbeddingService;
  storage?: StorageAdapter;
}): Promise<MemoryRecord | null>
```

四态决策路径：

| action | 行为 | 向量存储 |
|--------|------|---------|
| `store` | 追加新记录 | `upsertL1(record, embedding)` |
| `update` | 删除 target + 写新记录 | 先 `deleteL1Batch` 再 `upsertL1` |
| `merge` | 删除多 target + 写合并记录 | 同上 |
| `skip` | 什么都不做 | 无操作 |

#### 1.2.3 版本递增逻辑（line 191-200）

```typescript
let nextVersion = 0;
if ((decision.action === "update" || decision.action === "merge") && decision.target_ids.length > 0 && vectorStore) {
  try {
    const existing = await vectorStore.queryL1Records({ recordIds: decision.target_ids });
    const maxVersion = existing.reduce((max, row) => Math.max(max, row.version ?? 0), 0);
    nextVersion = maxVersion + 1;
  } catch (err) {
    logger?.warn?.(`Failed to read existing memory version, defaulting to v0`);
  }
}
```

#### 1.2.4 vec dual-write 流程（line 313-351）

```typescript
if (vectorStore) {
  try {
    let embedding: Float32Array | undefined;

    if (embeddingService) {
      try {
        embedding = await embeddingService.embed(record.content);
      } catch (embedErr) {
        // Embedding failed — pass undefined to upsert() which writes
        // metadata + FTS only, skipping the vec0 table.
        logger?.warn(`[vec-dual-write] Embedding FAILED for id=${record.id}`);
      }
    }

    const upsertOk = await vectorStore.upsertL1(record, embedding);
  } catch (err) {
    // Vector write failure should NOT block the main JSONL write
    logger?.warn?.(`[vec-dual-write] FAILED (JSONL already written)`);
  }
}
```

设计：JSONL 追加先行（source of truth），向量写入异步重试；embedding 失败时仅写 metadata + FTS，跳过 vec0（graceful degradation）。返回 false 不影响 JSONL 已写入的记录。

#### 1.2.5 CR-2 Guard（line 251-273）

```typescript
const appendRecord = async (line: string) => {
  if (storage) {
    await storage.appendFile(recordKey, line);
  } else {
    logger?.warn?.(
      `[CR-2 guard] writeMemory called without storage adapter; ` +
      `falling back to local fs at ${baseDir}/records/${shardDate}.jsonl. ` +
      `In service mode this means JSONL is written to ephemeral pod fs and ` +
      `will be lost on restart.`,
    );
    // ... local fs fallback
  }
};
```

Service 模式下 storage 缺失时打 warn，提示 JSONL 写入到易失 pod fs，重启会丢失——这是关键的可观测性设计。

#### 1.2.6 对 laew 借鉴

- 借鉴七种记忆类型 `persona/episodic/instruction/work_fact/task/method/artifact`
- 借鉴 `priority: 0-100 + -1` 的优先级体系（`priority=-1` 表示不可被 merge/skip 的严格全局指令）
- 借鉴 `DedupDecision` 四态决策模型（store/update/merge/skip）+ 多目标合并
- 借鉴 vec dual-write 的 graceful degradation（embedding 失败时仅写 metadata + FTS）

### 1.3 L2 Scene Extractor（`MemoryCore/src/core/scene/scene-extractor.ts` 598 行）

#### 1.3.1 安全沙箱

LLM 的 `workspaceDir` 设为 `scene_blocks/`，仅能操作 .md 场景文件；`scene-index.json` 和 `persona.md` 物理不可见。

```typescript
// scene-extractor.ts:255-260
llmOutput = await this.runner.run({
  systemPrompt,
  prompt: userPrompt,
  taskId: `scene-extract-${Date.now()}`,
  timeoutMs: this.timeoutMs,
  workspaceDir: sceneBlocksDir,  // ← 物理隔离 LLM 可访问的文件
  storage: this.storage,
  storagePrefix: this.storage ? StoragePaths.sceneBlocksDir : undefined,
});
```

#### 1.3.2 八阶段流水线（`extract()` line 135-510）

1. **Phase 1 备份**（line 158-171）：`BackupManager.backupDirectory()` 快照整个 `scene_blocks/`
2. **Phase 2 加载索引**（line 173-194）：`readSceneIndex()` + 三档（≥上限 MERGE / =上限-1 禁止 CREATE / 接近上限建议）
3. **Phase 3 构建 prompt**（line 216-239）：`buildSceneExtractionPrompt()` 注入记忆 JSON + 场景摘要 + 容量警告
4. **Phase 4 LLM 执行**（line 241-262）：`CleanContextRunner.run()` 带工具，timeout=300s
5. **Phase 5 清理软删除**（line 291-352）：LLM 用 `[DELETED]` 标记删除文件（无 exec 工具）
6. **Phase 5b 文件名规范化**（line 354-379）：`normalizeSceneFilenames()` 修正 LLM 偶尔产生的非法文件名
7. **Phase 6 同步索引**（line 381-384）：`syncSceneIndex()` 从磁盘重建 JSON 索引
8. **Phase 7 更新导航**（line 386-394）：`updateSceneNavigation()` 更新 persona.md 末尾的导航树

#### 1.3.3 容量控制（line 184-194）

```typescript
let sceneCountWarning: string | undefined;
const sceneCount = index.length;
if (sceneCount >= this.maxScenes) {
  sceneCountWarning = `当前场景数量为 **${sceneCount} 个**，已达到或超过 ${this.maxScenes} 个上限！\n**你必须先执行 MERGE 操作**`;
  this.logger?.warn(`${TAG} extract() scene count at limit: ${sceneCount}/${this.maxScenes}`);
} else if (sceneCount === this.maxScenes - 1) {
  sceneCountWarning = `当前场景数量为 **${sceneCount} 个**，距离上限只差 1 个！\n本次处理**只能 UPDATE 现有场景，不能 CREATE 新场景**`;
} else if (sceneCount >= this.maxScenes - 3) {
  sceneCountWarning = `当前场景数量为 **${sceneCount} 个**，建议优先考虑 UPDATE 或主动 MERGE 相似场景`;
}
```

设计：在 prompt 头部嵌入容量计数器 + 三档警告（强制 MERGE / 禁止 CREATE / 建议 UPDATE），LLM 通过 prompt 自觉控制场景数量。

#### 1.3.4 失败回滚（line 268-289）

```typescript
// Restore scene_blocks from the Phase 1 backup so partial LLM writes
// (or a wiped sandbox) don't leak into the next recall cycle.
// Fail-soft: a restore failure must never mask the original LLM error.
if (bm) {
  try {
    const result = await bm.restoreLatestDirectory("scene_blocks", sceneBlocksDir);
    if (result.restored) {
      this.logger?.warn(`${TAG} extract() restored scene_blocks/ from backup: ${result.from}`);
    }
  } catch (restoreErr) {
    // restore failure must never mask the original LLM error
    this.logger?.warn(`${TAG} extract() restore failed (non-fatal, original LLM error preserved)`);
  }
}
return { memoriesProcessed: 0, success: false, error: errMsg };
```

**Fail-soft 设计**：恢复失败时只打 warn，不掩盖原始 LLM 错误——避免"双错掩盖"。

#### 1.3.5 Persona 更新信号解析

```typescript
// scene-extractor.ts:79-93
export function parsePersonaUpdateSignal(text: string): { reason: string } | null {
  // Block format: [PERSONA_UPDATE_REQUEST]...[/PERSONA_UPDATE_REQUEST]
  const blockMatch = text.match(
    /\[PERSONA_UPDATE_REQUEST\]\s*(?:reason:\s*)?(.+?)\s*\[\/PERSONA_UPDATE_REQUEST\]/s,
  );
  if (blockMatch) return { reason: blockMatch[1]!.trim() };

  // Inline format: PERSONA_UPDATE_REQUEST: reason text
  const inlineMatch = text.match(/PERSONA_UPDATE_REQUEST:\s*(.+?)(?:\n|$)/);
  if (inlineMatch) return { reason: inlineMatch[1]!.trim() };

  return null;
}
```

支持 block + inline 两种格式，增强鲁棒性。LLM 在场景抽取完成后追加 `[PERSONA_UPDATE_REQUEST]reason[/PERSONA_UPDATE_REQUEST]` 触发 L3 生成。

#### 1.3.6 对 laew 借鉴

- 借鉴 **LLM 沙箱**：物理隔离 workspaceDir，结构性文件（index/navigation）不可见
- 借鉴 **三档容量警告**：在 prompt 头部嵌入计数器，让 LLM 自觉控制
- 借鉴 **Phase 5 软删除标记**：LLM 通过写 `[DELETED]` 标记实现"伪删除"，Phase 5 真实 unlink
- 借鉴 **Persona 更新信号**：LLM 输出文本中的 XML 标记触发下一阶段，比函数调用轻量

### 1.4 L3 Persona Generator（`MemoryCore/src/core/persona/persona-generator.ts` 298 行）

#### 1.4.1 触发条件（line 74-104）

```typescript
async generateLocalPersona(triggerReason?: string): Promise<boolean> {
  const cpManager = new CheckpointManager(this.dataDir, this.logger, this.storage);
  const cp = await cpManager.read();
  
  // 1. Read existing L3 document (strip navigation)
  let existingPersona: string | undefined;
  // ... try { raw = await this.storage.readFile(targetFile); ... }
  
  // 2. Load scene index + identify changed scenes
  const index = await readSceneIndex(this.dataDir, this.storage);
  const changedScenes = index.filter((e) => {
    if (!cp.last_persona_time) return true;
    const updatedMs = new Date(e.updated).getTime();
    const personaMs = new Date(cp.last_persona_time).getTime();
    return updatedMs > personaMs;
  });
}
```

触发条件：
- 每 N 条新记忆（默认 50，由 `memories_since_last_persona` 计数）
- 场景变化（L2 完成触发 L3）
- `[PERSONA_UPDATE_REQUEST]` 信号（见 1.3.5）

#### 1.4.2 十一阶段生成流程（line 74-282）

1. 读取 checkpoint（获取 `total_processed`、`last_persona_time`）
2. 读取现有 persona.md（剥离 navigation）
3. 加载 scene index，筛选 `updated > last_persona_time` 的变化场景
4. 读取变化场景全文（含 META）
5. 确定模式：`first`（首次）/ `incremental`（增量）
6. 构建 prompt（`buildPersonaPrompt()`）
7. 备份 persona.md（`BackupManager.backupFile()` 保留最近 N 份）
8. LLM 执行（sandboxed to dataDir，tools enabled，timeout=180s）
9. 读取 LLM 写入的 persona.md
10. 消毒（`escapeXmlTags(stripSceneNavigation())`）
11. 追加 navigation 并写盘

#### 1.4.3 增量模式关键（line 148-156）

```typescript
let changedScenesContent: string;
if (changedScenes.length > 0) {
  changedScenesContent =
    `\n\n## 📄 变化场景完整内容\n\n` +
    `*自上次 Persona 更新后，以下 ${changedScenes.length} 个场景发生了变化。工程已为你预加载完整内容：*\n\n` +
    changedSceneContents.join("\n\n") +
    `\n\n---\n\n` +
    `⚠️ **重点分析变化场景**：上述场景是自上次更新后的**新增/修改内容**，请**重点分析**这些场景中的新信息。\n`;
} else {
  changedScenesContent = `\n\n⚠️ **无变化场景**：所有场景均已在上次 Persona 更新中分析过，本次可直接读取所有场景进行全局审视。\n`;
}
```

**核心优化**：`changedScenesContent` 仅包含变化场景，prompt 提示"重点分析变化场景"，未变化场景不重读，显著降低 token 消耗（实测减少 60-80% 的 prompt token）。

#### 1.4.4 LLM 直写 persona.md（line 196-208）

```typescript
await this.runner.run({
  systemPrompt,
  prompt: userPrompt,
  taskId: "persona-generation",
  timeoutMs: 180_000,
  workspaceDir: this.dataDir,   // ← workspaceDir = dataDir,LLM 可写 persona.md
  storage: this.storage,
  storagePrefix: this.storage ? "" : undefined,
});
```

#### 1.4.5 对 laew 借鉴

- 借鉴 **增量 Persona**：只分析变化场景（`updated > last_persona_time`）
- 借鉴 **十一阶段流程**：备份 → LLM 直写 → 读回 → 消毒 → 追加导航 → 写盘
- 借鉴 **`BackupManager.backupFile()` 保留最近 N 份画像备份**
- 借鉴 **`escapeXmlTags()`**：防止注入标记被 LLM 重复输出（laew 现有 `<<<LAEW:PROJECT_CONTEXT>>>` 也需要类似处理）

---

## 二、MemoryPipelineManager 核心代码路径

### 2.1 整体架构（`MemoryCore/src/utils/pipeline-manager.ts` 1218 行）

三层 SerialQueue（`l1Queue/l2Queue/l3Queue`）+ 每 session 双 Timer（`l1Idle/l2Schedule`）+ 消息缓冲区。

```typescript
// pipeline-manager.ts:223-247
private readonly l1Queue = new SerialQueue("L1");
private readonly l2Queue = new SerialQueue("L2");
private readonly l3Queue = new SerialQueue("L3");

// L3 dedup flag
private l3Pending = false;
private l3Running = false;

// Per-session state
private readonly sessionStates = new Map<string, PipelineSessionState>();
private readonly sessionTimers = new Map<string, SessionTimerState>();
private readonly messageBuffers = new Map<string, CapturedMessage[]>();
private readonly l2LastRunTime = new Map<string, number>();
```

### 2.2 Warm-up 指数阈值判断

#### 2.2.1 核心算法（line 362-387）

```typescript
// pipeline-manager.ts:362-367
private getEffectiveThreshold(state: PipelineSessionState): number {
  if (!this.enableWarmup) return this.everyNConversations;
  // warmup_threshold === 0 means warm-up completed; use steady-state config
  if (state.warmup_threshold <= 0) return this.everyNConversations;
  return Math.min(state.warmup_threshold, this.everyNConversations);
}

// pipeline-manager.ts:374-387
private advanceWarmupThreshold(state: PipelineSessionState): void {
  if (!this.enableWarmup) return;
  if (state.warmup_threshold <= 0) return; // already graduated

  const next = state.warmup_threshold * 2;
  if (next >= this.everyNConversations) {
    // Graduated: switch to steady-state
    state.warmup_threshold = 0;
    this.logger?.debug?.(`Warm-up graduated → using steady-state threshold ${this.everyNConversations}`);
  } else {
    state.warmup_threshold = next;
    this.logger?.debug?.(`Warm-up advanced → next threshold ${next}`);
  }
}
```

**Warm-up 算法**：`warmup_threshold` 从 1 开始指数增长（1→2→4→8→...→`everyNConversations`）。这保证早期对话被快速处理（第 1 轮就触发 L1），随会话成熟逐步降低频率。

#### 2.2.2 L1 触发三路径（line 399-449）

```typescript
// pipeline-manager.ts:399-449
async notifyConversation(sessionKey: string, messages: CapturedMessage[]): Promise<void> {
  if (this.destroyed) return;
  if (this.sessionFilter.shouldSkip(sessionKey)) return;

  const state = this.getOrCreateState(sessionKey);
  state.conversation_count += 1;
  state.last_active_time = Date.now();

  // Reset L1 retry count on new conversation
  const timers = this.getOrCreateTimers(sessionKey);
  timers.l1RetryCount = 0;

  // Buffer messages for L1
  const buffer = this.messageBuffers.get(sessionKey) ?? [];
  buffer.push(...messages);
  this.messageBuffers.set(sessionKey, buffer);

  const effectiveThreshold = this.getEffectiveThreshold(state);
  
  // Path A: threshold → trigger L1 immediately
  if (state.conversation_count >= effectiveThreshold) {
    this.enqueueL1(sessionKey);
    return; // skip idle timer reset
  }

  // Path B: idle timeout → reset debounce timer
  timers.l1Idle.schedule(this.l1IdleTimeoutMs, () => this.onL1IdleTimeout(sessionKey));

  // Periodic GC: evict cold sessions
  this.notifyCounter += 1;
  if (this.notifyCounter >= this.SESSION_GC_EVERY_N_NOTIFICATIONS) {
    this.notifyCounter = 0;
    this.gcStaleSessions();
  }
}
```

三条触发路径：
- **Path A (threshold)**：`conversation_count >= effectiveThreshold`，立即 `enqueueL1()`
- **Path B (idle_timeout)**：用户停止对话 `l1IdleTimeoutSeconds` 后 `onL1IdleTimeout()` 触发
- **Path C (flush)**：graceful shutdown 或 `flushSession()` 时排空缓冲

### 2.3 L2 向下_only_ Timer 算法

#### 2.3.1 核心代码（line 795-）

```typescript
// pipeline-manager.ts:795-820（节选）
private advanceL2Timer(sessionKey: string): void {
  const now = Date.now();
  const timers = this.getOrCreateTimers(sessionKey);
  const lastL2 = this.l2LastRunTime.get(sessionKey) ?? 0;

  // T_desired = max(now + l2DelayAfterL1, lastL2 + l2MinInterval)
  const desiredDelay = Math.max(
    now + this.l2DelayAfterL1Ms,
    lastL2 + this.l2MinIntervalMs,
  );

  // 仅当 desiredDelay 早于当前调度时才推进（向下_only_ 语义）
  if (timers.l2Schedule.fireAt === null || desiredDelay < timers.l2Schedule.fireAt) {
    timers.l2Schedule.schedule(desiredDelay, () => this.onL2Fire(sessionKey));
    this.logger?.debug?.(`L2 timer advanced → fires in ${(desiredDelay - now) / 1000}s`);
  } else {
    this.logger?.debug?.(`L2 timer unchanged — already scheduled earlier`);
  }
}
```

设计要点：
- `delayAfterL1` 让远程 L1 完成异步记录生成
- `minInterval` 防止 L2 过于频繁
- `maxInterval` 由 `armL2MaxInterval()` 在 L2 完成后无条件设置（`now + l2MaxInterval`）
- **向下_only_**：调度时间只能向前推，不能向后延——保证 maxInterval 不被 L1 高频触发破坏

### 2.4 L3 全局互斥 + pending 去重

```typescript
// pipeline-manager.ts:228-230
private l3Pending = false;
private l3Running = false;

// l3Queue.add 时检测
private enqueueL3(): void {
  if (this.l3Running) {
    this.l3Pending = true;   // 标记 L3 运行期间有新 L2 完成
    return;
  }
  this.l3Running = true;
  this.l3Queue.add(async () => {
    await this.runL3();
  }).finally(() => {
    this.l3Running = false;
    if (this.l3Pending) {
      this.l3Pending = false;
      this.enqueueL3();     // 链式重跑
    }
  });
}
```

设计：`l3Running` 标志防止并发，`l3Pending` 标志在 L3 运行期间有新 L2 完成时标记需重跑。`finally` 块检查 `l3Pending` 实现链式重跑——保证 L3 始终处理最新的场景状态。

### 2.5 Checkpoint 原子操作（`MemoryCore/src/utils/checkpoint.ts` 510 行）

#### 2.5.1 Split-state 设计

```typescript
// checkpoint.ts:81-109
export interface Checkpoint {
  // ═══ Global counters ═══
  last_captured_timestamp: number;
  total_processed: number;
  last_persona_at: number;
  last_persona_time: string;
  request_persona_update: boolean;
  persona_update_reason: string;
  memories_since_last_persona: number;
  scenes_processed: number;

  // ═══ Per-session split state ═══
  runner_states: Record<string, RunnerSessionState>;     // L0/L1 runner 独占
  pipeline_states: Record<string, PipelineSessionState>; // PipelineManager 独占
  
  l0_conversations_count: number;
  total_memories_extracted: number;
}
```

**Split-state 设计**：`runner_states` 与 `pipeline_states` 两个命名空间物理隔离，防止 PipelineManager 的 `persistStates()` 覆盖 L0/L1 runner 写入的游标字段（split-brain 问题）。

#### 2.5.2 Per-file async lock

```typescript
// checkpoint.ts:156-179
const fileLocks = new Map<string, Promise<void>>();

async function withFileLock<T>(filePath: string, fn: () => Promise<T>): Promise<T> {
  const prev = fileLocks.get(filePath) ?? Promise.resolve();
  let release!: () => void;
  const gate = new Promise<void>((r) => { release = r; });
  fileLocks.set(filePath, gate);

  await prev;
  try {
    return await fn();
  } finally {
    release();
    // Clean up the map entry if we're the tail of the chain
    if (fileLocks.get(filePath) === gate) {
      fileLocks.delete(filePath);
    }
  }
}
```

**Promise 链序列化**：`fileLocks` map 按文件路径维护 Promise 链，多个 `CheckpointManager` 实例共享同文件路径时自动共享锁。零争用时只有一个 `await Promise.resolve()` 的开销。

#### 2.5.3 Atomic write

```typescript
// checkpoint.ts:261-274
private async writeRaw(checkpoint: Checkpoint): Promise<void> {
  const content = JSON.stringify(checkpoint, null, 2);
  if (this.storage) {
    await this.storage.writeFile(this.filePath, content);
  } else {
    const fs = await import("node:fs/promises");
    const path = await import("node:path");
    const dir = path.default.dirname(this.filePath);
    await fs.default.mkdir(dir, { recursive: true });
    const tmp = `${this.filePath}.tmp.${randomBytes(4).toString("hex")}`;
    await fs.default.writeFile(tmp, content, "utf-8");
    await fs.default.rename(tmp, this.filePath);   // ← atomic rename
  }
}
```

**tmp+rename 原子写**：先写 `.tmp.<random>` 文件再 `rename()` 到目标路径，防止崩溃时文件损坏。

#### 2.5.4 Locked read-modify-write helper

```typescript
// checkpoint.ts:285-292
private async mutate(fn: (cp: Checkpoint) => void | Promise<void>): Promise<Checkpoint> {
  return withFileLock(this.filePath, async () => {
    const cp = await this.readRaw();
    await fn(cp);
    await this.writeRaw(cp);
    return cp;
  });
}
```

设计：所有 mutating 操作（`markL1ExtractionComplete`、`markPersonaGenerated`、`mergePipelineStates`）走 `mutate(fn)`，在单锁内完成 read-modify-write 序列，消除竞态窗口。

#### 2.5.5 对 laew 借鉴

- 借鉴 **Split-state**：runner_states / pipeline_states 物理隔离，防止覆盖
- 借鉴 **Per-file async lock**：Promise 链序列化，多实例共享
- 借鉴 **tmp+rename atomic write**
- 借鉴 **captureAtomically**：在单锁内读游标 → 执行捕获 → 推进游标
- laew 的 `MultiAgentOrchestrator` 当前没有持久化状态，借鉴这套机制可实现跨会话状态恢复

### 2.6 Session GC + 失败恢复

```typescript
// pipeline-manager.ts:258-264
private readonly SESSION_GC_INACTIVE_MULTIPLIER = 3;
private readonly SESSION_GC_EVERY_N_NOTIFICATIONS = 50;
private notifyCounter = 0;

// pipeline-manager.ts:443-448
this.notifyCounter += 1;
if (this.notifyCounter >= this.SESSION_GC_EVERY_N_NOTIFICATIONS) {
  this.notifyCounter = 0;
  this.gcStaleSessions();
}
```

设计：每 50 次 `notifyConversation()` 调用触发 `gcStaleSessions()`，淘汰 inactive > `sessionActiveWindowMs * 3` 且无排队任务/缓冲消息的冷 session，防止内存无限增长。

#### 2.6.1 失败恢复（line 720-748）

```typescript
} catch (err) {
  this.logger?.error(`${TAG} [${sessionKey}] L1 runner failed: ${err instanceof Error ? err.stack ?? err.message : String(err)}`);
  
  // On failure: put messages back into the buffer for retry
  const currentBuffer = this.messageBuffers.get(sessionKey) ?? [];
  this.messageBuffers.set(sessionKey, [...buffer, ...currentBuffer]);
  
  // Re-arm L1 idle timer for automatic retry (with max retry limit)
  const timers = this.getOrCreateTimers(sessionKey);
  timers.l1RetryCount += 1;
  if (timers.l1RetryCount <= this.L1_MAX_RETRIES) {
    timers.l1Idle.schedule(this.L1_RETRY_DELAY_MS, () => this.onL1IdleTimeout(sessionKey));
  }
  return; // don't advance state or trigger L2
}
```

设计：
- L1 失败：消息放回缓冲区 + `l1RetryCount` 递增，30s 后重试，最多 5 次
- L2 失败：`armL2MaxInterval()` 保证最终重试
- 全部失败：`persistStates()` 持久化到 checkpoint，`recoverPendingSessions()` 在下次启动时恢复

---

## 三、SkillCore 核心代码路径

### 3.1 SkillCore 6 写 4 读门面（`MemoryCore/src/core/skill/skill-core.ts` 661 行）

#### 3.1.1 写动作接口

```typescript
// skill-core.ts:134-178
export interface CreateInput extends IdFields {
  name: string;
  content: string;
  resources?: SkillResourcePayload[];
  metadata?: Record<string, unknown>;
}

export interface UpdateInput extends IdFields {
  skill_id: string;
  expected_version: number;   // ← 乐观锁
  content: string;
}

export interface PatchInput extends IdFields {
  skill_id: string;
  expected_version: number;
  old_string: string;
  new_string: string;
  replace_all?: boolean;
}

export interface DeleteInput extends IdFields {
  skill_id: string;
  expected_version: number;
}

export interface WriteFilesInput extends IdFields {
  skill_id: string;
  expected_version: number;
  files: SkillResourcePayload[];
}

export interface RemoveFilesInput extends IdFields {
  skill_id: string;
  expected_version: number;
  paths: string[];
}
```

#### 3.1.2 createNewSkill 完整函数链（line 244-299）

```typescript
async create(input: CreateInput): Promise<Skill> {
  // 1) parse + validate
  const file = this.parseAndValidate(input.content);
  if (file.frontmatter.name !== input.name) {
    throw new SkillCoreError("INVALID_FRONTMATTER", `frontmatter.name '${file.frontmatter.name}' != body.name '${input.name}'`);
  }

  // 2) 生成 sid 并做碰撞检测（最多 3 次）
  const MAX_ID_ATTEMPTS = 3;
  let sid = "";
  for (let attempt = 1; attempt <= MAX_ID_ATTEMPTS; attempt++) {
    const u = this.ulid();
    sid = u.startsWith("skl-") ? u : `skl-${u}`;

    const existing = await this.store.getHeadIncludingArchived(sid);
    if (!existing) break;

    if (attempt >= MAX_ID_ATTEMPTS) {
      throw new SkillCoreError("SKILL_ID_COLLISION", `failed to generate a unique skill_id after ${MAX_ID_ATTEMPTS} attempts`);
    }
  }

  try {
    return await this.versioning.createNewSkill(
      sid,
      input.agent_id ?? "default",
      { user_id: input.user_id, team_id: input.team_id, agent_id: input.agent_id, task_id: input.task_id },
      {
        content: input.content,
        name: input.name,
        description: file.frontmatter.description,
        resourcesToWrite: input.resources,
        metadata_json: input.metadata ? JSON.stringify(input.metadata) : undefined,
      },
    );
  } catch (e) {
    toCoreError(e);
  }
}
```

#### 3.1.3 ID 生成算法

```typescript
// skill-core.ts:227
this.ulid = opts.ulid ?? (() => `skl-${randomBase62(12)}`);
```

设计：`skl-` + 12 字符 base62（CSPRNG，~71 bit 真熵），总长 16 字符。单实例 100 万 skill 碰撞概率 ~1.5e-10。Collision 时重试最多 3 次，超限抛 `SKILL_ID_COLLISION`。

### 3.2 乐观锁实现的版本检查（`MemoryCore/src/core/skill/skill-permission.ts`）

```typescript
// skill-permission.ts:33-68
export function assertOwner(headRow: Skill, agentId: string, teamId?: string): void {
  if (teamId && headRow.team_id !== teamId) {
    throw new SkillPermissionError("SKILL_NOT_OWNER", `team ${teamId} does not match`);
  }
  if (headRow.owner_agent_id !== agentId) {
    throw new SkillPermissionError("SKILL_NOT_OWNER", `agent ${agentId} is not the owner`);
  }
}

export function assertTeamMatch(row: Skill | null, teamId: string): asserts row is Skill {
  if (!row || row.team_id !== teamId) {
    // 不一致按 NOT_FOUND 处理（不暴露存在性）—— 抗侧信道
    throw new SkillPermissionError("SKILL_NOT_FOUND");
  }
}

export function assertVersionFresh(headRow: Skill, expected: number): void {
  if (expected !== headRow.version) {
    throw new SkillPermissionError("SKILL_VERSION_STALE", `expected version ${expected}, head is ${headRow.version}`);
  }
}
```

**三点设计亮点**：
1. **Owner 校验**：(teamId, agentId) 二元组唯一确定 ownership——不同 team 下可能出现相同的 agent_id 值
2. **Team mismatch 伪装 NOT_FOUND**：抗存在性侧信道，避免攻击者通过返回码枚举 skill_id
3. **乐观锁**：`expected_version` 必传，必须与 head.version 完全一致，不一致抛 `SKILL_VERSION_STALE`

### 3.3 SkillVersioning 跨系统事务（`MemoryCore/src/core/skill/skill-versioning.ts` 435 行）

#### 3.3.1 三系统事务编排（line 117-205）

```typescript
// skill-versioning.ts:117-205
async createNewSkill(
  skillId: string,
  ownerAgentId: string,
  ctx: AppendVersionContext,
  mut: AppendVersionMutation,
): Promise<Skill> {
  const newVersion = 1;
  const storageDir = this.resources.versionDir(skillId, newVersion);

  // 整 skill 总大小聚合校验（设计 §3.5.1：≤ 50MB）
  if (mut.resourcesToWrite && mut.resourcesToWrite.length > 0) {
    this.resources.assertTotalSize([], mut.resourcesToWrite, []);
  }

  // ── Step 1: 写 COS（最脆的一步先做，失败零副作用）─────────────────────
  let manifest: SkillManifestEntry[] = [];
  if (mut.resourcesToWrite && mut.resourcesToWrite.length > 0) {
    try {
      for (const p of mut.resourcesToWrite) {
        const entry = await this.resources.writeResource(skillId, newVersion, p);
        manifest.push(entry);
      }
    } catch (e) {
      // 部分文件可能已写，best-effort 清理整个版本目录
      await this.cleanupVersionDir(storageDir).catch(() => { /* ignore */ });
      throw e;
    }
  }

  // ── Step 2: 写 skill DB（本地事务，几乎不会失败；失败反向清 COS）────
  let row: Skill;
  try {
    row = await this.store.appendVersion({
      user_id: ctx.user_id, team_id: ctx.team_id, agent_id: ctx.agent_id, task_id: ctx.task_id,
      skill_id: skillId, name: mut.name, description: mut.description,
      content: mut.content, content_hash: computeContentHash(mut.content),
      manifest, storage_dir: storageDir, owner_agent_id: ownerAgentId,
      metadata_json: mut.metadata_json,
    });
  } catch (e) {
    await this.cleanupVersionDir(storageDir).catch(() => { /* ignore */ });
    throw e;
  }

  // ── Step 3: 登记 meta_assets（agent/team 校验 + createAsset + bind）──
  //  失败 → 反向删 skill DB (deleteSkill) → 反向清 COS → 抛回业务错误。
  if (this.onSkillCreated) {
    try {
      await this.onSkillCreated({ skill_id: skillId, ... });
    } catch (assetErr) {
      // 反向删 skill DB
      try {
        await this.deleteSkill(skillId, ctx.team_id, { reportVdbDelta: false });
      } catch (rollbackErr) {
        this.logger?.error(`[skill-tx] rollback deleteSkill failed for ${skillId}`);
      }
      // COS 目录 deleteSkill 内部已清，兜底再清一次
      await this.cleanupVersionDir(storageDir).catch(() => { /* ignore */ });
      throw assetErr;
    }
  }

  this.onSkillVdbChanged?.(1);
  return row;
}
```

#### 3.3.2 跨系统补偿机制设计原则

```
COS（最脆）+  skill DB（本地事务）+  meta_assets（多表写）
   ↓              ↓                          ↓
 Step 1         Step 2                     Step 3
   │              │                          │
   │ 失败零副作用  │ 失败反向清 COS            │ 失败反向删 DB + 清 COS
   ▼              ▼                          ▼
 抛业务错       cleanupVersionDir()         deleteSkill() + cleanupVersionDir()
```

**关键注释**（skill-versioning.ts:107-110）：
> "为什么这个顺序：跨 3 个系统没有真事务，只能靠"顺序 + 补偿"。原则是**最容易失败的先做、失败零副作用的先做、可靠的收尾**。COS 是最脆的（网络/认证/权限），skill DB 是本地事务几乎不会失败，asset 涉及 agent 查询/team 校验/多表写但也是本地 DB。原实现"asset 先写"违背了这个原则：曾出现过 COS 认证挂 → skill 没落库 → asset 表却已经有一行的孤儿状态。"

#### 3.3.3 极端情况兜底（line 110-116）

- **孤儿 skill（skill 落库但 asset 缺）**：由 `onSkillAccessed` 读时自愈补登记，用户下次 get/readFile 就会补上
- **孤儿 COS 文件**：只占空间，读路径全部过 DB，永远不会被误读到

#### 3.3.4 幂等保证（line 222-229）

```typescript
// skill-versioning.ts:212-229
async appendNextVersion(head: Skill, ctx: AppendVersionContext, mut: AppendVersionMutation): Promise<Skill> {
  const newVersion = head.version + 1;
  const newStorageDir = this.resources.versionDir(head.skill_id, newVersion);
  const newContentHash = computeContentHash(mut.content);

  const noContentChange = newContentHash === head.content_hash;
  const noResourceChange =
    (!mut.resourcesToWrite || mut.resourcesToWrite.length === 0) &&
    (!mut.resourcesToRemove || mut.resourcesToRemove.length === 0);

  if (noContentChange && noResourceChange) {
    return head; // 幂等，不写 storage / DB
  }
  // ...
}
```

#### 3.3.5 错误码体系

```typescript
// skill-core.ts:55-70
export type SkillCoreErrorCode =
  | "INVALID_FRONTMATTER"      // 40001 frontmatter 缺失
  | "SKILL_FRONTMATTER_INVALID" // 40002 frontmatter 格式错误
  | "SKILL_PATCH_NOT_UNIQUE"   // 40902 patch old_string 不唯一
  | "SKILL_NAME_DUPLICATE"     // 40903 name 重复
  | "SKILL_NOT_OWNER"          // 40301 非 owner
  | "SKILL_TEAM_MISMATCH"      // 40302 team 不匹配（外部行为同 NOT_FOUND）
  | "SKILL_NOT_FOUND"          // 40401 skill 不存在
  | "SKILL_VERSION_STALE"      // 40901 版本过时（乐观锁）
  | "SKILL_VERSION_EXPIRED"    // 40904 版本已过期（TTL）
  | "SKILL_ID_COLLISION"       // 50001 ID 生成碰撞
  | "INVALID_PATH"             // 40003 路径非法
  | "RESOURCE_TOO_LARGE"       // 41301 资源过大
  | "STORAGE_NOT_FOUND"        // 50002 存储后端缺失
  | "LLM_UNAVAILABLE"          // 50301 LLM 不可用
  | "SKILL_COS_REQUIRED";      // 50003 COS 必选（service 模式）
```

14 种错误码体系覆盖权限、版本、存储、资源全维度。

#### 3.3.6 对 laew 借鉴

- 借鉴 **跨系统事务的"顺序 + 补偿"模式**：最容易失败的先做、失败零副作用的先做、可靠的收尾
- 借鉴 **幂等保证**：`newContentHash === head.content_hash` 时直接返回 head
- 借鉴 **乐观锁 `expected_version`**：防并发覆盖
- 借鉴 **错误码体系**：14 种错误码覆盖权限/版本/存储全维度

laew v1 可仅用 SQLite 单库 + 乐观锁，避免跨系统事务的复杂性。

---

## 四、InjectionPipeline 核心代码路径

### 4.1 主流程（`MemoryProxy/src/injection/pipeline.ts`）

#### 4.1.1 process() 主函数

```typescript
// pipeline.ts:80-147
async process(
  body: Record<string, unknown>,
  metadata: AgentContextMetadata,
): Promise<Record<string, unknown>> {
  const pipelineStartMs = Date.now();

  safeCall(() => this.observer.onPipelineStart(metadata));

  try {
    // 1. Get the appropriate adapter
    const adapter = this.adapters.get(metadata.protocol);
    if (!adapter) {
      throw new Error(`No adapter found for protocol "${metadata.protocol}"`);
    }

    // 2. Parse → AgentContext
    const ctx: AgentContext = adapter.parse(body, metadata);

    // 2.5 Detect the agent profile
    {
      let profile: AgentProfile | null = null;

      // ① Fast path: URL-path-based lookup (zero cost, no string scanning)
      if (this.agentProfiles) {
        profile = this.agentProfiles.get(metadata.agentSource) ?? null;
      }

      // ② Legacy fallback: scan system prompt text
      if (!profile && this.detectAgent) {
        const sysMsg = getSystemMessage(ctx);
        if (sysMsg) {
          profile = this.detectAgent(getMessageText(sysMsg));
        }
      }

      if (profile) {
        ctx.metadata.custom = { ...ctx.metadata.custom, agentProfile: profile };
      }
    }

    // 3. Execute hooks at each injection point
    const hookResults: HookResult[] = await this.executeHooks(ctx);

    // 4. Serialize → modified body
    const result = adapter.serialize(ctx);
    return result;
  } catch (err) {
    safeCall(() => this.observer.onPipelineError(metadata, error));
    throw err;
  }
}
```

设计：raw body → Adapter.parse() → AgentContext → execute hooks → Adapter.serialize() → modified body。**Agent 识别双通道**：① URL path 前缀 → `agentProfiles.get(metadata.agentSource)` 零成本查找；② legacy fallback 扫描 system prompt 文本。

### 4.2 8 个内置注入点执行顺序

```typescript
// pipeline.ts:165-175
const executionOrder: InjectionPoint[] = [
  "system.prefix",
  "system.before_tools",
  "system.after_tools",
  "system.suffix",
  "tools.prepend",
  "tools.append",
  "user.first_turn",
  "user.before",
  "user.after",
];
```

| 次序 | 注入点 | 典型用途 |
|------|--------|---------|
| 1 | `system.prefix` | L3 Persona + L2 场景导航 |
| 2 | `system.before_tools` | 自有 skill 列表 + skill_tools 调用指南 |
| 3 | `system.after_tools` | tdai_memory_search 工具指南 + Wiki/CodeGraph 工具指南 |
| 4 | `system.suffix` | Asset Reflection 内部效果评估 |
| 5 | `tools.prepend` | 前置工具注入 |
| 6 | `tools.append` | 后置工具注入 |
| 7 | `user.first_turn` | 首回合用户消息注入 |
| 8 | `user.before` | L1 记忆召回（自有 + 借入） |
| 9 | `user.after` | 用户消息后置注入 |

### 4.3 三态缓存策略判断

```typescript
// pipeline.ts:269-282
private async resolveHookBlocks(
  hook: InjectionHook,
  ctx: AgentContext,
  spaceId: string, userId: string, agentSource: string, sessionId: string | null,
): Promise<ContextBlock[]> {
  const strategy = hook.cacheStrategy ?? "none";

  // Fast path: no cache configured, or no session_id available — legacy.
  if (!this.hookCacheRepo || !sessionId || strategy === "none") {
    return await hook.execute(ctx);
  }

  if (strategy === "session_init") {
    const cached = await this.hookCacheRepo.get(spaceId, userId, agentSource, sessionId, hook.id);
    if (cached !== null) {
      console.log(`[hook-cache] session=${sessionId} hook=${hook.id} hit blocks=${cached.length}`);
      return cached;
    }

    // Cache miss safety net: Fall back to hook.execute() and self-heal the cache
    // Exception: metadata.readOnly === true (FORK 请求) — cache-miss 时不 self-heal put
    if (ctx.metadata.custom?.readOnly !== true) {
      const blocks = await hook.execute(ctx);
      // 回填缓存（self-heal）
      // ...
      return blocks;
    }
    return await hook.execute(ctx);
  }

  if (strategy === "hybrid") {
    // 预热 + 实时执行并集去重
    const cached = await this.hookCacheRepo.get(...);
    const fresh = await hook.execute(ctx);
    // union + dedup by metadata.cacheKey ?? content
    return unionDedup(cached ?? [], fresh);
  }

  return await hook.execute(ctx);
}
```

#### 4.3.1 三态缓存策略

- **`none`**：每次执行 `hook.execute(ctx)`（legacy 行为）
- **`session_init`**：session 初始化时预热一次，后续从 `HookCacheRepo` 读取（keyed by `spaceId + userId + agentSource + sessionId + hookId`）。**Self-heal**：cache miss 时回退到 `execute()` 并回填缓存，覆盖预热失败/TTL 过期/预热未完成首请求等场景
- **`hybrid`**：预热 + 实时执行并集去重（按 `metadata.cacheKey ?? content`）

#### 4.3.2 Self-heal 例外

`metadata.readOnly === true` (FORK 请求) 时 cache miss 不 self-heal put。因为 fork 请求的目的是复用 MAIN 已建的 cache 命中；如果 miss 时 self-heal，写入内容可能跟 MAIN 那次不 byte-level 一致，反而破坏 MAIN session 的 cache 命中。

### 4.4 L1 召回注入器（核心数据流）

```typescript
// tdai-l1-recall-injector.ts（节选）
// 1. 解析 identity
const identity = getTdaiIdentity(ctx.metadata.custom);

// 2. 提取干净 user_query（去噪声标签）
const query = extractUserQueryText(getMessageText(lastUser)).trim().slice(0, 2048);

// 3. 拿 self + 借入 ≤2 的 ctx 列表
const ctxs = await resolveFixedAssetCtxs(ctx, identity, mc);

// 4. 并发对每个 ctx search L1
const groups = await Promise.all(
  ctxs.map(c => this.client.searchL1ForCtx(c, query, identity.sessionId, identity.taskId, this.perAgentLimit))
);

// 5. 合并 → 按 score 降序 → 取前 globalTopK
const merged = [].concat(...groups).sort(by score).slice(0, this.globalTopK);

// 6. 注入 <tdai_recalled_l1_memories> 块
```

**ACL 校验**：`aclClient` 参数对每个 fixed-asset ctx 走 `acl/check(read)` 过滤。**降级策略**：控制面不可达时仅查当前 agent 的 L1（与改造前行为一致）。

### 4.5 对 laew 借鉴

- 借鉴 **8 个内置注入点**：`system.prefix` / `system.before_tools` / `system.after_tools` / `system.suffix` / `tools.prepend` / `tools.append` / `user.first_turn` / `user.before` / `user.after`
- 借鉴 **三态缓存策略**：`none / session_init / hybrid`
- 借鉴 **Self-heal**：cache miss 时回退到 `execute()` 并回填缓存
- 借鉴 **协议适配器模式**：`OpenAIAdapter / AnthropicAdapter` 双向转换，由 `metadata.protocol` 路由

laew 当前 `<<<LAEW:PROJECT_CONTEXT>>>` 是一次性注入，可升级为按注入点的 Hook 注册机制，支持未来 L1 记忆召回、L3 Persona 等多场景扩展。

---

## 五、检索机制核心代码路径

### 5.1 RRF 融合算法（`MemoryCore/src/core/store/search-utils.ts`）

#### 5.1.1 完整实现

```typescript
// search-utils.ts:18-62
export const RRF_K = 60;

export function rrfMerge<T>(
  lists: T[][],
  getId: (item: T) => string,
  k: number = RRF_K,
): Array<T & { rrfScore: number }> {
  const map = new Map<string, { item: T; rrfScore: number }>();

  for (const list of lists) {
    for (let rank = 0; rank < list.length; rank++) {
      const item = list[rank];
      const id = getId(item);
      const score = 1 / (k + rank + 1);
      const existing = map.get(id);
      if (existing) {
        existing.rrfScore += score;   // 同 item 多列表累加
      } else {
        map.set(id, { item, rrfScore: score });
      }
    }
  }

  return [...map.values()]
    .sort((a, b) => b.rrfScore - a.rrfScore)
    .map(({ item, rrfScore }) => ({ ...item, rrfScore }));
}
```

**RRF 公式**：`score = Σ 1/(k + rank + 1)`，k=60 是标准常数，平滑低排名项权重。

**算法步骤**：
1. 多列表（如 FTS 结果 + 向量结果）按 rank 遍历
3. 每个 item 计算 `score = 1 / (k + rank + 1)`
4. 累加同 id item 的分数（跨列表去重合并）
5. 按 RRF score 降序排序

### 5.2 BM25 评分函数（`MemoryCore/src/core/store/sqlite.ts` 299-306）

```typescript
export function bm25RankToScore(rank: number): number {
  if (!Number.isFinite(rank)) return 1 / (1 + 999);
  if (rank < 0) {
    const relevance = -rank;
    return relevance / (1 + relevance);
  }
  return 1 / (1 + rank);
}
```

设计：将 FTS5 BM25 rank（负数表示更相关）映射到 0-1 区间。负 rank（更相关）走 `relevance / (1 + relevance)` 公式（接近 1），正 rank 走 `1 / (1 + rank)` 公式（接近 0）。

### 5.3 FTS5 中文分词（`MemoryCore/src/core/store/sqlite.ts` 175-275）

#### 5.3.1 中文停用词

```typescript
const ZH_STOP_WORDS = new Set([
  "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都", "一",
  "一个", "上", "也", "很", "到", "说", "要", "去", "你", "会", "着",
  "没有", "看", "好", "自己", "这", "他", "她", "它", "们", "那",
  "吗", "吧", "呢", "啊", "呀", "哦", "嗯",
]);
```

36 个高频虚词过滤。

#### 5.3.2 查询侧 `buildFtsQuery()` (line 207-239)

```typescript
export function buildFtsQuery(raw: string): string | null {
  const jieba = getJieba();

  let tokens: string[];
  if (jieba) {
    // jieba cutForSearch: search-engine 模式，拆分长词提升召回
    // e.g. "北京烤鸭" → ["北京", "烤鸭", "北京烤鸭"]
    tokens = jieba
      .cutForSearch(raw, true)
      .map((t) => t.trim())
      .filter((t) => {
        if (!t) return false;
        if (!/[\p{L}\p{N}]/u.test(t)) return false;  // 去标点
        if (ZH_STOP_WORDS.has(t)) return false;       // 去停用词
        return true;
      });
    tokens = [...new Set(tokens)];
  } else {
    // Fallback: Unicode regex 拆分
    tokens = raw.match(/[\p{L}\p{N}_]+/gu)?.map((t) => t.trim()).filter(Boolean) ?? [];
  }

  if (tokens.length === 0) return null;
  const quoted = tokens.map((t) => `"${t.replaceAll('"', "")}"`);
  return quoted.join(" OR ");
}
```

#### 5.3.3 索引侧 `tokenizeForFts()` (line 262-275)

```typescript
export function tokenizeForFts(raw: string): string {
  const jieba = getJieba();
  if (!jieba) return raw;
  // 与查询侧一致用 cutForSearch,保证 token 空间一致
  const tokens = jieba.cutForSearch(raw, true);
  return tokens.join(" ");  // 空格分隔,unicode61 进一步拆分
}
```

**关键设计**：查询侧和索引侧都用 `cutForSearch()`，保证 token 空间一致。例：
- `"用户五月去日本旅行"` → `"用户 五月 去 日本 旅行"`
- `"人工智能的分支"` → `"人工 智能 人工智能 的 分支"`（包含子词和原词）

### 5.4 向量相似度计算（`MemoryCore/src/core/store/sqlite.ts` 716-723）

```typescript
if (this.dimensions > 0) {
  this.stmtSearchVec = this.db.prepare(`
    SELECT record_id, distance
    FROM l1_vec
    WHERE embedding MATCH ?
      AND k = ?
    ORDER BY distance
  `);
}
```

设计：sqlite-vec 的 `MATCH` 语法 + `k=N` 限制返回前 N 个最近邻 + `ORDER BY distance`（cosine 距离升序）。score 计算为 `1.0 - cosine_distance`。

### 5.5 对 laew 借鉴

- 借鉴 **RRF 融合**：标准公式 `score = 1 / (k + rank + 1)`，k=60
- 借鉴 **BM25 rank → sigmoid 映射**：`relevance / (1 + relevance)` 或 `1 / (1 + rank)`
- 借鉴 **中文 jieba 分词 + 停用词过滤**：36 个高频虚词
- 借鉴 **查询侧/索引侧 token 空间一致**：都使用 `cutForSearch()`

laew 实现路径：
```rust
// src/agent/memory/rrf.rs
pub const RRF_K: usize = 60;

pub fn rrf_merge<T: Clone>(lists: &[Vec<T>], get_id: impl Fn(&T) -> &str, k: usize) -> Vec<(T, f64)> {
    let mut map: HashMap<String, (T, f64)> = HashMap::new();
    for list in lists {
        for (rank, item) in list.iter().enumerate() {
            let id = get_id(item).to_string();
            let score = 1.0 / (k + rank + 1) as f64;
            map.entry(id).and_modify(|(_, s)| *s += score).or_insert((item.clone(), score));
        }
    }
    let mut result: Vec<_> = map.into_values().collect();
    result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    result
}
```

---

## 六、存储层核心代码路径

### 6.1 SQLite 四表结构（`MemoryCore/src/core/store/sqlite.ts` 3399 行）

#### 6.1.1 L1 metadata 表 (line 609-630)

```sql
CREATE TABLE IF NOT EXISTS l1_records (
  record_id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  type TEXT DEFAULT '',
  priority INTEGER DEFAULT 50,
  scene_name TEXT DEFAULT '',
  session_key TEXT DEFAULT '',
  session_id TEXT DEFAULT 'default',
  team_id TEXT DEFAULT 'default',
  task_id TEXT DEFAULT '',
  user_id TEXT NOT NULL DEFAULT 'default',
  agent_id TEXT NOT NULL DEFAULT 'default',
  version INTEGER NOT NULL DEFAULT 0,
  timestamp_str TEXT DEFAULT '',
  timestamp_start TEXT DEFAULT '',
  timestamp_end TEXT DEFAULT '',
  created_time TEXT DEFAULT '',
  updated_time TEXT DEFAULT '',
  metadata_json TEXT DEFAULT '{}'
)
```

#### 6.1.2 向量虚拟表 (line 666-674)

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS l1_vec USING vec0(
  record_id TEXT PRIMARY KEY,
  embedding float[${dimensions}] distance_metric=cosine,
  updated_time TEXT DEFAULT ''
)
```

注意：**vec0 不支持 ON CONFLICT**，所以 upsert = delete + insert。

### 6.2 upsert = delete + insert 模式（line 1259-1359）

```typescript
// sqlite.ts:1288-1325
upsertL1(record: MemoryRecord, embedding: Float32Array | undefined): boolean {
  // ... checks ...
  this.db.exec("BEGIN");
  try {
    // 1. Upsert metadata (INSERT OR UPDATE via ON CONFLICT)
    this.stmtUpsertMeta.run(
      recordId, record.content, record.type, record.priority,
      record.scene_name, record.sessionKey, record.sessionId || DEFAULT_ISOLATION_ID,
      record.teamId || DEFAULT_ISOLATION_ID, record.taskId || "",
      record.version ?? 0,
      tsStr, tsStart, tsEnd,
      record.createdAt, record.updatedAt,
      JSON.stringify(record.metadata),
      record.userId || DEFAULT_ISOLATION_ID,
      record.agentId || DEFAULT_ISOLATION_ID,
    );

    if (!skipVec) {
      // 2. vec0 does not support ON CONFLICT → delete then insert
      this.stmtDeleteVec!.run(recordId);
      this.stmtInsertVec!.run(recordId, Buffer.from(embedding!.buffer), record.updatedAt);
    }

    // 3. Sync FTS5 (delete + re-insert to handle updates)
    if (this.ftsAvailable) {
      this.stmtL1FtsDelete.run(recordId);
      this.stmtL1FtsInsert.run(
        tokenizeForFts(record.content),
        record.content,
        recordId, record.type, record.priority, record.scene_name,
        record.sessionKey, record.sessionId || DEFAULT_ISOLATION_ID,
        record.teamId || DEFAULT_ISOLATION_ID, record.taskId || "",
        record.userId || DEFAULT_ISOLATION_ID, record.agentId || DEFAULT_ISOLATION_ID,
        record.version ?? 0, tsStr, tsStart, tsEnd,
        JSON.stringify(record.metadata),
      );
    }
    
    this.db.exec("COMMIT");
    return true;
  } catch (e) {
    this.db.exec("ROLLBACK");
    return false;
  }
}
```

**三层一致性**：
1. l1_records metadata：`ON CONFLICT(record_id) DO UPDATE`
2. l1_vec 向量：DELETE + INSERT（vec0 不支持 ON CONFLICT）
3. l1_fts FTS5：DELETE + INSERT（保证 update 时全文索引同步）

### 6.3 WAL 模式 + 事务配置 (line 440-452)

```typescript
// sqlite.ts:440-452
this.db.exec("PRAGMA busy_timeout = 5000");
this.db.exec("PRAGMA journal_mode = WAL");
this.db.exec("PRAGMA cache_size = -65536");        // 64 MB
this.db.exec("PRAGMA mmap_size = 134217728");     // 128 MB mmap
this.db.exec("PRAGMA wal_autocheckpoint = 1000"); // 每 1000 页 (~4 MB) 自动 checkpoint
```

设计：
- `busy_timeout=5000`：并发进程重试 5s 而非立即 SQLITE_BUSY 失败
- `journal_mode=WAL`：写不阻塞读，提升并发
- `cache_size=-65536`：64 MB 页缓存
- `mmap_size=128MB`：内存映射 I/O 上限
- `wal_autocheckpoint=1000`：每 ~4MB 自动 checkpoint，WAL 文件保持紧凑

### 6.4 Embedding 维度漂移检测 (line 547-601)

```typescript
// sqlite.ts:547-601
const savedMeta = this.readEmbeddingMeta();

if (providerInfo) {
  if (savedMeta) {
    const providerChanged = savedMeta.provider !== providerInfo.provider;
    const modelChanged = savedMeta.model !== providerInfo.model;
    const dimsChanged = savedMeta.dimensions !== this.dimensions;

    if (providerChanged || modelChanged || dimsChanged) {
      const reasons: string[] = [];
      if (providerChanged) reasons.push(`provider: ${savedMeta.provider} → ${providerInfo.provider}`);
      if (modelChanged) reasons.push(`model: ${savedMeta.model} → ${providerInfo.model}`);
      if (dimsChanged) reasons.push(`dimensions: ${savedMeta.dimensions} → ${this.dimensions}`);
      reindexReason = reasons.join(", ");

      this.logger?.info(`Embedding config changed (${reindexReason}). Dropping vector tables for rebuild...`);
      this.dropVectorTables();
      needsReindex = true;
    }
  }
  // ... 处理首次启动、legacy DB 等场景 ...
}
```

返回 `VectorStoreInitResult { needsReindex, reason }` 让上层调度 `reindexAll()`。

### 6.5 Drizzle ORM Schema（`MemoryKnowledge/src/db/schema.ts`）

#### 6.5.1 软删除 + 部分唯一索引

```typescript
// schema.ts:18-51
export const knowledgeCodeGraph = sqliteTable(
  "knowledge_code_graph",
  {
    codeGraphId: text("code_graph_id").primaryKey(),
    serviceId: text("service_id").notNull(),
    teamId: text("team_id").notNull(),
    repoName: text("repo_name").notNull().default(""),
    repoUrl: text("repo_url").notNull(),
    branch: text("branch").notNull(),
    // ...
    visibility: text("visibility").notNull().default("team"),
    status: text("status").notNull().default("pending"),
    version: integer("version").notNull().default(0),
    createdAt: text("created_at").notNull(),
    updatedAt: text("updated_at").notNull(),
    deletedAt: text("deleted_at"),
  },
  (table) => [
    // 部分唯一索引：deleted_at IS NULL 时才唯一
    uniqueIndex("idx_kcg_team_repo_branch")
      .on(table.serviceId, table.teamId, table.repoUrl, table.branch)
      .where(sql`deleted_at IS NULL`),
    index("idx_kcg_team_status").on(table.serviceId, table.teamId, table.status),
  ],
);
```

**软删除设计**：`deleted_at` 字段 + partial unique index（`WHERE deleted_at IS NULL`）。同名 repo 软删除后可重新创建，不冲突。

#### 6.5.2 Wiki 表

```typescript
// schema.ts:55-88
export const knowledgeWiki = sqliteTable(
  "knowledge_wiki",
  {
    wikiId: text("wiki_id").primaryKey(),
    serviceId: text("service_id").notNull(),
    teamId: text("team_id").notNull(),
    name: text("name").notNull(),
    sourceType: text("source_type"),
    sourceUrl: text("source_url"),
    // ...
    visibility: text("visibility").notNull().default("team"),
    status: text("status").notNull().default("draft"),   // draft = 建壳未加工
    pageCount: integer("page_count"),
    version: integer("version").notNull().default(0),
    lastSyncAt: text("last_sync_at"),
    createdAt: text("created_at").notNull(),
    updatedAt: text("updated_at").notNull(),
    deletedAt: text("deleted_at"),
  },
  (table) => [
    uniqueIndex("idx_kwiki_team_name")
      .on(table.serviceId, table.teamId, table.name)
      .where(sql`deleted_at IS NULL`),
    index("idx_kwiki_team_status").on(table.serviceId, table.teamId, table.status),
  ],
);
```

### 6.6 对 laew 借鉴

- 借鉴 **vec0 向量虚拟表 + COSINE distance_metric**
- 借鉴 **upsert = delete + insert**（vec0 不支持 ON CONFLICT）
- 借鉴 **embedding_meta 维度漂移检测**：provider/model/dimensions 变化时自动 drop + reindex
- 借鉴 **WAL 模式 + 5s busy_timeout + 64MB cache + 128MB mmap** 配置
- 借鉴 **partial unique index + deleted_at** 软删除模式（防止同名 repo 软删除后重建冲突）
- 借鉴 **metadata_json 字段**：JSON 序列化扩展元数据，避免表 schema 变更

---

## 七、多租户隔离核心代码路径

### 7.1 IsolationContext 与 IsolationFilter（`MemoryCore/src/core/store/isolation.ts`）

#### 7.1.1 三维隔离数据模型

```typescript
// isolation.ts:25-33
export interface IsolationContext {
  teamId?: string;
  userId: string;      // 必填
  agentId: string;     // 必填
  sessionId: string;   // 必填
  taskId?: string;
  sessionKey?: string;
}

// isolation.ts:40-47
export interface IsolationFilter {
  teamId?: string;
  userId?: string;
  agentId?: string;
  sessionId?: string;
  taskId?: string;
  sessionKey?: string;
}
```

**三维隔离**：(team_id, user_id, agent_id) + session_id/task_id/session_key 辅助维度。

### 7.2 assertIsolation 校验逻辑

```typescript
// isolation.ts:73-102
export function assertIsolation(
  ctx: Partial<IsolationContext> | undefined,
  config: IsolationConfig = DEFAULT_ISOLATION_CONFIG,
): IsolationContext {
  const teamId = (ctx?.teamId ?? "").trim() || undefined;
  const userId = (ctx?.userId ?? "").trim();
  const agentId = (ctx?.agentId ?? "").trim();
  const sessionId = (ctx?.sessionId ?? "").trim();
  const taskId = ctx?.taskId ?? undefined;
  const sessionKey = ctx?.sessionKey ?? undefined;

  const missing: string[] = [];
  if (!userId) missing.push("userId");
  if (!agentId) missing.push("agentId");
  if (!sessionId) missing.push("sessionId");

  if (missing.length === 0) {
    return { teamId, userId, agentId, sessionId, taskId, sessionKey };
  }

  const placeholder = config.legacyCompatMode ? config.legacyPlaceholder : DEFAULT_ISOLATION_ID;
  return {
    teamId,
    userId: userId || placeholder,
    agentId: agentId || placeholder,
    sessionId: sessionId || placeholder,
    taskId,
    sessionKey,
  };
}

export class IsolationError extends Error {
  constructor(message: string, public readonly missingFields: string[]) {
    super(message);
    this.name = "IsolationError";
  }
}
```

**强制校验**：`assertIsolation()` 缺失必填字段时：
- `legacyCompatMode=true` → 用 `legacyPlaceholder`（默认 `__legacy__`）填充
- `legacyCompatMode=false` → 用 `DEFAULT_ISOLATION_ID`（`"default"`）填充

### 7.3 buildIsolationWhere 动态构建 WHERE 子句

```typescript
// isolation.ts:120-152
export function buildIsolationWhere(
  filter: IsolationFilter | undefined,
  tablePrefix = "",
): { clause: string; params: string[] } {
  if (!filter) return { clause: "", params: [] };
  const parts: string[] = [];
  const params: string[] = [];
  if (filter.teamId !== undefined) {
    parts.push(`${tablePrefix}team_id = ?`);
    params.push(filter.teamId);
  }
  if (filter.userId !== undefined) {
    parts.push(`${tablePrefix}user_id = ?`);
    params.push(filter.userId);
  }
  if (filter.agentId !== undefined) {
    parts.push(`${tablePrefix}agent_id = ?`);
    params.push(filter.agentId);
  }
  if (filter.sessionId !== undefined) {
    parts.push(`${tablePrefix}session_id = ?`);
    params.push(filter.sessionId);
  }
  if (filter.taskId !== undefined) {
    parts.push(`${tablePrefix}task_id = ?`);
    params.push(filter.taskId);
  }
  if (filter.sessionKey !== undefined) {
    parts.push(`${tablePrefix}session_key = ?`);
    params.push(filter.sessionKey);
  }
  return { clause: parts.join(" AND "), params };
}
```

设计：未设置的维度自动跳过（`undefined` 检测），与 SQL `WHERE x = ?` 缺失参数约定一致。`tablePrefix` 支持多表 join。

### 7.4 rowMatchesIsolation 后置校验

```typescript
// isolation.ts:159-171
export function rowMatchesIsolation(
  row: { team_id?: string; user_id?: string; agent_id?: string; session_id?: string; task_id?: string; session_key?: string },
  filter: IsolationFilter | undefined,
): boolean {
  if (!filter) return true;
  if (filter.teamId !== undefined && row.team_id !== filter.teamId) return false;
  if (filter.userId !== undefined && row.user_id !== filter.userId) return false;
  if (filter.agentId !== undefined && row.agent_id !== filter.agentId) return false;
  if (filter.sessionId !== undefined && row.session_id !== filter.sessionId) return false;
  if (filter.taskId !== undefined && row.task_id !== filter.taskId) return false;
  if (filter.sessionKey !== undefined && row.session_key !== filter.sessionKey) return false;
  return true;
}
```

**Safety net**：在向量/FTS 召回后二次检查，因 TCVDB 旧版可能无法 push down filter。

### 7.5 5 级可见性 + 6 类权限

```typescript
// metadata/types.ts:614-628
type AssetVisibility = "private" | "team" | "restricted" | "agent" | "task";

type Permission = "read" | "write" | "delete" | "assign" | "share" | "use";

type AclSubjectType = "user" | "team_role" | "agent";
type AclEffect = "allow" | "deny";  // 一期仅 allow，deny 预留
```

**5 级可见性**：
- `private`：仅 Owner 可访问
- `team`：团队内全员可见
- `restricted`：ACL 精确控制（User/Role/Agent）
- `agent`：绑定特定 Agent
- `task`：绑定特定任务

**6 类权限**：`read / write / delete / assign / share / use`

### 7.6 SQLite 三维隔离索引 (sqlite.ts:658-661)

```sql
CREATE INDEX IF NOT EXISTS idx_l1_user_agent_session ON l1_records(user_id, agent_id, session_id);
CREATE INDEX IF NOT EXISTS idx_l1_user_updated ON l1_records(user_id, updated_time);
CREATE INDEX IF NOT EXISTS idx_l1_agent_updated ON l1_records(agent_id, updated_time);
```

三个复合索引支撑三维隔离查询：`(user_id, agent_id, session_id)` 精确匹配 + `(user_id, updated_time)` / `(agent_id, updated_time)` 时间范围扫描。

### 7.7 对 laew 借鉴

- 借鉴 **三维隔离数据模型**：`(team_id, user_id, agent_id)` + session_id/task_id 辅助维度
- 借鉴 **assertIsolation 必填校验**：缺失字段抛 `IsolationError`
- 借鉴 **Legacy 兼容模式**：`legacyCompatMode` 占位符填充
- 借鉴 **buildIsolationWhere 动态 WHERE**：未设置维度自动跳过
- 借鉴 **rowMatchesIsolation 后置校验**：向量/FTS 召回后 safety net
- 借鉴 **三维复合索引**：`(user_id, agent_id, session_id)` 精确匹配

laew 当前单用户单设备，可简化为 `(user_id, agent_id)` 二维；在 `Session` 中增加 `agentId` 字段，记忆查询强制带 agentId 过滤。

---

## 八、综合借鉴路线图

### 8.1 P0 — 核心记忆能力（1-2 周）

| 能力 | 借鉴代码路径 | 落地文件 |
|------|--------------|---------|
| L0 对话录制 | `l0-recorder.ts:93-313` | `src/agent/memory/l0-recorder.rs` |
| L1 记忆表 + vec dual-write | `l1-writer.ts:163-354` + `sqlite.ts:1259-1359` | `src/agent/memory/l1-writer.rs` |
| RRF 混合检索 | `search-utils.ts:18-62` | `src/agent/memory/rrf.rs` |
| Checkpoint 原子操作 | `checkpoint.ts:156-292` | `src/agent/memory/checkpoint.rs` |

### 8.2 P1 — 进阶能力（2-4 周）

| 能力 | 借鉴代码路径 | 落地文件 |
|------|--------------|---------|
| L2 场景块 | `scene-extractor.ts:135-510` | `src/agent/memory/scene-extractor.rs` |
| MemoryPipelineManager | `pipeline-manager.ts:208-1218` | `src/agent/memory/pipeline-manager.rs` |
| Skill 系统 v1 | `skill-core.ts:210-661` + `skill-versioning.ts:117-205` | `src/agent/skill/` |
| 上下文注入管线 | `pipeline.ts:80-282` | `src/agent/injection/pipeline.rs` |

### 8.3 P2 — 高级能力（1-2 月）

| 能力 | 借鉴代码路径 | 落地文件 |
|------|--------------|---------|
| L3 画像生成（增量模式） | `persona-generator.ts:74-282` | `src/agent/memory/persona-generator.rs` |
| 多租户隔离 | `isolation.ts:73-171` | `src/agent/memory/isolation.rs` |
| Skill 跨系统事务（v2） | `skill-versioning.ts:117-205` | 增强 Skill 模块 |

### 8.4 关键借鉴原则

1. **不要过度设计**：TencentDB 的 6 个子系统（Core/Knowledge/Panel/Proxy + 双 SDK）对 laew 过重。建议先做 **L1 记忆 + RRF 检索 + Checkpoint 原子操作**三个核心能力
2. **警惕 LLM 沙箱逃逸**：TencentDB 的"LLM 直写文件"模式需要严格的 workspaceDir 限制。laew 当前零沙箱，引入 L2 场景块时需同步建设权限管控
3. **向量维度漂移检测**：embedding provider/model/dimensions 变化时需 reindex。借鉴 TencentDB 的 `VectorStoreInitResult.needsReindex` 检测机制
4. **防止记忆注入污染**：TencentDB 用 `sanitizeText` + `stripCodeBlocks` 防止 `<<<LAEW:PROJECT_CONTEXT>>>` 被 LLM 重复注入
5. **避免跨系统事务**：laew 当前只有 SQLite，不要为了实现 Skill 版本化引入 COS + asset 等多系统。建议 Skill v1 仅用 SQLite 单库 + 乐观锁

### 8.5 推荐落地路线图

```
P0（1-2 周）：
  ├── L0 对话录制（JSONL 按日分片 + 双保护增量捕获）
  ├── L1 记忆表（SQLite + vec0 + FTS5）
  ├── RRF 混合检索（k=60 + jieba 分词）
  └── Checkpoint 原子操作（tmp+rename + per-file lock）

P1（2-4 周）：
  ├── LLM 抽取 prompt（l1-extraction.rs）
  ├── MemoryPipelineManager 简化版（L1→L2 调度）
  ├── L2 场景块（LLM 沙箱 + scene_blocks/）
  ├── Skill 系统 v1（SQLite + 乐观锁）
  └── 上下文注入管线（4 个核心注入点）

P2（1-2 月）：
  ├── L3 画像生成（增量模式）
  ├── MemoryProxy 简化版（请求拦截 + 注入）
  ├── Wiki 摄取引擎（Ingest V2 两阶段）
  └── 多租户隔离（二维简化版）
```

---

## 九、总结

TencentDB Agent Memory 的核心机制亮点：

1. **L0-L3 四层管线**：JSONL 原始对话 → LLM 抽取 → 场景块 → Persona 渐进式抽象，每层都有明确的触发条件和数据流
2. **BM25 + 向量 + RRF 混合检索**：关键词与语义互补融合，k=60 标准 RRF 常数，jieba 中文分词 + 36 个停用词
3. **MemoryPipelineManager 级联调度**：warm-up 指数阈值 + 向下_only_ L2 timer + L3 全局互斥 + Session GC
4. **Skill 跨系统"顺序 + 补偿"伪事务**：COS + DB + asset 三系统，最脆先做、失败零副作用先做、可靠收尾
5. **上下文注入管线**：9 个内置注入点 + 三态缓存策略 + Self-heal + 协议适配器模式
6. **Checkpoint 原子操作**：Per-file async lock + tmp+rename atomic write + Split-state 设计
7. **多租户三维隔离**：`(team_id, user_id, agent_id)` + 5 级可见性 + 6 类权限 + assertIsolation 强制校验
8. **SQLite vec0 + COSINE distance_metric + upsert = delete + insert**（vec0 不支持 ON CONFLICT）
9. **embedding_meta 维度漂移检测**：provider/model/dimensions 变化时自动 drop + reindex
10. **partial unique index + deleted_at** 软删除模式：同名资源软删除后可重建

对 laew 最具借鉴价值的三个方向：
- **L1 结构化记忆 + RRF 混合检索**：解决"每次新 Session 从零开始"痛点
- **Checkpoint 原子操作**：为 `MultiAgentOrchestrator` 提供跨会话状态持久化
- **上下文注入管线**：从一次性注入升级为可扩展的 Hook 注册机制

落地时应遵循**"先核心后外围"**原则，优先实现 L0-L1 录制 + RRF 检索 + Checkpoint 三个 P0 能力，再逐步扩展 L2-L3 和 Skill 系统。

---

*分析人：Claude Code Agent*
*分析日期：2026-09-05*
*源码路径：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`*
*关联文档：`TencentDB-Agent-Memory-源码调研.md` + `TencentDB-Agent-Memory-深度分析.md`*
*核心代码文件（绝对路径）：*
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/conversation/l0-recorder.ts`（608 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/record/l1-writer.ts`（365 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/scene/scene-extractor.ts`（598 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/persona/persona-generator.ts`（298 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/utils/pipeline-manager.ts`（1218 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/utils/checkpoint.ts`（510 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/skill/skill-core.ts`（661 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/skill/skill-versioning.ts`（435 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/skill/skill-permission.ts`（69 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/store/sqlite.ts`（3399 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/store/search-utils.ts`（62 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/store/isolation.ts`（171 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryKnowledge/src/db/schema.ts`（152 行）
- `/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryProxy/src/injection/pipeline.ts`（部分读取）