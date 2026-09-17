//! Chromium-WebUse 工具层(第 11 角色,5 个 Tool)。
//!
//! 工具面收敛为「写/读」对偶(移植自 go-web-debug-tool):
//! - `BrowserNew` / `BrowserList` / `BrowserClose`:生命周期管理
//! - `BrowserControl(page_id, action, params)`:全部写操作统一入口
//! - `BrowserInspect(page_id, info, params)`:全部只读观察统一入口
//!
//! 所有工具返回统一 JSON 信封 `{code,message,data}`(0 成功,1001 参数错误,
//! 2000 page_id 失效,2001 断连,2002 动作失败,2003 页面崩溃,3001 未检测到浏览器)
//! —— Agent 据错误码做机械决策(2000 → BrowserList 重新同步;2002 → 换 selector/路径重试)。
//!
//! 设计见 `docs/浏览器CDP工具/04-Chromium-WebUse-Agent设计与解决方案.md`。

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use super::Tool;
use crate::agent::browser::{BrowserManager, NO_BROWSER_SENTINEL};
use crate::error::Result;

fn envelope(code: i32, message: &str, data: Value) -> Result<String> {
    Ok(json!({"code": code, "message": message, "data": data}).to_string())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// 把字符串安全嵌入 JS 字面量(转义 `\`, `'`, 控制字符)。
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn now_millis_safe() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// 调 JS 并以 returnByValue 提取结果。
async fn eval_js_string(
    page: &chromiumoxide::Page,
    js: &str,
) -> std::result::Result<Value, String> {
    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
    let params = EvaluateParams::builder()
        .expression(js.to_string())
        .return_by_value(true)
        .build()
        .map_err(|e| e.to_string())?;
    let v = page.evaluate(params).await.map_err(|e| e.to_string())?;
    Ok(v.value().cloned().unwrap_or(Value::Null))
}

/// 找元素中心点 (x,y,w,h,visible);不存在返回 Err("not_found")。
async fn eval_find_center(
    page: &chromiumoxide::Page,
    selector: &str,
    nth: usize,
) -> std::result::Result<Value, String> {
    let sel = js_str(selector);
    let js = format!(
        r#"(() => {{
            const els = document.querySelectorAll({sel});
            const el = els[{nth}];
            if (!el) return null;
            try {{ el.scrollIntoView({{block:'center'}}); }} catch(e) {{}}
            const r = el.getBoundingClientRect();
            return {{x: r.x + r.width/2, y: r.y + r.height/2, w: r.width, h: r.height,
                    visible: r.width>0 && r.height>0}};
        }})()"#,
    );
    eval_js_string(page, &js).await
        .and_then(|v| if v.is_null() { Err("not_found".into()) } else { Ok(v) })
}

async fn ensure_page(id: &str) -> std::result::Result<chromiumoxide::Page, String> {
    BrowserManager::global().page(id).await.ok_or_else(|| "page_id 不存在".into())
}

// =================== BrowserNew ===================

pub struct BrowserNewTool;

