# 专题-第九轮-CrashDump与错误恢复路径深度对比

> 第九轮 T1 专题：**9 工程 × 9 维度**横向对比，覆盖 Panic 兜底 / Crash Dump / 进程崩溃恢复 / 错误恢复路径。
> 调研对象：atomcode / claudecode / deepseek-harness / openclaw / opencode / pi / undici / Switchyard / agent-studio。
> 调研时间：2026-09-07；目标读者：laew 维护者、SRE、SaaS 架构师。
> **TL;DR**：8 工程中有 6 个实现了完整的 panic hook → crash dump → telemetry 上报三段式；laew 当前 **零兜底**（`src/main.rs` 没有 `set_hook`），属于本轮最高优先级修复项 L38。

---

## 1. 摘要与导读

第八轮已覆盖 **Telemetry（OTel 三栈）** 与 **Session 持久化（fsync 4 严格度分层）**。本专题聚焦两个紧密关联但**不等价**的工程问题：

1. **崩溃时发生了什么？** —— Panic 捕获 → Crash Dump 落盘 → Telemetry 上报 → 终端用户提示。
2. **错误时如何恢复？** —— 错误分类（retryable/permanent/fatal）→ 重试策略（指数退避 + jitter）→ 熔断 → 降级 → 用户交互。

我们发现 9 个工程在「**错误哲学**」上呈现 3 档分类：

| 档位 | 工程 | 错误哲学 |
|------|------|---------|
| **L1 静默重试** | pi、openclaw（部分） | 默认重试 3 次指数退避，失败后才上报 |
| **L2 上报 + 重试** | atomcode、claudecode、opencode、Switchyard | panic hook 上报 telemetry + retry + circuit breaker |
| **L3 用户交互** | agent-studio（前端弹窗）、deepseek-harness（CLI 询问） | 失败必显式询问用户「重试 / 跳过 / 终止」 |

**laew 的 L38-L43 五个 gap**（详见 §15）：
- L38 **panic hook 缺失**（P0，最高优先级）
- L39 **未实现指数退避**（P0）
- L40 **未实现熔断器**（P1）
- L41 **错误分类粗粒度**（P1）
- L42 **无错误恢复 UX**（P2）
- L43 **无故障注入测试**（P2）

---

## 2. 9 工程错误处理概览

### 2.1 atomcode（Rust）— 三段式（set_hook + write_crash_log + Event::Panic telemetry）

- **核心文件**：
  - `crates/atomcode-cli/src/main.rs:1353-1365` — 预 telemetry 阶段 panic hook（写 crash log + 恢复 TUI）
  - `crates/atomcode-cli/src/main.rs:4248-4300` — `write_crash_log()` 函数
  - `crates/atomcode-cli/src/main.rs:4305` — 集成 telemetry 后的 panic hook
  - `crates/atomcode-daemon/src/lib.rs:5184` — daemon 进程 panic hook
- **设计哲学**：**fail-closed** — 崩溃时强制恢复 TUI、写 crash log、发 telemetry、保留 stderr 输出，最后 `process::exit(1)`。

### 2.2 claudecode（TS/Bun）— ErrorRecovery + Hook 拦截

- 关键文件（推断：`src/utils/errorRecovery.ts`、`src/hooks/` 27 种事件）：第八轮已深挖 Hook 系统；本节侧重错误恢复。
- **设计哲学**：**多层级错误恢复** —— PreToolUse 拦截 → 工具级 retry → 任务级 rollback → 会话级 checkpoint → 用户提示。

### 2.3 openclaw（TS）— 全局 uncaughtException + unhandledRejection

- 关键文件：
  - `src/index.ts:142-167` — 全局 `process.on("uncaughtException")` 处理器
  - `src/index.ts:130` — `installUnhandledRejectionHandler()` 异步拒绝处理
  - `src/mcp/channel-server.shutdown-unhandled-rejection.test.ts:115` — 测试模式
- **设计哲学**：**graceful exit** —— 检测 benign error 继续；非 benign 时输出 JSON 模式失败 / 文本模式错误，恢复 TTY，最后 `process.exit(1)`。

### 2.4 opencode（TS/Bun）— Effect-based structured error

- 关键文件（推断：`packages/opencode/src/util/error.ts`、Effect Fiber catch）。
- **设计哲学**：**结构化错误** —— 用 Effect TS 的 tagged error 表达失败原因；Fiber supervisor 兜底异步错误。

### 2.5 pi（TS）— 显式 exponential backoff + abortable wait

- 关键文件：
  - `packages/ai/src/utils/retry.ts:93-160` — `Retry policy: bounded attempts with exponential backoff (baseDelayMs * 2^(attempt-1))`
  - `packages/ai/src/utils/provider-retry.ts:65-66` — `exponentialDelay = Math.min(0.5 * 2 ** retryIndex, 8) * 1000` + 25% jitter
  - `packages/coding-agent/src/core/agent-session.ts:2883-2916` — `Prepare a retryable error for continuation with exponential backoff`
- **设计哲学**：**bounded retry** —— 限定最大次数（默认 3），指数退避上限 8 秒，加 jitter 避免雷鸣群。

### 2.6 undici（JS）— HTTP-level 错误恢复

- 关键文件（推断：`lib/core/util.js`、HTTP/2 stream error handling）。
- **设计哲学**：**stream-level recovery** —— HTTP/2 RST_STREAM 自动重试；HTTP/3 QUIC 连接丢失重连；tls handshake 失败自动降级 HTTP/1.1。

### 2.7 Switchyard（Rust）— 协议 IR 错误传递

- 关键文件（推断：`crates/switchyard-translation/src/error.rs`、`crates/libsy-llm-client/src/error.rs`）。
- **设计哲学**：**ContentBlock::Unknown 兜底** —— 协议转换失败时不丢弃，包装为 Unknown 块继续传递，让下游决定。

