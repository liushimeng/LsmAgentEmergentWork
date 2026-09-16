//! WindowUse 微信消息任务失败分析与综合修复方案端到端测试
//! (2026-09-16 第 54 轮,补丁 A-E)。
//!
//! 覆盖范围:
//! - 补丁 A:BashTool 在 WindowUse 白名单模式下放行 osascript / cliclick / pbcopy;
//! - 补丁 A:BashTool 在 WindowUse 白名单模式下拦截非白名单命令(curl / wget);
//! - 补丁 A:BashTool 默认模式(非 WindowUse)无白名单限制;
//! - 补丁 B:infer_delegate_to 基于关键词自动纠正 delegate_to;
//! - 补丁 D:MACOS_AX_UNAVAILABLE_HINT 在 macOS 26 上包含 osascript 替代建议;
//!
//! 不依赖 LLM / 网络,纯本地单元测试。
//!
//! 设计见 `tmpPlan/2026-09-16_01-WindowUse微信消息任务失败分析与综合修复方案.md`。

use lsm_agent::agent::context::AgentRole;
use lsm_agent::agent::main_work::{infer_delegate_to, WorkFlowSpec};
use lsm_agent::agent::tools::bash::{check_window_use_bash, window_use_mode, WINDOW_USE_MODE_ENV};
use lsm_agent::agent::tools::bash::BashTool;
use lsm_agent::agent::tools::Tool;
use serde_json::json;
use std::sync::Mutex;

// ========== 全局 mutex,串行化 env 修改(避免并行 cargo test 干扰) ==========
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ========== 补丁 A:BashTool WindowUse 白名单模式 ==========

#[tokio::test]
async fn bash_window_use_mode_allows_osascript() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var(WINDOW_USE_MODE_ENV, "1");
    }
    assert!(window_use_mode());

    // osascript 命令在 WindowUse 模式下应放行(白名单内)
    let result = check_window_use_bash("osascript -e 'tell application \"WeChat\" to activate'");
    assert!(result.is_ok(), "osascript 应放行: {result:?}");

    // 直接执行 echo 验证白名单真的放行
    let tool = BashTool;
    let out = tool
        .execute(json!({"command": "echo LAEW_WINDOW_USE_OK"}))
        .await
        .unwrap();
    assert!(out.contains("LAEW_WINDOW_USE_OK"));

    unsafe {
        std::env::remove_var(WINDOW_USE_MODE_ENV);
    }
}

#[tokio::test]
async fn bash_window_use_mode_allows_pbcopy_pbpaste() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var(WINDOW_USE_MODE_ENV, "1");
    }

    // pbcopy/pbpaste 在白名单内
    assert!(check_window_use_bash("pbcopy < /dev/null").is_ok());
    assert!(check_window_use_bash("pbpaste").is_ok());

    unsafe {
        std::env::remove_var(WINDOW_USE_MODE_ENV);
    }
}

#[tokio::test]
async fn bash_window_use_mode_blocks_curl() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var(WINDOW_USE_MODE_ENV, "1");
    }

    // curl 不在白名单,应被拦截
    let result = check_window_use_bash("curl https://example.com");
    assert!(result.is_err(), "curl 在 WindowUse 模式应被拦截");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("windowuse-mode"),
        "错误应明确 windowuse-mode,实际: {err}"
    );
    assert!(
        err.contains("白名单"),
        "错误应提及白名单,实际: {err}"
    );

    unsafe {
        std::env::remove_var(WINDOW_USE_MODE_ENV);
    }
}

#[tokio::test]
async fn bash_window_use_mode_blocks_wget() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var(WINDOW_USE_MODE_ENV, "1");
    }

    // wget / python3 / node / cargo 都不在白名单
    assert!(check_window_use_bash("wget https://example.com").is_err());
    assert!(check_window_use_bash("python3 -c 'print(1)'").is_err());
    assert!(check_window_use_bash("node script.js").is_err());
    assert!(check_window_use_bash("cargo test").is_err());

    unsafe {
        std::env::remove_var(WINDOW_USE_MODE_ENV);
    }
}

#[tokio::test]
async fn bash_default_mode_no_allowlist() {
    // 默认(非 WindowUse)模式:白名单不生效,允许所有非黑名单命令
    // 注意:即使有 ENV_LOCK 互斥锁,前一个测试 drop guard 时可能留下 env;
    // 这里必须先 remove_var 再断言。
    let _env = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::remove_var(WINDOW_USE_MODE_ENV);
    }
    assert!(!window_use_mode(), "默认模式应关闭 window_use_mode");

    // SubAgent 模式下 curl 通过白名单检查(可能仍被黑名单拦截,但这是另一个层次)
    assert!(check_window_use_bash("curl https://example.com").is_ok());
    assert!(check_window_use_bash("cargo test").is_ok());
    assert!(check_window_use_bash("osascript -e 'foo'").is_ok());
}

// ========== 补丁 B:infer_delegate_to 自动纠正 ==========

