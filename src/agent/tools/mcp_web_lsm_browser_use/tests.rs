//! MCP_Web_LsmAgentBrowser_Use 兼容层单元测试。
//!
//! 只测纯函数与 `data:` 页面端到端,不依赖外网、不依赖外部浏览器。

use super::*;
use serde_json::json;

#[test]
fn 信封形状固定() {
    let s = envelope(0, "ok", json!({"a": 1})).unwrap();
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["code"], 0);
    assert_eq!(v["message"], "ok");
    assert_eq!(v["data"]["a"], 1);
}

#[test]
fn page_id_提取兼容信封与文本() {
    let env = json!({"code": 0, "data": {"page_id": "p_00ab12cd"}}).to_string();
    assert_eq!(extract_page_id_from_text(&env).as_deref(), Some("p_00ab12cd"));
    assert_eq!(
        extract_page_id_from_text("page_id=p_deadbeef 已打开").as_deref(),
        Some("p_deadbeef")
    );
    assert!(extract_page_id_from_text("没有页面").is_none());
}

#[test]
fn 占位符递归替换() {
    let step = json!({
        "action": "control",
        "page_id": "$page_id",
        "params": {"selector": "#x", "text": "到 ${spawned_page_id}"}
    });
    let out = substitute(&step, "p_1", "p_2");
    assert_eq!(out["page_id"], "p_1");
    assert_eq!(out["params"]["text"], "到 p_2");
}

#[tokio::test]
async fn data_页面_open_inspect_close_闭环() {
    let tool = McpWebLsmBrowserUseTool;
    let url = "data:text/html,<html><head><title>Demo</title></head><body><h1>Hello</h1><a id='a' href='data:text/html,<h2>Next</h2>'>go</a></body></html>";
    let out = tool
        .execute(json!({"action": "open", "url": url, "mode": "hidden"}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "{v}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();
    assert_eq!(v["data"]["title"], "Demo");

    let out = tool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "elements",
                        "params": {"selector": "h1"}}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0);
    assert!(v["data"]["text"].as_str().unwrap().contains("Hello"));

    let out = tool
        .execute(json!({"action": "control", "page_id": page_id,
                        "control_action": "click", "params": {"selector": "#a"}}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["data"]["final_url"], "data:text/html,<h2>Next</h2>");

    let out = tool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 0);
}

