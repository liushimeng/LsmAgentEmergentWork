# 第七轮 · Git 集成与变更回滚 / Checkpoint 深度对比

> 对比对象：claudecode · opencode · pi · atomcode · deepseek-harness · openclaw ·
> cc-switch · agent-studio（共 8 个）外加 laew 现状。聚焦 **7 个分析维度**：
> Git 能力地图 / 变更追踪与归因 / checkpoint / undo / rewind（核心）/
> worktree 并发隔离 / Git 上下文注入 / 自动提交与 Co-Authored-By 约定。
>
> 数据来源全部以 `<项目相对路径>:<行号>` 锚点引自源码 + 已合入的横向专题。
>
> 去重声明：`专题-第三轮-会话持久化与崩溃恢复深度分析.md` 涉及 JSONL/SQLite session 持久化与 resume；
> `专题-第六轮-Goal状态机与任务生命周期深度对比.md` 涉及 Goal 状态机；
> 本专题**只写它们没写的**：文件系统级 Git 集成 + 文件变更回滚 + workspace checkpoint + Git 上下文注入。

---

## 目录

1. 结论速览
2. 逐项目剖析
   - claudecode —— checkpoint + rewind + Co-Authored-By 范本
   - opencode —— shadow git repo + effect + worktree 三件套
   - pi —— 会话树 + 无文件 checkpoint 空白
   - atomcode —— Rust 独立 git-dir + RewindScope::Code/Conversation 二态
   - deepseek-harness —— git 仅出现在 BashTool 内部文档
   - openclaw —— git 仅用于 skill source-install
   - cc-switch / agent-studio —— 完全无 git 集成
   - laew —— 零 git 集成
3. 横向对比大表（17 行）
4. checkpoint / undo 三种存储架构的 ASCII 图解
5. 13 个设计模式与反模式
6. laew 现状与 P0 / P1 / P2 路线图（含 Rust crate 选择 + SQLite 表结构 + API 草案）
7. 关键文件速查

---

## 1. 结论速览

1. **5 种 checkpoint 存储形态**并存，差异极大：
   - **claudecode**：`{configDir}/file-history/{sessionId}/{sha16@vN}` —— **明文拷贝 + 文件级版本号**，无 git，rewind 时 cp 还原 (`fileHistory.ts:725-836`)。
   - **opencode**：`{Global.Path.data}/snapshot/{project_id}/{worktree_hash}/.git/` —— **shadow git repo + 共享对象数据库 + alternates** (`snapshot/index.ts:71`, `snapshot/index.ts:198-232`)。
   - **atomcode**：`{config_dir}/rewind/{project_hash}/{session_id}/` —— **独立 bare git + 可选 before/after tree + Ledger v2 + 事务日志** (`rewind.rs:20-28`, `rewind.rs:202-225`)。
   - **pi**：**无**文件 checkpoint（"snapshot" 仅指 bash 输出截断快照, `output-accumulator.ts:91`）。
   - **deepseek-harness / openclaw / cc-switch / agent-studio**：**无** checkpoint。
2. **Git 集成方式 100% spawn child process**：所有 8 个项目都 `Command::new("git")` / `ChildProcess.make("git")` / `execa("git")`，**0 个用 libgit2 / gix / simple-git / dulwich**。`gix`(Rust) / `git2`(Rust) / `libgit2` 是 laew P0 的备选。
4. **Co-Authored-By 由 LLM 配置约定**生成（`Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>`, `claudecode attribution.ts:80`），所有项目都把 **自动 push 默认关闭**。
5. **worktree 三种用法**：
   - **claudecode**：`ExitWorktreeTool` + 命令行 "EnterWorktree" 文档（`ExitWorktreeTool.ts:148`） —— 引导 Claude 让用户手动操作。
   - **opencode**：完整 worktree 服务（create/list/remove/reset），命名 `opencode/{slug}`，沙箱白名单（`worktree/index.ts:182-197`）。
   - **atomcode**：未使用 git worktree，但 `WorkspaceCheckpoint` 用 `git_worktree_root()` 校验 worktree 必须是真实 git 仓库（`rewind.rs:212`），否则 rewind 拒绝。
6. **git 上下文注入 = snapshot-in-time 模板**：claudecode 把 git status/branch/log 拼成纯字符串注入 system prompt，会话开始一次性 memoize（`context.ts:36-103`）；opencode 把 git 状态暴露为 HTTP API `/file/status`，**不直接注入 prompt**，由 model 自己 `git status --short`（`command/template/review.txt:16,37`）。
7. **laew 完全无 git 集成**：源码 `grep "git"` 只命中 2 处（`main.rs:19` 的帮助文本、`grep.rs:6` 的 .git 跳过），**零 spawn / 零 context / 零 checkpoint**。

---

## 2. 逐项目剖析

### 2.1 claudecode —— checkpoint + rewind + Co-Authored-By 范本

#### 2.1.1 FileHistory 模块（核心 checkpoint 机制）

`claudecode/src/utils/fileHistory.ts` 是一个 **与 git 解耦的纯文件级 checkpoint 引擎**：

- 存储：`{getClaudeConfigHomeDir()}/file-history/{sessionId}/{sha16@vN}`，`sha16` 是文件路径 SHA-256 前 16 字节（`fileHistory.ts:725-731`）：
  ```typescript
  function getBackupFileName(filePath: string, version: number): string {
    const fileNameHash = createHash('sha256').update(filePath).digest('hex').slice(0, 16)
    return `${fileNameHash}@v${version}`
  }
  ```
- 触发时机：**每次 Edit/Write 工具调用前**调 `fileHistoryTrackEdit`（`fileHistory.ts:86-193`），**每个用户消息前**调 `fileHistoryMakeSnapshot`（`fileHistory.ts:198-342`）。
- 快照上限：`MAX_SNAPSHOTS = 100`（`fileHistory.ts:54`），超出后 slice 保留最新 100 个。
- 删除/新增兼容：文件不存在时 `backupFileName === null`（`fileHistory.ts:243-246`），rewind 时调用 `unlink`。
- 性能：先 mtime/size 短路（`compareStatsAndContent`, `fileHistory.ts:640-672`），相等直接复用 v1，不复制。
- 权限保留：`copyFile` 后 `chmod(backupPath, srcStats.mode)`（`fileHistory.ts:786`）。

> 关键代码段（`fileHistory.ts:86-117`）：
> ```typescript
> export async function fileHistoryTrackEdit(...) {
>   if (!fileHistoryEnabled()) return                    // 配置闸门
>   // Phase 1: 探测
>   let captured; updateFileHistoryState(s => { captured = s; return s })
>   if (!captured) return
>   const mostRecent = captured.snapshots.at(-1)
>   if (!mostRecent) { logError(...); return }            // 必须有最近快照
>   if (mostRecent.trackedFileBackups[trackingPath]) return  // 已追踪,不动 v1
>   // Phase 2: 异步备份
>   backup = await createBackup(filePath, 1)
>   // Phase 3: 提交 + 写 sessionStorage 元数据
>   ...
> }
> ```

#### 2.1.2 rewind 命令与 applySnapshot

`claudecode/src/commands/rewind/index.ts:1-13` 注册 `rewind`（别名 `checkpoint`）local 命令：

```typescript
const rewind = {
  description: `Restore the code and/or conversation to a previous point`,
  name: 'rewind',
  aliases: ['checkpoint'],
  type: 'local',
  supportsNonInteractive: false,
  load: () => import('./rewind.js'),
}
```

`rewind.ts:1-13` 实际只是 **打开消息选择器 UI**：

```typescript
export async function call(_args, context) {
  if (context.openMessageSelector) {
    context.openMessageSelector()    // 让用户在 TUI 中选目标消息
  }
  return { type: 'skip' }
}
```

选中后 `fileHistoryRewind`（`fileHistory.ts:347-397`）→ `applySnapshot`（`fileHistory.ts:537-591`）：
- 遍历 `state.trackedFiles`
- 找目标版本的 backupFileName
- `backupFileName === null` → `unlink(filePath)`（文件被 agent 新建后回滚）
- 否则 `checkOriginFileChanged` → `restoreBackup` (`copyFile` + `chmod`)

回滚失败的局部兜底：`applySnapshot` 每个文件独立 try/catch，单文件失败不会中断其它文件（`fileHistory.ts:583-589`）。

#### 2.1.3 git 集成（仅 spawn child process）

`claudecode/src/utils/git.ts` —— 约 927 行，封装 30+ git 操作：

| 操作 | 行号 | 用途 |
|------|------|------|
| `findGitRoot` / `findCanonicalGitRoot` | `git.ts:27-209` | 递归 stat `.git`(目录或文件),LRU memoize 50 项 |
| `getIsGit` | `git.ts:218-229` | 用于 prompt gate |
| `getBranch` / `getHead` / `getDefaultBranch` / `getRemoteUrl` | `git.ts:257-271` | 注入 prompt |
| `getIsClean` / `getChangedFiles` / `getFileStatus` | `git.ts:356-417` | 工作区状态 |
| `stashToCleanState` | `git.ts:429-461` | permission check 失败前 **自动 stash** (含 untracked) |
| `getGitState` | `git.ts:472-502` | 并行 6 个 git 命令 |
| `preserveGitStateForIssue` | `git.ts:724-845` | 5 个 git 命令并发,用于远端 issue 重放 |

