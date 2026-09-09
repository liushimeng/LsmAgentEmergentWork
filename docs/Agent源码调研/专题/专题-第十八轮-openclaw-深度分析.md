# 专题-第十八轮-openclaw-深度分析：用户交互体验层

> **目标项目**：openclaw（TypeScript/Bun，Gateway/Harness/Adapter 三层契约 + 162 extensions + 多端 native）
> **本轮主题**：用户交互体验层（User Interaction Experience Layer）
> **分析发起方**：laew（Rust Agent CLI），为 18 轮 8 维度专项深挖
> **gap 编号区间**：L1486-L1515（30 个槽位，按需分配）
> **日期**：2026-09-09

---

## 元信息

| 项 | 值 |
|---|---|
| 工程规模 | 7798 行主文档 + 多端 native 应用 + 162 extensions + 12 个 packages |
| 本轮主题 | 用户交互体验层（**D1-D8 八维度**） |
| 主要源码根 | `src/tui/`（TUI 引擎） + `src/auto-reply/`（命令注册/UsageBar/导出） + `src/gateway/server-methods/`（分享/回退/分支 RPC） + `src/wizard/`（onboarding） + `ui/src/pages/chat/`（Web UI composer + 回退/分支） + `src/agents/sessions/prompt-templates.ts`（prompt 模板） + `src/skills/runtime/refresh.ts`（chokidar 监听） + `src/gateway/config-reload.ts`（配置热加载） |
| 前 17 轮覆盖 | Gateway/Harness/Adapter、162 extensions、双向 MCP、Lane、Workshop、Custodian Skills、多端部署、Taxonomy、Security、5 层纵深、Failover 16 原因、SSRF、14 种 prompt 注入检测、Command Queue、session-lifecycle、自研 CDP/Playwright 等（**严禁重复**） |
| 本轮核心新增 | **Mention 系统 + 人类提及安全 + Slash 命令注册 + Prompt 模板发现 + 用户级回退/分支/编辑 + Chokidar 工作区感知 + Diff/Markdown 渲染 + 输入历史 + 粘贴/图片/键位 + Onboarding wizard + 风险确认 + 主题 12 套 + Telemetry opt-out + 会话导出 HTML/JSONL + 公网分享链接 AES-256-GCM + UsageBar 模板 DSL** |

---

## D1 @提及系统（@mention system）

### D1.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `ui/src/pages/chat/components/chat-composer-mention-menu.ts` | 1-409 | **人类 Mention Menu 主类 + 触发检测 + 选中替换** |
| `ui/src/lib/chat/human-mentions.ts` | — | `MAX_HUMAN_MENTIONS` 上限 + `updateHumanMentions` 重写 |
| `ui/src/pages/chat/components/chat-composer-mention-menu.ts` | 41-67 | `findMentionTarget()` 光标前 mention 检测 |
| 同上 | 178-194 | debounce 150ms + LRU 16 条查询缓存 |
| 同上 | 235-263 | `select()` 替换文本 + 排序 mentions（防止重叠） |
| `packages/gateway-protocol/src/schema/human-mentions.ts` | — | wire 协议 `UsersMentionableParams` / `Result` |

### D1.2 机制剖析

