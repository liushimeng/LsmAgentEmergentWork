//! 显示器尺寸探测(LsmAgentBrowser headed 查看器用)。
//!
//! **为什么需要**:查看器窗口按**视口尺寸**开(内容区 = 视口 + 工具栏)。
//! 视口缺省 1920×1080,但笔记本屏幕常只有 1440×900 —— 窗口会超出屏幕,
//! 底部区域在人工视角里根本看不到(「缺区域」/「渲染不全」)。
//!
//! 因此 headed 模式下把视口**夹到主显示器可用区域**内;hidden 模式(无窗口)
//! 不夹 —— 逻辑视口与物理屏无关,夹了反而影响站点布局。
//!
//! 探测不到屏幕时(容器/CI/无 GUI 会话)一律不夹,维持 1920×1080。

/// 窗口四周留白(px):贴着屏幕边缘会顶住菜单栏/任务栏。
const SCREEN_MARGIN: u32 = 48;

/// 工具栏高度(查看器顶部按钮条)——从可用高度里先扣掉。
const TOOLBAR_H: u32 = 38;

/// 夹紧后的最小视口(太小没法正常看页面)。
const MIN_VIEWPORT_W: u32 = 1024;
const MIN_VIEWPORT_H: u32 = 640;

/// 主显示器**逻辑**尺寸(探测不到返回 `None`)。
///
/// 逻辑点而非物理像素:视口/窗口尺寸是 DIP,拿 Retina 的物理像素去比会判成
/// 「屏幕够大」,夹紧直接失效(窗口照样溢出屏幕,底部区域人工看不到)。
pub fn screen_size() -> Option<(u32, u32)> {
    crate::agent::display_size::main_display_size()
}

/// 把视口尺寸夹到主显示器可用区域内(探测不到屏幕 → 原样返回)。
///
/// 返回 `(width, height)`:夹紧后的视口宽高。
pub fn fit_viewport_to_screen(width: u32, height: u32) -> (u32, u32) {
    let Some((sw, sh)) = screen_size() else {
        return (width, height);
    };
    let avail_w = sw.saturating_sub(SCREEN_MARGIN * 2).max(MIN_VIEWPORT_W);
    // 工具栏占掉的高度要从视口里扣(窗口外框 = 视口 + 工具栏)。
    let avail_h = sh
        .saturating_sub(SCREEN_MARGIN * 2)
        .saturating_sub(TOOLBAR_H)
        .max(MIN_VIEWPORT_H);
    let w = width.min(avail_w).max(MIN_VIEWPORT_W.min(avail_w));
    let h = height.min(avail_h).max(MIN_VIEWPORT_H.min(avail_h));
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 探测不到屏幕时原样返回() {
        if screen_size().is_none() {
            assert_eq!(fit_viewport_to_screen(1920, 1080), (1920, 1080));
        }
    }

    #[test]
    fn 夹紧不小于最小视口且不超过原值() {
        let (w, h) = fit_viewport_to_screen(1920, 1080);
        assert!(w <= 1920 && h <= 1080, "不得大于原值:{}x{}", w, h);
        assert!(w >= 640 && h >= 480, "不得过小:{}x{}", w, h);
    }

    #[test]
    fn 视口不超出屏幕() {
        let (w, h) = fit_viewport_to_screen(1920, 1080);
        if let Some((sw, sh)) = screen_size() {
            assert!(w <= sw.max(MIN_VIEWPORT_W), "宽不应超出屏:{} > {}", w, sw);
            assert!(
                h + TOOLBAR_H <= sh.max(MIN_VIEWPORT_H),
                "视口高+工具栏不应超出屏:{} > {}",
                h + TOOLBAR_H,
                sh
            );
        }
    }
}
