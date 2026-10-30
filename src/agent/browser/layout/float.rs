//! CSS 浮动(阶段七):`float:left/right` 在块容器内的浮动带布局。
//!
//! 此前 `float`/`clear` 只落在 `ComputedStyle.extra`,布局完全不可见 ——
//! 百度地图搜索卡片(`.clearfix` 浮动布局)等经典浮动排版会退化成纵向堆叠。
//!
//! 本模块只处理**块级浮动盒**(`layout_children` 在块级分支入口调用):
//! - 左/右浮动在当前行按带宽依次排列,带宽不足时下沉到既有浮动底边之下;
//! - 非浮动的 auto 宽块级子级在浮动占据部分带宽时收窄到剩余带(侧栏+内容布局);
//! - `clear:left/right/both` 在布置前把游标推到相应浮动底边之下;
//! - 容器 auto 高收编浮动底边(clearfix 友好近似,见下)。
//!
//! 已知边界(后续补齐,不在本轮):
//! - 行级元素(`input`/`a`/`span` 等)浮动仍走行内路径,避免行内替换/控件高度退化;
//! - `img` 替换元素与 flex/grid 容器的浮动未建模;
//! - 行盒不做文字环绕:浮动激活期间的行内内容一律下沉到浮动之下,不与浮动同行;
//! - 容器 auto 高收编浮动底边是**近似**(真实 CSS 仅 BFC/clearfix 才收编),
//!   取舍理由:经典 clearfix 站点的视觉意图就是收编,不收编会导致父盒塌缩、
//!   后续内容与浮动重叠,比近似更难看。

use kuchikiki::NodeRef;

use super::super::document;
use super::LayoutCtx;
use crate::agent::browser::style::ComputedStyle;

/// 一个已布置浮动的**外边距盒**(带计算以 margin 边为界)。
struct FloatRec {
    side_left: bool,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// 块容器内的浮动台账(每个 `layout_children` 调用一份,浮动不跨容器)。
#[derive(Default)]
pub(crate) struct FloatState {
    recs: Vec<FloatRec>,
}

impl FloatState {
    pub(crate) fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    /// 当前游标处仍占据垂直空间的浮动(bottom 严格大于 y 才占行)。
    fn active_at(&self, y: f64) -> impl Iterator<Item = &FloatRec> {
        self.recs
            .iter()
            .filter(move |r| r.top <= y && r.bottom > y)
    }

    /// y 行的可用水平带 `(左界, 右界)`,基准为容器内容区 `[x, x+width]`。
    fn band(&self, x: f64, width: f64, y: f64) -> (f64, f64) {
        let mut left = x;
        let mut right = x + width;
        for r in self.active_at(y) {
            if r.side_left {
                left = left.max(r.right);
            } else {
                right = right.min(r.left);
            }
        }
        if right < left {
            right = left;
        }
        (left, right)
    }

    /// 游标下沉到当前活动浮动的最低底边;无活动浮动时原样返回。
    fn below(&self, y: f64) -> f64 {
        let mut out = y;
        for r in self.active_at(y) {
            out = out.max(r.bottom);
        }
        out
    }

    /// 容器内容底:所有浮动的最低底边(用于 auto 高收编)。
    fn max_bottom(&self) -> f64 {
        self.recs.iter().fold(0.0, |m, r| m.max(r.bottom))
    }

    /// `clear` 语义:下沉到指定侧(或两侧)全部浮动底边之下。
    pub(crate) fn cleared_y(&self, side: &str, y: f64) -> f64 {
        let mut out = y;
        for r in &self.recs {
            let hit = match side {
                "left" => r.side_left,
                "right" => !r.side_left,
                "both" => true,
                _ => false,
            };
            if hit {
                out = out.max(r.bottom);
            }
        }
        out
    }

