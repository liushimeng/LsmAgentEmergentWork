//! FFI 回调防火墙:原生回调(`extern "C"` / `extern "system"`)体统一走 `guard`。
//!
//! 背景(2026-11-04 查看器滚轮崩溃事故):ObjC/Win32 回调是 **nounwind 边界**,
//! 回调体内任何 Rust panic 展开到边界都会触发「panic in a function that cannot
//! unwind」→ `process::abort()`,整个查看器/弹窗进程当场死亡、窗口消失。
//! 回调体包一层 `catch_unwind` 后,Rust panic 被就地吞掉(panic hook 照常写
//! CrashDump,证据不丢),进程与窗口存活。
//!
//! 注意:`catch_unwind` **拦不住 foreign exception**(ObjC NSException / C++
//! 异常)。对那一类的防御是**不发会抛异常的消息**——事件类选择器一律先经
//! `respondsToSelector:` 守卫再发(见 `view/macos.rs::respond`)。

/// 在 FFI 回调体内执行 `f`,吞掉 Rust panic(不跨越 nounwind 边界)。
///
/// `what` 是回调名,仅用于诊断输出(stderr;查看器 stderr 已转发宿主日志,
/// 见 `view/mod.rs` 的 stderr 转发线程)。
#[inline]
pub fn guard(what: &str, f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        eprintln!("[laew] ffi 回调 panic 已拦截({what}),进程存活;详见 CrashReport");
    }
}

/// 带返回值的 FFI 回调版(窗口过程/LRESULT 类):panic 时返回 `on_panic` 兜底值。
#[inline]
pub fn guard_ret<T>(what: &str, on_panic: T, f: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("[laew] ffi 回调 panic 已拦截({what}),进程存活;详见 CrashReport");
            on_panic
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 防火墙吞掉回调内恐慌不让其外泄() {
        let ran = std::cell::Cell::new(false);
        guard("测试回调", || {
            ran.set(true);
            panic!("故意恐慌");
        });
        assert!(ran.get(), "回调体应已执行");
    }

    #[test]
    fn 正常回调原样执行() {
        let ran = std::cell::Cell::new(false);
        guard("测试回调", || ran.set(true));
        assert!(ran.get(), "回调体应已执行");
    }
}
