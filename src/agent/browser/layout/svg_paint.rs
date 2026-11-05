//! SVG 形状显示项发射(第 50 轮)。
//!
//! svg 替换元素此前只画 `MediaFrameNote` 空盒占位(白板)。这里把
//! `browser::svg::collect_shapes` 的形状集逐个发射进显示列表(填充/
//! 描边由 `render/raster.rs` 完成);**有形状时撤掉占位**,无形状保留
//! 占位(能力边界可见,与「页面本身空白」可区分)。
//!
//! 挂点:`emit_canvas_pixels`(全部替换元素建盒路径的公共尾钩,块级/
//! 行内原子/flex/grid/absolute 通吃),svg 分支先于此文件的 canvas 分支。

use super::*;

impl<'a> LayoutCtx<'a> {
    /// svg 替换元素 → 形状显示项(元素内容盒局部坐标,clip = 内容盒,
    /// 即 SVG 视口默认 `overflow:hidden`)。
    pub(crate) fn emit_svg_shapes(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        seq: u64,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        if !document::tag_name(node).eq_ignore_ascii_case("svg") || !style_visible(style) {
            return;
        }
        let content_x = x + style.padding_left;
        let content_y = y + style.padding_top;
        let content_w = (w - style.padding_left - style.padding_right).max(0.0);
        let content_h = (h - style.padding_top - style.padding_bottom).max(0.0);
        if content_w <= 0.0 || content_h <= 0.0 {
            return;
        }
        let current_color = style.color;
        let shapes = crate::agent::browser::svg::collect_shapes(
            node,
            content_w,
            content_h,
            current_color,
        );
        if shapes.is_empty() {
            return;
        }
        // 有真图形 → 撤掉 emit_element 刚画的 svg 占位(同 seq 通道尾部回扫,
        // 只撤本元素的,先于任何兄弟发射)。
        if let Some(idx) = self.display.iter().rposition(|it| {
            it.seq == seq
                && matches!(&it.kind, DisplayKind::MediaFrameNote { tag, .. } if tag == "svg")
        }) {
            self.display.remove(idx);
        }
        let tf = self.own_transform(style, x, y, w, h);
        let z = style.pos.z_index.max(self.z_floor);
        let layer = self.layer_for(style);
        for shape in shapes {
            let seq_i = self.next_seq();
            self.display.push(DisplayItem {
                kind: DisplayKind::SvgShape {
                    subpaths: std::sync::Arc::new(shape.subpaths),
                    fill: shape.fill.map(|(c, a)| ((c, a), shape.evenodd)),
                    stroke: shape.stroke,
                    stroke_w: shape.stroke_w,
                    cap_round: shape.cap_round,
                },
                x: content_x,
                y: content_y,
                w: content_w,
                h: content_h,
                radius: [CornerRadius::Px(0.0); 4],
                z,
                seq: seq_i,
                opacity: style.opacity,
                clip: Some((content_x, content_y, content_w, content_h)),
                transform: tf,
                fixed: self.fixed_depth > 0,
                out_of_flow: false,
                layer,
                sticky: None,
            });
        }
    }
}
