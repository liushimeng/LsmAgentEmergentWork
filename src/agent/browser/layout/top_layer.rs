//! 顶层(top layer)浮层(第 41 轮)。
//!
//! CSS 的顶层概念:`dialog[open]`、popover 等浮层**脱离常规层叠上下文**,
//! 不受祖先 `overflow` 裁剪,永远画在普通内容与定位元素之上。本模块落地两块:
//!
//! 1. **模态对话框**(`<dialog open>`):按 `position:fixed` 语义居中于视口,
//!    并绘制 `::backdrop` 半透明遮罩。
//! 2. **`select` 展开面板**:控件下方的浮层 option 列表。
//!
//! 两者的显示项都带 `layer = 2`(高于定位层 1),并在祖先 `overflow:hidden`
//! 的裁剪交集里被跳过(见 `layout/mod.rs::apply_group` 的裁剪逃逸规则)。

use kuchikiki::NodeRef;

use super::super::document::{self, Document};
use super::super::style::ComputedStyle;
use super::{style_visible, LayoutCtx, TextLine};

/// 顶层绘制层档位(0 = 常规流,1 = 定位元素,2 = 顶层)。
pub(crate) const TOP_LAYER: u8 = 2;

/// 模态对话框与视口四周的留白(浏览器 `dialog` 的默认最大宽留白)。
const DIALOG_VIEWPORT_INSET: f64 = 32.0;

/// `::backdrop` 遮罩的不透明度(浏览器为 0.5 左右的半透明黑)。
const BACKDROP_ALPHA: u8 = 128;

/// select 展开面板最多显示的 option 条数(超出部分滚动,内核不实现滚动)。
const PANEL_MAX_ROWS: usize = 8;

/// select 展开面板的行高。
const PANEL_ROW_H: f64 = 20.0;

/// select 展开面板的内边距。
const PANEL_PAD: f64 = 4.0;

/// 布局模态对话框(`<dialog open>`)。
///
/// 返回 `true` 表示已按顶层语义处理,调用方应跳常规块级路径。
pub(crate) fn layout_dialog(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    depth: usize,
) -> bool {
    if document::tag_name(node).to_ascii_lowercase() != "dialog" {
        return false;
    }
    if document::attr(node, "open").is_none() {
        return false;
    }
    let (vw, vh) = ctx.viewport();
    let pad_l = style.padding_left;
    let pad_r = style.padding_right;
    let pad_t = style.padding_top;
    let pad_b = style.padding_bottom;
    let avail_w = (vw - DIALOG_VIEWPORT_INSET - pad_l - pad_r).max(40.0);

    // 第一遍:按可用宽测量,取内容的自然宽度(fix-content)。
    let first = ctx.measure_content(node, avail_w, style, depth);
    let natural_w = fragment_right(&first, ctx);
    let mut frag = first;
    let mut content_w = avail_w;
    if natural_w > 1.0 && natural_w < avail_w - 1.0 {
        // 内容比可用宽窄 → 按自然宽重排(浏览器 fit-content 语义)。
        content_w = natural_w;
        frag = ctx.measure_content(node, content_w, style, depth);
    }
    let content_h = frag.height;
    let bw = content_w + pad_l + pad_r;
    let bh = (content_h + pad_t + pad_b).max(1.0);
    // 视口居中(`position:fixed; inset:0; margin:auto`)。
    let bx = ((vw - bw) / 2.0).max(0.0);
    let by = ((vh - bh) / 2.0).max(0.0);

    // 顶层绘制:遮罩在下、对话框在上。
    let prev_layer = ctx.positioned_layer;
    ctx.positioned_layer = TOP_LAYER;
    // 对话框按视口定位,不随页面滚动平移。
    ctx.fixed_depth += 1;

    // `::backdrop`:覆盖整个视口的半透明黑遮罩(在对话框之下绘制)。
    // 浏览器语义:**只有模态对话框**(`showModal()`)才有 backdrop;
    // 单纯 `open` 属性的对话框是无模态的,铺遮罩会把整页盖灰(真实站点
    // 常用 `<dialog open>` 做浮层提示,铺遮罩会毁掉整页可读性)。
    // 模态标记由页面脚本 `dialog.showModal()` 写入 `data-lsm-modal`。
    if document::attr(node, "data-lsm-modal").is_some() {
        ctx.push_widget_fill(style, [0, 0, 0], BACKDROP_ALPHA, 0.0, 0.0, vw, vh, None);
    }

    // 对话框自身盒(背景/边框)与内容。
    let seq = ctx.next_seq();
    ctx.push_box(super::ElementBox {
        selector: Document::stable_selector(node),
        tag: document::tag_name(node),
        x: bx,
        y: by,
        w: bw,
        h: bh,
        background: style.background,
        background_alpha: style.background_alpha,
        visible: style_visible(style),
        z_index: style.pos.z_index,
        out_of_flow: true,
    });
    super::push_border_and_shadow(ctx, style, bx, by, bw, bh);
    ctx.emit_element(seq, style, "dialog", bx, by, bw, bh);
    // 内容片段整体平移到对话框内容盒起点,并抬到顶层绘制层。
    super::translate(&mut frag, bx + pad_l, by + pad_t);
    for it in &mut frag.display {
        it.layer = TOP_LAYER;
        // 视口定位:不随页面滚动平移。
        it.fixed = true;
    }
    for b in &mut frag.boxes {
        b.out_of_flow = true;
    }
    super::append(ctx, frag);

    ctx.fixed_depth -= 1;
    ctx.positioned_layer = prev_layer;
    true
}

