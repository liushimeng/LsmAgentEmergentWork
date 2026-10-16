//! MCP_Web_Use 真浏览器冒烟测试(需要本机安装 Chrome/Edge;离线,用 data: URL)。
//!
//! 2026-09-18 第 89 轮:原 Chromium-WebUse 5 工具冒烟改写为单工具 action 分发形态。
//! 单测试函数串行执行(BrowserManager 是进程内单例,并行多测试会互相回收浏览器)。
//! 默认 `#[ignore]`,显式运行:`cargo test --test web_use_smoke -- --ignored`

use lsm_agent::agent::browser::detect_browser;
use lsm_agent::agent::tools::mcp_web_use::McpWebUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// data: URL 中属性一律单引号(未编码双引号会被 Chrome 截断解析)
/// BrowserManager 是进程内单例,同二进制内的多个真浏览器冒烟测试必须串行,
/// 否则并行运行会互相回收对方的浏览器/页面。
static SMOKE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const PAGE: &str = "data:text/html,<html><head><title>laew-smoke</title></head>\
<body><input id='q' value=''><button id='go' onclick='document.title=\"clicked\"'>Go</button>\
<h1 id='h'>hello laew</h1></body></html>";

/// 第 100 轮:blockers 冒烟页(含短信验证码文案,percent-encode 防中文被截断)。
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn data_of(out: &str) -> Value {
    serde_json::from_str(out).unwrap_or(Value::Null)
}

/// 启动一次性本地 HTTP 服务,验证 CDP 下载事件与真实落盘路径。
async fn spawn_download_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let _ = serve_download_once(&mut socket).await;
        }
    });
    format!("http://{addr}/download")
}

/// 第 162 轮:地理位置权限冒烟 —— 启动本地 HTTP 服务返回探测页,
/// 页面把 `navigator.permissions.query({name:'geolocation'})` 的状态写进标题。
/// 起一个返回固定 HTML 的一次性 HTTP 服务（第 174 轮）。
///
/// 用 `http://127.0.0.1`（而非 `data:` URL）是刻意的：loopback 属**安全上下文**，
/// `navigator.userAgentData`（Client Hints）只有在那里才暴露 —— UA 去令牌路径要
/// 连同 `Sec-CH-UA` 品牌表一起验证，就必须在这个上下文里跑。
async fn spawn_html_server(html: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    tokio::spawn(async move {
                        let mut buf = vec![0_u8; 4096];
                        let mut read = 0;
                        loop {
                            match socket.read(&mut buf[read..]).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    read += n;
                                    if buf[..read].windows(4).any(|w| w == b"\r\n\r\n")
                                        || read == buf.len()
                                    {
                                        break;
                                    }
                                }
                            }
                        }
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            html.len(),
                            html
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    format!("http://{addr}/")
}

async fn spawn_geo_server() -> String {
    let html = "<html><head><title>geo:pending</title><script>navigator.permissions.query({name:'geolocation'}).then(r => { document.title = 'geo:' + r.state; }).catch(() => { document.title = 'geo:error'; });</script></head><body>geo</body></html>";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    tokio::spawn(async move {
                        let mut buf = vec![0_u8; 4096];
                        let mut read = 0;
                        loop {
                            match socket.read(&mut buf[read..]).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    read += n;
                                    if buf[..read].windows(4).any(|w| w == b"\r\n\r\n")
                                        || read == buf.len()
                                    {
                                        break;
                                    }
                                }
                            }
                        }
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            html.len(),
                            html
                        );
                        let _ = socket.write_all(resp.as_bytes()).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    format!("http://{addr}/")
}

async fn serve_download_once(socket: &mut TcpStream) -> std::io::Result<()> {
    let mut buf = vec![0_u8; 4096];
    let mut read = 0;
    loop {
        let n = socket.read(&mut buf[read..]).await?;
        if n == 0 {
            break;
        }
        read += n;
        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") || read == buf.len() {
            break;
        }
    }
    let body = b"laew-download-ok\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=laew-download.txt\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.shutdown().await
}

