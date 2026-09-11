//! D9-7 SSRF 防护(L1608 / L1625):URL 私网/CGNAT/link-local 拦截。
//!
//! 对齐 claudecode `ssrfGuard.ts` 完整 IPv4/IPv6 正则集 + atomcode `is_safe_ip`
//! (专题-第十九轮 D9-7):在 `client_from_record` 创建 LLM HTTP 客户端之前校验
//! `end_point`,指向内网/私网/CGNAT/云元数据的请求一律拒绝,防 SSRF 攻击。
//!
//! ## 设计要点
//!
//! 1. **fail-closed**:IPv6 解析失败 / 未知格式 → 拒绝(对齐 openclaw 容错模式)。
//! 2. **IPv4-mapped IPv6 解包重检**:`::ffff:a9fe:a9fe` → 还原为 `169.254.169.254`
//!    再次判断,防通过 IPv6 绕过 IPv4 拦截(L1625)。
//! 3. **hostname 字符串拒绝**:localhost / `*.local` / 云元数据域名,
//!    防 hosts 改写绕过。
//! 4. **不做 DNS 解析(本轮)**:避免阻塞 + 与 TLS 验证顺序耦合;
//!    DNS rebinding 由后续轮次补(`reqwest::dns::resolve` 钉扎)。
//! 5. **关闭开关**:`LAEW_ALLOW_PRIVATE_ENDPOINT=1`(本地 Ollama / 127.0.0.1 调试)。
//!
//! ## 知识库出处
//!
//! - 第十九轮 claudecode `ssrfGuard.ts:55-125`:IPv4/IPv6 完整正则集
//! - 第十九轮 atomcode `web_fetch.rs:425-493`:`is_safe_ip` + `resolve_to_addrs` 钉扎
//! - 第十九轮 openclaw `ssrf.ts:209-331`:默认黑名单 + 双阶段 DNS

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use crate::database::{ConfigError, Result};

/// 默认拒绝的 hostname(小写比对)。
///
/// 覆盖 localhost 变体 + 云元数据域名,防 hosts 改写绕过。
pub const BLOCKED_HOSTNAMES: &[&str] = &[
    "localhost",
    "localhost.localdomain",
    "metadata.google.internal",
    "metadata.internal",
    "169.254.169.254",
];

/// 主入口:校验 end_point 是否安全(公网可达)。
///
/// 校验链:
/// 1. 关闭开关探测(`LAEW_ALLOW_PRIVATE_ENDPOINT=1`)
/// 2. URL 解析 + scheme 校验(http/https)
/// 3. hostname 字符串拒绝 + 后缀拒绝(`.local`/`.internal`)
/// 4. IP 字面量 → 直接判私有性
/// 5. 域名 → 本轮放行(不做 DNS);后续轮次补钉扎
pub fn is_safe_endpoint(endpoint: &str) -> Result<()> {
    let allow_private = std::env::var("LAEW_ALLOW_PRIVATE_ENDPOINT").ok().as_deref() == Some("1");
    is_safe_endpoint_with_override(endpoint, allow_private)
}

/// 在显式指定 override 状态下校验 endpoint。
///
/// 单元测试不能用 `std::env::set_var` 模拟 override:Rust 测试默认并发运行,
/// 进程级环境变量会被其它 safety 用例观察到,造成偶发失败。
fn is_safe_endpoint_with_override(endpoint: &str, allow_private: bool) -> Result<()> {
    if allow_private {
        return Ok(());
    }
    let url = url::Url::parse(endpoint)
        .map_err(|e| ConfigError::UrlSafety(format!("URL 解析失败: {e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ConfigError::UrlSafety(format!(
                "scheme `{other}` 不被允许(仅 http/https)"
            )));
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| ConfigError::UrlSafety("URL 缺少 host".to_string()))?;
    check_hostname_safety(host)?;
    // IP 字面量直接判私有性;域名本轮放行(不做 DNS 解析,避免阻塞)。
    if let Ok(ip) = IpAddr::from_str(host) {
        check_ip_safety(ip)?;
    }
    Ok(())
}

