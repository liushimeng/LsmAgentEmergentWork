# TencentDB Agent Memory 深度分析

> **分析目标**：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`（v2.0.0-beta.1，2026-08 快照）
>
> **前置调研**：`TencentDB-Agent-Memory-源码调研.md`（已完成工程结构、模块边界、设计概览）
>
> **分析维度**：L0-L3 管线 · 记忆管线调度 · Skill 系统 · 上下文注入 · 检索机制 · 存储层 · Wiki 引擎 · CodeGraph · 多租户隔离 · MemoryProxy

---

## 一、L0-L3 管线深度分析

### 1.1 整体数据流

```
用户输入 → agent_end hook
  │
  ▼
L0 Recorder (l0-recorder.ts)
  │  JSONL 按日分片 (conversations/YYYY-MM-DD.jsonl)
  │  双保护增量捕获（位置切片 + 时间戳游标）
  ▼
L1 Writer (l1-writer.ts)
  │  LLM 抽取 → DedupDecision(store/update/merge/skip)
  │  双写：JSONL (source of truth) + vec0 (检索引擎)
  ▼
MemoryPipelineManager (pipeline-manager.ts)
  │  L1 完成 → advanceL2Timer → L2 Scene Extractor
  ▼
L2 Scene Extractor (scene-extractor.ts)
  │  LLM 沙箱（workspaceDir=scene_blocks/）
  │  输出 .md 场景文件 + persona 更新信号
  ▼
L3 Persona Generator (persona-generator.ts)
  │  增量模式（仅分析变化场景）
  │  LLM 直写 persona.md
  ▼
Checkpoint (checkpoint.ts)
     持久化进度（原子 tmp+rename）
```

### 1.2 L0 Recorder 关键实现

**代码路径**：`MemoryCore/src/core/conversation/l0-recorder.ts`（608 行）

**核心函数**：`recordConversation(params)` — 接收 `agent_end` hook 投递的全量 `rawMessages`，执行四步处理：

**Step 1 — 位置切片 + 时间戳游标双保护**：

```typescript
const usePositionSlice = originalUserMessageCount != null && originalUserMessageCount > 0
  && originalUserMessageCount <= rawMessages.length;
const slicedMessages = usePositionSlice
  ? rawMessages.slice(originalUserMessageCount)  // 仅保留 prompt 构建后的新消息
  : rawMessages;

// 第二层：严格大于 (>) 游标过滤
const cursor = afterTimestamp ?? 0;
const extracted = cursor !== 0
  ? allExtracted.filter((m) => m.timestamp > cursor)
  : allExtracted;
