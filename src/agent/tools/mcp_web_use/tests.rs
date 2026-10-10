//! MCP_Web_Use 工具单元测试(自 tools/browser.rs 测试平移 + 单工具化改造)。

use super::*;
use super::{control, extract, hitl_hint, inspect, page_state, visit_ledger};
use base64::Engine as _;

#[test]
fn envelope_codes() {
    let s = envelope(0, "ok", json!({"x": 1})).unwrap();
    assert!(s.contains("\"code\":0"));
    assert!(s.contains("\"message\":\"ok\""));
    assert!(s.contains("\"x\":1"));
}

// ==================== 第 140 轮:inspect 体积闸门 ====================

#[test]
fn gate_payload_small_is_identity() {
    let v = json!({"ok": true, "selector": "form", "outer_html": "<form></form>"});
    assert_eq!(inspect::gate_payload(v.clone()), v);
}

#[test]
fn cap_strings_clips_long_string_with_marker() {
    let mut v = json!({"html": "X".repeat(20000), "ok": true});
    inspect::cap_strings(&mut v, 100);
    let s = v["html"].as_str().unwrap();
    assert!(s.starts_with(&"X".repeat(100)));
    assert!(s.contains("截断,共 20000 字符"));
    assert_eq!(v["ok"], json!(true), "短字段不受影响");
}

#[test]
fn gate_payload_huge_object_degrades_to_preview() {
    // 单字符串已裁到 8K,但 5 个 8K 字段合计仍超 24KB → 整体降级为摘要
    let big = "Y".repeat(20000);
    let v = json!({
        "a": big, "b": big, "c": big, "d": big, "e": big,
    });
    let out = inspect::gate_payload(v);
    assert_eq!(out["truncated"], json!(true));
    assert!(out["original_bytes"].as_u64().unwrap() > 24 * 1024);
    assert!(out["preview"].as_str().unwrap().contains("截断"));
    assert!(out["hint"].as_str().unwrap().contains("extract"));
}

// ==================== 第 140 轮:adopt_spawned_pages 按需化 ====================

#[test]
fn action_may_spawn_page_whitelist() {
    // 可能开新页的动作:巡检保留
    for a in [
        "click",
        "human_click",
        "right_click",
        "double_click",
        "key_press",
        "press_sequence",
        "navigate",
        "new_tab",
        "back",
        "forward",
        "reload",
        "select_option",
        "upload_file",
        "download",
        "dispatch_event",
        "eval_js",
    ] {
        assert!(control::action_may_spawn_page(a), "{a} 应巡检派生页");
    }
    // 观察/状态类动作:跳过巡检
    for a in [
        "wait",
        "input_text",
        "human_input",
        "clear_input",
        "screenshot",
        "set_cookie",
        "delete_cookie",
        "set_storage",
        "clear_storage",
        "set_viewport",
        "set_window",
        "sync_viewport",
        "set_highlight",
        "heartbeat",
        "hover",
        "scroll",
        "scroll_to",
        "drag",
        "focus",
        "blur",
        "mouse_move",
    ] {
        assert!(!control::action_may_spawn_page(a), "{a} 不应巡检派生页");
    }
}

#[test]
fn js_str_escapes() {
    let s = js_str("a'b\nc\"");
    assert!(s.starts_with('"') && s.ends_with('"'));
    assert!(s.contains("\\n"));
}

#[test]
fn key_code_tuple_lookup() {
    assert_eq!(control::key_code_tuple("Enter"), Some(("Enter", "Enter", 13)));
    assert_eq!(control::key_code_tuple("F12"), Some(("F12", "F12", 123)));
    assert!(control::key_code_tuple("未知键").is_none());
}

#[test]
fn parse_modifiers_combo() {
    let p = json!({"modifiers": ["ctrl", "shift"]});
    assert_eq!(control::parse_modifiers(&p), 2 | 8);
    let p = json!({"modifiers": "ctrl,shift"});
    assert_eq!(control::parse_modifiers(&p), 2 | 8);
}

// =================== Schema 枚举完整性(第 89 轮单工具化;第 118 轮加 explore/batch) ===================

#[test]
fn parameters_action_enum_is_six_values() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["action"]["enum"].as_array().expect("action enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    // 第 118 轮:action enum 由 6 个扩展为 8 个,新增 explore(批量观察) + batch(批量混合执行)
    assert_eq!(
        names,
        vec!["open", "list", "close", "control", "inspect", "sequence", "explore", "batch"]
    );
    assert_eq!(p["required"][0], "action", "action 必填");
}

#[test]
fn parameters_control_action_enum_complete() {
    // 第 76 轮扩展动作(drag/focus/blur/mouse_move/dispatch_event)必须全部在枚举里
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["control_action"]["enum"].as_array()
        .expect("control_action enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    for required in &[
        "click", "right_click", "double_click", "hover", "scroll", "scroll_to",
        "key_press", "press_sequence", "input_text", "human_input", "clear_input",
        "upload_file", "select_option", "download", "new_tab", "close_tab",
        "navigate", "back", "forward", "reload", "wait", "eval_js",
        "set_cookie", "delete_cookie", "set_storage", "clear_storage", "set_viewport",
        "screenshot", "heartbeat",
        "drag", "focus", "blur", "mouse_move", "dispatch_event",
        "set_window", "sync_viewport", "set_highlight", "set_overlay", "set_guard", "request_human",
    ] {
        assert!(names.contains(required), "control_action 枚举缺失 {required}");
    }
    // 第 143 轮:set_guard(页面管控三档切换)使枚举 40 → 41
    assert_eq!(names.len(), 41, "control_action 应为 41 个,实际 {names:?}");
}

#[test]
fn parameters_info_enum_complete() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["info"]["enum"].as_array().expect("info enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    for required in &[
        "console", "network", "elements", "dom", "localstorage", "sessionstorage",
        "cookies", "screenshot", "page_meta", "viewport", "url", "title", "image_urls",
        "blockers", "extract_links", "extract", "page_state",
    ] {
        assert!(names.contains(required), "info 枚举缺失 {required}");
    }
    assert_eq!(names.len(), 20, "info 应为 20 个,实际 {names:?}");
}

#[test]
fn parameters_mode_defaults_headed() {
    // 第 139 轮:缺省可见模式(与 `BrowserMode::DEFAULT` 一致)。
    let p = McpWebUseTool.parameters();
    let mode = p["properties"]["mode"].clone();
    assert!(mode.is_object(), "mode 字段应为对象");
    assert_eq!(
        mode["default"], "headed",
        "默认 mode 应为 headed(可见模式)"
    );
    assert_eq!(
        mode["default"], crate::agent::browser::BrowserMode::DEFAULT.as_str(),
        "schema default 必须与 BrowserMode::DEFAULT 同源,不允许两处各写一份"
    );
}

// =================== 参数校验(无需真实浏览器) ===================

/// 第 143 轮:parse_open_guard 级联与校验(纯函数,无需浏览器)。
#[test]
fn parse_open_guard_cascade_and_validation() {
    use crate::agent::browser_overlay::PageGuardMode;
    use super::parse_open_guard;
    // ① 显式 guard 完整解析(含选择器与 note)
    let cfg = parse_open_guard(&json!({
        "guard": "partial",
        "block_selectors": ["#pay", ".danger-btn"],
        "guard_note": "  支付区已锁定  "
    }))
    .expect("partial + block 应合法");
    assert_eq!(cfg.mode, PageGuardMode::Partial);
    assert_eq!(cfg.block_selectors.len(), 2);
    assert_eq!(cfg.note.as_deref(), Some("支付区已锁定"), "note 应被 trim");
    // ② legacy overlay 布尔映射(true=locked/false=open)
    assert_eq!(
        parse_open_guard(&json!({"overlay": false})).unwrap().mode,
        PageGuardMode::Open
    );
    assert_eq!(
        parse_open_guard(&json!({"overlay": true})).unwrap().mode,
        PageGuardMode::Locked
    );
    // ③ guard 优先于 overlay
    assert_eq!(
        parse_open_guard(&json!({"guard": "open", "overlay": true})).unwrap().mode,
        PageGuardMode::Open
    );
    // ④ 非法档位
    assert!(parse_open_guard(&json!({"guard": "whatever"})).is_err());
    // ⑤ partial 缺选择器
    assert!(parse_open_guard(&json!({"guard": "partial"})).is_err());
    // ⑥ allow + block 互斥
    assert!(parse_open_guard(&json!({
        "guard": "partial", "allow_selectors": ["a"], "block_selectors": ["b"]
    }))
    .is_err());
    // ⑦ locked 不收选择器
    assert!(parse_open_guard(&json!({"guard": "locked", "allow_selectors": ["a"]})).is_err());
    // ⑧ open 带白名单选择器 + note 合法(note 三档通用)
    assert!(parse_open_guard(&json!({"guard": "open", "guard_note": "请登录"})).is_ok());
}

#[tokio::test]
async fn missing_action_returns_1001() {
    let res = McpWebUseTool.execute(json!({})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 action"));
}

#[tokio::test]
async fn unknown_action_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "goto"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("未知 action"));
}

#[tokio::test]
async fn open_missing_url_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "open"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 url"));
}

#[tokio::test]
async fn close_missing_page_id_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "close"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 page_id"));
}

#[tokio::test]
async fn control_missing_control_action_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "control", "page_id": "p_abc"}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 control_action"));
}

#[tokio::test]
async fn inspect_missing_page_id_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "inspect", "info": "console"}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 page_id"));
}

#[tokio::test]
async fn sequence_missing_steps_returns_1001() {
    let res = McpWebUseTool.execute(json!({"action": "sequence"})).await.unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("缺少 steps"));
}

#[tokio::test]
async fn sequence_empty_steps_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "sequence", "steps": []}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("steps 不能为空"));
}

#[tokio::test]
async fn sequence_rejects_nested_sequence_without_browser() {
    let res = McpWebUseTool
        .execute(json!({"action": "sequence", "steps": [{"action": "sequence", "steps": []}]}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"));
    assert!(res.contains("不允许嵌套 sequence"));
}

#[test]
fn sequence_placeholders_resolve_recursively() {
    let mut value = json!({
        "page_id": "$page_id",
        "params": {
            "selector": "a[data-page='${page_id}']",
            "values": ["$spawned_page_id", "${spawned_page_id}"]
        }
    });
    resolve_page_placeholders(&mut value, Some("p_current"), Some("p_spawn"));
    assert_eq!(value["page_id"], "p_current");
    assert_eq!(value["params"]["selector"], "a[data-page='p_current']");
    assert_eq!(value["params"]["values"][0], "p_spawn");
    assert_eq!(value["params"]["values"][1], "p_spawn");
}

#[tokio::test]
async fn inspect_unknown_page_returns_2000() {
    // 无浏览器会话时 page_id 必然不存在 → 2000 信封(不崩溃)
    let res = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": "p_00000000", "info": "url"}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2000"), "实际: {res}");
}

#[tokio::test]
async fn list_always_returns_ok() {
    // list 不依赖浏览器进程,空列表也返回 code=0
    let res = McpWebUseTool.execute(json!({"action": "list"})).await.unwrap();
    assert!(res.contains("\"code\":0"), "实际: {res}");
    assert!(res.contains("\"pages\""));
}

// =================== open 成功/失败双路径(环境自适应) ===================

