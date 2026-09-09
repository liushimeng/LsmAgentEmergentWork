# 专题-第十五轮-atomcode-深度分析

> 分析日期: 2026-09-09
> 目标工程: `/usr/local/LsmGitOpenSource/atomcode` (v5.0.9)
> 分析维度: 8 个新维度(前 14 轮未深入覆盖)
> 对比参考: claude-code / deepseek-harness / openclaw / opencode / pi / undici / Switchyard

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

### 1.1 连接池精细管理

**代码位置**: `crates/atomcode-capabilities/src/provider/retry.rs:45-54`

```rust
pub(crate) const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(15);
```

**核心机制**: atomcode 将 reqwest 默认的 90s 空闲超时压缩至 15s,原因是对真实网关负载均衡器的观察——LB 常在 30s 内关闭空闲连接,而客户端复用这些半开连接会触发 hyper `IncompleteMessage` 错误。15s 的设定既低于常见 LB 窗口,又足够覆盖工具循环中请求间的短间隙(远低于 15s)。

**设计意图**: 这是一个经过生产验证的调优——注释明确记载"30s proved too generous against a real gateway — half-open reuse there surfaced as hyper `IncompleteMessage`"。该常量被三个 Provider 适配器统一复用(Anthropic/OpenAI-Compat/Ollama),保证连接池行为一致。

**laew gap**: L836(无连接池空闲超时配置)

### 1.2 TLS 版本自动降级

**代码位置**: `crates/atomcode-config/src/tls.rs:1-172`

**核心机制**: 完整的三层 TLS 版本策略:
- **环境变量覆盖**: `ATOMCODE_TLS_MAX=1.2` 进程启动即全局生效
- **自动回退**: 首次连接失败后 latch `MANAGED_TLS12` 标志,后续对托管端点(*.atomgit.com)自动使用 TLS 1.2
- **作用域隔离**: 自动状态仅适用于 `is_managed_https_url()` 匹配的托管端点,不影响第三方 API

**设计意图**: 应对企业中间盒(middlebox)对 TLS 1.3 的 RST 攻击(`os error 10054`)。注释记载"observed in the wild against `*.atomgit.com`"。`latch_managed_tls12()` 仅在 TLS 1.2 回退请求成功后触发,避免盲目降级。

**laew gap**: L837(无 TLS 版本策略)

### 1.3 TLS 记录腐化检测

**代码位置**: `crates/atomcode-capabilities/src/provider/retry.rs:158-194`

```rust
pub(crate) fn chain_has_tls_corruption(err: &(dyn std::error::Error + 'static)) -> bool {
    let msg = e.to_string();
    if msg.contains("BadRecordMac")
        || msg.contains("DecryptError")
        || msg.contains("cannot decrypt peer's message")
    {
        return true;
    }
    // ...
}
```

**核心机制**: 通过错误链字符串匹配识别 TLS 记录腐化——包括对端拒绝我们发送的 record(`BadRecordMac`)和我们无法解密对端 record(`DecryptError`)两种方向。这是 TLS 1.3 长连接状态失同步或中间盒篡改的特征。

**设计意图**: 注释记载"跑着跑着就 BadRecordMac"——连接运行一段时间后出现的间歇性腐化。恢复策略限定在 OPEN 路径(响应字节未消费前):重建客户端池,托管端点升级到 TLS 1.2。握手失败(`HandshakeFailure`)被排除——那是 `is_connect()` 回退路径的职责。

**laew gap**: L838(无 TLS 错误分类)

### 1.4 陈旧连接综合检测

**代码位置**: `crates/atomcode-capabilities/src/provider/retry.rs:103-156`

**核心机制**: 三重检测覆盖陈旧/半开连接:
1. `is_timeout() || is_connect()` — 连接建立阶段失败
2. `chain_has_transient_io()` — 遍历 source chain 寻找 `ConnectionReset/ConnectionAborted/BrokenPipe/UnexpectedEof/NotConnected/TimedOut`
3. `chain_has_tls_corruption()` — TLS 记录级腐化

**设计意图**: 注释详述了真实案例——"the user-reported 'open failed' that `/login` 'fixed' by rebuilding the client's pool"。网关 LB 静默关闭空闲 keep-alive 后,客户端复用会触发 `error sending request` 包裹 `io::Error(ConnectionReset)`,而 `is_connect()` 对此返回 false(连接已建立完成)。

**laew gap**: L839(无陈旧连接检测)

### 1.5 SSE 流式解码与空闲看门狗

**代码位置**: `crates/atomcode-capabilities/src/provider/anthropic.rs:68-75`, `ollama.rs:171-200`

**核心机制**:
- **Anthropic**: 事件型 SSE(`message_start/content_block_*/message_delta/message_stop`),工具调用参数按 content-block index 缓冲,在 `content_block_stop` 时整体发射
- **Ollama**: NDJSON 行协议(无 `data:` 前缀,无 `[DONE]` 哨兵),工具调用完整到达(不碎片化)
- **空闲看门狗**: `idle_timeout` 字节级超时,无数据到达即终断定流

**设计意图**: 两个 Provider 的流式协议差异被封装在各自的 Decoder 中,对 kernel 统一暴露 `StreamEvent` 枚举。Ollama 工具调用无 id,Decoder 合成稳定 id 以便 kernel 配对 call/result,但 id 永不回传(tool result 按顺序匹配)。

**laew gap**: L840(无空闲超时配置)

### 1.6 OpenRouter 归因头

**代码位置**: `crates/atomcode-capabilities/src/provider/openai_compat.rs:48-102`

```rust
pub const OPENROUTER_ATTRIBUTION_HEADERS: &[(&str, &str); 3] = &[
    ("HTTP-Referer", "https://gitcode.com/atomgit_atomcode/atomcode"),
    ("X-OpenRouter-Title", "AtomCode"),
    ("X-OpenRouter-Categories", "cli-agent"),
];
```

