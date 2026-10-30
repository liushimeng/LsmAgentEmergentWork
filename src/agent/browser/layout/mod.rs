//! 布局引擎:块级流式布局 + 文本换行 + Flex 容器 + Grid 网格 + 定位体系(阶段三)。
//!
//! 样式全部来自 `style::StyleEngine` 的计算样式;本模块只负责几何计算。
//! 产出纯数据(`TextLine`/`ElementBox`/`PaintBox`),不持有 DOM 句柄,可跨 await 保存。
//! 目录分工:`mod.rs` 块级/行内主流,`flex.rs` Flex 格式化上下文,
//! `grid.rs` Grid 网格布局,`position.rs` 定位体系,`tests.rs` 布局单测。

mod flex;
mod float;
mod inline;
mod inline_atomic;
mod list_marker;
mod measure;
mod grid;
mod helpers;
mod position;
mod table;
mod top_layer;
mod widgets;
#[cfg(test)]
mod position_tests;
#[cfg(test)]
mod form_widget_tests;
#[cfg(test)]
mod inline_atomic_tests;
#[cfg(test)]
mod scroll_tests;
#[cfg(test)]
mod list_marker_tests;
#[cfg(test)]
mod pseudo_state_tests;
#[cfg(test)]
mod table_tests;
#[cfg(test)]
mod top_layer_tests;
#[cfg(test)]
mod widget_ui_tests;
#[cfg(test)]
mod inline_fragment_tests;
#[cfg(test)]
mod tests_jd_display;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use helpers::*;
use inline::InlineBuf;
pub(crate) use widgets::select_is_list;
pub(crate) use measure::{append, translate, Fragment};

use kuchikiki::NodeRef;

use super::document::{self, Document};
use super::images::ImageSizes;
use super::render::display::{self, BgFill, DisplayItem, DisplayKind, WidgetGlyphKind};
use super::render::FontSet;
use super::style::{Affine, BgLayer, CornerRadius, BorderColor, ComputedStyle, MediaCtx, StyleEngine, TextShadow};
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
    /// 背景色 alpha(0..=255,默认 255)。半透明背景在绘制期与已绘制的
    /// 底层背景合成(不在计算样式阶段按白底压平,见 `style::values`)。
    pub background_alpha: u8,
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
    /// 第 46 轮:可滚动容器(`overflow:auto|scroll`)的最大滚动量,
    /// key = stable selector,value = px。供工具层钳制滚动偏移。
    pub scroll_max: HashMap<String, f64>,
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
        scroll_max: HashMap::new(),
        seq: 0,
        images,
        base_url: doc.base_url.clone(),
        y: 0.0,
        z_floor: 0,
        fixed_depth: 0,
        pending_margin_bottom: 0.0,
        intrinsic: false,
        positioned_layer: 0,
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
    // 页高钳制(第 178 轮):布局几何的任何非有限/超界值在收尾归零标不可见,
    // 防止哨兵级包含块高(UNKNOWN_CB_H)泄漏成 4.49e307 天文坐标污染整页
    // 高度与渲染预算(实测猎聘 `.ant-spin-dot` 的 `top:50%`)。钳制放在
    // 画布背景发射前,保证 canvas_h 也取到有界值。
    let height = height.min(PAGE_H_SANITY_MAX);
    // 根画布背景传播(CSS 2.1 §14.2 / CSS Backgrounds 3 §2.2):根元素(html)
    // 的背景覆盖整个画布;根背景为空时 **body 的背景上传播为画布背景**。
    // 本布局器不给 html/body 建盒(初始包含块直接从 body 子元素起排),
    // 两者的背景此前从不发射 → 凡是「背景写在 body/html 上」的站点
    // (实测 zhipin 安全验证页 `body{background:var(--bg-page)}`)整页渲染
    // 成纯白,与 Chrome 对照构成「整体白屏」观感。此处按规范补传播:
    //   1. html 有背景色/渐变层 → 用 html 的;
    //   2. 否则用 body 的;
    //   3. 均无 → 不发射(维持渲染层白底基线)。
    // 画布背景位于一切内容之下(含负 z-index 元素),以 z=i64::MIN 保证最先
    // 绘制;高度取 max(页高, 视口高),整页截图(向下滚动区)同样铺满。
    // 注:html 与 body 同时设置背景时,规范语义是 html 的传播、body 的画在
    // 自身盒上;body 无盒故自身背景仍不发射(与本修复前一致,不劣化)。
    {
        let root_style = if html_style.background.is_some()
            || !html_style.background_layers.is_empty()
        {
            Some(&html_style)
        } else if body_style.background.is_some() || !body_style.background_layers.is_empty() {
            Some(&body_style)
        } else {
            None
        };
        if let Some(style) = root_style {
            let canvas_h = height.max(ctx.viewport_h);
            let mut canvas_items: Vec<DisplayItem> = Vec::new();
            let mut push_canvas = |fill: BgFill| {
                canvas_items.push(DisplayItem {
                    kind: DisplayKind::Background { fill },
                    x: 0.0,
                    y: 0.0,
                    w: ctx.width,
                    h: canvas_h,
                    radius: [CornerRadius::Px(0.0); 4],
                    z: i64::MIN,
                    seq: 0,
                    opacity: 1.0,
                    clip: None,
                    transform: None,
                    fixed: false,
                    layer: 0,
                });
            };
            if let Some(bg) = style.background {
                if style.background_alpha > 0 {
                    push_canvas(BgFill::Solid(bg, style.background_alpha));
                }
            }
            for layer in style.background_layers.iter().rev() {
                if let BgLayer::Gradient(g) = layer {
                    push_canvas(BgFill::Gradient(g.clone()));
                }
            }
            if !canvas_items.is_empty() {
                display_items.splice(0..0, canvas_items);
            }
        }
    }
    sanitize_layout_geometry(&mut ctx.boxes, &mut ctx.lines, &mut display_items);
    Layout {
        lines: ctx.lines,
        boxes: ctx.boxes,
        paints: ctx.paints,
        borders: ctx.borders,
        shadows: ctx.shadows,
        display: display_items,
        scroll_max: ctx.scroll_max,
        width: ctx.width,
        height,
    }
}