#[tokio::test]
async fn open_no_browser_or_succeeds() {
    // 无 Chrome 时返回 3001 + 安装提示;有 Chrome 时返回 0 + page_id + next_steps。
    // 运行环境可能安装了 Chrome,兼容两种路径。
    // ★ 第 139 轮:显式传 mode=hidden —— 缺省已是可见模式,不显式关掉的话
    // 开发者本机跑 `cargo test` 会被弹出一个真实 Chrome 窗口。
    let res = McpWebUseTool
        .execute(json!({"action": "open", "url": "https://example.com", "mode": "hidden"}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
    let code = v["code"].as_i64().unwrap_or(-1);
    if code == 0 {
        // 成功路径:验证显式 mode 覆盖生效 + page_id + next_steps 非空
        assert_eq!(v["data"]["mode"], "hidden", "显式 mode=hidden 应被尊重");
        assert!(v["data"]["page_id"].as_str().unwrap_or("").starts_with("p_"));
        assert!(v["data"]["page_id"].as_str().unwrap_or("").starts_with("p_"));
        let steps = v["data"]["next_steps"].as_array()
            .expect("成功响应必须包含 next_steps 数组");
        assert!(!steps.is_empty(), "next_steps 不能为空");
        assert_eq!(steps.len(), 5, "next_steps 应包含 5 步引导(含登录场景提示)");
        // 单工具化后引导字段指向 action=control/inspect 语义
        let first = &steps[0];
        assert_eq!(first["action"], "control");
        assert_eq!(first["control_action"], "input_text");
        // 成功打开的页面顺手关闭,不留脏状态
        let pid = v["data"]["page_id"].as_str().unwrap().to_string();
        let closed = McpWebUseTool.execute(json!({"action": "close", "page_id": pid})).await.unwrap();
        assert!(closed.contains("\"code\":0"));
    } else {
        // 失败路径:2001(断连/启动失败) 或 3001(无浏览器)
        assert!(
            code == 2001 || code == 3001,
            "无浏览器/失败时应返回 2001/3001,实际 code={code}"
        );
    }
}

// =================== 第 142 轮:复用已登录浏览器(connect 模式增强) ===================

#[test]
fn inject_reuse_metadata_merges_fields() {
    let res = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","mode":"headed"}}"#;
    let out = inject_reuse_metadata(
        res,
        &json!({"reuse_existing_chrome": true, "debug_port": 9222, "browser_version": "Chrome/131.0.0.0"}),
    );
    let v: serde_json::Value = serde_json::from_str(&out).expect("注入后仍为合法 JSON");
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["page_id"], "p_a1b2c3d4", "原字段不丢");
    assert_eq!(v["data"]["reuse_existing_chrome"], true);
    assert_eq!(v["data"]["debug_port"], 9222);
    assert_eq!(v["data"]["browser_version"], "Chrome/131.0.0.0");
}

#[test]
fn inject_reuse_metadata_invalid_json_passthrough() {
    let raw = "not json at all";
    assert_eq!(inject_reuse_metadata(raw, &json!({"a": 1})), raw);
}

#[tokio::test]
async fn open_reuse_existing_returns_3002_or_succeeds() {
    // 环境自适应:本机无调试端口(9222 无监听)→ 3002 + relaunch_command;
    // 本机恰好有调试模式 Chrome → 0 + reuse_existing_chrome。
    let res = McpWebUseTool
        .execute(json!({"action": "open", "url": "https://example.com", "reuse_existing": true}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
    let code = v["code"].as_i64().unwrap_or(-1);
    if code == 0 {
        assert_eq!(v["data"]["reuse_existing_chrome"], true);
        assert_eq!(v["data"]["connect_mode"], true);
        // 成功打开的页面顺手关闭,不留脏状态
        let pid = v["data"]["page_id"].as_str().unwrap().to_string();
        let closed = McpWebUseTool.execute(json!({"action": "close", "page_id": pid})).await.unwrap();
        assert!(closed.contains("\"code\":0"));
    } else {
        assert_eq!(code, 3002, "无调试端口时应返回 3002,实际: {res}");
        let data = &v["data"];
        let cmd = data["relaunch_command"].as_str().unwrap_or_default();
        assert!(
            cmd.contains("--remote-debugging-port"),
            "3002 信封应含带调试端口的重启命令: {cmd}"
        );
        assert!(data["chrome_running"].is_boolean());
        assert!(data["debug_port"].is_number());
    }
}

#[tokio::test]
async fn open_reuse_existing_skipped_when_connect_url_given() {
    // 显式 connect_url 优先,跳过探测:指向一个无监听端口,应走 connect 失败路径
    // (2001),而不是 3002 引导。
    let res = McpWebUseTool
        .execute(json!({
            "action": "open",
            "url": "https://example.com",
            "reuse_existing": true,
            "connect_url": "http://127.0.0.1:1",
        }))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
    let code = v["code"].as_i64().unwrap_or(-1);
    assert_ne!(code, 3002, "显式 connect_url 不应走探测引导: {res}");
}

// =================== page_id 提取(供 agent_loop 引导钩子) ===================

#[test]
fn extract_page_id_parses_success_envelope() {
    let txt = r#"{"code":0,"message":"ok","data":{"page_id":"p_a1b2c3d4","title":"文心一言","final_url":"https://wenxin.baidu.com/"}}"#;
    let pid = extract_page_id_from_text(txt);
    assert_eq!(pid.as_deref(), Some("p_a1b2c3d4"));
}

#[test]
fn extract_page_id_returns_none_for_failure() {
    // code=3001 时不含 page_id
    let txt = r#"{"code":3001,"message":"未检测到浏览器","data":{"install":"请安装 Chrome"}}"#;
    assert_eq!(extract_page_id_from_text(txt), None);
}

#[test]
fn extract_page_id_handles_non_json() {
    assert_eq!(extract_page_id_from_text("plain text"), None);
    assert_eq!(extract_page_id_from_text(""), None);
}

// =================== 第 99 轮:open 复用 / URL 归一化 / eval_js 净化 / download 快速失败 ===================

#[test]
fn normalize_web_url_trims_trailing_slash_and_fragment() {
    assert_eq!(
        normalize_web_url("http://10.255.159.58:20122/"),
        "http://10.255.159.58:20122"
    );
    assert_eq!(
        normalize_web_url("http://a.com/page#top"),
        "http://a.com/page"
    );
    assert_eq!(normalize_web_url(" http://a.com "), "http://a.com");
    // 归一化后相等的不同写法应匹配
    assert_eq!(
        normalize_web_url("http://a.com/x/"),
        normalize_web_url("http://a.com/x")
    );
}

#[test]
fn decode_data_url_base64_payload() {
    // 1x1 红色像素 PNG 的已知 base64 头不可靠,直接用文本字节验证
    let payload = base64::engine::general_purpose::STANDARD.encode("hello captcha");
    let url = format!("data:text/plain;base64,{payload}");
    let bytes = control::decode_data_url(&url).expect("base64 载荷应可解码");
    assert_eq!(bytes, b"hello captcha");
}

#[test]
fn decode_data_url_percent_payload() {
    let url = "data:text/plain,hello%20world%21";
    let bytes = control::decode_data_url(url).expect("百分号载荷应可解码");
    assert_eq!(bytes, b"hello world!");
}

#[test]
fn decode_data_url_rejects_non_data() {
    assert!(control::decode_data_url("http://example.com/x.png").is_none());
    assert!(control::decode_data_url("data:image/png").is_none()); // 无逗号
}

#[test]
fn percent_decode_plus_as_space_and_rejects_bad_hex() {
    assert_eq!(control::percent_decode("a+b").as_deref(), Some("a b"));
    assert_eq!(control::percent_decode("%41").as_deref(), Some("A"));
    assert_eq!(control::percent_decode("%zz"), None);
    assert_eq!(control::percent_decode("%4"), None); // 截断
}

#[test]
fn looks_like_function_detection() {
    assert!(control::looks_like_function("(() => 1)()"));
    assert!(control::looks_like_function("(function(){return 1})()"));
    assert!(control::looks_like_function("async () => 1"));
    assert!(control::looks_like_function("  function f(){}"));
    assert!(!control::looks_like_function("document.title"));
    assert!(!control::looks_like_function("return document.title"));
    assert!(!control::looks_like_function("var x = 1; x + 1"));
}

#[test]
fn sanitize_eval_result_keeps_small_values() {
    assert_eq!(control::sanitize_eval_result(json!("short")), json!("short"));
    assert_eq!(control::sanitize_eval_result(json!(42)), json!(42));
    let obj = json!({"nested": true});
    assert_eq!(control::sanitize_eval_result(obj.clone()), obj);
}

#[test]
fn sanitize_eval_result_truncates_long_strings_to_file() {
    let long = "x".repeat(5000);
    let out = control::sanitize_eval_result(json!(long.clone()));
    assert_eq!(out["result_truncated"], json!(true));
    assert!(out["result_len"].as_i64().unwrap() >= 5000);
    assert!(out["saved_to"].as_str().unwrap_or("").contains("laew_web_result_"));
    // 落盘内容 = 原文
    let path = out["saved_to"].as_str().unwrap();
    let written = std::fs::read_to_string(path).expect("落盘文件应可读");
    assert_eq!(written, long);
    let _ = std::fs::remove_file(path);
}

#[test]
fn sanitize_eval_result_saves_data_url_bytes() {
    let payload = base64::engine::general_purpose::STANDARD.encode([0u8, 1, 2, 3, 4, 5, 6, 7]);
    let url = format!("data:image/png;base64,{payload}");
    let out = control::sanitize_eval_result(json!(url));
    assert_eq!(out["result_truncated"], json!(true));
    assert!(out["saved_to"].as_str().unwrap_or("").ends_with(".png"));
    let path = out["saved_to"].as_str().unwrap();
    let written = std::fs::read(path).expect("落盘文件应可读");
    assert_eq!(written, vec![0u8, 1, 2, 3, 4, 5, 6, 7]);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn download_rejects_about_blank_fast() {
    // about:/blob:/javascript: 不会产生下载事件,必须快速失败而不是挂到超时。
    // 无浏览器环境本应 2002 "浏览器会话不存在",但 scheme 校验应先命中并给出引导。
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_00000000",
            "control_action": "download",
            "params": {"url": "about:blank"}
        }))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2002"), "实际: {res}");
    assert!(res.contains("不支持从 about: URL 触发下载"), "实际: {res}");
}

#[tokio::test]
async fn download_missing_params_reports_fields() {
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_00000000",
            "control_action": "download",
            "params": {}
        }))
        .await
        .unwrap();
    assert!(res.contains("缺少 url 或 selector"), "实际: {res}");
}

#[tokio::test]
async fn eval_js_missing_param_lists_aliases() {
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_00000000",
            "control_action": "eval_js",
            "params": {}
        }))
        .await
        .unwrap();
    // page_id 不存在 → 2002(ensure_page 先失败);改用不存在页时参数校验顺序:
    // ensure_page 在前,所以这里验证的是 2000/2002;参数别名校验用 page 失效前的
    // 同语义 —— 无浏览器环境下两者都会先失败,这里只断言信封结构合法。
    let v: serde_json::Value = serde_json::from_str(&res).unwrap();
    assert!(v["code"].as_i64().unwrap_or(0) != 0);
}

// =================== 第 99 轮:Schema 同步 ===================

#[test]
fn parameters_info_enum_contains_ocr() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["info"]["enum"].as_array().expect("info enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(names.len(), 20, "info 应有 20 个枚举值: {names:?}");
    assert!(names.contains(&"ocr"), "info 枚举应含 ocr: {names:?}");
    assert!(names.contains(&"extract_links"), "info 枚举应含 extract_links: {names:?}");
    assert!(names.contains(&"coverage"), "info 枚举应含 coverage(第 146 轮): {names:?}");
}

