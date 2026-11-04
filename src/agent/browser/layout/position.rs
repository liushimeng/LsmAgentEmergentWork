//! 定位体系(阶段三):absolute/fixed 脱离流延后布置。
//!
//! relative/sticky 的偏移在块级流内完成(mod.rs),这里只处理脱离流元素。
//! 包含块 = 最近定位祖先的 padding 盒(由调用方沿递归传递);
//! 高度未知时(`UNKNOWN_CB_H`)`bottom` 锚定退化为 `top` 语义。

use kuchikiki::NodeRef;

use super::super::document::{self, Document};
use super::super::style::ComputedStyle;
use super::{style_visible, translate, DeferredBox, ElementBox, LayoutCtx, UNKNOWN_CB_H};

/// 布置一批延后处理的脱离流元素。
pub(crate) fn layout_deferred(
    ctx: &mut LayoutCtx,
    deferred: Vec<DeferredBox>,
    cb: (f64, f64, f64, f64),
    depth: usize,
) {
    for d in deferred {
        let mark = ctx.display_len();
        let rides = layout_abs(ctx, &d.node, &d.style, cb, depth, (d.static_x, d.static_y));
        // 显示项的 out_of_flow 只服务「片段平移跳过」:全局坐标(rides=false)
        // 保持跳过;片段局部坐标(rides=true)清标志,随片段平移到锚点。
        // 只翻转普通流项(oof=false)—— 子树内嵌套 absolute 若已按**全局**
        // 包含块解析(oof=true,坐标是全局系),外层帧的平移同样不能动它。
        for it in &mut ctx.display[mark..] {
            if !it.out_of_flow {
                it.out_of_flow = !rides;
            }
        }
    }
}

/// 最近定位祖先的实际包含块。static 父盒与 Flex/Grid 的局部测量都会
/// 产生自己的临时坐标系,但它们都不是 CSS absolute 的包含块;必须沿 DOM
/// 上溯,不能直接使用调用方恰好传入的临时 cb。
///
/// 返回 (包含块, rides_frame):`rides_frame=true` 表示解析结果落在**当前
/// 测量片段**的局部坐标系里(pads 兜底 / 本帧内盒命中 / 片段内 fallback),
/// 该 absolute 子树的盒与显示项必须随片段平移;false = 全局坐标(外层盒
/// 命中或初始包含块),平移必须跳过 —— 否则双重平移。
///
/// 盒表命不中的根因:块级盒的 ElementBox 在子树布局完成后才写入,
/// 而子树内 absolute 在容器 flow 尾部就解析 —— 先序登记(PositionedPad)
/// 兜底(实测猎聘搜索条 `._40108NOybO` absolute 锚在 relative 祖先内,
/// 反查落空后回退片段局部原点,又因 out_of_flow 跳过平移,整条搜索条
/// 画到页面 (0,0) 与导航重叠)。
fn nearest_positioned_cb(
    ctx: &LayoutCtx,
    node: &NodeRef,
    fallback: (f64, f64, f64, f64),
) -> ((f64, f64, f64, f64), bool) {
    let frame_origin = *ctx.frame_stack.last().unwrap_or(&0);
    let in_frame = !ctx.frame_stack.is_empty();
    let mut cur = node.parent();
    let mut positioned_seen = false;
    while let Some(ancestor) = cur {
        if ancestor.as_element().is_some() {
            let style = ctx.engine.computed_chain(&ancestor);
            if style.pos.position != "static" || style.transform.is_some() {
                positioned_seen = true;
                let selector = Document::stable_selector(&ancestor);
                if let Some(idx) = ctx.boxes.iter().rposition(|b| b.selector == selector) {
                    let b = &ctx.boxes[idx];
                    let h = if b.h > 1.0 {
                        (b.h - style.padding_top - style.padding_bottom).max(0.0)
                    } else {
                        fallback.3
                    };
                    return (
                        (
                            b.x + style.padding_left,
                            b.y + style.padding_top,
                            (b.w - style.padding_left - style.padding_right).max(0.0),
                            h,
                        ),
                        // 本帧内发射的盒 = 片段局部坐标;外层盒 = 全局坐标。
                        in_frame && idx >= frame_origin,
                    );
                }
                // 盒表未命中(尚未写盒)→ 先序登记兜底;只认当前帧的登记,
                // 跨帧登记的坐标不属于本片段。
                if let Some(pad) = ctx
                    .positioned_pads
                    .iter()
                    .rev()
                    .find(|p| p.selector == selector && p.depth == ctx.frame_stack.len())
                {
                    let h = if pad.h > 1.0 && pad.h < super::UNKNOWN_CB_H { pad.h } else { fallback.3 };
                    return (((pad.x, pad.y, pad.w, h)), true);
                }
            }
        }
        cur = ancestor.parent();
    }
    if positioned_seen {
        // 定位祖先可能尚未收口写盒,沿用调用方携带的同一包含块。
        // 片段内的 fallback cb 是片段局部的 → 随片段平移;顶层 flow 的
        // fallback 本就是全局坐标 → 不平移。
        (fallback, in_frame)
    } else {
        // 没有定位祖先时是初始包含块(视口,全局坐标),不能回退到零宽临时父盒。
        let (vw, vh) = ctx.viewport();
        ((0.0, 0.0, vw, vh), false)
    }
}

