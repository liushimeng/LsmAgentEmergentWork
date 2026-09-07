# 专题 — 第十二轮 CLI 框架与命令分发与自动补全深度对比（2026-09-07）

> **覆盖维度**：CLI 框架选择 / 命令注册与路由 / 参数解析 / 帮助系统 / 自动补全 / 交互式提示 / 错误处理 / 插件 CLI / 配置集成 / 脚本友好。
>
> **分析工程（6 个）**：atomcode / claudecode / openclaw / opencode / pi / cc-switch。
>
> **laew 差距分析**：laew 是基于 clap 4 的 Rust Agent CLI，使用 `clap::Parser`/`Subcommand` derive，模块组织简单但缺乏多档补全 / i18n / 中间件管道。
>
> **目标行数**：2000-4000 行，覆盖源码级实现细节。

---

## 〇、整体架构总览

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     Agent CLI 框架架构对比图                              │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐         │
│  │    atomcode     │   │   claudecode    │   │    openclaw     │         │
│  │  (Rust+clap 4)  │   │ (TS+Commander)  │   │  (TS+Commander) │         │
│  │                 │   │                 │   │                 │         │
│  │  ┌───────────┐  │   │  ┌───────────┐  │   │  ┌───────────┐  │         │
│  │  │cli/main.rs│  │   │  │ main.tsx  │  │   │  │run-main.ts│  │         │
│  │  │  4987行   │  │   │  │  4683行   │  │   │  │  1697行   │  │         │
│  │  └─────┬─────┘  │   │  └─────┬─────┘  │   │  └─────┬─────┘  │         │
│  │        │        │   │        │        │   │        │        │         │
│  │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │         │
│  │  │Cli derive │  │   │  │Commander  │  │   │  │OpenClawCmd│  │         │
│  │  │+Commands  │  │   │  │Command+   │  │   │  │ + 24 子CLI│  │         │
│  │  │  enum     │  │   │  │.command() │  │   │  │ + 插件CLI │  │         │
│  │  └─────┬─────┘  │   │  └─────┬─────┘  │   │  └─────┬─────┘  │         │
│  │        │        │   │        │        │   │        │        │         │
│  │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │         │
│  │  │clap_complete│ │   │  │mcp/auth/  │  │   │  │clack/     │  │         │
│  │  │ bash/zsh/  │  │   │  │plugin/... │  │   │  │prompts    │  │         │
│  │  │ fish/ps    │  │   │  │(动态导入) │  │   │  │+ node-    │  │         │
│  │  └───────────┘  │   │  └───────────┘  │   │  │readline   │  │         │
│  └─────────────────┘   └─────────────────┘   └─────────────────┘         │
│                                                                          │
│  ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐         │
│  │    opencode     │   │       pi        │   │   cc-switch     │         │
│  │ (TS+yargs 18)   │   │(TS+自研 parse) │   │  (Rust+Tauri 2) │         │
│  │                 │   │                 │   │                 │         │
│  │  ┌───────────┐  │   │  ┌───────────┐  │   │  ┌───────────┐  │         │
│  │  │ index.ts  │  │   │  │ args.ts   │  │   │  │ main.rs   │  │         │
│  │  │ + 24 cmd  │  │   │  │ 447行解析 │  │   │  │  Tauri 2  │  │         │
│  │  └─────┬─────┘  │   │  └─────┬─────┘  │   │  └─────┬─────┘  │         │
│  │        │        │   │        │        │   │        │        │         │
│  │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │         │
│  │  │effect-cmd │  │   │  │Command<T> │  │   │  │tauri::    │  │         │
│  │  │  Effect   │  │   │  │.option()  │  │   │  │command    │  │         │
│  │  │.cmd()     │  │   │  │.action()  │  │   │  │ IPC attr  │  │         │
│  │  └─────┬─────┘  │   │  └─────┬─────┘  │   │  └─────┬─────┘  │         │
│  │        │        │   │        │        │   │        │        │         │
│  │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │   │  ┌─────▼─────┐  │         │
│  │  │@clack/    │  │   │  │chalk +    │  │   │  │commands/  │  │         │
│  │  │prompts    │  │   │  │手写 help  │  │   │  │30 个 IPC  │  │         │
│  │  │+ FormatErr│  │   │  │函数       │  │   │  │ Tauri模块 │  │         │
│  │  └───────────┘  │   │  └───────────┘  │   │  └───────────┘  │         │
│  └─────────────────┘   └─────────────────┘   └─────────────────┘         │
│                                                                          │
│  ★ 关键差异：                                                            │
│  • Rust: derive macro (atomcode/cc-switch) / Rust Tauri IPC (cc-switch)  │
│  • TS Framework: Commander(2) / yargs(1) / 自研(1)                        │
│  • 自研范式（pi）= 类型化 Option<T> + ParseResult<T> union                │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 一、atomcode（Rust + clap 4 + clap_complete）

### 1.1 CLI 框架选择

**核心依赖**（`crates/atomcode-cli/Cargo.toml` + `crates/atomcode-clix/Cargo.toml`）：

```toml
# crates/atomcode-cli/Cargo.toml
clap = { version = "4", features = ["derive"] }
clap_complete = "4"

# crates/atomcode-clix/Cargo.toml
clap = { version = "4", features = ["derive"] }
```

**框架特性**：
- 使用 `clap::Parser`/`Subcommand`/`ValueEnum`/`Args` derive 宏
- 完整的 4 个子 crate：
  - `atomcode-cli`（4987 行，主二进制入口）
  - `atomcode-clix`（1852 行，独立 clix 工具，code/tel 子命令）
- 集成 `clap_complete` 自动生成 5 种 Shell 补全（Bash/Zsh/Fish/PowerShell/Elvish）

### 1.2 命令注册与路由

**主入口结构**（`crates/atomcode-cli/src/main.rs:692-919`）：

```rust
#[derive(Parser)]
#[command(name = BIN_NAME, version = VERSION, about = "AI coding assistant in your terminal")]
#[command(group(
    ArgGroup::new("headless_input")
        .args(["prompt", "prompt_file"])
        .multiple(false)
))]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    // ... 全局参数 ...
}

#[derive(Subcommand)]
enum Commands {
    Login,                                          // 子命令 1
    Logout,                                         // 子命令 2
    Status,                                         // 子命令 3
    Resume { session: Option<String> },             // 子命令 4（带位置参数）
    Upgrade { #[arg(long)] force: bool },           // 子命令 5
    Rollback,                                       // 子命令 6
    #[command(hide = true)]
    Codingplan,                                     // 隐藏别名（兼容旧版）
    #[command(subcommand)]
    Mcp(McpCli),                                    // 多级嵌套：mcp add/login/logout
    Daemon { port: u16, client: Option<String> },  // 守护进程
    Webui { port: u16, host: String },              // Web UI
    #[command(subcommand)]
    Telemetry(TelemetryAction),                     // 嵌套 enum
    #[command(subcommand)]
    Plugin(PluginCli),                              // 插件管理
    Uninstall { yes: bool, purge: bool, keep_data: bool, dry_run: bool },
    Setup { force: bool },
    #[command(subcommand)]
    Hooks(HookCommands),                            // Hook 管理
    #[command(subcommand)]
    Schedule(schedule_cmd::ScheduleCli),            // 调度任务
    Completion(CompletionCommand),                  // 补全脚本生成
    #[command(name = "__askpass", hide = true)]
    Askpass { prompt: String },                     // 隐藏 askpass 辅助
    #[command(hide = true)]
    Acp,                                            // ACP 协议隐藏入口
}
```

**多级嵌套命令**（`main.rs:1091-1180`）：

```rust
#[derive(Subcommand)]
enum McpCli {
    /// Add or replace a stdio MCP server
    Add {
        name: String,
        #[arg(required = true, num_args = 1..)]   // 至少一个位置参数
        command: Vec<String>,
        #[arg(long)]
        global: bool,
        #[arg(short = 'C', long, value_hint = clap::ValueHint::DirPath)]
        dir: Option<PathBuf>,
    },
    /// Add GitHub's remote MCP server using OAuth
    AddGithubOauth {
        #[arg(default_value = "github")]
        name: String,
        // ... OAuth 流程参数 ...
    },
    Login { name: String, provider: String, client_id: Option<String>, ... },
    Logout { name: String },
}
```

**关键设计**：
- **隐藏命令**：`#[command(hide = true)]` 用于内部子命令（`Codingplan`、`Acp`、`__askpass`）
- **子命令分组**：`ArgGroup` 用于互斥参数（如 `-p` 与 `--prompt-file`）
- **环境变量后备**：`#[arg(env = "ATOMCODE_*")]` 不直接使用，但有 `scan_argv_for_lang` 等预扫描机制

### 1.3 参数解析

**完整 Cli 结构**（`main.rs:699-805`）：

```rust
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short = 'c', long = "continue")]
    continue_last: bool,

    #[arg(long = "resume", value_name = "ID_OR_NAME", conflicts_with_all = ["continue_last", "ephemeral"])]
    resume: Option<String>,

    #[arg(long)] provider: Option<String>,
    #[arg(long)] model: Option<String>,
    #[arg(long)] lang: Option<String>,

    #[arg(long, value_hint = clap::ValueHint::FilePath)]
    config: Option<PathBuf>,

    #[arg(long, value_name = "PATH", value_hint = clap::ValueHint::FilePath)]
    seed_config: Option<PathBuf>,

    #[arg(long, short = 'C', value_hint = clap::ValueHint::DirPath)]
    dir: Option<PathBuf>,

    #[arg(short = 'p', long)]
    prompt: Option<String>,

    #[arg(long, value_name = "PATH", conflicts_with = "prompt", value_hint = clap::ValueHint::FilePath)]
    prompt_file: Option<std::path::PathBuf>,

    #[arg(long, requires = "headless_input", conflicts_with = "continue_last")]
    ephemeral: bool,

    #[arg(long, requires = "headless_input")]
    no_tools: bool,

    #[arg(long, value_enum, default_value_t = HeadlessOutputFormat::Text, requires = "headless_input")]
    output_format: HeadlessOutputFormat,

    #[arg(short = 'v', long)] verbose: bool,
    #[arg(long)] dev: bool,
    #[arg(long = "no-telemetry", default_value_t = false, global = true)]
    pub no_telemetry: bool,

    #[arg(short = 'y', long = "dangerously-skip-permissions", default_value_t = false)]
    pub dangerously_skip_permissions: bool,
}
```

**关键参数特性**：
- **互斥**：`conflicts_with_all = ["continue_last", "ephemeral"]`
- **必填链**：`requires = "headless_input"`
- **枚举**：`value_enum` 自动派生 shell 补全
- **默认值**：`default_value_t = HeadlessOutputFormat::Text`
- **全局**：`global = true`（`--no-telemetry` 可作用于所有子命令）
- **文件路径提示**：`value_hint = clap::ValueHint::FilePath`（shell 补全触发文件路径补全）
- **值命名**：`value_name = "ID_OR_NAME"`（帮助文本显示）

### 1.4 帮助系统

**i18n 国际化帮助文本**（`main.rs:360-400`）：

```rust
/// Build the top-level clap Command with i18n-localised about and help text.
/// This replaces the default Cli::parse() flow so that --help output
/// respects the current locale (set by scan_argv_for_lang above).
fn build_i18n_command() -> clap::Command {
    use atomcode_tuix::i18n::{t, Msg};

    let cmd = Cli::command();

    // Mutate the top-level about
    let cmd = cmd.about(t(Msg::CliAbout).into_owned());

    // Mutate top-level argument help texts
    let cmd = cmd
        .mut_arg("continue_last", |a| a.help(t(Msg::CliHelpContinue).into_owned()))
        .mut_arg("provider", |a| a.help(t(Msg::CliHelpProvider).into_owned()))
        .mut_arg("model", |a| a.help(t(Msg::CliHelpModel).into_owned()))
        .mut_arg("lang", |a| a.help(t(Msg::CliHelpLang).into_owned()))
        // ... 约 20 个 mut_arg 调用
        .mut_arg("dangerously_skip_permissions", |a| {
            a.help(t(Msg::CliHelpDangerouslySkipPermissions).into_owned())
        });

    // Mutate subcommand about texts
    let cmd = cmd
        .mut_subcommand("login", |s| s.about(t(Msg::CliAboutLogin).into_owned()))
        .mut_subcommand("logout", |s| s.about(t(Msg::CliAboutLogout).into_owned()))
        .mut_subcommand("status", |s| s.about(t(Msg::CliAboutStatus).into_owned()))
        // ... 全部 13 个子命令的 about
        .mut_subcommand("hooks", |s| s.about(t(Msg::CliAboutHooks).into_owned()));

    cmd
}
```

**预扫描 --lang**（`main.rs:293-310`）：

```rust
/// Scan argv by hand to extract the value of --lang <VALUE> or --lang=VALUE.
/// This runs BEFORE clap parses the arguments, so that the i18n locale
/// can be set in time for clap to render localised --help text.
fn scan_argv_for_lang() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--lang" && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
        if let Some(val) = args[i].strip_prefix("--lang=") {
            return Some(val.to_string());
        }
        i += 1;
    }
    None
}
```

### 1.5 自动补全

**Shell 补全生成**（`main.rs:927-1027`）：

```rust
#[derive(clap::Args)]
struct CompletionCommand {
    /// Shell to generate completions for.
    #[arg(value_enum, default_value_t = Shell::Bash)]
    shell: Shell,
}

fn completion_command() -> clap::Command {
    let source = Cli::command();
    let visible_subcommands = source
        .get_subcommands()
        .filter(|command| !command.is_hide_set())    // 过滤隐藏命令
        .cloned()
        .collect::<Vec<_>>();

    clap::Command::new(BIN_NAME)
        .version(VERSION)
        .about("AI coding assistant in your terminal")
        .args(source.get_arguments().cloned())
        .groups(source.get_groups().cloned())
        .subcommands(visible_subcommands)
}

fn print_shell_completion(shell: Shell, out: &mut dyn Write) {
    let mut command = completion_command();
    clap_complete::generate(shell, &mut command, BIN_NAME, out);
}
```

**补全快速路径**（`main.rs:941-957, 1182`）：

