# Claude Code 核心机制深度分析（第二轮）

> **分析日期**: 2026-09-04
> **源码根路径**: `/usr/local/LsmGitOpenSource/claudecode`
> **核心文件数**: 约 40 个文件（直接引用分析）
> **分析范围**: Context 四级压缩管线 / Hook 系统 / Skill 系统 / MCP 架构 / Tool Runner / Multi-Agent

---

## 专题 1：Context 上下文管理（四级压缩管线）

### 核心文件清单

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/services/compact/compact.ts` | 1706 | 主压缩入口：`compactConversation` + `partialCompactConversation` |
| `src/services/compact/microCompact.ts` | 531 | 微压缩层：工具结果裁剪 + 时间触发型缓存编辑 |
| `src/services/compact/autoCompact.ts` | 352 | 自动压缩触发判断 + 阈值计算 |
| `src/services/compact/prompt.ts` | 375 | 压缩提示词生成（三种变体） |
| `src/services/compact/grouping.ts` | — | 消息按 API 轮次分组 |
| `src/services/compact/sessionMemoryCompact.ts` | — | Session Memory 压缩 |
| `src/services/compact/timeBasedMCConfig.ts` | — | 时间触发型微压缩配置 |
| `src/utils/context.ts` | — | `COMPACT_MAX_OUTPUT_TOKENS` 等常量 |
| `src/services/tokenEstimation.ts` | — | `roughTokenCountEstimation`（~4 chars/token） |

### 四级压缩架构总览

```
Level 1: Time-Based MC  ─→  间隔超时 → content-clear 旧工具结果
Level 2: Cached MC      ─→  工具数超阈值 → API cache_edits 远程删除
Level 3: Auto-Compact   ─→  token 超阈值 → LLM 全文摘要
Level 4: Partial Compact ─→  用户选择 → 按消息范围 LLM 摘要
```

### 第一级：Time-Based Microcompact

当两次对话间隔超过阈值（服务端缓存已失效），直接 content-clear 旧的工具结果：

```typescript
// microCompact.ts:422 - evaluateTimeBasedTrigger
export function evaluateTimeBasedTrigger(messages, querySource) {
  const config = getTimeBasedMCConfig();     // GrowthBook 配置
  if (!config.enabled || !querySource || !isMainThreadSource(querySource)) {
    return null;                              // 仅主线程触发
  }
  const lastAssistant = messages.findLast(m => m.type === 'assistant');
  const gapMinutes = (Date.now() - new Date(lastAssistant.timestamp).getTime()) / 60_000;
  if (!Number.isFinite(gapMinutes) || gapMinutes < config.gapThresholdMinutes) {
    return null;                              // 间隔不够
  }
  return { gapMinutes, config };
}
```

**压缩算法**（`maybeTimeBasedMicrocompact`，line 446）：
```typescript
const keepSet = new Set(compactableIds.slice(-keepRecent));  // 保留最近 N 个
const clearSet = new Set(compactableIds.filter(id => !keepSet.has(id)));
// 替换为常量：'[Old tool result content cleared]'
return { messages: result };
```

- **可压缩工具**（`COMPACTABLE_TOOLS`）：`Read`、`Bash`/`PowerShell`、`Grep`、`Glob`、`WebSearch`、`WebFetch`、`Edit`、`Write`
- **保留策略**：`config.keepRecent`（至少 1 个）

### 第二级：Cached Microcompact

利用 API 的 `cache_edits` 特性，远程删除工具结果而不破坏 prompt cache 前缀：

```typescript
// microCompact.ts:305 - cachedMicrocompactPath
async function cachedMicrocompactPath(messages, querySource) {
  const state = ensureCachedMCState();                    // 全局缓存 MC 状态
  const config = mod.getCachedMCConfig();                 // GrowthBook 配置

  // 注册新的工具结果
  for (const message of messages) {
    for (const block of message.message.content) {
      if (block.type === 'tool_result' && !state.registeredTools.has(block.tool_use_id)) {
        mod.registerToolResult(state, block.tool_use_id);
      }
    }
  }

  const toolsToDelete = mod.getToolResultsToDelete(state); // 基于触发/保留阈值
  if (toolsToDelete.length > 0) {
    const cacheEdits = mod.createCacheEditsBlock(state, toolsToDelete);
    pendingCacheEdits = cacheEdits;    // 推送到 API 层
  }
  return { messages };  // 消息不变，cache_edits 在 API 层生效
}
```

### 第三级：Auto-Compact

**触发判断**（`autoCompact.ts:160`）：

```typescript
export async function shouldAutoCompact(messages, model, querySource, snipTokensFreed) {
  // 排除：session_memory / compact / marble_origami 来源
  // 排除：禁用 auto-compact / reactive-only 模式 / context-collapse 模式
  const tokenCount = tokenCountWithEstimation(messages) - snipTokensFreed;
  const threshold = getAutoCompactThreshold(model);
  return tokenCount >= threshold;
}
```

**阈值计算**（`autoCompact.ts:72`）：

```typescript
export function getAutoCompactThreshold(model) {
  const effectiveContextWindow = getEffectiveContextWindowSize(model);
  // effectiveContextWindow = contextWindow - min(maxOutputTokens, 20000)
  return effectiveContextWindow - AUTOCOMPACT_BUFFER_TOKENS;  // 13000 token 缓冲
}
```

**执行流程**（`autoCompactIfNeeded`，line 241）：

```typescript
export async function autoCompactIfNeeded(messages, toolUseContext, cacheSafeParams, ...) {
  // 1. 断路器：连续失败 ≥ 3 次则停止重试
  if (tracking?.consecutiveFailures >= MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES) return;

  // 2. 先尝试 Session Memory 压缩（更轻量）
  const sessionMemoryResult = await trySessionMemoryCompaction(messages, ...);
  if (sessionMemoryResult) return { wasCompacted: true };

  // 3. 回退到全文压缩
  const compactionResult = await compactConversation(
    messages, toolUseContext, cacheSafeParams,
    true,        // suppressFollowUpQuestions
    undefined,   // customInstructions
    true,        // isAutoCompact
    recompactionInfo,
  );
  return { wasCompacted: true, compactionResult, consecutiveFailures: 0 };
}
```

### 第四级：Partial Compact

支持两个方向的精确压缩（`compact.ts:772`）：

```typescript
async function partialCompactConversation(allMessages, pivotIndex, context, ..., direction) {
  // direction='up_to': 压缩 pivotIndex 之前，保留之后的
  // direction='from':  压缩 pivotIndex 之后，保留之前的
  const messagesToSummarize = direction === 'up_to'
    ? allMessages.slice(0, pivotIndex)
    : allMessages.slice(pivotIndex);
  const messagesToKeep = direction === 'up_to'
    ? allMessages.slice(pivotIndex).filter(...)  // 过滤 progress 和旧 compact boundary
    : allMessages.slice(0, pivotIndex).filter(...);
}
```

### Prompt Cache 共享机制

压缩调用通过 `runForkedAgent` 复用主对话的 prompt cache：

```typescript
// compact.ts:1188 - streamCompactSummary
const result = await runForkedAgent({
  promptMessages: [summaryRequest],
  cacheSafeParams,            // 包含系统提示、工具定义等 cache 关键参数
  canUseTool: createCompactCanUseTool(),  // 禁止所有工具调用
  querySource: 'compact',
  maxTurns: 1,
  skipCacheWrite: true,       // 只读缓存，不写新缓存
  overrides: { abortController: context.abortController },  // 共享取消信号
});
```

### 压缩后恢复

压缩后重新注入关键附件（`compact.ts:532`）：

```typescript
const [fileAttachments, asyncAgentAttachments] = await Promise.all([
  createPostCompactFileAttachments(preCompactReadFileState, context, 5),  // 最近 5 个文件
  createAsyncAgentAttachmentsIfNeeded(context),  // 后台 Agent 状态
]);

