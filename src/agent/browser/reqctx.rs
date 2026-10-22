//! 浏览器请求头合成层(阶段四第二步补齐)。
//!
//! 真实浏览器的每一个子资源请求都带一整套「浏览器指纹头」:
//! `User-Agent` / `Referer` / `Origin` / `Accept` / `Accept-Language` /
//! `Sec-Fetch-Site` / `Sec-Fetch-Mode` / `Sec-Fetch-Dest`。
//! 内核此前只发 `User-Agent` + `Cookie`,**缺 `Referer`** ——
//! 大量站点的风控(百度 heicha/acs、猎聘、天猫 baxia)对「无 Referer 的
//! 子资源/接口请求」直接 403 或回 HTML 挑战页,页面脚本拿到非预期响应后
//! 抛 `TypeError: cannot convert 'null' or 'undefined' to object`,
//! 整条交互链路(路线规划/搜索建议/下单)静默失效。
//!
//! 本模块按 Chrome 语义合成这些头,供 `net::HttpClient` 与
//! `script::runtime::blocking_fetch`(页面 XHR/fetch)两条通道共用。
//! 合成头是**兜底**:调用方显式给出的同名头优先,不被覆盖。

use super::urlutil;

/// 请求目的地(决定 `Accept` 与 `Sec-Fetch-*` 的取值)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dest {
    /// 主文档导航(顶层/iframe 导航)。
    Document,
    /// `<script src>` 外链脚本。
    Script,
    /// `<link rel=stylesheet>` 样式表。
    Style,
    /// `<img>` / CSS 背景图。
    Image,
    /// `fetch` / `XMLHttpRequest`。
    Xhr,
    /// 字体文件。
    Font,
    /// 其他(favicon、manifest 等)。
    Other,
}

impl Dest {
    /// `Sec-Fetch-Dest` 取值。
    pub fn sec_fetch_dest(self) -> &'static str {
        match self {
            Dest::Document => "document",
            Dest::Script => "script",
            Dest::Style => "style",
            Dest::Image => "image",
            Dest::Xhr => "empty",
            Dest::Font => "font",
            Dest::Other => "empty",
        }
    }

    /// `Sec-Fetch-Mode` 取值(跨源 CORS 请求为 `cors`,同源为 `same-origin`)。
    pub fn sec_fetch_mode(self, cross_origin: bool) -> &'static str {
        match self {
            Dest::Document => "navigate",
            Dest::Xhr => {
                if cross_origin {
                    "cors"
                } else {
                    "same-origin"
                }
            }
            _ => "no-cors",
        }
    }

    /// `Accept` 取值(与 Chrome 一致)。
    pub fn accept(self) -> &'static str {
        match self {
            Dest::Document => {
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"
            }
            Dest::Style => "text/css,*/*;q=0.1",
            Dest::Image => "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
            Dest::Font => "*/*",
            _ => "*/*",
        }
    }
}

/// 一次子资源请求的上下文(发起页面 + 目的地 + 方法)。
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// 发起请求的页面 URL(主文档导航时为空)。
    pub page_url: String,
    /// 目的地。
    pub dest: Dest,
    /// 页面 UA(空则用内核默认 UA)。
    pub user_agent: String,
    /// `Accept-Language`。
    pub accept_language: String,
    /// 请求方法(决定是否发 `Origin`)。
    pub method: String,
}

impl RequestContext {
    /// 子资源请求上下文(带 Referer)。
    pub fn subresource(page_url: &str, dest: Dest, user_agent: &str) -> Self {
        Self {
            page_url: page_url.to_string(),
            dest,
            user_agent: user_agent.to_string(),
            accept_language: accept_language(),
            method: "GET".to_string(),
        }
    }

    /// 覆盖请求方法(`Origin` 的发送条件与之相关)。
    pub fn with_method(mut self, method: &str) -> Self {
        self.method = method.to_ascii_uppercase();
        self
    }

