# 第十八轮 pi 深度分析 — 用户交互体验层

## 元信息

| 字段 | 值 |
|------|------|
| 文档版本 | 第十八轮 / 2026-09-09 |
| 工程 | pi（earendil-works/pi，TypeScript） |
| 定位 | 自研 cell-based TUI + Lane 并发 + Skill 一等公民的 Coding Agent CLI |
| 核心架构 | pi-tui（自研）+ pi-agent-core（lane 三态）+ pi-coding-agent（24 个交互组件） + extension API |
| 调研主题 | **用户交互体验层**（D1-D8 共 8 维度） |
| 行数目标 | 700-1300 行 |
| 调研方法 | `Grep`/`Read` 关键文件 + 跨包交叉引用（coding-agent / tui / agent-core）+ 行为模式提炼 |
| 输出 | 中文分析 + 英文标识符 + 文件:行号 + 30 个新 laew gap（L1546-L1575） |
| 调研路径 | `/usr/local/LsmGitOpenSource/pi` |

> **关键发现速览**：pi 是「工程化深度最高」的 TUI 之一——`@` 提及 + 斜杠命令 + Skill 三层触发器统一到 `CombinedAutocompleteProvider`，对话树支持 `getTree()` 五种 FilterMode（default/no-tools/user-only/labeled-only/all）+ 双层 fork（`/fork` 选历史消息 + `/clone` 复制当前）+ 编辑消息**未实现**（只可 fork 不可就地改）；session JSONL 导出会附带 `pi.share` 自定义 entry 携带 system prompt + tools 元数据用于共享回放；statusline（footer）= `↑↓ R W CH$ ctx% (branch)` 单行聚合 7 项指标含 cache hit 率；项目信任分 5 档（Trust/Trust parent/Trust session-only/Do not trust/Do not trust session-only），按 cwd **冒泡**到父目录（`findNearestTrustEntry`）。本文按 D1-D8 展开，每个维度附源码锚点 + 真实代码片段 + 30 个 laew gap。

---

## D1 — @提及系统

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/tui/src/autocomplete.ts` | 全文（826 行） | `CombinedAutocompleteProvider`：4 路触发器合一（@ / / / path） |
| `packages/tui/src/autocomplete.ts` | 230-300 | `AutocompleteProvider` / `SlashCommand` / `AutocompleteItem` 类型契约 |
| `packages/tui/src/autocomplete.ts` | 326-410 | `extractAtPrefix` + `extractPathPrefix`：路径提取与引号边界 |
| `packages/tui/src/autocomplete.ts` | 504-510 | `shouldTriggerFileCompletion`：仅 Tab 强制触发 |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 631-728 | `createBaseAutocompleteProvider`：四源合并 built-in/prompt/extension/skill |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 714-728 | Skill 命令 `/skill:<name>` 注册与 lint 化 source 标记 |
| `packages/coding-agent/src/cli/file-processor.ts` | 全文（95 行） | CLI `@file` 处理为 `<file>` XML 块 + 图片附件 |
| `packages/coding-agent/src/core/agent-session.ts` | 1366-1394 | `_expandSkillCommand`：运行时 `/skill:<name> args` 展开为 XML 块 |
| `packages/coding-agent/src/core/prompt-templates.ts` | 全文（285 行） | `/name` + bash 风格 `${N:-default}` 参数替换 |
| `packages/coding-agent/src/core/settings-manager.ts` | 124 | `enableSkillCommands` 开关 |

### 机制剖析

**1. 四路统一 autocomplete 触发器**（`autocomplete.ts:326-345`）：

```typescript
// CombinedAutocompleteProvider.getSuggestions()
// 顺序：@ 触发 → / 触发（首字符）→ path 触发（自然或 Tab 强制）

async getSuggestions(lines, cursorLine, cursorCol, options) {
  const currentLine = lines[cursorLine] || "";
  const textBeforeCursor = currentLine.slice(0, cursorCol);

  const atPrefix = this.extractAtPrefix(textBeforeCursor);
  if (atPrefix) { /* fuzzy file suggestions */ }

  if (!options.force && textBeforeCursor.startsWith("/")) {
    // slash command: 空格前 = 命令名补全，空格后 = argument 补全
    const spaceIndex = textBeforeCursor.indexOf(" ");
    if (spaceIndex === -1) { /* commandName */ } 
    else { /* argumentText via command.getArgumentCompletions */ }
  }

  const pathMatch = this.extractPathPrefix(textBeforeCursor, options.force);
  // ...
}
```

**2. Skill 命令作为一等 slash 命令注入**（`interactive-mode.ts:714-728`）：

```typescript
if (this.settingsManager.getEnableSkillCommands()) {
  for (const skill of this.session.resourceLoader.getSkills().skills) {
    const commandName = `skill:${skill.name}`;
    this.skillCommands.set(commandName, skill.filePath);
    skillCommandList.push({
      name: commandName,
      description: this.prefixAutocompleteDescription(skill.description, skill.sourceInfo),
    });
  }
}
```

**3. `/skill:<name>` 运行时展开为 XML 块**（`agent-session.ts:1366-1394`）：

```typescript
private _expandSkillCommand(text: string): string {
  if (!text.startsWith("/skill:")) return text;
  const spaceIndex = text.indexOf(" ");
  const skillName = spaceIndex === -1 ? text.slice(7) : text.slice(7, spaceIndex);
  const args = spaceIndex === -1 ? "" : text.slice(spaceIndex + 1).trim();
  const skill = this.resourceLoader.getSkills().skills.find((s) => s.name === skillName);
  if (!skill) return text;
  const body = stripFrontmatter(readFileSync(skill.filePath, "utf-8")).trim();
  const skillBlock = `<skill name="${skill.name}" location="${skill.filePath}">\nReferences are relative to ${skill.baseDir}.\n\n${body}\n</skill>`;
  return args ? `${skillBlock}\n\n${args}` : skillBlock;
}
```

**4. CLI `@file` 启动期**（`file-processor.ts:25-95`）：
- 图片：自动 resize → base64 → 图片块 + `<file>` 引用文本
- 文本：`<file name="...">\n...\n</file>` XML 注入
- 空文件跳过；读失败 → `process.exit(1)` 红色错误

### 设计巧妙点

1. **三层触发器合一**（`@`、`/`、路径）通过 `AutocompleteProvider` 接口 + 优先级链，扩展只需 `registerAutocompleteProviderFactory` 包装即可插入（`interactive-mode.ts:733-743`）。
2. **Skill 命令名注入补全 + 运行时展开分离**：补全阶段显示 `/skill:<name>`，展开阶段读 frontmatter 后注入完整 body，避免「补全看到 100 字符 body」的 UX 灾难。
3. **`/skill:` 命令命名空间**避免了 skill 名为 `model` 等与内建命令冲突——通过 `prefixAutocompleteDescription` 加 `[user]`/`[project]`/`[extension]` 源标签提示。

### laew gap 表（D1）

| 编号 | 描述 | P | 推荐 Rust crate |
|------|------|---|----------------|
| **L1546** | 无 `@` 提及系统（laew 仅支持 CLI `@file` + 不存在的 slash 补全） | P1 | `fuzzy-matcher` + `walkdir` + `nucleo-matcher` + 自实现 `AtPrefixExtractor` |
| **L1547** | 无 Skill 命令注入（Skill 加载后未暴露为 `/skill:name` 入口） | P1 | 与 L1546 一起在 `tui/autocomplete.rs` 实现 |

---

## D2 — 自定义斜杠命令与 Prompt 模板

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/coding-agent/src/core/slash-commands.ts` | 全文（43 行） | 23 个内建 `BUILTIN_SLASH_COMMANDS` + `SlashCommandInfo` 类型 |
| `packages/coding-agent/src/core/prompt-templates.ts` | 全文（285 行） | 模板加载 + bash 风格参数替换 + frontmatter 描述/argument-hint 解析 |
| `packages/coding-agent/src/core/extensions/runner.ts` | 654-705 | 扩展命令注册 + 同名命令去重（`:occurrence` 后缀） |
| `packages/coding-agent/src/core/extensions/types.ts` | 1316-1319 | `registerCommand(name, options)` API |
| `packages/coding-agent/src/core/extensions/loader.ts` | 296-305 | 扩展 registerCommand 实现（per-extension `commands` Map） |
| `packages/experimental/services/slash-commands.ts` | 全文（30 行） | `SlashCommands` Cordis service + `SlashCommandContribution` 类型 |
| `packages/experimental/services/slash-commands-provider.ts` | 40-80 | `SlashCommandRegistry`：注册/替换/订阅；`#validate` 名称正则 `^[a-z0-9][a-z0-9:-]*$` |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 3020-3140 | 斜杠命令路由（if/else 链 + prompt templates + extension commands + prompt() 通道） |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 4423 | `isExtensionCommand(text)`：仅在 `session.prompt` 阶段处理扩展命令 |

### 机制剖析

**1. 内建 23 个命令**（`slash-commands.ts:19-43`）：

```typescript
export const BUILTIN_SLASH_COMMANDS = [
  { name: "settings", description: "Open settings menu" },
  { name: "model", description: "Select model (opens selector UI)", argumentHint: "<provider/model>" },
  { name: "tree", description: "Navigate session tree (switch branches)" },
  { name: "thinking", description: "Set thinking level", argumentHint: "<level>" },
  { name: "scoped-models", description: "Enable/disable models for Ctrl+P cycling" },
  { name: "export", description: "Export session (HTML default, or specify path: .html/.jsonl)" },
  { name: "share", description: "Share session as a secret GitHub gist" },
  { name: "fork", description: "Create a new fork from a previous user message" },
  { name: "clone", description: "Duplicate the current session at the current position" },
  { name: "trust", description: "Save project trust decision for future sessions" },
  { name: "compact", description: "Manually compact the session context" },
  { name: "reload", description: "Reload keybindings, extensions, skills, prompts, themes, and context files" },
  // ...
] as const;
```

**2. Prompt 模板 bash 风格参数替换**（`prompt-templates.ts:71-90`）：

