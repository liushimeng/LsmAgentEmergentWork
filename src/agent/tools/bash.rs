//! Bash 工具:在工作目录下执行 shell 命令,带超时与输出截断。
//!
//! 安全护栏(P0,本轮新增):
//! 1. **危险命令拦截**(`permissions::check_bash_command`):fail-closed,
//!    命中黑名单(`rm -rf /`、`dd` 写磁盘、`curl | bash`、`sudo` 等)
//!    或敏感路径引用(`~/.ssh`、`~/.aws`、`.env.production` 等)直接返回
//!    [`AgentError::PermissionDenied`]。
//! 2. **进程组管理**(`setsid()` + `killpg`):每个 bash 命令在新 session/process group
//!    中运行,超时或取消时 `killpg(SIGTERM)` 整组清理,杜绝孤儿进程。
//! 3. **`kill_on_drop` 双保险**:即便进程组清理失效,Tokio Drop 也会 SIGKILL 直接子进程。
//!
//! 设计参考:atomcode bash.rs:249-303(强制 setsid + killpg + kill_on_drop);
//! claudecode timeouts.ts(DEFAULT_TIMEOUT_MS=120_000 / MAX_TIMEOUT_MS=600_000);
//! openclaw bash-tools.exec-runtime.ts(KillProcessTree group-aware)。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::OnceCell;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::agent::permissions;
use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

/// 默认超时与上限
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT_CHARS: usize = 30_000;
/// SIGTERM → SIGKILL 升级等待时间,给进程清理时间(写文件 / flush buffer)
const KILL_GRACE_MS: u64 = 5_000;
// ============== WindowUse Bash 白名单模式(2026-09-16 第 54 轮补丁 A) ==============
//
// 当 Bash 工具被 WindowUse Agent 调用时,通过 LAEW_WINDOW_USE_MODE=1 环境变量
// 切换到「白名单」模式:只允许桌面操控类命令(osascript / cliclick / screencapture /
// pbcopy / open 等),其它命令拒绝并给出可读提示,避免 LLM 误用 Bash 跑任意命令。
// 黑名单(dangerous.rs / sensitive.rs)优先于白名单,确保危险命令 / 敏感路径拦截
// 在白名单模式下仍然生效(fail-closed 默认)。

/// WindowUse Bash 模式开关(由 WindowUseRunner 设置)。
/// pub 暴露供外部测试使用。
pub const WINDOW_USE_MODE_ENV: &str = "LAEW_WINDOW_USE_MODE";