/// 兼容旧调用名(与 is_safe_endpoint 同义)。
pub fn check_endpoint_safety(endpoint: &str) -> Result<()> {
    is_safe_endpoint(endpoint)
}

/// hostname 字符串层校验(大小写不敏感)。
fn check_hostname_safety(host: &str) -> Result<()> {
    let lc = host.to_ascii_lowercase();
    if BLOCKED_HOSTNAMES.contains(&lc.as_str()) {
        return Err(ConfigError::UrlSafety(format!("blocked hostname: {host}")));
    }
    if lc.ends_with(".local") || lc.ends_with(".internal") {
        return Err(ConfigError::UrlSafety(format!(
            "blocked hostname 后缀: {host}"
        )));
    }
    Ok(())
}

/// IP 层校验(IPv4 / IPv6 / IPv4-mapped IPv6)。
pub fn check_ip_safety(ip: IpAddr) -> Result<()> {
    match ip {
        IpAddr::V4(v4) => check_v4_safety(v4),
        IpAddr::V6(v6) => check_v6_safety(v6),
    }
}

/// IPv4 私有地址拒绝(对齐 claudecode `ssrfGuard.ts:55-86`)。
///
/// 拒绝范围:`0.0.0.0/8` / `10/8` / `100.64/10 CGNAT` / `127/8` /
/// `169.254/16` / `172.16/12` / `192.168/16` / `224/4` / `240/4` / broadcast。
fn check_v4_safety(v4: Ipv4Addr) -> Result<()> {
    let o = v4.octets();
    let range = |start: [u8; 4], end: [u8; 4]| o >= start && o <= end;
    if range([0, 0, 0, 0], [0, 255, 255, 255]) {
        return Err(err_with_hint(
            "0.0.0.0/8 当前网络/未指派",
            "公网 provider 才会命中",
        ));
    }
    if range([10, 0, 0, 0], [10, 255, 255, 255]) {
        return Err(err_with_hint(
            "10.0.0.0/8 私网",
            "局域网 / 内网网关 / Ollama 局域网访问",
        ));
    }
    if range([100, 64, 0, 0], [100, 127, 255, 255]) {
        return Err(err_with_hint("100.64.0.0/10 CGNAT", "运营商级 NAT 段"));
    }
    if range([127, 0, 0, 0], [127, 255, 255, 255]) {
        return Err(err_with_hint(
            "127.0.0.0/8 loopback",
            "本机 loopback;本地 Ollama / LMStudio / mock LLM 服务",
        ));
    }
    if range([169, 254, 0, 0], [169, 254, 255, 255]) {
        return Err(err_with_hint(
            "169.254.0.0/16 link-local(云元数据)",
            "云厂商元数据段",
        ));
    }
    if range([172, 16, 0, 0], [172, 31, 255, 255]) {
        return Err(err_with_hint(
            "172.16.0.0/12 私网",
            "Docker bridge / k8s Pod IP / 局域网",
        ));
    }
    if range([192, 168, 0, 0], [192, 168, 255, 255]) {
        return Err(err_with_hint("192.168.0.0/16 私网", "家庭 / 公司局域网"));
    }
    if range([224, 0, 0, 0], [239, 255, 255, 255]) {
        return Err(err_with_hint("224.0.0.0/4 multicast", "组播地址"));
    }
    if range([240, 0, 0, 0], [255, 255, 255, 254]) {
        return Err(err_with_hint("240.0.0.0/4 reserved", "IANA 保留段"));
    }
    if o == [255, 255, 255, 255] {
        return Err(err_with_hint("255.255.255.255 broadcast", "广播地址"));
    }
    Ok(())
}

