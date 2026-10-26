//! 布局引擎:块级流式布局 + 文本换行 + Flex 容器 + Grid 网格 + 定位体系(阶段三)。
//!
//! 样式全部来自 `style::StyleEngine` 的计算样式;本模块只负责几何计算。
//! 产出纯数据(`TextLine`/`ElementBox`/`PaintBox`),不持有 DOM 句柄,可跨 await 保存。
//! 目录分工:`mod.rs` 块级/行内主流,`flex.rs` Flex 格式化上下文,
//! `grid.rs` Grid 网格布局,`position.rs` 定位体系,`tests.rs` 布局单测。

mod flex;
mod float;
mod grid;
mod position;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use kuchikiki::NodeRef;

use super::document::{self, Document};
use super::images::ImageSizes;
use super::render::display::{self, BgFill, DisplayItem, DisplayKind};
use super::render::FontSet;
use super::style::{Affine, BgLayer, BorderColor, ComputedStyle, MediaCtx, StyleEngine, TextShadow};
use super::urlutil;

/// 测量宽度的"无穷大":超过任何真实换行阈值,用于 max-content 度量。
pub(crate) const MAX_CONTENT_WIDTH: f64 = 100_000.0;

/// 包含块高度未知标记:布局期父盒高度未定时使用;此时 `bottom` 锚定退化
/// 为 `top` 语义(见设计文档"已知边界")。
pub(crate) const UNKNOWN_CB_H: f64 = f64::MAX / 2.0;

/// 元素自身的「确定高度」:显式长度,或包含块高度确定时的百分比高度。
///
/// CSS 语义:百分比高度仅在包含块高度确定(非 auto、不依赖内容)时生效,
/// 否则按 auto 处理。根元素(html)的包含块是视口,调用方传 `Some(视口高)`。
pub(crate) fn definite_height(
    style: &ComputedStyle,
    parent_definite: Option<f64>,
) -> Option<f64> {
    if let Some(h) = style.height {
        return Some(h);
    }
    if let (Some(p), Some(ph)) = (style.height_percent, parent_definite) {
        return Some((p / 100.0 * ph).max(0.0));
    }
    None
}

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
    /// 边框绘制指令:绘制在元素背景之上、文字之下;不占布局几何。
    pub borders: Vec<BorderDraw>,
    /// 外阴影绘制指令(`box-shadow` 近似):绘制在元素背景之下。
    pub shadows: Vec<ShadowDraw>,
    /// 绘制显示列表(阶段六):布局期 DFS 按真实绘制序发射,渲染层唯一消费源;
    /// 为空时渲染层回退旧管线(平铺五表合成)。上五表继续服务几何寻址/inspect。
    pub display: Vec<DisplayItem>,
    pub width: f64,
    pub height: f64,
}

/// 边框绘制指令。`colors` 已在布局期把 currentColor 解析为文字色,
/// `transparent` 归一为 alpha=0。渲染层按四边实心矩形绘制(圆角不实现)。
#[derive(Debug, Clone)]
pub struct BorderDraw {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// top/right/bottom/left 线宽(px)。
    pub widths: [f64; 4],
    /// top/right/bottom/left 颜色(已解析)。
    pub colors: [([u8; 3], u8); 4],
    pub visible: bool,
    pub z_index: i64,
}

/// 外阴影绘制指令。`blur` 自阶段六起真实生效(渲染层三趟盒式模糊近似高斯);
/// blur=0 时退化为旧「偏移+扩散实心矩形」行为。
#[derive(Debug, Clone)]
pub struct ShadowDraw {
    /// 元素盒矩形(页坐标)。
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub dx: f64,
    pub dy: f64,
    pub blur: f64,
    pub spread: f64,
    pub color: ([u8; 3], u8),
    pub visible: bool,
    pub z_index: i64,
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
    /// 背景图在盒内的偏移(`background-position` 解析结果,像素)。
    pub off_x: f64,
    pub off_y: f64,
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

