//! 软件渲染:把布局结果绘制成 PNG。
//!
//! 使用 `fontdue` 栅格化字形、`tiny-skia` 承载像素与 PNG 编码。没有可用字体时
//! 退化为色块/占位线,仍会输出可打开的 PNG(响应附 `render_note`)。

use std::path::Path;

use fontdue::{Font, FontSettings};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

use super::images::{DecodedImage, ImageStore};
use super::layout::{Layout, TextLine};

pub struct FontSet {
    font: Option<Font>,
    pub path: String,
}

impl FontSet {
    pub fn load() -> Self {
        for path in candidate_fonts() {
            if let Ok(bytes) = std::fs::read(&path) {
                if let Some(font) = Font::from_bytes(bytes, FontSettings::default()).ok() {
                    return Self {
                        font: Some(font),
                        path,
                    };
                }
            }
        }
        Self::fallback()
    }

    pub fn fallback() -> Self {
        Self {
            font: None,
            path: String::new(),
        }
    }

    pub fn has_font(&self) -> bool {
        self.font.is_some()
    }

    pub fn char_width(&self, ch: char, size: f64) -> f64 {
        if let Some(font) = &self.font {
            let m = font.metrics(ch, size as f32);
            if m.advance_width > 0.0 {
                return m.advance_width as f64;
            }
        }
        fallback_char_width(ch, size)
    }

    pub fn text_width(&self, text: &str, size: f64) -> f64 {
        text.chars().map(|c| self.char_width(c, size)).sum()
    }

    /// 渲染一页。`scroll_y` 为垂直滚动偏移;`full_page=true` 时高度取整页。
    ///
    /// `images` 为已解码图片缓存(阶段三第三步);缺失的图片跳过绘制。
    pub fn render(
        &self,
        layout: &Layout,
        viewport_w: u32,
        viewport_h: u32,
        scroll_y: f64,
        full_page: bool,
        images: &ImageStore,
    ) -> Vec<u8> {
        self.render_pixmap(layout, viewport_w, viewport_h, scroll_y, full_page, images)
            .map(|p| p.encode_png().unwrap_or_default())
            .unwrap_or_default()
    }

    /// 渲染一页为 pixmap（阶段四第二步：供 clip 区域截图裁剪）。
    ///
    /// 所有 `fill_rect` 输入先整像素对齐(`fill_snap`):亚像素定位的细条带
    /// (如 1px 边框落在 x=91.476)经 tiny-skia 抗锯齿 FDot16 取整后宽度可为 0,
    /// debug 构建下触发 hairline_aa 的 `debug_assert!` 断言(release 则静默丢绘)。
    pub fn render_pixmap(
        &self,
        layout: &Layout,
        viewport_w: u32,
        viewport_h: u32,
        scroll_y: f64,
        full_page: bool,
        images: &ImageStore,
    ) -> Option<Pixmap> {
        let width = viewport_w.clamp(64, 7680);
        let mut height = if full_page {
            (layout.page_height().ceil() as u32).clamp(64, 20_000)
        } else {
            viewport_h.clamp(64, 4320)
        };
        // 像素总量上限 40M,防止超长页面把进程内存打爆。
        const MAX_PIXELS: u32 = 40_000_000;
        if width.saturating_mul(height) > MAX_PIXELS {
            height = (MAX_PIXELS / width).max(64);
        }
        let mut pixmap = Pixmap::new(width, height)?;
        pixmap.fill(Color::WHITE);

        // 外阴影绘制在元素背景之下(blur 不实现,按偏移+扩散实心矩形近似)。
        for s in &layout.shadows {
            if !s.visible {
                continue;
            }
            let rect = tiny_skia::Rect::from_xywh(
                fill_snap(s.x + s.dx - s.spread),
                fill_snap(s.y + s.dy - s.spread - scroll_y),
                fill_snap((s.w + s.spread * 2.0).max(1.0)),
                fill_snap((s.h + s.spread * 2.0).max(1.0)),
            );
            if let Some(rect) = rect {
                let mut paint = tiny_skia::Paint::default();
                paint.set_color_rgba8(s.color.0[0], s.color.0[1], s.color.0[2], s.color.1);
                pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
            }
        }

        for b in &layout.boxes {
            if !b.visible {
                continue;
            }
            if let Some(bg) = b.background {
                if let Some(rect) = tiny_skia::Rect::from_xywh(
                    fill_snap(b.x),
                    fill_snap(b.y - scroll_y),
                    fill_snap(b.w.max(1.0)),
                    fill_snap(b.h.max(1.0)),
                ) {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(bg[0], bg[1], bg[2], 255);
                    pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
                }
            }
        }

        // 边框绘制在背景之上、文字之下:四边实心矩形(alpha 含 transparent)。
        for bd in &layout.borders {
            if !bd.visible {
                continue;
            }
            let (x, y, w, h) = (bd.x, bd.y - scroll_y, bd.w, bd.h);
            let strips: [(f64, f64, f64, f64); 4] = [
                (x, y, w, bd.widths[0]),                    // top
                (x + w - bd.widths[1], y, bd.widths[1], h), // right
                (x, y + h - bd.widths[2], w, bd.widths[2]), // bottom
                (x, y, bd.widths[3], h),                    // left
            ];
            for (i, (sx, sy, sw, sh)) in strips.iter().enumerate() {
                if *sw <= 0.0 || *sh <= 0.0 {
                    continue;
                }
                let c = bd.colors[i];
                if let Some(rect) =
                    tiny_skia::Rect::from_xywh(fill_snap(*sx), fill_snap(*sy), fill_snap(*sw), fill_snap(*sh))
                {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(c.0[0], c.0[1], c.0[2], c.1);
                    pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
                }
            }
        }

        // 阶段三第三步:图像(背景图与内容图)先于文字绘制。
        for paint in &layout.paints {
            if !paint.visible {
                continue;
            }
            let Some(img) = images.get(&paint.src) else {
                continue;
            };
            draw_image(&mut pixmap, img, paint.x, paint.y, paint.w, paint.h, &paint.fit, scroll_y);
        }

        for line in &layout.lines {
            if !line.visible {
                continue;
            }
            let y = line.y - scroll_y;
            if y + line.size * 2.0 < 0.0 || y > height as f64 {
                continue;
            }
            self.draw_line(&mut pixmap, line, y);
        }
        Some(pixmap)
    }

