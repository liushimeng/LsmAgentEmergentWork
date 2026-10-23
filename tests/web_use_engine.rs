//! LsmAgentBrowser × MCP_Web_LsmAgentBrowser_Use 端到端集成测试。
//!
//! 使用进程内 mock HTTP 服务(真实 TCP/HTTP,OS 分配端口),不依赖外网,
//! 不启动任何外部浏览器,不使用 CDP。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 启动 mock 站点,返回 `http://127.0.0.1:{port}`。
/// `with_headers=true` 时按路径返回缓存/CSP 等自定义响应头(阶段四第二步集成测试)。
async fn start_site() -> String {
    start_site_impl(false).await
}

async fn start_site_with_headers() -> String {
    start_site_impl(true).await
}

async fn start_site_impl(with_headers: bool) -> String {
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
                let (status, ctype, body, extra_headers) = if path == "/logo.png" {
                    ("200 OK", "image/png", png_bytes(), vec![])
                } else {
                    let (s, b, h) = route(path, with_headers);
                    (s, "text/html; charset=utf-8", b.into_bytes(), h)
                };
                let mut header_str = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n",
                    body.len()
                );
                for (k, v) in &extra_headers {
                    header_str.push_str(&format!("{k}: {v}\r\n"));
                }
                header_str.push_str("Connection: close\r\n\r\n");
                let mut resp = header_str.into_bytes();
                resp.extend_from_slice(&body);
                let _ = socket.write_all(&resp).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{}", addr)
}

