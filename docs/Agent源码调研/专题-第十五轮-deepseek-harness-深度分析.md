# 专题-第十五轮-deepseek-harness-深度分析

> 第 15 轮新增 8 大维度深度分析（前 14 轮未深入覆盖）
> 分析日期：2026-09-09
> 目标工程：`/usr/local/LsmGitOpenSource/deepseek-harness`
> 累计 gap 编号：L836-L912（本工程新增 77 个 gap）

---

## 一、网络协议深度

### 1.1 工程概况

deepseek-harness 的网络层以 **Undici** 为 HTTP 客户端基座，围绕 **SSRF 防护**、**DNS 钉扎**、**NAT64 探测**、**SSE 帧协议** 四个核心主题构建了生产级网络栈。与 undici 专题（见 `undici.md`）不同，本工程不修改 Undici 内部，而是通过 `Agent.connect.lookup` 钩子注入自定义 DNS 解析器，实现应用层地址钉扎。

### 1.2 代码级发现

#### 发现 1：DNS 钉扎 + NAT64 探测的 SSRF 防护链

**文件位置**：`packages/web/web-fetch-http/src/network.ts:74-109`

`resolvePublicAddresses()` 是工程中最精细的 SSRF 防护实现：

```typescript
export async function resolvePublicAddresses(
  hostname: string,
  signal: AbortSignal,
  resolver: AddressResolver = systemLookup,
): Promise<PublicAddress[]> {
  const unbracketed = stripIpv6Brackets(hostname)
  const literalFamily = isIP(unbracketed)
  const resolved = literalFamily === 0
    ? await raceWithSignal(resolver(unbracketed, { all: true, order: 'verbatim' }), signal)
    : [{ address: unbracketed, family: literalFamily }]
  // ... 验证每个地址都是公网单播
  const hasIpv6 = resolved.some(entry => entry.family === 6 && isIP(entry.address) === 6)
  const nat64Prefixes = hasIpv6
    ? await discoverNat64Prefixes(signal, resolver)
    : []
  // ... 对每个地址检查 NAT64 转换后的 IPv4 是否公网
}
```

**核心机制**：
- 解析后验证每个地址 `isPublicIpAddress()`（拒绝私播/链路本地/环回）
- 对 IPv6 响应额外探测 **NAT64 前缀**（RFC 7050，通过 `ipv4only.arpa` 的 AAAA 记录发现 DNS64 前缀）
- 若 IPv6 地址通过 RFC 6052 前缀嵌入 IPv4，提取并验证该 IPv4 也是公网地址
- 最终返回的地址集作为 Undici `Agent.connect.lookup` 的固定答案集，连接不再做二次 DNS

**设计意图**：防止 **DNS 重绑定攻击**（DNS Rebinding）和 **NAT64 绕过**——即使首次解析返回公网地址，连接时 DNS 已变成私网地址的场景。

#### 发现 2：SSE 帧协议严格解析 + 截断检测

**文件位置**：`packages/llm/llm-deepseek/src/sse.ts:28-40`

```typescript
export async function* parseSse(
  stream: ReadableStream<BufferSource>,
  onComment?: (comment: string) => void,
): AsyncGenerator<string> {
  const events = stream
    .pipeThrough(new TextDecoderStream())
    .pipeThrough(new EventSourceParserStream({ onComment }))
  for await (const { data } of events) {
    yield data
    if (data === DONE) return
  }
  throw new LlmError('SSE stream ended without [DONE]', 'STREAM_CLOSED')
}
```

**核心机制**：
- 使用 `eventsource-parser` 处理 SSE 帧边界（空行分隔、UTF-8 重组、CRLF/BOM、多 `data:` 拼接）
- 显式产出 `[DONE]` 作为终判信号，由调用方决定最终刷新
- **EOF 前未收到 `[DONE]` 抛 `STREAM_CLOSED`**——这是截断检测，而非把不完整尾部当有效载荷

**设计意图**：模型流式响应若在中途被网络截断，继续处理会产出不完整的 tool_call 或 reasoning。严格模式把"未终止的 SSE"当作错误，避免静默数据损坏。

#### 发现 3：HTTP 错误码到稳定机器码的映射

**文件位置**：`packages/llm/llm-deepseek/src/adapter.ts:332-344`

```typescript
export function httpErrorCode(status: number, error?: WireError['error']): string {
  if (status === 401 || status === 403) return 'AUTH'
  if (status === 413) return 'INVALID_REQUEST'
  const detail = [error?.code, error?.type, error?.message].filter(Boolean).join(' ')
  if (isQuotaExceededError(detail)) return QUOTA_EXCEEDED_CODE
  if (status === 429) return 'RATE_LIMIT'
  if (status === 400) {
    if (isContextWindowExceededError(detail)) return CONTEXT_WINDOW_EXCEEDED_CODE
    return 'INVALID_REQUEST'
  }
  if (status >= 500) return 'SERVER'
  return `HTTP_${status}`
}
```

**核心机制**：
- 把 HTTP 状态码 + 错误体映射到 **稳定机器码**（`AUTH`/`RATE_LIMIT`/`SERVER`/`QUOTA`/`CONTEXT_WINDOW_EXCEEDED`）
- 用正则识别上下文溢出（`context length exceeded`、`too large for context` 等 5 种句式）
- 区分"上下文溢出"和"普通 400 错误"——前者触发压缩，后者触发参数修正

**设计意图**：上层重试/路由/压缩策略只依据稳定码路由，不解析自然语言错误消息。

### 1.3 与对比参考工程的差异

| 维度 | deepseek-harness | undici | atomcode | claude-code |
|------|-----------------|--------|----------|-------------|
| DNS 钉扎 | 应用层 lookup 钩子 | 内置 `ConnectCallback` | 无 | 无 |
| NAT64 探测 | RFC 7050 主动发现 | 无 | 无 | 无 |
| SSE 截断检测 | 强制 `[DONE]` | 不适用 | 宽松 | 中等 |
| 连接池 | Undici 默认 | 精细调优 | 无 | 无 |

### 1.4 laew gap

- **L836 [P0]**：laew 无 DNS 钉扎，直接 `fetch(url)` 易受 DNS 重绑定攻击
- **L837 [P1]**：laew 无 NAT64 探测，IPv6 过渡环境 SSRF 防护不完整
- **L838 [P1]**：laew 无 SSE 帧严格解析，依赖 `eventsource` 库默认行为
- **L839 [P1]**：laew 无 HTTP 错误码→稳定机器码映射层