```rust
fn try_print_shell_completion() -> bool {
    if !is_completion_invocation(std::env::args_os().skip(1)) {
        return false;
    }
    match Cli::try_parse() {
        Ok(Cli { command: Some(Commands::Completion(command)), .. }) => {
            print_shell_completion(command.shell, &mut std::io::stdout());
            true
        }
        Ok(_) => false,
        Err(error) => error.exit(),
    }
}

fn main() {
    // Completion generation must be a pure, fast CLI operation: no helper
    // thread, Tokio runtime, log file, config read, telemetry, or updater.
    // Shells may invoke completion helpers frequently, so even best-effort
    // startup work here would turn Tab into network/filesystem activity.
    if try_print_shell_completion() {
        return;
    }
    // ...
}
```

**支持的 Shell**：`Shell::Bash / Zsh / Fish / PowerShell / Elvish`（来自 `clap_complete::Shell` 枚举）

### 1.6 交互式提示

**Askpass 安全密码输入**（`crates/atomcode-cli/src/askpass.rs:1-50`）：

```rust
//! Blocking helper for the `atomcode __askpass` subcommand.
//!
//! Connects to the askpass Unix-domain socket, sends a `Request` frame
//! (nonce token + prompt text), and reads back a `Response` frame that
//! contains the password.  Uses blocking `std` I/O — no async runtime —
//! because the helper is a tiny short-lived process invoked by sudo/ssh.

#[cfg(unix)]
pub fn run_askpass(prompt: &str, sock: &Path, token: &str) -> Option<String> {
    let mut stream = UnixStream::connect(sock).ok()?;
    let req = Request {
        token: token.to_owned(),
        prompt: prompt.to_owned(),
    };
    write_frame(&mut stream, &req).ok()?;
    let mut reader = BufReader::new(stream);
    let resp: Response = read_frame(&mut reader).ok()?;
    resp.password
}
```

**设计要点**：
- 独立子进程：`__askpass` 通过 Unix Domain Socket 与 TUI 主进程通信
- 二进制帧协议：`Request{token, prompt}` → `Response{password}`
- 进程隔离：避免密码暴露在主进程内存中
- 阻塞 IO：短生命周期进程，无需异步运行时

### 1.7 错误处理

**辅助进程**：`__askpass` 隐藏子命令处理 I/O 错误：

```rust
match atomcode_updater::prepare_deferred_upgrade(&current, tx).await {
    Ok(_) => 0,
    Err(_) => 1,
}
```

### 1.8 插件 CLI

**Plugin 子命令**（`main.rs:1073-1089`）：

```rust
#[derive(Subcommand)]
enum PluginCli {
    /// Marketplace registry operations (add/remove/update/list).
    #[command(subcommand)]
    Marketplace(MarketplaceCli),
    /// Install a plugin from a registered marketplace.
    /// Spec format: `<plugin>@<marketplace>` (matches the slash command).
    Install {
        /// e.g. `ascend-model-agent-plugin@ascend-model-agent-plugin`
        spec: String,
    },
    Uninstall { spec: String },
    /// Trust an installed plugin's hooks so they run
    Trust { name: String },
    Untrust { name: String },
    List,
}

#[derive(Subcommand)]
enum MarketplaceCli {
    Add { url: String },        // Git URL
    Remove { name: String },
    Update { name: String },
    List,
}
```

**关键设计**：
- 命名空间 `<plugin>@<marketplace>` 格式（与 TUI `/plugin` 一致）
- `Trust/Untrust` 操作 hook-set-hash 内容寻址

### 1.9 配置集成

**优先级链**（从代码注释 + 配置文件读取）：

```rust
#[arg(long, value_hint = clap::ValueHint::FilePath)]
config: Option<PathBuf>,

#[arg(long, value_name = "PATH", value_hint = clap::ValueHint::FilePath)]
seed_config: Option<PathBuf>,    // FIRST-RUN ONLY seed
```

**PreScanConfig**（`main.rs:323-356`）：在 clap 解析之前预扫描配置文件的 3 个字段（language、brand_name、oauth_provider_name），用于本地化 --help 渲染。

### 1.10 脚本友好

**Headless 模式**（`main.rs:744-803`）：
- `-p <prompt>` / `--prompt-file <path>` 触发非交互模式
- `--output-format text|jsonl`（`HeadlessOutputFormat` 枚举）
- `--no-tools`（headless only，`requires = "headless_input"`）
- `--ephemeral`（headless only）
- 退出码语义化：`Ok(_) => 0, Err(_) => 1`

---

## 二、claudecode（TypeScript + Commander 15 + @commander-js/extra-typings）

### 2.1 CLI 框架选择

**核心依赖**：
- `@commander-js/extra-typings`（Commander 15 的强类型扩展）
- `commander`（运行时）
- `bun:bundle`（feature gating）

**框架特性**：
- **强类型**：通过 `@commander-js/extra-typings` 提供 `Command<string, Options>` 泛型
- **特性分片**：`feature('CHICAGO_MCP')` 等 bundle-time 标志控制 `--computer-use-mcp` 等子命令是否启用
- **极简 fast-path**：`--version`/`-v`/`-V` 在导入任何模块前直接处理

**Fast-path 启动**（`src/entrypoints/cli.tsx:34-37`）：

```typescript
const args = process.argv.slice(2);
// Fast-path for --version/-v: zero module loading needed
if (args.length === 1 && (args[0] === '--version' || args[0] === '-v' || args[0] === '-V')) {
  // ... 直接打印版本退出，不加载任何模块
}
```

### 2.2 命令注册与路由

**主程序构建**（`src/main.tsx:902-903, 968-1006`）：

```typescript
const program = new CommanderCommand()
  .configureHelp(createSortedHelpConfig())
  .enablePositionalOptions();

program.name('claude')
  .description(`Claude Code - starts an interactive session by default, use -p/--print for non-interactive output`)
  .argument('[prompt]', 'Your prompt', String)
  .helpOption('-h, --help', 'Display help for command')
  .option('-d, --debug [filter]', 'Enable debug mode with optional category filtering...')
  .addOption(new Option('-d2e, --debug-to-stderr', 'Enable debug mode (to stderr)').argParser(Boolean).hideHelp())
  .option('--debug-file <path>', 'Write debug logs to a specific file path...')
  .option('-p, --print', 'Print response and exit (useful for pipes)...', () => true)
  .option('--bare', 'Minimal mode: skip hooks, LSP, plugin sync...', () => true)
  .addOption(new Option('--output-format <format>', 'Output format (only works with --print)')
    .choices(['text', 'json', 'stream-json']))
  .addOption(new Option('--json-schema <schema>', 'JSON Schema for structured output validation...')
    .argParser(String))
  .option('-c, --continue', 'Continue the most recent conversation...', () => true)
  .option('-r, --resume [value]', 'Resume a conversation by session ID...', value => value || true)
  .option('--model <model>', 'Model for the current session...')
  .option('--effort <level>', `Effort level for the current session (low, medium, high, max)`)
  .option('--plugin-dir <path>', 'Load plugins from a directory...', (val: string, prev: string[]) => [...prev, val], [] as string[])
  .option('--setting-sources <sources>', 'Comma-separated list of setting sources to load (user, project, local).')
  .action(async (prompt, options) => { /* main action */ });

program.version(`${MACRO.VERSION} (Claude Code)`, '-v, --version', 'Output the version number');
```

**子命令注册**（`src/main.tsx:3894-4148` 多个 .command() 调用）：

```typescript
// mcp 子命令
const mcp = program.command('mcp').description('Configure and manage MCP servers')
  .configureHelp(createSortedHelpConfig()).enablePositionalOptions();
mcp.command('serve').description(`Start the Claude Code MCP server`).action(...)
mcp.command('remove <name>').description('Remove an MCP server').option('-s, --scope <scope>', ...).action(...)
mcp.command('list').description('List configured MCP servers...').action(...)
mcp.command('get <name>').description('Get details about an MCP server...').action(...)
mcp.command('add-json <name> <json>').description('Add an MCP server...').action(...)
mcp.command('add-from-claude-desktop').description('Import MCP servers from Claude Desktop...').action(...)
mcp.command('reset-project-choices').description('Reset all approved and rejected project-scoped (.mcp.json) servers...').action(...)

// server 子命令（远程服务）
program.command('server').description('Start a Claude Code session server')
  .option('--port <number>', 'HTTP port', '0')
  .option('--host <string>', 'Bind address', '0.0.0.0')
  .option('--auth-token <token>', 'Bearer token for auth')
  .option('--unix <path>', 'Listen on a unix domain socket')
  .option('--workspace <dir>', 'Default working directory for sessions...')
  .option('--idle-timeout <ms>', 'Idle timeout for detached sessions in ms (0 = never expire)', '600000')
  .option('--max-sessions <n>', 'Maximum concurrent sessions (0 = unlimited)', '32')
  .action(async (opts) => { ... });

// ssh 远程命令
program.command('ssh <host> [dir]').description('Run Claude Code on a remote host over SSH...')
  .option('--permission-mode <mode>', 'Permission mode for the remote session')
  .option('--dangerously-skip-permissions', 'Skip all permission prompts on the remote...')
  .option('--local', 'e2e test mode — spawn the child CLI locally...')
  .action(async () => { ... });

// auth 子命令（嵌套）
const auth = program.command('auth').description('Manage authentication')
  .configureHelp(createSortedHelpConfig());
auth.command('login').description('Sign in to your Anthropic account')
  .option('--email <email>', 'Pre-populate email address on the login page')
  .option('--sso', 'Force SSO login flow')
  .option('--console', 'Use Anthropic Console (API usage billing) instead of Claude subscription')
  .option('--claudeai', 'Use Claude subscription (default)')
  .action(async ({...}) => { ... });
auth.command('status').description('Show authentication status')
  .option('--json', 'Output as JSON (default)')
  .option('--text', 'Output as human-readable text')
  .action(async (opts) => { ... });
auth.command('logout').description('Log out from your Anthropic account').action(async () => { ... });

// plugin 子命令（带别名）
const pluginCmd = program.command('plugin').alias('plugins')
  .description('Manage Claude Code plugins')
  .configureHelp(createSortedHelpConfig());
```

**关键设计**：
- **统一别名**：`pluginCmd.alias('plugins')`
- **强类型 action**：`action(async (name: string, options: { scope?: string }) => {...})`
- **懒加载**：`const { completionHandler } = await import('./cli/handlers/ant.js')`
- **feature gating**：`feature('CHICAGO_MCP')` 在 bundle 时剔除未启用子命令

### 2.3 参数解析

**完整选项列表**（共 70+ 选项，节选自 `main.tsx:968-1006`）：

| 选项 | 简写 | 类型 | 默认 | 说明 |
|------|------|------|------|------|
| `--debug [filter]` | `-d` | string\|true | - | 调试模式 + 过滤 |
| `--debug-to-stderr` | `-d2e` | boolean | - | 调试到 stderr |
| `--debug-file <path>` | - | flag | - | 写入文件 |
| `--print` | `-p` | flag | false | 非交互模式 |
| `--bare` | - | flag | false | 最小模式 |
| `--output-format <format>` | - | enum | text | text/json/stream-json |
| `--json-schema <schema>` | - | string | - | JSON Schema 校验 |
| `--continue` | `-c` | flag | false | 续接最近会话 |
| `--resume [value]` | `-r` | string\|true | - | 恢复会话 |
| `--model <model>` | - | string | - | 模型别名/全名 |
| `--effort <level>` | - | enum | - | 投入度 (low/medium/high/max) |
| `--permission-mode <mode>` | - | enum | - | 权限模式 |
| `--thinking <mode>` | - | enum | - | 思考 (enabled/adaptive/disabled) |
| `--max-turns <turns>` | - | number | - | 最大轮数（headless only） |
| `--max-budget-usd <amount>` | - | number | - | 最大预算 USD（headless only） |
| `--allowedTools <tools>` | - | string[] | - | 工具白名单 |
| `--tools <tools>` | - | string[] | - | 工具列表 |
| `--disallowedTools <tools>` | - | string[] | - | 工具黑名单 |
| `--mcp-config <configs>` | - | string[] | - | MCP 配置 |
| `--system-prompt <prompt>` | - | string | - | 系统提示词 |
| `--append-system-prompt <prompt>` | - | string | - | 追加系统提示词 |
| `--plugin-dir <path>` | - | string[] | [] | 插件目录（可重复） |
| `--add-dir <directories>` | - | string[] | - | 允许的额外目录 |
| `--from-pr [value]` | - | string\|true | - | 从 PR 恢复 |
| `--session-id <uuid>` | - | string | - | 指定 session ID |

**argParser 校验**：
- `argParser(Number)` - 数字校验
- `argParser(Boolean)` - 布尔校验
- `.choices(['text', 'json', 'stream-json'])` - 枚举校验
- `argParser((rawValue: string) => {...})` - 自定义校验器

**累加器模式**：
```typescript
.option('--plugin-dir <path>', 'Load plugins from a directory for this session only (repeatable: --plugin-dir A --plugin-dir B)',
  (val: string, prev: string[]) => [...prev, val], [] as string[])
```

### 2.4 帮助系统

**自定义 Help Config**（`main.tsx:902`）：

```typescript
const program = new CommanderCommand()
  .configureHelp(createSortedHelpConfig())   // 排序帮助选项
  .enablePositionalOptions();
```

**隐藏帮助**：
- `.hideHelp()` 用于内部选项（如 `--init-only`、`--maintenance`）
- `program.command('completion <shell>', { hidden: true })` 隐藏完成命令

**Examples 块**：通过 `--bare`、`--max-turns <turns>` 等详尽的 help 文本内嵌示例和默认值。

### 2.5 自动补全

**Completion 子命令**（`main.tsx:4492-4502`）：

```typescript
// claude completion <shell>
program.command('completion <shell>', {
  hidden: true
}).description('Generate shell completion script (bash, zsh, or fish)')
  .option('--output <file>', 'Write completion script directly to a file instead of stdout')
  .action(async (shell: string, opts: { output?: string }) => {
    const { completionHandler } = await import('./cli/handlers/ant.js');
    await completionHandler(shell, opts, program);
  });
```

**特点**：
- 隐藏命令（`hidden: true`）
- 支持 `--output <file>` 直接写入文件
- 懒加载 handler 模块（避免主进程导入）

