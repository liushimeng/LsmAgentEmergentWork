//! 真机冒烟:LsmAgentBrowser 侧控制权两态 + 1080p 视口缺省。
//!
//! **需真机 GUI 会话**(headed 查看器),默认不参与 CI:本地跑
//! `LAEW_BROWSER_MODE=headed cargo test --test lsm_control_state_smoke -- --ignored --nocapture`。

use serde_json::{json, Value};

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;

fn parse(out: &str) -> Value {
    serde_json::from_str(out).unwrap_or_else(|e| panic!("信封不是 JSON: {out} ({e})"))
}

#[tokio::test]
#[ignore = "需真机 GUI 会话(本地 --ignored 跑)"]
async fn 真机_lsm控制权两态与1080p缺省() {
    // 1) 开页:缺省视口应为 1920×1080,控制权为 agent
    let out = McpWebLsmBrowserUseTool
        .execute(json!({
            "action": "open", "url": "about:blank", "mode": "headed"
        }))
        .await
        .unwrap();
    let v = parse(&out);
    assert_eq!(v["code"], 0, "{v}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();
    assert_eq!(v["data"]["control_state"], "agent", "{v}");
    assert_eq!(
        v["data"]["window"]["effective"]["width"], 1920,
        "缺省视口宽应为 1920(1080p):{v}"
    );
    assert_eq!(
        v["data"]["window"]["effective"]["height"], 1080,
        "缺省视口高应为 1080(1080p):{v}"
    );

    // 2) 切用户控制状态
    let sw = McpWebLsmBrowserUseTool
        .execute(json!({
            "action": "control", "page_id": page_id,
            "control_action": "set_control_state",
            "params": {"state": "user", "note": "冒烟:人工接管"}
        }))
        .await
        .unwrap();
    let sv = parse(&sw);
    println!("切换响应 = {sv}");
    assert_eq!(sv["code"], 0, "{sv}");
    assert_eq!(sv["data"]["control_state"], "user", "{sv}");
    assert_eq!(sv["data"]["result"]["mask_active"], false, "{sv}");

    // 3) 用户控制状态下 Agent 写操作被拒(4004),观测动作放行
    let denied = McpWebLsmBrowserUseTool
        .execute(json!({
            "action": "control", "page_id": page_id,
            "control_action": "click", "params": {"selector": "body"}
        }))
        .await
        .unwrap();
    let dv = parse(&denied);
    println!("用户态写操作响应 = {dv}");
    assert_eq!(dv["code"], 4004, "{dv}");

    let insp = McpWebLsmBrowserUseTool
        .execute(json!({
            "action": "inspect", "page_id": page_id, "info": "url"
        }))
        .await
        .unwrap();
    assert_eq!(parse(&insp)["code"], 0);

    // 4) 取回控制权
    let back = McpWebLsmBrowserUseTool
        .execute(json!({
            "action": "control", "page_id": page_id,
            "control_action": "set_control_state",
            "params": {"state": "agent"}
        }))
        .await
        .unwrap();
    let bv = parse(&back);
    assert_eq!(bv["data"]["control_state"], "agent", "{bv}");
    assert_eq!(bv["data"]["result"]["mask_active"], true, "{bv}");

    // 5) list 报控制权状态
    let list = McpWebLsmBrowserUseTool.execute(json!({"action": "list"})).await.unwrap();
    let lv = parse(&list);
    println!("list = {lv}");
    assert_eq!(lv["data"]["pages"][0]["control_state"], "agent", "{lv}");

    // 6) 关页
    let closed = McpWebLsmBrowserUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await
        .unwrap();
    assert_eq!(parse(&closed)["code"], 0);
}