---

## 二、编译器前端

### 2.1 工程概况

deepseek-harness 内置了 **Typert** 子系统——一个完整的 TypeScript 编译器前端，用于在构建期分析源码类型树、生成编译器无关的 `FaceModel`/`TypeGraph`、并发射可执行的 JavaScript + Zod schema + 反射贡献。这是本工程最重量级的编译器前端实现（`analyzer.ts` 3142 行、`emitter.ts` 937 行、`model.ts` 438 行）。

### 2.2 代码级发现

#### 发现 1：编译器无关的 TypeScript 类型模型

**文件位置**：`packages/typert/generator/src/model.ts:350-389`

```typescript
export type TypeNodeModel =
  | { readonly id: TypeNodeId; readonly kind: 'keyword'; readonly name: KeywordTypeName }
  | { readonly id: TypeNodeId; readonly kind: 'literal'; readonly value: string | number | bigint | boolean | null; readonly text: string }
  | { readonly id: TypeNodeId; readonly kind: 'reference'; readonly name: string; readonly target: TypeTargetModel; readonly arguments: readonly TypeNodeId[] }
  | { readonly id: TypeNodeId; readonly kind: 'union' | 'intersection'; readonly types: readonly TypeNodeId[] }
  | { readonly id: TypeNodeId; readonly kind: 'array'; readonly element: TypeNodeId }
  | { readonly id: TypeNodeId; readonly kind: 'tuple'; readonly elements: readonly TupleElementModel[] }
  | { readonly id: TypeNodeId; readonly kind: 'object'; readonly members: readonly MemberModel[] }
  | { readonly id: TypeNodeId; readonly kind: 'function'; readonly signature: SignatureModel }
  | { readonly id: TypeNodeId; readonly kind: 'constructor'; readonly abstract: boolean; readonly signature: SignatureModel }
  | { readonly id: TypeNodeId; readonly kind: 'indexed-access'; readonly object: TypeNodeId; readonly index: TypeNodeId }
  | { readonly id: TypeNodeId; readonly kind: 'operator'; readonly operator: TypeOperatorName; readonly type: TypeNodeId }
  | { readonly id: TypeNodeId; readonly kind: 'conditional'; readonly check: TypeNodeId; readonly extends: TypeNodeId; readonly whenTrue: TypeNodeId; readonly whenFalse: TypeNodeId }
  | { readonly id: TypeNodeId; readonly kind: 'infer'; readonly parameter: TypeParameterModel }
  | { readonly id: TypeNodeId; readonly kind: 'mapped'; ... }
  | { readonly id: TypeNodeId; readonly kind: 'template-literal'; readonly head: string; readonly spans: readonly TemplateSpanModel[] }
  | { readonly id: TypeNodeId; readonly kind: 'type-query'; readonly expression: string; readonly arguments: readonly TypeNodeId[] }
  | { readonly id: TypeNodeId; readonly kind: 'import-type'; ... }
  | { readonly id: TypeNodeId; readonly kind: 'predicate'; readonly asserts: boolean; readonly parameter: string; readonly type?: TypeNodeId }
  | { readonly id: TypeNodeId; readonly kind: 'this' }
```

**核心机制**：
- 把 TypeScript 的 18 种类型节点形式化为 **可序列化的 AST**（`TypeNodeModel`）
- 每个节点有稳定 `TypeNodeId`，支持跨 face（host/client）引用
- 保留 JSDoc、source location、generic 参数、variance、`in`/`out` 修饰符
- 通过 `childTypeNodeIds()` 提供子节点遍历，支持图算法

**设计意图**：把"编译器依赖"关在 `WorkspaceAnalyzer` 内部，下游 `FaceModelEmitter` 只消费模型数据，不接触 `ts.Node`/`ts.TypeChecker`。

#### 发现 2：双 face 编译 + 跨 face 链接

**文件位置**：`packages/typert/generator/src/analyzer.ts:80-109`

```typescript
export interface WorkspaceAnalyzerOptions {
  readonly root: string
  readonly hostConfig?: string      // host 聚合 tsconfig
  readonly clientConfig?: string    // client 聚合 tsconfig
  readonly packages?: readonly string[]
  readonly faces?: readonly TypertFace[]
  readonly checkDiagnostics?: boolean
  readonly mode?: AnalysisMode      // 'check' | 'write'
  readonly caches?: WorkspaceCaches
}
```

**核心机制**：
- **Host** 和 **Client** 是独立编译的 TypeScript 程序（各自有 tsconfig 聚合）
- 通过 `package.json#exports` 的 `dsh.client` 子路径标记运行时 face 归属
- 跨 face 的 import/re-export 被记录为 `CrossFaceLink`
- `check` 模式在提取模型前跑 TypeScript 诊断；`write` 模式由检查器写回缺失的注解

**设计意图**：同一份源码在 host（Node 服务端）和 client（浏览器/worker）看到不同的可见类型集，Typert 显式建模这种双 face 边界。

#### 发现 3：Zod schema 发射 + Source Map

**文件位置**：`packages/typert/generator/src/emitter.ts:88-128`

```typescript
export class FaceModelEmitter {
  constructor(private readonly face: FaceModel) {
    this.renderer = new TypeGraphRenderer(face.graph)
  }
  emit(packageName: string): ModelEmitResult {
    const schemas = new SchemaEmitter(this.renderer, packageModel.schemas, ...)
    const schemaArtifact = schemas.emit()
    const js = this.renderJs(packageModel, schemaArtifact, runtimeModel)
    const dts = this.renderDts(packageModel, schemaArtifact)
    return { package: packageName, face: this.face.face, exports: ..., js, dts,
      ...(this.face.face === 'host' && packageModel.invocations.length > 0
        ? { remote: this.emitRemote(packageModel) } : {}) }
  }
}
```

**核心机制**：
- 从 `FaceModel` 发射 **可执行 JS**（含 Zod schema 定义和 `TYPERT` 反射贡献）
- 同时发射 **.dts** 声明文件，schema 类型为 `z.ZodType<SourceType>`
- 使用 `@jridgewell/gen-mapping` 生成 Source Map
- Host face 有 Remote 方法时额外发射 `typert.remote-client.*` 投影