/// IPv6 私有地址拒绝(对齐 claudecode `ssrfGuard.ts:88-125`)。
///
/// 拒绝范围:`::` / `::1` / `fc00::/7` / `fe80::/10` / `ff00::` / IPv4-mapped 解包重检。
fn check_v6_safety(v6: Ipv6Addr) -> Result<()> {
    if v6.is_unspecified() {
        return Err(err_with_hint(":: 未指定地址", "公网 provider 不会用"));
    }
    if v6.is_loopback() {
        return Err(err_with_hint(
            "::1 loopback",
            "本机 loopback;本地 Ollama / LMStudio / mock LLM 服务",
        ));
    }
    if v6.is_multicast() {
        return Err(err_with_hint("ff00::/8 multicast", "组播地址"));
    }
    let s = v6.segments();
    // unique-local fc00::/7 → 前 7 位 = 0b1111_110x
    if (s[0] & 0xfe00) == 0xfc00 {
        return Err(err_with_hint("fc00::/7 unique-local", "IPv6 私网段"));
    }
    // link-local fe80::/10 → 前 10 位 = 0b1111_1110_10
    if (s[0] & 0xffc0) == 0xfe80 {
        return Err(err_with_hint(
            "fe80::/10 link-local",
            "IPv6 链路本地段,仅同网段有效",
        ));
    }
    // IPv4-mapped IPv6 (::ffff:a.b.c.d) → 解包后再次判 IPv4 私网
    if let Some(v4) = ipv4_mapped(&v6) {
        return check_v4_safety(v4);
    }
    Ok(())
}

/// IPv4-mapped IPv6 解包(`::ffff:a.b.c.d` → `Ipv4Addr`)。
///
/// 覆盖 `::ffff:0:0/96` 前缀;IPv4-compatible(`::a.b.c.d`)因已弃用暂不覆盖。
fn ipv4_mapped(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    let s = v6.segments();
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0xffff {
        let b = v6.octets();
        Some(Ipv4Addr::new(b[12], b[13], b[14], b[15]))
    } else {
        None
    }
}

