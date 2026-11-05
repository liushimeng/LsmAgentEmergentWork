//! SVG 形状/文本显示项发射(第 50 轮;第 55 轮补文本)。
//!
//! svg 替换元素此前只画 `MediaFrameNote` 空盒占位(白板)。这里把
//! `browser::svg::collect_nodes` 的形状/文本集按文档序逐个发射进显示列表
//! (填充/描边/渐变由 `render/raster.rs` 完成);**有形状时撤掉占位**,
//! 无形状保留占位(能力边界可见,与「页面本身空白」可区分)。
//!
//! 挂点:`emit_canvas_pixels`(全部替换元素建盒路径的公共尾钩,块级/
//! 行内原子/flex/grid/absolute 通吃),svg 分支先于此文件的 canvas 分支。

use super::*;
use crate::agent::browser::svg::SvgNode;

impl<'a> LayoutCtx<'a> {
    /// svg 替换元素 → 形状/文本显示项(元素内容盒局部坐标,clip = 内容盒,
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
        let nodes = crate::agent::browser::svg::collect_nodes(
            node,
            content_w,
            content_h,
            style.color,
        );
        if nodes.is_empty() {
            return;
        }
        // 有「可绘制」节点才撤占位:`<image>` 需资源已解码,否则保留占位
        //(能力边界可见,与「页面本身空白」可区分)。
        let drawable = |im: &crate::agent::browser::svg::SvgImage| {
            crate::agent::browser::urlutil::resolve(&self.base_url, &im.href)
                .map(|abs| self.image_loaded(&abs))
                .unwrap_or(false)
        };
        let any = nodes.iter().any(|n| match n {
            SvgNode::Shape(_) | SvgNode::Text(_) => true,
            SvgNode::Image(im) => drawable(im),
        });
        if !any {
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
        let clip = Some((content_x, content_y, content_w, content_h));
        for node in nodes {
            match node {
                SvgNode::Image(im) => {
                    let Some(abs) = crate::agent::browser::urlutil::resolve(&self.base_url, &im.href)
                    else {
                        continue;
                    };
                    if !self.image_loaded(&abs) {
                        continue;
                    }
                    // preserveAspectRatio:缺省 xMidYMid meet → contain;
                    // slice → cover;none → fill。
                    let fit = if im.stretch {
                        "fill"
                    } else if im.slice {
                        "cover"
                    } else {
                        "contain"
                    };
                    let seq_i = self.next_seq();
                    self.display.push(DisplayItem {
                        kind: DisplayKind::Image {
                            src: abs,
                            fit: fit.to_string(),
                            off_x: 0.0,
                            off_y: 0.0,
                        },
                        x: content_x + im.x,
                        y: content_y + im.y,
                        w: im.w,
                        h: im.h,
                        radius: [CornerRadius::Px(0.0); 4],
                        z,
                        seq: seq_i,
                        opacity: style.opacity,
                        clip,
                        transform: tf,
                        fixed: self.fixed_depth > 0,
                        out_of_flow: false,
                        layer,
                        sticky: None,
                    });
                }
                SvgNode::Shape(shape) => {
                    let seq_i = self.next_seq();
                    self.display.push(DisplayItem {
                        kind: DisplayKind::SvgShape {
                            subpaths: std::sync::Arc::new(shape.subpaths),
                            fill: shape.fill.map(|p| (p, shape.evenodd)),
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
                        clip,
                        transform: tf,
                        fixed: self.fixed_depth > 0,
                        out_of_flow: false,
                        layer,
                        sticky: None,
                    });
                }
                SvgNode::Text(text) => {
                    // text-anchor:按文本宽水平偏移(start=0/middle=0.5/end=1)。
                    let tw = self.font.text_width(&text.text, text.size);
                    let dx = -text.anchor * tw;
                    // dominant-baseline:middle|central → y 为字形盒垂直中心,
                    // 行顶按 0.35em 近似;缺省 y 即字母基线。
                    let baseline = if text.central {
                        text.y + text.size * 0.35
                    } else {
                        text.y
                    };
                    let top = baseline - text.size;
                    let seq_i = self.next_seq();
                    self.display.push(DisplayItem {
                        kind: DisplayKind::Text {
                            text: text.text,
                            size: text.size,
                            color: text.color,
                            bold: text.bold,
                        },
                        x: content_x + text.x + dx,
                        y: content_y + top,
                        w: tw,
                        h: text.size,
                        radius: [CornerRadius::Px(0.0); 4],
                        z,
                        seq: seq_i,
                        opacity: style.opacity * text.alpha,
                        clip,
                        transform: tf,
                        fixed: self.fixed_depth > 0,
                        out_of_flow: false,
                        layer,
                        sticky: None,
                    });
                }
            }
        }
    }
}
