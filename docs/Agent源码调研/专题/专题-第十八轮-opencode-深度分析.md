# 专题-第十八轮-opencode-深度分析 — 用户交互体验层 8 维度

> **生成信息**
> - 源项目: `/usr/local/LsmGitOpenSource/opencode` (TypeScript / Bun, Effect + Schema 全栈 DI, 34 包)
> - 调研日期: 2026-09-09
> - 调研方法: 8 个独立子领域 grep 定位 + 文件:行号引用 + 关键代码片段
> - 覆盖维度: D1 @提及系统 / D2 自定义斜杠命令 / D3 Rewind 时间旅行 / D4 文件监视 / D5 富文本渲染 / D6 输入体验 / D7 Onboarding-Trust-Theme / D8 状态线-成本-导出
> - 累计 laew gap: **L1516-L1545 共 30 个**(已分配、未越界;未实现维度不占 gap)
> - **本轮不重复声明**: Effect/Schema DI / LayerNode / Durable Object / R2 / SSE 5 机制 / 16+ Provider 路由 / Effect 数组式迁移 / drizzle-sqlite / Scope-driven eviction / OAuth 多账户 / 全局生命周期 / TUI 帧协议 / 插件生态 / 录制回放 / 三态权限引擎 — 这些已在前 17 轮专题文档中覆盖,本轮不复述

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
- [laew gap 汇总 (L1516-L1545)](#laew-gap-汇总-l1516-l1545)

---

## D1 @提及系统

### 1.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/tui/src/component/prompt/autocomplete.tsx` | 781 | 主补全 UI:@ / 两种模式 + fuzzysort 排序 + FilePart/AgentPart 注入 |
| `packages/tui/src/prompt/display.ts` | 48 | `mentionTriggerIndex` 光标前最近 @ 探测 |
| `packages/tui/src/prompt/frecency.tsx` | 80 | Frecency(JSONL 持久化,frequency / age 衰减)排序 |
| `packages/tui/src/prompt/history.tsx` | 111 | PromptHistory(JSONL 持久化,50 条上限) |
| `packages/tui/src/context/editor.ts` | 409 | EditorContext + WebSocket JSON-RPC + `at_mentioned` 事件 |
| `packages/opencode/src/session/message.ts:73-79` | — | `FilePart` Schema 定义 + `FilePartSource` 子类型 |
| `packages/sdk/openapi.json` | — | FilePartSource 三种子类型:`file` / `resource` / `symbol` |
| `packages/tui/src/component/prompt/index.tsx:223-235` | — | `fileStyleId` / `agentStyleId` / `pasteStyleId` extmark 样式 |
| `packages/tui/src/component/prompt/index.tsx:1396-1420` | — | `onPaste` 入口与 IME 双 setTimeout flush |

### 1.2 机制剖析

#### 1.2.1 双触发模式 `@` 与 `/`

`Autocomplete` 组件持有 `store.visible: false | "@" | "/"` 三态(autocomplete.tsx:103)。触发与隐藏由 `onInput(value)` 集中管理(autocomplete.tsx:676-708):

```ts
// 伪代码 / autocomplete.tsx:692-708
// Check for "/" at position 0 - reopen slash commands
if (value.startsWith("/") && !value.slice(0, offset).match(/\s/)) {
  show("/"); setStore("index", 0); return
}
// Check for "@" trigger - find the nearest "@" before cursor with no whitespace between
const idx = mentionTriggerIndex(value, offset)
if (idx !== undefined) { show("@"); setStore("index", idx) }
```

`mentionTriggerIndex` 通过 `Intl.Segmenter` grapheme 级切片(防 CJK/emoji 错位),取 `lastIndexOf("@")` 后校验前后空白(display.ts:38-48)。

#### 1.2.2 @ 补全三源

- **文件**(autocomplete.tsx:316-364):通过 `sdk.client.v2.fs.find({ query, limit: 20, location })` 从 fff 服务端拉取,**直接信任 fff 返回顺序**(frecency + fuzzy + filename bonus 已在内核算好);支持 `path#startLine-endLine` 行级 range(`extractLineRange` L32-57)。
- **Agent**(autocomplete.tsx:402-421):`sync.data.agent` 过滤 `!hidden && mode !== "primary"`,所有非主 Agent 都能 `@` 提及。
- **MCP Resource**(autocomplete.tsx:366-400):遍历 `sync.data.mcp_resource`,uri 不参与模糊匹配(避免无关命中)。
- **引用别名**(autocomplete.tsx:282-288):`referenceMatch` 优先于文件搜索,`@` 后 `alias/` 路径访问其他 worktree / git repo。

#### 1.2.3 提及展开为 FilePart

`insertPart` 是核心粘合点(autocomplete.tsx:172-240):

1. 计算当前光标 vs `store.index` 的 logicalCursor。
2. `deleteRange` 删去用户输入的 `query` 文本,`insertText` 写入 `@<displayText> `。
3. 通过 `@opentui/core` 的 `extmarks.create({ virtual: true, typeId: "prompt-part" })` 给虚拟文本挂 extmark(可独立配色、可点击跳转)。
4. 同步将 part push 到 `PromptInfo.parts`(文件型 / agent 型 / text 型),frecency 计入点击的文件路径。

`createFilePart` 是构造器(autocomplete.tsx:242-278),对带行号的 query 自动拼 `path#Lstart-Lend` + URL 上追加 `?start=&end=` query,与后端 `FilePart` Schema 对齐。

#### 1.2.4 编辑器协议层 `@` (`at_mentioned` JSON-RPC)

`editor.ts:74-78` 定义 `EditorMentionSchema`:

```ts
const EditorMentionSchema = Schema.Struct({
  filePath: Schema.String,
  lineStart: Schema.Number,
  lineEnd: Schema.Number,
})
```

WebSocket 客户端订阅 `method === "at_mentioned"` 通知(editor.ts:242-246),回调到 `mentionListeners`,由 `Autocomplete.onMount` 订阅并直接调用 `insertFileMention`。这是从 IDE(Zed/VS Code)按 `@` 拉文件的核心桥梁——与 TUI 本地 @ 触发共享同一管道。

#### 1.2.5 三色 extmark

`prompt/index.tsx:231-233`:

```ts
const fileStyleId = syntax().getStyleId("extmark.file")!
const agentStyleId = syntax().getStyleId("extmark.agent")!
const pasteStyleId = syntax().getStyleId("extmark.paste")!
```

extmark 类型在 `input.extmarks.registerType("prompt-part")` 注册(autocomplete.tsx:1428),三色统一在 `SyntaxStyle` 中注册。

### 1.3 设计巧妙点

- **fff 服务端权威排序**:TUI 不二次 fuzzy 重排,避免覆盖文件路径得分;frecency 仍以客户端加成形式叠在 `scoreFn` 上(autocomplete.tsx:518)。
- **extmark virtual 文本**:用户在文本区看到 `@README.md` 但内部仍按 `PromptInfo.parts` 序列化,提交时按 part 渲染为 FilePart。
- **行级范围语法**:`@src/foo.ts#10-20` 单字符 `#` 复用 git path fragment 习惯,服务端 URL 上追加 `?start=&end=` 透明传递。
- **frecency 公式**:`frequency / (1 + age / 86400000)` 朴素但有效(prompt/frecency.tsx:35);持久化采用 JSONL append-only(`appendText`)+ 启动自愈重写,坏行静默跳过。

### 1.4 laew gap 表(D1)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1516 | 无 @ 提及系统(@file/@agent/@url 三类语义) | P0 | `fuzzy-matcher`+`nucleo-matcher` |
| L1517 | 无 FilePart/AgentPart 多模态 part 序列化 | P0 | `serde`+`serde_json` 自定义 enum |
| L1518 | 无 extmark 虚拟文本与真实文本映射 | P1 | `tui-textarea`+自定义 extmark trait |
| L1519 | 无行级 range 语法 (`@path#Lstart-Lend`) | P1 | 手写 parser(20 行) |
| L1520 | 无编辑器 JSON-RPC 桥(从 IDE 拉 @) | P2 | `tokio-tungstenite`+`jsonrpsee` |

---

## D2 自定义斜杠命令与 Prompt 模板

### 2.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/opencode/src/command/index.ts` | 177 | Command.Service 注册 + 4 源合并(default / cfg / MCP / skill) |
| `packages/opencode/src/config/command.ts` | 39 | `ConfigCommand.load(dir)` 扫描 `{command,commands}/**/*.md` |
| `packages/opencode/src/command/template/initialize.txt` | — | 内置 `/init` 命令模板(项目 AGENTS.md 引导) |
| `packages/opencode/src/command/template/review.txt` | — | 内置 `/review` 命令模板(commit / branch / pr 三档) |
| `packages/core/src/v1/config/command.ts` | 13 | `ConfigCommandV1.Info` Schema(`template`/`description`/`agent`/`model`/`variant`/`subtask`) |
| `packages/opencode/src/config/config.ts:473` | — | `mergeDeep(...ConfigCommand.load(dir))` 多级目录合并 |
| `packages/tui/src/component/prompt/autocomplete.tsx:447-474` | — | `commands` createMemo 拼接 + 排序 + `:mcp` 后缀 |
| `packages/opencode/src/session/prompt.ts:1356-1488` | — | `SessionPrompt.command` 路由(找不到→提示 / 参数解析→命令 / `execute.before` hook) |

### 2.2 机制剖析

#### 2.2.1 Schema 与 frontmatter

`core/v1/config/command.ts` 定义 `Info`:

```ts
export const Info = Schema.Struct({
  template: Schema.String,         // markdown 正文
  description: Schema.optional(Schema.String),
  agent: Schema.optional(Schema.String),
  model: Schema.optional(Schema.String),
  variant: Schema.optional(Schema.String),
  subtask: Schema.optional(Schema.Boolean),
})
```

`config/command.ts:13-39` 通过 `Glob.scan("{command,commands}/**/*.md", { cwd: dir, absolute: true, dot: true, symlink: true })` 扫描 `.opencode/{command,commands}/<name>.md`,经 `ConfigMarkdown.parse(item)` 取 frontmatter,再用 `Schema.decodeUnknownExit` 强校验;**Schema 失败抛 InvalidError**(非静默跳过,确保用户写错立即知道)。

#### 2.2.2 四源合并

`command/index.ts:65-152` 的 `init(ctx)` 注册 4 类命令到 `commands: Record<string, Info>`:

1. **默认**:`init` / `review` 两个内置命令,L18-49。
2. **配置**:`for ... of cfg.command`(来自所有 `.opencode/` 目录),L90-103。
3. **MCP Prompt**:`for ... of mcp.prompts()`,L105-132。每个 MCP prompt 自动成为斜杠命令,`source: "mcp"`,`template` 是 lazy promise(`EffectBridge.promise` 封装)。
4. **Skill**:`for ... of skill.all()`,L134-152。Skill 隐式成为命令,`source: "skill"`,模板追加 `Base directory for this skill: <dir>`。

冲突解决:**后注册覆盖前者**(代码顺序即优先级);skill 命令 `if (commands[item.name]) continue` —— 显式优先 user 配置。

#### 2.2.3 `hints(template)` 占位符抽取

`command/index.ts:36-44`:

```ts
export function hints(template: string) {
  const result: string[] = []
  const numbered = template.match(/\$\d+/g)             // $1 / $2 ...
  if (numbered) for (const match of [...new Set(numbered)].sort()) result.push(match)
  if (template.includes("$ARGUMENTS")) result.push("$ARGUMENTS")
  return result
}
```

`hints` 是 TUI 上 `/` 菜单的占位符提示列,`$ARGUMENTS` 单独检测。两类占位语义:MCP 用 `$1 $2`(per-argument),自定义命令用 `$ARGUMENTS`(整段)。

#### 2.2.4 TUI 集成

`autocomplete.tsx:447-474` `commands` createMemo 拼接 `[...slashes(), ...sync.data.command]`:

- `slashes()` 来自 `useCommandSlashes()` —— TUI 内置(由 `command-palette.tsx` 提供)。
- 每个 server command 行:`label = source === "mcp" ? ":mcp" : ""`,display = `"/" + name + label`。
- `onSelect` 不走 `insertPart` 路径,而是 **清空整行 + 写入 `/<name> `**(autocomplete.tsx:457-461),把命令字面量提交给 `SessionPrompt.command` 路由。

#### 2.2.5 路由与 hook

`session/prompt.ts:1356-1488` 是命令执行入口:

1. `commands.get(input.command)` 查表,**未找到时回 hint**(L1364-1366):
   ```ts
   const available = (yield* commands.list()).map((c) => c.name)
   const hint = available.length ? ` Available commands: ${available.join(", ")}` : ""
   const error = new NamedError.Unknown({ message: `Command not found: "${input.command}".${hint}` })
   ```
2. 命令若是 subtask:`SessionPrompt.subtask` 派发独立 session(L1471-1488)。
3. 命令若是普通:替换为 `user` role 消息(模板渲染后),按 normal prompt 路径送入 LLM。

### 2.3 设计巧妙点

- **MCP Prompt 一等命令**:MCP server 的 prompt 直接暴露为斜杠命令,前端无需额外协议(`hints: prompt.arguments?.map((_, i) => '$' + (i+1))`),降低集成成本。
- **Skill 隐式命令**:Skill 文件即命令,补 `Base directory for this skill: ...` 两行,保留相对路径语义,避免 Skill 作者自己拼路径说明。
- **`hints` 与 schema 校验解耦**:Markdown frontmatter 由 `Info` 强校验,但占位符抽取是纯字符串扫描 —— 故意宽松,允许 `$FOO` 这类未声明变量在模板中作为普通文本。
- **Schema 失败立即抛**:与"坏配置降级"的常见做法相反 —— 命令模板是用户生产力工具,坏一条就开不了,让用户立刻发现。

### 2.4 laew gap 表(D2)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1521 | 无自定义斜杠命令(`.opencode/command/*.md` 机制) | P0 | `serde_yaml`+自定义 markdown frontmatter |
| L1522 | 无命令 frontmatter 校验(`agent`/`model`/`subtask` 元数据) | P1 | `serde`+`schemars` |
| L1523 | 无 `$ARGUMENTS` / `$1` 占位符抽取 | P1 | regex + handlebars-style mini engine |
| L1524 | 无 Skill/MCP/Prompt 命令合并优先级 | P2 | — |
| L1525 | 无命令找不到时的智能 hint(列出 available) | P2 | — |

---

## D3 对话 Rewind/分支/时间旅行(用户级)

### 3.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/opencode/src/session/revert.ts` | 137 | `SessionRevert.Service` 三接口(revert/unrevert/cleanup) |
| `packages/opencode/src/session/session.ts:691-732` | 42 | `Session.fork({ sessionID, messageID? })` 复制消息树 + 重映射 parentID |
| `packages/opencode/src/session/session.ts:787-812` | — | `setRevert`/`clearRevert`/`setShare` 三 patch |
| `packages/opencode/src/snapshot/index.ts` | 800+ | Snapshot Service:git shadow repo + track/restore/revert/diff |
| `packages/tui/src/routes/session/dialog-timeline.tsx` | 47 | 时间轴 Dialog:逆序列出 user message,点选跳转 |
| `packages/tui/src/routes/session/dialog-message.tsx` | 109 | 单消息操作:Revert / Copy / Fork |
| `packages/tui/src/routes/session/dialog-fork-from-timeline.tsx` | 76 | Fork 选择 Dialog:Full session / 从某 message 起 fork |
| `packages/tui/src/routes/session/index.tsx:610-670` | 60 | `session.undo` / `session.redo` 快捷 slash 命令 |
| `packages/tui/src/component/dialog-session-list.tsx` | 364 | Session list 列表 + pin + 续接 |

### 3.2 机制剖析

#### 3.2.1 Revert 三段式

`SessionRevert.revert` (revert.ts:38-89) 是核心:

```ts
// revert.ts:38-89
for (const msg of all) {
  if (msg.info.role === "user") lastUser = msg.info
  for (const part of msg.parts) {
    if (rev) {
      if (part.type === "patch") patches.push(part)        // 收集需要回滚的 patch part
      continue
    }
    if ((msg.info.id === input.messageID && !input.partID) || part.id === input.partID) {
      const partID = remaining.some((item) => ["text","tool"].includes(item.type)) ? input.partID : undefined
      rev = { messageID: !partID && lastUser ? lastUser.id : msg.info.id, partID }
    }
    remaining.push(part)
  }
}

rev.snapshot = session.revert?.snapshot ?? (yield* snap.track())         // 备份当前
if (session.revert?.snapshot) yield* snap.restore(session.revert?.snapshot)  // 还原到更早快照
yield* snap.revert(patches)                                              // git 倒序应用 patch 反转
if (rev.snapshot) rev.diff = yield* snap.diff(rev.snapshot)              // 与原始快照 diff
```

要点:
- **patch part 收集**:回滚一个 message 时,需收集该 message 之后的 patch part(工具修改文件的结果),倒序 reverse。
- **嵌套 revert**:`session.revert?.snapshot` 保留 —— 链式还原支持。
- **diff 输出**:`setRevert` 时把 `diff`(additions/deletions/files 统计 + 详细 patches)写入 session 行,UI 可直接展示。

#### 3.2.2 Unrevert 与 Cleanup

`unrevert`(revert.ts:91-99) 仅在 `session.revert` 存在时执行 `snap.restore` + `clearRevert`,**不动消息**。

`cleanup`(revert.ts:101-124) 在 session 切换时清理:
- 找 `revert.messageID` 索引,删掉它之后的所有 message(`sessions.removeMessage`)。
- 若 `revert.partID` 存在,删该 part 之后的部分 parts。
- **不可逆**:触发条件是 session 不再 busy 后实际写入数据库,避免 undo 状态污染。

#### 3.2.3 Fork(分支)

`session.fork`(session.ts:691-732) 与 Revert 互补 —— Fork 创建**新 session**,Revert 保留**同 session 的多个 revert 点**。

```ts
const idMap = new Map<string, MessageID>()
const target = input.messageID ? msgs.findIndex((m) => m.info.id === input.messageID) : msgs.length
for (const msg of msgs.slice(0, target < 0 ? msgs.length : target)) {
  const newID = MessageID.ascending()
  idMap.set(msg.info.id, newID)
  // parentID 重映射:assistant 的 parentID 是上一条 assistant message,需在新树里找对应 ID
  const parentID = msg.info.role === "assistant" && msg.info.parentID ? idMap.get(msg.info.parentID) : undefined
  const cloned = yield* updateMessage({ ...msg.info, sessionID: session.id, id: newID, ...(parentID && { parentID }) })
  for (const part of msg.parts) {
    const p = { ...part, id: PartID.ascending(), messageID: cloned.id, sessionID: session.id }
    if (p.type === "compaction" && p.tail_start_id) p.tail_start_id = idMap.get(p.tail_start_id)
    yield* updatePart(p)
  }
}
```

要点:
- **parentID 重映射**:assistant message 链由 parentID 串联;fork 时用 `idMap` 把旧 ID 映射到新 ID,保持 assistant 树形结构。
- **compaction tail_start_id**:compaction part 持有 `tail_start_id` 指向压缩边界前的 message ID,也要在 fork 时重映射。
- **标题**:`getForkedTitle`(L162-168) 生成 `"Original (fork #N)"` 自增序列号。

#### 3.2.4 Timeline UI

`dialog-timeline.tsx` 列出 user message(`!synthetic && !ignored`),title 是首行 text,footer 是 `Locale.time(time.created)`。点选 → `dialog.replace(() => <DialogMessage ...>)`。

`DialogMessage`(dialog-message.tsx) 提供三操作:
- **Revert**:`sdk.client.session.revert({ sessionID, messageID })`,同时把 prompt 还原到 message 的内容。
- **Copy**:聚合所有 text part 的 text 到剪贴板。
- **Fork**:`session.fork({ sessionID, messageID })`,prompt 携带该 message 的 parts 跳转。

#### 3.2.5 Undo/Redo 快捷 slash

`session/index.tsx:611-670` 提供两个 slash:

- `/undo`:`messagesBeforeRevert().findLast(role === "user")` → revert 到上一条 user message + 还原 prompt。
- `/redo`:若有 `session.revert.messageID`,找 `id > messageID` 的第一个 user message 重新 revert;若无,unrevert + 清空 prompt。

### 3.3 设计巧妙点

- **Revert 与 Fork 解耦**:Revert 在同 session 内维护多个 revert 栈(由 `session.revert.snapshot` 链式保留),适合"微调同一个对话";Fork 创建独立 session,适合"我想试不同方向但不丢原 session"。
- **assistant parentID 树**:`parentID` 不是简单链表,是树 —— 一条 user message 可以派生多条 assistant message(parallel tool / 工具调用的并行分支);fork 时需保留这个树形结构。
- **`!synthetic && !ignored` 过滤**:工具返回的合成 user message(`<system-reminder>` 自动注入)与被忽略的 part 不进入 timeline,避免 UI 噪音。

### 3.4 laew gap 表(D3)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1526 | 无 session revert(同 session 多 revert 点栈式) | P0 | `rusqlite`+自定义 revert 表 |
| L1527 | 无 session fork(parentID 重映射 + compation tail_start_id) | P0 | 树形序列化 + ID remap |
| L1528 | 无 timeline dialog(逆序 user message + 跳转滚动) | P1 | `ratatui` List + 滚动 API |
| L1529 | 无 undo/redo slash 快捷 | P1 | — |
| L1530 | 无 revert diff 渲染(additions/deletions/files 统计) | P2 | `similar` crate |

---

## D4 文件监视与工作区感知(运行时)

### 4.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/tui/src/component/dialog-workspace-file-changes.tsx` | 144 | 移动 session 时检测到外部修改的 yes/no 确认 Dialog |
| `packages/tui/src/component/dialog-workspace-create.tsx` | 308 | Workspace 创建(分支切换场景) |
| `packages/tui/src/component/dialog-workspace-list.tsx` | 112 | Workspace 列表 |
| `packages/tui/src/component/prompt/workspace.tsx` | 137 | 工作区选择器(嵌入 prompt) |
| `packages/opencode/src/snapshot/index.ts` | 800+ | git shadow repo 实现文件状态(被 revert 调用) |
| `packages/opencode/src/tool/apply_patch.ts` | 290+ | 文件编辑工具(返回 `fileChanges` 数组) |
| `packages/tui/src/routes/session/index.tsx:530-540` | — | `subagent-footer.tsx` 实时展示 usage/cost |

### 4.2 机制剖析

#### 4.2.1 opencode 的隐式策略:**无后台 fs.watch**

调研证实 opencode **没有实现 fs.watch / chokidar / fsevents / @parcel/watcher 主动监听文件变化**(全代码库无 `fs.watch`、`chokidar`、`onDidChangeWatchedFiles` 引用)。这是有意选择 —— 终端用户输入有明确时机,而文件监视需要权衡性能 vs 准确性。

#### 4.2.2 替代机制:操作边界检测

`dialog-workspace-file-changes.tsx` 是被动式文件感知(L133-144):

```ts
DialogWorkspaceFileChanges.show = (
  dialog: DialogContext,
  files: VcsFileStatus[],       // 由调用方传入的 git status 数组
  options?: { title?: string; message?: string },
) => {
  return new Promise<WorkspaceFileChangesChoice | undefined>((resolve) => {
    dialog.replace(
      () => <DialogWorkspaceFileChanges files={files} onSelect={resolve} {...options} />,
      () => resolve(undefined),
    )
  })
}
```

调用方在 **session 切换/移动**时执行 `git status --porcelain` 之类命令,把变化列表 `files` 传进来;Dialog 渲染:
- status badge:`A`(added)/`D`(deleted)/`M`(modified)
- 增减统计:`+N -N` 用 `theme.diffAdded`/`theme.diffRemoved`
- 鼠标点击 yes/no + Enter 提交,Esc 取消

#### 4.2.3 LSP 诊断订阅(隐式存在)

`packages/opencode/src/lsp/client.ts` 实现了 LSP JSON-RPC 客户端(详情见第 13 轮 / 第 15 轮),**但 TUI 层没有把 LSP `publishDiagnostics` 推到主消息流** —— 调研 grep 未发现 `lsp.diagnostic` 在 TUI 任何屏被订阅。`footer.tsx`(routes/session)只显示 LSP server 数量,不显示具体诊断。

`subagent-footer.tsx:15` 仅 `sync.data.lsp` 计数 —— LSP 接入但诊断不显示给用户。

#### 4.2.4 Git snapshot 感知

`Snapshot.Service`(snapshot/index.ts)是"何时备份"的决策点:
- `track()`:会话开始时记录当前 git 状态(commit hash)。
- `patch(hash)`:每次工具修改文件后,git 记录 patch。
- `diff(hash)`:与初始 hash 对比。

所以"外部编辑"在 revert 时被检测 —— 但**不在运行中显示**。

### 4.3 设计巧妙点

- **不做后台 fs.watch**:终端会话本身有清晰边界(start / message),无需长连接监听;降低 CPU/电池消耗,避免误报。
- **Workspace 概念抽象**:opencode 引入 workspace(worktree 副本)概念 —— 用户可"fork 一份当前 worktree 试另一个分支",文件状态在 workspace 创建/删除时被集中检测并提示。
- **被动模式可组合**:把"是否要带未提交修改"的选择权留给用户 —— 而不是 Agent 默默决定。

### 4.4 laew gap 表(D4)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1531 | 无 workspace 移动时的外部修改检测(被动式) | P1 | `git2`+`Command::new("git")` |
| L1532 | 无 LSP 诊断实时推送至 TUI(`publishDiagnostics` 订阅) | P1 | `lsp-types` + 自定义订阅 |
| L1533 | 无运行时 fs.watch(chokidar/fsevents) | P2 | `notify` crate(默认不启用) |

---

## D5 工具输出富文本内容渲染

### 5.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/ui/src/context/marked.tsx` | 28 | `createMarkdownParser` + `getSharedHighlighter`(shiki-wasm) |
| `packages/ui/src/context/marked-parser.tsx` | 68 | Marked 扩展 + KaTeX 数学公式 + marked-shiki |
| `packages/ui/src/context/marked-theme.tsx` | — | `OpenCodeTheme`(注入 shiki 主题) |
| `packages/ui/src/context/marked-theme-register.tsx` | — | `registerCustomTheme` 注册到 @pierre/diffs |
| `packages/ui/src/components/diff-changes.tsx` | 110+ | 文件级 +/- 计数徽章 |
| `packages/tui/src/feature-plugins/system/diff-viewer.tsx` | 700+ | TUI diff 查看器(file tree + 单 patch 模式 + 折叠) |
| `packages/tui/src/feature-plugins/system/diff-viewer-ui.tsx` | 103 | PanelGroup/Panel/Separator 布局原语 |
| `packages/opencode/src/snapshot/index.ts:4` | — | `diff` npm 包(structuredPatch)用于 patch 生成 |
| `packages/app/package.json` | — | `@pierre/diffs` + `shiki` + `marked-shiki` 依赖 |

### 5.2 机制剖析

#### 5.2.1 @pierre/diffs 双渲染后端

Web/Desktop 用 [@pierre/diffs](https://github.com/pierrecomputer/diffs),作为 Web Component(自定义元素 `<diffs>`)+ Shiki 语法高亮双能力:

```ts
// ui/src/context/marked.tsx:14-26
export const { use: useMarked, provider: MarkedProvider } = createSimpleContext({
  name: "Marked",
  init: () =>
    createMarkdownParser(async (code, language) => {
      const highlighter = await getSharedHighlighter({
        themes: ["OpenCode"],          // 自定义主题,见 marked-theme-register.tsx
        langs: [],
        preferredHighlighter: "shiki-wasm",   // wasm 版本,跨平台
      })
      const name = language in bundledLanguages ? language : "text"
      if (!highlighter.getLoadedLanguages().includes(name)) await highlighter.loadLanguage(name as BundledLanguage)
      return highlighter.codeToHtml(code, {
        lang: name, theme: "OpenCode", tabindex: false,
      })
    }),
})
```

要点:
- **shiki-wasm 优先**:跨平台(避免 Node 原生绑定问题),主题统一 OpenCode。
- **懒加载语言**:首次见某语言才 `loadLanguage`,避免启动全量 bundle 200+ 语言。
- **KaTeX 扩展**:marked 自定义 tokenizer 处理 `\\(...\\)` 行内 / `$$...$$` 块级数学公式(marked-parser.tsx:23-61)。

#### 5.2.2 TUI diff 查看器架构

TUI 不依赖 pierre/diffs,而是自建 `diff-viewer.tsx`(700+ 行):
- `PanelGroup`(axis: "x"/"y") + `Panel` + `Separator` 布局原语(diff-viewer-ui.tsx)。
- `DiffViewerFileTree`:左侧文件树(`diff-viewer-file-tree.tsx`,展开/折叠 + 文件过滤)。
- **单 patch 模式**(KV `diff_viewer_single_patch`)+ **统一/分栏模式**(`diff_toggle_view`)。
- 文件跳转:`diff_next_file` / `diff_previous_file` / `diff_next_hunk` / `diff_previous_hunk` 四组 keybind。
- 高亮:`diff_toggle`(enter/space 展开) / `diff_expand_all`(E 全展开)。

#### 5.2.3 折叠 / 分页 / 长输出

`scrollbox` + `scrollAcceleration`(util/scroll)是基础。`scrollbox height={min(count, 10, anchor.y)}`(autocomplete.tsx:712-717)限定最大高度 10 行。

`dialog-export-options.tsx`(`packages/tui/src/ui/`,7634 bytes)是导出选项对话框(用于导出 markdown 时选择 include reasoning / patches 等开关)。

#### 5.2.4 Markdown 渲染:Web 与 TUI 分裂

| 维度 | Web/Desktop | TUI |
|---|---|---|
| 引擎 | marked + shiki-wasm | @opentui/core SyntaxStyle |
| Diff | @pierre/diffs(自研) | 自建 diff-viewer |
| 数学 | KaTeX | 不支持 |
| 图片 | inline `<img>` | extmark 虚拟文本 `[Image N]` |

### 5.3 设计巧妙点

- **Web Component + Shiki 双栈**:`@pierre/diffs` 是 Web Component,直接在 .tsx 中 `<diffs>` 使用(ui/src/custom-elements.d.ts:1),无需额外封装。
- **`preferredHighlighter: "shiki-wasm"`**:跨平台 + 主题统一,服务端渲染 SSR 友好。
- **TUI 自建 diff-viewer**:完全控制布局(分栏/折叠/文件树),不依赖任何外部 TUI 库;`PanelGroup` 抽象支持 axis 切换。
- **KaTeX 扩展守边界**:`throwOnError: false`,公式错也不抛(降级为 LaTeX 原文)。

### 5.4 laew gap 表(D5)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1534 | 无 Shiki 语法高亮(服务端/客户端共享) | P1 | `syntect` 或 tree-sitter |
| L1535 | 无 Markdown 渲染组件(marked + 自定义扩展) | P1 | `pulldown-cmark`+`comrak` |
| L1536 | 无 TUI diff viewer(file tree + 折叠 + 单 patch 模式) | P1 | `ratatui` + `similar` |
| L1537 | 无图片/PDF 附件虚拟文本(`[Image N]`/`[PDF N]`) | P1 | base64 + ratatui image |
| L1538 | 无 KaTeX 数学公式渲染 | P2 | 跳过或用 WebView |

---

## D6 输入体验工程

### 6.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/tui/src/component/prompt/index.tsx` | 1716 | 主 Prompt 组件(TextareaRenderable + 粘贴 + 多行) |
| `packages/tui/src/component/prompt/autocomplete.tsx` | 781 | 补全菜单(详见 D1/D2) |
| `packages/tui/src/component/prompt/local-attachment.ts` | 48 | 本地附件读(text/binary 分类) |
| `packages/tui/src/component/prompt/move.tsx` | 205 | Move 操作 UI(移动 session 到其他 workspace) |
| `packages/tui/src/component/prompt/workspace.tsx` | 137 | 工作区选择器 |
| `packages/tui/src/component/prompt/stash.tsx` | 89 | Prompt 暂存(stash/pop/list) |
| `packages/tui/src/component/prompt/history.tsx` | 111 | 历史记录(50 条上限,自愈重写) |
| `packages/tui/src/component/prompt/index.tsx:1183-1222` | — | `pasteInputText` 函数 |
| `packages/tui/src/config/keybind.ts:161-200` | 40 行 | input_* 键位定义(40+ keybind) |

### 6.2 机制剖析

#### 6.2.1 编辑器核心:TextareaRenderable

opencode 不自研编辑器,而是采用 [@opentui/core](https://github.com/anthropics/opentui-archive) 的 `TextareaRenderable`(prompt/index.tsx:4-10):

```ts
import {
  BoxRenderable, TextareaRenderable, ScrollBoxRenderable,
  PasteEvent, type MouseEvent, type KeyEvent,
} from "@opentui/core"
```

关键能力:
- **多行**:`input_newline` = `shift+return,ctrl+return,alt+return,ctrl+j` 四种(keybind.ts:164)。
- **光标定位**:`input_line_home`(ctrl+a)/ `input_line_end`(ctrl+e)/ `input_buffer_home`(home)/ `input_buffer_end`(end)分四层(keybind.ts:173-184)。
- **删除**:`input_delete_to_line_end`(ctrl+k)/ `input_delete_to_line_start`(ctrl+u)/ `input_delete_word_forward`(alt+d)三类(keybind.ts:186-197)。
- **历史**:`history_previous`(up)/ `history_next`(down) —— 但不是默认 up/down(那是 `input_move_up/down`),需按 `history_previous` 才滚历史(prompt/history.tsx:69-84)。

#### 6.2.2 IME 双 setTimeout flush

中文/韩文 IME 提交时,最后一个组合字符可能在 `onSubmit` 触发时还未 flush 到 plainText。`prompt/index.tsx:1391-1395`:

```tsx
onSubmit={() => {
  // IME: double-defer so the last composed character (e.g. Korean hangul)
  // is flushed to plainText before we read it for submission.
  setTimeout(() => setTimeout(() => submit(), 0), 0)
}}
```

两轮 setTimeout 让 IME 的合成事件先被 process 一轮,第二轮才真正提交。**这是 laew 当前完全没有的细节**(laew 退格键 trap 已记录)。

#### 6.2.3 粘贴处理 5 档

`pasteInputText`(prompt/index.tsx:1183-1222) 是粘贴处理中枢:

1. **规范化**:`replace(/\r\n/g, "\n").replace(/\r/g, "\n")` —— Windows ConPTY/Terminal 经常给 CR-only 换行(注释在 L1403-1404)。
2. **空粘贴**:Windows Terminal <1.25 图片剪贴板发送空 bracketed paste → 触发 `prompt.paste` 命令(读 image clipboard)。
3. **本地文件路径检测**:`pastedFilepath`(prompt/index.tsx:78-100) 探测是否是路径,若是,读 attachment,text 渲染 `[SVG: filename]` / binary 渲染 `[Image N]` / `[PDF N]`。
4. **URL**:不读 attachment。
5. **大段文本折叠**:`(lineCount >= 3 || length > 150) && paste_summary_enabled` 触发折叠成 `[Pasted ~N lines]`。

折叠开关:`kv.get("paste_summary_enabled", !config.experimental?.disable_paste_summary)` —— 用户可持久化偏好(KV 在 `~/.config/opencode/kv.json`)。

#### 6.2.4 Tab 补全

`prompt.autocomplete.complete` = `tab`(keybind.ts:218)。在 `autocomplete.tsx:619-632` 中:

```ts
{
  name: "prompt.autocomplete.complete",
  run() {
    const selected = options()[store.selected]
    if (selected?.isDirectory) {
      expandDirectory()         // 若是目录,展开路径
      return
    }
    select()                    // 否则选中
  },
}
```

`expandDirectory` 把 `@src` 展开为 `@src/`(autocomplete.tsx:560-579) —— 渐进式目录探索。

#### 6.2.5 Prompt 暂存(stash)

`prompt/stash.tsx` 提供 `/stash` / `/stash pop` / `/stash list` 三 slash 命令(对应 keybind `prompt_stash` / `prompt_stash_pop` / `prompt_stash_list`,keybind.ts:155-157)。允许"把当前输入暂存起来去做别的事,回来再 pop 出来"。

### 6.3 设计巧妙点

- **行/段/缓冲三层光标**:`line_*` / `select_*` / `buffer_*` / `visual_line_*` 四种修饰符 × `home/end/left/right/up/down` 6 个方向 = 24 个组合,Emacs 风格。
- **空粘贴降级**:Windows Terminal <1.25 的图片剪贴板兼容,转 `prompt.paste` 命令兜底。
- **frecency 持久化自愈**:`parseFrecency` 启动时跳过解析失败的行,然后 **重写整个文件** 去坏行(prompt/frecency.tsx:51-52)。
- **extmark 虚拟文本**:`getClipboardText` 重写 —— 当粘贴回写操作系统剪贴板时,把 `[Image 1]` 还原成原始 base64(autocomplete.tsx 通过 Object.assign 注入到 input,prompt/index.tsx:1421-1426)。

### 6.4 laew gap 表(D6)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1539 | 无 IME 双 setTimeout flush(Korean/CJK 末字丢失修复) | P0 | tui-textarea 自定义提交回调 |
| L1540 | 无粘贴 5 档处理(空粘贴/路径/URL/折叠/二进制) | P1 | `arboard` + `mime` |
| L1541 | 无 Tab 渐进式目录展开(`@src/` 探索) | P2 | — |
| L1542 | 无 Prompt stash / pop / list(临时保存输入) | P2 | `directories` crate + JSONL |

---

## D7 Onboarding/目录信任/主题偏好

### 7.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/tui/src/theme/index.ts` | 700+ | 主题系统核心:DEFAULT_THEMES 30+ 内置 + Resolve 引擎 + system 派生 |
| `packages/tui/src/theme/assets/*.json` | 30 个 | 内置主题 JSON(aura/ayu/catppuccin/dracula/github/gruvbox/tokyonight 等) |
| `packages/tui/src/component/dialog-theme-list.tsx` | 50 | 主题选择 Dialog |
| `packages/opencode/src/config/tui.ts:83-226` | 144 | `TuiConfig.loadState` 八级配置发现链 |
| `packages/opencode/src/config/paths.ts` | 46 | `ConfigPaths.directories` / `projectFiles` 抽象 |
| `packages/tui/src/feature-plugins/home/tips-view.tsx` | 200+ | Home 屏 tips 列表(30+ 条) + 随机播 |
| `packages/tui/src/feature-plugins/home/tips.tsx` | 39 | tips.toggle keybind(`<leader>h`) |
| `packages/tui/src/feature-plugins/home/footer.tsx` | 100 | Home footer(mcp/version/directory) |
| `packages/tui/src/context/theme.tsx` | — | 主题 context(运行时切换) |
| `packages/opencode/src/config/tui-migrate.ts` | — | 旧 opencode.json → tui.json 迁移 |

### 7.2 机制剖析

#### 7.2.1 主题系统架构

主题 `Theme` 类型 64 个字段(theme/index.ts:36-91),涵盖 `primary/secondary/accent/error/warning/success/text/.../diffAdded/diffRemoved/markdown*/syntax*/` 全套。`ThemeJson` 支持 Hex / RefName / Variant(dark+light)/ RGBA 四类值(theme/index.ts:120-128)。

`DEFAULT_THEMES`(theme/index.ts:130-164)注册 30+ 内置主题:`aura/ayu/catppuccin/{frappe,macchiato}/cobalt2/cursor/dracula/everforest/flexoki/github/gruvbox/kanagawa/material/matrix/mercury/monokai/nightowl/nord/one-dark/opencode/orng/lucent-orng/osaka-jade/palenight/rosepine/solarized/synthwave84/tokyonight/vercel/vesper/zenburn/carbonfox` —— **远超 laew 当前的 1 个默认主题**。

#### 7.2.2 主题优先级链

`theme/index.ts:171-183` `listThemes()` 严格排序:

```ts
function listThemes() {
  const themes = {
    ...DEFAULT_THEMES,      // 最低:内置
    ...pluginThemes,         // 插件注册
    ...customThemes,         // 用户自定义文件
  }
  if (!systemTheme) return themes
  return { ...themes, system: systemTheme }   // 最高:运行时派生(根据 terminal 颜色)
}
```

`system` 是 **从 `TerminalColors` 自动派生**(theme/index.ts:360-460):根据 terminal 默认 fg/bg 计算 ANSI 16 色 → 灰阶 → diff alpha tint → 完整 theme。所以"终端绿底白字 + 暗色 diff"等组合全自动。

#### 7.2.3 Resolve 引擎:循环引用检测

`resolveTheme`(theme/index.ts:241-299)递归解析 ref / variant:

```ts
function resolveColor(c: ColorValue, chain: string[] = []): RGBA {
  if (typeof c === "string") {
    if (c === "transparent" || c === "none") return RGBA.fromInts(0, 0, 0, 0)
    if (c.startsWith("#")) return RGBA.fromHex(c)
    if (chain.includes(c)) {
      throw new Error(`Circular color reference: ${[...chain, c].join(" -> ")}`)   // ← 循环引用检测
    }
    const next = defs[c] ?? theme.theme[c as ThemeColor]
    if (next === undefined) throw new Error(`Color reference "${c}" not found in defs or theme`)
    return resolveColor(next, [...chain, c])
  }
  if (typeof c === "number") return ansiToRgba(c)
  return resolveColor(c[mode], chain)   // variant.dark / variant.light
}
```

支持:
- `$defs` 跨字段引用。
- `{ dark: ..., light: ... }` 自动按 `terminalMode` 切换。
- ANSI 0-255 数字(6×6×6 cube + grayscale ramp)。
- 透明背景的 selectedListItemText 用 luminance 公式自动反白(theme/index.ts:101-107)。

#### 7.2.4 配置发现 8 级链

`config/tui.ts:170-210` 是核心:

```ts
const directories = yield* ConfigPaths.directories(ctx.directory)        // 1. 全局 + .opencode 上溯
yield* Effect.promise(() => migrateTuiConfig({ directories, cwd: ctx.directory }))  // 2. 老格式迁移

const projectFiles = ...ConfigPaths.files("tui", ctx.directory)           // 3. 沿目录树上溯找 tui.json

// 顺序应用:
// 1. 全局 ~/.config/opencode/tui.json(.json 优先)
// 2. OPENCODE_TUI_CONFIG 环境变量
// 3. 项目 tui.json(root-first,越近越近越覆盖)
// 4. .opencode/ 目录里的 tui.json
```

`mergeDeep`(remeda)做字段级合并而非整文件覆盖 —— 用户可以只覆盖 `theme`,其余字段从默认继承。

#### 7.2.5 Tips(Onboarding 入口)

`tips-view.tsx:164-219` 列 30+ 条 tips,运行期随机选一条(Math.random 作 seed,tips-view.tsx:99)。覆盖范围:
- @/! 语法提示(L165-166)
- 快捷键(`/undo`、`/redo`、`/share`)
- 配置路径(`opencode.json` vs `tui.json` 区分,L213)
- 键位修改(tui.json keybinds 段,L217)
- MCP / Skill(L219)

`<leader>h`(keybind `tips_toggle`)显示/隐藏,tips 状态持久化到 KV:`kv.set("tips_hidden", !kv.get("tips_hidden", false))`(tips.tsx:18)。

#### 7.2.6 目录信任(trust)

**调研结论:opencode 当前没有 trust prompt 概念**。这是与 claude-code / cursor 等工具最大的差异 —— opencode 默认信任当前工作目录(通过 `ConfigPaths.directories(ctx.directory, ctx.worktree)` 直接读 .opencode 配置),不做"陌生仓库警告"。用户可通过 `Flag.OPENCODE_DISABLE_PROJECT_CONFIG`(paths.ts:27-33)完全禁用项目级配置。

### 7.3 设计巧妙点

- **`system` 自动派生主题**:不是每个用户都会配置主题;从 terminal 颜色自动生成,确保开箱即用。
- **链式引用 + 循环检测**:复杂主题可以用 `$defs` 共享色板,且不会因循环引用崩溃。
- **migrateTuiConfig 旧兼容**:老 `opencode.json` 时代的主题/键位迁移到 `tui.json` —— 升级用户体验。
- **`Flag.OPENCODE_DISABLE_PROJECT_CONFIG` 兜底**:企业场景一键禁用所有项目级配置,只信全局。

### 7.4 laew gap 表(D7)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1543 | 无多主题系统(30+ 内置 + system 派生) | P1 | 自研 theme JSON + Resolver |
| L1544 | 无 home tips 轮播(Onboarding 入口) | P2 | — |
| (未占) | 无目录 trust 提示(默认全信任) | n/a | — |

> opencode **未实现目录信任**,laew 不需要借鉴 —— laew 应在 prompt 注入(CLAUDE.md 五级链发现)层面处理即可。

---

## D8 会话导出 / 状态线 / 实时成本

### 8.1 源码定位

| 路径 | 行数 | 角色 |
|---|---|---|
| `packages/opencode/src/share/session.ts` | 58 | `SessionShare.Service`(share / unshare / create 含 autoShare fork) |
| `packages/opencode/src/share/share-next.ts` | 371 | `ShareNext` HTTP 客户端 + 增量同步队列 |
| `packages/opencode/src/share/sql.ts` | — | `SessionShareTable` SQLite 表 |
| `packages/opencode/src/cli/cmd/export.ts` | 292 | `export [sessionID]` CLI + sanitize 脱敏 |
| `packages/opencode/src/session/session.ts:355-405` | 50 | cost 计算函数(Decimal + tier + cache) |
| `packages/opencode/src/session/session.ts:231-233` | 3 | `cost: optional(Schema.Finite)` schema |
| `packages/opencode/src/cli/cmd/run/footer.view.tsx` | 900+ | 状态线 footer(width 自适应 + mode + activity + model) |
| `packages/opencode/src/cli/cmd/run/footer.width.ts` | 28 | width 80/66/120/150 四档断点 |
| `packages/tui/src/routes/session/index.tsx:467-505` | 38 | `session.share` slash(consent + toast) |
| `packages/tui/src/routes/session/index.tsx:586-609` | 23 | `session.unshare` slash |
| `packages/tui/src/routes/session/subagent-footer.tsx:33-55` | 22 | subagent 实时 usage + cost 显示 |
| `packages/tui/src/component/dialog-export-options.tsx` | 7634 bytes | 导出选项 Dialog |
| `packages/opencode/src/cli/cmd/stats.ts` | — | `stats` 子命令(累计统计) |

### 8.2 机制剖析

#### 8.2.1 SessionShare 三层架构

```
SessionShare.Service(业务层)
  ├─ create() ─── Session.create + (autoShare fork) ──┐
  ├─ share() ─── ShareNext.create ──┐                  │
  └─ unshare() ── ShareNext.remove ─┤                  │
                                    │                  │
ShareNext.Service(网络层)           ▼                  ▼
  ├─ HTTP POST /api/shares (or legacy /api/share)   SessionShareTable
  ├─ EventV2 watcher ──► session/message/part/diff 同步推送
  └─ 1s 批 flush + 双 base URL(legacy "opncd.ai" / console)

SessionShareTable (SQLite) ──── id / secret / url
```

`share-next.ts:151-203` 实现 **EventV2 → Share 队列** 联动:

```ts
yield* watch(Session.Event.Updated, (data) =>
  Effect.gen(function* () {
    const info = data.info
    yield* sync(info.id, [{ type: "session", data: structuredClone(info) as SDK.Session }])
  }),
)
yield* watch(MessageV2.Event.Updated, (data) => {
  if (info.role !== "user") return
  yield* provider.getModel(info.model.providerID, info.model.modelID)
  yield* sync(info.sessionID, [{ type: "model", data: [model] })
}),
yield* watch(MessageV2.Event.PartUpdated, (data) =>
  sync(data.part.sessionID, [{ type: "part", data: structuredClone(data.part) as SDK.Part }]),
),
yield* watch(Session.Event.Diff, (data) =>
  sync(data.sessionID, [{ type: "session_diff", data: structuredClone(data.diff) as SDK.SnapshotFileDiff[] }]),
),
```

要点:
- **`structuredClone` 强制深拷贝**:避免 Effect Stream 闭包共享同一引用导致脏数据。
- **`disabled` 早返回**:`process.env.OPENCODE_DISABLE_SHARE === "true" || "1"` 整体关闭(share-next.ts:23)。
- **1 秒批 flush**:每次更新不立即 POST,而是入 `state.queue`,`Effect.delay(1000)` 后统一推送(L141-145)。

#### 8.2.2 双 base URL + 鉴权切换

`share-next.ts:206-222` 根据账户状态自动切换:

```ts
const request = Effect.fn("ShareNext.request")(function* () {
  const headers: Record<string, string> = {}
  const active = yield* account.active()
  if (Option.isNone(active) || !active.value.active_org_id) {
    const baseUrl = (yield* cfg.get()).enterprise?.url ?? "https://opncd.ai"  // ← 旧 base
    return { headers, api: legacyApi, baseUrl } satisfies Req      // ← /api/share
  }
  const token = yield* account.token(active.value.id)
  if (Option.isNone(token)) throw new Error("No active account token available for sharing")
  headers.authorization = `Bearer ${token.value}`
  headers["x-org-id"] = active.value.active_org_id
  return { headers, api: consoleApi, baseUrl: active.value.url } satisfies Req   // ← /api/shares
})
```

未登录走 legacy `opncd.ai`,登录 enterprise console 走 `/api/shares` + Bearer + `x-org-id` 双 header。

#### 8.2.3 Share UI Flow

`session/index.tsx:465-505`:

1. **已分享**:`session()?.share?.url` 存在 → "Copy share link" → 复制 URL 到剪贴板 + toast。
2. **首次分享**:第一次 → `DialogConfirm` 二次确认("Are you sure you want to share it?")。
3. **持久 consent**:`kv.set("share_consent", true)` —— 一次确认后不再问。
4. **复制成功 toast**:"Share URL copied to clipboard!"

`session.unshare`(L586-609)对称:`enabled: !!session()?.share?.url`,按 SDK 调 `unshare`,成功 toast / 失败 toast。

#### 8.2.4 Export CLI + 脱敏

`cli/cmd/export.ts` 是 JSON 全量导出,`--sanitize` 开关触发 `sanitize()` 函数:
- 所有文本用 `redact("kind", id, value)` → `[redacted:kind:id]` 替换。
- 包含 file path / file symbol / file url / text / reasoning / subtask-prompt / tool-input 等 14+ 种 kind。
- `span()` 对 `FilePartSource.text` 做整体 redact,`diff()` 对 patch 文件名 redact,`source()` 递归处理 symbol / resource / file 三种 source。

无 markdown 导出路径(只有 JSON);Markdown 导出经由 `<leader>x`(session_export keybind)调 `dialog-export-options.tsx`。

#### 8.2.5 实时成本计算

`session.ts:355-405`:

```ts
const tokens = {
  total,
  input: adjustedInputTokens,                                    // 减去 cache.read/write 的纯 input
  output: safe(outputTokens - reasoningTokens),                 // 纯 output 不含 reasoning
  reasoning: reasoningTokens,
  cache: { write: cacheWriteInputTokens, read: cacheReadInputTokens },
}
const costInfo =
  input.model.cost?.tiers?.filter((item) => item.tier.type === "context" && contextTokens > item.tier.size)
    .sort((a, b) => b.tier.size - a.tier.size)[0] ??               // 按 context 大小匹配 tier
  (input.model.cost?.experimentalOver200K && contextTokens > 200_000
    ? input.model.cost.experimentalOver200K                          // 200K 溢价
    : input.model.cost)
return {
  cost: ...
    new Decimal(tokens.input).mul(finite(costInfo?.input ?? 0)).div(1_000_000)   // 每 1M token 单价
   .add(new Decimal(tokens.output).mul(finite(costInfo?.output ?? 0)).div(1_000_000))
   .add(new Decimal(tokens.cache.read).mul(finite(costInfo?.cache?.read ?? 0)).div(1_000_000))
   .add(new Decimal(tokens.cache.write).mul(finite(costInfo?.cache?.write ?? 0)).div(1_000_000))
   .add(new Decimal(tokens.reasoning).mul(finite(costInfo?.output ?? 0)).div(1_000_000))   // TODO: 临时按 output 同价
   .toNumber(),
  tokens,
}
```

要点:
- **Decimal 防浮点**:`decimal.js` 库避免 0.1+0.2 精度问题。
- **AI SDK v6 input 修正**:Anthropic/Bedrock 旧版 inputTokens 不含 cache,新版已统一包含,故显式 `safe(inputTokens - cacheReadInputTokens - cacheWriteInputTokens)` 修正(L361-364)。
- **reasoning 暂按 output 同价**:`TODO: update models.dev to have better pricing model, for now: charge reasoning tokens at the same rate as output tokens`(L398-399)。
- **context tier 溢价**:`experimentalOver200K` 单独定价。
- **Copilot 特殊**:`metadata?.copilot?.totalNanoAiu` 走完全不同的成本公式(L387-391)。

#### 8.2.6 Footer 状态线(宽度自适应)

`run/footer.view.tsx:1-900+` 是 single-pane / `-p` 模式的 footer,**与 TUI 交互模式的 footer 是两个独立实现**。前者由 `footer.width.ts` 的 4 档断点驱动:

| 宽度 | 断点 | 显示 |
|---|---|---|
| < 66 | (hidden) | 不显示 commandHint / contextHints |
| 66-79 | narrow | 仅 commandHint |
| 80-119 | compact | activityMeta + contextHints + commandHint,limit=1 |
| 120-149 | compact+model | + model 显示,contextHint limit=2 |
| ≥ 150 | spacious | + contextHint limit=undefined(全部) |

```ts
// footer.width.ts:3-27
const FOOTER_WIDTH_BREAKPOINTS = { compact: 80, commandHint: 66, model: 120, spacious: 150 } as const
export function footerWidthPolicy(width: number) {
  const compact = width >= FOOTER_WIDTH_BREAKPOINTS.compact
  const model = width >= FOOTER_WIDTH_BREAKPOINTS.model
  const spacious = width >= FOOTER_WIDTH_BREAKPOINTS.spacious
  return {
    dialog: { narrow: !compact },
    statusline: {
      showActivityMeta: compact,
      showCommandHint: width >= FOOTER_WIDTH_BREAKPOINTS.commandHint,
      showContextHints: compact,
      contextHintLimit: !compact ? 0 : spacious ? undefined : model ? 2 : 1,
      showModel: model,
    },
  }
}
```

实际渲染(footer.view.tsx:817-890):
- 左侧 mode badge(`statusAccent` 背景,`bold`)
- 中间 statusText(busy/interrupt/exit 状态色)
- 右侧 activityMeta + model(provider/variant) + contextHints(background/queued/subagents)
- 整行 `statuslineBackground = theme().status` 着色

#### 8.2.7 TUI 内 Footer(交互模式)

`routes/session/footer.tsx`(91 行)极简:
- 左:directory 路径(`abbreviateHome` 简化 `~/...`)
- 右:LSP count(绿/灰圆点) + MCP count + `/status` 入口提示

`subagent-footer.tsx:33-55` 提供 **实时 usage + cost**(从 `sync.data.message[sessionID]` 找最后一条 `AssistantMessage`,累加 `tokens.input + output + reasoning + cache.read + cache.write`,换算 context 百分比 + cost):

```tsx
const usage = createMemo(() => {
  const msg = messages()
  const last = msg.findLast((item): item is AssistantMessage => item.role === "assistant" && item.tokens.output > 0)
  if (!last) return
  const tokens = last.tokens.input + last.tokens.output + last.tokens.reasoning + last.tokens.cache.read + last.tokens.cache.cache.write
  if (tokens <= 0) return
  const model = sync.data.provider.find((item) => item.id === last.providerID)?.models[last.modelID]
  const pct = model?.limit.context ? `${Math.round((tokens / model.limit.context) * 100)}%` : undefined
  const cost = session()?.cost ?? 0
  // ...
  return {
    context: pct ? `${Locale.number(tokens)} (${pct})` : Locale.number(tokens),
    cost: cost > 0 ? money.format(cost) : undefined,
  }
})
```

> laew 当前 **完全没有实时 cost 显示**(只有单次任务的预估,在 prompt 前)。

### 8.3 设计巧妙点

- **EventV2 自动同步**:每个 `session.updated` / `message.updated` / `part.updated` / `session_diff` 都会推到 ShareNext,实现"实时"链接(用户打开链接能跟着对话进展)。
- **structuredClone 强制**:深拷贝防止 Effect 闭包捕获导致脏数据 —— 是个细节但 opencode 在多处都明确写。
- **consent 一次性**:第一次 share 弹确认后写 KV,后续不再问,降低摩擦。
- **宽度自适应**:`statusline.contextHintLimit = undefined` / `2` / `1` / `0` 四档精确控制 —— 这是真正的"responsive footer"。
- **Cost Decimal 精算**:避免浮点误差 + tier 模型 + cache 分价 + reasoning 同价 + Copilot 特殊公式,共 4 路径。
- **AI SDK v6 input 修正**(L361-364):这是 **2025-2026 跨 SDK 升级期很现实的补丁** —— 新 SDK 把 cached token 计入 input 但旧 SDK 没计,如果不修正会双重收费。

### 8.4 laew gap 表(D8)

| 编号 | 描述 | P0/P1/P2 | 推荐 Rust crate |
|---|---|---|---|
| L1545 | 无 session share(增量同步队列 + EventV2 watcher) | P0 | `reqwest` + 自研 queue |
| (未占) | 无 export CLI + sanitize 脱敏(14 种 kind) | n/a | 自研 + `regex` |
| (未占) | 无 cost 实时显示(TUI footer / subagent footer) | n/a | `rust_decimal` |
| (未占) | 无宽度自适应 statusline(80/120/150 四档断点) | n/a | `ratatui` width API |
| (未占) | 无 share consent KV 持久化 | n/a | `directories` crate |
| (未占) | 无 share.opencode.ai + /api/shares 双 base URL | n/a | — |

---

## laew gap 汇总 (L1516-L1545)

| 编号 | 维度 | 描述 | 级别 | 推荐 crate |
|---|---|---|---|---|
| L1516 | D1 | 无 @ 提及系统 | P0 | `nucleo-matcher` |
| L1517 | D1 | 无 FilePart/AgentPart 多模态序列化 | P0 | `serde` |
| L1518 | D1 | 无 extmark 虚拟文本 | P1 | tui-textarea |
| L1519 | D1 | 无行级 range 语法 | P1 | 手写 parser |
| L1520 | D1 | 无 IDE JSON-RPC 桥 | P2 | `jsonrpsee` |
| L1521 | D2 | 无自定义斜杠命令 | P0 | frontmatter parser |
| L1522 | D2 | 无命令 frontmatter 校验 | P1 | `schemars` |
| L1523 | D2 | 无 `$ARGUMENTS` 占位符抽取 | P1 | regex |
| L1524 | D2 | 无命令合并优先级 | P2 | — |
| L1525 | D2 | 无智能 hint | P2 | — |
| L1526 | D3 | 无 session revert 栈 | P0 | 自研 + SQLite |
| L1527 | D3 | 无 session fork(parentID 重映射) | P0 | 树形序列化 |
| L1528 | D3 | 无 timeline dialog | P1 | `ratatui` List |
| L1529 | D3 | 无 undo/redo slash | P1 | — |
| L1530 | D3 | 无 revert diff 渲染 | P2 | `similar` |
| L1531 | D4 | 无 workspace 移动外部修改检测 | P1 | `git2` |
| L1532 | D4 | 无 LSP 诊断推送 TUI | P1 | `lsp-types` |
| L1533 | D4 | 无运行时 fs.watch | P2 | `notify` |
| L1534 | D5 | 无 Shiki 语法高亮 | P1 | `syntect` |
| L1535 | D5 | 无 Markdown 渲染组件 | P1 | `comrak` |
| L1536 | D5 | 无 TUI diff viewer | P1 | `similar` |
| L1537 | D5 | 无图片/PDF 虚拟文本 | P1 | base64 |
| L1538 | D5 | 无 KaTeX 数学公式 | P2 | WebView |
| L1539 | D6 | 无 IME 双 setTimeout flush | P0 | tui-textarea |
| L1540 | D6 | 无粘贴 5 档处理 | P1 | `arboard` |
| L1541 | D6 | 无 Tab 渐进目录展开 | P2 | — |
| L1542 | D6 | 无 Prompt stash | P2 | `directories` |
| L1543 | D7 | 无多主题系统 | P1 | 自研 |
| L1544 | D7 | 无 home tips 轮播 | P2 | — |
| L1545 | D8 | 无 session share(增量同步) | P0 | `reqwest` |

### 维度覆盖统计

| 维度 | opencode 实现 | laew gap 编号 | 优先级建议 |
|---|---|---|---|
| D1 @ 提及系统 | 完整(file/agent/resource/IDE 桥 + frecency + extmark) | L1516-L1520 | P0 优先 L1516-L1517 |
| D2 自定义命令 | 完整(4 源合并 + frontmatter 强校验 + $ARGUMENTS) | L1521-L1525 | P0 优先 L1521 |
| D3 Rewind/Fork | 完整(revert 栈 + fork parentID 重映射 + undo/redo) | L1526-L1530 | P0 优先 L1526-L1527 |
| D4 文件监视 | 仅被动(workspace 移动时检测 + LSP 接入但诊断不显示) | L1531-L1533 | P1 优先 L1531 |
| D5 富文本渲染 | Web:shiki+marked+KaTeX;TUI:自建 diff viewer | L1534-L1538 | P1 优先 L1534-L1536 |
| D6 输入体验 | 完整(IME flush + 5 档粘贴 + Tab 目录展开 + stash) | L1539-L1542 | P0 优先 L1539 |
| D7 Onboarding/Theme | 主题完整(30+ 内置 + system 派生 + 8 级配置);trust 未实现 | L1543-L1544 | P1 优先 L1543 |
| D8 状态线/成本/分享 | 完整(share-next + EventV2 实时 + cost Decimal + 4 档 footer) | L1545 | P0 优先 L1545 |

### P0 必做(7 项):L1516 / L1517 / L1521 / L1526 / L1527 / L1539 / L1545
### P1 重要(14 项):L1518 / L1519 / L1522 / L1523 / L1528 / L1529 / L1531 / L1532 / L1534 / L1535 / L1536 / L1537 / L1540 / L1543
### P2 进阶(9 项):L1520 / L1524 / L1525 / L1530 / L1533 / L1538 / L1541 / L1542 / L1544

---

## 结语

opencode 在「用户交互体验层」的设计有以下关键启示值得 laew 借鉴:

1. **Part 化消息模型**:FilePart / AgentPart / TextPart 是统一的"prompt 附加物"抽象,extmark 虚拟文本桥接可见/不可见,这是 laew 当前完全缺失的。
2. **EventV2 watcher 自动同步**:share / sync / record 三个外部系统都通过订阅同一组 Event 事件自动接收 —— 同一模式可复用到 laew 的 ProjectContext/SessionContext/Compact Agent。
3. **AI SDK v6 修正补丁**(L361-364):这是 2025-2026 跨 SDK 升级期非常现实的补丁 —— laew 任何 inputTokens 计费都需要这种小心。
4. **宽度自适应 statusline**:4 档断点 + responsive contextHintLimit,比 laew 当前固定 footer 体验好得多。
5. **8 级配置发现链**:`mergeDeep` 字段级合并 + `OPENCODE_DISABLE_PROJECT_CONFIG` 企业兜底,可作为 laew 多环境配置的参考。
6. **30+ 内置主题**:虽非 P0,但显著降低新用户门槛;system 派生主题开箱即用。

> 全部源码引用自 `/usr/local/LsmGitOpenSource/opencode/`,绝对路径已在 D1-D8 各节给出。