// 单独注入：plan、plan_mode、skill、deferred tools delta、agent listing delta、MCP instructions delta
const planAttachment = createPlanAttachmentIfNeeded(context.agentId);
const planModeAttachment = await createPlanModeAttachmentIfNeeded(context);
const skillAttachment = createSkillAttachmentIfNeeded(context.agentId);
```

### Token 预算汇总

| 常量 | 值 | 用途 |
|------|-----|------|
| `AUTOCOMPACT_BUFFER_TOKENS` | 13,000 | auto-compact 触发缓冲 |
| `POST_COMPACT_TOKEN_BUDGET` | 50,000 | 压缩后文件恢复总预算 |
| `POST_COMPACT_MAX_TOKENS_PER_FILE` | 5,000 | 单文件恢复上限 |
| `POST_COMPACT_SKILLS_TOKEN_BUDGET` | 25,000 | 压缩后 skill 恢复总预算 |
| `POST_COMPACT_MAX_TOKENS_PER_SKILL` | 5,000 | 单 skill 恢复上限 |
| `POST_COMPACT_MAX_FILES_TO_RESTORE` | 5 | 恢复文件数上限 |
| `MAX_OUTPUT_TOKENS_FOR_SUMMARY` | 20,000 | 压缩摘要输出 token 上限 |
| `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES` | 3 | 断路器阈值 |
| `MAX_PTL_RETRIES` | 3 | prompt-too-long 重试上限 |

### 对 laew 的借鉴价值

1. **四级递进压缩架构值得完整借鉴**：从最轻量的工具结果裁剪到全文摘要，逐级递进
2. **缓存编辑型微压缩是关键创新**：利用 API 特性远程删除缓存条目而不破坏前缀
3. **时间触发型清理很实用**：当用户长时间不操作，主动清理旧结果减少重传
4. **压缩后恢复机制需要重点参考**：文件、plan、skill 等关键状态必须在压缩后重新注入
5. **断路器模式**（连续失败 ≥ 3 次停止）防止不可恢复的上下文反复触发压缩
6. **9 段式压缩提示词**确保摘要质量，特别是 "All User Messages" 和 "Optional Next Step" 段落

### laew 具体实现建议

**时间触发型微压缩**（P0，实现简单）：
```rust
// laew 实现思路
fn maybe_time_based_microcompact(messages: &mut Vec<Message>, gap_threshold_minutes: u64) {
    let last_assistant = messages.iter().rfind(|m| m.role == Role::Assistant);
    let gap = last_assistant.map(|m| m.timestamp.elapsed().as_minutes());
    if gap < Some(gap_threshold_minutes) { return; }

    let compactable_ids = collect_compactable_tool_ids(messages);
    let keep_set: HashSet<_> = compactable_ids.iter().rev().take(keep_recent).collect();
    for message in messages.iter_mut() {
        for block in message.content.iter_mut() {
            if let ContentBlock::ToolResult(tr) = block {
                if !keep_set.contains(&tr.tool_use_id) {
                    tr.content = "[Old tool result content cleared]".into();
                }
            }
        }
    }
}
```

**Hook 事件扩展**（P0）：
```rust
// laew 需要新增的 Hook 事件
enum HookEvent {
    PreToolUse { tool_name: String },
    PostToolUse { tool_name: String },
    PreCompact { trigger: CompactTrigger },
    PostCompact { trigger: CompactTrigger, summary: String },
    UserPromptSubmit,
    Stop,
    // ... 现有事件
}
```

---

## 专题 2：Hooks 系统（27 种 Hook）

### 核心文件清单

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/utils/hooks.ts` | 2700+ | Hook 执行引擎：匹配、执行、结果聚合 |
| `src/types/hooks.ts` | — | Hook 事件类型定义 |
| `src/hooks/useCanUseTool.tsx` | — | 权限 Hook 入口 |
| `src/hooks/toolPermission/PermissionContext.ts` | — | 权限上下文 |
| `src/utils/hooks/execPromptHook.ts` | — | Prompt 型 Hook |
| `src/utils/hooks/execAgentHook.ts` | — | Agent 型 Hook |
| `src/utils/hooks/execHttpHook.ts` | — | HTTP 型 Hook |
| `src/utils/hooks/sessionHooks.ts` | — | Session 级 Hook 注册 |
| `src/utils/hooks/AsyncHookRegistry.ts` | — | 异步 Hook 注册 |

### Hook 事件完整列表

**初始化阶段**：`Setup`、`SessionStart`、`InstructionsLoaded`、`ConfigChange`、`CwdChanged`、`FileChanged`

**用户交互**：`UserPromptSubmit`、`Stop`、`StopFailure`

**工具调用**：`PreToolUse`、`PostToolUse`、`PostToolUseFailure`、`PermissionRequest`、`PermissionDenied`

**压缩**：`PreCompact`、`PostCompact`

**Agent/Task**：`SubagentStart`、`SubagentStop`、`TeammateIdle`、`TaskCreated`、`TaskCompleted`

**MCP/通知/结束**：`Notification`、`Elicitation`、`ElicitationResult`、`SessionEnd`

### Hook 类型（5 种执行器）

```typescript
// 类型 1: command — Shell 命令（bash 或 PowerShell）
async function execCommandHook(hook, hookEvent, hookName, jsonInput, signal, ...) {
  const shellType = hook.shell ?? DEFAULT_HOOK_SHELL;
  const envVars = { ...subprocessEnv(), CLAUDE_PROJECT_DIR: toHookPath(projectDir) };
  child.stdin.write(jsonInput + '\n', 'utf8');
}

// 类型 2: prompt — 调用 LLM 评估条件（PreToolUse/PostToolUse/PermissionRequest）

// 类型 3: agent — 启动子 Agent 执行复杂逻辑（PreToolUse/PostToolUse/PermissionRequest）

// 类型 4: http — 发送 HTTP POST 请求，body 为 JSON hook 输入

// 类型 5: callback — SDK 注册的内存回调函数（快速路径，无进程创建）
```

### Hook 匹配与去重

```typescript
// hooks.ts:1346 - matchesPattern
function matchesPattern(matchQuery, matcher) {
  if (!matcher || matcher === '*') return true;           // 通配
  if (/^[a-zA-Z0-9_|]+$/.test(matcher)) {
    if (matcher.includes('|')) {                          // 管道分隔：Write|Edit
      return matcher.split('|').map(p => normalizeLegacyToolName(p.trim()))
        .includes(matchQuery);
    }
    return matchQuery === normalizeLegacyToolName(matcher);  // 精确匹配
  }
  return new RegExp(matcher).test(matchQuery);            // 正则匹配
}
```

### Hook `if` 条件匹配

```typescript
// hooks.ts:1390 - prepareIfConditionMatcher
async function prepareIfConditionMatcher(hookInput, tools) {
  const tool = findToolByName(tools, hookInput.tool_name);
  const input = tool?.inputSchema.safeParse(hookInput.tool_input);
  const patternMatcher = input?.success && tool?.preparePermissionMatcher
    ? await tool.preparePermissionMatcher(input.data)
    : undefined;

  return ifCondition => {
    const parsed = permissionRuleValueFromString(ifCondition);  // "Bash(git *)" → {toolName: "Bash", ruleContent: "git *"}
    if (normalizeLegacyToolName(parsed.toolName) !== toolName) return false;
    return patternMatcher ? patternMatcher(parsed.ruleContent) : false;
  };
}
```

### Hook JSON 输出协议

```typescript
// PreToolUse 专用：
{ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "allow|deny|ask", "updatedInput": {} } }

// PostToolUse 专用：
{ "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": "string", "updatedMCPToolOutput": {} } }

// 通用字段：
{ "continue": false, "stopReason": "string", "systemMessage": "string", "suppressOutput": true }
```

### 异步 Hook 协议

```typescript
// hooks.ts:1117 - 首行异步检测
const firstLine = firstLineOf(stdout).trim();
const parsed = jsonParse(firstLine);
if (isAsyncHookJSONOutput(parsed)) {
  // {"async": true, "asyncTimeout": 300000}
  executeInBackground({ processId, hookId, shellCommand, asyncResponse: parsed, ... });
}
```

### 对 laew 的借鉴价值

1. **27 种 Hook 事件覆盖完整生命周期**，laew 需要扩展到 PreToolUse/PostToolUse/PreCompact/PostCompact
2. **5 种 Hook 执行器**设计模式值得参考，特别是 `agent` 类型可以启动子 Agent
3. **`if` 条件匹配**避免对每个工具调用都触发所有 Hook
4. **异步 Hook 协议**支持后台执行不阻塞主循环

### laew 具体实现建议

**Hook 事件总线设计**（P0）：
```rust
// laew 的 Hook 系统应该支持以下执行器类型
enum HookExecutor {
    Command { command: String, shell: ShellType, timeout_ms: u64 },
    Callback(Box<dyn Fn(&HookInput) -> HookResult>),  // 内存回调
    // 未来扩展：Agent、Http、Prompt
}

// Hook 匹配器支持管道分隔和正则
fn matches_pattern(match_query: &str, matcher: &str) -> bool {
    if matcher == "*" { return true; }
    if matcher.contains('|') {
        return matcher.split('|').any(|p| p.trim() == match_query);
    }
    if let Ok(re) = Regex::new(matcher) {
        return re.is_match(match_query);
    }
    match_query == matcher
}
```

**Hook `if` 条件支持**（P1）：
```rust
// 支持 "Bash(git *)" 格式的细粒度条件匹配
struct HookIfCondition {
    tool_name: String,
    rule_content: Option<String>,  // "git *" 等模式
}
```

---

## 专题 3：Skill 系统（16+ 内置 Skill）

### 核心文件清单

| 文件 | 行数 | 职责 |
|------|------|------|
| `src/skills/loadSkillsDir.ts` | 1087 | Skill 发现、加载、解析 |
| `src/skills/bundledSkills.ts` | 221 | 内置 Skill 注册框架 |
| `src/skills/bundled/index.ts` | 80 | 内置 Skill 初始化入口 |
| `src/skills/mcpSkillBuilders.ts` | 45 | MCP Skill 构建器桥接 |
| `src/tools/SkillTool/SkillTool.ts` | 1109 | Skill 工具 |

