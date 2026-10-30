//! 页面子资源加载(阶段二第二步:外链样式表)。
//!
//! 外链 CSS 由宿主异步预取(与主文档共用 Cookie/重定向/SSRF/TLS/字符集策略),
//! 成功后**原位替换** `<link rel="stylesheet">` 为 `<style data-lsm-href=...>`,
//! 保持文档顺序;失败只记台账,不阻断页面。
//!
//! 图片/字体等其它子资源在阶段四资源加载器统一处理。

use super::document::{self, Document};
use super::{net, ConsoleRecord, NetworkRecord};
use super::Inner;

const MAX_STYLESHEETS_DEFAULT: usize = 16;
const MAX_CSS_BYTES_DEFAULT: usize = 2 * 1024 * 1024;
const MAX_TOTAL_CSS_BYTES: usize = 4 * 1024 * 1024;
const CSS_FETCH_TIMEOUT_MS: u64 = 15_000;

#[derive(Debug, Clone)]
pub struct StyleLink {
    pub href: String,
    pub media: String,
}

#[derive(Debug, Clone)]
pub struct LoadedSheet {
    pub url: String,
    pub media: String,
    pub bytes: usize,
    pub status: u16,
    /// `data:` 内联来源,未发起网络请求。
    pub inline: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ResourceOutcome {
    pub html: String,
    pub network: Vec<NetworkRecord>,
    pub console: Vec<ConsoleRecord>,
    pub sheets: Vec<LoadedSheet>,
    pub skipped: Vec<String>,
}

impl ResourceOutcome {
    pub fn loaded(&self) -> bool {
        !self.sheets.is_empty()
    }
}

/// `LAEW_WEB_STYLES=off` 关闭外链样式加载(内联 `<style>` 仍生效)。
pub fn styles_enabled() -> bool {
    !matches!(
        std::env::var("LAEW_WEB_STYLES")
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

/// 收集文档中的外链样式表(绝对 URL,文档顺序)。
pub fn collect_stylesheet_links(html: &str, base_url: &str) -> Vec<StyleLink> {
    let doc = Document::parse(html, base_url);
    let Ok(nodes) = doc.select("link") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in nodes {
        let rel = document::attr(&node, "rel")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "stylesheet") {
            continue;
        }
        if document::attr(&node, "disabled").is_some() {
            continue;
        }
        let Some(href) = document::attr(&node, "href") else {
            continue;
        };
        let Some(abs) = super::urlutil::resolve(base_url, &href) else {
            continue;
        };
        if !(abs.starts_with("http://") || abs.starts_with("https://") || abs.starts_with("data:"))
        {
            continue;
        }
        out.push(StyleLink {
            href: abs,
            media: document::attr(&node, "media").unwrap_or_default(),
        });
    }
    out
}

/// 把已抓取的 CSS 原位写回文档。`fetched` 为 `(绝对 URL, CSS 文本)`。
///
/// 没有任何命中时返回原 HTML(避免无谓的序列化)。
pub fn apply_fetched_styles(html: &str, base_url: &str, fetched: &[(String, String)]) -> String {
    if fetched.is_empty() {
        return html.to_string();
    }
    let doc = Document::parse(html, base_url);
    let Ok(nodes) = doc.select("link") else {
        return html.to_string();
    };
    let mut changed = false;
    for node in nodes {
        let rel = document::attr(&node, "rel")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "stylesheet") {
            continue;
        }
        let Some(href) = document::attr(&node, "href") else {
            continue;
        };
        let Some(abs) = super::urlutil::resolve(base_url, &href) else {
            continue;
        };
        let Some((_, css)) = fetched.iter().find(|(url, _)| url == &abs) else {
            continue;
        };
        let media = document::attr(&node, "media").unwrap_or_default();
        let style_html = format!(
            "<style data-lsm-href=\"{}\" media=\"{}\">{}</style>",
            escape_attr(&abs),
            escape_attr(&media),
            sanitize_css(css)
        );
        let fragments = parse_fragment_nodes(&style_html);
        if fragments.is_empty() {
            continue;
        }
        for frag in fragments {
            node.insert_before(frag);
        }
        node.detach();
        changed = true;
    }
    if changed {
        doc.serialize_full()
    } else {
        html.to_string()
    }
}

/// 预取并内联外链样式表。失败项降级为 console 警告。
pub async fn load_stylesheets(
    inner: &mut Inner,
    http: &net::HttpClient,
    html: &str,
    base_url: &str,
    ua: &str,
    timeout_ms: u64,
) -> ResourceOutcome {
    let mut out = ResourceOutcome {
        html: html.to_string(),
        ..Default::default()
    };
    if !styles_enabled() {
        out.skipped.push("LAEW_WEB_STYLES=off".to_string());
        return out;
    }
    let links = collect_stylesheet_links(html, base_url);
    if links.is_empty() {
        return out;
    }
    let max_sheets = env_usize("LAEW_WEB_MAX_STYLESHEETS", MAX_STYLESHEETS_DEFAULT, 1);
    let max_bytes = env_usize("LAEW_WEB_MAX_CSS_BYTES", MAX_CSS_BYTES_DEFAULT, 1024);
    let mut fetched: Vec<(String, String)> = Vec::new();
    let mut total = 0usize;
    for link in links.into_iter().take(max_sheets) {
        if let Some(pos) = fetched.iter().position(|(url, _)| url == &link.href) {
            let css = fetched[pos].1.clone();
            out.sheets.push(LoadedSheet {
                url: link.href,
                media: link.media,
                bytes: css.len(),
                status: 200,
                inline: true,
            });
            continue;
        }
        if let Some(data) = decode_data_css(&link.href) {
            if data.len() > max_bytes {
                out.skipped.push(format!("{} 超过单表上限", link.href));
                continue;
            }
            total += data.len();
            if total > MAX_TOTAL_CSS_BYTES {
                out.skipped.push("样式总量超过上限".to_string());
                break;
            }
            out.sheets.push(LoadedSheet {
                url: link.href.clone(),
                media: link.media.clone(),
                bytes: data.len(),
                status: 200,
                inline: true,
            });
            fetched.push((link.href, data));
            continue;
        }
        // 阶段四第二步：先查全内存 HTTP 缓存（强缓存命中跳过网络）。
        if super::cache_enabled() {
            if let Some(entry) = inner.http_cache.get(&link.href) {
                let css = super::urlutil::decode_body(&entry.bytes, &super::net::header_value(&entry.headers, "content-type").unwrap_or_default());
                if css.len() > max_bytes {
                    out.skipped.push(format!("{} 超过单表上限", link.href));
                    continue;
                }
                total += css.len();
                if total > MAX_TOTAL_CSS_BYTES {
                    out.skipped.push("样式总量超过上限".to_string());
                    break;
                }
                out.sheets.push(LoadedSheet {
                    url: link.href.clone(),
                    media: link.media.clone(),
                    bytes: css.len(),
                    status: entry.status,
                    inline: false,
                });
                out.network.push(NetworkRecord {
                    url: link.href.clone(),
                    method: "GET".to_string(),
                    status: entry.status,
                    content_type: super::net::header_value(&entry.headers, "content-type").unwrap_or_default(),
                    bytes: css.len(),
                    elapsed_ms: 0,
                    redirect: false,
                    from_cache: true,
                    at_ms: super::now_ms(),
                });
                fetched.push((link.href, css));
                continue;
            }
        }
        let mut req =
            net::FetchRequest::get(&link.href, timeout_ms.min(CSS_FETCH_TIMEOUT_MS).max(1_000));
        req.max_bytes = max_bytes;
        // 样式表子资源:浏览器语义带 Referer + Sec-Fetch-*(style) 指纹头。
        req.context = Some(super::reqctx::RequestContext::subresource(
            base_url,
            super::reqctx::Dest::Style,
            ua,
        ));
        match http.fetch(req, &mut inner.cookies).await {
            Ok(resp) => {
                let status = resp.status;
                let ct = resp.content_type();
                out.network.push(NetworkRecord {
                    url: link.href.clone(),
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
                        text: format!("外链样式加载失败 {}:HTTP {status}", link.href),
                        at_ms: super::now_ms(),
                    });
                    continue;
                }
                let css = resp.text();
                if css.len() > max_bytes {
                    out.skipped.push(format!("{} 超过单表上限", link.href));
                    continue;
                }
                total += css.len();
                if total > MAX_TOTAL_CSS_BYTES {
                    out.skipped.push("样式总量超过上限".to_string());
                    break;
                }
                out.sheets.push(LoadedSheet {
                    url: link.href.clone(),
                    media: link.media.clone(),
                    bytes: css.len(),
                    status,
                    inline: false,
                });
                // 阶段四第二步：可缓存响应写入全内存缓存。
                super::cache_put(&mut inner.http_cache, &link.href, &resp);
                fetched.push((link.href, css));
            }
            Err(e) => {
                out.console.push(ConsoleRecord {
                    level: "warn".to_string(),
                    text: format!("外链样式加载失败 {}:{e}", link.href),
                    at_ms: super::now_ms(),
                });
            }
        }
    }
    // 第 45 轮:抓取并内联 `@import`(此前只抓主样式表,`@import` 的依赖表
    // 完全没取 —— 站点把公共样式放在被导入的表里时,页面就是**无样式**的)。
    inline_css_imports(inner, http, &mut fetched, ua, timeout_ms, max_bytes, &mut out).await;
    out.html = apply_fetched_styles(html, base_url, &fetched);
    out
}

