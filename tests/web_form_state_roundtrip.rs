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

/// 第 41 轮:交互态伪类与浮层展开的端到端回归(mock 站点,无外网)。
async fn start_interaction_site() -> String {
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
                #btn{width:120px;height:40px;background:#dddddd}
                #btn:hover{background:#ff0000}
                #btn:focus{background:#00ff00}
                select{width:160px;height:32px}
                </style></head><body>
                <button id="btn">按钮</button>
                <select id="s"><option value="a">AAAA</option><option value="b">BBBB</option></select>
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

/// 采样截图指定矩形区域,统计接近目标色的像素数。
fn count_color(img: &image::RgbaImage, rect: (f64, f64, f64, f64), target: [u8; 3]) -> usize {
    let (x, y, w, h) = rect;
    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + w).max(0.0) as u32).min(img.width());
    let y1 = ((y + h).max(0.0) as u32).min(img.height());
    let mut n = 0;
    for py in y0..y1 {
        for px in x0..x1 {
            let p = img.get_pixel(px, py).0;
            if (p[0] as i32 - target[0] as i32).abs() < 24
                && (p[1] as i32 - target[1] as i32).abs() < 24
                && (p[2] as i32 - target[2] as i32).abs() < 24
            {
                n += 1;
            }
        }
    }
    n
}

/// 截图并解码,返回图像与按钮矩形。
async fn shot_button(
    tool: &McpWebLsmBrowserUseTool,
    page_id: &str,
    dir: &tempfile::TempDir,
    name: &str,
) -> (image::RgbaImage, (f64, f64, f64, f64)) {
    let path = dir.path().join(name);
    let shot = exec(
        tool,
        json!({"action":"control","page_id":page_id,"control_action":"screenshot","params":{"save_path":path.to_string_lossy(),"full_page":false}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    let bytes = std::fs::read(&path).expect("截图应落盘");
    let img = image::load_from_memory(&bytes).expect("截图 PNG 应可解码").to_rgba8();
    let btn = element(tool, page_id, "#btn").await;
    let rect = (
        btn["rect"]["x"].as_f64().unwrap(),
        btn["rect"]["y"].as_f64().unwrap(),
        btn["rect"]["width"].as_f64().unwrap(),
        btn["rect"]["height"].as_f64().unwrap(),
    );
    (img, rect)
}

#[tokio::test]
async fn 交互态伪类与下拉浮层端到端一致() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    let base = start_interaction_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action":"open","url":base,"mode":"hidden","reuse":false,"viewport":{"width":1280,"height":720}}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();
    let dir = tempfile::tempdir().unwrap();

    // hover:置运行时伪类状态 → `:hover` 样式命中(背景转红)。
    let hovered = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"hover","params":{"selector":"#btn"}}),
    )
    .await;
    assert_eq!(hovered["code"], 0, "{hovered}");
    assert_eq!(hovered["data"]["hovered"], true, "{hovered}");
    let btn = element(&tool, &page_id, "#btn").await;
    assert_eq!(
        btn["attributes"]["data-lsm-hover"], "",
        "hover 状态必须落到运行时状态属性:{btn}"
    );
    assert_eq!(btn["state"]["hovered"], true, "hover 状态必须进入一等 state 字段:{btn}");
    let (img, rect) = shot_button(&tool, &page_id, &dir, "hover.png").await;
    let red = count_color(&img, rect, [255, 0, 0]);
    assert!(red > 100, ":hover 样式必须真实进像素,红色像素数={red}");

    // focus:焦点样式(绿)覆盖 hover(红)。
    let focused = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"focus","params":{"selector":"#btn"}}),
    )
    .await;
    assert_eq!(focused["code"], 0, "{focused}");
    let (img, rect) = shot_button(&tool, &page_id, &dir, "focus.png").await;
    let green = count_color(&img, rect, [0, 255, 0]);
    assert!(green > 100, ":focus 样式必须真实进像素,绿色像素数={green}");

    // blur:焦点态撤销,回到 hover 态。
    let blurred = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"blur","params":{"selector":"#btn"}}),
    )
    .await;
    assert_eq!(blurred["code"], 0, "{blurred}");
    let (img, rect) = shot_button(&tool, &page_id, &dir, "blur.png").await;
    let red = count_color(&img, rect, [255, 0, 0]);
    assert!(red > 100, "blur 后应回到 hover 态,红色像素数={red}");

    // 下拉浮层:展开后全部 option 进入绘制文本。
    let opened = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"open_dropdown","params":{"selector":"#s"}}),
    )
    .await;
    assert_eq!(opened["code"], 0, "{opened}");
    assert_eq!(opened["data"]["open"], true, "{opened}");
    let text = exec(
        &tool,
        json!({"action":"inspect","page_id":page_id,"info":"text"}),
    )
    .await;
    let body_text = text["data"]["text"].as_str().unwrap_or_default();
    assert!(
        body_text.contains("AAAA") && body_text.contains("BBBB"),
        "展开的 select 面板应列出全部 option:{body_text}"
    );

    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}