### Skill 文件格式

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
支持 !`shell_command` 内联执行（非 MCP skill）
```

### Skill 发现与加载机制

```typescript
// loadSkillsDir.ts:638 - getSkillDirCommands
export const getSkillDirCommands = memoize(async (cwd) => {
  const userSkillsDir = join(getClaudeConfigHomeDir(), 'skills');     // ~/.claude/skills
  const managedSkillsDir = join(getManagedFilePath(), '.claude', 'skills'); // 策略管理
  const projectSkillsDirs = getProjectDirsUpToHome('skills', cwd);   // 项目级

  // 并行加载所有来源，然后基于 realpath 去重
  const [managedSkills, userSkills, projectSkills, additionalSkills, legacyCommands] =
    await Promise.all([...]);
});
```

### 条件激活 Skill

```typescript
// loadSkillsDir.ts:997
export function activateConditionalSkillsForPaths(filePaths, cwd) {
  for (const [name, skill] of conditionalSkills) {
    const skillIgnore = ignore().add(skill.paths);  // gitignore 风格匹配
    for (const filePath of filePaths) {
      if (skillIgnore.ignores(relativePath)) {
        dynamicSkills.set(name, skill);   // 激活
        conditionalSkills.delete(name);
        activatedConditionalSkillNames.add(name);
      }
    }
  }
}
```

### Skill 调用流程

```typescript
// SkillTool.ts:580 - call 方法
async call({ skill, args }, context, canUseTool, parentMessage, onProgress) {
  const command = findCommand(commandName, commands);
  recordSkillUsage(commandName);

  // 分支 1：fork 模式 → 启动子 Agent
  if (command.context === 'fork') {
    return executeForkedSkill(command, commandName, args, context, canUseTool, ...);
  }

  // 分支 2：inline 模式 → 注入消息到当前对话
  const processedCommand = await processPromptSlashCommand(commandName, args, commands, context);
  return {
    data: { success: true, commandName, allowedTools, model },
    newMessages,
    contextModifier(ctx) { /* 更新工具权限、模型覆盖 */ },
  };
}
```

### 内置 Skill 完整列表

| Skill | 文件 | 用途 | 模型可调用 | 用户可调用 |
|-------|------|------|-----------|-----------|
| `update-config` | `updateConfig.ts` | 配置 settings.json | ✅ | ✅ |
| `keybindings-help` | `keybindings.ts` | 自定义键盘快捷键 | ❌ | ❌ |
| `verify` | `verify.ts` | 验证代码变更 | ✅ | ✅ |
| `debug` | `debug.ts` | 调试会话问题 | ❌ | ✅ |
| `skillify` | `skillify.ts` | 捕获会话为 Skill | ❌ | ✅ |
| `remember` | `remember.ts` | 审查自动记忆 | ✅ | ✅ |
| `simplify` | `simplify.ts` | 代码审查（三 Agent） | ✅ | ✅ |
| `batch` | `batch.ts` | 并行批量变更 | ❌ | ✅ |
| `stuck` | `stuck.ts` | 诊断卡住的会话 | ✅ | ✅ |
| `schedule` | `scheduleRemoteAgents.ts` | 调度远程 Agent | ✅ | ✅ |
| `claude-api` | `claudeApi.ts` | Claude API 指南 | ✅ | ✅ |

### 对 laew 的借鉴价值

1. **Skill = Markdown + Frontmatter** 设计优雅：声明式、人类可读、Git 友好
2. **条件激活（paths）** 减少无关 Skill 的 token 占用
3. **`when_to_use`** 是模型自动触发的关键
4. **`context: fork` vs `inline`** 的选择影响 token 预算隔离
5. **内置 Skill 用 `registerBundledSkill` API 注册**，适合编译进二进制

### laew 具体实现建议

**Skill 文件格式**（P0）：
```yaml
---
name: deploy
description: "部署当前分支到 staging 环境"
when_to_use: "Use when the user wants to deploy code to staging. Examples: 'deploy to staging', 'push to staging', 'ship it'"
allowed-tools: [Bash(git:*), Bash(npm:*), Read]
context: fork
---

# Deploy to Staging

## Steps
1. Run tests: `npm test`
2. Build: `npm run build`
3. Push: `git push origin HEAD:staging`
4. Verify: curl staging endpoint
```

**条件激活实现**（P1）：
```rust
// laew 的 Skill 条件激活
struct ConditionalSkill {
    name: String,
    paths: Vec<Pattern>,  // gitignore 风格
    command: SkillCommand,
}

fn activate_conditional_skills(file_paths: &[PathBuf], cwd: &Path) -> Vec<String> {
    let mut activated = Vec::new();
    for skill in &mut conditional_skills {
        let matcher = GitignoreMatcher::new(&skill.paths);
        for path in file_paths {
            if matcher.matches(path, cwd) {
                dynamic_skills.insert(skill.name.clone(), skill.command.clone());
                activated.push(skill.name.clone());
                break;
            }
        }
    }
    activated
}
```

---

## 专题 4：MCP（Model Context Protocol）架构

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `src/services/mcp/client.ts` | MCP 客户端核心（3500+ 行） |
| `src/services/mcp/types.ts` | 传输类型、配置类型、连接状态类型 |
| `src/services/mcp/config.ts` | MCP 配置加载（7 个 scope） |
| `src/services/mcp/auth.ts` | OAuth 认证（含 XAA） |
| `src/services/mcp/InProcessTransport.ts` | 进程内传输 |
| `src/tools/MCPTool/` | MCP 工具包装 |

### 传输方式（8 种）

```typescript
// types.ts:23
export const TransportSchema = z.enum([
  'stdio',    // 标准 I/O（本地进程，最常用）
  'sse',      // Server-Sent Events（远程 HTTP 长连接）
  'sse-ide',  // IDE 扩展专用 SSE
  'http',     // Streamable HTTP（新标准）
  'ws',       // WebSocket
  'sdk',      // SDK 进程内
]);
// 额外：claudeai-proxy、ws-ide
```

### 连接状态机（5 种状态）

```typescript
type MCPServerConnection =
  | ConnectedMCPServer    // { type: 'connected', client, capabilities, instructions, cleanup }
  | FailedMCPServer       // { type: 'failed', error }
  | NeedsAuthMCPServer    // { type: 'needs-auth' }
  | PendingMCPServer      // { type: 'pending', reconnectAttempt, maxReconnectAttempts }
  | DisabledMCPServer     // { type: 'disabled' }
```

### 配置来源（7 个 scope）

```typescript
export type ConfigScope =
  | 'local'       // .claude/settings.local.json
  | 'user'        // ~/.claude/settings.json
  | 'project'     // .claude/settings.json
  | 'dynamic'     // 动态添加
  | 'enterprise'  // 企业管理策略
  | 'claudeai'    // Claude.ai 代理
  | 'managed'     // 托管策略
```

### MCP 工具调用完整链路

```
模型输出 tool_use: mcp__server__tool
  → ToolUseContext.options.tools 查找 MCPTool
  → MCPTool.call()
    → MCP 客户端 client.callTool({ name, arguments })
    → 传输层发送请求（stdio/sse/http/ws）
    → MCP 服务器处理
    → 返回结果
  → mapToolResultToToolResultBlockParam()
  → 注入 tool_result 到对话
```

### 对 laew 的借鉴价值

1. **8 种传输方式的抽象**值得学习：laew 可先从 `stdio` 开始
2. **连接状态机**（5 种状态）设计合理
3. **7 个配置 scope** 的优先级体系可参考
4. **MCP 工具命名前缀**（`mcp__server__tool`）是好的命名约定

### laew 具体实现建议

**MCP 客户端最小实现**（P1）：
```rust
// laew 的 MCP 客户端应该支持至少 stdio 传输
enum McpTransport {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    // 未来扩展：Sse, Http, WebSocket
}

enum McpServerState {
    Pending { reconnect_attempt: u32 },
    Connected { client: McpClient, capabilities: ServerCapabilities },
    Failed { error: String },
    NeedsAuth,
    Disabled,
}

// MCP 工具命名约定
fn mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp__{}__{}", server_name, tool_name)
}
```

**MCP 配置加载**（P1）：
```rust
// 支持多 scope 配置合并
struct McpConfig {
    servers: HashMap<String, McpServerConfig>,
    scope: ConfigScope,  // local | user | project | dynamic | enterprise | managed
}

