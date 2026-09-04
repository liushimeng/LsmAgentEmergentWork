# Pi 源码深度分析（8 维度）

> 分析对象：`/usr/local/LsmGitOpenSource/pi`（`@earendil-works/pi-coding-agent` + `@earendil-works/pi-agent-core`）
> 定位：**极简终端编码脚手架（harness）**，核心哲学是"**无内置=subagent/plan/mcp**"，一切靠 **Extensions / Skills / Prompt Templates / Themes** 扩展。
> 与 `laew` 的多 Agent 内置架构形成鲜明对比——pi 把"多 Agent"全部推给扩展层。

---

## 1. 多轮对话的实现

### 关键文件
- `packages/agent/src/agent-loop.ts`（803 行）— 协议无关的核心循环
- `packages/agent/src/agent.ts`（~350 行）— 有状态包装器，持有 transcript 与队列
- `packages/agent/src/types.ts`（~430 行）— `AgentLoopConfig` 全部回调定义
- `packages/coding-agent/src/core/agent-session.ts`（~3000 行）— 会话级编排

### 核心设计：双层循环 + 回调钩子

`agent-loop.ts:156-273` 的 `runLoop` 是核心：

```
外层 while(true)：
  内层 while(hasMoreToolCalls || pendingMessages.length > 0)：
    1. prepareNextTurn（可替换 context/model/thinking）
    2. 注入 pendingMessages（steering）
    3. streamAssistantResponse → 流式产出 AssistantMessage
    4. executeToolCalls → 并行/串行执行
    5. emit turn_end
    6. shouldStopAfterTurn? → 退出
  检查 getFollowUpMessages → 有则继续外层
```

**关键回调**（`types.ts:149-294`）：
- `convertToLlm(messages: AgentMessage[]): Message[]` — **AgentMessage→LLM 消息的边界**，每个 Agent 自定义（`agent-loop.ts:293`）
- `transformContext` — 在 convertToLlm 之前对 AgentMessage 做裁剪/注入（`agent-loop.ts:288`）
- `shouldStopAfterTurn` — 本轮结束后是否退出（`agent-loop.ts:252`）
- `prepareNextTurn` — 可替换下一轮 context/model/thinking（`agent-loop.ts:177`）
- `getSteeringMessages` / `getFollowUpMessages` — 运行中注入 / 结束后追加（`agent-loop.ts:257/261`）

### 流式转换边界（agent-loop.ts:279-370）

`streamAssistantResponse` 是**唯一**把 `AgentMessage[]` 转成 LLM `Message[]` 的地方：
```ts
let messages = context.messages;
if (config.transformContext) messages = await config.transformContext(messages, signal);
const llmMessages = await config.convertToLlm(messages);  // 边界
```

### Steering / FollowUp 队列（agent.ts:125-159）

`PendingMessageQueue` 支持两种 **QueueMode**（`types.ts:50`）：
- `"all"` — 一次排空
- `"one-at-a-time"` — 每次只取一条（默认）

`Agent` 暴露 `steer()` / `followUp()`（`agent.ts:282-290`），由 `AgentSession.prompt()` 在 streaming 时调用（`agent-session.ts:1210-1220`）。

### 设计要点
1. **AgentMessage 全程流转，仅在 LLM 边界转换** — 与 laew 的"统一消息模型"异曲同工
2. **双层循环**：内层处理 tool_calls，外层处理 follow-up，支持"agent 结束后再追加一轮"
3. **steering 机制**：用户可在 agent 运行中输入，消息作为"转向"注入下一轮（`getSteeringMessages`）
4. **prepareNextTurn 可换模型**：为"按档换模型"提供钩子（pi 自身未用，但扩展可）

---

## 2. Context 的管理和实现

