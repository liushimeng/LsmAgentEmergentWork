//! 自动化控制 API:`MCP_Web_Use(control, control_action=...)` 的内核实现。
//!
//! 所有动作在进程内完成:DOM 查询、表单序列化、点击/输入语义、滚动、JS 执行。
//! 需要网络的动作(链接跳转/表单提交)复用 [`super::LsmBrowserManager`] 的导航能力。

use serde_json::{json, Value};

use super::document::{self, Document};
use super::js::{self, JsPageMeta};
use super::script;
use super::{EngineError, EngineResult, LsmBrowserManager};
use crate::agent::browser::urlutil;

fn p_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn p_u64(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn p_f64(params: &Value, key: &str) -> Option<f64> {
    params.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

fn p_bool(params: &Value, key: &str, default: bool) -> bool {
    match params.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"),
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
        _ => default,
    }
}

fn require_selector<'a>(params: &'a Value) -> EngineResult<&'a str> {
    p_str(params, "selector").ok_or_else(|| EngineError::param("缺少 params.selector"))
}

fn nth(params: &Value) -> usize {
    p_u64(params, "nth").unwrap_or(0) as usize
}

fn timeout_ms(params: &Value, top: &Value, default: u64) -> u64 {
    p_u64(params, "timeout_ms")
        .or_else(|| p_u64(top, "timeout_ms"))
        .unwrap_or(default)
}

// ============ 阶段二:页面 JS 线程交互 ============

/// 若页面有 JS 线程,把事件分发到页面 listener。无线程/失败返回 None(并清理失效句柄)。
async fn js_dispatch(
    mgr: &LsmBrowserManager,
    pid: &str,
    selector: &str,
    event: &str,
) -> Option<script::JsReply> {
    let (handle, env, nodes) = {
        let inner = mgr.inner.lock().await;
        let tab = inner.tabs.get(pid)?;
        let handle = tab.js.clone()?;
        let mut env = script::build_env(
            &inner,
            &tab.url,
            &tab.title,
            &tab.user_agent,
            (tab.viewport.width, tab.viewport.height),
            &tab.session_storage,
            tab.security.get_csp(&tab.url),
        );
        env.scroll_x = tab.scroll_x;
        env.scroll_y = tab.scroll_y;
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc.snapshot_nodes_styled((tab.viewport.width, tab.viewport.height));
        (handle, env, nodes)
    };
    let sel = selector.to_string();
    let evt = event.to_string();
    let result = tokio::task::spawn_blocking(move || {
        handle.request(|tx| script::JsRequest::Dispatch {
            selector: sel,
            event: evt,
            env,
            nodes,
            reply: tx,
        })
    })
    .await;
    match result {
        Ok(Ok(reply)) => Some(reply),
        _ => {
            mgr.clear_js(pid).await;
            None
        }
    }
}

/// 应用 JS 回复:DOM 动作落到 `tab.html` 并重算布局;存储/Cookie/标题/滚动/台账合并。
/// 返回导航类待执行动作(click/navigate/submit/new_tab/back/forward/reload)。
async fn apply_js_reply(
    mgr: &LsmBrowserManager,
    pid: &str,
    reply: &script::JsReply,
) -> Vec<script::JsAction> {
    let mut inner = mgr.inner.lock().await;
    let (tab_url, tab_html) = {
        let Some(tab) = inner.tabs.get(pid) else {
            return Vec::new();
        };
        (tab.url.clone(), tab.html.clone())
    };
    // DOM 句柄不可跨 await(非 Send):DOM 应用与序列化放在同步辅助函数内完成。
    let (final_html, summary) =
        apply_dom_actions(&tab_html, &tab_url, &reply.outcome.actions);
    for line in &summary.cookies {
        inner
            .cookies
            .set_from_headers(&tab_url, &[("set-cookie".to_string(), line.clone())]);
    }
    let origin = urlutil::storage_origin(&tab_url);
    for (k, v) in &summary.storage_local {
        inner
            .local_storage
            .entry(origin.clone())
            .or_default()
            .insert(k.clone(), v.clone());
    }
    for k in &summary.storage_local_remove {
        if let Some(m) = inner.local_storage.get_mut(&origin) {
            m.remove(k);
        }
    }
    if summary.storage_local_clear {
        if let Some(m) = inner.local_storage.get_mut(&origin) {
            m.clear();
        }
    }
    let Some(t) = inner.tabs.get_mut(pid) else {
        return Vec::new();
    };
    // session 存储合并
    for (k, v) in &summary.storage_session {
        t.session_storage.insert(k.clone(), v.clone());
    }
    for k in &summary.storage_session_remove {
        t.session_storage.remove(k);
    }
    if summary.storage_session_clear {
        t.session_storage.clear();
    }
    t.html = final_html.clone();
    if let Some(title) = &summary.title {
        t.title = title.clone();
    }
    for (k, v) in summary.form_values {
        t.form_values.insert(k, v);
    }
    if let Some(s) = summary.scroll_to {
        t.scroll_y = s.max(0.0);
    }
    if let Some(s) = summary.scroll_by {
        t.scroll_y = (t.scroll_y + s).max(0.0);
    }
    if let Some(f) = summary.focus {
        t.focused = Some(f);
    }
    if summary.blur {
        t.focused = None;
    }
    for log in &reply.outcome.logs {
        if t.console.len() > 400 {
            t.console.remove(0);
        }
        t.console.push(super::ConsoleRecord {
            level: log.level.clone(),
            text: log.text.clone(),
            at_ms: super::now_ms(),
        });
    }
    for rec in &reply.network {
        if t.network.len() > 200 {
            t.network.remove(0);
        }
        t.network.push(super::NetworkRecord {
            url: rec.url.clone(),
            method: if rec.method.is_empty() {
                "FETCH".to_string()
            } else {
                rec.method.clone()
            },
            status: rec.status,
            content_type: rec.content_type.clone(),
            bytes: rec.bytes,
            elapsed_ms: rec.elapsed_ms,
            redirect: rec.redirect,
            from_cache: false,
            at_ms: super::now_ms(),
        });
    }
    // 阶段三第三步:补齐脚本新增的图片并重排(JS 动态插入 <img> 也能出图)。
    let (page_url, user_agent, existing_images) = {
        let Some(tab) = inner.tabs.get(pid) else {
            return Vec::new();
        };
        (tab.url.clone(), tab.user_agent.clone(), tab.images.clone())
    };
    let images_out = super::images::load_images(
        &mut inner,
        &mgr.http,
        &final_html,
        &page_url,
        &user_agent,
        15_000,
        &existing_images,
    )
    .await;
    if let Some(tab) = inner.tabs.get_mut(pid) {
        tab.images = images_out.store;
        let dom = Document::parse(&tab.html, &tab.url);
        let media = super::style::MediaCtx::new(tab.viewport.width as f64, 720.0);
        let sizes = super::images::ImageSizes::from_store(&tab.images);
        tab.layout = super::layout::compute_full(
            &dom,
            tab.viewport.width as f64,
            mgr.font(),
            &media,
            &sizes,
        );
        // 有头:JS 动作应用后重排完成,推帧给查看器。
        if tab.mode.is_headed() {
            mgr.push_frame(tab);
        }
        for rec in images_out.network {
            if tab.network.len() > 200 {
                tab.network.remove(0);
            }
            tab.network.push(super::NetworkRecord {
                url: rec.url,
                method: rec.method,
                status: rec.status,
                content_type: rec.content_type,
                bytes: rec.bytes,
                elapsed_ms: rec.elapsed_ms,
                redirect: rec.redirect,
                from_cache: false,
                at_ms: super::now_ms(),
            });
        }
        for log in images_out.console {
            if tab.console.len() > 400 {
                tab.console.remove(0);
            }
            tab.console.push(super::ConsoleRecord {
                level: log.level,
                text: log.text,
                at_ms: super::now_ms(),
            });
        }
    }
    reply
        .outcome
        .actions
        .iter()
        .filter(|a| {
            matches!(
                a.kind.as_str(),
                "click" | "navigate" | "new_tab" | "submit" | "back" | "forward" | "reload"
            )
        })
        .cloned()
        .collect()
}