    /// 是否跨源(相对发起页面)。
    pub fn is_cross_origin(&self, target_url: &str) -> bool {
        if self.page_url.is_empty() {
            return false;
        }
        !urlutil::same_origin(&self.page_url, target_url)
    }

    /// `Sec-Fetch-Site` 取值。
    fn sec_fetch_site(&self, target_url: &str) -> &'static str {
        if self.page_url.is_empty() {
            return "none";
        }
        if urlutil::same_origin(&self.page_url, target_url) {
            return "same-origin";
        }
        // 同站(同一 registrable domain)判定:取 host 的后两段做粗粒比较。
        let page_host = urlutil::host_of(&self.page_url).unwrap_or_default();
        let target_host = urlutil::host_of(target_url).unwrap_or_default();
        if eTLD(&page_host) == eTLD(&target_host) && !page_host.is_empty() {
            "same-site"
        } else {
            "cross-site"
        }
    }

    /// `Referer` 取值:跨源只发 origin(Chrome `strict-origin-when-cross-origin`)。
    fn referer(&self, target_url: &str) -> Option<String> {
        if self.page_url.is_empty() {
            return None;
        }
        if !self.page_url.starts_with("http://") && !self.page_url.starts_with("https://") {
            return None;
        }
        if self.is_cross_origin(target_url) {
            urlutil::origin_of(&self.page_url)
        } else {
            Some(urlutil::strip_fragment(&self.page_url))
        }
    }

    /// `Origin` 取值:CORS 模式请求(跨源,或非 GET/HEAD 方法)发送。
    fn origin(&self, target_url: &str) -> Option<String> {
        if !matches!(self.dest, Dest::Xhr) {
            return None;
        }
        if self.is_cross_origin(target_url) || !matches!(self.method.as_str(), "GET" | "HEAD") {
            urlutil::origin_of(&self.page_url)
        } else {
            None
        }
    }

    /// 合成请求头(不含 Cookie;Cookie 由 `net` 层单独注入)。
    ///
    /// 返回的头**都是兜底**:调用方显式提供的同名头优先。
    pub fn headers(&self, target_url: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let ua = if self.user_agent.is_empty() {
            super::DEFAULT_UA
        } else {
            self.user_agent.as_str()
        };
        out.push(("user-agent".to_string(), ua.to_string()));
        out.push(("accept".to_string(), self.dest.accept().to_string()));
        if !self.accept_language.is_empty() {
            out.push(("accept-language".to_string(), self.accept_language.clone()));
        }
        let cross = self.is_cross_origin(target_url);
        out.push((
            "sec-fetch-site".to_string(),
            self.sec_fetch_site(target_url).to_string(),
        ));
        out.push((
            "sec-fetch-mode".to_string(),
            self.dest.sec_fetch_mode(cross).to_string(),
        ));
        out.push((
            "sec-fetch-dest".to_string(),
            self.dest.sec_fetch_dest().to_string(),
        ));
        if let Some(r) = self.referer(target_url) {
            out.push(("referer".to_string(), r));
        }
        if let Some(o) = self.origin(target_url) {
            out.push(("origin".to_string(), o));
        }
        out
    }
}

/// 粗粒度 eTLD+1(取 host 末两段;`*.com.cn` 之类不细分,宁可判 same-site 也不误判 cross-site)。
fn eTLD(host: &str) -> String {
    let parts: Vec<&str> = host.split('.').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 {
        return parts.join(".");
    }
    parts[parts.len() - 2..].join(".")
}

/// `Accept-Language` 取值(可用 `LAEW_WEB_ACCEPT_LANGUAGE` 覆盖)。
pub fn accept_language() -> String {
    std::env::var("LAEW_WEB_ACCEPT_LANGUAGE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7".to_string())
}

