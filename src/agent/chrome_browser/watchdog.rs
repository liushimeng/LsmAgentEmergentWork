//! MCP_Web_Chrome_Use 的 Chrome 子进程**进程外**生命周期守护。
//!
//! `SIGKILL` / 任务管理器强杀会让 `laew` 失去执行 Drop、atexit 与信号处理的机会,
//! 进程内的 `close_all()` 因此天然不完整 —— 实测(2026-10-23 双浏览器工具测试)
//! 强杀 laew 后残留 3 个孤儿 Chrome 进程(各占 100MB+ 内存)。
//!
//! 方案(移植自历史 Chrome 时代实现,见 `docs/MCP_Web_Chrome_Use/reference实现归档/`):
//! 主进程额外启动一个极小 watchdog 子进程,主进程侧的 stdin pipe 关闭即代表主进程
//! 已退出(SIGKILL 同样会让 pipe EOF),watchdog 随后强制回收 Chrome 主进程与
//! 一次性 profile 目录。

use std::path::Path;
use std::time::Duration;

/// 隐藏子命令名(clap 中 hide)。
pub const WATCHDOG_COMMAND: &str = "__browser-watchdog";

/// 运行 watchdog,直到主进程 pipe EOF 或 Chrome 主进程退出。
/// `keep_profile=true` 时只回收进程,保留专用持久登录环境。
pub async fn run(browser_pid: u32, user_data_dir: &Path, keep_profile: bool) -> anyhow::Result<()> {
    use tokio::io::AsyncReadExt;
    // 目标进程已不在:直接收尾。**不要**进入 select —— `tokio::io::stdin()`
    // 会派发阻塞读线程,若 stdin 长期不关闭(测试 harness 的管道就是如此),
    // 后续 Runtime drop 会等待该阻塞任务,表现为看门狗测试永久挂起。
    if !process_alive(browser_pid) {
        cleanup_profile(user_data_dir, keep_profile);
        return Ok(());
    }
    let mut stdin = tokio::io::stdin();
    let mut buf = [0_u8; 64];
    loop {
        tokio::select! {
            read = stdin.read(&mut buf) => {
                // 主进程正常退出或被 SIGKILL 都会让 pipe 读端返回 0。
                if read.is_err() || matches!(read, Ok(0)) {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                // Chrome 正常退出时 watchdog 立即退出,不长期悬挂。
                // 此时主进程可能已先完成 shutdown,轮不到 EOF 分支;profile 必须
                // 在这里清理,否则 temp 下残留 laew_chrome_*(实测 2 个)。
                if !process_alive(browser_pid) {
                    cleanup_profile(user_data_dir, keep_profile);
                    return Ok(());
                }
            }
        }
    }

    terminate_process(browser_pid);
    remove_profile(user_data_dir);
    Ok(())
}

/// Unix/macOS 探活:kill 0 不发送信号,只检查进程存在与权限。
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    raw != 0 && unsafe { libc::kill(raw, 0) } == 0
}

/// Windows 探活:OpenProcess + GetExitCodeProcess(STILL_ACTIVE)。
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

/// 进程级终止(SIGTERM → 轮询 → SIGKILL / Windows TerminateProcess)。
#[cfg(unix)]
pub fn terminate_process(pid: u32) {
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

/// Windows 进程级终止。
#[cfg(windows)]
pub fn terminate_process(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
        }
    }
}

fn cleanup_profile(dir: &Path, keep: bool) {
    if !keep {
        remove_profile(dir);
    }
}

/// 删除一次性浏览器 profile 目录(带重试,覆盖 Chrome 退出延迟持有的锁)。
pub fn remove_profile(dir: &Path) {
    // Chrome 主进程退出后,helper/renderer 仍可能短暂持有 profile 内文件句柄;
    // 2.5s 窗口实测不够(残留 8~12KB 的 Default 目录),放宽到 5s。
    // 单次 remove_dir_all 成功也可能被残留 helper 重建,故要求「连续 2 次探测
    // 均不存在」(约 500ms 静默窗)才算清理完成。
    let mut clean_passes = 0;
    for _ in 0..20 {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(dir);
        }
        if dir.exists() {
            clean_passes = 0;
        } else {
            clean_passes += 1;
            if clean_passes >= 2 {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// 启动 watchdog 子进程:返回其句柄(主进程正常收口时可回收)。
///
/// `LAEW_BROWSER_WATCHDOG_EXE` 可覆盖 watchdog 可执行文件路径(集成测试场景下
/// 当前可执行文件是 test harness,不认识隐藏子命令)。
pub fn spawn_watchdog(
    browser_pid: u32,
    user_data_dir: &Path,
    keep_profile: bool,
) -> Option<std::process::Child> {
    let exe = std::env::var_os("LAEW_BROWSER_WATCHDOG_EXE")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_exe().ok())?;
    let mut command = std::process::Command::new(exe);
    command
        .arg(WATCHDOG_COMMAND)
        .arg(browser_pid.to_string())
        .arg(user_data_dir)
        .arg(if keep_profile { "--keep-profile" } else { "--remove-profile" })
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // 独立进程组:避免终端 Ctrl-C 同步终止 watchdog;父进程真正退出仍会关闭 pipe。
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_alive_接受当前进程_拒绝零() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
    }

    #[tokio::test]
    async fn run_目标已消失时立即返回() {
        let dir = tempfile::tempdir().unwrap();
        run(0, dir.path(), false).await.unwrap();
    }

    #[test]
    fn remove_profile_幂等() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("profile");
        std::fs::create_dir_all(&sub).unwrap();
        remove_profile(&sub);
        assert!(!sub.exists());
        remove_profile(&sub);
    }

    #[tokio::test]
    async fn run_持久profile只回收进程不删目录() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("persistent");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("Cookies"), b"keep").unwrap();
        run(0, &profile, true).await.unwrap();
        assert!(profile.join("Cookies").exists());
    }
}