fn layout_abs(
    ctx: &mut LayoutCtx,
    node: &NodeRef,
    style: &ComputedStyle,
    cb: (f64, f64, f64, f64),
    depth: usize,
    static_pos: (f64, f64),
) -> bool {
    if depth > 60 {
        return false;
    }
    let fixed = style.pos.position == "fixed";
    let ((cx, cy, cw, ch), mut rides) = if fixed {
        let (vw, vh) = ctx.viewport();
        ((0.0, 0.0, vw, vh), false)
    } else {
        nearest_positioned_cb(ctx, node, cb)
    };
    // fixed 视口坐标恒为全局系。
    if fixed {
        rides = false;
    }
    let pad_l = style.padding_left;
    let pad_r = style.padding_right;
    let pad_t = style.padding_top;
    let pad_b = style.padding_bottom;
    let avail_w = (cw - pad_l - pad_r).max(1.0);
    let avail_h_known = ch != UNKNOWN_CB_H;
    let avail_h = if avail_h_known { (ch - pad_t - pad_b).max(1.0) } else { f64::MAX };

    // 宽度按 border-box 求解;显式 content-box 再把 padding 加回。
    // auto 仍走 shrink-to-fit,最后统一应用 min/max 约束。
    // CSS 2.1 §10.3.7:width auto 且 left/right 均非 auto(典型 `inset:0`)
    // → 由约束求解拉伸铺满包含块,而非 shrink-to-fit(猎聘 hero 背景
    // `#home-banner-bg-root{inset:0}` 此前塌缩到内容 1×1)。
    let mut bw = if let Some(w) = style.width.or(style.width_percent.map(|p| p / 100.0 * avail_w)) {
        if style.box_sizing_content { w + pad_l + pad_r } else { w }
    } else if let (Some(l), Some(r)) = (style.pos.resolved_left(cw), style.pos.resolved_right(cw))
    {
        (cw - l - r - style.margin_left - style.margin_right).max(1.0)
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
    } else if avail_h_known {
        // §10.6.4 对称:height auto 且 top/bottom 均非 auto → 约束求解。
        match (
            style.pos.resolved_top(ch),
            style.pos.resolved_bottom(ch),
        ) {
            (Some(t), Some(b)) => (ch - t - b - style.margin_top - style.margin_bottom).max(1.0),
            _ => ctx.measure_content(node, content_w, style, depth).height.max(1.0) + pad_t + pad_b,
        }
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

    // 水平:left/right 百分比相对包含块宽;都无则取静态位置(第 48 轮修正:
    // 此前一律贴包含块左缘,无 inset 的角标/徽标整体错位)。
    let static_x = if fixed { cx + style.margin_left } else { static_pos.0 };
    let bx = if let Some(l) = style.pos.resolved_left(cw) {
        cx + l + style.margin_left
    } else if let Some(r) = style.pos.resolved_right(cw) {
        cx + cw - r - style.margin_right - bw
    } else {
        static_x
    };
    // 垂直:top/bottom 百分比相对包含块高,不能复用视口宽近似。
    let static_y = if fixed { cy + style.margin_top } else { static_pos.1 };
    let by = if let Some(t) = style.pos.resolved_top(ch) {
        cy + t + style.margin_top
    } else if let Some(b) = style.pos.resolved_bottom(ch) {
        if avail_h_known {
            cy + ch - b - style.margin_bottom - bh
        } else {
            static_y
        }
    } else {
        static_y
    };

    // 自身视觉的绘制序必须**早于**子树内容(浏览器语义:元素背景画在内容之下)。
    // seq 在测量子树**之前**预分配:此前在测量之后才取,绝对定位元素自己的
    // 背景 seq 更大 → 画在子树之上,把子树整块盖住(实测猎聘首页登录弹层
    // `.home-banner-login-container` 的白色背景把整张登录表单盖成白板)。
    let seq = ctx.next_seq();
    // 内容片段按最终宽度重排并平移注入。
    let mut frag = ctx.measure_content(node, content_w, style, depth);
    translate(&mut frag, bx + pad_l, by + pad_t);
    // 片段在测量期生成(此时 `fixed_depth` 尚未自增),固定定位子树的内容
    // 需补打 `fixed` 标记 —— 否则会随页面滚动平移,也会被祖先 overflow 裁剪。
    if fixed {
        for it in &mut frag.display {
            it.fixed = true;
        }
    }
    // fixed 子树整体标记:其盒坐标以视口为原点(上方包含块取 (0,0,vw,vh)),
    // 与普通项的页面原点不同系,光栅化侧据此跳过滚动偏移。
    if fixed {
        ctx.fixed_depth += 1;
    }
    // 阶段六:定位元素视觉发射(修复 absolute/fixed 边框/阴影/渐变缺口);
    // 自身视觉先于子树标记,子树组效果回补。
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
        background_alpha: style.background_alpha,
        visible: style_visible(style),
        z_index: style.pos.z_index,
        out_of_flow: true,
        rides_frame: rides,
    });
    // 阶段三第三步:定位体系里的替换元素(<img>)同样要贴图。
    // `measure_content` 的 img 分支只产出几何、不产出绘制指令(见 layout/mod.rs),
    // 于是**绝对定位**的图片「有盒子、没像素」——百度地图的栅格瓦片
    // (`qt=vtile` 返回 256×256 PNG,`position:absolute` 铺在地图容器里)
    // 因此整片画不出来,地图区域只剩一块灰底(实测 40/40 张瓦片几何正确但无像素)。
    // 这里补发 PaintBox + 显示项,与流内 img 路径保持一致。
    if document::tag_name(node).eq_ignore_ascii_case("img") {
        let (iw, ih, isrc) = ctx.replaced_size(node, style, avail_w, avail_h_known.then_some(avail_h));
        let tf = ctx.own_transform(style, bx, by, bw, bh);
        let img_x = bx + pad_l;
        let img_y = by + pad_t;
        let img_w = (iw - pad_l - pad_r).max(0.0);
        let img_h = (ih - pad_t - pad_b).max(0.0);
        match isrc {
            Some(src) if ctx.image_loaded(&src) => {
                let paint = ctx.content_image_paint(&src, style, img_x, img_y, img_w, img_h);
                ctx.emit_image(&paint, style.opacity, tf);
                ctx.push_paint(paint);
            }
            _ => ctx.emit_img_alt(node, style, img_x, img_y, img_w, img_h, tf),
        }
    }
    let mark = ctx.display_len();
    // 脱流子树的片段盒统一标记:这些盒随 abs 元素整体注入,坐标以包含块为
    // 语境(测量语境下可能落在 MAX_CONTENT_WIDTH 远端),绝不能回流进
    // 任何祖先的内在尺寸聚合(第 174 轮猎聘「登录/注册」0×0 根因的另一半:
    // measure_content 替换元素捷径盒 tag=img 曾以 oof=false 注入)。
    for b in &mut frag.boxes {
        let rode_frame = !b.out_of_flow || b.rides_frame;
        b.out_of_flow = true;
        // 只重标「随上面 translate 进本帧」的盒子;嵌套 absolute 已按全局
        // 包含块解析的盒(oof 且未随帧)保持全局语义,外层平移不得移动。
        if rode_frame {
            b.rides_frame = rides;
        }
    }
    super::append(ctx, frag);
    ctx.apply_group(mark, style, bx, by, bw, bh);
    if fixed {
        ctx.fixed_depth -= 1;
    }
    rides
}
