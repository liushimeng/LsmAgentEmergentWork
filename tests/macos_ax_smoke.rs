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
fn macos_list_windows_works_via_coregraphics_on_tahoe() {
    // 2026-09-15 修复:list_windows 使用 CoreGraphics(CGWindowListCopyWindowInfo),
    // 不需要 AX API,因此在 macOS 26+ 上仍然可用。
    // 但 inspect/act 仍然需要 AX API,在 macOS 26+ 上不可用。
    let driver = current_driver();
    let result = driver.list_windows(None);
    let windows = result.expect("macOS 26+ 上 list_windows 应成功(CoreGraphics 枚举)");
    println!("[smoke] list_windows 返回 {} 个窗口", windows.len());
    assert!(!windows.is_empty(), "应至少枚举到 1 个窗口");
    println!("[smoke] ✓ list_windows 通过 CoreGraphics 工作(无需 AX API)");
}

#[test]
fn macos_inspect_fail_closed_on_tahoe() {
    // inspect 需要 AX API,在 macOS 26+ 上不可用
    let driver = current_driver();
    let result = driver.inspect("1:0", 5, None);
    let err = result.expect_err("macOS 26+ 上 inspect 必须返回 Err");
    let msg = format!("{err}");
    println!("[smoke] inspect err = {msg}");
    assert!(
        msg.contains("macOS 26") && msg.contains("AX"),
        "inspect 应返回 macOS 26+ AX 不可用错误,实际: {msg}"
    );
    println!("[smoke] ✓ inspect 入口 fail-closed");
}
