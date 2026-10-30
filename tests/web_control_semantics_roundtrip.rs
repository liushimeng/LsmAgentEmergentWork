//! 第 42 轮:控件交互语义端到端回归(本地 mock,无外网)。
//!
//! 覆盖:页面脚本 `dialog.showModal()`、页面脚本 `focus()` 与 `:focus` 样式、
//! select 面板点击选中和 disabled/readonly 交互守卫。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn allow_loopback() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    std::env::set_var("LAEW_WEB_MAX_TABS", "32");
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
                let body = r##"<!doctype html><html><head><style>
                html,body{margin:0;background:#fff}
                #q{width:160px;height:36px}
                #q:focus{background:#00ff00}
                dialog{width:240px;height:120px}
                </style></head><body>
                <input id="q" value="">
                <input id="ro" readonly value="只读">
                <button id="dis" disabled>禁用</button>
                <select id="s"><option value="a">甲</option><option value="b">乙</option></select>
                <dialog id="d">模态内容</dialog>
                </body></html>"##;
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
    let text = tool.execute(args).await.expect("工具调用应成功");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON - {e}: {text}"))
}

async fn open_page(tool: &McpWebLsmBrowserUseTool, base: &str) -> String {
    let open = exec(
        tool,
        json!({"action":"open","url":base,"mode":"hidden","reuse":false,"viewport":{"width":1280,"height":720}}),
    )
    .await;
    assert_eq!(open["code"], 0, "打开 mock 页面失败:{open}");
    open["data"]["page_id"]
        .as_str()
        .expect("open 应返回 page_id")
        .to_string()
}

async fn element(tool: &McpWebLsmBrowserUseTool, page_id: &str, selector: &str) -> Value {
    let v = exec(
        tool,
        json!({"action":"inspect","page_id":page_id,"info":"elements","params":{"selector":selector}}),
    )
    .await;
    assert_eq!(v["code"], 0, "inspect {selector} 失败:{v}");
    v["data"].clone()
}

#[tokio::test]
async fn 页面脚本showModal打开模态对话框() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let page_id = open_page(&tool, &base).await;
    let shown = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"eval_js","params":{"expression":"document.querySelector('#d').showModal(); 'shown'"}}),
    )
    .await;
    assert_eq!(shown["code"], 0, "showModal 脚本应可调用:{shown}");
    assert_eq!(shown["data"]["result"], "shown");

    let dialog = element(&tool, &page_id, "#d").await;
    assert_eq!(dialog["state"]["open"], true, "showModal 后应写入 open:{dialog}");
    assert_eq!(
        dialog["attributes"]["data-lsm-modal"], "",
        "showModal 后应写入模态标记:{dialog}"
    );

    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}

#[tokio::test]
async fn 页面脚本focus同步伪类状态与像素() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let page_id = open_page(&tool, &base).await;
    let focused = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"eval_js","params":{"expression":"document.querySelector('#q').focus(); 'focused'"}}),
    )
    .await;
    assert_eq!(focused["code"], 0, "页面脚本 focus 应执行:{focused}");
    let input = element(&tool, &page_id, "#q").await;
    assert_eq!(input["state"]["focused"], true, "focus 必须进入状态字段:{input}");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("focus.png");
    let shot = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"screenshot","params":{"save_path":path.to_string_lossy(),"full_page":false}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    let bytes = std::fs::read(&path).expect("截图应落盘");
    let img = image::load_from_memory(&bytes)
        .expect("截图应可解码")
        .to_rgba8();
    let rect = input["rect"].clone();
    let (x, y, w, h) = (
        rect["x"].as_f64().unwrap(),
        rect["y"].as_f64().unwrap(),
        rect["width"].as_f64().unwrap(),
        rect["height"].as_f64().unwrap(),
    );
    let mut green = 0usize;
    for py in y.max(0.0) as u32..((y + h) as u32).min(img.height()) {
        for px in x.max(0.0) as u32..((x + w) as u32).min(img.width()) {
            let p = img.get_pixel(px, py).0;
            if p[1] > 200 && p[0] < 80 && p[2] < 80 {
                green += 1;
            }
        }
    }
    assert!(green > 100, "脚本 focus 后 :focus 样式必须进像素,绿色像素数={green}");

    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}

#[tokio::test]
async fn select浮层点击选项端到端收起并选中() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let page_id = open_page(&tool, &base).await;
    let opened = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"click","params":{"selector":"#s"}}),
    )
    .await;
    assert_eq!(opened["code"], 0, "{opened}");
    assert_eq!(opened["data"]["dropdown_open"], true, "{opened}");

    let option = element(&tool, &page_id, "#s option:nth-of-type(2)").await;
    assert!(
        option["rect"]["width"].as_f64().unwrap() > 0.0,
        "展开面板中的 option 应有可寻址几何:{option}"
    );
    let picked = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"click","params":{"selector":"#s option:nth-of-type(2)"}}),
    )
    .await;
    assert_eq!(picked["code"], 0, "{picked}");
    assert_eq!(picked["data"]["selected"], true, "{picked}");
    assert_eq!(picked["data"]["value"], "b");
    assert_eq!(picked["data"]["dropdown_closed"], true);

    let select = element(&tool, &page_id, "#s").await;
    assert_eq!(select["value"], "b", "点击 option 后运行时值应同步:{select}");
    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}

#[tokio::test]
async fn 禁用与只读控件在工具层被拒绝() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let page_id = open_page(&tool, &base).await;

    let clicked = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"click","params":{"selector":"#dis"}}),
    )
    .await;
    assert_eq!(clicked["code"], 0, "禁用点击是结构化结果而非异常:{clicked}");
    assert_eq!(clicked["data"]["clicked"], false, "{clicked}");
    assert_eq!(clicked["data"]["disabled"], true, "{clicked}");

    let typed = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"input_text","params":{"selector":"#ro","text":"写入"}}),
    )
    .await;
    assert_eq!(typed["code"], 2002, "只读输入必须返回动作失败:{typed}");
    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}