/// 页高/坐标健全性上限:真实页面不可能超过 ~1e5 逻辑像素;超出即视为
/// 计算事故(如哨兵值泄漏、百分比链爆炸),整页钳到该值防内存/渲染预算被打爆。
pub(crate) const PAGE_H_SANITY_MAX: f64 = 1_000_000.0;


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
    /// 第 46 轮:可滚动容器的最大滚动量(stable selector → px)。
    scroll_max: HashMap<String, f64>,
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
    /// 内在尺寸(max-content)测量模式:行内文本对齐偏移**不参与**宽度计算。
    ///
    /// max-content 宽度与 text-align 无关(居中文字不会把块的固有宽度撑大),
    /// 但布局在 MAX_CONTENT_WIDTH(10 万)语境下按居中偏移摆放 → 行起点
    /// 落到 ~5 万处,再被 flex 基础宽/abs 收缩宽度取 `l.x + 文本宽` 当内容宽,
    /// 条目宽度被撑到 5 万,后续布局全歪(实测猎聘登录页 tab 条目 50034px、
    /// 登录框容器撑满 1920px 被推出屏幕右侧)。
    pub(crate) intrinsic: bool,
    /// 绘制层:定位元素(position != static)及其子树按 CSS 2.1 附录 E 第 8 步
    /// 绘制在常规流内容之上。布局子树时置 1,离开后还原。
    pub(crate) positioned_layer: u8,
}

impl<'a> LayoutCtx<'a> {
    /// 当前元素的绘制层:定位元素自身为 1;其子树沿用 `positioned_layer`。
    pub(crate) fn layer_for(&self, style: &ComputedStyle) -> u8 {
        if style.pos.position != "static" {
            1
        } else {
            self.positioned_layer
        }
    }

