//! 全内存存储层:Cookie Jar、localStorage、sessionStorage。
//!
//! 无磁盘写入;`close(page_id="all")` 或进程退出即全部销毁。Cookie 解析/匹配
//! 采用 RFC6265 的常用子集,足以覆盖登录态与常见站点。

use serde_json::{json, Value};
use std::collections::HashMap;

use super::urlutil;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
    pub host_only: bool,
}

impl Cookie {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "value": self.value,
            "domain": self.domain,
            "path": self.path,
            "expires": self.expires,
            "httpOnly": self.http_only,
            "secure": self.secure,
            "sameSite": self.same_site,
        })
    }
}

/// 进程级 Cookie Jar。
#[derive(Debug, Default, Clone)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn set_from_headers(&mut self, url: &str, headers: &[(String, String)]) {
        let host = match urlutil::host_of(url) {
            Some(h) => h,
            None => return,
        };
        let req_path = url::Url::parse(url)
            .ok()
            .map(|u| u.path().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "/".to_string());
        for (k, v) in headers {
            if !k.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            if let Some(c) = parse_set_cookie(v, &host, &req_path) {
                self.upsert(c);
            }
        }
    }

    /// 写入 Cookie(显式 API 与响应解析共用)。
    pub fn upsert(&mut self, cookie: Cookie) {
        if let Some(old) = self.cookies.iter_mut().find(|c| {
            c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path
        }) {
            *old = cookie;
        } else {
            // 上限 3000 条,超出淘汰最早写入项。
            if self.cookies.len() >= 3000 {
                self.cookies.remove(0);
            }
            self.cookies.push(cookie);
        }
    }

    /// 构造请求头 `Cookie: a=b; c=d`。
    pub fn header_for(&self, url: &str) -> Option<String> {
        let host = urlutil::host_of(url)?;
        let path = url::Url::parse(url)
            .ok()
            .map(|u| u.path().to_string())
            .unwrap_or_else(|| "/".to_string());
        let secure = url.starts_with("https://");
        let mut parts = Vec::new();
        for c in &self.cookies {
            if !domain_matches(&host, &c.domain, c.host_only) {
                continue;
            }
            if !path_matches(&path, &c.path) {
                continue;
            }
            if c.secure && !secure {
                continue;
            }
            parts.push(format!("{}={}", c.name, c.value));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("; "))
        }
    }

    pub fn set_explicit(
        &mut self,
        name: &str,
        value: &str,
        domain: Option<&str>,
        path: Option<&str>,
        url: &str,
    ) {
        let host = urlutil::host_of(url).unwrap_or_else(|| "localhost".to_string());
        let domain = domain
            .map(|d| d.trim_start_matches('.').to_ascii_lowercase())
            .unwrap_or_else(|| host.clone());
        let host_only = domain == host;
        self.upsert(Cookie {
            name: name.to_string(),
            value: value.to_string(),
            domain,
            path: path.unwrap_or("/").to_string(),
            expires: None,
            secure: url.starts_with("https://"),
            http_only: false,
            same_site: None,
            host_only,
        });
    }

    pub fn delete(&mut self, name: &str, domain: Option<&str>) {
        let domain = domain.map(|d| d.trim_start_matches('.').to_ascii_lowercase());
        self.cookies.retain(|c| {
            if c.name != name {
                return true;
            }
            match &domain {
                Some(d) => c.domain != *d,
                None => false,
            }
        });
    }

    pub fn all(&self) -> &[Cookie] {
        &self.cookies
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }
}

fn parse_set_cookie(raw: &str, request_host: &str, request_path: &str) -> Option<Cookie> {
    let mut parts = raw.split(';');
    let first = parts.next()?.trim();
    let (name, value) = first.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut cookie = Cookie {
        name: name.to_string(),
        value: value.trim().to_string(),
        domain: request_host.to_ascii_lowercase(),
        path: default_path(request_path),
        expires: None,
        secure: false,
        http_only: false,
        same_site: None,
        host_only: true,
    };
    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        let (k, v) = match attr.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
            None => (attr.to_ascii_lowercase(), String::new()),
        };
        match k.as_str() {
            "domain" if !v.is_empty() => {
                let d = v.trim_start_matches('.').to_ascii_lowercase();
                if domain_matches(request_host, &d, false) {
                    cookie.domain = d;
                    cookie.host_only = false;
                }
            }
            "path" if !v.is_empty() => {
                cookie.path = if v.starts_with('/') { v } else { "/".to_string() };
            }
            "expires" => cookie.expires = Some(v),
            "max-age" => cookie.expires = Some(format!("max-age={v}")),
            "secure" => cookie.secure = true,
            "httponly" => cookie.http_only = true,
            "samesite" => cookie.same_site = Some(v),
            _ => {}
        }
    }
    Some(cookie)
}

fn default_path(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".to_string();
    }
    match request_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => request_path[..i].to_string(),
    }
}

fn domain_matches(host: &str, domain: &str, host_only: bool) -> bool {
    let host = host.to_ascii_lowercase();
    let domain = domain.to_ascii_lowercase();
    if host == domain {
        return true;
    }
    if host_only {
        return false;
    }
    host.ends_with(&format!(".{domain}"))
}

fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    cookie_path.ends_with('/') || request_path[cookie_path.len()..].starts_with('/')
}

/// 标签页级存储(localStorage 的 origin 命名空间由管理器提供)。
#[derive(Debug, Default, Clone)]
pub struct TabStorage {
    pub session: HashMap<String, String>,
}

impl TabStorage {
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        for (k, v) in &self.session {
            map.insert(k.clone(), Value::String(v.clone()));
        }
        Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 会话_cookie_可回读() {
        let mut jar = CookieJar::default();
        jar.set_from_headers(
            "https://a.test/login",
            &[("set-cookie".into(), "sid=abc; Path=/; HttpOnly; Secure".into())],
        );
        assert_eq!(jar.header_for("https://a.test/x").as_deref(), Some("sid=abc"));
        assert!(jar.header_for("http://a.test/x").is_none());
    }

    #[test]
    fn domain_cookie_对子域生效() {
        let mut jar = CookieJar::default();
        jar.set_from_headers(
            "https://www.a.test/",
            &[("Set-Cookie".into(), "k=v; Domain=.a.test; Path=/".into())],
        );
        assert!(jar.header_for("https://m.a.test/").is_some());
        assert!(jar.header_for("https://b.test/").is_none());
    }

    #[test]
    fn path_前缀按段匹配() {
        assert!(path_matches("/a/b", "/a"));
        assert!(path_matches("/a/b", "/a/"));
        assert!(!path_matches("/ab", "/a"));
    }

    #[test]
    fn 删除指定_cookie() {
        let mut jar = CookieJar::default();
        jar.set_explicit("x", "1", None, None, "https://a.test/");
        jar.delete("x", None);
        assert!(jar.header_for("https://a.test/").is_none());
    }
}