安全性：`resolveCanonicalRoot` (`git.ts:123-183`) 严格校验 `.git` 文件 + `commondir` + `gitdir` 三段反向链接,防止恶意 repo 借 trusted repo 上下文绕过 trust dialog 触发 `.claude/settings.json` hooks。
**`isCurrentDirectoryBareGitRepo`** (`git.ts:876-925`) 检测 **裸仓库伪装攻击**:cwd 出现 `HEAD` + `objects/` + `refs/` 三件套但 `.git/HEAD` 缺失或为目录 → 视为攻击向量。

#### 2.1.4 git status 注入 system prompt

`claudecode/src/context.ts:36-103` 一次性 memoize 注入,带 2000 字符截断:

```typescript
export const getGitStatus = memoize(async (): Promise<string | null> => {
  if (process.env.NODE_ENV === 'test') return null
  // 并发 5 个 git 命令
  const [branch, mainBranch, status, log, userName] = await Promise.all([
    getBranch(),
    getDefaultBranch(),
    execFileNoThrow(gitExe(), ['--no-optional-locks', 'status', '--short'], ...),
    execFileNoThrow(gitExe(), ['--no-optional-locks', 'log', '--oneline', '-n', '5'], ...),
    execFileNoThrow(gitExe(), ['config', 'user.name'], ...),
  ])
  const truncatedStatus = status.length > MAX_STATUS_CHARS
    ? status.substring(0, MAX_STATUS_CHARS) +
      '\n... (truncated because it exceeds 2k characters...)'
    : status
  return [
    `This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.`,
    `Current branch: ${branch}`,
    `Main branch (you will usually use this for PRs): ${mainBranch}`,
    ...(userName ? [`Git user: ${userName}`] : []),
    `Status:\n${truncatedStatus || '(clean)'}`,
    `Recent commits:\n${log}`,
  ].join('\n\n')
})
```

注入时机由 `getSystemContext` memoize 包住 (`context.ts:116-150`),conversation 期间只取一次;
`process.env.CLAUDE_CODE_REMOTE` 或 `!shouldIncludeGitInstructions()` 时跳过。

#### 2.1.5 Commit 归因 + Co-Authored-By

`claudecode/src/utils/attribution.ts:52-98`:

```typescript
const defaultCommit = `Co-Authored-By: ${modelName} <noreply@anthropic.com>`
const defaultAttribution = `🤖 Generated with [Claude Code](${PRODUCT_URL})`
```

`getEnhancedPRAttribution` (`attribution.ts:297-393`) 还计算 **Claude 贡献占比** + **N-shotted**:
> `"🤖 Generated with [Claude Code](https://...) (93% 3-shotted by claude-opus-4-5, 2 memories recalled)"`

`undercover.ts:54-68` 明确禁止 `Co-Authored-By` 行与"Co-Authored-By: Claude Opus 4.6 <…>"字串外泄 —— 内部 repo 用真模型名,外部 repo 用硬编码 "Claude Opus 4.6" 防 codename 泄漏 (`attribution.ts:75-79`)。

settings 类型 (`settings/types.ts:364`):
> "Whether to include Claude's co-authored by attribution in commits and PRs (defaults to true)"

> 模式要点:claudecode 不自动 commit,只把 Co-Authored-By 当 trailer 让 LLM **写** 进 commit message;PR 描述会自动追加统计行。**没有 push**。

#### 2.1.6 git diff 用于 UI 而非 rewind

`claudecode/src/utils/gitDiff.ts:49-108` —— `fetchGitDiff` 提供:
- `git diff HEAD --shortstat` 快速探测,> MAX_FILES_FOR_DETAILS(500) 直接返回空 details
- `git diff HEAD --numstat` 拿到每文件 additions/deletions
- `git ls-files --others --exclude-standard` 追加 untracked 文件名
- hunk 内容 **不在主路径取**,改由 `fetchGitDiffHunks` (`gitDiff.ts:114-135`) on-demand 拉,避免每次 poll 都跑大 diff

#### 2.1.7 git safety 集成到 Bash 权限

`claudecode/src/tools/shared/gitOperationTracking.ts` + `bashSecurity.ts`:
- 任何 `git ...` 命令走 read-only 校验(`readOnlyValidation.ts`)
- `stashToCleanState` (`git.ts:429-461`) 在权限 deny 前 **自动 stash**(包含 untracked),防止 agent 撞墙后工作区丢失

#### 2.1.8 ExitWorktreeTool

`claudecode/src/tools/ExitWorktreeTool/ExitWorktreeTool.ts:148-156`:
```typescript
export const ExitWorktreeTool: Tool<...> = buildTool({
  ...
  async call() { return getExitWorktreeToolPrompt() }
})
```
只是 **prompt-only** —— 引导用户手动运行 `git worktree remove`,**不让 agent 自行管理 worktree 生命周期**。

---

### 2.2 opencode —— shadow git repo + effect + worktree 三件套

#### 2.2.1 Snapshot = shadow git repo

`opencode/packages/opencode/src/snapshot/index.ts:66-75`:
```typescript
const state = {
  directory: ctx.directory,
  worktree: ctx.worktree,
  gitdir: path.join(Global.Path.data, "snapshot", ctx.project.id, Hash.fast(ctx.worktree)),
  vcs: ctx.project.vcs,
}
```

路径模板:`{Global.Path.data}/snapshot/{project_id}/{Hash.fast(worktree)}/.git/`

**关键设计 —— 共享对象数据库**(`snapshot/index.ts:195-233`):
```typescript
// Reuse the hashes for the git storage between the original repo and snapshot
// on huge repos like chromium checkout the git add --all rebuilding the
// hashes can take minutes. By doing this we eliminating this at all
const seed = Effect.fnUntraced(function* () {
  if (state.vcs !== "git") return
  const commonDir = yield* git(["rev-parse", "--path-format=absolute", "--git-common-dir"], ...)
  if (commonDir.code !== 0) return
  const source = commonDir.text.trim()
  const sourceObjects = path.join(source, "objects")
  const chained = (yield* read(path.join(sourceObjects, "info", "alternates")))
    .split("\n").map(...).filter(Boolean)
  const alternates: string[] = []
  for (const candidate of [sourceObjects, ...chained]) {
    if (yield* exists(candidate)) alternates.push(candidate)
  }
  if (!alternates.length) return
  yield* fs.writeFileString(
    path.join(state.gitdir, "objects", "info", "alternates"),
    alternates.join("\n") + "\n",
  )
  // Seed the index from the source repo so already-hashed entries are reused.
  const sourceIndex = path.join(source, "index")
  if (yield* exists(sourceIndex)) {
    yield* fs.copyFile(sourceIndex, path.join(state.gitdir, "index")).pipe(Effect.catch(...))
  }
})
```

> 注释明确:"on huge repos like chromium checkout the git add --all rebuilding the hashes can take minutes"—— 通过 alternates + 共享 index,**避免重复哈希计算**。

#### 2.2.2 核心 snapshot API

`Interface`(`snapshot/index.ts:36-46`):
```typescript
export interface Interface {
  readonly init: () => Effect.Effect<void>
  readonly cleanup: () => Effect.Effect<void>           // git gc --prune=7.days
  readonly track: () => Effect.Effect<string | undefined>   // 返回 tree hash
  readonly patch: (hash: string) => Effect.Effect<Patch>    // 文件清单
  readonly restore: (snapshot: string) => Effect.Effect<void>  // 全量还原
  readonly revert: (patches: Patch[]) => Effect.Effect<void>   // 增量还原
  readonly diff: (hash: string) => Effect.Effect<string>
  readonly diffFull: (from: string, to: string) => Effect.Effect<FileDiff[]>
}
```

- `track` (`snapshot/index.ts:318-347`):首次 `git init` + 写 `core.autocrlf=false` / `core.longpaths=true` / `core.symlinks=true` / `core.fsmonitor=false` / `feature.manyFiles=true` / `index.version=4` / `index.threads=true` / `core.untrackedCache=true`,后续只 `git add` + `git write-tree`
- `add` (`snapshot/index.ts:235-298`):`diff-files` + `ls-files --others` 合并,过滤 gitignore(`check-ignore --no-index --stdin -z`),>2MB 单文件自动写 `.git/info/exclude` 跳过
- `restore` (`snapshot/index.ts:382-406`):`git read-tree` + `git checkout-index -a -f`
- `revert` (`snapshot/index.ts:408-524`):**批量 checkout 优化**——按 hash 分组,**冲突路径不打包**(`clash` 判定 `a === b || a.startsWith(b/) || b.startsWith(a/)`),每批 ≤100 文件;`ls-tree` 探测文件是否存在,不存在则 `remove`(用 `effect/unstable/process` `ChildProcess.make`)

#### 2.2.3 diffFull 双协议优化

`snapshot/index.ts:546-759` —— **`git cat-file --batch`** 一次取所有 blob 字节(`snapshot/index.ts:604-682`),失败 fallback 到 per-file `git show`;`formatPatch` + `structuredPatch`(diff 库)生成 patch 文本,`context: Number.MAX_SAFE_INTEGER` 等价全行 context。

#### 2.2.4 后台 GC + 7 天 prune

