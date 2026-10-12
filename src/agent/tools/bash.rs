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
//!
//! 2026-09-22 第 112 轮(D17 大对象溢出):大输出自动落盘
//! `BashSpill/`(`bash_spill::maybe_spill`),返回头部+尾部+路径,
//! LLM 可用 Read 工具回读完整内容。落盘失败时降级到阈值截断。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::OnceCell;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::agent::permissions;
use crate::agent::tools::bash_spill::{self, BashStream};
use crate::agent::tools::Tool;
use crate::error::{AgentError, Result};

/// 默认超时与上限
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
// 注:原 MAX_OUTPUT_CHARS 阈值由 bash_spill::DEFAULT_THRESHOLD 接管(D17 大对象溢出)。
// 本常量保留仅为保留历史兼容位置;不再被本文件引用。
#[allow(dead_code)]
const MAX_OUTPUT_CHARS: usize = 30_000;
/// SIGTERM → SIGKILL 升级等待时间,给进程清理时间(写文件 / flush buffer)
const KILL_GRACE_MS: u64 = 5_000;
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
#[allow(dead_code)] // 调用点在上方 #[cfg(windows)] 探测块内;非 Windows 无消费方,
                    // 保留存根以对称表达「Unix 上 bash 探测恒真」语义
fn is_real_bash(_path: &str) -> bool {
    true
}

/// 2026-10-12 实机事故:从已解析出的 bash 路径反推 Git for Windows 的工具目录。
///
/// **背景**:本机 Git 装在 `D:\Program Files\Git`,而 Windows 用户 PATH 里只有
/// `Git\cmd`,没有 `Git\usr\bin`。`resolve_bash_binary()` 能正确解析到
/// `…\Git\usr\bin\bash.exe`,但 Git Bash **非交互**(`bash -c`)启动时不读
/// `/etc/profile`,不会把自身 `usr/bin` 补进 PATH —— 它只认继承来的那份。
/// 于是 `ls` / `head` / `cat` / `wc` 等 coreutils 全部 `command not found`
/// (退出码 127),而 Bash 工具当时**照常判成功**,Yolo 把「命令全失败 + stdout 为空」
/// 误读成「目录是空的」,白白多烧 2 轮 LLM 才改用 Glob 纠正。
///
/// **修复**:由 bash 二进制所在位置反推安装根目录,产出工具目录清单,
/// 由 [`prepend_bash_tool_dirs`] 前置进子进程 PATH(与交互式 Git Bash 行为一致)。
///
/// 返回 `(root/usr/bin, root/mingw64/bin, root/cmd)` 中**存在**的那些。
/// 不做存在性探测以外的过滤 —— 调用方 [`prepend_bash_tool_dirs`] 再统一过滤一次,
/// 保证纯函数可测(不依赖真实文件系统)。
#[cfg(windows)]
fn git_tool_dirs_from_bash(bash_bin: &str) -> Vec<PathBuf> {
    let p = PathBuf::from(bash_bin);
    let Some(bin_dir) = p.parent() else {
        return Vec::new();
    };
    let root = match git_root_from_bin_dir(bin_dir) {
        Some(r) => r,
        None => return Vec::new(),
    };
    ["usr", "bin"]
        .into_iter()
        .map(|a| {
            let mut d = root.clone();
            d.push(a);
            d.push("bin");
            d
        })
        .chain({
            let mut d = root.clone();
            d.push("mingw64");
            d.push("bin");
            std::iter::once(d)
        })
        .chain({
            let mut d = root.clone();
            d.push("cmd");
            std::iter::once(d)
        })
        .collect()
}