### 2.6 交互式提示

**REPL 交互模式**：通过 Ink 渲染（`@inkjs/ink`），非传统 readline prompt。

**认证流程**：auth 子命令走 OAuth Device Flow + Console 网页登录，不使用传统密码输入。

**Bridge 远程控制**：通过 `--remote` 模式接受外部命令（见 `cli/handlers/` 目录）。

### 2.7 错误处理

**exitOverride 与 CommanderError**：
- Commander 错误自动捕获，输出到 stderr 并以非零退出码退出
- `--json-schema` 校验失败：输出 JSON 错误到 stdout（`--output-format json`）

**错误格式**：
- `FormatError(e)` 函数统一格式化异常
- `throw err` 传播到顶层 `.fail()` 处理器

### 2.8 插件 CLI

**Plugin 子命令**：
```typescript
const pluginCmd = program.command('plugin').alias('plugins')
  .description('Manage Claude Code plugins')
  .configureHelp(createSortedHelpConfig());
```

**插件命令动态注入**：
- 通过 `/plugin-dir` 在每次启动时加载插件目录
- 插件可注册 slash command（`/plugin-name`），不是 CLI 子命令
- CLI 层面仅暴露 `plugin list/install/uninstall` 等管理命令

### 2.9 配置集成

**配置文件优先级链**（启动时扫描）：
1. `--settings <file-or-json>` CLI 参数（最高优先级）
2. Managed settings（企业策略）
3. User settings（`~/.claude/settings.json`）
4. Project settings（`.claude/settings.json`）
5. Plugin settings

**CLI 集成点**：
```typescript
.option('--settings <file-or-json>', 'Path to a settings JSON file or a JSON string to load additional settings from')
.option('--setting-sources <sources>', 'Comma-separated list of setting sources to load (user, project, local).')
```

### 2.10 脚本友好

**Headless 模式**（`--print` / `-p`）：
- `--output-format text` - 默认纯文本
- `--output-format json` - 单次 JSON
- `--output-format stream-json` - 流式 NDJSON（与 `--input-format stream-json` 配对）
- `--json-schema <schema>` - 结构化输出校验
- `--max-turns <turns>` - 早退出（自动化流水线）
- `--max-budget-usd <amount>` - 成本限制

**结构化输入**：`--input-format stream-json` + `--replay-user-messages` 用于 stdin/stdout 双向协议。

**Bridge 远程控制**：通过 `--remote` + WebSocket/HTTP 服务暴露给外部客户端（iOS/Android/Web）。

---

## 三、openclaw（TypeScript + Commander 15 + 自研 OpenClawCommand 子类）

### 3.1 CLI 框架选择

**核心依赖**（`package.json`）：
```json
{
  "commander": "15.0.0",
  "@clack/prompts": "1.7.0"
}
```

**框架特性**：
- 基于 Commander 15，扩展 `OpenClawCommand` 子类捕获解析错误
- `@clack/prompts` 提供完整的交互式 UI（confirm/select/text/password/multiselect）
- 完整的 lazy-import 策略：24+ 子 CLI 按需加载

**主入口**（`src/cli/run-main.ts:991-1037`）：

```typescript
export async function runCli(
  argv: string[] = process.argv,
  options: {
    additionalStartupTrace?: ReturnType<typeof createGatewayDispatchStartupTrace>;
    retainConsoleRoutingUntilProcessExit?: boolean;
  } = {},
) {
  const originalArgv = normalizeWindowsArgv(argv);
  const builtInMachineOutput = resolveBuiltInMachineOutput(originalArgv);
  return await withConsoleLogsRoutedToStderrForJson(
    originalArgv,
    () => {
      const run = async (harnessCleanup?: CliHarnessCleanup) => {
        try {
          return await runCliWithPreparedOutputMode(originalArgv, {
            ...options,
            builtInMachineOutput,
            harnessCleanup,
          });
        } catch (error) {
          if (isGatewayRunInvocationArgv(originalArgv) &&
              !resolveCliArgvInvocation(originalArgv).hasHelpOrVersion) {
            const { handleGatewayStartupMaintenance } =
              await import("./gateway-cli/startup-maintenance.js");
            if (await handleGatewayStartupMaintenance(error)) return;
          }
          throw error;
        }
      };
      const gatewayRun = isGatewayRunInvocationArgv(originalArgv);
      return withCliCommandCleanup(gatewayRun, (cleanup) =>
        gatewayRun ? run() : withPluginCache(createPluginCache(), () => run(cleanup)),
      );
    },
    { machineOutput: builtInMachineOutput, restoreChanges: true,
      retainRoutingUntilProcessExit: options.retainConsoleRoutingUntilProcessExit },
  );
}
```

### 3.2 命令注册与路由

**Command Registry**（`src/cli/program/command-registry-core.ts`）：

```typescript
const coreEntrySpecs: readonly CommandGroupDescriptorSpec<[ctx: ProgramContext]>[] = [
  [["setup", "crestodian"], async (program) => (await import("./register.setup.js")).registerSetupCommand(program)],
  [["onboard"], async (program) => (await import("./register.onboard.js")).registerOnboardCommand(program)],
  [["configure"], async (program) => (await import("./register.configure.js")).registerConfigureCommand(program)],
  [["config"], async (program) => (await import("../config-cli.js")).registerConfigCli(program)],
  [["claws"], async (program) => (await import("../claws-cli.js")).registerClawsCli(program)],
  [["backup"], async (program) => (await import("./register.backup.js")).registerBackupCommand(program)],
  [["database"], async (program) => (await import("./register.database.js")).registerDatabaseCommand(program)],
  [["migrate"], async (program) => (await import("./register.migrate.js")).registerMigrateCommand(program)],
  [["audit"], async (program) => (await import("./register.audit.js")).registerAuditCommand(program)],
  [["doctor", "triage", "dashboard", "reset", "uninstall"],
    async (program) => (await import("./register.maintenance.js")).registerMaintenanceCommands(program)],
  [["message"], async (program, ctx) => (await import("./register.message.js")).registerMessageCommands(program, ctx)],
  [["mcp"], async (program) => (await import("../mcp-cli.js")).registerMcpCli(program)],
  [["transcripts"], async (program) => (await import("./register.transcripts.js")).registerTranscriptsCli(program)],
  [["agent"], async (program, ctx) => (await import("./register.agent-turn.js")).registerAgentTurnCommand(program, {
    agentChannelOptions: ctx.agentChannelOptions,
  })],
  [["agents"], async (program) => (await import("./register.agent.js")).registerAgentsCommands(program)],
  [["status", "health", "sessions", "tasks"],
    async (program) => (await import("./register.status-health-sessions.js")).registerStatusHealthSessionsCommands(program)],
  // ... 共 24+ 个命令组
];
```

**Build Program**（`src/cli/program/build-program.ts`）：

```typescript
export function buildProgram() {
  const program = new OpenClawCommand();
  program.enablePositionalOptions();
  // Preserve Commander-computed exit codes while still aborting parse flow.
  // Without this, unknown nested commands can print an error
  // but still report success when exits are intercepted.
  program.exitOverride((err) => {
    process.exitCode = typeof err.exitCode === "number" ? err.exitCode : 1;
    throw err;
  });
  const ctx = createProgramContext();
  const argv = process.argv;

  setProgramContext(program, ctx);
  configureProgramHelp(program, ctx);
  registerPreActionHooks(program, ctx.programVersion);

  registerProgramCommands(program, ctx, argv);

  return program;
}
```

**关键设计**：
- **命令组（Command Group）**：每个子命令是一组（name1, name2, ...）+ factory 闭包
- **懒加载**：使用 `await import(...)` 动态导入，仅在用户实际调用时加载
- **preaction**：全局前处理钩子（`registerPreActionHooks`）
- **argv 预解析**：基于 `resolveCliArgvInvocation` 的 fast-path

### 3.3 参数解析

**全局根选项**（`src/cli/program/help.ts:90-110`）：

```typescript
program
  .name(CLI_NAME)
  .description("")
  .version(ctx.programVersion)
  .option("--container <name>", "Run the CLI inside a running Podman/Docker container named <name>...")
  .option("--dev", "Dev profile: isolate state under ~/.openclaw-dev, default gateway port 19001...")
  .option("--profile <name>", "Use a named profile (isolates OPENCLAW_STATE_DIR/OPENCLAW_CONFIG_PATH under ~/.openclaw-<name>)")
  .option("--log-level <level>", `Global log level override for file + console (${CLI_LOG_LEVEL_VALUES})`,
    parseCliLogLevelOption);
```

**Argv 自定义解析器**（`src/cli/argv.ts`）：提供 `hasFlag`、`getFlagValue`、`getPositiveIntFlagValue`、`getVerboseFlag` 等工具函数。

**重复值收集**（`src/cli/program/route-args.ts:24-40`）：

```typescript
function parseRepeatedFlagValues(argv: string[], name: string): string[] | null {
  const values: string[] = [];
  const args = argv.slice(2);
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (!arg || arg === "--") break;
    if (arg === name) {
      const next = args[i + 1];
      if (next === undefined || !isValueToken(next)) {
        // Invalid fast-path shapes fall back to Commander so its normal errors and help text win.
        return null;
      }
      values.push(next);
      i += 1;
      continue;
    }
    if (arg.startsWith(`${name}=`)) {
      const value = arg.slice(name.length + 1).trim();
      if (!value) return null;
      values.push(value);
    }
  }
  return values;
}
```

### 3.4 帮助系统

**自定义帮助格式化**（`src/cli/program/help.ts:60-72`）：

```typescript
export function formatProgramHelpOutput(str: string): string {
  let output = str;
  const isRootHelp = new RegExp(
    `^Usage:\\s+${CLI_NAME_PATTERN}\\s+\\[options\\]\\s+\\[command\\]\\s*$`,
    "m",
  ).test(output);
  if (isRootHelp && /^Commands:/m.test(output)) {
    output = output.replace(/^Commands:/m, `Commands:\n  ${theme.muted(ROOT_COMMANDS_HINT)}`);
  }
  return output
    .replace(/^Usage:/gm, theme.heading("Usage:"))
    .replace(/^Options:/gm, theme.heading("Options:"))
    .replace(/^Commands:/gm, theme.heading("Commands:"));
}
```

**Examples 块**（`help.ts:42-58`）：

```typescript
const EXAMPLES = [
  ["openclaw onboard", "Run guided setup for a local Gateway, workspace, auth, and channels."],
  ["openclaw setup", "Create the baseline config, workspace, and session folders."],
  ["openclaw configure", "Change models, Gateway, channels, plugins, skills, and health checks."],
  ["openclaw status", "Check Gateway, channel, model, and recent-session status."],
  ["openclaw doctor --fix", "Repair common config, service, plugin, and channel problems."],
  ["openclaw channels add", "Add or update a chat channel account with guided prompts."],
  ["openclaw channels status", "See connected messaging accounts and login state."],
  ["openclaw --dev gateway", "Run a dev Gateway (isolated state/config) on ws://127.0.0.1:19001."],
  ["openclaw gateway run --force", "Start the Gateway and replace anything bound to its port."],
  ["openclaw models status", "Show model/provider auth health before running agents."],
  ["openclaw plugins list", "Inspect enabled, disabled, and installed plugins."],
  // ... 共 13 个示例
] as const;
```

**预计算 Help**（`src/cli/precomputed-help.ts`）：对所有命令静态计算帮助文本，避免运行时延迟。

### 3.5 自动补全

**Completion CLI**（`src/cli/completion-cli.ts`，598 行）：

```typescript
import { Command, Option } from "commander";
import { generateBashCompletion } from "./completion-bash.js";
import { buildFishOptionCompletionLine, buildFishSubcommandCompletionLine } from "./completion-fish.js";
import { COMPLETION_SHELLS, COMPLETION_SKIP_PLUGIN_COMMANDS_ENV, installCompletion,
  isCompletionShell, resolveCompletionCachePath, resolveShellFromEnv } from "./completion-runtime.js";

export function getCompletionScript(shell: CompletionShell, program: Command): string {
  if (shell === "zsh") return generateZshCompletion(program);
  if (shell === "bash") return generateBashCompletion(program);
  if (shell === "powershell") return generatePowerShellCompletion(program);
  return generateFishCompletion(program);
}
```

**支持 Shell**：`COMPLETION_SHELLS = ["zsh", "bash", "powershell", "fish"] as const`

**关键特性**：
- **Fish 路径辅助**：自动生成 `function __openclaw_command_path_matches` 帮助函数
- **缓存路径**：`~/.openclaw/completions/openclaw.{bash,zsh,fish,ps1}`
- **编码处理**：PowerShell 在 Windows 上自动检测 UTF-8 BOM/UTF-16 LE/BE
- **Profile 安装**：`installCompletion` 自动写入用户的 shell profile（.bashrc/.zshrc/config.fish/Microsoft.PowerShell_profile.ps1）
- **跳过插件命令**：`OPENCLAW_COMPLETION_SKIP_PLUGIN_COMMANDS=1` 环境变量可剔除插件命令以加速补全
- **自动 Shell 检测**：`resolveShellFromEnv` 根据 `$SHELL` 环境变量选择默认 Shell

**Completion Command Tree**（`src/cli/completion-command-tree.ts`，103 行）：

```typescript
export const completionFlags = ...;
export const commandNameVariants = ...;
export const visibleCompletionCommands = ...;
export const collectShellCompletionCommandTree = ...;
```

### 3.6 交互式提示

**核心 Prompts**（`src/wizard/clack-prompter.ts`，使用 `@clack/prompts`）：

```typescript
import { autocomplete, autocompleteMultiselect, cancel, confirm, intro,
  isCancel, multiselect, password, select, settings, spinner, text } from "@clack/prompts";

// Clack-backed WizardPrompter implementation for interactive CLI setup
function guardCancel<T>(value: T | symbol, output: NodeJS.WriteStream, signal?: AbortSignal): T {
  if (isCancel(value)) {
    if (!signal?.aborted) {
      cancel(stylePromptTitle("Setup cancelled.") ?? "Setup cancelled.", { output });
    }
    throw new WizardCancelledError();
  }
  return value;
}
```

