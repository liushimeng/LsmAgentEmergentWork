//! 布局引擎:块级流式布局 + 文本换行 + Flex 容器 + Grid 网格 + 定位体系(阶段三)。
//!
//! 样式全部来自 `style::StyleEngine` 的计算样式;本模块只负责几何计算。
//! 产出纯数据(`TextLine`/`ElementBox`/`PaintBox`),不持有 DOM 句柄,可跨 await 保存。
//! 目录分工:`mod.rs` 块级/行内主流,`flex.rs` Flex 格式化上下文,
//! `grid.rs` Grid 网格布局,`position.rs` 定位体系,`tests.rs` 布局单测。

mod flex;
mod grid;
mod position;
#[cfg(test)]
mod tests;

use kuchikiki::NodeRef;

use super::document::{self, Document};
use super::images::ImageSizes;
use super::render::FontSet;
use super::style::{ComputedStyle, MediaCtx, StyleEngine};
use super::urlutil;

/// 测量宽度的"无穷大":超过任何真实换行阈值,用于 max-content 度量。
pub(crate) const MAX_CONTENT_WIDTH: f64 = 100_000.0;

/// 包含块高度未知标记:布局期父盒高度未定时使用;此时 `bottom` 锚定退化
/// 为 `top` 语义(见设计文档"已知边界")。
pub(crate) const UNKNOWN_CB_H: f64 = f64::MAX / 2.0;

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
    /// 绘制序(阶段三):`compute_with` 返回前按 z-index 稳定排序。
    pub z_index: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub lines: Vec<TextLine>,
    pub boxes: Vec<ElementBox>,
    /// 图像绘制指令(阶段三第三步):按绘制序存放,渲染层据此贴图。
    pub paints: Vec<PaintBox>,
    pub width: f64,
    pub height: f64,
}

/// 图像绘制指令(阶段三第三步)。
///
/// `src` 是图片缓存键(绝对 URL);`x/y/w/h` 是页坐标下的目标矩形;
/// `fit` 为 `object-fit` 关键字,决定图片在目标矩形内的填充方式。
#[derive(Debug, Clone)]
pub struct PaintBox {
    pub src: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub fit: String,
    pub kind: PaintKind,
    pub visible: bool,
    pub z_index: i64,
}

/// 图像类别:内容图(`<img>`)与背景图(`background-image`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintKind {
    Content,
    Background,
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
    compute_full(doc, viewport_width, font, media, &ImageSizes::default())
}