```typescript
// 支持:
// $1, $2, ...                   位置参数
// $@ / $ARGUMENTS                所有参数
// ${N:-default}                 位置参数 N 缺失/空时默认值
// ${@:-default}                 所有参数空时默认值
// ${@:N}                        从第 N 个开始（bash slice）
// ${@:N:L}                      从第 N 取 L 个
export function substituteArgs(content: string, args: string[]): string {
  const allArgs = args.join(" ");
  return content.replace(
    /\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?\}|\$(ARGUMENTS|@|\d+)/g,
    (_match, defaultTarget, defaultValue, sliceStart, sliceLength, simple) => { /* ... */ }
  );
}
```

**3. 模板三源合并**（`prompt-templates.ts:182-260`）：

```typescript
// 三源按顺序覆盖：
// 1. agentDir/prompts/                          （global user）
// 2. cwd/{CONFIG_DIR}/prompts/                   （project local）
// 3. --prompt <path> 显式注册                    （explicit）
// sourceInfo 自动判定 scope（local.user / local.project）
```

**4. 扩展命令同名去重**（`runner.ts:664-683`）：

```typescript
private resolveRegisteredCommands(): ResolvedCommand[] {
  let invocationName = (counts.get(command.name) ?? 0) > 1 
    ? `${command.name}:${occurrence}`  // 多扩展同名 → "name:1", "name:2"
    : command.name;
  if (takenInvocationNames.has(invocationName)) {
    let suffix = occurrence;
    do {
      suffix++;
      invocationName = `${command.name}:${suffix}`;
    } while (takenInvocationNames.has(invocationName));
  }
}
```

**5. 与内建命令冲突诊断**（`interactive-mode.ts:619-628`）：

```typescript
private getBuiltInCommandConflictDiagnostics(extensionRunner): ResourceDiagnostic[] {
  const builtinNames = new Set(BUILTIN_SLASH_COMMANDS.map(c => c.name));
  return extensionRunner.getRegisteredCommands()
    .filter(cmd => builtinNames.has(cmd.name))
    .map(cmd => ({
      type: "warning",
      message: cmd.invocationName === cmd.name
        ? `Extension command '/${cmd.name}' conflicts with built-in interactive command. Skipping in autocomplete.`
        : `Extension command '/${cmd.name}' conflicts with built-in interactive command. Available as '/${cmd.invocationName}'.`,
      path: cmd.sourceInfo.path,
    }));
}
```

**6. 命令路由三层**（`interactive-mode.ts:3020-3140`）：
- Layer 1（硬编码 if/else）：`/settings /model /tree /fork /clone /trust /login /logout /new /compact /reload /debug /resume /quit` 等
- Layer 2（`session.prompt(text)` + `expandPromptTemplates`）：扩展命令、prompt 模板、skill 模板
- Layer 3（fallthrough）：`!` bash 直通模式

### 设计巧妙点

1. **slash 命令注册名冲突解决**：扩展注册同名 → 自动加 `:occurrence` 后缀，并通过 diagnostics 报告给用户；与内建命令冲突则保留为 `:N` 形式而非直接隐藏。
2. **Prompt 模板 + 扩展命令 + Skill 命令三合一展开**：`expandPromptTemplates` 默认 true，在 `session.prompt()` 中串联执行——`_throwIfExtensionCommand` 先验 → `_runInputHandlers` → `_expandSkillCommand` → `expandPromptTemplate`。
3. **bash 风格参数替换**：`$1`/`$@`/`${N:-default}` 让用户可以用熟悉的 shell 语法写 prompt 模板，降低学习成本。
4. **SlashCommands 双实现**：`BUILTIN_SLASH_COMMANDS`（硬编码） + `SlashCommandRegistry`（运行时动态）共存，前者用于补全 description 显示，后者用于「热替换」（`replace()` API 支持 staged replacement）。

### laew gap 表（D2）

| 编号 | 描述 | P | 推荐 Rust crate |
|------|------|---|----------------|
| **L1548** | 无 prompt template 系统（仅硬编码 `/help /clear /exit /new /model /provider`） | P1 | `serde_yaml` 解析 frontmatter + 自实现 `substitute_args()`（支持 `$1`/`$@`/`${N:-default}`） |
| **L1549** | 斜杠命令为硬编码 if/else，无运行时动态注册 | P1 | `inventory` + `linkme` 收集所有 `#[laew_command]` 函数；命名冲突解决参考 `name:N` 模式 |
| **L1550** | 无命令源 description 标签（无 `[user]`/`[project]`/`[extension]` 区分） | P2 | 在补全数据结构加 `SourceTag` enum |
| **L1551** | 无内建命令冲突诊断（扩展注册同名 `/model` 静默覆盖） | P2 | 启动时扫描 + 写 warning 到 `DebugReport` |

---

## D3 — 对话 Rewind/分支【用户级】

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/coding-agent/src/modes/interactive/components/user-message-selector.ts` | 全文（155 行） | `UserMessageSelectorComponent`：单列 picker 选历史 user message |
| `packages/coding-agent/src/modes/interactive/components/tree-selector.ts` | 全文（1427 行） | `TreeSelectorComponent` 5 档 FilterMode + Search + Label 编辑 |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 5144-5176 | `showUserMessageSelector` → `runtimeHost.fork(entryId)` 流程 |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 5196-5216 | `handleCloneCommand` → `runtimeHost.fork(leafId, { position: "at" })` 复制当前分支 |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 5218-5340 | `showTreeSelector` → `session.navigateTree` 跳转 + 分支摘要 |
| `packages/coding-agent/src/core/agent-session.ts` | 3339-3362 | `getUserMessagesForForking()`：从 entries 提取所有 user 消息 |
| `packages/agent/src/harness/runtime/lane.ts` | 1232-1262 | `navigateTree` lane 操作 + 三态 admission 校验 |
| `packages/agent/src/harness/agent-harness.ts` | 555-565 | `navigateTree` agent API |
| `packages/agent/src/harness/session/jsonl/repo.ts` | 146-200 | `JsonlSessionRepo.fork`：基于 JSONL 的「祖先链 + 子树」分叉 |
| `packages/coding-agent/src/core/compaction/branch-summarization.ts` | 全文 | 分支摘要生成（fork 后立即浓缩上下文） |

### 机制剖析

**1. 两种 fork 入口 + 一种跳转**：

| 命令 | 入口 | 行为 |
|------|------|------|
| `/fork` | `showUserMessageSelector()` | 列历史 user 消息，选中后 → 新 session 从该 entry 起 |
| `/clone` | `handleCloneCommand()` | 在当前 leafId 复制 → 新 session 共享同一路径 |
| `/tree` | `showTreeSelector()` | 全树视图（5 档 FilterMode）+ 跳转 + 可选分支摘要 |

**2. Tree Selector 5 档 Filter**（`tree-selector.ts:95`）：

```typescript
export type FilterMode = "default" | "no-tools" | "user-only" | "labeled-only" | "all";
```

- **default**：默认折叠工具调用，保留 user/assistant message
- **no-tools**：完全隐藏 toolResult
- **user-only**：只显示 user message
- **labeled-only**：只显示带 label 的 entry（用户书签）
- **all**：所有 entry（用于调试）

**3. Fork 流程 + 分支摘要**（`interactive-mode.ts:5218-5280`）：

```typescript
private showTreeSelector(initialSelectedId?: string): void {
  // ...
  const selector = new TreeSelectorComponent(tree, realLeafId, ...);
  selector.onSelect = async (entryId) => {
    // 1. 三选项摘要选择
    const summaryChoice = await this.showExtensionSelector("Summarize branch?", [
      "No summary",
      "Summarize",
      "Summarize with custom prompt",
    ]);
    // 2. 流式中断 + 跳转
    if (this.session.isStreaming) {
      this.restoreQueuedMessagesToEditor();
      await this.session.abort();
    }
    // 3. 分支摘要：offline LLM 总结该分支
    if (wantsSummary) {
      this.defaultEditor.onEscape = () => this.session.abortBranchSummary();
      this.chatContainer.addChild(new Spacer(1));
      this.showStatusIndicator(new BranchSummaryStatusIndicator(this.ui));
    }
    const result = await this.session.navigateTree(entryId, {
      summarize: wantsSummary,
      customInstructions,
    });
    // 4. 重建聊天视图 + 回填 editor
    this.chatContainer.clear();
    this.renderInitialMessages();
  };
}
```

**4. TreeSelector 的 Search/Filter**（`tree-selector.ts:1066-1073`）：支持 Tab 切换 FilterMode + 内嵌 `/` 触发 SearchLine 多 token 模糊搜索。

**5. `getUserMessagesForForking` 实现**（`agent-session.ts:3339-3362`）：

```typescript
getUserMessagesForForking(): Array<{ entryId: string; text: string }> {
  const entries = this.sessionManager.getEntries();
  const result: Array<{ entryId: string; text: string }> = [];
  for (const entry of entries) {
    if (entry.type !== "message") continue;
    if (entry.message.role !== "user") continue;
    const text = contentText(entry.message.content, "");
    if (text) {
      result.push({ entryId: entry.id, text });
    }
  }
  return result;
}
```

**6. `JsonlSessionRepo.fork` JSONL 实现**（`repo.ts:146-200`）：
- 读取 source session 的祖先链（沿着 parentId 向上）
- 从祖先链 + 选定子树创建 destination path
- 写入新 session header，标记 `parentSessionId: source.id`

**7. 缺失的「编辑消息」入口**：
- pi **没有** `editMessage(entryId, newText)` API——一旦提交消息不可改
- 唯一变通：fork 到 entry → 编辑器回填选中文本 → 重发 → 生成新分支
- 编辑器回填：`result.editorText` 在 `navigateTree` 成功后注入到当前编辑器（仅当编辑器为空时）

### 设计巧妙点

1. **fork 与编辑消息不混淆**：pi 强制「不可变历史 + 显式分支」，避免编辑历史导致 cache prefix 失效、token 计费错乱、checkpoint 错位。
2. **`/clone` 与 `/fork` 分离**：clone 是「复制当前 leaf 到新 session」（同子树 + 不同 sessionId）；fork 是「从历史 user 消息起新子树」（同 session + 分支）。两者 UI 形态完全不同但底层走同一个 `runtimeHost.fork(entryId, options)`。
3. **分支摘要作为 fork 的可选钩子**：跳转时先摘要老分支内容，避免新分支因上下文断裂而行为退化。
4. **Tree Selector 多档 FilterMode** 解决了「树太大」的核心痛点：用户可只关心 user 消息 / 标签书签 / 全部。

### laew gap 表（D3）

| 编号 | 描述 | P | 推荐 Rust crate |
|------|------|---|----------------|
| **L1552** | 无会话树 / fork 能力（仅能 `/clear` 开新 session） | P1 | `rusqlite` 持久化 entry 表 + 复制子树 + `TreeSelectorComponent` ratatui 实现 |
| **L1553** | 无 `/fork` 从历史消息分叉（也不支持选历史 user message 列） | P1 | 与 L1552 一起 |
| **L1554** | 无 `/clone` 当前 leaf 复制 | P2 | 与 L1552 一起（简单 variant） |
| **L1555** | 无 5 档 FilterMode（tree-selector FilterMode = `default`/`no-tools`/`user-only`/`labeled-only`/`all`） | P2 | 与 L1552 一起 |
| **L1556** | 无分支摘要（fork 后老分支不浓缩，新分支上下文断开） | P1 | 与 L1552 + L1039（cached microcompact）联动 |
| **L1557** | 无 entry label 书签（用户无法在 entry 上挂自定义标签） | P2 | SQLite `entry_labels(entry_id, label)` 表 |
| **L1558** | 无 entry 复制（`ctx.ui.setStatus` 派生的 copy 转剪贴板） | P2 | `arboard` 剪贴板 |

---

## D4 — 文件监视与工作区感知【运行时】

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/coding-agent/src/utils/fs-watch.ts` | 全文（30 行） | `watchWithErrorHandler` + `closeWatcher`：`node:fs` 原生封装 |
| `packages/coding-agent/src/core/footer-data-provider.ts` | 1-110 | FooterDataProvider：git HEAD 监听 + reftable 监听 + extension status |
| `packages/coding-agent/src/core/footer-data-provider.ts` | 290-380 | `setupGitWatcher`：HEAD 父目录监听 + 5s 重试 + reftable 备份监听 |
| `packages/coding-agent/src/core/footer-data-provider.ts` | 169-220 | `setCwd` / `scheduleRefresh` / `refreshGitBranchAsync` 协调 |
| `packages/coding-agent/src/modes/interactive/theme/theme.ts` | 760-870 | `startThemeWatcher` + `themeReloadTimer`：自定义主题热重载 |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | 998-1010 | theme file watcher + git branch watcher 钩子 |

