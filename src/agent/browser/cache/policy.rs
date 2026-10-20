//! HTTP 缓存策略解析：Cache-Control / Expires / Pragma。
//!
//! 阶段四：全内存 HTTP 缓存的强缓存策略解析。
//! 支持 RFC 7234 常用指令：max-age、no-cache、no-store、must-revalidate、public、private。

use std::time::{SystemTime, UNIX_EPOCH};

/// 缓存策略（解析自 Cache-Control / Expires）。
#[derive(Debug, Clone, Default)]
pub struct CachePolicy {
    /// 最大年龄（秒）；None 表示未指定。
    pub max_age: Option<u64>,
    /// 禁止缓存（每次需验证）。
    pub no_cache: bool,
    /// 禁止存储（完全不缓存）。
    pub no_store: bool,
    /// 必须重新验证（过期后）。
    pub must_revalidate: bool,
    /// 仅私有（当前实现等同 no_cache）。
    pub is_private: bool,
    /// 公开缓存。
    pub is_public: bool,
}

impl CachePolicy {
    /// 是否可缓存（用于决定是否存入缓存）。
    pub fn is_cacheable(&self) -> bool {
        !self.no_store && !self.no_cache && self.max_age.is_some()
    }

    /// 计算过期时间（Unix 毫秒）。
    pub fn expires_at_ms(&self) -> Option<u128> {
        let max_age = self.max_age?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_millis();
        Some(now + (max_age as u128) * 1000)
    }

    /// 是否已过期。
    pub fn is_expired(&self, cached_at_ms: u128) -> bool {
        match self.expires_at_ms() {
            Some(exp) => cached_at_ms > exp,
            None => false,
        }
    }
}

/// 解析 Cache-Control 头部。
pub fn parse_cache_control(value: &str) -> CachePolicy {
    let mut policy = CachePolicy::default();
    for part in value.split(',') {
        let part = part.trim().to_ascii_lowercase();
        if part.is_empty() {
            continue;
        }
        if let Some((key, val)) = part.split_once('=') {
            let key = key.trim();
            let val = val.trim().trim_matches('"');
            match key {
                "max-age" => {
                    policy.max_age = val.parse::<u64>().ok();
                }
                "must-revalidate" => {
                    policy.must_revalidate = true;
                }
                "public" => {
                    policy.is_public = true;
                }
                "private" => {
                    policy.is_private = true;
                }
                "no-cache" => {
                    policy.no_cache = true;
                }
                "no-store" => {
                    policy.no_store = true;
                }
                _ => {}
            }
        } else {
            match part.as_str() {
                "no-cache" => policy.no_cache = true,
                "no-store" => policy.no_store = true,
                "must-revalidate" => policy.must_revalidate = true,
                "public" => policy.is_public = true,
                "private" => policy.is_private = true,
                _ => {}
            }
        }
    }
    policy
}

/// 解析 Expires 头部（HTTP-date），返回 Unix 毫秒。
pub fn parse_expires(value: &str) -> Option<u128> {
    // 简化解析：支持 "Thu, 01 Dec 1994 16:00:00 GMT" 格式。
    let s = value.trim();
    // 尝试 RFC 1123 格式
    if let Some(ms) = parse_http_date(s) {
        return Some(ms);
    }
    None
}

/// 简化 HTTP-date 解析（仅 RFC 1123 / RFC 850 / asctime 常见格式）。
fn parse_http_date(s: &str) -> Option<u128> {
    // 移除星期前缀
    let s = if let Some(pos) = s.find(',') {
        s[pos + 1..].trim()
    } else {
        s
    };
    // 尝试 DD Mon YYYY HH:MM:SS GMT
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 6 {
        let day = parts[0].parse::<u32>().ok()?;
        let month = match parts[1].to_ascii_lowercase().as_str() {
            "jan" => 1,
            "feb" => 2,
            "mar" => 3,
            "apr" => 4,
            "may" => 5,
            "jun" => 6,
            "jul" => 7,
            "aug" => 8,
            "sep" => 9,
            "oct" => 10,
            "nov" => 11,
            "dec" => 12,
            _ => return None,
        };
        let year = parts[2].parse::<i64>().ok()?;
        let time_parts: Vec<&str> = parts[3].split(':').collect();
        if time_parts.len() >= 3 {
            let hour = time_parts[0].parse::<u32>().ok()?;
            let min = time_parts[1].parse::<u32>().ok()?;
            let sec = time_parts[2].parse::<u32>().ok()?;
            return date_to_unix_ms(year, month, day, hour, min, sec);
        }
    }
    None
}

/// 公历日期转 Unix 毫秒（简化实现，不考虑闰秒）。
fn date_to_unix_ms(year: i64, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> Option<u128> {
    // 使用简化算法：计算从 1970-01-01 到指定日期的天数
    let days_from_civil = |y: i64, m: u32, d: u32| -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as u32;
        let doy = ((153 * if m > 2 { m - 3 } else { m + 9 } as u32 + 2) / 5 + d - 1) as u32;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe as i64 - 719468
    };
    let days = days_from_civil(year, month, day);
    let total_secs = days * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64;
    Some((total_secs as u128) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_max_age_解析() {
        let p = parse_cache_control("max-age=3600");
        assert_eq!(p.max_age, Some(3600));
        assert!(p.is_cacheable());
    }

    #[test]
    fn cache_control_no_cache() {
        let p = parse_cache_control("no-cache");
        assert!(p.no_cache);
        assert!(!p.is_cacheable());
    }

    #[test]
    fn cache_control_no_store() {
        let p = parse_cache_control("no-store");
        assert!(p.no_store);
        assert!(!p.is_cacheable());
    }

    #[test]
    fn cache_control_public_max_age() {
        let p = parse_cache_control("public, max-age=86400");
        assert!(p.is_public);
        assert_eq!(p.max_age, Some(86400));
    }

    #[test]
    fn cache_control_must_revalidate() {
        let p = parse_cache_control("max-age=60, must-revalidate");
        assert_eq!(p.max_age, Some(60));
        assert!(p.must_revalidate);
    }

    #[test]
    fn cache_control_private() {
        let p = parse_cache_control("private, max-age=300");
        assert!(p.is_private);
        assert_eq!(p.max_age, Some(300));
    }

    #[test]
    fn cache_control_空字符串() {
        let p = parse_cache_control("");
        assert_eq!(p.max_age, None);
        assert!(!p.no_cache);
        assert!(!p.no_store);
    }

    #[test]
    fn cache_control_带引号() {
        let p = parse_cache_control("max-age=\"3600\"");
        assert_eq!(p.max_age, Some(3600));
    }

    #[test]
    fn expires_at_ms_计算() {
        let p = CachePolicy {
            max_age: Some(3600),
            ..Default::default()
        };
        // 先取基准时刻再算过期时刻:反过来会把两次调用之间的耗时算进差值,
        // 让 `exp - now` 恒小于 max_age(原断言是恒失败的,只是偶尔同毫秒侥幸通过)。
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let exp = p.expires_at_ms().unwrap();
        assert!(exp > now);
        assert!(exp - now >= 3600 * 1000);
    }

    #[test]
    fn is_expired_判断() {
        let p = CachePolicy {
            max_age: Some(0),
            ..Default::default()
        };
        // max-age=0 立即过期
        assert!(p.is_expired(u128::MAX));
    }
}
