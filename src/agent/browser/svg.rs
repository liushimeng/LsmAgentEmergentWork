//! SVG 矢量图形解析(第 50 轮)。
//!
//! `<svg>` 此前只有替换元素几何 + 空盒占位,图标/箭头/星标一律白板。
//! 本模块把 svg 子树解析成**局部坐标折线形状集**(已含 transform 复合与
//! viewBox 映射),由 `layout/svg_paint.rs` 发射进显示列表、
//! `render/raster.rs` 扫描线填充 + 距离场描边。
//!
//! 支持子集:`rect`(含圆角)/`circle`/`ellipse`/`line`/`polyline`/`polygon`/
//! `path`(M L H V C S Q T A Z)/`g`/`defs`+`use`/嵌套 `svg`(按平移近似)/
//! `text`+`tspan`;绘制属性 `fill`/`stroke`/`stroke-width`/`fill-opacity`/
//! `stroke-opacity`/`opacity`/`fill-rule`/`stroke-linecap`;渐变
//! `linearGradient`/`radialGradient`(含 `<stop>`/`href` 继承/`gradientUnits`/
//! `gradientTransform`/`spreadMethod`);变换 `translate`/`scale`/`rotate`/
//! `matrix`/`skewX`/`skewY`;`viewBox` + `preserveAspectRatio`(meet/none/slice)。
//!
//! 边界(如实上报):`pattern`/`filter`/`mask`/`clipPath`/`marker`/`<image>`/
//! stroke-dasharray/CSS 作用于 svg 子元素均不支持;径向渐变焦点 `fx/fy`
//! 偏移不建模(按圆心距离近似);`color-interpolation` 统一按 sRGB。

use crate::agent::browser::document;
use crate::agent::browser::style::transform::Affine;
use crate::agent::browser::style::values::parse_color;
use kuchikiki::NodeRef;
use std::collections::HashMap;

/// 解析产物上限:防御畸形 path 无限展开(形状 × 子路径 × 点三级封顶)。
const MAX_SHAPES: usize = 512;
const MAX_SUBPATHS: usize = 256;
const MAX_POINTS: usize = 4096;

/// 渐变铺展方式(`spreadMethod`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpreadMethod {
    /// 钳位到首尾色标(缺省)。
    Pad,
    /// 镜像反射。
    Reflect,
    /// 周期平铺。
    Repeat,
}

/// 渐变停止点(`<stop>`);`offset` 已归一到 0..1。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GradientStop {
    pub offset: f64,
    pub color: [u8; 3],
    /// 0..1(已含 `stop-opacity`)。
    pub alpha: f64,
}

/// 渐变绘制参数。
///
/// `coords` 的语义由 `user_space` 决定:
/// - `userSpaceOnUse`:`coords` 已映射到**内容盒局部坐标**
///   (施加过 `viewbox_map ∘ tf ∘ gradientTransform`),`r` 按变换的
///   几何平均缩放预解;
/// - `objectBoundingBox`(缺省):`coords` 是**形状 bbox 单位方形**里的
///   梯度坐标,光栅期再套 `translate(bbox.x0,bbox.y0) ∘ scale(bbox.w,bbox.h)`;
///   `inv` 为 `gradientTransform` 的逆(把单位方形点变回渐变空间)。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GradientPaint {
    /// false = 线性,true = 径向。
    pub radial: bool,
    pub user_space: bool,
    /// 线性 `[x1,y1,x2,y2,0]`;径向 `[cx,cy,r,fx,fy]`。
    pub coords: [f64; 5],
    /// 仅 `objectBoundingBox` 使用:`gradientTransform` 的逆。
    pub inv: Option<Affine>,
    pub spread: SpreadMethod,
    pub stops: Vec<GradientStop>,
}

/// SVG 绘制属性:纯色或渐变。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SvgPaint {
    /// rgb + alpha(0..1)。
    Solid([u8; 3], f64),
    Gradient(GradientPaint),
}

impl SvgPaint {
    /// 折叠组透明度后的 u8 alpha(纯色取自身 alpha,渐变取首 stop alpha)。
    pub(crate) fn alpha0(&self) -> f64 {
        match self {
            SvgPaint::Solid(_, a) => *a,
            SvgPaint::Gradient(g) => g.stops.first().map_or(0.0, |s| s.alpha),
        }
    }
}

/// 单个可绘制形状。
///
/// `subpaths` 的点为**元素内容盒局部坐标**(原点 = 内容盒左上,已含全部
/// 变换);`(points, closed)` 的 closed 表示描边时闭合(Z / polygon)。
#[derive(Debug, Clone)]
pub(crate) struct SvgShape {
    pub subpaths: Vec<(Vec<(f64, f64)>, bool)>,
    /// `None` = 不填充(fill:none 或透明度折叠为 0)。
    pub fill: Option<SvgPaint>,
    pub evenodd: bool,
    pub stroke: Option<SvgPaint>,
    pub stroke_w: f64,
    pub cap_round: bool,
}

/// SVG 文本 run(一个 `<text>` 的直接文本或一个 `<tspan>`)。
///
/// 坐标为**内容盒局部坐标**,`y` 语义由 `central` 决定:
/// `false` = 字母基线;`true` = 字形盒垂直中心(`dominant-baseline:middle|central`)。
#[derive(Debug, Clone)]
pub(crate) struct SvgText {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub color: [u8; 3],
    pub alpha: f64,
    pub bold: bool,
    /// `text-anchor`:0=start、0.5=middle、1=end。
    pub anchor: f64,
    pub central: bool,
}

/// SVG `<image>` 位图引用(坐标为内容盒局部坐标)。
#[derive(Debug, Clone)]
pub(crate) struct SvgImage {
    /// `href` / `xlink:href` 原始值(相对地址由布局期按基准 URL 解析)。
    pub href: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// `preserveAspectRatio` 含 `slice`。
    pub slice: bool,
    /// `preserveAspectRatio="none"`(拉伸)。
    pub stretch: bool,
}

/// svg 子树的解析产物(按文档序):形状、文本与位图。
#[derive(Debug, Clone)]
pub(crate) enum SvgNode {
    Shape(SvgShape),
    Text(SvgText),
    Image(SvgImage),
}