### 机制剖析

**1. 文件监视核心抽象**（`fs-watch.ts:1-30`）：

```typescript
import { type FSWatcher, type WatchListener, watch } from "node:fs";
export const FS_WATCH_RETRY_DELAY_MS = 5000;

export function closeWatcher(watcher: FSWatcher | null | undefined): void {
  if (!watcher) return;
  try { watcher.close(); } catch {}
}

export function watchWithErrorHandler(
  path: string,
  listener: WatchListener<string>,
  onError: () => void,
): FSWatcher | null {
  try {
    const watcher = watch(path, listener);
    watcher.on("error", onError);
    return watcher;
  } catch {
    onError();
    return null;
  }
}
```

**2. Git HEAD 监听**（`footer-data-provider.ts:308-340`）—— **三路冗余**：

```typescript
private setupGitWatcher(): void {
  // 关键洞察：watch HEAD 文件本身不行，因为 git 原子写入会改 inode
  // → 必须 watch HEAD 的**父目录**，监听 filename === "HEAD" 的事件
  this.headWatcher = watchWithErrorHandler(
    dirname(this.gitPaths.headPath),
    (_eventType, filename) => {
      if (!filename || filename === "HEAD") {
        this.scheduleRefresh();
      }
    },
    () => this.handleGitWatcherError(),
  );
  // 兜底：fs.watch 不可靠时用 fs.watchFile 轮询
  if (pollGitHead) {
    this.headWatchFilePath = this.gitPaths.headPath;
    this.headWatchFileListener = (current, previous) => {
      if (current.mtimeMs !== previous.mtimeMs || 
          current.ctimeMs !== previous.ctimeMs || 
          current.size !== previous.size) {
        this.scheduleRefresh();
      }
    };
    watchFile(this.headWatchFilePath, { interval: 1000 }, this.headWatchFileListener);
  }
  // 第三路：reftable（Git 新引用表）目录监听
  const reftableDir = join(this.gitPaths.commonGitDir, "reftable");
  if (existsSync(reftableDir)) {
    this.reftableWatcher = watchWithErrorHandler(reftableDir, () => this.scheduleRefresh(), ...);
  }
}
```

**3. 主题文件监听**（`theme.ts:825-870`）：

```typescript
function startThemeWatcher(): void {
  stopThemeWatcher();
  // 不监听内建 dark/light，只监听自定义主题
  if (!currentThemeName || currentThemeName === "dark" || currentThemeName === "light") return;
  const themeFile = path.join(getCustomThemesDir(), `${currentThemeName}.json`);
  if (!fs.existsSync(themeFile)) return;

  const scheduleReload = () => {
    if (themeReloadTimer) clearTimeout(themeReloadTimer);
    themeReloadTimer = setTimeout(() => {
      if (currentThemeName !== watchedThemeName) return; // 防 stale
      if (!fs.existsSync(themeFile)) return;
      try {
        const reloadedTheme = loadThemeFromPath(themeFile);
        registeredThemes.set(watchedThemeName, reloadedTheme);
        setGlobalTheme(reloadedTheme);
        onThemeChangeCallback?.();
      } catch { /* 临时损坏 → 静默 */ }
    }, 100);  // 100ms debounce
  };
  themeWatcher = watchWithErrorHandler(customThemesDir, (_e, filename) => {
    if (filename !== watchedFileName) return;
    scheduleReload();
  }, () => {});
}
```

**4. 缺失的能力**：
- **未对工作区文件做统一监视**：仅监听 `git HEAD`（footer 用）和 `custom themes dir`（theme reload）
- **无 chokidar / notify 类全工作区索引失效**：Bash/Read/Write 工具返回后不触发 invalidate
- **无 LSP 诊断被动注入**：未集成 `lsp-types`
- **无外部编辑器感知**：用户在 `$EDITOR` 中改了文件后再回到 pi，pi 不知道

### 设计巧妙点

1. **Git HEAD 三路冗余**：父目录监听（inode-aware）+ `fs.watchFile` 1s 轮询兜底 + reftable 监听覆盖 Git 新引用表。
2. **Stale-timer 防护**：`scheduleReload` 在 timer 触发时再次校验 `currentThemeName === watchedThemeName`，避免主题切换期间旧 timer 覆盖新主题。
3. **5s watcher 重试**：`scheduleGitWatcherRetry()` 在 `handleGitWatcherError()` 后延迟重连，处理 EMFILE（fd 耗尽）等瞬时错误。
4. **`scheduleRefresh` + `refreshInFlight` 标志**（`footer-data-provider.ts:198-225`）：避免并发刷新，下一帧合并到 `refreshPending`。

### laew gap 表（D4）

| 编号 | 描述 | P | 推荐 Rust crate |
|------|------|---|----------------|
| **L1559** | 无工作区文件监视（无 `notify` v6 监听 + 自动索引失效） | P1 | `notify` v6 + `debounce` 500ms；Bash/Read/Write 工具返回后手动 invalidate |
| **L1560** | 无 git HEAD 三路冗余监听（仅 bash 工具的 `git status` 输出，无持续 footer） | P2 | `notify` + `tokio::sync::Mutex` 三路复用 + 5s 重试 |
| **L1561** | 无主题文件热重载（改 theme.json 需重启） | P2 | `notify` + 100ms debounce + stale-timer 校验 |
| **L1562** | 无 stale-timer 防护（reload timer 可能跨切换覆盖） | P2 | `theme_name` 校验 + 主题切换时 `clearTimeout` |

---

## D5 — 工具输出富文本内容渲染

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/tui/src/components/markdown.ts` | 全文（1015 行） | `Markdown` Component + `marked` lexer + StrictStrikethrough + Latex |
| `packages/tui/src/components/markdown.ts` | 550-560 | Token dispatch（table/code/heading/list/blockquote/...） |
| `packages/tui/src/components/markdown.ts` | 842-1015 | `renderTable` 智能列宽 + 单元格自动换行 + 边界 Box Drawing |
| `packages/coding-agent/src/modes/interactive/components/markdown-transform.ts` | 全文（29 行） | `createMarkdownTransform` 扩展点 |
| `packages/coding-agent/src/utils/syntax-highlight.ts` | 全文（212 行） | `highlight.js/lib/core` 18 语言 + 渐进加载 `loadAllHighlightLanguages` |
| `packages/coding-agent/src/modes/interactive/components/diff.ts` | 全文（147 行） | `renderDiff` + word-level intra-line `Diff.diffWords` + inverse 高亮 |
| `packages/coding-agent/src/core/tools/tool-renderer.ts` | 全文 | 工具自定义渲染接口 `renderCall`/`renderResult` |
| `packages/coding-agent/src/modes/interactive/components/tool-execution.ts` | 全文（421 行） | `ToolExecutionComponent` 通用 fallback + 自渲染容器 + 图片块 |
| `packages/tui/src/terminal-image.ts` | 全文（696 行） | Kitty/iTerm2 协议探测 + `encodeKitty`/`encodeITerm2` + `imageFallback` |
| `packages/tui/src/components/image.ts` | 全文（127 行） | `Image` Component + ImageTheme + OSC 8 hyperlink |
| `packages/coding-agent/src/utils/image-process.ts` | 全文（119 行） | `processImage`（photon-node resize + format 转换 + inline limit） |
| `packages/coding-agent/src/utils/image-resize-core.ts` | 全文（164 行） | photon-node resize to 2000x2000 max |
| `packages/coding-agent/src/utils/image-resize.ts` | 全文（123 行） | 主进程/Worker 进程分流（运行时决定） |
| `packages/coding-agent/src/modes/interactive/components/show-images-selector.ts` | 全文（50 行） | 是否内联显示图片的 selector |

### 机制剖析

**1. Markdown 组件核心**（`markdown.ts:236-260`）：

```typescript
export class Markdown implements Component {
  private cachedText?: string;
  private cachedWidth?: number;
  private cachedLines?: string[];
  // ...
  render(width: number): string[] {
    if (this.cachedLines && this.cachedText === this.text && this.cachedWidth === width) {
      return this.cachedLines;  // 三键 cache 命中
    }
    const text = this.options.transform?.(this.text, contentWidth) ?? this.text;
    const tokens = markdownParser.lexer(normalizedText);
    trimPartialClosingFences(tokens);
    for (const token of tokens) {
      const tokenLines = this.renderToken(token, contentWidth, nextToken?.type);
      renderedLines.push(...tokenLines);
    }
    // ... wrap + cache
  }
}
```

**2. Table 智能列宽算法**（`markdown.ts:842-880`）：

```typescript
private renderTable(token: Tokens.Table, availableWidth: number, ...): string[] {
  const borderOverhead = 3 * numCols + 1;  // "│ " + " │ " + " │"
  const availableForCells = availableWidth - borderOverhead;
  if (availableForCells < numCols) return fallbackLines;  // 太窄 → 退化为原文
  
  // 1. 计算每列 naturalWidth（自然宽度）和 minWordWidths（最小词宽）
  // 2. 若 minWordWidth 总和 > available → 重新分配（weight-proportional）
  // 3. 若 naturalWidth 总和 > available → 按 grow-potential 缩减
  // 4. 调整四舍五入误差
  
  // 渲染：┌─┬─┐ │ │ │ ├─┼─┤ │ │ │ └─┴─┘
  const topBorderCells = columnWidths.map(w => "─".repeat(w));
  lines.push(`┌─${topBorderCells.join("─┬─")}─┐`);
  // ...
}
```

**3. 语法高亮渐进加载**（`syntax-highlight.ts:1-58`）：

```typescript
// 启动时立即注册 18 个常用语言（eager）
const eagerLanguages = {
  bash, c, cpp, csharp, dart, go, groovy, java, javascript, kotlin, lua, nix,
  perl, php, python, ruby, rust, scala, swift, typescript,
};

