//! 键盘语义与控件级剪贴板(39 号设计文档:键盘路由 + P2a 光标/选区/编辑语义)。
//!
//! 从 `mod.rs` 拆出(1800 行规范)并按 P2a 改造:文本控件内的键入不再
//! 「尾部追加」,而是**光标处插入/删除 + 选区替换**,并补齐
//! 方向键移光标、Home/End、Ctrl 词跳、Ctrl+A/Z/Y、Delete 前删等浏览器语义。
//! 编辑运算的纯逻辑在 [`super::text_edit`],本文件只做状态读写与事件派发。

use super::*;

/// 编辑类组合键(P2a)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOp {
    /// Ctrl+A 全选(聚焦文本控件内)。
    SelectAll,
    /// Ctrl+Z 撤销。
    Undo,
    /// Ctrl+Y / Ctrl+Shift+Z 重做。
    Redo,
}

/// 由按键+修饰键判定编辑操作(与 [`clip_op_for`] 互补,均需 ctrl/meta)。
pub fn edit_op_for(key: &str, mods: &Mods) -> Option<EditOp> {
    if !(mods.ctrl || mods.meta) || mods.alt {
        return None;
    }
    match key.to_ascii_lowercase().as_str() {
        "a" if !mods.shift => Some(EditOp::SelectAll),
        "z" if mods.shift => Some(EditOp::Redo),
        "z" => Some(EditOp::Undo),
        "y" if !mods.shift => Some(EditOp::Redo),
        _ => None,
    }
}

/// 文本控件的键入/编辑上下文(锁内收敛)。
struct TextEditCtx {
    focused: Option<String>,
    ftag: String,
    ftyp: String,
    text_like: bool,
    editable: bool,
    focusables: Vec<String>,
    /// 当前值(form_values 优先)。
    value: String,
    /// 当前光标/选区(无记录时缺省「值尾折叠」= 尾部追加旧语义)。
    caret: CaretState,
    /// textarea 折行(渲染同源;无值时为空,单行控件不折行)。
    rows: Vec<(String, usize)>,
}