    /// 分配一个 DFS 先序序号(元素视觉发射 / 文本行 flush 各取各的)。
    pub(crate) fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq - 1
    }

    /// 显示列表当前长度(组效果回补区间起点)。
    pub(crate) fn display_len(&self) -> usize {
        self.display.len()
    }

    /// 绘制滚动条(第 46 轮):右侧竖条,轨道浅灰 + 拇指深灰。
    ///
    /// `ratio` = 滚动位置比例(0..=1);`visible` = 可见比例(内容高 / 盒高)。
    pub(crate) fn push_scrollbar(
        &mut self,
        style: &ComputedStyle,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        ratio: f64,
        visible: f64,
    ) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let seq = self.next_seq();
        self.display.push(DisplayItem {
            kind: DisplayKind::Background {
                fill: BgFill::Solid([235, 235, 235], 255),
            },
            x,
            y,
            w,
            h,
            radius: [CornerRadius::Px(0.0); 4],
            z: style.pos.z_index.max(self.z_floor),
            seq,
            opacity: style.opacity,
            clip: None,
            transform: None,
            fixed: self.fixed_depth > 0,
            layer: self.layer_for(style),
        });
        let thumb_h = (h * visible).max(8.0).min(h);
        let thumb_y = y + (h - thumb_h) * ratio;
        let seq = self.next_seq();
        self.display.push(DisplayItem {
            kind: DisplayKind::Background {
                fill: BgFill::Solid([150, 150, 150], 255),
            },
            x,
            y: thumb_y,
            w,
            h: thumb_h,
            radius: [CornerRadius::Px(0.0); 4],
            z: style.pos.z_index.max(self.z_floor),
            seq,
            opacity: style.opacity,
            clip: None,
            transform: None,
            fixed: self.fixed_depth > 0,
            layer: self.layer_for(style),
        });
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
            layer: self.layer_for(style),
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
        self.emit_element_tf(seq, style, tag, x, y, w, h, None)
    }

    /// `emit_element` 的**祖先变换链**变体(第 47 轮)。
    ///
    /// 行内片段盒在**行提交期**才发射,此时元素自身早已离开收集栈,祖先链
    /// 变换只能由文本 run 携带过来(`InlineRun::transform`);不传进来时,
    /// 变换祖先里的行内元素背景/边框会画在未变换坐标系上。
    pub(crate) fn emit_element_tf(
        &mut self,
        seq: u64,
        style: &ComputedStyle,
        tag: &str,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        inherited_tf: Option<Affine>,
    ) {
        let visible = style_visible(style);
        if !visible {
            return;
        }
        let tf = match (self.own_transform(style, x, y, w, h), inherited_tf) {
            (Some(m), Some(anc)) => Some(m.compose(&anc)),
            (Some(m), None) => Some(m),
            (None, Some(anc)) => Some(anc),
            (None, None) => None,
        };
        // 层叠上下文近似:自身 z 不得低于祖先链下限(见 z_floor 注释)。
        // 两者都先取到局部量:闭包会借用 self,而 push 需要 &mut self。
        let z = style.pos.z_index.max(self.z_floor);
        let is_fixed = self.fixed_depth > 0;
        let layer = self.layer_for(style);
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
            layer,
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
                fill: BgFill::Solid(bg, style.background_alpha),
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
            layer: self.positioned_layer,
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
                layer: self.positioned_layer,
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
            layer: self.positioned_layer,
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
                // 第 41 轮:裁剪逃逸 —— `position:fixed` 子树与顶层浮层
                // (dialog/select 面板,layer=2)不受祖先 overflow 裁剪
                // (CSS 定位/顶层元素脱离常规裁剪链)。
                if it.fixed || it.layer >= top_layer::TOP_LAYER {
                    continue;
                }
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
    ///
    /// 第 45 轮:`srcset`/`picture` 候选优先,`src` 兜底。
    pub(crate) fn image_src(&self, node: &NodeRef) -> Option<String> {
        self.srcset_pick(node).or_else(|| self.src_attr_abs(node))
    }

    /// `<img src>` 的绝对地址。
    fn src_attr_abs(&self, node: &NodeRef) -> Option<String> {
        let raw = document::attr(node, "src")?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        urlutil::resolve(&self.base_url, raw)
    }

    /// `srcset`/`picture` 选中的候选地址(未命中则 `None`)。
    ///
    /// 选���口径:`<picture>` 内的 `<source srcset>` 优先(按 `media`/`type` 过滤),
    /// 其次 `<img srcset>`;候选按视口宽与像素密度选择(见 `document::select_srcset`)。
    pub(crate) fn srcset_pick(&self, node: &NodeRef) -> Option<String> {
        let mut cands = document::picture_srcset(node, self.width);
        if cands.is_none() {
            if let Some(raw) = document::attr(node, "srcset") {
                let parsed = document::parse_srcset(&raw);
                if !parsed.is_empty() {
                    cands = Some(parsed);
                }
            }
        }
        let cands = cands?;
        let pick = document::select_srcset(&cands, self.width, 1.0)?;
        let pick = pick.trim();
        if pick.is_empty() {
            return None;
        }
        urlutil::resolve(&self.base_url, pick)
    }

    /// 实际用于显示的图片地址:优先已加载的 `srcset` 候选,回落已加载的 `src`。
    pub(crate) fn image_display_src(&self, node: &NodeRef) -> Option<String> {
        let picked = self.srcset_pick(node);
        if let Some(abs) = picked.as_ref() {
            if self.image_loaded(abs) {
                return Some(abs.clone());
            }
        }
        if let Some(abs) = self.src_attr_abs(node) {
            if self.image_loaded(&abs) {
                return Some(abs);
            }
        }
        picked.or_else(|| self.src_attr_abs(node))
    }

    /// 图片固有尺寸(解码后的自然宽高);未加载返回 `None`。
    ///
    /// `srcset` 候选未加载成功时回落 `src` 的固有尺寸(盒高不至于塌成 0)。
    pub(crate) fn natural_size(&self, node: &NodeRef) -> Option<(f64, f64)> {
        let src = self.image_src(node)?;
        if let Some(sz) = self.images.natural(&src) {
            return Some(sz);
        }
        if let Some(fallback) = self.src_attr_abs(node) {
            if let Some(sz) = self.images.natural(&fallback) {
                return Some(sz);
            }
        }
        None
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
            } else if tag.eq_ignore_ascii_case("canvas") {
                // canvas 的 HTML width/height 是位图缓冲固有尺寸,缺省 300×150。
                let aw = document::attr(node, "width")
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .filter(|v| *v > 0.0)
                    .unwrap_or(300.0);
                let ah = document::attr(node, "height")
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .filter(|v| *v > 0.0)
                    .unwrap_or(150.0);
                Some((aw, ah))
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
        // 绘制层(CSS 2.1 附录 E 第 8 步):定位元素(position != static)连同其
        // 子树绘制在常规流内容之上。此前只按 (z, seq) 排序,定位元素若在 DOM
        // 顺序上靠前会被后面的常规流图片/背景整块盖住(实测猎聘登录框被 hero
        // 背景图覆盖 → 表单完全看不见)。
        let prev_layer = self.positioned_layer;
        if current.pos.position != "static" {
            self.positioned_layer = 1;
        }
        // 阶段三:脱离流的定位元素延后到兄弟节点布置完再处理。
        let mut deferred: Vec<(NodeRef, ComputedStyle)> = Vec::new();
        // 阶段七:本容器内的浮动台账(浮动不跨块容器)。
        let mut floats = float::FloatState::default();
        // 阶段七:本容器内联上下文共享的行占用(行内浮动跨兄弟元素排同一行)。
        let mut inline_row = float::InlineRow::default();
        // 行内游标(第 178 轮):inline-box/文本兄弟水平推进。此前块容器的
        // 行内子级共用同一 x —— 两个 inline-block 药丸(font-size:0 容器内)
        // 同坐标叠印(实测猎聘行业卡标签)。文本经 buf 的 line_w 累计流动,
        // 游标服务于「盒先行、后续内容随后」的混排。
        let mut pen = x;
        for child in parent.children() {
            if let Some(text) = child.as_text() {
                let raw = text.borrow();
                if !raw.trim().is_empty() {
                    // 块级文本:变换由本块 `apply_group` 回补,run 不带变换;
                    // text-shadow 随 run 带到 flush(块级文本无行内样式承载)。
                    // x 基准 = 本容器 margin-box x(文本起点 = 容器内容盒 x)。
                    let base = pen - current.margin_left - current.padding_left;
                    // 块级文本的行内片段盒属主恒为空:块级容器自身的背景由
                    // 块级路径发射,行内片段盒只服务**普通行内元素**(第 47 轮)。
                    buf.push_styled(
                        &raw,
                        current,
                        base,
                        None,
                        current.text_shadow.clone(),
                        None,
                    );
                    // 文本推进游标,但不越出容器内容右缘(长文本换行后,
                    // 后续行内盒最多落在右缘内,而不是被总字宽推到天边)。
                    let tw = self.font.text_width(&raw, current.font_size);
                    pen += tw.min((x + width - pen).max(0.0));
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
            // 第 41 轮:模态对话框走顶层语义(视口居中 + ::backdrop),
            // 不参与常规块级流,也不被祖先 overflow 裁剪。
            if top_layer::layout_dialog(self, &child, &style, depth) {
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
            // 基础表格格式化上下文:tr/td/th 走行列网格,不再退化为普通块/行内。
            if style.display == "table" {
                self.y += self.pending_margin_bottom;
                self.pending_margin_bottom = 0.0;
                self.flush_inline(buf, width);
                table::layout_container(self, &child, x, width, depth, &style, cb);
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
            // 表单控件即使 CSS `display:block` 也保留替换式固有尺寸,
            // 否则无内容 input 会被普通块级路径量成 0 高全宽盒。
            if style.display == "block" && is_form_control_tag(&child) {
                self.flush_inline(buf, width);
                let (iw, ih) = self.inline_box_size(&child, &style, width);
                let bx = x + style.margin_left;
                let gap = collapse_margin(self.pending_margin_bottom, style.margin_top);
                self.pending_margin_bottom = 0.0;
                let by = self.y + gap;
                let seq = self.next_seq();
                self.boxes.push(ElementBox {
                    selector: Document::stable_selector(&child),
                    tag: document::tag_name(&child),
                    x: bx,
                    y: by,
                    w: iw,
                    h: ih,
                    background: style.background,
                    background_alpha: style.background_alpha,
                    visible: style_visible(&style),
                    z_index: style.pos.z_index,
                    out_of_flow: false,
                });
                push_border_and_shadow(self, &style, bx, by, iw, ih);
                let tag = document::tag_name(&child);
                self.emit_element(seq, &style, &tag, bx, by, iw, ih);
                widgets::emit_form_control_text(self, &child, &style, &tag, bx, by, iw, ih, None);
                self.y = by + ih;
                self.pending_margin_bottom =
                    collapse_margin(self.pending_margin_bottom, style.margin_bottom);
                continue;
            }
            // 第 44 轮:替换元素只有**块级**(`display:block`)时独占一行;
            // 行内替换元素(`img`/`svg`/`canvas`/`iframe`)走**行内原子盒**路径
            // —— 占宽、参与断行、按基线对齐(此前一律独占一行且顶对齐)。
            if is_replaced_tag(&child) && style.is_block() {
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
                    background_alpha: style.background_alpha,
                    visible,
                    z_index: style.pos.z_index,
                    out_of_flow: false,
                });
                let replaced_tag = document::tag_name(&child);
                self.emit_element(seq, &style, &replaced_tag, bx + dx, by + dy, iw, ih);
                self.emit_canvas_pixels(&child, &style, seq, bx + dx, by + dy, iw, ih);
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
                // 盒宽(border box,不含 margin):auto 时撑满父内容宽减 margin;
                // 百分比按父内容宽求解;显式长度直接采用。
                // 默认按 content-box(CSS 初始值,width 即内容盒宽,padding
                // 加回到边框盒——2026-10-30 京东真机修正,与 Chrome 一致);
                // 站点显式 `box-sizing:border-box` 时,width 是**边框盒**宽,
                // padding 从中扣减。
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
                // CSS 2.1 §10.3.3:auto 左右边距在**显式宽度**下按剩余空间求解
                // (双侧 auto = 居中;单侧 auto = 吃满剩余)。宽度 auto(默认撑满)
                // 时剩余恒 0,退化与旧行为一致。此前 auto 在样式层被丢成 0,
                // `width:1192px;margin:0 auto` 的页头恒贴左(实测猎聘页头 x=0)。
                let ml = if (style.margin_left_auto || style.margin_right_auto)
                    && (style.width.is_some() || style.width_percent.is_some())
                {
                    let free =
                        (width - bw - style.margin_left - style.margin_right).max(0.0);
                    match (style.margin_left_auto, style.margin_right_auto) {
                        (true, true) => free / 2.0,
                        // 仅左侧 auto:左侧吃满剩余(右侧保持原值)。
                        (true, false) => free,
                        // 仅右侧 auto:定位仍由左侧决定,右侧吸收剩余。
                        (false, true) => style.margin_left,
                        _ => style.margin_left,
                    }
                } else {
                    style.margin_left
                };
                let bx = x + ml;
                if explicit_content_w {
                    bw += style.padding_left + style.padding_right;
                }
                // 第 41 轮:`list-style-position: inside` 的 marker 占用行首,
                // li 内容整体右移「marker 宽 + 间隙」(近似:CSS 只让首行让位)。
                let marker_inset = list_marker::inside_inset(self, &child, &style);
                let content_w =
                    (bw - style.padding_left - style.padding_right - marker_inset).max(1.0);
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
                    bx + style.padding_left + marker_inset,
                    start_y + style.padding_top,
                    (bw - style.padding_left - style.padding_right - marker_inset).max(0.0),
                    cb_h,
                );
                // 第 46 轮:可滚动容器(`overflow:auto|scroll`)—— 内容按运行时
                // 滚动偏移上移(偏移量落在临时解析树的 `data-lsm-scroll-top`),
                // 超出盒高的部分由祖先裁剪链裁掉(见 `apply_group`)。
                let scroll_top = if style.overflow_scroll && own_definite_h {
                    document::attr(&child, document::ATTR_SCROLL_TOP)
                        .and_then(|v| v.trim().parse::<f64>().ok())
                        .unwrap_or(0.0)
                        .max(0.0)
                } else {
                    0.0
                };
                if scroll_top > 0.0 {
                    self.y -= scroll_top;
                }
                self.layout_children(
                    &child,
                    bx + style.padding_left + marker_inset,
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
                // 第 46 轮:可滚动容器的内容真实高(含被滚出视口的部分)与滚动条。
                if style.overflow_scroll && own_definite_h {
                    let visible_h = (self.y - (start_y + style.padding_top)).max(0.0);
                    let true_content_h = visible_h + scroll_top;
                    let inner_h = (box_h - style.padding_top - style.padding_bottom).max(0.0);
                    if true_content_h > inner_h + 0.5 {
                        let max_scroll = true_content_h - inner_h;
                        let stable = Document::stable_selector(&child);
                        self.scroll_max.insert(stable.clone(), max_scroll);
                        // 滚动条:右侧竖条(轨道 + 拇指),叠在内容之上。
                        let track_w = 6.0f64.min(bw * 0.2).max(4.0);
                        let track_x = bx + bw - track_w;
                        let track_y = start_y + style.padding_top;
                        let track_h = inner_h;
                        self.push_scrollbar(
                            &style,
                            track_x,
                            track_y,
                            track_w,
                            track_h,
                            (scroll_top / max_scroll).clamp(0.0, 1.0),
                            (inner_h / true_content_h).clamp(0.05, 1.0),
                        );
                    }
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
                    background_alpha: style.background_alpha,
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
                self.emit_list_marker(
                    &child,
                    &style,
                    bx + rdx + style.padding_left,
                    start_y + rdy,
                    box_h,
                );
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
                self.layout_inline_float(&child, &style, buf, depth, pen, width, &mut inline_row, None);
            } else {
                if inline_row.has_floats() {
                    let y = inline_row.float_bottom();
                    if y > self.y {
                        self.y = y;
                    }
                    inline_row.deactivate();
                }
                // 行内游标:inline-box 子级(inline-block/控件/显式尺寸/带 id)
                // 按 margin-box 宽推进,后续兄弟落在其后;纯 inline 子级按
                // 子树文本宽近似推进(文本自身仍经 buf 流动,推进只为让
                // 后续盒/文本不与其叠印)。inline_box_size 对空元素返回 0,
                // 幽灵元素不推进(与第 175 轮语义一致)。
                let is_box_child = self.inline_box_worthy(&child, &style);
                self.collect_inline(&child, &style, buf, depth + 1, pen, width, None, &mut inline_row);
                if is_box_child {
                    let (w, _h) = self.inline_box_size(&child, &style, width);
                    pen += style.margin_left
                        + style.padding_left
                        + w
                        + style.padding_right
                        + style.margin_right;
                } else {
                    let text = child.text_contents();
                    let tw = self.font.text_width(&text, style.font_size);
                    pen += tw.min((x + width - pen).max(0.0));
                }
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
        // 还原绘制层(定位元素子树的绘制层只在其子树内有效)。
        self.positioned_layer = prev_layer;
    }

}
