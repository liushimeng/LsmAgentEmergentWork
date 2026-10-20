//! 混合内容拦截：HTTPS 页面禁止加载 HTTP 资源。
//!
//! 阶段四：现代浏览器安全基线——HTTPS 页面中的脚本/样式/iframe 等
//! 主动混合内容必须被拦截；图片/音视频等被动混合内容可配置。

/// 混合内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedContentType {
    /// 主动混合内容（脚本、样式、iframe、fetch 等）——必须拦截。
    Active,
    /// 被动混合内容（图片、音视频、字体）——可配置。
    Passive,
}

/// 混合内容拦截结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedContentResult {
    /// 允许加载。
    Allowed,
    /// 拦截（主动混合内容）。
    Blocked,
    /// 警告（被动混合内容，可配置为允许）。
    Warned,
}

/// 判断是否为混合内容（HTTPS 页面加载 HTTP 资源）。
pub fn is_mixed_content(page_url: &str, resource_url: &str) -> bool {
    let page_https = page_url.starts_with("https://");
    let resource_http = resource_url.starts_with("http://");
    page_https && resource_http
}

/// 根据内容类型与配置判断是否拦截。
pub fn check_mixed_content(
    page_url: &str,
    resource_url: &str,
    content_type: MixedContentType,
    allow_passive: bool,
) -> MixedContentResult {
    if !is_mixed_content(page_url, resource_url) {
        return MixedContentResult::Allowed;
    }
    match content_type {
        MixedContentType::Active => MixedContentResult::Blocked,
        MixedContentType::Passive => {
            if allow_passive {
                MixedContentResult::Warned
            } else {
                MixedContentResult::Blocked
            }
        }
    }
}

/// 从 Content-Type 推断混合内容类型。
pub fn content_type_from_mime(mime: &str) -> MixedContentType {
    let mime = mime.to_ascii_lowercase();
    if mime.starts_with("text/css")
        || mime.starts_with("text/javascript")
        || mime.starts_with("application/javascript")
        || mime.starts_with("text/html")
    {
        MixedContentType::Active
    } else if mime.starts_with("image/")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime.starts_with("font/")
    {
        MixedContentType::Passive
    } else {
        MixedContentType::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_mixed_content_https页面http资源() {
        assert!(is_mixed_content(
            "https://example.com/",
            "http://example.com/style.css"
        ));
    }

    #[test]
    fn is_mixed_content_同源https() {
        assert!(!is_mixed_content(
            "https://example.com/",
            "https://example.com/style.css"
        ));
    }

    #[test]
    fn is_mixed_content_http页面() {
        assert!(!is_mixed_content(
            "http://example.com/",
            "http://example.com/style.css"
        ));
    }

    #[test]
    fn check_mixed_content_主动拦截() {
        let result = check_mixed_content(
            "https://example.com/",
            "http://example.com/script.js",
            MixedContentType::Active,
            false,
        );
        assert_eq!(result, MixedContentResult::Blocked);
    }

    #[test]
    fn check_mixed_content_被动允许() {
        let result = check_mixed_content(
            "https://example.com/",
            "http://example.com/img.png",
            MixedContentType::Passive,
            true,
        );
        assert_eq!(result, MixedContentResult::Warned);
    }

    #[test]
    fn check_mixed_content_被动拦截() {
        let result = check_mixed_content(
            "https://example.com/",
            "http://example.com/img.png",
            MixedContentType::Passive,
            false,
        );
        assert_eq!(result, MixedContentResult::Blocked);
    }

    #[test]
    fn content_type_from_mime_css() {
        assert_eq!(
            content_type_from_mime("text/css"),
            MixedContentType::Active
        );
    }

    #[test]
    fn content_type_from_mime_js() {
        assert_eq!(
            content_type_from_mime("application/javascript"),
            MixedContentType::Active
        );
    }

    #[test]
    fn content_type_from_mime_img() {
        assert_eq!(
            content_type_from_mime("image/png"),
            MixedContentType::Passive
        );
    }

    #[test]
    fn content_type_from_mime_font() {
        assert_eq!(
            content_type_from_mime("font/ttf"),
            MixedContentType::Passive
        );
    }
}
