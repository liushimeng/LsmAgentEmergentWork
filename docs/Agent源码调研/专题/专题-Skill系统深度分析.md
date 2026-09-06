# Skill 系统深度分析 —— 6 个 Agent 项目横向专题

- **分析日期**：2026-09-04
- **分析范围**：Claude Code / AtomCode / OpenClaw / OpenCode / deepseek-harness / pi
- **方法论**：从各项目「核心机制深度分析」+「深度分析」文档中提取 Skill 相关章节，逐维度横向对比
- **前置文档**：各项目 `*-核心机制深度分析.md`、`*-深度分析.md`（共 12 份）
- **目标读者**：laew 架构设计者，评估 Skill 系统引入方案

---

## 一、横向对比总览表

| 维度 | Claude Code | AtomCode | OpenClaw | OpenCode | deepseek-harness | pi |
|------|-------------|----------|----------|----------|-----------------|-----|
| **语言** | TypeScript | Rust | TypeScript | TypeScript (Effect) | TypeScript (Cordis) | TypeScript |
| **Skill 定义格式** | Markdown + YAML Frontmatter | Markdown + YAML Frontmatter | Markdown + YAML Frontmatter | Markdown + YAML Frontmatter | Markdown + YAML Frontmatter | Markdown + YAML Frontmatter |
| **文件命名** | `SKILL.md` 或 `*.md` | `SKILL.md` 或 `*.md` | `SKILL.md` | `SKILL.md` | `SKILL.md` 或 `skill.md` | `SKILL.md` 或 `*.md` |
| **发现路径** | `~/.claude/skills` + 项目 `.claude/skills` + 策略管理目录 | `~/.atomcode/skills` + `~/.claude/skills` + `~/.agents/skills` + 项目级 | `skills/` 内置 + `src/skills/` 加载 | `~/.claude/skills` + `~/.agents/skills` + 项目级 + 远程 URL | `.dsh/` + `.agents/` + 全局 + 作用域 | `~/.pi/skills` + 项目 `.pi/skills` + 多目录扫描 |
| **内置 Skill 数量** | 16+ | 外部加载 | 52 | 1+ (customize-opencode) | Provider 动态注册 | 外部加载 |
| **调用方式** | SkillTool（fork/inline） | UseSkillTool + ListSkillsTool | SkillTool + Skill Snapshot | SkillTool | SkillService.list/get | AgentLane.skill() + 模型自行读取 |
| **Prompt 注入** | XML 格式 `<available_skills>` | `=== AVAILABLE SKILLS ===` 目录 | XML 格式 `<available_skills>` | `<skill_content>` 标签 | formatSkillsForSystemPrompt | XML 格式 `<available_skills>` |
| **条件激活** | `paths` 字段（gitignore 风格） | 无 | 无 | 无 | 无 | 无 |
| **Fork 子 Agent** | `context: fork` | 无 | 无 | 无 | 无 | 无 |
| **参数展开** | `$arg1` 变量 | `$ARGUMENTS[N]` / `$N` / `$ARGUMENTS` / `!`cmd`` | 无 | 无 | 无 | 无 |
| **Shell 预执行** | `!`cmd`` | `!`cmd`` | 无 | 无 | 无 | 无 |
| **热更新** | 文件变更检测 + 缓存清除 | 无 | 无 | 无 | 无 | 无 |
| **权限检查** | deny/allow 规则 + SAFE_SKILL_PROPERTIES | allowed-tools 字段 | 无 | permission: "skill" | 无 | 无 |
| **使用追踪** | recordSkillUsage | 无 | 无 | 无 | 无 | 无 |
| **自演化** | 无 | 无 | Workshop（5 阶段） | 无 | 无 | 无 |
| **Prompt 预算** | 25K tokens（压缩后恢复） | 无 | 150 skills / 18K chars 渐进降级 | 无 | 无 | 无 |
| **Prune 保护** | 无 | 无 | 无 | skill 工具永不裁剪 | 无 | 无 |
| **命名空间** | 无 | `<namespace>:<skill-name>` | 无 | 无 | 无 | 无 |
| **优先级系统** | realpath 去重 | source_rank（用户 > 项目 > 插件） | 无 | 无 | SkillRank 枚举（7 级） | 无 |
| **安装 spec** | 无 | 无 | brew/node/go/uv/download | 无 | 无 | 无 |
| **MCP 桥接** | mcpSkillBuilders.ts | 无 | 无 | 无 | 无 | 无 |
| **版本兼容** | 无 | 无 | snapshot 版本 + 重建 | 无 | 无 | 无 |
| **Ignore 规则** | 无（用 paths 字段） | 无 | 无 | 无 | 无 | `.gitignore` / `.ignore` / `.fdignore` |
| **PromptTemplate** | 无（Skill 兼容） | 无 | 无 | 无 | 无 | 独立概念（`$@` 参数） |

---

## 二、各项目 Skill 系统详析

### 2.1 Claude Code —— 最成熟的 Skill 系统

#### 2.1.1 文件格式

Claude Code 的 Skill 采用 **Markdown + YAML Frontmatter** 格式，Frontmatter 支持丰富的元数据字段：

```yaml
---
name: "展示名称"
description: "一句话描述"            # 必填
when_to_use: "详细使用场景描述"      # 模型自动触发依据
allowed-tools: [Bash(git:*), Read]  # 工具权限模式
argument-hint: "<arg>"
arguments: [arg1, arg2]
context: fork | inline              # fork=子 Agent，inline=当前对话
agent: agent-name
model: opus | sonnet | haiku
effort: high | low | medium
user-invocable: true/false
disable-model-invocation: true
paths: ["src/**", "tests/**"]       # 条件激活路径
hooks: { PreToolUse: [...] }
---

# Skill 标题
Markdown 正文。支持 $arg1、${CLAUDE_SKILL_DIR}、${CLAUDE_SESSION_ID} 变量
支持 !`shell_command` 内联执行
```

**关键设计**：
- `when_to_use` 是模型自动触发的核心依据，模型根据描述判断是否需要调用该 Skill
- `allowed-tools` 支持通配符模式（如 `Bash(git:*)`），精细控制 Skill 执行期间的工具权限
- `context: fork` 将 Skill 隔离到子 Agent 执行，`context: inline` 在当前对话中执行
- `paths` 字段使用 gitignore 风格匹配，实现条件激活

#### 2.1.2 注册与发现机制

```typescript
// loadSkillsDir.ts:638
export const getSkillDirCommands = memoize(async (cwd) => {
  const userSkillsDir = join(getClaudeConfigHomeDir(), 'skills');     // ~/.claude/skills
  const managedSkillsDir = join(getManagedFilePath(), '.claude', 'skills'); // 策略管理
  const projectSkillsDirs = getProjectDirsUpToHome('skills', cwd);   // 项目级
  // 并行加载所有来源，基于 realpath 去重
  const [managedSkills, userSkills, projectSkills, ...] = await Promise.all([...]);
});
```

**三层发现**：策略管理目录 > 用户 home > 项目目录，并行加载后基于 `realpath` 去重。

**内置 Skill 注册**：通过 `registerBundledSkill` API 注册，适合编译进二进制。内置 Skill 支持 `files` 字段，首次调用时提取到磁盘供模型通过 Read/Grep 访问。

#### 2.1.3 条件激活

```typescript
// loadSkillsDir.ts:997
export function activateConditionalSkillsForPaths(filePaths, cwd) {
  for (const [name, skill] of conditionalSkills) {
    const skillIgnore = ignore().add(skill.paths);  // gitignore 风格匹配
    for (const filePath of filePaths) {
      if (skillIgnore.ignores(relativePath)) {
        dynamicSkills.set(name, skill);   // 激活
        conditionalSkills.delete(name);
      }
    }
  }
}
```

当用户或模型操作文件时，系统检查文件路径是否匹配 Skill 的 `paths` 模式，匹配则动态激活。这减少了无关 Skill 的 token 占用。

#### 2.1.4 调用流程

```typescript
// SkillTool.ts:580
async call({ skill, args }, context, ...) {
  const command = findCommand(commandName, commands);
  recordSkillUsage(commandName);  // 使用频率追踪

  // 分支 1：fork 模式 → 启动子 Agent
  if (command.context === 'fork') {
    return executeForkedSkill(command, commandName, args, context, ...);
  }

  // 分支 2：inline 模式 → 注入消息到当前对话
  const processedCommand = await processPromptSlashCommand(...);
  return {
    data: { success: true, commandName, allowedTools, model },
    newMessages,
    contextModifier(ctx) { /* 更新工具权限、模型覆盖 */ },
  };
}
```

**调用后效果**：SkillTool 的返回值包含 `contextModifier`，可以动态修改当前上下文的工具权限和模型参数。

#### 2.1.5 内置 Skill 列表

| Skill | 用途 | 模型可调用 | 用户可调用 |
|-------|------|-----------|-----------|
| `update-config` | 配置 settings.json | Yes | Yes |
| `verify` | 验证代码变更 | Yes | Yes |
| `debug` | 调试会话问题 | No | Yes |
| `skillify` | 捕获会话为 Skill | No | Yes |
| `remember` | 审查自动记忆 | Yes | Yes |
| `simplify` | 代码审查（三 Agent） | Yes | Yes |
| `batch` | 并行批量变更 | No | Yes |
| `stuck` | 诊断卡住的会话 | Yes | Yes |
| `schedule` | 调度远程 Agent | Yes | Yes |
| `claude-api` | Claude API 指南 | Yes | Yes |

#### 2.1.6 高级特性

