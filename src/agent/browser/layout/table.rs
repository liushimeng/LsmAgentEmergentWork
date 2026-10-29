//! 基础表格格式化上下文(行列网格 + 自动列宽 + 受控 colspan/rowspan)。
//!
//! 本轮只做「可验证的表格几何」:thead/tbody/tfoot/tr/td/th 走真实行列网格,
//! 单元格内部递归复用块级/行内/Flex/Grid 布局;不实现 border-collapse、
//! caption/colgroup、百分比列宽、table-layout:fixed 与嵌套表联合求解。

use std::collections::HashMap;

use kuchikiki::NodeRef;

use super::super::document::{self, Document};
use super::super::style::ComputedStyle;
use super::{
    append, constrain_height, push_border_and_shadow, style_visible, translate, ElementBox,
    Fragment, LayoutCtx, MAX_CONTENT_WIDTH,
};

struct Cell {
    node: NodeRef,
    style: ComputedStyle,
    selector: String,
    tag: String,
    row: usize,
    col: usize,
    row_span: usize,
    col_span: usize,
    seq: u64,
    max_w: f64,
    outer_h: f64,
    frag: Option<Fragment>,
}

/// 表格容器入口:布局子树并回填表格整体盒。
pub(crate) fn layout_container(
    ctx: &mut LayoutCtx,
    node: &NodeRef,
    x: f64,
    width: f64,
    depth: usize,
    style: &ComputedStyle,
    cb: (f64, f64, f64, f64),
) {
    ctx.advance_y(style.margin_top);
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
    if style.box_sizing_content && style.width.is_some() {
        bw += style.padding_left + style.padding_right;
    }
    let start_y = ctx.cursor_y();
    ctx.advance_y(style.padding_top);
    let inner_w = (bw - style.padding_left - style.padding_right).max(0.0);
    let content_x = bx + style.padding_left;
    let seq = ctx.next_seq();
    let dmark = ctx.display_len();
    ctx.push_box(ElementBox {
        selector: Document::stable_selector(node),
        tag: document::tag_name(node),
        x: bx,
        y: start_y,
        w: bw,
        h: 1.0,
        background: style.background,
        background_alpha: style.background_alpha,
        visible: style_visible(style),
        z_index: style.pos.z_index,
        out_of_flow: false,
    });
    let box_idx = ctx.boxes_len() - 1;
    let content_h = layout_content(ctx, node, content_x, inner_w, style, depth + 1, cb);
    let box_h = constrain_height(style, (content_h + style.padding_top + style.padding_bottom).max(1.0));
    ctx.set_box_height(box_idx, box_h);
    let tag = document::tag_name(node);
    ctx.finish_container(seq, dmark, node, style, &tag, bx, start_y, bw, box_h);
    ctx.advance_y(content_h + style.padding_bottom + style.margin_bottom.max(2.0));
}

