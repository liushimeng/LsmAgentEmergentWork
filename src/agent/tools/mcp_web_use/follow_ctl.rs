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

/// 文本定位查找脚本(IIFE,主 frame 与嵌套 frame 共用同一份):返回命中节点摘要或 null。
/// 作为 `CallFunctionOnParams` 的 `function_declaration` 也成立(表达式形态)。
fn scroll_to_text_find_js(text: &str, block: &str) -> String {
    format!(
        r#"(() => {{
            const t = {txt};
            if (!t) return null;
            const all = document.querySelectorAll('body *');
            let best = null, bestLen = Infinity;
            for (const el of all) {{
                const s = el.innerText || el.textContent || '';
                if (!s || s.indexOf(t) < 0) continue;
                const r = el.getBoundingClientRect();
                if (r.width <= 0 || r.height <= 0) continue;
                const cs = getComputedStyle(el);
                if (cs.visibility === 'hidden' || cs.display === 'none') continue;
                if (parseFloat(cs.opacity || '1') < 0.05) continue;
                if (s.length < bestLen) {{ best = el; bestLen = s.length; }}
            }}
            if (!best) return null;
            best.scrollIntoView({{block: {blk}, behavior:'instant'}});
            return {{ tag: best.tagName, text_len: bestLen,
                     head: (best.innerText || best.textContent || '').slice(0, 120) }};
        }})()"#,
        txt = js_str(text),
        blk = js_str(block)
    )
}

fn scroll_to_text_result(text: &str, block: &str, frame: &str, hit: &Value) -> Value {
    let obj = hit.as_object().cloned().unwrap_or_default();
    json!({
        "text": text,
        "block": block,
        "frame": frame,
        "found": true,
        "tag": obj.get("tag").cloned().unwrap_or(Value::Null),
        "text_len": obj.get("text_len").cloned().unwrap_or(Value::Null),
        "head": obj.get("head").cloned().unwrap_or(Value::Null),
    })
}

/// 文本定位滚动:按**可见文本**把页面滚到语义区块(如「累计评价」「商品参数」
/// 「规格」),免去「先 inspect 拿选择器再 scroll_to」的往返。
///
/// 选点策略:遍历 body 后代,取「含目标文本 + 有几何尺寸 + 可见」中**文本最短**的
/// 元素(最贴近文本所在的具体节点,避免命中整页/整卡片大容器导致只滚到页首)。
///
/// 嵌套 frame 通道:主 frame 未命中时,逐个 frame 的 execution context 重放同一查找
/// 脚本 —— 天猫/淘宝的评论区、详情参数常渲染在**嵌套 iframe** 内,主 frame 的
/// querySelectorAll 看不到。跨域 iframe 无访问权(报错即跳过,fail-open);同源嵌套
/// frame 可正确命中并滚动。响应 frame 字段回报命中位置(main / frame:<n>)。
pub(super) async fn act_scroll_to_text(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(text) = str_arg(p, "text") else { return Err("缺少 text".into()) };
    let block = str_arg(p, "block").unwrap_or("center");
    let page = ensure_page(id).await?;
    let find_js = scroll_to_text_find_js(text, block);

    // 1) 主 frame
    let hit = eval_js_string(&page, &find_js).await?;
    if !hit.is_null() {
        return Ok(scroll_to_text_result(text, block, "main", &hit));
    }

    // 2) 嵌套 frame:逐个 execution context 重放;跨域 iframe 报错即跳过
    let frames = page.frames().await.unwrap_or_default();
    let mut probed = 0usize;
    let mut probed_same_origin = 0usize;
    for (i, fid) in frames.iter().enumerate() {
        let Some(ctx) = page
            .frame_execution_context(fid.clone())
            .await
            .unwrap_or(None)
        else {
            continue;
        };
        probed += 1;
        let call = chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams::builder()
            .function_declaration(find_js.clone())
            .execution_context_id(ctx)
            .build()
            .map_err(|e| e.to_string())?;
        match page.evaluate_function(call).await {
            Ok(res) => {
                let v = res.value().cloned();
                if let Some(v) = v {
                    if !v.is_null() {
                        probed_same_origin += 1;
                        return Ok(scroll_to_text_result(
                            text,
                            block,
                            &format!("frame:{i}"),
                            &v,
                        ));
                    }
                }
            }
            // 跨域 iframe:拿不到执行权,跳过(下一个)
            Err(_) => continue,
        }
    }

    Err(format!(
        "页面可见文本「{text}」未找到(已探测主 frame + {probed} 个嵌套 frame 的执行上下文,\
         其中 {probed_same_origin} 个同源可达但未命中);可能尚未渲染(先 wait/scroll 触发懒加载)、\
         在跨域 iframe 内(拿不到执行权),或文案不同 —— 建议 inspect(info=elements) 复核真实文案",
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 查找脚本转义与结构() {
        // js_str 走 serde_json(双引号 JSON 字面量):反斜杠必须转义,单引号在双引号
        // 串内原样保留;双引号被转义成 \" —— 三者都不能破坏脚本
        let js = scroll_to_text_find_js("it's a\\b", "center");
        assert!(js.contains("it's"), "双引号串内单引号原样保留");
        assert!(js.contains("a\\\\b"), "反斜杠已转义");
        assert!(js.contains("scrollIntoView"), "滚动调用存在");
        assert!(js.contains("text_len"), "返回摘要存在");

        // block 参数按字面量注入(start/end 可改对齐方式)
        let js_end = scroll_to_text_find_js("规格", "end");
        assert!(js_end.contains("end"));
    }

    #[test]
    fn 结果装配字段() {
        let hit = json!({"tag": "DIV", "text_len": 42, "head": "累计评价 1234 条"});
        let out = scroll_to_text_result("累计评价", "center", "main", &hit);
        assert_eq!(out["found"], json!(true));
        assert_eq!(out["frame"], json!("main"));
        assert_eq!(out["tag"], json!("DIV"));
    }
}
