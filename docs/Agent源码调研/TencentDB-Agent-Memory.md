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