#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_open_control_inspect_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // BrowserManager 的 watchdog 需要调用 laew 隐藏子命令；测试 harness 自身的
    // current_exe 不能解析该 CLI，这里显式指向编译产物。
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    // 0) 信封永不 panic:无浏览器环境返回 code=3001 + 安装引导;有浏览器 code=0。
    //    先验证这一点,成功打开的页面立即关闭(不留脏状态,后续步骤重新开页)。
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": "data:text/html,<title>x</title>"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert!(
        v["code"] == 0 || v["code"] == 3001,
        "open 返回码应为 0 或 3001: {out}"
    );
    if v["code"] == 3001 {
        // 本机未安装浏览器:后续步骤的前提不成立,信封验证即完成
        return;
    }
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": v["data"]["page_id"]}))
        .await;

    // 1) 检测到浏览器
    assert!(
        detect_browser().is_some(),
        "本机应能检测到 Chromium 系浏览器"
    );

    // 2) action=open 打开 data: 页面(离线)
    let out = McpWebUseTool.execute(json!({"action": "open", "url": PAGE})).await.unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "open 应成功: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();

    // 3) action=inspect title
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "title"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["title"], "laew-smoke");

    // 4) action=control input_text(JS 原生 setter 路径)
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "input_text",
            "params": {"selector": "#q", "text": "你好 laew"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "input_text 应成功: {out}");

    // 5) eval_js 读回输入值
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "eval_js",
            "params": {"expression": "document.querySelector('#q').value"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["data"]["result"], "你好 laew");

    // 6) click 按钮改标题
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "click",
            "params": {"selector": "#go"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "click 应成功: {out}");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "title"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["data"]["title"], "clicked", "点击后标题应变");

    // 7.5) 连续执行模式:同一 page_id 批内占位符传递,一次完成输入/点击/验证
    let out = McpWebUseTool
        .execute(json!({
            "action": "sequence",
            "page_id": page_id,
            "steps": [
                {"action": "control", "page_id": "$page_id", "control_action": "input_text",
                 "params": {"selector": "#q", "text": "sequence 你好", "use_js": true}},
                {"action": "control", "page_id": "$page_id", "control_action": "click",
                 "params": {"selector": "#go"}},
                {"action": "control", "page_id": "$page_id", "control_action": "wait",
                 "params": {"duration_ms": 300}},
                {"action": "inspect", "page_id": "$page_id", "info": "title"}
            ]
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "sequence 应成功: {out}");
    assert_eq!(v["data"]["completed"], true);
    assert_eq!(v["data"]["step_count"], 4);
    assert_eq!(v["data"]["page_id"], page_id);
    assert_eq!(v["data"]["steps"][3]["data"]["title"], "clicked");

    // 7) elements 检视
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": page_id, "info": "elements", "params": {"selector": "#h"}}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["text"], "hello laew");

    // 8) 截图落盘
    let path = std::env::temp_dir().join(format!(
        "laew_smoke_{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "screenshot",
            "params": {"save_path": path.to_str().unwrap()}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "screenshot 应成功: {out}");
    assert!(path.exists() && std::fs::metadata(&path).unwrap().len() > 100);

    // 8.5) 真实下载:监听 Browser.downloadWillBegin/Progress,验证最终文件内容
    let url = spawn_download_server().await;
    let dir = std::env::temp_dir().join(format!(
        "laew_download_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "download",
            "params": {
                "url": url,
                "save_dir": dir.to_str().unwrap(),
                "filename": "laew-download-final.txt",
                "timeout_ms": 10000
            }
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "download 应成功: {out}");
    let saved = v["data"]["save_path"].as_str().unwrap().to_string();
    assert_eq!(v["data"]["byte_size"], json!("laew-download-ok\n".len()));
    assert_eq!(
        std::fs::read_to_string(&saved).unwrap(),
        "laew-download-ok\n"
    );

    // 9) action=close 幂等
    let out = McpWebUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    let out = McpWebUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 2000, "重复关闭应 2000");

    // 10) 第 100 轮:窗口可视化 + 人工介入元数据冒烟(headed 1080p / 高亮 /
    //      set_window / sync_viewport / blockers / 浏览器实例复用 / HITL 非交互 4001)
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
    let sms_html = percent_encode(
        "<html><head><title>sms-wall</title></head><body><p>短信验证码已发送,请输入</p></body></html>",
    );
    let sms_url = format!("data:text/html;charset=utf-8,{sms_html}");
    let out = McpWebUseTool
        .execute(json!({
            "action": "open",
            "url": sms_url,
            "mode": "headed",
            "window_width": 1280,
            "window_height": 800
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "headed open 应成功: {out}");
    assert_eq!(v["data"]["mode"], "headed", "实际 mode 应为 headed: {out}");
    assert_eq!(v["data"]["browser_reused"], false, "全新浏览器不应标记复用: {out}");
    assert_eq!(v["data"]["highlight"], true, "headed 默认注入高亮: {out}");
    // window 字段形状(第 161 轮后):{requested, default, min_viewport, hint},
    // 请求尺寸回显在 requested 数组。
    assert_eq!(
        v["data"]["window"]["requested"],
        serde_json::json!([1280, 800]),
        "窗口尺寸应回显请求值: {out}"
    );
    let headed_page = v["data"]["page_id"].as_str().unwrap().to_string();

    // set_window 运行时调窗(headed 冒烟用小尺寸,避免打扰桌面)
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": headed_page,
            "control_action": "set_window",
            "params": {"width": 1100, "height": 700}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "set_window 应成功: {out}");
    // 第 165 轮:Windows 实测 CDP setWindowBounds 不保证几何精确回环 ——
    // device-scale-factor=1.25 下 DIP↔物理取整 + 窗口边框,请求 1100×700 稳定
    // 回读 1102×702(999×777 → 1001×778)。断言改为容差比较并校验 requested 回显,
    // 不假设逐像素相等(macOS 通常相等,Windows 通常差 1~2px)。
    assert_eq!(
        v["data"]["requested"]["width"], 1100,
        "requested 应回显请求宽: {out}"
    );
    assert_eq!(
        v["data"]["requested"]["height"], 700,
        "requested 应回显请求高: {out}"
    );
    let gw = v["data"]["window"]["width"].as_i64().unwrap_or(0);
    let gh = v["data"]["window"]["height"].as_i64().unwrap_or(0);
    assert!((gw - 1100).abs() <= 4, "回读窗口宽偏差过大: {gw} (req 1100) {out}");
    assert!((gh - 700).abs() <= 4, "回读窗口高偏差过大: {gh} (req 700) {out}");

    // sync_viewport:视口=窗口内容区(自适应渲染)
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": headed_page,
            "control_action": "sync_viewport"
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "sync_viewport 应成功: {out}");
    assert!(v["data"]["width"].as_i64().unwrap_or(0) > 0, "视口宽应非零: {out}");

    // blockers:短信验证码墙检测
    let out = McpWebUseTool
        .execute(json!({
            "action": "inspect", "page_id": headed_page, "info": "blockers"
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "blockers 应成功: {out}");
    assert_eq!(v["data"]["blocked"], true, "应检出阻断: {out}");
    assert!(
        v["data"]["blockers"]
            .as_array()
            .map(|arr| arr.iter().any(|b| b["kind"] == "sms"))
            .unwrap_or(false),
        "应检出 sms 阻断(徽标文本不得误报 captcha): {out}"
    );
    assert_eq!(v["data"]["suggested_action"], "request_human");

    // set_highlight 运行时开关
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": headed_page,
            "control_action": "set_highlight", "params": {"enabled": false}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "set_highlight 应成功: {out}");
    assert_eq!(v["data"]["enabled"], false);

    // 浏览器实例复用:再次 open(同 URL)→ 页面 reused + 浏览器 browser_reused
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": sms_url, "mode": "headed"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "二次 open 应成功: {out}");
    assert_eq!(v["data"]["reused"], true, "同 URL 页面应复用: {out}");
    assert_eq!(v["data"]["browser_reused"], true, "浏览器实例应复用: {out}");
    assert_eq!(v["data"]["page_id"], headed_page.as_str(), "page_id 应不变: {out}");

    // request_human:测试进程非 TUI(未 attach)→ 4001 快速失败(不挂起)
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": headed_page,
            "control_action": "request_human",
            "params": {"reason": "sms", "timeout_ms": 10000}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 4001, "非交互环境应 4001: {out}");
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;

    // 11) 第 125 轮:视口基准 1080p + 2K 自动扩展(hidden 宽页触发横向裁切)。
    //     内容 2400px 宽 > 默认视口 1920 → open 后自动扩展,扩展后溢出消失、
    //     视口外按钮可点击;auto_expand_viewport=false 关闭后溢出保留、
    //     截图响应带 content_overflow hint。
    let wide_url = "data:text/html,<html><head><title>wide</title></head>\
<body style='width:2400px;height:1100px;margin:0'>\
<button id='far' style='position:absolute;left:2300px;top:10px' \
onclick='document.title=\"far-clicked\"'>F</button></body></html>";
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": wide_url}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "宽页 open 应成功: {out}");
    let wide_page = v["data"]["page_id"].as_str().unwrap().to_string();
    assert_eq!(
        v["data"]["viewport"]["expanded"],
        true,
        "内容 2400 > 视口 1920 应自动扩展视口: {out}"
    );
    let to_w = v["data"]["viewport"]["to"]["width"].as_f64().unwrap_or(0.0);
    assert!(to_w >= 2400.0, "扩展后视口宽应 ≥2400(实际 {to_w}): {out}");
    assert_eq!(
        v["data"]["viewport"]["clamped"],
        false,
        "2400 ≤ 2K 上限不应截断: {out}"
    );

    // inspect(info=viewport):扩展后横向溢出消失
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": wide_page, "info": "viewport"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    assert_eq!(
        v["data"]["overflow"]["horizontal"],
        false,
        "扩展后横向裁切应消失: {out}"
    );

    // 原视口外(left:2300)按钮在扩展后可真实点击生效
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": wide_page,
            "control_action": "click", "params": {"selector": "#far"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "扩展后视口外按钮 click 应成功: {out}");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": wide_page, "info": "title"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["data"]["title"], "far-clicked", "点击应真实命中按钮: {out}");
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;

    // auto_expand_viewport=false:不扩展(全新 hidden 实例,视口保持 1920)
    let out = McpWebUseTool
        .execute(json!({
            "action": "open", "url": wide_url, "auto_expand_viewport": false
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    assert!(
        v["data"]["viewport"].is_null(),
        "false 应关闭自动扩展且不回 viewport 字段: {out}"
    );
    let nofit_page = v["data"]["page_id"].as_str().unwrap().to_string();
    let out = McpWebUseTool
        .execute(json!({"action": "inspect", "page_id": nofit_page, "info": "viewport"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(
        v["data"]["overflow"]["horizontal"],
        true,
        "关闭扩展后横向裁切应保留: {out}"
    );

    // 截图不带 full_page 时附 content_overflow hint
    let clip_path = std::env::temp_dir().join(format!(
        "laew_smoke_clip_{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": nofit_page,
            "control_action": "screenshot",
            "params": {"save_path": clip_path.to_str().unwrap()}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "screenshot 应成功: {out}");
    assert_eq!(
        v["data"]["content_overflow"]["horizontal"],
        true,
        "溢出截图应带 content_overflow 提示: {out}"
    );
    let _ = std::fs::remove_file(&clip_path);

    std::env::remove_var("LAEW_WEB_GEO"); // 默认 grant 档

    // 12) 第 162 轮:地理位置权限自动处置 —— open http origin 页面 → data.permissions.geolocation 应为 granted
    let url = spawn_geo_server().await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "open 应成功: {out}");
    assert_eq!(
        v["data"]["permissions"]["geolocation"], "granted",
        "open 应自动预授权地理位置: {out}"
    );
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();

    // 13) 页面内 Permissions API 实时回读应为 granted(预授权真实生效;
    //    标题是页面加载瞬间的一次性快照,setPermission 在后,须重新查询)
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "eval_js",
            "params": {"expression":
                "navigator.permissions.query({name:'geolocation'}).then(r => r.state)",
                "await_promise": true}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(
        v["data"]["result"], "granted",
        "页面内 Permissions API 应读到 granted(原生弹窗不会再出现): {out}"
    );

    // 14) control(navigate) 同 origin 重放授权(navigate 路径同样生效)
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "navigate",
            "params": {"url": url}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "navigate 应成功: {out}");
    assert_eq!(
        v["data"]["permissions"]["geolocation"], "granted",
        "navigate 后应重放地理位置预授权: {out}"
    );

    // 15) data:/about: 页面 origin 抽取应为 skipped(不报错不授权)
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": "data:text/html,<title>g2</title>", "reuse": false}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "open data: 应成功: {out}");
    assert_eq!(
        v["data"]["permissions"]["geolocation"], "skipped",
        "非 http(s) origin 应跳过授权: {out}"
    );


    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// 2026-10-15:eval_js 顶层词法屏蔽真浏览器验证(闲鱼实测缺陷回归)。
///
/// 背景:`Runtime.evaluate` 在全局词法环境执行,顶层 `const` 声明跨调用常驻,
/// 同名再声明必 `SyntaxError: Identifier 'sleep' has already been declared`
/// (模型给临时变量起名高度趋同,会话越长越容易踩中)。修复后两次调用应都成功,
/// 且第二次的全局词法环境不受第一次污染(window 级赋值不受影响)。
#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_eval_js_lexical_shield_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": PAGE}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert!(
        v["code"] == 0 || v["code"] == 3001,
        "open 返回码应为 0 或 3001: {out}"
    );
    if v["code"] == 3001 {
        return; // 本机未安装浏览器:信封验证即完成
    }
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();

    // 第一次:顶层 const 声明(模型高频形态)。
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "eval_js",
            "params": {"expression": "const sleep = ms => new Promise(r => setTimeout(r, ms)); window.__lex_probe = 41; window.__lex_probe + 1"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "第一次顶层 const 声明应成功: {out}");
    assert_eq!(v["data"]["result"], 42, "表达式值应为 42: {out}");

    // 第二次:同名 const 再声明(修复前必 SyntaxError)。
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "eval_js",
            "params": {"expression": "const sleep = ms => ms; window.__lex_probe + 1"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "第二次同名 const 再声明不应 SyntaxError: {out}");
    assert_eq!(v["data"]["result"], 42, "值语义不变: {out}");

    // window 级赋值不受屏蔽影响(块级包裹只挡词法泄漏)。
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "eval_js",
            "params": {"expression": "window.__lex_probe"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["data"]["result"], 41, "window 赋值不受屏蔽影响: {out}");

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_multi_window_parallel_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 多窗口并行(双站同时工作):open(new_window=true) 必须真起一个独立可见窗口 ——
    // 两页面 visibilityState 均 visible(互不挂起)、window_id 不同、窗口被自动平铺。
    if detect_browser().is_none() {
        eprintln!("未检测到浏览器,跳过");
        return;
    }

    // 1) 主窗口打开第一个页面
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": PAGE, "reuse": false}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "首个 open 应成功: {out}");
    let page1 = v["data"]["page_id"].as_str().unwrap_or_default().to_string();
    assert!(!page1.is_empty());
    assert!(
        v["data"].get("new_window").is_none() || v["data"]["new_window"] == false,
        "普通 open 不应带 new_window 语义: {out}"
    );

    // 2) 第二个站点:独立新窗口打开
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": PAGE, "new_window": true}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "new_window open 应成功: {out}");
    assert_eq!(v["data"]["new_window"], true, "响应应标注 new_window: {out}");
    let page2 = v["data"]["page_id"].as_str().unwrap_or("").to_string();
    assert!(!page2.is_empty());
    assert_ne!(page1, page2, "新窗口必须分配新 page_id");

    // 3) 两窗口自动平铺报告(互不遮挡,防 occlusion 节流)
    assert!(
        v["data"]["windows"]["tiled"] == true,
        "第二个窗口打开后应触发平铺: {out}"
    );
    let detail = v["data"]["windows"]["detail"].as_array().cloned().unwrap_or_default();
    assert!(detail.len() >= 2, "平铺报告应覆盖 ≥2 窗口: {out}");

    // 4) list:两页面 window_id 不同、均有 foreground 探测能力
    let out = McpWebUseTool
        .execute(json!({"action": "list"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0);
    assert!(v["data"]["windows"].as_u64().unwrap_or(0) >= 2, "list 应报 ≥2 窗口: {out}");
    let pages = v["data"]["pages"].as_array().cloned().unwrap_or_default();
    let w1 = pages
        .iter()
        .find(|p| p["page_id"] == page1)
        .and_then(|p| p["window_id"].as_i64());
    let w2 = pages
        .iter()
        .find(|p| p["page_id"] == page2)
        .and_then(|p| p["window_id"].as_i64());
    assert!(w1.is_some() && w2.is_some(), "list 每页应有 window_id: {out}");
    assert_ne!(w1, w2, "两页面必须分属不同窗口: {out}");

    // 5) 两个窗口的页面同时活跃:visibilityState 均为 visible(互不挂起的核心证据)。
    // list 的 foreground 即「visibilityState=='visible'」的即时探测(人工切页实时反映)。
    let fg: std::collections::HashMap<String, bool> = pages
        .iter()
        .filter_map(|p| {
            Some((
                p["page_id"].as_str()?.to_string(),
                p["foreground"].as_bool()?,
            ))
        })
        .collect();
    for pid in [&page1, &page2] {
        assert_eq!(
            fg.get(pid.as_str()).copied(),
            Some(true),
            "两窗口页面应同时 visible(互不挂起),实际 foreground 表: {fg:?};list: {out}"
        );
    }

    // 6) control(new_window):在独立新窗口打开第三个页面并直接回 page_id
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page1, "control_action": "new_window",
            "params": {"url": PAGE}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "control(new_window) 应成功: {out}");
    assert_eq!(v["data"]["new_window"], true);
    let page3 = v["data"]["page_id"].as_str().unwrap_or("").to_string();
    assert!(!page3.is_empty() && page3 != page1 && page3 != page2);

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// 第 169 轮:隐身自检冒烟(京东风控墙根治回归锁)——
/// 实测 Chrome 155 下即使剔除 `--enable-automation` 且传
/// `--disable-blink-features=AutomationControlled`,页面 `navigator.webdriver`
/// 仍为 `true`;且注入 UI 文本(「LAEW Agent 控制中」)暴露在 `document.body.innerText`
/// ——两者被天猫/京东系风控 JS 采集后判为自动化环境(登录风控复核返回
/// `.jd-main-risk-fallback`「当前页面异常」,jsonpLogin/pc_home_feed 403)。
/// 本用例经 McpWebUseTool 真实启动浏览器验证:webdriver 覆写生效、注入 UI
/// 文本页面不可读(closed Shadow DOM)、window 无 laew 残留键、管控视觉层在场。
#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn stealth_selfcheck_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": PAGE, "guard": "locked"}))
        .await
        .unwrap();
    let v = data_of(&out);
    if v["code"] == 3001 {
        // 本机未安装浏览器:信封验证即完成
        return;
    }
    assert_eq!(v["code"], 0, "open 应成功: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();

    let eval = |expr: String| {
        let pid = page_id.clone();
        async move {
            let out = McpWebUseTool
                .execute(json!({
                    "action": "control", "page_id": pid, "control_action": "eval_js",
                    "params": {"expression": expr}
                }))
                .await
                .unwrap();
            let v = data_of(&out);
            assert_eq!(v["code"], 0, "eval_js 应成功: {out}");
            v["data"]["result"].as_str().unwrap_or_default().to_string()
        }
    };

    // ① navigator.webdriver 覆写生效(返回 false,与真实 Chrome 语义一致)
    let wd = eval("String(navigator.webdriver)".into()).await;
    assert_eq!(wd, "false", "navigator.webdriver 覆写应生效(风控指纹①)");

    // ② 注入 UI 文本页面不可读(徽标/状态条在 closed Shadow DOM)
    let text = eval("document.body.innerText + '|' + document.body.textContent".into()).await;
    assert!(
        !text.contains("LAEW Agent"),
        "页面文本面不得残留品牌签名(风控指纹②):{text}"
    );

    // ③ window 无可枚举 laew 残留键
    let leaked = eval(
        "String(Object.keys(window).filter((k) => k.toLowerCase().indexOf('laew') >= 0).length)".into(),
    )
    .await;
    assert_eq!(leaked, "0", "window 不应有可枚举 laew 键");

    // ④ 管控视觉层在影子树里真实存在(隐身不牺牲管控功能)
    let probe = eval(
        "JSON.stringify((window.%NS%GuardUiProbe && window.%NS%GuardUiProbe()) || null)".replace("%NS%", lsm_agent::agent::browser_ns::inject_ns()),
    )
    .await;
    let p: Value = serde_json::from_str(&probe).unwrap_or(Value::Null);
    assert_eq!(p["mask"], json!(true), "蒙层应在场:{probe}");
    assert_eq!(p["shields"], json!(true), "盾区容器应在场:{probe}");

    // ⑤ 第 171 轮:屏蔽页面状态栏(右上角)+ 屏蔽页面进度栏(左下角)在影子树里在场。
    //    左下角静态状态条已迁入状态栏,guard 探针不再有 `hint` 字段。
    assert!(p.get("hint").is_none(), "静态状态条已迁至状态栏(第 171 轮):{probe}");
    let sprobe = eval(
        "JSON.stringify((window.%NS%StatusProbe && window.%NS%StatusProbe()) || null)"
            .replace("%NS%", lsm_agent::agent::browser_ns::inject_ns()),
    )
    .await;
    let sp: Value = serde_json::from_str(&sprobe).unwrap_or(Value::Null);
    assert_eq!(sp["status"], json!(true), "屏蔽页面状态栏应在场:{sprobe}");
    assert_eq!(sp["progress"], json!(true), "屏蔽页面进度栏应在场:{sprobe}");
    assert_eq!(sp["status_display"], json!(""), "状态栏应可见:{sprobe}");
    assert_eq!(sp["progress_display"], json!(""), "进度栏应可见:{sprobe}");
    // 2026-10-14 Windows 1080p 实测:两条栏必须**在视口内可见**且各自
    // `position:fixed` 角落锚定(不再共用全屏 inset:0 宿主 —— 页面 transform 会
    // 劫持 fixed 包含块导致「显示不出来」)。
    assert!(sp["viewport"]["w"].as_i64().unwrap_or(0) > 0, "应报 viewport:{sprobe}");
    for g in sp["geom"].as_array().cloned().unwrap_or_default() {
        if g.is_null() {
            continue;
        }
        assert_eq!(g["in_view"], json!(true), "两条栏都必须在视口内可见:{sprobe}");
        // 定位在 host 上(每条栏独立 position:fixed 角落锚定),内层 bar 是普通块
        assert_eq!(
            g["host_position"], json!("fixed"),
            "两条栏的 host 须 position:fixed 角落锚定(不再共用全屏 inset:0 宿主):{sprobe}"
        );
        assert_eq!(g["computed"]["display"], json!("block"), "两条栏不得被 display:none:{sprobe}");
        assert!(
            g["w"].as_i64().unwrap_or(0) > 0 && g["h"].as_i64().unwrap_or(0) > 0,
            "两条栏包围盒不得为零(行高塌陷回归锁):{sprobe}"
        );
    }
    // 独立 host 在场(REV 3)
    assert_eq!(sp["host"], json!(true), "状态栏独立 host 应在场:{sprobe}");
    assert_eq!(sp["host_progress"], json!(true), "进度栏独立 host 应在场:{sprobe}");
    // 动作标签已推到进度栏(本用例刚跑完 open → control eval_js 两次动作)
    assert!(
        sp["action"].as_str().unwrap_or("").contains("eval_js"),
        "进度栏应显示当前动作:{sprobe}"
    );

    // ============ 第 174 轮:反检测档位(非侵入式保真 / 侵入式原生化) ============
    // ⑥ 保真档:插件/语言非空(真实桌面 Chrome 恒有),UA 无无头令牌
    let plugins = eval("String(navigator.plugins ? navigator.plugins.length : 0)".into()).await;
    assert!(
        plugins.parse::<u32>().unwrap_or(0) >= 3,
        "plugins 应为真实桌面数量(空 = 典型无头特征):{plugins}"
    );
    let langs = eval("String(navigator.languages ? navigator.languages.length : 0)".into()).await;
    assert!(langs.parse::<u32>().unwrap_or(0) >= 1, "languages 不得为空:{langs}");
    let chrome_rt = eval("String(!!(window.chrome && window.chrome.runtime))".into()).await;
    // 第 175 轮真机实测纠正:全新 profile 的**真实 Chrome 就没有 chrome.runtime**
    // (`Object.keys(chrome)` 恒为 [app,csi,loadTimes])。旧断言「应在场」逼着
    // 保真块凭空补一个空 runtime —— 既违反「不伪造本机没有的信息」铁律,又可被
    // 一行识破(真实扩展上下文的 runtime 必有 id/connect/sendMessage)。
    // 现改为**断言不得凭空造出**:要么没有,要么真有扩展上下文的完整形态。
    assert!(
        chrome_rt == "false",
        "不得凭空补 chrome.runtime(真机裸环境无此字段):{chrome_rt}"
    );
    let ua_headless = eval("String(navigator.userAgent.indexOf('Headless') >= 0)".into()).await;
    assert_eq!(ua_headless, "false", "UA 不得残留 HeadlessChrome 令牌");

    // ⑦ WebGL 不得是自动化兜底串(风控最常采的高风险项)
    let renderer = eval(
        r#"(() => { try { const c = document.createElement('canvas');
            const g = c.getContext('webgl') || c.getContext('experimental-webgl');
            if (!g) return 'no-webgl';
            const e = g.getExtension('WEBGL_debug_renderer_info');
            return String(e ? g.getParameter(e.UNMASKED_RENDERER_WEBGL) : g.getParameter(g.RENDERER));
        } catch (err) { return 'err'; } })()"#
            .into(),
    )
    .await;
    let low = renderer.to_ascii_lowercase();
    assert!(
        !low.contains("webkit") && !low.contains("swiftshader") && !low.contains("software"),
        "WebGL renderer 不得是自动化兜底串:{renderer}"
    );

    // ⑧ 描述符形态还原:被改写的 WebGL 方法必须 enumerable(原生 WebIDL 形态,
    //    否则 for…in 枚举原型链即可发现「改过」)
    let enumerable = eval(
        "String(Object.getOwnPropertyDescriptor(WebGLRenderingContext.prototype, 'getParameter').enumerable)"
            .into(),
    )
    .await;
    assert_eq!(enumerable, "true", "被改写方法描述符须与原生同形(enumerable)");

    // ============ 第 175 轮:对象形态自洽 + 屏幕几何自洽 ============
    // ⑨ plugins/mimeTypes 替身必须与原生同构。「形状错误的回填比不回填更危险」:
    //    缺插件只是弱信号(受限环境本就没有),而 tag/instanceof 失配是强信号
    //    (正常浏览器不会造错形状的对象)。真机实测十项判据全中即被识破。
    let shape = eval(
        r#"JSON.stringify({
            tag: Object.prototype.toString.call(navigator.plugins),
            inst: navigator.plugins instanceof PluginArray,
            named: (() => { try { return !!navigator.plugins.namedItem('PDF Viewer'); } catch (e) { return false; } })(),
            item0: (() => { try { return Object.prototype.toString.call(navigator.plugins.item(0)); } catch (e) { return 'err'; } })(),
            mimeLen: navigator.mimeTypes.length,
            elemInst: navigator.plugins[0] instanceof Plugin,
            iter: typeof navigator.plugins[Symbol.iterator]
        })"#
            .into(),
    )
    .await;
    let sh: Value = serde_json::from_str(&shape).unwrap_or(Value::Null);
    assert_eq!(
        sh["tag"],
        json!("[object PluginArray]"),
        "plugins 替身须报原生 toString 标签(失配即强信号):{shape}"
    );
    assert_eq!(sh["inst"], json!(true), "plugins 须 instanceof PluginArray:{shape}");
    assert_eq!(
        sh["named"],
        json!(true),
        "plugins.namedItem('PDF Viewer') 须回指实例具名键:{shape}"
    );
    assert_eq!(sh["item0"], json!("[object Plugin]"), "item(0) 须为 Plugin:{shape}");
    assert_eq!(sh["elemInst"], json!(true), "plugins[0] 须 instanceof Plugin:{shape}");
    assert_eq!(sh["iter"], json!("function"), "PluginArray 须可迭代:{shape}");
    assert_eq!(
        sh["mimeLen"], json!(2),
        "mimeTypes 须按 type 去重为 2 条(按插件 push 会放大成 10 条):{shape}"
    );

    // ⑩ 屏幕几何自洽:`outer > screen` 是物理不可能(真实显示器宽度永远 ≥ 窗口
    //     宽度),属零成本风控硬判据。chromiumoxide 视口覆盖不发 screen 字段,
    //     补齐后此处应为 false。
    let geom = eval(
        r#"JSON.stringify({
            screen: screen.width + 'x' + screen.height,
            outer: outerWidth + 'x' + outerHeight,
            outer_gt_screen: outerWidth > screen.width || outerHeight > screen.height,
            headless_default: screen.width === 800 && screen.height === 600
        })"#
            .into(),
    )
    .await;
    let g: Value = serde_json::from_str(&geom).unwrap_or(Value::Null);
    assert_eq!(
        g["outer_gt_screen"], json!(false),
        "窗口不得比屏幕还大(物理不可能,风控硬判据):{geom}"
    );

    // ⑪ 响应回报 data.stealth / data.fingerprint(人工与 QC 的对账面)
    // 缺省档自 2026-10-16(2f93414)起升为 hardened(侵入式原生化):BOSS 直聘实测
    // fidelity 档下被改写函数的 toString 源码可读,被 geetest 类风控采集加权。
    assert_eq!(
        v["data"]["stealth"]["mode"],
        json!("hardened"),
        "缺省档应为侵入式原生化:{out}"
    );
    assert_eq!(v["data"]["stealth"]["fidelity"], json!(true), "保真块应生效:{out}");
    assert_eq!(v["data"]["stealth"]["hardened"], json!(true), "原生化块应生效:{out}");
    let codes: Vec<String> = v["data"]["fingerprint"]["suspicious"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["code"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !codes.contains(&"webdriver_true".to_string()),
        "webdriver 覆写应生效:{codes:?}"
    );
    assert!(
        !codes.contains(&"headless_ua".to_string()),
        "UA 不应带无头令牌:{codes:?}"
    );

    // ⑩ 缺省档(hardened)的分界线:被改写函数的身份必须原生化 —— BOSS 直聘实测
    //     fidelity 档下此处会吐出我们的源码,被 geetest 类风控采集加权后 302 到
    //     verify.html。「改过」这件事本身不可反查,正是侵入式档的职责。
    let fid_src =
        eval("Function.prototype.toString.call(WebGLRenderingContext.prototype.getParameter)".into())
            .await;
    assert!(
        fid_src.contains("[native code]"),
        "缺省 hardened 档必须把被改写函数的身份原生化:{fid_src}"
    );

    // ⑪ 升侵入式档:被改写函数身份原生化(含 toString 自反闭合)
    let up = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id,
            "control_action": "set_stealth", "params": {"mode": "hardened"}
        }))
        .await
        .unwrap();
    assert_eq!(data_of(&up)["code"], json!(0), "set_stealth(hardened) 应成功:{up}");
    let hard_src =
        eval("Function.prototype.toString.call(WebGLRenderingContext.prototype.getParameter)".into())
            .await;
    assert!(
        hard_src.contains("[native code]"),
        "侵入式档应原生化被改写函数:{hard_src}"
    );
    let self_src =
        eval("Function.prototype.toString.call(Function.prototype.toString)".into()).await;
    assert!(self_src.contains("[native code]"), "toString 自反闭合:{self_src}");

    // ⑫ 非法档位取值必须拒绝(不静默改档)
    let bad = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id,
            "control_action": "set_stealth", "params": {"mode": "turbo"}
        }))
        .await
        .unwrap();
    assert_eq!(data_of(&bad)["code"], json!(2002), "非法档位应报错:{bad}");

    // ⑬ 切回缺省档(后续用例与实例复用不受影响)
    let down = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id,
            "control_action": "set_stealth", "params": {"mode": "fidelity"}
        }))
        .await
        .unwrap();
    assert_eq!(data_of(&down)["code"], json!(0), "切回保真档应成功:{down}");

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await;
}