/// 解析入口:`svg` 元素 → 内容盒局部坐标形状集(仅形状,不含文本)。
///
/// `current_color` 为 svg 元素 CSS `color` 计算值(`fill="currentColor"`
/// 的解析基准)。无形状返回空 Vec(调用方保留能力占位)。
pub(crate) fn collect_shapes(
    svg: &NodeRef,
    content_w: f64,
    content_h: f64,
    current_color: [u8; 3],
) -> Vec<SvgShape> {
    collect_nodes(svg, content_w, content_h, current_color)
        .into_iter()
        .filter_map(|n| match n {
            SvgNode::Shape(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// 解析入口:`svg` 元素 → 内容盒局部坐标的**形状 + 文本**(按文档序)。
///
/// 形状与文本同表输出,保证 `<text>` 与图形的上下叠放关系与 DOM 一致。
pub(crate) fn collect_nodes(
    svg: &NodeRef,
    content_w: f64,
    content_h: f64,
    current_color: [u8; 3],
) -> Vec<SvgNode> {
    if !(content_w.is_finite() && content_h.is_finite() && content_w > 0.0 && content_h > 0.0) {
        return Vec::new();
    }
    let mut ids = Vec::new();
    index_ids(svg, &mut ids, 0);
    let gradients = collect_gradients(svg, current_color, content_w, content_h);
    let map = viewbox_map(svg, content_w, content_h);
    let vb = viewbox_dims(svg);
    let mut raw = Vec::new();
    let st = SvgState::root(current_color);
    walk_children(svg, &st, &mut raw, false, &ids, &gradients, &map, vb, 0);
    let (sx, sy) = map_scale(&map);
    raw.truncate(MAX_SHAPES);
    raw.into_iter()
        .filter(|n| match n {
            SvgNode::Shape(s) => !shape_is_empty(&s.subpaths),
            SvgNode::Text(t) => !t.text.is_empty(),
            SvgNode::Image(im) => im.w > 0.0 && im.h > 0.0,
        })
        .map(|n| match n {
            SvgNode::Shape(s) => {
                let mut subpaths = Vec::with_capacity(s.subpaths.len());
                for (pts, closed) in s.subpaths {
                    let v: Vec<(f64, f64)> = pts
                        .iter()
                        .take(MAX_POINTS)
                        .map(|p| map.apply(p.0, p.1))
                        .collect();
                    subpaths.push((v, closed));
                }
                // 描边宽按用户空间变换 + viewBox 映射的几何平均缩放近似
                //(非等比变换与规范有偏差,图标场景可接受)。
                let stroke_w = (s.stroke_w * (sx * sy).abs().sqrt()).clamp(0.0, 512.0);
                SvgNode::Shape(SvgShape {
                    subpaths,
                    fill: s.fill,
                    evenodd: s.evenodd,
                    stroke: s.stroke,
                    stroke_w,
                    cap_round: s.cap_round,
                })
            }
            SvgNode::Text(t) => {
                let (x, y) = map.apply(t.x, t.y);
                let scale = map_scale(&map);
                let s = (scale.0 * scale.1).sqrt().max(1e-9);
                SvgNode::Text(SvgText {
                    x,
                    y,
                    size: (t.size * s).max(1.0),
                    ..t
                })
            }
            SvgNode::Image(im) => {
                let (x, y) = map.apply(im.x, im.y);
                let (x1, y1) = map.apply(im.x + im.w, im.y + im.h);
                SvgNode::Image(SvgImage {
                    x,
                    y,
                    w: (x1 - x).abs(),
                    h: (y1 - y).abs(),
                    ..im
                })
            }
        })
        .collect()
}

fn shape_is_empty(subpaths: &[(Vec<(f64, f64)>, bool)]) -> bool {
    subpaths.is_empty() || subpaths.iter().all(|(pts, _)| pts.len() < 2)
}

// ---------- 子树游走 ----------

/// 绘制属性继承态(SVG 缺省:fill 黑、stroke 无、宽 1)。
struct SvgState {
    /// 根用户空间 → 当前用户空间(含祖先 transform 链与自身 transform)。
    tf: Affine,
    /// 根用户空间 → 当前空间的描宽近似缩放(sqrt|det| 累计)。
    tf_scale: f64,
    /// alpha ∈ 0..1;`None` = paint none。
    fill: Option<SvgPaint>,
    stroke: Option<SvgPaint>,
    stroke_w: f64,
    opacity: f64,
    evenodd: bool,
    cap_round: bool,
    current_color: [u8; 3],
}

impl SvgState {
    fn root(current_color: [u8; 3]) -> Self {
        Self {
            tf: Affine::identity(),
            tf_scale: 1.0,
            fill: Some(SvgPaint::Solid([0, 0, 0], 1.0)),
            stroke: None,
            stroke_w: 1.0,
            opacity: 1.0,
            evenodd: false,
            cap_round: false,
            current_color,
        }
    }

    /// 子元素属性覆盖(缺省继承父态)。
    ///
    /// `gradients`/`map`/`vb` 供 `url(#id)` 渐变解析与 `userSpaceOnUse`
    /// 坐标预映射使用(见 `GradientPaint`)。
    fn inherit(
        node: &NodeRef,
        parent: &SvgState,
        gradients: &GradientMap,
        map: &Affine,
        vb: Option<(f64, f64)>,
    ) -> Self {
        let own_tf = parse_transform(node);
        let mut st = Self {
            tf: parent.tf.compose(&own_tf),
            tf_scale: parent.tf_scale * affine_scale(&own_tf),
            fill: parent.fill.clone(),
            stroke: parent.stroke.clone(),
            stroke_w: parent.stroke_w,
            opacity: parent.opacity,
            evenodd: parent.evenodd,
            cap_round: parent.cap_round,
            current_color: parent.current_color,
        };
        if let Some(v) = paint_value(node, "fill", st.current_color, gradients, &st.tf, map, vb) {
            st.fill = v;
        }
        if let Some(v) = paint_value(node, "stroke", st.current_color, gradients, &st.tf, map, vb) {
            st.stroke = v;
        }
        if let Some(v) = num_attr(node, "stroke-width") {
            st.stroke_w = v.max(0.0);
        }
        if let Some(v) = num_attr(node, "opacity") {
            st.opacity *= v.clamp(0.0, 1.0);
        }
        if let Some(v) = num_attr(node, "fill-opacity") {
            let k = v.clamp(0.0, 1.0);
            st.fill = st.fill.map(|p| scale_paint_alpha(p, k));
        }
        if let Some(v) = num_attr(node, "stroke-opacity") {
            let k = v.clamp(0.0, 1.0);
            st.stroke = st.stroke.map(|p| scale_paint_alpha(p, k));
        }
        if let Some(v) = document::attr(node, "fill-rule") {
            st.evenodd = v.trim().eq_ignore_ascii_case("evenodd");
        }
        if let Some(v) = document::attr(node, "stroke-linecap") {
            st.cap_round = v.trim().eq_ignore_ascii_case("round");
        }
        st
    }
}

/// 把绘制属性的 alpha 乘上系数(渐变对每个 stop 生效)。
fn scale_paint_alpha(p: SvgPaint, k: f64) -> SvgPaint {
    match p {
        SvgPaint::Solid(c, a) => SvgPaint::Solid(c, a * k),
        SvgPaint::Gradient(mut g) => {
            for s in &mut g.stops {
                s.alpha *= k;
            }
            SvgPaint::Gradient(g)
        }
    }
}

/// 变换的描宽近似缩放(sqrt|det|)。
fn affine_scale(t: &Affine) -> f64 {
    let det = t.a * t.d - t.b * t.c;
    det.abs().sqrt()
}

fn walk_children(
    node: &NodeRef,
    st: &SvgState,
    out: &mut Vec<SvgNode>,
    in_defs: bool,
    ids: &[(String, NodeRef)],
    gradients: &GradientMap,
    map: &Affine,
    vb: Option<(f64, f64)>,
    depth: usize,
) {
    if depth > 16 || out.len() >= MAX_SHAPES {
        return;
    }
    for child in node.children() {
        if child.as_element().is_none() {
            continue;
        }
        let tag = document::tag_name(&child).to_ascii_lowercase();
        match tag.as_str() {
            "g" | "a" | "switch" => {
                let cst = SvgState::inherit(&child, st, gradients, map, vb);
                walk_children(&child, &cst, out, in_defs, ids, gradients, map, vb, depth + 1);
            }
            // 资源容器:只登记 id 供 use/渐变引用,不直接渲染。
            "defs" | "symbol" | "marker" | "clippath" | "mask" | "pattern" | "lineargradient"
            | "radialgradient" | "filter" => {
                walk_children(&child, st, out, true, ids, gradients, map, vb, depth + 1);
            }
            "svg" => {
                // 嵌套 svg:按 x/y 平移近似(独立 viewBox 精确映射不在子集内)。
                let mut cst = SvgState::inherit(&child, st, gradients, map, vb);
                let x = num_attr(&child, "x").unwrap_or(0.0);
                let y = num_attr(&child, "y").unwrap_or(0.0);
                cst.tf = cst.tf.compose(&Affine::translate(x, y));
                walk_children(&child, &cst, out, in_defs, ids, gradients, map, vb, depth + 1);
            }
            "text" => {
                if !in_defs {
                    let cst = SvgState::inherit(&child, st, gradients, map, vb);
                    emit_text_runs(&child, &cst, out, gradients, map, vb);
                }
            }
            "image" => {
                if !in_defs {
                    emit_svg_image(&child, st, out, gradients, map, vb);
                }
            }
            "use" => {
                let href = document::attr(&child, "href")
                    .or_else(|| document::attr(&child, "xlink:href"))
                    .map(|v| v.trim().trim_start_matches('#').to_string());
                let Some(target_id) = href.filter(|v| !v.is_empty()) else {
                    continue;
                };
                let Some((_, target)) =
                    ids.iter().find(|(id, _)| id.eq_ignore_ascii_case(&target_id))
                else {
                    continue;
                };
                let mut cst = SvgState::inherit(&child, st, gradients, map, vb);
                let x = num_attr(&child, "x").unwrap_or(0.0);
                let y = num_attr(&child, "y").unwrap_or(0.0);
                cst.tf = cst.tf.compose(&Affine::translate(x, y));
                // 引用链防环:目标里再 use 回自身时由深度墙兜底。
                // 目标是图形元素(叶子)直接建形状;是容器(g/svg/…)则走子树。
                let ttag = document::tag_name(target).to_ascii_lowercase();
                match ttag.as_str() {
                    "g" | "a" | "switch" | "svg" => {
                        walk_children(
                            target, &cst, out, in_defs, ids, gradients, map, vb, depth + 1,
                        );
                    }
                    "text" => emit_text_runs(target, &cst, out, gradients, map, vb),
                    _ => emit_shape(target, &ttag, &cst, out, gradients, map, vb),
                }
            }
            _ => {
                if in_defs {
                    continue;
                }
                emit_shape(&child, &tag, st, out, gradients, map, vb);
            }
        }
    }
}

/// 图形元素 → 原始形状(应用元素自身 transform 与绘制属性;无图形返回)。
///
/// `use` 内联与直接渲染共用本函数(规范上 use 引用的是图形元素或 g)。
#[allow(clippy::too_many_arguments)]
fn emit_shape(
    node: &NodeRef,
    tag: &str,
    st: &SvgState,
    out: &mut Vec<SvgNode>,
    gradients: &GradientMap,
    map: &Affine,
    vb: Option<(f64, f64)>,
) {
    let Some(subpaths) = shape_subpaths(node, tag) else {
        return;
    };
    let cst = SvgState::inherit(node, st, gradients, map, vb);
    let fill = fold_paint(cst.fill, cst.opacity);
    let stroke = fold_paint(cst.stroke, cst.opacity);
    if fill.is_none() && (stroke.is_none() || cst.stroke_w <= 0.0) {
        return;
    }
    let mapped: Vec<(Vec<(f64, f64)>, bool)> = subpaths
        .into_iter()
        .take(MAX_SUBPATHS)
        .filter(|(pts, _)| pts.len() >= 2)
        .map(|(pts, closed)| {
            let v: Vec<(f64, f64)> =
                pts.iter().take(MAX_POINTS).map(|p| cst.tf.apply(p.0, p.1)).collect();
            (v, closed)
        })
        .collect();
    if mapped.is_empty() {
        return;
    }
    out.push(SvgNode::Shape(SvgShape {
        subpaths: mapped,
        fill,
        evenodd: cst.evenodd,
        stroke,
        stroke_w: cst.stroke_w * cst.tf_scale,
        cap_round: cst.cap_round,
    }));
}

/// 组透明度折叠(alpha<=0.01 视为无绘制)。
fn fold_paint(paint: Option<SvgPaint>, group_opacity: f64) -> Option<SvgPaint> {
    let p = paint?;
    let folded = scale_paint_alpha(p, group_opacity);
    (folded.alpha0() > 0.01).then_some(folded)
}

/// id 索引(use 目标反查)。
fn index_ids(node: &NodeRef, ids: &mut Vec<(String, NodeRef)>, depth: usize) {
    if depth > 16 || ids.len() >= 1024 {
        return;
    }
    for child in node.children() {
        if child.as_element().is_some() {
            if let Some(id) = document::attr(&child, "id") {
                let id = id.trim().to_string();
                if !id.is_empty() && !ids.iter().any(|(v, _)| v == &id) {
                    ids.push((id, child.clone()));
                }
            }
            index_ids(&child, ids, depth + 1);
        }
    }
}

// ---------- 基础图形 ----------

/// 图形元素 → 子路径(元素自身用户坐标,未含 transform)。
fn shape_subpaths(node: &NodeRef, tag: &str) -> Option<Vec<(Vec<(f64, f64)>, bool)>> {
    match tag {
        "rect" => rect_subpaths(node),
        "circle" => {
            let (cx, cy) =
                (num_attr(node, "cx").unwrap_or(0.0), num_attr(node, "cy").unwrap_or(0.0));
            let r = num_attr(node, "r").unwrap_or(0.0);
            (r > 0.0)
                .then(|| vec![(ellipse_pts(cx, cy, r, r, 0.0, std::f64::consts::TAU), true)])
        }
        "ellipse" => {
            let (cx, cy) =
                (num_attr(node, "cx").unwrap_or(0.0), num_attr(node, "cy").unwrap_or(0.0));
            let (rx, ry) =
                (num_attr(node, "rx").unwrap_or(0.0), num_attr(node, "ry").unwrap_or(0.0));
            (rx > 0.0 && ry > 0.0)
                .then(|| vec![(ellipse_pts(cx, cy, rx, ry, 0.0, std::f64::consts::TAU), true)])
        }
        "line" => {
            let pts = vec![
                (num_attr(node, "x1").unwrap_or(0.0), num_attr(node, "y1").unwrap_or(0.0)),
                (num_attr(node, "x2").unwrap_or(0.0), num_attr(node, "y2").unwrap_or(0.0)),
            ];
            Some(vec![(pts, false)])
        }
        "polyline" | "polygon" => {
            let pts = parse_points(&document::attr(node, "points").unwrap_or_default());
            (!pts.is_empty()).then(|| vec![(pts, tag == "polygon")])
        }
        "path" => parse_path(&document::attr(node, "d").unwrap_or_default()),
        _ => None,
    }
}

fn rect_subpaths(node: &NodeRef) -> Option<Vec<(Vec<(f64, f64)>, bool)>> {
    let x = num_attr(node, "x").unwrap_or(0.0);
    let y = num_attr(node, "y").unwrap_or(0.0);
    let w = num_attr(node, "width")?;
    let h = num_attr(node, "height")?;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let mut rx = num_attr(node, "rx").unwrap_or(0.0);
    let mut ry = num_attr(node, "ry").unwrap_or(rx);
    if rx <= 0.0 {
        rx = ry;
    }
    if ry <= 0.0 {
        ry = rx;
    }
    // 规范:圆角不得超过半宽/半高。
    rx = rx.min(w / 2.0);
    ry = ry.min(h / 2.0);
    let mut pts = Vec::with_capacity(40);
    if rx <= 0.0 || ry <= 0.0 {
        pts.push((x, y));
        pts.push((x + w, y));
        pts.push((x + w, y + h));
        pts.push((x, y + h));
    } else {
        // 四个四分之一椭圆角 + 四段直边(顺时针,屏幕坐标)。
        let q = std::f64::consts::FRAC_PI_2;
        let corner = |cx: f64, cy: f64, a0: f64| -> Vec<(f64, f64)> {
            ellipse_pts(cx, cy, rx, ry, a0, q)
        };
        pts.extend(corner(x + w - rx, y + ry, -q));
        pts.extend(corner(x + w - rx, y + h - ry, 0.0));
        pts.extend(corner(x + rx, y + h - ry, q));
        pts.extend(corner(x + rx, y + ry, std::f64::consts::PI));
    }
    Some(vec![(pts, true)])
}

/// 椭圆弧采样(a0 → a0+sweep,屏幕坐标顺时针为正角)。
fn ellipse_pts(cx: f64, cy: f64, rx: f64, ry: f64, a0: f64, sweep: f64) -> Vec<(f64, f64)> {
    let n = ((sweep.abs() / 0.35).ceil() as usize).clamp(8, 64);
    (0..=n)
        .map(|i| {
            let a = a0 + sweep * i as f64 / n as f64;
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect()
}

// ---------- 数字流解析 ----------

/// `points` 属性:数字流(逗号/空白/正负号分隔)成对取点。
pub(crate) fn parse_points(s: &str) -> Vec<(f64, f64)> {
    let nums = parse_number_list(s);
    nums.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1])).collect()
}

/// 逗号/空白/正负号分隔数字列表。
pub(crate) fn parse_number_list(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut p = NumParser { b: s.as_bytes(), i: 0 };
    while let Some(v) = p.next_number() {
        out.push(v);
        if out.len() >= MAX_POINTS {
            break;
        }
    }
    out
}

/// 数字流解析器(SVG 松散语法:空白/逗号/连续正负号均可分隔)。
pub(crate) struct NumParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> NumParser<'a> {
    fn skip_sep(&mut self) {
        while self.i < self.b.len()
            && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r' | b',')
        {
            self.i += 1;
        }
    }

    /// 读命令字母(path 用);非字母返回 None。
    fn eat_cmd(&mut self) -> Option<u8> {
        self.skip_sep();
        if self.i < self.b.len() && self.b[self.i].is_ascii_alphabetic() {
            let c = self.b[self.i];
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    pub(crate) fn next_number(&mut self) -> Option<f64> {
        self.skip_sep();
        let start = self.i;
        if self.i < self.b.len() && matches!(self.b[self.i], b'+' | b'-') {
            self.i += 1;
        }
        let mut seen_digit = false;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if c.is_ascii_digit() {
                seen_digit = true;
                self.i += 1;
            } else if c == b'.' {
                // 第二个小数点视为下一数字起点(松散语法 "1.5.5" = 1.5 / .5)。
                if self.b[start..self.i].iter().any(|&x| x == b'.') {
                    break;
                }
                self.i += 1;
            } else {
                break;
            }
        }
        if !seen_digit {
            self.i = start;
            return None;
        }
        // 指数部分。
        if self.i < self.b.len() && (self.b[self.i] | 0x20) == b'e' {
            let save = self.i;
            self.i += 1;
            if self.i < self.b.len() && matches!(self.b[self.i], b'+' | b'-') {
                self.i += 1;
            }
            let mut exp_digit = false;
            while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                exp_digit = true;
                self.i += 1;
            }
            if !exp_digit {
                self.i = save;
            }
        }
        std::str::from_utf8(&self.b[start..self.i])
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
    }
}

// ---------- path 解析 ----------

/// `d` 属性解析为子路径(M m L l H h V v C c S s Q q T t A a Z z)。
///
/// 约定:各段折线**不含起点**(起点由定位命令保证已入列);`Z` 闭合的
/// 子路径 closed=true(描边补闭合段;填充一律按闭合处理)。
pub(crate) fn parse_path(d: &str) -> Option<Vec<(Vec<(f64, f64)>, bool)>> {
    let mut p = NumParser { b: d.as_bytes(), i: 0 };
    let mut subpaths: Vec<(Vec<(f64, f64)>, bool)> = Vec::new();
    let mut cur = (0.0_f64, 0.0_f64);
    let mut sub_start = (0.0_f64, 0.0_f64);
    let mut cur_sub: Vec<(f64, f64)> = Vec::new();
    let mut cubic_cp: Option<(f64, f64)> = None;
    let mut quad_cp: Option<(f64, f64)> = None;
    // 隐式重复命令(M 后续坐标对按 lineto 语义,保留大小写)。
    let mut pending: Option<u8> = None;

    loop {
        let raw = match pending.take() {
            Some(c) => c,
            None => match p.eat_cmd() {
                Some(c) => c,
                None => break,
            },
        };
        let cmd = raw | 0x20;
        let rel = raw.is_ascii_lowercase();
        match cmd {
            b'm' => {
                let mut first = true;
                while let (Some(x), Some(y)) = (p.next_number(), p.next_number()) {
                    let t = if rel { (cur.0 + x, cur.1 + y) } else { (x, y) };
                    if first {
                        if !cur_sub.is_empty() {
                            subpaths.push((std::mem::take(&mut cur_sub), false));
                        }
                        sub_start = t;
                        first = false;
                    }
                    cur = t;
                    cur_sub.push(cur);
                }
                pending = Some(if rel { b'l' } else { b'L' });
            }
            b'l' => {
                while let (Some(x), Some(y)) = (p.next_number(), p.next_number()) {
                    cur = if rel { (cur.0 + x, cur.1 + y) } else { (x, y) };
                    cur_sub.push(cur);
                }
            }
            b'h' => {
                while let Some(v) = p.next_number() {
                    cur.0 = if rel { cur.0 + v } else { v };
                    cur_sub.push(cur);
                }
            }
            b'v' => {
                while let Some(v) = p.next_number() {
                    cur.1 = if rel { cur.1 + v } else { v };
                    cur_sub.push(cur);
                }
            }
            b'c' => {
                while let (
                    Some(x1),
                    Some(y1),
                    Some(x2),
                    Some(y2),
                    Some(x),
                    Some(y),
                ) = (
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                ) {
                    let (a1, a2, e) = if rel {
                        (
                            (cur.0 + x1, cur.1 + y1),
                            (cur.0 + x2, cur.1 + y2),
                            (cur.0 + x, cur.1 + y),
                        )
                    } else {
                        ((x1, y1), (x2, y2), (x, y))
                    };
                    cubic_cp = Some(a2);
                    quad_cp = None;
                    flatten_cubic(cur, a1, a2, e, &mut cur_sub);
                    cur = e;
                }
            }
            b's' => {
                while let (Some(x2), Some(y2), Some(x), Some(y)) = (
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                ) {
                    // 反射控制点:上一 cubic 的第二控制点关于当前点镜像。
                    let a1 = cubic_cp.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                    let (a2, e) =
                        if rel { ((cur.0 + x2, cur.1 + y2), (cur.0 + x, cur.1 + y)) } else { ((x2, y2), (x, y)) };
                    cubic_cp = Some(a2);
                    quad_cp = None;
                    flatten_cubic(cur, a1, a2, e, &mut cur_sub);
                    cur = e;
                }
            }
            b'q' => {
                while let (Some(x1), Some(y1), Some(x), Some(y)) = (
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                    p.next_number(),
                ) {
                    let (cp, e) = if rel {
                        ((cur.0 + x1, cur.1 + y1), (cur.0 + x, cur.1 + y))
                    } else {
                        ((x1, y1), (x, y))
                    };
                    quad_cp = Some(cp);
                    cubic_cp = None;
                    flatten_quad(cur, cp, e, &mut cur_sub);
                    cur = e;
                }
            }
            b't' => {
                while let (Some(x), Some(y)) = (p.next_number(), p.next_number()) {
                    let cp = quad_cp.map_or(cur, |c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1));
                    let e = if rel { (cur.0 + x, cur.1 + y) } else { (x, y) };
                    quad_cp = Some(cp);
                    cubic_cp = None;
                    flatten_quad(cur, cp, e, &mut cur_sub);
                    cur = e;
                }
            }
            b'a' => {
                loop {
                    let Some(rx) = p.next_number() else { break };
                    let (Some(ry), Some(rot), Some(large), Some(sweep), Some(x), Some(y)) = (
                        p.next_number(),
                        p.next_number(),
                        p.next_number(),
                        p.next_number(),
                        p.next_number(),
                        p.next_number(),
                    ) else {
                        break;
                    };
                    let e = if rel { (cur.0 + x, cur.1 + y) } else { (x, y) };
                    match arc_points(cur, e, rx, ry, rot, large != 0.0, sweep != 0.0) {
                        Some(list) => cur_sub.extend(list),
                        // 退化弧(rx/ry=0 或端点重合)按直线段处理。
                        None => cur_sub.push(e),
                    }
                    cubic_cp = None;
                    quad_cp = None;
                    cur = e;
                }
            }
            b'z' => {
                if !cur_sub.is_empty() {
                    subpaths.push((std::mem::take(&mut cur_sub), true));
                }
                cur = sub_start;
                cubic_cp = None;
                quad_cp = None;
            }
            _ => {
                // 未知命令:吃掉后续数字再找下一命令。
                while p.next_number().is_some() {}
            }
        }
        if cur_sub.len() > MAX_POINTS || subpaths.len() >= MAX_SUBPATHS {
            break;
        }
    }
    if !cur_sub.is_empty() {
        subpaths.push((cur_sub, false));
    }
    subpaths.retain(|(pts, _)| pts.len() >= 2);
    (!subpaths.is_empty()).then_some(subpaths)
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// 三次贝塞尔拍平(弦长自适应,8..=48 段;不含起点)。
fn flatten_cubic(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    out: &mut Vec<(f64, f64)>,
) {
    let chord = dist(p0, p1) + dist(p1, p2) + dist(p2, p3);
    let n = ((chord / 3.0).ceil() as usize).clamp(8, 48);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let x = u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0;
        let y = u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1;
        out.push((x, y));
    }
}

/// 二次贝塞尔拍平(不含起点)。
fn flatten_quad(
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    out: &mut Vec<(f64, f64)>,
) {
    let chord = dist(p0, p1) + dist(p1, p2);
    let n = ((chord / 3.0).ceil() as usize).clamp(8, 48);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        out.push((u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0, u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1));
    }
}

