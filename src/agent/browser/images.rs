//! 图片子资源加载与解码(阶段三第三步)。
//!
//! 宿主预取 `<img src>` 与 CSS `url(...)` 背景图:复用主文档的网络栈
//! (Cookie/重定向/SSRF/TLS 策略一致),解码为 RGBA8 **全内存**缓存,
//! 随标签页生命周期存在,进程退出即销毁。
//!
//! 布局层按固有尺寸排版替换元素(`<img>`),渲染层按 `object-fit` 绘制。
//! 设计见 `docs/MCP_Web_LsmAgentBrowser_Use/03-布局与渲染引擎/10-阶段三第三步图片渲染与替换元素设计.md`。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::document::{self, Document};
use super::{net, urlutil, ConsoleRecord, NetworkRecord, Inner};

/// 单页图片数量上限(默认 48 张)。
const MAX_IMAGES_DEFAULT: usize = 48;
/// 单张图片字节上限(默认 12MB)。
const MAX_IMAGE_BYTES_DEFAULT: usize = 12 * 1024 * 1024;
/// 单页图片总字节上限(默认 48MB)。
const MAX_TOTAL_BYTES: usize = 48 * 1024 * 1024;
/// 单张图片像素上限(40MP),防止解码炸弹打爆内存。
const MAX_PIXELS: u64 = 40_000_000;
/// 图片请求超时(与样式表一致)。
const IMAGE_FETCH_TIMEOUT_MS: u64 = 15_000;
/// 单页图片加载总预算(默认 20 秒),防止多图站点拖垮导航。
const IMAGE_BUDGET_MS_DEFAULT: u64 = 20_000;

/// 解码后的图片(RGBA8,行优先)。
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl DecodedImage {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 取像素(RGBA);越界返回全透明。
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let idx = (y as usize * self.width as usize + x as usize) * 4;
        if idx + 3 >= self.rgba.len() {
            return [0, 0, 0, 0];
        }
        [
            self.rgba[idx],
            self.rgba[idx + 1],
            self.rgba[idx + 2],
            self.rgba[idx + 3],
        ]
    }
}

/// URL → 解码图片(全内存缓存)。
pub type ImageStore = HashMap<String, Arc<DecodedImage>>;

/// URL → 固有尺寸(布局期只读;不含像素数据)。
#[derive(Debug, Clone, Default)]
pub struct ImageSizes {
    pub sizes: HashMap<String, (f64, f64)>,
}

impl ImageSizes {
    /// 查询固有尺寸(自然宽高)。
    pub fn natural(&self, url: &str) -> Option<(f64, f64)> {
        self.sizes.get(url).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    /// 由已解码图片表重建尺寸索引(视口变化后重排复用)。
    pub fn from_store(store: &ImageStore) -> Self {
        let mut sizes = HashMap::with_capacity(store.len());
        for (url, img) in store {
            sizes.insert(url.clone(), (img.width as f64, img.height as f64));
        }
        Self { sizes }
    }
}

/// 单张图片的加载结果(台账用)。
#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub url: String,
    pub bytes: usize,
    pub status: u16,
    pub width: u32,
    pub height: u32,
    /// `data:` 内联来源,未发起网络请求。
    pub inline: bool,
}

/// 图片加载产出。
#[derive(Debug, Clone, Default)]
pub struct ImageLoadOutcome {
    pub store: ImageStore,
    pub sizes: ImageSizes,
    pub network: Vec<NetworkRecord>,
    pub console: Vec<ConsoleRecord>,
    pub loaded: Vec<LoadedImage>,
    pub skipped: Vec<String>,
}

impl ImageLoadOutcome {
    /// 已解码图片数。
    pub fn count(&self) -> usize {
        self.store.len()
    }
}

/// `LAEW_WEB_IMAGES=off` 关闭图片子资源加载(页面仍可打开,只是没有图)。
pub fn images_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_IMAGES")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

fn env_usize(key: &str, default: usize, min: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|v| v.max(min))
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64, min: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|v| v.max(min))
        .unwrap_or(default)
}

/// 可由宿主取回的图片协议(其余协议不发起请求)。
fn is_fetchable(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("data:")
}

/// 收集文档中的 `<img src>` 绝对地址(文档顺序,去重)。
pub fn collect_image_urls(html: &str, base_url: &str) -> Vec<String> {
    let doc = Document::parse(html, base_url);
    let mut out: Vec<String> = Vec::new();
    for node in doc.select("img").unwrap_or_default() {
        let Some(src) = document::attr(&node, "src") else {
            continue;
        };
        let src = src.trim();
        if src.is_empty() {
            continue;
        }
        let Some(abs) = urlutil::resolve(base_url, src) else {
            continue;
        };
        if is_fetchable(&abs) && !out.contains(&abs) {
            out.push(abs);
        }
    }
    out
}