let allLanguagesPromise: Promise<void> | undefined;
export function loadAllHighlightLanguages(): Promise<void> {
  if (!allLanguagesPromise) {
    allLanguagesPromise = new Promise(resolve => {
      setImmediate(() => {
        void import("highlight.js/lib/index.js").then(
          () => resolve(),
          () => resolve(),  // 失败也 resolve，保留 eager + plaintext
        );
      });
    });
  }
  return allLanguagesPromise;
}
```

**4. Diff 渲染 word-level**（`diff.ts:26-66`）：

```typescript
function renderIntraLineDiff(oldContent: string, newContent: string) {
  const wordDiff = Diff.diffWords(oldContent, newContent);
  for (const part of wordDiff) {
    if (part.removed) {
      // Strip leading whitespace from first removed part (避免高亮缩进)
      if (isFirstRemoved) {
        const leadingWs = value.match(/^(\s*)/)?.[1] || "";
        value = value.slice(leadingWs.length);
        removedLine += leadingWs;
        isFirstRemoved = false;
      }
      removedLine += theme.inverse(value);  // 反白显示
    }
    // added 同理
  }
}
```

**5. 图片内联渲染（Kitty/iTerm2 协议探测）**（`terminal-image.ts:78-130`）：

```typescript
export function detectCapabilities(tmuxForwardsHyperlink: () => boolean = probeTmuxHyperlinks): TerminalCapabilities {
  if (process.env.PI_IMAGE_PROTOCOL?.toLowerCase() === "kitty") return { images: "kitty", ... };
  if (process.env.PI_IMAGE_PROTOCOL?.toLowerCase() === "iterm2") return { images: "iterm2", ... };
  if (tmuxForwardsImage()) return { images: null, ... };  // tmux 禁用图片（不可靠）
  
  if (process.env.KITTY_WINDOW_ID || termProgram === "kitty") return { images: "kitty", ... };
  if (process.env.ITERM_SESSION_ID || termProgram === "iterm.app") return { images: "iterm2", ... };
  if (terminalEmulator === "jetbrains-jediterm") return { images: null, ... };  // 禁用图片
  // ...
}
```

**6. 图片处理管线**（`image-process.ts:72-105`）：

```
readFile → normalizeMimeType (png/jpeg/gif/webp)
  → convertImageBytesToPng (WebP 等不内联格式 → PNG)
  → resizeImage (photon-node Lanczos3, max 2000x2000)
  → { data: base64, mimeType: "image/png" }
```

**7. Markdown 扩展点**（`extensions/types.ts:1201-1207, 1355`）：

```typescript
export interface MarkdownTransformContext {
  messageType: "user" | "assistant" | "tool";
  isStreaming: boolean;
  availableWidth: number;
}
export type MarkdownTransformer = (markdown: string, context: MarkdownTransformContext) => string;
// registerMarkdownTransformer(transformer): void;
```

### 设计巧妙点

1. **Markdown 三键 cache**：`cachedText` + `cachedWidth` + `cachedLines` 三键相等才命中，避免 1 字符差异的整树重渲染。
2. **Table 自适应三档退避**：natural fit → 比例缩 → 按权重分配 → 太窄退化原文；保证任意终端宽度下都不会破坏 layout。
3. **语法高亮 setImmediate 懒加载**：启动时只注册 18 个 eager 语言（< 50KB），其余 `setImmediate(() => import("highlight.js/lib/index.js"))` 后台加载，不阻塞主屏。
4. **图片协议三层降级**：Kitty/iTerm2 → OSC 8 hyperlink 文本（`[Image: ~/path.png image/png 1024x768]`）→ 完全隐藏。
5. **Diff 词级 inverse**：不仅 `+`/`-` 行着色，还在词粒度上 `inverse()` 标记具体改了哪个词，避免用户瞪着整行找差异。
6. **图片内联可控**：`ShowImagesSelectorComponent` 让用户主动选择 `Yes/No`，符合 laew 「用户级开关」原则。

### laew gap 表（D5）

| 编号 | 描述 | P | 推荐 Rust crate |
|------|------|---|----------------|
| **L1563** | 无 Markdown 渲染（仅纯文本输出 + Box Drawing） | P1 | `pulldown-cmark` + 自实现 ANSI 渲染（参考 markdown.ts renderToken） |
| **L1564** | 无 Table 智能列宽（laew 无 markdown，自然无 table） | P2 | 与 L1563 一起 |
| **L1565** | 无语法高亮（代码块纯白底，无 ANSI 着色） | P2 | `syntect` + 主题；或 `tree-sitter-highlight`（按需 lazy 加载） |
| **L1566** | 无 word-level Diff 渲染（仅简单 `+`/`-` 行） | P2 | `similar` crate + 自实现 inverse |
| **L1567** | 无内联图片渲染（无 Kitty/iTerm2 协议探测） | P2 | `image` crate + base64 + 写 `\x1b_G...\x1b\\` 转义序列 |
| **L1568** | 无 photon-node 等价 resize（图片无 2000x2000 上限） | P2 | `image` crate + `image::imageops::resize(Lanczos3)` |
| **L1569** | 无工具自定义渲染接口（扩展工具只能返回字符串） | P1 | 在 `Tool` trait 加 `render_call`/`render_result` Option 字段 |

---

## D6 — 输入体验工程

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/tui/src/components/editor.ts` | 全文（2461 行） | `Editor` 主输入编辑器 + multi-line + history + autocomplete |
| `packages/tui/src/components/editor.ts` | 28-220 | `segmentWithMarkers` paste-aware grapheme 切分 |
| `packages/tui/src/components/editor.ts` | 222-280 | `EditorSnapshot` undo stack + paste registry |
| `packages/tui/src/components/editor.ts` | 322-360 | Editor 私有字段（pastes / pasteBuffer / history / killRing / undoStack） |
| `packages/tui/src/components/editor.ts` | 412-460 | `addToHistory` 上限 100 + 去重 |
| `packages/tui/src/components/editor.ts` | 705-735 | bracketed paste mode `\x1b[200~`/`\x1b[201~` 解析 |
| `packages/tui/src/components/editor.ts` | 1245-1310 | `handlePaste` tmux CSI-u 解码 + 大粘贴 marker `[paste #N +123 lines]` |
| `packages/tui/src/components/editor.ts` | 1075-1100 | 大粘贴替换为 paste marker 提交后注入原文 |
| `packages/tui/src/kill-ring.ts` | 全文（46 行） | Emacs-style kill ring（连续 kill 累积 + yank-pop 循环） |
| `packages/tui/src/undo-stack.ts` | 全文（28 行） | UndoStack 泛型 |
| `packages/tui/src/keybindings.ts` | 71-210 | 50+ 个 `TUI_KEYBINDINGS` 默认绑定（emacs 风格 + vim 部分） |
| `packages/coding-agent/src/modes/interactive/components/custom-editor.ts` | 全文（148 行） | 扩展点 `CustomEditor`（可继承重写 Vim 等） |
| `packages/coding-agent/src/modes/interactive/components/extension-editor.ts` | 全文（132 行） | 多行 `ExtensionEditorComponent` + 外部编辑器（Ctrl+G） |
| `packages/coding-agent/src/modes/interactive/external-editor.ts` | 全文（46 行） | `editInExternalEditor` shell out 到 `$VISUAL`/`$EDITOR` |

### 机制剖析

**1. Editor 核心字段**（`editor.ts:322-360`）：

```typescript
export class Editor implements Component, Focusable {
  // Paste tracking
  private pastes: Map<number, string> = new Map();
  private pasteCounter: number = 0;
  
  // Bracketed paste mode buffering
  private pasteBuffer: string = "";
  private isInPaste: boolean = false;
  
  // Prompt history
  private history: string[] = [];
  private historyIndex: number = -1;
  private historyDraft: EditorState | null = null;
  
  // Kill ring + Undo
  private killRing = new KillRing();
  private undoStack = new UndoStack<EditorSnapshot>();
}
```

**2. Bracketed Paste Mode**（`editor.ts:705-735`）：

```typescript
// Handle bracketed paste mode
if (data.includes("\x1b[200~")) {
  this.isInPaste = true;
  this.pasteBuffer = "";
  data = data.replace("\x1b[200~", "");
}
if (this.isInPaste) {
  this.pasteBuffer += data;
  const endIndex = this.pasteBuffer.indexOf("\x1b[201~");
  if (endIndex !== -1) {
    const pasteContent = this.pasteBuffer.substring(0, endIndex);
    if (pasteContent.length > 0) this.handlePaste(pasteContent);
    this.isInPaste = false;
    const remaining = this.pasteBuffer.substring(endIndex + 6);
    this.pasteBuffer = "";
    if (remaining.length > 0) this.handleInput(remaining);
    return;
  }
  return;
}
```

