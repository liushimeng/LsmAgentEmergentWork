//! datalist 联想输入(39 号设计文档 P2d)。
//!
//! `<input list="id">` + `<datalist id>` 的自动补全面板:聚焦展开全量、
//! 键入按当前值重过滤、`↓/↑` 移动高亮、`Enter` 应用高亮项、`Esc`/外部
//! 点击/light dismiss 关闭。语义对齐 Chrome:
//!
//! * 过滤规则 = option `value` 或 `label` **包含**查询串(大小写不敏感),
//!   空查询 = 全量;展示上限 [`DL_MAX_ROWS`] 行(超出截断)。
//! * 应用 = 写 input 值 + 派发 `input`/`change` + 关面板。
//! * 面板渲染在布局层 `top_layer::emit_datalist_panel`,状态经
//!   `apply_widget_state` 落成内核私有属性(`data-lsm-datalist-open`/
//!   `data-lsm-dl-hit`/`data-lsm-dl-idx`),与 select 展开面板同哲学。

use super::*;

/// 面板最多显示的命中行数(Chrome 约 10 行)。
pub(crate) const DL_MAX_ROWS: usize = 10;

/// 无高亮哨兵。
pub(crate) const NO_HL: usize = usize::MAX;

/// 过滤规则(纯函数,单测覆盖):value/label 包含查询串(大小写不敏感)
/// 即命中;空查询 = 全量;截断到 [`DL_MAX_ROWS`] 行。
pub(crate) fn filter_hits(
    options: &[(String, String, String)],
    query: &str,
) -> Vec<(String, String, String)> {
    let q = query.trim().to_lowercase();
    options
        .iter()
        .filter(|(_, v, l)| {
            q.is_empty() || v.to_lowercase().contains(&q) || l.to_lowercase().contains(&q)
        })
        .take(DL_MAX_ROWS)
        .cloned()
        .collect()
}

/// 读取 input 关联的 datalist 选项(DOM 序):(option 稳定选择器, value, label)。
/// 无 `list` 属性 / datalist 不存在 / 无 option → `None`。
pub(crate) fn datalist_options(
    doc: &Document,
    input_sel: &str,
) -> Option<Vec<(String, String, String)>> {
    let input = doc.select(input_sel).ok()?.into_iter().next()?;
    let list_id = document::attr(&input, "list").filter(|v| !v.trim().is_empty())?;
    let dl = doc
        .select(&format!("datalist#{list_id}"))
        .ok()?
        .into_iter()
        .next()?;
    let options: Vec<(String, String, String)> = dl
        .select("option")
        .ok()?
        .into_iter()
        .map(|o| {
            let text = document::collapse_ws(&o.as_node().text_contents());
            let value = document::attr(o.as_node(), "value").unwrap_or_else(|| text.clone());
            let label = document::attr(o.as_node(), "label").unwrap_or_default();
            (Document::stable_selector(o.as_node()), value, label)
        })
        .collect();
    if options.is_empty() {
        None
    } else {
        Some(options)
    }
}