    fn push(&mut self, rec: FloatRec) {
        self.recs.push(rec);
    }
}

/// 块级子盒布置的输入调整结果;浮动盒另带收尾所需的位置记录。
pub(crate) struct Placement {
    pub(crate) x: f64,
    pub(crate) width: f64,
    floated: bool,
    side_left: bool,
    bx: f64,
    bw: f64,
    row_y: f64,
    margin_left: f64,
    margin_right: f64,
    line_x: f64,
    container_right: f64,
    flow_y: f64,
}

impl Placement {
    fn in_flow(x: f64, width: f64) -> Self {
        Self {
            x,
            width,
            floated: false,
            side_left: true,
            bx: 0.0,
            bw: 0.0,
            row_y: 0.0,
            margin_left: 0.0,
            margin_right: 0.0,
            line_x: 0.0,
            container_right: 0.0,
            flow_y: 0.0,
        }
    }
}

/// 块级子盒进入布局前的浮动带求解。
///
/// 非浮动:auto 宽且浮动占据部分带宽时收窄到剩余带;带被占满时先下沉。
/// 浮动:计算盒宽(显式宽/百分比/shrink-to-fit),在本行带内贴左或贴右,
/// 带宽不足则逐层下沉;`self.y` 被置于浮动行顶,`flow_y` 留给 `finish` 恢复。
pub(crate) fn place(
    ctx: &mut LayoutCtx,
    child: &NodeRef,
    style: &ComputedStyle,
    x: f64,
    width: f64,
    depth: usize,
    floats: &mut FloatState,
    inline: &mut InlineRow,
) -> Placement {
    if style.float == "none" {
        if style.width.is_none() && style.width_percent.is_none() {
            let inline_float_here = inline.has_floats()
                && inline
                    .active_row_y()
                    .map(|iy| (iy - ctx.y).abs() < 0.5)
                    .unwrap_or(false);
            if floats.is_empty() && !inline_float_here {
                return Placement::in_flow(x, width);
            }
            // auto 宽块级内容盒:合并块级浮动与行内浮动两本账求剩余带,
            // 有带则收窄(侧栏+内容经典布局),带被占满才整体下沉。
            let (mut bl, mut br) = (x, x + width);
            if !floats.is_empty() {
                let (l, r) = floats.band(x, width, ctx.y);
                bl = l;
                br = r;
            }
            if inline_float_here {
                bl = bl.max(inline.right_edge());
                br = br.min(inline.left_edge());
            }
            let band = br - bl;
            if band < 1.0 {
                let mut y = floats.below(ctx.y);
                if inline_float_here {
                    y = y.max(inline.row_bottom());
                }
                if y > ctx.y {
                    ctx.y = y;
                }
                inline.deactivate();
                return Placement::in_flow(x, width);
            }
            if band < width {
                return Placement::in_flow(bl, band);
            }
        }
        return Placement::in_flow(x, width);
    }

    let side_left = style.float != "right";
    let flow_y = ctx.y;
    let mut row_y = ctx.y;
    for _ in 0..8 {
        let (mut bl, mut br) = floats.band(x, width, row_y);
        // 块级/行内浮动共用一本账:当前行已有行内内容或行内浮动时,
        // 块级浮动只能排在它剩下的带里(否则与行内输入框等同点重叠)。
        if let Some(iy) = inline.active_row_y() {
            if (iy - row_y).abs() < 0.5 {
                bl = bl.max(inline.right_edge());
                br = br.min(inline.left_edge());
                if br < bl {
                    br = bl;
                }
            }
        }
        let avail = (br - bl).max(0.0);
        let (bw, pass_w) = float_width(ctx, child, style, width, avail, depth);
        let outer = bw + style.margin_left + style.margin_right;
        let inline_here = inline
            .active_row_y()
            .map(|iy| (iy - row_y).abs() < 0.5)
            .unwrap_or(false);
        let no_occupants = floats.active_at(row_y).next().is_none() && !inline_here;
        let fits = outer <= avail || no_occupants;
        if !fits {
            let mut next = floats.below(row_y);
            if inline_here {
                next = next.max(inline.row_bottom());
            }
            if next > row_y {
                row_y = next;
                continue;
            }
        }
        // 与块级分支保持同一套 bx/bw 推导:分支内 `bx = x + margin_left`。
        let (bx, px) = if side_left {
            (bl + style.margin_left, bl)
        } else {
            let bx = br - style.margin_right - bw;
            (bx, bx - style.margin_left)
        };
        ctx.y = row_y;
        return Placement {
            x: px,
            width: pass_w,
            floated: true,
            side_left,
            bx,
            bw,
            row_y,
            margin_left: style.margin_left,
            margin_right: style.margin_right,
            line_x: x,
            container_right: x + width,
            flow_y,
        };
    }
    Placement::in_flow(x, width)
}

/// 块级分支收尾:登记浮动矩形并把游标恢复到浮动前的流式位置
/// (浮动不推进流式游标,容器底由 `max_bottom` 收编)。
pub(crate) fn finish(ctx: &mut LayoutCtx, placement: Placement, floats: &mut FloatState, inline: &mut InlineRow) {
    if !placement.floated {
        return;
    }
    let bottom = ctx.y;
    // 带计算以外边距盒为界:bx/bw 是 border 盒,左右各补回 margin。
    floats.push(FloatRec {
        side_left: placement.side_left,
        left: placement.bx - placement.margin_left,
        top: placement.row_y,
        right: placement.bx + placement.bw + placement.margin_right,
        bottom,
    });
    // 同一本行占用账同步给行内浮动:后续行内浮动才能排在本盒右侧。
    inline.register_block_float(
        placement.side_left,
        placement.bx,
        placement.row_y,
        placement.bw,
        (bottom - placement.row_y).max(0.0),
        placement.margin_left,
        placement.margin_right,
        placement.line_x,
        placement.container_right,
    );
    ctx.y = placement.flow_y;
}

/// 容器内联内容布置前调用:浮动激活时行内内容下沉到浮动之下(不做文字环绕)。
pub(crate) fn sink_inline_below_floats(ctx: &mut LayoutCtx, floats: &FloatState) {
    if floats.is_empty() {
        return;
    }
    let y = floats.below(ctx.y);
    if y > ctx.y {
        ctx.y = y;
    }
}

/// 容器子级循环结束后调用:auto 高收编浮动底边(clearfix 友好近似)。
pub(crate) fn include_floats_in_content_bottom(ctx: &mut LayoutCtx, floats: &FloatState) {
    if floats.is_empty() {
        return;
    }
    let bottom = floats.max_bottom();
    if bottom > ctx.y {
        ctx.y = bottom;
    }
}

/// 浮动盒宽:显式宽/百分比与块级分支同口径;auto 走 shrink-to-fit。
///
/// 返回 `(期望盒宽, 传给块级分支的 width)`。块级分支对 auto 宽有 40px 下限,
/// 对百分比按传入 width 二次求解,故两种情形要分别构造传参,避免右浮动错位。
fn float_width(
    ctx: &mut LayoutCtx,
    child: &NodeRef,
    style: &ComputedStyle,
    parent_w: f64,
    avail: f64,
    depth: usize,
) -> (f64, f64) {
    let ml = style.margin_left;
    let mr = style.margin_right;
    let mut bw = if let Some(w) = style.width {
        return with_max_width(w.max(1.0), parent_w, style);
    } else if let Some(p) = style.width_percent {
        return with_max_width((p / 100.0 * parent_w).max(1.0), parent_w, style);
    } else {
        // shrink-to-fit:max-content 估算后收敛到剩余带宽。
        // 不能用「大宽度铺开布局再量盒宽」:auto 块级子级会撑满测量宽,
        // 量出来的永远是容器宽(实测百度搜索卡片容器因此独占整行)。
        let pref = max_content_width(ctx, child, style, depth + 1, parent_w);
        pref.min(avail.max(1.0)).max(40.0)
    };
    if let Some(mw) = style.max_width {
        bw = bw.min(mw.max(1.0));
    }
    // auto 宽:块级分支用 `(width - ml - mr).max(40)` 反推,故按同式构造传参。
    (bw, bw + ml + mr)
}

fn with_max_width(bw: f64, parent_w: f64, style: &ComputedStyle) -> (f64, f64) {
    let bw = match style.max_width {
        Some(mw) => bw.min(mw.max(1.0)),
        None => bw,
    };
    (bw, parent_w)
}

/// 行内格式化上下文的「行占用」台账(行内块/控件盒与行内浮动共用)。
///
/// 引擎的行内模型是「一个行内盒一行」,此前行内浮动(`input/button` 等
/// 带 `float:left` 的控件)只能纵向堆叠 —— 百度地图搜索卡片
/// `input 329 + 路线按钮 39 = 内容盒 368`、`内容盒 368 + 搜索按钮 57 = 425`
/// 这种精确横向浮动行完全还原不了。本台账记录当前行已占用的左右边界,
/// 让浮动排在剩余带内,不足时换行。
#[derive(Default)]
pub(crate) struct InlineRow {
    active: bool,
    y: f64,
    /// 当前行左向内容(行内块 + 左浮动)的右缘。
    right: f64,
    /// 当前行右浮动占据的左缘(无右浮动时为容器右缘)。
    left: f64,
    h: f64,
    has_float: bool,
    float_bottom: f64,
    max_bottom: f64,
}

impl InlineRow {
    /// 登记一个非浮动行内盒(行内块/控件),后续浮动可排在当前行剩余带。
    pub(crate) fn register(&mut self, bx: f64, by: f64, w: f64, h: f64, container_right: f64) {
        self.register_band(bx, by, w, h, container_right);
        self.max_bottom = self.max_bottom.max(by + h);
    }