/// 把合成头合并进请求头列表:已存在的同名头**不被覆盖**(调用方显式值优先)。
pub fn merge_headers(
    base: &[(String, String)],
    synthesized: &[(String, String)],
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = base.to_vec();
    for (k, v) in synthesized {
        if out.iter().any(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            continue;
        }
        out.push((k.clone(), v.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://map.baidu.com/index.html";

    fn ctx(dest: Dest) -> RequestContext {
        RequestContext::subresource(PAGE, dest, "TestUA/1.0")
    }

    #[test]
    fn 子资源请求带_referer_与_ua() {
        let h = ctx(Dest::Script).headers("https://map.baidu.com/a.js");
        let get = |k: &str| {
            h.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("referer").as_deref(), Some(PAGE));
        assert_eq!(get("user-agent").as_deref(), Some("TestUA/1.0"));
        assert_eq!(get("sec-fetch-dest").as_deref(), Some("script"));
        assert_eq!(get("sec-fetch-mode").as_deref(), Some("no-cors"));
        assert_eq!(get("sec-fetch-site").as_deref(), Some("same-origin"));
        assert_eq!(get("accept").as_deref(), Some("*/*"));
    }

    #[test]
    fn 跨源只发_origin_形式_referer() {
        let h = ctx(Dest::Image).headers("https://webmap0.bdimg.com/t.png");
        let get = |k: &str| {
            h.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("referer").as_deref(), Some("https://map.baidu.com"));
        assert_eq!(get("sec-fetch-site").as_deref(), Some("cross-site"));
        assert_eq!(get("sec-fetch-dest").as_deref(), Some("image"));
        assert!(get("origin").is_none(), "图片请求不发 Origin");
    }

    #[test]
    fn 同站不同子域判为_same_site() {
        let h = ctx(Dest::Image).headers("https://webmap0.bdimg.com/t.png");
        // map.baidu.com 与 bdimg.com 不同站
        assert!(h
            .iter()
            .any(|(n, v)| n == "sec-fetch-site" && v == "cross-site"));
        let h2 = RequestContext::subresource("https://a.example.com/", Dest::Script, "UA")
            .headers("https://b.example.com/x.js");
        assert!(h2
            .iter()
            .any(|(n, v)| n == "sec-fetch-site" && v == "same-site"));
    }

    #[test]
    fn 跨源_xhr_发_origin_同源不发() {
        let h = RequestContext::subresource(PAGE, Dest::Xhr, "UA")
            .with_method("GET")
            .headers("https://api.example.com/x");
        assert!(h
            .iter()
            .any(|(n, v)| n == "origin" && v == "https://map.baidu.com"));
        assert!(h
            .iter()
            .any(|(n, v)| n == "sec-fetch-mode" && v == "cors"));
        assert!(h
            .iter()
            .any(|(n, v)| n == "sec-fetch-dest" && v == "empty"));

        let h2 = RequestContext::subresource(PAGE, Dest::Xhr, "UA")
            .with_method("GET")
            .headers("https://map.baidu.com/api");
        assert!(!h2.iter().any(|(n, _)| n == "origin"));
        assert!(h2
            .iter()
            .any(|(n, v)| n == "sec-fetch-mode" && v == "same-origin"));
    }

    #[test]
    fn 显式头不被合成头覆盖() {
        let base = vec![("Referer".to_string(), "https://custom/".to_string())];
        let synth = ctx(Dest::Script).headers("https://map.baidu.com/a.js");
        let merged = merge_headers(&base, &synth);
        let r = merged
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("referer"))
            .map(|(_, v)| v.clone())
            .unwrap();
        assert_eq!(r, "https://custom/");
        // 合成头里其余项仍然补上
        assert!(merged.iter().any(|(n, v)| n == "sec-fetch-dest" && v == "script"));
    }

    #[test]
    fn 导航请求不带_referer() {
        let h = RequestContext::subresource("", Dest::Document, "UA")
            .headers("https://example.com/");
        assert!(!h.iter().any(|(n, _)| n == "referer"));
        assert!(h.iter().any(|(n, v)| n == "sec-fetch-site" && v == "none"));
        assert!(h
            .iter()
            .any(|(n, v)| n == "sec-fetch-mode" && v == "navigate"));
    }
}
