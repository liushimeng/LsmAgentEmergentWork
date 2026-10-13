//! 表单选择器一族:原生 `select` 选择(select_option)+ 自定义下拉/级联选择器
//! (select_dropdown,第 158 轮)+ 共享的文本命中辅助 `pick_element_js`。
//! (自 control.rs 拆出 —— control 超 1800 行规范;机械搬移零改写。)

use super::*;

pub(super) async fn act_select_option(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(sel) = str_arg(p, "selector") else { return Err("缺少 selector".into()); };
    let page = ensure_page(id).await?;

    // 第 158 轮:自定义控件回退 —— selector 指向的不是原生 <select> 时,自动走文本匹配路径。
    // 检测方式:元素不存在 el.options 即为自定义控件(Ant Design / Element UI / 自研组件库)。
    let detect_js = format!(
        r#"(() => {{
            const el = document.querySelector({sel});
            if (!el) return 'not_found';
            if (el.tagName === 'SELECT' && el.options) return 'native';
            return 'custom';
        }})()"#,
        sel = js_str(sel),
    );
    let detect: Value = eval_js_string(&page, &detect_js).await?;
    let detect_str = detect.as_str().unwrap_or("not_found");
    if detect_str == "not_found" {
        return Err(format!("select_option: selector「{sel}」未命中任何元素"));
    }
    if detect_str == "custom" {
        // 自定义控件回退:用 text 参数走文本匹配;无 text 则报错引导使用 select_dropdown
        let text = str_arg(p, "text");
        let Some(t) = text else {
            return Err(format!(
                "select_option: selector「{sel}」指向非原生 select 元素(自定义控件),\
                 请改用 select_dropdown(action='select_dropdown', params={{'option_text':'...'}})"
            ));
        };
        let js = pick_element_js(t, Some(sel), true);
        let result: Value = eval_js_string(&page, &js).await?;
        let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if !ok {
            return Err(format!(
                "select_option: 在「{sel}」范围内未找到文本匹配「{t}」的可见叶子节点"
            ));
        }
        return Ok(json!({"selected": ok, "selector": sel, "mode": "custom_fallback", "text": t}));
    }

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

// =================== 第 158 轮:自定义下拉/级联选择器(select_dropdown) ===================

