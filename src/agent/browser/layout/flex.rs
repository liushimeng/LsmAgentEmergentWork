//! Flex 弹性布局(阶段三)。
//!
//! 覆盖:主轴/交叉轴方向、grow/shrink/basis、justify-content、
//! align-items/align-self/stretch、wrap 多行 + align-content、gap、匿名文本条目。
//! 已知边界(见设计文档 08):baseline 按 flex-start;column 方向不支持 wrap;
//! 匿名盒按文本段合并;min-width 未建模;auto 外边距按 0。

use kuchikiki::NodeRef;

use super::super::document::{self, Document};
use super::super::style::{ComputedStyle, FlexWrap};
use super::{
    append, style_visible, translate, ElementBox, Fragment, LayoutCtx, PaintBox, PaintKind,
    MAX_CONTENT_WIDTH,
};

/// Flex 条目:元素子节点或匿名文本段。
struct Item {
    node: Option<NodeRef>,
    style: ComputedStyle,
    text: Option<String>,
    selector: String,
    tag: String,
    /// 替换元素图片地址(阶段三第三步;仅 `<img>` 条目非空)。
    image: Option<String>,
    /// 弹性前 border-box 主尺寸。
    base_main: f64,
    /// 弹性后 border-box 主尺寸。
    final_main: f64,
    /// border-box 交叉尺寸(stretch 之后)。
    final_cross: f64,
    /// 内容片段(条目内容在最终尺寸下的布局结果;定位时平移注入)。
    frag: Option<Fragment>,
}

impl Item {
    fn margins_main(&self, row: bool) -> (f64, f64) {
        if row {
            (self.style.margin_left, self.style.margin_right)
        } else {
            (self.style.margin_top, self.style.margin_bottom)
        }
    }
    fn margins_cross(&self, row: bool) -> (f64, f64) {
        if row {
            (self.style.margin_top, self.style.margin_bottom)
        } else {
            (self.style.margin_left, self.style.margin_right)
        }
    }
    fn pads_main(&self, row: bool) -> (f64, f64) {
        if row {
            (self.style.padding_left, self.style.padding_right)
        } else {
            (self.style.padding_top, self.style.padding_bottom)
        }
    }
    fn pads_cross(&self, row: bool) -> (f64, f64) {
        if row {
            (self.style.padding_top, self.style.padding_bottom)
        } else {
            (self.style.padding_left, self.style.padding_right)
        }
    }
    fn outer_main(&self, row: bool) -> f64 {
        let (b, a) = self.margins_main(row);
        self.final_main + b + a
    }
    fn outer_cross(&self, row: bool) -> f64 {
        let (b, a) = self.margins_cross(row);
        self.final_cross + b + a
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
        base_main: 0.0,
        final_main: 0.0,
        final_cross: 0.0,
        frag: None,
    }
}

/// 匿名文本条目:样式继承容器(近似;不经过样式引擎逐节点计算)。
fn anon_item(text: &str, parent: &ComputedStyle) -> Item {
    Item {
        node: None,
        style: parent.clone(),
        text: Some(text.to_string()),
        selector: String::new(),
        tag: String::new(),
        image: None,
        base_main: 0.0,
        final_main: 0.0,
        final_cross: 0.0,
        frag: None,
    }
}

/// 容器自身按块级惯例定位;背景盒先占位(保证条目绘制在容器背景之上),
/// 内容区交给 Flex 算法,结束后回填容器盒高度。
/// 调用方在进入本函数前已清空行内缓冲(与块级分支一致)。
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
    let content_h =
        layout_content(ctx, node, content_x, inner_w, style, depth + 1, cb);
    let box_h = (ctx.cursor_y() - start_y + content_h).max(1.0);
    ctx.set_box_height(box_idx, box_h);
    // 阶段六:组效果回补 + 自身视觉发射(含多层背景图,修复 flex 容器边框/阴影缺口)。
    let tag = document::tag_name(node);
    ctx.finish_container(seq, dmark, style, &tag, bx, start_y, bw, box_h);
    ctx.advance_y(content_h + style.padding_bottom + style.margin_bottom.max(2.0));
}

