# 专题-第八轮-Skill 一等公民 / Skills Catalog / Workshop 自演化 / SkillDevPipeline 深度对比

> 日期：2026-09-07
> 作者：源码深挖 SubAgent（第八轮）
> 范围：8 个工程 — atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / jiuwenswarm / TencentDB-Agent-Memory
> 重点：**SkillDevPipeline 12 阶段**（jiuwenswarm）+ **Workshop 自演化闭环**（openclaw 深入）+ **SkillCore 6 写 4 读**（TencentDB）+ **Skill 单元测试 / 评测 / 安全扫描**（全部工程横向）

## 与前两轮专题的边界

| 专题 | 范围 | 与本文档的关系 |
|---|---|---|
| 第五轮 `Skill系统深度分析.md`（1578 行） | Skill 全栈初探 | 仅作背景参考 |
| 第六轮 `专题-第六轮-Skill系统深度对比.md`（2290 行） | Skill 文件格式 / 6 源加载管线 / 4 触发方式 / 8KB 预算 / hot-reload / skillify / MCP 双向桥 | **不重复** |
| 第七轮 8 个专题 | 与 Skill 无关（编辑 / 检索 / Git / Bash / 多模态 / Caching / Schema / Web） | 互不重叠 |
| **本轮** | **SkillDevPipeline 12 阶段 / Workshop 12 阶段闭环 / SkillCore 6 写 4 读 / Skill 单元测试 / 描述优化 / 安全扫描器实现 / laew 路线图** | **唯一覆盖** |

---

## 目录

1. 摘要与 TL;DR
2. 背景：Skill 是 Agent 的「可组合积木」——为何需要 pipeline / 评测 / 闭环节
3. 每个工程的实际实现
   - 3.1 atomcode：SkillFirstHook + SkillCatalogHook
   - 3.2 claudecode：skillify + quick_validate.py + skill-creator
   - 3.3 deepseek-harness：4 包分层 + tool-skill `/name` + chokidar hot-reload
   - 3.4 openclaw：**Workshop 12 阶段自反馈闭环**（本轮最重头）
   - 3.5 opencode：SkillV2 + Discovery + Effect DI
   - 3.6 pi：Agent Skills 标准 + 一等公民 + 6 源注入
   - 3.7 jiuwenswarm：**SkillDevPipeline 12 阶段确定性状态机**（本轮最重头）
   - 3.8 TencentDB-Agent-Memory：**SkillCore 6 写 4 读 + SkillExtractor 抽取链路**
4. 横向对比大表（8 工程 × 7 维度）
5. SkillDevPipeline 12 阶段详解（jiuwenswarm 提炼）
6. Workshop 自演化闭环详解（openclaw 提炼）
7. SkillCore 6 写 4 读详解（TencentDB 提炼）
8. Skill 单元测试 / 评测体系
9. Skill 安全：签名 / 来源 / 沙箱 / 审计
10. 共性模式（5 大共性）
11. laew 现状差距与 P0/P1/P2 路线图
12. 附录 A：关键代码路径速查表（按工程）
13. 附录 B：关键文件行数表

---

## 1. 摘要与 TL;DR

**TL;DR**：

- **「Skill 一等公民」≠ 把 skill 当成 markdown 文件**。它意味着完整的资产化：`name + description + version + assets + signature`，外加 **生命周期管理**（创建 → 测试 → 评测 → 改进 → 打包 → 描述优化 → 部署 → 监控 → 退役）。
- **三大参考实现**：
  - **jiuwenswarm SkillDevPipeline**（**12 阶段**确定性状态机，INIT/PLAN/PLAN_CONFIRM/GENERATE/VALIDATE/TEST_DESIGN/TEST_RUN/EVALUATE/REVIEW/IMPROVE/PACKAGE/DESC_OPTIMIZE，3 个挂起点，2700+ 行 Python，~85% 单元测试覆盖，**官方 anthropic skill-creator 的 1:1 内化**）
  - **openclaw Workshop**（**12 阶段**提案-草稿-策略-评估-经验-策展-历史扫描-应用-回滚-过渡-审计-hook 派发，80+ 文件，~18000 行 TypeScript，**全状态机 + 提案预算 + 隔离执行 + proposalOnly reviewer**）
  - **TencentDB-Agent-Memory SkillCore**（**6 写 4 读**门面 + 乐观锁 + 跨系统事务（store + COS + asset）+ 完整版本链，~3900 行 TypeScript）
- **Skill 测试**：三档 — (1) 单元测试（pi 432 行 / opencode 585 行 / openclaw 194+189+302+260+611+217=~1773 行）覆盖 frontmatter 校验 / 路径安全 / 加载 / hash 不变性；(2) **Eval 集**（jiuwenswarm `evals.json` + `grader Agent` + `analyst Agent` + `benchmark.json` + `with_skill vs baseline` 对照实验）；(3) **安全扫描**（openclaw scanner.ts 1038 行 — 5 类规则 LINE_RULES / SOURCE_RULES / SKILL_CONTENT_RULES，provenance-aware child_process 检测）。
- **Skill 描述优化**：jiuwenswarm DESC_OPTIMIZE 阶段对标官方 `improve_description.py`：20 个 trigger queries（10 should_trigger + 10 should_not_trigger）→ 60/40 train/test split → 最多 5 轮 eval→improve 循环 → 选 test score 最高的 description（防过拟合）。
- **Skill 安全**：签名 / 来源校验 / 路径约束 / 字节上限（openclaw `maxSkillBytes=40_000` / jiuwenswarm `SKILL_NAME_MAX_LEN=64` + `SKILL_DESC_MAX_LEN=1024`） / 反 prompt-injection（`ignore all previous instructions` / `hidden system prompt` / `pipe-to-shell` 规则） / child_process provenance 追踪。
- **laew 差距**：当前**零 Skill 系统**（CLAUDE.md 无任何 skill 模块 / 文档），P0 = 引入 SKILL.md frontmatter + 5 源发现链 + 注入到 system prompt；P1 = 描述优化 + Eval 集 + 安全扫描；P2 = SkillDevPipeline 化（create / test / improve / package 全流程）。

---

## 2. 背景：Skill 是 Agent 系统的「可组合积木」

### 2.1 从 PoC 到生产级 Agent 的必经之路

一个 Agent CLI 从「能跑通 LLM 循环」到「生产力工具」必须解决以下问题：

1. **重复利用**：相同领域（如 `arxiv_searcher` / `github_pr_reviewer`）每次都靠 prompt 临时组装？→ 沉淀为 **Skill 包**（SKILL.md + 脚本 + 数据）。
2. **能力边界**：模型不知道有哪些 skill 存在 → **SkillCatalog** 必须显式注入 system prompt。
3. **质量保证**：用户怎么知道这个 skill 真的能用？→ **Eval 集 + Grader + Analyst** 三件套（with_skill vs baseline 对照）。
4. **持续改进**：用着用着发现 bug 怎么办？→ **Workshop / SkillDevPipeline 改进循环**（propose → evaluate → apply → rollback）。
5. **安全**：skill 是 markdown 写的，可以被注入任意指令 → **安全扫描器**（child_process / env 泄露 / prompt-injection）。

### 2.2 Skill 是「一等公民」的判据

对照 8 个工程，「一等公民」至少需要满足：

| 能力 | 含义 | 实现示例 |
|---|---|---|
| 1. 文件格式 | frontmatter 强制 + 校验 | jiuwenswarm `validate_skill_md` (validate_stage.py:46-153) |
| 2. 多源发现 | 项目 / 用户 / 内置 / 远程 | pi 6 源 / opencode 3 Source / atomcode 12 目录 |
| 3. 注册 / 加载 | LRU + 缓存 + 失效 | deepseek `revision + LRU` / opencode `SkillV2 Layer` |
| 4. 触发模型 | 自动匹配 / 显式 / slash | openclaw `use_skill` / pi `<skill name>` / atomcode `use_skill` tool |
| 5. 生命周期 | propose / apply / rollback / retire | openclaw Workshop / TencentDB version chain |
| 6. 改进闭环 | test → eval → improve → package | jiuwenswarm SkillDevPipeline 12 阶段 |
| 7. 测试 | 单元 / Eval / 扫描 | pi 432 / opencode 585 / openclaw scanner 1038 |
| 8. 安全 | 签名 / 来源 / 沙箱 / 审计 | openclaw 5 类规则 + 提案锁 |

### 2.3 三类典型架构

- **「轻量 Skill」**（claudecode / pi / opencode）：SKILL.md 即一切；6 源发现；运行时注入；评测由 skill-creator skill 承担。
- **「中量 Catalog + Hook」**（atomcode / deepseek-harness）：Registry + Provider + chokidar 热重载；Hook 注入 catalog；revision 链。
- **「重量 Workshop + Pipeline」**（openclaw / jiuwenswarm / TencentDB）：完整状态机 + 提案预算 + 跨系统事务 + 评测 + 描述优化。

---

## 3. 每个工程的实际实现

### 3.1 atomcode：SkillFirstHook + SkillCatalogHook

**核心目录**（`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/skills/`）：

| 文件 | 行数 | 职责 |
|---|---|---|
| `skill.rs` | 519 | Skill 数据结构 / 变量替换 / 模板 |
| `registry.rs` | 571 | 12 目录标准发现链 |
| `render.rs` | 401 | 8KB 预算门控 / 源优先级 / catalog 渲染 |
| `use_skill.rs` | 316 | use_skill 工具执行 |
| `catalog_hook.rs` | 128 | **SkillCatalogHook：会话启动时注入** |
| `skill_first.rs` | 146 | **SkillFirstHook：开轮强制 skill-first 提醒** |

**SkillFirstHook**（`atomcode-coding/src/skill_first.rs:1-146`）—— atomcode 独特设计：

```rust
// 强制在开轮（turn=1, round=1）时给模型注入一个 <system-reminder>：
// "Before you explore the codebase, plan, or propose a solution: check the
// '=== AVAILABLE SKILLS ===' catalog above. If this request matches a skill's
// description shown in that catalog, you MUST call `use_skill` with that
// exact listed name NOW and let it drive."
//
// 仅当 (a) 模型是 firm-execution（DeepSeek / Qwen）且 (b) catalog 非空时启用。
// 用 synthetic_system_reminder 包装注入到 user 末尾 —— 利用 OpenAI-compatible
// 协议允许连续 user 消息的特性。
//
// 关键 SAFETY INVARIANT：若 Anthropic-strict 模型被加入 firm_execution，
// 必须在 round=1 也 gate 掉，否则连续 user 消息会被协议拒绝。
```

**SkillCatalogHook**（`atomcode-capabilities/src/skills/catalog_hook.rs:1-128`）—— catalog 注入 hook：

```rust
// 关键设计：position-based reconcile
// - fresh 会话（catalog 块不存在）→ 在 leading-system run 末尾插入
// - resume 会话（catalog 块存在）→ 原地替换（byte-identical 不变）
// - resume 但 catalog 现在为空（skill 全删了）→ 删掉 stale block
//
// 用 CATALOG_HEADER 字符串识别（render.rs:1-25），用 leading_system_count
// 找到插入点。完美解决 "session_start 幂等" 问题。
```

**测试覆盖**：`catalog_hook.rs:80-128` 含 `fresh_inserts_after_leading_system_run` / `none_catalog_is_noop_on_fresh` 两个单元测试。

### 3.2 claudecode：skillify + quick_validate.py + skill-creator

**核心目录**（`/usr/local/LsmGitOpenSource/claudecode/src/skills/`）：

| 文件 | 职责 |
|---|---|
| `loadSkillsDir.ts` | 6 源加载管线 / 7 去重 / symlink 保护 |
| `bundledSkills.ts` | 16 个内置 SkillDefinition / 资源懒提取 |
| `skill-creator` skill | 内嵌 /skillify（自我扩展） |
| `quick_validate.py` | frontmatter 校验器（Python fallback） |

**skill-creator 的 workflow**（`/usr/local/LsmGitOpenSource/openclaw/skills/skill-creator/SKILL.md`，与 claudecode 同源）：

```markdown
# Skill Creator
## Workflow
1. Establish the contract.
2. Choose invocation.
3. Structure the skill.
4. Draft and persist.
5. Validate.  ← python {baseDir}/scripts/quick_validate.py
```

**quick_validate.py** 关键函数（`scripts/quick_validate.py:1-50`）：

```python
MAX_SKILL_NAME_LENGTH = 64

def _extract_frontmatter(content: str) -> Optional[str]:
    lines = content.splitlines()
    if not lines or lines[0].strip() != "---": return None
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return "\n".join(lines[1:i])
    return None

def _parse_simple_frontmatter(text: str) -> Optional[dict[str, str]]:
    """minimal fallback when PyYAML unavailable"""
```

**测试**：claudecode 没有专门的 skill 测试文件（其架构假设 skill 在运行时才出现，依赖加载器自检），但 init.ts 提示用户用 `/plugin install skill-creator@claude-plugins-official` 获得官方评测能力。

### 3.3 deepseek-harness：4 包分层 + tool-skill `/name` + chokidar hot-reload

**4 个包**（`/usr/local/LsmGitOpenSource/deepseek-harness/packages/skill/`）：

| 包 | 职责 |
|---|---|
| `skill/` | 核心 registry + 5 层 SkopedLayers + revision + LRU |
| `skill-filesystem/` | FileSystemSkillProvider + chokidar 监视 |
| `tool-skill/` | 注入到 LLM 的 tool（`/name` 显式触发） |
| `skill-badge/` | UI badge 展示 |

**5 层 SkopedLayers**（`skill/src/index.ts:327-461`）—— 分层注入 skill：

```typescript
// 1. User 层（~/.deepseek/skills/）
// 2. Project 层（.deepseek/skills/）
// 3. Workspace 层（cwd/skills/）
// 4. Bundle 层（plugin 内置）
// 5. Builtin 层（hardcoded）
// 每层独立 SkopedLayer；session 创建时按层级合并
```

**`/name` 显式触发**（`tool-skill/src/index.ts:177-204`）：

```typescript
// 用户在消息中带 "/<skill-name>" 前缀 → 直接加载该 skill 的 SKILL.md 内容
// 这是 opencode `Skill.tool` 和 atomcode `use_skill` 的同构实现
```

**Chokidar 热重载**（`skill-filesystem/src/index.ts:284-597`，444 行测试）：

```typescript
// 监听 skills 目录，文件 mtime/size 变化时 → invalidate LRU → 触发 L3 重新扫描
// 区分 add / change / unlink；保留 watch 实例复用
```

### 3.4 openclaw：**Workshop 12 阶段自演化闭环**（本轮最重头之一）

**Workshop 目录**（`/usr/local/LsmGitOpenSource/openclaw/src/skills/workshop/`）—— 80+ 文件，~18000 行 TypeScript：

