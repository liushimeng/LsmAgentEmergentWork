//! 布局引擎:块级流式布局 + 文本换行 + Flex 容器 + Grid 网格 + 定位体系(阶段三)。
//!
//! 样式全部来自 `style::StyleEngine` 的计算样式;本模块只负责几何计算。
//! 产出纯数据(`TextLine`/`ElementBox`/`PaintBox`),不持有 DOM 句柄,可跨 await 保存。
//! 目录分工:`mod.rs` 块级/行内主流,`flex.rs` Flex 格式化上下文,
//! `grid.rs` Grid 网格布局,`position.rs` 定位体系,`tests.rs` 布局单测。

mod flex;
mod float;
mod inline;
mod measure;
mod grid;
mod position;
#[cfg(test)]
mod position_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use inline::InlineBuf;
pub(crate) use measure::{append, translate, Fragment};

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
    /// 脱离流(position:absolute/fixed,阶段七第 174 轮):渲染照常发射,
    /// 但**不参与任何祖先的内在尺寸聚合** —— CSS 语义里 abs 元素对
    /// shrink-to-fit/max-content 高宽贡献恒为零。此前 flex 条目测量把
    /// 脱流徽标(right:30px + 无宽 svg)在 MAX_CONTENT_WIDTH 语境下求出的
    /// 盒坐标当成内容宽,后续条目被 flex 收缩压到 ~1px(猎聘「登录/注册」
    /// 入口 0×0 根因)。
    pub out_of_flow: bool,
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
        lines: Vec::new(),
        boxes: Vec::new(),
        paints: Vec::new(),
        borders: Vec::new(),
        shadows: Vec::new(),
        display: Vec::new(),
        seq: 0,
        images,
        base_url: doc.base_url.clone(),
        y: 0.0,
        z_floor: 0,
        fixed_depth: 0,
        pending_margin_bottom: 0.0,
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
    // 第 175 轮:初始包含块消费**真实** body margin(UA 默认 8px 仍来自样式层
    // 默认值,站点 reset margin:0 后布局应全幅)。此前硬编码 8/16,与
    // ComputedStyle 脱钩 —— 百度地图 #MapHolder 被逼到 (8,27,1904×1080),
    // 地图引擎按错误几何初始化,瓦片栅格错位且下半屏无瓦片。
    let body_margin_top = body_style.margin_top.max(0.0);
    let body_margin_left = body_style.margin_left.max(0.0);
    let body_margin_right = body_style.margin_right.max(0.0);
    let body_margin_bottom = body_style.margin_bottom.max(0.0);
    ctx.y = body_margin_top;
    let mut buf = InlineBuf::default();
    let content_width = (ctx.width - body_margin_left - body_margin_right).max(40.0);
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
    ctx.layout_children(&body, body_margin_left, content_width, &mut buf, 0, &body_style, icb);
    ctx.flush_inline(&mut buf, content_width);
    // body 末子块穿出的 margin_bottom 与 body 自身边距折叠(CSS 2.1 8.3.1);
    // body 有 padding-bottom/下边框/overflow 时收编进内容高。
    if contains_child_margins(&body_style) {
        ctx.y += ctx.pending_margin_bottom;
        ctx.pending_margin_bottom = 0.0;
    }
    let escaped = std::mem::take(&mut ctx.pending_margin_bottom);
    let height = ctx.y + collapse_margin(escaped, body_margin_bottom);
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
/// 替换类标签:`<img>` 与 `<svg>`(后者按 viewBox/属性取固有尺寸)。
pub(crate) fn is_replaced_tag(node: &NodeRef) -> bool {
    matches!(
        document::tag_name(node).to_ascii_lowercase().as_str(),
        // iframe(第 174 轮):按替换元素参与布局 —— 无 CSS/属性尺寸时取
        // 浏览器建议缺省 300×150,而非塌缩 0×0(猎聘页脚 iframe 实测 0×0,
        // 坐标点击与 inspect 全部失真)。
        "img" | "svg" | "iframe"
    )
}

