//! 电商页典型 Flex、零高度与比例骨架的跨模块布局回归测试。

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
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let body = r##"<html><head><style>
                    form{display:flex;width:792px;position:relative}.combobox{position:relative;width:100%;overflow:hidden}
                    .combobox>input{width:100%;height:34px;padding:9px 0;box-sizing:border-box;
                      vertical-align:middle}
                    .btn{position:absolute;right:2px;width:72px;height:36px}
                    .logo{width:240px;height:72px}.logo a{display:block;width:240px;height:0;
                      padding-top:72px;font-size:200px;line-height:290px;overflow:hidden}
                    .skeleton{width:304px}.skeleton .img-wrapper{width:100%;padding-top:100%;
                      position:relative;overflow:hidden}.skeleton-title{height:16px;margin-top:8px}
                    </style></head><body>
                    <form><i></i><div><button class="btn">搜索</button></div><div></div>
                      <div class="combobox"><input id="q" aria-label="请输入搜索文字"></div>
                      <input type="hidden" name="a"><input type="hidden" name="b">
                    </form>
                    <form id="form2"><div style="width:100px"></div><div><button id="btn2" class="btn">搜索</button></div></form>
                    <div class="logo"><h1><a id="logo">天猫</a></h1></div>
                    <div class="skeleton"><div class="img-wrapper" id="ratio"></div>
                      <div class="skeleton-title"></div></div>
                    </body></html>"##;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body
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

fn rect(value: &Value) -> (f64, f64, f64, f64) {
    (
        value["data"]["rect"]["x"].as_f64().unwrap(),
        value["data"]["rect"]["y"].as_f64().unwrap(),
        value["data"]["rect"]["width"].as_f64().unwrap(),
        value["data"]["rect"]["height"].as_f64().unwrap(),
    )
}

#[tokio::test]
async fn 电商搜索logo与比例骨架几何符合css语义() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_WEB_SCRIPT", "off");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(&tool, json!({"action":"open","url":base,"mode":"hidden","reuse":false})).await;
    assert_eq!(open["code"], 0, "mock 页面应打开成功: {open}");
    let page_id = open["data"]["page_id"].as_str().unwrap().to_string();

    for (selector, expected_w, expected_h) in [
        (".combobox", 792.0, 34.0),
        ("#q", 792.0, 34.0),
        ("#logo", 240.0, 72.0),
        ("#ratio", 304.0, 304.0),
    ] {
        let result = exec(&tool, json!({"action":"inspect","info":"elements","page_id":page_id,
            "params":{"selector":selector}})).await;
        let (_, _, w, h) = rect(&result);
        assert!((w - expected_w).abs() < 0.5, "{selector} 宽度应={expected_w}, 实际={w}: {result}");
        assert!((h - expected_h).abs() < 0.5, "{selector} 高度应={expected_h}, 实际={h}: {result}");
    }
    let button = exec(&tool, json!({"action":"inspect","info":"elements","page_id":page_id,
        "params":{"selector":".btn"}})).await;
    let (x, _, w, h) = rect(&button);
    assert!((x - 726.0).abs() < 0.5, "right:2px 应相对最近定位祖先 form,而非零宽父盒: {button}");
    assert!((w - 72.0).abs() < 0.5 && (h - 36.0).abs() < 0.5,
        "脱流按钮仍应在正式布局中发射,只是在内在尺寸测量时不贡献宽度: {button}");
    let second = exec(&tool, json!({"action":"inspect","info":"elements","page_id":page_id,
        "params":{"selector":"#btn2"}})).await;
    let (x2, _, _, _) = rect(&second);
    assert!((x2 - 726.0).abs() < 0.5,
        "非首个 Flex 条目中的 absolute 也应只平移一次: {second}");

    exec(&tool, json!({"action":"close","page_id":page_id})).await;
}