- **文件变更检测**：监控 `.claude/skills/` 目录，SKILL.md 修改时清除缓存并重新加载，支持热更新
- **使用频率追踪**：`recordSkillUsage` 记录使用频率，用于排序建议和自动推荐
- **MCP Skill 桥接**：`mcpSkillBuilders.ts` 解决循环依赖，MCP prompts 可注册为 `loadedFrom: 'mcp'` 的 Skill
- **权限检查**：deny/allow 规则 + `SAFE_SKILL_PROPERTIES` 白名单，不含 `allowedTools`/`hooks` 的 Skill 自动放行
- **压缩后恢复**：`POST_COMPACT_SKILLS_TOKEN_BUDGET = 25,000` tokens，`POST_COMPACT_MAX_TOKENS_PER_SKILL = 5,000` tokens
- **Fork 子 Agent**：`context: fork` 的 Skill 在独立子 Agent 中执行，共享 prompt cache，消息隔离

---

### 2.2 AtomCode —— Rust 实现的 Skill 系统

#### 2.2.1 文件格式

AtomCode 的 Skill 同样采用 **Markdown + YAML Frontmatter** 格式，支持两种形态：

- **扁平 `*.md`**：`name = file stem`，内容 = frontmatter + template
- **目录 `<dir>/SKILL.md`**：`name = directory name`，可捆绑 `scripts/` / `references/`

Frontmatter 字段：
```yaml
---
name: 技能名
description: 描述（默认取 template 首段）
allowed-tools: Bash Read  # 空格/逗号分隔
user-invocable: false     # 隐藏于菜单，模型仍可自动调用
---
```

**命名空间**：插件 Skill 注册为 `<namespace>:<skill-name>` 格式，与 MCP 工具的 `mcp__*` 命名空间隔离。

#### 2.2.2 加载与注册

```rust
// SkillRegistry (skills/registry.rs)
SkillRegistry::load(&[PathBuf])    // 扫描目录
load_dir(dir, namespace)            // 带命名空间加载

// 标准 Skill 目录（skills/render.rs:56-95）
standard_skill_dirs / runtime_skill_dirs:
  ~/.atomcode/skills
  ~/.claude/skills
  ~/.agents/skills
  project/.atomcode/skills
```

**优先级**（`source_rank`）：用户 home > 项目 > 插件。

#### 2.2.3 模板展开与 Shell 预执行

```rust
// skill.rs:32-59 - Skill::expand
// 模板变量：
$ARGUMENTS[N] / $N           // 位置参数
$ARGUMENTS                   // 全量参数
${CLAUDE_SESSION_ID}         // 会话 ID
${CLAUDE_SKILL_DIR}          // Skill 所在目录

// skill.rs:150-168 - Shell 预执行
!`cmd`                       // 执行 shell 命令并将结果注入模板
```

**expand_for_injection**（`skill.rs:64-70`）：目录 Skill 附带 `<system-reminder>` 安装路径提示，告知模型安装路径，避免在 cwd 搜索捆绑文件。

#### 2.2.4 调用工具

AtomCode 将 Skill 暴露为两个专用工具：

- **UseSkillTool**（`skills/use_skill.rs:12-89`）：查找 Skill → `expand_for_injection` → 返回内容；错误时列出可用 Skill 名
- **ListSkillsTool**（`skills/use_skill.rs:91-127`）：列出所有 Skill

通过 `register_skill_tools`（`skills/mod.rs:33-34`）注册到 `ToolRegistry`。

#### 2.2.5 Prompt 注入

```rust
// SkillCatalogHook (skills/catalog_hook.rs)
// session_start 时注入 === AVAILABLE SKILLS === 目录到 system prompt
render_catalog_prioritizing(render.rs:113)  // 按优先级渲染
```

**弱模型增强**：`SkillFirstHook`（`coding/src/skill_first.rs`）在 DeepSeek 等弱模型的首轮注入 "use_skill first" 提醒，引导模型优先使用 Skill 而非直接执行。

#### 2.2.6 设计要点

- Skill 是**受信用户内容**，Shell 注入（`!`cmd``）是设计意图而非安全漏洞
- 目录 Skill 通过 `<system-reminder>` 告知模型安装路径
- 与 MCP 工具命名空间隔离：`mcp__*` vs `<ns>:<skill>`
- 参数化系统提示词：根据 model/todo/enabled/subagent 等开关条件注入不同段落

---

### 2.3 OpenClaw —— 52 内置 Skill + Workshop 自演化

#### 2.3.1 文件格式

```yaml
---
name: coding-agent
description: "Delegate coding work to Codex, Claude Code, or OpenCode as background workers..."
metadata:
  openclaw:
    emoji: "🧩"
    requires:
      anyBins: ["claude", "codex", "opencode"]
      config: ["skills.entries.coding-agent.enabled"]
    install:
      - id: node-claude
        kind: node
        package: "@anthropic-ai/claude-code"
        bins: ["claude"]
---
```

**独特字段**：
- `metadata.openclaw.requires`：声明依赖的二进制文件和配置项
- `metadata.openclaw.install`：自动安装 spec，支持 `brew`/`node`/`go`/`uv`/`download` 五种 kind
- 安全校验：brew formula 正则、npm spec 校验、Go module 正则、URL 仅 http/https

#### 2.3.2 Skill 类型接口

```typescript
// skill-contract.ts
interface Skill {
  name: string;
  displayName?: string;       // 从 H1 标题解析
  description: string;
  locationNote?: string;
  readContent?: string;       // 非文件系统技能（node:// 等）
  filePath: string;
  baseDir: string;
  sourceInfo: SourceInfo;
  disableModelInvocation: boolean;
  source: string;
}
```

#### 2.3.3 Prompt 预算管理

`prepareSkillsForPrompt()`（`skill-prompt-limits.ts:78-187`）实现渐进降级：

1. `DEFAULT_MAX_SKILLS_IN_PROMPT = 150`
2. `DEFAULT_MAX_SKILLS_PROMPT_CHARS = 18,000`
3. **渐进降级策略**：
   - 先尝试 full 格式
   - 超预算 → compact 格式（描述截断到 `COMPACT_DESCRIPTION_MAX_CHARS = 220`）
   - 仍超 → 二分查找最大技能数
   - 最后尝试去掉 limit note
4. **截断警告**：`⚠️ Skills truncated: included X of Y`

#### 2.3.4 Skill Snapshot 与注入

`buildSkillSnapshot()`（`workspace-skill-prompt.ts:57-82`）：
- 解析 eligible skills → `filterPromptVisibleSkillEntries()` → `prepareSkillsForPrompt()`
- 输出 `SkillSnapshot`：`prompt` + `skills` + `skillFilter` + `skillOverrides` + `nodeSkillsEligibility` + `resolvedSkills` + `promptFormatVersion=4`
- **版本兼容**：`snapshotHasUnavailableSkill` + 旧格式 → `rebuildAfterUnsafeSnapshot()`；`snapshotHasLegacySkillIdentity` → 重建

#### 2.3.5 Workshop 自演化（最独特设计）

`src/skills/workshop/` 是 OpenClaw 最独特的部分——**技能可以自我演化**。完整流程：

| 阶段 | 文件 | 核心函数 | 说明 |
|------|------|---------|------|
| 1. History Scan | `history-scan.ts` | `runSkillHistoryScanCore()` | 扫描会话历史，发现技能改进机会。游标分页（`oldestCursor`/`newestCursor`），持久化状态 `StoredHistoryScanState` |
| 2. Experience Review | `experience-review.ts` | `prepareSkillExperienceReviewCandidate()` | 后台 Agent 评审技能使用体验。过滤 cron/heartbeat/memory/overflow/sandbox 会话。最少 10 次模型迭代，超时 120s |
| 3. Proposal Generation | `proposal-generation.ts` | `stageSkillProposalGeneration()` | 生成技能修改提案。原子写入（先 staging dir → move）。状态：`pending/applied/rejected/quarantined/stale` |
| 4. Autonomous Apply | `autonomous-apply.ts` | `applyAutonomousSkillProposal()` | 自动应用提案。workshop-owned 技能可直接应用；user-authored 技能 → `pending` 等待人工审核 |
| 5. Collection Plan | `collection-plan.ts` | `validateSkillCollectionPlan()` | 技能集合规划。每个 Agent 必须保留至少一个可见技能 |

**安全机制**：
- `isWorkshopOwnedSkillDir()`：只有 workshop 创建的技能可自动修改
- `revisionHash` + `expectedRevisionHash`：乐观并发控制
- `SkillProposalSupportFile`：附件 hash 校验
- `MAX_SKILL_PROPOSAL_ORIGIN_RUN_IDS = 4096`

**Workshop Schema**：`SKILL_WORKSHOP_SCHEMA = "openclaw.skill-workshop.proposal.v1"`
**Mutation Budget**：`SkillWorkshopProposalMutationBudget`：run 级预算（remaining/completed/successfulMutations/failedMutations）

#### 2.3.6 渐进披露

OpenClaw 的工具表面可能很大（52 个内置 Skill + MCP + 插件），引入**渐进披露**机制控制 token 消耗。

---

### 2.4 OpenCode —— 双轨制 + 远程拉取

#### 2.4.1 文件格式

OpenCode 采用标准的 **Markdown + YAML Frontmatter** 格式，`SKILL.md` 命名。内置 `customize-opencode` skill（`skill/index.ts:21-35`）。

#### 2.4.2 发现路径（最广泛）

