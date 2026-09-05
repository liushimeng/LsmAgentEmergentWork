# TencentDB Agent Memory 源码深度调研

> **工程定位**：面向 Agent 团队的"团队记忆系统"，核心理念 "Agents remember. Humans innovate."。将项目中已有的信息转化为可复用的"记忆资产"，支持在多个 Agent 和团队成员之间流动、共享和版本化管理。
>
> **调研对象**：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`（v2.0.0-beta.1，2026-08 快照）
>
> **代码规模**：TypeScript 主导的多仓工程，MemoryCore 单仓即 ~10 万行级；MemoryProxy 约 5000 行级；MemoryKnowledge 约 3000 行级；MemoryPanel 约 3000 行级；双 SDK 各约 2000 行级。

---

## 一、工程结构与多仓组织

### 1.1 顶层目录布局

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
│   ├── global-images/      # 三件套一键部署
│   ├── panel-knowledge-combined/
│   └── dockerhub/
└── docs/           # 设计文档（部分）
```

### 1.2 构建系统与工具链

- **包管理**：pnpm workspace（顶层 `pnpm-workspace.yaml` 仅作占位，各子仓独立 `package.json`）
- **构建工具**：`tsdown`（基于 Rolldown 的 TS 打包器）+ `tsx`（直接运行 TS）+ `tsc`（类型检查）
- **测试框架**：vitest（单元 + 集成）+ 独立 e2e 脚本（`__tests__/standalone/e2e.sh`）
- **代码生成**：`@kubb/cli` 从 OpenAPI 生成 TypeScript/SDK 类型
- **运行时要求**：Node.js ≥ 22.16（使用 `node:sqlite` 内置模块 + `sqlite-vec` 扩展）
- **镜像发布**：Docker Hub 公开镜像 `agentmemory/memory-core` / `memory-hub` / `memory-proxy`，支持 amd64 + arm64

### 1.3 子仓职责边界

| 子仓 | 入口文件 | 核心职责 |
|------|---------|---------|
| MemoryCore | `index.ts`（OpenClaw 插件入口）+ `src/gateway/server.ts`（HTTP 服务） | L0-L3 记忆引擎、Skill 系统、向量/BM25 存储、OpenClaw/Hermes 双宿主适配 |
| MemoryKnowledge | `src/server.ts`（Hono HTTP）+ `src/mcp/server.ts`（MCP stdio） | Wiki 摄取/图谱、CodeGraph 索引、知识检索 |
| MemoryPanel | `src/index.ts`（Hono HTTP） | Team/User/Agent/Task 元数据管控、资产治理、可见性 ACL |
| MemoryProxy | `src/index.ts`（Hono HTTP） | LLM 请求转发、上下文注入管线、Session 初始化、可观测性 |

---

## 二、核心架构与模块间通信

### 2.1 整体架构图（文字描述）

```
┌──────────────────────────────────────────────────────────────────────┐
│                        Agent 宿主（OpenClaw / Hermes）                │
│   ┌─────────────┐    ┌──────────────┐    ┌──────────────────────┐   │
│   │ before_prompt│    │  agent_end   │    │  tool_call / result  │   │
│   └──────┬───────┘    └──────┬───────┘    └──────────┬───────────┘   │
└──────────┼──────────────────┼───────────────────────┼───────────────┘
           │                  │                       │
           ▼                  ▼                       ▼
┌──────────────────────────────────────────────────────────────────────┐
│                     MemoryCore（记忆内核 Gateway :8420）              │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │                    TdaiCore（Host-neutral Facade）              │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐  │  │
│  │  │ L0 Recorder│ │ L1 Writer│ │ L2 Scene │ │ L3 Persona   │  │  │
│  │  │ (JSONL)  │  │ (dedup)  │  │ Extractor│ │ Generator    │  │  │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────────┘  │  │
│  │  ┌────────────────────────┐  ┌─────────────────────────────┐  │  │
│  │  │   SkillCore / Skill    │  │  MemoryPipelineManager      │  │  │
│  │  │   Versioning / Store   │  │  (L1→L2→L3 调度)            │  │  │
│  │  └────────────────────────┘  └─────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────────┘  │
│  ┌───────────────┐  ┌──────────────┐  ┌───────────────────────────┐  │
│  │ SQLite Store  │  │ TCVDB Client │  │ EmbeddingService          │  │
│  │ (sqlite-vec)  │  │ (腾讯云向量) │  │ (OpenAI-compatible)       │  │
│  └───────────────┘  └──────────────┘  └───────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
           │                                       │
           ▼                                       ▼
┌─────────────────────────┐         ┌─────────────────────────────────┐
│ MemoryKnowledge (:8424) │         │ MemoryProxy (:8096)             │
│  Wiki + CodeGraph       │         │  InjectionPipeline              │
│  MCP stdio server       │         │  (Skill/Tdai/Knowledge hooks)   │
└─────────────────────────┘         └─────────────────────────────────┘
           │                                       │
           └──────────────┬────────────────────────┘
                          ▼
                ┌──────────────────┐
                │ MemoryPanel (:8125)│
                │ 团队管控 + 资产治理│
                └──────────────────┘
```