/// 生成 4x4 纯红 PNG(图片子资源测试用)。
fn png_bytes() -> Vec<u8> {
    let mut img = image::RgbaImage::new(4, 4);
    for px in img.pixels_mut() {
        *px = image::Rgba([255, 0, 0, 255]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn route(path: &str, with_headers: bool) -> (&'static str, String, Vec<(String, String)>) {
    let _ = with_headers;
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
            vec![],
        )
    } else if path == "/image-page" {
        (
            "200 OK",
            "<html><head><title>图片页</title></head><body>\
             <img id=\"logo\" src=\"/logo.png\" width=\"40\" height=\"20\">\
             <p>图片下方文本</p></body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/next" {
        (
            "200 OK",
            "<html><head><title>第二页</title></head><body><h2>Next Page</h2></body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/visual-page" {
        // 阶段六富视觉页:渐变横幅 + 圆角卡片 + 模糊阴影 + 溢出裁剪。
        (
            "200 OK",
            "<!doctype html><html><head><title>富视觉页</title></head><body style='margin:0'>\
             <div id='banner' style='width:400px;height:60px;background:linear-gradient(to right, #ff0000, #0000ff)'></div>\
             <div id='card' style='width:120px;height:80px;background:#000000;border-radius:20px;margin-top:20px;box-shadow:6px 6px 14px #333333'></div>\
             <div id='clip' style='width:60px;height:30px;overflow:hidden;background:#ffffff;margin-top:20px'>\
             <div style='width:120px;height:60px;background:#00ff00'></div></div>\
             </body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/js-inline" {
        (
            "200 OK",
            "<html><head><title>JS内联页</title></head><body><div id=\"app\">加载中</div>\
             <script>document.getElementById('app').innerHTML = '<h1>内联渲染完成</h1><p>内容由脚本注入</p>';</script>\
             </body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/app.js" {
        (
            "200 OK",
            "document.getElementById('app').textContent = '外链脚本已执行';".to_string(),
            vec![],
        )
    } else if path == "/js-geometry" {
        (
            "200 OK",
            "<html><head><title>JS几何页</title><style>\
             #box{width:300px;height:200px;}\
             #right{position:absolute;left:120px;top:40px;width:50px;height:30px;}\
             </style></head><body>\
             <div id=\"box\"></div><div id=\"right\"></div>\
             <script>var b = document.getElementById('box');\
             b.setAttribute('data-w', b.clientWidth); b.setAttribute('data-h', b.clientHeight);\
             var c = document.getElementById('right');\
             var r = c.getBoundingClientRect();\
             c.setAttribute('data-left', r.left); c.setAttribute('data-top', r.top);</script>\
             </body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/js-external" {
        (
            "200 OK",
            "<html><head><title>JS外链页</title><script src=\"/app.js\"></script></head>\
             <body><div id=\"app\">占位</div></body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/api/data" {
        (
            "200 OK",
            "{\"items\": [\"甲\", \"乙\", \"丙\"]}".to_string(),
            vec![],
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
            vec![],
        )
    } else if path == "/wait-page" {
        (
            "200 OK",
            "<html><head><title>等待页</title></head><body>\
             <div id=\"result\">合计 42</div>\
             <div id=\"hidden\" style=\"display:none\">藏起来了</div>\
             <div id=\"late\">未就绪</div>\
             <script>window.__ready = 1;\
             setTimeout(function(){ document.getElementById('late').textContent = '延迟就绪'; }, 1500);</script>\
             </body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/js-timer" {
        (
            "200 OK",
            "<html><head><title>JS定时页</title></head><body><div id=\"later\">未就绪</div>\
             <script>setTimeout(function(){ document.getElementById('later').textContent = '延迟渲染完成'; }, 2000);</script>\
             </body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/css-page" {
        (
            "200 OK",
            "<!doctype html><html><head><title>样式页</title>\
             <link rel=\"stylesheet\" href=\"/site.css\"></head><body>\
             <p id=\"styled\">样式文本</p>\
             <div id=\"hidden-by-css\">外链样式隐藏</div>\
             </body></html>"
                .to_string(),
            vec![],
        )
    } else if path == "/site.css" {
        (
            "200 OK",
            "#hidden-by-css{display:none} #styled{color:rgb(1, 2, 3);font-size:30px} body{font-size:20px}"
                .to_string(),
            vec![],
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
            vec![],
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
            vec![],
        )
    } else if path == "/cached-page" {
        // 阶段四第二步：强缓存测试页（Cache-Control: max-age=60）。
        (
            "200 OK",
            "<html><head><title>缓存页</title></head><body><h1>CacheMe</h1></body></html>".to_string(),
            vec![("Cache-Control".to_string(), "max-age=60".to_string())],
        )
    } else if path == "/csp-inline" {
        // 阶段四第二步：CSP 内联脚本拦截测试（script-src 'self'，无 unsafe-inline）。
        (
            "200 OK",
            "<html><head><title>CSP页</title></head><body><div id=\"app\">原始</div>\
             <script>document.getElementById('app').textContent = '内联脚本已执行';</script>\
             </body></html>"
                .to_string(),
            vec![(
                "Content-Security-Policy".to_string(),
                "script-src 'self'".to_string(),
            )],
        )
    } else if path == "/csp-allow" {
        // 阶段四第二步：CSP 放行测试（script-src 'self' 'unsafe-inline'）。
        (
            "200 OK",
            "<html><head><title>CSP放行页</title></head><body><div id=\"app\">原始</div>\
             <script>document.getElementById('app').textContent = '内联脚本已执行';</script>\
             </body></html>"
                .to_string(),
            vec![(
                "Content-Security-Policy".to_string(),
                "script-src 'self' 'unsafe-inline'".to_string(),
            )],
        )
    } else {
        (
            "404 Not Found",
            "<html><head><title>404</title></head><body>not found</body></html>".to_string(),
            vec![],
        )
    }
}

async fn exec(tool: &McpWebLsmBrowserUseTool, args: Value) -> Value {
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
    let tool = McpWebLsmBrowserUseTool;

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
    let tool = McpWebLsmBrowserUseTool;
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
    let tool = McpWebLsmBrowserUseTool;
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
    let tool = McpWebLsmBrowserUseTool;
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
    let tool = McpWebLsmBrowserUseTool;
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
    let tool = McpWebLsmBrowserUseTool;
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
    let tool = McpWebLsmBrowserUseTool;
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
    let tool = McpWebLsmBrowserUseTool;
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
    let tool = McpWebLsmBrowserUseTool;
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

// ============ 阶段二第二步:外链样式与计算样式 ============

#[tokio::test]
async fn 外链样式加载与计算样式可观测() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/css-page"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 样式表清单:外链已内联为 data-lsm-href。
    let sheets = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "stylesheets"}),
    )
    .await;
    assert_eq!(sheets["code"], 0, "{sheets}");
    assert_eq!(sheets["data"]["count"], 1, "应加载 1 个外链样式表:{sheets}");
    assert!(sheets["data"]["rules"].as_u64().unwrap_or(0) >= 2, "{sheets}");
    assert!(
        sheets["data"]["sheets"][0]["href"]
            .as_str()
            .unwrap_or("")
            .ends_with("/site.css"),
        "{sheets}"
    );

    // 外链 CSS 的 display:none 参与计算样式。
    let hidden = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#hidden-by-css", "include_style": true}}),
    )
    .await;
    assert_eq!(
        hidden["data"]["computed_style"]["display"], "none",
        "外链样式应影响计算样式:{hidden}"
    );
    assert_eq!(
        hidden["data"]["visible"], false,
        "外链样式隐藏的元素可见性应为 false:{hidden}"
    );

    // 字号/颜色计算值。
    let styled = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#styled", "include_style": true}}),
    )
    .await;
    assert_eq!(styled["data"]["computed_style"]["font-size"], "30px", "{styled}");
    assert_eq!(
        styled["data"]["computed_style"]["color"], "rgb(1, 2, 3)",
        "{styled}"
    );

    // JS getComputedStyle 读取同一份计算样式。
    let computed = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "eval_js",
                "params": {"expression":
                    "getComputedStyle(document.getElementById('styled')).getPropertyValue('font-size')"}}),
    )
    .await;
    assert_eq!(computed["data"]["result"], "30px", "{computed}");

    // 外链样式请求进入网络台账。
    let net = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "network"}),
    )
    .await;
    let events = net["data"]["events"].as_array().cloned().unwrap_or_default();
    assert!(
        events
            .iter()
            .any(|e| e["url"].as_str().unwrap_or("").ends_with("/site.css")),
        "样式表请求应记录在台账:{net}"
    );

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn js渲染_元素几何属性可读_定宽容器() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/js-geometry"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 脚本在页面加载期读 clientWidth/clientHeight 写入 dataset:
    // JS 桥必须拿到预布局矩形(而非 undefined/0),widget 层(如 BMap 容器)才可初始化。
    let probe = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "eval_js",
                "params": {"expression": "JSON.stringify({w:document.getElementById('box').getAttribute('data-w'), h:document.getElementById('box').getAttribute('data-h'), left:document.getElementById('right').getAttribute('data-left'), top:document.getElementById('right').getAttribute('data-top'), cw:document.getElementById('box').clientWidth})"}}),
    )
    .await;
    let result = probe["data"]["result"].as_str().unwrap_or("");
    let v: Value = serde_json::from_str(result).unwrap_or(json!({}));
    assert_eq!(v["w"], "300", "clientWidth 应为布局宽度 300:{probe}");
    assert_eq!(v["h"], "200", "clientHeight 应为布局高度 200:{probe}");
    assert_eq!(v["cw"], 300, "交互期 eval_js 读 clientWidth 也应命中矩形:{probe}");
    assert_eq!(v["left"], "120", "getBoundingClientRect().left 应为 120:{probe}");
    assert_eq!(v["top"], "40", "getBoundingClientRect().top 应为 40:{probe}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn eval_js_全局跨调用保留() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
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

// ============ 阶段三第三步:图片子资源 ============

#[tokio::test]
async fn 图片子资源加载_解码_参与布局与渲染() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/image-page"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 图片已解码:固有尺寸可见。
    let images = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "image_urls"}),
    )
    .await;
    assert_eq!(images["code"], 0, "{images}");
    assert_eq!(images["data"]["decoded_count"], 1, "应解码 1 张图片:{images}");
    let first = &images["data"]["images"][0];
    assert_eq!(first["loaded"], true, "{images}");
    assert_eq!(first["natural_width"], 4, "{images}");
    assert_eq!(first["natural_height"], 4, "{images}");

    // 图片请求进入网络台账。
    let net = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "network"}),
    )
    .await;
    let events = net["data"]["events"].as_array().cloned().unwrap_or_default();
    assert!(
        events
            .iter()
            .any(|e| e["url"].as_str().unwrap_or("").ends_with("/logo.png")),
        "图片请求应记录在台账:{net}"
    );

    // 图片按 HTML 属性尺寸(40x20)排版,坐标可用于点击。
    let el = exec(
        &tool,
        json!({"action": "inspect", "page_id": page_id, "info": "elements",
                "params": {"selector": "#logo"}}),
    )
    .await;
    assert_eq!(el["code"], 0, "{el}");
    assert_eq!(el["data"]["rect"]["width"], 40.0, "{el}");
    assert_eq!(el["data"]["rect"]["height"], 20.0, "{el}");

    // 截图中图片区域为红色(真实像素验证渲染链路)。
    let shot = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "screenshot",
                "params": {"save_path": std::env::temp_dir().join("laew_image_test.png").to_string_lossy()}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    let path = shot["data"]["save_path"].as_str().unwrap().to_string();
    let bytes = std::fs::read(&path).expect("截图应落盘");
    let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
    let px = decoded.get_pixel(12, 12).0;
    assert_eq!(&px[..3], &[255, 0, 0], "图片区域应渲染为红色");
    let _ = std::fs::remove_file(&path);

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

