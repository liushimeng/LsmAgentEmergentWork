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
    assert_eq!(v["data"]["window"]["width"], 1280, "窗口宽应回显请求值: {out}");
    assert_eq!(v["data"]["window"]["height"], 800, "窗口高应回显请求值: {out}");
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
    assert_eq!(v["data"]["window"]["width"], 1100, "回读窗口宽: {out}");

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

}
