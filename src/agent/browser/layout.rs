//! 阶段一布局引擎:块级流式布局 + 文本换行 + 简化 CSS 级联。
//!
//! 产出纯数据(`TextLine`/`ElementBox`),不持有 DOM 句柄,可跨 await 保存。
//! 完整 Flex/Grid/定位在阶段三实现,接口保持不变。

use std::collections::HashMap;

use kuchikiki::NodeRef;

use super::document::{self, Document};
use super::render::FontSet;

#[derive(Debug, Clone)]
pub struct TextLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub color: [u8; 3],
    pub bold: bool,
}

#[derive(Debug, Clone)]
pub struct ElementBox {
    pub selector: String,
    pub tag: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub background: Option<[u8; 3]>,
    pub visible: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub lines: Vec<TextLine>,
    pub boxes: Vec<ElementBox>,
    pub width: f64,
    pub height: f64,
}

impl Layout {
    pub fn rect_for(&self, selector: &str) -> Option<(f64, f64, f64, f64)> {
        self.boxes
            .iter()
            .find(|b| b.selector == selector)
            .map(|b| (b.x, b.y, b.w, b.h))
    }

    pub fn page_height(&self) -> f64 {
        self.height.max(1.0)
    }
}

#[derive(Debug, Clone, Default)]
struct Style {
    display: String,
    color: [u8; 3],
    background: Option<[u8; 3]>,
    font_size: f64,
    bold: bool,
    margin_top: f64,
    margin_bottom: f64,
    margin_left: f64,
    padding_left: f64,
    padding_top: f64,
    line_height: f64,
}

impl Style {
    fn is_block(&self) -> bool {
        self.display == "block"
            || self.display == "flex"
            || self.display == "grid"
            || self.display == "table"
            || self.display == "list-item"
    }
}

#[derive(Debug, Clone)]
struct Rule {
    selectors: Vec<String>,
    decls: HashMap<String, String>,
    specificity: u32,
}

/// 解析文档并布局。
pub fn compute(doc: &Document, viewport_width: f64, font: &FontSet) -> Layout {
    let rules = parse_stylesheets(doc);
    let mut ctx = LayoutCtx {
        rules,
        font,
        width: viewport_width.max(320.0),
        lines: Vec::new(),
        boxes: Vec::new(),
        y: 8.0,
    };
    let body = doc
        .root()
        .select_first("body")
        .ok()
        .map(|n| n.as_node().clone())
        .unwrap_or_else(|| doc.root().clone());
    let mut buf = InlineBuf::default();
    let content_width = ctx.width - 16.0;
    let body_style = default_style("body");
    ctx.layout_children(&body, 8.0, content_width, &mut buf, 0, &body_style);
    ctx.flush_inline(&mut buf);
    let height = ctx.y + 8.0;
    Layout {
        lines: ctx.lines,
        boxes: ctx.boxes,
        width: ctx.width,
        height,
    }
}

struct LayoutCtx<'a> {
    rules: Vec<Rule>,
    font: &'a FontSet,
    width: f64,
    lines: Vec<TextLine>,
    boxes: Vec<ElementBox>,
    y: f64,
}

