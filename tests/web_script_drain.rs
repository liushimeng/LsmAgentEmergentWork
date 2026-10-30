//! 脚本管线超时截断后的补泵/邮箱收割端到端(2026-10-30 京东根因)。
//!
//! 场景:页面首个 `<script>` 是超预算的重脚本(Boa 下京东 pc-tk.js 同型),
//! open 管线在宽限内等不到完成 → 截断上报;线程稍后跑完,DOM 动作堆在
//! 回复邮箱里等下一次请求合并带回。观察类动作(inspect/wait/screenshot)
//! 的补泵必须把邮箱收割回来 —— 跳过 = 模块挂载动作永不出邮箱,
//! 页面永远半成品(京东分类侧栏/促销楼层不挂载的直接机制)。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 起 mock 站点:首个脚本忙等 8s 后改 title + 挂 DOM 节点。
async fn start_site() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let Ok(n) = socket.read(&mut tmp).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                    if buf.len() > 64 * 1024 {
                        return;
                    }
                }
                let body = "<!doctype html><html><head><title>初值</title></head><body>\
                    <div id='x'>base</div>\
                    <script>\
                    var t0 = Date.now(); while (Date.now() - t0 < 8000) {}\
                    document.title = 'mounted';\
                    document.getElementById('x').textContent = 'heavy-ran';\
                    </script></body></html>";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{}", addr)
}

async fn exec(tool: &McpWebLsmBrowserUseTool, args: Value) -> Value {
    let text = tool.execute(args).await.unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON:{e}\n{text}"))
}

fn allow_loopback() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    std::env::set_var("LAEW_WEB_MAX_TABS", "32");
    // 管线预算压到下限 + 小宽限:重脚本必然超时截断。
    std::env::set_var("LAEW_WEB_SCRIPT_PIPELINE_MS", "2000");
    std::env::set_var("LAEW_WEB_SCRIPT_GRACE_MS", "300");
}

#[tokio::test]
async fn 管线截断后观察动作补泵收割邮箱完成挂载() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;

    let open = exec(
        &tool,
        json!({"action": "open", "url": base, "mode": "hidden", "reuse": false, "wait_until": "domcontentloaded"}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 观察动作触发补泵:inspect 自身带 run_pending_scripts,把线程迟到的
    // 「mounted/heavy-ran」动作从回复邮箱收割回流。
    let mut mounted = false;
    for _ in 0..4 {
        let st = exec(
            &tool,
            json!({"action": "inspect", "info": "title", "page_id": page_id, "params": {}}),
        )
        .await;
        if st["data"]["title"].as_str() == Some("mounted") {
            mounted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(mounted, "补泵应把线程迟到的挂载动作收割回流(标题未更新)");
    let txt = exec(
        &tool,
        json!({"action": "inspect", "info": "text", "page_id": page_id, "params": {"max_chars": 200}}),
    )
    .await;
    assert!(
        txt["data"]["text"].as_str().unwrap_or("").contains("heavy-ran"),
        "DOM 动作应已应用: {txt}"
    );
    let _ = exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}