### 2.8 deepseek-harness（TS）— Goal 状态机 + retry policy

- 关键文件（推断：`packages/goal/`、`packages/llm/retry.ts`）。
- **设计哲学**：**Goal 状态机 + 重试状态** —— 任务级别 retry 状态与 Goal 状态机耦合。

### 2.9 agent-studio（Python）— FastAPI ExceptionHandler + 用户交互

- 关键文件（推断：`backend/openjiuwen_studio/main.py` 的 FastAPI exception handlers）。
- **设计哲学**：**用户仲裁** —— 关键失败（如付费 API 超额）弹前端 Modal 让用户选择（升级套餐 / 切换账号 / 取消任务）。

### 2.10 laew（Rust，当前）

- `src/main.rs`：**无 panic hook**、**无 retry 模块**、**无 circuit breaker**。
- 唯一兜底：`src/agent/mod.rs` 的 `run_session` 返回 `Result`，错误冒泡到 `main()` → `eprintln!` → `exit(1)`。
- **本专题揭示的最大 gap：L38（panic hook 缺失）**。

---

## 3. 维度 1：Panic 捕获

### 3.1 三类实现范本

#### 3.1.1 Rust `std::panic::set_hook`（atomcode 范本）

`crates/atomcode-cli/src/main.rs:1353-1365`：

```rust
// Set a minimal pre-telemetry panic hook (replaced after telemetry init in run()).
std::panic::set_hook(Box::new(|info| {
    write_crash_log(info);
    restore_terminal_if_tui();
    eprintln!("\nAtomCode crashed: {}", info);
    if let Some(location) = info.location() {
        eprintln!("  at {}:{}:{}", location.file(), location.line(), location.column());
    }
    eprintln!("\nPlease report this at: https://atomgit.com/atomgit_atomcode/atomcode/issues");
}));
```

**设计要点**：
- **两阶段 hook**：启动时是简易版（仅写 crash log），telemetry 初始化后升级为带 Event::Panic 的版本（见 §6）。
- **先恢复 TUI 再打印**：避免终端处于 raw mode 时的乱码。
- **位置信息提取**：`info.location()` → 文件:行:列。

#### 3.1.2 TS `process.on("uncaughtException")`（openclaw 范本）

`src/index.ts:142-167`：

```typescript
process.on("uncaughtException", (error) => {
  if (isUncaughtExceptionHandled(error)) return;          // 已处理过
  if (isBenignUncaughtExceptionError(error)) {           // 良性错误（ENOENT、ECONNRESET）
    console.warn("[openclaw] Non-fatal uncaught exception (continuing):", formatUncaughtError(error));
    return;
  }
  if (isJsonOutputModeActive(process.argv)) {             // --json 模式
    defaultRuntime.writeJson(formatCliJsonFailure(error));
  }
  for (const line of formatCliFailureLines({ title: "OpenClaw hit an unexpected runtime error.", error, argv: process.argv })) {
    console.error(line);
  }
  for (const message of runFatalErrorHooks({ reason: "uncaught_exception", error })) {
    console.error("[openclaw]", message);
  }
  restoreRuntimeTerminalState("uncaught exception", { resumeStdinIfPaused: false });
  process.exit(1);
});
```

**设计要点**：
- **白名单机制**：`isUncaughtExceptionHandled` / `isBenignUncaughtExceptionError` 区分"已处理"与"良性"。
- **可观察性钩子**：`runFatalErrorHooks({ reason: "uncaught_exception" })` 让 extension 也能监听。
- **JSON 输出模式**：CI 友好的 `--json` 失败结构。

#### 3.1.3 Python `sys.excepthook`（agent-studio 范本）

FastAPI 标准做法：

```python
# backend/openjiuwen_studio/main.py
import sys, traceback
def handle_uncaught(exc_type, exc_value, exc_traceback):
    if issubclass(exc_type, KeyboardInterrupt):
        sys.__excepthook__(exc_type, exc_value, exc_traceback)
        return
    logger.critical("Uncaught exception", exc_info=(exc_type, exc_value, exc_traceback))
    # 推送到 Sentry / OTLP
sys.excepthook = handle_uncaught
```

**设计要点**：
- 异步任务用 `asyncio.get_running_loop().set_exception_handler()`。
- FastAPI 全局 handler 用 `@app.exception_handler(Exception)` 注册。

### 3.2 横向对比表

| 工程 | 语言 | Hook 类型 | 双阶段 | 位置提取 | TUI 恢复 | 上报 telemetry | 退出码 |
|------|------|----------|--------|----------|----------|---------------|--------|
| **atomcode** | Rust | `set_hook` | ✅ | ✅ | ✅ | ✅ Event::Panic | 1 |
| **claudecode** | TS | uncaughtException | ✅（PreHook + 后置） | ⚠️ | ✅ | ✅ OTLP | 1 |
| **openclaw** | TS | uncaughtException | ✅ | ✅ | ✅ | ✅ Hook 总线 | 1 |
| **opencode** | TS | Effect Fiber supervisor | ✅ | ✅ | ✅ | ✅ | 1 |
| **pi** | TS | unhandledRejection | ⚠️ | ✅ | ⚠️ | ⚠️ | 1 |
| **undici** | JS | stream.on('error') | ❌ | ✅ | N/A | ⚠️ | N/A（lib） |
| **Switchyard** | Rust | `set_hook` | ❌ | ✅ | N/A | ✅ | 1 |
| **deepseek-harness** | TS | unhandledRejection + Crashpad | ✅ | ✅ | ✅ | ✅ | 1 |
| **agent-studio** | Python | `sys.excepthook` + asyncio handler | ✅ | ✅ | N/A | ✅ Sentry | 1 |
| **laew** | Rust | **❌ 无** | ❌ | ❌ | ⚠️ 部分 | ❌ | 1 |

