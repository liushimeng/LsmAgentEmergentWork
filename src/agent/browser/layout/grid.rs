//! Grid 网格布局(阶段三第二步)。
//!
//! 覆盖:显式/隐式网格、轨道尺寸(px/%/fr/auto/min-content/max-content/minmax/repeat)、
//! 自动放置(sparse)+ 显式放置(线号/span)、gap、条目/轨道级对齐。
//! 已知边界(见设计文档 09):dense 不支持;fr 内禀下限取 0;min-content 取最长单词宽;
//! 命名线/`grid-template-areas` 丢弃;% 行高在不定高容器退化 auto;
//! 负线号按显式轨道数折算;多跨度条目不参与轨道内禀生长。

use kuchikiki::NodeRef;

use super::super::document::{self, Document};
use super::super::style::{ComputedStyle, GridFlow, GridLine, TrackSize};
use super::flex::text_fragment;
use super::{
    append, style_visible, translate, ElementBox, Fragment, LayoutCtx, PaintBox, PaintKind,
    MAX_CONTENT_WIDTH,
};

/// Grid 条目:元素子节点或匿名文本段。
struct Item {
    node: Option<NodeRef>,
    style: ComputedStyle,
    text: Option<String>,
    selector: String,
    tag: String,
    /// 放置结果(0 基轨道下标)。
    col: usize,
    row: usize,
    col_span: usize,
    row_span: usize,
    /// 替换元素图片地址(阶段三第三步;仅 `<img>` 条目非空)。
    image: Option<String>,
    /// 内容片段(列宽确定后测量;定位时平移注入)。
    frag: Option<Fragment>,
}

impl Item {
    fn margins(&self) -> (f64, f64, f64, f64) {
        let s = &self.style;
        (s.margin_top, s.margin_right, s.margin_bottom, s.margin_left)
    }
    fn pads(&self) -> (f64, f64, f64, f64) {
        let s = &self.style;
        (s.padding_top, s.padding_right, s.padding_bottom, s.padding_left)
    }
}

fn element_item(node: &NodeRef, cs: ComputedStyle, image: Option<String>) -> Item {
    Item {
        selector: Document::stable_selector(node),
        tag: document::tag_name(node),
        node: Some(node.clone()),
        style: cs,
        text: None,
        image,
        col: 0,
        row: 0,
        col_span: 1,
        row_span: 1,
        frag: None,
    }
}

/// 匿名文本条目:样式继承容器(近似,与 flex 一致)。
fn anon_item(text: &str, parent: &ComputedStyle) -> Item {
    Item {
        node: None,
        style: parent.clone(),
        text: Some(text.to_string()),
        selector: String::new(),
        tag: String::new(),
        image: None,
        col: 0,
        row: 0,
        col_span: 1,
        row_span: 1,
        frag: None,
    }
}

/// 容器自身按块级惯例定位;背景盒先占位(保证条目绘制在容器背景之上),
/// 内容区交给 Grid 算法,结束后回填容器盒高度。
/// 调用方在进入本函数前已清空行内缓冲(与块级/flex 分支一致)。
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
    // 显式 content-box:width 即内容盒宽,padding 加回边框盒(见 mod.rs 同名注释)。
    if style.box_sizing_content && style.width.is_some() {
        bw += style.padding_left + style.padding_right;
    }
    let start_y = ctx.cursor_y();
    ctx.advance_y(style.padding_top);
    let inner_w = (bw - style.padding_left - style.padding_right).max(0.0);
    let content_x = bx + style.padding_left;
    // 阶段六:容器先序 seq 与子树显示项标记(组效果回补区间)。
    let seq = ctx.next_seq();
    let dmark = ctx.display_len();
    // 背景盒先占位,高度最后回填。
    ctx.push_box(ElementBox {
        selector: Document::stable_selector(node),
        tag: document::tag_name(node),
        x: bx,
        y: start_y,
        w: bw,
        h: 1.0,
        background: style.background,
        visible: style_visible(style),
        z_index: style.pos.z_index,
    });
    let box_idx = ctx.boxes_len() - 1;
    let content_h = layout_content(ctx, node, content_x, inner_w, style, depth + 1, cb);
    let box_h = (ctx.cursor_y() - start_y + content_h).max(1.0);
    ctx.set_box_height(box_idx, box_h);
    // 阶段六:组效果回补 + 自身视觉发射(含多层背景图,修复 grid 容器边框/阴影缺口)。
    let tag = document::tag_name(node);
    ctx.finish_container(seq, dmark, style, &tag, bx, start_y, bw, box_h);
    ctx.advance_y(content_h + style.padding_bottom + style.margin_bottom.max(2.0));
}

