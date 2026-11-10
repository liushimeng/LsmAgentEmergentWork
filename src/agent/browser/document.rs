//! DOM 封装:解析、选择器、稳定选择器、文本抽取、表单序列化。
//!
//! `Document` 只在单次同步操作内存在;标签页保存的是 HTML 字符串与纯数据缓存。
//! 这样 `TabState` 保持 `Send`,可以安全放进 `tokio::sync::Mutex`。

use std::collections::HashMap;

use kuchikiki::traits::*;
use kuchikiki::{ElementData, NodeDataRef, NodeRef};
use serde_json::{json, Value};

use super::style::{ComputedStyle, StyleEngine};
use super::urlutil;
use super::DatalistState;

/// 块级标签:文本抽取时插入换行。
const BLOCK_TAGS: &[&str] = &[
    "address", "article", "aside", "blockquote", "body", "dd", "div", "dl", "dt", "fieldset",
    "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr",
    "li", "main", "nav", "ol", "p", "pre", "section", "table", "tbody", "td", "tfoot", "th",
    "thead", "tr", "ul", "br", "option", "title",
];

const SKIP_TEXT_TAGS: &[&str] = &["script", "style", "noscript", "template", "svg"];

/// 不进快照的标签(纯元数据,子树无内容)。
const SNAPSHOT_EXCLUDE_TAGS: &[&str] = &[
    "title", "meta", "link", "base", "noscript", "template",
];

/// 进快照但子树不展开、文本置空的标签。
const SNAPSHOT_LEAF_TAGS: &[&str] = &["script", "style", "svg"];

// ========== 第 41 轮:运行时伪类状态 ==========

/// 运行时伪类状态标记属性名(内核私有,只写临时解析树,不回写源 HTML)。
///
/// 状态经 `apply_widget_state` 落到临时解析树,样式引擎的
/// `:hover`/`:focus`/`:focus-visible`/`:active` 再按属性查表命中 ——
/// 与 `:checked`/`:disabled` 的「运行时覆盖 → 属性 → 样式」通路完全同构。
pub const ATTR_PSEUDO_HOVER: &str = "data-lsm-hover";
pub const ATTR_PSEUDO_FOCUS: &str = "data-lsm-focus";
pub const ATTR_PSEUDO_FOCUS_VISIBLE: &str = "data-lsm-focus-visible";
pub const ATTR_PSEUDO_ACTIVE: &str = "data-lsm-active";

/// `select` 展开态标记(内核私有):布局层据此铺出浮层 option 面板。
pub const ATTR_SELECT_OPEN: &str = "data-lsm-select-open";
/// P2d:datalist 联想面板展开标记(落在 input 上,内核私有)。
pub const ATTR_DATALIST_OPEN: &str = "data-lsm-datalist-open";
/// P2d:datalist 命中项标记(落在 datalist 内匹配 option 上,内核私有)。
pub const ATTR_DL_HIT: &str = "data-lsm-dl-hit";
/// P2d:datalist 高亮行下标(落在 input 上,值为行号或 usize::MAX,内核私有)。
pub const ATTR_DL_IDX: &str = "data-lsm-dl-idx";
/// 第 46 轮:元素级滚动偏移的内核私有标记(落到临时解析树,不入页面 HTML)。
pub const ATTR_SCROLL_TOP: &str = "data-lsm-scroll-top";
/// 39 号 P2a:文本光标位置(char 索引)的内核私有标记,布局层据此画光标条。
pub const ATTR_CARET: &str = "data-lsm-caret";
/// 39 号 P2a:选区锚点(char 索引;与光标不等即有选区),布局层画选区高亮。
pub const ATTR_SEL: &str = "data-lsm-sel";
/// 39 号 P2b:页面级自由选区的首元素区间(值为 `start,end`),
/// 布局层据此画跨元素选区高亮(近似:只画首尾元素内区段)。
pub const ATTR_PAGE_SEL: &str = "data-lsm-page-sel";

/// inspect(elements) 与交互候选集的默认选择器唯一真源。
///
/// 第 42 轮纳入 option/summary/label/tabindex/contenteditable:展开面板里的
/// option、details 开关、表单标签与自定义可编辑控件此前完全不出现在默认发现集。
pub const INTERACTIVE_DEFAULT: &str = "a[href], button, input, textarea, select, option, summary, label, [role=\"button\"], [role=\"link\"], [role=\"checkbox\"], [role=\"radio\"], [role=\"tab\"], [role=\"menuitem\"], [role=\"combobox\"], [role=\"slider\"], [onclick], [tabindex], [contenteditable], [data-title], [aria-label]";

/// 运行时伪类状态位(按元素聚合)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PseudoFlags {
    /// 指针悬停。
    pub hover: bool,
    /// 获得焦点。
    pub focus: bool,
    /// 键盘可见焦点(本轮与 `focus` 同步置位,内核暂无键盘导航)。
    pub focus_visible: bool,
    /// 按下(点击)瞬间。
    pub active: bool,
}

impl PseudoFlags {
    /// 是否带任何状态位。
    pub fn any(&self) -> bool {
        self.hover || self.focus || self.focus_visible || self.active
    }
}

/// 文本控件的运行时光标 + 选区锚点(39 号 P2a,与 `PseudoFlags` 同族的
/// 运行时覆盖层)。索引为 **char 索引**(0..=len,中文/emoji 不裂)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaretState {
    /// 光标(选区头)位置。
    pub pos: usize,
    /// 选区锚点(Shift 点击/扩选的固定端);与 `pos` 相等 = 无选区。
    pub anchor: usize,
}

impl CaretState {
    /// 折叠光标(无选区)。
    pub fn collapsed(pos: usize) -> Self {
        Self { pos, anchor: pos }
    }

    /// 是否无选区。
    pub fn is_empty(&self) -> bool {
        self.pos == self.anchor
    }

    /// 选区区间(升序 `(start, end)`)。
    pub fn range(&self) -> (usize, usize) {
        if self.pos <= self.anchor {
            (self.pos, self.anchor)
        } else {
            (self.anchor, self.pos)
        }
    }
}

/// 页面级自由选区(39 号 P2b):跨元素拖选,**不属于任何单一控件**。
///
/// 与 [`CaretState`] 是**两本账**:`CaretState` 的字符索引空间是控件的
/// `value`(单值字符串);自由选区跨元素,索引必须带**元素身份**(稳定选择器),
/// 否则 DOM 一变就错位。`getSelection().toString()` 优先返回本选区的文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSelection {
    /// 锚点元素(P2b 拖选手势的起点)。
    pub anchor_sel: String,
    /// 锚点元素内的 char 索引(基于该元素**可见文本**)。
    pub anchor_idx: usize,
    /// 活动端元素(随指针移动)。
    pub head_sel: String,
    /// 活动端元素内的 char 索引。
    pub head_idx: usize,
}

impl PageSelection {
    /// 归一化:DOM 序靠前的端作 `start`。同元素时按索引升序。
    /// 返回 `(start_sel, start_idx, end_sel, end_idx)`。
    pub fn ordered(&self, doc: &Document) -> (String, usize, String, usize) {
        let a = doc
            .select(&self.anchor_sel)
            .ok()
            .and_then(|v| v.into_iter().next());
        let h = doc
            .select(&self.head_sel)
            .ok()
            .and_then(|v| v.into_iter().next());
        let a_pos = a.as_ref().map(node_doc_order).unwrap_or(usize::MAX);
        let h_pos = h.as_ref().map(node_doc_order).unwrap_or(usize::MAX);
        if a_pos < h_pos {
            (self.anchor_sel.clone(), self.anchor_idx, self.head_sel.clone(), self.head_idx)
        } else if a_pos > h_pos {
            (self.head_sel.clone(), self.head_idx, self.anchor_sel.clone(), self.anchor_idx)
        } else if self.anchor_idx <= self.head_idx {
            (self.anchor_sel.clone(), self.anchor_idx, self.head_sel.clone(), self.head_idx)
        } else {
            (self.head_sel.clone(), self.head_idx, self.anchor_sel.clone(), self.anchor_idx)
        }
    }

    /// 选区是否为空(两端同元素同索引)。
    pub fn is_empty(&self) -> bool {
        self.anchor_sel == self.head_sel && self.anchor_idx == self.head_idx
    }

    /// 选区文本:按**元素可见文本**(`node_text`,与 `page_text_caret_at`
    /// 的索引空间同源)拼接两端元素之间的内容后截取 `[start, end)`。
    ///
    /// **近似**(方案 §7.2):只取「无元素子代」的叶子元素(父元素文本已含子代,
    /// 重复计数会让索引整体错位);跨元素整片高亮未建模,文本仍按两端之间的
    /// 全部文本计算 —— 与渲染的首尾近似可能不完全重合,如实告知调用方。
    pub fn text_in(&self, doc: &Document) -> String {
        let (start_sel, start_idx, end_sel, end_idx) = self.ordered(doc);
        // 逐元素收集(保序),并记录每段在拼接串里的起点偏移 ——
        // start_idx/end_idx 是**元素内**索引,必须换算成拼接串的绝对偏移。
        let mut parts: Vec<String> = Vec::new();
        let mut in_range = false;
        collect_page_sel_text(doc.root(), &start_sel, &end_sel, &mut in_range, &mut parts);
        // 拼接并记录首/末段的起点偏移(元素内索引 → 拼接串绝对偏移)。
        let mut offsets: Vec<usize> = Vec::with_capacity(parts.len());
        let mut joined = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                joined.push('\n');
            }
            // 记录**段起点**(分隔符之后),元素内索引才能正确换算。
            offsets.push(joined.chars().count());
            joined.push_str(p);
        }
        let chars: Vec<char> = joined.chars().collect();
        let base_start = offsets.first().copied().unwrap_or(0);
        let base_end = offsets.last().copied().unwrap_or(0);
        let s = (base_start + start_idx).min(chars.len());
        let e = (base_end + end_idx).min(chars.len());
        if s >= e {
            return String::new();
        }
        chars[s..e].iter().collect()
    }
}