impl LsmBrowserManager {
    /// 控件级剪贴板(⌘/Ctrl+C/X/V 与查看器右键菜单共用)。
    ///
    /// P2a 起按选区语义:有选区复制/剪切选区、粘贴替换选区;无选区回落
    /// 整值复制/清空、光标处插入(无光标态 = 尾部追加,兼容旧行为)。
    pub async fn handle_human_clip(&self, pid: &str, op: ClipOp) -> Value {
        if !enabled() {
            return json!({"ignored": true, "reason": "LAEW_HUMAN_INPUT=off"});
        }
        if web_control::enabled() && !web_control::is_user_control(Engine::Lsm, pid) {
            return json!({"ignored": true, "reason": "agent_control"});
        }
        let ctx = self.collect_text_ctx(pid).await;
        let Some(focused) = ctx.focused.clone() else {
            // P2b:无控件焦点时,若存在**页面级自由选区** → 复制其文本
            //(浏览器语义:Ctrl+C 复制页面选区,无需先点进输入框)。
            if op == ClipOp::Copy {
                let text = {
                    let inner = self.inner.lock().await;
                    inner
                        .tabs
                        .get(pid)
                        .and_then(|t| t.page_sel.as_ref().map(|s| api::page_selection_text_pub(t, s)))
                        .unwrap_or_default()
                };
                if !text.is_empty() {
                    return match clipboard::write_text(&text) {
                        Ok(()) => {
                            self.note_human_event(pid, "人工复制(页面选区)");
                            json!({"op": "copy", "ok": true, "source": "page_selection", "len": text.chars().count()})
                        }
                        Err(e) => json!({"op": "copy", "ok": false, "error": e}),
                    };
                }
            }
            return json!({"op": op_name(op), "ignored": true, "reason": "no_focus"});
        };
        if !ctx.text_like {
            return json!({"op": op_name(op), "ignored": true, "reason": "no_text_focus"});
        }
        let selected = text_edit::selected_text(&ctx.value, ctx.caret);
        match op {
            ClipOp::Copy => {
                let payload = selected.clone().unwrap_or_else(|| ctx.value.clone());
                match clipboard::write_text(&payload) {
                    Ok(()) => {
                        if selected.is_some() {
                            self.note_human_event(pid, "人工复制(选区)");
                        } else {
                            self.note_human_event(pid, "人工复制(整值)");
                        }
                        json!({"op": "copy", "ok": true, "selector": focused, "len": payload.chars().count()})
                    }
                    Err(e) => json!({"op": "copy", "ok": false, "error": e}),
                }
            }
            ClipOp::Cut => {
                if !ctx.editable {
                    return json!({"op": "cut", "ignored": true, "reason": "readonly_or_disabled"});
                }
                let payload = selected.clone().unwrap_or_else(|| ctx.value.clone());
                match clipboard::write_text(&payload) {
                    Ok(()) => {
                        // 有选区 = 删选区;无选区 = 清空整值(旧行为)。
                        let (next, caret) = if selected.is_some() {
                            text_edit::edit_backspace(&ctx.value, ctx.caret)
                        } else {
                            (String::new(), CaretState::collapsed(0))
                        };
                        self.note_human_event(pid, "人工剪切");
                        let mut out = self
                            .text_commit(pid, &focused, next, caret, "cut", Some((ctx.value, ctx.caret)))
                            .await;
                        stamp_op(&mut out, "cut");
                        out
                    }
                    Err(e) => json!({"op": "cut", "ok": false, "error": e}),
                }
            }
            ClipOp::Paste => {
                if !ctx.editable {
                    return json!({"op": "paste", "ignored": true, "reason": "readonly_or_disabled"});
                }
                match clipboard::read_text() {
                    Ok(text) if !text.is_empty() => {
                        // 归一换行(\r\n → \n),与 textarea 行模型一致。
                        let text = text.replace("\r\n", "\n");
                        self.note_human_event(pid, "人工粘贴");
                        let (next, caret) =
                            text_edit::edit_insert(&ctx.value, ctx.caret, &text);
                        let mut out = self
                            .text_commit(
                                pid,
                                &focused,
                                next,
                                caret,
                                "paste",
                                Some((ctx.value, ctx.caret)),
                            )
                            .await;
                        stamp_op(&mut out, "paste");
                        out
                    }
                    Ok(_) => json!({"op": "paste", "ok": false, "error": "剪贴板为空"}),
                    Err(e) => json!({"op": "paste", "ok": false, "error": e}),
                }
            }
        }
    }

    /// 编辑组合键(Ctrl+A / Ctrl+Z / Ctrl+Y)。
    pub(super) async fn human_edit_op(&self, pid: &str, op: EditOp) -> Value {
        let ctx = self.collect_text_ctx(pid).await;
        let Some(focused) = ctx.focused.clone() else {
            return json!({"op": format!("{op:?}"), "ignored": true, "reason": "no_focus"});
        };
        if !ctx.text_like {
            return json!({"op": format!("{op:?}"), "ignored": true, "reason": "no_text_focus"});
        }
        match op {
            EditOp::SelectAll => {
                let caret = text_edit::select_all(&ctx.value);
                self.note_human_event(pid, "人工全选");
                self.caret_commit(pid, &focused, caret).await
            }
            EditOp::Undo | EditOp::Redo => {
                if !ctx.editable {
                    return json!({"op": format!("{op:?}"), "ignored": true, "reason": "readonly_or_disabled"});
                }
                let restored = {
                    let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                    let h = s.page(pid).edits.entry(focused.clone()).or_default();
                    if op == EditOp::Undo {
                        h.undo(ctx.value.clone(), ctx.caret)
                    } else {
                        h.redo(ctx.value.clone(), ctx.caret)
                    }
                };
                match restored {
                    Some((value, caret)) => {
                        self.note_human_event(pid, if op == EditOp::Undo { "人工撤销" } else { "人工重做" });
                        self.text_commit(pid, &focused, value, caret, "undo", None).await
                    }
                    None => json!({"op": format!("{op:?}"), "ok": false, "reason": "empty_history"}),
                }
            }
        }
    }

