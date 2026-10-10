//! 终端守卫(第 136 轮):根治「终端消失后 laew 空转不退 / kill 不掉」。
//!
//! ## 实测事故(macOS 活动监视器)
//!
//! ```text
//! 14 个残留 laew 进程,其中 11 个是 ./target/debug/laew --debug,
//! 每个 CPU 时间 639~655 分钟、%CPU 60~70%(合计吃掉约 9 个核心)。
//! ```
//!
//! `sample` 抓到的栈,主线程 1474 个采样点里 **1437 个**在这里:
//!
//! ```text
//! main → tui::run_with_debug → InputHandler::read_line → read_line_inner(loop)
//!      → crossterm::event::read → InternalEventReader::poll
//!      → UnixInternalEventSource::try_read → FileDesc::read → read()
//! ```
//!
//! ## 根因(两层,缺一不可)
//!
//! **第一层 · crossterm 0.27 在「fd 可读却读不到字节」时内部死转。**
//! 已用 pty 实验精确复现:当 pty **master 关闭**(终端窗口/标签被关掉、SSH 断线、
//! 终端 App 被 kill),slave 上的 `read()` **立即返回 0 字节(EOF)并永远如此**。
//! crossterm 把「读到 0 字节」当作「暂无事件」在 `try_read` 里无限重试 —— 既不
//! 返回事件、也不返回错误,外部完全无法察觉,表现为 100% CPU 空转。
//!
//! **第二层 · SIGHUP / SIGTERM 被 handler 吞掉,而主线程回不到检查点。**
//! `tui/mod.rs` 的主循环是「先查 `shutdown_sig.is_triggered()`,再 `read_line()`」。
//! 一旦进入 `read_line()` 就**永远回不到那个检查点**。而 `shutdown.rs` 又为
//! SIGHUP / SIGTERM 注册了 tokio handler —— 注册 handler 的副作用是**内核不再按
//! 默认动作终止进程**,handler 只是把标志位置起来。于是:
//!
//! - 终端关闭 → 内核送 SIGHUP → handler 置标志 → 主线程在 crossterm 里死转 → 永不退出;
//! - 用户 `kill <pid>` → 内核送 SIGTERM → 同上 → **`kill` 看起来完全没反应**(只有
//!   `kill -9` 能杀)。
//!
//! `tui/mod.rs` 里那句注释「即使 read_line 在阻塞等用户输入,也能立即 break」在
//! 这里**是错的**:检查点在阻塞之前,crossterm 内部空转时永远轮不到。
//!
//! ## 方案
//!
//! 1. **本模块的看门狗线程**(主修复):独立 `std::thread`,不依赖 tokio 调度器
//!    是否还能推进。轮询 fd 0 的 `POLLHUP` —— 实测 master 关闭后 `poll()` 立刻
//!    返回 `POLLHUP`(0.000s),且**只订阅 `POLLHUP` 就足够**(POLLIN/POLLERR/
//!    POLLNVAL 无条件上报,不需要放进 mask);正常交互时 `poll` 会阻塞满整个超时。
//!    命中即清理并退出。
//! 2. **shutdown 宽限兜底**:信号已触发但进程过了宽限期仍在 → 强制退出,让
//!    `kill` 真正有效(解决第二层的死结)。
//! 3. **输入循环纵深防御**:`read_line_inner` 在进入 `event::read()` **之前**先用
//!    [`stdin_hangup`] 做一次 0 超时探测,终端已消失就直接返回,而不是进死转。
//!
//! ## 平台差异
//!
//! - **macOS / Linux**:`POLLHUP` 语义完整,本模块全量生效。
//! - **Windows**:没有 pty,`POLLHUP` 无意义(控制台关闭由系统发
//!   `CTRL_CLOSE_EVENT` 并默认终止进程),[`stdin_hangup`] 恒返回 `false`,
//!   只有第 2 条宽限兜底仍然生效。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 看门狗轮询间隔。1s 足够快(终端关闭后 1s 内退出),也足够省(一次 poll 系统调用)。
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(1);