/// 第 174 轮:无头档 UA 去自动化令牌回归。
///
/// 无头 Chrome 的 UA 里的 `HeadlessChrome` 令牌同时出现在 UA 串、网络请求头与
/// `Sec-CH-UA` 品牌表三处,服务端一眼识别 —— 工具在首航**之前**经
/// `Emulation.setUserAgentOverride` 清掉（见 `browser_ns::apply_ua_destealth`）。
/// headed 档本无该令牌（零动作），故必须单独跑无头档才能覆盖这条路径。
#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn stealth_headless_ua_destealth_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let url = spawn_html_server("<html><head><title>ua</title></head><body>ua</body></html>").await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "mode": "hidden"}))
        .await
        .unwrap();
    let v = data_of(&out);
    if v["code"] == 3001 {
        return; // 本机未安装浏览器:信封验证即完成
    }
    assert_eq!(v["code"], 0, "hidden 档 open 应成功: {out}");
    assert_eq!(v["data"]["mode"], json!("hidden"), "应为无头档:{out}");
    assert_eq!(
        v["data"]["stealth"]["mode"],
        json!("hardened"),
        "缺省档应为侵入式原生化(2026-10-16 起):{out}"
    );
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();
    let eval = |expr: String| {
        let pid = page_id.clone();
        async move {
            let out = McpWebUseTool
                .execute(json!({
                    "action": "control", "page_id": pid, "control_action": "eval_js",
                    "params": {"expression": expr}
                }))
                .await
                .unwrap();
            let v = data_of(&out);
            assert_eq!(v["code"], 0, "eval_js 应成功: {out}");
            v["data"]["result"].as_str().unwrap_or_default().to_string()
        }
    };
    let ua = eval("navigator.userAgent".into()).await;
    assert!(
        !ua.contains("Headless"),
        "无头档 UA 不得残留 HeadlessChrome 令牌(须在首航前经 CDP 覆盖清掉):{ua}"
    );
    assert!(ua.contains("Chrome/"), "清理后仍应是正常 Chrome UA:{ua}");
    // Client Hints 品牌表同样不得残留无头品牌 —— 否则「UA 说 Chrome、Sec-CH-UA 说
    // Headless」是比原状更显眼的新破绽(只改 UA 串不改品牌表 = 半吊子清理)。
    let brands = eval(
        "JSON.stringify((navigator.userAgentData && navigator.userAgentData.brands) || [])".into(),
    )
    .await;
    let names = brands.to_ascii_lowercase();
    assert!(
        !names.contains("headless"),
        "Sec-CH-UA 品牌表不得残留无头品牌:{brands}"
    );
    assert!(
        brands.contains("Chromium") || brands.contains("Google Chrome"),
        "品牌表应是正常 Chrome 品牌(Client Hints 元数据未被清空):{brands}"
    );
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": page_id}))
        .await;
}