/// 椭圆弧(端点参数化 → 中心参数化,SVG 规范 F.6.5)采样;不含起点。
///
/// 退化(rx/ry≈0)返回 None(调用方按直线段处理)。
fn arc_points(
    p0: (f64, f64),
    p1: (f64, f64),
    rx: f64,
    ry: f64,
    rot_deg: f64,
    large: bool,
    sweep: bool,
) -> Option<Vec<(f64, f64)>> {
    let mut rx = rx.abs();
    let mut ry = ry.abs();
    if rx < 1e-9 || ry < 1e-9 {
        return None;
    }
    if (p0.0 - p1.0).abs() < 1e-9 && (p0.1 - p1.1).abs() < 1e-9 {
        return None;
    }
    let phi = rot_deg.to_radians();
    let (cph, sph) = (phi.cos(), phi.sin());
    let dx2 = (p0.0 - p1.0) / 2.0;
    let dy2 = (p0.1 - p1.1) / 2.0;
    let x1p = cph * dx2 + sph * dy2;
    let y1p = -sph * dx2 + cph * dy2;
    // 半径不足时按规范放大。
    let lam = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let co = if den <= 1e-12 {
        0.0
    } else {
        let v = (num / den).sqrt();
        // large-arc 与 sweep 同号取负(规范 F.6.5.4)。
        if large != sweep {
            v
        } else {
            -v
        }
    };
    let cxp = co * rx * y1p / ry;
    let cyp = -co * ry * x1p / rx;
    let cx = cph * cxp - sph * cyp + (p0.0 + p1.0) / 2.0;
    let cy = sph * cxp + cph * cyp + (p0.1 + p1.1) / 2.0;
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = angle_between(1.0, 0.0, ux, uy);
    let mut dtheta = angle_between(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f64::consts::TAU;
    }
    if sweep && dtheta < 0.0 {
        dtheta += std::f64::consts::TAU;
    }
    let n = ((dtheta.abs() / 0.35).ceil() as usize).clamp(6, 96);
    Some(
        (1..=n)
            .map(|i| {
                let a = theta1 + dtheta * i as f64 / n as f64;
                let (ca, sa) = (a.cos(), a.sin());
                (cx + cph * rx * ca - sph * ry * sa, cy + sph * rx * ca + cph * ry * sa)
            })
            .collect(),
    )
}

