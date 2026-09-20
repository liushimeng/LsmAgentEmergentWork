//! MCP_Web_Use launch 浏览器的进程外生命周期守护。
//!
//! `SIGKILL` / 任务管理器强杀会让 `laew` 失去执行 Drop、atexit 和信号处理的
//! 机会，进程内 `BrowserManager::cleanup_sync()` 因此天然不完整。这里由主进程
//! 额外启动一个极小 watchdog：主进程侧的 stdin pipe 关闭即代表主进程已退出，
//! watchdog 随后强制回收 launch 模式拥有的 Chromium 主进程与一次性 profile。
//!
//! 通过 `connect_url` 接管的外部浏览器不是 laew 所有，不启动本守护。

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncReadExt;

/// 隐藏子命令名（clap 中继续 hide）。
pub const WATCHDOG_COMMAND: &str = "__browser-watchdog";

/// 运行 watchdog，直到主进程 pipe EOF 或 browser 主进程退出。
pub async fn run(browser_pid: u32, user_data_dir: &Path) -> anyhow::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut buf = [0_u8; 64];
    loop {
        tokio::select! {
            read = stdin.read(&mut buf) => {
                // 主进程正常退出或 SIGKILL 都会让 pipe 读端返回 0。
                if read.is_err() || matches!(read, Ok(0)) {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                // 浏览器正常退出时 watchdog 立即退出，不长期悬挂。
                if !process_alive(browser_pid) {
                    return Ok(());
                }
            }
        }
    }

    terminate_process(browser_pid);
    remove_profile(user_data_dir);
    Ok(())
}

/// Unix/macOS 探活：kill 0 不发送信号，只检查进程存在与权限。
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // libc::pid_t 与 u32 在三大 Unix 平台均为 32 位有符号值。
    let raw = i32::try_from(pid).unwrap_or(0);
    raw != 0 && unsafe { libc::kill(raw, 0) } == 0
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut exit_code = 0_u32;
        let alive = GetExitCodeProcess(handle, &mut exit_code).is_ok() && exit_code == 259; // STILL_ACTIVE
        let _ = CloseHandle(handle);
        alive
    }
}

#[cfg(unix)]
fn terminate_process(pid: u32) {
    let Some(raw) = i32::try_from(pid).ok().filter(|v| *v > 0) else {
        return;
    };
    unsafe {
        libc::kill(raw, libc::SIGTERM);
    }
    for _ in 0..8 {
        if !process_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    unsafe {
        libc::kill(raw, libc::SIGKILL);
    }
}

#[cfg(windows)]
fn terminate_process(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
        }
    }
}

fn remove_profile(dir: &Path) {
    // Chrome 退出可能短暂持有 SingletonLock / cache fd，重试足够覆盖普通退出。
    for _ in 0..10 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) | Err(_) if !dir.exists() => return,
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_alive_accepts_current_and_rejects_zero() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
    }

    #[tokio::test]
    async fn run_returns_when_target_is_already_gone() {
        let dir = tempfile::tempdir().unwrap();
        run(0, dir.path()).await.unwrap();
    }
}