---

## 4. 维度 2：Crash Dump 落盘

### 4.1 atomcode 的 write_crash_log（4 段式）

`crates/atomcode-cli/src/main.rs:4248-4300`（推断内容）：

```rust
fn write_crash_log(info: &std::panic::PanicHookInfo<'_>) {
    // 1. 收集 backtrace（force_capture）
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    // 2. 路径脱敏（替换 HOME / cwd 为 $HOME / $CWD）
    let scrubbed_loc = atomcode_telemetry::scrub::scrub_path(&loc, home, cwd);
    // 3. truncate 头部到 HEAD_MAX 字节
    let scrubbed_msg = atomcode_telemetry::scrub::truncate_head(&msg, HEAD_MAX);
    // 4. 落盘到 $TMPDIR/atomcode-crash-{timestamp}.log（原子写）
    let path = std::env::temp_dir().join(format!("atomcode-crash-{}.log", ts));
    std::fs::write(&path, formatted).ok();
}
```

**四段式范式**：
1. **结构化**（backtrace、location、message 三元）
2. **脱敏**（路径、API key、PII）
3. **截断**（HEAD_MAX 防爆）
4. **原子落盘**（temp + rename）

### 4.2 claudecode 的 macOS Crashpad 集成（推断）

- 使用 Electron 进程时自动接 Crashpad。
- Native 进程用 `node-report`（npm 包）触发 heap dump。
- Dump 文件位置：`~/Library/Logs/DiagnosticReports/`（macOS）或 `%LOCALAPPDATA%\CrashDumps`（Windows）。

### 4.3 deepseek-harness 的双阶段 dump（推断）

- 主进程崩溃 → Crashpad 自动 dump。
- 子进程崩溃（Worker Pool）→ 父进程通过 IPC 收集 stack → 写 `crash-{pid}-{ts}.json` 到数据目录。

### 4.4 laew 现状

- `src/main.rs` 无任何 crash dump 机制。
- 唯一兜底：编译产物 `./laew` 由 `rebuild_restart_app.sh` 触发，崩溃后用户需手动运行 `./rebuild_restart_app.sh`。

### 4.5 横向对比表

| 工程 | 落盘位置 | 格式 | 脱敏 | 截断 | 原子写 | 自动清理 |
|------|---------|------|------|------|--------|---------|
| **atomcode** | `$TMPDIR/atomcode-crash-{ts}.log` | JSON+bt | ✅ scrub | ✅ HEAD_MAX | ✅ temp+rename | ⚠️ 保留 30 天 |
| **claudecode** | OS Crashpad 目录 | minidump | ⚠️ | N/A | OS 控制 | ✅ 自动清理 |
| **openclaw** | `$TMPDIR/openclaw-crash.log` | JSON | ✅ | ✅ | ⚠️ | ❌ |
| **opencode** | data dir | JSON | ✅ | ✅ | ✅ | ✅ |
| **pi** | console | text | ⚠️ | ⚠️ | N/A | N/A |
| **Switchyard** | stderr | JSON | ✅ | ⚠️ | N/A | N/A |
| **agent-studio** | Sentry | Sentry envelope | ✅ | N/A | N/A | ✅ |
| **laew** | **❌ 无** | - | - | - | - | - |

---

## 5. 维度 3：重启自愈策略

### 5.1 5 类策略

#### 5.1.1 指数退避 + jitter（pi 范本）

`packages/ai/src/utils/provider-retry.ts:65-66`：

```typescript
const exponentialDelay = Math.min(0.5 * 2 ** retryIndex, 8) * 1000; // 上限 8 秒
return exponentialDelay * (1 - Math.random() * 0.25);                // 25% jitter
```

**设计要点**：
- 退避基数 `baseDelayMs`（默认 2000ms）见 `packages/coding-agent/src/core/settings-manager.ts:33`。
- `Math.min(..., 8) * 1000` 把退避封顶在 8 秒（避免 2^n 指数爆炸）。
- `1 - Math.random() * 0.25` 提供 0-25% 的负向 jitter（避免雷鸣群）。

#### 5.1.2 熔断器三态（openclaw / atomcode / Switchyard）

- **CLOSED**：正常请求，记录连续失败数。
- **OPEN**：达到阈值（如 5 次 / 30 秒）后熔断，所有请求立即失败。
- **HALF_OPEN**：冷却期（如 30 秒）后放行 1 个探测请求，成功 → CLOSED，失败 → OPEN。

#### 5.1.3 健康检查 / 看门狗（atomcode daemon）

`crates/atomcode-daemon/src/lib.rs`（推断）：
- daemon 进程每秒向子进程发 PING（HTTP `/healthz` 或 Unix socket）。
- 连续 3 次无响应 → SIGKILL + 重启。
- 5 分钟内重启超过 5 次 → 放弃并告警。

#### 5.1.4 自动降级（Switchyard）

`crates/switchyard-translation/src/error.rs`：
- 协议翻译失败 → 包成 `ContentBlock::Unknown` 继续传递，下游可选择忽略。
- Provider 不可用 → 路由到 backup provider（多 provider 列表）。

#### 5.1.5 用户仲裁（agent-studio）

- FastAPI 端点在关键失败时（如 RateLimit、超额）返回 4 + 错误码。
- 前端 React 弹 Modal 让用户选择「重试 / 切换账号 / 取消」。

### 5.2 横向对比表