```typescript
// skill/index.ts:173 - discoverSkills
const discoverSkills = function* (...) {
  // 1. 外部目录（默认启用）
  if (!disableExternalSkills) {
    if (!disableClaudeCodeSkills) externalDirs.push(".claude")   // ~/.claude/skills/**/SKILL.md
    externalDirs.push(".agents")                                 // ~/.agents/skills/**/SKILL.md
    for (const dir of externalDirs) {
      yield* scan(state, path.join(global.home, dir), "skills/**/SKILL.md", { dot: true, scope: "global" })
    }
    // 向上查找项目级
    const upDirs = yield* fsys.up({ targets: externalDirs, start: directory, stop: worktree })
    for (const root of upDirs) {
      yield* scan(state, root, "skills/**/SKILL.md", { dot: true, scope: "project" })
    }
  }
  // 2. 配置目录
  const configDirs = yield* config.directories()
  for (const dir of configDirs) { yield* scan(state, dir, "{skill,skills}/**/SKILL.md") }
  // 3. 自定义路径
  for (const item of cfg.skills?.paths ?? []) { yield* scan(state, dir, "**/SKILL.md") }
  // 4. 远程 URL 拉取
  for (const url of cfg.skills?.urls ?? []) {
    const pulledDirs = yield* discovery.pull(url)
    for (const dir of pulledDirs) { yield* scan(state, dir, "**/SKILL.md") }
  }
}
```

**四层发现**：全局外部目录 → 项目级向上查找 → 配置目录 → 远程 URL 拉取。OpenCode 是唯一支持**远程 Skill 拉取**的项目。

#### 2.4.3 Frontmatter 解析与去重

```typescript
// skill/index.ts:105 - add
const add = function* (state, match, events) {
  const md = yield* Effect.tryPromise({ try: () => ConfigMarkdown.parse(match) })
  if (!isSkillFrontmatter(md.data)) return  // 必须有 name
  if (state.skills[md.data.name]) {
    yield* Effect.logWarning("duplicate skill name", {...})  // 去重
  }
  state.skills[md.data.name] = { name: md.data.name, description: md.data.description, location: match, content: md.content }
}
```

#### 2.4.4 SkillTool 运行时注入

```typescript
// tool/skill.ts:12
export const SkillTool = Tool.define("skill", Effect.gen(function* () {
  return {
    parameters: Schema.Struct({ name: Schema.String }),
    execute: (params, ctx) => Effect.gen(function* () {
      const info = yield* skill.require(params.name)
      yield* ctx.ask({ permission: "skill", patterns: [params.name] })
      const dir = path.dirname(info.location)
      const files = yield* ripgrep.find({ cwd: dir, pattern: "!**/SKILL.md", hidden: true, limit: 10 })
      return {
        title: `Loaded skill: ${info.name}`,
        output: [
          `<skill_content name="${info.name}">`,
          `# Skill: ${info.name}`, "", info.content.trim(), "",
          `Base directory for this skill: ${base}`,
          "Relative paths in this skill ... are relative to this base directory.",
          "<skill_files>", files.map(...), "</skill_files>",
          "</skill_content>",
        ].join("\n"),
      }
    }),
  }
}))
```

**独特设计**：
- SkillTool 返回 `<skill_content>` 标签，包含 Skill 正文 + 同目录下的关联文件列表
- 使用 `ripgrep` 查找 Skill 目录下的辅助文件（排除 SKILL.md 本身），帮助模型了解可用资源
- `ctx.ask({ permission: "skill" })` 请求权限确认

#### 2.4.5 Prune 保护

```typescript
// compaction.ts
const PRUNE_PROTECTED_TOOLS = ["skill"]  // skill 工具永不裁剪
```

在上下文压缩时，Skill 工具的输出被保护，不会被裁剪。这确保 Skill 内容在长会话中始终可用。

---

### 2.5 deepseek-harness —— Layered Registry 架构

#### 2.5.1 核心架构

deepseek-harness 的 Skill 系统基于 **Cordis Everything-is-a-Plugin** 框架，采用分层 Provider 注册模式。

```typescript
// packages/skill/skill/src/index.ts (Lines 1-869)
export class SkillRegistry extends Service {
  // 分层 Provider 注册
  private readonly layers = new ScopedLayers<SkillLayer>(
    scope => new SkillLayer(scope),
    () => { this.invalidateCache() },
  )

  registerProvider(create: (control: SkillProviderControl) => SkillProvider): () => void { ... }
  register(skill: SkillRegistration): () => void { ... }
  async list(options: SkillViewOptions = {}): Promise<SkillSummary[]> { ... }
  async get(name: string, options: SkillViewOptions = {}): Promise<SkillDefinition | undefined> { ... }
}
```

#### 2.5.2 分层架构

```typescript
class SkillLayer {
  scope: string | undefined  // undefined = global
  providers = new OrderedMap<string, { provider: SkillProvider; order: number }>()
  runtime = new Map<string, SkillDefinition>()
}

class ScopedLayers<T extends { scope: string | undefined }> {
  global: T
  private scopes = new Map<string, T>()

  chainLayers(scope: string | undefined): T[] {
    // 从远到近构建作用域链
    // 子作用域可覆盖父作用域的 Skill
  }
}
```

**核心设计**：每个 Cordis 作用域（scope）拥有独立的 `SkillLayer`，包含该作用域的 Provider 注册和运行时 Skill。`chainLayers` 从远到近构建作用域链，子作用域可覆盖父作用域的同名 Skill。

#### 2.5.3 优先级系统

```typescript
// packages/skill/skill/src/rank.ts
export const enum SkillRank {
  ProjectDSH = 100,      // .dsh/skill.md
  ProjectAgents = 200,   // .agents/skill.md
  Custom = 300,          // 自定义 Provider
  UserDSH = 400,         // ~/.dsh/skill.md
  UserAgents = 500,      // ~/.agents/skill.md
  Bundled = 600,         // 内置 Skill
  Runtime = 250,         // 运行时注册（特殊）
}
```

**排序规则**：Rank 降序 > Provider Order 升序 > 名称字母序。内置 Skill 优先级最高（600），项目级最低（100）。

#### 2.5.4 Provider 接口

```typescript
export interface SkillProvider {
  name: string
  list(options: SkillLookupOptions): Promise<SkillCandidate[]>
  get(candidate: SkillCandidate, options: SkillLookupOptions): Promise<SkillDefinition | undefined>
}

export interface SkillDefinition {
  name: string           // kebab-case
  description: string
  body: string           // Markdown 内容
  source: string         // 来源标识
  rank: SkillRank
  provider: string
  invocation: {
    modelInvocable: boolean
    userInvocable: boolean
  }
  metadata?: Record<string, unknown>
}
```

**文件系统 Provider**（`packages/skill/skill-filesystem/src/index.ts`）：
- `scanSkillFiles(directories, cwd)` 扫描目录
- `parseSkillFile(file)` 解析 frontmatter
- 构建 `SkillCandidate`（包含 `locator: { kind: 'filesystem', path: file.path }`）

#### 2.5.5 设计特点

- **Everything-is-a-Plugin**：SkillRegistry 本身是 Cordis Service，通过 `ctx.skills` 注入
- **Scoped Layers**：per-scope 的 Skill 隔离，子 Agent 可拥有独立的 Skill 集合
- **Provider 模式**：支持自定义 Provider 扩展，不限于文件系统
- **缓存失效**：Provider 注册/注销时自动 `invalidateCache()`

---

### 2.6 pi —— 极简主义 Skill 设计

#### 2.6.1 核心类型

```typescript
// packages/agent/src/harness/types.ts
export interface Skill {
  name: string;
  description: string;
  filePath: string;
  content: string;
  disableModelInvocation?: boolean;
}
```

**极简设计**：pi 的 Skill 接口只有 5 个字段，没有 `allowed-tools`、`context`、`hooks` 等高级特性。

#### 2.6.2 文件命名与发现

Skill 文件有两种命名约定：
- **`SKILL.md`**：每个目录显式声明一个 Skill（优先级高，递归时遇到即停止继续扫描子目录）
- **`*.md`**：目录下的普通 Markdown 文件也可被识别为 Skill

```typescript
// packages/agent/src/harness/skills.ts:104
async function loadSkillsFromDirInternal(
  env: ExecutionEnv,
  dir: string,
  includeRootFiles: boolean,   // 仅根目录扫描 *.md
  ignoreMatcher: IgnoreMatcher, // gitignore 风格规则
  rootDir: string,              // 用于计算相对路径
): Promise<{ skills: Skill[]; diagnostics: SkillDiagnostic[] }>
```

**递归策略**：
1. 先扫描当前目录是否有 `SKILL.md`，有则加载并**立即返回**（不再扫描子目录中的 SKILL.md）
2. 如果没有 SKILL.md，继续扫描：跳过 `.` 开头的目录和 `node_modules`
3. 子目录递归时 `includeRootFiles = false`（只有根目录的普通 `.md` 文件被识别为 Skill）
4. Ignore 规则从 `.gitignore` / `.ignore` / `.fdignore` 三个文件加载

#### 2.6.3 双路径调用

**路径 A：模型自行发现并读取 Skill 文件**

```typescript
// packages/agent/src/harness/system-prompt.ts:3
export function formatSkillsForSystemPrompt(skills: Skill[]): string {
  const visibleSkills = skills.filter((skill) => !skill.disableModelInvocation);
  const lines = [
    "The following skills provide specialized instructions for specific tasks.",
    "Read the full skill file when the task matches its description.",
    "When a skill file references a relative path, resolve it against the skill directory ...",
    "",
    "<available_skills>",
  ];
  for (const skill of visibleSkills) {
    lines.push("  <skill>");
    lines.push(`    <name>${escapeXml(skill.name)}</name>`);
    lines.push(`    <description>${escapeXml(skill.description)}</description>`);
    lines.push(`    <location>${escapeXml(skill.filePath)}</location>`);
    lines.push("  </skill>");
  }
  lines.push("</available_skills>");
  return lines.join("\n");
}
```

系统提示词注入 Skill 目录（XML 格式），模型根据 `description` 判断是否需要读取 Skill 文件的完整内容，然后使用 `read` 工具读取。

**路径 B：通过 `AgentLane.skill()` 显式调用**

```typescript
// agent-harness.ts:368
async skill(_name: string, _additionalInstructions?: string): Promise<RunResult>

