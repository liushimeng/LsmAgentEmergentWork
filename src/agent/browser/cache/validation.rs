//! HTTP 协商缓存：ETag / Last-Modified / If-None-Match / If-Modified-Since。
//!
//! 阶段四：强缓存过期后，用协商缓存验证资源是否变化。
//! 支持 ETag（强/弱验证器）与 Last-Modified（弱验证器）。

/// 协商缓存验证结果。
#[derive(Debug, Clone)]
pub enum ValidationResult {
    /// 资源未变化，可直接使用缓存（304）。
    NotModified,
    /// 资源已变化，需使用新响应（200）。
    Modified,
}

/// 协商缓存验证器。
#[derive(Debug, Clone, Default)]
pub struct Validator {
    /// ETag 值（含弱标记前缀 "W/"）。
    pub etag: Option<String>,
    /// Last-Modified 值（HTTP-date）。
    pub last_modified: Option<String>,
}

impl Validator {
    /// 从响应头构建验证器。
    pub fn from_headers(headers: &[(String, String)]) -> Self {
        let mut v = Self::default();
        for (k, val) in headers {
            if k.eq_ignore_ascii_case("etag") {
                v.etag = Some(val.clone());
            } else if k.eq_ignore_ascii_case("last-modified") {
                v.last_modified = Some(val.clone());
            }
        }
        v
    }

    /// 是否有任何验证器。
    pub fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }

    /// 生成 If-None-Match 头值。
    pub fn if_none_match(&self) -> Option<String> {
        self.etag.clone()
    }

    /// 生成 If-Modified-Since 头值。
    pub fn if_modified_since(&self) -> Option<String> {
        self.last_modified.clone()
    }

    /// 比较两个 ETag 是否匹配（忽略弱标记）。
    pub fn etag_matches(&self, other: &str) -> bool {
        if let Some(etag) = &self.etag {
            let a = etag.trim_start_matches("W/").trim();
            let b = other.trim_start_matches("W/").trim();
            a == b
        } else {
            false
        }
    }

    /// 比较 Last-Modified 是否变化（字符串精确比较）。
    pub fn modified_since(&self, other: &str) -> bool {
        match &self.last_modified {
            Some(lm) => lm.trim() != other.trim(),
            None => true,
        }
    }
}

/// 判断 304 响应是否可用于更新缓存。
pub fn can_update_cache(status: u16, headers: &[(String, String)]) -> bool {
    if status != 304 {
        return false;
    }
    // 304 响应应包含 ETag 或 Last-Modified
    let v = Validator::from_headers(headers);
    !v.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validator_from_headers_解析() {
        let headers = vec![
            ("etag".to_string(), "\"abc123\"".to_string()),
            ("last-modified".to_string(), "Thu, 01 Dec 1994 16:00:00 GMT".to_string()),
        ];
        let v = Validator::from_headers(&headers);
        assert_eq!(v.etag.as_deref(), Some("\"abc123\""));
        assert!(v.last_modified.is_some());
    }

    #[test]
    fn validator_空() {
        let v = Validator::default();
        assert!(v.is_empty());
    }

    #[test]
    fn validator_非空() {
        let v = Validator {
            etag: Some("\"abc\"".to_string()),
            ..Default::default()
        };
        assert!(!v.is_empty());
    }

    #[test]
    fn etag_matches_相同() {
        let v = Validator {
            etag: Some("\"abc\"".to_string()),
            ..Default::default()
        };
        assert!(v.etag_matches("\"abc\""));
    }

    #[test]
    fn etag_matches_弱验证器() {
        let v = Validator {
            etag: Some("W/\"abc\"".to_string()),
            ..Default::default()
        };
        assert!(v.etag_matches("\"abc\""));
        assert!(v.etag_matches("W/\"abc\""));
    }

    #[test]
    fn etag_matches_不同() {
        let v = Validator {
            etag: Some("\"abc\"".to_string()),
            ..Default::default()
        };
        assert!(!v.etag_matches("\"xyz\""));
    }

    #[test]
    fn modified_since_相同() {
        let v = Validator {
            last_modified: Some("Thu, 01 Dec 1994 16:00:00 GMT".to_string()),
            ..Default::default()
        };
        assert!(!v.modified_since("Thu, 01 Dec 1994 16:00:00 GMT"));
    }

    #[test]
    fn modified_since_不同() {
        let v = Validator {
            last_modified: Some("Thu, 01 Dec 1994 16:00:00 GMT".to_string()),
            ..Default::default()
        };
        assert!(v.modified_since("Fri, 02 Dec 1994 16:00:00 GMT"));
    }

    #[test]
    fn can_update_cache_304() {
        let headers = vec![("etag".to_string(), "\"abc\"".to_string())];
        assert!(can_update_cache(304, &headers));
    }

    #[test]
    fn can_update_cache_200() {
        let headers = vec![("etag".to_string(), "\"abc\"".to_string())];
        assert!(!can_update_cache(200, &headers));
    }

    #[test]
    fn can_update_cache_304_无验证器() {
        let headers = vec![];
        assert!(!can_update_cache(304, &headers));
    }

    #[test]
    fn if_none_match_值() {
        let v = Validator {
            etag: Some("\"abc\"".to_string()),
            ..Default::default()
        };
        assert_eq!(v.if_none_match().as_deref(), Some("\"abc\""));
    }

    #[test]
    fn if_modified_since_值() {
        let v = Validator {
            last_modified: Some("Thu, 01 Dec 1994 16:00:00 GMT".to_string()),
            ..Default::default()
        };
        assert_eq!(
            v.if_modified_since().as_deref(),
            Some("Thu, 01 Dec 1994 16:00:00 GMT")
        );
    }
}