```

设计意图：位置切片免疫重启后时间戳漂移（缓存的 `originalUserMessageCount` 在 `before_prompt_build` 时确定），时间戳游标作为缓存失效时的 fallback。**安全阀**：当位置切片不可用且时间戳过滤全量通过（>8 条）时打 warn，提示可能发生时间戳漂移。

**Step 2 — 污染消息替换**：框架在 `before_prompt_build` 之后给 user 消息追加 `prependContext`，导致原始 user 消息被污染。通过缓存的 `originalUserText` + `originalUserMessageCount` 定位污染消息并替换回干净版本，定位依据是 timestamp 匹配。

**Step 3 — 消毒过滤**：`sanitizeText()` 剥离注入标记防止反馈循环；`stripCodeBlocks()` 从 assistant 回复中移除围栏代码块（降低 embedding 噪声）；`shouldCaptureL0()` 过滤太短/无意义消息。

**Step 4 — 幂等写入 JSONL**：每条消息独立一行 `L0MessageRecord`，包含 `sessionKey/sessionId/userId/agentId/recordedAt/id/role/content/timestamp` 九字段。写入是 append-only，**同一批次要么全写要么全跳过**（由调用方保证不重试部分批次）。

**读取接口**：
- `readConversationRecords(sessionKey, baseDir)` — 按 sessionKey 过滤逐日 JSONL
- `readConversationMessages(sessionKey, baseDir, afterTimestamp?, limit?)` — 支持游标+限流
- `readConversationMessagesGroupedBySessionId(...)` — 按 sessionId 分组（同一 sessionKey 下不同 /reset 实例）

### 1.3 L1 Writer 关键实现

**代码路径**：`MemoryCore/src/core/record/l1-writer.ts`（365 行）

**记忆类型**（`MemoryType`）：`persona / episodic / instruction / work_fact / work_task / work_method / work_artifact` 七种，覆盖个人偏好、事件回忆、指令、工作事实/任务/方法/产物。

**优先级**：数值 0-100，-1 表示严格全局指令（不可被 merge/skip）。

**核心函数**：`writeMemory(params)` — 依据 `DedupDecision.action` 执行四种操作：

| action | 行为 | 向量存储 |
|--------|------|---------|
| `store` | 追加新记录 | 直接 upsertL1 |
| `update` | 删除旧 target + 写入新记录 | 先 deleteL1Batch 再 upsertL1 |
| `merge` | 删除多个旧 target + 写入合并记录 | 同上 |
| `skip` | 什么都不做 | 无操作 |

**版本控制**：update/merge 时先查询 target 最大版本号 `maxVersion`，新版本 = `maxVersion + 1`。

**双写策略**（`vec-dual-write`）：
1. JSONL 追加先行（source of truth，backup/recovery 用）
2. 异步调用 `embeddingService.embed()` 获取向量
3. 调用 `vectorStore.upsertL1(record, embedding)` 写入 vec0
4. Embedding 失败时仅写 metadata + FTS，跳过 vec0（graceful degradation）

**去重决策**（`DedupDecision`）由独立 LLM 抽取 prompt（`core/prompts/l1-dedup.ts`）产生，包含 `target_ids`（多目标合并）、`merged_content/merged_type/merged_priority/merged_timestamps` 合并产物字段。

### 1.4 L2 Scene Extractor 关键实现

**代码路径**：`MemoryCore/src/core/scene/scene-extractor.ts`（598 行）

**安全沙箱**：LLM 的 `workspaceDir` 设为 `scene_blocks/`，仅能操作 .md 场景文件；`scene-index.json` 和 `persona.md` 物理不可见。

**核心函数**：`extract(memories)` — 八阶段流水线：

1. **Phase 1 备份**：`BackupManager.backupDirectory()` 快照整个 `scene_blocks/`，LLM 失败时自动还原（fail-soft：还原失败不掩盖原始 LLM 错误）
2. **Phase 2 加载索引**：`readSceneIndex()` 读取已有场景清单，构建摘要 + 容量计数器（`当前场景总数：N / maxScenes`）+ 场景数警告（≥上限强制 MERGE、=上限-1 禁止 CREATE）
3. **Phase 3 构建 prompt**：`buildSceneExtractionPrompt()` 注入记忆 JSON + 场景摘要 + 警告
4. **Phase 4 LLM 执行**：`CleanContextRunner.run()` 带工具调用，timeout=300s
5. **Phase 5 清理软删除**：LLM 用 `[DELETED]` 标记"删除"文件（无 exec 工具），此阶段真实 unlink
6. **Phase 5b 文件名规范化**：`normalizeSceneFilenames()` 修正 LLM 偶尔产生的非法文件名（空格/标点）
7. **Phase 6 同步索引**：`syncSceneIndex()` 从磁盘重建 JSON 索引
8. **Phase 7 更新导航**：`updateSceneNavigation()` 更新 persona.md 末尾的导航树

**Persona 更新信号**：LLM 文本输出中的 `[PERSONA_UPDATE_REQUEST]reason[/PERSONA_UPDATE_REQUEST]` 被 `parsePersonaUpdateSignal()` 解析，写入 checkpoint 的 `request_persona_update` 字段，由后续 L3 处理。

**空抽取检测**：`preExtractIndex.size === 0 && postIndex.length === 0` 判定 LLM 未产出任何文件，触发告警。

### 1.5 L3 Persona Generator 关键实现

**代码路径**：`MemoryCore/src/core/persona/persona-generator.ts`（298 行）

**触发条件**：
- 每 N 条新记忆（默认 50，由 `memories_since_last_persona` 计数）
- 场景变化（L2 完成触发 L3）
- `[PERSONA_UPDATE_REQUEST]` 信号

**核心函数**：`generateLocalPersona(triggerReason)` — 十一阶段：

1. 读取 checkpoint（获取 `total_processed`、`last_persona_time`）
2. 读取现有 persona.md（剥离导航）
3. 加载 scene index，筛选 `updated > last_persona_time` 的变化场景
4. 读取变化场景全文（full raw content including META）
5. 确定模式：`first`（首次）或 `incremental`（增量）
6. 构建 prompt（`buildPersonaPrompt()`）
7. 备份 persona.md（`BackupManager.backupFile()` 保留最近 N 份）
8. LLM 执行（sandboxed to dataDir，tools enabled，timeout=180s）
9. 读取 LLM 写入的 persona.md
10. 消毒（`escapeXmlTags(stripSceneNavigation())`）
11. 追加导航并写盘

**增量模式关键**：`changedScenesContent` 仅包含变化场景，prompt 提示"重点分析变化场景"，未变化场景不重读，显著降低 token 消耗。

---

## 二、记忆管线调度深度分析

### 2.1 MemoryPipelineManager 核心算法

**代码路径**：`MemoryCore/src/utils/pipeline-manager.ts`（1219 行）

**架构**：三层 SerialQueue（`l1Queue/l2Queue/l3Queue`）+ 每 session 双 Timer（`l1Idle/l2Schedule`）+ 消息缓冲区。

**Warm-up 模式**：新 session 的 L1 触发阈值从 1 开始指数增长（`1 → 2 → 4 → 8 → ... → everyNConversations`）。`advanceWarmupThreshold()` 在每轮 L1 成功后翻倍，达到 `everyNConversations` 后置 `warmup_threshold=0` 标记毕业。这保证早期对话被快速处理（第 1 轮就触发 L1），随会话成熟逐步降低频率。

**L1 触发三路径**：
- **Path A (threshold)**：`conversation_count >= effectiveThreshold`，立即 `enqueueL1()`
- **Path B (idle_timeout)**：用户停止对话 `l1IdleTimeoutSeconds` 后 `onL1IdleTimeout()` 触发
- **Path C (flush)**：graceful shutdown 或 `flushSession()` 时排空缓冲

**L2 向下_only_ Timer**：`advanceL2Timer()` 计算 `T_desired = max(now + l2DelayAfterL1, lastL2 + l2MinInterval)`，仅当 `T_desired` 早于当前调度时才推进（`tryAdvanceTo()`）。这保证：
- `delayAfterL1` 让远程 L1 完成异步记录生成
- `minInterval` 防止 L2 过于频繁
- `maxInterval` 由 `armL2MaxInterval()` 在 L2 完成后无条件设置（`now + l2MaxInterval`）

**L3 全局互斥 + pending 去重**：`l3Running` 标志防止并发，`l3Pending` 标志在 L3 运行期间有新 L2 完成时标记需重跑。`enqueueL3()` 的 finally 块检查 `l3Pending` 实现链式重跑。

**Session GC**：每 50 次 `notifyConversation()` 调用触发 `gcStaleSessions()`，淘汰 inactive > `sessionActiveWindowMs * 3` 且无排队任务/缓冲消息的冷 session，防止内存无限增长。

**失败恢复**：
- L1 失败：消息放回缓冲区 + `l1RetryCount` 递增，调度 30s 后重试（最多 5 次）
- L2 失败：`armL2MaxInterval()` 保证最终重试
- 全部失败：`persistStates()` 持久化到 checkpoint，`recoverPendingSessions()` 在下次启动时恢复

### 2.2 Checkpoint 原子操作

**代码路径**：`MemoryCore/src/utils/checkpoint.ts`（511 行）

**Split-state 设计**：`runner_states` 与 `pipeline_states` 两个命名空间物理隔离，防止 PipelineManager 的 `persistStates()` 覆盖 L0/L1 runner 写入的游标字段（split-brain 问题）。

**Per-file async lock**：`withFileLock(filePath, fn)` 用 Promise 链序列化同一文件的并发 read-modify-write：

```typescript
const prev = fileLocks.get(filePath) ?? Promise.resolve();
let release!: () => void;
const gate = new Promise<void>((r) => { release = r; });
fileLocks.set(filePath, gate);
await prev;
try { return await fn(); } finally { release(); }
```

多个 `CheckpointManager` 实例共享同文件路径时自动共享锁。

**Atomic write**：`writeRaw()` 先写 `tmp` 文件再 `rename()` 到目标路径，防止崩溃时文件损坏。

**核心 API**：
- `captureAtomically(sessionKey, pluginStartTimestamp, fn)` — 在单锁内读游标 → 执行捕获 → 推进游标，消除竞态窗口
- `markL1ExtractionComplete(sessionKey, memoriesExtracted, cursorRecordedAtMs, lastSceneName)` — L1 完成后更新
- `markPersonaGenerated(totalProcessed)` — L3 完成后重置计数
- `mergePipelineStates(states)` — PipelineManager 独占写入 `pipeline_states`

---

## 三、Skill 系统深度分析

### 3.1 SkillCore 六写四读

**代码路径**：`MemoryCore/src/core/skill/skill-core.ts`

**6 个写动作**：

| action | 入参 | 行为 |
|--------|------|------|
| `create` | name, content, resources | 生成 skill_id（`skl-` + 12 字符 base62 CSPRNG），碰撞检测 3 次 |
| `update` | skill_id, expected_version, content | 替换 SKILL.md，禁止改名 |
| `patch` | skill_id, old_string, new_string | 单点串替，非唯一 old_string 要求 `replace_all=true` |
| `delete` | skill_id, expected_version | 物理删除所有版本 + storage + asset |
| `writeFiles` | skill_id, files | 增/改资源文件 |
| `removeFiles` | skill_id, paths | 删除资源文件 |

**4 个读动作**：`get` (detail，默认 head，可指定 version) / `list` (按 team_id + filters) / `search` (BM25/embedding/hybrid) / `listVersions` (历史版本) / `readFile` (资源字节)

**权限校验**（`skill-permission.ts`）：
- `assertOwner(head, agent_id)` — 非 owner 抛 `SKILL_NOT_OWNER`
- `assertTeamMatch(head, team_id)` — team 不匹配抛 `SKILL_TEAM_MISMATCH`
- `assertVersionFresh(head, expected_version)` — 版本过时不一致抛 `SKILL_VERSION_STALE`（乐观锁）

**ID 生成**：`randomBase62(12)` 提供 ~71 bit 真熵，单实例 100 万 skill 碰撞概率 ~1.5e-10。Collision 时重试最多 3 次，超限抛 `SKILL_ID_COLLISION`。

### 3.2 SkillVersioning 跨系统事务

**代码路径**：`MemoryCore/src/core/skill/skill-versioning.ts`

**跨系统事务编排（COS + skill DB + meta_assets 三系统）**：

```
createNewSkill():
  Step 1: writeResource → COS            ← 最脆先做，失败零副作用
  Step 2: store.appendVersion → skill DB ← 失败反向清 COS (cleanupVersionDir)
  Step 3: onSkillCreated → meta_assets    ← 失败反向删 DB + 清 COS

