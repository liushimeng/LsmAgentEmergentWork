//! 列表 marker 的类型与位置(第 41 轮)。
//!
//! 浏览器按 `list-style-type` 决定 marker 形状(圆点/方块/序号/字母/罗马数字),
//! 按 `list-style-position` 决定 marker 画在**内容盒外**(outside,默认,悬挂于
//! 左侧)还是**内容盒内**(inside,与首行文字同行)。
//!
//! inside 模式下 marker 占掉内容行首的一段,`li` 的内容整体右移
//! 「marker 宽 + 间隙」—— 见 `layout/mod.rs::layout_children` 的 `marker_inset`。
//! 近似说明:CSS 只让**首行**让位,本实现让整个内容块让位。

use kuchikiki::NodeRef;

use super::super::document;
use super::super::render::display::WidgetGlyphKind;
use super::super::style::ComputedStyle;
use super::{LayoutCtx, TextLine};

/// marker 图形边长(方块/圆点/圆环)。
const MARKER_SIZE: f64 = 10.0;

/// marker 与内容之间的间隙。
const MARKER_GAP: f64 = 6.0;

/// outside 模式下 marker 相对内容盒左缘的悬挂距离。
const OUTSIDE_OFFSET: f64 = 18.0;

/// marker 形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerKind {
    /// 圆点(`disc`)。
    Disc,
    /// 空心圆(`circle`)。
    Circle,
    /// 方块(`square`)。
    Square,
    /// 阿拉伯数字(`decimal`/`decimal-leading-zero`)。
    Decimal,
    /// 小写字母(`lower-alpha`)。
    LowerAlpha,
    /// 大写字母(`upper-alpha`)。
    UpperAlpha,
    /// 小写罗马数字(`lower-roman`)。
    LowerRoman,
    /// 大写罗马数字(`upper-roman`)。
    UpperRoman,
}

/// 解析后的 marker 描述。
pub(crate) struct Marker {
    pub kind: MarkerKind,
    /// 序号类 marker 的序号(1 起)。
    pub index: usize,
    /// `decimal-leading-zero` 时补零到两位。
    pub leading_zero: bool,
    /// marker 是否画在内容盒内。
    pub inside: bool,
}

/// 解析某个 `li` 的 marker 描述;`None` 表示不画 marker。
pub(crate) fn resolve(node: &NodeRef, style: &ComputedStyle) -> Option<Marker> {
    let parent = node.parent()?;
    let parent_tag = document::tag_name(&parent).to_ascii_lowercase();
    if parent_tag != "ul" && parent_tag != "ol" {
        return None;
    }
    let (ty, pos) = list_style(style, &parent_tag);
    let kind = match ty.as_str() {
        "none" => return None,
        "disc" => MarkerKind::Disc,
        "circle" => MarkerKind::Circle,
        "square" => MarkerKind::Square,
        "decimal" => MarkerKind::Decimal,
        "decimal-leading-zero" => MarkerKind::Decimal,
        "lower-alpha" | "lower-latin" => MarkerKind::LowerAlpha,
        "upper-alpha" | "upper-latin" => MarkerKind::UpperAlpha,
        "lower-roman" => MarkerKind::LowerRoman,
        "upper-roman" => MarkerKind::UpperRoman,
        // 未知取值按父容器默认处理(与浏览器「无效值回退」一致)。
        _ => {
            if parent_tag == "ol" {
                MarkerKind::Decimal
            } else {
                MarkerKind::Disc
            }
        }
    };
    let leading_zero = ty == "decimal-leading-zero";
    let index = if parent_tag == "ol" {
        ol_index(node, &parent)
    } else {
        1
    };
    Some(Marker {
        kind,
        index,
        leading_zero,
        inside: pos == "inside",
    })
}

/// `list-style-type` / `list-style-position` 的取值。
///
/// 优先读长属性;缺失时从 `list-style` 简写里拆出对应分量。
fn list_style(style: &ComputedStyle, parent_tag: &str) -> (String, String) {
    let default_ty = if parent_tag == "ol" { "decimal" } else { "disc" };
    let default_pos = "outside".to_string();

    // `list-style` 简写:`<type> || <position> || <image>`。
    let mut shorthand_ty: Option<String> = None;
    let mut shorthand_pos: Option<String> = None;
    if let Some(raw) = style.extra.get("list-style") {
        for part in raw.split_whitespace() {
            let p = part.to_ascii_lowercase();
            match p.as_str() {
                "inside" => shorthand_pos = Some(p),
                "outside" => shorthand_pos = Some(p),
                "none" => shorthand_ty = Some(p),
                "disc" | "circle" | "square" | "decimal" | "decimal-leading-zero"
                | "lower-alpha" | "upper-alpha" | "lower-roman" | "upper-roman" => {
                    shorthand_ty = Some(p)
                }
                _ => {}
            }
        }
    }
    let ty = style
        .extra
        .get("list-style-type")
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .or(shorthand_ty)
        .unwrap_or_else(|| default_ty.to_string());
    let pos = style
        .extra
        .get("list-style-position")
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .or(shorthand_pos)
        .unwrap_or(default_pos);
    (ty, pos)
}

/// `ol` 中当前 `li` 的序号(1 起,尊重 `start` 属性)。
fn ol_index(node: &NodeRef, parent: &NodeRef) -> usize {
    let mut idx = 1usize;
    for sib in parent.children() {
        if &sib == node {
            break;
        }
        if document::tag_name(&sib).to_ascii_lowercase() == "li" {
            idx += 1;
        }
    }
    if let Some(start) = document::attr(parent, "start")
        .and_then(|v| v.trim().parse::<i64>().ok())
    {
        let start = start.max(1) as usize;
        idx = start + idx - 1;
    }
    idx
}

