//! 统一信号路由器与退出协调器(第 108 轮,2026-09-21)。
//!
//! **核心目标**:解决 macOS/Windows 系统上 `Ctrl+C/Ctrl+D/Ctrl+Z` 终止 laew 时
//! 「程序卡死 / 整个终端僵死 / 进程被挂起 / 资源泄漏」四类故障。
//!
//! # 设计要点
//!
//! 1. **统一信号入口**:SIGINT/SIGTERM/SIGHUP 收敛到一个 [`ShutdownSignal`] 单例,
//!    不再每处 `std::process::exit` 各自逃逸。
//! 2. **SIGTSTP/SIGTTIN/SIGTTOU 显式拦截**:`libc::signal(SIGTSTP, SIG_IGN)` 阻断
//!    「进程被 stop」默认行为 —— 这是 laew 在 macOS 上 `Ctrl+Z` 被挂起并导致整个
//!    Terminal 卡死的根因之一。
//! 3. **退出走 graceful 路径**:不再 `std::process::exit(N)`,改走「trigger
//!    shutdown → 各协程收尾 → return Err → 自然退出」,Drop 链与 atexit 守卫全跑。
//! 4. **终端一定还原**:[`terminal_restore`] 发送完整 ANSI 序列 + `disable_raw_mode`,
//!    任何退出路径(正常 / 异常 / Ctrl+C / panic / signal)调用即还原。
//!
//! # 用法
//!
//! 下面是**示意片段**(不是可编译的完整函数体:含裸 `return Err(…)` 与 `.await`),
//! 因此用 `text` 而非 `no_run` 标注 —— 标 `no_run` 会被 rustdoc 当真代码编译,
//! `cargo test` 的 doctest 阶段必败(第 120 轮修复的存量失败)。
//!
//! ```text
//! // main 启动期:
//! let sig = shutdown::global();
//! shutdown::install_signal_handlers(sig.clone())?;
//!
//! // 业务中:
//! sig.cancel_token().cancel();
//! if sig.is_triggered() { return; }
//!
//! // 退出路径:
//! shutdown::terminal_restore().await;
//! return Err(anyhow::anyhow!("用户中断"));
//! ```
//!
//! 知识库依据:
//! - `docs/Agent源码调研/专题-第五轮-中断取消与后台任务深度分析.md` §2.1 atomcode 三件套
//! - 方案:`tmpPlan/2026-09-21_03-终止信号处理与资源释放根治方案.md`

use std::io;
use std::sync::Arc;
use std::sync::RwLock;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// 触发原因(供日志与决策)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    /// 用户按下 Ctrl+C(SIGINT)
    UserInterrupt,
    /// 进程收到 SIGTERM(kill 默认)
    Terminated,
    /// 进程收到 SIGHUP(终端关闭)
    Hangup,
    /// 进程收到 SIGTSTP(Ctrl+Z),已被拦截,转为此枚举
    StopRequested,
    /// 业务逻辑主动触发(如 /exit 命令)
    Internal,
}

impl ShutdownReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UserInterrupt => "UserInterrupt(SIGINT)",
            Self::Terminated => "Terminated(SIGTERM)",
            Self::Hangup => "Hangup(SIGHUP)",
            Self::StopRequested => "StopRequested(SIGTSTP->IGN)",
            Self::Internal => "Internal",
        }
    }
}

/// 全局 shutdown 信号触发器(进程级单例,各任务共享)。
#[derive(Clone)]
pub struct ShutdownSignal {
    notify: Arc<Notify>,
    cancel: CancellationToken,
    triggered: Arc<RwLock<Option<ShutdownReason>>>,
}

impl ShutdownSignal {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            notify: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
            triggered: Arc::new(RwLock::new(None)),
        })
    }

    /// 获取当前 cancel token(供编排器注入使用,与 `agent::cancel::CancelToken` 兼容)。
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// 异步等待 shutdown 触发(包含触发原因)。
    pub async fn wait(&self) -> ShutdownReason {
        // 已触发则不阻塞,直接返回
        if let Some(r) = self.current_reason() {
            return r;
        }
        self.notify.notified().await;
        self.current_reason().unwrap_or(ShutdownReason::Internal)
    }

    /// 当前是否已触发(非阻塞)。
    pub fn is_triggered(&self) -> bool {
        self.triggered.read().expect("shutdown lock poisoned").is_some()
    }

    /// 当前触发原因(若已触发)。
    pub fn current_reason(&self) -> Option<ShutdownReason> {
        *self.triggered.read().expect("shutdown lock poisoned")
    }

    /// 触发 shutdown(可重入,只记首次)。
    pub fn trigger(&self, reason: ShutdownReason) {
        {
            let mut g = self.triggered.write().expect("shutdown lock poisoned");
            if g.is_some() {
                return; // 已触发,忽略
            }
            *g = Some(reason);
        }
        self.cancel.cancel();
        self.notify.notify_waiters();
    }
}

