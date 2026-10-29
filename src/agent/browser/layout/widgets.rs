//! 表单控件**内置 UI** 的绘制(第 41 轮)。
//!
//! 浏览器为部分 `input` 类型提供**内置 UI**:滑块的轨道与拇指、颜色选择器的
//! 色块、日期/时间控件的日历与时钟图标、数字输入框的微调箭头、文件选择器的
//! 按钮文案。这些 UI **只发射显示项**,不改变控件盒几何 —— 控件占位尺寸仍由
//! `layout/mod.rs::inline_box_size` 的替换式固有尺寸规则决定,状态切换
//! (`checked`/`hover`/`focus`)亦不得改变占位,否则相邻元素会随状态抖动。
//!
//! 内置 UI 的几何**独立于父容器布局**:按控件盒的局部坐标计算,不参与常规流,
//! 也不会溢出控件边界(超出的部分被控件自身边框裁掉)。

use kuchikiki::NodeRef;

use super::super::document;
use super::super::render::display::{BgFill, DisplayItem, DisplayKind, WidgetGlyphKind};
use super::super::style::{values, ComputedStyle};
use super::super::style::Affine;
use super::LayoutCtx;

/// 内置 UI 图标的标准边长(像素)。
const ICON_SIZE: f64 = 14.0;

/// 数字输入框微调箭头区的宽度。
const SPINNER_W: f64 = 13.0;

/// 日期/时间类控件右侧图标区宽度。
const FIELD_ICON_W: f64 = 18.0;

/// 滑块轨道两端留白。
const RANGE_INSET: f64 = 4.0;

/// 滑块拇指宽。
const RANGE_THUMB_W: f64 = 9.0;

/// 滑块拇指高(约为轨道厚的 3 倍,视觉接近浏览器)。
const RANGE_THUMB_H: f64 = 14.0;

/// 滑块轨道厚。
const RANGE_TRACK_H: f64 = 4.0;

/// 颜色色块边长。
const SWATCH_SIZE: f64 = 18.0;

/// `input[type=range]`:轨道 + 拇指。拇指按 `min/max/value` 的比例定位。
pub(crate) fn emit_range(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let min = f64_attr(node, "min").unwrap_or(0.0);
    let max = f64_attr(node, "max").unwrap_or(100.0).max(min + f64::EPSILON);
    let value = f64_attr(node, "value").unwrap_or(min);
    let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0);

    let track_x = bx + RANGE_INSET;
    let track_w = (w - RANGE_INSET * 2.0).max(1.0);
    let track_y = by + (h - RANGE_TRACK_H) / 2.0;
    // 轨道底色:浏览器为浅灰槽 + 已填充段为强调色。
    ctx.push_widget_fill(style, [225, 225, 225], 255, track_x, track_y, track_w, RANGE_TRACK_H, tf);
    ctx.push_widget_fill(
        style,
        [70, 130, 210],
        255,
        track_x,
        track_y,
        (track_w * ratio).max(0.0),
        RANGE_TRACK_H,
        tf,
    );
    // 拇指:在可用行程内按比例定位(行程 = 轨道宽 - 拇指宽)。
    let travel = (track_w - RANGE_THUMB_W).max(0.0);
    let thumb_x = track_x + travel * ratio;
    let thumb_y = by + (h - RANGE_THUMB_H) / 2.0;
    ctx.emit_widget_glyph(
        WidgetGlyphKind::SliderThumb,
        false,
        thumb_x,
        thumb_y,
        RANGE_THUMB_W,
        RANGE_THUMB_H,
        style,
        tf,
    );
}

/// `input[type=color]`:左侧色块。颜色取值走 `style::values::parse_color`
/// (`#rgb`/`#rrggbb`/`rgb()`/`hsl()`/具名色),解析失败回落黑色(浏览器缺省)。
pub(crate) fn emit_color(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    _w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let raw = document::attr(node, "value").unwrap_or_default();
    let rgb = values::parse_color(raw.trim())
        .flatten()
        .map(|(c, _)| c)
        .unwrap_or([0, 0, 0]);
    let side = SWATCH_SIZE.min(h - 6.0).max(4.0);
    let x = bx + 3.0;
    let y = by + (h - side) / 2.0;
    ctx.push_widget_fill(style, rgb, 255, x, y, side, side, tf);
}

/// `input[type=date|datetime-local|month|week]`:右侧日历图标。
pub(crate) fn emit_calendar_icon(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    emit_right_icon(ctx, style, WidgetGlyphKind::CalendarIcon, bx, by, w, h, tf);
}

/// `input[type=time]`:右侧时钟图标。
pub(crate) fn emit_clock_icon(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    emit_right_icon(ctx, style, WidgetGlyphKind::ClockIcon, bx, by, w, h, tf);
}