**核心机制**: 仅当 URL 匹配 `openrouter.ai` 主机时附加归因头,通过 `is_openrouter_url()` 手工解析主机(避免引入 `url` crate 可选依赖),并防御 `userinfo@` 伪造攻击。

**设计意图**: 让真实用户流量归因到 OpenRouter 市场的"AtomCode"应用条目。注释明确"the headers are meaningless, and would only leak product identity, on any other OpenAI-compatible endpoint"。

**laew gap**: L841(无应用归因机制)

---

## 2. 编译器前端

### 2.1 手写 Bash 词法分析器

**代码位置**: `crates/atomcode-capabilities/src/tools/bash_workspace_gate.rs:209-250`

```rust
fn tokenize(seg: &str) -> Vec<String> {
    let chars: Vec<char> = seg.chars().collect();
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            cur.push(c);
            if q == '"' && c == '\\' && i + 1 < chars.len() {
                cur.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == q { quote = None; }
            i += 1; continue;
        }
        match c {
            '\'' | '"' => { cur.push(c); quote = Some(c); i += 1; }
            '\\' if i + 1 < chars.len() => {
                cur.push(chars[i + 1]); i += 2;
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() { toks.push(std::mem::take(&mut cur)); }
                i += 1;
            }
            '>' => { /* 重定向操作符独立成 token */ }
            // ...
        }
    }
    // ...
}
```

**核心机制**: 完整的 quote-aware 词法分析器,正确处理:
- 单引号/双引号嵌套
- 反斜杠转义(包括 `\$`、`\>`、`\ `)
- 重定向操作符(`>`/`>>`/`2>`)独立成 token
- fd 前缀吸收(`2>` → 单个 token)

**设计意图**: 为 `BashWorkspaceGate` 的破坏性命令检测提供精确的 token 流。注释强调"escaped `;`, `&`, or `|` remains ordinary word content rather than splitting the command"——防止注入绕过。

**laew gap**: L842(无 shell 词法分析器)

### 2.2 Tree-sitter Bash AST 解析

**代码位置**: `crates/atomcode-capabilities/src/tools/bash.rs:2176-2254`

```rust
fn parse_bash(command: &str) -> Option<tree_sitter::Tree> {
    thread_local! {
        static PARSER: RefCell<Option<tree_sitter::Parser>> = RefCell::new(None);
    }
    PARSER.with(|slot| {
        let mut opt = slot.borrow_mut();
        if opt.is_none() {
            let mut p = tree_sitter::Parser::new();
            p.set_language(&tree_sitter_bash::LANGUAGE.into()).ok()?;
            *opt = Some(p);
        }
        opt.as_mut().unwrap().parse(command, None)
    })
}
```

**核心机制**: 基于 `tree-sitter-bash` 的增量解析器,thread_local 缓存避免重复加载语法。`bash_invocations()` 遍历 AST 提取所有 `command` 节点(包括命令替换和子壳嵌套),保留源序。

**设计意图**: `is_read_only_bash()` 使用 AST 进行"可证明只读"判定——仅允许固定结构节点类型集合(`command/word/raw_string/string/number/redirect`),任何未知命名节点(命令替换 `$()`、子壳 `(...)`、变量展开 `$HOME`)都导致 fail-closed。注释记载"quoted metacharacters are DATA, not operators (`grep 'a\|b'` → true)"。

**laew gap**: L843(无 AST 级命令分析)

### 2.3 代码图构建(tree-sitter)

**代码位置**: `crates/atomcode-capabilities/src/codeintel/index.rs:14-131`

```rust
fn extract_calls(source: &str, lang: Lang, syms: &[Symbol]) -> Vec<RawCall> {
    let grammar = lang.grammar();
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    let tree = parser.parse(source, None)?;
    let query = Query::new(&grammar, q_src)?;
    let callee_idx = query.capture_index_for_name("callee")?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    // ...
}
```

**核心机制**: 使用 tree-sitter Query 提取调用边(`@callee` capture),归属到最近包围的 Function/Method symbol。支持 17 种源文件扩展名(rs/py/js/ts/go/java/c/cpp 等)。

**设计意图**: 为 `find_references/trace_callers/trace_callees` 等代码图工具提供调用边。注释强调"max start_line whose range covers the call"——通过行范围包含关系确定调用者,而非简单的词法最近。

**laew gap**: L844(无代码图索引)

### 2.4 HTML 分词器

**代码位置**: `crates/atomcode-capabilities/src/tools/web_fetch.rs:632-770`

```rust
fn tokenize_html(html: &str) -> Vec<HtmlToken> {
    // ...
}
```

**核心机制**: 手写 HTML 分词器,将 HTML 标签/属性/文本拆分为 `HtmlToken` 枚举,用于 WebFetch 工具的内容提取与截断。

**设计意图**: 在不引入完整 HTML 解析器依赖的前提下,实现足够精确的标签感知截断(避免在标签中间切断)。

**laew gap**: L845(无 HTML 分词器)

---

## 3. 操作系统内核交互

### 3.1 信号安全终端恢复

**代码位置**: `crates/atomcode-tuix/src/signal_restore.rs:1-123`

```rust
extern "C" fn handler(signo: c_int) {
    let seq = restore_writes();
    unsafe { libc::write(libc::STDOUT_FILENO, seq.as_ptr().cast(), seq.len()); }
    if TERMIOS_SAVED.load(Ordering::Acquire) {
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, addr_of!(ORIG_TERMIOS).cast()); }
    }
    unsafe { libc::signal(signo, libc::SIG_DFL); libc::raise(signo); }
}
```

**核心机制**: 为 SIGTERM/SIGINT/SIGHUP 安装 raw `sigaction` 处理器,仅使用 async-signal-safe 调用(`write/tcsetattr/signal/raise`):
1. 发送恢复字节序列(Kitty 键盘协议 pop、鼠标关闭、光标显示、自动换行、滚动区释放、bracketed-paste 关闭)
2. 恢复捕获的 cooked `termios`
3. 重新 raise 信号(保持正确退出状态)

