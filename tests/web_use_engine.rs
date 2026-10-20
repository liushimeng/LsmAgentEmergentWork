//! LsmAgentBrowser × MCP_Web_Use 端到端集成测试。
//!
//! 使用进程内 mock HTTP 服务(真实 TCP/HTTP,OS 分配端口),不依赖外网,
//! 不启动任何外部浏览器,不使用 CDP。

use lsm_agent::agent::tools::mcp_web_use::McpWebUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 启动 mock 站点,返回 `http://127.0.0.1:{port}`。
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
                let head = String::from_utf8_lossy(&buf).to_string();
                let first = head.lines().next().unwrap_or("");
                let path = first.split_whitespace().nth(1).unwrap_or("/");
                let (status, body) = route(path);
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{}", addr)
}

fn route(path: &str) -> (&'static str, String) {
    if path == "/" {
        (
            "200 OK",
            "<!doctype html><html><head><title>首页</title></head><body>\
             <h1>测试站点</h1>\
             <a id=\"next\" href=\"/next\">下一页</a>\
             <form action=\"/search\" method=\"get\">\
             <input id=\"q\" name=\"q\" value=\"\">\
             <select name=\"lang\"><option value=\"zh\">中文</option><option value=\"en\" selected>English</option></select>\
             <button id=\"go\" type=\"submit\">搜索</button>\
             </form></body></html>"
                .to_string(),
        )
    } else if path == "/next" {
        (
            "200 OK",
            "<html><head><title>第二页</title></head><body><h2>Next Page</h2></body></html>"
                .to_string(),
        )
    } else if path == "/js-inline" {
        (
            "200 OK",
            "<html><head><title>JS内联页</title></head><body><div id=\"app\">加载中</div>\
             <script>document.getElementById('app').innerHTML = '<h1>内联渲染完成</h1><p>内容由脚本注入</p>';</script>\
             </body></html>"
                .to_string(),
        )
    } else if path == "/app.js" {
        (
            "200 OK",
            "document.getElementById('app').textContent = '外链脚本已执行';".to_string(),
        )
    } else if path == "/js-external" {
        (
            "200 OK",
            "<html><head><title>JS外链页</title><script src=\"/app.js\"></script></head>\
             <body><div id=\"app\">占位</div></body></html>"
                .to_string(),
        )
    } else if path == "/api/data" {
        (
            "200 OK",
            "{\"items\": [\"甲\", \"乙\", \"丙\"]}".to_string(),
        )
    } else if path == "/js-fetch" {
        (
            "200 OK",
            "<html><head><title>JS取数页</title></head><body><ul id=\"list\"></ul>\
             <script>fetch('/api/data').then(function(r){ return r.json(); }).then(function(d){\
               document.getElementById('list').innerHTML = d.items.map(function(x){ return '<li>条目' + x + '</li>'; }).join('');\
             });</script>\
             </body></html>"
                .to_string(),
        )
    } else if path == "/js-timer" {
        (
            "200 OK",
            "<html><head><title>JS定时页</title></head><body><div id=\"later\">未就绪</div>\
             <script>setTimeout(function(){ document.getElementById('later').textContent = '延迟渲染完成'; }, 2000);</script>\
             </body></html>"
                .to_string(),
        )
    } else if path == "/js-click" {
        (
            "200 OK",
            "<html><head><title>JS交互页</title></head><body><div id=\"panel\" style=\"display:none\">面板</div>\
             <a id=\"btn\" href=\"/next\">切换</a>\
             <script>document.getElementById('btn').addEventListener('click', function(e){\
               e.preventDefault();\
               document.getElementById('panel').style.display = 'block';\
             });</script>\
             </body></html>"
                .to_string(),
        )
    } else if path.starts_with("/search") {
        let q = path
            .split_once('?')
            .map(|(_, q)| q)
            .unwrap_or("")
            .split('&')
            .find_map(|kv| kv.strip_prefix("q="))
            .unwrap_or("")
            .replace('+', " ");
        (
            "200 OK",
            format!(
                "<html><head><title>结果</title></head><body><h1>搜索结果</h1>\
                 <p id=\"query\">q={q}</p></body></html>"
            ),
        )
    } else {
        (
            "404 Not Found",
            "<html><head><title>404</title></head><body>not found</body></html>".to_string(),
        )
    }
}

