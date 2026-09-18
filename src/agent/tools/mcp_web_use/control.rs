//! MCP_Web_Use action=control:全部写操作统一入口(自 tools/browser.rs 平移)。
//!
//! `control_action` 枚举对应原 BrowserControl 的 `action` 字段(34 个);
//! 动作参数集中在 `params` 对象;派生标签页经响应 `spawned_page_id` 回传。

use base64::Engine;
use serde_json::{json, Value};

use super::*;

/// control 分发入口。
pub(super) async fn run(args: Value) -> crate::error::Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    let Some(action) = str_arg(&args, "control_action") else {
        return envelope(1001, "缺少 control_action", json!({}));
    };
    let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
    let result: std::result::Result<Value, String> = match action {
        "click" => act_click(id, &params, false).await,
        "human_click" => act_click(id, &params, true).await,
        "right_click" => act_simple_click(id, &params, "right").await,
        "double_click" => act_simple_click(id, &params, "double").await,
        "hover" => act_hover(id, &params).await,
        "scroll" => act_scroll(id, &params).await,
        "scroll_to" => act_scroll_to(id, &params).await,
        "key_press" => act_key_press(id, &params).await,
        "press_sequence" => act_press_sequence(id, &params).await,
        "input_text" => act_input_text(id, &params).await,
        "human_input" => act_human_input(id, &params).await,
        "clear_input" => act_clear_input(id, &params).await,
        "upload_file" => act_upload_file(id, &params).await,
        "select_option" => act_select_option(id, &params).await,
        "new_tab" => act_new_tab(id, &params).await,
        "close_tab" => act_close_tab(id).await,
        "navigate" => act_navigate(id, &params).await,
        "back" | "forward" => act_history(id, action == "back").await,
        "reload" => act_reload(id, &params).await,
        "wait" => act_wait(id, &params).await,
        "eval_js" => act_eval_js(id, &params).await,
        "set_cookie" => act_set_cookie(id, &params).await,
        "delete_cookie" => act_delete_cookie(id, &params).await,
        "set_storage" => act_set_storage(id, &params).await,
        "clear_storage" => act_clear_storage(id, &params).await,
        "set_viewport" => act_set_viewport(id, &params).await,
        "screenshot" => act_screenshot(id, &params).await,
        "heartbeat" => act_heartbeat(id).await,
        // 第 76 轮扩展动作面 —— 拖拽 / 焦点 / 鼠标移动 / 自定义事件分发
        "drag" => act_drag(id, &params).await,
        "focus" => act_focus_or_blur(id, &params, true).await,
        "blur" => act_focus_or_blur(id, &params, false).await,
        "mouse_move" => act_mouse_move(id, &params).await,
        "dispatch_event" => act_dispatch_event(id, &params).await,
        other => return envelope(1001, "未知 control_action", json!({"control_action": other})),
    };
    let spawned = crate::agent::browser::BrowserManager::global().adopt_spawned_pages().await;
    match result {
        Ok(mut data) => {
            if let Some(new_id) = spawned.first() {
                if let Value::Object(ref mut m) = data {
                    m.insert("spawned_page_id".into(), Value::String(new_id.clone()));
                }
            }
            envelope(0, "ok", data)
        }
        Err(e) => {
            let mut data = json!({});
            if let Some(new_id) = spawned.first() {
                data["spawned_page_id"] = json!(new_id);
            }
            envelope(2002, &e, data)
        }
    }
}

// =================== control_action 实现(逐行平移自 tools/browser.rs) ===================

async fn act_click(id: &str, p: &Value, human: bool) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let nth = p.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
    let page = ensure_page(id).await?;
    let find = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        eval_find_center(&page, sel, nth),
    )
    .await
    .map_err(|_| "timeout".to_string())??;
    let x = find["x"].as_f64().unwrap_or_default();
    let y = find["y"].as_f64().unwrap_or_default();
    if human {
        dispatch_mouse(&page, "mouseMoved", x, y, "left", 0, 0, 0).await?;
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    dispatch_mouse(&page, "mousePressed", x, y, "left", 1, 0, 0).await?;
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    dispatch_mouse(&page, "mouseReleased", x, y, "left", 1, 0, 0).await?;
    Ok(json!({"clicked": sel, "x": x, "y": y, "mode": if human {"human"} else {"cdp"}}))
}