**设计意图**: 解决"Ctrl-C twice to exit writes junk into the input box"——TUI 被信号杀死时内核直接拆除进程,不运行 Drop,导致终端留在 raw 模式。注释强调"NOT a `tokio` signal task, which the very wedge that triggers the kill would starve"——使用 tokio signal 会在死锁时失效。

**laew gap**: L846(无信号安全恢复)

### 3.2 原始模式与 termios 控制

**代码位置**: `crates/atomcode-auth/src/oauth.rs:283-325`

```rust
struct RawStdin {
    fd: std::os::unix::io::RawFd,
    orig: libc::termios,
}

impl RawStdin {
    fn new(fd: RawFd) -> io::Result<Self> {
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut orig) } != 0 { return Err(...); }
        let mut new = orig;
        new.c_lflag &= !(libc::ICANON | libc::ECHO);
        new.c_cc[libc::VMIN] = 0;
        new.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &new) } != 0 { return Err(...); }
        Ok(Self { fd, orig })
    }
}
```

**核心机制**: OAuth 登录流程需要非阻塞单字节读取(轮询浏览器回调),通过 `termios` 关闭 ICANON(规范模式)和 ECHO,设置 VMIN=0/VTIME=0 实现非阻塞。RAII Drop 恢复原始终端属性。

**设计意图**: 在不干扰 TUI 主循环的情况下,实现 stdin 的非阻塞轮询。注释强调"single write to the master pty, so a 32-byte non-blocking read sees"——PTY 主端单次 write 可被非阻塞 read 完整读取。

**laew gap**: L847(无 termios 原始模式)

### 3.3 非阻塞 I/O 与 poll()

**代码位置**: `crates/atomcode-auth/src/oauth.rs:981-1048`

```rust
fn stdin_readline(fd: RawFd, stop: &AtomicBool, buf: &mut String) -> Option<String> {
    let orig_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    unsafe { libc::fcntl(fd, libc::F_SETFL, orig_flags | libc::O_NONBLOCK); }
    // ...
    let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    let poll_rc = unsafe { libc::poll(&mut pfd, 1, 100) };
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    // ...
}
```

**核心机制**: 使用 `fcntl` 设置 `O_NONBLOCK` + `poll()` 实现 100ms 超时的 stdin 轮询,同时检查 `stop` 标志以支持取消。

**设计意图**: 在 OAuth 回调等待期间,既要轮询 stdin(用户可能粘贴回调 URL),又要响应取消信号。`poll()` 比纯忙等待更高效,100ms 超时平衡了响应性和 CPU 占用。

**laew gap**: L848(无 poll 非阻塞 I/O)

### 3.4 私有文件权限

**代码位置**: `crates/atomcode-capabilities/src/datalog.rs:7-13`

```rust
// On Unix the output directory and files are created private (0o700/0o600).
// On Windows there is no equivalent mode bit, so files are created with the
// directory's inherited ACLs — which already deny other standard users when the
// datalog lives under the user profile.
```

**核心机制**: datalog 写入器在 Unix 上使用 `OpenOptionsExt::mode(0o600)` 创建文件、`DirBuilderExt::mode(0o700)` 创建目录,确保仅属主可读写。

**设计意图**: datalog 包含完整请求体(系统提示、消息、工具定义),属于敏感数据。注释警告 Windows 用户"points `datalog.dir` at a world-readable location on a shared machine, the request bodies are readable by other local users"。

**laew gap**: L849(无私有文件权限)

---

## 4. 分布式共识

### 4.1 无 Raft/Paxos/Gossip 实现

**分析结论**: atomcode 作为单机 CLI + 可选 daemon 架构,未实现任何分布式共识协议(Raft/Paxos/Gossip)。其 daemon 模式为单实例 HTTP 服务,无集群协调需求。

**laew gap**: L850(无分布式共识实现)

### 4.2 广播通道与事件排序

**代码位置**: `crates/atomcode-daemon/src/live_hub.rs:187-200`

```rust
pub struct LiveViewHub {
    state: Mutex<HubState>,
    events: broadcast::Sender<LiveObservation>,
}

impl LiveViewHub {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self { state: Mutex::new(HubState::default()), events }
    }
}
```

**核心机制**: 使用 `tokio::sync::broadcast` 实现多订阅者事件分发,容量 1024。`LiveObservation` 包含 `binding_id/generation/cursor/event`,通过 `cursor` 单调递增实现事件排序。

**设计意图**: WebUI 的 `/live` SSE 端点允许多个浏览器订阅同一会话的实时事件流。`generation` 用于检测 stale binding(运行时已重启但订阅者仍持有旧 binding)。

**laew gap**: L851(无分布式事件总线)

### 4.3 会话绑定与代际追踪

**代码位置**: `crates/atomcode-daemon/src/live_hub.rs:36-43`

```rust
pub struct LiveBinding {
    pub id: u64,
    pub generation: u64,
    pub session_id: String,
    pub working_dir: PathBuf,
    pub provider: String,
    pub provider_fingerprint: String,
}
```

**核心机制**: 每个运行时绑定分配唯一 `id` + 单调递增 `generation`。当运行时重启(如用户切换模型),`generation` 递增,旧绑定自动失效。`provider_fingerprint` 检测配置变更。

**设计意图**: 防止 stale subscriber 将事件路由到错误的运行时实例。`HubError::RuntimeGenerationChanged { expected, actual }` 显式报告代际不匹配。

**laew gap**: L852(无会话代际追踪)

---

## 5. 机器学习推理

### 5.1 Ollama 本地推理集成

**代码位置**: `crates/atomcode-capabilities/src/provider/ollama.rs:1-125`