// formatSkillInvocation
const skillBlock = `<skill name="${skill.name}" location="${skill.filePath}">
References are relative to dirname(skill.filePath).
${skill.content}
</skill>`;
```

#### 2.6.4 Skill vs Tool vs MCP 设计哲学

pi 的 Skill 系统体现了**极简主义**设计信条：

> "No MCP. Build CLI tools with READMEs."

| 特性 | MCP Tool | pi Skill | laew Tool |
|------|----------|----------|-----------|
| 定义格式 | JSON Schema | Markdown + YAML | Rust struct + impl Tool |
| 发现方式 | 服务端注册 | 文件系统扫描 | builtin_registry() |
| 调用方式 | JSON-RPC tool call | 模型自行读取遵循 | LLM tool call → execute() |
| 参数传递 | JSON arguments | 无（纯文本） | JSON -> serde 反序列化 |
| 运行时执行 | MCP Server 进程 | 无（模型用现有工具） | Rust async fn |
| 适用场景 | 复杂 API 集成 | 知识/流程/checklist | 代码执行/文件操作 |

**核心洞察**：Skill 是**纯文本指令注入**，不是可调用的工具。Skill 没有执行层，不产生 tool call，不需要参数验证。Skill 的"执行"是模型阅读指令后自行使用已有的 bash/read/edit/write 工具完成任务。

#### 2.6.5 PromptTemplate：平行注入路径

```typescript
// packages/agent/src/harness/types.ts:60
export interface PromptTemplate {
  name: string;
  description?: string;
  content: string;  // 支持 $@ 参数占位符
}
```

PromptTemplate 与 Skill 的区别：
- **Skill**：模型自行发现，读取文件内容后遵循
- **PromptTemplate**：通过 `AgentLane.promptFromTemplate(name, args)` 显式调用，`$@` 被替换为参数

`.pi/prompts/` 目录下的文件（pr.md / cl.md / sa.md 等）是 PromptTemplate 而非 Skill，它们用于斜杠命令（如 `/pr <URL>`）。

#### 2.6.6 实际 Skill 文件示例

```yaml
---
name: add-llm-provider
description: Checklist for adding a new LLM provider to packages/ai...
---
```

展示了 pi 的**"开发工作流 as Skill"**模式：一份详细的 checklist（7 个步骤：Core Types -> Provider Impl -> Exports -> Model Gen -> Tests -> Coding Agent -> Docs），模型在接到"添加新 LLM provider"任务时，先读取此文件，然后按步骤执行。

---

## 三、设计模式提炼

### 3.1 共性模式

#### 模式 P1：Markdown + Frontmatter 声明式定义

**所有 6 个项目**都采用 `Markdown + YAML Frontmatter` 作为 Skill 定义格式。这是行业共识，优势在于：
- 人类可读、可编辑
- Git 友好（可 diff、merge）
- 支持富文本描述（Markdown 正文）
- 元数据结构化（YAML Frontmatter）

#### 模式 P2：文件系统扫描发现

所有项目都通过扫描特定目录发现 Skill 文件：
- 用户 home 目录（全局 Skill）
- 项目目录（项目级 Skill）
- 内置目录（编译时注册）

典型路径约定：`~/.<tool>/skills/` + `project/.<tool>/skills/`

#### 模式 P3：XML Prompt 注入

Claude Code、OpenClaw、pi 三个项目都使用 XML 格式将 Skill 目录注入系统提示词：

```xml
<available_skills>
  <skill>
    <name>...</name>
    <description>...</description>
    <location>...</location>
  </skill>
</available_skills>
```

这是一种成熟的模式：XML 标签结构清晰，模型容易解析，支持 escapeXml 防注入。

#### 模式 P4：模型自主决策 + 显式调用双轨

多数项目同时支持两种调用路径：
- **模型自主决策**：Skill 目录注入系统提示词，模型根据 `description`/`when_to_use` 自行判断是否调用
- **显式调用**：用户通过 SkillTool 或 `AgentLane.skill()` 显式触发

#### 模式 P5：变量展开

Claude Code 和 AtomCode 支持模板变量：
- `$arg1` / `$ARGUMENTS[N]` / `$N`：位置参数
- `${CLAUDE_SESSION_ID}` / `${CLAUDE_SKILL_DIR}`：上下文变量
- `!`cmd``：Shell 预执行

### 3.2 差异化模式

#### 模式 D1：条件激活（Claude Code 独有）

`paths` 字段 + gitignore 风格匹配，实现"只在操作特定文件时激活特定 Skill"。减少无关 Skill 的 token 占用。

#### 模式 D2：Fork 子 Agent（Claude Code 独有）

`context: fork` 将 Skill 隔离到子 Agent 执行，消息隔离但共享 prompt cache。适合需要独立工具权限的 Skill。

#### 模式 D3：Workshop 自演化（OpenClaw 独有）

5 阶段流水线（History Scan -> Experience Review -> Proposal Generation -> Autonomous Apply -> Collection Plan），让 Skill 从使用经验中自动改进。这是**最前沿的设计**，将 Skill 从静态知识升级为可自我优化的智能体。

#### 模式 D4：Layered Registry + Provider（deepseek-harness 独有）

`ScopedLayers` + `SkillProvider` 接口 + `SkillRank` 优先级，实现 per-scope 的 Skill 隔离和自定义 Provider 扩展。适合需要多租户隔离的场景。

#### 模式 D5：Prompt 预算管理（OpenClaw）

`DEFAULT_MAX_SKILLS_IN_PROMPT = 150` + `DEFAULT_MAX_SKILLS_PROMPT_CHARS = 18,000` + 渐进降级策略（full -> compact -> 二分查找 -> 去掉 limit note）。当 Skill 数量很多时，必须控制注入的 token 量。

#### 模式 D6：远程 Skill 拉取（OpenCode 独有）

`cfg.skills?.urls` 配置远程 URL，`discovery.pull(url)` 拉取并缓存。支持从远程仓库共享 Skill。

#### 模式 D7：Ignore 规则兼容（pi 独有）

使用 `ignore` 库兼容 `.gitignore` / `.ignore` / `.fdignore` 三种忽略规则，与项目已有的忽略配置无缝衔接。

### 3.3 反模式警示

#### 反模式 A1：Skill 数量无上限

不控制 Skill 注入数量会导致 prompt 膨胀。OpenClaw 的渐进降级策略是正面案例；无限制注入是反模式。

#### 反模式 A2：Shell 注入无安全边界

AtomCode 的 `!`cmd`` 是设计意图（受信用户内容），但如果 Skill 来源不可信（如远程拉取），Shell 注入就是安全漏洞。需要区分"受信 Skill"和"非受信 Skill"。

#### 反模式 A3：Skill 与 Tool 混淆

pi 的设计哲学明确区分了 Skill（纯文本指令）和 Tool（可执行 API）。将 Skill 当作 Tool 使用会增加不必要的复杂度（参数验证、执行层、错误处理）。

---

## 四、对 laew 的综合建议

### 4.1 现状分析

laew 当前**无 Skill 系统**。laew 的架构特点：
- Rust 实现，多 Agent（6 角色）
- 系统提示词是静态拼接（`system_prompt/mod.rs`）
- 工具通过 `builtin_registry()` 注册（Bash/Read/Write）
- 项目上下文通过五级链发现（CLAUDE.md -> AGENTS.md -> README.md -> 自动生成 -> 空）
- Session 记忆通过 `session_memory` 表注入

laew 的 `AgentProfile`（`profile.rs`）定义了每个 Agent 的名称/系统提示词/工具集，但没有可扩展的 Skill 注入层。

### 4.2 推荐方案：分阶段引入 Skill 系统

#### Phase 1（P0）：基础 Skill 框架

**目标**：让 laew 支持 Markdown + Frontmatter 格式的 Skill 文件，通过系统提示词注入。

**核心设计**：

```rust
// src/agent/skill/mod.rs

/// Skill 定义
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,           // Markdown 正文
    pub location: PathBuf,         // 文件路径
    pub allowed_tools: Vec<String>, // 可选：工具权限
    pub user_invocable: bool,
    pub model_invocable: bool,
}

/// Frontmatter 解析
pub fn parse_skill_frontmatter(content: &str) -> Result<Skill> {
    // 解析 YAML frontmatter + Markdown body
}

/// Skill 发现
pub fn discover_skills(cwd: &Path, home: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    // 1. ~/.laew/skills/**/*.md
    // 2. 项目级 .laew/skills/**/*.md（向上查找）
    // 3. 内置 Skill（编译时注册）
    skills
}

/// Skill 注入到系统提示词
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    // XML 格式：<available_skills>...</available_skills>
}
```

**Skill 文件格式**（对齐行业标准）：

```yaml
---
name: deploy-staging
description: "部署当前分支到 staging 环境"
allowed-tools: [Bash, Read]
user-invocable: true
model-invocable: true
---

# Deploy to Staging

## Steps
1. Run tests: `cargo test`
2. Build: `cargo build --release`
3. Push: `git push origin HEAD:staging`
4. Verify: curl staging endpoint
```

**注入时机**：在 `system_prompt/mod.rs` 的 `build()` 方法中，将 Skill 目录注入系统提示词尾部。每个 Agent 类型（Yolo/Plan/Main-Work/SubAgent-Work/Quality-Check）可以有不同的 Skill 集合。

#### Phase 2（P1）：Skill 工具 + 条件激活

**目标**：添加 `use_skill` / `list_skills` 工具，支持条件激活。

**use_skill 工具**：

```rust
// src/agent/tools/skill.rs
pub struct UseSkillTool;

