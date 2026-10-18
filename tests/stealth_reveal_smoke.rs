//! 第 178 轮:反检测的**服务端侧取证**与**回填替身真机差分**冒烟。
//!
//! 前四轮反检测的判据全在页面侧（`navigator.*` / `screen.*` 的读数），而站点风控有
//! 一半在服务端。本文件第一次把「我们发出去的请求长什么样」变成可断言的数据
//! （本地取证服务器记录 `User-Agent` / `Sec-CH-UA` 与请求序号），并在真机上把
//! **插件回填产物**与**同机原生形态**逐字段对账。
//!
//! 默认 `#[ignore]`（需本机安装 Chrome/Edge），显式运行：
//! `cargo test --test stealth_reveal_smoke -- --ignored --nocapture`
//!
//! 独立成文件（而非并入 `web_use_smoke.rs`）：两者各自一个测试二进制 = 各自一个进程
//! = 各自一个 `BrowserManager` 单例，天然串行隔离；`web_use_smoke.rs` 也已接近源码行数
//! 上限，不宜再长。

use lsm_agent::agent::tools::mcp_web_use::McpWebUseTool;
use lsm_agent::agent::tools::Tool;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 同二进制内的真浏览器用例必须串行（`BrowserManager` 是进程内单例）。
static SMOKE_LOCK: Mutex<()> = Mutex::new(());

fn data_of(out: &str) -> Value {
    serde_json::from_str(out).unwrap_or(Value::Null)
}