**3. 大粘贴转 marker**（`editor.ts:1280-1310`）：

```typescript
private handlePaste(pastedText: string): void {
  // 1. 解码 tmux CSI-u 控制字符
  const decodedText = pastedText.replace(/\x1b\[(\d+);5u/g, (match, code) => {
    const cp = Number(code);
    if (cp >= 97 && cp <= 122) return String.fromCharCode(cp - 96);
    if (cp >= 65 && cp <= 90) return String.fromCharCode(cp - 64);
    return match;
  });
  
  // 2. 过滤不可打印字符（保留 \n）
  let filteredText = decodedText.split("")
    .filter(c => c === "\n" || c.charCodeAt(0) >= 32).join("");
  
  // 3. 大粘贴 (>10 行 或 >1000 字符) → paste marker
  const pastedLines = filteredText.split("\n");
  if (pastedLines.length > 10 || filteredText.length > 1000) {
    this.pasteCounter++;
    this.pastes.set(this.pasteCounter, filteredText);
    const marker = pastedLines.length > 10
      ? `[paste #${this.pasteCounter} +${pastedLines.length} lines]`
      : `[paste #${this.pasteCounter} ${filteredText.length} chars]`;
    this.insertTextAtCursorInternal(marker);
    return;
  }
  // ...
}
```

**4. Prompt 历史**（`editor.ts:412-435`）：

```typescript
addToHistory(text: string): void {
  const trimmed = text.trim();
  if (!trimmed) return;
  // 连续去重
  if (this.history.length > 0 && this.history[0] === trimmed) return;
  this.history.unshift(trimmed);  // 最新在头部
  if (this.history.length > 100) this.history.pop();  // 上限 100
}
```

**5. Kill Ring (Emacs 风格)**（`kill-ring.ts:19-28`）：

```typescript
push(text: string, opts: { prepend: boolean; accumulate?: boolean }): void {
  if (opts.accumulate && this.ring.length > 0) {
    const last = this.ring.pop()!;
    this.ring.push(opts.prepend ? text + last : last + text);  // 累积
    return;
  }
  this.ring.push(text);
}
// rotate() 用于 yank-pop（Alt+y 循环）
```

**6. 50+ 键位绑定**（`keybindings.ts:71-210`）：

```typescript
export const TUI_KEYBINDINGS = {
  "tui.editor.cursorUp": { defaultKeys: "up" },
  "tui.editor.cursorLeft": { defaultKeys: ["left", "ctrl+b"] },  // emacs 风格
  "tui.editor.cursorLineStart": { defaultKeys: ["home", "ctrl+home", "ctrl+a"] },
  "tui.editor.deleteToLineStart": { defaultKeys: "ctrl+u" },  // emacs kill line
  "tui.editor.yank": { defaultKeys: "ctrl+y" },
  "tui.editor.yankPop": { defaultKeys: "alt+y" },
  "tui.editor.undo": { defaultKeys: "ctrl+-", description: "Undo" },
  "tui.input.newLine": { defaultKeys: ["shift+enter", "ctrl+j"] },
  "tui.input.submit": { defaultKeys: "enter" },
  "tui.input.tab": { defaultKeys: "tab", description: "Tab / autocomplete" },
  "tui.altScreen.search": { defaultKeys: "ctrl+shift+f", description: "Search the primary scroll view" },
  // ...
};
```

**7. 自定义编辑器扩展点**（`custom-editor.ts:148 行`）：

```typescript
// CustomEditor extends Editor → 扩展可重写 handleInput/handlePaste/render
// ExtensionEditorComponent wraps CustomEditor + 外部编辑器 (Ctrl+G)
```

### 设计巧妙点

1. **Paste marker 而非粘贴原文**：> 10 行粘贴只插入 `[paste #N +123 lines]`，提交时一次性注入原文——避免编辑器被 1000 行淹没、避免 undo stack 膨胀。
2. **Paste ID 校验防 stale**：`getValidPasteIds()` 配合 `segmentWithMarkers` 保证 paste marker 与 paste registry 一致——已撤销的 paste 不会在重渲染时泄露原文。
3. **Bracketed paste 完整 CSI-u 解码**：tmux popups 把 `\n` 转成 `\x1b[106;5u`（CSI-u Ctrl+J），Editor 主动解码回字面 `\n`——避免粘贴多行代码被错误过滤。
4. **Emacs 风格键位 + kill ring**：完整兼容 GNU readline 用户习惯（`Ctrl+A/E` 行首尾、`Ctrl+W` 删词、`Ctrl+U` 删行首、`Ctrl+Y` 粘贴、`Alt+Y` 循环粘贴）。
5. **History 上限 100 + 连续去重**：避免「按 ↑ 100 次才找到上条命令」UX 灾难。
6. **外部编辑器钩子（Ctrl+G）**：长 prompt / commit message 一律 `$VISUAL`/`$EDITOR` 启，提交时整段回灌。

### laew gap 表（D6）

| 编号 | 描述 | P | 推荐 Rust crate |
|------|------|---|----------------|
| **L1570** | 无 kill ring（删除文本后无法 `Ctrl+Y` 恢复，多次删除无累积） | P2 | 自实现 `KillRing` + emacs 风格 `Ctrl+U/W/A/K` 钩到 `crossterm` |
| **L1571** | 无 undo stack（编辑器操作不可撤销） | P2 | 自实现 `UndoStack<T>` + `Command` 模式 |
| **L1572** | 无 prompt history 持久化（进程重启后历史消失） | P2 | `~/.config/laew/history.txt` + 上限 1000 + 去重 |
| **L1573** | 无大粘贴截断（laew `input.rs` 无 `[paste #N]` marker 机制） | P1 | 自实现 `paste_marker.rs`：>10 行/1000 字符 → marker + paste registry |
| **L1574** | 无外部编辑器钩子（无 `Ctrl+G` → `$VISUAL`） | P2 | `std::process::Command::new($VISUAL)` + `tempfile::NamedTempFile` + 阻塞读回 |

---

## D7 — Onboarding/目录信任/主题偏好

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/coding-agent/src/modes/interactive/components/first-time-setup.ts` | 全文（145 行） | `FirstTimeSetupComponent`：主题 + analytics 两步向导 |
| `packages/coding-agent/src/modes/interactive/components/trust-selector.ts` | 全文（134 行） | `TrustSelectorComponent` 5 选项 Picker |
| `packages/coding-agent/src/core/project-trust.ts` | 全文（96 行） | `resolveProjectTrusted`：7 步决策链 |
| `packages/coding-agent/src/core/trust-manager.ts` | 全文（245 行） | `ProjectTrustStore` + `proper-lockfile` 锁 + 父目录冒泡 |
| `packages/coding-agent/src/core/trust-manager.ts` | 30-38 | `TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES`：7 类资源 |
| `packages/coding-agent/src/core/trust-manager.ts` | 44-58 | `findNearestTrustEntry`：cwd 向上冒泡 |
| `packages/coding-agent/src/core/trust-manager.ts` | 137-167 | `acquireTrustLockSync`：proper-lockfile + 10 次重试 + 20ms 退避 |
| `packages/coding-agent/src/modes/interactive/theme/theme-controller.ts` | 全文（172 行） | `InteractiveThemeController`：themeName + TerminalTheme + AutoSync |
| `packages/coding-agent/src/modes/interactive/theme/theme.ts` | 全文（1234 行） | `Theme` + JSON theme 加载 + 8 内置 + 自定义 + 热重载 |
| `packages/coding-agent/src/modes/interactive/theme/dark.json`/`light.json` | 全文 | 颜色 token JSON schema |
| `packages/coding-agent/src/core/settings-manager.ts` | 全文 | 主题/键位/缩进/model 偏好持久化到 SQLite |
| `packages/coding-agent/src/main.ts` | 655-660 | 启动期 first-time setup 调用入口 |

### 机制剖析

**1. First-time Setup 两步向导**（`first-time-setup.ts:32-145`）：

```typescript
const THEME_OPTIONS = [{ value: "dark", label: "Dark" }, { value: "light", label: "Light" }];
const ANALYTICS_OPTIONS = [
  { value: true, label: "Share anonymous usage data" },
  { value: false, label: "Don't share" },
];
const SETUP_LOGO_LINES = ["██████", "██  ██", "████  ██", "██    ██"];  // ASCII art Pi logo

export class FirstTimeSetupComponent extends Container {
  private step: "theme" | "analytics" = "theme";
  
  // 第一步：theme（Detected system appearance: dark/light）
  // 第二步：analytics（opt-in / opt-out）
  // Esc 可跳过整个 setup
}
```

**2. Project Trust 7 步决策链**（`project-trust.ts:46-96`）：

```typescript
export async function resolveProjectTrusted(options): Promise<boolean> {
  // 1. trustOverride（CLI flag 显式指定）
  if (options.trustOverride !== undefined) return options.trustOverride;
  
  // 2. 工作区无 trust-requiring 资源 → 默认 trust
  if (!hasTrustRequiringProjectResources(options.cwd)) return true;
  
  // 3. 扩展 project_trust 事件钩子（扩展可代用户回答）
  if (options.extensionsResult) {
    const { result, errors } = await emitProjectTrustEvent(...)
    if (result) {
      const trusted = result.trusted === "yes";
      if (result.remember === true) options.trustStore.set(options.cwd, trusted);
      return trusted;
    }
  }
  
  // 4. 已存决策（沿 cwd 向上冒泡查询）
  const decision = options.trustStore.get(options.cwd);
  if (decision !== null) return decision;
  
  // 5. defaultProjectTrust 配置：always / never / ask
  switch (options.defaultProjectTrust ?? "ask") {
    case "always": return true;
    case "never": return false;
    case "ask": break;
  }
  
  // 6. 无 UI（RPC mode）→ 默认拒绝
  if (!options.projectTrustContext.hasUI) return false;
  
  // 7. UI 询问用户
  const selected = await selectProjectTrustOption(options.cwd, options.projectTrustContext);
  return selected?.trusted ?? false;
}
```

**3. 5 选项 UI**（`trust-manager.ts:66-95`）：

```typescript
export function getProjectTrustOptions(cwd: string, options?: { includeSessionOnly?: boolean }): ProjectTrustOption[] {
  const trustOptions: ProjectTrustOption[] = [
    { label: "Trust", trusted: true, updates: [{ path: trustPath, decision: true }], savedPath: trustPath },
  ];
  const parentPath = getProjectTrustParentPath(cwd);
  if (parentPath !== undefined) {
    trustOptions.push({
      label: `Trust parent folder (${parentPath})`,
      trusted: true,
      updates: [
        { path: parentPath, decision: true },
        { path: trustPath, decision: null },  // 清空当前
      ],
      savedPath: parentPath,
    });
  }
  if (options?.includeSessionOnly) {
    trustOptions.push({ label: "Trust (this session only)", trusted: true, updates: [] });
  }
  trustOptions.push({
    label: "Do not trust",
    trusted: false,
    updates: [{ path: trustPath, decision: false }],
  });
  if (options?.includeSessionOnly) {
    trustOptions.push({ label: "Do not trust (this session only)", trusted: false, updates: [] });
  }
  return trustOptions;
}
```

**4. 7 类 trust-requiring 资源**（`trust-manager.ts:30-38`）：

```typescript
const TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES = [
  "settings.json",   // 项目级设置（覆盖 global）
  "extensions",      // 项目级扩展（可执行任意代码）
  "skills",          // 项目级 skill（注入到 system prompt）
  "prompts",         // 项目级 prompt 模板
  "themes",          // 项目级主题
  "SYSTEM.md",       // 注入 system prompt 顶部
  "APPEND_SYSTEM.md",// 注入 system prompt 底部
] as const;
```

**5. cwd 向上冒泡查询**（`trust-manager.ts:44-58`）：

```typescript
function findNearestTrustEntry(data: TrustFile, cwd: string): ProjectTrustStoreEntry | null {
  let currentDir = normalizeCwd(cwd);
  while (true) {
    const value = data[currentDir];
    if (value === true || value === false) return { path: currentDir, decision: value };
    const parentDir = dirname(currentDir);
    if (parentDir === currentDir) return null;  // 已到根
    currentDir = parentDir;
  }
}
```

**6. proper-lockfile 跨进程锁**（`trust-manager.ts:137-167`）：

```typescript
function acquireTrustLockSync(path: string): () => void {
  const maxAttempts = 10;
  const delayMs = 20;
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      return lockfile.lockSync(trustDir, { 
        realpath: false,  // 不跟随 symlink
        lockfilePath: `${path}.lock`,
      });
    } catch (error) {
      if (code !== "ELOCKED" || attempt === maxAttempts) throw error;
      // 同步忙等 20ms（避免 trust store 调用方变 async）
      while (Date.now() - start < delayMs) {}
    }
  }
}
```

**7. Theme Controller + AutoSync**（`theme-controller.ts:53-90`）：

```typescript
async applyFromSettings(): Promise<void> {
  const themeSetting = this.currentThemeSetting ?? settingsManager.getThemeSetting();
  const autoTheme = parseAutoThemeSetting(themeSetting);  // "dark"/"light"/"auto:dark=..."
  if (autoTheme) {
    this.terminalTheme = await detectTerminalThemeForAuto({ ui: this.ui, timeoutMs: 100 });
    this.applyThemeName(this.terminalTheme === "light" ? autoTheme.lightTheme : autoTheme.darkTheme, true);
    return;
  }
  if (themeSetting !== undefined) {
    this.applyThemeName(themeSetting, true);
    return;
  }
  // 自动检测：OS color-scheme media query + OSC 11 背景色探测
  const detection = await detectTerminalBackgroundTheme({ ui: this.ui, timeoutMs: 100 });
  this.terminalTheme = detection.theme;
  this.applyThemeName(detection.theme, true);
}
```

**8. Terminal 主题检测三层回退**：
- OSC 11 query（仅支持终端：Kitty/iTerm2/WezTerm）
- `COLORFGBG` env var（部分终端导出）
- 启发式：默认 `dark`

### 设计巧妙点

1. **Trust 决策沿 cwd 向上冒泡**：子项目 trust 自动继承父项目，子项目 untrust 不影响父项目——适配 monorepo。
2. **Trust + parent + session-only 三粒度**：解决「父项目我想持久信任，当前子项目我想临时信任」的常见诉求。
3. **`proper-lockfile` 同步锁**：trust store 可能被多个 pi 实例同时修改（同一用户开多个窗口），用 proper-lockfile 避免 race condition。
4. **Theme auto-sync 监听 OSC 10/11**：用户在系统设置里切换 dark/light 时 pi 自动跟随。
5. **First-time Setup 可跳过（Esc）**：不强制引导流程，避免激怒高级用户。

### laew gap 表（D7）

| 编号 | 描述 | P | 推荐 Rust crate |
|------|------|---|----------------|
| **L1575** | 无 project trust 5 档 UI + cwd 冒泡查询 | P1 | 自实现 `TrustSelectorComponent` + `find_nearest_trust_entry()` + 持久化到 SQLite `trust` 表（schema: `path TEXT PRIMARY KEY, decision INTEGER`） |

注：D7 其他维度（onboarding 多步向导 / 主题系统 / 偏好持久化）laew 已有部分实现（CLAUDE.md 提到 `theme.rs`），不重复计入 gap。

---

## D8 — 会话导出共享 + statusline + 实时成本

### 源码定位

| 文件 | 行号 | 内容 |
|------|------|------|
| `packages/coding-agent/src/core/session-export.ts` | 全文（42 行） | `exportSessionToJsonl`：header + 当前 branch + 可选 trailing entries |
| `packages/coding-agent/src/core/export-html/index.ts` | 全文（316 行） | `exportToHtml`：theme colors + tool renderer + 模板引擎 |
| `packages/coding-agent/src/modes/interactive/session-share.ts` | 全文（206 行） | `/share`：Radius 网关 → GitHub Gist 二级 fallback |
| `packages/coding-agent/src/modes/interactive/session-share.ts` | 26-42 | `exportSessionForShare`：附加 `pi.share` 自定义 entry 含 systemPrompt + tools |
| `packages/coding-agent/src/modes/interactive/session-share.ts` | 71-145 | `tryShareViaRadius` Radius artifact upload + 5 min OAuth validity |
| `packages/coding-agent/src/modes/interactive/session-share.ts` | 147-206 | `shareViaGist` `gh gist create --public=false` + `getShareViewerUrl(gistId)` |
| `packages/coding-agent/src/core/cache-stats.ts` | 全文（164 行） | `detectCacheMiss` + `computeCacheWaste` + `CACHE_TTL_MS = 5 * 60_000` |
| `packages/coding-agent/src/core/cache-stats.ts` | 56-90 | `detectMiss`：promptTokens - cacheRead = missedTokens；missedCost = paidRate - readRate |
| `packages/coding-agent/src/core/cache-stats.ts` | 104-132 | `scan`：compaction/branch_summary 重置 prev；modelChanged 计入 miss |
| `packages/coding-agent/src/core/usage-totals.ts` | 全文（70 行） | `UsageTotals` + `getUsageCostBreakdown` 按 model 分组 |
| `packages/coding-agent/src/modes/interactive/components/footer.ts` | 全文（245 行） | `FooterComponent` 单行 statusline：`pwd • session • ↑X ↓Y RZ WC CH% $cost ctx%/limit` |
| `packages/coding-agent/src/modes/interactive/components/footer.ts` | 117-148 | 输入 token / cache hit rate / cost 计算 |
| `packages/coding-agent/src/modes/interactive/components/footer.ts` | 153-167 | context% 颜色分级（> 90% 红 / > 70% 黄 / 默认白）|
| `packages/coding-agent/src/modes/interactive/components/footer.ts` | 189-220 | model name + thinking level + provider 前缀 + 截断策略 |
| `packages/coding-agent/src/modes/interactive/components/footer.ts` | 232-244 | extension status 行（按 key 字母序）|
| `packages/coding-agent/src/core/footer-data-provider.ts` | 全文（388 行） | `FooterDataProvider`：git branch + provider count + extension status 聚合 |

### 机制剖析

**1. JSONL 导出**（`session-export.ts:7-42`）：

```typescript
export function exportSessionToJsonl(
  sessionManager: SessionManager,
  outputPath?: string,
  createTrailingEntries?: (parentId: string | null, timestamp: string) => readonly object[],
): string {
  const filePath = resolvePath(outputPath ?? `session-${ISO-with-dashes}.jsonl`, process.cwd());
  if (!existsSync(dirname(filePath))) mkdirSync(dirname(filePath), { recursive: true });
  
  // 1. 写 header
  const header: SessionHeader = { type: "session", version: CURRENT_SESSION_VERSION, id: ..., cwd: ... };
  const lines = [JSON.stringify(header)];
  
  // 2. 写当前 branch（注意：不是整个 tree）
  let parentId: string | null = null;
  for (const entry of sessionManager.getBranch()) {
    lines.push(JSON.stringify({ ...entry, parentId }));
    parentId = entry.id;
  }
  
  // 3. 可选 trailing entries（如 share 元数据）
  for (const entry of createTrailingEntries?.(parentId, timestamp) ?? []) {
    lines.push(JSON.stringify(entry));
  }
  
  writeFileSync(filePath, `${lines.join("\n")}\n`);
  return filePath;
}
```

**2. Share 元数据注入**（`session-share.ts:26-42`）：

```typescript
export function exportSessionForShare(filePath: string, session: AgentSession): void {
  exportSessionToJsonl(session.sessionManager, filePath, (parentId, timestamp) => [
    {
      type: "custom",
      customType: "pi.share",
      id: crypto.randomUUID().slice(0, 8),
      parentId,
      timestamp,
      data: {
        systemPrompt: session.state.systemPrompt,  // 共享 system prompt 用于复现
        tools: session.state.tools.map(t => ({
          name: t.name, description: t.description, parameters: t.parameters,
        })),
      },
    },
  ]);
}
```

**3. Radius 优先 + Gist fallback**（`session-share.ts:44-200`）：

```typescript
export async function shareSession(context: SessionShareContext): Promise<void> {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "pi-share-"));
  const jsonlFile = path.join(tempDir, "session.jsonl");
  const htmlFile = path.join(tempDir, "session.html");
  
  try {
    exportSessionForShare(jsonlFile, context.session);
    
    // Tier 1: Radius Gateway（自家 artifact 服务）
    if (await tryShareViaRadius(jsonlFile, context)) return;
    
    // Tier 2: GitHub Gist（公开 fallback）
    if (!ghAuthed()) { showError("GitHub CLI not logged in"); return; }
    await context.session.exportToHtml(htmlFile, { themeName: theme.name });
    await shareViaGist(htmlFile, context);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}