impl Tool for UseSkillTool {
    fn name(&self) -> &str { "use_skill" }
    fn description(&self) -> &str { "加载并执行指定的 Skill" }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Skill 名称" }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let name = params["name"].as_str().unwrap();
        let skill = ctx.skill_registry.find(name)?;
        // 展开变量 + 返回内容
        Ok(ToolResult::new(skill.expand_for_injection()))
    }
}
```

**条件激活**（借鉴 Claude Code）：

```rust
struct ConditionalSkill {
    name: String,
    paths: Vec<String>,       // gitignore 风格模式
    command: Skill,
}

fn activate_conditional_skills(file_paths: &[PathBuf], cwd: &Path) -> Vec<String> {
    let mut activated = Vec::new();
    for skill in &mut conditional_skills {
        let matcher = GitignoreMatcher::new(&skill.paths);
        for path in file_paths {
            if matcher.matches(path, cwd) {
                dynamic_skills.insert(skill.name.clone(), skill.clone());
                activated.push(skill.name.clone());
                break;
            }
        }
    }
    activated
}
```

#### Phase 3（P2）：高级特性

**变量展开**（借鉴 AtomCode）：

```rust
fn expand_template(template: &str, args: &[String], skill_dir: &Path, session_id: &str) -> String {
    let mut result = template.to_string();
    // $1, $2, ... 位置参数
    for (i, arg) in args.iter().enumerate() {
        result = result.replace(&format!("${}", i + 1), arg);
    }
    // $ARGUMENTS 全量参数
    result = result.replace("$ARGUMENTS", &args.join(" "));
    // ${LAEW_SKILL_DIR} Skill 目录
    result = result.replace("${LAEW_SKILL_DIR}", &skill_dir.to_string_lossy());
    // ${LAEW_SESSION_ID} 会话 ID
    result = result.replace("${LAEW_SESSION_ID}", session_id);
    result
}
```

**多 Agent Skill 隔离**（借鉴 deepseek-harness 的 ScopedLayers）：

laew 的 6 个 Agent 角色（Yolo/Plan/Main-Work/SubAgent-Work/Quality-Check/SessionContext）各自需要不同的 Skill 集合：
- **Yolo Agent**：任务分类/意图识别相关 Skill
- **Plan Agent**：方案模板 Skill
- **Main-Work Agent**：工作流编排 Skill
- **SubAgent-Work Agent**：代码执行/部署 Skill（全集）
- **Quality-Check Agent**：验证 checklist Skill
- **SessionContext Agent**：摘要模板 Skill

```rust
pub struct AgentSkillRegistry {
    /// 全局 Skill（所有 Agent 共享）
    global: Vec<Skill>,
    /// 每个 Agent 类型的 Skill 集合
    per_agent: HashMap<AgentRole, Vec<Skill>>,
}

impl AgentSkillRegistry {
    pub fn skills_for(&self, role: AgentRole) -> Vec<&Skill> {
        let mut skills = self.global.iter().collect::<Vec<_>>();
        if let Some(agent_skills) = self.per_agent.get(&role) {
            skills.extend(agent_skills.iter());
        }
        skills
    }
}
```

**Prompt 预算管理**（借鉴 OpenClaw）：

```rust
const MAX_SKILLS_IN_PROMPT: usize = 50;
const MAX_SKILLS_PROMPT_CHARS: usize = 8_000;

fn prepare_skills_for_prompt(skills: &[Skill]) -> String {
    // 1. 先尝试 full 格式
    let full = format_skills_full(skills);
    if full.len() <= MAX_SKILLS_PROMPT_CHARS { return full; }
    // 2. 超预算 → compact 格式（描述截断）
    let compact = format_skills_compact(skills, 220);
    if compact.len() <= MAX_SKILLS_PROMPT_CHARS { return compact; }
    // 3. 仍超 → 二分查找最大技能数
    let max_count = binary_search_max_skills(skills, MAX_SKILLS_PROMPT_CHARS);
    format_skills_compact(&skills[..max_count], 220)
}
```

### 4.3 与现有架构的集成点

| 集成点 | 当前状态 | Skill 集成后 |
|--------|---------|-------------|
| `system_prompt/mod.rs::build()` | 静态拼接基础 + 工具说明 + 协议尾缀 | 追加 Skill 目录注入段 |
| `agent/tools/mod.rs::builtin_registry()` | Bash/Read/Write 三工具 | 追加 UseSkillTool / ListSkillsTool |
| `agent/profile.rs::AgentProfile` | 名称/系统提示词/工具集 | 追加 `skill_dirs: Vec<PathBuf>` 字段 |
| `agent/yolo.rs::YoloRunner` | 任务分类 → 路由 | 分类结果可影响 Skill 激活 |
| `config/mod.rs::Paths::detect()` | 根目录/工作目录检测 | 追加 `skills_dir()` 方法 |
| `session.rs::Session` | Session ID + 对话上下文 | Skill 变量展开需要 Session ID |

### 4.4 内置 Skill 建议

laew 首批内置 Skill 建议：

| Skill 名称 | 用途 | 目标 Agent |
|-----------|------|-----------|
| `deploy-staging` | 部署到 staging 环境 | SubAgent-Work |
| `code-review` | 代码审查 checklist | Quality-Check |
| `git-workflow` | Git 工作流规范（branch/commit/PR） | SubAgent-Work |
| `rust-best-practices` | Rust 编码最佳实践 | SubAgent-Work |
| `test-strategy` | 测试策略模板 | Quality-Check |
| `plan-template` | 方案模板（5W1H） | Plan Agent |
| `summary-template` | 会话摘要模板 | SessionContext |
| `task-classification` | 任务分类指引 | Yolo Agent |

### 4.5 优先级与工作量估计

| 优先级 | 内容 | 预估工作量 | 核心文件 |
|--------|------|-----------|---------|
| P0 | Skill 定义 + Frontmatter 解析 + 目录扫描 | 2-3 天 | 新增 `src/agent/skill/` 模块 |
| P0 | 系统提示词注入 | 1 天 | 修改 `src/agent/system_prompt/mod.rs` |
| P1 | UseSkillTool / ListSkillsTool | 1-2 天 | 新增 `src/agent/tools/skill.rs` |
| P1 | 条件激活（paths 字段） | 1 天 | `src/agent/skill/conditional.rs` |
| P2 | 变量展开 + Shell 预执行 | 1-2 天 | `src/agent/skill/expand.rs` |
| P2 | 多 Agent Skill 隔离 | 1 天 | `src/agent/skill/registry.rs` |
| P2 | Prompt 预算管理 | 1 天 | `src/agent/skill/budget.rs` |
| P3 | 内置 Skill 编译 | 2 天 | `src/agent/skill/bundled/` |
| P3 | 热更新（文件变更检测） | 1 天 | `src/agent/skill/watcher.rs` |

**总估计**：P0 约 3-4 天，P0+P1 约 6-8 天，P0+P1+P2 约 10-13 天。

### 4.6 关键决策点

1. **Skill 调用方式**：建议采用"系统提示词目录注入 + UseSkillTool 显式调用"双轨制（对齐 Claude Code / pi），而非仅靠模型自行发现
2. **变量展开**：建议至少支持 `$N` 位置参数和 `${LAEW_SKILL_DIR}` / `${LAEW_SESSION_ID}` 变量（对齐 AtomCode）
3. **Shell 预执行**：`!`cmd`` 功能强大但有安全风险，建议 P2 再引入，且仅限受信 Skill
4. **Fork 子 Agent**：laew 已有 SubAgent-Work 架构，可以考虑让 `context: fork` 的 Skill 通过 SubAgent-Work 执行
5. **Workshop 自演化**：OpenClaw 的 Workshop 是最前沿设计，但复杂度高（5 阶段流水线），建议作为 P3 远期目标

---

## 五、Skill 系统设计决策矩阵

| 决策项 | 选项 A | 选项 B | 推荐 | 理由 |
|--------|--------|--------|------|------|
| 定义格式 | JSON/YAML 配置 | Markdown + Frontmatter | Markdown + Frontmatter | 6/6 项目共识，人类可读，Git 友好 |
| 发现方式 | 注册表手动注册 | 文件系统扫描 | 文件系统扫描 | 零配置，约定优于配置 |
| Prompt 注入 | 内联全文 | XML 目录 + 模型按需读取 | XML 目录 + 按需读取 | 节省 token，模型自主决策 |
| 参数传递 | 无参数 | 位置参数 + 变量 | 位置参数 + 变量 | 通用性更强 |
| Shell 预执行 | 不支持 | 支持 `!`cmd`` | P2 支持 | 功能强大但需安全边界 |
| 条件激活 | 始终加载 | paths 字段匹配 | paths 字段 | 减少无关 Skill 的 token 占用 |
| 多 Agent 隔离 | 共享 Skill 集 | 每 Agent 独立集 | 每 Agent 独立集 | laew 6 角色职责分明 |
| 内置 Skill | 外部加载 | 编译进二进制 | 编译进二进制 | Rust 项目天然优势 |
| 自演化 | 无 | Workshop 流水线 | P3 远期 | OpenClaw 证明可行但复杂度高 |
| Prompt 预算 | 无限制 | 渐进降级 | 渐进降级 | 防止 prompt 膨胀 |

---

## 六、总结

### 6.1 核心发现

1. **Skill = Markdown + Frontmatter** 是 6 个项目的行业共识，laew 应直接采用
2. **XML Prompt 注入**是最成熟的注入模式（Claude Code / OpenClaw / pi 都采用）
3. **模型自主决策 + 显式调用双轨**是最灵活的调用方式
4. **条件激活**（Claude Code）和 **Prompt 预算管理**（OpenClaw）是控制 token 消耗的关键技术
5. **Workshop 自演化**（OpenClaw）是最前沿的设计，将 Skill 从静态知识升级为可自我优化的智能体
6. **Layered Registry**（deepseek-harness）是最灵活的扩展架构，适合多租户场景

### 6.2 laew 的 Skill 系统定位

laew 的 Skill 系统应定位为**知识注入层**，而非工具扩展层：
- Skill 是告诉模型"怎么做"的指令，不是"能做什么"的 API
- Skill 与 Tool 互补：Tool 提供执行能力，Skill 提供流程知识
- Skill 与现有系统提示词互补：系统提示词定义角色和规则，Skill 定义具体任务的工作流

### 6.3 借鉴优先级

| 借鉴来源 | 借鉴内容 | 优先级 |
|---------|---------|--------|
| pi | 极简 Skill 类型 + XML 注入 + 双路径调用 | P0 |
| Claude Code | Frontmatter 字段设计 + 条件激活 + 权限检查 | P0-P1 |
| AtomCode | 变量展开 + Shell 预执行 + 弱模型增强 | P1-P2 |
| OpenClaw | Prompt 预算管理 + Workshop 自演化 | P2-P3 |
| deepseek-harness | Layered Registry + Provider 模式 + ScopedLayers | P2 |
| OpenCode | 远程 Skill 拉取 + Prune 保护 | P3 |

---

---

## 七、Skill 系统架构模式详解

### 7.1 模式一：文件系统扫描 + 缓存（所有项目共用）

这是 Skill 系统最基础的架构模式，所有 6 个项目都采用。

```
┌─────────────────────────────────────────────────────────────────┐
│                    Skill 发现与加载流程                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  启动时                                                          │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐  │
│  │ 全局目录  │    │ 项目目录  │    │ 内置目录  │    │ 远程URL  │  │
│  │~/.laew/   │    │.laew/    │    │编译时注册 │    │(OpenCode)│  │
│  └────┬─────┘    └────┬─────┘    └────┬─────┘    └────┬─────┘  │
│       │               │               │               │        │
│       ▼               ▼               ▼               ▼        │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              并行扫描 + Frontmatter 解析                  │   │
│  │  scan(dir) → glob("SKILL.md" / "*.md") → parse(yaml)   │   │
│  └──────────────────────────┬──────────────────────────────┘   │
│                             │                                   │
│                             ▼                                   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              去重 + 优先级排序                             │   │
│  │  去重策略：realpath(ClaudeCode) / name(OpenCode)         │   │
│  │  优先级：用户home > 项目 > 插件(AtomCode)                │   │
│  │  优先级：Bundled > UserAgents > UserDSH > ... (deepseek) │   │
│  └──────────────────────────┬──────────────────────────────┘   │
│                             │                                   │
│                             ▼                                   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              缓存 + 热更新                                │   │
│  │  memoize(ClaudeCode) / invalidateCache(deepseek)        │   │
│  │  文件变更检测(ClaudeCode) / 无缓存(pi)                   │   │
│  └──────────────────────────┬──────────────────────────────┘   │
│                             │                                   │
│                             ▼                                   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              Skill 集合（内存态）                          │   │
│  │  Vec<Skill> / Map<name, SkillDefinition>                │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 7.2 模式二：Provider 注册表（deepseek-harness 独有）

