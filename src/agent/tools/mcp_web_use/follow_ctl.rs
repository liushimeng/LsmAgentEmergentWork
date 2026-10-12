//! 滚动 / 视口 / 自动跟随 control_action 子模块(第 155 轮自 control.rs 机械拆分)。
//!
//! control.rs 已越 1800 行硬线,按 CLAUDE.md 拆分规范把「滚动与可视性」一族
//! (`act_scroll` / `act_scroll_to` / `act_set_viewport` / `act_sync_viewport` /
//! `act_auto_follow`)搬到职责子模块,逐行机械搬移零改写;本轮唯一的功能性改动是
//! `act_auto_follow` 的 `visibility_note` 接线第 155 轮新签名(补 `human_scrollable`
//! 参数,见 `browser_follow::visibility_note`)。

use super::*;
use super::control::dispatch_mouse;

pub(super) async fn act_scroll(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    if let Some(sel) = str_arg(p, "selector") {
        let js = format!(
            r#"(() => {{ const el = document.querySelector({js}); if(!el) return false; el.scrollIntoView(); return true; }})()"#,
            js = js_str(sel)
        );
        let ok = eval_js_string(&page, &js).await?;
        return Ok(json!({"mode":"selector_into_view","found": ok}));
    }
    let dx = p.get("delta_x").and_then(Value::as_f64).unwrap_or(0.0);
    let dy = p.get("delta_y").and_then(Value::as_f64).unwrap_or(0.0);
    if p.get("use_wheel").and_then(Value::as_bool).unwrap_or(false) {
        dispatch_mouse(&page, "mouseWheel", 0.0, 0.0, "left", 0, dx as i64, dy as i64).await?;
        Ok(json!({"mode":"wheel","delta_x":dx,"delta_y":dy}))
    } else {
        let js = format!("window.scrollBy({dx},{dy})");
        eval_js_string(&page, &js).await?;
        Ok(json!({"mode":"scrollBy","delta_x":dx,"delta_y":dy}))
    }
}

pub(super) async fn act_scroll_to(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let block = str_arg(p, "block").unwrap_or("center");
    let page = ensure_page(id).await?;
    let js = format!(
        r#"(() => {{ const el = document.querySelector({js}); if(!el) return false;
        el.scrollIntoView({{block: {blk}, behavior:'instant'}}); return true; }})()"#,
        js = js_str(sel), blk = js_str(block)
    );
    let ok = eval_js_string(&page, &js).await?;
    Ok(json!({"selector": sel, "block": block, "found": ok}))
}

pub(super) async fn act_set_viewport(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let w = p.get("width").and_then(Value::as_i64).ok_or_else(|| "缺 width".to_string())?;
    let h = p.get("height").and_then(Value::as_i64).ok_or_else(|| "缺 height".to_string())?;
    // 第 125 轮:与 open(window_width/height)同一安全区间,防极端值把渲染撑爆
    let w = w.clamp(320, 7680);
    let h = h.clamp(240, 4320);
    let dsf = p.get("device_scale_factor").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.5, 4.0);
    let mobile = p.get("mobile").and_then(Value::as_bool).unwrap_or(false);
    let page = ensure_page(id).await?;
    let params = chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams::builder()
        .width(w).height(h).device_scale_factor(dsf).mobile(mobile)
        .build().map_err(|e| e.to_string())?;
    page.execute(params).await.map_err(|e| e.to_string())?;
    Ok(json!({"width": w, "height": h, "device_scale_factor": dsf, "mobile": mobile,
              "note": "device metrics 覆盖持续生效直到 sync_viewport/set_window 清除;还原用 sync_viewport"}))
}

/// control_action=sync_viewport:人工拖动浏览器窗口大小后,清除 device metrics
/// 覆盖使视口自适应窗口内容区(消除黑边/缺区域/渲染不全)。
pub(super) async fn act_sync_viewport(id: &str, _: &Value) -> std::result::Result<Value, String> {
    crate::agent::browser::BrowserManager::global().sync_viewport(id).await
}

/// control_action=set_highlight:运行时开关 Agent 高亮蓝框。
/// 第 152 轮:`control(auto_follow)` —— 开关「自动跟随最新内容」。
///
/// 实测事故:豆包多轮对话时消息栈不断变长,Agent 读得到全部内容、人工在浏览器
/// 窗口只看得到第一屏;而 locked 档的 CDP 输入拦截连滚轮一起拦,人工也滚不动。
/// 跟随器只在**用户本来就在底部**时吸底(见 [`browser_follow::should_follow`]),
/// 人工往上翻历史时不会被拽走。
///
/// `params.scroll_to_bottom=true` 表示「不改变开关状态,立刻吸底一次」。
pub(super) async fn act_auto_follow(id: &str, p: &Value) -> std::result::Result<Value, String> {
    use crate::agent::browser_follow;
    if BrowserManager::global().page(id).await.is_none() {
        return Err("page_id 不存在".into());
    }
    // 第 155 轮:locked 档缺省「可滚动观看」—— 诊断提示按人工是否可自行滚动分流
    let human_scrollable = BrowserManager::global().overlay_active().await
        && crate::agent::browser_overlay::locked_scroll_watch_enabled();
    let just_scroll = p
        .get("scroll_to_bottom")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let Some(page) = BrowserManager::global().page(id).await else {
        return Err("page_id 不存在".into());
    };
    if just_scroll {
        let ok = browser_follow::scroll_to_bottom(&page).await;
        let vis = browser_follow::visibility(&page).await;
        return Ok(json!({
            "scrolled_to_bottom": ok,
            "visibility": vis,
            "note": browser_follow::visibility_note(&vis, human_scrollable),
        }));
    }
    let enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let applied = browser_follow::set(&page, enabled).await;
    let vis = browser_follow::visibility(&page).await;
    Ok(json!({
        "auto_follow": enabled,
        "applied": applied,
        "visibility": vis,
        "note": browser_follow::visibility_note(&vis, human_scrollable),
    }))
}
