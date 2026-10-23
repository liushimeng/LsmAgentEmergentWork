//! 输入动作 control_action 子模块(第 157 轮自 control.rs 拆分并重写)。
//!
//! 收纳「键盘/文本输入」一族(control.rs 已破 1800 行硬线,按 CLAUDE.md 拆分
//! 规范搬到职责子模块):`act_key_press` / `act_press_sequence` / `act_input_text`
//! / `act_human_input` / `act_clear_input` / `key_code_tuple`。
//!
//! 本轮功能性重写(实测文心一言任务 wf-4 因输入链路三烧 953 秒,DEBUG 日志
//! 29 处 `TypeError: Illegal invocation` 实锤,见
//! `tmpPlan/2026-10-12_MCP_Web_Chrome_Use输入链路根治与批量效率升级方案.md`):
//! - **原生赋值通用化**:弃用「HTMLInputElement 短路 || textarea」写法(该写法
//!   对 textarea 恒拿 input 的 value setter,品牌校验即 Illegal invocation),
//!   改为沿元素自身原型链查找 `value` 访问器;contenteditable 走 textContent;
//! - **input_text 自动阶梯**:JS 原生赋值 → 页内回读校验 → 未生效自动降级
//!   CDP sendkeys → 再未生效降级 human_input 慢速逐字;响应带 `mode`/`applied`/
//!   `value_after` 回读摘要与 `fallbacks` 降级轨迹(消灭 `use_js` 坑默认值);
//! - **键名规范化**:大小写不敏感 + 别名层(enter/esc/space/del/ins/pgup/方向键),
//!   支持组合写法 `ctrl+a`/`cmd+v`;Char 分支单字符防护(根治 CDP
//!   "Invalid 'text' parameter" 黑话);
//! - **press_sequence 聚焦**:支持 `selector`(先 click 聚焦再逐键,旧实现忽略
//!   selector 导致按键打进失焦页面、回显 ok 但输入框为空),逐键经统一规范化,
//!   响应带 `pressed_count`。

use super::control::parse_modifiers;
use super::{ensure_page, eval_js_string, js_str, str_arg};
use serde_json::{json, Value};

// ===================== 键名规范化(大小写不敏感 + 别名层) =====================

/// 单字符表:键名 → (CDP `key`, `code`, windowsVirtualKeyCode)。
///
/// 第 157 轮:匹配改大小写不敏感并补别名层 —— 实测 LLM 传小写 `enter` 不在旧表
/// 落入 Char 分支,5 字符文本触发 CDP `Invalid 'text' parameter` 黑话错误;
/// `ins`/`pgup`/`pgdn`/`spacebar`/`bs` 等同理。旧表全部写法(Enter/Return/Esc/
/// Del/Left…)经 lowercase 后仍命中,行为严格超集。
pub(super) fn key_code_tuple(name: &str) -> Option<(&'static str, &'static str, i64)> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "enter" | "return" | "\r" | "\n" => ("Enter", "Enter", 13),
        "tab" => ("Tab", "Tab", 9),
        "escape" | "esc" => ("Escape", "Escape", 27),
        "backspace" | "bs" => ("Backspace", "Backspace", 8),
        "delete" | "del" => ("Delete", "Delete", 46),
        "insert" | "ins" => ("Insert", "Insert", 45),
        "arrowleft" | "left" => ("ArrowLeft", "ArrowLeft", 37),
        "arrowup" | "up" => ("ArrowUp", "ArrowUp", 38),
        "arrowright" | "right" => ("ArrowRight", "ArrowRight", 39),
        "arrowdown" | "down" => ("ArrowDown", "ArrowDown", 40),
        "home" => ("Home", "Home", 36),
        "end" => ("End", "End", 35),
        "pageup" | "pgup" => ("PageUp", "PageUp", 33),
        "pagedown" | "pgdn" => ("PageDown", "PageDown", 34),
        "space" | "spacebar" | " " => (" ", "Space", 32),
        "f1" => ("F1", "F1", 112), "f2" => ("F2", "F2", 113), "f3" => ("F3", "F3", 114),
        "f4" => ("F4", "F4", 115), "f5" => ("F5", "F5", 116), "f6" => ("F6", "F6", 117),
        "f7" => ("F7", "F7", 118), "f8" => ("F8", "F8", 119), "f9" => ("F9", "F9", 120),
        "f10" => ("F10", "F10", 121), "f11" => ("F11", "F11", 122), "f12" => ("F12", "F12", 123),
        _ => return None,
    })
}