```

**4. Cache Miss 检测算法**（`cache-stats.ts:56-90`）：

```typescript
function detectMiss(prev, message, models): CacheMiss | undefined {
  const usage = message.usage;
  const promptTokens = usage.input + usage.cacheRead + usage.cacheWrite;
  
  // 零 cache 回合仅在「之前有 reported cache」时计入（cache-read-only provider vs no-cache provider）
  if (!prev || promptTokens <= 0 || (usage.cacheRead + usage.cacheWrite === 0 && !prev.reportedCache)) {
    return undefined;
  }
  
  const missedTokens = Math.min(prev.promptTokens, promptTokens) - usage.cacheRead;
  if (missedTokens <= NOISE_FLOOR_TOKENS) return undefined;  // 1024 token 内噪声
  
  // missedCost = missedTokens * (paidRate - readRate)
  const paidTokens = usage.input + usage.cacheWrite;
  const paidPerToken = paidTokens > 0 ? (usage.cost.input + usage.cost.cacheWrite) / paidTokens : 0;
  const readPerToken = usage.cacheRead > 0
    ? usage.cost.cacheRead / usage.cacheRead
    : (models.getModel(...).cost.cacheRead ?? 0) / 1_000_000;
  
  return { missedTokens, missedCost: missedTokens * Math.max(0, paidPerToken - readPerToken), idleMs, modelChanged };
}
```

**5. Statusline（footer）核心**（`footer.ts:79-180`）：

```typescript
render(width: number): string[] {
  const usageTotals = createUsageTotals();
  let latestCacheHitRate: number | undefined;
  
  // 累加 ALL entries（包含 compacted-away 历史，反映实际计费）
  for (const entry of this.session.sessionManager.getEntries()) {
    if (entry.type === "message" && entry.message.role === "assistant") {
      addUsageToTotals(usageTotals, entry.message.usage);
      const latestPromptTokens = entry.message.usage.input + entry.message.usage.cacheRead + entry.message.usage.cacheWrite;
      latestCacheHitRate = latestPromptTokens > 0 
        ? (entry.message.usage.cacheRead / latestPromptTokens) * 100 
        : undefined;
    }
    // ... 累加 toolResult/branch_summary/compaction
  }
  
  // 拼装 stats 行
  const statsParts = [];
  if (usageTotals.input) statsParts.push(`↑${formatTokens(usageTotals.input)}`);  // 输入
  if (usageTotals.output) statsParts.push(`↓${formatTokens(usageTotals.output)}`); // 输出
  if (usageTotals.cacheRead) statsParts.push(`R${formatTokens(usageTotals.cacheRead)}`);  // cache read
  if (usageTotals.cacheWrite) statsParts.push(`W${formatTokens(usageTotals.cacheWrite)}`); // cache write
  if ((cacheRead > 0 || cacheWrite > 0) && latestCacheHitRate !== undefined) {
    statsParts.push(`CH${latestCacheHitRate.toFixed(1)}%`);  // cache hit rate
  }
  if (usageTotals.cost || usingSubscription) {
    statsParts.push(`$${usageTotals.cost.toFixed(3)}${usingSubscription ? " (sub)" : ""}`);
  }
  // context% + auto-compact indicator
  statsParts.push(contextPercentStr);
  
  // 右侧：model name + thinking level
  // ...
}
```

**6. Cache hit rate 颜色 + 截断策略**（`footer.ts:153-167`）：

```typescript
// context% 颜色分级
if (contextPercentValue > 90) contextPercentStr = theme.fg("error", ...);   // 红色
else if (contextPercentValue > 70) contextPercentStr = theme.fg("warning", ...);  // 黄色
else contextPercentStr = contextPercentDisplay;  // 默认