/// 由 bash 所在的 `bin` 目录反推 Git 安装根目录。
///
/// - `<root>/usr/bin/bash.exe` → `<root>`
/// - `<root>/bin/bash.exe`（部分分发/便携版）→ `<root>`
///
/// 只按目录名组件判定，不做路径字符串拼接 —— 避免混用 `/` 与 `\`
/// 拼出 `<root>/usr/usr/bin` 这类垃圾路径（首版实现的 bug，由单测锁死）。
#[cfg(windows)]
fn git_root_from_bin_dir(bin_dir: &std::path::Path) -> Option<PathBuf> {
    let is_bin = bin_dir.file_name().and_then(|s| s.to_str()) == Some("bin");
    if !is_bin {
        return None;
    }
    let parent = bin_dir.parent()?;
    match parent.file_name().and_then(|s| s.to_str()) {
        // <root>/usr/bin → <root>
        Some("usr") => parent.parent().map(|p| p.to_path_buf()),
        // <root>/bin → <root>
        _ => Some(parent.to_path_buf()),
    }
}

/// 把 [`git_tool_dirs_from_bash`] 产出的目录中**真实存在**的前置到 `PATH`。
///
/// - 只加存在的目录,不存在的一律跳过(便携版/Git LFS 缺 `mingw64\bin` 是常态);
/// - 前置而非追加:Git 自带工具优先于系统同名程序,与交互式 Git Bash 一致;
/// - 原 PATH 条目**全部保留**(Windows 分号 / Unix 冒号),不覆盖用户环境。
#[cfg(windows)]
fn prepend_bash_tool_dirs(bash_bin: &str, path_env: Option<&str>) -> Option<std::ffi::OsString> {
    let dirs: Vec<PathBuf> = git_tool_dirs_from_bash(bash_bin)
        .into_iter()
        .filter(|d| d.is_dir())
        .collect();
    if dirs.is_empty() {
        return None;
    }

    let mut parts: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
    if let Some(p) = path_env {
        for seg in p.split(';') {
            let seg = seg.trim();
            if !seg.is_empty() {
                parts.push(seg.to_string());
            }
        }
    }
    std::env::join_paths(parts).ok()
}

/// 2026-09-23 Round 124:BashTool 工作模式。
///
/// - `ReadWrite`(默认):维持现有行为,SubAgent / Yolo / SubAgent-Work 等
///   持有 `BashTool::new()` 的执行层沿用。
/// - `ReadOnly`:Main-Work 编排层专用 —— 禁止 `>` / `>>` / `tee` / `sed -i` /
///   `mv` / `rm` 等写命令,只放行 `ls` / `cat` / `grep` / `git log` / `curl -sI`
///   等只读侦察命令。配合 `permissions::check_bash_readonly` 强制拦截。
///
/// 设计参考:claudecode `isReadOnly(input)`(claudecode.md:332, 1302-1305);
/// atomcode `read_only_hint()`(atomcode.md:725-735)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashMode {
    /// 默认读写模式(危险命令 + 敏感路径仍由 `permissions::check_bash_command` 拦截)。
    ReadWrite,
    /// Main-Work 编排层专用只读模式(在 ReadWrite 拦截之上再加正向白名单)。
    ReadOnly,
}

/// Bash 工具(单例逻辑;`mode` 字段决定是否进入 readonly 拦截)。
///
/// **变更历史**:
/// - Round 122(2026-09-23):`pub struct BashTool;` 单例无字段,LLM 完全靠提示词自我约束。
/// - Round 124(2026-09-23):加 `BashMode` 字段 + `readonly()` 构造器,代码层强制
///   Main-Work 不落源代码(LLM 误用 `echo x > f` / `tee` / `sed -i` 等也会被 PermissionDenied)。
pub struct BashTool {
    mode: BashMode,
}

impl BashTool {
    /// 默认 ReadWrite 模式(SubAgent / Yolo / SubAgent-Work 等执行层沿用)。
    pub const fn new() -> Self {
        Self { mode: BashMode::ReadWrite }
    }

    /// ReadOnly 模式(Main-Work 编排层专用)。
    pub const fn readonly() -> Self {
        Self { mode: BashMode::ReadOnly }
    }