/// 信号已触发后的强制退出宽限期。
///
/// 实测正常 Ctrl-C 取消到进程退出约 2s(`testReport/run_e2e.sh` 第 10 节:
/// 「总耗时 2s < 6s」),10s 是它的 5 倍余量,既能容忍慢速任务收尾,
/// 又能让 `kill` 在可感知的短时间内真正生效。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// 保险丝:守卫线程开始收口后,再过这么久仍没退出就无条件终止进程。
///
/// 收口步骤(写 ANSI / 浏览器 kill)理论上都有界,但主线程卡死时可能持有
/// `atexit` 守卫或浏览器清理所需的锁。没有这根保险丝,守卫线程自己也会
/// 变成第二个杀不掉的进程(实测就卡在 stdio 全局互斥锁上)。
const HARD_EXIT_DEADLINE: Duration = Duration::from_secs(3);

/// 单例句柄,便于测试断言「只装一次」。
static INSTALLED: OnceLock<()> = OnceLock::new();

// =================== 挂断探测(纯函数,可单测) ===================

/// 探测 `fd` 是否已挂断(终端消失 / 对端关闭)。
///
/// 只订阅 `POLLHUP`:POSIX 规定 `POLLERR` / `POLLHUP` / `POLLNVAL` **无条件上报**,
/// 不需要在 `events` 里声明,而不带 `POLLIN` 可以保证「有按键待读」不会误判为挂断。
///
/// 返回 `false` 的场景(全部是**不该退出**的):
/// - `fd` 不是终端(管道 / 文件重定向)→ 不是 TTY 就直接放行;
/// - 终端正常存活 → `poll` 阻塞满 `timeout_ms`,返回 0 个事件;
/// - 调用方拿不到 fd(理论上不会发生)。
#[cfg(unix)]
pub fn fd_hangup(fd: std::os::raw::c_int, timeout_ms: i32) -> bool {
    // SAFETY:pfd 是栈上的有效 pollfd;nfds=1;poll 只读 fd,不消费任何字节,
    // 与 crossterm 的 mio 事件源并发使用安全(内核侧同一 fd 的就绪队列)。
    unsafe {
        let mut pfd = libc::pollfd {
            fd,
            events: 0, // 只要 HUP/ERR/NVAL,不要 POLLIN
            revents: 0,
        };
        let n = libc::poll(&mut pfd, 1, timeout_ms);
        if n <= 0 {
            // 0 = 超时无事件(终端健康);-1 = 被信号打断(EINTR),按未挂断处理,
            // 下一轮继续探测 —— 宁可多转一圈也不能误杀。
            return false;
        }
        pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
    }
}

/// 非 Unix 平台无 POLLHUP 语义,恒不判定挂断(见模块文档「平台差异」)。
#[cfg(not(unix))]
pub fn fd_hangup(_fd: std::os::raw::c_int, _timeout_ms: i32) -> bool {
    false
}