appendNextVersion():
  1. copyTree(oldDir → newDir)           ← 旧版本目录拷贝
  2. apply resources (write/remove)      ← 在新目录上应用增量
  3. store.appendVersion → DB            ← 事务内写
```

**幂等保证**：`appendNextVersion()` 检测 `newContentHash === head.content_hash` 且无资源变更时直接返回 head（无操作），避免无效版本。

**TTL 清理**：`cleanupExpiredVersionsForSkill(skill_id, versionTtlSeconds)` 异步 fire-and-forget，清理超期旧版本。

**错误码体系**（`SkillCoreErrorCode` 14 种）：覆盖 `INVALID_FRONTMATTER / SKILL_PATCH_NOT_UNIQUE / SKILL_NAME_DUPLICATE / SKILL_NOT_OWNER / SKILL_TEAM_MISMATCH / SKILL_NOT_FOUND / SKILL_VERSION_STALE / SKILL_ID_COLLISION / INVALID_PATH / RESOURCE_TOO_LARGE / STORAGE_NOT_FOUND / LLM_UNAVAILABLE / SKILL_COS_REQUIRED`。

### 3.3 Skill 文件格式

**代码路径**：`MemoryCore/src/core/skill/skill-format.ts`

```yaml
---
name: skill-name          # 必填，^[a-z0-9][a-z0-9-]*$
description: ...          # 必填，≤1024 字符
category: ...
resources:
  - path: ./template.sh
    type: executable      # text | executable | binary
