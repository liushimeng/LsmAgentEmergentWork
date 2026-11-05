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
        // 39 号 P2b:页面自由选区 + 选区脚本化验证页。
        "/sel" => (
            "200 OK",
            r#"<!doctype html><html><head><title>选区页</title></head><body>
<p id="pa" style="font-size:16px">甲甲甲甲甲</p>
<p id="pb" style="font-size:16px">乙乙乙乙乙</p>
<input id="q" style="width:200px;height:30px" value="hello">
<dialog id="dlg" open data-lsm-modal><p id="dcontent">弹窗内容</p></dialog>
</body></html>"#
                .to_string(),
        ),
        // P2c:页面查找(Ctrl/Cmd+F)验证页 —— 已知重复词与链接/输入框。
        "/find" => (
            "200 OK",
            r#"<!doctype html><html><head><title>查找页</title></head><body>
<p id="p1" style="font-size:16px">Apple banana apple</p>
<p id="p2" style="font-size:16px">Banana split</p>
<a id="lk" href="/next" style="display:block;width:120px;height:30px">链接</a>
<input id="q" style="width:200px;height:30px" value="">
</body></html>"#
                .to_string(),
        ),
        // P2c:模态 dialog 焦点锁验证页(弹内两按钮在前、弹外一按钮在后:
        // 无焦点锁时 Tab 会从弹内二走到弹外,有锁则回绕弹内一)。
        "/modal" => (
            "200 OK",
            r#"<!doctype html><html><head><title>模态页</title></head><body>
<dialog id="dlg" open data-lsm-modal>
<button id="d1" style="width:120px;height:30px">弹内一</button>
<button id="d2" style="width:120px;height:30px">弹内二</button>
</dialog>
<button id="outside" style="width:120px;height:30px">弹外</button>
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

/// 人工按组合键。
async fn key_mods(mgr: &LsmBrowserManager, pid: &str, k: &str, mods: Mods) -> Value {
    mgr.handle_human_input(pid, HumanInput::Key { key: k.to_string(), mods }).await
}

/// 元素盒(inspect elements 的 rect 为页坐标)。
async fn rect_of(tool: &McpWebLsmBrowserUseTool, pid: &str, selector: &str) -> (f64, f64, f64, f64) {
    let v = exec(
        tool,
        json!({"action": "inspect", "page_id": pid, "info": "elements",
               "params": {"selector": selector, "nth": 0}}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let r = &v["data"]["rect"];
    (
        r["x"].as_f64().unwrap(),
        r["y"].as_f64().unwrap(),
        r["width"].as_f64().unwrap(),
        r["height"].as_f64().unwrap(),
    )
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

// ---------- P2a:文本光标 / 选区 / 编辑语义 ----------

#[tokio::test]
async fn 点击定位光标_中间插入与拖选替换() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (rx, ry, rw, rh) = rect_of(&tool, &pid, "#q").await;
    let (x, y) = (rx + rw / 2.0, ry + rh / 2.0);
    human_click(&mgr, &pid, x, y).await;
    for ch in ["a", "b", "c", "d"] {
        key(&mgr, &pid, ch).await;
    }
    // 点击文本起排附近(盒左 + 4px):光标落 0/1(按 Down 回执的 pos 自适应断言,
    // 不依赖具体字体度量)。
    let down = mgr
        .handle_human_input(
            &pid,
            HumanInput::Down {
                x: rx + 4.0,
                y,
                button: MouseButton::Left,
                mods: Mods::default(),
            },
        )
        .await;
    let pos = down["caret"]["pos"].as_i64().unwrap_or(-1);
    assert!((0..4).contains(&pos), "点击文本起排附近应落前部光标:{down}");
    mgr.handle_human_input(
        &pid,
        HumanInput::Up {
            x: rx + 4.0,
            y,
            button: MouseButton::Left,
            mods: Mods::default(),
        },
    )
    .await;
    key(&mgr, &pid, "X").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    let orig = "abcd";
    let pos = pos.clamp(0, 4) as usize;
    let expect = format!("{}X{}", &orig[..pos], &orig[pos..]);
    assert_eq!(v["data"]["value"], expect, "键入应插入光标处:{v}");

    // 拖选:从按下点拖到最右(选区 = pos..4),键入整体替换选区。
    mgr.handle_human_input(
        &pid,
        HumanInput::Down {
            x: rx + 4.0,
            y,
            button: MouseButton::Left,
            mods: Mods::default(),
        },
    )
    .await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: rx + rw - 1.0, y }).await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: rx + rw - 1.0, y }).await;
    let up = mgr
        .handle_human_input(
            &pid,
            HumanInput::Up {
                x: rx + rw - 1.0,
                y,
                button: MouseButton::Left,
                mods: Mods::default(),
            },
        )
        .await;
    assert_eq!(up["selection"], true, "拖选收尾不应合成点击:{up}");
    key(&mgr, &pid, "Z").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(
        v["data"]["value"],
        format!("{}Z", &expect[..pos]),
        "拖选段应被键入替换:{v}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 双击选词_三击全选_后键入替换() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (rx, ry, rw, rh) = rect_of(&tool, &pid, "#q").await;
    let (x, y) = (rx + rw / 2.0, ry + rh / 2.0);
    human_click(&mgr, &pid, x, y).await;
    for ch in "hello world".chars() {
        key(&mgr, &pid, &ch.to_string()).await;
    }
    // 双击首词内任意位置(文本起排附近)→ 选中 "hello" → 键入替换。
    let x2 = rx + 4.0;
    for _ in 0..2 {
        human_click(&mgr, &pid, x2, y).await;
    }
    key(&mgr, &pid, "X").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "X world", "双击词应被替换:{v}");

    // 三击 → 全选 → 键入整体替换。
    for _ in 0..3 {
        human_click(&mgr, &pid, x2, y).await;
    }
    key(&mgr, &pid, "Y").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "Y", "三击全选应被替换:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn ctrl全选替换与撤销重做() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (x, y) = center_of(&tool, &pid, "#q").await;
    human_click(&mgr, &pid, x, y).await;
    for ch in ["a", "b"] {
        key(&mgr, &pid, ch).await;
    }
    let ctrl = Mods { ctrl: true, ..Default::default() };
    key_mods(&mgr, &pid, "a", ctrl).await;
    key(&mgr, &pid, "Z").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "Z", "Ctrl+A 全选后键入应整体替换:{v}");
    key_mods(&mgr, &pid, "z", ctrl).await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "ab", "Ctrl+Z 撤销应回旧值:{v}");
    key_mods(&mgr, &pid, "y", ctrl).await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "Z", "Ctrl+Y 重做应回新值:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn 选区剪贴板_双击词后剪切粘贴() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_hi(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (rx, ry, rw, rh) = rect_of(&tool, &pid, "#q").await;
    let (x, y) = (rx + rw / 2.0, ry + rh / 2.0);
    human_click(&mgr, &pid, x, y).await;
    for ch in "hello world".chars() {
        key(&mgr, &pid, &ch.to_string()).await;
    }
    // 双击选中 "hello" → ⌘X 剪切选区(非整值清空)。
    for _ in 0..2 {
        human_click(&mgr, &pid, rx + 4.0, y).await;
    }
    let meta = Mods { meta: true, ..Default::default() };
    let cut = key_mods(&mgr, &pid, "x", meta).await;
    assert_eq!(cut["ok"], true, "{cut}");
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], " world", "⌘X 应只剪掉选区:{v}");
    // 剪贴板内容校验(无剪贴板服务的会话软跳过)。
    match lsm_agent::agent::browser::human_input::clipboard::read_text() {
        Ok(text) => assert_eq!(text, "hello", "剪贴板应是选区串:{text}"),
        Err(e) => eprintln!("[skip] 本会话无剪贴板服务:{e}"),
    }
    // ⌘V 在光标处(选区删后 = 0)粘贴回来。
    let paste = key_mods(&mgr, &pid, "v", meta).await;
    assert_eq!(paste["ok"], true, "{paste}");
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "hello world", "粘贴应回到完整值:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