/// Canvas 在图片缓存中的稳定键;同一画布逐帧覆盖,不按变化的 data URL 累积旧帧。
pub fn canvas_key(selector: &str) -> String {
    format!("lsm-canvas:{selector}")
}

fn collect_canvas_images(html: &str, base_url: &str) -> Vec<(String, String)> {
    let doc = Document::parse(html, base_url);
    let mut out = Vec::new();
    for node in doc.select("canvas[data-lsm-canvas]").unwrap_or_default() {
        let Some(src) = document::attr(&node, "data-lsm-canvas") else {
            continue;
        };
        if src.starts_with("data:image/png;base64,") {
            out.push((canvas_key(&Document::stable_selector(&node)), src));
        }
    }
    out
}

/// 收集 CSS 里的图片地址(`<style>` 文本 + 内联 `style` 属性)。
pub fn collect_css_image_urls(html: &str, base_url: &str) -> Vec<String> {
    let doc = Document::parse(html, base_url);
    let mut texts: Vec<String> = Vec::new();
    for node in doc.select("style").unwrap_or_default() {
        texts.push(node.text_contents());
    }
    for node in doc.select("[style]").unwrap_or_default() {
        if let Some(s) = document::attr(&node, "style") {
            texts.push(s);
        }
    }
    let mut out: Vec<String> = Vec::new();
    for text in texts {
        for raw in extract_css_urls(&text) {
            let Some(abs) = urlutil::resolve(base_url, &raw) else {
                continue;
            };
            if is_fetchable(&abs) && !out.contains(&abs) {
                out.push(abs);
            }
        }
    }
    out
}

/// 抽取 CSS `url(...)` 中的地址(去引号)。
pub fn extract_css_urls(css: &str) -> Vec<String> {
    let lower = css.to_ascii_lowercase();
    let bytes = css.as_bytes();
    let mut out = Vec::new();
    let mut idx = 0usize;
    while let Some(pos) = lower[idx..].find("url(") {
        let start = idx + pos + 4;
        let Some(end_rel) = lower[start..].find(')') else {
            break;
        };
        let end = start + end_rel;
        let raw = css[start..end].trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if !raw.is_empty() && !raw.starts_with('#') {
            out.push(raw.to_string());
        }
        idx = end + 1;
        if idx >= bytes.len() {
            break;
        }
    }
    out
}

