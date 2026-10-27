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

#[test]
fn page_owner_判活与指针判等() {
    // 第 181 轮:所有权状态机——拥有者 Activity 存活时 alive/is_current 成立,
    // Drop 后变孤儿(alive=false),允许后续 close/收养。
    let act = crate::agent::web_activity::begin("Test-Agent", "s1", "任务", 8);
    let owner = PageOwner {
        agent: "Test-Agent".into(),
        started_ms: act.snapshot().started_ms,
        weak: act.inner_weak(),
    };
    assert!(owner.alive(), "Activity 存活时 owner 应活跃");
    let caller = crate::agent::web_activity::current_weak();
    // 无 task_local 作用域(本测试不在 Agent 任务树内):caller=None,is_current 恒 false
    assert!(!owner.is_current(caller.as_ref()));

    // 模拟并行单元:两个不同 Activity 的弱引用指针不相等
    let act2 = crate::agent::web_activity::begin("Other-Agent", "s1", "任务", 8);
    let owner2 = PageOwner {
        agent: "Other-Agent".into(),
        started_ms: 0,
        weak: act2.inner_weak(),
    };
    assert!(std::sync::Weak::ptr_eq(
        &owner2.weak,
        &act2.inner_weak()
    ));
    assert!(!std::sync::Weak::ptr_eq(&owner.weak, &owner2.weak));

    // Drop 拥有者:弱引用死亡 → 孤儿页
    drop(act);
    drop(act2);
    assert!(!owner.alive(), "Activity Drop 后 owner 应死亡(孤儿页)");
    assert!(!owner2.alive());
}
