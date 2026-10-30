//! 基础表格格式化上下文(行列网格 + 自动列宽 + 受控 colspan/rowspan)。
//!
//! 第 41 轮补齐五项表格能力:
//!
//! - `caption`:表格上方的标题块(参与表格总高)。
//! - `colgroup`/`col`:显式列宽(优先级高于内容自适应)。
//! - 百分比列宽:单元格/列的 `width:30%` 相对表格内容宽。
//! - `border-collapse: collapse`:相邻单元格共享边框(单线,不双线)。
//! - `table-layout: fixed`:列宽只由 `col`/首行单元格决定,不被内容撑开。
//!
//! 仍不实现:`border-spacing`、嵌套表联合求解、`caption-side: bottom`。

use std::collections::HashMap;

use kuchikiki::NodeRef;

use super::super::document::{self, Document};
use super::super::style::ComputedStyle;
use super::{
    append, constrain_height, push_border_and_shadow, style_visible, translate, ElementBox,
    Fragment, LayoutCtx, MAX_CONTENT_WIDTH,
};

/// 列宽最小值(像素)。
const MIN_COL_W: f64 = 24.0;

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

/// 列宽提示来源。
#[derive(Debug, Clone, Copy, Default)]
struct ColHint {
    /// 显式像素宽。
    px: Option<f64>,
    /// 百分比宽(相对表格内容宽)。
    pct: Option<f64>,
}

impl ColHint {
    fn is_empty(&self) -> bool {
        self.px.is_none() && self.pct.is_none()
    }
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
    // 第 41 轮:`caption` 排在表格内容之前,占表格总高的一段。
    let caption_h = layout_caption(ctx, node, content_x, inner_w, style, depth);
    let content_h =
        layout_content(ctx, node, content_x, inner_w, style, depth + 1, cb) + caption_h;
    let box_h = constrain_height(style, (content_h + style.padding_top + style.padding_bottom).max(1.0));
    ctx.set_box_height(box_idx, box_h);
    let tag = document::tag_name(node);
    ctx.finish_container(seq, dmark, node, style, &tag, bx, start_y, bw, box_h);
    ctx.advance_y(content_h + style.padding_bottom + style.margin_bottom.max(2.0));
}