/// inside 模式下 `li` 内容需要让位的宽度(marker 宽 + 间隙);outside 返回 0。
pub(crate) fn inside_inset(ctx: &LayoutCtx<'_>, node: &NodeRef, style: &ComputedStyle) -> f64 {
    let Some(marker) = resolve(node, style) else {
        return 0.0;
    };
    if !marker.inside {
        return 0.0;
    }
    marker_width(ctx, &marker) + MARKER_GAP
}

/// marker 的占位宽度。
fn marker_width(ctx: &LayoutCtx<'_>, marker: &Marker) -> f64 {
    match marker.kind {
        MarkerKind::Disc | MarkerKind::Circle | MarkerKind::Square => MARKER_SIZE,
        _ => {
            let text = marker_text(marker);
            ctx.font().text_width(&text, 12.0)
        }
    }
}

/// 序号类 marker 的文本。
fn marker_text(marker: &Marker) -> String {
    match marker.kind {
        MarkerKind::Decimal => {
            if marker.leading_zero {
                format!("{:02}.", marker.index)
            } else {
                format!("{}.", marker.index)
            }
        }
        MarkerKind::LowerAlpha => format!("{}.", to_alpha(marker.index)),
        MarkerKind::UpperAlpha => format!("{}.", to_alpha(marker.index).to_uppercase()),
        MarkerKind::LowerRoman => format!("{}.", to_roman(marker.index)),
        MarkerKind::UpperRoman => format!("{}.", to_roman(marker.index).to_uppercase()),
        _ => String::new(),
    }
}

/// 序号 → 小写字母(1→a, 26→z, 27→aa)。
fn to_alpha(mut n: usize) -> String {
    if n == 0 {
        return "a".to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push((b'a' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    out.reverse();
    out.into_iter().collect()
}

/// 序号 → 罗马数字(1→i, 4→iv, 9→ix)。
fn to_roman(mut n: usize) -> String {
    const TABLE: &[(usize, &str)] = &[
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (v, sym) in TABLE {
        while n >= *v {
            out.push_str(sym);
            n -= v;
        }
    }
    if out.is_empty() {
        "i".to_string()
    } else {
        out
    }
}

/// 发射 marker。
///
/// `content_x` 为 `li` 内容盒左缘,`y` 为首行顶,`h` 为行高。
pub(crate) fn emit(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    content_x: f64,
    y: f64,
    h: f64,
) {
    let Some(marker) = resolve(node, style) else {
        return;
    };
    let tf = ctx.own_transform(style, content_x, y, 20.0, h);
    if marker.inside {
        // 内容盒内:marker 画在内容左缘,与首行文字同行。
        match marker.kind {
            MarkerKind::Disc | MarkerKind::Circle | MarkerKind::Square => {
                let glyph = match marker.kind {
                    MarkerKind::Disc => WidgetGlyphKind::Bullet,
                    MarkerKind::Circle => WidgetGlyphKind::BulletRing,
                    _ => WidgetGlyphKind::BulletSquare,
                };
                ctx.emit_widget_glyph(
                    glyph,
                    false,
                    style.color,
                    content_x,
                    y + (h - MARKER_SIZE) / 2.0,
                    MARKER_SIZE,
                    MARKER_SIZE,
                    style,
                    tf,
                );
            }
            _ => {
                let text = marker_text(&marker);
                let size = style.font_size.max(8.0);
                let line = TextLine {
                    text,
                    x: content_x,
                    y: y + 1.0,
                    size,
                    color: style.color,
                    bold: false,
                    visible: super::style_visible(style),
                };
                ctx.emit_text(&line, tf, &[]);
                ctx.lines.push(line);
            }
        }
        return;
    }
    // outside(默认):marker 悬挂于内容盒左侧。
    match marker.kind {
        MarkerKind::Disc | MarkerKind::Circle | MarkerKind::Square => {
            let glyph = match marker.kind {
                MarkerKind::Disc => WidgetGlyphKind::Bullet,
                MarkerKind::Circle => WidgetGlyphKind::BulletRing,
                _ => WidgetGlyphKind::BulletSquare,
            };
            ctx.emit_widget_glyph(
                glyph,
                false,
                style.color,
                content_x - OUTSIDE_OFFSET,
                y + 2.0,
                MARKER_SIZE,
                MARKER_SIZE,
                style,
                tf,
            );
        }
        _ => {
            let text = marker_text(&marker);
            let size = style.font_size.max(8.0);
            let line = TextLine {
                text,
                x: content_x - OUTSIDE_OFFSET - 4.0,
                y: y + 1.0,
                size,
                color: style.color,
                bold: false,
                visible: super::style_visible(style),
            };
            ctx.emit_text(&line, tf, &[]);
            ctx.lines.push(line);
        }
    }
}

impl<'a> LayoutCtx<'a> {
    /// 列表项 marker(第 41 轮):类型由 `list-style-type` 决定
    /// (disc/circle/square/decimal/decimal-leading-zero/字母/罗马数字),
    /// 位置由 `list-style-position` 决定(inside/outside)。
    /// 实现在 `layout/list_marker.rs`。
    pub(crate) fn emit_list_marker(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        x: f64,
        y: f64,
        h: f64,
    ) {
        if document::tag_name(node) != "li" {
            return;
        }
        emit(self, node, style, x, y, h);
    }
}