async fn act_simple_click(id: &str, p: &Value, kind: &str) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let nth = p.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
    let page = ensure_page(id).await?;
    let find = eval_find_center(&page, sel, nth).await?;
    let x = find["x"].as_f64().unwrap_or_default();
    let y = find["y"].as_f64().unwrap_or_default();
    let (button, clicks) = match kind {
        "right" => ("right", 1i64),
        "double" => ("left", 2i64),
        _ => ("left", 1),
    };
    dispatch_mouse(&page, "mousePressed", x, y, button, clicks, 0, 0).await?;
    dispatch_mouse(&page, "mouseReleased", x, y, button, clicks, 0, 0).await?;
    Ok(json!({"action": kind, "selector": sel, "x": x, "y": y}))
}

async fn act_hover(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let nth = p.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
    let page = ensure_page(id).await?;
    let find = eval_find_center(&page, sel, nth).await?;
    let x = find["x"].as_f64().unwrap_or_default();
    let y = find["y"].as_f64().unwrap_or_default();
    dispatch_mouse(&page, "mouseMoved", x, y, "left", 0, 0, 0).await?;
    Ok(json!({"hovered": sel}))
}

async fn act_scroll(id: &str, p: &Value) -> std::result::Result<Value, String> {
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

async fn act_scroll_to(id: &str, p: &Value) -> std::result::Result<Value, String> {
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

pub(super) fn parse_modifiers(p: &Value) -> i64 {
    let mut bit = 0i64;
    let arr: Vec<String> = if let Some(a) = p.get("modifiers").and_then(Value::as_array) {
        a.iter().filter_map(|v| v.as_str().map(|s| s.to_lowercase())).collect()
    } else if let Some(s) = str_arg(p, "modifiers") {
        s.split(',').map(|s| s.trim().to_lowercase()).collect()
    } else { Vec::new() };
    for m in arr {
        match m.as_str() {
            "alt" | "option" => bit |= 1,
            "ctrl" | "control" => bit |= 2,
            "meta" | "cmd" | "command" => bit |= 4,
            "shift" => bit |= 8,
            _ => {}
        }
    }
    bit
}

/// 单字符表:键名 → (CDP `key`, `code`, windowsVirtualKeyCode)。
pub(super) fn key_code_tuple(name: &str) -> Option<(&'static str, &'static str, i64)> {
    Some(match name {
        "Enter" | "Return" => ("Enter", "Enter", 13),
        "Tab" => ("Tab", "Tab", 9),
        "Escape" | "Esc" => ("Escape", "Escape", 27),
        "Backspace" => ("Backspace", "Backspace", 8),
        "Delete" | "Del" => ("Delete", "Delete", 46),
        "ArrowLeft" | "Left" => ("ArrowLeft", "ArrowLeft", 37),
        "ArrowUp" | "Up" => ("ArrowUp", "ArrowUp", 38),
        "ArrowRight" | "Right" => ("ArrowRight", "ArrowRight", 39),
        "ArrowDown" | "Down" => ("ArrowDown", "ArrowDown", 40),
        "Home" => ("Home", "Home", 36),
        "End"  => ("End", "End", 35),
        "PageUp" => ("PageUp", "PageUp", 33),
        "PageDown" => ("PageDown", "PageDown", 34),
        "Space" => (" ", "Space", 32),
        "F1" => ("F1", "F1", 112), "F2" => ("F2", "F2", 113), "F3" => ("F3", "F3", 114),
        "F4" => ("F4", "F4", 115), "F5" => ("F5", "F5", 116), "F6" => ("F6", "F6", 117),
        "F7" => ("F7", "F7", 118), "F8" => ("F8", "F8", 119), "F9" => ("F9", "F9", 120),
        "F10" => ("F10", "F10", 121), "F11" => ("F11", "F11", 122), "F12" => ("F12", "F12", 123),
        _ => return None,
    })
}

async fn act_key_press(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(k) = str_arg(p, "key") else { return Err("缺少 key".into()); };
    let mods = parse_modifiers(p);
    let page = ensure_page(id).await?;
    if let Some((key, code, vk)) = key_code_tuple(k) {
        let params = chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventParams::builder()
            .r#type(chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventType::KeyDown)
            .key(key).code(code)
            .windows_virtual_key_code(vk).native_virtual_key_code(vk)
            .modifiers(mods).build().map_err(|e| e.to_string())?;
        page.execute(params).await.map_err(|e| e.to_string())?;
        let params = chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventParams::builder()
            .r#type(chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventType::KeyUp)
            .key(key).code(code)
            .windows_virtual_key_code(vk).native_virtual_key_code(vk)
            .modifiers(mods).build().map_err(|e| e.to_string())?;
        page.execute(params).await.map_err(|e| e.to_string())?;
        Ok(json!({"key": key, "modifiers": mods, "mode":"keyevent"}))
    } else {
        let params = chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventParams::builder()
            .r#type(chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventType::Char)
            .text(k).key(k).modifiers(mods).build().map_err(|e| e.to_string())?;
        page.execute(params).await.map_err(|e| e.to_string())?;
        Ok(json!({"key": k, "modifiers": mods, "mode":"char"}))
    }
}

async fn act_press_sequence(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let keys_str = if let Some(arr) = p.get("keys").and_then(Value::as_array) {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        str_arg(p, "keys").unwrap_or("").to_string()
    };
    for token in keys_str.split_whitespace() {
        let (mods_part, key_part) = if let Some((m, k)) = token.split_once('+') {
            (m, k)
        } else { ("", token) };
        let mut sub = json!({"key": key_part});
        if !mods_part.is_empty() { sub["modifiers"] = json!(mods_part); }
        act_key_press(id, &sub).await?;
    }
    Ok(json!({"pressed": keys_str}))
}

async fn act_input_text(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let Some(text) = str_arg(p, "text") else { return Err("缺少 text".into()); };
    let use_js = p.get("use_js").and_then(Value::as_bool).unwrap_or(true);
    let page = ensure_page(id).await?;
    if use_js {
        let js = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return false;
                const desc = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value')
                    || Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value');
                desc.set.call(el, {txt});
                el.dispatchEvent(new Event('input', {{bubbles:true}}));
                el.dispatchEvent(new Event('change', {{bubbles:true}}));
                return true;
            }})()"#,
            sel = js_str(sel), txt = js_str(text)
        );
        let ok = eval_js_string(&page, &js).await?;
        Ok(json!({"selector": sel, "text": text, "mode":"js", "applied": ok}))
    } else {
        let el = page.find_element(sel.to_string()).await.map_err(|e| e.to_string())?;
        el.click().await.map_err(|e| e.to_string())?;
        el.type_str(text.to_string()).await.map_err(|e| e.to_string())?;
        Ok(json!({"selector": sel, "text": text, "mode":"sendkeys"}))
    }
}