/// 组合键拆分(纯函数,可单测):`ctrl+a` / `cmd+shift+p` → (修饰段, 主键)。
///
/// 规则:按**最后一个** `+` 拆分(修饰段自身可含 `ctrl+shift`);尾部 `+`、
/// 开头 `+` 或主键为 `+` 视为字面加号(如 `shift+=` 表示加号,不拆);
/// 后半不是单字符也不是已知特殊键时整体按字面处理(防 `C++` 之类误拆)。
pub(super) fn split_combined_key(k: &str) -> (String, String) {
    let k = k.trim();
    if let Some(idx) = k.rfind('+') {
        if idx == 0 || idx == k.len() - 1 {
            return (String::new(), k.to_string());
        }
        let (mods, key_part) = (&k[..idx], &k[idx + 1..]);
        if key_part != "+" && (key_part.chars().count() == 1 || key_code_tuple(key_part).is_some()) {
            return (mods.to_string(), key_part.to_string());
        }
    }
    (String::new(), k.to_string())
}

/// 修饰键字符串(逗号或加号分隔)→ CDP modifiers 位掩码。
/// 与 `control::parse_modifiers` 同表(control.rs 内的版本从 params 取值,
/// 本函数服务组合写法拆分结果,语义保持一致)。
fn mods_from_str(s: &str) -> i64 {
    let mut bit = 0i64;
    for m in s.split(['+', ',']).map(|x| x.trim().to_ascii_lowercase()) {
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

/// 无法识别按键时的统一报错文案(给 LLM 可执行的修正方向,不再透传 CDP 黑话)。
const UNKNOWN_KEY_HINT: &str = "无法识别的按键;特殊键别名(大小写不敏感): \
enter/esc/tab/space/del/ins/pgup/pgdn/方向键(up/down/left/right)/home/end/f1-f12; \
组合写法如 ctrl+a、cmd+v;输入文本请用 input_text,连续按键请用 press_sequence";

// ===================== key_press / press_sequence =====================

pub(super) async fn act_key_press(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(k) = str_arg(p, "key") else { return Err("缺少 key".into()); };
    let page = ensure_page(id).await?;
    // 组合写法(第 157 轮):key="ctrl+a" 与 modifiers 参数可叠加(| 位或)。
    let (mods_str, key_part) = split_combined_key(k);
    let mods = parse_modifiers(p) | mods_from_str(&mods_str);
    if let Some((key, code, vk)) = key_code_tuple(&key_part) {
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
        // Char 分支单字符防护:CDP char 事件要求 text 恰为单字符,多字符直接报
        // "Invalid 'text' parameter" 黑话;这里前置拦截并给别名提示(第 157 轮)。
        let mut chars = key_part.chars();
        let Some(c) = chars.next() else { return Err(UNKNOWN_KEY_HINT.into()); };
        if chars.next().is_some() {
            return Err(format!("按键「{k}」不是单字符,不能按 char 事件发送;{UNKNOWN_KEY_HINT}"));
        }
        let s = c.to_string();
        let params = chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventParams::builder()
            .r#type(chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventType::Char)
            .text(s.clone()).key(s.clone()).modifiers(mods).build().map_err(|e| e.to_string())?;
        page.execute(params).await.map_err(|e| e.to_string())?;
        Ok(json!({"key": c.to_string(), "modifiers": mods, "mode":"char"}))
    }
}

pub(super) async fn act_press_sequence(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let keys_str = if let Some(arr) = p.get("keys").and_then(Value::as_array) {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        str_arg(p, "keys").unwrap_or("").to_string()
    };
    if keys_str.trim().is_empty() {
        return Err("缺少 keys".into());
    }
    // 第 157 轮:selector 先聚焦再逐键。旧实现忽略 selector,实测按键打进失焦
    // 页面(press_sequence 回 ok 但输入框为空),QC 误判「页面锁定拦截输入」。
    // click 走既有动作路径,守卫让路(先解后锁/先挂闸后复位)由外层 dispatch 统一承载。
    let focused = if let Some(sel) = str_arg(p, "selector") {
        super::control::act_click(id, &json!({"selector": sel}), false).await.is_ok()
    } else {
        false
    };
    let mut pressed: usize = 0;
    for token in keys_str.split_whitespace() {
        let (mods, key_part) = split_combined_key(token);
        if !mods.is_empty() || key_code_tuple(&key_part).is_some() || key_part.chars().count() == 1 {
            let mut sub = json!({"key": key_part});
            if !mods.is_empty() { sub["modifiers"] = json!(mods); }
            act_key_press(id, &sub).await?;
            pressed += 1;
        } else {
            // 多字符文本 token(实测 LLM 把整句中文当 keys 传入):逐字 Char 事件
            // 发送,等价 human_input 快路径 —— 旧实现直接报 CDP 黑话错。
            for ch in key_part.chars() {
                let sub = json!({"key": ch.to_string()});
                act_key_press(id, &sub).await?;
                pressed += 1;
            }
        }
    }
    Ok(json!({"pressed": pressed, "keys": keys_str, "focused": focused}))
}

// ===================== 原生赋值 JS(原型链 setter,受控组件通用) =====================

/// 沿元素自身原型链查找 `value` 访问器的赋值脚本(纯函数,可单测)。
///
/// 根治「HTMLInputElement 短路 || textarea」恒真短路:textarea 拿到 input 的
/// value setter,品牌校验抛 Illegal invocation(实测文心一言 29 次)。
/// contenteditable 元素走 textContent。返回 `'not_found'` / `'set'`。
pub(super) fn native_set_js(sel: &str, text_js: &str) -> String {
    format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) return 'not_found';
            if (el.isContentEditable) {{ el.textContent = {txt}; }}
            else {{
                let d = null, p = el;
                while (p) {{
                    const dd = Object.getOwnPropertyDescriptor(p, 'value');
                    if (dd) {{ d = dd; break; }}
                    p = Object.getPrototypeOf(p);
                }}
                if (d && d.set) {{ d.set.call(el, {txt}); }} else {{ el.value = {txt}; }}
            }}
            el.dispatchEvent(new Event('input', {{bubbles:true}}));
            el.dispatchEvent(new Event('change', {{bubbles:true}}));
            return 'set';
        }})()"#,
        sel = js_str(sel),
        txt = text_js,
    )
}