#[async_trait]
impl Tool for BrowserNewTool {
    fn name(&self) -> &str {
        "BrowserNew"
    }
    fn description(&self) -> &str {
        "启动/接管 Chromium 并打开一个页面。默认纯 CDP 嵌入式无头浏览器(无可见窗口),\
         也可通过 connect_url 接管已用 --remote-debugging-port 启动的浏览器。\
         返回 {page_id,title,final_url,mode,next_steps}。未检测到浏览器返回 code=3001。\
         mode 枚举:hidden(默认,纯 CDP 无窗口)/new_headless/headed(显式开窗)。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "url":{"type":"string","description":"必填,目标网址"},
                "mode":{"type":"string","enum":["hidden","new_headless","headed"],"default":"hidden",
                    "description":"浏览器模式;hidden=纯 CDP 无窗口(默认,推荐),new_headless=旧 headless=true,headed=可见窗口(调试截图)"},
                "wait_until":{"type":"string","enum":["load","domcontentloaded","networkidle"],"default":"load"},
                "user_agent":{"type":"string","description":"覆盖 User-Agent"},
                "connect_url":{"type":"string","description":"接管已开浏览器,如 http://127.0.0.1:9222"},
                "block_resources":{"type":"array","items":{"type":"string"},"description":"拦截资源类型(image/stylesheet/font/media/script),第 1 轮仅记录"}
            },
            "required":["url"],
            "additionalProperties":false
        })
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(url) = str_arg(&args, "url") else {
            return envelope(1001, "缺少 url", json!({}));
        };
        // 2026-09-17 第 74 轮:三档 mode 替换旧 bool headless。
        // 默认 hidden(纯 CDP 无窗口),解决"误开 macOS 系统默认浏览器"问题。
        let mode = match str_arg(&args, "mode") {
            Some("headed") | Some("head") => crate::agent::browser::BrowserMode::Headed,
            Some("new_headless") | Some("old_headless") => crate::agent::browser::BrowserMode::NewHeadless,
            _ => crate::agent::browser::BrowserMode::Hidden,
        };
        let connect = str_arg(&args, "connect_url");
        let ua = str_arg(&args, "user_agent");
        match BrowserManager::global().new_page(url, mode, connect, ua).await {
            Ok((page_id, title, final_url)) => {
                if str_arg(&args, "wait_until") == Some("networkidle") {
                    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                }
                let mut data = json!({
                    "page_id": page_id,
                    "title": title,
                    "final_url": final_url,
                    "mode": match mode {
                        crate::agent::browser::BrowserMode::Hidden => "hidden",
                        crate::agent::browser::BrowserMode::NewHeadless => "new_headless",
                        crate::agent::browser::BrowserMode::Headed => "headed",
                    },
                    // 2026-09-17 第 74 轮:next_steps —— 分步引导,降低 LLM 编排成本。
                    // WebUseRunner 会把 next_steps 注入到 LLM 上下文,显著降低
                    // 「16 次迭代 tool_calls=0」类失败模式的概率。
                    "next_steps": [
                        {"step": 1, "tool": "BrowserControl", "action": "input_text",
                         "selector_hint": "textarea, [contenteditable=true], input[type=text]",
                         "tip": "在对话框/输入框输入你的查询文本"},
                        {"step": 2, "tool": "BrowserControl", "action": "click",
                         "selector_hint": "button[type=submit], .submit-btn, [class*=send], [class*=submit], img[class*=button]",
                         "tip": "点击提交按钮(图片按钮可用 selector 命中 img 元素)"},
                        {"step": 3, "tool": "BrowserControl", "action": "wait",
                         "selector_hint": "[class*=response], [class*=answer], [class*=result], [class*=message]",
                         "timeout_ms": 30000,
                         "tip": "等待 AI 回复出现,最长等 30 秒"},
                        {"step": 4, "tool": "BrowserInspect", "info": "elements",
                         "selector_hint": "[class*=response], [class*=answer], [class*=result]",
                         "include_text": true,
                         "tip": "提取 AI 回复文本;若 include_text 太短,可改 info=dom 获取 outer_html"}
                    ],
                });
                if let Some(arr) = args.get("block_resources").and_then(Value::as_array) {
                    data["block_resources_hint"] = json!(arr);
                }
                envelope(0, "ok", data)
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains(NO_BROWSER_SENTINEL) {
                    envelope(
                        3001,
                        "未检测到 Chrome/Edge/Chromium",
                        json!({"install":"请安装 Google Chrome 或 Microsoft Edge,或设置 LAEW_BROWSER_PATH"}),
                    )
                } else {
                    envelope(2001, &msg, json!({}))
                }
            }
        }
    }
}

// =================== BrowserList ===================

pub struct BrowserListTool;
#[async_trait]
impl Tool for BrowserListTool {
    fn name(&self) -> &str {
        "BrowserList"
    }
    fn description(&self) -> &str {
        "列出当前存活页面 [{page_id,url,title,created_at}];返回前自动清理失效 entry。"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{},"additionalProperties":false})
    }
    async fn execute(&self, _: Value) -> Result<String> {
        let pages: Vec<Value> = BrowserManager::global()
            .list_pages()
            .await
            .into_iter()
            .map(|(id, url, title, ts)| json!({"page_id":id,"url":url,"title":title,"created_at":ts}))
            .collect();
        envelope(0, "ok", json!({"pages": pages}))
    }
}

// =================== BrowserClose ===================

pub struct BrowserCloseTool;
#[async_trait]
impl Tool for BrowserCloseTool {
    fn name(&self) -> &str {
        "BrowserClose"
    }
    fn description(&self) -> &str {
        "关闭指定 page_id;最后一个页面关闭时回收内部浏览器进程(launch 模式)。幂等。"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"page_id":{"type":"string"}},"required":["page_id"],"additionalProperties":false})
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(id) = str_arg(&args, "page_id") else {
            return envelope(1001, "缺少 page_id", json!({}));
        };
        if BrowserManager::global().close_page(id).await {
            envelope(0, "closed", json!({"page_id":id}))
        } else {
            envelope(2000, "page_id 不存在", json!({"page_id":id}))
        }
    }
}

