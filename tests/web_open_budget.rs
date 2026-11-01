//! open 契约回归(2026-11-01 天猫真机):显式 `timeout_ms` 约束整条调用、
//! `wait_until=domcontentloaded` 首轮即返、截断事实随 open 响应如实上报。
//!
//! 真机实测:`open(timeout_ms=45000, wait_until=domcontentloaded)` 实际
//! 282.9s 才返回 —— `wait_until` 只在 schema 里零消费,`timeout_ms` 只约束
//! 网络抓取,脚本管线独立吃满 240s(+2×60s 延长)预算。本文件锁死:
//! 1. 显式 timeout_ms → 抓取+管线共享预算,超限以 script_budget_cut/script_pending 如实上报;
//! 2. wait_until=domcontentloaded → 管线只跑首轮,后续脚本留待补泵,open 响应直接可见;
//! 3. 不传 timeout_ms → 维持旧行为(整管线,不被 60s 默认值误伤)。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 起 mock 站点:首个脚本忙等 8s(远超调用方预算)后再写标题。
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
                // 跨轮结构:首轮脚本忙等 3s 后往 DOM 注入**文本 script**
                // (无 src,宿主下一轮 collect 才执行)——管线轮次/预算夹紧
                // 才有确定性的切割点;单条忙等脚本无法中途打断。
                let body = "<!doctype html><html><head><title>初值</title></head><body>\
                    <div id='x'>base</div>\
                    <script>\
                    var t0 = Date.now(); while (Date.now() - t0 < 3000) {}\
                    var s = document.createElement('script');\
                    s.text = \"document.title='mounted'; document.getElementById('x').textContent='round2-ran';\";\
                    document.body.appendChild(s);\
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
    // 管线预算放到默认大值:证明夹紧来自调用方 timeout_ms,而非 env 小预算。
    std::env::set_var("LAEW_WEB_SCRIPT_PIPELINE_MS", "240000");
}

#[tokio::test]
async fn 显式timeout_ms约束整条open调用并如实上报截断() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;

    let t0 = Instant::now();
    let open = exec(
        &tool,
        json!({"action": "open", "url": base, "mode": "hidden", "reuse": false, "timeout_ms": 1500}),
    )
    .await;
    let elapsed = t0.elapsed();
    assert_eq!(open["code"], 0, "{open}");
    assert!(
        elapsed < Duration::from_secs(20),
        "显式 1500ms 预算不得拖满默认 240s 管线(3s 忙等页也必须在预算附近收口;20s 上界已含并行测试的 CPU 争抢余量),实际 {elapsed:?}"
    );
    assert_eq!(
        open["data"]["wait_until"], "load",
        "缺省 wait_until 应回显 load: {open}"
    );
    let cut = open["data"]["script_budget_cut"].as_bool().unwrap_or(false);
    assert!(
        cut,
        "预算截断必须随 open 响应如实上报(script_budget_cut=true): {open}"
    );
    assert_eq!(
        open["data"]["title"].as_str(),
        Some("初值"),
        "预算截断后注入脚本未跑,标题应停在初值: {open}"
    );
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let _ = exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn wait_until_domcontentloaded首轮即返且pending可见() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;

    let t0 = Instant::now();
    let open = exec(
        &tool,
        json!({"action": "open", "url": base, "mode": "hidden", "reuse": false,
               "wait_until": "domcontentloaded", "timeout_ms": 30000}),
    )
    .await;
    let elapsed = t0.elapsed();
    assert_eq!(open["code"], 0, "{open}");
    assert_eq!(
        open["data"]["wait_until"], "domcontentloaded",
        "wait_until 必须真实生效并回显: {open}"
    );
    assert!(
        elapsed < Duration::from_secs(25),
        "domcontentloaded 不得等满整条管线(3s 忙等页首轮即返;上界含并行余量),实际 {elapsed:?}"
    );
    // 首轮收口后注入的文本 script 未执行:标题应停在初值(补泵后才 mounted)
    assert_eq!(
        open["data"]["title"].as_str(),
        Some("初值"),
        "domcontentloaded 只跑首轮,注入脚本应留待补泵: {open}"
    );
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let _ = exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn 不传timeout_ms维持整管线旧行为() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;

    let t0 = Instant::now();
    let open = exec(
        &tool,
        json!({"action": "open", "url": base, "mode": "hidden", "reuse": false}),
    )
    .await;
    let elapsed = t0.elapsed();
    assert_eq!(open["code"], 0, "{open}");
    // 8s 忙等页:旧行为 = 管线跑完(脚本执行完 → mounted),不受 60s 默认值砍半
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let st = exec(
        &tool,
        json!({"action": "inspect", "info": "title", "page_id": page_id, "params": {}}),
    )
    .await;
    assert_eq!(
        st["data"]["title"].as_str(),
        Some("mounted"),
        "不传 timeout_ms 时管线应完整跑完(旧语义不回归),open 耗时 {elapsed:?}: {st}"
    );
    let _ = exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}