// 加载顺序：managed → user → project → local（后覆盖前）
fn load_mcp_configs() -> McpConfig {
    let mut merged = HashMap::new();
    for scope in [Managed, User, Project, Local] {
        if let Some(config) = load_config_for_scope(scope) {
            merged.extend(config.servers);
        }
    }
    McpConfig { servers: merged, scope: Local }
}
```

---

## 专题 5：Tool Runner / Agent Loop

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `src/Tool.ts` | Tool 接口定义（793 行）+ `buildTool` 工厂 |
| `src/query.ts` | 主查询循环 |
| `src/QueryEngine.ts` | 查询引擎抽象 |

### Tool 接口核心

```typescript
// Tool.ts:362
export type Tool<Input, Output, P> = {
  name: string;
  shouldDefer?: boolean;            // 延迟加载（需 ToolSearch）
  alwaysLoad?: boolean;             // 永不延迟
  maxResultSizeChars: number;       // 超过后持久化到磁盘

  // 三阶段执行
  validateInput?(input, context): Promise<ValidationResult>;
  checkPermissions(input, context): Promise<PermissionResult>;
  call(args, context, canUseTool, parentMessage, onProgress): Promise<ToolResult<Output>>;

  // 行为标记
  isConcurrencySafe(input): boolean;     // 默认 false
  isReadOnly(input): boolean;            // 默认 false
  isDestructive?(input): boolean;
  interruptBehavior?(): 'cancel' | 'block';

  // 安全
  toAutoClassifierInput(input): unknown;
  preparePermissionMatcher?(input): Promise<(pattern: string) => boolean>;
}
```

### `buildTool` 工厂

```typescript
// Tool.ts:757
const TOOL_DEFAULTS = {
  isEnabled: () => true,
  isConcurrencySafe: () => false,        // 保守默认
  isReadOnly: () => false,
  isDestructive: () => false,
  checkPermissions: (input) => Promise.resolve({ behavior: 'allow', updatedInput: input }),
  toAutoClassifierInput: () => '',
};

export function buildTool<D extends AnyToolDef>(def: D): BuiltTool<D> {
  return { ...TOOL_DEFAULTS, userFacingName: () => def.name, ...def };
}
```

### ToolResult 返回结构

```typescript
export type ToolResult<T> = {
  data: T;
  newMessages?: (UserMessage | AssistantMessage)[];  // 注入新消息（SkillTool 用）
  contextModifier?: (context: ToolUseContext) => ToolUseContext;
  mcpMeta?: { _meta?, structuredContent? };
}
```

### 对 laew 的借鉴价值

1. **`validateInput` → `checkPermissions` → `call`** 三阶段流程清晰安全
2. **`isConcurrencySafe`** 允许多工具并行执行
3. **`shouldDefer` + ToolSearch** 延迟加载减少初始 prompt 大小
4. **`maxResultSizeChars` + 磁盘持久化** 结果截断策略
5. **`interruptBehavior`** 区分 cancel/block 中断模式

### laew 具体实现建议

**Tool 三阶段执行**（P0）：
```rust
// laew 的 Tool trait 应该包含三阶段
#[async_trait]
trait Tool {
    fn name(&self) -> &str;
    fn is_concurrency_safe(&self, input: &Value) -> bool { false }
    fn is_read_only(&self, input: &Value) -> bool { false }
    fn max_result_size_chars(&self) -> usize { 100_000 }

    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> Result<ValidationResult>;
    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> Result<PermissionResult>;
    async fn call(&self, input: &Value, ctx: &ToolUseContext, can_use: &CanUseToolFn) -> Result<ToolResult>;
}
```

**并行工具执行**（P1）：
```rust
// 标记为 concurrency_safe 的工具可以并行执行
async fn execute_tool_calls(tool_calls: Vec<ToolCall>, ctx: &ToolUseContext) -> Vec<ToolResult> {
    let (safe, unsafe_calls): (Vec<_>, Vec<_>) = tool_calls.into_iter()
        .partition(|tc| find_tool(&tc.name).is_concurrency_safe(&tc.input));

    let safe_results = join_all(safe.iter().map(|tc| execute_tool(tc, ctx))).await;
    let unsafe_results = execute_sequentially(unsafe_calls, ctx).await;

    merge_results(safe_results, unsafe_results)
}
```

**结果截断**（P1）：
```rust
// 大结果持久化到磁盘
fn truncate_tool_result(result: &str, max_chars: usize, tool_name: &str) -> ToolResultOutput {
    if result.len() <= max_chars {
        return ToolResultOutput::Inline(result.to_string());
    }

    let file_path = save_to_temp_file(result);
    let preview = &result[..max_chars.min(1000)];  // 预览前 1000 字符
    ToolResultOutput::Truncated {
        preview: format!("{}...\n\n[Full output saved to {}]", preview, file_path),
        file_path,
    }
}
```

---

## 专题 6：多 Agent / SubAgent 机制

### 核心文件清单

| 文件 | 职责 |
|------|------|
| `src/tools/AgentTool/` | Agent 工具定义 |
| `src/tools/AgentTool/runAgent.ts` | Agent 执行器 |
| `src/tools/SendMessageTool/` | Agent 间消息传递 |
| `src/tools/TaskCreateTool/` | 后台任务创建 |
| `src/tools/TaskGetTool/` | 任务结果获取 |
| `src/tools/TeamCreateTool/` | 团队创建 |
| `src/coordinator/coordinatorMode.ts` | 协调器模式（19K） |
| `src/tasks/LocalAgentTask/` | 本地 Agent 任务 |
| `src/tasks/InProcessTeammateTask/` | 进程内队友任务 |
| `src/tasks/RemoteAgentTask/` | 远程 Agent 任务 |
| `src/utils/forkedAgent.ts` | Fork 子 Agent |
| `src/utils/swarm/` | Swarm 并发框架 |

### SubAgent 体系四层架构

```
层次 1: Fork 子 Agent    — 轻量级，共享 prompt cache，用于 compact/skill
层次 2: AgentTool        — 模型通过 Agent 工具启动子 Agent（同步/异步）
层次 3: Task 系统        — 后台任务（异步执行 + 结果检索）
层次 4: Team/Swarm       — 多 Agent 团队协作
```

### Fork 子 Agent（最轻量）

```typescript
// forkedAgent.ts - runForkedAgent
export async function runForkedAgent({
  promptMessages,      // 要处理的消息
  cacheSafeParams,     // 系统提示、工具等 cache 关键参数
  canUseTool,          // 权限函数
  maxTurns,            // 最大轮次（compact=1）
  skipCacheWrite,      // 只读缓存
}) {
  // 共享父 Agent 的 cache-safe 参数，复用 prompt cache
  // 独立的消息上下文（只有 promptMessages）
}
```

### 上下文传递方式

| 模式 | 上下文隔离 | Prompt Cache | 用途 |
|------|-----------|-------------|------|
| Fork 子 Agent | 隔离消息，共享 cache-safe 参数 | ✅ 共享 | compact、skill fork |
| AgentTool（同步） | 完全隔离 | ❌ 独立 | 模型启动的子任务 |
| AgentTool（异步） | 完全隔离 | ❌ 独立 | 后台任务 |
| InProcessTeammate | 共享进程 | ❌ 独立 | 进程内队友 |

### Task 系统

```
TaskCreateTool  → 创建后台任务（LocalAgentTask / RemoteAgentTask / DreamTask）
TaskListTool    → 列出所有任务及状态
TaskGetTool     → 获取任务结果
TaskStopTool    → 停止任务
```

### 对 laew 的借鉴价值

1. **四层 SubAgent 体系**分层设计值得参考
2. **Fork 子 Agent 共享 prompt cache** 是关键优化
3. **Task 系统**的后台异步执行 + 结果检索模式很实用
4. **SendMessageTool** 实现 Agent 间通信
5. **协调器模式**是高级编排层

### laew 具体实现建议

**Fork 子 Agent**（P0）：
```rust
// laew 的 Fork 子 Agent 实现
async fn run_forked_agent(
    prompt_messages: Vec<Message>,
    cache_safe_params: CacheSafeParams,  // 共享的系统提示、工具定义
    can_use_tool: CanUseToolFn,
    max_turns: usize,
) -> Result<Vec<Message>> {
    // 1. 构建子 Agent 的 ToolUseContext（隔离消息）
    let mut ctx = create_subagent_context(cache_safe_params);

    // 2. 共享 prompt cache（通过相同的系统提示和工具定义）
    //    laew 的 API 层需要支持 cache-safe 参数匹配

    // 3. 执行查询循环（与主循环相同，但独立消息）
    let result = run_query_loop(&mut ctx, prompt_messages, max_turns).await?;

    Ok(result)
}
```

**Task 系统**（P2）：
```rust
// laew 的后台任务系统
struct TaskManager {
    tasks: HashMap<TaskId, TaskState>,
}

enum TaskState {
    Pending,
    Running { agent_id: AgentId },
    Completed { result: TaskResult },
    Failed { error: String },
    Cancelled,
}

