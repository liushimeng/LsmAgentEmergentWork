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
        // P2d:pointer 事件 / hover 兼容事件 / wheel / datalist 联想 /
        // select 多选 / number 步进 验证页。
        "/p2d" => (
            "200 OK",
            r#"<!doctype html><html><head><title>P2d页</title></head><body>
<div id="wall" style="width:100px;height:40px;background:#ccc">滚我</div>
<div id="pa" style="width:100px;height:40px;background:#eee">甲区</div>
<div id="pb" style="width:100px;height:40px;background:#ddd">乙区</div>
<div style="position:absolute;left:600px;top:0;width:360px">
<span id="out"></span><span id="out2"></span><span id="out3"></span>
<span id="out4"></span><span id="out5"></span><span id="out6"></span>
</div>
<input id="ac" list="fruits" style="width:220px;height:30px" value="">
<datalist id="fruits">
<option value="Apple" label="苹果"></option>
<option value="Banana" label="香蕉"></option>
<option value="app-store"></option>
<option value="Cherry" label="樱桃"></option>
</datalist>
<select id="ms" multiple name="ms" style="width:120px;height:104px">
<option value="a">甲</option><option value="b">乙</option>
<option value="c">丙</option><option value="d">丁</option>
</select>
<input id="num" type="number" min="0" max="10" step="2" style="width:120px;height:30px">
<div style="height:2400px"></div>
<script>
var l1=[],l2=[],l3=[],l4=[],l5=[],l6=[];
function put(el,a){a.push(el);document.getElementById(a===l1?'out':a===l2?'out2':a===l3?'out3':a===l4?'out4':a===l5?'out5':'out6').textContent=a.join('|');}
document.addEventListener('pointerdown',function(e){put('PD:'+e.pointerType+':'+e.pointerId,l1);});
document.getElementById('wall').addEventListener('pointerdown',function(e){put('WALL:'+e.pointerType,l1);});
document.addEventListener('pointerup',function(e){put('PU:'+e.pointerType,l1);});
document.getElementById('wall').addEventListener('click',function(){put('CLK',l1);});
['pa','pb'].forEach(function(id){
  document.getElementById(id).addEventListener('pointerover',function(){put('POV-'+id[1].toUpperCase(),l2);});
  document.getElementById(id).addEventListener('mouseover',function(){put('OV-'+id[1].toUpperCase(),l2);});
  document.getElementById(id).addEventListener('pointerout',function(){put('POUT-'+id[1].toUpperCase(),l2);});
  document.getElementById(id).addEventListener('mouseout',function(){put('OUT-'+id[1].toUpperCase(),l2);});
});
document.getElementById('wall').addEventListener('wheel',function(e){e.preventDefault();put('WHEEL',l3);});
document.getElementById('ac').addEventListener('input',function(){put('IN:'+this.value,l4);});
document.getElementById('ac').addEventListener('change',function(){put('CH:'+this.value,l4);});
document.getElementById('ms').addEventListener('change',function(){put('MS',l5);});
document.getElementById('num').addEventListener('input',function(){put('NUM:'+this.value,l6);});
</script>
</body></html>"#
                .to_string(),
        ),
        // P2e:边界事件(enter/leave)· pointer capture · select 读值桥 ·
        // number 微调箭头点击 · 跨元素选区整片高亮 验证页。
        "/p2e" => (
            "200 OK",
            r#"<!doctype html><html><head><title>P2e页</title></head><body>
<div id="wrap" style="width:300px">
<div id="boxA" style="width:100px;height:40px;background:#eee">甲</div>
<div id="boxB" style="width:100px;height:40px;background:#ddd">乙</div>
</div>
<div id="drag" style="width:120px;height:40px;background:#ccc">拖柄</div>
<div id="far" style="width:200px;height:60px;background:#bbb">远处</div>
<select id="ss"><option value="a">甲</option><option value="b" selected>乙</option><option value="c">丙</option></select>
<input id="nn" type="number" min="0" max="10" step="2" style="width:120px;height:30px" value="4">
<p id="pa" style="font-size:16px">甲甲甲甲甲</p>
<p id="pm" style="font-size:16px">中中中中中</p>
<p id="pc" style="font-size:16px">丙丙丙丙丙</p>
<div style="position:absolute;left:700px;top:0;width:400px">
<span id="out"></span><span id="out2"></span><span id="out3"></span>
</div>
<script>
var logs = {h: [], cap: [], num: []};
function flush(key){
  var el = document.getElementById(key === 'h' ? 'out' : key === 'cap' ? 'out2' : 'out3');
  el.textContent = logs[key].join('|');
}
function on(id, ev, key, tag){
  document.getElementById(id).addEventListener(ev, function(){ logs[key].push(tag); flush(key); });
}
// 边界事件:甲/乙与公共祖先 #wrap 各挂 enter/leave(公共祖先在
// 兄弟切换时不应触发 —— 不冒泡语义的根基)。
on('boxA','pointerenter','h','PE-A'); on('boxA','mouseenter','h','ME-A');
on('boxA','pointerleave','h','PL-A'); on('boxA','mouseleave','h','ML-A');
on('boxB','pointerenter','h','PE-B'); on('boxB','mouseenter','h','ME-B');
on('boxB','pointerleave','h','PL-B'); on('boxB','mouseleave','h','ML-B');
on('wrap','pointerenter','h','PE-W'); on('wrap','mouseenter','h','ME-W');
on('wrap','pointerleave','h','PL-W'); on('wrap','mouseleave','h','ML-W');
// pointer capture:d3-drag 形态 —— pointerdown 里捕获自己,只监听捕获元素。
document.getElementById('drag').addEventListener('pointerdown', function(e){
  e.target.setPointerCapture(1); logs.cap.push('DOWN'); flush('cap');
});
document.getElementById('drag').addEventListener('pointermove', function(){ logs.cap.push('MOVE'); flush('cap'); });
document.getElementById('drag').addEventListener('pointerup', function(){ logs.cap.push('UP'); flush('cap'); });
document.getElementById('drag').addEventListener('lostpointercapture', function(){ logs.cap.push('LOST'); flush('cap'); });
document.getElementById('drag').addEventListener('gotpointercapture', function(){ logs.cap.push('GOT'); flush('cap'); });
document.getElementById('drag').addEventListener('click', function(){ logs.cap.push('CLK'); flush('cap'); });
document.getElementById('nn').addEventListener('input', function(){ logs.num.push('N:' + this.value); flush('num'); });
</script>
</body></html>"#
                .to_string(),
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

/// 系统剪贴板互斥:pbcopy/pbpaste 是进程外全局资源,并行用例会互相
/// 抢走对方刚写入的内容(高负载下偶发「粘贴串值」)。涉及剪贴板的
/// 用例串行执行,其余用例保持并行。
static CLIPBOARD_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let _clip_guard = CLIPBOARD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
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
    let _clip_guard = CLIPBOARD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
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
    let _clip_guard = CLIPBOARD_MUTEX.lock().unwrap_or_else(|p| p.into_inner());
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


// ---------- P2d 用例(指针事件 / hover 兼容 / wheel / datalist / 多选 / number) ----------

/// 打开 P2d 验证页并转用户控制态。
async fn open_p2d(tool: &McpWebLsmBrowserUseTool, base: &str) -> String {
    let open = exec(
        tool,
        json!({"action": "open", "url": format!("{base}/p2d"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let pid = open["data"]["page_id"].as_str().unwrap().to_string();
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    pid
}

/// 带修饰键的人工左键单击。
async fn human_click_mods(
    mgr: &LsmBrowserManager,
    pid: &str,
    x: f64,
    y: f64,
    mods: Mods,
) -> Value {
    mgr.handle_human_input(pid, HumanInput::Down { x, y, button: MouseButton::Left, mods }).await;
    mgr.handle_human_input(pid, HumanInput::Up { x, y, button: MouseButton::Left, mods }).await
}

fn mods_of(ctrl: bool, shift: bool) -> Mods {
    Mods { shift, ctrl, alt: false, meta: false }
}

/// 元素文本(inspect elements 的 text)。
async fn text_of(tool: &McpWebLsmBrowserUseTool, pid: &str, selector: &str) -> String {
    let v = exec(
        tool,
        json!({"action": "inspect", "page_id": pid, "info": "elements",
               "params": {"selector": selector}}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    v["data"]["text"].as_str().unwrap_or_default().to_string()
}

#[tokio::test]
async fn 人工点击派发pointer事件序列() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2d(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 完整点击序列:pointerdown 先于 mousedown 等价物、pointerup 先于 click,
    // pointerType/pointerId 可读(只挂 pointer 监听的图表库语义)。
    let (x, y) = center_of(&tool, &pid, "#wall").await;
    let out = human_click(&mgr, &pid, x, y).await;
    assert_eq!(out["clicked"], true, "{out}");
    let log = text_of(&tool, &pid, "#out").await;
    assert_eq!(log, "PD:mouse:1|WALL:mouse|PU:mouse|CLK", "pointer 兼容事件序列应按 Chrome 顺序派发:{log}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn hover切换派发mouseover与pointer兼容事件() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2d(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 移入甲区:pointerover + mouseover;再移到乙区(P2e 起 Chrome 完整序:
    // pointerout → pointerleave → pointerover → pointerenter →
    // mouseout → mouseleave → mouseover → mouseenter,enter/leave 本页无监听):
    // 可见序 = 甲 out → 乙 over(pointer 先行)→ 甲 mouseout → 乙 mouseover。
    let (ax, ay) = center_of(&tool, &pid, "#pa").await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: ax, y: ay }).await;
    let (bx, by) = center_of(&tool, &pid, "#pb").await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: bx, y: by }).await;
    let log = text_of(&tool, &pid, "#out2").await;
    assert_eq!(
        log, "POV-A|OV-A|POUT-A|POV-B|OUT-A|OV-B",
        "hover 切换应按 Chrome 完整序派发(out/over 先于 mouseout/mouseover):{log}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn wheel事件preventDefault阻止原生滚动() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2d(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // #wall 的 wheel 监听 preventDefault → 页面不滚,但事件已送达。
    // 并行测试挤 CPU 时 dispatch 往返可能超 800ms 上限(超时按未拦截放行,
    // 这是设计内的 fail-open),重试至多 5 次拿稳定结论。
    let (x, y) = center_of(&tool, &pid, "#wall").await;
    let mut out = json!({});
    for _ in 0..5 {
        out = mgr
            .handle_human_input(&pid, HumanInput::Wheel { x, y, dx: 0.0, dy: -2.0 })
            .await;
        if out["mode"] == "js_prevented" {
            break;
        }
    }
    assert_eq!(out["mode"], "js_prevented", "preventDefault 应拦截原生滚动:{out}");
    let log = text_of(&tool, &pid, "#out3").await;
    assert_eq!(log, "WHEEL", "wheel 事件应送达页面监听器:{log}");

    // 无监听区域照常滚动(视口滚动 scrollY 增加)。
    let (nx, ny) = center_of(&tool, &pid, "#num").await;
    let out = mgr
        .handle_human_input(&pid, HumanInput::Wheel { x: nx, y: ny, dx: 0.0, dy: -2.0 })
        .await;
    assert!(out["scrollY"].as_f64().unwrap_or(0.0) > 0.0, "未拦截的滚轮应照常滚动视口:{out}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn datalist联想_过滤高亮与应用() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2d(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 聚焦带 list 的 input → 展开全量建议(datalist option 获得几何)。
    let (x, y) = center_of(&tool, &pid, "#ac").await;
    human_click(&mgr, &pid, x, y).await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "datalist#fruits option", "nth": 0}}),
    )
    .await;
    assert_eq!(v["code"], 0, "面板展开时命中项应有几何:{v}");
    assert_eq!(v["data"]["rect"]["laid_out"], true, "面板展开时命中项应有真实布局:{v}");
    assert!(v["data"]["rect"]["height"].as_f64().unwrap_or(0.0) > 0.0, "{v}");

    // 键入 app → 只剩 Apple/app-store 两行;↓ 高亮首行;Enter 应用。
    for ch in ["a", "p", "p"] {
        key(&mgr, &pid, ch).await;
    }
    key(&mgr, &pid, "ArrowDown").await;
    let out = key(&mgr, &pid, "Enter").await;
    assert_eq!(out["datalist_applied"], true, "Enter 应应用高亮建议:{out}");
    let val = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#ac"}}),
    )
    .await;
    assert_eq!(val["data"]["value"], "Apple", "建议值应写入输入框:{val}");
    let log = text_of(&tool, &pid, "#out4").await;
    assert!(
        log.contains("IN:Apple") && log.contains("CH:Apple"),
        "应用建议应派发 input+change:{log}"
    );

    // 面板已关:再次聚焦重开(全量),Esc 只关面板不清值。
    human_click(&mgr, &pid, x, y).await;
    let out = key(&mgr, &pid, "Escape").await;
    assert_eq!(out["datalist"], "closed", "Esc 应只关闭联想面板:{out}");
    let closed = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "datalist#fruits option", "nth": 0}}),
    )
    .await;
    assert_eq!(closed["data"]["rect"]["laid_out"], false, "面板关闭后命中项不应再有布局:{closed}");

    // 外部点击 light dismiss:重开后点空白处 → 面板收起。
    human_click(&mgr, &pid, x, y).await;
    let (px, py) = center_of(&tool, &pid, "#pa").await;
    human_click(&mgr, &pid, px, py).await;
    let closed = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "datalist#fruits option", "nth": 0}}),
    )
    .await;
    assert_eq!(closed["data"]["rect"]["laid_out"], false, "点击面板外应 light dismiss:{closed}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn select多选_Ctrl翻转与Shift范围() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2d(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 列表形态 option 行的几何(nth=1 → 「乙」)。
    let opt_rect = |n: i64| {
        let tool = &tool;
        let pid = pid.clone();
        async move {
            let v = exec(
                tool,
                json!({"action": "inspect", "page_id": pid, "info": "elements",
                       "params": {"selector": "#ms option", "nth": n}}),
            )
            .await;
            assert_eq!(v["code"], 0, "{v}");
            let r = &v["data"]["rect"];
            (
                r["x"].as_f64().unwrap() + r["width"].as_f64().unwrap() / 2.0,
                r["y"].as_f64().unwrap() + r["height"].as_f64().unwrap() / 2.0,
            )
        }
    };
    let (a_x, a_y) = opt_rect(0).await;
    let (b_x, b_y) = opt_rect(1).await;
    let (c_x, c_y) = opt_rect(2).await;

    // 普通点击「乙」→ 独占 {b}。
    human_click(&mgr, &pid, b_x, b_y).await;
    // Ctrl+点击「丙」→ 翻转加入 → {b,c}(.value = 首个选中 = b)。
    human_click_mods(&mgr, &pid, c_x, c_y, mods_of(true, false)).await;
    let log = text_of(&tool, &pid, "#out5").await;
    assert_eq!(log, "MS|MS", "前两次点击(普通+Ctrl)各应派发一次 change:{log}");

    // Shift+点击「甲」→ 锚点(丙)到甲的范围 → {a,b,c}。
    human_click_mods(&mgr, &pid, a_x, a_y, mods_of(false, true)).await;
    // Ctrl+点击「甲」→ 翻转移除 → {b,c}。
    human_click_mods(&mgr, &pid, a_x, a_y, mods_of(true, false)).await;
    // 选中态断言:乙/丙 selected、甲/丁 未选。
    for (n, want) in [(0, false), (1, true), (2, true), (3, false)] {
        let v = exec(
            &tool,
            json!({"action": "inspect", "page_id": &pid, "info": "elements",
                   "params": {"selector": "#ms option", "nth": n}}),
        )
        .await;
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(
            v["data"]["state"]["selected"], want,
            "第 {n} 个 option 的选中态应为 {want}:{v}"
        );
    }

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

#[tokio::test]
async fn number输入方向键步进() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2d(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 聚焦 number(step=2,min=0,max=10):↑→2、↑→4、↓→2。
    let (x, y) = center_of(&tool, &pid, "#num").await;
    human_click(&mgr, &pid, x, y).await;
    key(&mgr, &pid, "ArrowUp").await;
    key(&mgr, &pid, "ArrowUp").await;
    key(&mgr, &pid, "ArrowDown").await;
    let log = text_of(&tool, &pid, "#out6").await;
    assert_eq!(log, "NUM:2|NUM:4|NUM:2", "方向键应按 step 步进:{log}");
    // 连按十次 ↑:钳制在 max=10。
    for _ in 0..10 {
        key(&mgr, &pid, "ArrowUp").await;
    }
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#num"}}),
    )
    .await;
    assert_eq!(v["data"]["value"], "10", "步进应钳制在 max:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

// ---------- P2e 用例(边界事件 / pointer capture / select 读值 / number 微调 / 选区整片) ----------

/// 打开 P2e 验证页并转用户控制态。
async fn open_p2e(tool: &McpWebLsmBrowserUseTool, base: &str) -> String {
    let open = exec(
        tool,
        json!({"action": "open", "url": format!("{base}/p2e"), "mode": "hidden", "reuse": false}),
    )
    .await;
    assert_eq!(open["code"], 0, "{open}");
    let pid = open["data"]["page_id"].as_str().unwrap().to_string();
    web_control::set_state(Engine::Lsm, &pid, ControlState::User, None, "test");
    pid
}

/// hover 切换派发完整边界事件序:兄弟元素间切换时,目标各自 enter/leave,
/// **公共祖先 #wrap 的 enter/leave 不触发**(同一祖先内移动 —— 菜单语义根基)。
#[tokio::test]
async fn hover切换派发enter与leave边界事件_公共祖先不触发() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2e(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // 甲 → 乙:pointerout/leave(甲)→ over/enter(乙)→ mouse out/leave/over/enter。
    let (ax, ay) = center_of(&tool, &pid, "#boxA").await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: ax, y: ay }).await;
    let (bx, by) = center_of(&tool, &pid, "#boxB").await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: bx, y: by }).await;
    let log = text_of(&tool, &pid, "#out").await;
    // 进入甲(自空白):pointer 轮先完(enter 甲 → enter wrap,自内向外),
    // mouse 轮后完(同序);切换甲→乙:甲 leave → 乙 enter(pointer 轮)、
    // 甲 mouseleave → 乙 mouseenter(mouse 轮);wrap 是公共祖先全程不触发。
    assert_eq!(
        log, "PE-A|PE-W|ME-A|ME-W|PL-A|PE-B|ML-A|ME-B",
        "边界事件应按 Chrome 序派发且公共祖先不触发:{log}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// pointer capture 真语义(d3-drag 形态):pointerdown 里 setPointerCapture 后,
/// 拖到别的元素上 pointermove/pointerup 仍派发到**捕获元素**;隐式释放时
/// lostpointercapture;捕获目标 == 按下目标时 click 照常合成且 pointerup 不双发。
#[tokio::test]
async fn pointerCapture重定向移动与抬起事件() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2e(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (dx_, dy_) = center_of(&tool, &pid, "#drag").await;
    let (fx, fy) = center_of(&tool, &pid, "#far").await;
    // 第一段:按下(监听器捕获自己)→ 拖到远处 → 抬起 = 拖拽收尾
    //(位移超阈值,不合成 click;pointerup 重定向到捕获元素后隐式释放)。
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: dx_, y: dy_, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    // 捕获建立后的移动要越过 100ms 节流:等一拍再发(节流窗口按到达时刻)。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: fx, y: fy }).await;
    let up = mgr
        .handle_human_input(
            &pid,
            HumanInput::Up { x: fx, y: fy, button: MouseButton::Left, mods: Mods::default() },
        )
        .await;
    assert_eq!(up["clicked"], false, "拖离按下点(捕获目标 == 按下目标)按拖拽收尾:{up}");
    // 隐式释放由日志佐证(下方 LOST);drag_started 分支的回执不带 capture_released。
    let log = text_of(&tool, &pid, "#out2").await;
    // GOT(pointerdown 后建立)→ MOVE(拖到 #far 上仍落捕获元素)→
    // UP(重定向)→ LOST(隐式释放);位移大无 CLK。
    assert_eq!(
        log, "DOWN|GOT|MOVE|UP|LOST",
        "捕获应重定向 pointer 事件并在抬起时隐式释放:{log}"
    );

    // 释放后再移动:pointermove 回落命中元素(#far),不再进捕获元素。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: fx, y: fy + 10.0 }).await;
    let log = text_of(&tool, &pid, "#out2").await;
    assert_eq!(log, "DOWN|GOT|MOVE|UP|LOST", "释放后不应再有捕获元素的事件:{log}");

    // 第二段:原地按下+抬起(捕获目标 == 按下目标、位移小)→ click 照常合成,
    // 且 pointerup 只派发一次(捕获路径已发,click_inner 不再补发)。
    let click = human_click(&mgr, &pid, dx_, dy_).await;
    assert_eq!(click["clicked"], true, "原地点击应照常合成:{click}");
    let log = text_of(&tool, &pid, "#out2").await;
    assert_eq!(
        log, "DOWN|GOT|MOVE|UP|LOST|DOWN|GOT|UP|LOST|CLK",
        "原地点击:UP 单次 + CLK 合成(不双发 pointerup):{log}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// select 读值桥:页面脚本 `select.value/selectedIndex` 读到真实选中值
/// (HTML 初始与人工点选后两个时点)。
#[tokio::test]
async fn select读值桥_页面脚本可读选中值() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2e(&tool, &base).await;

    // 初始(HTML selected=乙,value="b"):value=b / selectedIndex=1。
    let v = eval_js(
        &tool,
        &pid,
        "var s=document.getElementById('ss'); s.value + ':' + s.selectedIndex",
    )
    .await;
    assert_eq!(v.as_str().unwrap_or(""), "b:1", "初始选中值应可读:{v}");

    // 人工点开下拉并点「丙」→ value=3 / selectedIndex=2。
    let mgr = LsmBrowserManager::global();
    let (sx, sy) = center_of(&tool, &pid, "#ss").await;
    human_click(&mgr, &pid, sx, sy).await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "elements",
               "params": {"selector": "#ss option", "nth": 2}}),
    )
    .await;
    assert_eq!(v["code"], 0, "面板展开后 option 应有几何:{v}");
    let r = &v["data"]["rect"];
    let (ox, oy) = (
        r["x"].as_f64().unwrap() + r["width"].as_f64().unwrap() / 2.0,
        r["y"].as_f64().unwrap() + r["height"].as_f64().unwrap() / 2.0,
    );
    human_click(&mgr, &pid, ox, oy).await;
    let v = eval_js(
        &tool,
        &pid,
        "var s=document.getElementById('ss'); s.value + ':' + s.selectedIndex",
    )
    .await;
    assert_eq!(v.as_str().unwrap_or(""), "c:2", "人工点选后读值应更新:{v}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// number 微调箭头点击步进:右上 +step、右下 −step(Chrome 语义)。
#[tokio::test]
async fn number微调箭头点击步进() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2e(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    let (x, y, w, h) = rect_of(&tool, &pid, "#nn").await;
    // 右上角(箭头区宽 13px):value 4 → 6(step=2)。
    let up = human_click(&mgr, &pid, x + w - 6.0, y + h * 0.25).await;
    assert_eq!(up["spinner"], 1, "右上应命中递增箭头:{up}");
    assert_eq!(up["value"], "6", "点击递增箭头应 +step:{up}");
    // 右下角:6 → 4。
    let dn = human_click(&mgr, &pid, x + w - 6.0, y + h * 0.75).await;
    assert_eq!(dn["spinner"], -1, "右下应命中递减箭头:{dn}");
    assert_eq!(dn["value"], "4", "点击递减箭头应 −step:{dn}");
    // 文本区点击仍走光标语义(不步进)。
    let mid = human_click(&mgr, &pid, x + w / 2.0, y + h / 2.0).await;
    assert!(mid.get("spinner").is_none(), "文本区点击不应步进:{mid}");
    // input 事件每个步进派发一次。
    let log = text_of(&tool, &pid, "#out3").await;
    assert_eq!(log, "N:6|N:4", "步进应派发 input 事件:{log}");

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}

/// 跨元素拖选:中间元素整片点亮(page_health.page_sel.marked_elements
/// 覆盖首 + 中 + 尾)。
#[tokio::test]
async fn 跨元素拖选中间元素整片点亮() {
    allow_loopback();
    let base = start_site().await;
    let tool = McpWebLsmBrowserUseTool;
    let pid = open_p2e(&tool, &base).await;
    let mgr = LsmBrowserManager::global();

    // #pa 左端按下 → 拖到 #pc 右端(越过 #pm)。
    let (ax, ay, _, ah) = rect_of(&tool, &pid, "#pa").await;
    let (cx, cy, cw, ch) = rect_of(&tool, &pid, "#pc").await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Down { x: ax + 6.0, y: ay + ah / 2.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    mgr.handle_human_input(&pid, HumanInput::Move { x: cx + cw - 6.0, y: cy + ch / 2.0 }).await;
    mgr.handle_human_input(
        &pid,
        HumanInput::Up { x: cx + cw - 6.0, y: cy + ch / 2.0, button: MouseButton::Left, mods: Mods::default() },
    )
    .await;
    let v = exec(
        &tool,
        json!({"action": "inspect", "page_id": &pid, "info": "page_health"}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let ps = &v["data"]["page_sel"];
    assert_eq!(ps["active"], true, "拖选后应有页面选区:{ps}");
    let marked = ps["marked_elements"].as_i64().unwrap_or(0);
    assert!(
        marked >= 3,
        "首 + 中间(#pm)+ 尾都应点亮(实际 {marked}):{ps}"
    );

    web_control::clear(Engine::Lsm, &pid);
    exec(&tool, json!({"action": "close", "page_id": &pid})).await;
}