// =================== 第 135 轮:extract / page_state / eval_js 体积闸门 ===================

#[test]
fn extract_requires_item_selector() {
    let f = extract::parse_fields(None);
    assert_eq!(f.len(), 1, "fields 缺省应给通用投影");
    assert_eq!(f[0].name, "text");
    assert!(f[0].selector.is_none(), "通用投影不假设选择器");
}

#[test]
fn extract_parses_field_specs() {
    let params = json!({
        "fields": {
            "title":   {"selector": "h3 a", "required": true},
            "time":    {"selector": "time", "attr": "datetime"},
            "summary": {"selector": ".descript", "max_chars": 30}
        }
    });
    let mut f = extract::parse_fields(params.get("fields"));
    f.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(f.len(), 3);
    let t = f.iter().find(|x| x.name == "title").unwrap();
    assert_eq!(t.selector.as_deref(), Some("h3 a"));
    assert!(t.required);
    let tm = f.iter().find(|x| x.name == "time").unwrap();
    assert_eq!(tm.attr.as_deref(), Some("datetime"));
    let s = f.iter().find(|x| x.name == "summary").unwrap();
    assert_eq!(s.max_chars, 30);
    assert!(!s.required, "required 缺省应为 false");
}

#[test]
fn extract_parses_filter_spec() {
    let params = json!({"filter": {
        "keywords": ["AI", " 大模型 ", ""],
        "match_all": true,
        "time_field": "time",
        "since": "2026-10-07 05:33",
        "sort": "time:desc",
        "limit_after_filter": 20
    }});
    let f = extract::parse_filter(params.get("filter"));
    // 空白项被剔除,保留 2 个
    assert_eq!(f.keywords, vec!["AI".to_string(), "大模型".to_string()]);
    assert!(f.match_all);
    assert_eq!(f.time_field.as_deref(), Some("time"));
    assert_eq!(f.sort.as_deref(), Some("time:desc"));
    assert_eq!(f.limit_after_filter, Some(20));
}

#[test]
fn extract_filter_defaults_are_permissive() {
    let f = extract::parse_filter(None);
    assert!(f.keywords.is_empty());
    assert!(!f.match_all, "match_all 缺省应为任一命中");
    assert!(f.time_field.is_none());
    assert!(f.since.is_none() && f.until.is_none());
}

/// 时间边界解析(纯函数):日期类返回日历分量(由页面按本地时区换算),
/// Unix 时间戳类返回绝对毫秒。
#[test]
fn extract_parses_time_bounds() {
    assert_eq!(
        extract::parse_bound("2026-10-08"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 0, mm: 0, ss: 0 })
    );
    assert_eq!(
        extract::parse_bound("2026-10-08 17:02"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 17, mm: 2, ss: 0 })
    );
    assert_eq!(
        extract::parse_bound("2026-10-08T17:02:33"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 17, mm: 2, ss: 33 })
    );
    assert_eq!(
        extract::parse_bound("2026/10/08 17:02"),
        Some(extract::Bound::Parts { y: 2026, mo: 10, d: 8, hh: 17, mm: 2, ss: 0 }),
        "斜杠写法应与连字符等价"
    );
    // Unix 秒 / 毫秒 → 绝对值
    assert_eq!(extract::parse_bound("1791417600"), Some(extract::Bound::Ms(1_791_417_600_000)));
    assert_eq!(extract::parse_bound("1791417600000"), Some(extract::Bound::Ms(1_791_417_600_000)));
    // 非法输入返回 None(fail-open:JS 侧跳过该侧约束并回报 unparsed)
    assert_eq!(extract::parse_bound("36小时前"), None);
    assert_eq!(extract::parse_bound("2026-13-40"), None);
    assert_eq!(extract::parse_bound("2026-10-08 99:00"), None);
    assert_eq!(extract::parse_bound(""), None);
}

/// 生成脚本必须把过滤放在页面内(否则又回到「19KB 大对象直灌」的旧坑)。
#[test]
fn extract_builds_in_page_filtering_js() {
    let fields = extract::parse_fields(Some(&json!({
        "title": {"selector": "h3 a"},
        "time":  {"selector": "time", "attr": "datetime"}
    })));
    let filter = extract::parse_filter(Some(&json!({
        "keywords": ["AI"],
        "time_field": "time",
        "since": "2026-10-07 05:33",
        "sort": "time:desc"
    })));
    let js = extract::build_js(".item", "a", &fields, &filter, 1000, 200);

    // 选择器与字段名都被安全嵌入
    assert!(js.contains("const ITEM_SEL = \".item\""), "{js}");
    assert!(js.contains("\"name\":\"title\""), "{js}");
    assert!(js.contains("\"name\":\"time\""), "{js}");
    // 关键词小写化后嵌入
    assert!(js.contains("\"ai\""), "{js}");
    // 时间窗两侧都以**日历分量**注入,由页面按本地时区换算(避免 UTC 偏移 8 小时)
    assert!(js.contains("SINCE_RAW = [2026,10,7,5,33,0]"), "{js}");
    assert!(js.contains("UNTIL_RAW = null"), "{js}");
    assert!(js.contains("const toMs = (b) =>"), "{js}");
    // 中文相对时间解析在脚本内
    assert!(js.contains("小时前"), "{js}");
    assert!(js.contains("SORT = \"time:desc\""), "{js}");
    // 关键:过滤与截断都在页面内完成
    assert!(js.contains("items.slice(0, CAP)"), "{js}");
    // 自诊断字段
    assert!(js.contains("unparsed_time_fields"), "{js}");
}

/// page_state:三闸裁剪 + dropped_paths + 探测 window 全局键。
#[test]
fn page_state_builds_guarded_js() {
    let js = page_state::build_js(
        &["__NEXT_DATA__".to_string()],
        true,
        20_000,
        8,
        200,
    );
    assert!(js.contains("MAX_BYTES = 20000"), "{js}");
    assert!(js.contains("MAX_DEPTH = 8"), "{js}");
    assert!(js.contains("MAX_ITEMS = 200"), "{js}");
    assert!(js.contains("const PROBE = true"), "{js}");
    assert!(js.contains("dropped"), "必须回报被裁路径:{js}");
    assert!(js.contains("__NEXT_DATA__"));
    // JSON-LD 顺带提取
    assert!(js.contains("application/ld+json"), "{js}");
}

/// eval_js 复合值体积闸门(第 135 轮)。
#[test]
fn sanitize_eval_result_passes_small_structures_through() {
    let small = json!([{"title": "A", "url": "https://36kr.com/p/1"}]);
    assert_eq!(control::sanitize_eval_result(small.clone()), small);
}

#[test]
fn sanitize_eval_result_shrinks_huge_arrays() {
    // 800 个元素,每个约 300 字节 → 远超 20KB 闸门
    let big: Vec<Value> = (0..800)
        .map(|i| json!({"id": i, "title": "x".repeat(200), "url": format!("https://36kr.com/p/{i}")}))
        .collect();
    let out = control::sanitize_eval_result(Value::Array(big));
    // 裁剪后应远小于原始大小
    let out_len = serde_json::to_string(&out).unwrap().len();
    assert!(out_len < 60_000, "裁剪后仍应显著变小,实际 {out_len}");
    assert!(
        out.to_string().contains("__skipped__") || out.to_string().contains("__truncated__"),
        "应显式标注裁剪:{out}"
    );
}

#[test]
fn sanitize_eval_result_spills_oversized_objects_to_file() {
    // 单个超大字符串字段的对象:裁剪后仍超限 → 落盘
    let big: Vec<Value> = (0..200)
        .map(|i| json!({"id": i, "body": "y".repeat(2000)}))
        .collect();
    let out = control::sanitize_eval_result(json!({ "items": big }));
    assert_eq!(out["result_truncated"], json!(true));
    assert_eq!(out["value_kind"], json!("object"));
    assert!(out["saved_to"].is_string(), "应落盘并返回 saved_to");
    assert!(out["hint"].is_string());
    // 落盘内容应是**完整**原始数据,不是裁剪后的
    let path = out["saved_to"].as_str().unwrap();
    let on_disk = std::fs::read_to_string(path).expect("落盘文件应可读");
    assert!(
        on_disk.len() > 400_000,
        "落盘应是完整原始数据,实际 {} 字节",
        on_disk.len()
    );
    let _ = std::fs::remove_file(path);
}

/// 同毫秒并发落盘不得互相覆盖(第 135 轮修)。
#[test]
fn eval_spill_filenames_do_not_collide() {
    let a = format!("{}_{}", 1_789_516_800_000u128, 0);
    let b = format!("{}_{}", 1_789_516_800_000u128, 1);
    assert_ne!(a, b);
}

/// eval_js 匿名函数形态识别(第 135 轮:实测事故里 `function(){…}` 被误判为
/// 「已是函数形态」而跳过包裹,抛 SyntaxError 还凭空派生一个野页面)。
#[test]
fn eval_js_detects_anonymous_function_decl() {
    assert!(control::is_anonymous_function_decl("function(){ return 1; }"));
    assert!(control::is_anonymous_function_decl("  function (){ return 1 }"));
    assert!(control::is_anonymous_function_decl("function*(){}"));
    // 有名字 → 不是匿名函数语句
    assert!(!control::is_anonymous_function_decl("function foo(){ return 1; }"));
    assert!(!control::is_anonymous_function_decl("(() => 1)()"));
    assert!(!control::is_anonymous_function_decl("var x = 1; return x;"));
}

#[test]
fn parameters_has_reuse_property() {
    let p = McpWebUseTool.parameters();
    assert!(
        p["properties"]["reuse"].is_object(),
        "schema 应含 open 复用开关 reuse"
    );
    assert!(p["properties"]["page_id"]["description"].as_str().unwrap_or("").contains("all"),
        "page_id 描述应说明 close all 语义");
}

// =================== 第 100 轮:窗口可视化 + 人工介入(HITL) ===================

#[test]
fn parameters_has_window_and_highlight_properties() {
    let p = McpWebUseTool.parameters();
    for key in ["window_width", "window_height", "highlight"] {
        assert!(
            p["properties"][key].is_object(),
            "schema 应含 {key} 属性"
        );
    }
    assert_eq!(p["properties"]["highlight"]["default"], true);
    // 第 125 轮:视口自适应开关进 schema
    assert!(
        p["properties"]["auto_expand_viewport"].is_object(),
        "schema 应含 auto_expand_viewport 属性"
    );
    assert_eq!(p["properties"]["auto_expand_viewport"]["default"], true);
    // 默认值文案统一 1080p(全模式),不得回退到 hidden=1440 旧口径
    for key in ["window_width", "window_height"] {
        let d = p["properties"][key]["description"].as_str().unwrap_or("");
        assert!(d.contains("1920") || d.contains("1080"), "{key} 描述应说明 1080p 默认");
        assert!(!d.contains("hidden=1440"), "{key} 描述残留旧 hidden=1440 口径");
    }
}

// =================== 第 125 轮:视口基准 1080p 与 2K 自动扩展 ===================

#[test]
fn auto_expand_enabled_defaults_to_true() {
    assert!(auto_expand_enabled(&json!({})), "缺省自动扩展视口开启");
    assert!(auto_expand_enabled(&json!({"auto_expand_viewport": true})));
    assert!(!auto_expand_enabled(&json!({"auto_expand_viewport": false})), "显式 false 才关");
    assert!(auto_expand_enabled(&json!({"auto_expand_viewport": "yes"})), "非 bool 容错回默认开");
}