impl TaskManager {
    fn create_task(&mut self, task_type: TaskType) -> TaskId { /* ... */ }
    fn get_task(&self, id: &TaskId) -> Option<&TaskState> { /* ... */ }
    fn list_tasks(&self) -> Vec<(TaskId, &TaskState)> { /* ... */ }
    async fn stop_task(&mut self, id: &TaskId) -> Result<()> { /* ... */ }
}
```

**Agent 间消息传递**（P2）：
```rust
// laew 的 SendMessage 工具
struct SendMessageTool;

impl Tool for SendMessageTool {
    fn name(&self) -> &str { "SendMessage" }

    async fn call(&self, input: &Value, ctx: &ToolUseContext) -> Result<ToolResult> {
        let target = input["to"].as_str().unwrap();
        let message = input["message"].as_str().unwrap();

        // 查找目标 Agent
        let target_agent = find_agent(target)?;

        // 发送消息（异步，不阻塞）
        target_agent.send_message(message).await?;

        Ok(ToolResult::new(json!({ "status": "sent" })))
    }
}
```

---

## 附录 A：Context 分析与 Token 追踪

### Token 统计维度

```typescript
// contextAnalysis.ts:27 - analyzeContext
type TokenStats = {
  toolRequests: Map<string, number>;      // 按工具名统计 tool_use token
  toolResults: Map<string, number>;       // 按工具名统计 tool_result token
  humanMessages: number;
  assistantMessages: number;
  localCommandOutputs: number;
  attachments: Map<string, number>;
  duplicateFileReads: Map<string, { count: number; tokens: number }>;
  other: number;
  total: number;
}
```

### 重复文件读取检测

```typescript
// contextAnalysis.ts:84
fileReadStats.forEach((data, path) => {
  if (data.count > 1) {
    const averageTokensPerRead = Math.floor(data.totalTokens / data.count);
    const duplicateTokens = averageTokensPerRead * (data.count - 1);
    stats.duplicateFileReads.set(path, { count: data.count, tokens: duplicateTokens });
  }
});
```

---

## 附录 B：PTL（Prompt-Too-Long）重试机制

当压缩调用本身也超出上下文窗口时，通过丢弃最旧的消息组来解决：

```typescript
// compact.ts:243 - truncateHeadForPTLRetry
export function truncateHeadForPTLRetry(messages, ptlResponse) {
  const groups = groupMessagesByApiRound(input);
  const tokenGap = getPromptTooLongTokenGap(ptlResponse);
  let dropCount = /* 精确丢弃覆盖差额，或回退丢弃 20% */;
  dropCount = Math.min(dropCount, groups.length - 1);  // 至少保留 1 组
  const sliced = groups.slice(dropCount).flat();
  // 确保第一条是 user 消息（API 要求）
}
```

重试循环：最多 `MAX_PTL_RETRIES = 3` 次。

---

## 附录 C：Hook 输出协议详细参考

### PreToolUse Hook 输出

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow | deny | ask",
    "permissionDecisionReason": "原因",
    "updatedInput": { "file_path": "/modified/path" },
    "additionalContext": "注入上下文"
  }
}
```

### SessionStart Hook 输出

```json
{
  "hookSpecificOutput": {
    "hookEventName": "SessionStart",
    "additionalContext": "注入系统提示",
    "initialUserMessage": "可选初始消息",
    "watchPaths": ["src/**"]
  }
}
```

---

## 附录 D：压缩提示词深度分析

### 压缩提示词的三阶段设计

**阶段 1：禁止工具调用的前置声明**

```typescript
// prompt.ts:19 - NO_TOOLS_PREAMBLE
const NO_TOOLS_PREAMBLE = `CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.
- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.`;
```

这一设计防止压缩调用浪费在工具调用上（Sonnet 4.6+ 自适应思考模型有时会尝试工具调用）。

**阶段 2：9 段式摘要结构**

```typescript
// prompt.ts:61 - BASE_COMPACT_PROMPT（核心要求）
// 1. Primary Request and Intent — 用户的明确请求和意图
// 2. Key Technical Concepts — 技术概念、框架
// 3. Files and Code Sections — 文件名 + 完整代码片段 + 重要性说明
// 4. Errors and Fixes — 错误及修复方法 + 用户反馈
// 5. Problem Solving — 已解决和进行中的问题
// 6. All User Messages — 所有非工具结果的用户消息（关键！）
// 7. Pending Tasks — 待处理任务
// 8. Current Work — 当前正在做的工作（精确到文件名和代码片段）
// 9. Optional Next Step — 下一步（引用最近对话的原文）
```

**阶段 3：禁止工具调用的后置提醒**

```typescript
// prompt.ts:269 - NO_TOOLS_TRAILER
const NO_TOOLS_TRAILER = '\n\nREMINDER: Do NOT call any tools. Respond with plain text only — ' +
  'an <analysis> block followed by a <summary> block. ' +
  'Tool calls will be rejected and you will fail the task.';
```

### 压缩后的摘要格式化

```typescript
// prompt.ts:311 - formatCompactSummary
export function formatCompactSummary(summary) {
  // 1. 剥离 <analysis> 草稿区（提升摘要质量的思维过程，不需要保留）
  formattedSummary = formattedSummary.replace(/<analysis>[\s\S]*?<\/analysis>/, '');
  // 2. 提取 <summary> 内容并替换为可读标题
  const summaryMatch = formattedSummary.match(/<summary>([\s\S]*?)<\/summary>/);
  // 3. 清理多余空行
  formattedSummary = formattedSummary.replace(/\n\n+/g, '\n\n');
  return formattedSummary.trim();
}
```

### 压缩后消息包装

```typescript
// prompt.ts:337 - getCompactUserSummaryMessage
export function getCompactUserSummaryMessage(summary, suppressFollowUpQuestions, transcriptPath) {
  const formattedSummary = formatCompactSummary(summary);
  let baseSummary = `This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n${formattedSummary}`;

  if (transcriptPath) {
    baseSummary += `\n\nIf you need specific details from before compaction, read the full transcript at: ${transcriptPath}`;
  }

  if (suppressFollowUpQuestions) {
    return `${baseSummary}\nContinue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, do not preface with "I'll continue" or similar. Pick up the last task as if the break never happened.`;
  }

  return baseSummary;
}
```

### Partial Compact 的两种方向提示词差异

**direction='from'（压缩后续消息，保留前缀）**：

```typescript
// PARTIAL_COMPACT_PROMPT - 总结"最近消息"
"Your task is to create a detailed summary of the RECENT portion of the conversation —
 the messages that follow earlier retained context."
```

**direction='up_to'（压缩前缀消息，保留后续）**：

```typescript
// PARTIAL_COMPACT_UP_TO_PROMPT - 总结"早期消息"
"Your task is to create a detailed summary of this conversation. This summary will be
 placed at the start of a continuing session; newer messages that build on this context
 will follow after your summary."
// 额外包含 "Context for Continuing Work" 段落
```

---

## 附录 E：Hook 系统高级特性

### Hook 配置快照

```typescript
// hooksConfigSnapshot.ts
// 在会话启动时捕获 hooks 配置的快照
// 防止运行时配置变更影响已启动的 hooks
// 快照来源：settings.json 的 hooks 字段
```

### Session Hooks（运行时注册）

```typescript
// sessionHooks.ts
// 支持在运行时动态注册 hook
// 来源：Agent frontmatter hooks、Skill hooks、SDK callbacks
// 生命周期：当前会话
// 隔离：每个 Agent 有独立的 session hooks
```

### Hook 的安全边界

```typescript
// hooks.ts:286 - shouldSkipHookDueToTrust
export function shouldSkipHookDueToTrust(): boolean {
  const isInteractive = !getIsNonInteractiveSession();
  if (!isInteractive) return false;  // SDK 模式隐式信任
  const hasTrust = checkHasTrustDialogAccepted();
  return !hasTrust;  // 交互模式必须接受信任对话框
}
```

所有 Hook 执行都需要工作区信任，这是防止 RCE 的纵深防御。

### Hook 执行超时

```typescript
const TOOL_HOOK_EXECUTION_TIMEOUT_MS = 10 * 60 * 1000;  // 10 分钟（工具 hook）
const SESSION_END_HOOK_TIMEOUT_MS_DEFAULT = 1500;        // 1.5 秒（会话结束 hook）
// 可通过 hook.timeout 字段自定义（单位：秒）
// 可通过 CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS 环境变量覆盖 session end 超时
```

### Hook 去重机制

```typescript
// hooks.ts:1453 - hookDedupKey
function hookDedupKey(m: MatchedHook, payload: string): string {
  return `${m.pluginRoot ?? m.skillRoot ?? ''}\0${payload}`;
}
// command hooks: shell\0command\0ifCondition
// prompt hooks: prompt\0ifCondition
// agent hooks: prompt\0ifCondition
// http hooks: url\0ifCondition
// callback/function hooks: 不去重（每个唯一）
```

---

## 附录 F：Skill 系统高级特性

### Skill 文件变更检测

```typescript
// skillChangeDetector.ts
// 监控 .claude/skills/ 目录变更
// 当 SKILL.md 文件被修改时，清除缓存并重新加载
// 支持热更新：无需重启会话即可生效
```

