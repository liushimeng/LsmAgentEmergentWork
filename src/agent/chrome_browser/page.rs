//! Chrome 页面级操作(基础框架):导航 / eval / 截图 / 点击 / 输入 / 滚动 / 等待。
//!
//! 所有 CDP 调用统一走有界墙钟预算(超时即失败返回),根治「导航状态机卡死 →
//! 后续 CDP 消息全部排队 → Agent 循环永久挂起」事故(依据见归档
//! `src/agent/browser_cdp.rs` 头注)。

use std::time::Duration;

use chromiumoxide::Page;
use serde_json::Value;

use crate::error::{AgentError, Result};

/// 单条 CDP 轻量调用的默认墙钟预算(与历史 `CDP_BOUND_SECS` 同值同语义)。
const CDP_BOUND_SECS: u64 = 10;
/// 导航级调用的墙钟预算(load 事件等待,给慢站点留余量)。
const NAV_BOUND_SECS: u64 = 30;

fn err(what: &str, e: impl std::fmt::Display) -> AgentError {
    AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("{what} 失败: {e}")}
}

fn timeout_err(what: &str, secs: u64) -> AgentError {
    AgentError::ToolExecution { tool: "chrome_browser".into(), reason: format!("{what} 超时({secs}s),浏览器 handler 可能已卡死")}
}

/// 导航到 URL(goto + 有界等待装载)。
pub async fn navigate(page: &Page, url: &str) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(NAV_BOUND_SECS), page.goto(url))
        .await
        .map_err(|_| timeout_err(&format!("page.goto({url})"), NAV_BOUND_SECS))?
        .map_err(|e| err(&format!("page.goto({url})"), e))?;
    Ok(())
}

/// 读取当前 URL 与标题(探测类,拿不到按空串处理)。
pub async fn url_and_title(page: &Page) -> (String, String) {
    let url = tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.url())
        .await
        .ok()
        .and_then(|r| r.ok().flatten())
        .unwrap_or_default();
    let title = tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.get_title())
        .await
        .ok()
        .and_then(|r| r.ok().flatten())
        .unwrap_or_default();
    (url, title)
}

/// 页面级 eval_js:表达式/语句求值,返回 JSON 值(异常/无值按 Null 处理)。
pub async fn eval_js(page: &Page, script: &str) -> Result<Value> {
    let res = tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.evaluate(script))
        .await
        .map_err(|_| timeout_err("eval_js", CDP_BOUND_SECS))?
        .map_err(|e| err("eval_js", e))?;
    Ok(res.value().cloned().unwrap_or(Value::Null))
}

/// 全页 PNG 截图,返回 base64(与 Lsm 版 `inspect(screenshot)` 信封对齐)。
///
/// CDP 的 `Page.captureScreenshot` 返回的 `data` **本身已是 base64 字符串**,
/// 直接透传(不可再二次编码,否则落盘得到的不是 PNG)。
pub async fn screenshot_base64(page: &Page) -> Result<String> {
    use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotParams;
    let resp = tokio::time::timeout(
        Duration::from_secs(CDP_BOUND_SECS),
        page.execute(CaptureScreenshotParams::builder().build()),
    )
    .await
    .map_err(|_| timeout_err("screenshot", CDP_BOUND_SECS))?
    .map_err(|e| err("screenshot", e))?;
    // `Binary` 是 base64 文本 newtype(其内部 String 即 base64 文本)。
    let b64: &str = AsRef::<str>::as_ref(&resp.result.data);
    Ok(b64.to_string())
}

/// 按 CSS 选择器点击(JS 通道:存在性检查 + click),返回是否命中元素。
pub async fn click(page: &Page, selector: &str) -> Result<bool> {
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return false;
            el.scrollIntoView({{block:'center'}});
            el.click();
            return true;
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let v = eval_js(page, &js).await?;
    Ok(v.as_bool().unwrap_or(false))
}

