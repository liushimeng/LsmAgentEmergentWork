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

/// UA 默认强调色(CSS `accent-color` 缺省值,Chrome 为 `#0075FF`)。
const DEFAULT_ACCENT: [u8; 3] = [0, 117, 255];

/// 禁用态控件的灰化色(UA 皮肤)。
const DISABLED_GRAY: [u8; 3] = [150, 150, 150];

/// meter 的三段配色:最优区(绿)/次优区(琥珀)/最差区(红)。
const METER_GOOD: [u8; 3] = [80, 160, 90];
const METER_FAIR: [u8; 3] = [215, 170, 60];
const METER_POOR: [u8; 3] = [190, 75, 75];

/// select 是否为**列表形态**(`size>1` 或 `multiple`)。
///
/// 浏览器语义:`size>1` 或 `multiple` 的 select 直接铺开全部 option 行,
/// 不再有下拉箭头与浮层面板(第 44 轮)。
pub(crate) fn select_is_list(node: &NodeRef) -> bool {
    if document::attr(node, "multiple").is_some() {
        return true;
    }
    document::attr(node, "size")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|n| n > 1.0)
        .unwrap_or(false)
}

/// select 列表形态的行数(option 行 + optgroup 标题行)。
pub(crate) fn select_list_rows(node: &NodeRef) -> usize {
    let mut rows = 0usize;
    for child in node.children() {
        if child.as_element().is_none() {
            continue;
        }
        let el = child.clone();
        match document::tag_name(&el).to_ascii_lowercase().as_str() {
            "option" => rows += 1,
            "optgroup" => {
                if document::attr(&el, "label").is_some() {
                    rows += 1;
                }
                for opt in el.children() {
                    if opt.as_element().is_some()
                        && document::tag_name(&opt).eq_ignore_ascii_case("option")
                    {
                        rows += 1;
                    }
                }
            }
            _ => {}
        }
    }
    rows
}

/// 控件是否处于禁用态(`disabled` 属性;运行时状态不改写该属性)。
pub(crate) fn is_disabled(node: &NodeRef) -> bool {
    document::attr(node, "disabled").is_some()
}

/// 控件文字色:禁用态灰化(UA 皮肤),否则取元素文字色。
pub(crate) fn control_text_color(node: &NodeRef, style: &ComputedStyle) -> [u8; 3] {
    if is_disabled(node) {
        DISABLED_GRAY
    } else {
        style.color
    }
}

/// 控件强调色:`accent-color` 优先,禁用态灰化。
pub(crate) fn accent_color(node: &NodeRef, style: &ComputedStyle) -> [u8; 3] {
    if is_disabled(node) {
        DISABLED_GRAY
    } else {
        style.accent_color.unwrap_or(DEFAULT_ACCENT)
    }
}

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
    // 轨道底色:浏览器为浅灰槽 + 已填充段为强调色(第 44 轮:`accent-color`)。
    let accent = accent_color(node, style);
    ctx.push_widget_fill(style, [225, 225, 225], 255, track_x, track_y, track_w, RANGE_TRACK_H, tf);
    ctx.push_widget_fill(
        style,
        accent,
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
        accent,
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
    color: [u8; 3],
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    emit_right_icon(ctx, style, WidgetGlyphKind::CalendarIcon, color, bx, by, w, h, tf);
}

/// `input[type=time]`:右侧时钟图标。
pub(crate) fn emit_clock_icon(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    color: [u8; 3],
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    emit_right_icon(ctx, style, WidgetGlyphKind::ClockIcon, color, bx, by, w, h, tf);
}

/// 右侧图标区:统一放在控件右端,垂直居中。
fn emit_right_icon(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    glyph: WidgetGlyphKind,
    color: [u8; 3],
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let side = ICON_SIZE.min(h - 4.0).max(6.0);
    let x = bx + w - FIELD_ICON_W + (FIELD_ICON_W - side) / 2.0;
    let y = by + (h - side) / 2.0;
    ctx.emit_widget_glyph(glyph, false, color, x, y, side, side, style, tf);
}

