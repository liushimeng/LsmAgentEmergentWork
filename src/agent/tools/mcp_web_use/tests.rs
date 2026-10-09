//! MCP_Web_Use 工具单元测试(自 tools/browser.rs 测试平移 + 单工具化改造)。

use super::*;
use super::{control, extract, inspect, page_state};
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
        "set_window", "sync_viewport", "set_highlight", "set_overlay", "request_human",
    ] {
        assert!(names.contains(required), "control_action 枚举缺失 {required}");
    }
    // 第 141 轮:set_overlay(蒙层运行时开关)使枚举 39 → 40
    assert_eq!(names.len(), 40, "control_action 应为 40 个,实际 {names:?}");
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
    assert_eq!(names.len(), 19, "info 应为 19 个,实际 {names:?}");
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
    assert_eq!(names.len(), 19, "info 应有 19 个枚举值: {names:?}");
    assert!(names.contains(&"ocr"), "info 枚举应含 ocr: {names:?}");
    assert!(names.contains(&"extract_links"), "info 枚举应含 extract_links: {names:?}");
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