/// Flex 内容区布局:布置全部条目并注入几何,返回内容高度。
/// `measure_content` 对 flex 容器条目复用本入口(嵌套格式化上下文)。
#[allow(clippy::too_many_arguments)]
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
    let row = style.flex.direction.is_row();
    let reverse = style.flex.direction.is_reverse();
    let gap_main = if row { style.flex.col_gap } else { style.flex.row_gap };
    let gap_cross = if row { style.flex.row_gap } else { style.flex.col_gap };

    // 1. 收集条目(元素 + 匿名文本段);脱离流定位元素延后到条目之后。
    let mut items = collect_items(ctx, node, style);
    let deferred = std::mem::take(&mut items.1);
    let mut items = items.0;
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
    // order 排序(稳定)。
    items.sort_by_key(|i| i.style.flex_item.order);

    // 2. 容器主/交叉尺寸。
    let inner_main = if row {
        content_w.max(0.0)
    } else {
        style
            .height
            .map(|h| (h - style.padding_top - style.padding_bottom).max(0.0))
            .unwrap_or(0.0)
    };
    let definite_main = !row && style.height.is_some();
    let definite_cross: Option<f64> = if row {
        style
            .height
            .map(|h| (h - style.padding_top - style.padding_bottom).max(0.0))
    } else {
        Some(content_w.max(0.0))
    };

    // 3. base 主尺寸(列方向在此决定交叉宽并测量内容)。
    for item in &mut items {
        resolve_base(ctx, item, row, content_w, definite_cross, &style.flex.align_items, depth);
    }

    // 4. 行打包(row + wrap;column 恒单行)。
    let lines = pack_lines(&items, row, style.flex.wrap, inner_main, gap_main);

    // 5. 每行弹性分配 + 内容重排 + 交叉尺寸。
    let mut line_cross: Vec<f64> = Vec::with_capacity(lines.len());
    for line in &lines {
        distribute_main(ctx, &mut items, line, row, inner_main, definite_main, gap_main);
        let mut cross = 0.0f64;
        for &i in line {
            let item = &mut items[i];
            if item.text.is_some() {
                // 匿名文本:交叉 = 行高。
                let cs = &item.style;
                item.final_cross = cs.line_height.max(cs.font_size * 1.35);
            } else if row {
                // 行方向:按最终主尺寸重排内容,交叉 = 显式 height 或内容高。
                let inner = (item.final_main - item.pads_main(row).0 - item.pads_main(row).1).max(1.0);
                let mut frag =
                    ctx.measure_content(item.node.as_ref().unwrap(), inner, &item.style, depth);
                item.final_cross = item
                    .style
                    .height
                    .unwrap_or(frag.height + item.pads_cross(row).0 + item.pads_cross(row).1);
                item.frag = Some(std::mem::take(&mut frag));
            }
            // column 方向:final_cross 已在 resolve_base 决定。
            cross = cross.max(item.outer_cross(row));
        }
        let line_cross_size = definite_cross.unwrap_or(cross).max(cross);
        // stretch:行方向无显式 height 且对齐 stretch 的条目拉伸到行交叉。
        for &i in line {
            let item = &mut items[i];
            let align = effective_align(&item.style.flex_item.align_self, &style.flex.align_items);
            let definite_item_cross = if row {
                item.style.height.is_some()
            } else {
                item.style.width.is_some()
            };
            if !definite_item_cross && align == "stretch" && row {
                let (b, a) = item.margins_cross(row);
                item.final_cross = (line_cross_size - b - a).max(item.final_cross);
            }
        }
        line_cross.push(line_cross_size);
    }

    // 6. 行间分布(align-content)与行交叉偏移。
    let total_cross: f64 = line_cross.iter().sum::<f64>()
        + gap_cross * line_cross.len().saturating_sub(1) as f64;
    let container_cross = definite_cross.unwrap_or(total_cross).max(total_cross);
    let line_offsets = line_offsets(
        container_cross - total_cross,
        &line_cross,
        gap_cross,
        &style.flex.align_content,
    );

    // 7. 定位与输出。
    let base_y = ctx.cursor_y();
    for (li, line) in lines.iter().enumerate() {
        let used = line.iter().map(|&i| items[i].outer_main(row)).sum::<f64>()
            + gap_main * line.len().saturating_sub(1) as f64;
        let free = if row || definite_main {
            inner_main - used
        } else {
            0.0
        };
        let (start, between) = justify_offsets(free, line.len(), &style.flex.justify);
        let line_y = base_y + line_offsets[li];
        let main_span = if row || definite_main {
            inner_main
        } else {
            used
        };
        let mut cursor = start;
        for &i in line {
            let item = &mut items[i];
            let (mb, _ma) = item.margins_main(row);
            let main_pos = cursor + mb;
            cursor += item.outer_main(row) + gap_main + between;
            let align = effective_align(&item.style.flex_item.align_self, &style.flex.align_items);
            let (cb, _ca) = item.margins_cross(row);
            let cross_pos = align_cross_offset(&align, line_cross[li], item.outer_cross(row)) + cb;
            let (ix, iy) = if row {
                let mx = if reverse {
                    content_x + main_span - main_pos - item.final_main
                } else {
                    content_x + main_pos
                };
                (mx, line_y + cross_pos)
            } else {
                let my = if reverse {
                    base_y + main_span - main_pos - item.final_main
                } else {
                    base_y + main_pos
                };
                (content_x + cross_pos, my)
            };
            emit_item(ctx, item, ix, iy, row, depth);
        }
    }

    // 8. 脱离流定位元素最后布置(条目之上、容器背景之下由盒序保证)。
    if !deferred.is_empty() {
        let pad_cb = (
            content_x - style.padding_left,
            ctx.cursor_y() - style.padding_top,
            content_w + style.padding_left + style.padding_right,
            super::UNKNOWN_CB_H,
        );
        super::position::layout_deferred(ctx, deferred, pad_cb, depth);
    }
    container_cross.max(0.0)
}