/// Grid 内容区布局:放置 → 列轨道 → 行轨道 → 摆放输出,返回内容高度。
/// `measure_content` 对 grid 容器条目复用本入口(嵌套格式化上下文)。
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
    let (mut items, deferred) = collect_items(ctx, node, style);
    if items.is_empty() {
        if !deferred.is_empty() {
            let pad_cb = (
                content_x - style.padding_left,
                ctx.cursor_y() - style.padding_top,
                content_w + style.padding_left + style.padding_right,
                super::UNKNOWN_CB_H,
            );
            super::position::layout_deferred(ctx, deferred, pad_cb, depth);
        }
        return 0.0;
    }

    // 1. 放置(显式 + 自动 sparse),得到轨道规模。
    place_items(&mut items, style);
    let n_cols = items
        .iter()
        .map(|i| i.col + i.col_span)
        .max()
        .unwrap_or(1)
        .max(style.grid.template_cols.len())
        .max(1);
    let n_rows = items
        .iter()
        .map(|i| i.row + i.row_span)
        .max()
        .unwrap_or(1)
        .max(style.grid.template_rows.len())
        .max(1);

    // 2. 列轨道尺寸(fr/百分比按内容宽)。
    let col_intr = axis_intrinsics(ctx, &items, true, n_cols);
    let cols = size_tracks(
        &style.grid.template_cols,
        &style.grid.auto_cols,
        n_cols,
        content_w,
        true,
        style.grid.col_gap,
        &col_intr,
    );
    let col_offsets = track_offsets(
        &cols,
        style.grid.col_gap,
        content_w,
        &style.grid.justify_content,
    );

    // 3. 条目列宽确定后测量内容,行轨道按内容高生长。
    measure_items(ctx, &mut items, &cols, style, depth);
    let row_intr = axis_intrinsics(ctx, &items, false, n_rows);
    // 行 `fr`/百分比在不定高容器下退化为内容高(见设计文档边界)。
    let definite_h = style.height.is_some();
    let rows = size_tracks(
        &style.grid.template_rows,
        &style.grid.auto_rows,
        n_rows,
        style
            .height
            .map(|h| (h - style.padding_top - style.padding_bottom).max(0.0))
            .unwrap_or(0.0),
        definite_h,
        style.grid.row_gap,
        &row_intr,
    );
    let total_h: f64 =
        rows.iter().sum::<f64>() + style.grid.row_gap * n_rows.saturating_sub(1) as f64;
    let avail_h = if definite_h {
        style
            .height
            .map(|h| (h - style.padding_top - style.padding_bottom).max(0.0))
            .unwrap_or(0.0)
            .max(total_h)
    } else {
        total_h
    };
    let row_offsets = track_offsets(&rows, style.grid.row_gap, avail_h, &style.grid.align_content);

    // 4. 摆放与输出。
    let base_y = ctx.cursor_y();
    for item in &mut items {
        let area_x = col_offsets[item.col.min(col_offsets.len() - 1)];
        let area_w: f64 = cols[item.col..item.col + item.col_span.min(cols.len() - item.col)]
            .iter()
            .sum::<f64>()
            + style.grid.col_gap * item.col_span.saturating_sub(1) as f64;
        let area_y = row_offsets[item.row.min(row_offsets.len() - 1)];
        let area_h: f64 = rows[item.row..item.row + item.row_span.min(rows.len() - item.row)]
            .iter()
            .sum::<f64>()
            + style.grid.row_gap * item.row_span.saturating_sub(1) as f64;
        emit_item(
            ctx,
            item,
            content_x + area_x,
            base_y + area_y,
            area_w,
            area_h,
            style,
        );
    }

    // 5. 脱离流定位元素最后布置。
    if !deferred.is_empty() {
        let pad_cb = (
            content_x - style.padding_left,
            ctx.cursor_y() - style.padding_top,
            content_w + style.padding_left + style.padding_right,
            super::UNKNOWN_CB_H,
        );
        super::position::layout_deferred(ctx, deferred, pad_cb, depth);
    }
    avail_h.max(total_h)
}