/// `input[type=number]`:右端上下微调箭头(上箭头 + 下箭头)。
///
/// 浏览器仅在悬停/聚焦时显示微调箭头;本轮**恒显**,以保证控件类型在静态
/// 截图里可辨(确定性优先,已在方案文档标注为近似)。
pub(crate) fn emit_number_spinner(
    ctx: &mut LayoutCtx<'_>,
    style: &ComputedStyle,
    color: [u8; 3],
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
        color,
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
        color,
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
    color: [u8; 3],
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
        color,
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
    color: [u8; 3],
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
        color,
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
                    // 第 44 轮:勾/圆点取 `accent-color`(禁用态灰化)。
                    let accent = accent_color(node, style);
                    if indeterminate {
                        emit_indeterminate(ctx, node, style, accent, bx, by, w, h, tf);
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
                            accent,
                            bx + (w - side) / 2.0,
                            by + (h - side) / 2.0,
                            side,
                            side,
                            style,
                            tf,
                        );
                    } else if ty == "radio" {
                        emit_radio_ring(ctx, style, accent, bx, by, w, h, tf);
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
                    emit_number_spinner(ctx, style, color, bx, by, w, h, tf);
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
                    emit_calendar_icon(ctx, style, color, bx, by, w, h, tf);
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
                    emit_clock_icon(ctx, style, color, bx, by, w, h, tf);
                }
                // 第 41 轮:文件选择器 —— 文件名 / 「选择文件」文案。
                "file" => {
                    emit_file_picker(ctx, node, style, bx, by, w, h, tf);
                }
                // 第 42 轮:图片按钮。内核不发起真实文件选取,先以 alt/value
                // 文案保住控件语义,缺省给可见中文占位而非空边框。
                "image" => {
                    let text = document::attr(node, "alt")
                        .filter(|v| !v.trim().is_empty())
                        .or_else(|| document::attr(node, "value"))
                        .unwrap_or_else(|| "图片按钮".to_string());
                    emit_singleline_text(ctx, &text, style.color, style, bx, by, w, h, tf);
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
            // 第 44 轮:列表形态(`size>1`/`multiple`)直接铺开全部 option 行。
            if select_is_list(node) {
                emit_select_list(ctx, node, style, bx, by, w, h, tf);
                return;
            }
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
                control_text_color(node, style),
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

/// select 列表形态的一行:option 行或 optgroup 标题行。
enum SelectRow {
    /// option 行(带节点,用于登记几何)。
    Option {
        node: NodeRef,
        label: String,
        selected: bool,
        disabled: bool,
    },
    /// optgroup 标题行。
    Group(String),
}

/// select 的**列表形态**渲染(第 44 轮):逐行铺出 option,`optgroup` 出标题行,
/// `multiple` 行首带勾选位,选中行铺高亮底色。
fn emit_select_list(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
    tf: Option<Affine>,
) {
    let size = style.font_size.max(8.0);
    let row_h = (size * 1.45).max(14.0);
    let multiple = document::attr(node, "multiple").is_some();
    let marker_w = if multiple { 16.0 } else { 0.0 };
    let max_rows = (((h - 4.0) / row_h).floor() as usize).max(1);
    let text_color = control_text_color(node, style);
    let accent = accent_color(node, style);

    // 先收集行(标题行 + option 行),再统一绘制,避免闭包借用冲突。
    let mut rows: Vec<SelectRow> = Vec::new();
    for child in node.children() {
        if child.as_element().is_none() {
            continue;
        }
        let el = child.clone();
        match document::tag_name(&el).to_ascii_lowercase().as_str() {
            "option" => {
                let label = document::collapse_ws(&el.text_contents());
                let selected = document::attr(&el, "selected").is_some();
                let disabled = document::attr(&el, "disabled").is_some();
                rows.push(SelectRow::Option {
                    label,
                    selected,
                    disabled,
                    node: el,
                });
            }
            "optgroup" => {
                if let Some(label) = document::attr(&el, "label") {
                    if !label.trim().is_empty() {
                        rows.push(SelectRow::Group(label));
                    }
                }
                for opt in el.children() {
                    if opt.as_element().is_none()
                        || !document::tag_name(&opt).eq_ignore_ascii_case("option")
                    {
                        continue;
                    }
                    let label = document::collapse_ws(&opt.text_contents());
                    let selected = document::attr(&opt, "selected").is_some();
                    let disabled = document::attr(&opt, "disabled").is_some();
                    rows.push(SelectRow::Option {
                        label,
                        selected,
                        disabled,
                        node: opt.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    let mut y = by + 2.0;
    let mut drawn = 0usize;
    for row in rows.into_iter().take(max_rows) {
        match row {
            SelectRow::Group(label) => {
                // 分组标题行(浏览器为禁用态小字)。
                emit_singleline_text(
                    ctx,
                    &label,
                    [120, 120, 120],
                    style,
                    bx + 3.0,
                    y,
                    (w - 6.0).max(1.0),
                    row_h,
                    tf,
                );
            }
            SelectRow::Option {
                node: opt,
                label,
                selected,
                disabled,
            } => {
                let color = if disabled { DISABLED_GRAY } else { text_color };
                if selected {
                    // 选中行:铺高亮底色(浏览器为浅蓝)。
                    ctx.push_widget_fill(style, [214, 232, 250], 255, bx + 1.0, y, w - 2.0, row_h, tf);
                }
                if multiple {
                    // 勾选位:选中画勾,未选画方框(几何图元,避免字体缺字)。
                    let side = 12.0_f64.min(row_h - 4.0).max(8.0);
                    let mx = bx + 3.0;
                    let my = y + (row_h - side) / 2.0;
                    if selected {
                        ctx.emit_widget_glyph(
                            WidgetGlyphKind::CheckMark,
                            false,
                            accent,
                            mx,
                            my,
                            side,
                            side,
                            style,
                            tf,
                        );
                    } else {
                        ctx.push_widget_fill(style, [150, 150, 150], 255, mx, my, side, side, tf);
                    }
                }
                let text_x = bx + 3.0 + marker_w;
                let text_w = (w - 6.0 - marker_w).max(1.0);
                emit_singleline_text(ctx, &label, color, style, text_x, y, text_w, row_h, tf);
                // option 行登记几何:列表形态下 option 常驻可见,必须可点可寻址。
                ctx.boxes.push(super::ElementBox {
                    selector: super::Document::stable_selector(&opt),
                    tag: "option".to_string(),
                    x: bx + 1.0,
                    y,
                    w: w - 2.0,
                    h: row_h,
                    background: None,
                    background_alpha: 255,
                    visible: !disabled,
                    z_index: style.pos.z_index,
                    out_of_flow: false,
                });
            }
        }
        y += row_h;
        drawn += 1;
    }
    let _ = drawn;
}

pub(crate) fn input_display_text(
    node: &NodeRef,
    style: &ComputedStyle,
    ty: &str,
) -> (String, [u8; 3]) {
    // 第 44 轮:禁用态控件文字灰化(UA 皮肤)。
    let color = control_text_color(node, style);
    let value = document::attr(node, "value").unwrap_or_default();
    if !value.trim().is_empty() {
        return (value, color);
    }
    if matches!(ty, "submit" | "button" | "reset") {
        let fallback = match ty {
            "submit" => "Submit",
            "reset" => "Reset",
            _ => "Button",
        };
        return (fallback.to_string(), color);
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
    let max = if is_meter {
        super::parse_f64_attr(node, "max").unwrap_or(1.0)
    } else {
        1.0
    }
    .max(min + f64::EPSILON);
    let raw_value = super::parse_f64_attr(node, "value");
    let value = raw_value.unwrap_or(min);
    let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let track_y = by + (h * 0.30).max(1.0);
    let track_h = (h * 0.40).max(2.0);
    let track_x = bx + 2.0;
    let track_w = (w - 4.0).max(1.0);
    ctx.push_widget_fill(style, [225, 225, 225], 255, track_x, track_y, track_w, track_h, tf);
    if !is_meter {
        // 第 44 轮:`progress` 的不确定态(无 `value` 属性)与强调色。
        let accent = accent_color(node, style);
        if raw_value.is_some() {
            ctx.push_widget_fill(
                style,
                accent,
                255,
                track_x,
                track_y,
                track_w * ratio,
                track_h,
                tf,
            );
        } else {
            // 不确定态:浏览器是循环动画,静态近似为轨道居中的一段。
            let seg = (track_w * 0.3).max(2.0);
            ctx.push_widget_fill(
                style,
                accent,
                255,
                track_x + (track_w - seg) / 2.0,
                track_y,
                seg,
                track_h,
                tf,
            );
        }
        return;
    }
    // 第 44 轮:`meter` 按 `low`/`high`/`optimum` 三段配色 ——
    // 经典 HTML 渲染口径:最优值所在侧为绿,次优为琥珀,最差为红。
    let low = super::parse_f64_attr(node, "low")
        .unwrap_or(min)
        .clamp(min, max);
    let high = super::parse_f64_attr(node, "high")
        .unwrap_or(max)
        .clamp(min, max);
    let optimum = super::parse_f64_attr(node, "optimum")
        .unwrap_or((min + max) / 2.0)
        .clamp(min, max);
    // 三区:低于 low / low..high / 高于 high。
    let zone = |v: f64| -> usize {
        if v < low {
            0
        } else if v > high {
            2
        } else {
            1
        }
    };
    let zone_color = |z: usize| -> [u8; 3] {
        if optimum < low {
            // 最优值在左端:左侧绿、中间琥珀、右侧红。
            match z {
                0 => METER_GOOD,
                1 => METER_FAIR,
                _ => METER_POOR,
            }
        } else if optimum > high {
            // 最优值在右端:左侧红、中间琥珀、右侧绿。
            match z {
                0 => METER_POOR,
                1 => METER_FAIR,
                _ => METER_GOOD,
            }
        } else {
            // 最优值居中:中段绿,两侧红。
            match z {
                1 => METER_GOOD,
                _ => METER_POOR,
            }
        }
    };
    let fill = if is_disabled(node) {
        DISABLED_GRAY
    } else {
        zone_color(zone(value))
    };
    ctx.push_widget_fill(
        style,
        fill,
        255,
        track_x,
        track_y,
        track_w * ratio,
        track_h,
        tf,
    );
    // `low`/`high` 分界刻度 + `optimum` 标记(浏览器在最优值位置画一道缺口)。
    let tick = |ctx: &mut LayoutCtx<'_>, pos: f64, color: [u8; 3]| {
        let x = track_x + (track_w * pos).clamp(0.0, 1.0);
        ctx.push_widget_fill(style, color, 255, x, track_y, 1.5, track_h, tf);
    };
    if low > min {
        tick(ctx, (low - min) / (max - min), [120, 120, 120]);
    }
    if high < max {
        tick(ctx, (high - min) / (max - min), [120, 120, 120]);
    }
    let opt_pos = ((optimum - min) / (max - min)).clamp(0.0, 1.0);
    let ox = track_x + track_w * opt_pos;
    ctx.push_widget_fill(
        style,
        [40, 40, 40],
        255,
        (ox - 1.0).max(track_x),
        track_y - 1.0,
        2.0,
        track_h + 2.0,
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
    color: [u8; 3],
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    style: &ComputedStyle,
    tf: Option<Affine>,
) {
    let seq = self.next_seq();
    self.display.push(DisplayItem {
        kind: DisplayKind::WidgetGlyph { glyph, open, color },
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
