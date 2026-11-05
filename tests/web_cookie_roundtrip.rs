//! 闲鱼真机回归(2026-11-05)的两条契约固化,mock HTTP 无外网:
//! 1. XHR/fetch 收到的 Set-Cookie 必须随下一次请求回发(批内令牌往返);
//!    且要回传宿主 CookieJar(refresh 不丢)。根因:mtop `_m_h5_tk` 令牌
//!    拿不到 → FAIL_SYS_TOKEN_EMPTY 死循环 → 首页骨架永不填充。
//! 2. open(page_id=X, url=Y≠X.url) 必须在 X 页内原地导航,不得悄悄另开新页。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn start_site() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                let header_end = loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    };
                    if data.len() > 64 * 1024 {
                        return;
                    }
                };
                let head = String::from_utf8_lossy(&data[..header_end]).to_string();
                let request_line = head.lines().next().unwrap_or("");
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let cookie_header = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("cookie").then(|| value.trim().to_string())
                    })
                    .unwrap_or_default();
                let (extra_headers, body): (String, String) = if path == "/set-cookie" {
                    // 模拟 mtop 网关:响应带 Set-Cookie 下发令牌。
                    (
                        "Set-Cookie: tk=roundtrip123; Path=/\r\n".to_string(),
                        "{\"ok\":true}".to_string(),
                    )
                } else if path == "/echo-cookie" {
                    // 回显请求携带的 Cookie 头,验证令牌是否随第二次请求回发。
                    (
                        String::new(),
                        json!({"cookie": cookie_header, "documentCookieSeen": false}).to_string(),
                    )
                } else if path == "/page2" {
                    (
                        String::new(),
                        "<html><head><title>第二页</title></head><body><div id=\"mark\">PAGE2</div></body></html>"
                            .to_string(),
                    )
                } else {
                    (
                        String::new(),
                        "<html><head><title>首页</title></head><body><div id=\"mark\">HOME</div><script>\
                         fetch('/set-cookie').then(function(r){ return r.text(); }).then(function(){\
                           var seen = String(document.cookie);\
                           return fetch('/echo-cookie').then(function(r){ return r.text(); }).then(function(t){\
                             var d = document.createElement('div'); d.id = 'result';\
                             d.textContent = JSON.stringify({cookieInSecond: t, docCookieAfterSet: seen});\
                             document.body.appendChild(d);\
                           });\
                         }).catch(function(e){\
                           var d = document.createElement('div'); d.id = 'result';\
                           d.textContent = 'ERROR:' + String(e);\
                         });\
                         </script></body></html>"
                            .to_string(),
                    )
                };
                let ct = if path.starts_with("/set-cookie") || path.starts_with("/echo-cookie") {
                    "application/json"
                } else {
                    "text/html; charset=utf-8"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    ct,
                    extra_headers,
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

async fn exec(tool: &McpWebLsmBrowserUseTool, args: Value) -> Value {
    let text = tool.execute(args).await.expect("工具调用应成功");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON - {e}: {text}"))
}

#[tokio::test]
async fn xhr收到的set_cookie必须随下一次请求回发且落入宿主jar() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action":"open","url":base,"mode":"hidden","reuse":false}),
    )
    .await;
    assert_eq!(open["code"], 0, "mock 页面应打开成功: {open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 等脚本管线把 fetch 链跑完。
    let wait = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"wait",
               "params":{"selector":"#result","state":"attached","timeout_ms":15000}}),
    )
    .await;
    assert_eq!(wait["code"], 0, "脚本链应收敛: {wait}");

    let result = exec(
        &tool,
        json!({"action":"inspect","info":"elements","page_id":page_id,
               "params":{"selector":"#result"}}),
    )
    .await;
    let text = result["data"]["text"].as_str().unwrap_or("");
    assert!(!text.contains("ERROR:"), "fetch 链不应报错: {result}");
    assert!(
        text.contains("tk=roundtrip123"),
        "第二次请求必须带上第一次响应下发的令牌 cookie(批内往返): {text}"
    );

    // 宿主 CookieJar 也必须拿到(refresh 用 jar 整串覆盖 JS 侧,不回传就丢)。
    let cookies = exec(
        &tool,
        json!({"action":"inspect","info":"cookies","page_id":page_id}),
    )
    .await;
    let cookie_text = serde_json::to_string(&cookies).unwrap_or_default();
    assert!(
        cookie_text.contains("tk") && cookie_text.contains("roundtrip123"),
        "宿主 CookieJar 应含 set-cookie 动作回传的令牌: {cookie_text}"
    );

    exec(&tool, json!({"action":"close","page_id":"all"})).await;
}

#[tokio::test]
async fn open带page_id与不同url时必须在该页原地导航() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    // 与上一测试共享全局内核(cargo test 默认并行):先清场,断言只看自己开的页。
    let tool = McpWebLsmBrowserUseTool;
    let _ = exec(&tool, json!({"action":"close","page_id":"all"})).await;
    let base = start_site().await;
    let open = exec(
        &tool,
        json!({"action":"open","url":base,"mode":"hidden","reuse":false}),
    )
    .await;
    assert_eq!(open["code"], 0);
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 同 page_id + 不同 URL:期望原地导航,不再另开新页。
    let nav = exec(
        &tool,
        json!({"action":"open","page_id":page_id,"url":format!("{base}/page2"),"mode":"hidden"}),
    )
    .await;
    assert_eq!(nav["code"], 0, "原地导航应成功: {nav}");
    assert_eq!(
        nav["data"]["page_id"].as_str().unwrap_or(""),
        page_id,
        "open(page_id,新URL) 必须复用点名页面,返回同一 page_id: {nav}"
    );
    assert!(
        nav["data"]["final_url"]
            .as_str()
            .unwrap_or("")
            .contains("/page2"),
        "页面应已导航到新 URL: {nav}"
    );

    // 页面总量应为 1(不得悄悄开新页),且该页内容就是第二页。
    let list = exec(&tool, json!({"action":"list"})).await;
    let pages = list["data"]["pages"].as_array().cloned().unwrap_or_default();
    assert_eq!(pages.len(), 1, "open(page_id,新URL) 不得另开新页: {list}");
    assert_eq!(
        pages[0]["title"].as_str().unwrap_or(""),
        "第二页",
        "点名页应已加载新文档: {list}"
    );

    exec(&tool, json!({"action":"close","page_id":"all"})).await;
}
