//! MCP_Web_Use action=control:全部写操作统一入口(自 tools/browser.rs 平移)。
//!
//! `control_action` 枚举对应原 BrowserControl 的 `action` 字段(35 个);
//! 动作参数集中在 `params` 对象;派生标签页经响应 `spawned_page_id` 回传。

use base64::Engine;
use serde_json::{json, Value};

use super::*;
// 第 135 轮:返回值净化与 data: URL 落盘已独立到 eval_sanitize.rs(复合值体积闸门),
// 这里再导出一次保持既有 `control::fn` 引用路径不变(测试与兄弟模块都在用)。
#[allow(unused_imports)]
pub(super) use super::eval_sanitize::{
    data_ext_from_mime, data_mime, decode_data_url, percent_decode, sanitize_eval_result,
    EVAL_INLINE_CHAR_LIMIT,
};
// 第 143 轮:页面管控 control_action(让路辅助 + set_overlay/set_guard)拆分至
// guard_ctl.rs(control.rs 逼近 1800 行硬线),机械搬移零改写。
use super::guard_ctl::{
    act_set_guard, act_set_overlay, guard_lift_for_input, guard_restore_after_input,
};

// =================== 第 99 轮:eval_js / screenshot / download 效能与健壮性辅助 ===================

/// eval_js 表达式是否已是函数形态(IIFE / 箭头函数 / function 声明)——
/// 失败时不再包一层 IIFE(包裹只会把返回值变成 undefined)。
pub(super) fn looks_like_function(expr: &str) -> bool {
    let t = expr.trim_start();
    t.starts_with('(') || t.starts_with("async") || t.starts_with("function")
}

/// 表达式是否为**匿名函数声明**(第 135 轮)。
///
/// `function(){ … }` 在 JS 里是「函数语句」,必须有函数名才能作为语句出现,
/// 直接交给 `Runtime.evaluate` 必然报
/// `SyntaxError: Function statements require a function name`。
///
/// 实测事故(`llaew_20261008_173357.log` 36 氪任务 L363):模型写了
/// `function(){ var d = window.initialState && … }`,旧实现在 `looks_like_function`
/// 命中 `function` 前缀而**跳过自动包裹**,直接上抛 SyntaxError,并顺带
/// **凭空 spawn 了一个野页面** `p_89de574e`(全程 0 次引用,最后被 `close(all)`
/// 顺手清掉)。这类形态应当包成 `(fn)()` 调用,而不是当成已成型函数。
pub(super) fn is_anonymous_function_decl(expr: &str) -> bool {
    let t = expr.trim_start();
    let Some(rest) = t.strip_prefix("function") else {
        return false;
    };
    // `function name(...)` 是有名字的函数声明/表达式,不是匿名语句形态;
    // `function*` / `function (` / `function{` 才是匿名函数。
    let next = rest.trim_start().chars().next();
    !matches!(next, Some(c) if c.is_alphanumeric() || c == '_' || c == '$')
}

/// eval_js 单行错误摘要(原始错误常带多行 stack,只留首行截断)。
fn err_head(e: &str) -> String {
    let first = e.lines().next().unwrap_or(e);
    let mut out: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        out.push('…');
    }
    out
}

/// 把 data: URL 内容直接落盘(download 快速路径,不走 CDP 下载事件 ——
/// Chrome 对 data: 锚点点击不产生可靠下载事件,实测 about:blank/data: 会挂到超时)。
async fn save_data_url_file(s: &str, p: &Value) -> std::result::Result<Value, String> {
    let bytes = decode_data_url(s)
        .ok_or("data: URL 解析失败(仅支持 base64 或百分号编码载荷)")?;
    let base = match str_arg(p, "save_dir").filter(|s| !s.is_empty()) {
        Some(dir) => {
            let d = std::path::PathBuf::from(dir);
            tokio::fs::create_dir_all(&d)
                .await
                .map_err(|e| format!("创建下载目录失败({}):{e}", d.display()))?;
            d
        }
        None => std::env::temp_dir(),
    };
    let filename = str_arg(p, "filename").map(str::to_string).unwrap_or_else(|| {
        format!(
            "laew_download_{}.{}",
            now_millis_safe(),
            data_ext_from_mime(data_mime(s))
        )
    });
    let safe_name = std::path::Path::new(&filename)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .ok_or_else(|| format!("非法下载文件名:{filename}"))?;
    let target = base.join(safe_name);
    tokio::fs::write(&target, &bytes)
        .await
        .map_err(|e| format!("写入 data: URL 内容失败({}):{e}", target.display()))?;
    Ok(json!({
        "save_path": target.display().to_string(),
        "byte_size": bytes.len(),
        "mode": "data_url",
    }))
}