/// 右侧图标区:统一放在控件右端,垂直居中。
fn emit_right_icon(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    glyph: WidgetGlyphKind,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let side = ICON_SIZE.min(h - 4.0).max(6.0);
    let x = bx + w - FIELD_ICON_W + (FIELD_ICON_W - side) / 2.0;
    let y = by + (h - side) / 2.0;
    ctx.emit_widget_glyph(glyph, false, x, y, side, side, style, tf);
}

/// `input[type=number]`:右端上下微调箭头(上箭头 + 下箭头)。
///
/// 浏览器仅在悬停/聚焦时显示微调箭头;本轮**恒显**,以保证控件类型在静态
/// 截图里可辨(确定性优先,已在方案文档标注为近似)。
pub(crate) fn emit_number_spinner(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let arrow = 5.0_f64.min(h / 4.0).max(3.0);
    let x = bx + w - SPINNER_W + (SPINNER_W - arrow) / 2.0;
    let mid = by + h / 2.0;
    ctx.emit_widget_glyph(
        WidgetGlyphKind::CaretUp,
        false,
        x,
        mid - arrow - 1.0,
        arrow,
        arrow,
        style,
        tf,
    );
    ctx.emit_widget_glyph(
        WidgetGlyphKind::Caret,
        true,
        x,
        mid + 1.0,
        arrow,
        arrow,
        style,
        tf,
    );
}

/// `input[type=file]`:有值画文件名,无值画「选择文件」文案。
pub(crate) fn emit_file_picker(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let raw = document::attr(node, "value").unwrap_or_default();
    let text = if raw.trim().is_empty() {
        "选择文件".to_string()
    } else {
        // 浏览器显示的是文件名(不含目录);内核记录的是完整路径,取末段。
        raw.split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .next_back()
            .unwrap_or(raw.as_str())
            .to_string()
    };
    emit_singleline_text(ctx, &text, style.color, style, bx, by, w, h, tf);
}

/// `input[type=checkbox]` 的 indeterminate(半选)横杠。
///
/// 半选是**运行时状态**(脚本设置 `.indeterminate = true`),内核用
/// `indeterminate` 属性承载(与 `checked` 同构的覆盖层)。
pub(crate) fn emit_indeterminate(
    ctx: &mut LayoutCtx<'_>,
    _node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let side = w.min(h).min(18.0);
    ctx.emit_widget_glyph(
        WidgetGlyphKind::Indeterminate,
        false,
        bx + (w - side) / 2.0,
        by + (h - side) / 2.0,
        side,
        side,
        style,
        tf,
    );
}

/// `input[type=radio]` 未选中时的圆环轮廓。
pub(crate) fn emit_radio_ring(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let side = w.min(h).min(18.0);
    ctx.emit_widget_glyph(
        WidgetGlyphKind::BulletRing,
        false,
        bx + (w - side) / 2.0,
        by + (h - side) / 2.0,
        side,
        side,
        style,
        tf,
    );
}

/// 读取数值属性(`min`/`max`/`value`/`step`)。
fn f64_attr(node: &NodeRef, name: &str) -> Option<f64> {
    document::attr(node, name)?.trim().parse::<f64>().ok()
}