| 文件 | 职责 |
|---|---|
| `service.ts` | 编排主入口（revise / reject / quarantine / apply） |
| `service-propose.ts` | propose 阶段（composeSkillBodyPatch / proposeCreateSkill / proposeUpdateSkill） |
| `proposal-draft.ts` | draft 阶段（nextProposalVersion / prepareSkillProposalDraft） |
| `proposal-hash.ts` / `revision-hash.ts` | 内容 / 修订 hash |
| `policy.ts` / `policy.runtime.ts` | 策略机（agent-filter / bin 依赖 / OS 限制） |
| `service-evaluation.ts` | evaluate 阶段（evaluateSkillProposal / MAX_EVALUATION_OUTCOMES=64） |
| `store-evaluation.ts` | evaluation 持久化 |
| `experience-review.ts` / `experience-review-scheduler.ts` | 经验评估（按 cron 调度） |
| `history-scan.ts` / `history-scan-candidates.ts` / `history-scan-progress.ts` | 历史会话扫描 |
| `curator.ts` | 策展（active / archived / stale） |
| `apply-transition.ts` / `reconcile-transition.ts` | apply 阶段 + 重对账 |
| `plugin-hooks.ts` | 派发 `proposal-changed` / `skill_proposal_evaluate` |
| `proposal-scan.ts` | 安全扫描（proposal 提交时） |
| `proposal-origin-validation.ts` | 来源校验 |
| `collection-backup.ts` / `collection-restore.ts` / `collection-rollback.ts` | 集合备份 / 恢复 / 回滚 |
| `collection-review-state.ts` | 集合审阅状态（记录 review outcome） |
| `workspace-skill-read.ts` / `workspace-skill-write.ts` | 写 workspace skill 的核心 API |
| `store-sqlite-event.ts` / `store-sqlite-record.ts` / `store-sqlite-rollback.ts` / `store-sqlite-transition.ts` | SQLite 持久化 4 件套 |
| `model-context-budget.ts` | 投影到 LLM 上下文时的预算 |
| `config.ts` | Workshop 配置（autonomous.mode / approvalPolicy / maxPending / maxSkillBytes） |
| `types.ts` | 全部类型 / schema / 事件类型 |
| `skills-root.ts` | 解析 workshop skills 根目录（per-agent） |
| `target-lock.ts` | per-target 写锁 |
| `review-run.ts` / `review-outcome.ts` / `maintenance-prompt.ts` / `learn-prompt.ts` | review 工具链 |
| `scanner.ts`（在 `security/`） | **1038 行安全扫描器**（5 类规则） |
| `proposal-tree-readers.test.ts` 等 30+ 个 `.test.ts` | 单元测试 |

**Workshop 状态机**（`types.ts:12-19`）：

```typescript
type SkillProposalKind = "create" | "update";
export type SkillProposalStatus = "pending" | "applied" | "rejected" | "quarantined" | "stale";
```

**apply 阶段状态转换表**（`apply-transition.ts:62-78`）：

```typescript
const SKILL_PROPOSAL_APPLY_TRANSITIONS: Readonly<
  Record<SkillProposalStatus, Partial<Record<SkillProposalApplyOutcome, SkillProposalStatus>>>
> = {
  pending: {
    apply_failed: "pending",
    apply_succeeded: "applied",
    scan_failed: "quarantined",
    target_changed: "stale",
  },
  applied: {},
  rejected: {},
  quarantined: {},
  stale: {},
};
```

**Workshop Config**（`config.ts:11-29`）：

```typescript
type SkillWorkshopConfig = {
  autonomous: { mode: SkillsWorkshopAutonomousMode };  // "off" | "propose" | "auto"
  approvalPolicy: "pending" | "auto";
  maxPending: number;       // 默认 50，范围 1-200
  maxSkillBytes: number;    // 默认 40_000，范围 1024-200_000
};

// autonomous.mode 三档：
//   "off"     → 经验评估完全停用（config.ts:35）
//   "propose" → 仅产 proposal，需要用户审批
//   "auto"    → 经验评估后直接 apply（默认）
```

**proposal 修订 / apply 完整流程**（`service.ts:67-330`）：

```typescript
// reviseSkillProposal(input) — 提案可经历多次修订
// 流程：
// 1. withPendingSkillProposalRevision 锁住 pending 状态
// 2. assertInsideSkillsRoot — 路径必须在 skills root 内
// 3. 比较 currentContentHash（防 TOCTOU）
// 4. nextProposalVersion — 修订版本号递增
// 5. prepareSkillProposalDraft — 准备草稿 + 触发 proposal-scan
// 6. replaceSkillProposalDraft — SQLite 替换 + 写 event log
// 7. withSkillProposalLifecycleDispatch → dispatchSkillProposalChanged
```

**安全扫描器**（`/usr/local/LsmGitOpenSource/openclaw/src/skills/security/scanner.ts` — **1038 行**）—— 5 类规则：

```typescript
// 1. LINE_RULES — 单行匹配（line:148-184）
//    - dangerous-exec: child_process exec/spawn 系列
//    - dynamic-code-execution: eval / new Function
//    - crypto-mining: stratum+tcp / coinhive / xmrig
//    - suspicious-network: WebSocket 非标端口

// 2. SOURCE_RULES — 全文 + 上下文窗口（line:189-219）
//    - potential-exfiltration: readFile + fetch/post 8 行内
//    - obfuscated-code: 6+ 连续 \xNN 或 200+ base64
//    - env-harvesting: process.env + 网络 send 8 行内

// 3. SKILL_CONTENT_RULES — 对 SKILL.md 文本内容（line:221-260）
//    - prompt-injection-ignore-instructions: "ignore all previous instructions"
//    - prompt-injection-system: 引用 "system prompt / hidden instructions"
//    - prompt-injection-tool: "run tool without permission"
//    - shell-pipe-to-shell: curl/wget | sh
//    - secret-exfiltration: env + http 80 字符内
//    - destructive-delete: rm -rf / $HOME
//    - unsafe-permissions: chmod 777

// 4. provenance-aware child_process 检测（line:336-413）
//    - 解析 ESM named import { spawn as launch }
//    - ESM default import cp from "child_process"
//    - CJS destructured const { exec: run } = require
//    - 避免 RegExp.exec 误报

// 5. cache & dir limit（line:80-86）
//    - FILE_SCAN_CACHE_MAX = 5000（pruneMapToMaxSize）
//    - DIR_ENTRY_CACHE_MAX = 5000
//    - DEFAULT_MAX_FILE_BYTES = 1MB
//    - DEFAULT_MAX_SCAN_FILES = 500
```

**collection-backup / collection-restore / collection-rollback**（`collection-backup.ts` / `collection-restore.ts` / `collection-rollback.ts`）—— 三件套支持提案失败的完整回滚：

```typescript
// 每次 apply 之前先做 collection-backup，restore 是 inverse
// rollback 表 schema: "openclaw.skill-workshop.rollback.v1" (types.ts:6)
// 限制 backup 数量和大小（collection-backup.ts:1-50）
```

**Workshop 完整生命周期（聚合自上）**：

```
LLM 调 skill_workshop tool
  ↓
1. propose 阶段 (service-propose.ts)
   - composeSkillBodyPatch / proposeCreateSkill / proposeUpdateSkill
   - 生成 proposalId + draftHash + revisionHash
  ↓
2. draft 阶段 (proposal-draft.ts)
   - prepareSkillProposalDraft
   - 写 SQLite proposal_drafts
   - assertInsideSkillsRoot 路径检查
   - description ≤ 160 字节 (proposal-draft.ts:194)
  ↓
3. policy 阶段 (policy.ts + policy.runtime.ts)
   - agent-filter / bin 依赖检查
   - OS 限制 (bionic/darwin)
   - approval policy (pending | auto)
  ↓
4. evaluate 阶段 (service-evaluation.ts)
   - runSkillProposalEvaluators (plugin hook)
   - MAX_EVALUATION_OUTCOMES = 64
   - MAX_EVALUATION_FINDINGS = 200
   - MAX_EVALUATION_METRICS = 64
   - assertSkillProposalEvaluationWithinLimit
  ↓
5. experience-review 阶段 (experience-review.ts)
   - runSkillExperienceReview
   - runWithGatewayIndependentRootWorkAdmission
   - 收集 recent session transcripts
   - LLM 决定 propose / apply / nothing
   - recordSkillExperienceReviewOutcome
  ↓
6. history-scan 阶段 (history-scan.ts)
   - HISTORY_SCAN_SESSION_SEGMENT
   - selectSkillHistoryScanCandidates
   - reconcileSkillHistoryScanProgress
   - 分页扫描历史会话找 idea
  ↓
7. curator 阶段 (curator.ts)
   - readSkillReviewOutcomes
   - 重 active / archived / stale
   - SKILL_LIFECYCLE_CURATION_RETIRED_MESSAGE (curator.ts:25)
  ↓
8. apply 阶段 (apply-transition.ts)
   - assertExpectedRevisionHash
   - applyWorkspaceSkillMutation
   - writeSkillProposalRollback (写 rollback 记录)
   - commitPendingSkillProposalTransition (commit lock)
   - dispatchCommittedSkillChangeBestEffort
  ↓
9. collection-backup / restore / rollback (collection-*.ts)
   - 失败时 restoreWorkspaceSkillMutation
  ↓
10. plugin-hooks 派发 (plugin-hooks.ts)
   - skill_proposal_evaluate (eval 时)
   - skill_proposal_changed (生命周期)
   - normalizeSkillProposalCorrelationId ≤ 256
  ↓
11. lifecycle hooks (plugin-hooks.ts)
   - createSkillProposalEvent → randomUUID eventId
   - hashSkillProposalRevision
   - sequence 自增
  ↓
12. store 持久化 (store-sqlite-*.ts)
   - event / record / rollback / transition 4 表
   - ensureSkillWorkshopSchema
```

### 3.5 opencode：SkillV2 + Discovery + Effect DI

**核心目录**（`/usr/local/LsmGitOpenSource/opencode/packages/core/src/skill/`）：

| 文件 | 行数 | 职责 |
|---|---|---|
| `skill.ts` | 133 | SkillV2 (Effect-based) 数据 |
| `discovery.ts` | 137 | 远程 SkillDiscovery |
| `tool/skill.ts` | 109 | skill tool 输出 |

**3 种 Source**（`schema/src/skill.ts:7-55`）—— SkillV2 数据来源：

```typescript
// 1. bundled — 内置（customize-opencode）
// 2. local — 本地 ~/.claude/skills/
// 3. remote — git pull 远程（location = "plugin.git#v1.3.0/SKILL.md"）
```

**Effect DI 拓扑**（`opencode/test/skill/skill.test.ts:29-36`）：

```typescript
const it = testEffect(Layer.mergeAll(LayerNode.compile(Skill.node), node, testInstanceStoreLayer))
const itWithoutClaudeCodeSkills = testEffect(
  Layer.mergeAll(
    LayerNode.compile(Skill.node, [[RuntimeFlags.node, RuntimeFlags.layer({ disableClaudeCodeSkills: true })]]),
    ...
  ),
)
```

**Skill 注入**（`tool/skill.ts:35-52`）—— 把 skill 内容当 system reminder 注入：

```typescript
// XML 安全 escape（openclaw 同源）：
// Skill.fmt([...]) 把数组渲染为
//   <available_skills>
//     <skill>
//       <name>...</name>
//       <description>...</description>
//       <location>...</location>
//     </skill>
//   </available_skills>
```

**测试覆盖**（`opencode/test/skill/skill.test.ts:585 行`）：

- 6 个 fixture skills（`valid-skill` / `name-mismatch` / `invalid-name-chars` / `long-name` / `missing-description` / `unknown-field` / `nested` / `root-skill-preferred` / `skills-collision`）
- 验证加载 / 错误处理 / 嵌套 / 优先级 / collision

### 3.6 pi：Agent Skills 标准 + 一等公民 + 6 源注入

**核心文件**（`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/skills.ts:507 行`）：

```typescript
const MAX_NAME_LENGTH = 64;
const MAX_DESCRIPTION_LENGTH = 1024;
const IGNORE_FILE_NAMES = [".gitignore", ".ignore", ".fdignore"];

function validateName(name: string): string[] {
  // 1. 长度 ≤ 64
  // 2. /^[a-z0-9-]+$/
  // 3. 不能以 - 开头或结尾
  // 4. 不能包含 -- (consecutive hyphens)
}

function validateDescription(description: unknown): string[] {
  // 1. 必须是 string
  // 2. trim() 不能为空
  // 3. ≤ 1024 字符
}
```

**gitignore 风格过滤**（`skills.ts:178-242`）—— 4 个 ifmatch matcher：

```typescript
// prefixIgnorePattern — 把 .gitignore 模式加 prefix（因为是从 dir 加载的）
// addIgnoreRules — 读 .gitignore / .ignore / .fdignore 并 add 到 matcher
// 在 loadSkillsFromDirInternal 中按需 add
```

**6 源发现**（`skills.ts:407-507`，与第六轮专题对照）：

```typescript
// 1. project (./.pi/skills/)
// 2. user (~/.pi/skills/)
// 3. builtin (./examples/skills/...)
// 4. extension (extension/skills/...)
// 5. SDK
// 6. path
// 4 source 标识优先级
```

**Agent harness skills**（`/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/skills.ts:386 行`）：

```typescript
// ExecutionEnv 抽象 — loadSkills(env, dirs) 在远程沙箱也能跑
// loadSourcedSkills — 携带 source tag，便于上层 provenance 处理
// formatSkillInvocation — 渲染为 <skill name="x" location="..."> ... </skill>
```

**测试覆盖**（`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/test/skills.test.ts:432 行`）：

- `load valid skill` / `allow names that don't match parent directory` / `warn when name contains invalid characters` / `warn when name exceeds 64` / `warn and skip when description is missing` / `ignore unknown frontmatter fields` / `load nested skills recursively` / `prefer a directory's root SKILL.md over nested SKILL.md files` / `skip files without frontmatter`

### 3.7 jiuwenswarm：**SkillDevPipeline 12 阶段确定性状态机**（本轮最重头之一）

**核心目录**（`/usr/local/LsmGitOpenSource/jiuwenswarm/jiuwenswarm/server/runtime/skill/skilldev/`）—— **11 文件 / 12 阶段**：

| 文件 | 行数 | 职责 |
|---|---|---|
| `schema.py` | 669 | 阶段枚举 / 状态 / 事件类型 / 挂起点配置 / Eval 数据结构 |
| `pipeline.py` | 194 | **SkillDevPipeline 编排器**（run/resume） |
| `service.py` | 360 | SkillDevService（无状态请求处理器） |
| `context.py` | 114 | 阶段运行环境（emit + create_stage_agent） |
| `store.py` | 84 | 状态持久化（StateStore） |
| `workspace.py` | 65 | 任务工作区管理 |
| `deps.py` | 37 | 依赖注入（SkillDevDeps） |
| `state_utils.py` | 164 | state 序列化辅助 |
| `stages/init_stage.py` | 128 | INIT：资源预处理 |
| `stages/plan_stage.py` | 167 | PLAN：需求分析 |
| `stages/generate_stage.py` | 208 | GENERATE：SKILL.md 生成 |
| `stages/validate_stage.py` | 153 | VALIDATE：frontmatter 校验 |
| `stages/test_design_stage.py` | 140 | TEST_DESIGN：测试用例设计 |
| `stages/test_run_stage.py` | 143 | TEST_RUN：with_skill vs baseline |
| `stages/evaluate_stage.py` | 327 | EVALUATE：Grader + Benchmark + Analyst |
| `stages/improve_stage.py` | 138 | IMPROVE：根据 feedback 改进 |
| `stages/package_stage.py` | 100 | PACKAGE：打包为 .skill |
| `stages/desc_optimize_stage.py` | 394 | **DESC_OPTIMIZE：描述优化循环** |
| `stages/base.py` | 40 | StageHandler 抽象基类 |
| `stages/__init__.py` | 34 | 阶段注册 |
| `DESIGN.md` | 700+ | 完整设计文档（12 章节） |

**3 模式自动识别**（`DESIGN.md:13-19`）：

```python
# | 模式 | 触发条件 | 场景 |
# | "create" | 仅有 query | 从零创建新 Skill |
# | "create_with_resources" | query + resources | 携带参考资料（文档/代码）创建 |
# | "modify" | query + existing_skill | 修改/升级已有 Skill |
# 三个入口模式（系统自动识别，无需前端传入标志位）
```

**完整 12 阶段流程**（`schema.py:23-49` + `DESIGN.md:75-97`）：