    fn draw_line(&self, pixmap: &mut Pixmap, line: &TextLine, top: f64) {
        let size = line.size as f32;
        let baseline = top as f32 + size;
        let mut pen_x = line.x as f32;
        let Some(font) = &self.font else {
            // 无字体:画一条占位线,宽度反映文本长度。
            let w = (line.text.chars().count() as f32 * size * 0.5).min(pixmap.width() as f32);
            fill_rect(
                pixmap,
                pen_x,
                top as f32 + size * 0.4,
                w,
                (size * 0.35).max(2.0),
                line.color,
            );
            return;
        };
        for ch in line.text.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            let (metrics, bitmap) = font.rasterize(ch, size);
            if metrics.width > 0 && metrics.height > 0 {
                let gx = pen_x + metrics.xmin as f32;
                let gy = baseline - metrics.height as f32 - metrics.ymin as f32;
                blend_glyph(pixmap, gx, gy, metrics.width, metrics.height, &bitmap, line.color);
                if line.bold {
                    blend_glyph(
                        pixmap,
                        gx + 1.0,
                        gy,
                        metrics.width,
                        metrics.height,
                        &bitmap,
                        line.color,
                    );
                }
            }
            pen_x += metrics.advance_width;
        }
    }
}

/// 目标盒内的绘制矩形(偏移 + 尺寸),由 `object-fit` 决定。
///
/// `fill` 拉伸铺满;`contain` 等比内缩居中;`cover` 等比放大后由目标盒裁剪;
/// `none` 按原始尺寸左上对齐;`scale-down` 取 `none` 与 `contain` 的较小者。
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