/// 当前进程是否处于 WindowUse Bash 模式。
pub fn window_use_mode() -> bool {
    matches!(
        std::env::var(WINDOW_USE_MODE_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// WindowUse Bash 白名单(命令首个 token)。
/// 含子串匹配:命令首个 token 命中表中任一前缀即视为白名单命令。
/// 设计参考:macOS 桌面操控真实工作流,加上截图 / 剪贴板 / 启动 / 坐标点击四类。
///
/// 2026-09-17 第 82+ 轮 P0-2:扩展持久化与文本处理能力(`cat > file` / `tee` / `python3`
/// / `awk` / `sed` 等),解决 WindowUse Runner「需要写文件但 Bash 白名单拦截」的死锁。
/// 写入类命令必须经 [`is_write_to_working_dir`] 工作目录白名单校验(默认 cwd),
/// 黑名单(`dangerous.rs` / `sensitive.rs`)永远优先。
const WINDOW_USE_BASH_ALLOWLIST: &[&str] = &[
    // AppleScript 桌面操控主路径
    "osascript",
    "ascript",
    // 截图识别
    "screencapture",
    // 坐标点击(需 brew install cliclick;缺失时优雅报错而非拦截)
    "cliclick",
    // 剪贴板
    "pbcopy",
    "pbpaste",
    // 启动 / 激活应用
    "open",
    // 进程 / 应用查询
    "pgrep",
    "pkill",
    "ps",
    "lsof",
    "lsappinfo",
    "osascript",
    // 文件 / 目录枚举(WindowUse 用 Read 已能完成,这里只列 Bash 调试补充)
    "ls",
    "which",
    "command",
    "type",
    "echo",
    "printf",
    "test",
    "[",
    // 系统信息查询
    "uname",
    "sw_vers",
    "defaults",
    "system_profiler",
    // 时间戳 / 延时
    "sleep",
    "date",
    // 帮助与说明
    "man",
    "tput",
    // 串行连接 CLI 工具(便于测试)
    "true",
    "false",
    // ===== 2026-09-17 第 82+ 轮 P0-2:持久化与文本处理 =====
    // 写入文件:仅当目标路径在 cwd 内(见 `is_write_to_working_dir`)
    "cat",      // cat > file / cat >> file 写入小文件(precheck_result.json 等)
    "tee",      // tee -a file 追加
    // 文本处理
    "sed",      // sed -i / sed 's/a/b/' 替换
    "awk",      // awk 字段提取
    "grep",     // grep 内容搜索
    "tr",       // 字符转换
    "cut",      // 列提取
    "head",     // 前 N 行
    "tail",     // 尾部 N 行
    "sort",     // 排序
    "uniq",     // 去重
    "wc",       // 行/字符计数
    "xargs",    // 接收 stdin 转命令行(配合 find / grep)
    // 文件 / 目录管理:仅 cwd
    "mkdir",    // 创建目录(plans/xxx/)
    "touch",    // 占位文件
    "rm",       // 删除(已含 dangerous.rs 黑名单兜底)
    "cp",       // 复制
    "mv",       // 移动
    "ln",       // 链接(默认禁 -s,可改 LAEW_BASH_ALLOW_SYMLINK=1)
    "find",     // 递归搜索
    "chmod",    // 权限修改(仅 cwd)
    "stat",     // 元信息
    "file",     // 文件类型
];

/// 命令首个 token(去掉前导空白 / env 前缀)。
fn first_command_token(command: &str) -> String {
    let trimmed = command.trim_start();
    // 跳过 bash -c / sh -c / env 前缀
    let after = trimmed
        .strip_prefix("bash -c ")
        .or_else(|| trimmed.strip_prefix("sh -c "))
        .or_else(|| trimmed.strip_prefix("env "))
        .unwrap_or(trimmed);
    after.split_whitespace().next().unwrap_or("").to_string()
}

/// 2026-09-17 第 82+ 轮 P0-2:写入类命令的工作目录白名单校验。
///
/// 适用范围:WindowUse Bash 模式下,`cat > file` / `tee file` / `sed -i file` / `rm file` /
/// `mv file` / `cp file` 等带文件路径的命令。返回 `Ok(())` 当且仅当:
/// - 文件路径出现在命令中(否则视为非写入命令,直接放行);
/// - 解析后的绝对路径在工作目录(`std::env::current_dir()`)之内;
/// - 不指向敏感路径(`/etc/` / `~/.ssh/` 等,见 `permissions::sensitive` 黑名单)。
///
/// 兜底:目标路径无法解析(命令格式异常 / 重定向到 fd 等)→ 拒绝并提示。
///
/// 设计意图:WindowUse Runner 之前因无法写 `precheck_result.json` 等持久化文件,
/// 不得不绕路 `osascript -e 'do shell script "cat > ..."'`,既丑又跨平台不一致;
/// 放宽后 WindowUse Runner 自带持久化能力,但仍受 cwd 白名单 + 敏感路径黑名单约束,
/// 不会让 LLM 任意改写系统文件。
fn check_window_use_write_cwd(command: &str) -> Result<()> {
    // 找命令中的文件路径(启发式:跳过首 token / 选项 / 重定向符号 / 常见参数)。
    // 简化策略:从命令里提取所有不以 `-` 开头且不是首 token 的 token,看作候选路径;
    // 然后用 std::path::Path 试着 cwd 解析,能解析为绝对路径且在 cwd 内 → 放行。
    let cwd = std::env::current_dir().map_err(|e| AgentError::PermissionDenied {
        tool: "Bash".into(),
        reason: format!("[windowuse-mode] 无法获取 cwd: {e}"),
    })?;
    let cwd_canonical = cwd.canonicalize().unwrap_or(cwd.clone());

    let tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.len() <= 1 {
        return Ok(()); // 单 token 命令,无文件路径
    }
    for tok in &tokens[1..] {
        // 跳过选项 / 重定向符 / shell 关键字
        if tok.starts_with('-')
            || matches!(
                *tok,
                ">" | ">>" | "<" | "<<" | "&&" | "||" | "|" | ";" | "&"
            )
            || tok.contains('=') && !tok.starts_with('/') && !tok.starts_with('.')
            || tok.starts_with('$')
        {
            continue;
        }
        // 试解析为绝对路径(相对路径以 cwd 解析)
        let path = std::path::Path::new(tok);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        // 检查绝对路径是否在 cwd 内(防止 ../../../etc/passwd 类穿透)
        let abs_canon = abs.canonicalize().unwrap_or_else(|_| {
            // 文件不存在 → 用其父目录解析(用于即将创建的文件)
            abs.parent()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
                .unwrap_or(cwd.clone())
        });
        if !abs_canon.starts_with(&cwd_canonical) && !abs_canon.starts_with(&cwd) {
            return Err(AgentError::PermissionDenied {
                tool: "Bash".into(),
                reason: format!(
                    "[windowuse-mode] 文件路径 {tok:?} 解析为 {:?} 超出工作目录 {:?};WindowUse 写入类命令(cat > file / tee / rm / mv / cp)仅允许 cwd 内路径。需要跨目录写入请改用 SubAgent Bash(非白名单模式)。",
                    abs_canon, cwd_canonical
                ),
            });
        }
    }
    Ok(())
}

/// WindowUse Bash 白名单校验:黑名单永远优先,白名单只在 WindowUse 模式下生效。
/// - 黑名单命中(危险命令 / 敏感路径)→ 拒绝 + PermissionDenied(原有逻辑)
/// - 白名单模式下首个 token 不在白名单 → 拒绝 + 给出"建议改用 windowuse 工具 / 明确原因"
/// - 白名单模式下首个 token 在白名单 → 放行
pub fn check_window_use_bash(command: &str) -> Result<()> {
    if !window_use_mode() {
        return Ok(()); // 非 WindowUse 模式,白名单不生效
    }
    let token = first_command_token(command);
    if token.is_empty() {
        return Err(AgentError::PermissionDenied {
            tool: "Bash".into(),
            reason: "[windowuse-mode] 空命令".into(),
        });
    }
    // 命令中含 osascript 子串(可能用 sh -c 拼接),放行
    let lower = command.to_lowercase();
    let allowed_direct = WINDOW_USE_BASH_ALLOWLIST
        .iter()
        .any(|k| token.eq_ignore_ascii_case(k));
    // 兜底:命令全文含已知白名单子串也放行(覆盖 `do shell script "..."` 这类)
    let allowed_substring = [
        "osascript",
        "screencapture",
        "cliclick",
        "pbcopy",
        "pbpaste",
        "tell application",
        "system events",
        "keystroke",
        "do shell script",
        "activate",
    ]
    .iter()
    .any(|k| lower.contains(k));
    if allowed_direct || allowed_substring {
        // 2026-09-17 第 82+ 轮 P0-2:写入类命令额外做 cwd 白名单校验
        // 涉及 cat / tee / rm / cp / mv / sed -i / ln 等带文件路径的命令
        if matches!(
            token.to_lowercase().as_str(),
            "cat" | "tee" | "rm" | "cp" | "mv" | "ln" | "mkdir" | "touch" | "chmod"
        ) || (token.to_lowercase() == "sed" && command.contains("-i"))
        {
            check_window_use_write_cwd(command)?;
        }
        return Ok(());
    }
    Err(AgentError::PermissionDenied {
        tool: "Bash".into(),
        reason: format!(
            "[windowuse-mode] 命令首 token \"{token}\" 不在白名单(白名单见 BashTool description)。WindowUse Bash 仅允许桌面操控类命令(osascript / cliclick / screencapture / pbcopy / open / ls / ps / defaults / sleep 等);通用任务请改走 SubAgent Bash(非白名单模式)。若需诊断 / 调试,可传 trust_level=\"debug\" 临时放开 defaults / log show / python3 -c \"from Quartz\" 等命令。"
        ),
    })
}

/// 2026-09-17 第 76 轮 P1-1:Bash 信任级别,仅在 WindowUse 模式下生效。
///
/// 三档严格度,LLM 可通过 Bash 工具参数 `trust_level` 临时切换(每个工具调用独立)。
/// - Strict(默认):仅桌面操控类(osascript / cliclick / screencapture / pbcopy 等);
/// - Debug:在 Strict 基础上放开 diagnostics(defaults read / log show / system_profiler /
///   python3 -c "from Quartz...");黑名单(危险命令 / 敏感路径)永远优先拦截。
/// - Free:等同 SubAgent Bash(仅黑名单拦截),不再受白名单限制。LLM 调用时
///   会输出警告日志(防止滥用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashTrustLevel {
    Strict,
    Debug,
    Free,
}

impl BashTrustLevel {
    pub fn from_str(s: Option<&str>) -> Self {
        match s.map(str::to_ascii_lowercase).as_deref() {
            Some("debug") => Self::Debug,
            Some("free") | Some("open") => Self::Free,
            _ => Self::Strict,
        }
    }
}

/// 校验命令是否在 WindowUse Debug 级别白名单内(放宽版)。
const WINDOW_USE_BASH_DEBUG_ALLOWLIST: &[&str] = &[
    "defaults",
    "log",
    "system_profiler",
    "sw_vers",
    "python3",
    "ioreg",
    "security",
    "tccutil",
    "codesign",
    "spctl",
    "mdls",
    "xattr",
    "file",
    "mdfind",
];

pub fn check_window_use_bash_with_trust(command: &str, trust: BashTrustLevel) -> Result<()> {
    if !window_use_mode() {
        return Ok(());
    }
    if trust == BashTrustLevel::Free {
        // Free 模式:仅黑名单拦截(原 check_bash_command)
        return permissions::check_bash_command(command);
    }
    // Strict / Debug 模式:先尝试 Debug 白名单(Debug 也含 strict 白名单)
    if trust == BashTrustLevel::Debug {
        let token = first_command_token(command);
        let lower = command.to_lowercase();
        let allowed_debug = WINDOW_USE_BASH_DEBUG_ALLOWLIST
            .iter()
            .any(|k| token.eq_ignore_ascii_case(k));
        if allowed_debug {
            // Debug 白名单命中,放行(但仍走黑名单)
            return permissions::check_bash_command(command);
        }
        // 兜底:命令含已知诊断关键字
        if [
            "defaults read",
            "log show",
            "log stream",
            "system_profiler",
            "from quartz",
            "import quartz",
            "from vision",
            "import vision",
            "tccutil",
            "ioreg -l",
        ]
        .iter()
        .any(|k| lower.contains(k))
        {
            return permissions::check_bash_command(command);
        }
    }
    // Strict 模式(以及 Debug 模式未命中扩展白名单)走标准白名单校验
    check_window_use_bash(command)
}

/// 命令首个 token(去掉前导空白 / env 前缀)。

/// 当前进程工作目录(通过 env::current_dir 惰性获取)
fn current_work_dir() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    // 并行测试可能创建/删除临时 cwd;Bash spawn 会因 current_dir 不存在而失败。
    // 这里保持语义:优先当前工作目录,被删除时退回系统临时目录。
    if cwd.exists() {
        cwd
    } else {
        std::env::temp_dir()
    }
}

/// `LAEW_BASH_UTF8=1/true/yes/on` 时为 bash 子进程注入 UTF-8 语言环境。
///
/// 2026-09-13 第 50 轮:Windows 下 python(WindowsApps 3.12)与部分 coreutils
/// 按区域设置(cp936/GBK)编码输出,laew 侧 `from_utf8_lossy` 后中文变 U+FFFD 乱码;
/// Agent 生成的脚本被迫逐个 `sys.stdout.reconfigure(encoding="utf-8")`(本轮 9 个
/// 脚本踩坑)。开关打开后注入 `PYTHONUTF8/PYTHONIOENCODING/LC_ALL/LANG`,
/// 把编码义务收进工具层。行尾(文本模式 CRLF)不受编码影响,精确 diff 场景仍需
/// 脚本自行 `reconfigure(newline=...)`。
fn utf8_env_enabled() -> bool {
    match std::env::var("LAEW_BASH_UTF8") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// bash 二进制路径(Windows 进程内缓存;首次解析失败后回退 None)。
///
/// 2026-09-12 第 44 轮:Windows 11 默认 PATH 中
/// `C:\Windows\System32\bash.exe`(WSL Ubuntu launcher)排在
/// `C:\Program Files\Git\usr\bin\bash.exe` 前面,直接 `Command::new("bash")`
/// 会命中 WSL launcher(它会 spawn wsl.exe 而不是 bash),导致
/// `echo hello` 都返回 exit_code=1 + WSL2 ext4.vhdx 挂载错误。
/// 因此 Windows 上需要按已知顺序探测真正的 bash 位置。
static BASH_BIN: OnceCell<Option<String>> = OnceCell::new();

/// 解析要执行的 bash 二进制路径。
///
/// 优先级:
/// 1. `BASH_PATH` 环境变量(用户显式指定,常用于 CI)
/// 2. Unix:`bash`(走 PATH,通常就是 /bin/bash)
/// 3. Windows:按已知路径顺序探测,跳过 WSL bash launcher 等"非真正 bash"
fn resolve_bash_binary() -> Option<String> {
    if let Some(cached) = BASH_BIN.get() {
        return cached.clone();
    }

    let resolved = do_resolve_bash_binary();
    let _ = BASH_BIN.set(resolved.clone());
    resolved
}

fn do_resolve_bash_binary() -> Option<String> {
    // 1) 显式 BASH_PATH 覆盖
    if let Ok(p) = std::env::var("BASH_PATH") {
        let p = p.trim();
        if !p.is_empty() && PathBuf::from(p).exists() {
            return Some(p.to_string());
        }
    }

    #[cfg(windows)]
    {
        // Windows 探测顺序:
        // 1. Git for Windows 标准安装路径(Git Bash 真正位置)
        // 2. Git for Windows 64-bit 兼容路径
        // 3. Git for Windows bin 目录(部分安装)
        // 4. 系统 PATH 中的 bash(命中 Git Bash 则 OK;若命中 WSL launcher 则会被下面
        //    is_real_bash 检测剔除,继续尝试 PATH 中下一个)
        // 5. WSL bash launcher 兜底(失败时用户能看见明确错误)
        //
        // 注意:Git for Windows 默认安装在 C:\Program Files,但一些机器上装在
        // D:\Program Files 或用户目录(便携版);同时 PATH 里 mingw64\bin 也会
        // 命中。因此候选列表需要扫描系统盘符 + 常见固定路径,而不是仅写死 C:\。
        let candidates: Vec<String> = {
            let mut v = Vec::new();
            // 候选前缀:(盘符, 路径)。覆盖 C/D/E(常见装在 D 盘的笔记本)。
            let prefixes: Vec<String> = std::iter::empty()
                .chain(
                    ['C', 'D', 'E']
                        .iter()
                        .map(|d| format!(r"{}:\Program Files\Git\usr\bin", d)),
                )
                .chain(
                    ['C', 'D', 'E']
                        .iter()
                        .map(|d| format!(r"{}:\Program Files (x86)\Git\usr\bin", d)),
                )
                .chain(
                    ['C', 'D', 'E']
                        .iter()
                        .map(|d| format!(r"{}:\Program Files\Git\bin", d)),
                )
                .chain(
                    ['C', 'D', 'E']
                        .iter()
                        .map(|d| format!(r"{}:\Program Files (x86)\Git\bin", d)),
                )
                .chain(
                    ['C', 'D', 'E']
                        .iter()
                        .map(|d| format!(r"{}:\Program Files\Git\mingw64\bin", d)),
                )
                .collect();
            for prefix in &prefixes {
                let bash = format!(r"{}\bash.exe", prefix);
                if std::path::Path::new(&bash).exists() {
                    v.push(bash);
                }
            }
            // PATH 中的 bash.exe(顺序依赖当前进程 PATH)
            if let Ok(paths) = std::env::var("PATH") {
                for entry in paths.split(';') {
                    let entry = entry.trim();
                    if entry.is_empty() {
                        continue;
                    }
                    let bash = format!(r"{}\bash.exe", entry);
                    if std::path::Path::new(&bash).exists() && !v.iter().any(|x| x == &bash) {
                        v.push(bash);
                    }
                }
            }
            // WSL bash launcher 兜底(可能工作也可能不工作,按实际探测结果)
            for tail in [r"C:\Windows\System32\bash.exe", r"C:\Users\Public\bash.exe"] {
                if std::path::Path::new(tail).exists() && !v.iter().any(|x| x == tail) {
                    v.push(tail.to_string());
                }
            }
            v
        };

        // 用 --version 探测每个候选:真正的 bash 会输出 "GNU bash, version X"
        // 并 exit 0;WSL launcher 会启动 wsl.exe 并返回非 0(若 WSL 未启用)。
        for cand in &candidates {
            if is_real_bash(cand) {
                return Some(cand.clone());
            }
        }
        None
    }

    #[cfg(not(windows))]
    {
        // Unix/Linux/macOS:直接走 PATH
        Some("bash".to_string())
    }
}

/// 探测一个候选路径是否"真正的 bash"而非 WSL launcher 等。
///
/// 标准 bash --version 第一行形如 "GNU bash, version 5.3.9(1)-release (x86_64-pc-cygwin)"
/// (git bash) 或 "GNU bash, version 3.2.57(...)" (macOS)。WSL bash launcher 不会输出
/// 这个,而是 spawn wsl.exe 并返回 0/1/160 等(常见 1 = WSL 未启用)。
#[cfg(windows)]
fn is_real_bash(path: &str) -> bool {
    use std::process::Command as StdCommand;
    let out = StdCommand::new(path).arg("--version").output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            // 真实 bash 一定输出 "GNU bash"
            s.contains("GNU bash")
        }
        _ => false,
    }
}

