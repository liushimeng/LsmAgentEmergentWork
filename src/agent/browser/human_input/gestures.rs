//! 手势处理器:指针移动 / 按下 / 抬起 / 滚轮(39 号设计文档宿主管线)。

//! 从 `mod.rs` 拆出(1800 行规范):`human_move`/`human_down`/`human_up`/
//! `human_wheel` 四个手势处理器逐行机械搬移至此,方法仍是
//! `impl LsmBrowserManager` 块,调用方(`mod.rs::handle_human_input` 等)
//! 路径零改动;状态类型与纯函数仍在 `mod.rs`。

use super::*;

impl LsmBrowserManager {
    /// 指针移动:hover 链更新(目标变化才重排)+ HTML5 drag 近似 +
    /// JS mousemove 节流派发(带坐标载荷)。
    pub(super) async fn human_move(&self, pid: &str, x: f64, y: f64) -> Value {
        let hit = self.human_hit_test(pid, x, y).await;
        let hit_sel = hit.as_ref().map(|h| h.selector.clone());
        // P2c:光标形状跟随悬停目标(仅在变化时发帧)。
        if let Some(h) = &hit {
            self.push_cursor_kind(pid, h.cursor).await;
        }
        // HTML5 drag 近似:按下中且位移超阈值 → dragstart 一次,后续 dragover。
        // 无 dataTransfer 模型(页面拿不到拖拽数据),只保证事件序列与坐标。
        let drag_evt: Option<(String, String, bool)> = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            match (&st.press, &hit) {
                // P2a:文本控件内拖动是选区拖选,不触发 HTML5 drag。
                (Some(p), Some(h))
                    if p.selector.is_some()
                        && st.text_drag.is_none()
                        && (p.x - x).hypot(p.y - y) > CLICK_DIST =>
                {
                    let src = p.selector.clone().unwrap_or_default();
                    if !st.drag_started {
                        st.drag_started = true;
                        Some(("dragstart".to_string(), src, true))
                    } else {
                        Some(("dragover".to_string(), h.selector.clone(), false))
                    }
                }
                _ => None,
            }
        };
        if let Some((ev, sel, immediate)) = drag_evt {
            let dispatch = if immediate {
                api::js_dispatch_bounded_with(
                    self, pid, &sel, &ev,
                    Some(mouse_payload(x, y, MouseButton::Left, true, 0)),
                )
                .await
            } else {
                let due = {
                    let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                    let st = s.page(pid);
                    let now = now_ms();
                    if now.saturating_sub(st.last_js_move_ms) >= JS_MOVE_THROTTLE_MS {
                        st.last_js_move_ms = now;
                        true
                    } else {
                        false
                    }
                };
                if due {
                    api::js_dispatch_bounded_with(
                        self, pid, &sel, &ev,
                        Some(mouse_payload(x, y, MouseButton::Left, true, 0)),
                    )
                    .await
                } else {
                    None
                }
            };
            if let Some(reply) = dispatch {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        // P2a:文本控件内拖动 = 选区拖选(锚点固定、头随指针;浏览器语义)。
        let text_drag = {
            let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            s.pages.get(pid).and_then(|st| st.text_drag.clone())
        };
        if let Some(td) = text_drag {
            let pressed = {
                let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                s.pages.get(pid).map(|st| st.press.is_some()).unwrap_or(false)
            };
            if pressed {
                if let Some(h) = hit.as_ref().filter(|h| h.selector == td.selector) {
                    if let Some(idx) = self.text_caret_at(pid, h, x, y).await {
                        let changed = {
                            let mut inner = self.inner.lock().await;
                            match inner.tabs.get_mut(pid).and_then(|t| t.form_caret.get_mut(&td.selector)) {
                                Some(st) if st.pos != idx => {
                                    st.pos = idx;
                                    true
                                }
                                _ => false,
                            }
                        };
                        if changed {
                            {
                                let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                                s.page(pid).text_dragging = true;
                            }
                            api::relayout_and_push(self, pid).await;
                        }
                    }
                }
            }
        }
        // P2b:页面级自由选区拖选(锚点固定、头随指针)。
        //
        // 索引映射是纯几何计算(不碰 DOM 重排),所以**每次移动都刷新头端**;
        // 只有「头端元素变化」才需要重排(与 hover 去重同口径)。
        let page_drag = {
            let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            s.pages.get(pid).and_then(|st| st.page_drag.clone())
        };
        if let Some(pd) = page_drag {
            let pressed = {
                let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                s.pages.get(pid).map(|st| st.press.is_some()).unwrap_or(false)
            };
            let moved = (pd.start_x - x).hypot(pd.start_y - y) > CLICK_DIST;
            if pressed && moved {
                // 头端元素:命中可选文本元素时用命中者,否则沿用锚点元素
                //(拖过容器间隙时选区仍连续,不因一次未命中而断)。
                let head = hit.as_ref().filter(|h| is_page_selectable(&h.tag, &h.typ));
                let head_sel = head
                    .as_ref()
                    .map(|h| h.selector.clone())
                    .unwrap_or_else(|| pd.anchor_sel.clone());
                let head_idx = match head {
                    Some(h) => self.page_text_caret_at(pid, h, x, y).await.unwrap_or(pd.anchor_idx),
                    None => pd.anchor_idx,
                };
                let mut relayout = false;
                {
                    let mut inner = self.inner.lock().await;
                    if let Some(tab) = inner.tabs.get_mut(pid) {
                        let need = match &tab.page_sel {
                            Some(cur) => {
                                cur.anchor_sel != pd.anchor_sel
                                    || cur.head_sel != head_sel
                                    || cur.head_idx != head_idx
                            }
                            None => true,
                        };
                        if need {
                            tab.page_sel = Some(document::PageSelection {
                                anchor_sel: pd.anchor_sel.clone(),
                                anchor_idx: pd.anchor_idx,
                                head_sel: head_sel.clone(),
                                head_idx,
                            });
                            relayout = true;
                        }
                    }
                }
                if relayout {
                    {
                        let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                        s.page(pid).page_dragging = true;
                    }
                    api::relayout_and_push(self, pid).await;
                }
            }
        }
        // 拖拽中的滑块:按住移动持续设值(浏览器拖拽滑块语义)。
        let press_range = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            match (&st.press, &hit) {
                (Some(p), Some(h))
                    if p.selector.as_deref() == Some(h.selector.as_str())
                        && h.tag == "input"
                        && h.typ == "range" =>
                {
                    Some((h.selector.clone(), h.rect))
                }
                _ => None,
            }
        };
        if let Some((sel, rect)) = &press_range {
            self.set_range_by_x(pid, sel, *rect, x).await;
            api::relayout_and_push(self, pid).await;
        }
        let mut focused = None;
        let (changed, hover_prev) = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            let changed = st.hover.as_deref() != hit_sel.as_deref();
            let prev = st.hover.clone();
            if changed {
                st.hover = hit_sel.clone();
            }
            (changed, prev)
        };
        // P2d:hover 目标切换派发兼容事件:旧目标 pointerout/mouseout、
        // 新目标 pointerover/mouseover(指针事件先于 mouse 兼容事件)。
        // JS 驱动的悬停菜单/tooltip(只挂 mouseover/pointerover)由此可用。
        if changed {
            self.dispatch_hover_change(pid, hover_prev.as_deref(), hit_sel.as_deref(), x, y)
                .await;
        }
        if changed {
            // 锁块收窄:relayout_and_push 内部会再锁 inner(tokio Mutex 非重入,
            // 持锁调用会自死锁 —— 拖拽测试首轮即命中,修复于阶段二)。
            {
                let mut inner = self.inner.lock().await;
                if let Some(tab) = inner.tabs.get_mut(pid) {
                    tab.mouse = Some((x, y));
                    focused = tab.focused.clone();
                    match &hit {
                        Some(h) => api::set_hover_chain(tab, &h.hover_chain),
                        None => api::set_pseudo_state(tab, "", api::PseudoDim::Hover, false),
                    }
                }
            }
            api::relayout_and_push(self, pid).await;
        }
        // JS mousemove:命中目标(无命中时聚焦元素)节流派发,页面 hover 菜单可用。
        if focused.is_none() {
            let inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get(pid) {
                focused = tab.focused.clone();
            }
        }
        let pressed = {
            let s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            s.pages.get(pid).map(|st| st.press.is_some()).unwrap_or(false)
        };
        let due = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            let now = now_ms();
            if now.saturating_sub(st.last_js_move_ms) >= JS_MOVE_THROTTLE_MS {
                st.last_js_move_ms = now;
                true
            } else {
                false
            }
        };
        if due {
            if let Some(sel) = hit_sel.as_deref().or(focused.as_deref()) {
                // P2d:pointermove+mousemove 成对一次往返(共用 100ms 节流)。
                if let Some(reply) = api::js_dispatch_seq_bounded(
                    self,
                    pid,
                    sel,
                    vec![
                        ("pointermove", Some(pointer::pointer_payload(x, y, MouseButton::Left, pressed, 0))),
                        ("mousemove", Some(mouse_payload(x, y, MouseButton::Left, pressed, 0))),
                    ],
                )
                .await
                {
                    api::apply_js_reply(self, pid, &reply).await;
                }
            }
        }
        // P2c:回执带光标形状(测试断言与日志用;查看器不消费回执)。
        let cursor = hit
            .as_ref()
            .map(|h| h.cursor.as_str())
            .unwrap_or("default");
        json!({"x": x, "y": y, "hover": hit_sel, "restyled": changed, "cursor": cursor})
    }

    /// 按下:hover 链 + `:active` + 可聚焦元素聚焦 + JS mousedown;
    /// P2a:文本控件内按下 = 定位光标(Shift+点击 = 扩选)并武装选区拖选。
    pub(super) async fn human_down(&self, pid: &str, x: f64, y: f64, button: MouseButton, mods: Mods) -> Value {
        if button != MouseButton::Left {
            // 右键/中键在抬起时处理(contextmenu/新标签),按下不产生页面语义。
            return json!({"x": x, "y": y, "button": "non-left", "pressed": false});
        }
        let hit = self.human_hit_test(pid, x, y).await;
        {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            st.press = Some(PressState {
                selector: hit.as_ref().map(|h| h.selector.clone()),
                x,
                y,
            });
            st.text_drag = None;
            st.text_dragging = false;
            st.page_drag = None;
            st.page_dragging = false;
        }
        let Some(hit) = hit else {
            return json!({"x": x, "y": y, "hit": false, "pressed": true});
        };
        self.push_cursor_kind(pid, hit.cursor).await;
        // disabled 控件:人工点击不分发事件也不聚焦(与 Agent 侧 `click_inner`
        // 的 disabled 闸同语义;39 号 P2b 顺带补齐)。
        let disabled = is_disabled_control(&hit.tag, &hit.typ)
            && {
                let inner = self.inner.lock().await;
                inner
                    .tabs
                    .get(pid)
                    .map(|t| control_disabled(t, &hit.selector))
                    .unwrap_or(false)
            };
        let focusable = !disabled && is_focusable(&hit.tag, &hit.typ);
        // P2a:文本控件点击定位光标(Shift+点击沿用旧锚点扩选)。
        let mut caret_out = Value::Null;
        if !disabled && is_text_like(&hit.tag, &hit.typ) {
            if let Some(st) = self.place_text_caret(pid, &hit, x, y, mods.shift).await {
                {
                    let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                    s.page(pid).text_drag =
                        Some(TextDragState { selector: hit.selector.clone(), anchor: st.anchor });
                }
                caret_out = json!({"pos": st.pos, "anchor": st.anchor});
            }
        }
        // P2b:页面级自由选区锚点(非文本控件、有可见文本、非 user-select:none)。
        let mut page_sel_anchor = Value::Null;
        if !disabled && !is_text_like(&hit.tag, &hit.typ) && is_page_selectable(&hit.tag, &hit.typ)
        {
            let armed = {
                let inner = self.inner.lock().await;
                inner
                    .tabs
                    .get(pid)
                    .map(|t| {
                        let doc = t.document_with_state();
                        doc.select(&hit.selector)
                            .ok()
                            .and_then(|v| v.into_iter().next())
                            .map(|n| {
                                !user_select_none(
                                    &StyleEngine::from_document(
                                        &doc,
                                        MediaCtx::new(
                                            t.viewport.width as f64,
                                            t.viewport.height as f64,
                                        ),
                                    ),
                                    &n,
                                )
                            })
                            .unwrap_or(false)
                    })
                    .unwrap_or(false)
            };
            if armed {
                if let Some(idx) = self.page_text_caret_at(pid, &hit, x, y).await {
                    {
                        let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
                        s.page(pid).page_drag = Some(PageDragState {
                            anchor_sel: hit.selector.clone(),
                            anchor_idx: idx,
                            start_x: x,
                            start_y: y,
                        });
                    }
                    page_sel_anchor = json!({"selector": hit.selector, "idx": idx});
                }
            }
        }
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.mouse = Some((x, y));
                api::set_hover_chain(tab, &hit.hover_chain);
                api::set_pseudo_state(tab, &hit.selector, api::PseudoDim::Active, true);
                if focusable {
                    tab.focused = Some(hit.selector.clone());
                    api::set_pseudo_state(tab, &hit.selector, api::PseudoDim::Focus, true);
                }
            }
        }
        let sel = hit.selector.clone();
        let tag = hit.tag.clone();
        // P2d:pointerdown+mousedown **一次往返**成对派发(指针事件先行,
        // Chrome 兼容事件顺序;逐事件往返的开销会吃掉双击/三击连击窗口)。
        if let Some(reply) = api::js_dispatch_seq_bounded(
            self,
            pid,
            &sel,
            vec![
                ("pointerdown", Some(pointer::pointer_payload(x, y, MouseButton::Left, true, 1))),
                ("mousedown", Some(mouse_payload(x, y, MouseButton::Left, true, 1))),
            ],
        )
        .await
        {
            api::apply_js_reply(self, pid, &reply).await;
        }
        // P2d:点击聚焦带 list 的 input → 展开 datalist 联想面板
        //(点进联想输入框 = Chrome 的展开全量语义)。
        if focusable {
            self.on_focus_maybe_datalist(pid, &sel).await;
        }
        api::relayout_and_push(self, pid).await;
        json!({"x": x, "y": y, "hit": true, "selector": sel, "tag": tag, "focused": focusable, "disabled": disabled, "caret": caret_out, "page_sel_anchor": page_sel_anchor})
    }

    /// 抬起:同目标且位移小 → 完整点击语义(复用 `click_inner`);否则只发 mouseup。
    pub(super) async fn human_up(
        &self,
        pid: &str,
        x: f64,
        y: f64,
        button: MouseButton,
        mods: Mods,
        arrived: u128,
    ) -> Value {
        let (press, drag_started, text_drag, text_dragging, page_drag, page_dragging) = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            (
                st.press.take(),
                std::mem::take(&mut st.drag_started),
                st.text_drag.take(),
                std::mem::take(&mut st.text_dragging),
                st.page_drag.take(),
                std::mem::take(&mut st.page_dragging),
            )
        };
        // 释放按下态(无论是否成点击)。
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                api::release_active(tab);
            }
        }
        let Some(press) = press else {
            return json!({"x": x, "y": y, "released": false, "reason": "no_press"});
        };
        let dist = (press.x - x).hypot(press.y - y);
        let hit = self.human_hit_test(pid, x, y).await;

        // 非左键:右键 = contextmenu 事件;中键 = 链接新标签。
        match button {
            MouseButton::Right => {
                if let Some(h) = hit.as_ref() {
                    let sel = h.selector.clone();
                    if let Some(reply) = api::js_dispatch_bounded_with(
                        self,
                        pid,
                        &sel,
                        "contextmenu",
                        Some(mouse_payload(x, y, MouseButton::Right, false, 0)),
                    )
                    .await
                    {
                        api::apply_js_reply(self, pid, &reply).await;
                    }
                    self.note_human_event(pid, &format!("人工右键 ·{}", h.tag));
                    return json!({"button": "right", "selector": sel});
                }
                return json!({"button": "right", "hit": false});
            }
            MouseButton::Middle => {
                if let Some(h) = hit.as_ref().filter(|h| h.tag == "a") {
                    let href = self.attr_of(pid, &h.selector, "href").await;
                    let url = match href {
                        Some(href) => self.resolve_page_url(pid, &href).await,
                        None => None,
                    };
                    if let Some(url) = url {
                        let spawned = self.new_tab(pid, &url, 60_000).await;
                        self.note_human_event(pid, "人工中键新标签");
                        return match spawned {
                            Ok(o) => json!({"button": "middle", "new_tab": o.page_id}),
                            Err(e) => json!({"button": "middle", "error": e.message}),
                        };
                    }
                }
                return json!({"button": "middle", "hit": false});
            }
            MouseButton::Left => {}
        }

        // P2b:页面级自由选区收尾。拖过阈值 → 落 `page_sel` 并重排(高亮要重绘);
        // 未过阈值 → 清掉拖选态,回落既有 click 语义(见下方)。
        if let Some(pd) = page_drag {
            let moved = (pd.start_x - x).hypot(pd.start_y - y) > CLICK_DIST;
            if moved && page_dragging {
                // `page_sel` 已在拖选途中由 `human_move` 实时提交,这里只收口。
                api::relayout_and_push(self, pid).await;
                self.note_human_event(pid, "人工拖选(页面选区)");
                return json!({"x": x, "y": y, "clicked": false, "page_selection": true});
            }
            // 未越阈值:清掉拖选痕迹(半提交的 page_sel 一并清掉,回落 click 语义)。
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                if tab.page_sel.is_some() {
                    tab.page_sel = None;
                }
            }
        }
        // P2a:文本拖选收尾 = 只落选区,不合成 click / HTML5 drag(浏览器语义:
        // 文本框里按住拖动是选区手势,松开不算点击)。
        if text_dragging {
            if let Some(td) = &text_drag {
                if let Some(h) = hit.as_ref().filter(|h| h.selector == td.selector) {
                    if let Some(idx) = self.text_caret_at(pid, h, x, y).await {
                        let mut inner = self.inner.lock().await;
                        if let Some(tab) = inner.tabs.get_mut(pid) {
                            tab.form_caret.insert(
                                td.selector.clone(),
                                CaretState { pos: idx, anchor: td.anchor },
                            );
                        }
                    }
                }
                api::relayout_and_push(self, pid).await;
                self.note_human_event(pid, "人工拖选(文本选区)");
                return json!({"x": x, "y": y, "clicked": false, "selection": true});
            }
        }
        // 模态 dialog 遮罩点击关闭(39 号 P2b;浏览器语义:点 `::backdrop` 关窗)。
        // 放在点击合成之前:点到 dialog 之外的任意元素(或**没有命中任何元素**,
        // 即点在了遮罩上)都算「点遮罩」,与真实浏览器一致。
        let backdrop_hit = match hit.as_ref() {
            Some(h) => !in_open_dialog_hit(self, pid, &h.selector).await,
            // 无命中:若存在模态 dialog,这点必然落在遮罩上。
            None => self.has_modal_dialog(pid).await,
        };
        if backdrop_hit {
            let closed = self.try_close_modal_backdrop(pid).await;
            if closed {
                api::relayout_and_push(self, pid).await;
                self.note_human_event(pid, "人工点击遮罩关闭弹窗");
                return json!({"x": x, "y": y, "clicked": false, "dialog_closed": true});
            }
        }

        // 左键:同目标且位移小 → 点击;否则视为拖拽收尾。
        let same_target = press.selector.is_some()
            && hit.as_ref().map(|h| h.selector.as_str()) == press.selector.as_deref();
        if !same_target || dist > CLICK_DIST {
            // HTML5 drag 近似:dragstart 已派发 → drop(命中他者)+ dragend(源)。
            if drag_started {
                if let Some(src) = press.selector.clone() {
                    if let Some(h) = hit.as_ref() {
                        if h.selector != src {
                            let drop_sel = h.selector.clone();
                            if let Some(reply) = api::js_dispatch_bounded_with(
                                self,
                                pid,
                                &drop_sel,
                                "drop",
                                Some(mouse_payload(x, y, MouseButton::Left, false, 0)),
                            )
                            .await
                            {
                                api::apply_js_reply(self, pid, &reply).await;
                            }
                        }
                    }
                    if let Some(reply) = api::js_dispatch_bounded_with(
                        self,
                        pid,
                        &src,
                        "dragend",
                        Some(mouse_payload(x, y, MouseButton::Left, false, 0)),
                    )
                    .await
                    {
                        api::apply_js_reply(self, pid, &reply).await;
                    }
                    self.note_human_event(pid, "人工拖拽(dragstart→drop/dragend)");
                    return json!({"x": x, "y": y, "clicked": false, "drag": true});
                }
            }
            if let Some(sel) = press.selector.clone().or(hit.as_ref().map(|h| h.selector.clone())) {
                // P2d:pointerup+mouseup 成对一次往返(拖拽收尾同样带 pointer 兼容事件)。
                if let Some(reply) = api::js_dispatch_seq_bounded(
                    self,
                    pid,
                    &sel,
                    vec![
                        ("pointerup", Some(pointer::pointer_payload(x, y, MouseButton::Left, false, 1))),
                        ("mouseup", Some(mouse_payload(x, y, MouseButton::Left, false, 1))),
                    ],
                )
                .await
                {
                    api::apply_js_reply(self, pid, &reply).await;
                }
                api::relayout_and_push(self, pid).await;
            }
            let reason = if dist > CLICK_DIST { "drag" } else { "target_changed" };
            return json!({"x": x, "y": y, "clicked": false, "reason": reason});
        }

        let mut target = hit.clone().unwrap();
        // label[for] 转发:点 label 等价于点关联控件(浏览器原生语义)。
        if target.tag == "label" {
            if let Some(for_id) = target.label_for.clone() {
                if let Some(fwd) = self.selector_by_id(pid, &for_id).await {
                    target.selector = fwd;
                }
            }
        }
        let sel = target.selector.clone();
        let tag = target.tag.clone();

        // 连击判定(同目标、500ms/6px 窗口):2 = 双击(选词 + JS dblclick),
        // 3+ = 三击(全选);窗口外重新计数。时刻用事件到达时刻(`arrived`)。
        let now = arrived;
        let count = {
            let mut s = self.human.lock().unwrap_or_else(|p| p.into_inner());
            let st = s.page(pid);
            let within = st
                .last_click
                .as_ref()
                .map(|m| {
                    m.selector == sel
                        && now.saturating_sub(m.at_ms) <= DBLCLICK_MS
                        && dist <= DBLCLICK_DIST
                })
                .unwrap_or(false);
            if within {
                let m = st.last_click.as_mut().unwrap();
                m.count = (m.count + 1).min(3);
                m.at_ms = now;
                m.count
            } else {
                st.last_click = Some(ClickMark {
                    selector: sel.clone(),
                    at_ms: now,
                    count: 1,
                });
                1
            }
        };
        let dbl = count == 2;
        let triple = count >= 3;

        // input[type=range]:点击轨道 = 按 x 比例设值(先设值再走点击,
        // 页面 click 监听可读到新值)。
        if tag == "input" && target.typ == "range" {
            self.set_range_by_x(pid, &sel, target.rect, x).await;
        }

        let mut params = json!({"selector": sel, "client_x": x, "client_y": y});
        // P2d:列表多选 select 的 option 点击携带修饰键(Ctrl 翻转/Shift 范围);
        // 「Cmd/Ctrl+点击 = 新标签」只是链接语义,option 目标不适用。
        let multi_option = tag == "option" && self.option_in_multi_select(pid, &sel).await;
        if multi_option {
            if mods.ctrl || mods.meta {
                params["multi_ctrl"] = json!(true);
            }
            if mods.shift {
                params["multi_shift"] = json!(true);
            }
        } else if mods.ctrl || mods.meta {
            // Cmd/Ctrl+点击 = 新标签打开(点击语义保留,导航目标改 _blank)。
            params["force_target"] = json!("_blank");
        }
        // P2d:pointerup 挂到 click_inner 的 click 派发前**同一次往返**发出
        //(完整序列 = 按下时 pointerdown/mousedown + 此处 pointerup + click;
        // 单独往返的 pointerup 会把双击/三击连击窗口吃掉)。
        params["pointer_pre_click"] = json!({"x": x, "y": y});
        let clicked = api::click_inner(self, pid, &params, "human").await;
        if dbl {
            if let Some(reply) = api::js_dispatch_bounded_with(
                self,
                pid,
                &sel,
                "dblclick",
                Some(mouse_payload(x, y, MouseButton::Left, false, 2)),
            )
            .await
            {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        // P2a:双击选词 / 三击全选(仅文本控件;单击已在 down 落光标)。
        if (dbl || triple) && text_drag.is_some() {
            let value = self.current_value(pid, &sel).await;
            let pos = {
                let inner = self.inner.lock().await;
                inner
                    .tabs
                    .get(pid)
                    .and_then(|t| t.form_caret.get(&sel).map(|c| c.pos))
                    .unwrap_or_else(|| value.chars().count())
            };
            let caret = if triple {
                text_edit::select_all(&value)
            } else {
                text_edit::select_word(&value, pos)
            };
            {
                let mut inner = self.inner.lock().await;
                if let Some(tab) = inner.tabs.get_mut(pid) {
                    tab.form_caret.insert(sel.clone(), caret);
                }
            }
            api::relayout_and_push(self, pid).await;
        }
        let label = if triple {
            "人工三击"
        } else if dbl {
            "人工双击"
        } else {
            "人工单击"
        };
        self.note_human_event(pid, &format!("{label} ·{tag}"));
        match clicked {
            Ok(out) => json!({"x": x, "y": y, "clicked": true, "selector": sel, "double": dbl, "count": count, "outcome": out}),
            Err(e) => json!({"x": x, "y": y, "clicked": false, "selector": sel, "count": count, "error": e.message}),
        }
    }

    /// 滚轮:命中元素向上找最近可滚容器 → 元素内滚动;否则视口滚动。
    pub(super) async fn human_wheel(&self, pid: &str, x: f64, y: f64, dx: f64, dy: f64) -> Value {
        let hit = self.human_hit_test(pid, x, y).await;
        // P2d:先派发 wheel 事件(页面可 preventDefault 阻止原生滚动,Chrome 语义)。
        // 命中目标优先,无命中回落聚焦元素;短 bound 防页面线程卡顿拖死滚动。
        if dx != 0.0 || dy != 0.0 {
            let wheel_sel = match hit.as_ref().map(|h| h.selector.clone()) {
                Some(sel) => Some(sel),
                None => {
                    let inner = self.inner.lock().await;
                    inner.tabs.get(pid).and_then(|t| t.focused.clone())
                }
            };
            if let Some(sel) = wheel_sel {
                if self.dispatch_wheel_event(pid, &sel, x, y, dx, dy).await {
                    return json!({"mode": "js_prevented", "selector": sel});
                }
            }
        }
        // 命中元素的可滚祖先链(含自身);文档对象不跨 await 存活,锁内收敛。
        let scroll_target: Option<String> = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return json!({"ignored": true});
            };
            match &hit {
                Some(h) => {
                    let doc = tab.document_with_state();
                    let mut cur = doc.select(&h.selector).ok().and_then(|v| v.into_iter().next());
                    let mut found = None;
                    while let Some(n) = cur {
                        let sel = Document::stable_selector(&n);
                        if tab.layout.scroll_max.get(&sel).copied().unwrap_or(0.0) > 0.0 {
                            found = Some(sel);
                            break;
                        }
                        cur = n.parent();
                    }
                    found
                }
                None => None,
            }
        };
        let mut inner = self.inner.lock().await;
        let Some(tab) = inner.tabs.get_mut(pid) else {
            return json!({"ignored": true});
        };
        if let Some(sel) = scroll_target {
            let cur = tab.scroll_offsets.get(&sel).copied().unwrap_or(0.0);
            let max = tab.layout.scroll_max.get(&sel).copied().unwrap_or(0.0);
            // dy>0 = 向上滚 = scrollTop 减小;dx>0 = 向右滚。
            let next_y = (cur - dy * WHEEL_STEP).clamp(0.0, max.max(0.0));
            tab.scroll_offsets.insert(sel.clone(), next_y);
            drop(inner);
            api::relayout_and_push(self, pid).await;
            return json!({"mode": "element_scroll", "selector": sel, "scrollTop": next_y, "max": max});
        }
        tab.scroll_x = (tab.scroll_x + dx * WHEEL_STEP).max(0.0);
        tab.scroll_y = (tab.scroll_y - dy * WHEEL_STEP).max(0.0);
        let (sx, sy) = (tab.scroll_x, tab.scroll_y);
        drop(inner);
        self.push_frame_if_headed(pid).await;
        json!({"mode": "viewport_scroll", "scrollX": sx, "scrollY": sy})
    }
}
