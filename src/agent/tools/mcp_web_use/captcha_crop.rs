//! 人工介入附图的验证码元素裁剪采集（第 133 轮）。
//!
//! 第 132 轮 `reason=captcha` 自动附图为**整视口截图**，弹窗小窗缩放后验证码本体
//! 只剩几个像素，人工无法读码（2026-10-08 云智眼登录实测）。本模块把自动附图升级为
//! **元素级裁剪截图**：页面内 JS 探测验证码元素（img/canvas/svg 关键字命中 → 可见
//! canvas 兜底）→ `getBoundingClientRect` 取 CSS 像素 bbox → 外扩边距并钳制到视口
//! → CDP `Page.captureScreenshot` 传 `clip` 只截该区域。
//!
//! 降级链（fail-open，任何一级失败都不阻断提问本身）：
//! 元素裁剪失败 → 整视口截图（第 132 轮行为）→ 无图。
//!
//! `HitlImagePlan` / `hitl_image_plan` 自 `control.rs` 迁入本模块（其判定与采集
//! 同属附图决策链），`control.rs` 以 `pub use` 再导出保持外部路径零改动。

use serde_json::Value;

use super::{ensure_page, eval_js_string, now_millis_safe};

/// 人工介入弹窗附带截图的解析决策（纯函数，可单测；第 132 轮自 control.rs 迁入）。
///
/// 优先级：显式 `params.image_path`（文件存在才用）> `reason=captcha` 自动 CDP
/// 截图 > 无图。返回 `(计划, 来源标签)`；来源标签回传信封供对账。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitlImagePlan {
    /// 使用显式路径（已确认文件存在）。
    Explicit(String),
    /// reason=captcha 且未显式给图 → 自动截取（元素裁剪优先，视口兜底）。
    Auto,
    /// 不附图。
    None,
}

pub fn hitl_image_plan(reason: &str, explicit: Option<&str>) -> (HitlImagePlan, &'static str) {
    match explicit {
        Some(path) if !path.trim().is_empty() => {
            if std::path::Path::new(path.trim()).is_file() {
                (HitlImagePlan::Explicit(path.trim().to_string()), "explicit")
            } else {
                // 显式指定但文件不存在：忽略并标注，fail-open 不阻断提问
                (HitlImagePlan::None, "explicit_missing")
            }
        }
        _ if reason == "captcha" => (HitlImagePlan::Auto, "auto"),
        _ => (HitlImagePlan::None, "none"),
    }
}

/// 截图区域（CSS/DIP 像素，与 CDP `Page.Viewport` 同坐标系，无需 DPR 换算）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl ClipRect {
    /// 序列化为信封对账字段。
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "x": (self.x * 100.0).round() / 100.0,
            "y": (self.y * 100.0).round() / 100.0,
            "width": (self.w * 100.0).round() / 100.0,
            "height": (self.h * 100.0).round() / 100.0,
        })
    }
}

/// bbox 外扩边距（px）并钳制到视口内；退化（太小/完全不可见）返回 None。
///
/// 纯函数，可单测。坐标系：CSS 像素，原点在视口左上角，y 向下。
pub fn clamp_clip(x: f64, y: f64, w: f64, h: f64, vw: f64, vh: f64, pad: f64) -> Option<ClipRect> {
    if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() {
        return None;
    }
    if w < 8.0 || h < 8.0 || vw <= 0.0 || vh <= 0.0 {
        return None;
    }
    let nx = (x - pad).max(0.0);
    let ny = (y - pad).max(0.0);
    let nw = (x + w + pad).min(vw) - nx;
    let nh = (y + h + pad).min(vh) - ny;
    if nw < 8.0 || nh < 8.0 {
        return None;
    }
    Some(ClipRect { x: nx, y: ny, w: nw, h: nh })
}

/// 解析验证码元素探测 JS 的返回 → 合法 ClipRect（尺寸合理性校验后再钳制）。
///
/// 纯函数，可单测。`v` 为 null / 缺字段 / 尺寸非法时返回 None（调用方回退视口截图）。
pub fn parse_captcha_candidate(v: &Value, vw: f64, vh: f64) -> Option<ClipRect> {
    let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
    let (x, y, w, h) = (f("x"), f("y"), f("w"), f("h"));
    // 合理性：不小于 24×16（验证码最小可读本体），不超过视口（防误命中整页容器）
    if !(w >= 24.0 && h >= 16.0 && w <= vw.max(0.0) && h <= vh.max(0.0)) {
        return None;
    }
    clamp_clip(x, y, w, h, vw, vh, 12.0)
}