**支持的提示类型**：
- `text` - 文本输入
- `password` - 密码输入（隐藏显示）
- `confirm` - 确认（Y/n）
- `select` - 单选
- `multiselect` - 多选
- `autocomplete` - 自动补全（搜索式）
- `autocompleteMultiselect` - 多选自动补全
- `spinner` - 进度指示器

**简化 Prompt**（`src/cli/prompt.ts`，66 行）：

```typescript
import readline from "node:readline/promises";

/** Prompts for yes/no input, honoring global `--yes` before opening stdin. */
export async function promptYesNo(question: string, defaultYes = false): Promise<boolean> {
  if (isVerbose() && isYes()) return true;
  if (isYes()) return true;
  const rl = readline.createInterface({ input, output });
  const suffix = defaultYes ? " [Y/n] " : " [y/N] ";
  const answer = normalizeLowercaseStringOrEmpty(
    await questionUntilClose(rl, `${question}${suffix}`).finally(() => rl.close()),
  );
  if (!answer) return defaultYes;
  return answer.startsWith("y");
}

export async function promptText(question: string): Promise<string> {
  const rl = readline.createInterface({ input, output });
  return await questionUntilClose(rl, question).finally(() => rl.close());
}

/** Signals that an interactive prompt lost stdin before a complete answer arrived. */
export class PromptInputClosedError extends Error {
  constructor() { super("Prompt input closed before an answer was received."); this.name = "PromptInputClosedError"; }
}
```

**导航增强**（`src/wizard/clack-navigation-prompts.ts`）：为 Clack 添加 Vim/Arrow 键导航页脚。

### 3.7 错误处理

**OpenClawCommand.error**（`src/cli/program/openclaw-command.ts:24-52`）：

```typescript
export class OpenClawCommand extends Command {
  override createCommand(name?: string): Command { return new OpenClawCommand(name); }

  override error(message: string, errorOptions?: ErrorOptions): never {
    const restoreErrorCommand = setCommanderErrorCommand(this);
    try {
      return super.error(message, errorOptions);
    } catch (error) {
      if (error instanceof CommanderError && error.exitCode !== 0 &&
          (isJsonOutputModeActive(process.argv) || isCommandJsonOutputMode(this, process.argv))) {
        if (!isCommandJsonOutputMode(this, process.argv) &&
            !hasCommanderOptionToken(this, process.argv, new Set(["--json"]), "flag")) {
          applyResolvedCommandOutputMode(false);
          throw error;
        }
        applyResolvedCommandOutputMode(true);
        throw createCliParseError(message, {
          argv: process.argv,
          commandPath: getCommanderErrorCommandPath(this),
          commandNames: getCommanderErrorCommandNames(this),
        }, { humanOutputWritten: true });
      }
      throw error;
    } finally {
      restoreErrorCommand();
    }
  }
  // ...
}
```

**JSON 错误模式**：当 `--json` 激活时，错误以结构化 JSON 输出而非人类可读文本输出。

**Exit Code 保持**：`exitOverride` + `process.exitCode = err.exitCode` 确保退出码语义正确。

### 3.8 插件 CLI

**Plugin 命令命名空间**（`src/cli/program/register.subclis.ts:10-22`）：

```typescript
const completionEntries = buildCommandGroupEntries(getSubCliEntryDescriptors(), [
  [["completion"], async (program) => (await import("../completion-cli.js")).registerCompletionCli(program)],
]);

export async function registerSubCliByName(
  program: Command, name: string, argv: string[] = process.argv,
  context: SubCliRegistrationContext = {},
): Promise<boolean> {
  if (await registerSubCliByNameCore(program, name, argv, context)) return true;
  return registerCommandGroupByName(program, completionEntries, name);
}

export function registerSubCliCommands(program: Command, argv: string[] = process.argv) {
  registerSubCliCommandsCore(program, argv);
  const { primary } = resolveCliArgvInvocation(argv);
  registerCommandGroups(program, completionEntries, {
    eager: shouldEagerRegisterSubcommands(),
    primary,
    registerPrimaryOnly: Boolean(primary && shouldRegisterPrimarySubcommandOnly(argv)),
  });
}
```

**插件命令注册策略**（`src/cli/command-registration-policy.ts`）：
- `shouldRegisterPrimaryCommandOnly` - 只注册用户实际调用的主命令
- `shouldSkipPluginCommandRegistration` - 跳过插件命令注册（cold path）
- `isReservedNonPluginCommandRoot` - 保留命令（非插件）

### 3.9 配置集成

**Profile 系统**（`src/cli/profile.ts`）：

```typescript
export function applyCliProfileEnv({ profile }: { profile: string }): void {
  // 解析 --profile <name>，设置 OPENCLAW_STATE_DIR/OPENCLAW_CONFIG_PATH 到 ~/.openclaw-<name>
}
```

**配置优先级**：
1. `--profile <name>`（CLI 参数）
2. `--dev` 模式（隔离到 `~/.openclaw-dev`）
3. `--container <name>`（Podman/Docker 容器）
4. 环境变量 `OPENCLAW_PROFILE`
5. 默认 `~/.openclaw`

### 3.10 脚本友好

**JSON 输出模式**（`src/cli/json-output-mode.ts`）：

```typescript
export function isJsonOutputModeActive(argv: string[]): boolean { ... }
export function hasJsonOutputFlag(argv: string[]): boolean { ... }
export function withConsoleLogsRoutedToStderrForJson<T>(...): T { ... }
```

**机器输出模式**（`src/cli/machine-output-argv.ts`）：
- `--json` 全局 JSON 输出
- `--machine` 子集
- `--output-dir <dir>` 文件输出

**容器模式**（`src/cli/container-target.ts`）：
- `--container <name>` 在指定 Podman/Docker 容器中运行
- 适用于 CI/CD 和隔离测试

**退出码语义**：
- 0 - 成功
- 1 - 一般错误
- 2 - 配置错误
- 64 - EX_USAGE（参数错误，借鉴 BSD sysexits.h）

---

## 四、opencode（TypeScript + yargs 18 + Effect + @clack/prompts）

### 4.1 CLI 框架选择

**核心依赖**（`packages/opencode/package.json`）：
```json
{
  "yargs": "18.0.0",
  "@clack/prompts": "1.0.0-alpha.1",
  "@types/yargs": "17.0.33"
}
```

**框架特性**：
- **yargs 18**：使用 `parserConfiguration({ "populate--": true })` 支持 `--` 透传
- **Effect 集成**：通过 `effect-cmd.ts` 包装 yargs handler 为 Effect，提供 `CliError`
- **clack/prompts**：交互式 UI
- **FormatError**：统一的错误格式化

### 4.2 命令注册与路由

**主入口**（`packages/opencode/src/index.ts:32-200`）：

```typescript
import yargs from "yargs";
import { hideBin } from "yargs/helpers";
import { RunCommand } from "./cli/cmd/run";
import { GenerateCommand } from "./cli/cmd/generate";
import { ConsoleCommand } from "./cli/cmd/account";
import { ProvidersCommand } from "./cli/cmd/providers";
import { AgentCommand } from "./cli/cmd/agent";
import { UpgradeCommand } from "./cli/cmd/upgrade";
import { UninstallCommand } from "./cli/cmd/uninstall";
import { ModelsCommand } from "./cli/cmd/models";
// ... 共 24+ 个命令

const args = hideBin(process.argv);
const cli = yargs(args)
  .parserConfiguration({ "populate--": true })   // 自动将 -- 后的参数填充到 argv["--"]
  .scriptName("opencode")
  .wrap(100)                                     // 帮助文本换行宽度
  .help("help", "show help")
  .alias("help", "h")
  .version("version", "show version number", InstallationVersion)
  .alias("version", "v")
  .option("print-logs", { describe: "print logs to stderr", type: "boolean" })
  .option("log-level", {
    describe: "log level",
    type: "string",
    choices: ["DEBUG", "INFO", "WARN", "ERROR"],
  })
  .option("pure", { describe: "run without external plugins", type: "boolean" })
  .middleware(async (opts) => {
    if (opts.printLogs) process.env.OPENCODE_PRINT_LOGS = "1";
    if (opts.logLevel) process.env.OPENCODE_LOG_LEVEL = opts.logLevel;
    if (opts.pure) process.env.OPENCODE_PURE = "1";
    Heap.start();
    process.env.AGENT = "1";
    process.env.OPENCODE = "1";
    process.env.OPENCODE_PID = String(process.pid);
  })
  .usage("")
  .completion("completion", "generate shell completion script")    // ← 自动生成补全
  .command(AcpCommand).command(McpCommand).command(TuiThreadCommand).command(AttachCommand)
  .command(RunCommand).command(GenerateCommand).command(DebugCommand).command(ConsoleCommand)
  .command(ProvidersCommand).command(AgentCommand).command(UpgradeCommand).command(UninstallCommand)
  .command(ServeCommand).command(WebCommand).command(ModelsCommand).command(StatsCommand)
  .command(ExportCommand).command(ImportCommand).command(GithubCommand).command(PrCommand)
  .command(SessionCommand).command(PluginCommand).command(DbCommand)
  .fail((msg, err) => {
    if (msg?.startsWith("Unknown argument") || msg?.startsWith("Not enough non-option arguments") ||
        msg?.startsWith("Invalid values:")) {
      if (err) throw err;
      cli.showHelp(show);
    }
    if (err) throw err;
    process.exit(1);
  })
  .strict();
```

**Effect-Cmd**（`src/cli/effect-cmd.ts`）：

```typescript
export class CliError extends Schema.TaggedErrorClass<CliError>()("CliError", {
  message: Schema.String,
  exitCode: Schema.optional(Schema.Number),
}) {}

export const fail = (message: string, exitCode = 1) => Effect.fail(new CliError({ message, exitCode }));

interface EffectCmdOpts<Args, A> {
  command: string | readonly string[];
  aliases?: string | readonly string[];
  describe: string | false;
  builder?: (yargs: Argv) => Argv<Args>;
  instance?: boolean | ((args: Args) => boolean);  // 是否需要项目实例
  directory?: (args: Args) => string;
  handler: (args: WithDoubleDash<Args>) => Effect.Effect<A, CliError, AppServices | InstanceStore.Service>;
}
```

**Cmd Type**（`src/cli/cmd/cmd.ts`）：

```typescript
import type { CommandModule } from "yargs";

export type WithDoubleDash<T> = T & { "--"?: string[]; _?: Array<string | number> };

export function cmd<T, U>(input: CommandModule<T, WithDoubleDash<U>>) {
  return input;
}
```

### 4.3 参数解析

**Run Command Builder**（`src/cli/cmd/run.ts:127-260`）：

```typescript
{
  command: "run [message..]",
  describe: "run opencode with a message",
  instance: (args) => !args.attach,         // 动态决定是否需要 instance
  directory: (args) => (args.dir && !args.attach ? path.resolve(process.cwd(), args.dir) : process.cwd()),
  builder: (yargs: Argv) =>
    yargs
      .positional("message", {
        describe: "message to send",
        type: "string",
        array: true,                          // 多个 message
        default: [],
      })
      .option("command", { describe: "the command to run, use message for args", type: "string" })
      .option("continue", { alias: ["c"], describe: "continue the last session", type: "boolean" })
      .option("session", { alias: ["s"], describe: "session id to continue", type: "string" })
      .option("fork", { describe: "fork the session before continuing (requires --continue or --session)", type: "boolean" })
      .option("share", { type: "boolean", describe: "share the session" })
      .option("model", { type: "string", alias: ["m"], describe: "model to use in the format of provider/model" })
      .option("agent", { type: "string", describe: "agent to use" })
      .option("format", {
        type: "string",
        choices: ["default", "json"],         // ← 枚举校验
        default: "default",
        describe: "format: default (formatted) or json (raw JSON events)",
      })
      .option("file", { alias: ["f"], type: "string", array: true, describe: "file(s) to attach to message" })
      .option("title", { type: "string", describe: "title for the session" })
      .option("attach", { type: "string", describe: "attach to a running opencode server (e.g., http://localhost:4096)" })
      .option("password", { alias: ["p"], type: "string", describe: "basic auth password" })
      .option("username", { alias: ["u"], type: "string", describe: "basic auth username" })
      .option("dir", { type: "string", describe: "directory to run in, path on remote server if attaching" })
      .option("port", { type: "number", describe: "port for the local server (defaults to random port if no value provided)" })
      .option("variant", { type: "string", describe: "model variant (provider-specific reasoning effort)" })
      .option("thinking", { type: "boolean", describe: "show thinking blocks" })
      .option("mini", { type: "boolean", hidden: true, default: false })
      .option("replay", { type: "boolean", default: true, hidden: true, describe: "replay interactive session history" })
      .option("interactive", { alias: ["i"], type: "boolean", describe: "run in direct interactive split-footer mode", default: false })
      .option("auto", { type: "boolean", describe: "auto-approve permissions that are not explicitly denied (dangerous!)", default: false })
      .option("yolo", { type: "boolean", hidden: true, default: false })
      .option("dangerously-skip-permissions", { type: "boolean", hidden: true, default: false })
      // ... 共 30+ 选项
}
```

### 4.4 帮助系统

**Yargs 自动帮助**：
- `.wrap(100)` 控制换行宽度
- `.help("help", "show help").alias("help", "h")` 自定义 help 标志
- `.usage("")` 隐藏默认 Usage 行（自定义）

**隐藏选项**：`hidden: true` 在帮助中不显示（如 `--mini`、`--yolo`、`--dangerously-skip-permissions`）

**Show Help**（`src/index.ts:107-115`）：

```typescript
.fail((msg, err) => {
  if (msg?.startsWith("Unknown argument") ||
      msg?.startsWith("Not enough non-option arguments") ||
      msg?.startsWith("Invalid values:")) {
    if (err) throw err;
    cli.showHelp(show);
  }
  if (err) throw err;
  process.exit(1);
})
```

**自定义 Help 显示**（`show` 函数）：

```typescript
function show(out: string) {
  const text = out.trimStart()
  if (!text.startsWith("opencode ")) {
    process.stderr.write(UI.logo() + EOL + EOL)        // 在 help 前加 logo
    process.stderr.write(text + EOL)
    return
  }
  process.stderr.write(out)
}
```

### 4.5 自动补全