/// 探测 **stdin** 是否已挂断(0 超时,纯查询不阻塞)。
///
/// 输入循环每次进 `event::read()` 前调一次:终端已消失时直接返回,
/// 避免进入 crossterm 0.27 的内部死转。
pub fn stdin_hangup() -> bool {
    if !stdin_is_tty() {
        return false;
    }
    #[cfg(unix)]
    {
        fd_hangup(0, 0)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// stdin 是否是终端(非 TTY 一律不做挂断判定)。
pub fn stdin_is_tty() -> bool {
    #[cfg(unix)]
    {
        // SAFETY:isatty 只做 ioctl 查询,不读写任何数据。
        unsafe { libc::isatty(0) == 1 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

// =================== 看门狗 ===================

/// 安装终端守卫看门狗(幂等,可重复调用只生效一次)。
///
/// 必须在 Tokio runtime 内调用(需要 [`tokio::runtime::Handle`] 回收浏览器子进程)。
/// 非 TTY(管道 / CI / e2e)直接跳过 —— 那种场景没有「终端被关掉」可言,
/// 且挂断判定对普通文件描述符没有意义。
pub fn install_watchdog(rt: tokio::runtime::Handle) {
    if !stdin_is_tty() {
        return;
    }
    if INSTALLED.set(()).is_err() {
        return; // 已装过
    }
    // 独立 std::thread:tokio worker 可能被长任务占满,而这个守卫必须在
    // 「主线程已经卡死」的场景下依然能跑。
    std::thread::Builder::new()
        .name("laew-term-guard".into())
        .spawn(move || watch_loop(rt))
        .ok();
}

/// 看门狗主循环(独立线程)。
fn watch_loop(rt: tokio::runtime::Handle) {
    // 记录 shutdown 首次被观察到的时刻,用于宽限兜底。
    let mut shutdown_seen_at: Option<Instant> = None;

    loop {
        std::thread::sleep(WATCHDOG_INTERVAL);

        // ① 终端消失 —— 立即清理退出。用户已经不在了,继续跑只会空转并泄漏
        //    浏览器子进程;所有输出都写向一个已经不存在的终端,没有保留价值。
        if stdin_hangup() {
            finish(rt, "终端已断开(pty master 关闭),守卫线程回收进程", 0);
            return;
        }

        // ② shutdown 已触发但进程仍存活 —— 宽限期满则强制退出。
        //    覆盖 SIGTERM / SIGHUP 被 handler 吞掉、主线程卡在 crossterm 死转
        //    导致 `kill` 无效的场景(见模块文档第二层根因)。
        let sig = crate::shutdown::global();
        if sig.is_triggered() {
            let since = *shutdown_seen_at.get_or_insert_with(Instant::now);
            if since.elapsed() >= SHUTDOWN_GRACE {
                finish(
                    rt,
                    &format!(
                        "shutdown 已触发({:?})超过 {}s 进程仍存活,守卫线程强制退出",
                        sig.current_reason(),
                        SHUTDOWN_GRACE.as_secs()
                    ),
                    // SIGTERM / SIGHUP 语义上等价于「被外部终止」,用 128+signal 惯例
                    // 而不是 0,免得调用方把强杀误读成正常成功退出。
                    match sig.current_reason() {
                        Some(crate::shutdown::ShutdownReason::UserInterrupt) => 130,
                        _ => 143,
                    },
                );
                return;
            }
        } else {
            shutdown_seen_at = None;
        }
    }
}

/// 守卫线程的收口:还原终端 → 回收浏览器 → 退出进程。
///
/// **刻意用 `std::process::exit` 而不是 `libc::_exit`**:前者会跑 `atexit` 守卫
/// (`main.rs::install_browser_cleanup_guard` 注册的 `BrowserManager::cleanup_sync`),
/// 后者不会 —— 用 `_exit` 会把 Chrome 子进程留在系统里,正是要根治的泄漏之一。
fn finish(rt: tokio::runtime::Handle, reason: &str, code: i32) {
    tracing::warn!("[终端守卫] {reason}");

    // 0) 保险丝:再起一个线程,3s 后无条件退出。
    //    本函数后面每一步都可能因为「主线程正卡在某个全局锁上」而阻塞
    //    (实测就卡在 stderr 的 stdio 互斥锁上,见 force_terminal_restore 注释),
    //    没有这根保险丝,守卫线程自己也会变成第二个杀不掉的进程。
    std::thread::Builder::new()
        .name("laew-term-guard-exit".into())
        .spawn(move || {
            std::thread::sleep(HARD_EXIT_DEADLINE);
            std::process::exit(code);
        })
        .ok();

    // 1) 还原终端。**刻意不用 `shutdown::terminal_restore_sync()`** —— 它走
    //    `std::io::stdout()` / `stderr()`,会 acquiring Rust stdio 的**全局互斥锁**;
    //    主线程此刻正卡死、完全不会释放这把锁,守卫线程会一起永久阻塞
    //    (实测 `sample` 抓到 `finish → terminal_restore_sync → Stderr::lock →
    //    _pthread_mutex_firstfit_lock_slow` 无限等待)。这里改用 `libc::write(2)`
    //    直接写 fd 1/2,不碰任何锁。
    force_terminal_restore();

    // 2) 回收浏览器子进程 —— 两条腿,互为兜底:
    //    a) 派发异步 `shutdown()`(带 5s CDP 超时,走优雅关闭);
    //    b) 同步 `cleanup_sync()`(进程级 kill,不依赖 tokio 调度器能否推进)。
    //    主线程此刻正卡在 crossterm 死转里,异步那条不保证能跑完,所以 b 是关键。
    rt.spawn(async {
        crate::agent::browser::BrowserManager::global()
            .shutdown()
            .await;
    });
    crate::agent::browser::BrowserManager::cleanup_sync();

    // 3) 退出。用 `process::exit` 而非 `libc::_exit`:前者会跑 atexit 守卫
    //    (main.rs 注册的 cleanup_sync),后者不会 —— 用 `_exit` 会把 Chrome 留在系统里。
    std::process::exit(code);
}

/// 无锁还原终端:直接 `write(2)` 到 fd 1 / fd 2 + ioctl 关 raw mode。
///
/// 序列与 [`crate::shutdown::terminal_restore_sync`] 完全一致
/// (`?25h` 光标 / `?1049l` 退 alt screen / `r` 重置滚动区 / `?7h` 自动换行 /
/// `m` 重置 SGR),区别只在**不经过 Rust stdio 的全局锁**。
/// 第 149 轮:与 terminal_restore_sync 同步删除 `\x1bc`(RIS)—— 它会清空
/// 用户屏幕(「终端像崩溃重启」观感的根因),且 shutdown 卡死场景下用户终端
/// 仍存活,抹掉它同样有害;laew 用过的全部模式已由上述序列逐一还原。
///
/// # 为什么必须绕开 stdio
///
/// 这是本轮实测踩到的最隐蔽的一处:守卫线程已经正确判定「该退出了」,却死在
/// `terminal_restore_sync()` 的 `Stderr::lock()` 上 —— 主线程卡死时持有 stdio
/// 全局互斥锁且永不释放,`process::exit` 根本执行不到,于是「修复进程不退出」
/// 的代码自己变成了又一个杀不掉的进程。
#[cfg(unix)]
fn force_terminal_restore() {
    const SEQ: &[&[u8]] = &[
        b"\x1b[?25h",     // 显示光标
        b"\x1b[?1049l",   // 退出 alt screen
        b"\x1b[r",        // 重置滚动区(全屏)
        b"\x1b[?7h",      // 启用自动换行
        b"\x1b[m",        // 重置 SGR 属性
    ];
    for seq in SEQ {
        for fd in [1i32, 2i32] {
            // SAFETY:write(2) 到 fd 1/2,buf 是 'static 字节串,len 与之匹配。
            // 终端已消失时返回 -1/EBADF 属预期,忽略即可 —— 进程本来就要退出。
            unsafe {
                libc::write(fd, seq.as_ptr() as *const libc::c_void, seq.len());
            }
        }
    }
    // disable_raw_mode 走 tty fd 的 ioctl,不碰 stdio 锁。
    let _ = crossterm::terminal::disable_raw_mode();
}

/// 非 Unix 平台没有 `libc::write` 这条无锁直写路径(该路径专为 unix 的
/// 「主线程卡死时 stdio 互斥锁永不释放」而设);Windows 上退回 crossterm 的
/// 同步还原(此时主线程未必卡死,拿得到 stdio 锁)。
#[cfg(not(unix))]
fn force_terminal_restore() {
    let _ = crossterm::terminal::disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    // 以下两个用例直接依赖 `std::os::unix::io::AsRawFd`,unix 专属。
    #[cfg(unix)]
    #[test]
    fn stdin_hangup_on_regular_fd_is_false() {
        // 非 TTY 的 fd(普通文件)永远不该被判为挂断,否则管道/e2e 会被误杀
        let f = std::fs::File::create("/tmp/laew_term_guard_probe.txt").unwrap();
        use std::os::unix::io::AsRawFd;
        assert!(!fd_hangup(f.as_raw_fd(), 0));
        let _ = std::fs::remove_file("/tmp/laew_term_guard_probe.txt");
    }

    #[cfg(unix)]
    #[test]
    fn fd_hangup_on_live_pipe_is_false() {
        // 管道两端都在,不应判挂断
        let (a, _b) = std::io::pipe().unwrap();
        use std::os::unix::io::AsRawFd;
        assert!(!fd_hangup(a.as_raw_fd(), 0));
    }

    #[test]
    fn stdin_is_tty_matches_crossterm_view() {
        // 与 crossterm 的判定保持一致,避免两套口径打架
        assert_eq!(stdin_is_tty(), crossterm::tty::IsTty::is_tty(&std::io::stdin()));
    }

    #[test]
    fn shutdown_grace_is_longer_than_observed_cancel_latency() {
        // 实测 Ctrl-C 取消到退出约 2s(run_e2e.sh 第 10 节),宽限期必须显著大于它,
        // 否则会截断正常任务的收口
        assert!(
            SHUTDOWN_GRACE > Duration::from_secs(2) * 2,
            "宽限期过短会截断正常收口"
        );
    }

    #[test]
    fn watchdog_install_is_idempotent() {
        // install 依赖 tokio Handle,单测里只在非 TTY 场景验证「不 panic 且幂等」
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        if stdin_is_tty() {
            return; // CI 通常非 TTY;真 TTY 下由 e2e 覆盖
        }
        install_watchdog(rt.handle().clone());
        install_watchdog(rt.handle().clone());
        assert!(INSTALLED.get().is_none(), "非 TTY 不应安装看门狗");
    }
    /// 回归:守卫线程的收口**绝不能**阻塞在 stdio 全局互斥锁上。
    ///
    /// 本轮实测踩到:守卫已正确判定「该退出了」,却死在
    /// `shutdown::terminal_restore_sync()` → `Stderr::lock()` 上 —— 主线程卡死时
    /// 持有 stdio 锁且永不释放,`process::exit` 根本执行不到,于是「修复进程不退出」
    /// 的代码自己变成了第二个杀不掉的进程。
    ///
    /// 用例:让另一个线程霸占 stdout 锁 3 秒,要求 `force_terminal_restore()`
    /// 在 1 秒内返回 —— 它走 `libc::write(2)` 而不是 `std::io::stdout()`,不受影响。
    #[test]
    fn force_terminal_restore_does_not_block_on_stdio_lock() {
        let holder = std::thread::spawn(|| {
            let _lock = std::io::stdout().lock();
            std::thread::sleep(Duration::from_secs(3));
        });
        // 等 holder 真正拿到锁再开测
        std::thread::sleep(Duration::from_millis(300));

        let t0 = Instant::now();
        force_terminal_restore();
        let elapsed = t0.elapsed();

        assert!(
            elapsed < Duration::from_secs(1),
            "还原终端被 stdio 全局锁阻塞了 {elapsed:?},会连带守卫线程一起卡死"
        );
        drop(holder);
    }

    /// 看门狗的两条退出判据都不应因「终端健康」而误触发。
    ///
    /// `stdin_hangup()` 在健康终端上恒为 false(实测 `poll` 会阻塞满超时),
    /// 这条钉死「不误杀」;SIGTERM 宽限兜底需要真实信号,不在单测覆盖范围。
    #[test]
    fn healthy_terminal_is_never_flagged_as_hangup() {
        if !stdin_is_tty() {
            return; // 非 TTY 场景 fd_hangup 恒 false,已在其它用例覆盖
        }
        for _ in 0..3 {
            assert!(!stdin_hangup(), "健康终端不得被判为挂断");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