/// 行内子树是否含**结构性内容**(非空文本或可渲染元素)。
///
/// 第 175 轮:纯结构判定(不查 CSS),供 `inline_box_size` 把空行内元素
/// 量成 0×0 —— Chrome 语义:空行内元素(如 `<a name=x></a>`、纯空白、
/// 仅含空子元素)不产生行盒,高度 0。此前一律给 16px+ 幻影高,
/// 百度地图 body 首子 DIV(仅含空锚点)被量出 1904×17 幽灵条带,
/// 把 #app 与地图容器整体下推。
pub(crate) fn has_structural_inline_content(node: &NodeRef, depth: usize) -> bool {
    if depth > 6 {
        // 防御性:过深按有内容处理,避免极端嵌套误判成空。
        return true;
    }
    for child in node.children() {
        if let Some(t) = child.as_text() {
            if !t.borrow().trim().is_empty() {
                return true;
            }
            continue;
        }
        if child.as_element().is_none() {
            continue;
        }
        let tag = document::tag_name(&child).to_ascii_lowercase();
        // 替换/控件/换行类标签一律算内容;纯结构标签递归下钻。
        if matches!(
            tag.as_str(),
            "img" | "svg" | "iframe" | "canvas" | "video" | "audio" | "object" | "embed"
                | "input" | "select" | "textarea" | "button" | "br" | "hr"
        ) {
            return true;
        }
        if has_structural_inline_content(&child, depth + 1) {
            return true;
        }
    }
    false
}

/// `<svg>` 固有尺寸:`width`/`height` 属性优先,缺省回落 `viewBox`
/// 的宽高,都无则按 CSS 替换元素建议缺省 300×150。
pub(crate) fn svg_natural_size(node: &NodeRef) -> (f64, f64) {
    let attr_num = |name: &str| {
        document::attr(node, name)
            .and_then(|v| {
                let v = v.trim();
                v.strip_suffix("px").unwrap_or(v).parse::<f64>().ok()
            })
            .filter(|v| *v > 0.0)
    };
    if let (Some(w), Some(h)) = (attr_num("width"), attr_num("height")) {
        return (w, h);
    }
    if let Some(vb) = document::attr(node, "viewBox")
        .map(|v| v.trim().split_whitespace().filter_map(|t| t.parse::<f64>().ok()).collect::<Vec<_>>())
        .filter(|v| v.len() == 4 && v[2] > 0.0 && v[3] > 0.0)
    {
        let (w, h) = (vb[2], vb[3]);
        return (
            attr_num("width").unwrap_or(w),
            attr_num("height").unwrap_or(h),
        );
    }
    (attr_num("width").unwrap_or(300.0), attr_num("height").unwrap_or(150.0))
}

pub(crate) fn style_visible(style: &ComputedStyle) -> bool {
    style.visibility != "hidden" && style.visibility != "collapse" && style.opacity > 0.0
}

/// CSS 高度钳序:先 max-height,再 min-height(冲突时 min 胜出)。
pub(crate) fn constrain_height(style: &ComputedStyle, height: f64) -> f64 {
    let capped = style.max_height.map(|max| height.min(max)).unwrap_or(height);
    style.min_height.map(|min| capped.max(min)).unwrap_or(capped)
}

/// `position:relative|sticky` 的整体偏移(不改变占位)。
pub(crate) fn relative_offset(
    style: &ComputedStyle,
    containing_width: f64,
    containing_height: f64,
) -> (f64, f64) {
    if matches!(style.pos.position.as_str(), "relative" | "sticky") {
        style.pos.relative_offsets(containing_width, containing_height)
    } else {
        (0.0, 0.0)
    }
}

/// 相邻兄弟外边距折叠(CSS 2.1 8.3.1 简化):两者皆非负取 max;任一为负直接相加
/// (负边距不折叠,按叠加语义近似)。
fn collapse_margin(a: f64, b: f64) -> f64 {
    if a >= 0.0 && b >= 0.0 {
        a.max(b)
    } else {
        a + b
    }
}