/// 抓取 `@import` 依赖并原位内联(广度优先;深度上限与循环检测沿用
/// `resource::import` 口径)。
///
/// 抓取阶段与内联阶段分开:先把所有被导入的样式表取回来(异步),再用
/// `import::resolve_imports` 同步内联 —— 避免异步递归借用冲突。
async fn inline_css_imports(
    inner: &mut Inner,
    http: &net::HttpClient,
    fetched: &mut Vec<(String, String)>,
    ua: &str,
    timeout_ms: u64,
    max_bytes: usize,
    out: &mut ResourceOutcome,
) {
    // 1) 广度优先收集要抓的 import 地址。
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<(String, usize)> = Vec::new();
    for (sheet_url, css) in fetched.iter() {
        for (raw, _media) in super::resource::import_urls(css) {
            if let Some(abs) = super::urlutil::resolve(sheet_url, &raw) {
                if visited.insert(abs.clone()) {
                    queue.push((abs, 1));
                }
            }
        }
    }
    if queue.is_empty() {
        return;
    }
    // 2) 逐层抓取(被导入表里的 @import 继续入队)。
    let mut imported: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    while let Some((url, depth)) = queue.pop() {
        if depth > super::resource::MAX_IMPORT_DEPTH {
            out.skipped.push(format!("{url} @import 递归深度超限"));
            continue;
        }
        let Some(css) = fetch_import_css(inner, http, &url, ua, timeout_ms, max_bytes, out).await
        else {
            continue;
        };
        for (raw, _media) in super::resource::import_urls(&css) {
            if let Some(abs) = super::urlutil::resolve(&url, &raw) {
                if visited.insert(abs.clone()) {
                    queue.push((abs, depth + 1));
                }
            }
        }
        imported.insert(url, css);
    }
    // 3) 同步内联:逐张主样式表把 @import 换成被导入的 CSS 文本。
    for (sheet_url, css) in fetched.iter_mut() {
        if !super::resource::has_import(css) {
            continue;
        }
        let mut seen = std::collections::HashSet::new();
        let mut loader = |u: &str| -> Result<String, String> {
            Ok(imported.get(u).cloned().unwrap_or_default())
        };
        if let Ok(inlined) =
            super::resource::resolve_imports(css, sheet_url, 0, &mut seen, &mut loader)
        {
            *css = inlined;
        }
    }
}