### 2.2 模块间通信协议

- **MemoryCore ↔ Agent 宿主**：OpenClaw 走 in-process 插件 API（`OpenClawHostAdapter`）；Hermes 走 HTTP（`StandaloneHostAdapter` + `MemoryTencentdbSdkClient` Python 端）
- **MemoryProxy ↔ MemoryCore**：HTTP `/v3/*` 协议（`TdaiClient` 封装）
- **MemoryPanel ↔ MemoryCore**：HTTP `/v3/meta/*` 元数据路由
- **MemoryKnowledge ↔ Agent**：MCP stdio（`@modelcontextprotocol/sdk`）+ HTTP `/v3/tools/*`
- **MemoryProxy ↔ MemoryKnowledge**：HTTP `/v3/wiki/*` + `/v3/code-graph/*`（`KnowledgeToolsInjector`）

### 2.3 核心门面类：TdaiCore

`MemoryCore/src/core/tdai-core.ts`（1205 行）是 Host-neutral 的统一门面：

```typescript
export class TdaiCore {
  private hostAdapter: HostAdapter;        // OpenClaw / Standalone
  private vectorStore?: IMemoryStore;     // SQLite / TCVDB
  private embeddingService?: EmbeddingService;
  private scheduler?: MemoryPipelineManager; // L1→L2→L3 调度
  private skillCore?: SkillCore;
  private skillExtractor?: SkillExtractor;

  async initialize(): Promise<void>              // 初始化 store + pipeline
  async handleBeforeRecall(userText, sessionKey): Promise<RecallResult>  // 自动召回
  async handleTurnCommitted(messages, ...): Promise<CaptureResult>       // 自动捕获
  async handleMemorySearch(params): Promise<...> // 主动搜索
}
```

关键设计：
- **HostAdapter 抽象**：隔离 OpenClaw / Hermes / Gateway 三种宿主
- **Promise gate 并发保护**：`schedulerStartPromise` 防止并发启动
- **Skill 生命周期钩子**：`SkillAssetHooks` 把 create/access/archive 同步到上层 asset 注册表

---

## 三、L0-L3 记忆模型

### 3.1 四层提炼架构

```
L0 Chat（原始对话）
   │  agent_end hook → l0-recorder.ts
   ▼
L1 Atom（结构化记忆）persona / episodic / instruction / work_*
   │  LLM 提取 + 向量去重 → l1-writer.ts
   ▼
L2 Scenario（场景块）scene_blocks/*.md
   │  LLM 场景抽取 → scene-extractor.ts
   ▼
L3 Persona（用户画像）persona.md
   │  LLM 全局综合 → persona-generator.ts
```

### 3.2 L0 Recorder（`core/conversation/l0-recorder.ts`）

- **存储格式**：JSONL，一行一条消息，按日分片 `conversations/YYYY-MM-DD.jsonl`
- **增量捕获**：双保护机制
  - 位置切片（`originalUserMessageCount`）：缓存 before_prompt_build 时的消息数，只取新增
  - 时间戳游标（`afterTimestamp`）：严格大于上一批 max(timestamp)
- **防污染**：用缓存的 `originalUserText` 替换被 `prependContext` 污染的用户消息
- **幂等性**：同一批次消息要么全写入要么全跳过

### 3.3 L1 Writer（`core/record/l1-writer.ts`）

- **记忆类型**：`persona / episodic / instruction / work_fact / work_task / work_method / work_artifact`
- **优先级**：0-100 数值，-1 表示严格全局指令
- **去重策略**：LLM 驱动的 `DedupDecision`（`store / update / merge / skip`），基于向量相似度 + 关键词冲突检测
- **存储双写**：JSONL 追加（source of truth）+ SQLite vec0 虚拟表（检索引擎）

### 3.4 L2 Scene Extractor（`core/scene/scene-extractor.ts`）