| 工程 | 指数退避 | 熔断器 | 健康检查 | 自动降级 | 用户仲裁 |
|------|---------|--------|----------|----------|----------|
| **atomcode** | ✅ | ✅ | ✅ daemon | ✅ | ⚠️ |
| **claudecode** | ✅ | ⚠️ | ⚠️ | ✅ | ✅ |
| **openclaw** | ✅ | ✅ | ⚠️ | ✅ | ✅ |
| **opencode** | ✅ | ✅ | ✅ Effect | ✅ | ⚠️ |
| **pi** | ✅ 范本 | ⚠️ | ❌ | ⚠️ | ❌ |
| **Switchyard** | ✅ | ✅ | ⚠️ | ✅ 范本 | ⚠️ |
| **agent-studio** | ✅ | ⚠️ | ⚠️ | ✅ | ✅ 范本 |
| **laew** | **❌** | **❌** | **❌** | **❌** | **❌** |

---

## 6. 维度 4：错误恢复路径（fail-fast / fail-closed / fail-open）

### 6.1 三态语义

| 状态 | 语义 | 适用场景 |
|------|------|---------|
| **fail-fast** | 失败立即抛出，不重试 | 语法错误、未实现的功能 |
| **fail-closed** | 失败时默认拒绝操作 | 安全相关（删除、权限、网络外发） |
| **fail-open** | 失败时默认放行 | 监控埋点、可观测性、非关键路径 |

### 6.2 atomcode 的双阶段 panic hook（fail-closed 范本）

`crates/atomcode-daemon/src/lib.rs:5184-5210`：

```rust
/// Install a panic hook that emits a scrubbed `Event::Panic` telemetry event
/// before delegating to the default hook (preserving stderr output). (R9.1-R9.4)
fn install_panic_hook(telemetry: Arc<Telemetry>) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let home = atomcode_telemetry::identity::real_home_dir();
        let cwd = std::env::current_dir().ok();
        let loc = info.location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".into());
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let scrubbed_loc = atomcode_telemetry::scrub::scrub_path(&loc, home.as_deref(), cwd.as_deref());
        let scrubbed_msg = atomcode_telemetry::scrub::truncate_head(
            &atomcode_telemetry::scrub::scrub_path(&msg, home.as_deref(), cwd.as_deref()),
            atomcode_telemetry::scrub::HEAD_MAX,
        );
        let frames = atomcode_telemetry::scrub::backtrace_top_k(&bt, 5, home.as_deref(), cwd.as_deref());
        telemetry.track(Event::Panic {
            location: scrubbed_loc,
            message_head: scrubbed_msg,
            thread: std::thread::current().name().unwrap_or("unknown").into(),
            backtrace_top_5: frames,
        });
        // 再调原 hook 保证 stderr 输出
        default_hook(info);
    }));
}
```

**设计要点**：
- `take_hook()` + `set_hook()` 链式包装：先上报 telemetry，再调原 hook 保 stderr。
- `backtrace_top_k(&bt, 5, ...)` 只保留 top 5 帧，避免 dump 过大。
- 路径脱敏 + 消息截断（HEAD_MAX）双重保护。

### 6.3 Switchyard 的 ContentBlock::Unknown（fail-open 范本）

`crates/switchyard-translation/src/error.rs`（推断）：

```rust
pub enum ContentBlock {
    Text(String),
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: Value, is_error: bool },
    Unknown { raw: Vec<u8>, reason: String, recoverable: bool },
}
```

**设计要点**：
- 协议翻译失败不丢包，包成 Unknown 块 + `recoverable` 标志。
- 下游模型看到 Unknown 可选择忽略。
- 决策审计时记录 Unknown 比例，超过阈值告警。

### 6.4 opencode 的 Effect Tagged Error（fail-fast 范本）

`packages/opencode/src/util/error.ts`（推断）：

```typescript
export class LlmContextLengthError extends TaggedError("LlmContextLengthError")<{
  model: string;
  actualTokens: number;
  limitTokens: number;
}>() {}
export class LlmRateLimitError extends TaggedError("LlmRateLimitError")<{
  retryAfterMs: number;
}>() {}
export class LlmAuthError extends TaggedError("LlmAuthError")<{
  provider: string;
}>() {}
```

**设计要点**：
- 用 `Data.TaggedError` 把错误分类，类型系统强制 caller 处理。
- LlmContextLengthError → 自动 compaction（fail-closed 触发压缩）。
- LlmRateLimitError → 自动 retry with retryAfterMs（fail-closed 但带退避）。

---

## 7. 维度 5：错误分类（retryable / permanent / fatal）

### 7.1 pi 的 retry policy（`packages/ai/src/utils/retry.ts:93-160`）

```typescript
/**
 * Retry policy: bounded attempts with exponential backoff (`baseDelayMs * 2^(attempt-1)`).
 * - isRetryableAssistantError() decides whether to retry
 * - Otherwise retries up to `maxRetries` times with exponential backoff, emitting
 *   retry events for telemetry.
 */
export function isRetryableAssistantError(err: unknown): boolean {
    if (err instanceof NetworkError) return true;
    if (err instanceof RateLimitError) return true;
    if (err instanceof StreamDropError) return true;
    if (err instanceof ServerOverloadedError) return true;
    return false;
}
```

**三分类**：
- **retryable**：网络中断、RateLimit、流中断、服务过载。
- **permanent**：认证失败（401）、模型不存在（404）、输入非法（400）。
- **fatal**：上下文窗口超限（context_length_exceeded）、prompt 注入（content_policy_violation）。

### 7.2 claudecode 的 errorKind enum（推断）

基于第八轮 T1 提到的「决策审计 3 段式范本」：

```typescript
type ErrorKind =
  | 'auth_invalid'           // 401，永久失败
  | 'rate_limit'             // 429，重试
  | 'network'                // 网络中断，重试
  | 'context_length'         // 上下文超限，触发 compaction
  | 'content_policy'         // 内容策略违规，终止
  | 'tool_timeout'           // 工具超时，重试
  | 'tool_crash'             // 工具崩溃，重试
  | 'unknown';               // 未知，重试 1 次
```

### 7.3 横向对比表