/// 抓取一张被 `@import` 的样式表(走全内存缓存优先)。
async fn fetch_import_css(
    inner: &mut Inner,
    http: &net::HttpClient,
    url: &str,
    ua: &str,
    timeout_ms: u64,
    max_bytes: usize,
    out: &mut ResourceOutcome,
) -> Option<String> {
    if super::cache_enabled() {
        if let Some(entry) = inner.http_cache.get(url) {
            let css = super::urlutil::decode_body(
                &entry.bytes,
                &super::net::header_value(&entry.headers, "content-type").unwrap_or_default(),
            );
            out.network.push(NetworkRecord {
                url: url.to_string(),
                method: "GET".to_string(),
                status: entry.status,
                content_type: super::net::header_value(&entry.headers, "content-type")
                    .unwrap_or_default(),
                bytes: css.len(),
                elapsed_ms: 0,
                redirect: false,
                from_cache: true,
                at_ms: super::now_ms(),
            });
            return Some(css);
        }
    }
    let mut req = net::FetchRequest::get(url, timeout_ms.min(CSS_FETCH_TIMEOUT_MS).max(1_000));
    req.max_bytes = max_bytes;
    req.context = Some(super::reqctx::RequestContext::subresource(
        url,
        super::reqctx::Dest::Style,
        ua,
    ));
    match http.fetch(req, &mut inner.cookies).await {
        Ok(resp) => {
            let status = resp.status;
            out.network.push(NetworkRecord {
                url: url.to_string(),
                method: "GET".to_string(),
                status,
                content_type: resp.content_type(),
                bytes: resp.bytes.len(),
                elapsed_ms: resp.elapsed_ms,
                redirect: resp.redirect_chain.len() > 1,
                from_cache: false,
                at_ms: super::now_ms(),
            });
            if !(200..300).contains(&status) {
                out.console.push(ConsoleRecord {
                    level: "warn".to_string(),
                    text: format!("@import 样式加载失败 {url}:HTTP {status}"),
                    at_ms: super::now_ms(),
                });
                return None;
            }
            let css = resp.text();
            super::cache_put(&mut inner.http_cache, url, &resp);
            Some(css)
        }
        Err(e) => {
            out.console.push(ConsoleRecord {
                level: "warn".to_string(),
                text: format!("@import 样式加载失败 {url}:{e}"),
                at_ms: super::now_ms(),
            });
            None
        }
    }
}