/// 收集条目与脱离流定位元素。
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
        // 阶段三第三步:`<img>` 条目记录图片地址(按固有宽度参与弹性分配)。
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

/// 弹性前 base 主尺寸;列方向顺带决定交叉宽并测量内容片段。
fn resolve_base(
    ctx: &mut LayoutCtx,
    item: &mut Item,
    row: bool,
    content_w: f64,
    definite_cross: Option<f64>,
    align_items: &str,
    depth: usize,
) {
    let cs = &item.style;
    let explicit = if row {
        cs.flex_item.basis.or(cs.width)
    } else {
        cs.flex_item.basis.or(cs.height)
    };
    if row {
        let mut base = explicit.unwrap_or_else(|| match &item.text {
            Some(text) => ctx.font().text_width(text, cs.font_size),
            None => {
                // 阶段三第三步:图片条目按固有宽度参与弹性分配。
                if let Some(src) = &item.image {
                    let natural = ctx.images().natural(src).map(|(w, _)| w).unwrap_or(0.0);
                    natural + item.pads_main(row).0 + item.pads_main(row).1
                } else {
                    let frag = ctx.measure_content(
                        item.node.as_ref().unwrap(),
                        MAX_CONTENT_WIDTH,
                        cs,
                        depth,
                    );
                    let mut max_main = 0.0f64;
                    for l in &frag.lines {
                        let w = ctx.font().text_width(&l.text, l.size);
                        max_main = max_main.max(l.x + w);
                    }
                    for b in &frag.boxes {
                        max_main = max_main.max(b.x + b.w);
                    }
                    max_main + item.pads_main(row).0 + item.pads_main(row).1
                }
            }
        });
        if let Some(mw) = cs.max_width {
            base = base.min(mw.max(1.0));
        }
        item.base_main = base.max(0.0);
    } else {
        // 列方向:交叉宽 = width ?? (stretch ? 容器交叉 : max-content)。
        let align = effective_align(&cs.flex_item.align_self, align_items);
        let avail = definite_cross.unwrap_or(content_w).max(0.0);
        let (pb, pa) = item.pads_cross(row);
        let width = cs.width.unwrap_or_else(|| {
            if align == "stretch" {
                (avail - item.margins_cross(row).0 - item.margins_cross(row).1).max(1.0)
            } else {
                let frag = ctx.measure_content(
                    item.node.as_ref().unwrap(),
                    MAX_CONTENT_WIDTH,
                    cs,
                    depth,
                );
                let mut w = 0.0f64;
                for l in &frag.lines {
                    let lw = ctx.font().text_width(&l.text, l.size);
                    w = w.max(l.x + lw);
                }
                for b in &frag.boxes {
                    w = w.max(b.x + b.w);
                }
                (w + pb + pa).min(avail).max(1.0)
            }
        });
        item.final_cross = width.max(1.0);
        let inner_cross = (item.final_cross - pb - pa).max(1.0);
        if let Some(node) = &item.node {
            let frag = ctx.measure_content(node, inner_cross, cs, depth);
            item.base_main = explicit
                .unwrap_or(frag.height + item.pads_main(row).0 + item.pads_main(row).1)
                .max(0.0);
            item.frag = Some(frag);
        } else {
            let lh = cs.line_height.max(cs.font_size * 1.35);
            item.base_main = explicit.unwrap_or(lh).max(0.0);
        }
    }
}