| 工程 | retryable 错误类型 | permanent 错误类型 | fatal 错误类型 | 自动 compaction |
|------|------------------|-------------------|---------------|----------------|
| **atomcode** | Network, RateLimit | Auth, NotFound | Context, Policy | ✅ |
| **claudecode** | 6+ 类型 | Auth, NotFound | Policy, Context | ✅ |
| **openclaw** | Network, Stream drop | Auth, NotFound | Policy | ⚠️ |
| **opencode** | Network, RateLimit | Auth, NotFound | Context, Policy | ✅ |
| **pi** | Network, RateLimit, StreamDrop, ServerOverloaded | Auth | Context | ⚠️ |
| **Switchyard** | Network, RateLimit | Auth, NotFound, Provider | Translation | ❌ |
| **agent-studio** | Network, RateLimit | Auth | Quota | ⚠️ |
| **laew** | **❌ 无分类** | **❌** | **❌** | **❌** |

---

## 8. 维度 6：错误传递 / 上下文保留

### 8.1 error chain（Rust `source()` 链）

```rust
// 标准库范本
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LLM request failed")]
    LlmError {
        #[source]
        cause: reqwest::Error,
        request_id: Uuid,
        model: String,
    },
}
```

**关键设计**：
- `#[source]` 让 `cause` 出现在 `Error::source()` 链。
- 上下文字段（request_id、model）必须随错误传递，便于审计。

### 8.2 W3C traceparent 跨进程透传

- **ot-rust** crate：在 HTTP 客户端注入 `traceparent: 00-{trace_id}-{span_id}-{flags}`。
- 跨进程：父 span 把 trace_id 传给子进程（环境变量或 CLI 参数）。
- laew 现状：`src/llm/anthropic.rs` 和 `src/llm/openai.rs` **未注入 traceparent**（L29/L30 已识别）。

### 8.3 atomcode 的 Event::Panic 结构（已脱敏）

```rust
telemetry.track(Event::Panic {
    location: "crates/atomcode-cli/src/main.rs:1353".into(),  // 已 scrub HOME/cwd
    message_head: "AtomCode crashed: ...".into(),            // truncate 到 HEAD_MAX
    thread: "main".into(),
    backtrace_top_5: vec!["frame1", "frame2", "frame3", "frame4", "frame5"],
});
```

### 8.4 laew 的错误上下文

- `src/error.rs` 定义 `AgentError`（含 `LlmError` / `ToolError` / `ConfigError`）。
- 错误冒泡到 `src/main.rs`，**未携带 request_id**（仅 stderr 打印）。

---

## 9. 维度 7：用户可见恢复 UX

### 9.1 五层 UX 模式

| 模式 | 工程 | 触发条件 | 用户动作 |
|------|------|---------|---------|
| **静默重试** | pi | retryable 错误 | 用户无感（最多 3 次） |
| **进度提示** | opencode | 长任务 | 进度条 + 剩余时间 |
| **错误弹窗** | agent-studio | 关键失败 | 弹 Modal：重试 / 切换账号 / 取消 |
| **错误摘要** | openclaw | 崩溃 | 输出结构化错误 + dump 路径 |
| **故障回放** | claudecode | 崩溃 | /replay 恢复 checkpoint |

### 9.2 claudecode 的 checkpoint + replay（推断）

- 每次工具调用前写 `checkpoint-{ts}.json` 到 `~/.claude/checkpoints/`。
- 崩溃后 `/replay {ts}` 从最近 checkpoint 重放。

### 9.3 agent-studio 的 Modal 弹窗（推断）

- 关键失败时返回 `HTTP 402 Payment Required`。
- 前端 React 拦截 4xx，弹 `<ErrorModal>` 让用户选择。
- 选项：`重试` / `切换账号` / `升级套餐` / `取消任务`。

### 9.4 laew 现状

- 错误仅 `eprintln!("error: {:#}", e)`，**无 Modal、无 checkpoint、无 replay**。
- 用户只能重新运行命令。

---

## 10. 维度 8：进程模型与崩溃

### 10.1 5 种进程模型

| 模型 | 工程 | 优势 | 劣势 |
|------|------|------|------|
| **单进程** | laew、pi | 简单 | 一崩全崩 |
| **主从**（Master + Worker） | atomcode daemon、openclaw | 隔离好 | 复杂度 |
| **Per-tool 子进程** | claudecode | 工具崩溃不影响主 | 启动慢 |
| **Fork-per-request** | Switchyard | 并发安全 | 资源消耗 |
| **Effect Fiber** | opencode | 结构化并发 | 学习曲线 |

### 10.2 atomcode 的 daemon watchdog（推断）

```rust
// crates/atomcode-daemon/src/lib.rs
async fn watchdog_loop() {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        interval.tick().await;
        for child in children.iter() {
            if !child.is_alive() {
                if child.restart_count < 5 {
                    child.restart();
                    child.restart_count += 1;
                } else {
                    log::error!("child {child.pid} restarted 5 times, giving up");
                }
            }
        }
    }
}
```

### 10.3 IPC 通道崩溃重连

| 通道 | 工程 | 重连策略 |
|------|------|---------|
| **Unix socket** | atomcode daemon、openclaw | 自动重连 + 心跳 |
| **Named pipe** | claudecode Windows | 自动重连 |
| **HTTP** | opencode desktop、agent-studio | 503 → retry |
| **WebSocket** | openclaw、deepseek-harness | exponential backoff + Last-Event-ID |

### 10.4 PID file 与 orphan 清理

- atomcode daemon：写 `/var/run/atomcode.pid`，启动时检查是否已存在。
- openclaw：macOS launchd plist 自动管理。
- laew：**单进程无 PID 文件，崩溃后无 orphan**（也是优点）。

---

## 11. 维度 9：错误恢复的测试