/// 第 170 轮:navigate 路径 page_health 回归 —— 服务器返回 200 空文档时
/// (猎聘 IP 风控实测形态:goto 成功但页面恒白),navigate 响应必须带
/// data.page_health{blank:true, reason:"empty_dom"} + landed_url,
/// 让 Agent 第一时间如实判定,而不是 wait-selector 超时瞎猜。
async fn serve_empty_html(socket: &mut TcpStream) -> std::io::Result<()> {
    let mut buf = vec![0_u8; 4096];
    let mut read = 0;
    loop {
        let n = socket.read(&mut buf[read..]).await?;
        if n == 0 {
            break;
        }
        read += n;
        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") || read == buf.len() {
            break;
        }
    }
    // 200 + 空 HTML 文档:WAF 空文档形态的最小复刻(主文档无错误码,DOM 仅 html/head/body)。
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await?;
    socket.shutdown().await
}

#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_navigate_page_health_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    tokio::spawn(async move {
                        let _ = serve_empty_html(&mut socket).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    let empty_url = format!("http://{addr}/");

    // 正常页打开,再 navigate 到空文档服务器。
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": "data:text/html,<title>base</title><h1>base</h1>"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "open 失败: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap_or_default().to_string();
    assert!(!page_id.is_empty());

    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "navigate",
            "params": {"url": empty_url}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "navigate 失败: {out}");
    let health = &v["data"]["page_health"];
    assert!(
        health["blank"].as_bool().unwrap_or(false),
        "200 空文档必须透出 page_health.blank: {out}"
    );
    assert_eq!(
        health["reason"].as_str().unwrap_or_default(),
        "empty_dom",
        "200 空文档 reason 应为 empty_dom: {out}"
    );
    assert_eq!(
        v["data"]["landed_url"].as_str().unwrap_or_default(),
        empty_url,
        "landed_url 应回传真实落地 URL: {out}"
    );

    // 对照:正常内容页 navigate 后**不带** page_health(零噪声)。
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "navigate",
            "params": {"url": "data:text/html,<title>ok</title><h1>real content</h1>"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "对照 navigate 失败: {out}");
    assert!(
        v["data"]["page_health"].is_null(),
        "健康页不应带 page_health: {out}"
    );

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// 第 171 轮:站点导航节奏闸冒烟 —— 用 `[::1]` 独立监听(其它冒烟测试全用
/// `127.0.0.1`,host_of 折叠后互不共享冷却键,不污染后续用例),验证:
/// ①频控冷却中 navigate/open 返 **6004**(动作不执行,附 host/remaining_secs);
/// ②`params.force=true` 逃生口放行;③`force` 放行后页面正常落地。
#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_pace_cooldown_smoke() {
    use lsm_agent::agent::browser_pace::WebPace;
    use std::time::Duration;
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    tokio::spawn(async move {
                        let _ = serve_empty_html(&mut socket).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    let cool_url = format!("http://[::1]:{}/", addr.port());

    // 基页(data: URL 无主机,天然不过节奏闸)
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": "data:text/html,<title>pace</title><h1>pace base</h1>"}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "open 基页失败: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap_or_default().to_string();

    // 标记 [::1] 频控冷却 60s(> 自动等待默认 45s → 走 6004 拦截)
    WebPace::global().mark_rate_limited_for("::1", Duration::from_secs(60));

    // ① navigate 命中节奏闸
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "navigate",
            "params": {"url": cool_url}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 6004, "频控冷却中 navigate 应返 6004: {out}");
    assert_eq!(v["data"]["host"].as_str().unwrap_or_default(), "::1", "host 应为 ::1: {out}");
    assert!(
        v["data"]["remaining_secs"].as_i64().unwrap_or(0) > 0,
        "remaining_secs 应 >0: {out}"
    );

    // ①' open 同闸(open 全路径共用节奏闸)
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": cool_url}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 6004, "频控冷却中 open 应返 6004: {out}");

    // ② force 逃生口:navigate 放行且真实落地
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "navigate",
            "params": {"url": cool_url, "force": true}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "force=true 应放行 navigate: {out}");

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// 2026-10-14:整页抹除(site_wipe)治理冒烟 —— 302→about:blank 确定性复刻
/// 「显式导航最终落地 about:blank」形态(猎聘实测:主文档 200 后被站点脚本抹除,
/// 此处用确定性 redirect 落地模拟同一终态)。验证链:
/// ① open 落 blank → `data.site_wipe{streak:1}` + `page_health{reason:"site_wipe"}`
///    (按**请求 URL** 扫网络缓冲给证据,主文档 302 可见 —— 诊断盲区根治);
/// ② 第二次 open → `streak:2, cooldown_triggered:true`(连击熔断);
///    且响应带 `closed_blank_pages`(死页清扫,防空白窗口堆积);
/// ③ 冷却期内同域 open 返 6004 且 `data.cause=="site_wipe"`(专属建议,非频控等待);
/// ④ `close(page_id="all")` 回收后冷却解除(全新 profile=全新身份,恢复路径不被
///    自己的熔断卡住)。
/// host 用 `localhost`(独立冷却键,与 127.0.0.1 / [::1] 用例互不污染)。
#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_site_wipe_governance_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    tokio::spawn(async move {
                        let _ = serve_wipe_redirect(&mut socket).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    let wipe_url = format!("http://localhost:{}/", addr.port());

    // 清场:既跑用例可能残留的冷却/连击全清(本测试自己从零开始)。
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;

    // ① 第一次 open:落 about:blank → 连击 1 + 证据段
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": wipe_url, "reuse": false}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "第一次 open 应成功(动作执行了,落地被抹): {out}");
    assert_eq!(
        v["data"]["final_url"].as_str().unwrap_or_default(),
        "about:blank",
        "302→about:blank 后 final_url 应为 about:blank: {out}"
    );
    let wipe = &v["data"]["site_wipe"];
    assert_eq!(wipe["streak"], 1, "首次抹除连击应为 1: {out}");
    assert_eq!(wipe["cooldown_triggered"], false, "阈值 2,首次不应触发冷却: {out}");
    let health = &v["data"]["page_health"];
    assert_eq!(
        health["reason"].as_str().unwrap_or_default(),
        "site_wipe",
        "按请求 URL 扫网络缓冲应判 site_wipe: {out}"
    );
    assert_eq!(
        health["main_doc_status"], 200,
        "主文档 200 证据应可见(诊断盲区根治): {out}"
    );

    // ② 第二次 open:连击 2 → 触发冷却;并清扫上一页死页
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": wipe_url, "reuse": false}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "第二次 open 应成功: {out}");
    assert_eq!(v["data"]["site_wipe"]["streak"], 2, "连击应累计到 2: {out}");
    assert_eq!(
        v["data"]["site_wipe"]["cooldown_triggered"], true,
        "连击 2 应触发 site_wipe 冷却: {out}"
    );
    assert!(
        v["data"]["closed_blank_pages"].as_i64().unwrap_or(0) >= 1,
        "应清扫上一次抹除遗留的 about:blank 死页: {out}"
    );

    // ③ 冷却期内同域 open 返 6004,cause=site_wipe(专属建议)
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": wipe_url, "reuse": false}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 6004, "抹除冷却中 open 应返 6004: {out}");
    assert_eq!(
        v["data"]["cause"].as_str().unwrap_or_default(),
        "site_wipe",
        "6004 成因应为 site_wipe(建议=换 profile,非冷却等待): {out}"
    );
    let advice = v["data"]["advice"].as_str().unwrap_or_default();
    assert!(
        advice.contains("reuse_existing") || advice.contains("close(page_id=\"all\")"),
        "site_wipe 建议应指路换身份: {advice}"
    );

    // ④ close(all) 回收浏览器 → 冷却解除(恢复路径闭环),新身份连击重来
    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": wipe_url, "reuse": false}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "close(all) 后应放行(冷却已清): {out}");
    assert_eq!(
        v["data"]["site_wipe"]["streak"], 1,
        "新身份连击应重新计数: {out}"
    );

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// 2026-10-14 闲鱼实测:同一商品页被点击/自愈点击重复打开 7~8 次,单任务残留
/// 12 个标签页(`adopt_spawned_pages` 逐个收编成独立 page_id,响应只回第一个,
/// Agent 不知道有堆积)。本用例验证:
/// ① 同 URL 派生页被**去重关闭**(响应 `deduped_pages`),不新增 page_id;
/// ② 不同 URL 派生页照常收编(合法双开不误杀);
/// ③ `spawned_page_ids` 全量回报(不再是只回第一个)。
#[tokio::test]
#[ignore = "需要本机安装 Chrome/Edge(真浏览器冒烟)"]
async fn mcp_web_use_spawned_page_dedup_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let url = spawn_html_server(
        "<html><head><title>dedup</title></head><body>dedup base</body></html>",
    )
    .await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url}))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "open 基页失败: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap_or_default().to_string();

    // ① 同 URL 派生 → 去重关闭(不新增 page_id)
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "eval_js",
            "params": {"expression": format!("window.open({}); 'opened'", serde_json::to_string(&url).unwrap())}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "eval_js 开页失败: {out}");
    let deduped = v["data"]["deduped_pages"].as_array().cloned().unwrap_or_default();
    assert!(
        !deduped.is_empty(),
        "同 URL 派生页应被去重关闭并回报 deduped_pages: {out}"
    );
    assert!(
        v["data"]["spawned_page_ids"].is_null(),
        "去重后不应再收编为 spawned_page_ids: {out}"
    );

    // ② 不同 URL 派生 → 照常收编(合法双开不误杀)
    let other = spawn_html_server(
        "<html><head><title>other</title></head><body>other page</body></html>",
    )
    .await;
    let out = McpWebUseTool
        .execute(json!({
            "action": "control",
            "page_id": page_id,
            "control_action": "eval_js",
            "params": {"expression": format!("window.open({}); 'opened'", serde_json::to_string(&other).unwrap())}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "eval_js 开异页失败: {out}");
    let ids = v["data"]["spawned_page_ids"].as_array().cloned().unwrap_or_default();
    assert!(
        !ids.is_empty(),
        "不同 URL 派生页应被正常收编: {out}"
    );
    // ③ 全量回报:spawned_page_id 与数组首元素一致
    assert_eq!(
        v["data"]["spawned_page_id"].as_str().unwrap_or_default(),
        ids.first().and_then(|x| x.as_str()).unwrap_or_default(),
        "spawned_page_id 应与 spawned_page_ids[0] 一致: {out}"
    );

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// 整页抹除冒烟的测试服务器:200 HTML + 子资源齐全 + 解析完成后
/// `location.replace('about:blank')` 整页抹除(猎聘实测形态的最小复刻;
/// 主文档 200 与子资源进 Network 缓冲作 site_wipe 证据)。
async fn serve_wipe_redirect(socket: &mut TcpStream) -> std::io::Result<()> {
    let mut buf = vec![0_u8; 4096];
    let mut read = 0;
    loop {
        let n = socket.read(&mut buf[read..]).await?;
        if n == 0 {
            break;
        }
        read += n;
        if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") || read == buf.len() {
            break;
        }
    }
    // 请求路径分流:/a.png 等回 1x1 PNG(子资源计数证据),主文档回自抹 HTML。
    let path = String::from_utf8_lossy(&buf[..read]);
    if path.starts_with("GET /w/") {
        // 1x1 PNG
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
            0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00,
            0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
            0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
            0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let mut resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            PNG.len()
        )
        .into_bytes();
        resp.extend_from_slice(PNG);
        socket.write_all(&resp).await?;
    } else {
        let html = r#"<!doctype html><html><head><title>wipe</title>
<script>location.replace('about:blank');</script></head><body>
<h1>loaded content</h1>
<img src="/w/1.png"><img src="/w/2.png"><img src="/w/3.png"><img src="/w/4.png"><img src="/w/5.png">
</body></html>"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            html.len(),
            html
        );
        socket.write_all(resp.as_bytes()).await?;
    }
    socket.shutdown().await
}