```rust
pub struct OllamaConfig {
    pub api_key: String,  // OPTIONAL — Ollama itself needs none
    pub base_url: String, // e.g. http://localhost:11434
    pub model: String,
    pub max_tokens: Option<u32>, // → options.num_predict
    pub think: bool, // Enable thinking (think: true)
    pub idle_timeout: Duration,
    pub connect_timeout: Duration,
    pub retry: RetryPolicy,
    pub skip_tls_verify: bool,
}
```

**核心机制**: 完整的 Ollama `/api/chat` 适配器,支持:
- NDJSON 流(每行一个完整 JSON 对象)
- 思考模式(`think: true`,推理模型)
- 可选 bearer token(本地无需,前置代理可能需要)
- 工具调用 id 合成(Ollama 不返回 id,Decoder 内部合成)

**设计意图**: 注释详述"Ollama tool calls carry NO id — the decoder SYNTHESIZES a stable id so the kernel can pair the call with its result; the id stays internal (never sent back, since a tool result is matched by ORDER on the wire)"。

**laew gap**: L853(无 Ollama 集成)

### 5.2 NDJSON 流解码与透明重连

**代码位置**: `crates/atomcode-capabilities/src/provider/ollama.rs:171-200`

```rust
const MAX_STREAM_ATTEMPTS: u32 = 3;
let mut stream_attempt = 1u32;
let mut reconnect_attempts = 0u32;
'reopen: loop {
    let mut dec = OllamaNdjsonDecoder::new();
    let mut emitted_replay_sensitive = false;
    let mut pending_metadata = Vec::new();
    let byte_stream = resp.bytes_stream();
    // ...
}
```

**核心机制**: 流读取失败时支持最多 3 次透明重连(1 次初始打开 + 2 次重开)。关键约束:仅在未发射 replay-sensitive 输出(text/reasoning/tool call)前可重放整个请求;元数据(id/model/usage)可重复,内容和工具数据不可。

**设计意图**: 注释解释"a gateway resetting connections under load can drop more than one attempt before a healthy backend answers"。`pending_metadata` 缓冲元数据直到首个 replay-sensitive 事件确认当前尝试有效。

**laew gap**: L854(无透明流重连)

### 5.3 视觉模型启发式检测

**代码位置**: `crates/atomcode-capabilities/src/provider/openai_compat.rs:170-191`

```rust
pub fn model_suggests_vision(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("vision") || n.contains("-vl") || n.contains("vl-")
        || n.contains("ocr") || n.contains("-4v") || n.contains("-4.1v")
        || n.starts_with("gpt-4o") || n.starts_with("claude-3")
        || n.starts_with("claude-4") || n.starts_with("claude-5")
        || n.starts_with("claude-6") || n.starts_with("claude-7")
        || n.starts_with("claude-sonnet") || n.starts_with("claude-opus")
        || n.starts_with("claude-haiku") || n.starts_with("gemini")
        || n.starts_with("pixtral") || n.contains("llava") || n.contains("qvq")
}
```

**核心机制**: 基于模型名的启发式视觉能力检测,控制图像编码和 `read_file` 视觉路径。当模型不支持视觉时,历史图像降级为纯文本字符串(保留 caption,丢弃图像字节)。

**设计意图**: 避免"re-sending a historical image to a text-only model 400s the whole request (`glm-5.2 is not a multimodal model`) on every resumed turn"。

**laew gap**: L855(无视觉模型检测)

### 5.4 推理策略派生

**代码位置**: `crates/atomcode-capabilities/src/provider/reasoning.rs:149`

```rust
ReasoningPolicy::derive("some-model", "https://api-inference.xiaomimimo.com/v1")
```

**核心机制**: `ReasoningPolicy::derive()` 根据模型名和端点 URL 自动推导推理策略(Include/Exclude/Adaptive),处理不同厂商的 thinking/reasoning 字段差异。

**设计意图**: 统一 Anthropic 的 signed thinking block 和 OpenAI 的 `reasoning_content` 两种推理往返格式。

**laew gap**: L856(无推理策略派生)

---

## 6. 形式化验证

### 6.1 契约符合性测试框架

**代码位置**: `crates/atomcode-kernel/src/conformance/mod.rs:1-219`

```rust
pub struct ConformanceReport {
    pub seam: &'static str,  // "Tool"/"ToolMiddleware"/"LifecycleHooks"/"LlmProvider"
    pub subject: String,
    pub checks: Vec<CheckOutcome>,
}

impl ConformanceReport {
    pub fn assert_conformant(&self) {
        if self.passed() { return; }
        panic!("{} conformance FAILED for `{}` ({} of {} checks failed):\n{}",
            self.seam, self.subject, self.failures().len(), self.checks.len(), ...);
    }
}
```

**核心机制**: 为 kernel 的四个扩展缝(LlmProvider/Tool/ToolMiddleware/LifecycleHooks)提供契约检查器。每个 `seam::check(impl) -> ConformanceReport` 对任意 trait 对象执行契约验证,返回结构化通过/失败报告。

**设计意图**: 注释强调"A third-party adapter / tool / middleware / hook that violates one of these breaks the host in ways a normal unit test won't surface"——第三方实现违反契约会导致宿主在正常测试中难以发现的问题。

**laew gap**: L857(无契约符合性框架)

### 6.2 Provider 契约验证

**代码位置**: `crates/atomcode-kernel/src/conformance/provider.rs:23-122`

```rust
pub async fn check(provider: Arc<dyn LlmProvider>) -> ConformanceReport {
    // model_name(): non-empty + stable
    r.record("model_name_non_empty", !a.is_empty(), "model_name() must be non-empty");
    r.record("model_name_stable", a == b, format!("model_name() must be stable across calls"));
    
    // context_window(): stable
    r.record("context_window_stable", a == b, format!("context_window() must be stable"));
    
    // chat_stream(): opens (Ok) or fails cleanly (Err), no panic; must TERMINATE
    r.record("stream_terminates", true, "the stream MUST terminate");
    
    // chat_stream must HANDLE non-default ChatOptions
    r.record("chat_stream_handles_options", true, "an adapter must MAP or IGNORE each neutral knob, never panic");
}
```