**内置 yargs completion**（`src/index.ts:80`）：

```typescript
.completion("completion", "generate shell completion script")
```

yargs 自动支持 bash/zsh/fish。

**调用方式**：`opencode completion bash > ~/.bashrc.d/opencode` 等。

### 4.6 交互式提示

**Clack Prompts 用法**（`src/cli/cmd/agent.ts:55-100`）：

```typescript
import * as prompts from "@clack/prompts";

// 在 agent.ts 中：
const scopeResult = await prompts.select({
  message: "Where should the agent run?",
  options: [
    { value: "local", label: "Local", hint: "Run on the current machine" },
    { value: "remote", label: "Remote", hint: "Run on a remote server" },
  ],
});
```

**多类型支持**：
- `prompts.text()` - 文本输入
- `prompts.confirm()` - 确认
- `prompts.select()` - 单选
- `prompts.multiselect()` - 多选
- `prompts.password()` - 密码
- `prompts.spinner()` - 进度

### 4.7 错误处理

**FormatError**（`src/cli/error.ts:33-200`）：

```typescript
export function FormatError(input: unknown): string | undefined {
  if (input instanceof Error && isRecord(input.cause) && "body" in input.cause) {
    const formatted = FormatError(input.cause.body);
    if (formatted) return formatted;
  }

  // CliError: domain failure surfaced from an effectCmd handler via fail("...")
  if (isTaggedError(input, "CliError")) {
    if (typeof input.exitCode === "number") process.exitCode = input.exitCode;
    return stringField(input, "message") ?? "";
  }

  // MCPFailed: { name: string }
  if (NamedError.hasName(input, "MCPFailed")) {
    const data = isRecord(input) && isRecord(input.data) ? stringField(input.data, "name") : undefined;
    return `MCP server "${data}" failed. Note, opencode does not support MCP authentication yet.`;
  }

  // AccountServiceError, AccountTransportError: TaggedErrorClass
  if (isTaggedError(input, "AccountServiceError") || isTaggedError(input, "AccountTransportError")) {
    return stringField(input, "message") ?? "";
  }

  // ProviderModelNotFoundError: { providerID: string, modelID: string, suggestions?: string[] }
  const providerModelNotFound = configData(input, "ProviderModelNotFoundError");
  if (providerModelNotFound) {
    const suggestions = Array.isArray(providerModelNotFound.suggestions)
      ? providerModelNotFound.suggestions.filter((x) => typeof x === "string") : [];
    return [
      `Model not found: ${stringField(providerModelNotFound, "providerID")}/${stringField(providerModelNotFound, "modelID")}`,
      ...(suggestions.length ? ["Did you mean: " + suggestions.join(", ")] : []),
      `Try: \`opencode models\` to list available models`,
      `Or check your config (opencode.json) provider/model names`,
    ].join("\n");
  }

  // ProviderInitError: { providerID: string }
  const providerInit = configData(input, "ProviderInitError");
  if (providerInit) {
    return `Failed to initialize provider "${stringField(providerInit, "providerID")}". Check credentials and configuration.`;
  }
  // ... 更多 TaggedError 类型处理
}
```

**错误链**：通过 `input.cause` 递归格式化嵌套错误。

**Exit 处理**（`src/index.ts:113-129`）：

```typescript
try {
  if (args.includes("-h") || args.includes("--help")) {
    await cli.parse(args, (err: Error | undefined, _argv: unknown, out: string) => {
      if (err) throw err;
      if (!out) return;
      show(out);
    });
  } else {
    await cli.parse();
  }
} catch (e) {
  const formatted = FormatError(e);
  if (formatted) UI.error(formatted);
  if (formatted === undefined) {
    UI.error("Unexpected error" + EOL);
    process.stderr.write(errorMessage(e) + EOL);
  }
  process.exitCode = 1;
} finally {
  // Explicitly exit to avoid any hanging subprocesses (docker MCP servers).
  process.exit();
}
```

### 4.8 插件 CLI

**PluginCommand**（`src/cli/cmd/plug.ts`）：

通过 yargs `.command(PluginCommand)` 注册，plugin 命令动态加载自 `OPENCODE_PURE=0` 模式下的插件目录。

**Plugin vs Pure 模式**：
- `.option("pure", { describe: "run without external plugins", type: "boolean" })`
- `OPENCODE_PURE=1` 环境变量禁用插件

### 4.9 配置集成

**配置发现链**：
- `./opencode.json`（项目级）
- `~/.config/opencode/config.json`（用户级）
- `$XDG_CONFIG_HOME/opencode/`（XDG）

**实例管理**：
- `instance: (args) => !args.attach` 动态决定是否需要加载实例
- `directory: (args) => path.resolve(process.cwd(), args.dir)` 解析目录

### 4.10 脚本友好

**JSON 模式**：
- `run --format json` - 流式 NDJSON 事件
- `run --format default` - 格式化输出

**服务端模式**：
- `serve` 子命令启动 HTTP 服务（端口 4096）
- `attach` 子命令连接到运行中的服务

**MCP 兼容**：
- 自动注入 `OPENCODE=1`、`AGENT=1`、`OPENCODE_PID=<pid>` 到子进程
- 显式 `process.exit()` 在 finally 块中，避免 docker MCP 子进程悬挂

---

## 五、pi（TypeScript + 完全自研 parseArgs）

### 5.1 CLI 框架选择

**核心范式**：**零依赖自研参数解析器**（不使用 yargs/commander）

**位置**：`packages/coding-agent/src/cli/args.ts`（447 行）

**特点**：
- 手写 for 循环扫描 argv
- 严格类型化 `Args` 接口（30+ 字段）
- 内嵌 `diagnostics` 错误收集（不是抛出异常）
- 手写 help 函数（`printHelp`）

### 5.2 命令注册与路由

**Args 接口**（节选自 `args.ts:13-70`）：

```typescript
export interface Args {
  provider?: string;
  model?: string;
  apiKey?: string;
  systemPrompt?: string;
  appendSystemPrompt?: string[];
  thinking?: ThinkingLevel;
  continue?: boolean;
  resume?: boolean;
  help?: boolean;
  version?: boolean;
  mode?: Mode;                                 // "text" | "json" | "rpc"
  name?: string;
  noSession?: boolean;
  session?: string;
  sessionId?: string;
  fork?: string;
  sessionDir?: string;
  models?: string[];
  tools?: string[];
  excludeTools?: string[];
  noTools?: boolean;
  noBuiltinTools?: boolean;
  extensions?: string[];
  noExtensions?: boolean;
  print?: boolean;
  export?: string;
  noSkills?: boolean;
  skills?: string[];
  promptTemplates?: string[];
  noPromptTemplates?: boolean;
  themes?: string[];
  useTheme?: string;
  noThemes?: boolean;
  noContextFiles?: boolean;
  listModels?: string | true;
  offline?: boolean;
  tuiMode?: TuiMode;
  messages: string[];                          // 位置参数
  fileArgs: string[];                          // @file 参数
  unknownFlags: Map<string, string | boolean>;  // 未知 flag 收集
  diagnostics: Array<{ type: "error" | "warning"; message: string }>;
}
```

**parseArgs 实现**（`args.ts:71-260`）：

```typescript
export function parseArgs(args: string[]): Args {
  const result: Args = {
    messages: [], fileArgs: [], unknownFlags: new Map(), diagnostics: [],
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];

    if (arg === "--") {
      // 透传：-- 后所有参数视为 messages 或 @file
      for (const positionalArg of args.slice(i + 1)) {
        if (positionalArg.startsWith("@")) result.fileArgs.push(positionalArg.slice(1));
        else result.messages.push(positionalArg);
      }
      break;
    } else if (arg === "--help" || arg === "-h") {
      result.help = true;
    } else if (arg === "--version" || arg === "-v") {
      result.version = true;
    } else if (arg === "--mode" && i + 1 < args.length) {
      const mode = args[++i];
      if (mode === "text" || mode === "json" || mode === "rpc") {
        result.mode = mode;
      }
    } else if (arg === "--continue" || arg === "-c") {
      result.continue = true;
    } else if (arg === "--resume" || arg === "-r") {
      result.resume = true;
    } else if (arg === "--provider" && i + 1 < args.length) {
      result.provider = args[++i];
    } else if (arg === "--model" && i + 1 < args.length) {
      result.model = args[++i];
    } else if (arg === "--api-key" && i + 1 < args.length) {
      result.apiKey = args[++i];
    } else if (arg === "--system-prompt" && i + 1 < args.length) {
      result.systemPrompt = args[++i];
    } else if (arg === "--append-system-prompt" && i + 1 < args.length) {
      result.appendSystemPrompt = result.appendSystemPrompt ?? [];
      result.appendSystemPrompt.push(args[++i]);
    } else if (arg === "--name" || arg === "-n") {
      if (i + 1 < args.length) {
        result.name = args[++i];
      } else {
        result.diagnostics.push({ type: "error", message: "--name requires a value" });
      }
    } else if (arg === "--no-session") {
      result.noSession = true;
    } else if (arg === "--session" && i + 1 < args.length) {
      result.session = args[++i];
    } else if (arg === "--session-id" && i + 1 < args.length) {
      result.sessionId = args[++i];
    } else if (arg === "--fork" && i + 1 < args.length) {
      result.fork = args[++i];
    } else if (arg === "--session-dir" && i + 1 < args.length) {
      result.sessionDir = args[++i];
    } else if (arg === "--models" && i + 1 < args.length) {
      result.models = args[++i].split(",").map((s) => s.trim());
    } else if (arg === "--no-tools" || arg === "-nt") {
      result.noTools = true;
    } else if (arg === "--no-builtin-tools" || arg === "-nbt") {
      result.noBuiltinTools = true;
    } else if ((arg === "--tools" || arg === "-t") && i + 1 < args.length) {
      result.tools = args[++i].split(",").map((s) => s.trim()).filter((name) => name.length > 0);
    } else if ((arg === "--exclude-tools" || arg === "-xt") && i + 1 < args.length) {
      result.excludeTools = args[++i].split(",").map((s) => s.trim()).filter((name) => name.length > 0);
    } else if (arg === "--thinking" && i + 1 < args.length) {
      const level = args[++i];
      if (isValidThinkingLevel(level)) {
        result.thinking = level;
      } else {
        result.diagnostics.push({
          type: "warning",
          message: `Invalid thinking level "${level}". Valid values: ${VALID_THINKING_LEVELS.join(", ")}`,
        });
      }
    } else if (arg === "--print" || arg === "-p") {
      result.print = true;
      const next = args[i + 1];
      if (next !== undefined && !next.startsWith("@") && (!next.startsWith("-") || next.startsWith("---"))) {
        result.messages.push(next);
        i++;
      }
    }
    // ... 共 60+ 个 else if 分支
    else if (arg.startsWith("@")) {
      result.fileArgs.push(arg.slice(1)); // 移除 @ 前缀
    } else if (arg.startsWith("--")) {
      // 未知长选项：收集到 unknownFlags
      const eqIndex = arg.indexOf("=");
      if (eqIndex !== -1) {
        result.unknownFlags.set(arg.slice(2, eqIndex), arg.slice(eqIndex + 1));
      } else {
        const flagName = arg.slice(2);
        const next = args[i + 1];
        if (next !== undefined && !next.startsWith("-") && !next.startsWith("@")) {
          result.unknownFlags.set(flagName, next);
          i++;
        } else {
          result.unknownFlags.set(flagName, true);
        }
      }
    } else if (arg.startsWith("-") && !arg.startsWith("--")) {
      result.diagnostics.push({ type: "error", message: `Unknown option: ${arg}` });
    } else if (!arg.startsWith("-")) {
      result.messages.push(arg);
    }
  }

  return result;
}
```

**关键设计**：
- **零异常**：所有错误累积到 `diagnostics` 数组
- **未知 flag 收集**：通过 `unknownFlags: Map` 传递给扩展
- **@file 参数**：`@prompt.md` 形式（类似 Claude CLI）
- **-- 终止符**：剩余参数全部视为 messages

**子命令**（隐式通过 messages[0]）：
- `pi install <source> [-l]`
- `pi remove <source> [-l]`
- `pi uninstall <source> [-l]`（alias）
- `pi update [source|self|pi]`
- `pi list`
- `pi config [-l]`
- `pi auth <command>`

### 5.3 参数解析

**所有支持的 CLI 选项**（节选自 `printHelp`，`args.ts:279-348`）：

```typescript
// Provider/Model
--provider <name>              Provider name (default: google)
--model <pattern>              Model pattern or ID (supports "provider/id" and optional ":<thinking>")
--api-key <key>                API key (defaults to env vars)

// System Prompt
--system-prompt <text>         System prompt (default: coding assistant prompt)
--append-system-prompt <text>  Append text or file contents to the system prompt (can be used multiple times)

// Mode
--mode <mode>                  Output mode: text (default), json, or rpc

// Print Mode
--print, -p                    Non-interactive mode: process prompt and exit

// Session
--continue, -c                 Continue previous session
--resume, -r                   Select a session to resume
--session <path|id>            Use specific session file or partial UUID
--session-id <id>              Use exact project session ID, creating it if missing
--fork <path|id>               Fork specific session file or partial UUID into a new session
--session-dir <dir>            Directory for session storage and lookup
--no-session                   Don't save session (ephemeral)
--name, -n <name>              Set session display name