/// 隐身脚本**注入时序**真机冒烟（第 177 轮）。
///
/// 这是本轮价值最高的一条改动，此前只有源码顺序断言：把 `inject_stealth` 从
/// `page.goto` 之后移到之前。静态断言只能证明「代码顺序对」，证明不了「首个文档
/// 真的在 document-start 就是处置过的环境」—— 而这恰恰是全部价值所在。
///
/// 判据设计：页面 `<head>` 内联脚本在**解析期**（document-start 之后立即）把读数写进
/// `window.__early`。若隐身脚本仍在 goto 之后注入，本读数会是未处置的真值
/// （`webdriver=true`）；移到 goto 之前则应为处置过的读数。关键在于**取样点必须在页面
/// 脚本里**，事后 eval 取不到时序信息。
#[tokio::test]
#[ignore]
async fn stealth_document_start_timing_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    const HTML: &str = r#"<html><head><script>
window.__early = {
  webdriver: navigator.webdriver,
  plugins: navigator.plugins ? navigator.plugins.length : -1,
  mimeTypes: navigator.mimeTypes ? navigator.mimeTypes.length : -1,
  languages: navigator.languages ? navigator.languages.length : -1
};
</script></head><body>timing</body></html>"#;
    let url = spawn_html_server(HTML).await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "guard": "locked"}))
        .await
        .unwrap();
    let v = data_of(&out);
    if v["code"] == 3001 {
        return; // 本机未安装浏览器
    }
    assert_eq!(v["code"], 0, "open 应成功: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();

    let eval_on = |pid: String, expr: String| async move {
        let out = McpWebUseTool
            .execute(json!({
                "action": "control", "page_id": pid, "control_action": "eval_js",
                "params": {"expression": expr}
            }))
            .await
            .unwrap();
        let v = data_of(&out);
        assert_eq!(v["code"], 0, "eval_js 应成功: {out}");
        v["data"]["result"].as_str().unwrap_or_default().to_string()
    };

    // —— 核心判据：解析期读到的必须是处置过的值 ——
    let early_raw = eval_on(page_id.clone(), "JSON.stringify(window.__early || null)".into()).await;
    let early: serde_json::Value =
        serde_json::from_str(&early_raw).unwrap_or(serde_json::Value::Null);
    assert!(
        !early.is_null(),
        "页面 head 内联脚本未留下 document-start 读数,冒烟判据失效: {early}"
    );
    assert_eq!(
        early["webdriver"], serde_json::json!(false),
        "document-start 读数里 navigator.webdriver 仍为 true —— 隐身脚本没赶在页面脚本之前生效: {early}"
    );
    assert!(
        early["plugins"].as_i64().unwrap_or(0) >= 1,
        "document-start 读数里 plugins 为空: {early}"
    );
    assert!(
        early["mimeTypes"].as_i64().unwrap_or(0) >= 1,
        "document-start 读数里 mimeTypes 为空: {early}"
    );
    assert!(
        early["languages"].as_i64().unwrap_or(0) >= 1,
        "document-start 读数里 languages 为空: {early}"
    );

    // —— 侵入式档升档后,新文档在 document-start 就该是原生化的 ——
    let up = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "set_stealth",
            "params": {"mode": "hardened"}
        }))
        .await
        .unwrap();
    assert_eq!(data_of(&up)["code"], 0, "升档应成功: {up}");
    let nav = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "navigate",
            "params": {"url": url}
        }))
        .await
        .unwrap();
    assert_eq!(data_of(&nav)["code"], 0, "重导航应成功: {nav}");
    let after_raw = eval_on(page_id, "JSON.stringify(window.__early || null)".into()).await;
    let after: serde_json::Value =
        serde_json::from_str(&after_raw).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        after["webdriver"], serde_json::json!(false),
        "升档后新文档解析期读数异常: {after}"
    );
    assert!(
        after["plugins"].as_i64().unwrap_or(0) >= 1,
        "升档后新文档解析期读数异常: {after}"
    );
}

