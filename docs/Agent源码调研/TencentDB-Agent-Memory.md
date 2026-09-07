# TencentDB Agent Memory 综合深度分析

> 调研对象:TencentDB-Agent-Memory(TypeScript+Python,团队记忆系统)
> 调研日期:2026-09-05
> 原始文档:3 份(源码调研 938 行 + 深度分析 977 行 + 核心机制深度分析 1893 行,合计 3808 行)
> 总行数:~1,850 行(合并后)

---

## 一、项目元信息

### 1.1 工程定位

面向 Agent 团队的"团队记忆系统"，核心理念 "Agents remember. Humans innovate."。将项目中已有的信息转化为可复用的"记忆资产"，支持在多个 Agent 和团队成员之间流动、共享和版本化管理。

- **调研对象**：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`（v2.0.0-beta.1，2026-08 快照）
- **代码规模**：TypeScript 主导的多仓工程，MemoryCore 单仓即 ~10 万行级；MemoryProxy 约 5000 行级；MemoryKnowledge 约 3000 行级；MemoryPanel 约 3000 行级；双 SDK 各约 2000 行级

### 1.2 顶层目录布局

```
TencentDB-Agent-Memory/
├── MemoryCore/     # 记忆内核（L0-L3 引擎 + Skill + Gateway 服务）
├── MemoryKnowledge/# 知识服务（Wiki + CodeGraph 引擎）
├── MemoryPanel/    # 团队管控平台（Team/User/Agent/Asset 管理）
├── MemoryProxy/    # 上下文注入代理（LLM 请求拦截 + 注入）
├── sdk/            # 双 SDK（TypeScript + Python）
│   ├── memory-core/        # 记忆核心 SDK
│   └── memory-tencentdb/   # 腾讯云 DB SDK
├── deploy/         # 一键部署脚本与 Docker Compose
└── docs/           # 设计文档
```

### 1.3 构建系统与工具链

- **包管理**：pnpm workspace
- **构建工具**：`tsdown`（基于 Rolldown 的 TS 打包器）+ `tsx` + `tsc`
- **测试框架**：vitest + 独立 e2e 脚本
- **代码生成**：`@kubb/cli` 从 OpenAPI 生成 TypeScript/SDK 类型
- **运行时要求**：Node.js ≥ 22.16（使用 `node:sqlite` 内置模块 + `sqlite-vec` 扩展）
- **镜像发布**：Docker Hub 公开镜像 `agentmemory/memory-core` / `memory-hub` / `memory-proxy`

### 1.4 子仓职责边界

| 子仓 | 入口文件 | 核心职责 |
|------|---------|---------|
| MemoryCore | `index.ts` + `src/gateway/server.ts` | L0-L3 记忆引擎、Skill 系统、向量/BM25 存储、OpenClaw/Hermes 双宿主适配 |
| MemoryKnowledge | `src/server.ts` + `src/mcp/server.ts` | Wiki 摄取/图谱、CodeGraph 索引、知识检索 |
| MemoryPanel | `src/index.ts` | Team/User/Agent/Task 元数据管控、资产治理、可见性 ACL |
| MemoryProxy | `src/index.ts` | LLM 请求转发、上下文注入管线、Session 初始化、可观测性 |

### 1.5 模块间通信协议

- **MemoryCore ↔ Agent 宿主**：OpenClaw 走 in-process 插件 API（`OpenClawHostAdapter`）；Hermes 走 HTTP（`StandaloneHostAdapter` + `MemoryTencentdbSdkClient` Python 端）
- **MemoryProxy ↔ MemoryCore**：HTTP `/v3/*` 协议（`TdaiClient` 封装）
- **MemoryPanel ↔ MemoryCore**：HTTP `/v3/meta/*` 元数据路由
- **MemoryKnowledge ↔ Agent**：MCP stdio + HTTP `/v3/tools/*`
- **MemoryProxy ↔ MemoryKnowledge**：HTTP `/v3/wiki/*` + `/v3/code-graph/*`

### 1.6 核心门面类：TdaiCore

`MemoryCore/src/core/tdai-core.ts`（1205 行）是 Host-neutral 的统一门面：

```typescript
export class TdaiCore {
  private hostAdapter: HostAdapter;        // OpenClaw / Standalone
  private vectorStore?: IMemoryStore;     // SQLite / TCVDB
  private embeddingService?: EmbeddingService;
  private scheduler?: MemoryPipelineManager; // L1→L2→L3 调度
  private skillCore?: SkillCore;
  private skillExtractor?: SkillExtractor;

  async initialize(): Promise<void>
  async handleBeforeRecall(userText, sessionKey): Promise<RecallResult>
  async handleTurnCommitted(messages, ...): Promise<CaptureResult>
  async handleMemorySearch(params): Promise<...>
}
```

关键设计：
- **HostAdapter 抽象**：隔离 OpenClaw / Hermes / Gateway 三种宿主
- **Promise gate 并发保护**：`schedulerStartPromise` 防止并发启动
- **Skill 生命周期钩子**：`SkillAssetHooks` 把 create/access/archive 同步到上层 asset 注册表

---

## 二、L0-L3 管线（Chat→Atom→Scenario→Persona）

### 2.1 四层提炼架构

```
L0 Chat（原始对话）
   │  agent_end hook → l0-recorder.ts (608 行)
   ▼
L1 Atom（结构化记忆）persona / episodic / instruction / work_*
   │  LLM 提取 + 向量去重 → l1-writer.ts (365 行)
   ▼
L2 Scenario（场景块）scene_blocks/*.md
   │  LLM 场景抽取 → scene-extractor.ts (598 行)
   ▼
L3 Persona（用户画像）persona.md
   │  LLM 全局综合 → persona-generator.ts (298 行)
```

### 2.2 L0 Recorder（`l0-recorder.ts` 608 行）

**入口函数**：`recordConversation(params)` — 接收 `agent_end` hook 投递的全量 `rawMessages`，执行四步处理：

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

设计意图：位置切片免疫重启后时间戳漂移，时间戳游标作为缓存失效时的 fallback。**安全阀**：当位置切片不可用且时间戳过滤全量通过（>8 条）时打 warn。

**Step 2 — 污染消息替换**：框架在 `before_prompt_build` 之后给 user 消息追加 `prependContext`，通过缓存的 `originalUserText` + timestamp 定位并替换回干净版本。

**Step 3 — 消毒过滤**：`sanitizeText()` 剥离注入标记防止反馈循环；`stripCodeBlocks()` 从 assistant 回复中移除围栏代码块（降低 embedding 噪声）；`shouldCaptureL0()` 过滤太短/无意义消息。

**Step 4 — 幂等写入 JSONL**：每条消息独立一行 `L0MessageRecord`，包含 `sessionKey/sessionId/userId/agentId/recordedAt/id/role/content/timestamp` 九字段。append-only，**同一批次要么全写要么全跳过**。

**读取接口**：
- `readConversationRecords(sessionKey, baseDir)` — 按 sessionKey 过滤逐日 JSONL
- `readConversationMessages(sessionKey, baseDir, afterTimestamp?, limit?)` — 支持游标+限流
- `readConversationMessagesGroupedBySessionId(...)` — 按 sessionId 分组

### 2.3 L1 Writer（`l1-writer.ts` 365 行）

**核心数据模型**：

```typescript
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
  version?: number;
  sessionKey: string;
  sessionId: string;
  teamId?: string;
  userId?: string;
  agentId?: string;
}

export interface DedupDecision {
  record_id: string;
  action: "store" | "update" | "merge" | "skip";
  target_ids: string[];
  merged_content?: string;
  merged_type?: MemoryType;
  merged_priority?: number;
  merged_timestamps?: string[];
}
```

**四态决策路径**：

| action | 行为 | 向量存储 |
|--------|------|---------|
| `store` | 追加新记录 | `upsertL1(record, embedding)` |
| `update` | 删除 target + 写新记录 | 先 `deleteL1Batch` 再 `upsertL1` |
| `merge` | 删除多 target + 写合并记录 | 同上 |
| `skip` | 什么都不做 | 无操作 |

**版本递增逻辑**：update/merge 时先查询 target 最大版本号 `maxVersion`，新版本 = `maxVersion + 1`。

**vec dual-write 流程**：
1. JSONL 追加先行（source of truth，backup/recovery 用）
2. 异步调用 `embeddingService.embed()` 获取向量
3. 调用 `vectorStore.upsertL1(record, embedding)` 写入 vec0
4. Embedding 失败时仅写 metadata + FTS，跳过 vec0（graceful degradation）

**去重决策**（`DedupDecision`）由独立 LLM 抽取 prompt（`core/prompts/l1-dedup.ts`）产生，包含 `target_ids`（多目标合并）、`merged_content/merged_type/merged_priority/merged_timestamps` 合并产物字段。

### 2.4 L2 Scene Extractor（`scene-extractor.ts` 598 行）

**安全沙箱**：LLM 的 `workspaceDir` 设为 `scene_blocks/`，仅能操作 .md 场景文件；`scene-index.json` 和 `persona.md` 物理不可见。

**八阶段流水线**（`extract()` line 135-510）：

1. **Phase 1 备份**：`BackupManager.backupDirectory()` 快照整个 `scene_blocks/`，LLM 失败时自动还原（fail-soft：还原失败不掩盖原始 LLM 错误）
2. **Phase 2 加载索引**：`readSceneIndex()` 读取已有场景清单，构建摘要 + 容量计数器（`当前场景总数：N / maxScenes`）+ 场景数警告（≥上限强制 MERGE、=上限-1 禁止 CREATE）
3. **Phase 3 构建 prompt**：`buildSceneExtractionPrompt()` 注入记忆 JSON + 场景摘要 + 警告
4. **Phase 4 LLM 执行**：`CleanContextRunner.run()` 带工具调用，timeout=300s
5. **Phase 5 清理软删除**：LLM 用 `[DELETED]` 标记"删除"文件（无 exec 工具），此阶段真实 unlink
6. **Phase 5b 文件名规范化**：`normalizeSceneFilenames()` 修正 LLM 偶尔产生的非法文件名（空格/标点）
7. **Phase 6 同步索引**：`syncSceneIndex()` 从磁盘重建 JSON 索引
8. **Phase 7 更新导航**：`updateSceneNavigation()` 更新 persona.md 末尾的导航树

**容量控制**（line 184-194）：在 prompt 头部嵌入容量计数器 + 三档警告（强制 MERGE / 禁止 CREATE / 建议 UPDATE），LLM 通过 prompt 自觉控制场景数量。

**Persona 更新信号**：LLM 文本输出中的 `[PERSONA_UPDATE_REQUEST]reason[/PERSONA_UPDATE_REQUEST]` 被 `parsePersonaUpdateSignal()` 解析，支持 block + inline 两种格式，写入 checkpoint 的 `request_persona_update` 字段，由后续 L3 处理。

**空抽取检测**：`preExtractIndex.size === 0 && postIndex.length === 0` 判定 LLM 未产出任何文件，触发告警。

### 2.5 L3 Persona Generator（`persona-generator.ts` 298 行）

**触发条件**：
- 每 N 条新记忆（默认 50，由 `memories_since_last_persona` 计数）
- 场景变化（L2 完成触发 L3）
- `[PERSONA_UPDATE_REQUEST]` 信号

**十一阶段生成流程**（line 74-282）：

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

**增量模式关键**：`changedScenesContent` 仅包含变化场景，prompt 提示"重点分析变化场景"，未变化场景不重读，显著降低 token 消耗（实测减少 60-80% 的 prompt token）。

---

## 三、MemoryPipelineManager

### 3.1 整体架构（`pipeline-manager.ts` 1218 行）

三层 SerialQueue（`l1Queue/l2Queue/l3Queue`）+ 每 session 双 Timer（`l1Idle/l2Schedule`）+ 消息缓冲区。

```typescript
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

### 3.2 Warm-up 指数阈值判断

**核心算法**：

```typescript
private getEffectiveThreshold(state: PipelineSessionState): number {
  if (!this.enableWarmup) return this.everyNConversations;
  if (state.warmup_threshold <= 0) return this.everyNConversations;
  return Math.min(state.warmup_threshold, this.everyNConversations);
}

private advanceWarmupThreshold(state: PipelineSessionState): void {
  if (!this.enableWarmup) return;
  if (state.warmup_threshold <= 0) return; // already graduated

  const next = state.warmup_threshold * 2;
  if (next >= this.everyNConversations) {
    state.warmup_threshold = 0;  // Graduated
  } else {
    state.warmup_threshold = next;
  }
}
```

**Warm-up 算法**：`warmup_threshold` 从 1 开始指数增长（1→2→4→8→...→`everyNConversations`）。保证早期对话被快速处理（第 1 轮就触发 L1），随会话成熟逐步降低频率。

### 3.3 L1 触发三路径

- **Path A (threshold)**：`conversation_count >= effectiveThreshold`，立即 `enqueueL1()`
- **Path B (idle_timeout)**：用户停止对话 `l1IdleTimeoutSeconds` 后 `onL1IdleTimeout()` 触发
- **Path C (flush)**：graceful shutdown 或 `flushSession()` 时排空缓冲

### 3.4 L2 向下_only_ Timer 算法

```typescript
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
  }
}
```

设计要点：
- `delayAfterL1` 让远程 L1 完成异步记录生成
- `minInterval` 防止 L2 过于频繁
- `maxInterval` 由 `armL2MaxInterval()` 在 L2 完成后无条件设置（`now + l2MaxInterval`）
- **向下_only_**：调度时间只能向前推，不能向后延

### 3.5 L3 全局互斥 + pending 去重

```typescript
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

### 3.6 Checkpoint 原子操作（`checkpoint.ts` 510 行）

**Split-state 设计**：`runner_states` 与 `pipeline_states` 两个命名空间物理隔离，防止 PipelineManager 的 `persistStates()` 覆盖 L0/L1 runner 写入的游标字段（split-brain 问题）。

```typescript
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

**Per-file async lock**：`withFileLock(filePath, fn)` 用 Promise 链序列化同一文件的并发 read-modify-write。多个 `CheckpointManager` 实例共享同文件路径时自动共享锁。

**Atomic write**：`writeRaw()` 先写 `tmp` 文件再 `rename()` 到目标路径，防止崩溃时文件损坏。

**核心 API**：
- `captureAtomically(sessionKey, pluginStartTimestamp, fn)` — 在单锁内读游标 → 执行捕获 → 推进游标
- `markL1ExtractionComplete(sessionKey, memoriesExtracted, cursorRecordedAtMs, lastSceneName)` — L1 完成后更新
- `markPersonaGenerated(totalProcessed)` — L3 完成后重置计数
- `mergePipelineStates(states)` — PipelineManager 独占写入 `pipeline_states`

### 3.7 Session GC + 失败恢复

**Session GC**：每 50 次 `notifyConversation()` 调用触发 `gcStaleSessions()`，淘汰 inactive > `sessionActiveWindowMs * 3` 且无排队任务/缓冲消息的冷 session，防止内存无限增长。

**失败恢复**：
- L1 失败：消息放回缓冲区 + `l1RetryCount` 递增，30s 后重试（最多 5 次）
- L2 失败：`armL2MaxInterval()` 保证最终重试
- 全部失败：`persistStates()` 持久化到 checkpoint，`recoverPendingSessions()` 在下次启动时恢复

### 3.8 Timer Scanner

- **定时清理**：`LocalMemoryCleaner` 按 `l0l1RetentionDays` 清理过期 L0/L1 文件
- **执行时间**：可配（默认 03:00）
- **Skill TTL**：`SkillVersioning` 支持旧版本 TTL 过期

---

## 四、SkillCore 6 写 4 读

### 4.1 Skill 生命周期

```
Extract（抽取）→ Review（审核）→ Create/Update/Patch（版本化）
     → Archive（归档）→ Share（跨 Agent 共享）
```

### 4.2 SkillCore 门面（`skill-core.ts` 661 行）

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

**ID 生成算法**：`skl-` + 12 字符 base62（CSPRNG，~71 bit 真熵），总长 16 字符。单实例 100 万 skill 碰撞概率 ~1.5e-10。Collision 时重试最多 3 次，超限抛 `SKILL_ID_COLLISION`。

### 4.3 乐观锁实现的版本检查（`skill-permission.ts` 69 行）

```typescript
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
1. **Owner 校验**：(teamId, agentId) 二元组唯一确定 ownership
2. **Team mismatch 伪装 NOT_FOUND**：抗存在性侧信道
3. **乐观锁**：`expected_version` 必传，必须与 head.version 完全一致

### 4.4 SkillVersioning 跨系统事务（`skill-versioning.ts` 435 行）

**跨系统事务编排（COS + skill DB + meta_assets 三系统）**：

```
createNewSkill():
  Step 1: writeResource → COS            ← 最脆先做，失败零副作用
  Step 2: store.appendVersion → skill DB ← 失败反向清 COS (cleanupVersionDir)
  Step 3: onSkillCreated → meta_assets    ← 失败反向删 DB + 清 COS
```

**关键注释**（skill-versioning.ts:107-110）：
> "为什么这个顺序：跨 3 个系统没有真事务，只能靠"顺序 + 补偿"。原则是**最容易失败的先做、失败零副作用的先做、可靠的收尾**。COS 是最脆的（网络/认证/权限），skill DB 是本地事务几乎不会失败，asset 涉及 agent 查询/team 校验/多表写但也是本地 DB。原实现"asset 先写"违背了这个原则：曾出现过 COS 认证挂 → skill 没落库 → asset 表却已经有一行的孤儿状态。"

**极端情况兜底**：
- **孤儿 skill（skill 落库但 asset 缺）**：由 `onSkillAccessed` 读时自愈补登记
- **孤儿 COS 文件**：只占空间，读路径全部过 DB，永远不会被误读到

**幂等保证**：`appendNextVersion()` 检测 `newContentHash === head.content_hash` 且无资源变更时直接返回 head（无操作），避免无效版本。

**TTL 清理**：`cleanupExpiredVersionsForSkill(skill_id, versionTtlSeconds)` 异步 fire-and-forget，清理超期旧版本。

### 4.5 Skill 文件格式（`skill-format.ts`）

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

### 4.6 Skill Extractor（`skill-extractor.ts`）

- **LLM 驱动抽取**：把对话 transcript 喂给 LLM，通过 `skill_create / skill_update / skill_patch` 工具持久化
- **Head-tail 截断**：`headChars=8000 + tailChars=32000` 适配 LLM 上下文窗口
- **Recent Skills 注入**：`buildRecentSkillsBlock()` 把已有 skill 作为上下文注入抽取 prompt
- **主 Agent 提示**：`reason` 字段可注入主 Agent 意图说明

### 4.7 Skill 共享机制

- **可见性**：`private`（owner 私有）/ `team`（团队共享）/ `restricted`（ACL 精确控制）
- **跨 Agent 借入**：`ChatMemoryAgentRel` 支持借入 ≤2 个 agent 的记忆
- **Asset 注册表**：`meta_assets` + `meta_agent_fixed_assets` 两张元数据表

### 4.8 错误码体系（14 种）

```typescript
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

---

## 五、InjectionPipeline 8 注入点

### 5.1 主流程（`MemoryProxy/src/injection/pipeline.ts`）

```
raw body → Adapter.parse() → AgentContext → execute hooks → Adapter.serialize() → modified body
```

**协议适配器**：`OpenAIAdapter` / `AnthropicAdapter` 双向转换，由 `metadata.protocol` 路由。

**Agent 识别双通道**：
1. **Fast path**：URL path 前缀（如 `/claude-code/...`）→ `agentProfiles.get(metadata.agentSource)` 零成本查找
2. **Legacy fallback**：扫描 system prompt 文本匹配（deprecated）

### 5.2 9 个内置注入点（执行顺序）

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

### 5.3 三态缓存策略

```typescript
type CacheStrategy = "none" | "session_init" | "hybrid";
```

- **`none`**：每次执行 `hook.execute(ctx)`（legacy 行为）
- **`session_init`**：session 初始化时预热一次，后续从 `HookCacheRepo` 读取（keyed by `spaceId + userId + agentSource + sessionId + hookId`）。**Self-heal**：cache miss 时回退到 `execute()` 并回填缓存
- **`hybrid`**：预热 + 实时执行并集去重（按 `metadata.cacheKey ?? content`）

**Self-heal 例外**：`metadata.readOnly === true` (FORK 请求) 时 cache miss 不 self-heal put。

### 5.4 内置 Injector 清单

| Injector | 注入点 | 功能 |
|---------|-------|------|
| `SkillInjector` | system.before_tools | `<available_skills>` 自有 skill 列表 |
| `SkillToolsInjector` | system.before_tools | `<skill_tools>` curl 调用指南 |
| `TdaiL1RecallInjector` | user.before | 自有 + 借入 L1 记忆召回 |
| `TdaiProfileMemoryInjector` | system.prefix | L3 Persona + L2 场景导航 |
| `TdaiToolsInjector` | system.after_tools | `tdai_memory_search` 等工具指南 |
| `KnowledgeToolsInjector` | system.after_tools | Wiki/CodeGraph 工具指南 |
| `AssetReflectionInjector` | system.suffix | `<asset_reflection>` 内部效果评估 |

### 5.5 L1 召回注入器（`tdai-l1-recall-injector.ts`）

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

**ACL 校验**：`aclClient` 参数对每个 fixed-asset ctx 走 `acl/check(read)` 过滤。**降级策略**：控制面不可达时仅查当前 agent 的 L1。

### 5.6 Session 上下文注入（`session/context-injector.ts`）

- **Agent/Task 身份**：`<session_context>` 标记包裹，注入到每个请求
- **去重保证**：per-session dedup 防止重复注入
- **与 injection 管线分离**：session 身份是必选项，不走可选的 hook 管线

---

## 六、MemoryProxy

### 6.1 LLM 请求转发（`MemoryProxy/src/handler.ts`）

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

### 6.2 JSONL 请求日志（`MemoryProxy/src/requestLog.ts`）

- 原始请求/响应写入 JSONL
- 日志轮转配置（`rotate`）
- 失败即时上报 ClickHouse（`writeFailedReportRaw()`）

### 6.3 限流实现（`MemoryProxy/src/rate-limit/guard.ts`）

- `enforceRateLimit()` — TPM（tokens/min）/ QPM（queries/min）双限流
- `recordInputTokenUsage()` — 记录用量
- `isRateLimitExceededError()` — 识别限流错误

### 6.4 计费上报（`MemoryProxy/src/credit-reporter.ts`）

- `tryReportCreditFromPath()` — 从 URL 路径提取 spaceId 并上报
- `extractSpaceIdFromPath()` — 路径解析

### 6.5 可观测性四件套

| 系统 | 代码路径 | 用途 |
|------|---------|------|
| OpenTelemetry | `MemoryKnowledge/src/telemetry.ts` | OTLP HTTP/gRPC exporter |
| Langfuse | `MemoryProxy/src/langfuse.ts` | Trace 语义：1 trace = 1 turn（`sessionKey + turnSeq` → SHA-256 前 32 位 hex） |
| Opik | `MemoryProxy/src/opik.ts` | 独立 LLM Span（与 Langfuse 完全独立） |
| ClickHouse | `MemoryProxy/src/clickhouse.ts` | 结构化请求日志 + 失败上报 |

### 6.6 优雅关闭

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

## 七、多租户隔离

### 7.1 三维隔离数据模型（`isolation.ts` 171 行）

```typescript
export interface IsolationContext {
  teamId?: string;      // 可选业务维度
  userId: string;       // 必填
  agentId: string;      // 必填
  sessionId: string;    // 必填
  taskId?: string;      // 可选
  sessionKey?: string;  // 遗留聚合键
}

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
```

**强制校验**：`assertIsolation()` 缺失必填字段时：
- `legacyCompatMode=true` → 用 `legacyPlaceholder`（默认 `__legacy__`）填充
- `legacyCompatMode=false` → 用 `DEFAULT_ISOLATION_ID`（`"default"`）填充

### 7.3 buildIsolationWhere 动态构建 WHERE 子句

```typescript
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
  // ... agentId / sessionId / taskId / sessionKey 类似
  return { clause: parts.join(" AND "), params };
}
```

设计：未设置的维度自动跳过（`undefined` 检测），与 SQL `WHERE x = ?` 缺失参数约定一致。`tablePrefix` 支持多表 join。

### 7.4 rowMatchesIsolation 后置校验

```typescript
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

**角色分层**：
- 全局层：`system_admin`
- 团队层：`admin / member / reviewer`
- 资产层：`owner` 自动拥有管理权限

### 7.6 SQLite 三维复合索引

```sql
CREATE INDEX IF NOT EXISTS idx_l1_user_agent_session ON l1_records(user_id, agent_id, session_id);
CREATE INDEX IF NOT EXISTS idx_l1_user_updated ON l1_records(user_id, updated_time);
CREATE INDEX IF NOT EXISTS idx_l1_agent_updated ON l1_records(agent_id, updated_time);
```

三个复合索引支撑三维隔离查询：`(user_id, agent_id, session_id)` 精确匹配 + `(user_id, updated_time)` / `(agent_id, updated_time)` 时间范围扫描。

---

## 八、RRF 混合检索

### 8.1 混合检索架构

```
用户查询
   ├─ FTS5 BM25（关键词）──┐
   │                       ├─ RRF 融合 → 排序截断
   └─ vec0 向量（语义）────┘
```

### 8.2 RRF 融合算法（`search-utils.ts` 62 行）

```typescript
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
2. 每个 item 计算 `score = 1 / (k + rank + 1)`
3. 累加同 id item 的分数（跨列表去重合并）
4. 按 RRF score 降序排序

### 8.3 BM25 评分函数（`sqlite.ts` 299-306）

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

### 8.4 FTS5 中文分词（`sqlite.ts` 175-275）

**中文停用词**：

```typescript
const ZH_STOP_WORDS = new Set([
  "的", "了", "在", "是", "我", "有", "和", "就", "不", "人", "都", "一",
  "一个", "上", "也", "很", "到", "说", "要", "去", "你", "会", "着",
  "没有", "看", "好", "自己", "这", "他", "她", "它", "们", "那",
  "吗", "吧", "呢", "啊", "呀", "哦", "嗯",
]);
```

36 个高频虚词过滤。

**查询侧 `buildFtsQuery()`**：

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

**索引侧 `tokenizeForFts()`**：

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

### 8.5 向量相似度计算（`sqlite.ts` 716-723）

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

### 8.6 Embedding 服务（`embedding.ts`）

- **多 Provider**：OpenAI-compatible（`@ai-sdk/openai`）
- **代理支持**：`proxyUrl` 本地代理转发
- **维度适配**：支持 Matryoshka 截断（`sendDimensions` 开关）
- **超时分级**：全局 / recall / capture 三档超时（capture 更短避免阻塞对话）

### 8.7 Embedding 维度漂移检测（`sqlite.ts` 547-601）

```typescript
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
}
```

返回 `VectorStoreInitResult { needsReindex, reason }` 让上层调度 `reindexAll()`。

---

## 九、存储层设计

### 9.1 SQLite 四表结构（`sqlite.ts` 3399 行）

**L1 metadata 表**：

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

**向量虚拟表**：

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS l1_vec USING vec0(
  record_id TEXT PRIMARY KEY,
  embedding float[${dimensions}] distance_metric=cosine,
  updated_time TEXT DEFAULT ''
)
```

注意：**vec0 不支持 ON CONFLICT**，所以 upsert = delete + insert。

**附加表**：
- `skills` + `skill_fts` + `skill_vec`：Skill 主表 + 全文索引 + 向量索引
- `profiles`：L2/L3 画像文件元数据
- `memory_audit`：审计日志

### 9.2 upsert = delete + insert 模式

```typescript
upsertL1(record: MemoryRecord, embedding: Float32Array | undefined): boolean {
  this.db.exec("BEGIN");
  try {
    // 1. Upsert metadata (INSERT OR UPDATE via ON CONFLICT)
    this.stmtUpsertMeta.run(...);

    if (!skipVec) {
      // 2. vec0 does not support ON CONFLICT → delete then insert
      this.stmtDeleteVec!.run(recordId);
      this.stmtInsertVec!.run(recordId, Buffer.from(embedding!.buffer), record.updatedAt);
    }

    // 3. Sync FTS5 (delete + re-insert to handle updates)
    if (this.ftsAvailable) {
      this.stmtL1FtsDelete.run(recordId);
      this.stmtL1FtsInsert.run(...);
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

### 9.3 WAL 模式 + 事务配置

```typescript
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

### 9.4 Knowledge DB Drizzle ORM（`schema.ts` 152 行）

| 表 | 用途 |
|---|------|
| `knowledge_code_graph` | 代码仓库索引元数据 |
| `knowledge_wiki` | Wiki 知识库元数据 |
| `knowledge_wiki_audit` | Wiki 状态变更审计 |
| `knowledge_code_graph_audit` | CodeGraph 状态变更审计 |
| `llm_binding` | Per-instance LLM 路由绑定 |

**软删除 + 部分唯一索引**：

```typescript
export const knowledgeCodeGraph = sqliteTable(
  "knowledge_code_graph",
  {
    codeGraphId: text("code_graph_id").primaryKey(),
    serviceId: text("service_id").notNull(),
    teamId: text("team_id").notNull(),
    repoName: text("repo_name").notNull().default(""),
    repoUrl: text("repo_url").notNull(),
    branch: text("branch").notNull(),
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

**版本乐观锁**：`version` 字段

### 9.5 TCVDB 存储（`tcvdb.ts`）

腾讯云向量数据库后端：

- **服务端 embedding**：`embeddingItems` 配置
- **客户端稀疏向量**：`BM25LocalEncoder`（纯 TS 实现，基于 `@tencentdb-agent-memory/tcvdb-text`）
- **原生混合搜索**：`hybridSearch`（dense + sparse + RRFRerank）
- **Filter 表达式**：标量字段过滤

**统一接口**：`IMemoryStore` trait 隔离 SQLite / TCVDB，上层无感知切换。

---

## 十、Wiki 引擎

### 10.1 架构概览（`engines/wiki/manager.ts`）

- **摄取引擎**：`engines/wiki/ingest-v2/index.ts` 编排 `ingestSource()`
- **索引存储**：每个 wiki 私有的 `index.db`（SQLite：`wiki_fts` + `page_meta` + `graph_edge`）
- **图谱构建**：`graphology` 临时内存实例做多跳 BFS（`graph-search.ts`）

### 10.2 摄取流程（Ingest V2）

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

### 10.3 Graph 多跳检索（`graph-search.ts`）

**算法**：BFS 分层扩展，从 BM25 seed 出发沿 `[[wikilink]]` 边行走 `hop` 步（0-5），每跳 score 乘以 `decay`（默认 0.5），低于 `minScore`（默认 0.1）剪枝。

**关键不变量**：
- **Seed 冻结**：seed 节点保持 hop=0 和原始 BM25 score，不参与衰减
- **首次到达即定 hop**：BFS 层序保证第一次到达某节点时 hop 最小
- **最高分优先**：非 seed 节点多条路径到达时取最高分
- **DoS 防护**：`maxNodes` 硬上限（默认 200）

**graphology 集成**：临时内存实例做多跳 BFS，社区发现使用 `graphology-communities-louvain` Louvain 算法。

---

## 十一、CodeGraph 引擎

### 11.1 封装层（`engines/code/bridge.ts`）

封装 `@colbymchenry/codegraph` 核心 API：

- **索引操作**：`indexProject()` / `openIndex()` / `syncIndex()` / `closeIndex()`
- **工具执行**：`executeTool(toolName, params)` 复用 `ToolHandler` 格式化输出
- **平台包解析**：`resolveToolsPath()` 从 npm 主包解析平台包路径

### 11.2 MCP 12 工具集（`src/mcp/tools.ts`）

**Code-Graph (8)**：`code_search / code_explore / code_callers / code_callees / code_impact / code_node / code_status / code_files`

**Wiki (4)**：`wiki_search / wiki_read / wiki_list / wiki_graph`

**设计约束**：仅暴露查询类工具（只读），管理操作（create/delete/sync）不暴露给 MCP agent，由管控 UI 处理。

### 11.3 索引池（Instance Pool）

- **懒加载**：`instancePool.loadIfMissing()` 首次访问时打开索引
- **共享队列**：`BuildQueue` 串行 wiki + code 任务
- **Source Fetcher**：`SourceFetcherRegistry` 路由 git/local/ftp，含 SSRF 防护

### 11.4 MCP Server 传输（`MemoryKnowledge/src/mcp/server.ts`）

- **传输方式**：`StdioServerTransport`（独立进程）
- **工具转发**：`callApi()` 把 MCP tool call 转发到 Hono HTTP API
- **错误处理**：`{text, isError}` 透传

---

## 十二、插件体系

### 12.1 OpenClaw 插件（`MemoryCore/index.ts`）

- **插件声明**：`openclaw.plugin.json` 定义 id / name / activation / contracts
- **入口注册**：`openclaw.extensions: ["./index.ts"]`
- **Hook 注册**：
  - `before_prompt_build`：缓存原始 user prompt + 触发 auto-recall
  - `agent_end`：触发 auto-capture + pipeline 调度
- **工具注册**：`tdai_memory_search` / `tdai_conversation_search` / `tdai_read_cos`
- **双模式**：`local`（in-process）/ `client`（连接外部 Gateway）

### 12.2 Hermes 插件（`MemoryCore/hermes-plugin/`）

Python 实现，实现 `MemoryProvider` 接口：

- **Gateway Supervisor**：`supervisor.py` 管理 Node.js Gateway 子进程
- **Circuit Breaker**：连续 N 次失败后暂停 API 调用（60s 冷却）
- **Watchdog**：10s 间隔巡检，死亡自动复活
- **后台 sync 线程**：`_MAX_INFLIGHT_SYNCS=4` 并发上限
- **v3 迁移**：数据面走 `/v3/*` 端点 + team/agent/user 隔离

### 12.3 宿主适配层

- **OpenClawHostAdapter**（`adapters/openclaw/host-adapter.ts`）：进程内调用
- **StandaloneHostAdapter**（`adapters/standalone/`）：HTTP 调用 Gateway
- **LLM Runner 抽象**：`LLMRunnerFactory` 隔离宿主 LLM 能力

---

## 十三、SDK 设计

### 13.1 TypeScript SDK（`sdk/memory-core/typescript/`）

- **包名**：`@tencentdb-agent-memory/memory-sdk-ts-v2`
- **版本兼容**：默认 export 指向 v3，保留 `/v3` 子路径别名
- **核心类**：`MemoryClient` / `AsyncMemoryClient`
- **传输层**：`HttpTransport` 可替换
- **COS 直读**：`MemoryFileReader` + `StsCredentialManager`

### 13.2 Python SDK（`sdk/memory-core/python/`）

- **包名**：`tencentdb-agent-memory-sdk-python`
- **构建系统**：hatchling
- **依赖**：`httpx>=0.24.0`
- **版本布局**：默认 export v2，`from ...v3 import MemoryClient` 切 v3
- **子模块**：`v2/client.py` / `v3/client.py` / `v3/metadata_client.py` / `v3/skill_client.py`

### 13.3 SDK 能力矩阵

| 能力 | TypeScript | Python |
|------|-----------|--------|
| L0 对话 ✓增✓查✓搜 | ✓ | ✓ |
| L1 记忆 ✓增✓改✓查✓搜 | ✓ | ✓ |
| L2 场景 ✓列✓读✓写 | ✓ | ✓ |
| L3 画像 ✓读✓写 | ✓ | ✓ |
| Skill ✓ CRUD | ✓ | ✓ |
| Meta ✓ 元数据 | ✓ | ✓ |
| COS ✓ 直读 | ✓ | ✓ |

---

## 十四、部署架构

### 14.1 Docker 三件套

```
docker network: tdai-memory-stack
├── tdai-memory-core  (:8420)  — 记忆内核 Gateway
├── tdai-memory-hub   (:8125)  — 面板 + 知识服务
└── tdai-proxy        (:8096)  — 上下文注入代理
```

### 14.2 一键部署脚本（`deploy/global-images/start-all.sh`）

```bash
./start-all.sh            # 本地镜像直接起
PULL=1 ./start-all.sh     # 先 pull 最新镜像
```

**启动顺序**：
1. `start-memory-core.sh`：起 memory → healthy 检查
2. `start-memory-hub.sh`：起 memory-hub → healthy 检查
3. `start-proxy.sh`：起 proxy（默认开 full stack：auth + sessionInit + tdai 注入）

### 14.3 环境变量配置

**两组独立 LLM 参数**：
- `MEMORY_*`：memory + memory-hub 内部调用（embed / summarize / wiki ingest）
- `PROXY_*`：proxy 转发到的上游 LLM（coding agent 请求）

### 14.4 客户端接入

```bash
# 通过 proxy 用 Claude Code
export ANTHROPIC_BASE_URL=http://127.0.0.1:8096/claude-code/default
export ANTHROPIC_AUTH_TOKEN='<admin-key>'
claude --model <upstream-model>
```

---

## 十五、对 laew 的借鉴

### 15.1 可直接借鉴的设计模式

#### 15.1.1 L0-L3 四层记忆提炼

**laew 现状**：当前仅有 `session_memory` 表存摘要，无分层提炼。

**借鉴点**：
- **L0 原始对话**：JSONL 按日分片，增量捕获（位置切片 + 时间戳游标双保护）
- **L1 结构化记忆**：LLM 抽取 + 向量去重（`store / update / merge / skip` 四态决策）
- **L2 场景块**：LLM 沙箱限制 workspaceDir，物理隔离元数据文件
- **L3 画像综合**：增量模式只分析变化场景，降低 token 消耗

**落地建议**：
- 在 `src/agent/memory/` 新增 `l0-recorder.rs` / `l1-writer.rs` / `l2-scene.rs` / `l3-persona.rs`
- 存储用 SQLite（laew 已有 `LsmAgentEmergentWork.db`）+ sqlite-vec 扩展
- 引入 `node:sqlite`（Node 22+ 内置）

#### 15.1.2 RRF 混合检索

**laew 现状**：无记忆检索，仅靠 SessionContext 注入最近 N 条摘要。

**借鉴点**：
- FTS5 BM25（关键词）+ vec0 向量（语义）+ RRF 融合
- 标准 RRF 公式：`score = Σ 1/(k + rank + 1)`，k=60
- 中文 jieba 分词 + 停用词过滤

**落地建议**：
- 新增 `src/agent/memory/rrf.rs` 实现 `rrfMerge()`
- 在 `src/agent/tools/` 新增 `memory_search` 工具
- 召回结果注入 system prompt（带 `<<<LAEW:RECALLED_MEMORIES>>>` 标记）

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

#### 15.1.3 上下文注入管线

**laew 现状**：`Yolo 项目上下文注入` 是简单的一次性注入（`<<<LAEW:PROJECT_CONTEXT>>>`）。

**借鉴点**：
- **Hook 注册机制**：按 injection point 注册多个 hook
- **协议适配器**：`OpenAIAdapter` / `AnthropicAdapter` 双向转换
- **缓存策略**：`session_init` / `hybrid` / `none` 三态
- **Prewarm 预热**：session 初始化时预热，后续复用

**落地建议**：
- 新增 `src/agent/injection/` 模块
- 定义 `InjectionHook` 接口 + `HookRegistry`
- 支持 `system.prefix` / `system.suffix` / `user.before` 等注入点

#### 15.1.4 Skill 版本化系统

**laew 现状**：无 Skill 系统。

**借鉴点**：
- **SKILL.md 格式**：YAML frontmatter + Markdown body
- **单表多行多版本**：`(skill_id, version)` + `is_head` 标记
- **跨系统事务**：COS + DB + asset 三系统"顺序 + 补偿"
- **乐观锁**：`expected_version` 防并发覆盖

**落地建议**：
- 新增 `src/agent/skill/` 模块
- 存储复用 SQLite `skills` 表 + FTS5
- 工具 `skill_create` / `skill_update` / `skill_search`

#### 15.1.5 多租户隔离

**laew 现状**：单用户单设备，无多租户概念。

**借鉴点**：
- **三维隔离**：`(team_id, user_id, agent_id)`
- **强制校验**：`assertIsolation()` 必填字段
- **Legacy 兼容**：`legacyCompatMode` 占位符填充

**落地建议**：
- 当前阶段可简化为 `(user_id, agent_id)` 二维
- 在 `Session` 中增加 `agentId` 字段
- 记忆查询强制带 agentId 过滤

### 15.2 架构层面的启示

#### 15.2.1 Host-neutral 门面模式

TdaiCore 的 `HostAdapter` 抽象值得借鉴：

```typescript
// laew 可借鉴的抽象
interface HostAdapter {
  getLogger(): Logger;
  getLlmRunner(): LlmRunner;
  getStorage(): StorageAdapter;
  getDataDir(): string;
}
```

laew 当前直接依赖 OpenClaw/Hermes，可抽象出 `HostAdapter` 隔离宿主。

#### 15.2.2 Promise Gate 并发保护

`schedulerStartPromise` 防止并发启动的模式，适用于 laew 的 `MultiAgentOrchestrator`：

```typescript
private schedulerStartPromise?: Promise<void>;
async ensureSchedulerStarted() {
  if (this.schedulerStartPromise) return this.schedulerStartPromise;
  this.schedulerStartPromise = this.doStart();
  return this.schedulerStartPromise;
}
```

#### 15.2.3 Checkpoint 原子操作

`captureAtomically()` 文件锁保护读-写-推进序列，适用于 laew 的 Session 状态持久化。

### 15.3 工具与存储建议

#### 15.3.1 新增工具清单

| 工具 | 用途 | 优先级 |
|------|------|-------|
| `memory_search` | L1 记忆混合检索 | P0 |
| `memory_write` | 手动写入/修改记忆 | P0 |
| `skill_create` | 创建 Skill | P1 |
| `skill_search` | 搜索 Skill | P1 |
| `persona_read` | 读取当前 Persona | P1 |
| `scene_list` | 列出场景块 | P2 |

#### 15.3.2 SQLite 表扩展

```sql
-- L1 记忆表
CREATE TABLE l1_memories (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  type TEXT NOT NULL,           -- persona / episodic / instruction
  priority INTEGER DEFAULT 50,  -- 0-100
  scene_name TEXT,
  source_message_ids TEXT,      -- JSON array
  metadata_json TEXT,
  timestamps TEXT,              -- JSON array
  session_key TEXT,
  session_id TEXT,
  agent_id TEXT,
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

### 15.4 反模式警示

1. **不要过度设计**：TencentDB 的 6 个子系统（Core/Knowledge/Panel/Proxy + 双 SDK）对 laew 来说过重。建议先做 L1 记忆 + RRF 检索两个核心能力。
2. **避免 LLM 沙箱逃逸**：TencentDB 的"LLM 直写文件"模式需要严格的 workspaceDir 限制，laew 当前零沙箱，引入时需同步建设权限管控。
3. **警惕向量维度漂移**：embedding provider/model/dimensions 变化时需要 reindex，laew 需设计 `needsReindex` 检测机制。
4. **防止记忆注入污染**：TencentDB 用 `sanitizeText` + `stripCodeBlocks` 防止注入标记被 LLM 重复注入，laew 的 `<<<LAEW:PROJECT_CONTEXT>>>` 也需类似处理。
5. **避免跨系统事务**：laew 当前只有 SQLite，不要为了实现 Skill 版本化引入 COS + asset 等多系统。建议 Skill v1 仅用 SQLite 单库 + 乐观锁。

### 15.5 推荐落地路线图

```
P0（1-2 周）：L0 对话录制 + L1 记忆表 + RRF 混合检索
P1（2-4 周）：L2 场景块 + Skill 系统 + memory_search 工具
P2（1-2 月）：L3 画像 + 上下文注入管线 + 多租户隔离
P3（2-3 月）：Wiki/CodeGraph 知识引擎 + MCP 服务
```

---

## 十六、总结

TencentDB Agent Memory 是一个**工程完整度高、设计模式成熟**的团队记忆系统。其核心亮点包括：

1. **L0-L3 四层提炼管线**：从原始对话到用户画像的渐进式抽象
2. **BM25 + 向量 + RRF 混合检索**：关键词与语义的互补融合
3. **上下文注入管线**：协议适配器 + Hook 注册 + 缓存策略的三层抽象
4. **Skill 版本化系统**：跨系统事务 + 乐观锁 + ACL 的完整生命周期
5. **多宿主适配**：OpenClaw / Hermes / Gateway 三种宿主的统一门面
6. **MemoryPipelineManager 级联调度**：warm-up 模式 + 向下_only_ timer + L3 全局互斥 + Session GC
7. **Checkpoint 原子操作**：per-file async lock + tmp+rename + split-state 设计
8. **多租户三维隔离**：`(team_id, user_id, agent_id)` + 5 级可见性 + 6 类权限
9. **Wiki 知识引擎**：Ingest V2 两阶段 LLM 摄取 + graphology 图谱多跳检索
10. **MemoryProxy 可观测性**：OpenTelemetry + Langfuse + Opik + ClickHouse 四件套

对 laew 而言，**L1 结构化记忆 + RRF 混合检索 + 上下文注入管线** 是最具借鉴价值的三个方向，可显著提升 Agent 的"记忆"能力，解决当前"每次新 Session 从零开始"的痛点。

---

*调研人：Claude Code Agent*
*调研日期：2026-09-05*
*源码路径：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`*
*原始文档：*
- `TencentDB-Agent-Memory-源码调研.md`（938 行）
- `TencentDB-Agent-Memory-深度分析.md`（977 行）
- `TencentDB-Agent-Memory-核心机制深度分析.md`（1893 行）
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
# TencentDB Agent Memory 第十轮深挖 — 8 新维度深度分析

> 调研对象:TencentDB-Agent-Memory(TypeScript+Python,团队记忆系统)
> 调研日期:2026-09-07
> 前 9 轮覆盖:L0-L3 管线 / SkillCore / InjectionPipeline / MemoryProxy / 多租户隔离 / RRF 混合检索 / 存储层 / Wiki / CodeGraph / 插件 / SDK / 部署 / 系统提示词 / 可观测性 / LSP / 遥测 / Web 检索 等 50+ 维度
> 本轮 8 新维度:**CrashDump 与错误恢复** / **WebUI 与 DesktopApp** / **OAuth 认证与多账号** / **i18n 国际化** / **Release 工程化与 AutoUpdate** / **WebSocket 与 SSE** / **DevContainer 与容器化** / **CRDT 与多端冲突**
> 总行数:~2,300 行

---

## 一、CrashDump 与错误恢复

### 1.1 架构全景

TencentDB-Agent-Memory 采用**分层错误分类 + 优雅降级 + 指数退避重试**三层策略，无传统 CrashDump（核心转储）机制，依赖结构化错误码与 traceId 追踪。

```
异常抛出
   │
   ├─ MemoryProxy 层 ──→ withL0Retry (指数退避 3 次)
   │                 ──→ pending-writes flush (SIGTERM 前兜底)
   │                 → classifyError → HTTP 响应 + traceId
   │
   ├─ MemoryCore 层 ──→ RecallErrors 4 级分类 (1xxxx/2xxxx/3xxxx/9xxxx)
   │                 → embedding 重试 (MAX_RETRIES=0 退化为单次 + 超时 abort)
   │                 → L1 失败回缓冲 + 5 次重试上限
   │
   └─ MemoryPanel 层 → onBlur 级 request-id envelope (code 500 + request_id)
```

### 1.2 Gateway 错误分类器（error-handler.ts）

`MemoryCore/src/gateway/error-handler.ts` 是 MemoryCore 网关的统一错误出口：

```typescript
export interface ClientFacingError {
  code: number;        // 业务码
  message: string;     // 已消毒的安全消息
  trace_id: string;    // UUID 用于日志关联
  retryable?: boolean; // 客户端是否值得重试
}

export function classifyError(err: unknown): ClassifiedError {
  const trace_id = randomUUID();

  // 1. PayloadTooLargeError (CR-7) — duck-typed
  if (statusCode === 413) return { status: 413, ... retryable: false };

  // 1b. Invalid JSON body — 400
  // 1c. COS AppendPositionErr — 409 Concurrent write conflict, retryable: true
  // 1d. Unsupported Content-Encoding — 415

  // 2. RecallFailure (H-15) — 已分类的 recall 错误
  if (err instanceof RecallFailure) {
    return {
      status: re.category === "config" ? 503 : 500,
      client: { code: re.code, message: re.message, retryable: re.retryable },
    };
  }

  // 4. Generic 5xx — 严格隐藏 err.message
  return {
    status: 500,
    client: { code: 500, message: "Internal server error", retryable: true },
  };
}
```

**关键设计**：
- **traceId 隔离**: 客户端拿 UUID 报障，服务商用 UUID 查完整日志
- **strict hide**: 5xx 场景 err.message / err.stack 绝不返回给客户端
- **duck-typing**: 避免 server.ts ↔ error-handler.ts 循环依赖

### 1.3 日志脱敏（sanitize）

```typescript
export function sanitize(input: string): string {
  return input
    .replace(/sk-ant-[A-Za-z0-9_-]{16,}/g, "sk-ant-***")   // Anthropic
    .replace(/sk-[A-Za-z0-9_-]{16,}/g, "sk-***")           // OpenAI/DeepSeek
    .replace(/(Bearer|Basic)\s+[A-Za-z0-9._\-+/=]+/gi, "$1 ***")
    .replace(/"(?:SecretKey|apiKey|api_key|password|token|authorization|TmpSecretId|TmpSecretKey|TmpToken)"\s*:\s*"[^"]*"/gi, '$1"***"');
}
```

**脱敏范围**: API Key（sk-ant-/sk-）、Bearer/Basic 授权头、JSON 敏感字段（SecretKey/apiKey/password/authorization/TmpSecret* 临时凭证）。

### 1.4 RecallError 四级分类（recall-errors.ts）

```
1xxxx — config (non-retryable, 需运维介入)
         10001 configMissingEmbedding
         10002 configInvalidStrategy

2xxxx — dependency (retryable, 瞬时故障)
         20001 dependencyTimeout
         20002 dependencyUnavailable

3xxxx — storage (retryable)
         30001 storageError

9xxxx — internal (non-retryable, 代码 bug)
         90001 internalError
```

**设计哲学**: 数字编码是稳定 wire contract，永不复用；`retryable` 字段驱动客户端/Proxy 的重试决策。

### 1.5 L0 写入重试（pending-writes.ts）

MemoryProxy streaming 场景的 **fire-and-forget** 风险兜底：

```typescript
// 指数退避重试: 3 次总尝试, 间隔 500ms → 1s → 2s (含 jitter)
export async function withL0Retry<T>(
  fn: () => Promise<T>,
  opts: { attempts?: number; baseMs?: number } = {},
): Promise<T> {
  const attempts = opts.attempts ?? 3;
  const baseMs = opts.baseMs ?? 500;
  for (let i = 0; i < attempts; i++) {
    try { return await fn(); }
    catch (err) {
      if (!isRetryable(err) || i === attempts - 1) throw err;
      const wait = baseMs * (2 ** i) + Math.floor(Math.random() * 200);
      await new Promise((r) => setTimeout(r, wait));
    }
  }
}

function isRetryable(err: unknown): boolean {
  // 网络错、5xx、408、429 值得重试；4xx 客户端错直接抛
  if (/abort|econnreset|enotfound|etimedout|fetch failed|network|timeout/i.test(msg)) return true;
  const m = msg.match(/HTTP (\d{3})/);
  if (!m) return true;  // 无状态码信息 → 保守 retry
  const code = Number(m[1]);
  return code >= 500 || code === 408 || code === 429;
}
```

**关键数字**：
- 默认 3 次总尝试（含原始那次），间隔 500/1000/2000ms + 0-200ms jitter
- 总最长 ~3.5s，K8s SIGTERM grace period 通常 30s，flushPendingWrites 默认 10s
- 重复写风险：tdai `/v3/conversation/add` 没有 idempotency-key；但 L1/L2/L3 蒸馏管线幂等（同 hash 单条），仅 L0 冗余可接受

### 1.6 Graceful Shutdown（MemoryProxy）

```typescript
// 顺序: L0 flush (10s) → langfuse → clickhouse → logger → exit
async function gracefulShutdown(signal: "SIGTERM" | "SIGINT"): Promise<void> {
  const pending = pendingWriteCount();
  if (pending > 0) {
    const { drained, remaining } = await flushPendingWrites(10_000);
  }
  await shutdownGuard();       // Redis session store
  await shutdownLangfuse();    // tracing
  await shutdownClickHouse();  // 遥测
  await shutdownLogger();
  process.exit(0);
}
process.on("SIGTERM", () => { void gracefulShutdown("SIGTERM"); });
process.on("SIGINT", () => { void gracefulShutdown("SIGINT"); });
```

### 1.7 L1 Pipeline 失败重试

`MemoryCore/src/utils/pipeline-manager.ts` 的 L1 失败处理：

```typescript
// 失败时消息放回缓冲区 + l1RetryCount 递增，30s 后重试（最多 5 次）
} catch (err) {
  // On failure: put messages back into the buffer for retry
  timers.l1RetryCount += 1;
  if (timers.l1RetryCount <= this.L1_MAX_RETRIES) {  // 5
    setTimeout(() => this.onL1RetryTimeout(sessionKey), this.L1_RETRY_DELAY_MS);  // 30s
  } else {
    // giving up auto-retry, messages remain buffered
  }
}
// Success: reset retry count
timers.l1RetryCount = 0;
```

### 1.8 Embedding 远程调用重试

`MemoryCore/src/core/store/embedding.ts`：

```typescript
const MAX_RETRIES = 0;  // 默认关闭重试，仅 timeout abort
for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
  try {
    const timeoutId = setTimeout(() => controller.abort(), timeoutOverride ?? this.timeoutMs);
    // ...
  } catch (err) {
    if (err instanceof EmbeddingApiError && err.isClientError()) throw err;  // 4xx 直接抛
    if (attempt < MAX_RETRIES) {
      const delay = 500 * (attempt + 1);  // 500ms, 1000ms
      await new Promise((r) => setTimeout(r, delay));
    }
  }
}
```

**设计取舍**: `MAX_RETRIES=0` 意味着 embedding 默认不重试（仅超时 abort）；实际生产中 embedding 失败时 graceful degradation：跳过 vec0 写入，仅写 metadata + FTS。

### 1.9 Rate Limit 降级

`MemoryProxy/src/rate-limit/redis-store.ts`：

```typescript
private degradedDecision(reason = "redis_unavailable"): RateLimitDecision {
  return {
    allowed: true,        // 降级时放行
    degraded: true,
    degradedReason: reason,
    reason: null,
    retryAfterSeconds: 0,
  };
}
```

**策略**: Redis 不可用时 rate limiter **fall open**（放行），避免单点故障阻断全部流量。这是典型的**故障开放**模式，优先可用性。

### 1.10 缺失项（CrashDump gap）

| 缺失 | 影响 |
|------|------|
| 无 CrashDump / core dump 机制 | Node.js 进程崩溃后无法事后回溯调用栈 |
| 无 Sentry / Bugsnag 集成 | 前端/后端异常无自动聚合报警 |
| 无 panic hook | 未捕获异常依赖 Hono onError / process.on |
| 无结构化错误上报 | 错误分散在 log 中，无统一错误看板 |
| 无重试次数动态配置 | attempts/baseMs 硬编码，无法运行时调参 |
| L2/L3 失败无重试上限 | 仅靠 armL2MaxInterval 最终重试，无放弃机制 |
| 无断路器 (circuit breaker) | 上游持续 5xx 时仍会继续请求，无快速失败 |
| idempotency-key 缺失 | L0 重试可能产生重复记录 |

---

## 二、WebUI 与 DesktopApp（MemoryPanel Web）

### 2.1 技术栈总览

`MemoryPanel/web/` 是完整的 React 单页应用：

| 维度 | 选型 |
|------|------|
| 框架 | React 18.3 + TypeScript 5.7 |
| 构建 | Vite 6.0 + tsc |
| 路由 | react-router-dom 7 (HashRouter) |
| 状态管理 | zustand 5 |
| UI 组件库 | tea-component 2.8 (腾讯内部组件库) |
| 国际化 | react-i18next 17 + i18next 26 |
| 样式 | TailwindCSS 3.4 + PostCSS |
| 可视化 | sigma 3 + graphology 0.26 + @react-sigma/core 5 |
| Markdown | react-markdown 10 + remark-gfm |
| Toast | sonner 1.7 |
| 时间处理 | moment 2.30 |

**包名**: `community-loop-lab-web` v0.1.0（开源版重命名）。

### 2.2 路由结构（8 大业务页面）

```typescript
// src/routes — HashRouter (避免静态部署刷新 404)
export const routes: RouteObject[] = [
  {
    path: '/',
    element: <ConsoleLayout />,
    children: [
      { index: true, element: <WorkbenchPage /> },          // 任务看板
      { path: 'wiki', element: <WikiPage /> },              // Wiki 知识库
      { path: 'code', element: <CodePage /> },              // Code_Graph
      { path: 'skills', element: <SkillsPage /> },          // Skill 技能
      { path: 'memory', element: <ChatMemoryPage /> },      // Chat_Memory
      { path: 'team/members', element: <MembersPage /> },   // 成员管理
      { path: 'team/agents', element: <AgentsPage /> },     // Agents 管理
      { path: 'team/api-keys', element: <ApiKeysPage /> },  // API Key
    ],
  },
];
```

**资源页** `ResourcePage` 作为独立页（资产分配、资产范围管理、AdminResourceLock）。

### 2.3 核心组件体系

```
components/
├── LoginGate.tsx          — 登录入口 (user_key 鉴权)
├── RouteGuards.tsx        — 路由守卫
├── SettingsDialog.tsx     — 设置对话框
└── MarkdownView.tsx       — Markdown 渲染

layouts/
├── ConsoleLayout.tsx      — 控制台布局 (侧栏 + 顶栏 + 内容)
├── GlobalHeader/          — 全局顶栏 (同步/设置/资料/登出)
└── TabBar/                — 标签栏

pages/
├── workbench/WorkbenchPage/  — 任务看板 (任务列表/创建/详情)
├── wiki/WikiPage/            — Wiki 来源/图谱/页面/搜索
├── code/CodePage/            — CodeGraph 仓库/索引/搜索
├── skills/SkillsPage/        — Skill 全部/团队池/Agent资产
├── memory/ChatMemoryPage/    — L0-L3 分层记忆 (BlockDetail/Allocate/Import)
├── team/                     — Members/Agents/ApiKeys
└── ResourcePage/             — 资产管理 (分配/锁)
```

### 2.4 登录认证流程（LoginGate）

```
无 Cookie、无 OAuth，Header 双凭证鉴权:

1. GET /api/v1/meta/instances     → 选记忆实例
2. 用户输入自持的 user_key（sk-mem-…）
3. POST /api/v1/meta/auth/verify  → valid=true 登录成功
4. 前端缓存 { instance_id, user_key, user } 到 localStorage
5. 后续每个 meta 请求注入双 Header (X-Tdai-Service-Id + Authorization)
```

**AuthState 结构**：
```typescript
export interface AuthState {
  user: string;         // display_name || username
  user_id: string;      // ULID (owner 判定 key)
  instance_id: string;  // 元数据实例 ID
  instance_name: string;
  loggedInAt: number;
  isAdmin: boolean;     // user_type === 'system_admin'
}
```

### 2.5 前端数据获取策略

**无 WebSocket 实时推送**，全部采用 **tab 切换 + 写操作后 poll** 模式：

```typescript
// SkillsPage — "poll on tab change + after every write action. No setInterval"
// CodePage — poll CodeGraph 索引状态
const poll = async () => { ... };
void poll();
const timer = setInterval(() => { void poll(); }, 5000);  // 5s 间隔

// WikiPage — poll ingest 状态
const timer = window.setInterval(poll, 2000);  // 2s 间隔
```

**请求序号防竞态**（ChatMemoryPanel）：
```typescript
const fetchSeqRef = useRef(0);
const fetchBlocks = useCallback(async () => {
  const seq = ++fetchSeqRef.current;
  const data = await chatMemoryApi.getBlocks(teamId);
  if (seq !== fetchSeqRef.current) return;  // 旧响应丢弃
  setBlocks(data);
}, [teamId]);
```

### 2.6 知识图谱可视化

```typescript
import { Graph } from "graphology";
import { circular } from "graphology-layout";
import forceAtlas2 from "graphology-layout-forceatlas2";
import Sigma from "sigma";
import { SigmaContainer, ControlsContainer } from "@react-sigma/core";
```

**能力**: 知识图谱的 ForceAtlas2 力导向布局、社区发现（Louvain 算法）、节点交互。

### 2.7 Panel HTTP 后端路由

`MemoryPanel/src/panel/http/` 基于 **Hono** 框架：

```typescript
// buildPanelApp 注册的路由
registerHealthRoutes(app);           // /health
const api = new Hono();
registerMetaInstanceRoutes(api);     // /api/v1/meta/instances
registerMetaProxyRoutes(api);        // /api/v1/meta/*
registerSkillProxyRoutes(api);       // /api/v1/skill/* → 内核 /v3/skill/*
registerChatMemoryRoutes(api);       // /api/v1/chat-memory/*
registerTaskRoutes(api);             // /api/v1/task/*
registerAgentOverviewRoutes(api);    // /api/v1/agent/overview
registerAgentLifecycleRoutes(api);   // /api/v1/agent/delete-cascade
registerKnowledgeRoutes(api);        // /api/v1/knowledge/*
app.route(API_PREFIX, api);

// SPA fallback
app.use('/*', serveStatic({ root: distDir }));
app.get('*', serveStatic({ path: path.join(distDir, 'index.html') }));
```

**中间件**：requestLogger 全局注入；onError 返回 `{ code: 500, message: "INTERNAL", request_id }`。

### 2.8 缺失项（DesktopApp gap）

| 缺失 | 影响 |
|------|------|
| 无 Electron / Tauri 桌面壳 | 仅浏览器访问，无法离线/系统集成 |
| 无 PWA / Service Worker | 无离线缓存、无桌面图标 |
| 无 WebSocket 实时推送 | 状态更新靠轮询（2-5s），有延迟 |
| 无原生通知集成 | 任务完成/失败无系统通知 |
| 无多窗口/多标签页同步 | 跨 tab 仅共享 localStorage，无 BroadcastChannel |
| 无本地文件导出/导入 UI | 仅 API 层支持导入，无拖拽上传 |

---

## 三、OAuth 认证与多账号

### 3.1 认证架构总览

**结论先行：项目无 OAuth/JWT/Passport，采用自研 user_key API Key 体系**。

```
用户请求
   │  Header: x-tdai-user-key: sk-mem-XXXXXXXXXXXXXXXXXXXX
   │  Header: x-tdai-service-id: <instance_id>
   ▼
MemoryProxy.auth.ts → verifyUserKey()
   │  POST /v3/meta/auth/verify → tdai kernel
   │  Response: { data: { valid, user: { user_id, ... } } }
   ▼
user_id + teamId + agentId 注入请求上下文
```

### 3.2 user_key 生成（crypto.ts）

```typescript
export function generateUserKey(): string {
  return USER_KEY_PREFIX + randomBytes(24).toString("base64url");
}
// USER_KEY_PREFIX = "sk-mem-"
// 24 字节 = 192bit 熵 → base64url 32 字符 → 总长度 ~40 字符
```

**设计要点**：
- base64url 字符集 `[A-Za-z0-9_-]`，URL/Header/JSON 安全
- 192bit 熵远高于碰撞界；单实例百万 key 碰撞概率可忽略

### 3.3 密码哈希（scrypt + pepper）

```typescript
export function hashPassword(plain: string, config: PasswordHashConfig): string {
  const salt = randomBytes(SALT_LEN);  // 16 bytes
  const hash = scryptHash(plain, salt, config);
  return `$scrypt$${N},${r},${p}$${salt_b64}$${hash_b64}`;
}

function scryptHash(plain: string, salt: Buffer, config: PasswordHashConfig): Buffer {
  const input = Buffer.concat([config.pepper, Buffer.from(plain, "utf8")]);
  return scryptSync(input, salt, config.keylen, { N: config.scryptN, r: config.scryptR, p: config.scryptP });
}
// 默认: N=16384, r=8, p=1, keylen=32
// pepper: 32 字节 base64 编码，部署时通过 TDAI_PASSWORD_PEPPER 环境变量注入
```

**安全分层**：
- **per-user salt**（随机 16 字节）
- **全局 pepper**（环境变量注入，不出现在代码/配置中）
- **scrypt** 内存硬哈希抗 GPU 破解

### 3.4 多租户隔离（instance_id 路由）

`MemoryCore/src/metadata/router/instance.ts`：

```typescript
export function extractInstanceId(headers: IncomingHttpHeaders): string {
  const raw = headers["x-tdai-service-id"];
  const id = Array.isArray(raw) ? raw[0] : raw ?? "";
  return normalizeInstanceIdForRoute(id);
}

// factory.ts — 每个 instance_id 独立 SQLite 库
export function resolveMetadataDbName(instanceId: string, prefix = "tdai_metadata"): string {
  return `${prefix}_${instanceId}`;  // 物理隔离
}
```

**三级数据隔离**：
1. **Instance 级**：不同 `x-tdai-service-id` → 不同 SQLite 文件
2. **Team 级**：所有查询强制带 `team_id` 过滤
3. **User/Agent 级**：`owner_agent_id` + `user_id` 行级 ACL

### 3.5 Proxy Auth 模块（auth.ts）

```typescript
export async function verifyUserKey(userKey: string, serviceId: string): Promise<VerifyUserResult> {
  if (!config) return { userId: "", rejected: false };        // auth disabled
  if (!serviceId) return { userId: "", rejected: true, rejectReason: "missing service_id" };
  if (!userKey) return { userId: "", rejected: true, rejectReason: "missing user_key" };

  const resp = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", "x-tdai-service-id": serviceId },
    body: JSON.stringify({ user_key: userKey }),
    signal: AbortSignal.timeout(config.timeoutMs),
  });
  // 解析 data.valid + data.user.user_id
}
```

**关键特性**：
- **无缓存**：每次请求实时打 auth 服务（安全优先，拒绝 stale token 风险）
- **fail-closed**：auth 服务不可用时 reject（可配置 fail-open）
- **永不抛异常**：返回结构化结果，由调用方决定

### 3.6 System User Passthrough

内部服务账号（CodeBuddy/Wiki-Indexer 等）绕过全量 pipeline：

```typescript
// 匹配 systemUsers 注册表中的 apiKey → 跳过：
//   - auth/verify
//   - session-init / conversation binding
//   - injection pipeline (skill/memory/wiki)
//   - routing decisions
//   - body rewrites (除 model-alias resolution)
// 保留：Opik/Langfuse/ClickHouse 可观测性 + credit report
```

### 3.7 Multi-account 支持

**不支持传统意义的 OAuth 多账号**：
- 一个 user_key 绑定一个 user_id
- 无 refresh token / token rotation
- 无 scope / permission 细分
- API Key 长期有效，无 TTL

**实际多账号模式**：
- 用户可加入多个 Team（TeamSwitcher 切换）
- 一个 User 可创建多个 Agent
- 一个 Agent 可分配多个 Skill/Memory Asset

### 3.8 缺失项（OAuth gap）

| 缺失 | 影响 |
|------|------|
| 无 OAuth 2.0 / OpenID Connect | 不支持 Google/GitHub/企业 SSO 登录 |
| 无 JWT / refresh token | user_key 长期有效，泄露风险高 |
| 无 token rotation | 无法定期更换凭证 |
| 无 scope / RBAC | API Key 粒度粗，无法限制读写权限 |
| 无 MFA / 2FA | 仅 user_key 单因子认证 |
| 无 session 过期 | localStorage 缓存无 TTL |
| 无 password 登录入口 | v3.1 后仅 user_key，password 仅历史兼容 |
| API Key 明文传输风险 | Header 传递，依赖 TLS 保护 |

---

## 四、i18n 国际化

### 4.1 技术架构

MemoryPanel/web 采用 **react-i18next + i18next**，实现运行时语言切换：

```typescript
// src/i18n/index.ts
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import { zhCN } from './zh-CN';    // 1113 行
import { enUS } from './en-US';    // 1160 行

function detectInitialLanguage(): string {
  const stored = localStorage.getItem('tdai-memory.lang');
  if (stored === 'zh-CN' || stored === 'en-US') return stored;  // 优先用户选择
  const nav = navigator.language || 'zh-CN';
  return nav.startsWith('zh') ? 'zh-CN' : 'en-US';              // 浏览器语言回退
}

i18n.use(initReactI18next).init({
  resources: {
    'zh-CN': { translation: zhCN },
    'en-US': { translation: enUS },
  },
  lng: detectInitialLanguage(),
  fallbackLng: 'zh-CN',
  interpolation: { escapeValue: false },
  react: { useSuspense: false },
});
```

### 4.2 翻译文件规模

| 文件 | 行数 | 内容分布 |
|------|------|---------|
| zh-CN.ts | 1,113 行 | 菜单/导航/全局/登录/工作台/Wiki/Code/Skill/Memory/Team/Error |
| en-US.ts | 1,160 行 | 完全对齐 zh-CN（en 略长） |

**命名空间扁平**：单 namespace `translation`，无模块化拆分。

### 4.3 翻译键结构（节选）

```typescript
// ===== Menu / Navigation =====
'menu.workbench_board': '任务看板',
'menu.wiki': 'Wiki 知识库',
'menu.code': 'Code_Graph',
'menu.skills': 'Skill 技能',
'menu.chat_memory': 'Chat_Memory',
'menu.team_members': '成员管理',
'menu.team_agents': 'Agents 管理',
'menu.api_keys': 'API Key',

// ===== TeamSwitcher =====
'teamSwitcher.selectTeam': '选择 team',
'teamSwitcher.empty.admin': '暂无 team。点击下方「新建团队」创建。',
'teamSwitcher.empty.member': '你还没有被加入任何 team。请联系管理员将你加入团队。',

// ===== Error messages =====
'error.UPSTREAM_ERROR': 'Upstream service call failed. Please try again later.',
```

### 4.4 tea-component 语言同步

```typescript
// App.tsx — react-i18next → tea-component ConfigProvider 同步
function toTeaLocale(lang: string): 'zh' | 'en' {
  return lang.startsWith('zh') ? 'zh' : 'en';
}
useEffect(() => {
  const handler = (lng: string) => setTeaLocale(toTeaLocale(lng));
  i18n.on('languageChanged', handler);
  return () => i18n.off('languageChanged', handler);
}, [i18n]);
```

**原因**: tea-component 是腾讯内部组件库，内置文案独立于 react-i18next；通过事件监听同步切换。

### 4.5 文档双语体系

| 中文文档 | 英文文档 | 备注 |
|---------|---------|------|
| README_CN.md | README.md | 产品总览 |
| INSTALL_CN.md | INSTALL.md | 安装指南 |
| CONTRIBUTING_CN.md | CONTRIBUTING.md | 贡献指南 |
| ROADMAP_CN.md | ROADMAP.md | 路线图 |
| — | README.deployment.md | 仅英文部署文档 |
| — | README.docker.md | 仅英文 Docker 文档 |

**文档策略**: 核心文档双语、部署文档英文优先。

### 4.6 缺失项（i18n gap）

| 缺失 | 影响 |
|------|------|
| 仅 2 语言 (zh-CN / en-US) | 无法覆盖日韩西班牙语等市场 |
| 无 RTL (阿拉伯语/希伯来语) | 布局方向固定 LTR |
| 无 ICU MessageFormat | 复数/性别/占位符需手动拼接 |
| 无翻译管理平台 | 翻译靠 PR 提交，无 crowdin/lokalise 集成 |
| 无运行时语言包加载 | 全量打包进 bundle，增加首屏体积 |
| 无服务端 i18n | 错误消息硬编码中文/英文，无 Accept-Language 探测 |
| 无日期/数字/货币本地化 | moment 仅做时间格式化，无区域数字格式 |
| 键名无命名空间 | 单 translation 扁平结构，大规模易冲突 |
| 无翻译覆盖率检查 | 缺少键时直接显示 key，无编译期报错 |

---

## 五、Release 工程化与 AutoUpdate

### 5.1 版本策略

| 子仓 | npm 版本 | Docker 镜像 tag |
|------|---------|----------------|
| MemoryCore | `@tencentdb-agent-memory/memory-tencentdb-v2` 2.0.0-beta.1 | `agentmemory/memory-core` |
| MemoryProxy | 0.1.0 | `agentmemory/memory-proxy` |
| MemoryPanel | 0.1.0 | — (合并到 memory-hub) |
| MemoryKnowledge | 0.1.0 | — (合并到 memory-hub) |
| SDK TS | `@tencentdb-agent-memory/memory-sdk-ts-v2` | — |
| SDK Python | `tencentdb-agent-memory-sdk-python` | — |

**版本分离**：npm 版本走 SemVer，Docker tag 独立（`2.0.0-beta.1` 镜像发 `:1.0.0-beta.1`）。

### 5.2 CHANGELOG 规范

遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) + SemVer：

```markdown
## [2.0.0] — 2026-08-03
### 🧠 四种记忆资产 · 首次完整开源
- Chat Memory — L0→L1→L2→L3 逐层提取
- Skill — 可复用 SOP
- Wiki — 结构化页面 + 链接图谱
- CodeGraph — 符号/文件/调用关系索引

### 🎛️ Memory Hub · 面向团队的操作台
- 三级可见性：private / team / restricted

### 🔀 Memory Proxy · Agent 挂上记忆的通道
- Anthropic / OpenAI 双协议

### 🚀 一条命令拉起完整三件套
```

### 5.3 CI/CD 工作流（pr-ci.yml）

```yaml
# 单 workflow，5 job 串行/并行混合
jobs:
  install:    # npm install + cache node_modules
  pack:       # npm pack → upload .tgz artifact
  manifest:   # 校验 openclaw.plugin.json + package.json openclaw metadata
  size:       # 包大小守卫 (MAX_KB=2048)
  isolation:  # Skill queue 隔离守卫 (禁止触碰红线文件)
```

**触发条件**: `pull_request → main`；concurrency group `ci-${{ github.ref }}` cancel-in-progress。

### 5.4 Docker 镜像发布（publish.sh）

```bash
cd deploy/dockerhub
VERSION=1.0.0 ./publish.sh all           # 三件套一次发布
VERSION=1.0.0 ./publish.sh memory-core   # 单组件
DRY_RUN=1 VERSION=1.0.0 ./publish.sh all # 干跑 (仅 secret-scan + context 准备)
ALSO_LATEST=1 VERSION=1.0.0 ./publish.sh all  # 同时更新 :latest
```

**安全前置**：`secret-scan.sh` 检查源码中的敏感信息（API Key/密码），防止泄漏到镜像。

### 5.5 一键部署（start-all.sh）

```bash
# deploy/global-images/
./start-all.sh            # 本地已有镜像直接用
PULL=1 ./start-all.sh     # 先 docker pull 升级到最新 :latest

# 内部顺序:
# Step 1/3: memory (内核) → healthy check
# Step 2/3: memory-hub (面板+知识) → healthy check
# Step 3/3: proxy
```

**辅助脚本**: `stop-all.sh --purge` 彻底清 volume + admin key；`verify.sh` 自检。

### 5.6 Multi-stage Docker 构建

**MemoryCore Dockerfile** (3 stage)：
```dockerfile
# Stage 1: deps-builder (node:22-slim + python3/make/g++)
FROM node:22-slim AS deps-builder
RUN npm install -g npm@11   # 规避 npm@10.9 arborist edgesOut crash
RUN npm install --omit=dev --omit=optional --ignore-scripts
RUN npm install --no-save esbuild  # tsx 运行时依赖

# Stage 2: runtime
FROM node:22-slim AS runtime
RUN apt-get install curl tini ca-certificates
COPY --from=deps-builder /build /app
HEALTHCHECK --interval=30s --timeout=5s CMD curl -fsS http://127.0.0.1:8420/health
ENTRYPOINT ["/usr/bin/tini", "--"]   # PID 1 信号传播
CMD ["node", "--import", "tsx", "src/gateway/server.ts"]
```

**关键设计**：
- `tini` PID 1：SIGTERM 正确传播给 Node + pipeline workers，回收僵尸进程
- BuildKit cache mount：`/var/cache/apt` + `/root/.npm` 加速重复构建
- apt 镜像可配：`APT_MIRROR=mirrors.tencent.com` 内网加速

### 5.7 Package 大小守卫

```yaml
# pr-ci.yml size job
MAX_KB=2048   # 2MB 上限
if [ "$SIZE_KB" -gt "$MAX_KB" ]; then exit 1; fi
```

### 5.8 缺失项（AutoUpdate gap）

| 缺失 | 影响 |
|------|------|
| 无 AutoUpdate 机制 | 用户需手动 `PULL=1 ./start-all.sh` 升级 |
| 无 hot reload | 配置变更需重启进程（OpenClaw 插件重注册除外） |
| 无版本检查 API | 客户端无感知远端有新版本 |
| 无 code signing | npm 包和镜像无 GPG/sigstore 签名 |
| 无 SBOM 生成 | 无软件物料清单，供应链审计困难 |
| 无灰度/金丝雀发布 | 一键全量起停，无流量切分 |
| 无回滚脚本 | 升级失败后需手动 `docker pull :prev` |
| CI 仅 PR 触发 | 无 nightly build / 自动发布 |
| 无 GitHub Release | 用户需手动查 CHANGELOG |
| 包大小硬编码 | 2MB 上限无法按组件微调 |

---

## 六、WebSocket 与 SSE

### 6.1 现状：无 WebSocket，SSE 仅用于 Form 响应

**搜索结论**：全仓库仅 1 处 TODO 提及 WebSocket，无实际实现。

```typescript
// MemoryPanel/src/panel/http/routes/knowledge/callback-routes.ts:194
// TODO: WebSocket push to frontend for real-time UI update
```

### 6.2 SSE（Server-Sent Events）应用

SSE 仅在 MemoryProxy 的 **session form**（首次 session 初始化引导）场景使用：

```typescript
// MemoryProxy/src/session/form.ts
return new Response(stream, {
  status: 200,
  headers: {
    "Content-Type": "text/event-stream",
    "Cache-Control": "no-cache",
    "Connection": "keep-alive",
  },
});

// MemoryProxy/src/session/codebuddy/form.ts
headers: { "Content-Type": "text/event-stream", ... };

// MemoryProxy/src/session/claude-code/form.ts
headers: { "Content-Type": "text/event-stream", ... };
```

**用途**: 模拟 Anthropic 流式响应格式，把"引导问题"伪装成流式 assistant 消息，让 CC/CodeBuddy 客户端渲染。

### 6.3 前端实时性策略：轮询替代推送

| 场景 | 策略 | 间隔 |
|------|------|------|
| Wiki 摄入状态 | `pollWikiStatus` + `setInterval` | 2s |
| CodeGraph 索引状态 | `pollCodeGraphStatus` + `setInterval` | 5s |
| Skill 列表刷新 | tab 切换 + 写操作后 refetch | 事件驱动 |
| Agent/Team 数据 | tab 切换 refetch | 事件驱动 |
| Chat Memory 资产 | 写操作后 refetch | 事件驱动 |

**典型代码**（WikiSourcesPanel.tsx）：
```typescript
const poll = async () => {
  const detail = await wikiApi.getDetail(wikiId);
  setDetail(detail);
  if (detail.status === "processing") {
    timer = window.setTimeout(poll, 2000);  // 2s 后继续轮询
  }
};
void poll();
```

### 6.4 Proxy 流式转发

Proxy 的 Anthropic/OpenAI 转发走 **HTTP streaming passthrough**，不解析 SSE chunk：

```typescript
// MemoryProxy/src/auxiliaryHandler.ts
const isStream = contentType.includes("event-stream");
// 直接 pipe 给客户端，不逐 chunk 处理
```

### 6.5 缺失项（WebSocket/SSE gap）

| 缺失 | 影响 |
|------|------|
| 无 WebSocket 服务端 | 无法主动推状态变更到前端 |
| 无 SSE 推送通道 | 长任务（Wiki ingest）只能轮询 |
| 无长连接管理 | 无法感知客户端掉线 |
| 无订阅/广播机制 | 多 tab 同步需手动刷新 |
| 无背压控制 | 轮询固定间隔，服务端压力大 |
| SSE 仅用于 Form 模拟 | 非真正的服务器→客户端事件推送 |
| 无重连退避 | 网络抖动时轮询中断即停止 |

---

## 七、DevContainer 与容器化

### 7.1 Docker 镜像矩阵

| 镜像 | 基础镜像 | 入口 | 端口 |
|------|---------|------|------|
| `agentmemory/memory-core` | node:22-slim | `tsx src/gateway/server.ts` | 8420 |
| `agentmemory/memory-proxy` | node:22-slim | Proxy server | 8096 |
| `agentmemory/memory-hub` | node:22-slim | Panel + Knowledge | 8125 + 8424 |

**多架构支持**: `linux/amd64` + `linux/arm64`（Apple Silicon 原生）。

### 7.2 Dockerfile 清单

```
./MemoryCore/Dockerfile                         # MemoryCore 生产镜像
./MemoryKnowledge/Dockerfile                    # Knowledge 服务
./MemoryKnowledge/docker-compose.yml            # Knowledge 本地开发
./MemoryProxy/Dockerfile                        # Proxy 镜像
./deploy/panel-knowledge-combined/Dockerfile    # Panel + Knowledge 合并 (memory-hub)
./MemoryPanel/docker/local/Dockerfile.local     # Panel 本地模式 (Panel Control HTTP :8123)
```

### 7.3 memory-hub 合并镜像构建（panel-knowledge-combined/Dockerfile）

```dockerfile
# 3 个 builder stage + 1 个 runtime
FROM base AS panel-ui-builder     # npm run build → /build/panel-web/dist
FROM base AS panel-builder        # Panel 后端 tsc + 前端产物 → ./web/dist
FROM base AS knowledge-builder    # Knowledge 后端 tsc
FROM base AS runtime              # COPY 三者 → 单镜像

# 特点：单容器起 Panel + Knowledge，运维简单
```

### 7.4 Panel 本地模式（Dockerfile.local）

```dockerfile
FROM base AS ui-builder
ARG WEB_UI=1
RUN if [ "$WEB_UI" = "0" ]; then
      # 跳过 UI 构建，生成占位 dist（离线/无内部源场景）
      mkdir -p /ui/dist && echo "..." > /ui/dist/index.html;
    else npm install && npm run build; fi

FROM base AS runtime
COPY --from=ui-builder /ui/dist ./web/dist
ENV UI_DIST_DIR=./web/dist
CMD ["node", "--import", "tsx/esm", "src/index.ts"]
```

**双模式**:
- `WEB_UI=1`（默认）：正常构建面板 UI
- `WEB_UI=0`：跳过 UI 构建，仅后端可用（`/api` `/health` 正常，静态面板不可用）

### 7.5 Health Check 策略

```dockerfile
# MemoryCore
HEALTHCHECK --interval=30s --timeout=5s --retries=3 --start-period=15s \
  CMD curl -fsS http://127.0.0.1:${TDAI_GATEWAY_PORT:-8420}/health || exit 1

# MemoryPanel (本地模式)
HEALTHCHECK --interval=15s --timeout=10s --retries=20 --start-period=60s \
  CMD curl -fsS http://127.0.0.1:8123/health || exit 1
```

**注意**: 默认值写在 Dockerfile 而非 ENV，防止镜像环境变量覆盖挂载配置里的 `server.port`。

### 7.6 运行时配置

```dockerfile
# MemoryCore
ENV NODE_ENV=production \
    TDAI_GATEWAY_CONFIG=/data/config/tdai-gateway.yaml \
    TDAI_DATA_DIR=/data/tdai-memory \
    NODE_OPTIONS="--max-old-space-size=1536"

# 数据目录挂载点 (K8s PVC)
RUN mkdir -p /data/tdai-memory /data/config
```

### 7.7 缺失项（DevContainer gap）

| 缺失 | 影响 |
|------|------|
| 无 `.devcontainer.json` | VS Code Remote Container 无法一键起开发环境 |
| 无 docker-compose 全仓编排 | 仅有 MemoryKnowledge 单文件，无跨服务 compose |
| 无 development Dockerfile | 生产镜像不含 devDeps，本地需 tsx 直跑 |
| 无热重载容器配置 | 开发时需手动重启 |
| 无 rootless 指导 | 默认 root 运行，无 `USER node` 建议 |
| 无 trivy/grype 扫描 | CI 中无镜像漏洞扫描 stage |
| 无 distroless 基础镜像 | 用 slim（含 shell），攻击面大于 distroless |
| 无 resource limits 建议 | K8s deployment 无 CPU/Memory request/limit 范例 |
| 无 init container 示例 | DB migration 无 init container 模式 |

---

## 八、CRDT 与多端冲突

### 8.1 现状：无 CRDT，乐观锁 + 版本检查

**搜索结论**：全仓库无 Yjs / Automerge / CRDT 实现，冲突解决依赖 **乐观锁 (optimistic lock)**。

### 8.2 Skill 乐观锁（expected_version）

`MemoryCore/src/core/skill/skill-tools.ts`：

```typescript
// Skill 工具定义中强制要求 expected_version
expected_version: {
  type: "number",
  description: "Required optimistic lock — the version you just read (skill_list/skill_view).
                 After a successful write use the returned version for the next edit."
}
```

**实现**（skill-permission.ts）：
```typescript
export function assertVersionFresh(headRow: Skill, expected: number): void {
  if (expected !== headRow.version) {
    throw new SkillPermissionError("SKILL_VERSION_STALE",
      `expected version ${expected}, head is ${headRow.version}`);
  }
}
```

**语义**: 读取 skill 时记住 `version`；写入时必须带相同 `version`，否则 409 冲突。

### 8.3 Profile Sync 乐观锁

`MemoryCore/src/core/store/types.ts`：

```typescript
/** Profile upsert payload with optimistic-lock baseline from the last pull. */
export interface ProfileUpsertPayload {
  expected_version: number;  // 上次读取的版本基线
  // ... 其他字段
}
```

### 8.4 Skill Bridge 写操作注入

`MemoryProxy/src/skill/skill-bridge.ts`：

```typescript
// Write (update/patch/files_write/files_remove): inject `expected_version` → optimistic lock
// Proxy 层拦截写请求，从 session 上下文中取 expected_version 注入 body
```

### 8.5 Pipeline 并发控制

**Checkpoint 原子操作**（checkpoint.ts）：
```typescript
// Per-file async lock：Promise 链序列化同一文件的并发 read-modify-write
export function withFileLock(filePath: string, fn: () => Promise<T>): Promise<T> {
  // 多个 CheckpointManager 实例共享同文件路径时自动共享锁
}