    /// 取当前工作模式。
    pub const fn mode(&self) -> BashMode {
        self.mode
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        // Round 124:ReadOnly 模式的 description 显式标注 readonly 红字,
        // 配合 tools_hint 让 LLM 一开始就知道不能用 Bash 写盘。
        if self.mode == BashMode::ReadOnly {
            "【Round 124 readonly】只读侦察 Bash 命令(代码层强制只读)。\n\
             在工作目录下执行 bash 命令,返回 stdout + stderr + 退出码。\n\
             - 放行:ls / cat / grep / git log / git diff / git status / curl -sI / find 等\n\
             - 拦截:`>` / `>>` / `tee` / `sed -i` / `awk -i` / `perl -i` / `mv` / `rm` /\n\
               `cp` / `chmod` / `chown` / `touch` / `mkdir` / `ln` / `dd` / `kill -9` /\n\
               `apt install` / `pip install` / `cargo install` / `brew install` 等\n\
             - 旁路:设置 `LAEW_BASH_READONLY=off` 可临时关闭只读拦截(诊断用)\n\
             \n\
             超时默认 120000ms,最大 600000ms。**禁止写源代码 / 写临时文件**,\n\
             有写需求时通过 SubAgent 委派。\n\n\
             【大输出落盘(D17)】stdout/stderr 超 30000 字符自动落盘 BashSpill/bash_*.log,\n\
             返回头部 5K + 完整路径 + 尾部 5K,可用 Read 回读。\n\n\
             【安全提示】危险命令(rm -rf /、dd 写磁盘、sudo、curl|bash 等)\n\
             和敏感路径(~/.ssh、~/.aws、.env.production 等)始终被拦截。"
        } else {
            "在工作目录下执行 bash 命令,返回 stdout + stderr + 退出码。\n\
             - 超时单位毫秒,默认 120000,最大 600000。\n\
             - 一次调用执行一条命令;多条命令请用 && / ; 连接。\n\
             - 避免使用 cat/head/tail/sed/awk/echo 这类专用命令 — 请改用 Read / Write 等专用工具。\n\
             \n\
             【大输出落盘(D17)】当 stdout 或 stderr 超过 30000 字符时,完整内容会\n\
             自动写入根目录 BashSpill/bash_*.log,工具返回头部 5K + 完整路径 + 尾部 5K。\n\
             LLM 可用 Read 工具打开该路径取回完整输出。\n\
             \n\
             【安全提示】以下命令会被自动拦截:\n\
             - 危险命令:rm -rf /、rm -rf ~、dd 写磁盘、mkfs/fdisk、chmod 777、shutdown/reboot、sudo、curl|bash 等\n\
             - 敏感路径:~/.ssh、~/.aws、~/.gnupg、.env.production、/proc/self/environ 等\n\
             如需执行上述操作,请直接在终端运行(laew 之外)。"
        }
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

        // Round 124:Main-Work Bash readonly 模式拦截(在 ReadWrite 危险/敏感路径
        // 拦截之前;readonly 是更严的子集)。
        if self.mode == BashMode::ReadOnly {
            permissions::check_bash_readonly(&command)?;
        }

        // P0:危险命令 + 敏感路径拦截(fail-closed)
        permissions::check_bash_command(&command)?;

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
        // 2026-10-12:Windows 下修正子进程 PATH —— Git Bash 非交互启动不读
        // /etc/profile,只认继承值;而很多机器的 Windows PATH 只有 `Git\cmd`
        // 没有 `Git\usr\bin`,导致 ls/head/cat 全部 command not found(127)。
        // 由 bash 路径反推工具目录并前置,详见 git_tool_dirs_from_bash 文档。
        #[cfg(windows)]
        if let Some(p) = prepend_bash_tool_dirs(&bash_bin, std::env::var("PATH").ok().as_deref())
        {
            cmd.env("PATH", p);
        }
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

                let root_dir = bash_spill::spill_root_dir();
                let stdout_outcome = bash_spill::maybe_spill(BashStream::Stdout, &stdout, &root_dir);
                let stderr_outcome = bash_spill::maybe_spill(BashStream::Stderr, &stderr, &root_dir);

                let stdout_block = bash_spill::format_stream_block(BashStream::Stdout, &stdout_outcome);
                let stderr_block = bash_spill::format_stream_block(BashStream::Stderr, &stderr_outcome);

                let mut buf = String::new();
                if !stdout_block.is_empty() {
                    buf.push_str(&stdout_block);
                    buf.push('\n');
                }
                if !stderr_block.is_empty() {
                    buf.push_str(&stderr_block);
                    buf.push('\n');
                }
                if buf.is_empty() {
                    buf.push_str("<stdout>\n(无输出)\n");
                }
                buf.push_str(&format!("\n<exit_code>{code}</exit_code>"));

                // 2026-10-12:命令压根没跑起来(126/127)必须判is_error。
                //
                // 背景:此前只要子进程 spawn 成功就一律 Ok,exit_code 只拼进文本
                // 供展示,完全不影响 is_error —— 127 与 0 在下游被同等对待。
                // 后果是下游 Agent 把「命令全部失败 + stdout 为空」误读为
                // 「目录是空的」这一**错误事实**(实机事故:Yolo 首轮 ls 127,
                // 误判 AI-Skill 目录为空,多烧 2 轮才改用 Glob 纠正)。
                //
                // 只认 126/127 而非全部非零:grep 无匹配的 1、[ -f x ] 为假的 1
                // 是 shell 脚本的**正常语义**,全量判错会造成大量误伤与重试风暴。
                if matches!(code, 126 | 127) {
                    return Err(AgentError::ToolExecution {
                        tool: self.name().into(),
                        reason: format!(
                            "[Bash] 命令不存在或不可执行(退出码 {code})。\
                             这不是「没有输出」,而是**命令压根没跑起来** —— \
                             禁止把空 stdout 当成「目录为空/查询无结果」的证据。\
                             若 stderr 出现 `command not found`,多半是本机 shell 的 PATH \
                             缺少 coreutils(如 Git Bash 非交互启动不读 /etc/profile)。\
                             请改用不依赖该命令的方案(Read/Glob/Grep 工具),不要重复调用同一命令。\
                             stderr 原文: {}",
                            if stderr.trim().is_empty() {
                                "(空)"
                            } else {
                                stderr.trim()
                            }
                        ),
                    });
                }

                // 其余非零退出码维持 Ok(语义性),但补一行显式失败标注,
                // 杜绝「空 stdout 被误读为空目录」。
                if code != 0 {
                    buf.push_str(&format!(
                        "\n<status>命令以非零退出码 {code} 结束 —— 这是**失败**结果,\
                         上面若 stdout 为空,不得据此推断「目录为空 / 查询无结果」。</status>"
                    ));
                }
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
        let out = BashTool::new()
            .execute(json!({"command": "echo hello"}))
            .await
            .unwrap();
        assert!(out.contains("hello"));
        assert!(out.contains("<exit_code>0</exit_code>"));
    }

