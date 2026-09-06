# 专题-第七轮-Bash命令执行与PTY进程管理深度对比

> **本专题范围**：命令执行子系统本身——进程生命周期、PTY、输出采集、截断策略、超时与取消、并发与资源。
> 与其它专题的边界：`专题-第五轮-中断取消与后台任务深度分析.md` 覆盖取消原语与后台任务闭环，本专题不展开 abort signal 三家族；`专题-权限管控深度分析.md` / `专题-沙箱设计深度分析.md` 覆盖黑名单与 BubbleWrap/seccomp，本专题只在 BashTool 边界处引用。

---

## 目录

1. 结论速览
2. 逐项目剖析
   - claudecode：snapshot + sentinel-cwd + persistent TaskOutput
   - opencode (Bash V2 core)：Effect + ChildProcess + detached + forceKillAfter
   - pi：sanitize → 滚动缓冲 → 落盘 + tail truncate
   - atomcode：setsid + killpg(SIGKILL) + JobObject 等价
   - deepseek-harness：resolve(request) → ShellExecSpec + 6 个 CollectedOutput 字段
   - openclaw：execa + KillProcessTree(SIGTERM→3s→SIGKILL,group-aware)
3. 横向对比大表（16 行）
4. persistent shell 与 PTY 的架构图（ASCII）
5. 超时/kill 的正确姿势（三阶段 kill + 进程组）
6. 输出截断策略矩阵（6 项目 × 7 维度）
7. 12~18 个设计模式与反模式
8. laew 现状与 P0/P1/P2 路线图（含 BashTool 完整 Schema）
9. 关键文件速查

---

## 1. 结论速览

| 维度 | 结论 |
|------|------|
| 执行模型 | claudecode 是唯一用 **persistent shell** 的项目（snapshot 函数集复用 + cwd sentinel 文件）；其余五家全部 **一次性 spawn**（每次新进程，cwd/env 通过参数传入）。 |
| PTY | 只有 opencode 与 openclaw 通过 **`@lydell/node-pty`** 走 PTY（用于交互式终端/Tmux/Web TUI）；Bash 工具路径上几乎全部走 pipe，claudecode 主动归一 `2>&1` 后再做语义分析。 |
| 默认超时 | claudecode/opencode **120s**、pi **无默认**（schema 仅校验 `>0`）、atomcode **60s**、deepseek **由 policy 决定**、openclaw **由 policy 决定**。 |
| 上限 | claudecode/opencode **600s（10 分钟）**、atomcode **300s（5 分钟）**、pi **2^31-1 毫秒**、openclaw **由 `resolveSafeTimeoutDelayMs` 决定**。 |
| 进程组 | atomcode 强制 `setsid()` + `killpg(pgid, SIGKILL)`；openclaw 检测 pgid==pid 才 group-kill，否则只 direct pid；claudecode 走 `execa detached: true` + 三阶段 kill；opencode 走 `detached: true` + `forceKillAfter: 3s`；pi/dsh 走 `AbortSignal` 触发 SIGTERM → 超时。 |
| 输出截断 | claudecode **`BASH_MAX_OUTPUT_DEFAULT = 30_000` / `MAX_OUTPUT_UPPER_LIMIT = 150_000`**、opencode **1 MB**、pi **50 KB + 2000 lines**、atomcode **60 KB 上限**、openclaw **200 KB aggregated + 30 KB pending**、deepseek **`stdoutMaxBytes` per-call**。 |
| 输出文件落盘 | claudecode `>64MB 截断后落到 tool-results/<taskId>`；pi `>50KB 时 start write to /tmp/pi-bash-<id>.log`；opencode 用 `maxOutputBytes` 内存中截断 + 输出截断 marker。 |
| 二进制检测 | pi `sanitizeBinaryOutput` 把 `<=0x1F`(除 `\t\n\r`) 与 `0xFFF9~B` 全过滤；openclaw `createStreamingBinaryOutputSanitizer`；opencode/atomcode 未明示（走 `String.from_utf8_lossy`/`output.toString("utf8")`）。 |
| cwd 延续 | claudecode 唯一显式 `pwd -P >| /tmp/claude-<id>-cwd` 回写，**下次命令通过 snapshot + 文件回读**；其余每次 `Command::new(cwd)` 显式传。 |
| ANSI 处理 | pi `stripAnsi` + `sanitizeBinaryOutput` + `.replace(/\r/g, "")` 三件套；openclaw `truncateUtf16Safe` + `pty-dsr.ts` 过滤 DSR 请求；opencode 完全不做（信任模型/上层）。 |

---

## 2. 逐项目剖析

### 2.1 claudecode（BashTool + LocalShellTask + TaskOutput）

claudecode 是六家中**唯一**构建了「长期存活 shell 会话」抽象的项目。其核心机制分三层：

**Layer 1 — shell snapshot（一次会话内所有命令共享）**

`src/utils/bash/ShellSnapshot.ts:413` 的 `createAndSaveSnapshot` 通过 `binShell -c -l snapshotScript` 把用户 `~/.bashrc` / `~/.zshrc` 完整 source 后，把所有函数、别名、shell 选项 dump 到 `$XDG_CONFIG_HOME/claude/shell-snapshots/snapshot-{zsh|bash}-<ts>-<rand>.sh`：

```ts
// /usr/local/LsmGitOpenSource/claudecode/src/utils/bash/ShellSnapshot.ts:362-385
const script = `SNAPSHOT_FILE=${quote([snapshotFilePath])}
    ${configFileExists ? `source "${configFile}" < /dev/null` : '# No user config file to source'}
    echo "# Snapshot file" >| "$SNAPSHOT_FILE"
    echo "unalias -a 2>/dev/null || true" >> "$SNAPSHOT_FILE"
    ${userContent}
    ${claudeCodeContent}
    if [ ! -f "$SNAPSHOT_FILE" ]; then
      echo "Error: Snapshot file was not created at $SNAPSHOT_FILE" >&2
      exit 1
    fi