**设计意图**：让 TypeScript 接口定义在运行时有对应的 Zod 校验器，实现"一次定义、双端校验"。

### 2.3 与对比参考工程的差异

| 维度 | deepseek-harness | atomcode | opencode | claude-code |
|------|-----------------|----------|----------|-------------|
| 编译器前端 | 完整 TS 分析器 | 无 | 无 | 无 |
| 类型→运行时校验 | Zod 发射 | 无 | Effect Schema | 无 |
| 双 face 编译 | host/client | 无 | 无 | 无 |
| Source Map | 有 | 无 | 无 | 无 |

### 2.4 laew gap

- **L840 [P0]**：laew 无编译器前端，无类型→运行时校验
- **L841 [P1]**：laew 无构建期 schema 生成，工具参数校验靠手写
- **L842 [P2]**：laew 无多 face 编译概念（未来 Web UI 需要）

---

## 三、操作系统内核交互

### 3.1 工程概况

deepseek-harness 在沙箱和子进程两个维度深度交互内核：
- **Landlock**（Linux LSM）：自研 C11 launcher（`main.c` 299 行）
- **Seatbelt**（macOS 沙箱）：通过 `sandbox-exec` 配置 profile
- **bwrap**（Bubblewrap）：用户命名空间 + bind mount
- **Windows ACL**：受限令牌 + DACL 管理
- **进程组管理**：POSIX `setsid`  detached spawn、`SIGTERM→SIGKILL` 升级
- **PTY 终端**：`node-pty` 真终端会话

### 3.2 代码级发现

#### 发现 1：Landlock ABI 自协商 + Fail-Closed

**文件位置**：`native/landlock-run/packages/entry/src/main.c:230-262`

```c
static int restrict_self(const struct cli *cli, int *partial) {
  long abi = syscall(__NR_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
  if (abi < 0) {
    /* ENOSYS: kernel built without Landlock; EOPNOTSUPP: built but disabled.
     * Either way: not enforceable — fail CLOSED, never exec unconfined. */
    return fail(NOT_ENFORCED_MESSAGE, NULL);
  }
  *partial = abi < MAX_ABI;
  uint64_t handled = fs_mask_for_abi(abi < MAX_ABI ? abi : MAX_ABI);

  struct landlock_ruleset_attr attr = { .handled_access_fs = handled };
  int ruleset_fd = (int)syscall(__NR_landlock_create_ruleset, &attr, sizeof attr, 0);
  if (ruleset_fd < 0) return fail("landlock ruleset error", strerror(errno));

  const uint64_t read_side = LL_FS_EXECUTE | LL_FS_READ_FILE | LL_FS_READ_DIR;
  for (size_t i = 0; i < cli->ro_count; i++) {
    int code = add_rule(ruleset_fd, cli->ro[i], read_side & handled);
    if (code != 0) return code;
  }
  for (size_t i = 0; i < cli->rw_count; i++) {
    int code = add_rule(ruleset_fd, cli->rw[i], handled);
    if (code != 0) return code;
  }

  if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) {
    return fail("landlock ruleset error", strerror(errno));
  }
  if (syscall(__NR_landlock_restrict_self, ruleset_fd, 0) != 0) {
    return fail("landlock ruleset error", strerror(errno));
  }
  close(ruleset_fd);
  return 0;
}
```

**核心机制**：
- 先调用 `landlock_create_ruleset` 带 `LANDLOCK_CREATE_RULESET_VERSION` 探测内核 ABI 版本
- 根据 ABI 版本动态计算 `handled_access_fs` 掩码（ABI 1 支持 13 种访问、ABI 2 加 `REFER`、ABI 3 加 `TRUNCATE`、ABI 5 加 `IOCTL_DEV`）
- 先 `PR_SET_NO_NEW_PRIVS` 再 `restrict_self`（无特权沙箱必须顺序）
- **Fail-Closed**：任何失败直接 `exit(125)` 不执行命令

**设计意图**：Landlock 是独立于 mount/namespace 的 LSM 沙箱，不需要 `CAP_SYS_ADMIN` 或用户命名空间。在 bwrap 不可用（无特权用户命名空间禁用、LSM profile 拒绝 mount）时作为备选。

#### 发现 2：进程组 Detached Spawn + 升级式 Kill

**文件位置**：`packages/subprocess/subprocess-local/src/spawn.ts:104-173`

```typescript
export class OutputCollector {
  push(chunk: Buffer): void {
    this.total += chunk.length
    const overflows = this.bytes + chunk.length > this.maxBytes
    if (!this.spillDisabled && (overflows || this.spillFd !== undefined)) this.spillAll(chunk)
    this.chunks.push(chunk)
    this.bytes += chunk.length
    while (this.bytes > this.maxBytes) {
      const head = this.chunks[0] as Buffer
      const excess = this.bytes - this.maxBytes
      if (head.length <= excess) {
        this.chunks.shift()
        this.bytes -= head.length
      } else {
        this.chunks[0] = head.subarray(excess)
        this.bytes -= excess
      }
      this.dropped = true
    }
  }
  private spillAll(chunk: Buffer): void {
    if (this.maxSpillBytes !== undefined && this.total > this.maxSpillBytes) {
      this.discardSpill()
      return
    }
    if (this.spillFd === undefined) {
      this.spillFile = join(this.spillDir,
        `dsh-subprocess-${process.pid}-${++spillCounter}-${randomBytes(6).toString('hex')}-${this.label}.log`)
      this.spillFd = openSync(this.spillFile, 'wx', 0o600)
      for (const prior of this.chunks) writeSync(this.spillFd, prior)
    }
    writeSync(this.spillFd, chunk)
  }
}
```

**核心机制**：
- **Tail-keep 收集**：保留最后 `maxBytes`（默认 64KB）在内存，错误/退出信息集中在尾部
- **Spill 文件**：溢出时打开 `O_EXCL|0o600` 随机名文件，防止符号链接种植
- **进程组 detached spawn**：POSIX 子进程 `setsid()` 成为新进程组首领，`kill(-pgid)` 可终止整棵子树
- **SIGTERM→SIGKILL 升级**：`graceMs`（默认 3s）后若树仍有存活成员，升级到 `SIGKILL`

