//! 主显示器**逻辑尺寸**探测(浏览器窗口/视口按屏幕夹紧时用)。
//!
//! ## 为什么要逻辑尺寸而不是物理像素
//!
//! macOS Retina 主屏物理像素是逻辑尺寸的 2 倍(3200×1800 逻辑 / 6400×3600 物理)。
//! 窗口尺寸(`--window-size` / NSWindow frame / 视口宽高)全部是**逻辑点(DIP)**,
//! 拿物理像素去比会把「屏幕比窗口小」判成「屏幕够大」,夹紧直接失效 ——
//! 窗口照样溢出屏幕,底部区域人工看不到。
//!
//! macOS 用 `CGDisplayBounds`(逻辑点,全局显示坐标矩形);Windows 用
//! `GetSystemMetrics`(已是逻辑像素)。探测不到(容器/CI/无 GUI 会话)返回 `None`,
//! 调用方一律**不夹**(宁大不缺,也不凭空缩小视口影响站点布局)。

/// 主显示器逻辑尺寸 `(宽, 高)`(单位:DIP / 逻辑像素)。探测不到返回 `None`。
pub fn main_display_size() -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    {
        use core_graphics::display::{CGDisplayBounds, CGMainDisplayID};
        let id = unsafe { CGMainDisplayID() };
        // `CGDisplayBounds` 返回全局显示坐标里的矩形(逻辑点),不是物理像素。
        let rect = unsafe { CGDisplayBounds(id) };
        let w = rect.size.width;
        let h = rect.size.height;
        // 合理性护栏:异常值(0 / 负 / 离谱大)一律当作探测失败。
        if w.is_finite() && h.is_finite() && w >= 100.0 && h >= 100.0 && w <= 20000.0 && h <= 20000.0 {
            return Some((w.round() as u32, h.round() as u32));
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
        };
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) } as u32;
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) } as u32;
        if w > 0 && h > 0 {
            return Some((w, h));
        }
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 探测值为正且在合理区间() {
        if let Some((w, h)) = main_display_size() {
            assert!(w >= 100 && h >= 100, "尺寸过小:{w}x{h}");
            assert!(w <= 20000 && h <= 20000, "尺寸过大:{w}x{h}");
        }
    }

    #[test]
    fn mac上取的是逻辑点而非物理像素() {
        // Retina 主屏物理像素是逻辑的 2 倍;若误用物理像素,这里的值会是逻辑的 2 倍。
        #[cfg(target_os = "macos")]
        if let Some((w, _)) = main_display_size() {
            assert!(w <= 10000, "疑似取到物理像素(应 ≤10000 逻辑点):{w}");
        }
    }
}
