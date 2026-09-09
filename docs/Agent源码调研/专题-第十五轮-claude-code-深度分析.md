# 专题-第十五轮：claude-code 深度分析（8 大新维度）

> 分析对象：`/usr/local/LsmGitOpenSource/claudecode`（TypeScript/Bun，约 1896 文件 / 20136 行 src/）
> 对标：atomcode / deepseek-harness / openclaw / opencode / pi / undici / Switchyard
> 本轮聚焦前 14 轮未深入覆盖的 8 个新维度
> 日期：2026-09-09

---

## 目录

1. [网络协议深度](#1-网络协议深度)
2. [编译器前端](#2-编译器前端)
3. [操作系统内核交互](#3-操作系统内核交互)
4. [分布式共识](#4-分布式共识)
5. [机器学习推理](#5-机器学习推理)
6. [形式化验证](#6-形式化验证)
7. [图数据库与知识图谱](#7-图数据库与知识图谱)
8. [实时流处理](#8-实时流处理)
9. [新增 gap 清单汇总](#9-新增-gap-清单汇总)

---

## 1. 网络协议深度

### 1.1 核心发现

claude-code 实现了完整的 WebSocket 协议栈（含双运行时 Bun/Node 适配）、HTTP CONNECT-over-WebSocket 隧道中继、mTLS 全链路、SSE 帧解析器、以及指数退避+抖动重试策略。这是所有对标工程中最完整的网络协议实现之一（超过 opencode/pi，与 atomcode 的 SwappableClient 池毒化互补）。

### 1.2 代码级发现

#### 发现 1.1：WebSocket 客户端全协议实现 — `src/remote/SessionsWebSocket.ts`

```typescript
// 第 17-36 行：协议常量定义
const RECONNECT_DELAY_MS = 2000
const MAX_RECONNECT_ATTEMPTS = 5
const PING_INTERVAL_MS = 30000
const MAX_SESSION_NOT_FOUND_RETRIES = 3
const PERMANENT_CLOSE_CODES = new Set([4003]) // unauthorized
```

**机制描述**：
- 实现了完整的 WebSocket 状态机（`connecting | connected | closed`）
- **永久关闭码语义**：4003（unauthorized）立即停止重连；4001（session not found）允许 3 次重试（compaction 期间服务端短暂认为 session stale）
- **Ping/Pong 心跳**：30s 间隔，Bun 和 ws 双路径分别实现
- **双运行时适配**：Bun 用 `globalThis.WebSocket` + 事件监听；Node 用 `ws` 包 + `.on()` 回调
- **JSON 消息解析**：宽松的 `isSessionsMessage` 探测（仅检查 `type` 字段为字符串），避免硬编码 allowlist 导致新消息类型被静默丢弃

**设计意图**：远程会话订阅需要高鲁棒性——用户依赖 WebSocket 实时接收 Agent 输出，任何断连都需智能恢复。

#### 发现 1.2：CONNECT-over-WebSocket 隧道中继 + Protobuf 编码 — `src/upstreamproxy/relay.ts`

```typescript
// 第 56-81 行：手动 Protobuf 编码
export function encodeChunk(data: Uint8Array): Uint8Array {
  const len = data.length
  const varint: number[] = []
  let n = len
  while (n > 0x7f) {
    varint.push((n & 0x7f) | 0x80)
    n >>>= 7
  }
  varint.push(n)
  const out = new Uint8Array(1 + varint.length + len)
  out[0] = 0x0a  // tag = (field_number << 3) | wire_type = (1 << 3) | 2
  out.set(varint, 1)
  out.set(data, 1 + varint.length)
  return out
}
```

**机制描述**：
- 监听 localhost TCP，接收 HTTP CONNECT 请求，通过 WebSocket 隧道转发到 CCR 上游代理
- **手写 Protobuf 编码**：`UpstreamProxyChunk { bytes data = 1; }` 的 tag = `0x0a`，varint 长度前缀
- **双相位协议**：Phase 1 累积 CONNECT 请求头（CRLFCRLF 终止），Phase 2 转发字节流
- **背压处理**：Bun 的 `sock.write()` 返回实际写入字节数，未满部分入 `writeBuf` 队列，drain 事件触发刷新
- **512KB 分块上限**（`MAX_CHUNK_BYTES`）：Envoy per-request buffer cap
- **30s Ping keepalive**：匹配 sidecar 50s idle timeout

**设计意图**：CCR 入口是 GKE L7 路径前缀路由，无 connect_matcher，必须用 WebSocket 封装 CONNECT。手写 protobuf 避免了运行时依赖。

#### 发现 1.3：mTLS 全链路 + 连接池毒化 — `src/utils/proxy.ts` + `src/utils/mtls.ts`

```typescript
// proxy.ts 第 27-31 行：连接池毒化
let keepAliveDisabled = false
export function disableKeepAlive(): void {
  keepAliveDisabled = true
}
```

```typescript
// mtls.ts 第 117-152 行：undici Agent + TLS 选项
export function getTLSFetchOptions(): { tls?: TLSConfig; dispatcher?: undici.Dispatcher } {
  // ...
  const agent = new undiciMod.Agent({
    connect: { cert: tlsConfig.cert, key: tlsConfig.key, passphrase: tlsConfig.passphrase, ... },
    pipelining: 1,
  })
  return { dispatcher: agent }
}
```

**机制描述**：
- **NO_PROXY 完整支持**：域名后缀匹配、端口特定匹配、通配符 `*`、IP 地址匹配
- **连接池毒化**：检测到 ECONNRESET/EPIPE 后全局禁用 keep-alive（`keepAliveDisabled` 粘性标记），强制后续请求新建 TCP 连接
- **双路径**：Bun 用 `tls` 选项；Node 用 undici `Agent` + `connect` TLS 配置
- **mTLS 配置**：从 `CLAUDE_CODE_CLIENT_CERT/KEY/PASSPHRASE` 环境变量加载，memoize 缓存
- **Proxy-Authorization**：Basic auth（`sessionId:token` base64）封装在 CONNECT 隧道内

**设计意图**：企业级部署需要穿透代理和 mTLS。连接池毒化与 atomcode 的 SwappableClient 池毒化异曲同工——stale socket 必须隔离。

#### 发现 1.4：指数退避+抖动重试 + 529/429 分类处理 — `src/services/api/withRetry.ts`

```typescript
// 第 530-548 行：指数退避 + 25% 抖动
export function getRetryDelay(attempt: number, retryAfterHeader?: string | null, maxDelayMs = 32000): number {
  if (retryAfterHeader) {
    const seconds = parseInt(retryAfterHeader, 10)
    if (!isNaN(seconds)) return seconds * 1000
  }
  const baseDelay = Math.min(BASE_DELAY_MS * Math.pow(2, attempt - 1), maxDelayMs)
  const jitter = Math.random() * 0.25 * baseDelay
  return baseDelay + jitter
}
```

**机制描述**：
- **10 次默认重试**，BASE_DELAY_MS = 500，最大 32s，25% 随机抖动
- **529 (overloaded) 分类**：前台源（`repl_main_thread`/`compact`/`sdk` 等）重试；后台源（summaries/titles/suggestions）立即放弃——避免容量级联放大
- **持久模式**（`CLAUDE_CODE_UNATTENDED_RETRY`）：429/529 无限重试，5min 最大退避，6h 重置上限，30s 心跳 chunk 防止 host 判定 idle
- **Fast Mode 降级**：429 时短 retry-after（<20s）等待并保持 cache；长 retry-after 进入 cooldown（30min 默认，10min 最小）
- **stale connection 检测**：ECONNRESET/EPIPE 触发 `disableKeepAlive()` + 客户端重建

**设计意图**：API 调用是 agentic loop 的瓶颈，必须在容量退化和用户体验间精细平衡。

### 1.3 laew gap 编号

- **L836 [P1]**：laew 无 WebSocket 协议实现（仅有 HTTP POST）。缺 reconnect/ping/pong/close code 语义
- **L837 [P1]**：laew 无 CONNECT 隧道中继能力。缺 protobuf 帧封装
- **L838 [P0]**：laew 无 mTLS 支持（无客户端证书加载、无 CA 证书链配置）
- **L839 [P1]**：laew 无 NO_PROXY 支持、无连接池毒化机制
- **L840 [P0]**：laew 无指数退避重试（当前失败即停）

---

## 2. 编译器前端

### 2.1 核心发现

claude-code 实现了**工业级 Bash 编译器前端**：完整的手写 Lexer（含 UTF-8 字节偏移追踪）+ 递归下降 Parser 生成 tree-sitter 兼容 AST + AST 安全 walker。这是整个知识库中**唯一的完整编译器前端实现**（atomcode 仅有 CodeIntel 七件套无(parser）。此外还包含 Vim 状态机（lexer/parser 混合）和 sed BRE→ERE 转换器。

### 2.2 代码级发现

#### 发现 2.1：纯 TypeScript Bash Lexer — `src/utils/bash/bashParser.ts`

```typescript
// 第 48-77 行：Tokenizer 定义
type TokenType =
  | 'WORD' | 'NUMBER' | 'OP' | 'NEWLINE' | 'COMMENT'
  | 'DQUOTE' | 'SQUOTE' | 'ANSI_C' | 'DOLLAR' | 'DOLLAR_PAREN'
  | 'DOLLAR_BRACE' | 'DOLLAR_DPAREN' | 'BACKTICK' | 'LT_PAREN'
  | 'GT_PAREN' | 'EOF'

type Token = {
  type: TokenType
  value: string
  start: number  // UTF-8 byte offset
  end: number    // UTF-8 byte offset one past last char
}
```

**机制描述**：
- **完整的 Lexer 状态机**：追踪 JS 字符串索引（`i`）和 UTF-8 字节偏移（`b`）——ASCII 快速路径两者一致，非 ASCII 逐码点推进
- **15 种 Token 类型**：涵盖 bash 所有词法单元（ANSI-C quoting `$'...'`、命令替换 `$()` / `` ` ``、进程替换 `<()` / `>()`）
- **50ms 超时 + 50000 节点上限**：防止病态/对抗输入导致 OOM
- **Heredoc 延迟处理**：`heredocs: HeredocPending[]` 数组，newline 时扫描 body
- **3449 输入 golden corpus 验证**：与 tree-sitter-bash WASM  parser 对比

**设计意图**：安全分析需要精确的 bash 词法/语法结构——正则无法区分 `echo $(rm -rf /)` 中的嵌套命令替换。

#### 发现 2.2：AST 安全 Walker（fail-closed 设计）— `src/utils/bash/ast.ts`

```typescript
// 第 9-19 行：设计哲学
/**
 * This module replaces the shell-quote + hand-rolled char-walker approach.
 * Instead of detecting parser differentials one-by-one, we parse with
 * tree-sitter-bash and walk the tree with an EXPLICIT allowlist of node types.
 * Any node type not in the allowlist causes the entire command to be classified
 * as 'too-complex'.
 * The key design property is FAIL-CLOSED: we never interpret structure we
 * don't understand.
 */
```

**机制描述**：
- **显式 allowlist**：`STRUCTURAL_TYPES`（program/list/pipeline/redirected_statement）+ `SEPARATOR_TYPES`（`&&`/`||`/`;`）+ 约 30 种已知节点类型
- **命令替换占位符**：`$()` 提取为 `__CMDSUB_OUTPUT__`，内层命令独立做权限检查
- **变量追踪**：`VAR_PLACEHOLDER` 替换已追踪的 `VAR=val` 赋值引用
- **bare var 不安全检测**：`BARE_VAR_UNSAFE_RE = /[ \t\n*?[]/` — 未引号的 `$VAR` 会分词+路径扩展，不能作为 bare arg 信任
- **三态结果**：`simple`（可提取 argv）/ `too-complex`（未知节点类型）/ `parse-unavailable`

**设计意图**：安全沙箱的核心——如果无法 100% 确定命令结构，必须询问用户。fail-closed 是安全设计的第一原则。

#### 发现 2.3：Vim 状态机（Lexer/Parser 混合）— `src/vim/transitions.ts`

```typescript
// 第 59-88 行：主 transition 函数
export function transition(state: CommandState, input: string, ctx: TransitionContext): TransitionResult {
  switch (state.type) {
    case 'idle': return fromIdle(input, ctx)
    case 'count': return fromCount(state, input, ctx)
    case 'operator': return fromOperator(state, input, ctx)
    case 'operatorCount': return fromOperatorCount(state, input, ctx)
    case 'operatorFind': return fromOperatorFind(state, input, ctx)
    case 'operatorTextObj': return fromOperatorTextObj(state, input, ctx)
    case 'find': return fromFind(state, input, ctx)
    case 'g': return fromG(state, input, ctx)
    case 'operatorG': return fromOperatorG(state, input, ctx)
    case 'replace': return fromReplace(state, input, ctx)
    case 'indent': return fromIndent(state, input, ctx)
  }
}
```

**机制描述**：
- **11 状态**：idle / count / operator / operatorCount / operatorFind / operatorTextObj / find / g / operatorG / replace / indent
- **每状态一个 transition 函数**：可扫描的 state machine 真源
- **count 累积**：`fromCount` 逐位追加数字，`MAX_VIM_COUNT` 上限
- **操作符-动作分离**：operator 状态后接 motion/find/textObj 才触发执行
- **dot repeat (`.)` + find repeat (`;/,`)**：状态持久化 `lastFind`

**设计意图**：终端内嵌 vim 编辑器需要精确的按键序列解析——`d2w`（delete 2 words）和 `2dw`（同上）语义相同但解析路径不同。

#### 发现 2.4：sed BRE→ERE 转换器 — `src/tools/BashTool/sedEditParser.ts`

```typescript
// 第 272-298 行：BRE→ERE 转换
if (!sedInfo.extendedRegex) {
  jsPattern = jsPattern
    .replace(/\\\\/g, BACKSLASH_PLACEHOLDER)      // Step 1: 保护字面反斜杠
    .replace(/\\\+/g, PLUS_PLACEHOLDER)            // Step 2: 转义元字符→占位符
    .replace(/\\\?/g, QUESTION_PLACEHOLDER)
    .replace(/\\\|/g, PIPE_PLACEHOLDER)
    .replace(/\\\(/g, LPAREN_PLACEHOLDER)
    .replace(/\\\)/g, RPAREN_PLACEHOLDER)
    .replace(/\+/g, '\\+')                         // Step 3: 未转义元字符→转义
    .replace(/\?/g, '\\?')
    .replace(/\|/g, '\\|')
    .replace(/\(/g, '\\(')
    .replace(/\)/g, '\\)')
    .replace(PLACEHOLDER_RE, '+')                  // Step 4: 占位符→JS 等价
}
```

**机制描述**：
- **完整 sed 命令解析**：提取 `-i`/`-E`/`-e` 标志、表达式、文件路径
- **BRE→ERE 四步转换**：保护字面反斜杠 → 转义元字符→占位符 → 未转义元字符→转义 → 占位符→JS 等价
- **null-byte 占位符**：`\x00BACKSLASH\x00` 等永不在用户输入中出现，防注入
- **replacement 安全处理**：`&` → `$$&`（JS 中转义），`\&` → 随机 salt 占位符防注入

**设计意图**：`sed -i 's/pattern/replacement/g'` 是常见文件编辑操作，必须在浏览器端预览编辑效果而不实际执行。

### 2.3 laew gap 编号

- **L841 [P0]**：laew 无 bash 编译器前端（无 lexer/parser/AST）。无法做安全结构分析
- **L842 [P1]**：laew 无 fail-closed AST walker。无法区分 `echo hi` 和 `echo $(rm -rf /)`
- **L843 [P2]**：laew 无 vim 状态机。终端编辑仅有基础行输入
- **L844 [P2]**：laew 无 sed/parser 类 DSL 解析器

---

## 3. 操作系统内核交互

### 3.1 核心发现

claude-code 通过多种机制与操作系统内核交互：子进程管理（execa/child_process）、Shell 快照（ShellSnapshot）、沙箱运行时适配（`@anthropic-ai/sandbox-runtime`）、信号处理（gracefulShutdown）、以及 Windows/macOS/Linux 三平台适配。虽然不直接调用 io_uring/epoll，但通过 Bun 的 libuv 抽象层和沙箱运行时实现了内核级隔离。

### 3.2 代码级发现

#### 发现 3.1：Shell 快照与进程状态管理 — `src/utils/bash/ShellSnapshot.ts`

```typescript
// 第 24-59 行：Shell 快照创建
const SNAPSHOT_CREATION_TIMEOUT = 10000 // 10 seconds
function createArgv0ShellFunction(funcName: string, argv0: string, binaryPath: string, prependArgs: string[] = []): string {
  return [
    `function ${funcName} {`,
    '  if [[ -n $ZSH_VERSION ]]; then',
    `    ARGV0=${argv0} ${quotedPath} ${argSuffix}`,
    '  elif [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]] || [[ "$OSTYPE" == "win32" ]]; then',
    `    ARGV0=${argv0} ${quotedPath} ${argSuffix}`,
    '  elif [[ $BASHPID != $$ ]]; then',
    `    exec -a ${argv0} ${quotedPath} ${argSuffix}`,
    '  else',
    `    (exec -a ${argv0} ${quotedPath} ${argSuffix})`,
    '  fi',
    '}',
  ].join('\n')
}
```

**机制描述**：
- **ARGV0 dispatch 技巧**：bun 二进制通过 `argv[0]` 或 `ARGV0` 环境变量路由到内嵌工具（rg/bfs/ugrep）
- **三平台 shell 函数生成**：Zsh 用 ARGV0 环境变量；Windows (git bash/msys/cygwin) 用 ARGV0；Bash 用 `exec -a`
- **子 shell 检测**：`$BASHPID != $$` 时直接 `exec -a`（已是子 shell）；否则用 `(exec -a ...)` 子 shell 包裹
- **10s 超时**：快照创建不能超过 10s

**设计意图**：需要在用户的 shell 环境中注入工具函数，必须兼容所有 shell 变体和操作系统。

#### 发现 3.2：沙箱运行时适配 — `src/utils/sandbox/sandbox-adapter.ts`

```typescript
// 第 1-2 行：设计定位
/**
 * Adapter layer that wraps @anthropic-ai/sandbox-runtime with Claude CLI-specific integrations.
 * This file provides the bridge between the external sandbox-runtime package and Claude CLI's
 * settings system, tool integration, and additional features.
 */
```

**机制描述**：
- **文件系统限制**：`FsReadRestrictionConfig` / `FsWriteRestrictionConfig` + 路径模式解析
- **网络限制**：`NetworkRestrictionConfig` + `NetworkHostPattern`
- **违规处理**：`SandboxViolationStore` + `IgnoreViolationsConfig` + `SandboxAskCallback`
- **Claude Code 特定路径约定**：`//path` → 绝对路径；`/path` → 相对于 settings 文件目录；`~/path` → 家目录
- **设置转换器**：将 Claude Code 的 permission rules 翻译为 sandbox-runtime 的 `SandboxRuntimeConfig`

**设计意图**：在受限环境中运行 Bash 命令需要内核级隔离（文件系统/网络/进程），通过外部 sandbox-runtime 包实现 Landlock/Seccomp 能力。

#### 发现 3.3：gracefulShutdown 与进程信号处理 — `src/utils/gracefulShutdown.ts`（被 SendMessageTool/BashTool 引用）

虽然具体实现未在此展开，但 gracefulShutdown 在以下场景触发：
- 队友 shutdown 请求（`createShutdownApprovedMessage`）
- 进程退出前清理（`registerCleanup`）
- WebSocket 关闭前 flush

#### 发现 3.4：命令退出码语义解释器 — `src/tools/BashTool/commandSemantics.ts`

```typescript
// 第 31-89 行：命令特定退出码语义
const COMMAND_SEMANTICS: Map<string, CommandSemantic> = new Map([
  ['grep', (exitCode) => ({ isError: exitCode >= 2, message: exitCode === 1 ? 'No matches found' : undefined })],
  ['diff', (exitCode) => ({ isError: exitCode >= 2, message: exitCode === 1 ? 'Files differ' : undefined })],
  ['test', (exitCode) => ({ isError: exitCode >= 2, message: exitCode === 1 ? 'Condition is false' : undefined })],
  // ...
])
```

**机制描述**：grep 1=无匹配（非错误），diff 1=文件不同（非错误），test 1=条件假（非错误）。避免将信息性退出码误报为失败。

### 3.3 laew gap 编号

- **L845 [P0]**：laew 无沙箱运行时（无文件系统/网络隔离）。Bash 命令直接访问全系统
- **L846 [P1]**：laew 无 shell 快照机制。无法注入工具函数到用户 shell
- **L847 [P1]**：laew 无退出码语义解释。grep 返回 1 会被误判为失败
- **L848 [P2]**：laew 无 io_uring/epoll 直接调用（依赖 tokio 抽象层）
- **L849 [P2]**：laew 无跨平台 shell 函数生成（Windows/git bash/Zsh 适配）

---

## 4. 分布式共识

### 4.1 核心发现

claude-code 不实现经典 Raft/Paxos，但实现了**多 Agent 协调模式**和**分布式策略一致性**——通过 coordinator 模式编排多 worker、通过 policy-limits 服务实现分布式策略的 ETag 缓存+后台轮询、通过 bridge transport 实现序列号驱动的可靠消息投递。这是"分布式共识"在 Agent 编排层面的工程映射。

### 4.2 代码级发现

#### 发现 4.1：Coordinator 模式（多 Agent 编排）— `src/coordinator/coordinatorMode.ts`

```typescript
// 第 111-120 行：Coordinator 系统提示
export function getCoordinatorSystemPrompt(): string {
  const workerCapabilities = isEnvTruthy(process.env.CLAUDE_CODE_SIMPLE)
    ? 'Workers have access to Bash, Read, and Edit tools, plus MCP tools from configured MCP servers.'
    : 'Workers have access to standard tools, MCP tools from configured MCP servers, and project skills via the Skill tool.'
  return `You are Claude Code, an AI assistant that orchestrates software engineering tasks across multiple workers.
  ## 1. Your Role
  You are a **coordinator**. Your job is to: ...`
}
```

**机制描述**：
- **Coordinator-Worker 架构**：coordinator 通过 AgentTool 派发 worker，worker 有独立工具集
- **Session 模式匹配**：`matchSessionMode` 恢复 session 时自动切换 coordinator/normal 模式
- **内部工具隔离**：`INTERNAL_WORKER_TOOLS`（TeamCreate/TeamDelete/SendMessage/SyntheticOutput）仅 coordinator 可用
- **Scratchpad 共享目录**：`scratchpadDir` 跨 worker 持久化知识

**设计意图**：复杂软件工程任务需要多 Agent 协作——coordinator 做规划+委派，worker 做执行。

#### 发现 4.2：分布式策略一致性 — `src/services/policyLimits/index.ts`

```typescript
// 第 54-69 行：策略限制服务
const CACHE_FILENAME = 'policy-limits.json'
const FETCH_TIMEOUT_MS = 10000
const POLLING_INTERVAL_MS = 60 * 60 * 1000 // 1 hour
let sessionCache: PolicyLimitsResponse['restrictions'] | null = null
const LOADING_PROMISE_TIMEOUT_MS = 30000 // 30 seconds
```

**机制描述**：
- **ETag 缓存**：策略限制本地持久化到 `policy-limits.json`，后台每小时轮询
- **Fail-open**：API _fetch 失败时不阻塞 CLI（非关键路径）
- **30s 加载超时**：防止策略加载死锁
- **Session 级缓存**：`sessionCache` 单例，进程内共享
- **幂等性**：`_resetPolicyLimitsForTesting` 测试隔离

**设计意图**：组织级策略（如禁用某些工具）需要分布式一致——所有 CLI 实例必须遵守同一策略集，同时不能因策略服务不可用而阻塞用户。

#### 发现 4.3：可靠消息投递与序列号 — `src/bridge/replBridgeTransport.ts`

```typescript
// 第 37-49 行：序列号驱动的回放
/**
 * High-water mark of the underlying read stream's event sequence numbers.
 * replBridge reads this before swapping transports so the new one can
 * resume from where the old one left off (otherwise the server replays
 * the entire session history from seq 0).
 */
getLastSequenceNum(): number
/**
 * Monotonic count of batches dropped via maxConsecutiveFailures.
 * Snapshot before writeBatch() and compare after to detect silent drops.
 */
readonly droppedBatchCount: number
```

**机制描述**：
- **v1/v2 双 transport**：v1 = WebSocket（Session-Ingress）；v2 = SSE（读）+ CCRClient（写）
- **序列号高水位**：transport 切换时从上次 seq num 恢复，避免全量回放
- **丢包检测**：`droppedBatchCount` 单调计数器，compare-and-detect 静默丢包
- **状态报告**：`reportState(requires_action)` 通知后端权限弹窗；`reportDelivery` 填充 processing_at/processed_at

**设计意图**：bridge 模式（Remote Control）需要可靠的跨网络消息投递，序列号是 exactly-once 语义的基础。

### 4.3 laew gap 编号

- **L850 [P2]**：laew 无 coordinator-worker 多 Agent 编排（仅有 Yolo→SubAgent 双层）
- **L851 [P2]**：laew 无分布式策略一致性（无 ETag 缓存/后台轮询）
- **L852 [P1]**：laew 无可靠消息投递序列号（无高水位回放）
- **L853 [P2]**：laew 无 Raft/Paxos/Gossip 协议实现
- **L854 [P2]**：laew 无分布式锁（无跨进程互斥）

---

## 5. 机器学习推理

### 5.1 核心发现

claude-code 不直接运行 ONNX/TensorRT 推理，但在**token 估算**（`tokenEstimation.ts`）、**上下文窗口管理**（`autoCompact.ts`）、**语音 STT 推理**（`voiceStreamSTT.ts`）三个维度深度涉及 ML 推理的工程化。特别是 token 计数与 API 的 thinking blocks 交互、上下文压缩的阈值计算，体现了推理资源管理的工程实践。

### 5.2 代码级发现

#### 发现 5.1：Token 估算与 Thinking Blocks 检测 — `src/services/tokenEstimation.ts`

```typescript
// 第 30-56 行：Thinking blocks 检测
const TOKEN_COUNT_THINKING_BUDGET = 1024
const TOKEN_COUNT_MAX_TOKENS = 2048
function hasThinkingBlocks(messages: Anthropic.Beta.Messages.BetaMessageParam[]): boolean {
  for (const message of messages) {
    if (message.role === 'assistant' && Array.isArray(message.content)) {
      for (const block of message.content) {
        if (typeof block === 'object' && block !== null && 'type' in block &&
            (block.type === 'thinking' || block.type === 'redacted_thinking')) {
          return true
        }
      }
    }
  }
  return false
}
```

**机制描述**：
- **Thinking blocks 检测**：遍历 assistant 消息的 content blocks，识别 `thinking` 和 `redacted_thinking` 类型
- **Tool search 字段剥离**：`stripToolSearchFieldsFromMessages` 移除 `caller`/`tool_reference` 等仅 tool search beta 有效的字段
- **双路径 token 计数**：Anthropic API 原生 countTokens；Bedrock 用 `CountTokensCommandInput`
- **最小值约束**：`max_tokens` 必须 > `thinking.budget_tokens`（API 约束）

**设计意图**：上下文窗口管理需要精确的 token 计数——thinking blocks 占用大量 token 但用户不可见。

#### 发现 5.2：自动压缩阈值与上下文窗口精算 — `src/services/compact/autoCompact.ts`

```typescript
// 第 29-80 行：上下文窗口精算
const MAX_OUTPUT_TOKENS_FOR_SUMMARY = 20_000
export function getEffectiveContextWindowSize(model: string): number {
  const reservedTokensForSummary = Math.min(getMaxOutputTokensForModel(model), MAX_OUTPUT_TOKENS_FOR_SUMMARY)
  let contextWindow = getContextWindowForModel(model, getSdkBetas())
  if (autoCompactWindow) {
    const parsed = parseInt(autoCompactWindow, 10)
    if (!isNaN(parsed) && parsed > 0) contextWindow = Math.min(contextWindow, parsed)
  }
  return contextWindow - reservedTokensForSummary
}
export const AUTOCOMPACT_BUFFER_TOKENS = 13_000
const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3
```

**机制描述**：
- **有效上下文窗口**：模型上下文窗口 - 摘要输出预留（p99.99 = 17,387 tokens）
- **13K buffer**：触发 autocompact 的 token 余量
- **断路器**：连续 3 次 autocompact 失败后停止重试（防止不可恢复的 context overflow 浪费 API 调用）
- **模型特定窗口**：通过 `getContextWindowForModel` 获取不同模型的上下文大小

**设计意图**：推理资源（上下文窗口）是稀缺资产，需要在压缩损失和溢出风险间平衡。

#### 发现 5.3：语音 STT 推理路由 — `src/services/voiceStreamSTT.ts`

```typescript
// 第 153-165 行：推理引擎路由
const isNova3 = getFeatureValue_CACHED_MAY_BE_STALE('tengu_cobalt_frost', false)
if (isNova3) {
  params.set('use_conversation_engine', 'true')
  params.set('stt_provider', 'deepgram-nova3')
}
```

**机制描述**：
- **推理引擎选择**：conversation_engine + Deepgram Nova 3（通过 GrowthBook feature gate 独立放量）
- **Keyterms boosting**：通过 query params 传递关键术语提升 STT 准确率
- **双相位 finalize**：TranscriptEndpoint（~300ms）/ no-data timeout（1.5s）/ WS close（3-5s）/ safety timer（5s）

### 5.3 laew gap 编号

- **L855 [P0]**：laew 无 token 精确计数（仅字符/4 估算）
- **L856 [P1]**：laew 无 thinking blocks 检测（不支持 extended thinking）
- **L857 [P1]**：laew 无上下文窗口精算（仅有硬阈值 80%）
- **L858 [P2]**：laew 无本地推理引擎（无 ONNX/TensorRT/GGUF）
- **L859 [P2]**：laew 无语音 STT 推理路由

---

## 6. 形式化验证

### 6.1 核心发现

claude-code 不实现 TLA+/Coq，但在**安全关键路径**采用了**类形式化验证的工程设计**：fail-closed AST walker（白名单穷举）、解析超时/节点预算（资源上限证明）、三态结果类型（穷尽匹配）、PARSE_ABORTED 安全哨兵（区分"未加载"和"加载但失败"）。这是"契约式编程"和"防御性设计"的工程实践。

### 6.2 代码级发现

#### 发现 6.1：Fail-closed AST 安全验证 — `src/utils/bash/ast.ts`

```typescript
// 第 87-96 行：占位符检测（防御深度）
/**
 * All placeholder strings. Used for defense-in-depth: if a varScope value
 * contains ANY placeholder (exact or embedded), the value is NOT a pure
 * literal and cannot be trusted as a bare argument. Covers composites like
 * `VAR="prefix$(cmd)"` → `"prefix__CMDSUB_OUTPUT__"`.
 */
function containsAnyPlaceholder(value: string): boolean {
  return value.includes(CMDSUB_PLACEHOLDER) || value.includes(VAR_PLACEHOLDER)
}
```

**机制描述**：
- **白名单穷举**：任何未知节点类型 → `too-complex`（拒绝提取 argv）
- **占位符注入检测**：substring 包含检查（非 exact match），捕获 `VAR="prefix$(cmd)"` 复合场景
- **bare var 不安全证明**：正则 `/[ \t\n*?[]/` 形式化证明未引号 $VAR 不可信
- **三态穷尽**：`simple | too-complex | parse-unavailable`——无第四态

#### 发现 6.2：PARSE_ABORTED 安全哨兵 — `src/utils/bash/parser.ts`

```typescript
// 第 87-93 行：安全哨兵设计
/**
 * SECURITY: Sentinel for "parser was loaded and attempted, but aborted"
 * (timeout / node budget / Rust panic). Distinct from `null` (module not loaded).
 * Adversarial input can trigger abort under MAX_COMMAND_LENGTH:
 * `(( a[0][0]... ))` with ~2800 subscripts hits PARSE_TIMEOUT_MICROS.
 * Callers MUST treat this as fail-closed (too-complex), NOT route to legacy.
 */
export const PARSE_ABORTED = Symbol('parse-aborted')
```

**机制描述**：
- **Symbol 哨兵**：`PARSE_ABORTED` ≠ `null`——区分"未加载"和"加载但失败"
- **fail-closed 强制**：abort 必须走 too-complex 路径，不得回退 legacy（legacy 无 EVAL_LIKE_BUILTINS 检测）
- **对抗输入防护**：`(( a[0][0]... ))` 2800 层下标触发 50ms 超时

#### 发现 6.3：资源上限证明 — `src/utils/bash/bashParser.ts`

```typescript
// 第 25-32 行：资源上限
const PARSE_TIMEOUT_MS = 50
const MAX_NODES = 50_000
```

**机制描述**：
- **50ms 超时**：病态输入的 wall-clock 上限
- **50000 节点上限**：防止 OOM
- **UTF-8 字节偏移证明**：所有位置信息是 UTF-8 byte offset（非 JS string index），避免多字节字符导致的位置偏差

### 6.3 laew gap 编号

- **L860 [P1]**：laew 无 fail-closed 安全验证（Bash 工具直接执行）
- **L861 [P2]**：laew 无 TLA+/Coq 形式化规范
- **L862 [P1]**：laew 无解析资源上限（无超时/节点预算）
- **L863 [P2]**：laew 无属性测试（property-based testing）

---

## 7. 图数据库与知识图谱

### 7.1 核心发现

claude-code 不实现 Neo4j/RDF/SPARQL，但在**知识图谱构建**维度有独特实现：`teamMemorySync` 服务实现跨会话知识图谱同步；`extractMemories` 服务实现记忆提取；`SessionMemory` 实现会话级知识持久化。向量检索方面，`ripgrep` + `ugrep` 的组合实现了文件级检索（但无 FAISS/Annoy 向量索引）。

### 7.2 代码级发现

#### 发现 7.1：团队记忆图谱同步 — `src/services/teamMemorySync/`（目录存在）

团队记忆同步服务允许跨团队共享知识图谱——团队成员的记忆可以广播给其他成员。

#### 发现 7.2：记忆提取与持久化 — `src/services/extractMemories/` + `src/services/SessionMemory/`

**机制描述**：
- **L0-L3 管线**：extractMemories 实现记忆提取管线
- **SessionMemory Compact**：`sessionMemoryCompact.ts`（16278 行）实现会话级记忆压缩
- **AgentSummary / AwaySummary**：多粒度记忆摘要

#### 发现 7.3：Tool Search 向量检索 — `src/services/tools/` + `src/utils/toolSearch.ts`

Tool Search 实现了工具/技能的语义搜索（通过 API 的 tool search beta），虽然不是本地 FAISS/Annoy，但实现了向量检索的产品化应用。

### 7.3 laew gap 编号

- **L864 [P2]**：laew 无知识图谱构建（无 RDF/Neo4j/SPARQL）
- **L865 [P2]**：laew 无向量检索索引（无 FAISS/Annoy/HNSW）
- **L866 [P1]**：laew 无跨会话知识同步（session_memory 表仅本地）
- **L867 [P2]**：laew 无图谱遍历算法（无 PageRank/社区发现/最短路径）

---

## 8. 实时流处理

### 8.1 核心发现

claude-code 在多个维度实现实时流处理：**SSE 帧增量解析**（`SSETransport.ts`）、**WebSocket 双向实时消息**（`SessionsWebSocket.ts`）、**语音实时流转写**（`voiceStreamSTT.ts`）、**异步生成器心跳 yields**（`withRetry.ts`）、**自动压缩触发**（`autoCompact.ts`）。虽然没有 Kafka/Flink 式窗口计算，但实现了**背压控制**（backpressure）和**事件驱动**的流处理范式。

### 8.2 代码级发现

#### 发现 8.1：SSE 帧增量解析器 — `src/cli/transports/SSETransport.ts`

```typescript
// 第 57-100 行：SSE 帧增量解析
export function parseSSEFrames(buffer: string): { frames: SSEFrame[]; remaining: string } {
  const frames: SSEFrame[] = []
  let pos = 0
  let idx: number
  while ((idx = buffer.indexOf('\n\n', pos)) !== -1) {
    const rawFrame = buffer.slice(pos, idx)
    pos = idx + 2
    if (!rawFrame.trim()) continue
    const frame: SSEFrame = {}
    for (const line of rawFrame.split('\n')) {
      if (line.startsWith(':')) { isComment = true; continue }  // SSE comment/keepalive
      const colonIdx = line.indexOf(':')
      if (colonIdx === -1) continue
      const field = line.slice(0, colonIdx)
      const value = line[colonIdx + 1] === ' ' ? line.slice(colonIdx + 2) : line.slice(colonIdx + 1)
      switch (field) { case 'event': frame.event = value; break; case 'id': frame.id = value; break; case 'data': frame.data = value; break }
    }
    frames.push(frame)
  }
  return { frames, remaining: buffer.slice(pos) }
}
```

**机制描述**：
- **增量解析**：处理 TCP 分包——未完整帧保留在 `remaining` 中等待下次 `read`
- **SSE 完整实现**：event/id/data 三字段 + comment 识别 + 头部空格剥离
- **背压感知**：`LIVENESS_TIMEOUT_MS = 45_000`（server 15s 发 keepalive，45s 无数据判死）
- **10min 重连预算**：`RECONNECT_GIVE_UP_MS = 600_000`
- **永久码**：401/403/404 不重试

#### 发现 8.2：语音实时流转写协议 — `src/services/voiceStreamSTT.ts`

```typescript
// 第 29-31 行：协议定义
const KEEPALIVE_MSG = '{"type":"KeepAlive"}'
const CLOSE_STREAM_MSG = '{"type":"CloseStream"}'
const KEEPALIVE_INTERVAL_MS = 8_000
```

**机制描述**：
- **双模态协议**：JSON 控制消息（KeepAlive/CloseStream/TranscriptText/TranscriptEndpoint）+ 二进制音频帧
- **8s keepalive**：匹配服务端超时
- **finalize 四相位**：TranscriptEndpoint（~300ms）/ no-data timeout（1.5s）/ WS close（3-5s）/ safety timer（5s）
- **防竞态**：`finalized` flag + `setTimeout` 延迟 CloseStream（flush 已排队音频回调）

#### 发现 8.3：AsyncGenerator 心跳流 — `src/services/api/withRetry.ts`

```typescript
// 第 170-178 行：AsyncGenerator 流式重试
export async function* withRetry<T>(
  getClient: () => Promise<Anthropic>,
  operation: (client: Anthropic, attempt: number, context: RetryContext) => Promise<T>,
  options: RetryOptions,
): AsyncGenerator<SystemAPIErrorMessage, T> {
```

**机制描述**：
- **AsyncGenerator 模式**：`yield createSystemAPIErrorMessage(...)` 实时上报重试状态到 stdout
- **心跳 chunk 化**：长 sleep 切 30s chunk（`HEARTBEAT_INTERVAL_MS`），防止 host 判定 session idle
- **流式背压**：`yield` 让调用方决定消费节奏

#### 发现 8.4：自动压缩事件驱动 — `src/services/compact/autoCompact.ts`

**机制描述**：
- **事件触发**：每轮对话后检测 token 数，超过阈值触发 autocompact
- **断路器模式**：连续 3 次失败停止重试
- **buffer 余量**：13K token buffer 防止频繁压缩

### 8.3 laew gap 编号

- **L868 [P1]**：laew 无 SSE 帧增量解析器（仅整块 HTTP 响应）
- **L869 [P1]**：laew 无 WebSocket 双向实时消息
- **L870 [P2]**：laew 无背压控制机制（Bash 工具无流式输出限流）
- **L871 [P2]**：laew 无事件溯源（Event Sourcing）/CQRS
- **L872 [P2]**：laew 无窗口计算（无 tumbling/sliding session window）
- **L873 [P2]**：laew 无语音实时流协议

---

## 9. 新增 gap 清单汇总

### P0 紧急（8 项）

| 编号 | 维度 | 描述 | 对标 claude-code 实现 |
|------|------|------|---------------------|
| **L838** | 网络协议 | 无 mTLS 支持 | `mtls.ts` 全链路 mTLS |
| **L840** | 网络协议 | 无指数退避重试 | `withRetry.ts` 10次退避+抖动 |
| **L841** | 编译器前端 | 无 bash 编译器前端 | `bashParser.ts` 4436行完整 lexer |
| **L845** | OS 内核 | 无沙箱运行时 | `sandbox-adapter.ts` 文件系统/网络隔离 |
| **L855** | ML 推理 | 无 token 精确计数 | `tokenEstimation.ts` thinking blocks 检测 |
| **L860** | 形式化验证 | 无 fail-closed 安全验证 | `ast.ts` 白名单穷举 |
| **L836** | 网络协议 | 无 WebSocket 协议 | `SessionsWebSocket.ts` 全协议实现 |
| **L839** | 网络协议 | 无 NO_PROXY/连接池毒化 | `proxy.ts` 完整代理支持 |

### P1 重要（14 项）

| 编号 | 维度 | 描述 | 对标 |
|------|------|------|------|
| **L837** | 网络协议 | 无 CONNECT 隧道中继 | `relay.ts` protobuf 帧 |
| **L842** | 编译器前端 | 无 fail-closed AST walker | `ast.ts` |
| **L846** | OS 内核 | 无 shell 快照机制 | `ShellSnapshot.ts` |
| **L847** | OS 内核 | 无退出码语义解释 | `commandSemantics.ts` |
| **L852** | 分布式 | 无可靠消息投递序列号 | `replBridgeTransport.ts` |
| **L856** | ML 推理 | 无 thinking blocks 检测 | `tokenEstimation.ts` |
| **L857** | ML 推理 | 无上下文窗口精算 | `autoCompact.ts` |
| **L861** | 形式化验证 | 无 TLA+/Coq 规范 | fail-closed 设计模式 |
| **L862** | 形式化验证 | 无解析资源上限 | 50ms/50000节点 |
| **L866** | 图数据库 | 无跨会话知识同步 | teamMemorySync |
| **L868** | 实时流处理 | 无 SSE 帧增量解析 | `SSETransport.ts` |
| **L869** | 实时流处理 | 无 WebSocket 双向消息 | `SessionsWebSocket.ts` |
| **L850** | 分布式 | 无 coordinator-worker 编排 | `coordinatorMode.ts` |
| **L851** | 分布式 | 无分布式策略一致性 | `policyLimits.ts` ETag 缓存 |

### P2 进阶（13 项）

| 编号 | 维度 | 描述 | 对标 |
|------|------|------|------|
| **L843** | 编译器前端 | 无 vim 状态机 | `transitions.ts` 11状态 |
| **L844** | 编译器前端 | 无 sed/DSL 解析器 | `sedEditParser.ts` BRE→ERE |
| **L848** | OS 内核 | 无 io_uring/epoll 直接调用 | 依赖 Bun libuv |
| **L849** | OS 内核 | 无跨平台 shell 函数生成 | `createArgv0ShellFunction` |
| **L853** | 分布式 | 无 Raft/Paxos/Gossip | coordinator 模式 |
| **L854** | 分布式 | 无分布式锁 | 无跨进程互斥 |
| **L858** | ML 推理 | 无本地推理引擎 | 无 ONNX/GGUF |
| **L859** | ML 推理 | 无语音 STT 路由 | `voiceStreamSTT.ts` |
| **L863** | 形式化验证 | 无属性测试 | fast-closed 设计 |
| **L864** | 图数据库 | 无知识图谱构建 | 无 RDF/Neo4j |
| **L865** | 图数据库 | 无向量检索索引 | 无 FAISS/Annoy |
| **L867** | 图数据库 | 无图谱遍历算法 | 无 PageRank |
| **L870-873** | 实时流处理 | 无背压/CQRS/窗口/语音流 | 多维度缺失 |

### 总计

- 新增 gap：**35 项**（P0 × 8 + P1 × 14 + P2 × 13）
- 累计 gap：L1–L873（共 873 个）
- claude-code 独特优势维度：**编译器前端**（工业级 bash lexer/parser）、**网络安全**（fail-closed AST walker）、**网络协议栈**（WebSocket + CONNECT 隧道 + mTLS + SSE 全覆盖）
- claude-code 薄弱维度：**分布式共识**（无经典协议）、**ML 推理**（无本地引擎）、**形式化验证**（无 TLA+/Coq 但有工程化 fail-closed）

---

## 附录：关键文件路径索引

| 维度 | 文件路径 | 行数 | 核心机制 |
|------|---------|------|---------|
| 网络协议 | `src/remote/SessionsWebSocket.ts` | 404 | WebSocket 状态机 + reconnect + ping/pong |
| 网络协议 | `src/upstreamproxy/relay.ts` | 456 | CONNECT-over-WS + protobuf 编码 |
| 网络协议 | `src/utils/proxy.ts` | 427 | mTLS + NO_PROXY + 连接池毒化 |
| 网络协议 | `src/utils/mtls.ts` | 182 | mTLS 配置 + undici Agent |
| 网络协议 | `src/services/api/withRetry.ts` | 823 | 指数退避 + 529/429 分类 |
| 网络协议 | `src/cli/transports/SSETransport.ts` | ~300 | SSE 帧增量解析 |
| 编译器前端 | `src/utils/bash/bashParser.ts` | 4436 | 纯 TS bash Lexer + Parser |
| 编译器前端 | `src/utils/bash/ast.ts` | 2679 | AST 安全 Walker (fail-closed) |
| 编译器前端 | `src/utils/bash/parser.ts` | 231 | AST 遍历 + 命令提取 |
| 编译器前端 | `src/vim/transitions.ts` | 491 | Vim 11状态机 |
| 编译器前端 | `src/tools/BashTool/sedEditParser.ts` | 323 | sed BRE→ERE 转换 |
| OS 内核 | `src/utils/sandbox/sandbox-adapter.ts` | ~500 | 沙箱运行时适配 |
| OS 内核 | `src/utils/bash/ShellSnapshot.ts` | 582 | Shell 快照 + ARGV0 dispatch |
| OS 内核 | `src/tools/BashTool/commandSemantics.ts` | ~120 | 退出码语义解释 |
| 分布式 | `src/coordinator/coordinatorMode.ts` | ~200 | Coordinator-Worker 编排 |
| 分布式 | `src/services/policyLimits/index.ts` | ~300 | ETag 缓存 + 后台轮询 |
| 分布式 | `src/bridge/replBridgeTransport.ts` | ~300 | 序列号驱动可靠投递 |
| ML 推理 | `src/services/tokenEstimation.ts` | ~400 | Token 计数 + thinking blocks |
| ML 推理 | `src/services/compact/autoCompact.ts` | ~200 | 上下文窗口精算 |
| ML 推理 | `src/services/voiceStreamSTT.ts` | ~500 | 语音 STT 推理路由 |
| 形式化验证 | `src/utils/bash/ast.ts` | 2679 | fail-closed 白名单 |
| 形式化验证 | `src/utils/bash/parser.ts` | 231 | PARSE_ABORTED 哨兵 |
| 实时流处理 | `src/cli/transports/SSETransport.ts` | ~300 | SSE 增量解析 |
| 实时流处理 | `src/services/voiceStreamSTT.ts` | ~500 | 语音实时流协议 |
| 实时流处理 | `src/services/api/withRetry.ts` | 823 | AsyncGenerator 心跳流 |
