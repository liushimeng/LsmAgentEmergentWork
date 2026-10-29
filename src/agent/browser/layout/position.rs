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

    // 宽度按 border-box 求解;显式 content-box 再把 padding 加回。
    // auto 仍走 shrink-to-fit,最后统一应用 min/max 约束。
    let mut bw = if let Some(w) = style.width.or(style.width_percent.map(|p| p / 100.0 * avail_w)) {
        if style.box_sizing_content { w + pad_l + pad_r } else { w }
    } else {
        let frag = ctx.measure_content(node, super::MAX_CONTENT_WIDTH, style, depth);
        let mut w = 0.0f64;
        for l in &frag.lines {
            let lw = ctx.font().text_width(&l.text, l.size);
            w = w.max(l.x + lw);
        }
        for b in &frag.boxes {
            if b.out_of_flow {
                continue;
            }
            w = w.max(b.x + b.w);
        }
        w.min(avail_w).max(1.0) + pad_l + pad_r
    };
    if let Some(max) = style.max_width {
        bw = bw.min(max);
    }
    if let Some(min) = style.min_width {
        bw = bw.max(min);
    }
    let content_w = (bw - pad_l - pad_r).max(1.0);
    bw = content_w + pad_l + pad_r;

    let mut bh = if let Some(h) = style.height.or_else(|| {
        avail_h_known.then(|| style.height_percent.map(|p| p / 100.0 * avail_h)).flatten()
    }) {
        if style.box_sizing_content { h + pad_t + pad_b } else { h }
    } else {
        ctx.measure_content(node, content_w, style, depth).height.max(1.0) + pad_t + pad_b
    };
    if avail_h_known {
        bh = bh.min(avail_h + pad_t + pad_b);
    }
    if let Some(max) = style.max_height {
        bh = bh.min(max);
    }
    if let Some(min) = style.min_height {
        bh = bh.max(min);
    }
    let content_h = (bh - pad_t - pad_b).max(1.0);
    bh = content_h + pad_t + pad_b;

    // 水平:left/right 百分比相对包含块宽;都无则贴左缘(静态位置简化)。
    let bx = if let Some(l) = style.pos.resolved_left(cw) {
        cx + l + style.margin_left
    } else if let Some(r) = style.pos.resolved_right(cw) {
        cx + cw - r - style.margin_right - bw
    } else {
        cx + style.margin_left
    };
    // 垂直:top/bottom 百分比相对包含块高,不能复用视口宽近似。
    let by = if let Some(t) = style.pos.resolved_top(ch) {
        cy + t + style.margin_top
    } else if let Some(b) = style.pos.resolved_bottom(ch) {
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
    ctx.emit_canvas_pixels(node, style, seq, bx, by, bw, bh);
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
        out_of_flow: true,
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
    // 脱流子树的片段盒统一标记:这些盒随 abs 元素整体注入,坐标以包含块为
    // 语境(测量语境下可能落在 MAX_CONTENT_WIDTH 远端),绝不能回流进
    // 任何祖先的内在尺寸聚合(第 174 轮猎聘「登录/注册」0×0 根因的另一半:
    // measure_content 替换元素捷径盒 tag=img 曾以 oof=false 注入)。
    for b in &mut frag.boxes {
        b.out_of_flow = true;
    }
    super::append(ctx, frag);
    ctx.apply_group(mark, style, bx, by, bw, bh);
    if fixed {
        ctx.fixed_depth -= 1;
    }
}
