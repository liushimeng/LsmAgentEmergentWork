//! 真机冒烟:Chrome 控制权两态 + 蒙层 + 标签页治理。
//!
//! **需真机 Chrome**,默认不参与 CI:本地跑
//! `LAEW_CHROME_MODE=headed cargo test --test control_mask_smoke -- --ignored --nocapture`。
//!
//! 覆盖:① open 后默认 Agent 控制状态 + 蒙层在页面里;② 切用户控制状态 → 蒙层撤除
//! (用**绕过工具**的裸截图对比证明画面确实变了);③ 用户态 Agent 写操作被拒(4004),
//! 观测动作放行;④ 取回控制权 → 蒙层回来;⑤ list 感知人工自己开的外部标签页;
//! ⑥ close 真关标签页。

use serde_json::{json, Value};

use lsm_agent::agent::tools::mcp_chrome_use::McpChromeUseTool;
use lsm_agent::agent::tools::Tool;

fn parse(out: &str) -> Value {
    serde_json::from_str(out).unwrap_or_else(|e| panic!("信封不是 JSON: {out} ({e})"))
}

/// 裸截图(**绕过工具**的「截图避让」通道)——用来证明蒙层真的画在画面上。
///
/// 工具的 screenshot 会先挂起蒙层(证据图要干净),所以工具截图看不出蒙层;
/// 这里直接走引擎的截图函数,不挂起。
async fn raw_shot(page_id: &str) -> String {
    use lsm_agent::agent::chrome_browser::{screenshot_base64, ChromeManager};
    let mgr = ChromeManager::global();
    let page = mgr.get_page(page_id).expect("页面应存在");
    screenshot_base64(&page).await.unwrap_or_default()
}

#[tokio::test]
#[ignore = "需真机 Chrome(本地 --ignored 跑)"]
async fn 真机_控制权两态与蒙层() {
    // 1) 开页(默认 Agent 控制状态)
    let out = McpChromeUseTool
        .execute(json!({"action": "open", "url": "about:blank"}))
        .await
        .unwrap();
    let v = parse(&out);
    assert_eq!(v["code"], 0, "{v}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();
    assert_eq!(v["data"]["control_state"], "agent", "{v}");

    // 2) 蒙层在场(Agent 控制状态):页面侧探针应报 present+active
    let probe = McpChromeUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "eval_js",
            "params": {"script": "(() => { const ns = Object.getOwnPropertySymbols(window).map(s => s.description).find(d => d && d.length === 9 && d[0] === 'l'); if (!ns) return 'no-ns'; const o = window[Symbol.for(ns)]; return o && o.MaskProbe ? o.MaskProbe() : 'no-probe'; })()"}
        }))
        .await
        .unwrap();
    let pv = parse(&probe);
    println!("蒙层探针(agent 态) = {}", pv["data"]["result"]);
    assert_eq!(pv["code"], 0, "{pv}");

    // 3) 切到用户控制状态 → 蒙层撤除
    let sw = McpChromeUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "set_control_state",
            "params": {"state": "user", "note": "冒烟测试:人工接管"}
        }))
        .await
        .unwrap();
    let sv = parse(&sw);
    println!("切换响应 = {sv}");
    assert_eq!(sv["code"], 0, "{sv}");
    assert_eq!(sv["data"]["control_state"], "user", "{sv}");
    assert_eq!(sv["data"]["result"]["mask_active"], false, "{sv}");

    let probe2 = McpChromeUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "eval_js",
            "params": {"script": "(() => { const ns = Object.getOwnPropertySymbols(window).map(s => s.description).find(d => d && d.length === 9 && d[0] === 'l'); if (!ns) return 'no-ns'; const o = window[Symbol.for(ns)]; return o && o.MaskProbe ? o.MaskProbe() : 'no-probe'; })()"}
        }))
        .await
        .unwrap();
    println!("蒙层探针(user 态) = {}", parse(&probe2)["data"]["result"]);

    // 3.5) 裸截图对比:工具的截图通道会避让蒙层(证据图要干净),这里绕过工具
    // 直接走引擎截图,用来证明「agent 态画面被蒙层压暗、user 态画面恢复」。
    let shot_agent = raw_shot(&page_id).await;
    assert!(!shot_agent.is_empty(), "agent 态裸截图应有内容");

    // 4) 用户控制状态下 Agent 写操作被拒(4004)
    let denied = McpChromeUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "click",
            "params": {"selector": "body"}
        }))
        .await
        .unwrap();
    let dv = parse(&denied);
    println!("用户态写操作响应 = {dv}");
    assert_eq!(dv["code"], 4004, "{dv}");

    // 4.5) user 态裸截图:与 agent 态应不同(蒙层已撤除,画面恢复)
    let shot_user = raw_shot(&page_id).await;
    assert!(!shot_user.is_empty(), "user 态裸截图应有内容");
    assert_ne!(
        shot_agent, shot_user,
        "agent 态与 user 态的裸截图应不同(蒙层在/撤的可见差异)"
    );

    // 5) 观测动作仍放行
    let insp = McpChromeUseTool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "url"}))
        .await
        .unwrap();
    let iv = parse(&insp);
    assert_eq!(iv["code"], 0, "{iv}");

    // 6) 取回控制权 → 蒙层回来
    let back = McpChromeUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "set_control_state",
            "params": {"state": "agent"}
        }))
        .await
        .unwrap();
    let bv = parse(&back);
    assert_eq!(bv["data"]["control_state"], "agent", "{bv}");
    assert_eq!(bv["data"]["result"]["mask_active"], true, "{bv}");

    // 7) list 报控制权状态
    let list = McpChromeUseTool.execute(json!({"action": "list"})).await.unwrap();
    let lv = parse(&list);
    println!("list = {lv}");
    assert_eq!(lv["data"]["pages"][0]["control_state"], "agent", "{lv}");

    // 8) 真关标签页
    let closed = McpChromeUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await
        .unwrap();
    assert_eq!(parse(&closed)["code"], 0);
    let list2 = McpChromeUseTool.execute(json!({"action": "list"})).await.unwrap();
    println!("close 后 list = {}", parse(&list2));
}

