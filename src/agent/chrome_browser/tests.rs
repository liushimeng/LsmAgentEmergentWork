//! chrome_browser 单元测试(不拉起真实 Chrome)。

use super::*;

#[test]
fn 启动开关必须是裸名() {
    for headless in [false, true] {
        for f in custom_launch_flags(headless) {
            assert!(
                !f.starts_with('-'),
                "启动开关必须是裸名(否则被渲染成 ----xxx 被 Chrome 忽略): {f}"
            );
        }
    }
}

#[test]
fn 无头档追加容器兜底参数() {
    let headed = custom_launch_flags(false);
    let hidden = custom_launch_flags(true);
    assert!(!headed.iter().any(|f| f == "no-sandbox"));
    assert!(hidden.iter().any(|f| f == "no-sandbox"));
    assert!(hidden.iter().any(|f| f == "disable-dev-shm-usage"));
}

#[test]
fn 反检测与证书参数在基础集中() {
    let flags = custom_launch_flags(false);
    assert!(flags.iter().any(|f| f == "ignore-certificate-errors"));
    assert!(
        flags
            .iter()
            .any(|f| f == "disable-blink-features=AutomationControlled")
    );
}

#[test]
fn page_id_形如_c_加八位十六进制() {
    let mgr = ChromeManager::new();
    let id = mgr.next_page_id();
    assert!(id.starts_with("c_"));
    assert_eq!(id.len(), 10);
    let id2 = mgr.next_page_id();
    assert_ne!(id, id2);
}

#[test]
fn 模式解析_headed_hidden() {
    std::env::set_var("LAEW_CHROME_MODE", "headed");
    assert_eq!(ChromeMode::from_env_or_default(), ChromeMode::Headed);
    std::env::set_var("LAEW_CHROME_MODE", "hidden");
    assert_eq!(ChromeMode::from_env_or_default(), ChromeMode::Hidden);
    std::env::remove_var("LAEW_CHROME_MODE");
}
