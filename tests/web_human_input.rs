//! 人类键鼠交互端到端(39 号设计文档):查看器输入协议 → 宿主
//! `human_input` 管线 → 复用 Agent 侧交互语义。
//!
//! 进程内 mock HTTP 服务 + 工具层开页/断言;人工输入直接调
//! `LsmBrowserManager::handle_human_input`(与查看器命令通道同一入口),
//! 不启动查看器窗口、不依赖外网。

use lsm_agent::agent::browser::human_input::{HumanInput, Mods, MouseButton};
use lsm_agent::agent::browser::LsmBrowserManager;
use lsm_agent::agent::tools::mcp_web_lsm_browser_use::McpWebLsmBrowserUseTool;
use lsm_agent::agent::tools::Tool;
use lsm_agent::agent::web_control::{self, ControlState, Engine};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ---------- mock 站点 ----------

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
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    match path {
        "/hi" => (
            "200 OK",
            r#"<!doctype html><html><head><title>交互页</title></head><body>
<button id="hit-btn" style="width:120px;height:40px">点我</button>
<span id="out"></span>
<span id="out2"></span>
<span id="out3"></span>
<a id="nav" href="/next" style="display:block;width:100px;height:30px">下一页</a>
<form id="f" action="/search" method="get">
<input id="q" name="q" style="width:200px;height:30px" value="">
<input id="cb" type="checkbox" name="ok">
<select id="dd"><option value="a">甲</option><option value="b">乙</option></select>
<input id="rg" type="range" name="r" min="0" max="100" value="10" style="width:200px">
</form>
<div id="d1" draggable="true" style="width:80px;height:40px;background:#eee">拖我</div>
<div id="d2" style="width:80px;height:40px;background:#ddd">放下</div>
<a id="tab2" href="/next">T2</a><button id="tab3">T3</button>
<div style="height:3000px"></div>
<script>
document.getElementById('hit-btn').addEventListener('click', function(){
  document.getElementById('out').textContent = 'HIT';
});
document.addEventListener('keydown', function(e){
  document.getElementById('out2').textContent = 'K:' + e.key + ':' + e.keyCode;
});
document.getElementById('d1').addEventListener('dragstart', function(){
  document.getElementById('out3').textContent = 'DS';
});
document.getElementById('d2').addEventListener('drop', function(){
  document.getElementById('out3').textContent += '|DROP';
});
</script>
</body></html>"#
                .to_string(),
        ),
        "/next" => (
            "200 OK",
            "<html><head><title>下一页</title></head><body><h1>NEXT</h1></body></html>".to_string(),
        ),
        p if p.starts_with("/search") => (
            "200 OK",
            format!(
                "<html><head><title>结果-{}</title></head><body><h1>OK</h1></body></html>",
                p.split_once('?').map(|(_, q)| q).unwrap_or("")
            ),
        ),
        _ => ("404 Not Found", "<html><body>404</body></html>".to_string()),
    }
}

// ---------- 夹具 ----------

async fn exec(tool: &McpWebLsmBrowserUseTool, args: Value) -> Value {
    let text = tool.execute(args).await.unwrap();
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("返回不是 JSON:{e}\n{text}"))
}

fn allow_loopback() {
    std::env::set_var("LAEW_ALLOW_PRIVATE_ENDPOINT", "1");
    std::env::set_var("LAEW_SITE_RISK_PERSIST", "off");
    std::env::set_var("LAEW_WEB_MAX_TABS", "32");
}