deepseek-harness 的 Skill 系统采用 Provider 模式，将 Skill 的发现和加载抽象为可插拔的 Provider 接口：

```
┌─────────────────────────────────────────────────────────────────┐
│                SkillRegistry (Provider 模式)                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                   ScopedLayers                           │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐                 │   │
│  │  │ Global  │  │ Scope A │  │ Scope B │  ...             │   │
│  │  │ Layer   │  │ Layer   │  │ Layer   │                  │   │
│  │  ├─────────┤  ├─────────┤  ├─────────┤                 │   │
│  │  │providers│  │providers│  │providers│                  │   │
│  │  │runtime  │  │runtime  │  │runtime  │                  │   │
│  │  └─────────┘  └─────────┘  └─────────┘                 │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  Provider 接口:                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │  Filesystem  │  │   Runtime    │  │   Custom     │          │
│  │  Provider    │  │   Provider   │  │   Provider   │          │
│  │              │  │              │  │              │          │
│  │ scan(dir)   │  │ register()   │  │ 自定义逻辑   │          │
│  │ get(file)   │  │ get(name)    │  │              │          │
│  └──────────────┘  └──────────────┘  └──────────────┘          │
│                                                                 │
│  优先级排序:                                                     │
│  Bundled(600) > UserAgents(500) > UserDSH(400) > Custom(300)   │
│  > Runtime(250) > ProjectAgents(200) > ProjectDSH(100)          │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**设计优势**：
- 新增 Skill 来源只需实现 `SkillProvider` 接口并注册，无需修改核心逻辑
- `ScopedLayers` 支持 per-scope 的 Skill 隔离，子 Agent 可拥有独立 Skill 集合
- 优先级系统确保同名 Skill 不会冲突

### 7.3 模式三：双轨调用（pi / Claude Code）

```
┌─────────────────────────────────────────────────────────────────┐
│                    Skill 双轨调用模式                             │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  路径 A：模型自主发现（Passive）                                  │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐  │
│  │系统提示词 │───>│模型阅读   │───>│模型判断   │───>│read 工具 │  │
│  │注入目录   │    │Skill描述  │    │是否匹配   │    │读取全文  │  │
│  └──────────┘    └──────────┘    └──────────┘    └──────────┘  │
│                                                                 │
│  路径 B：显式调用（Active）                                      │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐  │
│  │用户输入   │───>│SkillTool │───>│查找Skill │───>│展开+注入 │  │
│  │/skill xxx │    │或Lane    │    │Frontmatter│    │到消息流  │  │
│  └──────────┘    └──────────┘    └──────────┘    └──────────┘  │
│                                                                 │
│  关键区别:                                                       │
│  - 路径 A：模型决定是否使用，无 tool call 开销                     │
│  - 路径 B：确定性调用，可传递参数，可请求权限确认                   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 7.4 模式四：Workshop 自演化流水线（OpenClaw 独有）