/// OCR 截图 PNG 文件(macOS Vision;图像来自 CDP 截图字节,不触碰 CGWindow,
/// 无需屏幕录制权限)。region=(x,y,w,h) 时只保留中心点落入区域的词块。
/// 第 106 轮:增强错误诊断,返回结构化 ocr_error 信息帮助 LLM 决策下一步。
#[cfg(target_os = "macos")]
fn ocr_png_text(
    path: &std::path::Path,
    region: Option<(i64, i64, i64, i64)>,
) -> std::result::Result<(String, usize), String> {
    let blocks = crate::agent::window::macos_vision_ocr::ocr_png_file(path, None)
        .map_err(|e| {
            let err_str = e.to_string();
            // 第 106 轮:分类错误类型,给出明确指导
            if err_str.contains("Swift") || err_str.contains("compile") {
                format!("OCR 失败:Swift 脚本编译错误({err_str})。Vision 框架不可用,请停止 OCR 尝试,直接标注需要人工输入验证码")
            } else if err_str.contains("permission") || err_str.contains("Permission") {
                format!("OCR 失败:权限不足({err_str})。请停止 OCR 尝试,直接标注需要人工输入验证码")
            } else if err_str.contains("file") || err_str.contains("No such file") {
                format!("OCR 失败:截图文件不存在({err_str})。请先 screenshot 落盘再 OCR")
            } else {
                format!("OCR 失败:({err_str})。已无更多 OCR 路径,请停止 OCR 尝试,直接标注需要人工输入验证码")
            }
        })?;
    let kept: Vec<_> = blocks
        .into_iter()
        .filter(|b| match region {
            Some((x, y, w, h)) => {
                let cx = b.x + b.width / 2;
                let cy = b.y + b.height / 2;
                cx >= x && cx < x + w && cy >= y && cy < y + h
            }
            None => true,
        })
        .collect();
    let n = kept.len();
    let text = kept
        .iter()
        .map(|b| b.text.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((text, n))
}

/// 非 macOS 平台:OCR 暂不可用的结构化提示(保持动作面一致,LLM 据此如实上报)。
#[cfg(not(target_os = "macos"))]
fn ocr_png_text(
    _path: &std::path::Path,
    _region: Option<(i64, i64, i64, i64)>,
) -> std::result::Result<(String, usize), String> {
    Err(
        "OCR 当前仅 macOS 支持(Vision 框架);验证码等图片文字请改在 macOS 环境执行,或人工提供文字内容,不要猜测".to_string(),
    )
}

/// 从 params 提取 region{x,y,width,height}(容忍 w/h 简写)。
fn parse_region(p: &Value) -> Option<(i64, i64, i64, i64)> {
    let r = p.get("region")?;
    let x = r.get("x")?.as_i64()?;
    let y = r.get("y")?.as_i64()?;
    let w = r
        .get("width")
        .or_else(|| r.get("w"))?
        .as_i64()?;
    let h = r
        .get("height")
        .or_else(|| r.get("h"))?
        .as_i64()?;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((x, y, w, h))
}

/// control 分发入口。
pub(super) async fn run(args: Value) -> crate::error::Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    let Some(action) = str_arg(&args, "control_action") else {
        return envelope(1001, "缺少 control_action", json!({}));
    };
    let params = args.get("params").cloned().unwrap_or_else(|| json!({}));
    // 第 128 轮:任务锚点守卫 —— `navigate` / `new_tab` 是显式带 URL 的「主动选择目标站点」
    // 动作,与 `open` 同源(实测漂移发生点),必须同门把守。
    // 需要特殊信封(6001),不走通用 2002 映射,否则 LLM 会把越界当成「换 selector 重试」类
    // 可恢复动作继续尝试(2002 的既有对策语义),而不是停下报告失败。
    // 点击导航不在此列:落地页无法预知,且从锚点站点出发的外链跳转是合法任务形态,
    // 由 target_drift 信号 + QC 目标一致性门事后对账。
    if matches!(action, "navigate" | "new_tab") {
        if let Some(url) = str_arg(&params, "url") {
            if let Some(blocked) = super::target_anchor_guard(url) {
                return blocked;
            }
        }
    }
    // 第 141 轮:蒙层输入锁「先解后锁」—— 输入类动作(经 CDP Input.dispatch*
    // 或 chromiumoxide Element click/type)在蒙层激活时会被 setIgnoreInputEvents
    // 一并吃掉(实测,见 browser_overlay 模块文档),动作前解锁、动作后复锁。
    // 第 143 轮扩展:partial 档下同类动作会被 DOM 盾罩吃掉(hit-test 层 CDP 合成
    // 输入与真实输入不可区分),改为「先隐盾后复盾」。窗口为毫秒级;管控未激活
    // 或 open 档时零开销(不多发任何 CDP/JS 往返)。
    let guard_lift = if crate::agent::browser_overlay::action_dispatches_input(&action) {
        guard_lift_for_input(id).await
    } else {
        None
    };
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
        "download" => act_download(id, &params).await,
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
        // 第 100 轮:窗口可视化 + 人工介入(HITL)
        "set_window" => act_set_window(id, &params).await,
        "sync_viewport" => act_sync_viewport(id, &params).await,
        "set_highlight" => act_set_highlight(id, &params).await,
        // 第 141 轮:可视化蒙层运行时开关(legacy,委托 set_guard)
        "set_overlay" => act_set_overlay(id, &params).await,
        // 第 143 轮:页面管控三档运行时切换
        "set_guard" => return act_set_guard(id, &params).await,
        // request_human 需要特殊信封(4001/4002),不走通用 2002 映射
        "request_human" => return act_request_human(id, &params).await,
        other => return envelope(1001, "未知 control_action", json!({"control_action": other})),
    };
    // 动作收尾:复锁输入 / 复盾(仅刚才让路过路的页面)
    if let Some((page, kind)) = guard_lift {
        guard_restore_after_input(&page, kind).await;
    }
    // 第 143 轮:导航类动作产生新文档 —— CDP 输入拦截实测跨导航持久,管控 JS 按
    // 注入时烘焙的配置重建;运行期 set_guard 改档后再导航,脚本会以旧档回归。
    // 用实例期望态 re-assert,保证各层收敛(仅在成功导航后,headed 非 connect)。
    if result.is_ok()
        && crate::agent::browser_overlay::action_needs_overlay_reassert(&action)
        && crate::agent::browser::BrowserManager::global().guard_visuals_active().await
    {
        let mgr = crate::agent::browser::BrowserManager::global();
        let cfg = mgr.current_guard().await;
        if let Some(page) = mgr.page(id).await {
            let _ = crate::agent::browser_overlay::apply_page_guard(&page, &cfg).await;
        }
    }
    // 第 140 轮提速:仅「可能派生新标签页」的动作才做浏览器级 pages() 巡检
    // (每次巡检 = 一次浏览器级 CDP 往返 + 全局锁)。wait/input_text/screenshot/
    // eval_js(纯读)/set_* 等动作不会开新页,批量 sequence 一次可省 N 次巡检。
    // 名单按「动作可能触发浏览器开新 tab/window」保守圈定:点击(链接/target=_blank)、
    // 键盘(Enter 提交表单跳转)、导航类、select/upload(表单提交)、dispatch_event(自定义事件)。
    let spawned = if action_may_spawn_page(action) {
        crate::agent::browser::BrowserManager::global().adopt_spawned_pages().await
    } else {
        Vec::new()
    };
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