/// 子块 `margin_bottom` 是否折叠穿出本盒(CSS 2.1 8.3.1):
/// 有 padding-bottom / 下边框 / overflow 裁剪(建 BFC)时收编进内容高,
/// 否则穿出本盒、与本盒自身 margin 折叠(经典 `<section><p>` 段距语义)。
fn contains_child_margins(style: &ComputedStyle) -> bool {
    style.padding_bottom > 0.0
        || (style.border_drawn[2] && style.border_widths[2] > 0.0)
        || style.overflow_clip
}

pub(crate) struct LayoutCtx<'a> {
    engine: &'a StyleEngine,
    font: &'a FontSet,
    width: f64,
    viewport_h: f64,
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
    /// 相邻兄弟外边距折叠暂存(CSS 2.1 8.3.1 简化):前一个块级兄弟的
    /// `margin_bottom` 尚未落地,与下一个兄弟的 `margin_top` 取折叠值;
    /// 仅参与块级流,行内行盒/替换元素出现时收编,浮动与绝对定位子树不消费。
    pending_margin_bottom: f64,
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
    pub(crate) fn own_transform(&self, style: &ComputedStyle, x: f64, y: f64, w: f64, h: f64) -> Option<Affine> {
        let t = style.transform.as_ref()?;
        Some(t.resolve_with_origin(x, y, w, h, style.transform_origin))
    }

    /// flex/grid 容器收尾(阶段六):组效果回补 → 自身视觉发射(含多层背景图)。
    /// 与块级路径的「回补先于自身发射」次序一致。
    pub(crate) fn finish_container(
        &mut self,
        seq: u64,
        mark: usize,
        node: &NodeRef,
        style: &ComputedStyle,
        tag: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        self.apply_group(mark, style, x, y, w, h);
        self.emit_element(seq, style, tag, x, y, w, h);
        self.emit_canvas_pixels(node, style, seq, x, y, w, h);
    }

    /// Canvas 已有真实 PNG 时覆盖能力占位;未产生像素时仍保留 CanvasNote。
    fn emit_canvas_pixels(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        seq: u64,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        if !document::tag_name(node).eq_ignore_ascii_case("canvas") || !style_visible(style) {
            return;
        }
        if document::attr(node, "data-lsm-canvas").is_none() {
            return;
        }
        let src = super::images::canvas_key(&Document::stable_selector(node));
        self.display.push(DisplayItem {
            kind: DisplayKind::Image { src, fit: "fill".to_string(), off_x: 0.0, off_y: 0.0 },
            x,
            y,
            w,
            h,
            radius: style.border_radius,
            z: style.pos.z_index.max(self.z_floor),
            seq,
            opacity: style.opacity,
            clip: None,
            transform: self.own_transform(style, x, y, w, h),
            fixed: self.fixed_depth > 0,
        });
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
    ///
    /// `avail_w` 为包含块内容宽,百分比宽高在此求解(样式层不再按视口
    /// 宽预解 —— `width:100%` 的 logo 曾被解成视口绝对宽,按方图比例
    /// 推高后巨图撑爆卡片容器)。`avail_h` 为包含块内容高(未知传 None,
    /// 百分比高度按 auto 处理)。
    ///
    /// `<svg>` 同按替换元素处理:固有尺寸取 `width`/`height` 属性,
    /// 缺省回落 `viewBox`;既无属性也无 viewBox 时按 300×150(CSS 替换
    /// 元素缺省建议尺寸)。此前 svg 当普通块级元素,`display:block` +
    /// auto 宽在 max-content 测量语境下撑满测量宽(10 万 px),行内
    /// 图标把整个 tab 条目撑爆(猎聘行业分类 tab 竖排+负坐标根因)。
    pub(crate) fn replaced_size(
        &self,
        node: &NodeRef,
        style: &ComputedStyle,
        avail_w: f64,
        avail_h: Option<f64>,
    ) -> (f64, f64, Option<String>) {
        let tag = document::tag_name(node);
        let is_svg = tag.eq_ignore_ascii_case("svg");
        let src = if is_svg { None } else { self.image_src(node) };
        let natural = src.as_ref().and_then(|u| self.images.natural(u)).or_else(|| {
            if is_svg {
                Some(svg_natural_size(node))
            } else if tag.eq_ignore_ascii_case("iframe") {
                // CSS 替换元素建议缺省尺寸(真实浏览器对无尺寸 iframe 一致)。
                Some((300.0, 150.0))
            } else {
                None
            }
        });
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
        // 百分比宽:相对包含块内容宽;百分比高:包含块高已知才生效。
        // avail 无效(≤0,如 shrink-to-fit 的 max-content 估算没有容器宽
        // 语义)时百分比按 auto 处理,落回固有尺寸。
        let pct_w = style
            .width_percent
            .filter(|_| avail_w > 0.0)
            .map(|p| (p / 100.0 * avail_w).max(1.0));
        let pct_h = style
            .height_percent
            .zip(avail_h.filter(|h| *h > 0.0))
            .map(|(p, ah)| (p / 100.0 * ah).max(1.0));
        let css_w = style.width.or(pct_w);
        let css_h = style.height.or(pct_h);
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
                    // x 基准 = 本容器 margin-box x(文本起点 = 容器内容盒 x)。
                    buf.push_styled(
                        &raw,
                        current,
                        x - current.margin_left - current.padding_left,
                        None,
                        current.text_shadow.clone(),
                    );
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
                // flex/grid 容器边距不参与兄弟折叠(近似:收编前置 pending,
                // 保持求和语义),容器内部亦不得消费外层 pending。
                self.y += self.pending_margin_bottom;
                self.pending_margin_bottom = 0.0;
                self.flush_inline(buf, width);
                flex::layout_container(self, &child, x, width, depth, &style, cb);
                continue;
            }
            // 阶段三第二步:Grid 格式化上下文。
            if style.display == "grid" {
                self.y += self.pending_margin_bottom;
                self.pending_margin_bottom = 0.0;
                self.flush_inline(buf, width);
                grid::layout_container(self, &child, x, width, depth, &style, cb);
                continue;
            }
            // 阶段三第三步:替换元素(<img>/<svg>)独占一行,避免与文本重叠。
            if is_replaced_tag(&child) {
                self.flush_inline(buf, width);
                let avail_h = (cb.3 != UNKNOWN_CB_H).then_some(cb.3);
                let (iw, ih, isrc) = self.replaced_size(&child, &style, width, avail_h);
                let bx = x + style.margin_left;
                // 与块级兄弟同样参与外边距折叠。
                let gap = collapse_margin(self.pending_margin_bottom, style.margin_top);
                self.pending_margin_bottom = 0.0;
                let by = self.y + gap;
                let visible = style_visible(&style);
                // relative/sticky 整体偏移(不影响占位)。
                let (dx, dy) = if matches!(style.pos.position.as_str(), "relative" | "sticky") {
                    relative_offset(&style, width, if cb.3 == UNKNOWN_CB_H { 0.0 } else { cb.3 })
                } else {
                    (0.0, 0.0)
                };
                // 阶段六:显示项发射(元素视觉 + 内容图)。
                let seq = self.next_seq();
                self.boxes.push(ElementBox {
                    selector,
                    // 真实标签(img/svg/iframe,第 174 轮 iframe 亦为替换元素)。
                    tag: document::tag_name(&child).to_ascii_lowercase(),
                    x: bx + dx,
                    y: by + dy,
                    w: iw,
                    h: ih,
                    background: style.background,
                    visible,
                    z_index: style.pos.z_index,
                    out_of_flow: false,
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
                self.y = by + ih;
                self.pending_margin_bottom = style.margin_bottom.max(0.0);
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
                // 浮动盒不参与外边距折叠:整支路存还 pending(流内游标亦由
                // float::finish 收还,浮动 margin 不影响后续兄弟的折叠间隙)。
                let float_saved_pending = if style.float != "none" {
                    std::mem::take(&mut self.pending_margin_bottom)
                } else {
                    0.0
                };
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
                // 相邻兄弟外边距折叠(CSS 2.1 8.3.1):间隙取折叠值而非求和。
                let gap = collapse_margin(self.pending_margin_bottom, style.margin_top);
                self.pending_margin_bottom = 0.0;
                self.y += gap;
                let bx = x + style.margin_left;
                // 盒宽(border box,不含 margin):auto 时撑满父内容宽减 margin;
                // 百分比按父内容宽求解;显式长度直接采用。
                // 默认按 border-box(CSS width 即边框盒宽,padding 从中扣减——
                // 主流站点 `*{box-sizing:border-box}` 全局重置的既成事实语义);
                // 站点显式 `box-sizing:content-box` 时,width 是**内容盒**宽,
                // padding 加回到边框盒上。
                let explicit_content_w = style.box_sizing_content
                    && (style.width.is_some() || style.width_percent.is_some());
                let mut bw = (width - style.margin_left - style.margin_right).max(40.0);
                // max-content 测量语境(flex/grid 条目测量传 MAX_CONTENT_WIDTH)
                // 下百分比宽无意义 —— `width:100%` 的 svg 会被解成 10 万 px,
                // 条目巨宽居中到负 x(猎聘行业分类 tab 竖排+负坐标根因)。
                // 按百分比受可用宽约束的正常语义,只在真实包含块宽下求解。
                if let Some(p) = style.width_percent.filter(|_| width < MAX_CONTENT_WIDTH) {
                    bw = (p / 100.0 * width).max(1.0);
                } else if let Some(w) = style.width {
                    bw = w.max(1.0);
                }
                if let Some(mw) = style.max_width {
                    bw = bw.min(mw.max(1.0));
                }
                // min-width 地板(在 max-width 之后:冲突时 max 优先,与 CSS 钳序一致)。
                if let Some(mw) = style.min_width {
                    bw = bw.max(mw);
                }
                if explicit_content_w {
                    bw += style.padding_left + style.padding_right;
                }
                let content_w = (bw - style.padding_left - style.padding_right).max(1.0);
                let start_y = self.y;
                self.y += style.padding_top;
                // 盒高先于子代布局求出:显式/百分比高度不依赖子代内容;
                // auto 高(占位 0)待子代布局完成后用内容高回填。
                // content-box 显式高度同样按内容盒语义(padding 加回)。
                let own_definite_h = style.height.is_some()
                    || (style.height_percent.is_some() && cb.3 != UNKNOWN_CB_H);
                // 显式/百分比高度是**确定**高(height:0 合法,内容溢出不算),
                // auto 高(None)才在子代布局后按内容回填。
                let explicit_h = match style.height {
                    Some(h) => {
                        let h = h.max(0.0);
                        if style.box_sizing_content {
                            Some(h + style.padding_top + style.padding_bottom)
                        } else {
                            Some(h)
                        }
                    }
                    None => match style.height_percent {
                        Some(p) if cb.3 != UNKNOWN_CB_H => Some(((p / 100.0) * cb.3).max(0.0)),
                        _ => None, // auto:子代布局后回填
                    },
                };
                let box_h = explicit_h.unwrap_or(0.0);
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
                // 尾部行内按**本盒内容宽**换行(此前误传外层可用宽,
                // 显式窄宽块里的文本不换行、直接横向溢出)。
                self.flush_inline(buf, content_w);
                // 末子块 margin-bottom 穿出/收编判定(CSS 2.1 8.3.1):
                // 本盒收编边界(padding-bottom/下边框/overflow)收编进内容高,
                // 否则穿出、在 1326 与本盒自身 margin 折叠。
                if contains_child_margins(&style) {
                    self.y += self.pending_margin_bottom;
                    self.pending_margin_bottom = 0.0;
                }
                // 内容高按真实游标推进量;空块/纯空白块为 0(CSS 语义)。
                // 此前 .max(1.0) 把空块顶成 1px,与相邻块2px假间隙叠加,
                // 全页逐块累积漂移(SPA 空壳页面尤甚)。
                let content_h = (self.y - (start_y + style.padding_top)).max(0.0);
                // max-content 测量语境(可用宽取 MAX_CONTENT_WIDTH 哨兵)下,
                // auto 宽块级盒按内容需求收缩(shrink-to-fit),不撑满测量宽
                // —— 否则条目内部任何块级子元素(如 tab 文本 div)都会把
                // 条目撑到 10 万 px,wrap 后单条目独占一行、居中到负 x
                // (猎聘行业分类 tab 竖排 + 负坐标根因)。
                let auto_w = style.width.is_none()
                    && style.width_percent.is_none()
                    && width >= MAX_CONTENT_WIDTH;
                if auto_w {
                    let mut right = bx + style.padding_left;
                    for l in &self.lines[mark_lines..] {
                        right = right.max(l.x + self.font.text_width(&l.text, l.size));
                    }
                    for b in &self.boxes[mark_boxes..] {
                        if b.out_of_flow {
                            continue;
                        }
                        right = right.max(b.x + b.w);
                    }
                    let need = right - bx + style.padding_right;
                    if need < bw {
                        bw = need.max(40.0);
                    }
                }
                let box_h = match explicit_h {
                    Some(h) => h,
                    None => (content_h + style.padding_top + style.padding_bottom).max(0.0),
                };
                // min/max-height:显式与 auto 高统一按 CSS 钳序求解。
                let box_h = constrain_height(&style, box_h);
                // 游标停在盒底;本盒 margin_bottom 先折叠暂存,与下一兄弟
                // margin_top 取折叠值落地(CSS 2.1 8.3.1)。此前
                // `.max(2.0)` 给每块强加 2px 下边距,全页逐块累积漂移。
                self.y = start_y + box_h;
                self.pending_margin_bottom =
                    collapse_margin(self.pending_margin_bottom, style.margin_bottom);
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
                    out_of_flow: false,
                });
                // 边框/外阴影绘制指令(不占几何;relative 偏移需同步平移)。
                push_border_and_shadow(self, &style, bx, start_y, bw, box_h);
                // 阶段三:relative/sticky 整体偏移(不影响占位):先平移子树几何,
                // 再以**偏移后**盒矩形做组回补与自身发射 —— 变换原点随之正确。
                let (rdx, rdy) = if matches!(style.pos.position.as_str(), "relative" | "sticky") {
                    relative_offset(&style, width, if cb.3 == UNKNOWN_CB_H { 0.0 } else { cb.3 })
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
                self.emit_canvas_pixels(&child, &style, seq, bx + rdx, start_y + rdy, bw, box_h);
                // 阶段七:浮动盒登记并恢复流式游标(非浮动为 no-op)。
                float::finish(self, placement, &mut floats, &mut inline_row);
                if style.float != "none" {
                    self.pending_margin_bottom = float_saved_pending;
                }
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
            // 绝对/固定定位子树脱流:不得消费流内 pending,其内边距亦不外泄。
            let saved_pending = std::mem::take(&mut self.pending_margin_bottom);
            position::layout_deferred(self, deferred, cb, depth);
            self.pending_margin_bottom = saved_pending;
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
            || style.display == "inline-flex"
            || style.width.is_some()
            || style.height.is_some()
            || style.width_percent.is_some()
            // 第 174 轮:带 id 的行内元素一律可寻址 —— id 是 Agent 侧最常用的
            // 选择器锚点,猎聘登录入口 span#header-quick-menu-login 是纯
            // display:inline 无显式尺寸,不在此列时快照矩形恒 0×0、点击坐标
            // 落在 (1,1) 死区(近似矩形不影响流,只服务几何寻址)。
            || document::attr(node, "id").map(|v| !v.trim().is_empty()).unwrap_or(false)
    }

    /// 行内盒的近似尺寸:显式 CSS 尺寸优先,其次按文本量估算。
    ///
    /// 百分比宽度按包含块宽度(`avail_w`)求解;百分比高度在父高未知时按 `auto`
    /// 处理(CSS 中行内元素的百分比高度同样不生效)。
    fn inline_box_size(&self, node: &NodeRef, style: &ComputedStyle, avail_w: f64) -> (f64, f64) {
        // 第 175 轮:空行内元素(无文本、无显式尺寸、无可渲染子元素)量成 0×0 ——
        // 不产生行盒(Chrome 语义);盒照常发射,保住第 174 轮的几何寻址诉求
        // (Chrome 空行内元素 rect 亦为 0×0)。表单控件/替换类标签自身即有
        // 固有尺寸(void 元素无子节点,不能按「无子内容」判空)。
        let explicit_dim =
            style.width.is_some() || style.height.is_some() || style.width_percent.is_some();
        let self_replaced = matches!(
            document::tag_name(node).to_ascii_lowercase().as_str(),
            "input" | "select" | "textarea" | "button" | "img" | "svg" | "iframe" | "canvas"
                | "object" | "embed" | "video" | "audio" | "hr"
        );
        if !self_replaced && !explicit_dim && !has_structural_inline_content(node, 0) {
            return (0.0, 0.0);
        }
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
        // 子代 x 基准 = 本元素 **margin-box x**(text/img/嵌套各自由此加
        // 自身 margin+padding 得内容起点);own_box 盒需回退自身偏移。
        let mut inner_x = line_x;
        if own_box {
            let (w, h) = self.inline_box_size(node, style, avail_w);
            let (dx, dy) = relative_offset(style, avail_w, self.viewport_h);
            let bx = line_x + style.margin_left + style.padding_left + dx;
            let by = self.y + dy;
            tf = self.emit_inline_box(node, style, bx, by, w, h, tf);
            // 阶段七:登记当前行占用,后续行内浮动可排在它的右侧剩余带。
            // 第 175 轮:空行内元素 0×0 不占流(Chrome 语义)—— 只发射寻址盒,
            // 不登记行占用、不推进游标,消灭 `<a name=x></a>` 类幽灵行高。
            if w > 0.0 && h > 0.0 {
                row.register(bx, by, w, h, line_x + avail_w);
            }
            box_x = bx;
            box_w = w;
            inner_x = bx - style.margin_left - style.padding_left;
        }
        // 行内块/控件盒为子代建立独立行内上下文(浮动不越出该盒),子代几何
        // 也相对自身盒求解(此前一律沿用祖先 line_x/avail_w)。
        let mut inner_row = float::InlineRow::default();
        let child_row: &mut float::InlineRow = if own_box { &mut inner_row } else { row };
        self.collect_inline_children(node, style, buf, depth, inner_x, box_w, child_row, tf);
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
                    // line_x = 本元素 margin-box x;push_styled 再加自身
                    // margin+padding 得文本内容起点。
                    buf.push_styled(&raw, style, line_x, tf, style.text_shadow.clone());
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
            // 阶段三第三步:行内替换元素(<img>/<svg>):记录矩形并预留行高。
            if tag.eq_ignore_ascii_case("img") || tag.eq_ignore_ascii_case("svg") {
                let (iw, ih, isrc) = self.replaced_size(&child, &child_style, avail_w, None);
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
                    out_of_flow: false,
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
            // 子级 margin-box 起点 = 本元素内容盒 x(margin-box + 自身 margin+padding)。
            let child_x = line_x + style.margin_left + style.padding_left;
            if child_style.float != "none" {
                self.layout_inline_float(&child, &child_style, buf, depth, child_x, avail_w, row, tf);
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
            self.collect_inline(&child, &child_style, buf, depth + 1, child_x, avail_w, tf, row);
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
            out_of_flow: false,
        });
        // 输入框/按钮等交互元素的边框与外阴影(绘制不占几何)。
        push_border_and_shadow(self, style, bx, by, w, h);
        let tag = document::tag_name(node);
        self.emit_element(seq, style, &tag, bx, by, w, h);
        self.emit_canvas_pixels(node, style, seq, bx, by, w, h);
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

}
