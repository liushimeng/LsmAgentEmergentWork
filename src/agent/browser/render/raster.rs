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
use super::super::style::{CornerRadius, GradientKind, GradientSpec};
use super::blur::{blur_mask, half_width_for, MAX_MASK_PIXELS};
use super::display::{BgFill, DisplayItem, DisplayKind};
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

/// 项级裁剪窗口:项自带 clip(页坐标)平移滚动量后与画布取交。
fn scissor_for(item: &DisplayItem, scroll_y: f64, pw: i64, ph: i64) -> Scissor {
    let mut sc = Scissor::full(pw, ph);
    if let Some((cx, cy, cw, ch)) = item.clip {
        sc = sc.intersect_rect(cx, cy - scroll_y, cw, ch);
    }
    sc
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
    scroll_y: f64,
    images: &ImageStore,
) {
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    for item in items {
        if item.opacity <= 0.0 {
            continue;
        }
        let top = item.y - scroll_y;
        // 便宜剔除:整项(含阴影扩张)在画布外。
        let (ex0, ey0, ex1, ey1) = match &item.kind {
            DisplayKind::Shadow { blur, spread, .. } => (
                item.x - blur - spread - 8.0,
                top - blur - spread - 8.0,
                item.x + item.w + blur + spread + 8.0,
                top + item.h + blur + spread + 8.0,
            ),
            DisplayKind::Text { size, .. } => (
                item.x,
                top,
                item.x + 4096.0,
                top + size * 2.5,
            ),
            _ => (item.x, top, item.x + item.w, top + item.h),
        };
        if ex1 < 0.0 || ex0 > pw as f64 || ey1 < 0.0 || ey0 > ph as f64 {
            continue;
        }
        let sc = scissor_for(item, scroll_y, pw, ph);
        if sc.empty() {
            continue;
        }
        match &item.kind {
            DisplayKind::Background { fill } => {
                draw_background(pixmap, &sc, item, top, fill);
            }
            DisplayKind::Border { widths, colors } => {
                draw_border(pixmap, &sc, item, top, widths, colors);
            }
            DisplayKind::Shadow {
                dx,
                dy,
                blur,
                spread,
                color,
            } => {
                draw_shadow(pixmap, &sc, item, top, *dx, *dy, *blur, *spread, *color);
            }
            DisplayKind::Image {
                src,
                fit,
                off_x,
                off_y,
            } => {
                if let Some(img) = images.get(src) {
                    draw_image(pixmap, &sc, img, item, top, fit, *off_x, *off_y);
                }
            }
            DisplayKind::Text {
                text,
                size,
                color,
                bold,
            } => {
                draw_text(font, pixmap, &sc, text, item.x, top, *size, *color, *bold, item.opacity);
            }
            DisplayKind::CanvasNote { pale } => {
                draw_canvas_note(font, pixmap, &sc, item, top, *pale);
            }
        }
    }
}

