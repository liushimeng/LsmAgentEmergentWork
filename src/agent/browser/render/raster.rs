//! 显示列表光栅化器(阶段六第一步):逐项绘制,统一圆角几何 / scissor 裁剪 /
//! 透明度乘法三条主线。
//!
//! 全部图元走**手写逐像素填充**(像素中心采样、硬边),不依赖 tiny-skia 的
//! 路径/mask 能力——原因:① 圆角矩形是凸区域,逐行跨度有解析解,逐像素
//! 判定 O(1);② scissor(overflow 裁剪 ∩ 视口)与 alpha(组透明度)可以在
//! 同一套像素循环里自然落地;③ 硬边输出让像素断言确定性可复现。
//! 图片/文字沿用既有双线性采样与字形 coverage 混合。

use tiny_skia::{Pixmap, PremultipliedColorU8};

use super::super::images::{DecodedImage, ImageStore};
use super::super::style::{Affine, CornerRadius, GradientKind, GradientSpec};
use super::super::svg::{GradientPaint, GradientStop, SpreadMethod, SvgPaint};
use super::blur::{blur_mask, half_width_for, MAX_MASK_PIXELS};
use super::display::{BgFill, DisplayItem, DisplayKind, TextDecorKind, WidgetGlyphKind};
use super::FontSet;

/// 像素裁剪窗口(半开区间 `[x0,x1) × [y0,y1)`,画布坐标)。
#[derive(Clone, Copy)]
struct Scissor {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl Scissor {
    fn full(pw: i64, ph: i64) -> Self {
        Self {
            x0: 0,
            y0: 0,
            x1: pw,
            y1: ph,
        }
    }

    fn intersect_rect(&self, x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            x0: self.x0.max(x.floor() as i64),
            y0: self.y0.max(y.floor() as i64),
            x1: self.x1.min((x + w).ceil() as i64),
            y1: self.y1.min((y + h).ceil() as i64),
        }
    }

    fn empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }
}

/// 项级裁剪窗口:项自带 clip(与项盒同一坐标系)变换后平移滚动量再与画布取交。
///
/// ★ clip 必须随项的 transform 一起变换:clip 记录的是**变换前**的裁剪窗口,
/// 项的画布位置却由 transform 决定 —— 两者不同步时,变换后的内容会被留在
/// 原位的裁剪窗口整个裁掉(实测猎聘登录框 `translate(-50%,-50%)` 后白色
/// 面板整块消失,只剩边框)。
fn scissor_for(item: &DisplayItem, scroll_x: f64, scroll_y: f64, pw: i64, ph: i64) -> Scissor {
    let mut sc = Scissor::full(pw, ph);
    if let Some((cx, cy, cw, ch)) = item.clip {
        let (x0, y0, x1, y1) = match item.transform.as_ref() {
            Some(t) => {
                let p0 = t.apply(cx, cy);
                let p1 = t.apply(cx + cw, cy + ch);
                (
                    p0.0.min(p1.0),
                    p0.1.min(p1.1),
                    p0.0.max(p1.0),
                    p0.1.max(p1.1),
                )
            }
            None => (cx, cy, cx + cw, cy + ch),
        };
        sc = sc.intersect_rect(x0 - scroll_x, y0 - scroll_y, x1 - x0, y1 - y0);
    }
    sc
}

/// 画布像素 → 项**局部(页坐标)**映射(阶段六第二步:CSS 2D transform 逆映射)。
///
/// 无变换时退化为恒等 + 滚动偏移(`local = (px+0.5+scroll_x, py+0.5+scroll_y)`),
/// 与重构前「画布坐标 == 页坐标 - 滚动量」逐像素等价。
struct Mapper<'a> {
    inv: Option<Affine>,
    scroll_x: f64,
    scroll_y: f64,
    item: &'a DisplayItem,
}

impl<'a> Mapper<'a> {
    fn new(item: &'a DisplayItem, scroll_x: f64, scroll_y: f64) -> Self {
        Self {
            inv: item.transform.and_then(|t| t.invert()),
            scroll_x,
            scroll_y,
            item,
        }
    }

    /// 画布像素中心 → 页坐标局部点。
    #[inline]
    fn local(&self, px: i64, py: i64) -> (f64, f64) {
        let x = px as f64 + 0.5 + self.scroll_x;
        let y = py as f64 + 0.5 + self.scroll_y;
        match self.inv {
            None => (x, y),
            Some(m) => (m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f),
        }
    }