/// 按**可见文字**点击元素(最小文本命中优先),可限定作用域选择器。
///
/// 现代站点大量按钮没有稳定 id/class(「选为终点」「查询」等纯文本按钮),
/// 只给 CSS 选择器会迫使模型反复猜选择器甚至改用 eval_js 手搓事件。
/// 返回 `{hit, tag, id, cls, text}`;`exact=true` 时要求整段文本完全相等。
pub async fn click_text(
    page: &Page,
    text: &str,
    scope: Option<&str>,
    exact: bool,
) -> Result<Value> {
    let js = format!(
        r#"(function(){{
            var target = {text};
            var scopeSel = {scope};
            var exact = {exact};
            var root = null;
            try {{ root = scopeSel ? document.querySelector(scopeSel) : document.body; }} catch (e) {{}}
            if (!root) return JSON.stringify({{hit:false, reason:'scope_not_found'}});
            function norm(s) {{ return String(s == null ? '' : s).replace(/\s+/g, ' ').trim(); }}
            // 表单控件的 placeholder 是输入提示,不是按钮标签。
            // 实测百度地图搜索框 placeholder=「搜地点、查公交、找路线」,
            // 旧实现会把点击「路线」「公交」误判成点击 INPUT#sole-input。
            function ownText(el) {{
                var tag = String(el.tagName || '').toUpperCase();
                if (tag === 'INPUT' || tag === 'TEXTAREA') {{
                    if (/^(button|submit|reset)$/i.test(el.type || '')) return norm(el.value);
                    return '';
                }}
                if (tag === 'SELECT') return '';
                return norm(el.innerText || el.textContent || '');
            }}
            function labelOf(el) {{
                try {{
                    return norm(el.getAttribute('aria-label') || el.getAttribute('title') ||
                                el.getAttribute('data-title') || '');
                }} catch (e) {{ return ''; }}
            }}
            // 可点击性打分:纯文字按钮常是 div,图标按钮只有 aria-label/title。
            function clickScore(el) {{
                var s = 0;
                var tag = String(el.tagName || '').toUpperCase();
                if (tag === 'BUTTON' || tag === 'A') s += 100;
                if (tag === 'INPUT' && /^(button|submit|reset)$/i.test(el.type || '')) s += 100;
                var cls = '', id = '';
                try {{ cls = String(el.className || ''); }} catch (e) {{}}
                try {{ id = String(el.id || ''); }} catch (e) {{}}
                if (/btn|button|tab|link|item/i.test(cls + ' ' + id)) s += 50;
                try {{
                    var role = String(el.getAttribute('role') || '');
                    if (role === 'button' || role === 'tab' || role === 'link' || role === 'menuitem') s += 40;
                }} catch (e) {{}}
                try {{ if (window.getComputedStyle(el).cursor === 'pointer') s += 30; }} catch (e) {{}}
                try {{ if (typeof el.onclick === 'function') s += 10; }} catch (e) {{}}
                try {{ if (el.tabIndex >= 0) s += 5; }} catch (e) {{}}
                return s;
            }}
            function visible(el) {{
                try {{ var r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; }}
                catch (e) {{ return false; }}
            }}
            var all = root.querySelectorAll('*');
            var best = null, bestScore = -1, bestLen = 1e9, bestText = '';
            for (var i = 0; i < all.length; i++) {{
                var el = all[i];
                if (!visible(el)) continue;
                var tag = String(el.tagName || '').toUpperCase();
                if (tag === 'TEXTAREA' || tag === 'SELECT') continue;
                if (tag === 'INPUT' && !/^(button|submit|reset)$/i.test(el.type || '')) continue;
                var txt = ownText(el);
                var label = labelOf(el);
                if (!txt && !label) continue;
                var hit = exact
                    ? (txt === target || label === target)
                    : (txt.indexOf(target) >= 0 || label.indexOf(target) >= 0);
                if (!hit) continue;
                var score = clickScore(el);
                // 完全相等优先于包含匹配(容器文本更长,最小者最接近按钮本身)。
                if (txt && txt === target) score += 20;
                var len = txt ? txt.length : label.length;
                if (score > bestScore || (score === bestScore && len < bestLen)) {{
                    best = el; bestScore = score; bestLen = len; bestText = txt || label;
                }}
            }}
            if (!best) return JSON.stringify({{hit:false, reason:'text_not_found', target:target}});
            var clicked = best, via = 'self';
            var cls0 = '';
            try {{ cls0 = String(best.className || ''); }} catch (e) {{}}
            // tooltip / 纯标签节点本身没有点击动作。改点它矩形周边最近的可点击元素
            // (实测百度地图「路线」:真实按钮是 tooltip 上方的 route-button 图标,
            //  tooltip 是 body 的直接子节点,向上找祖先找不到按钮)。
            var labelish = /tooltip|label|hint|tip/i.test(cls0) || bestScore < 30;
            if (labelish) {{
                var br = best.getBoundingClientRect();
                var tcx = (br.left + br.right) / 2, tcy = (br.top + br.bottom) / 2;
                var nBest = null, nDist = 1e9, nScore = -1;
                for (var j = 0; j < all.length; j++) {{
                    var cand = all[j];
                    if (cand === best || !visible(cand)) continue;
                    var sc = clickScore(cand);
                    if (sc < 40) continue;
                    var cr = cand.getBoundingClientRect();
                    var ccx = (cr.left + cr.right) / 2, ccy = (cr.top + cr.bottom) / 2;
                    var d = Math.sqrt((ccx - tcx) * (ccx - tcx) + (ccy - tcy) * (ccy - tcy));
                    if (d > 160) continue;
                    if (d < nDist || (d === nDist && sc > nScore)) {{
                        nBest = cand; nDist = d; nScore = sc;
                    }}
                }}
                if (nBest) {{ clicked = nBest; via = 'neighbor'; bestScore = nScore; }}
                else if (bestScore < 30) {{
                    return JSON.stringify({{hit:false, reason:'matched_non_clickable',
                        matched:{{tag:best.tagName, id:best.id||'', cls:cls0.slice(0,120), text:bestText}},
                        hint:'命中文字但没有可点击元素;用 inspect(info="elements", params.text=...) 查看可点击元素,或改用 click(params.selector)'}});
                }}
            }}
            try {{ clicked.scrollIntoView({{block:'center'}}); }} catch (e) {{}}
            try {{ clicked.click(); }} catch (e) {{}}
            var cls = '';
            try {{ cls = String(clicked.className || ''); }} catch (e) {{}}
            return JSON.stringify({{hit:true, via:via, tag:clicked.tagName, id:clicked.id||'', cls:cls.slice(0,120),
                                     score:bestScore, matched_text:bestText,
                                     text:norm(clicked.innerText||clicked.textContent||clicked.value||'').slice(0,200)}});
        }})()"#,
        text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into()),
        scope = serde_json::to_string(&scope).unwrap_or_else(|_| "null".into()),
        exact = if exact { "true" } else { "false" },
    );
    let v = eval_js(page, &js).await?;
    let raw = v.as_str().unwrap_or("{}");
    Ok(serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({"hit": false, "raw": raw})))
}