**核心机制**: 验证 Provider 的契约条款:
- `model_name()` 非空且跨调用稳定(用于存储元数据/遥测)
- `context_window()` 稳定(0 = 未知允许)
- `chat_stream()` 打开成功或干净失败,无 panic;打开的流必须在边界内终止
- 处理非默认 ChatOptions(temperature/max_tokens/tool_choice)时不 panic

**设计意图**: 注释强调"a non-terminating stream parks the agent forever"——kernel 的 turn loop 每轮消费流到完成,半开/静默流会导致 agent 永久挂起。

**laew gap**: L858(无 Provider 契约验证)

### 6.3 流良构性纯检查器

**代码位置**: `crates/atomcode-kernel/src/conformance/provider.rs:134-176`

```rust
pub fn check_stream_wellformed(events: &[StreamEvent]) -> ConformanceReport {
    // no_events_after_terminal — Done/Error 必须是最后一个事件
    r.record("no_events_after_terminal", i + 1 == events.len(), ...);
    
    // at_most_one_done — Done 最多发射一次
    r.record("at_most_one_done", dones <= 1, ...);
    
    // midstream_error_has_no_http_status — 中间错误不携带 http_status
    r.record("midstream_error_has_no_http_status", !bad_status, ...);
}
```

**核心机制**: 纯函数检查器,无需真实后端——适配器将解码后的事件(SSE fixture 通过解码器)送入验证。检查:
- 终端事件(Done/Error)后无事件
- Done 最多一次
- 中间 `StreamEvent::Error` 的 `http_status == None`(http_status 保留给 OPEN 失败)

**设计意图**: 注释解释"an adapter feeds its DECODED events (e.g. an SSE fixture run through its decoder) into this — so the kernel's stream contract is verified against the real decoder without a live backend"。

**laew gap**: L859(无流良构性检查)

### 6.4 工具调用重建契约

**代码位置**: `crates/atomcode-kernel/src/conformance/provider.rs:178-200`

```rust
pub fn check_stream_reconstruction(events: &[StreamEvent]) -> ConformanceReport {
    let mut groups: BTreeMap<u32, (Option<String>, Option<String>, String)> = BTreeMap::new();
    let mut complete: Vec<ToolCall> = Vec::new();
    for ev in events {
        match ev {
            StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                // 按 index 分组,拼接 arguments
            }
            StreamEvent::ToolCall(tc) => complete.push(tc.clone()),
            // ...
        }
    }
}
```

**核心机制**: 验证流式工具调用契约——按 index 分组的 `ToolCallDelta.arguments` 拼接后必须重现 kernel 实际执行的完整 `StreamEvent::ToolCall`。完整调用按顺序与 delta index 相关联(第 i 个流式 index ↔ 第 i 个完整调用)。

**设计意图**: 确保 index-buffering 解码器在 finish 时正确刷新。注释强调"A stream with NO `ToolCallDelta` is trivially conformant"——不支持流式工具调用的适配器直接发射完整 ToolCall。

**laew gap**: L860(无工具调用重建验证)

### 6.5 必须不 panic 契约

**代码位置**: `crates/atomcode-kernel/src/conformance/mod.rs:130-174`

```rust
pub(crate) async fn catch_async<F: Future>(fut: F) -> Result<F::Output, String> {
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(v) => Ok(v),
        Err(e) => Err(panic_message(e)),
    }
}

pub(crate) async fn run_void<F: Future<Output = ()>>(
    report: &mut ConformanceReport,
    name: &'static str,
    fut: F,
) {
    match with_timeout(DEFAULT_CHECK_TIMEOUT, catch_async(fut)).await {
        Ok(Ok(())) => report.record(name, true, ""),
        Ok(Err(p)) => report.record(name, false, format!("panicked: {p} — see the seam's must-not-panic PANIC CONTRACT")),
        Err(t) => report.record(name, false, t),
    }
}
```

**核心机制**: 将"必须不 panic"契约可执行化——在 `catch_unwind` 中运行注入调用,panic 被捕获并报告为失败检查。配合 `with_timeout(DEFAULT_CHECK_TIMEOUT=2s)` 检测挂起/无限循环。

**设计意图**: 注释解释"This works under the default `cargo test` (unwind) profile, where a panicking impl is caught and reported as a failed check. Under the `panic = "abort"` release profile `catch_unwind` is a no-op"——因此必须在测试 profile 下运行。

**laew gap**: L861(无 panic 契约验证)

---

## 7. 图数据库与知识图谱

### 7.1 代码图数据结构

**代码位置**: `crates/atomcode-capabilities/src/codeintel/graph.rs:9-82`

```rust
pub struct CodeGraph {
    pub nodes: HashMap<SymbolId, SymbolNode>,
    pub edges_out: HashMap<SymbolId, Vec<Edge>>,  // 正向: from → callee
    pub edges_in: HashMap<SymbolId, Vec<Edge>>,   // 反向: to → caller
    pub file_symbols: HashMap<PathBuf, Vec<SymbolId>>,
    pub file_mtimes: HashMap<PathBuf, u64>,
    #[serde(skip)]
    pub by_name: HashMap<String, Vec<SymbolId>>,  // 名称索引
}

pub enum SymbolKind {
    Function, Method, Struct, Class, Trait, Interface,
    Enum, Constant, Variable, Module, Import, TypeAlias, Other(String),
}

pub enum EdgeKind {
    Calls, Imports, Inherits, Implements, References,
}
```

**核心机制**: 内存中的代码图,包含符号节点(Function/Method/Class/Trait/Enum 等 12 种)和边(Calls/Imports/Inherits/Implements/References 5 种)。双向邻接表(`edges_out` + `edges_in`)支持正反向遍历。

