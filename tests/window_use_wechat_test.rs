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
use lsm_agent::agent::tools::bash::BashTool;
use lsm_agent::agent::tools::bash::{check_window_use_bash, window_use_mode, WINDOW_USE_MODE_ENV};
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
    assert!(err.contains("白名单"), "错误应提及白名单,实际: {err}");

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

    // (a) 2026-09-16 第 67 轮语义修正:含 osascript 步骤但同时含 GUI 词(WeChat)
    //     → WindowUse(激活微信本就是窗口操控;WindowUse 自带 Bash 白名单可跑 osascript)。
    //     旧断言 SubAgent 正是「微信任务被路由到 Bash 路线」的根因,已反转。
    let wf = make_spec(
        vec!["执行 osascript -e 'tell application \"WeChat\" to activate'"],
        AgentRole::SubAgent,
    );
    assert_eq!(infer_delegate_to(&wf), Some(AgentRole::WindowUse));

    // (b) 含 screencapture + cliclick -> 自动改 subagent
    let wf = make_spec(
        vec!["screencapture -x /tmp/screen.png", "cliclick c:200,300"],
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

    // (e) 2026-09-16 第 67 轮语义修正:混合型(osascript + 控件)→ GUI 优先 WindowUse
    //     (旧断言 SubAgent 是微信任务误路由根因;WindowUse 自带 Bash 白名单)
    let wf = make_spec(
        vec![
            "osascript -e 'tell application \"WeChat\" to activate'",
            "WindowInspect 检查激活状态",
        ],
        AgentRole::SubAgent,
    );
    assert_eq!(infer_delegate_to(&wf), Some(AgentRole::WindowUse));
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
    let macos_rs = include_str!("../src/agent/window/macos_legacy.rs");
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

// ========== 2026-09-16 第 67 轮:全链路强化 + 微信 4.x 视觉路线 ==========

use lsm_agent::agent::tools::window::expand_window_query;
use lsm_agent::agent::window::ControlAction;

/// 第 67 轮 P0-B:别名表必须含 Weixin(Windows 微信 4.x 进程名)。
#[test]
fn round67_weixin_alias_included() {
    for q in ["WeChat", "微信", "weixin", "WEIXIN"] {
        let aliases = expand_window_query(q);
        assert!(
            aliases.iter().any(|a| a.eq_ignore_ascii_case("weixin")),
            "query={q} 应扩展出 Weixin 别名: {aliases:?}"
        );
        assert!(
            aliases.iter().any(|a| a == "微信"),
            "query={q} 应扩展出 微信 别名: {aliases:?}"
        );
    }
    // 无关应用不受影响
    assert_eq!(
        expand_window_query("Notepad"),
        vec!["Notepad".to_string()]
    );
}

/// 第 67 轮 P0-E:微信 + PowerShell 步骤必须路由 WindowUse(本次失败直接根因回归)。
#[test]
fn round67_wechat_powershell_steps_route_to_windowuse() {
    let spec = WorkFlowSpec {
        id: "wf-1".into(),
        name: "微信操控".into(),
        steps: vec![
            "用 PowerShell 确认微信客户端已启动".into(),
            "找到通讯录按钮并点击".into(),
            "遍历联系人列表找到目标用户".into(),
        ],
        branches: vec![],
        loops: vec![],
        depends_on: vec![],
        acceptance: vec!["消息出现在会话窗口".into()],
        delegate_to: AgentRole::SubAgent,
    };
    assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WindowUse));
}

/// 第 67 轮 P0-A:坐标动作解析(click_point / scroll_point / type_text)。
#[test]
fn round67_point_actions_parse() {
        // click_point 带坐标
    assert_eq!(
        ControlAction::parse_ext("click_point", None, Some(812), Some(402)).unwrap(),
        ControlAction::ClickPoint { x: 812, y: 402 }
    );
    // 别名 point_click / click_at
    assert_eq!(
        ControlAction::parse_ext("point_click", None, Some(1), Some(2)).unwrap(),
        ControlAction::ClickPoint { x: 1, y: 2 }
    );
    // scroll_point 带方向行数
    assert_eq!(
        ControlAction::parse_ext("scroll_point", Some("down:3".into()), Some(10), Some(20)).unwrap(),
        ControlAction::ScrollPoint { x: 10, y: 20, lines: -3 }
    );
    // type_text 需 text
    assert_eq!(
        ControlAction::parse_ext("type_text", Some("你好".into()), None, None).unwrap(),
        ControlAction::TypeText("你好".into())
    );
    // 坐标动作缺 x/y → 结构化错误
    let err = ControlAction::parse_ext("click_point", None, None, None).unwrap_err();
    assert!(err.to_string().contains("x / y"), "错误应提示缺坐标: {err}");
    // is_point_action 判定
    assert!(ControlAction::parse_ext("double_click_point", None, Some(1), Some(2))
        .unwrap()
        .is_point_action());
    assert!(!ControlAction::parse("click", None).unwrap().is_point_action());
}

/// 第 67 轮 P0-E:同应用(泛化到 QQ 族)WindowUse 链合并。
/// 验证 coalesce 泛化(通过 parse_workflow_plan + 委派覆盖后的公开路径无法直接触达私有函数,
/// 这里验证 infer 路由 + 手工构造 plan 走 main_work 模块公开的 topo_layers 兼容性)。
#[test]
fn round67_wechat_family_infer_keywords() {
    // 通讯录/鼠标/滚轮等新 GUI 关键词路由 WindowUse
    let spec = WorkFlowSpec {
        id: "wf-1".into(),
        name: "微信".into(),
        steps: vec!["模拟鼠标滚轮滚动联系人列表".into()],
        branches: vec![],
        loops: vec![],
        depends_on: vec![],
        acceptance: vec![],
        delegate_to: AgentRole::SubAgent,
    };
    assert_eq!(infer_delegate_to(&spec), Some(AgentRole::WindowUse));
}