- **LLM 沙箱**：workspaceDir 限制为 `scene_blocks/`，LLM 只能操作 .md 场景文件
- **场景格式**：`scene-format.ts` 定义 frontmatter（summary / heat / created / updated）+ 正文
- **索引维护**：`scene-index.ts` 维护 JSON 索引，LLM 不可见（物理隔离）
- **导航生成**：`scene-navigation.ts` 生成场景导航树注入 Persona 生成 prompt
- **Persona 更新信号**：LLM 输出 `[PERSONA_UPDATE_REQUEST]...[/PERSONA_UPDATE_REQUEST]` 触发 L3

### 3.5 L3 Persona Generator（`core/persona/persona-generator.ts`）

- **触发条件**：每 N 条新记忆（默认 50）或场景变化
- **生成模式**：`first`（首次）/ `incremental`（增量，只分析变化场景）
- **LLM 直写**：通过工具让 LLM 直接把生成的 persona.md 写到 dataDir
- **备份机制**：`BackupManager` 保留最近 N 份画像备份

---

## 四、记忆管线（Pipeline）

### 4.1 MemoryPipelineManager

`src/utils/pipeline-manager.ts` 负责 L1→L2→L3 的调度：

- **触发策略**：每 N 轮对话（默认 5）触发 L1 批处理
- **Warm-up 模式**：新 session 从 1 轮开始，每次 L1 后翻倍（1→2→4→...→everyN），加速早期记忆提取
- **级联延迟**：L1 完成后延迟 N 秒触发 L2；L2 有最小/最大间隔（默认 15min/60min）
- **空闲超时**：用户停止对话后 N 秒（默认 600s）触发 L1 批处理
- **Session 活跃窗口**：超过 N 小时（默认 24h）不活跃的 session 停止 L2 轮询

### 4.2 Checkpoint 机制

`src/utils/checkpoint.ts` 持久化每个 session 的处理进度：

```typescript
interface Checkpoint {
  total_processed: number;      // 已处理消息总数
  last_persona_at?: string;     // 上次 Persona 更新时间
  last_persona_time?: string;
  sessionStates: Map<string, SessionState>; // 每 session 游标
}
```

- **原子操作**：`captureAtomically()` 文件锁保护读-写-推进序列，防止并发重复捕获

### 4.3 Timer Scanner

- **定时清理**：`LocalMemoryCleaner` 按 `l0l1RetentionDays` 清理过期 L0/L1 文件
- **执行时间**：可配（默认 03:00）
- **Skill TTL**：`SkillVersioning` 支持旧版本 TTL 过期

---

## 五、Skill 系统

### 5.1 Skill 生命周期

```
Extract（抽取）→ Review（审核）→ Create/Update/Patch（版本化）
     → Archive（归档）→ Share（跨 Agent 共享）
```

### 5.2 Skill Core（`core/skill/skill-core.ts`）

- **6 个写动作**：create / update / patch / delete / writeFiles / removeFiles
- **4 个读动作**：get / list / search / listVersions / readFile
- **错误码体系**：`SkillCoreErrorCode` 14 种错误（含权限、版本、存储）
- **权限校验**：`skill-permission.ts` 提供 `assertOwner` / `assertTeamMatch` / `assertVersionFresh`（乐观锁）

### 5.3 Skill Versioning（`core/skill/skill-versioning.ts`）

- **单表多行多版本**：`(skill_id, version)` 一行，`is_head=1` 标记当前版本
- **跨系统事务**：COS + skill DB + meta_assets 三系统"顺序 + 补偿"伪事务
  1. 写 COS（最脆先做，失败零副作用）
  2. 写 skill DB（失败反向清 COS）
  3. 挂 asset 钩子（失败反向删 DB + 清 COS）
- **幂等保证**：`IdempotentNoOpError` 当 content_hash 与 head 完全相同时抛出

### 5.4 Skill Extractor（`core/skill/skill-extractor.ts`）

- **LLM 驱动抽取**：把对话 transcript 喂给 LLM，通过 `skill_create / skill_update / skill_patch` 工具持久化
- **Head-tail 截断**：`headChars=8000 + tailChars=32000` 适配 LLM 上下文窗口
- **Recent Skills 注入**：`buildRecentSkillsBlock()` 把已有 skill 作为上下文注入抽取 prompt
- **主 Agent 提示**：`reason` 字段可注入主 Agent 意图说明

### 5.5 Skill 存储（`core/skill/skill-store.ts`）