### 关键文件
- `packages/coding-agent/src/core/compaction/compaction.ts`（1012 行）— 压缩主逻辑
- `packages/coding-agent/src/core/compaction/branch-summarization.ts`（381 行）— 分支切换摘要
- `packages/coding-agent/src/core/session-manager.ts`（~1300 行）— Session 树 + buildSessionContext
- `packages/coding-agent/src/core/system-prompt.ts`（170 行）— 系统提示词组装
- `packages/coding-agent/src/core/resource-loader.ts`（~300 行）— 项目上下文文件发现

### Token 估算（compaction.ts:146-306）

**双轨制**：
- 有真实 `usage.totalTokens` 时用真实值（`calculateContextTokens`）
- 否则用 `chars/4` 启发式（`estimateTokens`），按 role 分类估算（text/thinking/toolCall/bashExecution/branchSummary/compactionSummary）

```ts
export function estimateContextTokens(messages: AgentMessage[]): ContextUsageEstimate {
  const usageInfo = getLastAssistantUsageInfo(messages);
  // usageTokens + trailingTokens（最后 usage 之后的消息估算）
}
```

### 压缩触发（compaction.ts:235-238）

```ts
export function shouldCompact(contextTokens, contextWindow, settings): boolean {
  if (!settings.enabled) return false;
  return contextTokens > contextWindow - settings.reserveTokens;  // 默认 reserve 16384
}
```

默认设置（`compaction.ts:132-136`）：`reserveTokens: 16384`、`keepRecentTokens: 20000`。

### 切点算法（compaction.ts:403-461）

`findCutPoint` 从 newest 向 oldest 累加 token，超 `keepRecentTokens` 时切。**只切 user/assistant/bashExecution/custom/branchSummary/compactionSummary**，**绝不切 toolResult**（`isCutPointMessage`）。支持"split turn"——切到 turn 中段时会额外生成 turn-prefix 摘要。

### 摘要生成（compaction.ts:467-726）

- `SUMMARIZATION_PROMPT`（固定格式：Goal / Constraints / Progress / Key Decisions / Next Steps / Critical Context）
- `UPDATE_SUMMARIZATION_PROMPT` — 有 previousSummary 时增量更新
- 通过 `completeSummarization`（`compaction.ts:579-599`）调用 LLM，带 retry 策略
- 同时追踪 `readFiles` / `modifiedFiles`（`CompactionDetails`），附在摘要末尾

### 分支摘要（branch-summarization.ts）

树状 Session 导航时，**离开的分支**会被摘要（`collectEntriesForBranchSummary` + `generateBranchSummary`），避免上下文丢失。`prepareBranchEntries` 从 newest→oldest 收集，尊重 token budget。

### buildSessionContext（session-manager.ts:461-470）

```ts
export function buildSessionContext(entries, leafId?, byId?): SessionContext {
  const path = buildSessionPath(entries, leafId, byId);
  const { thinkingLevel, model } = getSessionContextSettings(path);
  const messages = buildContextEntries(entries, leafId, byId).flatMap(sessionEntryToContextMessages);
  return { messages, thinkingLevel, model };
}
```

Session 是**树状结构**（支持 branch），`buildSessionPath` 从 leaf 向 root 回溯，处理 compaction/branch_summary 节点。

### 项目上下文文件（resource-loader.ts:71-157）

**候选链**：`AGENTS.override.md` → `AGENTS.md` → `AGENTS.MD` → `CLAUDE.md` → `CLAUDE.MD`
- 全局：`~/.pi/agent/` 目录
- 祖先链：从 cwd 向上遍历到根目录
- **git worktree 感知**：`findShadowedContextFile` 避免主仓库与 worktree 重复注入

注入到 system prompt（`system-prompt.ts:151-159`）：
```xml
<project_context>
Project-specific instructions and guidelines:
<project_instructions path="...">content</project_instructions>
</project_context>
```