**设计意图**: 注释警告"EDGE CONVENTION (load-bearing, from production): `edges_out[from]` holds `Edge{to: callee}` (forward); `edges_in[to]` holds `Edge{to: from}` — i.e. in the reverse map the `to` field stores the SOURCE (caller). Do not 'fix' this."——这是从生产代码移植的负载约定。

**laew gap**: L862(无代码图数据结构)

### 7.2 BFS 调用链追踪

**代码位置**: `crates/atomcode-capabilities/src/codeintel/graph.rs:153-185`

```rust
pub fn trace_callers(&self, id: SymbolId, max_depth: usize) -> Vec<(SymbolId, usize)> {
    self.trace(id, max_depth, true)
}

pub fn trace_callees(&self, id: SymbolId, max_depth: usize) -> Vec<(SymbolId, usize)> {
    self.trace(id, max_depth, false)
}

fn trace(&self, id: SymbolId, max_depth: usize, callers: bool) -> Vec<(SymbolId, usize)> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut result = Vec::new();
    visited.insert(id);
    queue.push_back((id, 0usize));
    while let Some((cur, depth)) = queue.pop_front() {
        if depth >= max_depth { continue; }
        let edges = if callers { self.callers(cur) } else { self.callees(cur) };
        if let Some(edges) = edges {
            for e in edges {
                if visited.insert(e.to) {
                    result.push((e.to, depth + 1));
                    queue.push_back((e.to, depth + 1));
                }
            }
        }
    }
    result
}
```

**核心机制**: 双向 BFS 遍历,支持 `trace_callers`(谁调用了 id)和 `trace_callees`(id 调用了谁),深度限制 + 去重。返回 `(symbol_id, depth)` 对。

**设计意图**: 为 `find_references/trace_callers/trace_callees` 工具提供调用链分析。`max_depth` 防止无限循环(循环调用)。

**laew gap**: L863(无 BFS 调用链追踪)

### 7.3 最短调用路径

**代码位置**: `crates/atomcode-capabilities/src/codeintel/graph.rs:188-200`

```rust
pub fn shortest_path(&self, from: SymbolId, to: SymbolId) -> Option<Vec<SymbolId>> {
    if from == to { return Some(vec![from]); }
    const MAX_HOPS: usize = 10;
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    let mut parent: HashMap<SymbolId, SymbolId> = HashMap::new();
    visited.insert(from);
    queue.push_back((from, 0usize));
    while let Some((cur, depth)) = queue.pop_front() {
        if depth >= MAX_HOPS { continue; }
        // BFS + parent 记录以重建路径
    }
}
```

**核心机制**: BFS 最短路径算法,最大 10 跳限制。通过 `parent` HashMap 重建完整路径(包含两端)。

**设计意图**: 为 `trace_chain` 工具提供调用链最短路径分析。`MAX_HOPS` 防止大图上的过度搜索。

**laew gap**: L864(无最短路径算法)

### 7.4 文件索引与 @-mention

**代码位置**: `usr/local/LsmGitOpenSource/atomcode/crates/atomcode-capabilities/src/file_index.rs:14-178`

```rust
const STALE_TTL: Duration = Duration::from_secs(3);
const MAX_INDEX_ENTRIES: usize = 50_000;
const ALLOWLIST_DIR_MAX_ENTRIES: usize = 2_000;

fn push_indexed(out: &mut Vec<Entry>, seen: &mut HashSet<String>, rel: &Path, is_dir: bool, max_entries: usize) -> bool {
    // 跳过 .git 在任何深度
    if rel.components().any(|c| c.as_os_str() == ".git") { return false; }
    // 跳过含空格的路径(破坏 detect_at_mention 的空格终止规则)
    if s.contains(char::is_whitespace) { return false; }
    // 去重
    if !seen.insert(s.clone()) { return false; }
    out.push(Entry { rel_path: s, is_dir, depth });
    out.len() >= max_entries
}
```

**核心机制**: gitignore 感知的文件遍历 + 多通道索引:
- **允许通道**: `.claude/.atomcode/.agents` 目录强制索引(即使被 gitignore),每目录上限 2000 条目
- **主通道**: gitignore 感知遍历,上限 50000 条目
- **新鲜度**: 3s TTL,后台 re-walk 不阻塞查询

**设计意图**: 注释解释"the gitignore-aware walk is otherwise unbounded, so launching atomcode in a giant tree (an accidental `~` / `/`, or a repo with a huge non-ignored generated dir) used to peg a core at 100% for minutes walking millions of files (macOS `~/Library` alone)"。

**laew gap**: L865(无 @-mention 文件索引)

### 7.5 确定性符号 ID

**代码位置**: `crates/atomcode-capabilities/src/codeintel/graph.rs:89-96`

```rust
pub fn make_id(file: &Path, name: &str, start_line: usize) -> SymbolId {
    let mut h = DefaultHasher::new();
    file.hash(&mut h);
    name.hash(&mut h);
    start_line.hash(&mut h);
    h.finish()
}
```

**核心机制**: 基于(file, name, start_line)的确定性哈希,跨运行稳定。`SymbolId = u64` 作为图节点的全局标识。

**设计意图**: 确保增量重建时符号 ID 稳定,避免因文件扫描顺序变化导致 ID 漂移。

**laew gap**: L866(无确定性符号 ID)

---

## 8. 实时流处理

### 8.1 SSE 流与空闲看门狗

**代码位置**: `crates/atomcode-capabilities/src/provider/anthropic.rs:68-75`

```rust
pub idle_timeout: Duration,    // 每字节流空闲看门狗
pub connect_timeout: Duration, // 连接建立超时
pub open_timeout: Duration,    // 首字节(TTFB)超时
```

**核心机制**: 三层超时体系:
- `connect_timeout`(30s): TCP/TLS 握手
- `open_timeout`(90s): 等待响应 HEAD(首字节)
- `idle_timeout`(120s): 字节流空闲(无数据到达)