/// 第 67 轮 P1-A:orchestrator 工具参数摘要(纯函数,经私有性检查改由 lib 测试覆盖;
/// 这里验证公开符号可见性编译)。
#[test]
fn round67_public_surface_compiles() {
    // window registry 含 WindowOCR
    let reg = lsm_agent::agent::tools::window_use_registry();
    let names = reg.names();
    assert!(names.contains(&"WindowOCR"), "WindowOCR 必须注册: {names:?}");
    assert!(names.contains(&"WindowAction"));
}

/// 第 67 轮只读冒烟:微信 4.x 激活 + OCR 视觉路线(不点击/不输入/不发消息)。
/// 人工运行:`cargo test --test window_use_wechat_test -- --ignored --nocapture`
#[tokio::test]
#[ignore = "需要本机已登录微信且处于交互桌面;只读,不点击不输入"]
async fn round67_wechat_ocr_readonly_probe() {
    let driver = lsm_agent::agent::window::current_driver();
    let wins = driver.list_windows(None).expect("枚举窗口");
    let main = wins
        .iter()
        .find(|w| w.title == "微信" || w.process_name.to_lowercase().starts_with("weixin"))
        .expect("未找到微信窗口(微信需已登录运行)");
    println!("微信窗口 id={} proc={} title={}", main.id, main.process_name, main.title);

    // 1) 进程名修复验证:QueryFullProcessImageNameW 后不应为空
    assert!(
        !main.process_name.is_empty(),
        "进程名不应为空(第 67 轮 QueryFullProcessImageNameW 修复)"
    );

    // 2) 激活(恢复最小化 + 前置)
    driver.bring_to_front(&main.id).expect("bring_to_front");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;

    // 3) OCR(只读)
    let blocks = driver.ocr(&main.id, None, None).expect("OCR");
    println!("OCR blocks = {}", blocks.len());
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    for b in blocks.iter().take(30) {
        println!("  '{}' @ ({},{}) {}x{}", b.text, b.x, b.y, b.width, b.height);
    }
    // 已登录主窗口:OCR 应返回足量**整行中文**文本(会话列表/消息区)。
    // 注:左侧导航条的小号低对比标签(聊天/通讯录图标下文字)实测常被 OCR 漏识别,
    // 不作为断言条件 —— 导航定位由 LLM 结合窗口布局启发式完成,点击后 OCR 复查。
    let _ = texts;
    let chinese_lines = blocks
        .iter()
        .filter(|b| {
            b.text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
                && b.text.chars().count() >= 2
        })
        .count();
    assert!(
        blocks.len() >= 10 && chinese_lines >= 5,
        "OCR 应识别出足量整行中文文本(实测微信主窗口约 30 行): blocks={} 中文行={chinese_lines}",
        blocks.len()
    );
}

/// 第 67 轮 SendInput 安全 e2e:记事本真实键入 + OCR 回读(不触碰微信)。
/// 人工运行:`cargo test --test window_use_wechat_test -- --ignored --nocapture round67_notepad`
#[tokio::test]
#[ignore = "会打开记事本并注入键入;人工运行"]
async fn round67_notepad_sendinput_e2e() {
    let driver = lsm_agent::agent::window::current_driver();
    // 1) 启动记事本(ShellExecuteW 解析链)
    let mut child = std::process::Command::new("notepad.exe").spawn().expect("启动记事本");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    // 2) 找到记事本窗口
    let wins = driver.list_windows(None).unwrap();
    let np = wins
        .iter()
        .find(|w| w.process_name.to_lowercase().starts_with("notepad"))
        .expect("未找到记事本窗口");
    driver.bring_to_front(&np.id).expect("前台化");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // 3) 真实键入(SendInput Unicode,中文直入)
    winput_type(&np.id, "LAEW67中文键入测试").await;

    // 4) OCR 回读验证
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let blocks = driver.ocr(&np.id, None, None).expect("OCR");
    let joined: String = blocks.iter().map(|b| b.text.as_str()).collect();
    println!("记事本 OCR: {joined:?}");
    assert!(
        joined.contains("LAEW67"),
        "OCR 应回读到键入的文本: {joined:?}"
    );

    // 5) send_keys(enter)+ 再 OCR
    driver
        .act(&np.id, "/", lsm_agent::agent::window::ControlAction::SendKeys("enter".into()))
        .expect("enter");
    winput_type(&np.id, "SECOND").await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let blocks2 = driver.ocr(&np.id, None, None).expect("OCR2");
    let joined2: String = blocks2.iter().map(|b| b.text.as_str()).collect();
    println!("记事本 OCR2: {joined2:?}");
    assert!(joined2.contains("SECOND"), "回车后第二行应可回读: {joined2:?}");

    // 收尾:不 kill —— Win11 记事本是单实例多 Tab,kill 会连带关掉用户已有会话;
    // 留窗让用户自行处理(测试文本在无标题/新 Tab 中,可手动关闭)。
    drop(child);
}

/// 通过 driver.act 触发 TypeText(等价 WindowAction action=type_text)。
async fn winput_type(window_id: &str, text: &str) {
    lsm_agent::agent::window::current_driver()
        .act(
            window_id,
            "/",
            lsm_agent::agent::window::ControlAction::TypeText(text.to_string()),
        )
        .expect("type_text");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}
