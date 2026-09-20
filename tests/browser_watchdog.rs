//! MCP_Web_Use 进程外浏览器 watchdog 语义验证。
//!
//! 用 `sleep` 伪装 browser 主进程，验证 laew 被 SIGKILL（等价 stdin pipe 关闭）
//! 后，watchdog 会在优雅退出超时后强制回收目标进程并清理 profile。

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
#[tokio::test]
async fn watchdog_kills_target_after_parent_pipe_closes() {
    let profile = tempfile::tempdir().unwrap();
    let mut target = Command::new("sleep")
        .arg("30")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let target_pid = target.id();

    let mut watchdog = Command::new(env!("CARGO_BIN_EXE_laew"))
        .args([
            "__browser-watchdog",
            &target_pid.to_string(),
            &profile.path().display().to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // 确认 watchdog 已进入等待，再模拟父进程 SIGKILL 的关键事实：pipe 读端 EOF。
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut stdin = watchdog.stdin.take().unwrap();
    stdin.write_all(b"started\n").unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let started = Instant::now();
    loop {
        if matches!(target.try_wait(), Ok(Some(_))) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "watchdog did not kill the fake browser"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let status = watchdog.wait().unwrap();
    assert!(status.success());
    assert!(!profile.path().exists(), "watchdog left the temp profile");
    assert!(matches!(target.wait(), Ok(_)));
}
