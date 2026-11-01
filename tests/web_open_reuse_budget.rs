//! 2026-11-01 天猫二轮回归:open 复用契约与整调用预算边界。
//!
//! 真机证据(tmall_02):
//! - Plan `open(url)` 命中同 URL 复用 → 全量重导航 327.7s 黑盒,且丢弃首开
//!   后台自建进度(JS 线程随导航重启)。契约:精确同 URL 默认**廉价复用**
//!   (不重新抓取,小额补泵返回现态),`reload=true` 才走全量导航 —— 与
//!   Chrome 工具「同 URL 自动复用无主页」语义对齐。
//! - 显式 `timeout_ms=60000` 实测 83.9s:图片阶段吃独立 20s 预算,在整调用
//!   预算之外。契约:whole-call 预算夹紧样式/脚本/图片全部后置阶段,预算
//!   归零时图片阶段直接跳过。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 带**请求计数**的 mock 站点:
/// - `/pg` 返回含 `<img src="/logo.png">` 的页面(响应 `Cache-Control: no-store`,
///   主文档每次都走网络,计数可判「是否重新抓取」);
/// - `/slow-doc` 睡 2.6s 再返回页面(消耗整调用预算,把图片阶段挤到归零);
/// - `/slow.png` 睡 6s 再返回图片(若图片阶段未被预算夹紧,调用必超 6s)。
async fn start_counting_site(counter: &'static AtomicUsize) -> String {
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
                counter.fetch_add(1, Ordering::SeqCst);
                let head = String::from_utf8_lossy(&buf).to_string();
                let first = head.lines().next().unwrap_or("");
                let path = first.split_whitespace().nth(1).unwrap_or("/");
                let (status, ctype, body, extra): (&str, &str, Vec<u8>, &str) = match path {
                    "/pg" => (
                        "200 OK",
                        "text/html; charset=utf-8",
                        b"<html><head><title>reuse</title></head><body><h1>R</h1>\
                          <img src=\"/logo.png\"></body></html>"
                            .to_vec(),
                        "Cache-Control: no-store",
                    ),
                    "/slow-doc" => {
                        tokio::time::sleep(std::time::Duration::from_millis(2_600)).await;
                        (
                            "200 OK",
                            "text/html; charset=utf-8",
                            b"<html><head><title>slow</title></head><body><h1>S</h1>\
                              <img src=\"/slow.png\"></body></html>"
                                .to_vec(),
                            "Cache-Control: no-store",
                        )
                    }
                    _ => (
                        "200 OK",
                        "image/png",
                        png_bytes(),
                        "",
                    ),
                };
                let mut header = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n",
                    body.len()
                );
                if !extra.is_empty() {
                    header.push_str(extra);
                    header.push_str("\r\n");
                }
                header.push_str("Connection: close\r\n\r\n");
                let mut resp = header.into_bytes();
                resp.extend_from_slice(&body);
                let _ = socket.write_all(&resp).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{}", addr)
}

/// 4x4 纯红 PNG。
fn png_bytes() -> Vec<u8> {
    let mut img = image::RgbaImage::new(4, 4);
    for (_x, _y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgba([255, 0, 0, 255]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

async fn exec(tool: &McpWebLsmBrowserUseTool, args: Value) -> Value {
    let text = tool.execute(args).await.unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON:{e}\n{text}"))
}

fn allow_loopback() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    std::env::set_var("LAEW_WEB_MAX_TABS", "32");
}

/// 精确同 URL 二次 open = 廉价复用(不重新抓取主文档);reload=true 才重载。
#[tokio::test]
async fn open同url二次为廉价复用_reload为全量重载() {
    allow_loopback();
    static HITS: AtomicUsize = AtomicUsize::new(0);
    let base = start_counting_site(&HITS).await;
    let tool = McpWebLsmBrowserUseTool;
    let url = format!("{base}/pg");

    let first = exec(
        &tool,
        json!({"action": "open", "url": &url, "mode": "hidden", "reuse": true}),
    )
    .await;
    assert_eq!(first["code"], 0, "{first}");
    let hits_after_first = HITS.load(Ordering::SeqCst);
    assert!(hits_after_first >= 1, "首开应抓取主文档(及其图片)");

    let second = exec(
        &tool,
        json!({"action": "open", "url": &url, "mode": "hidden", "reuse": true}),
    )
    .await;
    assert_eq!(second["code"], 0, "{second}");
    assert_eq!(second["data"]["reused"], true, "{second}");
    assert_eq!(
        second["data"]["timings"]["reused"],
        true,
        "廉价复用应在 timings 标注:{second}"
    );
    assert_eq!(
        HITS.load(Ordering::SeqCst),
        hits_after_first,
        "精确同 URL 廉价复用不应重新抓取主文档(命中数不得增长)"
    );

    let third = exec(
        &tool,
        json!({"action": "open", "url": &url, "mode": "hidden", "reuse": true, "reload": true}),
    )
    .await;
    assert_eq!(third["code"], 0, "{third}");
    assert!(
        HITS.load(Ordering::SeqCst) > hits_after_first,
        "reload=true 应触发全量重导航重新抓取主文档"
    );
    // 清理:关页,不给共享单例留尾巴。
    let pid = third["data"]["page_id"].as_str().unwrap().to_string();
    let _ = exec(&tool, json!({"action": "close", "page_id": pid})).await;
}

/// 显式整调用预算必须覆盖图片阶段:慢主文档(2.6s)把 3s 预算耗尽,
/// 6s 慢图阶段应被「预算归零跳过」,总时长收敛在预算附近而不是 8s+。
#[tokio::test]
async fn 显式timeout预算夹紧图片阶段() {
    allow_loopback();
    static HITS: AtomicUsize = AtomicUsize::new(0);
    let base = start_counting_site(&HITS).await;
    let tool = McpWebLsmBrowserUseTool;
    let url = format!("{base}/slow-doc");

    let started = std::time::Instant::now();
    let out = exec(
        &tool,
        json!({"action": "open", "url": &url, "mode": "hidden", "reuse": false,
               "timeout_ms": 3_000}),
    )
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    assert_eq!(out["code"], 0, "{out}");
    // 2.6s 抓取 + 截断的管线 + 跳过的图片 + 布局:总时长应远小于
    // 「抓取 + 6s 慢图」的旧无夹紧行为(≥ 8.6s)。容差放到 6.5s
    // 防 CI 抖动,但足以判死「图片阶段照跑」的回归。
    assert!(
        elapsed_ms < 6_500,
        "整调用预算 3s,实际 {elapsed_ms}ms —— 图片阶段未被预算夹紧(旧无夹紧行为 ≥ 8.6s)"
    );
    let pid = out["data"]["page_id"].as_str().unwrap().to_string();
    let _ = exec(&tool, json!({"action": "close", "page_id": pid})).await;
}