// Atomic write：先写 tmp 再 rename，防止崩溃时文件损坏
writeRaw() {
  fs.writeFileSync(tmpPath, data);
  fs.renameSync(tmpPath, targetPath);
}
```

### 8.6 请求序号防竞态（前端）

```typescript
// ChatMemoryPanel.tsx
const fetchSeqRef = useRef(0);
const fetchBlocks = useCallback(async () => {
  const seq = ++fetchSeqRef.current;
  const data = await chatMemoryApi.getBlocks(teamId);
  if (seq !== fetchSeqRef.current) return;  // 旧响应丢弃，避免竞态
  setBlocks(data);
}, [teamId]);
```

### 8.7 COS 并发写冲突

`MemoryCore/src/gateway/error-handler.ts`：

```typescript
// COS AppendPositionErr — 并发 append 冲突。客户端应该重试。
if (/AppendPositionErr|Position not equal object length/i.test(err.message)) {
  return {
    status: 409,
    client: { code: 409, message: "Concurrent write conflict, please retry", retryable: true },
  };
}
```

**场景**: 多个 Agent 同时往同一 COS 对象 append，position 不匹配 → 409 + 客户端重试。

### 8.8 缺失项（CRDT gap）

| 缺失 | 影响 |
|------|------|
| 无 Yjs / Automerge | 无法实现真正的多端实时协同编辑 |
| 无 Vector Clock | 无法判断事件的因果序 |
| 无 OT (Operational Transform) | 文档并发编辑需锁，无法无冲突合并 |
| 无 multi-master 同步 | 仅单 instance 写入，无跨地域复制 |
| 无 CRDT-based 计数 | L0 计数器依赖 SQLite 事务，无分布式计数 |
| 无 P2P 同步 | 无客户端间直连同步能力 |
| 无离线编辑 | 断网无法编辑，恢复后无自动合并 |
| 无冲突 UI | 409 冲突时用户需手动重试，无差异对比视图 |
| 乐观锁粒度粗 | Skill 级别版本检查，无字段级 merge |
| 无 causal consistency | 无 happens-before 保证 |

---

## 九、8 维度横向对比总结

### 9.1 与 laew 的 gap 映射

| 维度 | TencentDB-Agent-Memory 现状 | laew 差距 |
|------|---------------------------|----------|
| CrashDump | 无 core dump / 无 Sentry / 指数退避重试完善 | L79: laew 无 panic hook, 无错误聚合 |
| WebUI | 完整 React SPA (tea-component) / 无 Electron | L80: laew 是 TUI (crossterm) |
| Auth | user_key API Key / 无 OAuth | L81: laew 无 auth，直接 Bearer token |
| i18n | react-i18next 双语 / 无 RTL | L82: laew TUI 中文硬编码 |
| Release | SemVer + Docker Hub / 无 AutoUpdate | L83: laew 手动 release |
| WebSocket/SSE | 无 WebSocket / SSE 仅 Form 模拟 | L84: laew 无 SSE 流式 |
| DevContainer | 4 Dockerfile / 无 .devcontainer.json | L85: laew 无容器化 |
| CRDT | 乐观锁 + 版本检查 / 无 CRDT | L86: laew 无多端同步 |

### 9.2 成熟度评分

| 维度 | 评分 | 说明 |
|------|------|------|
| CrashDump / 错误恢复 | 75/100 | 分层分类 + traceId + 指数退避完善；缺 Sentry / 断路器 / CrashDump |
| WebUI / DesktopApp | 70/100 | 完整 React SPA 8 大业务页；缺 Electron / WebSocket / 桌面通知 |
| OAuth / 多账号 | 55/100 | user_key + scrypt 安全基底；缺 OAuth/JWT/rotation/scope |
| i18n | 60/100 | react-i18next + 双语文档；缺 RTL/ICU/翻译平台 |
| Release / AutoUpdate | 50/100 | SemVer + Docker Hub + CI；缺 AutoUpdate/签名/SBOM |
| WebSocket / SSE | 30/100 | SSE 仅 Form 模拟；无服务端推送；轮询为主 |
| DevContainer / 容器化 | 65/100 | 多阶段 Dockerfile + tini + HEALTHCHECK；缺 devcontainer |
| CRDT / 多端冲突 | 40/100 | 乐观锁 + 请求序号；无 CRDT/vector clock/OT |

### 9.3 P0-P2 改造路线图

**P0 紧急**:
- 集成 Sentry（前后端错误聚合）
- 断路器（上游 LLM 持续 5xx 快速失败）
- idempotency-key（L0 写入去重）

**P1 重要**:
- WebSocket/SSE 实时推送（替代前端轮询）
- AutoUpdate 机制（版本检查 + 一键升级）
- .devcontainer.json（开发体验）
- Code Signing + SBOM

**P2 进阶**:
- OAuth 2.0 / SSO 集成
- CRDT (Yjs) 协同编辑
- Electron/Tauri 桌面壳
- RTL + ICU MessageFormat

---

## 十、关键文件索引

| 维度 | 核心文件 | 行数 |
|------|---------|------|
| 错误恢复 | `MemoryCore/src/gateway/error-handler.ts` | ~150 |
| 错误分类 | `MemoryCore/src/core/hooks/recall-errors.ts` | ~120 |
| L0 重试 | `MemoryProxy/src/tdai/pending-writes.ts` | ~110 |
| Graceful Shutdown | `MemoryProxy/src/index.ts:109-130` | ~25 |
| WebUI 入口 | `MemoryPanel/web/src/App.tsx` | ~60 |
| 路由表 | `MemoryPanel/web/src/routes/index.ts` | ~30 |
| LoginGate | `MemoryPanel/web/src/components/LoginGate.tsx` | 382 |
| Panel HTTP | `MemoryPanel/src/panel/http/app.ts` | ~50 |
| 登录页 | `MemoryPanel/web/src/components/LoginGate.tsx` | 382 |
| user_key 生成 | `MemoryCore/src/metadata/utils/crypto.ts` | ~200 |
| Auth 模块 | `MemoryProxy/src/auth.ts` | ~120 |
| Rate Limit | `MemoryProxy/src/rate-limit/redis-store.ts` | ~320 |
| i18n 初始化 | `MemoryPanel/web/src/i18n/index.ts` | ~40 |
| zh-CN 翻译 | `MemoryPanel/web/src/i18n/zh-CN.ts` | 1,113 |
| en-US 翻译 | `MemoryPanel/web/src/i18n/en-US.ts` | 1,160 |
| CI 工作流 | `.github/workflows/pr-ci.yml` | ~150 |
| Docker 发布 | `deploy/dockerhub/publish.sh` | ~100 |
| 一键部署 | `deploy/global-images/start-all.sh` | ~80 |
| MemoryCore Dockerfile | `MemoryCore/Dockerfile` | ~130 |
| Panel Dockerfile | `MemoryPanel/docker/local/Dockerfile.local` | ~80 |
| memory-hub Dockerfile | `deploy/panel-knowledge-combined/Dockerfile` | ~100 |
| 乐观锁 | `MemoryCore/src/core/skill/skill-tools.ts:139` | ~5 |
| 版本检查 | `MemoryCore/src/core/skill/skill-permission.ts:441` | ~10 |
| SSE Form | `MemoryProxy/src/session/form.ts:472` | ~10 |

---

## 十一、与前 9 轮的关系

本轮 8 维度是第 1-9 轮未覆盖的**横向基础设施**维度：

- **第 1-4 轮**: 核心业务逻辑（L0-L3 / Skill / Injection / Proxy）
- **第 5-6 轮**: 存储与检索（多租户 / RRF / 存储层 / Wiki / CodeGraph）
- **第 7-8 轮**: 工程化（插件 / SDK / 部署 / 系统提示词）
- **第 9 轮**: 可观测性（LSP / 遥测 / Web 检索）
- **第 10 轮 (本轮)**: 基础设施（CrashDump / WebUI / Auth / i18n / Release / WS / 容器 / CRDT）

**关键发现**:
1. **错误恢复**做得相当成熟（traceId + 指数退避 + 四级分类），是生产级 Agent 系统的典范
2. **WebUI** 是完整 React SPA 但偏"后台管理系统"，无消费级桌面体验
3. **Auth** 采用简单有效的 user_key API Key，适合 B2B 但不适合 B2C
4. **i18n** 双语齐全但扩展性不足
5. **Release** 工程化程度中等，缺 AutoUpdate/签名/SBOM 等现代供应链安全
6. **实时通信** 是最大短板（无 WebSocket），前端靠轮询
7. **容器化** 成熟，但缺 devcontainer 开发体验
8. **CRDT** 完全缺失，仅乐观锁保护 Skill 写操作