```

`ShellSnapshot.ts:65-92` 还会在 snapshot 里塞入 `rg`、`find`、`grep` 的**argv0 dispatch** wrapper——bun 检测到 `ARGV0=rg /path/to/binary` 就走内嵌 ripgrep（不依赖系统 rg）。`ShellSnapshot.ts:181-191` 的 `getConfigFile` 按 shell 类型选 `~/.zshrc` / `~/.bashrc` / `~/.profile`，超时 `SNAPSHOT_CREATION_TIMEOUT = 10000`（10s，`ShellSnapshot.ts:24`）。

**Layer 2 — 每次执行前拼装 wrapper 命令**

`src/utils/shell/bashProvider.ts:77-198` 的 `buildExecCommand` 把 snapshot 路径、session env、disable-extglob、用户命令一起用 `&&` 串成一条 bash 命令，并**最后追加 `pwd -P >| /tmp/claude-<id>-cwd`** 作为 cwd sentinel：

```ts
// bashProvider.ts:181-187
commandParts.push(`eval ${quotedCommand}`)
// Use `pwd -P` to get the physical path of the current working directory for consistency with `process.cwd()`
commandParts.push(`pwd -P >| ${quote([shellCwdFilePath])}`)
let commandString = commandParts.join(' && ')
```

`bashProvider.ts:200-206` 的 `getSpawnArgs` 关键：snapshot 存在时跳过 `-l`（避免登录 shell 重复 source），缺失时回退 `-l`：

```ts
getSpawnArgs(commandString: string): string[] {
  const skipLoginShell = lastSnapshotFilePath !== undefined
  if (skipLoginShell) {
    logForDebugging('Spawning shell without login (-l flag skipped)')
  }
  return ['-c', ...(skipLoginShell ? [] : ['-l']), commandString)]
}
```

`snapshot` 文件被外部清理时的自愈：`bashProvider.ts:93-103` 通过 `access()` 检查，文件丢失时清空 `lastSnapshotFilePath`，下次回退 `-l` 走登录 shell 重新 init。

**Layer 3 — LocalShellTask 状态机**

`src/tasks/LocalShellTask/LocalShellTask.tsx:180-252` 的 `spawnShellTask` 在「后台执行」场景下：
- 注册 cleanup → graceful shutdown 时 `killTask`；
- `taskState` 初值 `isBackgrounded: true`；
- 通过 `shellCommand.background(taskId)` 切换 TaskOutput 状态；
- `shellCommand.result.then(...)` 在进程退出后 `flushAndCleanup` + `evictTaskOutput` + 通过 `enqueueShellNotification` 通知前端。

`LocalShellTask.tsx:259-287` 的 `registerForeground` 路径：执行长命令到 2s 后 UI 提示「切换后台」，用户点击则调 `backgroundTask` 把 `isBackgrounded: false → true`，同一进程不重启，TaskOutput 自动继续接收数据（`LocalShellTask.tsx:218-220` 注释：**Data flows through TaskOutput automatically — no stream listeners needed**）。

**Layer 4 — 输出持久化**

`src/tools/BashTool/BashTool.tsx:728-753` 的「大输出落盘」：当 `result.outputFilePath` 存在时把 stdout 拷到 `tool-results/<taskId>`；如果 > 64 MB (`MAX_PERSISTED_SIZE = 64 * 1024 * 1024`) 截断。`BashTool.tsx:424` 的 `maxResultSizeChars: 30_000` 是**结果持久化阈值**（小于它就不落盘）。

**超时与归一**：`src/utils/timeouts.ts:2-3` 是真实常量——`DEFAULT_TIMEOUT_MS = 120_000` / `MAX_TIMEOUT_MS = 600_000`，并通过 `BASH_DEFAULT_TIMEOUT_MS` / `BASH_MAX_TIMEOUT_MS` 两个 envvar 覆盖（`timeouts.ts:12-21`、`timeouts.ts:28-39`）。`BashTool.tsx:229` 的 schema 描述里直接模板字符串：`describe(\`Optional timeout in milliseconds (max ${getMaxTimeoutMs()})\`)`。

**输出字符上限**：`src/utils/shell/outputLimits.ts:3-4` 是真实常量——`BASH_MAX_OUTPUT_DEFAULT = 30_000`、`BASH_MAX_OUTPUT_UPPER_LIMIT = 150_000`，由 `BASH_MAX_OUTPUT_LENGTH` envvar 覆盖（`outputLimits.ts:6-14`）。

**环境注入**：`ShellSnapshot.ts:466` 在 snapshot 阶段注入 `CLAUDECODE=1`；`bashProvider.ts:228-234` 注入 `TMUX` 覆盖（指向 Claude 的隔离 socket）；`bashProvider.ts:249-251` 把 `/env` 设置的 session 变量透传给子进程。

**结论**：claudecode 的「persistent shell」不是真的长期 shell 进程，而是**函数集/别名/选项的快照复用 + cwd sentinel 文件**，每次新进程但环境等价于登录 shell。

---

### 2.2 opencode（Bash V2 core / Effect / ChildProcess / detached）

`/usr/local/LsmGitOpenSource/opencode/packages/core/src/tool/bash.ts` 是 207 行的精简 V2 实现，挂在 Effect 框架下，参数校验用 `Schema`。

**Schema**（`bash.ts:18-33`）：
```ts
export const name = "bash"
export const DEFAULT_TIMEOUT_MS = 2 * 60 * 1_000    // 120s
export const MAX_TIMEOUT_MS = 10 * 60 * 1_000        // 600s
export const MAX_CAPTURE_BYTES = 1024 * 1024         // 1 MB
export const Input = Schema.Struct({
  command: Schema.String.annotate({...}),
  workdir: Schema.String.pipe(Schema.optional).annotate({...}),
  timeout: PositiveInt.check(Schema.isLessThanOrEqualTo(MAX_TIMEOUT_MS))
    .pipe(Schema.optional).annotate({...}),
})
```

**执行**（`bash.ts:158-196`）：
```ts
const command = ChildProcess.make(input.command, [], {
  cwd: target.canonical,
  shell,                                     // /bin/sh 或 COMSPEC
  stdin: "ignore",
  detached: process.platform !== "win32",    // Unix 上 detached=true
  forceKillAfter: Duration.seconds(3),       // 3s 软超时后 SIGKILL
})
const result = yield* appProcess.run(command, {
  combineOutput: true,                       // stdout+stderr 合并
  timeout: Duration.millis(timeout),
  maxOutputBytes: MAX_CAPTURE_BYTES,         // 1 MB 内存中截断
})
```

`bash.ts:66-77` 留了一组 `// TODO` 标记，明确**当前 V2 是 minimal 边界**，尚缺：tree-sitter bash parser、BashArity 前缀批准、PowerShell/cmd 处理、plugin shell.env 注入、durable 后台任务状态、HTTP 后台任务观察、process-group cleanup 平台覆盖、二进制输出检测。

**输出呈现**（`bash.ts:51-57`）：错误文案是 `Command exited with code ${output.exit}.` 或 `Command timed out before completion.`；truncation 是 `result.outputTruncated ? "[output capture truncated at the in-memory safety limit]" : undefined`（`bash.ts:187-188`）。

**PTY 模块**：`/usr/local/LsmGitOpenSource/opencode/packages/core/src/pty/pty.ts`（25 行）定义了 `Proc` 接口，`pty.node.ts` 通过 `@lydell/node-pty` 实现 spawn (`pty.node.ts:6-29`)。bash 工具本身**不走 PTY**，PTY 主要给 TUI panel / Web 终端。

**结论**：opencode 是六家中**唯一一家内存截断 + 默认 1 MB 上限**的项目，比 claudecode 的 30 KB 大 33 倍，比 pi 的 50 KB 大 20 倍。

---

### 2.3 pi（truncateTail + 落盘 + tail output）

pi 把 Bash 执行拆成两层：

**Layer 1 — `BashOperations`（抽象 / SSH/容器）**

`/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/tools/bash.ts:1-161` 是 `createBashTool` 工厂，schema 只有 `command` + `timeout`（无 `cwd`，从 `context.env.cwd` 取）：
```ts
const bashSchema = Type.Object({
  command: Type.String({ description: "Bash command to execute" }),
  timeout: Type.Optional(Type.Number({ description: "Timeout in seconds (optional, no default timeout)" })),
});
const MAX_TIMEOUT_SECONDS = 2_147_483_647 / 1000;  // i32::MAX 毫秒 = ~24.8 天
const BASH_UPDATE_THROTTLE_MS = 100;
```

**输出节流**：`bash.ts:74-105` 用 `BASH_UPDATE_THROTTLE_MS = 100ms` 做 `setTimeout` 节流，**避免每次 chunk 都通知 UI**。`scheduleOutputUpdate` 计算 `delay = BASH_UPDATE_THROTTLE_MS - (Date.now() - lastUpdateAt)`，到点才调 `onUpdate`。

**Layer 2 — `executeBashWithOperations`（实际采集）**

`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/bash-executor.ts:50-156` 的核心循环：
```ts
const outputChunks: string[] = []
let outputBytes = 0
const maxOutputBytes = DEFAULT_MAX_BYTES * 2  // 100 KB 滚动上限
let totalBytes = 0
const decoder = new TextDecoder()

const onData = (data: Buffer) => {
  totalBytes += data.length
  // sanitize: stripAnsi + sanitizeBinaryOutput + replace \r
  const text = sanitizeBinaryOutput(stripAnsi(decoder.decode(data, { stream: true }))).replace(/\r/g, "")
  if (totalBytes > DEFAULT_MAX_BYTES) ensureTempFile()
  if (tempFileStream) tempFileStream.write(text)
  outputChunks.push(text)
  outputBytes += text.length
  while (outputBytes > maxOutputBytes && outputChunks.length > 1) {
    outputBytes -= outputChunks.shift()!.length
  }
  options?.onChunk?.(text)
}
```

关键设计：**`outputChunks` 是滚动缓冲（ring by 长度，不是环形数组）**——`while (outputBytes > maxOutputBytes)` 从头部 `shift()` 丢弃旧 chunk，保留尾部。`ensureTempFile` 在 `>50KB` 时启动 `/tmp/pi-bash-<id>.log` 完整落盘（`bash-executor.ts:64-74`，文件名 `pi-bash-${randomBytes(8).toString("hex")}.log`）。

**截断结果呈现**：`bash.ts:130-141` 文案：
```
[Showing last 12.5KB of line 1234 (line is 45.2KB). Full output: /tmp/pi-bash-abc.log]
[Showing lines 1234-2341 of 5621. Full output: /tmp/pi-bash-abc.log]
[Showing lines 1234-2341 of 5621 (50.0KB limit). Full output: /tmp/pi-bash-abc.log]
```

**截断真实常量**：`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/tools/truncate.ts:11-12`：
```ts
export const DEFAULT_MAX_LINES = 2000;
export const DEFAULT_MAX_BYTES = 50 * 1024;  // 50KB
```

**truncate 策略**（`truncate.ts:78-`）：`truncateHead` 保留首 N 行/字节；`truncateTail`（`truncate.ts:168-`）保留尾 N 行/字节，**遇到单行 > maxBytes 时保留该行末尾**（`lastLinePartial: true`）。

**结论**：pi 是六家中**唯一显式落盘路径回灌 + 三种截断分支文案**的项目。

---

### 2.4 atomcode（Rust / `tokio::process` + setsid + killpg）

`/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/tools/bash.rs` 是 4303 行的重型实现，核心 30~325 行清晰展示了 Rust 的进程治理：

**默认/上限**（`bash.rs:26-27`）：
```rust
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_TIMEOUT_SECS: u64 = 300;
```

**Schema**（`bash.rs:141-142`）：
```rust
"timeout": { "type": "integer", "description": "Max seconds to wait (default 60, max 300)" }
```

**强制 setsid + killpg**（`bash.rs:249-303`）：
```rust
cmd.current_dir(&ctx.working_dir)
   .stdin(std::process::Stdio::null())
   .stdout(std::process::Stdio::piped())
   .stderr(std::process::Stdio::piped())
   .kill_on_drop(true);   // 取消/超时 → SIGKILL 直接子进程

#[cfg(unix)]
unsafe {
  cmd.pre_exec(|| {
    detach_child_from_controlling_tty();   // setsid + TIOCNOTTY
    Ok(())
  });
}

let child = cmd.spawn()?;
let child_pid = child.id();
let wait = child.wait_with_output();

let kill_tree = || {
  #[cfg(windows)]
  crate::process_utils::kill_windows_tree(&job_guard, child_pid);
  #[cfg(not(target_os = "windows"))]
  if let Some(pgid) = child_pid {
    // SIGKILL 整组;kill_on_drop 已 SIGKILL 直接子进程
    unsafe { killpg(pgid as i32, SIGKILL) };
  }
};

tokio::select! {
  biased;
  _ = ctx.cancel.cancelled() => {
    kill_tree();
    err("bash: cancelled before completion.".to_string())
  }
  res = tokio::time::timeout(dur, wait) => match res {
    Ok(Ok(output)) => format_output(&output),
    Ok(Err(e)) => err(format!("bash: error running command: {e}")),
    Err(_) => {
      kill_tree();
      err(format!("bash: timed out after {secs}s — pass a larger `timeout` if this command is expected to run longer."))
    }
  }
}
```

**Windows 等价**（`bash.rs:276-289`）：Job Object + `KILL_ON_JOB_CLOSE`，关闭 handle 自动 reaps 进程树——这是 Rust 在 Windows 上相对 node 的杀手锏，**免去 taskkill /T 二次 spawn**。

**取消协同**（`bash.rs:30`）：注释明确「30 → 90 to tolerate legitimate silent」，把 `graceful cancel` 等待从 30s 提到 90s。

**环境注入**（`bash.rs:238-244`）：Windows 上强制 `PYTHONUTF8=1` + `PYTHONIOENCODING=utf-8` 防止 GBK 编码陷阱；同时关闭 console window flash。

**截断**：`bash.rs` 未给出独立的截断常量；但 `format_output` 之后由 agent 层（`atomcode-coding`）截断到 60 KB（待补查具体常量）。

**结论**：atomcode 是六家中**进程治理最严谨**的——强制 setsid、双平台 process-group reaper、kill_on_drop + killpg 双保险、askpass 注入、PYTHONIOENCODING locale 修复。

---

### 2.5 deepseek-harness（resolve → ShellExecSpec / 6 字段正交结果）

dsh 把 bash 执行做成了**完整的 Service Definition + Service Provider 架构**：

**Seam 定义**：`/usr/local/LsmGitOpenSource/deepseek-harness/packages/shell/shell/src/index.ts:65-104`：
```ts
export abstract class ShellExecutor extends Service {
  constructor(ctx: Context) { super(ctx, 'shell') }
  abstract resolve(request: ShellExecRequest): ShellExecSpec
  abstract run(spec: ShellExecSpec): Promise<ShellRunResult>
  abstract start(spec: ShellExecSpec): ShellProcess
}
```

**Request vs Spec**（`docs/subsystems/shell.md:24-99`）：Request 是模型/插件传入（workdir/timeoutMs/stdoutMaxBytes 可选），Spec 是 `resolve()` 之后所有字段必填。`stdin` 与 `env` 仅 in-process 插件可用，模型 bash tool **不暴露**。

**正交结果**（`docs/subsystems/shell.md:107-136`）：
```ts
interface ShellRunResult {
  exitCode: number | null            // null = signal 死亡
  signal: NodeJS.Signals | null
  timedOut: boolean                  // executor 自己的超时第一原因
  aborted: boolean                   // 调用方 AbortSignal 第一原因（与 timedOut 互斥）
  timeoutMs: number                  // 实际生效值
  stdout: CollectedOutput            // (text, 截断标记, spill 文件)
  stderr: CollectedOutput
  sandbox?: ShellSandboxInfo
}
```

**关键创新**：`timedOut` 与 `aborted` 是**互斥**的（first-cause 分类，`shell.md:121`），因为一个融合 deadline 同时驱动超时和取消。

**DSH_ENV_PREFIX**（`shell.md:11`）：harness 拥有 `DSH_*` 受管变量，`subprocess` 服务在合并时丢弃 ambient `DSH_*`，调用方 `env` 不能覆盖受管事实——是**类型化的环境隔离**。

**结论**：dsh 是六家中**抽象最干净**的——把 request/spec 正交化、把 sandbox/policy/managed-env 全部类型化、把 timeout 与 abort 用单一 deadline 融合。

---

### 2.6 openclaw（execa + KillProcessTree group-aware + 多模 timeout）

openclaw 的实现散布在 ~60 个 bash-tools.exec*.ts 文件中（`/usr/local/LsmGitOpenSource/openclaw/src/agents/bash-tools*.ts`），核心三个抽象：

**1. `ExecProcessRuntime` 与三阶段超时**（`bash-tools.exec-runtime.ts:86-91`）：
```ts
function resolveExecTimeoutMs(timeoutSec: number | null | undefined): number | undefined {
  if (typeof timeoutSec !== "number" || !Number.isFinite(timeoutSec) || timeoutSec <= 0) {
    return undefined;
  }
  return resolveSafeTimeoutDelayMs(timeoutSec * 1000);
}
```

**Failure 类目**（`exec-runtime.ts:136-144`）：
```ts
type ExecProcessFailureKind =
  | "shell-command-not-found"
  | "shell-not-executable"
  | "overall-timeout"        // 总时长超时
  | "no-output-timeout"      // 静默超时（额外维度）
  | "signal"
  | "aborted"
  | "runtime-error";
```

**`no-output-timeout` 是 openclaw 独有维度**——比 `overall-timeout` 更细粒度，命令还在跑但 N 秒没新输出则预警；`exec-runtime.ts:521-538` 给模型两种不同的错误文案：
- `"Command timed out after ${timeoutSec} seconds."` (overall-timeout)
- `"Command produced no output for N seconds..."` (no-output-timeout)

**2. 输出字符上限**（`exec-runtime.ts:110-122`）：
```ts
export const DEFAULT_MAX_OUTPUT = clampWithDefault(
  readEnvInt("OPENCLAW_BASH_MAX_OUTPUT_CHARS", "PI_BASH_MAX_OUTPUT_CHARS"),
  200_000, 1_000, 200_000,        // aggregate (默认 200 KB)
);
export const DEFAULT_PENDING_MAX_OUTPUT = clampWithDefault(
  readEnvInt("OPENCLAW_BASH_PENDING_MAX_OUTPUT_CHARS"),
  30_000, 1_000, 200_000,         // pending（默认 30 KB）
);
```

**3. PTY 路径**：`/usr/local/LsmGitOpenSource/openclaw/src/process/terminal-pty.ts:91-108` 通过 `@lydell/node-pty` spawn：
```ts
const { spawn } = await import("@lydell/node-pty");
const pty = spawn(invocation.file, invocation.args, {
  name: terminalName, cols: params.cols, rows: params.rows,
  cwd: params.cwd, env,
});
```

`terminal-pty.ts:127-144` 的 `killPtyTree` 把 `SIGKILL/SIGTERM` 路由到 `signalPtySessionTree(pty.pid, sig)`（**forkpty 进程组**）。

**4. group-aware 进程树 kill**：`/usr/local/LsmGitOpenSource/openclaw/packages/agent-core/src/harness/env/kill-tree.ts:31-64`：
```ts
export function killProcessTree(pid: number, opts?: KillProcessTreeOptions): void {
  const useGroupKill =
    opts?.detached === true || (opts?.detached !== false && isProcessGroupLeader(pid));
  if (opts?.force === true) {
    signalProcessTreeUnix(pid, "SIGKILL", useGroupKill);
    return;
  }
  const graceMs = normalizeGraceMs(opts?.graceMs);
  signalProcessTreeUnix(pid, "SIGTERM", useGroupKill);
  setTimeout(() => {
    const stillAlive = useGroupKill
      ? isProcessAlive(-pid) || isProcessAlive(pid)
      : isProcessAlive(pid);
    if (!stillAlive) return;
    signalProcessTreeUnix(pid, "SIGKILL", useGroupKill);
  }, graceMs).unref();
}
```

**核心创新**：`isProcessGroupLeader(pid)` 检测——Linux 走 `/proc/<pid>/stat`（避免 spawn ps 子进程），其他平台 `ps -p pid -o pgid=`。**只有当 PID 是 pgid leader 时才走 group kill**（`process.kill(-pid, ...)`），否则只 `process.kill(pid, ...)`——防止误杀网关进程组。

`DEFAULT_GRACE_MS = 3000`（kill-tree.ts:5）——SIGTERM 后 3 秒升级 SIGKILL。Windows 走 `taskkill /T /PID` 不带 `/F`，3s 后 `taskkill /F /T /PID`（`kill-tree.ts:301-329`）。

**5. `process.kill(-pid, ...)` 防御**：`signalUnixTarget`（`kill-tree.ts:262-267`）捕获 ESRCH 等错误，注释：**Already gone or not signalable; remaining exact targets still run**——不抛异常，best-effort。

**结论**：openclaw 是六家中**进程治理最完备**——SIGTERM→3s→SIGKILL、group-aware kill、PTY session tree、独立 no-output-timeout、fallback path、双平台 taskkill /T。

---

## 3. 横向对比大表（16 行）

| 维度 | claudecode | opencode | pi | atomcode | deepseek-harness | openclaw |
|------|-----------|----------|----|----------|------------------|----------|
| **执行模型** | persistent (snapshot+cwd sentinel) | one-shot spawn | one-shot spawn | one-shot spawn + setsid | one-shot (Service) | one-shot execa + PTY 双路径 |
| **PTY 工具** | 否（pipe+2>&1 合并） | `@lydell/node-pty`（仅 TUI/PTY 工具） | 否 | 否 | 否 | `@lydell/node-pty`（terminal 路径） |
| **PTY 用途** | 无 | TUI panel / 远程 shell | 无 | 无 | 无 | 网关 terminal / 交互式命令 |
| **默认超时** | 120_000 ms | 120_000 ms | 无默认 | 60s | 取决于 policy | 取决于 policy / `resolveSafeTimeoutDelayMs` |
| **最大超时** | 600_000 ms | 600_000 ms | i32::MAX ms | 300s | 取决于 cap | 由 `resolveSafeTimeoutDelayMs` 决定 |
| **进程组 kill** | execa `detached:true` | `detached:true` + `forceKillAfter:3s` | 通过 AbortSignal | `setsid()` + `killpg(pgid,SIGKILL)` | AbortSignal 触发 SIGTERM | group-aware（pgid==pid 才 group-kill）|
| **强制 SIGKILL** | execa `forceKillAfter` | `Duration.seconds(3)` | `child.kill()` | `kill_on_drop=true` + `killpg` | `start` 后台无超时 | 3s 宽限期后 SIGKILL |
| **输出字符上限** | 30 KB 默认 / 150 KB 顶 | 1 MB | 50 KB + 2000 lines | 60 KB (待补查) | `stdoutMaxBytes` per-call | 200 KB aggregate / 30 KB pending |
| **输出落盘路径** | `tool-results/<taskId>`（>64MB 截断） | 仅 in-memory marker | `/tmp/pi-bash-<hex>.log` | 待查 | spill 文件由 CollectedOutput 引用 | 待查 |
| **二进制处理** | `String.from_utf8_lossy` | `result.output.toString("utf8")` | `sanitizeBinaryOutput` (过滤 `<=0x1F` 非 tab/lf/cr) | UTF-8 lossy | UTF-8 text | `createStreamingBinaryOutputSanitizer` |
| **ANSI 处理** | 不主动 strip | 不主动 strip | `stripAnsi` + `.replace(/\r/g,"")` | 不主动 strip | 不主动 strip | `truncateUtf16Safe` + `stripDsrRequests` |
| **cwd 延续** | `pwd -P >\| /tmp/claude-<id>-cwd` | `cwd: target.canonical` 参数 | `env.cwd` 参数 | `current_dir(&ctx.working_dir)` | `spec.workdir` | `cwd: opts.cwd` |
| **shell 路径** | `~/.bashrc` / `~/.zshrc` 决定 | `Config.entries().shell ?? "/bin/sh"` | 由 `context.env.shell` 决定 | `bash -lc <cmd>` | 由 executor 配置 | `resolveTrustedWindowsCmdExe` / `/bin/sh` |
| **env 注入** | `CLAUDECODE=1` / `TMUX` / `/env` | （待 plugin） | 透传 `env` | `PYTHONUTF8=1` 等 | `DSH_*` 受管 + `stdin/env` 仅插件 | `markOpenClawExecEnv` |
| **截断方向** | head（按 30K 字符） | head（1MB marker） | tail（保留末尾 2000 行 / 50KB） | head | tail（per-call） | head（pending 30KB 限制）+ tail（aggregate 200KB） |
| **后台任务** | LocalShellTask / TaskOutput | TODO（V2 minimal） | SDK `runInBackground` | 无（每调用一次性） | `ShellProcess.start()` | ProcessSupervisor |

---

## 4. persistent shell 与 PTY 的架构图（ASCII）

### 4.1 claudecode 伪 persistent shell

```
┌─────────────────────────────────────────────────────────────┐
│ Session 启动:createAndSaveSnapshot(binShell)                │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ spawn `bash -c -l snapshotScript`  ← 一次性 source rc  │ │
│ │ source ~/.zshrc → declare -f → base64 → $SNAPSHOT_FILE  │ │
│ │ 把所有用户函数 / 别名 / shopt /  PATH 写到 /xdg/.../    │ │
│ │ snapshot-zsh-<ts>-<rand>.sh                             │ │
│ └─────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ BashTool.call(command)                                      │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ 1. snapshotPromise → access() 检查 (自愈)              │ │
│ │ 2. buildExecCommand:                                   │ │
│ │    `source <snapshot> 2>/dev/null || true`             │ │
│ │    `eval <user-command>`                               │ │
│ │    `pwd -P >| /tmp/claude-<id>-cwd` ← cwd sentinel     │ │
│ │ 3. spawn `bash -c <commandString>` (无 -l,已 source)   │ │
│ │ 4. 下次 BashTool 调用:从 <cwd> 文件回读新 cwd          │ │
│ └─────────────────────────────────────────────────────────┘ │
│ cwd 变化追踪:LocalShellTask → state.cwd                     │
└─────────────────────────────────────────────────────────────┘

关键点:
- 「persistent」只是函数集/别名复用,进程每次新
- cwd 靠文件 sentinel 回传,不是变量
- snapshot 缺失自愈 → 回退 -l 登录 shell
```

### 4.2 真 PTY vs pipe 抽象

```
                ┌─────────────────────────────────────┐
                │      Bash Tool                      │
                │     (model input)                   │
                └────────────────┬────────────────────┘
                                 │
                ┌────────────────┴──────────────────┐
                │                                   │
        ┌───────▼────────┐              ┌───────────▼──────────┐
        │  pipe path     │              │  PTY path            │
        │  (默认)        │              │  (interactive / web) │
        │                │              │                      │
        │ spawn(cmd,     │              │ @lydell/node-pty     │
        │  shell,stdin:  │              │  .spawn(file,args,   │
        │  "ignore",     │              │   {cols,rows,cwd,    │
        │  stdout:pipe,  │              │    env, name:term})  │
        │  stderr:pipe)  │              │                      │
        │                │              │ onData → sanitize    │
        │ capture        │              │  → 清洗 ANSI/DSR     │
        │  → stdout +    │              │  → 字节 vs 字符      │
        │    stderr 分离 │              │  → 颜色保持          │
        │                │              │                      │
        │ 优点:          │              │ 优点:                │
        │ - 干净字节流   │              │ - 终端交互 (vim, git │
        │ - 大缓冲       │              │   log, fzf)          │
        │ - 简单         │              │ - 颜色 / 宽度        │
        │                │              │                      │
        │ 缺点:          │              │ 缺点:                │
        │ - 无 ANSI      │              │ - 回显重复输出       │
        │ - 无颜色       │              │ - CRLF 转换          │
        │ - 进度条断     │              │ - DSR 请求噪声       │
        └────────────────┘              └──────────────────────┘
```

### 4.3 claudecode TaskOutput 数据流（背景化）

```
┌────────────┐     spawn bash -c      ┌─────────────────┐
│ BashTool   │ ─────────────────────► │  shell process  │
│ call       │     commandString      │  (detached)     │
└────┬───────┘                        └────────┬────────┘
     │                                         │
     │ startStallWatchdog()                    │ onData(chunk)
     │                                         ▼
     │                                ┌─────────────────┐
     │                                │  TaskOutput     │
     │                                │  / <taskId>     │
     │                                │  - 文件落盘     │
     │                                │  - 行计数       │
     │                                │  - 进度更新     │
     │                                └────────┬────────┘
     │                                         │
     │ UI 切后台 / 超时                       │ exit(code)
     ▼                                         ▼
┌─────────────────┐                ┌──────────────────┐
│ BackgroundHint  │   background()  │ enqueueShellNotif│
│ (UI 用户感知)   │ ◄────────────── │ ication(killed/  │
└─────────────────┘                 │  completed/      │
                                    │  failed)         │
                                    └──────────────────┘
```

---

## 5. 超时/kill 的正确姿势（三阶段 kill + 进程组）

### 5.1 阶段一：graceful cancel (SIGTERM)

**目的**：给程序一次自我清理机会（flush buffers、关连接、写 checkpoint）。

```rust
// atomcode bash.rs:295-310
let kill_tree = || {
  #[cfg(windows)]
  crate::process_utils::kill_windows_tree(&job_guard, child_pid);
  #[cfg(not(target_os = "windows"))]
  if let Some(pgid) = child_pid {
    unsafe { killpg(pgid as i32, SIGKILL) };  // ← 直接 SIGKILL,不优雅
  }
};
```

**注**：atomcode 在 kill 时**跳过了 SIGTERM**，直接 SIGKILL。原因是「killpg 已能保证无孤儿」，但这是**反模式**——若进程正在写文件，SIGKILL 会导致文件截断。**正确的姿势**应当是：

```ts
// openclaw 正确姿势（kill-tree.ts:31-64）
signalProcessTreeUnix(pid, "SIGTERM", useGroupKill);  // 阶段1
setTimeout(() => {
  if (!isProcessAlive(pid)) return;
  signalProcessTreeUnix(pid, "SIGKILL", useGroupKill);  // 阶段2
}, 3000).unref();
```

### 5.2 阶段二：进程组 kill（防孤儿）

**为什么必须用进程组**：单 `process.kill(pid)` 只杀直接子进程，`bash -c 'sleep 100'` 里的 `sleep` 子进程会**变孤儿**被 init 接管，下次 BashTool 调用可能看到 1 万个 `sleep` 进程。

**姿势 A — setsid 主动制造新进程组**（atomcode bash.rs:255-265）：
```rust
#[cfg(unix)]
unsafe {
  cmd.pre_exec(|| {
    detach_child_from_controlling_tty();  // setsid() + TIOCNOTTY
    Ok(())
  });
}
// 之后 killpg(pgid, SIGKILL) 一次清掉整组
```

**姿势 B — execa detached**（opencode bash.ts:163 / claudecode）：
```ts
const command = ChildProcess.make(input.command, [], {
  detached: process.platform !== "win32",  // Unix 上 detached=true
  forceKillAfter: Duration.seconds(3),
})
```

**姿势 C — group-aware 检测**（openclaw kill-tree.ts:46-47）：
```ts
const useGroupKill =
  opts?.detached === true || (opts?.detached !== false && isProcessGroupLeader(pid));
```

### 5.3 阶段三：硬杀 SIGKILL + 兜底

```rust
// atomcode
.kill_on_drop(true);  // dropping the wait future (cancel/timeout) SIGKILLs the child
```

```ts
// openclaw
setTimeout(() => {
  const stillAlive = useGroupKill
    ? isProcessAlive(-pid) || isProcessAlive(pid)
    : isProcessAlive(pid);
  if (!stillAlive) return;
  signalProcessTreeUnix(pid, "SIGKILL", useGroupKill);
}, graceMs).unref();
```

### 5.4 阶段四（兜底）：Windows Job Object

atomcode 的 Rust 在 Windows 上用 `assign_child_to_kill_on_close_job(&child)`（`bash.rs:288`），**drop guard 自动 reap**——这比 node + taskkill 优雅：

| 平台 | 优雅姿势 | 兜底姿势 |
|------|---------|---------|
| Unix | `setsid()` + `killpg(pgid, SIGTERM)` | `killpg(pgid, SIGKILL)` + `waitpid` reaper |
| Windows | `taskkill /T /PID` (不带 /F) | `taskkill /F /T /PID` + Job Object 自动 reap |

### 5.5 laew 的正确姿势（建议）

```rust
// laew agent/tools/bash.rs 应该改造为:
const GRACE_MS: u64 = 3000;

let mut cmd = Command::new("bash");
cmd.arg("-lc").arg(&command);
cmd.current_dir(current_work_dir());
cmd.stdin(Stdio::null());
cmd.stdout(Stdio::piped());
cmd.stderr(Stdio::piped());
cmd.kill_on_drop(true);

#[cfg(unix)]
unsafe {
  cmd.pre_exec(|| {
    libc::setsid();  // 制造新进程组
    Ok(())
  });
}

let child = cmd.spawn()?;
let pid = child.id().expect("pid");

let kill_tree = || unsafe {
  #[cfg(unix)]
  libc::killpg(pid as i32, libc::SIGTERM);
  #[cfg(windows)]
  // 用 taskkill /T /F <pid>
};

let kill_hard = || unsafe {
  #[cfg(unix)]
  libc::killpg(pid as i32, libc::SIGKILL);
  #[cfg(windows)]
  // taskkill /T /F <pid>
};

// 阶段1: 先 SIGTERM, 3s 后升级 SIGKILL
kill_tree();
tokio::spawn(async move {
  tokio::time::sleep(Duration::from_millis(GRACE_MS)).await;
  kill_hard();
});

cmd.wait().await?;
```

---

## 6. 输出截断策略矩阵

| 项目 | 默认上限 | 顶值 | envvar | 落盘 | 截断方向 | 文案模板 | 二进制处理 | ANSI 处理 |
|------|---------|------|--------|------|---------|---------|-----------|-----------|
| **claudecode** | 30_000 chars | 150_000 chars | `BASH_MAX_OUTPUT_LENGTH` | `tool-results/<taskId>`（>64MB truncate） | head | `Output truncated to 30000 chars. Full output: <path>` | UTF-8 lossy | 不主动 strip |
| **opencode** | 1_048_576 bytes | 1 MB（hardcoded）| 无 | 仅 in-memory marker | head | `[output capture truncated at the in-memory safety limit]` | `toString("utf8") \|\| "(no output)"` | 不主动 strip |
| **pi** | 50 KB / 2000 lines | 50 KB / 2000 lines | 无 | `/tmp/pi-bash-<hex>.log` | tail | `[Showing lines 1234-2341 of 5621 (50.0KB limit). Full output: /tmp/pi-bash-...log]` | `sanitizeBinaryOutput`（过滤 `<=0x1F`/`0xFFF9-B`）| `stripAnsi` + `.replace(/\r/g,"")` |
| **atomcode** | 60 KB（推测）| 待补查 | 无 | 无 | head | 待补查 | UTF-8 lossy | 不主动 strip |
| **deepseek-harness** | per-call `stdoutMaxBytes` | per-call | 无 | spill 文件（CollectedOutput 引用）| tail | `text` + `truncated:true` + `spillFile` | UTF-8 text | 不主动 strip |
| **openclaw** | 200_000 chars aggregate / 30_000 pending | 200_000 chars | `OPENCLAW_BASH_MAX_OUTPUT_CHARS` / `OPENCLAW_BASH_PENDING_MAX_OUTPUT_CHARS` | 待查 | head (pending) + tail (aggregate) | `truncateUtf16Safe` + `…` | `createStreamingBinaryOutputSanitizer` | `truncateUtf16Safe` + `stripDsrRequests` |
| **laew 现状** | 30_000 chars | 30_000 chars（hardcoded）| 无 | 无（仅 stderr 计数）| head | `...[stdout 截断,省略 N 字符]` | UTF-8 lossy | 不主动 strip |

**关键观察**：
1. **claudecode 与 opencode 默认值相差 33 倍**——claudecode 把模型成本当首要约束（30K ≈ 8K tokens），opencode 把内存当首要约束（1 MB 是 OS 单页）。
2. **pi 是唯一 truncate-tail**（保留末尾 2000 行 / 50KB），其余全部 truncate-head（保留开头）。**Tail 对调试日志/错误信息更友好**，head 对文件读更友好。
3. **openclaw 的 pending/aggregate 二级缓冲**是业界少见的设计——pending 给 UI 增量更新（30 KB），aggregate 给最终落盘（200 KB），解耦 UI 与存储。
4. **deepseek 把 stdoutMaxBytes 设计成 trusted plugin-only**（`shell.md:103`）——模型 bash tool 不暴露，但内部消费者可按需调大，是受控的灵活性。

---

## 7. 12~18 个设计模式与反模式

### 设计模式

**M1 — 渐进式取消（graceful → hard）**
- 案例：openclaw `SIGTERM → 3s → SIGKILL`（kill-tree.ts:53-63）
- 收益：给程序 flush 缓冲机会，避免文件截断；3s 后强杀兜底
- 适用：所有外部进程

**M2 — 进程组 kill 防孤儿**
- 案例：atomcode `setsid + killpg`（bash.rs:260-302）
- 收益：`bash -c 'sleep 100'` 里的 sleep 一起被 kill，不会泄漏到 init
- 适用：所有 spawn 长期 shell 路径

**M3 — cwd sentinel 文件**
- 案例：claudecode `pwd -P >| /tmp/claude-<id>-cwd`（bashProvider.ts:186）
- 收益：每次新进程但环境等价于登录 shell，无需长期 shell 进程
- 适用：snapshot 模式天然搭档

**M4 — 落盘路径回灌模型**
- 案例：pi `/tmp/pi-bash-<hex>.log`（bash-executor.ts:64-74）+ `Full output: <path>` 文案
- 收益：30 KB 给 LLM 看，500 KB 给模型自己 FileRead 看，零成本切换
- 适用：所有 truncate 工具

**M5 — 三档截断分支（last-line partial / by-lines / by-bytes）**
- 案例：pi bash.ts:130-141 三种 `truncatedBy` 文案
- 收益：模型从截断原因即可推断工具行为
- 适用：所有 truncate 模块

**M6 — pending/aggregate 二级缓冲**
- 案例：openclaw `DEFAULT_MAX_OUTPUT=200_000` + `DEFAULT_PENDING_MAX_OUTPUT=30_000`
- 收益：UI 更新走小缓冲（30 KB 足够），落盘走大缓冲（200 KB 足够）
- 适用：所有需要增量 UI + 全量落盘的工具

**M7 — no-output-timeout 单独维度**
- 案例：openclaw `failureKind: "no-output-timeout"`（exec-runtime.ts:139-141）
- 收益：`git clone` 卡 30 分钟但没新输出，提前预警；与 overall-timeout 解耦
- 适用：所有长任务工具

**M8 — request vs spec 分离**
- 案例：deepseek `ShellExecRequest`（可选字段）→ `ShellExecSpec`（必填）
- 收益：默认值与上限由 executor 自己决定，模型/插件接口稳定
- 适用：所有 capability seam 设计

**M9 — 受管环境变量类型化**
- 案例：deepseek `DSH_*` 前缀 + `DshEnvironmentKey` 枚举（shell.md:11）
- 收益：受管事实不被 ambient 覆盖，类型系统保证不变量
- 适用：所有需要注入 harness 内部状态的工具

**M10 — group-aware kill（pgid 检测后才走 group kill）**
- 案例：openclaw `isProcessGroupLeader(pid)`（kill-tree.ts:233-239）
- 收益：避免误杀网关进程组（detached:false 子进程）
- 适用：所有需要处理非 detached 进程的代码

**M11 — 文件大小截断而非行截断（harness）**
- 案例：claudecode `MAX_PERSISTED_SIZE = 64 * 1024 * 1024`（BashTool.tsx:732）
- 收益：1GB 落盘文件硬上限，避免磁盘爆炸
- 适用：所有落盘文件

**M12 — 流式节流（onChunk throttling）**
- 案例：pi `BASH_UPDATE_THROTTLE_MS = 100`（bash.ts:9）
- 收益：避免每 chunk 都通知 UI，UI fps 稳定
- 适用：所有流式输出工具

**M13 — forkpty session tree**
- 案例：openclaw `signalPtySessionTree(pid, sig)`（kill-tree.ts:88-127）
- 收益：PTY 进程组（forkpty 创建新 session）单独遍历 ps 子进程集合
- 适用：所有 PTY 工具

**M14 — 默认值与上限双 envvar**
- 案例：claudecode `BASH_DEFAULT_TIMEOUT_MS` + `BASH_MAX_TIMEOUT_MS`（timeouts.ts:13-39）
- 收益：用户可调默认（企业内部 5 分钟）和上限（保险）
- 适用：所有 timeout 类参数

### 反模式

**A1 — 没有进程组 kill**（laew 现状）
- 表现：`tokio::process::Command` 默认无 `kill_on_drop(true)`，超时后 `wait` 抛 `Elapsed` 但子进程仍在运行
- 后果：每超时一次，泄漏一个 bash + 子进程；100 次超时 = 100 个 zombie
- 修复：`cmd.kill_on_drop(true)` + `setsid()` + `killpg(pgid, SIGKILL)`

**A2 — 无超时上限**（pi 现状）
- 表现：`MAX_TIMEOUT_SECONDS = 2_147_483_647 / 1000` ≈ 24.8 天
- 后果：模型一次错误调用 `timeout: 999999999`，BashTool 卡 24 天把整台机器吃满
- 修复：`timeout` 必须 `<=` 一个合理上限（如 600_000ms）

**A3 — 无落盘路径回灌**（opencode / laew 现状）
- 表现：截断后只显示 marker，不告诉模型去哪看完整输出
- 后果：模型看到 30K 字符截断，下一步盲目重跑命令拿完整输出（多花 N 倍 token）
- 修复：截断时生成 `/tmp/<tool>-<hex>.log`，文案附 `Full output: <path>`

**A4 — stdout/stderr 完全合并丢失信息**（opencode `combineOutput: true`）
- 表现：`result.output.toString("utf8")` 单一字符串
- 后果：错误日志与正常输出混在一起，模型难以定位失败原因
- 修复：claudecode 走 `2>&1` 合并但保留行级 metadata（time/source）

**A5 — 不做 ANSI / DSR 清洗**（opencode / claudecode）
- 表现：`\x1b[?1h` (smkx) / `\x1b[?1l` (rmkx) 等控制字符原样传给模型
- 后果：模型上下文被 ANSI 转义污染，token 浪费
- 修复：pi 风格 `stripAnsi` + openclaw 风格 `stripDsrRequests` 双管齐下

**A6 — `cwd` 默认为 `process.cwd()` 而非项目根**（laew bash.rs:18-20）
- 表现：`fn current_work_dir() -> std::path::PathBuf { std::env::current_dir()... }`
- 后果：`laew -p "git status"` 在用户工作目录而非项目根执行，行为不一致
- 修复：把项目根从 Yolo project_context 注入到 BashTool 的 `cwd` 默认值

**A7 — 无 process group kill 但仍 `kill_on_drop`**（混合错误）
- 表现：直接 SIGKILL 直接子进程但不杀 grandchildren（`bash -c 'sleep 100'` 里的 sleep 仍存活）
- 后果：进程组泄漏，每次超时累积
- 修复：必须 `setsid` + `killpg`，二者缺一不可

**A8 — snapshot 文件无自愈**（snapshot-only 设计）
- 表现：snapshot 文件被 `/tmp` 清理，下次命令 env 丢失
- 后果：用户感觉「我的 alias 怎么不见了」
- 修复：claudecode 风格 `access()` 检查 + 回退 `-l` 登录 shell

**A9 — 截图/二进制污染模型上下文**（无 sanitize）
- 表现：`cat image.png` 或 `xxd file` 输出全推给模型
- 后果：10 MB 二进制数据变成 N 万 token
- 修复：pi `sanitizeBinaryOutput` 过滤控制字符 + `truncateTail`

**A10 — 默认超时硬编码不一致**
- 表现：claudecode 120s / atomcode 60s / pi 无默认 / opencode 120s
- 后果：跨项目迁移体验差
- 修复：行业共识 120s（2 分钟），envvar 可调

**A11 — 无并发上限**
- 表现：模型并发触发 1000 个 `git status`
- 后果：CPU 100%、内存爆炸
- 修复：claudecode `isConcurrencySafe()`（BashTool.tsx:434-436）+ 序列化队列

**A12 — 输出回传但无 truncateTail**（truncate head 模型看不到末尾错误）
- 表现：命令报错在末尾，head 截断后模型看不到
- 后果：模型误判命令成功，反复重试
- 修复：默认 tail 截断（保留末尾 N 行/字节），head 仅用于文件读

---

## 8. laew 现状与 P0/P1/P2 路线图

### 8.1 laew 现状（src/agent/tools/bash.rs）

| 维度 | 现状 |
|------|------|
| 模型 | `tokio::process::Command` + `bash -lc <command>` |
| 默认超时 | `DEFAULT_TIMEOUT_MS = 120_000` (2 分钟) |
| 上限 | `MAX_TIMEOUT_MS = 600_000` (10 分钟) |
| 输出字符上限 | `MAX_OUTPUT_CHARS = 30_000` (硬编码) |
| 截断方向 | head（保留首 30K） |
| 进程组 kill | ❌ 无 |
| kill_on_drop | ❌ 无（`tokio::time::timeout` 抛 Elapsed 后子进程仍在跑）|
| 落盘路径 | ❌ 无 |
| cwd 默认 | `std::env::current_dir()`（工作目录，非项目根）|
| env 注入 | 仅 `bash -lc` 自动 source `/etc/profile`/`~/.bashrc` |
| ANSI / 二进制 | ❌ 全 UTF-8 lossy 不清洗 |
| 并发上限 | ❌ 无 |

### 8.2 完整 BashTool Schema 与参数默认值表（建议 v0.2）

```rust
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 30_000;
const MAX_OUTPUT_CHARS: usize = 150_000;
const GRACE_MS: u64 = 3_000;

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "待执行的 bash 命令字符串"
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_TIMEOUT_MS as i64,
                "default": DEFAULT_TIMEOUT_MS,
                "description": "可选超时(毫秒),默认 120000,最大 600000"
            },
            "max_output_chars": {
                "type": "integer",
                "minimum": 1_000,
                "maximum": MAX_OUTPUT_CHARS as i64,
                "default": DEFAULT_MAX_OUTPUT_CHARS,
                "description": "可选输出字符上限,默认 30000,最大 150000;超过时落盘到 /tmp/laew-bash-<hex>.log"
            },
            "cwd": {
                "type": "string",
                "description": "工作目录;默认项目根(Yolo 注入)"
            },
            "description": {
                "type": "string",
                "description": "一句话描述命令用途(便于审计)"
            },
            "run_in_background": {
                "type": "boolean",
                "default": false,
                "description": "是否后台运行;true 时返回 task_id,可由 BashOutput 增量读取"
            }
        },
        "required": ["command"],
        "additionalProperties": false
    })
}
```

### 8.3 P0 — 必须做（安全与稳定）

**P0-1 进程组 kill（防孤儿）**

```rust
let mut cmd = Command::new("bash");
cmd.arg("-lc").arg(&command);
cmd.stdin(Stdio::null());
cmd.stdout(Stdio::piped());
cmd.stderr(Stdio::piped());
cmd.kill_on_drop(true);

#[cfg(unix)]
unsafe {
    cmd.pre_exec(|| {
        libc::setsid();
        Ok(())
    });
}

let child = cmd.spawn()?;
let pid = child.id().ok_or(...)?;
let output_fut = child.wait_with_output();

let kill_tree = || unsafe {
    #[cfg(unix)]
    {
        libc::killpg(pid as i32, libc::SIGTERM);
        // 3s 后 SIGKILL 在另一 tokio task 中执行
        let pid_copy = pid;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(GRACE_MS)).await;
            libc::killpg(pid_copy as i32, libc::SIGKILL);
        });
    }
    #[cfg(windows)]
    {
        // taskkill /T /F /PID <pid>
    }
};
```

**P0-2 截断时落盘路径回灌**

```rust
use std::fs::OpenOptions;
use std::io::Write;

let pid_suffix = format!("{:x}", std::process::id());
let log_path = format!("/tmp/laew-bash-{}.log", &random_hex(8));

let on_chunk = |chunk: &[u8]| -> bool {
    let mut file = OpenOptions::new().create(true).append(true).open(&log_path)?;
    file.write_all(chunk)?;
    // truncate 时给模型提示
    Ok(true)
};

// 截断时返回:
"<stdout>...(省略 1234 字符)...\n\n[完整输出: {log_path}]\n</stdout>\n<exit_code>0</exit_code>"
```

**P0-3 默认 cwd 改为项目根（注入 Yolo project_context）**

```rust
async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String> {
    let cwd = args.get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| ctx.get::<ProjectContext>().map(|pc| pc.project_root.clone()))
        .unwrap_or_else(current_work_dir);
    cmd.current_dir(cwd);
}
```

### 8.4 P1 — 应该做（UX 提升）

**P1-1 ANSI / 二进制清洗**（参考 pi）
```rust
fn sanitize_binary_output(s: &str) -> String {
    s.chars().filter(|c| {
        let cp = c as u32;
        cp == 0x09 || cp == 0x0a || cp == 0x0d || (cp > 0x1f && (cp < 0xfff9 || cp > 0xfffb))
    }).collect()
}
```

**P1-2 tail 截断（保留末尾 N 行/字节）**
参考 pi `truncateTail` 算法（`/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/tools/truncate.ts:168-`）。

**P1-3 截断字符上限 envvar**
```rust
let max = args.get("max_output_chars")
    .and_then(Value::as_u64)
    .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS as u64)
    .min(MAX_OUTPUT_CHARS as u64) as usize;
```

**P1-4 后台任务（run_in_background + BashOutput）**
参考 claudecode LocalShellTask 模式：
- `BashOutput(task_id, offset, filter)` 增量读取
- `KillShell(task_id)` 取消
- 任务注册表 `Mutex<HashMap<TaskId, TaskHandle>>`
- 输出环形缓冲 200 KB

### 8.5 P2 — 可以做（高级特性）

**P2-1 PTY 支持（portable-pty）**
只在交互式命令需要时（vim、git log、fzf）：
```rust
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

let pty_system = native_pty_system();
let pair = pty_system.openpty(PtySize { rows: 30, cols: 100, ... })?;
let mut cmd = CommandBuilder::new("bash");
cmd.arg("-lc");
pair.slave.spawn_command(cmd)?;
```

**P2-2 no-output-timeout**
openclaw 风格，命令还在跑但 N 秒无输出时预警。

**P2-3 pending/aggregate 二级缓冲**
参考 openclaw，UI 增量推送 30 KB，落盘 200 KB。

**P2-4 snapshot 缓存**
claudecode 风格，把用户 `~/.bashrc` 函数集/别名 dump 到 `$XDG_CACHE_HOME/laew/shell-snapshot-<hash>.sh`，每次执行前 source。

**P2-5 受管环境变量类型化**
参考 deepseek，定义 `LAEW_*` 前缀 + typed keys，禁止模型覆盖。

**P2-6 并发上限**
参考 claudecode `isConcurrencySafe`：
- 只读命令（`git status`、`ls`、`cat`）可并发
- 写命令（`git commit`、`npm install`）串行
- 同时最多 N 个（默认 4）

---

## 9. 关键文件速查

### claudecode

- `/usr/local/LsmGitOpenSource/claudecode/src/utils/bash/ShellSnapshot.ts` — snapshot 生成（413-582）
- `/usr/local/LsmGitOpenSource/claudecode/src/utils/shell/bashProvider.ts` — bash 命令拼装（58-255）
- `/usr/local/LsmGitOpenSource/claudecode/src/utils/shell/outputLimits.ts` — 30K/150K 输出上限（3-14）
- `/usr/local/LsmGitOpenSource/claudecode/src/utils/timeouts.ts` — 120s/600s 超时常量（2-39）
- `/usr/local/LsmGitOpenSource/claudecode/src/tools/BashTool/BashTool.tsx` — BashTool 主实现（420-1143）
- `/usr/local/LsmGitOpenSource/claudecode/src/tasks/LocalShellTask/LocalShellTask.tsx` — LocalShellTask 状态机（173-368）

### opencode

- `/usr/local/LsmGitOpenSource/opencode/packages/core/src/tool/bash.ts` — Bash V2 core（18-207）
- `/usr/local/LsmGitOpenSource/opencode/packages/core/src/pty/pty.ts` — PTY 接口（1-25）
- `/usr/local/LsmGitOpenSource/opencode/packages/core/src/pty/pty.node.ts` — `@lydell/node-pty` 实现（1-29）

### pi

- `/usr/local/LsmGitOpenSource/pi/packages/agent/src/harness/tools/bash.ts` — `createBashTool` 工厂（1-161）
- `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/bash-executor.ts` — 滚动缓冲 + 落盘（50-156）
- `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/tools/truncate.ts` — `truncateHead` + `truncateTail`（78-, 168-）
- `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/core/utils/shell.ts` — `sanitizeBinaryOutput`（30-41）
- `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/utils/ansi.ts` — `stripAnsi`

### atomcode

- `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/tools/bash.rs` — Bash 主实现（26-4303）

### deepseek-harness

- `/usr/local/LsmGitOpenSource/deepseek-harness/packages/shell/shell/src/index.ts` — ShellExecutor Service（65-104）
- `/usr/local/LsmGitOpenSource/deepseek-harness/docs/subsystems/shell.md` — bash 子系统文档（24-136）

### openclaw

- `/usr/local/LsmGitOpenSource/openclaw/src/agents/bash-tools.exec-runtime.ts` — ExecProcessRuntime（86-122, 192-243）
- `/usr/local/LsmGitOpenSource/openclaw/src/agents/bash-tools.schemas.ts` — `execSchema` 114 行
- `/usr/local/LsmGitOpenSource/openclaw/src/agents/bash-tools.shared.ts` — 通用工具
- `/usr/local/LsmGitOpenSource/openclaw/src/process/terminal-pty.ts` — `@lydell/node-pty` spawn（83-144）
- `/usr/local/LsmGitOpenSource/openclaw/src/process/exec-spawn.ts` — execa 调用封装（27-94）
- `/usr/local/LsmGitOpenSource/openclaw/packages/agent-core/src/harness/env/kill-tree.ts` — 进程树 kill（1-348）

### laew（当前工程）

- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/src/agent/tools/bash.rs` — BashTool 184 行
- `/usr/local/LsmGitOpenSource/LsmAgentEmergentWork/CLAUDE.md` — 工程概览

---

## 附录 A — 截断文案真实模板（可直接复用）

### laew 建议的截断文案

```
<stdout>
（前 30000 字符）
...[stdout 截断,省略 12345 字符,完整输出: /tmp/laew-bash-abcd1234.log]
</stdout>

<stderr>
...</stderr>

<exit_code>0</exit_code>
```

### claudecode 真实文案（参考）

```ts
// BashTool.tsx:732
const MAX_PERSISTED_SIZE = 64 * 1024 * 1024;
// >64MB 截断后落盘
await fsTruncate(result.outputFilePath, MAX_PERSISTED_SIZE);
// 文案在 persistOutputFileAs 路径中插入 marker
```

### pi 真实文案（参考）

```ts
// bash.ts:130-141
if (capture.truncation.lastLinePartial) {
  const lastLineSize = formatSize(capture.lastLineBytes);
  outputText += `\n\n[Showing last ${formatSize(capture.truncation.outputBytes)} of line ${endLine} (line is ${lastLineSize}). Full output: ${capture.fullOutputPath}]`;
} else if (capture.truncation.truncatedBy === "lines") {
  outputText += `\n\n[Showing lines ${startLine}-${endLine} of ${capture.truncation.totalLines}. Full output: ${capture.fullOutputPath}]`;
} else {
  outputText += `\n\n[Showing lines ${startLine}-${endLine} of ${capture.truncation.totalLines} (${formatSize(DEFAULT_MAX_BYTES)} limit). Full output: ${capture.fullOutputPath}]`;
}
```

### opencode 真实文案（参考）

```ts
// bash.ts:51-57
const modelOutput = (output: Output) => {
  const warnings = output.warnings?.length
    ? `\n\nWarnings:\n${output.warnings.map((warning) => `- ${warning}`).join("\n")}`
    : ""
  if (output.timeout) return `${warnings.trimStart()}${warnings ? "\n\n" : ""}Command timed out before completion.`
  return `${warnings.trimStart()}${warnings ? "\n\n" : ""}Command exited with code ${output.exit}.`
}

// bash.ts:187-188
const notice = result.outputTruncated
  ? "[output capture truncated at the in-memory safety limit]"
  : undefined
```

### openclaw 真实文案（参考）

```ts
// exec-runtime.ts:521-538
case "overall-timeout": {
  const timeoutText =
    typeof params.timeoutSec === "number" && params.timeoutSec > 0
      ? `Command timed out after ${params.timeoutSec} seconds.`
      : `Command timed out.`;
  const retryGuidance = appendExecTimeoutRetryGuidance(timeoutText, params.failureKind);
  return retryGuidance;
}
case "no-output-timeout":
  // 文案："Command produced no output for N seconds..."
```

---

## 附录 B — 信号与进程组速查

| POSIX 信号 | 数字 | 默认行为 | 用途 |
|-----------|------|---------|------|
| SIGTERM | 15 | 终止 | graceful cancel |
| SIGKILL | 9 | 终止（不可捕获）| hard kill |
| SIGHUP | 1 | 终止 + 控制 tty 断开 | tmux session close |
| SIGINT | 2 | 终止 | Ctrl+C |
| SIGQUIT | 3 | core dump | Ctrl+\ |
| SIGTSTP | 20 | 停止 | Ctrl+Z |

进程组相关系统调用：
- `setsid()` — 创建新 session + process group
- `killpg(pgid, sig)` — 给整个进程组发信号
- `setpgid(pid, pgid)` — 把进程加入进程组
- `getpgid(pid)` / `getpgrp()` — 查询

Rust crate：
- `libc::setsid()` / `libc::killpg()` / `libc::SIGKILL`
- `tokio::process::Command::pre_exec()` 注入 setsid
- `tokio::process::Child::id()` 获取 PID（pgid 在 setsid 后 == pid）

Windows 等价：
- `CreateJobObject` + `SetInformationJobObject(JobObjectExtendedLimitInformation, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)`
- `AssignProcessToJobObject` 把子进程加入 job
- Drop job handle → 自动 KILL_ON_JOB_CLOSE

参考 atomcode `bash.rs:287-289` 的 `assign_child_to_kill_on_close_job(&child)`。
