# pi 核心机制深度分析（第二轮）

- 分析日期：2026-09-04
- 源码根路径：`/usr/local/LsmGitOpenSource/pi`
- 核心文件数：约 65 个 TypeScript 源文件
- 分析范围：packages/agent、packages/ai、packages/coding-agent 的运行时核心
- 前置文档：`pi-源码调研.md`（880 行，第一轮分析）、`pi-深度分析.md`（对应框架对比文档中 pi 条目）

---

## 专题 1：Skill 系统 —— 一等公民设计

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/agent/src/harness/types.ts` | `Skill` 接口定义 |
| `packages/agent/src/harness/skills.ts` | Skill 发现、加载、解析、格式化 |
| `packages/agent/src/harness/system-prompt.ts` | `formatSkillsForSystemPrompt` —— Skill XML 注入 |
| `packages/agent/src/harness/agent-harness.ts` | `AgentLane.skill()` —— Skill 调用入口 |
| `packages/coding-agent/src/utils/frontmatter.ts` | Frontmatter YAML 解析工具 |
| `.pi/skills/add-llm-provider.md` | 实际 Skill 文件示例 |
| `.pi/prompts/pr.md` | PromptTemplate 示例 |

### 1.1 Skill 核心类型

```typescript
// packages/agent/src/harness/types.ts
export interface Skill {
  name: string;
  description: string;
  content: string;       // Markdown body（完整指令）
  filePath: string;      // 绝对路径，兼做模型可见的 location
  disableModelInvocation?: boolean; // 从系统提示词中隐藏，但仍可手动调用
}
```

设计要点：Skill 不是 tool，没有 `parameters` / `execute` 字段。Skill 是**纯文本指令**，注入系统提示词供模型自行决定是否读取和遵循。这是 pi 与 MCP 的根本分歧点。

### 1.2 Skill 文件格式与解析

Skill 文件有两种命名约定：

- **`SKILL.md`**：每个目录显式声明一个 skill（优先级高，递归时遇到即停止继续扫描子目录）
- **`*.md`**：根目录下带 frontmatter 的普通 Markdown 文件（需有 `description` 字段才会被识别）

Frontmatter 解析逻辑（`parseFrontmatter`）：
```typescript
// packages/agent/src/harness/skills.ts:323
function parseFrontmatter<T>(content: string): Result<{ frontmatter: T; body: string }, Error> {
  // 1. 以 "---" 开头的 YAML 块
  // 2. 用 yaml 库解析
  // 3. body = 剩余 Markdown 内容
}
```

验证规则（`validateName`）：名称必须全小写、仅允许 `a-z0-9-`、不能以 `-` 开头/结尾、不能有连续 `--`、最长 64 字符。**名称必须与父目录名一致**（这是强约束）。

### 1.3 Skill 发现与注册

`loadSkills(env, dirs)` 是入口函数：
1. 遍历目录，遵守 `.gitignore` / `.ignore` / `.fdignore` 规则（使用 `ignore` 库）
2. 优先查找 `SKILL.md`，找到后该目录不再递归
3. 否则扫描根目录 `.md` 文件（带 frontmatter + description）
4. 递归进入子目录
5. 返回 `{ skills: Skill[]; diagnostics: SkillDiagnostic[] }`

`loadSourcedSkills` 是带来源标记的变体，支持按来源（如内置/用户自定义）区分 skill。

### 1.4 Skill 如何被调用

**关键发现**：Skill 不是通过 tool 调用的。存在两种调用路径：

**路径 A：模型自行发现并读取 Skill 文件**
`formatSkillsForSystemPrompt` 将 skill 列表以 XML 格式注入系统提示词：
```xml
<available_skills>
  <skill>
    <name>add-llm-provider</name>
    <description>Checklist for adding a new LLM provider...</description>
    <location>/absolute/path/to/SKILL.md</location>
  </skill>