/// 取证服务器：记录每个 `GET /` 的 `User-Agent` / `Sec-CH-UA` 与请求序号。
///
/// 返回 `(url, 请求计数, 头部行日志)` —— 日志按请求顺序两行一组
/// （UA 行、Sec-CH-UA 行），便于断言「第 N 次请求长什么样」。
async fn spawn_reveal_server(
    html: &'static str,
) -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let (c, l) = (counter.clone(), log.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _)) => {
                    let (c2, l2) = (c.clone(), l.clone());
                    tokio::spawn(async move {
                        let mut buf = vec![0_u8; 8192];
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
                        let head = String::from_utf8_lossy(&buf[..read]).to_string();
                        if head.starts_with("GET / ") {
                            c2.fetch_add(1, Ordering::SeqCst);
                            let pick = |name: &str| {
                                head.lines()
                                    .find(|line| line.to_ascii_lowercase().starts_with(name))
                                    .unwrap_or("(absent)")
                                    .trim()
                                    .to_string()
                            };
                            l2.lock().unwrap_or_else(|e| e.into_inner()).extend([
                                pick("user-agent:"),
                                pick("sec-ch-ua:"),
                            ]);
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
    (format!("http://{addr}/"), counter, log)
}

/// **首文档盲区根治**（第 178 轮，本轮最高价值）。
///
/// 修复前：`new_page_impl` 把目标 URL 交给 `Target.createTarget`，而 chromiumoxide
/// 会等它**加载完成**才返回 —— 首个真实文档跑在隐身 / UA / 屏幕几何处置**之前**
/// （站点风控 SDK 恰在文档最早期采指纹并上报，第 177 轮「注册早于 goto」对它无效），
/// 随后 `page.goto` 又把同一 URL **再加载一次**。真机实测：目标 URL 被请求 **2** 次，
/// `req#1` 的 UA 原样带 `HeadlessChrome`；而同一新会话对同一 URL 连续加载两次，
/// 本身就是异常导航序列。
///
/// 修复后：以 `about:blank` 起页（空载起点、无站点脚本、无网络请求），真实 URL 只由
/// `goto` 加载**一次**，且这一次自始就在处置之下。
///
/// 走 **hidden 档**是刻意的：只有无头档的 UA 才带自动化令牌，headed 档下
/// 「首个请求 UA 干净」恒真、测不出东西。
#[tokio::test]
#[ignore]
async fn stealth_first_document_single_load_smoke() {
    let _guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let (url, count, log) = spawn_reveal_server(
        "<html><head><title>firstdoc</title></head><body>firstdoc</body></html>",
    )
    .await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "mode": "hidden", "guard": "locked"}))
        .await
        .unwrap();
    let v = data_of(&out);
    if v["code"] == 3001 {
        return; // 本机未安装浏览器
    }
    assert_eq!(v["code"], 0, "hidden 档 open 应成功: {out}");
    assert_eq!(v["data"]["mode"], json!("hidden"), "应为无头档: {out}");
    let page_id = v["data"]["page_id"].as_str().unwrap().to_string();

    let hits = count.load(Ordering::SeqCst);
    let headers = log.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(
        hits, 1,
        "目标 URL 必须只被加载一次(修复前 createTarget 一次 + goto 一次 = 2);\
         实际 {hits} 次,取证日志: {headers:?}"
    );
    let first_ua = headers.first().cloned().unwrap_or_default();
    assert!(
        !first_ua.contains("Headless"),
        "唯一一次真实请求的 UA 不得带 HeadlessChrome 令牌 —— 它必须发生在 UA 去令牌之后\
         (修复前 req#1 是裸奔的): {first_ua}"
    );
    assert!(first_ua.contains("Chrome/"), "UA 应仍是正常 Chrome: {first_ua}");
    // 品牌表顺带锁死:真机实测无头档的 Sec-CH-UA 不含自动化品牌(令牌只在 UA 串里)
    let first_ch = headers.get(1).cloned().unwrap_or_default();
    assert!(
        !first_ch.to_ascii_lowercase().contains("headless"),
        "Sec-CH-UA 不得残留自动化品牌: {first_ch}"
    );

    // 页面侧读数同样得干净(document-start 时序由 stealth_document_start_timing_smoke 覆盖)
    let out2 = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": page_id, "control_action": "eval_js",
            "params": {"expression":
                "JSON.stringify({wd: navigator.webdriver, ua: navigator.userAgent, plugins: navigator.plugins.length})"}
        }))
        .await
        .unwrap();
    let probe = data_of(&out2)["data"]["result"].as_str().unwrap_or_default().to_string();
    let p: Value = serde_json::from_str(&probe).unwrap_or(Value::Null);
    assert_eq!(p["wd"], json!(false), "页面侧 webdriver 应已处置: {probe}");
    assert!(
        !p["ua"].as_str().unwrap_or("").contains("Headless"),
        "页面侧 UA 应已去令牌: {probe}"
    );

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// **回填替身与同机原生的逐字段差分**（第 178 轮，闭合 31 §九.2）。
///
/// 31 号文遗留：「回填替身的形态只有 Node 垫片背书，没有真机对照」—— 因为本机原生
/// 就有 5 条 PDF 插件，`length === 0` 的触发条件**根本不会命中**，回填路径是完全的
/// 未走路径。而垫片只能验证「代码按我设想的方式工作」，验证不了「我的设想符合原生」
/// （31 §五 正是栽在这一点的教训）。
///
/// 本测试在真机上**强制触发**回填：先把 `Navigator.prototype.plugins/mimeTypes`
/// 覆写成空数组、清掉幂等闸标记，再注入保真档脚本文本；然后把**回填产物**与
/// **同一次运行读到的原生形态**逐字段对账。基准取自同一引擎的实时读数（不是硬编码
/// 常量），故它对「设想不符合原生」这类问题同样有效 —— 本轮就靠它查出了四处中间
/// 原型缺 own `constructor`（`Object.create(原型)` 只继承该键，一行
/// `getOwnPropertyNames` 即可看出「不是原生原型」）。
#[tokio::test]
#[ignore]
async fn stealth_plugin_backfill_real_machine_diff_smoke() {
    /// 形态读数单（字段顺序即对账清单）。
    const SHAPE_JS: &str = r#"JSON.stringify((() => {
      const p = navigator.plugins, m = navigator.mimeTypes;
      const own = (o) => Object.getOwnPropertyNames(o);
      const el = p && p.length ? p[0] : null;
      return {
        tag: p ? Object.prototype.toString.call(p) : 'none',
        inst: (p && typeof PluginArray === 'function') ? (p instanceof PluginArray) : 'n/a',
        ctor_is_pluginarray: (p && typeof PluginArray === 'function') ? (p.constructor === PluginArray) : 'n/a',
        proto_own: p ? own(Object.getPrototypeOf(p)) : 'n/a',
        proto_own_syms: p ? Object.getOwnPropertySymbols(Object.getPrototypeOf(p)).map(String) : 'n/a',
        instance_own: p ? own(p) : 'n/a',
        iter_len: (p && p[Symbol.iterator]) ? [...p].length : 'no-iter',
        iter_self: (p && p[Symbol.iterator]) ? (typeof p[Symbol.iterator]()[Symbol.iterator]) : 'n/a',
        named: (p && p.namedItem) ? (p.namedItem('PDF Viewer') ? 'hit' : 'miss') : 'n/a',
        named_is_index0: (p && p.namedItem && p.length) ? (p.namedItem('PDF Viewer') === p[0]) : 'n/a',
        item0_is_index0: (p && p.item) ? (p.item(0) === p[0]) : 'n/a',
        len: p ? p.length : 'n/a',
        mlen: m ? m.length : 'n/a',
        el_own: el ? own(el) : 'n/a',
        el_proto_own: el ? own(Object.getPrototypeOf(el)) : 'n/a',
        el_ctor: el ? (typeof Plugin === 'function' ? el.constructor === Plugin : 'n/a') : 'n/a',
        el_iter: el ? ((el[Symbol.iterator] && typeof el[Symbol.iterator]) || 'none') : 'n/a',
        el_has_mimetypes: el ? ('mimeTypes' in el) : 'n/a',
        m_tag: m ? Object.prototype.toString.call(m) : 'n/a',
        m_proto_own: m ? own(Object.getPrototypeOf(m)) : 'n/a',
        m_el_own: (m && m.length) ? own(m[0]) : 'n/a',
        m_el_proto_own: (m && m.length) ? own(Object.getPrototypeOf(m[0])) : 'n/a',
        m_el_ctor: (m && m.length) ? (typeof MimeType === 'function' ? (m[0].constructor === MimeType) : 'n/a') : 'n/a',
        m_enabled_plugin_is_p0: (m && m.length && p && p.length) ? (m[0].enabledPlugin === p[0]) : 'n/a',
        p_names: p ? Array.prototype.map.call(p, (x) => x.name) : 'n/a'
      };
    })())"#;

    let _guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    let url =
        spawn_html_server("<html><head><title>backfill</title></head><body>b</body></html>").await;
    let out = McpWebUseTool
        .execute(json!({"action": "open", "url": url, "stealth": "off", "guard": "locked"}))
        .await
        .unwrap();
    let v = data_of(&out);
    if v["code"] == 3001 {
        return; // 本机未安装浏览器
    }
    assert_eq!(v["code"], 0, "open 应成功: {out}");
    // 裸环境是本测试的前提:单例浏览器若已在别的配置下启动,本用例无法成立(信封已验证)
    if v["data"]["stealth"]["mode"] != json!("off") {
        let _ = McpWebUseTool
            .execute(json!({"action": "close", "page_id": "all"}))
            .await;
        return;
    }
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

    // ① 原生基线(同一引擎、同一页面、注入之前)
    let native_raw = eval(SHAPE_JS.to_string()).await;
    let native: Value = serde_json::from_str(&native_raw).unwrap_or(Value::Null);
    assert!(!native.is_null(), "取原生基线失败: {native_raw}");
    assert_eq!(
        native["len"], json!(5),
        "本机原生插件数不是 5,形态基线前提变化: {native}"
    );

    // ② 强制触发回填:置空 → 清幂等闸 → 注入保真档脚本文本
    // 第 179 轮:幂等闸标记挂在符号命名空间对象上(不再是 window 字符串键)。
    let ns = lsm_agent::agent::browser_ns::inject_ns();
    let force = format!(
        "(() => {{ Object.defineProperty(Navigator.prototype, 'plugins', {{ get: () => [], configurable: true }});\
          Object.defineProperty(Navigator.prototype, 'mimeTypes', {{ get: () => [], configurable: true }});\
          try {{ const NS = window[Symbol.for('{ns}')]; if (NS) delete NS.StealthRev; }} catch (e) {{}}\
          return JSON.stringify({{ len: navigator.plugins.length, mlen: navigator.mimeTypes.length }}); }})()"
    );
    let forced: Value = serde_json::from_str(&eval(force).await).unwrap_or(Value::Null);
    assert_eq!(
        (forced["len"].clone(), forced["mlen"].clone()),
        (json!(0), json!(0)),
        "强制置空后 plugins/mimeTypes 应为空(否则回填分支不会触发): {forced}"
    );
    let script = lsm_agent::agent::browser_ns::stealth_js_for(
        lsm_agent::agent::stealth::StealthMode::Fidelity,
    )
    .expect("保真档必须产出脚本");
    let _ = eval(script).await;

    // ③ 回填产物 vs 原生基线逐字段对账
    let filled_raw = eval(SHAPE_JS.to_string()).await;
    let filled: Value = serde_json::from_str(&filled_raw).unwrap_or(Value::Null);
    assert!(!filled.is_null(), "取回填形态失败: {filled_raw}");
    assert_eq!(filled["len"], json!(5), "回填应产出 5 条 PDF 插件: {filled}");
    const FIELDS: &[&str] = &[
        "tag",
        "inst",
        "ctor_is_pluginarray",
        "proto_own",
        "proto_own_syms",
        "instance_own",
        "iter_len",
        "iter_self",
        "named",
        "named_is_index0",
        "item0_is_index0",
        "mlen",
        "el_own",
        "el_proto_own",
        "el_ctor",
        "el_iter",
        "el_has_mimetypes",
        "m_tag",
        "m_proto_own",
        "m_el_own",
        "m_el_proto_own",
        "m_el_ctor",
        "m_enabled_plugin_is_p0",
        "p_names",
    ];
    let mut diffs: Vec<String> = Vec::new();
    for field in FIELDS {
        if native.get(field) != filled.get(field) {
            diffs.push(format!(
                "{field}: 原生={} 回填={}",
                native.get(field).cloned().unwrap_or(Value::Null),
                filled.get(field).cloned().unwrap_or(Value::Null)
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "回填替身与原生形态不符(形状错误的回填比不回填更危险):\n  {}",
        diffs.join("\n  ")
    );

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// **派生页首文档早期武装**（第 180 轮）。
///
/// 33 §六.1 的遗留:页面自己派生的新标签页(`<a target=_blank>` / `window.open` /
/// `control(new_tab)`)此前要等 adopt(触发动作收尾)才被反检测处置。本用例在
/// 无头档(hidden)点击 `target=_blank` 派生子页,对子页做「提交后武装」对账。
///
/// **判别性断言是落地后的 `sw == 1920x1080`**:chromiumoxide init 链对派生 target
/// 的 `setDeviceMetricsOverride` 不带 screen 字段,而主分支的 adopt 序列**从不**补
/// `apply_screen_sync_override` —— 主分支上派生页落地后 `screen` 恒为 `800x600`
/// 无头默认值(`outer > screen` 物理不可能硬判据),本修复(watcher 提交时刻武装 +
/// adopt 二道防线)后读到窗口尺寸。
///
/// **提交门(实测钉死,docs/MCP_Web_Use/34 §三)**:chromiumoxide 把页面句柄的全部
/// 命令压在 target init 链之后,而 init 链要等 provisional 导航**提交**才完成
/// (400ms/1500ms 双延迟实测:命令响应与提交同步)。因此子页 `<head>` 内联脚本
/// (解析期 = 提交后即刻)读到的 `sw`/`ua` 仍是原生值 —— 这是经 chromiumoxide
/// 通道的物理下限,断言只覆盖 `parse.wd`(launch 旗标浏览器级生效,恒 false);
/// `parse.ua`/`parse.sw` 采集留作诊断数据不参与断言。提交后的一切(子资源请求/
/// XHR/信标/后续文档)全部在处置之下:落地后 `ua` 干净、`sw` 已同步、符号命名空间
/// 在场。
///
/// 子页响应刻意延迟 400ms:真实站点首文档的网络时延常态,给 watcher 与 adopt
/// 的武装留出与真实世界同量级的窗口。
#[tokio::test]
#[ignore]
async fn stealth_spawned_page_early_armed_smoke() {
    let _guard = SMOKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("LAEW_BROWSER_WATCHDOG_EXE", env!("CARGO_BIN_EXE_laew"));
    // 诊断期:打印 watcher/adopt 的时序日志(正式断言不依赖它)。
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter("lsm_agent=debug")
        .try_init();
    // 两路由服务器:`/` 父页(带 target=_blank 锚点),`/child` 延迟 400ms 的子页。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
                        let head = String::from_utf8_lossy(&buf[..read]).to_string();
                        let is_child = head.starts_with("GET /child");
                        if is_child {
                            // 首文档网络时延建模:给早期武装留出真实站点量级的窗口。
                            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        }
                        let html: &str = if is_child {
                            // 解析期取样:document-start 处置是否就位,只有这一次机会。
                            r#"<html><head><script>
window.__parseProbe = {
  wd: navigator.webdriver,
  ua: navigator.userAgent,
  sw: screen.width + 'x' + screen.height
};</script><title>child</title></head><body>child</body></html>"#
                        } else {
                            r#"<html><head><title>parent</title></head>
<body><a id="sp" href="/child" target="_blank">go</a></body></html>"#
                        };
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
    let base = format!("http://{addr}/");

    // ① 无头档开父页(只有无头档才有 UA 令牌与 screen=800x600 默认值可对账)。
    // 显式 stealth=hardened:单例浏览器的档位是进程级粘滞的(open(stealth=off) 的
    // 其它用例先跑时,本用例会被继承 Off 而全部断言落空 —— 套件内顺序无关性)。
    let out = McpWebUseTool
        .execute(json!({
            "action": "open", "url": base, "mode": "hidden", "guard": "locked",
            "stealth": "hardened"
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    if v["code"] == 3001 {
        return; // 本机未安装浏览器
    }
    assert_eq!(v["code"], 0, "hidden 档 open 应成功: {out}");
    let parent_id = v["data"]["page_id"].as_str().unwrap().to_string();

    // ② 点击 target=_blank 锚点派生子页(CDP Input 是可信输入,带用户激活,
    //    不会被弹窗拦截)。响应携带 adopt 回传的 spawned_page_id。
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": parent_id, "control_action": "click",
            "params": {"selector": "#sp"}
        }))
        .await
        .unwrap();
    let v = data_of(&out);
    assert_eq!(v["code"], 0, "click 应成功: {out}");
    let mut child_id = v["data"]["spawned_page_id"].as_str().map(str::to_string);
    // 兜底:偶尔 click 的 CDP 响应先于 target 创建返回,adopt 巡检扑空 —— `list`
    // 只见**已收编**页面,恢复手段是再触发一次会巡检的动作(dispatch_event 在
    // `action_may_spawn_page` 名单内,对 body 派发自定义事件无害),有界 5s。
    if child_id.is_none() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while child_id.is_none() && std::time::Instant::now() < deadline {
            let out = McpWebUseTool
                .execute(json!({
                    "action": "control", "page_id": parent_id, "control_action": "dispatch_event",
                    "params": {"selector": "body", "type": "laew-smoke-probe"}
                }))
                .await
                .unwrap();
            let v = data_of(&out);
            child_id = v["data"]["spawned_page_id"].as_str().map(str::to_string);
        }
    }
    let child_id = child_id.expect("派生子页应被 adopt 收编回传 page_id");

    // ③ 子页内取「解析期快照 + 当前读数 + 注入面对账」
    let out = McpWebUseTool
        .execute(json!({
            "action": "control", "page_id": child_id, "control_action": "eval_js",
            "params": {"expression":
                "JSON.stringify({parse: window.__parseProbe, wd: navigator.webdriver, ua: navigator.userAgent, sw: screen.width + 'x' + screen.height, syms: Object.getOwnPropertySymbols(window).length})"}
        }))
        .await
        .unwrap();
    let out = data_of(&out);
    assert_eq!(out["code"], 0, "eval_js 应成功: {out}");
    let probe = out["data"]["result"].as_str().unwrap_or_default().to_string();
    let p: Value = serde_json::from_str(&probe).unwrap_or(Value::Null);
    assert!(!p.is_null(), "取派生页读数失败: {probe}");

    // 解析期(parse)只有 webdriver 参与断言:launch 旗标 `--disable-blink-features=
    // AutomationControlled` 浏览器级生效,派生页同样覆盖;`sw`/`ua` 受提交门约束
    // (见用例文档注释),采集留作诊断数据。
    assert_eq!(
        p["parse"]["wd"],
        json!(false),
        "派生页解析期 webdriver 应已处置(launch 旗标浏览器级生效): {probe}"
    );
    // 判别性断言(落地后):screen 必须已同步 —— 主分支上 adopt 从不补 screen_sync,
    // 派生页落地后恒读 800x600 无头默认值 → outer>screen 物理不可能硬判据。
    assert_eq!(
        p["sw"].as_str().unwrap_or(""),
        "1920x1080",
        "派生页落地后 screen 必须已同步(主分支为 800x600): {probe}"
    );
    assert!(
        !p["ua"].as_str().unwrap_or("").contains("Headless"),
        "派生页落地后 UA 不得带 HeadlessChrome 令牌: {probe}"
    );
    assert!(
        p["syms"].as_u64().unwrap_or(0) >= 1,
        "派生页应有符号命名空间(隐身脚本已注入): {probe}"
    );

    let _ = McpWebUseTool
        .execute(json!({"action": "close", "page_id": "all"}))
        .await;
}

/// 简版 HTML 服务器（本文件独立于 `web_use_smoke.rs`，故自带夹具）。
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