// ============ 阶段四第二步：缓存/CSP/CORS 集成测试 ============

#[tokio::test]
async fn 强缓存_二次导航命中缓存() {
    allow_loopback();
    let base = start_site_with_headers().await;
    let tool = McpWebLsmBrowserUseTool;

    let open = exec(&tool, json!({"action": "open", "url": format!("{base}/cached-page"), "mode": "hidden", "reuse": false})).await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 第一次导航：网络请求，from_cache=false。
    let net1 = exec(&tool, json!({"action": "inspect", "page_id": page_id, "info": "network"})).await;
    let events1 = net1["data"]["events"].as_array().cloned().unwrap_or_default();
    let cached1 = events1.iter().find(|e| e["url"].as_str().unwrap_or("").ends_with("/cached-page"));
    assert!(cached1.is_some(), "首次导航应有主文档请求:{net1}");
    assert_eq!(cached1.unwrap()["from_cache"], false, "首次应走网络");

    // 第二次导航（同 URL）：命中强缓存，from_cache=true。
    let nav = exec(&tool, json!({"action": "control", "page_id": page_id, "control_action": "navigate", "params": {"url": format!("{base}/cached-page")}})).await;
    assert_eq!(nav["code"], 0, "{nav}");
    let net2 = exec(&tool, json!({"action": "inspect", "page_id": page_id, "info": "network"})).await;
    let events2 = net2["data"]["events"].as_array().cloned().unwrap_or_default();
    let hits = events2.iter().filter(|e| e["url"].as_str().unwrap_or("").ends_with("/cached-page") && e["from_cache"] == true).count();
    assert!(hits >= 1, "二次导航应命中缓存:{net2}");
    // 缓存统计可见。
    assert!(net2["data"]["cache_stats"].is_object(), "network 输出应含 cache_stats:{net2}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn csp_script_src_拦截内联脚本() {
    allow_loopback();
    let base = start_site_with_headers().await;
    let tool = McpWebLsmBrowserUseTool;

    // script-src 'self'：内联脚本被拒。
    let open = exec(&tool, json!({"action": "open", "url": format!("{base}/csp-inline"), "mode": "hidden", "reuse": false})).await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let el = exec(&tool, json!({"action": "inspect", "page_id": page_id, "info": "elements", "params": {"selector": "#app"}})).await;
    assert_eq!(el["data"]["text"], "原始", "内联脚本应被 CSP 拦截:{el}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;

    // script-src 'self' 'unsafe-inline'：内联脚本放行。
    let open2 = exec(&tool, json!({"action": "open", "url": format!("{base}/csp-allow"), "mode": "hidden", "reuse": false})).await;
    assert_eq!(open2["code"], 0, "{open2}");
    let page_id2 = open2["data"]["page_id"].as_str().unwrap().to_string();
    let el2 = exec(&tool, json!({"action": "inspect", "page_id": page_id2, "info": "elements", "params": {"selector": "#app"}})).await;
    assert_eq!(el2["data"]["text"], "内联脚本已执行", "unsafe-inline 应放行:{el2}");

    exec(&tool, json!({"action": "close", "page_id": page_id2})).await;
}

/// 启动带 CORS 头的 API mock 服务（独立端口 = 跨域）。
async fn start_cors_api(with_cors: bool) -> String {
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
                let body = b"{\"ok\":true}";
                let mut headers = String::new();
                if with_cors {
                    headers.push_str("Access-Control-Allow-Origin: *\r\n");
                    headers.push_str("Access-Control-Allow-Methods: GET, POST\r\n");
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let mut out = resp.into_bytes();
                out.extend_from_slice(body);
                let _ = socket.write_all(&out).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    format!("http://{}", addr)
}

#[tokio::test]
async fn cors_跨域fetch_无头拒绝_有头放行() {
    allow_loopback();
    let base = start_site_with_headers().await;
    let tool = McpWebLsmBrowserUseTool;

    let api_no_cors = start_cors_api(false).await;
    let api_cors = start_cors_api(true).await;
    let open = exec(&tool, json!({"action": "open", "url": format!("{base}/"), "mode": "hidden", "reuse": false})).await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 无 CORS 头的跨域 fetch → reject（两步：先触发写 window.__r，再读取）。
    let t1 = exec(&tool, json!({"action": "control", "page_id": page_id, "control_action": "eval_js", "params": {"expression": format!("window.__r=null; fetch('{api_no_cors}').then(function(r){{return r.text();}}).then(function(t){{window.__r='ok:'+t;}}).catch(function(){{window.__r='reject';}}); 'triggered'")}})).await;
    assert_eq!(t1["code"], 0, "{t1}");
    let r1 = exec(&tool, json!({"action": "control", "page_id": page_id, "control_action": "eval_js", "params": {"expression": "window.__r || 'pending'"}})).await;
    assert_eq!(r1["code"], 0, "{r1}");
    assert!(
        r1["data"]["result"].as_str().unwrap_or("").starts_with("reject"),
        "无 CORS 头应 reject:{r1}"
    );

    // 有 CORS 头的跨域 fetch → 放行。
    let t2 = exec(&tool, json!({"action": "control", "page_id": page_id, "control_action": "eval_js", "params": {"expression": format!("window.__r=null; fetch('{api_cors}').then(function(r){{return r.text();}}).then(function(t){{window.__r='ok:'+t;}}).catch(function(){{window.__r='reject';}}); 'triggered'")}})).await;
    assert_eq!(t2["code"], 0, "{t2}");
    let r2 = exec(&tool, json!({"action": "control", "page_id": page_id, "control_action": "eval_js", "params": {"expression": "window.__r || 'pending'"}})).await;
    assert_eq!(r2["code"], 0, "{r2}");
    assert!(
        r2["data"]["result"].as_str().unwrap_or("").starts_with("ok:"),
        "有 CORS 头应放行:{r2}"
    );

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

// ============ 阶段五第一步:就绪判定原语 ============

#[tokio::test]
async fn wait_元素状态与文本条件() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/wait-page"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 元素可见 + 文本命中 → 立即返回。
    let ok = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"selector": "#result", "state": "visible", "text": "合计", "timeout_ms": 3000}}),
    )
    .await;
    assert_eq!(ok["code"], 0, "可见且文本命中应返回:{ok}");

    // hidden:元素存在但不可见。
    let hid = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"selector": "#hidden", "state": "hidden", "timeout_ms": 3000}}),
    )
    .await;
    assert_eq!(hid["code"], 0, "隐藏元素应判定为 hidden:{hid}");

    // 修复前 state=hidden 对「不存在的元素」恒为真(元素还没出现就判已隐藏),
    // 现在必须超时失败。
    let miss = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"selector": "#nope", "state": "hidden", "timeout_ms": 800}}),
    )
    .await;
    assert_eq!(miss["code"], 2002, "不存在的元素不应被判为 hidden:{miss}");

    // detached:元素不存在 → 立即成立。
    let det = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"selector": "#nope", "state": "detached", "timeout_ms": 800}}),
    )
    .await;
    assert_eq!(det["code"], 0, "detached 应立即成立:{det}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn wait_js条件与网络静默() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/wait-page"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    // 页面脚本置位的全局量:JS 条件成立。
    let js = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"js": "window.__ready === 1", "timeout_ms": 3000}}),
    )
    .await;
    assert_eq!(js["code"], 0, "JS 条件成立应返回:{js}");

    // JS 条件不成立 → 超时失败。
    let no = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"js": "window.__never === 1", "timeout_ms": 800}}),
    )
    .await;
    assert_eq!(no["code"], 2002, "JS 条件不成立应超时:{no}");

    // 网络静默:台账无新请求 → 立即返回。
    let idle = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"network_idle": 200, "timeout_ms": 3000}}),
    )
    .await;
    assert_eq!(idle["code"], 0, "网络静默应返回:{idle}");

    // 组合条件:元素可见 且 JS 条件成立。
    let both = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "wait",
                "params": {"selector": "#late", "state": "visible", "js": "window.__ready === 1",
                            "network_idle": 200, "timeout_ms": 3000}}),
    )
    .await;
    assert_eq!(both["code"], 0, "组合条件应返回:{both}");

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}