/// 预取并解码页面图片。`existing` 中已有的图片跳过(增量加载)。
pub async fn load_images(
    inner: &mut Inner,
    http: &net::HttpClient,
    html: &str,
    base_url: &str,
    ua: &str,
    timeout_ms: u64,
    existing: &ImageStore,
) -> ImageLoadOutcome {
    let mut out = ImageLoadOutcome {
        store: existing.clone(),
        ..Default::default()
    };
    if !images_enabled() {
        out.skipped.push("LAEW_WEB_IMAGES=off".to_string());
        return out;
    }
    let max_images = env_usize("LAEW_WEB_MAX_IMAGES", MAX_IMAGES_DEFAULT, 1);
    let max_bytes = env_usize("LAEW_WEB_MAX_IMAGE_BYTES", MAX_IMAGE_BYTES_DEFAULT, 1024);
    let budget_ms = env_u64("LAEW_WEB_IMAGE_BUDGET_MS", IMAGE_BUDGET_MS_DEFAULT, 1_000);
    let canvases = collect_canvas_images(html, base_url);
    let active_canvas_keys: std::collections::HashSet<String> =
        canvases.iter().map(|(key, _)| key.clone()).collect();
    // 画布被移除时回收对应缓存;同一 key 的新帧随后覆盖旧帧,避免动画无界增长。
    out.store.retain(|key, _| {
        !key.starts_with("lsm-canvas:") || active_canvas_keys.contains(key)
    });
    for (key, data_url) in canvases.into_iter().take(max_images) {
        let Some((bytes, _)) = decode_data_url(&data_url) else {
            continue;
        };
        if bytes.len() > max_bytes {
            out.skipped.push(format!("{key} 超过单图上限"));
            continue;
        }
        if let Some(img) = decode_image(&bytes) {
            out.loaded.push(LoadedImage {
                url: key.clone(),
                bytes: bytes.len(),
                status: 200,
                width: img.width,
                height: img.height,
                inline: true,
            });
            out.sizes.sizes.insert(key.clone(), (img.width as f64, img.height as f64));
            out.store.insert(key, Arc::new(img));
        }
    }
    let mut urls = collect_image_urls(html, base_url);
    for u in collect_css_image_urls(html, base_url) {
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    urls.retain(|u| !out.store.contains_key(u));
    if urls.is_empty() {
        out.sizes = ImageSizes::from_store(&out.store);
        return out;
    }
    let deadline = Instant::now() + Duration::from_millis(budget_ms);
    let mut total = 0usize;
    // 第 174 轮降噪:SVG 图片解码失败每页只记一条能力边界告警(猎聘首页
    // 一次刷十几条「图片解码失败 data:image/svg+xml,...」长 base64 文案)。
    let mut svg_failed = 0usize;
    for url in urls.into_iter().take(max_images) {
        if total >= MAX_TOTAL_BYTES {
            out.skipped.push("图片总量超过上限".to_string());
            break;
        }
        // 总预算耗尽:剩余图片放弃加载(页面照常可用)。
        if Instant::now() >= deadline {
            out.skipped.push(format!("图片加载超出总预算({budget_ms}ms),剩余图片已跳过"));
            break;
        }
        // data: 内联图片:不走网络。
        if let Some((bytes, mime)) = decode_data_url(&url) {
            // 第 174 轮:@font-face 的 woff/woff2 等**非图像** data: URL 不该进
            // 图片解码器 —— 之前每条都产「图片解码失败 data:application/font-woff,
            // ...(巨长 base64)」告警刷屏,纯属噪音。
            let mime_l = mime.trim().to_ascii_lowercase();
            if !mime_l.is_empty() && !mime_l.starts_with("image/") {
                continue;
            }
            // SVG 光栅化是内核能力边界:不计逐条失败,收口为一条统计告警。
            if mime_l.starts_with("image/svg") {
                svg_failed += 1;
                continue;
            }
            if bytes.len() > max_bytes {
                out.skipped.push(format!("{url} 超过单图上限"));
                continue;
            }
            total += bytes.len();
            match decode_image(&bytes) {
                Some(img) => {
                    out.loaded.push(LoadedImage {
                        url: url.clone(),
                        bytes: bytes.len(),
                        status: 200,
                        width: img.width,
                        height: img.height,
                        inline: true,
                    });
                    out.sizes
                        .sizes
                        .insert(url.clone(), (img.width as f64, img.height as f64));
                    out.store.insert(url.clone(), Arc::new(img));
                }
                None => out.console.push(ConsoleRecord {
                    level: "warn".to_string(),
                    text: format!("图片解码失败 {url}"),
                    at_ms: super::now_ms(),
                }),
            }
            continue;
        }
        // 单图超时不超过总预算剩余时间。
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        let mut req = net::FetchRequest::get(
            &url,
            timeout_ms
                .min(IMAGE_FETCH_TIMEOUT_MS)
                .min(remaining.max(1_000))
                .max(1_000),
        );
        req.max_bytes = max_bytes;
        // 图片子资源:浏览器语义带 Referer + Sec-Fetch-*(image) 指纹头。
        req.context = Some(super::reqctx::RequestContext::subresource(
            base_url,
            super::reqctx::Dest::Image,
            ua,
        ));
        match http.fetch(req, &mut inner.cookies).await {
            Ok(resp) => {
                let status = resp.status;
                let ct = resp.content_type();
                out.network.push(NetworkRecord {
                    url: url.clone(),
                    method: "GET".to_string(),
                    status,
                    content_type: ct.clone(),
                    bytes: resp.bytes.len(),
                    elapsed_ms: resp.elapsed_ms,
                    redirect: resp.redirect_chain.len() > 1,
                    from_cache: false,
                    at_ms: super::now_ms(),
                });
                if !(200..300).contains(&status) {
                    out.console.push(ConsoleRecord {
                        level: "warn".to_string(),
                        text: format!("图片加载失败 {url}:HTTP {status}"),
                        at_ms: super::now_ms(),
                    });
                    continue;
                }
                // 文本响应多半是错误页/占位 HTML,不浪费解码开销。
                if ct.starts_with("text/html") {
                    out.skipped.push(format!("{url} 返回 HTML(非图片)"));
                    continue;
                }
                if resp.bytes.len() > max_bytes {
                    out.skipped.push(format!("{url} 超过单图上限"));
                    continue;
                }
                // SVG 光栅化是内核能力边界(与 data: 路径同一收口)。
                if ct.contains("svg") || url.to_ascii_lowercase().ends_with(".svg") {
                    svg_failed += 1;
                    continue;
                }
                total += resp.bytes.len();
                match decode_image(&resp.bytes) {
                    Some(img) => {
                        out.loaded.push(LoadedImage {
                            url: url.clone(),
                            bytes: resp.bytes.len(),
                            status,
                            width: img.width,
                            height: img.height,
                            inline: false,
                        });
                        out.sizes
                            .sizes
                            .insert(url.clone(), (img.width as f64, img.height as f64));
                        out.store.insert(url.clone(), Arc::new(img));
                    }
                    None => out.console.push(ConsoleRecord {
                        level: "warn".to_string(),
                        text: format!("图片解码失败 {url}"),
                        at_ms: super::now_ms(),
                    }),
                }
            }
            Err(e) => {
                out.console.push(ConsoleRecord {
                    level: "warn".to_string(),
                    text: format!("图片加载失败 {url}:{e}"),
                    at_ms: super::now_ms(),
                });
            }
        }
    }
    if svg_failed > 0 {
        out.console.push(ConsoleRecord {
            level: "warn".to_string(),
            text: format!(
                "SVG 图片暂不渲染(内核无 SVG 光栅化),本页跳过 {svg_failed} 个;图标缺失属能力边界而非加载失败"
            ),
            at_ms: super::now_ms(),
        });
    }
    out.sizes = ImageSizes::from_store(&out.store);
    out
}

/// 解码图片字节(RGBA8)。失败返回 `None`(格式不支持/像素超限)。
pub fn decode_image(bytes: &[u8]) -> Option<DecodedImage> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let rgba = decoded.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    Some(DecodedImage {
        width: w,
        height: h,
        rgba: rgba.into_raw(),
    })
}