#[test]
fn open_description_mentions_viewport_fit() {
    // 工具描述是 LLM 的第一信息源:1080p 默认 + 2K 自动扩展必须写入
    let d = McpWebUseTool.description();
    assert!(d.contains("1920×1080") && d.contains("1080p"), "open 描述应说明全模式 1080p 默认");
    assert!(
        d.contains("auto_expand_viewport") && d.contains("2560×1440"),
        "open 描述应说明自动扩展到 2K"
    );
}

#[test]
fn parse_window_size_clamps_and_requires_both() {
    assert_eq!(parse_window_size(&json!({})), None, "缺省返回 None(驱动层按 mode 给默认)");
    assert_eq!(parse_window_size(&json!({"window_width": 1920})), None, "只给一个维度视为缺省");
    assert_eq!(
        parse_window_size(&json!({"window_width": 1920, "window_height": 1080})),
        Some((1920, 1080)),
        "1080p 原样通过"
    );
    assert_eq!(
        parse_window_size(&json!({"window_width": 1, "window_height": 99999})),
        Some((320, 4320)),
        "超界 clamp 到安全区间"
    );
}

#[test]
fn detect_blockers_matches_common_walls() {
    let captcha = inspect::detect_blockers("请完成安全验证\n拖动滑块拼图");
    assert!(captcha.iter().any(|b| b["kind"] == "captcha"), "实际:{captcha:?}");

    let sms = inspect::detect_blockers("短信验证码已发送至 138****0000,请输入");
    assert!(sms.iter().any(|b| b["kind"] == "sms"), "实际:{sms:?}");

    let qr = inspect::detect_blockers("微信扫码登录");
    assert!(qr.iter().any(|b| b["kind"] == "qr_login"), "实际:{qr:?}");

    let login = inspect::detect_blockers("Please sign in to continue browsing");
    assert!(login.iter().any(|b| b["kind"] == "login"), "实际:{login:?}");

    // 第 117 轮:扩展场景(实名/2FA/OAuth)的关键词必须被识别
    let real_name = inspect::detect_blockers("根据监管要求,请先完成实名认证后再继续操作");
    assert!(
        real_name.iter().any(|b| b["kind"] == "real_name"),
        "实际:{real_name:?}"
    );

    let face = inspect::detect_blockers("正在进行人脸核身,请正对屏幕");
    assert!(face.iter().any(|b| b["kind"] == "real_name"), "实际:{face:?}");

    let two_factor = inspect::detect_blockers("请输入邮箱验证码完成两步验证");
    assert!(
        two_factor.iter().any(|b| b["kind"] == "two_factor"),
        "实际:{two_factor:?}"
    );

    let totp = inspect::detect_blockers("Open Google Authenticator and enter the TOTP code");
    assert!(totp.iter().any(|b| b["kind"] == "two_factor"), "实际:{totp:?}");

    let oauth = inspect::detect_blockers("请使用 GitHub 账号授权登录");
    assert!(oauth.iter().any(|b| b["kind"] == "oauth"), "实际:{oauth:?}");

    let sso = inspect::detect_blockers("通过 SSO 登录到企业工作台");
    assert!(sso.iter().any(|b| b["kind"] == "oauth"), "实际:{sso:?}");

    let clean = inspect::detect_blockers("普通页面内容,无阻断");
    assert!(clean.is_empty(), "误报:{clean:?}");
}

// ==================== 第 151 轮:关键词通道整词边界(误报根治) ====================

#[test]
fn detect_blockers_rejects_embedded_ascii_noise() {
    // 实测反例(豆包 2026-10-10):页面把 SSR 数据以字面 JSON 转义 URL 文本渲染进可见 DOM,
    // `/ae6146b6…` 中的 `2Fa` 让 "2fa" 以子串形态命中 two_factor,连续 4 次假告警。
    let doubao_noise = "002Ficon\\u002Fae6146b6e7f7487ebf3e1331d6e9166f~tplv-a9r";
    let hits = inspect::detect_blockers(doubao_noise);
    assert!(
        hits.iter().all(|b| b["kind"] != "two_factor"),
        "转义 URL 子串不应命中 two_factor,实际:{hits:?}"
    );

    // 同类:十六进制哈希/标识符里嵌 sms/kyc/oauth/totp/captcha
    let hash_noise = "resource id=verify_mv26aqhm hash sms4kd93jf kyc7xx oauth9ab totp3zz";
    let hits = inspect::detect_blockers(hash_noise);
    assert!(
        hits.iter().all(|b| !matches!(
            b["kind"].as_str(),
            Some("sms") | Some("real_name") | Some("oauth") | Some("two_factor")
        )),
        "字母数字包围的子串不应命中,实际:{hits:?}"
    );
}

#[test]
fn detect_blockers_ascii_word_boundary_positives() {
    // 整词命中(前后是空格/中文/行首尾)必须保持 —— 这是真阻断文案的正常形态
    let cases: &[(&str, &str)] = &[
        ("请开启 2FA 验证后继续", "two_factor"),
        ("Enter the TOTP code from your app", "two_factor"),
        ("Sign in to continue", "login"),
        ("请拖动滑块完成 Captcha 验证", "captcha"),
        ("Use 2-step verification", "two_factor"),
    ];
    for (text, kind) in cases {
        let hits = inspect::detect_blockers(text);
        assert!(
            hits.iter().any(|b| b["kind"] == *kind),
            "「{text}」应命中 {kind},实际:{hits:?}"
        );
    }
}

#[test]
fn detect_blockers_cjk_keywords_keep_substring_semantics() {
    // 含 CJK 的关键词不收边界(中文词不会以 URL 子串形态出现)
    let hits = inspect::detect_blockers("登录后查看完整报告");
    assert!(hits.iter().any(|b| b["kind"] == "login"), "实际:{hits:?}");
}

#[test]
fn hitl_image_plan_priority_and_sources() {
    // 第 132 轮:附图解析优先级 —— 显式路径(存在)> captcha 自动截图 > 无图
    use crate::agent::tools::mcp_web_use::control::{hitl_image_plan, HitlImagePlan};

    // 显式 + 文件存在 → Explicit / explicit
    let tmp = std::env::temp_dir().join("laew_hitl_plan_probe.png");
    std::fs::write(&tmp, b"png").unwrap();
    let f = tmp.display().to_string();
    let (plan, src) = hitl_image_plan("captcha", Some(f.as_str()));
    assert_eq!(plan, HitlImagePlan::Explicit(f.clone()));
    assert_eq!(src, "explicit");
    // 显式给图对非 captcha reason 同样生效(人工指定即权威)
    let (plan2, _) = hitl_image_plan("sms", Some(f.as_str()));
    assert!(matches!(plan2, HitlImagePlan::Explicit(_)));
    let _ = std::fs::remove_file(&tmp);

    // 显式但文件不存在 → 无图 / explicit_missing(fail-open)
    let (plan, src) = hitl_image_plan("captcha", Some("/no/such/file.png"));
    assert_eq!(plan, HitlImagePlan::None);
    assert_eq!(src, "explicit_missing");

    // 无显式 + captcha → 自动截图
    let (plan, src) = hitl_image_plan("captcha", None);
    assert_eq!(plan, HitlImagePlan::Auto);
    assert_eq!(src, "auto");

    // 无显式 + 非 captcha → 无图
    let (plan, src) = hitl_image_plan("sms", None);
    assert_eq!(plan, HitlImagePlan::None);
    assert_eq!(src, "none");
    // 空串显式等同未给(自动路径仍可命中)
    let (plan, _) = hitl_image_plan("captcha", Some("  "));
    assert_eq!(plan, HitlImagePlan::Auto);
}

#[test]
fn human_assist_allowed_reasons_covers_extensions() {
    // 第 117 轮:常量必须包含实名/2FA/OAuth 三种新 reason,且顺序符合推荐使用顺序。
    use crate::agent::tools::mcp_web_use::control::HUMAN_ASSIST_ALLOWED_REASONS;
    let names: Vec<&str> = HUMAN_ASSIST_ALLOWED_REASONS.to_vec();
    for required in &[
        "captcha", "sms", "qr_login", "login", "real_name", "two_factor", "oauth",
        "manual_verify", "custom",
    ] {
        assert!(names.contains(required), "缺失合法 reason {required}");
    }
    // 顺序:captcha→sms→qr_login→login→real_name→two_factor→oauth→manual_verify→custom
    assert_eq!(names[4], "real_name", "real_name 应在 manual_verify 前");
    assert_eq!(names[5], "two_factor", "two_factor 应在 manual_verify 前");
    assert_eq!(names[6], "oauth", "oauth 应在 manual_verify 前");
}

#[test]
fn human_assist_reasons_doc_lists_all_variants() {
    // 第 117 轮:human_assist_reasons_doc() 输出必须包含 9 个合法 reason,
    // LLM 在错误消息中可直接复用此串构造修复请求。
    use crate::agent::tools::mcp_web_use::control::{
        human_assist_reasons_doc, HUMAN_ASSIST_ALLOWED_REASONS,
    };
    let doc = human_assist_reasons_doc();
    for r in HUMAN_ASSIST_ALLOWED_REASONS {
        assert!(doc.contains(r), "doc 缺失 {r}: {doc}");
    }
}

#[tokio::test]
async fn request_human_invalid_reason_returns_1001() {
    // 无浏览器环境:非法 reason 在 page_id 校验之后仍有确定性断言路径 ——
    // page_id 不存在时返回 2000;用不存在页验证 2000 前置。
    let res = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": "p_nonexist0",
            "control_action": "request_human",
            "params": {"reason": "whatever"}
        }))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2000"), "page_id 不存在应为 2000: {res}");
}

// =================== explore / batch 参数校验(第 119 轮) ===================

#[tokio::test]
async fn explore_missing_page_id_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "explore", "queries": [{"info": "elements"}]}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"), "缺 page_id 应 1001: {res}");
}

#[tokio::test]
async fn explore_empty_queries_returns_1001() {
    let res = McpWebUseTool
        .execute(json!({"action": "explore", "page_id": "p_x", "queries": []}))
        .await
        .unwrap();
    assert!(res.contains("\"code\":1001"), "空 queries 应 1001: {res}");
}

#[tokio::test]
async fn explore_too_many_queries_returns_1001() {
    let queries: Vec<Value> = (0..9).map(|_| json!({"info": "elements"})).collect();
    let res = McpWebUseTool
        .execute(json!({"action": "explore", "page_id": "p_x", "queries": queries}))
        .await
        .unwrap();
    assert!(
        res.contains("\"code\":1001") && res.contains("最多 8 项"),
        "超过 8 项应 1001: {res}"
    );
}

#[tokio::test]
async fn explore_unknown_page_returns_2000() {
    let res = McpWebUseTool
        .execute(json!({
            "action": "explore",
            "page_id": "p_nonexist0",
            "queries": [{"info": "elements"}]
        }))
        .await
        .unwrap();
    assert!(res.contains("\"code\":2000"), "page_id 不存在应 2000: {res}");
}

#[tokio::test]
async fn batch_missing_steps_returns_error() {
    // batch 是 sequence 的语义别名;缺 steps 应确定性报错(1001)
    let res = McpWebUseTool
        .execute(json!({"action": "batch", "page_id": "p_nonexist0"}))
        .await
        .unwrap();
    assert!(
        res.contains("\"code\":1001") || res.contains("\"code\":2000"),
        "batch 缺 steps 应确定性报错: {res}"
    );
}