async fn act_human_input(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let text = p.get("text").and_then(Value::as_str).unwrap_or("");
    let page = ensure_page(id).await?;
    if let Ok(el) = page.find_element(sel.to_string()).await {
        let _ = el.click().await;
    }
    for ch in text.chars() {
        let s = ch.to_string();
        let params = chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventParams::builder()
            .r#type(chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventType::Char)
            .text(s.clone()).key(s.clone()).build().map_err(|e| e.to_string())?;
        let _ = page.execute(params).await;
        let delay = 50 + (ch as u32 % 150);
        tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
    }
    Ok(json!({"selector": sel, "text": text, "mode":"human"}))
}

async fn act_clear_input(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let page = ensure_page(id).await?;
    let js = format!(
        r#"(() => {{ const el = document.querySelector({sel}); if(!el) return false;
        const d = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value')
              || Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value');
        d.set.call(el,''); el.dispatchEvent(new Event('input',{{bubbles:true}})); return true; }})()"#,
        sel = js_str(sel)
    );
    let ok = eval_js_string(&page, &js).await?;
    Ok(json!({"cleared": ok, "selector": sel}))
}

async fn act_upload_file(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let files: Vec<String> = if let Some(arr) = p.get("file_paths").and_then(Value::as_array) {
        arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
    } else if let Some(s) = str_arg(p, "file_path") { vec![s.to_string()] }
    else { return Err("缺少 file_paths".into()); };
    let page = ensure_page(id).await?;
    let doc = page
        .execute(chromiumoxide::cdp::browser_protocol::dom::GetDocumentParams::default())
        .await
        .map_err(|e| e.to_string())?;
    let root_id = doc.root.node_id;
    let sel_params =
        chromiumoxide::cdp::browser_protocol::dom::QuerySelectorParams::new(root_id, sel.to_string());
    let node = page.execute(sel_params).await.map_err(|e| e.to_string())?;
    let target_id = node.node_id;
    let mut params = chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams::new(files.clone());
    params.node_id = Some(target_id);
    page.execute(params).await.map_err(|e| e.to_string())?;
    Ok(json!({"files": files, "count": files.len()}))
}