impl LsmBrowserManager {
    /// 聚焦变化钩子:聚焦带 `list` 的 input → 开面板;焦点移出已开面板的
    /// input → 关面板(light dismiss 的键盘路径)。
    pub(super) async fn on_focus_maybe_datalist(&self, pid: &str, focused_sel: &str) {
        let open_for = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return;
            };
            let dl_open_for = tab.datalist.as_ref().map(|d| d.input_sel.clone());
            match dl_open_for {
                Some(sel) if sel == focused_sel => None, // 焦点仍在联想输入框
                Some(sel) => Some(sel),                   // 移出 → 待关闭
                None => None,
            }
        };
        if let Some(sel) = open_for {
            self.close_datalist(pid, &sel).await;
        }
        // 廉价预过滤:页面 HTML 里根本没有 datalist/list 属性就不做文档解析
        //(此钩子挂在每次点击聚焦/Tab 聚焦上,全量 document_with_state 的
        // 解析成本在无 datalist 的页面上是纯浪费)。
        let has_datalist_marker = {
            let inner = self.inner.lock().await;
            inner
                .tabs
                .get(pid)
                .map(|t| t.html.contains("<datalist") && t.html.contains("list="))
                .unwrap_or(false)
        };
        if !has_datalist_marker {
            return;
        }
        let doc_input = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return;
            };
            let doc = tab.document_with_state();
            datalist_options(&doc, focused_sel).map(|opts| {
                let cur = tab
                    .form_values
                    .get(focused_sel)
                    .cloned()
                    .or_else(|| {
                        doc.select(focused_sel)
                            .ok()
                            .and_then(|v| v.into_iter().next())
                            .and_then(|n| document::attr(&n, "value"))
                    })
                    .unwrap_or_default();
                (cur, opts)
            })
        };
        if let Some((cur, opts)) = doc_input {
            let hits = filter_hits(&opts, &cur);
            if !hits.is_empty() {
                {
                    let mut inner = self.inner.lock().await;
                    if let Some(tab) = inner.tabs.get_mut(pid) {
                        tab.datalist = Some(DatalistState {
                            input_sel: focused_sel.to_string(),
                            hits,
                            highlight: NO_HL,
                        });
                    }
                }
                api::relayout_and_push(self, pid).await;
                self.note_human_event(pid, "datalist 联想面板展开");
            }
        }
    }

    /// 键入后刷新:按当前值重过滤;命中清空 → 关面板;高亮复位。
    pub(super) async fn refresh_datalist(&self, pid: &str) {
        let input_sel = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return;
            };
            match &tab.datalist {
                Some(d) => d.input_sel.clone(),
                None => return,
            }
        };
        let next = {
            let inner = self.inner.lock().await;
            let Some(tab) = inner.tabs.get(pid) else {
                return;
            };
            let doc = tab.document_with_state();
            let opts = datalist_options(&doc, &input_sel);
            let cur = tab
                .form_values
                .get(&input_sel)
                .cloned()
                .or_else(|| {
                    doc.select(&input_sel)
                        .ok()
                        .and_then(|v| v.into_iter().next())
                        .and_then(|n| document::attr(&n, "value"))
                })
                .unwrap_or_default();
            opts.map(|o| filter_hits(&o, &cur))
        };
        let hits = next.unwrap_or_default();
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                if hits.is_empty() {
                    tab.datalist = None;
                } else if let Some(d) = tab.datalist.as_mut() {
                    d.hits = hits;
                    d.highlight = NO_HL;
                }
            }
        }
        api::relayout_and_push(self, pid).await;
    }

    /// 关闭面板(指定 input;状态不匹配时不动)。
    pub(crate) async fn close_datalist(&self, pid: &str, input_sel: &str) {
        let closed = {
            let mut inner = self.inner.lock().await;
            match inner.tabs.get_mut(pid) {
                Some(tab) => match &tab.datalist {
                    Some(d) if d.input_sel == input_sel => {
                        tab.datalist = None;
                        true
                    }
                    _ => false,
                },
                None => false,
            }
        };
        if closed {
            api::relayout_and_push(self, pid).await;
        }
    }

    /// 联想面板键盘路由:`Some` = 已消费(调用方跳过默认键语义)。
    /// `Enter` 无高亮时关面板并返回 `None`(默认提交语义继续)。
    pub(super) async fn datalist_key(
        &self,
        pid: &str,
        key: &str,
        mods: &Mods,
    ) -> Option<Value> {
        if mods.combo() {
            return None;
        }
        let input_sel = {
            let inner = self.inner.lock().await;
            inner.tabs.get(pid)?.datalist.as_ref()?.input_sel.clone()
        };
        match key {
            "ArrowDown" | "ArrowUp" => {
                let moved = {
                    let mut inner = self.inner.lock().await;
                    let d = inner.tabs.get_mut(pid)?.datalist.as_mut()?;
                    let len = d.hits.len();
                    let dir = if key == "ArrowDown" { 1i64 } else { -1 };
                    let cur = if d.highlight == NO_HL {
                        if dir < 0 { len as i64 - 1 } else { 0 }
                    } else {
                        d.highlight as i64 + dir
                    };
                    // 循环移动(NO_HL 与首尾互绕)。
                    let next = if cur < 0 {
                        NO_HL
                    } else if cur >= len as i64 {
                        NO_HL
                    } else {
                        cur as usize
                    };
                    if next == d.highlight {
                        false
                    } else {
                        d.highlight = next;
                        true
                    }
                };
                if moved {
                    api::relayout_and_push(self, pid).await;
                }
                Some(json!({"key": key, "datalist": "highlight"}))
            }
            "Enter" => {
                let hit = {
                    let inner = self.inner.lock().await;
                    let d = inner.tabs.get(pid)?.datalist.as_ref()?;
                    (d.highlight != NO_HL).then(|| d.hits[d.highlight.min(d.hits.len() - 1)].clone())
                };
                if let Some((_sel, value, _label)) = hit {
                    let out = self.apply_datalist_value(pid, &input_sel, &value).await;
                    Some(out)
                } else {
                    // 无高亮:关面板,Enter 的默认语义(提交等)继续。
                    self.close_datalist(pid, &input_sel).await;
                    None
                }
            }
            "Escape" => {
                self.close_datalist(pid, &input_sel).await;
                Some(json!({"key": key, "datalist": "closed"}))
            }
            _ => None,
        }
    }

    /// 应用命中值:写 input 值 + `input`/`change` 事件 + 关面板。
    /// 点击面板项与 `Enter` 应用共用。
    pub(crate) async fn apply_datalist_value(
        &self,
        pid: &str,
        input_sel: &str,
        value: &str,
    ) -> Value {
        self.set_runtime_value(pid, input_sel, value).await;
        // 值变化后清光标到尾部(编辑语义与键入一致)。
        {
            let mut inner = self.inner.lock().await;
            if let Some(tab) = inner.tabs.get_mut(pid) {
                tab.form_caret.insert(
                    input_sel.to_string(),
                    CaretState { pos: value.chars().count(), anchor: value.chars().count() },
                );
            }
        }
        for ev in ["input", "change"] {
            if let Some(reply) = api::js_dispatch_bounded(self, pid, input_sel, ev).await {
                api::apply_js_reply(self, pid, &reply).await;
            }
        }
        self.close_datalist(pid, input_sel).await;
        self.note_human_event(pid, "datalist 联想应用");
        json!({"datalist_applied": true, "selector": input_sel, "value": value})
    }
}
