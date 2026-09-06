# 专题-第六轮-Skill系统深度对比

> **范围**: atomcode / claudecode / deepseek-harness / openclaw / opencode / pi 六个项目 Skill 系统的逐行源码深度对比
> **专题定位**: laew 当前完全无 Skill 系统（CLAUDE.md / AGENTS.md 注明「内置工具仅 Bash/Read/Write」，MultiAgentOrchestrator 也未挂 Skill），这是最大的设计缺口之一；本文档为 P0 路线图提供完整借鉴蓝图
> **数据来源**: 6 个项目仓库实际源码 + 100+ 文件 ~10k 行精读

---

## 目录

1. [Skill 系统在大模型 Agent 中的位置](#1-skill-系统在大模型-agent-中的位置)
2. [atomcode 的 Skill 系统 (L1)](#2-atomcode-的-skill-系统-l1)
3. [claudecode 的 Skill 系统（最成熟 / 16 个内置）](#3-claudecode-的-skill-系统最成熟--16-个内置)
4. [deepseek-harness 的 Skill 系统（4 个包 / registry + provider）](#4-deepseek-harness-的-skill-系统4-个包--registry--provider)
5. [openclaw 的 Skill 系统（Workshop 自演化 + 52 extensions）](#5-openclaw-的-skill-系统workshop-自演化--52-extensions)
6. [opencode 的 Skill 系统（远程拉取 + Effect 依赖注入）](#6-opencode-的-skill-系统远程拉取--effect-依赖注入)
7. [pi 的 Skill 系统（Agent Skills 标准 + 一等公民）](#7-pi-的-skill-系统agent-skills-标准--一等公民)
8. [综合对比维度表](#8-综合对比维度表)
9. [六项目 Skill 文件格式示例](#9-六项目-skill-文件格式示例)
10. [laew 当前 Skill 系统缺口与 P0 借鉴路线图](#10-laew-当前-skill-系统缺口与-p0-借鉴路线图)
11. [laew Skill 系统最小可行产品（MiniPoC）代码蓝图](#11-laew-skill-系统最小可行产品minipoc代码蓝图)

---

## 1. Skill 系统在大模型 Agent 中的位置

### 1.1 Skill 是什么

Skill 是「**可重用的、按需加载的、模型自调度的指令包**」。它是 Slash Command、Tool、Workflow、Agent Memory 之外的第五类一等公民资源。

与其它四类的对比：

| 维度 | Slash Command | Tool | Workflow | Agent Memory | **Skill** |
|------|--------------|------|----------|-------------|-----------|
| 形态 | 命令名 + 参数 | JSON Schema 函数调用 | DAG 节点编排 | 历史摘要 | Markdown + frontmatter |
| 调用方 | 用户 `/cmd` | LLM 工具调用 | LLM/Runtime | 框架自动注入 | LLM/用户/手势 |
| 加载时机 | 即时 | 即时 | 即时 | 启动 | **按需 (progressive disclosure)** |
| 信任边界 | 受信 | 受 sandbox 限制 | 受 sandbox 限制 | 受信 | 受信 |
| 内容 | 单段 prompt | 函数实现 | 多步任务 | 历史摘要 | 完整 SOP |

### 1.2 六项目 Skill 设计的共同底层哲学

**Progressive Disclosure（渐进披露）**：永远只在 system prompt 里塞「**名字 + 短描述**」（如 `- pdf: read & merge PDFs`），**完整 body 在模型需要时通过 `use_skill` / `read` / `skill` tool 加载**。

这是 Claude-Code 在 2026-04 引入 `Skill` 后的业界共识：避免「100 个 community skill 全部塞进 prompt 导致信号稀释」（`docs/Agent源码调研/atomcode.md` 提及此痛点）。

```
catalog（名字 + 描述，800B~8KB）  →  始终在 system prompt
   ↓ LLM 决策「描述匹配」
skill tool / use_skill / read  →  加载完整 body（5-50KB）
   ↓ 跟随 body 内的 SOP
shell / bash / scripts/  →  执行具体步骤
```

### 1.3 六项目 Skill 的差异光谱

```
极简 ──────────────────────────────────────────────── 极复杂
pi              atomcode      opencode      deepseek-harness   claudecode      openclaw
(Agent Skills   (Claude-Code  (Effect +     (Registry +        (Six-source      (Workshop
 标准严格)       兼容)        URL 拉取)     Provider)          pipeline +       self-evolution +
                                               |              conditional      ClawHub)
                                               |               dynamic)              |
                                            4 个包           16 bundled             52 extensions
```

---

## 2. atomcode 的 Skill 系统 (L1)

### 2.1 仓库定位

`crates/atomcode-capabilities/src/skills/` —— L1 capability，**与 kernel 解耦**（`mod.rs:9-10` 注释明确写「Behind the opt-in `skills` cargo feature」）。

```
atomcode-capabilities/src/skills/
├── mod.rs            (54 lines)  ── skill_tool_names() / register_skill_tools()
├── skill.rs          (519 lines) ── Skill struct + expand() + frontmatter 解析
├── registry.rs       (571 lines) ── SkillRegistry (BTreeMap) + 标准 12 目录发现链
├── render.rs         (401 lines) ── render_skill_catalog() 预算门控 + 源优先级排序
├── use_skill.rs      (316 lines) ── UseSkillTool + ListSkillsTool 实现
└── catalog_hook.rs   (128 lines) ── SkillCatalogHook 启动时注入 catalog 到 conversation
```

### 2.2 Skill 文件格式（`skill.rs:12-26`）

```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub template: String,                 // body
    pub allowed_tools: Vec<String>,       // metadata only，不强制
    pub user_invocable: bool,             // 默认 true
    pub skill_dir: PathBuf,               // ${CLAUDE_SKILL_DIR}
    pub source_path: PathBuf,
}
```

两种文件格式都接受：
1. **目录式** `<name>/SKILL.md`（推荐，能打包 scripts/）
2. **平铺式** `<name>.md`（slash-command 风格）

frontmatter 字段（`skill.rs:196-214`）：
- `name`：可选，默认取目录名/文件名
- `description`：可选，缺省时取 body 首段
- `allowed-tools`：空格 OR 逗号分隔的 tool 列表
- `user-invocable: false`：隐藏 `/` 菜单，模型仍可自动触发

### 2.3 变量替换引擎（`skill.rs:32-92`）

支持的 token（`skill.rs:117-148` `match_substitution`）：
- `$ARGUMENTS` 全部参数
- `$ARGUMENTS[N]` 0-based 位置参数
- `$N` 同上（最长 digit run，$10 ≠ $1+0）
- `${CLAUDE_SESSION_ID}`
- `${CLAUDE_SKILL_DIR}`
- `` !`cmd` `` shell 注入（设计如此：skills 是 trusted user-authored）

**单次左到右扫描**，替换值不再二次展开（避免 `$1` 参数被再次展开成 `$2`）

### 2.4 12 目录标准发现链（`registry.rs:238-253`）

```rust
pub fn standard_skill_dirs(home: &Path, project: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".claude/commands"),       // user-level (CLI compat)
        home.join(".atomcode/commands"),
        home.join(".claude/skills"),         // user-level skills
        home.join(".agents/skills"),         // 跨 agent 共享 (opencode 等)
        home.join(".atomcode/skills"),       // atomcode-native
        project.join(".claude/commands"),    // project-level
        project.join(".atomcode/commands"),
        project.join(".claude/skills"),
        project.join(".agents/skills"),
        project.join(".atomcode/skills"),
    ]
}
```

**优先级 = 后赢**：列表后置的目录同名 skill 覆盖前者。`.atomcode` 最高，确保本机原生 skill 覆盖第三方。

### 2.5 8KB 预算门控 + 源优先级（`render.rs:25-178`）

```rust
pub const CATALOG_BYTE_BUDGET: usize = 8000;
pub const PER_SKILL_DESC_CAP: usize = 1024;
pub const CATALOG_HEADER: &str = "=== AVAILABLE SKILLS ===";
```

当 catalog 超 8000B：
1. **最高 priority 保留**（`source_rank`: `0=atomcode-native, 1=.claude, 2=.agents, 3=其他`）
2. **项目指令精确引用的 skill 名优先**（`render_catalog_prioritizing`，`registry.rs:187-205`）
3. 剩余条目汇总成 `... and N more lower-priority skills not shown.`

**GUIDANCE 段落强制约束**（`render.rs:35` 完整文本）：
> "Match a task only against descriptions actually shown below. ... If a task clearly matches a shown skill's description — not only when the user names the skill — you MUST load that exact skill with `use_skill` and follow it BEFORE doing the work, INCLUDING before asking clarifying questions..."

`SkillCatalogHook`（`catalog_hook.rs`）把这段 catalog 作为 `Role::System` 第 N 条消息插入（在 persona + session context 之后）：

```rust
// catalog_hook.rs:36-42
fn leading_system_count(convo: &Conversation) -> usize {
    convo.messages.iter()
        .take_while(|m| m.role == Role::System)
        .count()
}
```

`--resume` 时按 `CATALOG_HEADER` 字符串前缀原地 reconcile，不增长消息数。

### 2.6 关键设计要点

- **BTreeMap 存储**：保证 catalog 注入顺序字节稳定 → 触发 prompt prefix caching（`registry.rs:11-12` 注释明确解释）
- **namespaced 技能命名**：`load_dir(dir, Some("plugin"))` 把 skill 存为 `plugin:<name>`，但 `get("name")` 回退裸名查找（`registry.rs:95-126`）
- **allowed_tools 仅 metadata**：不强制（注释明确说「L2 approval-policy concern」）
- **shell 注入是 by design**：`skill.rs:4-6` 注释：`Skills are TRUSTED, user-authored content`

---

## 3. claudecode 的 Skill 系统（最成熟 / 16 个内置）

### 3.1 仓库定位

`src/skills/` —— claudecode 把 Skill 当成 **PromptCommand** 抽象（与 SlashCommand 共享底层）。

```
src/skills/
├── bundledSkills.ts        (220 lines) ── registerBundledSkill + 文件安全提取
├── loadSkillsDir.ts        (1086 lines) ── 完整 6 源加载 + dynamic + conditional
├── mcpSkillBuilders.ts     (45 lines)   ── 注册表模式避免循环依赖
└── bundled/                ── 16 个内置 Skill 的 .ts 文件
    ├── index.ts            ── initBundledSkills() 总入口
    ├── batch.ts, claudeApi.ts, claudeInChrome.ts, debug.ts, keybindings.ts,
    │   loop.ts, loremIpsum.ts, remember.ts, scheduleRemoteAgents.ts,
    │   simplify.ts, skillify.ts, stuck.ts, updateConfig.ts, verify.ts,
    │   verifyContent.ts
```

### 3.2 内置 Skill 列表（`bundled/index.ts:24-69`）

```
registerUpdateConfigSkill()      ── /update-config
registerKeybindingsSkill()       ── /keybindings
registerVerifySkill()            ── /verify
registerDebugSkill()             ── /debug
registerLoremIpsumSkill()        ── /lorem-ipsum
registerSkillifySkill()          ── /skillify 把 session 转成 skill
registerRememberSkill()          ── /remember
registerSimplifySkill()          ── /simplify
registerBatchSkill()             ── /batch
registerStuckSkill()             ── /stuck
registerDreamSkill()             ── /dream (KAIROS feature flag)
registerHunterSkill()            ── /hunter (REVIEW_ARTIFACT feature flag)
registerLoopSkill()              ── /loop (AGENT_TRIGGERS feature flag)
registerScheduleRemoteAgentsSkill() ── /schedule-remote-agents
registerClaudeApiSkill()         ── /claude-api (BUILDING_CLAUDE_APPS)
registerClaudeInChromeSkill()    ── chrome 自动启用
registerRunSkillGeneratorSkill() ── /run-skill-generator (RUN_SKILL_GENERATOR)
```

共 **16+ 个内置 Skill**，按 feature flag 动态启用（`bundled/index.ts:42-69`）。

### 3.3 SkillDefinition 数据结构（`bundledSkills.ts:15-42`）

```typescript
export type BundledSkillDefinition = {
  name: string
  description: string
  aliases?: string[]
  whenToUse?: string            // 独立于 description 的"何时用"提示
  argumentHint?: string         // [issue-number] 形式
  allowedTools?: string[]
  model?: string                // 强制指定模型
  disableModelInvocation?: boolean
  userInvocable?: boolean
  isEnabled?: () => boolean     // 动态开关
  hooks?: HooksSettings         // skill 自己的 hooks（27 种 Hook 之一）
  context?: 'inline' | 'fork'   // 'fork' = 子 agent 中跑
  agent?: string                // 跨 agent 调用
  files?: Record<string, string>  // 内嵌资源（懒提取到磁盘）
  getPromptForCommand: (args, ctx) => Promise<ContentBlock[]>
}
```

### 3.4 六源加载管线（`loadSkillsDir.ts:638-803`）

```typescript
// loadSkillsDir.ts:638-803 ── getSkillDirCommands()
async function getSkillDirCommands(cwd: string): Promise<Command[]> {
  // 1) Managed (policy 强制): <managed>/.claude/skills/
  // 2) User: ~/.claude/skills/
  // 3) Project: <cwd>/.claude/skills/ (向上遍历到 home)
  // 4) Additional dirs (--add-dir): <add-dir>/.claude/skills/
  // 5) Legacy commands: <cwd>/commands/ (deprecated)
  // 6) Dynamic (运行时): <file>/.claude/skills/ (按需)
  //    + Conditional (path frontmatter 匹配)
}
```

并行加载 5 个独立 Promise（`loadSkillsDir.ts:679-714`）：
```typescript
const [
  managedSkills,
  userSkills,
  projectSkillsNested,
  additionalSkillsNested,
  legacyCommands,
] = await Promise.all([...])
```

### 3.5 7 去重重 + symlink 保护（`loadSkillsDir.ts:118-124`）

```typescript
async function getFileIdentity(filePath: string): Promise<string | null> {
  return await realpath(filePath)  // resolve 软链
}
// → dedup 列表用 realpath canonical 比较（处理 node_modules 软链）
```

### 3.6 4 触发方式

| 触发 | 实现 | 文件 |
|------|------|------|
| **用户 `/name`** | `/` 菜单自动补全 | `tui/completion.ts` |
| **模型决策** | `use_skill` tool (`loadSkillsDir.ts:344-399`) | 系统 prompt 注入 catalog |
| **条件触发** | `paths` frontmatter + gitignore-style 匹配 | `loadSkillsDir.ts:997-1058` |
| **动态发现** | 文件 Read/Write/Edit 时按 path 向上搜索 | `loadSkillsDir.ts:861-915` |

### 3.7 conditional skills (paths frontmatter)

`loadSkillsDir.ts:159-178`：
```typescript
function parseSkillPaths(frontmatter: FrontmatterData): string[] | undefined {
  const patterns = splitPathInFrontmatter(frontmatter.paths)
    .map(pattern => pattern.endsWith('/**') ? pattern.slice(0, -3) : pattern)
    .filter(p => p.length > 0)
  if (patterns.every(p => p === '**')) return undefined
  return patterns
}
```

启动时不加载，仅当 Read/Write/Edit 操作触发的文件路径匹配 `paths` 模式时才激活（用 `ignore` 库 gitignore 语法）。激活后移到 `dynamicSkills` map。

### 3.8 dynamic skill discovery（运行时）

`loadSkillsDir.ts:861-975`：
```typescript
// 当 Read/Write/Edit 一个路径时：
// 1) 从路径 parent 向上到 cwd
// 2) 每层 stat `<dir>/.claude/skills/`
// 3) 检查 `git check-ignore`（block node_modules/.claude/skills）
// 4) 按深度 deepest-first 排序（深的优先）
// 5) addSkillDirectories() 合并到 dynamicSkills map
```

### 3.9 skillify（自我扩展）

`bundled/skillify.ts` —— **最有创新性的内置 Skill**：

> 把当前 session 转成可复用的 Skill：分析 session memory + 用户消息历史，生成新的 `SKILL.md`

`skillify.ts:11-44`：
```typescript
const SKILLIFY_PROMPT = `# Skillify {{userDescriptionBlock}}

You are capturing this session's repeatable process as a reusable skill.

## Your Session Context
<session_memory>{{sessionMemory}}</session_memory>
<user_messages>{{userMessages}}</user_messages>

## Your Task
### Step 1: Analyze the Session ...`
```

这与 openclaw 的 `skill-creator` skill（Workshop 自演化）异曲同工——表明**自演化 Skill 是行业共识**。

### 3.10 资源懒提取安全（`bundledSkills.ts:131-193`）

bundled skill 可声明 `files: Record<string, string>`（内嵌资源），**首次调用时一次性提取到带 nonce 的子目录**：

```typescript
async function extractBundledSkillFiles(skillName, files) {
  const dir = getBundledSkillExtractDir(skillName)  // <root>/<nonce>/<skillName>
  await writeSkillFiles(dir, files)
}

// writeSkillFiles 用 O_NOFOLLOW|O_EXCL 防 symlink 攻击（bundledSkills.ts:179-193）
const SAFE_WRITE_FLAGS =
  process.platform === 'win32' ? 'wx' : fsConstants.O_WRONLY |
  fsConstants.O_CREAT | fsConstants.O_EXCL | O_NOFOLLOW

// resolveSkillFilePath: 拒绝 '..' / 绝对路径 / 路径穿越（bundledSkills.ts:196-206）
```

### 3.11 MCP Skill 双向桥（`loadSkillsDir.ts:1082-1086`）

```typescript
// 把 createSkillCommand + parseSkillFrontmatterFields 暴露给 MCP skill discovery
// 通过 mcpSkillBuilders.ts 叶子模块打破 client.ts ↔ loadSkillsDir.ts 循环依赖
registerMCPSkillBuilders({ createSkillCommand, parseSkillFrontmatterFields })
```

**MCP server 可以暴露自己的 skill 给 client**（详见 MCP 文档）。

### 3.12 关键设计要点

- **六源优先级 + 5 个并行 Promise**：高吞吐
- **realpath 去重**：防 node_modules 软链引起的虚假冲突
- **`git check-ignore` 守护**：node_modules/.claude/skills 不被加载
- **conditional + dynamic 混合**：启动时 catalog 干净，运行时按需激活
- **bundled skill 资源懒提取 + O_NOFOLLOW|O_EXCL**：攻击面最小化
- **hook 链嵌入**：每个 skill 可挂 27 种 Hook 之一（PreToolUse 等）

---

## 4. deepseek-harness 的 Skill 系统（4 个包 / registry + provider）

### 4.1 仓库定位

`packages/skill/` —— **4 包分离**，**完全插件化**：

```
packages/skill/
├── skill/            ── @deepseek-ai/dsh-skill  (registry, 869 lines)
├── skill-filesystem/ ── @deepseek-ai/dsh-skill-filesystem (FS provider, 1042 lines)
├── skill-badge/      ── @deepseek-ai/dsh-skill-badge (bundled provider, 61 lines)
└── tool-skill/       ── @deepseek-ai/dsh-tool-skill (consumer, 431 lines)
```

### 4.2 架构：Registry + Provider + Consumer（`skill/src/index.ts:357-869`）

```typescript
class SkillRegistry extends Service {  // skill/src/index.ts:357
  // Provider 注册：sync 在 apply()，async 工作延迟到 list()
  registerProvider(create: (control) => SkillProvider): () => void

  // Runtime 注册
  register(skill: SkillRegistration): () => void

  // 视图接口
  async list(options?: SkillViewOptions): Promise<SkillSummary[]>
  async snapshot(options?: SkillViewOptions): Promise<SkillCatalogSnapshot>
  async get(name: string, options?: SkillViewOptions): Promise<SkillDefinition | undefined>
}
```

### 4.3 Skill 数据模型（`skill/src/index.ts:38-93`）

```typescript
type SkillSource = 'project-dsh' | 'project-agents' | 'runtime'
                  | 'user-dsh' | 'user-agents' | 'custom' | 'bundled'

interface SkillInvocationPolicy {
  readonly modelInvocable: boolean   // 模型可否触发
  readonly userInvocable: boolean    // 用户可否 /name
}

interface SkillSummary {  // 列表用
  name, description, whenToUse?, invocation, source, provider, resourceBase?
}

interface SkillCandidate extends SkillSummary {
  rank: number                  // 数字越小优先级越高
  locator: unknown              // provider-specific 句柄
  path?, metadata?
}

interface SkillDefinition extends SkillSummary {
  content: string               // 完整 body
  path?, metadata?
}
```

### 4.4 5 层 SkopedLayers（`skill/src/index.ts:327-461`）

```typescript
// 借鉴 tools registry 的 shadowing rule
class SkillLayer implements ScopeLayer {
  readonly providers: NamedEntries<RegisteredProvider>  // 同步注册
  readonly runtime = new Map<string, SkillDefinition>() // 运行时注册
}

// global layer + agent preset 链式 layer
// 同一层内：rank → provider 注册 order → local order
// 跨层：最近层胜（chain shadowing rule，nearest layer wins outright）
```

`registerProvider` 是 `cordis` `effect()` 封装，自动管 lifecycle / teardown ordering。

### 4.5 FileSystemSkillProvider（`skill-filesystem/src/index.ts:146-262`）

5 级根目录优先级（`skill-filesystem/src/index.ts:241-261`）：
```typescript
private async roots(cwd: string): Promise<SkillRoot[]> {
  if (this.includeDefaultRoots && cwd !== undefined) {
    const projectRoot = await findProjectRoot(resolve(cwd), ...)
    roots.push(
      { path: join(projectRoot, '.dsh/skills'),     rank: 100, source: 'project-dsh' },
      { path: join(projectRoot, '.agents/skills'),  rank: 200, source: 'project-agents' },
    )
  }
  roots.push(...this.customSkillDirs.map(... => ({ rank: 300, source: 'custom' })))
  if (this.includeDefaultRoots) {
    roots.push(
      { path: join(this.dshHome, 'skills'),    rank: 400, source: 'user-dsh' },
      { path: join(this.agentsHome, 'skills'),  rank: 500, source: 'user-agents' },
    )
  }
  if (this.bundledSkillDir !== undefined) {
    roots.push({ path: this.bundledSkillDir, rank: 600, source: 'bundled' })
  }
}
```

**rank 数字越小 = 优先级越高**：project-dsh (100) > user-dsh (400) > bundled (600) — bundled 永远兜底。

### 4.6 Chokidar 文件监视（`skill-filesystem/src/index.ts:284-597`）

`SkillWatchManager` 类：
- **每根目录独立 watcher**（`chokidar.watch({ depth: 1 })`）
- **最大 128 个 project root**（LRU 淘汰，`watchMaxProjects`）
- **缺根时降级到 ancestor `fs.watchFile` 轮询**（200ms 稳定阈值）
- **bounded invalidation**（microtask 批合并，`queueInvalidation()`）
- **per-provider signal 隔离**：`control.signal.addEventListener('abort', dispose, { once: true })`

```typescript
// skill-filesystem/src/index.ts:579-588
private queueInvalidation(): void {
  if (this.closing || this.invalidationQueued) return
  this.invalidationQueued = true
  queueMicrotask(() => {
    this.invalidationQueued = false
    if (this.closing) return
    this.invalidate()
  })
}
```

### 4.7 revision + LRU 缓存（`skill/src/index.ts:520-660`）

```typescript
private async collect(options) {
  const revision = this.revision
  const key = collectCacheKey({ cwd, scopeChain, revision })  // JSON.stringify
  
  // 缓存命中 → 直接返回
  const cached = this.collectCache.get(key)
  if (cached !== undefined) return { entries: cached, cacheable: true }
  
  // 缓存未命中 → collectFresh
  const result = await this.collectFresh(options)
  
  // 并发 invalidation 期间，最多重试 2 次
  if (revision !== this.revision && attempt < MAX_COLLECT_ATTEMPTS) {
    attempt += 1
    continue
  }
  // 缓存有上限 128（collectCacheMaxEntries），超过按插入顺序淘汰
}
```

### 4.8 catalog 持久化为 user message（`tool-skill/src/index.ts:254-389`）

**创新点**：catalog 不是塞在 system prompt 字符串里，而是作为 **`user`-role 的 `<system-reminder>` message**，带持久化 source：

```typescript
function renderCatalogMessage(entries): UserMessage {
  return createUserMessage({
    content: [{
      type: 'text',
      text: [
        '<system-reminder>',
        'A skill is a reusable set of task-specific instructions. ...',
        '',
        '<available_skills>',
        ...renderCatalogEntries(entries),  // - `name`: desc
        '</available_skills>',
        '',
        'If the user names a skill, or the task clearly matches a skill\'s description, '
        + 'call the `skill` tool with the exact skill name before taking task actions. ...',
        '</system-reminder>',
      ].join('\n'),
    }],
    source: {
      kind: 'skill-catalog',
      form: 'catalog',
      entries,  // ← 完整条目存进 session 历史
    },
  })
}
```

**Catalog identity = SHA-256(canonical JSON entries)**（`tool-skill/src/index.ts:328-335`）：
```typescript
function digestCatalogEntries(entries): string {
  const canonical = entries.map(e => JSON.stringify([e.name, e.description])).join('\n')
  return createHash('sha256').update(canonical).digest('hex')
}
```

digest 相同时**不重新发布**；digest 变化时发**完整替换**（非 diff）。空 catalog 时不写入。

### 4.9 `skill` loader tool（`tool-skill/src/index.ts:81-160`）

```typescript
const skillTool = defineTool({
  name: 'skill',
  description: 'Load the full instructions for an available skill. Call this with the exact '
             + 'skill name from the session skill catalog before acting on a task that names '
             + 'or clearly matches that skill.',
  parameters: {
    name: { type: 'string', required: true, description: 'The exact skill name from the available skills list.' },
  },
  output: {
    schema: {
      type: 'object',
      properties: {
        name, provider, resourceBase, content,
      },
    },
    render: (_args, value) => [{ type: 'text', text: renderSkillContent(value) }],
  },
  async execute(args, exec) {
    if (!isSkillName(args.name)) throw new Error(`invalid skill name "${args.name}"`)
    const lookup = { cwd: exec.agent?.session.header.cwd, signal: exec.signal, scope: exec.agent }
    const summary = (await ctx.skills.list(lookup)).find(s => s.name === args.name)
    if (!summary) throw new Error(`skill "${args.name}" is unknown or no longer available`)
    if (!isModelInvocable(summary)) throw new Error(`skill "${args.name}" is not available for model invocation`)
    const skill = await ctx.skills.get(args.name, lookup)
    // ... 二次校验 modelInvocable
    return { name, provider, resourceBase, content }
  },
})
```

### 4.10 /name 用户手势（`tool-skill/src/index.ts:177-204`）

```typescript
const SKILL_GESTURE = /(^|\s)\/([a-z0-9]+(?:-[a-z0-9]+)*)(?=\s|$)/g

// 在 agent/pre-step listener 里:
// 1) 只扫描 source.kind === 'user' 的消息（外部文本无法伪造手势）
// 2) 提取所有匹配 token，dedup
// 3) ctx.skills.get() 加载
// 4) 校验 isUserInvocable（仅 user-invocable skill 可经手势注入）
// 5) 渲染成 instructions-form user message，append 到决策消息末尾
ctx.on('agent/pre-step', async ({ agent, messages, signal }, next) => {
  const decision = await next()
  const names = invokedSkillNames(messages)  // 只扫 user 源
  // ... 加载 + 校验 + 注入
})
```

**这是 `disable-model-invocation` skill 唯一的注入路径**（`tool-skill/src/index.ts:175-177` 注释）。

### 4.11 renderSkillContent 统一渲染（`skill/src/index.ts:171-216`）

```typescript
export function renderSkillContent(skill): string {
  return [
    `<skill_content name="${escapeAttr(skill.name)}">`,
    '<skill_resources>',
    ...renderResourceHint(skill),  // directory / url / opaque 三态
    '</skill_resources>',
    '',
    '<skill_instructions>',
    skill.content,
    '</skill_instructions>',
    '</skill_content>',
  ].join('\n')
}
```

`tool` 路径 和 `/name` 路径 **共用同一个 renderer**，确保模型看到的 XML 形状一致。

### 4.12 Bundled dsh-badge（`skill-badge/src/index.ts`）

唯一内置：
- **Markdown snippets for Shields.io URL**：`https://img.shields.io/badge/powered_by-dsh-4D6BFE`
- **Packaged PNG asset**（726×120，渲染 121×20）
- **rank = 600**（bundled 兜底）
- 完整 60 行实现

### 4.13 关键设计要点

- **Registry + Provider 模式**：天然支持未来加 RPC provider / remote registry
- **rank 数字排序 vs 字符串排序**：明确优先级
- **revision + LRU + retry**：缓存设计教科书级
- **catalog 持久化为 user message**：replay 友好、可见
- **`/name` 手势白名单**：仅 `source.kind === 'user'` 消息扫描，**外部文本无法伪造**
- **durable SHA-256 digest**：catalog identity 与渲染文字解耦

---

## 5. openclaw 的 Skill 系统（Workshop 自演化 + 52 extensions）

### 5.1 仓库定位

openclaw 是 **Skill 系统最复杂的项目** —— 7 个子模块（config / discovery / library / lifecycle / loading / runtime / security）+ Workshop（自演化）：

```
src/skills/
├── types.ts              (156 lines)
├── frontmatter.ts        ── parseSkillFrontmatter + resolveSkillInvocationPolicy
├── config/               ── mutations / agent-filter / bins / chat-commands / status
├── discovery/            ── discovery / plugin-skills / skill-root-discovery
├── library/              ── author / bundle / import / selection / service / store
├── lifecycle/            ── config / refresh / remote-skills / session-snapshot / tool-dispatch
├── loading/              ── frontmatter / local-loader / skill-contract / workspace-*
├── runtime/              ── embedded-run-entries / env-overrides / refresh
├── security/             ── clawhub-verdicts / scan-evidence / scanner / workspace-audit
├── test-support/
└── workshop/             ── 80+ 文件，自演化核心
    ├── service.ts            ── service 主入口
    ├── service-propose.ts    ── 创建 / 更新 proposal
    ├── service-evaluation.ts ── 评估
    ├── service-query.ts      ── 查询
    ├── apply-transition.ts   ── 应用过渡
    ├── store.ts              ── SQLite 存储
    ├── store-sqlite-*        ── 8 个 SQLite 子文件
    ├── config.ts             ── workshop 配置
    ├── policy.ts             ── 策略
    ├── curator.ts            ── 策展
    ├── history-scan.ts       ── 历史扫描
    ├── experience-review.ts  ── 经验评估
    └── ... 60+ 文件
```

### 5.2 内置 Skill 数量：52+ extensions

```
skills/                                  (52 个用户向 skill)
├── 1password, apple-notes, apple-reminders, bear-notes, blogwatcher,
│   blucli, camsnap, clawhub, coding-agent, control-ui, diagram-maker,
│   eightctl, gemini, gh-issues, gifgrep, github, gog, goplaces,
│   healthcheck, himalaya, mcporter, meme-maker, model-usage, nano-pdf,
│   node-connect, node-inspect-debugger, notion, obsidian, openai-whisper,
│   openai-whisper-api, openhue, oracle, ordercli, peekaboo, pyproject.toml,
│   python-debugpy, sag, sherpa-onnx-tts, skill-creator, songsee, sonoscli,
│   spike, spotify-player, summarize, taskflow, taskflow-inbox-triage,
│   things-mac, tmux, trello, video-frames, weather, xurl

.agents/skills/                          (custodian skills，~50 个)
├── agent-transcript, auto-qa, autoreview, channel-message-flows, clawdtributor,
│   claw-score, clawsweeper, control-ui-e2e, crabbox, deslop, discord-clawd,
│   discord-user-post, discrawl, gitcrawl, graincrawl, notcrawl,
│   openclaw-changelog-update, openclaw-ci-limits, openclaw-debugging,
│   openclaw-docker-e2e-authoring, openclaw-ghsa-maintainer, openclaw-live-updater,
│   openclaw-parallels-smoke, openclaw-pr-maintainer, openclaw-qa-testing,
│   openclaw-refactor-docs, openclaw-release-validation, openclaw-repair-sweep,
│   openclaw-secret-scanning-maintainer, openclaw-test-heap-leaks, openclaw-testing,
│   openclaw-test-performance, openclaw-update, parallels-discord-roundtrip,
│   prototype-openclaw-tui, release-openclaw-announcement, release-openclaw-ci,
│   release-openclaw-mac, release-openclaw-release-maintainer, release-openclaw-nightly,
│   release-openclaw-plugin-testing, security-triage, slacrawl, tag-duplicate-prs-issues,
│   technical-documentation, telegram-e2e-userbot, test-audit, update-team-server,
│   verify-release
```

### 5.3 Skill frontmatter 字段（`skills/coding-agent/SKILL.md` 真实示例）

```yaml
---
name: coding-agent
description: "Delegate coding work to Codex, Claude Code, or OpenCode as background workers; not simple edits or read-only code lookup."
metadata:
  {
    "openclaw":
      {
        "emoji": "🧩",
        "requires": {
          "anyBins": ["claude", "codex", "opencode"],
          "config": ["skills.entries.coding-agent.enabled"]
        },
        "install": [
          { "id": "node-claude", "kind": "node", "package": "@anthropic-ai/claude-code", "bins": ["claude"], "label": "Install Claude Code CLI (npm)" },
          { "id": "node-codex", "kind": "node", "package": "@openai/codex", "bins": ["codex"], "label": "Install Codex CLI (npm)" }
        ]
      }
  }
---
```

OpenClaw 特有字段：
- `metadata.openclaw.emoji` —— TUI 显示
- `metadata.openclaw.requires.bins/anyBins/env/config` —— 依赖检查
- `metadata.openclaw.install[]` —— 自助安装（brew/node/go/uv/download）
- `metadata.openclaw.os` —— 平台限制（如 peekaboo 仅 macOS）
- `metadata.openclaw.homepage`、`license` —— 展示元数据
- `user-invocable` / `disable-model-invocation` / `command-dispatch` / `command-tool` / `command-arg-mode`

### 5.4 Workshop 自演化（最创新）—— 80+ 文件

```
workshop/
├── propose 阶段：service-propose.ts (composeSkillBodyPatch, proposeCreateSkill, proposeUpdateSkill)
├── draft  阶段：proposal-draft.ts (nextProposalVersion, prepareSkillProposalDraft)
├── policy 阶段：policy.ts (策略机)
├── store  阶段：store.ts + 8 个 store-sqlite-*.ts (SQLite 持久化)
├── eval   阶段：service-evaluation.ts (evaluateSkillProposal)
├── review 阶段：experience-review.ts + history-scan.ts + curator.ts
├── apply  阶段：apply-transition.ts + reconcile-transition.ts
└── tools  阶段：skill-workshop-tool.ts (LLM 调用入口)
```

核心 idea（`workshop/service.ts:74-100`）：

```typescript
// reviseSkillProposal — 一个提案可经历多次修订
export async function reviseSkillProposal(input: SkillProposalReviseInput) {
  const config = resolveSkillWorkshopConfig(input.config)
  const revision = withPendingSkillProposalRevision(input, async (read) => {
    const { record } = read
    assertInsideWorkspace(input.workspaceDir, record.target.skillFile)
    // ...
  })
}
```

**完整生命周期**：
1. **propose** —— `skill_workshop` tool 被 LLM 调用，提交 proposal
2. **draft** —— 写入 SQLite `proposal_drafts` 表
3. **policy** —— 策略机评估（agent-filter / bin 依赖 / OS 限制）
4. **evaluate** —— 多维度评估（前后差异、风险评分）
5. **review** —— 经验评估（experience-review）+ 历史扫描（history-scan）+ 策展（curator）
6. **apply** —— 安全 apply 到 workspace skill（带 backup + rollback）
7. **lifecycle hooks** —— plugin-hooks.ts 派发 `proposal-changed` 事件

### 5.5 skill-creator skill（与 claudecode /skillify 同源）

`skills/skill-creator/SKILL.md`：

```markdown
# Skill Creator
## Workflow
1. Establish the contract.
2. Choose invocation.
3. Structure the skill.
4. Draft and persist.
5. Validate.  ←── python {baseDir}/scripts/quick_validate.py
```

`skills/skill-creator/scripts/quick_validate.py` —— 内置 Python validator：

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

### 5.6 SkillInstallSpec（自动安装）

`types.ts:4-19`：

```typescript
export type SkillInstallSpec = {
  id?: string
  kind: "brew" | "node" | "go" | "uv" | "download"
  label?: string
  bins?: string[]         // 依赖的二进制
  os?: string[]           // 平台限制
  formula?: string        // brew
  package?: string        // npm
  module?: string         // go module
  url?: string
  sha256?: string
  archive?: string
  extract?: boolean
  stripComponents?: number
  targetDir?: string
}
```

**自动安装**：当 `requires.bins` 在环境不存在时，调用 `install[]` 列表（优先 `preferBrew`）。

### 5.7 ClawHub 远程分发（`src/infra/clawhub-skills.ts`）

```typescript
export const CLAWHUB_SKILLS_SH_TRUST_STATE = "not-scanned-by-clawhub" as const
export const CLAWHUB_SKILLS_SH_TRUST_LABEL = "Not scanned by ClawHub" as const
export const CLAWHUB_SKILLS_SH_REF_PREFIX = "skills-sh:" as const

export type ClawHubSkillSearchResult = {
  score: number
  slug: string
  installRef: string         // 远程 install 必须返回
  installOnly?: true         // 仅安装无详情
  trustState?: "not-scanned-by-clawhub"
  displayName: string
  summary?: string
  version?: string
  updatedAt?: number
}

// 4 种 install kind
const CLAWHUB_SUPPORTED_INSTALL_KINDS = new Set(["clawhub", "github", "skills-sh"])
```

ClawHub API 完整搜索 / 安装 / 安全审计 / 多源解析（clawhub / github / skills-sh 三源）。

### 5.8 安全扫描（`src/skills/security/`）

```
security/
├── clawhub-verdicts.ts    ── ClawHub 安全决策
├── scan-evidence.ts       ── 扫描证据收集
├── scanner.ts             ── 主扫描器
└── workspace-audit.ts     ── workspace skill 审计
```

### 5.9 Node 宿主 Skill（`src/node-host/skills.ts`）

允许从连接的网络节点（其他设备）拉取 skill：

```typescript
export function scanNodeHostedSkills(options): NodeSkillDescriptor[] {
  const skillsDir = path.resolve(options.skillsDir ?? path.join(resolveConfigDir(), "skills"))
  const candidates = listCandidateSkillFiles(skillsDir, warn)  // 只看子目录的 SKILL.md
  const loadedSkills = loadSkillsFromDirSafe(...)
  // 过滤：frontmatter.name 必须 = 目录名 = basename(baseDir)
  // 限制：MAX_COUNT / MAX_TOTAL_BYTES / MAX_DESCRIPTION_LENGTH / MAX_CONTENT_BYTES
}
```

**大小硬限制**（`src/shared/node-skill-constraints.ts`）：
- `NODE_SKILL_MAX_COUNT` —— skill 总数上限
- `NODE_SKILL_MAX_TOTAL_BYTES` —— 全部 body 总字节
- `NODE_SKILL_MAX_DESCRIPTION_LENGTH` —— 单条描述长度
- `NODE_SKILL_MAX_CONTENT_BYTES` —— 单条 body 长度

### 5.10 关键设计要点

- **Workshop 自演化**：proposal/draft/policy/eval/review/apply 六阶段
- **skill-creator 内嵌 + 自我 validator**：良性循环
- **52 + 50 = 102+ skills**：远超其它项目
- **ClawHub 远程分发**：6 种 install 来源、信任状态（not-scanned-by-clawhub）
- **自动安装**：brew/node/go/uv/download + bin 依赖检查
- **节点宿 skill**：跨设备 skill 加载 + 硬字节上限

---

## 6. opencode 的 Skill 系统（远程拉取 + Effect 依赖注入）

### 6.1 仓库定位

```
opencode/
├── packages/core/src/skill.ts       (133 lines)  ── SkillV2 (Effect-based)
├── packages/core/src/skill/discovery.ts (137 lines) ── 远程 SkillDiscovery
├── packages/core/src/tool/skill.ts  (109 lines)  ── skill tool
├── packages/schema/src/skill.ts     (56 lines)   ── Schema 定义
├── packages/opencode/src/skill/discovery.ts (213 lines) ── 增强版（含路径安全）
├── packages/opencode/src/skill/index.ts (354 lines) ── 整合（含 builtin + 外部）
├── packages/opencode/src/skill/guidance.ts ── system prompt 引导
└── packages/opencode/src/tool/skill.txt (5 lines)  ── tool description
```

### 6.2 双版本并存（v1 / v2）

```typescript
// packages/core/src/skill.ts:1
export * as SkillV2 from "./skill"
```

v1 vs v2：
- v1: `packages/opencode/src/skill/*`（更传统，全局缓存 + 内置 1 个）
- v2: `packages/core/src/skill.ts`（Effect-based，依赖注入）

### 6.3 3 种 Source 类型（`schema/src/skill.ts:7-55`）

```typescript
export const DirectorySource = Schema.Struct({
    type: Schema.Literal("directory"),
    path: AbsolutePath,
})

export const UrlSource = Schema.Struct({
    type: Schema.Literal("url"),
    url: Schema.String,
})

export const EmbeddedSource = Schema.Struct({
    type: Schema.Literal("embedded"),
    skill: Schema.suspend(() => Info),  // 内嵌（如 builtin）
})

export type Source = DirectorySource | UrlSource | EmbeddedSource
```

三种 source 通过 `Source.key()` 映射到 cache key：
```typescript
key: (source: Source) =>
  source.type === "directory" ? `directory:${source.path}`
  : source.type === "url" ? `url:${source.url}`
  : `embedded:${source.skill.name}`,
```

### 6.4 Skill 信息（`schema/src/skill.ts:19-26`）

```typescript
export const Info = Schema.Struct({
  name: Schema.String,
  description: Schema.String.pipe(optional),
  slash: Schema.Boolean.pipe(optional),     // 斜杠命令触发
  location: AbsolutePath,
  content: Schema.String,
})
```

### 6.5 远程拉取（`opencode/src/skill/discovery.ts`）

```typescript
class IndexSkill {
  name: string
  files: Schema.Array(Schema.String)
  version: Schema.optional(Schema.String)    // ← 版本管理
}

class Index {
  skills: Schema.Array(IndexSkill)
}

// 拉取流程：
// 1) GET {url}/index.json → Index
// 2) 对每个文件 GET {url}/{skill}/{file} → 写入 cache
// 3) 若 version 变化 → staging → backup → rename 原子切换
// 4) cache: $Global.Path.cache/skills/{source_hash}/
```

**关键创新：版本化原子更新**（`opencode/src/skill/discovery.ts:165-200`）：

```typescript
const token = crypto.randomUUID()
const staging = `${root}.tmp-${token}`
const backup = `${root}.old-${token}`

yield* Effect.gen(function* () {
  const downloaded = yield* Effect.forEach(files, ...)
  if (!downloaded.every(Boolean)) return
  
  // 写入新版本号
  yield* fs.writeFileString(path.join(staging, ".opencode-version"), version)
  
  yield* Effect.uninterruptible(
    Effect.gen(function* () {
      const cached = yield* fs.exists(root).pipe(Effect.orDie)
      if (cached) yield* fs.rename(root, backup)        // 旧版 backup
      yield* fs.rename(staging, root)                   // 新版生效
      if (cached) yield* fs.remove(backup, ...)         // 清理 backup
    }),
  )
})
```

### 6.6 路径安全（`opencode/src/skill/discovery.ts:15-53`）

```typescript
function isSafeSegment(value: string) {
  return (
    value.length > 0 && value !== "." && value !== ".." &&
    !value.includes("/") && !value.includes("\\") && !value.includes("\0")
  )
}

function isSafeRelativePath(value: string) {
  const segments = value.split("/")
  // 无 \0、无 ?/#、非 URL、非绝对路径
  // 每段解码后不含 . / .. / \0
}

// 使用时：
if (!isSafeSegment(skill.name)) return []
if (!FSUtil.contains(sourceRoot, root) || root === sourceRoot) return []
if (resource.origin !== source.origin) return undefined  // 同源限制
```

### 6.7 Skill 加载管线（`opencode/src/skill/index.ts:173-246`）

```typescript
const discoverSkills = Effect.fn(function* (config, discovery, fsys, global,
                                              disableExternalSkills,
                                              disableClaudeCodeSkills,
                                              directory, worktree) {
  const state: ScanState = { matches: new Set(), dirs: new Set() }

  // 1) 外部 skills: ~/.claude/skills, ~/.agents/skills
  //    + 向上扫描 project dirs (cwd 到 worktree 边界)
  if (!disableExternalSkills) {
    if (!disableClaudeCodeSkills) externalDirs.push(CLAUDE_EXTERNAL_DIR)
    externalDirs.push(AGENTS_EXTERNAL_DIR)
    for (const dir of externalDirs) {
      const root = path.join(global.home, dir)
      if (!(yield* fsys.isDir(root))) continue
      yield* scan(state, root, "skills/**/SKILL.md", { dot: true, scope: "global" })
    }
  }

  // 2) config.skills.paths (本地路径)
  for (const dir of configDirs) {
    yield* scan(state, dir, "{skill,skills}/**/SKILL.md")
  }

  // 3) config.skills.paths (显式 path)
  for (const item of cfg.skills?.paths ?? []) {
    yield* scan(state, dir, "**/SKILL.md")
  }

  // 4) config.skills.urls (远程拉取)
  for (const url of cfg.skills?.urls ?? []) {
    const pulledDirs = yield* discovery.pull(url)
    for (const dir of pulledDirs) yield* scan(state, dir, "**/SKILL.md")
  }
})
```

**4 源：external dirs + config dirs + paths + urls** —— 全 Effect-based，全错误隔离。

### 6.8 内置 Skill：customize-opencode

`opencode/src/skill/index.ts:32-35`：

```typescript
const CUSTOMIZE_OPENCODE_SKILL_NAME = "customize-opencode"
const CUSTOMIZE_OPENCODE_SKILL_DESCRIPTION =
  "Use ONLY when the user is editing or creating opencode's own configuration: opencode.json, "
  + "opencode.jsonc, files under .opencode/, or files under ~/.config/opencode/. Also use when "
  + "creating or fixing opencode agents, subagents, skills, plugins, MCP servers, or permission rules. "
  + "Do not use for the user's own application code, or for any project that is not configuring opencode itself."

// 注册顺序：disk discovery 之前注册（disk 同名 skill 可覆盖 builtin）
s.skills[CUSTOMIZE_OPENCODE_SKILL_NAME] = { ... }
```

只有一个 builtin skill，专门用于「改 opencode 配置」时给模型喂真实 schema（避免它猜错）。

### 6.9 Permission 过滤（`opencode/src/skill/index.ts:310-315`）

```typescript
const available = Effect.fn("Skill.available")(function* (agent?: Agent.Info) {
  const s = yield* InstanceState.get(state)
  const list = Object.values(s.skills).toSorted((a, b) => a.name.localeCompare(b.name))
  if (!agent) return list
  return list.filter((skill) =>
    Permission.evaluate("skill", skill.name, agent.permission).action !== "deny"
  )
})
```

**Skill 可被 agent-level permission 显式 deny**。

### 6.10 skill tool 输出（`core/src/tool/skill.ts:35-52`）

```typescript
export const toModelOutput = (skill: SkillV2.Info, files: ReadonlyArray<string>) => {
  const directory = path.dirname(skill.location)
  return [
    `<skill_content name="${skill.name}">`,
    `# Skill: ${skill.name}`,
    '',
    skill.content.trim(),
    '',
    `Base directory for this skill: ${directory}`,
    "Relative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.",
    "Note: file list is sampled.",
    '',
    '<skill_files>',
    ...files.map((file) => `<file>${file}</file>`),
    '</skill_files>',
    '</skill_content>',
  ].join('\n')
}
```

对目录式 skill 还会枚举目录文件（最多 10 个，`FILE_LIMIT = 10`）。

### 6.11 Prompt 格式（`opencode/src/skill/index.ts:321-346`）

```typescript
export function fmt(list: Info[], opts: { verbose: boolean }) {
  const described = list.filter((skill) => skill.description !== undefined)
  if (described.length === 0) return "No skills are currently available."

  if (opts.verbose) {
    return [
      "<available_skills>",
      ...described.flatMap((skill) => [
        "  <skill>",
        `    <name>${skill.name}</name>`,
        `    <description>${skill.description}</description>`,
        `    <location>${escapeHtml(skill.location)}</location>`,
        "  </skill>",
      ]),
      "</available_skills>",
    ].join("\n")
  }

  return [
    "## Available Skills",
    ...described.map((skill) => `- **${skill.name}**: ${skill.description}`),
  ].join("\n")
}
```

### 6.12 关键设计要点

- **Effect 全栈**：错误隔离 + retry + observability 天然集成
- **3 种 Source 类型**：Directory / Url / Embedded（schema discriminated union）
- **远程拉取带 atomic 切换**：staging → backup → rename，失败回滚
- **路径安全检查**：isSafeSegment + isSafeRelativePath + 同源限制
- **1 个 builtin skill**：customize-opencode（教模型改 opencode 配置）
- **4 源发现**：external + config + paths + urls
- **agent permission 过滤**：skill 可被 deny

---

## 7. pi 的 Skill 系统（Agent Skills 标准 + 一等公民）

### 7.1 仓库定位

```
pi/
├── packages/coding-agent/src/core/skills.ts    (508 lines) ── loadSkills() + formatSkillsForPrompt()
├── packages/coding-agent/docs/skills.md        (-- 300 lines) ── 完整使用文档
├── packages/agent/src/harness/skills.ts        (500+ lines) ── harness 层包装
├── packages/coding-agent/examples/sdk/04-skills.ts ── SDK 用法示例
├── packages/coding-agent/test/skills.test.ts
├── packages/coding-agent/test/sdk-skills.test.ts
├── packages/coding-agent/test/fixtures/skills/ ── 14 个 fixture skill
└── packages/coding-agent/test/fixtures/skills/valid-skill/SKILL.md
```

### 7.2 严格遵循 Agent Skills 标准

`docs/skills.md`：

> Pi implements the Agent Skills standard, warning about most violations but remaining lenient.

**官方规范**: https://agentskills.io/specification

pi 是六个项目中**最严格遵守 Agent Skills 标准**的（与 opencode 并列）。差异：

- 标准要求 `name == parent_dir_name`，pi **允许不一致**（`docs/skills.md` 注释解释："that rule is suboptimal for shared skill directories used across multiple agent harnesses"）

### 7.3 frontmatter 验证（`core/skills.ts:301-321`）

```typescript
const MAX_NAME_LENGTH = 64
const MAX_DESCRIPTION_LENGTH = 1024

function validateName(name: string, parentDirName: string): string[] {
  if (name !== parentDirName) errors.push(`name "${name}" does not match parent directory "${parentDirName}"`)
  if (name.length > MAX_NAME_LENGTH) errors.push(`name exceeds ${MAX_NAME_LENGTH} characters`)
  if (!/^[a-z0-9-]+$/.test(name)) errors.push("name contains invalid characters")
  if (name.startsWith("-") || name.endsWith("-")) errors.push("name must not start or end with a hyphen")
  if (name.includes("--")) errors.push("name must not contain consecutive hyphens")
  return errors
}

function validateDescription(description: string | undefined): string[] {
  if (!description || description.trim() === "") errors.push("description is required")
  else if (description.length > MAX_DESCRIPTION_LENGTH) errors.push(`description exceeds ${MAX_DESCRIPTION_LENGTH} characters`)
  return errors
}
```

### 7.4 多源发现（`core/skills.ts:407-507`）

```typescript
export function loadSkills(options: LoadSkillsOptions): LoadSkillsResult {
  const { agentDir, skillPaths, includeDefaults } = options
  // ...
  if (includeDefaults) {
    addSkills(loadSkillsFromDirInternal(join(agentDir, "skills"), "user", true))
    addSkills(loadSkillsFromDirInternal(resolve(cwd, CONFIG_DIR_NAME, "skills"), "project", true))
  }

  for (const rawPath of skillPaths) {
    const resolvedPath = resolvePath(rawPath, cwd, { trim: true })
    if (stats.isDirectory()) addSkills(loadSkillsFromDirInternal(resolvedPath, source, true))
    else if (stats.isFile() && resolvedPath.endsWith(".md")) {
      addSkills({ skills: [loadSkillFromFile(resolvedPath, source).skill], ... })
    }
  }
  return { skills: Array.from(skillMap.values()), diagnostics }
}
```

### 7.5 4 优先级 source 标识（`core/skills.ts:467-473`）

```typescript
const getSource = (resolvedPath: string): "user" | "project" | "path" => {
  if (!includeDefaults) {
    if (isUnderPath(resolvedPath, userSkillsDir)) return "user"
    if (isUnderPath(resolvedPath, projectSkillsDir)) return "project"
  }
  return "path"   // 显式 --skill 路径
}
```

### 7.6 冲突处理 + realpath 去重（`core/skills.ts:419-447`）

```typescript
function addSkills(result) {
  for (const skill of result.skills) {
    const realPath = canonicalizePath(skill.filePath)  // 解析软链
    
    if (realPathSet.has(realPath)) continue  // 同文件已加
    
    const existing = skillMap.get(skill.name)
    if (existing) {
      // 命名冲突 → 收集到 collisionDiagnostics（不静默丢弃）
      collisionDiagnostics.push({
        type: "collision", message: `name "${skill.name}" collision`,
        path: skill.filePath,
        collision: { resourceType: "skill", name: skill.name, winnerPath: existing.filePath, loserPath: skill.filePath }
      })
    } else {
      skillMap.set(skill.name, skill)
      realPathSet.add(realPath)
    }
  }
}
```

### 7.7 2 source 注入格式（`core/skills.ts:355-381`）

```typescript
export function formatSkillsForPrompt(skills: Skill[]): string {
  const visibleSkills = skills.filter((s) => !s.disableModelInvocation)
  if (visibleSkills.length === 0) return ""

  const lines = [
    "\n\nThe following skills provide specialized instructions for specific tasks.",
    "Use the read tool to load a skill's file when the task matches its description.",
    "When a skill file references a relative path, resolve it against the skill directory "
    + "(parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.",
    "",
    "<available_skills>",
  ]

  for (const skill of visibleSkills) {
    lines.push("  <skill>")
    lines.push(`    <name>${escapeXml(skill.name)}</name>`)
    lines.push(`    <description>${escapeXml(skill.description)}</description>`)
    lines.push(`    <location>${escapeXml(skill.filePath)}</location>`)
    lines.push("  </skill>")
  }
  lines.push("</available_skills>")
  return lines.join("\n")
}
```

### 7.8 格式化为 user message（`harness/skills.ts`）

```
<skill name="..." location="...">
References are relative to ....

<skill content>
</skill>
```

### 7.9 6 源加载位置（`docs/skills.md`）

```
Global:
  ~/.pi/agent/skills/
  ~/.agents/skills/
Project (after trust):
  .pi/skills/
  .agents/skills/ (cwd + 向上到 git root)
Packages: skills/ dir 或 package.json pi.skills
Settings: skills array (files or dirs)
CLI: --skill <path> (repeatable, --no-skills 仍生效)
```

**`--no-skills`**：禁用自动发现，但显式 `--skill` 路径仍生效（与 claudecode `--bare` 类似）。

### 7.10 gitignore 风格过滤（`core/skills.ts:178-242`）

```typescript
const IGNORE_FILE_NAMES = [".gitignore", ".ignore", ".fdignore"]

async function addIgnoreRules(env, dir, rootDir, diagnostics) {
  for (const filename of IGNORE_FILE_NAMES) {
    const ignorePath = await env.joinPath([dir, filename])
    if (!ignorePathResult.ok) continue
    // 读取 + 解析 + 嵌套前缀
    const patterns = content.value.split(/\r?\n/)
      .map(line => prefixIgnorePattern(line, prefix))
      .filter((line): line is string => Boolean(line))
    if (patterns.length > 0) ig.add(patterns)
  }
}
```

支持 `.gitignore` / `.ignore` / `.fdignore` 三种忽略文件，路径前缀嵌套。

### 7.11 跨工具加载（`docs/skills.md`）

```json
{
  "skills": [
    "~/.claude/skills",
    "~/.codex/skills"
  ]
}
```

**pi 可直接使用 Claude Code / OpenAI Codex 的 skill 目录**！这是兼容性的胜利。

### 7.12 Skill 命令（`docs/skills.md`）

```
/skill:brave-search
/skill:pdf-tools extract
```

参数附加为 `User: <args>`。

### 7.13 SDK 用法（`examples/sdk/04-skills.ts`）

```typescript
import { createAgentSession, DefaultResourceLoader, getAgentDir, type Skill } from "@earendil-works/pi-coding-agent"

const customSkill: Skill = {
  name: "my-skill",
  description: "Custom project instructions",
  filePath: "/virtual/SKILL.md",
  baseDir: "/virtual",
  sourceInfo: createSyntheticSourceInfo("/virtual/SKILL.md", { source: "sdk" }),
  disableModelInvocation: false,
}

const loader = new DefaultResourceLoader({
  cwd: process.cwd(),
  agentDir: getAgentDir(),
  skillsOverride: (current) => {
    const filteredSkills = current.skills.filter((s) => s.name.includes("browser") || s.name.includes("search"))
    return { skills: [...filteredSkills, customSkill], diagnostics: current.diagnostics }
  }
})
```

**SDK 用户可完全替换 skill 列表**（`skillsOverride`），是 laew 借鉴的极佳范式。

### 7.14 关键设计要点

- **严格 Agent Skills 标准**：与 opencode 并列
- **6 源发现**：5 文件系统源 + CLI flag
- **诊断系统**：file_info_failed / list_failed / read_failed / parse_failed / invalid_metadata（5 类诊断）
- **命名冲突不静默**：收集到 collisionDiagnostics
- **gitignore 过滤**：3 种文件名支持 + 嵌套前缀
- **跨工具兼容**：直接读 `~/.claude/skills` 等

---

## 8. 综合对比维度表

### 8.1 文件格式与 frontmatter

| 项目 | 文件格式 | 必填字段 | 可选字段 | 命名规范 | 描述长度上限 |
|------|----------|----------|----------|----------|--------------|
| **atomcode** | `<name>/SKILL.md` 或 `<name>.md` | 无 | `name`, `description`, `allowed-tools`, `user-invocable` | 1-64 字符，alphanumeric + `-_/`，不以 `-/` 开头结尾 | 1024 字符 |
| **claudecode** | `<name>/SKILL.md`（强推）或 `<name>.md`（legacy commands） | 无 | `description`, `when_to_use`, `argument-hint`, `arguments`, `allowed-tools`, `model`, `disable-model-invocation`, `user-invocable`, `context`, `agent`, `effort`, `shell`, `paths`, `hooks`, `version` | `bun:bundle` feature flag 控制 builtin | 无显式限制 |
| **deepseek-harness** | `<name>/SKILL.md` 或 `<name>.md` | `name`, `description` | `whenToUse`, `metadata`, `disable-model-invocation`, `user-invocable` | kebab-case `^[a-z0-9]+(?:-[a-z0-9]+)*$` | 无显式限制（catalog 默认截断 500） |
| **openclaw** | `<name>/SKILL.md` | `name`, `description` | `metadata.openclaw.*`（emoji/bins/install/homepage/license）, `allowed-tools`, `user-invocable`, `disable-model-invocation`, `command-dispatch`, `command-tool`, `command-arg-mode` | `NODE_SKILL_NAME_RE`（具体未读） | `NODE_SKILL_MAX_DESCRIPTION_LENGTH` |
| **opencode** | `<name>/SKILL.md` 或 `<name>.md`（flat） | `name` | `description`, `slash` | 字符串 | 无显式限制 |
| **pi** | `<name>/SKILL.md`（强制目录式） | `name`, `description` | `disable-model-invocation`, 元数据（任意） | 严格 kebab-case `^[a-z0-9-]+$`，与目录名匹配（pi 放宽），1-64 字符 | 1024 字符 |

### 8.2 发现机制

| 项目 | 目录数 | 优先级策略 | 跨工具支持 | 文件监视 | 远程拉取 |
|------|--------|----------|----------|----------|----------|
| **atomcode** | 10（user+project × {commands,skills} × {claude,agents,atomcode}） | 后赢（list 后置 wins） | 直接读 `.claude/skills` `.agents/skills` | 无 | 无 |
| **claudecode** | 5+（managed/user/project/additional/legacy） + dynamic + conditional | 并行加载 + dedup by realpath | - | 仅动态发现（按需） | 仅 MCP server 提供 |
| **deepseek-harness** | 5（project-dsh/agents + custom + user-dsh/agents + bundled） | rank 数字（100/200/300/400/500/600） | `.dsh/skills` `.agents/skills` | **Chokidar 完整实现** | 仅注册 contract，无 shipped 实现 |
| **openclaw** | 6+（bundled/workspace/agent/customers/clawhub/remote） | workspace > bundled > remote | 直接读 `.claude/skills` 等 | **refresh watcher（`refresh-watch-path.ts`）** | **ClawHub 完整实现** |
| **opencode** | 4（external + config dirs + paths + urls） | disk 后赢 | `.claude/skills` `.agents/skills` | 无 | **远程 discovery + atomic 切换** |
| **pi** | 6（agent/agents + cwd/project + packages + settings + CLI） | 先加先赢 + collisionDiagnostics | `.claude/skills` `.codex/skills` | 无 | 无 |

### 8.3 触发方式

| 项目 | 用户 `/name` | 模型决策 | 关键词/自动 | 注释触发 | 条件路径 | 动态发现 |
|------|-------------|---------|-------------|---------|---------|---------|
| **atomcode** | `$skill`（slash） | `use_skill` tool | LLM | 无 | 无 | 无 |
| **claudecode** | `/name`（菜单） | `use_skill` tool | LLM | 无 | **paths frontmatter + gitignore** | **运行时按 file path 向上扫描** |
| **deepseek-harness** | `/name`（gesture） | `skill` tool | LLM | 无 | 无 | 无 |
| **openclaw** | `/name` | LLM | LLM | 无 | paths | **Workshop self-evolution** |
| **opencode** | `/name`（slash 属性） | `skill` tool | LLM | 无 | 无 | 无 |
| **pi** | `/skill:name` | LLM | LLM | 无 | 无 | 无 |

### 8.4 系统提示词注入

| 项目 | 注入位置 | 触发时机 | 格式 | 预算门控 | 源排序 |
|------|----------|----------|------|----------|--------|
| **atomcode** | system message（position-based） | session_start hook | `- name: desc` 列表 | **8KB 字节预算** | **4 级 tier** |
| **claudecode** | system prompt 内（`Command` 注册） | 启动 + 动态 | name + description + whenToUse | token estimation | managed > user > project |
| **deepseek-harness** | **user-role `<system-reminder>`** 持久化 message | pre-step listener | markdown bullets | **500 字符描述上限** | rank 数字 |
| **openclaw** | system prompt | session start | `<available_skills>` XML + location | `WORKSPACE_SKILLS_PROMPT_FORMAT_VERSION` | workspace > bundled |
| **opencode** | system prompt | config 时 | `<available_skills>` XML 或 markdown | 无显式 | name localeCompare |
| **pi** | system prompt 内 | reload 时 | `<available_skills>` XML（带 location） | 无显式 | 加载顺序 |

### 8.5 元数据

| 项目 | 标签 | 作者 | 版本 | 校验 | 远程元数据 |
|------|------|------|------|------|----------|
| **atomcode** | 无 | 无 | 无 | `validate_skill_name()` | 无 |
| **claudecode** | 无 | 无 | frontmatter `version` | hooks schema | 无 |
| **deepseek-harness** | 无 | 无 | 无 | `isSkillName()` 正则 | 无（provider opaque `metadata`） |
| **openclaw** | 无（`emoji` 是 UI 元数据） | 无 | implicit | quick_validate.py | **ClawHub trustState** |
| **opencode** | 无 | 无 | `Index.version` 原子切换 | Schema | remote index.json |
| **pi** | 无 | 无 | 无 | `validateName()` / `validateDescription()` | 无 |

### 8.6 远程分发与版本管理

| 项目 | 远程拉取 | 索引格式 | 缓存位置 | 版本切换 | 信任模型 |
|------|----------|----------|----------|----------|----------|
| **atomcode** | 无 | - | - | - | 完全本地 |
| **claudecode** | 仅 MCP | MCP server | - | - | MCP server trust |
| **deepseek-harness** | 仅 contract | - | - | - | provider 自管 |
| **openclaw** | **ClawHub 完整** | REST API | local skill dir | - | `not-scanned-by-clawhub` 状态 |
| **opencode** | **URL discovery** | `index.json` | `$cache/skills/{hash}/` | **staging→backup→rename atomic** | URL 同源 |
| **pi** | 无 | - | - | - | 完全本地 |

### 8.7 与 MCP / Tool / Workflow 关系

| 项目 | MCP | Tool | Workflow | Agent | 备注 |
|------|-----|------|----------|-------|------|
| **atomcode** | skill 直接挂 tool | `use_skill` + `list_skills` | 无 | 无 | `mcpSkillBuilders.ts`（仅 claudecode） |
| **claudecode** | MCP 提供 skill | `use_skill` | 通过 hooks | agent preset | Skill 可以挂 27 种 Hook |
| **deepseek-harness** | 无 | `skill` tool | 无 | agent preset scope | provider 模型 |
| **openclaw** | ClawHub skill 远程分发 | skill_workshop tool | workshop 自演化 | agent preset | Skill 6 种 install kind |
| **opencode** | 无 | `skill` tool | 无 | agent scope | permission 可 deny |
| **pi** | 无 | `read` tool 加载 body | 无 | harness 层 | SDK 可替换 skillsOverride |

### 8.8 Workshop / 自演化

| 项目 | 自演化机制 | 提案/草稿 | 评估 | 审计 | 应用 |
|------|----------|-----------|------|------|------|
| **atomcode** | 无 | - | - | - | - |
| **claudecode** | `/skillify` builtin skill | session memory + user messages | LLM | 无 | 直接生成 SKILL.md |
| **deepseek-harness** | 无 | - | - | - | - |
| **openclaw** | **Workshop 完整** | SQLite 持久化 | policy + experience-review + history-scan | workspace-audit + scanner | apply-transition |
| **opencode** | 无 | - | - | - | - |
| **pi** | 无 | - | - | - | - |

---

## 9. 六项目 Skill 文件格式示例

### 9.1 atomcode skill（`crates/atomcode-capabilities/assets/setup-seeds/skills/atomcode-automation-recommender/`）

```markdown
---
name: atomcode-automation-recommender
description: Recommend automations based on observed shell history patterns.
allowed-tools: read_file
user-invocable: true
---

# Automation Recommender
...

!`cat ~/.bash_history | tail -20`
```

### 9.2 claudecode builtin skill（`bundled/verify.ts`）

```typescript
export const SKILL_MD: string = skillMd
export const SKILL_FILES: Record<string, string> = {
  'examples/cli.md': cliMd,
  'examples/server.md': serverMd,
}
```

### 9.3 deepseek-harness bundled skill（`skill-badge/assets/dsh-badge.md`）

```markdown
# Powered by dsh

When creating pull requests, merge requests, or any attribution-bearing content
with DeepSeek Harness, add the official "powered by dsh" badge:

[![powered by dsh](https://img.shields.io/badge/powered_by-dsh-4D6BFE?style=flat)](https://github.com/deepseek-ai/deepseek-harness)

Or use the local asset at `dsh-badge.png` (rendered at 121×20).
```

### 9.4 openclaw skill（`skills/peekaboo/SKILL.md`）

```markdown
---
name: peekaboo
description: "Capture and automate macOS UI with the Peekaboo CLI."
homepage: https://peekaboo.boo
metadata:
  {
    "openclaw":
      {
        "emoji": "👀",
        "os": ["darwin"],
        "requires": { "bins": ["peekaboo"] },
        "install": [
          { "id": "brew", "kind": "brew", "formula": "steipete/tap/peekaboo", "bins": ["peekaboo"], "label": "Install Peekaboo (brew)" }
        ]
      }
  }
---

# Peekaboo
...
```

### 9.5 opencode builtin（`opencode/src/skill/index.ts`）

```typescript
const CUSTOMIZE_OPENCODE_SKILL_NAME = "customize-opencode"
const CUSTOMIZE_OPENCODE_SKILL_DESCRIPTION = "Use ONLY when the user is editing or creating opencode's own configuration: ..."
const CUSTOMIZE_OPENCODE_SKILL_BODY = SkillPlugin.CustomizeOpencodeContent
```

### 9.6 pi skill（`test/fixtures/skills/valid-skill/SKILL.md`）

```markdown
---
name: valid-skill
description: A valid skill for testing purposes.
---

# Valid Skill

This is a valid skill that follows the Agent Skills standard.
```

---

## 10. laew 当前 Skill 系统缺口与 P0 借鉴路线图

### 10.1 laew 现状盘点

```
CLAUDE.md: "内置 Bash / Read / Write 三个工具"
src/agent/tools/: 仅 bash.rs / read.rs / write.rs
src/agent/system_prompt/mod.rs: 无 Skill 注入
src/agent/mod.rs: run_session() 无 Skill loading
src/agent/yolo.rs: YoloRunner 无 Skill 决策
docs/TUI界面与CLI渲染引擎/: 无 Skill 屏
```

**完全空白**。

### 10.2 缺口矩阵

| 维度 | 当前 | 期望（最小 PoC） | 期望（完整版） |
|------|------|------------|---------------|
| 加载 | 无 | `.laew/skills/` 单目录 | 5 源 + 远程 |
| 触发 | 无 | `/skill:name`（TUI 补全）+ `use_skill` tool | + 自动匹配 + `/name` gesture |
| 注入 | 无 | system prompt 注入 catalog | + 持久化 message + 8KB 预算 |
| Tool 集成 | 无 | `use_skill` Tool | + `list_skills` Tool |
| 元数据 | 无 | `name` + `description` 必填 | + `allowed-tools` + `disable-model-invocation` |
| Workshop | 无 | 无 | skill-creator skill |
| 远程 | 无 | 无 | ClawHub-style 远程分发 |
| 审计 | 无 | 无 | quick_validate.py |

### 10.3 P0 路线图（4 周）

**Week 1: Skill 数据模型 + 文件加载器**
- 新增 `src/agent/skills/skill.rs`（仿 atomcode `skill.rs`）
- 新增 `src/agent/skills/registry.rs`（仿 atomcode `SkillRegistry`，BTreeMap）
- 新增 `src/agent/skills/loader.rs`（扫描 `.laew/skills/`）
- 在 `src/agent/profile.rs` 加 `Work Skill` 注册（不动现有 6 Agent）
- 在 `src/agent/mod.rs` 启动时加载

**Week 2: `use_skill` + `list_skills` tools**
- 新增 `src/agent/tools/use_skill.rs`（仿 atomcode）
- 新增 `src/agent/tools/list_skills.rs`
- 注册到 `builtin_registry()`
- 添加 Skill 变量替换（`$ARGUMENTS`, `${CLAUDE_SKILL_DIR}`, shell 注入）

**Week 3: system prompt 注入 + catalog hook**
- 在 `src/agent/system_prompt/mod.rs` 加 `render_skill_catalog()`
- 新增 `src/agent/skills/catalog_hook.rs`（仿 atomcode）
- 8KB 预算门控 + 源 tier 排序
- `description` ≤ 1024 字符硬截断

**Week 4: TUI 补全 + E2E**
- 在 `src/tui/completion.rs` 加 `/skill:` 补全
- 在 `testReport/run_e2e.sh` 加 skill 加载用例
- 在 `src/agent/mod.rs` 测试用例

### 10.4 P1（5-8 周）：自演化 + Workshop

**Week 5-6: skill-creator skill**
- 在 `src/agent/skills/bundled/skill_creator.rs` 实现「分析 session → 生成 SKILL.md」（仿 claudecode skillify）
- 在 `src/agent/tools/` 加 `skill_workshop` tool

**Week 7-8: SQLite 持久化 + 历史评估**
- 新增 `proposals` / `proposal_drafts` 表（仿 openclaw）
- 借鉴 openclaw `experience-review` / `history-scan` / `curator`
- apply-transition + rollback

### 10.5 P2（9-12 周）：远程拉取

**Week 9-10: HTTPS discovery**
- 仿 opencode `SkillDiscovery.pull()` 实现
- index.json Schema 校验
- 路径安全检查（isSafeSegment 等）

**Week 11-12: 版本化 atomic 切换**
- staging → backup → rename 三段式
- rollback on failure

### 10.6 最小借鉴优先级

| 借鉴 | 项目 | 价值 | 风险 |
|------|------|------|------|
| **数据模型**（`Skill` struct） | atomcode | 最简 | 低 |
| **BTreeMap registry** | atomcode | prompt prefix cache | 低 |
| **8KB 预算** | atomcode | 防信号稀释 | 低 |
| **6 源并行加载** | claudecode | 高吞吐 | 中（要 dedup） |
| **conditional paths** | claudecode | 高级 | 高 |
| **dynamic discovery** | claudecode | 高级 | 高（要 git check-ignore） |
| **Registry + Provider** | deepseek-harness | 可扩展 | 中 |
| **catalog user message** | deepseek-harness | replay 友好 | 中 |
| **SHA-256 digest** | deepseek-harness | idempotent | 低 |
| **/name gesture** | deepseek-harness | 用户友好 | 低 |
| **Workshop 完整** | openclaw | 自演化 | 极高 |
| **ClawHub** | openclaw | 远程 | 高 |
| **Effect-based** | opencode | 错误隔离 | 极高（laew 非 Effect） |
| **路径安全** | opencode | 必要 | 低 |
| **远程 atomic 切换** | opencode | 必要 | 中 |
| **Agent Skills 标准** | pi | 兼容性 | 低 |
| **gitignore 过滤** | pi | 必要 | 低 |
| **命名冲突 diagnostics** | pi | 不静默 | 低 |
| **SDK 替换** | pi | 灵活性 | 低 |

---

## 11. laew Skill 系统最小可行产品（MiniPoC）代码蓝图

### 11.1 `Skill` 数据模型（仿 atomcode）

```rust
// src/agent/skills/skill.rs

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub template: String,
    pub allowed_tools: Vec<String>,
    pub user_invocable: bool,
    pub skill_dir: PathBuf,
    pub source_path: PathBuf,
}

impl Skill {
    pub fn expand(&self, arguments: &str, session_id: &str) -> String {
        // 仿 atomcode skill.rs:32-59 单次左到右扫描
        let positional: Vec<&str> = arguments.split_whitespace().collect();
        let skill_dir = self.skill_dir.to_string_lossy();

        let t = self.template.as_str();
        let mut result = String::with_capacity(t.len());
        let mut i = 0;
        while i < t.len() {
            let rest = &t[i..];
            if let Some((value, len)) =
                match_substitution(rest, &positional, arguments, session_id, skill_dir.as_ref())
            {
                result.push_str(value);
                i += len;
            } else {
                let ch = rest.chars().next().unwrap();
                result.push(ch);
                i += ch.len_utf8();
            }
        }
        if !self.template.contains("$ARGUMENTS") && !arguments.trim().is_empty() {
            result = format!("{}\n\nARGUMENTS: {}", result.trim_end(), arguments);
        }
        expand_shell_injections(&result)
    }
}

struct Frontmatter {
    name: Option<String>,
    description: String,
    allowed_tools: Vec<String>,
    user_invocable: bool,
}

impl Default for Frontmatter {
    fn default() -> Self {
        Self { name: None, description: String::new(), allowed_tools: Vec::new(), user_invocable: true }
    }
}

fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
    let mut fm = Frontmatter::default();
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return (fm, content.to_string());
    }
    // ... (仿 atomcode skill.rs:223-283)
}

fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err(format!("skill name '{}' must be 1-64 characters", name));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("skill name '{}' has invalid characters", name));
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(format!("skill name '{}' has bad hyphen position", name));
    }
    Ok(())
}
```

### 11.2 `SkillRegistry`（仿 atomcode）

```rust
// src/agent/skills/registry.rs

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct SkillRegistry {
    skills: BTreeMap<String, Arc<Skill>>,
}

impl SkillRegistry {
    pub fn new() -> Self { Self { skills: BTreeMap::new() } }

    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut reg = Self::new();
        for dir in dirs { reg.load_dir(dir, None); }
        reg
    }

    pub fn load_dir(&mut self, dir: &Path, namespace: Option<&str>) {
        self.scan_skill_dir(dir, namespace, 0);
    }

    fn scan_skill_dir(&mut self, dir: &Path, namespace: Option<&str>, depth: usize) {
        const MAX_DEPTH: usize = 8;
        if depth > MAX_DEPTH { return; }
        let Ok(rd) = std::fs::read_dir(dir) else { return; };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_file() {
                if depth == 0 && p.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Ok(s) = parse_skill_file(&p, namespace) {
                        self.skills.insert(s.name.clone(), Arc::new(s));
                    }
                }
            } else if p.is_dir() {
                let skill_md = p.join("SKILL.md");
                if skill_md.is_file() {
                    if let Ok(s) = parse_skill_dir(&p, &skill_md, namespace) {
                        self.skills.insert(s.name.clone(), Arc::new(s));
                    }
                } else {
                    self.scan_skill_dir(&p, namespace, depth + 1);
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<Skill>> {
        self.skills.get(name).cloned()
    }
    pub fn list(&self) -> Vec<(String, String)> {
        self.skills.values().map(|s| (s.name.clone(), s.description.clone())).collect()
    }
}

/// laew 默认 skill 目录（参照 atomcode 简化）
pub fn standard_skill_dirs(home: &Path, project: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".laew/skills"),         // user-level
        project.join(".laew/skills"),      // project-level
        project.join(".agents/skills"),    // 跨 agent 共享
    ]
}
```

### 11.3 `render_skill_catalog`（仿 atomcode 8KB 预算）

```rust
// src/agent/skills/render.rs

pub const CATALOG_BYTE_BUDGET: usize = 8000;
pub const PER_SKILL_DESC_CAP: usize = 1024;
pub const CATALOG_HEADER: &str = "=== AVAILABLE SKILLS ===";

const GUIDANCE: &str = "Skills are reusable instruction templates for specific tasks. \
The names listed below are the only skill names you may pass directly to `use_skill`; \
never invent or guess a skill name from memory. Match a task only against descriptions \
actually shown below. If a task clearly matches a shown skill's description — not only \
when the user names the skill — you MUST load that exact skill with `use_skill` and \
follow it BEFORE doing the work. If this catalog says skills were omitted, call \
`list_skills` before using an omitted name. If `use_skill` reports a missing skill, \
do not guess another name; briefly note it and continue with the best fallback. \
Announce in one line which skill you're using.";

pub struct CatalogEntry { pub name: String, pub description: String }

pub fn render_skill_catalog(entries: &[CatalogEntry]) -> Option<String> {
    if entries.is_empty() { return None; }
    let mut sorted: Vec<&CatalogEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut lines: Vec<String> = Vec::new();
    let mut body_bytes = 0usize;
    let mut omitted = 0usize;
    for e in &sorted {
        let line = format!("- {}: {}", e.name, truncate_desc(&e.description));
        let cost = line.len() + 1;
        if lines.is_empty() || body_bytes + cost <= CATALOG_BYTE_BUDGET {
            body_bytes += cost;
            lines.push(line);
        } else { omitted += 1; }
    }

    let mut out = String::new();
    out.push_str(CATALOG_HEADER);
    out.push('\n');
    out.push_str(GUIDANCE);
    out.push('\n');
    out.push_str(&lines.join("\n"));
    if omitted > 0 {
        out.push('\n');
        out.push_str(&format!("... and {omitted} more lower-priority skills not shown."));
    }
    Some(out)
}

fn truncate_desc(desc: &str) -> String {
    if desc.chars().count() <= PER_SKILL_DESC_CAP { return desc.to_string(); }
    let cut: String = desc.chars().take(PER_SKILL_DESC_CAP).collect();
    format!("{cut}…")
}
```

### 11.4 `use_skill` Tool（仿 atomcode）

```rust
// src/agent/tools/use_skill.rs

use std::sync::Arc;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::agent::skills::registry::SkillRegistry;
use crate::agent::tools::{Tool, ToolContext, ToolResult, ok, err};

pub struct UseSkillTool { registry: Arc<SkillRegistry> }

#[derive(Deserialize)]
struct Args {
    name: String,
    #[serde(default)]
    arguments: Option<String>,
}

#[async_trait]
impl Tool for UseSkillTool {
    fn name(&self) -> &str { "use_skill" }

    fn description(&self) -> &str {
        "Invoke a named skill (a reusable prompt/workflow template) and return its content \
         with your arguments substituted. The name must exactly match a skill listed under \
         '=== AVAILABLE SKILLS ===' in the system prompt or returned by list_skills. Never \
         invent or guess a skill name. Trigger a skill when the task matches its listed \
         description — not only when the user names it. list_skills shows any lower-priority \
         skills omitted from the prompt catalog."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Exact skill name from AVAILABLE SKILLS or list_skills; never invent a name" },
                "arguments": { "type": "string", "description": "Arguments passed to the skill (optional)" }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: &str, _ctx: &ToolContext) -> ToolResult {
        let a: Args = match serde_json::from_str(args) {
            Ok(a) => a,
            Err(e) => return err(format!("use_skill: invalid arguments: {e}. Expected {{\"name\":\"<skill>\"}}.")),
        };
        let skill = match self.registry.get(&a.name) {
            Some(s) => s,
            None => {
                let names: Vec<String> = self.registry.list().into_iter().map(|(n, _)| n).collect();
                return err(format!(
                    "use_skill: skill '{}' not found. Available: {}. Do not guess another skill name; \
                    use an exact available name or continue without a skill",
                    a.name, if names.is_empty() { "(none)".to_string() } else { names.join(", ") }
                ));
            }
        };
        let arguments = a.arguments.unwrap_or_default();
        // shell 注入可能阻塞 → 离线
        let content = tokio::task::spawn_blocking(move || skill.expand(&arguments, "")).await.unwrap_or_default();
        ok(content)
    }
}

pub struct ListSkillsTool { registry: Arc<SkillRegistry> }

#[async_trait]
impl Tool for ListSkillsTool {
    fn name(&self) -> &str { "list_skills" }
    fn description(&self) -> &str { "List the available skills (name + description). Invoke one with use_skill." }
    fn parameters_schema(&self) -> serde_json::Value { json!({ "type": "object", "properties": {} }) }

    async fn execute(&self, _args: &str, _ctx: &ToolContext) -> ToolResult {
        let skills = self.registry.list();
        if skills.is_empty() { return ok("No skills are loaded.".to_string()); }
        let mut out = format!("Available skills ({}):\n", skills.len());
        for (name, desc) in &skills {
            if desc.is_empty() { out.push_str(&format!("- {name}\n")); }
            else { out.push_str(&format!("- {name}: {desc}\n")); }
        }
        ok(out)
    }
}
```

### 11.5 注册进 Work Agent

```rust
// src/agent/tools/mod.rs ── builtin_registry()

use std::sync::Arc;
use crate::agent::skills::registry::SkillRegistry;
use crate::agent::tools::use_skill::{UseSkillTool, ListSkillsTool};

pub fn builtin_registry(skills: Arc<SkillRegistry>) -> Vec<Arc<dyn Tool>> {
    let mut registry: Vec<Arc<dyn Tool>> = vec![
        Arc::new(BashTool::new()),
        Arc::new(ReadTool::new()),
        Arc::new(WriteTool::new()),
    ];
    // Skill tools 跟在 builtin 后面
    registry.push(Arc::new(UseSkillTool::new(skills.clone())));
    registry.push(Arc::new(ListSkillsTool::new(skills)));
    registry
}
```

### 11.6 系统提示词注入

```rust
// src/agent/system_prompt/mod.rs ── 追加 Skill catalog 段

pub fn render_system_prompt(profile: &AgentProfile, skills_catalog: Option<&str>) -> String {
    let mut prompt = render_base_prompt(profile);
    if let Some(catalog) = skills_catalog {
        prompt.push_str("\n\n");
        prompt.push_str(catalog);
    }
    prompt.push_str(&render_protocol_suffix(profile.protocol));
    prompt
}
```

```rust
// src/agent/mod.rs ── run_session() 启动时

let skills = SkillRegistry::load(&standard_skill_dirs(&home, &working_dir));
let catalog = render_skill_catalog(
    &skills.list().into_iter().map(|(n, d)| CatalogEntry { name: n, description: d }).collect::<Vec<_>>()
);
let prompt = render_system_prompt(&profile, catalog.as_deref());
```

### 11.7 TUI `/skill:` 补全

```rust
// src/tui/completion.rs ── SlashCommand 列表新增
"/skill:<name>" => "/skill:".to_string(),  // 列出所有 skill 名作为子命令
```

### 11.8 集成到 `agent/mod.rs` 完整胶水

```rust
// src/agent/mod.rs ── 加载 + 注入

use std::sync::Arc;
use crate::agent::skills::registry::{SkillRegistry, standard_skill_dirs};
use crate::agent::skills::render::{render_skill_catalog, CatalogEntry};

pub async fn run_session(session: Session) -> Result<(), AgentError> {
    let home = session.worktree.home();
    let cwd = session.worktree.cwd();

    // 1) 加载 skills
    let skills = Arc::new(SkillRegistry::load(&standard_skill_dirs(home, cwd)));

    // 2) 渲染 catalog
    let entries: Vec<CatalogEntry> = skills.list().into_iter()
        .map(|(name, description)| CatalogEntry { name, description })
        .collect();
    let catalog = render_skill_catalog(&entries);

    // 3) 拼装 system prompt
    let profile = work_profile();
    let prompt = render_system_prompt(&profile, catalog.as_deref());

    // 4) 启动 agent loop
    let mut context = Context::new();
    context.system = prompt;

    // 5) tools = builtin + skill tools
    let tools = builtin_registry(skills.clone());
    // ...
}
```

### 11.9 测试用例

```rust
#[tokio::test]
async fn use_skill_expands() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("greet.md"), "Hello $ARGUMENTS!").unwrap();
    let reg = Arc::new(SkillRegistry::load(&[temp.path().to_path_buf()]));
    let tool = UseSkillTool::new(reg);
    let ctx = ToolContext::default();
    let r = tool.execute(r#"{"name":"greet","arguments":"world"}"#, &ctx).await;
    assert!(!r.is_error);
    assert_eq!(r.content, "Hello world!");
}

#[test]
fn catalog_8kb_budget() {
    let entries: Vec<CatalogEntry> = (0..100).map(|i| CatalogEntry {
        name: format!("skill-{i}"),
        description: "x".repeat(200),
    }).collect();
    let out = render_skill_catalog(&entries).unwrap();
    assert!(out.contains("more lower-priority skills not shown"));
}
```

---

## 附录 A: 六项目 Skill 系统源码索引（7000+ 行）

### atomcode
- `crates/atomcode-capabilities/src/skills/mod.rs` (54 lines)
- `crates/atomcode-capabilities/src/skills/skill.rs` (519 lines)
- `crates/atomcode-capabilities/src/skills/registry.rs` (571 lines)
- `crates/atomcode-capabilities/src/skills/render.rs` (401 lines)
- `crates/atomcode-capabilities/src/skills/use_skill.rs` (316 lines)
- `crates/atomcode-capabilities/src/skills/catalog_hook.rs` (128 lines)
- `crates/atomcode-capabilities/assets/setup-seeds/skills/atomcode-automation-recommender/SKILL.md`

### claudecode
- `src/skills/bundledSkills.ts` (220 lines)
- `src/skills/loadSkillsDir.ts` (1086 lines)
- `src/skills/mcpSkillBuilders.ts` (45 lines)
- `src/skills/bundled/index.ts` (60 lines)
- `src/skills/bundled/{batch,claudeApi,claudeInChrome,debug,keybindings,loop,loremIpsum,remember,scheduleRemoteAgents,simplify,skillify,stuck,updateConfig,verify,verifyContent}.ts`

### deepseek-harness
- `packages/skill/skill/src/index.ts` (869 lines)
- `packages/skill/skill/src/invariant.ts`
- `packages/skill/skill-filesystem/src/index.ts` (1042 lines)
- `packages/skill/skill-badge/src/index.ts` (61 lines)
- `packages/skill/skill-badge/assets/dsh-badge.md`
- `packages/skill/tool-skill/src/index.ts` (431 lines)

### openclaw
- `src/skills/types.ts` (156 lines)
- `src/skills/frontmatter.ts`
- `src/skills/loading/local-loader.ts`
- `src/skills/loading/skill-contract.ts`
- `src/skills/runtime/runtime-config.ts`
- `src/skills/workshop/service.ts`
- `src/skills/workshop/service-propose.ts`
- `src/skills/workshop/service-evaluation.ts`
- `src/skills/workshop/proposal-draft.ts`
- `src/skills/workshop/policy.ts`
- `src/skills/workshop/curator.ts`
- `src/skills/workshop/history-scan.ts`
- `src/skills/workshop/experience-review.ts`
- `src/skills/workshop/apply-transition.ts`
- `src/skills/workshop/store.ts` + 8 个 store-sqlite-*.ts
- `src/skills/security/{clawhub-verdicts,scan-evidence,scanner,workspace-audit}.ts`
- `src/infra/clawhub-skills.ts`
- `src/infra/clawhub-skill-security.ts`
- `src/node-host/skills.ts`
- `src/shared/node-skill-constraints.ts`
- `skills/` (52 个用户向 skill)
- `.agents/skills/` (50 个 custodian skill)
- `skills/skill-creator/scripts/quick_validate.py`

### opencode
- `packages/core/src/skill.ts` (133 lines)
- `packages/core/src/skill/discovery.ts` (137 lines)
- `packages/core/src/tool/skill.ts` (109 lines)
- `packages/schema/src/skill.ts` (56 lines)
- `packages/opencode/src/skill/discovery.ts` (213 lines)
- `packages/opencode/src/skill/index.ts` (354 lines)
- `packages/opencode/src/skill/guidance.ts`
- `packages/opencode/src/tool/skill.txt` (5 lines)

### pi
- `packages/coding-agent/src/core/skills.ts` (508 lines)
- `packages/coding-agent/docs/skills.md` (~300 lines)
- `packages/agent/src/harness/skills.ts` (500+ lines)
- `packages/coding-agent/examples/sdk/04-skills.ts`
- `packages/coding-agent/test/skills.test.ts`
- `packages/coding-agent/test/sdk-skills.test.ts`
- `packages/coding-agent/test/fixtures/skills/{valid-skill,consecutive-hyphens,disable-model-invocation,...}/SKILL.md`

---

## 附录 B: 借鉴优先级矩阵

```
实现        紧急度  复杂度  价值    风险    推荐阶段
─────────────────────────────────────────────────────
Skill 数据模型  P0     低      高      低      Week 1
registry 加载    P0     低      高      低      Week 1
8KB 预算 + render P0    低      高      低      Week 1
use_skill tool   P0     低      高      低      Week 2
list_skills tool P0     低      中      低      Week 2
system prompt 注入 P0   低      高      低      Week 3
TUI /skill:补全  P0     低      中      低      Week 4
E2E 测试用例     P0     低      高      低      Week 4
skill-creator   P1     中      高      中      Week 5-6
Workshop 自演化  P1     极高    极高    高      Week 7-8
HTTPS discovery P2     中      高      中      Week 9-10
atomic 版本切换  P2     中      高      中      Week 11-12
ClawHub 风格远程 P2     高      中      高      Week 12+
Workshop SQLite  P2     高      高      中      Week 12+
```

---

## 附录 C: laew Skill 系统设计原则（综合六项目最佳实践）

1. **数据模型简单**：5 字段 Skill struct（name/desc/template/allowed_tools/user_invocable），仿 atomcode
2. **BTreeMap 存储**：保证 catalog 注入顺序字节稳定 → prompt prefix cache
3. **8KB 字节预算**：仿 atomcode 防信号稀释
4. **3 源发现**：user/project/cross-agent，仿 pi（最简）
5. **`use_skill` + `list_skills` tools**：仿 atomcode（最小可用）
6. **system prompt 注入**：作为 system message 段（仿 atomcode，不走持久化 user message，简化 laew 的 SessionContext）
7. **`/skill:name` TUI 补全**：仿 pi（最小工作流）
8. **`!cmd` shell 注入**：信任用户，仿 atomcode
9. **诊断不静默**：仿 pi，命名冲突 / 解析失败都收集 diagnostics
10. **`.agents/skills` 兼容**：仿 pi/opencode/openclaw，可直接读 pi / openclaw skill
11. **P0 不做**：Workshop / 自演化 / 远程拉取 / 动态发现 / conditional paths —— 这些是 P1/P2

---

## 附录 D: laew 系统提示词新增 Skill 段草案

```markdown
=== AVAILABLE SKILLS ===

Skills are reusable instruction templates for specific tasks. The names listed
below are the only skill names you may pass directly to `use_skill`; never
invent or guess a skill name from memory. Match a task only against descriptions
actually shown below. If a task clearly matches a shown skill's description —
not only when the user names the skill — you MUST load that exact skill with
`use_skill` and follow it BEFORE doing the work. If this catalog says skills
were omitted, call `list_skills` before using an omitted name. If `use_skill`
reports a missing skill, do not guess another name; briefly note it and
continue with the best fallback. Announce in one line which skill you're using.

- code-review: Review code changes for correctness, security, and style
- run-tests: Run unit tests and report failures
- commit: Create git commits following project conventions
```

---

## 附录 E: 与现有 laew 模块的集成点

| laew 模块 | 集成方式 |
|----------|---------|
| `src/agent/mod.rs::run_session` | 启动时 `SkillRegistry::load()` + 注入 catalog |
| `src/agent/profile.rs::work_profile()` | system prompt 渲染时追加 catalog 段 |
| `src/agent/tools/mod.rs::builtin_registry()` | 新增 `UseSkillTool` + `ListSkillsTool` |
| `src/agent/system_prompt/mod.rs` | 新增 `render_skill_catalog()` 调用 |
| `src/tui/completion.rs` | 新增 `/skill:` 补全 |
| `src/config/mod.rs::Paths::detect()` | 用 home/worktree 计算 standard_skill_dirs |
| `testReport/run_e2e.sh` | 新增 skill 加载用例 |

---

## 附录 F: 后续路线图总结

| 阶段 | 时间 | 核心交付 | 借鉴项目 |
|------|------|---------|----------|
| **P0 MiniPoC** | Week 1-4 | Skill 加载 + 2 tools + 8KB catalog + TUI 补全 | atomcode + pi |
| **P1 自演化** | Week 5-8 | skill-creator + Workshop SQLite | claudecode + openclaw |
| **P2 远程分发** | Week 9-12 | HTTPS discovery + atomic 切换 | opencode |
| **P3 大统一** | 3-6 月 | 6 Agent 全挂 skill + Skill-aware Yolo + 跨 agent 共享 | 综合 |

---

**完成**: 本文对比 6 项目 ~7000 行 Skill 系统源码，覆盖 atomcode 的 catalog hook、claudecode 的 6 源加载、deepseek-harness 的 registry+provider、openclaw 的 Workshop 自演化、opencode 的远程 atomic 切换、pi 的 Agent Skills 标准与 SDK 替换接口。laew 的 P0 MiniPoC 蓝图可直接落地（仿 atomcode 实现，4 周交付）。