async fn act_select_option(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let page = ensure_page(id).await?;
    // 四种选择方式(value/values/text/index)归一化为 JS 字面量(第 76 轮重写)。
    let value_js: String = if let Some(v) = p.get("value").and_then(Value::as_str) {
        js_str(v)
    } else if let Some(arr) = p.get("values").and_then(Value::as_array) {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| js_str(s)))
            .collect();
        format!("[{}]", parts.join(","))
    } else if let Some(t) = p.get("text").and_then(Value::as_str) {
        // 文本选择:在 JS 端遍历 <option>,找到 text 匹配的 index 后赋 value
        let js = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return false;
                const target = {txt};
                let found = -1;
                for (let i = 0; i < el.options.length; i++) {{
                    if (el.options[i].text === target) {{ found = i; break; }}
                }}
                if (found < 0) return false;
                el.selectedIndex = found;
                el.dispatchEvent(new Event('change',{{bubbles:true}}));
                return true;
            }})()"#,
            sel = js_str(sel),
            txt = js_str(t),
        );
        let ok = eval_js_string(&page, &js).await?;
        return Ok(json!({"selected": ok, "selector": sel, "mode": "text"}));
    } else if let Some(i) = p.get("index").and_then(Value::as_i64) {
        // 索引选择:在 JS 端直接设 selectedIndex
        let js = format!(
            r#"(() => {{
                const el = document.querySelector({sel});
                if (!el) return false;
                if ({i} < 0 || {i} >= el.options.length) return false;
                el.selectedIndex = {i};
                el.dispatchEvent(new Event('change',{{bubbles:true}}));
                return true;
            }})()"#,
            sel = js_str(sel),
            i = i,
        );
        let ok = eval_js_string(&page, &js).await?;
        return Ok(json!({"selected": ok, "selector": sel, "mode": "index", "index": i}));
    } else {
        return Err("缺 value/values/text/index 之一".into());
    };
    let js = format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) return false;
            el.value = {v};
            el.dispatchEvent(new Event('change',{{bubbles:true}}));
            return true;
        }})()"#,
        sel = js_str(sel), v = value_js
    );
    let ok = eval_js_string(&page, &js).await?;
    Ok(json!({"selected": ok, "selector": sel, "mode": "value"}))
}

async fn act_new_tab(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(url) = str_arg(p, "url") else { return Err("缺少 url".into()); };
    let page = ensure_page(id).await?;
    let js = format!("window.open({url}, '_blank')", url = js_str(url));
    let _ = eval_js_string(&page, &js).await;
    Ok(json!({"opened": url}))
}

async fn act_close_tab(id: &str) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let _ = eval_js_string(&page, "window.close()").await;
    Ok(json!({"closed": true}))
}

async fn act_navigate(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(url) = str_arg(p, "url") else { return Err("缺少 url".into()); };
    let page = ensure_page(id).await?;
    // navigate 超时 30s(第 66 轮),防止页面挂起导致无限等待
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        page.goto(url.to_string()),
    )
    .await
    .map_err(|_| format!("navigate 超时(30s): {url}"))?
    .map_err(|e| e.to_string())?;
    if let Some(ms) = p.get("wait_ms").and_then(Value::as_u64) {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
    Ok(json!({"url": url}))
}