**设计意图**: 注释强调"no overall request timeout is set here (deliberately: a streaming response must not be capped)"——流式响应故意不设整体超时,但字节级空闲看门狗检测"接受连接但永不响应"的网关。

**laew gap**: L867(无字节级空闲超时)

### 8.2 广播通道与事件分发

**代码位置**: `crates/atomcode-daemon/src/live_hub.rs:187-200`

```rust
const BROADCAST_CAPACITY: usize = 1024;

pub struct LiveViewHub {
    state: Mutex<HubState>,
    events: broadcast::Sender<LiveObservation>,
}

pub struct LiveObservation {
    pub binding_id: u64,
    pub generation: u64,
    pub cursor: u64,  // 单调递增事件游标
    pub event: LiveViewEvent,
}
```

**核心机制**: `tokio::sync::broadcast` 通道容量 1024,支持多订阅者。`cursor` 单调递增保证事件序,`generation` 检测 stale binding。

**设计意图**: WebUI 的 `/live` SSE 端点允许多浏览器订阅同一会话。`BROADCAST_CAPACITY` 限制内存占用,慢消费者会 lag 而非 OOM。

**laew gap**: L868(无广播事件分发)

### 8.3 滚动窗口限流

**代码位置**: `crates/atomcode-coding/src/rate_limit.rs:84-115`

```rust
pub fn decide_from_windows(windows: &[RateLimitWindow], hint: &RateLimitHint) -> RateLimitDecision {
    // 仅看 5h 滚动窗口(≤18000s; 30d 月窗口已退役)
    let w = windows.iter()
        .filter(|w| w.window_size_seconds > 0 && w.window_size_seconds <= 18_000 && w.quota_exhausted)
        .min_by_key(|w| w.window_size_seconds);
    
    let secs = w.seconds_until_reset.max(0) as u64;
    if secs <= RATE_LIMIT_AUTO_WAIT_SECS {
        RateLimitDecision::WaitAndRetry { secs }
    } else {
        RateLimitDecision::Pause { reset_at_display, reset_label, secs_until_reset: Some(secs) }
    }
}
```

**核心机制**: 基于 CodingPlan 配额窗口的智能限流决策:
- 仅当窗口被服务器标记 `quota_exhausted` 时才暂停
- 选择最小 reopens 的窗口
- 瞬态 429(网关负载脱落)退化为 hint 的短 retry-after

**设计意图**: 注释警告"We must NOT fall back to a non-exhausted window's `seconds_until_reset`: a 5h ROLLING window's countdown is large at almost all times regardless of remaining quota, so pausing on it would misreport a transient 429 (e.g. usage 2%) as '5-hour window exhausted' for ~5h"。

**laew gap**: L869(无滚动窗口限流)

### 8.4 背压处理

**代码位置**: `crates/atomcode-tuix/src/lib.rs:656`

```rust
// we never want the upgrade task to block on UI backpressure.
```

**核心机制**: 升级任务与 UI 渲染解耦,避免 UI 背压阻塞升级流程。通过异步通道和独立任务实现。

**设计意图**: TUI 渲染在高负载时可能滞后,升级任务不应因此挂起。

**laew gap**: L870(无背压控制)

### 8.5 流重建契约

**代码位置**: `crates/atomcode-kernel/src/conformance/provider.rs:178-200`

```rust
pub fn check_stream_reconstruction(events: &[StreamEvent]) -> ConformanceReport {
    let mut groups: BTreeMap<u32, (Option<String>, Option<String>, String)> = BTreeMap::new();
    let mut complete: Vec<ToolCall> = Vec::new();
    for ev in events {
        match ev {
            StreamEvent::ToolCallDelta { index, id, name, arguments } => {
                let g = groups.entry(*index).or_insert((None, None, String::new()));
                g.0 = g.0.clone().or_else(|| Some(id.clone()));
                g.1 = g.1.clone().or_else(|| Some(name.clone()));
                g.2.push_str(arguments);
            }
            StreamEvent::ToolCall(tc) => complete.push(tc.clone()),
            // ...
        }
    }
    // 验证: 第 i 个 index 的拼接结果 == 第 i 个完整调用
}
```

**核心机制**: 验证流式工具调用的重建契约——按 index 拼接 `ToolCallDelta.arguments` 必须重现完整 `ToolCall`。完整调用按顺序与 delta index 相关联。

**设计意图**: 确保 index-buffering 解码器在 finish 时正确刷新。这是流处理正确性的核心契约。

**laew gap**: L871(无流重建验证)

### 8.6 流重连与尝试限制

**代码位置**: `crates/atomcode-capabilities/src/provider/ollama.rs:171-200`

```rust
const MAX_STREAM_ATTEMPTS: u32 = 3;  // 1 初始 + 2 重开
let mut stream_attempt = 1u32;
let mut reconnect_attempts = 0u32;
'reopen: loop {
    let mut dec = OllamaNdjsonDecoder::new();
    let mut emitted_replay_sensitive = false;
    let mut pending_metadata = Vec::new();
    // ...
}
```

**核心机制**: 流读取失败时最多 3 次透明重连。关键约束:仅在未发射 replay-sensitive 输出前可重放整个请求;元数据可重复,内容和工具数据不可。

**设计意图**: 注释解释"a gateway resetting connections under load can drop more than one attempt before a healthy backend answers"。`pending_metadata` 缓冲元数据直到首个 replay-sensitive 事件确认当前尝试有效。

**laew gap**: L872(无流重连机制)

---

## 9. 新增 gap 清单汇总

### P0 紧急(必须实现)

