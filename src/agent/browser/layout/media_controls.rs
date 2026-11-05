//! 媒体控件条(第 50 轮):`video[controls]` 底部悬浮条与 `audio[controls]`
//! 整条形态;**媒体 poster 真实绘制**(第 55 轮)。
//!
//! 内核不解码音视频(48 轮边界不变),但 `controls` 属性声明了「这里应有
//! 播放器 UI」——此前只画空盒占位,截图取证无法区分「有控件的播放器」
//! 与「一块空白」。这里补**静态控件条**:半透明黑底 + 播放三角 +
//! 进度条(无解码态恒 0)+ 时间文案。条本体不占布局(悬浮绘制,叠在
//! 元素内容之上),发射在 `emit_canvas_pixels` 公共尾钩。
//!
//! 第 55 轮补 `emit_media_poster`:`video[poster]`/`object[data]`/
//! `embed[src]` 已解码时画真实像素并撤占位(必须先于控件条发射)。

use super::*;

/// 控件条高度(px)。
const BAR_H: f64 = 28.0;

impl<'a> LayoutCtx<'a> {
    /// video:盒底悬浮条(盒 ≥ 48×56 才画,太矮的盒画了只剩黑条);
    /// audio:整盒即控件条(固有尺寸 300×30,见 `media_natural_size`)。
    pub(crate) fn emit_media_controls(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        seq: u64,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        if !style_visible(style) || document::attr(node, "controls").is_none() {
            return;
        }
        let tag = document::tag_name(node).to_ascii_lowercase();
        let (bar_y, bar_h) = match tag.as_str() {
            "video" => {
                if w < 48.0 || h < 56.0 {
                    return;
                }
                (y + h - BAR_H, BAR_H)
            }
            "audio" => {
                if w < 48.0 || h < 12.0 {
                    return;
                }
                (y, h.min(BAR_H + 4.0))
            }
            _ => return,
        };
        let _ = seq;
        // 条背景:半透明黑(叠在海报/占位之上)。
        let bar_seq = self.next_seq();
        let z = style.pos.z_index.max(self.z_floor);
        let layer = self.layer_for(style);
        let tf = self.own_transform(style, x, y, w, h);
        self.display.push(DisplayItem {
            kind: DisplayKind::Background {
                fill: BgFill::Solid([12, 12, 12], 150),
            },
            x,
            y: bar_y,
            w,
            h: bar_h,
            radius: [CornerRadius::Px(0.0); 4],
            z,
            seq: bar_seq,
            opacity: style.opacity,
            clip: Some((x, y, w, h)),
            transform: tf,
            fixed: self.fixed_depth > 0,
            out_of_flow: false,
            layer,
            sticky: None,
        });
        // 播放三角(白色,条内左侧)。
        let g_h = (bar_h * 0.5).min(14.0);
        let g_y = bar_y + (bar_h - g_h) / 2.0;
        self.emit_widget_glyph(
            WidgetGlyphKind::PlayTriangle,
            false,
            [255, 255, 255],
            x + 10.0,
            g_y,
            g_h * 0.9,
            g_h,
            style,
            tf,
        );
        // 进度条:轨道浅白;无解码态填充恒 0。
        let track_x = x + 10.0 + g_h * 0.9 + 10.0;
        let track_w = (w - (track_x - x) - 44.0).max(12.0);
        let track_y = bar_y + bar_h / 2.0 - 1.5;
        let track_seq = self.next_seq();
        self.display.push(DisplayItem {
            kind: DisplayKind::Background {
                fill: BgFill::Solid([255, 255, 255], 110),
            },
            x: track_x,
            y: track_y,
            w: track_w,
            h: 3.0,
            radius: [CornerRadius::Px(1.5); 4],
            z,
            seq: track_seq,
            opacity: style.opacity,
            clip: Some((x, y, w, h)),
            transform: tf,
            fixed: self.fixed_depth > 0,
            out_of_flow: false,
            layer,
            sticky: None,
        });
        // 时间文案:无解码态恒 0:00。
        let tw = self.font.text_width("0:00", 10.0);
        let text_seq = self.next_seq();
        self.display.push(DisplayItem {
            kind: DisplayKind::Text {
                text: "0:00".to_string(),
                size: 10.0,
                color: [235, 235, 235],
                bold: false,
            },
            x: x + w - tw - 8.0,
            y: bar_y + (bar_h - 10.0) / 2.0,
            w: 0.0,
            h: 0.0,
            radius: [CornerRadius::Px(0.0); 4],
            z,
            seq: text_seq,
            opacity: style.opacity,
            clip: Some((x, y, w, h)),
            transform: tf,
            fixed: self.fixed_depth > 0,
            out_of_flow: false,
            layer,
            sticky: None,
        });
    }
}

impl<'a> LayoutCtx<'a> {
    /// 媒体/嵌入 poster 真实绘制(第 55 轮)。
    ///
    /// `video[poster]` / `object[data]` / `embed[src]` 的资源第 48 轮已进
    /// 预取链并用于固有尺寸,但像素一直不过绘制链 —— 播放器首帧图已下载
    /// 却只画 `MediaFrameNote` 空盒占位。这里在资源已解码时发射真实
    /// `Image`(按 `object-fit` 缺省 `fill`),并撤掉本元素的占位。
    ///
    /// 仍未解码/非图片资源时保留占位(「能力边界可见」语义不变)。
    pub(crate) fn emit_media_poster(
        &mut self,
        node: &NodeRef,
        style: &ComputedStyle,
        seq: u64,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) {
        let tag = document::tag_name(node).to_ascii_lowercase();
        if !matches!(tag.as_str(), "video" | "object" | "embed") || !style_visible(style) {
            return;
        }
        let Some(src) = self.media_image_src(node) else {
            return;
        };
        if !self.image_loaded(&src) {
            return;
        }
        // 撤掉 emit_element 画的本元素 MediaFrameNote(同 seq 尾部回扫)。
        if let Some(idx) = self.display.iter().rposition(|it| {
            it.seq == seq
                && matches!(&it.kind, DisplayKind::MediaFrameNote { tag: t, .. } if *t == tag)
        }) {
            self.display.remove(idx);
        }
        self.display.push(DisplayItem {
            kind: DisplayKind::Image {
                src,
                fit: style.object_fit.clone(),
                off_x: 0.0,
                off_y: 0.0,
            },
            x,
            y,
            w,
            h,
            radius: style.border_radius,
            z: style.pos.z_index.max(self.z_floor),
            seq,
            opacity: style.opacity,
            clip: None,
            transform: self.own_transform(style, x, y, w, h),
            fixed: self.fixed_depth > 0,
            out_of_flow: false,
            layer: self.layer_for(style),
            sticky: None,
        });
    }
}