async fn act_history(id: &str, back: bool) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let js = if back { "history.back()" } else { "history.forward()" };
    eval_js_string(&page, js).await?;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    Ok(json!({"nav": if back {"back"} else {"forward"}}))
}

async fn act_reload(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let mut params = chromiumoxide::cdp::browser_protocol::page::ReloadParams::default();
    if p.get("ignore_cache").and_then(Value::as_bool).unwrap_or(false) {
        params.ignore_cache = Some(true);
    }
    page.execute(params).await.map_err(|e| e.to_string())?;
    Ok(json!({"reloaded": true}))
}

async fn act_wait(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    if let Some(sel) = str_arg(p, "selector") {
        // wait 默认 30s 覆盖 AI 类网站(文心一言/ChatGPT/DeepSeek)首次响应长尾
        // (30-60s 常见,第 82 轮);timeout_ms 可覆盖,上限 120s 防止 hang 死。
        let timeout_ms = p
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(30_000)
            .min(120_000);
        let state = str_arg(p, "state").unwrap_or("visible");
        let js = format!(
            r#"async () => {{
                const sel = {sel}; const end = Date.now() + {timeout_ms};
                while (Date.now() < end) {{
                    const el = document.querySelector(sel);
                    if (el && ('{state}'==='attached' || ('{state}'==='visible' && el.offsetWidth>0) || ('{state}'==='hidden' && !el.offsetWidth))) {{
                        return true;
                    }}
                    await new Promise(r => setTimeout(r, 100));
                }}
                return false;
            }}()"#,
            sel = js_str(sel), timeout_ms = timeout_ms, state = state
        );
        let found = eval_js_string(&page, &js).await?;
        if found.as_bool() == Some(true) {
            return Ok(json!({"waited_for": sel, "state": state}));
        }
        return Err(format!("wait 超时({timeout_ms}ms): {sel} 未满足 {state}"));
    }
    let ms = p.get("duration_ms").and_then(Value::as_u64).unwrap_or(500).min(10000);
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    Ok(json!({"slept_ms": ms}))
}