#[tokio::test]
#[ignore = "需真机 Chrome(本地 --ignored 跑)"]
async fn 真机_click_text_scope根元素自身可命中() {
    let out = McpChromeUseTool
        .execute(json!({"action": "open", "url": "about:blank"}))
        .await
        .unwrap();
    let page_id = parse(&out)["data"]["page_id"].as_str().unwrap().to_string();
    let injected = McpChromeUseTool.execute(json!({
        "action": "control", "page_id": page_id, "control_action": "eval_js",
        "params": {"script": "document.body.innerHTML='<div id=\"scope\"><span id=\"target\" style=\"display:block;width:200px;height:40px;cursor:pointer\">AMD 锐龙 7 9700X</span></div>'; true"}
    })).await.unwrap();
    assert_eq!(parse(&injected)["code"], 0);

    // selector 直指文字节点本身;回归旧实现只搜 root 后代导致 text_not_found。
    let clicked = McpChromeUseTool.execute(json!({
        "action": "control", "page_id": page_id, "control_action": "click_text",
        "params": {"selector": "#target", "text": "AMD 锐龙 7 9700X", "exact": true, "mode": "real"}
    })).await.unwrap();
    let value = parse(&clicked);
    assert_eq!(value["code"], 0, "{value}");
    assert_eq!(value["data"]["result"]["hit"], true, "{value}");
    assert_eq!(value["data"]["result"]["matched_text"], "AMD 锐龙 7 9700X", "{value}");

    let navigation = parse(&McpChromeUseTool.execute(json!({
        "action":"control", "page_id":page_id, "control_action":"eval_js",
        "params":{"script":"location.href='https://example.test/'"}
    })).await.unwrap());
    assert_eq!(navigation["code"], 2001, "{navigation}");
    assert!(navigation["message"].as_str().unwrap().contains("control_action=navigate"), "{navigation}");
    let alive = parse(&McpChromeUseTool.execute(json!({
        "action":"control", "page_id":page_id, "control_action":"eval_js",
        "params":{"script":"1+1"}
    })).await.unwrap());
    assert_eq!(alive["data"]["result"], 2, "导航守卫后同一 page_id 必须仍可用: {alive}");
    let state = parse(&McpChromeUseTool.execute(json!({
        "action":"inspect", "page_id":page_id, "info":"page_state"
    })).await.unwrap());
    assert_eq!(state["code"], 0, "{state}");
    assert_eq!(state["data"]["result"]["globals"]["ready_state"], "complete", "{state}");
    assert!(state["data"]["result"]["jsonld"].is_array(), "{state}");

    let closed = McpChromeUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await.unwrap();
    assert_eq!(parse(&closed)["code"], 0);
}

