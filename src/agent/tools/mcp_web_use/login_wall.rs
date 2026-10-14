//! 登录墙识别(第 172 轮):把「服务端把页面 302 到登录页」变成结构化事实。
//!
//! 背景(BOSS 直聘实测,见
//! `tmpPlan/20261014_164500_BOSS直聘页面自刷新与反爬识别治理方案.md`):
//! 未登录访问 `https://www.zhipin.com/web/geek/job?query=AI&city=…` 被站点
//! **302 到 `/web/user/`(登录页)**,而 `control(navigate)` 当时只回报请求 URL,
//! 模型完全看不见落地变化,连续 3 轮试探才自行发现「要登录」;SEO 类目页被 302
//! 回首页时同理 —— Agent 把首页推荐位卡片当成职位列表抓取。
//!
//! 判据分两路(任一命中即可,URL 路权重更高):
//! - URL 路:路径 / 查询串出现登录语义 token;
//! - DOM 路:存在密码框或典型登录按钮文案,且**没有**任何会话标记。
//!
//! 纯函数 + 单测,不硬编码任何站点域名。

use serde_json::{json, Value};

/// URL 登录语义 token(小写匹配)。
const URL_TOKENS: &[&str] = &[
    "/login",
    "/signin",
    "/sign-in",
    "/signup",
    "/sign-up",
    "/passport",
    "/web/user",
    "/user/login",
    "/account/login",
    "/authorize",
    "/auth/login",
    "/session/new",
    "/sso",
    "/verify/login",
    "redirect=",
];

/// DOM 会话标记(命中即认为「已登录态」,压制 DOM 路判定)。
///
/// 形态说明:既接受 `login_dom_probe` 产出的标记名(`session_logout` 等),
/// 也接受原始 HTML 片段 —— 故这里用**子串**判定(类名/属性名在两者里都成立)。
/// 注意:早期版本误把 CSS 选择器字面量当子串去匹配 HTML,永远匹配不上
/// (选择器 `a[href*='logout']` 不是 HTML 文本),已由单测锁死。
const SESSION_MARKERS: &[&str] = &[
    "session_logout",
    "session_avatar",
    "logout",
    "user-info",
    "userinfo",
    "user_info",
    "avatar",
    "data-uid",
    "退出登录",
    "退出",
];

/// 登录按钮文案片段。
const LOGIN_TEXTS: &[&str] = &["登录/注册", "登录注册", "立即登录", "登录", "sign in", "log in"];

/// URL 路判定:返回命中的 token。
pub fn match_url_token(url: &str) -> Option<&'static str> {
    let lower = url.to_ascii_lowercase();
    // 查询串命中优先级高于路径,避免 `redirect=` 之外的噪声抢先。
    URL_TOKENS
        .iter()
        .find(|t| lower.contains(**t))
        .copied()
}

/// DOM 路判定:命中返回命中的标记文案。
pub fn match_dom_marker(dom: &str) -> Option<&'static str> {
    let lower = dom.to_ascii_lowercase();
    if SESSION_MARKERS.iter().any(|m| lower.contains(m)) {
        return None;
    }
    let has_password = lower.contains("type=\"password\"")
        || lower.contains("type='password'")
        || lower.contains("type=password");
    if has_password {
        return Some("password_input");
    }
    LOGIN_TEXTS
        .iter()
        .find(|t| lower.contains(&t.to_ascii_lowercase()))
        .copied()
}

/// 综合判定。`dom_probe` 为可选的页面片段(调用方用 `querySelector` 判断式取,
/// 不塞整页 HTML,避免撑爆响应)。
pub fn detect(final_url: &str, dom_probe: Option<&str>) -> Option<Value> {
    let url_hit = match_url_token(final_url);
    let dom_hit = dom_probe.and_then(match_dom_marker);
    if url_hit.is_none() && dom_hit.is_none() {
        return None;
    }
    let evidence = match (url_hit, dom_hit) {
        (Some(u), Some(d)) => vec![format!("url_token:{u}"), format!("dom_marker:{d}")],
        (Some(u), None) => vec![format!("url_token:{u}")],
        (None, Some(d)) => vec![format!("dom_marker:{d}")],
        (None, None) => unreachable!(),
    };
    Some(json!({
        "login_wall": true,
        "login_url": final_url,
        "evidence": evidence,
        "next_action": "control(request_human, reason=\"login\") 让人工完成登录(短信/扫码/微信通常必须人工);\
                        若该域登录态无法在本实例建立,改 open(reuse_existing=true) 接管用户已登录的真实浏览器;\
                        不要反复 navigate 试探(服务端每次都把你弹回登录页,纯烧迭代)"
    }))
}

/// 附加到响应对象(未命中零噪声)。
pub fn attach(data: &mut Value, final_url: &str, dom_probe: Option<&str>) {
    if let Some(v) = detect(final_url, dom_probe) {
        data["login_wall"] = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url路命中登录页() {
        assert_eq!(match_url_token("https://www.zhipin.com/web/user/"), Some("/web/user"));
        // 同义路径按列表顺序命中(此处 `/login` 先于 `/account/login`);
        // 判据语义是「命中登录语义」,具体 token 不影响下游处置。
        assert_eq!(
            match_url_token("https://example.com/accounts/login?next=/x"),
            Some("/login")
        );
        assert_eq!(
            match_url_token("https://id.example.com/oauth/authorize?client_id=1"),
            Some("/authorize")
        );
    }

    #[test]
    fn url路正常页不误伤() {
        assert_eq!(match_url_token("https://www.zhipin.com/beijing/"), None);
        assert_eq!(match_url_token("https://x.test/jobs/ai?city=1"), None);
    }

    #[test]
    fn dom路有会话标记则压制() {
        let dom = "<a href='/logout'>退出</a><input type=\"password\">";
        assert_eq!(match_dom_marker(dom), None);
    }

    #[test]
    fn dom路命中密码框或登录文案() {
        assert_eq!(match_dom_marker("<input type='password'>"), Some("password_input"));
        assert_eq!(match_dom_marker("<button>登录/注册</button>"), Some("登录/注册"));
    }

    #[test]
    fn 综合判定_健康页零噪声() {
        assert!(detect("https://x.test/jobs", Some("<div class=\"job-card\">")).is_none());
        let v = detect("https://x.test/login", Some("<input type=password>")).unwrap();
        assert_eq!(v["login_wall"], json!(true));
        assert!(v["next_action"].as_str().unwrap().contains("request_human"));
    }

    #[test]
    fn attach_只在命中时写字段() {
        let mut d = json!({"url": "https://x.test/jobs"});
        attach(&mut d, "https://x.test/jobs", None);
        assert!(d.get("login_wall").is_none());
        attach(&mut d, "https://x.test/login", None);
        assert!(d.get("login_wall").is_some());
    }
}