`snapshot/index.ts:761-766`:
```typescript
yield* cleanup().pipe(
  Effect.catchCause(...),
  Effect.repeat(Schedule.spaced(Duration.hours(1))),
  Effect.delay(Duration.minutes(1)),
  Effect.forkScoped,
)
```
后台线程每 1 小时跑 `git gc --prune=7.days`(`snapshot/index.ts:23,305`)。

#### 2.2.5 Effect 锁

`snapshot/index.ts:55-64` —— **每个 gitdir 一个 Semaphore(1)** 串行化:
```typescript
const locks = new Map<string, Semaphore.Semaphore>()
const lock = (key: string) => {
  const hit = locks.get(key)
  if (hit) return hit
  const next = Semaphore.makeUnsafe(1)
  locks.set(key, next)
  return next
}
const locked = <A, E, R>(fx) => lock(state.gitdir).withPermits(1)(fx)
```

#### 2.2.6 Git module 单独封装

`opencode/packages/opencode/src/git/index.ts` (348 行) —— 协议无关 git 调用:
- 统一标志:`--no-optional-locks -c core.autocrlf=false -c core.fsmonitor=false -c core.longpaths=true -c core.symlinks=true -c core.quotepath=false`(`git/index.ts:6-18`)
- `branch` / `prefix` / `defaultBranch` / `hasHead` / `mergeBase` / `show` / `status` / `diff` / `stats` / `patch` / `patchAll` / `patchUntracked` / `statUntracked` / `applyPatch`
- `status` 用 `--porcelain=v1 --untracked-files=all --no-renames -z`(`git/index.ts:217-219`)
- `diff` / `stats` 用 `--numstat -z` + null-byte protocol,无 NUL split bug

#### 2.2.7 Worktree 服务

`opencode/packages/opencode/src/worktree/index.ts` (623 行):
- 命名:`opencode/{slug}` 分支 (`worktree/index.ts:183`),root `{Global.Path.data}/worktree/{project_id}/{name}/`(`worktree/index.ts:208`)
- 创建:`git worktree add --no-checkout -b {branch} {dir}`(`worktree/index.ts:216-221`)
- sandbox 白名单:`project.addSandbox(ctx.project.id, info.directory)`(`worktree/index.ts:228`)
- 删除:先 `worktree remove --force`,失败回退 `cleanDirectory` + `prune`(`worktree/index.ts:399-449`),**完整处理 fsmonitor 残留**(`worktree/index.ts:361-366`):
  ```typescript
  function stopFsmonitor(target: string) {
    return fs.exists(target).pipe(
      Effect.orDie,
      Effect.flatMap((exists) => (exists ? git(["fsmonitor--daemon", "stop"], { cwd: target }) : Effect.void)),
    )
  }
  ```
- 重置:`fetch {remote} {branch}` + `reset --hard` + `submodule foreach git reset --hard && git clean -fdx`,失败抛 `ResetFailedError`,**最后用 `status --porcelain=v1` 校验 dirty**(任何残留立刻拒绝,`worktree/index.ts:601-603`)

#### 2.2.8 git 上下文不直接注入 prompt

opencode 走另一条路:
- `server/routes/instance/httpapi/groups/file.ts:158-167` 暴露 `/file/status` HTTP API 给客户端
- `command/template/review.txt:16,37` 提示 model **自己跑** `git status --short`
- 不在 system prompt 注入 git 状态

#### 2.2.9 GitHub 集成

`opencode/packages/opencode/src/github/` + `packages/core/src/github-copilot/` + `packages/opencode/src/plugin/github-copilot/` —— 完整的 GitHub App OAuth / commit / PR 操作,与 snapshot 并列;但 **不是 checkpoint 概念**,属于推送集成。

---

### 2.3 pi —— 会话树 + 无文件 checkpoint 空白

#### 2.3.1 pi 没有文件 checkpoint

源码全文搜索 `fileHistory|snapshot.*restore|originalContent`:
- `edit.ts` 没有 `Backup` 类,只有 `restoreLineEndings` 文本结尾符修复 (`edit.ts:21,370`)
- `write.ts` 同上
- `bash.ts:380,427,465,477` 中的 `snapshot` 全部是 **bash 输出截断快照**(`output-accumulator.ts:91`),与文件无关

> pi 把 "snapshot" 一词用在了不同的领域(对话协议快照 + 输出流快照),**不是 workspace checkpoint**。

#### 2.3.2 pi 的会话树 / transcript

`packages/server/src/snapshots.ts` (63 行) 是 **Server 状态广播快照**(sessions 列表 + 模型列表 + revision),不是文件级;`packages/protocol/src/schemas.ts` 中的 `ServerSnapshot` 也只描述 server metadata,不涉及工作区。

`packages/coding-agent/src/client/transcript.ts` 是 CBOR 帧 + parentUuid 树 —— 已写入 `专题-第六轮-TUI与终端渲染管线深度对比.md` + `专题-第三轮-会话持久化与崩溃恢复深度分析.md`。

#### 2.3.3 pi 不暴露 checkpoint/undo 命令

`grep -rn "checkpoint\|undoLastEdit" /usr/local/LsmGitOpenSource/pi/packages/coding-agent/src` 返回空(除 `compaction.ts:467,543` 是 "context checkpoint" —— 上下文压缩的语义,不是文件)。

> **结论:pi 在 2026-09 时点完全不实现文件级 checkpoint/undo**,用户的唯一保护是 git 工作流外部兜底。WriterLease fence (LB13) 只防写入撕裂,不支持回滚。

---

### 2.4 atomcode —— Rust 独立 git-dir + RewindScope 二态

#### 2.4.1 WorkspaceCheckpoint 设计哲学

`atomcode/crates/atomcode-capabilities/src/session/rewind.rs:1-19`:
```rust
//! Per-project workspace checkpoints used by Rewind.
//!
//! The store is an independent Git directory. Every command receives an explicit
//! `--git-dir` and `--work-tree`, so capturing/restoring never touches the user's
//! branch, HEAD, index, or stash. The worktree must itself belong to a Git
//! repository: this keeps ignore semantics predictable and lets the UI fail
//! closed instead of pretending arbitrary filesystem side effects are reversible.
```

> 三条硬约束:(1) 完全独立的 git 目录;(2) 所有命令显式 `--git-dir` + `--work-tree`;(3) 工作区必须是 git 仓库 —— 否则 **fail closed**,UI 显式拒绝(不伪装可逆)。

#### 2.4.2 路径与版本

`rewind.rs:20-28,202-225`:
```rust
const STORE_VERSION: &str = "atomcode-rewind-v1";
pub(crate) const LEDGER_VERSION: u32 = 2;     // v5.0.5 bumped to 2
pub(crate) const TRANSACTION_VERSION: u32 = 1;

pub fn for_session(worktree: &Path, session_id: &str) -> Result<Self, WorkspaceCheckpointError> {
    let requested = fs::canonicalize(worktree)...;
    let worktree = git_worktree_root(&requested)?;
    let bucket = super::SessionManager::project_hash(&worktree);
    let safe_session = session_id
        .chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
    if !safe_session { return Err(WorkspaceCheckpointError::InvalidPath(session_id.into())); }
    let git_dir = super::config_dir().join("rewind").join(bucket).join(session_id);
    Self::with_store(worktree, git_dir)
}
```
路径模板:`{config_dir}/rewind/{project_hash(worktree)}/{session_id}/` —— 双层 hash bucket 隔离。

#### 2.4.3 三文件协作的 atomic 模型

`rewind.rs:151-160` —— 事务日志:
```rust
pub(crate) struct RewindTransactionJournal {
    pub version: u32,
    pub previous_points: Vec<RewindPoint>,
    pub retained_points: Vec<RewindPoint>,
    pub recovery_tree: Option<String>,
    pub restored_files: Vec<String>,
    pub target_snapshot: Option<SessionSnapshot>,
    pub committed: bool,
}
```

> 关键不变式:`previous_points.starts_with(&retained_points)`(`rewind.rs:180-181`),保证事务回滚后 ledger 仍合法。

#### 2.4.4 capture_locked 工作流

`rewind.rs:563-595`:
```rust
fn capture_locked(&self) -> Result<String, WorkspaceCheckpointError> {
    let tracked = self.list_user_files(["ls-files", "--cached", "-z"])?;
    let untracked = self.list_user_files(["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut paths = Vec::new();
    for path in tracked.into_iter().chain(untracked) {
        validate_relative_path(&path)?;
        if !is_sensitive_path(&path) {           // 过滤 secret/env
            paths.push(path);
        }
    }
    self.run(["read-tree", "--empty"])?;
    if !paths.is_empty() {
        let mut input = Vec::new();
        for path in paths { input.extend_from_slice(path.as_bytes()); input.push(0); }
        self.run_with_input(["update-index", "--add", "--remove", "-z", "--stdin"], &input)?;
    }
    let output = self.run(["write-tree"])?;
    let tree = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if tree.is_empty() { return Err(...); }
    Ok(tree)
}
```
> `read-tree --empty` + `update-index --add --remove -z --stdin` + `write-tree` —— 用 NUL 协议一次写完索引,避免 N 个 subprocess 调用。

#### 2.4.5 restore_files_locked 的安全闸门