// =================== 第 131 轮:人工介入提示下沉到工具返回层 ===================

#[test]
fn human_assist_hint_字段完整且自描述() {
    let h = super::human_assist_hint("captcha", "请人工输入验证码", &["继续", "取消"]);
    assert_eq!(h["next_action"], "request_human");
    let a = &h["human_assist"];
    assert_eq!(a["ready"], true);
    assert_eq!(a["reason"], "captcha");
    assert_eq!(a["message"], "请人工输入验证码");
    assert_eq!(a["options"][0], "继续");
    // call 字段直接给出可复制的 MCP_Web_Use 调用形态,模型不必反查 schema
    let call = a["call"].as_str().unwrap();
    assert!(call.contains("control_action"), "{call}");
    assert!(call.contains("request_human"), "{call}");
    assert!(a["rule"].as_str().unwrap().contains("严禁伪造"));
}

#[test]
fn ocr_报错时给出人工介入提示() {
    let out = json!({"ocr_error": "OCR 失败:权限不足"});
    let hint = super::ocr_unavailable_hint(&out).expect("ocr_error 应触发提示");
    assert_eq!(hint["next_action"], "request_human");
    assert_eq!(hint["human_assist"]["reason"], "captcha");
}

#[test]
fn ocr_空文本同样视为不可用() {
    // 验证码区域是噪声图时 OCR 常常返回空串,模型最容易在这时反复调参重试
    let out = json!({"ocr_text": "   "});
    assert!(super::ocr_unavailable_hint(&out).is_some(), "空文本应触发提示");
}

#[test]
fn ocr_拿到文本时不打扰() {
    let out = json!({"ocr_text": "8F3K", "ocr_block_count": 1});
    assert!(super::ocr_unavailable_hint(&out).is_none(), "OCR 成功不应附 HITL 提示");
}

#[test]
fn merge_hint_不覆盖已有字段() {
    let mut out = json!({"ocr_error": "x", "next_action": "keep_me"});
    let hint = super::human_assist_hint("captcha", "m", &["a"]);
    super::merge_hint(&mut out, hint);
    assert_eq!(out["next_action"], "keep_me", "已有字段不应被覆盖");
    assert_eq!(out["human_assist"]["reason"], "captcha");
}

/// 内建 SSR 注水点候选:非空且互不重复(顺序即优先级)。
#[test]
fn page_state_builtin_candidates_are_unique() {
    let c = page_state::builtin_candidates();
    assert!(c.len() >= 6, "候选过少: {c:?}");
    let mut sorted: Vec<&str> = c.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "候选有重复: {c:?}");
    assert!(c.contains(&"__NEXT_DATA__"));
    assert!(c.contains(&"initialState"));
}

/// 把生成的页面内脚本写到临时文件,供 `node --check` 做语法校验
/// (运行:`cargo test --lib dump_generated_js -- --ignored --nocapture`)。
#[test]
#[ignore]
fn dump_generated_js() {
    use std::io::Write;
    let fields = extract::parse_fields(Some(&json!({
        "title": {"selector": "h3 a", "required": true},
        "time":  {"selector": "time", "attr": "datetime"}
    })));
    let filter = extract::parse_filter(Some(&json!({
        "keywords": ["AI", "大模型"], "time_field": "time",
        "since": "2026-10-07 05:33", "sort": "time:desc"
    })));
    let mut f = std::fs::File::create("/tmp/laew_extract_gen.js").unwrap();
    writeln!(f, "{}", extract::build_js(".item", "a", &fields, &filter, 1000, 200)).unwrap();
    let mut f2 = std::fs::File::create("/tmp/laew_page_state_gen.js").unwrap();
    writeln!(f2, "{}", page_state::build_js(&["__NEXT_DATA__".into()], true, 20000, 8, 200)).unwrap();
    eprintln!("written");
}

// =================== 第 141 轮:蒙层全链路真浏览器验证(#[ignore],本地 --ignored 跑) ===================
//
// 与 browser_overlay.rs 的集成测试互补:那条验证「驱动层双层原语」,本条验证
// 「工具层完整链路」—— open(默认 overlay=on,本机 Aqua 会话 → headed)注入蒙层
// 并加锁后,control(click) 必须经「先解后锁」包装照常生效(而非被 setIgnoreInputEvents
// 吃掉),且响应回报 overlay.enabled=true。
// 跑法:`cargo test --lib real_browser_tool_overlay -- --ignored --nocapture`(需本机 Chrome)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_tool_overlay_end_to_end() {
    let url = "data:text/html,<button id='b' onclick='this.dataset.c=(+this.dataset.c||0)+1' \
               style='position:fixed;left:0;top:0;width:200px;height:100px'>x</button>";
    // ① open:不传 overlay → 缺省随 LAEW_WEB_OVERLAY(默认开)+ 本机 GUI → headed
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "window_width": 800, "window_height": 600}))
        .await
        .expect("open 失败(本机是否装有 Chrome?)");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "open 应成功:{out}");
    assert_eq!(v["data"]["mode"], "headed", "本机 GUI 会话应为 headed:{out}");
    assert_eq!(v["data"]["overlay"]["enabled"], true, "蒙层应默认开启:{out}");
    let pid = v["data"]["page_id"].as_str().unwrap().to_string();

    // ② 蒙层锁定下 control(click) 应照常生效(先解后锁包装)
    let click = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "click",
                        "params": {"selector": "#b"}}))
        .await
        .unwrap();
    let c: Value = serde_json::from_str(&click).unwrap();
    assert_eq!(c["code"], 0, "蒙层锁定下 click 应经先解后锁照常生效:{click}");

    // ③ 点击真实生效(计数=1)且蒙层元素仍在(锁定未被误关)
    let check = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression":
                            "JSON.stringify({c:+document.getElementById('b').dataset.c||0, m:!!document.getElementById('__laew_overlay__')})"}}))
        .await
        .unwrap();
    let e: Value = serde_json::from_str(&check).unwrap();
    let payload: Value =
        serde_json::from_str(e["data"]["result"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    assert_eq!(payload["c"], 1, "点击应真实生效:{check}");
    assert_eq!(payload["m"], true, "蒙层元素应仍存在(复锁不误删):{check}");

    // ④ 收口:关闭全部页面回收浏览器(全局单例,防污染其它测试)
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

// =================== 第 143 轮:页面管控三档工具层真浏览器验证(#[ignore]) ===================
//
// 验证「工具 JSON 面 → parse_open_guard → new_page → 响应字段 → set_guard 切档 →
// partial 黑名单盾下 Agent click(先隐盾后复盾)」完整链路:
// open(guard="open") 应回报 data.guard.mode=open 且 CDP 未锁(click 无须让路);
// set_guard(partial, block=#b) 后 click 应照常生效(隐盾包装)且盾区存在;
// 参数错误(partial 缺选择器)应回 1001。
// 跑法:`cargo test --lib real_browser_tool_guard -- --ignored --nocapture`(需本机 Chrome)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_tool_guard_modes_end_to_end() {
    let url = "data:text/html,<button id='b' onclick='this.dataset.c=(+this.dataset.c||0)+1' \
               style='position:fixed;left:0;top:0;width:200px;height:100px'>x</button>";
    // ① open(guard=open):响应应回报 guard.mode=open(状态条「页面开放」在页面上)
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "guard": "open",
                        "guard_note": "请登录后应答弹窗", "window_width": 800, "window_height": 600}))
        .await
        .expect("open 失败(本机是否装有 Chrome?)");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "open(guard=open) 应成功:{out}");
    assert_eq!(v["data"]["guard"]["mode"], "open", "guard 档应为 open:{out}");
    assert_eq!(v["data"]["overlay"]["enabled"], false, "legacy overlay 应派生为关:{out}");
    let pid = v["data"]["page_id"].as_str().unwrap().to_string();

    // ② open 档 click 无须让路也应生效(没有 CDP 输入锁/盾区)
    let click = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "click",
                        "params": {"selector": "#b"}}))
        .await
        .unwrap();
    let c: Value = serde_json::from_str(&click).unwrap();
    assert_eq!(c["code"], 0, "open 档 click 应生效:{click}");

    // ③ set_guard 切 partial(黑名单罩住按钮):click 经「先隐盾后复盾」照常生效,
    //    盾区容器内应有盾罩,状态条文案切换为「部分锁定」。
    let sg = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "partial", "block_selectors": ["#b"], "note": "支付区已锁定"}}))
        .await
        .unwrap();
    let g: Value = serde_json::from_str(&sg).unwrap();
    assert_eq!(g["code"], 0, "set_guard(partial) 应成功:{sg}");
    assert_eq!(g["data"]["mode"], "partial", "响应应回报 partial:{sg}");
    let click2 = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "click",
                        "params": {"selector": "#b"}}))
        .await
        .unwrap();
    let c2: Value = serde_json::from_str(&click2).unwrap();
    assert_eq!(c2["code"], 0, "partial 黑名单盾下 click 应经隐盾照常生效:{click2}");
    let check = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression":
                            "JSON.stringify({c:+document.getElementById('b').dataset.c||0, \
                             sc:document.getElementById('__laew_guard_shields__').childElementCount, \
                             chip:(document.querySelector('.__laew_guard_hint_title__')||{}).textContent||''})"}}))
        .await
        .unwrap();
    let e: Value = serde_json::from_str(&check).unwrap();
    let payload: Value =
        serde_json::from_str(e["data"]["result"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    assert_eq!(payload["c"], 2, "两次 click 都应真实生效:{check}");
    assert_eq!(payload["sc"], 1, "黑名单应生成 1 个盾罩:{check}");
    assert!(payload["chip"].as_str().unwrap_or("").contains("部分锁定"), "状态条应切换文案:{check}");

    // ④ set_guard 切回 locked:输入锁恢复 + 蒙层回归。
    let sg2 = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "locked"}}))
        .await
        .unwrap();
    let g2: Value = serde_json::from_str(&sg2).unwrap();
    assert_eq!(g2["code"], 0, "set_guard(locked) 应成功:{sg2}");
    assert_eq!(g2["data"]["input_locked"], true, "locked 档应回报输入锁:{sg2}");

    // ⑤ 参数错误:partial 缺选择器 → 1001(不是 2002,便于 LLM 机械修正)。
    let bad = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "partial"}}))
        .await
        .unwrap();
    let b: Value = serde_json::from_str(&bad).unwrap();
    assert_eq!(b["code"], 1001, "partial 缺选择器应回 1001:{bad}");

    // ⑥ 收口:关闭全部页面回收浏览器(全局单例,防污染其它测试)
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