/// 行打包:row + wrap 贪心断行;其余单行。
fn pack_lines(
    items: &[Item],
    row: bool,
    wrap: FlexWrap,
    inner_main: f64,
    gap_main: f64,
) -> Vec<Vec<usize>> {
    if row && wrap != FlexWrap::Nowrap {
        let mut lines: Vec<Vec<usize>> = Vec::new();
        let mut cur: Vec<usize> = Vec::new();
        let mut used = 0.0f64;
        for (idx, item) in items.iter().enumerate() {
            let outer = item.base_main + item.margins_main(row).0 + item.margins_main(row).1;
            let add = outer + if cur.is_empty() { 0.0 } else { gap_main };
            if !cur.is_empty() && used + add > inner_main {
                lines.push(std::mem::take(&mut cur));
                used = 0.0;
            }
            used += outer + if cur.is_empty() { 0.0 } else { gap_main };
            cur.push(idx);
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        if wrap == FlexWrap::WrapReverse {
            lines.reverse();
        }
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        lines
    } else {
        vec![(0..items.len()).collect()]
    }
}

/// 行内主尺寸弹性分配(grow/shrink)。
fn distribute_main(
    ctx: &mut LayoutCtx,
    items: &mut [Item],
    line: &[usize],
    row: bool,
    inner_main: f64,
    definite_main: bool,
    gap_main: f64,
) {
    let _ = ctx;
    let gaps = gap_main * line.len().saturating_sub(1) as f64;
    let total_outer: f64 = line
        .iter()
        .map(|&i| items[i].base_main + items[i].margins_main(row).0 + items[i].margins_main(row).1)
        .sum();
    let free = if row || definite_main {
        inner_main - total_outer - gaps
    } else {
        0.0
    };
    let total_grow: f64 = line.iter().map(|&i| items[i].style.flex_item.grow).sum();
    if free > 0.0 && total_grow > 0.0 {
        for &i in line {
            let g = items[i].style.flex_item.grow;
            items[i].final_main = items[i].base_main + free * g / total_grow;
        }
    } else if free < 0.0 {
        let weighted: f64 = line
            .iter()
            .map(|&i| items[i].style.flex_item.shrink * items[i].base_main)
            .sum();
        if weighted > 0.0 {
            let deficit = -free;
            for &i in line {
                let w = items[i].style.flex_item.shrink * items[i].base_main;
                items[i].final_main = (items[i].base_main - deficit * w / weighted).max(0.0);
            }
        } else {
            for &i in line {
                items[i].final_main = items[i].base_main;
            }
        }
    } else {
        for &i in line {
            items[i].final_main = items[i].base_main;
        }
    }
}

/// justify-content → (行内起始偏移, 条目间额外间隔)。
fn justify_offsets(free: f64, n: usize, justify: &str) -> (f64, f64) {
    if free <= 0.0 {
        // 溢出/无余量:space-* 退化为 flex-start;flex-end/center 允许负偏移。
        return match justify {
            "flex-end" => (free, 0.0),
            "center" => (free / 2.0, 0.0),
            _ => (0.0, 0.0),
        };
    }
    match justify {
        "flex-end" => (free, 0.0),
        "center" => (free / 2.0, 0.0),
        "space-between" => (0.0, if n > 1 { free / (n - 1) as f64 } else { 0.0 }),
        "space-around" => {
            let u = free / n.max(1) as f64;
            (u / 2.0, u)
        }
        "space-evenly" => {
            let u = free / (n + 1) as f64;
            (u, u)
        }
        _ => (0.0, 0.0),
    }
}

/// 条目在行内交叉偏移(baseline 按 flex-start,见设计文档边界)。
fn align_cross_offset(align: &str, line_cross: f64, outer_cross: f64) -> f64 {
    match align {
        "flex-end" => (line_cross - outer_cross).max(0.0),
        "center" => ((line_cross - outer_cross) / 2.0).max(0.0),
        _ => 0.0,
    }
}

fn effective_align(align_self: &str, align_items: &str) -> String {
    if align_self == "auto" || align_self.is_empty() {
        align_items.to_string()
    } else {
        align_self.to_string()
    }
}

/// align-content 行间分布:返回每行交叉偏移。
/// 基线为行高顺序累加(含 gap),有余量时按 align-content 平移;
/// stretch 按 flex-start 处理(行高不重新拉伸,见设计文档边界)。
fn line_offsets(free: f64, line_cross: &[f64], gap_cross: f64, align_content: &str) -> Vec<f64> {
    let n = line_cross.len();
    let mut offsets = Vec::with_capacity(n);
    let mut acc = 0.0f64;
    for &c in line_cross {
        offsets.push(acc);
        acc += c + gap_cross;
    }
    if n <= 1 || free <= 0.0 {
        return offsets;
    }
    match align_content {
        "flex-end" => {
            for o in &mut offsets {
                *o += free;
            }
        }
        "center" => {
            for o in &mut offsets {
                *o += free / 2.0;
            }
        }
        "space-between" => {
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

/// 条目输出:条目背景盒 + 内容片段平移注入;匿名文本按最终主尺寸换行。
fn emit_item(ctx: &mut LayoutCtx, item: &mut Item, x: f64, y: f64, row: bool, depth: usize) {
    if let Some(text) = item.text.clone() {
        let wrap_w = if row { item.final_main } else { item.final_cross };
        let mut frag = text_fragment(ctx, &text, &item.style, wrap_w);
        translate(&mut frag, x, y);
        append(ctx, frag);
        return;
    }
    let cs = item.style.clone();
    let iw = if row { item.final_main } else { item.final_cross };
    let ih = if row { item.final_cross } else { item.final_main };
    // 阶段六:条目视觉发射(修复 flex 条目边框/阴影/渐变缺口)。
    let seq = ctx.next_seq();
    ctx.push_box(ElementBox {
        selector: item.selector.clone(),
        tag: item.tag.clone(),
        x,
        y,
        w: iw,
        h: ih,
        background: cs.background,
        visible: style_visible(&cs),
        z_index: cs.pos.z_index,
    });
    ctx.emit_element(seq, &cs, &item.tag, x, y, iw, ih);
    // 阶段三第三步:图片条目按最终尺寸绘制(object-fit 决定填充方式)。
    if let Some(src) = item.image.clone().filter(|s| ctx.image_loaded(s)) {
        let cw = (iw - cs.padding_left - cs.padding_right).max(0.0);
        let ch = (ih - cs.padding_top - cs.padding_bottom).max(0.0);
        let paint = PaintBox {
            src,
            x: x + cs.padding_left,
            y: y + cs.padding_top,
            w: cw,
            h: ch,
            fit: cs.object_fit.clone(),
            kind: PaintKind::Content,
            visible: style_visible(&cs),
            z_index: cs.pos.z_index,
            off_x: 0.0,
            off_y: 0.0,
        };
        let item_tf = ctx.own_transform(&cs, x, y, iw, ih);
        ctx.emit_image(&paint, cs.opacity, item_tf);
        ctx.push_paint(paint);
        return;
    }
    let mark = ctx.display_len();
    let mut frag = item.frag.take().unwrap_or_default();
    let (pb, _) = item.pads_main(row);
    let (cb, _) = item.pads_cross(row);
    let (dx, dy) = if row { (x + pb, y + cb) } else { (x + cb, y + pb) };
    translate(&mut frag, dx, dy);
    append(ctx, frag);
    // 阶段六:条目子树组效果回补(opacity / overflow)。
    ctx.apply_group(mark, &cs, x, y, iw, ih);
    let _ = depth;
}

/// 匿名文本条目片段:按给定宽度贪心换行,行高取样式 line-height。
/// Grid 匿名文本条目共用(阶段三第二步)。
pub(crate) fn text_fragment(ctx: &mut LayoutCtx, text: &str, cs: &ComputedStyle, width: f64) -> Fragment {
    let size = cs.font_size.max(8.0);
    let line_height = cs.line_height.max(size * 1.35);
    let max_w = width.max(1.0);
    let visible = style_visible(cs);
    let bold = cs.font_weight >= 600;
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0.0f64;
    let mut y = 0.0f64;
    for ch in text.chars() {
        let w = ctx.font().char_width(ch, size);
        if cur_w + w > max_w && !cur.is_empty() {
            lines.push(super::TextLine {
                text: std::mem::take(&mut cur),
                x: 0.0,
                y,
                size,
                color: cs.color,
                bold,
                visible,
            });
            y += line_height;
            cur_w = 0.0;
        }
        cur.push(ch);
        cur_w += w;
    }
    if !cur.trim().is_empty() {
        lines.push(super::TextLine {
            text: cur,
            x: 0.0,
            y,
            size,
            color: cs.color,
            bold,
            visible,
        });
        y += line_height;
    }
    Fragment {
        height: y,
        lines,
        boxes: Vec::new(),
        paints: Vec::new(),
        borders: Vec::new(),
        shadows: Vec::new(),
        display: Vec::new(),
    }
}