</available_skills>
```
系统提示词还告诉模型："Read the full skill file when the task matches its description"。模型需要自行决定是否用 `read` 工具去读取 skill 文件的完整内容。

**路径 B：通过 `AgentLane.skill()` 显式调用**
```typescript
// packages/agent/src/harness/agent-harness.ts:368
async skill(_name: string, _additionalInstructions?: string): Promise<RunResult>
```
`formatSkillInvocation` 将 skill 内容包装为带 `<skill>` 标签的消息：
```typescript
const skillBlock = `<skill name="${skill.name}" location="${skill.filePath}">
References are relative to dirname(skill.filePath).
${skill.content}
</skill>`;
```
这条消息被注入用户消息流，触发一轮新的 agent run。

### 1.5 Skill vs Tool vs MCP 的设计哲学

Skill 是**纯文本指令注入**，不是可调用的工具。这体现了 pi 的核心设计信条：

> "No MCP. Build CLI tools with READMEs."

这意味着：
- MCP 本质是**工具协议**（JSON-RPC + tool schema + runtime execution）
- Skill 本质是**知识注入**（Markdown + frontmatter → 系统提示词）
- Skill 没有执行层，不产生 tool call，不需要参数验证
- Skill 的"执行"是模型阅读指令后自行使用已有的 bash/read/edit/write 工具完成任务

这是一种**极简主义**设计：用最少的抽象（Markdown 文件）覆盖最多的场景。

对比三种范式：

| 特性 | MCP Tool | pi Skill | laew Tool |
|------|----------|----------|-----------|
| 定义格式 | JSON Schema | Markdown + YAML | Rust struct + impl Tool |
| 发现方式 | 服务端注册 | 文件系统扫描 | builtin_registry() |
| 调用方式 | JSON-RPC tool call | 模型自行读取遵循 | LLM tool call → execute() |
| 参数传递 | JSON arguments | 无（纯文本） | JSON → serde 反序列化 |
| 运行时执行 | MCP Server 进程 | 无（模型用现有工具） | Rust async fn |
| 适用场景 | 复杂 API 集成 | 知识/流程/checklist | 代码执行/文件操作 |

pi 的 Skill 范式特别适合**流程性知识**（如"添加新 provider 的7步 checklist"），这类任务不需要新的 API 端点，只需要告诉模型怎么做。MCP 在这种场景下是过度工程化。

### 1.6 Skill 的加载细节：Ignore 规则与递归策略

`loadSkillsFromDirInternal` 的递归策略值得深入分析：

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

关键行为：
1. 先扫描当前目录是否有 `SKILL.md`，有则加载并**立即返回**（不再扫描子目录中的 SKILL.md）
2. 如果没有 SKILL.md，继续扫描：跳过 `.` 开头的目录和 `node_modules`
3. 子目录递归时 `includeRootFiles = false`（只有根目录的普通 `.md` 文件被识别为 skill）
4. Ignore 规则从 `.gitignore` / `.ignore` / `.fdignore` 三个文件加载，使用 `ignore` 库（与 gitignore 完全兼容）
5. 路径前缀处理：子目录的 ignore 规则需要加上相对于 rootDir 的前缀

### 1.7 PromptTemplate：与 Skill 平行的另一条注入路径

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

### 1.8 `formatSkillsForSystemPrompt` 的完整渲染

```typescript
// packages/agent/src/harness/system-prompt.ts:3
export function formatSkillsForSystemPrompt(skills: Skill[]): string {
  const visibleSkills = skills.filter((skill) => !skill.disableModelInvocation);
  if (visibleSkills.length === 0) return "";

  const lines = [
    "The following skills provide specialized instructions for specific tasks.",
    "Read the full skill file when the task matches its description.",
    "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.",
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

注意 XML 转义（`escapeXml`）：`&` → `&amp;`，`<` → `&lt;`，`>` → `&gt;`，`"` → `&quot;`，`'` → `&apos;`。这是防御性编程，防止 skill 的 description 或 filePath 包含特殊字符导致 XML 注入。

### 1.9 实际 Skill 文件分析（`add-llm-provider.md`）

```yaml
---
name: add-llm-provider
description: Checklist for adding a new LLM provider to packages/ai...
---
```

这个 skill 文件展示了 pi 的"开发工作流 as Skill"模式：
- 它是一份详细的 checklist（7 个步骤：Core Types → Provider Impl → Exports → Model Gen → Tests → Coding Agent → Docs）
- 模型在接到"添加新 LLM provider"任务时，先读取此文件，然后按步骤执行
- 每个步骤涉及不同目录和文件，模型需要自行使用 read/bash/edit 工具完成

### 对 laew 的借鉴价值

1. **Skill 注入机制值得借鉴**：laew 的 SystemPrompt 目前是静态拼接，可以考虑将"项目说明"和"任务指引"做成类似 Skill 的动态注入层。
2. **Frontmatter + Markdown 格式**：比纯 Rust 字符串更易维护，laew 可以用类似格式管理 system prompt 模板。
3. **但 laew 的多 Agent 架构需要不同设计**：laew 的 Yolo/Plan/Main-Work 各有不同职责，不适合统一注入同一个 system prompt。可以考虑每个 Agent 类型维护独立的"指令库"。
4. **PromptTemplate 模式**：laew 的 `-f` 文件提示词模式可以借鉴 PromptTemplate 的 `$@` 参数替换机制，让模板文件支持参数化。

---

## 专题 2：lane 并发模型

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/agent/src/harness/agent-harness.ts` | `AgentLane` 接口 + `AgentHarness` 类 + lane 管理 |
| `packages/agent/src/harness/session/types.ts` | `LanePointer` / `LaneRecord` / `Entry` 类型 |
| `packages/agent/src/harness/session/session.ts` | `Session.view(lane)` —— lane 视图隔离 |
| `packages/agent/src/harness/events.ts` | `HarnessEventBus` —— lane 事件广播 |

### 2.1 Lane 的概念

Lane 是 pi 的**并发执行单元**，本质是 session tree 上的一个命名分支指针。每个 lane 拥有：
- 独立的 `leafId`（指向 session tree 中的最新 entry）
- 独立的运行队列（steer / followUp / nextRun）
- 独立的操作状态（running / suspended / aborting）

```typescript
// packages/agent/src/harness/agent-harness.ts:152
export interface LaneInfo {
  name: string;
  leafId: string | null;
  operation: null | {
    id: string;
    kind: "run" | "compaction" | "navigation";
    status: "running" | "suspended" | "aborting";
  };
}
```

### 2.2 Lane 的创建与调度

`AgentLane` 接口定义了 lane 的全部操作：
```typescript
export interface AgentLane {
  readonly name: string;
  prompt(text: string, images?: ImageContent[]): Promise<RunResult>;
  skill(name: string, additionalInstructions?: string): Promise<RunResult>;
  promptFromTemplate(name: string, args?: string[]): Promise<RunResult>;
  compact(options?: { customInstructions?: string }): Promise<CompactionResult>;
  navigateTree(targetId: string | null, options?: NavigateOptions): Promise<NavigationResult>;
  steer(text: string, images?: ImageContent[]): Promise<QueueResult>;
  followUp(text: string, images?: ImageContent[]): Promise<QueueResult>;
  nextRun(text: string, images?: ImageContent[]): Promise<QueueResult>;
  // ... abort, resume, watch, etc.
}
```

Lane 之间的通信通过三种队列：
- **steer**：注入到当前运行中（"在你做 A 的同时，也注意 B"）
- **followUp**：在当前 run 结束后追加（"做完 A 后做 B"）
- **nextRun**：在下一次独立 run 时注入

### 2.3 Lane 的隔离与 Session Tree

Session 不是线性历史，而是一棵**树**。`Session.view(lane)` 返回一个 `SessionTree` 视图，只暴露该 lane 的路径：

```typescript
// packages/agent/src/harness/session/session.ts:116
view(lane: string): SessionTree {
  if (lane === "main") return this;
  return {
    getLeafId: () => this.getLeafIdForLane(lane),
    findEntriesOnBranch: (query) => this.queryBranchEntries(lane, query),
    // ...
  };
}
```

`createLane(lane, at)` 在指定 entry 处分叉创建新 lane，`moveLane(lane, to)` 移动 lane 的 leaf 指针。

### 2.4 并发限制与资源控制

`LaneBusy` 错误确保同一 lane 同时只有一个操作运行：
```typescript
export class LaneBusy extends TaggedError("LaneBusy")<{
  lane: string;
  operationId: string;
  operationKind: "run" | "compaction" | "navigation";
}> {}
```

lane 的操作有三种可能状态：running（正在执行）、suspended（崩溃后恢复时暂停）、aborting（正在中止）。

### 2.5 Lane 状态还原器（Reducer）

pi 的 lane 状态不是简单地存储在内存中，而是通过**事件溯源**（event sourcing）从持久化记录中重建。`reduceLaneState` 是核心还原函数：

```typescript
// packages/agent/src/harness/reducer.ts:506
export function reduceLaneState(input: LaneReductionInput): LaneReductionResult {
  validateRecordLog(input);  // 先验证记录一致性
  // ... 从 records + entries 重建 LaneState
}
```

`LaneState` 包含丰富的运行时信息：
```typescript
export interface LaneState {
  lane: string;
  leafId: string | null;
  operation: null | {
    id: string;
    kind: "run" | "compaction" | "navigation";
    intent: OperationStartedRecord["intent"];
    aborting: boolean;
    step: null | { kind: "assistant" | "compaction" | "branch_summary"; attempts: number; ... };
    toolBatch: ToolBatchState | null;  // 当前工具批次的执行状态
    pendingSteer: ProvisionedEntry[];
    pendingFollowUp: ProvisionedEntry[];
    deferred: DeferredHandle | null;   // 异步等待句柄
    overflowRecoveryUsed: boolean;      // 是否触发了溢出恢复压缩
  };
  pendingNextRun: ProvisionedEntry[];
}
```

### 2.6 Record Log 验证（一致性保护）

`validateRecordLog` 执行严格的协议一致性检查，防止损坏的持久化数据导致运行时错误：

```typescript
// packages/agent/src/harness/reducer.ts:312
export function validateRecordLog(input: RecordLogSlice): void {
  // 1. 不能有多个未完成的操作
  if (input.openOperations.length > 1) corrupt("multiple_open_operations", ...);

  // 2. 所有记录必须引用已知操作
  // 3. 操作完成后不能有新记录
  // 4. attempt 编号必须连续
  // 5. tool_started 必须匹配 assistant 中的 toolCall
  // 6. queue_enqueued 在 abort 后不允许
  // 7. provisioned entry 内容必须一致
}
```

`RecordLogCorruptionReason` 枚举了14种可能的损坏类型，每种都有明确的语义含义。这是 pi 对持久化层的**防御性设计**——即使数据库被手动修改或程序崩溃导致写入不完整，恢复时也能检测并拒绝进入不一致状态。

### 2.7 Steering 模式的两种策略

```typescript
export type QueueMode = "all" | "one-at-a-time";
```

- **`"one-at-a-time"`**（默认）：每次 turn 结束时只注入一条 steering 消息，剩余的留到下一次 turn
- **`"all"`**：一次性注入所有 pending 的 steering 消息

这影响了 agent 的"注意力集中度"：`one-at-a-time` 让 agent 每次只处理一个新输入，不会被多条 steering 消息淹没；`all` 模式适合需要批量注入的场景（如初始化时注入多条配置消息）。

### 2.8 Deferred Handle：异步工具结果

pi 支持**延迟工具结果**（Deferred Handle）：当 LLM 返回 `stopReason: "deferred"` 时，意味着请求已被发送到异步后端（如 Anthropic 的 batch API），需要稍后轮询结果：

```typescript
export interface DeferredHandle {
  provider: string;
  modelId: string;
  api: string;
  id: string;          // provider-specific token (response id, batch id, etc.)
  expiresAt?: number;
  pollAfterMs?: number;
  data?: JsonValue;    // provider-specific conversion data
}
```

lane 操作进入 `suspended` 状态时，可以通过 `resume()` 恢复并 fetch deferred 结果。这使得 pi 可以支持长时间运行的推理任务（如 Claude 的 extended thinking）。

### 对 laew 的借鉴价值

1. **Lane 比 laew 的 SubAgent 更灵活**：laew 的 SubAgent 是一次性执行单元，没有"分支-合并-导航"能力。如果 laew 需要支持"探索性编程"（尝试方案 A，不行再回到方案 B），可以引入类似 lane 的树形历史。
2. **steer/followUp/nextRun 队列机制**：laew 的 Main-Work Agent 拆 WorkFlow 后是线性执行。如果需要支持"边执行边调整"，可以借鉴 steer 队列设计。
3. **但 lane 模型对 laew 来说过重**：laew 是 CLI 单轮/多轮模式，不需要 session tree 分叉。laew 的 session_memory 表已经足够。建议保持现有架构，仅借鉴 steer 注入机制用于中断/调整正在进行的任务。
4. **事件溯源 + 状态还原**：laew 目前没有操作日志。如果需要支持"崩溃恢复"或"操作回放"，可以借鉴 pi 的 record log + reduceLaneState 模式。
5. **RecordLogCorruption 验证**：laew 的 SQLite 数据目前没有一致性校验。可以加入类似的验证层防止数据损坏。

1. **Lane 比 laew 的 SubAgent 更灵活**：laew 的 SubAgent 是一次性执行单元，没有"分支-合并-导航"能力。如果 laew 需要支持"探索性编程"（尝试方案 A，不行再回到方案 B），可以引入类似 lane 的树形历史。
2. **steer/followUp/nextRun 队列机制**：laew 的 Main-Work Agent 拆 WorkFlow 后是线性执行。如果需要支持"边执行边调整"，可以借鉴 steer 队列设计。
3. **但 lane 模型对 laew 来说过重**：laew 是 CLI 单轮/多轮模式，不需要 session tree 分叉。laew 的 session_memory 表已经足够。建议保持现有架构，仅借鉴 steer 注入机制用于中断/调整正在进行的任务。

---

## 专题 3：Harness 扩展架构

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/agent/src/harness/agent-harness.ts` | `AgentHarness` 类 + `AgentHarnessOptions` |
| `packages/agent/src/harness/types.ts` | `AgentHarnessTool` / `AgentHarnessStreamOptions` / `FileSystem` / `Shell` |
| `packages/agent/src/harness/events.ts` | `HarnessEventBus` 事件系统 |
| `packages/coding-agent/src/server/create-harness.ts` | `createCodingAgentHarness` —— 实际应用层 Harness |

### 3.1 Harness 的核心抽象

Harness 是 pi 的**运行时容器**，封装了以下能力：

```typescript
// packages/agent/src/harness/agent-harness.ts:243
export interface AgentHarnessOptions {
  session: Session;
  models: Models;
  model: Model<Api>;
  thinkingLevel?: ThinkingLevel;
  tools?: HarnessTool[];
  toolContext?: object | (() => object | Promise<object>);
  systemPrompt?: string | (() => string | Promise<string>);
  resources?: Resources;           // Skills + PromptTemplates
  streamOptions?: StreamOptions;
  retry?: RetryPolicy;
  compaction?: CompactionSettings;
  steeringMode?: QueueMode;
  followUpMode?: QueueMode;
  toolExecution?: "sequential" | "parallel";
  drive?: "automatic" | "manual";
  toProviderMessages?: (messages: AgentMessage[]) => Message[];
  entryProjectors?: Record<string, EntryProjector>;
}
```

### 3.2 扩展点

Harness 的扩展主要通过三个维度：

**a) 工具扩展（AgentHarnessTool）**
```typescript
export type AgentHarnessTool<TContext, TParameters, TDetails> = {
  name: string;
  description: string;
  parameters: TParameters;
  execute(toolCallId, params, signal, onUpdate, context): Promise<AgentToolResult>;
};
```
工具通过泛型 `TContext` 绑定应用上下文。coding-agent 注入 `ExecutionToolContext { env: ExecutionEnv }`，使 bash/read/edit/write 能访问文件系统。

**b) Hooks（生命周期钩子）**
```typescript
export type HookName =
  | "before_run" | "before_resume" | "before_run_end"
  | "transform_context" | "before_request" | "before_payload"
  | "after_response" | "before_tool" | "after_tool"
  | "before_compaction" | "before_navigation";
```
但注意：`AgentHarness` 基类的 `hooks` 实际是 `UnavailableRegistry`，调用会抛 `HarnessNotImplemented`。真正的 hook 实现在更上层（server 包的完整 Harness）。