/// 应用 DOM 动作并序列化(同步辅助函数)。
///
/// DOM 句柄(`Rc` 树)不可跨 await 存活,故把"解析 → 应用 → 序列化"整体收在
/// 同步函数内,调用方只拿纯数据(`String` + `ApplySummary`)。
fn apply_dom_actions(
    html: &str,
    base_url: &str,
    actions: &[script::JsAction],
) -> (String, script::dom_ops::ApplySummary) {
    let doc = Document::parse(html, base_url);
    let summary = script::dom_ops::apply(&doc, actions, base_url);
    let final_html = doc.serialize_full();
    (final_html, summary)
}

/// eval_js 走页面 JS 线程(持久全局)。线程不可用时返回 None,由调用方回退快照桥。
async fn js_eval_thread(
    mgr: &LsmBrowserManager,
    pid: &str,
    script_text: &str,
) -> Option<script::JsReply> {
    if !script::scripts_enabled() {
        return None;
    }
    {
        // 懒启动:页面无脚本时首次 eval_js 建线程,获得持久会话。
        let mut inner = mgr.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            if tab.js.is_none() {
                tab.js = Some(script::JsHandle::spawn());
            }
        }
    }
    let (handle, env, nodes) = {
        let inner = mgr.inner.lock().await;
        let tab = inner.tabs.get(pid)?;
        let handle = tab.js.clone()?;
        let mut env = script::build_env(
            &inner,
            &tab.url,
            &tab.title,
            &tab.user_agent,
            (tab.viewport.width, tab.viewport.height),
            &tab.session_storage,
            tab.security.get_csp(&tab.url),
        );
        env.scroll_x = tab.scroll_x;
        env.scroll_y = tab.scroll_y;
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc.snapshot_nodes_styled((tab.viewport.width, tab.viewport.height));
        (handle, env, nodes)
    };
    let script_text = script_text.to_string();
    let result = tokio::task::spawn_blocking(move || {
        handle.request(|tx| script::JsRequest::Eval {
            script: script_text,
            env,
            nodes,
            reply: tx,
        })
    })
    .await;
    match result {
        Ok(Ok(reply)) => Some(reply),
        _ => {
            mgr.clear_js(pid).await;
            None
        }
    }
}