/// 页面存活探测:`receiver is gone`(target 被销毁/崩溃)时立即失败。
pub async fn page_alive(page: &Page) -> bool {
    tokio::time::timeout(Duration::from_secs(5), page.evaluate("1"))
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
}

/// 列出当前可见、可交互的元素(元素发现原语)。
///
/// 现代站点大量按钮无稳定 id/class,整页 HTML 又可能 40 万字符。Agent 需要
/// 一个「先看有哪些可点/可输入元素」的入口,再决定 click/type 的 selector。
/// 返回 `{elements:[{selector,tag,id,cls,role,type,placeholder,text,label,x,y,w,h}], count}`;
/// `selector` 由 id/稳定 class + `:nth-of-type` 组成,可直接回传给 click/type。
pub async fn visible_elements(
    page: &Page,
    scope: Option<&str>,
    text_filter: Option<&str>,
    max: usize,
) -> Result<Value> {
    let max = max.clamp(1, 200);
    let js = format!(
        r#"(function(){{
            var scopeSel = {scope};
            var filter = {filter};
            var max = {max};
            var root = null;
            try {{ root = scopeSel ? document.querySelector(scopeSel) : document.body; }} catch (e) {{}}
            if (!root) return JSON.stringify({{elements:[], count:0, reason:'scope_not_found'}});
            function norm(s) {{ return String(s == null ? '' : s).replace(/\s+/g, ' ').trim(); }}
            function esc(s) {{
                if (window.CSS && CSS.escape) return CSS.escape(s);
                return String(s).replace(/([^a-zA-Z0-9_-])/g, '\\$1');
            }}
            function uniq(sel, el) {{
                try {{ var list = document.querySelectorAll(sel); return list.length === 1 && list[0] === el; }}
                catch (e) {{ return false; }}
            }}
            // 稳定选择器:优先 #id;否则 tag + 稳定 class + :nth-of-type 逐级上溯。
            function cssPath(el) {{
                if (el.id && uniq('#' + esc(el.id), el)) return '#' + esc(el.id);
                var parts = [], cur = el, depth = 0;
                while (cur && cur.nodeType === 1 && cur !== document.documentElement && depth < 6) {{
                    var seg = cur.tagName.toLowerCase();
                    var cls = String(cur.className || '').trim().split(/\s+/).filter(function(c) {{
                        return /^[A-Za-z_][\w-]*$/.test(c) && c.length <= 30 && !/\d{{3,}}/.test(c);
                    }}).slice(0, 2);
                    if (cls.length) seg += '.' + cls.map(esc).join('.');
                    var parent = cur.parentElement;
                    if (parent) {{
                        var same = Array.prototype.filter.call(parent.children, function(x) {{
                            return x.tagName === cur.tagName;
                        }});
                        if (same.length > 1) seg += ':nth-of-type(' + (same.indexOf(cur) + 1) + ')';
                    }}
                    parts.unshift(seg);
                    var candidate = parts.join(' > ');
                    if (uniq(candidate, el)) return candidate;
                    cur = parent; depth++;
                }}
                return parts.join(' > ');
            }}
            function visible(el) {{
                try {{
                    var r = el.getBoundingClientRect();
                    if (r.width <= 0 || r.height <= 0) return false;
                    var s = getComputedStyle(el);
                    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
                }} catch (e) {{ return false; }}
            }}
            function textOf(el) {{
                var tag = String(el.tagName || '').toUpperCase();
                if (tag === 'INPUT' || tag === 'TEXTAREA') {{
                    if (/^(button|submit|reset)$/i.test(el.type || '')) return norm(el.value);
                    return '';
                }}
                return norm(el.innerText || el.textContent || '');
            }}
            function isInteractive(el) {{
                var tag = String(el.tagName || '').toUpperCase();
                if (tag === 'A' || tag === 'BUTTON' || tag === 'SELECT' || tag === 'TEXTAREA') return true;
                if (tag === 'INPUT') return !/^hidden$/i.test(el.type || '');
                var role = String(el.getAttribute('role') || '');
                if (/^(button|tab|link|menuitem|option|checkbox|radio|switch)$/.test(role)) return true;
                if (el.hasAttribute('onclick') || el.hasAttribute('contenteditable')) return true;
                if (el.hasAttribute('tabindex') && Number(el.getAttribute('tabindex')) >= 0) return true;
                var cls = String(el.className || '') + ' ' + String(el.id || '');
                if (/btn|button|tab|link|item|switch|checkbox|radio|menu|tool|icon/i.test(cls)) return true;
                try {{ if (getComputedStyle(el).cursor === 'pointer') return true; }} catch (e) {{}}
                return false;
            }}
            var out = [], scanned = 0, all = root.querySelectorAll('*');
            for (var i = 0; i < all.length && out.length < max && scanned < 40000; i++) {{
                var el = all[i]; scanned++;
                if (el === root || !visible(el) || !isInteractive(el)) continue;
                var txt = textOf(el).slice(0, 80);
                var ph = '';
                try {{ ph = norm(el.getAttribute('placeholder') || ''); }} catch (e) {{}}
                var label = '';
                try {{
                    label = norm(el.getAttribute('aria-label') || el.getAttribute('title') ||
                                el.getAttribute('data-title') || '');
                }} catch (e) {{}}
                if (!txt && !ph && !label && String(el.tagName).toUpperCase() !== 'INPUT') continue;
                if (filter) {{
                    var blob = (txt + ' ' + ph + ' ' + label).toLowerCase();
                    if (blob.indexOf(filter.toLowerCase()) < 0) continue;
                }}
                var r = el.getBoundingClientRect();
                out.push({{
                    selector: cssPath(el),
                    tag: el.tagName,
                    id: el.id || '',
                    cls: String(el.className || '').slice(0, 100),
                    role: el.getAttribute('role') || '',
                    type: el.getAttribute('type') || '',
                    placeholder: ph,
                    text: txt,
                    label: label,
                    x: Math.round(r.x), y: Math.round(r.y),
                    w: Math.round(r.width), h: Math.round(r.height),
                }});
            }}
            return JSON.stringify({{elements: out, count: out.length, scope: scopeSel || 'body'}});
        }})()"#,
        scope = serde_json::to_string(&scope).unwrap_or_else(|_| "null".into()),
        filter = serde_json::to_string(&text_filter).unwrap_or_else(|_| "null".into()),
        max = max,
    );
    let v = eval_js(page, &js).await?;
    let raw = v.as_str().unwrap_or("{}");
    Ok(serde_json::from_str(raw).unwrap_or_else(|_| serde_json::json!({"elements": [], "count": 0, "raw": raw})))
}