/// 收集条目与脱离流定位元素(与 flex 同构)。
fn collect_items(
    ctx: &mut LayoutCtx,
    node: &NodeRef,
    style: &ComputedStyle,
) -> (Vec<Item>, Vec<(NodeRef, ComputedStyle)>) {
    let mut items = Vec::new();
    let mut deferred = Vec::new();
    let mut pending_text = String::new();
    for child in node.children() {
        if let Some(text) = child.as_text() {
            let raw = text.borrow();
            let t = raw.trim();
            if !t.is_empty() {
                if !pending_text.is_empty() {
                    pending_text.push(' ');
                }
                pending_text.push_str(t);
            }
            continue;
        }
        if child.as_element().is_none() {
            continue;
        }
        let cs = ctx.style_for(&child, style);
        if cs.display == "none" {
            continue;
        }
        if matches!(cs.pos.position.as_str(), "absolute" | "fixed") {
            deferred.push((child.clone(), cs));
            continue;
        }
        if !pending_text.is_empty() {
            items.push(anon_item(&pending_text, style));
            pending_text.clear();
        }
        // 阶段三第三步:`<img>` 条目记录图片地址(按固有尺寸参与轨道度量)。
        let image = if document::tag_name(&child).eq_ignore_ascii_case("img") {
            ctx.image_src(&child)
        } else {
            None
        };
        items.push(element_item(&child, cs, image));
    }
    if !pending_text.is_empty() {
        items.push(anon_item(&pending_text, style));
    }
    (items, deferred)
}

// ===================== 放置 =====================

/// 一轴放置解析:返回 (0 基起点?, 跨度);`auto` 线参与自动放置。
fn resolve_axis(start: GridLine, end: GridLine, explicit_len: usize) -> (Option<usize>, usize) {
    let neg = |n: i64| -> usize { (explicit_len as i64 + n).max(0) as usize };
    let start_pos: Option<usize> = match start {
        GridLine::Int(n) if n > 0 => Some((n - 1) as usize),
        GridLine::Int(n) => Some(neg(n)),
        _ => None,
    };
    let span = match (start, end) {
        (GridLine::Span(n), _) => n.max(1) as usize,
        (_, GridLine::Span(n)) => n.max(1) as usize,
        (GridLine::Int(_), GridLine::Int(e)) => {
            let s0 = start_pos.unwrap_or(0);
            let e0 = if e > 0 {
                (e - 1) as usize
            } else {
                neg(e)
            };
            e0.saturating_sub(s0).max(1)
        }
        _ => 1,
    };
    (start_pos, span)
}

/// 显式放置的条目是否至少一轴定值。
fn has_explicit(gi: &ComputedStyle) -> bool {
    let it = &gi.grid_item;
    !matches!(it.col_start, GridLine::Auto)
        || !matches!(it.col_end, GridLine::Auto)
        || !matches!(it.row_start, GridLine::Auto)
        || !matches!(it.row_end, GridLine::Auto)
}

