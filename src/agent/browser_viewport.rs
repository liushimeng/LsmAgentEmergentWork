//! 浏览器窗口 / 视口尺寸决策(第 125 轮起从 `browser.rs` 机械搬移,第 139 轮扩展)。
//!
//! 本模块是「尺寸」的唯一真源,只放纯函数 + 测量 JS,**不持有任何浏览器状态**,
//! 便于单测锁死。两类决策:
//!
//! 1. [`viewport_fit_plan`] —— 页面**内容**超出**视口**时向上扩(第 125 轮,
//!    ≤2K 上限);只放大不缩小。
//! 2. [`headed_window_plan`] —— 有头模式下窗口**外框**超出**屏幕工作区**时向下收
//!    (第 139 轮),并补偿浏览器 chrome 高度,保证**页面视口** ≥ 720p。
//!
//! 为什么不用自己探测屏幕分辨率:`window.screen.availWidth/availHeight` 就是
//! 浏览器算好的**工作区**(已扣 Dock/任务栏),`outerHeight - innerHeight` 就是
//! **chrome 高度**,两者天生跨平台且随 DPR 正确换算,比自建各平台原生 FFI 可靠。

use serde_json::Value;

// =================== 第 125 轮(2026-09-23):视口基准 1080p 与 2K 自动扩展 ===================

/// 全模式默认启动窗口:1920×1080(1080p)。hidden 原 1440×900 视口过窄,
/// 现代 Web 应用(min-width > 1440 的后台/SaaS)横向被裁 → 元素不可见不可点、
/// 截图显示不全,本轮根治。
pub const DEFAULT_WINDOW_W: u32 = 1920;
/// 全模式默认启动窗口高(1080p)。
pub const DEFAULT_WINDOW_H: u32 = 1080;
/// 视口自动扩展上限宽(2K = 2560×1440):内容超此宽度仍溢出时保持滚动 +
/// full_page 截图既有路径,不再无限撑大。
pub const VIEWPORT_FIT_MAX_W: u32 = 2560;
/// 视口自动扩展上限高(2K)。
pub const VIEWPORT_FIT_MAX_H: u32 = 1440;
/// 溢出判定容差(px):亚像素布局/阴影圆角/1px 边框误差不触发扩展。
pub(crate) const VIEWPORT_FIT_TOL: f64 = 2.0;

// =================== 第 139 轮(2026-10-09):可见模式视口下限 720p ===================

/// 页面视口最小宽(720p 的宽)。低于此值横向裁切风险大,现代后台/SaaS 常
/// `min-width` 1280,视口不够宽会出现横向滚动条甚至元素点不到。
pub const MIN_VIEWPORT_W: u32 = 1280;
/// 页面视口最小高(720p 的高)。
pub const MIN_VIEWPORT_H: u32 = 720;
/// 有头模式窗口四周留白(px):避免窗口贴死屏幕边缘 / 挡住 Dock 与任务栏。
pub const HEADED_SCREEN_MARGIN: u32 = 8;
/// chrome 高度探测失败时的兜底估计(px):标签栏 + 地址栏 + 书签栏。
pub const HEADED_CHROME_FALLBACK_H: u32 = 140;

/// 默认启动窗口尺寸(第 125 轮:全模式统一 1080p)。
pub fn default_window_size() -> (u32, u32) {
    (DEFAULT_WINDOW_W, DEFAULT_WINDOW_H)
}

/// 单维度适配判定:内容维度 > 视口维度 + 容差 且仍有扩展空间时返回 Some(目标)。
/// 只放大不缩小;当前已 ≥ cap 时维持原状(返回 None,交给滚动/full_page)。
fn fit_dim(cur: f64, content: f64, cap: u32) -> Option<u32> {
    if content <= cur + VIEWPORT_FIT_TOL {
        return None;
    }
    let cur_i = cur.max(0.0).ceil() as u32;
    if cur_i >= cap {
        return None;
    }
    let target = content.ceil() as u32;
    let target = target.clamp(cur_i + 1, cap);
    if target > cur_i {
        Some(target)
    } else {
        None
    }
}