    /// 键盘:组合键(⌘/Ctrl+C/X/V、A/Z/Y)→ 剪贴板/编辑;其余先派 JS keydown
    /// (带 e.key/e.keyCode 载荷),再按原生语义路由,收尾尽力补发 keyup。
    pub(super) async fn human_key(&self, pid: &str, key: &str, mods: &Mods) -> Value {
        // P2c:查找模式(Ctrl/Cmd+F)下全部键进查找栏,不产生页面语义。
        if self.find_active(pid).await {
            return self.find_key(pid, key, mods).await;
        }
        if let Some(op) = clip_op_for(key, mods) {
            return self.handle_human_clip(pid, op).await;
        }
        if let Some(op) = edit_op_for(key, mods) {
            return self.human_edit_op(pid, op).await;
        }
        // P2a:文本编辑类组合键(Ctrl+方向词跳/Ctrl+Backspace 删词/Ctrl+Home·End)
        // 放行进原生语义路由;其余组合键(非纯 Shift)不产生页面文本/导航语义。
        let text_combo = (mods.ctrl && !mods.meta && !mods.alt)
            && matches!(
                key,
                "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Backspace" | "Home" | "End"
            );
        if mods.combo() && !text_combo {
            return json!({"key": key, "ignored": true, "reason": "combo"});
        }
        let out = Box::pin(self.human_key_inner(pid, key, mods)).await;
        // keyup 尽力派发(Enter 提交等路径可能已换页,自然 no-op)。
        let focused = {
            let inner = self.inner.lock().await;
            inner.tabs.get(pid).and_then(|t| t.focused.clone())
        };
        if let Some(sel) = focused {
            if let Some(reply) = api::js_dispatch_bounded_with(
                self,
                pid,
                &sel,
                "keyup",
                Some(key_payload(key, mods)),
            )
            .await
            {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        out
    }

    /// 键盘原生语义路由(keydown 已派发后的默认行为)。
    async fn human_key_inner(&self, pid: &str, key: &str, mods: &Mods) -> Value {
        let ctx = self.collect_text_ctx(pid).await;
        // JS keydown(P1 起带 e.key/e.keyCode/修饰键载荷)。
        if let Some(sel) = ctx.focused.clone() {
            if let Some(reply) = api::js_dispatch_bounded_with(
                self,
                pid,
                &sel,
                "keydown",
                Some(key_payload(key, mods)),
            )
            .await
            {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        match key {
            "Enter" if ctx.text_like && ctx.ftag == "textarea" && ctx.editable => {
                let sel = ctx.focused.clone().unwrap();
                return self.text_edit_key(pid, &sel, &ctx, |v, c| text_edit::edit_insert(v, c, "\n"), "Enter")
                    .await;
            }
            "Enter"
                if ctx.ftag == "button"
                    || (ctx.ftag == "input"
                        && matches!(ctx.ftyp.as_str(), "button" | "submit" | "reset")) =>
            {
                let sel = ctx.focused.clone().unwrap();
                let out = api::click_inner(self, pid, &json!({"selector": sel}), "human").await;
                self.note_human_event(pid, "人工 Enter 触发按钮");
                return json!({"key": key, "activated": true, "outcome": out.ok()});
            }
            // 表单默认提交路径(submit 事件可被 preventDefault 拦截)。
            "Enter" => {
                let params = match &ctx.focused {
                    Some(sel) => json!({"key": "Enter", "selector": sel}),
                    None => json!({"key": "Enter"}),
                };
                let out = api::act_key_press(self, pid, &params, &json!({})).await;
                return json!({"key": key, "outcome": out.ok()});
            }
            "Escape" => {
                // 收起展开的 select 面板(light dismiss)→ 关闭顶层 dialog。
                let panels = {
                    let mut inner = self.inner.lock().await;
                    let Some(tab) = inner.tabs.get_mut(pid) else {
                        return json!({"key": key, "ignored": true});
                    };
                    let n = tab.widget_open.len();
                    tab.widget_open.clear();
                    n
                };
                let dialog = api::act_key_press(self, pid, &json!({"key": "Escape"}), &json!({})).await;
                api::relayout_and_push(self, pid).await;
                return json!({"key": key, "panels_closed": panels, "dialog": dialog.ok()});
            }
            "Tab" => {
                // P2c:模态 dialog 焦点锁 —— 有模态框时 Tab/Shift+Tab 只在
                // dialog 子树内循环(无模态框保持全量文档序,与现状一致)。
                // `ctx.focusables` 由 `collect_text_ctx` 已算好,直接复用
                // (避免在锁内重跑一遍 `focusable_selectors` 的 O(n·boxes) 扫描)。
                let focus_list = {
                    let inner = self.inner.lock().await;
                    let Some(tab) = inner.tabs.get(pid) else {
                        return json!({"key": key, "moved": false});
                    };
                    let doc = tab.document_with_state();
                    if dialog_selectors(&doc).is_empty() {
                        ctx.focusables.clone()
                    } else {
                        ctx.focusables
                            .iter()
                            .filter(|s| in_open_dialog(&doc, s))
                            .cloned()
                            .collect::<Vec<_>>()
                    }
                };
                let next = next_focusable(&focus_list, ctx.focused.as_deref(), !mods.shift);
                let Some(next) = next else {
                    return json!({"key": key, "moved": false});
                };
                {
                    let mut inner = self.inner.lock().await;
                    if let Some(tab) = inner.tabs.get_mut(pid) {
                        if let Some(old) = tab.focused.take() {
                            api::set_pseudo_state(tab, &old, api::PseudoDim::Focus, false);
                        }
                        tab.focused = Some(next.clone());
                        api::set_pseudo_state(tab, &next, api::PseudoDim::Focus, true);
                    }
                }
                api::relayout_and_push(self, pid).await;
                return json!({"key": key, "moved": true, "focused": next, "backward": mods.shift});
            }
            "Backspace" if ctx.editable => {
                let sel = ctx.focused.clone().unwrap();
                let word = mods.ctrl;
                return self
                    .text_edit_key(pid, &sel, &ctx, move |v, c| {
                        if word {
                            text_edit::edit_delete_word_back(v, c)
                        } else {
                            text_edit::edit_backspace(v, c)
                        }
                    }, if word { "Ctrl+Backspace" } else { "Backspace" })
                    .await;
            }
            "Delete" if ctx.editable => {
                let sel = ctx.focused.clone().unwrap();
                return self
                    .text_edit_key(pid, &sel, &ctx, text_edit::edit_delete, "Delete")
                    .await;
            }
            "Space" if ctx.editable => {
                let sel = ctx.focused.clone().unwrap();
                return self.text_edit_key(pid, &sel, &ctx, |v, c| text_edit::edit_insert(v, c, " "), "Space").await;
            }
            "Space"
                if ctx.ftag == "button"
                    || (ctx.ftag == "input"
                        && matches!(
                            ctx.ftyp.as_str(),
                            "button" | "submit" | "reset" | "checkbox" | "radio"
                        )) =>
            {
                let sel = ctx.focused.clone().unwrap();
                let out = api::click_inner(self, pid, &json!({"selector": sel}), "human").await;
                self.note_human_event(pid, "人工 Space 激活控件");
                return json!({"key": key, "activated": true, "outcome": out.ok()});
            }
            "Space" => {
                return self.scroll_viewport_by(pid, 0.0, -PAGE_RATIO).await;
            }
            "ArrowUp" | "ArrowDown" => {
                let dir = if key == "ArrowUp" { -1 } else { 1 };
                if ctx.ftag == "select" {
                    return self.step_select(pid, ctx.focused.as_deref().unwrap_or(""), dir).await;
                }
                if ctx.ftag == "input" && ctx.ftyp == "range" {
                    return self.step_range(pid, ctx.focused.as_deref().unwrap_or(""), dir).await;
                }
                if ctx.text_like {
                    let sel = ctx.focused.clone().unwrap();
                    // textarea 按渲染同源折行行移动;单行 input = Home/End 语义。
                    let next = if ctx.ftag == "textarea" {
                        text_edit::caret_move_vert(&ctx.value, ctx.caret, &ctx.rows, dir, mods.shift)
                    } else {
                        let target = if dir < 0 { 0 } else { ctx.value.chars().count() };
                        text_edit::move_to(&ctx.value, ctx.caret, target, mods.shift)
                    };
                    return self.caret_commit(pid, &sel, next).await;
                }
                if ctx.focused.is_none() {
                    return self.scroll_viewport_by(pid, 0.0, -(dir as f64)).await;
                }
                return json!({"key": key, "ignored": true});
            }
            "ArrowLeft" | "ArrowRight" => {
                let dir = if key == "ArrowRight" { 1 } else { -1 };
                if ctx.ftag == "input" && ctx.ftyp == "range" {
                    return self.step_range(pid, ctx.focused.as_deref().unwrap_or(""), dir).await;
                }
                if ctx.text_like {
                    let sel = ctx.focused.clone().unwrap();
                    let next =
                        text_edit::caret_move(&ctx.value, ctx.caret, dir, mods.ctrl, mods.shift);
                    return self.caret_commit(pid, &sel, next).await;
                }
                if ctx.focused.is_none() {
                    return self.scroll_viewport_by(pid, dir as f64, 0.0).await;
                }
                return json!({"key": key, "ignored": true});
            }
            "PageUp" | "PageDown" => {
                let dy = if key == "PageDown" { -PAGE_RATIO } else { PAGE_RATIO };
                return self.scroll_viewport_by(pid, 0.0, dy).await;
            }
            "Home" | "End" if ctx.text_like => {
                let sel = ctx.focused.clone().unwrap();
                let end = key == "End";
                let next = text_edit::caret_to_edge(
                    &ctx.value,
                    ctx.caret,
                    &ctx.rows,
                    end,
                    mods.ctrl,
                    mods.shift,
                );
                return self.caret_commit(pid, &sel, next).await;
            }
            "Home" => {
                return self.scroll_viewport_to(pid, 0.0, 0.0).await;
            }
            "End" => {
                let page_h = {
                    let inner = self.inner.lock().await;
                    inner
                        .tabs
                        .get(pid)
                        .map(|t| t.layout.page_height() - t.viewport.height as f64)
                        .unwrap_or(0.0)
                };
                return self.scroll_viewport_to(pid, 0.0, page_h.max(0.0)).await;
            }
            _ => {}
        }
        // 可打印单字符:插入聚焦文本控件的光标处(P2a;替换选区)。
        if key.chars().count() == 1 {
            if ctx.editable {
                let sel = ctx.focused.clone().unwrap();
                let ch = key.chars().next().unwrap();
                let piece = if mods.shift { shift_char(ch).to_string() } else { ch.to_string() };
                return self
                    .text_edit_key(pid, &sel, &ctx, move |v, c| text_edit::edit_insert(v, c, &piece), "char")
                    .await;
            }
            return json!({"key": key, "ignored": true, "reason": "no_text_focus"});
        }
        json!({"key": key, "ignored": true})
    }

    // ---------- 文本编辑状态读写 ----------

    /// 收集聚焦控件的编辑上下文(值/光标/折行/可聚焦集;文档对象不跨 await)。
    async fn collect_text_ctx(&self, pid: &str) -> TextEditCtx {
        let inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get(pid) else {
            return TextEditCtx {
                focused: None,
                ftag: String::new(),
                ftyp: String::new(),
                text_like: false,
                editable: false,
                focusables: Vec::new(),
                value: String::new(),
                caret: CaretState::default(),
                rows: Vec::new(),
            };
        };
        let doc = tab.document_with_state();
        let focused = tab.focused.clone();
        let (mut ftag, mut ftyp) = (String::new(), String::new());
        let (mut text_like, mut editable) = (false, false);
        let (mut value, mut caret, mut rows) = (String::new(), CaretState::default(), Vec::new());
        if let Some(sel) = &focused {
            if let Some(node) = doc.select(sel).ok().and_then(|v| v.into_iter().next()) {
                ftag = document::tag_name(&node);
                ftyp = document::attr(&node, "type").unwrap_or_default().to_ascii_lowercase();
                let disabled = document::attr(&node, "disabled").is_some();
                let readonly = document::attr(&node, "readonly").is_some();
                text_like = is_text_like(&ftag, &ftyp);
                editable = text_like && !disabled && !readonly;
                if text_like {
                    value = tab
                        .form_values
                        .get(sel)
                        .cloned()
                        .or_else(|| document::attr(&node, "value"))
                        .unwrap_or_default();
                    if ftag == "textarea" {
                        value = value.replace("\r\n", "\n");
                    }
                    let len = value.chars().count();
                    caret = tab.form_caret.get(sel).copied().unwrap_or_else(|| CaretState::collapsed(len));
                    caret.pos = caret.pos.min(len);
                    caret.anchor = caret.anchor.min(len);
                    if ftag == "textarea" {
                        // 折行用渲染同源几何(widgets:字号 max(8)、可用宽 w-6)。
                        let (w, h) = tab
                            .layout
                            .rect_for(sel)
                            .map(|(_, _, w, h)| (w, h))
                            .unwrap_or((200.0, 60.0));
                        let size = widget_font_size(&doc, sel);
                        let line_height = size * 1.35;
                        let max_rows = (((h - 4.0) / line_height).floor() as usize).max(1);
                        rows = self
                            .font
                            .wrap_text_rows(&value, size, (w - 6.0).max(1.0), max_rows.max(64));
                    }
                }
            }
        }
        TextEditCtx {
            focused,
            ftag,
            ftyp,
            text_like,
            editable,
            focusables: focusable_selectors(tab, &doc),
            value,
            caret,
            rows,
        }
    }

    /// 一次文本编辑的收口:入撤销栈 → 写值/光标 → JS input → 重排推帧。
    async fn text_edit_key<F>(
        &self,
        pid: &str,
        selector: &str,
        ctx: &TextEditCtx,
        op: F,
        via: &str,
    ) -> Value
    where
        F: FnOnce(&str, CaretState) -> (String, CaretState),
    {
        let (next, caret) = op(&ctx.value, ctx.caret);
        if next == ctx.value && caret == ctx.caret {
            return json!({"via": via, "selector": selector, "unchanged": true});
        }
        self.text_commit(pid, selector, next, caret, via, Some((ctx.value.clone(), ctx.caret)))
            .await
    }

    /// 写值/光标 + 派发 input + 重排推帧;`snapshot` 为编辑前 `(值, 光标)`,
    /// 有则先入撤销栈(撤销/重做路径自身不入栈,传 None)。
    pub(super) async fn text_commit(
        &self,
        pid: &str,
        selector: &str,
        value: String,
        caret: CaretState,
        via: &str,
        snapshot: Option<(String, CaretState)>,
    ) -> Value {
        {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((old_v, old_c)) = snapshot {
                s.page(pid).edits.entry(selector.to_string()).or_default().push(old_v, old_c);
            }
        }
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.form_values.insert(selector.to_string(), value.clone());
                tab.form_caret.insert(selector.to_string(), caret);
            }
        }
        if let Some(reply) = api::js_dispatch_bounded(self, pid, selector, "input").await {
            api::apply_js_reply(self, pid, &reply).await;
        }
        api::relayout_and_push(self, pid).await;
        json!({"via": via, "selector": selector, "value_len": value.chars().count(), "caret": caret.pos})
    }

    /// 只更新光标/选区(不改值、不派 input),重排推帧刷新光标条/选区高亮。
    pub(super) async fn caret_commit(&self, pid: &str, selector: &str, caret: CaretState) -> Value {
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.form_caret.insert(selector.to_string(), caret);
            }
        }
        api::relayout_and_push(self, pid).await;
        json!({"selector": selector, "caret": caret.pos, "anchor": caret.anchor, "selected": !caret.is_empty()})
    }