```
INIT → PLAN → PLAN_CONFIRM* → GENERATE → VALIDATE
  → TEST_DESIGN → TEST_RUN → EVALUATE → REVIEW*
  → IMPROVE → (循环回 TEST_RUN)
  → PACKAGE → DESC_OPTIMIZE_CONFIRM* → DESC_OPTIMIZE → COMPLETED

标注 * 的为挂起点（Suspension Point）
终态：COMPLETED / ERROR
```

**Pipeline run() 内部逻辑**（`pipeline.py:67-99`）：

```python
async def run(self) -> AsyncIterator[SkillDevEvent]:
    while self.state.stage not in (COMPLETED, ERROR):
        # 命中挂起点：推送确认请求 → checkpoint → 暂停
        if self.state.stage in SUSPENSION_POINTS:
            suspension = SUSPENSION_POINTS[self.state.stage]
            await self._emit(SkillDevEventType.TODOS_UPDATE, ...)  # 后端驱动 todo
            await self._emit(SkillDevEventType.CONFIRM_REQUEST, {  # 驱动前端弹窗
                "confirm_type": suspension.confirm_type,
                "title": suspension.title,
                "message": suspension.message,
                "data": suspension.extract_data(self.state),
                "actions": suspension.actions,
            })
            await self._checkpoint()
            break

        # 执行当前阶段
        handler_cls = self.STAGE_HANDLERS[self.state.stage]
        result = await handler.execute(ctx)
        self.state.stage = result.next_stage
        await self._checkpoint()
```

**3 个挂起点配置**（`schema.py:218-303`）—— 声明式：

```python
SUSPENSION_POINTS = {
    PLAN_CONFIRM: SuspensionConfig(
        confirm_type="plan_confirm",
        title="请审阅开发计划",
        actions=[
            {"id": "confirm", "label": "确认", "style": "primary"},
            {"id": "modify", "label": "修改", "style": "secondary"},
        ],
        extract_data=_plan_extract_data,
        on_resume=_plan_confirm_on_resume,
        next_stage=GENERATE,
    ),
    REVIEW: SuspensionConfig(
        confirm_type="review",
        title="评测结果审阅",
        actions=[
            {"id": "accept", "label": "通过，进入打包", "style": "primary"},
            {"id": "improve", "label": "继续改进", "style": "secondary"},
        ],
        extract_data=_review_extract_data,
        on_resume=_review_on_resume,
        next_stage=_review_next_stage,  # 函数式：动态决定下一阶段
    ),
    DESC_OPTIMIZE_CONFIRM: SuspensionConfig(
        confirm_type="desc_optimize_confirm",
        actions=[
            {"id": "optimize", "label": "优化", "style": "primary"},
            {"id": "skip", "label": "跳过", "style": "secondary"},
        ],
        extract_data=_desc_opt_extract_data,
        on_resume=_desc_optimize_confirm_on_resume,
        next_stage=_desc_optimize_confirm_next_stage,
    ),
}
```

**eval 数据结构**（`schema.py:317-486`）—— 对齐官方 anthropic skill-creator：

```python
@dataclass
class EvalCase:
    id: int
    prompt: str
    expected_output: str
    files: list[str]
    expectations: list[str]  # 可客观验证的 assertion

@dataclass
class GradingResult:
    expectations: list[GradingExpectation]
    pass_rate: float
    passed_count: int
    failed_count: int

@dataclass
class Benchmark:
    skill_name: str
    runs: list[BenchmarkRun]  # with_skill vs baseline
    run_summary: dict         # mean/stddev/min/max
    notes: list[str]          # Analyst 输出的观察
```

**TEST_DESIGN_SYSTEM_PROMPT**（`test_design_stage.py:38-72`）—— LLM 设计测试用例的 prompt：

```python
TEST_DESIGN_SYSTEM_PROMPT = """根据以下 Skill 内容，设计 {count} 个测试用例。
## 测试用例设计原则
### prompt 要求
- 模拟真实用户输入（包含文件路径、个人背景、具体数据名称等细节）
- 混合不同长度和表达风格（正式/随意/简短/详细）
- 覆盖不同复杂度和边缘场景
- 有些用户不会明确提到 skill 名称，但确实需要这个 skill 的功能
### expectations 要求
- 每条 expectation 是一个可客观验证的声明（字符串）
- 好的 expectation 是 *区分性的*：使用 skill 时通过，不使用时大概率失败
- 避免太容易通过的检查（如只检查文件名存在，不检查内容）
### 输出 JSON 格式
{ "skill_name": ..., "evals": [{"id", "prompt", "expected_output", "files", "expectations"}] }
"""
```

**Grader / Analyst System Prompt**（`evaluate_stage.py:46-93`）—— 对齐官方 agents/grader.md / agents/analyzer.md：

```python
GRADER_SYSTEM_PROMPT = """你是一个评测 Grader。
**PASS**: transcript / outputs 中有明确证据证明 expectation 为真
**FAIL**: 找不到证据，或证据与 expectation 矛盾
**不确定时**: 按 FAIL 处理（举证责任在 expectation 一方）
对每条 expectation 输出：{text, passed, evidence}"""

ANALYST_SYSTEM_PROMPT = """你是一个 Benchmark 分析师。
- 某 expectation 在 with_skill 和 baseline 都 100% pass → 不具区分力
- 某 expectation 在两者都 fail → 超出能力或 expectation 本身有问题
- 某 eval 高方差 → 可能是 flaky 测试
- with_skill 反而劣于 baseline 的指标 → skill 可能在某方面产生负面影响
输出 JSON 字符串数组，每条是一句简洁的观察。"""
```

**DESC_OPTIMIZE 阶段**（`desc_optimize_stage.py:1-394`）—— **对标官方 `improve_description.py`**：

```python
MAX_ITERATIONS = 5
HOLDOUT_RATIO = 0.4  # 40% test，60% train

# 核心流程：
# 1. Agent 生成 ~20 个 trigger eval queries（10 should_trigger + 10 should_not_trigger）
# 2. Train/test split (60% / 40%) by should_trigger 分层
# 3. 迭代优化循环（最多 max_iterations 轮）：
#    a. 对每个 query，调用模型判断当前 description 是否会触发
#    b. 统计 pass rate
#    c. 基于失败案例，调用模型生成改进的 description
#    d. 如果 train 全部通过则提前退出
# 4. 选 test score 最高的 description（防过拟合）
# 5. 将 best_description 写回 SKILL.md frontmatter

TRIGGER_QUERY_GEN_PROMPT = """生成 20 个测试查询。
should_trigger=true (10 个): 用户确实需要这个 Skill 时会说的话
should_trigger=false (10 个): 关键词相近但实际不需要的**近似场景**
不要用明显无关的查询。"""

IMPROVE_DESC_PROMPT = """根据失败案例，写一个更好的 description：
- 从失败中**泛化**，不要过拟合到具体查询
- 用祈使句（"Use when..." 而非 "This skill does..."）
- 聚焦用户意图而非实现细节
- 严格不超过 {max_len} 字符"""
```

**VALIDATE 阶段**（`validate_stage.py:30-153`）—— frontmatter 校验：

```python
ALLOWED_FRONTMATTER_KEYS = frozenset({
    "name", "description", "license", "allowed-tools", "metadata", "compatibility"
})
SKILL_NAME_MAX_LEN = 64
SKILL_DESC_MAX_LEN = 1024

# 校验失败 → 回退 GENERATE 重新生成
# 校验成功 → 进入 TEST_DESIGN
```

**IMPROVE 阶段**（`improve_stage.py:32-77`）—— 改进哲学 prompt：

```python
IMPROVE_SYSTEM_PROMPT = """### 1. 从反馈中泛化，不要过拟合
### 2. 保持精简，删除无效内容
### 3. 解释 why，用心智模型替代死板规则
### 4. 发现重复工作 → 捆绑脚本
### 5. 关注 Benchmark 异常模式
### 6. 先写草稿，再以新鲜眼光审视
请输出改进后的完整文件内容。"""
```

**PACKAGE 阶段**（`package_stage.py:19-99`）—— 打包为 .skill (zip)：

```python
# 排除规则
_EXCLUDE_DIRS = {"__pycache__", "node_modules", ".git"}
_EXCLUDE_FILES = {".DS_Store"}
_EXCLUDE_GLOBS = {"*.pyc"}
_ROOT_EXCLUDE_DIRS = {"evals"}  # 根目录级排除

# 官方格式为 .skill（本质是 zip）
skill_filename = f"{skill_name}.skill"
```

**StateStore 持久化**（`store.py:1-84`）—— checkpoint 序列化：

```python
# 存储路径：~/.jiuwenswarm/agent/workspace/skilldev/{task_id}/state.json
# 每次 stage 边界 serialize SkillDevState to JSON
# 可替换为 Redis（多实例部署）
```

**WorkspaceProvider**（`workspace.py:1-65`）—— 任务工作区：

```python
# ~/.jiuwenswarm/agent/workspace/skilldev/{task_id}/
# ├── state.json         ← checkpoint
# ├── resources/         ← 上传的资源
# ├── skill/             ← 生成的 Skill 目录
# ├── evals/             ← evals.json + iteration-{N}/{grading,timing}.json
# └── output/            ← {skill_name}.skill 打包产物
```

**6 个 Method 接口**（`service.py:48-55`）—— 统一 service.py 入口：

```python
_METHOD_DISPATCH = {
    ReqMethod.SKILLDEV_START: "_handle_start",
    ReqMethod.SKILLDEV_RESPOND: "_handle_respond",
    ReqMethod.SKILLDEV_STATUS: "_handle_status",
    ReqMethod.SKILLDEV_DOWNLOAD: "_handle_download",
    ReqMethod.SKILLDEV_CANCEL: "_handle_cancel",
    ReqMethod.SKILLDEV_FILE_LIST: "_handle_file_list",
    ReqMethod.SKILLDEV_FILE_READ: "_handle_file_read",
}
```

**11 类事件**（`schema.py:74-99`）—— 后端驱动前端：

```python
# --- 流程控制 ---
STAGE_CHANGED = "skilldev.stage_changed"      # 阶段切换
PROGRESS = "skilldev.progress"                 # 阶段内进度
ERROR = "skilldev.error"                       # 错误
# --- 对话流交互 ---
AGENT_THINKING = "skilldev.agent_thinking"     # LLM 推理流
TEST_PROGRESS = "skilldev.test_progress"       # 测试执行进度
# --- 结构化 UI 驱动 ---
CONFIRM_REQUEST = "skilldev.confirm_request"   # 弹窗
TODOS_UPDATE = "skilldev.todos_update"         # Todo 列表
ARTIFACT_READY = "skilldev.artifact_ready"     # 产物列表
# --- 数据载体 ---
EVAL_READY = "skilldev.eval_ready"             # benchmark JSON
VALIDATE_RESULT = "skilldev.validate_result"   # 校验报告
DESC_OPT_READY = "skilldev.desc_opt_ready"     # 描述优化 before/after
```

**compute_todos 后端驱动**（`schema.py:547-590`）：

```python
_STAGE_GROUPS = [
    _StageGroup(id="plan",          stages={INIT, PLAN, PLAN_CONFIRM}),
    _StageGroup(id="generate",      stages={GENERATE, VALIDATE}),
    _StageGroup(id="test",          stages={TEST_DESIGN, TEST_RUN, EVALUATE, REVIEW}),
    _StageGroup(id="improve",       stages={IMPROVE}),
    _StageGroup(id="package",       stages={PACKAGE}),
    _StageGroup(id="desc_optimize", stages={DESC_OPTIMIZE_CONFIRM, DESC_OPTIMIZE}),
]
# 前端只做渲染，Todo 状态由 compute_todos(stage, mode) 计算
```

### 3.8 TencentDB-Agent-Memory：**SkillCore 6 写 4 读 + SkillExtractor 抽取链路**

**核心目录**（`/usr/local/LsmGitOpenSource/TencentDB-Agent-Memory/MemoryCore/src/core/skill/`）—— **15 文件 / 3897 行**：

| 文件 | 行数 | 职责 |
|---|---|---|
| `index.ts` | 140 | 入口（v2 redesign 2026-06-17） |
| `skill-core.ts` | 661 | **SkillCore 门面（6 写 4 读）** |
| `skill-format.ts` | 208 | SKILL.md ↔ SkillFile (de)serialization |
| `skill-versioning.ts` | 435 | 版本事务编排（跨系统补偿） |
| `skill-store.ts` | 733 | SqliteSkillStore（CRUD + search + delete） |
| `skill-resource-store.ts` | 253 | SkillResourceStore（COS 字节存储） |
| `skill-store-ddl.ts` | 104 | DDL（skills / skill_fts / skill_vec） |
| `skill-store.interface.ts` | 123 | ISkillStore 抽象 |
| `skill-permission.ts` | 68 | assertOwner / assertTeamMatch / assertVersionFresh |
| `skill-tools.ts` | 208 | SkillToolsV2（给 Review Agent 6 个 tool） |
| `skill-extractor.ts` | 372 | SkillExtractor（LLM 抽取主链路） |
| `skill-config.ts` | 214 | 配置解析 |
| `skill-fast-path.ts` | 44 | 快路径（无 runner 注入） |
| `types.ts` | 334 | 全部类型 |

**6 写 4 读 SkillCore 门面**（`skill-core.ts:14-30`）：

```typescript
// 6 个写动作：
//   - create        新建 skill v1
//   - update        替换 SKILL.md
//   - patch         单点串替（old_string → new_string）
//   - delete        head status=archived (v2 改为物理真删)
//   - writeFiles    增/改资源
//   - removeFiles   删资源

// 4 个读动作：
//   - get           返回 detail（默认 head；可指定 version）
//   - list          按 team_id + filters 返回 head 行
//   - search        FTS 命中
//   - listVersions  历史版本元信息
//   - readFile      读资源字节
```

**create 主流程**（`skill-core.ts:244-300`）—— 跨系统补偿：

```typescript
// 1) parse + validate frontmatter
const file = this.parseAndValidate(input.content);

// 2) 生成 skill_id（CSPRNG base62 12 字符） + 碰撞检测
//    默认 ulid 走 CSPRNG base62 12 字符（~71 bit 真熵）
//    单实例 100 万 skill 时碰撞概率 ~1.5e-10
const MAX_ID_ATTEMPTS = 3;
for (let attempt = 1; attempt <= MAX_ID_ATTEMPTS; attempt++) {
  const u = this.ulid();
  sid = u.startsWith("skl-") ? u : `skl-${u}`;
  const existing = await this.store.getHeadIncludingArchived(sid);
  if (!existing) break;
  if (attempt >= MAX_ID_ATTEMPTS) {
    throw new SkillCoreError("SKILL_ID_COLLISION", ...);
  }
}

// 3) versioning.createNewSkill — 跨系统"事务"编排
```

**跨系统事务（COS + skill DB + meta_assets）**（`skill-versioning.ts:90-130`）—— 顺序 + 补偿：

```typescript
// 1. writeResource → COS               ← 最不可靠，先做；失败零 DB 副作用
// 2. store.appendVersion → skill DB    ← 失败：反向清 COS
// 3. onSkillCreated → meta_assets      ← 失败：反向删 skill DB + 清 COS

// 原则：最容易失败的先做、失败零副作用的先做、可靠的收尾
// 原实现 "asset 先写" 违背了这个原则：曾出现过 COS 认证挂 → skill 没落库 →
// asset 表却已经有一行的孤儿状态。

// 极端情况：
// - 孤儿 skill（skill 落库但 asset 缺）由 onSkillAccessed 读时自愈补登记
// - 孤儿 COS 文件只占空间，读路径全部过 DB，永远不会被误读到
```

**patch（单点串替）**（`skill-core.ts:327-364`）—— 唯一性 + 乐观锁：