- **SQLite 实现**：`SqliteSkillStore` 单表 `skills` + FTS5 全文索引 + 可选 vec0 向量索引
- **五元组身份**：`user_id / owner_agent_id / team_id / task_id / skill_id`
- **版本写入**：旧 head 改 0 → INSERT 新行 → fts5 同步（事务原子）
- **TCVDB 实现**：`TcvdbSkillStore` 走腾讯云向量数据库

### 5.6 Skill 文件格式（`core/skill/skill-format.ts`）

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

### 5.7 Skill 共享机制

- **可见性**：`private`（owner 私有）/ `team`（团队共享）/ `restricted`（ACL 精确控制）
- **跨 Agent 借入**：`ChatMemoryAgentRel` 支持借入 ≤2 个 agent 的记忆
- **Asset 注册表**：`meta_assets` + `meta_agent_fixed_assets` 两张元数据表

---

## 六、Wiki 引擎

### 6.1 架构概览（`engines/wiki/manager.ts`）

- **摄取引擎**：`engines/wiki/ingest-v2/index.ts` 编排 `ingestSource()`
- **索引存储**：每个 wiki 私有的 `index.db`（SQLite：`wiki_fts` + `page_meta` + `graph_edge`）
- **图谱构建**：`graphology` 临时内存实例做多跳 BFS（`graph-search.ts`）

### 6.2 摄取流程（Ingest V2）

```
读源文件全文 → 加载模板 schema.md/purpose.md → 扫已有页清单
  → （超长分块）→ 两阶段 LLM 生成 FILE 块：
    阶段 A：分析产出抽取计划
    阶段 B：依据计划生成 FILE 块
  → 解析 FILE 块 → 路径白名单 → dedup（locked / 合并）
  → 写盘（跳过结构性文件）→ 重建 wiki/index.md → 追加 wiki/log.md
```

- **两阶段模式**（默认）：先分析后生成，质量更稳
- **单阶段模式**：源全文直接产出，省 token
- **路径规范化**：`canonicalizePagePath()` 用 frontmatter type + title 重新规范化落盘路径，保证 dedup 不变量
- **合并策略**：`mergePage()` 旧页超阈值走追加模式

### 6.3 检索机制

- **BM25 全文检索**：`wiki_fts` FTS5 虚拟表
- **图谱多跳扩展**：`hop` 参数控制 BFS 深度（0-5），`decay` 控制每跳衰减
- **社区发现**：`graphology-communities-louvain` 做 Louvain 社区划分

---

## 七、CodeGraph 引擎

### 7.1 架构（`engines/code/bridge.ts`）

封装 `@colbymchenry/codegraph` 核心 API：

- **索引操作**：`indexProject()` / `openIndex()` / `syncIndex()` / `closeIndex()`
- **工具执行**：`executeTool(toolName, params)` 复用 `ToolHandler` 格式化输出
- **平台包解析**：`resolveToolsPath()` 从 npm 主包解析平台包路径

### 7.2 MCP 工具集（`src/mcp/tools.ts`）

12 个查询类工具：

- **Code-Graph (8)**：`code_search` / `code_explore` / `code_callers` / `code_callees` / `code_impact` / `code_node` / `code_status` / `code_files`
- **Wiki (4)**：`wiki_search` / `wiki_read` / `wiki_list` / `wiki_graph`

### 7.3 索引池（Instance Pool）

- **懒加载**：`instancePool.loadIfMissing()` 首次访问时打开索引
- **共享队列**：`BuildQueue` 串行 wiki + code 任务
- **Source Fetcher**：`SourceFetcherRegistry` 路由 git/local/ftp，含 SSRF 防护

---

## 八、上下文注入机制（MemoryProxy）

### 8.1 InjectionPipeline（`src/injection/pipeline.ts`）

```
raw body → Adapter.parse() → AgentContext → execute hooks → Adapter.serialize() → modified body
```

- **协议适配器**：`OpenAIAdapter` / `AnthropicAdapter` 双向转换
- **Agent 识别**：URL path 前缀 → `agentProfiles` 快速查找；legacy fallback 扫描 system prompt
- **Hook 执行顺序**：

```
system.prefix → system.before_tools → system.after_tools → system.suffix
→ tools.prepend → tools.append
→ user.first_turn → user.before → user.after
```

### 8.2 Hook 缓存策略

- **`none`**：每次执行
- **`session_init`**：预热一次，后续从 `HookCacheRepo` 读取
- **`hybrid`**：预热 + 实时执行并集去重