/// 该 control_action 是否可能派生新标签页/窗口(第 140 轮)。
///
/// 用途:决定动作收尾是否跑 `adopt_spawned_pages()`(浏览器级 `Target.getTargets`
/// CDP 往返 + 全局锁)。保守圈定「点击 / 键盘 / 导航 / 表单提交 / 自定义事件」——
/// 这些可能经 `<a target=_blank>`、`window.open`、表单跳转开新页;观察类与纯
/// 状态设置类动作(wait/screenshot/eval_js/set_*)不会开页,跳过巡检零风险。
pub(super) fn action_may_spawn_page(action: &str) -> bool {
    matches!(
        action,
        "click"
            | "human_click"
            | "right_click"
            | "double_click"
            | "key_press"
            | "press_sequence"
            | "navigate"
            | "new_tab"
            | "back"
            | "forward"
            | "reload"
            | "select_option"
            | "upload_file"
            | "download"
            | "dispatch_event"
            | "eval_js"
    )
}

// =================== 第 141/143 轮:管控让路辅助已拆分至 guard_ctl.rs ===================
// (control.rs 逼近 1800 行硬线,按拆分规范机械搬移;此处仅保留引用)

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
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    }
    dispatch_mouse(&page, "mousePressed", x, y, "left", 1, 0, 0).await?;
    // 第 140 轮:按下→抬起 40ms→15ms(合成事件无须拟人停顿,页面 JS 的
    // mousedown/up 处理在同帧内完成,15ms 足够事件队列排空)。
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
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

/// 下载文件:支持直接 URL 或点击页面上的下载链接/按钮。
///
/// BrowserManager 会先配置 Chrome 下载目录并监听 Browser 域下载事件,再触发动作;
/// 完成后返回绝对 `save_path` 与字节数。`filename` 只允许文件名,不能携带路径。
///
/// 第 99 轮:
/// - `data:` URL 直接解码落盘(不走 CDP 下载事件 —— Chrome 对 data: 锚点不产生
///   可靠下载事件,实测会挂到超时);
/// - `about:` / `blob:` / `javascript:` URL 快速失败(不会产生下载事件,只会白等)。
async fn act_download(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let url = str_arg(p, "url");
    let selector = str_arg(p, "selector");
    if url.is_none() && selector.is_none() {
        return Err("缺少 url 或 selector".into());
    }
    if let Some(u) = url {
        let lower = u.trim().to_ascii_lowercase();
        if lower.starts_with("data:") {
            return save_data_url_file(u, p).await;
        }
        for scheme in ["about:", "blob:", "javascript:"] {
            if lower.starts_with(scheme) {
                return Err(format!(
                    "不支持从 {scheme} URL 触发下载(Chrome 不产生下载事件,只会等待到超时):请提供 http(s) 直链,或用 selector 点击页面上的下载元素"
                ));
            }
        }
    }
    crate::agent::browser::BrowserManager::global()
        .download(
            id,
            url,
            selector,
            str_arg(p, "save_dir"),
            str_arg(p, "filename"),
            p.get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(120_000),
        )
        .await
}

async fn act_new_tab(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(url) = str_arg(p, "url") else { return Err("缺少 url".into()); };
    let page = ensure_page(id).await?;
    let js = format!("window.open({url}, '_blank')", url = js_str(url));
    let _ = eval_js_string(&page, &js).await;
    // 第 146 轮:访问台账记账 + 重复访问警示(新标签页 page_id 由 adopt 机制另行回传)
    super::visit_ledger::record_visit(id, url, "", super::visit_ledger::VisitSource::NewTab);
    let mut out = json!({"opened": url});
    if let Some(note) = super::visit_ledger::visit_note(url) {
        out["visit_note"] = note;
    }
    Ok(out)
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
    // 第 146 轮:访问台账记账(记**目标 URL**而非跳转后 URL —— 覆盖度关心
    // 「去了哪个路由」,登录重定向不稀释覆盖)+ 重复访问警示
    super::visit_ledger::record_visit(id, url, "", super::visit_ledger::VisitSource::Navigate);
    let mut out = json!({"url": url});
    if let Some(note) = super::visit_ledger::visit_note(url) {
        out["visit_note"] = note;
    }
    Ok(out)
}

