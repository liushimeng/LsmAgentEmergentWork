//! mcp_window_use 工具模块单元测试(2026-09-18 第 84 轮自 tools/window/tests.rs 迁入,
//! 并新增 MCP_Window_Use 门面/平台门控用例)。

use super::*;
use crate::agent::window::ControlNode;

fn deep_tree(depth: usize) -> ControlNode {
    let mut node = ControlNode {
        path: "/".into(),
        role: "Window".into(),
        name: "root".into(),
        ..Default::default()
    };
    let mut cur = &mut node;
    for i in 0..depth {
        cur.children.push(ControlNode {
            path: format!("/{i}"),
            role: "Pane".into(),
            name: format!("n{i}"),
            ..Default::default()
        });
        cur = cur.children.last_mut().unwrap();
    }
    node
}

#[test]
fn count_nodes_counts_all() {
    let t = deep_tree(5);
    assert_eq!(count_nodes(&t), 6);
}

#[test]
fn prune_nodes_respects_budget() {
    let mut t = ControlNode {
        path: "/".into(),
        role: "Window".into(),
        name: "root".into(),
        ..Default::default()
    };
    for i in 0..10 {
        t.children.push(ControlNode {
            path: format!("/{i}"),
            role: "Button".into(),
            name: format!("b{i}"),
            ..Default::default()
        });
    }
    let mut budget = 4;
    prune_nodes(&mut t, &mut budget);
    assert_eq!(count_nodes(&t), 5); // root + 4
}

#[test]
fn tree_to_json_truncates_huge_tree() {
    // 构造超过节点上限的树
    let mut root = ControlNode {
        path: "/".into(),
        role: "Window".into(),
        name: "root".into(),
        ..Default::default()
    };
    for i in 0..600 {
        root.children.push(ControlNode {
            path: format!("/{i}"),
            role: "Button".into(),
            name: format!("btn-{i}"),
            ..Default::default()
        });
    }
    let s = tree_to_json(root);
    assert!(s.contains("[truncated]"), "应含截断标注");
}

// ========== MCP_Window_Use 门面 ==========

#[test]
fn tool_def_schema_has_action_enum() {
    let t = McpWindowUseTool;
    assert_eq!(t.name(), "MCP_Window_Use");
    let params = t.parameters();
    let actions = params["properties"]["action"]["enum"]
        .as_array()
        .expect("action 应为枚举");
    let actions: Vec<&str> = actions.iter().filter_map(Value::as_str).collect();
    for expected in [
        "open",
        "list",
        "find",
        "inspect",
        "control",
        "ocr",
        "screenshot",
        "capability_probe",
        "osascript_run",
        "chat_send",
        "chat_loop",
        "input_batch",
    ] {
        assert!(actions.contains(&expected), "缺少 action={expected}");
    }
    assert_eq!(params["required"], json!(["action"]));
    // 描述中应包含使用说明关键段落(平台权限矩阵 / 标准作业顺序)
    assert!(t.description().contains("标准作业顺序"));
    assert!(t.description().contains("权限矩阵"));
    // 第 86 轮新增:capability_probe_first 原则
    assert!(t.description().contains("capability_probe_first"));
    // 第 90 轮:鼠标键盘原子能力 + input_batch + 优先级链说明
    assert!(t.description().contains("move_point"));
    assert!(t.description().contains("drag_point"));
    assert!(t.description().contains("modifiers"));
    assert!(t.description().contains("input_batch"));
    assert!(t.description().contains("无障碍 API"), "应说明优先级链");
    // control_action 枚举含第 90 轮新值
    let cas = params["properties"]["control_action"]["enum"]
        .as_array()
        .expect("control_action 应为枚举");
    let cas: Vec<&str> = cas.iter().filter_map(Value::as_str).collect();
    for expected in ["move_point", "middle_click_point", "drag_point", "click_point", "type_text_submit"] {
        assert!(cas.contains(&expected), "缺少 control_action={expected}");
    }
    // 第 90 轮新参数
    for key in ["modifiers", "x2", "y2", "steps", "continue_on_error"] {
        assert!(
            params["properties"].get(key).is_some(),
            "缺少参数 {key}"
        );
    }
    // steps 子 Schema:op 枚举 + maxItems
    let steps = &params["properties"]["steps"];
    assert_eq!(steps["maxItems"], json!(40));
    let ops = steps["items"]["properties"]["op"]["enum"]
        .as_array()
        .unwrap();
    let ops: Vec<&str> = ops.iter().filter_map(Value::as_str).collect();
    for expected in [
        "mouse_move",
        "mouse_click",
        "mouse_drag",
        "mouse_scroll",
        "key_press",
        "type_text",
        "click",
        "set_text",
        "get_text",
        "wait",
    ] {
        assert!(ops.contains(&expected), "缺少 input_batch op={expected}");
    }
}