impl<'a> LayoutCtx<'a> {
    fn layout_children(
        &mut self,
        parent: &NodeRef,
        x: f64,
        width: f64,
        buf: &mut InlineBuf,
        depth: usize,
        current: &Style,
    ) {
        if depth > 60 {
            return;
        }
        for child in parent.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    buf.push_styled(&raw, current);
                }
                continue;
            }
            if child.as_element().is_none() {
                continue;
            }
            let style = self.computed_style(&child);
            if style.display == "none" {
                continue;
            }
            let selector = Document::stable_selector(&child);
            if style.is_block() || child.children().next().is_some() {
                if style.is_block() {
                    self.flush_inline(buf);
                    self.y += style.margin_top;
                    let bx = x + style.margin_left;
                    let bw = (width - style.margin_left - style.padding_left).max(40.0);
                    let start_y = self.y;
                    self.y += style.padding_top;
                    self.layout_children(&child, bx + style.padding_left, bw, buf, depth + 1, &style);
                    self.flush_inline(buf);
                    self.y += style.margin_bottom.max(2.0);
                    self.boxes.push(ElementBox {
                        selector,
                        tag: document::tag_name(&child),
                        x: bx,
                        y: start_y,
                        w: bw,
                        h: (self.y - start_y).max(1.0),
                        background: style.background,
                        visible: document::is_visible(&child),
                    });
                    continue;
                }
            }
            // 行内元素:递归收集文本,保留父级样式。
            self.collect_inline(&child, &style, buf, depth + 1);
        }
    }

    fn collect_inline(&mut self, node: &NodeRef, style: &Style, buf: &mut InlineBuf, depth: usize) {
        if depth > 40 || buf.chars > 200_000 {
            return;
        }
        for child in node.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    buf.push_styled(&raw, style);
                }
                continue;
            }
            if child.as_element().is_none() {
                continue;
            }
            let child_style = self.computed_style(&child);
            if child_style.display == "none" {
                continue;
            }
            let tag = document::tag_name(&child);
            if tag == "br" {
                buf.force_break();
                continue;
            }
            // 交互类行内元素:记录近似矩形,供 inspect(elements) 与坐标点击使用。
            if matches!(tag.as_str(), "a" | "button" | "input" | "select" | "textarea" | "label") {
                let selector = Document::stable_selector(&child);
                let text = child.text_contents();
                let w = self.font.text_width(&text, child_style.font_size).max(40.0);
                self.boxes.push(ElementBox {
                    selector,
                    tag: tag.clone(),
                    x: 8.0 + child_style.margin_left + child_style.padding_left,
                    y: self.y,
                    w,
                    h: (child_style.font_size * 1.45).max(16.0),
                    background: child_style.background,
                    visible: document::is_visible(&child),
                });
            }
            self.collect_inline(&child, &child_style, buf, depth + 1);
        }
    }

    fn flush_inline(&mut self, buf: &mut InlineBuf) {
        for run in buf.runs.drain(..) {
            let size = run.size.max(8.0);
            let line_height = run.line_height.max(size * 1.35);
            let mut current = String::new();
            let mut current_w = 0.0;
            let max_w = (self.width - 24.0).max(80.0);
            for ch in run.text.chars() {
                if ch == '\n' {
                    self.push_line(&current, run.x, run.size, run.color, run.bold, line_height);
                    current.clear();
                    current_w = 0.0;
                    continue;
                }
                let w = self.font.char_width(ch, size);
                if current_w + w > max_w && !current.is_empty() {
                    self.push_line(&current, run.x, run.size, run.color, run.bold, line_height);
                    current.clear();
                    current_w = 0.0;
                }
                current.push(ch);
                current_w += w;
            }
            if !current.trim().is_empty() {
                self.push_line(&current, run.x, run.size, run.color, run.bold, line_height);
            }
        }
        buf.chars = 0;
    }

    fn push_line(
        &mut self,
        text: &str,
        x: f64,
        size: f64,
        color: [u8; 3],
        bold: bool,
        line_height: f64,
    ) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        self.lines.push(TextLine {
            text: trimmed.to_string(),
            x,
            y: self.y,
            size,
            color,
            bold,
        });
        self.y += line_height;
    }

    fn computed_style(&self, node: &NodeRef) -> Style {
        let tag = document::tag_name(node);
        let mut st = default_style(&tag);
        // 作者样式:按选择器命中 + 特异性升序应用。
        let mut matched: Vec<(&Rule, u32)> = Vec::new();
        for rule in &self.rules {
            for sel in &rule.selectors {
                if match_selector(node, sel) {
                    matched.push((rule, rule.specificity));
                    break;
                }
            }
        }
        matched.sort_by_key(|(_, s)| *s);
        for (rule, _) in matched {
            for (k, v) in &rule.decls {
                apply_decl(&mut st, k, v);
            }
        }
        if let Some(inline) = document::attr(node, "style") {
            for decl in inline.split(';') {
                if let Some((k, v)) = decl.split_once(':') {
                    apply_decl(&mut st, k.trim(), v.trim());
                }
            }
        }
        if document::attr(node, "hidden").is_some()
            || document::attr(node, "aria-hidden").map(|v| v == "true").unwrap_or(false)
        {
            st.display = "none".to_string();
        }
        st
    }
}

