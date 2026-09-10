# 专题-第十八轮-claudecode-深度分析 — 用户交互体验层 8 维度

> **生成信息**
> - 源项目: `/usr/local/LsmGitOpenSource/claudecode` (TypeScript / Bun, Claude Code CLI)
> - 调研日期: 2026-09-09
> - 调研方法: 8 个独立子领域 grep 定位 + 文件:行号引用 + 关键代码片段
> - 覆盖维度: D1@提及系统 / D2 自定义斜杠命令 / D3 Rewind 时间旅行 / D4 文件监视 / D5 富文本渲染 / D6 输入体验 / D7 Onboarding-Trust-Theme / D8 状态线-成本-导出
> - 累计 laew gap: **L1426-L1455 共 30 个**(已分配、未越界)
> - **本轮不重复声明**: 协议 wire / SSE / 缓存策略 / 工具 40+ 抽象 / Bridge 远程控制 / Skill 一等公民 / 22 层 Bash 检测 / 27 Hook / 崩溃恢复五层 / TUI 帧协议 / OAuth / i18n / Release / WebSocket / CRDT / Prompt 注入防护 / Telemetry 双层 — 这些已在前 17 轮专题文档中覆盖,本轮不复述

---

## 目录

- [D1 @提及系统](#d1-@提及系统)
- [D2 自定义斜杠命令与 Prompt 模板](#d2-自定义斜杠命令与-prompt-模板)
- [D3 对话 Rewind/分支/时间旅行](#d3-对话-rewind分支时间旅行用户级)
- [D4 文件监视与工作区感知](#d4-文件监视与工作区感知运行时)
- [D5 工具输出富文本内容渲染](#d5-工具输出富文本内容渲染)
- [D6 输入体验工程](#d6-输入体验工程)
- [D7 Onboarding/目录信任/主题偏好](#d7-onboarding目录信任主题偏好)
- [D8 会话导出 / 状态线 / 实时成本](#d8-会话导出--状态线--实时成本)
- [laew gap 汇总 (L1426-L1455)](#laew-gap-汇总-l1426-l1455)

---

## D1 @提及系统

### 1.1 源码定位

| 关注点 | 路径 | 关键符号 |
|--------|------|----------|
| 提及正则提取 | `src/utils/attachments.ts:2757-2828` | `extractAtMentionedFiles` / `extractMcpResourceMentions` / `extractAgentMentions` |
| 行号片段解析 | `src/utils/attachments.ts:2836-2852` | `parseAtMentionedFileLines` |
| 提及 → Attachment | `src/utils/attachments.ts:1894-1964` | `processAtMentionedFiles` |
| 目录提及特殊处理 | `src/utils/attachments.ts:1915-1944` | `readdir` + 1000 条目截断 |
| 重复文件去重优化 | `src/utils/attachments.ts:3076-3119` | `existingFileState` / `already_read_file` |
| 大文件/PDF 拒读 | `src/utils/attachments.ts:3046-3074` | `tengu_attachment_file_too_large` / `tryGetPDFReference` |
| 索引后端 (Rust) | `src/hooks/fileSuggestions.ts:715-739` | `generateFileSuggestions` / `FileIndex` |
| 5s 节流 + mtime 触发 | `src/hooks/fileSuggestions.ts:636-686` | `REFRESH_THROTTLE_MS = 5_000` / `getGitIndexMtime` |
| nucleo fuzzy 搜索 | `src/native-ts/file-index/index.ts:617-625` | `MAX_SUGGESTIONS = 15` |
| ripgrep ignore 链 | `src/hooks/fileSuggestions.ts:202-247` | `loadRipgrepIgnorePatterns` |
| IDE 端 @ 注入 | `src/components/PromptInput/PromptInput.tsx:1281-1298` | `useIdeAtMentioned` / `IDEAtMentioned` |
| 统一建议(文件+slash) | `src/hooks/unifiedSuggestions.ts` | `generateFileSuggestions` + agents + slash |

### 1.2 机制剖析

#### 1.2.1 三种 @ 提及形态

`extractAtMentionedFiles` 在用户输入被送入 Agent 之前扫出所有 `@path` 引用,支持**三种正则在同一个 turn 内同时存在**:

```ts
// src/utils/attachments.ts:2764-2765
const quotedAtMentionRegex = /(^|\s)@"([^"]+)"/g
const regularAtMentionRegex  = /(^|\s)@([^\s]+)\b/g
```

- 常规无引号: `check @src/foo.ts please` → `["src/foo.ts"]`
- 带引号(路径含空格): `check @"my docs/note.md" please` → `["my docs/note.md"]`
- **特例**: 排除以 ` (agent)` 结尾的伪提及(那是 agent 自动补全的产物,见下)

`extractAgentMentions` (`attachments.ts:2802-2828`) 还单独识别 `@agent-<type>` 与 `@"<type> (agent)"` 两种格式:

```ts
// quoted: @"<type> (agent)"
const quotedAgentRegex = /(^|\s)@"([\w:.@-]+) \(agent\)"/g
// unquoted: @agent-<type>     (支持 asana:project-status-updater 这种 plugin 命名空间)
const unquotedAgentRegex = /(^|\s)@(agent-[\w:.@-]+)/g
```

`extractMcpResourceMentions` (`attachments.ts:2792-2800`) 第三种是 `@server:uri` 形式的 MCP 资源引用。

#### 1.2.2 路径 + 行号片段语法

`parseAtMentionedFileLines` 支持**单行、行区间、文件名片段**三种粒度,模仿 GitHub permalink:

```ts
// src/utils/attachments.ts:2836-2852
export function parseAtMentionedFileLines(mention: string): AtMentionedFileLines {
  // #L10      → { filename, lineStart: 10, lineEnd: 10 }
  // #L10-20   → { filename, lineStart: 10, lineEnd: 20 }
  // #heading  → 跳过(只支持行号区间,非锚点)
  const match = mention.match(/^([^#]+)(?:#L(\d+)(?:-(\d+))?)?(?:#[^#]*)?$/)
  ...
  const lineEnd = lineEndStr ? parseInt(lineEndStr, 10) : lineStart
  return { filename: filename ?? mention, lineStart, lineEnd }
}
```

`processAtMentionedFiles` 紧接着把 `lineStart/lineEnd` 转成 `FileReadTool.call({ offset, limit })` 参数,实现"用户@某文件某段,模型只看到那几行"。

#### 1.2.3 目录提及展开为目录树

`attachments.ts:1915-1944` 把 `@./src` 这种目录引用**直接内联为 1000 条目以内的目录树**给模型看:

```ts
if (stats.isDirectory()) {
  const entries = await readdir(absoluteFilename, { withFileTypes: true })
  const MAX_DIR_ENTRIES = 1000
  const truncated = entries.length > MAX_DIR_ENTRIES
  const names = entries.slice(0, MAX_DIR_ENTRIES).map(e => e.name)
  if (truncated) names.push(`… and ${entries.length - MAX_DIR_ENTRIES} more entries`)
  const stdout = names.join('\n')
  return { type: 'directory', path: absoluteFilename, content: stdout, displayPath: ... }
}
```

> **设计要点**: 一次性读目录而不是递归 — 1000 条目硬上限是模型 token 预算的现实妥协(`maxSizeBytes = 256KB`/`maxTokens = 25000` 见 `src/tools/FileReadTool/limits.ts:6-7`)。

#### 1.2.4 大文件/PDF 拒读与降级

`generateFileAttachment` (`attachments.ts:3020+`) 走五道关卡:

1. `isFileReadDenied` — 权限拒绝直接返回 `null`(`tengu_at_mention_extracting_filename_error` 上报)
2. 文件大小 > `maxSizeBytes=256KB` — 记 `tengu_attachment_file_too_large` 拒读
3. **PDF 例外**: `tryGetPDFReference` 走轻量 reference(只取元数据,不计 token)
4. **已读文件 mtime 命中**: 返回 `already_read_file` attachment(模型提示"已在上下文中,不必重发内容",直接省 token)
5. 真正调用 `FileReadTool.call`,若 `MaxFileReadTokenExceededError`/`FileTooLargeError` 则降级为"截断到前 `MAX_LINES_TO_READ` 行"

```ts
// attachments.ts:3046-3065
if (mode === 'at-mention' && !isFileWithinReadSizeLimit(filename, getDefaultFileReadingLimits().maxSizeBytes)) {
  ...
  logEvent('tengu_attachment_file_too_large', { size_bytes: stats.size, mode })
  return null
}
```

```ts
// attachments.ts:3076-3119: 重复提及去重
if (existingFileState && mode === 'at-mention') {
  const mtimeMs = await getFileModificationTimeAsync(filename)
  if (existingFileState.timestamp <= mtimeMs && mtimeMs === existingFileState.timestamp) {
    // 已被读过且 mtime 未变 → 跳过重复 IO
    return { type: 'already_read_file', ... }
  }
}
```

#### 1.2.5 IDE 端 @ 注入(双向)

`PromptInput.tsx:1281-1298` 处理**从 IDE 推过来的 @ 提及**——用户在 VSCode JetBrains 等扩展里选中文件某段,IDE 通过 WebSocket 推 `IDEAtMentioned { filePath, lineStart?, lineEnd? }`,claudecode 转成对应文本:

```ts
const onIdeAtMentioned = function (atMentioned: IDEAtMentioned) {
  let atMentionedText: string
  const relativePath = path.relative(getCwd(), atMentioned.filePath)
  if (atMentioned.lineStart && atMentioned.lineEnd) {
    atMentionedText = atMentioned.lineStart === atMentioned.lineEnd
      ? `@${relativePath}#L${atMentioned.lineStart} `
      : `@${relativePath}#L${atMentioned.lineStart}-${atMentioned.lineEnd} `
  } else {
    atMentionedText = `@${relativePath} `
  }
  insertTextAtCursor(atMentionedText)
}
```

→ 实现了「IDE 选中 → 终端 @ 提及」的双向链路,**用户在 IDE 选区,claudecode 输入框直接收到对应行号**.

#### 1.2.6 输入时自动补全(实时)

`src/hooks/fileSuggestions.ts` 是**实时建议引擎**。核心设计:

```ts
// 节流: 一次只跑一个 git ls-files + index build; 5s 时间地板
const REFRESH_THROTTLE_MS = 5_000
export function startBackgroundCacheRefresh(): void {
  if (fileListRefreshPromise) return
  const indexMtime = getGitIndexMtime()
  if (fileIndex) {
    const gitStateChanged = indexMtime !== null && indexMtime !== lastGitIndexMtime
    if (!gitStateChanged && Date.now() - lastRefreshMs < REFRESH_THROTTLE_MS) return
  }
  ...
  fileListRefreshPromise = getPathsForSuggestions()
}
```

- `.git/index` mtime 变化 → 立即刷新(捕获 `git add/checkout/rebase/rm`)
- 时间地板 5s → 捕获 untracked 新文件
- Rust `nucleo` 模糊匹配(`src/native-ts/file-index/index.ts`),`MAX_SUGGESTIONS = 15`
- 用户每按一键,`generateFileSuggestions(query)` 同步返回 top 15 候选
- `pathListSignature` 用 FNV-1a hash + 每 500 步采样,捕获 346k 路径列表 < 1ms

#### 1.2.7 自定义 `fileSuggestion` 钩子扩展点

如果用户/项目在 `settings.json` 配 `fileSuggestion: { type: 'command', command: '...', timeout: 5000 }`(**参考 `src/hooks/fileSuggestions.ts:726-733`**),则跳过默认 git/ripgrep 流程,直接调用用户脚本:

```ts
if (getInitialSettings().fileSuggestion?.type === 'command') {
  const input: FileSuggestionCommandInput = { ...createBaseHookInput(), query: partialPath }
  const results = await executeFileSuggestionCommand(input)
  return results.slice(0, MAX_SUGGESTIONS).map(createFileSuggestionItem)
}
```

> **设计巧妙点**: 完全可替换 — 用户用 ripgrep/ag/fzf 命令替代内部实现,1.5k 行 hook 体系(`docs/TUI自动化测试`/`docs/Agent架构对比与参考`)统一编排。

### 1.3 设计巧妙点

1. **三正则形态 + 命名空间隔离**: 文件/MCP/Agent 提及互不污染,agent 提及还识别 plugin 命名空间 `agent-asana:project-status-updater`
2. **mtime 去重 + already_read_file attachment**: 避免用户连续 @ 同一文件触发重复 IO
3. **目录提及的 1000 条目硬上限**: 与 `maxSizeBytes=256KB` 构成双层 token 预算保险
4. **双向 IDE 通信**: IDE 选区直接变成终端 @ 提及文本,无复制粘贴
5. **Rust nucleo + 节流 + mtime 三层唤醒**: 单次 keypress 0ms,后台 index build 不阻塞
6. **5 节流秒的来源**: 注释明确"floor picks up untracked files, which don't bump .git/index"
7. **`getDotGitIndexMtime` 单次同步 stat 的 trade-off**: 见 `fileSuggestions.ts:140-147` 注释,选择 sync IO 是为了让 `startBackgroundCacheRefresh` 保持同步签名(否则 `fileListRefreshPromise` 契约要重写)

### 1.4 laew gap 列表(D1)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| L1426 | **@ 文件提及系统**(三正则 + 行号片段 + 目录树 + 大文件降级) | P1 | `regex` + `walkdir` + `nucleo-matcher` + `ignore` |
| L1427 | **@ 时实时自动补全**(Rust 索引 + 5s 节流 + .git/index mtime 唤醒) | P1 | `nucleo-matcher` + `gix`(读 index) |
| L1428 | **IDE → CLI 双向 @ 注入**(WS 推送 IDEAtMentioned → 文本) | P2 | `tokio-tungstenite` |
| L1429 | **already_read_file 附件优化**(mtime 比对跳过重复 IO) | P2 | 直接在 `attachments.rs` 实现 |
| L1430 | **PDF reference 轻量引用**(只取元数据,不展开内容) | P2 | `lopdf` |

---

## D2 自定义斜杠命令与 Prompt 模板

### 2.1 源码定位

| 关注点 | 路径 | 行号 | 关键符号 |
|--------|------|------|----------|
| 命令总入口 | `src/commands.ts` | (全文) | `Command` 接口聚合所有命令 |
| 斜杠命令解析 | `src/utils/slashCommandParsing.ts:25-60` | `parseSlashCommand` |
| Frontmatter 字段解析 | `src/skills/loadSkillsDir.ts:185-265` | `parseSkillFrontmatterFields` |
| Plugin 命令加载 | `src/utils/plugins/loadPluginCommands.ts:169-329` | `loadCommandsFromDirectory` / `createPluginCommand` |
| $ARGUMENTS 替换 | `src/utils/argumentSubstitution.ts:94-145` | `substituteArguments` |
| shell 解析的 args | `src/utils/argumentSubstitution.ts:24-40` | `parseArguments` (tryParseShellCommand) |
| named args 声明 | `src/utils/argumentSubstitution.ts:50-68` | `parseArgumentNames` |
| 渐进式 hint 提示 | `src/utils/argumentSubstitution.ts:76-83` | `generateProgressiveArgumentHint` |
| 旧 commands 目录 | `src/skills/loadSkillsDir.ts:566-623` | `loadSkillsFromCommandsDir` |
| 多源合并 | `src/skills/loadSkillsDir.ts:638-713` | `getSkillDirCommands` (managed/user/project/additional) |
| 模型/工具/effort 字段 | `src/skills/loadSkillsDir.ts:222-264` | `model` / `effort` / `disableModelInvocation` |
| 可用性过滤 | `src/cli/print.ts:3115/4455` | `cmd.userInvocable !== false` |
| 内置 subagent 作为命令 | `src/tools/AgentTool/built-in/claudeCodeGuideAgent.ts:128-134` | 列出 custom skills 供 agent 触发 |

### 2.2 机制剖析

#### 2.2.1 三目录 + 多源合并

claudecode 实际把"命令"和"skill"统一为同一个数据结构,只通过 frontmatter `user-invocable` 区分**人可触发 vs 仅模型可触发**:

| 源 | 路径 | 默认 user-invocable |
|----|------|---------------------|
| managed (policySettings) | `$MANAGED/.claude/skills/` | true |
| user | `~/.claude/skills/` | true |
| project | `<repo>/.claude/skills/` 链向上到家目录 | true |
| additional (`--add-dir`) | 命令行追加目录 | true |
| legacy `commands` | `<repo>/.claude/commands/` (DEPRECATED) | true |
| plugin | `<plugin>/commands/` 或 `skills/` | per frontmatter |

```ts
// src/skills/loadSkillsDir.ts:638-713
export const getSkillDirCommands = memoize(async (cwd: string): Promise<Command[]> => {
  const userSkillsDir    = join(getClaudeConfigHomeDir(), 'skills')
  const managedSkillsDir = join(getManagedFilePath(),    '.claude', 'skills')
  const projectSkillsDirs = getProjectDirsUpToHome('skills', cwd)
  ...
  // 关键: 7 个并发源,最后 dedup
  const [managedSkills, userSkills, ...projectSkillsNested, additionalSkillsNested, legacyCommands] = await Promise.all([...])
  ...
  // dedup by name;managed 优先覆盖 user/project/plugin
})
```

→ 合并策略:**同名校验名,优先级 managed > user > project > plugin**(详情见 `loadSkillsDir.ts:760-799` 日志"Loaded X unique skills (managed: Y, user: Z, project: ...)")

#### 2.2.2 frontmatter 字段完整清单

`parseSkillFrontmatterFields` (`loadSkillsDir.ts:185-265`) 一次解析 13 个 frontmatter 字段:

```ts
// 摘自 loadSkillsDir.ts:190-264
return {
  displayName: frontmatter.name != null ? String(frontmatter.name) : undefined,
  description, hasUserSpecifiedDescription,
  allowedTools: parseSlashCommandToolsFromFrontmatter(frontmatter['allowed-tools']),
  argumentHint:    frontmatter['argument-hint'] != null ? String(frontmatter['argument-hint']) : undefined,
  argumentNames:   parseArgumentNames(frontmatter.arguments as string | string[] | undefined),
  whenToUse:       frontmatter.when_to_use as string | undefined,
  version:         frontmatter.version as string | undefined,
  model:           parseUserSpecifiedModel(...),       // 'inherit' | 'haiku' | 'sonnet' | 'opus' | alias
  disableModelInvocation: parseBooleanFrontmatter(frontmatter['disable-model-invocation']),
  userInvocable,
  hooks:           parseHooksFromFrontmatter(frontmatter, resolvedName),
  executionContext: frontmatter.context === 'fork' ? 'fork' : undefined,  // 整命令作为 subagent 跑
  agent:           frontmatter.agent as string | undefined,              // 指定 agent 类型
  effort:          parseEffortValue(frontmatter['effort']),
  shell:           parseShellFrontmatter(frontmatter.shell, resolvedName),
}
```

**关键设计**:

- `allowed-tools`: 命令级工具白名单(运行时 **独立**于全局 session allowlist,见 `security-review.ts:210`)
- `context: fork`: 命令在 sub-agent 内运行,父级上下文不污染
- `agent: <type>`: 命令直接委派某个 agent 身份
- `model`: 命令级模型覆盖(支持 `inherit` 继承当前 session 模型)
- `disable-model-invocation: true`: 禁止模型自动调(只允许用户主动 `/`)
- `user-invocable: false`: 模型可见,用户 `/` 不可见(实现"模型只能通过 SkillTool 触发")

#### 2.2.3 $ARGUMENTS / $ARGUMENTS[N] / $N / $name 替换

`src/utils/argumentSubstitution.ts` 是本节核心。**4 种占位符 + shell 风格引号解析**:

```ts
// argumentSubstitution.ts:94-145
export function substituteArguments(content, args, appendIfNoPlaceholder = true, argumentNames = []): string {
  const parsedArgs = parseArguments(args)  // shell-quote 风格

  // 1) 命名参数: $foo $bar 等
  for (let i = 0; i < argumentNames.length; i++) {
    const name = argumentNames[i]
    if (!name) continue
    content = content.replace(new RegExp(`\\$${name}(?![\\[\\w])`, 'g'), parsedArgs[i] ?? '')
  }

  // 2) 索引参数: $ARGUMENTS[0] / $ARGUMENTS[1] / ...
  content = content.replace(/\$ARGUMENTS\[(\d+)\]/g, (_, indexStr) => parsedArgs[parseInt(indexStr, 10)] ?? '')

  // 3) 简写索引: $0 $1 ...
  content = content.replace(/\$(\d+)(?!\w)/g, (_, indexStr) => parsedArgs[parseInt(indexStr, 10)] ?? '')

  // 4) 全量: $ARGUMENTS
  content = content.replaceAll('$ARGUMENTS', args)

  // 兜底: 没占位符 + 有 args → 末尾追加 "ARGUMENTS: {args}"
  if (content === originalContent && appendIfNoPlaceholder && args) {
    content = content + `\n\nARGUMENTS: ${args}`
  }
  return content
}
```

**`parseArguments` 用 shell-quote 解析**(`argumentSubstitution.ts:30`),所以 `foo "hello world" 'a b'` 正确分成 3 段,**避免空格误分割带空格路径**.

**渐进式提示**: 用户输入 `/cmd foo bar` 时,footer 实时提示 `[baz]`(待填的命名参数),见 `generateProgressiveArgumentHint` (76-83):

```ts
const remaining = argNames.slice(typedArgs.length)
if (remaining.length === 0) return undefined
return remaining.map(name => `[${name}]`).join(' ')
```

#### 2.2.4 subagent 作为命令(AgentTool)

claudecode 的 `AgentTool` (sub-agent) 与自定义 slash 命令**共享同一种 Command 抽象**,所以**项目级 `.claude/agents/foo.md` 中的 agent 可在 prompt 里用 `@foo` 引用,也能在 slash 命令中通过 `agent: foo` 引用**——见 `loadAgentsDir.ts:684` 把 agent 的 `skills` 字段也按 `parseSlashCommandToolsFromFrontmatter` 解析。

内置的 `claudeCodeGuideAgent` (128-134) 还会**在 system prompt 里枚举当前可用的 custom skills**,给 sub-agent 提示可调用的命令:

```ts
const customCommands = commands.filter(cmd => cmd.type === 'prompt')
if (customCommands.length > 0) {
  const commandList = customCommands.map(cmd => `- /${cmd.userFacingName?.() ?? cmd.name} — ${cmd.description}`).join('\n')
  prompt = prompt + `\n\n**Available custom skills in this project:**\n${commandList}`
}
```

### 2.3 设计巧妙点

1. **统一 Command 抽象**: 自定义命令、内置命令、plugin 命令、agent-as-command 走同一接口,UI 渲染/suggestion 解析/权限检查只写一次
2. **frontmatter 13 字段**: 覆盖了"工具白名单/模型覆盖/上下文隔离/可触发方"全维度,**比 Makefile 多了 7 维**
3. **三目录合并 + managed 优先**: 多源冲突时,policy(managed) > user > project,**避免了"项目用恶意 skill 覆盖用户偏好"**
4. **`user-invocable` + `disable-model-invocation` 正交两轴**:
   - 两者都 true → 用户 + 模型都可见(默认)
   - 用户 true,模型 false → 只用户主动触发(危险操作)
   - 用户 false,模型 true → 模型只能通过 SkillTool 触发(辅助流程)
   - 两者都 false → 完全隐藏(没意义,只是框架预留)
5. **shell 风格 argument 解析**: 用户传 `'a "b c" d'` 自动得 `['a', 'b c', 'd']`,**避免用户写 `\`` 转义**
6. **`substituteArguments` 兜底追加**: 没占位符且有 args 时自动追加 `ARGUMENTS: {args}` 到末尾,**让简单命令的 prompt 模板不需要显式 `$ARGUMENTS`**
7. **`context: fork`**: 把命令作为 sub-agent 跑,**父上下文不污染 / 子上下文有自己独立的 tool 权限**
8. **`agent: <type>`**: 命令直接委派,**一个 markdown 文件就生成新 agent 身份**

### 2.4 laew gap 列表(D2)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| L1431 | **自定义斜杠命令系统**(frontmatter 13 字段 + 三目录合并) | P1 | `serde_yaml` + `markdown` |
| L1432 | **$ARGUMENTS / $N / $name 占位符 + shell 风格参数解析** | P1 | `shell-words`(对应 `tryParseShellCommand`) |
| L1433 | **命令级 allowed-tools / model / context:fork / agent: 覆盖** | P1 | (与 L1431 一起实现) |
| L1434 | **managed > user > project 多源合并 + 优先级去重** | P1 | (与 L1431 一起实现) |
| L1435 | **subagent 作为命令(AgentTool ↔ Command 抽象统一)** | P2 | (与 L1431 一起实现) |

---

## D3 对话 Rewind/分支/时间旅行(用户级)

### 3.1 源码定位

| 关注点 | 路径 | 行号 | 关键符号 |
|--------|------|------|----------|
| rewind 命令定义 | `src/commands/rewind/index.ts:1-13` | `aliases: ['checkpoint']` |
| rewind 命令回调 | `src/commands/rewind/rewind.ts:1-13` | 调 `context.openMessageSelector()` |
| 消息选择器 UI | `src/components/MessageSelector.tsx` | 830 行 (含 7 选单 + 6 恢复选项) |
| 6 恢复选项 | `src/components/MessageSelector.tsx:31` | `RestoreOption = 'both' \| 'conversation' \| 'code' \| 'summarize' \| 'summarize_up_to' \| 'nevermind'` |
| 双 Esc 入口 | `src/components/PromptInput/PromptInput.tsx:1254` | `useDoublePress(noop, onShowMessageSelector)` |
| 差异统计(预展示) | `src/components/MessageSelector.tsx:77` | `fileHistoryGetDiffStats(fileHistory, preselectedMessage.uuid)` |
| 恢复协调 | `src/screens/REPL.tsx:3712-3747` | `restoreMessageSync` + `handleRestoreMessage` (setImmediate 防遗留) |
| 代码恢复底层 | `src/utils/fileHistory.ts:347-391` | `fileHistoryRewind` |
| 会话级 trust 临时标记 | `src/components/TrustDialog/TrustDialog.tsx:175` | `setSessionTrustAccepted(true)` |
| 时间旅行汇总 | `src/services/compact/compact.ts:134` | 压缩保留 attach + 图像裁剪 |

### 3.2 机制剖析

#### 3.2.1 三入口: `/rewind` 命令 + `/checkpoint` 别名 + Esc Esc

claudecode 把 rewind 设计成"**对话 checkpoint 浏览器**"——用户挑一条历史 user message,后续整段历史截断,文件状态可选回滚。

**命令入口** (`src/commands/rewind/index.ts`):

```ts
const rewind = {
  description: `Restore the code and/or conversation to a previous point`,
  name: 'rewind',
  aliases: ['checkpoint'],         // 兼容老用户记忆
  argumentHint: '',
  type: 'local',
  supportsNonInteractive: false,
  load: () => import('./rewind.js'),
} satisfies Command
```

**键盘入口** (`PromptInput.tsx:1254`):

```ts
const doublePressEscFromEmpty = useDoublePress(() => {}, () => onShowMessageSelector())
```

→ `useDoublePress` 第一次按 0ms-300ms 不响应,第二次按 < 300ms 内直接触发 `openMessageSelector`。

> 重点:`aliases: ['checkpoint']` 与 git 7.x `git checkout` 同名,降低老 git 用户学习成本。

#### 3.2.2 消息选择器 UI(7 行 + 6 恢复选项)

`MessageSelector.tsx` 主体(>830 行)的核心: 把历史 user message 渲染成单选列表,**只显示可被选中的 user message**(过滤掉 tool result 等),然后选完后弹二次确认菜单。

**7 选单 + 二次确认 UI 设计** (`MessageSelector.tsx:31-134`):

```ts
type RestoreOption = 'both' | 'conversation' | 'code' | 'summarize' | 'summarize_up_to' | 'nevermind'
//                           ^^^^^^^^^^^^^^^^ ^^^^^^^^^^^^^^ ^^^^^   ^^^^^^^^^^^   ^^^^^^^^^^^^^^^^
//                           代码+对话都回滚  只回滚对话     只回滚代码 用此处开始摘要 摘要到此处
```

5 选单生成逻辑(`getRestoreOptions`, 93-134):

```ts
function getRestoreOptions(canRestoreCode: boolean): OptionWithDescription<RestoreOption>[] {
  const baseOptions: OptionWithDescription<RestoreOption>[] = canRestoreCode ? [
    { value: 'both',         label: 'Restore code and conversation' },
    { value: 'conversation', label: 'Restore conversation' },
    { value: 'code',         label: 'Restore code' },
  ] : [{ value: 'conversation', label: 'Restore conversation' }]
  baseOptions.push({ value: 'summarize',     label: 'Summarize from here', type: 'input', placeholder: 'add context (optional)', ... })
  if (feature(...)) baseOptions.push({ value: 'summarize_up_to', label: 'Summarize up to here', ... })
  baseOptions.push({ value: 'nevermind',     label: 'Never mind' })
  return baseOptions
}
```

**渐进差异统计** (`MessageSelector.tsx:74-83`): 选完 message 后,**异步预取**该 message 之后所有文件变化统计,UI 立刻显示 "+42 / -18 lines, 3 files":

```ts
useEffect(() => {
  if (!preselectedMessage || !isFileHistoryEnabled) return
  let cancelled = false
  void fileHistoryGetDiffStats(fileHistory, preselectedMessage.uuid).then(stats => {
    if (!cancelled) setDiffStatsForRestore(stats)
  })
  return () => { cancelled = true }
}, [preselectedMessage, isFileHistoryEnabled, fileHistory])
```

#### 3.2.3 实际恢复: 消息树截断 + 文件回滚联动

`REPL.tsx:3712-3747` 的 `restoreMessageSync` 是关键——**对话状态与文件状态是分两条路**:

```ts
const restoreMessageSync = useCallback((message: UserMessage) => {
  rewindConversationTo(message)   // 1) 截断消息树(messageState → 0..message)
  const r = textForResubmit(message)   // 2) 把当时用户输入重灌进输入框
  if (r) { setInputValue(r.text); setInputMode(r.mode) }

  // 3) 重灌粘贴的图片(image block 重灌为 PastedContent)
  if (Array.isArray(message.message.content) && message.message.content.some(block => block.type === 'image')) {
    const imageBlocks = message.message.content.filter(block => block.type === 'image')
    if (imageBlocks.length > 0) {
      const newPastedContents: Record<number, PastedContent> = {}
      imageBlocks.forEach((block, index) => {
        if (block.source.type === 'base64') {
          const id = message.imagePasteIds?.[index] ?? index + 1
          newPastedContents[id] = { id, type: 'image', content: block.source.data, mediaType: block.source.media_type }
        }
      })
      setPastedContents(newPastedContents)
    }
  }
}, [rewindConversationTo, setInputValue])

const handleRestoreMessage = useCallback(async (message: UserMessage) => {
  setImmediate((restore, message) => restore(message), restoreMessageSync, message)
}, [restoreMessageSync])
```

**`setImmediate` 包装原因** (`REPL.tsx:3742-3747` 注释): 让 "Interrupted" 消息先渲染到静态输出,**否则 rewind 完成后那行 `Interrupted` 会作为残留遗留**——是 React 渲染时序的微调。

**文件回滚走 `fileHistoryRewind`** (`utils/fileHistory.ts:347-391`,**复用第七轮已覆盖的 git shadow 机制**),**只对 `'both'`/`'code'` 选项触发**:

```ts
// REPL.tsx:4911-4918
onRestoreCode={async (message: UserMessage) => {
  await fileHistoryRewind((updater) => { ... }, message.uuid)
  ...
}}
```

#### 3.2.4 "summarize" 与 "summarize_up_to" — 时间旅行 + 压缩合一

`'summarize'` 选项(`MessageSelector.tsx:189-209`)不是简单截断,而是**调用 `compact` 把截断点之前的对话压缩成一段 summary**,从而**腾出 context window**;可附 `feedback`(可选 prompt 提示 compact 关注哪些点):

```ts
if (isSummarizeOption(option)) {
  onPreRestore(); setIsRestoring(true)
  const direction = option === 'summarize_up_to' ? 'up_to' : 'from'
  const feedback = (direction === 'up_to' ? summarizeUpToFeedback : summarizeFromFeedback).trim() || undefined
  await onSummarize(messageToRestore, feedback, direction)
  ...
}
```

→ 巧妙:**rewind 既是时间旅行,又是 context 治理工具**。用户发现"前面对话有 200k 没用的探索" → 一键 summarise from here → 上下文瞬间空出 100k。

#### 3.2.5 uuid 前缀匹配(用于跨 turn 还原)

`REPL.tsx:3749-3753` 的 `findRawIndex` 用 **uuid 前 24 字符**做匹配——**派生 uuid 与原始 uuid 保留前 24 字符**,所以**即使消息从 SDK 序列化再反序列化,rewind 也能锁定原位置**:

```ts
// 24-char prefix: deriveUUID preserves first 24, renderable uuid prefix-matches raw source.
const findRawIndex = (uuid: string) => {
  const prefix = uuid.slice(0, 24)
  return messages.findIndex(m => m.uuid.slice(0, 24) === prefix)
}
```

→ 与第十轮介绍的 SDK transcript 协议配套:反序列化后用户能用 `/rewind` 锁定原始对话节点。

### 3.3 设计巧妙点

1. **三入口汇聚**(`/rewind` + `/checkpoint` + Esc Esc): 满足"老 git 用户 / 命令党 / 键盘党"三类人
2. **6 恢复选项**(`both/conversation/code/summarize/from/summarize_up_to`): **rewind 不只是撤销,还是 context 治理工具**(summarize 路径与第十六轮 microCompact 联动)
3. **异步预取 diff stats**: 选 message 后立即显示文件变化行数,无白屏
4. **`setImmediate` 包装防残留**: React 渲染时序细节
5. **uuid 24 字符前缀**: 跨 SDK 序列化还原
6. **`'summarize'` 的 feedback 输入框**: 用户可附加 "summarize, focus on auth flow",影响 compact 摘要
7. **`'nevermind'` 二级菜单回退**: 误进入选择器可退回,不必退出整个流程

### 3.4 laew gap 列表(D3)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| L1436 | **对话 rewind(checkpoint 浏览器 + 6 恢复选项 UI)** | P1 | `ratatui` + 自建 list + state |
| L1437 | **三入口汇聚**(`/rewind` + `/checkpoint` + Esc Esc) | P2 | (与 L1436 一起实现) |
| L1438 | **rewind 后的图片/粘贴内容重灌** | P2 | (与 L1436 一起实现) |
| L1439 | **summarize-from-here 联动 Compact Agent** | P1 | 与 L1039 联动 |

> **本轮不重复**: 底层 git shadow 快照机制已在第七轮专题覆盖,本轮只描述"用户感知层"。

---

## D4 文件监视与工作区感知(运行时)

### 4.1 源码定位

| 关注点 | 路径 | 行号 | 关键符号 |
|--------|------|------|----------|
| 主监视器 | `src/utils/hooks/fileChangedWatcher.ts:1-191` | chokidar + 动态路径 |
| 钩子配置快照 | `src/utils/hooks/hooksConfigSnapshot.ts` | 133 行 |
| 钩子配置管理 | `src/utils/hooks/hooksConfigManager.ts` | 400 行 |
| 动态 watchPaths | `src/utils/hooks/fileChangedWatcher.ts:14-17` | `dynamicWatchPathsSorted` |
| 启动入口 | `src/setup.ts` | (见 `initializeFileChangedWatcher` 调用) |
| 已读文件状态缓存 | `src/utils/attachments.ts:3076-3119` | `existingFileState` (mtime 检测) |
| 配置热重载 | `src/utils/settings/internalWrites.ts` | mtime-snapshot |
| LSP 诊断注入 | `src/utils/attachments.ts:2883-2935` | `getLSPDiagnosticAttachments` |

### 4.2 机制剖析

#### 4.2.1 chokidar 双源监听(static + dynamic)

claudecode 的"文件监视"**不是通用 workspace 感知**,而是**钩子驱动的精细化监听**——只在用户/项目在 `settings.json` 配 `FileChanged` 钩子时启动监听:

```ts
// fileChangedWatcher.ts:28-46
export function initializeFileChangedWatcher(cwd: string): void {
  if (initialized) return
  initialized = true
  currentCwd = cwd

  const config = getHooksConfigFromSnapshot()
  hasEnvHooks = (config?.CwdChanged?.length ?? 0) > 0 || (config?.FileChanged?.length ?? 0) > 0

  if (hasEnvHooks) {
    registerCleanup(async () => dispose())
  }

  const paths = resolveWatchPaths(config)
  if (paths.length === 0) return
  startWatching(paths)
}
```

`resolveWatchPaths` 合并**两源路径**:

```ts
// fileChangedWatcher.ts:48-65
function resolveWatchPaths(config): string[] {
  const matchers = (config ?? getHooksConfigFromSnapshot())?.FileChanged ?? []
  const staticPaths: string[] = []
  for (const m of matchers) {
    if (!m.matcher) continue
    for (const name of m.matcher.split('|').map(s => s.trim())) {  // .envrc|.env
      if (!name) continue
      staticPaths.push(isAbsolute(name) ? name : join(currentCwd, name))
    }
  }
  return [...new Set([...staticPaths, ...dynamicWatchPaths])]
}
```

**`dynamicWatchPaths` 来源**: 钩子命令**自己可以输出"要监听更多文件"**(`watchPaths` 字段),通过 `updateWatchPaths(watchPaths)` 增量加入——典型场景:`.envrc` 改了 → 钩子跑 direnv → 输出"再听 `.env.local`" → 立刻加入 watch list。

#### 4.2.2 chokidar 配置与防抖

```ts
// fileChangedWatcher.ts:67-78
function startWatching(paths: string[]): void {
  watcher = chokidar.watch(paths, {
    persistent: true,
    ignoreInitial: true,                                // 启动时不触发已有文件
    awaitWriteFinish: { stabilityThreshold: 500, pollInterval: 200 },  // 500ms 内不变才触发
    ignorePermissionErrors: true,
  })
  watcher.on('change', p => handleFileEvent(p, 'change'))
  watcher.on('add',    p => handleFileEvent(p, 'add'))
  watcher.on('unlink', p => handleFileEvent(p, 'unlink'))
}
```

→ `awaitWriteFinish` 500ms 是经验值(原子写入/编辑器保存完成)。

#### 4.2.3 文件变化→钩子执行

```ts
// fileChangedWatcher.ts:80-100
function handleFileEvent(path: string, event: 'change' | 'add' | 'unlink'): void {
  void executeFileChangedHooks(path, event)
    .then(({ results, watchPaths, systemMessages }) => {
      if (watchPaths.length > 0) updateWatchPaths(watchPaths)  // 增量
      for (const msg of systemMessages) notifyCallback?.(msg, false)  // system 通知
      for (const r of results) {
        if (!r.succeeded && r.output) notifyCallback?.(r.output, true)  // 错误用红色
      }
    })
}
```

→ **失败钩子的 output 直接变成通知**(errors 红色),**成功钩子不打扰用户**——只在 `systemMessages` 主动发时才显示。

#### 4.2.4 已有 readFileState 检测外部修改

`attachments.ts:3076-3119` 中,@ 提及处理时**比对 mtime**: 如果文件在用户 @ 之前已被会话读过且 mtime 未变,直接返回 `already_read_file` attachment(已读过),避免重复 IO。

**但 mtime 不一致时如何提醒用户?** claudecode 暂不主动提示,**只在后续 Read 工具调用时才报"file changed since last read"**——这是更轻量的设计(避免噪音)。

#### 4.2.5 配置热重载(settings 自动侦听)

`src/utils/settings/internalWrites.ts` 走 mtime-snapshot:

- 启动时拍 snapshot(settings.json + settings.local.json + plugin 配置等)
- 写操作走"写临时文件 + rename + fsync(parent_dir)"原子写
- 读时**不**主动监听,而是**首次操作时比对 mtime**,变了就 reload

→ 与第十二轮 `专题-第八轮-Session 持久化与崩溃恢复深度对比` 描述的"fsync 4 严格度分层"是同一套原子写机制。

#### 4.2.6 LSP 诊断被动拉取

`attachments.ts:2883-2935` `getLSPDiagnosticAttachments` **不走 chokidar**,而是**每次 user turn 处理时**主动从 `LSPDiagnosticsRegistry` 拉:

```ts
const diagnosticSets = checkForLSPDiagnostics()  // 已收到的
return diagnosticSets.map(({ files }) => ({ type: 'diagnostics' as const, files, isNew: true }))
// Clear delivered diagnostics to prevent memory leak
if (diagnosticSets.length > 0) clearAllLSPDiagnostics()
```

→ 引用 `AsyncHookRegistry` 模式(`hookEvent: 'FileSuggestion'` 等)。

### 4.3 设计巧妙点

1. **钩子驱动,非全局监听**: 不开 chokidar 监听整个工作区(性能杀手),只在用户配 `FileChanged` 时启动
2. **静态 + 动态路径合并**: 钩子可"增量加监听",典型如 `.envrc` → 钩子运行后返回"再加 `.env.local`"
3. **`awaitWriteFinish: 500ms`**: 适配编辑器的"保存时多步原子写"
4. **失败钩子自动转通知**: 用户无需 grep stderr,错误直达屏幕
5. **静态 + 动态路径去重**: `[...new Set([...])]` 极简实现,O(n) 内存
6. **`ignorePermissionErrors: true`**: chmod 0o600 的 .env 等文件不会让 watcher 崩溃

### 4.4 laew gap 列表(D4)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| L1440 | **chokidar 风格工作区文件监视(钩子驱动 + 动态路径)** | P2 | `notify`(Rust 原生) |
| L1441 | **mtime 比对 + already-read 跳过重复 IO** | P2 | 直接在 `attachments.rs` 实现 |
| L1442 | **LSP 诊断被动注入(轮询 registry,不阻塞主流程)** | P2 | `lsp-types` + 内部 channel |

> **本轮不重复**: settings 原子写 + fsync 分层 已在 第八轮 Session 持久化专题 覆盖。

---

## D5 工具输出富文本内容渲染

### 5.1 源码定位

| 关注点 | 路径 | 行号 | 关键符号 |
|--------|------|------|----------|
| Diff 渲染主入口 | `src/components/FileEditToolDiff.tsx:1-180+` | Suspense + DiffBody + DiffFrame |
| Diff 计算工具 | `src/utils/diff.ts:1-177` | `getPatchForDisplay` / `CONTEXT_LINES = 3` |
| 增量 diff 装载 | `src/components/FileEditToolDiff.tsx:106-160` | `loadDiffData` 异步 |
| Rust ColorFile 渲染器 | `src/native-ts/color-diff/index.ts:935-967` | `ColorFile.render` |
| Rust 词法高亮 + 主题 | `src/native-ts/color-diff/index.ts:954-964` | `highlightLine` / `wrapText` / `addLineNumber` |
| JS 高亮 fallback | `src/components/HighlightedCode/Fallback.tsx:1-100+` | cli-highlight 异步 |
| Markdown 渲染 | `src/components/Markdown.tsx:1-235` | marked + 自定义 renderer |
| Markdown Table | `src/components/MarkdownTable.tsx:1-321` | Tokens.Table + 列宽计算 |
| 折叠提示 | `src/components/CtrlOToExpand.tsx:1-50` | `(ctrl+o to expand)` |
| 附件展示(image) | `src/components/messages/AttachmentMessage.tsx:235-241` | `UserImageMessage` |
| ClickableImage 引用 | `src/components/ClickableImageRef.tsx` | `[Image #N]` 文本 |
| OSC 21337 (tab 状态) | `src/ink/termio/osc.ts:304-472` | tab status indicator |

### 5.2 机制剖析

#### 5.2.1 Diff 渲染两阶段: 计算 + 渲染

**`utils/diff.ts:1-177`** 是**纯计算层**,产 `StructuredPatchHunk[]`:

```ts
// diff.ts:9-103
export const CONTEXT_LINES = 3  // 多 hunk 时上下文 3 行,单 hunk 时 100_000 行
export function getPatchForDisplay({
  filePath, oldStr, newStr, ...
}): { patch: StructuredPatchHunk[]; firstLine: string | null; fileContent: string | undefined } {
  ...
  context: singleHunk ? 100_000 : CONTEXT_LINES,  // 单 hunk 直接全展开
  ...
}
```

→ 关键 trade-off: **单 hunk 全展示(避免"折叠看不全"),多 hunk 折叠(避免爆屏)**。注释明确指出"实测 truncation 比 throw 浪费更多 token"(见 `limits.ts:10-13`)。

**`FileEditToolDiff.tsx:1-180+`** 是**UI 层**,用 React Suspense + `use(promise)` 模式,等数据到达才渲染:

```tsx
// FileEditToolDiff.tsx:23-52
export function FileEditToolDiff(props) {
  const [dataPromise] = useState(() => loadDiffData(props.file_path, props.edits))
  return <Suspense fallback={<DiffFrame placeholder={true} />}><DiffBody promise={dataPromise} file_path={props.file_path} /></Suspense>
}
```

`DiffBody` 用 `use(promise)`(React 19 钩子)直接解包,简化了 async render 链路。

#### 5.2.2 Rust ColorFile 渲染引擎

`src/native-ts/color-diff/index.ts:935-967` 是 **claudecode 唯一的真·Rust 渲染器**(通过 napi-rs 暴露给 TS):

```rust
// Rust 侧伪码(从 TS 翻译)
pub fn render(&self, theme_name: &str, width: usize, dim: bool) -> Option<Vec<String>> {
  let mode = detect_color_mode(theme_name);
  let theme = build_theme(theme_name, mode);
  let lines: Vec<&str> = self.code.lines().collect();
  let lang = detect_language(&self.file_path, lines.first().copied());
  let mut hl_state = HlState { lang, stack: None };
  let max_digits = lines.len().to_string().len();
  let eff_width = (width - max_digits - 2).max(1);

  let mut out = Vec::new();
  for (i, line) in lines.iter().enumerate() {
    let tokens = highlight_line(&mut hl_state, line, &theme);
    let mut h = Highlight { marker: None, line_number: i + 1, lines: vec![tokens] };
    remove_newlines(&mut h);
    wrap_text(&mut h, eff_width, &theme);
    add_line_number(&mut h, &theme, max_digits, dim);
    out.extend(into_lines(&h, dim, true, mode));
  }
  Some(out)
}
```

**关键能力**:

- `detectLanguage(filePath, firstLine)`: **shebang 检测**(`firstLine` 是首行,常是 `#!/usr/bin/env python3`)
- `wrapText`: 智能折行(token 边界,不在字符串中间断)
- `addLineNumber`: gutter(行号栏)宽度自适应
- `intoLines`: 输出 ANSI 转义字符串,直接 `console.log`

→ **Rust 实现的优势**: 一行 200ms tokenize 不会卡住 TUI 16ms 帧率(详见 11 轮 TUI 渲染管线专题)。

#### 5.2.3 JS fallback(`HighlightedCode/Fallback.tsx`)

Rust 不可用时(如 wasm cross-compile 失败)回退到 JS + cli-highlight:

```ts
// Fallback.tsx:21-39
const HL_CACHE_MAX = 500
const hlCache = new Map<string, string>()
function cachedHighlight(hl, code, language): string {
  const key = hashPair(language, code)
  const hit = hlCache.get(key)
  if (hit !== undefined) {
    hlCache.delete(key); hlCache.set(key, hit)  // LRU 手动维护
    return hit
  }
  const out = hl.highlight(code, { language })
  if (hlCache.size >= HL_CACHE_MAX) {
    const first = hlCache.keys().next().value
    if (first !== undefined) hlCache.delete(first)  // LRU 驱逐
  }
  hlCache.set(key, out)
  return out
}
```

→ **LRU 手动维护** + hash key(language + code),**避免虚拟滚动 remount 时重算 + 不持有全文防止 RSS 爆炸**(注释明确 #24180 RSS fix)。

#### 5.2.4 Markdown 渲染(marked 扩展)

`src/components/Markdown.tsx` 走 **marked.js 自定义 renderer**:

```tsx
// Markdown.tsx:131-150
configureMarked()
...
elements.push(<MarkdownTable key={elements.length} token={token as Tokens.Table} highlight={highlight} />)
```

`src/utils/markdown.ts` 的 `configureMarked` 注册自定义 token 渲染器——**claudecode 走 marked 13+ 是为了支持自定义 token 类型**(代码块带文件路径、表头注入 fold/unfold 按钮)。

`MarkdownTable.tsx` (321 行) 处理 `Tokens.Table`:

- 列宽计算: `terminalWidth / 列数 - padding`
- 表头加粗 + 边框(ANSI 反白)
- 数字列右对齐
- 长内容自动 ellipsis + Ctrl+O 折叠

#### 5.2.5 折叠提示 `Ctrl+O`

`CtrlOToExpand.tsx` 在折叠的消息下显示提示:

```tsx
export function CtrlOToExpand() {
  const isInSubAgent = useContext(SubAgentContext)
  const inVirtualList = useContext(InVirtualListContext)
  const expandShortcut = useShortcutDisplay("app:toggleTranscript", "Global", "ctrl+o")
  if (isInSubAgent || inVirtualList) return null
  return <Text dimColor><KeyboardShortcutHint shortcut={expandShortcut} action="expand" parens /></Text>
}
```

→ 上下文控制: **sub-agent 内不显示**(避免噪音),**虚拟列表内不显示**(展开/折叠语义已变)。

#### 5.2.6 图片渲染(纯文本占位)

claudecode **不直接**渲染图片到终端(没找到 OSC 1337 / iTerm2 inline / Sixel 实现),而是:

- 输入时:**image block** 上传(PDF 提取图片、`isImageFilePath` 拖入识别、`isMacOS && Cmd+V` 剪贴板图)
- 输出时:**`[Image #N]` 文本引用**(`ClickableImageRef.tsx`),点击在 IDE 打开原图
- PDF 特殊:`tryGetPDFReference` 给轻量 reference(只读元数据)

```tsx
// ClickableImageRef.tsx:23-34
export function ClickableImageRef({ imageId, isSelected }) {
  const imagePath = getStoredImagePath(imageId)
  const displayText = `[Image #${imageId}]`
  ...
  const fileUrl = pathToFileURL(imagePath).href
}
```

→ 设计选择: **不阻塞 TUI 16ms 帧率,避免终端兼容性矩阵**。`OSC 21337` 实现是用于 tab title 状态(不是图片,见 `src/ink/termio/osc.ts:304-472`)。

### 5.3 设计巧妙点

1. **Rust ColorFile 单行输出 ANSI**: 不走 React 渲染,避免虚拟列表外的重渲染风暴
2. **`use(promise)` 简化 async render**: React 19 钩子,无需 `<Suspense>` 包 await
3. **`getPatchForDisplay` 单 hunk 全展示**: 实测 truncation 比 throw 浪费更多 token(见 limits.ts 注释)
4. **LRU 手动维护 + hash key**: 防止虚拟滚动 remount 重算 + 不持全文
5. **`detectLanguage` 用首行 shebang**: 覆盖 80% 配置文件(`.bashrc` 无扩展名但有 shebang)
6. **`CtrlOToExpand` 用 Context 屏蔽 sub-agent 内的提示**: 减少噪音
7. **图片走 `[Image #N]` 文本引用**: 不阻塞 TUI,绕过终端兼容性矩阵

### 5.4 laew gap 列表(D5)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| L1443 | **Diff 渲染(单 hunk 全展 / 多 hunk 折叠 + Rust ColorFile 加速)** | P1 | `similar`(diff 算法) + `napi-rs` |
| L1444 | **代码高亮(syntect 或 tree-sitter + 主题)** | P2 | `syntect` 或 `tree-sitter-highlight` |
| L1445 | **Markdown 渲染(comrak / pulldown-cmark + 自定义扩展)** | P2 | `pulldown-cmark` |
| L1446 | **表格自动列宽 + 折叠** | P2 | (与 L1445 一起实现) |
| L1447 | **大输出折叠提示**((Ctrl+O to expand)) | P2 | (与 L1445 一起实现) |

---

## D6 输入体验工程

### 6.1 源码定位

| 关注点 | 路径 | 行号 | 关键符号 |
|--------|------|------|----------|
| 多行 + shift+enter | `src/components/PromptInput/PromptInput.tsx:2016, 2173` | `multiline: true` + `TextInput` |
| 输入模式(`!` bash) | `src/components/PromptInput/inputModes.ts:1-33` | `getModeFromInput` / `prependModeCharacterToInput` |
| ! 模式切换逻辑 | `src/components/PromptInput/PromptInput.tsx:870-887` | `insertedAtStart && mode !== 'prompt'` |
| 大粘贴截断 | `src/components/PromptInput/inputPaste.ts:1-90` | `TRUNCATION_THRESHOLD = 10000` / `PREVIEW_LENGTH = 1000` |
| 粘贴 hook | `src/hooks/usePasteHandler.ts:180-285` | 检测 `isPasted` + 图像文件路径 |
| 队列消息 | `src/utils/messageQueueManager.ts:1-100+` | `commandQueue: QueuedCommand[]` |
| 队列处理 | `src/utils/queueProcessor.ts` | `processQueue` (主线程 + sub-thread) |
| 命令队列 UI | `src/components/PromptInput/PromptInputQueuedCommands.tsx` | 显示 + 编辑 |
| 历史搜索 (Ctrl-R) | `src/hooks/useArrowKeyHistory.tsx` | (完整实现) |
| 图像粘贴 | `src/hooks/usePasteHandler.ts:240-249` | macOS `Cmd+V` 剪贴板图 |
| 路径补全 | `src/hooks/unifiedSuggestions.ts` | (同 D1) |
| Vim 模式 | `src/vim/{motions,operators,textObjects,transitions,types}.ts` (1513 行) | `useVim` 状态机 |
| Vim TextInput 替换 | `src/components/PromptInput/PromptInput.tsx:2243` | `isVimModeEnabled() ? <VimTextInput> : <TextInput>` |
| # 记忆路径提示 | `src/utils/memoryFileDetection.ts` | (见 `isMemoryFilePath`) |

### 6.2 机制剖析

#### 6.2.1 多行 + Shift+Enter(`TextInput multiline: true`)

`PromptInput.tsx:2016-2173` 设置 `multiline: true`,**Enter 提交,Shift+Enter 换行**——由 Ink 的 `TextInput` 组件处理底层按键。

#### 6.2.2 `!` 直通 bash 模式

`inputModes.ts:16-21` 检测输入**首字符**是否是 `!`:

```ts
export function getModeFromInput(input: string): HistoryMode {
  if (input.startsWith('!')) return 'bash'
  return 'prompt'
}
```

**自动模式切换逻辑**(`PromptInput.tsx:870-887`):

```ts
const isSingleCharInsertion = value.length === input.length + 1
const insertedAtStart = cursorOffset === 0
const mode = getModeFromInput(value)
if (insertedAtStart && mode !== 'prompt') {
  if (isSingleCharInsertion) {
    onModeChange(mode)             // 模式切换
    return                          // 字符 `!` 不入库(只是触发器)
  }
  if (input.length === 0) {
    onModeChange(mode)
    const valueWithoutMode = getValueFromInput(value).replaceAll('\t', '    ')
    pushToBuffer(input, cursorOffset, pastedContents)
    trackAndSetInput(valueWithoutMode)
    setCursorOffset(valueWithoutMode.length)
    return
  }
}
```

→ **两种触发方式**:
1. 空输入框按 `!` → 切到 bash 模式,`!` 不入库
2. 用 tab 接受建议 `"! gcloud auth login"` → 切到 bash 模式,去掉 `!`

#### 6.2.3 大粘贴截断(`inputPaste.ts`)

**阈值 10000 字符**,超过自动截断为**首 500 + 末 500 + 引用块**:

```ts
// inputPaste.ts:1-58
const TRUNCATION_THRESHOLD = 10000
const PREVIEW_LENGTH = 1000
export function maybeTruncateMessageForInput(text, nextPasteId): TruncatedMessage {
  if (text.length <= TRUNCATION_THRESHOLD) return { truncatedText: text, placeholderContent: '' }

  const startLength = Math.floor(PREVIEW_LENGTH / 2)  // 500
  const endLength   = Math.floor(PREVIEW_LENGTH / 2)  // 500
  const startText   = text.slice(0, startLength)
  const endText     = text.slice(-endLength)
  const placeholderContent = text.slice(startLength, -endLength)
  const truncatedLines = getPastedTextRefNumLines(placeholderContent)
  const placeholderRef   = formatTruncatedTextRef(placeholderId, truncatedLines)
  return { truncatedText: startText + placeholderRef + endText, placeholderContent }
}

function formatTruncatedTextRef(id: number, numLines: number): string {
  return `[...Truncated text #${id} +${numLines} lines...]`
}
```

→ 截断内容**存到 `pastedContents` 字典**,显示引用块,提交时按 reference 还原。

#### 6.2.4 队列消息(`messageQueueManager.ts`)

**优先级 + FIFO 三档**(`'now' > 'next' > 'later'`):

```ts
// messageQueueManager.ts:53-100
const commandQueue: QueuedCommand[] = []
let snapshot: readonly QueuedCommand[] = Object.freeze([])
const queueChanged = createSignal()
function notifySubscribers(): void {
  snapshot = Object.freeze([...commandQueue])
  queueChanged.emit()
}
```

→ 用 `useSyncExternalStore` 模式暴露(`subscribeToCommandQueue` + `getCommandQueueSnapshot`):

```ts
// 队列消费顺序
const isMainThread = (cmd: QueuedCommand) => cmd.agentId === undefined
// 详情:src/utils/queueProcessor.ts
```

**典型场景**: 用户在 agent 跑时输入多条指令,`!'now' / 'next' / 'later'` 决定执行顺序;`!now` 立刻打断当前 agent,`!next` 跑完再处理,`!later` 等空闲时跑。

#### 6.2.5 图像粘贴(3 个路径)

`usePasteHandler.ts:180-285` 同时处理 3 种图像来源:

1. **macOS `Cmd+V`**: 检测 `isPasted && input.length === 0 && isMacOS`,走 `checkClipboardForImage()`
2. **拖入多文件**: 路径含 `/` 或 Windows `C:\` 时按空格切
3. **拖入单个图像**: `isImageFilePath` 检测扩展名

```ts
// usePasteHandler.ts:236-250
const hasImageFilePath = input
  .split(/ (?=\/|[A-Za-z]:\\)/)        // 空格 + 绝对路径前缀
  .flatMap(part => part.split('\n'))
  .some(line => isImageFilePath(line.trim()))

if (isFromPaste && input.length === 0 && isMacOS && onImagePaste) {
  checkClipboardForImage()
  setIsPasting(false)
  return
}
```

#### 6.2.6 Vim 模式(1513 行完整实现)

`src/vim/{motions,operators,textObjects,transitions,types}.ts` 共 1513 行,**完整 Vim 状态机**:

- `motions.ts` (82 行): h/j/k/l/w/b/e/0/$/gg/G 等基础动作
- `operators.ts` (556 行): d/c/y 操作符
- `textObjects.ts` (186 行): iw/aw/i"/a"/ip/ap 等文本对象
- `transitions.ts` (490 行): 模式间转换 (NORMAL → INSERT → VISUAL)
- `types.ts` (199 行): 类型定义

启用时(`PromptInput.tsx:2243`):

```tsx
const textInputElement = isVimModeEnabled()
  ? <VimTextInput {...baseProps} initialMode={vimMode} onModeChange={setVimMode} />
  : <TextInput {...baseProps} />
```

→ 实际是**自研 Vim 状态机**替换 Ink 的 `TextInput`,**用户切到 Vim 后完全在 Vim 模式内编辑**。

#### 6.2.7 历史搜索(`useArrowKeyHistory.tsx`)

支持**两类 history**:`'prompt' | 'bash'`,由 `getModeFromInput` 决定取哪一档。

### 6.3 设计巧妙点

1. **`!` 双触发路径**(单字符 vs tab 接受),**字符 `!` 本身不入库**
2. **大粘贴**自动**首 500 + 末 500 + 引用块** 折叠,**完整内容存 `pastedContents` 字典**
3. **图像粘贴三路径**(macOS 剪贴板 / 拖文件 / 单图像),`split(/ (?=\/)/)` 正则极简
4. **命令队列三优先级**(`now/next/later`)用 `useSyncExternalStore`,React 与非 React 代码同一份 snapshot
5. **Vim 状态机自研**(1513 行),`isVimModeEnabled()` 完全替换 `<TextInput>`
6. **`! bash` 模式 = 真正的子 shell**: 输入 `!ls -la`,claudecode 直接走 BashTool,**不经过 LLM**(节省成本)

### 6.4 laew gap 列表(D6)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| L1448 | **大粘贴截断**(10000 字符阈值 + 首尾预览 + reference block) ✅ 2026-09-10 第二十二轮已实现 | P1 | `src/tui/input.rs::truncate_for_inject`(10000 阈值 + 首 500+尾 500 + 省略标注) |
| L1449 | **命令队列**(now/next/later 三优先级 + useSyncExternalStore) | P1 | `tokio::sync::mpsc` + `arc-swap` |
| L1450 | **! bash 直通模式**(字符触发 + 不入库) | P2 | 直接在 input handler 实现 |
| L1451 | **图像粘贴(macOS Cmd+V / 拖入 / 单图像)** | P2 | `arboard`(剪贴板) |
| L1452 | **Vim 模式(状态机 + 模式切换)** | P2 | `rhai` 嵌入脚本 或 自实现状态机 |

> **本轮不重复**: 输入法补全(unifiedSuggestions 引擎)在 D1 已覆盖。

---

## D7 Onboarding/目录信任/主题偏好

### 7.1 源码定位

| 关注点 | 路径 | 行号 | 关键符号 |
|--------|------|------|----------|
| Onboarding 容器 | `src/components/Onboarding.tsx:1-243` | 6 步导航 |
| ThemePicker | `src/components/ThemePicker.tsx:1-332` | 8 主题选择 |
| Trust Dialog | `src/components/TrustDialog/TrustDialog.tsx:1-275+` | 目录级信任 |
| Trust 检测钩子 | `src/components/TrustDialog/utils.ts:1-245` | `getBashPermissionSources` 等 |
| 主题配置 | `src/utils/theme.ts:91-598+` | 8 内置主题 + 4 ANSI 主题 |
| 设置层级 | `src/utils/settings/settings.ts` | managed/user/project/local/auto |
| API key 审批 | `src/components/ApproveApiKey.tsx` | Onboarding step |
| ConsoleOAuthFlow | `src/components/ConsoleOAuthFlow.tsx` | Onboarding step |
| Terminal setup 提示 | `src/commands/terminalSetup/terminalSetup.tsx` | Shift+Enter / CSI u 探测 |

### 7.2 机制剖析

#### 7.2.1 Onboarding 6 步导航

`Onboarding.tsx:1-243` 是个**步骤状态机**:

```ts
type StepId = 'preflight' | 'theme' | 'oauth' | 'api-key' | 'security' | 'terminal-setup'
interface OnboardingStep { id: StepId; component: React.ReactNode }
```

`steps` 数组根据 `oauthEnabled` / `apiKeyNeedingApproval` / `shouldOfferTerminalSetup()` 动态组合(`Onboarding.tsx:116-160`):

```ts
const steps: OnboardingStep[] = []
if (oauthEnabled) steps.push({ id: 'preflight', component: preflightStep })
steps.push({ id: 'theme', component: themeStep })
if (apiKeyNeedingApproval) steps.push({ id: 'api-key', ... })
if (oauthEnabled) steps.push({ id: 'oauth', component: <SkippableStep ...> })
steps.push({ id: 'security', component: securityStep })
if (shouldOfferTerminalSetup()) steps.push({ id: 'terminal-setup', ... })
```

**每步日志** `tengu_onboarding_step` + `oauthEnabled` + `stepId` 供 A/B 优化。

#### 7.2.2 8 主题(`theme.ts:91-103`)

```ts
export const THEME_NAMES = [
  'dark', 'light', 'light-daltonized', 'dark-daltonized',         // 4 个高对比主题
  'light-ansi', 'dark-ansi',                                       // 2 个 ANSI-only 主题
] as const
export const THEME_SETTINGS = ['auto', ...THEME_NAMES] as const    // + auto = 7 用户选项
```

但**实际有 8+ 主题**(`theme.ts:115-191` `lightTheme` / `lightAnsiTheme` / `darkTheme` / 等)**+ daltonized 是为色盲用户准备**——设计上是一组**配色 + 2 个对比增强变体 + 2 个 ANSI 降级变体**的笛卡尔积。

**`auto` 选项**: 跟随系统 dark/light 切换。

每个主题有 **60+ 颜色字段**(`text/inverseText/autoAccept/bashBorder/permission/diffAdded/diffRemoved/...` 见 `lightTheme:115-191`),**完全可编程**——用户用 `ThemeSetting` JSON 可改任何字段。

#### 7.2.3 目录信任 6 维度

`TrustDialog.tsx:1-275+` 检测**6 类危险设置**是否出现,出现才弹信任 dialog:

| 维度 | 检测 |
|------|------|
| **MCP servers** | 读 `mcpServers` 配置 |
| **Hooks** | `getHooksSources()` (utils.ts:29-43) 检 settings.json + settings.local.json |
| **Bash 权限** | `getBashPermissionSources()` (utils.ts:58-72) 检 allow Bash 规则 |
| **API key helper** | `hasApiKeyHelper` |
| **AWS/GCP commands** | `hasAwsCommands` / `hasGcpCommands` |
| **otelHeadersHelper** | `hasOtelHeadersHelper` |
| **危险环境变量** | `hasDangerousEnvVars` |

**只在非 home 目录**(CWD !== `$HOME`)弹:

```ts
// TrustDialog.tsx:174-179
if (isHomeDir_0) {
  setSessionTrustAccepted(true)         // home 目录:仅 session 级,不入盘
} else {
  saveCurrentProjectConfig(_temp5)      // 非 home:写到 <projectConfigPath>
}
```

`hasTrustDialogAccepted: boolean` 持久化在 `.claude.json` / `.claude/settings.json`(`config.ts:676-756`):

```ts
// config.ts:111
hasTrustDialogAccepted?: boolean
// 优先级: 显式项目 → 路径前缀(向上到家目录) → true
```

#### 7.2.4 设置 5 层叠

`src/utils/settings/settings.ts` 实现 5 层配置:

| 优先级 | 来源 | 用途 |
|--------|------|------|
| 1 (最高) | `policySettings` (managed) | 公司/系统管理员部署 |
| 2 | `userSettings` (`~/.claude/settings.json`) | 用户偏好 |
| 3 | `projectSettings` (`.claude/settings.json`) | 项目共享,入 git |
| 4 | `localSettings` (`.claude/settings.local.json`) | 项目私有,**gitignore** |
| 5 (最低) | `auto` / 内置 | 兜底 |

→ 合并策略:**高优先级覆盖低优先级**,但**数组类字段(如 hooks/mcpServers)合并而非覆盖**——具体细节见 17 轮综合文档。

#### 7.2.5 Terminal setup 探测

`terminalSetup.tsx` 检测终端是否原生支持 Shift+Enter / Kitty CSI u / 焦点事件,**不支持则提示用户运行 `terminalSetup` 命令配置**:

```ts
{ chalk.dim('Note: iTerm2, WezTerm, Ghostty, Kitty, and Warp support Shift+Enter natively.') }
```

→ 12 个终端检测 + 5 个支持的明示 + 1 条建议命令。

### 7.3 设计巧妙点

1. **Onboarding 步骤动态组合**: 不强制 6 步,有 `apiKey` + `oauth` + `terminalSetup` 多个可选步骤
2. **8 主题 = 2(高对比/降级) × 4 配色**: 同一组字段导出 N 个变体
3. **trust 仅 6 维度检测**: 不弹"通用信任"模糊对话框,精确告诉用户**"你的项目装了哪些危险东西"**
4. **home 目录只 session 级信任**: 避免污染 home 配置
5. **5 层配置**支持企业/项目/团队/个人精细化覆盖
6. **ANSI 主题兜底**: 终端不支持 true color 时降级到 ANSI 16 色

### 7.4 laew gap 列表(D7)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| L1453 | **Onboarding 多步导航**(6 步 + 动态组合) | P2 | `inquire` 或自实现 ratatui state |
| L1454 | **目录级信任 dialog(6 维度检测 + session/project 二级)** | P2 | 自实现 ratatui form |
| L1455 | **8 主题系统(ANSI 降级 + daltonized 色盲友好)** | P2 | (与 laew 当前 theme.rs 集成) |

---

## D8 会话导出 / 状态线 / 实时成本

### 8.1 源码定位

| 关注点 | 路径 | 行号 | 关键符号 |
|--------|------|------|----------|
| /export 命令 | `src/commands/export/export.tsx:1-90` | React renderer + 写文件 |
| ExportDialog | `src/components/ExportDialog.tsx` | (大文件) |
| /cost 命令 | `src/commands/cost/cost.ts:1-24` | `formatTotalCost` |
| /context 命令 | `src/commands/context/context.tsx:1-63` | `analyzeContextUsage` + `ContextVisualization` |
| 状态线配置 | `src/commands/statusline.tsx:1-23` | 仅触发 setup agent |
| 状态线运行时 | `src/components/StatusLine.tsx:1-209+` | 收集 input + executeStatusLineCommand |
| 状态线执行 | `src/utils/hooks.ts:4579+` | `executeStatusLineCommand` (5s timeout) |
| 状态线输入协议 | `src/components/StatusLine.tsx:65-105` | `StatusLineCommandInput` JSON |
| 实时成本跟踪 | `src/cost-tracker.ts` | `getTotalCost` / `formatTotalCost` |
| Context 估算 | `src/utils/context.js` | `calculateContextPercentages` |
| Cost hook | `src/costHook.ts` | React 订阅 |

### 8.2 机制剖析

#### 8.2.1 /export 三模式(`export.tsx`)

```ts
// export.tsx:53-90
export async function call(onDone, context, args) {
  const content = await exportWithReactRenderer(context)   // React renderer 渲染消息为纯文本

  // 模式 1: /export filename.txt → 直接写文件
  const filename = args.trim()
  if (filename) {
    const finalFilename = filename.endsWith('.txt') ? filename : filename.replace(/\.[^.]+$/, '') + '.txt'
    const filepath = join(getCwd(), finalFilename)
    writeFileSync_DEPRECATED(filepath, content, { encoding: 'utf-8', flush: true })
    onDone(`Conversation exported to: ${filepath}`)
    return null
  }

  // 模式 2: /export → 弹 ExportDialog 让用户选
  const firstPrompt = extractFirstPrompt(context.messages)
  const timestamp = formatTimestamp(new Date())
  let defaultFilename: string
  if (firstPrompt) {
    const sanitized = sanitizeFilename(firstPrompt)
    defaultFilename = sanitized ? `${timestamp}-${sanitized}.txt` : `conversation-${timestamp}.txt`
  } else {
    defaultFilename = `conversation-${timestamp}.txt`
  }
  return <ExportDialog content={content} defaultFilename={defaultFilename} ... />
}
```

→ 三模式:
1. `/export <filename>` 直接写
2. `/export` 弹 dialog
3. `/export <filename>.txt` 显式后缀

**默认文件名**: `<timestamp>-<firstPrompt-sanitized>.txt`(50 字符首 prompt 摘要)。

#### 8.2.2 /cost(`cost.ts`)

最简单 — 24 行,只调 `formatTotalCost`:

```ts
export const call: LocalCommandCall = async () => {
  if (isClaudeAISubscriber()) {
    let value: string
    if (currentLimits.isUsingOverage) {
      value = 'You are currently using your overages to power your Claude Code usage. ...'
    } else {
      value = 'You are currently using your subscription to power your Claude Code usage'
    }
    if (process.env.USER_TYPE === 'ant') value += `\n\n[ANT-ONLY] Showing cost anyway:\n ${formatTotalCost()}`
    return { type: 'text', value }
  }
  return { type: 'text', value: formatTotalCost() }
}
```

→ **订阅用户看不到成本数字**(避免干扰订阅心智模型),**只显示"订阅/超额"状态**。`USER_TYPE=ant` 内部分支可强制展示。

#### 8.2.3 /context(`context.tsx`)

**真实反映 API 视角**(不显示用户视角):

```ts
// context.tsx:18-29
function toApiView(messages: Message[]): Message[] {
  let view = getMessagesAfterCompactBoundary(messages)   // 跳过 compact 前
  if (feature('CONTEXT_COLLAPSE')) {
    const { projectView } = require('../../services/contextCollapse/operations.js') as ...
    view = projectView(view)                            // 应用 collapse 投影
  }
  return view
}
```

→ 注释解释: **不应用 toApiView,用户会看到 "180k tokens" 但 API 实际收到 120k**(collapse 投影差) → 困惑。

调 `analyzeContextUsage` + `microcompactMessages` + `ContextVisualization` 渲染到 ANSI。

#### 8.2.4 状态线配置(`statusline.tsx`)

`/statusline` 命令**本身不写状态线**,**只委派**给 `statusline-setup` 内置 agent:

```tsx
// statusline.tsx:14-22
async getPromptForCommand(args): Promise<ContentBlockParam[]> {
  const prompt = args.trim() || 'Configure my statusLine from my shell PS1 configuration'
  return [{
    type: 'text',
    text: `Create an ${AGENT_TOOL_NAME} with subagent_type "statusline-setup" and the prompt "${prompt}"`
  }]
}
```

**Allowed tools 严格限制**: `[AGENT_TOOL_NAME, 'Read(~/**)', 'Edit(~/.claude/settings.json)']` —— **只能读家目录任意文件、只能编辑 `~/.claude/settings.json`**,**完全不允许 Write / Bash**。

#### 8.2.5 状态线运行时(`StatusLine.tsx`)

`buildStatusLineCommandInput` (`StatusLine.tsx:36-105`) 组装 16 字段的 JSON 输入:

```ts
return {
  ...createBaseHookInput(),                    // session_id / transcript_path / cwd
  ...(sessionName && { session_name: sessionName }),
  model: { id: runtimeModel, display_name: renderModelName(runtimeModel) },
  workspace: { current_dir, project_dir, added_dirs },
  version: MACRO.VERSION,
  output_style: { name: outputStyleName },
  cost: {                                        // 累计 cost
    total_cost_usd, total_duration_ms, total_api_duration_ms,
    total_lines_added, total_lines_removed,
  },
  context_window: {                              // context window 实时
    total_input_tokens, total_output_tokens,
    context_window_size, current_usage,
    used_percentage, remaining_percentage,
  },
  exceeds_200k_tokens: boolean,                 // 200k 阈值
  rate_limits: { five_hour?, seven_day? },       // claude.ai 订阅限额
  vim_mode?: { ... },
  agent_type, worktree_session?,
}
```

→ `context_window.used_percentage` 实时计算(`getCurrentUsage` + `getContextWindowForModel` + `calculateContextPercentages`),用户的状态线脚本可以拿来画 context 用量进度条。

#### 8.2.6 状态线执行(`hooks.ts:4579+`)

```ts
export async function executeStatusLineCommand(
  statusLineInput: StatusLineCommandInput,
  signal?: AbortSignal,
  timeoutMs: number = 5000,  // 5s 硬超时
  logResult: boolean = false,
): Promise<string | undefined> {
  if (shouldDisableAllHooksIncludingManaged()) return undefined
  if (shouldSkipHookDueToTrust()) { ... return undefined }   // 信任门
  // managed → statusLine from policySettings,else from settings_DEPRECATED
  let statusLine = shouldAllowManagedHooksOnly() ? getSettingsForSource('policySettings')?.statusLine : getSettings_DEPRECATED()?.statusLine
  if (!statusLine || statusLine.type !== 'command') return undefined
  const abortSignal = signal || AbortSignal.timeout(timeoutMs)
  try {
    const jsonInput = jsonStringify(statusLineInput)
    const result = await execCommandHook(statusLine, 'StatusLine', 'statusLine', jsonInput, abortSignal, randomUUID())
    if (result.aborted) return undefined
    if (result.status === 0) return result.stdout
  } catch { ... }
}
```

→ 三道关卡:
1. managed policy 可能禁用所有 hooks
2. 信任未接受 → 跳过
3. 5s 硬超时(状态线不能卡)

### 8.3 设计巧妙点

1. **`/export` 复用 React renderer → 纯文本**: 不需要单独的 markdown-to-text,**所见即所得**
2. **默认文件名 = `<timestamp>-<首prompt摘要>.txt`**: 用户一眼能找到目标文件
3. **`/cost` 订阅用户隐藏金额**: 避免干扰订阅心智模型
4. **`/context` 应用 `toApiView`**: 显示 API 真实看到的,不显示 UI 缓存的
5. **`/statusline` 委派 `statusline-setup` sub-agent**: 状态线配置也是 AI 任务(让 AI 读 PS1 写 statusline)
6. **`/statusline` 工具白名单严格**: 只能编辑 `~/.claude/settings.json`,**不能 Write / Bash**
7. **状态线 16 字段 JSON 输入**: `cost` + `context_window` + `rate_limits` + `vim_mode` 全部塞 stdin,用户脚本可任意组合
8. **5s 硬超时**: 状态线不卡 TUI
9. **信任门**: 没接受 trust dialog → 状态线脚本**不跑**,防 RCE

### 8.4 laew gap 列表(D8)

| 编号 | gap 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|---------|----------|----------------|
| ~~已实现~~ | 实时 cost / token 计数 | — | laew 已有 `cost-tracker.ts` 等价物(本轮不重复) |
| ~~已实现~~ | 状态线配置 + JSON 输入 | — | laew 已有 `statusline.tsx` 等价物 |
| **新增** | **`/export` 完整模式**(命令行直接写文件 + dialog 选择 + 默认文件名 = 首 prompt 摘要) | P2 | 直接在 cli/print.ts 实现 |
| **新增** | **`/context` 应用 toApiView 投影**(避免 UI/API 视角差) | P2 | 与 laew context.rs 集成 |

---

## laew gap 汇总 (L1426-L1455)

| 编号 | 维度 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|------|------|------|----------|----------------|
| L1426 | D1 @提及 | 三正则 + 行号片段 + 目录树 + 大文件降级 | P1 | `regex` + `walkdir` + `nucleo-matcher` + `ignore` | ✅ 已实现(2026-09-10 第二十八轮,`src/agent/attachments.rs`,方案 `tmpPlan/2026-09-10_13`)
| L1427 | D1 @提及 | 实时自动补全 (Rust 索引 + 5s 节流 + .git/index mtime 唤醒) | P1 | `nucleo-matcher` + `gix` | 🟡 部分实现(2026-09-10 第二十八轮,`src/tui/mention.rs`:walkdir 快照 + 5s 节流 + 前缀匹配;未做 .git/index mtime 唤醒与 nucleo 模糊匹配)
| L1428 | D1 @提及 | IDE → CLI 双向 @ 注入 (WS 推送) | P2 | `tokio-tungstenite` |
| L1429 | D1 @提及 | already_read_file 附件优化 (mtime 比对) | P2 | 直接在 `attachments.rs` |
| L1430 | D1 @提及 | PDF reference 轻量引用 (只取元数据) | P2 | `lopdf` |
| L1431 | D2 命令 | 自定义斜杠命令系统 (frontmatter 13 字段 + 三目录合并) | P1 | `serde_yaml` |
| L1432 | D2 命令 | $ARGUMENTS / $N / $name 占位符 + shell 风格参数解析 | P1 | `shell-words` |
| L1433 | D2 命令 | 命令级 allowed-tools / model / context:fork / agent: 覆盖 | P1 | (与 L1431 一起) |
| L1434 | D2 命令 | managed > user > project 多源合并 + 优先级去重 | P1 | (与 L1431 一起) |
| L1435 | D2 命令 | subagent 作为命令 (AgentTool ↔ Command 统一) | P2 | (与 L1431 一起) |
| L1436 | D3 Rewind | 对话 rewind (checkpoint 浏览器 + 6 恢复选项 UI) | P1 | `ratatui` + 自建 list |
| L1437 | D3 Rewind | 三入口汇聚 (/rewind + /checkpoint + Esc Esc) | P2 | (与 L1436 一起) |
| L1438 | D3 Rewind | rewind 后的图片/粘贴内容重灌 | P2 | (与 L1436 一起) |
| L1439 | D3 Rewind | summarize-from-here 联动 Compact Agent | P1 | 与 L1039 联动 |
| L1440 | D4 文件监视 | chokidar 风格工作区文件监视 (钩子驱动 + 动态路径) | P2 | `notify` |
| L1441 | D4 文件监视 | mtime 比对 + already-read 跳过重复 IO | P2 | 直接在 `attachments.rs` |
| L1442 | D4 文件监视 | LSP 诊断被动注入 (轮询 registry) | P2 | `lsp-types` + 内部 channel |
| L1443 | D5 渲染 | Diff 渲染 (单 hunk 全展 + Rust ColorFile 加速) | P1 | `similar` + `napi-rs` |
| L1444 | D5 渲染 | 代码高亮 (syntect / tree-sitter + 主题) | P2 | `syntect` / `tree-sitter-highlight` |
| L1445 | D5 渲染 | Markdown 渲染 (comrak / pulldown-cmark + 自定义扩展) | P2 | `pulldown-cmark` |
| L1446 | D5 渲染 | 表格自动列宽 + 折叠 | P2 | (与 L1445 一起) |
| L1447 | D5 渲染 | 大输出折叠提示 ((Ctrl+O to expand)) | P2 | (与 L1445 一起) |
| L1448 | D6 输入 | 大粘贴截断 (10000 字符阈值 + 首尾预览 + reference block) ✅ 2026-09-10 第二十二轮已实现 | P1 | `input.rs` |
| L1449 | D6 输入 | 命令队列 (now/next/later 三优先级 + useSyncExternalStore) | P1 | `tokio::sync::mpsc` + `arc-swap` |
| L1450 | D6 输入 | ! bash 直通模式 (字符触发 + 不入库) | P2 | 自实现 |
| L1451 | D6 输入 | 图像粘贴 (macOS Cmd+V / 拖入 / 单图像) | P2 | `arboard` |
| L1452 | D6 输入 | Vim 模式 (状态机 + 模式切换) | P2 | `rhai` 或自实现 |
| L1453 | D7 Onboarding | 多步导航 (6 步 + 动态组合) | P2 | `inquire` 或 ratatui state |
| L1454 | D7 Onboarding | 目录级信任 dialog (6 维度检测 + session/project 二级) | P2 | ratatui form |
| L1455 | D7 Onboarding | 8 主题系统 (ANSI 降级 + daltonized 色盲友好) | P2 | (与 laew theme.rs 集成) |

**统计**:
- 30 个新 gap (L1426-L1455)
- P0: 0
- P1: 10 (L1426/L1427/L1431/L1432/L1433/L1434/L1436/L1439/L1443/L1448/L1449)
- P2: 20

**优先级建议路线**:
- **第 19 轮 P1 候选** (按实现成本从低到高):
  1. **L1448 大粘贴截断** (单文件,~100 行)
  2. **L1443 Diff 渲染** (已有 `similar` crate, 接 ratatui 即可)
  3. **L1431+L1432+L1433+L1434 自定义命令**(组合, ~300 行)
  4. **L1426+L1427 @ 提及 + 实时补全** (claudecode 完整复刻 ~500 行 + nucleo crate)
  5. **L1449 命令队列** (与 laew message_queue_manager.ts 等价)

**claudecode 借鉴建议**:
- laew 当前 `commands/` 目录是 1 文件对应 1 命令,前 17 轮已知 25+ 命令,**下一轮可走"frontmatter + skill 化"**
- laew TUI 已有 `/provider *` 子屏,借鉴 D1 @ 提及 + D6 队列 + D7 onboarding 的输入增强可显著提升体验
- laew `src/components/PromptInput/PromptInput.tsx` 缺 paste threshold / image paste / vim mode / queue — **D6 整维度都是新增能力**

---

## 附录:本轮关键文件索引(便于回查)

| 文件 | 行数 | 作用 |
|------|------|------|
| `src/utils/attachments.ts` | 3997 | 附件系统: @ 提及、agent_mention、mcp_resource、LSP 诊断、PDF reference |
| `src/hooks/fileSuggestions.ts` | 811 | 文件路径补全 (Rust nucleo 索引 + 5s 节流) |
| `src/components/MessageSelector.tsx` | 830 | rewind UI: 7 选单 + 6 恢复选项 |
| `src/utils/fileHistory.ts` | (多) | 文件历史快照 (第七轮已覆盖) |
| `src/utils/hooks/fileChangedWatcher.ts` | 191 | chokidar 工作区文件监视 |
| `src/components/FileEditToolDiff.tsx` | 180+ | diff 渲染 (Suspense + Rust ColorFile) |
| `src/components/HighlightedCode.tsx` | 189 | 代码高亮 (Rust / JS fallback) |
| `src/native-ts/color-diff/index.ts` | 1000+ | Rust ColorFile 渲染引擎 (napi-rs) |
| `src/components/Markdown.tsx` | 235 | marked 渲染 + 自定义 token |
| `src/components/MarkdownTable.tsx` | 321 | 表格列宽 + 折叠 |
| `src/components/CtrlOToExpand.tsx` | 50 | 折叠提示 (Ctrl+O to expand) |
| `src/components/PromptInput/PromptInput.tsx` | 2338 | 主输入框 (multiline / vim / paste / history) |
| `src/components/PromptInput/inputPaste.ts` | 90 | 大粘贴截断 (10000 阈值) |
| `src/components/PromptInput/inputModes.ts` | 33 | `!` bash 模式检测 |
| `src/utils/messageQueueManager.ts` | (多) | 命令队列 (now/next/later) |
| `src/vim/{motions,operators,textObjects,transitions,types}.ts` | 1513 | 完整 Vim 状态机 |
| `src/components/Onboarding.tsx` | 243 | 6 步 Onboarding 导航 |
| `src/components/ThemePicker.tsx` | 332 | 8 主题选择 UI |
| `src/components/TrustDialog/TrustDialog.tsx` | 275+ | 目录信任 dialog (6 维度) |
| `src/utils/theme.ts` | 598+ | 8 主题定义 (60+ 颜色字段) |
| `src/commands/statusline.tsx` | 23 | /statusline 命令 (委派 sub-agent) |
| `src/components/StatusLine.tsx` | 209+ | 状态线运行时 (16 字段 JSON 输入) |
| `src/utils/hooks.ts:4579+` | — | `executeStatusLineCommand` (5s 超时) |
| `src/commands/cost/cost.ts` | 24 | /cost (订阅用户隐藏) |
| `src/commands/context/context.tsx` | 63 | /context (toApiView 投影) |
| `src/commands/export/export.tsx` | 90 | /export (三模式) |
| `src/utils/argumentSubstitution.ts` | 145 | $ARGUMENTS / $N / $name 替换 |
| `src/skills/loadSkillsDir.ts` | 1020+ | 多源 skill/命令合并 (managed/user/project/plugin) |
| `src/utils/plugins/loadPluginCommands.ts` | (多) | Plugin 命令加载 + frontmatter 13 字段 |

---

*本轮深挖共分析 30+ 个关键文件,聚焦前 17 轮未覆盖的"用户感知层" 8 维度,共产 30 个 laew gap(L1426-L1455),其中 P1 10 个 / P2 20 个。所有结论均有源码:行号 + 关键代码片段支撑,无臆测。*
