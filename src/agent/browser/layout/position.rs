//! 定位体系(阶段三):absolute/fixed 脱离流延后布置。
//!
//! relative/sticky 的偏移在块级流内完成(mod.rs),这里只处理脱离流元素。
//! 包含块 = 最近定位祖先的 padding 盒(由调用方沿递归传递);
//! 高度未知时(`UNKNOWN_CB_H`)`bottom` 锚定退化为 `top` 语义。

use kuchikiki::NodeRef;

use super::super::document::{self, Document};
use super::super::style::ComputedStyle;
use super::{style_visible, translate, ElementBox, LayoutCtx, PaintBox, PaintKind, UNKNOWN_CB_H};

/// 布置一批延后处理的脱离流元素。
pub(crate) fn layout_deferred(
    ctx: &mut LayoutCtx,
    deferred: Vec<(NodeRef, ComputedStyle)>,
    cb: (f64, f64, f64, f64),
    depth: usize,
) {
    for (node, style) in deferred {
        layout_abs(ctx, &node, &style, cb, depth);
    }
}

fn layout_abs(
    ctx: &mut LayoutCtx,
    node: &NodeRef,
    style: &ComputedStyle,
    cb: (f64, f64, f64, f64),
    depth: usize,
) {
    if depth > 60 {
        return;
    }
    let fixed = style.pos.position == "fixed";
    let (cx, cy, cw, ch) = if fixed {
        let (vw, vh) = ctx.viewport();
        (0.0, 0.0, vw, vh)
    } else {
        cb
    };
    let pad_l = style.padding_left;
    let pad_r = style.padding_right;
    let pad_t = style.padding_top;
    let pad_b = style.padding_bottom;
    let avail_w = (cw - pad_l - pad_r).max(1.0);
    let avail_h_known = ch != UNKNOWN_CB_H;
    let avail_h = if avail_h_known { (ch - pad_t - pad_b).max(1.0) } else { f64::MAX };

    // 宽度:显式 width ?? 收缩适配(max-content 受可用宽约束)。
    let content_w = style.width.unwrap_or_else(|| {
        let frag = ctx.measure_content(node, super::MAX_CONTENT_WIDTH, style, depth);
        let mut w = 0.0f64;
        for l in &frag.lines {
            let lw = ctx.font().text_width(&l.text, l.size);
            w = w.max(l.x + lw);
        }
        for b in &frag.boxes {
            w = w.max(b.x + b.w);
        }
        w.min(avail_w).max(1.0)
    });
    let content_h = style
        .height
        .unwrap_or_else(|| {
            let frag = ctx.measure_content(node, content_w, style, depth);
            frag.height.max(1.0)
        })
        .min(if avail_h_known { avail_h } else { f64::MAX })
        .max(1.0);

    let bw = content_w + pad_l + pad_r;
    let bh = content_h + pad_t + pad_b;
    // 水平:left 优先;否则 right 锚定;都无则贴包含块左缘(静态位置简化)。
    let bx = if let Some(l) = style.pos.left {
        cx + l + style.margin_left
    } else if let Some(r) = style.pos.right {
        cx + cw - r - style.margin_right - bw
    } else {
        cx + style.margin_left
    };
    // 垂直:top 优先;否则 bottom 锚定(包含块高度未知时退化);都无则贴包含块上缘。
    let by = if let Some(t) = style.pos.top {
        cy + t + style.margin_top
    } else if let Some(b) = style.pos.bottom {
        if avail_h_known {
            cy + ch - b - style.margin_bottom - bh
        } else {
            cy + style.margin_top
        }
    } else {
        cy + style.margin_top
    };

    // 内容片段按最终宽度重排并平移注入。
    let mut frag = ctx.measure_content(node, content_w, style, depth);
    translate(&mut frag, bx + pad_l, by + pad_t);
    // fixed 子树整体标记:其盒坐标以视口为原点(上方包含块取 (0,0,vw,vh)),
    // 与普通项的页面原点不同系,光栅化侧据此跳过滚动偏移。
    if fixed {
        ctx.fixed_depth += 1;
    }
    // 阶段六:定位元素视觉发射(修复 absolute/fixed 边框/阴影/渐变缺口);
    // 自身视觉先于子树标记,子树组效果回补。
    let seq = ctx.next_seq();
    let tag = document::tag_name(node);
    ctx.emit_element(seq, style, &tag, bx, by, bw, bh);
    ctx.push_box(ElementBox {
        selector: Document::stable_selector(node),
        tag: document::tag_name(node),
        x: bx,
        y: by,
        w: bw,
        h: bh,
        background: style.background,
        visible: style_visible(style),
        z_index: style.pos.z_index,
    });
    // 阶段三第三步:定位体系里的替换元素(<img>)同样要贴图。
    // `measure_content` 的 img 分支只产出几何、不产出绘制指令(见 layout/mod.rs),
    // 于是**绝对定位**的图片「有盒子、没像素」——百度地图的栅格瓦片
    // (`qt=vtile` 返回 256×256 PNG,`position:absolute` 铺在地图容器里)
    // 因此整片画不出来,地图区域只剩一块灰底(实测 40/40 张瓦片几何正确但无像素)。
    // 这里补发 PaintBox + 显示项,与流内 img 路径保持一致。
    if document::tag_name(node).eq_ignore_ascii_case("img") {
        let (iw, ih, isrc) = ctx.replaced_size(node, style, avail_w, avail_h_known.then_some(avail_h));
        if let Some(src) = isrc.filter(|s| ctx.image_loaded(s)) {
            let paint = PaintBox {
                src,
                x: bx + pad_l,
                y: by + pad_t,
                w: (iw - pad_l - pad_r).max(0.0),
                h: (ih - pad_t - pad_b).max(0.0),
                fit: style.object_fit.clone(),
                kind: PaintKind::Content,
                visible: style_visible(style),
                z_index: style.pos.z_index,
                off_x: 0.0,
                off_y: 0.0,
            };
            let tf = ctx.own_transform(style, bx, by, bw, bh);
            ctx.emit_image(&paint, style.opacity, tf);
            ctx.push_paint(paint);
        }
    }
    let mark = ctx.display_len();
    super::append(ctx, frag);
    ctx.apply_group(mark, style, bx, by, bw, bh);
    if fixed {
        ctx.fixed_depth -= 1;
    }
}