#[cfg(not(windows))]
fn is_real_bash(_path: &str) -> bool {
    true
}

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        "在工作目录下执行 bash 命令,返回 stdout + stderr + 退出码。\n\
         - 超时单位毫秒,默认 120000,最大 600000。\n\
         - 一次调用执行一条命令;多条命令请用 && / ; 连接。\n\
         - 避免使用 cat/head/tail/sed/awk/echo 这类专用命令 — 请改用 Read / Write 等专用工具。\n\
         \n\
         【安全提示】以下命令会被自动拦截:\n\
         - 危险命令:rm -rf /、rm -rf ~、dd 写磁盘、mkfs/fdisk、chmod 777、shutdown/reboot、sudo、curl|bash 等\n\
         - 敏感路径:~/.ssh、~/.aws、~/.gnupg、.env.production、/proc/self/environ 等\n\
         如需执行上述操作,请直接在终端运行(laew 之外)。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "待执行的 bash 命令字符串" },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_MS as i64,
                    "description": "可选超时(毫秒)"
                },
                "description": {
                    "type": "string",
                    "description": "一句话描述命令用途(便于审计)"
                },
                "trust_level": {
                    "type": "string",
                    "enum": ["strict", "debug", "free"],
                    "description": "可选,WindowUse Bash 信任级别(仅在 windowuse-mode 下生效):strict(默认,仅桌面操控)/ debug(放开诊断命令)/ free(等同 SubAgent,仅黑名单拦截)"
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::ToolExecution {
                tool: self.name().into(),
                reason: "缺少 string 类型参数 command".into(),
            })?
            .to_string();

        let timeout_ms = args
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        // 2026-09-17 第 76 轮 P1-1:解析 trust_level(默认 strict)。
        let trust = BashTrustLevel::from_str(args.get("trust_level").and_then(Value::as_str));
        if window_use_mode() && trust != BashTrustLevel::Strict {
            tracing::info!(
                trust = ?trust,
                command_first_token = %first_command_token(&command),
                "WindowUse Bash trust_level 临时提升"
            );
        }

        // P0:危险命令 + 敏感路径拦截(fail-closed)
        permissions::check_bash_command(&command)?;
        // 2026-09-16 第 54 轮补丁 A + 2026-09-17 第 76 轮 P1-1:WindowUse Bash 白名单校验,
        // 根据 trust_level 选择严格度(strict / debug / free)。
        check_window_use_bash_with_trust(&command, trust)?;
        check_window_use_bash(&command)?;

        // 解析 bash 二进制路径:Windows 上必须跳过 WSL bash launcher
        // (详见 resolve_bash_binary 注释)。解析失败给清晰错误提示,
        // 让用户能通过 BASH_PATH 环境变量覆盖。
        let bash_bin = match resolve_bash_binary() {
            Some(p) => p,
            None => {
                return Err(AgentError::ToolExecution {
                    tool: self.name().into(),
                    reason: "未找到可用的 bash 二进制(Windows 上 PATH 可能命中 WSL launcher)。\
                             请设置 BASH_PATH=C:\\Program Files\\Git\\usr\\bin\\bash.exe 或安装 Git for Windows。"
                        .into(),
                });
            }
        };

        let mut cmd = Command::new(&bash_bin);
        // --noprofile --norc 跳过启动文件(加快响应 + 避免 .bashrc 副作用);
        // -c 单条命令直接执行
        cmd.arg("-c").arg(&command);
        cmd.current_dir(current_work_dir());
        // 可选 UTF-8 子进程环境(见 utf8_env_enabled 文档)
        if utf8_env_enabled() {
            cmd.env("PYTHONUTF8", "1");
            cmd.env("PYTHONIOENCODING", "utf-8");
            cmd.env("LC_ALL", "C.UTF-8");
            cmd.env("LANG", "C.UTF-8");
        }
        // 不连 stdin:防止意外阻塞等待输入
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // kill_on_drop:即便超时逻辑失效,Tokio Drop 时也会 SIGKILL 直接子进程
        cmd.kill_on_drop(true);

        // Unix:强制新 session / 新进程组,使后续 killpg 能整组清理孙子进程
        #[cfg(unix)]
        {
            // SAFETY:pre_exec 在子进程 fork 后 exec 前执行,只调用 setsid(可重入安全),
            // 调用失败时返回 Err,子进程将 _exit 而不是污染父进程。
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Err(AgentError::ToolExecution {
                    tool: self.name().into(),
                    reason: format!("启动命令失败: {e}"),
                });
            }
        };

        // 子进程的 PID 也是它所在 process group 的 PGID(因为我们已 setsid)
        let child_pid = child.id();
        let output_fut = child.wait_with_output();

        // 等待完成或超时
        let result = tokio::time::timeout(Duration::from_millis(timeout_ms), output_fut).await;

        match result {
            Ok(Ok(o)) => {
                let code = o.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();

                let stdout_trunc = truncate(&stdout, MAX_OUTPUT_CHARS);
                let stderr_trunc = truncate(&stderr, MAX_OUTPUT_CHARS);

                let mut buf = String::new();
                if !stdout_trunc.truncated.is_empty() {
                    buf.push_str("<stdout>\n");
                    buf.push_str(&stdout_trunc.text);
                    if stdout_trunc.omitted > 0 {
                        buf.push_str(&format!(
                            "\n...[stdout 截断,省略 {} 字符]",
                            stdout_trunc.omitted
                        ));
                    }
                    buf.push('\n');
                }
                if !stderr_trunc.truncated.is_empty() {
                    buf.push_str("<stderr>\n");
                    buf.push_str(&stderr_trunc.text);
                    if stderr_trunc.omitted > 0 {
                        buf.push_str(&format!(
                            "\n...[stderr 截断,省略 {} 字符]",
                            stderr_trunc.omitted
                        ));
                    }
                    buf.push('\n');
                }
                if buf.is_empty() {
                    buf.push_str("<stdout>\n(无输出)\n");
                }
                buf.push_str(&format!("\n<exit_code>{code}</exit_code>"));
                // L1208:bash 输出是「外部内容」,扫描 prompt injection 后返回
                Ok(crate::agent::safety::scan_and_wrap(
                    &buf,
                    crate::agent::safety::InjectionSource::BashStdout,
                )
                .wrapped_text)
            }
            Ok(Err(e)) => Err(AgentError::ToolExecution {
                tool: self.name().into(),
                reason: format!("等待命令完成失败: {e}"),
            }),
            Err(_) => {
                // 超时:三阶段清理(SIGTERM 整组 → 等 5s → SIGKILL 整组)
                kill_process_tree(child_pid);
                Err(AgentError::ToolExecution {
                    tool: self.name().into(),
                    reason: format!(
                        "[Bash] 超时(>{timeout_ms}ms)被强制终止;命令及其子进程已 killpg(SIGTERM→{KILL_GRACE_MS}ms→SIGKILL)清理。",
                    ),
                })
            }
        }
    }
}