    /// 登记行内盒的**占用带**(供后续浮动排布),但**不**计入内容底。
    ///
    /// 第 44 轮:行内盒(行内块/表单控件)的高度已由行盒的基线分解承担,
    /// 若再计入 `max_bottom`,块游标会在文本 flush 之前被提前推下去 ——
    /// 段首凭空多出一个盒高的空洞(实测 `<p>Hello <a>link</a> World</p>`
    /// 的文字整体下沉 23px)。此处只更新占用带,不动内容底。
    pub(crate) fn register_band(&mut self, bx: f64, by: f64, w: f64, h: f64, container_right: f64) {
        if !self.active || by > self.y + 0.5 {
            self.active = true;
            self.y = by;
            self.right = bx + w;
            self.left = container_right;
            self.h = h;
            self.has_float = false;
        } else {
            self.right = self.right.max(bx + w);
            self.h = self.h.max(h);
        }
    }

    /// 行内浮动定位:返回 `(bx, by)`;同步更新行占用与内容底。
    pub(crate) fn place_float(
        &mut self,
        style: &ComputedStyle,
        line_x: f64,
        container_right: f64,
        cursor_y: f64,
        w: f64,
        h: f64,
    ) -> (f64, f64) {
        let side_left = style.float != "right";
        let ml = style.margin_left;
        let mr = style.margin_right;
        let mut bx;
        let mut by;
        if self.active {
            by = self.y;
            if side_left {
                bx = self.right + ml;
                if bx + w + mr > container_right + 0.01 {
                    // 剩余带不足:下沉一行,从行首重新排。
                    by = self.y + self.h;
                    self.y = by;
                    self.right = line_x;
                    self.left = container_right;
                    self.h = 0.0;
                    bx = line_x + ml;
                }
            } else {
                bx = self.left - mr - w;
                if bx - ml < self.right - 0.01 {
                    by = self.y + self.h;
                    self.y = by;
                    self.right = line_x;
                    self.left = container_right;
                    self.h = 0.0;
                    bx = container_right - mr - w;
                }
            }
        } else {
            by = cursor_y;
            self.active = true;
            self.y = by;
            self.h = 0.0;
            self.right = line_x;
            self.left = container_right;
            bx = if side_left {
                line_x + ml
            } else {
                container_right - mr - w
            };
        }
        if side_left {
            self.right = self.right.max(bx + w + mr);
        } else {
            self.left = self.left.min(bx - ml);
        }
        self.h = self.h.max(h + style.margin_top + style.margin_bottom);
        self.has_float = true;
        self.float_bottom = self.float_bottom.max(by + h + style.margin_bottom);
        self.max_bottom = self.max_bottom.max(by + h + style.margin_bottom);
        (bx, by)
    }