/// 键位元数据:(key, code, windowsVirtualKeyCode, text)。
fn key_meta(key: &str) -> (String, String, i64, Option<String>) {
    match key {
        "Enter" => ("Enter".into(), "Enter".into(), 13, Some("\r".into())),
        "Tab" => ("Tab".into(), "Tab".into(), 9, None),
        "Escape" | "Esc" => ("Escape".into(), "Escape".into(), 27, None),
        "Backspace" => ("Backspace".into(), "Backspace".into(), 8, None),
        "Delete" => ("Delete".into(), "Delete".into(), 46, None),
        "ArrowDown" | "Down" => ("ArrowDown".into(), "ArrowDown".into(), 40, None),
        "ArrowUp" | "Up" => ("ArrowUp".into(), "ArrowUp".into(), 38, None),
        "ArrowLeft" | "Left" => ("ArrowLeft".into(), "ArrowLeft".into(), 37, None),
        "ArrowRight" | "Right" => ("ArrowRight".into(), "ArrowRight".into(), 39, None),
        "Home" => ("Home".into(), "Home".into(), 36, None),
        "End" => ("End".into(), "End".into(), 35, None),
        "PageUp" => ("PageUp".into(), "PageUp".into(), 33, None),
        "PageDown" => ("PageDown".into(), "PageDown".into(), 34, None),
        " " | "Space" => (" ".into(), "Space".into(), 32, Some(" ".into())),
        other => {
            let code = if other.len() == 1 && other.chars().all(|c| c.is_ascii_alphanumeric()) {
                let ch = other.chars().next().unwrap().to_ascii_uppercase();
                format!("Key{ch}")
            } else {
                String::new()
            };
            let vk = other
                .chars()
                .next()
                .map(|c| c.to_ascii_uppercase() as i64)
                .unwrap_or(0);
            (other.to_string(), code, vk, Some(other.to_string()))
        }
    }
}