async fn open_hi(tool: &McpWebLsmBrowserUseTool, base: &str) -> String {
    let open = exec(
        tool,
        json!({"action": "open", "url": format!("{base}/hi"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let pid = open["data"]["page_id"].as_str().unwrap().to_string();
    // 人工输入只在用户控制态放行(控制权状态机铁律)。
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    pid
}

/// 元素盒中心(inspect elements 的 rect 为页坐标;初始 scroll=0,与视口一致)。
async fn center_of(tool: &McpWebLsmBrowserUseTool, pid: &str, selector: &str) -> (f64, f64) {
    let v = exec(
        tool,
        json!({"action": "inspect", "page_id": pid, "info": "elements",
               "params": {"selector": selector, "nth": 0}}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let r = &v["data"]["rect"];
    (
        r["x"].as_f64().unwrap() + r["width"].as_f64().unwrap() / 2.0,
        r["y"].as_f64().unwrap() + r["height"].as_f64().unwrap() / 2.0,
    )
}

/// 人工左键单击(Down+Up 同点)。
async fn human_click(mgr: &LsmBrowserManager, pid: &str, x: f64, y: f64) -> Value {
    mgr.handle_human_input(
        pid,
        HumanInput::Down { x, y, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(
        pid,
        HumanInput::Up { x, y, button: MouseButton::Left, mods: Mods::default() },
    )
    .await
}

/// 人工按一个键(无修饰键)。
async fn key(mgr: &LsmBrowserManager, pid: &str, k: &str) -> Value {
    mgr.handle_human_input(
        pid,
        HumanInput::Key { key: k.to_string(), mods: Mods::default() },
    )
    .await
}

// ---------- 用例 ----------

#[tokio::test]
async fn 人工点击触发页面监听与控件默认行为() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // JS click 监听写回 DOM。
    let (x, y) = center_of(&tool, &pid, "#hit-btn").await;
    let out = human_click(&mgr, &pid, x, y).await;
    assert_eq!(out["clicked"], true, "{out}");
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#out"}}),
    )
    .await;
    assert_eq!(v["data"]["text"], "HIT", "监听器应被人工点击触发:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 人工点击链接真实导航() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (x, y) = center_of(&tool, &pid, "#nav").await;
    let out = human_click(&mgr, &pid, x, y).await;
    assert_eq!(out["clicked"], true, "{out}");
    assert!(
        out["outcome"]["final_url"].as_str().unwrap_or("").contains("/next"),
        "链接点击应触发导航:{out}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 人工键入并回车提交表单() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 点击输入框聚焦 → 逐字符键入 → 回车提交。
    let (x, y) = center_of(&tool, &pid, "#q").await;
    human_click(&mgr, &pid, x, y).await;
    for ch in ["l", "a", "e", "w"] {
        key(&mgr, &pid, ch).await;
    }
    let out = key(&mgr, &pid, "Enter").await;
    assert!(
        out["outcome"]["submitted"].as_bool().unwrap_or(false),
        "回车应提交表单:{out}"
    );
    assert!(
        out["outcome"]["final_url"].as_str().unwrap_or("").contains("q=laew"),
        "提交应携带人工键入的查询串:{out}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 人工点击checkbox切换选中态() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (x, y) = center_of(&tool, &pid, "#cb").await;
    let out = human_click(&mgr, &pid, x, y).await;
    assert_eq!(out["outcome"]["toggled"], true, "{out}");
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#cb"}}),
    )
    .await;
    assert_eq!(v["data"]["state"]["checked"], true, "checkbox 应被人工勾选:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 人工滚轮滚动视口与range点击设值() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // range:点轨道最右端 → 值吸附到 max=100(先于滚动,坐标即页坐标)。
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#rg"}}),
    )
    .await;
    let r = &v["data"]["rect"];
    let x = r["x"].as_f64().unwrap() + r["width"].as_f64().unwrap() - 0.5;
    let y = r["y"].as_f64().unwrap() + r["height"].as_f64().unwrap() / 2.0;
    human_click(&mgr, &pid, x, y).await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#rg"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "100", "点击轨道右端应把滑块设到最大值:{v}");

    // 滚轮向下(dy<0)→ scroll_y 增加。
    let out = mgr
        .handle_human_input(&pid, HumanInput::Wheel { x: 400.0, y: 300.0, dx: 0.0, dy: -2.0 })
        .await;
    assert!(out["scrollY"].as_f64().unwrap_or(0.0) > 0.0, "{out}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn Tab焦点循环与Esc收起下拉面板() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 点击 select 展开面板。
    let (x, y) = center_of(&tool, &pid, "#dd").await;
    human_click(&mgr, &pid, x, y).await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#dd"}}),
    )
    .await;
    assert_eq!(v["data"]["state"]["select_open"], true, "点击应展开下拉:{v}");

    // Esc 收起面板。
    key(&mgr, &pid, "Escape").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#dd"}}),
    )
    .await;
    assert_eq!(v["data"]["state"]["select_open"], false, "Esc 应收起面板:{v}");

    // Tab:焦点从 select 移到下一个可聚焦元素(range)。
    let out = key(&mgr, &pid, "Tab").await;
    assert_eq!(out["moved"], true, "{out}");
    assert!(
        out["focused"].as_str().unwrap_or("").contains("rg"),
        "Tab 应把焦点移到 range(文档序):{out}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn agent态人工输入被闸门忽略() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let open = exec(
        &tool,
        json!({"action": "open", "url": format!("{base}/hi"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let pid = open["data"]["page_id"].as_str().unwrap().to_string();
    // 新页默认 agent 态(无记录 = Agent 控制状态)。
    let mgr = LsmBrowserManager::global();
    let out = mgr
        .handle_human_input(
            &pid,
            HumanInput::Down { x: 60.0, y: 20.0, button: MouseButton::Left, mods: Mods::default() },
        )
        .await;
    assert_eq!(out["ignored"], true, "agent 态输入应被忽略:{out}");
    assert_eq!(out["reason"], "agent_control");

    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

// ---------- 阶段二:事件载荷 / HTML5 drag / 控件级剪贴板 ----------

#[tokio::test]
async fn 键盘事件载荷可被页面读取() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 点击输入框聚焦,按 'a':document 级 keydown 监听应读到 e.key='a' e.keyCode=65。
    let (x, y) = center_of(&tool, &pid, "#q").await;
    human_click(&mgr, &pid, x, y).await;
    key(&mgr, &pid, "a").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#out2"}}),
    )
    .await;
    assert_eq!(v["data"]["text"], "K:a:65", "keydown 应携带 key/keyCode 载荷:{v}");

    // ArrowLeft(命名键):e.key='ArrowLeft' keyCode=37。不用 Enter —— 它会
    // 触发表单提交换页,页面元素随导航消失(提交语义另有用例覆盖)。
    key(&mgr, &pid, "ArrowLeft").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#out2"}}),
    )
    .await;
    assert_eq!(v["data"]["text"], "K:ArrowLeft:37", "命名键载荷应与 DOM 一致:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 拖拽事件序列_dragstart_drop_dragend() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (x1, y1) = center_of(&tool, &pid, "#d1").await;
    let (x2, y2) = center_of(&tool, &pid, "#d2").await;
    // 按下 #d1 → 移到 #d2(超阈值触发 dragstart + dragover)→ 在 #d2 抬起(drop)。
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: x1, y: y1, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: x2, y: y2 }).await;
    let out = mgr
        .handle_human_input(
            &pid,
            HumanInput::Up { x: x2, y: y2, button: MouseButton::Left, mods: Mods::default() },
        )
        .await;
    assert_eq!(out["drag"], true, "{out}");
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#out3"}}),
    )
    .await;
    assert_eq!(
        v["data"]["text"], "DS|DROP",
        "dragstart 与 drop 监听都应收到事件:{v}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 控件级复制粘贴_整值近似() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 聚焦 #q 并键入 abc → ⌘C 复制 → 清空(⌘X)→ ⌘V 粘贴回来。
    let (x, y) = center_of(&tool, &pid, "#q").await;
    human_click(&mgr, &pid, x, y).await;
    for ch in ["a", "b", "c"] {
        key(&mgr, &pid, ch).await;
    }
    let copied = mgr
        .handle_human_input(
            &pid,
            HumanInput::Key { key: "c".to_string(), mods: Mods { meta: true, ..Default::default() } },
        )
        .await;
    assert_eq!(copied["ok"], true, "复制应成功:{copied}");
    // 剪贴板内容校验(无剪贴板服务的会话软跳过)。
    match lsm_agent::agent::browser::human_input::clipboard::read_text() {
        Ok(text) => assert!(text.contains("abc"), "剪贴板应含整值:{text}"),
        Err(e) => eprintln!("[skip] 本会话无剪贴板服务:{e}"),
    }
    // ⌘X 剪切清空。
    let cut = mgr
        .handle_human_input(
            &pid,
            HumanInput::Key { key: "x".to_string(), mods: Mods { meta: true, ..Default::default() } },
        )
        .await;
    assert_eq!(cut["ok"], true, "{cut}");
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "", "剪切后应为空:{v}");
    // ⌘V 粘贴回来。
    let paste = mgr
        .handle_human_input(
            &pid,
            HumanInput::Key { key: "v".to_string(), mods: Mods { meta: true, ..Default::default() } },
        )
        .await;
    assert_eq!(paste["value_len"], 3, "{paste}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}