/// 背景填充:纯色圆角矩形或渐变(渐变按像素求值,支持圆角与裁剪)。
fn draw_background(
    pixmap: &mut Pixmap,
    sc: &Scissor,
    item: &DisplayItem,
    top: f64,
    fill: &BgFill,
) {
    let rect = sc.intersect_rect(item.x, top, item.w, item.h);
    if rect.empty() {
        return;
    }
    let radius = resolve_radius(&item.radius, item.w, item.h);
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    match fill {
        BgFill::Solid(rgb) => {
            let a = (item.opacity * 255.0).round().clamp(0.0, 255.0) as u8;
            for py in rect.y0..rect.y1 {
                for px in rect.x0..rect.x1 {
                    if inside_rounded(
                        px as f64 + 0.5,
                        py as f64 + 0.5,
                        item.x,
                        top,
                        item.w,
                        item.h,
                        &radius,
                    ) {
                        blend_pixel(pixels, w, px, py, rgb[0], rgb[1], rgb[2], a);
                    }
                }
            }
        }
        BgFill::Gradient(spec) => {
            for py in rect.y0..rect.y1 {
                for px in rect.x0..rect.x1 {
                    let cx = px as f64 + 0.5;
                    let cy = py as f64 + 0.5;
                    if !inside_rounded(cx, cy, item.x, top, item.w, item.h, &radius) {
                        continue;
                    }
                    let t = gradient_t(spec, cx, cy, item.x, top, item.w, item.h);
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
    item: &DisplayItem,
    top: f64,
    widths: &[f64; 4],
    colors: &[([u8; 3], u8); 4],
) {
    let outer_r = resolve_radius(&item.radius, item.w, item.h);
    let rect = sc.intersect_rect(item.x, top, item.w, item.h);
    if rect.empty() {
        return;
    }
    let w = pixmap.width() as i64;
    let pixels = pixmap.pixels_mut();
    // 每边的内侧矩形与内侧半径(仅该边内缩)。
    let inners: [(f64, f64, f64, f64, [f64; 4]); 4] = [
        (
            item.x,
            top + widths[0],
            item.w,
            (item.h - widths[0]).max(0.0),
            shrink(outer_r, widths[0]),
        ),
        (
            item.x + widths[1],
            top,
            (item.w - widths[1]).max(0.0),
            item.h,
            shrink(outer_r, widths[1]),
        ),
        (
            item.x,
            top,
            item.w,
            (item.h - widths[2]).max(0.0),
            shrink(outer_r, widths[2]),
        ),
        (
            item.x + widths[3],
            top,
            (item.w - widths[3]).max(0.0),
            item.h,
            shrink(outer_r, widths[3]),
        ),
    ];
    for py in rect.y0..rect.y1 {
        for px in rect.x0..rect.x1 {
            let cx = px as f64 + 0.5;
            let cy = py as f64 + 0.5;
            if !inside_rounded(cx, cy, item.x, top, item.w, item.h, &outer_r) {
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
    item: &DisplayItem,
    top: f64,
    dx: f64,
    dy: f64,
    blur: f64,
    spread: f64,
    color: ([u8; 3], u8),
) {
    let sw = item.w + spread * 2.0;
    let sh = item.h + spread * 2.0;
    let sx = item.x + dx - spread;
    let sy = top + dy - spread;
    let outer_r = resolve_radius(&item.radius, item.w, item.h);
    let s_radius = [
        (outer_r[0] + spread).max(0.0),
        (outer_r[1] + spread).max(0.0),
        (outer_r[2] + spread).max(0.0),
        (outer_r[3] + spread).max(0.0),
    ];
    let (rgb, ca) = color;
    if blur <= 0.5 {
        // 实心近似:与旧渲染器一致(偏移+扩散实心矩形),带圆角与透明度。
        let solid = DisplayItem {
            kind: DisplayKind::Background {
                fill: BgFill::Solid(rgb),
            },
            x: sx,
            y: sy,
            w: sw,
            h: sh,
            radius: [
                CornerRadius::Px(s_radius[0]),
                CornerRadius::Px(s_radius[1]),
                CornerRadius::Px(s_radius[2]),
                CornerRadius::Px(s_radius[3]),
            ],
            z: 0,
            seq: 0,
            opacity: item.opacity * ca as f64 / 255.0,
            clip: None,
        };
        draw_background(pixmap, sc, &solid, sy, &BgFill::Solid(rgb));
        return;
    }
    let sigma = blur / 2.0;
    let hw = half_width_for(sigma);
    let pad = hw as f64 * 3.0 + 2.0;
    let bounds = sc.intersect_rect(sx - pad, sy - pad, sw + pad * 2.0, sh + pad * 2.0);
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
        let mx = bounds.x0 as f64 + (i % mw) as f64 + 0.5;
        let my = bounds.y0 as f64 + (i / mw) as f64 + 0.5;
        if inside_rounded(mx, my, sx, sy, sw, sh, &s_radius) {
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

/// 文本绘制:字形 coverage 混合 + scissor + 组透明度;无字体画占位条。
fn draw_text(
    font: &FontSet,
    pixmap: &mut Pixmap,
    sc: &Scissor,
    text: &str,
    x: f64,
    top: f64,
    size: f64,
    color: [u8; 3],
    bold: bool,
    opacity: f64,
) {
    let size_f = size as f32;
    let baseline = top as f32 + size_f;
    let mut pen_x = x as f32;
    let (w, h) = (pixmap.width() as i64, pixmap.height() as i64);
    let pixels = pixmap.pixels_mut();
    if !font.has_font() {
        // 无字体:占位色条,宽度反映文本长度(与旧行为一致)。
        let tw = (text.chars().count() as f32 * size_f * 0.5).min(w as f32);
        let a = (255.0 * opacity).round().clamp(0.0, 255.0) as u8;
        for py in (sc.y0)..(sc.y1) {
            for px in (sc.x0)..(sc.x1) {
                let cx = px as f64 + 0.5;
                let cy = py as f64 + 0.5;
                if cx >= x
                    && cx <= x + tw as f64
                    && cy >= top + size * 0.4
                    && cy <= top + size * 0.4 + (size * 0.35).max(2.0)
                {
                    blend_pixel(pixels, w, px, py, color[0], color[1], color[2], a);
                }
            }
        }
        return;
    }
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            continue;
        }
        let Some((metrics, bitmap)) = font.rasterize_glyph(ch, size_f) else {
            continue;
        };
        if metrics.width > 0 && metrics.height > 0 {
            let gx = pen_x + metrics.xmin as f32;
            let gy = baseline - metrics.height as f32 - metrics.ymin as f32;
            blend_glyph_scissored(
                pixels, w, h, sc, gx, gy, metrics.width, metrics.height, &bitmap, color, opacity,
            );
            if bold {
                blend_glyph_scissored(
                    pixels, w, h, sc, gx + 1.0, gy, metrics.width, metrics.height, &bitmap,
                    color, opacity,
                );
            }
        }
        pen_x += metrics.advance_width;
    }
}

/// 字形混合(scissor 版):coverage × 组透明度。
#[allow(clippy::too_many_arguments)]
fn blend_glyph_scissored(
    pixels: &mut [PremultipliedColorU8],
    pw: i64,
    ph: i64,
    sc: &Scissor,
    x: f32,
    y: f32,
    w: usize,
    h: usize,
    bitmap: &[u8],
    color: [u8; 3],
    opacity: f64,
) {
    let base_x = x.round() as i64;
    let base_y = y.round() as i64;
    for row in 0..h as i64 {
        let py = base_y + row;
        if py < sc.y0 || py >= sc.y1 || py < 0 || py >= ph {
            continue;
        }
        for col in 0..w as i64 {
            let px = base_x + col;
            if px < sc.x0 || px >= sc.x1 || px < 0 || px >= pw {
                continue;
            }
            let coverage = bitmap[(row * w as i64 + col) as usize];
            if coverage == 0 {
                continue;
            }
            let a = (coverage as f64 * opacity).round().clamp(0.0, 255.0) as u8;
            blend_pixel(pixels, pw, px, py, color[0], color[1], color[2], a);
        }
    }
}

// ============ 图片(沿用双线性采样,接入 scissor 与透明度) ============

/// 目标盒内的绘制矩形(偏移 + 尺寸),由 `object-fit` 决定。
fn fit_rect(fit: &str, nw: f64, nh: f64, w: f64, h: f64) -> (f64, f64, f64, f64) {
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
    img: &DecodedImage,
    item: &DisplayItem,
    top: f64,
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
    let dy = top + oy + off_y;
    // 绘制区 = scissor ∩ 目标盒(目标盒即裁剪区,与 cover/none 语义一致)。
    let bounds = sc.intersect_rect(item.x, top, item.w, item.h);
    if bounds.empty() {
        return;
    }
    let width = pixmap.width() as i64;
    let height = pixmap.height() as i64;
    let pixels = pixmap.pixels_mut();
    for py in bounds.y0.max(0)..bounds.y1.min(height) {
        for pxi in bounds.x0.max(0)..bounds.x1.min(width) {
            let u = (pxi as f64 + 0.5 - dx) / dw;
            let v = (py as f64 + 0.5 - dy) / dh;
            let c = sample_bilinear(img, u * nw - 0.5, v * nh - 0.5);
            let a = (c[3] as f64 * item.opacity).round().clamp(0.0, 255.0) as u8;
            if a == 0 {
                continue;
            }
            blend_pixel(pixels, width, pxi, py, c[0], c[1], c[2], a);
        }
    }
}

// ============ canvas 占位说明块 ============

/// canvas 区域:淡底(可选) + 虚线边框 + 居中说明文字(与旧渲染一致)。
fn draw_canvas_note(
    font: &FontSet,
    pixmap: &mut Pixmap,
    sc: &Scissor,
    item: &DisplayItem,
    top: f64,
    pale: bool,
) {
    let (bx, by, bw, bh) = (item.x, top, item.w, item.h);
    if bw < 48.0 || bh < 28.0 {
        return;
    }
    let bounds = sc.intersect_rect(bx, by, bw, bh);
    if bounds.empty() {
        return;
    }
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    let pixels = pixmap.pixels_mut();
    let inside = |x: f64, y: f64| {
        x >= bx && x <= bx + bw && y >= by && y <= by + bh
    };
    if pale {
        for py in bounds.y0.max(0)..bounds.y1.min(ph) {
            for px in bounds.x0.max(0)..bounds.x1.min(pw) {
                blend_pixel(pixels, pw, px, py, 236, 238, 242, 255);
            }
        }
    }
    // 虚线边框(灰,8px 段)。
    let dash = 8.0f64;
    let edge = [176u8, 182, 190];
    let mut px = bx;
    while px < bx + bw {
        let seg = dash.min(bx + bw - px);
        fill_seg(pixels, pw, ph, &bounds, px, by, seg, 1.0, edge, inside);
        fill_seg(pixels, pw, ph, &bounds, px, by + bh - 1.0, seg, 1.0, edge, inside);
        px += dash * 2.0;
    }
    let mut py = by;
    while py < by + bh {
        let seg = dash.min(by + bh - py);
        fill_seg(pixels, pw, ph, &bounds, bx, py, 1.0, seg, edge, inside);
        fill_seg(pixels, pw, ph, &bounds, bx + bw - 1.0, py, 1.0, seg, edge, inside);
        py += dash * 2.0;
    }
    // 居中说明文字(宽度不足时只留短句)。
    let full = "canvas 区域:内核未实现 2D/WebGL 像素渲染(此区域无内容)";
    let short = "canvas 区域:内核未实现像素渲染";
    let label = if bw >= 420.0 { full } else { short };
    let size = 13.0f64;
    let est_w = label.chars().count() as f64 * size * 0.62;
    let tx = bx + ((bw - est_w) / 2.0).max(8.0);
    let ty = by + ((bh - size) / 2.0).max(4.0);
    draw_text(
        font,
        pixmap,
        &bounds,
        label,
        tx,
        ty,
        size,
        [120, 128, 138],
        false,
        1.0,
    );
}

/// 虚线小段填充(段内逐像素,越界由 bounds 与画布共同钳制)。
#[allow(clippy::too_many_arguments)]
fn fill_seg(
    pixels: &mut [PremultipliedColorU8],
    pw: i64,
    ph: i64,
    bounds: &Scissor,
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
            if inside(px as f64 + 0.5, py as f64 + 0.5) {
                blend_pixel(pixels, pw, px, py, color[0], color[1], color[2], 255);
            }
        }
    }
}
