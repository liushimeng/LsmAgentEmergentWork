//! OS 显示器拓扑真值（第 184 轮）—— `screen.isExtended` 回填的唯一真值来源。
//!
//! 真机双档基线（`docs/MCP_Web_Use/38` §二）：`screen.isExtended`（Window Management
//! API 的多显示器标记）是**显示器派生读数** —— 有头档跟随 OS 拓扑（本机双显示器读
//! `true`），无头档的虚拟屏幕恒 `false`。该 API 不在 `Emulation.setEmulatedMedia`
//! 仿真面内（无 CDP 原生通道），故由 `FIDELITY_JS ⑥` 走 JS 回填，真值由本模块从
//! OS 实读供给（不伪造：OS 说单显示器就不补，`false` 即真值）。
//!
//! 进程级 `OnceLock` 缓存：显示器热插拔需重启进程才反映（文档口径）。

use std::sync::OnceLock;

/// OS 是否连接了多台显示器（`None` = 读取失败，不处置，fail-open）。
pub(crate) fn multi_display() -> Option<bool> {
    static CACHE: OnceLock<Option<bool>> = OnceLock::new();
    *CACHE.get_or_init(detect_multi_display)
}

/// `%MULTI_DISPLAY%` 占位符字面量（`true` / `false` / `null`，供
/// `browser_ns::stealth_js_for` 模板替换）。
pub(crate) fn multi_display_literal() -> &'static str {
    match multi_display() {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    }
}

fn detect_multi_display() -> Option<bool> {
    #[cfg(windows)]
    {
        // SM_CMONITORS(80)：桌面显示器数量。计数值不受 DPI 虚拟化影响。
        // user32 缺席/失败返回 0 —— 0 台显示器物理不可能，按读取失败处理(fail-open)。
        let count = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CMONITORS,
            )
        };
        if count <= 0 {
            return None;
        }
        Some(count > 1)
    }
    #[cfg(target_os = "macos")]
    {
        // CoreGraphics C API 直接 FFI(macos_legacy 同款手法,零新 crate):
        // CGGetActiveDisplayList(0, null, &count) 只取计数不取句柄。
        #[repr(C)]
        #[allow(non_camel_case_types)]
        type CGDirectDisplayID = u32;
        extern "C" {
            fn CGGetActiveDisplayList(
                max_displays: u32,
                active_displays: *mut CGDirectDisplayID,
                display_count: *mut u32,
            ) -> i32;
        }
        let mut count: u32 = 0;
        // 返回值 0 = kCGErrorSuccess
        let err = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
        if err != 0 {
            return None;
        }
        Some(count > 1)
    }
    #[cfg(target_os = "linux")]
    {
        // xrandr --listmonitors 输出首行 "Monitors: N"(wmctrl/xdotool 同族子进程手法);
        // 无 X / 无 xrandr 一律 fail-open 返回 None,不猜测。
        let out = std::process::Command::new("xrandr")
            .arg("--listmonitors")
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().find(|l| l.trim_start().starts_with("Monitors:"))?;
        let n: u64 = line
            .split(':')
            .nth(1)?
            .trim()
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        Some(n > 1)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真值三态字面量必须是合法 JS 字面量 —— 模板占位符替换后脚本能直接跑。
    /// (具体 true/false 取决于本机拓扑,不 assert 方向,只 assert 形状。)
    #[test]
    fn 显示器真值字面量形状() {
        assert!(
            ["true", "false", "null"].contains(&multi_display_literal()),
            "占位符字面量必须是 JS 合法字面量:{}",
            multi_display_literal()
        );
    }

    /// Windows/macOS 上 OS 读取应成功(拿得到确定真值);读取失败(返回 None)时
    /// 上游零动作 —— 本测试在双显示器开发机上顺带把「真值=Some(true)」钉进日志。
    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn 主流平台能读到确定真值() {
        let v = multi_display();
        assert!(v.is_some(), "Windows/macOS 应能读到确定真值(fail-open 是 Linux 无 X 的预期)");
        tracing::info!(?v, "显示器拓扑真值(开发机基线钉死用)");
    }
}