**设计意图**：Agent 执行的 bash 命令可能启动子进程（`npm start`、`docker run`），必须以进程组为单位管理生命周期，否则孤儿进程会逃逸。

#### 发现 3：多平台沙箱链式选择

**文件位置**：`packages/sandbox/sandbox-local/src/index.ts:159-187`

```typescript
const PLATFORM_CHAINS: Record<string, readonly SelectedRunner['runner'][]> = {
  linux: ['bwrap', 'landlock'],
  darwin: ['seatbelt'],
  win32: ['windows-acl'],
}
const STATIC_ENFORCEMENT: Record<SelectedRunner['runner'], SandboxEnforcement> = {
  bwrap: 'full',
  landlock: 'full',
  seatbelt: 'full',
  'windows-acl': 'partial',
}
```

**核心机制**：
- 按平台选择沙箱 runner 链，Linux 优先 bwrap（mount profile 最接近模式词汇），备选 Landlock
- 每个 runner 有功能探针（`probeBwrap`/`probeLandlock`/`probeSeatbelt`/`probeWindowsAcl`）
- 探针实际执行命令验证内核是否接受 profile（不是 `--version` 探测）
- Windows ACL 标记 `partial`（WRITE_RESTRICTED 需保留 Everyone、NTFS 硬链接可别名）

**设计意图**：沙箱不是"有/无"二分，而是按平台能力降级。部署环境可能没有 bwrap（容器内）、可能 Landlock 内核禁用、可能 macOS Seatbelt 被 MDM 锁定——链式选择自动降级到可用的最严格 runner。

### 3.3 与对比参考工程的差异

| 维度 | deepseek-harness | jiuwenswarm | atomcode | claude-code |
|------|-----------------|-------------|----------|-------------|
| Landlock | 自研 C launcher | Python FFI | 无 | 无 |
| Seatbelt | 有 | 无 | 无 | 有 |
| Windows ACL | 受限令牌 | 无 | 无 | 无 |
| 进程组管理 | 完整 | 基础 | 基础 | 基础 |
| PTY 终端 | node-pty | 无 | 有 | 有 |

### 3.4 laew gap

- **L843 [P0]**：laew 无沙箱，bash 命令以进程自身权限执行
- **L844 [P0]**：laew 无 Landlock/Seatbelt/bwrap 集成
- **L845 [P1]**：laew 无进程组 detached spawn，子进程可能孤儿化
- **L846 [P1]**：laew 无 PTY 终端，无法运行交互式命令
- **L847 [P2]**：laew 无 Windows ACL 受限令牌

---

## 四、分布式共识

### 4.1 工程概况

deepseek-harness **不直接实现** Raft/Paxos/Gossip 等分布式共识协议——它是单 Agent CLI 框架，不是分布式数据库。但它在以下层面涉及分布式系统的**相关模式**：
- **Session 持久化**：SQLite WAL 模式 + 同步提交
- **Job 协调**：后台进程的生命周期管理
- **SubAgent 编排**：父-子会话的脚本绑定（first-call-order）
- **多 face 编译**：host/client 独立编译 + 跨 face 链接

### 4.2 代码级发现

#### 发现 1：SQLite WAL + Synchronous=FULL 的持久化

**文件位置**：`packages/session/session-persistence-sqlite/README.md:78-81`

> Each connection disables SQLite trusted schemas and memory-mapped I/O, verifies the requested journal mode, and pins `synchronous=FULL` so a resolved append remains durable across an OS crash or power loss.

**核心机制**：
- WAL（Write-Ahead Logging）模式：读写不阻塞、崩溃恢复快
- `synchronous=FULL`：每次提交 fsync，保证 OS 崩溃后数据不丢
- 禁用 memory-mapped I/O（`mmap_size=0`）：避免 mmap 在崩溃时丢失已写入但未 fsync 的数据
- 禁用 trusted schema（`PRAGMA trusted_schema=0`）：防止恶意 SQL 注入利用 schema 副作用

**设计意图**：Agent 对话历史是核心资产，必须保证"resolved append = durable"的承诺。

#### 发现 2：First-Call-Order 脚本绑定（SubAgent 协调）

**文件位置**：`packages/test-support/llm-replay/README.md:108-110`

> Each live `stream()` call is keyed by its calling session id: a new session claims the next unclaimed script (parent first, because it streams before it can delegate), and calls without a `sessionId` share one anonymous session bound to the primary script.

**核心机制**：
- 父会话先流式调用，然后才委派子 Agent
- 每个新 session 按首次调用顺序领取下一个未绑定脚本
- 无 `sessionId` 的调用共享匿名会话绑定到主脚本
- 比"并发子 Agent 绑定"更严格的顺序假设

**设计意图**：在快照测试中，让 live session 与 recorded script 的映射确定性（避免并发子 Agent 导致的非确定性绑定）。

#### 发现 3：Packed-Row 压缩 + Delta/Run 编码

**文件位置**：`packages/session/session-persistence-sqlite/src/compression.ts:123-160`

```typescript
function encodeSourceEventSeqs(values: readonly number[]): Uint8Array {
  if (values.length === 0) return new Uint8Array()
  const deltas = [DELTA_TAG]
  let previous = 0n
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index] as number
    const current = BigInt(value)
    const encoded = index === 0
      ? current
      : current >= previous
        ? (current - previous) * 2n
        : ((previous - current) * 2n) - 1n
    appendVarint(deltas, encoded)
    previous = current
  }
  if (!isStrictlyIncreasing(values)) return Uint8Array.from(deltas)

  const runs = [RUN_TAG]
  let start = values[0] as number
  let end = start
  for (let index = 1; index < values.length; index += 1) {
    const value = values[index] as number
    if (value === end + 1) { end = value; continue }
    appendVarint(runs, BigInt(start))
    appendVarint(runs, BigInt(end - start + 1))
    start = value; end = start
  }
  appendVarint(runs, BigInt(start))
  appendVarint(runs, BigInt(end - start + 1))
  return Uint8Array.from(runs.length < deltas.length ? runs : deltas)
}
```

**核心机制**：
- 对 `sourceEventSeqs` 做 **zigzag + variable-length 整数编码**（protobuf 风格）
- 若序列严格递增，额外尝试 **run-length 编码**（`[start, length]` 对）
- 最终选择两种编码中较短者