/// `text_in` 的收集器:先序遍历,收集 start..=end 之间**叶子元素**的文本。
///
/// 只取叶子(无元素子代)是必须的:父元素的 `node_text` 已含子代文本,
/// 两者都收会重复计数,字符索引整体错位。
///
/// 返回 `true` = 已越过收尾元素,调用方应停止整棵树的遍历(剪枝)。
fn collect_page_sel_text(
    node: &NodeRef,
    start_sel: &str,
    end_sel: &str,
    in_range: &mut bool,
    parts: &mut Vec<String>,
) -> bool {
    if node.as_element().is_some() {
        // 起点判定独立于 in_range:同一元素既是起点又是终点时(同元素选区)
        // 两个分支都要走到。
        if !*in_range && Document::stable_selector(node) == start_sel {
            *in_range = true;
        }
        if *in_range {
            let has_el_child = node.children().any(|c| c.as_element().is_some());
            if !has_el_child {
                let t = node_text(node);
                if !t.is_empty() {
                    parts.push(t);
                }
            }
        }
        // 收尾:收集完即剪枝(同元素时上面已收,这里直接返回)。
        if *in_range && Document::stable_selector(node) == end_sel {
            return true;
        }
    }
    for c in node.children() {
        if collect_page_sel_text(&c, start_sel, end_sel, in_range, parts) {
            return true;
        }
    }
    false
}

/// P2e:`collect_page_sel_middle` 的遍历状态(先序 = 文档序)。
enum PageSelMidState {
    /// 尚未遇到起始元素。
    Before,
    /// 已过起始、未到终点(中间候选区间)。
    Middle,
    /// 已到终点(剪枝)。
    Done,
}

/// P2e:收集起止元素**之间**(不含两端)的叶子文本元素选择器(文档序)。
///
/// 与 [`collect_page_sel_text`] 同一套「叶子才计数」口径(父元素的
/// `node_text` 已含子代,重复标记会让高亮越界;script/style/head 已被
/// `node_text` 排除)。这些元素将整段点亮(`data-lsm-page-sel = "0,-1"`)。
fn collect_page_sel_middle(
    node: &NodeRef,
    start_sel: &str,
    end_sel: &str,
    state: &mut PageSelMidState,
    out: &mut Vec<String>,
) -> bool {
    if node.as_element().is_some() {
        let sel_here = Document::stable_selector(node);
        match state {
            PageSelMidState::Before => {
                if sel_here == *start_sel {
                    *state = if sel_here == *end_sel {
                        PageSelMidState::Done
                    } else {
                        PageSelMidState::Middle
                    };
                }
            }
            PageSelMidState::Middle => {
                if sel_here == *end_sel {
                    *state = PageSelMidState::Done;
                    return true;
                }
                let has_el_child = node.children().any(|c| c.as_element().is_some());
                if !has_el_child && !node_text(node).is_empty() {
                    out.push(sel_here);
                }
            }
            PageSelMidState::Done => return true,
        }
    }
    for c in node.children() {
        if collect_page_sel_middle(&c, start_sel, end_sel, state, out) {
            return true;
        }
    }
    false
}

/// 节点的文档序(先序遍历序号):`PageSelection::ordered` 的比较依据。
/// 取不到时返回 `usize::MAX`(排到最后,不 panic)。
fn node_doc_order(node: &NodeRef) -> usize {
    let mut idx = 0usize;
    let mut cur = node.clone();
    // 先上溯到根,再自上而下数先序序号 —— 实现简单、对选区排序足够。
    let mut depth = 0usize;
    let mut p = cur.parent();
    while let Some(parent) = p {
        depth += 1;
        if depth > 4096 {
            return usize::MAX;
        }
        cur = parent.clone();
        p = cur.parent();
    }
    // 自根先序遍历,命中即返回计数。
    fn walk(n: &NodeRef, target: &NodeRef, counter: &mut usize) -> Option<usize> {
        if n == target {
            return Some(*counter);
        }
        if n.as_element().is_some() {
            *counter += 1;
        }
        for c in n.children() {
            if let Some(hit) = walk(&c, target, counter) {
                return Some(hit);
            }
        }
        None
    }
    walk(&cur, node, &mut idx).unwrap_or(usize::MAX)
}

/// 解析后的文档(局部值,不跨 await 保存)。
pub struct Document {
    root: NodeRef,
    pub base_url: String,
    pub html: String,
}

impl Document {
    pub fn parse(html: &str, base_url: &str) -> Self {
        let root = kuchikiki::parse_html().one(html);
        Self {
            root,
            base_url: base_url.to_string(),
            html: html.to_string(),
        }
    }

    pub fn root(&self) -> &NodeRef {
        &self.root
    }