#[tokio::test]
async fn 富视觉页面_渐变圆角阴影与溢出裁剪截图() {
    // 阶段六端到端:显示列表渲染管线(渐变/圆角/模糊阴影/overflow 裁剪)
    // 在真实工具链路(open → 截图落盘)下的像素级验证。
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;

    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/visual-page"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    let dir = std::env::temp_dir().join(format!("lsm_visual_{}", std::process::id()));
    let path = dir.join("visual.png");
    let shot = exec(
        &tool,
        json!({"action": "control", "page_id": page_id, "control_action": "screenshot",
                "params": {"save_path": path.to_string_lossy(), "full_page": true}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    let bytes = std::fs::read(&path).expect("截图文件应存在");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "截图应为 PNG");

    let img = image::load_from_memory(&bytes).unwrap().to_rgba8();
    let px = |x: u32, y: u32| *img.get_pixel(x, y);
    // 渐变横幅(body 默认 8px 边距,banner (8,8)-(408,68)):
    // 左端红、右端蓝、中点混合。
    let left = px(12, 30);
    assert!(left[0] > 200 && left[2] < 60, "横幅左端应红:{left:?}");
    let right = px(403, 30);
    assert!(right[2] > 200 && right[0] < 60, "横幅右端应蓝:{right:?}");
    let mid = px(207, 30);
    assert!(
        (90..170).contains(&mid[0]) && (90..170).contains(&mid[2]),
        "横幅中点应约紫:{mid:?}"
    );
    // 圆角卡片(y∈[88,168),x∈[8,128),r=20):角外白、中心黑。
    let corner = px(12, 92);
    assert!(corner[0] > 200, "卡片左上角外应白:{corner:?}");
    let center = px(68, 128);
    assert!(center[0] < 50, "卡片中心应黑:{center:?}");
    // 溢出裁剪(y∈[188,218),x∈[8,68)):容器内绿、越界白。
    let inside = px(30, 200);
    assert!(inside[1] > 200 && inside[0] < 60, "裁剪容器内应绿:{inside:?}");
    let beyond = px(100, 200);
    assert!(beyond[0] > 200 && beyond[1] > 200, "水平越界应被裁白:{beyond:?}");
    // 模糊阴影:卡片右缘外侧(卡片 x∈[8,128))应出现半透明灰(20..235)。
    let shadow_px = px(135, 120);
    assert!(
        shadow_px[0] > 20 && shadow_px[0] < 235,
        "模糊阴影区应半透明:{shadow_px:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);

    exec(&tool, json!({"action": "close", "page_id": page_id})).await;
}