**c) Events（事件系统）**
```typescript
export class HarnessEventBus implements Events {
  emit(event: HarnessEvent): void;
  watch<TSnapshot>(captureSnapshot: () => TSnapshot): WatchHandle<TSnapshot>;
}
```
事件类型目前只有 `run_start` 和 `run_end`，但 watch 机制支持快照+缓冲区模式，可扩展。

**d) FileSystem / Shell 抽象**
```typescript
export interface FileSystem {
  cwd: string;
  readTextFile(path): Promise<Result<string, FileError>>;
  writeFile(path, content): Promise<Result<void, FileError>>;
  // ... 完整的文件系统操作
}
export interface Shell {
  exec(command, options): Promise<Result<{ stdout; stderr; exitCode }, ExecutionError>>;
}
```
这使得 AgentHarness 完全与运行环境解耦：可以是本地 Node.js、远程 sandbox、或测试 mock。

### 3.3 与其他 Agent 框架 Harness 的区别

- **Claude Code**：Harness 是隐式的，直接在 CLI 主循环中实现
- **Codex**：Harness 是 `AgentLoop` 函数，接受配置参数
- **pi**：Harness 是**面向对象的持久实体**，拥有状态（tools/model/resources/session）和生命周期（create/close），支持 lane 并发

pi 的 Harness 是最"重量级"的——它不仅仅是编排器，更是**运行时状态容器**。

### 3.4 自定义 Harness 的方式

```typescript
// packages/coding-agent/src/server/create-harness.ts
export async function createCodingAgentHarness(options: CreateCodingAgentHarnessOptions) {
  // 1. 创建工具（bash/read/edit/write）
  // 2. 绑定上下文（ExecutionToolContext { env }）
  // 3. 构建系统提示词
  // 4. AgentHarness.create({ session, models, model, tools, ... })
}
```

### 3.5 Harness 的惰性初始化模式

`createCodingAgentHarness` 使用了一个巧妙的**闭包惰性引用**模式：

```typescript
// packages/coding-agent/src/server/create-harness.ts:92
let harness: AgentHarness | undefined;
const getHarness = (): AgentHarness => {
  if (!harness) throw new Error("Coding-agent Harness callback ran before Harness initialization");
  return harness;
};
```

这是因为工具的 `prepare` 回调需要访问 harness（获取当前 model/thinkingLevel），但 harness 在工具创建之后才被初始化。通过闭包捕获 `harness` 变量的引用，工具在执行时可以安全地访问已初始化的 harness。

bash tool 的 `prepare` 回调展示了这种模式的实际用途：
```typescript
prepare: async (execution) => {
  const currentHarness = getHarness();
  const [model, thinkingLevel] = await Promise.all([
    currentHarness.getModel(),
    currentHarness.getThinkingLevel(),
  ]);
  execution.env.PI_SESSION_ID = metadata.id;
  execution.env.PI_PROVIDER = model.provider;
  execution.env.PI_MODEL = model.id;
  execution.env.PI_REASONING_LEVEL = thinkingLevel;
},
```

这使得每个 bash 命令都能知道当前使用的模型和推理级别，通过环境变量暴露给外部脚本。

### 3.6 CodingAgentHarnessTool 的 promptSnippet 机制

每个工具可以携带 `promptSnippet`（一句话描述）和 `promptGuidelines`（使用指南数组），这些被注入系统提示词：

```typescript
// packages/coding-agent/src/server/create-harness.ts:56
export function buildCodingAgentHarnessSystemPrompt(options): string {
  const activeTools = options.activeToolNames.flatMap((name) => {
    const tool = options.tools.find((candidate) => candidate.name === name);
    return tool ? [tool] : [];
  });
  const toolSnippets = Object.fromEntries(
    activeTools.flatMap((tool) => {
      const promptSnippet = tool.promptSnippet?.replace(/[\r\n]+/g, " ").replace(/\s+/g, " ").trim();
      return promptSnippet ? [[tool.name, promptSnippet]] : [];
    }),
  );
  const promptGuidelines = activeTools.flatMap((tool) => tool.promptGuidelines ?? []);
  return buildSystemPrompt({ ...options, selectedTools, toolSnippets, promptGuidelines });
}
```

这形成了一个**工具-提示词联动**机制：添加一个新工具时，只需要提供 `{ tool, promptSnippet, promptGuidelines }`，系统提示词会自动更新。

### 3.7 `drive: "automatic" | "manual"` 模式

```typescript
drive?: "automatic" | "manual";
```

- **`"automatic"`**：Harness 自动运行 agent loop 直到完成
- **`"manual"`**：外部代码逐步驱动（`peekAction()` → `executeAction()`）

manual 模式使得 TUI 可以在每个 action 之间插入 UI 更新、用户确认等操作。这是 pi 的 TUI 实现能够在工具执行时显示实时进度的关键。

### 对 laew 的借鉴价值

1. **FileSystem / Shell 抽象层**：laew 的 BashTool 直接调用 `std::process::Command`。如果要支持远程执行或 sandbox，可以借鉴 pi 的 `ExecutionEnv` 抽象。
2. **Hooks 机制**：laew 目前没有生命周期钩子。在 `agent/mod.rs` 的主循环中，可以在 `before_tool` / `after_tool` 处插入权限检查钩子。
3. **toolContext 注入模式**：pi 的 `AgentHarnessTool.execute(..., context)` 将上下文注入工具。laew 的 `Tool::execute` 目前直接接收 `Session`，可以考虑改为显式 context 对象。
4. **promptSnippet 联动机制**：laew 的工具描述目前硬编码在 system_prompt 中。可以借鉴 pi 的模式，让每个工具携带自己的系统提示词贡献，自动拼接。
5. **drive manual 模式**：laew 的 TUI 目前是阻塞式等待 agent 完成。如果需要在工具执行时显示实时输出，可以借鉴 manual drive 模式逐步推进 agent loop。

1. **FileSystem / Shell 抽象层**：laew 的 BashTool 直接调用 `std::process::Command`。如果要支持远程执行或 sandbox，可以借鉴 pi 的 `ExecutionEnv` 抽象。
2. **Hooks 机制**：laew 目前没有生命周期钩子。在 `agent/mod.rs` 的主循环中，可以在 `before_tool` / `after_tool` 处插入权限检查钩子。
3. **toolContext 注入模式**：pi 的 `AgentHarnessTool.execute(..., context)` 将上下文注入工具。laew 的 `Tool::execute` 目前直接接收 `Session`，可以考虑改为显式 context 对象。

---

