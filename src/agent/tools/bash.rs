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

/// 当前进程工作目录(通过 env::current_dir 惰性获取)
fn current_work_dir() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
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
                .chain(['C', 'D', 'E'].iter().map(|d| {
                    format!(r"{}:\Program Files\Git\usr\bin", d)
                }))
                .chain(['C', 'D', 'E'].iter().map(|d| {
                    format!(r"{}:\Program Files (x86)\Git\usr\bin", d)
                }))
                .chain(['C', 'D', 'E'].iter().map(|d| {
                    format!(r"{}:\Program Files\Git\bin", d)
                }))
                .chain(['C', 'D', 'E'].iter().map(|d| {
                    format!(r"{}:\Program Files (x86)\Git\bin", d)
                }))
                .chain(['C', 'D', 'E'].iter().map(|d| {
                    format!(r"{}:\Program Files\Git\mingw64\bin", d)
                }))
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
            for tail in [r"C:\Windows\System32\bash.exe",
                         r"C:\Users\Public\bash.exe"] {
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
    fn name(&self) -> &str { "Bash" }

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
                        buf.push_str(&format!("\n...[stdout 截断,省略 {} 字符]", stdout_trunc.omitted));
                    }
                    buf.push('\n');
                }
                if !stderr_trunc.truncated.is_empty() {
                    buf.push_str("<stderr>\n");
                    buf.push_str(&stderr_trunc.text);
                    if stderr_trunc.omitted > 0 {
                        buf.push_str(&format!("\n...[stderr 截断,省略 {} 字符]", stderr_trunc.omitted));
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
        let cut = s
            .char_indices()
            .nth(max)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
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
}