/// 绘制一张图片(阶段三第三步)。
///
/// `x/y` 为页坐标,`w/h` 为目标盒(内容盒)尺寸;绘制矩形由 `fit` 决定,
/// 超出目标盒的部分被裁剪(与 `cover` / `none` 语义一致)。
fn draw_image(
    pixmap: &mut Pixmap,
    img: &DecodedImage,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    fit: &str,
    scroll_y: f64,
) {
    let nw = img.width as f64;
    let nh = img.height as f64;
    if nw <= 0.0 || nh <= 0.0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let top = y - scroll_y;
    let (ox, oy, dw, dh) = fit_rect(fit, nw, nh, w, h);
    let dx = x + ox;
    let dy = top + oy;
    let pw = pixmap.width() as f64;
    let ph = pixmap.height() as f64;
    // 绘制区与目标盒求交(目标盒即裁剪区)。
    let px0 = dx.max(x).max(0.0);
    let py0 = dy.max(top).max(0.0);
    let px1 = (dx + dw).min(x + w).min(pw);
    let py1 = (dy + dh).min(top + h).min(ph);
    if px1 <= px0 || py1 <= py0 {
        return;
    }
    let width = pixmap.width() as i64;
    let height = pixmap.height() as i64;
    let pixels = pixmap.pixels_mut();
    for py in py0.floor() as i64..py1.ceil() as i64 {
        if py < 0 || py >= height {
            continue;
        }
        for pxi in px0.floor() as i64..px1.ceil() as i64 {
            if pxi < 0 || pxi >= width {
                continue;
            }
            let u = (pxi as f64 + 0.5 - dx) / dw;
            let v = (py as f64 + 0.5 - dy) / dh;
            let c = sample_bilinear(img, u * nw - 0.5, v * nh - 0.5);
            let a = c[3] as u32;
            if a == 0 {
                continue;
            }
            let idx = (py * width + pxi) as usize;
            let old = pixels[idx].demultiply();
            let ia = 255 - a;
            let r = ((c[0] as u32 * a + old.red() as u32 * ia) / 255) as u8;
            let g = ((c[1] as u32 * a + old.green() as u32 * ia) / 255) as u8;
            let b = ((c[2] as u32 * a + old.blue() as u32 * ia) / 255) as u8;
            if let Some(px) = PremultipliedColorU8::from_rgba(r, g, b, 255) {
                pixels[idx] = px;
            }
        }
    }
}

fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: [u8; 3]) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)) {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], 255);
        pixmap.fill_rect(rect, &paint, tiny_skia::Transform::identity(), None);
    }
}

fn blend_glyph(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: usize,
    h: usize,
    bitmap: &[u8],
    color: [u8; 3],
) {
    let pw = pixmap.width() as i64;
    let ph = pixmap.height() as i64;
    let base_x = x.round() as i64;
    let base_y = y.round() as i64;
    let pixels = pixmap.pixels_mut();
    for row in 0..h {
        let py = base_y + row as i64;
        if py < 0 || py >= ph {
            continue;
        }
        for col in 0..w {
            let px = base_x + col as i64;
            if px < 0 || px >= pw {
                continue;
            }
            let coverage = bitmap[row * w + col];
            if coverage == 0 {
                continue;
            }
            let idx = (py * pw + px) as usize;
            let old = pixels[idx].demultiply();
            let a = coverage as u32;
            let ia = 255 - a;
            let r = ((color[0] as u32 * a + old.red() as u32 * ia) / 255) as u8;
            let g = ((color[1] as u32 * a + old.green() as u32 * ia) / 255) as u8;
            let b = ((color[2] as u32 * a + old.blue() as u32 * ia) / 255) as u8;
            if let Some(px) = PremultipliedColorU8::from_rgba(r, g, b, 255) {
                pixels[idx] = px;
            }
        }
    }
}

fn fallback_char_width(ch: char, size: f64) -> f64 {
    if ch.is_ascii_whitespace() {
        size * 0.28
    } else if ch.is_ascii() {
        size * 0.55
    } else {
        size
    }
}

fn candidate_fonts() -> Vec<String> {
    let mut out = Vec::new();
    match std::env::consts::OS {
        "macos" => {
            out.push("/System/Library/Fonts/Supplemental/Arial Unicode.ttf".into());
            out.push("/System/Library/Fonts/Supplemental/Arial.ttf".into());
            out.push("/System/Library/Fonts/Geneva.ttf".into());
            out.push("/Library/Fonts/Arial Unicode.ttf".into());
        }
        "windows" => {
            out.push("C:\\Windows\\Fonts\\msyh.ttc".into());
            out.push("C:\\Windows\\Fonts\\simsun.ttc".into());
            out.push("C:\\Windows\\Fonts\\segoeui.ttf".into());
            out.push("C:\\Windows\\Fonts\\arial.ttf".into());
        }
        _ => {
            out.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
            out.push("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf".into());
            out.push("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into());
            out.push("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf".into());
        }
    }
    out
}

/// fill_rect 输入整像素对齐:f64 坐标四舍五入到整数像素。
/// 亚像素细条带(1px 边框落在 x=91.476 之类)经 tiny-skia FDot16 抗锯齿取整后
/// 宽/高可为 0,debug 构建触发 hairline_aa 断言、release 静默丢绘;对齐后规避。
fn fill_snap(v: f64) -> f32 {
    v.round() as f32
}

/// 把 `data:`/绝对/相对路径的图片读取为 PNG/JPEG 原始字节(阶段一只用于下载与展示)。
pub fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    )
}