/// 表格内容区布局:返回内容高度(不推进全局游标,容器入口负责收口)。
pub(crate) fn layout_content(
    ctx: &mut LayoutCtx,
    node: &NodeRef,
    content_x: f64,
    content_w: f64,
    style: &ComputedStyle,
    depth: usize,
    _cb: (f64, f64, f64, f64),
) -> f64 {
    if depth > 60 {
        return 0.0;
    }
    let rows = collect_rows(node);
    if rows.is_empty() {
        return 0.0;
    }
    let mut cells: Vec<Cell> = Vec::new();
    let mut occupied: HashMap<(usize, usize), bool> = HashMap::new();
    let mut n_cols = 0usize;
    for (r, row_node) in rows.iter().enumerate() {
        let row_style = ctx.style_for(row_node, style);
        let mut c = 0usize;
        for cell_node in row_node.children() {
            let tag = document::tag_name(&cell_node);
            if tag != "td" && tag != "th" {
                continue;
            }
            let cell_style = ctx.style_for(&cell_node, &row_style);
            let col_span = clamp_span(document::attr(&cell_node, "colspan"), 16);
            let row_span = clamp_span(document::attr(&cell_node, "rowspan"), rows.len() - r);
            while occupied.get(&(r, c)).copied().unwrap_or(false) {
                c += 1;
            }
            let selector = Document::stable_selector(&cell_node);
            cells.push(Cell {
                node: cell_node.clone(),
                style: cell_style,
                selector,
                tag,
                row: r,
                col: c,
                row_span,
                col_span,
                seq: 0,
                max_w: 0.0,
                outer_h: 0.0,
                frag: None,
            });
            for rr in r..(r + row_span).min(rows.len()) {
                for cc in c..c + col_span {
                    occupied.insert((rr, cc), true);
                }
            }
            c += col_span;
            n_cols = n_cols.max(c);
        }
    }
    if cells.is_empty() || n_cols == 0 {
        return 0.0;
    }

    // 两遍式自动列宽:先取单跨度 max-content,再让跨列格补齐差额。
    let mut col_w = vec![0.0f64; n_cols];
    for cell in &mut cells {
        let frag = ctx.measure_content(&cell.node, MAX_CONTENT_WIDTH, &cell.style, depth + 1);
        let inner = fragment_width(ctx, &frag);
        let (pl, pr, _, _) = cell_padding_border(&cell.style);
        cell.max_w = (inner + pl + pr).max(24.0);
    }
    for cell in cells.iter().filter(|c| c.col_span == 1) {
        col_w[cell.col] = col_w[cell.col].max(cell.max_w);
    }
    for cell in cells.iter().filter(|c| c.col_span > 1) {
        let end = (cell.col + cell.col_span).min(n_cols);
        let cur: f64 = col_w[cell.col..end].iter().sum();
        if cell.max_w > cur {
            let extra = (cell.max_w - cur) / (end - cell.col) as f64;
            for w in &mut col_w[cell.col..end] {
                *w += extra;
            }
        }
    }
    normalize_columns(&mut col_w, content_w);

    // 最终宽度下逐格测量,得到内容片段与外高。
    for cell in &mut cells {
        let end = (cell.col + cell.col_span).min(n_cols);
        let outer_w: f64 = col_w[cell.col..end].iter().sum();
        let (pl, pr, pt, pb) = cell_padding_border(&cell.style);
        let inner_w = (outer_w - pl - pr).max(1.0);
        cell.seq = ctx.next_seq();
        let frag = ctx.measure_content(&cell.node, inner_w, &cell.style, depth + 1);
        cell.outer_h = (frag.height + pt + pb).max(1.0);
        cell.frag = Some(frag);
    }

    // 行高:先吸收单行格,再把跨行格的超出量摊到占位行。
    let mut row_h = vec![0.0f64; rows.len()];
    for cell in cells.iter().filter(|c| c.row_span == 1) {
        row_h[cell.row] = row_h[cell.row].max(cell.outer_h);
    }
    for cell in cells.iter().filter(|c| c.row_span > 1) {
        let end = (cell.row + cell.row_span).min(rows.len());
        let cur: f64 = row_h[cell.row..end].iter().sum();
        if cell.outer_h > cur {
            let extra = (cell.outer_h - cur) / (end - cell.row) as f64;
            for h in &mut row_h[cell.row..end] {
                *h += extra;
            }
        }
    }
    for h in &mut row_h {
        *h = h.max(1.0);
    }

    let top = ctx.cursor_y();
    let total_w: f64 = col_w.iter().sum();
    let mut row_y = vec![0.0f64; rows.len()];
    let mut acc = 0.0;
    for (i, h) in row_h.iter().enumerate() {
        row_y[i] = top + acc;
        acc += h;
    }
    // 行盒用于 inspect/点击寻址;自身无背景时只作几何锚点。
    for (r, row_node) in rows.iter().enumerate() {
        let row_style = ctx.style_for(row_node, style);
        ctx.push_box(ElementBox {
            selector: Document::stable_selector(row_node),
            tag: document::tag_name(row_node),
            x: content_x,
            y: row_y[r],
            w: total_w,
            h: row_h[r],
            background: row_style.background,
            background_alpha: row_style.background_alpha,
            visible: style_visible(&row_style),
            z_index: row_style.pos.z_index,
            out_of_flow: false,
        });
    }

    for cell in &mut cells {
        let endc = (cell.col + cell.col_span).min(n_cols);
        let x = content_x + col_w[..cell.col].iter().sum::<f64>();
        let w: f64 = col_w[cell.col..endc].iter().sum();
        let endr = (cell.row + cell.row_span).min(rows.len());
        let y = row_y[cell.row];
        let h: f64 = row_h[cell.row..endr].iter().sum();
        let (pl, _pr, pt, pb) = cell_padding_border(&cell.style);
        let mut frag = cell.frag.take().unwrap_or_default();
        let align = cell
            .style
            .extra
            .get("vertical-align")
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let inner_h = (h - pt - pb).max(0.0);
        let dy = match align.as_str() {
            "middle" => ((inner_h - frag.height) / 2.0).max(0.0),
            "bottom" => (inner_h - frag.height).max(0.0),
            _ => 0.0,
        };
        translate(&mut frag, x + pl, y + pt + dy);
        ctx.push_box(ElementBox {
            selector: cell.selector.clone(),
            tag: cell.tag.clone(),
            x,
            y,
            w,
            h,
            background: cell.style.background,
            background_alpha: cell.style.background_alpha,
            visible: style_visible(&cell.style),
            z_index: cell.style.pos.z_index,
            out_of_flow: false,
        });
        push_border_and_shadow(ctx, &cell.style, x, y, w, h);
        ctx.emit_element(cell.seq, &cell.style, &cell.tag, x, y, w, h);
        append(ctx, frag);
    }
    acc
}