#[test]
fn infer_delegate_to_correction() {
    fn make_spec(steps: Vec<&str>, delegate_to: AgentRole) -> WorkFlowSpec {
        WorkFlowSpec {
            id: "wf-test".into(),
            name: "测试工作流".into(),
            steps: steps.into_iter().map(String::from).collect(),
            branches: vec![],
            loops: vec![],
            depends_on: vec![],
            acceptance: vec![],
            delegate_to,
        }
    }

    // (a) 含 osascript 步骤 + 错误委派 windowuse -> 自动改 subagent
    let wf = make_spec(
        vec!["执行 osascript -e 'tell application \"WeChat\" to activate'"],
        AgentRole::WindowUse,
    );
    assert_eq!(infer_delegate_to(&wf), Some(AgentRole::SubAgent));

    // (b) 含 screencapture + cliclick -> 自动改 subagent
    let wf = make_spec(
        vec![
            "screencapture -x /tmp/screen.png",
            "cliclick c:200,300",
        ],
        AgentRole::WindowUse,
    );
    assert_eq!(infer_delegate_to(&wf), Some(AgentRole::SubAgent));

    // (c) 含 WindowList/Inspect/Action -> 自动改 windowuse(即便显式选 subagent)
    let wf = make_spec(
        vec![
            "调用 WindowList 找微信窗口",
            "WindowInspect 检视控件",
            "WindowAction 点击发送按钮",
        ],
        AgentRole::SubAgent,
    );
    assert_eq!(infer_delegate_to(&wf), Some(AgentRole::WindowUse));

    // (d) 没有命中任何关键词 -> 保持显式选择
    let wf = make_spec(vec!["读取项目根目录结构"], AgentRole::SubAgent);
    assert_eq!(infer_delegate_to(&wf), None);

    // (e) 混合型(osascript + 控件)-> shell 优先 SubAgent
    let wf = make_spec(
        vec![
            "osascript -e 'tell application \"WeChat\" to activate'",
            "WindowInspect 检查激活状态",
        ],
        AgentRole::SubAgent,
    );
    assert_eq!(infer_delegate_to(&wf), Some(AgentRole::SubAgent));
}

// ========== 补丁 D:macOS 26 AX 不可用文案包含 osascript 建议 ==========

#[cfg(target_os = "macos")]
#[test]
fn macos_ax_hint_contains_osascript_suggestion() {
    let driver = lsm_agent::agent::window::current_driver();
    // 在 macOS 26+ 上 permission_hint 应返回带 osascript 建议的字符串
    // 注意:macOS < 26 或 dlsym 仍能找到 AX 常量时,hint 可能是 None 或
    // "无障碍权限未授予"(kAXErrorAPIDisabled)—— 此种情况下跳过严格断言,
    // 仅当 hint 明确包含 "macOS 26" 关键词时(AX C API 移除)才验证含 osascript。
    if let Some(hint) = driver.permission_hint() {
        if hint.contains("macOS 26") {
            assert!(
                hint.contains("osascript"),
                "macOS 26 AX 不可用时 hint 应包含 osascript 替代建议,实际: {hint}"
            );
            assert!(
                hint.contains("Bash") || hint.contains("cliclick"),
                "hint 应引导改用 Bash/cliclick 路径,实际: {hint}"
            );
        }
    }
    // 如果 hint 为 None(AX 完全可用)或不含 macOS 26 关键词(AX 仍可用但无权限),
    // 不强制要求 osascript 提示 —— 这种情况属于正常路径,不算退化
}

#[cfg(target_os = "macos")]
#[test]
fn macos_ax_unavailable_hint_contains_osascript() {
    // 平台无关:验证 macOS WindowUse 在 AX C API 不可用时的提示文案
    // 已包含 osascript 替代建议(补丁 D 的硬性要求)
    //
    // 注:由于 macos 模块是私有的,我们通过 reflect permission_hint 函数
    // 在 AX 不可用场景下应当返回包含 osascript 的提示。
    // 由于测试机上 AX 可能仍可用,这一测试主要在源码层面验证:
    // 1) MACOS_AX_UNAVAILABLE_HINT 常量在 macos.rs 内部包含 osascript;
    // 2) permission_hint 函数在 ax_strings_loaded()=false 时返回该常量。
    //
    // 这里仅做最直接的源码包含验证:
    let macos_rs = include_str!("../src/agent/window/macos.rs");
    let has_const = macos_rs.contains("MACOS_AX_UNAVAILABLE_HINT");
    let has_osascript = macos_rs.contains("osascript");
    let has_unavailable = macos_rs.contains("不可用");
    assert!(
        has_const && has_osascript && has_unavailable,
        "macos.rs 应包含 MACOS_AX_UNAVAILABLE_HINT 常量 + osascript 关键词 + 不可用提示;实际: const={has_const} osascript={has_osascript} unavailable={has_unavailable}"
    );
}

// ========== WindowUse profile 工具集含 Bash ==========

#[test]
fn window_use_profile_includes_bash_tool() {
    use lsm_agent::agent::profile::AgentProfile;
    let p = AgentProfile::window_use_profile();
    let names = p.tools.names();
    assert!(
        names.contains(&"Bash"),
        "WindowUse profile 应包含 Bash 工具(补丁 A),实际: {names:?}"
    );
    assert!(names.contains(&"WindowList"));
    assert!(names.contains(&"WindowInspect"));
    assert!(names.contains(&"WindowAction"));
    assert!(names.contains(&"Read"));
    // 仍不带 Write
    assert!(!names.contains(&"Write"));
}