/// 在行内表单控件盒内绘制原生控件内容。
///
/// 文本类画 value/placeholder;checkbox/radio 画几何勾/点;select 只画当前
/// option 与箭头;textarea 多行换行;progress/meter 用轨道+比例填充表达。
/// 只发射显示项,不推进游标、不改变既有盒几何。
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_form_control_text(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    tag: &str,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    match tag {
        "input" => {
            let ty = document::attr(node, "type")
                .unwrap_or_default()
                .to_ascii_lowercase();
            match ty.as_str() {
                "checkbox" | "radio" => {
                    let checked = document::attr(node, "checked").is_some();
                    // 第 41 轮:半选(indeterminate)优先于选中;未选中的
                    // radio 画圆环轮廓,让控件类型在截图里可辨。
                    let indeterminate = document::attr(node, "indeterminate").is_some();
                    if indeterminate {
                        emit_indeterminate(ctx, node, style, bx, by, w, h, tf);
                    } else if checked {
                        let glyph = if ty == "checkbox" {
                            WidgetGlyphKind::CheckMark
                        } else {
                            WidgetGlyphKind::RadioDot
                        };
                        let side = w.min(h).min(18.0);
                        ctx.emit_widget_glyph(
                            glyph,
                            false,
                            bx + (w - side) / 2.0,
                            by + (h - side) / 2.0,
                            side,
                            side,
                            style,
                            tf,
                        );
                    } else if ty == "radio" {
                        emit_radio_ring(ctx, style, bx, by, w, h, tf);
                    }
                }
                "" | "text" | "search" | "url" | "tel" | "email" | "password"
                | "submit" | "button" | "reset" => {
                    let (mut text, color) = input_display_text(node, style, &ty);
                    if ty == "password" {
                        text = "•".repeat(text.chars().count());
                    }
                    emit_singleline_text(ctx, &text, color, style, bx, by, w, h, tf);
                }
                // 第 41 轮:数字输入框 = 文本 + 右端微调箭头。
                "number" => {
                    let (mut text, color) = input_display_text(node, style, &ty);
                    if ty == "password" {
                        text = "•".repeat(text.chars().count());
                    }
                    let arrow_w = 13.0_f64.min(w * 0.3);
                    emit_singleline_text(ctx, 
                        &text,
                        color,
                        style,
                        bx,
                        by,
                        (w - arrow_w).max(1.0),
                        h,
                        tf,
                    );
                    emit_number_spinner(ctx, style, bx, by, w, h, tf);
                }
                // 第 41 轮:滑块 —— 轨道 + 拇指(内置 UI 独立计算几何)。
                "range" => {
                    emit_range(ctx, node, style, bx, by, w, h, tf);
                }
                // 第 41 轮:颜色选择器 —— 左侧色块。
                "color" => {
                    emit_color(ctx, node, style, bx, by, w, h, tf);
                }
                // 第 41 轮:日期/时间类 —— 文本 + 右侧日历/时钟图标。
                "date" | "datetime-local" | "month" | "week" => {
                    let (text, color) = input_display_text(node, style, &ty);
                    emit_singleline_text(ctx, 
                        &text,
                        color,
                        style,
                        bx,
                        by,
                        (w - 18.0).max(1.0),
                        h,
                        tf,
                    );
                    emit_calendar_icon(ctx, style, bx, by, w, h, tf);
                }
                "time" => {
                    let (text, color) = input_display_text(node, style, &ty);
                    emit_singleline_text(ctx, 
                        &text,
                        color,
                        style,
                        bx,
                        by,
                        (w - 18.0).max(1.0),
                        h,
                        tf,
                    );
                    emit_clock_icon(ctx, style, bx, by, w, h, tf);
                }
                // 第 41 轮:文件选择器 —— 文件名 / 「选择文件」文案。
                "file" => {
                    emit_file_picker(ctx, node, style, bx, by, w, h, tf);
                }
                _ => {}
            }
        }
        "button" => {
            let text = document::collapse_ws(&node.text_contents());
            let text = if text.is_empty() {
                document::attr(node, "value").unwrap_or_default()
            } else {
                text
            };
            emit_singleline_text(ctx, &text, style.color, style, bx, by, w, h, tf);
        }
        "select" => {
            let text = super::select_display_text(node);
            let caret_w = 14.0_f64.min(w * 0.25);
            emit_singleline_text(ctx, 
                &text,
                style.color,
                style,
                bx,
                by,
                (w - caret_w).max(1.0),
                h,
                tf,
            );
            let side = caret_w.min(h * 0.5).max(6.0);
            // 展开态:箭头朝上(浏览器语义),并铺出浮层 option 面板。
            let expanded = document::attr(node, "data-lsm-select-open").is_some();
            let caret = if expanded {
                WidgetGlyphKind::CaretUp
            } else {
                WidgetGlyphKind::Caret
            };
            ctx.emit_widget_glyph(
                caret,
                true,
                bx + w - caret_w + (caret_w - side) / 2.0,
                by + (h - side) / 2.0,
                side,
                side,
                style,
                tf,
            );
            super::top_layer::emit_select_panel(ctx, node, style, bx, by, w, h);
        }
        "textarea" => {
            let text = if document::attr(node, "value").is_some() {
                document::attr(node, "value").unwrap_or_default()
            } else {
                node.text_contents()
            };
            let (text, color) = if text.trim().is_empty() {
                (document::attr(node, "placeholder").unwrap_or_default(), [117, 117, 117])
            } else {
                (text, style.color)
            };
            emit_multiline_text(ctx, &text, color, style, bx, by, w, h, tf);
        }
        "progress" => emit_meter_like(ctx, node, style, bx, by, w, h, tf, false),
        "meter" => emit_meter_like(ctx, node, style, bx, by, w, h, tf, true),
        _ => {}
    }
}