## 专题 4：Context 管理 —— 压缩 + 分支摘要

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/agent/src/harness/compaction/compaction.ts` | 压缩核心逻辑 |
| `packages/agent/src/harness/compaction/branch-summarization.ts` | 分支摘要生成 |
| `packages/agent/src/harness/compaction/utils.ts` | 文件操作提取 + 会话序列化 |
| `packages/agent/src/harness/session/context.ts` | 上下文构建（entry → messages 转换） |
| `packages/agent/src/harness/session/types.ts` | `CompactionEntry` / `BranchSummaryEntry` 类型 |
| `packages/agent/src/harness/messages.ts` | `convertToLlm` / `createCompactionSummaryMessage` |

### 4.1 压缩策略

**触发条件**（`shouldCompact`）：
```typescript
export function shouldCompact(contextTokens: number, contextWindow: number, settings: CompactionSettings): boolean {
  if (!settings.enabled) return false;
  return contextTokens > contextWindow - settings.reserveTokens;
}
```

默认配置：
```typescript
export const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = {
  enabled: true,
  reserveTokens: 16384,    // 给摘要 prompt 预留的 token
  keepRecentTokens: 20000,  // 压缩后保留的最近上下文 token 数
};
```

**切割点选择**（`findCutPoint`）：
1. 从后往前累计 token，当累积 >= `keepRecentTokens` 时找到切割位置
2. 切割点必须是有效的 turn 边界（user 消息 / bashExecution / branchSummary）
3. 如果切割点在 turn 中间（`isSplitTurn`），记录 `turnStartIndex` 用于生成 turn 前缀摘要

**摘要生成**（`generateSummary`）：
- 使用当前模型（非独立摘要模型）
- 两次 LLM 调用：第一次生成历史摘要，第二次生成 turn 前缀摘要（仅 `isSplitTurn` 时）
- 摘要格式固定为 `## Goal` / `## Constraints` / `## Progress` / `## Key Decisions` / `## Next Steps` / `## Critical Context`
- 摘要末尾追加文件操作列表（readFiles / modifiedFiles）

**增量更新**（`UPDATE_SUMMARIZATION_PROMPT`）：
当已有旧摘要时，使用增量更新 prompt，保留旧信息并合并新进展，避免从头重新生成。

### 4.2 Token 估算

```typescript
export function estimateTokens(message: AgentMessage): number {
  // 字符数 / 4 的粗略估算
  // 图片按 4800 字符估算
  // thinking + toolCall 参数都计入
}
```

上下文使用量优先使用 provider 返回的 `usage`（精确），仅在没有 provider 数据时回退到字符估算。

### 4.3 分支摘要（Branch Summarization）

当用户从 lane A 导航到 lane B 时，lane A 的工作需要被摘要保存：

```typescript
// branch-summarization.ts:82
export async function collectEntriesForBranchSummary(
  session: Session,
  oldLeafId: string | null,
  targetId: string,
): Promise<CollectEntriesResult>
```

1. 找到 oldLeafId 和 targetId 的**最近公共祖先**（common ancestor）
2. 收集 oldLeafId 到 common ancestor 之间的所有 entry
3. 在 token 预算内（`contextWindow - reserveTokens`）选择要摘要的 entry
4. 生成摘要并追加到 session tree 的目标分支上

摘要格式与压缩摘要类似，但前缀不同：
```
The user explored a different conversation branch before returning here.
Summary of that exploration:
## Goal / ## Constraints / ## Progress / ## Key Decisions / ## Next Steps
```

### 4.4 上下文构建流程

`buildSessionContext(pathEntries)` 是核心：
```typescript
// session/context.ts:90
export function buildSessionContext(pathEntries: Entry[]): SessionContext {
  const state = deriveSessionContextState(pathEntries); // 提取 thinkingLevel/model/activeTools
  const contextEntries = buildContextEntries(pathEntries); // 应用 compaction 压缩
  const messages = contextEntries.flatMap(entry =>
    sessionEntryToContextMessages(entry)
  );
  return { ...state, messages };
}
```

`defaultContextEntryTransform` 的关键逻辑：如果找到最近的 compaction entry，则只保留它 + 后续的 entry（丢弃更早的历史）。

### 4.5 文件操作追踪（Compaction Details）

压缩摘要的末尾会追加文件操作列表，这是 pi 独有的设计：

```typescript
// compaction/utils.ts:24
export function extractFileOpsFromMessage(message: AgentMessage, fileOps: FileOperations): void {
  if (message.role !== "assistant") return;
  for (const block of message.content) {
    if (block.type !== "toolCall") continue;
    const path = typeof args.path === "string" ? args.path : undefined;
    if (!path) continue;
    switch (block.name) {
      case "read": fileOps.read.add(path); break;
      case "write": fileOps.written.add(path); break;
      case "edit": fileOps.edited.add(path); break;
    }
  }
}
```

摘要输出格式：
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

### 4.6 会话序列化（`serializeConversation`）

摘要生成时，消息被序列化为纯文本格式：

```typescript
// compaction/utils.ts:91
export function serializeConversation(messages: Message[]): string {
  const parts: string[] = [];
  for (const msg of messages) {
    if (msg.role === "user") {
      parts.push(`[User]: ${content}`);
    } else if (msg.role === "assistant") {
      if (thinkingParts.length > 0) parts.push(`[Assistant thinking]: ${thinking}`);
      parts.push(`[Assistant]: ${text}`);
      if (toolCalls.length > 0) parts.push(`[Assistant tool calls]: ${toolCalls.join("; ")}`);
    } else if (msg.role === "toolResult") {
      parts.push(`[Tool result]: ${truncateForSummary(content, 2000)}`);  // 截断到 2000 字符
    }
  }
  return parts.join("\n\n");
}
```

注意 tool result 被截断到 2000 字符（`TOOL_RESULT_MAX_CHARS`）。这是因为 bash 输出可能有数万行，全部传给摘要 LLM 会浪费 token。

### 4.7 压缩后的 CompactionEntry 持久化结构

```typescript
// session/types.ts:44
export interface CompactionEntry extends EntryBase {
  type: "compaction";
  summary: string;              // 生成的摘要文本
  retainedTail: AgentMessage[]; // 压缩后保留的最近消息（完整保留，不截断）
  tokensBefore: number;         // 压缩前的总 token 数
  details?: unknown;            // CompactionDetails { readFiles, modifiedFiles }
  usage?: Usage;                // 摘要生成时的 LLM 使用量
}
```

`retainedTail` 的设计很关键：它不是摘要的一部分，而是**完整保留的最近消息**。这意味着压缩后的上下文 = compaction summary + retainedTail。模型看到的是"摘要 + 原始最近对话"的组合，而非纯摘要。

### 4.8 Split Turn 处理

当压缩切割点落在一个 turn 的中间时（`isSplitTurn`），pi 会分别生成两份摘要：

1. **历史摘要**：切割点之前的所有消息
2. **Turn 前缀摘要**：切割点到 turn 开始之间的消息（用 `TURN_PREFIX_SUMMARIZATION_PROMPT`）

最终 compaction summary = 历史摘要 + `---` + turn 前缀摘要。这确保了即使压缩发生在工具执行过程中，模型也能理解"之前做了什么"。

### 对 laew 的借鉴价值

1. **laew 目前没有上下文压缩**：随着对话变长，token 会超出窗口。可以借鉴 pi 的 `shouldCompact` + `findCutPoint` 逻辑。
2. **摘要格式化模板**：pi 的结构化摘要（Goal/Progress/Next Steps）比 laew 的 SessionContext Agent 的自由格式更利于恢复。
3. **文件操作追踪**：pi 在摘要末尾追加 `readFiles / modifiedFiles`，便于模型知道哪些文件已被操作过。laew 可以从 tool result 中提取类似信息。
4. **branch summary 对 laew 的意义有限**：laew 不需要 lane 分叉，但如果需要"方案探索"功能，可以借鉴公共祖先算法来处理分支历史。
5. **Tool result 截断**：laew 的 tool result 目前没有长度控制。在压缩/摘要场景下，需要截断过长的 bash 输出。pi 的 2000 字符限制值得参考。
6. **retainedTail 设计**：laew 的 SessionContext 摘要是纯文本。如果引入压缩，应该保留最近 N 条完整消息而非全部摘要化。

1. **laew 目前没有上下文压缩**：随着对话变长，token 会超出窗口。可以借鉴 pi 的 `shouldCompact` + `findCutPoint` 逻辑。
2. **摘要格式化模板**：pi 的结构化摘要（Goal/Progress/Next Steps）比 laew 的 SessionContext Agent 的自由格式更利于恢复。
3. **文件操作追踪**：pi 在摘要末尾追加 `readFiles / modifiedFiles`，便于模型知道哪些文件已被操作过。laew 可以从 tool result 中提取类似信息。
4. **branch summary 对 laew 的意义有限**：laew 不需要 lane 分叉，但如果需要"方案探索"功能，可以借鉴公共祖先算法来处理分支历史。

---