async fn act_history(id: &str, back: bool) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    let js = if back { "history.back()" } else { "history.forward()" };
    eval_js_string(&page, js).await?;
    // 第 140 轮:300ms→120ms —— history API 异步生效但 popstate 在 100ms 内
    // 必达,后续 inspect 的等待语义由调用方自行保证。
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
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
    // 第 99 轮:参数别名扩展 —— 实测 LLM 常猜 function/js/code,一并接受,
    // 消除「参数名摸索」迭代黑洞;缺失时错误信息列出全部合法别名。
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
        else if let Some(f) = str_arg(p, "function") { f.to_string() }
        else if let Some(j) = str_arg(p, "js") { j.to_string() }
        else if let Some(c) = str_arg(p, "code") { c.to_string() }
        else {
            return Err("缺少 JS 代码:params.expression(或别名 function/js/code,均支持 _b64 变体)".into());
        };
    let await_promise = p.get("await_promise").and_then(Value::as_bool).unwrap_or(false);
    use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
    let evaluate = |expr: String| {
        let mut b = EvaluateParams::builder()
            .expression(expr)
            .return_by_value(true);
        if await_promise { b = b.await_promise(true); }
        b.build().map_err(|e| e.to_string())
    };
    // 第 99 轮:return / 多语句容错 —— 首次按原样评估;失败且表达式不是函数形态时
    // 自动包 IIFE(async 语境包 async IIFE)重试一次;仍失败则双错误上抛。
    //
    // 第 135 轮:匿名 `function(){…}` 语句先就地转成 `(function(){…})()` 调用再评估 ——
    // 该形态必然 SyntaxError,旧实现误判为「已是函数形态」而跳过包裹,白烧一轮迭代
    // 还凭空派生一个野页面。
    let expr = if is_anonymous_function_decl(&expr) {
        format!("(\n{expr}\n)()")
    } else {
        expr
    };
    let first = evaluate(expr.clone());
    let (built, wrapped, first_err) = match first {
        Ok(params) => (params, false, None),
        Err(e1) => {
            if looks_like_function(&expr) {
                return Err(format!("eval_js 失败: {}", err_head(&e1)));
            }
            let wrapped_expr = if await_promise {
                format!("(async()=>{{\n{expr}\n}})()")
            } else {
                format!("(()=>{{\n{expr}\n}})()")
            };
            match evaluate(wrapped_expr) {
                Ok(params) => (params, true, Some(e1)),
                Err(e2) => {
                    return Err(format!(
                        "eval_js 失败(原始与 IIFE 包裹均失败): ① {} ② {}",
                        err_head(&e1),
                        err_head(&e2)
                    ));
                }
            }
        }
    };
    let v = page.evaluate(built).await.map_err(|e| e.to_string())?;
    let raw = v.value().cloned().unwrap_or(Value::Null);
    // 第 99 轮:大结果 / data: URL 自动落盘,防上下文爆炸(实测 140KB 验证码
    // data-url 直灌上下文,还诱发后续工具参数超限被截断)。
    let val = sanitize_eval_result(raw);
    let mut out = json!({"result": val, "await_promise": await_promise});
    if wrapped {
        out["wrapped_iife"] = json!(true);
        if let Some(e1) = first_err {
            out["first_error"] = json!(err_head(&e1));
        }
    }
    Ok(out)
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