```typescript
// 1. 取 head + 乐观锁
const head = await this.requireHead(input.skill_id, input.team_id);
assertVersionFreshWrap(head, input.expected_version);

// 2. 串替唯一性
const occ = countOccurrences(head.content, input.old_string);
if (occ === 0) throw new SkillCoreError("SKILL_PATCH_NOT_UNIQUE", "old_string not found");
if (occ > 1 && !input.replace_all) {
  throw new SkillCoreError("SKILL_PATCH_NOT_UNIQUE", `occurs ${occ} times; pass replace_all=true`);
}

// 3. 应用 + 重新 parse + validate（防 rename）
const newContent = input.replace_all
  ? splitJoin(head.content, input.old_string, input.new_string)
  : head.content.replace(input.old_string, input.new_string);
const file = this.parseAndValidate(newContent);
if (file.frontmatter.name !== head.name) {
  throw new SkillCoreError("INVALID_FRONTMATTER", "patch attempted to rename skill");
}
```

**乐观锁 / 权限**（`skill-permission.ts:1-68`）：

```typescript
// 三种 assertion：
//   - assertOwner: (teamId, agentId) 二元组必须与 headRow 匹配；否则抛 40301
//   - assertTeamMatch: row.team_id 必须等于请求 teamId；不一致按 NOT_FOUND
//                     处理（不暴露存在性 — 防止存在性侧信道）
//   - assertVersionFresh: expected_version 必传，必须与 head.version 完全一致

export class SkillPermissionError extends Error {
  constructor(public readonly code: SkillPermissionErrorCode, message?: string) {
    // SKILL_NOT_OWNER     (40301)
    // SKILL_TEAM_MISMATCH (40302 → 外部行为同 NOT_FOUND)
    // SKILL_NOT_FOUND     (40401)
    // SKILL_VERSION_STALE (40901)
  }
}
```

**SkillExtractor 抽取链路**（`skill-extractor.ts:1-372`）—— LLM 抽取主链路：

```typescript
// 入参 messages 必须是 ExtractMessage[]，不再接受裸字符串
// 每次调用都走 LLM，不做任何对话级去重/缓存 —— 缓存机制已移除
//
// 内部把 messages 串成 transcript（保留 role 标记）
// 工具调用走 SkillToolsV2（操作 SkillCore）
// 候选返回 ExtractedSkillCandidate 形态（含 skill_id / version）
//
// transcript 头尾截断：headChars=8000 / tailChars=32000

class SkillExtractor {
  async extract(input: ExtractInput): Promise<ExtractResult> {
    const t0 = Date.now();
    const transcript = formatTranscript(messages);
    const truncated = truncateHeadTail(transcript, this.headChars, this.tailChars);

    // 主 Agent 注入抽取提示（reason 非空时放在 prompt 最前面）
    if (input.reason && input.reason.trim().length > 0) {
      const hintBlock = [
        "## 主 Agent 的抽取提示",
        input.reason,
      ].join("\n");
      prompt = `${hintBlock}\n\n---\n\n${prompt}`;
    }

    // 工具调用（6 个 tool） + auditSink 收集 candidates
    const auditSink: ExtractedSkillCandidate[] = [];
    // ... tool 调用循环 ...
    return { candidates: auditSink };
  }
}
```

**SkillToolsV2 6 个 tool**（`skill-tools.ts:1-208`）—— 给 Review Agent：

```typescript
// 暴露 4 写 2 读：
//   - skill_list          列出团队内可见 skill
//   - skill_view          查看单个 skill 详情
//   - skill_create        新建 skill
//   - skill_update        全量替换 SKILL.md
//   - skill_patch         单点串替
//   - skill_files_write   增/改资源
//
// 不暴露 delete / files_remove —— 抽取流程不应能销毁团队 skill
// 工具错误以 JSON.stringify({error}) 返回，让 LLM 能 self-correct
// 每次成功的写操作都 push 一条 ExtractedSkillCandidate 到 auditSink
```

**frontmatter 解析**（`skill-format.ts:34-208`）—— 严格 + 宽容：

```typescript
// 严格：
// - 必须以 "---\n" 开头
// - closing fence "\n---" 后跟 \n 或 EOF
// - name: 1..64 字符, ^[a-z0-9][a-z0-9-]*$
// - description: 1..1024 字符
// - body: ≤ 50_000 字符
// - resources[*].type ∈ {text, executable, binary}
//
// 宽容：
// - 接受 CRLF 或 LF
// - YAML 类型推断：null / true / 123 / comment-like #… 统一 coerce 为 string
// - 区分 "key absent" (undefined) 和 "key present but non-string" (null/true/123)
```

**Round-trip 安全**（`skill-format.ts:194-202`）：

```typescript
// parseSkillFile(formatSkillFile(f)) 一定 yield 同样 logical fields 和 body
// serialize 为 canonical SKILL.md 文本
export function formatSkillFile(file: SkillFile): string {
  const fm = {
    name: file.frontmatter.name,
    description: file.frontmatter.description,
    // ... optional 字段
  };
  const yamlBlock = stringifyYaml(fm).replace(/\n+$/, "");
  return `---\n${yamlBlock}\n---\n\n${file.body}`;
}
```

**自愈补登记钩子**（`skill-core.ts:103-122`）—— 兜底修复 asset 缺失：

```typescript
// onSkillAccessed 读路径自愈补登记：
//   - 触发时机：get / readFile 成功返回单个 skill 之后
//   - 不触发：list / search / listing / listVersions
//   - 契约：fire-and-forget，异常吞掉
//   - 上层实现须幂等且带 LRU
//   - 用途：兜底修复 asset 缺失（历史数据 / 迁移遗漏 / 人工误删）
```

**usage facts**（`curator.ts:88-95`）—— 跨工程借鉴点：

```typescript
// skill_usage 表: skill_file, last_used_at_ms, use_count
// 单 reader for recorded usage; callers pass canonical skill files
function readSkillUsageByFile(skillFiles: readonly string[], options) {
  return new Map(rows.map(row => [row.skill_file, { lastUsedAtMs, useCount }]));
}
```

---

## 4. 横向对比大表（8 工程 × 7 维度）

| 维度 | atomcode | claudecode | deepseek-harness | openclaw | opencode | pi | jiuwenswarm | TencentDB-Mem |
|---|---|---|---|---|---|---|---|---|
| **Skill 一等公民** | 是（catalog hook） | 是（16 builtin） | 是（4 包） | **是**（80+ 文件 Workshop） | 是（Effect DI） | **是**（Agent Skills 标准） | **是**（12 阶段 Pipeline） | **是**（6 写 4 读 SkillCore） |
| **文件格式** | frontmatter（12 字段） | YAML + 内嵌脚本 | YAML + 6 字段 | YAML + goal/evidence | YAML + 3 Source | **Agent Skills 标准** | **anthropic skill-creator 1:1** | **TEN-STOR spec** |
| **多源发现** | 12 目录 | 6 源 | 5 层 SkopedLayers | 3 源（user / project / path） | 3 Source (bundled / local / remote) | **6 源** | workspace | workspace (per team) |
| **生命周期管理** | 无（持久 catalog） | 无（无版本链） | revision + LRU | **12 阶段 Workshop + 5 status + 4 outcome + rollback** | SkillV2 (无 version) | 无（无版本链） | **12 阶段 SkillDevPipeline** | **版本链 v1/v2/v3 + 物理删** |
| **改进闭环** | 无 | skillify（自我扩展） | 无 | **propose→draft→policy→eval→review→apply→rollback** | 无 | 无 | **TEST_RUN→EVALUATE→REVIEW→IMPROVE→loop** | 无（无自动改进） |
| **测试** | 2 unit（catalog hook） | 0（运行时加载器自检） | chokidar watcher 444 行 | **scanner 1038 行 + 17 测试** | **skill 585 行** | **skills 432 行** | **DESIGN 700+ 行 + 11 文件** | format/permission/extract 单元 |
| **安全** | 无 | quick_validate.py | 无 | **5 类规则 + provenance child_process** | SkillV2 path 安全 | frontmatter 校验 | **frontmatter 校验 + name/desc 长度** | **owner/team/version 三断言** |
| **总行数** | ~2134 (skill 域) | ~6000 (含 16 skill) | ~4500 (4 包) | **~18000 (Workshop 80+ 文件)** | ~379 (skill) | ~893 (skill) | **~2700 (skilldev)** | **~3897 (skill)** |
| **关键创新** | SkillFirstHook（强注入） | skillify（自我扩展） | chokidar 热重载 | **状态机 + 提案预算 + 隔离执行** | **Effect DI 拓扑** | **gitignore 过滤** | **12 阶段 Pipeline + 描述优化** | **跨系统补偿事务** |

---

## 5. SkillDevPipeline 12 阶段详解（jiuwenswarm 提炼）

> 摘自 `/usr/local/LsmGitOpenSource/jiuwenswarm/jiuwenswarm/server/runtime/skill/skilldev/`

### 5.1 阶段全图

```
┌─────┐  ┌─────┐  ┌──────────────┐  ┌──────────┐  ┌──────────┐
│INIT │→ │PLAN │→ │PLAN_CONFIRM* │→ │GENERATE  │→ │VALIDATE  │
└─────┘  └─────┘  └──────────────┘  └──────────┘  └──────────┘
                                                       │
┌─────────────┐  ┌──────────┐  ┌──────────┐  ┌────────┐ │
│TEST_DESIGN  │← │EVALUATE  │← │TEST_RUN  │← │(loop)  │←┘
└─────────────┘  └──────────┘  └──────────┘  └────────┘
       ↓
┌─────────┐  ┌─────────┐  ┌─────────┐  ┌──────────────────────┐
│REVIEW*  │→ │IMPROVE  │→ │TEST_RUN │ (loop until accept)  │
└─────────┘  └─────────┘  └─────────┘
       ↓ (accept)
┌─────────┐  ┌───────────────────────┐  ┌──────────────┐
│PACKAGE  │→ │DESC_OPTIMIZE_CONFIRM* │→ │DESC_OPTIMIZE │→ COMPLETED
└─────────┘  └───────────────────────┘  └──────────────┘

标注 * 的为挂起点（Suspension Point）
```

### 5.2 阶段 1：INIT（init_stage.py）

**职责**：资源解压、已有 Skill 加载、状态初始化

```python
# 入参：query, tools, resources, existing_skill
# 行为：
# 1. 解压 resources 到 workspace/resources/
# 2. 如果 modify 模式，读 existing_skill 到 state.existing_skill_md
# 3. 初始化 stage = INIT，iteration = 0
# 4. 推送 INIT 完成事件
```

### 5.3 阶段 2：PLAN（plan_stage.py:1-167）

**职责**：ReActAgent 分析需求，输出结构化开发计划

```python
# 输出 plan 形如：
# {
#   "skill_name": "arxiv_searcher",
#   "description": "Use when the user wants to find and download arXiv papers...",
#   "tools": ["web_search", "web_fetch", "file_write"],
#   "structure": { "scripts": ["search.py", "download.py"] }
# }
# 推荐工具：web_search
# System Prompt 焦点：分析需求，输出结构化 plan JSON
```

### 5.4 阶段 2.5：**PLAN_CONFIRM**（挂起点 1/3）

```python
SuspensionConfig(
    confirm_type="plan_confirm",
    title="请审阅开发计划",
    actions=[
        {"id": "confirm", "label": "确认", "style": "primary"},
        {"id": "modify", "label": "修改", "style": "secondary"},
    ],
    extract_data=lambda state: {"plan": state.plan},
    on_resume=lambda state, data: state.plan = data.get("plan", state.plan),
    next_stage=GENERATE,
)
# 用户点确认 → state.plan 落定 → 进 GENERATE
# 用户点修改 → 通过 skilldev.respond 携带新 plan 重新提交
```

### 5.5 阶段 3：GENERATE（generate_stage.py:1-208）

**职责**：ReActAgent 按 plan 生成 SKILL.md

```python
# 推荐工具：file_write, file_read
# System Prompt 焦点：按 plan 生成 SKILL.md 及支撑文件
# 输出写入 workspace/skill/SKILL.md + workspace/skill/scripts/*.py
```

### 5.6 阶段 4：VALIDATE（validate_stage.py:30-153）

**职责**：静态校验 SKILL.md 格式

```python
# 校验项：
# 1. YAML frontmatter 存在且合法
# 2. name 是 kebab-case，≤ 64 字符
# 3. description ≤ 1024 字符，无 < >
# 4. 只包含 ALLOWED_FRONTMATTER_KEYS = {name, description, license, allowed-tools, metadata, compatibility}
# 校验失败 → 回退 GENERATE 重新生成
# 校验成功 → 进入 TEST_DESIGN
```

### 5.7 阶段 5：TEST_DESIGN（test_design_stage.py:1-140）

**职责**：Agent 设计测试用例集（EvalSet）

```python
# 输出 evals.json 格式（对齐官方 references/schemas.md）：
# {
#   "skill_name": "example-skill",
#   "evals": [
#     {
#       "id": 1,
#       "prompt": "User's task prompt",
#       "expected_output": "Description of expected result",
#       "files": [],
#       "expectations": [
#         "The output includes X",
#         "The skill used script Y"
#       ]
#     }
#   ]
# }
```

### 5.8 阶段 6：TEST_RUN（test_run_stage.py:1-143）

**职责**：并行执行 with_skill vs baseline

```python
# 为每个测试用例并行创建两个子 Agent：
#   with_skill: 注入当前生成的 Skill 后执行用例
#   baseline:   不注入 Skill，作为对照组
# 收集两组结果，写入 workspace/evals/iteration-{N}/{eval_name}/{with_skill,baseline}/
# 推送 TEST_PROGRESS 事件反馈进度
# 这是整个 Pipeline 中技术复杂度最高的阶段
# 待实现：with_skill/baseline 的实际执行逻辑封装在 SkillDevTestRunner 中
```

### 5.9 阶段 7：EVALUATE（evaluate_stage.py:1-327）

**职责**：Grader 评分 → Benchmark 聚合 → Analyst 分析

```python
# Step 1: Grader Agent 评分
#   - 读取执行 transcript + output 文件
#   - 对每条 assertion 评分（PASS / FAIL / 不确定时按 FAIL）
#   - 输出 grading.json（expectations[].text/passed/evidence 格式）

# Step 2: Benchmark 聚合（内化自官方 aggregate_benchmark.py）
#   - 遍历所有 grading.json + timing.json
#   - 计算 per-config 的 mean/stddev/min/max
#   - 计算 delta（with_skill vs baseline）
#   - 输出 benchmark.json

# Step 3: Analyst Agent 分析
#   - 发现 aggregate stats 隐藏的模式
#   - 输出 notes 列表（前端展示为分析摘要）
#   - 关注：100% pass 不区分 / 100% fail 超能力 / 高方差 / 反向劣化

# 推送 EVAL_READY 事件 → 进入 REVIEW 挂起点
```

### 5.10 阶段 7.5：**REVIEW**（挂起点 2/3）

```python
SuspensionConfig(
    confirm_type="review",
    title="评测结果审阅",
    actions=[
        {"id": "accept", "label": "通过，进入打包", "style": "primary"},
        {"id": "improve", "label": "继续改进", "style": "secondary"},
    ],
    extract_data=lambda state: {
        "benchmark": state.eval_results.get("benchmark"),
        "report": state.eval_results.get("report"),
        "iteration": state.iteration,
    },
    on_resume=lambda state, data: state.feedback_history.append(
        {"iteration": state.iteration, "feedback": data.get("feedback", "")}
    ) if data.get("feedback") else None,
    next_stage=lambda data: IMPROVE if data.get("action") == "improve" else PACKAGE,
)
# REVIEW 的下一阶段由用户 action 动态决定
# improve → feedback_history 追加记录 → IMPROVE
# accept → PACKAGE
```