## 专题 5：主循环与工具系统

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/agent/src/agent-loop.ts` | Agent 主循环（`runLoop`） |
| `packages/agent/src/types.ts` | `AgentLoopConfig` / `AgentTool` / `AgentEvent` 类型 |
| `packages/agent/src/harness/tools/bash.ts` | BashTool 实现 |
| `packages/agent/src/harness/tools/read.ts` | ReadTool 实现 |
| `packages/agent/src/harness/tools/edit.ts` | EditTool 实现 |
| `packages/agent/src/harness/tools/write.ts` | WriteTool 实现 |
| `packages/agent/src/harness/tools/file-mutation-queue.ts` | 文件写入串行化 |

### 5.1 主循环架构

`runLoop` 是核心，采用**双层循环**结构：

```typescript
// agent-loop.ts:156
async function runLoop(initialContext, newMessages, initialConfig, signal, emit, streamFunction) {
  // 外层循环：处理 followUp 消息（agent 停止后追加的新任务）
  while (true) {
    // 内层循环：处理 tool calls + steering 消息
    while (hasMoreToolCalls || pendingMessages.length > 0) {
      // 1. prepareNextTurn（可能触发 compaction）
      // 2. 注入 pending steering messages
      // 3. streamAssistantResponse（调用 LLM）
      // 4. 检查 tool calls
      // 5. executeToolCalls（串行或并行）
      // 6. shouldStopAfterTurn 检查
    }
    // 检查 followUp 消息
    const followUpMessages = (await config.getFollowUpMessages?.()) || [];
    if (followUpMessages.length > 0) { pendingMessages = followUpMessages; continue; }
    break;
  }
}
```

### 5.2 工具系统

**Tool 接口**（`AgentTool`）：
```typescript
export interface AgentTool<TParameters extends TSchema = TSchema, TDetails = unknown> {
  name: string;
  label?: string;
  description: string;
  parameters: TParameters;
  executionMode?: "sequential" | "parallel";
  constrainedSampling?: false | ConstrainedSamplingConfig;
  prepareArguments?(input: unknown): unknown;  // 参数预处理
  execute(
    toolCallId: string,
    params: Static<TParameters>,
    signal: AbortSignal | undefined,
    onUpdate: AgentToolUpdateCallback | undefined,
  ): Promise<AgentToolResult>;
}
```

**Tool 与 AgentHarnessTool 的区别**：
- `AgentTool`：纯接口，无上下文绑定
- `AgentHarnessTool<TContext>`：带上下文泛型，execute 多接收 `context: TContext` 参数
- `HarnessTool`：增加了 `replay: "never" | "safe"` 字段，控制重放行为

### 5.3 工具调用流程

`prepareToolCall` → `executePreparedToolCall` → `finalizeExecutedToolCall` 三阶段：

1. **prepareToolCall**：
   - 查找 tool（找不到返回 error result）
   - 调用 `prepareArguments`（参数预处理，如 edit tool 的 legacy 参数兼容）
   - 调用 `validateToolArguments`（schema 验证）
   - 调用 `config.beforeToolCall`（可拦截/阻止）

2. **executePreparedToolCall**：
   - 调用 `tool.execute(...)` 
   - 支持 `onUpdate` 回调（实时进度，如 bash 输出流）
   - 捕获异常转为 error result

3. **finalizeExecutedToolCall**：
   - 调用 `config.afterToolCall`（可修改结果/标记错误/注入 terminate）

### 5.4 串行 vs 并行执行

```typescript
// agent-loop.ts:417
const hasSequentialToolCall = toolCalls.some(
  (tc) => currentContext.tools?.find((t) => t.name === tc.name)?. executionMode === "sequential"
);
if (config.toolExecution === "sequential" || hasSequentialToolCall) {
  return executeToolCallsSequential(...);
}
return executeToolCallsParallel(...);
```

并行执行使用懒求值：先按序 prepare 所有 tool call，然后 `Promise.all` 并行执行。但 `emit` 事件仍然在 finalize 后按序发射。

### 5.5 权限控制

`beforeToolCall` 是权限控制的注入点：
```typescript
export interface BeforeToolCallResult {
  block?: boolean;        // 阻止执行
  reason?: string;        // 阻止原因
  terminate?: boolean;    // 终止整个工具批次
}
```

`afterToolCall` 可以修改结果，但主要用于内容过滤或注入使用量，不用于权限控制。

### 5.6 流式输出处理

`streamAssistantResponse` 处理流式 LLM 响应：
- `start`：创建 partial message 并插入 context
- `text_delta` / `thinking_delta` / `toolcall_delta`：更新 partial message 并 emit 更新事件
- `done` / `error`：获取 final message，替换 context 中的 partial

### 5.7 截断消息保护

当 `stopReason === "length"`（token 限制截断）时，所有 tool call 的参数都可能不完整：
```typescript
const executedToolBatch = message.stopReason === "length"
  ? await failToolCallsFromTruncatedMessage(toolCalls, emit)
  : await executeToolCalls(...);
```
截断的 tool call 不执行，返回错误提示让模型重新发送。

### 5.8 文件操作串行化

`withFileMutationQueue` 确保同一文件的并发写入被串行化：
```typescript
export async function withFileMutationQueue<T>(env: ExecutionEnv, path: string, fn: () => Promise<T>) {
  // 使用 WeakMap<ExecutionEnv, Map<canonicalPath, Promise<void>>>
  // 链式 promise 实现 per-file 排他锁
}
```

实现细节：
```typescript
// packages/agent/src/harness/tools/file-mutation-queue.ts
const states = new WeakMap<ExecutionEnv, MutationQueueState>();