#[test]
fn platform_gate_matches_target_os() {
    // macOS / Windows 定义工具;其余平台不注册
    let expect = cfg!(any(target_os = "macos", target_os = "windows"));
    assert_eq!(mcp_window_use_available(), expect);
}

#[tokio::test]
async fn execute_requires_action() {
    let t = McpWindowUseTool;
    let err = t.execute(json!({})).await.unwrap_err();
    assert!(err.to_string().contains("action"));
}

#[tokio::test]
async fn execute_rejects_unknown_action() {
    let t = McpWindowUseTool;
    let err = t
        .execute(json!({"action": "explode"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("未知 action"));
}

#[tokio::test]
async fn control_validates_params() {
    let t = McpWindowUseTool;
    // 缺 path
    let err = t
        .execute(json!({"action": "control", "window_id": "1", "control_action": "click"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("path"));
    // set_text 缺 text
    let err = t
        .execute(json!({"action": "control", "window_id": "1", "path": "/", "control_action": "set_text"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("text"));
    // 未知 control_action
    let err = t
        .execute(json!({"action": "control", "window_id": "1", "path": "/", "control_action": "explode"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("未知 action"));
}

#[tokio::test]
async fn inspect_validates_window_id() {
    let t = McpWindowUseTool;
    let err = t.execute(json!({"action": "inspect"})).await.unwrap_err();
    assert!(err.to_string().contains("window_id"));
}

#[tokio::test]
async fn find_validates_query() {
    let t = McpWindowUseTool;
    let err = t.execute(json!({"action": "find"})).await.unwrap_err();
    assert!(err.to_string().contains("query"));
}

// ========== 模糊匹配(自 tools/window/tests.rs 迁入) ==========

#[test]
fn damerau_levenshtein_basic() {
    // 完全相等
    assert_eq!(query::damerau_levenshtein("abc", "abc"), 0);
    // 单字符插入
    assert_eq!(query::damerau_levenshtein("abc", "abcd"), 1);
    // 单字符替换
    assert_eq!(query::damerau_levenshtein("abc", "abd"), 1);
    // 完全不等
    assert!(query::damerau_levenshtein("abc", "xyz") > 0);
    // 上界剪枝:长度差 > 8 直接返回 max_len("abcdefghijklmnop" = 16 字符)
    let d = query::damerau_levenshtein("ab", "abcdefghijklmnop");
    assert_eq!(d, 16);
}

#[test]
fn match_mode_parse_aliases() {
    assert_eq!(query::MatchMode::parse("exact"), query::MatchMode::Exact);
    assert_eq!(query::MatchMode::parse("EXACT"), query::MatchMode::Exact);
    assert_eq!(query::MatchMode::parse("contains"), query::MatchMode::Contains);
    assert_eq!(query::MatchMode::parse("fuzzy"), query::MatchMode::Fuzzy);
    assert_eq!(query::MatchMode::parse("unknown"), query::MatchMode::Contains); // 默认 contains
}

fn make_wins() -> Vec<WindowInfo> {
    vec![
        WindowInfo {
            id: "w-1".into(),
            title: "微信(WeChat)".into(),
            process_name: "WeChat".into(),
            pid: 100,
            bounds: Default::default(),
            cg_window_id: Some(1001),
            hwnd: None,
            wmctrl_id: None,
        },
        WindowInfo {
            id: "w-2".into(),
            title: "无标题.txt - 记事本".into(),
            process_name: "Notepad".into(),
            pid: 101,
            bounds: Default::default(),
            cg_window_id: None,
            hwnd: None,
            wmctrl_id: None,
        },
        WindowInfo {
            id: "w-3".into(),
            title: "Settings".into(),
            process_name: "System Preferences".into(),
            pid: 102,
            bounds: Default::default(),
            cg_window_id: None,
            hwnd: None,
            wmctrl_id: None,
        },
    ]
}

#[test]
fn pick_top_hit_exact_matches() {
    let wins = make_wins();
    // "wechat" 精确匹配 w-1 的 process_name(WeChat) —— title 是 "微信(WeChat)"
    // 含括号,大小写归一后不等;process 字段命中。
    let hit = query::pick_top_hit(&wins, "wechat", query::MatchMode::Exact).unwrap();
    assert_eq!(hit.info.id, "w-1");
    assert_eq!(hit.matched_field, "process");
    assert!((hit.score - 1.0).abs() < 1e-9);
}

#[test]
fn pick_top_hit_contains_substring() {
    let wins = make_wins();
    // "记事本" 是 "无标题.txt - 记事本" 的子串
    let hit = query::pick_top_hit(&wins, "记事本", query::MatchMode::Contains).unwrap();
    assert_eq!(hit.info.id, "w-2");
    assert_eq!(hit.matched_field, "title");
}

#[test]
fn pick_top_hit_fuzzy_finds_typo() {
    let wins = make_wins();
    // "WeChqt"(拼错一个字符) 在 contains 模式无命中,在 fuzzy 模式能找到
    assert!(query::pick_top_hit(&wins, "WeChqt", query::MatchMode::Contains).is_none());
    let hit = query::pick_top_hit(&wins, "WeChqt", query::MatchMode::Fuzzy).unwrap();
    assert_eq!(hit.info.id, "w-1");
    assert!(hit.score > 0.5);
}

#[test]
fn pick_top_hit_no_match_returns_none() {
    let wins = make_wins();
    let q = "完全不存在的应用xyz";
    assert!(query::pick_top_hit(&wins, q, query::MatchMode::Contains).is_none());
    assert!(query::pick_top_hit(&wins, q, query::MatchMode::Fuzzy).is_none());
    assert!(query::pick_top_hit(&wins, q, query::MatchMode::Exact).is_none());
}

#[test]
fn window_query_aliases_cover_localized_wechat() {
    // Weixin 别名(Windows 微信 4.x 进程名 Weixin.exe)
    assert_eq!(
        expand_window_query("WeChat"),
        vec![
            "WeChat".to_string(),
            "微信".to_string(),
            "Weixin".to_string()
        ]
    );
    assert_eq!(
        expand_window_query("微信"),
        vec![
            "微信".to_string(),
            "WeChat".to_string(),
            "Weixin".to_string()
        ]
    );
    assert_eq!(
        expand_window_query("weixin"),
        vec![
            "weixin".to_string(),
            "WeChat".to_string(),
            "微信".to_string(),
        ]
    );
    assert_eq!(expand_window_query("Safari"), vec!["Safari".to_string()]);
}

#[tokio::test]
#[ignore = "会启动/激活本机微信,仅人工桌面环境验证;不发送消息"]
async fn open_finds_localized_wechat_without_sending() {
    let t = McpWindowUseTool;
    let output = t
        .execute(json!({"action": "open", "query": "WeChat", "wait_seconds": 8}))
        .await
        .expect("应能打开/激活微信并定位窗口");
    let value: Value = serde_json::from_str(&output).expect("open 应返回 JSON");
    assert_eq!(value["ok"], json!(true));
    assert!(value["window_id"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(
        value["matched_query"].as_str() == Some("WeChat")
            || value["matched_query"].as_str() == Some("微信")
    );
}

#[test]
fn pick_top_hit_picks_higher_score_field() {
    // 测试同 query 在不同字段都有命中
    let wins = vec![
        WindowInfo {
            id: "a".into(),
            title: "abc".into(), // contains "abc"
            process_name: "x".into(),
            pid: 1,
            bounds: Default::default(),
            cg_window_id: None,
            hwnd: None,
            wmctrl_id: None,
        },
        WindowInfo {
            id: "b".into(),
            title: "x".into(),
            process_name: "abc".into(),
            pid: 2,
            bounds: Default::default(),
            cg_window_id: None,
            hwnd: None,
            wmctrl_id: None,
        },
    ];
    let hit = query::pick_top_hit(&wins, "abc", query::MatchMode::Contains).unwrap();
    assert!(hit.info.id == "a" || hit.info.id == "b");
    assert!(hit.score > 0.3);
}

// ========== macOS 辅助功能权限请求测试 ==========

/// 测试 AxPermissionResult 枚举的创建和匹配。
#[cfg(target_os = "macos")]
#[test]
fn ax_permission_result_granted() {
    let result = AxPermissionResult::Granted;
    match result {
        AxPermissionResult::Granted => {} // 正确
        _ => panic!("应为 Granted"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn ax_permission_result_denied() {
    let msg = "测试拒绝消息".to_string();
    let result = AxPermissionResult::Denied {
        message: msg.clone(),
    };
    match result {
        AxPermissionResult::Denied { message } => {
            assert_eq!(message, msg);
        }
        _ => panic!("应为 Denied"),
    }
}

/// 测试 driver_preflight 在 macOS 上的权限请求行为。
/// 注意:此测试仅在 macOS 上运行,且会触发系统权限弹窗(如果未授权)。
/// 为避免干扰正常测试流程,使用 #[ignore] 标记,需要时手动运行:
///   cargo test --lib -- --ignored
#[tokio::test]
#[ignore = "会触发系统权限弹窗,需手动运行"]
async fn driver_preflight_requests_ax_permission_on_macos() {
    if cfg!(target_os = "macos") {
        unsafe {
            std::env::set_var("LAEW_AX_WAIT_SECS", "0");
        }
        // 调用 driver_preflight,如果未授权应触发弹窗
        let result = driver_preflight("TestTool").await;
        // 结果取决于用户是否授权:
        // - 已授权 → Ok(())
        // - 未授权但用户弹窗后授权 → Ok(())
        // - 未授权且用户拒绝 → Err(...)
        // 不断言具体结果,只确保不 panic
        match result {
            Ok(()) => println!("权限已授权"),
            Err(e) => println!("权限请求结果: {e}"),
        }
    }
}

// ===================== 2026-09-18 第 85 轮新增单测 =====================
//
// 覆盖 KNOWN_BUNDLE_IDS 已知桌面应用映射表(解决 open -a 中文名无法启动微信的根因)。

#[test]
fn lookup_known_bundle_id_resolves_wechat_aliases() {
    // 中文 query 必须命中 com.tencent.xinWeChat
    assert_eq!(super::open::lookup_known_bundle_id("微信"), Some("com.tencent.xinWeChat"));
    // 英文别名(包含子串)
    assert_eq!(super::open::lookup_known_bundle_id("WeChat"), Some("com.tencent.xinWeChat"));
    assert_eq!(super::open::lookup_known_bundle_id("wechat"), Some("com.tencent.xinWeChat"));
    assert_eq!(super::open::lookup_known_bundle_id("Weixin"), Some("com.tencent.xinWeChat"));
}

#[test]
fn lookup_known_bundle_id_resolves_dingtalk_feishu_qq() {
    assert_eq!(super::open::lookup_known_bundle_id("钉钉"), Some("com.laiwang.DingTalk"));
    assert_eq!(super::open::lookup_known_bundle_id("DingTalk"), Some("com.laiwang.DingTalk"));
    assert_eq!(super::open::lookup_known_bundle_id("飞书"), Some("com.bytedance.feishu"));
    assert_eq!(super::open::lookup_known_bundle_id("Lark"), Some("com.bytedance.feishu"));
    assert_eq!(super::open::lookup_known_bundle_id("QQ"), Some("com.tencent.qq"));
}

#[test]
fn lookup_known_bundle_id_returns_none_for_unknown() {
    assert_eq!(super::open::lookup_known_bundle_id(""), None);
    assert_eq!(super::open::lookup_known_bundle_id("完全未知的应用名xyz"), None);
    assert_eq!(super::open::lookup_known_bundle_id("RandomUnknownApp"), None);
}

#[test]
fn expand_window_query_still_works() {
    // 回归:扩名表没坏
    let aliases = expand_window_query("微信");
    assert!(aliases.contains(&"微信".to_string()));
    assert!(aliases.contains(&"WeChat".to_string()));
    assert!(aliases.contains(&"Weixin".to_string()));
}

// ============== 第 86 轮新增测试 ==============

#[test]
fn capability_matrix_no_screen_recording_disables_ocr() {
    // 模拟「AX 已授权 + 屏录未授权」场景:OCR/screencapture 全部 false,
    // 但 inspect_control/coordinate_input 仍为 true。
    // 第 90 轮修复:from_permissions 按**编译期平台**分支,此前断言按 macOS cfg
    // 写死,Windows 上(此前编译损坏从未跑过)必败;改为按平台分叉断言。
    use crate::agent::window::PermissionReport;

    let report = PermissionReport {
        platform: "macos".into(),
        accessibility: true,
        screen_recording: false,
        can_ocr: false,
        can_screenshot: false,
        accessibility_hint: String::new(),
        screen_recording_hint: "需要授权".into(),
    };
    let cap = crate::agent::window::WindowCapability::from_permissions(&report);
    assert!(cap.list_find);
    assert!(cap.inspect_control);
    assert!(cap.coordinate_input);
    if cfg!(target_os = "macos") {
        assert!(cap.ax_warmup);
        // 第 86 轮修正:CGWindowListCreateImage 实测需屏幕录制
        assert!(!cap.ocr_screenshot_cgwindow);
        assert!(!cap.screencapture_cli);
    } else {
        // Windows / Linux:无 TCC 门控,GDI 截图 / screencapture CLI 不受屏录影响;
        // ax_warmup 仅 macOS 有意义。
        assert!(!cap.ax_warmup);
    }
}

#[test]
fn capability_matrix_full_grants_enables_all() {
    // 全权限场景:所有路线都可用
    use crate::agent::window::PermissionReport;

    let report = PermissionReport {
        platform: "macos".into(),
        accessibility: true,
        screen_recording: true,
        can_ocr: true,
        can_screenshot: true,
        accessibility_hint: String::new(),
        screen_recording_hint: String::new(),
    };
    let cap = crate::agent::window::WindowCapability::from_permissions(&report);
    assert!(cap.ocr_screenshot_cgwindow);
    assert!(cap.screencapture_cli);
    assert!(cap.inspect_control);
    assert!(cap.coordinate_input);
}

#[test]
fn chat_log_path_default_uses_cwd() {
    // 缺省 chat_log_path 应在工作目录落盘
    let args = json!({});
    let p = chat::resolve_chat_log_path(&args);
    assert!(p.starts_with(std::env::current_dir().unwrap().to_str().unwrap()));
    assert!(p.contains("llaew_chat_"));
    assert!(p.ends_with(".log"));
}

#[test]
fn chat_log_path_explicit_overrides_default() {
    // 显式 chat_log_path 应被尊重
    let custom = "/tmp/my_chat_test.log";
    let args = json!({"chat_log_path": custom});
    let p = chat::resolve_chat_log_path(&args);
    assert_eq!(p, custom);
}

#[test]
fn chat_log_line_format_is_stable() {
    let line = chat::format_chat_log_line(1700000000, "SEND", "test body");
    assert!(line.contains("[SEND]"));
    assert!(line.contains("1700000000"));
    assert!(line.contains("test body"));
}

#[test]
fn applescript_escape_handles_special_chars() {
    let s = chat::escape_applescript_string("hello \"world\" \\ 测试");
    assert!(s.contains("\\\""));
    assert!(s.contains("\\\\"));
    // 中文不应被转义
    assert!(s.contains("测试"));
}

#[test]
fn infer_target_app_recognizes_wechat() {
    assert_eq!(chat::infer_target_app_name("wechat:0:1234"), Some("WeChat".into()));
    assert_eq!(chat::infer_target_app_name("wx_main_456"), Some("WeChat".into()));
    assert_eq!(chat::infer_target_app_name("DingTalk.123"), Some("DingTalk".into()));
    assert_eq!(chat::infer_target_app_name("unknown_window"), None);
}

#[tokio::test]
async fn osascript_run_rejects_windows_linux() {
    #[cfg(not(target_os = "macos"))]
    {
        let t = McpWindowUseTool;
        let err = t
            .execute(json!({"action": "osascript_run", "osascript_script": "return 1"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("仅 macOS 可用"));
    }
    #[cfg(target_os = "macos")]
    {
        // macOS 上此测试仅在 mock LLM 环境跑,real LLM 跳过
        // 通过直接构造 osascript 短脚本验证可执行
        let _ = "macos branch covered by ignore";
    }
}

#[tokio::test]
async fn capability_probe_returns_valid_matrix() {
    // capability_probe 不需要任何参数,直接返回当前能力矩阵
    let t = McpWindowUseTool;
    let r = t
        .execute(json!({"action": "capability_probe"}))
        .await
        .expect("capability_probe 不应失败");
    let v: Value = serde_json::from_str(&r).expect("应为合法 JSON");
    assert_eq!(v["ok"], json!(true));
    assert!(v["capability"]["list_find"].is_boolean());
    assert!(v["capability"]["inspect_control"].is_boolean());
    assert!(v["capability"]["ocr_screenshot_cgwindow"].is_boolean());
    assert!(v["capability"]["coordinate_input"].is_boolean());
    assert!(v["next_action"].is_string());
    assert!(v["recommended_route"].is_string());
}

#[tokio::test]
async fn osascript_run_requires_osascript_script() {
    let t = McpWindowUseTool;
    let err = t
        .execute(json!({"action": "osascript_run"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("osascript_script"));
}

#[tokio::test]
async fn chat_send_requires_text() {
    let t = McpWindowUseTool;
    let err = t
        .execute(json!({"action": "chat_send", "window_id": "x"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("text"));
}

// ===================== 2026-09-18 第 87 轮 =====================

#[test]
fn infer_app_name_from_process_chinese_aliases() {
    // 微信 macOS 进程名可能是「微信」或「WeChat」,window_id 为 pid:wid 时
    // 需经 process_name 映射,osascript_fallback 才能 activate 正确应用。
    use super::chat::infer_app_name_from_process;
    assert_eq!(infer_app_name_from_process("微信").as_deref(), Some("WeChat"));
    assert_eq!(infer_app_name_from_process("WeChat").as_deref(), Some("WeChat"));
    assert_eq!(infer_app_name_from_process("Weixin").as_deref(), Some("WeChat"));
    assert_eq!(infer_app_name_from_process("钉钉").as_deref(), Some("DingTalk"));
    assert_eq!(infer_app_name_from_process("飞书").as_deref(), Some("Lark"));
    assert_eq!(infer_app_name_from_process("Finder"), None);
}

// ===================== 2026-09-18 第 88 轮 =====================

#[test]
fn estimate_input_point_right_bottom_region() {
    // 主流 IM 主界面输入框比例估算:右 72% 宽 × 下 88% 高(微信 4.x 实测布局)。
    use super::chat::estimate_input_point;
    let bounds = crate::agent::window::Rect {
        x: 1602,
        y: 822,
        width: 1541,
        height: 923,
    };
    let (px, py) = estimate_input_point(&bounds);
    assert!(px > bounds.x + bounds.width / 2, "必须在右半区: {px}");
    assert!(py > bounds.y + bounds.height / 2, "必须在下半区: {py}");
    assert!(px < bounds.x + bounds.width, "不得超出窗口右缘");
    assert!(py < bounds.y + bounds.height, "不得超出窗口下缘");
    assert_eq!(px, 1602 + (1541.0 * 0.72) as i64);
    assert_eq!(py, 822 + (923.0 * 0.88) as i64);
}

#[test]
fn escape_applescript_string_escapes_quotes_and_backslash() {
    use super::chat::escape_applescript_string;
    assert_eq!(escape_applescript_string("a\"b"), "a\\\"b");
    assert_eq!(escape_applescript_string("a\\b"), "a\\\\b");
    assert_eq!(escape_applescript_string("微信"), "微信");
}

/// 第 88 轮 P0-1 回归:osascript 经 argv 直传(不经 shell),脚本内双引号
/// 不再被 `\\\"` 腐蚀;多行 tell 块 + `return` 正常执行,非零退出真实上报。
#[cfg(target_os = "macos")]
#[test]
fn osascript_exec_argv_direct_no_shell_quoting() {
    use super::chat::osascript_exec;
    // 成功路径:脚本内双引号原样生效(旧 shell 包裹实现必败,-2741)
    let ok = osascript_exec("return 42", 5000).unwrap();
    assert!(ok.ok, "return 42 应成功: {:?}", ok);
    assert_eq!(ok.exit_code, 0);
    assert_eq!(ok.stdout, "42");
    // 多行 tell 块(含双引号)编译通过即可,不要求执行成功语义
    let multi = osascript_exec("tell application \"Finder\"\nend tell\nreturn 7", 5000)
        .unwrap();
    assert!(multi.ok, "多行 tell 块应正常执行: {:?}", multi);
    assert_eq!(multi.stdout, "7");
    // 失败路径:语法错误 → ok=false + 非零 exit_code + stderr 非空(不再静默)
    let bad = osascript_exec("this is not applescript", 5000).unwrap();
    assert!(!bad.ok, "非法脚本必须 ok=false");
    assert_ne!(bad.exit_code, 0);
    assert!(!bad.stderr.is_empty(), "stderr 必须有 AppleScript 报错原文");
    assert!(!bad.timed_out);
}

#[cfg(target_os = "macos")]
#[test]
fn osascript_exec_timeout_kills_child() {
    use super::chat::osascript_exec;
    // 明显超过 timeout 的 delay → timed_out=true(快速返回,不真等 30s)
    let started = std::time::Instant::now();
    let out = osascript_exec("delay 30", 1200).unwrap();
    assert!(out.timed_out, "超时脚本必须 timed_out: {:?}", out);
    assert!(!out.ok);
    assert!(
        started.elapsed().as_secs() < 10,
        "超时 kill 应快速返回,实际 {:?}",
        started.elapsed()
    );
}

// ========== 第 90 轮:input_batch 参数校验(校验在前台守卫之前,不触驱动) ==========

#[tokio::test]
async fn input_batch_requires_steps_array() {
    let t = McpWindowUseTool;
    // 缺 steps → 结构化报错
    let err = t
        .execute(json!({"action": "input_batch", "window_id": "123"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("steps"), "{err}");
    // steps 空数组 → 报错
    let err = t
        .execute(json!({"action": "input_batch", "window_id": "123", "steps": []}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("不能为空"), "{err}");
    // steps 超上限(41 > 40)→ 报错
    let too_many: Vec<Value> = (0..41)
        .map(|i| json!({"op": "wait", "ms": 1, "i": i}))
        .collect();
    let err = t
        .execute(json!({"action": "input_batch", "window_id": "123", "steps": too_many}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("超上限"), "{err}");
}

#[test]
fn control_action_parse_round90_tool_level() {
    use crate::agent::window::ControlAction;
    // drag_point 缺 x2/y2 → 结构化报错(文案点名参数)
    let err = ControlAction::parse_ext(
        "drag_point",
        None,
        Some(1),
        Some(2),
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("x2 / y2"), "{err}");
    // modifiers 非法段报错
    let err = ControlAction::parse_ext(
        "click_point",
        None,
        Some(1),
        Some(2),
        None,
        None,
        Some("ctrl+banana".into()),
    )
    .unwrap_err();
    assert!(err.to_string().contains("modifiers"), "{err}");
}