    /// 全量矩形映射(稳定选择器 → (x, y, w, h)):JS 桥几何属性注入用,
    /// 与 `rect_for` 同源。同选择器多实例时后者覆盖前者(快照按选择器匹配,语义一致)。
    pub fn rect_map(&self) -> HashMap<String, (f64, f64, f64, f64)> {
        self.boxes
            .iter()
            .map(|b| (b.selector.clone(), (b.x, b.y, b.w, b.h)))
            .collect()
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
        borders: Vec::new(),
        shadows: Vec::new(),
        display: Vec::new(),
        seq: 0,
        images,
        base_url: doc.base_url.clone(),
        y: 8.0,
        z_floor: 0,
        fixed_depth: 0,
    };
    let body = doc
        .root()
        .select_first("body")
        .ok()
        .map(|n| n.as_node().clone())
        .unwrap_or_else(|| doc.root().clone());
    let initial = engine.initial_style();
    let html_style = doc
        .first("html")
        .ok()
        .flatten()
        .map(|n| engine.computed(&n, &initial))
        .unwrap_or_else(|| initial.clone());
    let body_style = engine.computed(&body, &html_style);
    let mut buf = InlineBuf::default();
    let content_width = ctx.width - 16.0;
    // 初始包含块:整页坐标。高度传播链:视口 → html → body。
    // 百分比高度仅在包含块高度确定时生效(CSS),故 html/body 的确定高
    // 逐级解析后作为 ICB 高度;chain 任一环 auto 则 ICB 高度未知,
    // 后代百分比高度全部按 auto 退化(如 height:100% 页面不再塌陷)。
    let html_definite = definite_height(&html_style, Some(ctx.viewport_h));
    let body_definite = definite_height(&body_style, html_definite);
    let icb_h = body_definite
        .map(|h| (h - body_style.padding_top - body_style.padding_bottom).max(0.0))
        .unwrap_or(UNKNOWN_CB_H);
    let icb = (0.0, 0.0, ctx.width, icb_h);
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
    // 边框/阴影随盒序按 z-index 稳定排序,渲染层按序叠画。
    ctx.borders.sort_by_key(|b| b.z_index);
    ctx.shadows.sort_by_key(|s| s.z_index);
    // 阶段六:显示列表终排(z 分层 → DFS 先序 → 同元素通道序)。
    let mut display_items = std::mem::take(&mut ctx.display);
    display::sort_items(&mut display_items);
    Layout {
        lines: ctx.lines,
        boxes: ctx.boxes,
        paints: ctx.paints,
        borders: ctx.borders,
        shadows: ctx.shadows,
        display: display_items,
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
    pub borders: Vec<BorderDraw>,
    pub shadows: Vec<ShadowDraw>,
    /// 片段内发射的显示项(阶段六):随片段整体平移注入。
    pub display: Vec<DisplayItem>,
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
    for bd in &mut fragment.borders {
        bd.x += dx;
        bd.y += dy;
    }
    for s in &mut fragment.shadows {
        s.x += dx;
        s.y += dy;
    }
    for it in &mut fragment.display {
        it.translate(dx, dy);
    }
}

/// 片段注入全局结果。
pub(crate) fn append(ctx: &mut LayoutCtx, fragment: Fragment) {
    ctx.lines.extend(fragment.lines);
    ctx.boxes.extend(fragment.boxes);
    ctx.paints.extend(fragment.paints);
    ctx.borders.extend(fragment.borders);
    ctx.shadows.extend(fragment.shadows);
    ctx.display.extend(fragment.display);
}

/// 解析四边边框颜色:currentColor 代入文字色。
fn resolved_border_colors(style: &ComputedStyle) -> [([u8; 3], u8); 4] {
    let mut out = [([0u8; 3], 0u8); 4];
    for (i, c) in style.border_colors.iter().enumerate() {
        out[i] = match c {
            BorderColor::CurrentColor => (style.color, 255),
            BorderColor::Rgba(rgb, a) => (*rgb, *a),
        };
    }
    out
}

/// 盒的边框/阴影绘制指令入队(四边均无线宽时零开销)。
pub(crate) fn push_border_and_shadow(
    ctx: &mut LayoutCtx,
    style: &ComputedStyle,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) {
    let visible = style_visible(style);
    if style
        .border_widths
        .iter()
        .zip(style.border_drawn.iter())
        .any(|(w, d)| *w > 0.0 && *d)
    {
        ctx.borders.push(BorderDraw {
            x,
            y,
            w,
            h,
            widths: style.border_widths,
            colors: resolved_border_colors(style),
            visible,
            z_index: style.pos.z_index,
        });
    }
    for layer in &style.box_shadow {
        ctx.shadows.push(ShadowDraw {
            x,
            y,
            w,
            h,
            dx: layer.dx,
            dy: layer.dy,
            blur: layer.blur,
            spread: layer.spread,
            color: layer.color.unwrap_or((style.color, 255)),
            visible,
            z_index: style.pos.z_index,
        });
    }
}

/// 元素是否参与绘制(visibility/opacity)。
pub(crate) fn style_visible(style: &ComputedStyle) -> bool {
    style.visibility != "hidden" && style.visibility != "collapse" && style.opacity > 0.0
}

/// `position:relative|sticky` 的整体偏移(不改变占位)。
pub(crate) fn relative_offset(style: &ComputedStyle) -> (f64, f64) {
    if matches!(style.pos.position.as_str(), "relative" | "sticky") {
        (
            style.pos.left.unwrap_or(0.0) - style.pos.right.unwrap_or(0.0),
            style.pos.top.unwrap_or(0.0) - style.pos.bottom.unwrap_or(0.0),
        )
    } else {
        (0.0, 0.0)
    }
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
    borders: Vec<BorderDraw>,
    shadows: Vec<ShadowDraw>,
    /// 绘制显示列表(阶段六):DFS 发射,终排后进 `Layout::display`。
    display: Vec<DisplayItem>,
    /// DFS 先序序号(显示列表排序键之一)。
    seq: u64,
    /// 图片固有尺寸索引(阶段三第三步)。
    images: &'a ImageSizes,
    /// 页面 URL(相对图片地址解析基准)。
    base_url: String,
    y: f64,
    /// 祖先链 z-index 下限(层叠上下文近似):父级 z-index 创建层叠上下文后,
    /// 整个子树都应在兄弟节点之上,故子项取 `max(自身 z, 本下限)`。
    /// 不传播的话,父 z:9999 的整棵子树会被一个兄弟节点 z:1 压住(实测百度地图
    /// 的 #platform/#mask/瓦片层/控件层层叠全乱)。
    z_floor: i64,
    /// `position: fixed` 子树深度:>0 时发射的项带 `fixed` 标记,
    /// 光栅化侧据此跳过滚动偏移(固定控件不得随页面滚动平移)。
    fixed_depth: usize,
}

impl<'a> LayoutCtx<'a> {
    /// 分配一个 DFS 先序序号(元素视觉发射 / 文本行 flush 各取各的)。
    pub(crate) fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq - 1
    }

