//! 渲染层(阶段六第一步重构为目录):布局显示列表 → 像素。
//!
//! 目录分工:
//! - `mod.rs`      字体集(FontSet:度量/字形栅格/覆盖层文本)+ 渲染入口 + PNG 裁剪;
//! - `display.rs`  绘制显示列表数据模型与旧管线回退合成器;
//! - `raster.rs`   光栅化器(圆角/渐变/边框/模糊阴影/图片/文本,统一 scissor+alpha);
//! - `blur.rs`     三趟盒式模糊(近似高斯);
//! - `tests.rs`    渲染单测。
//!
//! 像素管线:`Layout::display`(非空)→ 直接光栅化;为空(手构 Layout/旧路径)
//! → `display::build_legacy` 按旧通道顺序合成,输出与重构前一致。
//! 设计见 `docs/MCP_Web_LsmAgentBrowser_Use/03-布局与渲染引擎/22-专业渲染引擎重构设计与实现.md`。

pub mod display;
mod blur;
mod raster;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod widget_tests;

pub use display::{BgFill, ClipRect, DisplayItem, DisplayKind};

use std::collections::HashMap;
use std::sync::Mutex;
use std::path::Path;

use fontdue::{Font, FontSettings};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

use super::images::ImageStore;
use super::layout::Layout;

pub struct FontSet {
    /// 字体回退链:主字体在前,按 `pick` 的字形覆盖逐字选字。
    /// TTC 字体集合由 fontdue `collection_index` 取第 0 面,天然可用。
    fonts: Vec<Font>,
    pub path: String,
    /// 字形栅格缓存((char, size 位模式) → 度量+位图):重绘热点
    /// (page_bars 1Hz 重绘 / 截图重渲染),命中免曲线展平。
    glyph_cache: Mutex<HashMap<(char, u64), (fontdue::Metrics, Vec<u8>)>>,
    /// 字宽缓存((char, size 位模式) → advance):换行循环逐字查度量的热点。
    width_cache: Mutex<HashMap<(char, u64), f64>>,
}

/// 字形/字宽缓存容量上限:超出整体清空(条目小、简单有界即可)。
const FONT_CACHE_CAP: usize = 4096;