/// 完整布局入口(阶段三第三步):额外提供图片固有尺寸索引。
///
/// `images` 为已解码图片的 URL → 固有尺寸映射;`<img>` 按替换元素排版。
pub fn compute_full(
    doc: &Document,
    viewport_width: f64,
    font: &FontSet,
    media: &MediaCtx,
    images: &ImageSizes,
) -> Layout {
    let engine = StyleEngine::from_document(doc, *media);
    let mut ctx = LayoutCtx {
        engine: &engine,
        font,
        width: viewport_width.max(320.0),
        viewport_h: media.viewport_height.max(1.0),
        text_base: 8.0,
        lines: Vec::new(),
        boxes: Vec::new(),
        paints: Vec::new(),
        images,
        base_url: doc.base_url.clone(),
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
    // 初始包含块:整页坐标,高度未知(bottom 锚定退化)。
    let icb = (0.0, 0.0, ctx.width, UNKNOWN_CB_H);
    ctx.layout_children(&body, 8.0, content_width, &mut buf, 0, &body_style, icb);
    ctx.flush_inline(&mut buf, content_width);
    let height = ctx.y + 8.0;
    // 阶段三:按 z-index 稳定排序绘制序(同 z 保持 DOM/流内先后顺序)。
    ctx.boxes.sort_by_key(|b| b.z_index);
    // 阶段三第三步:图像绘制序(背景图先于内容图,同 z 保持先后)。
    ctx.paints.sort_by_key(|p| match p.kind {
        PaintKind::Background => 0,
        PaintKind::Content => 1,
    });
    ctx.paints.sort_by_key(|p| p.z_index);
    Layout {
        lines: ctx.lines,
        boxes: ctx.boxes,
        paints: ctx.paints,
        width: ctx.width,
        height,
    }
}

/// 局部布局片段:一次内容测量的产出,可整体平移后注入全局结果。
#[derive(Default)]
pub(crate) struct Fragment {
    pub height: f64,
    pub lines: Vec<TextLine>,
    pub boxes: Vec<ElementBox>,
    pub paints: Vec<PaintBox>,
}

/// 平移片段内全部几何。
pub(crate) fn translate(fragment: &mut Fragment, dx: f64, dy: f64) {
    for l in &mut fragment.lines {
        l.x += dx;
        l.y += dy;
    }
    for b in &mut fragment.boxes {
        b.x += dx;
        b.y += dy;
    }
    for p in &mut fragment.paints {
        p.x += dx;
        p.y += dy;
    }
}

/// 片段注入全局结果。
pub(crate) fn append(ctx: &mut LayoutCtx, fragment: Fragment) {
    ctx.lines.extend(fragment.lines);
    ctx.boxes.extend(fragment.boxes);
    ctx.paints.extend(fragment.paints);
}

/// 元素是否参与绘制(visibility/opacity)。
pub(crate) fn style_visible(style: &ComputedStyle) -> bool {
    style.visibility != "hidden" && style.visibility != "collapse" && style.opacity > 0.0
}

pub(crate) struct LayoutCtx<'a> {
    engine: &'a StyleEngine,
    font: &'a FontSet,
    width: f64,
    viewport_h: f64,
    /// 行内文本的 x 基准:块级流为 body 左边距 8.0,局部测量为 0.0。
    text_base: f64,
    lines: Vec<TextLine>,
    boxes: Vec<ElementBox>,
    paints: Vec<PaintBox>,
    /// 图片固有尺寸索引(阶段三第三步)。
    images: &'a ImageSizes,
    /// 页面 URL(相对图片地址解析基准)。
    base_url: String,
    y: f64,
}

impl<'a> LayoutCtx<'a> {
    /// 子元素计算样式(委托样式引擎)。
    pub(crate) fn style_for(&self, node: &NodeRef, parent: &ComputedStyle) -> ComputedStyle {
        self.engine.computed(node, parent)
    }

    pub(crate) fn font(&self) -> &FontSet {
        self.font
    }

    /// 图片固有尺寸索引(阶段三第三步)。
    pub(crate) fn images(&self) -> &ImageSizes {
        self.images
    }

    /// 页面 URL(相对地址解析基准)。
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 记录一条图像绘制指令。
    pub(crate) fn push_paint(&mut self, paint: PaintBox) {
        self.paints.push(paint);
    }

    /// 视口尺寸(页坐标宽度 / 视口高度),fixed 定位的包含块用。
    pub(crate) fn viewport(&self) -> (f64, f64) {
        (self.width, self.viewport_h)
    }

    pub(crate) fn cursor_y(&self) -> f64 {
        self.y
    }

    pub(crate) fn advance_y(&mut self, dy: f64) {
        self.y += dy;
    }

    pub(crate) fn push_box(&mut self, b: ElementBox) {
        self.boxes.push(b);
    }

    pub(crate) fn boxes_len(&self) -> usize {
        self.boxes.len()
    }

    /// 回填先占位盒的高度(flex 容器背景盒在内容布局前占位)。
    pub(crate) fn set_box_height(&mut self, idx: usize, h: f64) {
        if let Some(b) = self.boxes.get_mut(idx) {
            b.h = h;
        }
    }

    // ============ 阶段三第三步:替换元素(图片)============

    /// 解析 `<img>` 的绝对地址(相对地址按页面 URL 解析)。
    pub(crate) fn image_src(&self, node: &NodeRef) -> Option<String> {
        let raw = document::attr(node, "src")?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        urlutil::resolve(&self.base_url, raw)
    }

    /// 图片固有尺寸(解码后的自然宽高);未加载返回 `None`。
    pub(crate) fn natural_size(&self, node: &NodeRef) -> Option<(f64, f64)> {
        let src = self.image_src(node)?;
        self.images.natural(&src)
    }

    /// 图片是否已解码(未解码不产出绘制指令)。
    pub(crate) fn image_loaded(&self, src: &str) -> bool {
        self.images.natural(src).is_some()
    }

