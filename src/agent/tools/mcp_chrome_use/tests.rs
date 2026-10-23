//! MCP_Web_Chrome_Use 工具单元测试(不拉起真实 Chrome)。

use super::*;
use crate::agent::tools::Tool;

#[test]
fn 工具名与常量一致() {
    let tool = McpChromeUseTool;
    assert_eq!(tool.name(), "MCP_Web_Chrome_Use");
    assert_eq!(MCP_WEB_CHROME_USE_TOOL_NAME, "MCP_Web_Chrome_Use");
}

#[test]
fn schema_含五个基础_action() {
    let tool = McpChromeUseTool;
    let schema = tool.parameters();
    let enums = schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(enums, vec!["open", "list", "close", "control", "inspect"]);
    assert!(schema["required"].as_array().unwrap().contains(&json!("action")));
    // control_action 含 input_text 别名(对齐 Lsm 版命名)。
    let c_enums = schema["properties"]["control_action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(c_enums.contains(&"input_text"));
    assert!(c_enums.contains(&"type"));
    // 纯文字按钮 / 真实键盘事件(第 178 轮实测:百度地图「选为终点」无稳定选择器)。
    assert!(c_enums.contains(&"click_text"));
    assert!(c_enums.contains(&"key_press"));
    // params 含 save_path / timeout_ms / text / key / exact / max_chars。
    let props = &schema["properties"]["params"]["properties"];
    assert!(props.get("save_path").is_some());
    assert!(props.get("timeout_ms").is_some());
    assert!(props.get("text").is_some());
    assert!(props.get("key").is_some());
    assert!(props.get("exact").is_some());
    assert!(props.get("max_chars").is_some());
}

#[test]
fn cap_chars_按字符边界截断并保留统计() {
    let (s, chars, truncated) = cap_chars("中文abc".repeat(3000), 1_000);
    assert!(truncated);
    assert_eq!(chars, 15_000);
    assert!(s.chars().count() > 1_000, "截断提示本身也应保留:{s}");
    assert!(s.contains("原始 15000 字符"), "{s}");
    let (s2, chars2, t2) = cap_chars("短文本".to_string(), 20_000);
    assert!(!t2 && chars2 == 3 && s2 == "短文本");
}

#[test]
fn max_chars_arg_夹取范围与默认值() {
    assert_eq!(max_chars_arg(&json!({})), 20_000);
    assert_eq!(max_chars_arg(&json!({"max_chars": 10})), 1_000);
    assert_eq!(max_chars_arg(&json!({"max_chars": 999_999})), 200_000);
    assert_eq!(max_chars_arg(&json!({"max_chars": "40000"})), 40_000);
}

#[test]
fn page_missing_message_识别跨工具_page_id() {
    let m = page_missing_message("p_00000001");
    assert!(m.contains("MCP_Web_Chrome_Use") || m.contains("LsmAgentBrowser"), "m={m}");
    assert!(m.contains("不跨工具"), "m={m}");
    // 本工具自己的未知 id 走通用文案。
    let m2 = page_missing_message("c_00ff00ff");
    assert!(m2.contains("页面不存在"), "m2={m2}");
}

#[test]
fn png_size_解析_IHDR() {
    let mut png = vec![0u8; 24];
    png[0..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    png[12..16].copy_from_slice(b"IHDR");
    png[16..20].copy_from_slice(&1920u32.to_be_bytes());
    png[20..24].copy_from_slice(&1080u32.to_be_bytes());
    assert_eq!(crate::agent::chrome_browser::png_size(&png), Some((1920, 1080)));
    assert_eq!(crate::agent::chrome_browser::png_size(&[0u8; 10]), None);
}

#[tokio::test]
async fn 缺少_action_返_1001() {
    let tool = McpChromeUseTool;
    let out = tool.execute(json!({})).await.unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 1001);
}

#[tokio::test]
async fn 关闭开关返回_4030() {
    std::env::set_var("LAEW_CHROME_USE", "off");
    let tool = McpChromeUseTool;
    let out = tool.execute(json!({"action": "list"})).await.unwrap();
    std::env::remove_var("LAEW_CHROME_USE");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 4030);
}

#[tokio::test]
async fn 未知_action_返_1001() {
    let tool = McpChromeUseTool;
    let out = tool.execute(json!({"action": "fly"})).await.unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 1001);
    assert_eq!(v["data"]["action"], "fly");
}

#[tokio::test]
async fn close_未知_page_返_2004() {
    let tool = McpChromeUseTool;
    let out = tool
        .execute(json!({"action": "close", "page_id": "c_deadbeef"}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["code"], 2004);
}

#[test]
fn params_of_兼容顶层直传() {
    let args = json!({"selector": "#a"});
    assert_eq!(params_of(&args)["selector"], "#a");
    let args2 = json!({"params": {"selector": "#b"}, "selector": "#ignored"});
    assert_eq!(params_of(&args2)["selector"], "#b");
}

#[test]
fn i64_arg_兼容字符串数字() {
    assert_eq!(i64_arg(&json!({"y": 42}), "y"), Some(42));
    assert_eq!(i64_arg(&json!({"y": "-42"}), "y"), Some(-42));
    assert_eq!(i64_arg(&json!({"y": "x"}), "y"), None);
    assert_eq!(i64_arg(&json!({}), "y"), None);
}