---
正文 body（≤50000 字符）
```

**存储实现**：`SqliteSkillStore` 单表 `skills` + `skill_fts` (FTS5 全文索引) + `skill_vec` (可选 vec0 向量索引)。五元组身份 `(user_id, owner_agent_id, team_id, task_id, skill_id)`，版本写入：旧 head `is_head` 改 0 → INSERT 新行 → fts5 同步（事务原子）。

**TCVDB 实现**：`TcvdbSkillStore` 走腾讯云向量数据库，统一实现 `ISkillStore` 接口。

---

## 四、上下文注入深度分析

### 4.1 InjectionPipeline 核心流程

**代码路径**：`MemoryProxy/src/injection/pipeline.ts`

```
raw body → Adapter.parse() → AgentContext → execute hooks → Adapter.serialize() → modified body
```

**协议适配器**：`OpenAIAdapter` / `AnthropicAdapter` 双向转换，由 `metadata.protocol` 路由。

**Agent 识别双通道**：
1. **Fast path**：URL path 前缀（如 `/claude-code/...`）→ `agentProfiles.get(metadata.agentSource)` 零成本查找
2. **Legacy fallback**：扫描 system prompt 文本匹配（deprecated）

### 4.2 8 个内置注入点（执行顺序）

| 次序 | injection point | 典型用途 |
|------|----------------|---------|
| 1 | `system.prefix` | L3 Persona + L2 场景导航 |
| 2 | `system.before_tools` | 自有 skill 列表 + skill_tools 调用指南 |
| 3 | `system.after_tools` | tdai_memory_search 工具指南 + Wiki/CodeGraph 工具指南 |
| 4 | `system.suffix` | Asset Reflection 内部效果评估 |
| 5 | `tools.prepend` | 前置工具注入 |
| 6 | `tools.append` | 后置工具注入 |
| 7 | `user.first_turn` | 首回合用户消息注入 |
| 8 | `user.before` | L1 记忆召回（自有 + 借入） |
| 9 | `user.after` | 用户消息后置注入 |

### 4.3 缓存策略三态

```typescript
type CacheStrategy = "none" | "session_init" | "hybrid";
```

- **`none`**：每次执行 `hook.execute(ctx)`（legacy 行为）
- **`session_init`**：session 初始化时预热一次，后续从 `HookCacheRepo` 读取（keyed by `spaceId + userId + agentSource + sessionId + hookId`）
- **`hybrid`**：预热 + 实时执行并集去重（按 `metadata.cacheKey ?? content`）

**Self-heal**：`session_init` 在 cache miss 时回退到 `execute()` 并回填缓存，覆盖预热失败/TTL 过期/预热未完成首请求等场景。

### 4.4 内置 Injector 清单

| Injector | 注入点 | 功能 |
|---------|-------|------|
| `SkillInjector` | system.before_tools | `<available_skills>` 自有 skill 列表 |
| `SkillToolsInjector` | system.before_tools | `<skill_tools>` curl 调用指南 |
| `TdaiL1RecallInjector` | user.before | 自有 + 借入 L1 记忆召回 |
| `TdaiProfileMemoryInjector` | system.prefix | L3 Persona + L2 场景导航 |
| `TdaiToolsInjector` | system.after_tools | `tdai_memory_search` 等工具指南 |
| `KnowledgeToolsInjector` | system.after_tools | Wiki/CodeGraph 工具指南 |
| `AssetReflectionInjector` | system.suffix | `<asset_reflection>` 内部效果评估 |

### 4.5 L1 召回注入器（`tdai-l1-recall-injector.ts`）

**核心流程**：

```typescript
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

---

## 五、检索机制深度分析

### 5.1 RRF 融合（`core/store/search-utils.ts`）

```typescript
export const RRF_K = 60;

export function rrfMerge<T>(lists: T[][], getId: (item: T) => string, k = RRF_K) {
  const map = new Map<string, { item: T; rrfScore: number }>();
  for (const list of lists) {
    for (let rank = 0; rank < list.length; rank++) {
      const item = list[rank];
      const id = getId(item);
      const score = 1 / (k + rank + 1);
      const existing = map.get(id);
      if (existing) existing.rrfScore += score;
      else map.set(id, { item, rrfScore: score });
    }
  }
  return [...map.values()].sort((a, b) => b.rrfScore - a.rrfScore);
}
```

- **去重合并**：同一 item 在多列表中出现时分数累加
- **k=60**：标准 RRF 常数，平滑低排名项权重；k 越大低排名项权重越高

### 5.2 FTS5 + jieba 中文分词

**代码路径**：`MemoryCore/src/core/store/sqlite.ts`

**查询侧**（`buildFtsQuery()`）：
- 优先使用 `@node-rs/jieba` `cutForSearch()`（search-engine mode）切分中文
- 例如 `"用户喜欢编程和TypeScript"` → `"用户" OR "喜欢" OR "编程" OR "TypeScript"`
- 停用词过滤：`ZH_STOP_WORDS` 集合（的/了/在/是/我/有/和/... 等 36 个高频虚词）
- jieba 不可用时回退到 Unicode-regex `/[\p{L}\p{N}_]+/gu`

**索引侧**（`tokenizeForFts()`）：
- 同样使用 `cutForSearch()` 切分后用空格拼接
- 存入 FTS5 `content` 列，`unicode61` tokenizer 进一步拆分
- 保证查询侧和索引侧 token 空间一致

**BM25 排名转分**（`bm25RankToScore()`）：
```typescript
export function bm25RankToScore(rank: number): number {
  if (!Number.isFinite(rank)) return 1 / (1 + 999);
  return 1 / (1 + Math.exp(-rank));  // sigmoid 映射到 0-1
}
```

### 5.3 vec0 向量检索

**存储**：`l1_vec` / `l0_vec` 虚拟表，cosine 距离。

**维度适配**：支持 Matryoshka 截断（`sendDimensions` 开关），降低存储和检索开销。

**upsert = delete + insert**：vec0 不支持 ON CONFLICT，需先 DELETE 再 INSERT。

### 5.4 Embedding 服务

**代码路径**：`MemoryCore/src/core/store/embedding.ts`

- **多 Provider**：OpenAI-compatible（`@ai-sdk/openai`）
- **代理支持**：`proxyUrl` 本地代理转发
- **超时分级**：全局 / recall / capture 三档超时（capture 更短避免阻塞对话）

---

## 六、存储层深度分析

### 6.1 SQLite 四表结构（`core/store/sqlite.ts`）

| 表 | 用途 | 关键索引 |
|---|------|---------|
| `l1_records` | L1 结构化记忆元数据 | 主键 + 多维度索引 |
| `l1_vec` | L1 向量索引 | vec0 虚拟表（cosine） |
| `l0_conversations` | L0 原始对话元数据 | session_key + timestamp |
| `l0_vec` | L0 向量索引 | vec0 虚拟表 |

**附加表**：
- `skills` + `skill_fts` + `skill_vec`：Skill 主表 + 全文索引 + 向量索引
- `profiles`：L2/L3 画像文件元数据
- `memory_audit`：审计日志