    /// 显示列表当前长度(组效果回补区间起点)。
    pub(crate) fn display_len(&self) -> usize {
        self.display.len()
    }

    /// 元素自身变换(页坐标,含 transform-origin);无 transform 返回 `None`。
    ///
    /// 变换只影响「画」,不改占位几何;子树项的祖先链合成由 `apply_group` 回补
    /// (元素盒高在子代布局完成后才定,故用回补而非变换栈)。
    fn own_transform(&self, style: &ComputedStyle, x: f64, y: f64, w: f64, h: f64) -> Option<Affine> {
        let t = style.transform.as_ref()?;
        Some(t.resolve_with_origin(x, y, w, h, style.transform_origin))
    }

    /// flex/grid 容器收尾(阶段六):组效果回补 → 自身视觉发射(含多层背景图)。
    /// 与块级路径的「回补先于自身发射」次序一致。
    pub(crate) fn finish_container(
        &mut self,
        seq: u64,
        mark: usize,
        style: &ComputedStyle,
        tag: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        self.apply_group(mark, style, x, y, w, h);
        self.emit_element(seq, style, tag, x, y, w, h);
    }

    /// 元素视觉发射器(阶段六):按 shadow→背景(色/渐变)→边框→canvas 占位
    /// 的同 seq 通道序发射自身绘制项。全部建盒点(块级/行内/替换元素/flex/
    /// grid/absolute)统一走这里,修复后三类路径边框与阴影静默丢失的缺口。
    /// 组效果(自身 opacity/overflow)不在此应用——由 `apply_group` 对子树回补。
    pub(crate) fn emit_element(
        &mut self,
        seq: u64,
        style: &ComputedStyle,
        tag: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        let visible = style_visible(style);
        if !visible {
            return;
        }
        let tf = self.own_transform(style, x, y, w, h);
        // 层叠上下文近似:自身 z 不得低于祖先链下限(见 z_floor 注释)。
        // 两者都先取到局部量:闭包会借用 self,而 push 需要 &mut self。
        let z = style.pos.z_index.max(self.z_floor);
        let is_fixed = self.fixed_depth > 0;
        let base = |kind| DisplayItem {
            kind,
            x,
            y,
            w,
            h,
            radius: style.border_radius,
            z,
            seq,
            opacity: style.opacity,
            clip: None,
            transform: tf,
            fixed: is_fixed,
        };
        for layer in &style.box_shadow {
            self.display.push(base(DisplayKind::Shadow {
                dx: layer.dx,
                dy: layer.dy,
                blur: layer.blur,
                spread: layer.spread,
                color: layer.color.unwrap_or((style.color, 255)),
            }));
        }
        // 背景:色在最下,其上为各背景层。CSS 首层在最上 → **逆序发射**
        // (先发射者先画、被后画者覆盖),故声明序的最后一层先画、首层最后画。
        if let Some(bg) = style.background {
            self.display.push(base(DisplayKind::Background {
                fill: BgFill::Solid(bg),
            }));
        }
        for layer in style.background_layers.iter().rev() {
            match layer {
                BgLayer::Gradient(g) => {
                    self.display.push(base(DisplayKind::Background {
                        fill: BgFill::Gradient(g.clone()),
                    }));
                }
                BgLayer::Image(u) => {
                    // 图片层需图片缓存定固有尺寸;未加载则跳过。
                    if let Some(paint) = self.bg_paint_for_url(style, u, x, y, w, h) {
                        self.push_paint(paint.clone());
                        self.emit_image(&paint, style.opacity, tf);
                    }
                }
            }
        }
        if style
            .border_widths
            .iter()
            .zip(style.border_drawn.iter())
            .any(|(bw, d)| *bw > 0.0 && *d)
        {
            self.display.push(base(DisplayKind::Border {
                widths: style.border_widths,
                colors: resolved_border_colors(style),
            }));
        }
        if tag.eq_ignore_ascii_case("canvas") && w >= 48.0 && h >= 28.0 {
            self.display.push(base(DisplayKind::CanvasNote {
                pale: style.background.is_none(),
            }));
        }
    }

    /// 图像显示项发射(与平铺 `PaintBox` 并行;`opacity` 为所属元素自身透明度,
    /// `transform` 为所属元素自身变换)。
    pub(crate) fn emit_image(&mut self, paint: &PaintBox, opacity: f64, transform: Option<Affine>) {
        if !paint.visible {
            return;
        }
        let seq = self.next_seq();
        self.display.push(DisplayItem {
            kind: DisplayKind::Image {
                src: paint.src.clone(),
                fit: paint.fit.clone(),
                off_x: paint.off_x,
                off_y: paint.off_y,
            },
            x: paint.x,
            y: paint.y,
            w: paint.w,
            h: paint.h,
            radius: [super::style::CornerRadius::Px(0.0); 4],
            z: paint.z_index,
            seq,
            opacity,
            clip: None,
            transform,
            fixed: self.fixed_depth > 0,
        });
    }