    /// 替换元素尺寸:CSS 宽高优先,其次 HTML `width`/`height` 属性,
    /// 最后落到固有尺寸;只给一个维度时按固有宽高比推导另一个。
    pub(crate) fn replaced_size(
        &self,
        node: &NodeRef,
        style: &ComputedStyle,
    ) -> (f64, f64, Option<String>) {
        let src = self.image_src(node);
        let natural = src.as_ref().and_then(|u| self.images.natural(u));
        let attr_w = document::attr(node, "width")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0);
        let attr_h = document::attr(node, "height")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0);
        let (nw, nh) = match natural {
            Some((w, h)) => (w, h),
            None => (attr_w.unwrap_or(0.0), attr_h.unwrap_or(0.0)),
        };
        let css_w = style.width;
        let css_h = style.height;
        let mut w = css_w.or(attr_w).unwrap_or(nw);
        let mut h = css_h.or(attr_h).unwrap_or(nh);
        let ratio_known = nw > 0.0 && nh > 0.0;
        if css_w.is_some() && css_h.is_none() {
            if ratio_known {
                h = w * nh / nw;
            }
        } else if css_h.is_some() && css_w.is_none() {
            if ratio_known {
                w = h * nw / nh;
            }
        } else if attr_w.is_some() && attr_h.is_none() && css_w.is_none() {
            if ratio_known {
                h = w * nh / nw;
            }
        } else if attr_h.is_some() && attr_w.is_none() && css_h.is_none() {
            if ratio_known {
                w = h * nw / nh;
            }
        }
        (w.max(0.0), h.max(0.0), src)
    }

    /// 背景图绘制指令(`background-image: url(...)`)。
    pub(crate) fn background_paint(
        &self,
        style: &ComputedStyle,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) -> Option<PaintBox> {
        let raw = style.background_image.as_ref()?;
        let src = urlutil::resolve(&self.base_url, raw)?;
        Some(PaintBox {
            src,
            x,
            y,
            w: w.max(0.0),
            h: h.max(0.0),
            fit: "cover".to_string(),
            kind: PaintKind::Background,
            visible: style_visible(style),
            z_index: style.pos.z_index,
        })
    }

    /// 在局部坐标(原点 0,0)下测量节点内容,返回片段;全局游标与已产出
    /// 几何零副作用。`content_width` 为内容盒宽度(调用方已扣除 padding)。
    /// 节点自身是 flex/grid 容器时走对应格式化上下文(嵌套容器的子树几何正确)。
    pub(crate) fn measure_content(
        &mut self,
        node: &NodeRef,
        content_width: f64,
        style: &ComputedStyle,
        depth: usize,
    ) -> Fragment {
        // 阶段三第三步:替换元素(<img>)没有子内容,直接按固有尺寸成盒。
        if document::tag_name(node).eq_ignore_ascii_case("img") {
            let (w, h, _src) = self.replaced_size(node, style);
            let selector = Document::stable_selector(node);
            return Fragment {
                height: h,
                lines: Vec::new(),
                boxes: vec![ElementBox {
                    selector,
                    tag: "img".to_string(),
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                    background: style.background,
                    visible: style_visible(style),
                    z_index: style.pos.z_index,
                }],
                paints: Vec::new(),
            };
        }
        let y0 = self.y;
        let lines0 = self.lines.len();
        let boxes0 = self.boxes.len();
        let paints0 = self.paints.len();
        let base0 = self.text_base;
        self.y = 0.0;
        self.text_base = 0.0;
        let height = if style.display == "flex" {
            // flex/grid 格式化上下文不推进全局游标,高度取返回值。
            flex::layout_content(
                self,
                node,
                0.0,
                content_width.max(1.0),
                style,
                depth,
                (0.0, 0.0, content_width.max(1.0), UNKNOWN_CB_H),
            )
        } else if style.display == "grid" {
            grid::layout_content(
                self,
                node,
                0.0,
                content_width.max(1.0),
                style,
                depth,
                (0.0, 0.0, content_width.max(1.0), UNKNOWN_CB_H),
            )
        } else {
            let mut buf = InlineBuf::default();
            let cb = (0.0, 0.0, content_width.max(1.0), UNKNOWN_CB_H);
            self.layout_children(node, 0.0, content_width, &mut buf, depth, style, cb);
            self.flush_inline(&mut buf, content_width);
            self.y
        };
        let lines = self.lines.split_off(lines0);
        let boxes = self.boxes.split_off(boxes0);
        let paints = self.paints.split_off(paints0);
        self.y = y0;
        self.text_base = base0;
        Fragment {
            height,
            lines,
            boxes,
            paints,
        }
    }

    fn layout_children(
        &mut self,
        parent: &NodeRef,
        x: f64,
        width: f64,
        buf: &mut InlineBuf,
        depth: usize,
        current: &ComputedStyle,
        cb: (f64, f64, f64, f64),
    ) {
        if depth > 60 {
            return;
        }
        // 阶段三:脱离流的定位元素延后到兄弟节点布置完再处理。
        let mut deferred: Vec<(NodeRef, ComputedStyle)> = Vec::new();
        for child in parent.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    buf.push_styled(&raw, current, self.text_base);
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
            if matches!(style.pos.position.as_str(), "absolute" | "fixed") {
                deferred.push((child.clone(), style));
                continue;
            }
            // 阶段三:Flex 格式化上下文。
            if style.display == "flex" {
                self.flush_inline(buf, width);
                flex::layout_container(self, &child, x, width, depth, &style, cb);
                continue;
            }
            // 阶段三第二步:Grid 格式化上下文。
            if style.display == "grid" {
                self.flush_inline(buf, width);
                grid::layout_container(self, &child, x, width, depth, &style, cb);
                continue;
            }
            // 阶段三第三步:替换元素(<img>)独占一行,避免与文本重叠。
            if document::tag_name(&child).eq_ignore_ascii_case("img") {
                self.flush_inline(buf, width);
                let (iw, ih, isrc) = self.replaced_size(&child, &style);
                let bx = x + style.margin_left;
                let by = self.y + style.margin_top;
                let visible = style_visible(&style);
                // relative/sticky 整体偏移(不影响占位)。
                let (dx, dy) = if matches!(style.pos.position.as_str(), "relative" | "sticky") {
                    (
                        style.pos.left.unwrap_or(0.0) - style.pos.right.unwrap_or(0.0),
                        style.pos.top.unwrap_or(0.0) - style.pos.bottom.unwrap_or(0.0),
                    )
                } else {
                    (0.0, 0.0)
                };
                self.boxes.push(ElementBox {
                    selector,
                    tag: "img".to_string(),
                    x: bx + dx,
                    y: by + dy,
                    w: iw,
                    h: ih,
                    background: style.background,
                    visible,
                    z_index: style.pos.z_index,
                });
                if let Some(src) = isrc.filter(|s| self.image_loaded(s)) {
                    self.paints.push(PaintBox {
                        src,
                        x: bx + dx + style.padding_left,
                        y: by + dy + style.padding_top,
                        w: (iw - style.padding_left - style.padding_right).max(0.0),
                        h: (ih - style.padding_top - style.padding_bottom).max(0.0),
                        fit: style.object_fit.clone(),
                        kind: PaintKind::Content,
                        visible,
                        z_index: style.pos.z_index,
                    });
                }
                self.y = by + ih + style.margin_bottom.max(0.0);
                continue;
            }
            if style.is_block() {
                self.flush_inline(buf, width);
                let mark_lines = self.lines.len();
                let mark_boxes = self.boxes.len();
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
                // 定位元素为后代 absolute 建立新的包含块(padding 盒);
                // 高度布局期未知,bottom 锚定退化(见 UNKNOWN_CB_H)。
                let child_cb = if style.pos.is_positioned() {
                    (
                        bx + style.padding_left,
                        start_y + style.padding_top,
                        (bw - style.padding_left - style.padding_right).max(0.0),
                        UNKNOWN_CB_H,
                    )
                } else {
                    cb
                };
                self.layout_children(
                    &child,
                    bx + style.padding_left,
                    (bw - style.padding_left - style.padding_right).max(40.0),
                    buf,
                    depth + 1,
                    &style,
                    child_cb,
                );
                self.flush_inline(buf, width);
                self.y += style.padding_bottom + style.margin_bottom.max(2.0);
                let box_h = (self.y - start_y).max(1.0);
                self.boxes.push(ElementBox {
                    selector,
                    tag: document::tag_name(&child),
                    x: bx,
                    y: start_y,
                    w: bw,
                    h: box_h,
                    background: style.background,
                    visible: style_visible(&style),
                    z_index: style.pos.z_index,
                });
                // 阶段三第三步:背景图(绘制在元素背景色之上、文字之下)。
                if let Some(paint) = self.background_paint(&style, bx, start_y, bw, box_h) {
                    self.push_paint(paint);
                }
                // 阶段三:relative/sticky 整体偏移(不影响占位)。
                if matches!(style.pos.position.as_str(), "relative" | "sticky") {
                    let dx = style.pos.left.unwrap_or(0.0) - style.pos.right.unwrap_or(0.0);
                    let dy = style.pos.top.unwrap_or(0.0) - style.pos.bottom.unwrap_or(0.0);
                    if dx != 0.0 || dy != 0.0 {
                        for l in &mut self.lines[mark_lines..] {
                            l.x += dx;
                            l.y += dy;
                        }
                        for b in &mut self.boxes[mark_boxes..] {
                            b.x += dx;
                            b.y += dy;
                        }
                    }
                }
                continue;
            }
            // 行内元素:递归收集文本,保留自身样式。
            self.collect_inline(&child, &style, buf, depth + 1, x);
        }
        if !deferred.is_empty() {
            position::layout_deferred(self, deferred, cb, depth);
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
                    buf.push_styled(&raw, style, self.text_base);
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
            // 阶段三第三步:行内替换元素(<img>):记录矩形并预留行高。
            if tag.eq_ignore_ascii_case("img") {
                let (iw, ih, isrc) = self.replaced_size(&child, &child_style);
                let ix = line_x + child_style.margin_left + child_style.padding_left;
                let iy = self.y;
                let visible = style_visible(&child_style);
                self.boxes.push(ElementBox {
                    selector: Document::stable_selector(&child),
                    tag: "img".to_string(),
                    x: ix,
                    y: iy,
                    w: iw,
                    h: ih,
                    background: child_style.background,
                    visible,
                    z_index: child_style.pos.z_index,
                });
                if let Some(src) = isrc.filter(|s| self.image_loaded(s)) {
                    self.paints.push(PaintBox {
                        src,
                        x: ix,
                        y: iy,
                        w: (iw - child_style.padding_left - child_style.padding_right).max(0.0),
                        h: (ih - child_style.padding_top - child_style.padding_bottom).max(0.0),
                        fit: child_style.object_fit.clone(),
                        kind: PaintKind::Content,
                        visible,
                        z_index: child_style.pos.z_index,
                    });
                }
                buf.reserve_height(ih);
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
                    visible: style_visible(&child_style),
                    z_index: child_style.pos.z_index,
                });
            }
            self.collect_inline(&child, &child_style, buf, depth + 1, line_x);
        }
    }

    fn flush_inline(&mut self, buf: &mut InlineBuf, max_w: f64) {
        let max_w = max_w.max(80.0);
        for run in buf.runs.drain(..) {
            // 阶段三第三步:占位 run(替换元素预留高度)只推进游标,不产出文本行。
            if run.text.is_empty() {
                self.y += run.line_height;
                continue;
            }
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
pub(crate) struct InlineBuf {
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
    pub(crate) fn push_styled(&mut self, text: &str, st: &ComputedStyle, x_base: f64) {
        self.chars += text.chars().count();
        let size = st.font_size;
        let bold = st.font_weight >= 600;
        let visible = st.visibility != "hidden" && st.visibility != "collapse" && st.opacity > 0.0;
        let x = x_base + st.margin_left + st.padding_left;
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

    /// 预留行高(阶段三第三步:行内替换元素撑开所在行)。
    ///
    /// 无可用 run 时插入一个空文本占位 run,`flush_inline` 只按其高度推进游标。
    pub(crate) fn reserve_height(&mut self, h: f64) {
        if h <= 0.0 {
            return;
        }
        if let Some(last) = self.runs.last_mut() {
            last.line_height = last.line_height.max(h);
            return;
        }
        self.runs.push(InlineRun {
            text: String::new(),
            x: 0.0,
            size: 8.0,
            color: [0, 0, 0],
            bold: false,
            line_height: h,
            visible: true,
            align: "left".to_string(),
            wrap: false,
            preserve: false,
        });
    }
}