/// **原生形态基线**（第 177 轮真机实测，Chrome 155 本机 headed）。
///
/// 这条不是测我们的代码，而是把「原生 Chrome 到底长什么样」钉成断言，作用是
/// **防止又一次凭想象补字段**。本轮就栽过一次：据「原生 `Plugin` 必带 `mimeTypes`」
/// 给回填替身补了该字段，而真机读数 `'mimeTypes' in navigator.plugins[0]` 为
/// **false** —— WebIDL 的 `Plugin` 接口压根没有这个属性。同批实测还确认
/// `refresh` **只属 `PluginArray`**、不属 `MimeTypeArray`。
///
/// 走 `stealth=off` 的裸环境取基线：注入关闭时读到的就是原生形态。
#[tokio::test]
#[ignore]
async fn stealth_native_shape_baseline_smoke() {
    let _smoke_guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let url = spawn_html_server("<html><head><title>baseline</title></head><body>baseline</body></html>")
        .await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "stealth": "off", "guard": "locked"}))
        .await
        .unwrap();
    let v = data_of(&out);
    if v["code"] == 3001 {
        return;
    }
    assert_eq!(v["code"], 0, "open 应成功: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();
    let out2 = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "eval_js",
            "params": {"expression":
                "JSON.stringify({\
                  plugin_has_mimetypes: navigator.plugins.length ? ('mimeTypes' in navigator.plugins[0]) : null,\
                  plugin_proto: Object.getOwnPropertyNames(Plugin.prototype),\
                  mime_type_array_proto: Object.getOwnPropertyNames(MimeTypeArray.prototype),\
                  plugin_array_proto: Object.getOwnPropertyNames(PluginArray.prototype),\
                  mime_type_proto: Object.getOwnPropertyNames(MimeType.prototype)\
                })"}
        }))
        .await
        .unwrap();
    let raw = data_of(&out2)["data"]["result"].as_str().unwrap_or_default().to_string();
    let shape: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    assert!(!shape.is_null(), "取基线失败: {raw}");

    // `Plugin` 没有 `mimeTypes` —— 本轮曾据相反的臆测补过该字段,真机否掉
    assert_eq!(
        shape["plugin_has_mimetypes"], serde_json::json!(false),
        "WebIDL 的 Plugin 接口没有 mimeTypes 属性(实测基线变化,请重新核实): {shape}"
    );
    assert_eq!(
        shape["plugin_proto"],
        serde_json::json!(["name", "filename", "description", "length", "item", "namedItem", "constructor"]),
        "Plugin.prototype 自有属性基线变化: {shape}"
    );
    // MimeTypeArray 没有 refresh,PluginArray 才有
    assert_eq!(
        shape["mime_type_array_proto"],
        serde_json::json!(["length", "item", "namedItem", "constructor"]),
        "MimeTypeArray.prototype 无 refresh(基线变化): {shape}"
    );
    assert_eq!(
        shape["plugin_array_proto"],
        serde_json::json!(["length", "item", "namedItem", "refresh", "constructor"]),
        "PluginArray.prototype 有 refresh(基线变化): {shape}"
    );
}