// Models
--models <patterns>            Comma-separated model patterns for Ctrl+P cycling
                               Supports globs (anthropic/*, *sonnet*) and fuzzy matching

// Tools
--no-tools, -nt                Disable all tools by default (built-in and extension)
--no-builtin-tools, -nbt       Disable built-in tools by default but keep extension/custom tools enabled
--tools, -t <tools>            Comma-separated allowlist of tool names to enable
                               Applies to built-in, extension, and custom tools
--exclude-tools, -xt <tools>   Comma-separated denylist of tool names to disable

// Thinking
--thinking <level>             Set thinking level: off, minimal, low, medium, high, xhigh, max

// Extensions/Skills/Templates/Themes
--extension, -e <path>         Load an extension file (can be used multiple times)
--no-extensions, -ne           Disable extension discovery (explicit -e paths still work)
--skill <path>                 Load a skill file or directory (can be used multiple times)
--no-skills, -ns               Disable skills discovery and loading
--prompt-template <path>       Load a prompt template file or directory (can be used multiple times)
--no-prompt-templates, -np     Disable prompt template discovery and loading
--theme <path>                 Load a theme file or directory (can be used multiple times)
--use-theme <name[/name]>      Set the initial interactive theme for this run
--no-themes                    Disable theme discovery and loading
--no-context-files, -nc        Disable AGENTS.md and CLAUDE.md discovery and loading

// Export
--export <file>                Export session file to HTML and exit

// List Models
--list-models [search]         List available models (with optional fuzzy search)

// TUI Mode
--tui-mode <mode>              TUI mode: regular (default) or fullscreen
--verbose                      Force verbose startup (overrides quietStartup setting)

// Project Trust
--approve, -a                  Trust project-local files for this run
--no-approve, -na              Ignore project-local files for this run

// Network
--offline                      Disable startup network operations (same as PI_OFFLINE=1)

--                             End option parsing; treat remaining arguments as messages/files
--help, -h                     Show this help
--version, -v                  Show version number
```

### 5.4 帮助系统

**printHelp**（`args.ts:262-440`，约 178 行）：

```typescript
export function printHelp(extensionFlags?: ExtensionFlag[]): void {
  const extensionFlagsText = extensionFlags && extensionFlags.length > 0
    ? `\n${chalk.bold("Extension CLI Flags:")}\n${extensionFlags
        .map((flag) => {
          const value = flag.type === "string" ? " <value>" : "";
          const description = flag.description ?? `Registered by ${flag.extensionPath}`;
          return `  --${flag.name}${value}`.padEnd(30) + description;
        })
        .join("\n")}\n`
    : "";
  console.log(`${chalk.bold(APP_NAME)} - AI coding assistant with read, bash, edit, write tools

${chalk.bold("Usage:")}
  ${APP_NAME} [options] [--] [@files...] [messages...]

${chalk.bold("Commands:")}
  ${APP_NAME} install <source> [-l]     Install extension source and add to settings
  ${APP_NAME} remove <source> [-l]      Remove extension source from settings
  ${APP_NAME} uninstall <source> [-l]   Alias for remove
  ${APP_NAME} update [source|self|pi]   Update pi, extensions, or model catalogs
  ${APP_NAME} list                      List installed extensions from settings
  ${APP_NAME} config [-l]               Open TUI to enable/disable package resources (Tab switches scope)
  ${APP_NAME} auth <command>            Print credentials or check provider readiness
  ${APP_NAME} <command> --help          Show help for install/remove/uninstall/update/list/config/auth

${chalk.bold("Options:")}
  --provider <name>              Provider name (default: google)
  --model <pattern>              Model pattern or ID (supports "provider/id" and optional ":<thinking>")
  ... (60+ options)

${chalk.bold("Examples:")}
  # Print a provider API key for an external client
  ${APP_NAME} auth print-api-key --provider openai

  # Interactive mode
  ${APP_NAME}

  # Non-interactive mode
  ${APP_NAME} -p "List all .ts files in src/"

  # Prompt beginning with a dash
  ${APP_NAME} -p -- "- Summarize these points"

  # Continue previous session
  ${APP_NAME} --continue "What did we discuss?"

  ...

${chalk.bold("Environment Variables:")}
  ANTHROPIC_AUTH_TOKEN             - Anthropic bearer auth token
  ANTHROPIC_API_KEY                - Anthropic Claude API key
  ... (40+ env vars)

${chalk.bold("Built-in Tool Names:")}
  read       - Read file contents
  bash       - Execute bash commands
  ... (8 tools)
`);
}
```

**特点**：
- **chalk 着色**：使用 chalk 高亮标题
- **Examples 块**：真实可运行的示例
- **Environment Variables**：完整的 env 变量清单
- **Extension CLI Flags**：动态注入扩展 flag（来自 extensions）

### 5.5 自动补全

**pi 无内置 Shell 补全**：依赖 Fish/Zsh/Bash 自动补全机制 + extensions。

**Experimental Command System**（`packages/coding-agent/src/cli/experimental/`）：

```typescript
// command.ts
export interface CommandOption<TValue> {
  readonly name: `--${string}`;
  parse(value: string): CommandOptionParseResult<TValue>;
}

export interface ParsedCommandInput {
  readonly remainingArgs: readonly string[];
  value<TValue>(option: CommandOption<TValue>): TValue | undefined;
  values<TValue>(option: CommandOption<TValue>): readonly TValue[];
}
```

### 5.6 交互式提示

**Project Trust**（`cli/project-trust.ts`）：检查项目本地文件信任状态，决定是否加载 `.pi/`、`AGENTS.md`、`CLAUDE.md` 等。

**Session Picker**（`cli/session-picker.ts`）：列出已有 session，使用 `@inquirer/select` 选择。

**First-Time Setup**（`cli/startup-ui.ts`）：显示 onboarding 提示。

### 5.7 错误处理

**Diagnostics 模式**：所有错误累积到 `diagnostics: Array<{ type, message }>`，不抛出异常。

**Unknown Flags**：`unknownFlags: Map<string, string | boolean>` 收集未知 flag，传递给 extensions。

**Main Loop 处理**（`main.ts`）：

```typescript
const args = parseArgs(process.argv.slice(2));
if (args.diagnostics.length > 0) {
  for (const diag of args.diagnostics) {
    if (diag.type === "error") console.error(chalk.red(`Error: ${diag.message}`));
    else console.warn(chalk.yellow(`Warning: ${diag.message}`));
  }
}
```

### 5.8 插件 CLI

**Extension Flag 注册**：

```typescript
// 在 printHelp 中：
const extensionFlagsText = extensionFlags && extensionFlags.length > 0
  ? `\n${chalk.bold("Extension CLI Flags:")}\n${extensionFlags.map(...)}`
  : "";
```

**Extension 系统**（`core/extensions/`）：扩展可以注册自己的 `--flag` 参数（通过 `ExtensionFlag` 接口），与主解析器集成。

### 5.9 配置集成

**优先级链**：
1. CLI 参数（最高）
2. 环境变量（`ANTHROPIC_API_KEY`、`OPENAI_API_KEY` 等）
3. `~/.pi/agent/settings.json`（用户级）
4. `.pi/settings.json`（项目级）
5. Built-in defaults

**AGENTS.md / CLAUDE.md 注入**：
- `--no-context-files` / `-nc` 禁用
- 否则自动发现并注入到系统提示词

### 5.10 脚本友好

**Mode 选项**：
- `text` - 默认纯文本
- `json` - JSON 输出
- `rpc` - JSON-RPC 协议模式（用于 IDE 集成）

**--print / -p**：
- 非交互模式
- 与 `--export <file>` 配合生成 HTML 报告
- 接受 `---prompt` 形式（开头是 `-` 的 prompt）

**@file 参数**：
- `pi @prompt.md @image.png "What color is the sky?"`
- 自动加载文件内容到初始消息

**env 变量清单**（约 40 个，覆盖 38 个 Provider + 配置目录 + session 目录等）

---

## 六、cc-switch（Rust + Tauri 2，无传统 CLI）

### 6.1 CLI 框架选择

**核心依赖**（`src-tauri/Cargo.toml`）：

```toml
tauri = { version = "2.8.2", features = ["tray-icon", "protocol-asset", "image-png"] }
tauri-plugin-log = "2"
tauri-plugin-opener = "2"
tauri-plugin-process = "2"
tauri-plugin-updater = "2"
tauri-plugin-dialog = "2"
tauri-plugin-store = "2"
tauri-plugin-deep-link = "2"
tauri-plugin-window-state = "2"
```

**框架特性**：
- **不是传统 CLI**：cc-switch 是 **Tauri 2 桌面应用**，主交互是 GUI（React 前端）
- **IPC 命令**：通过 `#[tauri::command]` 属性宏暴露函数给前端
- **无 clap/structopt**：因为没有命令行参数解析

### 6.2 命令注册与路由

**主入口**（`src-tauri/src/main.rs`）：

```rust
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKit/DMA-BUF 环境变量处理
    #[cfg(target_os = "linux")] {
      // ... 解决 Linux WebKit 黑屏问题 ...
    }
    cc_switch_lib::run();
}
```

**Lib Run**（`src-tauri/src/lib.rs`）：

```rust
#[tauri::command]
pub fn run() {
    // 启动 Tauri 应用
}
```

**30+ IPC 命令文件**（`src-tauri/src/commands/`）：

| 文件 | 行数 | 职责 |
|------|------|------|
| `provider.rs` | 1267 | 供应商 CRUD |
| `auth.rs` | 459 | 认证（OAuth、API Key） |
| `config.rs` | 439 | 应用配置 |
| `proxy.rs` | 468 | 代理服务 |
| `misc.rs` | 7051 | 杂项（MCP、Skills、Settings 等） |
| `mcp.rs` | - | MCP 服务器管理 |
| `skill.rs` | 340 | Skill 管理 |
| `failover.rs` | 299 | 故障转移 |
| `stream_check.rs` | 249 | 流检查 |
| `import_export.rs` | 248 | 导入导出 |
| `settings.rs` | 720 | 设置 |
| ... | | |

**示例 IPC 命令**（`src-tauri/src/commands/provider.rs:31-37`）：

```rust
/// 获取所有供应商
#[tauri::command]
pub fn get_providers(
    state: State<'_, AppState>,
    app: String,
) -> Result<IndexMap<String, Provider>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::list(state.inner(), app_type).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_current_provider(state: State<'_, AppState>, app: String) -> Result<String, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::current(state.inner(), app_type).map_err(|e| e.to_string())
}
```

**特点**：
- **State 注入**：`State<'_, AppState>` 由 Tauri 容器注入
- **async 支持**：`pub async fn add_provider(...)`
- **错误传播**：`Result<_, String>`（或自定义 AppError）

### 6.3 参数解析

**Tauri IPC 参数**：通过函数签名自动反序列化（JSON）：
- `app: String` - 应用类型（claude/codex/openclaw/...）
- `provider: Provider` - 嵌套结构体
- `addToLive: Option<bool>` - 可选 snake_case 转 camelCase

### 6.4 帮助系统

**无 CLI 帮助**：所有交互通过 GUI（React 前端）。

**Deep Link**（`src-tauri/src/deeplink.rs`）：
- `cc-switch://` URL scheme
- 通过 `tauri-plugin-deep-link` 处理
- 可被外部应用调用

### 6.5 自动补全

**无**：纯桌面应用，不暴露给 Shell。

### 6.6 交互式提示

**Tray 菜单**（`src-tauri/src/tray.rs`）：系统托盘菜单。

**对话框**（通过 `tauri-plugin-dialog`）：文件选择、消息确认、输入对话框。

### 6.7 错误处理

**统一错误类型**（`src-tauri/src/error.rs`）：

```rust
pub enum AppError {
  // 各种错误变体
}
```

**错误传播**：通过 `Result<T, String>`（或 `AppError`）从 IPC 命令返回前端。

### 6.8 插件 CLI

**无 CLI 插件系统**：但支持插件管理（`plugin.rs`），通过 GUI 安装/卸载插件。

### 6.9 配置集成

**8 款应用适配**：claude/codex/openclaw/opencode/pi/copilot/gemini/grok 等。

**配置存储**：通过 `tauri-plugin-store` 持久化到本地。

### 6.10 脚本友好

**单实例 + Deep Link**：外部应用可通过 `cc-switch://` URL 触发特定操作。

**Updater**（`tauri-plugin-updater`）：自动更新。

**全局快捷键**（通过 `tauri-plugin-global-shortcut` 间接）：切换窗口。

---

## 七、横向对比表

### 7.1 CLI 框架选择

| 项目 | 语言 | 框架 | 框架版本 | 关键特性 | 入口文件 |
|------|------|------|---------|---------|---------|
| **atomcode** | Rust | clap (derive) | 4.x | `clap::Parser`/`Subcommand`/`ValueEnum`，clap_complete 5 Shell | `crates/atomcode-cli/src/main.rs` (4987行) |
| **claudecode** | TypeScript | @commander-js/extra-typings | commander 15 | 强类型、feature gating、fast-path | `src/main.tsx` (4683行) |
| **openclaw** | TypeScript | Commander (自定义子类) | 15.0.0 | 24+ 子命令、lazy-import、preaction | `src/cli/run-main.ts` (1697行) |
| **opencode** | TypeScript | yargs | 18.0.0 | Effect 集成、`--` 透传、middleware | `packages/opencode/src/index.ts` (200行) |
| **pi** | TypeScript | **自研 parseArgs** | - | 零依赖、diagnostics 累积、unknownFlags Map | `packages/coding-agent/src/cli/args.ts` (447行) |
| **cc-switch** | Rust | Tauri IPC | 2.8.2 | 桌面应用、30+ `#[tauri::command]` 函数 | `src-tauri/src/main.rs` → `lib.rs::run()` |

### 7.2 命令注册与路由

| 项目 | 命令注册 | 嵌套深度 | 动态注册 | 别名 | 隐藏命令 |
|------|---------|---------|---------|------|---------|
| atomcode | `enum Commands` derive | 3级（`mcp add-github-oauth`） | 否 | `Codingplan` (hide) | `Acp`/`__askpass` |
| claudecode | `program.command().command()...` | 3级（`mcp add-from-claude-desktop`） | 懒加载（动态 import） | `plugin → plugins` | `completion` |
| openclaw | `commandGroupDescriptors` 注册表 | 4级（`models auth login`） | **完全 lazy-import** | `crestodian→setup` | - |
| opencode | `yargs.command(XxxCommand)` | 2级（`db list`） | 懒加载（`await import`） | `continue→c` | `mini`/`yolo` |
| pi | 隐式（messages[0]） | 3级（`auth print-api-key`） | extension 注册 flag | `uninstall→remove` | - |
| cc-switch | IPC 函数（无 CLI） | N/A | N/A | N/A | N/A |

### 7.3 参数解析

| 项目 | 互斥参数 | 依赖参数 | 枚举校验 | 路径补全提示 | 累加器 |
|------|---------|---------|---------|------------|-------|
| atomcode | `conflicts_with_all` | `requires` | `value_enum` (自动) | `ValueHint::FilePath/DirPath` | - |
| claudecode | `.conflicts()` | - | `.choices([...])` | - | `(val, prev) => [...prev, val]` |
| openclaw | 自定义 fast-path | 自定义 fast-path | `parseCliLogLevelOption` | - | `parseRepeatedFlagValues` |
| opencode | - | - | `choices: [...]` | - | `array: true` |
| pi | - | - | `isValidThinkingLevel` | - | `result.appendSystemPrompt ?? []; .push()` |
| cc-switch | N/A | N/A | N/A | N/A | N/A |

### 7.4 帮助系统

| 项目 | 自动生成 | 多语言 | 自定义样式 | 错误时显示 |
|------|---------|--------|-----------|----------|
| atomcode | 是 (clap) | **是** (i18n `Msg::CliAbout`) | `mut_arg`/`mut_subcommand` | `clap::Error::exit()` |
| claudecode | 是 (Commander) | - | `createSortedHelpConfig()` | 默认 |
| openclaw | 是 (Commander) | - | `formatProgramHelpOutput` | `--help`/`-h` |
| opencode | 是 (yargs) | - | `show()` 函数（加 logo） | `cli.showHelp(show)` |
| pi | 自写 printHelp | - | chalk 着色 | `printHelp()` |
| cc-switch | N/A | N/A | N/A | N/A |

### 7.5 自动补全

| 项目 | Shell 支持 | 生成方式 | 缓存 | Profile 安装 | 动态补全 |
|------|----------|---------|------|------------|---------|
| atomcode | Bash/Zsh/Fish/PowerShell/Elvish | `clap_complete::generate` | - | - | 枚举自动 |
| claudecode | bash/zsh/fish | `completionHandler` | - | - | - |
| openclaw | bash/zsh/fish/powershell | 自实现 (598行) | `~/.openclaw/completions/` | 自动写入 `.bashrc` 等 | `OPENCLAW_COMPLETION_SKIP_PLUGIN_COMMANDS` |
| opencode | bash/zsh/fish | `yargs.completion()` | - | - | yargs 自动 |
| pi | - | - | - | - | - |
| cc-switch | N/A | N/A | N/A | N/A | N/A |

### 7.6 交互式提示

| 项目 | 库 | 文本输入 | 密码 | 选择 | 多选 | 自动补全 |
|------|---|---------|------|------|------|---------|
| atomcode | `__askpass` + Unix Socket | - | **独立子进程** | - | - | - |
| claudecode | Ink (React) | - | - | - | - | - |
| openclaw | `@clack/prompts` 1.7.0 | `text()` | `password()` | `select()` | `multiselect()` | `autocomplete()` |
| opencode | `@clack/prompts` 1.0.0-alpha | `prompts.text()` | `prompts.password()` | `prompts.select()` | `prompts.multiselect()` | - |
| pi | `@inquirer/*` | 是 | - | 是 | - | - |
| cc-switch | `tauri-plugin-dialog` | 是 | - | - | - | - |

### 7.7 错误处理

| 项目 | 错误累积 | JSON 错误 | Tagged Error | 退出码 | 嵌套错误链 |
|------|---------|----------|-------------|-------|----------|
| atomcode | - | - | `thiserror` | `error.exit()` | - |
| claudecode | - | `FormatError` | - | `process.exit(1)` | - |
| openclaw | `diagnostics` | `isJsonOutputModeActive` | - | `process.exitCode` | - |
| opencode | - | - | `Schema.TaggedErrorClass` (Effect) | `process.exitCode = 1` | `input.cause` 递归 |
| pi | **`diagnostics: Array<{type, message}>`** | - | - | - | - |
| cc-switch | `Result<_, AppError>` | - | - | - | - |

### 7.8 插件 CLI

| 项目 | 插件命令注册 | 命令命名空间 | 动态加载 |
|------|------------|------------|---------|
| atomcode | `<plugin>@<marketplace>` | `plugin install/remove/trust` | git pull |
| claudecode | `/plugin-name` (slash) | N/A | `--plugin-dir` |
| openclaw | `commandGroupDescriptors` + lazy | `claws/*` | 完全 lazy |
| opencode | `PluginCommand` | - | OPENCODE_PURE=1 |
| pi | `ExtensionFlag` 注入 CLI | `pi install <source>` | extensions 目录 |
| cc-switch | `plugin.rs` (GUI) | - | - |

### 7.9 配置集成

| 项目 | CLI 参数 | 环境变量 | 配置文件 | Profile 隔离 |
|------|---------|---------|---------|-----------|
| atomcode | `--config` `--seed-config` | `ATOMCODE_*` | `~/.atomcode/config.toml` | - |
| claudecode | `--settings` | 多 | `~/.claude/settings.json` | `--bare` |
| openclaw | `--profile` `--dev` | `OPENCLAW_*` | `~/.openclaw/` | `~/.openclaw-<name>/` |
| opencode | - | `OPENCODE_*` | `~/.config/opencode/` | `--pure` |
| pi | - | 40+ provider env | `~/.pi/agent/settings.json` | - |
| cc-switch | N/A | - | Tauri Store | - |

### 7.10 脚本友好

| 项目 | 非交互模式 | JSON 输出 | 容器化 | 退出码 |
|------|----------|----------|-------|-------|
| atomcode | `-p <prompt>` | `--output-format jsonl` | - | 0/1 |
| claudecode | `-p` | `--output-format json/stream-json` | - | - |
| openclaw | `--json` `--machine` | JSON | `--container <name>` | 0/1/2/64 |
| opencode | `--print` `--format json` | NDJSON | - | `process.exitCode` |
| pi | `-p` `--mode json/rpc` | JSON | - | - |
| cc-switch | N/A | N/A | N/A | N/A |

---

## 八、laew 差距分析（L281-L320）

### 8.1 laew 现状（基于 `src/main.rs`）

```rust
// LsmAgentEmergentWork/src/main.rs (节选)
// 使用 clap::Parser + Subcommand derive
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    // ... 全局参数 ...
}

#[derive(Subcommand)]
enum Commands {
    /// -p 单轮任务
    Print { prompt: Option<String> },
    /// -f 文件提示词
    File { path: PathBuf },
    /// TUI 交互模式（默认）
    Tui,
    /// Provider 管理
    Provider { #[command(subcommand)] action: ProviderAction },
}

#[derive(Subcommand)]
enum ProviderAction {
    List, Add, Use { id: String }, Del,
}
```

### 8.2 laew 与 6 工程对比差距

| 维度 | laew 现状 | 最佳实践（参考） | 差距级别 |
|------|----------|----------------|---------|
| **CLI 框架** | clap 4 derive | clap 4 + clap_complete | ✅ 已对齐 |
| **命令嵌套深度** | 2 级（`provider add`） | 3-4 级（atomcode mcp login/openclaw models auth login） | L281 |
| **多语言帮助** | 无 | atomcode `Msg::CliAbout` i18n | L282 |
| **命令隐藏** | 无 | atomcode `#[command(hide = true)]` | L283 |
| **ArgGroup 互斥** | 简单 `#[arg(conflicts_with)]` | atomcode `ArgGroup::new("headless_input")` | L284 |
| **路径补全提示** | 无 | atomcode `value_hint = ValueHint::FilePath` | L285 |
| **Shell 补全** | 无 | atomcode `clap_complete` 5 Shell | L286 |
| **补全 Profile 安装** | 无 | openclaw `installCompletion` 自动写 `.bashrc` | L287 |
| **补全缓存** | 无 | openclaw `~/.openclaw/completions/openclaw.{bash,zsh,fish,ps1}` | L288 |
| **fish 路径辅助函数** | 无 | openclaw `function __openclaw_command_path_matches` | L289 |
| **跳过插件命令** | 无 | openclaw `OPENCLAW_COMPLETION_SKIP_PLUGIN_COMMANDS` | L290 |
| **Windows 编码处理** | 无 | openclaw `utf8bom/utf16le/utf16be` 自动检测 | L291 |
| **错误累积模式** | 直接 panic | pi `diagnostics: Array<{type, message}>` | L292 |
| **未知 flag 收集** | 直接报错 | pi `unknownFlags: Map<string, string|boolean>` | L293 |
| **@file 参数** | 无 | pi `@prompt.md @image.png` | L294 |
| **Mode 三档** | 无 | pi `--mode text/json/rpc` | L295 |
| **结构化输出** | 无 | opencode `--format json` (NDJSON) / claudecode `--output-format stream-json` | L296 |
| **Effect 集成** | 无 | opencode `Effect` + `Schema.TaggedErrorClass` | L297 |
| **clack/prompts** | 无（自有 TUI 表单） | openclaw `@clack/prompts` 1.7.0 | L298 |
| **密码隐藏输入** | 内部 API Key mask | opencode `password()` / openclaw `password()` | L299 |
| **选择列表** | `/provider list` 子屏 | opencode `prompts.select()` | L300 |
| **Tagged Error** | `thiserror` derive | opencode `Schema.TaggedErrorClass` (Effect) | L301 |
| **错误链递归** | 无 | opencode `input.cause` 递归格式化 | L302 |
| **lazy-import 子命令** | 无 | openclaw 完全 lazy 24+ 子命令 | L303 |
| **命令组 Descriptor** | 无 | openclaw `commandGroupDescriptors` 注册表 | L304 |
| **preaction 钩子** | 无 | openclaw `registerPreActionHooks` | L305 |
| **argv fast-path** | 无 | atomcode `try_print_shell_completion` / claudecode `--version` fast-path | L306 |
| **env 优先级** | 部分 | pi 40+ env vars + override | L307 |
| **Profile 隔离** | 无 | openclaw `--profile <name>` → `~/.openclaw-<name>/` | L308 |
| **容器化模式** | 无 | openclaw `--container <name>` | L309 |
| **机器输出模式** | 无 | openclaw `--machine` / `--json` | L310 |
| **退出码语义** | 0/1 | openclaw 0/1/2/64 (BSD sysexits.h) | L311 |
| **--replay/--no-replay** | 无 | opencode `replay: { default: true, hidden: true }` | L312 |
| **TUI 模式切换** | 无 | pi `--tui-mode regular|fullscreen` | L313 |
| **Project Trust** | `--approve` 默认行为 | pi `--approve/-a` / `--no-approve/-na` | L314 |
| **PluginDir 累加** | 无 | claudecode `(val, prev) => [...prev, val]` | L315 |
| **枚举自动补全** | 手动 | clap `value_enum` / yargs `choices` | ✅ 已对齐 |
| **Help 排序** | 默认 | claudecode `createSortedHelpConfig()` | L316 |
| **Examples 块** | 无 | openclaw `EXAMPLES[13]` / pi `printHelp()` 60+ 行 | L317 |
| **Env Var 文档** | 部分 | pi `printHelp()` 含 40+ env vars | L318 |
| **Extension Flag 注入** | 无 | pi `extensionFlags` 动态注入到 printHelp | L319 |
| **Plugin 命令命名空间** | 无 | atomcode `<plugin>@<marketplace>` / pi `pi install <source>` | L320 |

### 8.3 laew 升级路线图

**P0 紧急**（影响日常使用）：

1. **集成 clap_complete**（参照 atomcode）：
```toml
# Cargo.toml
[dependencies]
clap = { version = "4", features = ["derive"] }
clap_complete = "4"
clap_complete_nushell = "4"  # Nushell 支持（可选）
```

2. **添加 Completion 子命令**（atomcode `main.rs:927-957`）：
```rust
#[derive(Subcommand)]
enum Commands {
    // ... 现有命令 ...
    /// Generate a shell completion script on stdout.
    Completion(CompletionCommand),
}

#[derive(clap::Args)]
struct CompletionCommand {
    #[arg(value_enum, default_value_t = Shell::Bash)]
    shell: Shell,
}
```

3. **API Key 密码隐藏**（使用 `rpassword` 或 `dialoguer`）：
```toml
[dependencies]
rpassword = "7"
dialoguer = { version = "0.11", features = ["password"] }
```

4. **结构化输出 `--output-format json`**：
```rust
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    #[default]
    Text,
    Jsonl,
}

#[arg(long, value_enum, default_value_t = OutputFormat::Text, requires = "headless_input")]
output_format: OutputFormat,
```

**P1 重要**（增强用户体验）：

5. **Profile 隔离**（参照 openclaw）：
```rust
#[arg(long)]
profile: Option<String>,

#[arg(long)]
dev: bool,  // 隔离到 ~/.laew-dev
```

6. **错误累积 diagnostics**（参照 pi）：
```rust
struct Cli {
    diagnostics: Vec<Diagnostic>,
    unknown_flags: HashMap<String, String>,
}

struct Diagnostic { level: Level, message: String }
enum Level { Error, Warning }
```

7. **路径补全提示**：
```rust
#[arg(long, value_hint = clap::ValueHint::FilePath)]
config: Option<PathBuf>,

#[arg(long, value_hint = clap::ValueHint::DirPath)]
work_dir: Option<PathBuf>,
```

8. **隐藏命令**：
```rust
#[derive(Subcommand)]
enum Commands {
    /// 隐藏的内部命令
    #[command(hide = true)]
    Internal { /* ... */ },
}
```

**P2 进阶**（生产级特性）：

9. **clap_complete Profile 安装**（参照 openclaw）：
- 自动写入 `~/.bashrc` / `~/.zshrc`
- 缓存到 `~/.laew/completions/`
- 检测平台编码（PowerShell UTF-8 BOM/UTF-16 LE/BE）

10. **Effect 风格错误**（参照 opencode）：
```rust
use effect_rs::Schema;