/// 输入回读脚本(纯函数,可单测):页内比对实际值与期望文本,只回传
/// len/match/head(100 字符),不把整段输入文本搬回上下文(大文本防护)。
/// 元素不存在返回 null。感知升级:LLM 当轮即知输入有没有真的生效。
pub(super) fn readback_js(sel: &str, expected_js: &str) -> String {
    format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) return null;
            const v = el.isContentEditable
                ? (el.innerText || el.textContent || '')
                : String(el.value ?? '');
            return {{ len: v.length, match: v === {exp}, head: v.slice(0, 100) }};
        }})()"#,
        sel = js_str(sel),
        exp = expected_js,
    )
}

/// 回读结果(Rust 侧)。
#[derive(Debug, Clone)]
pub(super) struct Readback {
    pub len: usize,
    pub matched: bool,
    pub head: String,
}

fn parse_readback(v: &Value) -> Option<Readback> {
    let o = v.as_object()?;
    Some(Readback {
        len: o.get("len").and_then(Value::as_u64).unwrap_or(0) as usize,
        matched: o.get("match").and_then(Value::as_bool).unwrap_or(false),
        head: o.get("head").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

/// 执行回读脚本:Ok(Some)=元素在场;Ok(None)=元素不存在;Err=eval 失败。
async fn readback(page: &chromiumoxide::Page, sel: &str, text: &str) -> std::result::Result<Option<Readback>, String> {
    let v = eval_js_string(page, &readback_js(sel, &js_str(text))).await?;
    Ok(if v.is_null() { None } else { parse_readback(&v) })
}

// ===================== input_text / human_input / clear_input =====================

pub(super) async fn act_input_text(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let Some(text) = str_arg(p, "text") else { return Err("缺少 text".into()); };
    // 第 157 轮:use_js 缺省 = auto(自动阶梯 + 回读驱动)。显式 true/false 仍
    // 强制单路径(向后兼容);旧默认 true 是坑 —— 实测 LLM 漏传时走 JS 路径在
    // 受控组件上必炸,LLM 还以为自己在用 CDP 原生输入。
    let use_js = p.get("use_js").and_then(Value::as_bool);
    let page = ensure_page(id).await?;
    let mut fallbacks: Vec<String> = Vec::new();

    // 级别 1:JS 原生赋值(原型链 setter)+ 页内回读校验。
    if use_js.unwrap_or(true) {
        match js_set_text(&page, sel, text).await {
            Ok(Some(rb)) if rb.matched => {
                return Ok(input_text_result(sel, text, "js", &rb, &fallbacks));
            }
            Ok(Some(rb)) => {
                fallbacks.push(format!("js:值未生效(len={},head={:?})", rb.len, rb.head));
            }
            Ok(None) => {
                fallbacks.push("js:selector 未命中".to_string());
            }
            Err(e) => {
                fallbacks.push(format!("js:执行失败({e})"));
            }
        }
    }
    if use_js == Some(true) {
        return Err(format!(
            "input_text(use_js=true) 未生效:{};可去掉 use_js 走自动阶梯,或 inspect(info=elements) 重探输入框",
            fallbacks.join("; ")
        ));
    }

    // 级别 2:CDP sendkeys(click 聚焦 + type_str,真实键事件,React 受控组件兼容)。
    match sendkeys_text(&page, sel, text).await {
        Ok(Some(rb)) if rb.matched => {
            fallbacks.push("sendkeys".to_string());
            return Ok(input_text_result(sel, text, "sendkeys", &rb, &fallbacks));
        }
        Ok(Some(rb)) => {
            fallbacks.push(format!("sendkeys:值未生效(len={},head={:?})", rb.len, rb.head));
        }
        Ok(None) => {
            fallbacks.push("sendkeys:selector 未命中".to_string());
        }
        Err(e) => {
            fallbacks.push(format!("sendkeys:执行失败({e})"));
        }
    }
    if use_js == Some(false) {
        return Err(format!(
            "input_text(use_js=false) 未生效:{};可去掉 use_js 走自动阶梯(含 human_input 慢速逐字兜底)",
            fallbacks.join("; ")
        ));
    }

    // 级别 3:human_input 慢速逐字(50~200ms 间隔,最接近真实键入,最后兜底)。
    match human_text(&page, sel, text).await {
        Ok(Some(rb)) if rb.matched => {
            fallbacks.push("human".to_string());
            return Ok(input_text_result(sel, text, "human", &rb, &fallbacks));
        }
        Ok(Some(rb)) => {
            fallbacks.push(format!("human:值未生效(len={},head={:?})", rb.len, rb.head));
        }
        Ok(None) => {
            fallbacks.push("human:selector 未命中".to_string());
        }
        Err(e) => {
            fallbacks.push(format!("human:执行失败({e})"));
        }
    }
    Err(format!(
        "输入三级阶梯均未生效(selector={sel}):{};页面可能拦截合成输入。建议 inspect(info=elements) 重探输入框真实选择器,或 request_human 请人工输入",
        fallbacks.join("; ")
    ))
}

fn input_text_result(sel: &str, text: &str, mode: &str, rb: &Readback, fallbacks: &[String]) -> Value {
    json!({
        "selector": sel,
        "text_len": text.chars().count(),
        "mode": mode,
        "applied": rb.matched,
        "value_after": {"len": rb.len, "matched": rb.matched, "head": rb.head},
        "fallbacks": fallbacks,
    })
}

/// JS 级别:原生赋值 + 回读。Ok(Some)=回读到;Ok(None)=元素不存在;Err=eval 失败。
async fn js_set_text(page: &chromiumoxide::Page, sel: &str, text: &str) -> std::result::Result<Option<Readback>, String> {
    let r = eval_js_string(page, &native_set_js(sel, &js_str(text))).await?;
    if r.as_str() == Some("not_found") {
        return Ok(None);
    }
    readback(page, sel, text).await
}

/// 输入前清空既有值(fail-open):降级键入路径是**追加**语义,若输入框里已有
/// 旧文本(搜索框历史词、筛选框残留)会拼成「旧词+新词」导致回读永不 matched,
/// 三级阶梯白烧。JS 原生赋值本身就是整体替换,故只补 sendkeys / human 两条。
async fn clear_before_typing(page: &chromiumoxide::Page, sel: &str) {
    let _ = eval_js_string(page, &native_set_js(sel, "''")).await;
}

/// sendkeys 级别:find_element + click 聚焦 + 清空既有值 + type_str 真实键事件。
async fn sendkeys_text(page: &chromiumoxide::Page, sel: &str, text: &str) -> std::result::Result<Option<Readback>, String> {
    let el = page.find_element(sel.to_string()).await.map_err(|e| e.to_string())?;
    el.click().await.map_err(|e| e.to_string())?;
    clear_before_typing(page, sel).await;
    el.type_str(text.to_string()).await.map_err(|e| e.to_string())?;
    readback(page, sel, text).await
}

/// human 级别:click 聚焦 + 清空既有值 + 逐字 Char 事件 + 50~200ms 随机间隔(类真实键入)。
async fn human_text(page: &chromiumoxide::Page, sel: &str, text: &str) -> std::result::Result<Option<Readback>, String> {
    if let Ok(el) = page.find_element(sel.to_string()).await {
        let _ = el.click().await;
    } else {
        return Ok(None);
    }
    clear_before_typing(page, sel).await;
    for ch in text.chars() {
        let s = ch.to_string();
        let params = chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventParams::builder()
            .r#type(chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventType::Char)
            .text(s.clone()).key(s.clone()).build().map_err(|e| e.to_string())?;
        let _ = page.execute(params).await;
        let delay = 50 + (ch as u32 % 150);
        tokio::time::sleep(std::time::Duration::from_millis(delay as u64)).await;
    }
    readback(page, sel, text).await
}

pub(super) async fn act_human_input(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let text = p.get("text").and_then(Value::as_str).unwrap_or("");
    let page = ensure_page(id).await?;
    human_text(&page, sel, text).await?;
    let after = readback(&page, sel, text).await.ok().flatten();
    Ok(json!({
        "selector": sel,
        "text": text,
        "mode": "human",
        "applied": after.as_ref().map(|b| b.matched).unwrap_or(true),
        "value_after": after.map(|b| json!({"len": b.len, "matched": b.matched, "head": b.head})).unwrap_or(Value::Null),
    }))
}

pub(super) async fn act_clear_input(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let page = ensure_page(id).await?;
    // 第 157 轮:复用原型链 setter(根治与 input_text 同源的 Illegal invocation)+ 回读。
    let r = eval_js_string(&page, &native_set_js(sel, "''")).await?;
    if r.as_str() == Some("not_found") {
        return Err(format!("selector 未命中:{sel};建议 inspect(info=elements) 重探"));
    }
    let rb = readback(&page, sel, "").await?;
    let cleared = rb.as_ref().map(|b| b.len == 0).unwrap_or(false);
    Ok(json!({"cleared": cleared, "selector": sel, "len_after": rb.map(|b| b.len).unwrap_or(0)}))
}
