//! 块级流式布局 + 文本换行。
//!
//! 样式全部来自 `style::StyleEngine` 的计算样式;本模块只负责几何计算。
//! 产出纯数据(`TextLine`/`ElementBox`),不持有 DOM 句柄,可跨 await 保存。
//! 完整 Flex/Grid/定位在阶段三实现,接口保持不变。

use kuchikiki::NodeRef;

use super::document::{self, Document};
use super::render::FontSet;
use super::style::{ComputedStyle, MediaCtx, StyleEngine};

#[derive(Debug, Clone)]
pub struct TextLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub color: [u8; 3],
    pub bold: bool,
    /// `visibility:hidden` 的文本仍占布局空间,但不绘制。
    pub visible: bool,
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

/// 解析文档并布局(默认媒体上下文)。
pub fn compute(doc: &Document, viewport_width: f64, font: &FontSet) -> Layout {
    let media = MediaCtx::new(viewport_width, 720.0);
    compute_with(doc, viewport_width, font, &media)
}

/// 指定媒体上下文(视口/配色/打印)的布局。
pub fn compute_with(
    doc: &Document,
    viewport_width: f64,
    font: &FontSet,
    media: &MediaCtx,
) -> Layout {
    let engine = StyleEngine::from_document(doc, *media);
    let mut ctx = LayoutCtx {
        engine: &engine,
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
    let initial = engine.initial_style();
    let parent_style = doc
        .first("html")
        .ok()
        .flatten()
        .map(|n| engine.computed(&n, &initial))
        .unwrap_or_else(|| initial.clone());
    let body_style = engine.computed(&body, &parent_style);
    let mut buf = InlineBuf::default();
    let content_width = ctx.width - 16.0;
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
    engine: &'a StyleEngine,
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
        current: &ComputedStyle,
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
            let style = self.engine.computed(&child, current);
            if style.display == "none" {
                continue;
            }
            let selector = Document::stable_selector(&child);
            if style.is_block() {
                self.flush_inline(buf);
                self.y += style.margin_top;
                let bx = x + style.margin_left;
                let mut bw = (width
                    - style.margin_left
                    - style.margin_right
                    - style.padding_left
                    - style.padding_right)
                    .max(40.0);
                if let Some(w) = style.width {
                    bw = w.max(40.0);
                }
                if let Some(mw) = style.max_width {
                    bw = bw.min(mw.max(1.0));
                }
                let start_y = self.y;
                self.y += style.padding_top;
                self.layout_children(
                    &child,
                    bx + style.padding_left,
                    (bw - style.padding_left - style.padding_right).max(40.0),
                    buf,
                    depth + 1,
                    &style,
                );
                self.flush_inline(buf);
                self.y += style.padding_bottom + style.margin_bottom.max(2.0);
                self.boxes.push(ElementBox {
                    selector,
                    tag: document::tag_name(&child),
                    x: bx,
                    y: start_y,
                    w: bw,
                    h: (self.y - start_y).max(1.0),
                    background: style.background,
                    visible: style.visibility != "hidden"
                        && style.visibility != "collapse"
                        && style.opacity > 0.0,
                });
                continue;
            }
            // 行内元素:递归收集文本,保留自身样式。
            self.collect_inline(&child, &style, buf, depth + 1, x);
        }
    }

    fn collect_inline(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        buf: &mut InlineBuf,
        depth: usize,
        line_x: f64,
    ) {
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
            let child_style = self.engine.computed(&child, style);
            if child_style.display == "none" {
                continue;
            }
            let tag = document::tag_name(&child);
            if tag == "br" {
                buf.force_break();
                continue;
            }
            // 交互类行内元素:记录近似矩形,供 inspect(elements) 与坐标点击使用。
            if matches!(
                tag.as_str(),
                "a" | "button" | "input" | "select" | "textarea" | "label"
            ) {
                let selector = Document::stable_selector(&child);
                let text = child.text_contents();
                let w = self.font.text_width(&text, child_style.font_size).max(40.0);
                self.boxes.push(ElementBox {
                    selector,
                    tag: tag.clone(),
                    x: line_x + child_style.margin_left + child_style.padding_left,
                    y: self.y,
                    w,
                    h: (child_style.font_size * 1.45).max(16.0),
                    background: child_style.background,
                    visible: child_style.visibility != "hidden"
                        && child_style.visibility != "collapse"
                        && child_style.opacity > 0.0,
                });
            }
            self.collect_inline(&child, &child_style, buf, depth + 1, line_x);
        }
    }

    fn flush_inline(&mut self, buf: &mut InlineBuf) {
        let max_w = (self.width - 24.0).max(80.0);
        for run in buf.runs.drain(..) {
            let size = run.size.max(8.0);
            let line_height = run.line_height.max(size * 1.35);
            let mut current = String::new();
            let mut current_w = 0.0;
            for ch in run.text.chars() {
                if ch == '\n' {
                    self.push_line(
                        &current,
                        run.x,
                        &run,
                        line_height,
                        current_w,
                        max_w,
                    );
                    current.clear();
                    current_w = 0.0;
                    continue;
                }
                let w = self.font.char_width(ch, size);
                if run.wrap && current_w + w > max_w && !current.is_empty() {
                    self.push_line(
                        &current,
                        run.x,
                        &run,
                        line_height,
                        current_w,
                        max_w,
                    );
                    current.clear();
                    current_w = 0.0;
                }
                current.push(ch);
                current_w += w;
            }
            if !current.is_empty() && (run.preserve || !current.trim().is_empty()) {
                self.push_line(&current, run.x, &run, line_height, current_w, max_w);
            }
        }
        buf.chars = 0;
    }

    fn push_line(
        &mut self,
        text: &str,
        x: f64,
        run: &InlineRun,
        line_height: f64,
        line_w: f64,
        max_w: f64,
    ) {
        let content = if run.preserve {
            text.trim_end_matches('\n').to_string()
        } else {
            text.trim().to_string()
        };
        if content.is_empty() {
            return;
        }
        let offset = match run.align.as_str() {
            "center" => ((max_w - line_w) / 2.0).max(0.0),
            "right" => (max_w - line_w).max(0.0),
            _ => 0.0,
        };
        self.lines.push(TextLine {
            text: content,
            x: x + offset,
            y: self.y,
            size: run.size,
            color: run.color,
            bold: run.bold,
            visible: run.visible,
        });
        self.y += line_height;
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
    visible: bool,
    align: String,
    wrap: bool,
    preserve: bool,
}