/// 放置全部条目(显式先落位 → 自动 sparse 游标)。
fn place_items(items: &mut [Item], style: &ComputedStyle) {
    let template_cols = style.grid.template_cols.len();
    let template_rows = style.grid.template_rows.len();
    let mut occ: Vec<Vec<bool>> = Vec::new();
    let mut placed = vec![false; items.len()];
    // 显式网格边界(模板 + 显式放置延伸),自动放置换行以它为限。
    let mut explicit_max_col = template_cols;
    let mut explicit_max_row = template_rows;

    // 1. 显式条目先落位,建立占用表与显式网格边界。
    for (idx, item) in items.iter_mut().enumerate() {
        if !has_explicit(&item.style) {
            continue;
        }
        let gi = &item.style.grid_item;
        let (col, col_span) = resolve_axis(gi.col_start, gi.col_end, template_cols);
        let (row, row_span) = resolve_axis(gi.row_start, gi.row_end, template_rows);
        item.col_span = col_span;
        item.row_span = row_span;
        match (col, row) {
            (Some(c), Some(r)) => {
                item.col = c;
                item.row = r;
            }
            (Some(c), None) => {
                // 列锁死,行从 0 起找最小空位(近似 sparse)。
                item.col = c;
                item.row = first_free_line(&occ, true, c, col_span, row_span);
            }
            (None, Some(r)) => {
                item.row = r;
                item.col = first_free_line(&occ, false, r, row_span, col_span);
            }
            (None, None) => continue,
        }
        occupy(&mut occ, item.row, item.col, item.row_span, item.col_span);
        explicit_max_col = explicit_max_col.max(item.col + item.col_span);
        explicit_max_row = explicit_max_row.max(item.row + item.row_span);
        placed[idx] = true;
    }

    // 2. 自动放置(sparse:游标单调前进,不回头)。
    let row_flow = style.grid.auto_flow == GridFlow::Row;
    // 行优先时游标沿列推进、换行受列数限制;列优先对调。显式模板为 0 时
    // 限制取 1(浏览器一致:无模板 `display:grid` 单列堆叠 / `auto-flow:column` 单行横排)。
    let wrap = if row_flow {
        explicit_max_col.max(1)
    } else {
        explicit_max_row.max(1)
    };
    let (mut major, mut minor) = (0usize, 0usize);
    for (idx, item) in items.iter_mut().enumerate() {
        if placed[idx] {
            continue;
        }
        let (minor_span, major_span) = if row_flow {
            (item.col_span.min(wrap), item.row_span)
        } else {
            (item.row_span.min(wrap), item.col_span)
        };
        if row_flow {
            item.col_span = minor_span;
        } else {
            item.row_span = minor_span;
        }
        loop {
            if minor + minor_span > wrap {
                major += 1;
                minor = 0;
                continue;
            }
            let (row, col) = if row_flow {
                (major, minor)
            } else {
                (minor, major)
            };
            let (row_span, col_span) = if row_flow {
                (major_span, minor_span)
            } else {
                (minor_span, major_span)
            };
            if is_free(&occ, row, col, row_span, col_span) {
                item.row = row;
                item.col = col;
                item.row_span = row_span;
                item.col_span = col_span;
                occupy(&mut occ, row, col, row_span, col_span);
                minor += minor_span;
                break;
            }
            minor += 1;
        }
    }
}

fn occupy(occ: &mut Vec<Vec<bool>>, row: usize, col: usize, row_span: usize, col_span: usize) {
    while occ.len() < row + row_span {
        occ.push(Vec::new());
    }
    for r in occ.iter_mut().skip(row).take(row_span) {
        while r.len() < col + col_span {
            r.push(false);
        }
        for c in r.iter_mut().skip(col).take(col_span) {
            *c = true;
        }
    }
}

fn is_free(occ: &[Vec<bool>], row: usize, col: usize, row_span: usize, col_span: usize) -> bool {
    for r in row..row + row_span {
        for c in col..col + col_span {
            if occ
                .get(r)
                .and_then(|rr| rr.get(c))
                .copied()
                .unwrap_or(false)
            {
                return false;
            }
        }
    }
    true
}

/// 一轴定值、一轴 auto:找另一轴最小可放置线号(0 基)。
/// `fixed_is_col=true` 时 `fixed=(起点, 跨度)` 描述列,扫描行;否则对调。
fn first_free_line(
    occ: &[Vec<bool>],
    fixed_is_col: bool,
    fixed: usize,
    fixed_span: usize,
    other_span: usize,
) -> usize {
    for other in 0..1024 {
        let (row, col, row_span, col_span) = if fixed_is_col {
            (other, fixed, other_span, fixed_span)
        } else {
            (fixed, other, fixed_span, other_span)
        };
        if is_free(occ, row, col, row_span, col_span) {
            return other;
        }
    }
    0
}

// ===================== 轨道尺寸 =====================