/// 向页面发送**真实键盘事件**(CDP Input.dispatchKeyEvent)。
///
/// 与 `type_text` 的 JS setter 路线互补:站点联想/快捷键监听 keydown 时,
/// 仅赋值+input 事件不会触发(实测百度地图地址联想)。可先聚焦 selector,
/// 再发送 keyDown/keyUp;`Enter` 带 `text="\r"`,表单与联想确认都能收到。
pub async fn key_press(page: &Page, key: &str, selector: Option<&str>) -> Result<Value> {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchKeyEventParams, DispatchKeyEventType,
    };
    if let Some(sel) = selector {
        let js = format!(
            r#"(function(){{ var el=document.querySelector({sel}); if(!el) return false; el.focus(); return document.activeElement === el; }})()"#,
            sel = serde_json::to_string(sel).unwrap_or_default()
        );
        let _ = eval_js(page, &js).await?;
    }
    let (key_name, code, vk, text) = key_meta(key);
    let mut down = DispatchKeyEventParams::builder()
        .r#type(DispatchKeyEventType::KeyDown)
        .key(key_name.clone())
        .code(code.clone())
        .windows_virtual_key_code(vk)
        .native_virtual_key_code(vk);
    if let Some(t) = text.clone() {
        down = down.text(t);
    }
    let down = down.build().map_err(|e| err("key_press(keyDown)", e))?;
    tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.execute(down))
        .await
        .map_err(|_| timeout_err("key_press(keyDown)", CDP_BOUND_SECS))?
        .map_err(|e| err("key_press(keyDown)", e))?;
    let up = DispatchKeyEventParams::builder()
        .r#type(DispatchKeyEventType::KeyUp)
        .key(key_name.clone())
        .code(code.clone())
        .windows_virtual_key_code(vk)
        .native_virtual_key_code(vk)
        .build()
        .map_err(|e| err("key_press(keyUp)", e))?;
    tokio::time::timeout(Duration::from_secs(CDP_BOUND_SECS), page.execute(up))
        .await
        .map_err(|_| timeout_err("key_press(keyUp)", CDP_BOUND_SECS))?
        .map_err(|e| err("key_press(keyUp)", e))?;
    Ok(serde_json::json!({
        "key": key_name,
        "code": code,
        "windows_virtual_key_code": vk,
        "focused": selector,
    }))
}