/// 视口自动扩展决策(纯函数,可单测):输入当前视口 (iw,ih) 与内容滚动尺寸
/// (sw,sh),需要扩展返回 Some((target_w,target_h)),否则 None。
pub fn viewport_fit_plan(
    iw: f64,
    ih: f64,
    sw: f64,
    sh: f64,
    max_w: u32,
    max_h: u32,
) -> Option<(u32, u32)> {
    match (fit_dim(iw, sw, max_w), fit_dim(ih, sh, max_h)) {
        (Some(w), Some(h)) => Some((w, h)),
        (Some(w), None) => Some((w, ih.max(0.0).ceil() as u32)),
        (None, Some(h)) => Some((iw.max(0.0).ceil() as u32, h)),
        (None, None) => None,
    }
}

// =================== 有头模式窗口收边与视口下限(第 139 轮) ===================

/// 有头模式窗口尺寸决策(纯函数,可单测)。
///
/// 输入:
/// - `cur_w` / `cur_h`:当前**窗口外框**尺寸(CSS 像素,来自 `--window-size`);
/// - `avail_w` / `avail_h`:屏幕**工作区**(来自 `window.screen.avail*`,已扣 Dock);
/// - `chrome_h`:浏览器 chrome 高度(标签栏 + 地址栏 + 书签栏),
///   由 `outerHeight - innerHeight` 实测;≤0 时按 [`HEADED_CHROME_FALLBACK_H`] 兜底。
///
/// 返回 `Some((win_w, win_h))` 表示需要 `Browser.setWindowBounds` 调整;
/// `None` 表示当前窗口已满足「完全落在屏幕内 + 视口 ≥ 720p」。
///
/// 决策规则(按序):
/// 1. `avail` 任一维 ≤ 0(探测失败)→ `None`,不做任何猜测调整;
/// 2. 上限 `cap = avail - 2 * HEADED_SCREEN_MARGIN`,保证窗口完整可见(用户反馈的
///    「显示不全」根因:1920×1080 窗口硬塞进 1512×982 屏,右侧被挤出屏外);
/// 3. 下限:窗口高要 ≥ `MIN_VIEWPORT_H + chrome_h`(否则页面视口不足 720p),
///    宽要 ≥ `MIN_VIEWPORT_W`;屏幕实在给不出时以 `cap` 为准(如实退让,不硬撑);
/// 4. 目标是 `(clamp(cur_w), clamp(cur_h))` 的默认值 —— 即**默认 1080p**,
///    小屏自动降到「屏幕能给的、且视口仍 ≥720p」的最大值。
pub fn headed_window_plan(
    cur_w: f64,
    cur_h: f64,
    avail_w: f64,
    avail_h: f64,
    chrome_h: f64,
) -> Option<(u32, u32)> {
    if !(avail_w > 0.0) || !(avail_h > 0.0) {
        return None;
    }
    let m = HEADED_SCREEN_MARGIN as f64;
    // cap:窗口外框最大可取值(留边)。
    let cap_w = (avail_w - m * 2.0).floor().max(1.0);
    let cap_h = (avail_h - m * 2.0).floor().max(1.0);
    // chrome 兜底:实测为 0/负(部分平台 outer==inner)时按经验值补。
    let chrome = if chrome_h > 0.0 {
        chrome_h
    } else {
        HEADED_CHROME_FALLBACK_H as f64
    };
    // 视口下限换算成外框下限。
    let floor_w = MIN_VIEWPORT_W as f64;
    let floor_h = MIN_VIEWPORT_H as f64 + chrome;
    // 下限与上限取交:屏幕给不出 720p 视口时,如实用 cap(视口不足但至少窗口全可见)。
    let lo_w = floor_w.min(cap_w);
    let lo_h = floor_h.min(cap_h);

    // 目标 = 请求尺寸(缺省 1080p),clamp 进 [lo, cap]。
    let want_w = if cur_w > 0.0 { cur_w } else { DEFAULT_WINDOW_W as f64 };
    let want_h = if cur_h > 0.0 { cur_h } else { DEFAULT_WINDOW_H as f64 };
    let target_w = want_w.clamp(lo_w, cap_w).floor().max(1.0);
    let target_h = want_h.clamp(lo_h, cap_h).floor().max(1.0);

    let changed = (target_w - cur_w).abs() > VIEWPORT_FIT_TOL
        || (target_h - cur_h).abs() > VIEWPORT_FIT_TOL;
    if changed {
        Some((target_w as u32, target_h as u32))
    } else {
        None
    }
}

