//! Canvas 像素经浏览器主管线进入截图的端到端回归(仅本地 mock,无外网)。

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
                html,body{margin:0;height:100%}#page{display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:100vh;background:#181818}
                #card{width:100%;max-width:520px;min-height:515px;background:#fff}#c{width:40px;height:40px}
                </style></head><body><div id="page"><div id="card">安全验证<canvas id="c" width="4" height="4"></canvas></div></div>
                <script>var c=document.getElementById('c');var x=c.getContext('2d');x.fillStyle='#00ff00';x.fillRect(0,0,4,4);</script>
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

#[tokio::test]
async fn canvas像素与验证卡几何进入真实截图() {
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
    let path = dir.path().join("canvas.png");
    let shot = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"screenshot","params":{"save_path":path.to_string_lossy(),"full_page":false}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    assert!(path.exists(), "截图应真实落盘:{shot}");
    let bytes = std::fs::read(&path).unwrap();
    let image = image::load_from_memory(&bytes).expect("截图 PNG 应可解码").to_rgba8();
    assert_eq!((image.width(), image.height()), (1920, 1080));

    let geometry = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"eval_js","params":{"script":"JSON.stringify({card:(function(){var r=document.getElementById('card').getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height};})(),canvas:(function(){var r=document.getElementById('c').getBoundingClientRect();return {x:r.x,y:r.y,w:r.width,h:r.height};})(),page:document.getElementById('page').getBoundingClientRect().height})"}}),
    )
    .await;
    let result = geometry["data"]["result"].as_str().unwrap_or("{}");
    let geo: Value = serde_json::from_str(result).unwrap_or_else(|_| panic!("几何结果非法:{geometry}"));
    assert!((geo["page"].as_f64().unwrap() - 1080.0).abs() < 2.0, "验证页高度错误:{geo}");
    assert!((geo["card"]["w"].as_f64().unwrap() - 520.0).abs() < 2.0, "验证卡宽度错误:{geo}");
    let cx = (geo["canvas"]["x"].as_f64().unwrap() + geo["canvas"]["w"].as_f64().unwrap() / 2.0) as u32;
    let cy = (geo["canvas"]["y"].as_f64().unwrap() + geo["canvas"]["h"].as_f64().unwrap() / 2.0) as u32;
    assert_eq!(image.get_pixel(cx, cy).0, [0, 255, 0, 255], "Canvas 中心像素未进入截图:({cx},{cy}) geo={geo}");
    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}

/// 第 56 轮:Canvas 2D 路径(圆环描边)/渐变/文字经宿主光栅化进入真实截图。
#[tokio::test]
async fn canvas路径渐变文字进入真实截图() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let Ok(n) = socket.read(&mut tmp).await else { return };
                    if n == 0 { return }
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") { break }
                }
                let body = r##"<!doctype html><html><head><style>
                html,body{margin:0}#wrap{padding:20px;background:#ffffff}
                #c{width:160px;height:90px;display:block}
                </style></head><body><div id="wrap"><canvas id="c" width="160" height="90"></canvas></div>
                <script>
                var c = document.getElementById('c');
                var x = c.getContext('2d');
                var g = x.createLinearGradient(0, 0, 160, 0);
                g.addColorStop(0, '#ff2200');
                g.addColorStop(1, '#0033ff');
                x.fillStyle = g;
                x.fillRect(0, 0, 160, 30);
                x.beginPath();
                x.arc(45, 60, 20, 0, Math.PI * 2);
                x.lineWidth = 6;
                x.strokeStyle = '#00cc44';
                x.stroke();
                x.font = 'bold 14px sans-serif';
                x.fillStyle = '#802010';
                x.fillText('LAEW', 100, 65);
                </script>
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
    let base = format!("http://{}", addr);
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action":"open","url":base,"mode":"hidden","reuse":false,"viewport":{"width":1280,"height":800}}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    let geometry = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"eval_js","params":{"script":"(function(){var r=document.getElementById('c').getBoundingClientRect();return JSON.stringify({x:r.x,y:r.y,w:r.width,h:r.height});})()"}}),
    )
    .await;
    let result = geometry["data"]["result"].as_str().unwrap_or("{}");
    let geo: Value = serde_json::from_str(result).unwrap_or_else(|_| panic!("几何结果非法:{geometry}"));
    assert!((geo["w"].as_f64().unwrap() - 160.0).abs() < 2.0, "canvas 几何:{geo}");
    assert!((geo["h"].as_f64().unwrap() - 90.0).abs() < 2.0, "canvas 几何:{geo}");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("canvas56.png");
    let shot = exec(
        &tool,
        json!({"action":"control","page_id":page_id,"control_action":"screenshot","params":{"save_path":path.to_string_lossy(),"full_page":false}}),
    )
    .await;
    assert_eq!(shot["code"], 0, "{shot}");
    assert!(path.exists(), "截图应真实落盘:{shot}");
    let bytes = std::fs::read(&path).unwrap();
    let image = image::load_from_memory(&bytes).expect("截图 PNG 应可解码").to_rgba8();

    let ox = geo["x"].as_f64().unwrap().round() as i32;
    let oy = geo["y"].as_f64().unwrap().round() as i32;
    let at = |lx: i32, ly: i32| -> [u8; 4] {
        let px = (ox + lx).clamp(0, image.width() as i32 - 1) as u32;
        let py = (oy + ly).clamp(0, image.height() as i32 - 1) as u32;
        image.get_pixel(px, py).0
    };
    // 渐变左端偏红、右端偏蓝;圆环描边偏绿。
    let left = at(8, 10);
    assert!(left[0] > 180 && left[2] < 90, "渐变左端应偏红:{left:?} geo={geo}");
    let right = at(152, 10);
    assert!(right[2] > 150 && right[0] < 90, "渐变右端应偏蓝:{right:?} geo={geo}");
    let ring = at(45, 40);
    assert!(ring[1] > 120, "圆环描边应偏绿:{ring:?} geo={geo}");
    // 截图无 alpha 通道语义:圆环中心孔应露出页面白底(无绿)。
    let hole = at(45, 60);
    assert!(hole[0] as u32 + hole[1] as u32 + hole[2] as u32 > 660, "圆环中心孔应近白:{hole:?} geo={geo}");
    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}