/// 执行一个 control_action。
pub async fn control(
    mgr: &LsmBrowserManager,
    page_id: Option<&str>,
    action: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    let pid = page_id.unwrap_or("");
    match action {
        "click" | "human_click" => act_click(mgr, pid, params, action == "human_click").await,
        "right_click" => act_click_kind(mgr, pid, params, "right").await,
        "double_click" => act_click_kind(mgr, pid, params, "double").await,
        "hover" => act_hover(mgr, pid, params).await,
        "mouse_move" => act_mouse_move(mgr, pid, params).await,
        "input_text" | "human_input" => act_input(mgr, pid, params).await,
        "clear_input" => act_clear(mgr, pid, params).await,
        "select_option" => act_select_option(mgr, pid, params).await,
        "select_dropdown" => act_select_dropdown(mgr, pid, params).await,
        "key_press" => act_key_press(mgr, pid, params, top).await,
        "press_sequence" => act_press_sequence(mgr, pid, params, top).await,
        "scroll" => act_scroll(mgr, pid, params).await,
        "scroll_to" => act_scroll_to(mgr, pid, params).await,
        "scroll_to_text" => act_scroll_to_text(mgr, pid, params).await,
        "navigate" => {
            require_page(pid)?;
            let url = p_str(params, "url")
                .or_else(|| p_str(top, "url"))
                .ok_or_else(|| EngineError::param("navigate 需要 params.url"))?;
            let t = timeout_ms(params, top, 60_000);
            let nav = mgr.navigate(pid, url, t).await?;
            Ok(nav_json(&nav))
        }
        "back" | "forward" | "reload" => {
            require_page(pid)?;
            let nav = mgr.history_nav(pid, action, timeout_ms(params, top, 60_000)).await?;
            Ok(nav_json(&nav))
        }
        "new_tab" => {
            require_page(pid)?;
            let url = p_str(params, "url").ok_or_else(|| EngineError::param("new_tab 需要 params.url"))?;
            let out = mgr.new_tab(pid, url, timeout_ms(params, top, 60_000)).await?;
            Ok(json!({
                "opened": true,
                "page_id": out.page_id,
                "spawned_page_id": out.page_id,
                "title": out.title,
                "final_url": out.final_url,
            }))
        }
        "new_window" => {
            let url = p_str(params, "url").ok_or_else(|| EngineError::param("new_window 需要 params.url"))?;
            let out = mgr
                .new_page(
                    url,
                    super::BrowserMode::Hidden,
                    None,
                    None,
                    None,
                    true,
                    false,
                    timeout_ms(params, top, 60_000),
                )
                .await?;
            Ok(json!({
                "opened": true,
                "page_id": out.page_id,
                "spawned_page_id": out.page_id,
                "title": out.title,
                "final_url": out.final_url,
                "window_id": out.window_id,
            }))
        }
        "close_tab" => {
            require_page(pid)?;
            let out = mgr.close_page(pid).await;
            Ok(json!({"closed": out.closed.contains(&pid.to_string())}))
        }
        "bring_to_front" => {
            require_page(pid)?;
            Ok(json!({"brought_to_front": mgr.bring_to_front(pid).await}))
        }
        "wait" => act_wait(mgr, pid, params, top).await,
        "eval_js" => act_eval(mgr, pid, params, top).await,
        "set_cookie" => {
            let name = p_str(params, "name").ok_or_else(|| EngineError::param("set_cookie 需要 name"))?;
            let value = params.get("value").and_then(Value::as_str).unwrap_or("");
            mgr.set_cookie(
                if pid.is_empty() { None } else { Some(pid) },
                name,
                value,
                p_str(params, "domain"),
                p_str(params, "path"),
                p_str(params, "url"),
            )
            .await
        }
        "delete_cookie" => {
            let name = p_str(params, "name").ok_or_else(|| EngineError::param("delete_cookie 需要 name"))?;
            require_page(pid)?;
            mgr.delete_cookie(Some(pid), name, p_str(params, "domain")).await
        }
        "set_storage" => {
            require_page(pid)?;
            let kind = if p_str(params, "kind") == Some("session") { "session" } else { "local" };
            let items = storage_items(params)?;
            mgr.storage_set(pid, kind, &items).await
        }
        "clear_storage" => {
            require_page(pid)?;
            let kind = if p_str(params, "kind") == Some("session") { "session" } else { "local" };
            let keys: Vec<String> = params
                .get("keys")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            mgr.storage_clear(pid, kind, &keys).await
        }
        "set_viewport" => {
            require_page(pid)?;
            let width = p_u64(params, "width").ok_or_else(|| EngineError::param("set_viewport 需要 width"))? as u32;
            let height = p_u64(params, "height").ok_or_else(|| EngineError::param("set_viewport 需要 height"))? as u32;
            mgr.set_viewport(pid, width, height).await
        }
        "screenshot" => {
            require_page(pid)?;
            mgr.screenshot(pid, params).await
        }
        "download" => {
            require_page(pid)?;
            mgr.download(pid, params).await
        }
        "set_window" => {
            require_page(pid)?;
            let mut inner = mgr.inner.lock().await;
            let tab = inner
                .tabs
                .get_mut(pid)
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            if let Some(w) = p_u64(params, "width") {
                tab.viewport.width = (w as u32).clamp(320, 7680);
            }
            if let Some(h) = p_u64(params, "height") {
                tab.viewport.height = (h as u32).clamp(240, 4320);
            }
            let doc = Document::parse(&tab.html, &tab.url);
            // 阶段三第三步:视口变化复用已解码图片的固有尺寸重排。
            let media = super::style::MediaCtx::new(tab.viewport.width as f64, 720.0);
            let sizes = super::images::ImageSizes::from_store(&tab.images);
            tab.layout = super::layout::compute_full(
                &doc,
                tab.viewport.width as f64,
                mgr.font(),
                &media,
                &sizes,
            );
            // 有头:视口/窗口变更后重排完成,推帧给查看器。
            if tab.mode.is_headed() {
                mgr.push_frame(tab);
            }
            Ok(json!({
                "width": tab.viewport.width,
                "height": tab.viewport.height,
                "left": p_u64(params, "left"),
                "top": p_u64(params, "top"),
                "window_state": p_str(params, "window_state").unwrap_or("normal"),
                "note": "LsmAgentBrowser 阶段一为逻辑窗口,原生窗口在阶段三实现",
            }))
        }
        "sync_viewport" => {
            require_page(pid)?;
            super::tab_view(mgr, pid).await.map(|(_, _, _, _, vp, _, _)| {
                json!({"width": vp.width, "height": vp.height, "synced": true})
            })
        }
        "set_highlight" => {
            require_page(pid)?;
            let enabled = p_bool(params, "enabled", true);
            let mut inner = mgr.inner.lock().await;
            let tab = inner
                .tabs
                .get_mut(pid)
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            tab.highlight = enabled;
            Ok(json!({"highlight": enabled, "supported": false, "note": "阶段一无原生窗口,高亮不绘制"}))
        }
        "set_guard" => act_set_guard(mgr, pid, params).await,
        "set_overlay" => {
            require_page(pid)?;
            let enabled = p_bool(params, "enabled", true);
            act_set_guard(
                mgr,
                pid,
                &json!({"mode": if enabled { "locked" } else { "open" }}),
            )
            .await
        }
        "auto_follow" => {
            require_page(pid)?;
            let enabled = p_bool(params, "enabled", true);
            let mut inner = mgr.inner.lock().await;
            let tab = inner
                .tabs
                .get_mut(pid)
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            tab.auto_follow = enabled;
            if p_bool(params, "scroll_to_bottom", false) {
                tab.scroll_y = tab.layout.page_height();
            }
            Ok(json!({"auto_follow": enabled, "visibility": "visible", "note": "阶段一无窗口滚动跟随"}))
        }
        "set_stealth" => Ok(json!({
            "mode": p_str(params, "mode").unwrap_or("off"),
            "applied": false,
            "note": "LsmAgentBrowser 原生内核不注入任何伪装脚本;该参数仅为兼容保留",
        })),
        "heartbeat" => {
            require_page(pid)?;
            let summary = mgr
                .page_summary(pid)
                .await
                .ok_or_else(|| EngineError::page_not_found(pid))?;
            Ok(json!({"ok": true, "title": summary.title}))
        }
        "focus" | "blur" => act_focus(mgr, pid, params, action).await,
        "dispatch_event" => act_dispatch_event(mgr, pid, params, top).await,
        "upload_file" => act_upload(mgr, pid, params).await,
        "drag" => act_drag(mgr, pid, params).await,
        other => Err(EngineError::param(format!("未知 control_action:{other}"))),
    }
}

fn require_page(pid: &str) -> EngineResult<()> {
    if pid.is_empty() {
        Err(EngineError::param("缺少 page_id"))
    } else {
        Ok(())
    }
}