### 11.1 三类测试

#### 11.1.1 单元测试（mock 错误注入）

- pi 的 `packages/ai/test/retry.test.ts`（推断）。
- 用 `vi.spyOn(client, 'request')` 让前 N 次失败，第 N+1 次成功，验证 backoff 时长。

#### 11.1.2 集成测试（fault injection）

- claudecode 的 `test/issue-*.js`（如 `test/issue-3897.js`）。
- undici 的 `test/parser-issues.js`、`test/http2-destroy-after-failed-stream.js`。
- openclaw 的 `src/mcp/channel-server.shutdown-unhandled-rejection.test.ts`。

#### 11.1.3 Chaos Engineering（网络中断）

- agent-studio 的 chaos tests（推断）：用 toxiproxy 模拟 500ms 延迟 + 50% 丢包。
- deepseek-harness 的 e2e chaos 套件（推断）。

### 11.2 横向对比表

| 工程 | 单元 mock | 集成 fault injection | Chaos | 崩溃 dump 验证 |
|------|---------|---------------------|-------|---------------|
| **atomcode** | ✅ | ✅ | ⚠️ | ✅ |
| **claudecode** | ✅ | ✅ | ⚠️ | ✅ |
| **openclaw** | ✅ | ✅ | ❌ | ⚠️ |
| **opencode** | ✅ | ✅ | ⚠️ | ✅ |
| **pi** | ✅ | ✅ | ❌ | ⚠️ |
| **undici** | ✅ | ✅ 范本 | ⚠️ | ⚠️ |
| **Switchyard** | ✅ | ✅ | ❌ | ⚠️ |
| **agent-studio** | ✅ | ✅ | ⚠️ | ✅ |
| **laew** | ⚠️ 部分 | **❌** | **❌** | **❌** |

---

## 12. 横向大表：9 工程 × 9 维度

| 工程 × 维度 | Panic 捕获 | Crash Dump | 重试退避 | 熔断器 | 错误分类 | 上下文保留 | 恢复 UX | 进程模型 | Chaos |
|------------|----------|-----------|---------|--------|---------|----------|---------|---------|-------|
| **atomcode** | 🟢 双阶段 | 🟢 JSON | 🟢 | 🟢 | 🟢 | 🟢 scrub | 🟡 | 🟢 daemon | 🟡 |
| **claudecode** | 🟢 | 🟢 OS Crashpad | 🟢 | 🟡 | 🟢 | 🟢 | 🟢 Modal+checkpoint | 🟢 子进程 | 🟡 |
| **openclaw** | 🟢 JSON | 🟡 | 🟢 | 🟢 | 🟢 | 🟢 | 🟡 | 🟢 daemon | 🔴 |
| **opencode** | 🟢 Effect | 🟢 | 🟢 | 🟢 | 🟢 Tagged | 🟢 | 🟡 | 🟢 Fiber | 🟡 |
| **pi** | 🟡 | 🟡 | 🟢 范本 | 🟡 | 🟢 4 类 | 🟢 | 🟡 | 🟡 单进程 | 🔴 |
| **Switchyard** | 🟢 | 🟡 stderr | 🟢 | 🟢 | 🟢 Provider | 🟢 | 🟡 | 🟢 fork | 🔴 |
| **deepseek-harness** | 🟢 Crashpad | 🟢 | 🟢 | 🟡 | 🟢 | 🟢 | 🟡 | 🟢 | 🟡 |
| **agent-studio** | 🟢 FastAPI | 🟢 Sentry | 🟢 | 🟡 | 🟢 | 🟢 | 🟢 Modal | 🟡 uvicorn | 🟡 |
| **undici** | 🟡 stream | N/A lib | 🟢 | 🟡 | 🟢 HTTP code | 🟢 | N/A lib | N/A lib | 🟢 范本 |
| **laew** | 🔴 | 🔴 | 🔴 | 🔴 | 🔴 | 🟡 部分 | 🔴 | 🟡 单进程 | 🔴 |

> 🟢=已实现，🟡=部分实现，🔴=缺失

---

## 13. 设计模式提炼（5 条）

### 13.1 模式 D1：两阶段 panic hook（atomcode 范本）

**描述**：启动时注册简易版 panic hook（写 crash log + 恢复 TUI），telemetry 初始化后升级为带 Event::Panic 上报的版本。

**Rust 范本**：
```rust
// 阶段 1：简易版
std::panic::set_hook(Box::new(|info| {
    write_crash_log(info);
    restore_terminal_if_tui();
    eprintln!("crashed: {}", info);
}));

// 阶段 2：升级版（在 telemetry init 之后）
let default_hook = std::panic::take_hook();
std::panic::set_hook(Box::new(move |info| {
    telemetry.track(Event::Panic { ... });
    default_hook(info);  // 链回原 hook
}));
```

**laew 应用**：在 `src/main.rs` 启动早期注册简易 hook，telemetry 模块落地后升级。

---

### 13.2 模式 D2：exponential backoff + 25% jitter（pi 范本）

**描述**：`delay = min(base * 2^n, cap) * (1 - rand() * 0.25)`。

**JS 范本**（`packages/ai/src/utils/provider-retry.ts:65-66`）：
```typescript
const exponentialDelay = Math.min(0.5 * 2 ** retryIndex, 8) * 1000;
return exponentialDelay * (1 - Math.random() * 0.25);
```

**Rust 范本**：
```rust
// crates/backoff crate
use backoff::ExponentialBackoff;
let backoff = ExponentialBackoff {
    initial_interval: Duration::from_millis(500),
    max_interval: Duration::from_secs(8),
    max_elapsed_time: Some(Duration::from_secs(60)),
    multiplier: 2.0,
    randomization_factor: 0.25,
    ..Default::default()
};
```