### 8.3 内置 Injector

| Injector | 注入点 | 功能 |
|---------|-------|------|
| `SkillInjector` | system.before_tools | 注入 `<available_skills>` 自有 skill 列表 |
| `SkillToolsInjector` | system.before_tools | 注入 `<skill_tools>` curl 调用指南 |
| `TdaiL1RecallInjector` | user.before | 自有 + 借入 L1 记忆召回 |
| `TdaiProfileMemoryInjector` | system.prefix | L3 Persona + L2 场景导航 |
| `TdaiToolsInjector` | system.after_tools | 注入 `tdai_memory_search` 等工具指南 |
| `KnowledgeToolsInjector` | system.after_tools | 注入 Wiki/CodeGraph 工具指南 |
| `AssetReflectionInjector` | system.suffix | 内部效果评估 `<asset_reflection>` |

### 8.4 L1 召回注入器（`tdai-l1-recall-injector.ts`）

```typescript
// 核心流程
const ctxs = await resolveFixedAssetCtxs(ctx, identity, mc); // self + 借入 ≤2
const groups = await Promise.all(
  ctxs.map(c => this.client.searchL1ForCtx(c, query, ...))   // 并发搜索
);
const merged = [].concat(...groups).sort(by score).slice(0, globalTopK);
// 注入 <tdai_recalled_l1_memories> 块
```

- **ACL 校验**：`aclClient` 参数对每个 fixed-asset ctx 走 `acl/check(read)`
- **降级策略**：控制面不可达时仅查当前 agent 的 L1

### 8.5 Session 上下文注入（`session/context-injector.ts`）

- **Agent/Task 身份**：`<session_context>` 标记包裹，注入到每个请求
- **去重保证**：per-session dedup 防止重复注入
- **与 injection 管线分离**：session 身份是必选项，不走可选的 hook 管线

---

## 九、检索机制（BM25 + 向量 + RRF）

### 9.1 混合检索架构

```
用户查询
   ├─ FTS5 BM25（关键词）──┐
   │                       ├─ RRF 融合 → 排序截断
   └─ vec0 向量（语义）────┘
```

### 9.2 RRF 实现（`core/store/search-utils.ts`）

