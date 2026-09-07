# 第八轮深挖：LSP / Language Server Protocol / IDE 集成 / CodeIntel 深度对比

> 调研范围：atomcode、claudecode、deepseek-harness、openclaw、opencode、pi、undici、hermes-agent
> 调研维度：JSON-RPC stdio LSP 完整实现、多语言 Server 管理、tree-sitter 符号提取、Dev Container 集成、IDE 客户端适配、CodeIntel 七件套、semantic_tokens / inlay_hints、workspace/symbol 与 executeCommand
> 与第七轮不重复章节：Bash/PTY、子进程管理（仅在 LSP 子进程管理关联处引用）、结构化输出（仅 LSP 协议层用到的部分）。

---

## 1. 摘要与 TL;DR

**LSP（Language Server Protocol）** 是一份基于 JSON-RPC 2.0 的「编辑器/客户端 ↔ 语言服务器」协议：客户端打开文件，服务器推送 diagnostics/补全/定义/引用/hover 等语义信息。对 Agent 而言，LSP 是把"读懂代码"从「字符串匹配/正则猜」升级为「真实编译期语义」的关键能力。本专题对比 7 个工程（undici 不是 Agent 工程，仅作 HTTP 协议层引用），得出 7 条核心结论：

1. **「LSP 客户端」实现上分两派**：自研轻量 JSON-RPC 帧（atomcode、deepseek-harness、hermes-agent，~80–350 行）vs 复刻 `vscode-jsonrpc/node`（opencode、claudecode，依赖 npm 上 vscode-languageserver-protocol 一族）。前者可控、可单测、可移植；后者省事但与生态深度绑定。
2. **「CodeIntel 七件套」是 atomcode 独有的设计语言**（`crates/atomcode-capabilities/src/codeintel/`）：把 LSP 的「request/response」语义拆成 7 个模型可调用的具名工具（`list_symbols`/`read_symbol`/`find_references`/`trace_callers`/`trace_callees`/`trace_chain`/`blast_radius`/`file_dependencies`），且 LSP 是 feature-gated 的可选后端 —— 不是默认通道。这是把 LSP 从「后台服务」转译成「LLM 工具」的最完整范式。
3. **多语言 Server 管理**：每个工程都有"扩展名 → 二进制 → 启动参数 → 根目录标记"的注册表。差异在**根目录解析**（claudecode 用向上最近文件匹配，opencode 把 root 当作 `async (file, ctx) => ctx.directory` 默认 + 最近标记覆盖，atomcode 走 `server_root(workspace_root, path, markers)` 向上爬升）和**粘性失败处理**（atomcode 把启动失败的 server 写入 `unavailable` HashMap，避免重试风暴；opencode 维护 `broken: Set<string>`；claudecode 用 `restartCount` 计数器）。
4. **tree-sitter 与 LSP 的关系**：所有用 LSP 的工程都同时用 tree-sitter 做"轻量、不依赖二进制"的代码感知（atomcode 12 个 grammar，openclaw 用 `web-tree-sitter` 跑 bash 单 grammar 做命令解释，claudecode 在 `utils/bash/treeSitterAnalysis.ts` 用 tree-sitter 给 bash 风险判定）。两者不是替代关系 —— tree-sitter 总是先于/旁路于 LSP 存在。
5. **Dev Container 集成** 在 7 个工程里几乎都是 **空缺**：没有工程把 `.devcontainer/devcontainer.json` 当作 LSP 启动策略来源（容器内的 `rust-analyzer` 需要在容器内 spawn，而不是 host）。这是行业空白点，也是 laew 可以独占的位置。
6. **IDE 客户端适配**：opencode 在 `packages/opencode/src/ide/index.ts` 实现了**反向**IDE 集成 —— 检测 `TERM_PROGRAM=vscode` + `GIT_ASKPASS` 启发式识别 IDE，提供扩展市场安装命令（VSCode/Cursor/Windsurf/VSCodium）。claudecode 走 `src/utils/ide.ts` + `commands/ide/` 命令子屏。opencode 的策略是「让 IDE 主动连 opencode」，claudecode 是「在 TUI 内做 IDE 风格的交互」。
7. **workspace/executeCommand 与 semantic_tokens/inlay_hints** 在所有 7 个 Agent 工程里**全部未实现**：executeCommand（让模型触发 `cargo build` 等外部命令）只在部分工程间接支持（atomcode Bash 工具），semantic_tokens/inlay_hints（语义高亮/内联提示）完全缺失 —— 这些属于 IDE 体验特性，Agent 工具场景优先级低。

**laew 现状（gap L26-L34）**：零 LSP 客户端、零 CodeIntel 工具、零 tree-sitter、零 IDE 适配、零 Dev Container 集成。**P0 路线图**（见 §7）：先 tree-sitter 符号提取（零依赖、纯 Rust crate），再 LSP JSON-RPC stdio（`lsp-types` + `tokio`），最后才考虑 IDE 反向集成。

---

## 2. 背景：Agent 为什么需要 LSP

### 2.1 字符串匹配的局限

Laew 当前唯一"读懂代码"的工具是 `read.rs` + `BashTool` 跑 grep。没有 LSP 时，模型只能靠正则猜符号引用：

```
$ grep -rn "fn main\b" src/
src/main.rs:7:fn main() {
```

这只能找定义，找不到**跨文件的引用、类型、hover 文档、调用层次、影响范围**。复杂重构（如改 `tokio::spawn` 的参数签名）模型必须依靠反复 read + 推理，命中率低。

### 2.2 LSP 给 Agent 的能力清单

| 能力 | LSP 方法 | 传统方式 | 命中率差异 |
|------|----------|----------|------------|
| 跳转到定义 | `textDocument/definition` | grep + 人工推断 | 显著提升 |
| 查找引用 | `textDocument/references` | grep 全文 + 过滤注释 | 显著提升 |
| Hover 文档 | `textDocument/hover` | 离线 README 推测 | 显著提升 |
| 文档符号 | `textDocument/documentSymbol` | tree-sitter / 人工 | 略升 |
| 工作区符号 | `workspace/symbol` | find/grep + ls | 略升 |
| 诊断 | `textDocument/publishDiagnostics` | 跑 compiler → 解析 stderr | 显著提升 |
| 调用层次 | `textDocument/prepareCallHierarchy` + `incomingCalls/outgoingCalls` | graph search | 显著提升 |
| 重命名/重构 | `workspace/executeCommand` + `textDocument/rename` | sed/perl 脚本 | 显著提升 |

### 2.3 三种实现路径

| 路径 | 依赖 | 启动成本 | 语义质量 | 工程典型代表 |
|------|------|----------|----------|--------------|
| **tree-sitter 纯本地 AST** | `tree-sitter` crate + 各语言 grammar（12 个 ~50MB 编译） | 零进程 | 仅结构 | atomcode（`codeintel/symbols.rs`） |
| **LSP 真服务器** | `lsp-types` + 子进程 + JSON-RPC stdio | 启动 `rust-analyzer` ~2-5s | 编译期语义 | opencode、claudecode、deepseek-harness、hermes-agent |
| **ctags/正则 grep** | 零依赖 | 零 | 仅文本 | laew 当前（Bash + grep） |

7 个工程里：**atomcode** 是唯一同时拥有 tree-sitter + LSP 双后端、可切换的工程；**opencode / claudecode** 是重度 LSP 用户；**deepseek-harness** 在 `packages/lsp/` 做了协议栈抽象 + JSON-RPC stdio transport 层；**hermes-agent** 在 `agent/lsp/` 走的是完整诊断回写路径；**openclaw** 只在命令解释里用了 tree-sitter bash；**pi/undici** 没有 LSP。

---

## 3. 每个工程的实际实现

### 3.1 atomcode — Rust 工程的 CodeIntel 七件套 + 完整 LSP 后端（最重要的参考）

#### 3.1.1 模块拓扑

```
crates/atomcode-capabilities/src/codeintel/
├── mod.rs                入口：注册 8 个 CodeIntel 工具（不含 LSP 工具）
├── lang.rs               Lang 枚举（12 语言）+ tree-sitter grammar + symbols/calls query
├── symbols.rs            单文件 AST 符号提取（stateless）
├── list_symbols.rs       "list_symbols" 工具
├── read_symbol.rs        "read_symbol" 工具（按名提取单个符号）
├── index.rs              跨文件 CodeIndex + build_graph
├── graph.rs              CodeGraph / Edge / EdgeKind / Visibility
├── find_references.rs    "find_references"（基于 CodeIndex 的反向引用）
├── trace_callers.rs      "trace_callees" / "trace_callers" / "trace_chain"
├── trace_callees.rs      同上三个工具
├── trace_chain.rs
├── blast_radius.rs       "blast_radius" 影响范围分析
├── file_deps.rs          "file_dependencies" 文件依赖图
├── diagnostics.rs        "diagnostics" LSP 工具（feature=lsp）
├── lsp_tool.rs           "lsp" 多合一 LSP 工具（feature=lsp）
└── lsp/
    ├── mod.rs            子模块出口
    ├── jsonrpc.rs        Content-Length 帧编解码（92 行）
    ├── client.rs         传输无关 LspClient（1170 行）
    ├── manager.rs        LspManager：池化、按 (root, command) 分组、sticky 失败（460 行）
    ├── registry.rs       LspServerRegistry + 默认 7 个 server
    └── types.rs          协议中立 Diagnostic/Location（124 行）
```

**代码量**：CodeIntel 模块总计 ~5500 行（不含 feature=lsp 时 ~3400 行）。其中 LSP 子模块 ~1990 行。

#### 3.1.2 LSP JSON-RPC 帧（最小可用集）

`crates/atomcode-capabilities/src/codeintel/lsp/jsonrpc.rs:1-92` —— **92 行**实现完整 Content-Length 帧编解码：

```rust
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;  // 16 MB 上限

pub fn encode(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

pub async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> std::io::Result<Vec<u8>> {
    // 读 header 块到空行 → 取 Content-Length → 精确读 N 字节
}
```