    /// 当前行是否激活,以及行 y(供块级浮动判断能否共享同一行)。
    pub(crate) fn active_row_y(&self) -> Option<f64> {
        if self.active {
            Some(self.y)
        } else {
            None
        }
    }

    /// 当前行左向内容右缘(块级左浮动从其之后开始排)。
    pub(crate) fn right_edge(&self) -> f64 {
        self.right
    }

    /// 当前行右浮动左缘(块级右浮动在其之前排)。
    pub(crate) fn left_edge(&self) -> f64 {
        self.left
    }

    /// 块级浮动盒登记(与行内浮动共用同一本行占用账,避免两套带互相重叠)。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn register_block_float(
        &mut self,
        side_left: bool,
        bx: f64,
        by: f64,
        bw: f64,
        bh: f64,
        ml: f64,
        mr: f64,
        line_x: f64,
        container_right: f64,
    ) {
        if !self.active || by > self.y + 0.5 {
            self.active = true;
            self.y = by;
            self.h = 0.0;
            self.right = line_x;
            self.left = container_right;
        }
        if side_left {
            self.right = self.right.max(bx + bw + mr);
        } else {
            self.left = self.left.min(bx - ml);
        }
        self.h = self.h.max(bh);
        self.has_float = true;
        self.float_bottom = self.float_bottom.max(by + bh);
        self.max_bottom = self.max_bottom.max(by + bh);
    }