    /// 文本行显示项发射(flush 时调用,seq 取当下 → 文本按 flush 时机
    /// 与前后块级兄弟正确分层)。
    ///
    /// `transform` 为行内元素自身变换(行内文本在父级 flush,变换栈里没有它);
    /// `shadows` 为行内样式携带的 text-shadow 层。
    pub(crate) fn emit_text(
        &mut self,
        line: &TextLine,
        transform: Option<Affine>,
        shadows: &[TextShadow],
    ) {
        if !line.visible {
            return;
        }
        let seq = self.next_seq();
        // 阴影层(先声明在下):每层一个 TextShadow 项,排在本体之前绘制。
        // 颜色 alpha 折进项透明度(与组透明度回补乘法复合)。
        for sh in shadows {
            let (rgb, ca) = sh.color.unwrap_or((line.color, 255));
            self.display.push(DisplayItem {
                kind: DisplayKind::TextShadow {
                    text: line.text.clone(),
                    size: line.size,
                    color: rgb,
                    blur: sh.blur,
                },
                x: line.x + sh.dx,
                y: line.y + sh.dy,
                w: 0.0,
                h: 0.0,
                radius: [super::style::CornerRadius::Px(0.0); 4],
                z: 0,
                seq,
                opacity: ca as f64 / 255.0,
                clip: None,
                transform,
                fixed: self.fixed_depth > 0,
            });
        }
        self.display.push(DisplayItem {
            kind: DisplayKind::Text {
                text: line.text.clone(),
                size: line.size,
                color: line.color,
                bold: line.bold,
            },
            x: line.x,
            y: line.y,
            w: 0.0,
            h: 0.0,
            radius: [super::style::CornerRadius::Px(0.0); 4],
            z: 0,
            seq,
            opacity: 1.0,
            clip: None,
            transform,
            fixed: self.fixed_depth > 0,
        });
    }

    /// 组效果回补(阶段六):对子树显示项应用 `transform` 前乘、`opacity < 1`
    /// 乘法与 `overflow:hidden` padding 盒裁剪。`mark` 须在子树发射前记录;
    /// 自身视觉项在调用**之后**发射,天然免于自身裁剪/二次透明。
    pub(crate) fn apply_group(
        &mut self,
        mark: usize,
        style: &ComputedStyle,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        // CSS 2D transform(阶段六第二步):子树整体随元素变换(前乘合成)。
        // 元素自身视觉项在下方 `emit_element` 里取自身变换,祖先链由外层回补叠加。
        if let Some(m) = self.own_transform(style, x, y, w, h) {
            for it in &mut self.display[mark..] {
                it.compose_transform(m);
            }
        }
        if style.opacity < 1.0 {
            let op = style.opacity;
            for it in &mut self.display[mark..] {
                it.multiply_opacity(op);
            }
        }
        if style.overflow_clip {
            let rect = (
                x + style.padding_left,
                y + style.padding_top,
                (w - style.padding_left - style.padding_right).max(0.0),
                (h - style.padding_top - style.padding_bottom).max(0.0),
            );
            for it in &mut self.display[mark..] {
                it.intersect_clip(rect);
            }
        }
        // z-index 下传(层叠上下文近似):父级 z 高于子级时,整个子树都要抬到父级之上,
        // 否则子项会按自身 z 排到无关兄弟之下(地图的层叠面板即如此)。
        let zf = style.pos.z_index.max(self.z_floor);
        for it in &mut self.display[mark..] {
            if it.z < zf {
                it.z = zf;
            }
        }
    }

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