/// 在页面中查找文本匹配的可见元素并点击，返回结果 JSON 字符串。
///
/// 查找范围优先 `scope_sel`，否则全局；精确匹配优先，`fuzzy=true` 时兜底子串匹配。
/// 匹配语义（实测闲鱼城市选择器面板踩坑后的通用化修复）：
/// - 文本取自 `textContent`（确定性），可见性单独用「几何尺寸 + 计算样式」判定——
///   组件库收起的面板常保留 `visibility:hidden` 的挂载副本：其 `getBoundingClientRect`
///   仍为正、`innerText` 为空，旧实现「尺寸判可见 + innerText 取文本」两侧互相矛盾，
///   既可能误判隐藏副本为可见，也可能让真实可见项因 innerText 为空而漏配；
/// - 触发器这类「文本 + 图标」非叶子控件（如 `<span>区域<img></span>`）按
///   「自身直接文本（不含元素子节点）」参与匹配，不再要求必须是叶子；
/// - 多候选时取面积最小者（最深/最贴近文本节点），避免命中外层容器；
/// - 失败时返回诊断（文本命中数/可见数/hint），供调用方一次性给出可行动的错误信息。
/// 点击前 `scrollIntoView({block:'center'})` 确保元素可见。
pub(in crate::agent::tools::mcp_web_use) fn pick_element_js(target: &str, scope_sel: Option<&str>, fuzzy: bool) -> String {
    let scope = match scope_sel {
        Some(s) => format!("document.querySelector({})", js_str(s)),
        None => "document.body".to_string(),
    };
    format!(
        r#"(() => {{
            const target = {target};
            const scope = {scope} || document.body;
            const norm = s => (s || '').replace(/\s+/g, ' ').trim();
            // 真实可见性:几何尺寸 + 计算样式(visibility 会继承,display 不继承但防御)
            const vis = e => {{
                const r = e.getBoundingClientRect();
                if (r.width <= 0 || r.height <= 0) return false;
                const cs = getComputedStyle(e);
                return cs.visibility !== 'hidden' && cs.display !== 'none' && cs.opacity !== '0';
            }};
            // 自身直接文本(仅元素子节点以外的文本节点):覆盖「文本+图标」非叶子控件
            const ownText = e => {{
                let t = '';
                for (const n of e.childNodes) {{ if (n.nodeType === 3) t += n.nodeValue; }}
                return norm(t);
            }};
            const textOf = e => norm(e.textContent);
            const all = [...scope.querySelectorAll('span,div,a,li,label,button,i,p')];
            // 精确匹配:叶子全文本 或 自身直接文本
            let pool = all.filter(e =>
                (e.children.length === 0 && textOf(e) === target) || ownText(e) === target);
            let mode = 'exact';
            if (!pool.length && {fuzzy}) {{
                // 模糊兜底:叶子全文本子串,或「小容器(≤3 个元素子节点)」自身直接文本子串,
                // 防止把整页大容器当候选
                pool = all.filter(e =>
                    (e.children.length === 0 && textOf(e).includes(target)) ||
                    (e.childElementCount <= 3 && ownText(e).includes(target)));
                mode = 'fuzzy';
            }}
            const visible = pool.filter(vis);
            if (!visible.length) {{
                return JSON.stringify({{
                    ok: false, matched: pool.length, visible: 0,
                    hint: pool.length ? 'matched_but_hidden' : 'no_text_match',
                }});
            }}
            // 多候选取面积最小者(最深/最贴近文本节点)
            const el = visible.sort((a, b) => {{
                const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
                return (ra.width * ra.height) - (rb.width * rb.height);
            }})[0];
            el.scrollIntoView({{ block: 'center' }});
            el.click();
            return JSON.stringify({{ ok: true, tag: el.tagName, text: ownText(el) || textOf(el), mode }});
        }})()"#,
        target = js_str(target),
        scope = scope,
        fuzzy = if fuzzy { "true" } else { "false" },
    )
}

/// 只探测不点击的 find-only 变体(第 164 轮 select_dropdown 选项优先通道):
/// 与 `pick_element_js` 同一套匹配/可见性语义,但不做 scrollIntoView/click,
/// 返回 `{found, count, hint, tag, cls, text}` 供调用方先判断再决定是否点击。
/// 实测场景(闲鱼区域面板):面板已被前序动作点开时,直接选选项即可;
/// 若误点触发器,组件库会把面板 toggle 关回去,后续全部探测落空。
pub(in crate::agent::tools::mcp_web_use) fn probe_element_js(target: &str, scope_sel: Option<&str>, fuzzy: bool) -> String {
    let scope = match scope_sel {
        Some(s) => format!("document.querySelector({})", js_str(s)),
        None => "document.body".to_string(),
    };
    format!(
        r#"(() => {{
            const target = {target};
            const scope = {scope} || document.body;
            const norm = s => (s || '').replace(/\s+/g, ' ').trim();
            const vis = e => {{
                const r = e.getBoundingClientRect();
                if (r.width <= 0 || r.height <= 0) return false;
                const cs = getComputedStyle(e);
                return cs.visibility !== 'hidden' && cs.display !== 'none' && cs.opacity !== '0';
            }};
            const ownText = e => {{
                let t = '';
                for (const n of e.childNodes) {{ if (n.nodeType === 3) t += n.nodeValue; }}
                return norm(t);
            }};
            const textOf = e => norm(e.textContent);
            const all = [...scope.querySelectorAll('span,div,a,li,label,button,i,p')];
            let pool = all.filter(e =>
                (e.children.length === 0 && textOf(e) === target) || ownText(e) === target);
            let mode = 'exact';
            if (!pool.length && {fuzzy}) {{
                pool = all.filter(e =>
                    (e.children.length === 0 && textOf(e).includes(target)) ||
                    (e.childElementCount <= 3 && ownText(e).includes(target)));
                mode = 'fuzzy';
            }}
            const visible = pool.filter(vis);
            if (!visible.length) {{
                return JSON.stringify({{
                    found: false, count: pool.length, visible: 0, mode,
                    hint: pool.length ? 'matched_but_hidden' : 'no_text_match',
                }});
            }}
            const el = visible.sort((a, b) => {{
                const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
                return (ra.width * ra.height) - (rb.width * rb.height);
            }})[0];
            return JSON.stringify({{
                found: true, count: visible.length, visible: visible.length, mode,
                tag: el.tagName, cls: String(el.className || '').slice(0, 120),
                text: ownText(el) || textOf(el),
            }});
        }})()"#,
        target = js_str(target),
        scope = scope,
        fuzzy = if fuzzy { "true" } else { "false" },
    )
}

