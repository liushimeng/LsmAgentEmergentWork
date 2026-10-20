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
        let mut pixmap = match Pixmap::new(width, height) {
            Some(p) => p,
            None => return Vec::new(),
        };
        pixmap.fill(Color::WHITE);

        for b in &layout.boxes {
            if !b.visible {
                continue;
            }
            if let Some(bg) = b.background {
                if let Some(rect) = tiny_skia::Rect::from_xywh(
                    b.x as f32,
                    (b.y - scroll_y) as f32,
                    b.w.max(1.0) as f32,
                    b.h.max(1.0) as f32,
                ) {
                    let mut paint = tiny_skia::Paint::default();
                    paint.set_color_rgba8(bg[0], bg[1], bg[2], 255);
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
        pixmap.encode_png().unwrap_or_default()
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
}