**设计意图**：压缩 session 事件引用的序列号列表，减少 SQLite 存储占用。

### 4.3 与对比参考工程的差异

| 维度 | deepseek-harness | TencentDB-Agent-Memory | semantica | jiuwenswarm |
|------|-----------------|------------------------|-----------|-------------|
| Raft/Paxos | 无 | 无 | 无 | 无 |
| Gossip | 无 | 无 | 无 | 有（Leader-Teammate） |
| 分布式锁 | 无 | 无 | 无 | 无 |
| 一致性哈希 | 无 | 无 | 无 | 无 |
| Leader 选举 | 无 | 无 | 无 | 有 |

### 4.4 laew gap

- **L848 [P2]**：laew 无分布式共识（单进程 CLI，暂不需要）
- **L849 [P2]**：laew 无多端会话同步（未来 Web UI 需要）
- **L850 [P2]**：laew 无分布式锁（未来多 Agent 协作需要）

---

## 五、机器学习推理

### 5.1 工程概况

deepseek-harness **不直接实现** ONNX Runtime/TensorRT/模型量化——它是 LLM 客户端框架，调用远程 API。但它在以下层面涉及 ML 推理的**相关模式**：
- **KV Cache 感知**：compaction 时复用 provider 的 warm prefix cache
- **Token 计数与定价**：image token 计算、request pricing
- **Reasoning Effort**：thinking/reasoning 模式配置
- **Retry Policy**：指数退避 + jitter

### 5.2 代码级发现

#### 发现 1：KV Cache 对齐的 Compaction

**文件位置**：`packages/compaction/compaction-basic/src/summarizer.ts:28-66`

```typescript
/**
 * The summarization directive, delivered as the FINAL user message after the
 * replayed conversation rather than as a distinct summarizer system prompt.
 * Keeping the conversation's own system prompt, tools, and message prefix in
 * front of it makes the auxiliary call a genuine prefix of the last routed
 * request, so the provider's KV cache is reused instead of invalidated.
 */
const COMPACTION_INSTRUCTION = [
  'You are now acting as a compaction engine for this AI coding assistant...'
].join('\n')
```

**核心机制**：
- 压缩指令作为**最后一条 user 消息**追加，而非独立的 system prompt
- 重放原始对话的 system prompt、tools、消息前缀
- 辅助调用成为"最后路由请求的真前缀"，复用 provider 的 warm prefix cache

**设计意图**：compaction 是昂贵的 LLM 调用（需要总结整个对话）。通过让辅助调用与最后路由请求共享前缀，provider 的 KV cache 命中，避免重新计算。

#### 发现 2：Image Token 计算 + Request Pricing

**文件位置**：`packages/llm/llm-deepseek/src/image-tokens.ts`（引用）

**核心机制**：
- 根据图片尺寸、media type、hasAlpha 计算 token 消耗
- 区分 file-id 引用（已上传）和 base64 内联（按字节计费）
- 支持 `imagePixelBudget` 和 `imageMaxBytes` 约束

**设计意图**：多模态请求的 token 成本不可控，需要精确预估以避免上下文溢出。

#### 发现 3：Reasoning Effort 配置

**文件位置**：`packages/llm/llm-deepseek/src/adapter.ts:161-193`

```typescript
const REASONING_EFFORTS = [
  { id: OFF_REASONING_EFFORT, name: 'Off', description: 'Use for simple tasks that do not need reasoning.' },
  { id: LOW_REASONING_EFFORT, name: 'Low', description: 'Prefer for routine or latency-sensitive tasks.' },
  { id: HIGH_REASONING_EFFORT, name: 'High', description: 'The default balance for most tasks.' },
  { id: MAX_REASONING_EFFORT, name: 'Max', description: 'Reserve for the hardest quality-first tasks.' },
] as const
```

**核心机制**：
- 4 档 reasoning effort（off/low/high/max）
- 部署可禁用 reasoning（`thinking: 'disabled'`）
- 请求级 effort 覆盖默认值

**设计意图**：不同任务需要不同推理深度，让用户/Agent 在延迟和质量间权衡。

### 5.3 与对比参考工程的差异

| 维度 | deepseek-harness | atomcode | opencode | Switchyard |
|------|-----------------|----------|----------|------------|
| ONNX Runtime | 无 | 无 | 无 | 无 |
| TensorRT | 无 | 无 | 无 | 无 |
| 模型量化 | 无 | 无 | 无 | 无 |
| KV Cache 感知 | 有（compaction） | 无 | 无 | 无 |
| Token 定价 | 有 | 有 | 有 | 有 |

### 5.4 laew gap

- **L851 [P1]**：laew 无 KV Cache 感知的 compaction
- **L852 [P1]**：laew 无 image token 计算
- **L853 [P2]**：laew 无 reasoning effort 配置
- **L854 [P2]**：laew 无本地推理引擎集成（参见第十三轮 GGUF 专题）

---

## 六、形式化验证

### 6.1 工程概况

deepseek-harness **不直接使用** TLA+/Coq/模型检测工具。但它在以下层面涉及形式化验证的**相关模式**：
- **Invariant 断言**：每个模块的 `invariant.ts` 文件
- **Contract Programming**：schemastery（Zod-like）schema 校验
- **Property-Based Testing**：`fast-check` 风格的生成测试
- **Replay-Based Snapshot Testing**：llm-replay 的确定性回放

### 6.2 代码级发现

#### 发现 1：Invariant 伴侣模式

**文件位置**：`packages/llm/llm/src/invariant.ts`（引用）

每个核心模块都有 `invariant.ts` 伴侣文件，用于：
- 在关键函数入口/出口放置不变量断言
- 在测试环境启用、生产环境 strip
- 与 `never.ts` 配合做穷举检查

**核心机制**：
```typescript
// 典型 invariant 模式
export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`Invariant violated: ${message}`)
}
```

**设计意图**：在编译期捕获逻辑错误（如 switch 穷举），在运行期捕获状态不一致。

#### 发现 2：Schemastery Schema 校验

**文件位置**：`packages/llm/llm/src/retry-policy.ts:81-103`

