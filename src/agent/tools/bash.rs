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

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
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

        let mut cmd = Command::new("bash");
        cmd.arg("-lc").arg(&command);
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