#[derive(Schema, Debug)]
#[effect(tag = "CliError")]
struct CliError {
    message: String,
    exit_code: Option<u8>,
}
```

11. **Command Group 描述符注册表**（参照 openclaw）：
```rust
struct CommandGroupDescriptor {
    names: &'static [&'static str],
    factory: fn(&mut clap::Command) -> &mut clap::Command,
}

const COMMAND_GROUPS: &[CommandGroupDescriptor] = &[
    CommandGroupDescriptor { names: &["provider"], factory: register_provider_group },
    CommandGroupDescriptor { names: &["mcp"], factory: register_mcp_group },
    // ...
];
```

12. **Fish 路径辅助函数**（参照 openclaw）：
```fish
function __laew_command_path_matches
  # 帮助 fish 匹配嵌套命令路径
end
```

---

## 九、推荐 Rust crate 清单

### 9.1 CLI 框架与补全

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `clap` | 4.5 | CLI 解析（derive + builder） | 已在用 |
| `clap_complete` | 4.5 | 5 Shell 补全生成 | 新增 |
| `clap_complete_nushell` | 4.5 | Nushell 补全 | 可选 |
| `clap-verbosity-flag` | 3.0 | `-v`/`-vv`/`-vvv` 自动 | 新增 |
| `clap_mangen` | 0.2 | man page 生成 | 可选 |

### 9.2 交互式提示

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `dialoguer` | 0.11 | 文本/密码/选择/多选/确认 | `@clack/prompts` |
| `inquire` | 0.7 | 强类型选择 + 自动补全 | `@inquirer/prompts` |
| `rpassword` | 7.3 | 密码隐藏输入 | `@clack/prompts.password()` |
| `console` | 0.15 | 终端控制（颜色、光标） | chalk |
| `indicatif` | 0.17 | 进度条 + spinner | `@clack/prompts.spinner()` |

### 9.3 配置与环境变量

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `figment` | 0.10 | 多源配置合并（TOML/JSON/YAML/env） | openclaw profile |
| `config` | 0.14 | 配置库 | 同上 |
| `dotenvy` | 0.15 | `.env` 文件加载 | 已有 |
| `envy` | 0.4 | env → struct 反序列化 | pi 40+ env vars |

### 9.4 错误处理

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `thiserror` | 1.0 | 自定义错误 derive | 已在用 |
| `anyhow` | 1.0 | 通用错误包装 | 可选 |
| `color-eyre` | 0.6 | 美化错误报告 + source chain | opencode `input.cause` |
| `human-panic` | 2.0 | panic 时打印友好信息 | openclaw |

### 9.5 终端/TUI 渲染

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `crossterm` | 0.28 | 跨平台终端控制 | 已在用 |
| `ratatui` | 0.29 | TUI 框架 | 替代品（如需重写） |
| `termion` | 2.0 | 类 Unix 终端控制 | 可选 |
| `unicode-width` | 0.1 | Unicode 字符宽度 | 已在用（间接） |

### 9.6 序列化与 JSON

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `serde` | 1.0 | 序列化框架 | 已在用 |
| `serde_json` | 1.0 | JSON | 已在用 |
| `serde_yaml` | 0.9 | YAML | 可选 |
| `toml` | 0.8 | TOML | 已在用 |

### 9.7 Shell 集成

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `shell-escape` | 0.1 | Unix shell 参数转义 | `quote-cli-arg.ts` |
| `shell-words` | 1.1 | Unix shell 词法分析 | openclaw |
| `dirs` | 5.0 | 用户目录定位 | openclaw `homedir()` |

### 9.8 补全进阶

| crate | 版本 | 用途 | 替代对象 |
|-------|------|------|---------|
| `clap_complete_fig` | 4.5 | Fig 补全 | 可选 |
| `clap_complete_aid` | 0.1 | Aid 补全（嵌入式） | 可选 |

### 9.9 完整推荐清单（精简版）

```toml
# Cargo.toml
[dependencies]
# CLI 框架（已在用 + 新增）
clap = { version = "4", features = ["derive", "cargo", "wrap_help"] }
clap_complete = "4"