### 设计要点
1. **压缩是纯函数**（`compaction.ts` 头部注释），I/O 由 SessionManager 处理
2. **双轨 token 估算**：真实 usage + chars/4 启发式
3. **树状 Session + 分支摘要**：支持回溯时保留被放弃分支的语义
4. **增量摘要**：有 previousSummary 时走 update prompt，避免信息丢失
5. **文件操作追踪**：readFiles/modifiedFiles 贯穿 compaction 与 branch summary

---

## 3. Yolo 识别 / 任务分类

### 结论：**Pi 没有内置的 Yolo / 意图识别 / 任务分类层**

搜索 `intent` / `classify` / `task level` / `simple|medium|hard` 在 `packages/` 仅在 harness reducer 出现（资源意图，非任务分类）。

### 关键证据
- `packages/agent/src/harness/reducer.ts` 的 `intent` 是资源 provision 意图（`run`/`compaction`/`navigation`），**不是**任务分类
- `packages/coding-agent/src/core/system-prompt.ts` 无任何分类提示
- `packages/coding-agent/README.md:497`："Pi is aggressively extensible... Features that other tools bake in can be built with extensions, skills..."

### 替代方案（扩展层）

| 需求 | Pi 的替代 |
|------|-----------|
| 意图识别 | 无内置；可在 system prompt 中引导模型自行判断 |
| 任务分类 | 无内置；`plan-mode` 扩展提供"只读探索"一种分类 |
| 入口层 | `AgentSession.prompt()`（`agent-session.ts:1159`）是统一入口，但不做分类 |

### 设计要点
1. **极简核心**：pi 把"任务分类"视为用户/扩展该做的事
2. **system prompt 是主要控制面**：通过 `buildSystemPrompt` 注入 guidelines
3. **扩展可拦截 input**：`input` 事件（`agent-session.ts:1186`）可在 LLM 调用前改写/分流
4. **与 laew 对比**：laew 内置 Yolo 三步分析 + 三档分类；pi 完全交给模型自主判断

---

## 4. 质检检查

### 结论：**Pi 没有内置质检层（Quality-Check）**

搜索 `qa` / `quality` 仅在示例扩展出现：`examples/extensions/overlay-qa-tests.ts`（1451 行）——但这是 **TUI overlay 的 QA 测试工具**，不是代码质检。

### 关键文件
- `packages/coding-agent/examples/extensions/overlay-qa-tests.ts` — 覆盖层定位/溢出/动画测试
- `packages/coding-agent/examples/extensions/subagent/agents/reviewer.md` — **示例** reviewer agent（非内置）

### 替代方案

| 质检需求 | Pi 的替代 |
|---------|-----------|
| 代码 review | `subagent` 扩展的 `reviewer.md` agent |
| 测试运行 | 直接 `bash` 工具跑测试 |
| 静态分析 | 通过 `bash` 调用 linter |
| 覆盖层 QA | `overlay-qa-tests.ts` 扩展（仅 TUI） |

### 设计要点
1. **质检不是核心关注**：pi 定位为"编码脚手架"，质检由外部工具/bash 承担
2. **subagent 扩展可实现 review 流**：`prompts/implement-and-review.md` 示例展示 worker→reviewer→worker 链
3. **与 laew 对比**：laew 内置 Quality-Check Agent（必经门控）；pi 把质检交给 bash/扩展

---

## 5. 任务拆解

### 结论：**Pi 没有内置任务拆解引擎**，但提供 **subagent 扩展** 作为一等公民机制。

### 关键文件
- `packages/coding-agent/examples/extensions/subagent/index.ts`（1039 行）— 子进程 subagent 工具
- `packages/coding-agent/examples/extensions/subagent/agents.ts`（158 行）— Agent 发现
- `packages/coding-agent/examples/extensions/subagent/agents/*.md` — 示例 agent（planner/reviewer/scout/worker）
- `packages/coding-agent/examples/extensions/subagent/prompts/*.md` — 工作流预设
- `packages/coding-agent/examples/extensions/todo.ts`（~270 行）— 任务列表工具
- `packages/coding-agent/examples/extensions/plan-mode/index.ts` — 只读规划模式