/// 选后「确认按钮」通道(第 164 轮):自定义面板选中选项后常需再点
/// 「确定/完成」才真正生效(实测闲鱼区域面板选中态已置但筛选未生效,
/// 模型反复盲点 4.7 分钟)。在面板作用域内探测确认类按钮并点击:
/// - 显式 `scope_sel` 时信任该范围,只要求可见;
/// - 无 scope 时要求候选位于「面板状」容器内(fixed/absolute 定位祖先或
///   role=dialog/listbox/menu/tooltip/presentation),防止误点页面其它表单
///   的提交按钮。
pub(in crate::agent::tools::mcp_web_use) fn confirm_panel_js(scope_sel: Option<&str>) -> String {
    let scope = match scope_sel {
        Some(s) => format!("document.querySelector({})", js_str(s)),
        None => "document.body".to_string(),
    };
    format!(
        r#"(() => {{
            const CONFIRM = ['确定', '确认', '完成', '应用', 'OK', 'Ok', 'ok', '保存'];
            const scoped = {scoped};
            const scope = {scope} || document.body;
            const norm = s => (s || '').replace(/\s+/g, ' ').trim();
            const vis = e => {{
                const r = e.getBoundingClientRect();
                if (r.width <= 0 || r.height <= 0) return false;
                const cs = getComputedStyle(e);
                return cs.visibility !== 'hidden' && cs.display !== 'none' && cs.opacity !== '0';
            }};
            const ownText = e => {{
                let t = '';
                for (const n of e.childNodes) {{ if (n.nodeType === 3) t += n.nodeValue; }}
                return norm(t);
            }};
            const textOf = e => norm(e.textContent);
            const all = [...scope.querySelectorAll('button,a,span,div,li,label,i,p')];
            const hit = e => CONFIRM.includes(textOf(e)) || CONFIRM.includes(ownText(e));
            let cands = all.filter(e => (e.children.length === 0 && hit(e)) || hit(e));
            cands = cands.filter(vis);
            if (!scoped) {{
                // 无显式 scope:必须位于面板状容器内,防误点页面级提交按钮
                cands = cands.filter(e => {{
                    let n = e.parentElement, steps = 0;
                    while (n && n !== document.body && steps < 8) {{
                        const cs = getComputedStyle(n);
                        if (cs.position === 'fixed' || cs.position === 'absolute') return true;
                        const role = n.getAttribute('role') || '';
                        if (['dialog', 'listbox', 'menu', 'tooltip', 'presentation'].includes(role)) return true;
                        n = n.parentElement; steps++;
                    }}
                    return false;
                }});
            }}
            if (!cands.length) return JSON.stringify({{ clicked: false, reason: 'no_confirm_button' }});
            const el = cands.sort((a, b) => {{
                const ra = a.getBoundingClientRect(), rb = b.getBoundingClientRect();
                return (ra.width * ra.height) - (rb.width * rb.height);
            }})[0];
            el.click();
            return JSON.stringify({{ clicked: true, text: textOf(el) || ownText(el) }});
        }})()"#,
        scoped = if scope_sel.is_some() { "true" } else { "false" },
        scope = scope,
    )
}