/// 截图(control 与 inspect 的 screenshot 共用)。
///
/// 第 99 轮改造:
/// - **一律落盘**(save_path 指定路径,否则临时目录),不再内联 base64 ——
///   文本模型无法消费图片,base64 内联纯属上下文浪费;`return_base64=true`
///   显式开启才内联;
/// - `ocr=true` 时对截图跑 OCR(macOS Vision),返回 `ocr_text`/`ocr_block_count`;
///   `region{x,y,width,height}` 过滤词块(只影响 OCR 输出,落盘仍为整帧)。
pub(super) async fn act_screenshot(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let page = ensure_page(id).await?;
    // 第 143 轮:截图避让升级 —— headed 非 connect(注入了管控脚本)时拍前挂起、
    // 拍完恢复全部管控视觉(蒙层+盾区+状态条,均不进证据链,OCR/看图/HITL 附图
    // 不受遮罩与提示条干扰)。只动视觉层,不动 CDP 输入锁。
    let visuals_hidden = crate::agent::browser::BrowserManager::global()
        .guard_visuals_active()
        .await
        && crate::agent::browser_overlay::guard_suspend_visuals(&page, true).await;
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
    let shot = page.screenshot(b.build()).await;
    if visuals_hidden {
        // 失败路径也要恢复(视觉残留会污染后续所有截图)
        let _ = crate::agent::browser_overlay::guard_suspend_visuals(&page, false).await;
    }
    let bytes = shot.map_err(|e| e.to_string())?;
    let path = match p.get("save_path").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        Some(custom) => custom.to_string(),
        None => std::env::temp_dir()
            .join(format!(
                "laew_web_{}.{}",
                now_millis_safe(),
                if format_str.eq_ignore_ascii_case("jpeg") || format_str.eq_ignore_ascii_case("jpg") { "jpg" } else { "png" }
            ))
            .display()
            .to_string(),
    };
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
    let mut out = json!({"save_path": path, "byte_size": bytes.len(), "format": format_str});
    // 第 125 轮:非整页截图时检测内容是否超出视口(显示不全的直接信号),
    // 溢出则附 content_overflow + 对策 hint;测量失败静默跳过(fail-open)。
    if !full_page {
        if let Ok(m) =
            crate::agent::browser::page_viewport_metrics(&page).await
        {
            let g = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            let (vw, vh, cw, ch) = (g("innerWidth"), g("innerHeight"), g("scrollWidth"), g("scrollHeight"));
            if cw > vw + 2.0 || ch > vh + 2.0 {
                out["content_overflow"] = json!({
                    "viewport": {"width": vw, "height": vh},
                    "content": {"width": cw, "height": ch},
                    "horizontal": cw > vw + 2.0,
                    "vertical": ch > vh + 2.0,
                    "hint": "本截图仅覆盖视口区域,页面内容超出:整页捕获用 params.full_page=true;视口过窄可重开 open(默认自动扩展视口到 2K)或 control(set_viewport)",
                });
            }
        }
    }
    // OCR(默认关;验证码/图表标签等图片文字读取)
    if p.get("ocr").and_then(Value::as_bool).unwrap_or(false) {
        match ocr_png_text(std::path::Path::new(&path), parse_region(p)) {
            Ok((text, n)) => {
                out["ocr_text"] = json!(text);
                out["ocr_block_count"] = json!(n);
            }
            Err(e) => {
                out["ocr_error"] = json!(e);
                // 第 131 轮:OCR 走到尽头 → 工具返回层直接给出人工介入的下一步,
                // 不再依赖模型自己从系统提示词第 13/16 条里回忆规则。
                if let Some(hint) = super::ocr_unavailable_hint(&out) {
                    super::merge_hint(&mut out, hint);
                }
            }
        }
    }
    if p.get("return_base64").and_then(Value::as_bool).unwrap_or(false) {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        out["image_base64"] = json!(b64);
    }
    Ok(out)
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
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    dispatch_mouse(&page, "mousePressed", sx, sy, "left", 1, 0, 0).await?;
    // 8 步插值,每步 8ms(第 140 轮 15ms→8ms:滑块轨迹形状保留,总时长减半;
    // 轨迹形态是防检测要素,步数与插值曲线不动)
    for step in 1..=8 {
        let t = step as f64 / 8.0;
        let mx = sx + (dx - sx) * t;
        let my = sy + (dy - sy) * t;
        dispatch_mouse(&page, "mouseMoved", mx, my, "left", 0, 0, 0).await?;
        tokio::time::sleep(std::time::Duration::from_millis(8)).await;
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
    // 第 140 轮:selector 命中时一次性取中心点坐标(x/y 各探测一次是纯浪费,
    // 每次探测 = 一次 JS eval)。
    let (x, y) = if let Some(sel) = str_arg(p, "selector") {
        let nth = p.get("nth").and_then(Value::as_u64).unwrap_or(0) as usize;
        let find = eval_find_center(&page, sel, nth).await?;
        (
            find["x"].as_f64().unwrap_or_default(),
            find["y"].as_f64().unwrap_or_default(),
        )
    } else {
        (
            p.get("x").and_then(Value::as_f64).unwrap_or_default(),
            p.get("y").and_then(Value::as_f64).unwrap_or_default(),
        )
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

// =================== 第 100 轮:窗口可视化 + 人工介入(HITL) ===================

/// control_action=set_window:运行时调整真实浏览器窗口位置/尺寸/状态。
///
/// 参数(width/height/left/top/window_state)全部可选,至少给一个;
/// window_state ∈ {normal, maximized, minimized, fullscreen},状态与几何互斥
/// (CDP 协议约束)。调整后自动清除视口覆盖 → 渲染=窗口内容区,不缺区域。
async fn act_set_window(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let width = p.get("width").and_then(Value::as_i64);
    let height = p.get("height").and_then(Value::as_i64);
    let left = p.get("left").and_then(Value::as_i64);
    let top = p.get("top").and_then(Value::as_i64);
    let window_state = str_arg(p, "window_state");
    if width.is_none()
        && height.is_none()
        && left.is_none()
        && top.is_none()
        && window_state.is_none()
    {
        return Err("至少提供 width/height/left/top/window_state 之一".into());
    }
    for (name, v) in [("width", width), ("height", height)] {
        if let Some(n) = v {
            if !(320..=7680).contains(&n) {
                return Err(format!("{name} 超出安全区间(320~7680):{n}"));
            }
        }
    }
    crate::agent::browser::BrowserManager::global()
        .set_window_bounds(id, width, height, left, top, window_state)
        .await
}

/// control_action=sync_viewport:人工拖动浏览器窗口大小后,清除 device metrics
/// 覆盖使视口自适应窗口内容区(消除黑边/缺区域/渲染不全)。
async fn act_sync_viewport(id: &str, _: &Value) -> std::result::Result<Value, String> {
    crate::agent::browser::BrowserManager::global().sync_viewport(id).await
}

/// control_action=set_highlight:运行时开关 Agent 高亮蓝框。
async fn act_set_highlight(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    crate::agent::browser::BrowserManager::global()
        .set_highlight(id, enabled)
        .await
}

/// request_human 的 reason → 默认文案与展示标签。
/// 第 130 轮收口到 `human_assist::{kind_label, default_message}` 单一事实源
/// (弹窗 / TUI / 工具侧三处共用);完整合法 reason 列表见
/// [`HUMAN_ASSIST_ALLOWED_REASONS`],新增 reason 同步更新那边 + 校验顺序 + 文档。
fn human_assist_defaults(reason: &str) -> (&'static str, &'static str) {
    (
        crate::agent::human_assist::kind_label(reason),
        crate::agent::human_assist::default_message(reason),
    )
}

/// request_human 合法 reason 集合(用于参数校验与对外暴露给 LLM)。
/// 顺序即为推荐使用顺序;新增 reason 必须同步更新此处 + TUI 标签 + 文档。
pub const HUMAN_ASSIST_ALLOWED_REASONS: &[&str] = &[
    "captcha",
    "sms",
    "qr_login",
    "login",
    "real_name",
    "two_factor",
    "oauth",
    "manual_verify",
    "custom",
];

/// 第 133 轮:附图决策与采集迁入 `captcha_crop` 子模块(验证码元素裁剪优先，
/// 视口兜底);此处再导出保持 `control::hitl_image_plan` 外部路径零改动。
pub use super::captcha_crop::{hitl_image_plan, HitlImagePlan};

/// 把 reason 列表渲染成斜杠分隔字符串(用于错误消息/工具 Schema 描述)。
pub fn human_assist_reasons_doc() -> String {
    HUMAN_ASSIST_ALLOWED_REASONS.join("/")
}

/// 应用 HITL 提问期间的临时放行态(第 144 轮,纯应用不落期望态)。
///
/// - `allow` 非空:**partial + 白名单挖洞**(账号/密码/验证码等人工必填区域成为
///   非屏蔽区域,其余保持 Agent 管控,最小权限;状态条第三行引导人工);
/// - `allow` 为空:整页 open(第 143 轮行为,探测不到凭证区的回退档)。
///
/// 第 145 轮:两档的引导 note 都指明收口动作「回到弹窗点『提交 / 继续』」——
/// 管控三档(locked/open/partial)只在 Agent 全面操作时生效;HITL 期间是临时放行态,
/// 人工完成操作后的交棒动作必须显性可见(实测反馈:整页 open 回退档只有通用
/// 「页面开放」状态条,人工不知道做完要去弹窗点继续)。
///
/// 成功返回导航重放协程句柄(提问期间人工提交登录触发导航时重放临时态);
/// 应用失败返回 None(等同未解锁,提问照常进行,fail-open)。
async fn apply_hitl_unlock(
    page: &chromiumoxide::Page,
    allow: &[String],
) -> Option<tokio::task::JoinHandle<()>> {
    use crate::agent::browser_overlay::{apply_page_guard, PageGuardConfig};
    let cfg = if allow.is_empty() {
        let mut c = PageGuardConfig::open();
        c.note = Some("人工介入中·页面临时开放,完成后回弹窗点『提交 / 继续』".to_string());
        c
    } else {
        let mut c = PageGuardConfig::partial(allow.to_vec(), vec![]);
        c.note = Some("人工输入区已开放(部分锁定),完成后回弹窗点『提交 / 继续』".to_string());
        c
    };
    apply_page_guard(page, &cfg).await.ok()?;
    Some(super::unlock_zone::spawn_unlock_reassert(page.clone(), cfg))
}

/// control_action=request_human:人工介入请求(HITL 闭环)。
///
/// 流程:可选 `bring_to_front`(默认 true)把浏览器窗口带到前台 → 第 132 轮:
/// 解析附图(显式 `params.image_path` 优先,`reason=captcha` 自动 CDP 视口截图,
/// 弹窗内直接展示验证码图片)→ 经 HumanAssistHub 发起结构化提问
/// (macOS/Windows 桌面**弹窗优先**(第 130 轮,置顶+倒计时+时间轴+可选中复制,
/// `-p` 模式同样可弹),TUI 兜底行读)→ oneshot 回填。
/// 返回专用信封:
/// - code=0:`data.human_response` 为人工输入(选项文本或自由文本),
///   `data.assist_channel` 标注应答通道(gui/tui);
/// - code=4001:超时 / 弹窗与 TUI 均不可用;
/// - code=4002:人工明确取消(弹窗取消/Esc/关窗,或 TUI 输入 q/取消)。
async fn act_request_human(id: &str, p: &Value) -> crate::error::Result<String> {
    // page_id 有效性前置校验(2000 语义与其它动作一致)
    if BrowserManager::global().page(id).await.is_none() {
        return envelope(2000, "page_id 不存在", json!({"page_id": id}));
    }
    let reason = str_arg(p, "reason").unwrap_or("custom");
    if !HUMAN_ASSIST_ALLOWED_REASONS.contains(&reason) {
        return envelope(
            1001,
            &format!("非法 reason(允许 {})", human_assist_reasons_doc()),
            json!({"reason": reason, "allowed": HUMAN_ASSIST_ALLOWED_REASONS}),
        );
    }
    let (label, default_message) = human_assist_defaults(reason);
    let message = str_arg(p, "message").unwrap_or(default_message).to_string();
    let mut options: Vec<String> = p
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if options.is_empty() {
        options = vec![
            "我已完成人工操作,继续".to_string(),
            "取消任务".to_string(),
        ];
    }
    if options.len() > 6 {
        options.truncate(6);
    }
    let timeout_ms = p.get("timeout_ms").and_then(Value::as_u64).unwrap_or_else(|| {
        // 第 118 轮:按 reason 分档默认超时(LLM 显式传 timeout_ms 仍走传入值)
        // - captcha / sms / two_factor:120_000(2 分钟)
        //   短文本回 TUI,人工可直接输入,无需更长等待;对齐 claudecode 实践
        // - qr_login / real_name / oauth / login / manual_verify / custom:300_000(5 分钟)
        //   扫码/刷脸/账密登录等需要人工在浏览器窗口操作的场景,留足余量
        match reason {
            "captcha" | "sms" | "two_factor" => 120_000,
            _ => 300_000,
        }
    });
    let bring = p.get("bring_to_front").and_then(Value::as_bool).unwrap_or(true);

    // 第 144 轮:HITL 凭证区域部分放行 —— unlock_page=true(默认)时优先
    // **partial + allow_selectors 白名单挖洞**(显式传参 > 自动探测账号/密码/验证码
    // 等人工必填区域,见 unlock_zone),探测不到才回退整页 open(第 143 轮行为)。
    // 提问期间人工可能提交表单触发导航(登录→2FA),新文档按烘焙态回锁会打断
    // 多步人工流程 —— spawn 导航重放协程持续维持临时放行态,收口时回收。
    // 纯问答场景(确认继续?)可传 unlock_page=false。
    let unlock_page = p.get("unlock_page").and_then(Value::as_bool).unwrap_or(true);
    let explicit_allow = {
        let raw: Vec<String> = p
            .get("allow_selectors")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        super::unlock_zone::sanitize_selector_list(&raw)
    };
    let mut unlock_source: &str = "none";
    let mut unlock_allow: Vec<String> = Vec::new();
    let mut unlock_reassert: Option<tokio::task::JoinHandle<()>> = None;
    let unlock_applied = unlock_page
        && crate::agent::browser::BrowserManager::global().guard_visuals_active().await
        && crate::agent::browser::BrowserManager::global().current_guard().await.mode
            != crate::agent::browser_overlay::PageGuardMode::Open
        && match crate::agent::browser::BrowserManager::global().page(id).await {
            Some(page) => {
                let zones = if !explicit_allow.is_empty() {
                    unlock_source = "explicit";
                    explicit_allow.clone()
                } else {
                    let z = super::unlock_zone::probe_credential_zones(&page).await;
                    if !z.is_empty() {
                        unlock_source = "auto";
                    }
                    z
                };
                if zones.is_empty() {
                    unlock_source = "fallback_open";
                }
                unlock_allow = zones.clone();
                match apply_hitl_unlock(&page, &zones).await {
                    Some(handle) => {
                        unlock_reassert = Some(handle);
                        true
                    }
                    None => {
                        unlock_source = "none";
                        unlock_allow.clear();
                        false
                    }
                }
            }
            None => false,
        };

    // 先把窗口带到前台(headed 才有视觉效果;hidden 模式静默失败不阻断提问)
    if bring {
        let _ = BrowserManager::global().bring_page_to_front(id).await;
    }
    let url = match BrowserManager::global().page(id).await {
        Some(pg) => pg.url().await.ok().flatten().unwrap_or_default(),
        None => String::new(),
    };

    // 第 133 轮:验证码现场截图 —— 显式 params.image_path 优先;reason=captcha
    // 自动采集(验证码元素裁剪优先,整视口兜底,见 captcha_crop);弹窗直接展示
    // 图片,人工不必切去浏览器找图。fail-open:截图失败按无图继续,绝不阻断提问。
    let (image_path, image_source, image_mode, image_clip) = {
        let (plan, source) = hitl_image_plan(reason, str_arg(p, "image_path"));
        match plan {
            HitlImagePlan::Explicit(path) => (path, source, "explicit", Value::Null),
            HitlImagePlan::Auto => match super::captcha_crop::capture_hitl_screenshot(id).await {
                Ok((path, mode, clip)) => (
                    path,
                    mode,
                    "auto",
                    clip.map(|c| c.to_json()).unwrap_or(Value::Null),
                ),
                Err(_) => (String::new(), "auto_failed", "none", Value::Null),
            },
            HitlImagePlan::None => (String::new(), source, "none", Value::Null),
        }
    };

    let outcome = crate::agent::human_assist::HumanAssistHub::global()
        .request(reason, &message, options.clone(), &url, id, timeout_ms, &image_path)
        .await;
    // 第 143/144 轮:人工介入收口(应答/超时/取消/不可用)后按实例期望态恢复管控
    // —— 无论何种结局,「人工可操作窗口」都必须关闭:先回收导航重放协程(防恢复后
    // 又被重放覆盖),再恢复原档;fail-open(恢复失败不阻断信封返回,下一个输入动作
    // 的「先解/先隐」与导航 re-assert 会自愈)。
    if unlock_applied {
        if let Some(handle) = unlock_reassert.take() {
            handle.abort();
        }
        if let Some(page) = crate::agent::browser::BrowserManager::global().page(id).await {
            let cfg = crate::agent::browser::BrowserManager::global().current_guard().await;
            let _ = crate::agent::browser_overlay::apply_page_guard(&page, &cfg).await;
        }
    }
    use crate::agent::human_assist::{AssistVia, HumanAssistOutcome};
    match outcome {
        HumanAssistOutcome::Answered { text: answer, via } => {
            envelope(
                0,
                "人工已响应",
                json!({
                    "reason": reason,
                    "kind_label": label,
                    "human_response": answer,
                    "assist_channel": via.as_str(),
                    // 第 144 轮:提问期间页面放行方式对账(partial=凭证区挖洞 /
                    // open=整页回退 / none=未放行;source: explicit|auto|fallback_open|none)。
                    "page_unlock": json!({
                        "applied": unlock_applied,
                        "mode": if !unlock_applied { "none" }
                            else if unlock_allow.is_empty() { "open" }
                            else { "partial" },
                        "allow_selectors": unlock_allow,
                        "source": unlock_source,
                    }),
                    // 第 132/133 轮:附图对账(人工读的是哪张图 / 元素裁剪 or 整视口)
                    "image_path": image_path,
                    "image_source": image_source,
                    "image_mode": image_mode,
                    "image_clip": image_clip,
                    // 第 145 轮:按「输码 vs 已完成」分流 —— 自由文本(验证码)+ captcha/
                    // sms/two_factor → 立即 input_text 填码提交;选项应答 → inspect 验证。
                    "next_hint": super::hitl_hint::hitl_answer_hint(reason, &answer),
                }),
            )
        }
        HumanAssistOutcome::Timeout { via } => envelope(
            4001,
            "人工介入等待超时",
            json!({
                "reason": reason,
                "kind_label": label,
                "assist_channel": via.as_str(),
                "hint": "等待人工响应超时。可再次 request_human 重试,或如实向用户报告需要人工配合后结束",
            }),
        ),
        HumanAssistOutcome::Cancelled { via } => envelope(
            4002,
            "人工已取消介入",
            json!({
                "reason": reason,
                "kind_label": label,
                "assist_channel": via.as_str(),
                "hint": "人工选择取消。终止当前浏览器流程,汇总已完成部分向用户报告,不要继续重试",
            }),
        ),
        HumanAssistOutcome::Unavailable => envelope(
            4001,
            "人工介入不可用(弹窗与 TUI 均不可用)",
            json!({
                "reason": reason,
                "kind_label": label,
                "assist_channel": AssistVia::Tui.as_str(),
                // 第 131 轮:原信封只说「不可用」,用户无从判断是没弹、弹失败、
                // 还是压根没走到这一步。附本机能力诊断以便排障。
                "human_ui_diagnostics": crate::agent::human_ui::diagnostics(),
                "hint": "当前环境既无桌面弹窗(macOS/Windows 会话)也无 TUI 交互。如实告知用户:请在 laew TUI 交互模式或桌面会话下重新执行该任务",
            }),
        ),
    }
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

// =================== 第 118 轮:explore / batch action ===================

/// `action=explore`:批量观察。一次调用合并多个 inspect 维度(elements/dom/
/// screenshot/blockers 等),返回合并结果,节省 LLM round-trip。
///
/// 入参形态:
/// ```json
/// {"action":"explore","page_id":"p_xxx",
///  "queries":[
///    {"info":"elements"},
///    {"info":"dom","params":{"selector":"form"}},
///    {"info":"screenshot","params":{"out":"login.png"}},
///    {"info":"blockers"}
///  ],
///  "summary_hint":"登录页 DOM 结构"}
/// ```
///
/// 返回:`{code:0, data:{page_id, results:[{info,code,message,data},...], summary_hint, total}}`。
pub(super) async fn run_explore(args: Value) -> crate::error::Result<String> {
    let Some(id) = str_arg(&args, "page_id") else {
        return envelope(1001, "缺少 page_id", json!({}));
    };
    let queries = match args.get("queries").and_then(Value::as_array) {
        Some(q) if !q.is_empty() => q,
        Some(_) => return envelope(1001, "queries 不能为空", json!({})),
        None => return envelope(1001, "缺少 queries", json!({})),
    };
    if queries.len() > 8 {
        return envelope(
            1001,
            "queries 最多 8 项(超出请分批调用,避免单次返回过大)",
            json!({"got": queries.len(), "max": 8}),
        );
    }
    // 校验 page_id 有效性(2000 语义),所有 query 共用一次失败检查
    if BrowserManager::global().page(id).await.is_none() {
        return envelope(2000, "page_id 不存在", json!({"page_id": id}));
    }

    let summary_hint = args
        .get("summary_hint")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let mut results: Vec<Value> = Vec::with_capacity(queries.len());
    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    for q in queries {
        // 构造单步 inspect 入参(action=inspect + info + params)
        let mut single = serde_json::Map::new();
        single.insert("action".into(), Value::String("inspect".into()));
        single.insert("page_id".into(), Value::String(id.to_string()));
        // 第 119 轮:记录 LLM 请求的 info(用于 results[].info 对齐)。
        // 早前实现从 envelope data.info 反查,而 inspect 成功响应并不带该字段,
        // 导致 explore 结果的 info 恒为 null、blockers 特判永不命中。
        let requested_info = match q.get("info").cloned() {
            Some(info) if info.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false) => info,
            _ => {
            // 缺 info → 1001,跳过本次 query(不影响后续)
            results.push(json!({
                "info": Value::Null,
                "code": 1001,
                "message": "query 缺少 info 字段",
                "data": {},
            }));
            err_count += 1;
            continue;
            }
        };
        single.insert("info".into(), requested_info.clone());
        if let Some(params) = q.get("params").cloned() {
            single.insert("params".into(), params);
        }

        // 调 inspect::run(已有)
        match crate::agent::tools::mcp_web_use::inspect::run(Value::Object(single)).await {
            Ok(envelope_str) => {
                // envelope_str 是 JSON 字符串,parse 后嵌入 results
                match serde_json::from_str::<Value>(&envelope_str) {
                    Ok(envelope_value) => {
                        // 第 119 轮:用 LLM 请求的 info 作为结果标识(不再反查 data.info)
                        let info = requested_info.clone();
                        let code = envelope_value
                            .get("code")
                            .cloned()
                            .unwrap_or(json!(2002));
                        let message = envelope_value
                            .get("message")
                            .cloned()
                            .unwrap_or(Value::Null);
                        let data = envelope_value
                            .get("data")
                            .cloned()
                            .unwrap_or(json!({}));
                        // blockers 命中提示附带 next_action hint
                        let data = if info.as_str() == Some("blockers") {
                            let blocked = data
                                .get("blockers")
                                .and_then(Value::as_array)
                                .map(|a| !a.is_empty())
                                .unwrap_or(false);
                            if blocked {
                                let mut d = data.as_object().cloned().unwrap_or_default();
                                d.insert(
                                    "next_action".into(),
                                    json!("立即 control(request_human, reason=<kind>) 让人工介入,不要再 inspect 浪费时间"),
                                );
                                Value::Object(d)
                            } else {
                                data
                            }
                        } else {
                            data
                        };
                        let code_num = code.as_i64().unwrap_or(2002);
                        if code_num == 0 {
                            ok_count += 1;
                        } else {
                            err_count += 1;
                        }
                        results.push(json!({
                            "info": info,
                            "code": code,
                            "message": message,
                            "data": data,
                        }));
                    }
                    Err(_) => {
                        // envelope 解析失败,降级保留原始字符串
                        err_count += 1;
                        results.push(json!({
                            "info": Value::Null,
                            "code": 2002,
                            "message": "inspect 返回 envelope 解析失败",
                            "data": {"raw": envelope_str},
                        }));
                    }
                }
            }
            Err(e) => {
                err_count += 1;
                results.push(json!({
                    "info": q.get("info").cloned().unwrap_or(Value::Null),
                    "code": 2001,
                    "message": format!("inspect 调用失败:{e}"),
                    "data": {},
                }));
            }
        }
    }

    envelope(
        0,
        "ok",
        json!({
            "page_id": id,
            "results": results,
            "summary_hint": summary_hint,
            "ok_count": ok_count,
            "err_count": err_count,
            "total": queries.len(),
        }),
    )
}

/// `action=batch`:批量混合执行。复用 sequence 执行器逻辑(内部调用 `run_sequence`),
/// 保留 batch 作为「允许任意 inspect + control 混合」的语义别名,提示 LLM
/// 「探索后批量执行」的标准操作。
///
/// 与 sequence 区别:batch 在 Schema 描述中明示「用于 control + inspect 混合的
/// 稳定流程」(登录/表单类场景),sequence 描述为「通用连续执行」。
pub(super) async fn run_batch(args: Value) -> crate::error::Result<String> {
    // batch = sequence 的语义别名;直接转发到 run_sequence
    super::run_sequence(args).await
}
