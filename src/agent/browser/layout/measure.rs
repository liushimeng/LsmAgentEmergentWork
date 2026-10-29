//! 局部内容测量(max-content/不定宽条目):`Fragment` 片段模型 + 平移/注入。
//!
//! 从 `layout/mod.rs` 机械拆分(单文件超 1800 行规范):片段在局部坐标
//! (原点 0,0)下测量,整体平移后注入全局结果,不污染全局游标。

use super::*;

/// 局部布局片段:一次内容测量的产出,可整体平移后注入全局结果。
#[derive(Default)]
pub(crate) struct Fragment {
    pub height: f64,
    pub lines: Vec<TextLine>,
    pub boxes: Vec<ElementBox>,
    pub paints: Vec<PaintBox>,
    pub borders: Vec<BorderDraw>,
    pub shadows: Vec<ShadowDraw>,
    /// 片段内发射的显示项(阶段六):随片段整体平移注入。
    pub display: Vec<DisplayItem>,
}

/// 平移片段内全部几何。
pub(crate) fn translate(fragment: &mut Fragment, dx: f64, dy: f64) {
    for l in &mut fragment.lines {
        l.x += dx;
        l.y += dy;
    }
    for b in &mut fragment.boxes {
        b.x += dx;
        b.y += dy;
    }
    for p in &mut fragment.paints {
        p.x += dx;
        p.y += dy;
    }
    for bd in &mut fragment.borders {
        bd.x += dx;
        bd.y += dy;
    }
    for s in &mut fragment.shadows {
        s.x += dx;
        s.y += dy;
    }
    for it in &mut fragment.display {
        it.translate(dx, dy);
    }
}

/// 片段注入全局结果。
pub(crate) fn append(ctx: &mut LayoutCtx, fragment: Fragment) {
    ctx.lines.extend(fragment.lines);
    ctx.boxes.extend(fragment.boxes);
    ctx.paints.extend(fragment.paints);
    ctx.borders.extend(fragment.borders);
    ctx.shadows.extend(fragment.shadows);
    ctx.display.extend(fragment.display);
}

impl<'a> LayoutCtx<'a> {
    /// 在局部坐标(原点 0,0)下测量节点内容,返回片段;全局游标与已产出
    /// 几何零副作用。`content_width` 为内容盒宽度(调用方已扣除 padding)。
    /// 节点自身是 flex/grid 容器时走对应格式化上下文(嵌套容器的子树几何正确)。
    pub(crate) fn measure_content(
        &mut self,
        node: &NodeRef,
        content_width: f64,
        style: &ComputedStyle,
        depth: usize,
    ) -> Fragment {
        // 阶段三第三步:替换元素(<img>/<svg>)没有子内容,直接按固有尺寸成盒。
        if is_replaced_tag(node) {
            let (w, h, _src) = self.replaced_size(node, style, content_width, None);
            let selector = Document::stable_selector(node);
            return Fragment {
                height: h,
                lines: Vec::new(),
                boxes: vec![ElementBox {
                    selector,
                    // 真实标签(img/svg/iframe):第 174 轮起 iframe 也是替换元素,
                    // 探针/断言按 tag 定位需要真名,不再一律写 "img"。
                    tag: document::tag_name(node).to_ascii_lowercase(),
                    x: 0.0,
                    y: 0.0,
                    w,
                    h,
                    background: style.background,
                    visible: style_visible(style),
                    z_index: style.pos.z_index,
                    out_of_flow: false,
                }],
                paints: Vec::new(),
                borders: Vec::new(),
                shadows: Vec::new(),
                display: Vec::new(),
            };
        }
        let y0 = self.y;
        let lines0 = self.lines.len();
        let boxes0 = self.boxes.len();
        let paints0 = self.paints.len();
        let borders0 = self.borders.len();
        let shadows0 = self.shadows.len();
        let display0 = self.display.len();
        self.y = 0.0;
        let height = if style.display == "flex" {
            // flex/grid 格式化上下文不推进全局游标,高度取返回值。
            flex::layout_content(
                self,
                node,
                0.0,
                content_width.max(1.0),
                style,
                depth,
                (0.0, 0.0, content_width.max(1.0), UNKNOWN_CB_H),
            )
        } else if style.display == "grid" {
            grid::layout_content(
                self,
                node,
                0.0,
                content_width.max(1.0),
                style,
                depth,
                (0.0, 0.0, content_width.max(1.0), UNKNOWN_CB_H),
            )
        } else {
            let mut buf = InlineBuf::default();
            let cb = (0.0, 0.0, content_width.max(1.0), UNKNOWN_CB_H);
            self.layout_children(node, 0.0, content_width, &mut buf, depth, style, cb);
            self.flush_inline(&mut buf, content_width);
            self.y
        };
        let lines = self.lines.split_off(lines0);
        let boxes = self.boxes.split_off(boxes0);
        let paints = self.paints.split_off(paints0);
        let borders = self.borders.split_off(borders0);
        let shadows = self.shadows.split_off(shadows0);
        let display_items = self.display.split_off(display0);
        self.y = y0;
        Fragment {
            height,
            lines,
            boxes,
            paints,
            borders,
            shadows,
            display: display_items,
        }
    }
}