    /// 当前行内容(含非浮动行内块)的底边,供块级浮动换行时下沉。
    pub(crate) fn row_bottom(&self) -> f64 {
        if self.active {
            self.y + self.h
        } else {
            0.0
        }
    }

    /// 当前行是否已有浮动(非浮动内容据此下沉,不做文字环绕)。
    pub(crate) fn has_floats(&self) -> bool {
        self.active && self.has_float
    }

    /// 当前行浮动的最低底边。
    pub(crate) fn float_bottom(&self) -> f64 {
        self.float_bottom
    }

    /// 结束当前行(后续行内内容另起一行)。
    pub(crate) fn deactivate(&mut self) {
        self.active = false;
        self.has_float = false;
    }

    /// 本子树(浮动盒内部)内容底并入父行台账。
    pub(crate) fn note_bottom(&mut self, bottom: f64) {
        self.max_bottom = self.max_bottom.max(bottom);
    }

    /// 行内上下文内容底(供容器 auto 高收编)。
    pub(crate) fn content_bottom(&self) -> f64 {
        self.max_bottom
    }
}

/// shrink-to-fit 的 max-content 估算(不铺开布局,沿 DOM 子树累加)。
///
/// 块级子级取最大值;行内子级与浮动子级参与同一行累加(它们本来就排在
/// 同一行,如「输入框 329 + 路线按钮 39 = 内容盒 368」)。
/// 显式宽/`img` 按自身尺寸;行内子树用 `inline_box_size` 的文本宽口径。
fn max_content_width(
    ctx: &mut LayoutCtx,
    node: &NodeRef,
    style: &ComputedStyle,
    depth: usize,
    containing_width: f64,
) -> f64 {
    if depth > 24 {
        return 0.0;
    }
    let mut max_block: f64 = 0.0;
    let mut inline_acc: f64 = 0.0;
    for child in node.children() {
        if let Some(text) = child.as_text() {
            let raw = text.borrow();
            if !raw.trim().is_empty() {
                let font = ctx.font();
                inline_acc += font.text_width(&raw, style.font_size);
            }
            continue;
        }
        if child.as_element().is_none() {
            continue;
        }
        let mut cs = ctx.engine.computed(&child, style);
        cs.resolve_padding_percent(containing_width);
        if cs.display == "none" {
            continue;
        }
        let margins =
            cs.margin_left + cs.margin_right + cs.padding_left + cs.padding_right;
        // 浮动子级与行内子级一样排在同一行,不参与块级取最大。
        let inline_like = cs.float != "none" || !cs.is_block();
        if document::tag_name(&child).eq_ignore_ascii_case("img") {
            // max-content 估算没有容器宽语义,百分比宽按 auto(见 replaced_size)。
            let (w, _h, _src) = ctx.replaced_size(&child, &cs, 0.0, None);
            if inline_like {
                inline_acc += w + margins;
            } else {
                max_block = max_block.max(inline_acc);
                inline_acc = 0.0;
                max_block = max_block.max(w + margins);
            }
            continue;
        }
        if let Some(w) = cs.width {
            if inline_like {
                inline_acc += w + margins;
            } else {
                max_block = max_block.max(inline_acc);
                inline_acc = 0.0;
                max_block = max_block.max(w + margins);
            }
            continue;
        }
        if inline_like {
            let (w, _h) = ctx.inline_box_size(&child, &cs, 0.0);
            inline_acc += w + margins;
        } else {
            max_block = max_block.max(inline_acc);
            inline_acc = 0.0;
            let own = max_content_width(ctx, &child, &cs, depth + 1, containing_width);
            max_block = max_block.max(own + margins);
        }
    }
    max_block.max(inline_acc)
}