#[derive(Default)]
struct InlineBuf {
    runs: Vec<InlineRun>,
    chars: usize,
}

struct InlineRun {
    text: String,
    x: f64,
    size: f64,
    color: [u8; 3],
    bold: bool,
    line_height: f64,
}

impl InlineBuf {
    fn push_styled(&mut self, text: &str, st: &Style) {
        self.chars += text.chars().count();
        let size = st.font_size;
        if let Some(last) = self.runs.last_mut() {
            if (last.size - size).abs() < 0.1
                && last.color == st.color
                && last.bold == st.bold
                && last.x == st.margin_left
            {
                last.text.push_str(text);
                return;
            }
        }
        self.runs.push(InlineRun {
            text: text.to_string(),
            x: 8.0 + st.margin_left + st.padding_left,
            size,
            color: st.color,
            bold: st.bold,
            line_height: if st.line_height > 0.0 {
                st.line_height
            } else {
                size * 1.45
            },
        });
    }

    fn force_break(&mut self) {
        if let Some(last) = self.runs.last_mut() {
            last.text.push('\n');
        }
    }
}

fn default_style(tag: &str) -> Style {
    let block = matches!(
        tag,
        "html"
            | "body"
            | "div"
            | "p"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "form"
            | "table"
            | "thead"
            | "tbody"
            | "tr"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "nav"
            | "aside"
            | "blockquote"
            | "pre"
            | "figure"
            | "figcaption"
            | "fieldset"
    );
    let font_size = match tag {
        "h1" => 32.0,
        "h2" => 28.0,
        "h3" => 24.0,
        "h4" => 20.0,
        "h5" => 18.0,
        "h6" => 16.0,
        "small" => 13.0,
        _ => 16.0,
    };
    let margin = match tag {
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "pre" | "blockquote" => 8.0,
        _ => 0.0,
    };
    Style {
        display: if block { "block" } else { "inline" }.to_string(),
        color: [32, 32, 32],
        background: None,
        font_size,
        bold: tag.starts_with('h') || tag == "strong" || tag == "b",
        margin_top: margin,
        margin_bottom: margin,
        margin_left: if tag == "li" { 20.0 } else { 0.0 },
        padding_left: 0.0,
        padding_top: 0.0,
        line_height: font_size * 1.45,
    }
}

fn apply_decl(st: &mut Style, key: &str, value: &str) {
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    match key.as_str() {
        "display" => st.display = value.to_ascii_lowercase(),
        "visibility" if value.eq_ignore_ascii_case("hidden") => st.display = "none".to_string(),
        "color" => {
            if let Some(c) = parse_color(value) {
                st.color = c;
            }
        }
        "background-color" | "background" => {
            st.background = parse_color(value);
        }
        "font-size" => {
            if let Some(v) = parse_px(value) {
                st.font_size = v.clamp(8.0, 96.0);
                st.line_height = st.font_size * 1.45;
            }
        }
        "font-weight" => {
            st.bold = value == "bold" || value.parse::<u32>().map(|n| n >= 600).unwrap_or(false);
        }
        "margin" => {
            if let Some(v) = parse_px(value) {
                st.margin_top = v;
                st.margin_bottom = v;
                st.margin_left = v;
            }
        }
        "margin-top" => st.margin_top = parse_px(value).unwrap_or(st.margin_top),
        "margin-bottom" => st.margin_bottom = parse_px(value).unwrap_or(st.margin_bottom),
        "margin-left" => st.margin_left = parse_px(value).unwrap_or(st.margin_left),
        "padding" => {
            if let Some(v) = parse_px(value) {
                st.padding_left = v;
                st.padding_top = v;
            }
        }
        "padding-left" => st.padding_left = parse_px(value).unwrap_or(st.padding_left),
        "padding-top" => st.padding_top = parse_px(value).unwrap_or(st.padding_top),
        "line-height" => {
            if let Some(v) = parse_px(value) {
                st.line_height = v;
            }
        }
        _ => {}
    }
}

fn parse_px(value: &str) -> Option<f64> {
    let v = value.trim();
    let num = v.trim_end_matches("px").trim();
    num.parse::<f64>().ok()
}