async fn act_eval_js(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let expr = if let Some(e) = str_arg(p, "expression") { e.to_string() }
        else if let Some(b) = str_arg(p, "expression_b64") {
            let bytes = base64::engine::general_purpose::STANDARD.decode(b)
                .map_err(|e| format!("base64 解码失败: {e}"))?;
            String::from_utf8(bytes).map_err(|e| format!("utf8 解析失败: {e}"))?
        }
        else if let Some(s) = str_arg(p, "script") { s.to_string() }
        else if let Some(b) = str_arg(p, "script_b64") {
            let bytes = base64::engine::general_purpose::STANDARD.decode(b)
                .map_err(|e| format!("base64 解码失败: {e}"))?;
            String::from_utf8(bytes).map_err(|e| format!("utf8 解析失败: {e}"))?
        }
        else { return Err("缺 expression/expression_b64/script/script_b64".into()); };
    let await_promise = p.get("await_promise").and_then(Value::as_bool).unwrap_or(false);
    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
    let mut b = EvaluateParams::builder()
        .expression(expr.clone())
        .return_by_value(true);
    if await_promise { b = b.await_promise(true); }
    let v = page.evaluate(b.build().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?;
    let val = v.value().cloned().unwrap_or(Value::Null);
    Ok(json!({"result": val, "await_promise": await_promise}))
}

async fn act_set_cookie(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(name) = str_arg(p, "name") else { return Err("缺少 name".into()); };
    let Some(value) = str_arg(p, "value") else { return Err("缺少 value".into()); };
    let page = ensure_page(id).await?;
    let mut b = chromiumoxide::cdp::browser_protocol::network::SetCookieParams::builder()
        .name(name).value(value);
    if let Some(u) = str_arg(p, "url") { b = b.url(u.to_string()); }
    if let Some(d) = str_arg(p, "domain") { b = b.domain(d.to_string()); }
    if let Some(p) = str_arg(p, "path") { b = b.path(p.to_string()); }
    page.execute(b.build().map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
    Ok(json!({"name": name, "value": value}))
}

async fn act_delete_cookie(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(name) = str_arg(p, "name") else { return Err("缺少 name".into()); };
    let page = ensure_page(id).await?;
    let mut params = chromiumoxide::cdp::browser_protocol::network::DeleteCookiesParams::new(name);
    if let Some(d) = str_arg(p, "domain") { params.domain = Some(d.to_string()); }
    if let Some(u) = str_arg(p, "url") { params.url = Some(u.to_string()); }
    page.execute(params).await.map_err(|e| e.to_string())?;
    Ok(json!({"name": name}))
}

async fn act_set_storage(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let kind = str_arg(p, "kind").unwrap_or("local");
    let page = ensure_page(id).await?;
    let (items_js, written) = if let Some(items) = p.get("items").and_then(Value::as_object) {
        let parts: Vec<String> = items.iter()
            .map(|(k, v)| format!("[{},{}]",
                js_str(k),
                serde_json::to_string(v).unwrap_or_else(|_| "null".into())))
            .collect();
        (format!("Object.fromEntries([{}])", parts.join(",")), items.len())
    } else if let (Some(k), Some(v)) = (str_arg(p, "key"), p.get("value")) {
        (format!("Object.fromEntries([[{},{}]])",
            js_str(k),
            serde_json::to_string(v).unwrap_or_else(|_| "null".into())), 1)
    } else { return Err("缺 items 或 key/value".into()); };
    let storage_obj = if kind == "session" { "sessionStorage" } else { "localStorage" };
    let js = format!(
        r#"(() => {{ const s={storage}; const items={items};
            for (const [k,v] of Object.entries(items)) {{ s.setItem(k, typeof v==='string'?v:JSON.stringify(v)); }}
            const snap={{}}; for (let i=0;i<s.length;i++) {{ const kk=s.key(i); snap[kk]=s.getItem(kk); }} return {{written:{written}, snapshot:snap}}; }})()"#,
        storage = storage_obj, items = items_js, written = written
    );
    let r = eval_js_string(&page, &js).await?;
    Ok(json!({"kind": kind, "written": written, "snapshot": r}))
}

async fn act_clear_storage(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let kind = str_arg(p, "kind").unwrap_or("local");
    let page = ensure_page(id).await?;
    let keys_js = if let Some(arr) = p.get("keys").and_then(Value::as_array) {
        let list = arr.iter().filter_map(|v| v.as_str().map(|s| js_str(s))).collect::<Vec<_>>().join(",");
        format!("[{}]", list)
    } else { "null".into() };
    let storage_obj = if kind == "session" { "sessionStorage" } else { "localStorage" };
    let js = format!(
        r#"(() => {{ const s={storage};
            const ks={keys};
            if (ks === null) s.clear(); else for (const k of ks) s.removeItem(k);
            return {{cleared:true, length:s.length}}; }})()"#,
        storage = storage_obj, keys = keys_js
    );
    let r = eval_js_string(&page, &js).await?;
    Ok(json!({"kind": kind, "result": r}))
}

async fn act_set_viewport(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let w = p.get("width").and_then(Value::as_i64).ok_or_else(|| "缺 width".to_string())?;
    let h = p.get("height").and_then(Value::as_i64).ok_or_else(|| "缺 height".to_string())?;
    let dsf = p.get("device_scale_factor").and_then(Value::as_f64).unwrap_or(1.0);
    let mobile = p.get("mobile").and_then(Value::as_bool).unwrap_or(false);
    let page = ensure_page(id).await?;
    let params = chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams::builder()
        .width(w).height(h).device_scale_factor(dsf).mobile(mobile)
        .build().map_err(|e| e.to_string())?;
    page.execute(params).await.map_err(|e| e.to_string())?;
    Ok(json!({"width": w, "height": h, "device_scale_factor": dsf, "mobile": mobile}))
}