/// 片段的自然宽度(行/盒右缘的最大值)。
fn fragment_right(frag: &super::Fragment, ctx: &LayoutCtx<'_>) -> f64 {
    let mut w = 0.0f64;
    for l in &frag.lines {
        w = w.max(l.x + ctx.font().text_width(&l.text, l.size));
    }
    for b in &frag.boxes {
        if b.out_of_flow {
            continue;
        }
        w = w.max(b.x + b.w);
    }
    w
}

/// `select` 展开面板:控件下方的浮层 option 列表。
///
/// 展开态由 `Document::apply_widget_state` 写入的 `data-lsm-select-open`
/// 属性承载(内核私有标记,与 `widget_open` 覆盖层同源)。
pub(crate) fn emit_select_panel(
    ctx: &mut LayoutCtx<'_>,
    node: &NodeRef,
    style: &ComputedStyle,
    bx: f64,
    by: f64,
    w: f64,
    h: f64,
) {
    if document::attr(node, "data-lsm-select-open").is_none() {
        return;
    }
    let Ok(options) = node.select("option") else {
        return;
    };
    let options: Vec<NodeRef> = options.map(|o| o.as_node().clone()).collect();
    let labels: Vec<String> = options
        .iter()
        .map(|o| document::collapse_ws(&o.text_contents()))
        .collect();
    if labels.is_empty() {
        return;
    }
    // 选中项按 option 位置判定。运行时值经 `apply_widget_state` 已落到
    // 匹配 option 的 `selected` 属性;不能按 label 文本比较 —— 重复 label
    // 会高亮错项(两个 option 都叫「请选择」时实测必错)。
    let selected_index = options
        .iter()
        .position(|o| document::attr(o, "selected").is_some());

    let rows = labels.len().min(PANEL_MAX_ROWS);
    let panel_w = w.max(40.0);
    let panel_h = rows as f64 * PANEL_ROW_H + PANEL_PAD * 2.0;
    let panel_x = bx;
    let (_, vh) = ctx.viewport();
    let below_y = by + h;
    // 视口下缘放不下时向上翻转(浏览器原生下拉的同款策略)。
    let panel_y = if below_y + panel_h > vh {
        (by - panel_h).max(0.0)
    } else {
        below_y
    };

    let prev_layer = ctx.positioned_layer;
    ctx.positioned_layer = TOP_LAYER;

    // 边框(先画略大的深色矩形)→ 面板底色 → 选中项高亮 → 文本。
    ctx.push_widget_fill(style, [110, 110, 110], 255, panel_x - 1.0, panel_y - 1.0, panel_w + 2.0, panel_h + 2.0, None);
    ctx.push_widget_fill(style, [255, 255, 255], 255, panel_x, panel_y, panel_w, panel_h, None);
    let size = style.font_size.max(10.0);
    for (i, label) in labels.iter().take(rows).enumerate() {
        let row_y = panel_y + PANEL_PAD + i as f64 * PANEL_ROW_H;
        let selected = selected_index == Some(i);
        if selected {
            ctx.push_widget_fill(
                style,
                [0, 120, 215],
                255,
                panel_x,
                row_y,
                panel_w,
                PANEL_ROW_H,
                None,
            );
        }
        // option 行登记真实几何:面板不再是纯视觉文本,Agent 可以按 rect
        // 定位/点击浮层选项(inspect elements 与 click 使用同一份盒子)。
        ctx.boxes.push(super::ElementBox {
            selector: Document::stable_selector(&options[i]),
            tag: "option".to_string(),
            x: panel_x,
            y: row_y,
            w: panel_w,
            h: PANEL_ROW_H,
            background: style.background,
            background_alpha: style.background_alpha,
            visible: style_visible(style) && document::attr(&options[i], "disabled").is_none(),
            z_index: style.pos.z_index,
            out_of_flow: true,
        });
        let line = TextLine {
            text: label.clone(),
            x: panel_x + PANEL_PAD,
            y: row_y + (PANEL_ROW_H - size) / 2.0,
            size,
            color: if selected {
                [255, 255, 255]
            } else if document::attr(&options[i], "disabled").is_some() {
                [150, 150, 150]
            } else {
                style.color
            },
            bold: false,
            visible: style_visible(style),
        };
        ctx.emit_text(&line, None, &[]);
        ctx.lines.push(line);
    }
    ctx.positioned_layer = prev_layer;
}