```
┌─────────────────────────────────────────────────────────────────┐
│                    Workshop 自演化流水线                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐  │
│  │ History  │───>│Experience│───>│ Proposal │───>│Autonomous│  │
│  │ Scan     │    │ Review   │    │Generation│    │  Apply   │  │
│  └──────────┘    └──────────┘    └──────────┘    └──────────┘  │
│       │               │               │               │        │
│       ▼               ▼               ▼               ▼        │
│  扫描会话历史    后台Agent评审    生成修改提案    自动/人工审核   │
│  发现改进机会    使用体验分析    原子写入draft   workshop可自动   │
│  游标分页持久    过滤非相关会话  状态机管理      user需人工审核   │
│                                                                 │
│       ┌──────────────────────────────────────────────┐         │
│       │           Collection Plan                     │         │
│       │  验证写/删/原因完整性                          │         │
│       │  每个Agent至少保留一个可见Skill                │         │
│       └──────────────────────────────────────────────┘         │
│                                                                 │
│  安全机制:                                                       │
│  - revisionHash + expectedRevisionHash (乐观并发控制)             │
│  - workshop-owned 技能可自动修改，user-authored 需审核            │
│  - Mutation Budget (run 级预算控制)                               │
│  - Proposal ID 格式校验 + 附件 hash 校验                         │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## 八、Skill 系统的演进路径分析

### 8.1 行业演进时间线

Skill 系统在 Agent 生态中的演进可以划分为三个阶段：

| 阶段 | 时间 | 代表项目 | 核心特征 | 设计哲学 |
|------|------|---------|---------|---------|
| **V1: 静态指令** | 2024 早期 | pi | Markdown 文件 + 系统提示词注入 | "No MCP. Build CLI tools with READMEs" |
| **V2: 动态框架** | 2024 中期 | Claude Code / AtomCode | Frontmatter 元数据 + 工具调用 + 条件激活 | Skill 是可编程的指令模板 |
| **V3: 自演化** | 2025 | OpenClaw | Workshop 流水线 + 经验学习 + 自动改进 | Skill 是可自我优化的智能体知识 |

**关键转折点**：
- V1 -> V2：从"纯文本"到"结构化元数据"，Frontmatter 引入了 `allowed-tools`、`context`、`paths` 等控制字段
- V2 -> V3：从"人工维护"到"自动演化"，OpenClaw 的 Workshop 让 Skill 从使用经验中自动改进

### 8.2 各项目的 Skill 系统成熟度评估

| 项目 | 成熟度 | 核心能力 | 缺失能力 |
|------|--------|---------|---------|
| Claude Code | L4（生产级） | 完整生命周期管理（发现/加载/激活/调用/追踪/热更新/权限） | 自演化 |
| OpenClaw | L4+（前沿） | 52 内置 + Workshop 自演化 + Prompt 预算 + 版本兼容 | 条件激活、参数展开 |
| deepseek-harness | L3（架构级） | Layered Registry + Provider + ScopedLayers + 优先级 | 条件激活、Shell 预执行、自演化 |
| AtomCode | L3（Rust 实现） | Frontmatter + 变量展开 + Shell 预执行 + 命名空间 + 弱模型增强 | 条件激活、自演化、Prompt 预算 |
| OpenCode | L3（双轨制） | 远程拉取 + Prune 保护 + Effect DI | 条件激活、自演化、参数展开 |
| pi | L2（极简级） | 纯文本注入 + 双路径调用 + Ignore 规则 + PromptTemplate | 条件激活、参数展开、工具调用、自演化 |

**成熟度等级定义**：
- L1（概念级）：仅定义了 Skill 类型和文件格式
- L2（可用级）：支持发现、加载、注入，可实际使用
- L3（架构级）：有完整的注册/发现/调用架构，支持扩展
- L4（生产级）：有权限控制、预算管理、热更新、使用追踪等生产特性
- L5（前沿级）：有自演化、远程分发、多租户隔离等前沿特性

### 8.3 laew 的 Skill 系统演进路线图

```
Phase 1 (P0)         Phase 2 (P1)         Phase 3 (P2)         Phase 4 (P3)
基础框架              工具集成              高级特性              前沿探索
───────────────────────────────────────────────────────────────────────────
Skill 定义            UseSkillTool          变量展开              Workshop 自演化
Frontmatter 解析      ListSkillsTool        Shell 预执行          远程 Skill 拉取
目录扫描              条件激活              多 Agent 隔离         Skill Marketplace
XML Prompt 注入       权限检查              Prompt 预算管理       社区 Skill 共享
内置 Skill            热更新                Prune 保护            Skill 版本管理
```

---

## 九、Skill 安全模型深度分析

### 9.1 安全威胁面

| 威胁 | 描述 | 受影响项目 | 防御措施 |
|------|------|-----------|---------|
| **Shell 注入** | `!`cmd`` 执行任意命令 | AtomCode、Claude Code | 区分受信/非受信 Skill |
| **Prompt 注入** | Skill 正文包含恶意指令覆盖 | 所有项目 | XML escape、内容校验 |
| **路径遍历** | `${CLAUDE_SKILL_DIR}/../../../etc/passwd` | 有变量展开的项目 | 路径规范化 + 白名单 |
| **权限提升** | Skill 声明 `allowed-tools: [Bash(rm:*)]` | Claude Code | deny 规则优先 + 用户确认 |
| **供应链攻击** | 远程拉取的 Skill 被篡改 | OpenCode | hash 校验 + 签名验证 |
| **Token 耗尽** | 大量 Skill 注入耗尽上下文窗口 | 所有项目 | Prompt 预算管理 |

### 9.2 各项目安全策略对比

| 安全策略 | Claude Code | AtomCode | OpenClaw | OpenCode | deepseek-harness | pi |
|---------|-------------|----------|----------|----------|-----------------|-----|
| 受信模型 | SAFE_SKILL_PROPERTIES 白名单 | allowed-tools 白名单 | 无 | permission: "skill" | 无 | 无 |
| 非受信隔离 | fork 子 Agent | 无 | 无 | 无 | 无 | 无 |
| 权限检查 | deny/allow 规则 + 前缀匹配 | 工具级白名单 | 无 | ask 确认 | 无 | 无 |
| 内容校验 | skillHasOnlySafeProperties | 无 | 安装 spec 校验 | 无 | 无 | 无 |
| 路径安全 | O_NOFOLLOW\|O_EXCL 防符号链接 | 无 | URL 仅 http/https | 无 | 无 | 无 |
| Prompt 注入防御 | XML escape | 无 | 无 | 无 | 无 | XML escape |

### 9.3 laew 安全建议

**P0 安全措施**（基础框架阶段必须实现）：

1. **XML 转义**：所有注入到系统提示词的 Skill 字段必须经过 XML escape，防止 `<available_skills>` 结构被破坏
2. **路径规范化**：Skill 文件路径必须规范化（canonicalize），防止 `../` 遍历攻击
3. **文件大小限制**：单个 Skill 文件上限（建议 50KB），防止恶意大文件耗尽内存
4. **Frontmatter 校验**：必填字段（name、description）缺失时跳过，不报错中断

**P1 安全措施**（工具集成阶段）：

1. **allowed-tools 白名单**：Skill 声明的工具权限必须经过校验，不在白名单内的工具调用被拒绝
2. **用户确认**：包含 `allowed-tools` 或 `hooks` 的 Skill 首次调用时需要用户确认（借鉴 Claude Code 的 `SAFE_SKILL_PROPERTIES`）
3. **Skill 来源标记**：内置 Skill 标记为 `trusted`，用户 Skill 标记为 `untrusted`，区别对待

**P2 安全措施**（高级特性阶段）：

1. **Shell 预执行沙箱**：`!`cmd`` 执行必须在受限环境中，禁止 `rm -rf`、`sudo`、网络请求等危险操作
2. **远程 Skill 签名验证**：远程拉取的 Skill 必须验证签名或 hash
3. **Skill 使用审计**：记录每次 Skill 调用的输入/输出/工具调用，用于安全审计

---

## 十、Skill 与 MCP 的关系辨析

### 10.1 两种范式的本质区别

| 维度 | MCP Tool | Skill |
|------|----------|-------|
| **本质** | 可执行的 API 端点 | 可阅读的指令文档 |
| **定义** | JSON Schema（parameters + execute） | Markdown + Frontmatter（纯文本） |
| **执行** | MCP Server 进程处理请求 | 模型阅读后用现有工具执行 |
| **参数** | JSON arguments，需验证 | 无参数，或文本变量替换 |
| **适用场景** | 复杂 API 集成、外部服务调用 | 工作流 checklist、最佳实践、模板 |
| **错误处理** | 结构化错误码 | 模型自行处理 |
| **权限控制** | 工具级权限 | Skill 级权限（allowed-tools） |

### 10.2 pi 的设计哲学（最鲜明的对比）

pi 项目明确提出了"MCP vs Skill"的设计选择：

> "No MCP. Build CLI tools with READMEs."

这意味着：
- MCP 本质是**工具协议**（JSON-RPC + tool schema + runtime execution）
- Skill 本质是**知识注入**（Markdown + frontmatter -> 系统提示词）
- Skill 没有执行层，不产生 tool call，不需要参数验证
- Skill 的"执行"是模型阅读指令后自行使用已有的 bash/read/edit/write 工具完成任务

**核心洞察**：Skill 特别适合**流程性知识**（如"添加新 LLM provider 的 7 步 checklist"），这类任务不需要新的 API 端点，只需要告诉模型怎么做。MCP 在这种场景下是过度工程化。

### 10.3 Claude Code 的桥接设计

Claude Code 同时支持 MCP 和 Skill，并通过 `mcpSkillBuilders.ts` 实现桥接：

```typescript
// mcpSkillBuilders.ts - 解决循环依赖的关键设计
// loadSkillsDir.ts → mcpSkillBuilders.ts ← mcpSkills.ts
// 写入一次注册模式：loadSkillsDir.ts 模块初始化时注册
// 运行时通过 getMCPSkillBuilders() 获取

// MCP prompts 作为 loadedFrom: 'mcp' 的 Skill 注册
const mcpSkills = context.getAppState().mcp.commands
  .filter(cmd => cmd.type === 'prompt' && cmd.loadedFrom === 'mcp');
```

这说明 MCP 的 `prompts` 能力可以映射为 Skill，两者不是互斥的。

### 10.4 laew 的选择建议

laew 当前无 MCP、无 Skill。建议的引入顺序：

1. **先引入 Skill**（P0）：成本低（纯文件解析），收益高（知识注入），与现有架构兼容
2. **后引入 MCP**（P2+）：成本高（需要 MCP 客户端/服务端），收益在外部工具集成
3. **桥接设计**：如果未来同时支持两者，参考 Claude Code 的 `mcpSkillBuilders` 模式，将 MCP prompts 映射为 Skill

---

## 十一、Skill 在压缩/裁剪中的行为

### 11.1 各项目的处理方式

| 项目 | 压缩（Compaction） | 裁剪（Prune） | Skill 恢复 |
|------|-------------------|--------------|-----------|
| Claude Code | 4 级压缩管线 | 工具输出裁剪 | `POST_COMPACT_SKILLS_TOKEN_BUDGET = 25,000` tokens |
| AtomCode | CompactionStrategy trait | NoCompaction 默认 | 无特殊处理 |
| OpenClaw | 内置 compaction + 可插拔 ContextEngine | 无 | Skill Snapshot 重新构建 |
| OpenCode | 自动 compaction | `PRUNE_PROTECTED_TOOLS = ["skill"]` | Skill 工具输出永不裁剪 |
| deepseek-harness | BasicCompactionEngine 插件 | 无 | 无特殊处理 |
| pi | 无 compaction | 无 | 无特殊处理 |

### 11.2 关键设计洞察

1. **Claude Code 的压缩后恢复**是最精细的设计：Skill 有独立的 token 预算（25K），单个 Skill 上限 5K，确保压缩后关键 Skill 内容不会丢失
2. **OpenCode 的 Prune 保护**是最简单有效的设计：`PRUNE_PROTECTED_TOOLS = ["skill"]` 一行代码确保 Skill 输出在上下文中始终保留
3. **laew 当前无 compaction**（依赖模型上下文窗口），但如果未来引入 compaction，Skill 的恢复策略必须在设计阶段考虑

### 11.3 laew 建议

即使 laew 当前无 compaction，也建议在 Skill 系统设计中预留压缩感知接口：

```rust
/// Skill 压缩策略
pub enum SkillCompactionStrategy {
    /// 始终保留（默认）
    Always,
    /// 按 token 预算保留
    TokenBudget { max_total: usize, max_per_skill: usize },
    /// 仅保留最近使用的 N 个
    RecentUsed(usize),
}

impl Skill {
    pub fn compaction_strategy(&self) -> SkillCompactionStrategy {
        SkillCompactionStrategy::Always // 默认：Skill 输出永不裁剪
    }
}
```

---

## 十二、Skill 的 Prompt 工程最佳实践

### 12.1 Prompt 注入格式对比

**XML 格式**（Claude Code / OpenClaw / pi 采用）：

```xml
<available_skills>
  <skill>
    <name>deploy-staging</name>
    <description>Deploy current branch to staging environment</description>
    <location>/home/user/.laew/skills/deploy-staging.md</location>
  </skill>
</available_skills>
```

优势：结构清晰，模型容易解析，支持嵌套，escapeXml 防注入。

**纯文本格式**（AtomCode 采用）：

```
=== AVAILABLE SKILLS ===
deploy-staging: Deploy current branch to staging environment
  Location: /home/user/.atomcode/skills/deploy-staging.md