/// 验证码元素探测 JS（页面内一次 eval，返回最优候选 bbox 或 null）。
///
/// 两级策略：① `id/class/src/aria-label` 命中验证码关键字（captcha/verify/验证码/
/// slide/puzzle/geetest/yidun…）的可见 `img/canvas/svg`，取面积最大者；② 无关键字
/// 命中时兜底取视口内尺寸合理的可见 `canvas`（滑块验证码常见形态）。只查
/// img/canvas/svg 三类可渲染元素，且单元素尺寸不超过视口，从源头压住误裁剪风险。
const CAPTCHA_PROBE_JS: &str = r#"(() => {
  const KEY = /captcha|verify\s?code|verifycode|vcode|seccode|验证码|slide|slider|puzzle|geetest|yidun|nc_wrapper|rotate/i;
  const vw = window.innerWidth, vh = window.innerHeight;
  const rect = (el) => {
    if (!el || !el.getBoundingClientRect) return null;
    const r = el.getBoundingClientRect();
    if (r.width < 24 || r.height < 16) return null;
    if (r.bottom <= 0 || r.right <= 0 || r.top >= vh || r.left >= vw) return null;
    let st = null;
    try { st = window.getComputedStyle(el); } catch (e) { return null; }
    if (!st || st.visibility === 'hidden' || st.display === 'none' || Number(st.opacity) === 0) return null;
    return r;
  };
  const hit = (el) => {
    try {
      const id = el.id || '';
      const cls = (typeof el.className === 'string' ? el.className : (el.getAttribute && el.getAttribute('class')) || '');
      const src = (el.currentSrc || el.src || (el.getAttribute && el.getAttribute('src')) || '');
      const aria = (el.getAttribute && el.getAttribute('aria-label')) || '';
      return KEY.test(id) || KEY.test(cls) || KEY.test(src) || KEY.test(aria);
    } catch (e) { return false; }
  };
  const pack = (r, tag, how) => ({
    x: r.x, y: r.y, w: r.width, h: r.height,
    area: r.width * r.height, tag: tag, how: how,
  });
  const cands = [];
  let els = document.querySelectorAll('img,canvas,svg');
  for (let i = 0; i < els.length; i++) {
    if (!hit(els[i])) continue;
    const r = rect(els[i]);
    if (r && r.width <= vw && r.height <= vh) cands.push(pack(r, els[i].tagName.toLowerCase(), 'keyword'));
  }
  if (!cands.length) {
    els = document.querySelectorAll('canvas');
    for (let i = 0; i < els.length; i++) {
      const r = rect(els[i]);
      if (r && r.width >= 100 && r.width <= Math.min(vw, 800)
            && r.height >= 40 && r.height <= Math.min(vh, 600)) {
        cands.push(pack(r, 'canvas', 'fallback'));
      }
    }
  }
  if (!cands.length) return null;
  cands.sort((a, b) => b.area - a.area);
  const top = cands[0];
  return { x: top.x, y: top.y, w: top.w, h: top.h, tag: top.tag, how: top.how };
})()"#;

/// 自动截取验证码现场图为人工介入弹窗附图（第 133 轮：元素裁剪优先，视口兜底）。
///
/// CDP `CaptureScreenshot` 在浏览器进程内完成，**不受宿主录屏权限（TCC）影响**
/// —— 与 macOS Vision OCR 的权限路径不同，本机实测 OCR 因录屏权限不可用时，
/// 截图通道依然可用。返回 `(落盘路径, 模式标签, 裁剪区域)`：
/// - 模式 `auto_element`：成功裁剪验证码元素区域；
/// - 模式 `auto_viewport`：元素探测失败，回退第 132 轮整视口截图；
/// - `Err`：调用方 fail-open 为无图（`auto_failed`），不阻断提问。
pub async fn capture_hitl_screenshot(
    id: &str,
) -> std::result::Result<(String, &'static str, Option<ClipRect>), String> {
    let page = ensure_page(id).await?;

    // 第 143 轮:管控视觉避让 —— HITL 附图不能带蒙层遮罩/盾区/状态条(人工读
    // 验证码会被干扰)。request_human(unlock_page=true)路径此时已临时切 open,
    // 这里是 unlock_page=false 或 open 档下该通道仍干净的兜底;只动视觉层,
    // 不动 CDP 输入锁。
    let visuals_hidden = crate::agent::browser::BrowserManager::global()
        .guard_visuals_active()
        .await
        && crate::agent::browser_overlay::guard_suspend_visuals(&page, true).await;
    let r = capture_hitl_screenshot_inner(&page).await;
    if visuals_hidden {
        let _ = crate::agent::browser_overlay::guard_suspend_visuals(&page, false).await;
    }
    r
}

/// 实际采集(元素裁剪优先,视口兜底)。
async fn capture_hitl_screenshot_inner(
    page: &chromiumoxide::Page,
) -> std::result::Result<(String, &'static str, Option<ClipRect>), String> {
    // ① 元素裁剪通道：探测 bbox → clip 截图
    let viewport = eval_js_string(
        &page,
        "({w: window.innerWidth, h: window.innerHeight})",
    )
    .await
    .ok()
    .and_then(|v| {
        let w = v.get("w").and_then(Value::as_f64)?;
        let h = v.get("h").and_then(Value::as_f64)?;
        (w > 0.0 && h > 0.0).then_some((w, h))
    });
    if let Some((vw, vh)) = viewport {
        if let Ok(candidate) = eval_js_string(&page, CAPTCHA_PROBE_JS).await {
            if let Some(clip) = parse_captcha_candidate(&candidate, vw, vh) {
                if let Ok(path) =
                    save_clip_screenshot(&page, &clip, "laew_hitl_captcha").await
                {
                    return Ok((path, "auto_element", Some(clip)));
                }
            }
        }
    }

    // ② 视口兜底通道（第 132 轮行为）
    let bytes = page
        .screenshot(chromiumoxide::page::ScreenshotParams::builder().build())
        .await
        .map_err(|e| e.to_string())?;
    let path = std::env::temp_dir()
        .join(format!("laew_hitl_captcha_{}.png", now_millis_safe()));
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok((path.display().to_string(), "auto_viewport", None))
}