### Subagent 扩展核心设计（subagent/index.ts）

**三种模式**（`subagent/index.ts:459-469`）：
```ts
const SubagentParams = Type.Object({
  agent: Type.Optional(Type.String()),   // single
  task: Type.Optional(Type.String()),
  tasks: Type.Optional(Type.Array(TaskItem)),  // parallel
  chain: Type.Optional(Type.Array(ChainItem)), // sequential
});
```

**执行方式**：每个 subagent 是**独立 `pi` 子进程**（`subagent/index.ts:344-421`）：
```ts
const proc = spawn(invocation.command, invocation.args, {
  cwd: cwd ?? defaultCwd, shell: false, stdio: ["ignore", "pipe", "pipe"],
});
```
- 通过 `--mode json -p --no-session` 让子进程输出 JSON 流
- 父进程逐行解析 `message_end` / `tool_result_end` 事件
- 支持 **并发限制**（`MAX_CONCURRENCY = 4`，`MAX_PARALLEL_TASKS = 8`）
- 支持 **abort 传播**（`signal` → `proc.kill`）

**Chain 模式**：`{previous}` 占位符替换（`subagent/index.ts:556`），前一步输出注入后一步。

### Agent 发现（subagent/agents.ts）

- **User agents**：`~/.pi/agent/agents/*.md`
- **Project agents**：`.pi/agents/*.md`（向上查找）
- Frontmatter：`name` / `description` / `tools` / `model`
- **Trust 机制**：project agents 需用户确认（`subagent/index.ts:520-548`）

### Todo 扩展（todo.ts）

**状态存储在 tool result details 中**（`todo.ts:24-29`），而非外部文件——这样 branch 时状态自动正确：
```ts
interface TodoDetails { action: "list"|"add"|"toggle"|"clear"; todos: Todo[]; nextId: number; }
```
通过 `pi.on("session_start")` / `pi.on("session_tree")` 重建状态（`todo.ts:132-133`）。

### Plan Mode 扩展（plan-mode/index.ts）

- 只读模式：禁用 `edit` / `write`，bash 限制为安全命令
- 提取 `Plan:` 章节的编号步骤，支持 `[DONE:n]` 标记
- 通过 `pi.registerFlag("plan")` 提供启动标志

### 设计要点
1. **子进程隔离**：每个 subagent 有独立 context window，避免污染主会话
2. **Markdown 即 agent 定义**：`.md` 文件 + frontmatter，极低门槛
3. **三种编排模式**：single / parallel / chain，覆盖常见拆解场景
4. **Todo 状态与 session 树绑定**：利用 tool result details 实现分支安全
5. **与 laew 对比**：laew 内置 Plan→Main→SubAgent 编排；pi 用扩展实现，更灵活但需用户装配

---

## 6. 任务分类

### 结论：**Pi 没有内置任务分类（simple/medium/hard 或类似分档）**

搜索 `task level` / `classify` / `simple|medium|hard` 在源码中**零命中**（除 harness reducer 的资源意图）。

### 关键证据
- `packages/coding-agent/README.md:15`："Pi ships with powerful defaults but skips features like sub agents and plan mode."
- `packages/coding-agent/src/core/system-prompt.ts` 无任何分档提示
- 模型选择通过 `/model` 命令或 `--model` 标志，**不按任务分档**

### 替代方案

| 分类需求 | Pi 的替代 |
|---------|-----------|
| 按复杂度分档 | 无；用户手动切换 model |
| 自动分类 | 无；模型自行判断 |
| Plan 模式 | `plan-mode` 扩展（一种"手动分档"） |

### 设计要点
1. **极简哲学**：pi 认为"任务分类"应由用户或模型自主判断
2. **Model 选择是主要杠杆**：通过 `Ctrl+L` 切换模型应对不同复杂度
3. **与 laew 对比**：laew 内置三档分类（simple/medium/hard）+ 对应流程；pi 无此层