fn storage_items(params: &Value) -> EngineResult<Vec<(String, String)>> {
    if let Some(obj) = params.get("items").and_then(Value::as_object) {
        return Ok(obj
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()),
                )
            })
            .collect());
    }
    if let Some(key) = p_str(params, "key") {
        let value = params
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        return Ok(vec![(key.to_string(), value)]);
    }
    Err(EngineError::param("set_storage 需要 items 或 key/value"))
}

fn nav_json(nav: &super::NavOutcome) -> Value {
    json!({
        "page_id": nav.page_id,
        "url": nav.url,
        "requested_url": nav.requested_url,
        "final_url": nav.url,
        "title": nav.title,
        "status": nav.status,
        "redirected": nav.redirect_chain.len() > 1,
        "redirect_chain": nav.redirect_chain,
        "content_type": nav.content_type,
        "byte_size": nav.source_bytes,
    })
}

enum NavPlan {
    Same { url: String, method: String, body: Option<Vec<u8>> },
    NewTab { url: String },
}

async fn act_click(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    human: bool,
) -> EngineResult<Value> {
    require_page(pid)?;
    click_inner(mgr, pid, params, if human { "human" } else { "click" }).await
}

async fn act_click_kind(mgr: &LsmBrowserManager, pid: &str, params: &Value, kind: &str) -> EngineResult<Value> {
    require_page(pid)?;
    click_inner(mgr, pid, params, kind).await
}

async fn click_inner(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    kind: &str,
) -> EngineResult<Value> {
    let selector = require_selector(params)?;
    let idx = nth(params);
    let timeout = timeout_ms(params, params, 60_000);
    let (plan, rect, stable, toggled, laid_out) = {
        let inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc
            .select(selector)
            .map_err(EngineError::action_failed)?;
        let node = nodes.get(idx).ok_or_else(|| {
            EngineError::action_failed(format!("选择器未命中或索引越界:{selector}[{idx}]"))
        })?;
        let stable = Document::stable_selector(node);
        let rect_opt = tab.layout.rect_for(&stable);
        let rect = rect_opt.unwrap_or((0.0, 0.0, 0.0, 0.0));
        let tag = document::tag_name(node);
        let typ = document::attr(node, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut toggled = None;
        if tag == "input" && matches!(typ.as_str(), "checkbox" | "radio") {
            let cur = tab
                .form_values
                .get(&stable)
                .map(|v| v == "true")
                .unwrap_or_else(|| document::attr(node, "checked").is_some());
            toggled = Some((stable.clone(), (!cur).to_string()));
        }
        let mut plan = None;
        if toggled.is_none() && !matches!(kind, "right" | "double") {
            if let Some(href) = document::attr(node, "href") {
                if let Some(url) = urlutil::resolve(&tab.url, &href) {
                    let blank = document::attr(node, "target")
                        .map(|t| t.eq_ignore_ascii_case("_blank"))
                        .unwrap_or(false);
                    plan = Some(if blank {
                        NavPlan::NewTab { url }
                    } else {
                        NavPlan::Same {
                            url,
                            method: "GET".to_string(),
                            body: None,
                        }
                    });
                }
            } else if matches!(tag.as_str(), "button" | "input")
                || typ == "submit"
                || document::attr(node, "role").as_deref() == Some("button")
            {
                if let Some(form) = doc.form_request(node, &tab.form_values) {
                    let (url, body) = form.to_request();
                    plan = Some(NavPlan::Same {
                        url,
                        method: form.method,
                        body,
                    });
                }
            }
        }
        (plan, rect, stable, toggled, rect_opt.is_some())
    };

    if let Some((sel, value)) = toggled {
        let mut inner = mgr.inner.lock().await;
        if let Some(tab) = inner.tabs.get_mut(pid) {
            tab.form_values.insert(sel, value);
        }
        return Ok(json!({
            "clicked": true, "x": rect.0, "y": rect.1, "mode": kind,
            "selector": stable, "toggled": true, "laid_out": laid_out,
        }));
    }

    let mut out = json!({
        "clicked": true,
        "x": rect.0,
        "y": rect.1,
        "mode": kind,
        "selector": stable,
        "laid_out": laid_out,
    });
    // 阶段二:页面 listener 分发。preventDefault 或 JS 产生导航动作时优先按 JS 语义执行;
    // listener 未拦截时继续默认行为(handler 与默认导航并存,贴近真实浏览器)。
    if !matches!(kind, "right" | "double" | "js") {
        if let Some(reply) = js_dispatch(mgr, pid, &stable, "click").await {
            let pending = apply_js_reply(mgr, pid, &reply).await;
            if reply.handled {
                out["js_handled"] = json!(true);
            }
            if reply.default_prevented {
                out["default_prevented"] = json!(true);
                return Ok(out);
            }
            if let Some(action) = pending.into_iter().next() {
                let nav_result: EngineResult<Option<(String, String)>> = match action.kind.as_str() {
                    "navigate" => mgr.navigate(pid, &action.url, timeout).await.map(|n| Some((n.url, n.title))),
                    "new_tab" => mgr.new_tab(pid, &action.url, timeout).await.map(|n| Some((n.final_url, n.title))),
                    "back" | "forward" | "reload" => mgr.history_nav(pid, &action.kind, timeout).await.map(|n| Some((n.url, n.title))),
                    "click" | "submit" => {
                        let p = json!({"selector": action.selector});
                        // Box::pin 打断 async 递归(click 动作 → click_inner → 再分发)。
                        Box::pin(click_inner(mgr, pid, &p, "js")).await?;
                        Ok(None)
                    }
                    _ => Ok(None),
                };
                match nav_result {
                    Ok(Some((url, title))) => {
                        out["final_url"] = json!(url);
                        out["title"] = json!(title);
                        return Ok(out);
                    }
                    Ok(None) => return Ok(out),
                    Err(e) => return Err(e),
                }
            }
        }
    }
    match plan {
        Some(NavPlan::NewTab { url }) => {
            let spawned = mgr.new_tab(pid, &url, timeout).await?;
            out["spawned_page_id"] = json!(spawned.page_id);
            out["final_url"] = json!(spawned.final_url);
            out["title"] = json!(spawned.title);
        }
        Some(NavPlan::Same { url, method, body }) => {
            let nav = mgr.submit_form(pid, &url, &method, body, timeout).await?;
            out["final_url"] = json!(nav.url);
            out["title"] = json!(nav.title);
        }
        None => {}
    }
    Ok(out)
}

async fn act_hover(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("选择器未命中"))?;
    let stable = Document::stable_selector(node);
    let laid_out = tab.layout.rect_for(&stable).is_some();
    let rect = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
    Ok(json!({
        "hovered": true, "selector": stable, "x": rect.0, "y": rect.1,
        "laid_out": laid_out,
        "note": if laid_out { Value::Null } else { json!("元素无布局盒(可能被祖先 display:none 隐藏或为非寻址行内元素),坐标不可靠") },
    }))
}