// ---------- 39 号 P2b:页面自由选区与选区脚本化 ----------

async fn open_sel(tool: &McpWebLsmBrowserUseTool, base: &str) -> String {
    let open = exec(
        tool,
        json!({"action": "open", "url": format!("{base}/sel"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let pid = open["data"]["page_id"].as_str().unwrap().to_string();
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    pid
}

async fn open_find(tool: &McpWebLsmBrowserUseTool, base: &str) -> String {
    let open = exec(
        tool,
        json!({"action": "open", "url": format!("{base}/find"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let pid = open["data"]["page_id"].as_str().unwrap().to_string();
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    pid
}

async fn open_modal(tool: &McpWebLsmBrowserUseTool, base: &str) -> String {
    let open = exec(
        tool,
        json!({"action": "open", "url": format!("{base}/modal"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let pid = open["data"]["page_id"].as_str().unwrap().to_string();
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    pid
}

/// 读回某元素 `inspect elements` 的 `state`(聚焦/悬停/选中等)。
async fn elem_state(tool: &McpWebLsmBrowserUseTool, pid: &str, selector: &str) -> Value {
    let v = exec(
        tool,
        json!({"action": "inspect", "page_id": pid, "info": "elements",
               "params": {"selector": selector, "nth": 0}}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    v["data"]["state"].clone()
}

/// eval_js 在 **user 控制态**下被 4004 拒(它是写操作)。人工输入用例需要
/// 读回页面状态时,临时取回 agent 态执行 eval,随后还回 user 态。
async fn eval_js(tool: &McpWebLsmBrowserUseTool, pid: &str, js: &str) -> Value {
    web_control::set_state(Engine::Lsm, pid, ControlState::Agent, None, "test-eval");
    let v = exec(
        tool,
        json!({"action": "control", "page_id": pid, "control_action": "eval_js",
               "params": {"js": js}}),
    )
    .await;
    web_control::set_state(Engine::Lsm, pid, ControlState::User, None, "test-eval-done");
    assert_eq!(v["code"], 0, "{v}");
    // `result` 是 JS 返回值的 JSON 序列化;字符串会带引号,这里解一层。
    let r = v["data"]["result"].clone();
    match r {
        Value::String(s) => s.into(),
        other => other,
    }
}

/// 文本控件的 selectionStart/End 可被页面脚本读取(P2b-1)。
#[tokio::test]
async fn 页面脚本读文本控件选区_点击与双击后() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_sel(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (rx, ry, rw, rh) = rect_of(&tool, &pid, "#q").await;
    let y = ry + rh / 2.0;
    human_click(&mgr, &pid, rx + 6.0, y).await;
    // 键入 "XY" → 光标应在索引 2(初始 value 为 "hello",光标落 0 后插入)。
    for ch in "XY".chars() {
        key(&mgr, &pid, &ch.to_string()).await;
    }
    let v = eval_js(
        &tool,
        &pid,
        "var q=document.getElementById('q'); q.value + '|' + q.selectionStart + '|' + q.selectionEnd",
    )
    .await;
    let s = v.as_str().unwrap_or("");
    assert!(s.contains("XYhello"), "值应为 XYhello:{s}");
    assert!(s.ends_with("|2|2"), "光标应在索引 2:{s}");

    // 双击选词 → selectionStart/End 应为选区两端。
    for _ in 0..2 {
        human_click(&mgr, &pid, rx + 6.0, y).await;
    }
    let v = eval_js(
        &tool,
        &pid,
        "var q=document.getElementById('q'); q.selectionStart + ':' + q.selectionEnd",
    )
    .await;
    let s = v.as_str().unwrap_or("");
    assert!(s.contains(':'), "应读到选区:{s}");
    assert_ne!(s, "0:0", "双击后应有非空选区:{s}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// setSelectionRange 回写宿主:渲染层的光标/选区随之移动(P2b-2)。
#[tokio::test]
async fn 页面脚本setSelectionRange_回写宿主选区() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_sel(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 先聚焦输入框(否则 setSelectionRange 无焦点上下文)。
    let (rx, ry, rw, rh) = rect_of(&tool, &pid, "#q").await;
    human_click(&mgr, &pid, rx + 6.0, ry + rh / 2.0).await;

    let v = eval_js(
        &tool,
        &pid,
        "var q=document.getElementById('q'); q.setSelectionRange(1,3); q.selectionStart + ':' + q.selectionEnd",
    )
    .await;
    assert_eq!(v.as_str().unwrap_or(""), "1:3", "{v}");

    // 键入应替换 [1,3) 选区("hello" 的 "el")→ "hXlo"。
    key(&mgr, &pid, "X").await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#q"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "hXlo", "键入应替换 setSelectionRange 的选区:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// document.getSelection().toString() 覆盖控件内选区与页面自由选区(P2b-1/P2b-3)。
#[tokio::test]
async fn getSelection文本_控件选区与页面自由选区() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_sel(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 控件内:三击全选 → getSelection().toString() 应为整值。
    let (rx, ry, rw, rh) = rect_of(&tool, &pid, "#q").await;
    let y = ry + rh / 2.0;
    for _ in 0..3 {
        human_click(&mgr, &pid, rx + 6.0, y).await;
    }
    let v = eval_js(&tool, &pid, "document.getSelection().toString()").await;
    assert_eq!(v.as_str().unwrap_or(""), "hello", "{v}");

    // 页面自由选区:点 #pa 拖到 #pa 右端。
    let (ax, ay, aw, ah) = rect_of(&tool, &pid, "#pa").await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: ax + 6.0, y: ay + ah / 2.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: ax + aw - 4.0, y: ay + ah / 2.0 }).await;
    let up = mgr.handle_human_input(
        &pid,
        HumanInput::Up { x: ax + aw - 4.0, y: ay + ah / 2.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    assert_eq!(up["page_selection"], true, "拖选应收官:{up}");
    let v = eval_js(&tool, &pid, "document.getSelection().toString()").await;
    let s = v.as_str().unwrap_or("");
    assert!(!s.is_empty(), "页面自由选区文本应非空:{v}");
    assert!(s.chars().all(|c| c == '甲'), "应只含甲:{s}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// 无控件焦点时 Ctrl+C 复制页面自由选区(P2b-4)。
#[tokio::test]
async fn 页面选区复制_无焦点时走页面选区() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_sel(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (ax, ay, aw, ah) = rect_of(&tool, &pid, "#pb").await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: ax + 6.0, y: ay + ah / 2.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: ax + aw - 4.0, y: ay + ah / 2.0 }).await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Up { x: ax + aw - 4.0, y: ay + ah / 2.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    // 清空焦点(点空白处)→ Ctrl+C 应复制页面选区。
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: 5.0, y: 5.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Up { x: 5.0, y: 5.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    let meta = Mods { meta: true, ..Default::default() };
    let out = key_mods(&mgr, &pid, "c", meta).await;
    assert_eq!(out["ok"], true, "页面选区复制应成功:{out}");
    assert_eq!(out["source"], "page_selection", "{out}");
    match lsm_agent::agent::browser::human_input::clipboard::read_text() {
        Ok(text) => assert!(text.chars().all(|c| c == '乙'), "剪贴板应只含乙:{text}"),
        Err(e) => eprintln!("[skip] 本会话无剪贴板服务:{e}"),
    }

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// 模态 dialog 遮罩点击关闭(P2b-4)。
#[tokio::test]
async fn 点击遮罩关闭模态弹窗() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_sel(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 点 dialog 之外的区域(页面顶端空白)→ 应关闭。
    let up = mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: 700.0, y: 5.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    let up = mgr.handle_human_input(
        &pid,
        HumanInput::Up { x: 700.0, y: 5.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    assert_eq!(up["dialog_closed"], true, "点遮罩应关闭弹窗:{up}");
    let v = eval_js(
        &tool,
        &pid,
        "document.getElementById('dlg').hasAttribute('open')",
    )
    .await;
    assert_eq!(v.as_str().unwrap_or(""), "false", "dialog 应已关闭:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

// ---------- P2c:页面查找 / 光标形状 / 模态焦点锁 ----------

/// 页面查找(Ctrl/Cmd+F):开 → 输入 query 命中 → Enter 切换 → Esc 关闭。
#[tokio::test]
async fn 页面查找_输入命中与切换() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_find(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 开启(查看器命令同一入口;隐藏模式下无查找栏像素,经回执断言)。
    let out = mgr.toggle_find(&pid).await;
    assert_eq!(out["active"], true, "{out}");

    // 输入 "apple"(大小写不敏感;p1 出现两次)。
    for c in "apple".chars() {
        let r = key(&mgr, &pid, &c.to_string()).await;
        assert_eq!(r["query_changed"], true, "{r}");
    }
    // 首个 Enter:切到下一个并回报总数。
    let step = key(&mgr, &pid, "Enter").await;
    assert_eq!(step["total"], 2, "apple 应命中 2 次:{step}");
    assert_eq!(step["current"], 1, "{step}");
    // 再按 Enter 回绕到 0。
    let step = key(&mgr, &pid, "Enter").await;
    assert_eq!(step["current"], 0, "回绕:{step}");
    // Shift+Enter 反向。
    let step = key_mods(&mgr, &pid, "Enter", Mods { shift: true, ..Default::default() }).await;
    assert_eq!(step["current"], 1, "反向:{step}");

    // Esc 关闭;关闭后 Enter 不再产生查找语义(回到页面原生语义)。
    let closed = key(&mgr, &pid, "Escape").await;
    assert_eq!(closed["closed"], true, "{closed}");
    let again = key(&mgr, &pid, "Enter").await;
    assert!(again.get("current").is_none(), "关闭后不再查找:{again}");
    assert!(again.get("outcome").is_some(), "关闭后 Enter 走页面原生语义:{again}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// 光标形状跟随:悬停链接 = 手型,悬停文本框 = I 型,空白 = 箭头。
#[tokio::test]
async fn 光标形状_悬停目标决定手型与文本型() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_find(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (lx, ly) = center_of(&tool, &pid, "#lk").await;
    let out = mgr
        .handle_human_input(&pid, HumanInput::Move { x: lx, y: ly })
        .await;
    assert_eq!(out["cursor"], "pointer", "链接应为手型:{out}");

    let (tx, ty) = center_of(&tool, &pid, "#q").await;
    let out = mgr
        .handle_human_input(&pid, HumanInput::Move { x: tx, y: ty })
        .await;
    assert_eq!(out["cursor"], "text", "文本框应为 I 型:{out}");

    // 空白处(无命中)回落箭头。
    let out = mgr
        .handle_human_input(&pid, HumanInput::Move { x: 700.0, y: 5.0 })
        .await;
    assert_eq!(out["cursor"], "default", "空白应为箭头:{out}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// 模态 dialog 焦点锁:焦点在弹内时,Tab/Shift+Tab 只在弹内按钮间循环。
///
/// 页面顺序 = 弹内一 → 弹内二 → 弹外:**无焦点锁时** Tab 会从弹内二走到弹外;
/// 有锁则回绕弹内一。弹内点击不关闭弹窗(与点遮罩区分)。
#[tokio::test]
async fn 模态dialog焦点锁_Tab不出弹窗() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_modal(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 点弹内二 → 聚焦弹内二(弹内点击不触发遮罩关闭)。
    let (d2x, d2y) = center_of(&tool, &pid, "#d2").await;
    human_click(&mgr, &pid, d2x, d2y).await;
    assert_eq!(elem_state(&tool, &pid, "#d2").await["focused"], true, "点击后弹内二应聚焦");

    // Tab:有锁 → 回绕弹内一;无锁会走到弹外。
    let tab = key(&mgr, &pid, "Tab").await;
    assert_eq!(tab["moved"], true, "{tab}");
    assert_eq!(elem_state(&tool, &pid, "#d1").await["focused"], true, "Tab 应回绕弹内一:{tab}");
    assert_eq!(elem_state(&tool, &pid, "#outside").await["focused"], false, "焦点不应出弹窗");

    // Shift+Tab 反向回弹内二;仍不出弹窗。
    let back = key_mods(&mgr, &pid, "Tab", Mods { shift: true, ..Default::default() }).await;
    assert_eq!(back["moved"], true, "{back}");
    assert_eq!(elem_state(&tool, &pid, "#d2").await["focused"], true, "Shift+Tab 应回弹内二");
    assert_eq!(elem_state(&tool, &pid, "#outside").await["focused"], false, "焦点仍不应出弹窗");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