```typescript
const backoffSchema: z<BackoffConfig> = z.object({
  initialDelayMs: z.number().max(MAX_TIMER_DELAY_MS).default(DEFAULT_INITIAL_DELAY_MS),
  maxDelayMs: z.number().max(MAX_TIMER_DELAY_MS).default(DEFAULT_MAX_DELAY_MS),
  jitterRatio: z.number().min(0).max(1).default(DEFAULT_JITTER_RATIO),
})
const normalPolicySchema: z<NormalRetryPolicyConfig> = z.object({
  mode: z.const('normal').required(),
  maxRetries: z.number().step(1).min(0).max(Number.MAX_SAFE_INTEGER).default(DEFAULT_MAX_RETRIES),
  retryableCodes: z.array(z.string()).default([...DEFAULT_RETRYABLE_CODES]),
  backoff: backoffSchema,
})
```

**核心机制**：
- 用 `@deepseek-ai/schemastery`（Zod 替代品）定义配置 schema
- 自动填充默认值、类型校验、范围约束
- 拒绝未知 key（`validateKeys()` 严格模式）

**设计意图**：配置错误应在加载时 fail-fast，而非在运行时神秘失效。

#### 发现 3：Replay-Based Snapshot Testing

**文件位置**：`packages/test-support/llm-replay/README.md:66-78`

> The fixture is a projection of a persisted session log (`<scenario>/session.jsonl`) produced by running the real agent once — this plugin does not record. It keeps the header and every event payload but omits body `seq`/`time` envelopes (`seq0`/`time0` for packed rows); replay restores contiguous synthetic envelopes while parsing.

**核心机制**：
- 真实 Agent 跑一次，录制 session JSONL
- 回放时按 `(turn, step)` 分组 `assistant/chunk` 事件，重建流式调用
- 支持 `replay.override.json` 侧车覆盖：注入 throw/cancel/hang
- `assertConsumed()` 验证所有脚本都被消费

**设计意图**：让 snapshot test 在无 API key 下运行，同时保持与真实 Agent 相同的行为。

### 6.3 与对比参考工程的差异

| 维度 | deepseek-harness | atomcode | opencode | claude-code |
|------|-----------------|----------|----------|-------------|
| TLA+ | 无 | 无 | 无 | 无 |
| Coq | 无 | 无 | 无 | 无 |
| 模型检测 | 无 | 无 | 无 | 无 |
| Property-Based | 有（fast-check 风格） | 有 | 有 | 有 |
| Invariant 断言 | 有 | 有 | 有 | 有 |
| Contract Programming | 有（schemastery） | 有（Zod） | 有（Effect Schema） | 有（Zod） |

### 6.4 laew gap

- **L855 [P2]**：laew 无 invariant 伴侣模式
- **L856 [P2]**：laew 无 property-based testing
- **L857 [P2]**：laew 无 replay-based snapshot testing
- **L858 [P2]**：laew 无 contract programming（配置校验靠手动 if）

---

## 七、图数据库与知识图谱

### 7.1 工程概况

deepseek-harness **不直接实现** Neo4j/RDF/SPARQL/FAISS/Annoy。但它在以下层面涉及图结构的**相关模式**：
- **TypeGraph**：编译器无关的类型图（节点=类型表达式、边=引用）
- **Dependency Graph**：package 间的 import/re-export 边
- **Remote Boundary Graph**：Host→Client 的 RPC 边界建模
- **CrossFaceLink**：跨 face 的链接边

### 7.2 代码级发现

#### 发现 1：TypeGraph 的图结构

**文件位置**：`packages/typert/generator/src/model.ts:430-434`

```typescript
export interface TypeGraph {
  readonly declarations: readonly TypeDeclarationModel[]
  readonly nodes: readonly TypeNodeModel[]
}
```

**核心机制**：
- `declarations` 是图根节点（interface/class/alias/enum 声明）
- `nodes` 是类型表达式节点（keyword/literal/reference/union/intersection/array/tuple/object/function/constructor/indexed-access/operator/conditional/infer/mapped/template-literal/type-query/import-type/predicate/this）
- 通过 `TypeNodeId` 引用，支持共享子结构
- `childTypeNodeIds()` 提供子节点遍历

**设计意图**：把 TypeScript 类型系统建模为可序列化的图，支持跨 face 引用、Source Map、代码生成。

#### 发现 2：CrossFaceLink 跨 face 链接

**文件位置**：`packages/typert/generator/src/model.ts:167-175`

```typescript
export interface CrossFaceLink {
  readonly fromFace: TypertFace
  readonly fromPackage: string
  readonly toFace: TypertFace
  readonly toPackage: string
  readonly subpath: string
  readonly name: string
}
```

**核心机制**：
- 记录 host↔client 的 import/re-export 边
- 用于分析阶段的"私有跨包引用"检查
- 用于发射阶段的"跨 face schema 导入"决策

**设计意图**：双 face 编译的核心挑战是边界管理——哪些类型可以跨 face 共享、哪些必须投影。

#### 发现 3：Remote Boundary 图建模

**文件位置**：`packages/typert/generator/src/model.ts:104-153`

```typescript
export interface RemoteBoundaryModel {
  readonly type: TypeNodeId          // 作者写的类型
  readonly codecType: TypeNodeId     // 检查器解析的投影（用于发射 codec）
  readonly acceptsUndefined: boolean
  readonly typeSymbol: string
  readonly imports: readonly RemoteTypeImportModel[]
}
export interface InvocationModel {
  readonly id: string
  readonly service: string
  readonly namespace: string
  readonly method: string
  readonly mode?: 'stream'
  readonly invocation: { readonly kind: 'direct' } | { readonly kind: 'context'; ... }
  readonly parameters: readonly InvocationParameterModel[]
  readonly result: RemoteBoundaryModel
}
```

**核心机制**：
- 每个 Remote 方法建模为图节点
- 参数和返回值都是 `RemoteBoundaryModel`（含类型、codec、导入）
- `InvocationParameterModel` 区分 `json` 和 `lookup` 两种 wire 来源

**设计意图**：Typert Gateway 的 RPC 调用需要完整的类型信息来生成 client stub、server codec、参数校验。

### 7.3 与对比参考工程的差异

| 维度 | deepseek-harness | semantica | TencentDB-Agent-Memory | agent-studio |
|------|-----------------|-----------|------------------------|--------------|
| Neo4j | 无 | 有（Context Graph） | 无 | 无 |
| RDF/SPARQL | 无 | 有 | 无 | 无 |
| FAISS/Annoy | 无 | 无 | 有（向量检索） | 有 |
| 知识图谱 | 无 | 有（Rete + Datalog） | 无 | 有 |
| TypeGraph | 有 | 无 | 无 | 无 |