### Skill 使用频率追踪

```typescript
// SkillTool.ts:619
recordSkillUsage(commandName);  // 记录使用频率
// 用于：1. 排序建议 2. 分析 3. 自动推荐
```

### MCP Skill 构建器桥接

```typescript
// mcpSkillBuilders.ts - 解决循环依赖的关键设计
// loadSkillsDir.ts → mcpSkillBuilders.ts ← mcpSkills.ts
// 写入一次注册模式：loadSkillsDir.ts 模块初始化时注册
// 运行时通过 getMCPSkillBuilders() 获取
```

### Skill 的 `files` 字段（内置 Skill 专用）

```typescript
// bundledSkills.ts:34
files?: Record<string, string>;  // { 'relative/path': 'content' }
// 首次调用时提取到磁盘（getBundledSkillExtractDir）
// 模型可以通过 Read/Grep 访问这些文件
// 安全：O_NOFOLLOW|O_EXCL 防止符号链接攻击
```

### Skill 权限检查流程

```typescript
// SkillTool.ts:432 - checkPermissions
// 1. 检查 deny 规则（支持精确匹配和前缀匹配 "review:*"）
// 2. 检查 allow 规则
// 3. 安全属性检查：skillHasOnlySafeProperties — 仅含安全属性的 skill 自动放行
// 4. 默认行为：ask（弹出确认框）

const SAFE_SKILL_PROPERTIES = new Set([
  'type', 'progressMessage', 'contentLength', 'argNames', 'model', 'effort',
  'source', 'pluginInfo', 'disableNonInteractive', 'skillRoot', 'context', 'agent',
  'getPromptForCommand', 'frontmatterKeys', 'name', 'description',
  'hasUserSpecifiedDescription', 'isEnabled', 'isHidden', 'aliases', 'isMcp',
  'argumentHint', 'whenToUse', 'paths', 'version', 'disableModelInvocation',
  'userInvocable', 'loadedFrom', 'immediate', 'userFacingName',
]);
// 不含 allowedTools/hooks → 有这些字段的 skill 需要用户确认
```

---

## 附录 G：MCP 客户端实现细节

### OAuth 认证流程

```typescript
// auth.ts - 完整 OAuth 流程
// 1. 发现 OAuth metadata（.well-known/oauth-authorization-server）
// 2. 动态注册客户端（client_registration）
// 3. 授权码流程（authorization_code）
// 4. Token 刷新（refresh_token）
// 5. XAA 跨应用访问（SEP-990）
```

### MCP 工具命名与注册

```typescript
// MCP 工具通过 Tool 接口包装
tool.mcpInfo = { serverName, toolName };
tool.isMcp = true;
tool.name = `mcp__${serverName}__${toolName}`;  // 命名前缀

// MCP prompts 作为 loadedFrom: 'mcp' 的 Skill 注册
// SkillTool.ts:81
const mcpSkills = context.getAppState().mcp.commands
  .filter(cmd => cmd.type === 'prompt' && cmd.loadedFrom === 'mcp');
```

### Elicitation 处理

```typescript
// elicitationHandler.ts - MCP 服务器的 elicit 请求处理
// MCP 服务器可以通过 elicit 协议请求用户输入
// 支持 URL elicitations（跳转浏览器）和表单 elicitations
```

---

## 附录 H：协调器模式与 Swarm 框架

### 协调器模式（coordinatorMode.ts，19K）

协调器是 Claude Code 的高级多 Agent 编排层：

```typescript
// coordinatorMode.ts - 核心功能
// 1. 消息路由：将用户消息路由到合适的 Agent
// 2. 任务分配：将大任务拆解为子任务分配给多个 Agent
// 3. 结果聚合：收集各 Agent 的结果并汇总
// 4. Agent 间通信：通过 SendMessageTool 实现
```

### Swarm 框架

```
src/utils/swarm/
├── backends/     # 多种后端实现
```

Swarm 支持多 Agent 并发执行，有独立的后端抽象。支持：
- 进程内并发（InProcessTeammateTask）
- 远程并发（RemoteAgentTask）
- 本地 Shell 并发（LocalShellTask）

---

## 附录 I：工具结果生命周期管理

Claude Code 对工具结果有完整的生命周期管理：

```
1. 执行时：结果存入 messages + readFileState 缓存
2. 压缩前：检测哪些文件需要保留（createPostCompactFileAttachments()）
3. 压缩后：重新注入最近读取的文件（最多 5 个，总计 50K tokens）
4. MicroCompact：旧工具结果被替换为 '[Old tool result content cleared]'
5. 去重：已读文件的后续读取使用 FILE_UNCHANGED_STUB 占位符
```

### readFileState 缓存

```typescript
// ToolUseContext.readFileState: FileStateCache
// 缓存最近读取的文件内容和时间戳
// 用于：1. 压缩后恢复 2. 文件变更检测 3. 去重
```

### FILE_UNCHANGED_STUB

```typescript
// FileReadTool/prompt.ts
export const FILE_UNCHANGED_STUB = '[File unchanged since last read]';
// 当文件未变更时，返回占位符而非完整内容
// 节省 token：避免重复注入相同的文件内容
```

---

## 附录 J：laew 借鉴优先级详细路线图

### P0（立即可做，1-2 周）

| 特性 | 实现难度 | 收益 | 参考代码 |
|------|---------|------|---------|
| 时间触发型微压缩 | 低 | 高 | `microCompact.ts:446` |
| Hook 事件扩展（PreToolUse/PostToolUse） | 中 | 高 | `hooks.ts:1952` |
| Skill `when_to_use` 字段 | 低 | 高 | `loadSkillsDir.ts:185` |
| Tool 三阶段执行 | 中 | 高 | `Tool.ts:362` |

### P1（近期规划，2-4 周）

| 特性 | 实现难度 | 收益 | 参考代码 |
|------|---------|------|---------|
| Skill 条件激活（paths） | 中 | 中 | `loadSkillsDir.ts:997` |
| Skill fork 模式 | 高 | 高 | `SkillTool.ts:122` |
| Tool 并行执行 | 中 | 中 | `Tool.ts:402` |
| MCP stdio 传输 | 高 | 高 | `mcp/client.ts` |
| 断路器模式 | 低 | 中 | `autoCompact.ts:70` |
| Hook `if` 条件匹配 | 中 | 中 | `hooks.ts:1390` |

### P2（中长期，1-2 月）

| 特性 | 实现难度 | 收益 | 参考代码 |
|------|---------|------|---------|
| 缓存编辑型微压缩 | 高 | 高 | `microCompact.ts:305` |
| Task 后台任务系统 | 高 | 高 | `tasks/` |
| Team/Swarm 多 Agent | 高 | 高 | `utils/swarm/` |
| ToolSearch 延迟加载 | 中 | 中 | `tools/ToolSearchTool/` |
| Fork 子 Agent 共享缓存 | 高 | 高 | `utils/forkedAgent.ts` |
| MCP WebSocket/SSE 传输 | 高 | 中 | `mcp/client.ts` |
| 协调器模式 | 高 | 高 | `coordinator/coordinatorMode.ts` |

---

## 跨专题核心洞察

### 关键架构模式

| 模式 | 出现位置 | laew 适用度 |
|------|---------|-----------|
| 延迟加载 + 按需发现 | Skill（paths）、Tool（ToolSearch） | ⭐⭐⭐ |
| 分级递进压缩 | Context 四级压缩管线 | ⭐⭐⭐ |
| Hook 事件总线 | 27 种生命周期事件 | ⭐⭐⭐ |
| Fork 共享缓存 | Compact forked-agent、Skill fork | ⭐⭐⭐ |
| 状态机连接管理 | MCP 5 种连接状态 | ⭐⭐ |
| 异步后台任务 | Task 系统 + Hook async 协议 | ⭐⭐⭐ |
| 断路器 | Auto-compact 连续失败停止 | ⭐⭐ |
| 事件驱动条件激活 | Skill paths + Hook if 条件 | ⭐⭐⭐ |

### laew 借鉴优先级

**P0（立即可做）**：
1. 时间触发型微压缩
2. Skill 的 `when_to_use` + 条件激活
3. Hook 事件扩展（PreToolUse/PostToolUse/PreCompact/PostCompact）

**P1（近期规划）**：
1. Tool 的 `isConcurrencySafe` + 并行执行
2. Skill 的 `context: fork` 子 Agent 执行
3. MCP 客户端的 stdio 传输支持

**P2（中长期）**：
1. Task 系统（后台异步 Agent）
2. Team/Swarm 系统
3. 缓存编辑型微压缩
4. ToolSearch 延迟加载

---

## 附录 K：关键数据结构速查

### Message 类型体系