async fn act_mouse_move(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let (x, y) = if let Some(sel) = p_str(params, "selector") {
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc.select(sel).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("选择器未命中"))?;
        let stable = Document::stable_selector(node);
        let r = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
        (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
    } else {
        (p_f64(params, "x").unwrap_or(0.0), p_f64(params, "y").unwrap_or(0.0))
    };
    tab.mouse = Some((x, y));
    Ok(json!({"x": x, "y": y, "note": "指针坐标为布局近似值(非真实设备像素)" }))
}

async fn act_input(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let text = params.get("text").and_then(Value::as_str).unwrap_or("");
    let stable = {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| {
            EngineError::action_failed(format!("输入目标未命中:{selector}"))
        })?;
        let stable = Document::stable_selector(node);
        if !document::is_visible(node) {
            return Err(EngineError::action_failed("输入目标不可见").with_data(json!({"selector": stable})));
        }
        tab.form_values.insert(stable.clone(), text.to_string());
        stable
    };
    // 阶段二:input + change 事件分发到页面 listener(锁外执行)。
    let mut js_events = false;
    for evt in ["input", "change"] {
        if let Some(reply) = js_dispatch(mgr, pid, &stable, evt).await {
            apply_js_reply(mgr, pid, &reply).await;
            js_events = js_events || reply.handled;
        }
    }
    Ok(json!({
        "selector": stable,
        "text": text,
        "mode": "native",
        "applied": true,
        "value_after": {"len": text.chars().count(), "matched": true, "head": text.chars().take(40).collect::<String>()},
        "fallbacks": 0,
        "js_events": js_events,
    }))
}

async fn act_clear(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("清空目标未命中"))?;
    let stable = Document::stable_selector(node);
    tab.form_values.insert(stable.clone(), String::new());
    Ok(json!({"cleared": true, "selector": stable, "len_after": 0}))
}

async fn act_select_option(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    // 非 Send 的 DOM 句柄全部收在块内,await 前销毁。
    let (select_selector, value, idx, label, mode) = {
        let mut inner = mgr.inner.lock().await;
        let tab = inner
            .tabs
            .get_mut(pid)
            .ok_or_else(|| EngineError::page_not_found(pid))?;
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("select 未命中"))?;
        let select = if document::tag_name(node) == "select" {
            node.clone()
        } else {
            document::closest_tag(node, "select")
                .ok_or_else(|| EngineError::action_failed("目标不是 select 且不在 select 内"))?
        };
        let select_selector = Document::stable_selector(&select);
        let options: Vec<_> = select
            .select("option")
            .map_err(|_| EngineError::action_failed("非法选择器"))?
            .into_iter()
            .collect();
    let value_filter = p_str(params, "value").map(str::to_string);
    let values_filter: Vec<String> = params
        .get("values")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let text_filter = p_str(params, "text").map(str::to_string);
    let index_filter = p_u64(params, "index").map(|v| v as usize);

    let mut chosen: Option<(String, usize, String)> = None;
    for (i, opt) in options.iter().enumerate() {
        let value = document::attr(opt.as_node(), "value")
            .unwrap_or_else(|| document::collapse_ws(&opt.text_contents()));
        let label = document::collapse_ws(&opt.text_contents());
        let hit = if let Some(v) = &value_filter {
            &value == v
        } else if !values_filter.is_empty() {
            values_filter.contains(&value)
        } else if let Some(t) = &text_filter {
            &label == t || label.contains(t.as_str())
        } else if let Some(idx) = index_filter {
            i == idx
        } else {
            false
        };
        if hit {
            chosen = Some((value, i, label));
            break;
        }
    }
    if value_filter.is_none() && values_filter.is_empty() && text_filter.is_none() && index_filter.is_none() {
        return Err(EngineError::param("select_option 需要 value/values/text/index 之一"));
    }
    let (value, idx, label) = chosen.ok_or_else(|| EngineError::action_failed("没有匹配的 option"))?;
        tab.form_values.insert(select_selector.clone(), value.clone());
        let mode = if value_filter.is_some() {
            "value"
        } else if text_filter.is_some() {
            "text"
        } else if index_filter.is_some() {
            "index"
        } else {
            "value"
        };
        (select_selector, value, idx, label, mode)
    };
    // 阶段二:change 事件分发到页面 listener。
    let mut js_events = false;
    if let Some(reply) = js_dispatch(mgr, pid, &select_selector, "change").await {
        apply_js_reply(mgr, pid, &reply).await;
        js_events = reply.handled;
    }
    Ok(json!({
        "selected": true,
        "selector": select_selector,
        "value": value,
        "text": label,
        "index": idx,
        "mode": mode,
        "js_events": js_events,
    }))
}

async fn act_select_dropdown(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let option_text = p_str(params, "option_text")
        .ok_or_else(|| EngineError::param("select_dropdown 需要 option_text"))?;
    let selector = p_str(params, "selector");
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let selects = if let Some(sel) = selector {
        doc.select(sel).map_err(EngineError::action_failed)?
    } else {
        doc.select("select").map_err(EngineError::action_failed)?
    };
    for sel_node in selects {
        let options = sel_node
            .select("option")
            .map_err(|_| EngineError::action_failed("非法选择器"))?;
        for opt in options {
            let label = document::collapse_ws(&opt.text_contents());
            if label == option_text || label.contains(option_text) {
                let value = document::attr(opt.as_node(), "value").unwrap_or_else(|| label.clone());
                let select_selector = Document::stable_selector(&sel_node);
                tab.form_values.insert(select_selector.clone(), value.clone());
                return Ok(json!({
                    "selected": true,
                    "option_text": option_text,
                    "value_after": value,
                    "selector": select_selector,
                    "mode": "custom_dropdown",
                    "note": "原生 select 已直接选中;非原生下拉在阶段二接入页面脚本后可展开",
                }));
            }
        }
    }
    Err(EngineError::action_failed(format!(
        "未找到包含「{option_text}」的下拉选项"
    )))
}

