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
