//! `position: sticky` 吸附标记与参数求解(第 50 轮)。
//!
//! 页面级滚动(`act_scroll` 无 selector 分支)只更新 `tab.scroll_y` 推帧、
//! **不重排**;布局产物是文档坐标、光栅减滚动量。因此吸附不能在布局期
//! 求解(那要求每次滚动全量重排),而是:
//!
//! 1. 块级分支进入 sticky 元素时登记 `StickyMark`(seq 半开区间 + 静态几何
//!    + 包含块 selector),退出时回填 `end_seq/static_y/outer_h`;
//! 2. 布局收尾(`compute_full`)在显示列表终排**前**求解 `max_shift`
//!    (包含块底界 - 静态盒底),给区间内显示项打 `sticky` 标记;
//! 3. 光栅期按滚动量现算平移(`StickyInfo::shift`),与 fixed 的滚动豁免
//!    同一通道。
//!
//! 范围:常规块流的视口吸附(top 优先于 bottom);元素级滚动容器内的
//! sticky、表格行吸附、left/right 吸附不在本轮(40 号文档矩阵如实标注)。

use super::*;
use crate::agent::browser::render::display::StickyInfo;

/// 布局期登记的 sticky 子树(进入时 push,块级分支退出时回填几何)。
pub(crate) struct StickyMark {
    /// 子树显示项 seq 半开区间起点(= sticky 元素自身 seq)。
    pub start_seq: u64,
    /// 子树显示项 seq 半开区间终点(回填,= 退出时的 seq 计数)。
    pub end_seq: u64,
    pub top: Option<f64>,
    pub bottom: Option<f64>,
    /// 静态文档坐标 y(未吸附时的盒位置)。
    pub static_y: f64,
    pub outer_h: f64,
    /// 布局期视口高(bottom 吸附需要)。
    pub viewport_h: f64,
    /// 包含块(父块)stable selector。
    pub cb_selector: String,
}

/// 布局收尾:求解 max_shift 并给区间内显示项打吸附标记。
///
/// 必须在 `display::sort_items` 之前调用(排序不改 seq,但保持时序清晰);
/// 包含块盒未命中(测量语境/极端裁剪)时 max_shift=0 —— 吸附退化为
/// 永不位移,不劣化既有布局。
pub(crate) fn resolve(ctx: &mut LayoutCtx) {
    let marks = std::mem::take(&mut ctx.sticky_marks);
    if marks.is_empty() {
        return;
    }
    for m in &marks {
        // 包含块底界:优先取包含块盒;布局器不给 html/body 建盒
        //(初始包含块直接从 body 子元素起排),sticky 挂在 body 直下时
        // 回落到**页高**(初始包含块 = 视口,吸附下界 = 文档底)。
        let cb_bottom = ctx
            .boxes
            .iter()
            .filter(|b| b.selector == m.cb_selector)
            .map(|b| b.y + b.h)
            .fold(f64::NEG_INFINITY, f64::max);
        let cb_bottom = if cb_bottom.is_finite() {
            cb_bottom
        } else {
            ctx.y.max(ctx.viewport_h)
        };
        let max_shift = if cb_bottom.is_finite() {
            (cb_bottom - (m.static_y + m.outer_h)).max(0.0)
        } else {
            0.0
        };
        let info = StickyInfo {
            top: m.top,
            bottom: m.bottom,
            static_y: m.static_y,
            outer_h: m.outer_h,
            viewport_h: m.viewport_h,
            max_shift,
        };
        for it in ctx.display.iter_mut() {
            // fixed 项坐标以视口为原点,吸附平移对其无意义;多层 sticky
            // 嵌套时内层先打(先登记后求解顺序相反,此处只认首个标记)。
            if it.seq >= m.start_seq && it.seq < m.end_seq && it.sticky.is_none() && !it.fixed {
                it.sticky = Some(info);
            }
        }
    }
}