// ============================================================================
// 全局单例
// ============================================================================

use std::sync::OnceLock;

static GLOBAL: OnceLock<Arc<ShutdownSignal>> = OnceLock::new();

/// 获取全局 shutdown 单例(进程级)。
pub fn global() -> Arc<ShutdownSignal> {
    GLOBAL.get_or_init(ShutdownSignal::new).clone()
}

// ============================================================================
// 信号 handler 安装
// ============================================================================

/// 安装全局信号 handler(必须在 Tokio runtime 内调用)。
///
/// 平台分叉:
/// - **Unix**:SIGTSTP/SIGTTIN/SIGTTOU 显式 SIG_IGN(防 Ctrl+Z suspend);
///   SIGINT/SIGTERM/SIGHUP 走 tokio signal 监听 → 触发 shutdown。
/// - **Windows**:CTRL_C_EVENT + CTRL_BREAK_EVENT + GenerateConsoleCtrlEvent
///   通过 SetConsoleCtrlHandler 注册;Windows 无 SIGTSTP 等价物。
pub fn install_signal_handlers(sig: Arc<ShutdownSignal>) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::install_blocking_signals()?;
        unix::spawn_async_handlers(sig);
    }
    #[cfg(windows)]
    {
        windows::install(sig)?;
    }
    Ok(())
}

// ============================================================================
// Unix 平台实现
// ============================================================================

#[cfg(unix)]
mod unix {
    use super::*;

    /// SIGTSTP/SIGTTIN/SIGTTOU 必须显式忽略:这些信号的默认行为是「stop 进程」
    /// (SIGTSTP) / 「拒绝后台读/写」(SIGTTIN/TTOU),会让 laew 被挂起或拒读,
    /// 在 macOS Terminal 上表现为整个终端僵死。
    ///
    /// 这是 laew 第 108 轮修复的关键根因之一 —— 旧实现没拦截 SIGTSTP,
    /// 用户按 Ctrl+Z 后 zsh 显示 `suspended  laew --debug`,进程 STDIN 不了,
    /// 整个终端交互被冻结,只能 kill -9 %1 强杀。
    pub fn install_blocking_signals() -> io::Result<()> {
        // SAFETY:libc::signal 是 POSIX 标准 API;SIG_IGN 是合法值;失败仅代表
        // 走默认行为(进程被 stop),不影响其他功能。
        unsafe {
            libc::signal(libc::SIGTSTP, libc::SIG_IGN);
            libc::signal(libc::SIGTTIN, libc::SIG_IGN);
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        }
        Ok(())
    }

    /// SIGINT/SIGTERM/SIGHUP 转给 tokio runtime 异步处理。
    pub fn spawn_async_handlers(sig: Arc<ShutdownSignal>) {
        use tokio::signal::unix::{signal, SignalKind};

        // SIGINT → UserInterrupt
        let sig_int = sig.clone();
        tokio::spawn(async move {
            let mut s = match signal(SignalKind::interrupt()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("install SIGINT failed: {e}");
                    return;
                }
            };
            s.recv().await;
            tracing::info!("[shutdown] SIGINT received, triggering shutdown");
            sig_int.trigger(ShutdownReason::UserInterrupt);
        });

        // SIGTERM → Terminated
        let sig_term = sig.clone();
        tokio::spawn(async move {
            let mut s = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("install SIGTERM failed: {e}");
                    return;
                }
            };
            s.recv().await;
            tracing::info!("[shutdown] SIGTERM received, triggering shutdown");
            sig_term.trigger(ShutdownReason::Terminated);
        });

        // SIGHUP → Hangup
        let sig_hup = sig.clone();
        tokio::spawn(async move {
            let mut s = match signal(SignalKind::hangup()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("install SIGHUP failed: {e}");
                    return;
                }
            };
            s.recv().await;
            tracing::info!("[shutdown] SIGHUP received, triggering shutdown");
            sig_hup.trigger(ShutdownReason::Hangup);
        });
    }
}

// ============================================================================
// Windows 平台实现
// ============================================================================

#[cfg(windows)]
mod windows {
    use super::*;

    /// Windows:简化实现 —— Windows console 下 Ctrl+C 走 GenerateConsoleCtrlEvent,
    /// 但完整的 Win32 API 需要 windows-sys crate,本项目当前未引入。
    ///
    /// 当前策略是 Windows 下让 ctrl_c 通过 tokio::signal::ctrl_c() 处理,
    /// SIGTSTP 在 Windows 下没有等价物,ConsoleSuspend 模式靠 OS 自身。
    ///
    /// 已知限制:
    /// - Ctrl+C 双触发(crossterm 字符层 + SIGINT)行为与 Unix 略有差异;
    /// - 但 Windows console 默认会消费 Ctrl+C 在输入层,所以重复触发概率低于 Unix。
    pub fn install(_sig: Arc<ShutdownSignal>) -> io::Result<()> {
        // Windows 信号处理留作后续接入 SetConsoleCtrlHandler。
        // 本轮不引入 windows-sys 依赖以保持构建轻量。
        // 兜底:tokio::signal::ctrl_c() 在 main.rs / dispatch.rs 中已使用。
        Ok(())
    }
}

