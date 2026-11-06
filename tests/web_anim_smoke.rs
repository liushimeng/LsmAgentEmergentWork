//! 第 57 轮:CSS 过渡/关键帧动画端到端冒烟(真时钟短过渡,本地 mock,无外网)。
//!
//! 断言用宽容差(起终点之间)防 CI 时序抖动;只验「动画值真的进了
//! 几何/JS getComputedStyle」这一条主线。

use std::time::Duration;

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
                }
                let body = r##"<!doctype html><html><head><style>
                html,body{margin:0;background:#fff}
                #t{width:100px;height:20px;background:#333;transition:width 2s linear}
                @keyframes fadeout { from { opacity: 1 } to { opacity: 0 } }
                #k{width:40px;height:40px;background:#c00;opacity:1;animation: fadeout 2s linear both}
                </style></head><body>
                <div id="t"></div><div id="k"></div>
                </body></html>"##;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{}", addr)
}

async fn exec(tool: &McpWebLsmBrowserUseTool, args: Value) -> Value {
    let text = tool.execute(args).await.expect("工具调用应成功");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON - {e}: {text}"))
}

async fn eval_js(tool: &McpWebLsmBrowserUseTool, page_id: &str, script: &str) -> Value {
    let v = exec(
        tool,
        json!({"action":"control","page_id":page_id,"control_action":"eval_js","params":{"script":script}}),
    )
    .await;
    assert_eq!(v["code"], 0, "eval_js 失败:{v}");
    v["data"].clone()
}

async fn width_of(tool: &McpWebLsmBrowserUseTool, page_id: &str, selector: &str) -> f64 {
    let v = exec(
        tool,
        json!({"action":"inspect","page_id":page_id,"info":"elements","params":{"selector":selector}}),
    )
    .await;
    assert_eq!(v["code"], 0, "inspect 失败:{v}");
    let data = v["data"].clone();
    // 默认模式回单元素对象,几何在 `rect`。
    data["rect"]["width"]
        .as_f64()
        .unwrap_or_else(|| panic!("应有 {selector} 几何盒:{data}"))
}

#[tokio::test]
async fn 过渡与关键帧动画值进几何与js计算样式() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action":"open","url":base,"mode":"hidden","reuse":false,"viewport":{"width":1280,"height":720}}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 起点:过渡未触发,几何 100。
    let w0 = width_of(&tool, &page_id, "#t").await;
    assert!((w0 - 100.0).abs() < 3.0, "起点宽度应 100,实际 {w0}");

    // 触发过渡:改内联 width → 计算值 100→300,2s 线性。
    let r = eval_js(
        &tool,
        &page_id,
        "document.getElementById('t').style.width='300px';'ok'",
    )
    .await;
    assert!(r["result"].as_str().unwrap_or("").contains("ok"), "{r}");

    // 半程(约 1s):宽度应在起终点之间(宽容差)。
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let w1 = width_of(&tool, &page_id, "#t").await;
    assert!(
        w1 > 120.0 && w1 < 280.0,
        "半程宽度应在 120~280 之间,实际 {w1}"
    );

    // 关键帧动画:open 后已跑约 1s(2s 动画),getComputedStyle.opacity 中段可见。
    let op = eval_js(
        &tool,
        &page_id,
        "String(getComputedStyle(document.getElementById('k')).opacity)",
    )
    .await;
    let op_v: f64 = op["result"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .expect(&format!("应读到 opacity:{op}"));
    assert!(
        op_v > 0.05 && op_v < 0.95,
        "关键帧中途 opacity 应在 (0.05,0.95),实际 {op_v}"
    );

    // 收尾:过渡到终点后几何应到 300。
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let w2 = width_of(&tool, &page_id, "#t").await;
    assert!((w2 - 300.0).abs() < 3.0, "终点宽度应 300,实际 {w2}");
}