### 5.11 阶段 8：IMPROVE（improve_stage.py:1-138）

**职责**：根据用户反馈改进 Skill

```python
# Agent 工具白名单：["file_read", "file_write"]
# 改进原则（写入 Prompt）：
# 1. 从反馈中泛化，不要过拟合
# 2. 保持精简，删除无效内容
# 3. 解释 why 而非堆砌 MUST/NEVER
# 4. 发现重复工作 → 捆绑脚本
# 5. 关注 Benchmark 异常模式
# 6. 先写草稿，再以新鲜眼光审视
# iteration 计数 +1，跳转回 TEST_RUN 开启新一轮测试
```

### 5.12 阶段 9：PACKAGE（package_stage.py:1-100）

**职责**：打包为 .skill 文件

```python
# 将 skill/ 目录打包为 {skill_name}.skill（zip 格式）
# 排除 evals/（根目录级）、__pycache__、node_modules、.DS_Store、*.pyc
# 推送 ARTIFACT_READY 事件 → 跳转到 DESC_OPTIMIZE_CONFIRM
```

### 5.13 阶段 9.5：**DESC_OPTIMIZE_CONFIRM**（挂起点 3/3）

```python
SuspensionConfig(
    confirm_type="desc_optimize_confirm",
    title="描述优化",
    actions=[
        {"id": "optimize", "label": "优化", "style": "primary"},
        {"id": "skip", "label": "跳过", "style": "secondary"},
    ],
    extract_data=lambda state: {
        "current_description": (state.plan or {}).get("description", "")
    },
    on_resume=lambda state, data: None,  # 纯路由决策
    next_stage=lambda data: DESC_OPTIMIZE if data.get("action") == "optimize" else COMPLETED,
)
```

### 5.14 阶段 10：DESC_OPTIMIZE（desc_optimize_stage.py:1-394）

**职责**：优化 SKILL.md 的 description 以提高触发准确率

```python
# Step 1: Agent 生成 ~20 个 trigger eval queries
#   - 10 should_trigger=true（用户确实需要时）
#   - 10 should_trigger=false（关键词相近但实际不需要的近似场景）
#
# Step 2: Train/test split (60% / 40%) by should_trigger 分层
HOLDOUT_RATIO = 0.4
#
# Step 3: 迭代优化循环（最多 MAX_ITERATIONS=5 轮）
#   a. 对每个 query，调用模型判断当前 description 是否会触发
#   b. 统计 pass rate
#   c. 基于失败案例，调用模型生成改进的 description
#   d. 如果 train 全部通过则提前退出
#
# Step 4: 选 test score 最高的 description（防过拟合）
#
# Step 5: 将 best_description 写回 SKILL.md frontmatter
#
# 关键 prompt:
# - TRIGGER_QUERY_GEN_PROMPT: 生成 20 个 trigger queries
# - IMPROVE_DESC_PROMPT: "从失败中**泛化**，不要过拟合到具体查询"
```

### 5.15 关键设计决策（DESIGN.md §11）

| 决策 | 原因 | 权衡 |
|---|---|---|
| Pipeline 不长驻内存 | 避免大量并发任务的内存积压；强制所有状态经过 StateStore 持久化 | 每次请求有 load/save_state 文件 I/O 开销，分钟级任务可忽略 |
| 单一 `skilldev.respond` 入口 | 前端不需要知道当前处于哪个挂起点 | 新增挂起点时前端代码无需修改 |
| 后端驱动 UI 状态 | Todo 列表、弹框、产物列表全部由后端事件携带 | `compute_todos()` 是 Todo 状态唯一计算来源 |
| 每阶段独立 Agent | 工具隔离、Prompt 隔离、内存隔离 | `create_stage_agent()` 接口已定义，实际接入 `openjiuwen ReActAgent` 待实现 |
| 工作区路径统一 | SkillDev 任务目录必须在统一工作区下 | `~/.jiuwenswarm/agent/workspace/skilldev/{task_id}/` |

---

## 6. Workshop 自演化闭环详解（openclaw 提炼）

> 摘自 `/usr/local/LsmGitOpenSource/openclaw/src/skills/workshop/`

### 6.1 Workshop 12 阶段全图

```
LLM 调 skill_workshop tool
  ↓
1. propose (service-propose.ts)
   - composeSkillBodyPatch
   - proposeCreateSkill / proposeUpdateSkill
  ↓
2. draft (proposal-draft.ts)
   - nextProposalVersion
   - prepareSkillProposalDraft
   - description ≤ 160 字节
  ↓
3. policy (policy.ts + policy.runtime.ts)
   - agent-filter / bin 依赖 / OS 限制
   - approval policy (pending | auto)
  ↓
4. evaluate (service-evaluation.ts)
   - runSkillProposalEvaluators (plugin hook)
   - MAX_EVALUATION_OUTCOMES = 64
  ↓
5. experience-review (experience-review.ts)
   - runSkillExperienceReview
   - 按 cron 调度，收集 recent session transcripts
   - LLM 决定 propose / apply / nothing
  ↓
6. history-scan (history-scan.ts)
   - selectSkillHistoryScanCandidates
   - 分页扫描历史会话找 idea
   - HISTORY_SCAN_MAX_PROPOSAL_MUTATIONS
  ↓
7. curator (curator.ts)
   - readSkillReviewOutcomes
   - 重 active / archived / stale
  ↓
8. apply (apply-transition.ts)
   - applyWorkspaceSkillMutation
   - writeSkillProposalRollback
   - commitPendingSkillProposalTransition
   - dispatchCommittedSkillChangeBestEffort
  ↓
9. collection-backup / restore / rollback
   - 失败时 restoreWorkspaceSkillMutation
  ↓
10. plugin-hooks 派发
    - skill_proposal_evaluate (eval 时)
    - skill_proposal_changed (生命周期)
  ↓
11. lifecycle hooks
    - createSkillProposalEvent
    - hashSkillProposalRevision
  ↓
12. store 持久化 (4 表)
    - event / record / rollback / transition
```

### 6.2 关键配置项（config.ts:11-29）

```typescript
const DEFAULT_CONFIG: SkillWorkshopConfig = {
  autonomous: { mode: "auto" },
  approvalPolicy: "auto",
  maxPending: 50,        // 1-200
  maxSkillBytes: 40_000, // 1024-200_000
};

// autonomous.mode 三档（proposal 自主程度）：
//   "off"     → 经验评估完全停用
//   "propose" → 仅产 proposal，需要用户审批
//   "auto"    → 经验评估后直接 apply（默认）
```

### 6.3 apply 状态转换表（apply-transition.ts:62-78）

```typescript
const SKILL_PROPOSAL_APPLY_TRANSITIONS = {
  pending: {
    apply_failed: "pending",
    apply_succeeded: "applied",
    scan_failed: "quarantined",
    target_changed: "stale",
  },
  applied: {},
  rejected: {},
  quarantined: {},
  stale: {},
};
// 只有 pending 可以继续 transition；其他状态是终态
```

### 6.4 安全扫描器 5 类规则（security/scanner.ts:1038 行）

| 规则类型 | 数量 | 行号 | 严重度 |
|---|---|---|---|
| LINE_RULES（单行） | 4 | 148-184 | critical / warn |
| SOURCE_RULES（全文+上下文） | 3 | 189-219 | critical / warn |
| SKILL_CONTENT_RULES（文本内容） | 7 | 221-260 | critical / warn |
| provenance-aware child_process | 1 套 | 336-413 | critical |
| cache & dir limit | 5 常量 | 80-86 | n/a |

**5 类规则详表**：

| ruleId | severity | 匹配模式 |
|---|---|---|
| dangerous-exec | critical | `exec|execSync|spawn|...(` + child_process |
| dynamic-code-execution | critical | `eval(\|new Function(` |
| crypto-mining | critical | stratum+tcp / coinhive / cryptonight / xmrig |
| suspicious-network | warn | WebSocket 非标端口 |
| potential-exfiltration | warn | readFileSync + fetch/post 8 行内 |
| obfuscated-code | warn | 6+ 连续 \xNN 或 200+ base64 |
| env-harvesting | critical | process.env + 网络 send 8 行内 |
| prompt-injection-ignore-instructions | critical | `ignore (all|any) (previous|above) instructions` |
| prompt-injection-system | critical | `system prompt / developer message / hidden instructions` |
| prompt-injection-tool | critical | `run|execute|invoke|call` ... `tool` ... `without` ... `permission` |
| shell-pipe-to-shell | critical | `curl|wget` ... `| sh|bash|zsh` |
| secret-exfiltration | critical | `process.env|env` ... `fetch|curl|wget|http` 80 字符内 |
| destructive-delete | warn | `rm -rf /|$HOME|~|.` |
| unsafe-permissions | warn | `chmod (-R )?777` |

### 6.5 提案预算（types.ts:55-69）

```typescript
export type SkillWorkshopProposalMutationBudget = {
  remaining: number;                            // 剩余可调用次数
  successfulMutations?: number;                 // 成功持久化 mutation 数
  failedMutations?: number;                     // 失败或未完成 checkpoint 的 mutation
  mutatedProposalIds?: Set<string>;             // 当前 run idea 集合
  readSkillHashes?: Map<string, string>;        // 活 skill read 本次 run 的 content hash
  preparedSkillPatches?: Map<string, SkillWorkshopPreparedPatch>;  // 单次 exact-span patch 权限
};

// autonomous.mode === "propose" 时：{ remaining: 1, readSkillHashes: new Map() }
// 限制每个 LLM run 的 proposal mutation 次数，防止无限重试
```

### 6.6 revision 原子性（service.ts:300-340）

```typescript
// withPendingSkillProposalRevision — 把"读 + 写"原子化
// 1. readRequiredProposal
// 2. withSkillProposalTargetLock 锁住
// 3. 再次 read（reconcile: false）确认 revision 未变
// 4. assertExpectedRevisionHash — hash 校验
// 5. if create kind: 读 currentContent；如果 file 存在 → markStale
// 6. if update kind: assertSupportTargetsUnchanged
// 7. nextProposalVersion
// 8. prepareSkillProposalDraft + scan
// 9. replaceSkillProposalDraft (expected=record) — SQLite 原子替换
```

### 6.7 collection 备份 / 还原（collection-backup.ts / collection-restore.ts / collection-rollback.ts）

```typescript
// 每次 apply 之前先做 collection-backup
// restore 是 inverse，rollback 表 schema: "openclaw.skill-workshop.rollback.v1"
// 限制 backup 数量和大小
// writeSkillProposalRollback 写 rollback 记录到 SQLite
// clearSkillProposalRollback 在确认 commit 后清理
```

### 6.8 经验评估调度（experience-review-scheduler.ts）

```typescript
// 按 cron 调度 runSkillExperienceReview
// 收集 recent session transcripts（受 HISTORY_SCAN_MAX_SESSION_CHARS 限制）
// HISTORY_SCAN_SESSION_OVERHEAD_CHARS 给 prompt 留 budget
// runWithGatewayIndependentRootWorkAdmission 隔离执行（不被 gateway drain 中断）
// registerAgentRunContext 注册 runId
// recordSkillExperienceReviewOutcome 记录 outcome（completed / applied / proposed / nothing / failed）
```

### 6.9 历史扫描（history-scan.ts + history-scan-candidates.ts）

```typescript
// 扫描历史会话找 skill 改进 idea
// selectSkillHistoryScanCandidates — 按 reviewedTimes / lastConsidered 选 candidate
// reconcileSkillHistoryScanProgress — 增量进度
// resolveSkillHistoryScanHasMore — 是否还有更多
// toStoredState — 状态机持久化
// 关键状态：oldestCursor / newestCursor / oldestReviewedAt / newestReviewedAt
// 当会话 source rotated or changed 时：finalizeUnreplayableSkillHistoryScan
//   保留 durable proposals（有用的工作），finalize partial batch
```

### 6.10 提案对比 workshop 与 skilldev

| 维度 | openclaw Workshop | jiuwenswarm SkillDevPipeline |
|---|---|---|
| 入口 | LLM 调 `skill_workshop` tool | 用户在对话框发 `skilldev.start` |
| 流程 | 12 阶段（propose → apply） | 12 阶段（init → package） |
| 状态机 | 5 status（pending/applied/rejected/quarantined/stale） | 14 阶段（含挂起点） |
| 评估 | 插件 hook（plugin_evaluators） | Grader Agent + Analyst Agent |
| 改进循环 | experience-review 主动 propose | user-driven improve（feedback） |
| 描述优化 | 无 | DESC_OPTIMIZE 阶段（5 轮 eval→improve 循环） |
| 持久化 | SQLite 4 表（event/record/rollback/transition） | state.json + workspace |
| rollback | collection-rollback 表 | state checkpoint 重载 |
| 测试 | 17 个 .test.ts（含 scanner 完整） | 11 文件 + DESIGN.md |
| 总体量 | ~18000 行（80 文件） | ~2700 行（11 文件 + DESIGN 700+） |

---

## 7. SkillCore 6 写 4 读详解（TencentDB 提炼）

### 7.1 6 写详解

| 写动作 | API | 关键约束 | 错误码 |
|---|---|---|---|
| create | `core.create({name, content, resources?, metadata?})` | frontmatter 校验 + name match + sid 碰撞检测（3 次） | INVALID_FRONTMATTER / SKILL_NAME_DUPLICATE / SKILL_ID_COLLISION |
| update | `core.update({skill_id, expected_version, content})` | name 不能跨版本改变 + 跨系统事务 | SKILL_VERSION_STALE / SKILL_NOT_OWNER |
| patch | `core.patch({skill_id, expected_version, old_string, new_string, replace_all?})` | old_string 唯一性（occ=0 → 失败，occ>1 + replace_all=false → 失败） | SKILL_PATCH_NOT_UNIQUE |
| delete | `core.delete({skill_id, expected_version})` | 物理真删（v2 变更，原为软删）；跨系统清理 | SKILL_NOT_FOUND |
| writeFiles | `core.writeFiles({skill_id, expected_version, files[]})` | 资源增改（path 必须合法） | INVALID_PATH / RESOURCE_TOO_LARGE |
| removeFiles | `core.removeFiles({skill_id, expected_version, paths[]})` | 资源删除 | INVALID_PATH |

### 7.2 4 读详解

| 读动作 | API | 关键约束 |
|---|---|---|
| get | `core.get({skill_id, version?, include_content?, include_manifest?})` | 默认 head；可指定历史版本；可只取 metadata |
| list | `core.list({team_id, filters?, pagination?})` | owner_agent_id / name_prefix / status[] 过滤 |
| search | `core.search({query, top_k?, mode?})` | bm25 / embedding / hybrid（FTS5 + VDB） |
| listVersions | `core.listVersions({skill_id, pagination?})` | 历史版本元信息 |
| readFile | `core.readFile({skill_id, version?, path, encoding?})` | 资源字节读（utf-8 / base64） |

### 7.3 跨系统补偿事务（skill-versioning.ts:90-130）

```typescript
// 原则：最容易失败的先做、失败零副作用的先做、可靠的收尾
//
// 1. writeResource → COS               ← 最不可靠，先做；失败零 DB 副作用
// 2. store.appendVersion → skill DB    ← 失败：反向清 COS
// 3. onSkillCreated → meta_assets      ← 失败：反向删 skill DB + 清 COS
//
// 极端情况（步骤 2/3 rollback 又失败）：
//   - 孤儿 skill（skill 落库但 asset 缺）由 onSkillAccessed 读时自愈补登记
//   - 孤儿 COS 文件只占空间，读路径全部过 DB，永远不会被误读到
```