```typescript
type Message =
  | UserMessage        // 用户输入或 tool_result
  | AssistantMessage   // 模型输出（text/thinking/tool_use）
  | SystemMessage      // 系统消息（compact boundary 等）
  | AttachmentMessage  // 附件（file/Hook/Skill/Plan 等）
  | ProgressMessage    // 进度消息（Hook 执行中等）
  | TombstoneMessage   // 已删除消息的占位符

// UserMessage 结构
interface UserMessage {
  type: 'user'
  message: { role: 'user'; content: string | ContentBlockParam[] }
  uuid: UUID
  timestamp: string
  isMeta?: boolean              // 是否为元消息（不计入对话历史）
  isCompactSummary?: boolean    // 是否为压缩摘要
  isCompactBoundary?: boolean   // 是否为压缩边界
}

// AssistantMessage 结构
interface AssistantMessage {
  type: 'assistant'
  message: {
    id: string                  // API 响应 ID（并行工具调用共享）
    role: 'assistant'
    content: ContentBlock[]     // text/thinking/tool_use blocks
    model: string
    usage: Usage                // Token 使用量
  }
  uuid: UUID
  timestamp: string
  isApiErrorMessage?: boolean   // 是否为 API 错误消息
}
```

### ToolUseContext 结构

```typescript
interface ToolUseContext {
  options: {
    tools: Tools                    // 可用工具列表
    mainLoopModel: string           // 当前模型
    mcpClients: MCPServerConnection[]  // MCP 客户端
    agentDefinitions: { activeAgents: AgentDefinition[] }
    isNonInteractiveSession: boolean
    querySource?: QuerySource
    appendSystemPrompt?: string
  }
  agentId?: AgentId                 // 当前 Agent ID
  abortController: AbortController  // 取消控制器
  readFileState: Map<string, { content: string; timestamp: number }>
  getAppState(): AppState
  setStreamMode?: (mode: 'requesting' | 'responding') => void
  setResponseLength?: (fn: (prev: number) => number) => void
  addNotification?: (notification: Notification) => void
  onCompactProgress?: (progress: CompactProgress) => void
  setSDKStatus?: (status: 'compacting' | null) => void
  queryTracking?: { chainId: string; depth: number }
}
```

### AppState 结构

```typescript
interface AppState {
  toolPermissionContext: ToolPermissionContext
  tasks: Record<string, TaskState>
  mcp: {
    clients: MCPServerConnection[]
    tools: Tool[]
    commands: Command[]
    resources: Record<string, ServerResource[]>
  }
  plugins: { enabled: LoadedPlugin[] }
  effortValue?: EffortValue
  sessionHooks: Map<string, SessionHookState>
}
```

---

## 附录 L：性能关键路径

### Hot Path 分析

1. **每轮查询**：`query()` → `microcompactMessages()` → `normalizeMessagesForAPI()` → `queryModelWithStreaming()`
2. **工具执行**：`canUseTool()` → `executePreToolUseHooks()` → `tool.call()` → `executePostToolUseHooks()`
3. **Hook 匹配**：`getHooksConfig()` → `getMatchingHooks()` → `matchesPattern()` → `prepareIfConditionMatcher()`

### 性能优化技巧

```typescript
// 1. Callback Hook 快速路径（hooks.ts:2041）
// 当所有 Hook 都是 internal callback 时，跳过 JSON 序列化和 span 创建
if (matchedHooks.every(m => m.hook.type === 'callback' || m.hook.type === 'function')) {
  // 直接执行，~1.8µs per hook
  for (const [i, { hook }] of matchingHooks.entries()) {
    if (hook.type === 'callback') {
      await hook.callback(hookInput, toolUseID, signal, i, context)
    }
  }
  return
}

// 2. Hook 存在性预检（hooks.ts:1582）
function hasHookForEvent(hookEvent, appState, sessionId): boolean {
  // 轻量级检查，避免完整的 getMatchingHooks 开销
  const snap = getHooksConfigFromSnapshot()?.[hookEvent]
  if (snap && snap.length > 0) return true
  const reg = getRegisteredHooks()?.[hookEvent]
  if (reg && reg.length > 0) return true
  return false
}

// 3. JSON 序列化惰性化（hooks.ts:2128）
// hookInput 的 JSON 序列化只在需要时执行，且跨所有 Hook 共享
let jsonInputResult: { ok: true; value: string } | undefined
function getJsonInput() {
  if (jsonInputResult !== undefined) return jsonInputResult
  return (jsonInputResult = { ok: true, value: jsonStringify(hookInput) })
}
```

### 内存管理

```typescript
// 1. 压缩后释放文件状态缓存
context.readFileState.clear()
context.loadedNestedMemoryPaths?.clear()

// 2. Fork Agent 结果释放
agentMessages.length = 0  // 释放消息数组

// 3. Skill 内容注册与释放
addInvokedSkill(commandName, skillPath, content, agentId)
clearInvokedSkillsForAgent(agentId)  // Agent 完成后释放
```

---

## 附录 M：配置与环境变量速查

### 上下文管理相关

| 环境变量 | 用途 | 默认值 |
|----------|------|--------|
| `CLAUDE_CODE_AUTO_COMPACT_WINDOW` | 覆盖自动压缩窗口大小 | 无 |
| `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` | 覆盖自动压缩百分比阈值 | 无 |
| `CLAUDE_CODE_BLOCKING_LIMIT_OVERRIDE` | 覆盖阻塞限制 | 无 |
| `DISABLE_COMPACT` | 禁用所有压缩 | false |
| `DISABLE_AUTO_COMPACT` | 仅禁用自动压缩 | false |
| `CLAUDE_CODE_MAX_CONTEXT_TOKENS` | 覆盖上下文窗口大小 | 无 |

### Hook 相关

| 环境变量 | 用途 | 默认值 |
|----------|------|--------|
| `CLAUDE_CODE_SIMPLE` | 跳过所有 Hook | false |
| `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` | SessionEnd Hook 超时 | 1500ms |
| `CLAUDE_CODE_SHELL_PREFIX` | Hook 命令前缀 | 无 |

### MCP 相关

| 环境变量 | 用途 | 默认值 |
|----------|------|--------|
| `CLAUDE_CODE_DISABLE_POLICY_SKILLS` | 禁用策略 Skill | false |
| `CLAUDE_CODE_DISABLE_1M_CONTEXT` | 禁用 1M 上下文 | false |

---

## 附录 N：补充深度发现（第二轮验证）

### N.1 StreamingToolExecutor 并发控制细节

`StreamingToolExecutor`（`src/services/tools/StreamingToolExecutor.ts`）是流式工具执行的核心：

```typescript
// StreamingToolExecutor.ts:40 - 并发控制逻辑
export class StreamingToolExecutor {
  private tools: TrackedTool[] = []
  private hasErrored = false
  private siblingAbortController: AbortController  // Bash 错误级联

  // 并发判定：只有当所有正在执行的工具都是 concurrencySafe 时才允许并行
  private canExecuteTool(isConcurrencySafe: boolean): boolean {
    const executingTools = this.tools.filter(t => t.status === 'executing')
    return executingTools.length === 0 ||
      (isConcurrencySafe && executingTools.every(t => t.isConcurrencySafe))
  }

  // Bash 错误级联：Bash 工具失败时取消所有兄弟工具
  // 只有 Bash 触发级联，Read/WebFetch 等独立工具失败不影响兄弟
  if (isErrorResult && tool.block.name === BASH_TOOL_NAME) {
    this.hasErrored = true
    this.siblingAbortController.abort('sibling_error')
  }
}
```

**关键发现**：工具中断行为分两种：
- `interruptBehavior() === 'cancel'`：用户 ESC 时直接取消
- `interruptBehavior() === 'block'`：等待工具完成

### N.2 主循环完整状态机（8 种终止理由）

`query()` 是 AsyncGenerator，`State.transition` 记录上一次迭代为何继续：

```typescript
// 8 种终止理由（Terminal）：
'reason: completed'           // 正常完成（无 tool_use）
'reason: blocking_limit'      // token 超阻塞线
'reason: prompt_too_long'     // 413 错误且无法恢复
'reason: image_error'         // 图片过大
'reason: model_error'         // API 错误
'reason: aborted_streaming'   // 用户中断流式
'reason: aborted_tools'       // 用户中断工具执行
'reason: hook_stopped'        // Hook 阻止继续
'reason: max_turns'           // 达到最大轮次
'reason: stop_hook_prevented' // Stop Hook 阻止

// 6 种继续理由（Continue）：
'reason: next_turn'                    // 正常下一轮（有 tool_use）
'reason: reactive_compact_retry'       // 413 后压缩重试
'reason: collapse_drain_retry'         // Context Collapse 排空重试
'reason: max_output_tokens_escalate'   // 输出 token 升级重试（8K→64K）
'reason: max_output_tokens_recovery'   // 输出 token 恢复（最多 3 次）
'reason: stop_hook_blocking'           // Stop Hook 阻塞后重试
'reason: token_budget_continuation'    // Token 预算续跑
```

### N.3 ToolResult 预算管理（per-message budget）

