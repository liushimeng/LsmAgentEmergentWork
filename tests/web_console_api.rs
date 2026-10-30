//! 控制台标准方法在站点初始化链中的端到端回归测试。

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
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let body = "<html><body><div id=\"result\">初始化中断</div><script>\
                            setTimeout(console.clear.bind(console), 500);\
                            document.getElementById('result').textContent = 'console 初始化链完整';\
                            </script></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn console_clear可被bind并注册为初始化定时器() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = tool
        .execute(json!({"action":"open","url":base,"mode":"hidden","reuse":false}))
        .await
        .unwrap();
    let open: Value = serde_json::from_str(&open).unwrap();
    assert_eq!(open["code"], 0, "mock 页面应打开成功: {open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let inspected = tool
        .execute(json!({"action":"inspect","info":"elements","page_id":page_id,
                        "params":{"selector":"#result"}}))
        .await
        .unwrap();
    let inspected: Value = serde_json::from_str(&inspected).unwrap();
    assert_eq!(
        inspected["data"]["text"].as_str().unwrap_or(""),
        "console 初始化链完整",
        "console.clear.bind 不应中断导航初始化: {inspected}"
    );
    let _ = tool.execute(json!({"action":"close","page_id":page_id})).await;
}
