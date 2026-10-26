//! Chrome 真实鼠标通道(CDP `Input.dispatchMouseEvent`)。
//!
//! 背景(2026-10-26 百度地图实测):Chrome 工具的点击此前**一律**走 JS
//! `el.click()` 合成事件(全仓库无 `DispatchMouseEvent`),与已根治的 G1
//! (键盘输入只认可信事件流)是**同类问题** —— 重 JS 站点可能「点了但没反应」,
//! 表现为页面无任何变化、模型反复换 selector 空转。
//!
//! 本模块提供可信鼠标事件链(mouseMoved → mousePressed → mouseReleased),
//! 并对齐 Lsm 版的 `hover` / 双击 / 右键动作集(`browser/api.rs` 有、Chrome 侧缺)。
//!
//! 与 `type_text_real`(真实键盘通道)配套:输入走 `Input.insertText` + rawKeyDown,
//! 点击走 `Input.dispatchMouseEvent`,两者都产生 `isTrusted=true` 的事件。

use std::time::Duration;

use chromiumoxide::Page;
use serde_json::Value;

use crate::error::Result;

use super::page::{err, eval_js, timeout_err, CDP_BOUND_SECS};

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
};

/// 元素在视口中的中心点(CSS 像素);找不到或不可见(宽高为 0)返回 `None`。
///
/// 先 `scrollIntoView({behavior:'instant'})` 再量矩形 —— CDP 鼠标事件坐标是
/// **视口**坐标,元素若在视口外,坐标会落在错误位置。
pub(super) async fn element_center(page: &Page, selector: &str) -> Result<Option<(f64, f64)>> {
    let js = format!(
        r#"(function(){{
            var el = null;
            try {{ el = document.querySelector({sel}); }} catch (e) {{ return null; }}
            if (!el) return null;
            try {{ el.scrollIntoView({{block:'center', inline:'center', behavior:'instant'}}); }} catch (e) {{}}
            var r = el.getBoundingClientRect();
            if (!r || r.width <= 0 || r.height <= 0) return null;
            return JSON.stringify({{x: r.left + r.width / 2, y: r.top + r.height / 2}});
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let v = eval_js(page, &js).await?;
    let raw = v.as_str().unwrap_or("");
    if raw.is_empty() {
        return Ok(None);
    }
    let obj: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let x = obj.get("x").and_then(Value::as_f64);
    let y = obj.get("y").and_then(Value::as_f64);
    Ok(match (x, y) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    })
}

/// 派发一条 CDP 鼠标事件。
async fn dispatch_mouse(
    page: &Page,
    kind: DispatchMouseEventType,
    x: f64,
    y: f64,
    button: Option<MouseButton>,
    clicks: i64,
) -> Result<()> {
    let mut builder = DispatchMouseEventParams::builder()
        .r#type(kind)
        .x(x)
        .y(y)
        .click_count(clicks)
        .buttons(if clicks > 0 { 1 } else { 0 });
    if let Some(b) = button {
        builder = builder.button(b);
    }
    let params = builder.build().map_err(|e| err("dispatch_mouse", e))?;
    tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.execute(params))
        .await
        .map_err(|_| timeout_err("dispatch_mouse", CDP_BOUND_SECS))?
        .map_err(|e| err("dispatch_mouse", e))?;
    Ok(())
}

/// 真实鼠标左键单击(移动 → 按下 → 抬起)。
///
/// 返回 `{hit, mode:"cdp_mouse", selector, x, y}`;未命中返回 `hit:false` 与
/// 可操作提示(先 `inspect(info="elements")` 找可见元素)。
pub async fn click_real(page: &Page, selector: &str) -> Result<Value> {
    let Some((x, y)) = element_center(page, selector).await? else {
        return Ok(serde_json::json!({
            "hit": false, "mode": "cdp_mouse", "selector": selector,
            "hint": "选择器未命中或元素不可见(宽高为 0);先用 inspect(info=\"elements\", params.text=...) 找可见元素",
        }));
    };
    dispatch_mouse(page, DispatchMouseEventType::MouseMoved, x, y, None, 0).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    dispatch_mouse(page, DispatchMouseEventType::MousePressed, x, y, Some(MouseButton::Left), 1).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    dispatch_mouse(page, DispatchMouseEventType::MouseReleased, x, y, Some(MouseButton::Left), 1).await?;
    Ok(serde_json::json!({
        "hit": true, "mode": "cdp_mouse", "selector": selector,
        "x": x, "y": y, "button": "left", "clicks": 1,
    }))
}

/// 真实鼠标右键 / 双击。
///
/// `kind`:`right` / `double`(与 Lsm 版 `click_kind` 命名一致)。
pub async fn click_kind_real(page: &Page, selector: &str, kind: &str) -> Result<Value> {
    let (button, clicks) = match kind {
        "right" => (MouseButton::Right, 1),
        _ => (MouseButton::Left, 2),
    };
    let Some((x, y)) = element_center(page, selector).await? else {
        return Ok(serde_json::json!({
            "hit": false, "mode": "cdp_mouse", "selector": selector, "kind": kind,
            "hint": "选择器未命中或元素不可见(宽高为 0);先用 inspect(info=\"elements\") 找可见元素",
        }));
    };
    dispatch_mouse(page, DispatchMouseEventType::MouseMoved, x, y, None, 0).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    dispatch_mouse(page, DispatchMouseEventType::MousePressed, x, y, Some(button.clone()), clicks).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    dispatch_mouse(page, DispatchMouseEventType::MouseReleased, x, y, Some(button), clicks).await?;
    Ok(serde_json::json!({
        "hit": true, "mode": "cdp_mouse", "selector": selector,
        "x": x, "y": y, "kind": kind, "clicks": clicks,
    }))
}

/// 真实鼠标悬停(mouseMoved 到元素中心)。
///
/// 对齐 Lsm 版 `control(hover)`:常用于触发 tooltip / 懒加载菜单。
pub async fn hover_real(page: &Page, selector: &str) -> Result<Value> {
    let Some((x, y)) = element_center(page, selector).await? else {
        return Ok(serde_json::json!({
            "hit": false, "mode": "cdp_mouse", "selector": selector,
            "hint": "选择器未命中或元素不可见(宽高为 0);先用 inspect(info=\"elements\") 找可见元素",
        }));
    };
    dispatch_mouse(page, DispatchMouseEventType::MouseMoved, x, y, None, 0).await?;
    Ok(serde_json::json!({
        "hit": true, "mode": "cdp_mouse", "selector": selector, "x": x, "y": y,
    }))
}

/// 在已知视口坐标上派发一次真实左键单击(供「按文字定位」复用)。
pub async fn click_at(page: &Page, x: f64, y: f64) -> Result<()> {
    dispatch_mouse(page, DispatchMouseEventType::MouseMoved, x, y, None, 0).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    dispatch_mouse(page, DispatchMouseEventType::MousePressed, x, y, Some(MouseButton::Left), 1).await?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    dispatch_mouse(page, DispatchMouseEventType::MouseReleased, x, y, Some(MouseButton::Left), 1).await?;
    Ok(())
}