// =================== BrowserControl ===================

pub struct BrowserControlTool;
#[async_trait]
impl Tool for BrowserControlTool {
    fn name(&self) -> &str {
        "BrowserControl"
    }
    fn description(&self) -> &str {
        "写操作统一入口,action 枚举(移植自 go-web-debug-tool):\
         click/human_click/right_click/double_click/hover/scroll/scroll_to/key_press/\
         press_sequence/input_text/human_input/clear_input/upload_file/select_option/\
         new_tab/close_tab/navigate/back/forward/reload/wait/eval_js/set_cookie/\
         delete_cookie/set_storage/clear_storage/set_viewport/screenshot/heartbeat。\
         派生标签页经 spawned_page_id 回传。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "page_id":{"type":"string"},
                "action":{"type":"string","enum":[
                    "click","human_click","right_click","double_click","hover",
                    "scroll","scroll_to","key_press","press_sequence",
                    "input_text","human_input","clear_input",
                    "upload_file","select_option",
                    "new_tab","close_tab",
                    "navigate","back","forward","reload",
                    "wait","eval_js",
                    "set_cookie","delete_cookie",
                    "set_storage","clear_storage","set_viewport",
                    "screenshot","heartbeat"
                ]},
                "params":{"type":"object"}
            },
            "required":["page_id","action"],
            "additionalProperties":false
        })
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(id) = str_arg(&args, "page_id") else {
            return envelope(1001, "缺少 page_id", json!({}));
        };
        let Some(action) = str_arg(&args, "action") else {
            return envelope(1001, "缺少 action", json!({}));
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
            other => return envelope(1001, "未知 action", json!({"action": other})),
        };
        let spawned = BrowserManager::global().adopt_spawned_pages().await;
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
}

// =================== action implementations ===================

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

