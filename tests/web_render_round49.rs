//! 第 49 轮显示渲染补全的端到端回归(仅本地 mock,无外网)。
//!
//! 覆盖像素级验证:`object-position` 的 letterbox 留白、加载失败 `alt` 占位、
//! 表格 `border-spacing` 间隙、`caption-side: bottom`、`ol reversed`、
//! `fieldset[disabled]` 文字灰化。

use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const HTML: &str = r##"<!doctype html><html><head><style>
html,body{margin:0;background:#ffffff}
#objpos{width:120px;height:60px;object-fit:contain;object-position:0% 0%;border:1px solid #000000}
#alt{width:160px;height:40px;border:1px solid #000000}
#spaced{border-spacing:20px 10px;border-collapse:separate}
#spaced td{background:#0000ff;color:#ffffff;padding:4px}
.cap{border-collapse:collapse}
.cap td{border:1px solid #0000ff}
</style></head><body>
<img id="objpos" src="/red.png">
<img id="alt" src="/missing.png" alt="缺图占位">
<table id="spaced"><tr><td id="c1">A</td><td id="c2">B</td></tr><tr><td id="c3">C</td><td id="c4">D</td></tr></table>
<table class="cap" style="caption-side:bottom"><caption id="cap">标题</caption><tr><td id="k1">K</td></tr></table>
<ol id="rev" reversed><li>甲</li><li>乙</li><li>丙</li></ol>
<fieldset id="fs" disabled><input id="fi" value="禁用文本"></fieldset>
</body></html>"##;

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
                let req = String::from_utf8_lossy(&buf).to_string();
                let (ct, body): (&str, Vec<u8>) = if req.starts_with("GET /red.png") {
                    let mut png = Vec::new();
                    // 200×200 大于内容盒,`object-fit:contain` 不会放大。
                    let img = image::RgbaImage::from_pixel(200, 200, image::Rgba([255, 0, 0, 255]));
                    img.write_to(
                        &mut std::io::Cursor::new(&mut png),
                        image::ImageFormat::Png,
                    )
                    .unwrap();
                    ("image/png", png)
                } else {
                    ("text/html; charset=utf-8", HTML.as_bytes().to_vec())
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    ct,
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.write_all(&body).await;
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

async fn rect(tool: &McpWebLsmBrowserUseTool, page_id: &str, id: &str) -> (f64, f64, f64, f64) {
    let v = exec(
        tool,
        json!({"action":"control","page_id":page_id,"control_action":"eval_js","params":{"script":format!("(function(){{var r=document.getElementById('{id}').getBoundingClientRect();return JSON.stringify({{x:r.x,y:r.y,w:r.width,h:r.height}});}})()")}}),
    )
    .await;
    let s = v["data"]["result"].as_str().unwrap_or("{}");
    let o: Value = serde_json::from_str(s).unwrap_or_else(|_| panic!("几何非法:{v}"));
    (
        o["x"].as_f64().unwrap(),
        o["y"].as_f64().unwrap(),
        o["w"].as_f64().unwrap(),
        o["h"].as_f64().unwrap(),
    )
}

fn count_color(
    img: &image::RgbaImage,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    target: [u8; 3],
) -> usize {
    let mut n = 0;
    for py in y0.max(0)..y1.min(img.height() as i64) {
        for px in x0.max(0)..x1.min(img.width() as i64) {
            let p = img.get_pixel(px as u32, py as u32).0;
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

#[tokio::test]
async fn 显示渲染补全像素级端到端一致() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action":"open","url":base,"mode":"hidden","reuse":false,"viewport":{"width":1920,"height":1080}}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("round49.png");
    let shot = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"screenshot","params":{"save_path":path.to_string_lossy(),"full_page":false}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    let bytes = std::fs::read(&path).unwrap();
    let img = image::load_from_memory(&bytes).unwrap().to_rgba8();

    // 1) object-position:0% 0% → 200×200 图在 120×60 盒内 contain 成 60×60,
    //    贴左上角;letterbox 留白区不得被边缘像素拉伸。
    let (ox, oy, ow, oh) = rect(&tool, &page_id, "objpos").await;
    assert!((ow - 120.0).abs() < 2.0 && (oh - 60.0).abs() < 2.0, "objpos 几何:{ox},{oy},{ow},{oh}");
    let red_tl = count_color(&img, (ox + 4.0) as i64, (oy + 4.0) as i64, (ox + 14.0) as i64, (oy + 14.0) as i64, [255, 0, 0]);
    assert!(red_tl > 40, "object-position:0% 0% 应把图片贴到左上角,红像素={red_tl}");
    let red_right = count_color(&img, (ox + 100.0) as i64, (oy + 4.0) as i64, (ox + 110.0) as i64, (oy + 14.0) as i64, [255, 0, 0]);
    assert_eq!(red_right, 0, "object-position:0% 0% 时右侧留白不应有图,红像素={red_right}");

    // 2) alt 占位:未加载图片内容盒内应有灰色文本像素。
    let (ax, ay, aw, ah) = rect(&tool, &page_id, "alt").await;
    let gray = count_color(&img, (ax + 2.0) as i64, (ay + 2.0) as i64, (ax + aw - 2.0) as i64, (ay + ah - 2.0) as i64, [117, 117, 117]);
    assert!(gray > 10, "未加载图片应画 alt 灰色占位文本,灰像素={gray}");

    // 3) border-spacing:单元格之间有 20px 水平间隙(背景白)。
    let (c1x, c1y, c1w, c1h) = rect(&tool, &page_id, "c1").await;
    let (c2x, _c2y, _c2w, _c2h) = rect(&tool, &page_id, "c2").await;
    let gap = c2x - (c1x + c1w);
    assert!((gap - 20.0).abs() < 1.0, "border-spacing 水平间隙应为 20px,实际={gap}");
    let white_gap = count_color(&img, (c1x + c1w + 2.0) as i64, (c1y + 2.0) as i64, (c2x - 2.0) as i64, (c1y + c1h - 2.0) as i64, [255, 255, 255]);
    assert!(white_gap > 20, "单元格间隙应为背景白,白像素={white_gap}");

    // 4) caption-side:bottom → caption 在单元格之下。
    let (_capx, capy, _capw, _caph) = rect(&tool, &page_id, "cap").await;
    let (_k1x, k1y, _k1w, _k1h) = rect(&tool, &page_id, "k1").await;
    assert!(capy > k1y, "caption-side:bottom 时标题应在表格之下:cap={capy} cell={k1y}");

    // 5) ol reversed → 文本含倒序序号。
    let text = exec(&tool, json!({"action":"inspect","page_id":page_id,"info":"text"})).await;
    let body = text["data"]["text"].as_str().unwrap_or_default();
    assert!(body.contains('3') && body.contains('2') && body.contains('1'), "ol reversed 文本:{body}");

    // 6) fieldset[disabled] → 输入框文本灰化(150,150,150)。
    let (fx, fy, fw, fh) = rect(&tool, &page_id, "fi").await;
    let disabled_gray = count_color(&img, (fx + 2.0) as i64, (fy + 2.0) as i64, (fx + fw - 2.0) as i64, (fy + fh - 2.0) as i64, [150, 150, 150]);
    assert!(disabled_gray > 5, "fieldset[disabled] 内控件文字应灰化,灰像素={disabled_gray}");

    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}