/// 自定义下拉/级联选择器操作。
///
/// 解决 Ant Design / Element UI / 自研组件库的非原生 `<select>` 控件无法用
/// `select_option` 操作的痛点（实测闲鱼城市选择器卡 263s / 24 轮迭代）。
///
/// 参数：
/// - `option_text`（必填）：第一级选项文本（如"北京"）
/// - `trigger_text`（可选）：触发器文本（如"区域"），点击展开面板
/// - `cascader_text`（可选）：级联第二级选项文本（如"全北京"）
/// - `wait_ms`（可选）：等待面板出现的时间，默认 800ms
/// - `selector`（可选）：面板容器选择器，用于限定查找范围
///
/// 自愈策略（第 164 轮更新）：
/// 1. **选项优先**：先 probe（只找不点）选项，可见则直接点击 —— 面板已被前序
///    动作点开时零浪费，且避免「再点触发器把面板 toggle 关回去」的反向事故；
/// 2. 选项未命中 → 点击触发器展开面板 → 选项搜索最多 3 次（立即搜 → 等 wait_ms
///    搜 → 最终一次），容忍展开动画与异步渲染；
/// 3. 选后自动探测并点击面板内「确定/完成」类确认按钮，回读触发器文本
///    （`trigger_after`）与面板文本（`value_after`）如实上报生效事实；
/// 4. 仍失败时错误信息携带诊断（文本命中数/可见数），让模型一步定位而非盲试。
pub(super) async fn act_select_dropdown(id: &str, p: &Value) -> std::result::Result<Value, String> {
    let Some(option_text) = str_arg(p, "option_text") else {
        return Err("缺少 option_text".into());
    };
    let page = ensure_page(id).await?;
    let trigger_text = str_arg(p, "trigger_text");
    let cascader_text = str_arg(p, "cascader_text");
    let scope_sel = str_arg(p, "selector");
    let wait_ms = p
        .get("wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(800);

    // 第 164 轮:选项优先 —— 面板可能已被前序动作点开,此时直接选即可;
    // 若先去点触发器,组件库会把已开的面板 toggle 关回去,后续探测全部落空
    // (实测闲鱼区域面板:模型先点开面板,select_dropdown 再点触发器 → 面板关闭)。
    let mut trigger_clicked = false;
    let mut clicked_texts: Vec<String> = Vec::new();
    let mut option_ok = false;
    let mut last_diag = String::from("eval 未执行");
    {
        let probe_js = probe_element_js(option_text, scope_sel, true);
        let probe: Value = eval_js_string(&page, &probe_js).await?;
        if probe.get("found").and_then(Value::as_bool).unwrap_or(false) {
            let js = pick_element_js(option_text, scope_sel, true);
            let result: Value = eval_js_string(&page, &js).await?;
            option_ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
            if option_ok {
                clicked_texts.push(option_text.to_string());
            }
        } else {
            last_diag = format!(
                "选项预探测未命中(文本命中 {} 个/可见 0 个,{})",
                probe.get("count").and_then(Value::as_u64).unwrap_or(0),
                probe.get("hint").and_then(Value::as_str).unwrap_or("?")
            );
        }
    }

    // 选项未命中 → 触发器兜底点开面板(此时面板确实未开,点击是安全的)。
    if !option_ok {
        if let Some(tt) = trigger_text {
            let js = pick_element_js(tt, None, false);
            let result: Value = eval_js_string(&page, &js).await?;
            trigger_clicked = result
                .get("ok")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if trigger_clicked {
                clicked_texts.push(tt.to_string());
                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            } else {
                last_diag = format!("触发器「{tt}」未找到可见元素;{last_diag}");
            }
        }

        // 选项搜索:最多 3 次探测,前两次失败补等待后再搜(容忍面板展开动画/异步渲染)。
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            }
            let js = pick_element_js(option_text, scope_sel, true);
            let result: Value = eval_js_string(&page, &js).await?;
            let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
            if ok {
                clicked_texts.push(option_text.to_string());
                option_ok = true;
                last_diag.clear();
                break;
            }
            let matched = result.get("matched").and_then(Value::as_u64).unwrap_or(0);
            last_diag = format!(
                "文本命中 {matched} 个/可见 0 个({})",
                result.get("hint").and_then(Value::as_str).unwrap_or("?")
            );
            if attempt == 2 {
                return Err(format!(
                    "select_dropdown: 选项「{option_text}」未找到可见元素（scope={scope_sel:?}，{last_diag}）；\
                     处置:面板可能已收合(组件库常保留 visibility:hidden 的挂载副本)或选项文本不一致——\
                     先 inspect(info=elements, selector=<面板容器>) 确认选项真实文本与面板状态,或改用 eval_js 直接点击"
                ));
            }
        }
    }
    let _ = option_ok;

    if let Some(ct) = cascader_text {
        tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
        let js = pick_element_js(ct, scope_sel, true);
        let result: Value = eval_js_string(&page, &js).await?;
        let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if !ok {
            return Err(format!(
                "select_dropdown: 级联选项「{ct}」未找到可见元素（scope={scope_sel:?}）；\
                 处置:第一级已选中,inspect(info=elements, selector=<级联面板容器>) 查看第二级实际文本后重试"
            ));
        }
        clicked_texts.push(ct.to_string());
        tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
    }

    let readback_js = {
        let scope = match scope_sel {
            Some(s) => format!("document.querySelector({})", js_str(s)),
            None => "document.body".to_string(),
        };
        format!(
            r#"(() => {{
                const scope = {scope} || document.body;
                const txt = (scope.innerText || '').replace(/\s+/g, ' ').trim().slice(0, 200);
                return JSON.stringify({{ text: txt }});
            }})()"#,
            scope = scope,
        )
    };
    let readback: Value = eval_js_string(&page, &readback_js).await?;
    let value_after = readback
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // 第 164 轮:选后「确定/完成」确认按钮通道 —— 选中态已置但筛选未生效的
    // 面板(实测闲鱼)靠这一步闭环;无确认按钮时静默跳过(响应带事实)。
    let confirm_js = confirm_panel_js(scope_sel);
    let confirm: Value = eval_js_string(&page, &confirm_js).await?;
    let confirm_clicked = confirm
        .get("clicked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let confirm_text = confirm
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // 触发器选后回读:仍能以原文本找到触发器 → 未回显选中值;
    // 找不到 → 触发器文本已变化(通常已回显选中值),作为 applied 事实提示。
    let trigger_after = if let Some(tt) = trigger_text {
        let probe_js = probe_element_js(tt, None, false);
        let probe: Value = eval_js_string(&page, &probe_js).await?;
        json!({
            "found": probe.get("found").and_then(Value::as_bool).unwrap_or(false),
            "text": probe.get("text").and_then(Value::as_str).unwrap_or(""),
        })
    } else {
        json!(null)
    };

    Ok(json!({
        "selected": true,
        "option_text": option_text,
        "cascader_text": cascader_text,
        "trigger_clicked": trigger_clicked,
        "clicked_texts": clicked_texts,
        "confirm_clicked": confirm_clicked,
        "confirm_text": confirm_text,
        "trigger_after": trigger_after,
        "value_after": value_after,
        "applied_hint": "trigger_after.found=false 表示触发器文本已变化(多已回显选中值);仍为 true 且筛选未生效时,检查是否需切换排序/二次确认,勿对同一选项重复盲点(toggle 会取消选中)",
        "mode": "custom_dropdown",
    }))
}