export async function withFileMutationQueue<T>(env: ExecutionEnv, path: string, fn: () => Promise<T>) {
  const state = getState(env);
  const key = await getMutationQueueKey(env, path);  // canonicalPath 作为 key
  const currentQueue = state.queues.get(key) ?? Promise.resolve();
  let releaseNext = () => {};
  const nextQueue = new Promise<void>((resolve) => { releaseNext = resolve; });
  const chainedQueue = currentQueue.then(() => nextQueue);
  state.queues.set(key, chainedQueue);
  await currentQueue;  // 等待前一个操作完成
  try {
    return await fn();  // 执行当前操作
  } finally {
    releaseNext();  // 释放下一个操作
  }
}
```

这是经典的 **per-key Promise 链式锁** 模式：
- 每个文件路径有一个 Promise 队列
- 新操作排在队尾，等待前一个完成后才执行
- `WeakMap<ExecutionEnv, ...>` 确保不同 ExecutionEnv 实例的锁互不干扰
- canonicalPath 而非绝对路径作为 key，避免符号链接导致的锁失效

Edit 和 Write 工具都使用了这个机制：
```typescript
// edit.ts:105
return withFileMutationQueue(env, absolutePath, async () => {
  const readResult = await env.readTextFile(absolutePath, signal);
  // ... apply edits ...
  const writeResult = await env.writeFile(absolutePath, finalContent, signal);
  // ...
});
```

这确保了 read-modify-write 是原子的——不会出现两个 edit 操作同时读取同一文件、各自修改、后写入的覆盖前一个的情况。

### 5.9 Edit Tool 的参数兼容层

`prepareArguments` 是 pi 的一个精巧设计，用于处理 LLM 输出的参数格式不一致问题：

```typescript
// packages/agent/src/harness/tools/edit.ts:55
function prepareEditArguments(input: unknown): EditToolInput {
  // 1. edits 字段是字符串 → 尝试 JSON.parse
  if (typeof args.edits === "string") {
    try {
      const parsed = JSON.parse(args.edits);
      if (Array.isArray(parsed)) args.edits = parsed;
      else if (isSingleEditInput(parsed)) args.edits = [parsed];
    } catch {}
  }
  // 2. edits 是单个 {oldText, newText} 对象 → 包装为数组
  else if (isSingleEditInput(args.edits)) {
    args.edits = [args.edits];
  }
  // 3. 顶层有 oldText/newText → 迁移到 edits 数组（legacy 兼容）
  if (typeof legacy.oldText === "string" && typeof legacy.newText === "string") {
    edits.push({ oldText: legacy.oldText, newText: legacy.newText });
  }
  return { ...rest, edits };
}
```

这种"宽松输入、严格内部"的设计是 pi 应对 LLM 输出不稳定的关键策略。模型可能以不同格式输出参数，`prepareArguments` 在 schema 验证之前做标准化。

### 5.10 Bash Tool 的实时进度更新

```typescript
// packages/agent/src/harness/tools/bash.ts:74
const scheduleOutputUpdate = (): void => {
  updateDirty = true;
  const delay = BASH_UPDATE_THROTTLE_MS - (Date.now() - lastUpdateAt); //100ms 节流
  if (delay <= 0) {
    clearUpdateTimer();
    emitOutputUpdate();
    return;
  }
  updateTimer ??= setTimeout(() => { emitOutputUpdate(); }, delay);
};
```

bash 工具使用 100ms 节流的 `onUpdate` 回调实现实时输出流。`executeShellWithCapture` 在每次收到 stdout/stderr chunk 时调用 `onChunk`，触发节流后的 `onUpdate` 发射。

输出截断策略：
- 默认最大 500 行 或 32KB（取先达到的）
- 截断后保存完整输出到临时文件
- 在返回文本末尾附加提示：`[Showing lines X-Y of Z. Full output: /tmp/xxx]`

### 5.11 Read Tool 的图片处理

```typescript
// packages/agent/src/harness/tools/read.ts:57
const mimeType = detectSupportedImageMimeType(bytes);
if (mimeType) {
  if (options?.imageProcessor) {
    const processed = await options.imageProcessor(bytes, mimeType, {
      autoResizeImages: options.autoResizeImages ?? true,
    });
    return { content: [
      { type: "text", text: `Read image file [${processed.mimeType}]` },
      { type: "image", data: processed.data, mimeType: processed.mimeType },
    ]};
  }
  // 直接返回 base64
  return { content: [
    { type: "text", text: `Read image file [${mimeType}]` },
    { type: "image", data: encodeBase64(bytes), mimeType },
  ]};
}
```

Read Tool 支持 jpg/png/gif/webp/bmp 格式。BMP 需要额外的 imageProcessor 转换。图片被 base64 编码后作为 `ImageContent` 返回，直接进入 LLM 的多模态输入。

### 对 laew 的借鉴价值

1. **beforeToolCall/afterToolCall 钩子**：laew 的 Tool::execute 目前没有前/后拦截点。可以加入 hook 机制用于权限检查（类似 Yolo 的"拒绝危险操作"功能）。
2. **并行工具执行**：laew 目前是串行执行 tool calls。如果未来需要并行，可以借鉴 pi 的 lazy-execution 模式。
3. **截断消息保护**：laew 需要处理 LLM 响应被截断的情况，pi 的 `failToolCallsFromTruncatedMessage` 逻辑值得移植。
4. **文件写入串行化**：laew 的 WriteTool 目前没有并发保护。如果 SubAgent 并行运行，需要类似机制。
5. **prepareArguments 预处理**：laew 的工具参数直接从 JSON 反序列化，没有预处理/兼容层。可以考虑加入——LLM 输出的参数格式经常不一致。
6. **Bash 实时输出流**：laew 的 BashTool 目前是同步等待完成。如果 TUI 需要实时显示 bash 输出，可以借鉴 pi 的 `onUpdate` 回调 + 节流机制。

1. **beforeToolCall/afterToolCall 钩子**：laew 的 Tool::execute 目前没有前/后拦截点。可以加入 hook 机制用于权限检查（类似 Yolo 的"拒绝危险操作"功能）。
2. **并行工具执行**：laew 目前是串行执行 tool calls。如果未来需要并行，可以借鉴 pi 的 lazy-execution 模式。
3. **截断消息保护**：laew 需要处理 LLM 响应被截断的情况，pi 的 `failToolCallsFromTruncatedMessage` 逻辑值得移植。
4. **文件写入串行化**：laew 的 WriteTool 目前没有并发保护。如果 SubAgent 并行运行，需要类似机制。
5. **prepareArguments 预处理**：laew 的工具参数直接从 JSON 反序列化，没有预处理/兼容层。可以考虑加入。

---

## 专题 6：多 Provider 与协议适配

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `packages/ai/src/types.ts` | `Model` / `Provider` / `Api` / `AssistantMessage` 等核心类型 |
| `packages/ai/src/models.ts` | `Models` 接口 + `ModelsImpl` 实现 + `createProvider` 工厂 |
| `packages/ai/src/api/anthropic-messages.ts` | Anthropic Messages API 适配 |
| `packages/ai/src/api/openai-completions.ts` | OpenAI Chat Completions API 适配 |
| `packages/ai/src/api/openai-responses.ts` | OpenAI Responses API 适配 |
| `packages/ai/src/api/bedrock-converse-stream.ts` | AWS Bedrock Converse API 适配 |
| `packages/ai/src/api/google-generative-ai.ts` | Google Gemini API 适配 |
| `packages/ai/src/models-store.ts` | 模型目录持久化 |
| `packages/ai/src/auth/` | 认证管理（API Key / OAuth） |

### 6.1 Provider 统一抽象

```typescript
// packages/ai/src/models.ts:97
export interface Provider<TApi extends Api = Api> {
  readonly id: string;
  readonly name: string;
  readonly auth: ProviderAuth;
  getModels(): readonly Model<TApi>[];
  refreshModels?(context: RefreshModelsContext): Promise<void>;
  filterModels?(models: readonly Model<TApi>[], credential: Credential): readonly Model<TApi>[];
  stream(model, context, options): AssistantMessageEventStream;
  streamSimple(model, context, options): AssistantMessageEventStream;
  fetchDeferred?(model, handle, options): AssistantMessageEventStream;
  cancelDeferred?(model, handle, options): Promise<void>;
}
```

`createProvider` 工厂函数将分散的配置组合成 Provider：
```typescript
export function createProvider<TApi>(input: CreateProviderOptions<TApi>): Provider<TApi> {
  // input: { id, name, baseUrl, headers, auth, models, fetchModels?, filterModels?, api }
  // api 可以是单个 ProviderStreams 或 Map<Api, ProviderStreams>
}
```

### 6.2 统一消息模型

所有 API 的响应被转换为统一的 `AssistantMessage`：
```typescript
export interface AssistantMessage {
  role: "assistant";
  content: (TextContent | ThinkingContent | ToolCall)[];
  api: Api;
  provider: ProviderId;
  model: string;
  usage: Usage;
  stopReason: StopReason;  // "pending" | "stop" | "length" | "toolUse" | "error" | "aborted" | "deferred"
  timestamp: number;
}
```

输入消息统一为三种：
```typescript
export type Message = UserMessage | AssistantMessage | ToolResultMessage;
```

### 6.3 协议适配示例

**Anthropic Messages API**（`api/anthropic-messages.ts`）：
- SSE 流解析：自建 SSE 解析器（`iterateSseMessages`），不依赖 Anthropic SDK 的流式处理
- 事件类型映射：`content_block_start` → `text_start` / `thinking_start` / `toolcall_start`
- Thinking 处理：支持 `signature` 多轮一致性、`redacted_thinking` 加密载荷
- Tool call 流式参数：`partial_json` + `parseStreamingJson` 增量解析
- OAuth 身份伪装：注入 Claude Code 的 `User-Agent` 和 beta header
- Cache control：在 system prompt 和最后 user message 上加 `cache_control: ephemeral`

**OpenAI Completions API**（`api/openai-completions.ts`）：
- 使用 OpenAI SDK 客户端（不同于 Anthropic 的自建 SSE）
- Reasoning 细节处理：`reasoning_details` 数组（summary / encrypted / text 三种类型）
- 兼容性层：`OpenAICompletionsCompat` 包含约 20 个兼容性开关，覆盖 vLLM / DeepSeek / Qwen / Together 等变体
- Grammar constrained sampling：支持 Lark/regex 语法约束的工具调用
- Chat template kwargs：`qwen` / `deepseek` / `together` 等不同格式的 thinking 参数

### 6.4 流式输出统一抽象

`AssistantMessageEventStream` 是核心：
```typescript
// 所有 API 的 stream 函数返回统一的事件流
type AssistantMessageEvent =
  | { type: "start"; partial: AssistantMessage }
  | { type: "text_start" | "text_delta" | "text_end"; ... }
  | { type: "thinking_start" | "thinking_delta" | "thinking_end"; ... }
  | { type: "toolcall_start" | "toolcall_delta" | "toolcall_end"; ... }
  | { type: "done"; reason: StopReason; message: AssistantMessage }
  | { type: "error"; reason: StopReason; error: AssistantMessage };