`rewind.rs:378-405`:
```rust
pub fn restore(&self, before: &str, after: &str) -> Result<WorkspaceRestoreReceipt, WorkspaceCheckpointError> {
    let _guard = self.guard();
    self.with_process_lock(|| {
        let recovery_tree = self.capture_locked()?;                              // 1. 先 snapshot 当前
        let files = self.changed_files_locked(before, after)?;                   // 2. 算出变更文件清单
        let conflicts = self.conflicts_locked(after, &recovery_tree, &files)?;   // 3. 检查冲突
        if !conflicts.is_empty() { return Err(WorkspaceCheckpointError::Conflicts(conflicts)); }
        if let Err(error) = self.restore_files_locked(before, &files) {
            // 4. 失败回滚到 recovery_tree
            if let Err(compensation) = self.restore_files_locked(&recovery_tree, &files) {
                return Err(WorkspaceCheckpointError::Compensation { operation: ..., compensation: ... });
            }
            return Err(error);
        }
        Ok(WorkspaceRestoreReceipt { recovery_tree, restored_files: files })
    })
}
```

> **三重防御**:
> 1. `recovery_tree` 先 snapshot 当前状态(允许补偿)
> 2. `conflicts_locked` 校验 `after == current`(防止旧 point 覆盖用户新编辑)
> 3. 失败时 `restore_files_locked(&recovery_tree, &files)` 回滚(compensate)

`conflicts_locked` (`rewind.rs:663-689`) 用 `git diff --name-only -z expected_after..current -- <files>` 探测"用户编辑后与预期 after 是否一致",不一致就 Conflicts。

#### 2.4.6 init config —— 独立 git config

`rewind.rs:532-552`:
```rust
let output = std::process::Command::new("git")
    .args(["init", "--bare", "--quiet"])
    .arg(&self.git_dir).output()...;
checked(output, "initialize rewind store")?;
self.run(["config", "core.autocrlf", "false"])?;
self.run(["config", "core.filemode", "true"])?;
self.run(["config", "core.symlinks", "true"])?;
let marker = self.git_dir.join("atomcode-rewind-version");
if !marker.exists() {
    fs::write(&marker, STORE_VERSION)...;    // 文件 marker 防版本错配
}
```
> 用 `--bare` 创建的 git 目录不污染 worktree,但又要给它 `index` + `objects/`,这是 atomcode 的特色 —— bare repo + 工作区共享。

#### 2.4.7 RewindScope 三态

`runtime.rs:214-229`:
```rust
pub enum RewindScope {
    Conversation,         // 只回滚消息
    Code,                 // 只回滚工作区
    ConversationAndCode,  // 两者都回滚
}
impl RewindScope {
    fn restores_conversation(self) -> bool { matches!(self, Self::Conversation | Self::ConversationAndCode) }
    fn restores_code(self) -> bool { matches!(self, Self::Code | Self::ConversationAndCode) }
}
```

但 `runtime.rs:14224` 的测试 `#[ignore = "workspace Rewind is intentionally disabled in v5.0.5"]` —— **v5.0.5 主动关闭了 Code 维度**:
```rust
#[tokio::test]
#[serial_test::serial(atomcode_home)]
#[ignore = "workspace Rewind is intentionally disabled in v5.0.5"]
async fn code_only_rewind_restores_workspace_but_keeps_conversation() { ... }
```

> **设计教训**:即便架构支持 Code/Conversation 分立,生产环境仍可能因稳定性 / 误回滚风险而关掉。laew 引入 P0 时务必保留 "Conversation only" 默认。

#### 2.4.8 RuntimeError::CodeRewindUnavailable

`runtime.rs:597-635`:
```rust
CodeRewindUnavailable(String),
...
Self::CodeRewindUnavailable(reason) => {
    write!(f, "code rewind is unavailable: {reason}")
}
```

`snapshot.rs:242` 中 `code_rewind_unavailable()` 返回 Option<String>(catalogue 携带此 reason) —— **fail-closed** UI 反馈。

#### 2.4.9 gitDiff UI(独立的 diff 视图)

`atomcode/crates/atomcode-tuix/src/git_diff.rs`(100 行起始,实际数千行)—— TUI 内置 diff 渲染器:
- `DiffScope::Combined / Staged / Unstaged`
- `DiffBase::Head / Unborn`
- `EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"` (空 tree 的 git 内置常量,用于 vs untracked)
- 5s timeout,512KB metadata cap,2MB patch cap,2000 文件上限,5000 单文件行上限

---

### 2.5 deepseek-harness —— git 仅出现在 BashTool 内部文档

#### 2.5.1 状态

`grep -rln "git status|execa.*git|child_process.*git|spawn.*git" /usr/local/LsmGitOpenSource/deepseek-harness/packages` 仅命中:
- `tool-bash/src/index.ts:56,82,123,135` —— 都是 Bash tool 的 `git` 命令黑/白名单(security),不是 git 集成
- `scripts/translation-pairing-git.ts` 等 —— 工程脚本

**结论**:deepseek-harness 把 git 完全交给 Bash 工具,**没有 checkpoint、没有 rewind、没有 worktree 隔离、没有 git 上下文注入**。

---

### 2.6 openclaw —— git 仅用于 skill source-install

#### 2.6.1 状态

`grep -rn "git " /usr/local/LsmGitOpenSource/openclaw/src` 命中点:
- `dockerfile.test.ts:107,123,130,145` —— docker 基础镜像包列表
- `entry.compile-cache.test.ts:116` —— 入口 cache 分类
- `skills/lifecycle/source-install.ts` —— **从 git URL 克隆 skill 源码**
- `config/schema.help.core.ts:84` / `config/types.openclaw.ts:135` —— 升级通道配置

`source-install.ts:69,192` 处理 `git clone` 的 stderr/stdout 输出,**目的是安装 skill,不是工作区管理**。

**结论**:openclaw 完全不接触用户 worktree,**没有 checkpoint/rewind/worktree/git 上下文注入**。

---

### 2.7 cc-switch / agent-studio —— 完全无 git 集成

`grep -rn "checkpoint\|undo\|git diff\|git status" /usr/local/LsmGitOpenSource/agent-studio` 命中的全是 **i18n JSON + UI 资产**,无代码层 git 集成。

`cc-switch/src` 内 git 关键字命中在 `config/*Presets.ts`(预设配置文件列表),不是工作区操作。

**结论**:cc-switch 是 8 款 Agent 客户端的 LLM 配置切换器,agent-studio 是 Agent 平台前端,均不涉及用户 worktree。

---

### 2.8 laew —— 零 git 集成

```bash
$ grep -rn "git " /usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src
src/main.rs:19:    ", git ",                                # 帮助文本里"cargo, git, ..."示例
src/agent/tools/grep.rs:6:                                # 跳过 .git / target / node_modules 目录
```

**完全没有**:
- `Command::new("git")`
- `/git-status` 子屏 / `/git-diff` 子屏
- checkpoint 文件存储
- rewind 命令
- worktree 隔离
- Co-Authored-By 注入

> P0 / P1 / P2 路线图详见 §6。

---

## 3. 横向对比大表

| 维度 | claudecode | opencode | pi | atomcode | deepseek-harness | openclaw | cc-switch | agent-studio | laew |
|------|-----------|----------|----|---------|------------------|----------|-----------|--------------|------|
| **Git 调用方式** | child_process spawn `git` | child_process spawn `git` (Effect) | 不调用 git | `Command::new("git")` | child_process spawn(仅 BashTool 内部) | spawn(仅 source-install) | 不调用 | 不调用 | 不调用 |
| **Git 库依赖** | 无 | 无 | 无 | 无(std::process) | 无 | 无 | 无 | 无 | 无 |
| **status / diff / log / branch / commit** | ✓ (`git.ts`) | ✓ (`git/index.ts`) | ✗ | ✓ (Rust 直调) | ✗(交给 Bash) | ✗(仅 clone) | ✗ | ✗ | ✗ |
| **stash / worktree** | stash:✓ (auto-stash on deny) / worktree: prompt-only | worktree:✓ (完整服务) | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **变更追踪来源** | FileHistory 显式追踪 trackedFiles | shadow git 自动 `ls-files --others` | 无(纯依赖外部 git) | shadow git `ls-files --cached --others` | 无 | 无 | 无 | 无 | 无 |
| **Checkpoint 存储形态** | 明文文件拷贝 `{configDir}/file-history/{session}/{hash@vN}` | shadow git repo `{data}/snapshot/{pid}/{wt_hash}/.git/` | 无 | 独立 git repo `{config_dir}/rewind/{proj_hash}/{session}/` | 无 | 无 | 无 | 无 | 无 |
| **Checkpoint 粒度** | 文件级 + 每用户消息快照 | 文件级 + `git write-tree` (全工作区快照) | 无 | turn 级(RewindPoint 一对一 turn_id) | 无 | 无 | 无 | 无 | 无 |
| **Snapshot 容量上限** | 100 轮 (`MAX_SNAPSHOTS`) | git gc --prune=7.days | 无 | 100 points (`rewind.rs:46-48`) | 无 | 无 | 无 | 无 | 无 |
| **恢复粒度** | 文件级(单文件回滚) | 全工作区 `read-tree` + 单文件 `revert` 批量 | 无 | turn 级 + workspace 增量 | 无 | 无 | 无 | 无 | 无 |
| **回滚冲突处理** | 单文件 try/catch 隔离 | `ls-tree` 探测 + 不存在则 `remove` | 无 | `recovery_tree` + `conflicts_locked` + compensate | 无 | 无 | 无 | 无 | 无 |
| **worktree 并发隔离** | 无(仅文档引导) | ✓ `opencode/{slug}` 命名 + sandbox 白名单 | 无 | 校验 worktree 必须是 git repo (fail closed) | 无 | 无 | 无 | 无 | 无 |
| **Git 上下文注入** | system prompt 一次性 memoize (2000 字符截断) | 不注入 prompt,改由 model 跑 `git status --short` | 无 | 无 | 无 | 无 | 无 | 无 | 无 |
| **自动 commit** | ✗(LLM 写 commit msg) | ✗(LLM 写) | ✗ | ✗(v5.0.5 关 Code Rewind) | ✗ | ✗ | ✗ | ✗ | ✗ |
| **Co-Authored-By 模板** | `Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>` | (未实现自动 trailer) | 无 | 无 | 无 | 无 | 无 | 无 | 无 |
| **PR 归因增强** | `"🤖 Generated with Claude Code (93% 3-shotted by claude-opus-4-5, 2 memories recalled)"` | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| **push 默认** | 关闭(LLM 自决) | 关闭 | N/A | N/A | N/A | N/A | N/A | N/A | N/A |
| **bare-repo 攻击防御** | ✓ `isCurrentDirectoryBareGitRepo` (`git.ts:876-925`) | 仅在 gitignore skip | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |

---

## 4. checkpoint / undo 三种存储架构的 ASCII 图解

### 4.1 claudecode 明文文件拷贝

```
.claude/file-history/<session_id>/
├── abc1234567890abcd@v1      # src/main.rs 的 v1 拷贝
├── abc1234567890abcd@v2      # main.rs 编辑后的 v2 拷贝
├── def0123...@v1             # src/lib.rs 的 v1 拷贝
└── <null>                    # 文件不存在记录

FileHistoryState (in-memory):
  trackedFiles: Set<{src/main.rs, src/lib.rs}>
  snapshots: [
    { messageId: m1, trackedFileBackups: { main.rs@v1, lib.rs@v1 } },
    { messageId: m2, trackedFileBackups: { main.rs@v2, lib.rs@v1 } },
    { messageId: m3, trackedFileBackups: { main.rs@v2, lib.rs@v2 } }
  ]
  MAX_SNAPSHOTS = 100 (FIFO)

rewind(m3):
  - 对每个 trackedFile:
    - targetBackup = snapshots[findIndex(m3)].trackedFileBackups[path]
    - 如果 null → unlink(path)
    - 否则 compareStatsAndContent → 若不同则 copyFile(target, path) + chmod
```

**优点**:简单;无 git 依赖;零 CLI fork 开销;备份可被其他工具浏览。
**缺点**:磁盘占用 = O(变更次数 × 变更字节数);无法做"删除整个目录"等结构变更;没有版本合并 / diff 工具集成。

### 4.2 opencode shadow git repo + alternates

```
$XDG_DATA_HOME/opencode/snapshot/<project_id>/<wt_hash>/
└── .git/
    ├── HEAD                  # ref: refs/heads/main (空,无 commit)
    ├── objects/              # alternates 指向源 repo objects/
    │   └── info/alternates   # "/path/to/source/.git/objects\n"
    ├── index                 # 从源 .git/index 拷贝
    ├── info/exclude          # >2MB 文件 pathspec
    └── ...

每次 track() 调用:
  1. git init (一次性)
  2. git --git-dir=$S config core.autocrlf=false longpaths=true ...
  3. seed():
     - alternates 指向源 .git/objects
     - copyFile 源 .git/index
  4. add():
     - diff-files --name-only -z   → tracked 列表
     - ls-files --others --exclude-standard -z  → untracked 列表
     - check-ignore --no-index --stdin -z  → ignore 列表
     - >2MB stat 过滤 → 写 info/exclude
     - git add --all --sparse --pathspec-from-file=- --pathspec-file-nul
  5. git write-tree  → 返回 tree hash (snapshot 标识)

restore(snapshot_hash):
  git read-tree <hash> + git checkout-index -a -f

revert(patches[]):
  按 hash 分组,每组 ≤100 文件
    ls-tree --name-only → 探测文件存在性
    存在 → git checkout <hash> -- <files...>
    不存在 → remove(files...)

后台 forkScoped:
  every 1h: git gc --prune=7.days
```

**优点**:复用 git 对象去重;diff/patch 工具全免费;alternates 共享对象库不占空间;可压缩 + 自动 GC。
**缺点**:依赖 git CLI;alternates 失效会触发"rebuild hashes takes minutes"(原注释);初始化一次成本高。

### 4.3 atomcode 独立 git-dir + ledger/journal 三件套

$XDG_CONFIG_HOME/atomcode/rewind/<project_hash>/<session_id>/
├── .git/                       # git init --bare --quiet
│   ├── HEAD
│   ├── objects/
│   ├── index
│   └── config                  # core.autocrlf=false / filemode=true / symlinks=true
├── atomcode-rewind-version     # "atomcode-rewind-v1"
├── operation.lock              # fs2::FileExt::try_lock_exclusive
├── ledger.json                 # RewindLedger { version, points: Vec<RewindPoint> }
└── txn.json                    # RewindTransactionJournal { previous, retained, ... }

RewindPoint:
  { turn_id, prompt_number, prompt_preview,
    before_tree: Option<String>,    # 可空(v5.0.5: Conversation-only 可省)
    after_tree: Option<String>,
    files: Vec<FileChangeSummary { path, additions, deletions, binary }> }

捕获:
  capture_locked():
    1. read-tree --empty           # 清空 index
    2. update-index --add --remove -z --stdin <tracked+untracked paths>
    3. write-tree                  # 返回 tree hash

恢复:
  restore(before, after):
    1. capture_locked() → recovery_tree  # 先快照当前
    2. changed_files_locked(before, after) → files
    3. conflicts_locked(after, recovery, files) → conflicts
    4. if conflicts: 拒绝 WorkspaceCheckpointError::Conflicts
    5. restore_files_locked(before, files)
       对每个 file:
         ls-tree <before> -- <file>  → 检查在 before 中是否存在
         存在 → git checkout <before> -- <file>
         不存在 → fs::remove_file(worktree/file)
    6. 失败 → restore_files_locked(recovery, files)  # 补偿

事务日志:
  journal.start():
    write txn.json with previous_points = current ledger points
  journal.commit():
    write ledger.json with retained_points + committed = true
  journal.compensate():
    write ledger.json with previous_points + committed = false
