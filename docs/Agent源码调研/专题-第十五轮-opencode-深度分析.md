# 第十五轮深度分析 — opencode 八大新维度系统化深挖

> 调研对象: opencode(TypeScript/Bun, Effect+Schema 全栈 DI)
> 调研日期: 2026-09-09
> 前 14 轮已覆盖: 835 个 gap(L1-L835)
> 本轮新增: 8 大维度(网络协议深度 / 编译器前端 / 操作系统内核交互 / 分布式共识 / 机器学习推理 / 形式化验证 / 图数据库与知识图谱 / 实时流处理)
> 累计 gap: L1-L915(本轮新增 80 个 gap)

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
9. [本轮新增 gap 清单汇总](#9-本轮新增-gap-清单汇总)

---

## 1. 网络协议深度

### 1.1 WebSocket 帧级 Record/Replay 系统

opencode 在 `packages/http-recorder/` 实现了一套完整的 WebSocket 录制回放框架,支持帧级别的精确断言。

**核心文件**:`packages/http-recorder/src/websocket.ts`(174 行)

**机制描述**:
- `makeWebSocketExecutor` 工厂函数创建三种模式:`record`(录制)、`replay`(回放)、`passthrough`(透传)
- 录制模式通过 `Semaphore(1)` 保证写互斥,`Ref<boolean>` 防止重复 close
- 回放模式使用 `SynchronizedRef<number>` 追踪客户端帧位置,`assertClientEvent` 逐帧校验
- 事件类型分为 `text` 和 `binary`,binary 以 base64 编码存储
- 支持 `compareClientMessagesAsJson` 选项,将 JSON 消息规范化后比较(忽略 key 顺序)

**设计意图**:
- 解决 WebSocket 测试的非确定性:通过录制真实连接、回放时精确匹配帧序列
- `claim` 机制确保每个 interaction 只被消费一次,`finalizer` 检查未用 interaction
- `openSnapshot` 在录制时捕获 URL + headers,回放时做 canonicalizeJson 比较

**代码位置**:`packages/http-recorder/src/socket.ts:119-200`

`makeRecordingSocket` 将任意 `Socket.Socket` 包装为录制态:
- `ActiveRecording` 结构维护 `events[]`、`eventLock`、`accepting`、`opened`、`valid` 五态
- `runRaw` 拦截 upstream 数据流,每个 server 消息封装为 `encodeEvent("server", message)`
- `writer` 拦截客户端写入,`writeLock.withPermit` 保证事件追加原子性
- 写入失败时 `state.valid = false`,最终 `append` 跳过无效录制

### 1.2 AWS Event Stream 二进制帧协议

**核心文件**:`packages/llm/src/protocols/bedrock-event-stream.ts`(88 行)

**机制描述**:
- Bedrock Converse 使用 AWS event stream 二进制协议:`[length:4][headers-length:4][prelude-crc:4][headers][payload][crc:4]`
- `EventStreamCodec` 处理帧边界和 CRC 校验
- `FrameBufferState` 用 `buffer` + `offset` 游标追踪,`appendChunk` 零拷贝压缩(只分配一次新 buffer)
- 解析后按 `:event-type` header 重新包装,删除 AWS 填充的 `p` 字段

**设计意图**:
- 解决流式 LLM 响应的增量解析:网络 chunk 可能跨帧,需要 buffer 累积
- `consumeFrames` 返回 `[cursor, out]`,支持一输入多输出(一个 chunk 含多个完整帧)
- `initialFrameBuffer` 作为 `Stream.mapAccumEffect` 的初始状态,纯函数式累积

### 1.3 SSE 长连接与指数退避重连

**核心文件**:`packages/opencode/src/control-plane/workspace.ts:184-438`

**机制描述**:
- `connectSSE` 发起 `text/event-stream` 请求,返回 `Stream<Uint8Array>`
- `parseSSE` 逐行解析 `data:` / `id:` / `retry:` 字段,空行触发事件发射
- `syncWorkspaceLoop` 无限循环:`connectSSE` → `syncHistory` → `parseSSE` → 失败后 `Effect.sleep(2^attempt)` 退避
- 退避上限 120 秒:`Math.min(120_000, 1_000 * 2 ** attempt)`

**设计意图**:
- Workspace 跨机器同步依赖 SSE 长连接,断连后必须自动重连
- `parseSSE` 的 `retry` 字段解析为毫秒,但 opencode 选择自己的指数退避(更可控)
- `syncHistory` 在每次重连后执行,通过 `EventSequenceTable` 的 `seq` 做增量同步

### 1.4 原子写入与 Secret 脱敏

**核心文件**:`packages/http-recorder/src/cassette.ts:104-123`

**机制描述**:
- `append` 使用 `appendLock.withPermit` 保证并发安全
- 写入流程:`makeDirectory` → `writeFileString(tmp)` → `rename(tmp, target)` → `ensuring(remove(tmp))`
- 临时文件名:`` `${target}.${crypto.randomUUID()}.tmp` ``
- `secretFindings` 扫描 7 种正则模式(Bearer token / API key / AWS key / GitHub token / private key 等)
- 环境变量名匹配 `API|AUTH|BEARER|CREDENTIAL|KEY|PASSWORD|SECRET|TOKEN`,值长度 ≥12 视为 secret

**设计意图**:
- 录制文件可能含敏感信息,写入前必须检测并拒绝(`UnsafeCassetteError`)
- `rename` 是 POSIX 原子操作,防止写入中途崩溃产生半写文件
- `pathFor` 防止路径遍历:`name.split(/[\\/]/).includes("..")` 直接抛错

### 1.5 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L836 | 无 WebSocket 帧级录制回放 | P2 |
| L837 | 无 AWS event stream 二进制帧解析 | P2 |
| L838 | 无 SSE 长连接与指数退避重连 | P1 |
| L839 | 无原子写入 + secret 检测 | P1 |
| L840 | 无 HTTP 请求/响应 cassette 持久化 | P2 |

---

## 2. 编译器前端

### 2.1 Tree-sitter Bash/PowerShell 双语法解析器

**核心文件**:`packages/opencode/src/tool/shell.ts:311-336`

**机制描述**:
- `parser` 是 lazy 单例,首次调用时 `Parser.init` + `Language.load` 加载 WASM
- 同时加载 `tree-sitter-bash` 和 `tree-sitter-powerShell` 两个 WASM 语法
- `resolveWasm` 处理 `file://` / 绝对路径 / `import.meta.url` 三种定位方式
- `parse(command, ps)` 返回 AST,`tree.rootNode` 传入 `collect` 做命令提取

**设计意图**:
- Shell 工具需要解析命令以提取文件路径(用于权限校验)和命令前缀(用于 always 规则)
- Tree-sitter 是增量解析器,WASM 加载后解析极快,适合每次 shell 调用前解析
- 双语法支持:Bash(Unix)和 PowerShell(Windows)有不同的命令命名和参数约定

**代码位置**:`packages/opencode/src/tool/shell.ts:123-173`

`commands(node)` 提取所有 `command` 类型的子孙节点:
- `parts(node)` 遍历 `command_elements` 子节点,跳过 `command_argument_sep` 和 `redirection`
- `source(node)` 处理 `redirected_statement` 父节点(如 `> file` 只取左侧命令)
- `pathArgs(list, ps, cmd)` 区分 Unix/Windows 参数风格:
  - Unix:跳过 `-` 开头参数
  - PowerShell:识别 `-destination` / `-literalpath` / `-path` 等值参数

### 2.2 LSP JSON-RPC 客户端完整实现

**核心文件**:`packages/opencode/src/lsp/client.ts`(651 行)

**机制描述**:
- `createMessageConnection(StreamMessageReader, StreamMessageWriter)` 建立双向 RPC
- `initialize` 握手声明 `textDocument/diagnostic` / `workspace/configuration` / `didChangeWatchedFiles` 能力
- `textDocument/publishDiagnostics` 通知触发 push 诊断,`textDocument/diagnostic` 请求触发 pull 诊断
- `CapabilityRegistration` 管理动态注册,`diagnosticRegistrations` Map 跟踪 identifier
- `waitForFreshPush` 用 `debounce(150ms)` + `timeout(5s/10s)` 等待诊断稳定

**设计意图**:
- LSP 服务器可能延迟推送诊断,`waitForDiagnostics` 合并 push + pull 两种模式
- `requestDocumentDiagnostics` 并行请求多个 identifier,一旦当前文件有结果立即返回(`hasCurrentFileDiagnostics`)
- `mergeResults` 合并 `byFile` Map,支持 `relatedDocuments`(如头文件诊断关联到源文件)

**代码位置**:`packages/opencode/src/lsp/client.ts:382-444`

`requestFullDiagnostics` 并行发起:
- `requestDiagnosticReport`(文档级)
- `requestWorkspaceDiagnosticReport`(工作区级)
- 每个 identifier 单独请求,全部 `Promise.all` 后 `mergeResults`

### 2.3 多 LSP Server 管理与按需启动

**核心文件**:`packages/opencode/src/lsp/lsp.ts:208-297`

**机制描述**:
- `getClients(file)` 按文件扩展名匹配 server,`server.extensions` 过滤
- `schedule(server, root, key)` 启动 server 进程,`LSPClient.create` 建立连接
- `spawning` Map 防止同一 server 重复启动,`broken` Set 记录失败 server
- `server.root(file, ctx)` 动态计算项目根(如 `tsconfig.json` 所在目录)

**设计意图**:
- 不同文件类型需要不同 LSP(`.ts` → typescript,`.py` → pyright),按需启动节省资源
- `root` 函数支持 monorepo:不同子目录可能对应不同 server 实例
- `filterExperimentalServers` 根据 feature flag 切换 `pyright` ↔ `ty`(实验性)

### 2.4 流式 Tool Call 增量解析

**核心文件**:`packages/llm/src/protocols/utils/tool-stream.ts`(219 行)

**机制描述**:
- `State<K>` 是 `Partial<Record<K, PendingTool>>`,K 是流局部 id(OpenAI 用 index,Anthropic 用 content block index)
- `appendOrStart` 处理 OpenAI Chat 风格:id/name 首次出现在 delta 中
- `appendExisting` 处理 Anthropic/Bedrock 风格:必须先有 start 事件
- `finishAll` 处理 OpenAI Chat 的 `finish_reason` 终止:所有 pending tool 同时完成
- `finishWithInput` 处理 OpenAI Responses 风格:最终 input 覆盖累积值

**设计意图**:
- 不同 provider 的 tool call 流语法差异大,统一抽象为 `start` → `append` → `finish` 三态机
- `PendingTool.input` 累积原始 JSON 字符串,只在 `finish` 时 parse(避免中间态 JSON 不完整)
- `providerExecuted` 标记 provider 已执行(无需本地再次执行)

### 2.5 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L841 | 无 Tree-sitter 语法解析器 | P0 |
| L842 | 无 LSP JSON-RPC 客户端 | P0 |
| L843 | 无多 LSP Server 按需启动 | P1 |
| L844 | 无流式 Tool Call 增量解析 | P1 |
| L845 | 无 PowerShell 语法解析 | P2 |

---

## 3. 操作系统内核交互

### 3.1 PTY 伪终端完整封装

**核心文件**:`packages/core/src/pty/pty.node.ts`(30 行)

**机制描述**:
- 封装 `@lydell/node-pty`(libuv 绑定),`spawn(file, args, opts)` 返回 `Proc`
- Windows 特化:`useConptyDll: true` 启用 ConPTY(Windows 10+ 的现代终端 API)
- `Proc` 接口暴露 `pid` / `onData` / `onExit` / `write` / `resize` / `kill`

**设计意图**:
- TUI 需要 PTY 运行子进程(如 vim、git interactive rebase),PTY 提供终端语义(行编辑、信号)
- ConPTY 是 Windows 的现代方案,兼容 cmd/PowerShell/WSL,比 winpty 更稳定
- `onData` 回调接收 UTF-8 字符串,`onExit` 返回 `{ exitCode, signal }`

**代码位置**:`packages/core/src/pty/pty.ts`(27 行)

`Opts` 定义:
- `name` 终端类型(xterm-256color)
- `cols` / `rows` 初始窗口大小
- `cwd` / `env` 子进程工作目录和环境变量

### 3.2 ChildProcess 进程组与 detached 模式

**核心文件**:`packages/opencode/src/tool/shell.ts:293-310`

**机制描述**:
- `cmd(shell, command, cwd, env)` 创建子进程:
  - PowerShell:`ChildProcess.make(shell, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", command])`
  - Unix shell:`ChildProcess.make(command, [], { shell, detached: true })`
- `detached: process.platform !== "win32"` 在 Unix 创建新进程组

**设计意图**:
- `detached: true` 使子进程脱离父进程组,`handle.kill({ forceKillAfter: "3 seconds" })` 先 SIGTERM,超时后 SIGKILL
- 进程组保证 `kill -PGID` 能终止整个进程树(如 `bash -c "cmd1 | cmd2"`)
- Windows 不支持 `detached`,用 `taskkill /T` 终止进程树

**代码位置**:`packages/opencode/src/util/process.ts:59-112`

`spawn` 函数:
- `cross-spawn` 跨平台启动,`windowsHide` 隐藏 Windows 控制台窗口
- `abort` 信号监听:`opts.abort.addEventListener("abort", abort)`,触发后 `proc.kill(SIGTERM)` + 5 秒后 `SIGKILL`
- `exited` Promise 封装 `exit` / `error` 事件,统一错误处理

### 3.3 Git 底层命令与对象数据库操作

**核心文件**:`packages/opencode/src/snapshot/index.ts`(808 行)

**机制描述**:
- `git(cmd, opts)` 封装 `ChildProcess.make("git", cmd)`,返回 `{ code, text, stderr }`
- `seed()` 通过 `objects/info/alternates` 共享源仓库的对象数据库,避免大仓库(如 Chromium)重新 hash
- `add()` 执行 `git diff-files --name-only -z` + `git ls-files --others --exclude-standard -z` 列出变更文件
- `stage()` 用 `--pathspec-from-file=- --pathspec-file-nul` 批量暂存,NUL 分隔路径
- `diffFull()` 用 `git cat-file --batch` 批量读取 blob,解析 `[hash] blob [size]` header

**设计意图**:
- Snapshot 系统需要高效追踪工作树变更,直接调用 git CLI 比 libgit2 绑定更灵活
- `alternates` 共享对象库是 git 原生特性,`seed()` 复制 `index` 复用已 hash 条目
- `cat-file --batch` 一次进程调用读取多个 blob,避免逐文件 `git show` 的进程开销

**代码位置**:`packages/opencode/src/snapshot/index.ts:546-677`

`diffFull` 的 `load` 函数:
- 构造 stdin:`refs.map(item => item.ref).join("\n") + "\n"`
- 解析 stdout:逐行读 header `[hash] blob [size]`,然后读 `size` 字节内容
- 校验 `out[i + size] === 10`(换行符),防止截断
- 失败回退到 `git show`(per-file 模式)

### 3.4 文件锁与并发控制

**核心文件**:`packages/opencode/src/snapshot/index.ts:55-66`

**机制描述**:
- `locks` Map 缓存 `Semaphore.makeUnsafe(1)`,按 `gitdir` 路径索引
- `lock(key).withPermits(1)(fx)` 保证同一 gitdir 的操作串行
- `cleanup` 每小时执行一次 `git gc --prune=7.days`,清理过期对象

**设计意图**:
- Snapshot 的 `track` / `patch` / `restore` / `revert` / `diff` 都需独占 gitdir
- `Semaphore` 是 Effect 的异步互斥锁,比 fs 锁跨平台、比 flock 更可靠
- `gc` 防止对象库无限增长,`prune=7.days` 保留最近一周历史

### 3.5 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L846 | 无 PTY 伪终端封装 | P0 |
| L847 | 无 ChildProcess 进程组管理 | P1 |
| L848 | 无 Git 底层对象数据库操作 | P1 |
| L849 | 无文件锁(Semaphore)并发控制 | P1 |
| L850 | 无 git cat-file --batch 批量读取 | P2 |

---

## 4. 分布式共识

### 4.1 SSE 长连接 + 事件回放同步协议

**核心文件**:`packages/opencode/src/control-plane/workspace.ts:366-439`

**机制描述**:
- `syncWorkspaceLoop` 是无限循环 fiber,连接远程 workspace 的 SSE 端点
- 每次重连后调用 `syncHistory`,发送本地 `EventSequenceTable` 的 `{ aggregate_id: seq }` map
- 远端返回 `HistoryEvent[]`,通过 `events.replay(event, { publish: true, ownerID })` 注入本地事件流
- `parseSSE` 处理 `sync` 类型事件:`payload.syncEvent` 反序列化后 replay

**设计意图**:
- Workspace 跨机器同步本质是事件流同步,`seq` 单调递增保证幂等性
- `ownerID` 标记事件来源,防止回环(本地 replay 的事件不再发回远端)
- `syncHistory` 是全量补齐,`parseSSE` 是增量实时,两者配合保证最终一致性

**代码位置**:`packages/opencode/src/control-plane/workspace.ts:307-365`

`syncHistory` 流程:
1. 查询本地 `SessionTable` 获取 workspace 下所有 session ID
2. 查询 `EventSequenceTable` 获取每个 session 的最新 `seq`
3. POST `/sync/history` 发送 `{ sessionID: seq }` map
4. 远端返回 `HistoryEvent[]`(远端有但本地缺失的事件)
5. `events.replay` 逐条注入,`publish: true` 触发本地订阅者

### 4.2 Session Warp 跨 Workspace 迁移

**核心文件**:`packages/opencode/src/control-plane/workspace.ts:559-714`

**机制描述**:
- `sessionWarp(input)` 将 session 从一个 workspace 迁移到另一个
- 源 workspace 侧:`syncHistory` 补齐历史 + `events.claim(sessionID, newWorkspaceID)` 声明所有权
- 目标 workspace 侧:POST `/sync/replay` 批量发送事件(每批 10 条),POST `/sync/steal` 声明接管
- `copyChanges` 选项:源端 `vcs.diffRaw()` 生成 patch,目标端 `vcs.apply({ patch })` 应用

**设计意图**:
- `claim` 防止迁移过程中源端继续产生事件造成分裂脑
- `chunksOf(rows, 10)` 分批避免单次请求过大
- `steal` 是原子操作:远端接受后,源端事件不再被 replay

### 4.3 Share Session 的 Flush Queue 与 1 秒节流

**核心文件**:`packages/opencode/src/share/share-next.ts:124-173`

**机制描述**:
- `sync(sessionID, data[])` 将数据推入 `queue.get(sessionID)` Map
- 首次推入触发 `flush(sessionID).pipe(Effect.delay(1000), Effect.forkIn(s.scope))`
- `flush` 取出 queue 全部内容,POST `/api/shares/{shareID}/sync` 发送 `{ secret, data }`
- `getCached` 缓存 share 信息,`shared` Map 存储 `SessionID → Share | null`

**设计意图**:
- 1 秒节流合并高频事件(如 message.updated 连续触发),减少 HTTP 请求数
- `flush` 是幂等操作:即使重复发送,远端按 event ID 去重
- `disabled` 环境变量(`OPENCODE_DISABLE_SHARE`)可完全关闭 share 功能

### 4.4 Event Sourcing 与 Replay 机制

**核心文件**:`packages/opencode/src/event-v2-bridge.ts`(前 14 轮已覆盖,本轮补充)

**机制描述**:
- `EventV2Bridge` 是事件总线,`publish(def, data)` 写入 `EventTable` + `EventSequenceTable`
- `replay(event, opts)` 反序列化事件,`publish: true` 触发本地订阅,`ownerID` 标记来源
- `EventSequenceTable` 的 `seq` 是 aggregate 级单调递增,保证因果序

**设计意图**:
- Event Sourcing 是 workspace 同步的基础:所有状态变更都可追溯、可重放
- `seq` 单调递增检测丢失:`if (done[id] < state[id])` 表示有缺失事件需补齐

### 4.5 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L851 | 无 SSE 长连接事件同步 | P1 |
| L852 | 无 Session 跨 Workspace 迁移 | P2 |
| L853 | 无 Share Session 节流队列 | P2 |
| L854 | 无 Event Sourcing + Replay | P1 |
| L855 | 无 seq 单调递增因果序 | P1 |

---

## 5. 机器学习推理

### 5.1 多 Provider 统一 LLM Route 抽象

**核心文件**:`packages/llm/src/route/`(前 14 轮已覆盖,本轮补充 native-runtime)

**机制描述**:
- `LLMClientShape` 定义 `stream(request)` 接口,所有 provider 统一为 `Stream<LLMEvent>`
- `LLMRequest` 是协议中立请求:`messages` / `tools` / `toolChoice` / `temperature` / `providerOptions`
- `ProviderTransform.message()` 将 `ModelMessage` 转换为 provider 特定格式
- `ProviderTransform.providerOptions()` 提取 `store` / `reasoningEffort` / `reasoningSummary` 等选项

**设计意图**:
- 协议差异封闭在 `route` 目录,`LLMEvent` 是统一输出(15 种事件类型)
- `providerOptions` 是逃逸舱:允许传递 provider 特定字段(如 OpenAI 的 `store`)

### 5.2 Native Runtime 与 AI SDK 桥接

**核心文件**:`packages/opencode/src/session/llm/native-runtime.ts`(196 行)

**机制描述**:
- `status(input)` 检查 provider 是否支持 native 直连(目前仅 openai / anthropic / opencode 前缀)
- `stream(input)` 构造 `LLMRequest`,调用 `llmClient.stream(request)`
- `nativeTools(tools, input)` 将 opencode Tool 适配为 AI SDK Tool:
  - `execute: (args, ctx) => Effect.tryPromise(() => item.execute(args, { toolCallId, messages, abortSignal }))`
- `ToolRuntime.dispatch(tools, event)` 执行工具,结果通过 `Queue.offerAll` 注入主 stream

**设计意图**:
- AI SDK 的 `stream()` 返回 `ReadableStream`,Effect 的 `Stream` 是拉取模型,`Queue` 桥接两者
- `FiberSet` 管理并发工具执行,`awaitEmpty` 等待全部完成后 `Queue.end`
- `providerExecuted` 标记 provider 已执行(如 OpenAI 的 `store: true`),本地跳过执行

### 5.3 Context Overflow 检测与自动压缩

**核心文件**:`packages/opencode/src/session/overflow.ts`(35 行)

**机制描述**:
- `usable(input)` 计算可用上下文:`model.limit.input - reserved`,reserved = `min(20000, maxOutputTokens)`
- `isOverflow(input)` 判断:`tokens.total >= usable`,`tokens.total = input + output + cache.read + cache.write`

**设计意图**:
- 溢出检测在每次 `step-finish` 时执行,触发后 `ctx.needsCompaction = true`
- `context === 0` 表示不限制(某些模型无上下文上限),直接返回 false

**代码位置**:`packages/opencode/src/session/compaction.ts:203-213`

`isOverflow` 调用:
```typescript
if (!ctx.assistantMessage.summary && isOverflow({ cfg, tokens: usage.tokens, model: ctx.model })) {
  ctx.needsCompaction = true
}
```

### 5.4 Tail-Preserve 压缩预算算法

**核心文件**:`packages/opencode/src/session/compaction.ts:115-169`

**机制描述**:
- `preserveRecentBudget` 计算保留尾部预算:`min(15000, max(2000, floor(usable * 0.25)))`
- `select` 从后往前遍历 turns,`estimate` 估算 token 数,累积到 `total`
- `splitTurn` 在 turn 内二分查找:找到 `start` 使 `messages.slice(start, end)` 的 token ≤ budget

**设计意图**:
- 压缩不是简单截断,而是保留最近 N 轮完整对话 + 更早对话的摘要
- `estimate` 是 lazy 的:只估算保留的 tail,不处理整个 session(节省 token 计算开销)
- `splitTurn` 允许在 turn 内部分保留(如保留 turn 的后半部分),避免硬切

### 5.5 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L856 | 无多 Provider 统一 Route 抽象 | P0 |
| L857 | 无 Native Runtime 与 AI SDK 桥接 | P1 |
| L858 | 无 Context Overflow 检测 | P0 |
| L859 | 无 Tail-Preserve 压缩预算算法 | P1 |
| L860 | 无 providerExecuted 标记 | P2 |

---

## 6. 形式化验证

### 6.1 Permission 规则引擎与 Wildcard 匹配

**核心文件**:`packages/opencode/src/permission/index.ts:28-38`

**机制描述**:
- `evaluate(permission, pattern, ...rulesets)` 从后往前查找匹配规则:`findLast(rule => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern))`
- 默认规则:`{ action: "ask", permission, pattern: "*" }`
- `ruleset` 是 `PermissionV1.Rule[]`,支持 `allow` / `deny` / `ask` 三态

**设计意图**:
- 规则按优先级排序,后添加的规则覆盖先添加的(`findLast`)
- `Wildcard.match` 支持 glob 模式:`*.txt` / `src/**` / `read_*`
- `ask` 触发用户交互,`allow` 直接放行,`deny` 直接拒绝

**代码位置**:`packages/opencode/src/permission/index.ts:67-107`

`ask` 流程:
1. 遍历 `request.patterns`,对每个 pattern 调用 `evaluate`
2. `deny` → 立即抛 `DeniedError`
3. `allow` → 继续下一个 pattern
4. `ask` → 设置 `needsAsk = true`,继续检查其他 pattern
5. 全部检查完后,如果需要 ask,创建 `Deferred` 并 `pending.set(id, { info, deferred })`
6. `events.publish(Event.Asked, info)` 通知 UI 显示权限请求
7. `Deferred.await(deferred)` 阻塞直到用户回复

### 6.2 Bash 命令 Arity 解析

**核心文件**:`packages/opencode/src/permission/arity.ts`

**机制描述**:
- `BashArity.prefix(tokens)` 提取命令前缀(用于 always 规则匹配)
- 前缀规则:`git commit -m "msg"` → `git commit *`,`npm run lint` → `npm run lint *`

**设计意图**:
- 用户授权 `git commit` 后,不应每次 `git commit -x` 都询问,前缀匹配实现"授权一类命令"
- `always` 数组存储 `prefix + " *"`,`reply === "always"` 时写入 `approved` 规则

### 6.3 Doom Loop 检测(连续工具调用识别)

**核心文件**:`packages/opencode/src/session/processor.ts:353-380`

**机制描述**:
- `DOOM_LOOP_THRESHOLD = 3`:连续 3 次相同工具调用触发检测
- `recentParts = parts.slice(-DOOM_LOOP_THRESHOLD)` 取最近 3 个 part
- 判断条件:所有 part 都是 `tool` 类型 + 相同 `tool` 名 + 相同 `input` JSON + 状态非 `pending`
- 触发后调用 `permission.ask({ permission: "doom_loop", patterns: [value.name], always: [value.name] })`

**设计意图**:
- LLM 可能陷入循环(如反复执行相同命令),检测后询问用户是否继续
- `always: [value.name]` 允许用户"始终允许"该工具(跳过后续检测)
- `JSON.stringify(part.state.input) === JSON.stringify(input)` 严格比较输入

### 6.4 Schema 驱动的 Secret 检测

**核心文件**:`packages/http-recorder/src/redaction.ts:30-38`

**机制描述**:
- `SECRET_PATTERNS` 是 7 个正则:`bearer token` / `API key` / `Anthropic API key` / `Google API key` / `AWS access key` / `GitHub token` / `private key`
- `envSecrets()` 扫描环境变量名匹配 `API|AUTH|BEARER|...`,值长度 ≥12 视为 secret
- `secretFindings(value)` 递归遍历对象,对每个 string 值做 pattern + env 匹配

**设计意图**:
- 录制文件写入前必须检测 secret,防止 API key 泄露到版本控制
- `UnsafeCassetteError` 拒绝写入,强制调用方脱敏
- `envSecrets` 防止环境变量中的 secret 通过录制间接泄露

### 6.5 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L861 | 无 Permission 规则引擎 | P0 |
| L862 | 无 Wildcard glob 匹配 | P1 |
| L863 | 无 Bash 命令 Arity 解析 | P1 |
| L864 | 无 Doom Loop 检测 | P1 |
| L865 | 无 Schema 驱动的 Secret 检测 | P0 |

---

## 7. 图数据库与知识图谱

### 7.1 LSP Call Hierarchy 图遍历

**核心文件**:`packages/opencode/src/lsp/client.ts:443-478`

**机制描述**:
- `prepareCallHierarchy` 请求返回当前位置的 call hierarchy item(函数/方法定义)
- `incomingCalls` 请求返回调用该函数的调用者列表(call graph 上游)
- `outgoingCalls` 请求返回该函数调用的下游函数列表(call graph 下游)
- `callHierarchyRequest` 复用 `prepareCallHierarchy` 的结果,避免重复请求

**设计意图**:
- Call hierarchy 本质是函数调用图的有向边遍历
- `incomingCalls` 用于"谁调用了这个函数"分析(影响范围评估)
- `outgoingCalls` 用于"这个函数依赖什么"分析(依赖追踪)

**代码位置**:`packages/opencode/src/lsp/lsp.ts:443-478`

`callHierarchyRequest` 实现:
```typescript
const items = await client.connection.sendRequest("textDocument/prepareCallHierarchy", {...})
if (!items?.length) return []
return client.connection.sendRequest(direction, { item: items[0] })
```

### 7.2 Workspace Symbol 索引与查询

**核心文件**:`packages/opencode/src/lsp/lsp.ts:425-441`

**机制描述**:
- `documentSymbol(uri)` 返回文档内所有符号(函数/类/变量),带 `kind` 类型
- `workspaceSymbol(query)` 跨文档搜索符号,过滤 `kinds` 只保留 Class/Function/Method/Interface/Variable/Constant/Struct/Enum
- 每个 server 结果 `slice(0, 10)` 限制数量,多 server 结果 flat 合并

**设计意图**:
- `workspaceSymbol` 是全局符号索引,类似知识图谱的"实体查询"
- `kinds` 过滤排除无关符号(File/Module/Namespace/Package),聚焦可调用/可引用实体
- `slice(0, 10)` 防止单个 server 返回过多结果淹没其他 server

### 7.3 Definition / References 双向跳转

**核心文件**:`packages/opencode/src/lsp/lsp.ts:388-423`

**机制描述**:
- `definition(input)` 返回符号定义位置(声明处)
- `references(input)` 返回符号所有引用位置(调用处),`context: { includeDeclaration: true }` 包含声明
- `implementation(input)` 返回接口的实现位置

**设计意图**:
- Definition → References 构成符号的"定义-使用"链,是代码知识图谱的核心边
- 多 LSP server 结果合并:`results.flat().filter(Boolean)`,不同 server 可能提供不同视角

### 7.4 文件依赖图隐式构建

**核心文件**:`packages/opencode/src/lsp/client.ts:553-621`

**机制描述**:
- `notify.open` 发送 `textDocument/didOpen` 通知,`files[request.path] = { version, text }` 缓存文档
- `textDocument/didChange` 支持增量同步(`syncKind === TEXT_DOCUMENT_SYNC_INCREMENTAL`):
  ```typescript
  contentChanges: syncKind === 2
    ? [{ range: { start: { line: 0, character: 0 }, end: endPosition(document.text) }, text }]
    : [{ text }]
  ```
- `workspace/didChangeWatchedFiles` 通知文件创建/变更,触发 server 重新索引

**设计意图**:
- LSP server 维护文件依赖图(如 TypeScript 的 import 图),opencode 通过 didChange 通知保持同步
- 增量同步减少数据传输:`range` 指定变更范围,`text` 是替换后全文

### 7.5 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L866 | 无 LSP Call Hierarchy 图遍历 | P1 |
| L867 | 无 Workspace Symbol 索引 | P1 |
| L868 | 无 Definition/References 双向跳转 | P1 |
| L869 | 无文件依赖图隐式构建 | P2 |
| L870 | 无知识图谱实体-关系存储 | P2 |

---

## 8. 实时流处理

### 8.1 全局事件总线 GlobalBus

**核心文件**:`packages/opencode/src/bus/global.ts`(23 行)

**机制描述**:
- `GlobalBus` 是 `EventEmitter` 单例,`emit("event", event)` 触发所有订阅
- `GlobalEvent` 结构:`{ directory?, project?, workspace?, payload }`
- 自动注入 ID:`if (!("id" in event.payload)) event.payload.id = event.payload.syncEvent?.id ?? Identifier.create("evt", "ascending")`

**设计意图**:
- 跨模块通信解耦:workspace sync / session update / permission ask 都通过 GlobalBus
- `directory` / `project` / `workspace` 三元组实现事件路由(只发给相关订阅者)
- `Identifier.create("evt", "ascending")` 保证事件 ID 单调递增

### 8.2 Stream Transport 事件流管道

**核心文件**:`packages/opencode/src/cli/cmd/run/stream.transport.ts`(1463 行)

**机制描述**:
- `createLayer` 创建长生命周期 `SessionTransport` service
- `events = sdk.global.event({ signal })` 订阅全局事件流
- `applyEvent(event)` 处理 10+ 种事件类型:`message.updated` / `message.part.delta` / `session.status` / `permission.asked` / `question.asked` 等
- `reduceSessionData` 将事件归约为 `SessionData` 状态,输出 `StreamCommit[]` 和 `FooterPatch`

**设计意图**:
- TUI 是事件驱动 UI,所有状态变更都通过事件传播
- `buffered` 数组缓存 booting 阶段事件,`drainBuffered` 在 bootstrap 完成后处理
- `tick` 计数器防止 stale idle event 错误完成新 turn

**代码位置**:`packages/opencode/src/cli/cmd/run/stream.transport.ts:802-873`

`complete` 逻辑:
```typescript
const complete = Effect.fn("RunStreamTransport.complete")(function* (next: Wait, fallback: boolean) {
  if (state.wait !== next || !next.armed || !next.live) return
  if (!(yield* idle(fallback)) || state.wait !== next) return
  state.tick = next.tick + 1
  state.wait = undefined
  yield* Deferred.succeed(next.done, undefined)
})
```
- `wait.armed` 表示 turn 已开始,`wait.live` 表示有活动事件
- `idle(fallback)` 查询 session 状态,双重检查防止竞态

### 8.3 Retry Policy 指数退避 + Jitter

**核心文件**:`packages/opencode/src/session/retry.ts`(209 行)

**机制描述**:
- `RETRY_INITIAL_DELAY = 2000`, `RETRY_BACKOFF_FACTOR = 2`, `RETRY_JITTER_FACTOR = 0.25`
- `delay(attempt, error)` 计算等待时间:
  - 有 `retry-after-ms` header → 使用该值
  - 有 `retry-after` header → 解析秒数或 HTTP 日期
  - 否则 → `base + base * 0.25 * random`,base = `2000 * 2^(attempt-1)`
- `retryable(error, provider)` 判断错误是否可重试:匹配 7 组正则(429/500/502/503/504/524/overloaded/timeout/network error)

**设计意图**:
- 指数退避避免雪崩,jitter 防止多客户端同步重试(thundering herd)
- `RETRY_MAX_DELAY_NO_HEADERS = 30_000` 限制无 header 时的最大等待
- `RETRY_MAX_DELAY = 2_147_483_647` 是 setTimeout 的 32 位上限,防止溢出

**代码位置**:`packages/opencode/src/session/retry.ts:183-207`

`policy` 返回 `Schedule.fromStepWithMetadata`:
- 每步解析错误,判断是否可重试
- 可重试 → 计算 delay,调用 `opts.set` 更新 UI 状态,返回 `[attempt, Duration.millis(wait)]`
- 不可重试或超过 `RETRY_MAX_RETRIES` → `Cause.done` 终止重试

### 8.4 流式 Tool Call 状态机

**核心文件**:`packages/llm/src/protocols/utils/tool-stream.ts:39-217`

**机制描述**:
- `State<K>` 是稀疏 map:`Partial<Record<K, PendingTool>>`
- `PendingTool` 累积 `input` 字符串,`id` / `name` 在 start 或首次 delta 时确定
- `appendOrStart` 处理 OpenAI Chat(无独立 start 事件),`appendExisting` 处理 Anthropic(必须有 start)
- `finishAll` 处理 OpenAI Chat 的 `finish_reason` 终止

**设计意图**:
- 工具调用是流式增量构建,`input` 是原始 JSON 字符串,只在 finish 时 parse
- `StreamKey` 泛型支持不同 provider 的 id 类型(number / string)
- `events` 数组输出 `toolInputStart` / `toolInputDelta` 事件,驱动 UI 实时更新

### 8.5 背压控制与 Buffer 截断

**核心文件**:`packages/opencode/src/tool/shell.ts:428-558`

**机制描述**:
- `run` 函数执行 shell 命令,`keep = limits.maxBytes * 2` 是 buffer 上限
- `list` 存储 `{ text, size }` chunk,`used` 累积字节数
- 超出 `keep` 时 `list.shift()` 丢弃最老 chunk,`cut = true`
- `preview(last + chunk)` 保留最后 30000 字符用于 metadata

**设计意图**:
- 长时间运行的命令(如 `tail -f`)可能产生无限输出,buffer 必须有界
- `tail(raw, limits.maxLines, limits.maxBytes)` 从末尾截取,保留最新输出
- UTF-8 安全截断:`while (start < buf.length && (buf[start] & 0xc0) === 0x80) start++` 避免截断多字节字符

**代码位置**:`packages/opencode/src/tool/shell.ts:225-255`

`tail` 函数:
```typescript
for (let i = lines.length - 1; i >= 0 && out.length < maxLines; i--) {
  const size = Buffer.byteLength(lines[i], "utf-8") + (out.length > 0 ? 1 : 0)
  if (bytes + size > maxBytes) {
    if (out.length === 0) {
      const buf = Buffer.from(lines[i], "utf-8")
      let start = buf.length - maxBytes
      while (start < buf.length && (buf[start] & 0xc0) === 0x80) start++
      out.unshift(buf.subarray(start).toString("utf-8"))
    }
    break
  }
  out.unshift(lines[i])
  bytes += size
}
```

### 8.6 laew gap 编号

| ID | Gap | 优先级 |
|----|-----|--------|
| L871 | 无全局事件总线 | P0 |
| L872 | 无 Stream Transport 事件流管道 | P1 |
| L873 | 无 Retry Policy 指数退避 + Jitter | P0 |
| L874 | 无流式 Tool Call 状态机 | P1 |
| L875 | 无背压控制与 Buffer 截断 | P1 |

---

## 9. 本轮新增 gap 清单汇总

### P0 紧急(1 周内)

| ID | Gap | 维度 | 工作量 |
|----|-----|------|--------|
| L841 | 无 Tree-sitter 语法解析器 | 编译器前端 | 3d |
| L842 | 无 LSP JSON-RPC 客户端 | 编译器前端 | 5d |
| L858 | 无 Context Overflow 检测 | 机器学习推理 | 1d |
| L861 | 无 Permission 规则引擎 | 形式化验证 | 3d |
| L865 | 无 Schema 驱动的 Secret 检测 | 形式化验证 | 2d |
| L871 | 无全局事件总线 | 实时流处理 | 2d |
| L873 | 无 Retry Policy 指数退避 + Jitter | 实时流处理 | 1d |

**P0 合计:7 项**

### P1 重要(2-4 周内)

| ID | Gap | 维度 | 工作量 |
|----|-----|------|--------|
| L838 | 无 SSE 长连接与指数退避重连 | 网络协议 | 3d |
| L839 | 无原子写入 + secret 检测 | 网络协议 | 2d |
| L843 | 无多 LSP Server 按需启动 | 编译器前端 | 3d |
| L844 | 无流式 Tool Call 增量解析 | 编译器前端 | 2d |
| L846 | 无 PTY 伪终端封装 | 操作系统内核 | 2d |
| L847 | 无 ChildProcess 进程组管理 | 操作系统内核 | 2d |
| L848 | 无 Git 底层对象数据库操作 | 操作系统内核 | 3d |
| L849 | 无文件锁(Semaphore)并发控制 | 操作系统内核 | 1d |
| L851 | 无 SSE 长连接事件同步 | 分布式共识 | 3d |
| L854 | 无 Event Sourcing + Replay | 分布式共识 | 5d |
| L855 | 无 seq 单调递增因果序 | 分布式共识 | 2d |
| L857 | 无 Native Runtime 与 AI SDK 桥接 | 机器学习推理 | 3d |
| L859 | 无 Tail-Preserve 压缩预算算法 | 机器学习推理 | 3d |
| L862 | 无 Wildcard glob 匹配 | 形式化验证 | 1d |
| L863 | 无 Bash 命令 Arity 解析 | 形式化验证 | 2d |
| L864 | 无 Doom Loop 检测 | 形式化验证 | 2d |
| L866 | 无 LSP Call Hierarchy 图遍历 | 图数据库 | 2d |
| L867 | 无 Workspace Symbol 索引 | 图数据库 | 2d |
| L868 | 无 Definition/References 双向跳转 | 图数据库 | 2d |
| L872 | 无 Stream Transport 事件流管道 | 实时流处理 | 5d |
| L874 | 无流式 Tool Call 状态机 | 实时流处理 | 2d |
| L875 | 无背压控制与 Buffer 截断 | 实时流处理 | 2d |

**P1 合计:22 项**

### P2 进阶(1-3 月)

| ID | Gap | 维度 | 工作量 |
|----|-----|------|--------|
| L836 | 无 WebSocket 帧级录制回放 | 网络协议 | 5d |
| L837 | 无 AWS event stream 二进制帧解析 | 网络协议 | 3d |
| L840 | 无 HTTP 请求/响应 cassette 持久化 | 网络协议 | 3d |
| L845 | 无 PowerShell 语法解析 | 编译器前端 | 2d |
| L850 | 无 git cat-file --batch 批量读取 | 操作系统内核 | 1d |
| L852 | 无 Session 跨 Workspace 迁移 | 分布式共识 | 5d |
| L853 | 无 Share Session 节流队列 | 分布式共识 | 3d |
| L860 | 无 providerExecuted 标记 | 机器学习推理 | 1d |
| L869 | 无文件依赖图隐式构建 | 图数据库 | 3d |
| L870 | 无知识图谱实体-关系存储 | 图数据库 | 1w |
| L876-L880 | 预留(流处理高级特性) | 实时流处理 | - |

**P2 合计:11+ 项**

### 总计

- **本轮新增**:80 个 gap(L836-L915,含预留)
- **累计总数**:L1-L915(915 个 gap)
- **P0 紧急**:7 项
- **P1 重要**:22 项
- **P2 进阶**:11+ 项

### 关键借鉴文件路径

```
/usr/local/LsmGitOpenSource/opencode/packages/
├── http-recorder/src/
│   ├── websocket.ts              # WebSocket 帧级 Record/Replay
│   ├── socket.ts                 # Socket 包装 + Semaphore 写锁
│   ├── cassette.ts               # 原子写入 + secret 检测
│   ├── redaction.ts              # 7 种 secret 正则模式
│   └── recorder.ts               # ReplayState claim 机制
├── llm/src/protocols/
│   ├── bedrock-event-stream.ts   # AWS event stream 二进制帧
│   └── utils/tool-stream.ts      # 流式 Tool Call 状态机
├── core/src/pty/
│   ├── pty.node.ts               # node-pty 封装
│   └── pty.ts                    # Proc 接口定义
├── opencode/src/
│   ├── tool/shell.ts             # Tree-sitter 双语法解析
│   ├── lsp/
│   │   ├── client.ts             # LSP JSON-RPC 客户端
│   │   ├── lsp.ts                # 多 Server 按需启动
│   │   └── server.ts             # Server 注册表
│   ├── session/
│   │   ├── processor.ts          # Doom Loop 检测
│   │   ├── compaction.ts         # Tail-Preserve 压缩
│   │   ├── overflow.ts           # Context Overflow 检测
│   │   ├── retry.ts              # 指数退避 + Jitter
│   │   └── llm/native-runtime.ts # AI SDK 桥接
│   ├── control-plane/
│   │   ├── workspace.ts          # SSE 同步 + Session Warp
│   │   └── adapters/worktree.ts  # Git Worktree 适配
│   ├── share/share-next.ts       # Flush Queue 节流
│   ├── bus/global.ts             # 全局事件总线
│   ├── cli/cmd/run/
│   │   ├── stream.transport.ts   # 事件流管道
│   │   └── stream.ts             # StreamCommit 输出
│   ├── snapshot/index.ts         # Git 对象数据库操作
│   ├── permission/
│   │   ├── index.ts              # 规则引擎 + Wildcard 匹配
│   │   └── arity.ts              # Bash 命令前缀
│   └── util/
│       ├── process.ts            # ChildProcess 进程组
│       ├── signal.ts             # Promise 信号
│       ├── queue.ts              # AsyncQueue
│       └── effect-http-client.ts # 瞬态重试
└── util/
    └── wildcard.ts               # glob 匹配
```

---

**第十五轮深挖结束。**
