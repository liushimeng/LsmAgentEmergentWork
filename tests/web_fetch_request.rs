//! Fetch Request/Headers 与真实 HTTP 请求线的端到端契约测试。

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
                let content_length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                while data.len() < header_end + content_length {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                let request_line = head.lines().next().unwrap_or("");
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("GET");
                let path = parts.next().unwrap_or("/");
                let header = |name: &str| {
                    head.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case(name).then(|| value.trim().to_string())
                    })
                };
                let body = if path == "/echo" {
                    json!({
                        "method": method,
                        "token": header("x-token").unwrap_or_default(),
                        "override": header("x-override").unwrap_or_default(),
                        "body": String::from_utf8_lossy(
                            &data[header_end..data.len().min(header_end + content_length)]
                        ),
                    })
                    .to_string()
                } else {
                    "<html><body><pre id=\"result\">等待请求</pre><script>\
                     var h = new Headers([['X-Token', 'one']]);\
                     h.append('x-token', 'two');\
                     var req = new Request('/echo', {method:'post', headers:h, body:'request-body'});\
                     var cloned = req.clone();\
                     fetch(req).then(function(r){ return r.json(); }).then(function(first){\
                       return fetch(cloned, {method:'PUT', headers:{'X-Override':'yes'}, body:'override-body'})\
                         .then(function(r){ return r.json(); }).then(function(second){\
                           document.getElementById('result').textContent = JSON.stringify({\
                             requestMethod:req.method, token:h.get('X-TOKEN'), has:h.has('x-token'),\
                             signalType:typeof req.signal.addEventListener,\
                             first:first, second:second\
                           });\
                         });\
                     }).catch(function(e){\
                       document.getElementById('result').textContent = 'ERROR:' + String(e);\
                     });\
                     </script></body></html>"
                        .to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    if path == "/echo" { "application/json" } else { "text/html; charset=utf-8" },
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
    let text = tool.execute(args).await.unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON: {e}\n{text}"))
}

#[tokio::test]
async fn fetch支持request对象与init覆盖并发送真实请求线() {
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
    let result = exec(
        &tool,
        json!({"action":"inspect","info":"elements","page_id":page_id,
               "params":{"selector":"#result"}}),
    )
    .await;
    let text = result["data"]["text"].as_str().unwrap_or("");
    assert!(!text.contains("ERROR:"), "页面 Fetch 链不应报错: {result}");
    assert!(text.contains("\"requestMethod\":\"POST\""), "Request 方法应规范化: {text}");
    assert!(text.contains("\"token\":\"one, two\""), "Headers 应大小写不敏感并追加: {text}");
    assert!(text.contains("\"has\":true"), "Headers.has 应命中: {text}");
    assert!(text.contains("\"signalType\":\"function\""), "Request.signal 应提供事件订阅: {text}");
    let parsed: Value = serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("结果应是 JSON: {e}\n{text}"));
    assert_eq!(
        parsed["first"],
        json!({"method":"POST","token":"one, two","override":"","body":"request-body"}),
        "fetch(Request) 应发送 Request 的方法、头和 body"
    );
    assert_eq!(
        parsed["second"],
        json!({"method":"PUT","token":"","override":"yes","body":"override-body"}),
        "fetch init 应覆盖 Request 的方法、头和 body"
    );

    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}