fn collect_rows(table: &NodeRef) -> Vec<NodeRef> {
    let mut rows = Vec::new();
    for child in table.children() {
        let tag = document::tag_name(&child);
        if tag == "tr" {
            rows.push(child.clone());
        } else if matches!(tag.as_str(), "thead" | "tbody" | "tfoot") {
            for row in child.children() {
                if document::tag_name(&row) == "tr" {
                    rows.push(row.clone());
                }
            }
        }
    }
    rows
}

fn clamp_span(raw: Option<String>, max: usize) -> usize {
    let n = raw
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, max.max(1));
    n
}

fn cell_padding_border(style: &ComputedStyle) -> (f64, f64, f64, f64) {
    (
        style.padding_left + style.border_widths[3],
        style.padding_right + style.border_widths[1],
        style.padding_top + style.border_widths[0],
        style.padding_bottom + style.border_widths[2],
    )
}

fn fragment_width(ctx: &LayoutCtx, frag: &Fragment) -> f64 {
    let mut w = 0.0f64;
    for l in &frag.lines {
        w = w.max(l.x + ctx.font().text_width(&l.text, l.size));
    }
    for b in &frag.boxes {
        if !b.out_of_flow {
            w = w.max(b.x + b.w);
        }
    }
    w
}

fn normalize_columns(col_w: &mut [f64], avail: f64) {
    if col_w.is_empty() {
        return;
    }
    let min = 24.0f64;
    for w in col_w.iter_mut() {
        *w = w.max(min);
    }
    let total: f64 = col_w.iter().sum();
    if avail <= 0.0 {
        return;
    }
    if total < avail {
        let extra = (avail - total) / col_w.len() as f64;
        for w in col_w.iter_mut() {
            *w += extra;
        }
    } else if total > avail {
        let scale = avail / total;
        for w in col_w.iter_mut() {
            *w = (*w * scale).max(min);
        }
    }
}