### 7.4 乐观锁 + 权限

```typescript
// assertOwner: (teamId, agentId) 二元组必须与 headRow 匹配
// assertTeamMatch: row.team_id 必须等于请求 teamId；不一致按 NOT_FOUND 处理
// assertVersionFresh: expected_version 必传，必须与 head.version 完全一致
//
// 错误码：
//   SKILL_NOT_OWNER     (40301)
//   SKILL_TEAM_MISMATCH (40302 → 外部行为同 NOT_FOUND，避免存在性侧信道)
//   SKILL_NOT_FOUND     (40401)
//   SKILL_VERSION_STALE (40901)
```

### 7.5 SkillExtractor 抽取链路

```typescript
// 入参 messages 必须是 ExtractMessage[]，不再接受裸字符串
// 每次调用都走 LLM，不做任何对话级去重/缓存
// transcript 头尾截断：headChars=8000 / tailChars=32000
// 主 Agent 注入 reason 提示（放在 prompt 最前面）
// 工具调用（4 写 2 读）+ auditSink 收集 candidates
//
// 6 个 tool：
//   - skill_list          列出团队内可见 skill（先看有什么）
//   - skill_view          查看单个 skill 详情
//   - skill_create        新建 skill
//   - skill_update        全量替换 SKILL.md
//   - skill_patch         单点串替
//   - skill_files_write   增/改资源
//
// 不暴露 delete / files_remove —— 抽取流程不应能销毁团队 skill
```

---

## 8. Skill 单元测试 / 评测体系

### 8.1 各工程的 Skill 测试覆盖

| 工程 | 测试文件 | 行数 | 覆盖点 |
|---|---|---|---|
| atomcode | catalog_hook.rs / skill_first.rs | ~120 | hook 注入 / 位置 / 幂等 |
| claudecode | 0 专门测试（运行时自检） | - | - |
| deepseek-harness | skill-filesystem-watcher.spec.ts | 444 | chokidar 事件 / 路径解析 / 失效 / 重新加载 |
| openclaw | skills-proposal-manual-target.e2e.test.ts | 194 | proposal lifecycle / stale / apply 失败 |
| openclaw | cli-json-stdout.skills.e2e.test.ts | 189 | CLI 端到端 |
| openclaw | security/audit-workshop-skills.test.ts | 302 | scanner 5 类规则 + 隐藏 / shadowed skills |
| openclaw | collection-backup / collection-restore.test.ts | ~500 | rollback 全场景 |
| openclaw | curator.test.ts / experience-review.live.test.ts / history-scan.test.ts | ~1500 | 经验评估 / 历史扫描 |
| opencode | skill.test.ts / skill-discovery.test.ts / tool-skill.test.ts | 585+ | XML escape / path / 优先级 / collision |
| pi | skills.test.ts | 432 | 9 个 fixture + 加载 / 错误 / 嵌套 / 优先级 |
| pi | sdk-skills.test.ts | ~200 | SDK 入口 |
| pi | 2781-skill-collision-precedence.test.ts | ~100 | 碰撞优先级回归 |
| jiuwenswarm | 11 个 skilldev 文件 | ~2700 | 12 阶段 + DESIGN.md |
| TencentDB-Mem | format / permission / extract 单测 | ~600 | 解析 / 乐观锁 / 抽取 |

### 8.2 Eval 集（jiuwenswarm 唯一）

**`evals.json` schema**（`test_design_stage.py:11-32`）：

```json
{
  "skill_name": "arxiv_searcher",
  "evals": [
    {
      "id": 1,
      "prompt": "用具体细节描述的用户任务...",
      "expected_output": "预期结果的人可读描述",
      "files": [],
      "expectations": [
        "输出中包含 X 的结构化数据",
        "使用了 scripts/ 中的 Y 脚本"
      ]
    }
  ]
}
```

**benchmark.json schema**（`schema.py:454-485`）：

```json
{
  "metadata": { "skill_name": "arxiv_searcher", "timestamp": "2025-01-01T00:00:00Z" },
  "runs": [
    {
      "eval_id": 1,
      "eval_name": "search-arxiv-2024",
      "configuration": "with_skill",  // or "baseline"
      "run_number": 1,
      "result": { "pass_rate": 0.85, "time_seconds": 12.3, "tokens": 4500 },
      "expectations": [...]
    }
  ],
  "run_summary": {
    "with_skill": { "pass_rate": {mean, stddev, min, max}, "time_seconds": {...}, "tokens": {...} },
    "baseline":   { "pass_rate": {...}, ... },
    "delta":      { "pass_rate": "+0.15", "time_seconds": "+2.3", "tokens": "-200" }
  },
  "notes": ["观察1", "观察2"]
}
```

### 8.3 描述优化评测（jiuwenswarm 独有）

**触发测试 queries**（`desc_optimize_stage.py:55-77`）：

```python
TRIGGER_QUERY_GEN_PROMPT = """生成 20 个测试查询。

should_trigger=true (10 个):
- 用户确实需要这个 Skill 时会说的话
- 不同表达风格（正式/随意/简短/详细）
- 有些不直接提及 Skill 名称但确实需要其功能
- 包含具体细节（文件路径、个人背景、数据名称等）

should_trigger=false (10 个):
- 关键词相近但实际不需要这个 Skill 的 **近似场景**
- 相邻领域、歧义措辞、看似相关但应由其他工具处理
- 不要用明显无关的查询（"写斐波那契函数"对 PDF 技能来说太容易区分了）

输出 JSON 数组：
[{"query": "具体的用户查询", "should_trigger": true|false}, ...]
"""
```

**Train/test split**（`desc_optimize_stage.py:222-241`）：

```python
HOLDOUT_RATIO = 0.4  # 40% test，60% train

@staticmethod
def _split_eval_set(queries, holdout, seed=42):
    rng = random.Random(seed)
    trigger = [q for q in queries if q.should_trigger]
    no_trigger = [q for q in queries if not q.should_trigger]
    rng.shuffle(trigger)
    rng.shuffle(no_trigger)

    n_t = max(1, int(len(trigger) * holdout))
    n_nt = max(1, int(len(no_trigger) * holdout))

    test = trigger[:n_t] + no_trigger[:n_t]   # 40% by should_trigger 分层
    train = trigger[n_t:] + no_trigger[n_t:]  # 60%
    return train, test
```

**优化循环**（`desc_optimize_stage.py:252-300`）：

```python
MAX_ITERATIONS = 5

for i in range(1, MAX_ITERATIONS + 1):
    # 评估 train + test
    train_results = await self._eval_description(ctx, current_desc, train_set)
    test_results = await self._eval_description(ctx, current_desc, test_set)

    train_passed = sum(1 for r in train_results if r["pass"])
    iteration = DescOptimizeIteration(
        iteration=i,
        description=current_desc,
        train_passed=train_passed,
        train_total=len(train_set),
        test_passed=...,
        test_total=...,
    )
    history.append(iteration)

    # 早停：如果 train 全部通过
    if train_passed == len(train_set):
        break

    # 基于失败案例生成改进的 description
    improved_desc = await self._improve_description(ctx, current_desc, train_results, history)
    current_desc = improved_desc

# 选 test score 最高的 description（防过拟合）
best_iter = max(history, key=lambda h: h.test_passed or 0)
```

**IMPROVE_DESC prompt**（`desc_optimize_stage.py:88-105`）：

```python
IMPROVE_DESC_PROMPT = """根据失败案例，写一个更好的 description：
- 从失败中 **泛化**，不要过拟合到具体查询
- 用祈使句（"Use when..." 而非 "This skill does..."）
- 聚焦用户意图而非实现细节
- 让触发场景具体且可区分
- 严格不超过 {max_len} 字符

请在 <new_description> 标签中只输出新的 description 文本：
<new_description>新描述内容</new_description>
"""
```

### 8.4 安全扫描器测试（openclaw）

**核心 fixture**（`audit-workshop-skills.test.ts:14-25`）：

```typescript
async function writeAuditSkill(root: string, unsafe: boolean, name = "shared-procedure") {
  const dir = path.join(root, name);
  await fs.mkdir(dir, { recursive: true });
  await fs.writeFile(
    path.join(dir, "SKILL.md"),
    `---\nname: ${name}\ndescription: Test procedure\n---\nFollow the procedure.\n`,
  );
  if (unsafe) {
    await fs.writeFile(
      path.join(dir, "run.js"),
      'const { execSync } = require("node:child_process"); execSync(input);\n',
    );
  }
  return await fs.realpath(dir);
}

it.each(
  [
    { label: "default discovery", limits: {} },
    { label: "zero candidates", limits: { maxCandidatesPerRoot: 0 } },
    { label: "zero loaded skills", limits: { maxSkillsLoadedPerSource: 0 } },
    { label: "one candidate", limits: { maxCandidatesPerRoot: 1 } },
    { label: "one loaded skill", limits: { maxSkillsLoadedPerSource: 1 } },
    { label: "small prompt file cap", limits: { maxSkillFileBytes: 1 } },
  ].flatMap(({ label, limits }) => ["", "group"].map((group) => ({ label, limits, group }))),
)("audits hidden and shadowed Workshop skills with $label ($group)", async ({ limits, group }) => {
  // 验证 5 类规则在 default / group 嵌套下都能发现
});
```

**group 不可读场景**（`audit-workshop-skills.test.ts:64-100`）：

```typescript
it("reports an unreadable grouping directory without skipping readable siblings", async () => {
  // 制造一个 grouping dir 不可读（EACCES）
  // 验证 scanner 仍然能报告 readable sibling 的 finding
  // 同时报告 scan_failed finding
});
```

### 8.5 单元测试 fixture 模式

**opencode fixture**（`/usr/local/LsmGitOpenSource/opencode/packages/opencode/test/fixture/skills/`）：

```
agents-sdk/         # 复杂 frontmatter + tool 引用
cloudflare/         # remote source
index.json          # 注册表
```

**pi fixture**（`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/test/fixtures/skills/`）：

```
valid-skill/                # 标准 frontmatter
name-mismatch/              # frontmatter.name ≠ dir name
invalid-name-chars/         # 名称含非法字符
long-name/                  # 名称 > 64
missing-description/        # 缺 description
unknown-field/              # 未知字段（应忽略）
nested/child-skill/         # 嵌套 skill
root-skill-preferred/       # root SKILL.md vs nested
skills-collision/           # 碰撞优先级
```

---

## 9. Skill 安全：签名 / 来源 / 沙箱 / 审计

### 9.1 签名（普遍缺失）

**8 个工程现状**：
- ❌ 无任何工程对 skill 做加密签名
- ⚠️ openclaw 仅在 proposal 流转时做 **revision hash** + **content hash**（`proposal-hash.ts` / `revision-hash.ts`），用于检测内容漂移
- ⚠️ deepseek-harness 用 revision + LRU 缓存做内容版本控制
- ✅ TencentDB-Mem 用 `assertExpectedRevisionHash` 防止并发覆盖

**laew 建议**：
- P1：加 **content hash**（SHA-256）作为 skill 的 `signature` 字段
- P2：加 **签名链**（skill author 的 ed25519 私钥签名；load 时 verify）

### 9.2 来源校验

| 工程 | 实现 | 力度 |
|---|---|---|
| openclaw | `proposal-origin-validation.ts` + `normalizeProposalOrigin` + `mergeProposalOriginRunProvenance` | 强：记录 agentId / sessionKey / runId / messageId |
| openclaw | `proposal-source-validation` 在 `prepareSkillProposalDraft` 调用 `scanProposalBundle` 时 | 强：扫描 proposal 内容（含 description / goal / evidence） |
| deepseek-harness | `skill-filesystem` 用 chokidar 监听 path | 中：watch path 即来源 |
| atomcode | `SkillFirstHook` 检查 model + has_skills | 弱：仅触发条件 |
| pi | `createSyntheticSourceInfo(filePath, {source, scope, baseDir})` | 中：每个 skill 携带 source info |
| jiuwenswarm | 暂无（待实现） | 弱 |
| TencentDB-Mem | team_id + agent_id + owner_agent_id | 强：四元组身份 |

### 9.3 沙箱执行

| 工程 | 沙箱实现 | 力度 |
|---|---|---|
| atomcode | 无（依赖模型本身） | 弱 |
| claudecode | 依赖 Bash 工具的沙箱（外部） | 中 |
| openclaw | `sandbox.runtime.ts` 解析 sandbox runtime status | 强：apply mutation 前先检查 sandbox |
| openclaw | `propose` mode 在 sandbox 下不触发 autonomous propose | 强 |
| opencode | Effect DI 隔离 + per-instance Layer | 中 |
| jiuwenswarm | `sysop_config` 字段控制文件访问权限 | 强 |
| TencentDB-Mem | `StorageAdapter` 抽象 + COS 路径限制 | 强 |

### 9.4 审计

| 工程 | 审计实现 |
|---|---|
| openclaw | `createSkillProposalEvent` 记录 actor + payload + correlationId；`recordSkillExperienceReviewOutcome` 记录 outcome + usage（inputTokens / cachedInputTokens / outputTokens） |
| openclaw | `collection-review-state.ts` 持久化 review outcome + collectionReviews / experienceReviews |
| openclaw | `runSkillExperienceReview` 调用 `runWithGatewayIndependentRootWorkAdmission` 隔离执行 |
| jiuwenswarm | `trace` 集成 Langfuse，emit 事件全程可追溯 |
| TencentDB-Mem | 每次写 push ExtractedSkillCandidate 到 auditSink；onSkillCreated / onSkillArchived 钩子；onSkillAccessed 读路径补登记 |

### 9.5 反 prompt-injection（openclaw scanner 独有）

```typescript
// SKILL_CONTENT_RULES (scanner.ts:221-260)
// 7 类文本内容规则，专门检测 skill markdown 文本里嵌入的注入
//
// 1. prompt-injection-ignore-instructions (critical)
//    /\bignore\s+(?:(?:all|any)\s+)?(?:previous|above|prior|all|any)\s+instructions\b/i
//
// 2. prompt-injection-system (critical)
//    /\b(?:system\s+prompt|developer\s+message|hidden\s+instructions)\b/i
//
// 3. prompt-injection-tool (critical)
//    /\b(run|execute|invoke|call)\b[\s\S]{0,50}\btool\b[\s\S]{0,50}\bwithout\b[\s\S]{0,30}\b(permission|approval)/i
//
// 4. shell-pipe-to-shell (critical)
//    /\b(curl|wget)\b[^|\n]{0,120}\|\s*(sh|bash|zsh)\b/i
//
// 5. secret-exfiltration (critical)
//    /\b(process\.env|env)\b.{0,80}\b(fetch|curl|wget|http|https)\b/i
//
// 6. destructive-delete (warn)
//    /\brm\s+-rf\s+(\/|\$HOME|~|\.)/i
//
// 7. unsafe-permissions (warn)
//    /\bchmod\s+(-R\s+)?777\b/i
```

### 9.6 provenance-aware child_process 检测（scanner.ts:336-413）

```typescript
// 痛点：未限定的别名绑定可能产生误报
// 例：import { spawn as launch } from "./other" → launch() 不应被标记
// 例：RegExp.re["exec"](value) → 不应被标记
//
// 解决：解析 5 种 child_process 导入模式，记录 provenance
//
// ESM named import { spawn as launch, execFile } from "child_process"
// ESM default import cp from "child_process"
// ESM namespace import * as proc from "child_process"
// CJS destructured const { exec: run, spawn } = require("child_process")
// CJS namespace const proc = require("child_process")
//
// 收集：methodAliases（rename 映射）+ namespaceAliases（命名空间映射）
// 应用：matchAliasedChildProcessCalls — 在 line 中匹配 provenance 限定的 alias
```