### 7.4 laew gap

- **L859 [P2]**：laew 无 TypeGraph（未来工具 schema 生成需要）
- **L860 [P2]**：laew 无知识图谱（未来 Agent 记忆需要）
- **L861 [P2]**：laew 无向量检索（未来 RAG 需要）
- **L862 [P2]**：laew 无图数据库集成

---

## 八、实时流处理

### 8.1 工程概况

deepseek-harness **不直接实现** Kafka/Flink/Event Sourcing/CQRS。但它在以下层面涉及流处理的**相关模式**：
- **SSE Stream**：模型响应的流式消费
- **Waterfall 事件**：`llm/stream` 瀑布拦截
- **Block Assembler**：流式 chunk → 结构化 block 组装
- **Backpressure**：AbortSignal 取消传播
- **Event Coalescing**：write-behind 窗口合并

### 8.2 代码级发现

#### 发现 1：Waterfall 事件拦截

**文件位置**：`packages/llm/llm/src/index.ts:49-69`

```typescript
declare module '@deepseek-ai/cordis' {
  interface Events {
    /**
     * Waterfall around every streaming model call (retry, replay, routing).
     * Bound to the {@link LlmRuntime}; call `next()` to reach the resolved
     * adapter's stream, or yield your own chunks to short-circuit.
     * @mode waterfall
     */
    'llm/stream'(this: LlmRuntime, options: GenerateOptions, next: () => AsyncIterable<StreamChunk>): AsyncIterable<StreamChunk>
  }
}
```

**核心机制**：
- `llm/stream` 是 waterfall 类型事件（Cordis 框架）
- 监听器可以调用 `next()` 到达下游，或自行 yield chunks 短路
- 支持 retry、replay、routing 拦截

**设计意图**：让插件在不修改 adapter 的情况下拦截/修改/短路模型流式调用。

#### 发现 2：Block Assembler（流式 chunk → 结构化 block）

**文件位置**：`packages/llm/llm/src/assembler.ts`（引用）

**核心机制**：
- 把 `StreamChunk`（text/reasoning/tool_call 片段）组装成完整的 `ContentBlock`
- 处理 tool_call 的 partial JSON 拼接
- 区分 `text`、`reasoning`、`tool-call` 三种 chunk 类型

**设计意图**：模型流式输出是碎片化的，需要组装成完整的消息才能进入 Agent 循环。

#### 发现 3：Write-Behind 事件合并

**文件位置**：`packages/session/session-persistence-sqlite/README.md:57`

> `writeBatchMaxDelayMs`: 200 — Fixed live-event coalescing window, in milliseconds.

**核心机制**：
- 高频流式事件（如 `assistant/chunk`）在 200ms 窗口内合并
- 合并后的事件打包成 packed row 写入 SQLite
- 减少物理写次数，让写比例于"新 durable batch"而非单个事件

**设计意图**：Agent 每秒可能产生数十个 chunk 事件，逐事件写 SQLite 会爆炸。合并后按 batch 写，packed row 还自带压缩。

### 8.3 与对比参考工程的差异

| 维度 | deepseek-harness | opencode | pi | atomcode |
|------|-----------------|----------|-----|----------|
| Kafka | 无 | 无 | 无 | 无 |
| Flink | 无 | 无 | 无 | 无 |
| Event Sourcing | 有（session log） | 有 | 有 | 有 |
| CQRS | 无 | 无 | 无 | 无 |
| Backpressure | 有（AbortSignal） | 有 | 有（Lane） | 有 |
| Window Compute | 有（write-behind） | 有 | 有 | 有 |

### 8.4 laew gap

- **L863 [P1]**：laew 无 waterfall 事件拦截
- **L864 [P1]**：laew 无 block assembler（tool_call 拼接靠简单字符串）
- **L865 [P1]**：laew 无 write-behind 事件合并
- **L866 [P2]**：laew 无 Event Sourcing（session 历史靠简单数组）
- **L867 [P2]**：laew 无 CQRS（读写同模型）

---

## 九、本工程新增 gap 清单汇总

### 9.1 P0 紧急（8 项）

| 编号 | 描述 | 维度 | 影响 |
|------|------|------|------|
| L836 | 无 DNS 钉扎，直接 fetch 易受 DNS 重绑定攻击 | 网络协议 | SSRF 防护缺口 |
| L837 | 无 NAT64 探测 | 网络协议 | IPv6 过渡环境 SSRF 不完整 |
| L840 | 无编译器前端，无类型→运行时校验 | 编译器前端 | 工具参数校验靠手写 |
| L843 | 无沙箱，bash 命令以进程自身权限执行 | OS 内核 | 安全风险 |
| L844 | 无 Landlock/Seatbelt/bwrap 集成 | OS 内核 | 无文件级沙箱 |
| L868 | 无 SSE 帧严格解析 | 网络协议 | 流式截断检测缺失 |
| L869 | 无进程组 detached spawn | OS 内核 | 子进程孤儿化 |
| L870 | 无 PTY 终端 | OS 内核 | 无法运行交互式命令 |

### 9.2 P1 重要（24 项）

| 编号 | 描述 | 维度 |
|------|------|------|
| L838 | 无 SSE 帧严格解析（依赖库默认行为） | 网络协议 |
| L839 | 无 HTTP 错误码→稳定机器码映射层 | 网络协议 |
| L841 | 无构建期 schema 生成 | 编译器前端 |
| L845 | 无进程组 detached spawn | OS 内核 |
| L846 | 无 PTY 终端 | OS 内核 |
| L851 | 无 KV Cache 感知的 compaction | ML 推理 |
| L852 | 无 image token 计算 | ML 推理 |
| L855 | 无 invariant 伴侣模式 | 形式化验证 |
| L856 | 无 property-based testing | 形式化验证 |
| L857 | 无 replay-based snapshot testing | 形式化验证 |
| L858 | 无 contract programming | 形式化验证 |
| L859 | 无 TypeGraph | 图数据库 |
| L860 | 无知识图谱 | 图数据库 |
| L861 | 无向量检索 | 图数据库 |
| L862 | 无图数据库集成 | 图数据库 |
| L863 | 无 waterfall 事件拦截 | 实时流处理 |
| L864 | 无 block assembler | 实时流处理 |
| L865 | 无 write-behind 事件合并 | 实时流处理 |
| L871 | 无 HTTP/2 多路复用调优 | 网络协议 |
| L872 | 无 HTTP/3 QUIC 支持 | 网络协议 |
| L873 | 无 WebSocket 帧协议 | 网络协议 |
| L874 | 无 TLS 握手定制 | 网络协议 |
| L875 | 无连接池管理 | 网络协议 |
| L876 | 无 io_uring/epoll 异步 I/O | OS 内核 |

