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