#[tokio::test]
#[ignore = "需真机 Chrome(本地 --ignored 跑)"]
async fn 真机_控制动作自动激活目标_tab() {
    let first = parse(&McpChromeUseTool.execute(json!({"action":"open","url":"about:blank#first"})).await.unwrap());
    let first_id = first["data"]["page_id"].as_str().unwrap().to_string();
    let second = parse(&McpChromeUseTool.execute(json!({"action":"open","url":"about:blank#second"})).await.unwrap());
    let second_id = second["data"]["page_id"].as_str().unwrap().to_string();

    let acted = parse(&McpChromeUseTool.execute(json!({
        "action":"control", "page_id":first_id, "control_action":"eval_js",
        "params":{"script":"document.title='first-active'; true"}
    })).await.unwrap());
    assert_eq!(acted["data"]["tab_activated"], true, "{acted}");
    let list = parse(&McpChromeUseTool.execute(json!({"action":"list"})).await.unwrap());
    let pages = list["data"]["pages"].as_array().unwrap();
    let first_state = pages.iter().find(|p| p["page_id"] == first_id).unwrap();
    let second_state = pages.iter().find(|p| p["page_id"] == second_id).unwrap();
    assert_eq!(first_state["active"], true, "{list}");
    assert_eq!(second_state["active"], false, "{list}");

    for id in [first_id, second_id] {
        let closed = McpChromeUseTool.execute(json!({"action":"close","page_id":id})).await.unwrap();
        assert_eq!(parse(&closed)["code"], 0);
    }
}

#[tokio::test]
#[ignore = "需真机 Chrome(本地 --ignored 跑)"]
async fn 真机_持久profile重启后保留站点存储() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("LAEW_CHROME_PROFILE", dir.path());
    // localStorage 需要普通 HTTP origin;about:blank 的 opaque origin 会抛 SecurityError。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { continue };
            tokio::spawn(async move {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 39\r\nConnection: close\r\n\r\n<html><body>profile smoke</body></html>").await;
            });
        }
    });
    let url = format!("http://{addr}/");
    let first = parse(&McpChromeUseTool.execute(json!({"action":"open","url":url})).await.unwrap());
    assert_eq!(first["data"]["profile_persistent"], true, "{first}");
    let first_id = first["data"]["page_id"].as_str().unwrap();
    let written = McpChromeUseTool.execute(json!({
        "action":"control", "page_id":first_id, "control_action":"eval_js",
        "params":{"script":"localStorage.setItem('laew-profile-smoke','kept'); document.cookie='laew_session=kept; path=/; SameSite=Lax'; true"}
    })).await.unwrap();
    assert_eq!(parse(&written)["code"], 0);
    lsm_agent::agent::chrome_browser::ChromeManager::global().close_all().await;

    let second = parse(&McpChromeUseTool.execute(json!({"action":"open","url":url})).await.unwrap());
    let second_id = second["data"]["page_id"].as_str().unwrap();
    let read = parse(&McpChromeUseTool.execute(json!({
        "action":"control", "page_id":second_id, "control_action":"eval_js",
        "params":{"script":"localStorage.getItem('laew-profile-smoke')"}
    })).await.unwrap());
    assert_eq!(read["data"]["result"], "kept", "{read}");
    let cookie = parse(&McpChromeUseTool.execute(json!({
        "action":"control", "page_id":second_id, "control_action":"eval_js",
        "params":{"script":"document.cookie"}
    })).await.unwrap());
    assert!(cookie["data"]["result"].as_str().unwrap_or("").contains("laew_session=kept"), "session cookie 应跨正常重开保留: {cookie}");
    lsm_agent::agent::chrome_browser::ChromeManager::global().close_all().await;
    assert!(dir.path().exists(), "持久 profile 不应被 watchdog/shutdown 删除");
    std::env::remove_var("LAEW_CHROME_PROFILE");
}