fn parse_modifiers(p: &Value) -> i64 {
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
fn key_code_tuple(name: &str) -> Option<(&'static str, &'static str, i64)> {
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
    let value_js = if let Some(v) = p.get("value") {
        js_str(v.as_str().unwrap_or(""))
    } else if let Some(arr) = p.get("values").and_then(Value::as_array) {
        let parts: Vec<String> = arr.iter()
            .filter_map(|v| v.as_str().map(|s| js_str(s)))
            .collect();
        format!("[{}]", parts.join(","))
    } else if let Some(t) = p.get("text") { js_str(t.as_str().unwrap_or("")) }
    else if if let Some(i) = p.get("index").and_then(Value::as_i64) { i.to_string() } else { String::new() }.is_empty() {
        return Err("缺 value/values/text/index".into());
    } else { p.get("index").and_then(Value::as_i64).unwrap().to_string() };
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
    Ok(json!({"selected": ok, "selector": sel}))
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
    // 2026-09-16 第 66 轮:navigate 超时 30s,防止页面挂起导致无限等待
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
        let timeout_ms = p.get("timeout_ms").and_then(Value::as_u64).unwrap_or(15000);
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

async fn act_screenshot(id: &str, p: &Value) -> std::result::Result<Value, String> {
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
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(json!({"image_base64": b64, "byte_size": bytes.len(), "format": format_str}))
}

async fn act_heartbeat(id: &str) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let title = page.get_title().await.ok().flatten().unwrap_or_default();
    let _ = BrowserManager::global().events(id).await;
    Ok(json!({"ok": true, "title": title}))
}

// —— mouse 派发辅助 ——
async fn dispatch_mouse(
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

// =================== BrowserInspect ===================

pub struct BrowserInspectTool;
#[async_trait]
impl Tool for BrowserInspectTool {
    fn name(&self) -> &str { "BrowserInspect" }
    fn description(&self) -> &str {
        "只读观察统一入口,info 枚举(移植自 go-web-debug-tool):\
         console/network/elements/dom/localstorage/sessionstorage/cookies/screenshot/\
         page_meta/viewport/url/title/ping。"
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "page_id":{"type":"string"},
                "info":{"type":"string","enum":[
                    "console","network","elements","dom",
                    "localstorage","sessionstorage","cookies",
                    "screenshot","page_meta","viewport",
                    "url","title","ping","image_urls"
                ]},
                "params":{"type":"object"}
            },
            "required":["page_id","info"],
            "additionalProperties":false
        })
    }
    async fn execute(&self, args: Value) -> Result<String> {
        let Some(id) = str_arg(&args, "page_id") else {
            return envelope(1001, "缺少 page_id", json!({}));
        };
        let Some(kind) = str_arg(&args, "info") else {
            return envelope(1001, "缺少 info", json!({}));
        };
        let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
        if kind == "ping" {
            let ms = now_millis_safe();
            return envelope(0, "ok", json!({"ok": true, "page_id": id, "latency_ms": 0, "ts": ms}));
        }
        let page = match BrowserManager::global().page(id).await {
            Some(p) => p,
            None => return envelope(2000, "page_id 不存在", json!({"page_id": id})),
        };
        let res: std::result::Result<Value, String> = match kind {
            "console" => {
                let buf = BrowserManager::global().events(id).await;
                let (healthy, last) = match &buf {
                    Some(b) => b.collection_healthy(),
                    None => (true, None),
                };
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(100).min(500);
                let events: Vec<Value> = buf.map(|b| {
                    let q = b.console.lock().unwrap();
                    q.iter().rev().take(limit as usize).rev().map(|e| {
                        let mut v = e.data.clone();
                        v["ts"] = json!(e.ts_ms);
                        v
                    }).collect()
                }).unwrap_or_default();
                Ok(json!({"events": events, "count": events.len(), "collection_healthy": healthy, "last_event_at": last}))
            }
            "network" => {
                let buf = BrowserManager::global().events(id).await;
                let (healthy, last) = match &buf {
                    Some(b) => b.collection_healthy(),
                    None => (true, None),
                };
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(100).min(500);
                let events: Vec<Value> = buf.map(|b| {
                    let q = b.network.lock().unwrap();
                    q.iter().rev().take(limit as usize).rev().map(|e| {
                        let mut v = e.data.clone();
                        v["ts"] = json!(e.ts_ms);
                        v
                    }).collect()
                }).unwrap_or_default();
                Ok(json!({"events": events, "count": events.len(), "collection_healthy": healthy, "last_event_at": last}))
            }
            "elements" => {
                let Some(sel) = str_arg(&params, "selector") else {
                    return envelope(1001, "缺 selector", json!({}));
                };
                let nth = params.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
                let include_text = params.get("include_text").and_then(Value::as_bool).unwrap_or(true);
                let include_outer = params.get("include_outer_html").and_then(Value::as_bool).unwrap_or(false);
                let include_rect = params.get("include_rect").and_then(Value::as_bool).unwrap_or(true);
                let js = format!(
                    r#"(() => {{ const els=document.querySelectorAll({sel}); const el=els[{nth}];
                    if(!el) return {{ok:false, count: els.length, index:{nth}}};
                    const r=el.getBoundingClientRect();
                    return {{ok:true, tag:el.tagName.toLowerCase(), count:els.length, index:{nth},
                        text: ({itxt} ? el.innerText : null),
                        outer_html: ({iouter} ? el.outerHTML : null),
                        rect: {{x:r.x,y:r.y,width:r.width,height:r.height,visible:r.width>0&&r.height>0}}
                    }}; }})()"#,
                    sel = js_str(sel), nth = nth, itxt = include_text, iouter = include_outer
                );
                eval_js_string(&page, &js).await
            }
            "dom" => {
                let sel = str_arg(&params, "selector").unwrap_or("html");
                if sel != "html" {
                    let js = format!(
                        r#"(() => {{ const el=document.querySelector({sel});
                        if(!el) return {{ok:false, selector:{sel}}};
                        return {{ok:true, selector:{sel}, outer_html:el.outerHTML}}; }})()"#,
                        sel = js_str(sel)
                    );
                    eval_js_string(&page, &js).await
                } else {
                let max_depth = params.get("max_depth").and_then(Value::as_i64).unwrap_or(3).clamp(1, 10);
                let node_limit = params.get("node_count_limit").and_then(Value::as_u64).unwrap_or(2000).min(5000);
                let js = format!(
                    r#"(() => {{
                        const MAX_DEPTH={max_depth}, NODE_LIMIT={node_limit};
                        let nodes=0, truncated=false;
                        function walk(n, d) {{
                            if (nodes>=NODE_LIMIT) {{ truncated=true; return null; }}
                            if (d>MAX_DEPTH) return null;
                            const obj={{
                                node_type: n.nodeType, node_name: n.nodeName,
                                node_value: n.nodeValue, attributes: {{}};
                            }};
                            nodes++;
                            if (n.nodeType===1) {{
                                for (const a of n.attributes) obj.attributes[a.name]=a.value;
                                const children=[];
                                for (const c of n.childNodes) {{
                                    const r=walk(c, d+1); if (r) children.push(r);
                                    if (truncated) break;
                                }}
                                if (children.length) obj.children=children;
                            }}
                            return obj;
                        }}
                        const tree=walk(document, 0);
                        return {{document: tree, node_count:nodes, truncated, max_depth:MAX_DEPTH}};
                    }})()"#,
                    max_depth = max_depth, node_limit = node_limit
                );
                    eval_js_string(&page, &js).await
                }
            }
            "localstorage" | "sessionstorage" => {
                let storage_obj = if kind == "sessionstorage" { "sessionStorage" } else { "localStorage" };
                let prefix = str_arg(&params, "prefix");
                let contains = str_arg(&params, "contains");
                let filter_js = match (prefix, contains) {
                    (Some(p), None) => format!("(k.startsWith({}))", js_str(p)),
                    (None, Some(c)) => format!("(k.includes({}))", js_str(c)),
                    (Some(p), Some(c)) => format!("(k.startsWith({}) && k.includes({}))", js_str(p), js_str(c)),
                    (None, None) => "(() => true)".to_string(),
                };
                let js = format!(
                    r#"(() => {{ const s={storage}; const out={{}};
                        for (let i=0;i<s.length;i++) {{ const k=s.key(i); if ({filter}) out[k]=s.getItem(k); }}
                        return out; }})()"#,
                    storage = storage_obj, filter = filter_js
                );
                eval_js_string(&page, &js).await
            }
            "cookies" => {
                let r = match page.execute(
                    chromiumoxide::cdp::browser_protocol::network::GetCookiesParams::default(),
                ).await {
                    Ok(r) => r,
                    Err(e) => return envelope(2002, &e.to_string(), json!({})),
                };
                let cookies: Vec<Value> = r.cookies.iter().map(|c| json!({
                    "name": c.name, "value": c.value, "domain": c.domain, "path": c.path,
                    "expires": c.expires, "http_only": c.http_only, "secure": c.secure,
                    "same_site": format!("{:?}", c.same_site),
                })).collect();
                Ok(json!({"cookies": cookies}))
            }
            "screenshot" => act_screenshot(id, &params).await,
            "page_meta" => {
                let url = page.url().await.ok().flatten().unwrap_or_default();
                let title = page.get_title().await.ok().flatten().unwrap_or_default();
                let viewport = match eval_js_string(&page, "({w: window.innerWidth, h: window.innerHeight})").await {
                    Ok(v) => v,
                    Err(e) => json!({"error": e}),
                };
                let ua = page
                    .evaluate("navigator.userAgent")
                    .await
                    .ok()
                    .and_then(|v| v.value().cloned())
                    .unwrap_or(Value::Null);
                Ok(json!({"page_id": id, "url": url, "title": title, "viewport": viewport, "user_agent": ua}))
            }
            "viewport" => eval_js_string(&page,
                "({width: window.innerWidth, height: window.innerHeight, devicePixelRatio: window.devicePixelRatio, scrollX: window.scrollX, scrollY: window.scrollY, scrollWidth: document.documentElement.scrollWidth, scrollHeight: document.documentElement.scrollHeight})"
            ).await,
            "url" => Ok(json!({"url": page.url().await.ok().flatten().unwrap_or_default()})),
            "title" => Ok(json!({"title": page.get_title().await.ok().flatten().unwrap_or_default()})),
            // 2026-09-16 第 64 轮:image_urls —— 一键提取页面图片 URL + canvas/svg 计数。
            // 文心一言 K 线图 / ChatGPT 图表 / Claude.ai 生成的图都是 <img src=...> 或 canvas。
            // 限制 img_urls 最多 20 条避免大页面输出爆炸;canvas/svg 只计数。
            "image_urls" => {
                let max_n = params.get("max").and_then(Value::as_u64).unwrap_or(20).min(100) as usize;
                let js = format!(
                    r#"(() => {{
                        const N = {max_n};
                        const imgs = Array.from(document.querySelectorAll('img'))
                            .map(i => i.src || i.getAttribute('data-src') || '')
                            .filter(Boolean);
                        const data_imgs = imgs.filter(s => s.startsWith('data:image/'));
                        const canvases = document.querySelectorAll('canvas').length;
                        const svgs = document.querySelectorAll('svg').length;
                        const pics = document.querySelectorAll('picture').length;
                        return {{
                            img_count: imgs.length,
                            img_urls: imgs.slice(0, N),
                            data_image_count: data_imgs.length,
                            canvas_count: canvases,
                            svg_count: svgs,
                            picture_count: pics
                        }};
                    }})()"#,
                    max_n = max_n,
                );
                eval_js_string(&page, &js).await
            }
            other => return envelope(1001, "未知 info", json!({"info": other})),
        };
        match res {
            Ok(data) => envelope(0, "ok", data),
            Err(e) => envelope(2002, &e, json!({})),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_codes() {
        let s = envelope(0, "ok", json!({"x": 1})).unwrap();
        assert!(s.contains("\"code\":0"));
        assert!(s.contains("\"message\":\"ok\""));
        assert!(s.contains("\"x\":1"));
    }

    #[test]
    fn js_str_escapes() {
        let s = js_str("a'b\nc\"");
        assert!(s.starts_with('"') && s.ends_with('"'));
        assert!(s.contains("\\n"));
    }

    #[test]
    fn key_code_tuple_lookup() {
        assert_eq!(key_code_tuple("Enter"), Some(("Enter", "Enter", 13)));
        assert_eq!(key_code_tuple("F12"), Some(("F12", "F12", 123)));
        assert!(key_code_tuple("未知键").is_none());
    }

    #[test]
    fn parse_modifiers_combo() {
        let p = json!({"modifiers": ["ctrl", "shift"]});
        assert_eq!(parse_modifiers(&p), 2 | 8);
        let p = json!({"modifiers": "ctrl,shift"});
        assert_eq!(parse_modifiers(&p), 2 | 8);
    }

    // 2026-09-17 第 74 轮:BrowserNew next_steps & mode 测试。

    #[test]
    fn browser_new_parameters_has_mode_enum() {
        let p = BrowserNewTool.parameters();
        let mode = p["properties"]["mode"].clone();
        assert!(mode.is_object(), "mode 字段应为对象");
        assert_eq!(mode["default"], "hidden", "默认 mode 应为 hidden");
        let enums = mode["enum"].as_array().expect("enum 必须是数组");
        let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"hidden"));
        assert!(names.contains(&"new_headless"));
        assert!(names.contains(&"headed"));
    }

    #[test]
    fn browser_new_parameters_no_legacy_headless() {
        // 第 74 轮迁移:headless 字段被移除,改用 mode 枚举
        let p = BrowserNewTool.parameters();
        assert!(p["properties"]["headless"].is_null(),
            "headless bool 字段应被移除,改用 mode 枚举");
    }

    #[tokio::test]
    async fn browser_new_missing_url_returns_1001() {
        // 单元测试:缺 url 时返回参数错误(不需真实浏览器)
        let res = BrowserNewTool.execute(json!({})).await.unwrap();
        assert!(res.contains("\"code\":1001"));
        assert!(res.contains("缺少 url"));
    }

    #[tokio::test]
    async fn browser_new_no_browser_or_succeeds() {
        // 单元测试:无 Chrome 时返回 3001 + 安装提示;有 Chrome 时返回 0 + next_steps。
        // 由于运行环境可能安装了 Chrome,这里兼容两种路径。
        let res = BrowserNewTool.execute(json!({"url": "https://example.com"})).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
        let code = v["code"].as_i64().unwrap_or(-1);
        if code == 0 {
            // 成功路径:验证 mode=hidden + next_steps 非空
            assert_eq!(v["data"]["mode"], "hidden", "默认 mode 应为 hidden");
            let steps = v["data"]["next_steps"].as_array()
                .expect("成功响应必须包含 next_steps 数组");
            assert!(!steps.is_empty(), "next_steps 不能为空");
            assert_eq!(steps.len(), 4, "next_steps 应包含 4 步引导");
        } else {
            // 失败路径:2001(断连) 或 3001(无浏览器)
            assert!(code == 2001 || code == 3001,
                "无浏览器/失败时应返回 2001/3001,实际 code={code}");
        }
    }
}