---

## 10. 共性模式（5 大共性）

### 10.1 共性 1：**Skill frontmatter 必填 name + description**

| 工程 | name 校验 | description 校验 | 其他字段 |
|---|---|---|---|
| atomcode | kebab-case, ≤ 64 | 必填 | license / allowed-tools / metadata |
| claudecode | kebab-case | 必填 | model / tools |
| deepseek-harness | kebab-case | 必填 | - |
| openclaw | kebab-case, ≤ 64 | ≤ 1024, ≤ 160 字节（proposal） | goal / evidence / version / date |
| opencode | kebab-case | 必填 | - |
| pi | `/^[a-z0-9-]+$/`, ≤ 64, 无连续 `--`, 无前后 `-` | 必填, ≤ 1024 | disable-model-invocation |
| jiuwenswarm | kebab-case, ≤ 64 | 必填, ≤ 1024, 无 `<>` | license / allowed-tools / metadata / compatibility |
| TencentDB-Mem | `^[a-z0-9][a-z0-9-]*$`, ≤ 64 | 必填, ≤ 1024 | category / created_at / updated_at / source / resources[] |

### 10.2 共性 2：**多源发现 + 优先级**

| 工程 | 源数量 | 优先级策略 |
|---|---|---|
| atomcode | 12 目录 | source 优先级（render.rs:25-178） |
| claudecode | 6 源 | 7 去重 + symlink 保护（loadSkillsDir.ts:118-124） |
| deepseek-harness | 5 层 | SkopedLayers 注入 |
| openclaw | 3 源（user / project / path） | per-agent 解析 workshop skills root |
| opencode | 3 Source | bundled / local / remote |
| pi | 6 源 | 4 source 标识优先级 |
| jiuwenswarm | workspace | 单一 |
| TencentDB-Mem | team_id scope | owner_agent_id 过滤 |

### 10.3 共性 3：**触发模型 — 自动匹配 + 显式 + 工具**

| 触发方式 | 实现工程 |
|---|---|
| 自动匹配（description 命中） | atomcode / deepseek-harness / opencode / pi / jiuwenswarm |
| 显式（`/name` 或 `<skill name>`） | deepseek-harness（tool-skill /name）/ openclaw（use_skill）/ atomcode（use_skill tool）/ opencode（skill tool） |
| Slash command | claudecode（`/skill-creator`）/ pi |
| Tool call | claudecode / opencode / atomcode / jiuwenswarm / TencentDB-Mem |
| 自我扩展（skillify） | claudecode / openclaw |

### 10.4 共性 4：**版本 + 乐观锁**

| 工程 | 版本字段 | 乐观锁 |
|---|---|---|
| atomcode | 无（仅 catalog） | 无 |
| claudecode | 无（runtime 自检） | 无 |
| deepseek-harness | revision + LRU | 无（filesystem 内容 hash） |
| openclaw | proposedVersion + revisionHash + contentHash | assertExpectedRevisionHash |
| opencode | 无 | 无 |
| pi | 无 | 无 |
| jiuwenswarm | pipeline state + workspace | state.json checkpoint |
| TencentDB-Mem | skill.version (int 自增) | assertVersionFresh |

### 10.5 共性 5：**安全 — 路径约束 + 大小限制**

| 工程 | name 长度 | desc 长度 | body 字节 | 其它 |
|---|---|---|---|---|
| atomcode | 64 | - | 8KB 预算（render.rs） | - |
| claudecode | 64 | - | - | - |
| deepseek-harness | 64 | - | - | - |
| openclaw | 64 | 1024 (session.ts:23) / 160 (proposal) | **40_000 default, 200_000 max** | maxPending 1-200 |
| opencode | 64 | - | - | path 安全 (discovery.ts:15-53) |
| pi | 64 | 1024 | - | - |
| jiuwenswarm | 64 (schema.py:638) | 1024 (schema.py:639) | - | - |
| TencentDB-Mem | 64 | 1024 | **50_000** (BODY_MAX) | total ≤ 50MB（design §3.5.1） |

---

## 11. laew 现状差距与 P0/P1/P2 路线图

### 11.1 laew 现状（基于 CLAUDE.md / 源码扫描）

**完全没有 Skill 系统**：

```
laew/
├── src/agent/tools/      ── Bash / Read / Write（无 Skill）
├── src/agent/            ── protocol loop（无 Skill registry）
├── src/llm/              ── anthropic / openai（无 skill injection）
├── docs/                 ── 无 Skill 专题
└── LsmAgentEmergentWork.db ── 无 skill 表
```

**与 8 个工程的差距**：

| 能力 | laew | 8 工程平均 | 差距 |
|---|---|---|---|
| Skill frontmatter | 0 | 100% | 100% |
| 多源发现 | 0 | 100% | 100% |
| 触发模型 | 0 | 100% | 100% |
| 生命周期 | 0 | 75% | 75% |
| 改进闭环 | 0 | 50% | 50% |
| 单元测试 | 0 | 100% | 100% |
| 安全扫描 | 0 | 50% | 50% |

### 11.2 P0（必须做）— 基础 Skill 系统

**目标**：在 laew 实现 SKILL.md 多源发现 + catalog 注入 + use_skill 工具

```rust
// P0.1: Skill 数据模型
// src/agent/skill/mod.rs
pub struct Skill {
    pub name: String,            // kebab-case, ≤ 64
    pub description: String,     // 必填, ≤ 1024
    pub version: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub source: SkillSource,     // Bundled / User / Project / Path
    pub body: String,
}

// P0.2: 4 源发现链
// src/agent/skill/discovery.rs
pub fn discover_skills() -> Vec<Skill> {
    // 1. Bundled — laew 内置（随 binary 分发）
    // 2. User — ~/.laew/skills/
    // 3. Project — <cwd>/.laew/skills/
    // 4. Path — LAEW_SKILLS_PATH 环境变量
}

// P0.3: 5 源加载 + gitignore 风格过滤（参照 pi）
pub fn load_skills_from_dir(dir: &Path) -> Vec<Skill> { ... }

// P0.4: 注入到 system prompt
// src/agent/skill/inject.rs
pub fn render_catalog(skills: &[Skill]) -> String {
    format!("<available_skills>\n{}\n</available_skills>", 
        skills.iter().map(|s| format!(
            "<skill>\n<name>{}</name>\n<description>{}</description>\n<location>{}</location>\n</skill>",
            s.name, s.description, s.file_path.display()
        )).collect::<Vec<_>>().join("\n"))
}

// P0.5: use_skill 工具
// src/agent/tools/skill.rs
pub struct UseSkillTool;
#[async_trait]
impl Tool for UseSkillTool {
    fn name(&self) -> &str { "use_skill" }
    async fn execute(&self, params: Value) -> Result<String> {
        let name = params["name"].as_str().unwrap();
        // 读 skill body + 注入到下一轮 user message
        // 标记 <<<LAEW:SKILL_BODY:{name}>>> 隔离
    }
}
```

**P0 验证标准**：
- `cargo test` 加 5 个 skill 单元测试（frontmatter 解析 / 路径发现 / 注入渲染 / use_skill 执行 / 错误处理）
- e2e 加 1 节：把 `~/.laew/skills/test-skill/SKILL.md` 注入 catalog，验证 system prompt 包含
- TUI 加 `/skills` 命令展示已发现 skill 列表

### 11.3 P1（应该做）— 改进闭环 + 测试

**目标**：SkillDevPipeline lite（5 阶段）+ Eval 集 + 安全扫描

```rust
// P1.1: 5 阶段 SkillDevPipeline
// 1. PLAN — 需求分析（复用 Yolo 入口层）
// 2. GENERATE — 生成 SKILL.md
// 3. VALIDATE — frontmatter 校验
// 4. EVALUATE — Eval 集（参考 jiuwenswarm test_design + test_run + evaluate）
// 5. PACKAGE — 打包为 laew 兼容的 .skill.zip

// P1.2: Eval 集 + Grader
// - evals.json schema（对齐 jiuwenswarm）
// - with_skill vs baseline 对照实验
// - benchmark.json 输出

// P1.3: 安全扫描器（5 类规则）
// - LINE_RULES / SOURCE_RULES / SKILL_CONTENT_RULES
// - provenance-aware child_process（参照 openclaw）
// - 反 prompt-injection（ignore all previous instructions 等）
// - 路径约束（assertInsideSkillsRoot）

// P1.4: 描述优化
// - 20 trigger queries（10 should_trigger + 10 should_not_trigger）
// - 60/40 train/test split
// - 5 轮 eval→improve 循环
// - 选 test score 最高的 description（防过拟合）
```

**P1 验证标准**：
- `cargo test` 加 10 个 skill 单元测试 + 5 个 scanner 单元测试
- e2e 加 2 节：skill create pipeline + 安全扫描拦截危险 skill
- docs 加 `docs/Skill系统设计/01-设计与解决方案.md` + `02-SkillDevPipeline.md`

### 11.4 P2（可以做）— Workshop 自演化 + 跨工程借鉴

**目标**：openclaw Workshop lite（5 阶段）+ 跨系统补偿事务

```rust
// P2.1: Workshop 5 阶段
// 1. propose — LLM 提提案（proposeCreateSkill / proposeUpdateSkill）
// 2. draft — 草稿持久化到 SQLite
// 3. policy — 策略机（agent-filter / bin 依赖 / OS 限制）
// 4. evaluate — 插件 hook 评估
// 5. apply — 原子 apply + rollback

// P2.2: 提案预算
// SkillProposalMutationBudget { remaining, successfulMutations, ... }
// autonomous.mode: "off" | "propose" | "auto"

// P2.3: 跨系统补偿事务（参考 TencentDB-Mem skill-versioning.ts）
// 1. writeFile → workspace（最不可靠，先做；失败零 DB 副作用）
// 2. SQLite appendVersion（失败：反向清 workspace）
// 3. onSkillCreated → meta_assets（失败：反向删 SQLite + 清 workspace）

// P2.4: collection-backup / collection-restore / collection-rollback
// 每次 apply 之前先做 backup，restore 是 inverse
// 限制 backup 数量和大小

// P2.5: 经验评估 + 历史扫描
// 按 cron 调度
// 收集 recent session transcripts
// LLM 决定 propose / apply / nothing
// 记录 outcome（completed / applied / proposed / nothing / failed）
```

**P2 验证标准**：
- `cargo test` 加 15 个 workshop 单元测试
- e2e 加 3 节：propose → policy → apply 完整流程 / rollback 验证 / experience-review
- TUI 加 `/workshop` 命令进入子屏

### 11.5 借鉴点速查（按工程）

| 借鉴点 | 来源 | 优先级 |
|---|---|---|
| SkillFirstHook（开轮强注入） | atomcode | P0 |
| SkillCatalogHook（position-based reconcile） | atomcode | P0 |
| 6 源发现 + 优先级 | claudecode / pi | P0 |
| 7 去重 + symlink 保护 | claudecode | P1 |
| skillify（自我扩展） | claudecode / openclaw | P1 |
| 5 层 SkopedLayers | deepseek-harness | P0 |
| chokidar 热重载 | deepseek-harness | P1 |
| 4 包分层（skill / provider / tool / badge） | deepseek-harness | P1 |
| Workshop 12 阶段状态机 | openclaw | P2 |
| provenance-aware child_process 检测 | openclaw | P1 |
| 5 类 scanner 规则 | openclaw | P1 |
| 提案预算（mutation budget） | openclaw | P2 |
| 跨系统补偿事务 | TencentDB-Mem | P2 |
| SkillCore 6 写 4 读 | TencentDB-Mem | P1 |
| SkillExtractor 抽取链路 | TencentDB-Mem | P1 |
| 乐观锁 + 权限三断言 | TencentDB-Mem | P1 |
| 12 阶段 SkillDevPipeline | jiuwenswarm | P1 |
| 3 模式自动识别（create/modify/with_resources） | jiuwenswarm | P1 |
| 3 挂起点 + SuspensionConfig | jiuwenswarm | P1 |
| 后端驱动 Todo + 弹窗 | jiuwenswarm | P1 |
| Eval 集（with_skill vs baseline） | jiuwenswarm | P1 |
| 描述优化（5 轮 eval→improve） | jiuwenswarm | P1 |
| Agent Skills 标准（frontmatter 严格） | pi | P0 |
| gitignore 风格过滤 | pi | P0 |
| 4 source 标识优先级 | pi | P0 |
| XML escape / display name 提取 | opencode / openclaw | P0 |
| Effect DI 拓扑 | opencode | P2 |

---

## 12. 附录 A：关键代码路径速查表（按工程）

### 12.1 jiuwenswarm

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `skilldev/DESIGN.md` | 1-700+ | 12 阶段完整设计 |
| `skilldev/pipeline.py` | 67-99 | run() 主循环 |
| `skilldev/pipeline.py` | 105-128 | resume() 恢复逻辑 |
| `skilldev/schema.py` | 23-49 | SkillDevStage 12 阶段枚举 |
| `skilldev/schema.py` | 218-303 | 3 个挂起点 SUSPENSION_POINTS |
| `skilldev/schema.py` | 317-486 | Eval / Grading / Benchmark 数据结构 |
| `skilldev/schema.py` | 547-590 | _STAGE_GROUPS + compute_todos |
| `skilldev/schema.py` | 627-642 | ALLOWED_FRONTMATTER_KEYS / SKILL_NAME_MAX_LEN / SKILL_DESC_MAX_LEN |
| `skilldev/service.py` | 48-55 | _METHOD_DISPATCH 7 个 method 路由 |
| `skilldev/service.py` | 76-122 | _handle_start 发起新任务 |
| `skilldev/service.py` | 129-160 | _handle_respond 统一确认入口 |
| `skilldev/stages/init_stage.py` | 1-128 | INIT 阶段 |
| `skilldev/stages/plan_stage.py` | 1-167 | PLAN 阶段 |
| `skilldev/stages/generate_stage.py` | 1-208 | GENERATE 阶段 |
| `skilldev/stages/validate_stage.py` | 30-153 | VALIDATE 阶段 + validate_skill_md |
| `skilldev/stages/test_design_stage.py` | 1-140 | TEST_DESIGN 阶段 + TEST_DESIGN_SYSTEM_PROMPT |
| `skilldev/stages/test_run_stage.py` | 1-143 | TEST_RUN 阶段 + with_skill vs baseline |
| `skilldev/stages/evaluate_stage.py` | 46-93 | GRADER_SYSTEM_PROMPT + ANALYST_SYSTEM_PROMPT |
| `skilldev/stages/evaluate_stage.py` | 184-242 | _aggregate_benchmark |
| `skilldev/stages/improve_stage.py` | 32-77 | IMPROVE_SYSTEM_PROMPT 改进哲学 |
| `skilldev/stages/package_stage.py` | 19-99 | PACKAGE 阶段 + _EXCLUDE_* |
| `skilldev/stages/desc_optimize_stage.py` | 55-77 | TRIGGER_QUERY_GEN_PROMPT |
| `skilldev/stages/desc_optimize_stage.py` | 88-105 | IMPROVE_DESC_PROMPT |
| `skilldev/stages/desc_optimize_stage.py` | 222-241 | _split_eval_set 60/40 分层 |
| `skilldev/stages/desc_optimize_stage.py` | 252-300 | _optimization_loop 5 轮 |
| `skilldev/store.py` | 1-84 | StateStore checkpoint 持久化 |
| `skilldev/workspace.py` | 1-65 | WorkspaceProvider 任务工作区 |
| `skilldev/context.py` | 1-114 | SkillDevContext 阶段运行环境 |
| `skilldev/deps.py` | 1-37 | SkillDevDeps 依赖注入 |