---

## 7. 工具调用

### 关键文件
- `packages/agent/src/agent-loop.ts:409-424` — 工具调度
- `packages/agent/src/types.ts:42/269/409` — `ToolExecutionMode` 类型
- `packages/coding-agent/src/core/tools/*.ts` — 内置工具集
- `packages/coding-agent/src/core/tools/tool-definition-wrapper.ts` — 工具包装

### ToolExecutionMode（types.ts:42）

```ts
export type ToolExecutionMode = "sequential" | "parallel";
```

**决策逻辑**（`agent-loop.ts:417-423`）：
```ts
const hasSequentialToolCall = toolCalls.some(
  (tc) => currentContext.tools?.find((t) => t.name === tc.name)?.executionMode === "sequential"
);
if (config.toolExecution === "sequential" || hasSequentialToolCall) {
  return executeToolCallsSequential(...);  // 任一工具要求 sequential 则整批串行
}
return executeToolCallsParallel(...);
```

### 并行执行（agent-loop.ts:487-561）

1. **顺序 preflight**：先逐个 `prepareToolCall`（校验参数、调用 `beforeToolCall`）
2. **Promise.all 并发执行**：`executePreparedToolCall`
3. **按原始顺序回填结果**：`orderedFinalizedCalls` 保持 assistant 消息中的工具顺序
4. **流式更新**：`tool_execution_update` 事件在并发中即时发出

### 串行执行（agent-loop.ts:431-485）

逐个执行，每步完整走完 `prepare → execute → finalize → emit` 下一步。

### beforeToolCall / afterToolCall 钩子（agent-loop.ts:607-765）

**beforeToolCall**（`agent-loop.ts:626-654`）：
- 参数校验后、执行前调用
- 可返回 `{ block: true, reason, terminate }` 阻止执行
- 接收 abort signal

**afterToolCall**（`agent-loop.ts:731-758`）：
- 执行后、`tool_execution_end` 事件前调用
- 可覆盖 `content` / `details` / `isError` / `usage` / `terminate`
- **非 deep merge**：省略字段保持原值

### 内置工具集（coding-agent/src/core/tools/）

| 工具 | 文件 | 说明 |
|------|------|------|
| `bash` | `bash.ts` | Shell 执行，含 spawn hook |
| `read` | `read.ts` | 文件读取，支持 AGENTS.md 压缩 |
| `write` | `write.ts` | 文件写入 |
| `edit` / `edit-diff` | `edit.ts` / `edit-diff.ts` | 精确编辑 |
| `grep` | `grep.ts` | 内容搜索 |
| `find` | `find.ts` | 文件查找 |
| `ls` | `ls.ts` | 目录列表 |
| `powershell` | `powershell.ts` | Windows 支持 |

### 工具定义包装（tool-definition-wrapper.ts）

```ts
executionMode: definition.executionMode,  // 透传 per-tool 模式
```

### 设计要点
1. **per-tool + global 双轨**：`config.toolExecution` 全局默认，`tool.executionMode` 单工具覆盖
2. **preflight 串行 + 执行并行**：保证 beforeToolCall 顺序副作用安全
3. **terminate 机制**：工具可标记 `terminate: true`，整批结束后退出循环（`shouldTerminateToolBatch`，`agent-loop.ts:589-591`）
4. **truncated 保护**：`length` stop 时整批工具调用失败（`failToolCallsFromTruncatedMessage`，`agent-loop.ts:379-404`）
5. **与 laew 对比**：laew 工具层更简单（Bash/Read/Write）；pi 工具集更丰富（grep/find/ls/edit-diff）

---

## 8. MCP 设计与 SKILL 设计

### 8.1 MCP：**明确无 MCP**

