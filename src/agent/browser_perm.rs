//! 浏览器原生权限弹窗自动处置(地理位置授权等)
//!
//! 背景:Chrome 原生权限气泡(地理位置/通知等)是浏览器 UI 层,不是页面 DOM —
//! blocker_probe(弹层结构打分)/inspect(blockers)/HITL 三条感知通道全部不可见;
//! 页面 JS 调 `navigator.geolocation` 挂起等待授权,会把 Agent 工具链拖住,
//! 人工点完授权才恢复(实测闲鱼任务:「弹窗人工选择后等了好长时间才继续」)。
//!
//! 对策(CDP 标准解法):导航完成后对页面 origin 执行 `Browser.setPermission`
//! 预授权 —— 弹窗永不出现(零等待零人工);对已弹出的 pending 弹窗调用 grant
//! 也会立即消解。处置档位见 `LAEW_WEB_GEO`:grant(默认)/deny/prompt(原生)。
//!
//! 铁律:**connect 模式(复用用户已登录 Chrome)不做自动授权** —— 用户浏览器
//! 的权限归用户做主,保持零注入原则;仅 laew 自启的专用 profile 自动处置。

use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::browser::{
    PermissionDescriptor, PermissionSetting, SetPermissionParams,
};
use serde_json::{json, Value};

/// 地理位置权限处置档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoMode {
    /// 预授权 granted:弹窗永不出现。
    Grant,
    /// 预置 denied:页面拿到 denied 自行降级(如闲鱼退回手动选地区)。
    Deny,
    /// 完全不干预,保持原生行为(旧语义,调试用)。
    Prompt,
}

/// `LAEW_WEB_GEO` 解析:deny 系 → Deny;prompt/ask/native → Prompt;
/// 未设置/非法值 → Grant(默认)。
pub fn geo_mode_from_env() -> GeoMode {
    match std::env::var("LAEW_WEB_GEO")
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("deny" | "0" | "false" | "no" | "off" | "denied") => GeoMode::Deny,
        Some("prompt" | "ask" | "native") => GeoMode::Prompt,
        _ => GeoMode::Grant,
    }
}

/// 从 URL 提取 origin(`scheme://host[:port]`)。
/// 仅 http/https;`data:`/`about:`/`chrome://`/相对地址等返回 None。
pub fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let origin = u.origin().ascii_serialization();
    if origin == "null" {
        None
    } else {
        Some(origin)
    }
}

/// 对给定 origin 应用地理位置权限设置。
/// Prompt 档直接跳过;CDP 调用失败仅返回 Err 文本,调用方 fail-open warn。
pub async fn apply_geo_permission(
    browser: &Browser,
    origin: &str,
    mode: GeoMode,
) -> Result<PermissionSetting, String> {
    let setting = match mode {
        GeoMode::Grant => PermissionSetting::Granted,
        GeoMode::Deny => PermissionSetting::Denied,
        GeoMode::Prompt => return Ok(PermissionSetting::Prompt),
    };
    let params = SetPermissionParams::builder()
        .permission(PermissionDescriptor::new("geolocation"))
        .setting(setting.clone())
        .origin(origin.to_string())
        .build()
        .map_err(|e| format!("构造 setPermission 参数失败:{e}"))?;
    browser.execute(params).await.map_err(|e| e.to_string())?;
    Ok(setting)
}

/// 处置结果摘要(供 open 响应 data.permissions 透出与日志)。
pub fn geo_outcome_json(mode: GeoMode, origin: Option<&str>, applied: Option<&str>) -> Value {
    let geolocation = match (mode, applied) {
        (GeoMode::Prompt, _) => "prompt",
        (_, Some("granted")) => "granted",
        (_, Some("denied")) => "denied",
        _ => "skipped",
    };
    json!({
        "geolocation": geolocation,
        "mode": match mode {
            GeoMode::Grant => "grant",
            GeoMode::Deny => "deny",
            GeoMode::Prompt => "prompt",
        },
        "origin": origin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geo_mode_env_parsing() {
        // 默认(未设置)
        std::env::remove_var("LAEW_WEB_GEO");
        assert_eq!(geo_mode_from_env(), GeoMode::Grant);
        // grant 系与大小写/空白
        for v in ["grant", "GRANT", " Grant "] {
            std::env::set_var("LAEW_WEB_GEO", v);
            assert_eq!(geo_mode_from_env(), GeoMode::Grant, "{v}");
        }
        // deny 系
        for v in ["deny", "DENIED", "0", "false", "off", "no"] {
            std::env::set_var("LAEW_WEB_GEO", v);
            assert_eq!(geo_mode_from_env(), GeoMode::Deny, "{v}");
        }
        // prompt 系
        for v in ["prompt", "Ask", "native"] {
            std::env::set_var("LAEW_WEB_GEO", v);
            assert_eq!(geo_mode_from_env(), GeoMode::Prompt, "{v}");
        }
        // 非法值回退默认
        std::env::set_var("LAEW_WEB_GEO", "whatever");
        assert_eq!(geo_mode_from_env(), GeoMode::Grant);
        std::env::remove_var("LAEW_WEB_GEO");
    }

    #[test]
    fn origin_extraction() {
        assert_eq!(
            origin_of("https://www.goofish.com/"),
            Some("https://www.goofish.com".to_string())
        );
        assert_eq!(
            origin_of("https://8.130.85.252:29003/Anthropic"),
            Some("https://8.130.85.252:29003".to_string())
        );
        assert_eq!(
            origin_of("http://localhost:8080/a?b=1"),
            Some("http://localhost:8080".to_string())
        );
        // 非 http(s)
        assert_eq!(origin_of("data:text/html,<p>x"), None);
        assert_eq!(origin_of("about:blank"), None);
        assert_eq!(origin_of("chrome://new-tab-page"), None);
        // 非法/相对地址
        assert_eq!(origin_of("not a url"), None);
        assert_eq!(origin_of("/relative/path"), None);
    }

    #[test]
    fn geo_outcome_shape() {
        let v = geo_outcome_json(GeoMode::Grant, Some("https://a.com"), Some("granted"));
        assert_eq!(v["geolocation"], "granted");
        assert_eq!(v["origin"], "https://a.com");
        let v = geo_outcome_json(GeoMode::Prompt, None, None);
        assert_eq!(v["geolocation"], "prompt");
        let v = geo_outcome_json(GeoMode::Grant, None, None);
        assert_eq!(v["geolocation"], "skipped");
        let v = geo_outcome_json(GeoMode::Deny, Some("https://a.com"), Some("denied"));
        assert_eq!(v["geolocation"], "denied");
    }
}