/// 每轨道内禀 (min-content, max-content) 外尺寸;仅统计单跨度条目。
fn axis_intrinsics(
    ctx: &mut LayoutCtx,
    items: &[Item],
    is_col: bool,
    n: usize,
) -> Vec<(f64, f64)> {
    let mut out = vec![(0.0f64, 0.0f64); n];
    for item in items {
        let (start, span) = if is_col {
            (item.col, item.col_span)
        } else {
            (item.row, item.row_span)
        };
        if span != 1 || start >= n {
            continue;
        }
        let (_, mr, _, ml) = item.margins();
        let (pt, pr, pb, pl) = item.pads();
        let (max_c, min_c) = if is_col {
            (
                max_content_width(ctx, item) + pl + pr + ml + mr,
                min_content_width(ctx, item) + pl + pr + ml + mr,
            )
        } else {
            let h = item.frag.as_ref().map(|f| f.height).unwrap_or(0.0) + pt + pb;
            (h + mt(item), h + mt(item))
        };
        let e = &mut out[start];
        e.0 = e.0.max(min_c);
        e.1 = e.1.max(max_c);
    }
    out
}

fn mt(item: &Item) -> f64 {
    item.style.margin_top + item.style.margin_bottom
}

/// max-content 主尺寸(列向:内容最大宽)。
fn max_content_width(ctx: &mut LayoutCtx, item: &Item) -> f64 {
    match &item.text {
        Some(text) => ctx.font().text_width(text, item.style.font_size),
        None => {
            let cs = &item.style;
            let node = item.node.as_ref().unwrap();
            let frag = ctx.measure_content(node, MAX_CONTENT_WIDTH, cs, 1);
            let mut w = 0.0f64;
            for l in &frag.lines {
                let lw = ctx.font().text_width(&l.text, l.size);
                w = w.max(l.x + lw);
            }
            for b in &frag.boxes {
                w = w.max(b.x + b.w);
            }
            w
        }
    }
}

/// min-content 主尺寸:最长单词宽(嵌套盒不递归,见设计文档边界)。
fn min_content_width(ctx: &mut LayoutCtx, item: &Item) -> f64 {
    // 阶段三第三步:图片条目的 min-content 即固有宽度。
    if let Some(src) = &item.image {
        return ctx.images().natural(src).map(|(w, _)| w).unwrap_or(0.0);
    }
    let size = item.style.font_size;
    let text_owned;
    let text: &str = match &item.text {
        Some(t) => t.as_str(),
        None => {
            text_owned = item
                .node
                .as_ref()
                .map(|n| n.text_contents())
                .unwrap_or_default();
            text_owned.as_str()
        }
    };
    text.split_whitespace()
        .map(|w| ctx.font().text_width(w, size))
        .fold(0.0f64, f64::max)
}

/// 轨道基础尺寸 → fr/余量分配。
fn size_tracks(
    template: &[TrackSize],
    implicit: &TrackSize,
    n: usize,
    avail: f64,
    definite: bool,
    gap: f64,
    intr: &[(f64, f64)],
) -> Vec<f64> {
    let track_of = |i: usize| -> &TrackSize {
        if i < template.len() {
            &template[i]
        } else {
            implicit
        }
    };
    let mut sizes = Vec::with_capacity(n);
    let mut fr_total = 0.0f64;
    for i in 0..n {
        let t = track_of(i);
        let (min_c, max_c) = intr.get(i).copied().unwrap_or((0.0, 0.0));
        let base = match t {
            TrackSize::Px(v) => *v,
            TrackSize::Percent(p) => {
                if definite {
                    p / 100.0 * avail
                } else {
                    max_c
                }
            }
            TrackSize::Fr(f) => {
                fr_total += *f;
                0.0
            }
            TrackSize::MinContent => min_c,
            TrackSize::MaxContent | TrackSize::Auto => max_c,
            TrackSize::MinMax(lo, hi) => {
                let lo_v = match lo.as_ref() {
                    TrackSize::Px(v) => *v,
                    TrackSize::Percent(p) if definite => p / 100.0 * avail,
                    TrackSize::MinContent => min_c,
                    _ => 0.0,
                };
                let hi_v = match hi.as_ref() {
                    TrackSize::Px(v) => *v,
                    TrackSize::Percent(p) if definite => p / 100.0 * avail,
                    TrackSize::MinContent => min_c,
                    _ => max_c,
                };
                max_c.clamp(lo_v, hi_v.max(lo_v))
            }
        };
        sizes.push(base);
    }
    let gaps = gap * n.saturating_sub(1) as f64;
    let free = if definite {
        avail - sizes.iter().sum::<f64>() - gaps
    } else {
        0.0
    };
    if free > 0.0 {
        if fr_total > 0.0 {
            for i in 0..n {
                if let TrackSize::Fr(f) = track_of(i) {
                    sizes[i] += free * f / fr_total;
                }
            }
        } else {
            // `normal/stretch`:auto 类轨道均分余量(MinMax 封顶)。
            let grow_idx: Vec<usize> = (0..n)
                .filter(|&i| matches!(track_of(i), TrackSize::Auto | TrackSize::MinMax(_, _)))
                .collect();
            if !grow_idx.is_empty() {
                let share = free / grow_idx.len() as f64;
                for &i in &grow_idx {
                    let cap = match track_of(i) {
                        TrackSize::MinMax(_, hi) => match hi.as_ref() {
                            TrackSize::Px(v) => Some(*v),
                            _ => None,
                        },
                        _ => None,
                    };
                    sizes[i] += cap
                        .map(|c| share.min((c - sizes[i]).max(0.0)))
                        .unwrap_or(share);
                }
            }
        }
    }
    sizes
}