    /// 局部盒变换后的画布 AABB(x0, y0, w, h)。无变换时 = `(x-scroll_x, y-scroll_y, w, h)`。
    fn canvas_aabb(&self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
        let Some(t) = self.item.transform else {
            return (x - self.scroll_x, y - self.scroll_y, w, h);
        };
        let corners = [
            t.apply(x, y),
            t.apply(x + w, y),
            t.apply(x, y + h),
            t.apply(x + w, y + h),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (cx, cy) in corners {
            x0 = x0.min(cx);
            x1 = x1.max(cx);
            y0 = y0.min(cy);
            y1 = y1.max(cy);
        }
        (x0 - self.scroll_x, y0 - self.scroll_y, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    /// 局部盒 → 画布 scissor(与视口/项 clip 取交)。
    fn scissor_of(&self, sc: &Scissor, x: f64, y: f64, w: f64, h: f64) -> Scissor {
        let (ax, ay, aw, ah) = self.canvas_aabb(x, y, w, h);
        sc.intersect_rect(ax, ay, aw, ah)
    }
}

/// 四角半径解析:百分比按 `min(w,h)/2` 求解,长度钳制到 `min(w,h)/2`。
fn resolve_radius(radius: &[CornerRadius; 4], w: f64, h: f64) -> [f64; 4] {
    let half = (w.min(h) / 2.0).max(0.0);
    let one = |c: &CornerRadius| -> f64 {
        match c {
            CornerRadius::Px(v) => *v,
            CornerRadius::Pct(p) => p / 100.0 * half,
        }
        .clamp(0.0, half)
    };
    [
        one(&radius[0]),
        one(&radius[1]),
        one(&radius[2]),
        one(&radius[3]),
    ]
}

/// 像素中心是否在(轴对齐)圆角矩形内。半径顺序 tl/tr/br/bl。
fn inside_rounded(px: f64, py: f64, x: f64, y: f64, w: f64, h: f64, r: &[f64; 4]) -> bool {
    if w <= 0.0 || h <= 0.0 {
        return false;
    }
    if px < x || px > x + w || py < y || py > y + h {
        return false;
    }
    // 四角方块外的区域必在矩形内;落在角方块内时按圆判定。
    let corners = [
        (x + r[0], y + r[0], x, y),                 // tl
        (x + w - r[1], y + r[1], x + w, y),         // tr
        (x + w - r[2], y + h - r[2], x + w, y + h), // br
        (x + r[3], y + h - r[3], x, y + h),         // bl
    ];
    for (i, &(cx, cy, ox, oy)) in corners.iter().enumerate() {
        let rr = r[i];
        if rr <= 0.0 {
            continue;
        }
        let in_sq_x = if ox == x { px <= cx } else { px >= cx };
        let in_sq_y = if oy == y { py <= cy } else { py >= cy };
        if in_sq_x && in_sq_y {
            let dx = px - cx;
            let dy = py - cy;
            return dx * dx + dy * dy <= rr * rr;
        }
    }
    true
}

/// 直角(alpha 混合)写一个像素。调用方保证坐标在画布内。
#[inline]
fn blend_pixel(
    pixels: &mut [PremultipliedColorU8],
    stride: i64,
    x: i64,
    y: i64,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    if a == 0 {
        return;
    }
    let idx = (y * stride + x) as usize;
    let old = pixels[idx].demultiply();
    let ia = 255 - a as u32;
    let nr = ((r as u32 * a as u32 + old.red() as u32 * ia) / 255) as u8;
    let ng = ((g as u32 * a as u32 + old.green() as u32 * ia) / 255) as u8;
    let nb = ((b as u32 * a as u32 + old.blue() as u32 * ia) / 255) as u8;
    if let Some(px) = PremultipliedColorU8::from_rgba(nr, ng, nb, 255) {
        pixels[idx] = px;
    }
}

/// 显示列表光栅化主入口。
pub(crate) fn rasterize(
    font: &FontSet,
    pixmap: &mut Pixmap,
    items: &[DisplayItem],
    scroll_x: f64,
    scroll_y: f64,
    images: &ImageStore,
) {
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    for item in items {
        if item.opacity <= 0.0 {
            continue;
        }
        // `position: fixed`:盒坐标以**视口**为原点(布局层对 fixed 取包含块
        // (0,0,vw,vh)),与普通项的页面原点不同系 —— 这里必须把滚动量归零,
        // 否则固定控件会随页面滚动平移(地图缩放控件/工具栏一滚就跑位)。
        // 第 50 轮:sticky 子树按滚动量现算吸附平移,与滚动偏移同一通道,
        // 页面滚动不触发重排。画布位置 = 页坐标 - 滚动量 + 吸附位移,
        // 即等效滚动量 = scroll_y - shift。
        let sticky_dy = item.sticky.as_ref().map_or(0.0, |s| s.shift(scroll_y));
        let (item_sx, item_sy) = if item.fixed {
            (0.0, 0.0)
        } else {
            (scroll_x, scroll_y - sticky_dy)
        };
        let mapper = Mapper::new(item, item_sx, item_sy);
        // 局部盒:文本类按内容宽度(字形逐字绘制,盒宽取文本宽)。
        let (bx, by, bw, bh) = match &item.kind {
            DisplayKind::Text { text, size, .. }
            | DisplayKind::TextShadow { text, size, .. } => {
                (item.x, item.y, font.text_width(text, *size), *size * 2.0)
            }
            _ => (item.x, item.y, item.w, item.h),
        };
        let (ax, ay, aw, ah) = mapper.canvas_aabb(bx, by, bw, bh);
        // 便宜剔除:整项(含阴影扩张)在画布外。
        let pad = match &item.kind {
            DisplayKind::Shadow { blur, spread, .. } => *blur + *spread + 8.0,
            _ => 0.0,
        };
        let (ex0, ey0, ex1, ey1) = (ax - pad, ay - pad, ax + aw + pad, ay + ah + pad);
        if ex1 < 0.0 || ex0 > pw as f64 || ey1 < 0.0 || ey0 > ph as f64 {
            continue;
        }
        let sc = scissor_for(item, item_sx, item_sy, pw, ph);
        if sc.empty() {
            continue;
        }
        match &item.kind {
            DisplayKind::Background { fill } => {
                draw_background(pixmap, &sc, &mapper, item, fill);
            }
            DisplayKind::Border { widths, colors } => {
                draw_border(pixmap, &sc, &mapper, item, widths, colors);
            }
            DisplayKind::Shadow {
                dx,
                dy,
                blur,
                spread,
                color,
            } => {
                draw_shadow(pixmap, &sc, &mapper, item, *dx, *dy, *blur, *spread, *color);
            }
            DisplayKind::Image {
                src,
                fit,
                off_x,
                off_y,
            } => {
                if let Some(img) = images.get(src) {
                    draw_image(pixmap, &sc, &mapper, img, item, fit, *off_x, *off_y);
                }
            }
            DisplayKind::Text {
                text,
                size,
                color,
                bold,
            } => {
                draw_text(
                    font, pixmap, &sc, &mapper, text, item.x, item.y, *size, *color, *bold,
                    item.opacity, None,
                );
            }
            DisplayKind::TextShadow {
                text,
                size,
                color,
                blur,
            } => {
                draw_text(
                    font, pixmap, &sc, &mapper, text, item.x, item.y, *size, *color, false,
                    item.opacity, Some(*blur),
                );
            }
            DisplayKind::CanvasNote { pale } => {
                draw_canvas_note(font, pixmap, &sc, &mapper, item, *pale);
            }
            DisplayKind::MediaFrameNote { pale, tag } => {
                draw_media_frame_note(font, pixmap, &sc, &mapper, item, *pale, tag);
            }
            DisplayKind::TextDecor { kind, color } => {
                draw_text_decor(pixmap, &sc, &mapper, item, *kind, *color);
            }
            DisplayKind::WidgetGlyph { glyph, open, color } => {
                draw_widget_glyph(pixmap, &sc, &mapper, item, *glyph, *open, *color);
            }
            DisplayKind::SvgShape {
                subpaths,
                fill,
                stroke,
                stroke_w,
                cap_round,
            } => {
                draw_svg_shape(
                    pixmap, &sc, &mapper, item, subpaths, fill.clone(), stroke.clone(), *stroke_w,
                    *cap_round,
                );
            }
        }
    }
}

/// 控件几何图元:全部以盒内归一化坐标判定,天然支持 transform 与 scissor。
fn draw_widget_glyph(
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    item: &DisplayItem,
    glyph: WidgetGlyphKind,
    open: bool,
    color: [u8; 3],
) {
    let rect = mapper.scissor_of(sc, item.x, item.y, item.w, item.h);
    if rect.empty() {
        return;
    }
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    let a = (item.opacity * 255.0).round().clamp(0.0, 255.0) as u8;
    for py in rect.y0..rect.y1 {
        for px in rect.x0..rect.x1 {
            let (lx, ly) = mapper.local(px, py);
            if glyph_contains(glyph, open, lx, ly, item.x, item.y, item.w, item.h) {
                blend_pixel(pixels, w, px, py, color[0], color[1], color[2], a);
            }
        }
    }
}

fn glyph_contains(
    glyph: WidgetGlyphKind,
    open: bool,
    lx: f64,
    ly: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> bool {
    let nx = ((lx - x) / w.max(1.0)).clamp(0.0, 1.0);
    let ny = ((ly - y) / h.max(1.0)).clamp(0.0, 1.0);
    match glyph {
        WidgetGlyphKind::CheckMark => {
            // 两段折线构成勾:粗细按短边比例,端点均在盒内留白。
            let t = 0.11;
            let p0 = (0.18, 0.52);
            let p1 = (0.42, 0.75);
            let p2 = (0.82, 0.25);
            dist_segment(nx, ny, p0.0, p0.1, p1.0, p1.1) <= t
                || dist_segment(nx, ny, p1.0, p1.1, p2.0, p2.1) <= t
        }
        WidgetGlyphKind::RadioDot => {
            let r = 0.24;
            (nx - 0.5).powi(2) + (ny - 0.5).powi(2) <= r * r
        }
        WidgetGlyphKind::Bullet => {
            let r = 0.12;
            (nx - 0.5).powi(2) + (ny - 0.5).powi(2) <= r * r
        }
        WidgetGlyphKind::BulletRing => {
            // 未选中 radio:圆环轮廓(内外半径之间)。
            let d = ((nx - 0.5).powi(2) + (ny - 0.5).powi(2)).sqrt();
            d <= 0.40 && d >= 0.27
        }
        WidgetGlyphKind::BulletSquare => {
            // `list-style-type: square`:实心方块。
            (nx - 0.5).abs() <= 0.15 && (ny - 0.5).abs() <= 0.15
        }
        WidgetGlyphKind::Indeterminate => {
            // checkbox 半选:居中横杠。
            ny >= 0.43 && ny <= 0.57 && nx >= 0.18 && nx <= 0.82
        }
        WidgetGlyphKind::SliderThumb => {
            // 滑块拇指:竖向圆角条(盒高由布局给定,此处只判横向占比)。
            (nx - 0.5).abs() <= 0.26 && ny >= 0.06 && ny <= 0.94
        }
        WidgetGlyphKind::PlayTriangle => {
            // 播放三角:右指实心三角形(左顶点在 0.1,底边在 0.9)。
            let t = ((nx - 0.1) / 0.8).clamp(0.0, 1.0);
            let half = 0.42 * t;
            t > 0.0 && (ny - 0.5).abs() <= half
        }
        WidgetGlyphKind::CalendarIcon => {
            // 日历:外框 + 顶栏 + 上下两条装订短线。
            let frame = nx >= 0.12 && nx <= 0.88 && ny >= 0.16 && ny <= 0.86;
            let stroke = frame
                && (nx <= 0.24
                    || nx >= 0.76
                    || ny <= 0.36
                    || ny >= 0.74);
            let header = nx >= 0.12 && nx <= 0.88 && ny >= 0.16 && ny <= 0.34;
            let hangers = (nx - 0.32).abs() <= 0.07 && ny >= 0.04 && ny <= 0.26;
            let hangers2 = (nx - 0.68).abs() <= 0.07 && ny >= 0.04 && ny <= 0.26;
            stroke || header || hangers || hangers2
        }
        WidgetGlyphKind::ClockIcon => {
            // 时钟:圆环 + 时针(竖)与分针(横)。
            let d = ((nx - 0.5).powi(2) + (ny - 0.5).powi(2)).sqrt();
            let ring = d <= 0.44 && d >= 0.32;
            let hour = (nx - 0.5).abs() <= 0.07 && ny >= 0.26 && ny <= 0.54;
            let minute = (ny - 0.54).abs() <= 0.07 && nx >= 0.46 && nx <= 0.70;
            ring || hour || minute
        }
        WidgetGlyphKind::CaretUp => {
            // 向上三角:下宽上尖。
            ny >= 0.32 && ny <= 0.66 && (0.66 - ny) / 0.34 >= (nx - 0.5).abs() / 0.22
        }
        WidgetGlyphKind::Caret => {
            if open {
                // 向下三角:上宽下尖。
                ny >= 0.34 && ny <= 0.68 && (ny - 0.34) / 0.34 >= (nx - 0.5).abs() / 0.22
            } else {
                // 向右三角:左尖右宽。
                nx >= 0.34 && nx <= 0.68 && (nx - 0.34) / 0.34 >= (ny - 0.5).abs() / 0.22
            }
        }
    }
}

fn dist_segment(px: f64, py: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f64::EPSILON {
        0.0
    } else {
        (((px - x0) * dx + (py - y0) * dy) / len2).clamp(0.0, 1.0)
    };
    let qx = x0 + t * dx;
    let qy = y0 + t * dy;
    ((px - qx).powi(2) + (py - qy).powi(2)).sqrt()
}

/// 第 50 轮:SVG 形状光栅化(第 55 轮补渐变)。
///
/// 点集为元素内容盒局部坐标(item.x/y 为内容盒原点,页坐标系)。填充走
/// 2×2 超采样扫描判定(evenodd / 非零卷绕),描边走距离场(1px 羽化,
/// butt/round cap);填充先画、描边后画(SVG 语义)。
///
/// 渐变按**形状 bbox**(objectBoundingBox)或**预映射局部坐标**
/// (userSpaceOnUse)逐采样点求 `t`,再按 `spreadMethod` 与色标插值。
#[allow(clippy::too_many_arguments)]
fn draw_svg_shape(
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    item: &DisplayItem,
    subpaths: &[(Vec<(f64, f64)>, bool)],
    fill: Option<(SvgPaint, bool)>,
    stroke: Option<SvgPaint>,
    stroke_w: f64,
    cap_round: bool,
) {
    if subpaths.is_empty() {
        return;
    }
    let has_fill = fill.is_some();
    let has_stroke = stroke.is_some() && stroke_w > 0.0;
    if !has_fill && !has_stroke {
        return;
    }
    // 局部坐标 bbox → 画布 scissor(描边外扩半宽 + 羽化)。
    let (mut x0, mut y0, mut x1, mut y1) =
        (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (pts, _) in subpaths {
        for p in pts {
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
    }
    let half = if has_stroke { stroke_w / 2.0 + 1.0 } else { 0.0 };
    let rect = mapper.scissor_of(
        sc,
        item.x + x0 - half,
        item.y + y0 - half,
        (x1 - x0) + half * 2.0,
        (y1 - y0) + half * 2.0,
    );
    if rect.empty() {
        return;
    }
    let evenodd = fill.as_ref().map_or(false, |(_, e)| *e);
    // objectBoundingBox 渐变的单位方形基准(形状几何 bbox,局部坐标)。
    let bbox = (x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
    let hw = stroke_w / 2.0;
    let samples: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    for py in rect.y0..rect.y1 {
        for px in rect.x0..rect.x1 {
            let mut fill_cov = 0.0f64;
            let mut stroke_cov = 0.0f64;
            let mut fill_acc = (0u32, 0u32, 0u32, 0u32);
            let mut stroke_acc = (0u32, 0u32, 0u32, 0u32);
            let (mut fill_n, mut stroke_n) = (0u32, 0u32);
            for (ox, oy) in samples {
                // 画布亚像素 → 页坐标局部(与 Mapper::local 同式,含逆变换)。
                let cx = px as f64 + ox + mapper.scroll_x;
                let cy = py as f64 + oy + mapper.scroll_y;
                let (lx, ly) = match mapper.inv {
                    None => (cx, cy),
                    Some(m) => (m.a * cx + m.c * cy + m.e, m.b * cx + m.d * cy + m.f),
                };
                let sx = lx - item.x;
                let sy = ly - item.y;
                if has_fill && fill_contains(subpaths, sx, sy, evenodd) {
                    fill_cov += 0.25;
                    fill_n += 1;
                    if let Some((paint, _)) = &fill {
                        let (c, a) = svg_paint_at(paint, bbox, sx, sy);
                        fill_acc.0 += c[0] as u32;
                        fill_acc.1 += c[1] as u32;
                        fill_acc.2 += c[2] as u32;
                        fill_acc.3 += (a * 255.0).round() as u32;
                    }
                }
                if has_stroke {
                    let cov = stroke_coverage(subpaths, sx, sy, hw, cap_round);
                    if cov > 0.0 {
                        stroke_cov += 0.25 * cov;
                        stroke_n += 1;
                        if let Some(paint) = &stroke {
                            let (c, a) = svg_paint_at(paint, bbox, sx, sy);
                            stroke_acc.0 += c[0] as u32;
                            stroke_acc.1 += c[1] as u32;
                            stroke_acc.2 += c[2] as u32;
                            stroke_acc.3 += (a * 255.0).round() as u32;
                        }
                    }
                }
            }
            if fill_cov > 0.0 && fill_n > 0 {
                let a = (fill_acc.3 / fill_n) as f64 / 255.0 * fill_cov * item.opacity * 255.0;
                blend_pixel(
                    pixels,
                    w,
                    px,
                    py,
                    (fill_acc.0 / fill_n) as u8,
                    (fill_acc.1 / fill_n) as u8,
                    (fill_acc.2 / fill_n) as u8,
                    a.round().clamp(0.0, 255.0) as u8,
                );
            }
            if stroke_cov > 0.0 && stroke_n > 0 {
                let a =
                    (stroke_acc.3 / stroke_n) as f64 / 255.0 * stroke_cov * item.opacity * 255.0;
                blend_pixel(
                    pixels,
                    w,
                    px,
                    py,
                    (stroke_acc.0 / stroke_n) as u8,
                    (stroke_acc.1 / stroke_n) as u8,
                    (stroke_acc.2 / stroke_n) as u8,
                    a.round().clamp(0.0, 255.0) as u8,
                );
            }
        }
    }
}

/// 绘制属性在局部点 `(x,y)` 处的颜色与 alpha(0..1)。
fn svg_paint_at(paint: &SvgPaint, bbox: (f64, f64, f64, f64), x: f64, y: f64) -> ([u8; 3], f64) {
    match paint {
        SvgPaint::Solid(c, a) => (*c, *a),
        SvgPaint::Gradient(g) => match svg_gradient_t(g, bbox, x, y) {
            Some(t) => svg_gradient_color(&g.stops, t),
            None => ([0, 0, 0], 0.0),
        },
    }
}

/// 渐变参数 `t`(含 `spreadMethod`)。
///
/// - `userSpaceOnUse`:`coords` 已是内容盒局部坐标,直接算;
/// - `objectBoundingBox`:把局部点归一进单位方形,再按 `inv`
///   (`gradientTransform` 的逆)变回渐变空间。
fn svg_gradient_t(paint: &GradientPaint, bbox: (f64, f64, f64, f64), x: f64, y: f64) -> Option<f64> {
    let c = &paint.coords;
    let (px, py) = if paint.user_space {
        (x, y)
    } else {
        let (bx, by, bw, bh) = bbox;
        let qx = (x - bx) / bw.max(1e-9);
        let qy = (y - by) / bh.max(1e-9);
        match paint.inv {
            Some(inv) => inv.apply(qx, qy),
            None => (qx, qy),
        }
    };
    let raw = if paint.radial {
        let r = c[2];
        if !(r > 0.0) {
            return None;
        }
        // 焦点 fx/fy 偏移不建模(按圆心距离近似,见模块边界说明)。
        ((px - c[0]).powi(2) + (py - c[1]).powi(2)).sqrt() / r
    } else {
        let dx = c[2] - c[0];
        let dy = c[3] - c[1];
        let len2 = dx * dx + dy * dy;
        if len2 <= 1e-12 {
            0.0
        } else {
            ((px - c[0]) * dx + (py - c[1]) * dy) / len2
        }
    };
    Some(match paint.spread {
        SpreadMethod::Pad => raw.clamp(0.0, 1.0),
        SpreadMethod::Reflect => {
            let m = raw.abs() % 2.0;
            if m > 1.0 {
                2.0 - m
            } else {
                m
            }
        }
        SpreadMethod::Repeat => {
            let m = raw % 1.0;
            if m < 0.0 {
                m + 1.0
            } else {
                m
            }
        }
    })
}

/// 色标插值(sRGB 分量 + alpha 线性;首尾 Pad)。
fn svg_gradient_color(stops: &[GradientStop], t: f64) -> ([u8; 3], f64) {
    if stops.is_empty() {
        return ([0, 0, 0], 0.0);
    }
    let first = &stops[0];
    if t <= first.offset {
        return (first.color, first.alpha);
    }
    let last = &stops[stops.len() - 1];
    if t >= last.offset {
        return (last.color, last.alpha);
    }
    for i in 0..stops.len() - 1 {
        let a = &stops[i];
        let b = &stops[i + 1];
        if t >= a.offset && t <= b.offset {
            let span = (b.offset - a.offset).max(1e-9);
            let k = ((t - a.offset) / span).clamp(0.0, 1.0);
            let lerp = |va: u8, vb: u8| (va as f64 * (1.0 - k) + vb as f64 * k).round() as u8;
            return (
                [
                    lerp(a.color[0], b.color[0]),
                    lerp(a.color[1], b.color[1]),
                    lerp(a.color[2], b.color[2]),
                ],
                a.alpha * (1.0 - k) + b.alpha * k,
            );
        }
    }
    (last.color, last.alpha)
}

/// 扫描线点在多边形内判定(evenodd 交叉计数 / nonzero 卷绕数,跨全部子路径)。
fn fill_contains(subpaths: &[(Vec<(f64, f64)>, bool)], x: f64, y: f64, evenodd: bool) -> bool {
    let mut winding = 0i32;
    let mut crossings = 0i32;
    for (pts, _) in subpaths {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            if (a.1 <= y) != (b.1 <= y) {
                let t = (y - a.1) / (b.1 - a.1);
                let cx = a.0 + t * (b.0 - a.0);
                if cx > x {
                    crossings += 1;
                    winding += if b.1 > a.1 { 1 } else { -1 };
                }
            }
        }
    }
    if evenodd {
        crossings % 2 == 1
    } else {
        winding != 0
    }
}

/// 描边覆盖(距离场):到各段最短距离 ≤ 半宽,1px 羽化;闭合子路径补
/// 闭合段,round cap 额外计端点圆。
fn stroke_coverage(
    subpaths: &[(Vec<(f64, f64)>, bool)],
    x: f64,
    y: f64,
    half_w: f64,
    cap_round: bool,
) -> f64 {
    let mut best = f64::INFINITY;
    for (pts, closed) in subpaths {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        for i in 0..n - 1 {
            let d = dist_segment(x, y, pts[i].0, pts[i].1, pts[i + 1].0, pts[i + 1].1);
            if d < best {
                best = d;
            }
        }
        if *closed {
            let d = dist_segment(x, y, pts[n - 1].0, pts[n - 1].1, pts[0].0, pts[0].1);
            if d < best {
                best = d;
            }
        } else if cap_round {
            for p in [pts[0], pts[n - 1]] {
                let d = ((x - p.0).powi(2) + (y - p.1).powi(2)).sqrt();
                if d < best {
                    best = d;
                }
            }
        }
    }
    (half_w + 0.5 - best).clamp(0.0, 1.0)
}

/// 背景填充:纯色圆角矩形或渐变(渐变按像素求值,支持圆角、裁剪与 transform)。
fn draw_background(pixmap: &mut Pixmap, sc: &Scissor, mapper: &Mapper, item: &DisplayItem, fill: &BgFill) {
    let rect = mapper.scissor_of(sc, item.x, item.y, item.w, item.h);
    if rect.empty() {
        return;
    }
    let radius = resolve_radius(&item.radius, item.w, item.h);
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    match fill {
        BgFill::Solid(rgb, alpha) => {
            // 背景色自带 alpha:与已绘制的底层背景合成(半透明页头/卡片/遮罩
            // 都靠这一层还原真实浏览器观感)。
            let a = ((*alpha as f64 / 255.0) * item.opacity * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            for py in rect.y0..rect.y1 {
                for px in rect.x0..rect.x1 {
                    let (lx, ly) = mapper.local(px, py);
                    if inside_rounded(lx, ly, item.x, item.y, item.w, item.h, &radius) {
                        blend_pixel(pixels, w, px, py, rgb[0], rgb[1], rgb[2], a);
                    }
                }
            }
        }
        BgFill::Gradient(spec) => {
            for py in rect.y0..rect.y1 {
                for px in rect.x0..rect.x1 {
                    let (lx, ly) = mapper.local(px, py);
                    if !inside_rounded(lx, ly, item.x, item.y, item.w, item.h, &radius) {
                        continue;
                    }
                    let t = gradient_t(spec, lx, ly, item.x, item.y, item.w, item.h);
                    let (rgb, alpha) = gradient_color_at(spec, t);
                    let a = (alpha as f64 * item.opacity).round().clamp(0.0, 255.0) as u8;
                    blend_pixel(pixels, w, px, py, rgb[0], rgb[1], rgb[2], a);
                }
            }
        }
    }
}

/// 边框环带:逐边判定「在外圆角矩形内 && 不在该边内缩圆角矩形内」。
/// 半径按各边线宽缩减(内缘圆角半径 = 外半径 - 线宽,下限 0)。
fn draw_border(
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    item: &DisplayItem,
    widths: &[f64; 4],
    colors: &[([u8; 3], u8); 4],
) {
    let outer_r = resolve_radius(&item.radius, item.w, item.h);
    let rect = mapper.scissor_of(sc, item.x, item.y, item.w, item.h);
    if rect.empty() {
        return;
    }
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    // 每边的内侧矩形与内侧半径(仅该边内缩)。
    let inners: [(f64, f64, f64, f64, [f64; 4]); 4] = [
        (
            item.x,
            item.y + widths[0],
            item.w,
            (item.h - widths[0]).max(0.0),
            shrink(outer_r, widths[0]),
        ),
        (
            item.x + widths[1],
            item.y,
            (item.w - widths[1]).max(0.0),
            item.h,
            shrink(outer_r, widths[1]),
        ),
        (
            item.x,
            item.y,
            item.w,
            (item.h - widths[2]).max(0.0),
            shrink(outer_r, widths[2]),
        ),
        (
            item.x + widths[3],
            item.y,
            (item.w - widths[3]).max(0.0),
            item.h,
            shrink(outer_r, widths[3]),
        ),
    ];
    for py in rect.y0..rect.y1 {
        for px in rect.x0..rect.x1 {
            let (cx, cy) = mapper.local(px, py);
            if !inside_rounded(cx, cy, item.x, item.y, item.w, item.h, &outer_r) {
                continue;
            }
            for side in [3usize, 2, 1, 0] {
                let (ix, iy, iw, ih, ir) = inners[side];
                if widths[side] <= 0.0 {
                    continue;
                }
                if !inside_rounded(cx, cy, ix, iy, iw, ih, &ir) {
                    // 命中该边环带。反序扫描(left→bottom→right→top)首中即取,
                    // 等价于旧直角管线「后画的边覆盖先画的角交叠区」。
                    let (rgb, a) = colors[side];
                    let alpha = (a as f64 * item.opacity).round().clamp(0.0, 255.0) as u8;
                    blend_pixel(pixels, w, px, py, rgb[0], rgb[1], rgb[2], alpha);
                    break;
                }
            }
        }
    }
}

/// 某边内缩后的四角半径:按该边线宽统一缩减(近似;精确 CSS 需按两端线宽
/// 分别缩减对角,视觉差异 < 1px)。
fn shrink(r: [f64; 4], width: f64) -> [f64; 4] {
    [
        (r[0] - width).max(0.0),
        (r[1] - width).max(0.0),
        (r[2] - width).max(0.0),
        (r[3] - width).max(0.0),
    ]
}

/// 外阴影:blur=0 时实心圆角矩形(旧行为);否则蒙版 + 三趟盒式模糊。
fn draw_shadow(
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    item: &DisplayItem,
    dx: f64,
    dy: f64,
    blur: f64,
    spread: f64,
    color: ([u8; 3], u8),
) {
    let sw = item.w + spread * 2.0;
    let sh = item.h + spread * 2.0;
    let sx = item.x + dx - spread;
    let sy = item.y + dy - spread;
    let outer_r = resolve_radius(&item.radius, item.w, item.h);
    let s_radius = [
        (outer_r[0] + spread).max(0.0),
        (outer_r[1] + spread).max(0.0),
        (outer_r[2] + spread).max(0.0),
        (outer_r[3] + spread).max(0.0),
    ];
    let (rgb, ca) = color;
    if blur <= 0.5 {
        // 实心近似:偏移+扩散实心圆角矩形,带透明度(局部坐标 + transform)。
        let rect = mapper.scissor_of(sc, sx, sy, sw, sh);
        if rect.empty() {
            return;
        }
        let w = pixmap.width() as i64;
        let pixels = pixmap.pixels_mut();
        let a = (ca as f64 * item.opacity).round().clamp(0.0, 255.0) as u8;
        for py in rect.y0..rect.y1 {
            for px in rect.x0..rect.x1 {
                let (lx, ly) = mapper.local(px, py);
                if inside_rounded(lx, ly, sx, sy, sw, sh, &s_radius) {
                    blend_pixel(pixels, w, px, py, rgb[0], rgb[1], rgb[2], a);
                }
            }
        }
        return;
    }
    let sigma = blur / 2.0;
    let hw = half_width_for(sigma);
    let pad = hw as f64 * 3.0 + 2.0;
    let bounds = mapper.scissor_of(sc, sx - pad, sy - pad, sw + pad * 2.0, sh + pad * 2.0);
    if bounds.empty() {
        return;
    }
    let mw = (bounds.x1 - bounds.x0) as usize;
    let mh = (bounds.y1 - bounds.y0) as usize;
    if mw * mh > MAX_MASK_PIXELS {
        // 超大蒙版退化为实心近似(防内存/耗时失控)。
        draw_solid_rect(pixmap, &bounds, rgb, ca, item.opacity);
        return;
    }
    let mut mask = vec![0u8; mw * mh];
    for (i, cell) in mask.iter_mut().enumerate() {
        let px = bounds.x0 + (i % mw) as i64;
        let py = bounds.y0 + (i / mw) as i64;
        let (lx, ly) = mapper.local(px, py);
        if inside_rounded(lx, ly, sx, sy, sw, sh, &s_radius) {
            *cell = 255;
        }
    }
    blur_mask(&mut mask, mw, mh, sigma);
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    for (i, cell) in mask.iter().enumerate() {
        if *cell == 0 {
            continue;
        }
        let px = bounds.x0 + (i % mw) as i64;
        let py = bounds.y0 + (i / mw) as i64;
        let a = (*cell as f64 * (ca as f64 / 255.0) * item.opacity).round().clamp(0.0, 255.0) as u8;
        blend_pixel(pixels, w, px, py, rgb[0], rgb[1], rgb[2], a);
    }
}

/// 退化实心矩形(蒙版超限时)。
fn draw_solid_rect(pixmap: &mut Pixmap, bounds: &Scissor, rgb: [u8; 3], ca: u8, opacity: f64) {
    let (w, h) = (pixmap.width() as i64, pixmap.height() as i64);
    let pixels = pixmap.pixels_mut();
    let a = (ca as f64 * opacity).round().clamp(0.0, 255.0) as u8;
    for py in bounds.y0.max(0)..bounds.y1.min(h) {
        for px in bounds.x0.max(0)..bounds.x1.min(w) {
            blend_pixel(pixels, w, px, py, rgb[0], rgb[1], rgb[2], a);
        }
    }
}

/// 渐变参数 t(0..1,Pad 钳位):线性按梯度线投影,径向按归一距离。
fn gradient_t(spec: &GradientSpec, px: f64, py: f64, x: f64, y: f64, w: f64, h: f64) -> f64 {
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    match spec.kind {
        GradientKind::Linear { angle_deg } => {
            let rad = angle_deg.to_radians();
            // CSS:0deg 向上、顺时针 → 方向向量 (sin, -cos)。
            let (dx, dyy) = (rad.sin(), -rad.cos());
            // 梯度线长度:|w·sin| + |h·cos|。
            let len = (w * dx.abs() + h * dyy.abs()).max(1.0);
            let t = ((px - cx) * dx + (py - cy) * dyy) / len + 0.5;
            t.clamp(0.0, 1.0)
        }
        GradientKind::Radial { farthest_side } => {
            let (dx0, dx1) = (cx - x, x + w - cx);
            let (dy0, dy1) = (cy - y, y + h - cy);
            let r = if farthest_side {
                (dx0.max(dx1).hypot(dy0.max(dy1))).max(1.0)
            } else {
                let mut m = 0.0f64;
                for (ex, ey) in [(x, y), (x + w, y), (x, y + h), (x + w, y + h)] {
                    m = m.max((cx - ex).hypot(cy - ey));
                }
                m.max(1.0)
            };
            ((px - cx).hypot(py - cy) / r).clamp(0.0, 1.0)
        }
    }
}

/// 色标插值:sRGB 分量(含 alpha)线性插值;首尾 Pad。
fn gradient_color_at(spec: &GradientSpec, t: f64) -> ([u8; 3], u8) {
    let stops = &spec.stops;
    if t <= stops[0].pos {
        return stops[0].color;
    }
    if t >= stops[stops.len() - 1].pos {
        return stops[stops.len() - 1].color;
    }
    for i in 0..stops.len() - 1 {
        let (a, b) = (&stops[i], &stops[i + 1]);
        if t >= a.pos && t <= b.pos {
            let span = (b.pos - a.pos).max(1e-9);
            let k = ((t - a.pos) / span).clamp(0.0, 1.0);
            let lerp = |va: u8, vb: u8| (va as f64 * (1.0 - k) + vb as f64 * k).round() as u8;
            let (ca, aa) = a.color;
            let (cb, ab) = b.color;
            return (
                [
                    lerp(ca[0], cb[0]),
                    lerp(ca[1], cb[1]),
                    lerp(ca[2], cb[2]),
                ],
                lerp(aa, ab),
            );
        }
    }
    stops[stops.len() - 1].color
}

/// 文本绘制:字形 coverage 混合 + scissor + 组透明度 + transform;无字体画占位条。
///
/// `shadow_blur = Some(b)` 时走「字形覆盖蒙版 → 三趟盒式模糊 → 着色混合」
/// (text-shadow 模糊);`None` 逐字形直接混合。
#[allow(clippy::too_many_arguments)]
fn draw_text(
    font: &FontSet,
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    text: &str,
    x: f64,
    y: f64,
    size: f64,
    color: [u8; 3],
    bold: bool,
    opacity: f64,
    shadow_blur: Option<f64>,
) {
    let size_f = size as f32;
    let baseline = y as f32 + size_f;
    let mut pen_x = x as f32;
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    if !font.has_font() {
        // 无字体:占位色条,宽度反映文本长度(与旧行为一致)。
        let tw = (text.chars().count() as f32 * size_f * 0.5).min(w as f32);
        let a = (255.0 * opacity).round().clamp(0.0, 255.0) as u8;
        for py in sc.y0..sc.y1 {
            for px in sc.x0..sc.x1 {
                let (lx, ly) = mapper.local(px, py);
                if lx >= x
                    && lx <= x + tw as f64
                    && ly >= y + size * 0.4
                    && ly <= y + size * 0.4 + (size * 0.35).max(2.0)
                {
                    blend_pixel(pixels, w, px, py, color[0], color[1], color[2], a);
                }
            }
        }
        return;
    }
    if let Some(b) = shadow_blur {
        if b > 0.5 {
            draw_text_blurred(font, pixmap, sc, mapper, text, x, y, size, color, bold, opacity, b);
            return;
        }
    }
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            continue;
        }
        let Some((metrics, bitmap)) = font.rasterize_glyph(ch, size_f) else {
            // 缺字形(如未加载的图标字体码位):不画但按布局侧回退宽推进 pen,
            // 与 layout 的 char_width 同源 —— 否则其后文字相对布局左移,
            // 行内几何与像素整体错位(2026-10-30 京东图标字体空缺实测)。
            pen_x += font.char_width(ch, size_f as f64) as f32;
            continue;
        };
        if metrics.width > 0 && metrics.height > 0 {
            let gx = pen_x + metrics.xmin as f32;
            let gy = baseline - metrics.height as f32 - metrics.ymin as f32;
            blend_glyph_mapped(
                pixels, w, sc, mapper, gx, gy, metrics.width, metrics.height, &bitmap, color,
                opacity,
            );
            if bold {
                blend_glyph_mapped(
                    pixels, w, sc, mapper, gx + 1.0, gy, metrics.width, metrics.height, &bitmap,
                    color, opacity,
                );
            }
        }
        pen_x += metrics.advance_width;
    }
}

/// 字形混合(transform 版):画布像素反算局部坐标取 coverage × 组透明度。
#[allow(clippy::too_many_arguments)]
fn blend_glyph_mapped(
    pixels: &mut [PremultipliedColorU8],
    pw: i64,
    sc: &Scissor,
    mapper: &Mapper,
    x: f32,
    y: f32,
    w: usize,
    h: usize,
    bitmap: &[u8],
    color: [u8; 3],
    opacity: f64,
) {
    let bounds = mapper.scissor_of(sc, x as f64, y as f64, w as f64, h as f64);
    for py in bounds.y0..bounds.y1 {
        for px in bounds.x0..bounds.x1 {
            let (lx, ly) = mapper.local(px, py);
            let col = (lx - x as f64).round() as isize;
            let row = (ly - y as f64).round() as isize;
            if row < 0 || row >= h as isize || col < 0 || col >= w as isize {
                continue;
            }
            let coverage = bitmap[(row * w as isize + col) as usize];
            if coverage == 0 {
                continue;
            }
            let a = (coverage as f64 * opacity).round().clamp(0.0, 255.0) as u8;
            blend_pixel(pixels, pw, px, py, color[0], color[1], color[2], a);
        }
    }
}

/// `text-shadow` 模糊:字形覆盖蒙版 → 三趟盒式模糊 → 按阴影色着色混合。
#[allow(clippy::too_many_arguments)]
fn draw_text_blurred(
    font: &FontSet,
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    text: &str,
    x: f64,
    y: f64,
    size: f64,
    color: [u8; 3],
    bold: bool,
    opacity: f64,
    blur: f64,
) {
    let size_f = size as f32;
    let baseline = y as f32 + size_f;
    let tw = font.text_width(text, size);
    let bounds = mapper.scissor_of(sc, x, y, tw, size * 2.0);
    if bounds.empty() {
        return;
    }
    let mw = (bounds.x1 - bounds.x0) as usize;
    let mh = (bounds.y1 - bounds.y0) as usize;
    if mw == 0 || mh == 0 || mw * mh > MAX_MASK_PIXELS {
        return;
    }
    let mut mask = vec![0u8; mw * mh];
    let mut pen_x = x as f32;
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            continue;
        }
        let Some((metrics, bitmap)) = font.rasterize_glyph(ch, size_f) else {
            // 缺字形同样推进 pen(与主绘制路径一致),防阴影文字错位。
            pen_x += font.char_width(ch, size_f as f64) as f32;
            continue;
        };
        if metrics.width > 0 && metrics.height > 0 {
            let gx = pen_x + metrics.xmin as f32;
            let gy = baseline - metrics.height as f32 - metrics.ymin as f32;
            fill_glyph_mask(&mut mask, mw, &bounds, mapper, gx, gy, metrics.width, metrics.height, &bitmap);
            if bold {
                fill_glyph_mask(
                    &mut mask, mw, &bounds, mapper, gx + 1.0, gy, metrics.width,
                    metrics.height, &bitmap,
                );
            }
        }
        pen_x += metrics.advance_width;
    }
    blur_mask(&mut mask, mw, mh, blur / 2.0);
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    for (i, cov) in mask.iter().enumerate() {
        if *cov == 0 {
            continue;
        }
        let px = bounds.x0 + (i % mw) as i64;
        let py = bounds.y0 + (i / mw) as i64;
        let a = (*cov as f64 * opacity).round().clamp(0.0, 255.0) as u8;
        blend_pixel(pixels, w, px, py, color[0], color[1], color[2], a);
    }
}

/// 把单个字形的 coverage 写入蒙版(重叠取 max)。
#[allow(clippy::too_many_arguments)]
fn fill_glyph_mask(
    mask: &mut [u8],
    mw: usize,
    bounds: &Scissor,
    mapper: &Mapper,
    x: f32,
    y: f32,
    w: usize,
    h: usize,
    bitmap: &[u8],
) {
    let gb = mapper.scissor_of(bounds, x as f64, y as f64, w as f64, h as f64);
    for py in gb.y0..gb.y1 {
        for px in gb.x0..gb.x1 {
            let (lx, ly) = mapper.local(px, py);
            let col = (lx - x as f64).round() as isize;
            let row = (ly - y as f64).round() as isize;
            if row < 0 || row >= h as isize || col < 0 || col >= w as isize {
                continue;
            }
            let coverage = bitmap[(row * w as isize + col) as usize];
            if coverage == 0 {
                continue;
            }
            let idx = ((py - bounds.y0) * mw as i64 + (px - bounds.x0)) as usize;
            if coverage > mask[idx] {
                mask[idx] = coverage;
            }
        }
    }
}

// ============ 图片(沿用双线性采样,接入 scissor 与透明度) ============

/// 目标盒内的绘制矩形(偏移 + 尺寸),由 `object-fit` 决定。
pub(crate) fn fit_rect(fit: &str, nw: f64, nh: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
    let contain = |w: f64, h: f64| -> (f64, f64, f64, f64) {
        let s = (w / nw).min(h / nh);
        let dw = nw * s;
        let dh = nh * s;
        ((w - dw) / 2.0, (h - dh) / 2.0, dw, dh)
    };
    match fit {
        "contain" => contain(w, h),
        "cover" => {
            let s = (w / nw).max(h / nh);
            let dw = nw * s;
            let dh = nh * s;
            ((w - dw) / 2.0, (h - dh) / 2.0, dw, dh)
        }
        "none" => (0.0, 0.0, nw, nh),
        "scale-down" => {
            if nw <= w && nh <= h {
                (0.0, 0.0, nw, nh)
            } else {
                contain(w, h)
            }
        }
        _ => (0.0, 0.0, w, h),
    }
}

/// 双线性采样(源坐标为像素中心偏移前的连续坐标)。
fn sample_bilinear(img: &DecodedImage, x: f64, y: f64) -> [u8; 4] {
    let nw = img.width as f64;
    let nh = img.height as f64;
    let x = x.clamp(0.0, (nw - 1.0).max(0.0));
    let y = y.clamp(0.0, (nh - 1.0).max(0.0));
    let x0 = x.floor().min((nw - 1.0).max(0.0)) as u32;
    let y0 = y.floor().min((nh - 1.0).max(0.0)) as u32;
    let x1 = (x0 + 1).min(img.width.saturating_sub(1));
    let y1 = (y0 + 1).min(img.height.saturating_sub(1));
    let fx = x - x0 as f64;
    let fy = y - y0 as f64;
    let c00 = img.pixel(x0, y0);
    let c10 = img.pixel(x1, y0);
    let c01 = img.pixel(x0, y1);
    let c11 = img.pixel(x1, y1);
    let mut out = [0u8; 4];
    for ch in 0..4 {
        let top = c00[ch] as f64 * (1.0 - fx) + c10[ch] as f64 * fx;
        let bottom = c01[ch] as f64 * (1.0 - fx) + c11[ch] as f64 * fx;
        out[ch] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn draw_image(
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    img: &DecodedImage,
    item: &DisplayItem,
    fit: &str,
    off_x: f64,
    off_y: f64,
) {
    let nw = img.width as f64;
    let nh = img.height as f64;
    if nw <= 0.0 || nh <= 0.0 || item.w <= 0.0 || item.h <= 0.0 {
        return;
    }
    let (ox, oy, dw, dh) = fit_rect(fit, nw, nh, item.w, item.h);
    let dx = item.x + ox + off_x;
    let dy = item.y + oy + off_y;
    // 绘制区 = scissor ∩ 目标盒(目标盒即裁剪区,与 cover/none 语义一致)。
    let bounds = mapper.scissor_of(sc, item.x, item.y, item.w, item.h);
    if bounds.empty() {
        return;
    }
    let width = pixmap.width() as i64;
    let height = pixmap.height() as i64;
    let pixels = pixmap.pixels_mut();
    for py in bounds.y0.max(0)..bounds.y1.min(height) {
        for pxi in bounds.x0.max(0)..bounds.x1.min(width) {
            let (lx, ly) = mapper.local(pxi, py);
            // 只绘制实际图像矩形:`sample_bilinear` 对越界坐标做钳制,
            // 不跳过的话 `object-fit:contain/cover` 的留白区会被**边缘像素
            // 拉伸**满整盒(letterbox 失效,红图铺满 120×60)。
            if lx < dx || lx >= dx + dw || ly < dy || ly >= dy + dh {
                continue;
            }
            let u = (lx - dx) / dw;
            let v = (ly - dy) / dh;
            let c = sample_bilinear(img, u * nw - 0.5, v * nh - 0.5);
            let a = (c[3] as f64 * item.opacity).round().clamp(0.0, 255.0) as u8;
            if a == 0 {
                continue;
            }
            blend_pixel(pixels, width, pxi, py, c[0], c[1], c[2], a);
        }
    }
}

// ============ 第 48 轮:媒体/嵌入占位与文本装饰 ============

/// 媒体/嵌入空盒占位:与 canvas 同款淡底虚线,文案点明能力边界。
fn draw_media_frame_note(
    font: &FontSet,
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    item: &DisplayItem,
    pale: bool,
    tag: &str,
) {
    let (bx, by, bw, bh) = (item.x, item.y, item.w, item.h);
    if bw < 48.0 || bh < 28.0 {
        return;
    }
    let bounds = mapper.scissor_of(sc, bx, by, bw, bh);
    if bounds.empty() {
        return;
    }
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    let pixels = pixmap.pixels_mut();
    let inside = |x: f64, y: f64| x >= bx && x <= bx + bw && y >= by && y <= by + bh;
    if pale {
        for py in bounds.y0.max(0)..bounds.y1.min(ph) {
            for px in bounds.x0.max(0)..bounds.x1.min(pw) {
                let (lx, ly) = mapper.local(px, py);
                if inside(lx, ly) {
                    blend_pixel(pixels, pw, px, py, 236, 238, 242, 255);
                }
            }
        }
    }
    let dash = 8.0f64;
    let edge = [176u8, 182, 190];
    let mut px = bx;
    while px < bx + bw {
        let seg = dash.min(bx + bw - px);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, px, by, seg, 1.0, edge, inside);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, px, by + bh - 1.0, seg, 1.0, edge, inside);
        px += dash * 2.0;
    }
    let mut py = by;
    while py < by + bh {
        let seg = dash.min(by + bh - py);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, bx, py, 1.0, seg, edge, inside);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, bx + bw - 1.0, py, 1.0, seg, edge, inside);
        py += dash * 2.0;
    }
    let full = format!("{tag} 区域:内核未加载子文档/未解码媒体/无 SVG 光栅化,属能力边界");
    let short = format!("{tag} 区域:能力未实现");
    let label = if bw >= 520.0 { full } else { short };
    let size = 13.0f64;
    let est_w = label.chars().count() as f64 * size * 0.62;
    let tx = bx + ((bw - est_w) / 2.0).max(8.0);
    let ty = by + ((bh - size) / 2.0).max(4.0);
    draw_text(font, pixmap, &bounds, mapper, &label, tx, ty, size, [120, 128, 138], false, 1.0, None);
}

/// 文本装饰线:基线由 `item.y + size` 还原(与 `draw_text` 同口径)。
fn draw_text_decor(
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    item: &DisplayItem,
    kind: TextDecorKind,
    color: [u8; 3],
) {
    let size = item.h / 2.0;
    if size <= 0.0 {
        return;
    }
    let baseline = item.y + size;
    let tw = item.w;
    if tw <= 0.0 {
        return;
    }
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    let pixels = pixmap.pixels_mut();
    let a = (item.opacity * 255.0).round().clamp(0.0, 255.0) as u8;
    if a == 0 {
        return;
    }
    let mut line = |y: f64, dotted: bool| {
        let bounds = mapper.scissor_of(sc, item.x, y - 1.0, tw, 2.0);
        if bounds.empty() {
            return;
        }
        let inside = |x: f64, yy: f64| {
            x >= item.x && x <= item.x + tw && yy >= y - 1.0 && yy <= y + 1.0
        };
        if dotted {
            let mut x = item.x;
            while x < item.x + tw {
                let seg = 4.0f64.min(item.x + tw - x);
                fill_seg_mapped(pixels, pw, ph, &bounds, mapper, x, y, seg, 1.0, color, inside);
                x += 8.0;
            }
        } else {
            fill_seg_mapped(pixels, pw, ph, &bounds, mapper, item.x, y, tw, 1.0, color, inside);
        }
    };
    match kind {
        TextDecorKind::StrikeThrough => line(baseline - size * 0.35, false),
        TextDecorKind::Underline => line(baseline + size * 0.18, false),
        TextDecorKind::DottedUnderline => line(baseline + size * 0.18, true),
    }
}

// ============ canvas 占位说明块 ============

/// canvas 区域:淡底(可选) + 虚线边框 + 居中说明文字(与旧渲染一致,支持 transform)。
fn draw_canvas_note(
    font: &FontSet,
    pixmap: &mut Pixmap,
    sc: &Scissor,
    mapper: &Mapper,
    item: &DisplayItem,
    pale: bool,
) {
    let (bx, by, bw, bh) = (item.x, item.y, item.w, item.h);
    if bw < 48.0 || bh < 28.0 {
        return;
    }
    let bounds = mapper.scissor_of(sc, bx, by, bw, bh);
    if bounds.empty() {
        return;
    }
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    let pixels = pixmap.pixels_mut();
    let inside = |x: f64, y: f64| x >= bx && x <= bx + bw && y >= by && y <= by + bh;
    if pale {
        for py in bounds.y0.max(0)..bounds.y1.min(ph) {
            for px in bounds.x0.max(0)..bounds.x1.min(pw) {
                let (lx, ly) = mapper.local(px, py);
                if inside(lx, ly) {
                    blend_pixel(pixels, pw, px, py, 236, 238, 242, 255);
                }
            }
        }
    }
    // 虚线边框(灰,8px 段)。
    let dash = 8.0f64;
    let edge = [176u8, 182, 190];
    let mut px = bx;
    while px < bx + bw {
        let seg = dash.min(bx + bw - px);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, px, by, seg, 1.0, edge, inside);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, px, by + bh - 1.0, seg, 1.0, edge, inside);
        px += dash * 2.0;
    }
    let mut py = by;
    while py < by + bh {
        let seg = dash.min(by + bh - py);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, bx, py, 1.0, seg, edge, inside);
        fill_seg_mapped(pixels, pw, ph, &bounds, mapper, bx + bw - 1.0, py, 1.0, seg, edge, inside);
        py += dash * 2.0;
    }
    // 居中说明文字(宽度不足时只留短句)。
    let full = "canvas 区域:当前无像素,或使用了未实现的 2D 路径/WebGL 能力";
    let short = "canvas 区域:当前无像素或能力未实现";
    let label = if bw >= 420.0 { full } else { short };
    let size = 13.0f64;
    let est_w = label.chars().count() as f64 * size * 0.62;
    let tx = bx + ((bw - est_w) / 2.0).max(8.0);
    let ty = by + ((bh - size) / 2.0).max(4.0);
    draw_text(
        font,
        pixmap,
        &bounds,
        mapper,
        label,
        tx,
        ty,
        size,
        [120, 128, 138],
        false,
        1.0,
        None,
    );
}

/// 虚线小段填充(transform 版:画布像素反算局部坐标判定,越界由 bounds 与画布钳制)。
#[allow(clippy::too_many_arguments)]
fn fill_seg_mapped(
    pixels: &mut [PremultipliedColorU8],
    pw: i64,
    ph: i64,
    bounds: &Scissor,
    mapper: &Mapper,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    color: [u8; 3],
    inside: impl Fn(f64, f64) -> bool,
) {
    let x0 = x.floor() as i64;
    let x1 = (x + w).ceil() as i64;
    let y0 = y.floor() as i64;
    let y1 = (y + h).ceil() as i64;
    for py in y0.max(bounds.y0).max(0)..y1.min(bounds.y1).min(ph) {
        for px in x0.max(bounds.x0).max(0)..x1.min(bounds.x1).min(pw) {
            let (lx, ly) = mapper.local(px, py);
            if inside(lx, ly) {
                blend_pixel(pixels, pw, px, py, color[0], color[1], color[2], 255);
            }
        }
    }
}