async fn act_key_press(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let key = p_str(params, "key").ok_or_else(|| EngineError::param("key_press 需要 key"))?;
    let selector = p_str(params, "selector");
    let timeout = timeout_ms(params, top, 60_000);
    if key.eq_ignore_ascii_case("enter") {
        if let Some((url, method, body)) = resolve_submit(mgr, pid, selector).await? {
            // 阶段二:submit 事件先分发,preventDefault 可拦截提交(冒泡命中 form listener)。
            if let Some(dispatch_sel) = selector {
                if let Some(reply) = js_dispatch(mgr, pid, dispatch_sel, "submit").await {
                    apply_js_reply(mgr, pid, &reply).await;
                    if reply.default_prevented {
                        return Ok(json!({
                            "key": key, "mode": "keyevent", "submitted": false,
                            "default_prevented": true,
                        }));
                    }
                }
            }
            let nav = mgr.submit_form(pid, &url, &method, body, timeout).await?;
            return Ok(json!({
                "key": key, "mode": "keyevent", "submitted": true,
                "final_url": nav.url, "title": nav.title,
            }));
        }
    }
    Ok(json!({"key": key, "mode": "keyevent", "submitted": false}))
}

async fn act_press_sequence(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let keys: Vec<String> = match params.get("keys") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(s)) => s.split_whitespace().map(str::to_string).collect(),
        _ => return Err(EngineError::param("press_sequence 需要 keys")),
    };
    let selector = p_str(params, "selector");
    let timeout = timeout_ms(params, top, 60_000);
    let mut submitted = false;
    for key in &keys {
        if key.eq_ignore_ascii_case("enter") {
            if let Some((url, method, body)) = resolve_submit(mgr, pid, selector).await? {
                if let Some(dispatch_sel) = selector {
                    if let Some(reply) = js_dispatch(mgr, pid, dispatch_sel, "submit").await {
                        apply_js_reply(mgr, pid, &reply).await;
                        if reply.default_prevented {
                            return Ok(json!({
                                "pressed": true, "keys": keys, "focused": true,
                                "submitted": false, "default_prevented": true,
                            }));
                        }
                    }
                }
                let nav = mgr.submit_form(pid, &url, &method, body, timeout).await?;
                submitted = true;
                return Ok(json!({
                    "pressed": true, "keys": keys, "focused": selector.is_some(),
                    "submitted": submitted, "final_url": nav.url,
                }));
            }
        }
    }
    Ok(json!({"pressed": true, "keys": keys, "focused": selector.is_some(), "submitted": submitted}))
}

async fn resolve_submit(
    mgr: &LsmBrowserManager,
    pid: &str,
    selector: Option<&str>,
) -> EngineResult<Option<(String, String, Option<Vec<u8>>)>> {
    let inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let target = if let Some(sel) = selector {
        doc.select(sel)
            .map_err(EngineError::action_failed)?
            .into_iter()
            .next()
    } else if let Some(focused) = &tab.focused {
        doc.select(focused)
            .ok()
            .and_then(|nodes| nodes.into_iter().next())
    } else {
        None
    };
    let Some(node) = target else {
        return Ok(None);
    };
    let form = doc.form_request(&node, &tab.form_values);
    Ok(form.map(|f| {
        let (url, body) = f.to_request();
        (url, f.method, body)
    }))
}

async fn act_scroll(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    if let Some(sel) = p_str(params, "selector") {
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc.select(sel).map_err(EngineError::action_failed)?;
        let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("滚动目标未命中"))?;
        let stable = Document::stable_selector(node);
        if let Some((_, y, _, h)) = tab.layout.rect_for(&stable) {
            tab.scroll_y = (y - tab.viewport.height as f64 / 3.0 + h / 2.0).max(0.0);
            if tab.mode.is_headed() {
                mgr.push_frame(tab);
            }
            return Ok(json!({"mode": "selector_into_view", "found": true, "selector": stable, "scrollY": tab.scroll_y}));
        }
        return Ok(json!({"mode": "selector_into_view", "found": false, "selector": stable}));
    }
    let dx = p_f64(params, "delta_x").unwrap_or(0.0);
    let dy = p_f64(params, "delta_y")
        .or_else(|| p_f64(params, "y"))
        .unwrap_or(0.0);
    tab.scroll_x = (tab.scroll_x + dx).max(0.0);
    tab.scroll_y = (tab.scroll_y + dy).max(0.0);
    if tab.mode.is_headed() {
        mgr.push_frame(tab);
    }
    Ok(json!({"mode": "scrollBy", "delta_x": dx, "delta_y": dy, "scrollX": tab.scroll_x, "scrollY": tab.scroll_y}))
}

async fn act_scroll_to(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("滚动目标未命中"))?;
    let stable = Document::stable_selector(node);
    let rect = tab.layout.rect_for(&stable);
    if let Some(r) = rect {
        tab.scroll_y = (r.1 - tab.viewport.height as f64 / 3.0).max(0.0);
    }
    if tab.mode.is_headed() {
        mgr.push_frame(tab);
    }
    Ok(json!({"selector": stable, "block": p_str(params, "block").unwrap_or("center"), "found": rect.is_some(), "scrollY": tab.scroll_y, "rect": rect.map(|r| json!({"x": r.0, "y": r.1, "width": r.2, "height": r.3}))}))
}

async fn act_scroll_to_text(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let text = p_str(params, "text").ok_or_else(|| EngineError::param("scroll_to_text 需要 text"))?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let node = doc
        .find_by_text(text)
        .ok_or_else(|| EngineError::action_failed(format!("页面文本未找到:{text}")))?;
    let stable = Document::stable_selector(&node);
    let rect = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
    tab.scroll_y = (rect.1 - tab.viewport.height as f64 / 3.0).max(0.0);
    Ok(json!({
        "text": text, "block": p_str(params, "block").unwrap_or("center"),
        "found": true, "tag": document::tag_name(&node), "selector": stable,
        "text_len": node.text_contents().chars().count(),
        "head": document::truncate_chars(&document::collapse_ws(&node.text_contents()), 120),
        "scrollY": tab.scroll_y,
    }))
}

async fn act_focus(mgr: &LsmBrowserManager, pid: &str, params: &Value, mode: &str) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("目标未命中"))?;
    let stable = Document::stable_selector(node);
    if mode == "focus" {
        tab.focused = Some(stable.clone());
    } else {
        tab.focused = None;
    }
    Ok(json!({"selector": stable, "mode": mode, "active": mode == "focus"}))
}