/// 解析 `data:` URL(返回字节与 MIME)。
pub fn decode_data_url(url: &str) -> Option<(Vec<u8>, String)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let base64 = meta.to_ascii_lowercase().contains(";base64");
    let mime = meta
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let bytes = if base64 {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()?
    } else {
        percent_decode(payload).into_bytes()
    };
    Some((bytes, mime))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgba([(x * 40) as u8, (y * 40) as u8, 128, 255]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn 收集_img_绝对地址() {
        let html = r#"<body><img src="/a.png"><img src="b.jpg"><img><img src="https://c.test/c.gif"></body>"#;
        let urls = collect_image_urls(html, "https://a.test/dir/page");
        assert_eq!(
            urls,
            vec![
                "https://a.test/a.png".to_string(),
                "https://a.test/dir/b.jpg".to_string(),
                "https://c.test/c.gif".to_string()
            ]
        );
    }

    #[test]
    fn 收集_css_背景图地址() {
        let html = r#"<style>.a{background-image:url("/bg.png")}.b{background:url('x.png') no-repeat}</style>
                      <div style="background-image:url(/inline.png)"></div>"#;
        let urls = collect_css_image_urls(html, "https://a.test/");
        assert!(urls.contains(&"https://a.test/bg.png".to_string()));
        assert!(urls.contains(&"https://a.test/x.png".to_string()));
        assert!(urls.contains(&"https://a.test/inline.png".to_string()));
    }

    #[test]
    fn 解码_png_得到固有尺寸() {
        let img = decode_image(&png_bytes(4, 3)).unwrap();
        assert_eq!(img.dimensions(), (4, 3));
        assert_eq!(img.pixel(0, 0), [0, 0, 128, 255]);
        assert_eq!(img.pixel(99, 99), [0, 0, 0, 0]);
    }

    #[test]
    fn data_url_直接解码() {
        let bytes = png_bytes(2, 2);
        let b64 = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        };
        let url = format!("data:image/png;base64,{b64}");
        let (raw, mime) = decode_data_url(&url).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(raw.len(), bytes.len());
        assert!(decode_image(&raw).is_some());
    }

    #[test]
    fn 尺寸索引由缓存重建() {
        let mut store = ImageStore::new();
        store.insert(
            "https://a.test/a.png".to_string(),
            Arc::new(decode_image(&png_bytes(8, 6)).unwrap()),
        );
        let sizes = ImageSizes::from_store(&store);
        assert_eq!(sizes.natural("https://a.test/a.png"), Some((8.0, 6.0)));
        assert_eq!(sizes.natural("https://a.test/missing.png"), None);
    }

    #[test]
    fn 非图片字节解码失败() {
        assert!(decode_image(b"not an image").is_none());
    }
}
