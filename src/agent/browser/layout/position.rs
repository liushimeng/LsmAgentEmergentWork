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
) -> ((f64, f64, f64, f64), bool, bool) {
    let frame_origin = *ctx.frame_stack.last().unwrap_or(&0);
    let in_frame = !ctx.frame_stack.is_empty();
    let trace = std::env::var("LAEW_CB_TRACE").is_ok();
    let mut cur = node.parent();
    let mut positioned_seen = false;
    while let Some(ancestor) = cur {
        if ancestor.as_element().is_some() {
            let style = ctx.engine.computed_chain(&ancestor);
            if trace {
                eprintln!(
                    "[cbtrace] chain ancestor={} pos={} tf={} frame_depth={}",
                    Document::stable_selector(&ancestor),
                    style.pos.position,
                    style.transform.is_some(),
                    ctx.frame_stack.len()
                );
            }
            if style.pos.position != "static" || style.transform.is_some() {
                positioned_seen = true;
                let selector = Document::stable_selector(&ancestor);
                if let Some(idx) = ctx.boxes.iter().rposition(|b| b.selector == selector) {
                    let b = &ctx.boxes[idx];
                    if trace {
                        eprintln!(
                            "[cbtrace] ancestor={} box=({},{},{}x{}) idx={} total_boxes={}",
                            selector,
                            b.x,
                            b.y,
                            b.w,
                            b.h,
                            idx,
                            ctx.boxes.len()
                        );
                    }
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
                        // 盒表命中 = 包含块几何已确定。
                        true,
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
                    if trace {
                        eprintln!(
                            "[cbtrace] ancestor={} PAD=({},{},{}x{}) fallback={:?}",
                            selector, pad.x, pad.y, pad.w, pad.h, fallback
                        );
                    }
                    return (((pad.x, pad.y, pad.w, h)), true, true);
                }
                // ★ 最近定位祖先的几何**未解析成功** → 包含块就是它,
                //   不能继续上溯找更外层的定位祖先(CSS:包含块由**最近**的
                //   定位祖先决定)。继续上溯会拿到更大的包含块,把元素锚到
                //   错误的位置(实测:grid 条目内的绝对定位元素被锚到 grid
                //   容器右缘,而不是条目右缘)。
                //   几何拿不到时回退调用方携带的包含块,并置 resolved=false,
                //   由调用方决定是否延后解析。
                if trace {
                    eprintln!("[cbtrace] ancestor={} 未解析出几何 → 回退 fallback", selector);
                }
                return (fallback, in_frame, false);
            }
        }
        cur = ancestor.parent();
    }
    if positioned_seen {
        // 定位祖先可能尚未收口写盒,沿用调用方携带的同一包含块。
        // 片段内的 fallback cb 是片段局部的 → 随片段平移;顶层 flow 的
        // fallback 本就是全局坐标 → 不平移。
        //
        // ★ 几何**未解析成功**:最近的定位祖先存在但拿不到盒/登记 ——
        //   典型场景:该祖先是 flex/grid 条目,几何要到条目定位后才确定。
        //   此时 fallback 常是条目自身的临时盒(宽度退化为 1px),
        //   `right`/`bottom` 反向锚定会算出负坐标(天猫搜索按钮左移 950px)。
        //   调用方据此决定是否延后解析(见 layout_abs 的 abs_defer 分支)。
        (fallback, in_frame, false)
    } else {
        // 没有定位祖先时是初始包含块(视口,全局坐标),不能回退到零宽临时父盒。
        let (vw, vh) = ctx.viewport();
        ((0.0, 0.0, vw, vh), false, true)
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
    let trace = std::env::var("LAEW_CB_TRACE").is_ok();
    let fixed = style.pos.position == "fixed";
    let ((cx, cy, cw, ch), mut rides, resolved) = if fixed {
        let (vw, vh) = ctx.viewport();
        ((0.0, 0.0, vw, vh), false, true)
    } else {
        nearest_positioned_cb(ctx, node, cb)
    };
    // fixed 视口坐标恒为全局系。
    if fixed {
        rides = false;
    }
    // ★ 包含块几何未解析成功 + 当前处于 flex/grid 条目测量语境:
    //   最近的定位祖先在条目之外,其几何要到条目定位后才确定 —— 就地布局
    //   只能用条目自身的临时盒(宽度 1px),反向锚定直接算错。
    //   改为延后:压入待解析队列,由条目定位后用容器包含块重放
    //   (见 PendingAbs / flex.rs::emit_item)。
    if !fixed && ctx.abs_defer && !resolved {
        ctx.abs_pending.push(super::PendingAbs {
            node: node.clone(),
            style: style.clone(),
            static_x: static_pos.0,
            static_y: static_pos.1,
        });
        return false;
    }
    layout_abs_resolved(ctx, node, style, (cx, cy, cw, ch), rides, depth, static_pos)
}

/// 判定「条目内延后解析的绝对定位元素」的最近定位祖先是否**就是该条目本身**。
///
/// - `true` → 条目自身(position:relative 等)即包含块 → 用条目内容盒
///   (条目坐标系 → 容器坐标系)。
/// - `false` → 包含块在条目之外(通常是容器的包含块:容器外最近定位祖先)。
pub(crate) fn item_is_containing_block(
    ctx: &LayoutCtx,
    node: &NodeRef,
    item_node: &NodeRef,
) -> bool {
    let mut cur = node.parent();
    while let Some(ancestor) = cur {
        if ancestor.as_element().is_some() {
            let style = ctx.engine.computed_chain(&ancestor);
            if style.pos.position != "static" || style.transform.is_some() {
                return &ancestor == item_node;
            }
        }
        cur = ancestor.parent();
    }
    false
}

/// 强制包含块重放入口:flex/grid 条目定位完成后,用**容器自身的包含块**
/// (容器外最近定位祖先的 padding 盒,坐标系 = 容器坐标系)解析此前延后的
/// 绝对定位元素。坐标直接落在容器坐标系,条目片段平移不再影响它。
pub(crate) fn layout_abs_with_cb(
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
    if style.pos.position == "fixed" {
        let (vw, vh) = ctx.viewport();
        return layout_abs_resolved(ctx, node, style, (0.0, 0.0, vw, vh), false, depth, static_pos);
    }
    // 重放坐标属于当前(容器)坐标系:随容器片段一起平移。
    let rides = !ctx.frame_stack.is_empty();
    layout_abs_resolved(ctx, node, style, cb, rides, depth, static_pos)
}

/// 包含块已定型的绝对定位元素布置:解尺寸 → 定位 → 发射。
fn layout_abs_resolved(
    ctx: &mut LayoutCtx,
    node: &NodeRef,
    style: &ComputedStyle,
    cb: (f64, f64, f64, f64),
    rides: bool,
    depth: usize,
    static_pos: (f64, f64),
) -> bool {
    let (cx, cy, cw, ch) = cb;
    let fixed = style.pos.position == "fixed";
    let trace = std::env::var("LAEW_CB_TRACE").is_ok();
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

    if trace {
        eprintln!(
            "[cbtrace] abs node={} tag={} cb={:?} bx={} by={} bw={} bh={} left={:?} right={:?} top={:?} static={:?}",
            Document::stable_selector(node),
            document::tag_name(node),
            (cx, cy, cw, ch),
            bx,
            by,
            bw,
            bh,
            style.pos.resolved_left(cw),
            style.pos.resolved_right(cw),
            style.pos.resolved_top(ch),
            static_pos
        );
    }
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