/// `caption` 布局:表格上方的标题块(居中,浏览器默认 `caption-side: top`)。
///
/// 返回 caption 占的高度。
fn layout_caption(
    ctx: &mut LayoutCtx,
    table: &NodeRef,
    content_x: f64,
    inner_w: f64,
    table_style: &ComputedStyle,
    depth: usize,
) -> f64 {
    let Some(caption) = select_nodes(table, "caption").into_iter().next() else {
        return 0.0;
    };
    let cap_style = ctx.style_for(&caption, table_style, inner_w.max(1.0));
    let top = ctx.cursor_y();
    let mut frag = ctx.measure_content(&caption, inner_w.max(1.0), &cap_style, depth + 1);
    let h = frag.height;
    translate(&mut frag, content_x, top);
    ctx.push_box(ElementBox {
        selector: Document::stable_selector(&caption),
        tag: document::tag_name(&caption),
        x: content_x,
        y: top,
        w: inner_w.max(1.0),
        h,
        background: cap_style.background,
        background_alpha: cap_style.background_alpha,
        visible: style_visible(&cap_style),
        z_index: cap_style.pos.z_index,
        out_of_flow: false,
    });
    append(ctx, frag);
    ctx.advance_y(h);
    h
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
    // 第 41 轮:表格级格式化开关。
    let collapse = style
        .extra
        .get("border-collapse")
        .map(|s| s.trim().eq_ignore_ascii_case("collapse"))
        .unwrap_or(false);
    let fixed_layout = style
        .extra
        .get("table-layout")
        .map(|s| s.trim().eq_ignore_ascii_case("fixed"))
        .unwrap_or(false);
    let mut cells: Vec<Cell> = Vec::new();
    let mut occupied: HashMap<(usize, usize), bool> = HashMap::new();
    let mut n_cols = 0usize;
    for (r, row_node) in rows.iter().enumerate() {
        let row_style = ctx.style_for(row_node, style, content_w);
        let mut c = 0usize;
        for cell_node in row_node.children() {
            let tag = document::tag_name(&cell_node);
            if tag != "td" && tag != "th" {
                continue;
            }
            let cell_style = ctx.style_for(&cell_node, &row_style, content_w);
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

    // 第 41 轮:列宽提示(colgroup/col 优先,其次首行单元格,最后内容自适应)。
    let hints = collect_col_hints(ctx, node, &cells, n_cols);

    // 两遍式自动列宽:先取单跨度 max-content,再让跨列格补齐差额。
    let mut col_w = vec![0.0f64; n_cols];
    for cell in &mut cells {
        let frag = ctx.measure_content(&cell.node, MAX_CONTENT_WIDTH, &cell.style, depth + 1);
        let inner = fragment_width(ctx, &frag);
        let (pl, pr, _, _) = cell_padding_border(&cell.style);
        cell.max_w = (inner + pl + pr).max(MIN_COL_W);
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
    // 显式列宽覆盖内容自适应;固定布局下未指定列不再按内容撑开。
    let mut fixed_col = vec![false; n_cols];
    for (i, hint) in hints.iter().enumerate() {
        if i >= n_cols {
            break;
        }
        if let Some(px) = hint.px {
            col_w[i] = px.max(MIN_COL_W);
            fixed_col[i] = true;
        } else if let Some(pct) = hint.pct {
            col_w[i] = (content_w * pct / 100.0).max(MIN_COL_W);
            fixed_col[i] = true;
        } else if fixed_layout {
            // 固定布局:未指定宽度的列按剩余空间均分(不按内容撑开)。
            col_w[i] = MIN_COL_W;
            fixed_col[i] = false;
        }
    }
    normalize_columns(&mut col_w, content_w, &fixed_col);

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
        let row_style = ctx.style_for(row_node, style, content_w);
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
        // 第 41 轮:`border-collapse: collapse` 时相邻单元格共享边框 ——
        // 每格只画 top + left,末行补 bottom、末列补 right,消除双线。
        let mut cell_style = cell.style.clone();
        if collapse {
            let last_row = cell.row + cell.row_span >= rows.len();
            let last_col = cell.col + cell.col_span >= n_cols;
            cell_style.border_drawn = [true, last_col, last_row, true];
        }
        push_border_and_shadow(ctx, &cell_style, x, y, w, h);
        ctx.emit_element(cell.seq, &cell_style, &cell.tag, x, y, w, h);
        append(ctx, frag);
    }
    acc
}

/// 收集列宽提示。
///
/// 优先级:`<colgroup><col>` 的显式宽度 > 首行单元格的显式宽度。
fn collect_col_hints(
    ctx: &mut LayoutCtx,
    table: &NodeRef,
    cells: &[Cell],
    n_cols: usize,
) -> Vec<ColHint> {
    let mut hints = vec![ColHint::default(); n_cols];
    // 1. colgroup/col
    let mut col_idx = 0usize;
    for group in select_nodes(table, "colgroup") {
        for col in select_nodes(&group, "col") {
            if col_idx >= n_cols {
                break;
            }
            let hint = parse_col_width(&col);
            if !hint.is_empty() {
                hints[col_idx] = hint;
            }
            col_idx += 1;
        }
    }
    // 未被 colgroup 覆盖的裸 <col>(表格直接子级)。
    if col_idx < n_cols {
        for col in select_nodes(table, "col") {
            if col_idx >= n_cols {
                break;
            }
            let hint = parse_col_width(&col);
            if !hint.is_empty() {
                hints[col_idx] = hint;
            }
            col_idx += 1;
        }
    }
    // 2. 首行单元格的显式宽度(补 colgroup 未覆盖的列)。
    for cell in cells.iter().filter(|c| c.row == 0 && c.col_span == 1) {
        if cell.col >= n_cols {
            continue;
        }
        if !hints[cell.col].is_empty() {
            continue;
        }
        if let Some(pct) = cell.style.width_percent {
            hints[cell.col].pct = Some(pct);
        } else if let Some(px) = cell.style.width {
            hints[cell.col].px = Some(px);
        }
    }
    let _ = ctx;
    hints
}

/// 解析 `<col>` 的宽度:`width` 属性或内联 `style="width:…"`。
fn parse_col_width(col: &NodeRef) -> ColHint {
    let mut hint = ColHint::default();
    let mut take = |raw: &str| {
        let raw = raw.trim();
        if raw.is_empty() {
            return;
        }
        if let Some(p) = raw.strip_suffix('%') {
            if let Ok(v) = p.trim().parse::<f64>() {
                hint.pct = Some(v);
                return;
            }
        }
        let v = raw.strip_suffix("px").unwrap_or(raw);
        if let Ok(v) = v.trim().parse::<f64>() {
            hint.px = Some(v);
        }
    };
    if let Some(w) = document::attr(col, "width") {
        take(&w);
    }
    if let Some(style) = document::attr(col, "style") {
        for decl in style.split(';') {
            let Some((k, v)) = decl.split_once(':') else {
                continue;
            };
            if k.trim().eq_ignore_ascii_case("width") {
                take(v);
            }
        }
    }
    hint
}

/// 收集匹配的元素节点(选择器非法时返回空集)。
fn select_nodes(node: &NodeRef, selector: &str) -> Vec<NodeRef> {
    node.select(selector)
        .map(|it| it.map(|e| e.as_node().clone()).collect::<Vec<_>>())
        .unwrap_or_default()
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

/// 列宽归一化。
///
/// - 固定列(显式像素/百分比)不参与再分配;
/// - auto 列之间按内容宽度比例消化可用宽度的富余/不足;
/// - 全为固定列时按比例缩放以适配可用宽度。
fn normalize_columns(col_w: &mut [f64], avail: f64, fixed: &[bool]) {
    if col_w.is_empty() {
        return;
    }
    for (i, w) in col_w.iter_mut().enumerate() {
        let min = if fixed.get(i).copied().unwrap_or(false) {
            MIN_COL_W
        } else {
            MIN_COL_W
        };
        *w = w.max(min);
    }
    if avail <= 0.0 {
        return;
    }
    let fixed_total: f64 = col_w
        .iter()
        .zip(fixed)
        .filter(|(_, f)| **f)
        .map(|(w, _)| *w)
        .sum();
    let auto_idx: Vec<usize> = (0..col_w.len()).filter(|i| !fixed.get(*i).copied().unwrap_or(false)).collect();
    if auto_idx.is_empty() {
        // 全固定列:超出可用宽时按比例缩放(不足则保持,表格窄于容器)。
        let total: f64 = col_w.iter().sum();
        if total > avail {
            let scale = avail / total;
            for w in col_w.iter_mut() {
                *w = (*w * scale).max(MIN_COL_W);
            }
        }
        return;
    }
    let auto_total: f64 = auto_idx.iter().map(|i| col_w[*i]).sum();
    let rest = (avail - fixed_total).max(0.0);
    if auto_total < rest {
        // 富余:在 auto 列之间均分(与既有行为一致)。
        let extra = (rest - auto_total) / auto_idx.len() as f64;
        for i in auto_idx {
            col_w[i] += extra;
        }
    } else if auto_total > rest {
        // 不足:auto 列按比例缩放。
        let scale = rest / auto_total;
        for i in auto_idx {
            col_w[i] = (col_w[i] * scale).max(MIN_COL_W);
        }
    }
}