/// 截图(control 与 inspect 的 screenshot 共用)。
pub(super) async fn act_screenshot(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let mut b = chromiumoxide::page::ScreenshotParams::builder();
    let full_page = p.get("full_page").and_then(Value::as_bool).unwrap_or(false);
    if full_page {
        b = b.full_page(true).capture_beyond_viewport(true);
    }
    let format_str = str_arg(p, "format").unwrap_or("png");
    if format_str.eq_ignore_ascii_case("jpeg") || format_str.eq_ignore_ascii_case("jpg") {
        let q = p.get("quality").and_then(Value::as_i64).unwrap_or(80);
        b = b.format(chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Jpeg).quality(q);
    }
    let bytes = page.screenshot(b.build()).await.map_err(|e| e.to_string())?;
    let save_path = p.get("save_path").and_then(Value::as_str).map(|s| s.to_string());
    if let Some(path) = save_path {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
        return Ok(json!({"save_path": path, "byte_size": bytes.len(), "format": format_str}));
    }
    const AUTO_SAVE_THRESHOLD: usize = 200 * 1024;
    if bytes.len() > AUTO_SAVE_THRESHOLD {
        let path = std::env::temp_dir().join(format!("laew_web_{}.png", now_millis_safe()));
        tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
        return Ok(json!({"save_path": path.display().to_string(), "byte_size": bytes.len(), "auto_saved": true}));
    }
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(json!({"image_base64": b64, "byte_size": bytes.len(), "format": format_str}))
}

async fn act_heartbeat(id: &str) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let title = page.get_title().await.ok().flatten().unwrap_or_default();
    let _ = crate::agent::browser::BrowserManager::global().events(id).await;
    Ok(json!({"ok": true, "title": title}))
}

// =================== 第 76 轮扩展动作:拖拽 / 焦点 / 鼠标移动 / 事件分发 ===================

/// 拖拽:从源坐标 → 目标坐标(支持 source_selector 或 source_xy,target_selector 或 target_xy)。
/// 用于 HTML5 拖拽(滑块/排序)、文件拖拽、列表重排等场景。
async fn act_drag(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    // 源坐标
    let src = if let Some(sel) = str_arg(p, "source_selector") {
        let nth = p.get("source_nth").and_then(Value::as_u64).unwrap_or(0) as usize;
        eval_find_center(&page, sel, nth).await?
    } else {
        json!({
            "x": p.get("source_x").and_then(Value::as_f64).unwrap_or(0.0),
            "y": p.get("source_y").and_then(Value::as_f64).unwrap_or(0.0),
        })
    };
    // 目标坐标
    let dst = if let Some(sel) = str_arg(p, "target_selector") {
        let nth = p.get("target_nth").and_then(Value::as_u64).unwrap_or(0) as usize;
        eval_find_center(&page, sel, nth).await?
    } else {
        json!({
            "x": p.get("target_x").and_then(Value::as_f64).unwrap_or(0.0),
            "y": p.get("target_y").and_then(Value::as_f64).unwrap_or(0.0),
        })
    };
    let sx = src["x"].as_f64().unwrap_or_default();
    let sy = src["y"].as_f64().unwrap_or_default();
    let dx = dst["x"].as_f64().unwrap_or_default();
    let dy = dst["y"].as_f64().unwrap_or_default();
    // mousePressed → 多次 mouseMoved(平滑)→ mouseReleased
    dispatch_mouse(&page, "mouseMoved", sx, sy, "left", 0, 0, 0).await?;
    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    dispatch_mouse(&page, "mousePressed", sx, sy, "left", 1, 0, 0).await?;
    // 8 步插值,每步 15ms
    for step in 1..=8 {
        let t = step as f64 / 8.0;
        let mx = sx + (dx - sx) * t;
        let my = sy + (dy - sy) * t;
        dispatch_mouse(&page, "mouseMoved", mx, my, "left", 0, 0, 0).await?;
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
    dispatch_mouse(&page, "mouseReleased", dx, dy, "left", 1, 0, 0).await?;
    Ok(json!({"dragged_from":[sx, sy], "to":[dx, dy]}))
}

/// 焦点控制:focus=true 时 .focus(),false 时 .blur()。用于主动聚焦输入框、
/// 主动失焦(关闭下拉/隐藏 tooltip)、JS 受控组件等场景。
async fn act_focus_or_blur(id: &str, p: &Value, focus: bool) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let page = ensure_page(id).await?;
    let js = format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) return false;
            if ({focus}) el.focus(); else el.blur();
            return document.activeElement === el;
        }})()"#,
        sel = js_str(sel),
        focus = focus,
    );
    let ok = eval_js_string(&page, &js).await?;
    Ok(json!({"selector": sel, "mode": if focus {"focus"} else {"blur"}, "active": ok}))
}