/// 按 ClipRect 区域截图落盘（DIP 坐标，scale=1）。
async fn save_clip_screenshot(
    page: &chromiumoxide::Page,
    clip: &ClipRect,
    stem: &str,
) -> std::result::Result<String, String> {
    use chromiumoxide::cdp::browser_protocol::page::Viewport;
    let params = chromiumoxide::page::ScreenshotParams::builder()
        .clip(Viewport {
            x: clip.x,
            y: clip.y,
            width: clip.w,
            height: clip.h,
            scale: 1.0,
        })
        .build();
    let bytes = page.screenshot(params).await.map_err(|e| e.to_string())?;
    let path = std::env::temp_dir().join(format!("{stem}_{}.png", now_millis_safe()));
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clamp_clip_padding_and_clamp() {
        // 视口内正常外扩
        let c = clamp_clip(100.0, 100.0, 200.0, 80.0, 1920.0, 1080.0, 12.0).unwrap();
        assert_eq!(c, ClipRect { x: 88.0, y: 88.0, w: 224.0, h: 104.0 });
        // 左上角贴边：外扩被 0 钳制
        let c2 = clamp_clip(0.0, 0.0, 100.0, 50.0, 1920.0, 1080.0, 12.0).unwrap();
        assert_eq!(c2, ClipRect { x: 0.0, y: 0.0, w: 112.0, h: 62.0 });
        // 右下角贴边：被视口钳制
        let c3 = clamp_clip(1900.0, 1060.0, 20.0, 20.0, 1920.0, 1080.0, 12.0).unwrap();
        assert!((c3.x - 1888.0).abs() < 1e-9);
        assert!((c3.w - 32.0).abs() < 1e-9);
        assert!((c3.h - 32.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_clip_degenerate() {
        assert!(clamp_clip(0.0, 0.0, 4.0, 100.0, 1920.0, 1080.0, 12.0).is_none());
        assert!(clamp_clip(0.0, 0.0, 100.0, 4.0, 1920.0, 1080.0, 12.0).is_none());
        assert!(clamp_clip(f64::NAN, 0.0, 100.0, 100.0, 1920.0, 1080.0, 12.0).is_none());
        assert!(clamp_clip(0.0, 0.0, 100.0, 100.0, 0.0, 1080.0, 12.0).is_none());
        // 完全在视口外：钳制后宽高退化
        assert!(clamp_clip(-500.0, 100.0, 100.0, 100.0, 1920.0, 1080.0, 12.0).is_none());
    }

    #[test]
    fn parse_candidate_size_gate() {
        let vw = 1920.0;
        let vh = 1080.0;
        // 正常候选
        let ok = parse_captcha_candidate(&json!({"x": 10, "y": 20, "w": 150, "h": 50}), vw, vh);
        assert!(ok.is_some());
        // null / 缺字段
        assert!(parse_captcha_candidate(&Value::Null, vw, vh).is_none());
        assert!(parse_captcha_candidate(&json!({"x": 10, "y": 20}), vw, vh).is_none());
        // 太小（噪点/图标）
        assert!(parse_captcha_candidate(&json!({"x": 10, "y": 20, "w": 10, "h": 50}), vw, vh).is_none());
        // 超过视口（误命中整页容器）
        assert!(parse_captcha_candidate(&json!({"x": 0, "y": 0, "w": 5000, "h": 50}), vw, vh).is_none());
        // 非数字字段
        assert!(parse_captcha_candidate(&json!({"x": "a", "y": 0, "w": 100, "h": 50}), vw, vh).is_none());
    }

    #[test]
    fn clip_rect_json_rounding() {
        let c = ClipRect { x: 1.23456, y: 0.0, w: 100.1, h: 50.555 };
        let j = c.to_json();
        assert_eq!(j["x"], 1.23);
        assert_eq!(j["width"], 100.1);
        assert_eq!(j["height"], 50.56);
    }

    #[test]
    fn hitl_image_plan_unchanged() {
        // 迁入后行为与第 132 轮一致（tests.rs 还有外部路径用例对账）
        let f = std::env::temp_dir().join("laew_hitl_plan_probe.txt");
        std::fs::write(&f, "x").unwrap();
        let (p, s) = hitl_image_plan("captcha", Some(f.to_str().unwrap()));
        assert!(matches!(p, HitlImagePlan::Explicit(_)));
        assert_eq!(s, "explicit");
        let _ = std::fs::remove_file(&f);
        assert_eq!(hitl_image_plan("captcha", None), (HitlImagePlan::Auto, "auto"));
        assert_eq!(hitl_image_plan("sms", None), (HitlImagePlan::None, "none"));
    }
}