code-review: Review code changes with checklist
  Location: /home/user/.atomcode/skills/code-review.md
```

优势：token 消耗更少，人类可读性更好。

**推荐**：laew 建议采用 XML 格式（对齐 4/6 项目），在系统提示词中使用 `<available_skills>` 标签。理由：
1. XML 标签结构清晰，模型（尤其是 Claude/GPT 系列）解析准确率高
2. escapeXml 是成熟的安全实践
3. 未来如果需要嵌套属性（如 `<metadata>`），XML 天然支持

### 12.2 系统提示词中的 Skill 引导语

**pi 的引导语**（最简洁）：

```
The following skills provide specialized instructions for specific tasks.
Read the full skill file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory
(parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.
```

**Claude Code 的引导语**（更详细）：

系统提示词还会包含 Skill 的使用规则、权限约束、变量展开说明等。

**laew 建议引导语**：

```
以下 Skill 提供特定任务的专业指令。当任务匹配 Skill 描述时，使用 read 工具读取完整内容并遵循执行。
Skill 文件中的相对路径基于 Skill 所在目录解析。
使用 use_skill 工具可以显式加载指定 Skill。
```

### 12.3 Skill 正文写作规范

基于 6 个项目中实际 Skill 文件的分析，总结最佳实践：

| 规范 | 说明 | 示例 |
|------|------|------|
| **结构化步骤** | 使用编号列表，每步一个原子操作 | `1. Run tests: cargo test` |
| **相对路径** | 使用相对路径，系统自动解析 | `scripts/deploy.sh` 而非 `/absolute/path` |
| **错误处理** | 每步包含失败时的回退策略 | `If tests fail, stop and report errors` |
| **验证点** | 关键步骤后添加验证 | `4. Verify: curl http://staging.example.com/health` |
| **变量占位** | 使用 `$1`、`$ARGUMENTS` 等占位符 | `Deploy to environment: $1` |
| **代码块** | 命令使用代码块包裹 | `` `cargo build --release` `` |
| **条件分支** | 使用 if/else 描述分支逻辑 | `If branch is main, deploy to production; else deploy to staging` |

---

## 十三、附录

### 附录 A：关键源文件索引

| 项目 | 关键文件 | 行数 | 职责 |
|------|---------|------|------|
| Claude Code | `src/skills/loadSkillsDir.ts` | 1087 | Skill 发现、加载、解析 |
| Claude Code | `src/tools/SkillTool/SkillTool.ts` | 1109 | Skill 工具（fork/inline） |
| Claude Code | `src/skills/bundledSkills.ts` | 221 | 内置 Skill 注册框架 |
| Claude Code | `src/skills/mcpSkillBuilders.ts` | 45 | MCP Skill 桥接 |
| AtomCode | `atomcode-capabilities/src/skills/skill.rs` | 92 | Skill 定义与展开 |
| AtomCode | `atomcode-capabilities/src/skills/use_skill.rs` | 127 | use_skill / list_skills 工具 |
| AtomCode | `atomcode-capabilities/src/skills/catalog_hook.rs` | ~50 | SkillCatalogHook |
| AtomCode | `atomcode-capabilities/src/skills/registry.rs` | ~100 | SkillRegistry |
| OpenClaw | `src/skills/skill-contract.ts` | ~120 | Skill 类型与 Prompt 渲染 |
| OpenClaw | `src/skills/skill-prompt-limits.ts` | ~190 | Prompt 预算管理 |
| OpenClaw | `src/skills/workspace-skill-prompt.ts` | ~150 | Skill Snapshot |
| OpenClaw | `src/skills/workshop/` | ~2000+ | Workshop 自演化（5 个子模块） |
| OpenCode | `skill/index.ts` | 354 | Skill Service（发现/加载/查询） |
| OpenCode | `tool/skill.ts` | 70 | SkillTool（运行时注入） |
| OpenCode | `skill/discovery.ts` | 140 | 远程 Skill 拉取 |
| deepseek-harness | `packages/skill/skill/src/index.ts` | 869 | SkillRegistry |
| deepseek-harness | `packages/skill/skill/src/rank.ts` | ~50 | SkillRank 优先级 |
| deepseek-harness | `packages/skill/skill-filesystem/src/index.ts` | ~100 | 文件系统 Provider |
| pi | `packages/agent/src/harness/types.ts` | ~70 | Skill 接口定义 |
| pi | `packages/agent/src/harness/skills.ts` | ~350 | Skill 发现、加载、解析 |
| pi | `packages/agent/src/harness/system-prompt.ts` | ~50 | formatSkillsForSystemPrompt |
| pi | `packages/agent/src/harness/agent-harness.ts` | ~500 | AgentLane.skill() |

### 附录 B：术语表

| 术语 | 定义 | 首次出现 |
|------|------|---------|
| Skill | 可被 Agent 读取和遵循的结构化指令文档 | pi |
| Frontmatter | YAML 格式的文件元数据头部（`---` 包围） | 所有项目 |
| SKILL.md | 每个目录显式声明的 Skill 文件名 | pi / OpenClaw |
| SkillTool | 将 Skill 暴露为可调用工具的适配器 | Claude Code |
| SkillProvider | Skill 来源的抽象接口（文件系统/远程/运行时） | deepseek-harness |
| SkillRegistry | Skill 注册表，管理发现/加载/查询 | AtomCode / deepseek-harness |
| SkillSnapshot | Skill 的快照，用于 Prompt 注入 | OpenClaw |
| Workshop | Skill 自演化流水线（5 阶段） | OpenClaw |
| ConditionalSkill | 条件激活的 Skill（按文件路径匹配） | Claude Code |
| PromptTemplate | 与 Skill 平行的模板注入路径（支持 `$@` 参数） | pi |
| ScopedLayers | 分层的作用域隔离机制 | deepseek-harness |
| SkillRank | Skill 优先级枚举 | deepseek-harness |
| SkillCompaction | 上下文压缩时 Skill 内容的恢复策略 | Claude Code / OpenCode |

### 附录 C：6 项目的 Skill 系统代码量对比

| 项目 | Skill 相关代码行数 | 占总代码比例 | 评价 |
|------|-------------------|------------|------|
| Claude Code | ~3,500 行 | ~1.6%（总 218K） | 精炼，每个文件职责单一 |
| OpenClaw | ~3,000+ 行 | ~0.15%（总 201 万行） | Workshop 子系统复杂度高 |
| deepseek-harness | ~1,100 行 | ~1.4%（总 80K） | 架构清晰，Provider 模式优雅 |
| AtomCode | ~400 行 | ~0.27%（总 150K） | Rust 实现，精简高效 |
| OpenCode | ~600 行 | ~3.3%（总 18K） | Effect DI 风格，函数式 |
| pi | ~450 行 | ~0.7%（总 65K） | 极简设计，最少代码覆盖最多场景 |

### 附录 D：Skill 系统设计 Checklist（laew 引入用）

- [ ] **定义格式**：Markdown + YAML Frontmatter
- [ ] **Frontmatter 字段**：name、description、allowed-tools、user-invocable、model-invocable
- [ ] **发现路径**：`~/.laew/skills/` + 项目 `.laew/skills/` + 内置
- [ ] **文件命名**：`SKILL.md`（目录级）+ `*.md`（扁平）
- [ ] **Prompt 注入**：XML `<available_skills>` 格式
- [ ] **系统提示词集成**：在 `system_prompt/mod.rs::build()` 中追加
- [ ] **UseSkillTool**：暴露为可调用工具
- [ ] **ListSkillsTool**：列出所有可用 Skill
- [ ] **多 Agent 隔离**：每个 Agent 角色有独立 Skill 集合
- [ ] **Prompt 预算**：`MAX_SKILLS_IN_PROMPT` + `MAX_SKILLS_PROMPT_CHARS` 渐进降级
- [ ] **安全**：XML escape + 路径规范化 + 文件大小限制
- [ ] **内置 Skill**：编译进二进制（`include_str!`）
- [ ] **变量展开**：`$N` 位置参数 + `${LAEW_SKILL_DIR}` + `${LAEW_SESSION_ID}`
- [ ] **单元测试**：Frontmatter 解析 + 目录扫描 + Prompt 注入 + 变量展开
- [ ] **端到端测试**：`run_e2e.sh` 增加 Skill 相关用例

### 附录 E：Skill 系统测试策略

| 测试类型 | 测试内容 | 测试方法 |
|---------|---------|---------|
| **单元测试** | Frontmatter 解析正确性 | `#[test]` + 各种 YAML 格式 |
| **单元测试** | 变量展开正确性 | `#[test]` + 边界用例（空参数、特殊字符） |
| **单元测试** | Prompt 注入格式 | `#[test]` + XML 结构校验 |
| **单元测试** | 路径规范化 | `#[test]` + `../`、符号链接、绝对路径 |
| **集成测试** | Skill 发现 + 加载 | 创建临时目录结构，验证扫描结果 |
| **集成测试** | 条件激活 | 创建临时文件，验证 `paths` 匹配 |
| **端到端** | Skill 调用流程 | mock LLM + Skill 文件 + 验证工具调用 |
| **端到端** | Prompt 预算降级 | 创建大量 Skill，验证渐进降级 |
| **安全测试** | XML 注入防御 | 包含 `<script>` 等恶意内容的 Skill |
| **安全测试** | 路径遍历防御 | 包含 `../../../` 的 Skill 路径 |

---

**报告完成日期**：2026-09-04
**分析项目数**：6 个（Claude Code / AtomCode / OpenClaw / OpenCode / deepseek-harness / pi）
**引用文档数**：12 份（各项目的「核心机制深度分析」+「深度分析」）