// =================== 测量(CDP evaluate,returnByValue) ===================

/// 视口/内容尺寸测量 JS(fit 与截图溢出提示共用)。
pub(crate) const VIEWPORT_METRICS_JS: &str = r#"(() => {
    const de = document.documentElement, b = document.body;
    const sw = Math.max(de ? de.scrollWidth : 0, b ? b.scrollWidth : 0);
    const sh = Math.max(de ? de.scrollHeight : 0, b ? b.scrollHeight : 0);
    return {
        innerWidth: window.innerWidth, innerHeight: window.innerHeight,
        scrollWidth: sw, scrollHeight: sh,
        devicePixelRatio: window.devicePixelRatio || 1,
    };
})()"#;

/// 有头模式窗口/chrome/屏幕工作区测量 JS(第 139 轮)。
///
/// `screen.availWidth/availHeight` = 屏幕工作区(已扣 Dock/任务栏);
/// `outerWidth/outerHeight` = 窗口外框;与 inner 的差即浏览器 chrome。
pub(crate) const HEADED_WINDOW_METRICS_JS: &str = r#"(() => ({
    availWidth: window.screen.availWidth, availHeight: window.screen.availHeight,
    outerWidth: window.outerWidth, outerHeight: window.outerHeight,
    innerWidth: window.innerWidth, innerHeight: window.innerHeight,
    devicePixelRatio: window.devicePixelRatio || 1,
}))()"#;

/// 读取页面视口 + 内容滚动尺寸(CDP evaluate,returnByValue)。
pub(crate) async fn page_viewport_metrics(
    page: &chromiumoxide::Page,
) -> std::result::Result<Value, String> {
    page.evaluate(VIEWPORT_METRICS_JS)
        .await
        .map_err(|e| format!("测量视口失败: {e}"))?
        .value()
        .cloned()
        .ok_or_else(|| "测量视口失败: 返回为空".to_string())
}

/// 读取有头模式窗口指标(第 139 轮);失败返回 `Err` 由调用方 fail-open。
pub(crate) async fn headed_window_metrics(
    page: &chromiumoxide::Page,
) -> std::result::Result<Value, String> {
    page.evaluate(HEADED_WINDOW_METRICS_JS)
        .await
        .map_err(|e| format!("测量窗口失败: {e}"))?
        .value()
        .cloned()
        .ok_or_else(|| "测量窗口失败: 返回为空".to_string())
}

