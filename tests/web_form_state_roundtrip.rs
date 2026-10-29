//! 表单运行时状态、details 语义与基础表格几何的端到端回归(本地 mock,无外网)。

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
                html,body{margin:0;background:#fff}input,select{display:block;margin:8px}
                table{margin-top:12px}td{padding:6px 10px;border:1px solid #999}
                </style></head><body>
                <input id="q" value="旧值">
                <input id="ok" type="checkbox">
                <select id="s"><option value="a">AAAA</option><option value="b">BBBB</option></select>
                <details id="d"><summary>展开</summary><p id="body">正文内容</p></details>
                <table><tr><td id="c1">AA</td><td id="c2">BBBBBBBB</td></tr></table>
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
async fn 表单状态详情开合与表格几何端到端一致() {
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
    let _ = element(&tool, &page_id, "#q").await;
    let typed = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"input_text","params":{"selector":"#q","text":"新值"}}),
    )
    .await;
    assert_eq!(typed["code"], 0, "{typed}");
    let q = element(&tool, &page_id, "#q").await;
    assert_eq!(q["value"], "新值", "inspect 必须读取运行时输入值:{q}");
    let checked = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"click","params":{"selector":"#ok"}}),
    )
    .await;
    assert_eq!(checked["code"], 0, "{checked}");
    assert_eq!(checked["data"]["toggled"], true, "{checked}");
    let ok = element(&tool, &page_id, "#ok").await;
    assert_eq!(ok["value"], "true", "checkbox 运行时选中态应同步:{ok}");
    let selected = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"select_option","params":{"selector":"#s","value":"b"}}),
    )
    .await;
    assert_eq!(selected["code"], 0, "{selected}");
    let s = element(&tool, &page_id, "#s").await;
    assert_eq!(s["value"], "b", "select 运行时选中值应同步:{s}");
    let closed_body = element(&tool, &page_id, "#body").await;
    assert_eq!(closed_body["rect"]["width"], 0.0, "details 关闭时正文不应占位:{closed_body}");
    let toggled = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"click","params":{"selector":"summary"}}),
    )
    .await;
    assert_eq!(toggled["code"], 0, "{toggled}");
    assert_eq!(toggled["data"]["toggled_open"], true, "{toggled}");
    let opened_body = element(&tool, &page_id, "#body").await;
    assert!(opened_body["rect"]["height"].as_f64().unwrap() > 0.0, "details 展开后正文应有布局:{opened_body}");
    let c1 = element(&tool, &page_id, "#c1").await;
    let c2 = element(&tool, &page_id, "#c2").await;
    let (c1x, c1y, c1w) = (
        c1["rect"]["x"].as_f64().unwrap(),
        c1["rect"]["y"].as_f64().unwrap(),
        c1["rect"]["width"].as_f64().unwrap(),
    );
    let (c2x, c2y, c2w) = (
        c2["rect"]["x"].as_f64().unwrap(),
        c2["rect"]["y"].as_f64().unwrap(),
        c2["rect"]["width"].as_f64().unwrap(),
    );
    assert!((c1y - c2y).abs() < 0.5, "表格同行 y 应一致:{c1} {c2}");
    assert!(c2x > c1x, "表格右列 x 应递增:{c1} {c2}");
    assert!(c2w > c1w, "长内容列应更宽:{c1} {c2}");
    assert!(c1w > 0.0 && c2w > 0.0);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("form_state.png");
    let shot = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"screenshot","params":{"save_path":path.to_string_lossy(),"full_page":false}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    assert!(path.exists(), "状态闭环后截图应真实落盘:{shot}");

    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}