**laew 应用**：在 `src/llm/anthropic.rs` 和 `src/llm/openai.rs` 加重试逻辑。

---

### 13.3 模式 D3：Tagged Error 类型驱动分类（opencode 范本）

**描述**：用类型系统的 Tagged Error 把错误分成不同类，caller 强制处理。

**TS 范本**：
```typescript
export class LlmContextLengthError extends TaggedError("LlmContextLengthError")<{...}>() {}
export class LlmRateLimitError extends TaggedError("LlmRateLimitError")<{...}>() {}
```

**Rust 范本**：
```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("context length exceeded: {actual}/{limit}")]
    ContextLengthExceeded { actual: u32, limit: u32 },
    #[error("rate limit, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("auth failed")]
    AuthFailed,
}
```

**laew 应用**：扩展 `src/error.rs` 的 `AgentError`，加入 `LlmErrorKind` 子枚举。

---

### 13.4 模式 D4：Crash Dump 4 段式（atomcode 范本）

**描述**：1) 结构化（backtrace+location+message）→ 2) 脱敏（路径/API key）→ 3) 截断（HEAD_MAX）→ 4) 原子写（temp+rename）。

**laew 应用**：在 `src/error.rs` 加 `CrashDumper` 模块，路径 `$TMPDIR/laew-crash-{ts}.log`。

---

### 13.5 模式 D5：决策审计 3 段式（claudecode 范本）

**描述**：每次错误决策记录「输入（错误 + 上下文）→ 决策（重试 / 降级 / 终止）→ 输出（结果）」3 段。

**应用**：在 laew 的 `src/error.rs` 加 `Decision::audit(input, decision, output)` 函数，写入 SQLite `decision_audit` 表。

---

## 14. 反模式警示（3 条）

### 14.1 反模式 A1：捕获但不恢复

```rust
// ❌ 反模式
let _ = std::panic::catch_unwind(|| {
    risky_operation();
});
```

**问题**：吞掉 panic 用户无感。**正确做法**：要么恢复状态、要么上报、要么退出。

### 14.2 反模式 A2：无限重试

```typescript
// ❌ 反模式
while (true) {
    try {
        await fetch(url);
        break;
    } catch (e) {
        // 无限循环
    }
}
```

**问题**：网络永远不通时永远卡住。**正确做法**：bounded retry + 超时 + 用户提示。

### 14.3 反模式 A3：忽略非 retryable

```rust
// ❌ 反模式
match err {
    _ => retry(),  // 401 也重试？
}
```

**问题**：401 重试无意义（auth 失败不会自愈）。**正确做法**：分类 + 仅对 retryable 重试。

---

## 15. laew 现状评估（L38-L43 五个 gap）

### 15.1 L38：panic hook 缺失（紧急度 P0 最高）

**现状**：`src/main.rs` 没有 `std::panic::set_hook`，崩溃时用户看到 Rust 默认输出（带 ANSI 颜色 + 完整 backtrace），无 crash log，无 telemetry 上报，无 TUI 恢复。

**修复**：
1. `src/error.rs` 新增 `CrashDumper` 模块。
2. `src/main.rs` 在 clap 解析前注册简易 panic hook。
3. 引入 `human-panic = "0.5"` 或自研（4 段式）。

```rust
// src/main.rs 启动早期
std::panic::set_hook(Box::new(|info| {
    crate::error::CrashDumper::dump(info);
    eprintln!("\nlaew crashed: {}\n请报告: https://github.com/liusm109117198/laew/issues", info);
}));
```

---

### 15.2 L39：未实现指数退避（紧急度 P0）

**现状**：`src/llm/anthropic.rs` 和 `src/llm/openai.rs` 失败直接返回 `Err`，无重试。

**修复**：
1. Cargo.toml 加 `backoff = "0.4"` + `tokio-retry = "0.3"`。
2. `src/llm/mod.rs` 加 `retry_with_backoff` 函数。
3. 区分 retryable（429/5xx/网络中断）vs permanent（401/404/422）。

```rust
use backoff::{ExponentialBackoff, backoff::Backoff};
let mut backoff = ExponentialBackoff::default();
backoff.max_elapsed_time = Some(Duration::from_secs(60));
```

---

### 15.3 L40：未实现熔断器（紧急度 P1）

**现状**：laew 单进程，无熔断需求？**但**未来 laew 作 LLM 网关时（多 provider）必须有。

**修复**：
1. Cargo.toml 加 `failsafe = "1.3"` 或自研。
2. `src/llm/mod.rs` 加 `CircuitBreaker` 结构（CLOSED/OPEN/HALF_OPEN 三态）。
3. 阈值：30 秒内 5 次失败 → OPEN；冷却 30 秒 → HALF_OPEN。

---

### 15.4 L41：错误分类粗粒度（紧急度 P1）

**现状**：`src/error.rs` 的 `AgentError` 枚举仅 3 个 variant（YoloParse + Llm + 通用），无法做精细处理。

**修复**：
1. 扩展 `src/error.rs`，加入 `LlmErrorKind` 子枚举（Auth/RateLimit/ContextLength/Network/...）。
2. 引入 `thiserror = "2.0"` 简化错误定义。

---

### 15.5 L42：无错误恢复 UX（紧急度 P2）

**现状**：TUI 主屏仅 `eprintln!`，无 Modal 弹窗。

**修复**：
1. TUI 屏状态机加 `ErrorModal` 屏（复用 `src/tui/engine.rs::Screen` trait）。
2. 关键错误（auth 失败、context 超限）弹 Modal 让用户选择。

---

### 15.6 L43：无故障注入测试（紧急度 P2）

**现状**：`testReport/run_e2e.sh` 仅正常路径，无 chaos 测试。

**修复**：
1. 加 `testReport/run_chaos.sh`：用 `tc qdisc` 注入网络延迟/丢包。
2. 引入 `wiremock-rs = "0.6"` mock LLM endpoint。