/// 读取选择器命中的第一个元素文本(可选:文档无匹配返回 None)。
pub async fn element_text(page: &Page, selector: &str) -> Result<Option<String>> {
    let js = format!(
        r#"(function(){{ var el=null; try {{ el=document.querySelector({sel}); }} catch(e) {{ return null; }}
            if (!el) return null; return String(el.innerText || el.textContent || ''); }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let v = eval_js(page, &js).await?;
    Ok(v.as_str().map(str::to_string))
}

/// 读取选择器命中的第一个元素 outerHTML(可选)。
pub async fn element_html(page: &Page, selector: &str) -> Result<Option<String>> {
    let js = format!(
        r#"(function(){{ var el=null; try {{ el=document.querySelector({sel}); }} catch(e) {{ return null; }}
            if (!el) return null; return String(el.outerHTML || ''); }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    let v = eval_js(page, &js).await?;
    Ok(v.as_str().map(str::to_string))
}

/// 按 CSS 选择器输入文本(聚焦 + 原生 setter 赋值 + input/change 事件),
/// 返回是否命中元素。
pub async fn type_text(page: &Page, selector: &str, text: &str) -> Result<bool> {
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return false;
            el.scrollIntoView({{block:'center'}});
            el.focus();
            var proto = el instanceof HTMLTextAreaElement
                ? HTMLTextAreaElement.prototype
                : HTMLInputElement.prototype;
            var setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
            setter.call(el, {text});
            el.dispatchEvent(new Event('input', {{bubbles:true}}));
            el.dispatchEvent(new Event('change', {{bubbles:true}}));
            return true;
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default(),
        text = serde_json::to_string(text).unwrap_or_default()
    );
    let v = eval_js(page, &js).await?;
    Ok(v.as_bool().unwrap_or(false))
}

/// 按选择器输入文本;未命中时回可见输入框候选,便于模型改选正确 selector。
///
/// 实测模型会猜 `#route-inputstart`,而真实节点是 `input.route-start-input`
/// (无 id)。只回 `{hit:false}` 会迫使模型继续盲猜或改用 eval_js。
pub async fn type_text_ex(page: &Page, selector: &str, text: &str) -> Result<Value> {
    if type_text(page, selector, text).await? {
        return Ok(serde_json::json!({"selector": selector, "hit": true}));
    }
    let listed = visible_elements(page, None, None, 200).await?;
    let candidates: Vec<Value> = listed
        .get("elements")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|e| {
                    matches!(
                        e.get("tag").and_then(Value::as_str).unwrap_or(""),
                        "INPUT" | "TEXTAREA"
                    )
                })
                .take(8)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    Ok(serde_json::json!({
        "selector": selector,
        "hit": false,
        "candidates": candidates,
        "hint": "选择器未命中输入框;从 candidates[].selector 里选一个,或先 inspect(info=\"elements\", params.text=...) 查看可见元素",
    }))
}

