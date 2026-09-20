//! MCP_Web_Use 工具单元测试(自 tools/browser.rs 测试平移 + 单工具化改造)。

use super::*;

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
    ] {
        assert!(names.contains(required), "control_action 枚举缺失 {required}");
    }
    assert_eq!(names.len(), 35, "control_action 应为 35 个,实际 {names:?}");
}

#[test]
fn parameters_info_enum_complete() {
    let p = McpWebUseTool.parameters();
    let enums = p["properties"]["info"]["enum"].as_array().expect("info enum 应为数组");
    let names: Vec<&str> = enums.iter().filter_map(|v| v.as_str()).collect();
    for required in &[
        "console", "network", "elements", "dom", "localstorage", "sessionstorage",
        "cookies", "screenshot", "page_meta", "viewport", "url", "title", "image_urls",
    ] {
        assert!(names.contains(required), "info 枚举缺失 {required}");
    }
    assert_eq!(names.len(), 14, "info 应为 14 个,实际 {names:?}");
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
        assert_eq!(steps.len(), 4, "next_steps 应包含 4 步引导");
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