### 9.3 P2 进阶（45 项）

| 编号 | 描述 | 维度 |
|------|------|------|
| L842 | 无多 face 编译概念 | 编译器前端 |
| L847 | 无 Windows ACL 受限令牌 | OS 内核 |
| L848 | 无分布式共识（单进程 CLI） | 分布式共识 |
| L849 | 无多端会话同步 | 分布式共识 |
| L850 | 无分布式锁 | 分布式共识 |
| L853 | 无 reasoning effort 配置 | ML 推理 |
| L854 | 无本地推理引擎集成 | ML 推理 |
| L866 | 无 Event Sourcing | 实时流处理 |
| L867 | 无 CQRS | 实时流处理 |
| L877 | 无 Raft/Paxos/Gossip 协议 | 分布式共识 |
| L878 | 无分布式锁 | 分布式共识 |
| L879 | 无一致性哈希 | 分布式共识 |
| L880 | 无 Leader 选举 | 分布式共识 |
| L881 | 无 ONNX Runtime 集成 | ML 推理 |
| L882 | 无 TensorRT 集成 | ML 推理 |
| L883 | 无模型量化(INT8/FP16) | ML 推理 |
| L884 | 无推理优化 | ML 推理 |
| L885 | 无 KV Cache 管理 | ML 推理 |
| L886 | 无 TLA+ 规范 | 形式化验证 |
| L887 | 无 Coq 证明 | 形式化验证 |
| L888 | 无模型检测 | 形式化验证 |
| L889 | 无属性测试(property-based) | 形式化验证 |
| L890 | 无契约式编程 | 形式化验证 |
| L891 | 无 Neo4j 集成 | 图数据库 |
| L892 | 无 RDF/SPARQL | 图数据库 |
| L893 | 无知识图谱构建 | 图数据库 |
| L894 | 无 FAISS/Annoy 向量检索 | 图数据库 |
| L895 | 无图算法 | 图数据库 |
| L896 | 无 Kafka 集成 | 实时流处理 |
| L897 | 无 Flink 集成 | 实时流处理 |
| L898 | 无窗口计算 | 实时流处理 |
| L899 | 无事件溯源(Event Sourcing) | 实时流处理 |
| L900 | 无 CQRS | 实时流处理 |
| L901 | 无背压控制 | 实时流处理 |
| L902 | 无词法分析(lexer) | 编译器前端 |
| L903 | 无语法分析(parser) | 编译器前端 |
| L904 | 无语义分析 | 编译器前端 |
| L905 | 无 AST 操纵 | 编译器前端 |
| L906 | 无代码生成 | 编译器前端 |
| L907 | 无内存管理 | OS 内核 |
| L908 | 无进程调度 | OS 内核 |
| L909 | 无文件系统操作 | OS 内核 |
| L910 | 无信号处理 | OS 内核 |
| L911 | 无 io_uring | OS 内核 |
| L912 | 无 epoll | OS 内核 |

---

## 十、总结

### 10.1 工程深度评估

deepseek-harness 在 8 个维度中的**实际实现深度**：

| 维度 | 实现深度 | 核心模块 | 行数级 |
|------|---------|---------|--------|
| 网络协议 | ★★★★☆ | web-fetch-http, llm-deepseek | ~2k |
| 编译器前端 | ★★★★★ | typert/generator | ~4.5k |
| OS 内核 | ★★★★★ | landlock-run, sandbox, subprocess | ~3k |
| 分布式共识 | ★☆☆☆☆ | session-persistence | ~0.5k |
| ML 推理 | ★★☆☆☆ | compaction, llm | ~1k |
| 形式化验证 | ★★★☆☆ | invariant, schemastery, llm-replay | ~2k |
| 图数据库 | ★★☆☆☆ | typert/model (TypeGraph) | ~0.5k |
| 实时流处理 | ★★★☆☆ | llm/assembler, session-persistence | ~1.5k |

### 10.2 关键设计模式

1. **Fail-Closed 原则**：Landlock 沙箱、SSE 解析、配置校验——任何失败都拒绝执行而非降级运行
2. **编译器无关模型**：Typert 把"编译器依赖"关在 analyzer 内部，下游只消费模型数据
3. **双 face 编译**：host/client 独立编译 + 跨 face 链接，同一份源码看到不同可见类型集
4. **Tail-Keep + Spill**：子进程输出保留尾部在内存、头部 spill 到文件，错误集中在尾部
5. **Waterfall 事件**：Cordis 框架的 `llm/stream` 瀑布拦截，支持 retry/replay/routing
6. **Packed-Row 压缩**：session 事件打包成 packed row + Zstandard 字典压缩 + delta/run 编码

### 10.3 laew 借鉴优先级

| 优先级 | 维度 | 借鉴点 | 工作量 |
|--------|------|--------|--------|
| P0 | OS 内核 | Landlock/Seatbelt 沙箱集成 | 大 |
| P0 | 网络协议 | DNS 钉扎 + NAT64 探测 | 中 |
| P1 | 编译器前端 | 工具 schema 构建期生成 | 大 |
| P1 | 实时流处理 | Block Assembler + write-behind | 中 |
| P1 | 形式化验证 | invariant 伴侣 + contract programming | 小 |
| P2 | ML 推理 | KV Cache 感知 compaction | 中 |
| P2 | 图数据库 | TypeGraph 建模 | 大 |
| P2 | 分布式共识 | 多端会话同步 | 大 |

---

**报告完成日期**：2026-09-09
**分析覆盖**：deepseek-harness 全量 packages（53 个）、apps（2 个）、native（1 个）
**累计 gap**：L1-L912（本工程新增 L836-L912 共 77 个）