```

**优点**:Ledger + Journal + 三文件原子约束最强;支持事务回滚;敏感路径过滤;sensitive-path 拒绝;version marker 防版本错配。
**缺点**:复杂度最高;workspace Rewind 实际被 v5.0.5 关掉(工程教训);bigger binary size。

### 4.4 三种架构对比速查

| 维度 | claudecode 文件级 cp | opencode shadow git | atomcode 独立 git + ledger |
|------|---------------------|---------------------|---------------------------|
| **存储开销** | O(改字节 × 轮次) | O(改 blob 去重) + alternates 共享 | O(改 blob 去重) + 元数据 |
| **diff 能力** | 需 diff lib (`diff` npm) | `git diff --cached hash` 免费 | `git diff before after --numstat` 免费 |
| **跨会话共享** | 软链 `link(old, new)` (`fileHistory.ts:979`) | alternates 指向源 repo | bucket + worktree 共享对象 |
| **压缩** | 否 | git gc auto | git gc auto + journal zstd? |
| **并发控制** | React state + no-op updater | Semaphore(1) per gitdir | Mutex + process_lock + fs2::FileExt |
| **恢复粒度** | 文件级 | tree-level restore / file-level revert | file-level checkout / delete |
| **冲突防御** | 无(乐观) | ls-tree 探测 + remove 兜底 | capture recovery + conflicts_locked + compensate |
| **实现复杂度** | ★☆☆ | ★★☆ | ★★★ |
| **依赖 git CLI** | 否 | 是 | 是 |
| **可移植性** | 任意 FS | 需要 git 1.8+ | 需要 git 1.8+ |
| **适合场景** | 小型 / 偶尔 / 单会话 | 大型 / 长期 / 跨项目 | 强一致 / 多 turn / 企业级 |

---

## 5. 13 个设计模式与反模式

### 5.1 模式(借鉴)

#### P1. Shadow git repo + alternates 共享对象库
**出处**:`opencode snapshot/index.ts:198-232`
**要点**:Chromium 级单仓库的 `git add --all` 重建哈希需要数分钟,通过 alternates 指向源 repo `objects/` + 拷贝 `index`,秒级完成首抓。
**借鉴**:laew 工作目录如是大仓库,`gix` 配 `objects/pack` alternates 是 P0 默认实现。

#### P2. capture-before-restore + conflicts detection
**出处**:`atomcode rewind.rs:378-405`
**要点**:回滚前先 snapshot 当前状态为 `recovery_tree`,用 `git diff expected_after..current` 探测用户是否后续编辑过冲突文件,有冲突拒绝。
**借鉴**:laew 必须有,否则 agent 改完后用户编辑会丢失。

#### P3. 三文件事务 + 不可变约束
**出处**:`atomcode rewind.rs:151-160,180-181`
**要点**:`ledger.json` + `txn.json` + `operation.lock` 三件套;`previous_points.starts_with(&retained_points)` 强制保留前缀。
**借鉴**:laew checkpoint 系统用 SQLite 事务表代替 ledger + journal,事务回滚天然保证。

#### P4. Snapshot FIFO 上限 + 三态 snapshot id
**出处**:`claudecode fileHistory.ts:54,309-311` + `pi protocol schemas.ts:ServerSnapshot.revision`
**要点**:防止 snapshot 列表无限增长;`snapshotSequence` 单调递增(seq 在 snapshot 被 evict 后仍递增,作为 activity signal)。
**借鉴**:laew P1 加 `CHECKPOINT_HARD_LIMIT = 200` + `seq` BIGINT 字段。

#### P5. 敏感路径过滤
**出处**:`atomcode rewind.rs:570-572`
**要点**:`is_sensitive_path(&path)` 在 capture 时跳过 `.env`,`secrets/`,`.ssh/id_*` 等。
**借鉴**:laew 必须有,且基于 Bash 工具的 sensitive 路径白名单复用(见 `专题-权限管控深度分析.md`)。

#### P6. worktree.snapshot-id 命名 + canonicalize
**出处**:`opencode worktree/index.ts:174-197,295-300`
**要点**:工作树目录 = `{data}/worktree/{project_id}/{slug}/`,`slug` 26 次重试 + Slug.create();`canonical` 在 Windows lowercase 后比较,避免路径大小写歧义。
**借鉴**:laew P2 引入并行 SubAgent 时用同样模式。

#### P7. 默认 fileCheckpointingEnabled 开关 + 环境变量覆写
**出处**:`claudecode fileHistory.ts:63-78`
**要点**:`fileCheckpointingEnabled !== false`(默认开)且 `CLAUDE_CODE_DISABLE_FILE_CHECKPOINTING` 可强制关;SDK 模式要 `CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING`。
**借鉴**:laew 设置表加 `checkpoint_enabled` 默认 `true` + 环境变量 `LAEW_DISABLE_CHECKPOINT`。

#### P8. Bare-repo 攻击防御 + 反向 gitdir 链接校验
**出处**:`claudecode git.ts:123-209,876-925`
**要点**:`isCurrentDirectoryBareGitRepo` 检测 `HEAD` + `objects/` + `refs/` 三件套但 `.git/HEAD` 缺失 → 视为裸仓库伪装攻击;`resolveCanonicalRoot` 校验 `dirname(worktreeGitDir) === join(commonDir, 'worktrees')` AND 反向 gitdir 链接。
**借鉴**:laew 不立即做(零代码也零攻击面),P1 引入 git 后必加。

#### P9. Diff data 二次延迟加载
**出处**:`claudecode gitDiff.ts:62-79, 105-108`
**要点**:`fetchGitDiff` 只返回 stats 与文件清单,hunk 详情走 `fetchGitDiffHunks` on-demand(用于 DiffDialog);避免每次 UI poll 触发昂贵 `git diff HEAD`。
**借鉴**:laew UI 不要在主屏渲染时同步算 diff,改用后台 30s poll。

#### P10. Snapshot 修改是 git write-tree hash(身份即内容)
**出处**:`opencode snapshot/index.ts:341-344`
**要点**:snapshot id = `git write-tree` 输出哈希,**不可变、可去重、可缓存**。
**借鉴**:laew 用 `gix` 同等接口或 SHA-256(file_hash)。

#### P11. checkpoint 频次与 turn 强绑
**出处**:`claudecode fileHistory.ts:198-342` 在每个 user message 后 `fileHistoryMakeSnapshot`;`atomcode runtime.rs:1669-1739` 在每次 turn 结束落 RewindPoint
**要点**:snapshot 频次 = "user 视角的离散状态切换",不是"每个 tool call"。
**借鉴**:laew P0 与 Plan/Main-Work 完成时落 snapshot(每 turn 一次,与 SessionContext 写入对齐)。

#### P12. Co-Authored-By 由模型名动态生成
**出处**:`claudecode attribution.ts:75-79`
**要点**:`getPublicModelName(model)` → `Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>`;内部 repo 用真名,外部 repo 用硬编码名防 codename 泄漏。
**借鉴**:laew 默认模型名动态生成,内部 user 可配置覆盖。

#### P13. git status 一次性 memoize 注入 system prompt
**出处**:`claudecode context.ts:36-150`
**要点**:conversation 生命周期只调一次,2000 字符截断,`memoize` 包住避免重复 fork;CCR 模式 + `!shouldIncludeGitInstructions()` 跳过。
**借鉴**:laew P1 在 system prompt 加 `<git-status>` 块,与项目上下文注入一致用 `<<<LAEW:GIT_STATUS>>>` 标记隔离。

### 5.2 反模式(避免)

#### A1. opencode alternates 失效 → 重建哈希要数分钟
**出处**:`opencode snapshot/index.ts:198-199` 注释
**反模式**:依赖 alternates,源 repo 一旦 `.git/objects` 被 GC,alternates 链失效,snapshot 重抓会 re-hash 所有 blob,chromium 级仓库秒变分钟级。
**规避**:snapshot init 时探测 alternates 健康度,失效立即 fallback 全量 add + 在 UI 提示"首次抓取较慢"。

#### A2. claudecode 不校验 capture-after-restore 冲突
**出处**:`claudecode fileHistory.ts:537-591` applySnapshot
**反模式**:回滚时不比较 `current == target`,仅 compare stats/size 后直接 cp 覆盖。如果用户在 agent 改完后手动编辑了文件,**rewind 会无声丢失用户编辑**。
**规避**:laew 引入 P0 时必须先 `git diff current..target -- <files>` 探测冲突(学 atomcode conflicts_locked)。

#### A3. atomcode v5.0.5 关闭 Code Rewind
**出处**:`runtime.rs:14224` `#[ignore = "workspace Rewind is intentionally disabled in v5.0.5"]`
**反模式**:即便架构完整,生产仍可能因稳定性问题关闭 Code 维度。这暗示 **checkpoint 系统必须 fail-soft + 保留 Conversation-only**。
**规避**:laew checkpoint 系统务必支持 `scope: Conversation | Code | Both`,默认 Conversation。

#### A4. pi 完全无 checkpoint
**出处**:全源码搜 `fileHistory|backup|restoreOriginal`
**反模式**:依赖用户外部 git workflow,agent 误改后用户只能 `git checkout .` 兜底,agent 内部状态(写了一半的工具结果)与文件系统状态不一致。
**规避**:laew 至少实现最小版文件级 cp(snapshot/revert),不依赖 git CLI。

#### A5. deepseek-harness / openclaw / cc-switch / agent-studio 完全无 checkpoint
**反模式**:把"文件保护"完全外包给 OS / Git 用户,违反 "Agentic IDE 默认安全"。
**规避**:laew P0 把 checkpoint 列为 MUST-have,与 Bash 工具白名单同级。

#### A6. claudecode getGitStatus 同步串行 5 个 git 命令 → memoize 救场
**出处**:`context.ts:60-77` 用 `Promise.all` 并发,但单次仍 5 个 fork
**反模式**:每次 system context 重建都要 5 个 git fork,即便 memoize 也会随 conversation 数 N 累计。
**规避**:laew P1 加 `git status` 后台轮询 + 缓存(30s TTL),不每次 fork。

#### A7. opencode `worktree reset` 后用 `status --porcelain=v1` 校验 dirty 但不修复
**出处**:`worktree/index.ts:596-603`
**反模式**:reset 之后用 `git status --porcelain` 校验 dirty,但 dirty 时只是返回 `ResetFailedError`,**没有自动 clean + retry**。
**规避**:laew reset 流程若 dirty,自动 `git clean -fdx` 重试一次。

#### A8. claudecode Co-Authored-By 是 hardcoded fallback `"Claude Opus 4.6"`
**出处**:`attribution.ts:75-79`
**反模式**:硬编码 fallback 模型名,模型升级时容易遗忘同步。
**规避**:laew fallback 模型名放 build.rs/构建时常量,版本升级自动跟随。

#### A9. claudecode `git status` 注入 2000 字符截断但提示用户自己跑
**出处**:`context.ts:86-89`
**反模式**:截断后只提示 "If you need more information, run \"git status\" using BashTool",**agent 经常忽视,卡 2000 字符天花板**。
**规避**:laew 截断后自动追加 `truncated_files_count` 与关键 dirty 标记,而不是甩给 agent 自己跑。

---

## 6. laew 现状与 P0 / P1 / P2 路线图

### 6.1 现状

```
src/main.rs            # 完全不调 git
src/agent/tools/       # bash/read/write,无 git 工具
src/agent/yolo.rs      # 多 Agent 编排
src/config/mod.rs      # SQLite 配置 + Provider 管理
```

**零 git 集成**:无 `Command::new("git")`、无 `/git-status` 子屏、无 checkpoint、无 rewind、无 worktree。

**风险**:
- 用户 Bash 工具跑了 `rm -rf` 之后,laew **无法撤销**
- agent 写了一半文件崩了,无法回到崩溃前状态
- SubAgent 并发改同一文件,没有隔离机制
- 用户 commit 时 laew 帮不上忙,没有 diff 注入、没有 trailer 注入