#[tokio::test]
async fn 缺少参数返回_1001() {
    let tool = McpWebLsmBrowserUseTool;
    let v: Value = serde_json::from_str(&tool.execute(json!({"action": "open"})).await.unwrap())
        .unwrap();
    assert_eq!(v["code"], 1001);
    let v: Value = serde_json::from_str(
        &tool
            .execute(json!({"action": "control", "page_id": "p_x"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["code"], 1001);
}

#[tokio::test]
async fn reuse_existing_返回_3002() {
    let tool = McpWebLsmBrowserUseTool;
    let v: Value = serde_json::from_str(
        &tool
            .execute(json!({"action": "open", "url": "data:text/html,x", "reuse_existing": true}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["code"], 3002);
}

/// 2026-10-26 实测回归:把 Chrome 的 `c_` page_id 传给本工具,必须给出
/// 「属于另一浏览器工具」的可操作指引(文档 41 §F4 声称已修,但兼容层此前
/// 直发通用文案,跨工具诊断是死代码)。
#[tokio::test]
async fn 跨工具_page_id_给出诊断指引() {
    let tool = McpWebLsmBrowserUseTool;
    let out = tool
        .execute(json!({"action": "control", "page_id": "c_00000000",
                        "control_action": "wait", "params": {"ms": 10}}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 2000, "{v}");
    let msg = v["message"].as_str().unwrap();
    assert!(msg.contains("MCP_Web_Chrome_Use"), "跨工具诊断未生效:{msg}");
    assert!(msg.contains("不跨工具复用"), "跨工具诊断未生效:{msg}");
    assert_eq!(v["data"]["cross_tool_page_id"], json!(true), "{v}");
}

/// `inspect` 路径同样必须给出跨工具诊断(此前与 control 各自直发通用文案)。
#[tokio::test]
async fn 跨工具_page_id_inspect_给出诊断指引() {
    let tool = McpWebLsmBrowserUseTool;
    let out = tool
        .execute(json!({"action": "inspect", "page_id": "c_00000000", "info": "title"}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 2000, "{v}");
    assert!(
        v["message"].as_str().unwrap().contains("MCP_Web_Chrome_Use"),
        "{v}"
    );
}

/// 非跨工具前缀的未知 page_id 仍走通用文案(不得误报跨工具)。
#[tokio::test]
async fn 非跨工具_page_id_走通用文案() {
    let tool = McpWebLsmBrowserUseTool;
    let out = tool
        .execute(json!({"action": "control", "page_id": "p_00ff00ff",
                        "control_action": "wait", "params": {"ms": 10}}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 2000, "{v}");
    assert!(!v["message"].as_str().unwrap().contains("Chrome"), "{v}");
}

/// 控制权动作必须可发现:schema 枚举含 `set_control_state`,参数含 state/note。
#[test]
fn schema_含控制权切换动作与参数() {
    let schema = McpWebLsmBrowserUseTool.parameters();
    let c_enums = schema["properties"]["control_action"]["enum"]
        .as_array()
        .unwrap();
    assert!(
        c_enums.iter().any(|v| v == "set_control_state"),
        "control_action 枚举缺 set_control_state:{c_enums:?}"
    );
    assert!(schema["properties"].get("state").is_some(), "缺 state(agent|user)");
    assert!(schema["properties"].get("note").is_some(), "缺 note(状态栏提示)");
    let d = McpWebLsmBrowserUseTool.description();
    assert!(d.contains("set_control_state"), "description 缺控制权切换说明");
    assert!(d.contains("4004"), "description 应点明用户控制状态下写操作被拒");
}

/// 标签页治理必须写进 description(模型据此少开页、复用 page_id)。
#[test]
fn description_点明标签页治理() {
    let d = McpWebLsmBrowserUseTool.description();
    assert!(d.contains("复用"), "description 缺页签复用提示");
    assert!(d.contains("标签页"), "description 缺标签页治理提示");
    // 转义残留自查:description 是给模型看的自然语言,不该出现任何反斜杠
    // (行续行写成 `\\` 会让模型看到 "文字\\" 这种噪声)。
    assert!(!d.contains('\\'), "description 不得含反斜杠残留:{d}");
}

/// 2026-11-20 闲鱼真机回归:顶层有 url 时,`params` 里的已知标量键
/// (window_width/window_height 等)必须补缺上提 —— 此前早退导致
/// Agent 惯用 `{"url":...,"params":{"window_width":...}}` 的窗口参数
/// 被静默丢弃(响应 requested 恒 null),被迫追加 set_viewport 补救。
#[test]
fn merge_open_params_顶层有url时params已知键仍上提() {
    let mut args = json!({
        "action": "open",
        "url": "https://www.goofish.com/",
        "params": {
            "window_width": 1080,
            "window_height": 1920,
            "wait_until": "domcontentloaded"
        }
    });
    merge_open_params(&mut args);
    assert_eq!(args["window_width"], 1080, "params.window_width 必须上提到顶层");
    assert_eq!(args["window_height"], 1920, "params.window_height 必须上提到顶层");
    assert_eq!(args["wait_until"], "domcontentloaded", "params.wait_until 必须上提到顶层");
}

/// 顶层显式值优先:params 只补缺,不得覆盖调用方顶层的明确取值。
#[test]
fn merge_open_params_顶层显式值不被params覆盖() {
    let mut args = json!({
        "action": "open",
        "url": "https://www.goofish.com/",
        "window_width": 1920,
        "params": { "window_width": 800, "unknown_junk": "x" }
    });
    merge_open_params(&mut args);
    assert_eq!(args["window_width"], 1920, "顶层显式值优先,params 只补缺");
    assert!(args.get("unknown_junk").is_none(), "未知键仍须忽略(params 不当杂物盒)");
}

/// 顶层缺 url 的旧路径(整体上提救 url)行为保持不变。
#[test]
fn merge_open_params_顶层缺url时仍整体上提() {
    let mut args = json!({
        "action": "open",
        "params": { "url": "https://a.test/" }
    });
    merge_open_params(&mut args);
    assert_eq!(args["url"], "https://a.test/", "缺 url 时 params.url 必须上提");
    assert!(args.get("params").is_some(), "params 本体保留不删(其余键仍可读)");
}