### 12.2 openclaw

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `skills/workshop/types.ts` | 12-19 | SkillProposalKind / SkillProposalStatus / EventType |
| `skills/workshop/types.ts` | 55-69 | SkillWorkshopProposalMutationBudget 提案预算 |
| `skills/workshop/types.ts` | 6 | SKILL_WORKSHOP_ROLLBACK_SCHEMA |
| `skills/workshop/config.ts` | 11-29 | SkillWorkshopConfig 默认值 |
| `skills/workshop/config.ts` | 35-50 | readAutonomousMode 三档 |
| `skills/workshop/service.ts` | 67-330 | reviseSkillProposal 完整流程 |
| `skills/workshop/service-propose.ts` | 1-... | propose 阶段 composeSkillBodyPatch / proposeCreateSkill / proposeUpdateSkill |
| `skills/workshop/proposal-draft.ts` | 27-66 | prepareSkillProposalDraft |
| `skills/workshop/proposal-draft.ts` | 194 | description ≤ 160 字节 |
| `skills/workshop/service-evaluation.ts` | 1-150 | evaluateSkillProposal 评估 |
| `skills/workshop/service-evaluation.ts` | 11-13 | MAX_EVALUATION_OUTCOMES / FINDINGS / METRICS |
| `skills/workshop/experience-review.ts` | 1-200 | runSkillExperienceReview + runWithGatewayIndependentRootWorkAdmission |
| `skills/workshop/experience-review-scheduler.ts` | 1-... | cron 调度 |
| `skills/workshop/history-scan.ts` | 1-120 | toStoredState 状态机持久化 |
| `skills/workshop/history-scan-candidates.ts` | 1-... | selectSkillHistoryScanCandidates |
| `skills/workshop/curator.ts` | 25 | SKILL_LIFECYCLE_CURATION_RETIRED_MESSAGE |
| `skills/workshop/curator.ts` | 88-95 | readSkillUsageByFile usage facts |
| `skills/workshop/apply-transition.ts` | 62-78 | SKILL_PROPOSAL_APPLY_TRANSITIONS 状态转换表 |
| `skills/workshop/reconcile-transition.ts` | 1-... | 重对账 |
| `skills/workshop/plugin-hooks.ts` | 1-100 | createSkillProposalEvent + dispatchSkillProposalChanged |
| `skills/workshop/plugin-hooks.ts` | 17 | MAX_SKILL_PROPOSAL_CORRELATION_ID_LENGTH = 256 |
| `skills/workshop/proposal-scan.ts` | 1-... | scanProposalBundle 安全扫描 |
| `skills/workshop/proposal-origin-validation.ts` | 1-... | 来源校验 |
| `skills/workshop/collection-backup.ts` | 1-... | collection 备份 |
| `skills/workshop/collection-restore.ts` | 1-... | collection 恢复 |
| `skills/workshop/collection-rollback.ts` | 1-... | collection 回滚 |
| `skills/workshop/collection-review-state.ts` | 1-100 | review outcome 状态 |
| `skills/workshop/store-sqlite-event.ts` | 1-... | event 表 |
| `skills/workshop/store-sqlite-record.ts` | 1-... | record 表 |
| `skills/workshop/store-sqlite-rollback.ts` | 1-... | rollback 表 |
| `skills/workshop/store-sqlite-transition.ts` | 1-99 | transition 表 |
| `skills/workshop/target-lock.ts` | 1-73 | per-target 写锁 |
| `skills/loading/session.ts` | 23 | MAX_DESCRIPTION_LENGTH = 1024 |
| `skills/loading/session.ts` | 45-50 | validateName |
| `skills/loading/session.ts` | 62-70 | validateDescription |
| `skills/loading/skill-contract.ts` | 25-30 | escapeSkillXml / decodeSkillXml |
| `skills/loading/skill-contract.ts` | 35-45 | resolveSkillDisplayName |
| `skills/loading/skill-contract.ts` | 60-65 | truncateSkillDescription |
| `skills/loading/skill-contract.ts` | 100-130 | compactSkillsPromptForContext 二分搜索 |
| `skills/security/scanner.ts` | 80-86 | cache & dir limit |
| `skills/security/scanner.ts` | 148-184 | LINE_RULES（4 条） |
| `skills/security/scanner.ts` | 189-219 | SOURCE_RULES（3 条） |
| `skills/security/scanner.ts` | 221-260 | SKILL_CONTENT_RULES（7 条） |
| `skills/security/scanner.ts` | 268-336 | Core scanner 主体 |
| `skills/security/scanner.ts` | 336-413 | provenance-aware child_process 检测 |

### 12.3 TencentDB-Mem

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `skill/skill-core.ts` | 14-30 | 6 写 4 读 注释 |
| `skill/skill-core.ts` | 55-72 | SkillCoreError 14 错误码 |
| `skill/skill-core.ts` | 94-130 | SkillCoreOptions 注入 |
| `skill/skill-core.ts` | 220-242 | SkillCore 构造 + notifyAccessed |
| `skill/skill-core.ts` | 244-300 | create 跨系统 |
| `skill/skill-core.ts` | 302-326 | update 乐观锁 + 跨版本 name 不变 |
| `skill/skill-core.ts` | 327-364 | patch 唯一性 + 乐观锁 |
| `skill/skill-core.ts` | 366-390 | delete 物理真删 |
| `skill/skill-core.ts` | 392-411 | writeFiles |
| `skill/skill-core.ts` | 413-444 | removeFiles |
| `skill/skill-core.ts` | 446-470 | get |
| `skill/skill-core.ts` | 472-493 | list + search |
| `skill/skill-core.ts` | 495-510 | listVersions |
| `skill/skill-core.ts` | 512-525 | readFile |
| `skill/skill-format.ts` | 17-23 | 4 个常量 NAME_MAX / DESCRIPTION_MAX / BODY_MAX / NAME_REGEX |
| `skill/skill-format.ts` | 34-118 | parseSkillFile 严格 + 宽容 |
| `skill/skill-format.ts` | 124-145 | validateSkillFile 断言 |
| `skill/skill-format.ts` | 194-202 | formatSkillFile round-trip 安全 |
| `skill/skill-versioning.ts` | 90-130 | 跨系统补偿事务（COS + skill DB + meta_assets） |
| `skill/skill-permission.ts` | 1-68 | assertOwner / assertTeamMatch / assertVersionFresh |
| `skill/skill-tools.ts` | 1-208 | SkillToolsV2 6 tool（4 写 2 读） |
| `skill/skill-extractor.ts` | 1-372 | SkillExtractor LLM 抽取链路 |
| `skill/skill-store.ts` | 1-733 | SqliteSkillStore CRUD + search + delete |
| `skill/skill-store-ddl.ts` | 1-104 | skills / skill_fts / skill_vec DDL |

### 12.4 pi

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `core/skills.ts` | 14-16 | MAX_NAME_LENGTH / MAX_DESCRIPTION_LENGTH |
| `core/skills.ts` | 87-100 | validateName 4 校验 |
| `core/skills.ts` | 102-112 | validateDescription 3 校验 |
| `core/skills.ts` | 178-242 | gitignore 风格过滤（4 个 IGNORE_FILE_NAMES） |
| `core/skills.ts` | 301-321 | frontmatter 验证 |
| `core/skills.ts` | 355-381 | 2 source 注入格式 |
| `core/skills.ts` | 407-507 | 多源发现 |
| `core/skills.ts` | 419-447 | 冲突处理 + realpath 去重 |
| `core/skills.ts` | 467-473 | 4 优先级 source 标识 |
| `harness/skills.ts` | 1-100 | formatSkillInvocation |
| `harness/skills.ts` | 50-80 | loadSkills ExecutionEnv 抽象 |
| `harness/skills.ts` | 90-130 | loadSourcedSkills |
| `test/skills.test.ts` | 1-432 | 9 个 fixture + 9 个 describe/it |
| `test/sdk-skills.test.ts` | 1-200 | SDK 入口测试 |
| `test/suite/regressions/2781-skill-collision-precedence.test.ts` | 1-100 | 碰撞优先级回归 |

### 12.5 opencode

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `core/src/skill.ts` | 1-133 | SkillV2 Effect-based |
| `core/src/skill/discovery.ts` | 1-137 | 远程 SkillDiscovery |
| `core/src/skill/discovery.ts` | 15-53 | 路径安全 |
| `core/src/skill/index.ts` | 173-246 | Skill 加载管线 |
| `core/src/skill/index.ts` | 310-315 | Permission 过滤 |
| `core/src/skill/index.ts` | 321-346 | Prompt 格式 |
| `core/src/tool/skill.ts` | 35-52 | skill tool 输出 |
| `schema/src/skill.ts` | 7-55 | 3 Source 类型 |
| `schema/src/skill.ts` | 19-26 | Skill 信息 |
| `test/skill/skill.test.ts` | 1-585 | fixture + 11 个 test |
| `test/tool-skill.test.ts` | 1-... | tool 输出测试 |
| `test/fixture/skills/` | - | agents-sdk / cloudflare / index.json |

### 12.6 atomcode

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `capabilities/src/skills/skill.rs` | 1-519 | Skill 数据结构 + 变量替换 |
| `capabilities/src/skills/registry.rs` | 1-571 | 12 目录标准发现链（registry.rs:238-253） |
| `capabilities/src/skills/registry.rs` | 238-253 | 12 目录 |
| `capabilities/src/skills/render.rs` | 1-401 | 8KB 预算门控（render.rs:25-178） |
| `capabilities/src/skills/render.rs` | 25-178 | 8KB 预算 + 源优先级 |
| `capabilities/src/skills/catalog_hook.rs` | 1-128 | **SkillCatalogHook** 注入 |
| `capabilities/src/skills/use_skill.rs` | 1-316 | use_skill 工具执行 |
| `coding/src/skill_first.rs` | 1-146 | **SkillFirstHook** 开轮强注入 |
| `coding/src/skill_first.rs` | 53-60 | body() forceful reminder |

### 12.7 claudecode

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `skills/loadSkillsDir.ts` | 638-803 | 6 源加载管线 |
| `skills/loadSkillsDir.ts` | 118-124 | 7 去重 + symlink 保护 |
| `skills/loadSkillsDir.ts` | 1082-1086 | MCP Skill 双向桥 |
| `skills/bundledSkills.ts` | 15-42 | SkillDefinition 数据结构 |
| `skills/bundledSkills.ts` | 131-193 | 资源懒提取安全 |
| `skills/bundled/index.ts` | 24-69 | 16 个内置 Skill 列表 |
| `skills/skill-creator/SKILL.md` | - | self-skillify（与 openclaw 同源） |
| `skills/skill-creator/scripts/quick_validate.py` | 1-50 | frontmatter 校验器 |

### 12.8 deepseek-harness

| 文件 | 行号 | 关键内容 |
|---|---|---|
| `packages/skill/skill/src/index.ts` | 38-93 | Skill 数据模型 |
| `packages/skill/skill/src/index.ts` | 171-216 | renderSkillContent 统一渲染 |
| `packages/skill/skill/src/index.ts` | 327-461 | 5 层 SkopedLayers |
| `packages/skill/skill/src/index.ts` | 357-869 | Registry + Provider + Consumer |
| `packages/skill/skill/src/index.ts` | 520-660 | revision + LRU 缓存 |
| `packages/skill/skill-filesystem/src/index.ts` | 146-262 | FileSystemSkillProvider |
| `packages/skill/skill-filesystem/src/index.ts` | 284-597 | Chokidar 文件监视 |
| `packages/skill/tool-skill/src/index.ts` | 81-160 | `skill` loader tool |
| `packages/skill/tool-skill/src/index.ts` | 177-204 | `/name` 用户手势 |
| `packages/skill/tool-skill/src/index.ts` | 254-389 | catalog 持久化为 user message |
| `packages/skill/skill-badge/src/index.ts` | - | Bundled dsh-badge |
| `tests/skill-filesystem-watcher.spec.ts` | 1-444 | 9 个 describe/it + 4 fixture |

---

## 13. 附录 B：关键文件行数表

| 工程 | 模块 | 总行数 | 关键文件数 |
|---|---|---|---|
| atomcode | skills + skill_first | 2134 | 6 + 1 |
| claudecode | skills | ~6000 | 5+（含 16 builtin） |
| deepseek-harness | 4 包 | ~4500 | 12+ |
| openclaw | **Workshop** | **~18000** | **80+** |
| openclaw | security/scanner | 1038 | 1 |
| opencode | skill | ~379 | 5 |
| pi | core + harness | 893 | 2 |
| pi | test (skills + sdk + 2781) | ~730 | 3 |
| **jiuwenswarm** | **skilldev** | **~2700** | **12 文件 + DESIGN 700+** |
| jiuwenswarm | tests | ~3000+ | 30+ |
| TencentDB-Mem | **skill** | **~3897** | **15** |

**总计**：~41000 行 Skill 相关代码（不含测试 ~5000 行）

---

## 14. 总结

本轮深挖发现，**「Skill 一等公民」**不是一个文件格式问题，而是一个 **资产生命周期管理问题**。8 个工程在以下 5 维度都给出了参考实现：

1. **SkillDevPipeline 12 阶段**（jiuwenswarm）—— 端到端从需求到打包的确定性状态机，对标官方 anthropic skill-creator。
2. **Workshop 12 阶段自演化**（openclaw）—— 提案驱动 + 多状态机 + 隔离执行 + rollback，18000 行 TypeScript。
3. **SkillCore 6 写 4 读**（TencentDB-Mem）—— 跨系统补偿事务 + 乐观锁 + 权限三断言 + onSkillAccessed 自愈补登记。
4. **Skill 测试 / 评测**—— 三档：单元测试（pi 432 / opencode 585 / openclaw scanner 1038）+ Eval 集（jiuwenswarm with_skill vs baseline）+ 安全扫描（openclaw 5 类规则 + provenance-aware）。
5. **Skill 安全**—— 签名 / 来源 / 沙箱 / 审计 4 维度，openclaw scanner 给出最完整参考。

**laew P0/P1/P2 路线图**：
- P0：SKILL.md frontmatter + 4 源发现 + catalog 注入 + use_skill 工具（5 单元测试 + 1 e2e + /skills TUI 命令）
- P1：5 阶段 SkillDevPipeline + Eval 集 + 5 类 scanner + 描述优化（10 单元测试 + 2 e2e + docs）
- P2：Workshop 5 阶段 + 提案预算 + 跨系统补偿事务 + collection-backup/restore/rollback（15 单元测试 + 3 e2e + /workshop TUI 子屏）

**总借鉴点**：25 项（11 P0 + 8 P1 + 6 P2），全部已在 12 章节展开。

**关键发现**：
1. **「Skill 一等公民」= 完整状态机 + 评测 + 回滚**（不是文件格式）
2. **3 模式自动识别**（create / with_resources / modify）是 jiuwenswarm 独特设计
3. **5 类 scanner 规则 + provenance-aware child_process** 是 openclaw 独特设计
4. **跨系统补偿事务（COS + skill DB + meta_assets）** 是 TencentDB-Mem 独特设计
5. **6 写 4 读 SkillCore 门面 + onSkillAccessed 自愈补登记** 是 TencentDB-Mem 独特设计
6. **12 阶段 SkillDevPipeline + 后端驱动 Todo + 3 挂起点** 是 jiuwenswarm 独特设计
7. **提案预算（mutation budget）+ autonomous.mode 三档** 是 openclaw 独特设计
8. **gitignore 风格过滤（4 IGNORE_FILE_NAMES）** 是 pi 独特设计
9. **SkillFirstHook + SkillCatalogHook** 是 atomcode 独特设计
10. **laew 零 Skill 系统** —— 是 8 工程中唯一一个无任何 Skill 模块的工程，P0 优先级最高