    /// 把表单运行时值/details-dialog 开合/交互态覆盖落到解析树，供样式、布局与快照
    /// 共享同一份状态。只修改临时 `Document`，不回写 `tab.html`。
    ///
    /// 覆盖规则：`form_values` 的 stable selector 值优先于 HTML 属性；空值表示
    /// 清空（input/textarea 退回 placeholder）。`widget_open` 显式 `false` 可关闭
    /// 源 HTML 中带 `open` 的 details/dialog。`pseudo_states` 写入 `data-lsm-*`
    /// 状态属性，供 `:hover`/`:focus`/`:focus-visible`/`:active` 匹配。
    pub fn apply_widget_state(
        &mut self,
        form_values: &HashMap<String, String>,
        widget_open: &HashMap<String, bool>,
        pseudo_states: &HashMap<String, PseudoFlags>,
        datalist: Option<&DatalistState>,
    ) {
        for node in self.select("*").unwrap_or_default() {
            let tag = tag_name(&node);
            let stable = Document::stable_selector(&node);
            // 第 41 轮:交互态状态位 → `data-lsm-*` 属性(有则置、无则撤)。
            if let Some(flags) = pseudo_states.get(&stable) {
                let mut attrs = node.as_element().unwrap().attributes.borrow_mut();
                set_state_attr(&mut attrs, ATTR_PSEUDO_HOVER, flags.hover);
                set_state_attr(&mut attrs, ATTR_PSEUDO_FOCUS, flags.focus);
                set_state_attr(&mut attrs, ATTR_PSEUDO_FOCUS_VISIBLE, flags.focus_visible);
                set_state_attr(&mut attrs, ATTR_PSEUDO_ACTIVE, flags.active);
            }
            // P2d:datalist 联想面板状态(开面板/命中项/高亮行)落内核私有属性。
            if let Some(dl) = datalist {
                if tag == "input" && stable == dl.input_sel {
                    let mut attrs = node.as_element().unwrap().attributes.borrow_mut();
                    attrs.insert(ATTR_DATALIST_OPEN.to_string(), String::new());
                    attrs.insert(
                        ATTR_DL_IDX.to_string(),
                        dl.highlight.to_string(),
                    );
                } else if tag == "option" && dl.hits.iter().any(|(s, _, _)| *s == stable) {
                    node.as_element()
                        .unwrap()
                        .attributes
                        .borrow_mut()
                        .insert(ATTR_DL_HIT.to_string(), String::new());
                }
            }
            if let Some(open) = widget_open.get(&stable) {
                if matches!(tag.as_str(), "details" | "dialog") {
                    if *open {
                        node.as_element()
                            .unwrap()
                            .attributes
                            .borrow_mut()
                            .insert("open", String::new());
                    } else {
                        node.as_element()
                            .unwrap()
                            .attributes
                            .borrow_mut()
                            .remove("open");
                    }
                } else if tag == "select" {
                    // 第 41 轮:select 展开态用内核私有标记承载(布局层据此
                    // 铺出浮层 option 面板);`widget_open` 语义在此复用为开合。
                    let mut attrs = node.as_element().unwrap().attributes.borrow_mut();
                    set_state_attr(&mut attrs, ATTR_SELECT_OPEN, *open);
                }
            }
            let Some(value) = form_values.get(&stable) else {
                continue;
            };
            match tag.as_str() {
                "input" => {
                    let typ = attr(&node, "type").unwrap_or_default().to_ascii_lowercase();
                    if matches!(typ.as_str(), "checkbox" | "radio") {
                        let mut attrs = node.as_element().unwrap().attributes.borrow_mut();
                        if value == "true" {
                            attrs.insert("checked", String::new());
                        } else {
                            attrs.remove("checked");
                        }
                    } else {
                        let mut attrs = node.as_element().unwrap().attributes.borrow_mut();
                        if value.is_empty() {
                            attrs.remove("value");
                        } else {
                            attrs.insert("value", value.clone());
                        }
                    }
                }
                "textarea" => {
                    let mut attrs = node.as_element().unwrap().attributes.borrow_mut();
                    if value.is_empty() {
                        attrs.remove("value");
                    } else {
                        attrs.insert("value", value.clone());
                    }
                    drop(attrs);
                    for child in node.children().collect::<Vec<_>>() {
                        child.detach();
                    }
                    if !value.is_empty() {
                        node.append(NodeRef::new_text(value));
                    }
                }
                "select" => {
                    let Ok(options) = node.select("option") else {
                        continue;
                    };
                    // P2d:select[multiple] 的运行时值是 CSV(逗号连接,DOM 序),
                    // 集合内全部 option 置 `selected`(不互斥);单选仍单值互斥。
                    let multi = attr(&node, "multiple").is_some();
                    let multi_set: std::collections::HashSet<&str> = if multi {
                        value.split(',').filter(|t| !t.is_empty()).collect()
                    } else {
                        std::collections::HashSet::new()
                    };
                    for opt in options {
                        let opt = opt.as_node().clone();
                        let opt_value = attr(&opt, "value")
                            .unwrap_or_else(|| collapse_ws(&opt.text_contents()));
                        let hit = if multi {
                            multi_set.contains(opt_value.as_str())
                        } else {
                            opt_value == *value
                        };
                        let mut attrs = opt.as_element().unwrap().attributes.borrow_mut();
                        if hit {
                            attrs.insert("selected", String::new());
                        } else {
                            attrs.remove("selected");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// 39 号 P2b:页面级自由选区 → 临时解析树的首/尾元素 `data-lsm-page-sel`
    /// 属性(值为 `start,end`)。布局层据此画跨元素选区高亮。
    ///
    /// **近似**:只标首尾两端元素(首元素画 `[start,len)`,尾元素画 `[0,end)`,
    /// 中间整元素不画)——跨元素整片高亮留后续阶段(见方案 §7.2)。
    pub fn apply_page_sel(&mut self, sel: &PageSelection) {
        let (start_sel, start_idx, end_sel, end_idx) = sel.ordered(self);
        let apply = |doc: &mut Document, selector: &str, range: (i64, i64)| {
            let Ok(nodes) = doc.select(selector) else {
                return;
            };
            let Some(node) = nodes.first() else { return };
            let Some(el) = node.as_element() else {
                return;
            };
            let mut attrs = el.attributes.borrow_mut();
            // range.1 < 0 = 到文本末尾(首元素形态);range.0 < 0 或
            // (两端都非负且 start >= end)才是空区间 → 撤标记。
            let empty = range.0 < 0 || (range.1 >= 0 && range.1 <= range.0);
            if empty {
                attrs.remove(ATTR_PAGE_SEL);
            } else {
                attrs.insert(ATTR_PAGE_SEL, format!("{},{}", range.0, range.1));
            }
        };
        if start_sel == end_sel {
            apply(self, &start_sel, (start_idx as i64, end_idx as i64));
            return;
        }
        // 首元素:从 start 到文本末尾(-1 = 到末尾,布局层按可见文本长度解释)。
        apply(self, &start_sel, (start_idx as i64, -1));
        // P2e:中间叶子元素整段点亮(39 号 §7.2 的渲染近似收敛:此前只画
        // 首尾两端,视觉上"选了三段只有头尾亮",与 `getSelection().toString()`
        // 的文本范围不一致)。`0,-1` = 全段,渲染层既有语义零改动。
        let mut middles: Vec<String> = Vec::new();
        let mut state = PageSelMidState::Before;
        collect_page_sel_middle(self.root(), &start_sel, &end_sel, &mut state, &mut middles);
        for m in middles {
            apply(self, &m, (0, -1));
        }
        // 尾元素:从文本开头到 end。
        apply(self, &end_sel, (0, end_idx as i64));
    }

    /// 第 46 轮:元素级滚动偏移 → 临时解析树的 `data-lsm-scroll-top` 属性。
    ///
    /// 布局层据此把可滚动容器的内容上移(内容高超出盒高时按偏移滚动),
    /// 与页面级 `scroll_y` 的口径一致(布局期就把偏移烘进坐标)。
    pub fn apply_scroll_state(&mut self, scroll_offsets: &HashMap<String, f64>) {
        for (sel, top) in scroll_offsets {
            let Ok(nodes) = self.select(sel) else {
                continue;
            };
            let Some(node) = nodes.first() else {
                continue;
            };
            let Some(el) = node.as_element() else {
                continue;
            };
            let mut attrs = el.attributes.borrow_mut();
            attrs.insert(ATTR_SCROLL_TOP, format!("{top}"));
        }
    }

    /// 39 号 P2a:文本光标/选区态 → 临时解析树的 `data-lsm-caret` /
    /// `data-lsm-sel` 属性。布局层(`layout/widgets.rs`)据此在控件内画
    /// 光标条与选区高亮;无选区时撤掉选区标记。
    pub fn apply_caret_state(&mut self, carets: &HashMap<String, CaretState>) {
        for (sel, st) in carets {
            let Ok(nodes) = self.select(sel) else {
                continue;
            };
            let Some(node) = nodes.first() else {
                continue;
            };
            let Some(el) = node.as_element() else {
                continue;
            };
            let mut attrs = el.attributes.borrow_mut();
            attrs.insert(ATTR_CARET, format!("{}", st.pos));
            if st.is_empty() {
                attrs.remove(ATTR_SEL);
            } else {
                attrs.insert(ATTR_SEL, format!("{}", st.anchor));
            }
        }
    }

    pub fn title(&self) -> String {
        self.root
            .select_first("title")
            .ok()
            .map(|n| collapse_ws(&n.text_contents()))
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
    }

    /// 选择器查询。非法选择器返回 `Err`。
    pub fn select(&self, selector: &str) -> Result<Vec<NodeRef>, String> {
        let sel = selector.trim();
        if sel.is_empty() {
            return Err("选择器为空".to_string());
        }
        let it = self
            .root
            .select(sel)
            .map_err(|_| format!("非法选择器:{sel}"))?;
        Ok(it.map(|n: NodeDataRef<ElementData>| n.as_node().clone()).collect())
    }

    pub fn first(&self, selector: &str) -> Result<Option<NodeRef>, String> {
        Ok(self.select(selector)?.into_iter().next())
    }

    /// 可见正文(块级换行、跳过脚本/样式)。
    pub fn text(&self) -> String {
        let body = self.root.select_first("body").ok().map(|n| n.as_node().clone());
        let start = body.unwrap_or_else(|| self.root.clone());
        let mut out = String::new();
        collect_text(&start, &mut out, 0);
        let mut lines: Vec<String> = Vec::new();
        for line in out.lines() {
            let line = collapse_ws(line);
            if !line.is_empty() {
                lines.push(line);
            }
            if lines.len() >= 20_000 {
                break;
            }
        }
        truncate_chars(&lines.join("\n"), 1_000_000)
    }

    pub fn outer_html(&self) -> String {
        outer_html(&self.root)
    }

    /// 全保真序列化(kuchikiki 内置 serializer,保留全部属性与子树)。
    ///
    /// 阶段二动态 DOM 回写专用:脚本注入/修改后的文档必须无损回到 `tab.html`,
    /// `outer_html` 只重建已知属性集,不满足该要求。
    pub fn serialize_full(&self) -> String {
        self.root.to_string()
    }

    pub fn body_html(&self) -> String {
        self.root
            .select_first("body")
            .ok()
            .map(|n| outer_html(n.as_node()))
            .unwrap_or_else(|| self.outer_html())
    }

    /// 稳定选择器:优先唯一 id,否则构造 `tag:nth-of-type(n)` 路径。
    pub fn stable_selector(node: &NodeRef) -> String {
        if let Some(id) = attr(node, "id") {
            if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return format!("#{id}");
            }
            if !id.is_empty() {
                return format!("[id=\"{}\"]", escape_attr(&id));
            }
        }
        let mut parts: Vec<String> = Vec::new();
        let mut cur = Some(node.clone());
        let mut depth = 0;
        while let Some(n) = cur {
            if depth > 32 {
                break;
            }
            let Some(el) = n.as_element() else {
                break;
            };
            let tag = el.name.local.to_string();
            if tag == "html" {
                parts.push("html".to_string());
                break;
            }
            let index = same_tag_index(&n, &tag);
            if index > 1 {
                parts.push(format!("{tag}:nth-of-type({index})"));
            } else {
                parts.push(tag);
            }
            cur = n.parent();
            depth += 1;
        }
        parts.reverse();
        parts.join(" > ")
    }

    /// 元素快照(供 inspect elements / JS 桥)。
    pub fn element_json(
        &self,
        node: &NodeRef,
        index: usize,
        count: usize,
        rect: Option<(f64, f64, f64, f64)>,
        values: &HashMap<String, String>,
        pseudo_states: &HashMap<String, PseudoFlags>,
        focused: Option<&str>,
    ) -> Value {
        let selector = Self::stable_selector(node);
        let tag = tag_name(node);
        // 可见文本而非 text_contents():后者把 <script>/<style> 源码算进来,
        // inspect elements 在 body 上会返回 4000 字符 JS 源码(实测百度地图)。
        // visible_text_of 的 SKIP_TEXT_TAGS 与快照层同一套排除规则。
        let text = truncate_chars(&collapse_ws(&visible_text_of(node)), 4000);
        let attrs = common_attrs(node);
        let value = self.runtime_value(node, &selector, &attrs, values);
        // 关联 label[for=id] 文案(在 Document 上下文查询)。
        let label_for = attrs.get("id").filter(|id| !id.is_empty()).and_then(|id| {
            let sel = format!("label[for=\"{}\"]", id.replace('"', ""));
            self.root
                .select(&sel)
                .ok()?
                .map(|n| n.as_node().clone())
                .map(|n| collapse_ws(&n.text_contents()))
                .find(|t| !t.is_empty())
        });
        let label = accessible_name(node, &attrs, label_for);
        let flags = pseudo_states.get(&selector).copied().unwrap_or_default();
        // 控件/交互状态的一等字段。调用方不应理解 `value == "true"` 这类
        // 字符串协议,也不应从内核私有 `data-lsm-*` 属性反推运行时状态。
        let state = json!({
            "checked": values
                .get(&selector)
                .map(|v| v == "true" || v == "on")
                .unwrap_or_else(|| attr(node, "checked").is_some()),
            "selected": attr(node, "selected").is_some(),
            "disabled": attr(node, "disabled").is_some(),
            "readonly": attr(node, "readonly").is_some(),
            "indeterminate": attr(node, "indeterminate").is_some(),
            "open": attr(node, "open").is_some(),
            "select_open": attr(node, ATTR_SELECT_OPEN).is_some(),
            "focused": focused == Some(selector.as_str()) || flags.focus,
            "hovered": flags.hover,
            "active": flags.active,
        });
        let (x, y, w, h) = rect.unwrap_or((0.0, 0.0, 0.0, 0.0));
        json!({
            "ok": true,
            "page_id": Value::Null,
            "tag": tag,
            "selector": selector,
            "id": attrs.get("id").cloned().unwrap_or_default(),
            "classes": attrs.get("class").cloned().unwrap_or_default()
                .split_whitespace().collect::<Vec<_>>(),
            "text": text,
            "label": label,
            "value": value,
            "state": state,
            "href": attr(node, "href").and_then(|h| urlutil::resolve(&self.base_url, &h)),
            "attributes": attrs,
            "visible": is_visible(node),
            "index": index,
            "count": count,
            "rect": {"x": x, "y": y, "width": w, "height": h, "visible": is_visible(node)},
        })
    }

    /// 当前运行时值:表单覆盖表优先,其次属性/文本。
    pub fn runtime_value(
        &self,
        node: &NodeRef,
        selector: &str,
        attrs: &HashMap<String, String>,
        values: &HashMap<String, String>,
    ) -> String {
        if let Some(v) = values.get(selector) {
            return v.clone();
        }
        attrs.get("value").cloned().unwrap_or_else(|| {
            let tag = tag_name(node);
            if tag == "textarea" {
                collapse_ws(&node.text_contents())
            } else if tag == "select" {
                selected_option(node).unwrap_or_default()
            } else {
                String::new()
            }
        })
    }

    /// 交互元素列表(默认 inspect(elements) 的候选集)。
    ///
    /// 除原生可交互标签外,纳入「无文本图标控件」:`[data-title]` / `[aria-label]`
    /// —— Web App 的按钮普遍是 `<div class=route-button data-title="路线">` +
    /// CSS 背景图,DOM 里既无文本也无 href/role,不纳入就完全不可发现
    /// (实测百度地图路线按钮)。
    pub fn interactive_nodes(&self) -> Vec<NodeRef> {
        self.select(INTERACTIVE_DEFAULT)
            .unwrap_or_default()
    }

    /// 按可见文本查找元素。
    pub fn find_by_text(&self, needle: &str) -> Option<NodeRef> {
        let needle = collapse_ws(needle);
        if needle.is_empty() {
            return None;
        }
        let candidates = self.root.select("a, button, input, select, option, label, div, span, p, li, h1, h2, h3, h4, h5, h6, td, th").ok()?;
        for n in candidates {
            let node = n.as_node();
            if !is_visible(node) {
                continue;
            }
            if collapse_ws(&node.text_contents()).contains(&needle) {
                return Some(node.clone());
            }
        }
        None
    }

    /// 表单请求:从元素向上找 `<form>` 并序列化全部字段。
    pub fn form_request(
        &self,
        node: &NodeRef,
        overrides: &HashMap<String, String>,
    ) -> Option<FormRequest> {
        let form = ancestor_tag(node, "form")?;
        let action = attr(&form, "action")
            .and_then(|a| urlutil::resolve(&self.base_url, &a))
            .unwrap_or_else(|| self.base_url.clone());
        let method = attr(&form, "method")
            .unwrap_or_else(|| "GET".to_string())
            .to_ascii_uppercase();
        let mut fields: Vec<(String, String)> = Vec::new();
        for item in form.select("input, textarea, select").ok()? {
            let n = item.as_node();
            let name = attr(n, "name").unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            if attr(n, "disabled").is_some() {
                continue;
            }
            let tag = tag_name(n);
            let selector = Self::stable_selector(n);
            let typ = attr(n, "type").unwrap_or_default().to_ascii_lowercase();
            if tag == "input" && matches!(typ.as_str(), "submit" | "button" | "reset" | "image") {
                continue;
            }
            if tag == "input" && matches!(typ.as_str(), "checkbox" | "radio") {
                let checked = overrides
                    .get(&selector)
                    .map(|v| v == "true" || v == "on")
                    .unwrap_or_else(|| attr(n, "checked").is_some());
                if checked {
                    fields.push((
                        name,
                        overrides
                            .get(&selector)
                            .cloned()
                            .filter(|v| v != "true")
                            .or_else(|| attr(n, "value"))
                            .unwrap_or_else(|| "on".to_string()),
                    ));
                }
                continue;
            }
            // P2d:select[multiple] 序列化 = 每个选中值一对(Chrome
            // URLSearchParams 语义);运行时值是 CSV,或按 `selected` 属性遍历。
            if tag == "select" && attr(n, "multiple").is_some() {
                let values: Vec<String> = overrides
                    .get(&selector)
                    .map(|v| {
                        v.split(',')
                            .filter(|t| !t.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        n.select("option")
                            .map(|os| {
                                os.into_iter()
                                    .filter(|o| attr(o.as_node(), "selected").is_some())
                                    .map(|o| {
                                        let text = collapse_ws(&o.as_node().text_contents());
                                        attr(o.as_node(), "value").unwrap_or(text)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                for v in values {
                    fields.push((name.clone(), v));
                }
                continue;
            }
            let value = overrides
                .get(&selector)
                .cloned()
                .or_else(|| {
                    if tag == "select" {
                        selected_option(n)
                    } else if tag == "textarea" {
                        Some(collapse_ws(&n.text_contents()))
                    } else {
                        attr(n, "value")
                    }
                })
                .unwrap_or_default();
            fields.push((name, value));
        }
        Some(FormRequest {
            action,
            method,
            fields,
        })
    }

    pub fn link_json(&self, node: &NodeRef) -> Option<Value> {
        let href_raw = attr(node, "href")?;
        let href = urlutil::resolve(&self.base_url, &href_raw)?;
        let text = truncate_chars(&collapse_ws(&node.text_contents()), 300);
        let context = node
            .parent()
            .map(|p| truncate_chars(&collapse_ws(&p.text_contents()), 200))
            .unwrap_or_default();
        Some(json!({
            "href": href,
            "text": text,
            "context": context,
            "is_external": !urlutil::same_origin(&self.base_url, &href),
        }))
    }

    pub fn image_urls(&self) -> Vec<String> {
        self.select("img[src]")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|n| attr(&n, "src"))
            .filter_map(|s| urlutil::resolve(&self.base_url, &s))
            .collect()
    }

    /// 遍历 DOM 生成 JS 桥使用的元素快照。
    ///
    /// 阶段二起改为文档序(先根后子)递归遍历,并记录 `parent` 索引供 JS 侧事件冒泡使用。
    pub fn snapshot_nodes(&self) -> Vec<SnapshotNode> {
        self.snapshot_nodes_inner(None, None, None)
    }

    /// 带计算样式的快照:每个节点附 `css`(供 JS `getComputedStyle`)。
    pub fn snapshot_nodes_styled(&self, viewport: (u32, u32)) -> Vec<SnapshotNode> {
        let media = super::style::MediaCtx::new(viewport.0 as f64, viewport.1 as f64);
        let engine = StyleEngine::from_document(self, media);
        self.snapshot_nodes_inner(Some(&engine), None, None)
    }

    /// 带布局矩形的样式快照:rects 由 `layout::Layout::rect_map()` 生成,
    /// 按稳定选择器匹配注入,供 JS 桥暴露 clientWidth/offsetWidth 等几何属性。
    /// `anim_ov`(第 57 轮):动画覆盖快照,`getComputedStyle` 与布局同源。
    pub fn snapshot_nodes_with_rects(
        &self,
        viewport: (u32, u32),
        rects: &HashMap<String, (f64, f64, f64, f64)>,
        anim_ov: &super::anim::AnimOverrides,
    ) -> Vec<SnapshotNode> {
        let media = super::style::MediaCtx::new(viewport.0 as f64, viewport.1 as f64);
        let engine = StyleEngine::from_document(self, media);
        self.snapshot_nodes_inner(Some(&engine), Some(rects), Some(anim_ov))
    }

    fn snapshot_nodes_inner(
        &self,
        engine: Option<&StyleEngine>,
        rects: Option<&HashMap<String, (f64, f64, f64, f64)>>,
        anim_ov: Option<&super::anim::AnimOverrides>,
    ) -> Vec<SnapshotNode> {
        let mut out = Vec::new();
        // 起点用 `html` 而非 `body`:`document.documentElement` / `document.head`
        // 必须是真实节点,站点脚本常直接往 head 里插脚本/样式,拿不到就抛异常。
        let start = self
            .root
            .select_first("html")
            .ok()
            .map(|n| n.as_node().clone())
            .unwrap_or_else(|| self.root.clone());
        // 根样式:优先应用 `<html>` 的计算样式,保证 `html{font-size:...}` 等继承生效。
        let root_style = match engine {
            Some(e) => self
                .first("html")
                .ok()
                .flatten()
                .map(|n| e.computed(&n, &e.initial_style()))
                .unwrap_or_else(|| e.initial_style()),
            None => ComputedStyle::root(),
        };
        snapshot_walk(
            &start,
            usize::MAX,
            0,
            &self.base_url,
            &mut out,
            engine,
            &root_style,
            rects,
            anim_ov,
        );
        out
    }
}

#[allow(clippy::too_many_arguments)]
fn snapshot_walk(
    node: &NodeRef,
    parent: usize,
    depth: usize,
    base_url: &str,
    out: &mut Vec<SnapshotNode>,
    engine: Option<&StyleEngine>,
    parent_style: &ComputedStyle,
    rects: Option<&HashMap<String, (f64, f64, f64, f64)>>,
    anim_ov: Option<&super::anim::AnimOverrides>,
) {
    if depth > 40 || out.len() >= 5000 {
        return;
    }
    let mut child_parent = parent;
    let mut child_style = parent_style.clone();
    // 入快照但子树不展开、文本置空的标签(script/style/svg):
    // 站点脚本要用 `document.getElementsByTagName("script")[0]` 拿到锚点,
    // 若 script 根本不在快照里,拿到的就是 undefined,脚本当场抛异常。
    let mut leaf = false;
    if let Some(el) = node.as_element() {
        let tag = el.name.local.to_string();
        if engine.is_some() {
            child_style = engine
                .map(|e| e.computed(node, parent_style))
                .unwrap_or_else(|| parent_style.clone());
        }
        leaf = SNAPSHOT_LEAF_TAGS.contains(&tag.as_str());
        if !SNAPSHOT_EXCLUDE_TAGS.contains(&tag.as_str()) {
            let attrs = common_attrs(node);
            let idx = out.len();
            child_parent = idx;
            let selector = Document::stable_selector(node);
            // 第 57 轮:动画值覆盖(与布局同一时刻口径),JS getComputedStyle 同源。
            if let Some(ov) = anim_ov {
                super::anim::apply_overrides_map(ov, &selector, &mut child_style);
            }
            // 39 号 P2b:光标/选区 → 规范字段 `selStart/selEnd`(页面脚本读
            // `el.selectionStart/End`)。属性由 `apply_caret_state` 落。
            let sel_start = attr(node, ATTR_CARET).and_then(|v| v.trim().parse::<usize>().ok());
            let sel_end = sel_start.map(|pos| {
                attr(node, ATTR_SEL)
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(pos)
            });
            // P2e:select 读值桥 —— 运行时选中值经 `apply_widget_state` 已物化
            // 成 option 的 `selected` 属性,这里按「首个 selected,缺省回落
            // 首项」取值(与 `selected_option` 同口径),页面脚本
            // `select.value` 从此读到真实选中值而非恒空串(56 号文档缺口)。
            let (select_value, select_index, option_selected) = if tag == "select" {
                let opts: Vec<NodeRef> = node
                    .select("option")
                    .map(|os| os.into_iter().map(|o| o.as_node().clone()).collect())
                    .unwrap_or_default();
                let chosen = opts
                    .iter()
                    .enumerate()
                    .find(|(_, o)| attr(o, "selected").is_some())
                    .map(|(i, o)| (i as i64, o.clone()))
                    .or_else(|| opts.first().map(|o| (0i64, o.clone())));
                match chosen {
                    Some((i, o)) => (
                        Some(attr(&o, "value").unwrap_or_else(|| collapse_ws(&o.text_contents()))),
                        Some(i),
                        false,
                    ),
                    None => (Some(String::new()), Some(-1), false),
                }
            } else if tag == "option" {
                (None, None, attr(node, "selected").is_some())
            } else {
                (None, None, false)
            };
            out.push(SnapshotNode {
                index: idx,
                parent,
                rect: rects.and_then(|m| m.get(&selector).copied()),
                selector,
                tag: tag.clone(),
                id: attrs.get("id").cloned().unwrap_or_default(),
                class: attrs.get("class").cloned().unwrap_or_default(),
                name: attrs.get("name").cloned().unwrap_or_default(),
                typ: attrs.get("type").cloned().unwrap_or_default(),
                value: if tag == "select" {
                    select_value.unwrap_or_default()
                } else {
                    attrs.get("value").cloned().unwrap_or_default()
                },
                href: attr(node, "href")
                    .and_then(|h| urlutil::resolve(base_url, &h))
                    .unwrap_or_default(),
                checked: attr(node, "checked").is_some(),
                selected: option_selected,
                selected_index: select_index,
                sel_start,
                sel_end,
                // 可见文本:`text_contents()` 会把 `<script>` 源码算进来,
                // 导致 `body.innerText` 变成一整段 JS(实测百度地图即如此)。
                text: if leaf {
                    String::new()
                } else {
                    truncate_chars(&collapse_ws(&visible_text_of(node)), 4000)
                },
                visible: is_visible(node),
                attrs,
                css: if engine.is_some() {
                    child_style.to_map().into_iter().collect()
                } else {
                    HashMap::new()
                },
            });
        }
    }
    if !leaf {
        for child in node.children() {
            snapshot_walk(
                &child,
                child_parent,
                depth + 1,
                base_url,
                out,
                engine,
                &child_style,
                rects,
                anim_ov,
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotNode {
    pub index: usize,
    /// 父元素在快照数组中的索引;`usize::MAX` 表示无父(根)。
    pub parent: usize,
    pub selector: String,
    pub tag: String,
    pub id: String,
    pub class: String,
    pub name: String,
    pub typ: String,
    pub value: String,
    pub href: String,
    pub checked: bool,
    /// P2e:option 的选中态(读路径:页面脚本 `option.selected`、
    /// `select.selectedOptions` 都靠它)。
    pub selected: bool,
    /// P2e:select 的选中项序号(无 option = −1;仅 select 有值)。
    pub selected_index: Option<i64>,
    pub text: String,
    pub visible: bool,
    pub attrs: HashMap<String, String>,
    /// 计算样式(kebab-case);无样式引擎时为空。
    pub css: HashMap<String, String>,
    /// 布局矩形 `(x, y, w, h)`:JS 桥的 clientWidth/offsetWidth 等几何属性数据源。
    /// 快照时无布局(如纯样式快照)则为 None。
    pub rect: Option<(f64, f64, f64, f64)>,
    /// 文本控件的选区区间(升序 `(start, end)`,39 号 P2b)。
    /// 仅带 `data-lsm-caret` 的控件有值;其余为 `None`。
    pub sel_start: Option<usize>,
    pub sel_end: Option<usize>,
}

impl SnapshotNode {
    pub fn to_json(&self) -> Value {
        json!({
            "i": self.index,
            "parent": self.parent,
            "sel": self.selector,
            "tag": self.tag,
            "id": self.id,
            "class": self.class,
            "name": self.name,
            "type": self.typ,
            "value": self.value,
            "href": self.href,
            "checked": self.checked,
            // P2e:select 读值桥(option.selected / select.selectedIndex)。
            "selected": self.selected,
            "selectedIndex": self.selected_index,
            "text": self.text,
            "visible": self.visible,
            "attrs": self.attrs,
            "css": self.css,
            "rect": self.rect.map(|r| json!([r.0, r.1, r.2, r.3])),
            // 规范选区字段(39 号 P2b):页面脚本读 `el.selectionStart/End`。
            "selStart": self.sel_start,
            "selEnd": self.sel_end,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FormRequest {
    pub action: String,
    pub method: String,
    pub fields: Vec<(String, String)>,
}

impl FormRequest {
    pub fn encoded_fields(&self) -> String {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &self.fields {
            ser.append_pair(k, v);
        }
        ser.finish()
    }

    /// GET:把字段拼到 action URL;POST:返回表单编码请求体。
    pub fn to_request(&self) -> (String, Option<Vec<u8>>) {
        let encoded = self.encoded_fields();
        if self.method == "POST" {
            (self.action.clone(), Some(encoded.into_bytes()))
        } else {
            let sep = if self.action.contains('?') { '&' } else { '?' };
            (format!("{}{}{}", self.action, sep, encoded), None)
        }
    }
}

// ===================== 辅助 =====================

/// 置/撤运行时伪类状态标记属性。
fn set_state_attr(attrs: &mut kuchikiki::Attributes, name: &str, on: bool) {
    if on {
        attrs.insert(name.to_string(), String::new());
    } else {
        attrs.remove(name);
    }
}

pub fn tag_name(node: &NodeRef) -> String {
    node.as_element()
        .map(|e| e.name.local.to_string())
        .unwrap_or_default()
}

pub fn attr(node: &NodeRef, name: &str) -> Option<String> {
    node.as_element()
        .and_then(|e| e.attributes.borrow().get(name).map(str::to_string))
}

pub fn common_attrs(node: &NodeRef) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let names = [
        "id", "class", "name", "type", "value", "href", "src", "alt", "title", "placeholder",
        "role", "style", "checked", "selected", "disabled", "readonly", "aria-label",
        "aria-hidden", "data-testid", "data-id", "for", "action", "method", "rel", "target",
        // 替换元素固有尺寸:静态 Canvas/图片的 JS 包装同样需要真实 width/height。
        "width", "height",
        // 语义控件状态与表格跨距:快照/序列化必须保真,否则 JS 回写后
        // details/dialog/响应式图片/表格会在下一次重排时静默丢失关键属性。
        "open", "rowspan", "colspan", "srcset", "poster", "controls", "multiple", "size",
        "min", "max", "step", "list", "pattern", "required", "accept", "download",
        "datetime", "cite", "label", "contenteditable", "tabindex", "indeterminate",
    ];
    for n in names {
        if let Some(v) = attr(node, n) {
            out.insert(n.to_string(), v);
        }
    }
    // `data-*` / `aria-*` 全量保留:站点把可见文案大量放在 `data-title` 一类属性上
    // (百度地图的导航项就是 `data-title="路线"`),固定白名单会把它们整体丢掉,
    // Agent 于是"查不到任何按钮"。
    if let Some(el) = node.as_element() {
        for (k, v) in el.attributes.borrow().map.iter() {
            let key = k.local.to_string();
            if key.starts_with("data-") || key.starts_with("aria-") {
                out.insert(key, v.value.clone());
            }
        }
    }
    out
}

/// 可访问名(accessible name)推导:优先序 aria-label > data-title > title > alt >
/// placeholder > value > 关联 `<label for>`。Web App 普遍用「无文本图标按钮 +
/// data-title/CSS 背景图」,DOM 里没有文本节点,Agent 靠 `label` 才知道这是
/// 什么控件(实测百度地图路线按钮 `div.route-button[data-title=路线]`)。
pub fn accessible_name(
    node: &NodeRef,
    attrs: &HashMap<String, String>,
    label_for: Option<String>,
) -> String {
    for key in ["aria-label", "data-title", "title", "alt", "placeholder"] {
        if let Some(v) = attrs.get(key) {
            let v = collapse_ws(v);
            if !v.is_empty() {
                return v;
            }
        }
    }
    // 按钮型 input 的 value 即按钮文案。
    if tag_name(node) == "input" {
        if let Some(t) = attrs.get("type") {
            if matches!(t.as_str(), "button" | "submit" | "reset") {
                if let Some(v) = attrs.get("value") {
                    let v = collapse_ws(v);
                    if !v.is_empty() {
                        return v;
                    }
                }
            }
        }
    }
    if let Some(t) = label_for {
        if !t.is_empty() {
            return t;
        }
    }
    String::new()
}
///
/// kuchikiki 内部使用 markup5ever 0.11,与工程直接依赖的 html5ever 0.29 类型不兼容,
/// 因此不引入第二套序列化器;这里用 `common_attrs` 的已知属性集重建标签。
pub fn outer_html(node: &NodeRef) -> String {
    let mut out = String::new();
    serialize_node(node, &mut out, 0);
    out
}

fn serialize_node(node: &NodeRef, out: &mut String, depth: usize) {
    if depth > 80 || out.len() >= 2_000_000 {
        return;
    }
    match node.data() {
        kuchikiki::NodeData::Text(t) => escape_html_into(&t.borrow(), out),
        kuchikiki::NodeData::Comment(c) => {
            out.push_str("<!--");
            out.push_str(&c.borrow());
            out.push_str("-->");
        }
        kuchikiki::NodeData::Element(_) => {
            let tag = tag_name(node);
            let lower = tag.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "script" | "style" | "title" | "textarea"
            ) {
                out.push('<');
                out.push_str(&lower);
                out.push('>');
                for child in node.children() {
                    serialize_node(&child, out, depth + 1);
                }
                out.push_str("</");
                out.push_str(&lower);
                out.push('>');
                return;
            }
            out.push('<');
            out.push_str(&lower);
            let mut attrs: Vec<(String, String)> = common_attrs(node).into_iter().collect();
            attrs.sort_by(|a, b| a.0.cmp(&b.0));
            for (k, v) in attrs {
                out.push(' ');
                out.push_str(&k);
                out.push_str("=\"");
                escape_attr_into(&v, out);
                out.push('"');
            }
            if matches!(
                lower.as_str(),
                "br" | "img" | "input" | "meta" | "link" | "hr" | "source" | "area"
            ) {
                out.push('>');
                return;
            }
            out.push('>');
            for child in node.children() {
                serialize_node(&child, out, depth + 1);
            }
            out.push_str("</");
            out.push_str(&lower);
            out.push('>');
        }
        _ => {
            for child in node.children() {
                serialize_node(&child, out, depth + 1);
            }
        }
    }
}

fn escape_html_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

fn escape_attr_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
}

fn selected_option(select: &NodeRef) -> Option<String> {
    let options: Vec<NodeRef> = select.select("option").ok()?.map(|n| n.as_node().clone()).collect();
    let chosen = options
        .iter()
        .find(|n| attr(n, "selected").is_some())
        .or_else(|| options.first());
    chosen.map(|n| {
        attr(n, "value").unwrap_or_else(|| collapse_ws(&n.text_contents()))
    })
}

pub fn closest_tag(node: &NodeRef, tag: &str) -> Option<NodeRef> {
    ancestor_tag(node, tag)
}

fn ancestor_tag(node: &NodeRef, tag: &str) -> Option<NodeRef> {
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        if tag_name(&n) == tag {
            return Some(n);
        }
        cur = n.parent();
    }
    None
}

fn same_tag_index(node: &NodeRef, tag: &str) -> usize {
    let Some(parent) = node.parent() else {
        return 1;
    };
    let mut index = 0;
    for child in parent.children() {
        if tag_name(&child) == tag {
            index += 1;
            if child == *node {
                return index;
            }
        }
    }
    index.max(1)
}

/// 元素的可见文本(跳过 `<script>`/`<style>` 等非渲染内容)。
fn visible_text_of(node: &NodeRef) -> String {
    let mut out = String::new();
    collect_text(node, &mut out, 0);
    out
}

fn collect_text(node: &NodeRef, out: &mut String, depth: usize) {    if depth > 80 || out.len() >= 1_000_000 {
        return;
    }
    match node.data() {
        kuchikiki::NodeData::Text(t) => {
            let s = t.borrow();
            if !s.trim().is_empty() {
                out.push_str(&s);
            }
        }
        kuchikiki::NodeData::Element(_) => {
            let tag = tag_name(node);
            if SKIP_TEXT_TAGS.contains(&tag.as_str()) {
                return;
            }
            if tag == "head" {
                return;
            }
            let block = BLOCK_TAGS.contains(&tag.as_str());
            if block && !out.ends_with('\n') {
                out.push('\n');
            }
            for child in node.children() {
                collect_text(&child, out, depth + 1);
            }
            if block && !out.ends_with('\n') {
                out.push('\n');
            }
        }
        _ => {
            for child in node.children() {
                collect_text(&child, out, depth + 1);
            }
        }
    }
}

/// 元素级可见文本(块级换行 + 折叠空白):`wait` 的 `text` 条件与元素正文判定用。
///
/// 与 [`Document::text`] 的差别:只取该元素子树,不含 body 之外的内容;
/// `script`/`style`/`head` 等跳过(见 `collect_text`)。
pub fn node_text(node: &NodeRef) -> String {
    let mut out = String::new();
    collect_text(node, &mut out, 0);
    collapse_ws(&out)
}

/// `srcset` 候选的描述符。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SrcsetDescriptor {
    /// `w` 描述符:候选的固有宽度(px)。
    Width(f64),
    /// `x` 描述符:像素密度倍率(无描述符按 1x)。
    Density(f64),
}

/// 解析 `srcset` 属性:返回 `(URL, 描述符)` 候选列表(文档顺序)。
///
/// 语法:`url [描述符]` 列表,逗号分隔;描述符为 `w`(宽度)或 `x`(密度)。
/// URL 本身可含逗号(此时按「逗号后有描述符」切分,近似浏览器口径)。
pub fn parse_srcset(raw: &str) -> Vec<(String, SrcsetDescriptor)> {
    let mut out: Vec<(String, SrcsetDescriptor)> = Vec::new();
    for part in split_srcset(raw) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut it = part.split_whitespace();
        let Some(url) = it.next() else { continue };
        let desc = it.next().unwrap_or("1x");
        let d = if let Some(w) = desc.strip_suffix('w') {
            match w.trim().parse::<f64>() {
                Ok(v) if v > 0.0 => SrcsetDescriptor::Width(v),
                _ => SrcsetDescriptor::Density(1.0),
            }
        } else if let Some(x) = desc.strip_suffix('x') {
            match x.trim().parse::<f64>() {
                Ok(v) if v > 0.0 => SrcsetDescriptor::Density(v),
                _ => SrcsetDescriptor::Density(1.0),
            }
        } else {
            SrcsetDescriptor::Density(1.0)
        };
        out.push((url.to_string(), d));
    }
    out
}

/// `srcset` 候选切分(HTML 标准口径:URL 取到空白为止,描述符取到逗号为止)。
///
/// URL 内的逗号(如 `data:` URL)不会被误切:URL 只在遇到空白时才停。
fn split_srcset(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let n = chars.len();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < n {
        // 跳过候选之间的空白与逗号。
        while i < n && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        if i >= n {
            break;
        }
        // URL:取到空白为止。
        let start = i;
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        let mut url: String = chars[start..i].iter().collect();
        // URL 末尾的逗号 = 候选结束(无描述符,按 1x)。
        if url.ends_with(',') {
            url.pop();
            if !url.trim().is_empty() {
                out.push(url.trim().to_string());
            }
            continue;
        }
        // 描述符:取到逗号为止(可为空)。
        let dstart = i;
        while i < n && chars[i] != ',' {
            i += 1;
        }
        let desc: String = chars[dstart..i].iter().collect();
        if i < n {
            i += 1;
        }
        let piece = format!("{url} {desc}");
        let piece = piece.trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
    }
    out
}

/// 按视口宽与像素密度选出 `srcset` 候选(HTML 标准的近似口径)。
///
/// - 含 `w` 描述符:取「不小于源尺寸(视口宽 × DPR)的最小宽度候选」,
///   没有则取最大宽度候选;
/// - 只含 `x` 描述符:取「不小于 DPR 的最小密度候选」,没有则取最大密度候选。
pub fn select_srcset(
    candidates: &[(String, SrcsetDescriptor)],
    viewport_width: f64,
    dpr: f64,
) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    let target_w = (viewport_width.max(1.0) * dpr.max(0.1)).ceil();
    let has_w = candidates
        .iter()
        .any(|(_, d)| matches!(d, SrcsetDescriptor::Width(_)));
    if has_w {
        let mut best: Option<(f64, String)> = None;
        let mut fallback: Option<(f64, String)> = None;
        for (url, d) in candidates {
            let SrcsetDescriptor::Width(w) = d else { continue };
            let w = *w;
            if fallback.as_ref().map(|(fw, _)| w > *fw).unwrap_or(true) {
                fallback = Some((w, url.clone()));
            }
            if w >= target_w {
                if best.as_ref().map(|(bw, _)| w < *bw).unwrap_or(true) {
                    best = Some((w, url.clone()));
                }
            }
        }
        return best.or(fallback).map(|(_, u)| u);
    }
    let mut best: Option<(f64, String)> = None;
    let mut fallback: Option<(f64, String)> = None;
    for (url, d) in candidates {
        let SrcsetDescriptor::Density(x) = d else { continue };
        let x = *x;
        if fallback.as_ref().map(|(fx, _)| x > *fx).unwrap_or(true) {
            fallback = Some((x, url.clone()));
        }
        if x >= dpr {
            if best.as_ref().map(|(bx, _)| x < *bx).unwrap_or(true) {
                best = Some((x, url.clone()));
            }
        }
    }
    best.or(fallback).map(|(_, u)| u)
}

/// `<picture>` 内命中的 `<source>` 的 `srcset` 候选(按文档顺序取第一个匹配)。
///
/// `media` 只支持 `min-width`/`max-width` 的 px 口径;`type` 只放行 `image/*`。
pub fn picture_srcset(node: &NodeRef, viewport_width: f64) -> Option<Vec<(String, SrcsetDescriptor)>> {
    let parent = node.parent()?;
    if !tag_name(&parent).eq_ignore_ascii_case("picture") {
        return None;
    }
    for child in parent.children() {
        if child.as_element().is_none() || !tag_name(&child).eq_ignore_ascii_case("source") {
            continue;
        }
        if let Some(media) = attr(&child, "media") {
            if !media_matches(media.trim(), viewport_width) {
                continue;
            }
        }
        if let Some(t) = attr(&child, "type") {
            let t = t.trim().to_ascii_lowercase();
            if !t.is_empty() && !t.starts_with("image/") {
                continue;
            }
        }
        let raw = attr(&child, "srcset")?;
        let cands = parse_srcset(&raw);
        if !cands.is_empty() {
            return Some(cands);
        }
    }
    None
}

/// 极简 `media` 匹配:支持 `all`、`min-width`/`max-width`(px)与逗号分隔的「与」。
fn media_matches(media: &str, viewport_width: f64) -> bool {
    if media.is_empty() {
        return true;
    }
    for cond in media.split(',') {
        let cond = cond.trim().to_ascii_lowercase();
        let cond = cond
            .strip_prefix('(')
            .and_then(|c| c.strip_suffix(')'))
            .unwrap_or(&cond)
            .trim();
        if cond.is_empty() || cond == "all" {
            continue;
        }
        if let Some(v) = cond.strip_prefix("min-width:") {
            let px = v.trim().trim_end_matches("px").trim().parse::<f64>().unwrap_or(0.0);
            if viewport_width + 0.5 < px {
                return false;
            }
            continue;
        }
        if let Some(v) = cond.strip_prefix("max-width:") {
            let px = v.trim().trim_end_matches("px").trim().parse::<f64>().unwrap_or(f64::MAX);
            if viewport_width - 0.5 > px {
                return false;
            }
            continue;
        }
        // 未支持的条件一律不匹配(保守:不因未知条件选中错误候选)。
        return false;
    }
    true
}

pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push_str("…");
    out
}

fn escape_attr(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn is_visible(node: &NodeRef) -> bool {
    let mut cur = Some(node.clone());
    let mut depth = 0;
    while let Some(n) = cur {
        if depth > 40 {
            break;
        }
        if let Some(el) = n.as_element() {
            let tag = el.name.local.to_string();
            if matches!(tag.as_str(), "script" | "style" | "template" | "noscript" | "head") {
                return false;
            }
            let attrs = el.attributes.borrow();
            if attrs.contains("hidden") {
                return false;
            }
            if attrs.get("aria-hidden").map(|v| v.eq_ignore_ascii_case("true")).unwrap_or(false) {
                return false;
            }
            if attrs
                .get("style")
                .map(|v| {
                    let v = v.to_ascii_lowercase().replace(' ', "");
                    v.contains("display:none") || v.contains("visibility:hidden")
                })
                .unwrap_or(false)
            {
                return false;
            }
            if tag == "input"
                && attrs
                    .get("type")
                    .map(|v| v.eq_ignore_ascii_case("hidden"))
                    .unwrap_or(false)
            {
                return false;
            }
        }
        cur = n.parent();
        depth += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTML: &str = r#"<!doctype html><html><head><title>T</title><style>.a{color:red}</style></head>
<body><div id="main"><a class="a" href="/next">Next</a><form action="/search" method="get">
<input name="q" value="rust"><input type="checkbox" name="ok" checked><select name="s">
<option value="1">One</option><option value="2" selected>Two</option></select>
<button type="submit">Go</button></form></div></body></html>"#;

    #[test]
    fn 解析标题与正文() {
        let d = Document::parse(HTML, "https://a.test/");
        assert_eq!(d.title(), "T");
        let text = d.text();
        assert!(text.contains("Next"));
        assert!(!text.contains("color:red"));
    }

    #[test]
    fn 可访问名从_data_title_等属性推导() {
        // 无文本图标按钮(百度地图路线按钮形态):DOM 无文本节点,只能靠属性认控件。
        let html = r#"<!doctype html><html><body>
        <div class="searchbox-content-button route-button" data-title="路线"></div>
        <input id="q" type="text" placeholder="搜地点、查公交、找路线">
        <button title="搜索" class="b"></button>
        <img alt="卫星图">
        <label for="pw">密码</label><input id="pw" type="password">
        </body></html>"#;
        let d = Document::parse(html, "https://a.test/");
        let pick = |sel: &str| {
            let n = d.first(sel).unwrap().unwrap();
            let attrs = common_attrs(&n);
            let label_for = attrs.get("id").filter(|id| !id.is_empty()).and_then(|id| {
                let s = format!("label[for=\"{}\"]", id.replace('"', ""));
                d.root
                    .select(&s)
                    .ok()?
                    .map(|x| x.as_node().clone())
                    .map(|x| collapse_ws(&x.text_contents()))
                    .find(|t| !t.is_empty())
            });
            accessible_name(&n, &attrs, label_for)
        };
        assert_eq!(pick(".route-button"), "路线");
        assert_eq!(pick("input#q"), "搜地点、查公交、找路线");
        assert_eq!(pick("button.b"), "搜索");
        assert_eq!(pick("img"), "卫星图");
        assert_eq!(pick("input#pw"), "密码");
    }

    #[test]
    fn 元素JSON带可访问名字段() {
        let html = r#"<!doctype html><html><body>
        <div class="route-button" data-title="路线"></div></body></html>"#;
        let d = Document::parse(html, "https://a.test/");
        let n = d.first(".route-button").unwrap().unwrap();
        let v = d.element_json(&n, 0, 1, None, &HashMap::new(), &HashMap::new(), None);
        assert_eq!(v["label"], "路线");
        assert_eq!(v["text"], "");
    }

    #[test]
    fn 元素JSON输出归一化控件状态() {
        let html = r#"<!doctype html><html><body>
        <input id="half" type="checkbox" indeterminate>
        <input id="ro" readonly>
        <select id="s"><option selected>甲</option></select>
        </body></html>"#;
        let mut d = Document::parse(html, "https://a.test/");
        let mut pseudo: HashMap<String, PseudoFlags> = HashMap::new();
        pseudo.insert(
            "#half".to_string(),
            PseudoFlags {
                hover: true,
                ..Default::default()
            },
        );
        d.apply_widget_state(&HashMap::new(), &HashMap::new(), &pseudo, None);
        let node = d.first("#half").unwrap().unwrap();
        let v = d.element_json(&node, 0, 1, None, &HashMap::new(), &pseudo, None);
        assert_eq!(v["state"]["indeterminate"], true);
        assert_eq!(v["state"]["hovered"], true);
        assert_eq!(v["state"]["checked"], false);

        let node = d.first("#ro").unwrap().unwrap();
        let v = d.element_json(&node, 0, 1, None, &HashMap::new(), &pseudo, None);
        assert_eq!(v["state"]["readonly"], true);

        let node = d.first("#s option").unwrap().unwrap();
        let v = d.element_json(&node, 0, 1, None, &HashMap::new(), &pseudo, None);
        assert_eq!(v["state"]["selected"], true);
    }

    #[test]
    fn 交互候选集包含浮层选项与折叠开关() {
        let html = r#"<!doctype html><html><body>
        <select><option>甲</option></select>
        <details><summary>更多</summary><p>正文</p></details>
        <label for="q">标签</label><input id="q">
        <div tabindex="0">自定义焦点</div>
        </body></html>"#;
        let d = Document::parse(html, "https://a.test/");
        let nodes = d.interactive_nodes();
        let tags: Vec<String> = nodes.iter().map(|n| tag_name(n)).collect();
        assert!(tags.contains(&"option".to_string()), "默认候选集应包含 option:{tags:?}");
        assert!(tags.contains(&"summary".to_string()), "默认候选集应包含 summary:{tags:?}");
        assert!(tags.contains(&"label".to_string()), "默认候选集应包含 label:{tags:?}");
        assert!(tags.contains(&"div".to_string()), "默认候选集应包含 tabindex 控件:{tags:?}");
    }

    #[test]
    fn 稳定选择器按标签路径生成() {
        let d = Document::parse(HTML, "https://a.test/");
        let node = d.first("a").unwrap().unwrap();
        assert_eq!(Document::stable_selector(&node), "html > body > div > a");
    }

    #[test]
    fn 相对链接解析() {
        let d = Document::parse(HTML, "https://a.test/dir/");
        let node = d.first("a").unwrap().unwrap();
        let v = d.link_json(&node).unwrap();
        assert_eq!(v["href"], "https://a.test/next");
    }

    #[test]
    fn 表单序列化包含选中值() {
        let d = Document::parse(HTML, "https://a.test/");
        let input = d.first("input[name=q]").unwrap().unwrap();
        let f = d.form_request(&input, &HashMap::new()).unwrap();
        let (url, body) = f.to_request();
        assert!(url.starts_with("https://a.test/search?"));
        assert!(url.contains("q=rust"));
        assert!(url.contains("ok=on"));
        assert!(url.contains("s=2"));
        assert!(body.is_none());
    }

    #[test]
    fn post_表单返回请求体() {
        let f = FormRequest {
            action: "https://a.test/p".into(),
            method: "POST".into(),
            fields: vec![("a".into(), "1 2".into())],
        };
        let (url, body) = f.to_request();
        assert_eq!(url, "https://a.test/p");
        assert_eq!(body.unwrap(), b"a=1+2".to_vec());
    }

    #[test]
    fn 语义属性经快照序列化保真() {
        let html = r#"<body><details open><summary>标题</summary><p>正文</p></details>
        <table><tr><td colspan="2" rowspan="3">跨格</td></tr></table>
        <img src="/a.png" srcset="/a.png 1x, /b.png 2x" poster="/p.jpg">
        <video controls></video><input type="range" min="1" max="9" step="2" required></body>"#;
        let d = Document::parse(html, "https://a.test/");
        let out = d.serialize_full();
        for key in [
            "open", "colspan", "rowspan", "srcset", "poster", "controls", "min", "max",
            "step", "required",
        ] {
            assert!(out.contains(key), "序列化不应丢失语义属性 - {key}: {out}");
        }
    }

    #[test]
    fn 隐藏元素不可见() {        let d = Document::parse(
            "<body><div style='display:none'><a href='/x'>x</a></div><a href='/y'>y</a></body>",
            "https://a.test/",
        );
        let links = d.select("a").unwrap();
        assert!(!is_visible(&links[0]));
        assert!(is_visible(&links[1]));
    }
    #[test]
    fn srcset_候选解析与选择() {
        let cands = parse_srcset("/a.png 1x, /b.png 2x");
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].0, "/a.png");
        assert_eq!(cands[1].1, SrcsetDescriptor::Density(2.0));
        // 1x 视口选 1x 候选;2x 视口选 2x 候选。
        assert_eq!(
            select_srcset(&cands, 800.0, 1.0).as_deref(),
            Some("/a.png")
        );
        assert_eq!(
            select_srcset(&cands, 800.0, 2.0).as_deref(),
            Some("/b.png")
        );
    }

    #[test]
    fn srcset_宽度描述符按视口选候选() {
        let cands = parse_srcset("/s.png 100w, /m.png 800w, /l.png 1920w");
        // 视口 800 → 选不小于 800 的最小宽度候选。
        assert_eq!(
            select_srcset(&cands, 800.0, 1.0).as_deref(),
            Some("/m.png")
        );
        // 视口 400 → 100w 不够,选 800w。
        assert_eq!(
            select_srcset(&cands, 400.0, 1.0).as_deref(),
            Some("/m.png")
        );
        // 视口 2000 → 没有更大的,取最大宽度候选。
        assert_eq!(
            select_srcset(&cands, 2000.0, 1.0).as_deref(),
            Some("/l.png")
        );
    }

    #[test]
    fn picture_取命中的source候选() {
        let d = Document::parse(
            "<body><picture><source media='(min-width: 1000px)' srcset='/wide.png'><source srcset='/narrow.png'><img id='i' src='/fallback.png'></picture></body>",
            "https://a.test/",
        );
        let img = d.select("#i").unwrap().remove(0);
        // 视口 500 → 宽屏 source 不匹配,取第二个。
        let cands = picture_srcset(&img, 500.0).expect("应有 source 候选");
        assert_eq!(cands[0].0, "/narrow.png");
        // 视口 1200 → 命中宽屏 source。
        let cands = picture_srcset(&img, 1200.0).expect("应有 source 候选");
        assert_eq!(cands[0].0, "/wide.png");
    }

    #[test]
    fn 页面选区中间叶子元素整段点亮() {
        let html = r#"<body><p id="a">第一段文字</p><span id="mid">中间文字</span><span id="mid2"></span><p id="b">最后一段</p></body>"#;
        let mut d = Document::parse(html, "https://a.test/");
        let sel_a = d.first("#a").unwrap().unwrap();
        let sel_b = d.first("#b").unwrap().unwrap();
        let ps = PageSelection {
            anchor_sel: Document::stable_selector(&sel_a),
            anchor_idx: 1,
            head_sel: Document::stable_selector(&sel_b),
            head_idx: 2,
        };
        d.apply_page_sel(&ps);
        let attr_of = |sel: &str| {
            let n = d.first(sel).unwrap().unwrap();
            attr(&n, ATTR_PAGE_SEL)
        };
        // 首元素:从 1 到末尾(-1)。
        assert_eq!(attr_of("#a").as_deref(), Some("1,-1"));
        // 中间叶子(有文本)整段点亮;空文本叶子跳过。
        assert_eq!(attr_of("#mid").as_deref(), Some("0,-1"));
        assert_eq!(attr_of("#mid2"), None, "空文本叶子不标记");
        // 尾元素:0 到 end。
        assert_eq!(attr_of("#b").as_deref(), Some("0,2"));
    }

    #[test]
    fn 页面选区同元素不产生中间标记() {
        let html = r#"<body><p id="a">一段文字</p><p id="b">另一段</p></body>"#;
        let mut d = Document::parse(html, "https://a.test/");
        let a = d.first("#a").unwrap().unwrap();
        let ps = PageSelection {
            anchor_sel: Document::stable_selector(&a),
            anchor_idx: 1,
            head_sel: Document::stable_selector(&a),
            head_idx: 3,
        };
        d.apply_page_sel(&ps);
        assert_eq!(attr(&d.first("#a").unwrap().unwrap(), ATTR_PAGE_SEL).as_deref(), Some("1,3"));
        assert!(attr(&d.first("#b").unwrap().unwrap(), ATTR_PAGE_SEL).is_none());
    }

    #[test]
    fn 快照select读到选中值与序号() {
        let html = r#"<body><select id="s"><option value="1">One</option><option value="2" selected>Two</option></select>
<select id="empty"></select><select id="d"><option value="x">X</option></select></body>"#;
        let d = Document::parse(html, "https://a.test/");
        let nodes = d.snapshot_nodes();
        let find = |id: &str| nodes.iter().find(|n| n.id == id).unwrap();
        // 带 selected:值 = "2"、序号 = 1。
        let s = find("s");
        assert_eq!(s.value, "2");
        assert_eq!(s.selected_index, Some(1));
        // 无 option:值空、序号 −1。
        let empty = find("empty");
        assert_eq!(empty.value, "");
        assert_eq!(empty.selected_index, Some(-1));
        // 无 selected 属性:回落首项(浏览器 DOM 语义)。
        let dd = find("d");
        assert_eq!(dd.value, "x");
        assert_eq!(dd.selected_index, Some(0));
        // option 的 selected 布尔。
        let opt2 = nodes
            .iter()
            .find(|n| n.tag == "option" && n.value == "2")
            .expect("option 应在快照里");
        assert!(opt2.selected);
    }

}