    #[tokio::test]
    async fn exit_code_propagated() {
        let out = BashTool::new()
            .execute(json!({"command": "exit 7"}))
            .await
            .unwrap();
        assert!(out.contains("<exit_code>7</exit_code>"));
        // 2026-10-12:非零退出码必须带显式失败标注,杜绝「空 stdout 被误读为空目录」
        assert!(
            out.contains("<status>") && out.contains("非零退出码 7"),
            "非零退出码应附显式失败标注,实际输出: {out}"
        );
    }

    /// 2026-10-12 实机事故回归:命令不存在(127)必须判 `is_error`,
    /// 否则下游 Agent 会把「命令全失败 + stdout 为空」误读为「目录是空的」。
    #[tokio::test]
    async fn command_not_found_is_error_not_silent_success() {
        let err = BashTool::new()
            .execute(json!({"command": "definitely_not_a_real_cmd_xyz_987"}))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("命令不存在或不可执行"),
            "127 应判为工具错误并给出可行动指引,实际: {msg}"
        );
        assert!(
            msg.contains("禁止把空 stdout") && msg.contains("目录为空"),
            "错误文案须明确禁止把空 stdout 当成空目录的证据,实际: {msg}"
        );
    }

    /// 2026-10-12:语义性非零退出码(`grep` 无匹配的 1)不得判错 ——
    /// 全量判错会造成大量误伤与重试风暴。
    #[tokio::test]
    async fn semantic_nonzero_exit_is_not_error() {
        let out = BashTool::new()
            .execute(json!({"command": "grep -q __no_such_token__ /dev/null"}))
            .await
            .expect("grep 无匹配的退出码 1 属正常语义,不应判工具错误");
        assert!(out.contains("<exit_code>1</exit_code>"));
    }

    /// 2026-10-12 根因回归:受限 PATH(只有 `Git\cmd`、没有 `Git\usr\bin`)下,
    /// `ls` 必须可用 —— 这正是实机事故里 `ls: command not found`(127) 的场景。
    #[cfg(windows)]
    #[tokio::test]
    async fn coreutils_available_under_restricted_path() {
        // 用本机真实解析出的 bash(可能装在 D 盘等任意位置),反推工具目录
        let bash = resolve_bash_binary().expect("应能解析到 bash");
        let tool_dirs = git_tool_dirs_from_bash(&bash);
        let tool_paths: Vec<String> = tool_dirs.iter().map(|d| d.display().to_string()).collect();
        assert!(
            !tool_paths.is_empty(),
            "应能从 {bash} 反推出 Git 工具目录"
        );

        let mut cmd = std::process::Command::new(&bash);
        cmd.arg("-c").arg("ls --version | head -1");
        // 只给工具目录,完全不给父进程 PATH —— 模拟事故机器的受限环境
        cmd.env("PATH", std::env::join_paths(tool_paths.clone()).unwrap());
        let out = cmd.output().expect("执行 bash 失败");
        let s = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            s.to_lowercase().contains("ls"),
            "仅注入 Git 工具目录({tool_paths:?})后 ls 即可运行,实际输出: {s} / stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// 2026-10-12:从 `usr\\bin\\bash.exe` 反推 Git 安装根目录。
    #[cfg(windows)]
    #[test]
    fn git_tool_dirs_derived_from_bash_path() {
        let dirs = git_tool_dirs_from_bash(r"D:\Program Files\Git\usr\bin\bash.exe");
        let joined: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        assert!(
            joined.iter().any(|d| d == r"D:\Program Files\Git\usr\bin"),
            "应含 usr\\bin,实际: {joined:?}"
        );
        assert!(
            joined.iter().any(|d| d == r"D:\Program Files\Git\cmd"),
            "应含 cmd,实际: {joined:?}"
        );
        // `bin\\bash.exe` 形态也要能反推出根目录
        let dirs2 = git_tool_dirs_from_bash(r"D:\Program Files\Git\bin\bash.exe");
        assert!(
            dirs2.iter().any(|d| d.display().to_string() == r"D:\Program Files\Git\usr\bin"),
            "bin\\bash.exe 形态应反推出 usr\\bin,实际: {:?}",
            dirs2.iter().map(|d| d.display().to_string()).collect::<Vec<_>>()
        );
    }

    /// 2026-10-12:前置 PATH 时保留用户原有条目,不覆盖。
    #[cfg(windows)]
    #[test]
    fn prepend_bash_tool_dirs_keeps_original_path() {
        let original = r"C:\Windows\system32;D:\some\user\dir";
        let bash = resolve_bash_binary().expect("应能解析到 bash");
        let merged = prepend_bash_tool_dirs(&bash, Some(original))
            .expect("应产出合并后的 PATH");
        let s = merged.to_string_lossy().to_string();
        assert!(
            s.contains(r"C:\Windows\system32") && s.contains(r"D:\some\user\dir"),
            "原 PATH 条目必须保留,实际: {s}"
        );
    }

    #[tokio::test]
    async fn missing_command_argument_errors() {
        let err = BashTool::new().execute(json!({})).await.unwrap_err();
        match err {
            AgentError::ToolExecution { reason, .. } => assert!(reason.contains("command")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn dangerous_command_blocked() {
        // rm -rf / 应被拦截
        let err = BashTool::new()
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
        let err = BashTool::new()
            .execute(json!({"command": "sudo apt install foo"}))
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn sensitive_path_blocked() {
        let err = BashTool::new()
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
        let err = BashTool::new()
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
        let out = BashTool::new()
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
        let on = BashTool::new()
            .execute(json!({"command": "echo \"$LC_ALL/$PYTHONUTF8/$PYTHONIOENCODING\""}))
            .await
            .unwrap();
        assert!(
            on.contains("C.UTF-8/1/utf-8"),
            "子进程应注入 UTF-8 环境,实际: {on}"
        );
        std::env::remove_var("LAEW_BASH_UTF8");
        let off = BashTool::new()
            .execute(json!({"command": "echo \"[$PYTHONUTF8]\""}))
            .await
            .unwrap();
        assert!(off.contains("[]"), "默认不注入,实际: {off}");
    }

    /// 2026-09-22 第 112 轮:D17 大对象溢出。输出 > 30K 时应触发落盘,返回含
    /// `BashSpill/bash_*.stdout.log` 路径;LLM 可据此读完整内容。
    #[tokio::test]
    async fn bash_large_stdout_spills_to_disk_and_reports_path() {
        let _env = lock_env();
        // 临时把阈值降到 2K 加快测试
        let prev = std::env::var("LAEW_BASH_SPILL_THRESHOLD").ok();
        std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", "2048");
        // 生成 5K 字节的输出,超过 2K 阈值
        let out = BashTool::new()
            .execute(json!({
                "command": "python3 -c \"import sys; sys.stdout.write('A' * 5000)\"",
                "timeout_ms": 10000u64,
            }))
            .await
            .unwrap();
        match prev {
            Some(v) => std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", v),
            None => std::env::remove_var("LAEW_BASH_SPILL_THRESHOLD"),
        }
        // 应包含 spill 路径提示
        assert!(
            out.contains("BashSpill/bash_") && out.contains(".stdout.log"),
            "应包含 spill 路径,实际: {}",
            &out[..out.len().min(200)]
        );
        assert!(out.contains("完整输出"), "应说明完整内容已落盘");
        assert!(out.contains("省略"), "应给出省略字符数");
        assert!(out.contains("<exit_code>0</exit_code>"), "正常退出");
    }

    /// 2026-09-22 第 112 轮:小输出(<= 30K)不应落盘,保持原 truncate 行为。
    #[tokio::test]
    async fn bash_small_stdout_keeps_inline() {
        // 第 120 轮修存量抖动:本用例是 spill 家族里**唯一**没拿 `lock_env()` 的一个,
        // 而兄弟用例会改 `LAEW_BASH_SPILL_THRESHOLD`、其它模块的用例会改进程 cwd
        // (`GLOBAL_ENV_CWD_LOCK` 正是为此存在)。并行跑时环境变量/cwd 被并发改写,
        // 本用例会偶发失败(单跑恒过)。补锁与兄弟用例对齐。
        let _env = lock_env();
        let out = BashTool::new()
            .execute(json!({
                "command": "echo hello-spill-test",
                "timeout_ms": 5000u64,
            }))
            .await
            .unwrap();
        assert!(out.contains("hello-spill-test"));
        // 小输出应走 inline 路径,不出现 spill 提示
        assert!(!out.contains("BashSpill/bash_"), "小输出不应触发 spill");
        assert!(out.contains("<stdout>"), "应包含 <stdout> 标签");
        assert!(out.contains("<exit_code>0</exit_code>"));
    }

    /// 2026-09-22 第 112 轮:stderr 大输出也应独立 spill。
    #[tokio::test]
    async fn bash_large_stderr_spills_to_stderr_log() {
        let _env = lock_env();
        let prev = std::env::var("LAEW_BASH_SPILL_THRESHOLD").ok();
        std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", "1024");
        let out = BashTool::new()
            .execute(json!({
                "command": "python3 -c \"import sys; sys.stderr.write('B' * 3000)\"",
                "timeout_ms": 10000u64,
            }))
            .await
            .unwrap();
        match prev {
            Some(v) => std::env::set_var("LAEW_BASH_SPILL_THRESHOLD", v),
            None => std::env::remove_var("LAEW_BASH_SPILL_THRESHOLD"),
        }
        assert!(
            out.contains(".stderr.log"),
            "stderr 大输出应落盘到 stderr.log,实际: {}",
            &out[..out.len().min(300)]
        );
        assert!(out.contains("<stderr>"), "应包含 <stderr> 标签");
    }
}
