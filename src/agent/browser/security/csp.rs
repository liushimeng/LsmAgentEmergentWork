//! CSP（内容安全策略）解析与执行。
//!
//! 阶段四：解析 CSP 头部，校验脚本/样式/图片等资源是否被允许加载。
//! 支持 script-src、style-src、img-src、connect-src、default-src 指令。
//! 支持 'self'、'unsafe-inline'、'unsafe-eval'、host 来源、通配符。

use std::collections::HashMap;

/// CSP 策略。
#[derive(Debug, Clone, Default)]
pub struct CspPolicy {
    /// 指令 → 来源列表。
    directives: HashMap<String, Vec<String>>,
}

impl CspPolicy {
    /// 解析 CSP 头部（支持多个指令用分号分隔）。
    pub fn parse(header: &str) -> Self {
        let mut directives = HashMap::new();
        for part in header.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let tokens: Vec<&str> = part.split_whitespace().collect();
            if tokens.is_empty() {
                continue;
            }
            let directive = tokens[0].to_ascii_lowercase();
            let sources: Vec<String> = tokens[1..].iter().map(|s| s.to_string()).collect();
            directives.insert(directive, sources);
        }
        Self { directives }
    }

    /// 获取指定指令的来源列表。
    pub fn get(&self, directive: &str) -> Option<&Vec<String>> {
        self.directives.get(directive)
    }

    /// 检查脚本是否被允许（script-src）。
    pub fn is_script_allowed(&self, url: &str, is_inline: bool) -> bool {
        self.is_allowed("script-src", url, is_inline)
    }

    /// 检查样式是否被允许（style-src）。
    pub fn is_style_allowed(&self, url: &str, is_inline: bool) -> bool {
        self.is_allowed("style-src", url, is_inline)
    }

    /// 检查图片是否被允许（img-src）。
    pub fn is_img_allowed(&self, url: &str) -> bool {
        self.is_allowed("img-src", url, false)
    }

    /// 检查连接是否被允许（connect-src）。
    pub fn is_connect_allowed(&self, url: &str) -> bool {
        self.is_allowed("connect-src", url, false)
    }

    /// 通用检查：指令 → default-src 回退。
    fn is_allowed(&self, directive: &str, url: &str, is_inline: bool) -> bool {
        let sources = self
            .get(directive)
            .or_else(|| self.get("default-src"));
        let sources = match sources {
            Some(s) => s,
            None => return true, // 无策略 = 允许
        };
        if sources.is_empty() {
            return false;
        }
        for src in sources {
            match src.as_str() {
                "'none'" => return false,
                "'self'" => {
                    if url.starts_with("about:") || url.starts_with("data:") {
                        return true;
                    }
                }
                "'unsafe-inline'" => {
                    if is_inline {
                        return true;
                    }
                }
                "'unsafe-eval'" => {
                    // 仅影响 eval，不影响资源加载
                }
                "*" => return true, // 通配符（不带引号）
                _ => {
                    // host 来源匹配（简化：前缀匹配,带 scheme 的源走这条）
                    if url.starts_with(src) {
                        return true;
                    }
                    let host_of_url = url::Url::parse(url).ok().and_then(|u| {
                        u.host_str().map(|h| h.to_ascii_lowercase())
                    });
                    // 裸主机源(无 scheme,CSP 语义 = 任意 scheme 的该主机):
                    // `url.starts_with(src)` 对 `https://` 前缀 URL 恒 false,
                    // 站点 CSP 里的 `www.googletagmanager.com` 等裸主机全部
                    // 误判为被阻止(实测猎聘 googletagmanager 脚本假告警)。
                    if let Some(host) = &host_of_url {
                        let src_lower = src.to_ascii_lowercase();
                        let src_bare = src_lower
                            .strip_prefix("https://")
                            .or_else(|| src_lower.strip_prefix("http://"))
                            .unwrap_or(&src_lower);
                        let src_host = src_bare.split('/').next().unwrap_or(src_bare);
                        if host == src_host {
                            return true;
                        }
                    }
                    // 通配符 *.example.com（匹配 host 后缀，防止 example.com.evil.com 误匹配）
                    if let Some(domain) = src.strip_prefix("*.") {
                        if let Some(host) = &host_of_url {
                            let domain = domain.to_ascii_lowercase();
                            if host == &domain || host.ends_with(&format!(".{domain}")) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// 是否报告模式（Content-Security-Policy-Report-Only）。
    pub fn is_report_only(header: &str) -> bool {
        header.to_ascii_lowercase().contains("report-only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csp_parse_基本() {
        let policy = CspPolicy::parse("default-src 'self'; script-src 'self' https://cdn.com");
        assert!(policy.get("default-src").is_some());
        assert!(policy.get("script-src").is_some());
    }

    #[test]
    fn csp_script_allowed_self() {
        let policy = CspPolicy::parse("script-src 'self'");
        assert!(policy.is_script_allowed("about:blank", false));
        assert!(!policy.is_script_allowed("https://cdn.com/app.js", false));
    }

    #[test]
    fn csp_script_allowed_unsafe_inline() {
        let policy = CspPolicy::parse("script-src 'unsafe-inline'");
        assert!(policy.is_script_allowed("", true));
        assert!(!policy.is_script_allowed("https://cdn.com/app.js", false));
    }

    #[test]
    fn csp_script_allowed_host() {
        let policy = CspPolicy::parse("script-src https://cdn.com");
        assert!(policy.is_script_allowed("https://cdn.com/app.js", false));
        assert!(!policy.is_script_allowed("https://other.com/app.js", false));
    }

    #[test]
    fn csp_script_allowed_none() {
        let policy = CspPolicy::parse("script-src 'none'");
        assert!(!policy.is_script_allowed("about:blank", false));
        assert!(!policy.is_script_allowed("", true));
    }

    #[test]
    fn csp_default_src_回退() {
        let policy = CspPolicy::parse("default-src 'self'");
        // 未指定 script-src，回退到 default-src
        assert!(policy.is_script_allowed("about:blank", false));
        assert!(!policy.is_script_allowed("https://cdn.com/app.js", false));
    }

    #[test]
    fn csp_img_allowed() {
        let policy = CspPolicy::parse("img-src *");
        assert!(policy.is_img_allowed("https://any.com/img.png"));
    }

    #[test]
    fn csp_connect_allowed() {
        let policy = CspPolicy::parse("connect-src https://api.com");
        assert!(policy.is_connect_allowed("https://api.com/data"));
        assert!(!policy.is_connect_allowed("https://other.com/data"));
    }

    #[test]
    fn csp_report_only() {
        assert!(CspPolicy::is_report_only("Content-Security-Policy-Report-Only"));
        assert!(!CspPolicy::is_report_only("Content-Security-Policy"));
    }

    #[test]
    fn csp_无策略允许所有() {
        let policy = CspPolicy::parse("");
        assert!(policy.is_script_allowed("https://any.com/app.js", false));
    }

    #[test]
    fn csp_通配符域名() {
        let policy = CspPolicy::parse("script-src *.example.com");
        assert!(policy.is_script_allowed("https://cdn.example.com/app.js", false));
        assert!(!policy.is_script_allowed("https://example.com.evil.com/app.js", false));
    }
}

#[test]
fn csp_裸主机源跨scheme匹配() {
    // 猎聘实测:CSP script-src 含裸主机 `www.googletagmanager.com`,
    // 旧实现 url.starts_with(src) 对 https:// 前缀 URL 恒 false → 假告警。
    let policy = CspPolicy::parse("script-src 'self' www.googletagmanager.com *.baidu.com");
    assert!(
        policy.is_script_allowed("https://www.googletagmanager.com/gtag/js?id=X", false),
        "裸主机源应匹配该主机的 https URL"
    );
    assert!(
        policy.is_script_allowed("https://hm.baidu.com/hm.js?a=1", false),
        "*.baidu.com 应匹配 hm.baidu.com"
    );
    assert!(
        !policy.is_script_allowed("https://evil.com/x.js", false),
        "未列出的主机仍应被阻止"
    );
}