**关键设计**：
- **同步 API**：`DatabaseSync`（Node 22+ `node:sqlite` 内置模块）
- **WAL 模式**：线程安全
- **手动事务**：BEGIN/COMMIT 保证 metadata + vector 原子写入
- **upsert = delete + insert**：vec0 不支持 ON CONFLICT

### 6.2 Knowledge DB Drizzle ORM（`MemoryKnowledge/src/db/schema.ts`）

| 表 | 用途 |
|---|------|
| `knowledge_code_graph` | 代码仓库索引元数据 |
| `knowledge_wiki` | Wiki 知识库元数据 |
| `knowledge_wiki_audit` | Wiki 状态变更审计 |
| `knowledge_code_graph_audit` | CodeGraph 状态变更审计 |
| `llm_binding` | Per-instance LLM 路由绑定（proxy/byo 模式） |

**核心字段**：`serviceId / teamId / repoUrl / branch / status / visibility / version / deletedAt`

**软删除**：`deleted_at` + partial unique index（`WHERE deleted_at IS NULL`）

**版本乐观锁**：`version` 字段

### 6.3 TCVDB 迁移策略

**代码路径**：`MemoryCore/src/core/store/tcvdb.ts`

- **服务端 embedding**：`embeddingItems` 配置
- **客户端稀疏向量**：`BM25LocalEncoder`（纯 TS 实现，基于 `@tencentdb-agent-memory/tcvdb-text`）
- **原生混合搜索**：`hybridSearch`（dense + sparse + RRFRerank）
- **Filter 表达式**：标量字段过滤

**统一接口**：`IMemoryStore` trait 隔离 SQLite / TCVDB，上层无感知切换。

---

## 七、Wiki 引擎深度分析

### 7.1 Ingest V2 两阶段 LLM 摄取

**代码路径**：`MemoryKnowledge/src/engines/wiki/ingest-v2/index.ts`

```
读源文件全文 → 加载模板 schema.md/purpose.md → 扫已有页清单
  → （超长分块，SOURCE_CHAR_BUDGET=28000）
  → 两阶段 LLM 生成 FILE 块：
    阶段 A：分析产出抽取计划（analysis）
    阶段 B：依据计划生成 FILE 块（generate）
  → parseFileBlocks 解析 FILE 块 → canonicalizePagePath 路径规范化
  → dedup（locked 跳过 / mergePage 合并）
  → 写盘（跳过结构性文件 STRUCTURAL_FILES）
  → rebuildIndexFile 重建 wiki/index.md
  → appendIngestLog 追加 wiki/log.md
```

**单阶段模式**（`mode: "single-stage"`）：源全文直接产出 FILE 块，省一次 LLM 调用。

**去重不变量**（OQ-6）：不完全信任 LLM 选的路径，用 frontmatter `type + title` 重新规范化落盘路径（`canonicalizePagePath()`），保证"同一实体二次摄取 → 同一路径"。

**结构性文件保护**：`wiki/index.md / schema.md / purpose.md / log.md / overview.md` 禁止 ingest 写入/覆盖。

**合并策略**（`mergePage()`）：旧页超阈值（`fullRewriteMaxChars`）走追加模式，否则全量重写。

### 7.2 Graph 多跳检索

**代码路径**：`MemoryKnowledge/src/engines/wiki/graph-search.ts`

**算法**：BFS 分层扩展，从 BM25 seed 出发沿 `[[wikilink]]` 边行走 `hop` 步（0-5），每跳 score 乘以 `decay`（默认 0.5），低于 `minScore`（默认 0.1）剪枝。

**关键不变量**：
- **Seed 冻结**：seed 节点保持 hop=0 和原始 BM25 score，不参与衰减
- **首次到达即定 hop**：BFS 层序保证第一次到达某节点时 hop 最小，后续同分或更高分但更大 hop 的到达被忽略
- **最高分优先**：非 seed 节点多条路径到达时取最高分
- **DoS 防护**：`maxNodes` 硬上限（默认 200）

**graphology 集成**：临时内存实例做多跳 BFS，社区发现使用 `graphology-communities-louvain` Louvain 算法。

### 7.3 Wiki 索引存储（`index-db.ts`）

每个 wiki 私有 `index.db`（SQLite）：`wiki_fts` (FTS5) + `page_meta` + `graph_edge`（图谱边表）。

---

## 八、CodeGraph 引擎

### 8.1 封装层（`engines/code/bridge.ts`）

封装 `@colbymchenry/codegraph` 核心 API：

- **索引操作**：`indexProject()` / `openIndex()` / `syncIndex()` / `closeIndex()`
- **工具执行**：`executeTool(toolName, params)` 复用 `ToolHandler` 格式化输出
- **平台包解析**：`resolveToolsPath()` 从 npm 主包解析平台包路径

### 8.2 MCP 12 工具集（`src/mcp/tools.ts`）

**Code-Graph (8)**：`code_search / code_explore / code_callers / code_callees / code_impact / code_node / code_status / code_files`

**Wiki (4)**：`wiki_search / wiki_read / wiki_list / wiki_graph`

**设计约束**：仅暴露查询类工具（只读），管理操作（create/delete/sync）不暴露给 MCP agent，由管控 UI 处理。

### 8.3 索引池（Instance Pool）

- **懒加载**：`instancePool.loadIfMissing()` 首次访问时打开索引
- **共享队列**：`BuildQueue` 串行 wiki + code 任务
- **Source Fetcher**：`SourceFetcherRegistry` 路由 git/local/ftp，含 SSRF 防护

### 8.4 MCP Server 传输

**代码路径**：`MemoryKnowledge/src/mcp/server.ts`

- **传输方式**：`StdioServerTransport`（独立进程）
- **工具转发**：`callApi()` 把 MCP tool call 转发到 Hono HTTP API
- **错误处理**：`{text, isError}` 透传

---

## 九、多租户隔离深度分析

### 9.1 三维隔离（`core/store/isolation.ts`）

```typescript
interface IsolationContext {
  teamId?: string;      // 可选业务维度
  userId: string;       // 必填
  agentId: string;      // 必填
  sessionId: string;    // 必填
  taskId?: string;      // 可选
  sessionKey?: string;  // 遗留聚合键
}
```