```ts
// chat-composer-mention-menu.ts:41-67
function findMentionTarget(value: string, caret: number): MentionTarget | null {
  if (value.trimStart().startsWith("/")) return null;            // 斜杠命令优先
  const beforeCaret = value.slice(0, caret);
  const line = beforeCaret.slice(beforeCaret.lastIndexOf("\n") + 1);
  // Code fence 与 blockquote 内禁止触发人名选择
  if (/^\s*>/u.test(line) ||
      (beforeCaret.match(/```/gu)?.length ?? 0) % 2 !== 0 ||
      (line.match(/`/gu)?.length ?? 0) % 2 !== 0) return null;
  const match = /(?:^|[\s([{])@([\p{L}\p{N}\p{M}_.-]{0,64})$/u.exec(beforeCaret);
  if (!match) return null;
  const query = match[1] ?? "";
  const start = caret - query.length - 1;
  let end = caret;
  while (end < value.length && /[\p{L}\p{N}\p{M}_.-]/u.test(value[end] ?? "")) end += 1;
  return { start, end, query };
}
```

**核心设计点**：
1. **触发边界**：纯空格分隔 + Unicode `\p{L}\p{N}\p{M}_.-` 字符类，**最长 64 字符** query
2. **斜杠命令优先**：`value.trimStart().startsWith("/")` 直接排除（mention 不抢 slash）
3. **Code fence 守卫**：未配对 ``` 或行内 ` 都禁止触发（避免误选代码中的人名）
4. **Debounce + LRU**：150ms + 16 槽位客户端缓存（与服务端 presence snapshot 解耦）
5. **Mention 排序**：`toSorted((a, b) => a.start - b.start)` 保持 offset 升序，避免后续 edit 算错
6. **MAX_HUMAN_MENTIONS 上限**：服务端 schema 强约束，避免一个 user 提及 1000 人

### D1.3 服务端协议

`UsersMentionableParams` 支持 `query` + `ownerKey` 过滤；返回 `UsersMentionableResult { users[], truncated }`。TUI 侧（`src/tui/tui-autocomplete.ts`）通过 `CombinedAutocompleteProvider` 复用同一份触发逻辑，但产物是 TUI `<SelectList>` 而非 Web DOM `<listbox>`。

### D1.4 设计巧妙点

1. **locale 隔离过滤**：`canFilterMentionText()` 仅对 ASCII 且不含大写 I 的查询本地裁剪结果（`truncated=false` 前提下），其余全部强制回流服务端——避免客户端猜测大写转换出错
2. **复用而不复制**：mention 触发器与 slash 触发器在 `value.trimStart().startsWith("/")` 互斥，避免 UI 层出现「输 @ 又触发 slash 补全」的奇怪状态
3. **离线 graceful**：当 `search.kind === "error"` 时渲染 `t("chat.mentions.unavailable")` 而不是阻塞发送
4. **range selection 兼容**：`target.end` 会被扩展到 query 边界外，**用户输入完 query 后若光标前还有字符（如同伴后续字符），替换不会破坏上下文**

### D1.5 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| L1486 | 无 @提及系统（@agent / @file / @模式） | P0 | `regex` + `tui-input` 扩展 menu hook |
| L1487 | 触发检测未做 code fence / blockquote 守卫 | P1 | 复用 `pulldown-cmark` 解析状态机 |
| L1488 | 无客户端 LRU + debounce 缓存 | P2 | `lru` + `tokio::time::sleep` |

---

## D2 自定义斜杠命令与 Prompt 模板

### D2.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `src/agents/sessions/prompt-templates.ts` | 全文 226 行 | **Prompt 模板三源发现 + 加载 + 展开** |
| 同上 | 14-25 | `loadTemplateFromFile()` frontmatter 解析 + 60 字符摘要 |
| 同上 | 90-104 | `resolvePromptPath()` `~` 展开 + 相对路径解析 |
| 同上 | 119-218 | 三源合并：全局 + 项目 + 显式 `promptPaths` |
| 同上 | 222-226 | `expandPromptTemplate()` 正则匹配 `/name args` |
| `src/tui/commands.ts` | 87-181 | **TUI slash 命令表**（22 个内置命令 + 别名） |
| 同上 | 100-128 | `createLevelCompletion` 动态选项补全工厂 |
| `src/auto-reply/commands-registry.shared.ts` | 285-298 | `/export-session` `/export-trajectory` 注册 |
| `src/auto-reply/commands-args.ts` | — | `parseCommandArgs` / `substituteArgs` `$1..$@` |
| `ui/src/pages/chat/components/chat-composer-inline-slash.ts` | 全文 230 行 | **行内 slash 实时替换 + 多轮参数解析** |
| `packages/agent-core/src/harness/prompt-template-arguments.js` | — | `$1` `$2` `$@` `$*` 占位符替换 |

### D2.2 机制剖析

```ts
// prompt-templates.ts:119-218
export function loadPromptTemplates(options: LoadPromptTemplatesOptions): PromptTemplate[] {
  const templates: PromptTemplate[] = [];
  const globalPromptsDir = options.agentDir ? join(options.agentDir, "prompts") : resolvedAgentDir;
  const projectPromptsDir = resolve(resolvedCwd, CONFIG_DIR_NAME, "prompts");
  // 三源顺序：全局 → 项目 → 显式 promptPaths
  if (includeDefaults) {
    templates.push(...loadTemplatesFromDir(globalPromptsDir, getSourceInfo));
    templates.push(...loadTemplatesFromDir(projectPromptsDir, getSourceInfo));
  }
  for (const rawPath of promptPaths) { /* 显式 */ }
  return templates;
}

// expandPromptTemplate(text, templates)
const match = text.match(/^\/([^\s]+)(?:\s+([\s\S]*))?$/);
const templateName = match[1], argsString = match[2] ?? "";
const template = templates.find((t) => t.name === templateName);
if (template) {
  const args = parseCommandArgs(argsString);
  return substituteArgs(template.content, args);
}
return text;  // 不匹配 → 原文回退，不报错
```

**核心设计点**：
1. **三源优先级**：全局（`agentDir/prompts/`）→ 项目（`{cwd}/{CONFIG_DIR_NAME}/prompts/`）→ 显式 `promptPaths`，**全部 `.md` 非递归扫描**
2. **frontmatter 优先**：description 字段取自 YAML frontmatter，缺失则用首行 60 字符截断
3. **`argument-hint` 字段**：用户写 `argument-hint: <path>` 显示在 slash 菜单补全提示中
4. **显式路径别名**：`/template foo $1 $2` 把 `$1` 替换为第一个参数
5. **路径沙箱**：`isPathInside(globalPromptsDir, resolvedPath)` 区分 source（`local` / `project` / 显式）→ 让权限/审计可追溯

### D2.3 TUI slash 命令表

```ts
// commands.ts:140-181（节选）
const TUI_COMMAND_ROWS = [
  ["help", "Show slash command help", "/help"],
  ["gateway-status", "Show gateway status summary", ["/gateway-status","/gwstatus"]],
  ["usage", "Toggle per-response usage line or show cost summary", "/usage <off|tokens|full|cost|reset|inherit|clear|default>"],
  ["goal", undefined, "/goal <objective> | /goal [status] | /goal start <objective> | /goal edit <objective> | /goal pause|resume|complete|block|clear"],
  ["btw", undefined, "/btw <side question>"],
  ["new", "Spawn a new isolated session", "/new or /reset"],
  ["exit", "Exit the TUI", "/exit", undefined, { aliases: [{ name: "quit" }] }],
  // ...
];
```

`formatTuiLevelCommandUsage("verbose")` 等工厂保证 help 文本与实际指令补全自动同步。

### D2.4 行内 slash 实时替换（Web UI）

```ts
// chat-composer-inline-slash.ts:117-147
export function commitInlineSlashSelection(
  replacement: string, state, host
): boolean {
  // 选中 `/foo` → 在 textarea value 中替换为 `replacement`，
  // 自动加空格分隔符，焦点回 textarea，光标精确定位
  const after = current.slice(completion.end);
  const separator = after.length === 0 || !/^\s/u.test(after) ? " " : "";
  const next = `${current.slice(0, completion.start)}${replacement}${separator}${after}`;
  commitDraftWithCaret(host, next, completion.start + replacement.length + separator.length);
}
```

`findDirectInlineSlashArgumentInvocation()` 检测 `/foo bar baz`（命令后跟参数）且**首词匹配已注册命令**时，把 `bar baz` 解析为 inline args，支持 `allowsInlineMultiWordArgs` 多词参数命令。

### D2.5 设计巧妙点

1. **TUI help 与实际 completions 单源**：`TUI_COMMAND_ROWS` 是数据，`TUI_COMMAND_DESCRIPTORS` 是派生态，避免文档漂移
2. **frontmatter 优雅降级**：缺 description 自动用首行 60 字符（`truncateUtf16Safe` 防 emoji 切半）
3. **匹配失败回退**：`expandPromptTemplate` 不匹配 `/name` 时**返回原文**而不是抛错，避免把用户正常消息 `/something` 误判
4. **shell completion 自动接管**：`src/wizard/setup.completion.ts` 检测 shell profile 用了 slow dynamic pattern 时自动升级 cached 版本（`completion.runtime.js` 子模块）
5. **SourceInfo 跟踪**：每个模板都带 `sourceInfo`，允许后续权限模块判断「这是用户写的还是项目自带的」

### D2.6 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| L1489 | 无 prompt 模板发现机制（无 frontmatter 解析） | P0 | `serde_yaml` + walkdir |
| L1490 | 无 `/` 斜杠命令注册中心（仅硬编码 9 个） | P0 | `clap_complete` + 命令注册表 |
| L1491 | 无 `$1 $@ $*` 占位符替换 | P1 | 自实现 30 行 tokenize |
| L1492 | 无 inline slash 实时替换 | P2 | `tui-input` Menu hook |
| L1493 | 无 shell completion 安装向导 | P1 | `clap_complete` 生成 + `shell-words` |

---

## D3 对话 Rewind / 分支（用户级）

### D3.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `src/gateway/server-methods/sessions-rewind.ts` | 全文 700+ 行 | **Gateway 侧 rewind/fork/switch 三个 RPC** |
| 同上 | 110-130 | `mutateSessionAtMessage` action 分发 |
| 同上 | 75-93 | `EXTERNAL_CONVERSATION_ERROR` 上游 linked session fail-closed |
| 同上 | 209-273 | 生命周期幂等校验 + active run 阻塞 |
| `src/config/sessions/session-accessor.ts` | — | `forkSessionAtMessage` / `rewindSessionToMessage` / `switchSessionBranch` 实现 |
| `ui/src/pages/chat/chat-history-actions.ts` | 222-300 | **`rewindChatHistory()` UI 入口** |
| 同上 | 320-360 | `switchChatHistoryBranch()` 分支切换 |
| `packages/gateway-protocol/src/schema/protocol-schema-fragment-*.ts` | — | `sessions.rewind` / `sessions.fork` / `sessions.branches.switch` schema |
| `src/tui/tui-command-handlers.ts` | — | TUI `/rewind` 命令入口 |

### D3.2 机制剖析

```ts
// sessions-rewind.ts:209-273（节选）
async function mutateSessionAtMessage(options, action: MessageCutAction) {
  const commitGuard = () => {
    sessionMutationCommitGuard?.();
    sessionMutationAuthorization?.assertCurrent();
  };
  // 1. 解析 entryId（switch 用 leafEntryId，其他用 entryId）
  const entryId = action === "switch"
    ? typeof params.leafEntryId === "string" ? params.leafEntryId.trim() : ""
    : typeof params.entryId === "string" ? params.entryId.trim() : "";
  // 2. 解析 target agent + 加载 entry
  const initial = loadAccessorSessionEntryForGatewayTarget({...});
  if (!initial.entry?.sessionId) {
    respond(false, undefined, errorShape(INVALID_REQUEST, `session not found: ${sessionKey}`));
    return;
  }
  // 3. 初始化中拒绝
  if (rejectInitializing(initial.entry.initializationPending)) return;
  // 4. fork 才允许上游 linked session
  if (initialUpstreamLink && action !== "fork") {
    respond(false, undefined, errorShape(INVALID_REQUEST, EXTERNAL_CONVERSATION_ERROR));
    return;
  }
  // 5. 生命周期校验 + active run 阻塞
  await runExclusiveSessionLifecycleMutation({
    scope: initial.storePath,
    identities: [sessionKey, canonicalKey, storeKey, sessionId, lifecycleRevision],
    prepare: async () => {
      targetStillCurrent = current.entry?.sessionId === initialSessionId && ...;
      blockedByActiveRun = isCompetingSessionWorkAdmissionActive(...) || hasInferenceForSession(...);
    }
  });
  // 6. 执行
  const result = action === "rewind" ? await rewindSessionToMessage(...)
                  : action === "fork"  ? await forkSessionAtMessage(...)
                  : await switchSessionBranch(...);
}
```

**核心设计点**：
1. **三 action 统一编排**：rewind / fork / switch 共用 `mutateSessionAtMessage`，仅末尾 dispatcher 不同
2. **生命周期 revision 校验**：取 entry 时记 `lifecycleRevision`，commit 前再读一次校验（CAS 风格防并发漂移）
3. **上游 linked session fail-closed**：fork 可新建分支、rewind/switch 必须失败（避免在外部会话上乱改）
4. **Active run 阻塞**：`isCompetingSessionWorkAdmissionActive()` + worker inference 控制，**不让 cut 操作打断进行中的工作**
5. **Fork 创作校验**：`authorizeGatewaySessionCreation` 与 `new`/`reset` 走同一路径，权限一致
6. **EDITOR_MEDIA_REF_LIMIT = 10**：用户编辑消息携带的图片引用超过 10 直接截断（防 corrupt transcript 转大媒体读）

### D3.3 UI 入口（Web）

```ts
// chat-history-actions.ts:222-300
export async function rewindChatHistory(state, entryId) {
  const composerSignature = readComposer();  // 捕获当前草稿签名
  const ownsComposer = captureChatComposerReplacement(state, sessionKey, agentParams.agentId);
  try {
    const result = await state.sessions.rewind(sessionKey, entryId, agentParams);
    // 1. 清空消息缓存
    clearChatMessagesFromCache(...);
    // 2. 重新加载历史 + 分支列表
    if (viewMatches()) {
      resetChatHistoryProjection(...);
      await Promise.all([loadChatHistory(state), loadChatBranches(state)]);
    }
    // 3. 把 rewind 处的 editor 文本写回 composer
    if (!connectionIsCurrent() || !ownsComposer() || readComposer() !== composerSignature) return null;
    persistChatComposerState(state, sessionKey, { draft: editorText, mentions: [], ... });
    state.handleChatDraftChange(editorText, []);
    return result;
  } catch (error) { ... }
}
```

**Composer 签名守卫**：`composerSignature` 在 RPC 期间若被用户改动（打字 / 删字符），**放弃回填 editor 文本**，避免覆盖用户新草稿。

### D3.4 Switch 分支（不创建新会话）

```ts
// chat-history-actions.ts:320-360
export async function switchChatHistoryBranch(state, leafEntryId) {
  const result = await state.sessions.switchBranch(sessionKey, leafEntryId, agentParams);
  // 清缓存 → 重投影 → 同时 reload history + branches
  await Promise.all([loadChatHistory(state), loadChatBranches(state)]);
  return viewIsCurrent();
}
```

**关键**：switchBranch 不创建新 session，**只更新 leafEntryId** 让 transcript 投影换源；UI 端 chat-history-branches 显示所有分支树。

### D3.5 设计巧妙点

1. **CAS lifecycle revision**：session 任何并发变更都会 bump `lifecycleRevision`，commit 前再读一次保证原子切
2. **Composer 签名捕获**：避免在网络往返中覆盖用户的并发编辑
3. **fork 才允许上游**：上游 linked session 真实存在（Claude Code / Codex 等），rewind 改它会污染外部，**fail-closed**
4. **媒体 ref 限速**：corrupt transcript 触发 rewind 时不会让攻击者构造 `/sessions.rewind` 带 100 个大图把存储读爆

### D3.6 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| L1494 | 无用户级对话回退（仅整体 session reset） | P0 | 扩展 `session_memory` 表 + entry 链表 |
| L1495 | 无对话分支（session 在某 entry 分叉为多线） | P1 | SQLite 增加 leaf_entry_id 字段 + UI selector |
| L1496 | 无「回退到第 N 条消息后恢复 editor 草稿」机制 | P1 | agent 循环记录 user_message ↔ entry 映射 |
| L1497 | 无 lifecycle revision CAS 守卫 | P0 | laew 需 entry-level optimistic lock |
| L1498 | 无上游链接会话 fail-closed | P2 | `data: external_session_link` flag |

---

## D4 文件监视与工作区感知（运行时）

### D4.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `src/skills/runtime/refresh.ts` | 全文 700+ 行 | **Skills 目录 chokidar 多 workspace 共享 watcher** |
| 同上 | 60-80 | `SkillsPathWatchState` + WatchTarget + 缓存表 |
| 同上 | 480-540 | `createSkillsPathWatcher()` chokidar.watch + raw event 处理 |
| 同上 | 248-256 | **`awaitWriteFinish: { stabilityThreshold: 250ms, pollInterval: 100 }`** 防抖 |
| 同上 | 104-138 | 三层 ignore：`.git` / `node_modules` / `dist` / `.venv` / `__pycache__` / `.mypy_cache` / `.pytest_cache` / `build` / `.cache` |
| 同上 | 514-540 | `raw` event + `waitForStableSkillFile()` 200ms 文件大小+mtime 稳态检测 |
| 同上 | 313-360 | 容量耗尽 fallback：EMFILE/ENOSPC → 自动降级 polling |
| `src/gateway/config-reload.ts` | 全文 1000+ 行 | **配置文件 chokidar 热重载** |
| 同上 | 47-66 | `WATCHER_RECREATE_MAX_RETRIES = 3` + 退避 500/2000/5000ms |
| `src/auto-reply/usage-bar/template.ts` | 100-160 | usage bar 模板文件 fs.watch 单文件级 watcher |

### D4.2 机制剖析

```ts
// refresh.ts:480-540（节选）
function createSkillsPathWatcher(target: WatchTarget): SkillsPathWatchState {
  const usePolling = resolveSkillsWatcherUsePolling();
  // chokidar 缺根回退只保留 basename → 看丢多层不存在父目录的创建
  const watcher = runInSkillsWatcherContext(() =>
    chokidar.watch(target.watchRoot, {
      ignoreInitial: true,
      followSymlinks: false,
      usePolling,
      depth: target.depth + path.relative(target.watchRoot, target.path).split(path.sep).filter(Boolean).length,
      awaitWriteFinish: { stabilityThreshold: SKILLS_WATCH_DEBOUNCE_MS, pollInterval: 100 },
      ignored: (watchPath, stats) =>
        shouldIgnoreSkillsWatchPath(watchPath, stats, usePolling) ||
        (!isPathInside(target.path, watchPath) && !isPathInside(watchPath, target.path)),
    })
  );
  // ...raw 事件 / ready / error / all 钩子
  watcher.on("error", (err) => {
    if (watcher.closed) return;
    const capacityCode = usePolling ? undefined : getFileWatchCapacityCode(err);
    if (capacityCode) {
      if (!nativeWatchCapacityFailed) {
        nativeWatchCapacityFailed = true;
        log.warn(`skills native watcher capacity exhausted (${capacityCode}); refreshing skills during agent preparation`);
        for (const active of pathWatchers.values()) void teardownSkillsPathWatcher(active);
      }
    }
  });
}
```

**核心设计点**：
1. **多 workspace 共享 watcher**：相同 watchRoot 多个 agent 复用**一个** chokidar handle（避免 fd 爆炸）
2. **三层 ignore**：
   - `.git` / `node_modules` / `dist` / `.venv` / `__pycache__` / `.mypy_cache` / `.pytest_cache` / `build` / `.cache`
   - chokidar 自带
   - `!isPathInside(target.path, ...)` 容器外一律不触发
3. **`awaitWriteFinish`**：250ms 稳定窗口 + 100ms poll，写入中变更不刷版本
4. **`raw` event 兜底**：跨多层不存在父目录创建时 chokidar 会丢事件；用 `waitForStableSkillFile`（size + mtime 双指标）补足
5. **EMFILE/ENOSPC 自愈**：`nativeWatchCapacityFailed` 单次全局降级 polling；不用每次 agent turn 都重试
6. **`MAX_SYMLINK_WATCH_TARGETS_PER_ROOT = 100`**：信任 symlink 数量上限（防 symlink 风暴）
7. **`SKILLS_WORKSPACE_WATCH_IDLE_TTL_MS = 60min`**：长期空闲的 workspace 自动清理 watcher 订阅
8. **`bumpSkillsSnapshotVersion()`**：不直接 reload skill，**bump 版本号**让下一次 agent turn 自然重新加载（解耦 listener 与 reload）

### D4.3 配置热加载

```ts
// config-reload.ts:58-66
const WATCHER_RECREATE_MAX_RETRIES = 3;
const WATCHER_RECREATE_BACKOFF_MS = [500, 2000, 5000] as const;
// 注释：inotify watches 耗尽 → 退避重试 → 失败后降级 polling
```

每个变更通过 `GatewayReloadPlan`（`config-reload-plan.ts`）分类：`reloadHooks` / `reloadChannels` / `reloadAuth` / `restartRequired`，分别走 hot-reload 或 restart 路径。

### D4.4 设计巧妙点

1. **watcher 不直接 reload，bump 版本**：所有 skill reload 是「lazy on next turn」，**避免 hot-reload 期间 race**
2. **trust 链式 symlink 验证**：`isTrustedSymlinkSkillTarget` 检查 source ∈ {managed, personal} 或 root 是 containment（防 symlink 跳出 worktree）
3. **`AsyncLocalStorage.snapshot()` 继承 startup context**：后续 reload 仍共享 startup 时 import 的 lifetime（不依赖触发 turn 的 context）
4. **`usePolling` 检测**：`CHOKIDAR_USEPOLLING=true` 显式 + 默认仅 `os400` 强制 polling

### D4.5 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| L1499 | 无工作区文件 chokidar 监视（项目文件变更进入 Agent） | P0 | `notify`（Rust chokidar 等价）+ debouncer crate |
| L1500 | 无 EMFILE/ENOSPC 自愈降级 polling | P1 | `notify` 内置 `PollWatcher` + inotify limit 探测 |
| L1501 | 无 `awaitWriteFinish` 写完稳定检测 | P1 | `notify-debouncer-mini` |
| L1502 | 无 symlink trust 链验证 | P1 | `std::fs::canonicalize` + 容器包含判断 |
| L1503 | 无共享 watcher + workspace idle TTL | P2 | `Arc<Mutex<HashMap<PathBuf, Watcher>>>` |

---

## D5 工具输出富文本内容渲染

### D5.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `src/tui/components/markdown-message.ts` | 全文 35 行 | **TUI markdown 容器包装** |
| 同上 | 20-25 | `MarkdownMessageComponent extends Container` 内部委托给 `HyperlinkMarkdown` |
| 同上 | 29-31 | `setText()` 原地更新（不重建组件） |
| `src/tui/components/hyperlink-markdown.ts` | — | OSC-8 hyperlink + markdown 行内解析 |
| `src/tui/tui-formatters.ts` | 90-160 | **sanitize 控制字符 + RTL 隔离 + binary 折叠** |
| 同上 | 14-23 | `RENDER_CONTROL_CHARS_RE`（保留 TAB/LF/CR） |
| 同上 | 73-89 | `sanitizeRenderableLine` 控制字符剥离 |
| 同上 | 47-55 | `isolateRtlLine` U+2067/U+2069 双向隔离 |
| `src/auto-reply/usage-bar/translator.ts` | 291-320 | **`renderUsageBar()` 模板 DSL 渲染** |
| `src/auto-reply/usage-bar/default-template.ts` | 全文 | **8 套 meter scale + 多 surface 模板** |
| `ui/src/components/markdown.ts` | — | Web UI Markdown 渲染器（基于 markdown-it） |
| `ui/src/components/markdown-code-blocks.ts` | — | 代码块语法高亮 |
| `src/gateway/control-ui-public-session-render.ts` | — | 公共 session 渲染（sanitize 优先） |

### D5.2 机制剖析

#### TUI Markdown 容器

```ts
// markdown-message.ts
export class MarkdownMessageComponent extends Container {
  private body: HyperlinkMarkdown;
  constructor(text: string, y: number, defaultTextStyle?, options?) {
    super();
    this.body = new HyperlinkMarkdown(text, 0, y, markdownTheme, defaultTextStyle, options);
    this.addChild(new Spacer(1));  // 前置空行让 chat log row 对齐
    this.addChild(this.body);
  }
  setText(text: string) { this.body.setText(text); }  // 原地更新
}
```

**关键**：`setText` 不重建组件，让流式 streaming 时只刷新 body 而不重排整行（与 chat-log run-state 配合做「partial markdown」渐进渲染）。

#### sanitize 全套

```ts
// tui-formatters.ts:14-160
const RENDER_CONTROL_CHARS_RE = new RegExp(
  String.raw`[ ---]`, "g");  // 保留 TAB/LF/CR
const BINARY_LINE_REPLACEMENT_THRESHOLD = 12;

export function sanitizeRenderableLine(text: string): string {
  const line = sanitizeTerminalControlsAndBinary(text).replace(/\s+/gu, " ").trim();
  return applyRtlIsolation(line);
}

function redactBinaryLikeLine(line: string): string {
  const replacementCount = (line.match(REPLACEMENT_CHAR_RE) || []).length;
  if (replacementCount >= BINARY_LINE_REPLACEMENT_THRESHOLD && replacementCount * 2 >= line.length) {
    return "[binary data omitted]";  // 12+ 个 U+FFFD 且占行 50%+ → 整行折叠
  }
  return line;
}

function isolateRtlLine(line: string): string {
  if (!RTL_SCRIPT_RE.test(line)) return line;
  return `${RTL_ISOLATE_START}${line}${RTL_ISOLATE_END}`;  // U+2067 / U+2069
}
```

**sanitize 链路**：
1. ANSI 序列剥离（`stripAnsi`）→ 避免控制序列劫持终端
2. C0/DEL/C1 控制字符删除（**保留** TAB/LF/CR 否则 Markdown 表格会塌陷）
3. BIDI 控制字符删除（U+061C/U+200E-U+200F/U+202A-U+202E/U+2066-U+2069）
4. U+FFFD 累计 ≥12 且 ≥50% 行宽 → `[binary data omitted]`
5. RTL 行自动套 U+2067/U+2069 隔离（防 LTR 行混入 RTL 字符导致光标错位）

#### UsageBar 模板 DSL

```ts
// default-template.ts
export const DEFAULT_USAGE_BAR_TEMPLATE: UsageBarTemplate = {
  schema: "openclaw.usageBar.v1",
  scales: {
    braille: "⠐⡀⡄⡆⡇⣇⣧⣷⣿",
    block:   "░▏▎▍▌▋▊▉█",
    shade:   "░▒▓█",
    moon:    "🌑🌘🌗🌖🌕",
    level:   "▁▂▃▄▅▆▇█",
    weather: ["🥶","☁️","🌥","⛅️","🌤","☀️"],
    plants:  ["🪾","🍂","🌱","️","🍀","🌿"],
    moons6:  ["🌑","🌚","🌘","🌗","🌖","🌝"],
  },
  aliases: {
    models: { "claude-opus-4-6": "opus46", "gpt-5.5": "gpt5.5", ... },
    reasoning: { off:"🌑", minimal:"🌚", low:"🌘", medium:"🌗", high:"🌕", xhigh:"🌝" },
  },
  output: {
    sep: "",
    default: [
      { text: "{model.provider}{identity.emoji|🤖}{model.display_name|alias:models}" },
      { map: "model.is_fallback", cases: { true: "🔄" } },
      { map: "model.is_override", cases: { true: "📌" } },
      { when: "model.reasoning", text: "{model.reasoning|alias:reasoning}" },
      { map: "state.fast_mode", cases: { true: "⚡️", false: "🐌" } },
      { when: "context.max_tokens", text: " | 📚[{context.pct_used|meter:5:braille}]{context.max_tokens|num}" },
      { when: "cost.turn_usd", text: " 💰{cost.turn_usd|fixed:4}" },
    ],
    surfaces: {
      discord: [ /* -# markdown 顶部折行 */ ],
    }
  }
};
```

**模板 DSL 设计巧妙点**：
1. **8 套 meter scale**：用户可换 braille/block/shade/moon/level/weather/plants/moons6（覆盖色盲/无 emoji 终端）
2. **Filter pipe**：`{value|filter}` 链式：`alias:models` / `meter:5:braille` / `fixed:4` / `num`
3. **Conditional block**：`{ when: "model.reasoning", text: "..." }` 字段缺失/空时跳过
4. **Map switch**：`{ map: "model.is_fallback", cases: { true: "🔄" } }` 多分支
5. **Surface 区分**：TUI 用 `default`（单行），Discord 用 `surfaces.discord`（带 `-# -\n` 折行做小字）
6. **Schema 版本**：`schema: "openclaw.usageBar.v1"` 让 LRU cache 在 schema 升级时自动失效

### D5.3 设计巧妙点

1. **`setText()` 原地刷新**：streaming 时不重建组件，chat-log virtualized 重排成本最小
2. **`[binary data omitted]` 自动折叠**：避免一次二进制 dump 把终端刷卡（同时不阻断流）
3. **UsageBar 模板可外部覆盖**：`loadUsageBarTemplate` 支持 JSON 文件路径 + `fs.watch` 实时热更新
4. **`{ when: ..., text: ... }` 优雅 skip**：缺字段时不出错也不显示空字符串
5. **`surfaces` 多端共享同一 contract**：`buildUsageContract()` 输出 `openclaw.usageLine.v1`，TUI/Web/Discord 共用一套数据

### D5.4 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| L1504 | 无 TUI Markdown 渲染（仅纯文本） | P0 | `pulldown-cmark` + `termimad` / `termion` 包装 |
| L1505 | 无 ANSI/控制字符 sanitize | P0 | `strip-ansi-escapes`（`vte` derive） |
| L1506 | 无 RTL 双向隔离（U+2067/U+2069） | P2 | `unicode-bidi` |
| L1507 | 无 `[binary data omitted]` 折叠 | P1 | 自实现行扫描 + U+FFFD 计数 |
| L1508 | 无 UsageBar 模板 DSL（无 meter scale） | P1 | 自实现 80 行 tokenize + tiny_skia 可选条形 |

---

## D6 输入体验工程

### D6.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `src/tui/tui-submit.ts` | 全文 200+ 行 | **Submit 处理器 + 历史 + bang + trim** |
| 同上 | 38-44 | `createEditorSubmitHandler` 接受 `editor.addToHistory` 回调 |
| 同上 | 79-98 | `addToHistory(command)` + `addToHistory(value)` 路由 |
| `src/tui/tui.ts` | 1887 | `state.historyLoaded = false` 重置历史 |
| `src/tui/tui-input-history.test.ts` | 全文 | 4 项历史行为测试（trim、空不入、slash 路由、bang 路由） |
| `src/tui/tui-autocomplete.ts` | 全文 100 行 | **TUI 自动补全 + ANSI sanitize** |
| 同上 | 13-50 | `sanitizeAutocompleteProvider()` 过滤 terminal-unsafe |
| 同上 | 67-83 | `CombinedAutocompleteProvider` 合并 slash + 路径 |
| `ui/src/pages/chat/components/chat-composer-view.ts` | 505-509 | **Web UI paste 处理** |
| `ui/src/pages/chat/components/chat-composer-keydown.ts` | 全文 200 行 | **键位编排 + IME 守卫 + 多菜单 dispatch** |
| 同上 | 53-58 | `isComposing || keyCode === 229` 守卫（CJK IME） |
| 同上 | 80-103 | `goalComposer.active` 时 Enter/Ctrl+Enter 路由 |
| 同上 | 105-115 | `handleSkillMenuKeydown` / `handleInlineSlashArgKeydown` / `handleSlashMenuKeydown` 分层 |
| `ui/src/pages/chat/components/chat-composer-dom.ts` | — | `adjustTextareaHeight()` + `restoreHistoryCaret()` |

### D6.2 机制剖析

#### Submit 处理器

```ts
// tui-submit.ts:79-98
if (action !== "message") {
  clearSubmittedEditor();
  const command = action === "local shell" ? raw : value;
  const handle = action === "local shell" ? params.handleBangLine : params.handleCommand;
  params.editor.addToHistory(command);  // 命令也入历史（!ls 不入，仅 `/ls` 入）
  runSubmitAction(action, () => handle(command), params.onSubmitError);
  return;
}
// 普通消息
const admission = (snapshot ? params.admitMessage?.(value, snapshot) : params.admitMessage?.(value)) ?? { status: "allowed" };
if (admission.status === "blocked") {
  restoreBlockedEditor(trimChangesAction ? raw : value);  // 把被拒的消息回填
  params.onBlockedMessageSubmit?.(value, admission);
  return;
}
clearSubmittedEditor();
if (!trimChangesAction) {
  params.editor.addToHistory(value);  // 仅非 trim 改 action 才入历史
}
runSubmitAction("message", () => params.sendMessage(value), params.onSubmitError);
```

**关键设计**：
1. **trim 不入历史**：`onSubmit("   hi   ")` 仅入 `"hi"`，避免噪声
2. **空不入历史**：`onSubmit("")` / `onSubmit("   ")` 直接 return，不污染历史
3. **slash 路由前也入历史**：命令 `"/models"` 写入历史，下次 ↑ 直接召回
4. **被拒回填**：`admitMessage` 返回 blocked 时不丢草稿，**保留用户已输入内容**便于修改后重发

#### TUI 自动补全 sanitize

```ts
// tui-autocomplete.ts:13-50
function sanitizeAutocompleteProvider(inner: AutocompleteProvider): AutocompleteProvider {
  return {
    triggerCharacters: inner.triggerCharacters,
    async getSuggestions(...args) {
      const suggestions = await inner.getSuggestions(...args);
      const safeItems = suggestions.items.filter((item) =>
        isTerminalSafeAutocompleteValue(item.value),
      );
      if (safeItems.length === 0) return null;  // 全部不安全 → 隐藏菜单
      return {
        ...suggestions,
        items: Array.from(safeItems, (item) => {
          const { description: rawDescription, ...displayFields } = item;
          const label = sanitizeRenderableLine(item.label) || sanitizeRenderableLine(item.value) || "(unnamed)";
          // ... sanitize description
          return Object.defineProperty(displayItem, originalSafeItem, { value: item });
        }),
      };
    },
    applyCompletion(lines, cursorLine, cursorCol, item, prefix) {
      // 透传时还原原始 item（不传 sanitize 后的）
      return inner.applyCompletion(lines, cursorLine, cursorCol,
        (Reflect.get(item, originalSafeItem) as AutocompleteItem | undefined) ?? item, prefix);
    }
  };
}
```

**巧妙点**：`originalSafeItem` Symbol 让 sanitize 不污染传给 `applyCompletion` 的实际值（避免 label 被裁短后写入 draft）。

#### Web Composer 键位编排

```ts
// chat-composer-keydown.ts:53-115
return (event) => {
  const target = event.target;
  if (!(target instanceof HTMLTextAreaElement)) return;
  if (state.composerComposing || event.isComposing || event.keyCode === 229) return;  // IME 守卫

  // 1. mention menu 最优先
  if (state.mentionMenu.handleKeydown(event, mentionMenuHost, requestUpdate)) return;

  // 2. goal mode active 时拦截
  if (goalComposer.active) {
    if (event.key === "Escape") { ... }
    else if (event.key === "Enter" && !event.shiftKey &&
             (sendShortcut === "enter" || event.metaKey || event.ctrlKey) &&
             canSubmitDraft(target.value)) { ... }
    return;
  }

  // 3. skill menu → inline slash arg → slash menu → history nav
  if (props.connected && handleSkillMenuKeydown(...)) return;
  if (props.connected && handleInlineSlashArgKeydown(...)) return;
  if (props.connected && handleSlashMenuKeydown(...)) return;

  // 4. ArrowUp/Down history
  if ((event.key === "ArrowUp" || event.key === "ArrowDown") && props.onHistoryKeydown) {
    commitDraft(target.value);
    const result = props.onHistoryKeydown({...});
    if (result.handled) {
      if (result.preventDefault) event.preventDefault();
      requestUpdate();
      if (result.restoreCaret) restoreHistoryCaret(target, result.restoreCaret);
      return;
    }
  }
}
```

**分层 dispatch**：
1. IME composing → 不抢 key（避免拼音过程中误触发）
2. mention menu 优先（最贴近光标的语义）
3. goal mode 独立（Enter=commit）
4. skill → inline slash → slash menu 顺序（更具体优先）
5. ArrowUp/Down history 仅在以上都未 handled 时

#### Paste 图片/文件

```ts
// chat-composer-view.ts:505-509
@paste=${(event: ClipboardEvent) => {
  if (canCompose && !props.suggestionComposer) {
    handleChatAttachmentPaste(event, props);
  }
}}
```

Web `paste` event 直接路由到 attachment 入场（图片自动生成 base64 preview，文件转 attachment card）。

### D6.3 设计巧妙点

1. **pi-tui 编辑器抽象**：`editor.setText / addToHistory / getText / getExpandedText` 一组最小接口，TUI 与 Web 各自实现
2. **`restoreHistoryCaret` 字/字节双换算**：CJK 上下箭头恢复时光标必须在原字符边界（laew L1208 已修过同类 bug，可借鉴）
3. **`applyCompletion` Symbol 透传**：sanitize 不污染 apply 数据（`Reflect.get(item, originalSafeItem)`）
4. **IME `keyCode === 229` 守卫**：Chrome 中文/日文 IME enter 不触发 send
5. **历史持久化可扩展**：实际历史存储在 pi-tui 编辑器内部（OpenClaw 不持久化，laew 可加 `~/.cache/laew/history.jsonl`）
6. **`shouldEnableWindowsGitBashPasteFallback`**：检测 iTerm/Apple_Terminal 时启用 burst coalescing（多行粘贴合并为单条消息）

### D6.4 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| L1509 | 无命令入历史（仅普通消息） | P1 | tui-input `Editor::add_to_history` 改造 |
| L1510 | 无 ANSI sanitize 后的 apply 透传 | P2 | `Symbol` 模式 Rust 用 `Rc<RefCell<T>>` |
| L1511 | 无 IME composing 守卫 | P1 | crossterm `Event::Key` 的 `Event::Key::kind` 检测 |
| L1512 | 无 paste burst coalescing（iTerm 多行粘贴合并） | P1 | 200ms 时间窗聚合 |

---

## D7 Onboarding / 目录信任 / 主题偏好

### D7.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `src/wizard/setup.ts` | 全文 747 行 | **Setup Wizard 总编排** |
| 同上 | 51-92 | `runSetupWizard` + `runWizardWithPromptNavigation` |
| `src/wizard/setup.shared.ts` | 132-160 | **`requireRiskAcknowledgement()` 安全风险一次性确认** |
| 同上 | 170-200 | `requestTelemetryConsent()` 遥测 opt-in 写入 `consentedAt` |
| `src/wizard/setup.inference-verification.ts` | — | 推理验证（发一条测试 ping 验证 Provider） |
| `src/wizard/setup.completion.ts` | 全文 140 行 | **shell completion 自动检测 + 安装** |
| 同上 | 99-119 | `installCompletionForSetup()` 写 shell profile |
| `src/wizard/setup.gateway-config.ts` | — | Gateway 端口/绑定/Tailscale 模式选择 |
| `src/wizard/setup.migration-import.ts` | — | 从旧 config 迁移导入 |
| `src/wizard/setup.app-recommendations.ts` | — | 「推荐安装的 channel app」清单 |
| `src/wizard/setup.memory-import.ts` | — | 从其他 Agent 导入历史 memory |
| `src/wizard/i18n/` | 3 个 locale | `en.ts` / `zh-CN.ts` / `zh-TW.ts` |
| `src/wizard/plugin-capability-consent.ts` | — | 插件能力 consent（与 `clawhub-install-trust.ts` 联动） |
| `ui/src/app/theme.ts` | 全文 | **12 套 ThemeName × 2 ThemeMode 解析** |
| `ui/src/components/theme-mode-toggle.ts` | 全文 50 行 | `theme-change` 事件三态切换 |
| `src/infra/clawhub-install-trust.ts` | — | 插件/扩展安装 trust 检查（签名 + 来源） |
| `src/secrets/provider-env-vars.ts` | — | Provider 凭据 env 注入（trust 链路） |

### D7.2 机制剖析

#### Risk 一次性确认

```ts
// setup.shared.ts:132-160
export async function requireRiskAcknowledgement(params): Promise<OpenClawConfig> {
  if (params.config.wizard?.securityAcknowledgedAt) {
    return params.config;  // 已确认过 → 直接返回
  }
  if (params.opts.acceptRisk === true) {
    return applySecurityAcknowledgement(params.config);  // 非交互模式：CLI flag 跳过 UI
  }
  await params.prompter.note(getSecurityNoteMessage(), getSecurityNoteTitle());
  const ok = await params.prompter.confirm({
    message: getSecurityConfirmMessage(),
    initialValue: true,
    layout: "vertical",
  });
  if (!ok) throw new WizardCancelledError(t("wizard.setup.riskNotAccepted"));
  return applySecurityAcknowledgement(params.config);
}
function applySecurityAcknowledgement(config) {
  return inheritLegacyDefaultAgentId(config, {
    ...config,
    wizard: { ...config.wizard, securityAcknowledgedAt: new Date().toISOString() },
  });
}
```

**关键设计**：
1. **持久化到 `wizard.securityAcknowledgedAt`**：二次运行 setup 不再问
2. **`--accept-risk` CLI flag**：CI / 非交互模式一键跳过
3. **拒绝即终止**：`WizardCancelledError` 上抛，`process.exit(1)`
4. **layout: "vertical"**：长 message 自动垂直展开，不挤在一行

#### Telemetry opt-in

```ts
// setup.shared.ts:170-200
export async function requestTelemetryConsent(params): Promise<OpenClawConfig> {
  if (params.opts.nonInteractive === true || params.config.telemetry?.consentedAt) {
    return params.config;  // 非交互或已同意 → 跳过
  }
  await params.prompter.note(t("wizard.telemetry.description"), t("wizard.telemetry.title"));
  const enabled = await params.prompter.select<boolean>({
    message: t("wizard.telemetry.title"),
    options: [
      { value: false, label: t("wizard.telemetry.decline") },
      { value: true, label: t("wizard.telemetry.accept") },
    ],
    initialValue: false,  // 默认拒绝（opt-in 而非 opt-out）
  });
  return inheritLegacyDefaultAgentId(params.config, {
    ...params.config,
    telemetry: {
      ...params.config.telemetry,
      enabled,
      consentedAt: new Date().toISOString(),
    },
  });
}
```

**关键设计**：
1. **默认拒绝（opt-in）**：符合 GDPR / CCPA 隐私基线
2. **`consentedAt` 时间戳**：审计链（与 `enabled` flag 一起写入）
3. **`nonInteractive` 直接跳过**：自动化部署不会有 consent 痕迹

#### Shell Completion 自动接管

```ts
// setup.completion.ts:100-140
if (completionStatus.profileInstalled && !completionStatus.cacheExists) {
  await ensureCompletionCache();  // Case 2: profile 有但 cache 缺失 → 静默修
  return;
}
if (!completionStatus.profileInstalled) {
  const shouldInstall = params.flow === "quickstart"
    ? true  // quickstart 强制安装
    : await params.prompter.confirm({ message: t("wizard.completion.enable", {...}), initialValue: true });
  if (!shouldInstall) return;
  const cacheGenerated = await ensureCompletionCache();
  if (!cacheGenerated) return;
  const completionInstalled = await installCompletionForSetup();
  if (!completionInstalled) return;
  await params.prompter.note(t("wizard.completion.installed", { reloadHint }), ...);
}
```

**4 种 case 自动处理**：
- Case 1：profile 用了 slow dynamic pattern → 升级 cached
- Case 2：profile 有 cache 无 → 静默补 cache
- Case 3：都没有 → 提示并安装
- Case 4：profile + cache 都有 → 静默返回

#### 主题 12 套 × 2 模式

```ts
// ui/src/app/theme.ts
export type ThemeName =
  | "claw" | "knot" | "dash" | "absolutely" | "tide" | "beacon"
  | "phosphor" | "crt" | "manuscript" | "rose" | "miami" | "custom";
export type ThemeMode = "system" | "light" | "dark";
// → resolveTheme() 输出 22 种 ResolvedTheme

function parseThemeSelection(themeRaw: unknown, modeRaw: unknown) {
  const normalizedTheme = VALID_THEME_NAMES.has(themeRaw) ? themeRaw : "claw";
  const normalizedMode = VALID_THEME_MODES.has(modeRaw) ? modeRaw : "system";
  return { theme: normalizedTheme, mode: normalizedMode };
}
function resolveMode(mode: ThemeMode): "light" | "dark" {
  if (mode === "system") return prefersLightScheme() ? "light" : "dark";
  return mode;
}
```

**`theme-mode-toggle.ts`** 三态循环：`system → light → dark → system`，`prefers-color-scheme: light` 自动跟随。

### D7.3 设计巧妙点

1. **`securityAcknowledgedAt` + `consentedAt` 时间戳**：审计链可还原「用户何时确认了什么」
2. **`acceptRisk` / `nonInteractive` 双 flag**：自动化与人工流程分离
3. **4 case 自动判定**：shell completion 安装覆盖 99% 场景，零手动操作
4. **`theme === "claw"` 默认 + 11 套可选**：`custom` 占位为用户自定义
5. **plugin capability consent + clawhub trust 联动**：安装任何 plugin 都走 trust gate

### D7.4 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| L1513 | 无 onboarding wizard（首次启动无引导） | P0 | `dialoguer` / `inquire` 多步骤表单 |
| L1514 | 无安全风险一次性确认（仅 security_acknowledged_at 字段缺失） | P1 | `dialoguer::Confirm` |
| L1515 | 无 telemetry opt-in（缺省默认发送） | P0 | config schema 必填 `telemetry.consentedAt` |

> **说明**：L1486-L1515 共 30 个槽位，本轮实际分配 30 个 gap（已用满）。

---

## D8 会话导出 / 共享 + statusline + 实时成本

### D8.1 源码定位

| 文件 | 行 | 角色 |
|---|---|---|
| `src/auto-reply/reply/commands-export-session.ts` | 全文 600+ 行 | **HTML 导出主流程** |
| 同上 | 70-95 | `BACKEND_DELEGATED_WARNING` 后端 runtime 提示 |
| `src/auto-reply/reply/commands-export-session-file.ts` | 全文 80 行 | **fs-safe 写入 + 冲突后缀 `-1 -2 -3`** |
| `src/auto-reply/reply/export-html/` | 3 文件 | `template.html` / `template.css` / `template.js` + marked + highlight.js vendor |
| `src/auto-reply/reply/commands-export-trajectory.ts` | — | **JSONL 轨迹导出** |
| `src/auto-reply/commands-registry.shared.ts` | 285-298 | `/export-session /export` + `/export-trajectory /trajectory` 命令注册 |
| `src/gateway/control-ui-share.ts` | 全文 100 行 | **`/share/<token>` 公开预览页 + OG metadata** |
| 同上 | 70-92 | 完整 OG / Twitter Card meta + dark theme |
| `src/gateway/control-ui-public-session-token.ts` | 全文 200 行 | **AES-256-GCM 加密 share token + HKDF 派生** |
| 同上 | 1-15 | `v1.` 前缀 + 12 byte nonce + 16 byte tag |
| `src/gateway/server-methods/sessions-sharing.ts` | 全文 400+ 行 | **session visibility + public share RPC** |
| 同上 | 130-145 | `projectPublicSessionShare()` AES-GCM mint |
| `src/gateway/server-methods/sessions-sharing.ts` | — | `sessions.public-share.set` 权限校验 + 三态 actor evidence |
| `src/auto-reply/usage-bar/contract.ts` | 全文 80 行 | **`UsageContract` 数据契约 (v1 schema)** |
| `src/auto-reply/usage-bar/translator.ts` | 291-320 | `renderUsageBar()` 模板 → 文本 |
| `src/auto-reply/usage-bar/template.ts` | 全文 200 行 | **模板文件 + fs.watch 热更新** |
| 同上 | 100-130 | `cacheTemplateFile()` + chokidar fs.watch |
| `src/tui/tui-status-summary.ts` | 全文 100 行 | TUI statusline 完整版 |
| 同上 | 14-100 | `formatStatusSummary()` Gateway/Heartbeat/Sessions/Recent/QueuedEvents |
| `src/tui/tui-formatters.ts` | 全文 200 行 | `formatTuiFooter` 紧凑 footer（model + thinking + context + usage） |
| 同上 | 31-43 | `formatModelFooter()` 简化模型标识 |
| `src/tui/tui-formatters.ts` | 155-200 | `formatContextUsageLine` 上下文使用率行 |
| `src/agents/sessions/sessions-cost.ts` | — | 成本累计（per-turn / total） |
| `ui/src/pages/usage/usage-page.ts` | 全文 | **Web 端 usage 看板 + export.ts** |
| `ui/src/pages/usage/export.ts` | 全文 | `createUsageJsonExportRequest` JSON 下载 |
| `src/gateway/control-ui-public-session-read.ts` | — | 公开 token 解密读取会话 |

### D8.2 机制剖析

#### HTML 导出

```ts
// commands-export-session.ts（节选）
async function generateHtml(sessionData: SessionData): Promise<string> {
  const [template, templateCss, templateJs, markedJs, hljsJs] = await Promise.all([
    loadTemplate("template.html"),
    loadTemplate("template.css"),
    loadTemplate("template.js"),
    loadTemplate(path.join("vendor", "marked.min.js")),
    loadTemplate(path.join("vendor", "highlight.min.js")),
  ]);
  // 把动态数据通过 <script data-openclaw-export-placeholder="..."> 占位注入
  const next = template.replace(placeholder, (_match, openTag, closeTag) => {
    replaced = true;
    const finalOpenTag = openTag.replace(/\sdata-openclaw-export-placeholder="[^"]*"/, "");
    return `${finalOpenTag}${value}${closeTag}`;
  });
  if (!replaced) throw new Error(`Export HTML template missing ${name} placeholder`);
  return next;
}
```

**导出特性**：
1. **完整 HTML + vendored marked + highlight.js**：离线可打开，无外链
2. **dark session-export palette**：固定一套色板（cyan/blue/green/red/yellow）+ 工具状态色（pending / success / error）
3. **占位符注入**：`<script data-openclaw-export-placeholder="transcript">` 注入 JSON 数据（不污染 HTML 转义）
4. **fs-safe 写入**：`createUnusedFile()` 冲突时自动 `-1` `-2` 后缀，**最多 100 次尝试**

```ts
// commands-export-session-file.ts:18-32
async function createUnusedFile(workspaceRoot, filePath, contents): Promise<string> {
  for (let suffix = 1; suffix <= MAX_DEFAULT_FILENAME_ATTEMPTS; suffix++) {
    const candidate = suffix === 1 ? filePath : addCollisionSuffix(filePath, suffix);
    try {
      await workspaceRoot.create(candidate, contents, { encoding: "utf-8" });
      return candidate;
    } catch (error) {
      if (error instanceof FsSafeError && error.code === "already-exists") continue;
      throw error;
    }
  }
}
```

#### 公开分享链接（AES-256-GCM）

```ts
// control-ui-public-session-token.ts:1-30
const PUBLIC_SESSION_TOKEN_PREFIX = "v1.";
const PUBLIC_SESSION_TOKEN_CIPHER = "aes-256-gcm";
const PUBLIC_SESSION_TOKEN_NONCE_BYTES = 12;
const PUBLIC_SESSION_TOKEN_TAG_BYTES = 16;
const PUBLIC_SESSION_TOKEN_AAD = Buffer.from(
  "openclaw.public-session-share-locator.aad.v1", "utf8");
const PUBLIC_SESSION_TOKEN_KEY_SALT = Buffer.from(
  "openclaw.public-session-share-locator.salt.v1", "utf8");
const PUBLIC_SESSION_TOKEN_KEY_INFO = Buffer.from(
  "openclaw.public-session-share-locator.key.v1", "utf8");
const PUBLIC_SESSION_TOKEN_MAX_PLAINTEXT_BYTES = 5_000;
const PUBLIC_SESSION_TOKEN_CODEC_CACHE_LIMIT = 32;
```

**Token 设计**：
1. **`v1.` 前缀**：版本化，未来可换算法
2. **AES-256-GCM**：12 byte nonce + 16 byte tag
3. **AAD（Additional Authenticated Data）**：固定字符串绑定算法/上下文，防跨上下文 replay
4. **HKDF 派生 key**：从 device identity 用 salt + info 派生 → 不直接用主 key
5. **Plaintext 上限 5000 字节**：限制 locator payload 大小，防滥用
6. **Codec cache 32 槽**：mint/resolve 都走缓存，热点 token 0 CPU

```ts
// sessions-sharing.ts:130-145
function projectPublicSessionShare({ agentId, sessionKey, grant, codec }): SessionPublicShare {
  return {
    token: codec.mint({
      agentId, sessionKey, sessionId: grant.sessionId, shareId: grant.id,
    }),
    createdAt: grant.createdAt,
  };
}
```

**`/share/<token>` 公开页**（`control-ui-share.ts`）：
- 完整 OG / Twitter Card meta → 社交平台卡片预览
- `og:image` 指向 `/share/card.png`
- `X-Robots-Tag: noindex, nofollow` → 不被搜索引擎索引
- CSP 严格：`default-src 'none'`，仅 `img-src 'self'` + `style-src 'unsafe-inline'`
- `Content-Length` 预计算 + `no-store` cache

#### UsageBar / Statusline

```ts
// tui-formatters.ts:31-43
function formatModelFooter({ model, thinkingLevel }) {
  const model = splitTrailingAuthProfile(params.model ?? "").model || "unknown";
  const thinkingLevel = params.thinkingLevel?.trim();
  return thinkingLevel && thinkingLevel !== "off" ? `${model} ${thinkingLevel}` : model;
}

// formatTuiFooter
const footer = [
  `agent ${params.agentLabel}`,
  `session ${params.sessionLabel}`,
  formatModelFooter({ model: sessionInfo.model, thinkingLevel: params.thinkingLevel }),
  formatGoalFooter(sessionInfo.goal),
  fastLabel,
  verbose !== "off" ? `verbose ${verbose}` : null,
  traceLabel,
  reasoningLabel,
  `deliver:${params.deliver ? "on" : "off"}`,
  formatTokens(sessionInfo.totalTokens ?? null, sessionInfo.contextTokens ?? null),
].filter(Boolean).join(" | ");
```

**Statusline 组成**：
- `agent <name> | session <key> | <model> [thinking] | goal:... | fast | verbose | trace | reasoning | deliver:on/off | 1.2k/200k`

```ts
// tui-status-summary.ts:70-100（节选）
lines.push(`Session store: ${sessionPaths[0]}`);
lines.push(`Default model: ${defaultModel}${defaultCtx}`);
lines.push(`Active sessions: ${sessionCount}`);
lines.push("Recent sessions:");
for (const entry of recent) {
  const ageLabel = typeof entry.age === "number" ? formatTimeAgo(entry.age) : "no activity";
  const model = entry.model ?? "unknown";
  const usage = formatContextUsageLine({...});
  const flags = entry.flags?.length ? ` | flags: ${entry.flags.join(", ")}` : "";
  lines.push(`- ${entry.key}${entry.kind ? ` [${entry.kind}]` : ""} | ${ageLabel} | model ${model} | ${usage}${flags}`);
}
```

**UsageBar 模板热更新**（`usage-bar/template.ts:100-130`）：
```ts
function cacheTemplateFile(path: string): UsageBarTemplate | undefined {
  const result = readTemplateFile(path);
  // ...
  if (entry.template) {
    try {
      const watcher = watch(path, { persistent: false }, () => {
        const next = readTemplateFile(path);
        if (next.reason) warnInvalidUsageTemplate("file", next.reason, path);
        entry.template = next.template;
      });
      watcher.on("error", () => {
        watcher.close();
        entry.watcher = undefined;
        entry.template = undefined;
      });
      entry.watcher = watcher;
    } catch {}
  }
}
```

**巧妙点**：
1. **LRU 64 槽**：fileCache 上限 64，超过按插入序淘汰
2. **watcher.error 静默失效**：watcher 故障时不抛，仅清缓存，下次调用回退 default
3. **`warnedTemplateOverrides`** 独立 dedupe cache（256 槽）：同一坏配置不会反复 warn

#### Web usage 导出

```ts
// ui/src/pages/usage/export.ts
export function createUsageJsonExportRequest(host, gateway, query) {
  return createUsageRequest(host, {
    task: async (data, { signal }) => {
      // 1. 拿 base data
      // 2. 若任何 session.hasContextWeight → 重发请求带 includeContextWeight
      // 3. hydrate 缺失字段 + 比较两次状态一致性
      // 4. 失败抛 `usage.export.changed` 提示「导出期间 usage 变化」
      return { connection, filename: `openclaw-usage-${currentLocalDate()}.json`, data: hydratedData };
    },
    onComplete: ({ connection, filename, data }) => {
      if (gateway.isCurrent(connection)) {
        downloadTextFile(filename, JSON.stringify(data, null, 2), "application/json;charset=utf-8");
      }
    },
  });
}
```

**快照一致性**：导出期间 usage 变化 → 提示用户「数据可能不一致」让用户重新点。

### D8.3 设计巧妙点

1. **vendored marked + highlight.js**：HTML 导出离线可用，无 CDN 依赖
2. **`<script data-openclaw-export-placeholder>` 占位注入**：避免 HTML 转义 + LLM 输出含 `<script>` 时 RCE 风险（值是 JSON 序列化）
3. **AES-256-GCM + AAD + HKDF**：token 既能验证完整性，又绑定上下文，**不能用同一 token 跨 user/agent**
4. **`v1.` 前缀**：未来切算法只需加 `v2.` 分支
5. **Plaintext 5KB 上限**：限制 locator payload（agentId/sessionKey/sessionId/shareId 都短）
6. **公开页 CSP 严格**：`default-src 'none'` + `frame-ancestors 'none'` 防 clickjack
7. **`X-Robots-Tag: noindex, nofollow`**：分享链接不进搜索引擎索引
8. **`isCurrent(connection)` 导出期间防过期**：gateway 切换时不下载旧 snapshot
9. **`usage.export.changed` 显式失败**：不静默给过期数据
10. **TUI footer `deliver:on/off` 显式**：用户可见当前 channel 是否要外发（discriminating send mode）

### D8.4 laew gap

| 编号 | 描述 | P | 推荐 Rust crate |
|---|---|---|---|
| 部分 D5 | 无 HTML 会话导出（仅 `-debug` 写 debug_report .md） | P0 | `handlebars` + `pulldown-cmark` + `syntect` |
| 部分 D8 | 无 AES-256-GCM 公开分享链接 | P1 | `aes-gcm` + `hkdf` + URL-safe base64 |
| 部分 D8 | 无 TUI statusline（仅简易 footer） | P0 | crossterm 顶部 1 行 ANSI 渲染 |
| 部分 D8 | 无 token / cost 实时显示（仅 CompactAgent 计算 80%） | P0 | `tiktoken-rs` + `indicatif` |

> 注：上述 gap 已并入本轮 D5/D8 章节；L1486-L1515 已用满 30 个槽位。

---

## 末章 gap 汇总

### L1486-L1515 共 30 个 laew gap（按 P0/P1/P2 分组）

#### P0 紧急（10 项）
| 编号 | 描述 | 章节 | 推荐 Rust crate |
|---|---|---|---|
| L1486 | 无 @提及系统（@agent / @file / @模式） | D1 | `regex` + `tui-input` menu hook |
| L1489 | 无 prompt 模板发现机制（无 frontmatter 解析） | D2 | `serde_yaml` + `walkdir` |
| L1490 | 无 `/` 斜杠命令注册中心（仅硬编码 9 个） | D2 | `clap_complete` + 命令注册表 |
| L1494 | 无用户级对话回退（仅整体 session reset） | D3 | SQLite 增加 entry 链表 + `leaf_entry_id` |
| L1497 | 无 lifecycle revision CAS 守卫 | D3 | `entry-level optimistic lock` |
| L1499 | 无工作区文件 chokidar 监视 | D4 | `notify` + `notify-debouncer-mini` |
| L1504 | 无 TUI Markdown 渲染（仅纯文本） | D5 | `pulldown-cmark` + `termimad` |
| L1505 | 无 ANSI/控制字符 sanitize | D5 | `strip-ansi-escapes`（`vte` derive） |
| L1513 | 无 onboarding wizard（首次启动无引导） | D7 | `dialoguer` / `inquire` |
| L1515 | 无 telemetry opt-in（缺省默认发送） | D7 | config schema 必填 `telemetry.consentedAt` |
| 部分 D8 | 无 TUI statusline / 无 token + cost 实时显示 | D8 | `tiktoken-rs` + `indicatif` |
| 部分 D5/D8 | 无 HTML 会话导出 | D5/D8 | `handlebars` + `syntect` |

#### P1 重要（12 项）
| 编号 | 描述 | 章节 | 推荐 Rust crate |
|---|---|---|---|
| L1487 | mention 触发检测未做 code fence / blockquote 守卫 | D1 | `pulldown-cmark` 解析状态机 |
| L1491 | 无 `$1 $@ $*` 占位符替换 | D2 | 自实现 tokenize |
| L1493 | 无 shell completion 安装向导 | D2 | `clap_complete` + `shell-words` |
| L1495 | 无对话分支（session 在某 entry 分叉为多线） | D3 | SQLite + UI selector |
| L1496 | 无「回退到第 N 条消息后恢复 editor 草稿」机制 | D3 | agent 循环 user_message ↔ entry 映射 |
| L1500 | 无 EMFILE/ENOSPC 自愈降级 polling | D4 | `notify` `PollWatcher` + inotify limit 探测 |
| L1501 | 无 `awaitWriteFinish` 写完稳定检测 | D4 | `notify-debouncer-mini` |
| L1502 | 无 symlink trust 链验证 | D4 | `std::fs::canonicalize` |
| L1507 | 无 `[binary data omitted]` 折叠 | D5 | 自实现行扫描 + U+FFFD 计数 |
| L1508 | 无 UsageBar 模板 DSL（无 meter scale） | D5 | 自实现 80 行 tokenize |
| L1509 | 无命令入历史（仅普通消息） | D6 | tui-input 改造 |
| L1511 | 无 IME composing 守卫 | D6 | crossterm `Event::Key::kind` |
| L1512 | 无 paste burst coalescing（iTerm 多行粘贴合并） | D6 | 200ms 时间窗聚合 |
| L1514 | 无安全风险一次性确认 | D7 | `dialoguer::Confirm` |
| 部分 D8 | 无 AES-256-GCM 公开分享链接 | D8 | `aes-gcm` + `hkdf` |

#### P2 进阶（8 项）
| 编号 | 描述 | 章节 | 推荐 Rust crate |
|---|---|---|---|
| L1488 | 无客户端 LRU + debounce 缓存（mention） | D1 | `lru` |
| L1492 | 无 inline slash 实时替换 | D2 | tui-input Menu hook |
| L1498 | 无上游链接会话 fail-closed | D3 | `data: external_session_link` flag |
| L1503 | 无共享 watcher + workspace idle TTL | D4 | `Arc<Mutex<HashMap>>` |
| L1506 | 无 RTL 双向隔离（U+2067/U+2069） | D5 | `unicode-bidi` |
| L1510 | 无 ANSI sanitize 后的 apply 透传 | D6 | `Rc<RefCell<T>>` |

### 覆盖率统计
| 维度 | openclaw 实现度 | laew 现状 | 主要差距 |
|---|---|---|---|
| D1 @mention | 95% | 0% | 完全缺失 |
| D2 斜杠+模板 | 90% | 5%（仅 9 个硬编码） | 模板发现 + 多源 + frontmatter |
| D3 用户级回退/分支 | 100%（rewind/fork/switch 三 RPC 完整） | 0% | 完全缺失 |
| D4 文件监视 | 90%（chokidar 多 workspace + 自愈 + symlink trust） | 0% | 完全缺失 |
| D5 富文本渲染 | 95%（Markdown + UsageBar DSL + sanitize） | 10%（仅 ANSI） | 模板 DSL + sanitize 全套 |
| D6 输入体验 | 95%（历史 + sanitize + IME + 多菜单 dispatch） | 70%（基础有） | 命令入历史 + IME 守卫 + paste coalescing |
| D7 Onboarding | 100%（wizard + risk ack + telemetry opt-in + completion） | 0% | 完全缺失 |
| D8 导出/statusline | 95%（HTML+JSONL+AES-256-GCM 公开链接+UsageBar 热更） | 30%（仅 CompactAgent 80%） | HTML 导出 + AES 公开链接 + 实时 cost |

### laew 实现优先级（与本轮进度表 L1486-L1515 对齐）
- **本轮（2026-09-09 第 18 轮）计划推进**（按 P0 顺序）：
  1. L1505 ANSI sanitize（基于 laew 已有的 ANSI 处理）
  2. L1490 斜杠命令注册（基于 clap_complete）
  3. L1515 telemetry opt-in config 必填
  4. L1513 onboarding wizard（dialoguer 多步骤）
  5. L1499 chokidar 文件监视（notify + notify-debouncer-mini）

### 与前 17 轮的衔接
- **第十五轮 L836-L1035（网络/编译器/OS/共识/ML/形式化/图库/流处理）**：本轮 D4 文件监视属于 OS 内核交互维度补充；D5 UsageBar DSL 属于协议翻译上层；D8 statusline 与实时 cost 属于终端控制序列（TUI）的应用层。
- **第十六轮 L1036-L1165+（多轮恢复/加密/Hook/Cordis/LLM 协议栈/Daemon）**：本轮 D3 属于 session 状态机的用户面补全，D7 wizard 是 onboarding 配套。
- **第十七轮 L1166-L1395+（崩溃恢复/多租户/RRF/网关/Pregel/Skill/Agent池/Turn锁/HTTP/安全）**：本轮 D6 输入体验工程是 Session 多轮交互的补充，D1 mention 是 human-in-the-loop 的工具扩展，D8 公开分享是 session 共享的实现。

### 主要发现总结
1. **openclaw 用户交互层最成熟的是 D3（rewind/fork/switch 三 action 完整 RPC + lifecycle revision CAS）+ D8（AES-256-GCM + OG + CSP 严格的公开分享）**，代表 2026 年生产级 Agent CLI 的最前沿设计
2. **最值得借鉴的是 D7 onboarding wizard + shell completion 自动接管 + D5 UsageBar 模板 DSL**：覆盖 onboarding → first task → 持续 statusline 完整链路
3. **laew 在 D5/D6 已有部分基础（ANSI 处理、TUI 单行输入）但深度差距大**：需要补齐 Markdown 渲染 + 命令注册 + 文件监视 + HTML 导出四大块
4. **D8 AES-256-GCM 公开分享链接 + OG/Twitter Card + CSP `default-src 'none'`**：是 laew 从 PoC 升级到「可分享 demo」的必经能力，建议优先级 P1

---

> **第十八轮 openclaw 深度分析完成**。覆盖 D1-D8 共 8 大用户交互维度，每维度含源码定位 + 机制剖析 + 设计巧妙点 + laew gap 表。共识别 **30 个新 gap（L1486-L1515）**，P0:10 项 / P1:13 项 / P2:7 项，全部附 Rust crate 建议。
> 主文档 `openclaw.md` 文末同步追加「第 21 章 第十八轮深挖：用户交互体验层」浓缩章节（150-300 行），保留完整 30 个 gap 索引。