fn input_display_text(
    node: &NodeRef,
    style: &ComputedStyle,
    ty: &str,
) -> (String, [u8; 3]) {
    let value = document::attr(node, "value").unwrap_or_default();
    if !value.trim().is_empty() {
        return (value, style.color);
    }
    if matches!(ty, "submit" | "button" | "reset") {
        let fallback = match ty {
            "submit" => "Submit",
            "reset" => "Reset",
            _ => "Button",
        };
        return (fallback.to_string(), style.color);
    }
    (document::attr(node, "placeholder").unwrap_or_default(), [117, 117, 117])
}

pub(crate) fn emit_singleline_text(
    ctx: &mut LayoutCtx<'_>,
    text: &str,
    color: [u8; 3],
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    if text.trim().is_empty() {
        return;
    }
    let size = style.font_size.max(8.0);
    let max_w = (w - 6.0).max(0.0);
    if max_w < size {
        return;
    }
    let mut clipped = String::new();
    let mut used = 0.0;
    for ch in text.chars() {
        let cw = ctx.font().char_width(ch, size);
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
    let line = super::TextLine {
        text: clipped,
        x: bx + 3.0,
        y: by + ((h - line_height) / 2.0).max(1.0),
        size,
        color,
        bold: style.font_weight >= 600,
        visible: super::style_visible(style),
    };
    ctx.emit_text(&line, tf, &[]);
    ctx.lines.push(line);
}

pub(crate) fn emit_multiline_text(
    ctx: &mut LayoutCtx<'_>,
    text: &str,
    color: [u8; 3],
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let size = style.font_size.max(8.0);
    let line_height = size * 1.35;
    let max_w = (w - 6.0).max(1.0);
    let max_rows = (((h - 4.0) / line_height).floor() as usize).max(1);
    let mut rows: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").lines() {
        let mut cur = String::new();
        let mut used = 0.0;
        for ch in raw.chars() {
            let cw = ctx.font().char_width(ch, size);
            if !cur.is_empty() && used + cw > max_w {
                rows.push(std::mem::take(&mut cur));
                used = 0.0;
                if rows.len() >= max_rows {
                    break;
                }
            }
            cur.push(ch);
            used += cw;
        }
        if rows.len() >= max_rows {
            break;
        }
        rows.push(cur);
        if rows.len() >= max_rows {
            break;
        }
    }
    for (i, row) in rows.into_iter().take(max_rows).enumerate() {
        let line = super::TextLine {
            text: row,
            x: bx + 3.0,
            y: by + 2.0 + i as f64 * line_height,
            size,
            color,
            bold: style.font_weight >= 600,
            visible: super::style_visible(style),
        };
        ctx.emit_text(&line, tf, &[]);
        ctx.lines.push(line);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_meter_like(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
    is_meter: bool,
) {
    let min = if is_meter {
        super::parse_f64_attr(node, "min").unwrap_or(0.0)
    } else {
        0.0
    };
    let max = super::parse_f64_attr(node, "max").unwrap_or(1.0).max(min + f64::EPSILON);
    let value = super::parse_f64_attr(node, "value").unwrap_or(min);
    let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let track_y = by + (h * 0.30).max(1.0);
    let track_h = (h * 0.40).max(2.0);
    let track_x = bx + 2.0;
    let track_w = (w - 4.0).max(1.0);
    ctx.push_widget_fill(style, [225, 225, 225], 255, track_x, track_y, track_w, track_h, tf);
    ctx.push_widget_fill(
        style,
        if is_meter { [80, 160, 90] } else { [70, 130, 210] },
        255,
        track_x,
        track_y,
        track_w * ratio,
        track_h,
        tf,
    );
}

impl<'a> LayoutCtx<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_widget_fill(
        &mut self,
    style: &ComputedStyle,
    color: [u8; 3],
    alpha: u8,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let seq = self.next_seq();
    self.display.push(DisplayItem {
        kind: DisplayKind::Background {
            fill: BgFill::Solid(color, alpha),
        },
        x,
        y,
        w,
        h,
        radius: style.border_radius,
        z: style.pos.z_index.max(self.z_floor),
        seq,
        opacity: style.opacity,
        clip: None,
        transform: tf,
        fixed: self.fixed_depth > 0,
        layer: self.layer_for(style),
    });
}

pub(crate) fn emit_widget_glyph(
    &mut self,
    glyph: WidgetGlyphKind,
    open: bool,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    style: &ComputedStyle,
    tf: Option<Affine>,
) {
    let seq = self.next_seq();
    self.display.push(DisplayItem {
        kind: DisplayKind::WidgetGlyph { glyph, open },
        x,
        y,
        w,
        h,
        radius: style.border_radius,
        z: style.pos.z_index.max(self.z_floor),
        seq,
        opacity: style.opacity,
        clip: None,
        transform: tf,
        fixed: self.fixed_depth > 0,
        layer: self.layer_for(style),
    });
}
}