// =================== 单测(纯函数,不触达浏览器) ===================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 第 125 轮:内容溢出扩展 ----------

    #[test]
    fn default_window_is_1080p() {
        assert_eq!(default_window_size(), (1920, 1080));
        assert_eq!((DEFAULT_WINDOW_W, DEFAULT_WINDOW_H), (1920, 1080));
    }

    #[test]
    fn viewport_fit_plan_no_overflow() {
        assert_eq!(viewport_fit_plan(1920.0, 1080.0, 1920.0, 1080.0, 2560, 1440), None);
        assert_eq!(viewport_fit_plan(1920.0, 1080.0, 1922.0, 1082.0, 2560, 1440), None);
    }

    #[test]
    fn viewport_fit_plan_horizontal_clipped() {
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 2200.0, 1080.0, 2560, 1440),
            Some((2200, 1080))
        );
    }

    #[test]
    fn viewport_fit_plan_vertical_short_content() {
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 1920.0, 1200.0, 2560, 1440),
            Some((1920, 1200))
        );
    }

    #[test]
    fn viewport_fit_plan_clamps_to_2k_cap() {
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 4000.0, 5000.0, 2560, 1440),
            Some((2560, 1440))
        );
        assert_eq!(
            viewport_fit_plan(1920.0, 1080.0, 4000.0, 1080.0, 2560, 1440),
            Some((2560, 1080))
        );
    }

    #[test]
    fn viewport_fit_plan_never_shrinks_or_exceeds_cap_when_larger() {
        assert_eq!(viewport_fit_plan(3840.0, 1080.0, 4200.0, 1080.0, 2560, 1440), None);
    }

    // ---------- 第 139 轮:有头窗口收边与视口下限 ----------

    /// 大屏(2560×1440 工作区):1920×1080 请求原样保留,viewport 已 ≥720p。
    #[test]
    fn headed_plan_keeps_1080p_on_large_screen() {
        assert_eq!(
            headed_window_plan(1920.0, 1080.0, 2560.0, 1440.0, 140.0),
            None
        );
    }

    /// 小屏(1440×900 工作区,典型 MacBook):窗口被收进屏幕,不再溢出屏幕外。
    #[test]
    fn headed_plan_shrinks_to_fit_small_screen() {
        let plan = headed_window_plan(1920.0, 1080.0, 1440.0, 900.0, 140.0).expect("应触发收边");
        let (w, h) = plan;
        assert!(w as f64 <= 1440.0 - 2.0 * HEADED_SCREEN_MARGIN as f64, "宽必须留边:{w}");
        assert!(h as f64 <= 900.0 - 2.0 * HEADED_SCREEN_MARGIN as f64, "高必须留边:{h}");
        // 且视口仍 ≥ 720p(收边后仍达下限)。
        assert!(h as f64 - 140.0 >= MIN_VIEWPORT_H as f64, "视口高度不足 720p:{h}");
    }

    /// 极小屏(1280×720 工作区):视口给不到 720p,如实退让到「窗口全可见」,
    /// 绝不硬撑出屏外。
    #[test]
    fn headed_plan_degrades_gracefully_on_tiny_screen() {
        let (w, h) = headed_window_plan(1920.0, 1080.0, 1280.0, 720.0, 140.0).expect("应收边");
        assert_eq!(w as f64, 1280.0 - 2.0 * HEADED_SCREEN_MARGIN as f64);
        assert_eq!(h as f64, 720.0 - 2.0 * HEADED_SCREEN_MARGIN as f64);
    }

    /// chrome 高度实测为 0/负时按经验值兜底,不让窗口莫名变小。
    #[test]
    fn headed_plan_falls_back_on_chrome_height() {
        let (with, _) = headed_window_plan(1920.0, 1080.0, 1512.0, 982.0, 140.0).unwrap();
        let (without, _) = headed_window_plan(1920.0, 1080.0, 1512.0, 982.0, 0.0).unwrap();
        // chrome 兜底值(140)与实测(140)一致 → 决策相同。
        assert_eq!(with, without);
    }

    /// 探测失败(avail ≤ 0)→ 不猜、不动。
    #[test]
    fn headed_plan_no_gui_metrics_is_noop() {
        assert_eq!(headed_window_plan(1920.0, 1080.0, 0.0, 900.0, 140.0), None);
        assert_eq!(headed_window_plan(1920.0, 1080.0, 1440.0, 0.0, 140.0), None);
    }

    /// 窗口太小(用户手动缩小)→ 抬回视口下限,不做无谓的像素级抖动。
    #[test]
    fn headed_plan_expands_to_viewport_floor() {
        let (w, h) = headed_window_plan(800.0, 500.0, 2560.0, 1440.0, 140.0).expect("应抬高");
        assert_eq!(w, MIN_VIEWPORT_W);
        assert_eq!(h, MIN_VIEWPORT_H + 140);
    }

    /// 幂等:已在目标尺寸时返回 None(不重复 setWindowBounds)。
    #[test]
    fn headed_plan_is_idempotent() {
        let first = headed_window_plan(1920.0, 1080.0, 1512.0, 982.0, 140.0).unwrap();
        assert_eq!(headed_window_plan(first.0 as f64, first.1 as f64, 1512.0, 982.0, 140.0), None);
    }
}