**强制校验**：`assertIsolation(ctx, config)` 缺失必填字段时：
- `legacyCompatMode=true` → 用 `legacyPlaceholder`（默认 `__legacy__`）填充
- `legacyCompatMode=false` → 用 `DEFAULT_ISOLATION_ID`（`"default"`）填充

**查询过滤**：`buildIsolationWhere(filter, tablePrefix)` 动态构建 WHERE 子句：

```typescript
// 输出示例："team_id = ? AND user_id = ? AND agent_id = ?"
return { clause: parts.join(" AND "), params };
```

**后置校验**：`rowMatchesIsolation(row, filter)` 在向量/FTC 召回后二次检查（safety net，因 TCVDB 旧版可能无法 push down filter）。

### 9.2 5 级可见性 + 6 类权限

**可见性**（`AssetVisibility`）：
- `private`：仅 Owner
- `team`：团队内全员
- `restricted`：ACL 精确控制（User/Role/Agent）
- `agent`：绑定特定 Agent
- `task`：绑定特定任务

**权限类型**（`Permission`）：`read / write / delete / assign / share / use`

**ACL 模型**：
```typescript
type AclSubjectType = "user" | "team_role" | "agent";
type AclEffect = "allow" | "deny";  // 一期仅 allow，deny 预留
```

**角色分层**：
- 全局层：`system_admin`
- 团队层：`admin / member / reviewer`
- 资产层：`owner` 自动拥有管理权限

### 9.3 ACL 检查（`metadata/service/permission-checker.ts`）

- `checkAssetPermission(user_key, asset_id, action)`
- `user-visibility.ts` 控制用户可见资产
- Proxy 层 `aclClient.aclCheck()` 每个 fixed-asset ctx 走读权限校验

---

## 十、MemoryProxy 代理深度分析

### 10.1 LLM 请求转发

**代码路径**：`MemoryProxy/src/handler.ts`

**核心流程**：
1. 解析 Bearer token / x-api-key / query key
2. 验证 userKey（`verifyUserKey()`）
3. 提取 `spaceId`（从 URL path `/{agent}/{spaceId}/...`）
4. 构建 per-request `TdaiClient`（`spaceId` 覆盖 `config.tdai.serviceId`）
5. 通过 InjectionPipeline 执行 hook
6. 转发到 upstream LLM
7. 解析 usage → 上报 credit + ClickHouse + Opik/Langfuse trace
8. 异步记录 TDAI turn（`recordTdaiTurn()`）

**消息扁平化**（`flattenMessagesForOpik()`）：把 Anthropic 风格多内容块（text/tool_use/thinking）展平为纯文本 role/content，适配 Opik 日志。

### 10.2 JSONL 请求日志

**代码路径**：`MemoryProxy/src/requestLog.ts`

- 原始请求/响应写入 JSONL
- 日志轮转配置（`rotate`）
- 失败即时上报 ClickHouse（`writeFailedReportRaw()`）

### 10.3 限流实现

**代码路径**：`MemoryProxy/src/rate-limit/guard.ts`

- `enforceRateLimit()` — TPM（tokens/min）/ QPM（queries/min）双限流
- `recordInputTokenUsage()` — 记录用量
- `isRateLimitExceededError()` — 识别限流错误

### 10.4 计费上报

**代码路径**：`MemoryProxy/src/credit-reporter.ts`

- `tryReportCreditFromPath()` — 从 URL 路径提取 spaceId 并上报
- `extractSpaceIdFromPath()` — 路径解析

### 10.5 可观测性四件套

| 系统 | 代码路径 | 用途 |
|------|---------|------|
| OpenTelemetry | `MemoryKnowledge/src/telemetry.ts` | OTLP HTTP/gRPC exporter |
| Langfuse | `MemoryProxy/src/langfuse.ts` | Trace 语义：1 trace = 1 turn（`sessionKey + turnSeq` → SHA-256 前 32 位 hex） |
| Opik | `MemoryProxy/src/opik.ts` | 独立 LLM Span（与 Langfuse 完全独立） |
| ClickHouse | `MemoryProxy/src/clickhouse.ts` | 结构化请求日志 + 失败上报 |

### 10.6 优雅关闭

```typescript
async function gracefulShutdown(signal) {
  // 1. 等待 L0 flush（最多 10s）
  await flushPendingWrites(10_000);
  // 2. 关闭 guard / langfuse / clickhouse / logger
  await shutdownGuard();
  await shutdownLangfuse();
  await shutdownClickHouse();
  await shutdownLogger();
}
```

---

## 十一、对 laew 的深度借鉴建议

### 11.1 P0 — 核心记忆能力（1-2 周）

#### 11.1.1 L0 对话录制 + L1 结构化记忆

**laew 现状**：`session_memory` 表仅存摘要，无分层提炼。

**借鉴点**：
- **JSONL 按日分片**（`conversations/YYYY-MM-DD.jsonl`）：append-only，grep/stream 友好
- **双保护增量捕获**：位置切片（`originalUserMessageCount`）+ 时间戳游标（`afterTimestamp`）
- **防污染**：缓存 `originalUserText` 替换被 `prependContext` 污染的 user 消息
- **L1 七种记忆类型**：`persona / episodic / instruction / work_fact / work_task / work_method / work_artifact`

**落地路径**：
```
src/agent/memory/
  l0-recorder.ts        ← JSONL 录制（参考 TencentDB l0-recorder.ts）
  l1-writer.ts          ← 结构化记忆双写（参考 TencentDB l1-writer.ts）
  l1-dedup.ts           ← LLM 去重决策（参考 TencentDB l1-dedup.ts prompt）
  prompts/
    l1-extraction.ts    ← 抽取 prompt
    l1-dedup.ts         ← 去重 prompt
```

**存储扩展**：