**3 个测试**（`encode_has_header_and_body`、`round_trips_two_messages`、`oversized_message_is_rejected_before_allocation`）覆盖：基本编解码、两条消息连发、超过 16MB 消息**在分配前**直接拒绝（避免内存炸弹）。

#### 3.1.3 传输无关的 LspClient（核心架构）

`crates/atomcode-capabilities/src/codeintel/lsp/client.rs:1-1170` 的关键设计 —— **transport-agnostic**：

```rust
//! A minimal, TRANSPORT-AGNOSTIC LSP client: the protocol (initialize handshake,
//! didOpen/didChange, a background reader that correlates responses by id and caches
//! `publishDiagnostics`) runs over any `AsyncRead`+`AsyncWrite`. `spawn` is the thin
//! wrapper that wires a child process's stdio. Ported from production `lsp/client.rs`.
//!
//! Transport-agnosticism is what makes the protocol DETERMINISTICALLY testable: a test
//! pairs `connect` with `tokio::io::duplex` + a mock-server coroutine — no real language
//! server needed (see the tests below).
```

签名（来自源码注释）：
```rust
type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;
type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
type SharedWrite = Arc<AsyncMutex<BoxWrite>>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, Value>>>>>;
type DiagMap = Arc<Mutex<HashMap<PathBuf, Vec<Diagnostic>>>>;
```

**架构要点**：
- `connect(R, W)` 用任意 AsyncRead+AsyncWrite，**生产**接 `tokio::process::Child` 的 stdin/stdout，**测试**接 `tokio::io::duplex()` —— 不需要真起 rust-analyzer。
- 请求/响应通过 `AtomicU64` 自增 id 关联，单个 `Mutex<HashMap<u64, oneshot::Sender>>` 暂存 pending。
- 后台 reader 协程**同时处理** notification（无 id，含 `method`）和 response（有 id，回填 pending map）；`publishDiagnostics` 被截走并存入 `DiagMap`。
- 30 秒超时（`REQUEST_TIMEOUT_SECS = 30`），统一通过 `tokio::time::timeout` 包裹。

#### 3.1.4 服务端 → 客户端 请求的「最小可应答集」

`lsp/client.rs:48-110` 的 `handle_server_request` —— **读-only LSP 客户端必须应答的 7 类服务端请求**：

| 方法 | 响应 | 原因 |
|------|------|------|
| `workspace/configuration` | 返回 N 个 `null`（按 items 数） | rust-analyzer 启动时会问 `[rust-analyzer].cargo.allTargets` 等 |
| `workspace/workspaceFolders` | 返回 `[{uri, name}]` 单元素 | gopls / vscode-languageclient 必查 |
| `client/registerCapability` | 记录 `textDocument/diagnostic` 是否启用 pull 诊断 | 影响后续 `textDocument/diagnostic` 是否要发 |
| `client/unregisterCapability` | 对称处理 | 同上 |
| `window/workDoneProgress/create` | `null` | 进度条占位 |
| `window/showMessageRequest` | `null` | 不弹 UI |
| `workspace/applyEdit` | `{applied: false, failureReason: "AtomCode LSP integration is read-only"}` | **关键：拒绝改文件** |
| `window/showDocument` | `{success: false}` | 不弹窗 |
| 其它 | 发 `-32601 MethodNotFound` 错误响应 | 显式失败，不静默吞 |

这是 AtomCode 给"只读 LSP 客户端"立的标准范式 —— **任何 Agent 内嵌的 LSP 客户端都必须**应答这 7 类，否则启动阶段就被服务器主动断开。

#### 3.1.5 LspManager：池化 + sticky 失败

`lsp/manager.rs:1-80` 的设计要点：

```rust
const SETTLE_DELAY_MS: u64 = 350;  // didOpen 后等 350ms 让 server 攒够 diagnostics
const STARTUP_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ClientKey { root: PathBuf, command: String, args: Vec<String>, }

pub struct LspManager {
    clients: Mutex<HashMap<ClientKey, Arc<LspClient>>>,
    startup_locks: Mutex<HashMap<ClientKey, Arc<Mutex<()>>>>,
    unavailable: Mutex<HashMap<ClientKey, String>>,  // ← sticky 失败
    registry: LspServerRegistry,
    settle_delay_ms: u64,
}
```

**Sticky 失败注释**（原文）：

> Startup failures are sticky for this manager generation. Repeating a model tool call must not create a spawn/timeout loop; a runtime rebuild gives the user a clean retry after fixing PATH/config.

翻译：**同一个 `(root, command)` 的启动失败被永久记忆**，模型重复调用不会反复起进程超时；只有 L2 装配层显式 `rebuild` 才能重试。

**Server 根目录探测** `server_root(workspace_root, path, markers)`（`manager.rs:80`）：

```rust
fn server_root(workspace_root: &Path, path: &Path, markers: &[String]) -> PathBuf {
    // 从 path 父目录向上爬，直到找到 Cargo.toml / package.json / go.mod 等
    // 或爬到 workspace_root 停止；返回那个 marker 所在目录
}
```

#### 3.1.6 LspServerRegistry：默认 7 个 server

`lsp/registry.rs:24-58` —— **开箱即用**的语言服务器映射：

| 扩展 | 二进制 | args | 根标记 |
|------|--------|------|--------|
| `.rs` | `rust-analyzer` | `[]` | `Cargo.toml` |
| `.ts` | `typescript-language-server` | `["--stdio"]` | `tsconfig.json`, `package.json` |
| `.tsx` | `typescript-language-server` | `["--stdio"]` | `tsconfig.json` |
| `.js` | `typescript-language-server` | `["--stdio"]` | `package.json` |
| `.py` | `pylsp` | `[]` | `pyproject.toml`, `setup.py` |
| `.go` | `gopls` | `["serve"]` | `go.mod` |
| `.java` | `jdtls` | `[]` | `pom.xml`, `build.gradle` |

`extension_to_language_id("rs")` 返回 `"rust"`、`"tsx"` 返回 `"typescriptreact"`（这是 LSP `textDocument/didOpen` 的 `languageId` 字段），未知扩展**返回自身**（fall-through 让自定义 server 能识别）。

#### 3.1.7 LspTool：四合一模型工具

`codeintel/lsp_tool.rs:1-100` —— **单个工具名 `lsp`，4 种 operation**：

```rust
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation { Definition, References, Hover, Diagnostics }

#[derive(Debug, Deserialize)]
struct Args {
    operation: Operation,
    file_path: String,
    line: Option<u32>,        // 1-based
    character: Option<u32>,   // 1-based
    severity: Option<String>, // "error" | "warning" | "all"
}
```

常量约束：
```rust
const MAX_LOCATIONS: usize = 200;
const MAX_HOVER_CHARS: usize = 12_000;
const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
```

**位置转换注释**（`lsp_tool.rs:97-104`）原文：

> Validate the complete operation before touching the filesystem or lazily starting an external language server. Tool schemas are guidance rather than a trust boundary: direct callers and imperfect model output can still pass values outside the advertised enum.

**模型视角的协议中性** —— 4 MB 文档上限（超过则不发起 LSP 请求），1-based 行/列对外（与编辑器一致），LSP 协议内转 0-based。

#### 3.1.8 tree-sitter 路径（symbol 层）

`codeintel/lang.rs:1-80` —— 12 个 grammar + 13 个 scm 查询文件：

| Lang | grammar crate | symbols_query | calls_query |
|------|---------------|---------------|-------------|
| Rust | `tree-sitter-rust` | `queries/rust.scm` | `queries/rust_calls.scm` |
| Python | `tree-sitter-python` | `python.scm` | `python_calls.scm` |
| JavaScript | `tree-sitter-javascript` | `javascript.scm` | `javascript_calls.scm` |
| TypeScript | `tree-sitter-typescript::LANGUAGE_TYPESCRIPT` | `typescript.scm` | `javascript_calls.scm` |
| Tsx | `tree-sitter-typescript::LANGUAGE_TSX` | `typescript.scm` | `javascript_calls.scm` |
| Go | `tree-sitter-go` | `go.scm` | `go_calls.scm` |
| Java | `tree-sitter-java` | `java.scm` | `java_calls.scm` |
| C | `tree-sitter-c` | `c.scm` | — |
| Cpp | `tree-sitter-cpp` | `cpp.scm` | — |
| CSharp | `tree-sitter-c_sharp` | `csharp.scm` | — |
| Html | `tree-sitter-html` | `html.scm` | — |
| Php | `tree-sitter-php::LANGUAGE_PHP` | `php.scm` | — |

**注释原文**：

> TSX = typed JSX → the TS symbol query matches the TSX grammar's node types (the JS query does NOT compile against the TSX grammar).

—— 这是 tree-sitter 实战中的经典坑：**JS 的 scm query 不能直接套到 TSX grammar**（node 类型不完全相同），atomcode 用同一个 `typescript.scm` 同时给 TypeScript 和 Tsx 用，是经过验证的写法。

#### 3.1.9 七件套一览

| 工具名 | 输入 | 后端 | 状态 |
|--------|------|------|------|
| `list_symbols` | `{file_path}` | tree-sitter AST | always on |
| `read_symbol` | `{file_path, name}` | tree-sitter AST | always on |
| `find_references` | `{symbol}` | CodeIndex（tree-sitter + 文本扫描） | always on |
| `trace_callers` | `{symbol, depth?}` | CodeIndex BFS | always on |
| `trace_callees` | `{symbol, depth?}` | CodeIndex BFS | always on |
| `trace_chain` | `{from, to, max_depth?}` | CodeIndex BFS | always on |
| `blast_radius` | `{file}` | CodeIndex 反向 BFS | always on |
| `file_dependencies` | `{file}` | CodeIndex 反向 | always on |
| `lsp` (4 op) | `{operation, file_path, line?, character?}` | LSP server | feature=lsp |
| `diagnostics` | `{file?, severity?}` | LSP server | feature=lsp |

**架构关键注释**（`codeintel/mod.rs:1-12`）：