/// 轨道起点偏移表(含间隙与轨道级对齐分布),长度 = n。
fn track_offsets(sizes: &[f64], gap: f64, avail: f64, align: &str) -> Vec<f64> {
    let n = sizes.len();
    let mut offsets = Vec::with_capacity(n);
    let mut acc = 0.0f64;
    for &s in sizes {
        offsets.push(acc);
        acc += s + gap;
    }
    let total = (acc - if n > 0 { gap } else { 0.0 }).max(0.0);
    let free = avail - total;
    if free <= 0.0 || n == 0 {
        return offsets;
    }
    match align {
        "end" => {
            for o in &mut offsets {
                *o += free;
            }
        }
        "center" => {
            for o in &mut offsets {
                *o += free / 2.0;
            }
        }
        "space-between" if n > 1 => {
            let step = free / (n - 1) as f64;
            for (i, o) in offsets.iter_mut().enumerate() {
                *o += step * i as f64;
            }
        }
        "space-around" => {
            let u = free / n as f64;
            for (i, o) in offsets.iter_mut().enumerate() {
                *o += u * i as f64 + u / 2.0;
            }
        }
        "space-evenly" => {
            let u = free / (n + 1) as f64;
            for (i, o) in offsets.iter_mut().enumerate() {
                *o += u * (i + 1) as f64;
            }
        }
        _ => {}
    }
    offsets
}

/// 列宽确定后测量各条目内容(行高内禀与摆放共用)。
fn measure_items(
    ctx: &mut LayoutCtx,
    items: &mut [Item],
    cols: &[f64],
    style: &ComputedStyle,
    depth: usize,
) {
    for item in items.iter_mut() {
        let area_w: f64 = cols[item.col..item.col + item.col_span.min(cols.len() - item.col)]
            .iter()
            .sum::<f64>()
            + style.grid.col_gap * item.col_span.saturating_sub(1) as f64;
        let ml = item.style.margin_left;
        let mr = item.style.margin_right;
        let pl = item.style.padding_left;
        let pr = item.style.padding_right;
        let justify = effective_self(&item.style.grid_item.justify_self, &style.grid.justify_items);
        let used_w = item.style.width.unwrap_or_else(|| {
            let avail = (area_w - ml - mr).max(1.0);
            if justify == "stretch" {
                avail
            } else {
                (max_content_width(ctx, item) + pl + pr).min(avail).max(1.0)
            }
        });
        let inner_w = (used_w - pl - pr).max(1.0);
        if let Some(text) = item.text.clone() {
            let cs = item.style.clone();
            item.frag = Some(text_fragment(ctx, &text, &cs, inner_w));
        } else {
            let node = item.node.as_ref().unwrap().clone();
            let cs = item.style.clone();
            item.frag = Some(ctx.measure_content(&node, inner_w, &cs, depth));
        }
    }
}