```typescript
export const RRF_K = 60;  // 标准 RRF 常数

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
- **k=60**：平滑低排名项权重

### 9.3 BM25 本地编码（`core/store/bm25-local.ts`）

- **纯 TS 实现**：`BM25LocalEncoder` 基于 `@tencentdb-agent-memory/tcvdb-text`
- **双模式**：`encodeTexts()`（文档，TF-based）/ `encodeQueries()`（查询，IDF-based）
- **语言支持**：中文（jieba 分词）/ 英文

### 9.4 FTS5 查询构建（`core/store/sqlite.ts`）

- **中文分词**：`@node-rs/jieba` search-engine 模式（`cutForSearch`）
- **停用词过滤**：`ZH_STOP_WORDS` 集合
- **Query 构建**：tokens OR-joined 作为 quoted FTS5 phrase terms
- **Fallback**：jieba 不可用时回退到 Unicode-regex 分词

### 9.5 Embedding 服务（`core/store/embedding.ts`）

- **多 Provider**：OpenAI-compatible（`@ai-sdk/openai`）
- **代理支持**：`proxyUrl` 通过本地代理转发
- **维度适配**：支持 Matryoshka 截断（`sendDimensions` 开关）
- **超时分级**：全局 / recall / capture 三档超时

---

## 十、存储设计

### 10.1 SQLite 存储（`core/store/sqlite.ts`）

**四表结构**：

| 表 | 用途 | 索引 |
|---|------|------|
| `l1_records` | L1 结构化记忆元数据 | 主键 + 多维度索引 |
| `l1_vec` | L1 向量索引 | vec0 虚拟表（cosine） |
| `l0_conversations` | L0 原始对话元数据 | session_key + timestamp |
| `l0_vec` | L0 向量索引 | vec0 虚拟表 |

**附加表**：
- `skills` + `skill_fts` + `skill_vec`：Skill 主表 + 全文索引 + 向量索引
- `profiles`：L2/L3 画像文件元数据
- `memory_audit`：审计日志

**关键设计**：
- **同步 API**：`DatabaseSync`（Node 22+ `node:sqlite`）
- **WAL 模式**：线程安全
- **手动事务**：BEGIN/COMMIT 保证 metadata + vector 原子写入
- **upsert = delete + insert**：vec0 不支持 ON CONFLICT

### 10.2 TCVDB 存储（`core/store/tcvdb.ts`）

腾讯云向量数据库后端：

- **服务端 embedding**：`embeddingItems` 配置
- **客户端稀疏向量**：BM25 local encoder
- **原生混合搜索**：`hybridSearch`（dense + sparse + RRFRerank）
- **Filter 表达式**：标量字段过滤

### 10.3 Knowledge DB（`MemoryKnowledge/src/db/schema.ts`）

Drizzle ORM 4 表：

| 表 | 用途 |
|---|------|
| `knowledge_code_graph` | 代码仓库索引元数据 |
| `knowledge_wiki` | Wiki 知识库元数据 |
| `knowledge_wiki_audit` | Wiki 状态变更审计 |
| `knowledge_code_graph_audit` | CodeGraph 状态变更审计 |
| `llm_binding` | Per-instance LLM 路由绑定 |

- **软删除**：`deleted_at` + partial unique index
- **版本乐观锁**：`version` 字段

### 10.4 多租户隔离（`core/store/isolation.ts`）

**三维隔离**：`(team_id, user_id, agent_id)`

```typescript
interface IsolationContext {
  teamId?: string;
  userId: string;        // 必填
  agentId: string;       // 必填
  sessionId: string;     // 必填
  taskId?: string;
  sessionKey?: string;
}
```

- **强制校验**：`assertIsolation()` 缺失必填字段时抛 `IsolationError`
- **Legacy 兼容**：`legacyCompatMode` 用占位符填充
- **查询过滤**：`buildIsolationWhere()` 动态构建 WHERE 子句

---

## 十一、MCP 协议

### 11.1 Knowledge MCP Server（`MemoryKnowledge/src/mcp/server.ts`）

- **传输方式**：`StdioServerTransport`（独立进程）
- **工具转发**：`callApi()` 把 MCP tool call 转发到 Hono HTTP API
- **工具列表**：`ListToolsRequestSchema` 返回 12 个工具定义
- **错误处理**：`{text, isError}` 透传

### 11.2 MCP 工具定义（`src/mcp/tools.ts`）

```typescript
export interface McpToolDef {
  name: string;
  description: string;
  inputSchema: { type: "object"; properties: ...; required: string[] };
  endpoint: string;  // HTTP endpoint（如 /code-graph/search）
}
```

- **查询类工具**：只读操作，管理操作（create/delete/sync）不暴露
- **HTTP 桥接**：MCP tool call → HTTP POST → Knowledge API

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

## 十四、多租户/ACL

### 14.1 多级可见性

```typescript
type AssetVisibility = "private" | "team" | "restricted" | "agent" | "task";
```

- **`private`**：仅 Owner 可访问
- **`team`**：团队内所有成员可见
- **`restricted`**：通过 ACL 精确控制（User / Role / Agent）
- **`agent`**：绑定到特定 Agent
- **`task`**：绑定到特定任务

### 14.2 ACL 模型（`metadata/types.ts`）

```typescript
type Permission = "read" | "write" | "delete" | "assign" | "share" | "use";
type AclSubjectType = "user" | "team_role" | "agent";
type AclEffect = "allow" | "deny";  // 一期仅 allow，deny 预留
```

### 14.3 角色分层

- **全局层**：`system_admin` 管理用户和团队
- **团队层**：`admin`（团队管理者）/ `member`（普通成员）/ `reviewer`（审核员）
- **资产层**：`owner` 自动拥有管理权限

### 14.4 权限校验（`metadata/service/permission-checker.ts`）

- **Asset 权限**：`checkAssetPermission(user_key, asset_id, action)`
- **用户可见性**：`user-visibility.ts` 控制用户能看到哪些资产
- **ACL 检查**：Proxy 层 `aclClient.aclCheck()` 每个 fixed-asset ctx 走读权限校验

---

## 十五、可观测性

### 15.1 OpenTelemetry

- **初始化**：`initTelemetry()`（`MemoryKnowledge/src/telemetry.ts`）
- **Span 导出**：OTLP HTTP/gRPC exporter
- **Langfuse 集成**：`@langfuse/otel` SpanProcessor

### 15.2 Langfuse（`MemoryProxy/src/langfuse.ts`）

- **Trace 语义**：一个 trace = 一个 session 里的一次用户输入（一个 turn）
- **确定性 traceId**：`sessionKey + turnSeq` → SHA-256 → 前 32 位 hex
- **跨请求归并**：同一 turn 内工具循环的多次 LLM 调用共享 traceId
- **Graceful degradation**：配置缺失 / SDK 初始化失败时 no-op

### 15.3 Opik（`MemoryProxy/src/opik.ts`）

- **独立 traceId**：与 Langfuse 完全独立
- **LLM Span**：`opikCreateLlmSpan()` 记录每次 upstream 调用
- **Trace 更新**：`opikUpdateTrace()` 附加 metadata

### 15.4 ClickHouse（`MemoryProxy/src/clickhouse.ts`）

- **请求日志**：结构化写入 ClickHouse
- **失败上报**：`writeFailedReportRaw()` 即时写入

### 15.5 指标上报（MemoryCore）

- **Metric JSON**：结构化日志输出
- **Kafka**：`kafka-metric-producer.ts` 上报
- **分类指标**：
  - `metric-tracking-recall.ts`：召回指标
  - `metric-tracking-l2-latency.ts`：L2 延迟
  - `metric-tracking-l3-latency.ts`：L3 延迟

### 15.6 日志系统（`MemoryProxy/src/report/`）

- **结构化日志**：`file-logger.ts` + 多后端（`backends/`）
- **日志轮转**：`rotate` 配置
- **请求日志**：`requestLog.ts` 记录原始请求

---

## 十六、部署架构

### 16.1 Docker 三件套

```
docker network: tdai-memory-stack
├── tdai-memory-core  (:8420)  — 记忆内核 Gateway
├── tdai-memory-hub   (:8125)  — 面板 + 知识服务
└── tdai-proxy        (:8096)  — 上下文注入代理
```

### 16.2 一键部署脚本（`deploy/global-images/start-all.sh`）

```bash
./start-all.sh            # 本地镜像直接起
PULL=1 ./start-all.sh     # 先 pull 最新镜像
```

**启动顺序**：
1. `start-memory-core.sh`：起 memory → healthy 检查
2. `start-memory-hub.sh`：起 memory-hub → healthy 检查
3. `start-proxy.sh`：起 proxy（默认开 full stack：auth + sessionInit + tdai 注入）

### 16.3 环境变量配置（`.env.example`）

**两组独立 LLM 参数**：
- `MEMORY_*`：memory + memory-hub 内部调用（embed / summarize / wiki ingest）
- `PROXY_*`：proxy 转发到的上游 LLM（coding agent 请求）

**关键配置项**：
- 镜像 tag（默认 `latest`，可固定版本）
- 端口映射
- 数据卷名
- admin user_key（首次 init-admin 自动生成）
- Bearer gate（默认空，关闭）

### 16.4 数据持久化

- **Named Volume**：`tdai-memory-core-data` / `tdai-panel-data`
- **admin key 持久化**：`./.admin-key` 文件
- **容器重启策略**：`rm_container_if_exists` + volume 保留

### 16.5 客户端接入

```bash
# 通过 proxy 用 Claude Code
export ANTHROPIC_BASE_URL=http://127.0.0.1:8096/claude-code/default
export ANTHROPIC_AUTH_TOKEN='<admin-key>'
claude --model <upstream-model>
```

---

## 十七、对 laew 工程的借鉴建议

### 17.1 可直接借鉴的设计模式

#### 17.1.1 L0-L3 四层记忆提炼

**laew 现状**：当前仅有 `session_memory` 表存摘要，无分层提炼。

**借鉴点**：
- **L0 原始对话**：JSONL 按日分片，增量捕获（位置切片 + 时间戳游标双保护）
- **L1 结构化记忆**：LLM 抽取 + 向量去重（`store / update / merge / skip` 四态决策）
- **L2 场景块**：LLM 沙箱限制 workspaceDir，物理隔离元数据文件
- **L3 画像综合**：增量模式只分析变化场景，降低 token 消耗

**落地建议**：
- 在 `src/agent/memory/` 新增 `l0-recorder.ts` / `l1-writer.ts` / `l2-scene.ts` / `l3-persona.ts`
- 存储用 SQLite（laew 已有 `LsmAgentEmergentWork.db`）+ sqlite-vec 扩展
- 引入 `node:sqlite`（Node 22+ 内置）

#### 17.1.2 RRF 混合检索

**laew 现状**：无记忆检索，仅靠 SessionContext 注入最近 N 条摘要。

**借鉴点**：
- FTS5 BM25（关键词）+ vec0 向量（语义）+ RRF 融合
- 标准 RRF 公式：`score = Σ 1/(k + rank + 1)`，k=60
- 中文 jieba 分词 + 停用词过滤

**落地建议**：
- 新增 `src/agent/memory/rrf.ts` 实现 `rrfMerge()`
- 在 `src/agent/tools/` 新增 `memory_search` 工具
- 召回结果注入 system prompt（带 `<<<LAEW:RECALLED_MEMORIES>>>` 标记）

#### 17.1.3 上下文注入管线

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

#### 17.1.4 Skill 版本化系统

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

#### 17.1.5 多租户隔离

**laew 现状**：单用户单设备，无多租户概念。

**借鉴点**：
- **三维隔离**：`(team_id, user_id, agent_id)`
- **强制校验**：`assertIsolation()` 必填字段
- **Legacy 兼容**：`legacyCompatMode` 占位符填充

**落地建议**：
- 当前阶段可简化为 `(user_id, agent_id)` 二维
- 在 `Session` 中增加 `agentId` 字段
- 记忆查询强制带 agentId 过滤

### 17.2 架构层面的启示

#### 17.2.1 Host-neutral 门面模式

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

#### 17.2.2 Promise Gate 并发保护

`schedulerStartPromise` 防止并发启动的模式，适用于 laew 的 `MultiAgentOrchestrator`：

```typescript
private schedulerStartPromise?: Promise<void>;
async ensureSchedulerStarted() {
  if (this.schedulerStartPromise) return this.schedulerStartPromise;
  this.schedulerStartPromise = this.doStart();
  return this.schedulerStartPromise;
}
```

#### 17.2.3 Checkpoint 原子操作

`captureAtomically()` 文件锁保护读-写-推进序列，适用于 laew 的 Session 状态持久化。

### 17.3 工具与存储建议

#### 17.3.1 新增工具清单

| 工具 | 用途 | 优先级 |
|------|------|-------|
| `memory_search` | L1 记忆混合检索 | P0 |
| `memory_write` | 手动写入/修改记忆 | P0 |
| `skill_create` | 创建 Skill | P1 |
| `skill_search` | 搜索 Skill | P1 |
| `persona_read` | 读取当前 Persona | P1 |
| `scene_list` | 列出场景块 | P2 |

#### 17.3.2 SQLite 表扩展

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

### 17.4 反模式警示

1. **不要过度设计**：TencentDB 的 6 个子系统（Core/Knowledge/Panel/Proxy + 双 SDK）对 laew 来说过重。建议先做 L1 记忆 + RRF 检索两个核心能力。
2. **避免 LLM 沙箱逃逸**：TencentDB 的"LLM 直写文件"模式需要严格的 workspaceDir 限制，laew 当前零沙箱，引入时需同步建设权限管控。
3. **警惕向量维度漂移**：embedding provider/model/dimensions 变化时需要 reindex，laew 需设计 `needsReindex` 检测机制。
4. **防止记忆注入污染**：TencentDB 用 `sanitizeText` + `stripCodeBlocks` 防止注入标记被 LLM 重复注入，laew 的 `<<<LAEW:PROJECT_CONTEXT>>>` 也需类似处理。

### 17.5 推荐落地路线图

```
P0（1-2 周）：L0 对话录制 + L1 记忆表 + RRF 混合检索
P1（2-4 周）：L2 场景块 + Skill 系统 + memory_search 工具
P2（1-2 月）：L3 画像 + 上下文注入管线 + 多租户隔离
P3（2-3 月）：Wiki/CodeGraph 知识引擎 + MCP 服务
```

---

## 十八、总结

TencentDB Agent Memory 是一个**工程完整度高、设计模式成熟**的团队记忆系统。其核心亮点包括：

1. **L0-L3 四层提炼管线**：从原始对话到用户画像的渐进式抽象
2. **BM25 + 向量 + RRF 混合检索**：关键词与语义的互补融合
3. **上下文注入管线**：协议适配器 + Hook 注册 + 缓存策略的三层抽象
4. **Skill 版本化系统**：跨系统事务 + 乐观锁 + ACL 的完整生命周期
5. **多宿主适配**：OpenClaw / Hermes / Gateway 三种宿主的统一门面
6. **可观测性**：OpenTelemetry + Langfuse + Opik + ClickHouse 四件套

对 laew 而言，**L1 结构化记忆 + RRF 混合检索 + 上下文注入管线** 是最具借鉴价值的三个方向，可显著提升 Agent 的"记忆"能力，解决当前"每次新 Session 从零开始"的痛点。

---

*调研人：Claude Code Agent*
*调研日期：2026-09-05*
*源码路径：`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory`*