    /// 单个图片背景层(`background-image: url(...)`)的绘制指令。
    ///
    /// `background-size` 缺省为 `auto` → 按**固有尺寸**绘制(精灵图语义:
    /// 一张大图按 `background-position` 偏移取一块),而非拉伸铺满;
    /// `background-position` 按 CSS 公式解析成像素偏移:
    /// `offset = (盒尺寸 - 图尺寸) * 百分比`,长度值直接取用。
    pub(crate) fn bg_paint_for_url(
        &self,
        style: &ComputedStyle,
        raw: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) -> Option<PaintBox> {
        let src = urlutil::resolve(&self.base_url, raw)?;
        let (nw, nh) = self.images.natural(&src).unwrap_or((w.max(1.0), h.max(1.0)));
        let size = style.background_size.as_str();
        let (fit, draw_w, draw_h) = match size {
            "cover" => ("cover", w, h),
            "contain" => ("contain", w, h),
            _ => ("none", nw, nh),
        };
        let (off_x, off_y) = match style.background_position {
            Some(p) => (p.x.resolve(w, draw_w), p.y.resolve(h, draw_h)),
            None => (0.0, 0.0),
        };
        Some(PaintBox {
            src,
            x,
            y,
            w: w.max(0.0),
            h: h.max(0.0),
            fit: fit.to_string(),
            kind: PaintKind::Background,
            visible: style_visible(style),
            z_index: style.pos.z_index,
            off_x,
            off_y,
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
                borders: Vec::new(),
                shadows: Vec::new(),
                display: Vec::new(),
            };
        }
        let y0 = self.y;
        let lines0 = self.lines.len();
        let boxes0 = self.boxes.len();
        let paints0 = self.paints.len();
        let borders0 = self.borders.len();
        let shadows0 = self.shadows.len();
        let display0 = self.display.len();
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
        let borders = self.borders.split_off(borders0);
        let shadows = self.shadows.split_off(shadows0);
        let display_items = self.display.split_off(display0);
        self.y = y0;
        self.text_base = base0;
        Fragment {
            height,
            lines,
            boxes,
            paints,
            borders,
            shadows,
            display: display_items,
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
        // 阶段七:本容器内的浮动台账(浮动不跨块容器)。
        let mut floats = float::FloatState::default();
        // 阶段七:本容器内联上下文共享的行占用(行内浮动跨兄弟元素排同一行)。
        let mut inline_row = float::InlineRow::default();
        for child in parent.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    // 块级文本:变换由本块 `apply_group` 回补,run 不带变换;
                    // text-shadow 随 run 带到 flush(块级文本无行内样式承载)。
                    buf.push_styled(&raw, current, self.text_base, None, current.text_shadow.clone());
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
                // 阶段六:显示项发射(元素视觉 + 内容图)。
                let seq = self.next_seq();
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
                self.emit_element(seq, &style, "img", bx + dx, by + dy, iw, ih);
                if let Some(src) = isrc.filter(|s| self.image_loaded(s)) {
                    let paint = PaintBox {
                        src,
                        x: bx + dx + style.padding_left,
                        y: by + dy + style.padding_top,
                        w: (iw - style.padding_left - style.padding_right).max(0.0),
                        h: (ih - style.padding_top - style.padding_bottom).max(0.0),
                        fit: style.object_fit.clone(),
                        kind: PaintKind::Content,
                        visible,
                        z_index: style.pos.z_index,
                        off_x: 0.0,
                        off_y: 0.0,
                    };
                    let img_tf = self.own_transform(&style, bx + dx, by + dy, iw, ih);
                    self.emit_image(&paint, style.opacity, img_tf);
                    self.paints.push(paint);
                }
                self.y = by + ih + style.margin_bottom.max(0.0);
                continue;
            }
            // 阶段七:`clear` 在布置本盒前把游标推到相应浮动底边之下。
            if style.clear != "none" {
                if !floats.is_empty() {
                    let y = floats.cleared_y(&style.clear, self.y);
                    if y > self.y {
                        self.y = y;
                    }
                }
                if inline_row.has_floats() {
                    let y = inline_row.float_bottom();
                    if y > self.y {
                        self.y = y;
                    }
                    inline_row.deactivate();
                }
            }
            if style.is_block() {
                self.flush_inline(buf, width);
                // 阶段七:浮动盒求带/定位,或非浮动 auto 宽盒在浮动旁收窄。
                let placement = float::place(self, &child, &style, x, width, depth, &mut floats, &mut inline_row);
                let (x, width) = (placement.x, placement.width);
                let mark_lines = self.lines.len();
                let mark_boxes = self.boxes.len();
                let mark_borders = self.borders.len();
                let mark_shadows = self.shadows.len();
                // 阶段六:元素先序 seq 与子树显示项标记(组效果回补区间)。
                let seq = self.next_seq();
                let mark_display = self.display.len();
                self.y += style.margin_top;
                let bx = x + style.margin_left;
                // 盒宽(border box,不含 margin):auto 时撑满父内容宽减 margin;
                // 百分比按父内容宽求解;显式长度直接采用。
                let mut bw = (width - style.margin_left - style.margin_right).max(40.0);
                if let Some(p) = style.width_percent {
                    bw = (p / 100.0 * width).max(1.0);
                } else if let Some(w) = style.width {
                    bw = w.max(1.0);
                }
                if let Some(mw) = style.max_width {
                    bw = bw.min(mw.max(1.0));
                }
                let content_w = (bw - style.padding_left - style.padding_right).max(1.0);
                let start_y = self.y;
                self.y += style.padding_top;
                // 盒高先于子代布局求出:显式/百分比高度不依赖子代内容;
                // auto 高(占位 0)待子代布局完成后用内容高回填。
                let own_definite_h = style.height.is_some()
                    || (style.height_percent.is_some() && cb.3 != UNKNOWN_CB_H);
                let box_h = match style.height {
                    Some(h) => h.max(1.0),
                    None => match style.height_percent {
                        Some(p) if cb.3 != UNKNOWN_CB_H => ((p / 100.0) * cb.3).max(1.0),
                        _ => 0.0, // auto:子代布局后回填
                    },
                };
                // 子代包含块:定位元素为后代 absolute 建立新的包含块(padding 盒),
                // 高度布局期未知,bottom 锚定退化(见 UNKNOWN_CB_H);
                // 非定位子代的百分比高度以本盒内容高为包含块高 —— 仅当本盒高度
                // 确定时成立,auto 高必须传 UNKNOWN_CB_H(不得把祖父确定高透传,
                // 否则违背 CSS「百分比高度相对父内容高」的语义)。
                // 子代包含块高:本盒高度确定时(**无论是否定位**)即为内容盒高,
                // 后代 `height:100%` 可正常求解;只有 auto 高才未知。
                // 此前把「定位元素」一律当成未知高,`#app{height:720px}` 里的
                // `#MapHolder{height:100%}` 退化成内容高(8px),地图容器塌缩。
                let cb_h = if own_definite_h {
                    (box_h - style.padding_top - style.padding_bottom).max(0.0)
                } else {
                    UNKNOWN_CB_H
                };
                let child_cb = (
                    bx + style.padding_left,
                    start_y + style.padding_top,
                    (bw - style.padding_left - style.padding_right).max(0.0),
                    cb_h,
                );
                self.layout_children(
                    &child,
                    bx + style.padding_left,
                    content_w,
                    buf,
                    depth + 1,
                    &style,
                    child_cb,
                );
                self.flush_inline(buf, width);
                let content_h = (self.y - (start_y + style.padding_top)).max(1.0);
                let box_h = if box_h > 0.0 {
                    box_h
                } else {
                    (content_h + style.padding_top + style.padding_bottom).max(1.0)
                };
                self.y = start_y + box_h + style.margin_bottom.max(2.0);
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
                // 边框/外阴影绘制指令(不占几何;relative 偏移需同步平移)。
                push_border_and_shadow(self, &style, bx, start_y, bw, box_h);
                // 阶段三:relative/sticky 整体偏移(不影响占位):先平移子树几何,
                // 再以**偏移后**盒矩形做组回补与自身发射 —— 变换原点随之正确。
                let (rdx, rdy) = if matches!(style.pos.position.as_str(), "relative" | "sticky") {
                    (
                        style.pos.left.unwrap_or(0.0) - style.pos.right.unwrap_or(0.0),
                        style.pos.top.unwrap_or(0.0) - style.pos.bottom.unwrap_or(0.0),
                    )
                } else {
                    (0.0, 0.0)
                };
                if rdx != 0.0 || rdy != 0.0 {
                    for l in &mut self.lines[mark_lines..] {
                        l.x += rdx;
                        l.y += rdy;
                    }
                    for b in &mut self.boxes[mark_boxes..] {
                        b.x += rdx;
                        b.y += rdy;
                    }
                    for bd in &mut self.borders[mark_borders..] {
                        bd.x += rdx;
                        bd.y += rdy;
                    }
                    for s in &mut self.shadows[mark_shadows..] {
                        s.x += rdx;
                        s.y += rdy;
                    }
                    for it in &mut self.display[mark_display..] {
                        it.translate(rdx, rdy);
                    }
                }
                // 阶段六:子树组效果回补(先于自身视觉发射 → 自身不裁剪/不二次透明)。
                self.apply_group(mark_display, &style, bx + rdx, start_y + rdy, bw, box_h);
                let tag = document::tag_name(&child);
                self.emit_element(seq, &style, &tag, bx + rdx, start_y + rdy, bw, box_h);
                // 阶段七:浮动盒登记并恢复流式游标(非浮动为 no-op)。
                float::finish(self, placement, &mut floats, &mut inline_row);
                continue;
            }
            // 行内元素:递归收集文本,保留自身样式(祖先链无行内变换)。
            // 阶段七:浮动激活时行内内容下沉到浮动之下(不做文字环绕)。
            float::sink_inline_below_floats(self, &floats);
            if style.float != "none" {
                // 阶段七:块容器的直接行内子级带浮动时,同样走行内浮动带。
                self.layout_inline_float(&child, &style, buf, depth, x, width, &mut inline_row, None);
            } else {
                if inline_row.has_floats() {
                    let y = inline_row.float_bottom();
                    if y > self.y {
                        self.y = y;
                    }
                    inline_row.deactivate();
                }
                self.collect_inline(&child, &style, buf, depth + 1, x, width, None, &mut inline_row);
            }
        }
        // 阶段七:容器内容底收编浮动(clearfix 友好近似,auto 高不再塌缩)。
        float::include_floats_in_content_bottom(self, &floats);
        let inline_bottom = inline_row.content_bottom();
        if inline_bottom > self.y {
            self.y = inline_bottom;
        }
        if !deferred.is_empty() {
            position::layout_deferred(self, deferred, cb, depth);
        }
    }

    /// 行内级元素是否需要生成近似矩形(供 inspect/坐标点击/截图裁剪寻址)。
    ///
    /// 行内元素在 CSS 中生成行内盒而非块级盒,但 Agent 需要几何来定位与点击,
    /// 故对"可寻址"的行内元素各生成一个近似矩形:交互元素、`inline-block`、
    /// 以及带显式尺寸的行内元素。
    fn inline_box_worthy(&self, node: &NodeRef, style: &ComputedStyle) -> bool {
        let tag = document::tag_name(node);
        if matches!(
            tag.as_str(),
            "a" | "button" | "input" | "select" | "textarea" | "label"
        ) {
            return true;
        }
        style.display == "inline-block"
            || style.width.is_some()
            || style.height.is_some()
            || style.width_percent.is_some()
    }

    /// 行内盒的近似尺寸:显式 CSS 尺寸优先,其次按文本量估算。
    ///
    /// 百分比宽度按包含块宽度(`avail_w`)求解;百分比高度在父高未知时按 `auto`
    /// 处理(CSS 中行内元素的百分比高度同样不生效)。
    fn inline_box_size(&self, node: &NodeRef, style: &ComputedStyle, avail_w: f64) -> (f64, f64) {
        let text = node.text_contents();
        let text_w = self.font.text_width(&text, style.font_size);
        let w = if let Some(p) = style.width_percent {
            (p / 100.0 * avail_w).max(1.0)
        } else {
            style.width.unwrap_or_else(|| text_w.max(40.0)).max(1.0)
        };
        let h = style
            .height
            .unwrap_or_else(|| (style.font_size * 1.45).max(16.0))
            .max(1.0);
        (w, h)
    }

    /// 在行内表单控件盒内绘制 value/placeholder 文本。
    ///
    /// 只发射文本显示项,不推进游标、不改变既有盒几何;占位符用 UA 灰
    /// (#757575),密码按 `•` 掩码,超宽按盒内可用宽截断(与真实输入框裁剪一致)。
    #[allow(clippy::too_many_arguments)]
    fn emit_form_control_text(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        tag: &str,
        bx: f64,
        by: f64,
        w: f64,
        h: f64,
        tf: Option<Affine>,
    ) {
        if tag != "input" && tag != "textarea" {
            return;
        }
        let ty = document::attr(node, "type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let text_like = matches!(
            ty.as_str(),
            "" | "text" | "search" | "url" | "tel" | "email" | "password" | "number"
        );
        let button_like = matches!(ty.as_str(), "submit" | "button" | "reset");
        if tag == "input" && !text_like && !button_like {
            return;
        }
        let value = document::attr(node, "value").unwrap_or_default();
        let (mut text, color) = if !value.trim().is_empty() {
            (value, style.color)
        } else if tag == "textarea" {
            let t = node.text_contents();
            if t.trim().is_empty() {
                (document::attr(node, "placeholder").unwrap_or_default(), [117, 117, 117])
            } else {
                (t, style.color)
            }
        } else if text_like {
            (document::attr(node, "placeholder").unwrap_or_default(), [117, 117, 117])
        } else {
            return;
        };
        if text.trim().is_empty() {
            return;
        }
        if ty == "password" {
            text = "•".repeat(text.chars().count());
        }
        let size = style.font_size.max(8.0);
        let max_w = (w - 6.0).max(0.0);
        if max_w < size {
            return;
        }
        let mut clipped = String::new();
        let mut used = 0.0;
        for ch in text.chars() {
            let cw = self.font.char_width(ch, size);
            if used + cw > max_w {
                break;
            }
            used += cw;
            clipped.push(ch);
        }
        if clipped.is_empty() {
            return;
        }
        let line_height = size * 1.35;
        let line = TextLine {
            text: clipped,
            x: bx + 3.0,
            y: by + ((h - line_height) / 2.0).max(1.0),
            size,
            color,
            bold: style.font_weight >= 600,
            visible: style_visible(style),
        };
        self.emit_text(&line, tf, &[]);
        self.lines.push(line);
    }

    fn collect_inline(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        buf: &mut InlineBuf,
        depth: usize,
        line_x: f64,
        avail_w: f64,
        inherited_tf: Option<Affine>,
        row: &mut float::InlineRow,
    ) {
        if depth > 40 || buf.chars > 200_000 {
            return;
        }
        // 行内元素自身变换(与祖先链合成):行内文本在父级 flush,变换栈里没有它,
        // 故把变换随 run 带到 flush 时应用(块级文本由 apply_group 回补)。
        let mut tf = inherited_tf;
        // 行内元素自身建盒。此前只有"块级盒的直接行内子节点"才拿到盒子,
        // 导致 `<div><input></div>` 这类结构完全取不到几何(inspect 的 rect 恒 0×0)。
        let own_box = self.inline_box_worthy(node, style);
        let mut box_x = line_x;
        let mut box_w = avail_w;
        if own_box {
            let (w, h) = self.inline_box_size(node, style, avail_w);
            let (dx, dy) = relative_offset(style);
            let bx = line_x + style.margin_left + style.padding_left + dx;
            let by = self.y + dy;
            tf = self.emit_inline_box(node, style, bx, by, w, h, tf);
            // 阶段七:登记当前行占用,后续行内浮动可排在它的右侧剩余带。
            row.register(bx, by, w, h, line_x + avail_w);
            box_x = bx;
            box_w = w;
        }
        // 行内块/控件盒为子代建立独立行内上下文(浮动不越出该盒),子代几何
        // 也相对自身盒求解(此前一律沿用祖先 line_x/avail_w)。
        let mut inner_row = float::InlineRow::default();
        let child_row: &mut float::InlineRow = if own_box { &mut inner_row } else { row };
        self.collect_inline_children(node, style, buf, depth, box_x, box_w, child_row, tf);
        if own_box {
            let bottom = inner_row.content_bottom();
            if bottom > self.y {
                self.y = bottom;
            }
        }
    }

    /// `collect_inline` 的子代遍历(行内浮动入口复用同一套收集逻辑)。
    #[allow(clippy::too_many_arguments)]
    fn collect_inline_children(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        buf: &mut InlineBuf,
        depth: usize,
        line_x: f64,
        avail_w: f64,
        row: &mut float::InlineRow,
        tf: Option<Affine>,
    ) {
        for child in node.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    buf.push_styled(&raw, style, self.text_base, tf, style.text_shadow.clone());
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
                // 阶段六:显示项发射(元素视觉 + 内容图)。
                let seq = self.next_seq();
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
                self.emit_element(seq, &child_style, "img", ix, iy, iw, ih);
                if let Some(src) = isrc.filter(|s| self.image_loaded(s)) {
                    let paint = PaintBox {
                        src,
                        x: ix,
                        y: iy,
                        w: (iw - child_style.padding_left - child_style.padding_right).max(0.0),
                        h: (ih - child_style.padding_top - child_style.padding_bottom).max(0.0),
                        fit: child_style.object_fit.clone(),
                        kind: PaintKind::Content,
                        visible,
                        z_index: child_style.pos.z_index,
                        off_x: 0.0,
                        off_y: 0.0,
                    };
                    let img_tf = match (self.own_transform(&child_style, ix, iy, iw, ih), tf) {
                        (Some(m), Some(anc)) => Some(m.compose(&anc)),
                        (Some(m), None) => Some(m),
                        (None, anc) => anc,
                    };
                    self.emit_image(&paint, child_style.opacity, img_tf);
                    self.paints.push(paint);
                }
                buf.reserve_height(ih);
                continue;
            }
            // 阶段七:行内浮动排在当前行剩余带(输入框+按钮/图标+文字经典浮动行)。
            if child_style.float != "none" {
                self.layout_inline_float(&child, &child_style, buf, depth, line_x, avail_w, row, tf);
                continue;
            }
            // 非浮动行内内容:当前行已有浮动时下沉到浮动之下(不做文字环绕)。
            if row.has_floats() {
                let y = row.float_bottom();
                if y > self.y {
                    self.y = y;
                }
                row.deactivate();
            }
            // 交互类行内元素的矩形由 `collect_inline` 入口统一建盒(元素自身),
            // 此处只负责继续收集其子树文本。
            self.collect_inline(&child, &child_style, buf, depth + 1, line_x, avail_w, tf, row);
        }
    }

    /// 行内盒统一发射(自身视觉 + 边框/阴影 + 表单控件文本),返回合成后的变换。
    fn emit_inline_box(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        bx: f64,
        by: f64,
        w: f64,
        h: f64,
        inherited_tf: Option<Affine>,
    ) -> Option<Affine> {
        let mut tf = inherited_tf;
        if let Some(m) = self.own_transform(style, bx, by, w, h) {
            tf = Some(match tf {
                Some(anc) => m.compose(&anc),
                None => m,
            });
        }
        // 阶段六:显示项发射(行内元素视觉,边框/阴影/背景/渐变全量)。
        let seq = self.next_seq();
        self.boxes.push(ElementBox {
            selector: Document::stable_selector(node),
            tag: document::tag_name(node),
            x: bx,
            y: by,
            w,
            h,
            background: style.background,
            visible: style_visible(style),
            z_index: style.pos.z_index,
        });
        // 输入框/按钮等交互元素的边框与外阴影(绘制不占几何)。
        push_border_and_shadow(self, style, bx, by, w, h);
        let tag = document::tag_name(node);
        self.emit_element(seq, style, &tag, bx, by, w, h);
        // 阶段七:表单控件的 value/placeholder 文本(此前只画背景+边框,
        // 输入框在截图里是没有任何提示文字的灰盒)。
        self.emit_form_control_text(node, style, &tag, bx, by, w, h, tf);
        tf
    }

    /// 行内浮动:排在当前行已占用内容的剩余带,带宽不足换行;
    /// 子代在浮动盒内按独立行内上下文排布。
    #[allow(clippy::too_many_arguments)]
    fn layout_inline_float(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        buf: &mut InlineBuf,
        depth: usize,
        line_x: f64,
        avail_w: f64,
        row: &mut float::InlineRow,
        tf: Option<Affine>,
    ) {
        let (w, h) = self.inline_box_size(node, style, avail_w);
        let (bx, by) = row.place_float(style, line_x, line_x + avail_w, self.y, w, h);
        self.y = by;
        let tf = self.emit_inline_box(node, style, bx, by, w, h, tf);
        let mut inner = float::InlineRow::default();
        self.collect_inline_children(node, style, buf, depth + 1, bx, w, &mut inner, tf);
        row.note_bottom(inner.content_bottom());
        // 游标留在本行,后续浮动继续排在同一行剩余带。
        self.y = by;
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
        let line = TextLine {
            text: content,
            x: x + offset,
            y: self.y,
            size: run.size,
            color: run.color,
            bold: run.bold,
            visible: run.visible,
        };
        // 阶段六:文本显示项(flush 时取新 seq → 与前后块级兄弟正确分层)。
        self.emit_text(&line, run.transform, &run.text_shadow);
        self.lines.push(line);
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
    /// 行内元素自身变换(块级文本为 `None`,由 `apply_group` 回补)。
    transform: Option<Affine>,
    /// 行内样式携带的 text-shadow 层。
    text_shadow: Vec<TextShadow>,
}

impl InlineBuf {
    pub(crate) fn push_styled(
        &mut self,
        text: &str,
        st: &ComputedStyle,
        x_base: f64,
        transform: Option<Affine>,
        text_shadow: Vec<TextShadow>,
    ) {
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
                && last.transform == transform
                && last.text_shadow == text_shadow
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
            transform,
            text_shadow,
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
            transform: None,
            text_shadow: Vec::new(),
        });
    }
}