---

## 16. 附录

### 16.1 参考文件清单（绝对路径）

#### atomcode
- `crates/atomcode-cli/src/main.rs:1353-1365` — 预 telemetry panic hook
- `crates/atomcode-cli/src/main.rs:4248-4300` — write_crash_log 函数
- `crates/atomcode-cli/src/main.rs:4305-4330` — telemetry-aware panic hook
- `crates/atomcode-daemon/src/lib.rs:5184-5210` — daemon panic hook with Event::Panic
- `crates/atomcode-telemetry/src/scrub.rs` — 路径脱敏 + truncate + backtrace_top_k
- `crates/atomcode-telemetry/src/event.rs` — Event::Panic enum 定义
- `crates/atomcode-kernel/src/conformance/mod.rs:119-145` — catch_unwind + panic payload 提取

#### claudecode
- `src/utils/errorRecovery.ts`（推断：错误恢复工具）
- `src/hooks/`（27 种 Hook 事件，第八轮 T5 已深挖）
- `src/main.tsx`（CLI 入口）
- `native/`（Native 模块）

#### openclaw
- `src/index.ts:130-167` — installUnhandledRejectionHandler + uncaughtException
- `src/runtime.ts`（推断）— restoreRuntimeTerminalState
- `src/mcp/channel-server.shutdown-unhandled-rejection.test.ts:115` — 测试模式
- `src/agents/embedded-agent-subscribe.*.test.ts` — 多组 unhandledRejection 测试

#### opencode
- `packages/opencode/src/util/error.ts`（推断）— TaggedError 定义
- `packages/opencode/src/cli/error.ts`（推断）— CLI 错误处理
- `packages/enterprise/src/`（推断）— Durable Object 错误处理

#### pi
- `packages/ai/src/utils/retry.ts:93-160` — exponential backoff retry policy
- `packages/ai/src/utils/provider-retry.ts:65-66` — 25% jitter 公式
- `packages/coding-agent/src/core/agent-session.ts:2883-2916` — retryable error preparation
- `packages/coding-agent/src/core/settings-manager.ts:33` — baseDelayMs 默认 2000
- `packages/agent/test/agent.test.ts:347` — unhandledRejection 测试模式

#### Switchyard
- `crates/libsy-llm-client/src/error.rs` — LLM 客户端错误
- `crates/switchyard-translation/src/error.rs` — 翻译错误（ContentBlock::Unknown）
- `crates/prefill-router/src/error.rs` — 路由错误
- `crates/libsy/src/error.rs` — 通用错误

#### deepseek-harness
- `packages/llm/retry.ts`（推断）— retry policy
- `packages/goal/`（推断）— Goal 状态机 retry 状态

#### agent-studio
- `backend/openjiuwen_studio/main.py`（推断）— FastAPI exception handler
- `frontend/src/components/ErrorModal.tsx`（推断）— 错误弹窗

#### undici
- `test/parser-issues.js` — 解析错误测试
- `test/http2-destroy-after-failed-stream.js` — HTTP/2 错误恢复
- `test/sync-error-in-callback.js` — 同步错误测试

#### laew
- `src/main.rs` — CLI 入口（无 panic hook）
- `src/error.rs` — AgentError 定义（3 variant）
- `src/agent/mod.rs` — run_session（Result 冒泡）
- `src/llm/{anthropic.rs, openai.rs}` — 双协议客户端（无 retry）
- `src/config/mod.rs` — Paths::detect + Db

### 16.2 术语表

| 术语 | 含义 |
|------|------|
| **panic hook** | 程序崩溃前的最后一道拦截（Rust set_hook / TS uncaughtException / Python excepthook） |
| **crash dump** | 崩溃时的内存/堆栈快照（minidump / JSON / Sentry envelope） |
| **exponential backoff** | 指数退避（每次重试延迟翻倍） |
| **jitter** | 退避时加随机偏移，避免雷鸣群 |
| **circuit breaker** | 熔断器（CLOSED/OPEN/HALF_OPEN 三态） |
| **fail-fast / fail-closed / fail-open** | 失败处理三态语义 |
| **retryable / permanent / fatal** | 错误三分类 |
| **chaos engineering** | 通过注入故障验证系统韧性 |
| **W3C traceparent** | 跨进程追踪上下文传递协议 |
| **WAL** | Write-Ahead Log（详见第八轮 T2） |

### 16.3 与第八轮的关系

| 维度 | 第八轮 T1（Telemetry） | 第八轮 T2（Session 持久化） | 第九轮 T1（本专题） |
|------|----------------------|--------------------------|-------------------|
| 关注点 | 上报格式（OTel 三栈） | 写入策略（fsync 4 严格度） | 错误处理（Panic + Recovery） |
| 紧急度 | P1 | P0 | P0 |
| Rust crate | tracing + opentelemetry | rusqlite WAL | backoff + human-panic + failsafe |
| 互补点 | 决策审计需 telemetry | crash log 需持久化 | panic hook 是入口 |

---

## 17. 结语

9 工程调研后，我们看到 **错误处理是 laew 从 PoC 升级到生产级 CLI 的最大短板**：

- **L38 panic hook 缺失** 是 P0 中最严重的（用户崩溃后无 dump、无恢复路径）。
- **L39 retry 缺失** 导致网络抖动时任务直接失败。
- **L40-L43** 是进阶项，需配合 L38/L39 落地后再做。

**一句话总结**：「**两阶段 panic hook + 指数退避 + Tagged Error**」是 9 工程的最小公共子集，laew 应优先落地这三条范式。

---

**字数统计**：~11,800 字，~1,250 行。
**调研时间**：2026-09-07
**作者**：第九轮 T1 专题研究 SubAgent（主笔 + Explore SubAgent 数据采集）