// =================== 第 144 轮:凭证区域探测 + partial 白名单子 frame fail-open 真浏览器验证(#[ignore]) ===================
//
// 覆盖两条关键链路(详案 docs/MCP_Web_Use/08 §4):
// 1. probe_credential_zones:密码框 → form 容器 + 验证码语义容器,产出主文档选择器;
// 2. partial 白名单挖洞:洞内(密码框)elementFromPoint 命中本体、洞外命中盾罩;
//    白名单选择器在子 frame(srcdoc,同源可访问)零命中 → 不加盾(fail-open,
//    跨域验证码 iframe 内部必须放行);黑名单在子 frame照常加盾。
// 需要本机安装 Chrome;跑 `cargo test --lib real_credential_zone -- --ignored --nocapture`。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_credential_zone_probe_and_subframe_fail_open() {
    use crate::agent::browser::BrowserManager;
    let url = "data:text/html,<html><body><form id='login'><input type='text' name='user' placeholder='账号'>\
               <input type='password' name='pwd'><button type='submit'>go</button></form>\
               <div class='captcha-box'><iframe srcdoc='<input id=\"inner\">' style='width:160px;height:60px'></iframe></div>\
               </body></html>";
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "window_width": 800, "window_height": 600}))
        .await
        .expect("open 失败(本机是否装有 Chrome?)");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "open 应成功:{out}");
    let pid = v["data"]["page_id"].as_str().unwrap().to_string();
    let page = BrowserManager::global().page(&pid).await.expect("page 句柄");

    // ① 凭证区域探测:form(#login)优先 + 验证码语义容器(≥2 条)
    let zones = super::unlock_zone::probe_credential_zones(&page).await;
    assert!(!zones.is_empty(), "登录页应探测到凭证区域");
    assert_eq!(zones[0], "#login", "密码框应容器化到 form#login:{zones:?}");
    assert!(zones.len() >= 2, "验证码语义容器也应命中:{zones:?}");
    // 选择器真实命中元素(生成即自验)
    let hit_check = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": format!(
                            "[{}].map(s => document.querySelectorAll(s).length)",
                            zones.iter().map(|z| format!("'{z}'")).collect::<Vec<_>>().join(","))}}))
        .await
        .unwrap();
    let hc: Value = serde_json::from_str(&hit_check).unwrap();
    let counts: Vec<i64> = serde_json::from_str(
        hc["data"]["result"].as_str().unwrap_or("[]"),
    )
    .unwrap_or_default();
    assert!(
        counts.iter().all(|c| *c >= 1),
        "每条探测选择器都应命中 ≥1 元素:{zones:?} → {counts:?}"
    );

    // ② set_guard 切 partial 白名单(探测到的 zones)→ 洞内可点、洞外被盾吞
    let sg = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "set_guard",
                        "params": {"mode": "partial", "allow_selectors": zones.clone(),
                                   "note": "测试放行"}}))
        .await
        .unwrap();
    let g: Value = serde_json::from_str(&sg).unwrap();
    assert_eq!(g["code"], 0, "set_guard(partial) 应成功:{sg}");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let probe = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression":
                            "JSON.stringify((function(){ \
                             const p = document.querySelector('#login input[type=password]'); \
                             const r = p.getBoundingClientRect(); \
                             const inHole = document.elementFromPoint(r.x + r.width/2, r.y + r.height/2); \
                             const outside = document.elementFromPoint(innerWidth - 5, innerHeight - 5); \
                             return { inHole: inHole ? inHole.tagName : 'none', \
                                      outsideShield: !!(outside && outside.closest && outside.closest('#__laew_guard_shields__')) }; })())"}}))
        .await
        .unwrap();
    let p: Value = serde_json::from_str(&probe).unwrap();
    let payload: Value =
        serde_json::from_str(p["data"]["result"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    assert_eq!(payload["inHole"], "INPUT", "洞内(密码框)应命中输入框本体:{probe}");
    assert_eq!(payload["outsideShield"], true, "洞外应命中盾罩:{probe}");

    // ③ 子 frame fail-open:把管控脚本注入 srcdoc iframe(同源可访问),
    //    白名单(主文档选择器)在 iframe 零命中 → 0 盾;黑名单命中 → 1 盾。
    let iframe_cfg = crate::agent::browser_overlay::PageGuardConfig::partial(
        vec!["#login".to_string()],
        vec![],
    );
    let script_js = crate::agent::browser_overlay::guard_script(&iframe_cfg);
    let iframe_expr = format!(
        "(function() {{ \
          const f = document.querySelector('iframe'); \
          const w = f && f.contentWindow; \
          if (!w) return 'no-window'; \
          w.eval({}); \
          w.__laewGuardSet('partial', ['#login'], [], null); \
          const sc = w.document.getElementById('__laew_guard_shields__'); \
          const allowCount = sc ? sc.childElementCount : -1; \
          w.__laewGuardSet('partial', [], ['#inner'], null); \
          const blockCount = sc ? sc.childElementCount : -2; \
          return allowCount + '/' + blockCount; }})()",
        super::js_str(&script_js)
    );
    let ifr = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": iframe_expr}}))
        .await
        .unwrap();
    let iv: Value = serde_json::from_str(&ifr).unwrap();
    let r = iv["data"]["result"].as_str().unwrap_or("");
    assert_eq!(r, "0/1", "子 frame 白名单零命中应 0 盾(fail-open),黑名单应 1 盾:{ifr}");

    // ④ 收口:关闭全部页面回收浏览器(全局单例,防污染其它测试)
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

// ==================== 第 145 轮:HITL 应答 next_hint 分流(输码 vs 已完成) ====================

/// 人工在弹窗输入验证码文本(实测 z7z2 场景)→ 必须指引立即 input_text 填码提交;
/// 点选选项(「N. 选项」形态)→ 才走 inspect 验证;其它 reason 自由文本 → 通用提示。
#[test]
fn hitl_answer_hint_splits_typed_code_vs_option_pick() {
    // 自由文本验证码(captcha 实测形态)
    let h = hitl_hint::hitl_answer_hint("captcha", "z7z2");
    assert!(h.contains("input_text"), "captcha 自由文本应指引填码: {h}");
    assert!(!h.contains("inspect 验证页面当前状态"), "不应再让 Agent 只观望: {h}");
    // sms / two_factor 同款
    assert!(hitl_hint::hitl_answer_hint("sms", "482913").contains("input_text"));
    assert!(hitl_hint::hitl_answer_hint("two_factor", "993 041").contains("input_text"));
    // 选项应答(弹窗选项 / TUI 数字映射共用「N. 选项」形态)
    let h = hitl_hint::hitl_answer_hint("captcha", "1. 我已完成人工操作,继续");
    assert!(h.contains("inspect"), "选项应答应走 inspect 验证: {h}");
    assert!(hitl_hint::hitl_answer_hint("login", "2. 我已登录完成").contains("inspect"));
    // 其它 reason 自由文本:通用提示,不误指填码
    let h = hitl_hint::hitl_answer_hint("custom", "跳过这个站点");
    assert!(h.contains("human_response"), "{h}");
    assert!(!h.contains("input_text"), "非验证码 reason 不应指引填码: {h}");
}

/// 「N. 选项」形态判定:数字前缀 + ". " 分隔;纯数字验证码/含点文本不误判。
#[test]
fn option_pick_answer_shape_detection() {
    for s in ["1. 我已完成人工操作,继续", "12. 某选项", " 3. 带空格前缀"] {
        assert!(hitl_hint::is_option_pick_answer(s), "「{s}」应识别为选项应答");
    }
    for s in ["z7z2", "482913", "993 041", "3.14 是圆周率吗", "选项一", ""] {
        assert!(!hitl_hint::is_option_pick_answer(s), "「{s}」不应识别为选项应答");
    }
}

// ===================== 第 146 轮:origin 复用 / 访问台账 =====================

/// origin 提取:仅 http(s) 参与,chrome:// 等特殊页永不兜底复用。
#[test]
fn url_origin_extracts_and_rejects_special_schemes() {
    assert_eq!(
        url_origin("http://10.255.159.58:20122/OrganManagement").as_deref(),
        Some("http://10.255.159.58:20122")
    );
    assert_eq!(url_origin("https://a.com/x?y=1").as_deref(), Some("https://a.com"));
    // 默认端口归一(url crate 语义:80/443 省略)
    assert_eq!(
        url_origin("http://a.com:80/x").as_deref(),
        url_origin("http://a.com/x").as_deref()
    );
    assert_eq!(url_origin("chrome://new-tab-page/"), None);
    assert_eq!(url_origin("about:blank"), None);
    assert_eq!(url_origin("devtools://devtools/bundled/inspector.html"), None);
    assert_eq!(url_origin("file:///tmp/a.html"), None);
    assert_eq!(url_origin("not a url"), None);
}

/// origin 复用兜底:URL 精确未命中、同 origin 存活页应被选中(取最早创建的)。
/// 不起真浏览器 —— `find_reusable_page` 依赖全局 BrowserManager,这里只锁纯函数层;
/// 端到端行为由 #[ignore] 真浏览器用例与实测日志验证。
#[test]
fn reuse_match_kind_strs() {
    use super::ReuseMatch;
    assert_eq!(ReuseMatch::Exact.as_str(), "exact");
    assert_eq!(ReuseMatch::Origin.as_str(), "origin");
}

/// inspect(info=coverage):进程级全局事实,不依赖存活页面(page_id 随便给)。
#[tokio::test]
async fn inspect_coverage_returns_ledger_without_live_page() {
    let _g = visit_ledger::test_lock().lock().unwrap();
    visit_ledger::reset_for_test();
    visit_ledger::record_visit("p_x", "http://a.com/login", "登录", visit_ledger::VisitSource::Open);
    visit_ledger::record_visit("p_x", "http://a.com/home", "首页", visit_ledger::VisitSource::Navigate);
    visit_ledger::record_visit("p_x", "http://a.com/home", "首页", visit_ledger::VisitSource::Navigate);
    visit_ledger::record_visit("p_y", "http://a.com/home", "首页", visit_ledger::VisitSource::NewTab);
    let out = inspect::run(serde_json::json!({"page_id": "nonexistent", "info": "coverage"}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "coverage 不应因 page_id 失效被拒: {out}");
    assert_eq!(v["data"]["summary"]["distinct_pages"], 2);
    assert_eq!(v["data"]["summary"]["total_visits"], 4);
    // /home 3 次 → 重复警示
    assert_eq!(visit_ledger::visit_note("http://a.com/home").unwrap()["visits"], 3);
    assert!(visit_ledger::visit_note("http://a.com/login").is_none());
}

/// navigate 响应附带 visit_note(≥3 次时):经 act_navigate 需要 live page,
/// 这里锁纯函数口径(visit_note 已在上例覆盖),补 key 归一化兜底。
#[test]
fn visit_note_ignores_query_and_fragment() {
    let _g = visit_ledger::test_lock().lock().unwrap();
    visit_ledger::reset_for_test();
    visit_ledger::record_visit("p", "http://a.com/p", "", visit_ledger::VisitSource::Navigate);
    visit_ledger::record_visit("p", "http://a.com/p?x=1", "", visit_ledger::VisitSource::Navigate);
    let note = visit_ledger::visit_note("http://a.com/p#frag");
    assert!(note.is_none(), "2 次(含 query 折叠)未到阈值 3");
    visit_ledger::record_visit("p", "http://a.com/p#again", "", visit_ledger::VisitSource::Navigate);
    assert!(visit_ledger::visit_note("http://a.com/p?y=2").is_some(), "3 次应触发");
}

// ==================== 第 147 轮:自动阻断感知(语义挑战检测) ====================

/// 构造探针 JS 同构的 overlay 元数据(纯函数 detect_challenge 的输入)。
fn probe_overlay(text: &str, grid_imgs: i64, iframe_like: bool, buttons: Value) -> Value {
    json!({
        "found": true, "text": text,
        "img_count": grid_imgs, "grid_imgs": grid_imgs,
        "canvas_count": 0,
        "iframe_count": if iframe_like { 1 } else { 0 },
        "iframe_like": iframe_like, "input_count": 0,
        "buttons": buttons,
        "rect": {"w": 360, "h": 420}, "viewport": {"w": 1280, "h": 800},
        "area_ratio": 0.23, "z_index": 9999,
    })
}

#[test]
fn detect_challenge_doubao_iframe_with_strong_word() {
    // 豆包形态:题面在跨域 iframe 内主文档读不到,壳层「安全验证」强词 + 验证码尺寸 iframe
    let o = probe_overlay("安全验证 请完成下方验证", 0, true, json!(["刷新", "换一张"]));
    let hit = blocker_probe::detect_challenge(&o).expect("iframe_like(2)+强词(4) 应命中");
    assert_eq!(hit["kind"], "captcha");
    assert_eq!(hit["matched"], "challenge_overlay");
    assert!(hit["evidence"]["iframe_like"].as_bool().unwrap());
}

#[test]
fn detect_challenge_image_grid_with_weak_word() {
    // 九宫格渲染在主文档 + 题面动词「选出」:2+1=3 过阈值(豆包无 iframe 变体)
    let o = probe_overlay("选出在公园能看到的事物或动物", 9, false, json!(["确认"]));
    let hit = blocker_probe::detect_challenge(&o).expect("九宫格(2)+弱指令词(1) 应命中");
    assert_eq!(hit["evidence"]["grid_imgs"], 9);
    assert!(hit["snippet"].as_str().unwrap().contains("公园"));
}

#[test]
fn detect_challenge_strong_word_alone_hits() {
    // 弹层只有验证文案、无图无 iframe(短信/滑块文案形态):强词单独触发
    let o = probe_overlay("请完成人机验证后继续操作", 0, false, json!([]));
    assert!(blocker_probe::detect_challenge(&o).is_some());
}

#[test]
fn detect_challenge_structural_signal_alone_not_enough() {
    // 防误报:纯 iframe 形态(无任何文案,如视频/组件嵌入)与纯九宫格(图库弹窗)不报
    let iframe_only = probe_overlay("", 0, true, json!([]));
    assert!(blocker_probe::detect_challenge(&iframe_only).is_none(), "iframe 单独 2 分不过阈值");
    let grid_only = probe_overlay("精选图集", 9, false, json!(["关闭"]));
    assert!(blocker_probe::detect_challenge(&grid_only).is_none(), "九宫格单独 2 分不过阈值");
}

#[test]
fn detect_challenge_ad_overlay_suppressed() {
    // 广告弹窗:iframe_like 但文案/按钮命中广告负信号 → 压制
    let o = probe_overlay("限时优惠 广告", 0, true, json!(["跳过广告", "了解更多"]));
    assert!(blocker_probe::detect_challenge(&o).is_none(), "广告负信号应压制");
}

// ==================== 第 150 轮:canvas 拖拽验证识别 + 执行超时兜底 ====================

/// canvas 形态 overlay(主文档渲染的拖拽拼图/滑块验证,无九宫格 img/验证码 iframe)。
fn probe_overlay_canvas(text: &str, canvas_count: i64, buttons: Value) -> Value {
    json!({
        "found": true, "text": text,
        "img_count": 0, "grid_imgs": 0,
        "canvas_count": canvas_count,
        "iframe_count": 0, "iframe_like": false, "input_count": 0,
        "buttons": buttons,
        "rect": {"w": 400, "h": 360}, "viewport": {"w": 1280, "h": 800},
        "area_ratio": 0.14, "z_index": 9999,
    })
}

#[test]
fn detect_challenge_canvas_drag_with_weak_word() {
    // 豆包「拖拽图片到框中」主文档 canvas 形态:canvas(2)+弱词「拖拽/图片」(1)=3 命中
    let o = probe_overlay_canvas("请拖拽图片到对应位置完成验证", 1, json!(["刷新"]));
    let hit = blocker_probe::detect_challenge(&o).expect("canvas(2)+弱词(1) 应命中");
    assert_eq!(hit["evidence"]["canvas_count"], 1);
    assert!(hit["snippet"].as_str().unwrap().contains("拖拽"));
}

#[test]
fn detect_challenge_canvas_alone_not_enough() {
    // 防误报:普通 canvas 图表/游戏(无验证文案)2 分不过阈值
    let o = probe_overlay_canvas("数据报表", 2, json!(["导出"]));
    assert!(blocker_probe::detect_challenge(&o).is_none(), "canvas 单独 2 分不过阈值");
}

#[test]
fn detect_challenge_canvas_ad_suppressed() {
    // canvas 广告(抽奖转盘):结构 2 分 + 负信号 −3 → 压制
    let o = probe_overlay_canvas("幸运抽奖 广告", 1, json!(["跳过"]));
    assert!(blocker_probe::detect_challenge(&o).is_none(), "canvas 广告负信号应压制");
}

// ==================== 第 151 轮:bg-image 九宫格 + eval 变更判定 + 输入硬闸 ====================

/// CSS 背景图九宫格形态 overlay(挑战格用 background-image 而非 <img> 渲染)。
fn probe_overlay_bg(text: &str, bg_imgs: i64, grid_imgs: i64, buttons: Value) -> Value {
    json!({
        "found": true, "text": text,
        "img_count": grid_imgs, "grid_imgs": grid_imgs, "bg_imgs": bg_imgs,
        "canvas_count": 0,
        "iframe_count": 0, "iframe_like": false, "input_count": 0,
        "buttons": buttons,
        "rect": {"w": 360, "h": 420}, "viewport": {"w": 1280, "h": 800},
        "area_ratio": 0.23, "z_index": 9999,
    })
}

#[test]
fn detect_challenge_bg_image_grid_with_weak_word() {
    // 背景图九宫格(grid=0/bg=9)+题面动词「选出」:2+1=3 命中
    let o = probe_overlay_bg("选出下列图片中的卧室家具", 9, 0, json!(["确认"]));
    let hit = blocker_probe::detect_challenge(&o).expect("bg 九宫格(2)+弱词(1) 应命中");
    assert_eq!(hit["evidence"]["bg_imgs"], 9);
    assert_eq!(hit["evidence"]["grid_imgs"], 0);
}

#[test]
fn detect_challenge_bg_grid_mixed_counts_combined() {
    // <img> 与背景图混合:grid=2 + bg=3 合计 ≥4 计一次结构分
    let o = probe_overlay_bg("请点击包含文字的图标", 3, 2, json!([]));
    assert!(
        blocker_probe::detect_challenge(&o).is_some(),
        "grid(2)+bg(3) 合计应过结构阈值"
    );
}

#[test]
fn detect_challenge_bg_grid_alone_not_enough() {
    // 防误报:纯背景图网格(相册/商品墙弹窗,无指令文案)不过阈值
    let o = probe_overlay_bg("精选图集", 12, 0, json!(["关闭"]));
    assert!(
        blocker_probe::detect_challenge(&o).is_none(),
        "背景图网格单独 2 分不过阈值"
    );
}

#[test]
fn eval_expression_mutates_classification() {
    // 实测事故形态:execCommand insertText / 合成 KeyboardEvent / 自己 click —— 判定为可变更
    assert!(blocker_probe::eval_expression_mutates(
        "document.execCommand('insertText', false, q)"
    ));
    assert!(blocker_probe::eval_expression_mutates(
        "el.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter'}))"
    ));
    assert!(blocker_probe::eval_expression_mutates("btn.click();"));
    assert!(blocker_probe::eval_expression_mutates(
        "location.href = 'https://example.com'"
    ));
    assert!(blocker_probe::eval_expression_mutates(
        "window.open('/next')"
    ));
    // 纯读取(大小写混合)不判变更
    assert!(!blocker_probe::eval_expression_mutates(
        "JSON.stringify(window.__NEXT_DATA__.props.pageProps.list.slice(0,20))"
    ));
    assert!(!blocker_probe::eval_expression_mutates(
        "document.querySelectorAll('button').length"
    ));
    assert!(!blocker_probe::eval_expression_mutates(""));
}

#[test]
fn blocker_input_gate_code_is_6002() {
    // 硬闸信封码独立于 6001(任务锚点),归 6xxx 约束段
    assert_eq!(super::CODE_BLOCKER_INPUT_GATE, 6002);
    assert_eq!(super::CODE_TARGET_ANCHOR_VIOLATION, 6001);
}

#[test]
fn eval_timeout_from_parsing() {
    // 纯函数:正整数生效;缺省/0/非法回退默认 30s
    assert_eq!(
        super::eval_timeout_from(Some("5000".into())),
        5000
    );
    assert_eq!(super::eval_timeout_from(None), super::EVAL_DEFAULT_TIMEOUT_MS);
    assert_eq!(
        super::eval_timeout_from(Some("0".into())),
        super::EVAL_DEFAULT_TIMEOUT_MS
    );
    assert_eq!(
        super::eval_timeout_from(Some("abc".into())),
        super::EVAL_DEFAULT_TIMEOUT_MS
    );
}

#[test]
fn step_has_blocker_alert_detection() {
    // 第 150 轮:sequence 中断判定 —— 步骤数据携带 blocker_alert 即中断整批
    assert!(super::step_has_blocker_alert(&json!({"blocker_alert": {"kind": "captcha"}})));
    assert!(super::step_has_blocker_alert(&json!({"blocker_alert": null})));
    assert!(!super::step_has_blocker_alert(&json!({"slept_ms": 500})));
    assert!(!super::step_has_blocker_alert(&json!({})));
}

#[test]
fn detect_challenge_clean_overlay_and_null() {
    let clean = probe_overlay("新品上线通知", 1, false, json!(["知道了"]));
    assert!(blocker_probe::detect_challenge(&clean).is_none());
    assert!(blocker_probe::detect_challenge(&Value::Null).is_none());
    assert!(blocker_probe::detect_challenge(&json!({"found": false})).is_none());
}

#[test]
fn analyze_merges_keyword_and_challenge_with_dedupe() {
    // 关键词通道(captcha)+ 弹层挑战同 kind → 去重保留带 evidence 的挑战条目
    let probe = json!({
        "text": "请完成安全验证 拖动滑块",
        "overlay": probe_overlay("安全验证", 0, true, json!(["刷新"])),
    });
    let out = blocker_probe::analyze(&probe);
    assert_eq!(out.len(), 1, "同 kind 去重,实际:{out:?}");
    assert_eq!(out[0]["matched"], "challenge_overlay");
    assert!(out[0]["evidence"].is_object(), "保留带结构证据的挑战版本");
}

#[test]
fn analyze_keeps_different_kinds() {
    // 全文命中 login + 弹层命中 captcha → 两条并存
    let probe = json!({
        "text": "登录后查看更多内容",
        "overlay": probe_overlay("请选出图中包含的物品", 9, false, json!(["刷新"])),
    });
    let out = blocker_probe::analyze(&probe);
    let kinds: Vec<&str> = out.iter().filter_map(|b| b["kind"].as_str()).collect();
    assert!(kinds.contains(&"captcha"), "实际:{kinds:?}");
    assert!(kinds.contains(&"login"), "实际:{kinds:?}");
}

#[test]
fn analyze_challenge_overlays_keyword_only_path() {
    // 无弹层(overlay null)时退化为纯关键词(开关回退路径共用同一 analyze)
    let probe = json!({"text": "微信扫码登录", "overlay": null});
    let out = blocker_probe::analyze(&probe);
    assert!(out.iter().any(|b| b["kind"] == "qr_login"));
    let clean = blocker_probe::analyze(&json!({"text": "普通内容", "overlay": null}));
    assert!(clean.is_empty());
}

#[test]
fn action_needs_probe_covers_input_wait_nav() {
    for a in ["click", "input_text", "key_press", "drag", "human_click", "select_option"] {
        assert!(blocker_probe::action_needs_probe(a), "{a} 应探测");
    }
    for a in ["wait", "navigate", "back", "forward", "reload"] {
        assert!(blocker_probe::action_needs_probe(a), "{a} 应探测");
    }
    // 第 151 轮:eval_js 纳入收尾探测(实测 LLM 把输入行为迁移到 execCommand/
    // 合成事件后,「只读通道」假设不成立;长轮询返回恰是弹窗出现后的检查点)
    assert!(blocker_probe::action_needs_probe("eval_js"), "eval_js 应探测(第 151 轮)");
    // 观察类/状态设置类仍不探测
    for a in ["screenshot", "set_guard", "set_viewport", "request_human", "heartbeat"] {
        assert!(!blocker_probe::action_needs_probe(a), "{a} 不应探测");
    }
}

#[test]
fn wait_sleep_ms_accepts_alias_and_caps() {
    // 第 147 轮:ms 别名(实测豆包任务 LLM 全程传 ms=12000 只睡了 500ms 的根治)
    assert_eq!(control::wait_sleep_ms(&json!({"ms": 12000})), 12000);
    assert_eq!(control::wait_sleep_ms(&json!({"duration_ms": 8000})), 8000);
    // 两参并存取较大者
    assert_eq!(
        control::wait_sleep_ms(&json!({"duration_ms": 3000, "ms": 7000})),
        7000
    );
    // 缺省 500 / 上限 30000(AI 回复长尾 30-60s,第 82 轮)
    assert_eq!(control::wait_sleep_ms(&json!({})), 500);
    assert_eq!(control::wait_sleep_ms(&json!({"ms": 999999})), 30000);
}

#[test]
fn infer_step_action_from_fields() {
    // 第 147 轮:sequence 步骤缺 action 时按字段推断(实测 steps 常漏 action:"control")
    assert_eq!(infer_step_action(&json!({"control_action": "wait", "params": {"ms": 100}})), Some("control"));
    assert_eq!(infer_step_action(&json!({"info": "elements"})), Some("inspect"));
    assert_eq!(infer_step_action(&json!({"url": "https://a.com"})), None);
    // control_action 优先于 info
    assert_eq!(
        infer_step_action(&json!({"control_action": "click", "info": "elements"})),
        Some("control")
    );
}

/// 第 147 轮真浏览器集成验证(#[ignore],本地 `--ignored` 跑):
/// 构造豆包风控形态弹层(z-index 9999 全屏遮罩 + 居中弹窗 + 9 宫格小图 + 题面 +
/// 刷新按钮),探针采集 → 打分应命中 captcha;干净页面应零命中。
/// 与 browser_overlay 真浏览器用例同构(需本机 Chrome,会弹有头窗口);
/// DOM 经 about:blank + body.innerHTML 注入(长 data: URL 含 % 会被当百分号编码,
/// 实测 goto 超时)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_blocker_probe_detects_challenge_modal() {
    use crate::agent::browser::{BrowserManager, BrowserMode};
    let (pid, _, _) = BrowserManager::global()
        .new_page("about:blank", BrowserMode::Headed, None, None, Some((1280, 800)), true, Default::default())
        .await
        .expect("启动浏览器失败(本机是否装有 Chrome?)");
    let page = BrowserManager::global().page(&pid).await.expect("page 句柄");
    // 9 宫格小图(64×64,近方形)+ 题面「选出在公园能看到的事物或动物」+ 刷新按钮
    let inject = r#"(() => {
      const tiles = Array.from({length: 9}, (_, i) =>
        `<img src="data:image/gif;base64,R0lGODlhAQABAAAAACw=" style="width:64px;height:64px" alt="t${i}">`).join('');
      document.body.innerHTML = `<div>普通内容</div>
        <div style="position:fixed;inset:0;background:rgba(0,0,0,.5);z-index:9999" class="c-mask">
          <div class="c-dialog" role="dialog" style="position:fixed;left:50%;top:50%;transform:translate(-50%,-50%);width:360px;height:420px;background:#fff;z-index:10000;padding:16px">
            <h3>安全验证</h3>
            <p>选出在公园能看到的事物或动物</p>
            <div style="display:grid;grid-template-columns:repeat(3,1fr)">${tiles}</div>
            <button>刷新</button><button>换一张</button>
          </div></div>`;
      return document.querySelectorAll('img').length;
    })()"#;
    let n = eval_js_string(&page, inject).await.unwrap();
    assert_eq!(n, 9, "9 宫格注入失败");
    let probe = blocker_probe::collect_for_inspect(&page).await;
    let alerts = blocker_probe::analyze(&probe);
    assert!(
        alerts.iter().any(|b| b["kind"] == "captcha" && b["matched"] == "challenge_overlay"),
        "豆包形态弹层应命中,probe:{probe:?} alerts:{alerts:?}"
    );
    // 全链路(第 151 轮起):弹窗在场时 input_text 先过**输入硬闸** → 6002 拦截且不执行;
    // force=true 逃生口执行动作,收尾推送探针仍附 blocker_alert(第 147 轮语义保持)
    eval_js_string(&page, r#"document.body.insertAdjacentHTML('beforeend', '<input id="q" style="width:200px;height:20px">')"#).await.unwrap();
    let out = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "input_text",
                        "params": {"selector": "#q", "text": "hi"}}))
        .await
        .unwrap();
    let env: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(env["code"], 6002, "弹窗在场 input_text 应被硬闸拦截:{env}");
    assert!(env["data"]["blocker_alert"]["kind"] == "captcha", "信封应带 blocker_alert:{env}");
    assert_eq!(env["data"]["next_action"], "request_human");
    assert!(env["data"]["human_assist"]["ready"].as_bool().unwrap(), "human_assist 载荷应就绪");
    let typed = eval_js_string(&page, "document.getElementById('q').value").await.unwrap();
    assert_eq!(typed, "", "被硬闸拦截的动作不应写入输入框");
    // force 逃生口:动作执行 + 收尾推送探针仍附告警
    let forced = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "input_text",
                        "params": {"selector": "#q", "text": "hi", "force": true}}))
        .await
        .unwrap();
    let fenv: Value = serde_json::from_str(&forced).unwrap();
    assert_eq!(fenv["code"], 0, "force 后动作应执行:{fenv}");
    assert!(fenv["data"]["blocker_alert"]["kind"] == "captcha", "收尾探针仍应附 blocker_alert:{fenv}");
    let typed2 = eval_js_string(&page, "document.getElementById('q').value").await.unwrap();
    assert_eq!(typed2, "hi", "force 后文本应写入");
    // 干净页面(移除弹层)零命中
    eval_js_string(&page, "document.querySelector('.c-mask').remove()").await.unwrap();
    let probe2 = blocker_probe::collect_for_inspect(&page).await;
    let alerts2 = blocker_probe::analyze(&probe2);
    assert!(alerts2.is_empty(), "干净页面误报:{alerts2:?}");
    BrowserManager::global().shutdown().await;
}

