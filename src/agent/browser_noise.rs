//! 浏览器噪声页识别(第 152 轮)。
//!
//! 实测事故(2026-10-10 豆包任务):headed 模式下 HITL 收口后浏览器前台冒出
//! 一批空白/乱码标签页,用户完全不知道它们从哪来。
//!
//! 机制:[`crate::agent::browser::BrowserManager::adopt_spawned_pages`] 对
//! 「浏览器里有、laew 注册表里没有」的 target **一律收编并留在前台**。页面自己
//! 开的 `about:blank`、被风控脚本 `window.open('')` 开的空窗、下载触发的
//! 导航残留,全部落进这个洞 —— 收编 ≠ 关闭,于是它们越积越多。
//!
//! 本模块只提供**纯判定**:哪些页是噪声(不值得收编、可以直接关掉)。
//! 真正的「等一下再判」时序在 `browser.rs` 侧(见 `adopt_spawned_pages` 注释):
//! `target=_blank` 刚打开时 URL 短暂为 `about:blank`,**必须**给它一次导航的
//! 时间,否则会把真页面误杀。

/// 是否是噪声 URL(空白页 / 浏览器内部页 / 开发者页)。
///
/// 判定只认**浏览器自身的占位页与内部 scheme**,不认任何真实站点 ——
/// 真实站点 URL 永远不会被判成噪声。
pub fn is_noise_url(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() {
        return true;
    }
    let lower = u.to_ascii_lowercase();
    // 空白页:Chrome 有 `about:blank` / `about:newtab`,Edge 有 `edge://newtab`
    if lower == "about:blank" || lower.starts_with("about:blank?") || lower == "about:newtab" {
        return true;
    }
    // 浏览器内部页(用户手动开的新标签页 / 下载页 / 崩溃页)
    for p in [
        "chrome://newtab",
        "chrome://new-tab-page",
        "chrome://search-local-ntp",
        "chrome://downloads",
        "chrome://crash",
        "chrome://version",
        "edge://newtab",
        "brave://newtab",
        "devtools://",
    ] {
        if lower.starts_with(p) {
            return true;
        }
    }
    false
}

/// 是否是**空噪声页**:噪声 URL 且没有标题。
///
/// 标题非空即放行 —— `about:blank` 窗口在导航完成后常常仍保留浏览器给的默认
/// 标题,这条豁免把误杀面压到最小;同时真实内容页(任何 URL)永远不会命中。
///
/// 例外:`chrome://new-tab-page` / `chrome://newtab` 等**新标签页**即使标题非空
/// (Chrome 给的默认标题是「新标签页」)也判为噪声。实测(2026-10-12 闲鱼任务):
/// 点击商品链接触发 `target=_blank` / `window.open` 时,新 tab 初始停在
/// `chrome://new-tab-page`,标题豁免让它混进候选页并被 adopt 为 `spawned_page_id`,
/// 后续操作全部打到空白新标签页 —— Agent 以为在操作商品页,实际在操作新标签页,
/// 表现为「点击按钮打开新页面后 Agent 失去控制」。新标签页没有可操作内容,
/// 标题豁免对它无意义,直接按 URL 判噪声。
pub fn is_noise_page(url: &str, title: &str) -> bool {
    if !is_noise_url(url) {
        return false;
    }
    let lower = url.trim().to_ascii_lowercase();
    // 新标签页:标题非空也判噪声(Chrome 默认标题「新标签页」会命中标题豁免)。
    if lower.starts_with("chrome://new-tab-page")
        || lower.starts_with("chrome://newtab")
        || lower.starts_with("edge://newtab")
        || lower.starts_with("brave://newtab")
        || lower.starts_with("chrome://search-local-ntp")
    {
        return true;
    }
    title.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_internal_pages_are_noise() {
        for u in [
            "",
            "about:blank",
            "about:blank?x=1",
            "about:newtab",
            "chrome://newtab/",
            "chrome://new-tab-page/",
            "chrome://search-local-ntp/",
            "edge://newtab",
            "devtools://devtools/bundled/inspector.html",
        ] {
            assert!(is_noise_url(u), "{u:?} 应判为噪声 URL");
        }
    }

    #[test]
    fn real_sites_never_noise() {
        for u in [
            "https://www.doubao.com/chat/",
            "https://www.doubao.com/",
            "http://localhost:8080/",
            "about:config",
            "chrome://extensions",
            "data:text/html,<h1>x</h1>",
            "blob:https://x.test/1234",
        ] {
            assert!(!is_noise_url(u), "{u:?} 不应判为噪声 URL");
        }
    }

    #[test]
    fn titled_noise_page_is_kept() {
        // 标题非空 → 豁免(避免误杀刚导航完、URL 还没刷新的窗口)
        assert!(!is_noise_page("about:blank", "豆包"));
        assert!(is_noise_page("about:blank", ""));
        assert!(is_noise_page("about:blank", "   "));
        // 真实站点永远不算噪声页
        assert!(!is_noise_page("https://www.doubao.com/", ""));
    }

    /// 新标签页例外(2026-10-12 闲鱼实测):标题非空(Chrome 默认「新标签页」)
    /// 也必须判噪声 —— 否则被 adopt 为 spawned_page_id,后续操作全打到空白页。
    #[test]
    fn newtab_page_is_noise_even_with_title() {
        assert!(is_noise_page("chrome://new-tab-page/", "新标签页"));
        assert!(is_noise_page("chrome://newtab", "新标签页"));
        assert!(is_noise_page("chrome://search-local-ntp/", "New Tab"));
        assert!(is_noise_page("edge://newtab", "New Tab"));
        // 对照:about:blank 标题非空仍豁免(只有 new-tab-page 系列例外)
        assert!(!is_noise_page("about:blank", "新标签页"));
    }

    #[test]
    fn case_and_space_insensitive() {
        assert!(is_noise_url("  ABOUT:BLANK  "));
        assert!(is_noise_url("Chrome://NewTab"));
    }
}
