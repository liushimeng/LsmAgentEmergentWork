//! MCP_Web_Use 真浏览器冒烟测试(需要本机安装 Chrome/Edge;离线,用 data: URL)。
//!
//! 2026-09-18 第 89 轮:原 Chromium-WebUse 5 工具冒烟改写为单工具 action 分发形态。
//! 单测试函数串行执行(BrowserManager 是进程内单例,并行多测试会互相回收浏览器)。
//! 默认 `#[ignore]`,显式运行:`cargo test --test web_use_smoke -- --ignored`

use lsm_agent::agent::browser::detect_browser;
use lsm_agent::agent::tools::mcp_web_use::McpWebUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};

// data: URL 中属性一律单引号(未编码双引号会被 Chrome 截断解析)
const PAGE: &str = "data:text/html,<html><head><title>laew-smoke</title></head>\
<body><input id='q' value=''><button id='go' onclick='document.title=\"clicked\"'>Go</button>\
<h1 id='h'>hello laew</h1></body></html>";

fn data_of(out: &str) -> Value {
    serde_json::from_str(out).unwrap_or(Value::Null)
}

#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_open_control_inspect_smoke() {
    // 0) 信封永不 panic:无浏览器环境返回 code=3001 + 安装引导;有浏览器 code=0。
    //    先验证这一点,成功打开的页面立即关闭(不留脏状态,后续步骤重新开页)。
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": "data:text/html,<title>x</title>"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert!(
        v["code"] == 0 || v["code"] == 3001,
        "open 返回码应为 0 或 3001: {out}"
    );
    if v["code"] == 3001 {
        // 本机未安装浏览器:后续步骤的前提不成立,信封验证即完成
        return;
    }
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": v["data"]["page_id"]}))
        .await;

    // 1) 检测到浏览器
    assert!(
        detect_browser().is_some(),
        "本机应能检测到 Chromium 系浏览器"
    );

    // 2) action=open 打开 data: 页面(离线)
    let out = McpWebUseTool.execute(json!({"action": "open", "url": PAGE})).await.unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "open 应成功: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();

    // 3) action=inspect title
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "title"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["title"], "laew-smoke");

    // 4) action=control input_text(JS 原生 setter 路径)
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "input_text",
            "params": {"selector": "#q", "text": "你好 laew"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "input_text 应成功: {out}");

    // 5) eval_js 读回输入值
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "eval_js",
            "params": {"expression": "document.querySelector('#q').value"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["data"]["result"], "你好 laew");

    // 6) click 按钮改标题
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "click",
            "params": {"selector": "#go"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "click 应成功: {out}");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "title"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["data"]["title"], "clicked", "点击后标题应变");

    // 7) elements 检视
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "elements", "params": {"selector": "#h"}}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["text"], "hello laew");

    // 8) 截图落盘
    let path = std::env::temp_dir().join(format!(
        "laew_smoke_{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "screenshot",
            "params": {"save_path": path.to_str().unwrap()}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "screenshot 应成功: {out}");
    assert!(path.exists() && std::fs::metadata(&path).unwrap().len() > 100);

    // 9) action=close 幂等
    let out = McpWebUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    let out = McpWebUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 2000, "重复关闭应 2000");
}