```

`Models.streamSimple` 是上层入口，统一处理 auth 解析、header 合并、retry 逻辑后委托给 Provider.streamSimple。

### 6.5 Model 目录与动态刷新

`ModelsStore` 接口支持模型目录持久化：
```typescript
export interface ModelsStoreEntry {
  models: readonly Model<Api>[];
  lastModified?: number;
  checkedAt?: number;
  etag?: string;
}
```

动态 Provider（如 GitHub Copilot）通过 `refreshModels` 拉取模型列表，使用 ETag + Last-Modified 条件请求减少网络开销。`ModelsImpl.refresh` 支持并发刷新多个 provider，带 generation 检查和 abort 取消。

刷新流程的 Generation 检查机制：
```typescript
// packages/ai/src/models.ts:320
private supersedeProviderRefresh(providerId: string): number {
  const generation = (this.refreshGenerations.get(providerId) ?? 0) + 1;
  this.refreshGenerations.set(providerId, generation);
  const previous = this.refreshControllers.get(providerId);
  if (previous) {
    this.refreshControllers.delete(providerId);
    previous.abort();  // 取消之前的刷新
  }
  return generation;
}
```

每次 `setProvider` / `deleteProvider` 都会递增 generation 并取消进行中的刷新。`publishProviderModels` 在写入前检查 generation 是否仍然匹配，防止旧刷新覆盖新数据。Publication 链（`publicationChains`）确保同一 provider 的持久化操作按序执行，不产生竞态。

`ModelsImpl.refresh` 的并发策略：
```typescript
async refresh(options: ModelsRefreshOptions = {}): Promise<ModelsRefreshResult> {
  const refreshable = Array.from(this.providers.values()).filter(
    (provider) => provider.refreshModels !== undefined
  );
  const refresh = Promise.all(
    refreshable.map(async (provider) => {
      // 1. 先用 stored credential 恢复缓存（allowNetwork=false）
      await this.runProviderRefreshPhase(provider, storedCredential, false, ...);
      // 2. 再用 resolved credential 刷新（allowNetwork=true）
      await this.runProviderRefreshPhase(provider, credential, true, ...);
    })
  );
  await raceWithAbortSignal(refresh, callerSignal);
}
```

每个 provider 的刷新分两阶段：先离线恢复缓存（快速），再在线刷新（可能慢）。这确保了即使网络不可用，应用也能使用上次缓存的模型列表。

### 6.6 认证管理

三层认证模型：
- **API Key**：环境变量 / 配置文件直接注入
- **OAuth**：GitHub Copilot 等需要 token 刷新的场景
- **AWS**：SigV4 签名（Bedrock），通过 Smithy middleware 注入

`CredentialStore` 接口抽象凭证持久化，`InMemoryCredentialStore` 是默认实现。`resolveProviderAuth` 在每次请求前解析有效凭证。

### 6.7 Anthropic 适配的 OAuth 身份伪装

pi 对 Anthropic 的 OAuth 支持包含一个巧妙的**身份伪装**机制：

```typescript
// packages/ai/src/api/anthropic-messages.ts:926
if (apiKey && isOAuthToken(apiKey)) {
  const client = new Anthropic({
    apiKey: null,
    authToken: apiKey,
    defaultHeaders: mergeClientHeaders({
      "anthropic-beta": ["claude-code-20250219", "oauth-2025-04-20", ...betaFeatures].join(","),
      "user-agent": `claude-cli/${claudeCodeVersion}`,  // 伪装为 Claude Code
      "x-app": "cli",
    }),
  });
  return { client, isOAuthToken: true };
}
```

当使用 OAuth token 时，pi 会：
1. 设置 `user-agent` 为 `claude-cli/2.1.251`（模拟 Claude Code CLI）
2. 添加 Claude Code 专属 beta header
3. 在 tool name 和 system prompt 中注入 Claude Code 身份
4. Tool 名称双向转换：`toClaudeCodeName` / `fromClaudeCodeName`

```typescript
// tool name 转换映射
const claudeCodeTools = ["Read", "Write", "Edit", "Bash", "Grep", "Glob", ...];
const toClaudeCodeName = (name: string) => ccToolLookup.get(name.toLowerCase()) ?? name;
```

这意味着 pi 的工具名（小写 `read`）会被转换为 Claude Code 的工具名（大写 `Read`）发送给 Anthropic API，然后再转换回来。这是利用 Anthropic OAuth 通道的必要条件。

### 6.8 OpenAI Completions 兼容性层深度

`OpenAICompletionsCompat` 的约20个开关覆盖了大量自建模型的变体：

```typescript
export interface OpenAICompletionsCompat {
  supportsStore?: boolean;                    // OpenAI 特有
  supportsDeveloperRole?: boolean;            // developer vs system role
  supportsReasoningEffort?: boolean;          // reasoning_effort 参数
  supportsUsageInStreaming?: boolean;         // stream_options.include_usage
  supportsFinishReason?: boolean;             // 流式 finish_reason
  maxTokensField?: "max_completion_tokens" | "max_tokens";
  requiresToolResultName?: boolean;           // tool result 是否需要 name 字段
  requiresAssistantAfterToolResult?: boolean; // tool result 后是否需要 assistant 消息
  requiresThinkingAsText?: boolean;           // thinking 是否转为 <thinking> 标签
  thinkingFormat?: "openai" | "openrouter" | "deepseek" | "together" | "baseten" | "zai" | "qwen" | "chat-template" | "qwen-chat-template" | "string-thinking" | "ant-ling";
  supportsThinkingTokenBudget?: boolean;      // thinking_token_budget 字段
  supportsOpenAIGrammarTools?: boolean;       // Lark/regex 语法约束工具
  supportsStrictMode?: boolean;               // strict JSON schema
  cacheControlFormat?: "anthropic";           // Anthropic 风格的缓存控制
  sendSessionAffinityHeaders?: boolean;       // session affinity
  // ... 更多
}
```

`thinkingFormat` 是最复杂的兼容性开关，因为它映射了11种不同的推理参数格式。例如：
- **`"deepseek"`**: `{ thinking: { type: "enabled" }, reasoning_effort: "high" }`
- **`"qwen"`**: `{ enable_thinking: true }`
- **`"chat-template"`**: `{ chat_template_kwargs: { enable_thinking: true } }`
- **`"ant-ling"`**: `{ reasoning: { effort: "high" } }`（仅当映射的 effort 非 null）

这使得 pi 可以用同一套代码对接 vLLM、SGLang、llama.cpp、Ollama 等各种 OpenAI 兼容服务器。

### 6.9 Cost 计算与分层定价

```typescript
// packages/ai/src/models.ts:878
export function calculateCost<TApi extends Api>(model: Model<TApi>, usage: Usage): Usage["cost"] {
  const inputTokens = usage.input + usage.cacheRead + usage.cacheWrite;
  let rates: ModelCostRates = model.cost;
  let matchedThreshold = -1;
  // 分层定价：输入 token 超过阈值时使用更低的费率
  for (const tier of model.cost.tiers ?? []) {
    if (inputTokens > tier.inputTokensAbove && tier.inputTokensAbove > matchedThreshold) {
      rates = tier;
      matchedThreshold = tier.inputTokensAbove;
    }
  }
  // Anthropic 1h cache write 按 2x input 费率计算
  const longWrite = usage.cacheWrite1h ?? 0;
  const shortWrite = usage.cacheWrite - longWrite;
  usage.cost.cacheWrite = (rates.cacheWrite * shortWrite + rates.input * 2 * longWrite) / 1000000;
  // ...
}
```

这展示了 pi 对 Anthropic 1h cache write 的特殊处理——Anthropic 对 1 小时缓存写入收取 2 倍基础输入费率。

### 6.10 `Models` 接口的 Auth 解析流程

```typescript
// packages/ai/src/models.ts:636
private async applyAuth<TOptions>(model, options) {
  this.requireProvider(model);
  const resolution = await this.getAuth(model, {
    apiKey: options?.apiKey,
    env: options?.env,
    signal: options?.signal,
  });
  if (!resolution) throw new ModelsError("auth", `Provider is not configured: ${model.provider}`);

  // 优先级：显式 apiKey > auth 解析的 apiKey > provider headers
  const apiKey = options?.apiKey ?? resolution.auth.apiKey;
  let headers = mergeHeaders(resolution.auth.headers, options?.headers);
  if (options?.transformHeaders) headers = await options.transformHeaders(headers ?? {});

  const requestModel = resolution.auth.baseUrl
    ? { ...model, baseUrl: resolution.auth.baseUrl }  // auth 可以覆盖 baseUrl
    : model;

  return { requestModel, requestOptions: { ...options, apiKey, headers, env } };
}
```

这展示了 pi 的 auth 解析链：`Credential Store → Provider Auth → Model Headers → Request Options`，每一层都可以覆盖前一层的值。`transformHeaders` 最后运行，允许应用层做全局 header 注入（如 telemetry）。

### 对 laew 的借鉴价值

1. **laew 目前只有 Anthropic + OpenAI 两个协议**：借鉴 pi 的 `createProvider` + `ProviderStreams` 模式，可以轻松扩展到 Bedrock / Google / Mistral 等。
2. **兼容性开关体系**：pi 的 `OpenAICompletionsCompat` 有约 20 个开关（supportsStore / supportsDeveloperRole / supportsReasoningEffort 等），这些对于对接 vLLM / Ollama 等自建模型非常有用。laew 可以增加类似的兼容性层。
3. **AssistantMessageEventStream 统一流式协议**：laew 目前在 `llm/anthropic.rs` 和 `llm/openai.rs` 中分别处理流式，转换为统一消息后再进入 agent 循环。这个设计与 pi 一致，但 pi 的事件粒度更细（text_start/delta/end 分离），对 TUI 实时渲染更友好。
4. **动态模型刷新**：laew 的 provider 表是静态的。如果需要支持"云端 API 自动发现模型"，可以借鉴 pi 的 `refreshModels` + `ModelsStore` 模式。
5. **OAuth 支持**：laew 目前只有 API Key 认证。如果未来需要支持 GitHub Copilot 等 OAuth 场景，pi 的 `CredentialStore` + `resolveProviderAuth` 架构值得参考。
6. **thinkingFormat 兼容性**：laew 目前的 thinking/reasoning 支持仅限 Anthropic 和 OpenAI 两种格式。如果要对接 DeepSeek / Qwen 等模型，需要类似 pi 的 `thinkingFormat` 多格式映射。
7. **分层定价计算**：laew 的成本追踪目前是简单的 input * rate + output * rate。可以借鉴 pi 的 `ModelCostTier` 支持 Anthropic 等的分层定价。

1. **laew 目前只有 Anthropic + OpenAI 两个协议**：借鉴 pi 的 `createProvider` + `ProviderStreams` 模式，可以轻松扩展到 Bedrock / Google / Mistral 等。
2. **兼容性开关体系**：pi 的 `OpenAICompletionsCompat` 有约 20 个开关（supportsStore / supportsDeveloperRole / supportsReasoningEffort 等），这些对于对接 vLLM / Ollama 等自建模型非常有用。laew 可以增加类似的兼容性层。
3. **AssistantMessageEventStream 统一流式协议**：laew 目前在 `llm/anthropic.rs` 和 `llm/openai.rs` 中分别处理流式，转换为统一消息后再进入 agent 循环。这个设计与 pi 一致，但 pi 的事件粒度更细（text_start/delta/end 分离），对 TUI 实时渲染更友好。
4. **动态模型刷新**：laew 的 provider 表是静态的。如果需要支持"云端 API 自动发现模型"，可以借鉴 pi 的 `refreshModels` + `ModelsStore` 模式。
5. **OAuth 支持**：laew 目前只有 API Key 认证。如果未来需要支持 GitHub Copilot 等 OAuth 场景，pi 的 `CredentialStore` + `resolveProviderAuth` 架构值得参考。

---

## 总结：pi 的三个核心设计决策

1. **Skill 是文本注入，不是工具调用**：用最少的抽象（Markdown）覆盖知识管理场景，避免了 MCP 的协议复杂度。

2. **Lane 是 session tree 的命名分支**：不是独立的并行执行线程，而是共享同一个 session tree 的不同视角。这使得分支探索、回溯、摘要都变得自然。

3. **Harness 是状态容器，不是编排函数**：面向对象的 Harness 持有 tools/models/resources/session 等全部状态，支持运行时动态修改（换模型/加工具/切换 thinking level）。

这三个决策共同支撑了 pi 的核心特征：**极简 Skill + 并发 Lane + 可组合 Harness**。

---

## 附录：pi 的类型系统设计亮点

### A. TaggedError 模式

pi 使用了一种优雅的错误类型模式：

```typescript
export class LaneBusy extends TaggedError("LaneBusy")<{
  lane: string;
  operationId: string;
  operationKind: "run" | "compaction" | "navigation";
  message: string;
}> {}
```

`TaggedError` 是一个高阶函数，返回一个继承自 `Error` 的类，带有：
- `tag` 字段（字符串字面量，如 `"LaneBusy"`）用于 runtime 类型判断
- 泛型参数定义的附加字段
- 自动的 `name` 属性设置

这比 TypeScript 的 discriminated union 更适合错误处理，因为 `instanceof` 检查在 JS 中是可靠的。

### B. Result 类型

pi 定义了自己的 `Result` 类型（类似 Rust 的 `Result<T, E>`）：

```typescript
export type Result<TValue, TError> =
  | { ok: true; value: TValue }
  | { ok: false; error: TError };