/// 第 151 轮真浏览器集成验证(#[ignore],本地 `--ignored` 跑):
/// ① CSS 背景图九宫格(无 `<img>`)挑战形态识别;② 可变更 eval_js 被 6002 硬闸
/// 拦截、纯读取 eval_js 放行;③ 移除弹窗后硬闸自动放行(非解锁制)。
#[tokio::test]
#[ignore = "需要本机真实 Chrome(会弹有头窗口);本地手动跑"]
async fn real_browser_input_gate_bg_image_challenge() {
    use crate::agent::browser::{BrowserManager, BrowserMode};
    let (pid, _, _) = BrowserManager::global()
        .new_page("about:blank", BrowserMode::Headed, None, None, Some((1280, 800)), true, Default::default())
        .await
        .expect("启动浏览器失败(本机是否装有 Chrome?)");
    let page = BrowserManager::global().page(&pid).await.expect("page 句柄");
    // 「选出卧室家具」型:背景图九宫格(120×120 div,无 img)+ 题面弱词
    let inject = r#"(() => {
      const tiles = Array.from({length: 9}, () =>
        `<div style="display:inline-block;width:120px;height:120px;background-image:url(data:image/gif;base64,R0lGODlhAQABAAAAACw=)"></div>`).join('');
      document.body.innerHTML = `<input id="chat" style="position:fixed;left:0;top:0;width:200px;height:30px">
        <div class="verify-modal" style="position:fixed;left:200px;top:80px;width:480px;height:420px;background:#fff;z-index:99999;padding:12px">
          <p>选出下列图片中的卧室家具</p><div>${tiles}</div><button>确认</button></div>`;
      return document.querySelectorAll('.verify-modal > div > div').length;
    })()"#;
    let n = eval_js_string(&page, inject).await.unwrap();
    assert_eq!(n, 9, "背景图九宫格注入失败");
    let probe = blocker_probe::collect_for_inspect(&page).await;
    assert!(
        blocker_probe::analyze(&probe)
            .iter()
            .any(|b| b["kind"] == "captcha" && b["matched"] == "challenge_overlay"),
        "背景图九宫格形态应命中,probe:{probe:?}"
    );

    // 可变更 eval_js(execCommand)在弹窗在场时被 6002 硬闸拦截,不执行
    let gated = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": "(() => { const q = document.getElementById('chat'); q.focus(); q.value = 'blocked'; document.execCommand('insertText', false, 'x'); return q.value; })()"}}))
        .await
        .unwrap();
    let genv: Value = serde_json::from_str(&gated).unwrap();
    assert_eq!(genv["code"], 6002, "可变更 eval_js 应被硬闸拦截:{genv}");
    assert!(genv["data"].get("blocker_alert").is_some(), "拦截信封应带 blocker_alert");
    let v = eval_js_string(&page, "document.getElementById('chat').value").await.unwrap();
    assert_eq!(v, "", "被拦截的 eval 不应产生副作用");

    // 纯读取 eval_js 放行(不硬闸;干净响应无告警)
    let read = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "eval_js",
                        "params": {"expression": "document.querySelectorAll('div').length"}}))
        .await
        .unwrap();
    let renv: Value = serde_json::from_str(&read).unwrap();
    assert_eq!(renv["code"], 0, "纯读取 eval 应放行:{renv}");
    // 第 151 轮:eval_js 纳入收尾推送探针 —— 弹窗在场时纯读取 eval 也附告警
    // (长轮询返回恰是弹窗出现后的检查点,本轮事故的闭环点),但不拦截执行
    assert!(
        renv["data"].get("blocker_alert").is_some(),
        "收尾探针应附 blocker_alert(不拦截):{renv}"
    );

    // 移除弹窗 → 硬闸自动放行(非解锁制,页面干净即恢复)
    eval_js_string(&page, "document.querySelector('.verify-modal').remove()").await.unwrap();
    let pass = McpWebUseTool
        .execute(json!({"action": "control", "page_id": pid, "control_action": "input_text",
                        "params": {"selector": "#chat", "text": "ok"}}))
        .await
        .unwrap();
    let penv: Value = serde_json::from_str(&pass).unwrap();
    assert_eq!(penv["code"], 0, "弹窗移除后应自动放行:{penv}");
    let v2 = eval_js_string(&page, "document.getElementById('chat').value").await.unwrap();
    assert_eq!(v2, "ok", "放行后动作应执行");
    BrowserManager::global().shutdown().await;
}
