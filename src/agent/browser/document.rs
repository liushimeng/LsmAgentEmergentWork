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
    ) -> Value {
        let selector = Self::stable_selector(node);
        let tag = tag_name(node);
        // 可见文本而非 text_contents():后者把 <script>/<style> 源码算进来,
        // inspect elements 在 body 上会返回 4000 字符 JS 源码(实测百度地图)。
        // visible_text_of 的 SKIP_TEXT_TAGS 与快照层同一套排除规则。
        let text = truncate_chars(&collapse_ws(&visible_text_of(node)), 4000);
        let attrs = common_attrs(node);
        let value = self.runtime_value(node, &selector, &attrs, values);
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
            "value": value,
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
    pub fn interactive_nodes(&self) -> Vec<NodeRef> {
        self.select(
            "a[href], button, input, textarea, select, [role=\"button\"], [role=\"link\"], [onclick]",
        )
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
        self.snapshot_nodes_inner(None, None)
    }

    /// 带计算样式的快照:每个节点附 `css`(供 JS `getComputedStyle`)。
    pub fn snapshot_nodes_styled(&self, viewport: (u32, u32)) -> Vec<SnapshotNode> {
        let media = super::style::MediaCtx::new(viewport.0 as f64, viewport.1 as f64);
        let engine = StyleEngine::from_document(self, media);
        self.snapshot_nodes_inner(Some(&engine), None)
    }

    /// 带布局矩形的样式快照:rects 由 `layout::Layout::rect_map()` 生成,
    /// 按稳定选择器匹配注入,供 JS 桥暴露 clientWidth/offsetWidth 等几何属性。
    pub fn snapshot_nodes_with_rects(
        &self,
        viewport: (u32, u32),
        rects: &HashMap<String, (f64, f64, f64, f64)>,
    ) -> Vec<SnapshotNode> {
        let media = super::style::MediaCtx::new(viewport.0 as f64, viewport.1 as f64);
        let engine = StyleEngine::from_document(self, media);
        self.snapshot_nodes_inner(Some(&engine), Some(rects))
    }

    fn snapshot_nodes_inner(
        &self,
        engine: Option<&StyleEngine>,
        rects: Option<&HashMap<String, (f64, f64, f64, f64)>>,
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
                value: attrs.get("value").cloned().unwrap_or_default(),
                href: attr(node, "href")
                    .and_then(|h| urlutil::resolve(base_url, &h))
                    .unwrap_or_default(),
                checked: attr(node, "checked").is_some(),
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
    pub text: String,
    pub visible: bool,
    pub attrs: HashMap<String, String>,
    /// 计算样式(kebab-case);无样式引擎时为空。
    pub css: HashMap<String, String>,
    /// 布局矩形 `(x, y, w, h)`:JS 桥的 clientWidth/offsetWidth 等几何属性数据源。
    /// 快照时无布局(如纯样式快照)则为 None。
    pub rect: Option<(f64, f64, f64, f64)>,
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
            "text": self.text,
            "visible": self.visible,
            "attrs": self.attrs,
            "css": self.css,
            "rect": self.rect.map(|r| json!([r.0, r.1, r.2, r.3])),
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

/// 轻量 outerHTML 序列化:覆盖常见属性与子树文本。
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
    fn 隐藏元素不可见() {
        let d = Document::parse(
            "<body><div style='display:none'><a href='/x'>x</a></div><a href='/y'>y</a></body>",
            "https://a.test/",
        );
        let links = d.select("a").unwrap();
        assert!(!is_visible(&links[0]));
        assert!(is_visible(&links[1]));
    }
}