| gap 编号 | 描述 | 影响维度 | 参考位置 |
|---------|------|---------|---------|
| L836 | 无连接池空闲超时配置 | 网络协议 | `retry.rs:54` |
| L837 | 无 TLS 版本策略 | 网络协议 | `tls.rs:1-172` |
| L838 | 无 TLS 错误分类 | 网络协议 | `retry.rs:158-194` |
| L839 | 无陈旧连接检测 | 网络协议 | `retry.rs:103-156` |
| L842 | 无 shell 词法分析器 | 编译器前端 | `bash_workspace_gate.rs:209-250` |
| L843 | 无 AST 级命令分析 | 编译器前端 | `bash.rs:2176-2254` |
| L846 | 无信号安全恢复 | OS 内核 | `signal_restore.rs:1-123` |
| L847 | 无 termios 原始模式 | OS 内核 | `oauth.rs:283-325` |
| L853 | 无 Ollama 集成 | ML 推理 | `ollama.rs:1-125` |
| L857 | 无契约符合性框架 | 形式化验证 | `conformance/mod.rs:1-219` |
| L858 | 无 Provider 契约验证 | 形式化验证 | `conformance/provider.rs:23-122` |
| L862 | 无代码图数据结构 | 图数据库 | `codeintel/graph.rs:9-82` |
| L867 | 无字节级空闲超时 | 流处理 | `anthropic.rs:68-75` |

### P1 重要(应该实现)

| gap 编号 | 描述 | 影响维度 | 参考位置 |
|---------|------|---------|---------|
| L840 | 无空闲超时配置 | 网络协议 | `anthropic.rs:68-75` |
| L841 | 无应用归因机制 | 网络协议 | `openai_compat.rs:48-102` |
| L844 | 无代码图索引 | 编译器前端 | `codeintel/index.rs:14-131` |
| L845 | 无 HTML 分词器 | 编译器前端 | `web_fetch.rs:632-770` |
| L848 | 无 poll 非阻塞 I/O | OS 内核 | `oauth.rs:981-1048` |
| L849 | 无私有文件权限 | OS 内核 | `datalog.rs:7-13` |
| L854 | 无透明流重连 | ML 推理 | `ollama.rs:171-200` |
| L855 | 无视觉模型检测 | ML 推理 | `openai_compat.rs:170-191` |
| L856 | 无推理策略派生 | ML 推理 | `reasoning.rs:149` |
| L859 | 无流良构性检查 | 形式化验证 | `conformance/provider.rs:134-176` |
| L860 | 无工具调用重建验证 | 形式化验证 | `conformance/provider.rs:178-200` |
| L861 | 无 panic 契约验证 | 形式化验证 | `conformance/mod.rs:130-174` |
| L863 | 无 BFS 调用链追踪 | 图数据库 | `codeintel/graph.rs:153-185` |
| L864 | 无最短路径算法 | 图数据库 | `codeintel/graph.rs:188-200` |
| L865 | 无 @-mention 文件索引 | 图数据库 | `file_index.rs:14-178` |
| L866 | 无确定性符号 ID | 图数据库 | `codeintel/graph.rs:89-96` |
| L868 | 无广播事件分发 | 流处理 | `live_hub.rs:187-200` |
| L869 | 无滚动窗口限流 | 流处理 | `rate_limit.rs:84-115` |
| L871 | 无流重建验证 | 流处理 | `conformance/provider.rs:178-200` |
| L872 | 无流重连机制 | 流处理 | `ollama.rs:171-200` |

### P2 进阶(可以实现)

| gap 编号 | 描述 | 影响维度 | 参考位置 |
|---------|------|---------|---------|
| L850 | 无分布式共识实现 | 分布式共识 | N/A(架构不需要) |
| L851 | 无分布式事件总线 | 分布式共识 | `live_hub.rs:187-200` |
| L852 | 无会话代际追踪 | 分布式共识 | `live_hub.rs:36-43` |
| L870 | 无背压控制 | 流处理 | `tuix/src/lib.rs:656` |

---

## 分析总结

### 架构洞察

1. **协议层深度**: atomcode 对 HTTP/TLS 的理解远超基础用法——连接池调优(15s 空闲超时)、TLS 版本自动降级、陈旧连接三重检测、TLS 记录腐化识别,这些都是生产级网络编程的体现。

2. **编译器技术复用**: 手写 bash 词法分析器 + tree-sitter AST 解析的双轨策略,既保证了安全性(fail-closed),又避免了过度依赖。代码图构建是知识图谱技术在代码分析中的典型应用。

3. **内核级交互**: 信号安全终端恢复、termios 原始模式、poll 非阻塞 I/O,展示了 Unix 系统编程的深度。注释中"async-signal-safe calls ONLY"的强调体现了对 POSIX 语义的精确理解。

4. **形式化方法**: 契约符合性框架将"必须不 panic"、"流必须终止"等隐式假设显式化、可执行化。这是从"能跑"到"可靠"的关键跨越。

5. **ML 推理集成**: Ollama 本地推理支持 + 透明流重连 + 工具调用 id 合成,为离线/隐私场景提供了完整方案。

### 与 laew 的对比

- laew 当前聚焦于 Agent 编排层(多角色、工作流、熔断器),在网络协议、编译器前端、OS 内核交互等方面几乎空白
- atomcode 的契约符合性框架是 laew 缺失的关键质量保障机制
- atomcode 的代码图 + BFS/最短路径为 laew 的代码理解能力提供了参考模板
- atomcode 的信号安全恢复是 TUI 类应用的最佳实践

### 推荐优先级

1. **立即实现**: 连接池配置、TLS 版本策略、空闲超时(网络协议基础)
2. **短期实现**: 信号安全恢复、termios 原始模式(TUI 稳定性)
3. **中期实现**: 契约符合性框架(质量保障)、Ollama 集成(离线能力)
4. **长期实现**: 代码图索引(代码理解)、滚动窗口限流(配额管理)

---

**本工程新增 gap 共 37 个**(L836-L872):
- P0: 13 个
- P1: 20 个
- P2: 4 个

**累计 gap 总数**: L1-L835(前 14 轮) + L836-L872(第 15 轮) = **872 个 gap**