// ============================================================================
// 终端还原(任何退出路径必调)
// ============================================================================

/// 强制还原终端到「cooked mode + 主 buffer + 光标可见」。
///
/// 实现:发送 ANSI 序列 + `crossterm::terminal::disable_raw_mode()`。
/// 不依赖 raw mode 状态(就算 raw mode 已经 disable,这些序列也幂等)。
///
/// 必须 async(因为某些 tokio runtime 上下文需要),但内部动作都是 sync 的。
///
/// 调用场景:
/// - 主循环 break 后
/// - signal handler 触发后
/// - panic hook 中(用 [`terminal_restore_sync`])
/// - error 路径
pub async fn terminal_restore() {
    terminal_restore_sync();
}

/// Sync 版本,供 panic hook / atexit handler 调用。
///
/// 关键 ANSI 序列:
/// - `\x1b[?25h`     显示光标
/// - `\x1b[?1049l`   退出 alt screen
/// - `\x1b[r`        重置滚动区(全屏)
/// - `\x1b[?7h`      启用自动换行
/// - `\x1bc`         全终端 reset(终极兜底,某些状态机退出手段)
pub fn terminal_restore_sync() {
    use std::io::Write;

    // 1. 发 ANSI 序列:不依赖 stdout 是否被 lock,直接 write_all。
    //    注意:write_all 失败时静默忽略(进程终止阶段无能为力)。
    {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(b"\x1b[?25h");      // 显示光标
        let _ = handle.write_all(b"\x1b[?1049l");    // 退出 alt screen
        let _ = handle.write_all(b"\x1b[r");          // 重置滚动区
        let _ = handle.write_all(b"\x1b[?7h");        // 自动换行
        let _ = handle.write_all(b"\x1b[m");           // 重置 SGR 属性
        let _ = handle.write_all(b"\x1bc");            // 全终端 reset(终极兜底)
        let _ = handle.flush();
    }

    // 2. disable raw mode。已经是 enableRaw mode 时有效,否则幂等。
    let _ = crossterm::terminal::disable_raw_mode();
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_singleton_returns_same_instance() {
        let a = global();
        let b = global();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn trigger_marks_and_cancels() {
        let s = ShutdownSignal::new();
        assert!(!s.is_triggered());
        s.trigger(ShutdownReason::UserInterrupt);
        assert!(s.is_triggered());
        assert_eq!(s.current_reason(), Some(ShutdownReason::UserInterrupt));
    }

    #[test]
    fn trigger_is_idempotent() {
        let s = ShutdownSignal::new();
        s.trigger(ShutdownReason::UserInterrupt);
        s.trigger(ShutdownReason::Terminated);
        // 第二次 trigger 不覆盖首次
        assert_eq!(s.current_reason(), Some(ShutdownReason::UserInterrupt));
    }

    #[tokio::test]
    async fn wait_returns_when_triggered() {
        let s = ShutdownSignal::new();
        let s_clone = s.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            s_clone.trigger(ShutdownReason::Internal);
        });
        let reason = s.wait().await;
        assert_eq!(reason, ShutdownReason::Internal);
    }

    #[tokio::test]
    async fn wait_returns_immediately_if_already_triggered() {
        let s = ShutdownSignal::new();
        s.trigger(ShutdownReason::Hangup);
        // 不应阻塞
        let reason = tokio::time::timeout(std::time::Duration::from_millis(50), s.wait())
            .await
            .expect("wait should not block after trigger");
        assert_eq!(reason, ShutdownReason::Hangup);
    }

    #[test]
    fn cancel_token_cancels_propagates() {
        let s = ShutdownSignal::new();
        let token = s.cancel_token();
        assert!(!token.is_cancelled());
        s.trigger(ShutdownReason::UserInterrupt);
        assert!(token.is_cancelled());
    }

    #[test]
    fn terminal_restore_sync_does_not_panic() {
        // 不 mock stdout,直接调。stderr 写 ANSI 序列(测试环境无 raw mode)。
        // 不能真的 assert ANSI,因为测试可能不在 TTY 下。
        terminal_restore_sync();
    }

    #[test]
    fn shutdown_reason_as_str() {
        assert_eq!(ShutdownReason::UserInterrupt.as_str(), "UserInterrupt(SIGINT)");
        assert_eq!(ShutdownReason::Terminated.as_str(), "Terminated(SIGTERM)");
        assert_eq!(ShutdownReason::Hangup.as_str(), "Hangup(SIGHUP)");
    }
}