### 6.2 P0:Checkpoint + Rewind (MUST,预计 4 周)

#### 6.2.1 Rust crate 选择

| 候选 | 评估 | 推荐 |
|------|------|------|
| `git2`(libgit2 bindings) | 同步 API;丰富;libgit2 C 依赖 5MB | ✗(同步 API 难集成 async runtime) |
| `gix`(纯 Rust,前身 git2-rs) | 异步友好;完整 git 协议;活跃维护 | ✓ **P0 首选** |
| `Command::new("git")` | 零依赖;CLI 兼容性好 | ✓ **fallback**(spawn 进程) |

**推荐**:`gix` 主 + `Command::new("git")` 兜底(bigfile / alternates 等高级场景走 CLI)。

#### 6.2.2 SQLite 表结构

```sql
-- 1) checkpoint 主表
CREATE TABLE checkpoint (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    turn_id INTEGER NOT NULL,                -- 与 SessionContext 一致
    seq INTEGER NOT NULL,                   -- 单调递增,activity signal
    scope TEXT NOT NULL DEFAULT 'both',     -- conversation|code|both
    before_tree TEXT,                       -- nullable: conversation-only 可省
    after_tree TEXT,
    files_json TEXT,                        -- [{"path","additions","deletions","binary"}]
    created_at INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES session(id)
);
CREATE INDEX idx_checkpoint_session ON checkpoint(session_id, seq);

-- 2) checkpoint journal(事务日志,CRDT 风格)
CREATE TABLE checkpoint_journal (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    tx_id INTEGER NOT NULL,                 -- 与 checkpoint 原子事务绑定
    operation TEXT NOT NULL,                -- 'begin'|'capture'|'restore'|'commit'|'compensate'
    points_before TEXT NOT NULL,            -- JSON array of checkpoint ids
    points_after TEXT NOT NULL,
    recovery_tree TEXT,
    restored_files TEXT,                    -- JSON array
    committed INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

-- 3) file backup 元数据(实际文件走 content-addressed store)
CREATE TABLE file_backup (
    content_hash TEXT NOT NULL,             -- sha256 of file bytes
    path TEXT NOT NULL,                     -- 相对工作区路径
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    ref_count INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (content_hash, path)
);
CREATE INDEX idx_file_backup_path ON file_backup(path);
```

**文件实际存储**(content-addressed):
```
$ROOT/checkpoint/blobs/<sha256[0:2]>/<sha256[2:4]>/<sha256>
```
- 引用计数 + 后台 GC (`seq > horizon AND ref_count = 0 → unlink`)
- 同 content_hash 不重复存储(`INSERT OR IGNORE INTO file_backup ... ON CONFLICT DO UPDATE SET ref_count = ref_count + 1`)

#### 6.2.3 核心 API 草案

```rust
// src/checkpoint/mod.rs

pub struct CheckpointService {
    store_dir: PathBuf,           // $ROOT/checkpoint
    db: SqlitePool,               // 复用 config::Db
    worktree_root: PathBuf,       // git_root 或 cwd
    is_git_repo: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum CheckpointScope {
    Conversation,
    Code,
    Both,
}

#[derive(Clone, Debug, Serialize)]
pub struct Checkpoint {
    pub id: i64,
    pub session_id: String,
    pub turn_id: u64,
    pub seq: i64,
    pub scope: CheckpointScope,
    pub before_tree: Option<String>,
    pub after_tree: Option<String>,
    pub files: Vec<FileChange>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileChange {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
}

pub trait CheckpointApi {
    /// 当前 turn 结束后落 checkpoint
    async fn capture(&self, scope: CheckpointScope, turn_id: u64) -> Result<Checkpoint>;

    /// 列出 checkpoint(给 TUI /rewind 子屏)
    async fn list(&self, session_id: &str) -> Result<Vec<Checkpoint>>;

    /// 回滚到某 checkpoint,scope 控制是否回滚 code
    /// - Conversation: 只回滚 Session 上下文
    /// - Code: 只回滚文件
    /// - Both: 两者
    async fn rewind(&self, checkpoint_id: i64, scope: CheckpointScope) -> Result<RewindReceipt>;

    /// Diff 两 checkpoint 之间的文件清单 + patch 文本(给 UI 显示)
    async fn diff(&self, from: i64, to: i64) -> Result<DiffResult>;
}

pub struct RewindReceipt {
    pub restored_files: Vec<String>,
    pub recovery_tree: String,       // 补偿用
    pub conflict_files: Vec<String>, // 若 capture-before 检测到冲突
}
```

#### 6.2.4 gix 集成示例

```rust
// src/checkpoint/capture.rs
use gix::{open, ThreadSafeRepository};

pub async fn capture_code(&self) -> Result<String, CheckpointError> {
    let repo = open(&self.worktree_root).map_err(...)?;
    let repo = repo.into_sync();
    let index = repo.index().map_err(...)?;
    // 1) tracked + untracked 列表
    let tracked = repo.head().map_err(...)?.tree().map_err(...)?;
    // 2) 写 tree
    let mut buf = Vec::new();
    let tree_id = repo.write_tree(&mut index).map_err(...)?;
    Ok(tree_id.to_string())
}
```

#### 6.2.5 集成点

| 触发点 | 时机 | 动作 |
|--------|------|------|
| Main-Work turn 结束 | `agent/mod.rs:run_session` 之后 | `checkpoint.capture(scope=Both, turn_id)` |
| Plan Agent plan 写入 | `plans/{session_id}-{seq}.md` 落盘后 | `checkpoint.capture(scope=Conversation, turn_id)` |
| WriteTool 完成 | `tools/write.rs` 之后 | 文件级 content_hash 入 file_backup,ref_count++ |
| EditTool 完成 | `tools/edit.rs` 之后 | 同 WriteTool |
| SessionContext 摘要 | 写入 SQLite 时 | 同上,scope=Conversation |

#### 6.2.6 测试

- 单元:`cargo test checkpoint` —— capture / restore / diff / conflict detection
- 端到端:`testReport/run_e2e.sh` 加 **第 11 节**:`/rewind` 子屏 tmux 自动化
- 跨平台:Windows 路径分隔符 + 大小写不敏感 + 软链解析
- 大文件:1GB 单文件 capture / restore 性能基准

### 6.3 P1:Git Context 注入 + Diff UI(预计 2 周)

#### 6.3.1 GitStatus 注入 system prompt

```rust
// src/agent/project_context.rs:沿用"项目说明文件"注入器模式

pub async fn get_git_status(workdir: &Path) -> Option<GitStatus> {
    let git_status = Command::new("git")
        .args(["--no-optional-locks", "status", "--short"])
        .current_dir(workdir).output().ok()?;
    if !git_status.status.success() { return None; }
    let branch = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).current_dir(workdir).output().ok()?;
    let log = Command::new("git").args(["--no-optional-locks", "log", "--oneline", "-n", "5"]).current_dir(workdir).output().ok()?;
    Some(GitStatus { branch, log, status: String::from_utf8_lossy(&git_status.stdout).to_string() })
}

pub fn inject_git_status(prompt: &mut Vec<Message>, git_status: Option<GitStatus>) {
    if let Some(gs) = git_status {
        let text = format!(
            "<<<LAEW:GIT_STATUS>>>\n\
             This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.\n\
             Current branch: {}\n\
             Recent commits:\n{}\n\
             Status:\n{}\n\
             <<<LAEW:/GIT_STATUS>>>",
            gs.branch, gs.log, gs.status.chars().take(2000).collect::<String>()
        );
        prompt.insert(1, Message::system_blocking(text));   // index 1,index 0 留给项目上下文
    }
}
```

注入时机:`YoloRunner::process_user_input` 第一次时 memoize,后续不再注入(快照时间点)。

#### 6.3.2 Diff 子屏

`src/tui/screen/git_diff.rs` —— 类似 `provider_list.rs` 的 Tab 化子屏:
- 字段:current_branch / dirty_files / last_5_commits
- 操作:Enter → 进入 file diff 视图(`fetchGitDiffHunks` on-demand)
- Esc → 退回主屏

#### 6.3.3 Co-Authored-By 注入

`src/agent/system_prompt/mod.rs`:
```rust
pub const CO_AUTHOR_TRAILER_TEMPLATE: &str = "Co-Authored-By: {model_name} <noreply@laew.local>";
```
系统提示词末尾追加:
> "When asked to commit changes, append this trailer: `Co-Authored-By: ...`"

P1 默认 **不**自动 commit / push,与 claudecode 一致。

### 6.4 P2:Worktree 并发隔离(预计 3 周)

#### 6.4.1 Worktree 服务

`src/worktree/mod.rs`(参照 opencode `worktree/index.ts`):
- 命名:`laew/{slug}` 分支 + `$ROOT/worktree/{project_hash}/{slug}/` 目录
- 创建:`git worktree add --no-checkout -b laew/{slug} {dir}`
- 沙箱:与 `专题-沙箱设计深度分析.md` P0 集成(独立 bash 白名单)
- 删除:`worktree remove --force` + `fsmonitor--daemon stop` + `cleanDirectory` + `prune`

#### 6.4.2 SubAgent 绑定