# 交互式提示
dialoguer = { version = "0.11", default-features = false }
indicatif = "0.17"
console = "0.15"

# 配置 + 环境变量
figment = { version = "0.10", features = ["toml", "yaml", "env"] }
dotenvy = "0.15"

# 错误处理
thiserror = "1"
color-eyre = "0.6"

# 序列化
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Shell 集成
shell-words = "1.1"
dirs = "5"

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

### 9.10 推荐架构（laew v0.4+）

```
src/
├── main.rs                  # clap::Parser derive + try_main()
├── cli/
│   ├── mod.rs               # 公共类型 + 工具函数
│   ├── commands/
│   │   ├── mod.rs           # enum Commands derive
│   │   ├── print.rs         # -p 单轮
│   │   ├── file.rs          # -f 文件提示词
│   │   ├── tui.rs           # TUI 入口
│   │   ├── provider.rs      # /provider 子命令组
│   │   ├── completion.rs    # completion 子命令
│   │   └── internal.rs      # 隐藏内部命令
│   ├── prompts.rs           # dialoguer 封装
│   ├── completion.rs        # 自动补全生成 + 安装
│   ├── diagnostics.rs       # 错误累积
│   ├── output.rs            # text/JSON 输出
│   ├── profile.rs           # --profile 隔离
│   └── error.rs             # CLI 错误类型
```

---

## 十、关键源码路径速查

### 10.1 atomcode

| 文件 | 行数 | 关键内容 |
|------|------|---------|
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-cli/src/main.rs` | 4987 | `Cli`/`Commands` derive、`build_i18n_command`、`try_print_shell_completion` |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-cli/src/askpass.rs` | - | Unix Socket 密码输入 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-cli/Cargo.toml` | - | clap 4 + clap_complete 4 |
| `/usr/local/LsmGitOpenSource/atomcode/crates/atomcode-clix/src/main.rs` | 1852 | 独立 clix 工具入口 |

### 10.2 claudecode

| 文件 | 行数 | 关键内容 |
|------|------|---------|
| `/usr/local/LsmGitOpenSource/claudecode/src/main.tsx` | 4683 | Commander Command + 24+ 子命令 |
| `/usr/local/LsmGitOpenSource/claudecode/src/commands.ts` | 754 | 内部 slash command 注册 |
| `/usr/local/LsmGitOpenSource/claudecode/src/entrypoints/cli.tsx` | - | 极简 fast-path 入口 |
| `/usr/local/LsmGitOpenSource/claudecode/src/types/command.ts` | - | `CommandBase`/`Command` 类型 |
| `/usr/local/LsmGitOpenSource/claudecode/src/cli/handlers/` | - | 懒加载 handler |

### 10.3 openclaw

| 文件 | 行数 | 关键内容 |
|------|------|---------|
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/run-main.ts` | 1697 | 主入口 `runCli` |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/program/build-program.ts` | - | `buildProgram()` |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/program/command-registry.ts` | - | 程序命令注册 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/program/command-registry-core.ts` | - | 24+ 命令组描述符 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/program/openclaw-command.ts` | - | `OpenClawCommand` 错误捕获 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/program/help.ts` | - | 自定义 help 格式化 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/completion-cli.ts` | 598 | Shell 补全生成（bash/zsh/fish/powershell） |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/completion-runtime.ts` | 530 | 缓存路径 + profile 安装 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/completion-bash.ts` | 176 | Bash 补全生成 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/completion-fish.ts` | 54 | Fish 路径辅助函数 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/completion-command-tree.ts` | 103 | 命令树提取 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/prompt.ts` | 66 | 简化 readline prompts |
| `/usr/local/LsmGitOpenSource/openclaw/src/wizard/clack-prompter.ts` | - | Clack 交互式 UI |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/json-output-mode.ts` | - | JSON 输出模式 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/profile.ts` | - | Profile 隔离 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/argv.ts` | - | Argv 工具函数 |
| `/usr/local/LsmGitOpenSource/openclaw/src/cli/program/route-args.ts` | - | Fast-path 参数解析 |

### 10.4 opencode

| 文件 | 行数 | 关键内容 |
|------|------|---------|
| `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/index.ts` | 200 | yargs 18 主入口 |
| `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/cli/cmd/cmd.ts` | - | `cmd<T>` 类型 helper |
| `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/cli/cmd/run.ts` | - | `RunCommand` 完整 builder |
| `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/cli/effect-cmd.ts` | - | Effect 包装 + `CliError` |
| `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/cli/error.ts` | - | `FormatError` 统一错误 |
| `/usr/local/LsmGitOpenSource/opencode/packages/opencode/src/cli/cmd/agent.ts` | - | `prompts.select()` 用法 |

### 10.5 pi

| 文件 | 行数 | 关键内容 |
|------|------|---------|
| `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/cli/args.ts` | 447 | 自研 `parseArgs` + `printHelp` |
| `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/cli/experimental/command.ts` | - | 类型化 Command<T> 抽象 |
| `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/cli/experimental/commands/pi.ts` | - | `piCommand` 实例 |
| `/usr/local/LsmGitOpenSource/pi/packages/coding-agent/src/main.ts` | - | parseArgs 调用 + diagnostics 处理 |

### 10.6 cc-switch

| 文件 | 行数 | 关键内容 |
|------|------|---------|
| `/usr/local/LsmGitOpenSource/cc-switch/src-tauri/src/main.rs` | - | Tauri 主入口 |
| `/usr/local/LsmGitOpenSource/cc-switch/src-tauri/src/lib.rs` | - | `#[tauri::command] run()` |
| `/usr/local/LsmGitOpenSource/cc-switch/src-tauri/src/commands/` | 15387 | 30+ IPC 命令 |
| `/usr/local/LsmGitOpenSource/cc-switch/src-tauri/src/error.rs` | - | `AppError` 错误类型 |

---

## 十一、总结

### 11.1 核心发现

1. **Rust 阵营**（atomcode、cc-switch）使用 `clap 4` derive + `clap_complete` 自动生成 5 种 Shell 补全。atomcode 还实现了 **i18n 帮助文本**（`Msg::CliAbout` + `mut_arg`/`mut_subcommand`）和 **预扫描 `--lang`** 在 clap 解析之前。

2. **TypeScript Commander 阵营**（claudecode、openclaw）使用 `@commander-js/extra-typings` / 自定义 `OpenClawCommand` 子类。claudecode 强调 **fast-path**（`--version` 零导入）和 **feature gating**（`feature('CHICAGO_MCP')`）；openclaw 强调 **lazy-import 24+ 子命令** 和 **完整的 Shell 补全生态**（自动写 `.bashrc`、fish 路径辅助、Windows 编码检测）。

3. **TypeScript yargs 阵营**（opencode）使用 yargs 18 + Effect 集成。`effect-cmd.ts` 包装 handler 为 Effect，提供 `Schema.TaggedErrorClass` + `FormatError` 统一错误链（`input.cause` 递归）。

4. **TypeScript 自研阵营**（pi）是**唯一零依赖**的项目。手写 447 行 `parseArgs`，采用 **diagnostics 累积模式**（不抛异常）和 **`unknownFlags` Map 收集**（传递给 extensions）。

5. **Tauri 阵营**（cc-switch）没有传统 CLI，而是 30+ `#[tauri::command]` IPC 函数暴露给 React 前端。

### 11.2 laew 优先级建议

| 优先级 | 改动 | 参考实现 | 工作量 |
|--------|------|---------|--------|
| **P0** | 集成 `clap_complete` + 5 Shell 补全生成 | atomcode | 2-3 天 |
| **P0** | 添加 `--output-format json\|text\|jsonl` | atomcode | 1 天 |
| **P0** | 使用 `dialoguer` 替代裸 readline 输入密码 | opencode | 1 天 |
| **P1** | 添加 `--profile <name>` 配置隔离 | openclaw | 1-2 天 |
| **P1** | 引入 `diagnostics` 错误累积模式 | pi | 2-3 天 |
| **P1** | 添加 `value_hint = ValueHint::FilePath` 路径补全提示 | atomcode | 0.5 天 |
| **P2** | Shell 补全 Profile 自动安装（写 `.bashrc` 等） | openclaw | 3-5 天 |
| **P2** | Command Group 描述符注册表 | openclaw | 1 周 |
| **P2** | preaction 钩子系统 | openclaw | 1 周 |

### 11.3 架构建议

参考 opencode + openclaw 混合模式：
- 保留 laew 现有 `clap::Parser` derive 结构（清晰、类型安全）
- 引入 `clap_complete` 自动生成补全
- 引入 `dialoguer` 提供交互式 prompt（confirm/password/select）
- 引入 `figment` 统一配置多源（CLI args > env > TOML > defaults）
- 引入 `color-eyre` 美化错误 + 错误链
- 渐进式迁移到 `commandGroupDescriptors` 注册表模式（保留向后兼容）

---

## 附录：6 工程 CLI 范式对比图

```
                      Rust (clap)           TypeScript (框架)            TypeScript (自研)
                      ┌──────────────┐      ┌──────────────┐           ┌──────────────┐
                      │  atomcode    │      │  openclaw    │           │      pi      │
                      │  cc-switch*  │      │  claudecode  │           │  (零依赖)    │
                      │              │      │  (Commander) │           │              │
                      └──────────────┘      │  opencode    │           └──────────────┘
                                           │  (yargs)     │
                                           └──────────────┘
                            │                       │                       │
                            ▼                       ▼                       ▼
                  ┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐
                  │ #[derive(Parser)]│    │ new Command()    │    │ for (let arg ...) │
                  │ enum Commands    │    │ .command(X)      │    │ switch (arg) {}  │
                  │ enum SubCommand  │    │ .option(...)     │    │ if (arg=="--foo")│
                  │ #[arg(...)]      │    │ .action(async)   │    │ result.foo = ... │
                  └──────────────────┘    └──────────────────┘    └──────────────────┘
                            │                       │                       │
                            ▼                       ▼                       ▼
                  ┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐
                  │ clap_complete    │    │ @clack/prompts   │    │ diagnostics[]    │
                  │ ::generate()     │    │ text/select/...  │    │ unknownFlags Map │
                  │ (5 shells)       │    │ (1.7.0)          │    │ @file 参数       │
                  └──────────────────┘    └──────────────────┘    └──────────────────┘
                            │                       │                       │
                            ▼                       ▼                       ▼
                  ┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐
                  │ thiserror        │    │ CommanderError   │    │ chalk 着色       │
                  │ + Result<T, E>   │    │ process.exitCode │    │ + 手写 printHelp │
                  └──────────────────┘    └──────────────────┘    └──────────────────┘
```

*cc-switch 是 Tauri 桌面应用，IPC 命令模式

---

**报告行数**：~2800 行（含源码片段、表格、对比图）。

**核心产出**：
- 6 工程 CLI 框架深度分析
- 40 个 laew 差距（L281-L320）
- 完整 Rust crate 推荐清单
- laew v0.4+ 架构建议