/// 有向角(atan2(叉积, 点积),范围 -π..π)。
fn angle_between(ux: f64, uy: f64, vx: f64, vy: f64) -> f64 {
    (ux * vy - uy * vx).atan2(ux * vx + uy * vy)
}

// ---------- viewBox / transform ----------

/// `viewBox` 用户坐标 → 内容盒局部坐标映射。
fn viewbox_map(svg: &NodeRef, cw: f64, ch: f64) -> Affine {
    let vb = document::attr(svg, "viewBox")
        .or_else(|| document::attr(svg, "viewbox"))
        .unwrap_or_default();
    let n = parse_number_list(&vb);
    if n.len() < 4 || n[2] <= 0.0 || n[3] <= 0.0 {
        return Affine::identity();
    }
    let (minx, miny, vw, vh) = (n[0], n[1], n[2], n[3]);
    let par = document::attr(svg, "preserveAspectRatio").unwrap_or_default();
    let low = par.to_ascii_lowercase();
    let toks: Vec<&str> = low.split_whitespace().collect();
    // "defer xMidYMid meet" → 忽略 defer,取 align + meet/slice。
    let stretch = toks.iter().any(|t| *t == "none");
    let slice = toks.iter().any(|t| *t == "slice");
    let align = toks.iter().find(|t| t.len() == 8 && t.starts_with('x') && t.as_bytes()[4] == b'y');
    let frac = |s: &str| if s.ends_with("min") { 0.0 } else if s.ends_with("max") { 1.0 } else { 0.5 };
    let (ax, ay) = match align {
        Some(a) => (frac(&a[1..4]), frac(&a[5..8])),
        None => (0.5, 0.5),
    };
    let (sx, sy) = if stretch {
        (cw / vw, ch / vh)
    } else {
        let s = if slice { (cw / vw).max(ch / vh) } else { (cw / vw).min(ch / vh) };
        (s, s)
    };
    let tx = (cw - vw * sx) * ax;
    let ty = (ch - vh * sy) * ay;
    Affine {
        a: sx,
        d: sy,
        e: tx - minx * sx,
        f: ty - miny * sy,
        ..Affine::identity()
    }
}