README.md:499 原文：
> **No MCP.** Build CLI tools with READMEs (see Skills), or build an extension that adds MCP support. [Why?](https://mariozechner.at/posts/2025-11-02-what-if-you-dont-need-mcp/)

**替代方案**：Skills（见下）+ Extensions（可自建 MCP 桥接）

### 8.2 Skill 设计

#### 关键文件
- `packages/coding-agent/src/core/skills.ts`（507 行）— 用户侧 Skill 加载与提示注入
- `packages/agent/src/harness/skills.ts`（386 行）— Harness 侧 Skill 加载（异步 ExecutionEnv）
- `packages/coding-agent/src/core/system-prompt.ts:355-390` — `formatSkillsForPrompt`

#### Skill 规范遵循

遵循 [Agent Skills 标准](https://agentskills.io/)：
- **文件名**：`SKILL.md`（声明式）或目录内 `.md`（需 frontmatter）
- **Frontmatter**：`name` / `description` / `disable-model-invocation`
- **name 规则**（`skills.ts:92-112`）：≤64 字符、小写+数字+连字符、无首尾/连续连字符
- **description 规则**（`skills.ts:117-127`）：≤1024 字符、必填

#### 发现路径（skills.ts:450-501）

```ts
if (includeDefaults) {
  addSkills(loadSkillsFromDirInternal(join(resolvedAgentDir, "skills"), "user", true));       // ~/.pi/agent/skills/
  addSkills(loadSkillsFromDirInternal(resolve(resolvedCwd, CONFIG_DIR_NAME, "skills"), "project", true)); // .pi/skills/
}
```

**搜索规则**（`skills.ts:168-275`）：
- 目录含 `SKILL.md` → 视为 skill root，**不再递归**
- 否则加载根目录 `.md` 文件
- 递归子目录找 `SKILL.md`
- 遵循 `.gitignore` / `.ignore` / `.fdignore`

#### 提示注入格式（skills.ts:355-381）

```xml

The following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.
When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
  <skill>
    <name>my-skill</name>
    <description>...</description>
    <location>/path/to/SKILL.md</location>
  </skill>
</available_skills>
```

**关键设计**：Skill 内容**不直接注入**，只注入 name/description/location，模型按需 `read` 加载——**节省 token**。

#### disable-model-invocation（skills.ts:356）

```ts
const visibleSkills = skills.filter((s) => !s.disableModelInvocation);
```

标记 `disable-model-invocation: true` 的 skill **不出现在提示中**，只能通过 `/skill:name` 显式调用。

#### Harness 侧（agent/src/harness/skills.ts）

为**无文件系统环境**（如浏览器/WASM）设计，通过 `ExecutionEnv` 抽象（`fileInfo` / `listDir` / `readTextFile` / `canonicalPath` / `joinPath`）异步加载。

**Skill 调用格式**（`harness/skills.ts:38-41`）：
```ts
export function formatSkillInvocation(skill: Skill, additionalInstructions?: string): string {
  const skillBlock = `<skill name="${skill.name}" location="${skill.filePath}">
References are relative to ${dirnameEnvPath(skill.filePath)}.
${skill.content}
</skill>`;
  return additionalInstructions ? `${skillBlock}\n\n${additionalInstructions}` : skillBlock;
}
```

#### Skill 在 Agent 生命周期中的位置

- `AgentSession.prompt()` → `_expandSkillCommand()`（`agent-session.ts:1205`）展开 `/skill:name args`
- 展开后通过 `formatSkillInvocation` 包装成 `<skill>` 标签 user 消息
- 与 prompt templates 同级扩展（`expandPromptTemplate`）

### 设计要点
1. **Skill 是一等公民**：pi 把 MCP 的能力用 Skills + Extensions 替代
2. **延迟加载**：只注入元数据，模型按需 read——**token 经济**
3. **双轨加载**：用户侧（同步 fs）+ Harness 侧（异步 ExecutionEnv）
4. **命名空间隔离**：user / project / path 三种 source，同名碰撞产生 diagnostic
5. **显式 vs 隐式**：`disable-model-invocation` 控制是否可被模型自动调用
6. **与 laew 对比**：laew 无 Skill 系统；pi 的 Skill 是核心扩展机制，对标 MCP server

---

## 9. 综合对比（Pi vs Laew）

| 维度 | Pi | Laew |
|------|-----|------|
| 多轮对话 | 双层循环 + steering/followUp 队列 | 单循环 + 多 Agent 接力 |
| Context 管理 | 压缩 + 分支摘要 + 树状 Session | 无压缩（依赖模型 context window） |
| Yolo/意图识别 | **无内置** | 内置三步分析 |
| 质检 | **无内置** | 内置 Quality-Check Agent |
| 任务拆解 | subagent 扩展（子进程） | Plan→Main→SubAgent 内置 |
| 任务分类 | **无内置** | 三档（simple/medium/hard） |
| 工具调用 | 并行/串行 + before/after 钩子 | 简单顺序执行 |
| MCP | **明确无**，用 Skills 替代 | 无（直接 HTTP） |
| Skill | 一等公民，延迟加载 | 无 |
| 扩展性 | Extensions/Skills/Prompts/Themes | 仅工具/协议扩展 |
| 核心哲学 | **极简 + 极致可扩展** | **多 Agent 内置编排** |

---

## 10. 关键文件索引

| 文件 | 行数 | 职责 |
|------|------|------|
| `packages/agent/src/agent-loop.ts` | 803 | 协议无关核心循环 |
| `packages/agent/src/agent.ts` | ~350 | 有状态 Agent + 队列 |
| `packages/agent/src/types.ts` | ~430 | AgentLoopConfig 类型 |
| `packages/agent/src/harness/skills.ts` | 386 | Harness 侧 Skill 加载 |
| `packages/coding-agent/src/core/agent-session.ts` | ~3000 | 会话编排 |
| `packages/coding-agent/src/core/session-manager.ts` | ~1300 | Session 树 + buildSessionContext |
| `packages/coding-agent/src/core/compaction/compaction.ts` | 1012 | 压缩算法 |
| `packages/coding-agent/src/core/compaction/branch-summarization.ts` | 381 | 分支摘要 |
| `packages/coding-agent/src/core/skills.ts` | 507 | 用户侧 Skill 加载 |
| `packages/coding-agent/src/core/system-prompt.ts` | 170 | 系统提示词组装 |
| `packages/coding-agent/src/core/resource-loader.ts` | ~300 | 项目上下文发现 |
| `packages/coding-agent/examples/extensions/subagent/index.ts` | 1039 | Subagent 工具 |
| `packages/coding-agent/examples/extensions/subagent/agents.ts` | 158 | Agent 发现 |
| `packages/coding-agent/examples/extensions/todo.ts` | ~270 | Todo 工具 |
| `packages/coding-agent/examples/extensions/plan-mode/index.ts` | ~200 | Plan 模式 |
| `packages/coding-agent/examples/extensions/overlay-qa-tests.ts` | 1451 | TUI QA 测试 |

---

## 11. 对 Laew 的启示

1. **双层循环 + steering 队列**：laew 可借鉴"运行中注入消息"机制，实现用户中断/转向
2. **Token 估算 chars/4 启发式**：laew 的 `estimateContextTokens` 可复用此算法
3. **压缩的纯函数设计**：把压缩逻辑与 I/O 分离，便于测试
4. **Skill 延迟注入**：laew 的项目上下文注入可改为"只注入路径，按需 read"
5. **子进程 subagent**：laew 的 SubAgent-Work 可考虑进程隔离（而非仅 Agent 切换）
6. **树状 Session + 分支摘要**：laew 当前是线性上下文，可支持 branch/回溯
7. **beforeToolCall/afterToolCall 钩子**：laew 的工具层可加类似 hook（如 bash 命令审计）
