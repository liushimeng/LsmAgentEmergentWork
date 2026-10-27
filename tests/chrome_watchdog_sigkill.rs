//! Chrome watchdog 的「父进程被 SIGKILL」契约回归。
//!
//! 背景:2026-10-23 第 178 轮实测强杀 laew 后残留孤儿 Chrome(100MB+/个)与
//! 临时 profile,且 `pgrep browser-watchdog` 为空 —— 看门狗未存活/未回收。
//! 后续轮次修复后,2026-10-27 真机复核通过(laew → Chrome → watchdog 全链,
//! SIGKILL 后 20s 内 Chrome 被回收、watchdog 退出、profile 清理干净)。
//! 本用例把该验证固化为自动化回归,覆盖三段契约:
//!
//! 1. **管道在位 + 目标存活 → watchdog 必须仍活着且不动目标**(历史失败形态
//!    就是 watchdog 没起来或提前退出);
//! 2. **stdin 管道 EOF(等价父进程被 SIGKILL,SIGKILL 会关闭进程持有的全部
//!    管道)→ SIGTERM 回收目标进程**;
//! 3. **一次性 profile 目录被清理,watchdog 自身以 0 退出**。
//!
//! 不需要真 Chrome、不需要网络:用 `sleep` 扮演「Chrome 主进程」,关掉 stdin
//! 管道扮演「父进程被 SIGKILL」。跑的是真实 `laew` 二进制的隐藏子命令
//! `__browser-watchdog`,与生产路径完全同构(spawn_watchdog 的启动参数一致)。

// Windows 侧 terminate/process_alive 由单元测试覆盖(watchdog.rs);本用例的
// 进程信号语义只在 unix 上断言,Windows 上整文件为空,保证跨平台编译。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// 复制真实 laew 二进制到隔离目录(CLI 集成测试惯例,见 `cli_provider.rs`:
/// 二进制的 SQLite 落在程序目录,复制即换库,用例间零共享)。
fn isolated_laew() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("创建隔离目录失败");
    let bin = dir.path().join("laew");
    std::fs::copy(Path::new(env!("CARGO_BIN_EXE_laew")), &bin)
        .expect("复制 laew 二进制到隔离目录失败");
    // macOS 上 cp 出来的副本没有执行位时显式补上。
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&bin).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&bin, p).unwrap();
    }
    (dir, bin)
}

/// 扮演「Chrome 主进程」的长命目标进程。
fn spawn_target() -> Child {
    Command::new("/bin/sleep")
        .arg("600")
        .spawn()
        .expect("启动目标进程(sleep)失败")
}

#[test]
fn watchdog_父管道eof后回收目标进程与profile() {
    let (_dir, bin) = isolated_laew();
    let mut target = spawn_target();
    let target_pid = target.id();

    // 一次性 profile:带一点真实形状(Default/Preferences),清理与否可判定。
    let profile = tempfile::tempdir().expect("创建 profile 目录失败");
    let default = profile.path().join("Default");
    std::fs::create_dir_all(&default).expect("创建 Default 失败");
    std::fs::write(default.join("Preferences"), b"{}").expect("写入 Preferences 失败");

    // 与生产 spawn_watchdog 同参:__browser-watchdog <browser_pid> <user_data_dir>;
    // stdin 管道写端由本测试持有 —— 持有即「父进程存活」。
    let mut watchdog = Command::new(&bin)
        .arg("__browser-watchdog")
        .arg(target_pid.to_string())
        .arg(profile.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("启动 watchdog 子进程失败");
    let stdin = watchdog.stdin.take().expect("拿不到 watchdog stdin 管道");

    // 契约 1:管道在位 + 目标存活 → watchdog 不退出、不动目标。
    // (历史失败形态:watchdog 没起来或秒退,强杀后无人回收 —— 这里必须钉死。)
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        watchdog.try_wait().unwrap().is_none(),
        "父进程管道在位且目标存活时 watchdog 不应退出(历史回归:看门狗未存活)"
    );
    assert!(
        target.try_wait().unwrap().is_none(),
        "父进程管道在位时 watchdog 不应杀目标进程(误回收)"
    );

    // 契约 2:管道 EOF == 父进程被 SIGKILL(SIGKILL 关闭全部管道)。
    drop(stdin);

    // 契约 3:EOF → SIGTERM 目标 → profile 清理 → watchdog 以 0 退出。
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        let target_gone = target.try_wait().unwrap().is_some();
        let profile_gone = !profile.path().exists();
        if let Some(st) = watchdog.try_wait().unwrap() {
            assert!(
                target_gone,
                "watchdog 已退出但目标进程(pid={target_pid})仍存活 —— 回收契约失守"
            );
            assert!(
                profile_gone,
                "watchdog 已退出但 profile 目录({})仍残留 —— 清理契约失守",
                profile.path().display()
            );
            break st;
        }
        assert!(
            Instant::now() < deadline,
            "管道 EOF 后 20s 内 watchdog 未完成回收(watchdog 未退出;目标{};profile{})",
            if target_gone { "已回收" } else { "仍存活" },
            if profile_gone { "已清理" } else { "仍残留" },
        );
        std::thread::sleep(Duration::from_millis(250));
    };
    assert!(
        status.success(),
        "watchdog 应以退出码 0 收尾,实际 {status}(code={:?})",
        status.code()
    );
}

#[test]
fn watchdog_目标进程已消失时不悬挂直接清理profile() {
    let (_dir, bin) = isolated_laew();
    let mut target = spawn_target();
    let target_pid = target.id();
    // 先让目标退出再启动 watchdog —— 命中 run() 的「目标已不在:直接收尾」分支。
    let _ = target.kill();
    let _ = target.wait();

    let profile = tempfile::tempdir().expect("创建 profile 目录失败");
    std::fs::write(profile.path().join("SingletonLock"), b"x").expect("写入标记失败");

    let out = Command::new(&bin)
        .arg("__browser-watchdog")
        .arg(target_pid.to_string())
        .arg(profile.path())
        .stdin(Stdio::null())
        .output()
        .expect("拉起 watchdog 失败");
    assert!(
        out.status.success(),
        "目标已消失时 watchdog 应立即收尾,实际 {} stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !profile.path().exists(),
        "目标已消失路径也应清理 profile,但仍残留 {}",
        profile.path().display()
    );
}