/// 裁剪 pixmap 子区域并编码 PNG（阶段四第二步：clip 区域截图）。
/// `x/y/w/h` 为源图像素坐标（浮点，内部取整并 clamp 到边界）。
/// 区域完全越界时返回 None。
pub fn crop_pixmap(src: &Pixmap, x: f64, y: f64, w: f64, h: f64) -> Option<Vec<u8>> {
    let sw = src.width() as i64;
    let sh = src.height() as i64;
    let x0 = x.round() as i64;
    let y0 = y.round() as i64;
    let x1 = (x + w).round() as i64;
    let y1 = (y + h).round() as i64;
    let cx0 = x0.max(0).min(sw);
    let cy0 = y0.max(0).min(sh);
    let cx1 = x1.max(0).min(sw);
    let cy1 = y1.max(0).min(sh);
    if cx1 <= cx0 || cy1 <= cy0 {
        return None;
    }
    let cw = (cx1 - cx0) as u32;
    let ch = (cy1 - cy0) as u32;
    let mut dst = Pixmap::new(cw, ch)?;
    let src_pixels = src.pixels();
    let dst_pixels = dst.pixels_mut();
    for row in 0..ch as i64 {
        let srow = (cy0 + row) as usize;
        let soffset = srow * sw as usize + cx0 as usize;
        let drow = row as usize;
        let doffset = drow * cw as usize;
        dst_pixels[doffset..doffset + cw as usize]
            .copy_from_slice(&src_pixels[soffset..soffset + cw as usize]);
    }
    dst.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::browser::document::Document;
    use crate::agent::browser::layout;

    #[test]
    fn 渲染输出_png_签名() {
        let doc = Document::parse("<body><h1>Hello</h1><p>World</p></body>", "https://a.test/");
        let font = FontSet::fallback();
        let l = layout::compute(&doc, 640.0, &font);
        let png = font.render(&l, 640, 480, 0.0, false, &ImageStore::new());
        assert!(png.len() > 8);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn 渲染绘制图片() {
        let font = FontSet::fallback();
        let layout = Layout {
            paints: vec![layout::PaintBox {
                src: "u".to_string(),
                x: 10.0,
                y: 10.0,
                w: 20.0,
                h: 20.0,
                fit: "fill".to_string(),
                kind: layout::PaintKind::Content,
                visible: true,
                z_index: 0,
            }],
            width: 100.0,
            height: 100.0,
            ..Default::default()
        };
        let mut store = ImageStore::new();
        store.insert(
            "u".to_string(),
            std::sync::Arc::new(DecodedImage {
                width: 4,
                height: 4,
                rgba: (0..16).flat_map(|_| [255u8, 0, 0, 255]).collect(),
            }),
        );
        let png = font.render(&layout, 100, 100, 0.0, false, &store);
        let decoded = image::load_from_memory(&png).unwrap().to_rgba8();
        let inside = decoded.get_pixel(20, 20).0;
        assert_eq!(&inside[..3], &[255, 0, 0], "图片区域应为红色");
        let outside = decoded.get_pixel(5, 5).0;
        assert_eq!(&outside[..3], &[255, 255, 255], "图片外应为白底");
    }

    #[test]
    fn 字体度量回退可用() {
        let f = FontSet::fallback();
        assert!(f.char_width('中', 16.0) >= 16.0);
        assert!(f.char_width('a', 16.0) > 0.0);
    }

    #[test]
    fn crop_pixmap_裁剪子区域() {
        // 4x2 像素：左半红右半蓝。
        let mut src = Pixmap::new(4, 2).unwrap();
        let red = PremultipliedColorU8::from_rgba(255, 0, 0, 255).unwrap();
        let blue = PremultipliedColorU8::from_rgba(0, 0, 255, 255).unwrap();
        let pixels = src.pixels_mut();
        for row in 0..2 {
            for col in 0..4 {
                pixels[row * 4 + col] = if col < 2 { red } else { blue };
            }
        }
        // 裁剪右半（x=2, w=2）。
        let cropped = crop_pixmap(&src, 2.0, 0.0, 2.0, 2.0).unwrap();
        let decoded = image::load_from_memory(&cropped).unwrap().to_rgba8();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
        assert_eq!(&decoded.get_pixel(0, 0).0[..3], &[0, 0, 255]);
        assert_eq!(&decoded.get_pixel(1, 1).0[..3], &[0, 0, 255]);
    }

    #[test]
    fn crop_pixmap_越界区域返回_none() {
        let src = Pixmap::new(4, 2).unwrap();
        assert!(crop_pixmap(&src, 10.0, 10.0, 2.0, 2.0).is_none());
        assert!(crop_pixmap(&src, -5.0, 0.0, 2.0, 2.0).is_none());
    }

    #[test]
    fn render_pixmap_与_render_一致() {
        let doc = Document::parse("<body><h1>Hi</h1></body>", "https://a.test/");
        let font = FontSet::fallback();
        let l = layout::compute(&doc, 320.0, &font);
        let pm = font
            .render_pixmap(&l, 320, 240, 0.0, false, &ImageStore::new())
            .unwrap();
        let png = pm.encode_png().unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(pm.width(), 320);
        assert_eq!(pm.height(), 240);
    }
}

#[cfg(test)]
mod border_shadow_tests {
    use super::super::document::Document;
    use super::super::layout;
    use super::FontSet;

    fn pixel(pm: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
        let w = pm.width();
        let idx = ((y * w + x) * 4) as usize;
        let d = pm.data();
        [d[idx], d[idx + 1], d[idx + 2], d[idx + 3]]
    }

    #[test]
    fn 边框绘制在盒沿() {
        // 300×200 白底盒,2px 红边框:盒内白、盒沿红。
        let doc = Document::parse(
            "<body><div style='width:300px;height:200px;border:2px solid #ff0000;background:#ffffff'></div></body>",
            "https://a.test/",
        );
        let font = FontSet::fallback();
        let l = layout::compute(&doc, 800.0, &font);
        assert_eq!(l.borders.len(), 1, "应产出一条边框绘制指令");
        let pm = font.render_pixmap(&l, 800, 600, 0.0, false, &super::ImageStore::new()).unwrap();
        // tiny_skia 数据字节序为 RGBA(实证);盒在 body margin 8 处,2px 红边框。
        let edge = pixel(&pm, 8, 8);
        assert!(edge[0] > 200 && edge[1] < 50 && edge[2] < 50, "左上角应为红:{edge:?}");
        let inside = pixel(&pm, 30, 30);
        assert!(inside[0] > 200 && inside[1] > 200, "盒内应为白:{inside:?}");
    }

    #[test]
    fn 外阴影绘制在背景之下() {
        // 100×100 白盒 + 1px 2px 纯黑阴影:盒外偏移处应出现黑色像素。
        let doc = Document::parse(
            "<body><div style='width:100px;height:100px;background:#ffffff;box-shadow:1px 2px 0 #000000'></div></body>",
            "https://a.test/",
        );
        let font = FontSet::fallback();
        let l = layout::compute(&doc, 800.0, &font);
        assert_eq!(l.shadows.len(), 1, "应产出一条阴影绘制指令");
        let pm = font.render_pixmap(&l, 800, 600, 0.0, false, &super::ImageStore::new()).unwrap();
        // 盒 x∈[8,108]、阴影右移 1px 覆盖 x∈[9,109);背景绘制后仅 x=108 一列可见。
        let sp = pixel(&pm, 108, 30);
        assert!(sp[0] < 50 && sp[1] < 50 && sp[2] < 50, "盒外偏移处应为黑:{sp:?}");
    }

    #[test]
    fn 输入框_ua_默认边框可见() {
        // 裸 input(无页面样式):UA 1px 灰边框应出现在渲染输出里。
        let doc = Document::parse("<body><input style='width:200px;height:30px'></body>", "https://a.test/");
        let font = FontSet::fallback();
        let l = layout::compute(&doc, 800.0, &font);
        assert!(!l.borders.is_empty(), "输入框应有边框绘制指令");
        let pm = font.render_pixmap(&l, 800, 600, 0.0, false, &super::ImageStore::new()).unwrap();
        let edge = pixel(&pm, 8, 8);
        assert!(
            edge[0] < 200 && edge[1] < 200 && edge[2] < 200,
            "输入框边缘应为灰边框而非纯白:{edge:?}"
        );
    }
}