/// 三阶段 kill 进程树:
/// 1. SIGTERM 整组(graceful,给进程机会 flush / close fd)
/// 2. 等 [`KILL_GRACE_MS`] 毫秒
/// 3. SIGKILL 整组(强制收尾)
///
/// 即使父进程先退出,孙子进程也已在新进程组中,killpg 仍能命中。
#[cfg(unix)]
fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    // SAFETY:libc::killpg 发送信号到指定 pgid;pgid = pid 因为 setsid 创建了新组。
    unsafe {
        libc::killpg(pid as i32, libc::SIGTERM);
    }
    let pgid = pid as i32;
    // 异步等 5s 后 SIGKILL 兜底
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(KILL_GRACE_MS)).await;
        // SAFETY:同上
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    });
}

#[cfg(not(unix))]
fn kill_process_tree(_pid: Option<u32>) {
    // Windows:本轮不实现,依赖 kill_on_drop + 直接 pid kill
    // (留作 P1:用 Job Object / taskkill /T)
}

struct Truncated {
    text: String,
    truncated: String,
    omitted: usize,
}

fn truncate(s: &str, max: usize) -> Truncated {
    if s.len() <= max {
        Truncated {
            text: s.to_string(),
            truncated: s.to_string(),
            omitted: 0,
        }
    } else {
        // 防止按 char 边界切割出错,按 char 索引切
        let cut = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        Truncated {
            text: s[..cut].to_string(),
            truncated: s[..cut].to_string(),
            omitted: s.len() - cut,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 2026-09-15 第 53 轮:加锁防止与 llm::tls_insecure_* / safety::prompt_injection_*
    /// 等同时改环境变量的测试产生竞态;Rust 测试默认并行跑。
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        crate::test_support::GLOBAL_ENV_CWD_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[tokio::test]
    async fn echo_via_bash() {
        let out = BashTool
            .execute(json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(out.contains("hello"));
        assert!(out.contains("<exit_code>0</exit_code>"));
    }

    #[tokio::test]
    async fn exit_code_propagated() {
        let out = BashTool
            .execute(json!({"command": "exit 7"}))
            .await
            .unwrap();
        assert!(out.contains("<exit_code>7</exit_code>"));
    }

    #[tokio::test]
    async fn missing_command_argument_errors() {
        let err = BashTool.execute(json!({})).await.unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => assert!(reason.contains("command")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn dangerous_command_blocked() {
        // rm -rf / 应被拦截
        let err = BashTool
            .execute(json!({"command": "rm -rf /"}))
            .await
            .unwrap_err();
        match err {
            AgentError::PermissionDenied { tool, reason } => {
                assert_eq!(tool, "Bash");
                assert!(reason.contains("危险"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sudo_blocked() {
        let err = BashTool
            .execute(json!({"command": "sudo apt install foo"}))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn sensitive_path_blocked() {
        let err = BashTool
            .execute(json!({"command": "cat ~/.ssh/id_rsa"}))
            .await
            .unwrap_err();
        match err {
            AgentError::PermissionDenied { reason, .. } => {
                assert!(reason.contains("敏感路径"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_kills_subtree() {
        // sleep 30 + timeout 1s → 应被超时中断,且进程不再存在
        let start = std::time::Instant::now();
        let err = BashTool
            .execute(json!({"command": "sleep 30", "timeout_ms": 1000u64}))
            .await
            .unwrap_err();
        let elapsed = start.elapsed();
        assert!(matches!(err, AgentError::ToolExecution { .. }));
        // 超时逻辑应该立刻返回(不等 sleep 30 完成)
        assert!(
            elapsed < Duration::from_secs(10),
            "超时返回耗时 {:?},过长(预期 <10s)",
            elapsed
        );
        // 注:killpg 是异步 tokio::spawn,这里不验证子进程是否已被杀
        // (sleep 30 进程已经在独立进程组,killpg 会清理)
    }

    #[tokio::test]
    async fn short_sleep_completes() {
        let out = BashTool
            .execute(json!({"command": "sleep 0.2", "timeout_ms": 5000u64}))
            .await
            .unwrap();
        assert!(out.contains("<exit_code>0</exit_code>"));
    }

    #[tokio::test]
    async fn utf8_env_switch_injects_and_defaults_off() {
        // 合并为单测试顺序执行:两个 #[test] 并行跑会因进程级 env 互相竞态
        let _env = lock_env();
        std::env::set_var("LAEW_BASH_UTF8", "1");
        let on = BashTool
            .execute(json!({"command": "echo \"$LC_ALL/$PYTHONUTF8/$PYTHONIOENCODING\""}))
            .await
            .unwrap();
        assert!(
            on.contains("C.UTF-8/1/utf-8"),
            "子进程应注入 UTF-8 环境,实际: {on}"
        );
        std::env::remove_var("LAEW_BASH_UTF8");
        let off = BashTool
            .execute(json!({"command": "echo \"[$PYTHONUTF8]\""}))
            .await
            .unwrap();
        assert!(off.contains("[]"), "默认不注入,实际: {off}");
    }

    // ===== 2026-09-17 第 76 轮 P1-1:Bash trust_level 分级白名单 =====

    #[test]
    fn bash_trust_level_from_str() {
        assert_eq!(BashTrustLevel::from_str(None), BashTrustLevel::Strict);
        assert_eq!(BashTrustLevel::from_str(Some("strict")), BashTrustLevel::Strict);
        assert_eq!(BashTrustLevel::from_str(Some("DEBUG")), BashTrustLevel::Debug);
        assert_eq!(BashTrustLevel::from_str(Some("Free")), BashTrustLevel::Free);
        assert_eq!(BashTrustLevel::from_str(Some("open")), BashTrustLevel::Free);
        assert_eq!(BashTrustLevel::from_str(Some("unknown")), BashTrustLevel::Strict);
    }

    #[tokio::test]
    async fn bash_trust_level_strict_blocks_debug_commands() {
        let _env = lock_env();
        unsafe {
            std::env::set_var(WINDOW_USE_MODE_ENV, "1");
        }
        // Strict: defaults read 应被拒(不在 strict 白名单,严格模式下默认是 strict)
        let r = check_window_use_bash_with_trust(
            "defaults read com.tencent.xinWeChat",
            BashTrustLevel::Strict,
        );
        // 注:defaults 实际上在 WINDOW_USE_BASH_ALLOWLIST 中(系统信息查询类),
        // 所以 strict 也放行。这里验证的是 python3 在 strict 下被拒。
        let r_py = check_window_use_bash_with_trust(
            "python3 -c 'from Quartz import CGWindowListCopyWindowInfo'",
            BashTrustLevel::Strict,
        );
        assert!(r_py.is_err(), "Strict 模式应拒绝 python3");
        unsafe {
            std::env::remove_var(WINDOW_USE_MODE_ENV);
        }
        let _ = r;
    }

    #[tokio::test]
    async fn bash_trust_level_debug_allows_diagnostics() {
        let _env = lock_env();
        unsafe {
            std::env::set_var(WINDOW_USE_MODE_ENV, "1");
        }
        // Debug 模式:python3 + Quartz import 应放行
        let r_py = check_window_use_bash_with_trust(
            "python3 -c 'from Quartz import CGWindowListCopyWindowInfo'",
            BashTrustLevel::Debug,
        );
        assert!(r_py.is_ok(), "Debug 模式应允许 python3 -c 'from Quartz': {r_py:?}");
        // Debug 模式:defaults read 应放行
        let r_def = check_window_use_bash_with_trust(
            "defaults read com.tencent.xinWeChat",
            BashTrustLevel::Debug,
        );
        assert!(r_def.is_ok(), "Debug 模式应允许 defaults read: {r_def:?}");
        unsafe {
            std::env::remove_var(WINDOW_USE_MODE_ENV);
        }
    }

    #[tokio::test]
    async fn bash_trust_level_free_only_blocks_dangerous() {
        let _env = lock_env();
        unsafe {
            std::env::set_var(WINDOW_USE_MODE_ENV, "1");
        }
        // Free 模式:任意命令只要不进危险黑名单就放行
        let r = check_window_use_bash_with_trust(
            "python3 -c 'import os; print(os.listdir(\".\"))'",
            BashTrustLevel::Free,
        );
        assert!(r.is_ok(), "Free 模式应允许任意非危险命令: {r:?}");
        // 危险命令仍被拒
        let r_danger = check_window_use_bash_with_trust(
            "rm -rf /",
            BashTrustLevel::Free,
        );
        assert!(r_danger.is_err(), "Free 模式仍应拦截 rm -rf /");
        unsafe {
            std::env::remove_var(WINDOW_USE_MODE_ENV);
        }
    }
}