fn decode_data_css(url: &str) -> Option<String> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.to_ascii_lowercase().contains("text/css") {
        return None;
    }
    if meta.to_ascii_lowercase().contains(";base64") {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()?;
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    Some(percent_decode(payload))
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

fn sanitize_css(css: &str) -> String {
    // 防止 CSS 文本提前闭合 `<style>`。
    css.replace("</style", "<\\/style")
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn parse_fragment_nodes(html: &str) -> Vec<kuchikiki::NodeRef> {
    use kuchikiki::traits::TendrilSink;
    let wrapped = format!("<body>{html}</body>");
    let parsed = kuchikiki::parse_html().one(wrapped);
    parsed
        .select_first("body")
        .map(|b| b.as_node().children().collect::<Vec<_>>())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 收集外链样式表并解析相对地址() {
        let html = r#"<head>
            <link rel="stylesheet" href="/a.css">
            <link rel="Stylesheet" href="https://b.test/b.css" media="screen">
            <link rel="icon" href="/i.png">
            <link rel="stylesheet" disabled href="/off.css">
        </head><body>x</body>"#;
        let links = collect_stylesheet_links(html, "https://a.test/dir/page");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].href, "https://a.test/a.css");
        assert_eq!(links[1].href, "https://b.test/b.css");
        assert_eq!(links[1].media, "screen");
    }

    #[test]
    fn 原位替换保持顺序并保留_media() {
        let html = r#"<head><link rel="stylesheet" href="/a.css"><link rel="stylesheet" href="/b.css"></head><body>x</body>"#;
        let fetched = vec![
            (
                "https://a.test/a.css".to_string(),
                "body{color:red}".to_string(),
            ),
            (
                "https://a.test/b.css".to_string(),
                "p{font-size:20px}".to_string(),
            ),
        ];
        let out = apply_fetched_styles(html, "https://a.test/", &fetched);
        let a = out.find("color:red").unwrap();
        let b = out.find("font-size:20px").unwrap();
        assert!(a < b);
        assert!(out.contains("data-lsm-href=\"https://a.test/a.css\""));
        assert!(!out.contains("<link"));
    }

    #[test]
    fn data_url_样式直接内联() {
        let css = decode_data_css("data:text/css,body%7Bcolor:red%7D").unwrap();
        assert_eq!(css, "body{color:red}");
        let b64 = decode_data_css("data:text/css;base64,Ym9keXtjb2xvcjpyZWR9").unwrap();
        assert_eq!(b64, "body{color:red}");
    }

    #[test]
    fn 无命中时返回原_html() {
        let html = "<html><body>x</body></html>";
        assert_eq!(apply_fetched_styles(html, "https://a.test/", &[]), html);
    }
}