/// 滚动:按像素(y 正向下)或滚动到选择器处。
pub async fn scroll(page: &Page, y: i64, selector: Option<&str>) -> Result<()> {
    let js = match selector {
        Some(sel) => format!(
            "(function() {{ var el = document.querySelector({}); if (el) el.scrollIntoView({{block:'center'}}); }})()",
            serde_json::to_string(sel).unwrap_or_default()
        ),
        None => format!("window.scrollBy(0, {y})"),
    };
    eval_js(page, &js).await?;
    Ok(())
}

/// 全页 PNG 截图落盘:返回 `(路径, 字节数, 宽, 高)`。
///
/// 信封只回元数据不回 base64 —— 实测(2026-10-23 双浏览器工具测试)单张 PNG
/// base64 约 1.7MB 直进消息流,30+ 轮后 LLM 调用 >120s 网关超时,
/// Reflection 根因:「截图的大体积二进制回包滞留上下文后,推理失去前进锚点」。
pub async fn screenshot_to_file(page: &Page, path: &str) -> Result<(String, usize, u32, u32)> {
    let b64 = screenshot_base64(page).await?;
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.trim())
        .map_err(|e| err("screenshot 解码", e))?;
    let (w, h) = png_size(&bytes).unwrap_or((0, 0));
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    std::fs::write(path, &bytes)
        .map_err(|e| err("screenshot 落盘", e))?;
    Ok((path.to_string(), bytes.len(), w, h))
}

/// 从 PNG 字节解析 IHDR 宽高(8B 签名 + 4B 长度 + 4B "IHDR" 之后)。
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((w, h))
}

/// 就绪判定:轮询选择器直到元素可见(上限 timeout_ms,上限 30s)。
/// 返回是否在超时前命中。
pub async fn wait_for_selector(page: &Page, selector: &str, timeout_ms: u64) -> Result<bool> {
    let budget = Duration::from_millis(timeout_ms.min(30_000).max(200));
    let start = std::time::Instant::now();
    let js = format!(
        r#"(function() {{
            var el = document.querySelector({sel});
            if (!el) return false;
            var r = el.getBoundingClientRect();
            var s = getComputedStyle(el);
            return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none';
        }})()"#,
        sel = serde_json::to_string(selector).unwrap_or_default()
    );
    loop {
        let v = eval_js(page, &js).await?;
        if v.as_bool().unwrap_or(false) {
            return Ok(true);
        }
        if start.elapsed() >= budget {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 等待固定毫秒(页面 settle 用)。
pub async fn wait(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms.min(30_000))).await;
}

/// 页面可见文本(`body.innerText`,截断由调用方按信封预算处理)。
pub async fn visible_text(page: &Page) -> Result<String> {
    let v = eval_js(page, "document.body ? document.body.innerText : ''").await?;
    Ok(v.as_str().unwrap_or_default().to_string())
}

/// 页面 HTML(`documentElement.outerHTML`)。
pub async fn outer_html(page: &Page) -> Result<String> {
    let v = eval_js(
        page,
        "document.documentElement ? document.documentElement.outerHTML : ''",
    )
    .await?;
    Ok(v.as_str().unwrap_or_default().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 选择器含引号时经 JSON 转义注入,不产生 JS 语法破坏。
    #[test]
    fn click_js_选择器被_json_转义() {
        let sel = "input[name=\"a'b\"]";
        let encoded = serde_json::to_string(sel).unwrap();
        // JSON 转义后:内层双引号变为 \",单引号原样保留。
        assert_eq!(encoded, r#""input[name=\"a'b\"]""#);
        let js = format!("document.querySelector({encoded})");
        assert!(js.contains(&encoded));
        // 绝不允许把未转义的选择器直接拼进 JS 串字面量。
        assert!(!js.contains("document.querySelector(input"));
    }
}