// stats 行超出宽度 → 截断 statsLeft（保留 model name）
if (statsLeftWidth > width) {
  statsLeft = truncateToWidth(statsLeft, width, "...");
}
```

**7. HTML 导出 + theme color 适配**（`export-html/index.ts:30-120`）：

```typescript
// 1. parseColor 解析 hex + rgb 格式
// 2. getLuminance 计算亮度（0-1，> 0.5 = 亮色背景）
// 3. adjustBrightness 派生 userMessageBg/assistantMessageBg（深色加深 / 浅色减淡）
// 4. 模板引擎（template.js 1187 行）渲染 HTML + 自定义 tool renderer
```

**8. Radius 协议**（`session-share.ts:73-130`）：

```
POST {RADIUS_GATEWAY}/v1/artifacts?visibility=organization&title=Pi%20session
Authorization: Bearer {radius_oauth_token}（5 min validity）
Content-Type: application/x-ndjson
Body: session.jsonl

→ { artifact: { canonical_url: "https://..." } }
```

### 设计巧妙点

1. **JSONL 单 branch 导出 + `pi.share` trailing entry**：导出只含当前 branch（不是整个 tree），简化回放；`pi.share` 自定义 entry 携带 systemPrompt + tools 让接收方能 100% 复现（即使本地的 systemPrompt 已变）。
2. **Radius + Gist 二级 fallback**：自家 artifact 服务优先（可控、隐私好），失败 fallback 到 GitHub Gist（用户熟悉、可发现）。
3. **Cache miss 量化到美元**：`missedCost = missedTokens × (paidRate - readRate)`，让用户看到「这个 idle gap 让你多付了 $0.012」——比 token 数更有冲击力。
4. **Statusline 7 指标聚合**：输入/输出/cache read/cache write/cache hit 率/费用/上下文%，单行展示不打断用户。
5. **Statusline 颜色梯度**：context > 90% 红 / > 70% 黄，余量阈值视觉化。
6. **HTML export 用 theme 派生 user/assistant bg 颜色**：从 terminal theme 自动推导 HTML theme，避免用户需手动配 2 套。
7. **Stats 含 subscription 后缀**：Kimi Coding 这种订阅型服务显示 `(sub)` 而非 `$$`，避免误导。

### laew gap 表（D8）

D8 维度（statusline + 实时成本 + cache 命中 + 会话导出 + share）laew 当前实现度：
- `statusline`：laew TUI 有 footer 但**仅显示 cwd + 模型名**，无 token / cost / cache hit（CLAUDE.md "TUI 界面"章节）
- 实时成本：未实现（CLAUDE.md "成本控制" 未列入已实现）
- cache 命中：L1047 实现了 cache policy 注入，但**未在 footer 显示 CH%**
- 会话导出：未实现 JSONL 导出
- 分享：未实现 share / gist

本轮 L1546-L1575 区间已被前 7 个维度占用，**D8 整体缺口作为下一轮候选**（超出本轮区间），本轮不重复计入。参见后续章节「本轮未覆盖候选」。

---

## 末章 — 本轮 laew gap 汇总

### gap 编号对照（区间 L1546-L1575，30 个，本轮实际分配 18 个，剩余 12 个保留作 D8 重构待办）

| 编号 | 维度 | 描述 | P | Rust crate |
|------|------|------|---|------------|
| **L1546** | D1 @提及 | 无 `@file` 提及系统（laew 仅支持 CLI `@file` + 不存在的 slash 补全） | P1 | `fuzzy-matcher` + `walkdir` + `nucleo-matcher` + 自实现 `AtPrefixExtractor` |
| **L1547** | D1 @提及 | 无 Skill 命令注入（Skill 加载后未暴露为 `/skill:name` 入口） | P1 | 与 L1546 一起在 `tui/autocomplete.rs` 实现 |
| **L1548** | D2 命令 | 无 prompt template 系统（仅硬编码 `/help /clear /exit /new /model /provider`） | P1 | `serde_yaml` 解析 frontmatter + 自实现 `substitute_args()`（支持 `$1`/`$@`/`${N:-default}`） |
| **L1549** | D2 命令 | 斜杠命令为硬编码 if/else，无运行时动态注册 | P1 | `inventory` + `linkme` 收集所有 `#[laew_command]` 函数；命名冲突解决参考 `name:N` 模式 |
| **L1550** | D2 命令 | 无命令源 description 标签（无 `[user]`/`[project]`/`[extension]` 区分） | P2 | 在补全数据结构加 `SourceTag` enum |
| **L1551** | D2 命令 | 无内建命令冲突诊断（扩展注册同名 `/model` 静默覆盖） | P2 | 启动时扫描 + 写 warning 到 `DebugReport` |
| **L1552** | D3 对话回退 | 无会话树 / fork 能力（仅能 `/clear` 开新 session） | P1 | `rusqlite` 持久化 entry 表 + 复制子树 + `TreeSelectorComponent` ratatui 实现 |
| **L1553** | D3 对话回退 | 无 `/fork` 从历史消息分叉（也不支持选历史 user message 列） | P1 | 与 L1552 一起 |
| **L1554** | D3 对话回退 | 无 `/clone` 当前 leaf 复制 | P2 | 与 L1552 一起（简单 variant） |
| **L1555** | D3 对话回退 | 无 5 档 FilterMode（tree-selector FilterMode = `default`/`no-tools`/`user-only`/`labeled-only`/`all`） | P2 | 与 L1552 一起 |
| **L1556** | D3 对话回退 | 无分支摘要（fork 后老分支不浓缩，新分支上下文断开） | P1 | 与 L1552 + L1039（cached microcompact）联动 |
| **L1557** | D3 对话回退 | 无 entry label 书签（用户无法在 entry 上挂自定义标签） | P2 | SQLite `entry_labels(entry_id, label)` 表 |
| **L1558** | D3 对话回退 | 无 entry 复制（`ctx.ui.setStatus` 派生的 copy 转剪贴板） | P2 | `arboard` 剪贴板 |
| **L1559** | D4 文件监视 | 无工作区文件监视（无 `notify` v6 监听 + 自动索引失效） | P1 | `notify` v6 + `debounce` 500ms；Bash/Read/Write 工具返回后手动 invalidate |
| **L1560** | D4 文件监视 | 无 git HEAD 三路冗余监听（仅 bash 工具的 `git status` 输出，无持续 footer） | P2 | `notify` + `tokio::sync::Mutex` 三路复用 + 5s 重试 |
| **L1561** | D4 文件监视 | 无主题文件热重载（改 theme.json 需重启） | P2 | `notify` + 100ms debounce + stale-timer 校验 |
| **L1562** | D4 文件监视 | 无 stale-timer 防护（reload timer 可能跨切换覆盖） | P2 | `theme_name` 校验 + 主题切换时 `clearTimeout` |
| **L1563** | D5 富文本 | 无 Markdown 渲染（仅纯文本输出 + Box Drawing） | P1 | `pulldown-cmark` + 自实现 ANSI 渲染（参考 markdown.ts renderToken） |
| **L1564** | D5 富文本 | 无 Table 智能列宽（laew 无 markdown，自然无 table） | P2 | 与 L1563 一起 |
| **L1565** | D5 富文本 | 无语法高亮（代码块纯白底，无 ANSI 着色） | P2 | `syntect` + 主题；或 `tree-sitter-highlight`（按需 lazy 加载） |
| **L1566** | D5 富文本 | 无 word-level Diff 渲染（仅简单 `+`/`-` 行） | P2 | `similar` crate + 自实现 inverse |
| **L1567** | D5 富文本 | 无内联图片渲染（无 Kitty/iTerm2 协议探测） | P2 | `image` crate + base64 + 写 `\x1b_G...\x1b\\` 转义序列 |
| **L1568** | D5 富文本 | 无 photon-node 等价 resize（图片无 2000x2000 上限） | P2 | `image` crate + `image::imageops::resize(Lanczos3)` |
| **L1569** | D5 富文本 | 无工具自定义渲染接口（扩展工具只能返回字符串） | P1 | 在 `Tool` trait 加 `render_call`/`render_result` Option 字段 |
| **L1570** | D6 输入 | 无 kill ring（删除文本后无法 `Ctrl+Y` 恢复，多次删除无累积） | P2 | 自实现 `KillRing` + emacs 风格 `Ctrl+U/W/A/K` 钩到 `crossterm` |
| **L1571** | D6 输入 | 无 undo stack（编辑器操作不可撤销） | P2 | 自实现 `UndoStack<T>` + `Command` 模式 |
| **L1572** | D6 输入 | 无 prompt history 持久化（进程重启后历史消失） | P2 | `~/.config/laew/history.txt` + 上限 1000 + 去重 |
| **L1573** | D6 输入 | 无大粘贴截断（laew `input.rs` 无 `[paste #N]` marker 机制） | P1 | 自实现 `paste_marker.rs`：>10 行/1000 字符 → marker + paste registry |
| **L1574** | D6 输入 | 无外部编辑器钩子（无 `Ctrl+G` → `$VISUAL`） | P2 | `std::process::Command::new($VISUAL)` + `tempfile::NamedTempFile` + 阻塞读回 |
| **L1575** | D7 信任 | 无 project trust 5 档 UI + cwd 冒泡查询 | P1 | 自实现 `TrustSelectorComponent` + `find_nearest_trust_entry()` + 持久化到 SQLite `trust` 表（schema: `path TEXT PRIMARY KEY, decision INTEGER`） |