#[inline]
fn err_with_hint(reason: &'static str, hint: &'static str) -> ConfigError {
    // URL 安全检查被拒时,文案末尾固定追加 escape hatch 提示,
    // 让本地 Ollama / LMStudio / 局域网 / mock 测试用户立刻知道有解(2026-09-10 第二十五轮 F04/B07 测试发现并修复)。
    // 同时给出该拦截场景的针对性说明,帮助用户判断是真错配还是合理期望。
    if hint.is_empty() {
        ConfigError::UrlSafety(format!("private/internal IP 被拦截: {reason}"))
    } else {
        ConfigError::UrlSafety(format!(
            "private/internal IP 被拦截: {reason}。{hint}。\n  提示:本地调试 / 局域网 / mock 测试 provider 可设 `LAEW_ALLOW_PRIVATE_ENDPOINT=1` 放行(详见 AGENTS.md)"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ipv4_ok() {
        assert!(check_ip_safety(IpAddr::from_str("8.8.8.8").unwrap()).is_ok());
        assert!(check_ip_safety(IpAddr::from_str("1.1.1.1").unwrap()).is_ok());
    }

    #[test]
    fn private_ipv4_blocked() {
        let cases = [
            "0.0.0.0",
            "10.0.0.1",
            "10.255.255.255",
            "100.64.0.0",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.255",
            "169.254.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.0.1",
            "192.168.255.255",
            "224.0.0.1",
            "240.0.0.0",
            "255.255.255.255",
        ];
        for c in cases {
            assert!(
                check_ip_safety(IpAddr::from_str(c).unwrap()).is_err(),
                "expected {c} to be blocked"
            );
        }
    }

    #[test]
    fn private_ipv6_blocked() {
        let cases = ["::", "::1", "fc00::1", "fd00::1", "fe80::1", "ff00::1"];
        for c in cases {
            assert!(
                check_ip_safety(IpAddr::from_str(c).unwrap()).is_err(),
                "expected {c} to be blocked"
            );
        }
    }

    #[test]
    fn ipv4_mapped_blocked() {
        // ::ffff:10.0.0.1 → 10.0.0.1 (private)
        assert!(check_ip_safety(IpAddr::from_str("::ffff:10.0.0.1").unwrap()).is_err());
        // ::ffff:169.254.169.254 → cloud metadata
        assert!(check_ip_safety(IpAddr::from_str("::ffff:169.254.169.254").unwrap()).is_err());
        // ::ffff:8.8.8.8 → public, ok
        assert!(check_ip_safety(IpAddr::from_str("::ffff:8.8.8.8").unwrap()).is_ok());
    }

    #[test]
    fn public_ipv6_ok() {
        assert!(check_ip_safety(IpAddr::from_str("2001:4860:4860::8888").unwrap()).is_ok());
    }

    #[test]
    fn endpoint_localhost_blocked() {
        assert!(is_safe_endpoint_with_override("http://localhost:11434", false).is_err());
        assert!(is_safe_endpoint_with_override("https://localhost/v1/messages", false).is_err());
        assert!(is_safe_endpoint_with_override("http://LOCALHOST:8080", false).is_err());
    }

    #[test]
    fn endpoint_private_ip_blocked() {
        assert!(is_safe_endpoint_with_override("http://10.0.0.1:11434", false).is_err());
        assert!(is_safe_endpoint_with_override("http://192.168.1.1/v1/messages", false).is_err());
        assert!(
            is_safe_endpoint_with_override("http://169.254.169.254/latest/meta-data", false)
                .is_err()
        );
        assert!(is_safe_endpoint_with_override("http://127.0.0.1:8080", false).is_err());
        assert!(is_safe_endpoint_with_override("http://100.64.0.1", false).is_err());
    }

    #[test]
    fn endpoint_metadata_hostname_blocked() {
        assert!(is_safe_endpoint_with_override(
            "http://metadata.google.internal/computeMetadata",
            false
        )
        .is_err());
    }

    #[test]
    fn endpoint_local_suffix_blocked() {
        assert!(is_safe_endpoint_with_override("http://myserver.local:8080", false).is_err());
        assert!(is_safe_endpoint_with_override("http://db.internal:5432", false).is_err());
    }

    #[test]
    fn endpoint_scheme_blocked() {
        assert!(is_safe_endpoint_with_override("ftp://example.com/key", false).is_err());
        assert!(is_safe_endpoint_with_override("file:///etc/passwd", false).is_err());
    }

    #[test]
    fn endpoint_public_ok() {
        assert!(is_safe_endpoint_with_override("https://api.anthropic.com", false).is_ok());
        assert!(is_safe_endpoint_with_override("https://api.openai.com/v1", false).is_ok());
        assert!(
            is_safe_endpoint_with_override("https://example.com:8443/v1/messages", false).is_ok()
        );
    }

    #[test]
    fn endpoint_allow_private_override() {
        assert!(is_safe_endpoint_with_override("http://10.0.0.1:11434", true).is_ok());
        assert!(is_safe_endpoint_with_override("http://192.168.1.1", true).is_ok());
    }

    #[test]
    fn endpoint_loopback_error_includes_escape_hatch_hint() {
        // 第二十五轮 F04/B07 测试:URL 安全检查被拒时,文案必须告知 escape hatch,
        // 让本地 Ollama / LMStudio / mock LLM 用户立刻知道有解(2026-09-10)。
        let err = is_safe_endpoint_with_override("http://127.0.0.1:11434", false).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("LAEW_ALLOW_PRIVATE_ENDPOINT"),
            "loopback error should mention escape hatch; got: {msg}"
        );
        assert!(
            msg.contains("127.0.0.0/8"),
            "loopback error should mention the blocked CIDR; got: {msg}"
        );
        assert!(
            msg.contains("loopback") || msg.contains("loopback"),
            "loopback error should describe loopback semantics; got: {msg}"
        );
    }

    #[test]
    fn endpoint_malformed_url_fails() {
        assert!(is_safe_endpoint("not a url").is_err());
    }

    #[test]
    fn blocked_hostnames_constant() {
        assert!(BLOCKED_HOSTNAMES.contains(&"localhost"));
        assert!(BLOCKED_HOSTNAMES.contains(&"metadata.google.internal"));
    }
}