fn parse_color(value: &str) -> Option<[u8; 3]> {
    let v = value.trim().to_ascii_lowercase();
    let named = match v.as_str() {
        "white" => Some([255, 255, 255]),
        "black" => Some([0, 0, 0]),
        "red" => Some([220, 50, 50]),
        "green" => Some([40, 160, 60]),
        "blue" => Some([40, 80, 220]),
        "gray" | "grey" => Some([128, 128, 128]),
        "yellow" => Some([240, 220, 40]),
        "transparent" => None,
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
                Some([r, g, b])
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some([r, g, b])
            }
            _ => None,
        };
    }
    if let Some(rest) = v.strip_prefix("rgb(") {
        let rest = rest.trim_end_matches(')');
        let nums: Vec<u8> = rest
            .split(',')
            .filter_map(|s| s.trim().parse::<u8>().ok())
            .collect();
        if nums.len() == 3 {
            return Some([nums[0], nums[1], nums[2]]);
        }
    }
    None
}

fn parse_stylesheets(doc: &Document) -> Vec<Rule> {
    let mut rules = Vec::new();
    for style_node in doc.select("style").unwrap_or_default() {
        let css = style_node.text_contents();
        rules.extend(parse_css(&css));
    }
    rules
}

fn parse_css(css: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let selectors_raw = &rest[..open];
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let body = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let selectors: Vec<String> = selectors_raw
            .split(',')
            .map(|s| strip_pseudo(s.trim()))
            .filter(|s| !s.is_empty() && !s.starts_with('@'))
            .collect();
        if selectors.is_empty() {
            continue;
        }
        let mut decls = HashMap::new();
        for decl in body.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                decls.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        if decls.is_empty() {
            continue;
        }
        let spec = selectors.iter().map(|s| specificity(s)).max().unwrap_or(0);
        out.push(Rule {
            selectors,
            decls,
            specificity: spec,
        });
    }
    out
}

fn strip_pseudo(sel: &str) -> String {
    let mut out = String::new();
    let mut depth: i32 = 0;
    let mut chars = sel.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && depth == 0 {
            // 跳过伪类/伪元素(保留 :nth-of-type 供匹配)。
            let mut name = String::new();
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '-' {
                    name.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            if name == "nth-of-type" || name == "first-child" || name == "last-child" {
                out.push(':');
                out.push_str(&name);
                if let Some(&'(') = chars.peek() {
                    out.push('(');
                    chars.next();
                    let mut arg = String::new();
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if n == ')' {
                            break;
                        }
                        arg.push(n);
                    }
                    out.push_str(&arg);
                    out.push(')');
                }
            }
            continue;
        }
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth = depth.saturating_sub(1);
        }
        out.push(c);
    }
    out
}

fn specificity(sel: &str) -> u32 {
    let mut ids = 0;
    let mut classes = 0;
    let mut tags = 0;
    let bytes: Vec<char> = sel.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            '#' => {
                ids += 1;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '-' || bytes[i] == '_') {
                    i += 1;
                }
            }
            '.' | '[' => {
                classes += 1;
                i += 1;
            }
            c if c.is_ascii_alphabetic() => {
                tags += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '-') {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    ids * 100 + classes * 10 + tags
}

fn match_selector(node: &NodeRef, selector: &str) -> bool {
    let sel = selector.trim();
    if sel.is_empty() {
        return false;
    }
    // 只处理后代/子代组合:按空白切分,自右向左回溯祖先。
    let parts: Vec<&str> = sel
        .split_whitespace()
        .filter(|p| *p != ">")
        .collect();
    let Some(last) = parts.last() else {
        return false;
    };
    if !match_compound(node, last) {
        return false;
    }
    let mut cur = node.parent();
    for part in parts.iter().rev().skip(1) {
        let mut found = false;
        while let Some(n) = cur.clone() {
            if match_compound(&n, part) {
                cur = n.parent();
                found = true;
                break;
            }
            cur = n.parent();
        }
        if !found {
            return false;
        }
    }
    true
}

