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
    // 未设环境变量:工具参数里的 hidden 视为「模型没意见」,由交互性判定决定。
    // 非 TTY(测试进程)下交互默认 = hidden,所以仍是 hidden。
    assert_eq!(
        BrowserMode::from_env_or_arg(Some("hidden")),
        BrowserMode::Hidden,
        "模型选的 hidden 不再压过交互式默认(测试进程非 TTY → hidden)"
    );
    assert_eq!(
        BrowserMode::from_env_or_arg(Some("headed")),
        BrowserMode::Headed,
        "模型显式要 headed 仍然照办"
    );
    assert_eq!(
        BrowserMode::from_env_or_arg(Some("new_headless")),
        BrowserMode::Hidden
    );
    assert_eq!(BrowserMode::from_env_or_arg(None), BrowserMode::Hidden);
}