async fn act_dispatch_event(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let event = p_str(params, "event").ok_or_else(|| EngineError::param("dispatch_event 需要 event"))?;
    let ev = event.to_ascii_lowercase();
    if ev == "click" {
        let mut out = click_inner(mgr, pid, params, "event").await?;
        out["event"] = json!(event);
        out["dispatched"] = json!(true);
        return Ok(out);
    }
    if ev == "submit" {
        if let Some((url, method, body)) = resolve_submit(mgr, pid, Some(selector)).await? {
            if let Some(reply) = js_dispatch(mgr, pid, selector, "submit").await {
                apply_js_reply(mgr, pid, &reply).await;
                if reply.default_prevented {
                    return Ok(json!({"selector": selector, "event": event, "dispatched": true, "default_prevented": true}));
                }
            }
            let nav = mgr
                .submit_form(pid, &url, &method, body, timeout_ms(params, top, 60_000))
                .await?;
            return Ok(json!({"selector": selector, "event": event, "dispatched": true, "final_url": nav.url}));
        }
    }
    // 其余事件类型:优先分发到页面 listener(阶段二),无 listener 时保持记录语义。
    if let Some(reply) = js_dispatch(mgr, pid, selector, &ev).await {
        apply_js_reply(mgr, pid, &reply).await;
        return Ok(json!({
            "selector": selector, "event": event, "dispatched": true,
            "js_handled": reply.handled,
        }));
    }
    Ok(json!({"selector": selector, "event": event, "dispatched": true}))
}

async fn act_upload(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let selector = require_selector(params)?;
    let files: Vec<String> = match params.get("file_paths") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => p_str(params, "file_path").map(|s| vec![s.to_string()]).unwrap_or_default(),
    };
    if files.is_empty() {
        return Err(EngineError::param("upload_file 需要 file_paths 或 file_path"));
    }
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let nodes = doc.select(selector).map_err(EngineError::action_failed)?;
    let node = nodes.get(nth(params)).ok_or_else(|| EngineError::action_failed("上传控件未命中"))?;
    let stable = Document::stable_selector(node);
    tab.form_values.insert(stable, files.join(","));
    Ok(json!({"files": files, "count": files.len(), "note": "阶段一只记录文件路径;真实文件读取在阶段四接入"}))
}

async fn act_drag(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    let doc = Document::parse(&tab.html, &tab.url);
    let point = |sel_key: &str, x_key: &str, y_key: &str| -> Option<(f64, f64)> {
        if let Some(sel) = p_str(params, sel_key) {
            let nodes = doc.select(sel).ok()?;
            let node = nodes.first()?;
            let stable = Document::stable_selector(node);
            let r = tab.layout.rect_for(&stable).unwrap_or((0.0, 0.0, 0.0, 0.0));
            Some((r.0 + r.2 / 2.0, r.1 + r.3 / 2.0))
        } else {
            Some((p_f64(params, x_key)?, p_f64(params, y_key)?))
        }
    };
    let from = point("source_selector", "source_x", "source_y")
        .ok_or_else(|| EngineError::param("drag 需要 source_selector 或 source_x/source_y"))?;
    let to = point("target_selector", "target_x", "target_y")
        .ok_or_else(|| EngineError::param("drag 需要 target_selector 或 target_x/target_y"))?;
    Ok(json!({"dragged_from": [from.0, from.1], "to": [to.0, to.1], "note": "阶段一只记录拖拽轨迹"}))
}

async fn act_set_guard(mgr: &LsmBrowserManager, pid: &str, params: &Value) -> EngineResult<Value> {
    require_page(pid)?;
    let mode = p_str(params, "mode").unwrap_or("locked");
    if !matches!(mode, "locked" | "open" | "partial") {
        return Err(EngineError::param("guard.mode 允许 locked/open/partial"));
    }
    let allow: Vec<String> = params
        .get("allow_selectors")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let block: Vec<String> = params
        .get("block_selectors")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if mode == "partial" && allow.is_empty() && block.is_empty() {
        return Err(EngineError::param("partial 需要 allow_selectors 或 block_selectors"));
    }
    let mut inner = mgr.inner.lock().await;
    let tab = inner
        .tabs
        .get_mut(pid)
        .ok_or_else(|| EngineError::page_not_found(pid))?;
    tab.guard = super::GuardState {
        mode: mode.to_string(),
        allow_selectors: allow.clone(),
        block_selectors: block.clone(),
        note: p_str(params, "note").map(str::to_string),
    };
    Ok(json!({
        "mode": mode,
        "allow_selectors": allow,
        "block_selectors": block,
        "note": p_str(params, "note"),
        "supported": false,
        "hint": "阶段一无原生窗口;管控状态仅记录,不影响进程内 DOM 自动化",
    }))
}