/// 鼠标纯移动(不点击):用于悬停菜单、tooltip 触发、长按场景。
async fn act_mouse_move(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let x = if let Some(sel) = str_arg(p, "selector") {
        let nth = p.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
        let find = eval_find_center(&page, sel, nth).await?;
        find["x"].as_f64().unwrap_or_default()
    } else {
        p.get("x").and_then(Value::as_f64).unwrap_or_default()
    };
    let y = if let Some(sel) = str_arg(p, "selector") {
        let nth = p.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
        let find = eval_find_center(&page, sel, nth).await?;
        find["y"].as_f64().unwrap_or_default()
    } else {
        p.get("y").and_then(Value::as_f64).unwrap_or_default()
    };
    dispatch_mouse(&page, "mouseMoved", x, y, "left", 0, 0, 0).await?;
    Ok(json!({"x": x, "y": y}))
}

/// 分发自定义 DOM 事件:用于绕过被劫持的 input/change 监听,或触发受控组件的合成事件。
/// event 枚举:input / change / click / focus / blur / submit / keydown / keyup / mousedown / mouseup。
async fn act_dispatch_event(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let Some(event) = str_arg(p, "event") else { return Err("缺少 event".into()); };
    // 白名单事件,防止注入恶意事件名
    const ALLOWED: &[&str] = &[
        "input","change","click","focus","blur","submit","keydown","keyup",
        "mousedown","mouseup","mousemove","mouseenter","mouseleave","dblclick",
        "contextmenu","wheel","pointerdown","pointerup","pointermove",
    ];
    if !ALLOWED.contains(&event) {
        return Err(format!("不支持的事件:{event}(允许:{ALLOWED:?})"));
    }
    let page = ensure_page(id).await?;
    let js = format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) return false;
            el.dispatchEvent(new Event({evt}, {{bubbles:true, cancelable:true}}));
            return true;
        }})()"#,
        sel = js_str(sel),
        evt = js_str(event),
    );
    let ok = eval_js_string(&page, &js).await?;
    Ok(json!({"selector": sel, "event": event, "dispatched": ok}))
}

// —— mouse 派发辅助 ——
pub(super) async fn dispatch_mouse(
    page: &chromiumoxide::Page,
    type_: &str,
    x: f64,
    y: f64,
    button: &str,
    click_count: i64,
    delta_x: i64,
    delta_y: i64,
) -> std::result::Result<(), String> {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
    };
    let t = match type_ {
        "mousePressed" => DispatchMouseEventType::MousePressed,
        "mouseReleased" => DispatchMouseEventType::MouseReleased,
        "mouseMoved" => DispatchMouseEventType::MouseMoved,
        "mouseWheel" => DispatchMouseEventType::MouseWheel,
        _ => return Err(format!("unknown mouse type: {type_}")),
    };
    let b = match button {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::None,
    };
    let is_wheel = matches!(t, DispatchMouseEventType::MouseWheel);
    let mut params = DispatchMouseEventParams::builder().r#type(t).x(x).y(y);
    if !matches!(b, MouseButton::None) {
        let buttons = if is_wheel { 0 } else { 1 };
        params = params.button(b).buttons(buttons);
    }
    if click_count > 0 { params = params.click_count(click_count); }
    if is_wheel {
        params = params.delta_x(delta_x as f64).delta_y(delta_y as f64);
    }
    page.execute(params.build().map_err(|e| e.to_string())?).await.map_err(|e| e.to_string())?;
    Ok(())
}