impl InlineBuf {
    fn push_styled(&mut self, text: &str, st: &ComputedStyle) {
        self.chars += text.chars().count();
        let size = st.font_size;
        let bold = st.font_weight >= 600;
        let visible = st.visibility != "hidden" && st.visibility != "collapse" && st.opacity > 0.0;
        let x = 8.0 + st.margin_left + st.padding_left;
        let preserve = matches!(st.white_space.as_str(), "pre" | "pre-wrap" | "break-spaces");
        let wrap = !matches!(st.white_space.as_str(), "pre" | "nowrap");
        if let Some(last) = self.runs.last_mut() {
            if (last.size - size).abs() < 0.1
                && last.color == st.color
                && last.bold == bold
                && last.visible == visible
                && last.x == x
                && last.align == st.text_align
                && last.wrap == wrap
                && last.preserve == preserve
            {
                last.text.push_str(text);
                return;
            }
        }
        self.runs.push(InlineRun {
            text: text.to_string(),
            x,
            size,
            color: st.color,
            bold,
            line_height: if st.line_height > 0.0 {
                st.line_height
            } else {
                size * 1.45
            },
            visible,
            align: st.text_align.clone(),
            wrap,
            preserve,
        });
    }

    fn force_break(&mut self) {
        if let Some(last) = self.runs.last_mut() {
            last.text.push('\n');
        }
    }
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
    fn 继承的颜色与字号作用于子元素() {
        let doc = Document::parse(
            "<head><style>body{color:#00ff00;font-size:24px}</style></head>\
             <body><div><span>Child</span></div></body>",
            "https://a.test/",
        );
        let layout = compute(&doc, 800.0, &font());
        let line = layout.lines.iter().find(|l| l.text == "Child").unwrap();
        assert_eq!(line.color, [0, 255, 0]);
        assert!((line.size - 24.0).abs() < 0.1);
    }

    #[test]
    fn visibility_hidden_保留布局空间() {
        let doc = Document::parse(
            "<body><div style='visibility:hidden'>ghost</div><p>after</p></body>",
            "https://a.test/",
        );
        let layout = compute(&doc, 800.0, &font());
        let ghost = layout.lines.iter().find(|l| l.text.contains("ghost")).unwrap();
        assert!(!ghost.visible);
        assert!(layout.lines.iter().any(|l| l.text.contains("after")));
    }

    #[test]
    fn text_align_center_偏移行起点() {
        let doc = Document::parse(
            "<body><p style='text-align:center;width:400px'>mid</p></body>",
            "https://a.test/",
        );
        let layout = compute(&doc, 800.0, &font());
        let line = layout.lines.iter().find(|l| l.text == "mid").unwrap();
        assert!(line.x > 100.0);
    }
}