async fn exec(tool: &McpWebUseTool, args: Value) -> Value {
    let text = tool.execute(args).await.unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON:{e}\n{text}"))
}

fn allow_loopback() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    // 测试并行共享全局浏览器单例,放开标签页上限避免竞争。
    std::env::set_var("LAEW_WEB_MAX_TABS", "32");
}

#[tokio::test]
async fn open_读取_点击链接_表单提交闭环() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;

    let open = exec(&tool, json!({"action": "open", "url": base, "mode": "hidden", "reuse": false})).await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    assert_eq!(open["data"]["title"], "首页", "标题应来自 <title>");

    let title = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "title"}),
    )
    .await;
    assert_eq!(title["data"]["title"], "首页");

    let h1 = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "h1"}}),
    )
    .await;
    assert!(h1["data"]["text"].as_str().unwrap().contains("测试站点"));

    // 点击「下一页」
    let clicked = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "click",
                "params": {"selector": "#next"}}),
    )
    .await;
    assert_eq!(clicked["code"], 0, "{clicked}");
    assert!(clicked["data"]["final_url"].as_str().unwrap().ends_with("/next"));
    let title = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "title"}),
    )
    .await;
    assert_eq!(title["data"]["title"], "第二页");

    // 回到首页,输入并回车提交
    let nav = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "navigate",
                "params": {"url": base}}),
    )
    .await;
    assert_eq!(nav["code"], 0, "{nav}");
    let typed = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "input_text",
                "params": {"selector": "#q", "text": "rust"}}),
    )
    .await;
    assert_eq!(typed["code"], 0, "{typed}");
    assert_eq!(typed["data"]["value_after"]["len"], 4);

    let submitted = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "key_press",
                "params": {"key": "Enter", "selector": "#q"}}),
    )
    .await;
    assert_eq!(submitted["code"], 0, "{submitted}");
    assert_eq!(submitted["data"]["submitted"], true, "回车应提交表单");
    let body = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#query"}}),
    )
    .await;
    assert!(
        body["data"]["text"].as_str().unwrap().contains("q=rust"),
        "GET 表单应带上输入值:{body}"
    );

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn sequence_连续执行与_stop_on_error() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(&tool, json!({"action": "open", "url": base, "mode": "hidden", "reuse": false})).await;
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    let seq = exec(
        &tool,
        json!({
            "action": "sequence",
            "page_id": page_id,
            "steps": [
                {"control_action": "input_text", "params": {"selector": "#q", "text": "laew"}},
                {"control_action": "click", "params": {"selector": "#go"}},
                {"info": "elements", "params": {"selector": "#query"}}
            ]
        }),
    )
    .await;
    assert_eq!(seq["code"], 0, "{seq}");
    assert_eq!(seq["data"]["completed"], true);
    assert_eq!(seq["data"]["step_count"], 3);
    let last_text = seq["data"]["steps"][2]["data"]["text"].as_str().unwrap();
    assert!(last_text.contains("q=laew"), "最后一步应读到搜索结果:{last_text}");

    // 失败步:stop_on_error=true 时后续步骤不执行
    let seq = exec(
        &tool,
        json!({
            "action": "sequence",
            "page_id": page_id,
            "stop_on_error": true,
            "steps": [
                {"control_action": "click", "params": {"selector": "#not-exist"}},
                {"info": "title"}
            ]
        }),
    )
    .await;
    assert_ne!(seq["code"], 0);
    assert_eq!(seq["data"]["step_count"], 1, "首步失败后不应继续");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn explore_批量观测与截图落盘() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(&tool, json!({"action": "open", "url": base, "mode": "hidden", "reuse": false})).await;
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    let exp = exec(
        &tool,
        json!({
            "action": "explore",
            "page_id": page_id,
            "queries": [
                {"info": "title"},
                {"info": "elements", "params": {"selector": "a#next"}},
                {"info": "network"}
            ],
            "summary_hint": "首页三连观察"
        }),
    )
    .await;
    assert_eq!(exp["code"], 0, "{exp}");
    assert_eq!(exp["data"]["ok_count"], 3, "{exp}");
    assert_eq!(exp["data"]["summary_hint"], "首页三连观察");

    let dir = std::env::temp_dir().join(format!("lsm_web_engine_{}", std::process::id()));
    let path = dir.join("shot.png");
    let shot = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "screenshot",
                "params": {"save_path": path.to_string_lossy(), "full_page": true}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    let bytes = std::fs::read(&path).expect("截图文件应存在");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "截图应为 PNG");
    let _ = std::fs::remove_dir_all(&dir);

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn 失效_page_id_返回_2000() {
    allow_loopback();
    let tool = McpWebUseTool;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": "p_deadbeef", "info": "title"}),
    )
    .await;
    assert_eq!(v["code"], 2000, "{v}");
}