/// 条目输出:背景盒 + 内容片段平移注入;匿名文本按最终宽换行。
fn emit_item(
    ctx: &mut LayoutCtx,
    item: &mut Item,
    area_x: f64,
    area_y: f64,
    area_w: f64,
    area_h: f64,
    style: &ComputedStyle,
) {
    let (mt, mr, mb, ml) = item.margins();
    let (pt, _, pb, pl) = item.pads();
    let justify = effective_self(&item.style.grid_item.justify_self, &style.grid.justify_items);
    let align = effective_self(&item.style.grid_item.align_self, &style.grid.align_items);
    let max_w = (area_w - ml - mr).max(1.0);
    let max_h = (area_h - mt - mb).max(1.0);
    let used_w = item.style.width.unwrap_or_else(|| {
        if justify == "stretch" {
            max_w
        } else {
            (max_content_width(ctx, item) + pl + item.style.padding_right)
                .min(max_w)
                .max(1.0)
        }
    });
    let frag_h = item.frag.as_ref().map(|f| f.height).unwrap_or(0.0);
    let used_h = item.style.height.unwrap_or_else(|| {
        if align == "stretch" {
            max_h.max(frag_h + pt + pb)
        } else {
            (frag_h + pt + pb).max(1.0)
        }
    });
    let free_w = (max_w - used_w).max(0.0);
    let free_h = (max_h - used_h).max(0.0);
    let x = area_x
        + ml
        + match justify.as_str() {
            "end" => free_w,
            "center" => free_w / 2.0,
            _ => 0.0,
        };
    let y = area_y
        + mt
        + match align.as_str() {
            "end" => free_h,
            "center" => free_h / 2.0,
            _ => 0.0,
        };
    if item.text.is_some() {
        let text = item.text.clone().unwrap_or_default();
        let wrap_w = (used_w - pl - item.style.padding_right).max(1.0);
        let cs = item.style.clone();
        let mut frag = text_fragment(ctx, &text, &cs, wrap_w);
        translate(&mut frag, x + pl, y + pt);
        append(ctx, frag);
        return;
    }
    let cs = item.style.clone();
    // 阶段六:条目视觉发射(修复 grid 条目边框/阴影/渐变缺口)。
    let seq = ctx.next_seq();
    ctx.push_box(ElementBox {
        selector: item.selector.clone(),
        tag: item.tag.clone(),
        x,
        y,
        w: used_w,
        h: used_h,
        background: cs.background,
        visible: style_visible(&cs),
        z_index: cs.pos.z_index,
    });
    ctx.emit_element(seq, &cs, &item.tag, x, y, used_w, used_h);
    // 阶段三第三步:图片条目按最终尺寸绘制(object-fit 决定填充方式)。
    if let Some(src) = item.image.clone().filter(|s| ctx.image_loaded(s)) {
        let cw = (used_w - pl - item.style.padding_right).max(0.0);
        let ch = (used_h - pt - pb).max(0.0);
        let paint = PaintBox {
            src,
            x: x + pl,
            y: y + pt,
            w: cw,
            h: ch,
            fit: cs.object_fit.clone(),
            kind: PaintKind::Content,
            visible: style_visible(&cs),
            z_index: cs.pos.z_index,
            off_x: 0.0,
            off_y: 0.0,
        };
        let item_tf = ctx.own_transform(&cs, x, y, used_w, used_h);
        ctx.emit_image(&paint, cs.opacity, item_tf);
        ctx.push_paint(paint);
        return;
    }
    let mark = ctx.display_len();
    let mut frag = item.frag.take().unwrap_or_default();
    translate(&mut frag, x + pl, y + pt);
    append(ctx, frag);
    // 阶段六:条目子树组效果回补(opacity / overflow)。
    ctx.apply_group(mark, &cs, x, y, used_w, used_h);
}

/// 条目对齐:`auto` 跟随容器 `*-items`(start/end 已在样式层归一)。
fn effective_self(self_v: &str, items_v: &str) -> String {
    if self_v == "auto" || self_v.is_empty() {
        items_v.to_string()
    } else {
        self_v.to_string()
    }
}
