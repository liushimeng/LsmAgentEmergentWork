//! 布局几何与盒模型**辅助函数**(第 47 轮拆分)。
//!
//! 从 `layout/mod.rs` 机械拆分(单文件 ≤ 1800 行规范):几何防御性钳制、
//! 边框/阴影指令入队、标签类别判定、控件显示文本、高度钳序、相对偏移与
//! 外边距折叠判定。全部为**自由函数**,只被 `layout/` 内部使用。
//!
//! 搬移口径:代码逐行原样搬运(零改写),仅新增本模块文档头与 `use super::*;`
//! 取用父模块的 `LayoutCtx`/`ElementBox`/`TextLine`/`BorderDraw`/`ShadowDraw`
//! 等类型;`mod.rs` 以 `pub(crate) use helpers::*;` 重导入,故外部
//! `crate::…::layout::X` 路径与兄弟模块 `use super::*` 取用**零改动**。

use super::*;

/// 几何防御性钳制:任何非有限或超界(>1e6)的坐标/尺寸归零并标记不可见。
/// 这是哨兵泄漏(UNKNOWN_CB_H)与数值爆炸的**最后防线** —— 布局正确性
/// 修复在源头(见 resolved_top 的哨兵守卫),这里只保证渲染层与 inspect
/// 拿到的永远是有界值(实测猎聘 ant-spin-dot y=4.49e307 曾直通显示列表)。
pub(super) fn sanitize_layout_geometry(
    boxes: &mut [ElementBox],
    lines: &mut [TextLine],
    display: &mut [DisplayItem],
) {
    fn sane(v: f64) -> bool {
        v.is_finite() && v.abs() <= PAGE_H_SANITY_MAX
    }
    for b in boxes.iter_mut() {
        if !sane(b.x) || !sane(b.y) || !sane(b.w) || !sane(b.h) {
            b.x = 0.0;
            b.y = 0.0;
            b.w = 0.0;
            b.h = 0.0;
            b.visible = false;
        }
    }
    for l in lines.iter_mut() {
        if !sane(l.x) || !sane(l.y) {
            l.x = 0.0;
            l.y = 0.0;
            l.visible = false;
        }
    }
    for it in display.iter_mut() {
        if !sane(it.x) || !sane(it.y) || !sane(it.w) || !sane(it.h) {
            it.x = 0.0;
            it.y = 0.0;
            it.w = 0.0;
            it.h = 0.0;
        }
    }
}


/// 解析四边边框颜色:currentColor 代入文字色。
pub(super) fn resolved_border_colors(style: &ComputedStyle) -> [([u8; 3], u8); 4] {
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
        // 坐标点击与 inspect 全部失真)。canvas 同理:HTML width/height 是
        // 位图缓冲固有尺寸,缺省 300×150。
        "img" | "svg" | "iframe" | "canvas"
    )
}

/// 表单控件标签(即使 CSS `display:block` 也保留替换式固有尺寸)。
pub(crate) fn is_form_control_tag(node: &NodeRef) -> bool {
    matches!(
        document::tag_name(node).to_ascii_lowercase().as_str(),
        "input" | "select" | "textarea" | "button" | "progress" | "meter"
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

/// select 控件显示文本:运行时 `selected` option 的 label，否则首个 option。
pub(crate) fn select_display_text(node: &NodeRef) -> String {
    let Ok(options) = node.select("option") else {
        return String::new();
    };
    let mut first = String::new();
    for opt in options {
        let opt = opt.as_node().clone();
        let label = document::collapse_ws(&opt.text_contents());
        if first.is_empty() {
            first = label.clone();
        }
        if document::attr(&opt, "selected").is_some() {
            return label;
        }
    }
    first
}

pub(crate) fn parse_f64_attr(node: &NodeRef, key: &str) -> Option<f64> {
    document::attr(node, key)?.trim().parse::<f64>().ok()
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
pub(super) fn collapse_margin(a: f64, b: f64) -> f64 {
    if a >= 0.0 && b >= 0.0 {
        a.max(b)
    } else {
        a + b
    }
}

/// 子块 `margin_bottom` 是否折叠穿出本盒(CSS 2.1 8.3.1):
/// 有 padding-bottom / 下边框 / overflow 裁剪(建 BFC)时收编进内容高,
/// 否则穿出本盒、与本盒自身 margin 折叠(经典 `<section><p>` 段距语义)。
pub(super) fn contains_child_margins(style: &ComputedStyle) -> bool {
    style.padding_bottom > 0.0
        || (style.border_drawn[2] && style.border_widths[2] > 0.0)
        || style.overflow_clip
}