```

配合 `ok()` / `err()` / `getOrThrow()` / `getOrUndefined()` 工具函数，形成了函数式的错误处理风格。`FileSystem` 和 `Shell` 接口的所有方法都返回 `Result`，永不抛出异常——这是 pi 对 IO 操作的**显式错误处理**约定。

### C. TypeBox Schema

pi 使用 `typebox` 库定义工具参数 schema：
```typescript
const bashSchema = Type.Object({
  command: Type.String({ description: "Bash command to execute" }),
  timeout: Type.Optional(Type.Number({ description: "Timeout in seconds" })),
});
```

TypeBox 同时生成 TypeScript 类型（`Static<typeof bashSchema>`）和 JSON Schema，避免了手写两份定义。`validateToolArguments` 使用 TypeBox 的运行时验证器检查参数合法性。

---

## 附录：代码量与复杂度分布

| 模块 | 文件数 | 核心行数 | 复杂度 |
|------|--------|----------|--------|
| Skill 系统 | 3 | ~450 | 中（文件遍历 + YAML 解析） |
| Lane 模型 | 5 | ~1200 | 高（事件溯源 + 状态还原 + 验证） |
| Harness | 4 | ~800 | 中（接口定义 + 惰性初始化） |
| 压缩 + 分支摘要 | 5 | ~900 | 高（切割点算法 + LLM 摘要生成） |
| Agent 主循环 | 2 | ~800 | 高（双层循环 + 并行工具 + 流式处理） |
| 工具系统 | 6 | ~700 | 中（4 个工具 + 文件锁 + 参数兼容） |
| Provider 适配 | 10+ | ~3500 | 极高（10 个 API 适配器 + 认证 + 流式） |

---

## 附录：pi 的反模式与局限性

通过源码分析，pi 存在以下值得关注的设计局限：

1. **AgentHarness 基类是空壳**：`AgentHarness` 的几乎所有方法（prompt / skill / compact / navigateTree / steer / followUp 等）都调用 `this.unavailable()`，直接抛出 `HarnessNotImplemented`。真正的实现在更上层的 `server` 包中。这意味着 `agent` 包本身是**不可独立运行**的，必须依赖应用层注入完整实现。

2. **Hooks 是占位符**：`AgentHarness.hooks` 被赋值为 `UnavailableRegistry`，所有 `hooks.on(...)` 调用都会抛异常。11 个生命周期钩子（`before_run` / `after_tool` 等）的类型定义存在，但基类没有实现。这暗示完整 Harness 的实现可能在私有仓库中。

3. **Tool result 截断硬编码**：`serializeConversation` 中 tool result 被硬编码截断到 2000 字符。对于需要精确上下文的场景（如 diff 内容），这可能导致信息丢失。

4. **Token 估算粗糙**：`estimateTokens` 使用 `字符数 / 4` 的启发式，对中文/日文等多字节语言不准确。pi 没有使用 tiktoken 等精确 tokenizer。

5. **OAuth 身份伪装有风险**：pi 伪装为 Claude Code CLI 使用 Anthropic OAuth，这可能违反 Anthropic 的服务条款。`claudeCodeVersion` 硬编码为 `"2.1.251"`，需要随 Claude Code 版本更新。

6. **Session Storage 抽象不完整**：`SessionStorage` 接口定义了完整的读写操作，但 `session-backends/sqlite-node` 包的实现似乎仍在开发中（从 CHANGELOG 和测试覆盖看）。

---

## 附录：pi 与 laew 架构对照速查

| 维度 | pi | laew |
|------|----|------|
| Skill/指令管理 | Markdown + frontmatter → 系统提示词注入 | SystemPrompt 静态拼接 + project_context 五级链 |
| 并发模型 | Lane（session tree 分支） | SubAgent（一次性执行单元） |
| 运行时容器 | AgentHarness（OOP 状态容器） | MultiAgentOrchestrator（编排函数） |
| 上下文压缩 | shouldCompact + findCutPoint + LLM 摘要 | 无（SessionContext 自由格式摘要） |
| 工具钩子 | beforeToolCall / afterToolCall | 无（直接执行） |
| 协议适配 | 10+ API，20+ 兼容性开关 | Anthropic + OpenAI 双协议 |
| 文件操作保护 | withFileMutationQueue（per-file 排他锁） | 无 |
| 崩溃恢复 | Record Log + reduceLaneState 事件溯源 | 无 |
| 流式输出 | AssistantMessageEventStream（细粒度事件） | 统一消息模型后进入 agent 循环 |
| 认证管理 | API Key + OAuth + AWS SigV4 | API Key only |
| 错误处理 | Result 类型（不抛异常） | Result + AgentError enum |
| 参数验证 | TypeBox 运行时验证 + prepareArguments | serde 静态反序列化 |
| 会话持久化 | JSONL + SQLite（session-backends） | SQLite（session_memory 表） |
| TUI 集成 | drive: "manual" 逐步推进 | 阻塞式等待 agent 完成 |
| 测试策略 | vitest + harness conformance test | cargo test + run_e2e.sh tmux |

---

## 附录：关键源码文件路径索引

| 主题 | 文件路径 |
|------|----------|
| Skill 接口定义 | `packages/agent/src/harness/types.ts:46-57` |
| Skill 加载引擎 | `packages/agent/src/harness/skills.ts:50-76` |
| Skill 系统提示词注入 | `packages/agent/src/harness/system-prompt.ts:3-25` |
| Skill 显式调用 | `packages/agent/src/harness/agent-harness.ts:368-370` |
| AgentLane 接口 | `packages/agent/src/harness/agent-harness.ts:271-303` |
| Lane 状态还原器 | `packages/agent/src/harness/reducer.ts:506-667` |
| Record Log 验证 | `packages/agent/src/harness/reducer.ts:312-390` |
| Harness 创建 | `packages/coding-agent/src/server/create-harness.ts:80-161` |
| Harness 选项 | `packages/agent/src/harness/agent-harness.ts:243-263` |
| Hook 名称枚举 | `packages/agent/src/harness/agent-harness.ts:198-209` |
| 压缩触发判断 | `packages/agent/src/harness/compaction/compaction.ts:247-250` |
| 压缩切割点算法 | `packages/agent/src/harness/compaction/compaction.ts:374-422` |
| 压缩摘要生成 | `packages/agent/src/harness/compaction/compaction.ts:501-526` |
| 分支摘要生成 | `packages/agent/src/harness/compaction/branch-summarization.ts:208-280` |
| 上下文构建 | `packages/agent/src/harness/session/context.ts:90-100` |
| Agent 主循环 | `packages/agent/src/agent-loop.ts:156-273` |
| 流式响应处理 | `packages/agent/src/agent-loop.ts:279-370` |
| 工具调用流程 | `packages/agent/src/agent-loop.ts:409-561` |
| 截断消息保护 | `packages/agent/src/agent-loop.ts:379-404` |
| 文件写入串行化 | `packages/agent/src/harness/tools/file-mutation-queue.ts:29-56` |
| BashTool | `packages/agent/src/harness/tools/bash.ts:51-161` |
| ReadTool | `packages/agent/src/harness/tools/read.ts:45-144` |
| EditTool | `packages/agent/src/harness/tools/edit.ts:90-140` |
| WriteTool | `packages/agent/src/harness/tools/write.ts:15-39` |
| Model 类型 | `packages/ai/src/types.ts:830-859` |
| Provider 接口 | `packages/ai/src/models.ts:97-149` |
| Models 接口 | `packages/ai/src/models.ts:156-223` |
| createProvider 工厂 | `packages/ai/src/models.ts:762-862` |
| Anthropic 适配器 | `packages/ai/src/api/anthropic-messages.ts:501-798` |
| OpenAI 适配器 | `packages/ai/src/api/openai-completions.ts:311-400+` |
| 模型目录持久化 | `packages/ai/src/models-store.ts:1-46` |
| Skill 文件示例 | `.pi/skills/add-llm-provider.md` |
| PromptTemplate 示例 | `.pi/prompts/pr.md` |
