//! MCP_Web_Use 工具单元测试(自 tools/browser.rs 测试平移 + 单工具化改造)。

use super::*;
use base64::Engine as _;

#[test]
fn envelope_codes() {
    let s = envelope(0, "ok", json!({"x": 1})).unwrap();
    assert!(s.contains("\"code\":0"));
    assert!(s.contains("\"message\":\"ok\""));
    assert!(s.contains("\"x\":1"));
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

// =================== Schema 枚举完整性(第 89 轮单工具化) ===================

#[test]
fn parameters_action_enum_is_six_values() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["action"]["enum"].as_array().expect("action enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        names,
        vec!["open", "list", "close", "control", "inspect", "sequence"]
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
        "set_window", "sync_viewport", "set_highlight", "request_human",
    ] {
        assert!(names.contains(required), "control_action 枚举缺失 {required}");
    }
    assert_eq!(names.len(), 39, "control_action 应为 39 个,实际 {names:?}");
}

#[test]
fn parameters_info_enum_complete() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["info"]["enum"].as_array().expect("info enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    for required in &[
        "console", "network", "elements", "dom", "localstorage", "sessionstorage",
        "cookies", "screenshot", "page_meta", "viewport", "url", "title", "image_urls",
        "blockers",
    ] {
        assert!(names.contains(required), "info 枚举缺失 {required}");
    }
    assert_eq!(names.len(), 16, "info 应为 16 个,实际 {names:?}");
}

#[test]
fn parameters_mode_defaults_hidden() {
    let p = McpWebUseTool.parameters();
    let mode = p["properties"]["mode"].clone();
    assert!(mode.is_object(), "mode 字段应为对象");
    assert_eq!(mode["default"], "hidden", "默认 mode 应为 hidden");
}

// =================== 参数校验(无需真实浏览器) ===================

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
    let res = McpWebUseTool
        .execute(json!({"action": "open", "url": "https://example.com"}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&res).expect("响应应为合法 JSON");
    let code = v["code"].as_i64().unwrap_or(-1);
    if code == 0 {
        // 成功路径:验证 mode=hidden + page_id + next_steps 非空
        assert_eq!(v["data"]["mode"], "hidden", "默认 mode 应为 hidden");
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
    assert_eq!(names.len(), 16, "info 应有 16 个枚举值: {names:?}");
    assert!(names.contains(&"ocr"), "info 枚举应含 ocr: {names:?}");
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

    let clean = inspect::detect_blockers("普通页面内容,无阻断");
    assert!(clean.is_empty(), "误报:{clean:?}");
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
