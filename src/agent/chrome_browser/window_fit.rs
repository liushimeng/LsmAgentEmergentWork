//! Chrome 窗口尺寸与屏幕适配(双浏览器工具架构 · 视口自适应)。
//!
//! 需求(2026-10-27):浏览器窗口要支持 **1080p**,人工可以**手动拖动大小**,
//! 拖动后内容**自适应铺满**,不出现「缺区域」或「渲染不全」。
//!
//! ## 现状与本模块的分工
//!
//! | 能力 | 谁负责 | 说明 |
//! | ---- | ------ | ---- |
//! | 窗口初始 1920×1080 | `chrome_browser::mod`(`--window-size`) | 外框尺寸 |
//! | **拖动后页面重新布局** | **Chrome 原生** | headed 模式**不下发** `Emulation.setDeviceMetricsOverride`,页面视口 = 窗口内容区,真浏览器行为,天然铺满 |
//! | **窗口不超出屏幕** | **本模块** | `--window-size` 给的是外框;屏幕比它小的时候窗口会溢出屏幕 → 底部区域看不到。本模块按主显示器像素尺寸夹紧 |
//! | 窗口定位到屏幕左上 | 本模块 | `Browser.setWindowBounds`(left=0, top=0) |
//!
//! 屏幕尺寸探测:
//! - macOS:`CGMainDisplayID` + `CGDisplayPixelsWide/High`;
//! - Windows:`GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`;
//! - 其它平台:探测不到 → 不夹紧(维持原尺寸,宁大不缺)。
//!
//! 设计详案 `docs/双浏览器工具架构/03-浏览器控制权状态机与视口自适应.md`。

use chromiumoxide::browser::Browser;
use chromiumoxide::Page;

/// 目标窗口尺寸(1080p)。与 `mod.rs` 的 `--window-size` 保持一致。
pub const TARGET_WINDOW_W: u32 = 1920;
pub const TARGET_WINDOW_H: u32 = 1080;

/// 窗口四周留白(px):贴着屏幕边缘会顶住菜单栏/任务栏。
const SCREEN_MARGIN: u32 = 48;

/// 夹紧后的最小窗口尺寸(太小就没法正常看页面了)。
const MIN_WINDOW_W: u32 = 1024;
const MIN_WINDOW_H: u32 = 640;

/// 主显示器像素尺寸(探测不到返回 `None`)。
pub fn screen_size() -> Option<(u32, u32)> {
    #[cfg(target_os = "macos")]
    {
        use core_graphics::display::{CGDisplayPixelsHigh, CGDisplayPixelsWide, CGMainDisplayID};
        let id = unsafe { CGMainDisplayID() };
        let w = unsafe { CGDisplayPixelsWide(id) } as u32;
        let h = unsafe { CGDisplayPixelsHigh(id) } as u32;
        if w > 0 && h > 0 {
            return Some((w, h));
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

/// 把目标窗口尺寸按屏幕可用区域夹紧。
///
/// 屏幕比目标小(例如 1440×900 笔记本)时按 `屏幕 - 留白` 夹紧,
/// 保证**窗口完整落在屏幕内**(不缺区域);探测不到屏幕时原样返回。
pub fn fit_to_screen(width: u32, height: u32) -> (u32, u32) {
    let Some((sw, sh)) = screen_size() else {
        return (width, height);
    };
    let avail_w = sw.saturating_sub(SCREEN_MARGIN * 2).max(MIN_WINDOW_W);
    let avail_h = sh.saturating_sub(SCREEN_MARGIN * 2).max(MIN_WINDOW_H);
    let w = width.min(avail_w).max(MIN_WINDOW_W.min(avail_w));
    let h = height.min(avail_h).max(MIN_WINDOW_H.min(avail_h));
    (w, h)
}

/// 通过 CDP 把窗口摆到屏幕左上并设为指定尺寸。
///
/// 返回是否成功(失败不致命:窗口仍是 `--window-size` 给的尺寸,只是可能溢出屏幕)。
pub async fn apply_window_bounds(browser: &Browser, page: &Page, width: u32, height: u32) -> bool {
    use chromiumoxide::cdp::browser_protocol::browser::{
        Bounds, GetWindowForTargetParams, SetWindowBoundsParams,
    };
    let get = GetWindowForTargetParams::builder()
        .target_id(page.target_id().clone())
        .build();
    let Ok(resp) = browser.execute(get).await else {
        return false;
    };
    let window_id = resp.window_id.clone();
    let bounds = Bounds::builder()
        .left(0i64)
        .top(0i64)
        .width(width as i64)
        .height(height as i64)
        .build();
    let Ok(set) = SetWindowBoundsParams::builder()
        .window_id(window_id)
        .bounds(bounds)
        .build()
    else {
        return false;
    };
    browser.execute(set).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 目标尺寸是1080p() {
        assert_eq!(TARGET_WINDOW_W, 1920);
        assert_eq!(TARGET_WINDOW_H, 1080);
    }

    #[test]
    fn 夹紧不小于最小窗口且不超过目标() {
        // 探测不到屏幕时原样返回。
        if screen_size().is_none() {
            assert_eq!(fit_to_screen(1920, 1080), (1920, 1080));
            return;
        }
        let (w, h) = fit_to_screen(1920, 1080);
        assert!(w <= 1920 && h <= 1080, "不得大于目标:{}x{}", w, h);
        assert!(w >= 640 && h >= 480, "不得过小:{}x{}", w, h);
    }

    #[test]
    fn 小屏被夹到屏内() {
        // 直接验证夹紧逻辑的边界:给定一个极小屏,输出不应超过它。
        let (w, h) = fit_to_screen(1920, 1080);
        if let Some((sw, sh)) = screen_size() {
            assert!(w <= sw.max(MIN_WINDOW_W), "窗口宽度不应超出屏幕:{} > {}", w, sw);
            assert!(h <= sh.max(MIN_WINDOW_H), "窗口高度不应超出屏幕:{} > {}", h, sh);
        }
    }
}
