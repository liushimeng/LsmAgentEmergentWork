//! CORS 跨域资源共享校验。
//!
//! 阶段四：校验跨域请求是否被目标源允许。
//! 支持简单请求与预检请求（OPTIONS）的响应头解析。

use std::collections::HashSet;

/// CORS 校验结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorsResult {
    /// 允许跨域访问。
    Allowed,
    /// 不允许跨域访问（附原因）。
    Denied(String),
    /// 同源请求（无需 CORS 校验）。
    SameOrigin,
}

/// CORS 响应头集合。
#[derive(Debug, Clone, Default)]
pub struct CorsHeaders {
    /// Access-Control-Allow-Origin。
    pub allow_origin: Option<String>,
    /// Access-Control-Allow-Methods。
    pub allow_methods: Option<Vec<String>>,
    /// Access-Control-Allow-Headers。
    pub allow_headers: Option<Vec<String>>,
    /// Access-Control-Allow-Credentials。
    pub allow_credentials: bool,
    /// Access-Control-Max-Age（预检缓存秒数）。
    pub max_age: Option<u64>,
}

impl CorsHeaders {
    /// 从响应头解析 CORS 头。
    pub fn from_headers(headers: &[(String, String)]) -> Self {
        let mut cors = Self::default();
        for (k, v) in headers {
            if k.eq_ignore_ascii_case("access-control-allow-origin") {
                cors.allow_origin = Some(v.trim().to_string());
            } else if k.eq_ignore_ascii_case("access-control-allow-methods") {
                cors.allow_methods = Some(
                    v.split(',')
                        .map(|s| s.trim().to_ascii_uppercase())
                        .collect(),
                );
            } else if k.eq_ignore_ascii_case("access-control-allow-headers") {
                cors.allow_headers = Some(
                    v.split(',')
                        .map(|s| s.trim().to_ascii_lowercase())
                        .collect(),
                );
            } else if k.eq_ignore_ascii_case("access-control-allow-credentials") {
                cors.allow_credentials = v.trim().eq_ignore_ascii_case("true");
            } else if k.eq_ignore_ascii_case("access-control-max-age") {
                cors.max_age = v.trim().parse().ok();
            }
        }
        cors
    }

    /// 是否允许指定来源。
    pub fn allows_origin(&self, origin: &str) -> bool {
        match &self.allow_origin {
            Some(o) => o == "*" || o.eq_ignore_ascii_case(origin),
            None => false,
        }
    }

    /// 是否允许指定方法。
    pub fn allows_method(&self, method: &str) -> bool {
        match &self.allow_methods {
            Some(methods) => methods.iter().any(|m| *m == method.to_ascii_uppercase()),
            None => false,
        }
    }

    /// 是否允许指定请求头。
    pub fn allows_headers(&self, headers: &[String]) -> bool {
        match &self.allow_headers {
            Some(allowed) => {
                let allowed_set: HashSet<_> = allowed.iter().cloned().collect();
                headers.iter().all(|h| allowed_set.contains(&h.to_ascii_lowercase()))
            }
            None => false,
        }
    }
}

/// 校验简单跨域请求。
pub fn check_simple_request(
    request_origin: &str,
    request_method: &str,
    cors: &CorsHeaders,
    is_same_origin: bool,
) -> CorsResult {
    if is_same_origin {
        return CorsResult::SameOrigin;
    }
    if !cors.allows_origin(request_origin) {
        return CorsResult::Denied(format!(
            "Access-Control-Allow-Origin 不允许来源 {request_origin}"
        ));
    }
    if !cors.allows_method(request_method) {
        return CorsResult::Denied(format!(
            "Access-Control-Allow-Methods 不允许方法 {request_method}"
        ));
    }
    CorsResult::Allowed
}

/// 校验预检请求（OPTIONS）。
pub fn check_preflight_request(
    request_origin: &str,
    request_method: &str,
    request_headers: &[String],
    cors: &CorsHeaders,
    is_same_origin: bool,
) -> CorsResult {
    if is_same_origin {
        return CorsResult::SameOrigin;
    }
    if !cors.allows_origin(request_origin) {
        return CorsResult::Denied(format!(
            "预检:Access-Control-Allow-Origin 不允许来源 {request_origin}"
        ));
    }
    if !cors.allows_method(request_method) {
        return CorsResult::Denied(format!(
            "预检:Access-Control-Allow-Methods 不允许方法 {request_method}"
        ));
    }
    if !cors.allows_headers(request_headers) {
        return CorsResult::Denied("预检:Access-Control-Allow-Headers 不允许请求头".to_string());
    }
    CorsResult::Allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_headers() -> CorsHeaders {
        CorsHeaders {
            allow_origin: Some("https://example.com".to_string()),
            allow_methods: Some(vec!["GET".to_string(), "POST".to_string()]),
            allow_headers: Some(vec!["content-type".to_string()]),
            allow_credentials: false,
            max_age: None,
        }
    }

    #[test]
    fn cors_from_headers_解析() {
        let headers = vec![
            ("access-control-allow-origin".to_string(), "*".to_string()),
            (
                "access-control-allow-methods".to_string(),
                "GET, POST".to_string(),
            ),
        ];
        let cors = CorsHeaders::from_headers(&headers);
        assert_eq!(cors.allow_origin.as_deref(), Some("*"));
        assert_eq!(cors.allow_methods.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn cors_allows_origin_通配() {
        let cors = CorsHeaders {
            allow_origin: Some("*".to_string()),
            ..Default::default()
        };
        assert!(cors.allows_origin("https://any.com"));
    }

    #[test]
    fn cors_allows_origin_指定() {
        let cors = make_headers();
        assert!(cors.allows_origin("https://example.com"));
        assert!(!cors.allows_origin("https://other.com"));
    }

    #[test]
    fn cors_allows_method() {
        let cors = make_headers();
        assert!(cors.allows_method("GET"));
        assert!(cors.allows_method("POST"));
        assert!(!cors.allows_method("DELETE"));
    }

    #[test]
    fn cors_allows_headers() {
        let cors = make_headers();
        assert!(cors.allows_headers(&["content-type".to_string()]));
        assert!(!cors.allows_headers(&["x-custom".to_string()]));
    }

    #[test]
    fn check_simple_request_同源() {
        let cors = make_headers();
        let result = check_simple_request("https://a.com", "GET", &cors, true);
        assert_eq!(result, CorsResult::SameOrigin);
    }

    #[test]
    fn check_simple_request_允许() {
        let cors = make_headers();
        let result = check_simple_request("https://example.com", "GET", &cors, false);
        assert_eq!(result, CorsResult::Allowed);
    }

    #[test]
    fn check_simple_request_拒绝来源() {
        let cors = make_headers();
        let result = check_simple_request("https://other.com", "GET", &cors, false);
        assert!(matches!(result, CorsResult::Denied(_)));
    }

    #[test]
    fn check_preflight_request_允许() {
        let cors = make_headers();
        let result = check_preflight_request(
            "https://example.com",
            "POST",
            &["content-type".to_string()],
            &cors,
            false,
        );
        assert_eq!(result, CorsResult::Allowed);
    }

    #[test]
    fn check_preflight_request_拒绝方法() {
        let cors = make_headers();
        let result = check_preflight_request(
            "https://example.com",
            "DELETE",
            &[],
            &cors,
            false,
        );
        assert!(matches!(result, CorsResult::Denied(_)));
    }
}