```sql
-- L1 记忆表
CREATE TABLE l1_memories (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  type TEXT NOT NULL,           -- persona / episodic / instruction / work_*
  priority INTEGER DEFAULT 50,  -- 0-100
  scene_name TEXT,
  source_message_ids TEXT,      -- JSON array
  metadata_json TEXT,           -- EpisodicMetadata
  timestamps TEXT,              -- JSON array
  session_key TEXT,
  session_id TEXT,
  agent_id TEXT,                -- laew 六角色之一
  user_id TEXT,
  team_id TEXT,
  task_id TEXT,
  version INTEGER DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- L1 向量索引（sqlite-vec）
CREATE VIRTUAL TABLE l1_vec USING vec0(
  record_id TEXT PRIMARY KEY,
  embedding FLOAT[1536]
);

-- L1 全文索引
CREATE VIRTUAL TABLE l1_fts USING fts5(
  content, content='l1_memories', content_rowid='rowid'
);
```

#### 11.1.2 RRF 混合检索

**laew 现状**：无记忆检索，仅 SessionContext 注入最近 N 条摘要。

**借鉴点**：
- **FTS5 BM25（关键词）+ vec0 向量（语义）+ RRF 融合**（k=60）
- **中文 jieba 分词 + 停用词过滤**（`ZH_STOP_WORDS`）
- **BM25 rank → score sigmoid 映射**：`1 / (1 + Math.exp(-rank))`

**落地路径**：

```typescript
// src/agent/memory/rrf.ts
export const RRF_K = 60;
export function rrfMerge<T>(lists: T[][], getId: (item: T) => string, k = RRF_K) { /* ... */ }

// src/agent/memory/search.ts
export async function hybridSearch(query: string, agentId: string) {
  const ftsResults = await ftsSearch(query, agentId);  // FTS5 BM25
  const vecResults = await vecSearch(query, agentId);  // vec0 cosine
  return rrfMerge([ftsResults, vecResults], (r) => r.id);
}
```

**召回注入**：在 system prompt 前追加 `<<<LAEW:RECALLED_MEMORIES>>>` 标记包裹的召回结果，参考 TencentDB 的 `<tdai_recalled_l1_memories>` 格式。

#### 11.1.3 Checkpoint 原子操作

**借鉴点**：
- **Per-file async lock**（Promise 链序列化）
- **Atomic write**（tmp + rename）
- **Split-state 设计**（runner_states / pipeline_states 隔离）

**落地路径**：在 `src/agent/memory/checkpoint.ts` 实现 `CheckpointManager`，为 laew 的 `MultiAgentOrchestrator` 提供跨会话状态持久化。

### 11.2 P1 — 进阶能力（2-4 周）

#### 11.2.1 L2 场景块 + MemoryPipelineManager

**借鉴点**：
- **LLM 沙箱限制 workspaceDir**：`scene_blocks/` 目录隔离
- **场景格式**：frontmatter（summary / heat / created / updated）+ 正文
- **容量控制**：`maxScenes` 上限 + 强制 MERGE 警告
- **Pipeline 调度**：warm-up 模式 + 向下_only_ L2 timer + L3 全局互斥
- **Session GC**：`gcStaleSessions()` 淘汰冷 session

**落地路径**：
```
src/agent/memory/
  scene-extractor.ts    ← LLM 场景抽取（参考 TencentDB scene-extractor.ts）
  scene-index.ts        ← JSON 索引维护
  scene-format.ts       ← frontmatter 解析
  scene-navigation.ts   ← 导航树生成
  pipeline-manager.ts   ← L1→L2→L3 调度
```

#### 11.2.2 Skill 版本化系统

**laew 现状**：无 Skill 系统。

**借鉴点**：
- **SKILL.md 格式**：YAML frontmatter + Markdown body
- **单表多行多版本**：`(skill_id, version)` + `is_head`
- **跨系统事务**：COS + DB + asset 三系统"顺序 + 补偿"
- **乐观锁**：`expected_version` 防并发覆盖
- **ID 生成**：CSPRNG base62 12 字符（~71 bit 真熵）

**落地路径**：
```
src/agent/skill/
  skill-core.ts         ← 6 写 4 读门面（参考 TencentDB skill-core.ts）
  skill-versioning.ts   ← 跨系统事务编排
  skill-format.ts       ← frontmatter 解析校验
  skill-store.ts        ← SQLite 实现
  skill-permission.ts   ← assertOwner/assertTeamMatch/assertVersionFresh
  prompts/
    skill-extraction.ts ← LLM 抽取 prompt
```

**SQLite 表**：
```sql
CREATE TABLE skills (
  skill_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  is_head INTEGER DEFAULT 1,
  name TEXT NOT NULL,
  description TEXT,
  content TEXT NOT NULL,
  content_hash TEXT,
  manifest_json TEXT,
  storage_dir TEXT,
  owner_agent_id TEXT,
  team_id TEXT,
  user_id TEXT,
  task_id TEXT,
  metadata_json TEXT,
  status TEXT DEFAULT 'active',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (skill_id, version)
);
CREATE VIRTUAL TABLE skill_fts USING fts5(name, description, content);
```

#### 11.2.3 上下文注入管线

**laew 现状**：`Yolo 项目上下文注入` 是简单一次性注入（`<<<LAEW:PROJECT_CONTEXT>>>`）。

**借鉴点**：
- **Hook 注册机制**：按 injection point 注册多个 hook（8 个内置点）
- **协议适配器**：`OpenAIAdapter` / `AnthropicAdapter` 双向转换
- **缓存策略三态**：`none / session_init / hybrid`
- **Prewarm 预热**：session 初始化时预热 + self-heal cache miss

**落地路径**：
```
src/agent/injection/
  pipeline.ts           ← InjectionPipeline 主流程
  registry.ts           ← Hook 注册表
  adapters/
    openai.ts           ← OpenAI 协议适配
    anthropic.ts        ← Anthropic 协议适配
  hooks/
    project-context.ts  ← 项目上下文注入（现有）
    memory-recall.ts    ← L1 记忆召回
    skill-injector.ts   ← Skill 列表注入
  types.ts              ← InjectionHook / AgentContext / ContextBlock
```

