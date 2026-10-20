//! 软件渲染:把布局结果绘制成 PNG。
//!
//! 使用 `fontdue` 栅格化字形、`tiny-skia` 承载像素与 PNG 编码。没有可用字体时
//! 退化为色块/占位线,仍会输出可打开的 PNG(响应附 `render_note`)。

use std::path::Path;

use fontdue::{Font, FontSettings};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

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
    pub fn render(
        &self,
        layout: &Layout,
        viewport_w: u32,
        viewport_h: u32,
        scroll_y: f64,
        full_page: bool,
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
        let png = font.render(&l, 640, 480, 0.0, false);
        assert!(png.len() > 8);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn 字体度量回退可用() {
        let f = FontSet::fallback();
        assert!(f.char_width('中', 16.0) >= 16.0);
        assert!(f.char_width('a', 16.0) > 0.0);
    }
}
