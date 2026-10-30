//! 资源加载回归(第 45 轮):CSS `@import` 内联 + 响应式图片 `srcset` 候选选择。
//!
//! 本地 mock,无外网。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn allow_loopback() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
}

/// 生成指定尺寸的纯色 PNG。
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut img = image::RgbaImage::new(w, h);
    for px in img.pixels_mut() {
        *px = image::Rgba([10, 120, 200, 255]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

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
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                let (ctype, body): (&str, Vec<u8>) = match path.as_str() {
                    "/page" => (
                        "text/html; charset=utf-8",
                        r##"<!doctype html><html><head>
                        <link rel="stylesheet" href="/a.css">
                        </head><body>
                        <p id="from-b" class="from-b">导入表里的规则</p>
                        <img id="resp" srcset="/img1x.png 1x, /img2x.png 2x" style="width:20px;height:10px">
                        </body></html>"##
                            .as_bytes()
                            .to_vec(),
                    ),
                    "/a.css" => (
                        "text/css; charset=utf-8",
                        b"@import url(/b.css);\n.from-a{color:#00ff00}".to_vec(),
                    ),
                    "/b.css" => (
                        "text/css; charset=utf-8",
                        b".from-b{color:#0000ff}".to_vec(),
                    ),
                    "/img1x.png" => ("image/png", png_bytes(10, 5)),
                    "/img2x.png" => ("image/png", png_bytes(20, 10)),
                    _ => ("text/plain; charset=utf-8", b"not found".to_vec()),
                };
                let mut resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                resp.extend_from_slice(&body);
                let _ = socket.write_all(&resp).await;
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

#[tokio::test]
async fn import的样式表被内联并生效() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/page"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // `@import` 引入的表里的规则必须参与计算样式。
    let el = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
               "params": {"selector": "#from-b", "include_style": true}}),
    )
    .await;
    assert_eq!(
        el["data"]["computed_style"]["color"], "rgb(0, 0, 255)",
        "@import 的样式表应被内联并生效:{el}"
    );
}

#[tokio::test]
async fn srcset按密度选出候选并预取() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/page"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 密度描述符:1x 视口应选 1x 候选(内核 DPR 恒为 1)。
    let net = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "network"}),
    )
    .await;
    let urls: Vec<String> = net["data"]["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r["url"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        urls.iter().any(|u| u.ends_with("/img1x.png")),
        "应预取 srcset 选中的 1x 候选:{urls:?}"
    );
    assert!(
        !urls.iter().any(|u| u.ends_with("/img2x.png")),
        "1x 视口不应预取 2x 候选:{urls:?}"
    );
}