impl FontSet {
    /// 跨平台加载:按候选序构建回退链,**覆盖探针达标即停**
    /// (主字体已有拉丁+CJK 时只载一个,零额外启动开销;Linux 首选
    /// DejaVu 无 CJK 时继续载入 Noto/文泉驿等补齐,根治中文豆腐块)。
    pub fn load() -> Self {
        let mut fonts: Vec<Font> = Vec::new();
        let mut path = String::new();
        for p in candidate_fonts() {
            if fonts.len() >= 3 {
                break;
            }
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                if path.is_empty() {
                    path = p;
                }
                let covered = font.lookup_glyph_index('A') != 0
                    && font.lookup_glyph_index('中') != 0;
                fonts.push(font);
                if covered {
                    break;
                }
            }
        }
        Self {
            fonts,
            path,
            glyph_cache: Mutex::new(HashMap::new()),
            width_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn fallback() -> Self {
        Self {
            fonts: Vec::new(),
            path: String::new(),
            glyph_cache: Mutex::new(HashMap::new()),
            width_cache: Mutex::new(HashMap::new()),
        }
    }

    /// 仅测试用:按给定字体序构建回退链。
    #[cfg(test)]
    pub(crate) fn from_fonts(fonts: Vec<Font>, path: String) -> Self {
        Self {
            fonts,
            path,
            glyph_cache: Mutex::new(HashMap::new()),
            width_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn has_font(&self) -> bool {
        !self.fonts.is_empty()
    }

    /// 链上是否有任一字体覆盖该字符(诊断/测试用)。
    #[cfg(test)]
    pub(crate) fn covers(&self, ch: char) -> bool {
        self.pick(ch).is_some()
    }

    /// 按字形覆盖选字:首个 `lookup_glyph_index != 0` 的字体。
    fn pick(&self, ch: char) -> Option<&Font> {
        self.fonts
            .iter()
            .find(|f| f.lookup_glyph_index(ch) != 0)
    }

    pub fn char_width(&self, ch: char, size: f64) -> f64 {
        let key = (ch, size.to_bits());
        if let Some(w) = self.width_cache.lock().unwrap().get(&key) {
            return *w;
        }
        let w = match self.pick(ch) {
            Some(font) => {
                let m = font.metrics(ch, size as f32);
                if m.advance_width > 0.0 {
                    m.advance_width as f64
                } else {
                    fallback_char_width(ch, size)
                }
            }
            None => fallback_char_width(ch, size),
        };
        let mut cache = self.width_cache.lock().unwrap();
        if cache.len() >= FONT_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, w);
        w
    }

    pub fn text_width(&self, text: &str, size: f64) -> f64 {
        text.chars().map(|c| self.char_width(c, size)).sum()
    }

    /// 单字形栅格化(光栅化器用):回退链选字 + 栅格缓存;
    /// 全链无覆盖返回 None(调用方跳过绘制,不再画 tofu 框)。
    pub(crate) fn rasterize_glyph(&self, ch: char, size: f32) -> Option<(fontdue::Metrics, Vec<u8>)> {
        let key = (ch, (size as f64).to_bits());
        if let Some(v) = self.glyph_cache.lock().unwrap().get(&key) {
            return Some((v.0, v.1.clone()));
        }
        let font = self.pick(ch)?;
        let (metrics, bitmap) = font.rasterize(ch, size);
        let mut cache = self.glyph_cache.lock().unwrap();
        if cache.len() >= FONT_CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, (metrics, bitmap.clone()));
        Some((metrics, bitmap))
    }

    /// 合成层文本行(屏蔽页面状态栏/进度栏专用,`page_bars.rs` 调用):
    /// 按可用宽度 `max_w` 逐字截断(补 `…`)后栅格化绘制,返回实际绘制宽度。
    /// 无字体时画占位色条(与页面文本的退化行为一致)。
    /// `x/y` 为行左上角;`bold=true` 时双重绘制加粗(与页面 bold 同款近似)。
    pub(crate) fn draw_overlay_text(
        &self,
        pixmap: &mut Pixmap,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: [u8; 3],
        bold: bool,
        max_w: f32,
    ) -> f32 {
        if text.is_empty() || max_w <= 0.0 {
            return 0.0;
        }
        // 截断:按字符累计宽度,超出可用宽度即止并补省略号。
        let mut kept = String::new();
        let mut used = 0.0f32;
        let ellipsis = self.text_width("…", size as f64) as f32;
        for ch in text.chars() {
            let cw = self.char_width(ch, size as f64) as f32;
            if used + cw > max_w - ellipsis && !kept.is_empty() {
                kept.push('…');
                break;
            }
            kept.push(ch);
            used += cw;
            if used >= max_w {
                break;
            }
        }
        if !self.has_font() {
            // 无字体:占位色条,宽度反映文本长度(截断后)。
            fill_rect(
                pixmap,
                x,
                y + size * 0.35,
                used.min(max_w),
                (size * 0.35).max(2.0),
                color,
            );
            return used;
        }
        let baseline = y + size;
        let mut pen_x = x;
        for ch in kept.chars() {
            // 回退链逐字选字;全链无覆盖按启发式宽度推进,不画 tofu。
            let Some((metrics, bitmap)) = self.rasterize_glyph(ch, size) else {
                pen_x += self.char_width(ch, size as f64) as f32;
                continue;
            };
            if metrics.width > 0 && metrics.height > 0 {
                let gx = pen_x + metrics.xmin as f32;
                let gy = baseline - metrics.height as f32 - metrics.ymin as f32;
                blend_glyph(pixmap, gx, gy, metrics.width, metrics.height, &bitmap, color);
                if bold {
                    blend_glyph(pixmap, gx + 1.0, gy, metrics.width, metrics.height, &bitmap, color);
                }
            }
            pen_x += metrics.advance_width;
        }
        pen_x - x
    }

    /// 渲染一页。`scroll_x`/`scroll_y` 为滚动偏移;`full_page=true` 时取整页。
    ///
    /// `images` 为已解码图片缓存(阶段三第三步);缺失的图片跳过绘制。
    pub fn render(
        &self,
        layout: &Layout,
        viewport_w: u32,
        viewport_h: u32,
        scroll_x: f64,
        scroll_y: f64,
        full_page: bool,
        images: &ImageStore,
    ) -> Vec<u8> {
        self.render_pixmap(layout, viewport_w, viewport_h, scroll_x, scroll_y, full_page, images)
            .map(|p| p.encode_png().unwrap_or_default())
            .unwrap_or_default()
    }

    /// 渲染一页为 pixmap(阶段四第二步:供 clip 区域截图裁剪)。
    ///
    /// 显示列表优先;手构 `Layout`(display 为空)回退旧管线合成器,
    /// 输出与重构前逐像素一致。
    pub fn render_pixmap(
        &self,
        layout: &Layout,
        viewport_w: u32,
        viewport_h: u32,
        scroll_x: f64,
        scroll_y: f64,
        full_page: bool,
        images: &ImageStore,
    ) -> Option<Pixmap> {
        let legacy;
        let items: &[DisplayItem] = if layout.display.is_empty() {
            legacy = display::build_legacy(layout);
            &legacy
        } else {
            &layout.display
        };
        // 整页截图宽:内容横向溢出视口时把右缘包进来(2026-10-30 京东:
        // 此前恒视口宽,搜索栏越界到 2237 的部分在任何截图里都被硬裁)。
        let content_w = items
            .iter()
            .map(|it| (it.x + it.w).ceil().max(0.0) as u64)
            .max()
            .unwrap_or(0);
        let width = if full_page {
            ((viewport_w as u64).max(content_w) as u32).clamp(64, 7680)
        } else {
            viewport_w.clamp(64, 7680)
        };
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
        // 诊断:显示列表全量落盘(LAEW_DUMP_DISPLAY=路径),定位「布局有文本/渲染无像素」类矛盾。
        if let Ok(path) = std::env::var("LAEW_DUMP_DISPLAY") {
            let mut buf = String::new();
            for (i, it) in items.iter().enumerate() {
                let kind = match &it.kind {
                    DisplayKind::Background { fill } => format!("bg({:?})", fill),
                    DisplayKind::Border { .. } => "border".to_string(),
                    DisplayKind::Shadow { .. } => "shadow".to_string(),
                    DisplayKind::Image { src, .. } => format!("img({})", src),
                    DisplayKind::Text { text, size, color, bold } => format!(
                        "text(sz={:.0} color={:?} bold={} t={})",
                        size, color, bold,
                        &text.chars().take(14).collect::<String>()
                    ),
                    DisplayKind::TextShadow { .. } => "textshadow".to_string(),
                    DisplayKind::CanvasNote { .. } => "canvasnote".to_string(),
                    DisplayKind::WidgetGlyph { glyph, open, color } => {
                        format!("widget({glyph:?},open={open},color={color:?})")
                    }
                };
                buf.push_str(&format!(
                    "#{i} {} x={:.0} y={:.0} w={:.0} h={:.0} op={:.2} fixed={} z={:?} tf={:?} clip={:?}\n",
                    kind,
                    it.x,
                    it.y,
                    it.w,
                    it.h,
                    it.opacity,
                    it.fixed,
                    it.z,
                    it.transform.map(|t| (t.a, t.b, t.c, t.d, t.e, t.f)),
                    it.clip
                ));
            }
            std::fs::write(&path, buf).ok();
        }
        raster::rasterize(self, &mut pixmap, items, scroll_x, scroll_y, images);
        Some(pixmap)
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

/// 各平台候选字体序(主字体 → CJK 回退 → 一般拉丁回退):
/// 覆盖探针达标即停,通常只载 1 个;Linux 首选无 CJK 时继续补齐。
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
            // 首选(Debian/Ubuntu 与 Fedora/Arch 两种安装位)。
            out.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
            out.push("/usr/share/fonts/dejavu/DejaVuSans.ttf".into());
            // CJK 回退(此前排在拉丁备选之后,永远加载不到 → 中文豆腐块根因)。
            out.push("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into());
            out.push("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".into());
            out.push("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc".into());
            out.push("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc".into());
            out.push("/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc".into());
            out.push("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc".into());
            out.push("/usr/share/fonts/wenquanyi/wqy-zenhei/wqy-zenhei.ttc".into());
            out.push("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf".into());
            // 一般拉丁回退。
            out.push("/usr/share/fonts/liberation/LiberationSans-Regular.ttf".into());
            out.push("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf".into());
            out.push("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf".into());
            // 发行版安装位兜底(硬编码未命中时按目录扫描补)。
            out.extend(scan_font_dirs());
        }
    }
    out
}

/// Linux 字体目录兜底扫描:有界(深度≤3、最多 48 个文件),
/// CJK 偏好名优先(回退链按序取用,越靠前越早补齐覆盖)。
#[cfg(target_os = "linux")]
fn scan_font_dirs() -> Vec<String> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<String>) {
        if depth > 3 || out.len() >= 48 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                walk(&p, depth + 1, out);
            } else if matches!(
                p.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc"
            ) {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
    let mut files = Vec::new();
    for root in ["/usr/share/fonts", "/usr/local/share/fonts"] {
        walk(Path::new(root), 0, &mut files);
    }
    files.sort_by_key(|p| {
        let lower = p.to_ascii_lowercase();
        let score = if lower.contains("cjk") || lower.contains("wqy") || lower.contains("droid") {
            0
        } else {
            1
        };
        (score, p.clone())
    });
    files
}

#[cfg(not(target_os = "linux"))]
fn scan_font_dirs() -> Vec<String> {
    Vec::new()
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

/// 裁剪 pixmap 子区域并编码 PNG(阶段四第二步:clip 区域截图)。
/// `x/y/w/h` 为源图像素坐标(浮点,内部取整并 clamp 到边界)。
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