/// 映射的 x/y 缩放分量(描宽缩放用)。
fn map_scale(m: &Affine) -> (f64, f64) {
    (m.a.abs().max(1e-9), m.d.abs().max(1e-9))
}

/// `transform` 属性解析(translate/scale/rotate/matrix/skewX/skewY 链)。
pub(crate) fn parse_transform(node: &NodeRef) -> Affine {
    let Some(t) = document::attr(node, "transform") else {
        return Affine::identity();
    };
    transform_str(&t)
}

pub(crate) fn transform_str(t: &str) -> Affine {
    let b = t.as_bytes();
    let mut i = 0;
    let mut m = Affine::identity();
    while i < b.len() {
        if !b[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_alphabetic() {
            i += 1;
        }
        let name = std::str::from_utf8(&b[start..i]).unwrap_or("").to_ascii_lowercase();
        while i < b.len() && b[i] != b'(' {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        i += 1;
        let arg_start = i;
        while i < b.len() && b[i] != b')' {
            i += 1;
        }
        let args = parse_number_list(std::str::from_utf8(&b[arg_start..i.min(b.len())]).unwrap_or(""));
        if i < b.len() {
            i += 1;
        }
        let op = match name.as_str() {
            "translate" => Affine::translate(
                args.first().copied().unwrap_or(0.0),
                args.get(1).copied().unwrap_or(0.0),
            ),
            "scale" => Affine::scale(
                args.first().copied().unwrap_or(1.0),
                args.get(1).copied().unwrap_or_else(|| args.first().copied().unwrap_or(1.0)),
            ),
            "rotate" => {
                let a = args.first().copied().unwrap_or(0.0);
                if args.len() >= 3 {
                    let (cx, cy) = (args[1], args[2]);
                    Affine::translate(cx, cy)
                        .compose(&Affine::rotate_deg(a))
                        .compose(&Affine::translate(-cx, -cy))
                } else {
                    Affine::rotate_deg(a)
                }
            }
            "matrix" if args.len() >= 6 => Affine {
                a: args[0],
                b: args[1],
                c: args[2],
                d: args[3],
                e: args[4],
                f: args[5],
            },
            "skewx" => Affine::skew_deg(args.first().copied().unwrap_or(0.0), 0.0),
            "skewy" => Affine::skew_deg(0.0, args.first().copied().unwrap_or(0.0)),
            _ => Affine::identity(),
        };
        m = m.compose(&op);
    }
    m
}

// ---------- 属性辅助 ----------

fn num_attr(node: &NodeRef, name: &str) -> Option<f64> {
    document::attr(node, name)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// 绘制属性解析:`None`(属性不存在)= 继承;`Some(None)` = none/未解析;
/// `Some(Some(paint))` = 具体绘制(纯色或渐变)。
fn paint_value(
    node: &NodeRef,
    name: &str,
    current_color: [u8; 3],
    gradients: &GradientMap,
    tf: &Affine,
    map: &Affine,
    vb: Option<(f64, f64)>,
) -> Option<Option<SvgPaint>> {
    let v = document::attr(node, name)?;
    let v = v.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        return Some(None);
    }
    if v.eq_ignore_ascii_case("currentcolor") {
        return Some(Some(SvgPaint::Solid(current_color, 1.0)));
    }
    if v.starts_with("url(") {
        return Some(resolve_url_paint(v, current_color, gradients, tf, map));
    }
    parse_color(v).map(|inner| inner.map(|(c, a)| SvgPaint::Solid(c, a as f64 / 255.0)))
}

// ---------- 渐变 ----------

/// 渐变原始定义(坐标在渐变自身坐标系,未做 `userSpaceOnUse` 预映射)。
struct GradientDef {
    radial: bool,
    user_space: bool,
    /// 线性 `[x1,y1,x2,y2,0]`;径向 `[cx,cy,r,fx,fy]`。
    coords: [f64; 5],
    transform: Affine,
    spread: SpreadMethod,
    stops: Vec<GradientStop>,
}

/// `id → 渐变定义`。
type GradientMap = HashMap<String, GradientDef>;

/// 收集 svg 子树里的全部渐变定义(按 id 索引)。
fn collect_gradients(
    svg: &NodeRef,
    current_color: [u8; 3],
    content_w: f64,
    content_h: f64,
) -> GradientMap {
    let mut ids = Vec::new();
    index_ids(svg, &mut ids, 0);
    let mut out = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    for (id, node) in &ids {
        let tag = document::tag_name(node).to_ascii_lowercase();
        if !matches!(tag.as_str(), "lineargradient" | "radialgradient") {
            continue;
        }
        if let Some(def) = parse_gradient(node, current_color, &ids, &mut stack, 0, content_w, content_h)
        {
            out.insert(id.clone(), def);
        }
    }
    out
}

/// 解析单个渐变元素(`href` 继承父渐变的缺省属性与色标)。
#[allow(clippy::too_many_arguments)]
fn parse_gradient(
    node: &NodeRef,
    current_color: [u8; 3],
    ids: &[(String, NodeRef)],
    stack: &mut Vec<String>,
    depth: usize,
    content_w: f64,
    content_h: f64,
) -> Option<GradientDef> {
    let tag = document::tag_name(node).to_ascii_lowercase();
    let radial = match tag.as_str() {
        "lineargradient" => false,
        "radialgradient" => true,
        _ => return None,
    };
    if depth > 8 {
        return None;
    }
    let id = document::attr(node, "id")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if !id.is_empty() {
        if stack.contains(&id) {
            return None;
        }
        stack.push(id.clone());
    }
    let mut def = GradientDef {
        radial,
        user_space: false,
        coords: if radial {
            [0.5, 0.5, 0.5, 0.5, 0.5]
        } else {
            [0.0, 0.0, 1.0, 0.0, 0.0]
        },
        transform: Affine::identity(),
        spread: SpreadMethod::Pad,
        stops: Vec::new(),
    };
    // href 继承:父渐变提供缺省值与色标。
    if let Some(href) = href_id(node) {
        if let Some((_, parent)) = ids.iter().find(|(i, _)| i.eq_ignore_ascii_case(&href)) {
            if let Some(p) = parse_gradient(
                parent,
                current_color,
                ids,
                stack,
                depth + 1,
                content_w,
                content_h,
            ) {
                def = p;
            }
        }
    }
    if let Some(v) = document::attr(node, "gradientUnits") {
        def.user_space = v.trim().eq_ignore_ascii_case("userSpaceOnUse");
    }
    if let Some(v) = document::attr(node, "gradientTransform") {
        def.transform = transform_str(&v);
    }
    if let Some(v) = document::attr(node, "spreadMethod") {
        def.spread = match v.trim().to_ascii_lowercase().as_str() {
            "reflect" => SpreadMethod::Reflect,
            "repeat" => SpreadMethod::Repeat,
            _ => SpreadMethod::Pad,
        };
    }
    // 坐标:百分比按 gradientUnits 语义求解(objectBoundingBox 为单位方形分数,
    // userSpaceOnUse 为视口百分比)。
    let vb = if def.user_space {
        viewbox_dims(&root_of(node)).unwrap_or((content_w, content_h))
    } else {
        (1.0, 1.0)
    };
    let g = |name: &str, horiz: bool, def_v: f64| -> f64 {
        pct_attr(node, name, horiz, vb).unwrap_or(def_v)
    };
    if radial {
        let cx = g("cx", true, def.coords[0]);
        let cy = g("cy", false, def.coords[1]);
        let r = g("r", true, def.coords[2]);
        let fx = g("fx", true, cx);
        let fy = g("fy", false, cy);
        def.coords = [cx, cy, r, fx, fy];
    } else {
        let x1 = g("x1", true, def.coords[0]);
        let y1 = g("y1", false, def.coords[1]);
        let x2 = g("x2", true, def.coords[2]);
        let y2 = g("y2", false, def.coords[3]);
        def.coords = [x1, y1, x2, y2, 0.0];
    }
    // 色标:自身有 <stop> 子元素才覆盖(否则继承父渐变的)。
    let mut has_stops = false;
    for c in node.children() {
        if c.as_element().is_some() && document::tag_name(&c).eq_ignore_ascii_case("stop") {
            has_stops = true;
            break;
        }
    }
    if has_stops {
        def.stops = parse_stops(node, current_color);
    }
    if !id.is_empty() {
        stack.pop();
    }
    Some(def)
}

/// 节点所属 svg 根(向上走到顶层元素)。
fn root_of(node: &NodeRef) -> NodeRef {
    let mut cur = node.clone();
    while let Some(p) = cur.parent() {
        if p.as_element().is_some() {
            cur = p;
        } else {
            break;
        }
    }
    cur
}

/// `viewBox` 的宽高(用户单位);无 viewBox 返回 `None`。
fn viewbox_dims(svg: &NodeRef) -> Option<(f64, f64)> {
    let vb = document::attr(svg, "viewBox").or_else(|| document::attr(svg, "viewbox"))?;
    let n = parse_number_list(&vb);
    if n.len() >= 4 && n[2] > 0.0 && n[3] > 0.0 {
        Some((n[2], n[3]))
    } else {
        None
    }
}

/// `href` / `xlink:href` 原始值(不剥 `#`,供 `<image>` 等 URL 引用使用)。
fn href_raw(node: &NodeRef) -> Option<String> {
    document::attr(node, "href")
        .or_else(|| document::attr(node, "xlink:href"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `href` / `xlink:href` 的 `#id` 部分。
fn href_id(node: &NodeRef) -> Option<String> {
    href_raw(node).map(|v| {
        v.trim_start_matches('#')
            .trim_matches(|c| c == '\'' || c == '"')
            .trim()
            .to_string()
    })
}

/// 渐变坐标/半径属性:支持数字与百分比(百分比基准由 `vb` 给出)。
fn pct_attr(node: &NodeRef, name: &str, horizontal: bool, vb: (f64, f64)) -> Option<f64> {
    let raw = document::attr(node, name)?.trim().to_string();
    if let Some(num) = raw.strip_suffix('%') {
        let f: f64 = num.trim().parse().ok()?;
        let base = if horizontal { vb.0 } else { vb.1 };
        Some(f / 100.0 * base)
    } else {
        raw.parse::<f64>().ok().filter(|v| v.is_finite())
    }
}

/// 解析 `<stop>` 子元素(offset 钳到非递减)。
fn parse_stops(node: &NodeRef, current_color: [u8; 3]) -> Vec<GradientStop> {
    let mut stops = Vec::new();
    for child in node.children() {
        if child.as_element().is_none() || !document::tag_name(&child).eq_ignore_ascii_case("stop") {
            continue;
        }
        let offset = parse_stop_offset(document::attr(&child, "offset").as_deref());
        let color = document::attr(&child, "stop-color")
            .and_then(|v| parse_stop_color(&v, current_color))
            .or_else(|| {
                document::attr(&child, "style").and_then(|s| {
                    css_decl(&s, "stop-color").and_then(|v| parse_stop_color(v, current_color))
                })
            })
            .unwrap_or([0, 0, 0]);
        let alpha = document::attr(&child, "stop-opacity")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|v| v.clamp(0.0, 1.0))
            .or_else(|| {
                document::attr(&child, "style").and_then(|s| {
                    css_decl(&s, "stop-opacity").and_then(|v| v.trim().parse::<f64>().ok())
                })
            })
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        stops.push(GradientStop {
            offset,
            color,
            alpha,
        });
    }
    let mut prev = 0.0f64;
    for s in &mut stops {
        s.offset = s.offset.clamp(0.0, 1.0).max(prev);
        prev = s.offset;
    }
    stops
}

fn parse_stop_offset(raw: Option<&str>) -> f64 {
    let Some(raw) = raw else { return 0.0 };
    let raw = raw.trim();
    if let Some(num) = raw.strip_suffix('%') {
        num.trim()
            .parse::<f64>()
            .map(|v| v / 100.0)
            .unwrap_or(0.0)
    } else {
        raw.parse::<f64>().unwrap_or(0.0)
    }
}

fn parse_stop_color(v: &str, current_color: [u8; 3]) -> Option<[u8; 3]> {
    let v = v.trim();
    if v.eq_ignore_ascii_case("currentcolor") {
        return Some(current_color);
    }
    parse_color(v).and_then(|inner| inner).map(|(c, _)| c)
}

/// 从 `style="a:b;c:d"` 里取单个声明值。
fn css_decl<'a>(style: &'a str, name: &str) -> Option<&'a str> {
    for part in style.split(';') {
        let Some((k, v)) = part.split_once(':') else {
            continue;
        };
        if k.trim().eq_ignore_ascii_case(name) {
            return Some(v.trim());
        }
    }
    None
}

/// 把渐变定义解析成绘制参数:userSpaceOnUse 预映射到内容盒局部坐标。
fn resolve_gradient(def: &GradientDef, tf: &Affine, map: &Affine) -> GradientPaint {
    let mut coords = def.coords;
    let inv = if def.user_space {
        // gradientTransform → 元素用户空间 → 根用户空间 → 内容盒局部
        let m = map.compose(&tf.compose(&def.transform));
        coords = transform_coords(def.radial, coords, &m);
        None
    } else {
        def.transform.invert()
    };
    GradientPaint {
        radial: def.radial,
        user_space: def.user_space,
        coords,
        inv,
        spread: def.spread,
        stops: def.stops.clone(),
    }
}

/// 线性映射两个端点;径向映射圆心/焦点并按变换的几何平均缩放半径。
fn transform_coords(radial: bool, coords: [f64; 5], m: &Affine) -> [f64; 5] {
    if radial {
        let (cx, cy) = m.apply(coords[0], coords[1]);
        let (fx, fy) = m.apply(coords[3], coords[4]);
        let det = m.a * m.d - m.b * m.c;
        let scale = det.abs().sqrt().max(1e-9);
        [cx, cy, coords[2] * scale, fx, fy]
    } else {
        let (x1, y1) = m.apply(coords[0], coords[1]);
        let (x2, y2) = m.apply(coords[2], coords[3]);
        [x1, y1, x2, y2, 0.0]
    }
}

/// `url(#id) fallback` 解析:命中渐变用渐变,否则用兜底色。
fn resolve_url_paint(
    v: &str,
    current_color: [u8; 3],
    gradients: &GradientMap,
    tf: &Affine,
    map: &Affine,
) -> Option<SvgPaint> {
    let open = v.find('(')?;
    let close = v[open..].find(')')? + open;
    let inner = v[open + 1..close].trim();
    let id = inner
        .trim()
        .trim_start_matches('#')
        .trim_matches(|c| c == '\'' || c == '"')
        .trim();
    if let Some(def) = gradients.get(id) {
        return Some(SvgPaint::Gradient(resolve_gradient(def, tf, map)));
    }
    let rest = v[close + 1..].trim();
    if rest.is_empty() || rest.eq_ignore_ascii_case("none") {
        return None;
    }
    if rest.eq_ignore_ascii_case("currentcolor") {
        return Some(SvgPaint::Solid(current_color, 1.0));
    }
    parse_color(rest)
        .and_then(|inner| inner)
        .map(|(c, a)| SvgPaint::Solid(c, a as f64 / 255.0))
}

// ---------- 文本 ----------

/// 文本 run 的继承态。
#[derive(Debug, Clone)]
struct TextState {
    x: f64,
    y: f64,
    size: f64,
    color: [u8; 3],
    alpha: f64,
    bold: bool,
    anchor: f64,
    central: bool,
}

impl TextState {
    fn inherit(
        node: &NodeRef,
        parent: &TextState,
        gradients: &GradientMap,
        tf: &Affine,
        map: &Affine,
        vb: Option<(f64, f64)>,
    ) -> Self {
        let mut st = parent.clone();
        let dx = num_list_attr(node, "dx");
        let dy = num_list_attr(node, "dy");
        if let Some(v) = dx.first() {
            st.x += *v;
        }
        if let Some(v) = dy.first() {
            st.y += *v;
        }
        if let Some(v) = num_list_attr(node, "x").first() {
            st.x = *v;
        }
        if let Some(v) = num_list_attr(node, "y").first() {
            st.y = *v;
        }
        if let Some(v) = num_attr(node, "font-size") {
            if v > 0.0 {
                st.size = v;
            }
        }
        if let Some(v) = document::attr(node, "font-weight") {
            let w = v.trim().to_ascii_lowercase();
            st.bold = w == "bold" || w.parse::<i64>().map(|n| n >= 600).unwrap_or(false);
        }
        if let Some(v) = document::attr(node, "text-anchor") {
            st.anchor = match v.trim().to_ascii_lowercase().as_str() {
                "middle" => 0.5,
                "end" => 1.0,
                _ => 0.0,
            };
        }
        if let Some(v) = document::attr(node, "dominant-baseline") {
            let v = v.trim().to_ascii_lowercase();
            st.central = v == "middle" || v == "central";
        }
        if let Some(v) = paint_value(node, "fill", parent.color, gradients, tf, map, vb) {
            match v {
                Some(SvgPaint::Solid(c, a)) => {
                    st.color = c;
                    st.alpha = a;
                }
                // 渐变文字不在子集内,保留继承色(如实边界)。
                Some(SvgPaint::Gradient(_)) => {}
                None => st.alpha = 0.0,
            }
        }
        st
    }

    fn to_run(&self, text: String) -> SvgText {
        SvgText {
            text,
            x: self.x,
            y: self.y,
            size: self.size,
            color: self.color,
            alpha: self.alpha,
            bold: self.bold,
            anchor: self.anchor,
            central: self.central,
        }
    }
}

/// `<image>` 元素 → 位图引用(应用元素变换链,未做 viewBox 映射)。
#[allow(clippy::too_many_arguments)]
fn emit_svg_image(
    node: &NodeRef,
    st: &SvgState,
    out: &mut Vec<SvgNode>,
    gradients: &GradientMap,
    map: &Affine,
    vb: Option<(f64, f64)>,
) {
    let Some(href) = href_raw(node) else {
        return;
    };
    let x = num_attr(node, "x").unwrap_or(0.0);
    let y = num_attr(node, "y").unwrap_or(0.0);
    let w = num_attr(node, "width").unwrap_or(0.0);
    let h = num_attr(node, "height").unwrap_or(0.0);
    if !(w > 0.0 && h > 0.0) {
        return;
    }
    let cst = SvgState::inherit(node, st, gradients, map, vb);
    let (x0, y0) = cst.tf.apply(x, y);
    let (x1, y1) = cst.tf.apply(x + w, y + h);
    let par = document::attr(node, "preserveAspectRatio")
        .unwrap_or_default()
        .to_ascii_lowercase();
    out.push(SvgNode::Image(SvgImage {
        href,
        x: x0.min(x1),
        y: y0.min(y1),
        w: (x1 - x0).abs(),
        h: (y1 - y0).abs(),
        slice: par.contains("slice"),
        stretch: par.contains("none"),
    }));
}

/// `<text>` 元素 → 文本 run 列表(直接文本与每个 `<tspan>` 各一条)。
///
/// run 坐标先按 `<text>` 元素自身的变换链(`st.tf`,与形状点同一链)搬到
/// 根用户空间,再随 `collect_nodes` 的 viewBox 映射进内容盒局部坐标。
fn emit_text_runs(
    node: &NodeRef,
    st: &SvgState,
    out: &mut Vec<SvgNode>,
    gradients: &GradientMap,
    map: &Affine,
    vb: Option<(f64, f64)>,
) {
    // 文本基色取继承的 `fill`(与形状同源的 SvgState),`fill:none` 不画。
    let (base_color, base_alpha) = match &st.fill {
        Some(SvgPaint::Solid(c, a)) => (*c, *a),
        // 渐变文字不在子集内,退回 CSS 当前色。
        Some(SvgPaint::Gradient(_)) => (st.current_color, 1.0),
        None => (st.current_color, 0.0),
    };
    let base = TextState {
        x: 0.0,
        y: 0.0,
        size: 16.0,
        color: base_color,
        alpha: base_alpha,
        bold: false,
        anchor: 0.0,
        central: false,
    };
    let mut ts = TextState::inherit(node, &base, gradients, &st.tf, map, vb);
    ts.alpha *= st.opacity;
    let mut runs = Vec::new();
    push_text_children(node, &ts, &mut runs, gradients, map, vb, 0);
    for mut r in runs {
        let (x, y) = st.tf.apply(r.x, r.y);
        r.x = x;
        r.y = y;
        out.push(SvgNode::Text(r));
    }
}

fn push_text_children(
    node: &NodeRef,
    ts: &TextState,
    out: &mut Vec<SvgText>,
    gradients: &GradientMap,
    map: &Affine,
    vb: Option<(f64, f64)>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    for child in node.children() {
        if child.as_element().is_some() {
            if document::tag_name(&child).eq_ignore_ascii_case("tspan") {
                let cts = TextState::inherit(&child, ts, gradients, &Affine::identity(), map, vb);
                let direct: String = child
                    .children()
                    .filter_map(|c| c.as_text().map(|t| t.borrow().clone()))
                    .collect();
                let direct = normalize_text(&direct);
                if !direct.is_empty() {
                    out.push(cts.to_run(direct));
                }
                push_text_children(&child, &cts, out, gradients, map, vb, depth + 1);
            }
        } else if let Some(t) = child.as_text() {
            let s = normalize_text(&t.borrow());
            if !s.is_empty() {
                out.push(ts.to_run(s));
            }
        }
    }
}

/// SVG 缺省空白折叠(连续空白压成一个空格,首尾丢弃)。
fn normalize_text(s: &str) -> String {
    let mut out = String::new();
    let mut ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            ws = !out.is_empty();
        } else {
            if ws {
                out.push(' ');
                ws = false;
            }
            out.push(ch);
        }
    }
    out
}

/// 数字流属性(`x`/`y`/`dx`/`dy` 可以是列表)。
fn num_list_attr(node: &NodeRef, name: &str) -> Vec<f64> {
    document::attr(node, name)
        .map(|v| parse_number_list(&v))
        .unwrap_or_default()
}