`applyToolResultBudget`（`toolResultStorage.ts:924`）是查询循环中的关键预处理步骤：

```typescript
// 每条 user 消息内的所有 tool_result 总大小限制
const MAX_TOOL_RESULTS_PER_MESSAGE_CHARS = 100_000  // ~25K tokens

// 三个状态分区：
type CandidatePartition = {
  mustReapply: []   // 必须重新应用（已持久化，prompt cache 稳定性）
  frozen: []        // 冻结（已见过但未替换，不再替换）
  fresh: []         // 新消息（本轮新出现，检查预算）
}

// 工具排除：Read 工具的 maxResultSizeChars=Infinity 不参与预算
// 图片块不参与预算（单独处理）
// 已标记的内容（PERSISTED_OUTPUT_TAG）不重复处理
```

### N.4 AgentTool 输入 Schema 与隔离模式

```typescript
// AgentTool.tsx:82-100 - 完整输入 Schema
const baseInputSchema = z.object({
  description: z.string(),           // 短描述（3-5 词）
  prompt: z.string(),                // 任务提示
  subagent_type: z.string().optional(),
  model: z.enum(['sonnet', 'opus', 'haiku']).optional(),
  run_in_background: z.boolean().optional(),
})

const fullInputSchema = baseInputSchema().merge(multiAgentInputSchema).extend({
  isolation: z.enum(['worktree']).optional(),  // 隔离模式
  cwd: z.string().optional(),                  // 工作目录覆盖
})
```

**Fork 子 Agent 的关键设计**：
- Fork 子 Agent 继承父 Agent 的系统提示（cache-identical）
- `buildForkedMessages()` 克隆父 Agent 的完整助手消息（含所有 tool_use 块）
- 工具定义也继承父 Agent（cache-identical tool schema）
- 递归 Fork 防护：检测 `querySource === 'agent:builtin:fork'`

### N.5 MCP 工具注册完整链路

```
1. useManageMCPConnections.ts: loadAndConnectMcpConfigs()
2. → getMcpToolsCommandsAndResources(client.ts:2226)
3. → connectToServer(name, config)  // 创建传输层
4. → fetchToolsForClient(client)     // tools/list RPC
5. → 遍历 MCP tools → 创建 MCPTool 实例
   - name: 'mcp__<server>__<tool>'
   - call(): 转发到 MCP server（含 session 过期重试）
   - isConcurrencySafe(): 来自 tool.annotations.readOnlyHint
   - isReadOnly(): 来自 tool.annotations.readOnlyHint
6. → updateServer() → AppState.mcp.tools 更新
7. → query() 每轮刷新工具列表（refreshTools()）
```

### N.6 Hook 并行执行与结果聚合

```typescript
// hooks.ts:2143 - 所有 Hook 并行执行
const hookPromises = matchingHooks.map(async function* ({ hook }, hookIndex) {
  // 根据 hook.type 分发到不同执行器
  switch (hook.type) {
    case 'callback': yield executeHookCallback(...)  // 内存回调
    case 'function': yield executeFunctionHook(...)   // TypeScript 函数
    case 'prompt':   yield execPromptHook(...)        // LLM 评估
    case 'agent':    yield execAgentHook(...)         // 子 Agent 评估
    case 'http':     yield execHttpHook(...)          // HTTP POST
    case 'command':  yield execCommandHook(...)       // Shell 命令
  }
})

// 结果聚合：deny > ask > allow 优先级
for await (const result of all(hookPromises)) {
  switch (result.permissionBehavior) {
    case 'deny':  permissionBehavior = 'deny'; break   // 最高优先级
    case 'ask':   if (permissionBehavior !== 'deny') permissionBehavior = 'ask'; break
    case 'allow': if (!permissionBehavior) permissionBehavior = 'allow'; break
  }
}
```

### N.7 Skill 条件激活机制

```typescript
// loadSkillsDir.ts:997 - activateConditionalSkillsForPaths
export function activateConditionalSkillsForPaths(filePaths, cwd) {
  for (const [name, skill] of conditionalSkills) {
    const skillIgnore = ignore().add(skill.paths)  // gitignore 风格匹配
    for (const filePath of filePaths) {
      const relativePath = relative(cwd, filePath)
      if (skillIgnore.ignores(relativePath)) {
        dynamicSkills.set(name, skill)  // 激活！
        conditionalSkills.delete(name)
        activatedConditionalSkillNames.add(name)
        break
      }
    }
  }
}
```

**触发时机**：当 Claude 执行 Read/Write/Edit 工具时，工具结果中包含文件路径，触发条件技能检查。

### N.8 Session Hooks 的 Map 设计

```typescript
// sessionHooks.ts:62 - 使用 Map 而非 Record
export type SessionHooksState = Map<string, SessionStore>

// 为什么用 Map？
// 高并发工作流下，parallel() 同步触发 N 次 addFunctionHook
// Map.set 是 O(1)，返回 prev 避免触发 store 监听器
// Record + spread 每次 O(N) 复制，N 次调用 = O(N²)
```

---

**文档完成**。本文档基于对 claudecode 源码的直接阅读和函数追踪，涵盖了 6 个核心机制的深度分析。所有代码引用均指向实际源文件和函数名，可直接在源码中验证。

### 核心设计原则总结

1. **渐进式压缩**：不要等到上下文满了才压缩，而是在多个层次上持续优化
2. **事件驱动**：Hook 系统是核心扩展点，应该作为"一等公民"设计
3. **声明式配置**：Skill 的 Markdown + Frontmatter 格式比代码更易维护
4. **状态机管理**：MCP 连接、Agent 生命周期都应该用状态机管理
5. **安全纵深**：Hook 需要工作区信任、Tool 需要三阶段验证、Skill 需要权限检查
6. **缓存友好**：Fork 子 Agent 共享 prompt cache、压缩后恢复关键状态、工具结果去重

### 与 laew 现有架构的差异

| 维度 | Claude Code | laew | 差异 |
|------|------------|------|------|
| 压缩策略 | 四级递进 | 无 | laew 需要从零开始构建 |
| Hook 系统 | 27 种事件，5 种执行器 | 基础 Hook | 需要大幅扩展 |
| Skill 系统 | 声明式 Markdown，条件激活 | 基础 Skill | 需要补充 when_to_use 和 paths |
| MCP 支持 | 8 种传输，7 个 scope | 无 | 需要从 stdio 开始 |
| 工具并行 | isConcurrencySafe 标记 | 串行 | 需要添加并行执行 |
| SubAgent | 四层体系 | 多 Agent 架构 | 需要补充 Fork 和 Task 层 |

### 关键代码文件索引

| 文件 | 核心函数/类型 | 行数 | 用途 |
|------|-------------|------|------|
| `compact.ts` | `compactConversation()` | 387 | 主压缩入口 |
| `compact.ts` | `partialCompactConversation()` | 772 | 部分压缩 |
| `compact.ts` | `streamCompactSummary()` | 1136 | 压缩摘要生成 |
| `autoCompact.ts` | `shouldAutoCompact()` | 160 | 自动压缩判断 |
| `autoCompact.ts` | `autoCompactIfNeeded()` | 241 | 自动压缩执行 |
| `microCompact.ts` | `microcompactMessages()` | 253 | 微压缩入口 |
| `microCompact.ts` | `cachedMicrocompactPath()` | 305 | 缓存编辑型微压缩 |
| `microCompact.ts` | `evaluateTimeBasedTrigger()` | 422 | 时间触发判断 |
| `prompt.ts` | `getCompactPrompt()` | 293 | 压缩提示词生成 |
| `prompt.ts` | `formatCompactSummary()` | 311 | 摘要格式化 |
| `hooks.ts` | `executeHooks()` | 1952 | Hook 执行引擎 |
| `hooks.ts` | `getMatchingHooks()` | 1603 | Hook 匹配 |
| `hooks.ts` | `matchesPattern()` | 1346 | 模式匹配 |
| `hooks.ts` | `processHookJSONOutput()` | 489 | JSON 输出处理 |
| `loadSkillsDir.ts` | `getSkillDirCommands()` | 638 | Skill 加载 |
| `loadSkillsDir.ts` | `activateConditionalSkillsForPaths()` | 997 | 条件激活 |
| `loadSkillsDir.ts` | `parseSkillFrontmatterFields()` | 185 | Frontmatter 解析 |
| `SkillTool.ts` | `call()` | 580 | Skill 调用 |
| `SkillTool.ts` | `executeForkedSkill()` | 122 | Fork 执行 |
| `SkillTool.ts` | `checkPermissions()` | 432 | 权限检查 |
| `Tool.ts` | `buildTool()` | 783 | Tool 工厂 |
| `types.ts` (mcp) | `TransportSchema` | 23 | 传输类型 |
| `types.ts` (mcp) | `MCPServerConnection` | 221 | 连接状态 |
| `contextAnalysis.ts` | `analyzeContext()` | 27 | Token 分析 |