async fn act_wait(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    if let Some(selector) = p_str(params, "selector") {
        let total = timeout_ms(params, top, 30_000).min(120_000);
        let state = p_str(params, "state").unwrap_or("visible");
        let started = std::time::Instant::now();
        loop {
            let found = {
                let inner = mgr.inner.lock().await;
                let Some(tab) = inner.tabs.get(pid) else {
                    return Err(EngineError::page_not_found(pid));
                };
                let doc = Document::parse(&tab.html, &tab.url);
                match doc.select(selector) {
                    Ok(nodes) => nodes.first().map(|n| {
                        state != "visible" || state == "attached" || document::is_visible(n)
                    }),
                    Err(e) => return Err(EngineError::action_failed(e)),
                }
            };
            if found == Some(true) {
                return Ok(json!({"waited_for": selector, "state": state, "elapsed_ms": started.elapsed().as_millis() as u64}));
            }
            if started.elapsed().as_millis() as u64 >= total {
                return Err(EngineError::action_failed(format!("等待元素超时:{selector}"))
                    .with_data(json!({"elapsed_ms": started.elapsed().as_millis() as u64})));
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    let ms = p_u64(params, "duration_ms")
        .or_else(|| p_u64(params, "ms"))
        .or_else(|| p_u64(params, "wait_ms"))
        .unwrap_or(500)
        .min(30_000);
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    Ok(json!({"slept_ms": ms}))
}

async fn act_eval(
    mgr: &LsmBrowserManager,
    pid: &str,
    params: &Value,
    top: &Value,
) -> EngineResult<Value> {
    require_page(pid)?;
    let script = ["expression", "script", "function", "js", "code"]
        .iter()
        .find_map(|k| p_str(params, k))
        .ok_or_else(|| EngineError::param("eval_js 需要 expression"))?
        .to_string();
    let await_promise = p_bool(params, "await_promise", false);
    let _ = timeout_ms(params, top, 30_000);

    // 阶段二:优先走页面 JS 线程(持久全局,DOM 修改落回文档);
    // 线程不可用(LAEW_WEB_SCRIPT=off / 超时)回退阶段一快照桥。
    if let Some(reply) = js_eval_thread(mgr, pid, &script).await {
        let pending = apply_js_reply(mgr, pid, &reply).await;
        return finish_eval(mgr, pid, reply.outcome, pending, await_promise).await;
    }

    let (outcome, pending) = {
        let mut inner = mgr.inner.lock().await;
        let some_tab = inner.tabs.get(pid).cloned();
        let Some(mut tab) = some_tab else {
            return Err(EngineError::page_not_found(pid));
        };
        let doc = Document::parse(&tab.html, &tab.url);
        let nodes = doc.snapshot_nodes_styled((tab.viewport.width, tab.viewport.height));
        let cookie_header = inner.cookies.header_for(&tab.url).unwrap_or_default();
        let empty = empty_map();
        let meta = JsPageMeta {
            url: &tab.url,
            title: &tab.title,
            cookies: &cookie_header,
            local_storage: inner
                .local_storage
                .get(&urlutil::storage_origin(&tab.url))
                .unwrap_or(&empty),
            session_storage: &tab.session_storage,
            viewport_width: tab.viewport.width,
            viewport_height: tab.viewport.height,
            scroll_x: tab.scroll_x,
            scroll_y: tab.scroll_y,
            user_agent: &tab.user_agent,
        };
        let outcome = js::eval(&script, &nodes, &meta).map_err(EngineError::action_failed)?;

        // 非导航动作直接落到标签页/存储。
        let mut pending: Vec<js::JsAction> = Vec::new();
        for action in &outcome.actions {
            match action.kind.as_str() {
                "set_value" => {
                    tab.form_values.insert(action.selector.clone(), action.value.clone());
                }
                "set_checked" => {
                    tab.form_values.insert(action.selector.clone(), action.value.clone());
                }
                "set_cookie" => {
                    if let Some(t) = inner.tabs.get(pid) {
                        let url = t.url.clone();
                        inner.cookies.set_from_headers(
                            &url,
                            &[("set-cookie".to_string(), action.value.clone())],
                        );
                    }
                }
                "storage_local" => {
                    let origin = urlutil::storage_origin(&tab.url);
                    inner
                        .local_storage
                        .entry(origin)
                        .or_default()
                        .insert(action.key.clone(), action.value.clone());
                }
                "storage_local_remove" => {
                    let origin = urlutil::storage_origin(&tab.url);
                    if let Some(m) = inner.local_storage.get_mut(&origin) {
                        m.remove(&action.key);
                    }
                }
                "storage_local_clear" => {
                    let origin = urlutil::storage_origin(&tab.url);
                    if let Some(m) = inner.local_storage.get_mut(&origin) {
                        m.clear();
                    }
                }
                "storage_session" => {
                    tab.session_storage.insert(action.key.clone(), action.value.clone());
                }
                "storage_session_remove" => {
                    tab.session_storage.remove(&action.key);
                }
                "storage_session_clear" => tab.session_storage.clear(),
                "set_title" => tab.title = action.value.clone(),
                "scroll" => {
                    if let Ok(v) = action.value.parse::<f64>() {
                        tab.scroll_y = v.max(0.0);
                    }
                }
                "scroll_by" => {
                    if let Ok(v) = action.value.parse::<f64>() {
                        tab.scroll_y = (tab.scroll_y + v).max(0.0);
                    }
                }
                "focus" => tab.focused = Some(action.selector.clone()),
                "blur" => tab.focused = None,
                "fetch" => {
                    tab.network.push(super::NetworkRecord {
                        url: action.url.clone(),
                        method: "FETCH".to_string(),
                        status: 0,
                        content_type: String::new(),
                        bytes: 0,
                        elapsed_ms: 0,
                        redirect: false,
                        from_cache: false,
                        at_ms: super::now_ms(),
                    });
                }
                "click" | "navigate" | "new_tab" | "back" | "forward" | "reload" | "submit" => {
                    pending.push(action.clone());
                }
                _ => {}
            }
        }
        // 把非导航状态写回。
        if let Some(t) = inner.tabs.get_mut(pid) {
            t.form_values = tab.form_values;
            t.session_storage = tab.session_storage;
            t.title = tab.title;
            t.scroll_y = tab.scroll_y;
            t.focused = tab.focused;
            if tab.network.len() > t.network.len() {
                t.network = tab.network;
            }
        }
        (outcome, pending)
    };

    // 导航/点击动作在锁外执行。
    finish_eval(mgr, pid, outcome, pending, await_promise).await
}

/// eval_js 收尾:执行导航类动作并组装响应信封。
async fn finish_eval(
    mgr: &LsmBrowserManager,
    pid: &str,
    outcome: js::JsOutcome,
    pending: Vec<js::JsAction>,
    await_promise: bool,
) -> EngineResult<Value> {
    let mut applied = 0usize;
    for action in &pending {
        let result: EngineResult<()> = match action.kind.as_str() {
            "navigate" => mgr.navigate(pid, &action.url, 60_000).await.map(|_| ()),
            "click" | "submit" => {
                let p = json!({"selector": action.selector});
                click_inner(mgr, pid, &p, "js").await.map(|_| ())
            }
            "new_tab" => mgr.new_tab(pid, &action.url, 60_000).await.map(|_| ()),
            "back" | "forward" | "reload" => mgr
                .history_nav(pid, action.kind.as_str(), 60_000)
                .await
                .map(|_| ()),
            _ => Ok(()),
        };
        if result.is_ok() {
            applied += 1;
        }
    }
    let mut data = json!({
        "result": outcome.value,
        "value_type": outcome.value_type,
        "await_promise": await_promise,
        "ok": outcome.ok,
        "actions_applied": applied,
        "console": outcome.logs.iter().map(|l| json!({"level": l.level, "text": l.text})).collect::<Vec<_>>(),
    });
    if let Some(err) = outcome.error {
        data["js_error"] = json!(err);
    }
    if outcome.truncated {
        data["truncated"] = json!(true);
        data["truncate_hint"] = json!("返回值超过 256KB,已截断;可拆分表达式或分页读取");
    }
    if await_promise {
        data["promise_note"] = json!("阶段二 JS 线程在脚本结束后驱动微任务;已 resolve 的 Promise/await 续体会执行");
    }
    Ok(data)
}

fn empty_map() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}