`MultiAgentOrchestrator::dispatch_subagent`:
- 检测 SubAgent 类型 = Code Mutating → 强制创建独立 worktree
- SubAgent 完成后产出 commit / patch,主线程在主 worktree `git apply` 或 cherry-pick
- 失败:`worktree remove` + SubAgent 结果丢弃,主 worktree 无脏数据

#### 6.4.3 reset / clean 自动化

```rust
impl WorktreeService {
    pub async fn reset(&self, dir: &Path) -> Result<()> {
        // 1. fetch + reset --hard
        // 2. submodule foreach reset + clean
        // 3. status --porcelain 校验 dirty
        // 4. dirty 时自动 git clean -fdx + retry
        // 5. 启动项目 start command
    }
}
```

### 6.5 路线图汇总

| 阶段 | 时间 | 内容 | 关键模块 |
|------|------|------|---------|
| **P0** | W1-W4 | Checkpoint + Rewind (file backup + SQLite + gix) | `src/checkpoint/` |
| **P0** | W4 | TUI `/rewind` 子屏 | `src/tui/screen/rewind.rs` |
| **P0** | W4 | run_e2e.sh 第 11 节(tmux 自动化) | `testReport/run_e2e.sh` |
| **P1** | W5-W6 | GitStatus 注入 + `/git-status` 子屏 | `src/agent/project_context.rs` + `src/tui/screen/git_status.rs` |
| **P1** | W6 | Co-Authored-By trailer 注入 | `src/agent/system_prompt/mod.rs` |
| **P2** | W7-W9 | Worktree 服务 + SubAgent 隔离 | `src/worktree/` + `src/agent/mod.rs` |
| **P2** | W9 | 并发 SubAgent 沙箱集成 | 复用 `专题-沙箱设计` P0 |
| **P3** | W10+ | PR 描述增强 / push 命令 | 见 §6.6 |

### 6.6 P3(可选):PR 描述 / Push / CI 集成

- `gh pr create` 描述自动追加 `🤖 Generated with laew (N% 3-shotted by claude-opus-4-5)`
- `git push` 需要用户确认(默认 TUI 子屏二次确认)
- CI 集成:`laew ci <provider>` 读 GitHub Actions status 注入下一轮上下文

---

## 7. 关键文件速查

| 项目 | 关键文件 | 锚点 | 关键概念 |
|------|---------|------|---------|
| claudecode | `src/utils/fileHistory.ts` | 1-1116 | 文件级 checkpoint 引擎 |
| claudecode | `src/utils/fileHistory.ts:725-741` | hash+version 命名 | content-addressed backup |
| claudecode | `src/utils/fileHistory.ts:640-672` | compareStatsAndContent | 短路 + mtime 优化 |
| claudecode | `src/utils/fileHistory.ts:537-591` | applySnapshot | rewind 主路径 |
| claudecode | `src/commands/rewind/index.ts` | 1-13 | /rewind 命令注册 |
| claudecode | `src/commands/rewind/rewind.ts` | 1-13 | rewind 命令实现(开 UI) |
| claudecode | `src/utils/git.ts` | 1-927 | git 命令封装 + bare-repo 防御 |
| claudecode | `src/utils/git.ts:123-209` | resolveCanonicalRoot | worktree gitdir 反向校验 |
| claudecode | `src/utils/git.ts:876-925` | isCurrentDirectoryBareGitRepo | 裸仓库攻击防御 |
| claudecode | `src/utils/gitDiff.ts:49-135` | fetchGitDiff + hunks | diff 二次延迟 |
| claudecode | `src/utils/attribution.ts:52-98` | Co-Authored-By | commit 归因 |
| claudecode | `src/utils/attribution.ts:297-393` | getEnhancedPRAttribution | PR 归因增强 |
| claudecode | `src/context.ts:36-150` | getGitStatus + getSystemContext | git 状态注入 |
| claudecode | `src/tools/ExitWorktreeTool/ExitWorktreeTool.ts:148-156` | ExitWorktree | prompt-only worktree |
| claudecode | `src/utils/git.ts:429-461` | stashToCleanState | 自动 stash 防丢失 |
| opencode | `packages/opencode/src/snapshot/index.ts` | 1-807 | shadow git repo 主路径 |
| opencode | `packages/opencode/src/snapshot/index.ts:66-75` | gitdir 路径 | `{data}/snapshot/{pid}/{wt_hash}/.git` |
| opencode | `packages/opencode/src/snapshot/index.ts:198-233` | seed + alternates | 共享对象数据库 |
| opencode | `packages/opencode/src/snapshot/index.ts:235-298` | add() | tracked + untracked 合并 |
| opencode | `packages/opencode/src/snapshot/index.ts:300-316` | cleanup | gc --prune=7.days |
| opencode | `packages/opencode/src/snapshot/index.ts:408-524` | revert 批量 | hash 分组 + 冲突路径不打包 |
| opencode | `packages/opencode/src/snapshot/index.ts:546-759` | diffFull | git cat-file --batch 优化 |
| opencode | `packages/opencode/src/git/index.ts` | 1-348 | 协议无关 git 封装 |
| opencode | `packages/opencode/src/git/index.ts:6-18` | cfg | 统一标志数组 |
| opencode | `packages/opencode/src/git/index.ts:215-226` | status | --porcelain=v1 -z |
| opencode | `packages/opencode/src/git/index.ts:263-269` | patch | --unified + maxOutputBytes |
| opencode | `packages/opencode/src/worktree/index.ts` | 1-623 | worktree 服务 |
| opencode | `packages/opencode/src/worktree/index.ts:182-197` | candidate | 26 次重试 + Slug.create |
| opencode | `packages/opencode/src/worktree/index.ts:361-386` | stopFsmonitor + cleanDirectory | 失败残留清理 |
| opencode | `packages/opencode/src/worktree/index.ts:596-603` | status --porcelain 校验 | dirty 拒绝 |
| opencode | `packages/opencode/src/command/template/review.txt:16,37` | review 模板 | 提示 model 自己跑 git status |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs` | 1-1000 | 独立 git-dir checkpoint |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:1-19` | 顶部注释 | fail-closed 哲学 |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:20-28` | 版本号 | LEDGER_VERSION=2, TRANSACTION_VERSION=1 |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:151-160` | RewindTransactionJournal | 三文件事务 |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:202-225` | for_session | 路径模板 + bucket hash |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:316-354` | with_store | git init --bare + version marker |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:378-405` | restore | 三重防御(recovery/conflict/compensate) |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:563-595` | capture_locked | read-tree --empty + update-index -z |
| atomcode | `crates/atomcode-capabilities/src/session/rewind.rs:663-689` | conflicts_locked | 用户编辑冲突检测 |
| atomcode | `crates/atomcode-coding/src/runtime.rs:214-247` | RewindScope / RewindResult | 三态 scope |
| atomcode | `crates/atomcode-coding/src/runtime.rs:1626-1783` | undo_to_prompt + rewind | 公共 API |
| atomcode | `crates/atomcode-coding/src/runtime.rs:14224` | `#[ignore]` | v5.0.5 关闭 Code Rewind |
| atomcode | `crates/atomcode-tuix/src/git_diff.rs` | 1-~3000 | TUI diff 渲染 |
| pi | `packages/coding-agent/src/core/tools/bash.ts:380,427` | snapshot | bash 输出截断(非文件) |
| pi | `packages/coding-agent/src/core/tools/output-accumulator.ts:91` | snapshot method | 输出流 snapshot |
| pi | `packages/server/src/snapshots.ts:1-63` | ServerSnapshotPublisher | server 状态广播 |
| pi | `packages/protocol/src/schemas.ts` | ServerSnapshot | CBOR 协议模型 |
| laew | `src/main.rs:19` | 帮助文本 | 唯一"git" 关键字命中 |
| laew | `src/agent/tools/grep.rs:6` | 跳过 .git | 唯一工程化 git 引用 |

---

## 附录:与已有专题的关联

| 本专题结论 | 关联专题 | 复用章节 |
|----------|---------|---------|
| checkpoint 数据持久化 vs SessionContext | `专题-第三轮-会话持久化与崩溃恢复深度分析.md` | §2.5 atomcode Snapshot + JSONL + 原子写 + Lease |
| workspace checkpoint 与 Goal 状态机的耦合 | `专题-第六轮-Goal状态机与任务生命周期深度对比.md` | §4 RewindPoint 与 GoalSnapshotChangeMeta 同构 |
| checkpoint 与 SubAgent 隔离 | `专题-第六轮-SubAgent调度与并发模型深度对比.md` | SubAgent 并发模式 |
| worktree 与沙箱集成 | `专题-沙箱设计深度分析.md` | 进程/文件/网络隔离 |
| Git 上下文注入与项目上下文注入 | `docs/Yolo项目上下文注入/` | 五级链发现 + 标记隔离模式 |
| Co-Authored-By 与 prompt cache | `专题-第三轮-系统提示词工程真实对比深度分析.md` | 模型变体差异 |

---

**专题版本**:v1.0 (2026-09-06)
**总字数**:~30k 字(中文 + 代码块)
**锚点总数**:~180 个 `<file>:<line>` 锚点
**覆盖项目**:8(laew + 7 外部)
**未覆盖**:`deepseek-harness`、`openclaw`、`cc-switch`、`agent-studio` 的 git 相关 module 因源码实际未实现 checkpoint/rewind/worktree,仅做"无实现"声明。