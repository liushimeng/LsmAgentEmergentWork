//! `LAEW_BROWSER_MODE` 与工具 `mode` 参数的优先级契约。
//!
//! 独立成文件(独立进程)以避免与 lib 单测的进程内环境变量竞争。

use lsm_agent::agent::browser::BrowserMode;

#[test]
fn 模式解析_显式环境变量优先于工具参数() {
    // 用户显式 LAEW_BROWSER_MODE=headed = 「我要看着浏览器跑」,压过 LLM 选的 hidden。
    std::env::set_var("LAEW_BROWSER_MODE", "headed");
    assert_eq!(
        BrowserMode::from_env_or_arg(Some("hidden")),
        BrowserMode::Headed,
        "显式 headed 必须压过工具选的 hidden(否则用户看不到窗口)"
    );
    std::env::set_var("LAEW_BROWSER_MODE", "hidden");
    assert_eq!(
        BrowserMode::from_env_or_arg(Some("headed")),
        BrowserMode::Hidden,
        "显式 hidden 同样压过工具选的 headed"
    );
    std::env::remove_var("LAEW_BROWSER_MODE");
    // 未设环境变量:以工具参数为准。
    assert_eq!(BrowserMode::from_env_or_arg(Some("hidden")), BrowserMode::Hidden);
    assert_eq!(BrowserMode::from_env_or_arg(Some("headed")), BrowserMode::Headed);
    assert_eq!(
        BrowserMode::from_env_or_arg(Some("new_headless")),
        BrowserMode::Hidden
    );
}
