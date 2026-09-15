//! macOS 26+ AX 字符串常量移除的 fail-closed 烟雾测试(2026-09-15)。
//!
//! 背景:macOS 26.5(Darwin 25.5,Tahoe)已将 kAX*Attribute / kAX*Action
//! 字符串常量从 ApplicationServices.framework 的 C ABI 中移除(dlsym 返回 NULL)。
//! 本测试在集成测试同进程验证三件事:
//!   1. MacOsDriver 能实例化、WindowDriver trait 分发正常;
//!   2. dlopen + dlsym 在 Rust 运行时确实找不到 kAX*Attribute(实测为 NULL);
//!   3. permission_hint / list_windows 在 macOS 26+ 上 fail-closed,
//!      返回可读的「macOS 26+ 已移除 AX C API」错误,
//!      而不是误导用户去开启「辅助功能」权限。

#![cfg(target_os = "macos")]

use lsm_agent::agent::window::{current_driver, WindowDriver};

#[test]
fn macos_driver_compiles_and_runs() {
    let driver = current_driver();
    assert_eq!(driver.platform_name(), "macos");
    println!("[smoke] ✓ MacOsDriver 实例化 + platform_name = \"macos\"");
}

#[test]
fn macos_ax_dlsym_in_runtime() {
    use std::ffi::CString;
    unsafe {
        let path = CString::new(
            "/System/Library/Frameworks/ApplicationServices.framework/Versions/A/ApplicationServices"
        ).unwrap();
        let h = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
        assert!(!h.is_null(), "ApplicationServices 必须能 dlopen");
        for name in &[
            "kAXChildrenAttribute",
            "kAXRoleAttribute",
            "kAXTitleAttribute",
            "kAXValueAttribute",
            "kAXWindowsAttribute",
            "kAXFocusedAttribute",
            "kAXPressAction",
        ] {
            let c = CString::new(*name).unwrap();
            let s = libc::dlsym(h, c.as_ptr());
            println!("[smoke]   dlsym({:30}) = {:p}", name, s);
            assert!(s.is_null(), "macOS 26.5 期望 {name} = NULL,实际 {s:p}");
        }
        println!("[smoke] ✓ 确认 macOS 26.5 已移除 kAX*Attribute C ABI 符号");
    }
}

#[test]
fn macos_permission_hint_on_tahoe() {
    let driver = current_driver();
    let hint = driver.permission_hint();
    let msg = hint.expect("macOS 26+ 上 permission_hint 必须返回 Some(原因)");
    println!("[smoke] permission_hint = {msg}");
    assert!(
        msg.contains("macOS 26") && msg.contains("AX"),
        "应明确指出 macOS 26+ AX 已移除,实际: {msg}"
    );
    println!("[smoke] ✓ permission_hint 触发 fail-closed,无障碍权限问题被遮蔽");
}

#[test]
fn macos_list_windows_fail_closed_on_tahoe() {
    let driver = current_driver();
    let result = driver.list_windows(None);
    let err = result.expect_err("macOS 26+ 上 list_windows 必须返回 Err,而非空 Vec");
    let msg = format!("{err}");
    println!("[smoke] list_windows err = {msg}");
    assert!(
        msg.contains("macOS 26") && msg.contains("AX"),
        "list_windows 应返回 macOS 26+ AX 不可用错误,实际: {msg}"
    );
    println!("[smoke] ✓ list_windows 入口 fail-closed");
}