### 11.3 P2 — 高级能力（1-2 月）

#### 11.3.1 L3 画像 + MemoryProxy 代理

**借鉴点**：
- **增量 Persona 生成**：仅分析变化场景，降低 token 消耗
- **BackupManager**：保留最近 N 份画像备份
- **场景导航**：`generateSceneNavigation()` 注入 persona.md
- **MemoryProxy 架构**：LLM 请求拦截 → 注入 → 转发 → 计费

**落地路径**：
```
src/agent/memory/
  persona-generator.ts  ← L3 画像生成（参考 TencentDB persona-generator.ts）
  persona-trigger.ts    ← 触发条件判断
  backup-manager.ts     ← 文件备份管理
```

#### 11.3.2 Wiki + CodeGraph 知识引擎

**借鉴点**：
- **Ingest V2 两阶段 LLM 摄取**：先分析后生成
- **Graph 多跳检索**：BFS + decay + maxNodes 防护
- **路径规范化**：`canonicalizePagePath()` 保证 dedup 不变量
- **MCP 12 工具集**：8 code-graph + 4 wiki

**落地路径**：在 laew 中以独立 crate 实现，暴露 MCP stdio server。

#### 11.3.3 多租户隔离

**laew 现状**：单用户单设备。

**借鉴点**：
- **三维隔离**：`(team_id, user_id, agent_id)`
- **assertIsolation() 必填校验**
- **Legacy 兼容模式**：`legacyCompatMode` 占位符填充
- **5 级可见性 + 6 类权限**

**落地路径**：当前可简化为 `(user_id, agent_id)` 二维，在 `Session` 中增加 `agentId` 字段，记忆查询强制带 agentId 过滤。

### 11.4 反模式警示

1. **不要过度设计**：TencentDB 的 6 个子系统（Core/Knowledge/Panel/Proxy + 双 SDK）对 laew 过重。建议先做 **L1 记忆 + RRF 检索 + Checkpoint 原子操作**三个核心能力。
2. **警惕 LLM 沙箱逃逸**：TencentDB 的"LLM 直写文件"模式需要严格的 workspaceDir 限制。laew 当前零沙箱，引入 L2 场景块时需同步建设权限管控（参考 `专题-沙箱设计深度分析.md`）。
3. **向量维度漂移检测**：embedding provider/model/dimensions 变化时需 reindex。借鉴 TencentDB 的 `VectorStoreInitResult.needsReindex` 检测机制。
4. **防止记忆注入污染**：TencentDB 用 `sanitizeText` + `stripCodeBlocks` 防止 `<<<LAEW:PROJECT_CONTEXT>>>` 被 LLM 重复注入。laew 现有标记也需类似处理。
5. **避免跨系统事务**：laew 当前只有 SQLite，不要为了实现 Skill 版本化引入 COS + asset 等多系统。建议 Skill v1 仅用 SQLite 单库 + 乐观锁。

### 11.5 推荐落地路线图

```
P0（1-2 周）：
  ├── L0 对话录制（JSONL 按日分片 + 双保护增量捕获）
  ├── L1 记忆表（SQLite + vec0 + FTS5）
  ├── RRF 混合检索（k=60 + jieba 分词）
  └── Checkpoint 原子操作（tmp+rename + per-file lock）

P1（2-4 周）：
  ├── LLM 抽取 prompt（l1-extraction.ts）
  ├── MemoryPipelineManager 简化版（L1→L2 调度）
  ├── L2 场景块（LLM 沙箱 + scene_blocks/）
  ├── Skill 系统 v1（SQLite + 乐观锁）
  └── 上下文注入管线（4 个核心注入点）

P2（1-2 月）：
  ├── L3 画像生成（增量模式）
  ├── MemoryProxy 简化版（请求拦截 + 注入）
  ├── Wiki 摄取引擎（Ingest V2 两阶段）
  └── 多租户隔离（二维简化版）

P3（2-3 月）：
  ├── CodeGraph 引擎
  ├── MCP 服务（12 工具集）
  └── 可观测性（OpenTelemetry + Langfuse）
```

---

## 十二、总结

TencentDB Agent Memory 是**工程完整度高、设计模式成熟**的团队记忆系统。其核心设计亮点包括：

1. **L0-L3 四层提炼管线**：从 JSONL 原始对话到 Persona 画像的渐进式抽象，每层都有明确的触发条件和数据流
2. **BM25 + 向量 + RRF 混合检索**：关键词与语义互补融合，k=60 标准 RRF 常数，jieba 中文分词
3. **MemoryPipelineManager 级联调度**：warm-up 模式 + 向下_only_ timer + L3 全局互斥 + Session GC
4. **Skill 版本化跨系统事务**：COS + DB + asset 三系统"顺序 + 补偿"伪事务，乐观锁并发控制
5. **上下文注入管线**：8 个注入点 + 三态缓存策略 + prewarm self-heal
6. **Checkpoint 原子操作**：per-file async lock + tmp+rename + split-state 设计
7. **多租户三维隔离**：`(team_id, user_id, agent_id)` + 5 级可见性 + 6 类权限
8. **Wiki 知识引擎**：Ingest V2 两阶段 LLM 摄取 + graphology 图谱多跳检索
9. **MemoryProxy 可观测性**：OpenTelemetry + Langfuse + Opik + ClickHouse 四件套

对 laew 最具借鉴价值的三个方向：
- **L1 结构化记忆 + RRF 混合检索**：解决"每次新 Session 从零开始"痛点
- **Checkpoint 原子操作**：为 `MultiAgentOrchestrator` 提供跨会话状态持久化
- **上下文注入管线**：从一次性注入升级为可扩展的 Hook 注册机制

落地时应遵循**"先核心后外围"**原则，优先实现 L0-L1 录制 + RRF 检索 + Checkpoint 三个 P0 能力，再逐步扩展 L2-L3 和 Skill 系统。

---

*分析人：Claude Code Agent*
*分析日期：2026-09-05*
*源码路径：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`*
*关联文档：`TencentDB-Agent-Memory-源码调研.md`*