fn match_compound(node: &NodeRef, compound: &str) -> bool {
    let Some(el) = node.as_element() else {
        return false;
    };
    let tag = el.name.local.to_string();
    let attrs = el.attributes.borrow();
    let id = attrs.get("id");
    let classes: Vec<&str> = attrs
        .get("class")
        .map(|c| c.split_whitespace().collect())
        .unwrap_or_default();
    let mut rest = compound;
    // 标签 / *
    if !rest.starts_with(['#', '.', '[', ':']) {
        let end = rest
            .find(['#', '.', '[', ':'])
            .unwrap_or(rest.len());
        let tag_sel = &rest[..end];
        if tag_sel != "*" && !tag_sel.eq_ignore_ascii_case(&tag) {
            return false;
        }
        rest = &rest[end..];
    }
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('#') {
            let end = r.find(['#', '.', '[', ':']).unwrap_or(r.len());
            if id != Some(&r[..end]) {
                return false;
            }
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['#', '.', '[', ':']).unwrap_or(r.len());
            if !classes.contains(&&r[..end]) {
                return false;
            }
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else {
                return false;
            };
            let expr = &r[..end];
            if let Some((k, v)) = expr.split_once('=') {
                let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
                if attrs.get(k.trim()) != Some(v) {
                    return false;
                }
            } else if !attrs.contains(expr.trim()) {
                return false;
            }
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix(':') {
            let end = r.find(['#', '.', '[', ':']).unwrap_or(r.len());
            let pseudo = &r[..end];
            if let Some(arg) = pseudo.strip_prefix("nth-of-type(") {
                let arg = arg.trim_end_matches(')');
                let idx = sibling_index(node, &tag);
                if arg != "odd" && arg != "even" && arg.parse::<usize>().ok() != Some(idx) {
                    return false;
                }
            } else if pseudo == "first-child" && sibling_index(node, &tag) != 1 {
                return false;
            }
            rest = &r[end..];
        } else {
            break;
        }
    }
    true
}

fn sibling_index(node: &NodeRef, tag: &str) -> usize {
    let Some(parent) = node.parent() else {
        return 1;
    };
    let mut idx = 0;
    for child in parent.children() {
        if document::tag_name(&child) == tag {
            idx += 1;
            if child == *node {
                return idx;
            }
        }
    }
    idx.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> FontSet {
        FontSet::fallback()
    }

    #[test]
    fn 块级元素产生多行() {
        let doc = Document::parse(
            "<body><h1>Title</h1><p>Hello world</p><div><a href='/x'>link</a></div></body>",
            "https://a.test/",
        );
        let layout = compute(&doc, 800.0, &font());
        assert!(layout.lines.iter().any(|l| l.text == "Title"));
        assert!(layout.lines.iter().any(|l| l.text.contains("Hello")));
        assert!(layout.boxes.iter().any(|b| b.tag == "h1"));
        assert!(layout.height > 40.0);
    }

    #[test]
    fn 长文本按宽度换行() {
        let text = "word ".repeat(400);
        let html = format!("<body><p>{text}</p></body>");
        let doc = Document::parse(&html, "https://a.test/");
        let layout = compute(&doc, 400.0, &font());
        assert!(layout.lines.len() > 5);
    }

    #[test]
    fn display_none_不参与布局() {
        let doc = Document::parse(
            "<body><div style='display:none'>hidden</div><p>shown</p></body>",
            "https://a.test/",
        );
        let layout = compute(&doc, 800.0, &font());
        assert!(!layout.lines.iter().any(|l| l.text.contains("hidden")));
        assert!(layout.lines.iter().any(|l| l.text.contains("shown")));
    }

    #[test]
    fn 样式表字号生效() {
        let doc = Document::parse(
            "<head><style>h1{font-size:40px;color:#ff0000}</style></head><body><h1>X</h1></body>",
            "https://a.test/",
        );
        let layout = compute(&doc, 800.0, &font());
        let h1 = layout.lines.iter().find(|l| l.text == "X").unwrap();
        assert!((h1.size - 40.0).abs() < 0.1);
        assert_eq!(h1.color, [255, 0, 0]);
    }

    #[test]
    fn 选择器匹配_id_类_属性() {
        let doc = Document::parse(
            "<body><div id='a' class='x y' data-k='v'><span>S</span></div></body>",
            "https://a.test/",
        );
        let div = doc.first("div").unwrap().unwrap();
        assert!(match_selector(&div, "div#a.x"));
        assert!(match_selector(&div, "[data-k=v]"));
        assert!(!match_selector(&div, "div#b"));
    }
}
