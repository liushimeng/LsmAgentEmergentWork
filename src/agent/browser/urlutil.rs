//! URL 与文本编码辅助:LsmAgentBrowser 的纯函数层。
//!
//! 所有函数不依赖 DOM/网络,便于单元测试。设计见
//! `docs/MCP_Web_LsmAgentBrowser_Use/01-总体设计与架构/02-LsmAgentBrowser内核架构设计.md`。

use url::Url;

/// 解析用户输入或页面内相对地址。
///
/// 返回 `None` 表示无法解析(缺协议且无基准、非法 URL)。缺省补 `https://`。
pub fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if let Ok(u) = Url::parse(href) {
        return Some(u.to_string());
    }
    if let Ok(base) = Url::parse(base) {
        return base.join(href).ok().map(|u| u.to_string());
    }
    // 用户直接输入 `example.com/path` 时按 HTTPS 处理。
    Url::parse(&format!("https://{href}")).ok().map(|u| u.to_string())
}

/// 主机名(小写,去端口)。解析失败返回 `None`。
pub fn host_of(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

/// `scheme://host[:port]`,用于 localStorage 作用域与同源判断。
pub fn origin_of(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
    Some(format!("{}://{}{}", u.scheme(), host, port))
}

/// 两个 URL 是否同源。
pub fn same_origin(a: &str, b: &str) -> bool {
    match (origin_of(a), origin_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// 规范化页面 URL:去掉 fragment、统一小写 host,用于复用匹配。
pub fn canonical(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut u) => {
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => url.trim().to_string(),
    }
}

/// 去掉 fragment,保留其余部分(导航去重)。
pub fn strip_fragment(url: &str) -> String {
    canonical(url)
}

/// 是否为可导航协议。`about:blank` 允许(空页)。
pub fn is_navigable(url: &str) -> bool {
    match Url::parse(url) {
        Ok(u) => matches!(u.scheme(), "http" | "https" | "file" | "data" | "about"),
        Err(_) => false,
    }
}

/// 从 Content-Type/meta 探测字符集并解码 HTML/文本。
///
/// 顺序:HTTP 头 charset → `<meta charset>` → `<meta http-equiv>` → UTF-8。
/// 解码失败按 UTF-8 lossy,保证永远有可解析文本。
pub fn decode_body(bytes: &[u8], content_type: &str) -> String {
    let label = charset_from_content_type(content_type)
        .or_else(|| sniff_meta_charset(bytes))
        .unwrap_or_else(|| "utf-8".to_string());
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes())
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

fn charset_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let idx = lower.find("charset=")?;
    let raw = &content_type[idx + 8..];
    let raw = raw.split(';').next().unwrap_or(raw).trim();
    let raw = raw.trim_matches(|c| c == '"' || c == '\'');
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    // 只看前 4KB,避免大页面全量扫描。
    let head_len = bytes.len().min(4096);
    let head = String::from_utf8_lossy(&bytes[..head_len]).to_ascii_lowercase();
    if let Some(idx) = head.find("charset=") {
        let raw = &head[idx + 8..];
        let raw = raw
            .split(|c: char| c == '"' || c == '\'' || c == ';' || c == '>' || c.is_whitespace())
            .next()
            .unwrap_or("");
        if !raw.is_empty() {
            return Some(raw.to_string());
        }
    }
    None
}

/// 文件名净化:去掉路径分隔符与 Windows 保留字符,限制长度。
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned
        .split(|c| c == '/' || c == '\\' || c == '_')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("_");
    let cleaned = if cleaned.is_empty() {
        format!("download_{}", crate::agent::browser::now_ms())
    } else {
        cleaned
    };
    cleaned.chars().take(120).collect()
}

/// 从 URL 路径推断下载文件名。
pub fn filename_from_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.next_back().map(|x| x.to_string()))
        })
        .filter(|s| !s.is_empty())
        .map(|s| sanitize_filename(&s))
        .unwrap_or_else(|| format!("download_{}", crate::agent::browser::now_ms()))
}

/// 把 origin 作为存储命名空间(解析失败用 `about:blank`)。
pub fn storage_origin(url: &str) -> String {
    origin_of(url).unwrap_or_else(|| "about:blank".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 相对地址按基准解析() {
        assert_eq!(
            resolve("https://a.test/dir/page", "../x?q=1").as_deref(),
            Some("https://a.test/x?q=1")
        );
    }

    #[test]
    fn 裸域名补_https() {
        assert_eq!(resolve("", "example.com/a").as_deref(), Some("https://example.com/a"));
    }

    #[test]
    fn 同源判断含端口() {
        assert!(same_origin("https://a.test/x", "https://a.test/y"));
        assert!(!same_origin("https://a.test:8443/x", "https://a.test/y"));
        assert!(!same_origin("http://a.test/x", "https://a.test/x"));
    }

    #[test]
    fn 响应字符集优先于_meta() {
        let gbk = encoding_rs::GBK.encode("中文").0.into_owned();
        let text = decode_body(&gbk, "text/html; charset=gbk");
        assert_eq!(text, "中文");
    }

    #[test]
    fn 文件名净化拒绝路径穿越() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize_filename("a\\b:c.txt"), "a_b_c.txt");
    }

    #[test]
    fn 规范化去掉_fragment() {
        assert_eq!(canonical("https://A.test/p#x"), "https://a.test/p");
    }
}

/// JS 懒加载属性值归一(2026-10-30 京东):`data-lazyimg` 族常是 JSON
/// (`{"src":"..."}`),其余属性是裸 URL。JSON 形态宽松抽取 `src` 字段值,
/// 抽不出原样返回;调用方再做 URL 解析/校验。
pub fn lazy_attr_url(raw: &str) -> &str {
    let t = raw.trim();
    if !t.starts_with('{') {
        return t;
    }
    for key in ["\"src\"", "'src'"] {
        if let Some(pos) = t.find(key) {
            let rest = &t[pos + key.len()..];
            let rest = rest.trim_start().trim_start_matches(':').trim_start();
            if let Some(quote) = rest.chars().next() {
                if quote == '"' || quote == '\'' {
                    let rest2 = &rest[1..];
                    if let Some(end) = rest2.find(quote) {
                        let v = &rest2[..end];
                        if !v.is_empty() {
                            return v;
                        }
                    }
                }
            }
        }
    }
    t
}