> - **symbol layer** (single-file, STATELESS): `list_symbols` / `read_symbol` parse one file on demand — no shared state, nothing from the kernel `ToolContext` beyond `working_dir`.
> - **graph layer** (cross-file): `find_references` (whole-word text scan) plus `trace_callers` / `trace_callees` / `trace_chain` / `blast_radius` / `file_dependencies`, backed by a shared, lazily-built [`CodeIndex`] (the symbol layer's statelessness ends here — these tools HOLD an `Arc<CodeIndex>`).
>
> Deferred vs production: visibility inference; import-aware call resolution; background/incremental indexing (we rebuild on mtime change). Behind the opt-in `codeintel` cargo feature (12 grammars = heavy C compilation).

---

### 3.2 claudecode — TypeScript + vscode-jsonrpc，9 个 LSP 操作 + LRU 诊断去重

#### 3.2.1 模块拓扑

```
src/tools/LSPTool/                    # 模型工具层
├── LSPTool.ts            860 行   9 个 operation 路由 + 权限校验 + 格式化
├── schemas.ts            215 行   Zod discriminated union（按 operation 分支）
├── formatters.ts         592 行   9 种结果的字符串格式化
├── symbolContext.ts       90 行   UI 用：position → 单词提取（用于 hover label）
├── UI.tsx                227 行   React Ink 渲染
└── prompt.ts              21 行   系统提示词

src/services/lsp/                       # 客户端核心
├── config.ts              79 行   LSP server 配置加载
├── LSPClient.ts          447 行   vscode-jsonrpc 封装
├── LSPDiagnosticRegistry 386 行   LRU 去重 + cross-turn 抑制
├── LSPServerInstance.ts  511 行   单 server 实例 + 状态机 + 重试
├── LSPServerManager.ts   420 行   多 server 池 + 扩展名路由
├── manager.ts            289 行   工具层薄包装
├── passiveFeedback.ts    328 行   诊断作为附件回写对话
└── types.ts                       TypeScript 类型
```

**代码量**：LSP 相关 ~3500 行（工具 2005 + 服务 2460）。

#### 3.2.2 9 种 LSP operation

`src/tools/LSPTool/prompt.ts:1-21` 的完整列表：

```
- goToDefinition:    找定义位置
- findReferences:    找所有引用
- hover:             hover 文档/类型信息
- documentSymbol:    当前文件的符号列表
- workspaceSymbol:   全工作区搜索符号
- goToImplementation:接口/抽象方法实现
- prepareCallHierarchy:取出调用层次项
- incomingCalls:     谁调用了我
- outgoingCalls:     我调用了谁
```

**9 个中只有 4 个是 atomcode 默认实现**，另 5 个（`workspaceSymbol`/`goToImplementation`/`prepareCallHierarchy`/`incomingCalls`/`outgoingCalls`）是 claudecode 独有 —— 这反映 claudecode 把 LSP 当**首要代码理解工具**。

#### 3.2.3 Zod discriminated union schema

`src/tools/LSPTool/schemas.ts:1-80` 的设计 —— **每个 operation 一个独立 ZodObject**，外层用 `operation` 作为 discriminator：

```typescript
const goToDefinitionSchema = z.strictObject({
  operation: z.literal('goToDefinition'),
  filePath: z.string().describe('The absolute or relative path to the file'),
  line: z.number().int().positive().describe('The line number (1-based, as shown in editors)'),
  character: z.number().int().positive().describe('The character offset (1-based, as shown in editors)'),
});
// ... 8 more schemas
export const lspToolInputSchema = z.discriminatedUnion('operation', [...]);
```

注意 `strictObject` —— 拒绝额外字段（防止 LLM 误传 `languageId` 等 LSP 内部参数）。

#### 3.2.4 LSPClient 基于 vscode-jsonrpc

`src/services/lsp/LSPClient.ts:1-80` 的关键 import：

```typescript
import { createMessageConnection, StreamMessageReader, StreamMessageWriter } from 'vscode-jsonrpc/node'
import type { InitializeParams } from 'vscode-languageserver-protocol'
```

vscode-jsonrpc 提供：
- `createMessageConnection(StreamMessageReader, StreamMessageWriter)` 创建双向 JSON-RPC 通道
- `connection.onRequest(method, handler)` 注册客户端能力（如 `workspace/configuration`）
- `connection.sendRequest(method, params)` 发请求并 await 响应
- `connection.onNotification(method, handler)` 接诊断推送

**重试与降级** `LSPServerInstance.ts:11-22`：

```typescript
const LSP_ERROR_CONTENT_MODIFIED = -32801;
const MAX_RETRIES_FOR_TRANSIENT_ERRORS = 3;
const RETRY_BASE_DELAY_MS = 500;
// 实际退避：500ms, 1000ms, 2000ms
```

`-32801` 是 LSP 规范的"Content Modified"错误（rust-analyzer 索引中改了文件，server 拒绝响应）。claudecode 显式捕获并重试最多 3 次，**指数退避**。

#### 3.2.5 LRU 跨 turn 诊断去重

`src/services/lsp/LSPDiagnosticRegistry.ts:23-50` 的设计 —— 防止每个 turn 把同一文件的同一诊断重复塞给模型：

```typescript
const MAX_DIAGNOSTICS_PER_FILE = 10;   // 单文件最多 10 条
const MAX_TOTAL_DIAGNOSTICS = 30;       // 单次响应最多 30 条
const MAX_DELIVERED_FILES = 500;        // LRU 容量

const deliveredDiagnostics = new LRUCache<string, Set<string>>({
  max: MAX_DELIVERED_FILES,
});
```

诊断去重 key 用 `hash(message + severity + range)`，LRU 缓存已送达的 file → Set<key> 映射。同一文件同一诊断**第一次**出现在 5 秒延迟窗口内给到模型后，后续 turns 不再重复附。

#### 3.2.6 UI 增强：symbol 上下文提取

`src/tools/LSPTool/symbolContext.ts:14-90` —— 模型调用 LSP 时，TUI 头部显示"按行号反向找最近符号"：

```typescript
const symbolPattern = /[\w$'!]+|[+\-*/%&|^~<>=]+/g;
// Rust 生命周期 'a / 'static
// Rust 宏 macro_name!
// 算子 + - * 等
```

读文件**只读前 64KB**（注释：`Most LSP hover/goto targets are near recent edits; 64KB covers ~1000 lines of typical code`）。位置超出窗口返回 null，回退到显示 `line:char`。

#### 3.2.7 claudecode 的 IDE 集成路径

`src/utils/ide.ts` + `src/commands/ide/ide.tsx` —— claudecode 把 IDE 集成做成一个 `/ide` 斜杠命令屏（在 TUI 内的 modal），提供：
- `ide` 命令唤起 IDE 打开当前文件（`code <path>:<line>:<col>`）
- 在 IDE 内的 claudecode 扩展作为 RPC client，TUI 作为 RPC server

这与 opencode 的反向检测策略不同（见 3.4.7）。

---

### 3.3 deepseek-harness — Effect-式协议栈抽象 + JSON-RPC stdio transport

#### 3.3.1 模块拓扑

```
packages/lsp/                        # 一等公民：整个 lsp 工作区
├── lsp/                              # 抽象接口包（@deepseek-ai/dsh-lsp）
│   └── (类型、LspOperation 枚举、LspProvider、LspError 等)
└── lsp-stdio/                        # stdio transport 实现包（@deepseek-ai/dsh-lsp-stdio）
    ├── src/abort.ts         48 行   abortable + abortError
    ├── src/connection.ts   329 行   JSON-RPC endpoint + Pending map + onServerRequest
    ├── src/framing.ts      102 行   encodeMessage / MessageDecoder
    ├── src/host.ts         124 行   HostSource 抽象
    ├── src/index.ts        369 行   Provider 入口
    ├── src/instance.ts     347 行   LspInstance：单 server 实例 + 串行 query 队列
    ├── src/invariant.ts     30 行   不变量检查
    ├── src/protocol.ts      80 行   WireInitializeResult / WireServerCapabilities
    └── src/translate.ts    235 行   negotiatePositionEncoding / normalizeLocations 等
└── tool-lsp/                         # 模型工具包装（model-facing）
```

**代码量**：lsp 工作区 ~1700 行（stdio transport）。

#### 3.3.2 LspConnection：协议中立 + 单 file endpoint

`packages/lsp/lsp-stdio/src/connection.ts:1-80` 的模块注释原文：

```typescript
/**
 * A JSON-RPC endpoint over one language server spawned through the subprocess
 * capability. Owns id correlation, outbound requests/notifications, and inbound
 * server→client requests: it answers `workspace/configuration` from static
 * config, and rejects `workspace/applyEdit` (this host never applies edits or
 * runs commands). It caps stderr, surfaces framing/decoder failures as a
 * fatal close, and exposes tree-scoped termination through the handle so the
 * instance owns teardown; group/tree mechanics live in the subprocess
 * Service Provider.
 * @module @deepseek-ai/dsh-lsp-stdio/connection
 */
```

**与 atomcode 的关键差异**：
- deepseek-harness 显式拒绝 `workspace/applyEdit`（"this host never applies edits"）—— 与 atomcode `read-only` 设计一致，但通过**不发响应**实现（让 server 超时），atomcode 发 `{applied: false}` 显式拒绝
- 通过 `SubprocessHandle`（subprocess 包）抽象进程生命周期，不直接用 `child_process`
- `LspConnection` 只管**一个 endpoint**，**不**管多 server 池（池化在 Provider 层）

#### 3.3.3 LspInstance：串行 query 队列 + 一次性 teardown transaction

`packages/lsp/lsp-stdio/src/instance.ts:1-80` 的设计要点（注释原文）：

```typescript
/**
 * One language-server instance: a connection plus the initialize handshake, the serialized abortable
 * query queue, the transient `didOpen`→request→`didClose` lifecycle, and bounded teardown. One
 * instance owns one `(provider id, canonical workspace)` process. Queries serialize through a single
 * queue so a cancellation that fails to stop the server can terminate it without killing unrelated
 * work; distinct instances run in parallel.
 */
```

```typescript
private queue: Promise<unknown> = Promise.resolve();  // 串行链
private teardownPromise: Promise<void> | undefined;
private processClosed = false;
private readonly ready: Promise<void>;
```

**关键**：
1. **`queue` 单链** —— 所有 query `await queue = queue.then(() => doQuery())`，保证 `didOpen` 后才能 `definition`，`didClose` 后才能销毁文件
2. **abortable** —— `import { abortable } from './abort.ts'`，取消令牌取消**整个查询链**而不杀进程
3. **teardownPromise 单次触发** —— abort / failure / dispose 三个路径共用同一 teardown 事务，避免双关

#### 3.3.4 translate.ts：协议差异收敛

`translate.ts:1-235` 的关键函数（注释）：

```typescript
/** Negotiate utf-8/utf-16/utf-32 with the server's initialize result. */
negotiatePositionEncoding(...)
/** Convert vscode-languageserver-types Location to a normalized {file, line, column} 1-based form. */
normalizeLocations(...)
/** Return the LSP method name for a typed `LspOperation`. */
requestMethod(...)
/** True if the server's advertised capabilities include this operation. */
supportsOperation(...)
/** True if the server supports transient `didOpen`+request+`didClose` (rust-analyzer does). */
supportsTransientOpen(...)
```

`supportsTransientOpen` 是关键 —— `rust-analyzer` 等 server 支持「**临时打开文件** → 问 → **立刻关掉**」，无需把整个 workspace 喂给它；deepseek-harness 显式探测。

#### 3.3.5 Cordis 风格 Capability Seam

`packages/lsp/lsp-stdio/README.zh.md` + 工程根目录 `.agents/notes/implemented/architecture/2026-07-15-lsp-capability-seam.zh.md` —— deepseek-harness 把 LSP 实现成 Cordis 的**可插拔能力**：

> Capability Seam: 把 LSP 从"硬编码子进程"提升为"统一能力"。SubAgent 可在 Cordis Fiber 上注册额外的 LSP server（甚至第三方工具通过 `@deepseek-ai/dsh-tool-lsp` 暴露），而主 Agent 的 LSP Provider 负责单飞 + 池化 + 复用。

—— 这是把 LSP 与 Effect/Cordis 容器体系**深度整合**的范式，atomcode/claudecode 都是直接 spawn，deepseek-harness 是**通过 DI 容器注入**。

---

### 3.4 opencode — Effect DI + LayerNode + 32 个内置 LSP server + 反向 IDE 检测

#### 3.4.1 模块拓扑

```
packages/opencode/src/lsp/
├── lsp.ts              507 行   LSP.Service（Effect DI）+ 状态机
├── server.ts          1983 行   32 个 LSPServer.Info 实现（TypeScript/Rust/Python/...）
├── client.ts           650 行   LSPClient + vscode-jsonrpc + Capability registration
├── language.ts         121 行   100+ 扩展名 → language id
├── launch.ts            21 行   spawn 薄包装
└── diagnostic.ts        29 行   诊断结构

packages/opencode/src/ide/
└── index.ts             54 行   反向 IDE 检测 + 扩展市场安装

packages/opencode/src/cli/cmd/debug/lsp.ts        调试子命令
```

**代码量**：lsp 模块 ~3300 行 + ide 54 行。

#### 3.4.2 LSP Service：Effect DI + Layer

`packages/opencode/src/lsp/lsp.ts:1-120` 的核心接口定义（部分）：

```typescript
import { Layer, Context, Effect, Schema } from "effect"

export interface Interface {
  readonly init: () => Effect.Effect<void>
  readonly status: () => Effect.Effect<Status[]>
  readonly hasClients: (file: string) => Effect.Effect<boolean>
  readonly touchFile: (input: string, diagnostics?: "document" | "full") => Effect.Effect<void>
  readonly diagnostics: () => Effect.Effect<Record<string, LSPClient.Diagnostic[]>>
  readonly hover: (input: LocInput) => Effect.Effect<any>
  readonly definition: (input: LocInput) => Effect.Effect<any[]>
  readonly references: (input: LocInput) => Effect.Effect<any[]>
  readonly implementation: (input: LocInput) => Effect.Effect<any[]>
  readonly documentSymbol: (uri: string) => Effect.Effect<(DocumentSymbol | Symbol)[]>
  readonly workspaceSymbol: (query: string) => Effect.Effect<Symbol[]>
  readonly prepareCallHierarchy: (input: LocInput) => Effect.Effect<any[]>
  readonly incomingCalls: (input: LocInput) => Effect.Effect<any[]>
  readonly outgoingCalls: (input: LocInput) => Effect.Effect<any[]>
}
export class Service extends Context.Service<Service, Interface>()("@opencode/LSP") {}
```

**所有 LSP 操作都是 `Effect.Effect<T, E, R>`** —— 这是 opencode 第六轮 Effect DI 拓扑的具体落地（详见专题-第六轮-TUI与终端渲染管线深度对比）。注意 `hover`/`definition`/`references` 等 9 个方法与 claudecode 的 9 个 operation 一一对应。

#### 3.4.3 Schema 化 LSP 类型

`lsp.ts:7-55` 用 `Schema.Struct` 定义 `Range`/`Symbol`/`DocumentSymbol`/`Status`：

```typescript
const Position = Schema.Struct({ line: NonNegativeInt, character: NonNegativeInt, })
export const Range = Schema.Struct({ start: Position, end: Position, }).annotate({ identifier: "Range" })
export const Symbol = Schema.Struct({
  name: Schema.String,
  kind: NonNegativeInt,
  location: Schema.Struct({ uri: Schema.String, range: Range, }),
}).annotate({ identifier: "Symbol" })
export const DocumentSymbol = Schema.Struct({
  name: Schema.String, detail: Schema.optional(Schema.String),
  kind: NonNegativeInt, range: Range, selectionRange: Range,
}).annotate({ identifier: "DocumentSymbol" })
```

`@opencode-ai/schema/lsp-event` 包提供事件 schema，运行时可序列化。`SymbolKind` 枚举（1-26）覆盖 LSP 规范全部 SymbolKind。

#### 3.4.4 32 个内置 LSPServer.Info

`packages/opencode/src/lsp/server.ts:1-120` 的 Deno/TypeScript 实现片段（注释）：

```typescript
export const Deno: Info = {
  id: "deno",
  root: async (file, ctx) => { /* 向上找 deno.json/deno.jsonc */ },
  extensions: [".ts", ".tsx", ".js", ".jsx", ".mjs"],
  async spawn(root) {
    const deno = which("deno")
    if (!deno) return  // 优雅缺失
    return { process: spawn(deno, ["lsp"], { cwd: root }) }
  },
}
export const Typescript: Info = {
  id: "typescript",
  root: NearestRoot(
    ["package-lock.json", "bun.lockb", "bun.lock", "pnpm-lock.yaml", "yarn.lock"],
    ["deno.json", "deno.jsonc"],  // ← exclude
  ),
}
```

完整 32 个 server（来自 `packages/web/src/content/docs/lsp.mdx`）：

| Server | 扩展名 | 启动方式 |
|--------|--------|----------|
| astro | .astro | auto-install |
| bash | .sh/.bash/.zsh/.ksh | auto-install bash-language-server |
| clangd | .c/.cpp/.cc/.cxx/.h/.hpp | auto-install |
| csharp | .cs/.csx | .NET SDK |
| clojure-lsp | .clj/.cljs/.cljc/.edn | system |
| dart | .dart | system |
| deno | .ts/.tsx/.js/.jsx/.mjs | system + auto-detect deno.json |
| elixir-ls | .ex/.exs | system |
| eslint | .ts/.tsx/.js/.jsx/.mjs/.cjs/.mts/.cts/.vue | npm dep |
| fsharp | .fs/.fsi/.fsx/.fsscript | .NET SDK |
| gleam | .gleam | system |
| gopls | .go | system |
| hls | .hs/.lhs | system |
| jdtls | .java | Java 21+ |
| julials | .jl | system |
| kotlin-ls | .kt/.kts | auto-install |
| lua-ls | .lua | auto-install |
| nixd | .nix | system |
| ocaml-lsp | .ml/.mli | system |
| oxlint | .ts/.tsx/.js/.jsx/.mjs/.cjs/.mts/.cts/.vue/.astro/.svelte | npm dep |
| php intelephense | .php | auto-install |
| prisma | .prisma | system |
| pyright | .py/.pyi | npm dep（**默认**，不是 pylsp） |
| razor | .razor/.cshtml | .NET SDK + VSCode C# |
| ruby-lsp (rubocop) | .rb/.rake/.gemspec/.ru | ruby + gem |
| rust | .rs | rust-analyzer |
| sourcekit-lsp | .swift/.objc/.objcpp | swift / xcode |
| svelte | .svelte | auto-install |
| terraform | .tf/.tfvars | GitHub release |
| tinymist | .typ/.typc | GitHub release |
| typescript | .ts/.tsx/.js/.jsx/.mjs/.cjs/.mts/.cts | npm dep |
| vue | .vue | auto-install |
| yaml-ls | .yaml/.yml | Red Hat npm |
| zls | .zig/.zon | system |

—— 比 atomcode 的 7 个多 4 倍，比 claudecode（一般 ~12 个）多 2 倍。

#### 3.4.5 启动调度：extension → server → root → spawn

`lsp.ts:166-260` 的 `getClients` 实现（节选）：

```typescript
for (const server of Object.values(s.servers)) {
  if (server.extensions.length && !server.extensions.includes(extension)) continue
  const root = await server.root(file, ctx)
  if (!root) continue
  if (s.broken.has(root + server.id)) continue  // ← sticky broken
  
  async function schedule(server, root, key) {
    const handle = await server.spawn(root, ctx, flags).then(v => {
      if (!v) s.broken.add(key); return v
    }).catch(() => { s.broken.add(key); return undefined })
    // ...
  }
  schedule(server, root, `${root}|${server.id}`)
}
```

**State 字段**：
```typescript
interface State {
  clients: LSPClient.Info[]
  servers: Record<string, LSPServer.Info>
  broken: Set<string>                                    // sticky broken
  spawning: Map<string, Promise<LSPClient.Info | undefined>>  // 单飞
}
```

`spawning` Map 实现"同一 `(root, server.id)` 同时只能有一个启动协程"（避免竞争）。

#### 3.4.6 实验性 server 切换

`lsp.ts:62-72` 的 `filterExperimentalServers`：

```typescript
const filterExperimentalServers = (servers, flags) => {
  if (flags.experimentalLspTy) {
    if (servers["pyright"]) delete servers["pyright"]
  } else {
    if (servers["ty"]) delete servers["ty"]
  }
}
```

—— 通过 `RuntimeFlags` 在 `pyright`（默认）与 `ty`（实验性）之间切换。这是 opencode 的 A/B 试验模式。

#### 3.4.7 反向 IDE 检测 + 扩展市场安装

`packages/opencode/src/ide/index.ts:1-54` 的核心：

```typescript
const SUPPORTED_IDES = [
  { name: "Windsurf" as const, cmd: "windsurf" },
  { name: "Visual Studio Code - Insiders" as const, cmd: "code-insiders" },
  { name: "Visual Studio Code" as const, cmd: "code" },
  { name: "Cursor" as const, cmd: "cursor" },
  { name: "VSCodium" as const, cmd: "codium" },
]

export function ide() {
  if (process.env["TERM_PROGRAM"] === "vscode") {
    const v = process.env["GIT_ASKPASS"]
    for (const ide of SUPPORTED_IDES) {
      if (v?.includes(ide.name)) return ide.name
    }
  }
  return "unknown"
}

export function alreadyInstalled() {
  return process.env["OPENCODE_CALLER"] === "vscode" || 
         process.env["OPENCODE_CALLER"] === "vscode-insiders"
}

export async function install(ide) {
  const cmd = SUPPORTED_IDES.find(i => i.name === ide)?.cmd
  if (!cmd) throw new Error(`Unknown IDE: ${ide}`)
  const p = await Process.run([cmd, "--install-extension", "sst-dev.opencode"], { nothrow: true })
  if (p.code !== 0) throw new InstallFailedError({ stderr: p.stderr.toString() })
  if (stdout.includes("already installed")) throw new AlreadyInstalledError({})
}
```

**反向检测的 2 个启发式**：
1. `TERM_PROGRAM=vscode` + `GIT_ASKPASS` 含 `"Windsurf"`/`"Visual Studio Code"` 等子串
2. `OPENCODE_CALLER` env var 由 IDE 扩展注入（`vscode`/`vscode-insiders`）

**自动安装** —— 一行 `code --install-extension sst-dev.opencode`，无需用户手动操作。

#### 3.4.8 Effect 实例生命周期

`lsp.ts:175-195`：

```typescript
const s: State = { /* ... */ }
yield* Effect.addFinalizer(() =>
  Effect.promise(async () => {
    await Promise.all(s.clients.map((client) => client.shutdown()))
  }),
)
```

`InstanceState.make<State>` 创建的 state 在 Effect 容器销毁时（用户退出会话、IDE 断连）自动 finalizer 跑 `client.shutdown()` —— 这是 Effect 的 `Layer` 抽象带来的"资源随生命周期"特性。

---

### 3.5 openclaw — tree-sitter bash 做命令解释（仅此一处）

#### 3.5.1 模块位置

```
src/infra/command-explainer/
├── tree-sitter-runtime.ts        tree-sitter bash 加载 + 进度回调
├── extract.ts                    从 tree-sitter 树提取 commands
├── extract.test.ts               单元测试
└── explain.lazy.test.ts          lazy 集成测试
```

#### 3.5.2 web-tree-sitter 加载与超时

`tree-sitter-runtime.ts:1-80` 的关键注释：

```typescript
import * as TreeSitter from "web-tree-sitter";

const MAX_COMMAND_EXPLANATION_SOURCE_CHARS = 128 * 1024;
const MAX_COMMAND_EXPLANATION_PARSE_MS = 500;

async function loadParser(): Promise<TreeSitter.Parser> {
  await TreeSitter.Parser.init();
  const language = await TreeSitter.Language.load(
    require.resolve("tree-sitter-bash/tree-sitter-bash.wasm"),
  );
  return new TreeSitter.Parser().setLanguage(language);
}
```

**两条护城河**：
1. **128KB 源码上限** —— 超出直接拒绝（避免恶意/超大命令耗资源）
2. **500ms 解析超时** —— 通过 `Parser.parse(source, null, { progressCallback })` 的回调实现，回调返回 `true` 让 parser 早停

```typescript
const tree = parser.parse(source, null, {
  progressCallback: () => {
    timedOut = performance.now() > deadlineMs;
    return timedOut;
  },
});
if (!tree) {
  parser.reset();
  if (timedOut) throw new Error(`tree-sitter-bash timed out after ${MAX_COMMAND_EXPLANATION_PARSE_MS}ms`);
}
```

**lazy 加载 + 失败重置**：

```typescript
parserPromise ??= loadParser().catch((error: unknown) => {
  parserPromise = null;  // 重置 cache，下次重试
  throw error;
});
```

—— `parserPromise = null` 让瞬时的 wasm 加载失败不污染后续所有调用。

#### 3.5.3 与命令解释的协作

`extract.ts` 用 tree-sitter bash 语法树识别：`Pipes` / `&&` / `||` / `;` / 子 shell / 命令替换 / 环境变量赋值 —— 用于在用户输入复杂 bash 命令时**生成人类可读的解释**（"Run X; if succeeds, run Y; pipe to Z"），并在工具权限判定时提供更精细的语义。

—— openclaw 没用 LSP，纯 tree-sitter bash 单 grammar，目标是**安全/解释**，不是代码理解。

---

### 3.6 pi — 零 LSP / 零 CodeIntel（最小化设计）

```bash
grep -rn "lsp\|LSP\|tree-sitter" /usr/local/LsmGitOpenSource/pi/packages --include='*.ts' | head
# 无匹配
```

pi 的设计哲学是**「编码 agent 最小可行集」**：模型自己推理 + 文件工具 + Bash + 一个 Ed/Read/Patch 内置。不引入任何 IDE 集成或语言服务器。优点是**极小依赖、极快启动**，代价是大型项目重构需要模型自行用 grep + read 推理。

**对 laew 的启示**：P0 阶段可以借鉴 pi 的极简风格 —— 用 tree-sitter 做最低限度的符号提取（不需要起 LSP server），等模型用熟了再上 LSP。

---

### 3.7 undici — 非 Agent 工程，HTTP 客户端与 LSP 无关

undici 是 Node.js 官方 HTTP 客户端（详见专题-第七轮-Anthropic 与 OpenAI 协议调用真实实现对比）。它**不是 Agent**，本专题不涉及它的 LSP 实现（也不存在）。提一句仅作完整性说明 —— 之前几轮深挖把它列为研究对象是研究 HTTP 客户端视角，与 LSP/IDE 集成无任何交集。

---

### 3.8 hermes-agent — Python 实现，agent/lsp/ 完整包 + 与 Write 工具深度集成

#### 3.8.1 模块拓扑

```
agent/lsp/
├── __init__.py
├── client.py            LSP 客户端（process spawn + JSON-RPC）
├── cli.py               命令行入口（启动/停止/查询 LSP server）
├── eventlog.py          事件日志（用于事后分析）
├── install.py           自动下载 LSP server 二进制（rust-analyzer 等）
├── manager.py           多 server 池 + 扩展名路由
├── protocol.py          LSP 协议常量与类型
├── range_shift.py       当 LSP 返回 ranges 而文本已被 patch 修改时，位移修复
├── reporter.py          诊断报告生成（XML/JSON 双格式）
├── servers.py           内置 server 配置（rust-analyzer, pylsp, gopls, ...）
└── workspace.py         工作区文件夹发现

tools/patch_parser.py    与 Write 工具深度集成：每个 _apply_* 返回 (success, diff, lsp_diagnostics, lint)
```

#### 3.8.2 与 Write 工具的双向耦合（独特设计）

`tools/patch_parser.py:231-243`：

```python
# Every _apply_* returns (success, diff_or_error, lsp_diagnostics, lint_result).
def _record(self, success: bool, diff: str, result: WriteResult) -> PatchApplyOutcome:
    """Outcome of a write: its error, else success with LSP/lint propagated from the WriteResult."""
    return True, diff, getattr(result, "lsp_diagnostics", None), getattr(result, "lint", None)
```

每次写文件后，hermes-agent **主动查询 LSP server 拿 diagnostics**，把诊断**内联在 patch 输出的下面**返回给模型：

```python
# tools/patch_parser.py:269-295
lsp_blocks: List[str] = []
for op, file_ops in batched:
    ok, payload, lsp, lint = handler(op, file_ops)
    if lsp:
        lsp_blocks.append(lsp)  # 每个文件一块

# Join them — each LSP block has its own <diagnostics file="..."> header.
return PatchApplyOutcome(
    lint=lint_results or None,
    lsp_diagnostics="\n\n".join(lsp_blocks) or None
)
```

—— 这种"**写完立刻检查**"的反馈循环，是 hermes-agent 区别于其他工程的核心理念：模型看到的 patch output **直接包含新代码的 diagnostics**，下一轮 LLM 推理时已经被告知问题。

#### 3.8.3 install.py：自动下载 LSP 二进制

`agent/lsp/install.py`（hermes-agent 命名空间）的存在意味着 hermes-agent **自己负责获取** rust-analyzer 等二进制（不依赖用户在 PATH 安装）。这是 Hermes"开箱即用"的代价 —— 工程文件变大，但用户无需 `apt install rust-analyzer`。

#### 3.8.4 range_shift.py：诊断与 patch 的位移同步

当 LSP 返回 range（如 error from line 10 to 12），但用户/模型随后又 patch 了文件，range 需要**重新计算**（按 patch 偏移调整）。`range_shift.py` 实现了这个功能 —— 假设你改了第 5 行新增 3 行，原来在第 10 行的诊断现在在第 13 行。

这是 7 个工程里**独一份** —— 其他工程遇到这种 case 都直接丢弃诊断。

---

## 4. 横向对比大表（7 工程 × 8 维度）

### 4.1 维度矩阵

| 维度 | atomcode | claudecode | deepseek-harness | opencode | openclaw | pi | hermes-agent |
|------|----------|------------|------------------|----------|----------|----|--------------|
| **LSP 客户端协议栈** | 自研 Content-Length + 自研 JSON-RPC ~92 行 | `vscode-jsonrpc/node` | 自研 framing.ts + connection.ts | `vscode-jsonrpc/node` | 无 | 无 | 自研 Python + JSON-RPC |
| **LSP server 二进制来源** | 依赖系统 PATH | 依赖系统 PATH | 依赖系统 PATH | 系统 PATH + **auto-install** | N/A | N/A | **自下载**（`install.py`） |
| **内置 server 数量** | 7 个 | ~12 个 | 取决于 Provider | **32 个** | 0 | 0 | 5+（rust/pylsp/gopls/...） |
| **Server 根目录解析** | `server_root(workspace, path, markers)` 向上爬升 | `Filesystem.up({targets, start, stop})` | LSP Provider 注入 `canonical workspace` | `NearestRoot(include, exclude)` + `StrictNearestRoot` | N/A | N/A | `workspace.py` 文件夹发现 |
| **Sticky 失败处理** | `unavailable: HashMap<ClientKey, String>` | `restartCount` 计数器 | `processClosed` flag | `broken: Set<string>` + `spawning: Map<key, Promise>` 单飞 | N/A | N/A | `eventlog.py` 记录 |
| **模型工具抽象** | 4 op 单 `lsp` 工具 + 8 个 CodeIntel 工具（feature=lsp 分开） | 9 op 单 `LSP` 工具（discriminated union） | 通过 `@deepseek-ai/dsh-tool-lsp` 暴露 | 9 op 通过 Effect Service（直接函数） | N/A | N/A | 与 patch_parser 集成 |
| **tree-sitter 使用** | **12 个 grammar** + symbols/calls 双 query | 1 grammar（bash）做 `treeSitterAnalysis.ts` | 未发现 | 未发现 | 1 grammar（bash）做命令解释 | 无 | 未发现 |
| **CodeIntel 工具数量** | **8 个**（`list_symbols`/`read_symbol`/`find_references`/`trace_callers`/`trace_callees`/`trace_chain`/`blast_radius`/`file_dependencies`）| 0 | 0 | 0 | 0 | 0 | 0 |
| **IDE 反向检测** | 无 | `src/utils/ide.ts` + `/ide` 斜杠命令 | 无 | `ide()` 函数 + `TERMINFO=vscode` + `OPENCODE_CALLER` env | 无 | 无 | 无 |
| **IDE 扩展安装** | 无 | 无 | 无 | `code --install-extension sst-dev.opencode` 自动 | 无 | 无 | 无 |
| **Dev Container 集成** | 无 | 无 | 无 | 无 | 无 | 无 | 无 |
| **semantic_tokens** | 无 | 无 | 无 | 无 | 无 | 无 | 无 |
| **inlay_hints** | 无 | 无 | 无 | 无 | 无 | 无 | 无 |
| **workspace/executeCommand** | 间接（Bash 工具跑任意命令） | 间接 | 间接 | 间接 | 间接 | 间接 | 间接 |
| **workspace/symbol** | 无（CodeIndex 内置 name → ids 索引代替） | 有 | 通过 Provider | 有 | 无 | 无 | 无 |
| **callHierarchy (incoming/outgoing)** | 自建（trace_callers/trace_callees） | 有（5 个 LSP op） | 通过 Provider | 有 | 无 | 无 | 无 |
| **位置编码协商** | `PositionEncoding::{Utf8, Utf16, Utf32}` enum | `vscode-jsonrpc` 默认 | `negotiatePositionEncoding()` 函数 | 默认（vscode-jsonrpc） | N/A | N/A | 未发现 |
| **transient didOpen 支持探测** | 通过 `didOpen` → `didClose` 完整生命周期 | 同 | `supportsTransientOpen()` 显式探测 | 同 | N/A | N/A | 同 |
| **workspace/applyEdit 拒绝** | `{applied: false, failureReason: "read-only"}` 显式 | 通过 vscode-jsonrpc handler 拒绝 | "不响应"（让 server 超时） | 通过 vscode-jsonrpc handler 拒绝 | N/A | N/A | 未发现 |
| **诊断注入到对话的策略** | 显式 LspTool operation=`diagnostics` 触发 | `LSPDiagnosticRegistry` 异步 push + LRU 去重 | Provider 层 | `LSPEvent` schema 化推送 | N/A | N/A | 与 patch output 内联 |
| **Write 后立即查 diagnostics** | 否 | 否 | 否 | 否 | N/A | N/A | **是**（`patch_parser.py`） |
| **错误重试** | 无（依赖 30s timeout） | `-32801 ContentModified` 重试 3 次指数退避 | `abortable` + `deadline` | 通过 `LSPClient.create().catch` 重试 | N/A | N/A | 未发现 |
| **位置坐标系** | 模型 1-based ↔ LSP 0-based | 模型 1-based ↔ LSP 0-based | 模型 1-based ↔ LSP 0-based | 模型 1-based ↔ LSP 0-based | N/A | N/A | 未发现 |
| **feature gating** | `#[cfg(feature = "lsp")]` 区分 CodeIntel 与 LSP | 无（运行时） | 包分立（`lsp` 抽象 + `lsp-stdio` 实现 + `tool-lsp` 模型） | 无 | N/A | N/A | 无 |
| **Effect DI 集成** | 无（async-trait + Arc） | 无 | **有**（Cordis Provider） | **有**（Effect Layer） | N/A | N/A | 无 |
| **range shift（patch 后修复）** | 无 | 无 | 无 | 无 | N/A | N/A | **有**（`range_shift.py`） |

### 4.2 三个"独此一家"的事实清单

| 独此一家的能力 | 工程 | 代码位置 |
|----------------|------|----------|
| **8 个 CodeIntel 工具**（symbol+graph+LSP 三合一） | atomcode | `crates/atomcode-capabilities/src/codeintel/mod.rs:78-91` |
| **传输无关 LspClient**（用 `tokio::io::duplex()` 单测） | atomcode | `codeintel/lsp/client.rs:1-10` |
| **Effect DI 集成的 LSP Service** | opencode | `packages/opencode/src/lsp/lsp.ts:107-135` |
| **32 个内置 server + auto-install** | opencode | `packages/opencode/src/lsp/server.ts:1983 行` |
| **`OPENCODE_CALLER` 反向 IDE 检测 + 扩展市场安装** | opencode | `packages/opencode/src/ide/index.ts:38-50` |
| **`supportsTransientOpen` 探测** | deepseek-harness | `packages/lsp/lsp-stdio/src/translate.ts` |
| **9 种 LSP operation discriminated union** | claudecode | `src/tools/LSPTool/schemas.ts:9-215` |
| **LRU 跨 turn 诊断去重** | claudecode | `src/services/lsp/LSPDiagnosticRegistry.ts:23-50` |
| **`-32801` 重试 + 指数退避** | claudecode | `src/services/lsp/LSPServerInstance.ts:11-22` |
| **Write 后立即内联 diagnostics** | hermes-agent | `tools/patch_parser.py:269-295` |
| **range_shift（patch 后修复 LSP range）** | hermes-agent | `agent/lsp/range_shift.py` |
| **LSP 二进制自下载** | hermes-agent | `agent/lsp/install.py` |
| **tree-sitter bash 超时回调** | openclaw | `src/infra/command-explainer/tree-sitter-runtime.ts:46-58` |
| **Feature-gated LSP**（`#[cfg(feature = "lsp")]`） | atomcode | `codeintel/mod.rs:33-49` |

---

## 5. LSP vs tree-sitter vs ctags 三种方案对比

### 5.1 能力矩阵

| 能力 | LSP | tree-sitter | ctags / grep |
|------|-----|-------------|--------------|
| 启动时间 | 2-5s/语言 | <100ms/文件 | <100ms |
| 内存占用 | 200-500MB/server | 50MB + N×grammar | 几乎零 |
| 语义精度 | **编译期**（类型、trait、重载） | 语法树（无类型） | 文本（无结构） |
| 跨文件 | ✓ | ✗（需自建索引） | ✗ |
| 多语言 | 每个 server 一个二进制 | 每个 grammar 一个 wasm | 通用 |
| 跳定义 | ✓ | ✓ | ✗ |
| 跳引用 | ✓ | △（精确度差） | ✗ |
| Hover 类型 | ✓ | ✗ | ✗ |
| 文档符号 | ✓ | ✓（自建） | △（粗糙） |
| 工作区符号 | ✓ | △（自建 CodeIndex） | △（grep） |
| 诊断（diagnostics） | ✓ | ✗（需独立 linter） | ✗ |
| 调用层次 | ✓（专用 method） | △（自建） | ✗ |
| 重命名/重构 | ✓（专用 method） | ✗ | △（sed/perl） |
| 语义高亮 | △（semantic_tokens） | ✓（highlights.scm） | ✗ |
| 内联提示 | △（inlay_hints） | ✗ | ✗ |
| 复杂度（实现） | 高（协议+子进程） | 中（AST 查询） | 低（正则） |
| Rust crate | `lsp-types` / `tower-lsp` | `tree-sitter` + `tree-sitter-*` | `grep` / `regex` |

### 5.2 7 个工程的选择

| 工程 | 选择 | 原因 |
|------|------|------|
| atomcode | **tree-sitter + LSP 双轨** | CodeIntel 八件套优先 tree-sitter，LSP 是 feature=lsp 可选 |
| claudecode | **LSP only** | 9 op LSP 工具覆盖所有代码理解需求 |
| deepseek-harness | **LSP only + DI** | Effect/Cordis 容器，LSP Provider |
| opencode | **LSP only** | 32 server 覆盖广度，不自己写 AST |
| openclaw | **tree-sitter bash only** | 只为 bash 命令解释，不需要通用代码理解 |
| pi | **无** | 极简设计，让模型自己用 grep + read |
| hermes-agent | **LSP only + 自动下载** | Write 后立即反馈 |

### 5.3 laew 应该选什么？

**P0（v0.3）**：先 tree-sitter 单文件符号（`list_symbols` + `read_symbol`），零依赖、零进程、纯 Rust。crate 候选：`tree-sitter` + `tree-sitter-rust` + `tree-sitter-typescript` + `tree-sitter-python` + `tree-sitter-go`（4 个 grammar 编译成本可控）。

**P1（v0.4）**：上 LSP JSON-RPC stdio（`lsp-types` + `tower-lsp-server` 仅用于测试；客户端自研 100 行 = `jsonrpc.rs` + `client.rs`）。仅支持 rust-analyzer 一个 server（laew 当前主要使用 Rust），其它语言用户自配。

**P2（v0.5+）**：跨文件 CodeIndex（BFS 反向依赖）、IDE 反向检测（`OPENCODE_CALLER` env var + `--install-extension laew-claude-code` 风格）、Dev Container（`.devcontainer/devcontainer.json` 解析为 LSP 启动参数）。

---

## 6. 共性模式

### 6.1 模式 A：Content-Length 帧 + AtomicU64 id + HashMap<id, oneshot>

5/7 工程用这种最小 JSON-RPC 实现：

```
Header:  Content-Length: N\r\n\r\n
Body:    {"jsonrpc":"2.0","id":1,"method":"textDocument/definition","params":{...}}
Response:{"jsonrpc":"2.0","id":1,"result":[...]}
```

`atomcode/codeintel/lsp/jsonrpc.rs:20-46` + `client.rs` 用 `AtomicU64` 自增 id，`Mutex<HashMap<u64, oneshot::Sender>>` 关联响应。`deepseek-harness` 用类似模式（`packages/lsp/lsp-stdio/src/framing.ts:1-102` + `connection.ts:1-329`）。

### 6.2 模式 B：扩展名 → 二进制 + args + root_markers 注册表

7/7 工程都有某种形式的 server 注册表。差异在**根目录探测策略**：

| 工程 | 探测函数 | 策略 |
|------|----------|------|
| atomcode | `server_root(workspace, path, markers)` | 向上爬 marker 文件 |
| claudecode | `Filesystem.up({targets, start, stop})` | 异步生成器向上爬 |
| opencode | `NearestRoot(include, exclude)` + `StrictNearestRoot` | 同上，但支持 exclude 模式 |
| deepseek-harness | LSP Provider 注入 | 不做探测，由文件系统 Provider 提供 canonical workspace |

### 6.3 模式 C：Sticky 失败 + 单飞 spawn

避免模型重试导致启动风暴：

- atomcode：`unavailable: HashMap<ClientKey, String>`
- opencode：`broken: Set<string>` + `spawning: Map<key, Promise>`（同时只能有一个启动）
- claudecode：`restartCount` 计数器 + state machine（stopping → starting → running）

### 6.4 模式 D：拒绝 workspace/applyEdit（read-only 客户端）

任何 Agent 内嵌 LSP 客户端**必须**应答 7 类服务端请求，其中 `workspace/applyEdit` 必须**拒绝**（不要让 LSP server 改文件）。

实现差异：
- atomcode：`{applied: false, failureReason: "read-only"}` 显式
- claudecode：handler 抛错，让 vscode-jsonrpc 转 error response
- deepseek-harness：不响应，让 server 超时
- opencode：handler 抛错（同 claudecode）

### 6.5 模式 E：1-based 模型坐标 ↔ 0-based LSP 坐标

所有工程对外给模型的都是**1-based**（与编辑器一致），内部转 **0-based** 与 LSP 通信。

`atomcode/codeintel/lsp_tool.rs:97` + `claudecode/src/tools/LSPTool/LSPTool.ts:51-58` + `deepseek-harness/packages/lsp/lsp-stdio/src/translate.ts:normalizeLocations()` 都做这件事。

### 6.6 模式 F：lifecycle `didOpen` → query → `didClose`（transient）

deepseek-harness 的 `supportsTransientOpen()` 函数探测 server 是否支持**临时打开文件**：避免把整个 workspace 喂给 server。仅在 rust-analyzer 等少数 server 上有效，jdtls 反而要求完整 workspace。

### 6.7 模式 G：feature gating / 包分立

- atomcode 用 Rust `#[cfg(feature = "lsp")]` 把 LSP 子模块与 CodeIntel 主体分离
- deepseek-harness 用包分立：`@deepseek-ai/dsh-lsp`（抽象）+ `@deepseek-ai/dsh-lsp-stdio`（stdio transport）+ `@deepseek-ai/dsh-tool-lsp`（模型工具）

—— 这种"实现可换"的解耦让 laew 的 P0 阶段可以**只引入 stdio transport**，未来加 socket/WebSocket transport 不动工具层。

---

## 7. 对 laew 的 P0/P1/P2 路线图

### 7.1 现状盘点（2026-09-07）

`laew` 当前工具集：**Bash、Read、Write**（详见 CLAUDE.md）。零 LSP 客户端、零 tree-sitter、零 IDE 适配、零 Dev Container 集成、零 CodeIntel 工具。

### 7.2 P0（v0.3，预计 2 周）— tree-sitter 符号提取

**目标**：在不引入任何外部二进制的前提下，让模型能"读懂"源码结构。

**新增 crate 依赖**（`Cargo.toml`）：
```toml
tree-sitter = "0.24"
tree-sitter-rust = "0.23"
tree-sitter-typescript = "0.23"  # 提供 LANGUAGE_TYPESCRIPT + LANGUAGE_TSX
tree-sitter-python = "0.23"
tree-sitter-go = "0.23"
```

**新增模块**：
```
src/agent/tools/
├── list_symbols.rs     "list_symbols" Tool 实现（参考 atomcode list_symbols.rs:1-60）
├── read_symbol.rs      "read_symbol" Tool 实现
└── symbols.rs          tree-sitter 提取逻辑（参考 atomcode symbols.rs:1-100）
src/agent/codeintel/
└── mod.rs              CodeIntel 模块出口 + 注册到 builtin_registry()
```

**Schema 设计**：
```json
list_symbols: { "file_path": "string, required" }
read_symbol:  { "file_path": "string, required", "name": "string, required" }
```

**Acceptance**：
- `cargo build --release` 成功
- `cargo test list_symbols` 通过
- `./laew -p "用 list_symbols 读 src/main.rs"` 输出符号列表
- `./laew -p "用 read_symbol 找 main 函数"` 输出签名

### 7.3 P1（v0.4，预计 3 周）— LSP JSON-RPC stdio + 单 server（rust-analyzer）

**目标**：让模型能跨文件跳定义、查引用、读 hover 文档。

**新增 crate 依赖**：
```toml
lsp-types = "0.95"  # 协议类型
tokio = { version = "1", features = ["full"] }
```

**新增模块**：
```
src/agent/lsp/
├── mod.rs           入口
├── jsonrpc.rs       Content-Length 编解码（参考 atomcode jsonrpc.rs:1-92）
├── client.rs        传输无关 LspClient（参考 atomcode client.rs:1-200，先实现核心 5 个方法）
├── manager.rs       LspManager + sticky 失败（参考 atomcode manager.rs:1-80）
├── registry.rs      LspServerRegistry 默认支持 rust-analyzer 一种
└── types.rs         Diagnostic / Location 协议中立类型
src/agent/tools/
└── lsp.rs           "lsp" 工具（参考 atomcode lsp_tool.rs:1-100）
```

**Acceptance**：
- `./laew -p "用 lsp 工具找 main 函数的定义"` 返回 file:line:col
- 启动 rust-analyzer 失败时返回 sticky 错误（不重试）
- 4 MB 文档上限生效（>4MB 文件报错而非请求 LSP）

### 7.4 P2（v0.5+，预计 4 周）— 跨文件 CodeIndex + IDE 适配 + Dev Container

**P2.1 CodeIndex + 4 个图工具**
- 新增 `find_references`、`trace_callers`、`trace_callees`、`blast_radius`（参考 atomcode graph.rs + index.rs + blast_radius.rs）
- 用 `Arc<CodeIndex>` 单例 + mtime 检测增量更新
- 工具注册到 `builtin_registry()`

**P2.2 反向 IDE 检测**
- 新增 `src/ide/` 模块（参考 opencode ide/index.ts）
- 检测 `TERM_PROGRAM=claude`/`OPENCODE_CALLER=vscode` 等 env var
- 暂不实现扩展市场安装（laew 不是 IDE 扩展）

**P2.3 Dev Container 集成**
- 新增 `src/devcontainer/` 模块
- 解析 `.devcontainer/devcontainer.json` 的 `customizations.vscode.extensions`（参考 VSCode 规范）
- LSP 二进制在容器内 spawn 时把 `cwd` 设为 container workspace，`env` 注入 `${containerEnv}`

**P2.4 多语言 LSP 注册**
- 在 `LspServerRegistry::with_defaults()` 加入 typescript-language-server、pyright、gopls（参考 atomcode registry.rs:24-58）
- 每个扩展对应不同 server，server 之间不共享进程

### 7.5 不要做的事（negative 路线图）

- **不要实现 semantic_tokens**：Agent 场景下语义高亮毫无价值（不是 IDE）
- **不要实现 inlay_hints**：同上
- **不要把 workspace/executeCommand 暴露给模型**：相当于把任意 shell 命令执行暴露给 LSP server（攻击面），通过 Bash 工具跑命令更直接
- **不要实现 Lightbulb（codeAction）**：模型自己推理，不需要"快速修复"建议
- **不要做 LSP server 自下载**：hermes-agent 的 `install.py` 让工程文件巨大；laew 用户可以 `apt install rust-analyzer` 自决

---

## 8. 附录：关键代码路径速查表

### 8.1 atomcode

| 路径 | 行数 | 内容 |
|------|------|------|
| `crates/atomcode-capabilities/src/codeintel/mod.rs` | 216 | CodeIntel 模块入口 + 8 个工具注册 |
| `crates/atomcode-capabilities/src/codeintel/lang.rs` | 165 | Lang 枚举 + 12 个 tree-sitter grammar |
| `crates/atomcode-capabilities/src/codeintel/symbols.rs` | 294 | 单文件 tree-sitter 符号提取 |
| `crates/atomcode-capabilities/src/codeintel/list_symbols.rs` | 176 | `list_symbols` Tool |
| `crates/atomcode-capabilities/src/codeintel/read_symbol.rs` | 200 | `read_symbol` Tool |
| `crates/atomcode-capabilities/src/codeintel/graph.rs` | 356 | CodeGraph / Edge / SymbolKind |
| `crates/atomcode-capabilities/src/codeintel/index.rs` | 520 | CodeIndex + build_graph |
| `crates/atomcode-capabilities/src/codeintel/find_references.rs` | 227 | `find_references` Tool |
| `crates/atomcode-capabilities/src/codeintel/trace_callers.rs` | 151 | `trace_callers` Tool |
| `crates/atomcode-capabilities/src/codeintel/trace_callees.rs` | 134 | `trace_callees` Tool |
| `crates/atomcode-capabilities/src/codeintel/blast_radius.rs` | 172 | `blast_radius` Tool |
| `crates/atomcode-capabilities/src/codeintel/lsp_tool.rs` | 460 | `lsp` 工具（4 operation） |
| `crates/atomcode-capabilities/src/codeintel/lsp/jsonrpc.rs` | 92 | Content-Length 帧 |
| `crates/atomcode-capabilities/src/codeintel/lsp/client.rs` | 1170 | 传输无关 LspClient |
| `crates/atomcode-capabilities/src/codeintel/lsp/manager.rs` | 460 | LspManager（池化 + sticky 失败） |
| `crates/atomcode-capabilities/src/codeintel/lsp/registry.rs` | 141 | 7 个默认 server |
| `crates/atomcode-capabilities/src/codeintel/lsp/types.rs` | 124 | Diagnostic/Location 协议中立类型 |
| `crates/atomcode-config/src/lsp_registry.rs` | 80 | 配置层 LspServerConfig |

### 8.2 claudecode

| 路径 | 行数 | 内容 |
|------|------|------|
| `src/tools/LSPTool/LSPTool.ts` | 860 | 9 op LSP 工具入口 |
| `src/tools/LSPTool/schemas.ts` | 215 | Zod discriminated union |
| `src/tools/LSPTool/formatters.ts` | 592 | 9 种结果格式化 |
| `src/tools/LSPTool/symbolContext.ts` | 90 | UI 用 position → symbol |
| `src/tools/LSPTool/UI.tsx` | 227 | React Ink 渲染 |
| `src/tools/LSPTool/prompt.ts` | 21 | 系统提示词 |
| `src/services/lsp/config.ts` | 79 | LSP server 配置加载 |
| `src/services/lsp/LSPClient.ts` | 447 | vscode-jsonrpc 封装 |
| `src/services/lsp/LSPDiagnosticRegistry.ts` | 386 | LRU 去重 + cross-turn 抑制 |
| `src/services/lsp/LSPServerInstance.ts` | 511 | 单 server + 状态机 + 重试 |
| `src/services/lsp/LSPServerManager.ts` | 420 | 多 server 池 |
| `src/services/lsp/manager.ts` | 289 | 工具层薄包装 |
| `src/services/lsp/passiveFeedback.ts` | 328 | 诊断作为附件回写 |
| `src/utils/ide.ts` | - | IDE 检测 |
| `src/commands/ide/ide.tsx` | - | `/ide` 斜杠命令 |
| `src/utils/bash/treeSitterAnalysis.ts` | - | tree-sitter bash 风险判定 |

### 8.3 deepseek-harness

| 路径 | 行数 | 内容 |
|------|------|------|
| `packages/lsp/lsp-stdio/src/abort.ts` | 48 | abortable + abortError |
| `packages/lsp/lsp-stdio/src/connection.ts` | 329 | JSON-RPC endpoint |
| `packages/lsp/lsp-stdio/src/framing.ts` | 102 | encodeMessage / MessageDecoder |
| `packages/lsp/lsp-stdio/src/host.ts` | 124 | HostSource 抽象 |
| `packages/lsp/lsp-stdio/src/index.ts` | 369 | Provider 入口 |
| `packages/lsp/lsp-stdio/src/instance.ts` | 347 | LspInstance + 串行 queue |
| `packages/lsp/lsp-stdio/src/invariant.ts` | 30 | 不变量检查 |
| `packages/lsp/lsp-stdio/src/protocol.ts` | 80 | Wire 类型 |
| `packages/lsp/lsp-stdio/src/translate.ts` | 235 | normalize / supportsTransientOpen |
| `packages/lsp/` | - | 抽象接口包 |
| `packages/lsp/tool-lsp/` | - | 模型工具包装 |
| `.agents/notes/implemented/architecture/2026-07-15-lsp-capability-seam.zh.md` | - | Capability Seam 设计笔记 |

### 8.4 opencode

| 路径 | 行数 | 内容 |
|------|------|------|
| `packages/opencode/src/lsp/lsp.ts` | 507 | LSP.Service（Effect DI） |
| `packages/opencode/src/lsp/server.ts` | 1983 | 32 个 LSPServer.Info |
| `packages/opencode/src/lsp/client.ts` | 650 | LSPClient + vscode-jsonrpc |
| `packages/opencode/src/lsp/language.ts` | 121 | 100+ 扩展名 → language id |
| `packages/opencode/src/lsp/launch.ts` | 21 | spawn 薄包装 |
| `packages/opencode/src/lsp/diagnostic.ts` | 29 | 诊断结构 |
| `packages/opencode/src/ide/index.ts` | 54 | 反向 IDE 检测 + 扩展市场安装 |
| `packages/opencode/src/cli/cmd/debug/lsp.ts` | - | 调试子命令 |
| `packages/web/src/content/docs/lsp.mdx` | - | 32 server 文档 |
| `packages/opencode/test/lsp/client.test.ts` | - | 客户端测试 |

### 8.5 openclaw

| 路径 | 行数 | 内容 |
|------|------|------|
| `src/infra/command-explainer/tree-sitter-runtime.ts` | - | tree-sitter bash 加载 |
| `src/infra/command-explainer/extract.ts` | - | 树提取 commands |
| `src/infra/command-explainer/extract.test.ts` | - | 单测 |
| `src/infra/command-analysis/explain.lazy.test.ts` | - | lazy 集成测试 |

### 8.6 pi

无 LSP/CodeIntel 相关模块。

### 8.7 hermes-agent

| 路径 | 内容 |
|------|------|
| `agent/lsp/__init__.py` | 包入口 |
| `agent/lsp/client.py` | LSP 客户端（process + JSON-RPC） |
| `agent/lsp/cli.py` | CLI 入口 |
| `agent/lsp/eventlog.py` | 事件日志 |
| `agent/lsp/install.py` | 自动下载 LSP 二进制 |
| `agent/lsp/manager.py` | 多 server 池 |
| `agent/lsp/protocol.py` | 协议常量 |
| `agent/lsp/range_shift.py` | patch 后修复 LSP range |
| `agent/lsp/reporter.py` | 诊断报告生成 |
| `agent/lsp/servers.py` | 内置 server 配置 |
| `agent/lsp/workspace.py` | 工作区文件夹发现 |
| `tools/patch_parser.py` | Write 工具集成：patch output 内联 diagnostics |
| `tests/agent/lsp/test_client_e2e.py` | E2E 测试 |
| `tests/agent/lsp/test_install_and_lint_fixes.py` | install + lint 修复测试 |
| `tests/agent/lsp/test_delta_key.py` | 增量 key 测试 |
| `tests/agent/lsp/test_reporter.py` | 报告生成测试 |
| `tests/agent/lsp/_mock_lsp_server.py` | mock LSP server |

### 8.8 laew（待新增 — 路线图对应）

| 路径 | 阶段 | 内容 |
|------|------|------|
| `src/agent/codeintel/mod.rs` | P0 | CodeIntel 模块出口 |
| `src/agent/codeintel/lang.rs` | P0 | Lang 枚举（4 grammar：Rust/TS/Python/Go） |
| `src/agent/codeintel/symbols.rs` | P0 | tree-sitter 符号提取 |
| `src/agent/tools/list_symbols.rs` | P0 | `list_symbols` 工具 |
| `src/agent/tools/read_symbol.rs` | P0 | `read_symbol` 工具 |
| `src/agent/lsp/jsonrpc.rs` | P1 | Content-Length 帧（92 行模板） |
| `src/agent/lsp/client.rs` | P1 | LspClient（先实现 5 个核心方法） |
| `src/agent/lsp/manager.rs` | P1 | LspManager + sticky 失败 |
| `src/agent/lsp/registry.rs` | P1 | rust-analyzer 默认 |
| `src/agent/lsp/types.rs` | P1 | Diagnostic/Location |
| `src/agent/tools/lsp.rs` | P1 | `lsp` 工具（4 op） |
| `src/agent/codeintel/index.rs` | P2.1 | CodeIndex + 4 图工具 |
| `src/ide/mod.rs` | P2.2 | 反向 IDE 检测 |
| `src/devcontainer/mod.rs` | P2.3 | .devcontainer.json 解析 |

---

## 9. 结语

LSP 是 Agent 从「文本 grep」升级到「语义理解」的必经之路，但**实现路径有 4 个分叉点**：

1. **协议栈自研 vs vscode-jsonrpc**：自研可控可测（atomcode 92 行），复刻省事（opencode/claudecode）
2. **tree-sitter 单文件 vs LSP 跨文件**：atomcode 两者都做（分层），其他工程只选其一
3. **stdio transport only vs 多 transport**：deepseek-harness 包分立支持扩展
4. **IDE 反向集成 vs 无**：opencode 反向检测 + 自动安装，其他工程无

laew 的最优路径是 **P0 tree-sitter + P1 LSP stdio + P2 CodeIndex**，避免过早进入 IDE 集成和 Dev Container 复杂度。atomcode 的 CodeIntel 七件套是最完整的范式参考，但其 5500 行代码量在 laew 1 人维护规模下需要分 3 个版本迭代。

---
