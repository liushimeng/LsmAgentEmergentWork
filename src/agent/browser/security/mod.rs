//! 安全策略执行上下文：同源策略、CORS、CSP、混合内容拦截。
//!
//! 阶段四：为页面脚本 fetch/XHR 与资源加载提供安全边界。
//! 模块分工：`mod.rs` 安全上下文入口；`cors.rs` CORS 校验；
//! `csp.rs` CSP 解析与执行；`mixed_content.rs` 混合内容拦截。

mod cors;
mod csp;
mod mixed_content;

pub use cors::{check_preflight_request, check_simple_request, CorsHeaders, CorsResult};
pub use csp::CspPolicy;
pub use mixed_content::{
    check_mixed_content, content_type_from_mime, is_mixed_content, MixedContentResult,
    MixedContentType,
};

/// 安全上下文：为单个标签页维护安全策略状态。
#[derive(Debug, Default, Clone)]
pub struct SecurityContext {
    /// 页面源（origin）。
    page_origin: Option<String>,
    /// CORS 响应头缓存（按 URL）。
    cors_cache: std::collections::HashMap<String, CorsHeaders>,
    /// CSP 策略（按 URL）。
    csp_cache: std::collections::HashMap<String, CspPolicy>,
    /// 是否允许被动混合内容。
    allow_passive_mixed: bool,
}

impl SecurityContext {
    /// 创建默认安全上下文。
    pub fn new() -> Self {
        Self {
            page_origin: None,
            cors_cache: std::collections::HashMap::new(),
            csp_cache: std::collections::HashMap::new(),
            allow_passive_mixed: false,
        }
    }

    /// 设置页面源。
    pub fn set_page_origin(&mut self, url: &str) {
        self.page_origin = url::Url::parse(url)
            .ok()
            .map(|u| u.origin().ascii_serialization());
    }

    /// 判断两个 URL 是否同源。
    pub fn is_same_origin(&self, other: &str) -> bool {
        match &self.page_origin {
            Some(origin) => {
                url::Url::parse(other)
                    .map(|u| u.origin().ascii_serialization() == *origin)
                    .unwrap_or(false)
            }
            None => false,
        }
    }

    /// 缓存 CORS 响应头。
    pub fn cache_cors(&mut self, url: &str, headers: CorsHeaders) {
        self.cors_cache.insert(url.to_string(), headers);
    }

    /// 获取缓存的 CORS 响应头。
    pub fn get_cors(&self, url: &str) -> Option<&CorsHeaders> {
        self.cors_cache.get(url)
    }

    /// 缓存 CSP 策略。
    pub fn cache_csp(&mut self, url: &str, policy: CspPolicy) {
        self.csp_cache.insert(url.to_string(), policy);
    }

    /// 获取缓存的 CSP 策略。
    pub fn get_csp(&self, url: &str) -> Option<&CspPolicy> {
        self.csp_cache.get(url)
    }

    /// 设置是否允许被动混合内容。
    pub fn set_allow_passive_mixed(&mut self, allow: bool) {
        self.allow_passive_mixed = allow;
    }

    /// 是否允许被动混合内容。
    pub fn allows_passive_mixed(&self) -> bool {
        self.allow_passive_mixed
    }

    /// 清空缓存。
    pub fn clear(&mut self) {
        self.cors_cache.clear();
        self.csp_cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_context_同源判断() {
        let mut ctx = SecurityContext::new();
        ctx.set_page_origin("https://example.com/page");
        assert!(ctx.is_same_origin("https://example.com/other"));
        assert!(!ctx.is_same_origin("https://other.com/page"));
        assert!(!ctx.is_same_origin("http://example.com/page"));
    }

    #[test]
    fn security_context_cors缓存() {
        let mut ctx = SecurityContext::new();
        let cors = CorsHeaders {
            allow_origin: Some("*".to_string()),
            ..Default::default()
        };
        ctx.cache_cors("https://api.com/data", cors);
        assert!(ctx.get_cors("https://api.com/data").is_some());
        assert!(ctx.get_cors("https://other.com/data").is_none());
    }

    #[test]
    fn security_context_csp缓存() {
        let mut ctx = SecurityContext::new();
        let csp = CspPolicy::parse("script-src 'self'");
        ctx.cache_csp("https://example.com/", csp);
        assert!(ctx.get_csp("https://example.com/").is_some());
    }

    #[test]
    fn security_context_混合内容配置() {
        let mut ctx = SecurityContext::new();
        assert!(!ctx.allows_passive_mixed());
        ctx.set_allow_passive_mixed(true);
        assert!(ctx.allows_passive_mixed());
    }

    #[test]
    fn security_context_clear() {
        let mut ctx = SecurityContext::new();
        ctx.cache_cors(
            "https://api.com/",
            CorsHeaders {
                allow_origin: Some("*".to_string()),
                ..Default::default()
            },
        );
        ctx.cache_csp("https://example.com/", CspPolicy::parse("script-src 'self'"));
        ctx.clear();
        assert!(ctx.get_cors("https://api.com/").is_none());
        assert!(ctx.get_csp("https://example.com/").is_none());
    }

    #[test]
    fn is_same_origin_无页面源() {
        let ctx = SecurityContext::new();
        assert!(!ctx.is_same_origin("https://example.com/"));
    }
}