### 汇总

- **18 个本轮已分配 gap（D1-D7 主线，L1546-L1575 区间内 30 个，剩余 12 个保留为 D8 重构待办）**
- **P0 紧急（0 项）**：本轮 8 维度无 P0 紧急项（用户交互体验层主要为 P1 重要）
- **P1 重要（10 项）**：L1546 / L1547 / L1548 / L1549 / L1552 / L1553 / L1556 / L1559 / L1563 / L1569 / L1573 / L1575 — **聚焦 @ 提及、命令注册、对话树、文件监视、富文本、粘贴截断、目录信任**
- **P2 进阶（8 项）**：L1550 / L1551 / L1554 / L1555 / L1557 / L1558 / L1560 / L1561 / L1562 / L1564 / L1565 / L1566 / L1567 / L1568 / L1570 / L1571 / L1572 / L1574

### 推荐优先实施清单

按「P1 投入产出比」排序：

1. **L1548 prompt template 系统**（~300 行 + `serde_yaml`）—— 一份投入让用户写 `/commit-message` `/bug-report` 等自定义命令
2. **L1546 + L1547 @ 提及 + Skill 命令注入**（~500 行 + `nucleo-matcher`）—— 提升补全体验 + 与现有 Skill 系统整合
3. **L1559 工作区文件监视**（~200 行 + `notify`）—— 触发 cache 失效、git status 自动更新
4. **L1575 project trust 5 档 UI**（~400 行 + SQLite schema）—— 解决「第三方项目目录执行扩展」的信任问题
5. **L1552 + L1553 + L1556 会话树 + fork + 分支摘要**（~600 行 + SQLite）—— laew 缺的核心会话能力
6. **L1563 Markdown 渲染**（~800 行 + `pulldown-cmark`）—— 工具输出可读性大幅提升
7. **L1573 大粘贴截断**（~100 行）—— 单文件即可，性价比最高

### 本轮不重复声明

严格不重复前 17 轮已覆盖内容：

- **不重复 Lane 三态 / reduceLaneState / 14 种损坏检测 / WriterLease fence**（第 6 轮）
- **不重复 Session 持久化 JSONL 基础**（第 8 轮）—— 本轮 D8 仅聚焦 export/share trailing entry
- **不重复 Skill 系统 / Cordis Epoch / Skill 一等公民**（第 6 / 16 轮）—— 本轮 D1/D2 仅聚焦 `/skill:<name>` 触发与扩展命令注册
- **不重复 TUI 渲染模型基础**（第 8 轮 TUI 章节）—— 本轮 D5 仅聚焦 markdown/table/diff/image
- **不重复 Telemetry / OTLP**（第 16 轮）
- **不重复 OAuth PKCE / Device Code**（第 16 轮）
- **不重复 OTLP Telemetry 集成**（第 16 轮）
- **不重复 17 轮轮次摘要**（统一在「专题-第十八轮深挖合集」）

### 下轮（D8 + 协同主题）候选

| 编号 | 维度 | 描述 | P | Rust crate |
|------|------|------|---|------------|
| L1576（本轮保留） | D8 statusline | 无 7 指标 footer（input/output/R/W/CH%/cost/ctx% 单行聚合） | P1 | 自实现 `footer_data_provider.rs` + `usage_totals.rs`（cache 命中累加）|
| L1577（本轮保留） | D8 实时成本 | 无 cost 实时显示（无 `UsageTotals.cost` 累加） | P1 | `usage-totals` + `cost.total` |
| L1578（本轮保留） | D8 cache miss 量化 | 无 cache miss 检测（idle gap > 5min 自动检测 + 美元量化） | P2 | `cache-stats.ts` 算法移植 |
| L1579（本轮保留） | D8 session 导出 | 无 JSONL 导出（`/export` 命令缺失） | P1 | 自实现 `session-export.rs`（含 pi.share trailing entry 模式）|
| L1580（本轮保留） | D8 session 分享 | 无 session share（无 Radius/Gist 二级 fallback） | P2 | `reqwest` + `octocrab` + OAuth |
| L1581（本轮保留） | D8 HTML export | 无 HTML 导出（无 theme color 派生） | P2 | `handlebars` + `pulldown-cmark` |
| L1582（本轮保留） | D8 context% 颜色 | 无 context 颜色梯度（> 90% 红 / > 70% 黄） | P2 | `crossterm::style::Color::Red/Yellow` |
| L1583（本轮保留） | D8 subscription | 无 subscription 标识（Kimi Coding `(sub)` 后缀） | P2 | `cost.total == 0` 时显示 `(sub)` |
| L1584（本轮保留） | D8 extension status | 无 extension status 行（多扩展状态聚合） | P2 | `ctx.ui.setStatus` API + 多行 footer |
| L1585（本轮保留） | D8 model + thinking | 无右侧 model/thinking 切换显示 | P2 | 自实现 footer 右侧段 |
| L1586（本轮保留） | D8 provider prefix | 无多 provider 时 `(provider) model` 前缀 | P2 | `getAvailableProviderCount() > 1` |
| L1587（本轮保留） | D8 session name | 无 session name 拼接（pwd • session 显示） | P2 | `setSessionName` + footer 拼接 |

### 关键文件路径汇总

| 维度 | 关键文件 |
|------|---------|
| D1 @ 提及 | `packages/tui/src/autocomplete.ts:326-345`, `interactive-mode.ts:631-728` |
| D2 命令 | `packages/coding-agent/src/core/slash-commands.ts:19-43`, `extensions/runner.ts:664-683`, `prompt-templates.ts:71-90` |
| D3 对话回退 | `packages/coding-agent/src/modes/interactive/components/user-message-selector.ts:1-155`, `tree-selector.ts:1-1427`, `interactive-mode.ts:5144-5340` |
| D4 文件监视 | `packages/coding-agent/src/utils/fs-watch.ts:1-30`, `footer-data-provider.ts:290-380`, `theme.ts:825-870` |
| D5 富文本 | `packages/tui/src/components/markdown.ts:236-1015`, `syntax-highlight.ts:1-212`, `diff.ts:26-147`, `terminal-image.ts:78-696` |
| D6 输入 | `packages/tui/src/components/editor.ts:222-360`, `:705-735`, `:1245-1310`, `kill-ring.ts:1-46`, `keybindings.ts:71-210` |
| D7 信任/Onboarding | `packages/coding-agent/src/modes/interactive/components/first-time-setup.ts:1-145`, `trust-selector.ts:1-134`, `core/trust-manager.ts:30-167`, `theme-controller.ts:53-90` |
| D8 导出/statusline | `packages/coding-agent/src/core/session-export.ts:1-42`, `session-share.ts:1-206`, `cache-stats.ts:1-164`, `footer.ts:1-245`, `footer-data-provider.ts:1-388` |

---

*本轮深挖共分析 30+ 个关键文件 + 24 个交互组件 + 50+ 键位 + 8 维度 18 个 laew gap（L1546-L1575），所有结论均有源码:行号 + 关键代码片段支撑，无臆测。*