// ============ 阶段二:页面脚本与动态 DOM ============

#[tokio::test]
async fn js渲染_内联脚本正文可读() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/js-inline"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let app = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#app"}}),
    )
    .await;
    let text = app["data"]["text"].as_str().unwrap_or("");
    assert!(text.contains("内联渲染完成"), "脚本注入的正文应可读:{app}");
    assert!(!text.contains("加载中"), "占位文本应被替换:{app}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn js渲染_外链脚本执行() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/js-external"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let app = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#app"}}),
    )
    .await;
    assert!(
        app["data"]["text"].as_str().unwrap_or("").contains("外链脚本已执行"),
        "外链脚本内容应生效:{app}"
    );

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn js渲染_fetch_取数后渲染() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/js-fetch"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let list = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#list"}}),
    )
    .await;
    let text = list["data"]["text"].as_str().unwrap_or("");
    assert!(text.contains("条目甲"), "fetch 数据应渲染进 DOM:{list}");
    assert!(text.contains("条目丙"), "fetch 数据应渲染进 DOM:{list}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn js渲染_定时器延迟后可读() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/js-timer"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let later = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#later"}}),
    )
    .await;
    assert!(
        later["data"]["text"].as_str().unwrap_or("").contains("延迟渲染完成"),
        "虚拟时钟应快进执行定时回调:{later}"
    );

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn js交互_click监听阻止默认导航() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/js-click"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    let clicked = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "click",
                "params": {"selector": "#btn"}}),
    )
    .await;
    assert_eq!(clicked["code"], 0, "{clicked}");
    assert_eq!(clicked["data"]["default_prevented"], true, "preventDefault 应生效:{clicked}");
    assert_eq!(clicked["data"]["js_handled"], true, "listener 应命中:{clicked}");

    // 页面未导航:标题仍是 JS交互页,面板已显示。
    let title = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "title"}),
    )
    .await;
    assert_eq!(title["data"]["title"], "JS交互页", "preventDefault 后不应跳转:{title}");
    let panel = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#panel"}}),
    )
    .await;
    assert_eq!(panel["data"]["visible"], true, "listener 修改 style 后面板应可见:{panel}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn eval_js_全局跨调用保留() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/js-inline"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    let first = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "eval_js",
                "params": {"expression": "window.__laew_counter = 41; 'set'"}}),
    )
    .await;
    assert_eq!(first["data"]["result"], "set", "{first}");
    let second = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "eval_js",
                "params": {"expression": "window.__laew_counter + 1"}}),
    )
    .await;
    assert_eq!(second["data"]["result"], "42", "全局变量应跨 eval_js 保留:{second}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}