    // ---------- 控件换项/步进(键路由;原 mod.rs 机械移入) ----------

    /// select 上下键换项(文档序 ±1,派发 change)。
    async fn step_select(&self, pid: &str, selector: &str, dir: i32) -> Value {
        let picked = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return json!({"ignored": true});
            };
            let doc = tab.document_with_state();
            let Some(node) = doc.select(selector).ok().and_then(|v| v.into_iter().next()) else {
                return json!({"ignored": true});
            };
            // 只取该 select 直接的 option(文档序)。
            let options: Vec<(String, String)> = node
                .select("option")
                .ok()
                .map(|os| {
                    os.into_iter()
                        .map(|o| {
                            let o = o.as_node();
                            let label = document::collapse_ws(&o.text_contents());
                            let value =
                                document::attr(o, "value").unwrap_or_else(|| label.clone());
                            (value, label)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if options.is_empty() {
                return json!({"key": "select", "ignored": true});
            }
            let cur_value = tab
                .form_values
                .get(selector)
                .cloned()
                .or_else(|| {
                    node.select("option")
                        .ok()
                        .and_then(|os| {
                            os.into_iter()
                                .find(|o| document::attr(o.as_node(), "selected").is_some())
                        })
                        .and_then(|o| document::attr(o.as_node(), "value"))
                })
                .unwrap_or_else(|| options[0].0.clone());
            let cur_idx = options
                .iter()
                .position(|(v, _)| *v == cur_value)
                .unwrap_or(0) as i64;
            let next = (cur_idx + dir as i64).clamp(0, options.len() as i64 - 1) as usize;
            options[next].clone()
        };
        self.set_runtime_value(pid, selector, &picked.0).await;
        if let Some(reply) = api::js_dispatch_bounded(self, pid, selector, "change").await {
            api::apply_js_reply(self, pid, &reply).await;
        }
        api::relayout_and_push(self, pid).await;
        json!({"selector": selector, "value": picked.0, "label": picked.1})
    }

    /// range 步进(ArrowLeft/Right)。
    async fn step_range(&self, pid: &str, selector: &str, dir: i32) -> Value {
        let next_val = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return json!({"ignored": true});
            };
            let doc = tab.document_with_state();
            let Some(node) = doc.select(selector).ok().and_then(|v| v.into_iter().next()) else {
                return json!({"ignored": true});
            };
            let (min, max, step) = range_params(&node);
            let cur = tab
                .form_values
                .get(selector)
                .cloned()
                .or_else(|| document::attr(&node, "value"))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(min + (max - min) / 2.0);
            let next = (cur + dir as f64 * step).clamp(min.min(max), max.max(min));
            format_num(next)
        };
        self.set_runtime_value(pid, selector, &next_val).await;
        if let Some(reply) = api::js_dispatch_bounded(self, pid, selector, "input").await {
            api::apply_js_reply(self, pid, &reply).await;
        }
        api::relayout_and_push(self, pid).await;
        json!({"selector": selector, "value": next_val})
    }
}

/// 控件字号(渲染用 `style.font_size.max(8.0)`;键路由折行需要同一数值)。
fn widget_font_size(doc: &Document, selector: &str) -> f64 {
    let Some(node) = doc.select(selector).ok().and_then(|v| v.into_iter().next()) else {
        return 16.0;
    };
    let engine = StyleEngine::from_document(doc, MediaCtx::new(1920.0, 1080.0));
    node_font_size(&engine, &node)
}

/// 给 text_commit 回执补 `op`/`ok` 信封(剪贴板调用方按 `ok` 判定成功)。
fn stamp_op(out: &mut Value, op: &str) {
    if let Some(obj) = out.as_object_mut() {
        obj.insert("op".to_string(), json!(op));
        obj.insert("ok".to_string(), json!(true));
    }
}
