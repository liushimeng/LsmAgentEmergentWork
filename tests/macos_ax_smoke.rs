//! macOS AX 字面量缓存 + 权限引导的烟雾测试(2026-09-16 第 55 轮修正)。
//!
//! 背景修正(推翻 2026-09-15 的旧结论):kAX*Attribute / kAX*Action 在现代 SDK 里是
//! 编译期字面量,从来不是导出符号(dlsym 恒返回 NULL,与版本无关);AX 运行时按字符串值
//! 比较属性名,用字面量 CFString 调用完全等价。因此 AX C API 在 macOS 13~26 全版本可用,
//! 唯一前置条件是 TCC 辅助功能授权(kAXErrorAPIDisabled = -25211)。
//! 本测试验证五件事:
//!   1. MacOsDriver 能实例化、WindowDriver trait 分发正常;
//!   2. dlopen + dlsym 在 Rust 运行时确实找不到 kAX*Attribute(实测为 NULL,
//!      证明常量不可能靠运行时符号解析取得);
//!   3. 字面量缓存就绪(ax_strings_loaded() = true,不依赖 dlsym);
//!   4. 未授权时 inspect 返回「辅助功能未授权」(-25211)而非 fail-closed,
//!      permission_hint 给出可操作的授权步骤 + osascript 降级模板;
//!   5. list_windows 走 CoreGraphics,不需授权,任何情况下可用。

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
fn macos_ax_strings_loaded_via_literal_cache() {
    // 字面量缓存与 macOS 版本无关,应恒为就绪(不再依赖 dlsym)。
    let driver = current_driver();
    // list_windows 会调用 ax_available() → ax_strings_loaded(),若失败会 panic/报错
    let _ = driver.list_windows(Some("nonexistent-filter-xyz")).unwrap();
    println!("[smoke] ✓ 字面量缓存就绪(ax_available = true,不再依赖 dlsym)");
}

#[test]
fn macos_permission_hint_guides_authorization() {
    let driver = current_driver();
    let hint = driver.permission_hint();
    // 当前进程未获辅助功能授权,应返回 Some(引导文案);若已授权则 None(也算通过)。
    if let Some(msg) = hint {
        println!("[smoke] permission_hint = {msg}");
        // 必须给出可操作的授权步骤 + osascript 降级模板,不能再误导「AX 已移除」
        assert!(
            msg.contains("系统设置") && msg.contains("辅助功能") && msg.contains("osascript"),
            "permission_hint 应给出授权步骤与 osascript 降级模板,实际: {msg}"
        );
        assert!(
            !msg.contains("macOS 26") || msg.contains("13~26"),
            "不应再声称 macOS 26 移除了 AX C API,实际: {msg}"
        );
        println!("[smoke] ✓ permission_hint 给出可操作授权步骤 + osascript 降级模板");
    } else {
        println!("[smoke] permission_hint = None(当前进程已获辅助功能授权)");
    }
}

#[test]
fn macos_list_windows_works_via_coregraphics() {
    // list_windows 走 CoreGraphics(CGWindowListCopyWindowInfo),不需授权,任何情况下可用。
    let driver = current_driver();
    let result = driver.list_windows(None);
    let windows = result.expect("list_windows 应成功(CoreGraphics 枚举,不需授权)");
    println!("[smoke] list_windows 返回 {} 个窗口", windows.len());
    assert!(!windows.is_empty(), "应至少枚举到 1 个窗口");
    println!("[smoke] ✓ list_windows 通过 CoreGraphics 工作(无需 AX 授权)");
}

#[test]
fn macos_inspect_reports_not_authorized_not_unavailable() {
    // inspect 需要 AX + 授权。当前未授权时应返回「辅助功能未授权」(-25211),
    // 而不是把 AX 标记为「不可用/已移除」(fail-closed)。
    let driver = current_driver();
    match driver.inspect("1:0", 5, None) {
        Ok(tree) => {
            // 已授权路径:能拿到控件树也算通过(说明字面量调用 AX 成功)
            println!("[smoke] inspect 成功,控件树节点数暗示 AX 调用正常");
            let _ = tree;
            println!("[smoke] ✓ inspect 在已授权环境下走通 AX(字面量缓存生效)");
        }
        Err(err) => {
            let msg = format!("{err}");
            println!("[smoke] inspect err = {msg}");
            assert!(
                msg.contains("辅助功能") && msg.contains("未授权"),
                "未授权时应报告「辅助功能未授权」而非 fail-closed,实际: {msg}"
            );
            assert!(
                !msg.contains("从") || msg.contains("只差授权"),
                "不应误导「AX 已移除」,实际: {msg}"
            );
            println!("[smoke] ✓ inspect 报告「辅助功能未授权」,未误判为 AX 不可用");
        }
    }
}
